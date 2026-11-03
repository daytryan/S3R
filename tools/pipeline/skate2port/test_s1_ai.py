"""old_ai1 (Skate 1 tutorial P1 task 9, research.md 3.2 / 3.3 / 3.9): Shingo's two recorded path files, his S.K.A.T.E.
and Follow profiles and the round filters, as ai.json. SKATE1_DISC (the extracted Skate 1 disc folder) runs it."""
import json, os, subprocess, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here.parents[2]), str(here)]
DISC = os.environ.get('SKATE1_DISC')


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class AiTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='s1-ai-')
        subprocess.run([sys.executable, str(here/'old_ai1.py'), str(here.parents[2]), DISC, cls.tmp.name], check=True,
                       stdout=subprocess.DEVNULL)
        cls.ai = json.loads((Path(cls.tmp.name)/'private/challenges-skate1/ai.json').read_text(encoding='utf-8'))
        from tools.skate3_ai_paths import trick_table
        cls.s3 = [n for n, _, _ in trick_table()]

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_both_files_decode_on_the_skate1_layout(self):
        # 3.3: 0x50 headers, 40-byte nodes, 56-byte ext records (the decoder checks each ext sits at ext + 56 j)
        skate, follow = self.ai['paths']['he_u1_skate01'], self.ai['paths']['he_u2_follow01']
        names = [p['name'] for p in skate]
        self.assertEqual(len(skate), 29)
        self.assertTrue({'he_u1_skate01_0', 'he_u1_skate01_13', 'he_u1_skate01_O_0', 'he_u1_skate01_xxx_0'} <= set(names))
        self.assertNotIn('he_u1_skate01_O_2', names)  # named in RoundPaths, absent from the file (RETAIL anomaly)
        self.assertEqual(len(follow), 18)
        a1 = next(p for p in follow if p['name'] == 'he_u2_followleader01_A_1')
        self.assertEqual(len(a1['pos']), 171)
        for p in skate + follow:
            n = len(p['pos'])
            self.assertEqual([len(p[k]) for k in ('step', 'ticks', 'state', 'board', 'skater', 'widths', 'flags')], [n] * 7)
            self.assertTrue(set(p['state']) <= {0, 1, 2}, p['name'])  # +38: 1 trick start, 2 trick end
        # +37 width 150 x 0.01 m -> both sides, in the driver's 0.02 m unit; the S.K.A.T.E. trick runs record 0
        self.assertEqual(follow[0]['widths'][2], [75, 75])
        self.assertEqual({w for p in skate for w, _ in p['widths']}, {0})

    def test_trick_indices_name_skate3_rows_0_to_138(self):
        # D23: the S1 table (s1.pe 0x820cf940) is Skate 3's IDENTIFIERS[0..138]
        tricks = [t for ps in self.ai['paths'].values() for p in ps for t in p['tricks']]
        self.assertTrue(tricks)
        for t in tricks:
            self.assertLess(t['trick'], 139)
            self.assertEqual(t['name'], self.s3[t['trick']])
        spins = sorted(t['yaw'] for t in tricks if t['name'] == 'ollie' and t['type'] == 1 and t['yaw'] > 180)
        self.assertEqual(spins[:2], [222, 356])  # the S.K.A.T.E. spin paths, yaw in degrees
        grind = next(t for t in tricks if t['name'] == 'fs_50_50')
        self.assertEqual((grind['type'], grind['vel']), (5, [0.0, 0.0, 0.0]))

    def test_paths_ride_0_087_m_above_the_ground(self):
        self.assertEqual(self.ai['lift'], -0.087)  # 3.3, measured on the retail Sim collision (2035 nodes)

    def test_the_profiles_are_research_3_2(self):
        skate, follow = self.ai['profiles']['shingo_skate'], self.ai['profiles']['shingo_follow']
        self.assertEqual((skate['gesture'], skate['land'], skate['abd']), (90.0, [57.0, 10.0, 0.0, 10.0], 35))
        self.assertAlmostEqual(skate['decay'], 0.8, places=6)  # f32
        self.assertEqual((follow['gesture'], follow['land'], follow['decay'], follow['abd']), (80.0, [100.0, 75.0, 0.0, 75.0], 1.0, 30))
        self.assertEqual((skate['ollie'], skate['nollie'], skate['grabs']), (9.0, 1.0, True))
        regular = {self.s3[t]: round(w, 6) for t, w in skate['regular'] if w}  # f32
        self.assertEqual(regular, {'fspopshuvit': 1.0, 'heelflip': 5.0, 'heelflip2': 1.0, 'inwardheelflip': 0.2,
                                   'kickflip': 5.0, 'kickflip2': 1.5, 'ollie': 5.0, 'popshuvit': 5.0})
        self.assertEqual({self.s3[t]: w for t, w in skate['nollie_flips'] if w}['nollie'], 2.5)
        tiers = self.ai['tiers']
        self.assertEqual([tiers[n] for n in ('ollie', 'kickflip', 'bsgrab', '360flip', 'fs_50_50', 'nosemanual')], [0, 0, 0, 2, 3, 3])

    def test_the_rounds_and_riders_come_from_the_missions(self):
        self.assertEqual(self.ai['rounds']['he_u1_skate01'][0], [f'he_u1_skate01_O_{i}' for i in range(5)])
        self.assertEqual(self.ai['rounds']['he_u1_skate01'][3], ['he_u1_skate01_12', 'he_u1_skate01_13'])
        self.assertEqual(self.ai['riders'], {'he_u1_skate01': 'shingo_skate', 'he_u2_follow01': 'shingo_follow',
                                            'fp_u3_tutorial02': 'paul_rodriguez'})  # P6 D4: P-Rod's leader


if __name__ == '__main__':
    unittest.main()
