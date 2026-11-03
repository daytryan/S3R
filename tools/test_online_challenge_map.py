import unittest
from online_challenge_map import sort_titles, folder_label, compare_list

class OnlineMapTest(unittest.TestCase):
    def test_sort_black_box_first_then_letters_then_digits(self):
        rows = [("1-Up Spillage", "Industrial"), ("Aletown 1-Up", "DownTown"), ("Black Box 1-Up", "BlackBoxPark"),
                ("Art it Up", "Art_Gallery")]
        self.assertEqual([t for t, _ in sort_titles(rows)],
                         ["Black Box 1-Up", "Aletown 1-Up", "Art it Up", "1-Up Spillage"])

    def test_sort_letters_before_spaces_and_punctuation(self):
        # Retail captures: "Canal SKATE" < "Can You Spill SKATE?", "Ripple Threat" < "Rip That Line",
        # "Teaming the Kube" < "Team Mega Meltdown", "Drydocks Duel" < "D.I.Y.".
        rows = [(t, "DownTown") for t in ["Can You Spill SKATE?", "Canal SKATE", "Rip That Line", "Ripple Threat",
                                          "Team Mega Meltdown", "Teaming the Kube", "D.I.Y.", "Drydocks Duel"]]
        self.assertEqual([t for t, _ in sort_titles(rows)],
                         ["Canal SKATE", "Can You Spill SKATE?", "Drydocks Duel", "D.I.Y.", "Ripple Threat",
                          "Rip That Line", "Teaming the Kube", "Team Mega Meltdown"])

    def test_folder_label_counts_challenges_only(self):
        rows = [{"name": "h", "header": True}, {"name": "q", "command": "x"}, {"name": "a", "challenge": {}},
                {"name": "b", "challenge": {}}]
        self.assertEqual(folder_label("1-Up", rows), "1-Up (2)")

    def test_compare_list_reports_first_difference(self):
        self.assertIsNone(compare_list(["#A", "b"], ["#A", "b"], None))
        self.assertEqual(compare_list(["#A", "b"], ["#A", "c"], None), "row 2: expected 'b', got 'c'")
        self.assertIsNone(compare_list(["#A", "b"], ["#A", "b", "c"], 2), "partial: prefix only")

    def test_dlc_categories_sharing_an_order_interleave_headers(self):
        # Capture (Career Film): "#Films" rows, then DW / Art Gallery headers alternating by title.
        from online_challenge_map import category_rows
        picked = [(t, m, o, c, {"name": t}) for t, m, o, c in [
            ("Gazebo Film", "Danny_Way_Hawaiian_Dream", 20000, "DW"), ("Curtains", "DownTown", 10, "Films"),
            ("The Art of Rail", "Art_Gallery", 20000, "AG"),
            ("Danny's Mega Gaps", "Danny_Way_Hawaiian_Dream", 20000, "DW"),
            ("Gallery Gap Line", "Art_Gallery", 20000, "AG")]]
        self.assertEqual([("#" if r.get("header") else "") + r["name"] for r in category_rows(picked)],
                         ["#Films", "Curtains", "#DW", "Danny's Mega Gaps", "#AG", "Gallery Gap Line", "#DW",
                          "Gazebo Film", "#AG", "The Art of Rail"])

    def test_global_locators_read_matrix_and_spawn_slots_in_table_order(self):
        import struct
        from online_challenge_map import global_locators

        def m(x):
            return struct.pack(">16f", 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, 2, 3, 1)
        # Section at 64: header, one 128 B record @32, two 112 B children @160, names @384.
        names = b"Spot\0Spot::s_spawn_2\0Spot::s_spawn_1\0"
        record = m(10) + bytes(32) + struct.pack(">4I", 2, 160, 384, 0) + bytes(16)
        kids = b"".join(m(x) + bytes(32) + struct.pack(">I", at) + bytes(12) for x, at in [(22, 389), (11, 405)])
        section = struct.pack(">5I", 1, 2, 0, 32, 160) + bytes(12) + record + kids + names
        header = bytearray(64)
        struct.pack_into(">I", header, 32, 1)
        struct.pack_into(">I", header, 48, 64 + len(section))
        raw = bytes(header) + section + struct.pack(">6I", 64, 0, len(section), 0, 0, 0xEB0009)
        loc = global_locators(raw)["spot"]
        self.assertEqual(loc["matrix"][3], [10, 2, 3, 1])
        # The resolver (S2 0x82879950 / S1 0x823d9088) indexes the child table: _spawn_2 listed first is slot 0.
        self.assertEqual([s[3][0] for s in loc["slots"]], [22, 11], "slots in table order, not by _spawn_<n>")

    def test_boundary_points_are_x_z(self):
        from online_challenge_map import boundary
        self.assertEqual(boundary('<Boundary>\n  <Point>-78, -88</Point>\n  <Point>156.5,-79</Point>\n</Boundary>'),
                         [[-78.0, -88.0], [156.5, -79.0]])

    def test_rebase_moves_only_the_named_set_offsets(self):
        from online_challenge_map import rebase, LOCATION, MAP_START, LUA
        r = {"key": "k", "fields": {"Hash_" + LOCATION: {"data": "00000010"}, "Hash_" + MAP_START: {"data": "00000000"},
                                    "Hash_" + LUA: {"data": "00000004"}}}
        f = rebase([r], 0x100, (LOCATION, MAP_START))[0]["fields"]
        self.assertEqual(f["Hash_" + LOCATION]["data"], "00000110")
        self.assertEqual(f["Hash_" + MAP_START]["data"], "00000000", "unset stays unset")
        self.assertEqual(f["Hash_" + LUA]["data"], "00000004")
        self.assertEqual(r["fields"]["Hash_" + LOCATION]["data"], "00000010", "input untouched")

    def test_compact_json_puts_number_arrays_on_one_line(self):
        from online_challenge_map import compact_json
        v = {"name": "Top [1, 2]", "start": {"matrix": [[1, 0], [-2.5, 1e-05]], "slots": []}, "rows": [{"a": [3]}]}
        text = compact_json(v)
        self.assertIn('"matrix": [[1,0],[-2.5,1e-05]]', text)
        self.assertIn('"Top [1, 2]"', text, "strings untouched")
        self.assertEqual(__import__("json").loads(text), v)

if __name__ == "__main__":
    unittest.main()
