"""old_room (Skate 2's Edit Skater room, brief 10-31): each material reads one layer of its layer page, picked by the
u16 at +6 of its lightmap record, and the room is moved to where the PS3 mod put it."""
import struct
import unittest
from types import SimpleNamespace

import numpy as np
from tools.pipeline.skate2port.old_room import bake, layers, move


def page(w, h, rgba):
    return np.tile(np.array(rgba, np.uint8), (h, w, 1)).tobytes()


def material_section(groups):
    """One 0x00eb0005 section: 32-byte parameter records after an 8I header, names after the records."""
    names, records = b'', []
    count = sum(len(g) for g in groups)
    for g in groups:
        for kind, word6, guid in g:
            at = 32 + count * 32 + len(names)
            names += kind + b'\0'
            records.append(struct.pack('>IHH8xQ8x', at, 0, word6, guid))
    head = struct.pack('>8I', 0, count, 0, 32, 32 + count * 32, 0, 0, 0)
    return head + b''.join(records) + names


class Layers(unittest.TestCase):
    def test_the_lightmap_record_picks_the_layer_per_chromaticity_page(self):
        raw = material_section([
            [(b'Name', 0xFFFF, 0), (b'lightmap', 2, 0x11), (b'chromaticity', 0xFFFF, 0xC4)],
            [(b'Name', 0xFFFF, 0), (b'lightmap', 1, 0x11), (b'chromaticity', 0xFFFF, 0xC5)],
            [(b'Name', 0xFFFF, 0), (b'diffuse', 0xFFFF, 0x22)]])
        table = SimpleNamespace(entries=[SimpleNamespace(type_id=0x00eb0005, f0=0)])
        self.assertEqual(layers(raw, table), {0xC4: 2, 0xC5: 1})

    def test_one_chromaticity_page_read_with_two_layers_is_refused(self):
        raw = material_section([
            [(b'Name', 0xFFFF, 0), (b'lightmap', 0, 0x11), (b'chromaticity', 0xFFFF, 0xC4)],
            [(b'Name', 0xFFFF, 0), (b'lightmap', 1, 0x11), (b'chromaticity', 0xFFFF, 0xC4)]])
        with self.assertRaises(ValueError):
            layers(raw, SimpleNamespace(entries=[SimpleNamespace(type_id=0x00eb0005, f0=0)]))


class Bake(unittest.TestCase):
    def test_each_layer_reads_its_own_channel(self):
        c = (85, 85, 85, 255)  # 3 * C = 1
        for layer, expect in ((0, 40), (1, 120), (2, 200), (3, round(.33 * 40 + .34 * 120 + .33 * 200))):
            out = np.frombuffer(bake(page(2, 2, (40, 120, 200, 7)), 2, 2, page(2, 2, c), 2, 2, layer), np.uint8)
            self.assertTrue((abs(out.reshape(-1, 4)[:, :3].astype(int) - expect) <= 1).all(), layer)
            self.assertTrue((out.reshape(-1, 4)[:, 3] == 255).all())

    def test_the_runtime_reads_back_the_retail_light(self):
        c = (98, 87, 69, 255)
        out = np.frombuffer(bake(page(4, 4, (190, 0, 0, 255)), 4, 4, page(4, 4, c), 4, 4, 0), np.uint8).reshape(4, 4, 4)
        lit = 4.5 * (190 / 255) ** 2 * np.array(c[:3]) / 255
        self.assertTrue(np.allclose(1.5 * (out[..., :3] / 255) ** 2, lit, atol=0.01))


class Move(unittest.TestCase):
    def test_the_room_turns_about_y_then_moves(self):
        p = move(np.array([[1., 0, 0], [0, 1, 0]]))
        c, s = np.cos(0.7870465), np.sin(0.7870465)
        self.assertTrue(np.allclose(p[0], (c - 1.41272, 200.00017, -s - 6.7415), atol=1e-5))
        self.assertTrue(np.allclose(p[1], (-1.41272, 201.00017, -6.7415), atol=1e-5))
        self.assertTrue(np.allclose(move(np.array([[0., 0, 1]]), (0, 0, 0)), [[s, 0, c]], atol=1e-6))


if __name__ == '__main__':
    unittest.main()
