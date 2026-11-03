"""s1abin (Skate 1 tutorial P1 task 6, research.md 4.3a): Skate 1's .abin decoder (codec word 1) and the clip export the
tutorial scenes play: anim1/<clip>.bin = frames x bones x (rotation x, y, z, w, translation x, y, z, scale x, y, z)
little-endian f32, the bank's reference pose o the clip. SKATE1_DISC (the extracted Skate 1 disc folder) runs the disc tests on the two
tutorial banks in scene.big."""
import os, struct, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here.parents[2]), str(here)]
import s1abin

DISC = os.environ.get('SKATE1_DISC')
BANKS = ('Basic_Basics_Tutorial_S01_01.abin', 'NIS_ROP_2001_shingo_S01_01.abin')
DILL = 'NIS_Jason_Dill_ProTutorial_S02_01.abin'  # P6 U9: the disc's only codec-word-0 NIS bank


class WriterTest(unittest.TestCase):
    def test_a_clip_file_is_frames_by_bones_of_ten_little_endian_floats(self):
        frames = [[((1.0, 1.0, 1.0), (0.0, 0.0, 0.0, 1.0), (f, 0.0, -1.0)),
                   ((0.5, 0.75, 1.0), (0.5, 0.5, 0.5, 0.5), (0.0, 2.0 * f, 0.0))] for f in range(3)]
        with tempfile.TemporaryDirectory() as tmp:
            path = s1abin.write_clip(Path(tmp)/'anim1', 'TWO_BONES', frames)
            raw = path.read_bytes()
        self.assertEqual(path.name, 'TWO_BONES.bin')
        back = struct.unpack('<60f', raw)
        self.assertEqual(back[40:50], (0.0, 0.0, 0.0, 1.0, 2.0, 0.0, -1.0, 1.0, 1.0, 1.0))  # frame 2, bone 0
        self.assertEqual(back[50:], (0.5, 0.5, 0.5, 0.5, 0.0, 4.0, 0.0, 0.5, 0.75, 1.0))  # frame 2, bone 1

    def test_the_packed_quaternion_ends(self):
        self.assertEqual(s1abin.unpack_quat(0), (0.0, 0.0, 0.0, 1.0))  # t=0: identity
        self.assertAlmostEqual(s1abin.unpack_quat(0x7FF)[2], -1.0, delta=1e-6)  # t=1, beta=-pi/2


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class DiscTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from s2big import Big
        big = Big(Path(DISC)/'data/big/scene.big')
        cls.banks = {e.path.rsplit('/', 1)[1]: big.read(e) for e in big.entries if e.path.lower().endswith(tuple(n.lower() for n in (*BANKS, DILL)))}
        cls.banks = {next(n for n in (*BANKS, DILL) if n.lower() == k.lower()): v for k, v in cls.banks.items()}

    def test_the_tutorial_banks_decode(self):
        for bank in BANKS:
            with self.subTest(bank):
                s1abin.check(self.banks[bank])

    def test_the_basics_clip_exports_on_the_bank_pose(self):
        with tempfile.TemporaryDirectory() as tmp:
            meta = s1abin.export_clip(self.banks[BANKS[0]], 'BASIC_BASICS_TUTORIAL_A01_0', Path(tmp)/'anim1')
            raw = (Path(tmp)/meta['file']).read_bytes()
        self.assertEqual(meta['file'], 'anim1/BASIC_BASICS_TUTORIAL_A01_0.bin')
        self.assertEqual((meta['fps'], meta['frames'], len(meta['bones']), len(meta['parents'])), (30.0, 1101, 155, 155))
        self.assertEqual((meta['bones'][0], meta['parents'][1]), ('TRAJECTORY', -1))  # HIPS is a root
        # Procedural parts (record clip flag 0): HELPERBONES 70-77, SWAY_HAIR 126-146, CLOTH_HEM 147-154.
        self.assertEqual([i for i, a in enumerate(meta['animated']) if not a], [*range(70, 78), *range(126, 155)])
        values = struct.unpack(f'<{len(raw) // 4}f', raw)
        self.assertEqual(len(values), 1101 * 155 * 10)
        for a, b in zip(values[4:7], (-485.65, 194.80, -1973.15)):
            self.assertAlmostEqual(a, b, delta=0.01)
        worst = max(abs(sum(v * v for v in values[i:i + 4]) - 1.0) for i in range(0, len(values), 10))
        self.assertLess(worst, 1e-4)
        # Scale is kept (research 4.3a): unit everywhere but the lip / cheek leaves (bones 78+), 0.7 - 1.1.
        scales = [values[i + 7:i + 10] for i in range(0, 155 * 10, 10)]
        self.assertTrue(all(s == (1.0, 1.0, 1.0) for s in scales[:78]))
        self.assertTrue(any(s != (1.0, 1.0, 1.0) for s in scales[78:]))
        self.assertTrue(all(0.7 < min(s) <= max(s) < 1.1 for s in scales))


if __name__ == '__main__':
    unittest.main()


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class CodecZeroTest(unittest.TestCase):
    """P6 U9: Dill's ProTutorial bank's clip and pose carry codec word 0. RETAIL 4.3a: nothing reads blk+8, so the
    codec-1 decoder plays them; every part is the f32-rotation branch."""
    setUpClass = classmethod(DiscTest.setUpClass.__func__)

    def test_the_codec_zero_bank_decodes(self):
        d, bl, hier, clips, poses = s1abin.load(self.banks[DILL])
        self.assertEqual([b['codec'] for b in bl], [1, 0, 0])
        self.assertEqual((list(clips), list(poses)), (['NIS_JASON_DILL_PROTUTORIAL_A02_0'], ['JASON_DILL']))
        c = clips['NIS_JASON_DILL_PROTUTORIAL_A02_0']
        self.assertEqual((c['fps'], c['frames']), (30.0, 1381))
        self.assertTrue(all(p['rot_f32'] for p in c['parts'] + poses['JASON_DILL']['parts']))
        for anim in (c, poses['JASON_DILL']):
            for p in anim['parts']:
                self.assertEqual(s1abin.frame_size(p, hier['parts'][p['skel_part']]['sqt']), p['frame_size'])
            for f in range(0, anim['frames'], max(1, anim['frames'] // 40)):
                for s, q, t in s1abin.sample(d, hier, anim, f).values():
                    self.assertLess(abs(sum(x * x for x in q) - 1.0), 1e-5)

    def test_the_codec_zero_clip_exports(self):
        with tempfile.TemporaryDirectory() as tmp:
            meta = s1abin.export_clip(self.banks[DILL], 'NIS_JASON_DILL_PROTUTORIAL_A02_0', Path(tmp)/'anim1')
        self.assertEqual((meta['frames'], len(meta['bones'])), (1381, 155))
