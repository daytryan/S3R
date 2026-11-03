"""Skate 1's obstacle indicator (research.md 2.0 "Visual indicators", D19): every VisualIndicators row in the tutorial
window is type 4, drawn with model `oi000`, a flat downward arrow (3.71 x 4.56 m, plane z = -0.004).

Sources (data/content/world/models/):
  visual_treatments.rx2           the 0xEB0001 model descriptors in the order of the `<model>_0x...::visual_treatments`
                                  names; descriptor +0x80 = its 0xEB0000 mesh section, +0x8C its short name ('oi');
                                  meshes stride 24: pos f32x3 +0, packed normal +12, UV0 half2 +16, UV1 half2 +20
  visual_treatments_Textures.rx2  oi000's material ribbon_alpha_1: diffuse ribbonbrushstoke_16x16_d, transparent
                                  (alpha shape) ribbon_alpha3 (the material -> model order is INFERRED, research 2.0)

Writes ASSETS/private/challenges-skate1/indicator/oi000.glb (old_challenges.py, which rmtrees challenges-skate1/, runs
first): one mesh, TEXCOORD_0 = UV0, TEXCOORD_1 = UV1 (ribbon.psh clips while mTimer.x < -UV1.y / 20; all of oi000's
UV1.y are positive, so it only fades), images named by texture. The diffuse is the base colour; the alpha texture is
material extras.transparent (glTF has no separate alpha map). The packed normal is not decoded (the arrow is flat;
UNTRACED format 0x2a2190).

usage (numpy, PIL): py old_indicator1.py REPO SKATE1_ROOT ASSETS
"""
import io, re, struct, sys
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/vendor/utt'), str(repo/'tools/vendor/skate3_anim')]
from PIL import Image
import rx2_skeleton as R
from tools.asset_pipeline.character_glb import Glb
from vendor.skate3_ui.rw4 import read_textures
from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant

MODELS = s1game/'data/content/world/models'
MODEL, DESCRIPTOR = 'oi000', 0xEB0001
TEXTURES = {'diffuse': 'ribbonbrushstoke_16x16_d', 'transparent': 'ribbon_alpha3'}


def mesh(path):
    """oi000's vertices [(pos, uv0, uv1)] and triangle indices."""
    d = path.read_bytes()
    s = [(x['type_code'], x['file_offset'], x['size']) for x in R.parse_rx2(str(path), False)['sections']]
    names = [m.group(1).decode() for m in re.finditer(rb'([A-Za-z0-9]+)_0x[0-9a-f]{16}[^\0]*::visual_treatments\0', d)]
    descriptors = [at for kind, at, _ in s if kind == DESCRIPTOR]
    assert len(names) == len(descriptors), (names, len(descriptors))
    at = descriptors[names.index(MODEL)]
    short = d[at + 0x8C:d.index(b'\0', at + 0x8C)].decode()
    assert MODEL.startswith(short), (MODEL, short)
    _, base, _ = s[struct.unpack_from('>I', d, at + 0x80)[0]]
    u32 = lambda o: struct.unpack_from('>I', d, o)[0]
    ib, vb = s[u32(base + 0x28)], s[u32(base + 0x2C)]
    assert (ib[0], vb[0]) == (R.TYPE_IB_DESC, R.TYPE_VB_DESC), (hex(ib[0]), hex(vb[0]))
    vb_size, ib_size, count = u32(vb[1] + 0x20), u32(ib[1] + 0x1C), u32(ib[1] + 0x20)
    # The descriptors don't name their raw buffers; each size must be unique to be found (vtmesh.py's rule, asserted).
    (vbuf,) = [at for kind, at, size in s if kind == R.TYPE_RAW_BUFFER and size == vb_size]
    (ibuf,) = [at for kind, at, size in s if kind == R.TYPE_RAW_BUFFER and size == ib_size and at != vbuf]
    half = lambda o: list(struct.unpack_from('>2e', d, o))
    vertices = [(list(struct.unpack_from('>3f', d, o)), half(o + 16), half(o + 20))
                for o in range(vbuf, vbuf + vb_size, 24)]
    return vertices, list(struct.unpack_from('>%dH' % count, d, ibuf))


def png(texture):
    m = texture.metadata
    rgba = decode_rgba(texture.data, parse_fetch_constant(bytes.fromhex(m['fetch_constant_hex'])))
    out = io.BytesIO()
    Image.frombytes('RGBA', (m['width'], m['height']), rgba).save(out, 'PNG')
    return out.getvalue()


if __name__ == '__main__':
    vertices, indices = mesh(MODELS/'visual_treatments.rx2')
    glb = Glb()
    textures = {t.name.split('_0x')[0]: t for t in read_textures(MODELS/'visual_treatments_Textures.rx2')[1]}
    slots = {}
    for slot, name in TEXTURES.items():
        slots[slot] = glb.texture(png(textures[name]))
        glb.doc['images'][-1]['name'] = name
    glb.doc['materials'].append({'name': 'ribbon_alpha_1', 'alphaMode': 'BLEND', 'doubleSided': True,
                                 'pbrMetallicRoughness': {'baseColorTexture': slots['diffuse'], 'metallicFactor': 0.0,
                                                          'roughnessFactor': 1.0},
                                 'extras': {'transparent': slots['transparent']}})
    glb.doc['meshes'] = [{'name': MODEL, 'primitives': [{
        'attributes': {'POSITION': glb.accessor([p for p, _, _ in vertices], 'VEC3', bounds=True),
                       'TEXCOORD_0': glb.accessor([a for _, a, _ in vertices], 'VEC2'),
                       'TEXCOORD_1': glb.accessor([b for _, _, b in vertices], 'VEC2')},
        'indices': glb.accessor(indices, 'SCALAR', 5123), 'material': 0}]}]
    glb.doc['nodes'] = [{'name': MODEL, 'mesh': 0}]
    glb.doc['scenes'], glb.doc['scene'] = [{'nodes': [0]}], 0
    out = assets/'private/challenges-skate1/indicator'
    out.mkdir(parents=True, exist_ok=True)
    glb.save(out/f'{MODEL}.glb')
    lo = [min(p[i] for p, _, _ in vertices) for i in range(3)]
    hi = [max(p[i] for p, _, _ in vertices) for i in range(3)]
    print(MODEL, len(vertices), 'vertices', len(indices) // 3, 'triangles',
          'size', [round(b - a, 3) for a, b in zip(lo, hi)], '->', out/f'{MODEL}.glb')
