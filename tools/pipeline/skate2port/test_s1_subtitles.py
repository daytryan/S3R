"""old_challenges.subtitles1 (Skate 1 tutorial P1 task 3, research.md D6): miscboot subtitle.bin cue lists. A cue's time
is when its line appears (S1 subtitler 0x8248fcc0: cue x 100 <= elapsed ms); the line runs to the next record.
SKATE1_DISC (the extracted Skate 1 disc folder) runs the disc test."""
import struct, unittest
from pathlib import Path

from test_s1_progress import C, DISC

CLEAR = 0x001A20BA


def subtitle_bin(entries):
    """'subt', version 1, count, cue bytes, (name key, offset) x count, then each entry's 8-byte records + terminator."""
    body, index = b'', b''
    for name, cues in entries.items():
        index += struct.pack('>II', C['label_key'](name), len(body))
        for t, label in cues + [(0xFFFF, 0xBA201A00)]:
            body += struct.pack('>HHI', t, 0, label)
    return b'subt' + struct.pack('>III', 1, len(entries), len(body)) + index + body


class SubtitlesTest(unittest.TestCase):
    def test_a_line_runs_from_its_cue_to_the_next_record(self):
        data = subtitle_bin({'Basic_Test': [(0, CLEAR), (5, 0x11), (40, CLEAR), (45, 0x22), (66, CLEAR)]})
        got = C['subtitles1'](data, ['Basic_Test', 'Not_In_The_File'], {0x11: 'one', 0x22: 'two'})
        self.assertEqual(got, {'Basic_Test': [{'start': 0.5, 'end': 4.0, 'label': '00000011'},
                                              {'start': 4.5, 'end': 6.6, 'label': '00000022'}]})

    def test_a_cue_waits_for_the_one_before_it(self):
        # NIS_PRod_ProTutorial_S02_0 clears at 12.0 s after its next line's cue at 11.5 s. 0x8248fcc0 walks the cues in
        # file order, one per frame (research5.md 3): line b waits for the clear and shows from 12.0 s, not 11.5 s.
        data = subtitle_bin({'P': [(75, 0x11), (120, CLEAR), (115, 0x22), (150, CLEAR)]})
        got = C['subtitles1'](data, ['P'], {0x11: 'a', 0x22: 'b'})['P']
        self.assertEqual([(c['start'], c['end']) for c in got], [(7.5, 12.0), (12.0, 15.0)])

    def test_a_label_without_english_text_fails(self):
        with self.assertRaises(AssertionError):
            C['subtitles1'](subtitle_bin({'P': [(5, 0x33), (9, CLEAR)]}), ['P'], {})


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class DiscTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        boot = C['Big'](Path(DISC)/'data/big/miscboot.big')
        data = boot.read(next(e for e in boot.entries if e.path.endswith('/subtitle.bin')))
        eng, text = C['Big'](Path(DISC)/'data/big/english.big'), {}
        for e in eng.entries:
            if 'histogram' not in e.path.lower():
                text.update(C['table'](eng.read(e)))
        cls.text = text
        cls.cues = C['subtitles1'](data, ['Basic_Basics_Tutorial', 'NIS_ROP_2001_shingo_S01_0',
                                          'NIS_ROP_2002_shingo_S01_0', 'Game_Intro_1'], text)

    def test_the_metric_nollie_objective_has_english_text(self):
        # P5: the nollie objective's metric twin of _FEET ("{0:d} m" vs "{0:d} ft" units).
        self.assertIn('ID_PRO_TUTORIAL_NOLLIE_OBJ1', C['SCREEN_STRINGS'])
        self.assertIn(C['label_key']('ID_PRO_TUTORIAL_NOLLIE_OBJ1'), self.text)

    def test_the_mission_labels_and_unit_formats_have_english_text(self):
        # P5 L10: the objective HUD's labels, and the follow distance formats (no ID name, keyed by label hash).
        want = ['ID_OBJECTIVE_TITLE_GET_POINTS', 'ID_OBJECTIVE_BEATPOINTS'] + [
            f'ID_OBJECTIVE_TITLE_TRICK_{t}{s}' for t in ['FLIP', 'GAP', 'GRAB', 'GRIND', 'MANUAL', 'SPIN', 'REVERT']
            for s in ['', 'S']] + ['ID_CHALLENGE_FILM_HE_U1_FILM01_OBJ', 'ID_PRO_TUTORIAL_NOLLIE_OBJ1_FEET',
                                   'ID_OBJECTIVE_NOSKATEZONE', 'ID_OBJECTIVE_GETLINE', 'ID_COMMON_NUMBER_OVER_NUMBER']
        self.assertEqual(len(want), 21)
        for k in want:
            self.assertIn(k, C['SCREEN_STRINGS'])
            self.assertIn(C['label_key'](k), self.text, k)
        self.assertEqual([self.text[int(k, 16)] for k in C['UNIT_FORMATS']], ['{0:d} m', '{0:d} ft'])

    def test_the_basics_lesson_has_six_lines_from_half_a_second(self):
        basics = self.cues['Basic_Basics_Tutorial']
        self.assertEqual(len(basics), 6)
        self.assertEqual(basics[0]['start'], 0.5)
        self.assertTrue(self.text[int(basics[0]['label'], 16)].startswith('Dude, the community center'))

    def test_the_rop_scenes_clear_at_their_retail_times(self):
        self.assertEqual(self.cues['NIS_ROP_2001_shingo_S01_0'][-1]['end'], 14.0)
        self.assertEqual(self.cues['NIS_ROP_2002_shingo_S01_0'][-1]['end'], 13.0)

    def test_the_cameraman_speaks_199_seconds_into_the_first_intro(self):
        line = next(c for c in self.cues['Game_Intro_1'] if self.text[int(c['label'], 16)].startswith('Yo what'))
        self.assertEqual(line['start'], 199.0)


if __name__ == '__main__':
    unittest.main()
