"""old_sea.bake (New San Vanelona's sea, brief 10-29): Skate 2's ocean lights by 4.5 * L^2 * C, L the layer page's
green layer and C the chromaticity page; installed Skate 2 pages hold sqrt(3 * L^2 * C), which retail_world.wgsl
reads back as 1.5 * page^2."""
import unittest
import numpy as np
from tools.pipeline import stages
from tools.pipeline.skate2port.old_sea import bake


def page(w, h, rgba):
    return np.tile(np.array(rgba, np.uint8), (h, w, 1)).tobytes()


class Bake(unittest.TestCase):
    def test_one_page_from_the_layer_and_chromaticity(self):
        c = (98, 87, 69, 255)  # Skate 2's outer sea: chromaticity sums to 1
        out = np.frombuffer(bake(page(4, 4, (0, 190, 0, 255)), 4, 4, page(4, 4, c), 4, 4), np.uint8).reshape(4, 4, 4)
        lit = 4.5 * (190 / 255) ** 2 * np.array(c[:3]) / 255
        self.assertTrue(np.allclose(1.5 * (out[..., :3] / 255) ** 2, lit, atol=0.01))
        self.assertTrue((out[..., 3] == 255).all())

    def test_the_chromaticity_page_is_stretched_over_the_layer_page(self):
        layer = page(8, 8, (0, 255, 0, 255))
        chroma = np.zeros((2, 2, 4), np.uint8)
        chroma[:, :, 3] = 255
        chroma[:, 1, 2] = 85  # right half blue
        out = np.frombuffer(bake(layer, 8, 8, chroma.tobytes(), 2, 2), np.uint8).reshape(8, 8, 4)
        self.assertEqual(out[0, 0, 2], 0)
        self.assertGreater(out[0, 7, 2], 200)  # sqrt(3 * 85/255) = 1: the edge texel keeps the chromaticity's own value

    def test_a_stage_writes_the_backdrop(self):
        stage = next(s for s in stages.STAGES if s.name == 'old-sea')
        self.assertEqual(stage.roots, ('assets/private/native-backdrops/New_San_Vanelona.skate',))
        self.assertEqual(stage.inputs, ('s2-disc',))


if __name__ == '__main__':
    unittest.main()
