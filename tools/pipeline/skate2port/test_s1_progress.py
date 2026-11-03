"""old_challenges.progress (Skate 1 tutorial P1 task 1, research.md 1.1): miscgame.big progress.sp records, the career
chain the runtime walks: prerequisites, unlocks, slot actions in order, the points gate. SKATE1_DISC (the extracted
Skate 1 disc folder) runs the disc test."""
import os, struct, sys, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
repo = here.parents[2]
DISC = os.environ.get('SKATE1_DISC')
MAIN = '# ---------------------------------------------------------------- main'


def lib(script='old_challenges.py', game=None):
    """A script's library half, as old_online1 execs it."""
    ns = {'__name__': script[:-3] + '_lib', '__file__': str(here/script)}
    argv, sys.argv = sys.argv, [script, str(repo), str(game or here/'no-game'), str(here/'no-assets')]
    try:
        exec(compile((here/script).read_text(encoding='utf-8').split(MAIN)[0], script, 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


C = lib(game=DISC)


def record(fields, words=()):
    r = bytearray(0x400)
    for at, text in fields.items():
        r[at:at + len(text)] = text.encode('latin-1')
    for at, value in words:
        struct.pack_into('>I', r, at, value)
    r[0x3F0:0x400] = b'\xff' * 16
    return bytes(r)


class ProgressTest(unittest.TestCase):
    def test_a_record_reads_its_chain_unlocks_actions_and_points(self):
        rec = record({0: 'he_u1_skate01', 0x20: 'he_u1_film01', 0xE0: 'u=c:IP_shingo;u=g:x;', 0x2E0: 'r=2',
                      0x300: 'u=unlock', 0x340: 'm=314'}, [(0x3C8, 5), (0x3D0, 5), (0x3D8, 0x96)])
        data = struct.pack('>II', 1, 0x400) + rec
        got, = C['progress'](data)
        self.assertEqual(got['name'], 'he_u1_skate01')
        self.assertEqual(got['after'], ['he_u1_film01'])
        self.assertEqual(got['unlocks'], ['u=c:IP_shingo', 'u=g:x'])
        self.assertEqual(got['pre_actions'], [])
        # A sparse slot list keeps its order and drops nothing (the executor walks the slots in order).
        self.assertEqual(got['actions'], ['r=2', 'u=unlock', 'm=314'])
        self.assertEqual((got['points_required'], got['points_awarded']), (5, 5))
        self.assertEqual(got['words'], {'0x3c8': 5, '0x3d0': 5, '0x3d8': 0x96})
        self.assertIsNone(got['class'])


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class DiscTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        g = C['Big'](Path(DISC)/'data/big/miscgame.big')
        cls.recs = C['progress'](g.read(next(e for e in g.entries if e.path.endswith('/progress.sp'))))

    def test_the_window_runs_in_the_retail_order(self):
        r = self.recs
        self.assertEqual([x['name'] for x in r[:5]],
                         ['he_u1_film01', 'he_u1_skate01', 'he_u2_follow01', 'he_u2_photo01', 'he_u3_film01'])
        self.assertEqual(r[0]['after'], ['fp_u2_tutorial05'])
        self.assertEqual(r[0]['unlocks'], ['u=g:r_progressionstart'])
        self.assertEqual(r[0]['actions'], ['u=unlock', 's=101:NewChallenge'])
        self.assertEqual(r[1]['unlocks'], ['u=c:IP_shingo'])
        self.assertEqual(r[1]['actions'], ['r=2', 'u=unlock', 'm=314'])
        self.assertEqual(r[2]['actions'], ['u=unlock', 's=101:SequenceScoring'])
        self.assertEqual(r[3]['actions'], ['u=unlock', 's=101:FilmMenu'])
        self.assertEqual(r[4]['pre_actions'], ['m=2905'])
        self.assertEqual(r[4]['actions'], ['u=unlock', 'c=shingo01', 's=101:TrainStation'])
        self.assertEqual([x['points_required'] for x in r[1:5]], [0, 5, 10, 15])
        self.assertEqual([x['points_awarded'] for x in r[1:5]], [5, 5, 5, 5])

    def test_the_lessons_and_pro_tutorials(self):
        r = self.recs
        self.assertEqual([x['name'] for x in r[100:104]],
                         ['fp_u2_tutorial01', 'fp_u2_tutorial02', 'fp_u2_tutorial04', 'fp_u2_tutorial05'])
        self.assertEqual(r[101]['after'], ['fp_u2_tutorial01'])
        pros = {x['name']: x for x in r[104:109]}
        self.assertEqual(pros['fp_u3_tutorial02']['after'], ['he_u1_film01'])
        self.assertEqual(pros['fp_u3_tutorial01']['after'], ['he_u1_skate01'])
        self.assertEqual(pros['fp_u3_tutorial05']['after'], ['he_u2_photo01'])
        self.assertEqual(pros['fp_u3_tutorial04']['unlocks'], ['u=c:Chris_haslam'])


if __name__ == '__main__':
    unittest.main()
