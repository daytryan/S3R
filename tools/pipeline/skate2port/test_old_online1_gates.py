"""old_online1_gates (online audit 10-12): Skate 1's RaceGate / SpotGate models. SKATE1_DISC (the extracted Skate 1 disc folder,
with data/content/world/models) runs the disc test."""
import json, os, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
repo = here.parents[2]
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/vendor/skate3_anim'), str(here)]
import old_online1_gates

DISC = os.environ.get('SKATE1_DISC')


class PageTest(unittest.TestCase):
    def test_the_page_is_the_diffuse_under_the_alpha_textures_alpha(self):
        from PIL import Image
        alpha = Image.new('RGBA', (2, 2), (0, 0, 0, 128))
        diffuse = Image.new('RGBA', (4, 4), (10, 20, 30, 255))
        p = old_online1_gates.page(alpha, diffuse)
        self.assertEqual((p.size, p.getpixel((3, 3))), ((4, 4), (10, 20, 30, 128)))


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class DiscTest(unittest.TestCase):
    def test_the_race_and_spot_gates_come_out_with_their_material(self):
        import rx2_skeleton as R
        disc = Path(DISC)
        found = old_online1_gates.models(disc/old_online1_gates.MODELS, R)
        self.assertEqual(sorted(found), sorted(['ftld0', 'rgf00', 'post2', 'post3', 'sg000', 'oi000', 'poi00', 'rg000', 'post0', 'post1']))
        mats = old_online1_gates.materials(disc/old_online1_gates.MODELS, R)
        self.assertEqual(mats[found['rg000']['material']], ('ribbonbrushstoke_128x512', 'ribbonbrushstoke_128x512_d'))
        self.assertEqual(mats[found['sg000']['material']], ('ribbon_alpha3', 'ribbonbrushstoke_16x16_d'))
        with tempfile.TemporaryDirectory() as out:
            doc = old_online1_gates.export(disc, out, R)
            self.assertEqual(json.loads((Path(out)/'gates.json').read_text()), json.loads(json.dumps(doc)))
            rg, sg = doc['models']['rg000'], doc['models']['sg000']
            # The race gate: one upright 14 x 15 m quad across x; the spot gate: four 5.3 m walls round a 14.7 m square.
            self.assertEqual((len(rg['positions']), len(rg['indices'])), (4, 6))
            self.assertEqual((rg['min'], rg['max']), ([-6.999, 0.0, 0.0], [7.0, 14.943, 0.0]))
            self.assertEqual((len(sg['positions']), len(sg['indices'])), (16, 24))
            self.assertEqual((sg['min'][0], sg['max'][2], round(sg['max'][1], 2)), (-7.336, 7.336, 5.31))
            from PIL import Image
            alpha = Image.open(Path(out)/rg['texture']).getchannel('A').getextrema()
            self.assertEqual(alpha, (0, 255), 'the brush strokes')


if __name__ == '__main__':
    unittest.main()
