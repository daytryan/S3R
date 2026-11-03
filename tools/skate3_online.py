"""Build the data for Skate 3's online game modes from the player's own disc.

py -3 tools/skate3_online.py --disc DISC [--dlc PACKAGES] --assets INST/assets --out DIR [--install]

Inputs
- DISC is the extracted Skate 3 disc: default.xex, data/big/{db,scene,miscboot,fedata,fetexture,fedynamic}.big
  and data/content/missions.big; data/audio/post.big for the NIS's AUDIO_POST streams (vgmstream: SKATE_VGMSTREAM).
- INST/assets is the prepared install. Only these files are read from it:
  - private/challenge-map/runtime/challengemap.json ["language"] for the strings;
  - private/stock/skater-collections.json for the movies' fonts;
  - private/challenge-map/online/online.json: Domination's Location (a global locator online_challenge_map resolved).
- PACKAGES (optional) is a folder of the Skate 3 DLC STFS packages (files starting CON/LIVE/PIRS). skate and a mode
  with `dlc=True` (MODES) also export the packages' rows: their challengebanks, challenge_local_data, missions, intro
  scenes, global locators and strings, read like the disc's (DlcDb).
- The output is a pure function of those files. --install copies DIR/<mode>/ into INST/assets/private/skate3-online/.

Per mode it writes DIR/<mode>/ (--modes, default all) for its consumer in crates/skate-game/src/:
- skate (graph `skate`): skate3_skate_game.rs, described below;
- 1up (`1up`): skate3_one_up.rs;
- spot_battle (`spot_battle`) and hall_of_meat (`hall_of_meat`): skate3_uniform.rs;
- race (`race_shared`, + RaceHeats / Legs / Gates and the start / outro NIS definitions): skate3_race.rs;
- contest (`contest_shared`, street + vert, + ContestRounds with each round's intro and VisualIndicators):
  skate3_contest.rs;
- domination (`domination`, + Location and the Objectives' ScoringBoundary volumes from challengebanks):
  skate3_domination.rs.
The other modes write the same files: rules.json rows (title, online, intro, objectives count, challenge_type and
the mode's named fields, decoded), scripts/<graph>.lua, tutorial.json + scene/camera, strings.json.

S.K.A.T.E.'s indicators.json: each row's VisualIndicators (skate.lua AddIndicator: the neon arrows over its spots) for
skate2_arrows.rs, shapes and styles keyed "skate3:<type or style key>", textures by asset path, in indicator/.

S.K.A.T.E.'s rules.json
- template: the CentralMessageHALID* fields, mapped to string IDs.
- max_letters: ABDListItemLimit.
- time_limit: TimeLimit, from the challenge_local_data_framework template (5d4c / 316a / default chain).
- rows: one entry per S.K.A.T.E. row:
  - title, and online (challengebanks AvailableOnline);
  - time_limit;
  - intro: IntroPresentationEvents type 5;
  - spawn: TurnBasedAttemptSpawnPoint, as matrix and slots;
  - wait: TurnBasedWaitingLocation, as matrix and slots (the locator's children <name>::*_<n>);
  - start_volume (TurnBasedStartVolume), spot_volumes (SpotVolumes) and boundary (ChallengeBoundary): world boxes;
  - indicators (VisualIndicators locators).
  All of these come from missions.big missions/<row>/cSim_Global.xsf.

scripts/skate.lua
- The graph chain's handlers (skate, turn_based_leader, turn_based, default), decompiled with unluac
  (SKATE_JAVA / SKATE_UNLUAC). This is the rules' source.
- Optional: skipped when there is no Java.

tutorial.json + scene/camera/<scene>.json
- The intro flythroughs, read from miscboot.big scene_000.bin and scene.big ice_000.bin / ICELite.bin.
- They are evaluated by tools/pipeline/skate2port/old_tutorial.py's ICE section (exec'd unchanged, read-only),
  using Skate 3's full ICE parameter table.
- That table is emulated from the disc default.xex: the registration that files EYE_X .. CONTAINS_SUBTAKE, which
  is TU3 0x82903840..0x82908208. Every ice_000 record must size-check against it.

screens/runtime/<movie>.json
- fedynamic.big hud2/{hudabd,hudthrowmes,hudcentral,hudtimer} and hudComponents/endchallenge_online, prepared
  as prepare_core_menu does.
- Their payload paths are rewritten to resolve from the online-ui layer's root (private/online-ui).

strings.json
- The language entries the mode, its rows and its screens use.
"""
import argparse
import functools
import json
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path[:0] = [str(TOOLS.parent), str(TOOLS), str(TOOLS / "vendor"),str(TOOLS / "vendor/university/tools/vanilla_map_extraction/tools")]
import asset_pipeline.vlt as vlt  # noqa: E402
# ponytail: vlt.convert hashes every known name (names.txt + the vault's strings) on each call, ~40 calls here; the hash
# is a pure function, so one memo serves them all. Belongs in vlt.py itself (not this stage's file).
vlt.hash64 = functools.cache(vlt.hash64)
from asset_pipeline.vlt import convert, hash64  # noqa: E402
from build_skate2_image import build as xex_image  # noqa: E402
from owned_game.big import BigArchive  # noqa: E402
from skate3_streams import read_atoc, read_sfil  # noqa: E402
import unluac_batch  # noqa: E402

BASE = 0x82000000
F = lambda name: "Hash_%016X" % hash64(name)  # noqa: E731  an AttribSys field / record key
GRAPH, STATE, HANDLER = "Hash_3EACD8F6F7B31F63", "Hash_2DEE2B54D25C86C4", "Hash_038FA43A118377A5"
STEPS, HANDLERS, BLOB = "Hash_3CB7BA2AFC9BDE42", "Hash_810394ECEFE7E48B", "Hash_C3CD4B4183274819"
CHALLENGE, GLOBAL = "Hash_8876E0B556740E36", "Hash_A4F35B159D7432B5"   # challengebanks classes
OBJECTIVE = "Hash_59A34A6C5074F88A"   # challengebanks objective records (dom_dwtn_01_0 < dom_dwtn_01 < dominations)
# challenge_local_data: the row's own record. Other classes' records may share its key and come first (street_indu_02's
# 1E04F6C0 record, race_ind_01's 7CBD399D one); every row file has exactly one record of this class keyed by the row.
LOCAL = "Hash_46A56ADDF284C381"
STATEGRAPH = "Hash_7867A00C3A6F893C"                                    # challenge -> its stategraph
AVAILABLE_ONLINE, TITLE = "Hash_F9A9A1DDED84A643", "Hash_E2A50788B6C20C92"
# The modes' HUD2 components (each graph's make_hud list) and the online results, shared by every mode under hud/.
# leaderboardhud and homscoring are the online-ui layer's own.
SCREENS = {name: f"data/fe/source/screens/hud2/{name}" for name in
           ("hudabd", "hudthrowmes", "hudcentral", "hudtimer", "hudcountdown", "dpo", "hudobjective2", "huddistance",
            "hudonlinevoting", "onlineotsstat")}
SCREENS.update({"killedit": "data/fe/source/screens/challenge/killedit",
                "ownthespot": "data/fe/source/screens/challenge/ownthespot",
                # The spot battle intros' OVERLAY 2 (SpotFound_BottomLeft; screen table 0x8302f1a8; intro_overlay.rs).
                "spotfound": "data/fe/source/screens/challenge/spotfound",
                "endchallenge_online": "data/fe/source/screens/hudComponents/endchallenge_online"})
LAYER_ROOT = "private/online-ui"   # apt_screen ONLINE: payloads resolve from here
OUT_ROOT = "private/skate3-online"


# ---------------------------------------------------------------- disc access
def big(disc, rel):
    return BigArchive(Path(disc) / rel)


def read(archive, path):
    entry = next((e for e in archive.entries if e.path.lower() == path.lower()), None)
    if entry is None:
        raise SystemExit(f"{path} is not in {archive.path if hasattr(archive, 'path') else 'the archive'}")
    return archive.read(entry)


class Db:
    """db.big's AttribSys collections, converted with skaterschema."""

    def __init__(self, disc, work):
        self.work = Path(work)
        self.names = (TOOLS / "asset_pipeline/names.txt").read_text(encoding="utf-8").splitlines()
        archive = big(disc, "data/big/db.big")
        for e in archive.entries:
            low = e.path.lower()
            if low.startswith(("data/db/skaterschema.", "data/db/challengebanks/main.", "data/db/challenge_stategraph/",
                               "data/db/challenge_local_data", "data/db/skatercollections.")) and low.endswith((".vlt", ".bin")):
                (self.work / e.path).parent.mkdir(parents=True, exist_ok=True)
                (self.work / e.path).write_bytes(archive.read(e))

    @functools.cache
    def collections(self, rel):
        base = self.work / "data/db" / rel
        return convert(self.work / "data/db/skaterschema", base, self.names)["collections"]

    def bin(self, rel):
        return (self.work / "data/db" / (rel + ".bin")).read_bytes()


# The challengebanks .bin offsets (u32 words) in each field type this tool reads from a bank.
# ponytail: types it does not read (tHighlightDefinition, tSpeechInfo, ...) are left unrebased.
BIN_WORDS = {"Sk8::Challenge::tObjectiveDefinition": (0, 3, 4), "LuaState::tCompiledLua": (0,),
             "Sk8::Challenge::tTriggerVolumeInstanceID": (0,), "Sk8::Challenge::tLocationID": (0,),
             "Sk8::Challenge::tChallengePresentationEvent": (1,), "Sk8::Challenge::tVisualEditorData": (6,)}


def rebase(collections, delta):
    """`collections` with every bank .bin offset moved `delta` on (offset 0 is the .bin's "StrE" header: no string)."""
    def move(text, words):
        b = bytearray.fromhex(text)
        for w in words:
            if len(b) >= 4 * w + 4 and struct.unpack_from(">I", b, 4 * w)[0]:
                struct.pack_into(">I", b, 4 * w, struct.unpack_from(">I", b, 4 * w)[0] + delta)
        return b.hex().upper()
    for c in collections:
        for v in c["fields"].values():
            words = BIN_WORDS.get(v.get("type"))
            if words and "array" in v:
                v["array"]["items"] = [move(x, words) for x in v["array"].get("items", [])]
            elif words and "data" in v:
                v["data"] = move(v["data"], words)
    return collections


class DlcDb(Db):
    """A Skate 3 DLC package (STFS, read as online_scoring.py reads them) seen through the disc's database: its
    challengebanks after main's, their .bin placed at `delta` in main.bin's (offsets rebased, so every package's
    snippets keep distinct offsets), its local-data framework before the disc's, its own challenge_local_data. The
    package's archive is also its missions (data/content/missions/<row>/), intro scenes (data/scene/scene_00N.bin +
    ice_00N.bin) and global locators."""

    def __init__(self, base, archive, work, delta):
        self.base, self.archive, self.work, self.names, self.delta = base, archive, Path(work), base.names, delta
        for e in archive.entries:
            low = e.path.lower()
            if low.startswith(("data/db/challengebanks/", "data/db/challenge_local_data/")) or (
                    low.startswith("data/db/") and "local_data_framework." in low):
                if low.endswith((".vlt", ".bin")):
                    (self.work / e.path).parent.mkdir(parents=True, exist_ok=True)
                    (self.work / e.path).write_bytes(archive.read(e))
        self.banks = sorted((self.work / "data/db/challengebanks").glob("*.vlt"))
        self.bank_bin = b"".join(v.with_suffix(".bin").read_bytes() for v in self.banks)

    @functools.cache
    def collections(self, rel):
        conv, schema = convert, self.base.work / "data/db/skaterschema"
        if rel == "challengebanks/main":
            own, at = [], self.delta
            for v in self.banks:
                own += rebase(conv(schema, v.with_suffix(""), self.names)["collections"], at)
                at += len(v.with_suffix(".bin").read_bytes())
            return self.base.collections(rel) + own
        if rel == "challenge_local_data_framework":
            own = [c for v in sorted((self.work / "data/db").glob("*local_data_framework.vlt"))
                   for c in conv(schema, v.with_suffix(""), self.names)["collections"]]
            return own + self.base.collections(rel)
        if rel.startswith("challenge_local_data/"):
            return conv(schema, self.work / "data/db" / rel, self.names)["collections"]
        return self.base.collections(rel)

    def bin(self, rel):
        if rel == "challengebanks/main":
            main = self.base.bin(rel)
            return main + bytes(self.delta - len(main)) + self.bank_bin
        if rel.startswith("challenge_local_data/"):
            return (self.work / "data/db" / (rel + ".bin")).read_bytes()
        return self.base.bin(rel)

    def templates(self, graph):
        """The package's challenge templates whose StateGraph is `graph` (dlc_mmcn_own_the_spots, dlc_play_own_the_spots:
        no parent chain reaches the disc's own_the_spots)."""
        key = "%016X" % hash64(graph)
        return tuple(c["key"] for c in self.collections("challengebanks/main")[len(self.base.collections("challengebanks/main")):]
                     if c["class"] == CHALLENGE and c["fields"].get(STATEGRAPH, {}).get("data", "")[16:32] == key)

    def copies(self, names, graph):
        """The package's own copies of the disc templates `names` (dlc_dwgh_street, dlc_dwgh_vert) whose parent is one
        of its `graph` templates (dlc_dwgh_contests): a graph that also runs templates the disc mode leaves out
        (contest_shared's dlc_dwgh_demos)."""
        under = self.templates(graph)
        return tuple(c["key"] for c in self.collections("challengebanks/main")[len(self.base.collections("challengebanks/main")):]
                     if c["class"] == CHALLENGE and c["parent"] in under and c["key"].endswith(tuple("_" + n for n in names)))

    @functools.cached_property
    def global_locators(self):
        from online_challenge_map import global_locators
        out = {}
        for e in self.archive.entries:
            if e.path.lower().startswith("data/content/global_locators/") and e.path.lower().endswith(".rx2"):
                out.update(global_locators(self.archive.read(e)))
        return out

    @functools.cached_property
    def language(self):
        """The package's English strings by id (data/fe/languages labels + english, as online_challenge_map reads them)."""
        from online_challenge_map import language_table
        files = {e.path.lower(): e for e in self.archive.entries
                 if e.path.lower().startswith("data/fe/languages/") and "histogram" not in e.path.lower()}
        labels = [e for p, e in files.items() if "/labels/" in p and p.endswith(".bin")]
        english = [e for p, e in files.items() if "/english/" in p and p.endswith(".bin")]
        if not (labels and english):
            return {}
        ids, texts = language_table(self.archive.read(labels[0])), language_table(self.archive.read(english[0]))
        return {v.strip(): texts[k] for k, v in ids.items() if k in texts}

    def scenes(self):
        """[(scene_00N.bin, ice_00N.bin)] of the package (its intro NIS scenes and their ICE cameras)."""
        files = {Path(e.path).name.lower(): e for e in self.archive.entries if e.path.lower().startswith("data/scene/")}
        return [(self.archive.read(files[n]), self.archive.read(files["ice" + n[5:]])) for n in sorted(files)
                if n.startswith("scene_") and "ice" + n[5:] in files]


def dlc_packages(folder, base, work):
    """Every STFS package under `folder` (files starting CON/LIVE/PIRS, as online_scoring.py) with challengebanks, as
    a DlcDb; each package's bank .bin placed after the disc's and the earlier packages'."""
    from online_challenge_map import stfs_extract
    out, delta = [], len(base.bin("challengebanks/main"))
    for pkg in sorted(q for q in Path(folder).rglob("*") if q.is_file()):
        with open(pkg, "rb") as f:
            if f.read(4) not in (b"CON ", b"LIVE", b"PIRS"):
                continue
        target = Path(work) / pkg.name
        try:
            stfs_extract(pkg, target)
        except (IndexError, NotImplementedError) as e:
            # online_scoring.py's skip: the ~120 KB unlock packages carry no database.
            print(f"skipped DLC package {pkg.name} ({pkg.stat().st_size} bytes): {e!r}")
            continue
        for b in sorted(target.glob("*.big")):
            archive = BigArchive(b)
            if not any(e.path.lower().startswith("data/db/challengebanks/") for e in archive.entries):
                continue
            d = DlcDb(base, archive, target / "x", delta)
            delta += len(d.bank_bin)
            out.append(d)
    return out


def cstr(d, at):
    return d[at:d.index(b"\0", at)].decode("latin-1")


# ---------------------------------------------------------------- the Lua (decompiled for reference)
def mode_graphs(mode):
    """The stategraphs whose Lua a mode's scripts/ holds."""
    return sorted(OTL_GRAPHS.values()) if mode == "otl" else ["skate" if mode == "skate" else MODES[mode]["graph"]]


def graphs_lua(db, graphs):
    """{graph: skate.lua as the stategraph chain has it: every handler of every graph from the root down}, the handlers
    of all the graphs decompiled in one unluac batch (unluac_batch.py); None without Java/unluac."""
    java, unluac = os.environ.get("SKATE_JAVA", "java"), os.environ.get("SKATE_UNLUAC")
    if not unluac or not Path(unluac).is_file() or not shutil.which(java):
        return None
    lines = {}  # graph -> its lines: text, or (state, handler, chunk) for a handler
    for graph in dict.fromkeys(graphs):
        coll = db.collections(f"challenge_stategraph/{graph}")
        binary = db.bin(f"challenge_stategraph/{graph}")
        by = {(c["class"], c["key"]): c for c in coll}
        text = lines[graph] = [f"-- {graph}: Skate 3 challenge stategraph handlers, decompiled by unluac"]
        for g in (c for c in coll if c["class"] == GRAPH):
            text.append(f"-- graph {g['key']} parent {g['parent']!r}")
            for item in g["fields"].get(STEPS, {}).get("array", {}).get("items", []):
                st = by.get((STATE, "Hash_" + item[16:32]))
                if st is None:   # a step whose state record is not in this graph's collection (domination's chain)
                    text.append(f"-- missing state {item}")
                    continue
                for h in st["fields"].get(HANDLERS, {}).get("array", {}).get("items", []):
                    hr = by[(HANDLER, "Hash_" + h[16:32])]
                    size, off = struct.unpack(">II", bytes.fromhex(hr["fields"][BLOB]["data"]))
                    text.append((st, hr, binary[off:off + size]))
    done = unluac_batch.run([x[2] for text in lines.values() for x in text if isinstance(x, tuple)], java, unluac)
    name = lambda c: c["fields"].get("Name", {}).get("data", c["key"])  # noqa: E731

    def line(graph, x):
        if isinstance(x, str):
            return x
        stdout, code = done[x[2]]
        if code:
            raise SystemExit(f"unluac failed on {graph}")
        return f"-- [{name(x[0])}] {name(x[1])}\n{unluac_batch.text(stdout).strip()}\n"
    return {graph: "\n".join(line(graph, x) for x in text) + "\n" for graph, text in lines.items()}


# ---------------------------------------------------------------- rows
class Template:
    """A challenge_local_data record with its framework parents: field -> value, nearest first."""

    def __init__(self, db, local_parents=False):
        self.framework = {c["key"]: c for c in db.collections("challenge_local_data_framework")}
        # Own the Lot's activities: a row's parent is a challenge_local_data template (freeskate_activities_tricklist),
        # not a framework record. Its fields are plain values (no offsets into the row's .bin).
        if local_parents:
            for name in ("freeskate_activities", *OTL_GRAPHS):
                try:
                    for c in db.collections(f"challenge_local_data/{name}"):
                        self.framework[c["key"]] = self.framework[F(c["key"])] = c
                except (OSError, ValueError, KeyError):
                    pass

    def chain(self, record):
        out = dict(record["fields"])
        parent = record["parent"]
        while parent and parent in self.framework:
            for k, v in self.framework[parent]["fields"].items():
                out.setdefault(k, v)
            parent = self.framework[parent]["parent"]
        return out


def text_value(v):
    return v["data"]


def u32(v, at=0):
    return struct.unpack_from(">I", bytes.fromhex(v["data"]), at)[0]


def f32(v):
    return struct.unpack(">f", bytes.fromhex(v["data"])[:4])[0]


def location(binary, v):
    """tLocationID: a u32 offset into the row .bin's strings (a locator name)."""
    return cstr(binary, u32(v))


def volume_id(binary, item):
    """tTriggerVolumeInstanceID: (u32 offset of "DIST|name|0xid", pad, u64 id) -> the volume's name."""
    return cstr(binary, int(item[:8], 16)).split("|")[1]


def items(v):
    return v.get("array", {}).get("items", [])


def mission_placement(missions, row, splines=None):
    """missions/<row>/cSim_Global.xsf: locators (0xEB0009: 0x80-byte records; matrix @0, child count @0x60, child
    table @0x64, name ptr @0x68; children 0x70 bytes, matrix @0, name ptr @0x60) and trigger volumes (0xEB0019:
    0xF0-byte records; min @0x40, max @0x50, name ptr @0xE0) -- as skate2port/old_tutorial.mission_placement and
    online_challenge_map.global_locators read them. With a `splines` dict, the spline bank of the row (0xEB0021) is
    read into it too ([`rw4_placement`]); the return value is the same two dicts either way."""
    # A row whose id starts with a digit lives under "_<row>" (missions/_1up_dwtn_01/); an online copy of a career row
    # (ots_dwtn_01_ol) plays on its career mission's placement (its locator names are missions/ots_dwtn_01's), as
    # skate2port/old_online.py resolves Skate 2's _ol rows.
    for row in (row, f"_{row}", re.sub(r"_ol$", "", row)):
        files = {Path(e.path).name: e for e in missions.entries if f"/missions/{row}/".lower() in e.path.lower()}
        if files:
            break
    else:
        # A DLC map's row may have no mission: its placement is baked into the package's world stream (1up_dwmc_01's
        # _1up_dwmc_01_startlocator_round_N are DLC_DW_MegaCompund's). The disc's missions.big has no world stream.
        return tuple(dict(x) for x in world_placement(missions))   # copies: callers add to them
    locators, volumes = {}, {}
    with tempfile.TemporaryDirectory(prefix="skate3-mission-") as work:
        work = Path(work)
        wanted = {f"{row}_Sim.xst".lower(), "csim_global.xsf"}  # only the two read below (a later same-name-in-any-
        for n, e in files.items():                               # case file still replaces an earlier one)
            if n.lower() in wanted:
                (work / n).write_bytes(missions.read(e))
        records = read_atoc(work / f"{row}_Sim.xst")
        for asset in read_sfil(work / "cSim_Global.xsf", records):
            rw4_placement(asset.data, locators, volumes, splines)
    return locators, volumes


@functools.cache
def world_placement(archive):
    """The locators and volumes of every world stream district in `archive` (data/content/world/stream/<district>/:
    its <district>_Sim.xst and cSim_*.xsf tiles), read as a mission's ([`mission_placement`]); a record repeated in
    several tiles must be the same in each."""
    locators, volumes = {}, {}
    districts = {}
    for e in archive.entries:
        parts = e.path.split("/")
        if e.path.lower().startswith("data/content/world/stream/") and len(parts) == 6:
            districts.setdefault(parts[4], []).append(e)
    for district, entries in sorted(districts.items()):
        xst = next((e for e in entries if e.path.lower().endswith(f"/{district}_sim.xst".lower())), None)
        tiles = [e for e in entries if Path(e.path).name.lower().startswith("csim_") and e.path.lower().endswith(".xsf")]
        if xst is None or not tiles:
            continue
        with tempfile.TemporaryDirectory(prefix="skate3-world-") as work:
            work = Path(work)
            (work / "sim.xst").write_bytes(archive.read(xst))
            records = read_atoc(work / "sim.xst")
            index = {r.asset_id: r for r in records}
            for e in sorted(tiles, key=lambda e: e.path):
                (work / "tile.xsf").write_bytes(archive.read(e))
                for asset in read_sfil(work / "tile.xsf", records, require_all_records=False, record_index=index):
                    more_locators, more_volumes = {}, {}
                    rw4_placement(asset.data, more_locators, more_volumes)
                    for into, more in ((locators, more_locators), (volumes, more_volumes)):
                        for k, v in more.items():
                            if into.setdefault(k, v) != v:
                                raise SystemExit(f"{district}: {k} placed differently in two tiles")
    return locators, volumes


def rw4_placement(d, locators, volumes, splines=None):
    """One RW4 arena's locators (0xEB0009) and trigger volumes (0xEB0019), into `locators` / `volumes`. With a `splines`
    dict, its spline bank (0xEB0021, stage1_traces 3.2) is read into it as {instance id: {"flags", "points"}}; without
    one that object is skipped, as it always was."""
    if d[:4] != b"\x89RW4":
        return
    matrix = lambda d, at: [list(struct.unpack_from(">4f", d, at + 16 * k)) for k in range(4)]  # noqa: E731
    count_all, = struct.unpack_from(">I", d, 0x20)
    table_at, = struct.unpack_from(">I", d, 0x30)
    for i in range(count_all):
        at, _, _, _, _, kind = struct.unpack_from(">6I", d, table_at + 24 * i)
        if kind == 0xEB0009:
            count, _, _, first = struct.unpack_from(">4I", d, at)
            for r in range(count):
                o = at + first + 0x80 * r
                kids, kid_table = struct.unpack_from(">2I", d, o + 0x60)
                slots = []
                for k in range(kids):
                    c = at + kid_table + 0x70 * k
                    name = cstr(d, at + struct.unpack_from(">I", d, c + 0x60)[0])
                    number = re.search(r"(\d+)$", name)
                    slots.append((int(number.group(1)) if number else k, matrix(d, c)))
                slots.sort(key=lambda s: s[0])
                locators[cstr(d, at + struct.unpack_from(">I", d, o + 0x68)[0])] = {
                    "matrix": matrix(d, o), "slots": [m for _, m in slots]}
        elif kind == 0xEB0019:
            _, count, _, first = struct.unpack_from(">4I", d, at)
            for r in range(count):
                o = at + first + 0xF0 * r
                full = cstr(d, at + struct.unpack_from(">I", d, o + 0xE0)[0])
                # id @0xB8: the instance id (tTriggerVolumeInstanceID's u64; ObjectiveTrigger.EnteredVolume /
                # IsInVolume take it in decimal: ots_dwtn_01_spotvolume_01 0x0008B0CF03E38706).
                volumes[full.split("_0x", 1)[0]] = {
                    "min": list(struct.unpack_from(">3f", d, o + 0x40)),
                    "max": list(struct.unpack_from(">3f", d, o + 0x50)),
                    "id": struct.unpack_from(">Q", d, o + 0xB8)[0]}
        elif kind == 0xEB0021 and splines is not None:
            # u32 spline count, total points, records at +0x10, points. A 0x40-byte record: u64 hash, u64 instance id
            # (the id of a tVisualEditorData kind 1 item, +16), min xyz + pad, max xyz + pad, u32 flags, u32 point count,
            # u32 point offset (from the object), u32 0. A 64-byte point: vec4 A, vec4 B, vec4 0, vec4 position (w = 1).
            count, _, first, _ = struct.unpack_from(">4I", d, at)
            for r in range(count):
                o = at + first + 0x40 * r
                instance, = struct.unpack_from(">Q", d, o + 8)
                flags, points, offset, _ = struct.unpack_from(">4I", d, o + 48)
                splines[instance] = {"flags": flags, "points": [list(struct.unpack_from(">3f", d, at + offset + 64 * p + 48))
                                                                for p in range(points)]}


def template_rows(db, template):
    """Every challenge whose challengebanks chain reaches `template` and which has local data."""
    bank = db.collections("challengebanks/main")
    challenges = {c["key"]: c for c in bank if c["class"] == CHALLENGE}
    globals_ = {c["key"]: c for c in bank if c["class"] == GLOBAL}

    def under(key):
        while key in challenges:
            if challenges[key]["parent"] == template:
                return True
            key = challenges[key]["parent"]
        return False
    def chained(key):
        """The challenge_global_data record with its parents' fields (AvailableOnline is the template's)."""
        out, record = {}, globals_.get(key)
        while record:
            for k, v in record["fields"].items():
                out.setdefault(k, v)
            record = globals_.get(record["parent"])
        return {"fields": out}
    local = Path(db.work / "data/db/challenge_local_data")
    return {k: chained(k) for k in sorted(challenges) if under(k) and (local / f"{k}.vlt").is_file()}


def skate_rules(db, missions, template="s_k_a_t_e"):
    """`template` (or each of a tuple of templates: a DLC package's, DlcDb.templates("skate"))."""
    tpl = Template(db)
    out = {}
    template_fields = None
    found = {}
    for t in (template if isinstance(template, tuple) else (template,)):
        found.update(template_rows(db, t))
    for row, g in found.items():
        rel = f"challenge_local_data/{row}"
        record = next(c for c in db.collections(rel) if c["key"] in (row, F(row)) and c["class"] == LOCAL)
        fields, binary = tpl.chain(record), db.bin(rel)
        template_fields = template_fields or tpl.chain({"fields": {}, "parent": record["parent"]})
        splines = {}
        locators, volumes = mission_placement(missions, row, splines)
        own = record["fields"]   # offsets in the row's own .bin; the template's are defaults with no locator

        def loc(name):
            return locators.get(location(binary, own[F(name)])) if F(name) in own else None

        def vol(name):
            return volumes.get(volume_id(binary, own[F(name)]["data"])) if F(name) in own else None
        intro = [cstr(binary, int(i[8:16], 16)) for i in items(own.get(F("IntroPresentationEvents"), {}))
                 if int(i[:8], 16) == 5]
        out[row] = {
            "title": text_value(g["fields"][TITLE]) if g and TITLE in g["fields"] else None,
            "online": bool(g and AVAILABLE_ONLINE in g["fields"] and g["fields"][AVAILABLE_ONLINE]["data"][:2] != "00"),
            "time_limit": f32(fields[F("TimeLimit")]),
            "intro": intro[0] if intro else None,
            "spawn": loc("TurnBasedAttemptSpawnPoint"),
            "wait": loc("TurnBasedWaitingLocation"),
            "start_volume": vol("TurnBasedStartVolume"),
            "spot_volumes": [volumes.get(volume_id(binary, i)) for i in items(own.get(F("SpotVolumes"), {}))],
            "boundary": vol("ChallengeBoundary"),
            # tVisualEditorData (80 bytes): the locator's name offset at +24. An empty name places nothing
            # (skate_indu_02's second entry).
            "indicators": [locators.get(n, {}).get("matrix") for n in
                           (cstr(binary, int(i[48:56], 16)) for i in items(own.get(F("VisualIndicators"), {}))) if n],
            # The same VisualIndicators with their ribbon type and style (user 10-25: skate.lua AddIndicator draws them,
            # the neon arrow over each spot), for indicators.json.
            "visual_indicators": chevrons(own.get(F("VisualIndicators")), binary, locators, kinds=(0,)),
            "outlines": outlines(own.get(F("VisualIndicators")), splines),
        }
        missing = [k for k in ("spawn", "wait", "start_volume") if out[row][k] is None]
        missing += [k for k in ("spot_volumes", "indicators") if None in out[row][k] or not out[row][k]]
        if missing:
            print(f"{row}: unplaced {missing}")
    messages = {db_name: text_value(v) for db_name, v in
                ((n, template_fields.get(F(n))) for n in MESSAGES) if v is not None}
    return {
        "template": messages,
        # ABDListItemLimit: Int8 0x05000000 on the disc (the value is the first byte).
        "max_letters": bytes.fromhex(template_fields[F("ABDListItemLimit")]["data"])[0],
        "time_limit": f32(template_fields[F("TimeLimit")]),
        "rows": out,
    }


def decode(v, binary, locators, volumes):
    """A row field as the port reads it. Locations and volumes resolve through the row's own .bin (None when the
    field is the template's: its offsets are into the framework's strings, no mission places them)."""
    t, arr = v["type"], "array" in v
    raw = items(v) if arr else [v["data"]]
    if t == "EA::Reflection::Float":
        out = [struct.unpack(">f", bytes.fromhex(x)[:4])[0] for x in raw]
    elif t == "EA::Reflection::Bool":
        out = [x[:2] != "00" for x in raw]
    elif t == "EA::Reflection::Int32":
        out = [struct.unpack(">i", bytes.fromhex(x)[:4])[0] for x in raw]
    elif t == "EA::Reflection::Int16":
        # In the word's top half (PlaceHigherToWin "00010000" = 1).
        out = [struct.unpack(">h", bytes.fromhex(x)[:2])[0] for x in raw]
    elif t in ("EA::Reflection::UInt32", "Sk8::Challenge::eChallengeTypes", "Sk8::Challenge::eRaceType",
               "Sk8::Challenge::eSk8ChallengeIndicators", "Sk8::Challenge::eContestType", "Sk8::Challenge::eRunType",
               "Sk8::Challenge::eFreeskateType", "Sk8::Challenge::eFreeskateAccumulationType",
               "Sk8::Challenge::eFreeskateSurvivalType"):
        out = [struct.unpack(">I", bytes.fromhex(x)[:4])[0] for x in raw]
    elif t in ("EA::Reflection::UInt8", "EA::Reflection::Int8"):
        out = [bytes.fromhex(x)[0] for x in raw]
    elif t == "EA::Reflection::Text":
        return v["array"].get("text_items", []) if arr else v["data"]
    elif t == "Sk8::Challenge::tLocationID":
        if binary is None:
            return [] if arr else None
        names = [cstr(binary, int(x[:8], 16)) for x in raw]
        out = [dict(name=n, **locators[n]) if n in locators else None for n in names]
    elif t == "Sk8::Challenge::tTriggerVolumeInstanceID":
        if binary is None:
            return [] if arr else None
        names = [cstr(binary, int(x[:8], 16)).split("|")[1] if "|" in cstr(binary, int(x[:8], 16)) else "" for x in raw]
        out = [dict(name=n, **volumes[n]) if n in volumes else None for n in names]
    elif t == "Sk8::Challenge::tChallengePresentationEvent":
        if binary is None:
            return []
        out = [[int(x[:8], 16), cstr(binary, int(x[8:16], 16))] for x in raw]
    elif t == "Sk8::tNISPlaybackDefinition":
        # The NIS scene's name: a u32 offset into the row's strings (ots_dwtn_01_ol: 0x4D -> "ots_dwtn_01_intro").
        if binary is None:
            return [] if arr else None
        out = [cstr(binary, int(x[:8], 16)) or None for x in raw]
    elif t == "Sk8::Challenge::tVisualEditorData":
        # 80 bytes: the locator's name offset at +24 (as skate_rules reads VisualIndicators); an empty name places nothing.
        if binary is None:
            return [] if arr else None
        names = [cstr(binary, int(x[48:56], 16)) for x in raw]
        out = [locators.get(n, {}).get("matrix") for n in names if n]
    elif t == "Sk8::Challenge::tSplineBankObject":
        # A ribbon instance (RibbonInstance, RaceGateGreyRibbon, ...): the indicator type's key (class 3297556B..) at +8
        # and the style's (class F0DB8767..) at +32, as skate2port/old_death_race.py reads Skate 2's.
        out = [{"ribbon": x[16:32], "style": x[64:80]} for x in raw]
    else:
        out = raw
    return out if arr else out[0]


def global_objectives(db, g, locators, volumes, indicators=False):
    """The row's Location and its Objectives (challenge_global_data): each objective a challengebanks record chained by
    parent, its ScoringBoundary volume an offset into challengebanks/main.bin; with `indicators`, its VisualIndicators
    too (domination's team ribbons recolour them, domination.lua 533-541)."""
    by = {}
    for c in db.collections("challengebanks/main"):
        if c["class"] == OBJECTIVE:
            by[c["key"]] = by[F(c["key"])] = c
    binary = db.bin("challengebanks/main")
    out = []
    for item in items(g.get(F("Objectives"), {})):
        fields, r = {}, by.get("Hash_" + item[16:32])
        while r:
            for k, v in r["fields"].items():
                fields.setdefault(k, v)
            r = by.get(r["parent"])
        out.append({"ScoringBoundary": decode(fields[F("ScoringBoundary")], binary, locators, volumes)
                    if F("ScoringBoundary") in fields else None})
        if indicators:
            out[-1]["VisualIndicators"] = (decode(fields[F("VisualIndicators")], binary, locators, volumes)
                                           if F("VisualIndicators") in fields else [])
    location = decode(g[F("Location")], binary, locators, volumes) if F("Location") in g else None
    return location, out


# ---------------------------------------------------------------- objectives as data (skate3_objectives.rs)
# An objective's conditions are Lua source in challengebanks/main.bin, run by the objectives manager wrapped as
# `function h( vars ) <src> end` (format 0x820b88f4) with the prelude's InitVars (0x820b87d0). They are turned into an
# expression tree here; crates/skate-game/src/skate3_objectives.rs interprets it (no Lua runs in the game). The tree:
#   statements: ["local", [slot...], [expr...]]  ["set", [target...], [expr...]]  ["call", expr]  ["do", [stmt...]]
#               ["if", [[cond, [stmt...]]...], [stmt...] | None]  ["return", [expr...]]
#               ["for", slot, start, stop, step | None, [stmt...]] (numeric; hom_indu_02's ped count)
#   expressions: ["nil"] ["bool", b] ["num", x] ["str", s] ["slot", n] ["vars"] ["global", name] ["index", e, key]
#               ["native", "NS.Fn" | "InitVars", [arg...]] ["const", "NS.Enum.Value"] ["bin", op, a, b] ["un", op, a]
#               ["table", [[key | None, value]...]]
#   targets: ["slot", n], ["global", name] or ["index", e, key]
# Locals are numbered slots (one per declaration). Any other name is a global of the objectives' one Lua state
# ("cChallengeObjectivesManager::LuaStateManager"), shared by every snippet: unset, it reads nil.
LUA_KEYWORDS = {"and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil",
                "not", "or", "repeat", "return", "then", "true", "until", "while"}
# Lua 5.1 binary precedence (lparser.c priority[]): (left, right).
LUA_BINARY = {"or": (1, 1), "and": (2, 2), "<": (3, 3), ">": (3, 3), "<=": (3, 3), ">=": (3, 3), "~=": (3, 3),
              "==": (3, 3), "..": (5, 4), "+": (6, 6), "-": (6, 6), "*": (7, 7), "/": (7, 7), "%": (7, 7), "^": (10, 9)}
LUA_UNARY = 8


def lua_tokens(src):
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c.isspace():
            i += 1
        elif src.startswith("--", i):
            m = re.match(r"--\[(=*)\[", src[i:])
            if m:
                end = src.index("]" + m.group(1) + "]", i)
                i = end + len(m.group(1)) + 2
            else:
                j = src.find("\n", i)
                i = n if j < 0 else j
        elif c.isalpha() or c == "_":
            m = re.match(r"[A-Za-z_][A-Za-z_0-9]*", src[i:])
            w = m.group()
            out.append(("kw" if w in LUA_KEYWORDS else "name", w))
            i += len(w)
        elif c.isdigit() or (c == "." and i + 1 < n and src[i + 1].isdigit()):
            m = re.match(r"0[xX][0-9a-fA-F]+|(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?", src[i:])
            out.append(("num", float(int(m.group(), 16)) if m.group()[:2].lower() == "0x" else float(m.group())))
            i += len(m.group())
        elif c in "\"'":
            j, s = i + 1, []
            while src[j] != c:
                if src[j] == "\\":
                    j += 1
                    esc = {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", "\"": "\"", "'": "'"}
                    if src[j] in esc:
                        s.append(esc[src[j]])
                    elif src[j].isdigit():
                        m = re.match(r"\d{1,3}", src[j:])
                        s.append(chr(int(m.group())))
                        j += len(m.group()) - 1
                    else:
                        raise ValueError(f"escape \\{src[j]}")
                else:
                    s.append(src[j])
                j += 1
            out.append(("str", "".join(s)))
            i = j + 1
        else:
            for op in ("...", "==", "~=", "<=", ">=", "..", "<", ">", "=", "+", "-", "*", "/", "%", "^", "#", "(", ")",
                       "{", "}", "[", "]", ";", ":", ",", "."):
                if src.startswith(op, i):
                    out.append(("op", op))
                    i += len(op)
                    break
            else:
                raise ValueError(f"character {c!r}")
    out.append(("eof", None))
    return out


class LuaTree:
    """A snippet (the body of `function h( vars )`) to the tree above; `natives` collects the NS.Fn names it calls."""

    def __init__(self, src):
        self.t, self.i, self.scopes, self.slots, self.natives = lua_tokens(src), 0, [{}], 0, set()

    def peek(self, kind, value=None):
        k, v = self.t[self.i]
        return k == kind and (value is None or v == value)

    def take(self, kind, value=None):
        if not self.peek(kind, value):
            raise ValueError(f"expected {value or kind}, found {self.t[self.i]}")
        self.i += 1
        return self.t[self.i - 1][1]

    def chunk(self):
        body = self.block()
        self.take("eof")
        return body

    def block(self):
        self.scopes.append({})
        out = []
        while not (self.peek("eof") or any(self.peek("kw", k) for k in ("end", "else", "elseif", "until"))):
            if self.peek("kw", "return"):
                self.i += 1
                values = [] if self.at_block_end() or self.peek("op", ";") else self.exprs()
                if self.peek("op", ";"):
                    self.i += 1
                out.append(["return", values])
                break
            out.append(self.statement())
            if self.peek("op", ";"):
                self.i += 1
        self.scopes.pop()
        return out

    def at_block_end(self):
        return self.peek("eof") or any(self.peek("kw", k) for k in ("end", "else", "elseif", "until"))

    def statement(self):
        if self.peek("kw", "do"):
            self.i += 1
            body = self.block()
            self.take("kw", "end")
            return ["do", body]
        if self.peek("kw", "local"):
            self.i += 1
            names = [self.take("name")]
            while self.peek("op", ","):
                self.i += 1
                names.append(self.take("name"))
            values = []
            if self.peek("op", "="):
                self.i += 1
                values = self.exprs()
            slots = []
            for n in names:   # declared after the values are read (`local x = x` reads the outer x)
                self.scopes[-1][n] = self.slots
                slots.append(self.slots)
                self.slots += 1
            return ["local", slots, values]
        if self.peek("kw", "for"):
            self.i += 1
            name = self.take("name")
            self.take("op", "=")
            start = self.expr()
            self.take("op", ",")
            stop = self.expr()
            step = None
            if self.peek("op", ","):
                self.i += 1
                step = self.expr()
            self.take("kw", "do")
            self.scopes.append({name: self.slots})   # the loop variable, visible only in the body
            slot, self.slots = self.slots, self.slots + 1
            body = self.block()
            self.scopes.pop()
            self.take("kw", "end")
            return ["for", slot, start, stop, step, body]
        if self.peek("kw", "if"):
            self.i += 1
            arms = [[self.expr(), self.then_block()]]
            other = None
            while True:
                if self.peek("kw", "elseif"):
                    self.i += 1
                    arms.append([self.expr(), self.then_block()])
                elif self.peek("kw", "else"):
                    self.i += 1
                    other = self.block()
                    self.take("kw", "end")
                    break
                else:
                    self.take("kw", "end")
                    break
            return ["if", arms, other]
        target = self.suffixed()
        if self.peek("op", "=") or self.peek("op", ","):
            targets = [target]
            while self.peek("op", ","):
                self.i += 1
                targets.append(self.suffixed())
            self.take("op", "=")
            for t in targets:
                if t[0] not in ("slot", "global", "index"):
                    raise ValueError(f"cannot assign to {t}")
            return ["set", targets, self.exprs()]
        if target[0] != "native":
            raise ValueError(f"statement {target} is not a call")
        return ["call", target]

    def then_block(self):
        self.take("kw", "then")
        return self.block()

    def exprs(self):
        out = [self.expr()]
        while self.peek("op", ","):
            self.i += 1
            out.append(self.expr())
        return out

    def expr(self, limit=0):
        k, v = self.t[self.i]
        if (k == "kw" and v == "not") or (k == "op" and v in ("-", "#")):
            self.i += 1
            left = ["un", v, self.expr(LUA_UNARY)]
        else:
            left = self.simple()
        while True:
            k, v = self.t[self.i]
            op = v if k in ("op", "kw") and v in LUA_BINARY else None
            if op is None or LUA_BINARY[op][0] <= limit:
                return left
            self.i += 1
            left = ["bin", op, left, self.expr(LUA_BINARY[op][1])]

    def simple(self):
        k, v = self.t[self.i]
        if k == "num":
            self.i += 1
            return ["num", v]
        if k == "str":
            self.i += 1
            return ["str", v]
        if k == "kw" and v in ("true", "false"):
            self.i += 1
            return ["bool", v == "true"]
        if k == "kw" and v == "nil":
            self.i += 1
            return ["nil"]
        if k == "op" and v == "{":
            return self.table()
        return self.suffixed()

    def table(self):
        self.take("op", "{")
        items = []
        while not self.peek("op", "}"):
            if self.peek("op", "["):
                self.i += 1
                key = self.expr()
                self.take("op", "]")
                self.take("op", "=")
                items.append([key, self.expr()])
            elif self.peek("name") and self.t[self.i + 1] == ("op", "="):
                key = ["str", self.take("name")]
                self.i += 1
                items.append([key, self.expr()])
            else:
                items.append([None, self.expr()])
            if self.peek("op", ",") or self.peek("op", ";"):
                self.i += 1
        self.i += 1
        return ["table", items]

    def local(self, name):
        for scope in reversed(self.scopes):
            if name in scope:
                return scope[name]
        return None

    def suffixed(self):
        if self.peek("op", "("):
            self.i += 1
            e = self.expr()
            self.take("op", ")")
            # A parenthesised call keeps one value; no snippet relies on more.
        else:
            name = self.take("name")
            slot = self.local(name)
            if slot is not None:
                e = ["slot", slot]
            elif name == "vars":
                e = ["vars"]
            elif name == "InitVars":
                e = ["fn", "InitVars"]
            elif self.peek("op", ".") and self.t[self.i + 1][0] == "name":
                # A native namespace: NS.Fn(...) is a native, NS.Enum.Value a constant of its registration.
                self.i += 1
                e = ["fn", f"{name}.{self.take('name')}"]
                while self.peek("op", ".") and self.t[self.i + 1][0] == "name" and self.t[self.i + 2] != ("op", "("):
                    self.i += 1
                    e = ["fn", f"{e[1]}.{self.take('name')}"]
                if not self.peek("op", "(") and not self.peek("str"):
                    self.natives.add("const:" + e[1])
                    e = ["const", e[1]]
            else:
                # A global variable of the objectives' shared Lua state (an undeclared name reads nil until set).
                e = ["global", name]
        while True:
            if self.peek("op", "."):
                self.i += 1
                if e[0] == "fn":
                    raise ValueError(f"field of {e[1]}")
                e = ["index", e, ["str", self.take("name")]]
            elif self.peek("op", "["):
                self.i += 1
                key = self.expr()
                self.take("op", "]")
                e = ["index", e, key]
            elif self.peek("op", "("):
                self.i += 1
                args = [] if self.peek("op", ")") else self.exprs()
                self.take("op", ")")
                if e[0] != "fn":
                    raise ValueError(f"call of {e}")
                self.natives.add(e[1])
                e = ["native", e[1], args]
            elif self.peek("str"):
                if e[0] != "fn":
                    raise ValueError(f"call of {e}")
                self.natives.add(e[1])
                e = ["native", e[1], [["str", self.take("str")]]]
            elif self.peek("op", ":"):
                raise ValueError("method call")
            else:
                if e[0] == "fn":
                    raise ValueError(f"function value {e[1]}")
                return e


def snippet_tree(src):
    """(tree, natives) of a snippet; raises ValueError outside the subset."""
    p = LuaTree(src)
    return p.chunk(), sorted(p.natives)


OBJECTIVE_GROUP = "Hash_3CDBD8935AB90C06"   # challengebanks objective groups {Objectives, PointRequirement}
FAIL_FIRST = "Hash_B053106604767956"        # bool: Evaluate 0x82c752a8 runs the failures before the objective
COUNT_KIND = "Hash_2B245EC6D20A64AA"        # Int16: over 1, GetTotalNumCompletedObjectives counts it on argument 2 only
GLOBAL_POINTS = "global_points_objective"   # 0x82c6d4d8 makes it (key string 0x820b86d0) for a PointRequirement group
# challenge_local_data Bool (its name is not recovered): when set, the objective group's WipeoutOver handler
# (0x82c6d890 -> ResetActive 0x82c6d460) wipes the skater's still-Active objectives on every bail. The framework's
# default record holds 00; the disc sets it on four hom rows (peer-task10.md, U4). `local_bool` reads it per row.
WIPEOUT_RESETS_ACTIVE = "Hash_59F6A63F5900A8C9"


class Objectives:
    """challengebanks objective records with their parents' fields, and their snippets as trees (`sources` keeps each
    snippet's text by its main.bin offset: the oracle tests' input, written beside the graph's Lua)."""

    def __init__(self, db, disc=None):
        self.disc = disc
        self.by = {}
        for c in db.collections("challengebanks/main"):
            self.by[(c["class"], c["key"])] = self.by[(c["class"], F(c["key"]))] = c
        self.bin = db.bin("challengebanks/main")
        self.sources = {}
        self.points = None

    def ref(self, item):
        return self.by.get(("Hash_" + item[:16], "Hash_" + item[16:32]))

    def chain(self, r):
        out = {}
        while r:
            for k, v in r["fields"].items():
                out.setdefault(k, v)
            r = self.by.get((r["class"], r["parent"])) or self.by.get((r["class"], F(r["parent"]))) if r["parent"] else None
        return out

    def snippet(self, offset):
        """The snippet at a main.bin offset: {src, tree, natives}; an empty one is None (the handler's default)."""
        src = cstr(self.bin, offset)
        if not src.strip():
            return None
        tree, natives = snippet_tree(src)
        self.sources[str(offset)] = src
        return {"src": offset, "tree": tree, "natives": natives}

    def definitions(self, v):
        """tObjectiveDefinition (5 x u32: source offset, 0, 0x10, HAL text offset, param offset) or an array of them."""
        raw = items(v) if "array" in v else [v["data"]]
        return [self.snippet(int(x[:8], 16)) for x in raw]

    def objective(self, r, binary, locators, volumes):
        f = self.chain(r)
        get = lambda name: f.get(F(name))  # noqa: E731
        definition = self.definitions(get("ObjectiveDefinition"))[0] if get("ObjectiveDefinition") else None
        triggers = bytes.fromhex(get("ObjectiveTriggers")["data"]) if get("ObjectiveTriggers") else bytes(20)
        return {
            "key": r["key"],
            "activation": u32(get("ActivationMode")) if get("ActivationMode") else 0,
            "triggers": list(triggers),
            "fail_first": FAIL_FIRST in f and f[FAIL_FIRST]["data"][:2] != "00",
            "count_kind": struct.unpack(">h", bytes.fromhex(f[COUNT_KIND]["data"])[:2])[0] if COUNT_KIND in f else 0,
            "master": bool(get("MasterObjective")) and get("MasterObjective")["data"][:2] != "00",
            "requires_highlight": bool(get("RequiresHighlight")) and get("RequiresHighlight")["data"][:2] != "00",
            # GetNumCompletedObjectivesForScoring (0x82c6f670) counts only these (absent: true).
            "display_in_hud": not get("DisplayInHUD") or get("DisplayInHUD")["data"][:2] != "00",
            # Film: seconds the film timer gains when the objective completes (objectives_shared.lua 698-712).
            "bonus_time": f32(get("BonusTime")) if get("BonusTime") else 0.,
            "obj": definition,
            # LuaState::tCompiledLua: (source offset, 0, 0x10).
            "points_valid": self.snippet(int(get("PointsValid")["data"][:8], 16)) if get("PointsValid") else None,
            "failures": [d for d in self.definitions(get("FailureObjectives"))] if get("FailureObjectives") else [],
            "ScoringBoundary": decode(get("ScoringBoundary"), binary, locators, volumes) if get("ScoringBoundary") else None,
        }

    def groups(self, g, field, locators, volumes):
        """A challenge_global_data objective-group field: per group its PointRequirement (None: no points objective)
        and objectives (a bare objective refspec is a group of one, as MakeObjectivesFromGroup takes it)."""
        out = []
        for item in items(g.get(F(field), {})):
            r = self.ref(item)
            if r is None:
                out.append(None)
                continue
            if r["class"] == OBJECTIVE:
                out.append({"PointRequirement": None, "objectives": [self.objective(r, self.bin, locators, volumes)]})
                continue
            gf = self.chain(r)
            pr = gf.get(F("PointRequirement"))
            objs = [self.ref(x) for x in items(gf.get(F("Objectives"), {}))]
            out.append({"PointRequirement": struct.unpack(">i", bytes.fromhex(pr["data"])[:4])[0] if pr else None,
                        "objectives": [self.objective(o, self.bin, locators, volumes) if o else None for o in objs]})
        return out

    @functools.cached_property
    def global_locators(self):
        """The disc's global locators (data/content/global_locators/**.rx2, as online_challenge_map reads them), by
        lower-case name: a row's StartLocation that its mission does not place (ots_indu_02_challengelocator_01)."""
        from online_challenge_map import global_locators
        out = {}
        for p in sorted((Path(self.disc) / "data/content/global_locators").rglob("*.rx2")) if self.disc else []:
            out.update(global_locators(p.read_bytes()))
        return out

    def points_objective(self):
        if self.points is None:
            self.points = self.objective(self.by[(OBJECTIVE, GLOBAL_POINTS)], self.bin, {}, {})
        return self.points


TRIGGER_NATIVES = ("ObjectiveTrigger.EnteredVolume", "ObjectiveTrigger.IsInVolume")
# The natives naming a skatercollections trick group (argument 1), and the group records (members by scorable hash).
TRICK_GROUP_NATIVES = ("Tricks.DidSpecificTrickGroup", "Tricks.DidSpecificTrickGroupThisFrame", "Tricks.BeganTrickGroup")
TRICK_GROUP, TRICK_MEMBERS = "Hash_5B47469D2D895DDE", "Hash_033C23A65A34839D"


def trick_groups(db, wanted):
    """skatercollections' trick groups the snippets name: each group's members and its child groups' (as
    skate2port/old_tutorial.trick_groups), by scorable identifier (crates/skate-core/src/scoring/catalog.rs)."""
    names = re.findall(r'\("([a-z0-9_]+)",', (TOOLS.parent / "crates/skate-core/src/scoring/catalog.rs").read_text())
    trick = {"%016X" % hash64(n): n for n in names}
    groups = {r["key"]: r for r in db.collections("skatercollections") if r["class"] == TRICK_GROUP}
    children = {}
    for k, r in groups.items():
        children.setdefault(r["parent"], []).append(k)

    def members(k):
        own = groups[k]["fields"].get(TRICK_MEMBERS)
        out = [trick[i[16:32]] for i in ((items(own) if "array" in own else [own["data"]]) if own else []) if i[16:32] in trick]
        for c in children.get(k, []):
            out += members(c)
        return out
    key = lambda g: g if g.startswith("Hash_") else F(g)  # noqa: E731  an unnamed group (TrackUniqueTrickGroup)
    return {g: sorted(set(members(key(g)))) for g in sorted(wanted) if key(g) in groups}


def objective_entry(objectives, g, fields, locators, volumes):
    """A row's objective groups (`fields` of its challenge_global_data), every native its snippets (and the global
    points objective's) call, and the trigger volumes they name: the natives parse argument 1 as the decimal instance
    id (0x829254f0 tostring, 0x82ae8f78 parse; argument 2, the name, is unused), placed by the row's mission."""
    groups = {f: objectives.groups(g, f, locators, volumes) for f in fields}
    natives, ids, tricks = set(), set(), set()

    def walk(tree):
        if isinstance(tree, list):
            if tree[:1] == ["native"] and tree[1] in TRIGGER_NATIVES and tree[2] and tree[2][0][0] == "str":
                ids.add(tree[2][0][1])
            if tree[:1] == ["native"] and tree[1] in TRICK_GROUP_NATIVES and tree[2] and tree[2][0][0] == "str":
                tricks.add(tree[2][0][1])
            for x in tree:
                walk(x)
    snippets = [objectives.points_objective()]
    for gs in groups.values():
        for group in gs:
            snippets += [o for o in (group or {}).get("objectives", []) if o]
    for o in snippets:
        for s in [o.get("obj"), o.get("points_valid"), *o.get("failures", [])]:
            if s:
                natives.update(s["natives"])
                walk(s["tree"])
    by_id = {str(v["id"]): dict(name=n, **v) for n, v in volumes.items() if "id" in v}
    # tObjectiveFlow {type, count} (group+96 / +100, the activation 0x82c6d168: type 0 activates every objective).
    flow = g.get(F("ObjectiveFlow"))
    return {"groups": groups, "natives": sorted(natives), "trigger_volumes": {i: by_id.get(i) for i in sorted(ids)},
            "trick_groups": sorted(tricks), "flow": list(struct.unpack(">2I", bytes.fromhex(flow["data"][:16]))) if flow else None}


def online_starts(path):
    """{row: {matrix, slots}}: the install's online.json `start` per challenge (online_challenge_map: the global data's
    MapStartLocation else Location, a data/content/global_locators locator)."""
    out = {}

    def walk(x):
        if isinstance(x, dict):
            if isinstance(x.get("id"), str) and isinstance(x.get("start"), dict):
                out[x["id"]] = x["start"]
            for v in x.values():
                walk(v)
        elif isinstance(x, list):
            for v in x:
                walk(v)
    if path.is_file():
        walk(whole_json(path))
    return out


def whole_json(path, tries=300):
    """json.loads of a file that the Skate 2 / Skate 1 online stages may be rewriting in place right now (write_text:
    truncate, then write). What is read then is a prefix of the new file, and no prefix of a JSON object parses but the
    whole one, so read until it parses. (The rows and labels those stages add never reach this script's output, so
    which version is read does not matter.)"""
    import time
    for _ in range(tries):
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except ValueError:  # JSONDecodeError, or UnicodeDecodeError on a cut UTF-8 sequence
            time.sleep(0.1)
    return json.loads(path.read_text(encoding="utf-8"))


def local_bool(chained, key):
    """A Bool of a row's local data (its own value, else its framework chain's) that has no recovered name, by its hash
    `key`. False when the chain does not set it: the attribute's default (the framework's default record holds 00)."""
    v = chained.get(key)
    if v is None:
        return False
    if v["type"] != "EA::Reflection::Bool" or "array" in v:
        raise ValueError(f"{key} is {v['type']}, not a Bool")
    return v["data"][:2] != "00"


def mode_rules(db, missions, template, fields, intro_field=None, heats=False, rounds=False, objectives=False, groups=None,
               global_fields=(), bank=None, local_parents=False, nis_fields=(), objective_indicators=False, bool_fields=None):
    """Every row under `template` (or each of a tuple of templates): title, online, intro (IntroPresentationEvents type
    5, or the row's NIS `intro_field`), the global data's Objectives count and ChallengeType, and the named fields,
    decoded (the row's own value, else its framework template's); with `heats`, race_shared's RaceHeats
    ([`race_heats`]); with `rounds`, contest_shared's ContestRounds ([`contest_rounds`]); with `groups` (global data
    fields), the objective groups ([`objective_entry`], through `bank`, an [`Objectives`]); `global_fields` are global
    data fields, decoded through challengebanks/main.bin; `local_parents`: [`Template`]'s; `nis_fields`: row
    tNISPlaybackDefinitions as [`nis_definition`]s (None when only the template sets one: its scene offset is into the
    framework's strings); `objective_indicators`: each global objective's VisualIndicators ([`global_objectives`]);
    `bool_fields`: {name: hash key} of local data Bools that have no recovered name, each row's value ([`local_bool`])."""
    tpl = Template(db, local_parents)
    out = {}
    found = {}
    for t in (template if isinstance(template, tuple) else (template,)):
        found.update(template_rows(db, t))
    for row, g in found.items():
        rel = f"challenge_local_data/{row}"
        try:
            collection = db.collections(rel)
        except ValueError as e:   # a local data record asset_pipeline/vlt.py cannot read
            print(f"{row}: local data not parsed ({e}) - skipped")
            continue
        record = next(c for c in collection if c["key"] in (row, F(row)) and c["class"] == LOCAL)
        chained, binary, own = tpl.chain(record), db.bin(rel), record["fields"]
        splines = {}
        locators, volumes = mission_placement(missions, row, splines)
        # A row may place on another mission's locators (hom_dwtn_02: hom_dwtn_08_startlocator in missions/hom_dwtn_08/):
        # those come from the mission the name starts with.
        for v in own.values():
            if v.get("type") != "Sk8::Challenge::tLocationID":
                continue
            for x in (items(v) if "array" in v else [v["data"]]):
                name = cstr(binary, int(x[:8], 16))
                if name and name not in locators:
                    other = re.match(r"(.+?)_(?:startlocator|waitlocator)", name)
                    if other:
                        more, more_volumes = mission_placement(missions, other.group(1))
                        locators.update(more)
                        volumes = {**more_volumes, **volumes}
        events =decode(own[F("IntroPresentationEvents")], binary, {}, {}) if F("IntroPresentationEvents") in own else []
        intro = [name for kind, name in events if kind == 5]
        if intro_field and F(intro_field) in own:
            intro = [n for n in [decode(own[F(intro_field)], binary, {}, {})] if n]
        gf = g["fields"] if g else {}
        entry = {
            "title": text_value(gf[TITLE]) if TITLE in gf else None,
            "online": AVAILABLE_ONLINE in gf and gf[AVAILABLE_ONLINE]["data"][:2] != "00",
            "intro": intro[0] if intro else None,
            # challenge_global_data: the Objectives list (the Lua's self.Objectives.Count) and eChallengeTypes.
            "objectives": len(items(gf.get(F("Objectives"), {}))),
            "challenge_type": u32(gf[F("ChallengeType")]) if F("ChallengeType") in gf else None,
        }
        if bank:
            for v in own.values():
                if v.get("type") == "Sk8::Challenge::tLocationID":
                    for x in (items(v) if "array" in v else [v["data"]]):
                        name = cstr(binary, int(x[:8], 16))
                        if name and name not in locators and name.lower() in bank.global_locators:
                            locators[name] = bank.global_locators[name.lower()]
        for name in fields:
            if F(name) in chained:
                entry[name] = decode(chained[F(name)], binary if F(name) in own else None, locators, volumes)
        if "VisualIndicators" in fields:
            # G7: the same VisualIndicators with their look, drawn by look (mode_looks, skate3_indicators::show).
            entry["visual_indicators"] = chevrons(own.get(F("VisualIndicators")), binary, locators, kinds=(0,))
        # The kind-1 VisualIndicators: ribbons along the row's splines (Own the Spot's and Hall of Meat's areas).
        if found_outlines := outlines(own.get(F("VisualIndicators")), splines):
            entry["outlines"] = found_outlines
        for name in nis_fields:
            if F(name) in chained:
                entry[name] = nis_definition(binary, own[F(name)]) if F(name) in own else None
        for name, key in (bool_fields or {}).items():
            entry[name] = local_bool(chained, key)
        if heats:
            entry["heats"] = race_heats(db, rel, own, binary, locators, volumes)
            entry["chevrons"] = chevrons(own.get(F("VisualIndicators")), binary, locators)
            if F("OnlineOutroSoloNIS") in own:
                entry["outro_definition"] = nis_definition(binary, own[F("OnlineOutroSoloNIS")])
        if rounds:
            entry["rounds"] = contest_rounds(db, missions, rel, own, binary, locators, volumes)
        if objectives:
            # An OTL activity's Location is a global locator (otl_dwtn_01_gap_01_locator), not its mission's.
            if local_parents and bank and F("Location") in gf:
                name = cstr(db.bin("challengebanks/main"), u32(gf[F("Location")]))
                if name and name.lower() in bank.global_locators:
                    locators.setdefault(name, bank.global_locators[name.lower()])
            entry["Location"], entry["objective_list"] = global_objectives(db, gf, locators, volumes, objective_indicators)
        for name in global_fields:
            if F(name) in gf:
                entry[name] = decode(gf[F(name)], bank.bin, locators, volumes)
        if groups:
            entry.update(objective_entry(bank, gf, groups, locators, volumes))
            # TrackUniqueTrickGroup (local data; 0x82c5fe60 -> group+120, IsTrackedUniqueTrickGroup 0x82c60630): a
            # skatercollections trick group's key, unset when 0. main() names it when a snippet names it too.
            track = chained.get(F("TrackUniqueTrickGroup"))
            if track and int(track["data"][16:32], 16):
                entry["unique_trick_group"] = "Hash_" + track["data"][16:32]
        missing = [k for k, v in entry.items() if (v is None and k != "challenge_type") or (isinstance(v, list) and None in v)]
        if missing:
            print(f"{row}: unplaced {missing}")
        out[row] = entry
    return out


def nis_definition(binary, v):
    """tNISPlaybackDefinition {scene name offset, flags, 0, 0, position xyz + pad, direction xyz + pad, 4 x 0}: the
    scene and where NIS.Enter*UsingDefinition places it (as skate2port/old_death_race.definition reads Skate 2's).
    An empty one (name "") stays non-nil in retail."""
    w = struct.unpack(">16I", bytes.fromhex(v["data"])[:64])
    fl = lambda x: round(struct.unpack(">f", struct.pack(">I", x))[0], 4)  # noqa: E731
    return {"scene": cstr(binary, w[0]), "at": [fl(x) for x in w[4:7]], "dir": [fl(x) for x in w[8:11]]}


# race_shared's heat, leg and gate fields (RaceHeats -> tRaceHeatDefinition -> Legs -> Gates).
HEAT_FIELDS = ["StartLocation", "TimeLimit", "TimeLimitWarning", "Include321", "KnockoutPerLeg", "NoPushing",
               "WipeOutLimit", "AirUnderRequirement", "KilledItTime", "RaceTerminateVolumes"]
LEG_FIELDS = ["SplitTimeTriggers", "EndTimeBonus"]
GATE_FIELDS = ["GateVolume", "Time_Bonus", "IndicatorType", "RibbonInstance"]


def records(db, rel):
    """An AttribSys reference (class, key) -> the record's fields, from the row's own collection, chained by parent
    (then the framework)."""
    by = {(c["class"], c["key"]): c for c in db.collections(rel)}
    framework = {(c["class"], c["key"]): c for c in db.collections("challenge_local_data_framework")}

    def record(item):
        cls, key = "Hash_" + item[:16], "Hash_" + item[16:32]
        out, r = {}, by.get((cls, key)) or framework.get((cls, key))
        while r:
            for k, v in r["fields"].items():
                out.setdefault(k, v)
            r = by.get((cls, r["parent"])) or framework.get((cls, r["parent"]))
        return out
    return record


# contest_shared's round fields (ContestRounds -> tContestRound).
ROUND_FIELDS = ["ContestVariant", "RunType", "CumulativeScoring", "CumulativeMinScore", "RoundTime", "RoundTimeOnline",
                "StartLocation", "WaitLocation", "SpotVolumes", "RoundBoundary", "PathEndVolume",
                "RestartLastChanceVolume", "RestartOnWipeout", "AIOutOfBoundsVolumes", "SpeedVolumes", "SoloRunNum",
                "AudioOutOfBoundsWarningTime", "AudioRoundEndWarningTime", "ResultsScreenHALIDCol1", "ResultsScreenHALIDCol2",
                "ResultsScreenHALIDCol3", "ResultsScreenHALIDCol4"]


def contest_rounds(db, missions, rel, own, binary, locators, volumes):
    """ContestRounds: per round its fields ([`ROUND_FIELDS`]), the RequiredChallengeHulls count and the
    IntroPresentationEvents (kind, name); `intro` = the first kind-5 (NIS) event's scene; `visual_indicators` its
    VisualIndicators ([`chevrons`]: setup_round's AddIndicator, G7). A name no mission of the row places comes from the
    mission its first three parts name (street_dwtn_01's locators are missions/street_dtwn_01's, street_indu_01's
    missions/street_ind_01's)."""
    record = records(db, rel)
    out = []
    for item in items(own.get(F("ContestRounds"), {})):
        rec = record(item)
        for v in rec.values():
            if v.get("type") not in ("Sk8::Challenge::tLocationID", "Sk8::Challenge::tTriggerVolumeInstanceID",
                                     "Sk8::Challenge::tVisualEditorData"):
                continue
            for x in (items(v) if "array" in v else [v["data"]]):
                # A tVisualEditorData names its locator at +24 (chevrons).
                at = x[48:56] if v["type"] == "Sk8::Challenge::tVisualEditorData" else x[:8]
                name = cstr(binary, int(at, 16)).split("|")
                name = name[1] if len(name) > 1 else name[0]
                other = re.match(r"([a-z]+_[a-z]+_\d+)_", name)
                if name and name not in locators and name not in volumes and other:
                    more, more_volumes = mission_placement(missions, other.group(1))
                    locators.update(more)
                    volumes.update(more_volumes)
        r = {n: decode(rec[F(n)], binary, locators, volumes) for n in ROUND_FIELDS if F(n) in rec}
        r["RequiredChallengeHulls"] = len(items(rec.get(F("RequiredChallengeHulls"), {})))
        events = decode(rec[F("IntroPresentationEvents")], binary, {}, {}) if F("IntroPresentationEvents") in rec else []
        r["IntroPresentationEvents"] = events
        r["intro"] = next((name for kind, name in events if kind == 5), None)
        r["visual_indicators"] = chevrons(rec.get(F("VisualIndicators")), binary, locators)
        out.append(r)
    return out


def chevrons(v, binary, locators, kinds=None):
    """The row's VisualIndicators (setup_heat AddIndicator, 565-573): per row its locator (name offset at +24, its world
    matrix or None when no mission places it) and its ribbon (type key at +40, style key at +64), as
    skate2port/old_death_race.py reads Skate 2's words 10-11 and 16-17. Rows with no locator name place nothing; rows
    with no type (3 of the disc's, key 0) are left out: what the native draws for type 0 is not traced. `kinds`: only
    entries of these kinds (the u32 at +0; 0 a locator, 1 a spline: [`outlines`])."""
    out = []
    for x in items(v) if v else []:
        if kinds is not None and int(x[:8], 16) not in kinds:
            continue
        name = cstr(binary, int(x[48:56], 16))
        if name and int(x[80:96], 16):
            out.append({"locator": name, "matrix": locators.get(name, {}).get("matrix"), "ribbon": x[80:96], "style": x[128:144]})
    return out


def outlines(v, splines):
    """The row's kind-1 VisualIndicators: a ribbon along its spline (0xEB0021, the id at +16), closed with flags 8,
    whatever its type and style keys (0x82d4f760-0x82d4f7b4 read neither). S.K.A.T.E.'s blue outline of the trick
    area on the ground (skate_indu_03; skate_indu_02's second entry)."""
    out = []
    for x in items(v) if v else []:
        spline = splines.get(int(x[32:48], 16)) if int(x[:8], 16) == 1 else None
        if spline and spline["points"]:
            out.append({"points": spline["points"], "closed": bool(spline["flags"] & 8)})
    return out


def race_indicators(disc, vault, rows, out):
    """The ribbons race_shared draws (each gate's RibbonInstance, RaceGateGreyRibbon / RaceFinalGateRibbon /
    RaceLegFinishGateRibbon, the chevrons), as [`indicator_looks`] resolves them."""
    looks = [(r["ribbon"], r["style"]) for row in rows.values() for r in
             [row.get(k) for k in ("RaceGateGreyRibbon", "RaceFinalGateRibbon", "RaceLegFinishGateRibbon")]
             + [gate.get("RibbonInstance") for h in row.get("heats", []) for leg in h["Legs"] for gate in leg["Gates"]]
             + row.get("chevrons", []) if r]
    return indicator_looks(disc, vault, looks, out)


def skate_indicators(disc, vault, rows, out):
    """S.K.A.T.E.'s indicators.json (skate2_arrows.rs's format): per row's VisualIndicators locator its shape and style,
    namespaced beside Skate 2's keys (both games hash "default" alike), the textures by asset path."""
    looks = indicator_looks(disc, vault, [(c["ribbon"], c["style"]) for r in rows.values() for c in r["visual_indicators"]], out)
    folder, ns = f"{OUT_ROOT}/skate/indicator", "skate3:{}".format
    return {"shapes": {ns(k): v for k, v in looks["shapes"].items()},
            "styles": {ns(k): {"texture": f"{folder}/{v['texture']}", "tint": v["tint"],
                               "background": v["background"] and f"{folder}/{v['background']}"} for k, v in looks["styles"].items()},
            "locators": {c["locator"]: {"shape": ns(c["ribbon"]), "style": ns(c["style"])}
                         for r in rows.values() for c in r["visual_indicators"] if c["matrix"]}}


def indicator_looks(disc, vault, looks, out):
    """Per indicator type key (`looks`: (type, style) pairs) its outline (miscload.big visual_indicators_sk82.rx2, the
    type's model's asset), per style key its textures and tint (miscboot.big vitextures.rx2), through the
    skatercollections vault, read as skate2port/old_death_race.py reads Skate 2's. Writes out/indicator/<texture>.png and
    returns {"shapes", "styles"}."""
    src = (TOOLS / "pipeline/skate2port/old_indicators.py")
    g = {"__file__": str(src), "__name__": "__old_indicators__"}
    argv, sys.argv = sys.argv, ["x", str(TOOLS.parent), "", str(out)]
    try:
        exec(compile(src.read_text(encoding="utf-8"), str(src), "exec"), g)
    finally:
        sys.argv = argv
    vrows = {(c["class"], c["key"]): c for c in whole_json(vault)["collections"]}

    def field(cls, key, name):
        while (c := vrows.get((cls, key))) is not None:
            if name in c["fields"]:
                return c["fields"][name]["data"]
            key = c["parent"]
        return None
    # A named record's key is its name and a reference to it the name's hash (S.K.A.T.E.'s "arrow_medium").
    named = {"%016X" % hash64(k): k for _, k in vrows if not k.startswith("Hash_")}
    vault_key = lambda k: named.get(k, "Hash_" + k)  # noqa: E731  a record keyed by its name when the vault knows it
    misc = big(disc, "data/big/miscboot.big")
    textures, folder = {}, out / "indicator"
    folder.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        rx2 = Path(tmp) / "vitextures.rx2"
        rx2.write_bytes(read(misc, "data/content/textures/vitextures.rx2"))
        for t in g["read_textures"](rx2)[1]:
            name = t.name.removesuffix(".Texture")
            if name.startswith(("visual_indicator_", "volume_indicator_")):
                pixels = g["rgba"](t).copy()
                # The colour swatches are tints (alpha unused); the background keeps its own.
                if name.startswith("visual_indicator_") and name != "visual_indicator_background":
                    pixels[..., 3] = 255
                g["Image"].fromarray(pixels).save(folder / f"{name}.png")
                textures[t.name] = f"{name}.png"
    outlines = g["outlines"](read(big(disc, "data/big/miscload.big"), "data/content/world/models/visual_indicators_sk82.rx2"))

    def style(key):
        k = vault_key(key)
        tint = field(g["STYLE"], k, "Hash_E4F3418597AACAA5")
        return {"name": named.get(key), "texture": textures[field(g["STYLE"], k, "Hash_8E38A3CD0186F8DA")],
                "background": textures.get(field(g["STYLE"], k, "Hash_FF4DCA217E05B9BC")),
                "tint": list(struct.unpack(">4f", bytes.fromhex(tint))) if tint else [1, 1, 1, 1]}

    def shape(key):
        model = field(g["TYPE"], vault_key(key), "Hash_8D090C8B5931547A")[16:32]
        return outlines[field(g["MODEL"], vault_key(model), "Hash_1AFE1B981FB92727")]
    return {"shapes": {k: shape(k) for k in sorted({r for r, _ in looks})},
            "styles": {k: style(k) for k in sorted({s for _, s in looks})}}


def race_heats(db, rel, own, binary, locators, volumes):
    """RaceHeats: each an AttribSys reference (class, key) to a record in the row's own collection, chained by parent
    (then the framework); its Legs and their Gates likewise. Offsets resolve through the row's .bin."""
    record = records(db, rel)

    def fields(rec, names):
        return {n: decode(rec[F(n)], binary, locators, volumes) for n in names if F(n) in rec}
    heats = []
    for h in items(own.get(F("RaceHeats"), {})):
        rec = record(h)
        heat = fields(rec, HEAT_FIELDS)
        for n in ("NISFlythroughDefinition", "NISFlythroughDefinitionOnline"):
            if F(n) in rec:
                heat[n] = nis_definition(binary, rec[F(n)])
        heat["OnlineNISs"] = decode(rec[F("OnlineNISs")], binary, {}, {}) if F("OnlineNISs") in rec else []
        heat["MusicPace"] = len(items(rec.get(F("MusicPace"), {})))
        heat["Legs"] = []
        for leg_item in items(rec.get(F("Legs"), {})):
            leg_rec = record(leg_item)
            leg = fields(leg_rec, LEG_FIELDS)
            leg["Gates"] = [fields(record(g), GATE_FIELDS) for g in items(leg_rec.get(F("Gates"), {}))]
            heat["Legs"].append(leg)
        heats.append(heat)
    return heats


# HUD2.LeaderboardHUDSetVariantType's strings by eContestType (0x828519e8: tables 0x83025d30, and 0x83025d78 for the
# _JAM ones; ID_COMMON_UNKNOWN past the end and for AIR_LINE's _JAM).
CONTEST_VARIANTS = ["SEQUENCE", "SEQUENCE_GRAB", "SEQUENCE_GRIND", "SEQUENCE_FLIP", "SEQUENCE_FOOTPLANT",
                    "SEQUENCE_HANDPLANT", "SEQUENCE_HIPPYJUMP", "GRIND_LENGTH", "SPEED", "AIR_DISTANCE",
                    "AIR_GRAB_TIME", "AIR_HEIGHT", "AIR_SPIN", "AIR_TIME", "AIR_LINE", "AIR_ONLY", "GRIND_ONLY"]

# objectives_shared's row fields (its `self.` reads, less the audio and Follow the Leader ones).
FILM_FIELDS = ["StartLocation", "AltStartLocation", "AllowAltStartLocation", "UseFilmSetupTime", "SpawnOffboard",
               "VisualIndicators", "CountdownDelay", "PrefilmTime", "TotalFilmTime", "FilmTimerLowLimit",
               "OwnedItHUDDisplayTime", "KilledItHUDDisplayTime", "OutOfBoundsTime", "ChallengeBoundary",
               "PhotoChallengeBoundary", "AllowPlayerBail", "AllowObjectiveFailure", "AllowSessionMarkersToBeSet",
               "UseLineTimer", "ResetObjectivesOnPlayerUseSessionMarker", "AudioDefineChallengeWinLowTime",
               "TimeToWaitBeforeReplay", "HighlightKillOnlyInMasterObjective", "HighlightKillItObjectiveNum",
               "PointRequirement", "Competitors", "ModifiedCentralMessageHALIDTimeUp",
               "ModifiedCentralMessageHALIDOutOfBounds", "ModifiedCentralMessageHALIDScoredLow",
               "ModifiedCentralMessageHALIDLineTimerExpired", "CentralMessageHALIDReturnToChallengeArea",
               "CentralMessageHALIDChallengeFailed"]
FILM_STRINGS = ["ID_RACE_SECONDS_ADDED", "ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER",
                "ID_LEADERBOARD_HEADER_COMPLETION", "ID_LEADERBOARD_HEADER_TIME", "ID_COMMON_OWNED",
                "ID_CELEBRATE_KILLED", "ID_ONLINE_SOLO_RESULTS_SUB_HEADER_SESSION_TIME",
                # SimulPlay.RegisterHUD's DPO title (0x82564c10).
                "ID_DPO_PLAYERSFINISHED"]

# Own the Lot's activity templates and their graphs (otl_indu_03_tag_01 is an accumulation by its template).
OTL_GRAPHS = {"freeskate_activities_accumulation": "freeskate_ol_accumulation",
              "freeskate_activities_gap_tag": "freeskate_ol_gap_tag",
              "freeskate_activities_simultrick": "freeskate_ol_simultaneous_trick",
              "freeskate_activities_survival": "freeskate_ol_survival",
              "freeskate_activities_tricklist": "freeskate_ol_trick_list"}
ONLINE_FREESKATE_ACTIVITIES = "Hash_ABE749C80E8ADBCE"   # own_the_lots global data: the lot's activities (refspecs)

# Per mode: the challengebanks template, the graph, the row fields the port reads, and the strings the Lua names.
MODES = {
    "1up": dict(
        template="1ups", graph="1up", dlc=True,
        fields=["TimeLimit", "CumulativeScoring", "AllowPlayerBail", "UseStaticCameras", "TeamIntroductionDelay",
                "StartLocationRotation", "WaitLocationRotation", "CameraLocations",
                "CentralMessageHALIDDidNotBeatHighScore", "CentralMessageHALIDNameBailed",
                "CentralMessageHALIDRoundOver", "CentralMessageHALIDSetScore", "CentralMessageHALIDYourTurn",
                "ModifiedCentralMessageHALIDBailed", "ModifiedCentralMessageHALIDOneUpped"],
        strings=["ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ELIMINATED", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ISLEADING",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_NEWSCORESET", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHER_OUTFORROUND", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORENOTSET",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORESET", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SETYOURSCORE",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS", "ID_CHALLENGE_CENTRAL_MESSAGE_ROUNDOVER", "ID_COMMON_SCORE",
                 "ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_NAME_HEADER",
                 "ID_ONLINE_EVENT_LEAVE"]),
    # spot_battle < turn_based_uniform: the intro is the row's NISFlythroughDefinition (nis_flythrough).
    "spot_battle": dict(
        template="own_the_spots_battles", graph="spot_battle", intro_field="NISFlythroughDefinition", dlc=True,
        fields=["TimeLimit", "NumRounds", "NumSequencesToEndTurn", "CumulativeScoring", "CountdownDelay",
                "TeamIntroductionDelay", "TurnBasedAttemptSpawnPoint", "TurnBasedWaitingLocation", "SpotVolumes",
                "ScoringBoundary", "VisualIndicators", "CentralMessageHALIDYourTurn",
                "CentralMessageHALIDYourTurnTeam", "CentralMessageHALIDChallengeComplete",
                "CentralMessageHALIDSpotOwned", "CentralMessageHALIDBonusObjectiveComplete"],
        # The team intro (dynamic_character_nis: NIS.EnterNISUsingDefinition(self.IntroNIS) with teams).
        nis_fields=("IntroNIS",),
        strings=["ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS",
                 "ID_COMMON_SCORE", "ID_CONTEST_CENTRALMESSAGE_ROUNDNUMBER",
                 "ID_DOMINATION_CENTRAL_MESSAGE_SCOREINCREASED", "ID_LEADERBOARD_HEADER_RANK",
                 "ID_LEADERBOARD_NAME_HEADER", "ID_LEADERBOARD_POINTS_HEADER", "ID_PLAYERS_CHOICE",
                 "ID_VARIANT_SESSIONSPOT", "ID_SCORE_NOT_COUNTED"]),
    # hall_of_meat (HOM Battle) < turn_based_uniform, as spot_battle; scored by Trick.GetHOMScore at wipeoutover.
    "hall_of_meat": dict(
        template="hall_of_meats_battles", graph="hall_of_meat", intro_field="NISFlythroughDefinition",
        fields=["TimeLimit", "NumRounds", "NumSequencesToEndTurn", "CumulativeScoring", "CountdownDelay",
                "TeamIntroductionDelay", "TurnBasedAttemptSpawnPoint", "TurnBasedWaitingLocation",
                "VisualIndicators", "CentralMessageHALIDYourTurn",
                "CentralMessageHALIDYourTurnTeam", "CentralMessageHALIDChallengeComplete",
                "CentralMessageHALIDOutOfTime"],
        nis_fields=("IntroNIS",),
        strings=["ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS",
                 "ID_COMMON_SCORE", "ID_CONTEST_CENTRALMESSAGE_ROUNDNUMBER", "ID_LEADERBOARD_HEADER_RANK",
                 "ID_LEADERBOARD_NAME_HEADER", "ID_LEADERBOARD_POINTS_HEADER", "ID_PLAYERS_CHOICE",
                 "ID_VARIANT_SESSIONSPOT", "ID_SCORE_NOT_COUNTED"]),
    # Career Hall of Meat (hom_challenge < objectives_multiplayer), the hall_of_meats rows played online as SimulPlay:
    # the row's fields, both objective groups (ObjectiveFlow per row: hom_dwtn_03_sp and hom_indu_03 are type 1), the
    # enum constants their snippets name, the intro its IntroPresentationEvents' NIS. `wipeout_resets_active`: the row's
    # WIPEOUT_RESETS_ACTIVE (true on the four coop rows with a points requirement; read from each row, never listed).
    "hom": dict(
        template="hall_of_meats", graph="hom_challenge", groups=("ObjectivesOwned", "ObjectivesKilledIt"),
        constants=True, dlc=True, bool_fields={"wipeout_resets_active": WIPEOUT_RESETS_ACTIVE},
        fields=["TimeLimit", "CountdownDelay", "IsCoop", "ScaleFactorTime", "ScaleFactorScore", "OwnedItHUDDisplayTime",
                "KilledItHUDDisplayTime", "StartLocation", "ChallengeBoundary", "OutOfBoundsTime",
                "CentralMessageHALIDReturnToChallengeArea", "ModifiedCentralMessageHALIDScoredLow",
                "OutroPresentationEvents", "DisableHOM", "TeamIntroductionDelay"],
        strings=["ID_DPO_PLAYERSFINISHED", "ID_CHALLENGE_FREESKATEACTIVITY_GAP_HURRYUP",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_MODIFIED_1UP_SCOREDLOW",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_OBJECTIVE_RETURNTOCHAL"]
        + [f"ID_ONLINE_SOLO_RESULTS_SUB_HEADER_{k}" for k in ("SIMUL_FAIL", "COOP_FAIL", "OWNED", "KILLED", "FAILED",
                                                               "COMPLETE", "SESSION_TIME")]
        + ["ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_HEADER_OBJECTIVES",
           "ID_LEADERBOARD_HEADER_TIME", "ID_COMMON_SCORE"]),
    # Death Race (race_shared): the row's fields, its RaceHeats, and per heat the online start NIS (OnlineNISs) placed
    # by its online definition. The intro is setup_heat's online NIS, not a presentation (IntroPresentationEvents empty).
    "race": dict(
        template="races", graph="race_shared", heats=True,
        fields=["RaceType", "RaceGateSkipable", "PlaceHigherToWin", "RaceTrickForTimeMultiplier", "MaxMiniMapIcons",
                "CountdownDelay", "TeamIntroductionDelay", "OwnedItHUDDisplayTime", "KilledItHUDDisplayTime",
                "OnlineOutroSoloNIS", "OnlineEndCameraLocation", "CentralMessageHALIDChallengeFailed",
                "CentralMessageHALIDTeamIndication", "RaceGateGreyRibbon", "RaceFinalGateRibbon",
                "RaceLegFinishGateRibbon"],
        strings=["ID_RACE_VARIANT_DOWNHILL", "ID_RACE_VARIANT_TRICKFORTIME", "ID_RACE_FINISHED",
                 "ID_ONLINE_EVENT_CHALLENGE_FINISH", "ID_RACE_GATES_COMPLETED", "ID_RACE_SECONDS_ADDED",
                 "ID_CHALLENGE_COMMON_RACE_TIMES", "ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER",
                 "ID_LEADERBOARD_HEADER_TIME", "ID_CHALLENG_RESULT_STATUS_PASS", "ID_CHALLENG_RESULT_STATUS_WON",
                 "ID_CHALLENG_RESULT_STATUS_LOSE", "ID_RACE_DNF", "ID_CHALLENGE_COMMON_RACE_STANDINGS",
                 "ID_LEADERBOARD_HEADER_RACE_POINTS", "ID_DPO_POSITION"]),
    # Street and Vert contests (contest_shared): the row's fields and its ContestRounds, each round's intro the NIS of
    # its IntroPresentationEvents.
    "contest": dict(
        template=("street", "vert"), graph="contest_shared", rounds=True, dlc=True,
        fields=["ContestElimination", "AIOnline", "TeamIntroductionDelay", "CentralMessageHALIDFinalRound",
                "CentralMessageHALIDRoundNum", "CentralMessageHALIDReturnToChallengeArea",
                "CentralMessageHALIDNewLead", "CentralMessageHALIDNewLeadYou", "CentralMessageHALIDTeamIndication"],
        strings=["ID_CHALLENGE_COMMON_CONTEST_STANDINGS", "ID_CHALLENGE_COMMON_ROUND_RESULTS", "ID_CONTEST_WRONG_TYPE",
                 "ID_COMMON_DISTANCE", "ID_COMMON_UNKNOWN", "ID_CHALLENG_RESULT_STATUS_WON",
                 "ID_CHALLENG_RESULT_STATUS_LOSE"]
        + [f"ID_CONTEST_VARIANT_{v}{jam}" for v in CONTEST_VARIANTS for jam in ("", "_JAM")]),
    # Domination (domination < objectives_multiplayer): the row's fields, its Location and Objectives (global data),
    # the intro its IntroPresentationEvents' NIS; the Objectives as data (Objectives.MakeObjectives' group: one
    # objective per spot, each its own one-objective group here, flattened by the port).
    "domination": dict(
        template="dominations", graph="domination", objectives=True, groups=("Objectives",), dlc=True,
        fields=["TimeLimit", "CountdownDelay", "TeamIntroductionDelay", "CentralMessageHALIDDominationOwned",
                "CentralMessageHALIDDominationLostSpot", "CentralMessageHALIDDominationScoreIncreased",
                "CentralMessageHALIDNewLead", "CentralMessageHALIDNewLeadYou", "CentralMessageHALIDTeamIndication",
                "AIAutoBalanceTeams", "RibbonColoursTeams", "RibbonColoursPlayers", "RibbonColourNeutral",
                "RibbonIndicatorNeutral", "RibbonIndicatorOwned"],
        # The team intro NIS and the objectives' ribbons (domination.lua: dynamic_character_nis, the team colours).
        nis_fields=("IntroNIS",), objective_indicators=True,
        strings=["ID_DOMINATION_CENTRAL_MESSAGE_DOMINATING", "ID_LEADERBOARD_HEADER_RANK",
                 "ID_LEADERBOARD_HEADER_SKATER", "ID_COMMON_SCORE", "ID_LEADERBOARD_POINTS_HEADER",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_TEAMINDICATION", "ID_CHALLENGE_COMMON_ROUND_RESULTS"]),
    # Own the Spot (ownthespot_shared_mlui < objectives < default), a Career row played as SimulPlay co-op: the row's
    # fields, the ObjectivesOwned group (made with MakeObjectivesFromGroup) and the ObjectivesKilledIt group's
    # PointRequirement (KilledItScore), the global points objective, the boundary (global data), the intro NIS.
    "ots": dict(
        template="own_the_spots", graph="ownthespot_shared_mlui", groups=("ObjectivesOwned", "ObjectivesKilledIt"),
        global_fields=("OTSTriggerBoundary",), dlc=True,
        fields=["StartLocation", "OTSScoringBoundary", "OwnedItHUDDisplayTime", "KilledItHUDDisplayTime",
                "OutOfBoundsTime", "CentralMessageHALIDReturnToChallengeArea", "VisualIndicators"],
        strings=["ID_LEADERBOARD_NEW_PERSONAL_BEST", "ID_SIMULPLAY_OTS_HIGHSCORE", "ID_SIMULPLAY_OTS_KILLED",
                 "ID_SIMULPLAY_OTS_OWNED", "ID_SIMULPLAY_OTS_ENDING_SESSION", "ID_CHALLENGE_OTS_TARGET_NO_LEADER",
                 "ID_CHALLENGE_OTS_TARGET_SESSION_BEST", "ID_CHALLENGE_OBJECTIVE_BEAT_PERSONALSCORE",
                 "ID_CHALLENGE_OBJECTIVE_BEAT_OWNSPOT", "ID_CHALLENGE_OBJECTIVE_BEAT_KILLSPOT",
                 "ID_CHALLENGE_OBJECTIVE_BEAT_SESSION_BEST", "ID_OTS_OBJECTIVE_FRIEND", "ID_COMMON_TARGET",
                 "ID_DPO_PERSONALBEST", "ID_ONLINE_SOLO_RESULTS_SUB_HEADER_SESSION_TIME", "ID_LEADERBOARD_HEADER_RANK",
                 "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_STATUS_HEADER", "ID_COMMON_SCORE",
                 "ID_CROSSBAR_OTS_RETURN_TO_LOBBY", "ID_ONLINE_HELPER_MESSAGE_END_OTS",
                 # The SimulPlay results status column (skate3_ots: the level as Owned / Killed).
                 "ID_COMMON_OWNED", "ID_CELEBRATE_KILLED"]),
    # Film and Photo (objectives_shared < objectives < default; photo_skate2 overrides display_photo and
    # challenge_cleanup), Career rows played as SimulPlay (QuantifiedCompletion): skate3_film.rs. The row's fields
    # objectives_shared reads, both objective groups (each objective's BonusTime too), the global points objective,
    # the trick groups the snippets name, the intro NIS.
    "film": dict(template="films", graph="objectives_shared", groups=("ObjectivesOwned", "ObjectivesKilledIt"), dlc=True,
                 fields=FILM_FIELDS, strings=FILM_STRINGS),
    "photo": dict(template="photos", graph="photo_skate2", groups=("ObjectivesOwned", "ObjectivesKilledIt"), dlc=True,
                  fields=FILM_FIELDS, strings=FILM_STRINGS),
    # Own the Lot (skate3_otl.rs): the lots' activities, each a challenge on one of the five freeskate_ol graphs (by
    # its template, OTL_GRAPHS), its fields (local data through the activity template's), Location and Objectives
    # (global data: Objectives.MakeObjectives' group, and simultrick's "ObjectivesSolo"); `lots` (main) lists each
    # lot's OnlineFreeSkateActivities in order. The DLC lots (otl_artg_01, otl_dwmc_01 / 02, otl_mmcn_01 / 02) are the
    # packages' own templates (package_templates, otl_lots).
    "otl": dict(template=tuple(OTL_GRAPHS), graph="freeskate_ol", objectives=True, groups=("Objectives", "ObjectivesSolo"), dlc=True,
                global_fields=("FreeskateActivityType", "FreeskateSurvivalType"), local_parents=True,
                fields=["TimeLimit", "CountdownDelay", "OutOfBoundsTime", "ChallengeBoundary",
                        "CentralMessageHALIDReturnToChallengeArea", "FreeskateGoalAdjustment",
                        "FreeskateAccumulationType", "FreeskateSurvivalType", "FreeskateRequiredPoints",
                        "FreeskateRequiredTime", "FreeskateTolerance", "SpotVolumes", "VisualIndicators", "DisableHOM"],
                strings=["ID_CHALLENGE_FREESKATEACTIVITY_GAP_YOUFINISHEDFIRST", "ID_CHALLENGE_FREESKATEACTIVITY_GAP_YOUFINISHED",
                         "ID_CHALLENGE_FREESKATEACTIVITY_GAP_GAMERTAGFINISHEDFIRST",
                         "ID_CHALLENGE_FREESKATEACTIVITY_GAP_GAMERTAGFINISHED", "ID_CHALLENGE_FREESKATEACTIVITY_GAP_HURRYUP",
                         "ID_DPO_PLAYERSFINISHED", "ID_FSA_ACCUMULATION_TARGET", "ID_FSA_SURVIVAL_TARGET",
                         "ID_DPO_OBJECTIVESCOMPLETED", "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_YOUSKATEDSLOW",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_TOOSLOW",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_YOULEFTAREA",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_LEFTAREA",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_YOUBAILED",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_BAIL",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_YOUWENTOFFBOARD",
                         "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_OFFBOARD", "ID_LEADERBOARD_HEADER_RANK",
                         "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_HEADER_OBJECTIVES", "ID_LEADERBOARD_HEADER_TIME",
                         "ID_COMMON_SCORE", "ID_CROSSBAR_ONLINE_OTL_SELECT_ACTIVITY",
                         "ID_CHALLENGE_FREESKATEACTIVITY_ACTIVITYPROPOSED", "ID_EVENTLOG_FSA_PROPOSAL_FAILURE_1",
                         "ID_EVENTLOG_FSA_PROPOSAL_FAILURE_2"]),
}


# The shared HUD's own strings: the DPO's denominators (0x8266af68: Points, Ints).
# The DPO, and the online core's: endchallenge_online's status line (Mission.GetResultsStatusText) and the round sync
# point's wait (Online.OnlineChallengeRoundSyncPoint). The team names (Teams.GetName's EA Online names are gone:
# retail's unranked team names, online_moves::TEAM_NAMES).
HUD_STRINGS = ["ID_DPO_STR_POINTS", "ID_COMMON_NUMBER", "ID_ONLINE_RESULTS_SCREEN_MESSAGE_RETURN_TO_LOBBY",
               "ID_ONLINE_CHALLENGE_WAITING_FOR_OTHER_SKATERS", "ID_UNRANKEDNAME_TEAM1", "ID_UNRANKEDNAME_TEAM2"]


MESSAGES = ["CentralMessageHALIDChallengeComplete", "CentralMessageHALIDChallengeFailed",
            "CentralMessageHALIDNameBailed", "CentralMessageHALIDNameCopiedTrick",
            "CentralMessageHALIDNameCopyTrickName", "CentralMessageHALIDNameDidNotTrickValidArea",
            "CentralMessageHALIDNameGetsALetter", "CentralMessageHALIDNameHasSetTrickName",
            "CentralMessageHALIDNameNotCopyTrick", "CentralMessageHALIDNameRepeatedTrick",
            "CentralMessageHALIDNameSetTrick", "CentralMessageHALIDNewLead", "CentralMessageHALIDNewLeadYou",
            "CentralMessageHALIDOutOfTime", "CentralMessageHALIDRoundOver"]


# ---------------------------------------------------------------- the ICE parameter table (emulated from the exe)
class Image:
    def __init__(self, data):
        self.d = data

    def u32(self, va):
        return struct.unpack_from(">I", self.d, va - BASE)[0]

    def f32(self, va):
        return struct.unpack_from(">f", self.d, va - BASE)[0]

    def cstr(self, va):
        return cstr(self.d, va - BASE)


def bl_target(a, w):
    return (a + ((w & 0x3FFFFFC) - (0x4000000 if w & 0x2000000 else 0))) & 0xFFFFFFFF


def lua_enum(img, name):
    """A Lua enum's {key: value} from its builder, found through its native registration pair {name, builder}: the
    string in r4 at each call (lua_pushlstring; registers followed through lis / addi / mr), then the next lfs constant
    (disc: EBoneIndex 0x82c3e6a0, EDamageLevel 0x82c3f030, EWipeoutBodyTweakIndex 0x82c3f2a8, eObjectiveStatus
    0x82c3a270; TU3 0x82c67300 / 0x82c67c90 / 0x82c67f08 / 0x82c62e88 read the same). The builder ends at blr or a
    tail branch."""
    s = img.d.find(b"\0" + name.encode() + b"\0") + 1 + BASE
    assert s > BASE, f"no {name} in the image"
    at = next(a for a in range(BASE, BASE + len(img.d) - 8, 4) if img.u32(a) == s)
    reg, key, out, a = {}, None, {}, img.u32(at + 4)
    while True:
        w = img.u32(a)
        op, rd, ra, rb, imm = w >> 26, (w >> 21) & 31, (w >> 16) & 31, (w >> 11) & 31, w & 0xFFFF
        simm = imm - 0x10000 if imm & 0x8000 else imm
        if w == 0x4E800020 or (op == 18 and not w & 1):
            return out
        if op in (14, 15):
            base = 0 if ra == 0 else reg.get(ra)
            reg[rd] = None if base is None else (base + (simm if op == 14 else imm << 16)) & 0xFFFFFFFF
        elif op == 31 and (w >> 1) & 0x3FF == 444 and rd == rb:   # mr
            reg[ra] = reg.get(rd)
        elif op == 18:
            v = reg.get(4)
            key = img.cstr(v) if v and BASE <= v < BASE + len(img.d) else None
        elif op == 48 and key and reg.get(ra) is not None:     # lfs
            f = img.f32((reg[ra] + simm) & 0xFFFFFFFF)
            out[key], key = int(f) if f == int(f) else f, None
        a += 4


def lua_constants(img, rows):
    """{"NS.Enum.Value": value} for every constant the rows' snippets name (their natives' "const:" entries)."""
    names = sorted({n[6:] for r in rows.values() for n in r.get("natives", []) if n.startswith("const:")})
    enums = {e: lua_enum(img, e) for e in {n.split(".")[-2] for n in names}}
    return {n: enums[n.split(".")[-2]].get(n.split(".")[-1]) for n in names}


def ice_registration(img):
    """The function that registers the full ICE table: the one building the most addresses into the name block
    holding LENS_USE_DYNAMIC (TU3: 0x82903840..0x82908208), and the four functions it calls (TU3: allocate
    0x829037f0, file 0x828f5b78, float parameter 0x82c8e7c0, integer parameter 0x828f5d38)."""
    name = img.d.find(b"\0LENS_USE_DYNAMIC\0") + 1 + BASE
    assert name > BASE, "no LENS_USE_DYNAMIC in the image"
    lo, hi = name - 0x1400, name + 0x1400
    reg, refs = {}, []
    for a in range(BASE, BASE + len(img.d) - 4, 4):
        w = img.u32(a)
        op, rd, ra, imm = w >> 26, (w >> 21) & 31, (w >> 16) & 31, w & 0xFFFF
        if op == 15 and ra == 0:
            reg[rd] = (imm << 16) & 0xFFFFFFFF
        elif op == 14 and ra and ra in reg:
            v = (reg[ra] + (imm - 0x10000 if imm & 0x8000 else imm)) & 0xFFFFFFFF
            if lo <= v < hi:
                refs.append(a)
            if rd != ra:
                reg[rd] = v
        elif op == 18 and w & 1:
            reg.clear()  # a call ends what the scan can follow
    refs.sort()
    # The densest run of references (gaps under 0x400): the registration's body.
    runs, run = [], [refs[0]]
    for a in refs[1:]:
        if a - run[-1] < 0x400:
            run.append(a)
        else:
            runs.append(run)
            run = [a]
    runs.append(run)
    body = max(runs, key=len)
    start = body[0]
    while img.u32(start) != 0x7D8802A6:   # mflr r12
        start -= 4
    end = body[-1]
    while img.u32(end) != 0x4E800020:     # blr
        end += 4
    calls = {}
    for a in range(start, end, 4):
        w = img.u32(a)
        if w >> 26 == 18 and w & 1:
            t = bl_target(a, w)
            calls[t] = calls.get(t, 0) + 1
    head = lambda t: [img.u32(t + 4 * k) for k in range(24)]  # noqa: E731
    file_ = next(t for t in calls if img.u32(t) == 0x2B040000)              # cmplwi cr6,r4,0
    float_ = next(t for t in calls if 0xD0230020 in head(t))                # stfs f1,32(r3)
    int_ = next(t for t in calls if 0x91430020 in head(t))                  # stw r10,32(r3)
    alloc = max((t for t in calls if t not in (file_, float_, int_)), key=calls.get)
    return start, end, alloc, file_, float_, int_


def ice_table(img):
    """Emulates the registration linearly, as scratch s3params.py did for TU3: each parameter object gets name @8,
    track @0x10, flag @0x14, type @0x18, bits @0x1c, default @0x20, min @0x24, max @0x28, cubic @0x3c and tension
    @0x40, either inline (`stw`/`stfs` into r3) or through the float or integer constructor (r4..r10, f1..f3, stack
    args), and is filed by `table[r3] = r4` or `stw r3,16+4*pos(r28)`. Type 4 (f32) is stored whole: 32 bits."""
    start, end, alloc, file_, float_, int_ = ice_registration(img)
    r, f, cur, table, mem, fmem, last = {}, {}, None, {}, {}, {}, None
    for a in range(start, end, 4):
        w = img.u32(a)
        op, rd, ra, rb = w >> 26, (w >> 21) & 31, (w >> 16) & 31, (w >> 11) & 31
        imm = w & 0xFFFF
        simm = imm - 0x10000 if imm & 0x8000 else imm
        g = r.get
        if op == 14:
            r[rd] = simm & 0xFFFFFFFF if ra == 0 else ((g(ra) + simm) & 0xFFFFFFFF if g(ra) is not None else None)
        elif op == 15:
            r[rd] = (imm << 16) & 0xFFFFFFFF if ra == 0 else ((g(ra) + (imm << 16)) & 0xFFFFFFFF if g(ra) is not None else None)
        elif op == 24:
            r[ra] = (g(rd) | imm) if g(rd) is not None else None
        elif op == 48:  # lfs; from the stack: what an earlier stfs saved there
            f[rd] = fmem.get(simm) if ra == 1 else img.f32((g(ra) + simm) & 0xFFFFFFFF) if g(ra) is not None else None
        elif op == 36:
            if ra == 3:
                cur = cur if cur is not None else {}
                cur[simm] = g(rd)
            elif ra == 28 and rd == 3:
                table[(simm - 16) // 4] = cur
                cur = None
            elif ra == 1:
                mem[simm] = g(rd)
        elif op == 52 and ra == 1:
            fmem[simm] = f.get(rd)
        elif op == 52 and ra == 3:
            cur = cur if cur is not None else {}
            cur[("f", simm)] = f.get(rd)
        elif op == 38 and ra == 3:
            cur = cur if cur is not None else {}
            cur[("b", simm)] = g(rd)
        elif op == 32:  # lwz; from the stack: what an earlier stw saved there (a base kept across calls)
            r[rd] = mem.get(simm) if ra == 1 else None
        elif op == 63 and ((w >> 1) & 0x3FF) == 72:
            f[rd] = f.get(rb)
        elif op == 31 and ((w >> 1) & 0x3FF) == 444:
            r[ra] = g(rd) if rd == rb else None
        elif op == 31 and ((w >> 1) & 0x3FF) in (266, 23, 235, 40):
            r[rd] = None
        elif op == 7:
            r[rd] = None
        elif op == 18 and w & 1:
            t = bl_target(a, w)
            if t == float_:
                last = {8: g(4), 12: g(5), 0x10: g(6), 0x14: g(7), 0x18: g(8), 0x1C: g(9), ("f", 0x20): f.get(1),
                        ("f", 0x24): f.get(2), ("f", 0x28): f.get(3), 0x3C: mem.get(100), 0x40: mem.get(108)}
            elif t == int_:
                last = {8: g(4), 12: g(5), 0x10: g(6), 0x14: g(7), 0x18: g(8), 0x1C: g(9), 0x20: g(10),
                        0x24: mem.get(84), 0x28: mem.get(92), 0x3C: mem.get(100), 0x40: mem.get(108)}
            elif t == file_:
                table[g(3)] = last if last is not None else cur
                last = cur = None
            elif t == alloc:
                cur = {}
            for k in range(3, 13):
                r[k] = None
        elif op == 19 and ((w >> 1) & 0x3FF) == 528 and w & 1:
            cur = {}
            r[3] = None
    out = []
    for pos in sorted(table):
        o = table[pos]
        row = dict(pos=pos, name=img.cstr(o[8]), track=o.get(0x10), flag=o.get(0x14), type=o.get(0x18),
                   bits=o.get(0x1C), default=o.get(("f", 0x20), o.get(0x20)), min=o.get(("f", 0x24), o.get(0x24)),
                   max=o.get(("f", 0x28), o.get(0x28)), cubic=o.get(0x3C, 0xFFFF), tension=o.get(0x40, 0xFFFF))
        if row["type"] == 4:
            row["bits"] = 32
        out.append(row)
    bad = [p["name"] for p in out if None in (p["track"], p["flag"], p["type"], p["bits"])]
    assert not bad, f"ICE parameters not resolved: {bad}"
    return out


# ---------------------------------------------------------------- intro cameras
def ice_module(table):
    """old_tutorial.py's ICE section (decode, evaluation, shake), run unchanged on Skate 3's table."""
    src = (TOOLS / "pipeline/skate2port/old_tutorial.py").read_text(encoding="utf-8")
    start = src.index("# ---------------------------------------------------------------- ICE cameras")
    end = src.index("# ---------------------------------------------------------------- movies, speech")
    section = src[start:end]
    line = "ICE_PARAMS = sorted(json.loads((TABLES/'ice_params.json').read_text(encoding='utf-8')), key=lambda p: p['pos'])"
    assert line in section, "old_tutorial.py's ICE table line changed"
    section = section.replace(line, "ICE_PARAMS = sorted(S3_TABLE, key=lambda p: p['pos'])")
    import numpy as np
    ns = {"json": json, "math": math, "struct": struct, "functools": functools, "np": np, "S3_TABLE": table}
    exec(compile(section, "old_tutorial.py (ICE cameras)", "exec"), ns)
    return ns


_ICE_STATE = {}


def ice_state(disc, table):
    """(ICE module, ice_000.bin, ICELite.bin, {(hash, record bytes): camera}): one per process and table, so the modes
    share one exec, one 270 MB scene.big read, the shakes and the evaluated cameras (main passes one table)."""
    key = (str(disc), id(table))
    if key not in _ICE_STATE or _ICE_STATE[key][0] is not table:
        scene = big(disc, "data/big/scene.big")
        full, lite = read(scene, "data/scene/ice_000.bin"), read(scene, "data/scene/ICELite.bin")
        ice = ice_module(table)

        @functools.cache
        def shakes():
            out_ = []
            for _, (e, at, stop, _) in sorted(ice_records(lite).items(), key=lambda kv: kv[1][1]):
                t = e[at:stop]
                tracks = [struct.unpack_from("<HH", t, 0x14 + 4 * i) for i in range(34)]
                k, v = tracks[33]
                at2 = (0x9C + 2 * (max(k - 2, 0) + max(k - 1, 0)) + 3) & ~3
                block = ((v * 16 + 31) >> 3) & ~3
                cols = []
                for j in range(6):
                    bits = int.from_bytes(t[at2 + j * block:at2 + (j + 1) * block], "big")
                    cols.append([ice["ice_dequant"](ice["ICE"]["SHAKE_QUAT_X"], bits >> (8 * block - (x + 1) * 16) & 0xFFFF)
                                 for x in range(v)])
                out_.append((k, list(zip(*cols))))
            return out_
        ice["ice_shakes"] = shakes
        _ICE_STATE[key] = (table, ice, full, lite, {})
    return _ICE_STATE[key][1:]


def ice_records(e):
    """(type, size) LE chunks of records (1, size, hash, take count) + takes: hash -> [take bytes]."""
    out = {}
    o, end = 8, 8 + struct.unpack_from("<I", e, 4)[0]
    while o < end:
        chunk_end = o + 8 + struct.unpack_from("<I", e, o + 4)[0]
        r = o + 8
        while r + 8 <= chunk_end:
            _, size, h, n = struct.unpack_from("<4I", e, r)
            out[h] = (e, r + 0x10, r + 8 + size, n)
            r += 8 + size
        o = chunk_end
    return out


def scene_posts(ice, takes):
    """A scene's AUDIO_POST streams as (time, id): the takes' AUDIO_POST_ID at each AUDIO_POST_CMD 2 key, the stream
    old_tutorial's SCENE_AUDIO plays as `99_<id>_*` (Skate 2's post.big; Skate 3's is the same bank)."""
    out = []
    for t in takes:
        cmd, ids = t["values"].get("AUDIO_POST_CMD", []), t["values"].get("AUDIO_POST_ID", [])
        track = ice["ICE"]["AUDIO_POST_CMD"]["track"]
        times = t["keys"][track][1] if track < len(t["keys"]) else []
        out += [(round(times[j] * t["duration"], 3) if j < len(times) else 0.0, ids[j])
                for j, c in enumerate(cmd) if c == 2 and j < len(ids)]
    return out


# RwAudioCore's 5.1 -> stereo fold-down (exe string 0x820F0528, prepare_boot.FOLD): LFE dropped, centre and surrounds
# at -3 dB into the fronts, the whole mix a further -3 dB.
FOLD = 0.70710678


def fold_stereo(pcm):
    """vgmstream's post.big channels to stereo by the retail fold-down. The order is L C R [Ls Rs [LFE]], the SCHl
    movies' (prepare_boot), measured on all 38 post.big streams: ch1 is the loudest (centre), ch0~ch3 and ch2~ch4
    correlate (99_9128_DMODW 0.74 / 0.50, 99_9116_winbig_podium 0.82 / 0.69) and the billboards' ch5 is >= 97% below
    120 Hz. A stream of 2 channels or fewer is left as it is."""
    import numpy as np
    if pcm.shape[1] < 3:
        return pcm
    left, right = pcm[:, 0] + FOLD * pcm[:, 1], pcm[:, 2] + FOLD * pcm[:, 1]
    if pcm.shape[1] >= 5:
        left, right = left + FOLD * pcm[:, 3], right + FOLD * pcm[:, 4]
    return np.stack([left, right], 1) * FOLD


def post_streams(disc, ids, out):
    """post.big's `99_<id>_*.sns` (its .snr header in the bank's postresident.big) for each id, decoded by vgmstream
    (SKATE_VGMSTREAM) and folded to stereo: `out/nis/<stream>.wav`. Returns {id: {"audio", "seconds"}}."""
    import wave
    import numpy as np
    vgmstream = os.environ.get("SKATE_VGMSTREAM") or "vgmstream-cli"
    bank = big(disc, "data/audio/post.big")
    streams = {}
    for e in bank.entries:
        name = Path(e.path).name
        if name.startswith("99_") and name.endswith(".sns"):
            streams[int(name.split("_")[1])] = e
    got = {}
    with tempfile.TemporaryDirectory(prefix="skate3-post-") as work:
        resident = Path(work) / "postresident.big"
        resident.write_bytes(bank.read(next(e for e in bank.entries if e.path.endswith("postresident.big"))))
        heads_bank = BigArchive(resident)
        heads = {Path(e.path).name: e for e in heads_bank.entries}
        for i in sorted(ids):
            e = streams.get(i)
            if e is None:
                print(f"AUDIO_POST_ID {i}: no 99_{i}_*.sns in post.big - not exported")
                continue
            stem = Path(e.path).stem
            snr, raw = Path(work) / "a.snr", Path(work) / "a.wav"
            snr.write_bytes(heads_bank.read(heads[stem + ".snr"]))
            snr.with_suffix(".sns").write_bytes(bank.read(e))
            subprocess.run([vgmstream, "-o", str(raw), str(snr)], check=True, capture_output=True)
            with wave.open(str(raw)) as w:
                channels, rate, n = w.getnchannels(), w.getframerate(), w.getnframes()
                pcm = np.frombuffer(w.readframes(n), "<i2").reshape(-1, channels).astype(np.float32)
            pcm = fold_stereo(pcm)
            (out / "nis").mkdir(parents=True, exist_ok=True)
            with wave.open(str(out / f"nis/{stem}.wav"), "wb") as w:
                w.setnchannels(pcm.shape[1])
                w.setsampwidth(2)
                w.setframerate(rate)
                w.writeframes(np.clip(np.round(pcm), -32768, 32767).astype("<i2").tobytes())
            got[i] = {"audio": f"nis/{stem}.wav", "seconds": round(n / rate, 3)}
    return got


def nis_transition(kind, bloom_byte):
    """SceneManagerImpl's screen transition around a NIS's cut (TU3 0x827fc200 -> 0x82772248; findings 10-09 #10): none
    for the record's kind 1 (the "From X To Y" fronts), 0x16 (ViewScene) and 6 (photo_anywhere); kind 0x17 (film)
    takes its own path (untraced: none here); else record byte 6 (scene db slot 0x18, 0x827fec80) picks
    BloomToWhiteTransitionEffect over ColorFadeTransitionEffect type 1 (black). Each leg is 1 s."""
    if kind in (1, 6, 0x16, 0x17):
        return None
    return "bloom" if bloom_byte else "black"


def intro_cameras(disc, table, wanted, out, camera_only=(), extra=()):
    """{scene: {kind, origin, duration, tracks, camera}} as challenge::cutscene reads tutorial.json. A scene in
    `camera_only` with actor tracks is exported with its camera alone (the race starts: its racers' clips are not
    exported). `extra`: more (scene_00N.bin, ice_00N.bin) pairs (the DLC packages', DlcDb.scenes)."""
    ice, full, lite, cameras = ice_state(disc, table)
    records = ice_records(full)
    for _, more in extra:
        records.update(ice_records(more))
    takes = {}
    for h, (e, at, stop, n) in records.items():
        got = []
        for _ in range(n):
            got.append(ice["ice_take"](e[at:stop]))
            at = (at + got[-1]["size"] + 3) & ~3
        assert at == stop, f"ICE record {h:#x}: takes end at {at:#x}, the record at {stop:#x} (wrong table)"
        takes[h] = got

    boot = big(disc, "data/big/miscboot.big")
    scenes, posts = {}, {}
    for d in [read(boot, "data/scene/scene_000.bin")] + [sc for sc, _ in extra]:
        magic, _, n_scenes, _ = struct.unpack_from(">4sIII", d, 0)
        assert magic == b"scen"
        for i in range(n_scenes):
            o = 0x10 + i * 0x68
            name = d[o + 0x28:o + 0x68].split(b"\0")[0].decode("latin-1")
            if name not in wanted:
                continue
            f = struct.unpack_from(">7f", d, o + 8)
            count, _ = struct.unpack_from(">HH", d, o + 0x24)
            h = struct.unpack_from(">I", d, o)[0]
            if (count and name not in camera_only) or h not in takes:
                print(f"intro {name}: {count} actor tracks, camera {'found' if h in takes else 'missing'} - not exported")
                continue
            # The same record (by its bytes) gives the same camera: modes sharing an intro evaluate it once.
            e, at, stop, _ = records[h]
            if (h, e[at:stop]) not in cameras:
                try:
                    cameras[h, e[at:stop]] = ice["scene_camera"](takes[h])
                except AssertionError as error:
                    cameras[h, e[at:stop]] = error
            camera = cameras[h, e[at:stop]]
            if isinstance(camera, AssertionError):  # old_tutorial evaluates spaces 1-3 only (space 0 is not traced)
                print(f"intro {name}: camera spaces {camera} not evaluated - not exported")
                continue
            used = {k: sorted({x for t in takes[h] for x in t["values"][k]}) for k in ("LENS_USE_DYNAMIC", "RAWFOCUS_RELATIVE")}
            if any(any(v) for v in used.values()):
                print(f"intro {name}: S3-only lens flags set {used}")
            (out / "scene/camera").mkdir(parents=True, exist_ok=True)
            (out / f"scene/camera/{name}.json").write_text(json.dumps(camera, separators=(",", ":")), encoding="utf-8", newline="\n")
            scenes[name] = {"kind": d[o + 4], "origin": list(f[0:3]), "duration": f[6], "tracks": [], "audio": [],
                            "camera": f"scene/camera/{name}.json", "transition": nis_transition(d[o + 4], d[o + 6])}
            for at, post in scene_posts(ice, takes[h]):
                if at:  # ponytail: every online take starts its post at 0; a later start would need a delay field
                    print(f"intro {name}: AUDIO_POST {post} starts at {at} s - not exported")
                else:
                    posts.setdefault(name, []).append(post)
    # The SFX bed (whooshes, impacts). The takes' AUDIO_SPEECH events name a speech event whose voice is picked at run
    # time, not a stream: not exported (untraced).
    streams = post_streams(disc, {i for ids in posts.values() for i in ids}, out) if posts else {}
    for name, ids in posts.items():
        # Each stream once: every take names the same bed at 0 (bug 10-17 #43, as old_tutorial.post_ids).
        scenes[name]["audio"] = list({streams[i]["audio"]: streams[i] for i in ids if i in streams}.values())
    return scenes


# ---------------------------------------------------------------- screens
def screens(disc, collections, work, out):
    import prepare_core_menu as screen
    from prepare_online_ui import shadow_foreground
    manifests = {}
    for stem, path in SCREENS.items():
        rel = f"runtime/{stem}.json"
        result = screen.prepare(Path(disc), Path(work), Path(collections), path, rel, "skate3-online-hud")
        if stem == "hudonlinevoting" and result["unresolved_fonts"] == ["TradeBld"]:
            # Its one TradeBld field is the authoring label "CURRENT VOTE:OK" (mCurrentVote), placed above the top of
            # the screen (y ~ -106 on the 1280x720 canvas). The game draws a family it has no record for with its
            # "debug" record (FontManager lookup 0x82809208), which is no front-end bank; the movie's own Futura Shadow
            # stands in, unseen either way.
            result["fonts"]["TradeBld"] = result["fonts"]["Futura Shadow"]
            result["font_mappings"]["TradeBld"] = result["font_mappings"]["Futura Shadow"]
            result["unresolved_fonts"] = []
        # Futura Shadow draws with futuraheavy on top (apt_text); hudtimer imports only the shadow family.
        shadow_foreground(result, screen.AssetCache(Path(work)).font_asset)
        (Path(work) / rel).write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
        try:
            files = screen.runtime_files(Path(work), rel, "skate3-online-hud")
        except ValueError as e:   # onlineotsstat: its TradeBld family has no prepared font
            print(f"{stem}: not prepared ({e})")
            continue
        prefix = os.path.relpath(f"{OUT_ROOT}/hud", LAYER_ROOT).replace("\\", "/")
        text = (Path(work) / rel).read_text(encoding="utf-8")
        for f in files:
            if f != rel:
                text = text.replace(json.dumps(f), json.dumps(f"{prefix}/{f}"))
                (out / f).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(Path(work) / f, out / f)
        manifests[stem] = text
    return manifests


# Strings the Lua names directly (skate.lua 671-672, 863, 866, 1284, 1316-1318) and the results screen's.
SKATE_STRINGS = ["ID_SKATE_TRICK_NOT_SET", "ID_WRNGTRK_BRDGRAB", "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN",
                 "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN", "ID_LEADERBOARD_HEADER_RANK",
                 "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_NAME_HEADER", "ID_COMMON_SCORE"]


def race_starts(rows, scenes):
    """Per race heat, its online start NIS per scene as `<row>|<scene>`, placed by the heat's online definition, or
    its flythrough one when the online one's name is empty (Skate 2's NIS.EnterOnlineNISUsingDefinition 0x826dac08,
    skate2port/old_death_race.py; Skate 3's own is not traced): the scene's frame at `at`, turned to face `dir`
    (yaw; ponytail: pitch dropped, as the Skate 2 port). The plain scene entries stay for the camera files."""
    for name, row in rows.items():
        # outro_nis: OnlineOutroSoloNIS by its own definition (NIS.EnterNISUsingDefinition), as `<row>|outro`.
        outro = row.get("outro_definition")
        row["outro"] = None
        if outro and outro["scene"] in scenes:
            d = outro["dir"]
            scenes[f"{name}|outro"] = dict(scenes[outro["scene"]], origin=outro["at"], turn=round(math.atan2(d[0], d[2]), 6))
            row["outro"] = f"{name}|outro"
        for heat in row.get("heats", []):
            online, fly = heat.get("NISFlythroughDefinitionOnline"), heat.get("NISFlythroughDefinition")
            place = online if online and online["scene"] else fly
            heat["start_scenes"] = []
            for scene in heat["OnlineNISs"]:
                if scene not in scenes or not place:
                    heat["start_scenes"].append(None)
                    continue
                key = f"{name}|{scene}"
                d = place["dir"]
                scenes[key] = dict(scenes[scene], origin=place["at"], turn=round(math.atan2(d[0], d[2]), 6))
                heat["start_scenes"].append(key)


def package_templates(d, mode):
    """A DLC package's own templates of `mode`: Own the Lot's activity templates on its five graphs
    (dlc_mmcn_freeskate_activities_accumulation: no parent chain reaches the disc's); a mode of several disc templates
    (contest: street, vert) the package's copies of those; else the package's templates on the mode's graph."""
    spec = MODES[mode]
    if mode == "otl":
        return tuple(t for g in mode_graphs(mode) for t in d.templates(g))
    return d.copies(spec["template"], spec["graph"]) if isinstance(spec["template"], tuple) else d.templates(spec["graph"])


def domination_looks(rows):
    """Every (type key, style key) look Domination's spots take (domination.lua 461-464, 531-548): a row's
    RibbonIndicatorNeutral in RibbonColourNeutral, then RibbonIndicatorOwned in each RibbonColoursPlayers /
    RibbonColoursTeams colour. A refspec's key is its second 8 bytes."""
    key = lambda refspec: refspec[16:32]  # noqa: E731
    looks = set()
    for r in rows.values():
        if r.get("RibbonIndicatorNeutral") and r.get("RibbonColourNeutral"):
            looks.add((key(r["RibbonIndicatorNeutral"]), key(r["RibbonColourNeutral"])))
        if r.get("RibbonIndicatorOwned"):
            looks |= {(key(r["RibbonIndicatorOwned"]), key(c)) for c in r.get("RibbonColoursPlayers", []) + r.get("RibbonColoursTeams", [])}
    return sorted(looks)


def mode_looks(rows):
    """G7: every (type key, style key) look of the rows' `visual_indicators`, and of their contest rounds'
    ([`LOOK_MODES`]: their AddIndicator(self.VisualIndicators[i]) / (vars.CurRound.VisualIndicators[i]) draws each in
    its own look)."""
    return sorted({(c["ribbon"], c["style"]) for r in rows.values()
                   for c in r.get("visual_indicators", []) + [c for rd in r.get("rounds", []) for c in rd.get("visual_indicators", [])]})


# G7: the modes whose glues draw the rows' (contest: the rounds') VisualIndicators. Own the Lot's are not drawn (not in
# G7) and one of their styles is not in the skatercollections vault (indicator_looks would fail on it). 1-Up has none
# to draw: no 1-Up row, template or objective (1up_sequence_anywhere) sets VisualIndicators.
LOOK_MODES = ("spot_battle", "hall_of_meat", "ots", "film", "photo", "contest")


def look_file(mode, found, looks):
    """A mode's indicators.json in skate2_arrows.rs's format for VisualIndicators drawn by look (`found`: indicator_looks'
    shapes and styles): those namespaced "skate3:<key>" as skate_indicators', the textures by asset path, and per
    (type, style) look the locator entry "skate3:<type>:<style>" the mode's glue names (skate3_indicators::show)."""
    folder, ns = f"{OUT_ROOT}/{mode}/indicator", "skate3:{}".format
    return {"shapes": {ns(k): v for k, v in found["shapes"].items()},
            "styles": {ns(k): {"texture": f"{folder}/{v['texture']}", "tint": v["tint"],
                               "background": v["background"] and f"{folder}/{v['background']}"} for k, v in found["styles"].items()},
            "locators": {f"skate3:{t}:{st}": {"shape": ns(t), "style": ns(st)} for t, st in looks}}


# own_the_lots' StateGraph (the disc's, and each DLC package's own lot template: dlc_mmcn_own_the_lots).
LOTS_GRAPH = "freeskate"


def otl_lots(db, rows, dlcs=()):
    """Own the Lot: each lot (own_the_lots; a DLC package's own lot template, [`LOTS_GRAPH`]) and its
    OnlineFreeSkateActivities in order; each activity row in `rows` gets its graph (OTL_GRAPHS by its template; a
    package's activity templates by their own StateGraph)."""
    for template, graph in OTL_GRAPHS.items():
        for k in template_rows(db, template):
            if k in rows:
                rows[k]["graph"] = graph
    sources = [(db, "own_the_lots")]
    for d in dlcs:
        for graph in OTL_GRAPHS.values():
            for t in d.templates(graph):
                for k in template_rows(d, t):
                    if k in rows:
                        rows[k]["graph"] = graph
        sources += [(d, t) for t in d.templates(LOTS_GRAPH)]
    lots = {}
    for source, template in sources:
        named = {F(c["key"]): c["key"] for c in source.collections("challengebanks/main") if c["class"] == CHALLENGE}
        for lot, g in template_rows(source, template).items():
            activities = [named.get("Hash_" + i[16:32], "Hash_" + i[16:32]) for i in items(g["fields"].get(ONLINE_FREESKATE_ACTIVITIES, {}))]
            # A package's map free skate shares the graph (freeskate_dlc_maloof_nyc): no activities, no lot.
            if not activities:
                continue
            lots[lot] = activities
            missing = [a for a in lots[lot] if a not in rows]
            if missing:
                print(f"{lot}: activities without a row {missing}")
    return lots


def write_json(path, value, compact=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(value, separators=(",", ":")) if compact else json.dumps(value, indent=1, ensure_ascii=False)
    path.write_text(text, encoding="utf-8", newline="\n")


def row_strings(rows):
    """Every string id a row's fields name (the decoded Text values, in nested records too: contest rounds)."""
    found = set()

    def walk(v):
        if isinstance(v, dict):
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)
        elif isinstance(v, str) and v.startswith("ID_"):
            found.add(v)
    walk(list(rows.values()))
    return found


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--disc", type=Path, required=True, help="extracted Skate 3 disc folder")
    p.add_argument("--assets", type=Path, required=True, help="the install's assets folder (read)")
    p.add_argument("--dlc", type=Path, help="folder of the Skate 3 DLC STFS packages (the DLC rows of the modes that "
                                            "take them: skate and MODES[mode]['dlc'])")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--install", action="store_true", help="also copy DIR/* into assets/private/skate3-online")
    p.add_argument("--no-screens", action="store_true", help="skip the HUD movies (fast data-only runs)")
    p.add_argument("--inputs", type=Path, help="read challengemap.json and online.json from this folder instead of the "
                   "install (a copy taken before the Skate 2/1 online stages add their rows and folder labels: neither "
                   "reaches this script's output, so it need not wait for them)")
    p.add_argument("--modes", default="skate," + ",".join(MODES), help="comma-separated modes (default: all)")
    p.add_argument("--starts", choices=("skip", "only"), help="skip: online.json is not read (the modes with objectives "
                   "get no Location from it); only: just those modes, no hud/. The pipeline exports with skip from the "
                   "start and re-exports with only once the online challenge map has written online.json")
    a = p.parse_args(argv)
    wanted = [m for m in a.modes.split(",") if m and (a.starts != "only" or MODES.get(m, {}).get("objectives"))]
    a.no_screens |= a.starts == "only"
    root = a.out.resolve()
    challengemap = a.inputs / "challengemap.json" if a.inputs else a.assets / "private/challenge-map/runtime/challengemap.json"
    online_json = a.inputs / "online.json" if a.inputs else a.assets / "private/challenge-map/online/online.json"
    language = whole_json(challengemap)["language"]
    with tempfile.TemporaryDirectory(prefix="skate3-online-") as work:
        work = Path(work)
        db = Db(a.disc, work / "db")
        missions = big(a.disc, "data/content/missions.big")
        image = Image(xex_image(a.disc / "default.xex"))
        table = ice_table(image)
        dlcs = dlc_packages(a.dlc, db, work / "dlc") if a.dlc else []
        print(f"ICE table: {len(table)} parameters, {1 + max(p['track'] for p in table)} tracks")
        luas = graphs_lua(db, [g for mode in wanted for g in mode_graphs(mode)])
        for mode in wanted:
            out = root / mode
            shutil.rmtree(out, ignore_errors=True)
            out.mkdir(parents=True)
            if mode == "skate":
                graph, rules = "skate", skate_rules(db, missions)
                rows = rules["rows"]
                for d in dlcs:
                    # A package's S.K.A.T.E. rows (skate_artg_01): its own templates, missions and local data; the
                    # template-level fields (messages, max_letters, time_limit) stay the disc's.
                    if d.templates(graph):
                        rows.update(skate_rules(d, d.archive, d.templates(graph))["rows"])
                        language = {**d.language, **language}
                ids = set(rules["template"].values()) | set(SKATE_STRINGS) | set(HUD_STRINGS)
            else:
                spec = MODES[mode]
                bank = Objectives(db, a.disc) if spec.get("groups") else None
                opts = dict(intro_field=spec.get("intro_field"), heats=spec.get("heats", False),
                            rounds=spec.get("rounds", False), objectives=spec.get("objectives", False),
                            groups=spec.get("groups"), global_fields=spec.get("global_fields", ()),
                            local_parents=spec.get("local_parents", False), nis_fields=spec.get("nis_fields", ()),
                            objective_indicators=spec.get("objective_indicators", False),
                            bool_fields=spec.get("bool_fields"))
                graph, rows = spec["graph"], mode_rules(db, missions, spec["template"], spec["fields"], bank=bank, **opts)
                for d in dlcs if spec.get("dlc") else []:
                    # A package's rows: its own templates (by the graph), missions, local data and bank .bin, read with
                    # the disc rows' options.
                    dbank = Objectives(d, a.disc) if bank else None
                    if dbank:
                        dbank.global_locators.update(d.global_locators)
                    rows.update(mode_rules(d, d.archive, package_templates(d, mode), spec["fields"], bank=dbank, **opts))
                    if dbank:
                        bank.sources.update(dbank.sources)
                    language = {**d.language, **language}
                if spec.get("objectives"):
                    # Location is a global locator (not in the row's mission): the one online_challenge_map resolved.
                    known = online_starts(online_json) if a.starts != "skip" else {}
                    for k, r in rows.items():
                        if r.get("Location") is None and k in known:
                            r["Location"] = known[k]
                rules = {"rows": rows}
                if bank:
                    rules["points_objective"] = bank.points_objective()
                    named = {F(t): t for r in rows.values() for t in r.get("trick_groups", [])}
                    for r in rows.values():
                        if r.get("unique_trick_group"):
                            r["unique_trick_group"] = named.get(r["unique_trick_group"], r["unique_trick_group"])
                    rules["trick_groups"] = trick_groups(db, {t for r in rows.values()
                                                              for t in r.get("trick_groups", []) + [r.get("unique_trick_group")] if t})
                    # The snippets' own text by main.bin offset: the objective oracle's input (tests), beside the graph.
                    write_json(out / "scripts/objectives.json", bank.sources)
                if spec.get("constants"):
                    # The enum constants the snippets name (ObjectiveHOM.EBoneIndex.*, Flow.eObjectiveStatus.*).
                    rules["constants"] = lua_constants(image, rows)
                ids = row_strings(rows) | set(spec["strings"]) | set(HUD_STRINGS)
            starts = {n for r in rows.values() for h in r.get("heats", []) for n in h["OnlineNISs"]}
            starts |= {r["outro_definition"]["scene"] for r in rows.values() if r.get("outro_definition")}
            rounds = {rd["intro"] for r in rows.values() for rd in r.get("rounds", []) if rd["intro"]}
            extra = [s for d in dlcs for s in d.scenes()] if mode == "skate" or MODES[mode].get("dlc") else []
            scenes = intro_cameras(a.disc, table, {r["intro"] for r in rows.values() if r["intro"]} | starts | rounds, out, starts,
                                   extra)
            for row in rows.values():
                row["intro"] = row["intro"] if row["intro"] in scenes else None
                for rd in row.get("rounds", []):
                    rd["intro"] = rd["intro"] if rd["intro"] in scenes else None
            race_starts(rows, scenes)
            if mode == "race":
                write_json(out / "indicators.json", race_indicators(a.disc, a.assets / "private/stock/skater-collections.json",
                                                                    rows, out), compact=True)
            if mode == "skate":
                write_json(out / "indicators.json", skate_indicators(a.disc, a.assets / "private/stock/skater-collections.json",
                                                                     rows, out), compact=True)
            # G3 / G7: the looks a mode's glue names (S.K.A.T.E. and the race write their own above).
            looks = domination_looks(rows) if mode == "domination" else mode_looks(rows) if mode in LOOK_MODES else []
            if looks:
                found = indicator_looks(a.disc, a.assets / "private/stock/skater-collections.json", looks, out)
                write_json(out / "indicators.json", look_file(mode, found, looks), compact=True)
            if mode == "otl":
                rules["lots"] = otl_lots(db, rows, dlcs)
            for graph in mode_graphs(mode):
                lua = luas and luas[graph]
                if lua:
                    (out / "scripts").mkdir(exist_ok=True)
                    (out / f"scripts/{graph}.lua").write_text(lua, encoding="utf-8", newline="\n")
            ids |= {r["title"] for r in rows.values() if r["title"]}
            write_json(out / "rules.json", rules)
            write_json(out / "tutorial.json", {"scenes": scenes})
            write_json(out / "strings.json", {k: language[k] for k in sorted(ids) if k in language})
            online = [k for k, r in rows.items() if r["online"]]
            print(f"{mode}: {len(rows)} rows ({len(online)} online: {', '.join(online)}), {len(scenes)} intro cameras")
        if not a.no_screens:
            out = root / "hud"
            shutil.rmtree(out, ignore_errors=True)
            for stem, text in screens(a.disc, a.assets / "private/stock/skater-collections.json", work / "screens", out).items():
                (out / "runtime").mkdir(parents=True, exist_ok=True)
                (out / f"runtime/{stem}.json").write_text(text, encoding="utf-8", newline="\n")
            wanted.append("hud")
    if a.install:
        for name in wanted:
            dest = a.assets / OUT_ROOT / name
            shutil.rmtree(dest, ignore_errors=True)
            shutil.copytree(root / name, dest)
            print(f"installed {dest}")


if __name__ == "__main__":
    main()
