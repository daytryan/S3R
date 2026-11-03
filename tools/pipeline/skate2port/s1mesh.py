"""Skate 1 RW4 meshes for the Skate 3 rx2_skeleton parser.

Skate 1 models have no 0x00EB0023 mesh descriptor; an older 0x00EB0000 carries the same
data at other offsets: +0x00/+0x10 bbox min/max, +0x20 vertex-descriptor section index,
+0x28 index-buffer descriptor, +0x2C vertex-buffer descriptor, +0x34 offset of the draw
call {prim, 0, 0, index count}, +0x38 the descriptor of a small buffer holding the bone
palette (u16 file-bone indices; shoes: LeftFoot, LeftToeBase, RightFoot, RightToeBase) and
+0x40 its length. install() wraps parse_rx2 so every Skate 3 tool reads Skate 1 files."""
import struct

TYPE_MESH_DESC_V1 = 0x00EB0000


def _mesh(R, d, sections, base, bone_count):
    u32 = lambda o: struct.unpack_from('>I', d, o)[0]
    pick = lambda i, t: sections[i] if i < len(sections) and sections[i]['type_code'] == t else None
    palette_desc, count = u32(base + 0x38), u32(base + 0x40)
    palette_raw = pick(palette_desc - 1, R.TYPE_RAW_BUFFER) if pick(palette_desc, R.TYPE_IB_DESC) else None
    palette = list(struct.unpack_from('>%dH' % count, d, palette_raw['file_offset'])) if palette_raw else []
    if len(palette) != count or any(i >= bone_count for i in palette):
        raise ValueError('Skate 1 mesh: unreadable bone palette')
    mesh = {'bbox_min': R._f32v(d, base, 3), 'bbox_max': R._f32v(d, base + 0x10, 3),
            'used_bone_count': count, 'used_bone_indices': palette}
    vdesc_s, ibd_s, vbd_s = pick(u32(base + 0x20), R.TYPE_VTX_DESC), pick(u32(base + 0x28), R.TYPE_IB_DESC), pick(u32(base + 0x2C), R.TYPE_VB_DESC)
    if not (vdesc_s and ibd_s and vbd_s):
        return mesh
    vdesc = R._parse_vertex_descriptor(d, vdesc_s['file_offset'], vdesc_s['size'])
    vbd, ibd = R._parse_vb_desc(d, vbd_s['file_offset']), R._parse_ib_desc(d, ibd_s['file_offset'])
    raws = [s for s in sections if s['type_code'] == R.TYPE_RAW_BUFFER]
    vb_s = next(s for s in raws if s['size'] == vbd['byte_size'])
    ib_s = next(s for s in raws if s['size'] == ibd['byte_size'] and s is not vb_s and s is not palette_raw)
    count = vbd['byte_size'] // vdesc['stride']
    mesh.update(R._decode_vertices(d, vb_s['file_offset'], count, vdesc))
    mesh.update(vertex_count=count, stride=vdesc['stride'], vertex_elements=vdesc['elements'])
    n = ibd['index_count']
    mesh.update(indices=list(struct.unpack_from('>%dH' % n, d, ib_s['file_offset'])), index_count=n, triangle_count=n // 3)
    if 'normals' not in mesh:
        R._derive_normals(mesh)
    if 'skin_indices' in mesh:
        # A slot past the palette is bone 0 (`count` is the vertex count by here; one of the 10-15 cast's new models carries such slots).
        mesh['skin_bones'] = [tuple(palette[i] if i < len(palette) else 0 for i in q) for q in mesh['skin_indices']]
    return mesh


def install(R, character=None):
    """Patch rx2_skeleton.parse_rx2 and, given retail_character, decode_dense_morphs (idempotent).
    Skate 1 had no body-shape sliders, so its models carry no morph targets."""
    if character is not None and not getattr(character.decode_dense_morphs, 'skate1', False):
        dense = character.decode_dense_morphs

        def decode_dense_morphs(path, parsed, vertex_count, rx2_skeleton):
            types = {s['type_code'] for s in parsed['sections']}
            if R.TYPE_MESH_DESC not in types and TYPE_MESH_DESC_V1 in types:
                return []
            return dense(path, parsed, vertex_count, rx2_skeleton)
        decode_dense_morphs.skate1 = True
        character.decode_dense_morphs = decode_dense_morphs
    if getattr(R.parse_rx2, 'skate1', False):
        return
    original = R.parse_rx2

    def parse_rx2(path, geometry=True):
        result = original(path, geometry)
        if result['meshes']:
            return result
        with open(path, 'rb') as fh:
            d = fh.read()
        for s in result['sections']:
            if s['type_code'] == TYPE_MESH_DESC_V1:
                mesh = _mesh(R, d, result['sections'], s['file_offset'], len(result['bones']))
                result['meshes'].append(mesh if geometry else {k: mesh[k] for k in ('bbox_min', 'bbox_max', 'used_bone_count', 'used_bone_indices')})
        return result
    parse_rx2.skate1 = True
    R.parse_rx2 = parse_rx2
