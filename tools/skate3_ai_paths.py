"""Skate 3 Career AI paths: every Career Death Race / Street / Vert row's retail AI recording (`AIPATH_Challenge_ch_<row>.rx2`),
decoded, with the row's own path filters (`tRoundPaths`) and a per-row coverage report.

    py -3 tools/skate3_ai_paths.py --disc DISC --dlc DLC_FOLDER --out OUT [--rows a,b] [--raw] [--install DIR]

Read-only: the disc (an extracted folder holding data/big/db.big and data/big/miscload.big, as skate3_online.py's --disc)
and the DLC packages are only read; everything is written under OUT and DIR.
- --out OUT: OUT/<row>.json for each row, and OUT/coverage.json (the per-row coverage report).
- --install DIR (Task 10 of the story-mode plan): DIR/ai/<row>.json for each row (the same decode, compact) and DIR/ai/tricks.json, the sorted
  trick ids (indexes into the TU3 trick table, the `trick` of a path's trick records) the rows' paths use. tools/skate3_career.py gives the
  Follow the Leader rows (ftl_univ_02, ftl_univ_03: stage1_univ.md section 5) to export_rows and install_files and writes the files with the
  rest of its export, so the career export's ai/ folder is what `--rows ftl_univ_02,ftl_univ_03 --install DIR` writes.

Where the data is
- 31 rows' .rx2 are in the disc's miscload.big (data/content/aipaths/); street_mmcn_01 (Maloof) and vert_dwmc_01 (Hawaii) are in
  their DLC packages. The row's filters are in its challenge_local_data record (RaceHeats[].AIPaths, ContestRounds[].Paths).

The .rx2 (peer-ai-paths.md; RW4 arena, big-endian; every offset is relative to the record that holds it)
- file: +0x20 section count, +0x30 section index offset; an index entry is 6 u32s, the path data section's type id is 0x00EB0014.
- path data: u32 path count, u32 header-array offset (0x10), 0x60-byte headers. Header: bbox min/max (vec4 each), 16-byte key
  (+0x20), nodes (+0x30 offset, +0x34 count), ExtendedNodeData (+0x38), branch table (+0x3C offset, +0x40 count), +0x44 a u32
  (7, or 71 on a few; unrecovered).
- node, 44 bytes: f32x3 position, f32x3 displacement per tick, 4+4 unknown bytes, u32 ExtendedNodeData offset (node-relative, 0 =
  none), u8 ticks since the previous node (1/60 s), u8 +37, u8 +38 (unknown), u8 state (0 normal, 1 takeoff, 2 landing, 4, 66),
  4 flag bytes (+40). Decoded: +24 board / +28 skater quats ("board"/"skater"), +37/+38 half-widths in 0.02 m ("widths",
  left first), +40's top byte ("flags": 0x01 flipped, 0x02 crouch, 0x04 airborne, 0x08 off-board) - round2-research4 Q3.
- ExtendedNodeData, 40 bytes: f32x9 (launch position, launch velocity m/s, a third vector), s16 trick (-1 none; the index into
  crates/skate-core/src/scoring/catalog.rs IDENTIFIERS, the TU3 trick table), u8 +38, u8 +39 (1 = a launch record).
- branch table (resolved here, peer-ai-paths.md U7): `branch count` triples {offset, n, node}, each offset relative to its own
  triple, then n 24-byte entries {16-byte target path key, u32 target node, f32 weight}; a triple's entries are padded with 0xDE
  to 16 bytes (n = 1 takes 32, n = 2 takes 48). A triple is "at this node of this path, hand over to one of these recordings".
- tRoundPaths, 808 bytes: u32 name offsets x200 (into the row .bin; 0 = empty), u32 `all` (1 = no filter), u32 flag804.
"""
import argparse
import hashlib
import json
import math
import os
import re
import struct
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CATALOG = REPO / "crates/skate-core/src/scoring/catalog.rs"
AIPATHS = "data/content/aipaths/AIPATH_Challenge_ch_{}.rx2"   # an `_ol` row's file is its base row's (race_dwtn_01_ol: race_dwtn_01)

# The 33 Career Death Race / Street / Vert rows (peer-ai-paths.md section 4, the order of the Career tab's tables).
ROWS = ("race_dwtn_01_ol race_univ_01 race_univ_02 race_indu_02 race_univ_04 race_dwtn_02 race_univ_03 race_indu_03 race_indu_01 "
        "race_dwtn_03_ol street_univ_02 street_univ_01 street_indu_01 street_dwtn_02 street_dwtn_03 street_indu_03 street_mmcn_01 "
        "street_mmcp_01 street_univ_03 street_dwtn_01 street_indu_02 vert_dwmc_01 vert_univ_01 vert_indu_03 vert_indu_01 "
        "vert_mmcp_01 vert_indu_02 vert_skp5_01 vert_univ_03 vert_dwtn_02 vert_dwtn_03 vert_dwtn_01 vert_univ_02").split()

PATH_DATA = 0x00EB0014
NODE, EXT, ENTRY, HEADER = 44, 40, 24, 0x60
TRICK_COUNT = 332
PREFIXES = ["am", "ch", "ob", "te", "*", "am_*"]   # table 0x830432fc
KINDS = ["1up", "cap", "caf", "demo", "dice", "film", "ftl", "goto", "hom", "otl", "ots", "photo", "race", "skate",
         "domination", "street", "vert"]        # table 0x830432b0
STATES = {0: "normal", 1: "takeoff", 2: "landing", 4: "state4", 66: "state66"}


def f32(x):
    """A f32 as the shortest float that round-trips (JSON then holds the exact value)."""
    return float("%.9g" % x)


def quat(b):
    """A node's packed orientation (+24 board, +28 skater): u8 x4 -> b*2/255 - 1, normalized; x, y, z, w (0x823ebb48)."""
    q = [x * 2 / 255 - 1 for x in b]
    n = sum(x * x for x in q) ** 0.5 or 1
    return [f32(x / n) for x in q]


def key_name(k):
    """16-byte path key -> its CLD name (ch_race_univ_01_A_3). Layout: prefix, 4-char district, kind, row, letter group, u16 index."""
    if len(k) != 16:
        raise ValueError(f"path key of {len(k)} bytes")
    prefix = PREFIXES[k[0]] if k[0] < len(PREFIXES) else str(k[0])
    kind = KINDS[k[5]] if k[5] < len(KINDS) else str(k[5])
    letters = k[7:14].split(b"\0")[0].decode("latin-1")
    index = struct.unpack(">H", k[14:16])[0]
    return f"{prefix}_{kind}_{k[1:5].decode('latin-1')}_{k[6]:02d}" + (f"_{letters}" if letters else "") + f"_{index}"


def trick_table(source=CATALOG):
    """[(identifier, family, category)] x 332 from the Rust catalog (IDENTIFIERS: name, class, score type)."""
    text = Path(source).read_text(encoding="utf-8")
    body = text[text.index("pub const IDENTIFIERS"):]
    rows = re.findall(r'\(\s*"([^"]+)"\s*,\s*(\d+)\s*,\s*(\d+)\s*\)', body[:body.index("];")])
    if len(rows) != TRICK_COUNT:
        raise ValueError(f"{len(rows)} identifiers in {source}, expected {TRICK_COUNT}")
    return [(n, int(c), int(t)) for n, c, t in rows]


def decode(data, tricks):
    """An .rx2's paths. Pure: bytes in, dicts out. Raises ValueError when an offset leaves the file."""
    try:
        return _decode(data, tricks)
    except (struct.error, IndexError) as e:
        raise ValueError(f"rx2 truncated or inconsistent: {e}") from e


def _decode(d, tricks):
    u = lambda o: struct.unpack_from(">I", d, o)[0]  # noqa: E731
    if d[:4] != b"\x89RW4":
        raise ValueError("not an RW4 arena")
    index, count = u(0x30), u(0x20)
    base = next((s[0] for s in (struct.unpack_from(">6I", d, index + 24 * i) for i in range(count)) if s[5] == PATH_DATA), None)
    if base is None:
        raise ValueError(f"no section of type {PATH_DATA:#x}")
    n_paths, hdr = u(base), u(base + 4)
    if base + hdr + HEADER * n_paths > len(d):
        raise ValueError(f"{n_paths} path headers do not fit in {len(d)} bytes")
    heads = [base + hdr + HEADER * i for i in range(n_paths)]
    names = [key_name(d[h + 32:h + 48]) for h in heads]
    sizes = {names[i]: u(h + 52) for i, h in enumerate(heads)}
    paths = []
    for h, name in zip(heads, names):
        no, nc, eo, bo, bc, mask = struct.unpack_from(">6I", d, h + 48)
        pos, step, ticks, state, tr, raw, board, skater, widths, flags = [], [], [], [], [], [], [], [], [], []
        for k in range(nc):
            q = h + no + NODE * k
            pos.append([f32(x) for x in struct.unpack_from(">3f", d, q)])
            step.append([f32(x) for x in struct.unpack_from(">3f", d, q + 12)])
            ticks.append(d[q + 36])
            state.append(d[q + 39])
            board.append(quat(d[q + 24:q + 28]))
            skater.append(quat(d[q + 28:q + 32]))
            widths.append([d[q + 37], d[q + 38]])
            flags.append(d[q + 40])
            raw.append(d[q + 24:q + 32].hex() + d[q + 37:q + 39].hex() + d[q + 40:q + 44].hex())
            ext = u(q + 32)
            if ext:
                t = q + ext
                v = struct.unpack_from(">9f", d, t)
                idx = struct.unpack_from(">h", d, t + 36)[0]
                known = 0 <= idx < len(tricks)
                tr.append({"node": k, "trick": idx, "name": tricks[idx][0] if known else None,
                           "family": tricks[idx][1] if known else None, "category": tricks[idx][2] if known else None,
                           "launch": d[t + 39] == 1, "pos": [f32(x) for x in v[:3]], "vel": [f32(x) for x in v[3:6]],
                           "aux": [f32(x) for x in v[6:9]], "b38": d[t + 38], "b39": d[t + 39]})
        branches, anomalies = [], []
        for i in range(bc):
            at = h + bo + 12 * i
            off, n, node = struct.unpack_from(">3I", d, at)
            if node >= nc:
                anomalies.append(f"branch {i}: node {node} of {nc}")
            to = []
            for j in range(n):
                e = at + off + ENTRY * j
                target = key_name(d[e:e + 16])
                tnode, weight = struct.unpack_from(">If", d, e + 16)
                if target in sizes and tnode >= sizes[target]:
                    anomalies.append(f"branch {i}: {target} node {tnode} of {sizes[target]}")
                to.append({"path": target, "node": tnode, "weight": f32(weight), "found": target in sizes})
            branches.append({"node": node, "to": to})
        paths.append({"name": name, "key": d[h + 32:h + 48].hex(), "bmin": [f32(x) for x in struct.unpack_from(">3f", d, h)],
                      "bmax": [f32(x) for x in struct.unpack_from(">3f", d, h + 16)], "h44": mask, "pos": pos, "step": step,
                      "ticks": ticks, "state": state, "tricks": tr, "branches": branches, "anomalies": anomalies,
                      "raw": raw, "board": board, "skater": skater, "widths": widths, "flags": flags})
    return paths


def seconds(path):
    """The path's length: the ticks after its first node, at 60 Hz (node +36 counts from the previous node)."""
    return sum(path["ticks"][1:]) / 60


def metres(path):
    """The path's length in metres: the straight lines between its node positions (ai.md section 3's 634 m and 700 m for the two chains)."""
    return sum(math.dist(a, b) for a, b in zip(path["pos"], path["pos"][1:]))


def parse_round_paths(raw, resolve):
    """tRoundPaths (808 bytes) -> (names, all, flag804). `resolve(offset)` reads the NUL-terminated name in the row .bin."""
    if len(raw) != 808:
        raise ValueError(f"tRoundPaths of {len(raw)} bytes, expected 808")
    words = struct.unpack(">202I", raw)
    return [n for n in (resolve(w) if w else "" for w in words[:200]) if n], words[200], words[201]


def filter_report(names, all_flag, flag804, in_file):
    """One heat's or round's filter against the file. `ignored`: `all == 1` makes FilterPaths a no-op, so listed names do nothing."""
    found = [n for n in names if n in in_file]
    return {"names": names, "all": all_flag == 1, "flag804": flag804, "named": len(names), "found": len(found),
            "missing": [n for n in names if n not in in_file], "ignored": all_flag == 1 and bool(names)}


def row_filters(db, row):
    """[(tRoundPaths fields, AIPathIndexOverride|None) | None per heat/round] from the row's challenge_local_data."""
    sys.path[:0] = [str(Path(__file__).resolve().parent)]
    import skate3_online as s
    rel = f"challenge_local_data/{row}"
    rec = next((c for c in db.collections(rel) if c["key"] in (row, s.F(row)) and c["class"] == s.LOCAL), None)
    if rec is None:
        raise KeyError(f"no challenge_local_data record for {row}")
    binary, record = db.bin(rel), s.records(db, rel)
    key, field = ("RaceHeats", "AIPaths") if row.startswith("race") else ("ContestRounds", "Paths")
    out = []
    for item in s.items(rec["fields"].get(s.F(key), {})):
        r = record(item)
        if s.F(field) not in r:
            out.append(None)
            continue
        names, all_flag, flag804 = parse_round_paths(bytes.fromhex(r[s.F(field)]["data"]), lambda o: s.cstr(binary, o))
        over = r.get(s.F("AIPathIndexOverride"))
        out.append(((names, all_flag, flag804), struct.unpack(">i", bytes.fromhex(over["data"])[:4])[0] if over else None))
    return key, out


def row_export(row, data, source, filters, tricks, raw=False):
    """One row's output: paths, branch roots, filters and coverage. `filters` = (key, row_filters(...)[1])."""
    paths = decode(data, tricks)
    in_file = {p["name"] for p in paths}
    targeted = {t["path"] for p in paths for b in p["branches"] for t in b["to"]}
    kind, sets = filters
    report = []
    for i, f in enumerate(sets):
        if f is None:
            report.append(None)
            continue
        (names, all_flag, flag804), override = f
        r = filter_report(names, all_flag, flag804, in_file)
        r.update({"index": i, "set": "heat" if kind == "RaceHeats" else "round", "path_index_override": override,
                  "eligible": len(paths) if r["all"] else r["found"]})
        report.append(r)
    nodes = sum(len(p["pos"]) for p in paths)
    lengths = [seconds(p) for p in paths]
    coverage = {"paths": len(paths), "nodes": nodes, "trick_events": sum(1 for p in paths for t in p["tricks"] if t["trick"] >= 0),
                "bad_tricks": sum(1 for p in paths for t in p["tricks"] if t["trick"] >= len(tricks)),
                "branch_triples": sum(len(p["branches"]) for p in paths),
                "branch_entries": sum(len(b["to"]) for p in paths for b in p["branches"]),
                "branch_targets_missing": sorted({t["path"] for p in paths for b in p["branches"] for t in b["to"] if not t["found"]}),
                "anomalies": [f"{p['name']}: {a}" for p in paths for a in p["anomalies"]],
                "length_s": [round(min(lengths), 2), round(max(lengths), 2)] if lengths else None,
                "sets": [None if r is None else {k: r[k] for k in ("named", "found", "missing", "all", "ignored", "eligible")}
                         for r in report]}
    out = {"row": row, "kind": row.split("_")[0], "source": source, "sha256": hashlib.sha256(data).hexdigest(),
           "ticks_per_second": 60, "filters": report, "roots": sorted(in_file - targeted), "coverage": coverage,
           "paths": paths}
    if not raw:
        for p in paths:
            del p["raw"]
    return out


def summary(c):
    sets = "; ".join("none" if s is None else f"{s['found']}/{s['named']}" + (" all" if s["all"] else "") for s in c["sets"])
    return (f"{c['paths']:>4} paths {c['nodes']:>6} nodes {c['trick_events']:>4} tricks {c['branch_entries']:>4} branches"
            f"  sets {sets}" + (f"  MISSING TARGETS {c['branch_targets_missing']}" if c["branch_targets_missing"] else "")
            + (f"  ANOMALIES {len(c['anomalies'])}" if c["anomalies"] else ""))


def export_rows(disc, dlc, rows, tricks, raw=False, say=lambda text: None, strict=True):
    """({row: row_export output}, {row: coverage}) for each row of `rows`, from the .rx2 files of the disc (an extracted folder) and, when
    `dlc` (a folder of STFS packages) is given, of the packages. A row with no .rx2 anywhere is a KeyError when `strict` (the career export
    needs every row it asks for); otherwise it gets {"error": ...} in the coverage and no output (the command's default rows include two DLC
    rows). Read-only; the work folder is a temporary one."""
    sys.path[:0] = [str(Path(__file__).resolve().parent)]
    import skate3_online as s
    outs, coverage = {}, {}
    with tempfile.TemporaryDirectory(prefix="skate3-ai-paths-") as work:
        work = Path(work)
        db = s.Db(disc, work / "db")
        misc = s.big(disc, "data/big/miscload.big")
        sources = {e.path.lower(): ("disc", db, misc, e) for e in misc.entries if e.path.lower().startswith("data/content/aipaths/")}
        for d in s.dlc_packages(dlc, db, work / "dlc") if dlc else []:
            for e in d.archive.entries:
                if e.path.lower().startswith("data/content/aipaths/"):
                    sources.setdefault(e.path.lower(), (getattr(d.archive.path, "name", "dlc"), d, d.archive, e))
        for row in rows:
            found = sources.get(AIPATHS.format(row.removesuffix('_ol')).lower())
            if found is None:
                if strict:
                    raise KeyError(f"{row}: no .rx2 on the disc or in the DLC packages")
                coverage[row] = {"error": "no .rx2 on the disc or in the DLC packages"}
                say(f"{row:<18} NO FILE")
                continue
            source, rdb, archive, entry = found
            outs[row] = row_export(row, archive.read(entry), source, row_filters(rdb, row), tricks, raw)
            coverage[row] = {"source": source, **outs[row]["coverage"]}
            say(f"{row:<18} {source:<22} {summary(outs[row]['coverage'])}")
    return outs, coverage


def install_files(outs):
    """{"ai/<row>.json": text, "ai/tricks.json": text} for `outs` ({row: row_export output}): each row's decode, compact, and the sorted ids
    of the tricks the rows' paths use (each once; the -1 "no trick" marker is not an id). An id past the TU3 table (a bad_tricks record) is a
    ValueError: the file is not what the table describes. The text ends with a newline and has no CR."""
    ids = set()
    for row, out in outs.items():
        for p in out["paths"]:
            for t in p["tricks"]:
                if t["trick"] >= TRICK_COUNT:
                    raise ValueError(f"{row}: {p['name']} node {t['node']} names trick {t['trick']}, past the table's {TRICK_COUNT}")
                if t["trick"] >= 0:
                    ids.add(t["trick"])
    files = {f"ai/{row}.json": json.dumps(out, separators=(",", ":")) + "\n" for row, out in outs.items()}
    files["ai/tricks.json"] = json.dumps(sorted(ids), separators=(",", ":")) + "\n"
    return files


def install(folder, files):
    """Write install_files' text under `folder`: each file lands under a temp name and is renamed over the old one (UTF-8, LF)."""
    folder = Path(folder)
    for rel, text in files.items():
        target = folder / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        tmp = target.with_name(target.name + ".tmp")
        try:
            tmp.write_bytes(text.encode("utf-8"))
            os.replace(tmp, target)
        finally:
            tmp.unlink(missing_ok=True)


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--disc", type=Path, required=True, help="extracted Skate 3 disc folder (data/big/db.big, miscload.big)")
    p.add_argument("--dlc", type=Path, help="folder of the Skate 3 DLC STFS packages (street_mmcn_01, vert_dwmc_01)")
    p.add_argument("--out", type=Path, help="folder for <row>.json and coverage.json")
    p.add_argument("--install", type=Path, help="folder that receives ai/<row>.json and ai/tricks.json (the career export's folder)")
    p.add_argument("--rows", default=",".join(ROWS))
    p.add_argument("--raw", action="store_true", help="keep each node's unrecovered bytes (+24..31, +37, +38, +40..43) as hex")
    a = p.parse_args(argv)
    if a.out is None and a.install is None:
        p.error("give --out, --install or both")
    outs, coverage = export_rows(a.disc, a.dlc, [r for r in a.rows.split(",") if r], trick_table(), a.raw, print, strict=False)
    if a.out is not None:
        a.out.mkdir(parents=True, exist_ok=True)
        for row, out in outs.items():
            (a.out / f"{row}.json").write_text(json.dumps(out, separators=(",", ":")), encoding="utf-8", newline="\n")
        (a.out / "coverage.json").write_text(json.dumps(coverage, indent=1), encoding="utf-8", newline="\n")
    if a.install is not None:
        install(a.install, install_files(outs))


if __name__ == "__main__":
    main()
