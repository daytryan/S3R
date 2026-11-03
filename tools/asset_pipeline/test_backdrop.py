"""Global resource GUID selection without owned-game fixtures."""
import struct
import unittest
from types import SimpleNamespace

from .backdrop import texture_groups
from .sky import _texture


class BackdropTests(unittest.TestCase):
    def test_missing_channels_do_not_shift_later_materials(self):
        # Ocean has no diffuse here; foliage follows it. Name text and GUIDs
        # deliberately differ, as they do in the global University resource.
        parameters = [('Name', 0), ('normal', 19), ('Name', 0),
                      ('diffuse', 0x123456789ABCDEF0), ('lightmap', 23)]
        raw = bytearray(512)
        struct.pack_into('>8I', raw, 0, 0, len(parameters), 0, 32,
                         32 + len(parameters)*32, 0, 0, 0)
        at = 256
        for i, (kind, guid) in enumerate(parameters):
            word = kind.encode() + b'\0'
            raw[at:at+len(word)] = word
            struct.pack_into('>8I', raw, 32+i*32, at, 0, 0, 0,
                             guid >> 32, guid & 0xffffffff, 0, 0)
            at += len(word)
        table = SimpleNamespace(entries=[SimpleNamespace(type_id=0xeb0005, f0=0)])
        self.assertEqual(texture_groups(bytes(raw), table, ('diffuse', 'normal', 'lightmap')),
                         [{'normal': 19}, {'diffuse': 0x123456789ABCDEF0, 'lightmap': 23}])

    def test_skate2_terrain_slope_maps_and_scale_come_from_the_material_table(self):
        # baseterrain binds diffuse2/specular2 by GUID like diffuse; diffuse2UVScale's float is the first GUID word.
        from .retail_material import TERRAIN_CHANNELS, terrain_scalar
        scale = struct.unpack('>I', struct.pack('>f', 0.03))[0] << 32
        parameters = [('Name', 0), ('diffuse', 11), ('diffuse2', 0x40003c9503e3870a),
                      ('specular2', 0x4000342f03e3870a), ('diffuse2UVScale', scale)]
        raw = bytearray(512)
        struct.pack_into('>8I', raw, 0, 0, len(parameters), 0, 32, 32 + len(parameters)*32, 0, 0, 0)
        at = 256
        for i, (kind, guid) in enumerate(parameters):
            word = kind.encode() + b'\0'
            raw[at:at+len(word)] = word
            struct.pack_into('>8I', raw, 32+i*32, at, 0, 0, 0, guid >> 32, guid & 0xffffffff, 0, 0)
            at += len(word)
        table = SimpleNamespace(entries=[SimpleNamespace(type_id=0xeb0005, f0=0)])
        group, = texture_groups(bytes(raw), table, ('diffuse', *TERRAIN_CHANNELS))
        self.assertEqual((group['diffuse2'], group['specular2']), (0x40003c9503e3870a, 0x4000342f03e3870a))
        self.assertAlmostEqual(float(terrain_scalar(group['diffuse2UVScale'])), 0.03, places=7)

    def test_texture_guid_uses_handle_not_list_position(self):
        raw = bytearray(64)
        struct.pack_into('>2I', raw, 0, 1, 8)
        struct.pack_into('>6I', raw, 8, 0, 0, 0x12345678, 0x9abcdef0, 0, 27)
        wanted = SimpleNamespace(index=27)
        textures = SimpleNamespace(data=bytes(raw),
            entries=[SimpleNamespace(type_id=0xeb000b, f0=0)],
            textures=[SimpleNamespace(index=1), wanted])
        self.assertIs(_texture(textures, 0x123456789ABCDEF0), wanted)
        with self.assertRaises(ValueError):
            _texture(textures, 123)


if __name__ == '__main__':
    unittest.main()
