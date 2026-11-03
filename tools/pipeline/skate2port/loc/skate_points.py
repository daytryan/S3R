"""World-space vertex positions of a converted .skate map (SKATE v12-v15), textures skipped.
Layout follows tools/vendor/university/tools/blender_owned_map/analyze_skate.py."""
import struct, sys
from pathlib import Path
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[3]/'vendor/university/tools/blender_owned_map'))
from analyze_skate import Reader, VERTEX_BYTES_V12


def positions(path):
    r = Reader(Path(path).read_bytes())
    magic = r.take(8, 'magic')
    version = int(magic[5:7])
    assert 12 <= version <= 15 and r.u32('endian') == 0x12345678
    r.string('map name')
    r.skip(49 * 4, 'metadata')
    materials, textures, vertices, *_ = struct.unpack('<9I', r.take(36, 'counts'))
    outer = r
    if version >= 15:
        size = r.u32('material block size')
        r = Reader(r.stored(size, 'material block'))
    for i in range(materials):
        r.string('name'); r.take(76, 'fields')
        if version >= 13:
            r.u32('depth')
        if r.u32('retail enabled'):
            r.u64('guid'); r.u32('handle'); r.take(4, 'group'); r.string('shader'); r.u32('family'); r.u32('flags')
            for _ in range(r.u32('bindings')):
                r.string('semantic'); r.take(16, 'binding')
            for _ in range(r.u32('params')):
                r.string('param')
                for _ in range(r.u32('values')):
                    r.string('value')
            r.string('source')
    if version >= 15:
        r = outer
    for i in range(textures):
        r.string('texture name'); r.take(12, 'dims')
        r.u32('method'); r.skip(r.u32('stored size'), 'texture payload')
    data = r.stored(vertices * VERTEX_BYTES_V12, 'vertices')
    return np.frombuffer(data, dtype=np.float32).reshape(vertices, VERTEX_BYTES_V12 // 4)[:, :3].copy()


if __name__ == '__main__':
    p = positions(sys.argv[1])
    print(len(p), p.min(0), p.max(0))
    np.save(sys.argv[2], p)
