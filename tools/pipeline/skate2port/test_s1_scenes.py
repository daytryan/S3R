"""old_scenes1 (Skate 1 tutorial P1 task 7, research.md 4.3 / 4.4, nis-clock.md): the tutorial's cutscenes as
challenge::cutscene plays them. SKATE1_DISC (the extracted Skate 1 disc folder), SKATE_TABLES (ice_params1.json) and
SKATE_VGMSTREAM run it."""
import bisect, json, math, os, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
repo = here.parents[2]
DISC = os.environ.get('SKATE1_DISC')
MAIN = '# ---------------------------------------------------------------- main'


def at(samples, t):
    ts = [s[0] for s in samples]
    i = max(bisect.bisect_right(ts, t), 1) - 1
    a = samples[i]
    b = samples[i + 1] if i + 1 < len(samples) and samples[i + 1][0] > a[0] else None
    if b is None:
        return a
    f = min(max((t - a[0]) / (b[0] - a[0]), 0), 1)
    return [x + (y - x) * f for x, y in zip(a, b)]


def real(cam, dt=1 / 60):
    """Wall seconds to the camera's end on the round-2k clock (nis-clock.md 10): scene time advances by dt x
    clamp(TIME_SCALE (sample column 9), 0.01, 1); a freeze key holds FREEZE_TIME / 30 s."""
    end, t, hold, keys, wall = cam['samples'][-1][0], 0., 0., 0, 0.
    freeze = cam.get('freeze', [])
    while t < end and wall < 600:
        wall += dt
        if hold > 0:
            hold -= dt
            if hold > 0:
                continue
        t += dt * min(max(at(cam['samples'], t)[9], 0.01), 1.)
        while keys < len(freeze) and freeze[keys][0] <= t:
            _, c, f = freeze[keys]
            keys += 1
            hold = f / 30 if c in (1, 3, 4, 5) else float('inf') if c == 2 else hold
    return wall


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class ScenesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='s1-scenes-')
        ns = {'__name__': 'old_scenes1_lib', '__file__': str(here/'old_scenes1.py')}
        argv, sys.argv = sys.argv, ['old_scenes1.py', str(repo), DISC, cls.tmp.name]
        try:
            exec(compile((here/'old_scenes1.py').read_text(encoding='utf-8').split(MAIN)[0], 'old_scenes1.py', 'exec'), ns)
        finally:
            sys.argv = argv
        cls.root = Path(cls.tmp.name)/'private/challenges-skate1/nis'
        cls.scenes = ns['tutorial_scenes'](ns['SCENES'])

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def camera(self, name):
        return json.loads((self.root/self.scenes[name]['camera']).read_text(encoding='utf-8'))

    def test_the_lesson_edits_play_their_shots_on_the_scaled_clock(self):
        # research.md 4.4 / nis-clock.md: Skate 1's edit track is 32; TIME_SCALE slows the scene, not the game.
        for name, shots, seconds in (('Basic_Basics_Tutorial', 4, 37.14), ('Basic_Air_Tutorial', 15, 36.92),
                                     ('Basic_Grinds_Tutorial', 9, 27.19), ('Basic_Timing_Tutorial', 10, 38.23)):
            with self.subTest(name):
                cam = self.camera(name)
                self.assertEqual(len(cam['shots']), shots)
                self.assertAlmostEqual(real(cam), seconds, delta=0.1)

    def test_each_scene_plays_its_post_stream(self):
        # research.md 4.4: the ICE AUDIO_POST_ID of each scene (RETAIL).
        for name, stream in (('Basic_Basics_Tutorial', 5007), ('Basic_Timing_Tutorial', 5009),
                             ('Basic_Air_Tutorial', 5006), ('Basic_Grinds_Tutorial', 5008),
                             ('NIS_ROP_2001_shingo_S01_0', 5107), ('NIS_ROP_2002_shingo_S01_0', 5108),
                             ('NIS_IP_Shingo_SkateDecline_S01_0', 5300), ('he_u1_skate01', 5128)):
            with self.subTest(name):
                audio = self.scenes[name]['audio']
                self.assertEqual(len(audio), 1, audio)
                self.assertTrue(audio[0]['audio'].startswith(f'nis/99_{stream}_'), audio)
                self.assertTrue((self.root/audio[0]['audio']).is_file())
        self.assertEqual(self.scenes['he_u2_photo01']['audio'], [])

    def test_the_procedural_shake_is_played_not_flagged(self):
        # SHAKE_TYPE 0 (Default) keys with an amplitude: Basics' take 1, all of Grinds' (research5.md 2 traces it).
        self.assertNotIn('shake', self.scenes['Basic_Grinds_Tutorial'])

    def test_a_single_take_scene_has_no_shots(self):
        self.assertNotIn('shots', self.camera('NIS_ROP_2001_shingo_S01_0'))

    def test_the_tracks_carry_their_decoded_clips(self):
        track, = self.scenes['Basic_Basics_Tutorial']['tracks']
        self.assertEqual((track['character'], track['clip1']['frames']), ('tutorial_skater', 1101))
        self.assertTrue((self.root/track['clip1']['file']).is_file())
        self.assertEqual([t['character'] for t in self.scenes['NIS_ROP_2001_shingo_S01_0']['tracks']],
                         ['current_player', 'IP_shingo'])
        self.assertEqual(self.scenes['he_u1_skate01']['tracks'], [])

    def test_the_spoken_scenes_carry_their_subtitles(self):
        cue = self.scenes['Basic_Basics_Tutorial']['subtitles'][0]
        self.assertEqual(cue['start'], 0.5)
        self.assertEqual(self.scenes['NIS_ROP_2001_shingo_S01_0']['subtitles'][-1]['end'], 14.0)



class ShakeTest(unittest.TestCase):
    """s1ice's camera shake (research5.md 2): no disc needed."""
    sys.path.insert(0, str(here))

    def shake(self, kind, fov, amplitude=0.5, frequency=1., u=0.3, rows=None):
        import s1ice
        take = {'duration': 10., 'SHAKE_AMPLITUDE': amplitude, 'SHAKE_TYPE': kind, 'SHAKE_FREQUENCY': frequency}
        T = {'ice_value': lambda take, n, u, left: take[n], 'ice_shakes': lambda: [None, (len(rows or [0]), rows)]}
        return s1ice.ice_shake(T, take, u, False, fov)

    def test_the_noise_is_hugo_elias_value_noise(self):
        import s1ice
        # Reference values from an int32 numpy evaluation of h(n) (300000 << 13 wraps).
        for n, h in ((0, -0.2817909838631749), (1, -0.2263730512931943), (2, 0.29363288078457117),
                     (300000, -0.6446772636845708)):
            self.assertAlmostEqual(s1ice.noise(n), h, places=12)
        self.assertAlmostEqual(s1ice.vnoise(1.25), -0.2263730512931943 * 0.75 + 0.29363288078457117 * 0.25, places=12)
        self.assertAlmostEqual(s1ice.fbm(0.), -0.2817909838631749 * 1.96875, places=12)  # six octaves, 0.5 apart

    def test_a_recorded_shake_reads_one_frame_unscaled_by_the_lens(self):
        rows = [(0.1, 0.2, 0.3, 1., 2., 3.), (0.01, 0.02, 0.03, 4., 5., 6.)]
        # at = 0.3 x 10 s x 0.05 x 30 = 4.5 frames: frame 4 % 2 = 0, no blend toward frame 1.
        for fov in (math.pi / 6, math.pi / 2):
            turn, move = self.shake(1, fov, frequency=0.05, rows=rows)
            self.assertEqual(list(move), [-3. * 0.5, -1. * 0.5, 2. * 0.5])
            z, x, y = 0.3 * 0.5, 0.1 * 0.5, -0.2 * 0.5  # quaternion (QZ, QX, -QY) x A
            w = math.sqrt(1 - math.sqrt(z * z + x * x + y * y))
            self.assertAlmostEqual(turn[0][0], 1 - 2 * (x * x + y * y), places=12)
            self.assertAlmostEqual(turn[0][1], 2 * (z * x + y * w), places=12)

    def test_the_procedural_shake_turns_about_up_and_left_and_moves_left_and_up(self):
        import s1ice
        t = 0.3 * 10. * 1.
        for fov in (math.pi / 3, math.pi / 2):
            turn, move = self.shake(0, fov)
            k = math.sin(fov / 2) / math.sin(math.pi / 6)
            rz = math.asin(math.sin(math.radians(6 * 0.5 * s1ice.fbm(1.618 * t))) * k)
            ry = math.asin(math.sin(math.radians(4 * 0.5 * s1ice.fbm(1.382 * t))) * k)
            Rz = [[math.cos(rz), math.sin(rz), 0], [-math.sin(rz), math.cos(rz), 0], [0, 0, 1]]
            Ry = [[math.cos(ry), 0, -math.sin(ry)], [0, 1, 0], [math.sin(ry), 0, math.cos(ry)]]
            want = [[sum(Ry[i][j] * Rz[j][c] for j in range(3)) for c in range(3)] for i in range(3)]
            for i in range(3):
                for c in range(3):
                    self.assertAlmostEqual(turn[i][c], want[i][c], places=12)
            self.assertEqual(list(move), [0., 0.3 * 0.5 * s1ice.fbm(0.618 * t), 0.2 * 0.5 * s1ice.fbm(0.382 * t)])

    def test_no_amplitude_no_shake(self):
        self.assertIsNone(self.shake(0, math.pi / 3, amplitude=0.))


if __name__ == '__main__':
    unittest.main()
