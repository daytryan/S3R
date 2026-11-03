"""fit_corrections.npz (fit_clothing.py) is whole: every model's changes are there and fit each other, so the wardrobe
stages apply the 10-04 fit to all 365 models instead of failing on (or silently skipping) a damaged record; and apply()
fits the model the changes were made for, once, and leaves any other model as it is."""
import json, sys, tempfile, types, unittest
from unittest import mock
from pathlib import Path
import numpy as np

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here), str(here.parents[1]/'mixamo_to_skate')]
import fit_clothing


class Corrections(unittest.TestCase):
    def test_every_model_is_whole(self):
        data = self.enterContext(np.load(fit_clothing.DATA))
        mids = fit_clothing.models(data)
        self.assertEqual(len(mids), 365)
        for mid in mids:
            with self.subTest(mid=mid):
                self.assertEqual(data[f'{mid}/check'].shape, (32,))
                moved, delta = data[f'{mid}/moved'], data[f'{mid}/delta']
                self.assertEqual(delta.shape, (len(moved), 3))
                self.assertEqual(len(np.unique(moved)), len(moved))
                self.assertEqual(f'{mid}/joints' in data, f'{mid}/weights' in data)
                for t in fit_clothing.TARGETS:
                    if f'{mid}/target{t}/delta' in data:
                        self.assertEqual(data[f'{mid}/target{t}/delta'].shape, data[f'{mid}/target{t}/ndelta'].shape)
                self.assertTrue(json.loads(str(data[f'{mid}/meta'])))


class Apply(unittest.TestCase):
    def glb(self, path, positions):
        doc = types.SimpleNamespace(doc={'asset': {'version': '2.0'}, 'meshes': [{'primitives': [{'attributes': {}}]}],
                                         'nodes': [], 'accessors': [], 'bufferViews': []})
        data, attributes = bytearray(), doc.doc['meshes'][0]['primitives'][0]['attributes']
        attributes['POSITION'] = fit_clothing.append_accessor(doc, data, positions, 'VEC3', 5126, 34962, True)
        attributes['JOINTS_0'] = fit_clothing.append_accessor(doc, data, np.zeros((len(positions), 4), '<u2'), 'VEC4', 5123, 34962)
        attributes['WEIGHTS_0'] = fit_clothing.append_accessor(doc, data, np.eye(4, dtype='<f4')[[0] * len(positions)], 'VEC4', 5126, 34962)
        fit_clothing.save_glb(doc, data, path)

    def test_the_model_it_was_made_for_is_fitted_once_and_others_are_left(self):
        root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        made_for = np.arange(12, dtype='<f4').reshape(4, 3)
        self.glb(root/'m.glb', made_for)
        self.glb(root/'other.glb', made_for + 1)
        np.savez(root/'fit.npz', **{f'{m}/{k}': v for m in ('m', 'other') for k, v in
                                    {'check': fit_clothing.check(made_for), 'moved': np.array([1], np.uint32),
                                     'delta': np.array([[0, 0.25, 0]]), 'meta': np.array(json.dumps({'round': 2}))}.items()})
        library = {'models': {'m': {'scene': 'm.glb'}, 'other': {'scene': 'other.glb'}}}
        untouched = (root/'other.glb').read_bytes()
        with mock.patch.object(fit_clothing, 'DATA', root/'fit.npz'):
            accessory = {'models': {**library['models'], 'm': {'scene': 'm.glb', 'slot': 'Accessory'}}}
            self.assertEqual(fit_clothing.apply(root, accessory), set())  # never the Accessory slot
            self.assertFalse(fit_clothing.fitted_already(root/'m.glb'))
            self.assertEqual(fit_clothing.apply(root, library), {'m'})
            fitted, meta = fit_clothing.geometry(root/'m.glb')
            self.assertEqual(fitted['POSITION'][1].tolist(), [3, 4.25, 5])
            self.assertEqual(meta, {'round': 2})
            once = (root/'m.glb').read_bytes()
            self.assertEqual(fit_clothing.apply(root, library), {'m'})  # already fitted: still in the set, not refitted
        self.assertEqual((root/'m.glb').read_bytes(), once)
        self.assertEqual((root/'other.glb').read_bytes(), untouched)

    def test_pants_keep_their_own_waist_under_tops(self):
        root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        made_for = np.array([[0, 0.5, 0], [0, 0.935, 0], [0, 1.0, 0]], '<f4')
        np.savez(root/'fit.npz', **{'p/check': fit_clothing.check(made_for), 'p/moved': np.arange(3, dtype=np.uint32),
                                    'p/delta': np.full((3, 3), 0.02), 'p/meta': np.array(json.dumps({'round': 2}))})
        with mock.patch.object(fit_clothing, 'DATA', root/'fit.npz'):
            for slot, scale in (('Pants', [1, 0.5, 0]), ('OuterTorso', [1, 1, 1])):
                with self.subTest(slot=slot):
                    self.glb(root/'p.glb', made_for)
                    self.assertEqual(fit_clothing.apply(root, {'models': {'p': {'scene': 'p.glb', 'slot': slot}}}), {'p'})
                    moved = fit_clothing.geometry(root/'p.glb')[0]['POSITION'] - made_for
                    np.testing.assert_allclose(moved[:, 0], np.array(scale) * 0.02, atol=1e-6)


if __name__ == '__main__':
    unittest.main()
