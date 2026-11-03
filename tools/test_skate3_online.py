import json
import os
import struct
import tempfile
import unittest
from pathlib import Path

import skate3_online as s


class SnippetTreeTest(unittest.TestCase):
    def test_numeric_for_scopes_its_variable_to_the_body(self):
        tree, natives = s.snippet_tree("local n = 0 for i = 1, 3 do n = n + i end return i")
        self.assertEqual(tree, [["local", [0], [["num", 0.0]]],
                                ["for", 1, ["num", 1.0], ["num", 3.0], None,
                                 [["set", [["slot", 0]], [["bin", "+", ["slot", 0], ["slot", 1]]]]]],
                                ["return", [["global", "i"]]]])


class LuaEnumTest(unittest.TestCase):
    def test_reads_key_value_pairs_from_the_builder(self):
        # A registration pair {"Foo", builder}; the builder pushes "A" then loads 3.0, then "B" and 0.5.
        d = bytearray(0x400)
        B = s.BASE
        d[0x100:0x105] = b"\0Foo\0"
        d[0x110:0x113] = b"\0A\0"
        d[0x118:0x11B] = b"\0B\0"
        d[0x120:0x128] = struct.pack(">ff", 3.0, 0.5)
        d[0x130:0x138] = struct.pack(">II", B + 0x101, B + 0x200)
        ha = lambda v: ((v + 0x8000) >> 16) & 0xFFFF  # noqa: E731
        lo = lambda v: v & 0xFFFF  # noqa: E731
        code = []
        for key, val in ((B + 0x111, B + 0x120), (B + 0x119, B + 0x124)):
            code += [(15 << 26) | (4 << 21) | ha(key), (14 << 26) | (4 << 21) | (4 << 16) | lo(key),
                     (18 << 26) | 0x100 | 1,                                        # bl (lua_pushlstring)
                     (15 << 26) | (11 << 21) | ha(val), (48 << 26) | (1 << 21) | (11 << 16) | lo(val)]
        code.append(0x4E800020)
        d[0x200:0x200 + 4 * len(code)] = b"".join(struct.pack(">I", w) for w in code)
        self.assertEqual(s.lua_enum(s.Image(bytes(d)), "Foo"), {"A": 3, "B": 0.5})


class LocalBoolTest(unittest.TestCase):
    """The hom rows' WIPEOUT_RESETS_ACTIVE (peer-task10.md U4): a local data Bool with no recovered name, read per row."""

    @staticmethod
    def bool_(byte):
        return {"type": "EA::Reflection::Bool", "data": byte + "000000"}

    def test_the_rows_own_value_else_the_chains_else_false(self):
        k = s.WIPEOUT_RESETS_ACTIVE
        self.assertIs(s.local_bool({k: self.bool_("01")}, k), True)
        self.assertIs(s.local_bool({k: self.bool_("00")}, k), False)
        self.assertIs(s.local_bool({}, k), False)          # no record in the chain sets it: the attribute's default
        self.assertIs(s.local_bool({"Hash_1": self.bool_("01")}, k), False)   # other fields are not it

    def test_a_field_that_is_not_a_bool_is_refused_not_read_as_true(self):
        k = s.WIPEOUT_RESETS_ACTIVE
        with self.assertRaises(ValueError):
            s.local_bool({k: {"type": "EA::Reflection::Int32", "data": "00000001"}}, k)
        with self.assertRaises(ValueError):
            s.local_bool({k: {"type": "EA::Reflection::Bool", "data": "01", "array": {"items": ["01"]}}}, k)

    def test_the_hom_mode_exports_it_and_no_other_mode_does(self):
        self.assertEqual(s.MODES["hom"]["bool_fields"], {"wipeout_resets_active": "Hash_59F6A63F5900A8C9"})
        self.assertEqual([m for m, spec in s.MODES.items() if spec.get("bool_fields")], ["hom"])

    @unittest.skipUnless(os.environ.get("SKATE3_ONLINE_EXPORT"), "set SKATE3_ONLINE_EXPORT to an export's folder (the one holding hom/rules.json)")
    def test_an_export_has_exactly_the_four_coop_points_rows(self):
        rows = json.loads((Path(os.environ["SKATE3_ONLINE_EXPORT"]) / "hom/rules.json").read_text(encoding="utf-8"))["rows"]
        self.assertTrue(all(isinstance(r["wipeout_resets_active"], bool) for r in rows.values()))
        self.assertEqual(sorted(k for k, r in rows.items() if r["wipeout_resets_active"]),
                         ["hom_dwtn_03_sp", "hom_dwtn_06_sp", "hom_univ_03_sp", "hom_univ_07_sp"])


class SkateOutlineTest(unittest.TestCase):
    # skate_indu_03's and skate_indu_02's VisualIndicators as the disc has them (tVisualEditorData, 80 bytes).
    LOCATOR = "00000000000000000000005C00000000000000000000000000000117000000003297556BECB605E2B094183C726DEB4E0000000000000000F0DB8767D611AF94D7EDBD362D7D21520000000000000000"
    SPLINE = "000000010000000000000117000000002C7017070040000100000130000000003297556BECB605E2B094183C726DEB4E0000000000000000F0DB8767D611AF94D7EDBD362D7D21520000000000000000"

    def test_a_kind_1_entry_is_its_spline_outline_and_no_arrow(self):
        v = {"array": {"items": [self.LOCATOR, self.SPLINE]}}
        square = [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]
        out = s.outlines(v, {0x2C70170700400001: {"flags": 8, "points": square}})
        self.assertEqual(out, [{"points": square, "closed": True}])
        binary = bytearray(0x200)
        binary[0x117:0x11B] = b"loc\0"
        arrows = s.chevrons(v, bytes(binary), {"loc": {"matrix": "m"}}, kinds=(0,))
        self.assertEqual([a["locator"] for a in arrows], ["loc"], "the kind-1 entry is not an arrow")
        self.assertEqual(s.outlines(v, {}), [], "a spline the mission lacks draws nothing")


class RaceRibbonTest(unittest.TestCase):
    """Bug 10-09 #6: race_shared's AddTriggerIndicator / AddIndicator ribbons (gates, grey / final / leg-finish, chevrons)."""
    # race_dwtn_02's first gate RibbonInstance, as the disc has it.
    GATE = "3297556BECB605E2624E29026A97C02E0000000000000000F0DB8767D611AF94D7EDBD362D7D21520000000000000000"

    def test_a_ribbon_instance_is_its_type_and_style_keys(self):
        v = {"type": "Sk8::Challenge::tSplineBankObject", "data": self.GATE}
        self.assertEqual(s.decode(v, None, {}, {}), {"ribbon": "624E29026A97C02E", "style": "D7EDBD362D7D2152"})
        self.assertIn("RibbonInstance", s.GATE_FIELDS)
        self.assertTrue({"RaceGateGreyRibbon", "RaceFinalGateRibbon", "RaceLegFinishGateRibbon"} <= set(s.MODES["race"]["fields"]))

    def test_chevrons_name_their_locator_and_ribbon_and_skip_untyped_rows(self):
        binary = b"\0chev_01\0chev_02\0"
        row = lambda at, kind: ("00" * 8 + "00000008" + "00" * 12 + "%08X" % at + "00" * 4 + "3297556BECB605E2" + kind  # noqa: E731
                                + "00" * 8 + "F0DB8767D611AF94" + "D7EDBD362D7D2152" + "00" * 8)
        v = {"type": "Sk8::Challenge::tVisualEditorData", "array": {"items": [row(1, "D6C61BC5BC0F285A"), row(9, "00" * 8)]}}
        out = s.chevrons(v, binary, {"chev_01": {"matrix": [[1]]}})
        self.assertEqual(out, [{"locator": "chev_01", "matrix": [[1]], "ribbon": "D6C61BC5BC0F285A", "style": "D7EDBD362D7D2152"}])
        self.assertEqual(s.chevrons(None, binary, {}), [])

    @unittest.skipUnless(os.environ.get("SKATE3_ONLINE_EXPORT"), "set SKATE3_ONLINE_EXPORT to an export's folder (the one holding race/rules.json)")
    def test_an_export_draws_every_ribbon_it_names(self):
        root = Path(os.environ["SKATE3_ONLINE_EXPORT"]) / "race"
        rows = json.loads((root / "rules.json").read_text(encoding="utf-8"))["rows"]
        ind = json.loads((root / "indicators.json").read_text(encoding="utf-8"))
        looks = [r[k] for r in rows.values() for k in ("RaceGateGreyRibbon", "RaceFinalGateRibbon", "RaceLegFinishGateRibbon")]
        looks += [g["RibbonInstance"] for r in rows.values() for h in r["heats"] for leg in h["Legs"] for g in leg["Gates"]]
        looks += [c for r in rows.values() for c in r["chevrons"]]
        self.assertTrue(looks)
        for look in looks:
            self.assertGreater(len(ind["shapes"][look["ribbon"]]), 1)
            self.assertTrue((root / "indicator" / ind["styles"][look["style"]]["texture"]).is_file())


class Bank:
    """A db as template_rows / otl_lots read it: challengebanks records, an empty local data file per row marked
    `local`, and (a DLC package's) its own templates by StateGraph."""

    def __init__(self, work, records, templates=None):
        self.work, self.records, self.own = Path(work), records, templates or {}
        local = self.work / "data/db/challenge_local_data"
        local.mkdir(parents=True)
        for r in records:
            if r.get("local"):
                (local / f"{r['key']}.vlt").write_bytes(b"")

    def collections(self, rel):
        return self.records

    def templates(self, graph):
        return tuple(self.own.get(graph, ()))

    def copies(self, names, graph):
        return ()   # as DlcDb.copies for Own the Lot: no package template runs its mode graph (freeskate_ol)


def challenge(key, parent, local=True):
    return {"class": s.CHALLENGE, "key": key, "parent": parent, "fields": {}, "local": local}


def lot_data(key, activities):
    """A lot's challenge_global_data: OnlineFreeSkateActivities, refspecs (class, key = the activity's name hash, 0)."""
    refs = ["E2A50788B6C20C92" + s.F(a)[5:] + "00" * 8 for a in activities]
    return {"class": s.GLOBAL, "key": key, "parent": "", "fields": {s.ONLINE_FREESKATE_ACTIVITIES: {"array": {"items": refs}}}}


class OtlDlcLotsTest(unittest.TestCase):
    """Online audit 10-28: the five DLC lots (otl_artg_01, otl_dwmc_01 / 02, otl_mmcn_01 / 02). Each package has its
    own lot template (dlc_mmcn_own_the_lots: StateGraph "freeskate", as the disc's own_the_lots) and activity templates
    (dlc_mmcn_freeskate_activities_<kind>: StateGraph the freeskate_ol_* graph); no parent chain reaches the disc's."""

    def banks(self, tmp):
        disc = Bank(Path(tmp) / "disc", [
            challenge("own_the_lots", "main", local=False),
            challenge("freeskate_activities_accumulation", "freeskate_activities", local=False),
            challenge("otl_dwtn_01", "own_the_lots"), challenge("otl_dwtn_01_accu_01", "freeskate_activities_accumulation"),
            lot_data("otl_dwtn_01", ["otl_dwtn_01_accu_01"])])
        own = [challenge("dlc_mmcn_own_the_lots", "dlc_mmcn"), challenge("dlc_mmcn_freeskate_activities_gap_tag", "dlc_mmcn_freeskate_activities"),
               challenge("otl_mmcn_01", "dlc_mmcn_own_the_lots"), challenge("otl_mmcn_01_gap_01", "dlc_mmcn_freeskate_activities_gap_tag"),
               challenge("otl_mmcn_01_gap_02", "dlc_mmcn_freeskate_activities_gap_tag"),
               lot_data("otl_mmcn_01", ["otl_mmcn_01_gap_02", "otl_mmcn_01_gap_01"]),
               # The package's map free skate runs the same graph (freeskate_dlc_maloof_nyc): no activities, no lot.
               challenge("dlc_mmcn_freeskate", "dlc_mmcn"), challenge("freeskate_dlc_maloof_nyc", "dlc_mmcn_freeskate")]
        base = [dict(r, local=False) for r in disc.records]   # a package's bank: the disc's records, then its own
        pkg = Bank(Path(tmp) / "pkg", base + own, {"freeskate": ["dlc_mmcn_own_the_lots", "dlc_mmcn_freeskate"],
                                                   "freeskate_ol_gap_tag": ["dlc_mmcn_freeskate_activities_gap_tag"]})
        return disc, pkg

    def test_a_packages_lots_list_its_activities_in_order_with_their_graph(self):
        with tempfile.TemporaryDirectory() as tmp:
            disc, pkg = self.banks(tmp)
            rows = {k: {} for k in ("otl_dwtn_01_accu_01", "otl_mmcn_01_gap_01", "otl_mmcn_01_gap_02")}
            lots = s.otl_lots(disc, rows, [pkg])
        self.assertEqual(lots, {"otl_dwtn_01": ["otl_dwtn_01_accu_01"], "otl_mmcn_01": ["otl_mmcn_01_gap_02", "otl_mmcn_01_gap_01"]})
        self.assertEqual({k: r.get("graph") for k, r in rows.items()},
                         {"otl_dwtn_01_accu_01": "freeskate_ol_accumulation", "otl_mmcn_01_gap_01": "freeskate_ol_gap_tag",
                          "otl_mmcn_01_gap_02": "freeskate_ol_gap_tag"})

    def test_a_package_plays_own_the_lot_on_its_own_activity_templates(self):
        self.assertTrue(s.MODES["otl"].get("dlc"), "Own the Lot reads the DLC packages")
        with tempfile.TemporaryDirectory() as tmp:
            _, pkg = self.banks(tmp)
            self.assertEqual(s.package_templates(pkg, "otl"), ("dlc_mmcn_freeskate_activities_gap_tag",))
            self.assertEqual(s.package_templates(pkg, "contest"), (), "a mode of disc templates: their copies")


class DominationLooksTest(unittest.TestCase):
    """G3 (online audit 10-28): Domination's spots take RibbonIndicatorNeutral in RibbonColourNeutral, then
    RibbonIndicatorOwned in each RibbonColoursPlayers / RibbonColoursTeams colour (domination.lua 461-464, 531-548)."""
    # dom_dwtn_01's, as the export has them: type / style refspecs (class, key, 0).
    ROW = {"RibbonIndicatorNeutral": "3297556BECB605E2B094183C726DEB4E0000000000000000",
           "RibbonIndicatorOwned": "3297556BECB605E2769C2AEE9C86C11A0000000000000000",
           "RibbonColourNeutral": "F0DB8767D611AF941DA37485495114CE0000000000000000",
           "RibbonColoursPlayers": ["F0DB8767D611AF945D7D090ED988F3D30000000000000000",
                                    "F0DB8767D611AF9454603FFC31D8DBF30000000000000000"],
           "RibbonColoursTeams": ["F0DB8767D611AF945D7D090ED988F3D30000000000000000"]}

    def test_every_look_a_spot_takes(self):
        self.assertEqual(s.domination_looks({"dom_dwtn_01": self.ROW, "older": {}}),
                         [("769C2AEE9C86C11A", "54603FFC31D8DBF3"), ("769C2AEE9C86C11A", "5D7D090ED988F3D3"),
                          ("B094183C726DEB4E", "1DA37485495114CE")])

    def test_the_look_file_names_each_look_for_skate2_arrows(self):
        found = {"shapes": {"769C2AEE9C86C11A": [[0, 0], [1, 1]]},
                 "styles": {"5D7D090ED988F3D3": {"name": "red", "texture": "visual_indicator_red.png",
                                                 "background": "visual_indicator_bg_red.png", "tint": [1, 1, 1, 1]}}}
        out = s.look_file("domination", found, [("769C2AEE9C86C11A", "5D7D090ED988F3D3")])
        self.assertEqual(out["locators"], {"skate3:769C2AEE9C86C11A:5D7D090ED988F3D3":
                                           {"shape": "skate3:769C2AEE9C86C11A", "style": "skate3:5D7D090ED988F3D3"}})
        self.assertEqual(out["shapes"], {"skate3:769C2AEE9C86C11A": [[0, 0], [1, 1]]})
        self.assertEqual(out["styles"]["skate3:5D7D090ED988F3D3"],
                         {"texture": "private/skate3-online/domination/indicator/visual_indicator_red.png", "tint": [1, 1, 1, 1],
                          "background": "private/skate3-online/domination/indicator/visual_indicator_bg_red.png"})

    @unittest.skipUnless(os.environ.get("SKATE3_ONLINE_EXPORT"), "set SKATE3_ONLINE_EXPORT to an export's folder (the one holding domination/rules.json)")
    def test_an_export_draws_every_look_a_spot_takes(self):
        root = Path(os.environ["SKATE3_ONLINE_EXPORT"]) / "domination"
        rows = json.loads((root / "rules.json").read_text(encoding="utf-8"))["rows"]
        ind = json.loads((root / "indicators.json").read_text(encoding="utf-8"))
        looks = s.domination_looks(rows)
        self.assertGreaterEqual(len(looks), 7, "neutral + 6 player colours")
        for t, st in looks:
            look = ind["locators"][f"skate3:{t}:{st}"]
            self.assertGreater(len(ind["shapes"][look["shape"]]), 1)
            self.assertTrue((root / "indicator" / Path(ind["styles"][look["style"]]["texture"]).name).is_file())


class ModeLooksTest(unittest.TestCase):
    """G7 (online audit 10-28): Spot / HOM Battle, Own the Spot and Film / Photo draw each row VisualIndicator by its
    own look (AddIndicator(self.VisualIndicators[i])): the mode's indicators.json names every look once."""

    def test_every_look_of_the_rows_once(self):
        rows = {"a": {"visual_indicators": [{"locator": "x", "matrix": None, "ribbon": "T1", "style": "S1"},
                                            {"locator": "y", "matrix": None, "ribbon": "T1", "style": "S1"}]},
                "b": {"visual_indicators": [{"locator": "z", "matrix": None, "ribbon": "T2", "style": "S1"}]}, "c": {}}
        self.assertEqual(s.mode_looks(rows), [("T1", "S1"), ("T2", "S1")])

    @unittest.skipUnless(os.environ.get("SKATE3_ONLINE_EXPORT"), "set SKATE3_ONLINE_EXPORT to an export's folder (the one holding spot_battle/rules.json)")
    def test_an_export_draws_every_look_its_rows_name(self):
        for mode in ("spot_battle", "hall_of_meat", "ots", "film", "photo"):
            root = Path(os.environ["SKATE3_ONLINE_EXPORT"]) / mode
            rows = json.loads((root / "rules.json").read_text(encoding="utf-8"))["rows"]
            ind = json.loads((root / "indicators.json").read_text(encoding="utf-8"))
            placed = [c for r in rows.values() for c in r["visual_indicators"] if c["matrix"]]
            self.assertTrue(placed, mode)
            for c in placed:
                look = ind["locators"][f"skate3:{c['ribbon']}:{c['style']}"]
                self.assertGreater(len(ind["shapes"][look["shape"]]), 1, f"{mode} {c['locator']}")
                self.assertTrue((root / "indicator" / Path(ind["styles"][look["style"]]["texture"]).name).is_file(), mode)


class ContestLooksTest(unittest.TestCase):
    """G7 (online audit 10-28, round 2): a contest round's VisualIndicators (contest_shared setup_round 552-555
    AddIndicator(vars.CurRound.VisualIndicators[i])), each in its own look: no contest round is Best_Line, so
    SetVolumeHighlight (403-407) never recolours one."""
    ROUND = "C0FFEE00C0FFEE00" + "0000000000000001" + "00" * 8   # a round's refspec (class, key, 0)

    @staticmethod
    def vi(at, kind="02CED9657D9BAF21"):
        """An 80-byte tVisualEditorData: the locator name's offset at +24, type key at +40, style key at +64."""
        return ("00" * 8 + "00000008" + "00" * 12 + "%08X" % at + "00" * 4 + "3297556BECB605E2" + kind
                + "00" * 8 + "F0DB8767D611AF94" + "D7EDBD362D7D2152" + "00" * 8)

    def test_each_round_lists_its_visual_indicators_placed_through_the_mission_its_name_names(self):
        from unittest import mock
        binary = b"\0street_dtwn_01_vi_01\0own_vi\0"
        rnd = {"class": "Hash_C0FFEE00C0FFEE00", "key": "Hash_0000000000000001", "parent": "", "fields": {
            s.F("VisualIndicators"): {"type": "Sk8::Challenge::tVisualEditorData",
                                      "array": {"items": [self.vi(1), self.vi(22)]}}}}

        class Db:
            def collections(self, rel):
                return [rnd] if rel == "challenge_local_data/street_dwtn_01" else []
        own = {s.F("ContestRounds"): {"array": {"items": [self.ROUND]}}}
        other = ({"street_dtwn_01_vi_01": {"matrix": [[2]]}}, {})
        with mock.patch.object(s, "mission_placement", return_value=other) as placed:
            rounds = s.contest_rounds(Db(), None, "challenge_local_data/street_dwtn_01", own, binary,
                                      {"own_vi": {"matrix": [[1]]}}, {})
        placed.assert_called_once_with(None, "street_dtwn_01")
        self.assertEqual(rounds[0]["visual_indicators"],
                         [{"locator": "street_dtwn_01_vi_01", "matrix": [[2]], "ribbon": "02CED9657D9BAF21", "style": "D7EDBD362D7D2152"},
                          {"locator": "own_vi", "matrix": [[1]], "ribbon": "02CED9657D9BAF21", "style": "D7EDBD362D7D2152"}])

    def test_the_contest_writes_the_looks_of_its_rounds(self):
        c = lambda t: {"locator": "x", "matrix": None, "ribbon": t, "style": "S1"}  # noqa: E731
        rows = {"street_a": {"rounds": [{"visual_indicators": [c("T1")]}, {"visual_indicators": [c("T2"), c("T1")]}, {}]}}
        self.assertEqual(s.mode_looks(rows), [("T1", "S1"), ("T2", "S1")])
        self.assertIn("contest", s.LOOK_MODES)

    @unittest.skipUnless(os.environ.get("SKATE3_ONLINE_EXPORT"), "set SKATE3_ONLINE_EXPORT to an export's folder (the one holding contest/rules.json)")
    def test_an_export_draws_every_look_its_rounds_name(self):
        root = Path(os.environ["SKATE3_ONLINE_EXPORT"]) / "contest"
        rows = json.loads((root / "rules.json").read_text(encoding="utf-8"))["rows"]
        ind = json.loads((root / "indicators.json").read_text(encoding="utf-8"))
        placed = [c for r in rows.values() if r["online"] for rd in r["rounds"] for c in rd["visual_indicators"] if c["matrix"]]
        self.assertGreaterEqual(len(placed), 135, "probe4: 135 typed, placed kind 0 items in the online rows' rounds")
        for c in placed:
            look = ind["locators"][f"skate3:{c['ribbon']}:{c['style']}"]
            self.assertGreater(len(ind["shapes"][look["shape"]]), 1, c["locator"])
            self.assertTrue((root / "indicator" / Path(ind["styles"][look["style"]]["texture"]).name).is_file())
        # The highlight colours (403-407) are Best_Line's only: no round of any row is one.
        self.assertFalse([k for k, r in rows.items() for rd in r["rounds"] if rd.get("ContestVariant") == 14])


class NisPostTest(unittest.TestCase):
    """Bug 10-09 #10: an online NIS's AUDIO_POST stream (post.big 99_<id>_*), the scene's SFX bed."""

    def take(self, cmd, ids, times=(0.0,)):
        # ICE tracks: 33 POST (CMD and ID share it), as the Skate 3 table has them.
        return {"duration": 5.0, "values": {"AUDIO_POST_CMD": cmd, "AUDIO_POST_ID": ids}, "keys": [(None, [])] * 33 + [(None, list(times))]}

    def test_a_take_plays_the_post_id_its_cmd_2_key_names(self):
        ice = {"ICE": {"AUDIO_POST_CMD": {"track": 33}, "AUDIO_POST_ID": {"track": 33}}}
        self.assertEqual(s.scene_posts(ice, [self.take([2], [9119])]), [(0.0, 9119)])
        self.assertEqual(s.scene_posts(ice, [self.take([0], [9119])]), [])
        self.assertEqual(s.scene_posts(ice, [{"duration": 5.0, "values": {}, "keys": []}]), [])

    def test_the_fold_down_is_rwaudiocores(self):
        import numpy as np
        # L C R Ls Rs LFE (measured on post.big as on the movies): centre and surrounds -3 dB into the fronts, LFE
        # dropped, the whole mix -3 dB; a 5-channel stream lacks the LFE, a 3-channel one the surrounds.
        f = s.FOLD
        six = np.array([[1000, 2000, 3000, 400, 500, 9000]], np.float32)
        self.assertEqual(s.fold_stereo(six).round().tolist(), [[round((1000 + f * 2400) * f), round((3000 + f * 2500) * f)]])
        self.assertEqual(s.fold_stereo(six[:, :5]).round().tolist(), s.fold_stereo(six).round().tolist())
        three = np.array([[1000, 2000, 3000]], np.float32)
        self.assertEqual(s.fold_stereo(three).round().tolist(), [[round((1000 + f * 2000) * f), round((3000 + f * 2000) * f)]])
        self.assertEqual(s.fold_stereo(np.array([[100, 200]], np.float32)).tolist(), [[100, 200]])

    def test_the_nis_transition_is_scenemanagerimpls(self):
        # 0x827fc200: none for kinds 1 / 0x16 / 6 (0x17 its own path, untraced); else record byte 6 picks BloomToWhite.
        self.assertIsNone(s.nis_transition(1, 0))
        self.assertIsNone(s.nis_transition(22, 1))
        self.assertIsNone(s.nis_transition(6, 1))
        self.assertIsNone(s.nis_transition(23, 0))
        self.assertEqual(s.nis_transition(3, 0), "black")
        self.assertEqual(s.nis_transition(27, 0), "black")
        self.assertEqual(s.nis_transition(5, 1), "bloom")

    @unittest.skipUnless(os.environ.get("SKATE3_ONLINE_EXPORT"), "set SKATE3_ONLINE_EXPORT to an export's folder (the one holding race/rules.json)")
    def test_an_export_gives_the_race_nis_their_post_stream(self):
        import wave
        root = Path(os.environ["SKATE3_ONLINE_EXPORT"]) / "race"
        scenes = json.loads((root / "tutorial.json").read_text(encoding="utf-8"))["scenes"]
        # Sk3_online_racestart_2's take: AUDIO_POST_CMD 2 / AUDIO_POST_ID 9119 at 0 (99_9119_online_racstrt_2, 5 s).
        audio = scenes["Sk3_online_racestart_2"]["audio"]
        self.assertEqual([a["audio"] for a in audio], ["nis/99_9119_online_racstrt_2.wav"])
        self.assertAlmostEqual(audio[0]["seconds"], 5.0, places=1)
        self.assertEqual(scenes["Sk3_online_racestart_2"]["transition"], "black")
        with wave.open(str(root / audio[0]["audio"])) as w:
            self.assertEqual((w.getnchannels(), w.getsampwidth(), w.getframerate()), (2, 2, 48000))


if __name__ == "__main__":
    unittest.main()
