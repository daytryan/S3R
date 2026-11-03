"""old_activities.py's conversion (activity_convert.py): a Gap and a Tag row's snippets become the {src, tree, natives}
skate3_objectives.rs loads, the 5 trigger words the 20 bytes, Skate 2's activation modes Skate 3's. No disc needed: the
snippets are the retail text of ol_fa_dte_02_gap_matrixstairs (Gap) and ol_fa_dtw_01_tag_lunchtime's shape (Tag, a per-frame
objective), as old_activities.py exported them (peer-s2-activities.md)."""
import json, sys, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here)]
import activity_convert as convert  # noqa: E402

GAP = ('do\n\tlocal vol1 = ObjectiveTrigger.EnteredVolume(1764471414687494, "DIST_Downtown|ol_fa_DTE_02_gap_matrixstairs_volume_1" );\n'
       '\tlocal vol2 = ObjectiveTrigger.EnteredVolume(1764475709654790, "DIST_Downtown|ol_fa_DTE_02_gap_matrixstairs_volume_2" );\t\n'
       '\tlocal vol3 = ObjectiveTrigger.EnteredVolume(1764480004622086, "DIST_Downtown|ol_fa_DTE_02_gap_matrixstairs_volume_3" );\n'
       '\tlocal obj1 = Tricks.DidSpecificTrickGroup("normal_grinds");\n\t\n\treturn obj1 and vol1 and vol2 and vol3;\nend;')
# A Tag objective: every frame, inside a volume while grinding (the {12} per-frame trigger pattern).
TAG = ('do\n\tlocal inside = ObjectiveTrigger.IsInVolume(1764471414687494, "DIST_Downtown|ol_fa_DTW_01_tag_lunchtime_volume_1" );\n'
       '\treturn inside and Grind.IsGrinding();\nend;')
VALID = 'do\n\treturn true;\nend'
GAP_ROW = {'key': 'ol_fa_dte_02_gap_matrixstairs_0', 'source': GAP, 'points_valid': VALID, 'activation': 4, 'triggers': [65536, 0, 0, 0, 2],
           'master': False, 'requires_highlight': False, 'display_in_hud': True, 'fail_definition': [], 'fail_events': []}
TAG_ROW = {'key': 'ol_fa_dtw_01_tag_lunchtime_0', 'source': TAG, 'points_valid': VALID, 'activation': 0, 'triggers': [0, 0, 0, 16777216, 4096],
           'master': False, 'requires_highlight': False, 'display_in_hud': False, 'fail_definition': [], 'fail_events': []}


def ids(tree):
    return sorted(convert.volume_ids(tree))


class Conversion(unittest.TestCase):
    def test_a_gap_row_converts(self):
        s = convert.Snippets()
        d = convert.objective_def(GAP_ROW, {}, s, 'gap objective 0')
        self.assertEqual(s.rejected, [])
        self.assertEqual(d['obj']['natives'], ['ObjectiveTrigger.EnteredVolume', 'Tricks.DidSpecificTrickGroup'])
        # The volume ids are the decimal strings Skate 3's rules.json carries, exact (a 50-bit id is no f32).
        self.assertEqual(ids(d['obj']['tree']), ['1764471414687494', '1764475709654790', '1764480004622086'])
        self.assertIn(['native', 'ObjectiveTrigger.EnteredVolume', [['str', '1764471414687494'], ['str', 'DIST_Downtown|ol_fa_DTE_02_gap_matrixstairs_volume_1']]],
                      d['obj']['tree'][0][1][0][2])
        self.assertEqual(d['points_valid']['tree'], [['do', [['return', [['bool', True]]]]]])
        self.assertEqual((d['key'], d['master'], d['fail_first'], d['count_kind'], d['failures']), (GAP_ROW['key'], False, False, 0, []))
        json.dumps(d)

    def test_a_tag_row_converts(self):
        s = convert.Snippets()
        d = convert.objective_def(TAG_ROW, {}, s, 'tag objective 0')
        self.assertEqual(s.rejected, [])
        self.assertEqual(d['obj']['natives'], ['Grind.IsGrinding', 'ObjectiveTrigger.IsInVolume'])
        self.assertEqual(ids(d['obj']['tree']), ['1764471414687494'])
        self.assertFalse(d['display_in_hud'])
        # Both rows share the `return true` points snippet: one src.
        self.assertEqual(convert.objective_def(GAP_ROW, {}, s, 'again')['points_valid']['src'], d['points_valid']['src'])
        self.assertEqual(s.sources[str(d['points_valid']['src'])], VALID)

    def test_the_raw_fields_skate_3_reads_pass_through(self):
        raw = {convert.FAIL_FIRST: {'data': '01000000'}, convert.COUNT_KIND: {'data': '00020000'}}
        d = convert.objective_def(GAP_ROW, raw, convert.Snippets(), 'x')
        self.assertEqual((d['fail_first'], d['count_kind']), (True, 2))

    def test_a_snippet_outside_the_subset_is_reported_not_emptied(self):
        s = convert.Snippets()
        d = convert.objective_def(dict(GAP_ROW, source='do\n\treturn @;\nend'), {}, s, 'bad objective')
        self.assertIsNone(d['obj'])
        self.assertEqual(len(s.rejected), 1)
        self.assertEqual(s.rejected[0][0], 'bad objective')
        self.assertIn('ValueError', s.rejected[0][1])

    def test_a_volume_id_that_is_not_exact_is_refused(self):
        s = convert.Snippets()
        self.assertIsNone(s.convert('do return ObjectiveTrigger.EnteredVolume(1.5, "a|b"); end', 'x'))
        self.assertIsNone(s.convert(f'do return ObjectiveTrigger.EnteredVolume({2 ** 60}, "a|b"); end', 'y'))
        self.assertEqual([w for w, _ in s.rejected], ['x', 'y'])

    def test_trigger_words_are_big_endian_bytes(self):
        b = convert.trigger_bytes([65536, 0, 0, 0, 2])
        self.assertEqual(len(b), 20)
        self.assertEqual(b[1], 1)   # byte 1: on sequence complete
        self.assertEqual(b[19], 2)
        self.assertEqual([i for i, x in enumerate(b) if x], [1, 19])
        # The other event patterns of the 216 objectives.
        self.assertEqual([i for i, x in enumerate(convert.trigger_bytes([256, 0, 0, 0, 4])) if x], [2, 19])       # volume entered
        self.assertEqual([i for i, x in enumerate(convert.trigger_bytes([0, 0, 0, 16777216, 4096])) if x], [12, 18])  # every frame
        self.assertEqual(convert.trigger_bytes([0, 16777216, 0, 0, 16])[4], 1)                                    # second pass
        self.assertEqual(convert.trigger_bytes([16777216, 0, 0, 0, 1])[0], 1)                                     # wipeout over
        for bad in ([1, 2, 3], [0, 0, 0, 0, -1], [0, 0, 0, 0, 2 ** 32]):
            with self.assertRaises(ValueError):
                convert.trigger_bytes(bad)

    def test_activation_modes_shift_past_skate_3s_new_mode_1(self):
        self.assertEqual([convert.activation(n) for n in range(5)], [0, 2, 3, 4, 5])
        self.assertEqual(convert.objective_def(GAP_ROW, {}, convert.Snippets(), 'g')['activation'], 5)   # S2 4 = S3 5, the sequence mode
        self.assertEqual(convert.objective_def(TAG_ROW, {}, convert.Snippets(), 't')['activation'], 0)
        with self.assertRaises(ValueError):
            convert.activation(5)

    def test_trigger_volumes_name_every_id_and_none_when_the_row_has_no_such_volume(self):
        s = convert.Snippets()
        d = convert.objective_def(GAP_ROW, {}, s, 'g')
        by_id = {'1764471414687494': {'name': 'v1', 'id': 1764471414687494, 'min': [0, 0, 0], 'max': [1, 1, 1]}}
        self.assertEqual(convert.trigger_volumes([d], by_id),
                         {'1764471414687494': by_id['1764471414687494'], '1764475709654790': None, '1764480004622086': None})


if __name__ == '__main__':
    unittest.main()
