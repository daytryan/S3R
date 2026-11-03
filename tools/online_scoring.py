"""Extract retail Skate 3's per-challenge trick-score multiplier gate into scoring.json (schema v1).

py -3 tools/online_scoring.py --disc DISC --dlc DLC --out FILE

Retail (TU3) recomputes the ScoreModule's multiplier switch every frame (scoring-report.md, "Multiplier gate"):
8275F430 -> 82761548 -> input +2788 bit 0x04000000 -> 82DA3C68 -> ScoreModule +130. 82761548 reads the current
challenge's records:
  type      its GlobalData (24491BF0..) record's ChallengeType (02B828CC..), 824639E8;
  types 2/8/15 (demos/street/vert): its LocalData (D7BB2B16..) record's ContestRounds (7AFEB11F..)[round], 824826D0,
            then that round's CumulativeScoring (DD21D0F4..) Bool;
  else      its LocalData record's ScoringMultiplier (CB837EEF..) Bool.
Each lookup walks the collection's parents (82B722C0) and falls back to the zero bytes at 0x830D0850 (= false).
The challenge's start (8256C918) also reads its GlobalData's SetManualAndWalkAsConnector (0661A17A..) Bool, absent
on most rows, which 8256C840 gives every player's ManualsAreConnectors (input +2788 bit 0x40000000), and its LocalData's
connector spot volumes (0A675615..), inside which they connect anyway (8256CB60).
This writes the data; the decision is crates/skate-game/src/online_scoring.rs.

Inputs: DISC/data/big/db.big (challengebanks/main, challenge_local_data_framework, challenge_local_data/*) and the
DLC STFS packages under DLC (their challengebanks/* and challenge_local_data/*). Output:
  {"version": 1, "challenges": {key: {"type": n, "scoring_multiplier": bool, "cumulative_scoring": [bool, ...],
                                      "manual_connector": bool | null, "connector_volumes": n}}}
"""
import argparse
import json
import sys
import tempfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path[:0] = [str(TOOLS), str(TOOLS / "vendor")]
from asset_pipeline.vlt import convert, take, unpack, vault  # noqa: E402
from online_challenge_map import Vault, hx, stfs_extract, CHALLENGES, TYPE  # noqa: E402
from skate3_ui.big import BigArchive  # noqa: E402

GLOBAL_DATA, LOCAL_DATA = "24491BF00316AA45", "D7BB2B160CAF8ED6"
SCORING_MULTIPLIER, CUMULATIVE_SCORING, CONTEST_ROUNDS = 0xCB837EEF50958FBB, 0xDD21D0F4E716D82F, 0x7AFEB11FE8A45EDB
FIELD_CLASS, COLLECTION = 0x2A7895AC4A876152, 0xAD303B8F42B3307E
MANUAL_CONNECTOR, CONNECTOR_VOLUMES = "0661A17AA0E5C774", 0x0A67561520C85C04


def fields(schema_stem, stem, want):
    """{(class, key): (parent, {field: raw})} for the `want` fields of every collection in a vault.

    asset_pipeline.vlt.convert decodes every field and refuses a few retail local-data vaults (street_indu_02,
    vert_indu_02/03, four hom_*) on an unrelated field; this reads only the fields 82761548 and 8256C918 need."""
    sv, sb, se = vault(schema_stem)
    cv, cb, ce = vault(stem)
    classes = {}
    for _, kind, _, at in se:
        if kind == FIELD_CLASS:
            key, _, count, defs, _, _, _ = unpack("QIIIIII", sv, at)
            classes[key] = {unpack("QQHHHBB", sb, defs + i * 24)[0]: unpack("QQHHHBB", sb, defs + i * 24)[1:]
                            for i in range(count)}
    out = {}
    for _, kind, _, at in ce:
        if kind != COLLECTION:
            continue
        key, cls, parent, _, _, count, _, typeslen, layout, _ = unpack("QQQIIIHHII", cv, at)
        got = {}
        for fk, (_, offset, n, _, flags, align) in classes[cls].items():
            if fk in want and flags & 2 and layout:
                got[fk] = read(cb, layout + offset, n, flags, align)
        for i in range(count):
            fk, ptr, _, _, _ = unpack("QIHBB", cv, at + 48 + typeslen * 8 + i * 16)
            if fk in want:
                _, _, n, _, flags, align = classes[cls][fk]
                inline = n <= 4 and not flags & 1
                got[fk] = read(cv, at + 48 + typeslen * 8 + i * 16 + 8, 4, flags, align) if inline \
                    else read(cb, ptr, n, flags, align)
        out[(cls, key)] = (parent, got)
    return out


def read(data, pos, n, flags, align):
    """A field's bytes, or an array field's element list (ModernBase: u16 capacity, count, stride, then elements)."""
    if not flags & 1:
        return take(data, pos, n)
    _, count, stride, _ = unpack("4H", data, pos)
    items, cursor = [], pos + 8
    for _ in range(count):
        cursor = (cursor + (1 << align) - 1) & -(1 << align)
        items.append(take(data, cursor, stride))
        cursor += stride
    return items


def lookup(records, cls, key, field):
    """82B722C0: the field on the collection, else on its parents in turn; None when nothing sets it."""
    seen = set()
    while (cls, key) in records and (cls, key) not in seen:
        seen.add((cls, key))
        parent, got = records[(cls, key)]
        if field in got:
            return got[field]
        key = parent
    return None


def flag(raw):
    """An EA::Reflection::Bool; a missing one reads the zero bytes at 0x830D0850."""
    return raw is not None and raw[0] != 0


def build(banks, records):
    vault_ = Vault(banks)
    out = {}
    parents = {hx(r["parent"]) for r in banks if hx(r["class"]) == CHALLENGES and r["parent"]}
    for r in banks:
        if hx(r["class"]) != CHALLENGES or hx(r["key"]) in parents:
            continue
        g = vault_.ref(vault_.field(r, GLOBAL_DATA))
        local = vault_.field(r, LOCAL_DATA)
        if g is None or not local:
            continue
        kind = int(vault_.field(g, TYPE)[:8], 16)
        cls, key = int(local[:16], 16), int(local[16:32], 16)
        rounds = []
        for ref in lookup(records, cls, key, CONTEST_ROUNDS) or []:
            rcls, rkey = int.from_bytes(ref[:8], "big"), int.from_bytes(ref[8:16], "big")
            rounds.append(flag(lookup(records, rcls, rkey, CUMULATIVE_SCORING)))
        connector = vault_.field(g, MANUAL_CONNECTOR)
        out[r["key"]] = {"type": kind, "scoring_multiplier": flag(lookup(records, cls, key, SCORING_MULTIPLIER)),
                         "cumulative_scoring": rounds,
                         # 8256C918 leaves the players alone when the field is absent (the lookup fails): null.
                         "manual_connector": None if connector is None else connector[:2] != "00",
                         "connector_volumes": lookup(records, cls, key, CONNECTOR_VOLUMES) or 0}
    return out


def gather(db, schema, names, banks, records):
    """Add one data/db tree's challengebanks and challenge_local_data (+ framework) to banks / records."""
    want = {SCORING_MULTIPLIER, CUMULATIVE_SCORING, CONTEST_ROUNDS, CONNECTOR_VOLUMES}
    for v in sorted((db / "challengebanks").glob("*.vlt")):
        banks += convert(schema, v.with_suffix(""), names)["collections"]
    for v in sorted(db.glob("*local_data_framework.vlt")) + sorted((db / "challenge_local_data").glob("*.vlt")):
        for k, record in fields(schema, v.with_suffix(""), want).items():
            if CONNECTOR_VOLUMES in record[1]:
                # tTriggerVolumeInstanceID: the name's offset into this copy's .bin, the volume's id at +8. The
                # framework's default item has id 0, which no trigger volume matches (8256CBF0): count real ones.
                record[1][CONNECTOR_VOLUMES] = sum(any(item[8:16]) for item in record[1][CONNECTOR_VOLUMES])
            # DLC vaults repeat framework collections (parent links); every retail copy agrees on these fields.
            if records.setdefault(k, record) != record:
                raise SystemExit(f"{v.name}: collection {k[1]:016X} disagrees with an earlier copy")


def extract(big, work, prefixes):
    from concurrent.futures import ThreadPoolExecutor
    archive = BigArchive(big)
    written = []
    for e in archive.entries:
        if e.path.lower().startswith(prefixes):
            (work / e.path).parent.mkdir(parents=True, exist_ok=True)
            (work / e.path).write_bytes(archive.read(e))
            written.append(work / e.path)
    # Windows Defender scans each fresh file on its first open (~8 ms): opening them side by side here makes the vault
    # reads below (one file at a time) cheap. Nothing is read from these opens.
    with ThreadPoolExecutor(8) as pool:
        list(pool.map(lambda path: open(path, "rb").close(), written))


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--disc", required=True, help="retail disc root (contains data/big/db.big)")
    p.add_argument("--dlc", help="DLC packages root (folder holding the STFS packages)")
    p.add_argument("--out", required=True, help="scoring.json to write")
    a = p.parse_args(argv)
    names = (TOOLS / "asset_pipeline/names.txt").read_text(encoding="utf-8").splitlines()
    banks, records = [], {}
    db_files = ("data/db/skaterschema", "data/db/challengebanks/", "data/db/challenge_local_data", "data/db/dlc_")
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        extract(Path(a.disc) / "data/big/db.big", work / "base", db_files)
        schema = work / "base/data/db/skaterschema"
        gather(work / "base/data/db", schema, names, banks, records)
        for pkg in sorted(q for q in Path(a.dlc).rglob("*") if q.is_file()) if a.dlc else []:
            with open(pkg, "rb") as f:
                if f.read(4) not in (b"CON ", b"LIVE", b"PIRS"):
                    continue
            out = work / "dlc" / pkg.name
            try:
                stfs_extract(pkg, out)
            except (IndexError, NotImplementedError) as e:
                # Same skip as online_challenge_map.extract_dlc: the ~120 KB unlock packages carry no database.
                print(f"skipped DLC package {pkg.name} ({pkg.stat().st_size} bytes): {e!r}")
                continue
            for big in out.glob("*.big"):
                extract(big, out / "x", db_files[1:])
            gather(out / "x/data/db", schema, names, banks, records)
    challenges = build(banks, records)
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps({"version": 1, "challenges": challenges}, indent=1, sort_keys=True),
                           encoding="utf-8")
    print(f"wrote {a.out}: {len(challenges)} challenges")
    return 0


if __name__ == "__main__":
    sys.exit(main())
