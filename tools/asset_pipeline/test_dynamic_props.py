import json
import struct
import unittest
from unittest.mock import patch
from pathlib import Path
from tempfile import TemporaryDirectory
import numpy as np

from .dynamic_props import locators, transform_mesh, template_meshes, save_catalog, load_catalog, template_collision, template_grabs, write_package


def resource(kind, payload):
    raw = bytearray(88+len(payload))
    raw[:7] = b'\x89RW4xb2'
    struct.pack_into('>I', raw, 32, 1)
    struct.pack_into('>I', raw, 48, 64)
    struct.pack_into('>6I', raw, 64, 88, 0, len(payload), 0, 0, kind)
    raw[88:] = payload
    return raw


def resources(*records):
    """An RX2 of (kind, payload) records, in order."""
    table = 64
    raw = bytearray(table + 24*len(records))
    raw[:7] = b'\x89RW4xb2'
    struct.pack_into('>I', raw, 32, len(records))
    struct.pack_into('>I', raw, 48, table)
    for i, (kind, payload) in enumerate(records):
        struct.pack_into('>6I', raw, table + 24*i, len(raw), 0, len(payload), 0, 0, kind)
        raw += payload
    return bytes(raw)


def volume(matrix, kind, *rest):
    return struct.pack('>16f', *np.asarray(matrix, dtype=float).ravel()) + struct.pack('>I', kind) + rest[0].ljust(28, bytes(1))


def instance_table(*rows):
    """An EB000D tInstance table of 160 B rows (template, model, collision): template id +104, its EB0001 model
    record index +128, its EB000A +132."""
    out = bytearray(32 + 160*len(rows))
    struct.pack_into('>5I', out, 0, 0, len(rows), 1, 32, 32 + 160*len(rows))
    for k, (template, model, collision) in enumerate(rows):
        out[32+160*k+104:32+160*k+112] = bytes.fromhex(template)
        struct.pack_into('>2I', out, 32+160*k+128, model, collision)
    return bytes(out)


def instance_row(template, collision):
    return instance_table((template, 0, collision))


class DynamicPropsTests(unittest.TestCase):
    def test_template_collision_follows_its_instance_row_to_the_body(self):
        # Skate 2's wedge chain (round2 T4): the EB000D row (+132) -> EB000A -> body -> Volume -> aggregate of a box.
        # No EB0004 part table: the oil drum has none. The volume moves 5 m along X; the box is 2 x 4 x 6 m.
        moved = np.eye(4); moved[3, :3] = (5, 0, 0)
        box = volume(np.eye(4), 4, struct.pack('>3f', 1, 2, 3))
        # The list's count is at +40 (+36 holds another number: 2 or 5 on the disc, 7 here).
        aggregate = bytes(36) + struct.pack('>2I', 7, 1) + bytes(4) + struct.pack('>I', 0x40) + bytes(12) + box
        raw = resources((0xEB000D, instance_row('00016EBB03E38707', 1)), (0xEB000A, struct.pack('>3I', 0, 0xFFFFFFFF, 2)),
                        (0x40006, bytes(0x70) + struct.pack('>I', 3) + bytes(12)),
                        (0x80001, volume(moved, 6, struct.pack('>I', 4))), (0x80002, aggregate))
        tris = template_collision(raw, lambda mesh: self.fail('no clustered mesh here'))['00016EBB03E38707']
        self.assertEqual(tris.shape, (12, 3, 3))
        np.testing.assert_allclose(tris.reshape(-1, 3).min(0), (4, -2, -3))
        np.testing.assert_allclose(tris.reshape(-1, 3).max(0), (6, 2, 3))
        # Outward faces: each normal points away from the box centre.
        for t in tris:
            n = np.cross(t[1] - t[0], t[2] - t[0])
            self.assertGreater(np.dot(n, t.mean(0) - (5, 0, 0)), 0)

    def test_a_capsule_runs_along_its_local_z(self):
        # Skate 2's picnic table legs: type 2, half height +0x44, radius +0x50 (the RW Volume's radius field).
        moved = np.eye(4); moved[3, :3] = (1, 2, 3)
        raw = resources((0xEB000D, instance_row('00018AD603E38707', 1)), (0xEB000A, struct.pack('>3I', 0, 0xFFFFFFFF, 2)),
                        (0x40006, bytes(0x70) + struct.pack('>I', 3) + bytes(12)),
                        (0x80001, volume(moved, 2, struct.pack('>4f', 0.5, 0, 0, 0.1))))
        tris = template_collision(raw, lambda mesh: None)['00018AD603E38707']
        np.testing.assert_allclose(tris.reshape(-1, 3).min(0), (0.9, 1.9, 2.4), atol=1e-6)
        np.testing.assert_allclose(tris.reshape(-1, 3).max(0), (1.1, 2.1, 3.6), atol=1e-6)
        for t in tris:
            n = np.cross(t[1] - t[0], t[2] - t[0])
            self.assertGreater(np.linalg.norm(n), 0)
            self.assertGreater(np.dot(n, t.mean(0) - (1, 2, 3)), 0)

    def test_a_cylinder_runs_along_its_local_z_grown_by_its_fatness(self):
        # Skate 2's oil drum rim: type 5, half height +0x44, radius +0x48, fatness +0x50; local Z turned to world Y.
        upright = np.array([[1, 0, 0, 0], [0, 0, -1, 0], [0, 1, 0, 0], [0, 0.5, 0, 1]], dtype=float)
        raw = resources((0xEB000D, instance_row('00018ADB03E38707', 1)), (0xEB000A, struct.pack('>3I', 0, 0xFFFFFFFF, 2)),
                        (0x40006, bytes(0x70) + struct.pack('>I', 3) + bytes(12)),
                        (0x80001, volume(upright, 5, struct.pack('>4f', 0.4, 0.3, 0, 0.05))))
        tris = template_collision(raw, lambda mesh: None)['00018ADB03E38707']
        np.testing.assert_allclose(tris.reshape(-1, 3).min(0), (-0.35, 0.05, -0.35), atol=1e-6)
        np.testing.assert_allclose(tris.reshape(-1, 3).max(0), (0.35, 0.95, 0.35), atol=1e-6)
        for t in tris:
            n = np.cross(t[1] - t[0], t[2] - t[0])
            self.assertGreater(np.dot(n, t.mean(0) - (0, 0.5, 0)), 0)

    def test_a_template_is_grabbable_when_grab_data_sits_between_its_model_and_collision(self):
        # Skate 2 cPres_Global: the wedge's EB001F (157) lies between its model (154) and EB000A (158); the wood
        # panel's run (596..599) has none.
        raw = resources((0xEB000D, instance_table(('00016EBB03E38707', 1, 3), ('000196B203E38707', 4, 6))),
                        (0xEB0001, bytes(16)), (0xEB001F, bytes(16)), (0xEB000A, bytes(12)),
                        (0xEB0001, bytes(16)), (0xEB001B, bytes(16)), (0xEB000A, bytes(12)))
        self.assertEqual(template_grabs(raw), {'00016EBB03E38707'})

    def test_package_report_carries_each_instance_annotation(self):
        with TemporaryDirectory() as work:
            work = Path(work)
            np.savez(work/'model.npz', vertices_0=np.zeros((3, 3)), faces_0=np.array([[0, 1, 2]]))
            template = dict(matrix=np.eye(4), model_matrix=np.eye(4), npz=work/'model.npz', asset_id='model',
                            meshes=[dict(index=0, retail_texture_ids={})])
            moved = np.eye(4); moved[3, :3] = (1, 2, 3)
            item = dict(instance_id='00000000000000AA', template_id='T', matrix=moved.tolist(), name='WedgeRamp')
            with patch('tools.asset_pipeline.dynamic_props.write', lambda manifest, out, *a, **k: out.write_bytes(b'x')):
                placed = write_package('Map', 'District', [item], {'T': template}, {}, work/'p.skate',
                                       annotate=lambda item, transform: dict(origin=transform[3, :3].tolist()))
            report = json.loads((work/'p.json').read_text())
            self.assertTrue((work/'p.skate').is_file())
        self.assertEqual(placed, (1, 0))
        self.assertEqual(report['instances'][0]['origin'], [1, 2, 3])
        self.assertEqual(report['instances'][0]['name'], 'WedgeRamp')

    def test_template_collision_rejects_an_unknown_volume(self):
        raw = resources((0xEB000D, instance_row('00016EBB03E38707', 1)), (0xEB000A, struct.pack('>3I', 0, 0xFFFFFFFF, 2)),
                        (0x40006, bytes(0x70) + struct.pack('>I', 3) + bytes(12)),
                        (0x80001, volume(np.eye(4), 1, struct.pack('>f', 0.5))))
        with self.assertRaises(ValueError):
            template_collision(raw, lambda mesh: None)

    def test_transform_does_not_read_other_meshes(self):
        class LazyArrays(dict):
            def __getitem__(self, key):
                if key.endswith('_1'):
                    raise AssertionError('Unrelated mesh was decompressed')
                return super().__getitem__(key)
        arrays = LazyArrays(vertices_0=np.zeros((3, 3)), faces_0=np.array([[0, 1, 2]]), vertices_1=None)
        result = transform_mesh(arrays, 0, np.eye(4))
        np.testing.assert_array_equal(result['faces_0'], [[0, 1, 2]])

    def test_catalog_roundtrip_preserves_double_precision_and_bindings(self):
        template = dict(matrix=np.arange(16, dtype=float).reshape(4, 4)/7,
                        model_matrix=np.eye(4), npz=Path('model.npz'),
                        meshes=[dict(retail_texture_ids={'diffuse': '0xf123456789abcdef'})],
                        asset_id='original', mesh_info=[112])
        textures={'0xf123456789abcdef': dict(width=4, height=4, rgba='original.rgba')}
        with TemporaryDirectory() as work:
            path=Path(work)/'catalog.json'
            with patch('tools.asset_pipeline.dynamic_props.catalog', return_value=({'template':template}, textures)):
                save_catalog([], path)
            templates, actual_textures=load_catalog(path)
        actual=templates['template']
        for key in ('matrix','model_matrix'):
            self.assertEqual(actual[key].tobytes(),template[key].tobytes())
        self.assertEqual(actual['meshes'],template['meshes'])
        self.assertEqual(actual_textures,textures)

    def test_locator_id_and_row_matrix(self):
        payload = bytearray(166)
        struct.pack_into('>5I', payload, 0, 0, 1, 1, 32, 160)
        m = np.eye(4); m[3, :3] = [10, 20, -30]
        struct.pack_into('>16f', payload, 32, *m.ravel())
        struct.pack_into('>3Q2I', payload, 128, 123, 456, 789, 0, 160)
        payload[160:] = b'bench\0'
        rows = locators(resource(0xEB001D, payload))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]['template_id'], '0000000000000315')
        self.assertEqual(rows[0]['matrix'][3], [10, 20, -30, 1])
        self.assertEqual(rows[0]['name'], 'bench')
        struct.pack_into('>I', payload, 16, 159)
        with self.assertRaisesRegex(ValueError, 'layout'):
            locators(resource(0xEB001D, payload))

    def test_scaled_rotation_and_normal_transform(self):
        m = np.array([[0, 0, -2, 0], [0, 3, 0, 0], [4, 0, 0, 0], [10, 20, 30, 1]], dtype=float)
        arrays = {'vertices_0': np.array([[1, 2, 3]]), 'normals_0': np.array([[0, 0, 1]]), 'faces_0': np.array([[0, 0, 0]])}
        transformed = transform_mesh(arrays, 0, m)
        np.testing.assert_allclose(transformed['vertices_0'], [[22, 26, 28]])
        np.testing.assert_allclose(transformed['normals_0'], [[1, 0, 0]])
        np.testing.assert_array_equal(arrays['vertices_0'], [[1, 2, 3]])

    def test_missing_template_reference_is_rejected(self):
        payload = bytearray(192)
        struct.pack_into('>5I', payload, 0, 0, 1, 1, 32, 192)
        struct.pack_into('>I', payload, 160, 100)
        with self.assertRaisesRegex(ValueError, 'model reference'):
            template_meshes(resource(0xEB000D, payload))


if __name__ == '__main__':
    unittest.main()
