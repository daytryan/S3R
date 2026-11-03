"""old_challenges_cast (Skate 1 tutorial P1 task 8, research.md 4.2 / 4.3a / 4.5): the tutorial's cast as GLBs on the
skater rig keeping Skate 1's face joints, the pedestrian clips Shingo and Seb play, and cast.json (their vault rows).
SKATE1_DISC (the extracted Skate 1 disc folder) and SKATE_ASSETS (an installation's assets folder, read for
private/stock) run it."""
import io, json, os, struct, subprocess, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here.parents[2]), str(here)]
DISC, SOURCE = os.environ.get('SKATE1_DISC'), os.environ.get('SKATE_ASSETS')
# The scene actors (tutorial_skater, IP_shingo), Seb's model and the four pro tutorials' pros (research 4.5).
CAST = ['tutorial_skater', 'IP_shingo', 'Deran_Chung', 'Chris_haslam', 'jason_dill', 'mike_carroll', 'Paul_rodriguez']
SHINGO, SEB = '0D0BDC7925D47161', '7E5E359D6CE96AEC'  # class 60D703C77CAB1631 rows (research 4.2)
IDLE = 'PRO_WNDR_STND_IDLE_%d_CYC'


def gltf(path):
    data = path.read_bytes()
    n = struct.unpack_from('<I', data, 12)[0]
    return json.loads(data[20:20 + n]), data[28 + n:]


@unittest.skipUnless(DISC and SOURCE, 'set SKATE1_DISC and SKATE_ASSETS (an installation assets folder)')
class CastTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='s1-cast-')
        subprocess.run([sys.executable, str(here/'old_challenges_cast.py'), str(here.parents[2]), DISC, cls.tmp.name,
                        SOURCE], check=True, stdout=subprocess.DEVNULL)
        cls.root = Path(cls.tmp.name)/'private/challenges-skate1'
        cls.cast = json.loads((cls.root/'cast.json').read_text(encoding='utf-8'))
        import s1abin
        from s2big import Big
        big = Big(Path(DISC)/'data/big/scene.big')
        bank = next(e for e in big.entries if e.path.endswith('Basic_Basics_Tutorial_S01_01.abin'))
        cls.hier = s1abin.load(big.read(bank))[2]

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_every_model_keeps_the_skate1_face_on_its_hierarchy_parents(self):
        # research 4.3a: the face is hierarchy bones 78-125; keep all 48 with their parents (CHIN under JAW), unwelded.
        names, parents = self.hier['names'], self.hier['parents']
        for key in CAST:
            with self.subTest(key):
                self.assertEqual(self.cast['models'][key], f'cast/{key}.glb')
                doc, _ = gltf(self.root/'cast'/f'{key}.glb')
                nodes = [n.get('name') for n in doc['nodes']]
                parent = {c: nodes[i] for i, n in enumerate(doc['nodes']) for c in n.get('children', [])}
                for i in range(78, 126):
                    self.assertIn(names[i], nodes)
                    self.assertEqual(parent.get(nodes.index(names[i])), names[parents[i]], names[i])

    def test_hair_without_a_diffuse_cuts_by_its_transparency_map(self):
        # Shingo's hair material has detail/specular/transparency only; the transparency map's alpha cuts the cards.
        from PIL import Image
        doc, blob = gltf(self.root/'cast/IP_shingo.glb')
        hair, = [m for m in doc['materials'] if m['name'] == 'Retail_Hair']
        self.assertEqual(hair['alphaMode'], 'MASK')
        view = doc['bufferViews'][doc['images'][doc['textures'][hair['pbrMetallicRoughness']['baseColorTexture']['index']]['source']]['bufferView']]
        image = Image.open(io.BytesIO(blob[view.get('byteOffset', 0):view.get('byteOffset', 0) + view['byteLength']]))
        low, high = image.convert('RGBA').getchannel('A').getextrema()
        self.assertEqual((low, high), (0, 255))

    def test_a_two_material_part_draws_both(self):
        # Shingo's Leg part holds two meshes (trousers, arms) with a material each.
        doc, _ = gltf(self.root/'cast/IP_shingo.glb')
        self.assertEqual(sum(m['name'].startswith('Retail_Leg') for m in doc['materials']), 2)

    def test_the_idles_are_the_pedestrian_clips(self):
        # research 4.2 table: 15 fps, flags bit 28 (looping) on the _CYC idles; the collisions are 30 fps one-shots.
        for n, frames in ((0, 116), (3, 135), (4, 130), (6, 177)):
            with self.subTest(n):
                meta = self.cast['clips'][IDLE % n]
                self.assertEqual((meta['fps'], meta['frames'], meta['looping']), (15.0, frames, True))
                self.assertTrue((self.root/meta['file']).is_file())
        meta = self.cast['clips']['PRO_HRL_STND_F_0_N']
        self.assertEqual((meta['fps'], meta['frames'], meta['looping']), (30.0, 46, False))
        self.assertEqual(len([c for c in self.cast['clips'] if c.startswith('PHOT_')]), 17)

    def test_the_entities_keep_their_motion_graph(self):
        # P1 review: the remap's non-clip text fields (the motion graph each NPC runs, research 4.2) were dropped.
        texts = lambda key: [t for v in self.cast['entities'][key]['texts'].values() for t in v]
        self.assertTrue(any(t.endswith('motiongraph_marquee.xml') for t in texts(SHINGO)), texts(SHINGO))
        self.assertTrue(any(t.endswith('MotionGraph_Photographer.xml') for t in texts(SEB)), texts(SEB))

    def test_seb_is_the_living_world_photographer_mesh(self):
        # LOD0 0x00002d2203e38744: 7116 vertices, standing 0 to 1.73 m (research5.md B2).
        doc, _ = gltf(self.root/'cast/Deran_Chung.glb')
        verts = sum(doc['accessors'][p['attributes']['POSITION']]['count'] for m in doc['meshes'] for p in m['primitives'])
        self.assertEqual(verts, 7116)

    def test_the_entities_carry_their_vault_rows(self):
        shingo, seb = self.cast['entities'][SHINGO], self.cast['entities'][SEB]
        self.assertEqual((shingo['name'], shingo['model'], shingo['category'], shingo['draw']),
                         ('IP Shingo', 'IP_shingo', 1, 'IP_shingo'))
        self.assertNotIn('stand_in', shingo)
        # The missions spawn row 7E5E... (GlobalSpawn); its model row names Deran_Chung, a livingworld.big recipe
        # (category 0) whose one Rostral part, seb_photographer, is the whole body (research5.md B2).
        self.assertEqual((seb['name'], seb['model'], seb['category'], seb['draw']), ('Photographer', 'Deran_Chung', 0, 'Deran_Chung'))
        self.assertNotIn('stand_in', seb)
        # The remap rows (class 53E332F67B8124B2): one IdleBasicCyc entry is picked at random per start (0x8281ca60).
        self.assertEqual(shingo['remap']['IdleBasicCyc'], [IDLE % n for n in (0, 3, 4, 6)])
        self.assertEqual(seb['remap']['IdleBasicCyc'], [IDLE % n for n in (0, 3, 4)])
        self.assertEqual(seb['remap']['CollisionBackStanding'], ['PRO_HRL_STND_B_0_N'])
        # The row's own fields: the class default row's pedestrian nodes (NPC_*, SG_*) are not in these motion graphs.
        self.assertFalse([c for v in seb['remap'].values() for c in v if c.startswith(('NPC_', 'SG_'))])
        for entity in (shingo, seb):
            for clips in entity['remap'].values():
                self.assertTrue(set(clips) <= set(self.cast['clips']), clips)


if __name__ == '__main__':
    unittest.main()
