"""Extract retail Skate 3's Online Challenge Map tree into online.json (schema v1).

py -3 tools/online_challenge_map.py --disc DISC --dlc DLC --assets INST/assets --out DIR [--check]

DLC: the STFS packages under DLC are extracted (cached) to DIR/dlc/<package>; their challengebanks,
dlc_*.vlt collections (map_listing, categories, worlds) and language tables merge over the base data.

Inputs (owned disc + prepared install, nothing from research scratchpads):
  tree       INST/assets/private/stock/skater-collections.json (map_listing root "online"), plus the
             raw data/db/skatercollections.bin from DISC/data/big/db.big for the visibility Lua;
  challenges data/db/challengebanks/main.{vlt,bin} from db.big, converted with skaterschema;
  strings    INST/assets/private/challenge-map/runtime/challengemap.json ["language"];
  teleports  INST/assets/private/teleports.json.
Challenge details (discovery on photo_dwtn_04 "Wayward Waterfall", capture frames 068-069):
  objectives  no field holds their text IDs; the language table names them ID_CHALLENGE_<KEY>_OBJ<n>
              ("Gap the Waterfall" = ID_CHALLENGE_PHOTO_DWTN_04_OBJ0), listed while the next ID exists;
  description DescriptionOnline 713D1933.. else Description 6B0BD2BA..; bonus UInt32 2E4824C8.. (1875);
  position    global-data Math::Vector3 B4F74CB7.. (photo_dwtn_04: -123, 47.7, 31.5, retail world
              units), null when absent (the game falls back to the map spawn); teleport stays null;
  icon        per ChallengeType (ICONS); Spot Battle's down arrow = mini_spotfound (capture frame 005);
  map_image   DLC challenges show their pack's zoomed map (frame 005: DW green park), MAP_IMAGES.
Spawn and online area (retail: AI.RepositionAI(player, self.Location, playerSlot)):
  start       global-data MapStartLocation A6E89247.. else Location F1C6D5FA.. (u32 offsets into the bank's .bin
              strings) name a global locator (data/content/global_locators/**.rx2, section EB0009):
              {matrix: its 4x4 (heading included), slots: its children <name>::*_spawn_1..6, one per lobby slot};
              skateparks: their world's start locator (Text 3735C5C1..); null when unresolved;
  boundary    data/boundary/<key>.xml (miscload.big + DLC) <Point>x, z</Point>: the online-area polygon, world X/Z;
  oob_time    challenge_local_data/<key> OutOfBoundsTime (Int32 8DD0BDA0..) when it overrides the default 5 s.
Top level, markers: {challenge_local_data key: [set, use, use in a wipeout]}, AllowSessionMarkersToBeSet / ToBeUsed /
  ToBeUsedInWipeout through the record's parents (framework templates), disc and DLC (marker_flags); every key, as a
  match may play a row the tree doesn't list (an Own the Lot activity).
--install also writes online.json, icons/ and maps/ into INST/assets/private/challenge-map/online/.
Every run checks the result against tools/online_challenge_map_reference.json (the retail captures)
and refuses to write when it does not match. --check only checks.
"""
import argparse
import json
import re
import struct
import sys
import tempfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path[:0] = [str(TOOLS), str(TOOLS / "vendor")]
from asset_pipeline.vlt import convert, hash64  # noqa: E402
from skate3_ui.big import BigArchive  # noqa: E402
from skate3_ui.project import extract_project  # noqa: E402
from skate3_ui.rw4 import extract_textures  # noqa: E402

REFERENCE = TOOLS / "online_challenge_map_reference.json"

# skatercollections classes and fields (AttribSys 64-bit hashes).
LISTING = "B84BB2A6EF94DBB0"          # map_listing: the tab tree
KINDS = {"971D2A3BED31B4D0": "filter", "83CA2228D49A9BBB": "category", "D629DA96E6DE101E": "command"}
CATEGORY = "83CA2228D49A9BBB"
NODE_REF, NODE_ORDER = "E0762EA3F90C9EEB", "982BD1EAE2334504"   # order = upper 16 bits
NAME, CATEGORY_NAME, HELPER = "6766EFD84D74F4F0", "465698DD3CC73CA2", "5FBB2C3F59FC6F1A"
LUA = "5604268225257009"              # LuaState::tCompiledLua: offset of the source in skatercollections.bin
CATEGORY_ORDER = "1349BB6EC1D1E751"
FILTER_GROUP = "1926B6BCBBB2221A"     # filter -> challenge_global_data group (its ChallengeType)
WORLD_CLASS = "%016X" % hash64("world")
WORLD_NAME, IS_SKATEPARK, IN_PARKS_TAB = "095E1D9778F0C6F4", "0C54AD24155E3220", "887AB7600B7FA9A0"
# challengebanks classes and fields.
CHALLENGES, GLOBAL, GLOBAL_REF = "8876E0B556740E36", "A4F35B159D7432B5", "24491BF00316AA45"
AVAILABLE_ONLINE, DEBUG_ONLY = "F9A9A1DDED84A643", "FD40E4A31027C984"
TITLE, WORLD, TYPE, MAP_CATEGORY = "E2A50788B6C20C92", "7565CD2A80E93A5E", "02B828CCDB4785E4", "50D9293B818507EC"
TAGS, DESCRIPTION, DESCRIPTION_ONLINE = "FBEE0A4276F904BA", "6B0BD2BAE508B742", "713D193371A2B4E6"
# UInt32 reward: photo_dwtn_04 (Wayward Waterfall) = 1875, its captured "Online Bonus: 1,875" (frame 068).
# OnlineReward (412EB92D...) is never set in the base bank.
BONUS = "2E4824C81FDAE87C"
START = "B4F74CB7A94FC13B"   # Math::Vector3 start position
LOCATION, MAP_START = "F1C6D5FA4A23951E", "A6E8924715A15CEC"  # LocationID: u32 offset into the bank's .bin
WORLD_START = "3735C5C12E8E7AE1"  # world record Text: start locator name (Z_SP_DT_Start)
OOB_TIME, OOB_DEFAULT = "8DD0BDA08407E230", 5  # challenge_local_data Int32 OutOfBoundsTime (framework default 5)
# ChallengeType -> marker icon (fedynamic.big data/fe/source/images/icons/<name>.rx2).
# ponytail: Own The Spot / Own The Lot have no capture; mini_spotowned is a guess.
ICONS = {7: "mini_spotfilm", 0x26: "mini_spotphoto", 9: "mini_spotowned", 0x34: "mini_spotowned",
         0x1F: "mini_hom", 0x21: "mini_hom", 0x35: "mini_spotfound", 0x13: "mini_deathrace", 0x20: "mini_1up",
         0x33: "mini_domination", 0x0F: "mini_vert", 8: "mini_street", 4: "mini_skate", 0x16: "mini_teleport"}
ICON_DIR, MAP_DIR = "data/fe/source/images/icons/", "data/fe/source/images/map/"
# DLC map -> its pack's zoomed map (no world field names it; each pack ships exactly one map/*.rx2 for it).
MAP_IMAGES = {"Danny_Way_Hawaiian_Dream": "map_danny", "Maloof_Money_Cup_NYC": "DLC_Maloof_Map_512",
              "Sanatorium": "map_afterdark", "Art_Gallery": "map_partypack"}

# Retail world record -> engine .skate map (child worlds resolve through their parents).
WORLD_MAPS = {"dist_downtown": "DownTown", "dist_industrial": "Industrial", "dist_university": "University",
              "dist_skateschool": "SkateSchool", "dist_maloofmoneycup": "MaloofMoneyCup",
              "dist_blackboxpark": "BlackBoxPark", "dist_downtownpark": "DownTownSkatePark",
              "dist_startpark": "StartPark", "dist_industrialpark": "IndustrialSkatePark", "dist_megapark": "MegaPark",
              # DLC worlds (main() checks every map exists in the installation's maps.json)
              "dlc_dw_megacompound": "Danny_Way_Hawaiian_Dream", "dist_maloofmoneycupdlc": "Maloof_Money_Cup_NYC",
              "dist_sanitariumdlc": "Sanatorium", "dist_artgallerydlc": "Art_Gallery",
              "dist_backlotpark": "Back_Lot_Park", "dist_downtownparknight": "Downtown_Night_Park",
              "dist_zenplazapark": "Zen_Plaza"}
# District locator streams (data/content/global_locators/<stream>) -> engine map.
DISTRICTS = {"DIST_DownTown": "DownTown", "DIST_Industrial": "Industrial", "DIST_University": "University"}
# Challenge group carrying the ChallengeType of filters that name the type only in Lua.
TYPE_GROUPS = {"Film": "films", "SpotPhoto": "photos", "OwnTheSpot": "own_the_spots", "HallOfMeat": "hall_of_meats",
               "OnlineFreeSkate": "freeskate_locations"}
# Rows the data lists but retail does not show outside a session, with no rule in the data/Lua:
# capture: frames 001-052 (Team root has no Own The Lot), frames 071-085 (Freeskate root ends at Black Box Park;
# Maloof Money Cup returns as an engine extra, spec "Engine extras").
HIDDEN = {("online_team", "otl")}
# Engine extras (spec "Engine extras"): rows after the captured ones. Maloof Money Cup goes after the base
# Freeskate rows (before the DLC rows); the Skate 1/2 cities follow as folders of their teleports.json spots.
EXTRAS = {("online_freeskate", "freeskate_location_maloof")}
OLD_CITIES = {"New_San_Vanelona": "New San Vanelona", "Skate_1_San_Vanelona": "Skate 1 San Vanelona"}
# capture: frames 081-083. skate.Park park order: record order, unlocks and skp numbers all differ.
PARK_ORDER = ["dist_downtownpark", "dist_startpark", "dist_industrialpark", "dist_megapark",
              "dist_backlotpark", "dist_downtownparknight", "dist_zenplazapark"]


def sort_titles(rows):
    """(title, map, ...) rows in retail order: Black Box Park first, then a case-insensitive
    collation where letters sort before anything else (spaces, punctuation, digits)."""
    return sorted(rows, key=lambda r: (r[1] != "BlackBoxPark",
                                       [(0, c) if c.isalpha() else (1, c) for c in r[0].lower()]))


def category_rows(picked, options=()):
    """(title, map, category order, category name, row) -> rows: categories in their order, challenges of
    categories sharing an order interleave by title (Career Film's alternating DLC pack headers, the DLC
    categories all have order 20000), a header whenever the category changes."""
    rows, header = list(options), None
    for _, _, _, name, row in sorted(sort_titles(picked), key=lambda p: p[2]):
        if name != header:
            rows.append({"name": name, "header": True})
            header = name
        rows.append(row)
    return rows


def folder_label(text, rows):
    return f"{text} ({sum(1 for r in rows if 'challenge' in r or 'location' in r)})"


def compare_list(expected, got, prefix):
    n = prefix if prefix is not None else max(len(expected), len(got))
    for i in range(n):
        e = expected[i] if i < len(expected) else None
        g = got[i] if i < len(got) else None
        if e != g:
            return f"row {i + 1}: expected {e!r}, got {g!r}"
    return None


def hx(name):
    """Hash (16 hex digits) of a converted class/key/field name, resolved or 'Hash_...'."""
    return name[5:] if name.startswith("Hash_") else "%016X" % hash64(name)


def need(pattern, lua, flt):
    m = re.search(pattern, lua)
    if not m:
        raise SystemExit(f"filter {flt['key']}: its Lua has no {pattern!r}:\n{lua}")
    return m.group(1)


class Vault:
    """Converted AttribSys collections indexed by (class hash, key hash), with field inheritance."""

    def __init__(self, collections):
        self.by, self.fields = {}, {}
        for r in collections:  # later collections (DLC) override earlier ones with the same key
            k = (hx(r["class"]), hx(r["key"]))
            self.by[k] = r
            self.fields[k] = {hx(n): v for n, v in r["fields"].items()}

    def ref(self, data):
        return self.by.get((data[:16], data[16:32])) if data else None

    def field(self, r, f, whole=False):
        while r is not None:
            cls = hx(r["class"])
            v = self.fields[(cls, hx(r["key"]))].get(f)
            if v is not None:
                return v if whole else v["data"]
            r = self.by.get((cls, hx(r["parent"]))) if r["parent"] else None
        return None

    def is_true(self, r, f):
        return (self.field(r, f) or "")[:2] == "01"


def extract_db(disc, work):
    archive = BigArchive(Path(disc) / "data/big/db.big")
    want = {f"data/db/{n}" for n in ("skaterschema.vlt", "skaterschema.bin", "skatercollections.bin",
                                      "challengebanks/main.vlt", "challengebanks/main.bin",
                                      "challenge_local_data_framework.vlt", "challenge_local_data_framework.bin")}
    for e in archive.entries:
        low = e.path.lower()
        if low in want or (low.startswith("data/db/challenge_local_data/") and low.endswith((".vlt", ".bin"))):
            (work / e.path).parent.mkdir(parents=True, exist_ok=True)
            (work / e.path).write_bytes(archive.read(e))
    missing = [w for w in want if not (work / w).is_file()]
    if missing:
        raise SystemExit(f"db.big is missing {missing}")
    names = (TOOLS / "asset_pipeline/names.txt").read_text(encoding="utf-8").splitlines()
    banks = convert(work / "data/db/skaterschema", work / "data/db/challengebanks/main", names)["collections"]
    oob = oob_times(work / "data/db/challenge_local_data", names, work / "data/db/skaterschema")
    return (banks, (work / "data/db/skatercollections.bin").read_bytes(),
            (work / "data/db/challengebanks/main.bin").read_bytes(), oob)


def compact_json(value):
    """indent=1 JSON with number-only arrays (matrices, slots, polygons) on one line each."""
    text = json.dumps(value, indent=1, ensure_ascii=False)
    # Strings match first and are kept as they are.
    return re.sub(r'"(?:[^"\\]|\\.)*"|\[[-\d.eE+,\[\]\s]*\]',
                  lambda m: m[0] if m[0][0] == '"' else re.sub(r"\s+", "", m[0]), text)


def spawn_data(disc, dlc_x):
    """Global locators and online-area polygons: the disc's, then the DLC packages' (their x/ folders)."""
    locators, polygons = {}, {}
    for root in [Path(disc) / "data/content/global_locators", *(x / "data/content/global_locators" for x in dlc_x)]:
        for p in sorted(root.rglob("*.rx2")):
            for name, loc in global_locators(p.read_bytes()).items():
                locators.setdefault(name, dict(loc, stream=p.parent.name))
    misc = BigArchive(Path(disc) / "data/big/miscload.big")
    for e in misc.entries:
        if e.path.lower().startswith("data/boundary/") and e.path.lower().endswith(".xml"):
            polygons[Path(e.path).stem.lower()] = boundary(misc.read(e).decode("latin-1"))
    for x in dlc_x:
        for p in sorted((x / "data/boundary").glob("*.xml")):
            polygons.setdefault(p.stem.lower(), boundary(p.read_text(encoding="latin-1")))
    return locators, polygons


def stfs_extract(pkg, out):
    """Minimal Xbox 360 STFS (CON/LIVE/PIRS) reader: write every file of `pkg` under `out`."""
    f = open(pkg, "rb")

    def read(off, n):
        f.seek(off)
        return f.read(n)
    hdr = read(0, 0x1000)
    header_size = struct.unpack(">I", hdr[0x340:0x344])[0]
    vd = hdr[0x379:0x379 + 0x24]
    ft_count, ft_block = struct.unpack("<H", vd[3:5])[0], int.from_bytes(vd[5:8], "little")
    sex = (~vd[2]) & 1  # 0: one hash table per level (female), 1: male
    base = (header_size + 0xFFF) & ~0xFFF

    def addr(b):
        r = (((b + 0xAA) // 0xAA) << sex) + b
        if b >= 0xAA:
            r += ((b + 0x70E4) // 0x70E4) << sex
            if b >= 0x70E4:
                r += 1 << sex
        return (r << 12) + base

    def next_block(b):
        if sex:
            raise NotImplementedError("non-consecutive file in a male STFS package")
        n = 0 if b < 0xAA else (b // 0xAA) * 0xAB + (((b // 0x70E4) + 1) << sex) + (1 << sex if b // 0x70E4 else 0)
        return int.from_bytes(read((n << 12) + base + (b % 0xAA) * 0x18 + 0x15, 3), "big")

    def chain(start, count, consecutive):
        b = start
        for _ in range(count):
            yield b
            b = b + 1 if consecutive else next_block(b)
    table = b"".join(read(addr(b), 0x1000) for b in chain(ft_block, ft_count, False))
    entries = []
    for i in range(0, len(table), 0x40):
        e = table[i:i + 0x40]
        if e[0x28] & 0x3F == 0:
            break
        entries.append((e[:e[0x28] & 0x3F].decode("latin-1"), bool(e[0x28] & 0x80), bool(e[0x28] & 0x40),
                        int.from_bytes(e[0x29:0x2C], "little"), int.from_bytes(e[0x2F:0x32], "little"),
                        struct.unpack(">h", e[0x32:0x34])[0], struct.unpack(">I", e[0x34:0x38])[0]))

    def path(i):
        name, *_, parent, _ = entries[i]
        return path(parent) / name if parent != -1 else Path(name)
    for i, (name, is_dir, cons, blocks, start, _, size) in enumerate(entries):
        p = Path(out) / path(i)
        if is_dir:
            p.mkdir(parents=True, exist_ok=True)
            continue
        if p.is_file() and p.stat().st_size == size:
            continue  # extracted by an earlier run
        p.parent.mkdir(parents=True, exist_ok=True)
        left = size
        with open(p, "wb") as w:
            for b in chain(start, blocks, cons):
                chunk = read(addr(b), min(0x1000, left))
                w.write(chunk)
                left -= len(chunk)
    f.close()


def language_table(raw):
    """Retail language table: {key hash: text}."""
    count, start, strings = struct.unpack_from("<3I", raw, 8)
    start, strings = start + 8, strings + 8
    out = {}
    for i in range(count):
        key, off = struct.unpack_from("<2I", raw, start + i * 8)
        out[key] = raw[strings + off:raw.index(b"\0", strings + off)].decode("latin-1")
    return out


def global_locators(raw):
    """{name (lower case): {matrix, slots}} of an RX2's EB0009 locator table. Record 128 B: 4x4 matrix @0
    (row-major, translation in the last row), child count @96, child table @100, name @104; child 112 B:
    matrix @0, name @96 (<parent>::*_spawn_<n>). Offsets are section-relative. Slots stay in table order: slot i is
    child i (the slot resolver, S2 0x82879950 / S1 0x823d9088, indexes the table; 12 S1/S2 locators number theirs
    out of order)."""
    count, table = struct.unpack_from(">I", raw, 32)[0], struct.unpack_from(">I", raw, 48)[0]
    out = {}
    for i in range(count):
        base, _, _, _, _, kind = struct.unpack_from(">6I", raw, table + i * 24)
        if kind != 0xEB0009:
            continue
        n, _, _, start, _ = struct.unpack_from(">5I", raw, base)

        def name(at):
            o = base + struct.unpack_from(">I", raw, at)[0]
            return raw[o:raw.index(b"\0", o)].decode("latin-1")

        def matrix(at):
            m = [round(v, 4) for v in struct.unpack_from(">16f", raw, at)]
            return [m[0:4], m[4:8], m[8:12], m[12:16]]
        for j in range(n):
            at = base + start + j * 128
            kids, kid_table = struct.unpack_from(">2I", raw, at + 96)
            slots = [matrix(base + kid_table + k * 112) for k in range(kids)]
            out[name(at + 104).lower()] = {"matrix": matrix(at), "slots": slots}
    return out


def boundary(xml):
    """data/boundary/<key>.xml: [[x, z], ...] of its <Point>x, z</Point> list."""
    return [[float(x), float(z)] for x, z in re.findall(r"<Point>\s*([-\d.eE+]+)\s*,\s*([-\d.eE+]+)\s*</Point>", xml)]


def rebase(records, delta, fields=(LUA,)):
    """Copies of `records` whose .bin offsets (compiled-Lua sources, LocationID strings) point `delta` bytes
    further (the .bin they index is appended after `delta` bytes of other .bin data)."""
    out = []
    for r in records:
        rec = {n: (dict(v, data="%08X" % (int(v["data"][:8], 16) + delta) + v["data"][8:])
                   if hx(n) in fields and int(v["data"][:8], 16) else v) for n, v in r["fields"].items()}
        out.append(dict(r, fields=rec))
    return out


def oob_times(local_dir, names, schema):
    """{challenge key: OutOfBoundsTime} of every challenge_local_data/<key>.vlt that overrides the default."""
    out = {}
    for v in sorted(Path(local_dir).glob("*.vlt")):
        if bytes.fromhex(OOB_TIME) not in v.read_bytes():
            continue  # no record sets it (converting all 456 files takes a minute)
        records = convert(schema, v.with_suffix(""), names)["collections"]
        times = [int(f["data"][:8], 16) for r in records for n, f in r["fields"].items() if hx(n) == OOB_TIME]
        override = next((t for t in times if t != OOB_DEFAULT), None)
        if override is not None:
            out[v.stem.lower()] = override
    return out


MARKER_FIELDS = ("AllowSessionMarkersToBeSet", "AllowSessionMarkersToBeUsed", "AllowSessionMarkersToBeUsedInWipeout")


def marker_flags(dbs):
    """{challenge key: [set, use, use in a wipeout]} for every challenge_local_data/<key>.vlt under the data/db folders
    `dbs` (the disc's first: it holds the framework templates). TU3 0x8289BA10 (place) and 0x8289BAE0 (return) read
    them off the running challenge's record through its parents (Attrib lookup 0x82B72420); a miss reads the zero byte
    0x830D0850, so false. Inline Bool entries: one byte at entry + 8."""
    from asset_pipeline.vlt import vault, unpack
    want, local = {hash64(n): i for i, n in enumerate(MARKER_FIELDS)}, hash64("challenge_local_data")
    records, rows = {}, {}
    files = [d / "challenge_local_data_framework.vlt" for d in dbs[:1]] + [
        v for d in dbs for v in sorted((d / "challenge_local_data").glob("*.vlt"))]
    for v in files:
        if not v.is_file() or not v.with_suffix(".bin").is_file():
            continue
        cv, _, exports = vault(v.with_suffix(""))
        for _, kind, _, at in exports:
            if kind != 0xAD303B8F42B3307E:
                continue
            key, cls, parent, _, _, count, _, typeslen, _, _ = unpack("QQQIIIHHII", cv, at)
            if cls != local:
                continue
            own, entries = {}, at + 48 + typeslen * 8
            for i in range(count):
                fk, = unpack("Q", cv, entries + i * 16)
                if fk in want:
                    own[want[fk]] = cv[entries + i * 16 + 8] != 0
            records.setdefault(key, (parent, own))
        if v.parent.name == "challenge_local_data":
            rows[hash64(v.stem.lower())] = v.stem.lower()

    def flag(key, i, depth=0):
        parent, own = records.get(key, (0, {}))
        if i in own:
            return own[i]
        return bool(parent) and depth < 32 and flag(parent, i, depth + 1)
    return {name: [flag(key, i) for i in range(3)] for key, name in rows.items() if key in records}


def extract_dlc(dlc_root, work, schema):
    """[(package, challengebank records, skatercollections records, Lua .bin, {string ID: English},
    challengebank .bin)]."""
    names = (TOOLS / "asset_pipeline/names.txt").read_text(encoding="utf-8").splitlines()
    packs = []
    for pkg in sorted(p for p in Path(dlc_root).rglob("*") if p.is_file()):
        with open(pkg, "rb") as f:
            if f.read(4) not in (b"CON ", b"LIVE", b"PIRS"):
                continue
        out = Path(work) / "dlc" / pkg.name
        try:
            stfs_extract(pkg, out)
        except (IndexError, NotImplementedError) as e:
            # ponytail: the ~120 KB unlock/presell packages use a block layout this reader does not
            # follow; they carry no database. Upgrade the reader if a package with data hits this.
            print(f"skipped DLC package {pkg.name} ({pkg.stat().st_size} bytes): {e!r}")
            continue
        for big in out.glob("*.big"):
            archive = BigArchive(big)
            x = out / "x"
            for e in archive.entries:
                low = e.path.lower()
                if (low.startswith("data/db/") and low.endswith((".vlt", ".bin"))
                        and ("local_data" not in low or low.startswith("data/db/challenge_local_data/"))
                        and "progressionbanks" not in low) or (low.startswith("data/fe/languages/")
                                                              and "histogram" not in low) \
                        or low.startswith((MAP_DIR.lower(), "data/content/global_locators/", "data/boundary/")):
                    (x / e.path).parent.mkdir(parents=True, exist_ok=True)
                    (x / e.path).write_bytes(archive.read(e))
            banks, bank_bin = [], b""
            for v in sorted((x / "data/db/challengebanks").glob("*.vlt")):
                banks += rebase(convert(schema, v.with_suffix(""), names)["collections"], len(bank_bin),
                                (LOCATION, MAP_START))
                if v.with_suffix(".bin").is_file():
                    bank_bin += v.with_suffix(".bin").read_bytes()
            colls, lua = [], b""
            for v in sorted((x / "data/db").glob("dlc_*.vlt")):
                if v.with_suffix(".bin").is_file():
                    colls += rebase(convert(schema, v.with_suffix(""), names)["collections"], len(lua))
                    lua += v.with_suffix(".bin").read_bytes()
            lang = {}
            labels = list((x / "data/fe/languages").glob("labels/*.BIN"))
            english = list((x / "data/fe/languages").glob("english/*.BIN"))
            if labels and english:
                ids, texts = language_table(labels[0].read_bytes()), language_table(english[0].read_bytes())
                lang = {v.strip(): texts[k] for k, v in ids.items() if k in texts}
            if banks or colls:
                packs.append((pkg.name, banks, colls, lua, lang, bank_bin))
    return packs


def export_textures(online, disc, out, dest):
    """Copy every icon / zoomed map the challenges name into dest/{icons,maps}/<name>.rgba and fill
    online["icons"] with {rgba (relative to private/challenge-map), width, height}."""
    rows = [online["tabs"]]
    challenges = []
    while rows:
        for r in rows.pop():
            rows.append(r.get("rows", []))
            if r.get("challenge"):
                challenges.append(r["challenge"])
    icons = sorted({c["icon"] for c in challenges})
    work = Path(out) / "images"
    extract_project(Path(disc), work, include_dynamic=True, prefixes=tuple(ICON_DIR + n for n in icons), update=True)
    sources = [(n, "icons", n, work / "assets" / ICON_DIR / n,
                json.loads((work / "assets" / ICON_DIR / n / "manifest.json").read_text(encoding="utf-8")))
               for n in icons]
    for image in sorted({c["map_image"] for c in challenges if c["map_image"]}):
        rx2 = next(Path(out).glob(f"dlc/*/x/{MAP_DIR}{image}"), None)
        if rx2 is None:
            raise SystemExit(f"no DLC package carries {MAP_DIR}{image}")
        folder = work / "dlc" / image
        sources.append((image, "maps", Path(image).stem, folder, extract_textures(rx2, folder, force=True, decode_png=True)))
    for key, sub, name, folder, manifest in sources:
        t = next(t for t in manifest["textures"] if t.get("rgba_file"))
        target = Path(dest) / sub / f"{name}.rgba"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((folder / t["rgba_file"]).read_bytes())
        online["icons"][key] = {"rgba": f"online/{sub}/{name}.rgba", "width": t["width"], "height": t["height"]}
    return challenges


class Builder:
    def __init__(self, tree, banks, lua_bin, lang, teleports, dlc_lang=None, dlc_keys=frozenset(),
                 bank_bin=b"", locators=None, polygons=None, oob=None):
        self.t, self.c, self.bin, self.lang, self.tp = Vault(tree), Vault(banks), lua_bin, lang, teleports
        self.dlc_lang, self.dlc_keys = dlc_lang or {}, dlc_keys
        self.bank_bin, self.locators, self.polygons, self.oob = bank_bin, locators or {}, polygons or {}, oob or {}
        parents = {hx(r["parent"]) for r in banks if hx(r["class"]) == CHALLENGES and r["parent"]}
        # Leaf challenges only (group records are challenges too), as their global data.
        self.challenges = [(r["key"], self.c.ref(self.c.field(r, GLOBAL_REF))) for r in banks
                           if hx(r["class"]) == CHALLENGES and hx(r["key"]) not in parents]
        self.worlds = [r for r in self.t.by.values() if hx(r["class"]) == WORLD_CLASS]

    def text(self, sid):
        return sid[1:] if sid and sid.startswith("#") else self.lang.get(sid) or self.dlc_lang.get(sid, sid or "")

    def name(self, sid):
        """Row name: a base string ID as is, a DLC string ID as its '#literal' English text."""
        return sid if sid in self.lang or sid not in self.dlc_lang else "#" + self.dlc_lang[sid]

    def lua(self, r):
        at = int((self.t.field(r, LUA) or "0" * 8)[:8], 16)
        return self.bin[at:self.bin.index(b"\0", at)].decode("latin-1") if at else ""

    def visible(self, r):
        """Evaluate a row's Lua condition for the "not in a session" state."""
        lua = self.lua(r)
        return "NotInSession" in lua or not re.search(r"OnlineMode\.(Host|Client)", lua)

    def world(self, r):
        """WORLD_MAPS key of a world record, through its parents."""
        while r is not None and r["key"] not in WORLD_MAPS:
            r = self.t.by.get((hx(r["class"]), hx(r["parent"]))) if r["parent"] else None
        return r["key"] if r else None

    def challenge_world(self, g):
        return self.world(self.t.by.get((WORLD_CLASS, (self.c.field(g, WORLD) or "")[:16])))

    def teleport(self, title, map_name):
        return next((d["id"] for d in self.tp if d["map"] == map_name and d["name"] == title), None)

    def bank_string(self, data):
        at = int((data or "0" * 8)[:8], 16)
        return self.bank_bin[at:self.bank_bin.index(b"\0", at)].decode("latin-1") if at else ""

    def locator(self, *names):
        """The first named global locator that exists ({matrix, slots, stream}), else None."""
        return next((self.locators[n.lower()] for n in names if n and n.lower() in self.locators), None)

    @staticmethod
    def start(loc):
        """{matrix, slots} of a locator (from locator()), else None."""
        return {"matrix": loc["matrix"], "slots": loc["slots"]} if loc else None

    def spawn(self, key, g):
        """start / boundary / oob_time of an online row's global data, and its locator's stream."""
        loc = self.locator(self.bank_string(self.c.field(g, MAP_START)), self.bank_string(self.c.field(g, LOCATION)))
        return {"start": self.start(loc), "boundary": self.polygons.get(key.lower()),
                "oob_time": self.oob.get(key.lower())}, loc and loc["stream"]

    # Folder contents -------------------------------------------------------------------------------
    def online_challenges(self, flt):
        """(key, global data) of the challenges a filter lets through: its ChallengeType, AvailableOnline
        (the tab filters), not DebugOnly, minus Pro/GettingStarted tags when its Lua excludes them."""
        lua = self.lua(flt)
        group = self.c.ref(self.t.field(flt, FILTER_GROUP))
        if group is None:  # filters without a group reference name their type in Lua only
            group = self.c.by[(GLOBAL, hx(TYPE_GROUPS[need(r"ChallengeType == Challenge\.eChallengeTypes\.(\w+)",
                                                           lua, flt)]))]
        kind = self.c.field(group, TYPE)
        for key, g in self.challenges:
            if g is None or self.c.field(g, TYPE) != kind or not self.c.is_true(g, AVAILABLE_ONLINE) \
                    or self.c.is_true(g, DEBUG_ONLY):
                continue
            tags = (self.c.field(g, TAGS, whole=True) or {}).get("array", {}).get("text_items", [])
            if not any(f'"{t}"' in lua and t in tags for t in ("Pro", "GettingStarted")):
                yield key, g

    def challenge_rows(self, flt, options):
        picked = []
        for key, g in self.online_challenges(flt):
            world = self.challenge_world(g)
            category = self.t.ref(CATEGORY + (self.c.field(g, MAP_CATEGORY) or "")[:16])
            title = self.c.field(g, TITLE)
            bonus = self.c.field(g, BONUS)
            start = self.c.field(g, START)
            if start and not int(start[:24], 16):
                start = None  # unset vector (battles): the game falls back to the map spawn
            objectives = []
            # ponytail: OBJ<n> / OBJ_<n> only; films' named sub-objectives (OBJ_GRAB...) are left out.
            for sep in ("", "_"):
                sid = lambda: f"ID_CHALLENGE_{key.upper()}_OBJ{sep}{len(objectives)}"  # noqa: E731
                while sid() in self.lang or sid() in self.dlc_lang:
                    objectives.append(self.name(sid()))
                if objectives:
                    break
            kind = int(self.c.field(g, TYPE)[:8], 16)
            map_name = WORLD_MAPS.get(world)
            row = {"name": title, "challenge": {
                "id": key, "map": map_name, "teleport": None,
                "position": [round(v, 3) for v in struct.unpack(">3f", bytes.fromhex(start[:24]))] if start else None,
                "description": self.name(self.c.field(g, DESCRIPTION_ONLINE) or self.c.field(g, DESCRIPTION)),
                "objectives": objectives, "bonus": int(bonus[:8], 16) if bonus else 0,
                "icon": "mini_gvr" if "gvr" in key.lower() else ICONS[kind],
                "map_image": MAP_IMAGES[map_name] + ".rx2" if map_name in MAP_IMAGES else None}}
            spawn, stream = self.spawn(key, g)
            row["challenge"].update(spawn)
            # vert_skp5_01 (World dist_megapark) starts on a DIST_University locator: the converted MegaPark has
            # no collision there, the University map does (Super Ultra Mega Park). Travel where the spawn is.
            row["challenge"]["map"] = DISTRICTS.get(stream, map_name)
            order = int(self.t.field(category, CATEGORY_ORDER) or "0", 16)
            picked.append((self.text(title), map_name, order, self.t.field(category, CATEGORY_NAME), row))
        return category_rows(picked, options)

    def location_rows(self, flt):
        place = need(r'IsLocationInWorld\( item, "(\w+)"', self.lua(flt), flt)
        picked = []
        for key, g in self.online_challenges(flt):
            if self.challenge_world(g) != place:
                continue
            title, map_name = self.c.field(g, TITLE), WORLD_MAPS[place]
            picked.append((self.text(title), map_name, {"name": title, "location": {
                "map": map_name, "teleport": self.teleport(self.text(title), map_name), **self.spawn(key, g)[0]}}))
        return [p[2] for p in sort_titles(picked)]

    def park_rows(self):
        parks = [w for w in self.worlds if self.t.is_true(w, IS_SKATEPARK) and self.t.is_true(w, IN_PARKS_TAB)]
        rows = []
        for park in PARK_ORDER:
            here = [w for w in parks if self.world(w) == park]
            if here:
                map_name = WORLD_MAPS[park]
                # Header: the park's map category (Zen Plaza sits under "The Art Gallery"), capture frame 083.
                world = self.t.by[(WORLD_CLASS, hx(park))]
                category = self.t.ref(CATEGORY + (self.t.field(world, MAP_CATEGORY) or "")[:16])
                rows.append({"name": self.t.field(category, CATEGORY_NAME) if category
                             else self.t.field(world, WORLD_NAME), "header": True})
                tp = next((d["id"] for d in self.tp if d["map"] == map_name), None)
                # Spawn: the world's start locator (no lobby slots), else freeskate_<park>_locator (Zen Plaza).
                # ponytail: the base parks' freeskate_<park>_locator has 6 slots but sits at (-110, 2.1, -14.6),
                # outside the converted Downtown Night collision; prefer it once the park conversions are checked.
                fallback = f"freeskate_{park.removeprefix('dist_')}_locator"
                picked = [(self.text(self.t.field(w, WORLD_NAME)), map_name,
                           {"name": self.t.field(w, WORLD_NAME), "location": {
                               "map": map_name, "teleport": tp,
                               "start": self.start(self.locator(self.t.field(w, WORLD_START), fallback)),
                               "boundary": None, "oob_time": None}})
                          for w in here]
                rows += [p[2] for p in sort_titles(picked)]
        return rows

    # Tree ------------------------------------------------------------------------------------------
    def build(self):
        nodes = [r for r in self.t.by.values() if hx(r["class"]) == LISTING]
        kids = {}
        for r in nodes:
            kids.setdefault(hx(r["parent"]), []).append(r)
        order = lambda r: int(self.t.field(r, NODE_ORDER) or "0", 16) >> 16  # noqa: E731
        tabs = []
        for tab in sorted(kids[hx("online")], key=order):
            tab_filter = self.t.ref(self.t.field(tab, NODE_REF))
            # The tab filter's Lua adds its "<tab>_challengetypequickmatch" command to every type
            # sub-list (team, competitive); that command's map category is the Options header.
            quick = re.search(r'item\.Name == "(\w+challengetypequickmatch)"', self.lua(tab_filter))
            options = []
            if quick:
                cmd = self.t.by[("D629DA96E6DE101E", hx(quick.group(1)))]
                category = self.t.ref(CATEGORY + self.t.field(cmd, MAP_CATEGORY)[16:32])
                options = [{"name": self.t.field(category, CATEGORY_NAME), "header": True},
                           {"name": self.t.field(cmd, NAME), "help": self.t.field(cmd, HELPER),
                            "command": quick.group(1)}]
            rows, extras, base_end = [], [], 0
            for node in sorted(kids.get(hx(tab["key"]), []), key=order):
                ref = self.t.field(node, NODE_REF)
                target, kind = self.t.ref(ref), KINDS.get(ref[:16])
                if target is None or not self.visible(target) or (tab["key"], target["key"]) in HIDDEN:
                    continue
                if hx(node["key"]) not in self.dlc_keys:
                    base_end = len(rows) + 1
                if kind == "category":
                    rows.append({"name": self.t.field(target, CATEGORY_NAME), "header": True})
                    continue
                row = {"name": self.t.field(target, NAME)}
                if self.t.field(target, HELPER):
                    row["help"] = self.t.field(target, HELPER)
                if kind == "command":
                    row["command"] = target["key"]
                elif "IsLocationInWorld" in self.lua(target):
                    row.update(folder=True, rows=self.location_rows(target))
                elif "eChallengeTypes" in self.lua(target):
                    row.update(folder=True, rows=self.challenge_rows(target, options))
                else:  # skatepark: worlds with IsSkatePark and AllowInSkateParksTab
                    row.update(folder=True, rows=self.park_rows())
                if (tab["key"], target["key"]) in EXTRAS:
                    extras.append(row)
                    base_end -= 1
                else:
                    rows.append(row)
            rows[base_end:base_end] = extras  # engine extras after the captured base rows, before DLC rows
            if tab["key"] == "online_freeskate":
                for map_name, title in OLD_CITIES.items():
                    picked = [(d["name"], map_name, {"name": "#" + d["name"],
                                                     "location": {"map": map_name, "teleport": d["id"]}})
                              for d in self.tp if d["map"] == map_name]
                    rows.append({"name": "#" + title, "folder": True, "rows": [p[2] for p in sort_titles(picked)]})
            tabs.append({"name": self.t.field(tab_filter, NAME), "rows": rows})

        def names(rows):
            for r in rows:
                r["name"] = self.name(r["name"])
                names(r.get("rows", []))
        names(tabs)
        return {"format": "skate3-online-map", "version": 1, "icons": {}, "tabs": tabs}


# Check -----------------------------------------------------------------------------------------------
def render(online, text):
    """Display strings of every list, keyed like the reference ("Tab" and "Tab/Folder (n)")."""
    def show(r):
        t = text(r["name"]).replace("«", "skate.")
        return "#" + t if r.get("header") else folder_label(t, r["rows"]) if r.get("folder") else t
    lists, tabs = {}, []
    for tab in online["tabs"]:
        name = text(tab["name"])
        tabs.append(name)
        lists[name] = [show(r) for r in tab["rows"]]
        for r in tab["rows"]:
            if r.get("folder"):
                lists[f"{name}/{show(r)}"] = [show(s) for s in r["rows"]]
    return tabs, lists


def strip_count(s):
    return re.sub(r" \(\d+\)$", "", s)


def is_subsequence(expected, got):
    it = iter(got)
    return all(e in it for e in expected)


def check(online, text):
    """Print every mismatch against the captures; True when every list matches. Lists in the
    reference's "save_dependent" depend on the captured save's career progress: "counts" compares
    without folder counts, "subsequence" requires the captured rows in captured order among ours."""
    ref = json.loads(REFERENCE.read_text(encoding="utf-8"))
    tabs, lists = render(online, text)
    by_stem = {strip_count(k): k for k in lists}
    ok = True
    if tabs != ref["tabs"]:
        print(f"MISMATCH tabs: expected {ref['tabs']}, got {tabs}")
        ok = False
    for key, expected in ref["lists"].items():
        got_key = key if key in lists else by_stem.get(strip_count(key))
        got = lists.get(got_key)
        if got is None:
            print(f"MISSING  {key}")
            ok = False
            continue
        mode = ref["save_dependent"].get(key)
        if mode == "counts":
            diff = compare_list([strip_count(e) for e in expected], [strip_count(g) for g in got], None)
        elif mode == "subsequence":
            diff = None if is_subsequence(expected, got) else f"captured rows not in order among {got}"
        else:
            diff = compare_list(expected, got, ref["partial"].get(key))
        if diff is None:
            print(f"MATCH    {key}" + (f" (save-dependent: {mode})" if mode else ""))
            continue
        print(f"MISMATCH {key} (got list {got_key!r}): {diff}")
        ok = False
    if ok:
        print(f"all {len(ref['lists'])} reference lists match")
    return ok


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--disc", required=True, help="retail disc root (contains data/big/db.big)")
    p.add_argument("--dlc", required=True, help="DLC packages root (folder holding the STFS packages)")
    p.add_argument("--assets", required=True, help="prepared installation assets directory")
    p.add_argument("--out", required=True, help="output directory for online.json (DLC extraction cache in dlc/)")
    p.add_argument("--check", action="store_true", help="only check against the captures")
    p.add_argument("--install", action="store_true", help="write into ASSETS/private/challenge-map/online/")
    a = p.parse_args(argv)
    assets, out = Path(a.assets), Path(a.out)
    tree = json.loads((assets / "private/stock/skater-collections.json").read_text(encoding="utf-8"))["collections"]
    lang = json.loads((assets / "private/challenge-map/runtime/challengemap.json").read_text(
        encoding="utf-8"))["language"]
    teleports = json.loads((assets / "private/teleports.json").read_text(encoding="utf-8"))["destinations"]
    installed = {m["name"] for m in json.loads((assets.parent / "maps.json").read_text(encoding="utf-8"))}
    missing = sorted(set(WORLD_MAPS.values()) - installed)
    if missing:
        raise SystemExit(f"maps not in {assets.parent / 'maps.json'}: {missing}")
    with tempfile.TemporaryDirectory() as work:
        banks, lua_bin, bank_bin, oob = extract_db(a.disc, Path(work))
        dlc_lang, dlc_keys = {}, set()
        packs = extract_dlc(a.dlc, out, Path(work) / "data/db/skaterschema")
        for _, dlc_banks, colls, lua, pack_lang, dlc_bin in packs:
            banks += rebase(dlc_banks, len(bank_bin), (LOCATION, MAP_START))
            bank_bin += dlc_bin
            tree += rebase(colls, len(lua_bin))
            lua_bin += lua
            dlc_lang.update(pack_lang)
            dlc_keys |= {hx(r["key"]) for r in colls}
        markers = marker_flags([Path(work) / "data/db", *(out / "dlc" / p[0] / "x/data/db" for p in packs)])
    locators, polygons = spawn_data(a.disc, [out / "dlc" / p[0] / "x" for p in packs])
    builder = Builder(tree, banks, lua_bin, lang, teleports, dlc_lang, dlc_keys, bank_bin, locators, polygons, oob)
    online = builder.build()
    online["markers"] = markers
    ok = check(online, builder.text)
    spots, todo = {}, [online["tabs"]]
    while todo:
        for r in todo.pop():
            todo.append(r.get("rows", []))
            s = r.get("challenge") or r.get("location")
            if s:
                spots[(s.get("id"), r["name"], s["map"])] = s
    unresolved = sorted(f"{m}:{i or n}" for (i, n, m), s in spots.items() if not s.get("start"))
    print(f"spawns: {len(spots)} rows, {sum(1 for s in spots.values() if s.get('start'))} start, "
          f"{sum(1 for s in spots.values() if s.get('start') and len(s['start']['slots']) >= 6)} with 6+ slots, "
          f"{sum(1 for s in spots.values() if s.get('boundary'))} boundary, "
          f"{sum(1 for s in spots.values() if s.get('oob_time'))} oob_time; {len(markers)} marker rules; "
          f"no start: {unresolved}")
    if a.check or not ok:
        return 0 if ok else 1
    # Only new files under private/challenge-map/online/; challengemap.json is never touched.
    dest = assets / "private/challenge-map/online" if a.install else out / "online"
    challenges = export_textures(online, a.disc, out, dest)
    (dest / "online.json").write_text(compact_json(online), encoding="utf-8")
    print(f"wrote online.json: {len(online['tabs'])} tabs, {len(challenges)} challenges, "
          f"{len(online['icons'])} icons -> {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
