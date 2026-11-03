"""Skate 1's race gate models for skate1_online.rs: the RaceGate (rg000, Death Race) and SpotGate (sg000, Spot Race)
visual treatments from data/content/world/models/visual_treatments.rx2 + _Textures.rx2 (loose files on the disc).

The exe's visual treatment manager (0x826b5c08) loads ten models by name hash (table 0x8217b2c0): rg000 'RaceGate',
sg000 'SpotGate', poi00 / oi000 / ftld0 'Follow The Leader', post0-3, cb000 'Challenge Boundary' (not in the file),
and pools four RaceGates (the Death Race's next gate and three more). rgf00 is in the file, but no code names it.
A model (0xEB0001, in the file's name order) names its mesh section (+0x80) and its material (+0x84, low byte: the
order of the 0xEB0005 material list, each an alpha texture then a diffuse one). The mesh (0xEB0000, Skate 1's layout:
s1mesh) names its vertex descriptor (+0x20), index buffer descriptor (+0x28) and vertex buffer descriptor (+0x2C); each
buffer descriptor comes right after its raw buffer section. Vertices: FLOAT3 position, 11:11:10 normal, two HALF2
texcoords; a triangle list of u16 indices.
ponytail: the treatments' shader isn't traced (its second texcoord set, the manager's 'mTimer' and 'color3d' tint):
a material becomes one RGBA page, the diffuse's colour under the alpha texture's alpha, on texcoord 0."""
import json, re, struct
from pathlib import Path

MODELS = 'data/content/world/models/visual_treatments.rx2'
TEXTURES = 'data/content/world/models/visual_treatments_Textures.rx2'
WANTED = ('rg000', 'sg000')
MODEL, MESH, MATERIALS = 0xEB0001, 0xEB0000, 0xEB0005
FLOAT3, HALF2 = 0x2A23B9, 0x2C235F


def models(path, R):
    """{name: {positions, uvs, indices, material}} of every model in a visual treatments arena (R: rx2_skeleton)."""
    d = Path(path).read_bytes()
    secs = R.parse_rx2(str(path), geometry=False)['sections']
    u32 = lambda o: struct.unpack_from('>I', d, o)[0]
    names = [m.group(1).decode() for m in re.finditer(rb'([a-z0-9]{4,5})_0x0000025e03e38705', d)]
    found = [s for s in secs if s['type_code'] == MODEL]
    assert len(names) == len(found), (names, len(found))
    out = {}
    for name, s in zip(names, found):
        bone = d[s['file_offset'] + 0x8c:].split(b'\0')[0].decode()
        assert name.startswith(bone), (name, bone)  # the model's own (shortened) name
        mesh = secs[u32(s['file_offset'] + 0x80)]
        assert mesh['type_code'] == MESH, name
        m = mesh['file_offset']
        vd, ib, vb = (u32(m + k) for k in (0x20, 0x28, 0x2C))
        vdesc = R._parse_vertex_descriptor(d, secs[vd]['file_offset'], secs[vd]['size'])
        vbd, ibd = R._parse_vb_desc(d, secs[vb]['file_offset']), R._parse_ib_desc(d, secs[ib]['file_offset'])
        vraw, iraw = secs[vb - 1], secs[ib - 1]
        assert (vraw['size'], iraw['size']) == (vbd['byte_size'], ibd['byte_size']), name
        el = {(e['usage_name'], e['usage_index']): e for e in vdesc['elements']}
        assert el['POSITION', 0]['format'] == FLOAT3 and el['TEXCOORD', 0]['format'] == HALF2, (name, el)
        stride, n = vdesc['stride'], vbd['byte_size'] // vdesc['stride']
        at = lambda v, k: vraw['file_offset'] + v*stride + el[k]['offset']
        out[name] = {
            'positions': [[round(c, 4) for c in struct.unpack_from('>3f', d, at(v, ('POSITION', 0)))] for v in range(n)],
            'uvs': [[round(c, 4) for c in struct.unpack_from('>2e', d, at(v, ('TEXCOORD', 0)))] for v in range(n)],
            'indices': list(struct.unpack_from('>%dH' % ibd['index_count'], d, iraw['file_offset'])),
            'material': u32(s['file_offset'] + 0x84) & 0xFF}
    return out


def materials(path, R):
    """[(alpha texture, diffuse texture)] in material order (the 0xEB0005 list: a shader name, then its textures)."""
    d = Path(path).read_bytes()
    s = next(s for s in R.parse_rx2(str(path), geometry=False)['sections'] if s['type_code'] == MATERIALS)
    out = []
    for m in re.finditer(rb'([A-Za-z0-9_]+)_0x0000[0-9a-f]{4}03e387(17|0a)', d[s['file_offset']:s['file_offset'] + s['size']]):
        if m.group(2) == b'17':
            out.append([])
        else:
            out[-1].append(m.group(1).decode())
    assert all(len(t) == 2 for t in out), out
    return [tuple(t) for t in out]


def pages(path):
    """{texture name: RGBA PIL image} of a textures arena."""
    from PIL import Image
    from vendor.skate3_ui.rw4 import read_textures
    from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant
    out = {}
    for t in read_textures(Path(path))[1]:
        m = t.metadata
        rgba = decode_rgba(t.data, parse_fetch_constant(bytes.fromhex(m['fetch_constant_hex'])))
        out[t.name.replace('\\', '/').split('/')[-1].split('_0x')[0]] = Image.frombytes('RGBA', (m['width'], m['height']), rgba)
    return out


def page(alpha, diffuse):
    """One material's RGBA page: the diffuse's colour, its alpha times the alpha texture's (at the diffuse's size)."""
    from PIL import ImageChops
    a = alpha.resize(diffuse.size).getchannel('A')
    out = diffuse.copy()
    out.putalpha(ImageChops.multiply(diffuse.getchannel('A'), a))
    return out


def export(s1game, out, R):
    """Writes out/gates.json (the WANTED models: positions, uvs, triangle indices, bounds, their page) and the pages."""
    s1game, out = Path(s1game), Path(out)
    found, mats, tex = models(s1game/MODELS, R), materials(s1game/MODELS, R), pages(s1game/TEXTURES)
    out.mkdir(parents=True, exist_ok=True)
    gates = {}
    for name in WANTED:
        m = found[name]
        alpha, diffuse = mats[m['material']]
        png = f'{name}.png'
        page(tex[alpha], tex[diffuse]).save(out/png)
        lo = [min(p[i] for p in m['positions']) for i in range(3)]
        hi = [max(p[i] for p in m['positions']) for i in range(3)]
        gates[name] = {'positions': m['positions'], 'uvs': m['uvs'], 'indices': m['indices'], 'min': lo, 'max': hi,
                       'texture': png, 'material': [alpha, diffuse]}
    doc = {'source': f'Skate 1 {MODELS} + {TEXTURES} (old_online1_gates.py)', 'models': gates}
    (out/'gates.json').write_text(json.dumps(doc, separators=(',', ':')) + '\n', encoding='utf-8')
    return doc
