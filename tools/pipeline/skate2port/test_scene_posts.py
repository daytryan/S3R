"""old_tutorial.post_ids / post_wavs (bug 10-09 #10): an online NIS's AUDIO_POST stream, the scene's SFX bed, for the
Skate 2 and Skate 1 online scenes (Skate 2's post.big, Skate 1's nis.big `99_<id>_*`). SKATE2_DISC / SKATE1_DISC (an
extracted disc folder with data/audio) and SKATE_VGMSTREAM run the decode test."""
import os, sys, tempfile, unittest, wave
from pathlib import Path

here = Path(__file__).resolve().parent
repo = here.parents[2]


def lib():
    """old_tutorial's library half, as the online scripts exec it (all of it before main; the ICE table is
    SKATE_TABLES/ice_params.json, re-derived from the player's disc)."""
    ns = {'__name__': 'old_tutorial_lib', '__file__': str(here/'old_tutorial.py')}
    argv, sys.argv = sys.argv, ['old_tutorial.py', str(repo), str(here/'no-game'), str(here/'no-assets')]
    try:
        src = (here/'old_tutorial.py').read_text(encoding='utf-8')
        exec(compile(src.split('# ---------------------------------------------------------------- main')[0], 'old_tutorial.py', 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


TABLES = Path(os.environ.get('SKATE_TABLES') or here)
T = lib() if (TABLES/'ice_params.json').is_file() else None


@unittest.skipIf(T is None, 'set SKATE_TABLES to the re-derived tables (ice_params.json)')
class PostIdsTest(unittest.TestCase):
    def take(self, cmd, ids, times=(0.0,)):
        track = T['ICE']['AUDIO_POST_CMD']['track']
        return {'duration': 5.0, 'values': {'AUDIO_POST_CMD': cmd, 'AUDIO_POST_ID': ids},
                'keys': [(None, [])] * track + [(None, list(times))]}

    def test_the_cmd_2_keys_name_the_stream(self):
        self.assertEqual(T['post_ids']([self.take([2], [5200])]), [5200])
        self.assertEqual(T['post_ids']([self.take([0], [5200])]), [])
        self.assertEqual(T['post_ids']([{'duration': 1.0, 'values': {}, 'keys': []}]), [])

    def test_a_late_post_is_left_out(self):
        # Every online take starts its post at 0 (Skate 2 and Skate 1 probed); a later one would need a delay field.
        self.assertEqual(T['post_ids']([self.take([2], [5200], times=(0.5,))]), [])

    def test_takes_sharing_a_stream_play_it_once(self):
        # Skate 1's lesson edits: copies of a take (Basics' 0 and 1) and the first shot's take both key 99_<id>; the
        # stream is the whole edit's bed (5007's 41.4 s against the edit's 37.1 s), so the scene plays it once.
        self.assertEqual(T['post_ids']([self.take([2], [5007]), self.take([2], [5007]), self.take([2], [5300])]),
                         [5007, 5300])

    def test_the_fold_down_is_rwaudiocores(self):
        import numpy as np
        f = 0.70710678
        six = np.array([[1000, 2000, 3000, 400, 500, 9000]], np.float32)
        self.assertEqual(T['fold_stereo'](six).round().tolist(), [[round((1000 + f * 2400) * f), round((3000 + f * 2500) * f)]])
        self.assertEqual(T['fold_stereo'](np.array([[1, 2]], np.float32)).tolist(), [[1, 2]])

    def test_post_audio_fills_each_scenes_audio_from_its_posts(self):
        wavs = {5200: {'audio': 'nis/99_5200_x.wav', 'seconds': 3.0}}
        scenes = {'a': {'audio': [], 'posts': [5200, 7]}, 'b': {'audio': []}}
        old, T['post_wavs'] = T['post_wavs'], lambda bank, ids, out: {i: w for i, w in wavs.items() if i in ids}
        try:
            T['post_audio'](scenes, 'bank', 'out')
        finally:
            T['post_wavs'] = old
        self.assertEqual(scenes, {'a': {'audio': [wavs[5200]]}, 'b': {'audio': []}})

    @unittest.skipUnless(os.environ.get('SKATE2_DISC') and os.environ.get('SKATE_VGMSTREAM'), 'set SKATE2_DISC and SKATE_VGMSTREAM')
    def test_skate2_post_big_decodes_to_stereo(self):
        with tempfile.TemporaryDirectory() as out:
            got = T['post_wavs'](Path(os.environ['SKATE2_DISC'])/'data/audio/post.big', {5200}, Path(out))
            self.assertEqual(list(got), [5200])
            with wave.open(str(Path(out)/got[5200]['audio'])) as w:
                self.assertEqual((w.getnchannels(), w.getsampwidth()), (2, 2))
            self.assertGreater(got[5200]['seconds'], 1)

    @unittest.skipUnless(os.environ.get('SKATE1_DISC') and os.environ.get('SKATE_VGMSTREAM'), 'set SKATE1_DISC and SKATE_VGMSTREAM')
    def test_skate1_keeps_its_posts_in_nis_big(self):
        with tempfile.TemporaryDirectory() as out:
            got = T['post_wavs'](Path(os.environ['SKATE1_DISC'])/'data/audio/english/nis.big', {5213}, Path(out))
            self.assertEqual(list(got), [5213])
            self.assertTrue((Path(out)/got[5213]['audio']).is_file())


if __name__ == '__main__':
    unittest.main()
