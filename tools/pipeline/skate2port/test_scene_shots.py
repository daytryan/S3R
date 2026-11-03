"""old_tutorial.edit_shots (round2k, nis-clock.md 2 and 5): an ICE edit's shots, each its take, subtake start and edit
window. scene_camera writes them as the camera's `shots`, where the runtime sets the actors at each cut. Needs
SKATE_TABLES (ice_params.json), as test_scene_posts."""
import unittest

from test_scene_posts import T


@unittest.skipIf(T is None, 'set SKATE_TABLES to the re-derived tables (ice_params.json)')
class EditShotsTest(unittest.TestCase):
    def test_each_subtake_key_is_a_shot_and_the_tail_holds(self):
        takes = [{'duration': 10.0}, {'duration': 4.0}]
        edit = {'duration': 12.0, 'keys': [None] * 35 + [([0, 0, 0], [0.0, 0.25, 0.5, 1.0])],
                'values': {'CONTAINS_SUBTAKE': [1, 1, 0], 'TAKE_NUMBER': [1, 0, 0], 'TAKE_START': [0.25, 0.5, 0.0]}}
        # TAKE_START is a fraction of the shot's take (nis-clock.md 2: frac = keytime + (t/subdur - TAKE_START)...).
        self.assertEqual(T['edit_shots'](edit, takes), [(1, 1.0, 0.0, 3.0), (0, 5.0, 3.0, 6.0)])

    def test_the_edit_track_is_the_tables(self):
        # Skate 1's table (ice_params1.json) puts CONTAINS_SUBTAKE on track 32 (Skate 2's: 35); track 35 is a decoy.
        takes = [{'duration': 10.0}, {'duration': 4.0}]
        edit = {'duration': 12.0, 'keys': [None] * 32 + [([0, 0, 0], [0.0, 0.25, 0.5, 1.0])] + [None] * 2
                + [([0], [0.0, 1.0])],
                'values': {'CONTAINS_SUBTAKE': [1, 1, 0], 'TAKE_NUMBER': [1, 0, 0], 'TAKE_START': [0.25, 0.5, 0.0]}}
        ice = T['ICE']
        T['ICE'] = {**ice, 'CONTAINS_SUBTAKE': {**ice['CONTAINS_SUBTAKE'], 'track': 32}}
        try:
            self.assertEqual(T['edit_shots'](edit, takes), [(1, 1.0, 0.0, 3.0), (0, 5.0, 3.0, 6.0)])
        finally:
            T['ICE'] = ice


if __name__ == '__main__':
    unittest.main()
