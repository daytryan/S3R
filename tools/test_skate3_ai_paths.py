"""skate3_ai_paths.py on synthetic bytes: a tiny .rx2 built here with the layout peer-ai-paths.md and the real files show
(headers 0x60, nodes 44, ExtendedNodeData 40, branch triples + 24-byte entries padded to 16). No disc needed.
    py -3 -m unittest tools.test_skate3_ai_paths"""
import json
import os
import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path[:0] = [str(Path(__file__).resolve().parent)]
import skate3_ai_paths as ai  # noqa: E402

TRICKS = [(f"t{i}", i % 5, i % 12) for i in range(332)]


def key(prefix, district, kind, row, letters, index):
    return bytes([prefix]) + district.encode() + bytes([kind, row]) + letters.encode().ljust(7, b"\0") + struct.pack(">H", index)


A = key(1, "univ", 12, 1, "A", 0)   # ch_race_univ_01_A_0
B = key(1, "univ", 12, 1, "", 7)    # ch_race_univ_01_7


def build(paths):
    """paths: [(key, nodes, branches)]; node = (pos, step, ticks, state, ext|None), ext = (floats9, trick, b38, b39);
    branch = (node, [(target key, target node, weight)])."""
    start = 0x80
    heads = start + 0x10
    body = bytearray()
    out = bytearray(heads + 0x60 * len(paths))
    for i, (k, nodes, branches) in enumerate(paths):
        h = heads + 0x60 * i
        at_nodes = len(out) + len(body)
        n0 = at_nodes
        ext_at = n0 + 44 * len(nodes)
        nb, eb = bytearray(), bytearray()
        for j, (pos, step, ticks, state, ext) in enumerate(nodes):
            ptr = 0
            if ext:
                ptr = ext_at + len(eb) - (n0 + 44 * j)
                eb += struct.pack(">9fhBB", *ext[0], ext[1], ext[2], ext[3])
            nb += struct.pack(">3f3f", *pos, *step) + b"\x01\x02\x03\x04\x05\x06\x07\x08" + struct.pack(">I", ptr) \
                + bytes([ticks, 255, 9, state]) + b"\x00\x00\x00\x01"
        branch_at = ext_at + len(eb)
        bb = bytearray(12 * len(branches))
        for t, (node, to) in enumerate(branches):
            here = branch_at + 12 * t
            entries = b"".join(tk + struct.pack(">If", tn, w) for tk, tn, w in to)
            entries += b"\xde" * (-len(entries) % 16)
            struct.pack_into(">3I", bb, 12 * t, branch_at + len(bb) - here, len(to), node)
            bb += entries
        body += nb + eb + bb
        struct.pack_into(">3f", out, h, -1, -2, -3)
        struct.pack_into(">3f", out, h + 16, 1, 2, 3)
        out[h + 32:h + 48] = k
        struct.pack_into(">6I", out, h + 48, n0 - h, len(nodes), ext_at - h if eb else 0, branch_at - h if branches else 0,
                         len(branches), 7)
    out += body
    out[0:4] = b"\x89RW4"
    struct.pack_into(">I", out, 0x20, 1)
    struct.pack_into(">I", out, 0x30, 0x60)
    struct.pack_into(">6I", out, 0x60, start, 0, len(out) - start, 0x10, 0, ai.PATH_DATA)
    struct.pack_into(">II", out, start, len(paths), 0x10)
    return bytes(out)


LAUNCH = ((1.5, 2.5, 3.5, 0.0, 4.0, 0.5, 0.0, 0.875, 0.0), 6, 3, 1)
FILE = build([
    (A, [((0, 0, 0), (0, 0, 0), 0, 0, None), ((1, 0, 0), (0.25, 0, 0), 4, 1, LAUNCH), ((2, 0, 0), (0.25, 0, 0), 4, 2, None)],
     [(1, [(B, 1, 0.25), (A, 2, 0.5)])]),
    (B, [((5, 5, 5), (0, 1, 0), 0, 0, None), ((5, 6, 5), (0, 1, 0), 60, 0, ((0,) * 9, -1, 0, 0))], []),
])


class Decode(unittest.TestCase):
    def test_nodes_carry_their_orientations_widths_and_flags(self):
        # round2 T5 (Skate 2 riders): node +24/+28 board/skater quats, u8 x4 -> b*2/255 - 1 normalized (x, y, z, w;
        # 0x823ebb48); +37/+38 half-widths in 0.02 m (left, right); +40 top byte the flags (0x01 flipped .. 0x08 off-board).
        a, _ = ai.decode(FILE, TRICKS)
        q = [b * 2 / 255 - 1 for b in (1, 2, 3, 4)]
        n = sum(x * x for x in q) ** 0.5
        for got, want in zip(a["board"][1], [x / n for x in q]):
            self.assertAlmostEqual(got, want, places=6)
        self.assertAlmostEqual(a["skater"][1][3], (8 * 2 / 255 - 1) / sum((b * 2 / 255 - 1) ** 2 for b in (5, 6, 7, 8)) ** 0.5,
                               places=6)
        self.assertEqual(a["widths"][0], [255, 9])
        self.assertEqual(a["flags"], [0, 0, 0])

    def test_key_names(self):
        self.assertEqual(ai.key_name(A), "ch_race_univ_01_A_0")
        self.assertEqual(ai.key_name(B), "ch_race_univ_01_7")
        self.assertEqual(ai.key_name(key(1, "dwtn", 15, 3, "AA", 41)), "ch_street_dwtn_03_AA_41")
        self.assertEqual(ai.key_name(key(1, "dwmc", 16, 1, "", 0)), "ch_vert_dwmc_01_0")
        with self.assertRaises(ValueError):
            ai.key_name(A[:15])

    def test_nodes_tricks_and_branches(self):
        a, b = ai.decode(FILE, TRICKS)
        self.assertEqual((a["name"], b["name"]), ("ch_race_univ_01_A_0", "ch_race_univ_01_7"))
        self.assertEqual(a["pos"], [[0, 0, 0], [1, 0, 0], [2, 0, 0]])
        self.assertEqual(a["step"][1], [0.25, 0, 0])
        self.assertEqual((a["ticks"], a["state"]), ([0, 4, 4], [0, 1, 2]))
        self.assertEqual((a["bmin"], a["bmax"], a["h44"]), ([-1, -2, -3], [1, 2, 3], 7))
        self.assertEqual(a["raw"][0], "0102030405060708" + "ff09" + "00000001")
        (t,) = a["tricks"]
        self.assertEqual((t["node"], t["trick"], t["name"], t["family"], t["category"], t["launch"]), (1, 6, "t6", 1, 6, True))
        self.assertEqual((t["pos"], t["vel"], t["aux"]), ([1.5, 2.5, 3.5], [0.0, 4.0, 0.5], [0.0, 0.875, 0.0]))
        # A -1 record (a "no trick" marker) is kept, with no name.
        self.assertEqual((b["tricks"][0]["trick"], b["tricks"][0]["name"]), (-1, None))
        # Two entries in one triple: the second sits 24 bytes after the first, padded to 16 after the pair.
        self.assertEqual(a["branches"], [{"node": 1, "to": [{"path": "ch_race_univ_01_7", "node": 1, "weight": 0.25, "found": True},
                                                          {"path": "ch_race_univ_01_A_0", "node": 2, "weight": 0.5, "found": True}]}])
        self.assertEqual((a["anomalies"], b["branches"]), ([], []))
        self.assertEqual((ai.seconds(a), ai.seconds(b)), (8 / 60, 1.0))

    def test_a_trick_outside_the_table_is_kept_and_counted_bad(self):
        data = build([(A, [((0, 0, 0), (0, 0, 0), 0, 0, ((0,) * 9, 400, 0, 0))], [])])
        out = ai.row_export("race_univ_01", data, "disc", ("RaceHeats", []), TRICKS)
        self.assertEqual((out["paths"][0]["tricks"][0]["trick"], out["paths"][0]["tricks"][0]["name"]), (400, None))
        self.assertEqual(out["coverage"]["bad_tricks"], 1)

    def test_a_branch_to_a_missing_path_or_node_is_reported(self):
        gone = key(1, "univ", 12, 1, "", 99)
        data = build([(A, [((0, 0, 0), (0, 0, 0), 0, 0, None)], [(0, [(gone, 0, 1.0)]), (5, [(A, 9, 1.0)])])])
        (p,) = ai.decode(data, TRICKS)
        self.assertFalse(p["branches"][0]["to"][0]["found"])
        self.assertEqual(p["anomalies"], ["branch 1: node 5 of 1", "branch 1: ch_race_univ_01_A_0 node 9 of 1"])

    def test_a_damaged_file_is_refused(self):
        for bad in (FILE[:100], b"\0" * 0x200, FILE[:0x60] + bytes(8) + FILE[0x68:]):
            with self.assertRaises(ValueError):
                ai.decode(bad, TRICKS)
        with self.assertRaises(ValueError):   # a header whose node block lies past the end
            ai.decode(FILE[:0x80 + 0x10 + 0x60 + 0x20], TRICKS)


class Filters(unittest.TestCase):
    def raw(self, offsets, all_flag, flag):
        return struct.pack(">202I", *(list(offsets) + [0] * (200 - len(offsets))), all_flag, flag)

    def test_round_paths(self):
        strings = {10: "ch_race_univ_01_A_0", 40: "ch_race_univ_01_9"}
        self.assertEqual(ai.parse_round_paths(self.raw([10, 0, 40], 0, 1), strings.__getitem__),
                         (["ch_race_univ_01_A_0", "ch_race_univ_01_9"], 0, 1))
        self.assertEqual(ai.parse_round_paths(self.raw([], 1, 0), strings.__getitem__), ([], 1, 0))
        with self.assertRaises(ValueError):
            ai.parse_round_paths(bytes(804), strings.__getitem__)

    def test_coverage_counts_names_the_file_lacks(self):
        got = ai.filter_report(["ch_race_univ_01_A_0", "ch_race_univ_01_9"], 0, 1, {"ch_race_univ_01_A_0"})
        self.assertEqual((got["named"], got["found"], got["missing"], got["all"], got["ignored"]), (2, 1, ["ch_race_univ_01_9"], False, False))
        # all == 1 turns FilterPaths off: listed names (race_univ_04's 11) do nothing.
        got = ai.filter_report(["x"], 1, 0, set())
        self.assertEqual((got["all"], got["ignored"], got["missing"]), (True, True, ["x"]))

    def test_row_export(self):
        filters = ("ContestRounds", [(([ai.key_name(A), "ch_race_univ_01_9"], 0, 1), None), None, (([], 1, 0), None)])
        out = ai.row_export("street_univ_01", FILE, "disc", filters, TRICKS)
        c = out["coverage"]
        self.assertEqual((c["paths"], c["nodes"], c["trick_events"], c["branch_entries"], c["length_s"]), (2, 5, 1, 2, [0.13, 1.0]))
        self.assertEqual([s and (s["found"], s["named"], s["eligible"]) for s in c["sets"]], [(1, 2, 1), None, (0, 0, 2)])
        self.assertEqual(out["filters"][0]["set"], "round")
        # A recording nothing branches into is a root; both here are targets (A_0 of itself, B of A_0).
        self.assertEqual(out["roots"], [])
        self.assertNotIn("raw", out["paths"][0])
        self.assertIn("raw", ai.row_export("street_univ_01", FILE, "disc", filters, TRICKS, raw=True)["paths"][0])
        self.assertEqual(len(out["sha256"]), 64)


class Install(unittest.TestCase):
    """--install DIR: DIR/ai/<row>.json for each row and DIR/ai/tricks.json, the sorted trick ids the rows use (Task 10)."""
    ROWS = {"ftl_univ_02": ai.row_export("ftl_univ_02", FILE, "disc", ("ContestRounds", []), TRICKS),
            "ftl_univ_03": ai.row_export("ftl_univ_03", build([(A, [((0, 0, 0), (0, 0, 0), 0, 0, ((0,) * 9, 40, 0, 0)),
                                                                     ((1, 0, 0), (0, 0, 0), 2, 0, ((0,) * 9, 6, 0, 1)),
                                                                     ((2, 0, 0), (0, 0, 0), 2, 0, ((0,) * 9, -1, 0, 0))], [])]),
                                         "disc", ("ContestRounds", []), TRICKS)}

    def test_the_files_are_one_per_row_and_the_trick_ids(self):
        files = ai.install_files(self.ROWS)
        self.assertEqual(sorted(files), ["ai/ftl_univ_02.json", "ai/ftl_univ_03.json", "ai/tricks.json"])
        # Sorted, each id once, the "no trick" marker (-1) left out: row 02 uses 6, row 03 uses 40, 6 and -1.
        self.assertEqual(json.loads(files["ai/tricks.json"]), [6, 40])
        self.assertEqual(json.loads(files["ai/ftl_univ_02.json"]), json.loads(json.dumps(self.ROWS["ftl_univ_02"])))
        for text in files.values():
            self.assertNotIn("\r", text)
            self.assertTrue(text.endswith("\n"))

    def test_a_trick_outside_the_table_is_not_an_id(self):
        row = ai.row_export("ftl_univ_02", build([(A, [((0, 0, 0), (0, 0, 0), 0, 0, ((0,) * 9, 400, 0, 0))], [])]), "disc",
                            ("ContestRounds", []), TRICKS)
        with self.assertRaises(ValueError):
            ai.install_files({"ftl_univ_02": row})

    def test_install_writes_under_ai_and_leaves_no_temp_file(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "ai").mkdir()
            (Path(d) / "ai/old.json").write_text("{}", encoding="utf-8")
            ai.install(Path(d), ai.install_files(self.ROWS))
            self.assertEqual(sorted(p.name for p in (Path(d) / "ai").iterdir()),
                             ["ftl_univ_02.json", "ftl_univ_03.json", "old.json", "tricks.json"])
            self.assertEqual(json.loads((Path(d) / "ai/tricks.json").read_text(encoding="utf-8")), [6, 40])
            self.assertNotIn(b"\r", (Path(d) / "ai/tricks.json").read_bytes())

    def test_metres_are_the_path_length(self):
        a, b = ai.decode(FILE, TRICKS)
        self.assertEqual((ai.metres(a), ai.metres(b)), (2.0, 1.0))

    def test_the_command_needs_somewhere_to_write(self):
        with self.assertRaises(SystemExit):
            ai.main(["--disc", "nowhere"])


# The Follow the Leader chains, the recordings the leader skates in order (ai.md section 3 and ai_runtime.md 184-186 in the story-mode
# reference; stage1_univ.md section 5): (chain, paths in the file, seconds, metres).
CHAINS = {"ftl_univ_02": ([11, 2, 3, 7, 8, 9, 10], 7, 71.1, 634), "ftl_univ_03": ([3, 12, 5, 6, 7, 8, 9, 10, 11], 10, 91.0, 700)}


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class FtlRows(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rows, cls.coverage = ai.export_rows(Path(os.environ["SKATE3_DISC"]), None, ["ftl_univ_02", "ftl_univ_03"], ai.trick_table())

    def test_ftl_rows_decode(self):
        self.assertEqual(sorted(self.rows), ["ftl_univ_02", "ftl_univ_03"])
        for row, (chain, count, seconds, metres) in CHAINS.items():
            out = self.rows[row]
            self.assertEqual(len(out["paths"]), count, row)
            by_name = {p["name"]: p for p in out["paths"]}
            path = [by_name[f"ch_{row}_{k}"] for k in chain]
            self.assertAlmostEqual(sum(ai.seconds(p) for p in path), seconds, delta=0.1, msg=row)
            self.assertAlmostEqual(sum(ai.metres(p) for p in path), metres, delta=1.0, msg=row)
            self.assertEqual(out["coverage"]["anomalies"], [], row)
            self.assertEqual(out["coverage"]["bad_tricks"], 0, row)

    def test_the_install_files_hold_the_ids_the_two_paths_use(self):
        files = ai.install_files(self.rows)
        ids = json.loads(files["ai/tricks.json"])
        used = {t["trick"] for out in self.rows.values() for p in out["paths"] for t in p["tricks"] if t["trick"] >= 0}
        self.assertEqual(ids, sorted(used))
        self.assertTrue(ids and all(0 <= i < 332 for i in ids))

    def test_the_command_installs_into_a_folder(self):
        with tempfile.TemporaryDirectory() as d:
            ai.main(["--disc", os.environ["SKATE3_DISC"], "--rows", "ftl_univ_02,ftl_univ_03", "--install", d])
            self.assertEqual(sorted(p.name for p in (Path(d) / "ai").iterdir()), ["ftl_univ_02.json", "ftl_univ_03.json", "tricks.json"])
            self.assertEqual(sorted(p.name for p in Path(d).iterdir()), ["ai"])

    def test_a_row_with_no_file_is_refused(self):
        with self.assertRaises(KeyError):
            ai.export_rows(Path(os.environ["SKATE3_DISC"]), None, ["ftl_univ_99"], ai.trick_table())


class Catalog(unittest.TestCase):
    def test_the_real_catalog(self):
        t = ai.trick_table()
        self.assertEqual((len(t), t[0], t[6], t[331][0]), (332, ("fspowerslide", 0, 9), ("bs_5_o", 1, 5), "lateflip_darkcatch"))

    def test_a_catalog_of_the_wrong_size_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "c.rs"
            f.write_text('pub const IDENTIFIERS: [(&str, u32, usize); 2] = [\n ("a", 0, 1),\n ("b", 0, 2),\n];', encoding="utf-8")
            with self.assertRaises(ValueError):
                ai.trick_table(f)


if __name__ == "__main__":
    unittest.main()
