"""old_screens1 + old_indicator1 (Skate 1 tutorial P1 task 10, research.md 2.0, D18 / D19 / D21): the tutorial window's
apt screens as apt_screen manifests, and the obstacle indicator oi000 as a GLB. SKATE1_DISC (the extracted Skate 1 disc
folder) runs it."""
import json, os, struct, subprocess, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
DISC = os.environ.get('SKATE1_DISC')
STEMS = ('challenge_signup', 'hud_tutorial_challenge_overlay', 'tutorial', 'trick_gestures', 'tutorial_end',
         'endchallengeoptions', 'skate101', 'incomingcalls', 'hud_action_overlay', 'actionoverlay', 'inkaction',
         'hudobjective', 'objectivehud', 'hud_skate_challenge_overlay', 'hud_followme_challenge_overlay',
         'hud_photo_challenge_overlay', 'hud_film_challenge_overlay', 'welcomeloader', 'quickloader', 'tipsloader',
         'cacinfo')


def glb(path):
    d = path.read_bytes()
    n = struct.unpack_from('<I', d, 12)[0]
    doc = json.loads(d[20:20 + n])
    return doc, d[28 + n:]


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class ScreensTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='s1-screens-')
        env = os.environ | {'SKATE_WORK': str(Path(cls.tmp.name)/'work')}
        for script in ('old_screens1.py', 'old_indicator1.py'):
            subprocess.run([sys.executable, str(here/script), str(here.parents[2]), DISC, cls.tmp.name], check=True,
                           stdout=subprocess.DEVNULL, env=env)
        cls.out = Path(cls.tmp.name)/'private/hud-skate1/tutorial'
        cls.indicator = Path(cls.tmp.name)/'private/challenges-skate1/indicator/oi000.glb'

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def manifest(self, stem):
        return json.loads((self.out/'runtime'/(stem + '.json')).read_text(encoding='utf-8'))

    def test_every_tutorial_screen_has_a_manifest_with_its_payloads(self):
        self.assertEqual(sorted(p.stem for p in (self.out/'runtime').glob('*.json')), sorted(STEMS))
        natives = json.loads((self.out/'natives.json').read_text(encoding='utf-8'))
        self.assertEqual(sorted(natives), sorted(STEMS))
        for stem in STEMS:
            with self.subTest(stem):
                m = self.manifest(stem)
                self.assertEqual(m['source']['game'], 'skate1')
                self.assertTrue(m['characters'])
                used = {ch['font']['name'] for ch in m['characters'] if ch['type_name'] == 'font'}
                self.assertEqual(set(m['fonts']), used)
                sizes = {p['texture']['rgba']: (p['texture']['width'], p['texture']['height'])
                         for prims in m['shapes'].values() for p in prims}
                sizes |= {f['texture']: (f['definition']['textures'][0]['width'], f['definition']['textures'][0]['height'])
                          for f in m['fonts'].values()}
                for rel, (w, h) in sizes.items():
                    self.assertEqual((self.out/rel).stat().st_size, w * h * 4, rel)

    def test_a_font_missing_from_the_exes_table_draws_in_bit_kitbold(self):
        # Times New Roman (menupicker's 'M' placeholder): 0x827535b8 falls back to table index 0 (research5.md B3).
        self.assertEqual(self.manifest('challenge_signup')['font_mappings']['Times New Roman']['file_name'], 'bitkitbold')

    def test_the_screens_read_skate1s_own_strings(self):
        # 2.0: the overlay's SetState 13 / 15 lines, the lesson end's options, the medical form's labels
        overlay = self.manifest('hud_tutorial_challenge_overlay')['language']
        self.assertTrue(overlay['ID_TUTORIAL_LESSON_COMPLETE'])
        self.assertTrue(overlay['ID_TUTORIAL_TUTORIAL_COMPLETE'])
        self.assertTrue(self.manifest('tutorial_end')['language']['ID_COMMON_RETRY_CAPS'])
        self.assertTrue(self.manifest('cacinfo')['language']['ID_CAC_LABEL_STANCE'])

    def test_the_ids_the_game_hands_at_run_time_are_in_the_table(self):
        # Get101Header/Get101Desc's labels for the window's four 101s and n = 15 REPEATED_TRICK (research8 1, exe
        # 0x82134e8b / 0x82135377) and the sign-up options
        language = self.manifest('skate101')['language']
        for name in ('NEW_CHALLENGE_COMPLETED', 'SEQUENCE_SCORING', 'FILM_MENU', 'TRAIN_STATION', 'REPEATED_TRICK'):
            for part in ('TITLE', 'DESC'):
                self.assertTrue(language.get(f'ID_SKATE_101_{name}_{part}'), name + part)
        signup = self.manifest('challenge_signup')['language']
        self.assertTrue(signup.get('ID_COMMON_ACCEPT_CAPS'))
        self.assertTrue(signup.get('ID_COMMON_DECLINE'))

    def test_the_distance_hud_formats_are_in_the_table_by_their_hex_label(self):
        # Mission.GetDistanceHUDText's text (0x824838f0): 0x92099686 "{0:d} m", 0x413c56da "{0:d} ft"
        language = self.manifest('hud_tutorial_challenge_overlay')['language']
        self.assertEqual((language.get('92099686'), language.get('413c56da')), ('{0:d} m', '{0:d} ft'))

    def test_the_placed_menu_pickers_keep_their_typed_construct_values(self):
        # Skate 1's construct events push numbers (0xB5 / 0xB4 / 0x5A + setVariable): without them mMenuArrowType is
        # undefined and menupicker's gotoAndStop gets NaN (Skate 101 failed to load)
        for stem in ('skate101', 'tutorial_end', 'endchallengeoptions'):
            with self.subTest(stem):
                m = self.manifest(stem)
                inits = [c['init'] for ch in m['characters'] for f in ch.get('frames', []) for c in f['controls']
                         if c.get('name') in ('mMenuPicker', 'mcMenuPicker') and 'init' in c]
                self.assertTrue(inits)
                for init in inits:
                    self.assertEqual(init['mMenuArrowType'], 'arrow1')
                    self.assertTrue(float(init['mNumVisibleItems']) > 0)
        picker = next(c['init'] for ch in self.manifest('skate101')['characters'] for f in ch.get('frames', [])
                      for c in f['controls'] if c.get('name') == 'mMenuPicker' and 'init' in c)
        self.assertEqual((picker['mNumVisibleItems'], picker['mSpeed'], picker['mMenuTransitionSpeed']), ('2', '10', '1'))
        self.assertEqual(picker['mMenuScale'], repr(struct.unpack('>f', struct.pack('>I', 1063675494))[0]))

    def test_imports_are_merged_into_the_screen(self):
        # cacinfo imports controls/button_item (its Button Item): merged, so the screen draws without the control bundle
        m = self.manifest('cacinfo')
        self.assertGreater(len(m['bundles']), 1)
        self.assertIn('Button Item', m['exports'])

    def test_oi000_is_the_flat_arrow_with_its_two_textures(self):
        # research 2.0 "Visual indicators": 3.71 x 4.56 m, 48 vertices / 48 triangles, ribbon_alpha_1's two textures
        doc, _ = glb(self.indicator)
        (prim,) = doc['meshes'][0]['primitives']
        pos = doc['accessors'][prim['attributes']['POSITION']]
        lo, hi = pos['min'], pos['max']
        self.assertAlmostEqual(hi[0] - lo[0], 3.7, delta=0.05)
        self.assertAlmostEqual(hi[1] - lo[1], 4.6, delta=0.05)
        self.assertEqual(pos['count'], 48)
        self.assertEqual(doc['accessors'][prim['indices']]['count'], 144)
        self.assertIn('TEXCOORD_1', prim['attributes'])  # the ribbon clip coordinate
        self.assertEqual(sorted(i['name'] for i in doc['images']), ['ribbon_alpha3', 'ribbonbrushstoke_16x16_d'])


if __name__ == '__main__':
    unittest.main()
