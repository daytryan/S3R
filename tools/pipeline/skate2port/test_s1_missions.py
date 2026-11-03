"""old_challenges challenge rows, sign-ups, spawns and volumes (Skate 1 tutorial P1 task 2, research.md 1.1 "Type 3",
2.5-2.9, 4.1). SKATE1_DISC (the extracted Skate 1 disc folder) runs the disc test."""
import struct, unittest
from pathlib import Path

from test_s1_progress import C, DISC

MISSIONS = ['he_u1_film01', 'he_u1_skate01', 'he_u2_follow01', 'he_u2_photo01', 'he_u3_film01']


class SpawnRowTest(unittest.TestCase):
    def test_a_spawn_row_is_entity_class_key_and_place(self):
        pool = {0x40: 'he_u2_follow01_seb_locator', 0x60: 'he_u2_photo01_challengelocator_01', 8: None}
        raw = struct.pack('>QQIIIIII', 0x60D703C77CAB1631, 0x7E5E359D6CE96AEC, 0, 0, 0x40, 2, 0x60, 0)
        self.assertEqual(C['spawn_row'](raw, pool.get), {
            'class': '60D703C77CAB1631', 'key': '7E5E359D6CE96AEC', 'locator': 'he_u2_follow01_seb_locator',
            'mode': 2, 'second': 'he_u2_photo01_challengelocator_01'})
        raw = struct.pack('>QQIIIIII', 0x60D703C77CAB1631, 0x0D0BDC7925D47161, 0, 0, 0x40, 0, 8, 0)
        self.assertIsNone(C['spawn_row'](raw, pool.get)['second'])


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class DiscTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        v = C['Vault']()
        C['_luac'].clear()
        got = {name: C['challenge'](v, name, lua=False)[0] for name in MISSIONS}
        cls.rows = {name: c['self'] for name, c in got.items()}
        cls.steps = {name: c['steps'] for name, c in got.items()}
        cls.vols = C['volumes'](MISSIONS)

    def test_a_handler_without_a_chunk_is_a_no_op(self):
        # Three stateenter rows (class 038FA43A) carry a tLuaScript pool offset, not a bytecode blob; all three
        # point at the same empty `do end` text.
        self.assertIsNone(self.steps['he_u1_skate01']['new_state']['stateenter'])
        self.assertIsNone(self.steps['he_u2_follow01']['leaving']['stateenter'])
        self.assertIsNone(self.steps['he_u2_follow01']['leader_warning']['stateenter'])

    def test_the_sign_up_boxes_and_global_types(self):
        me = self.rows
        self.assertEqual([me[n]['SignupRadius'] for n in MISSIONS[1:4]], [10.0, 5.0, 5.0])
        self.assertEqual([me[n]['GlobalType'] for n in MISSIONS], [4, 3, 3, 3, 0])

    def test_who_stands_at_each_sign_up(self):
        shingo, = self.rows['he_u1_skate01']['GlobalSpawn']
        self.assertEqual((shingo['key'], shingo['locator'], shingo['mode']),
                         ('0D0BDC7925D47161', 'he_u1_skate01_challengelocator', 0))
        follow = {s['key']: s for s in self.rows['he_u2_follow01']['GlobalSpawn']}
        self.assertEqual(follow['7E5E359D6CE96AEC']['mode'], 1)
        seb, = self.rows['he_u2_photo01']['GlobalSpawn']
        self.assertEqual((seb['locator'], seb['mode'], seb['second']),
                         ('he_u2_follow01_seb_locator', 2, 'he_u2_photo01_challengelocator_01'))
        self.assertTrue(self.rows['he_u1_skate01']['ClearOtherNPCs'])
        self.assertFalse(self.rows['he_u2_follow01']['ClearOtherNPCs'])

    def test_follow_shingo_ends_in_its_destination_box(self):
        box = self.vols['he_u2_follow01_destination']
        for got, want in zip(box['min'] + box['max'], [-78, 168, -1654, -61, 172, -1637]):
            self.assertAlmostEqual(got, want, delta=0.5)


if __name__ == '__main__':
    unittest.main()
