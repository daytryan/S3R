"""Skate 2's Edit Skater room (brief 10-31) from Skate 2's own disc: the global model DIST_petes in miscboot.big, which
the PS3 "Skate 2 Background" mod moved whole into place as DIST_CreateSakter. Written as the render-only backdrop
CreateSkater_Skate2 where the mod put it, its layer pages baked with their chromaticity pages into the page the runtime
reads. Normal maps need no conversion: the environment shaders read 2 * rgb - 1, as the runtime does.

usage: old_room.py REPO SKATE2_GAME OUT_DIR
"""
import struct
import sys
from pathlib import Path

import numpy as np

# Where the room stands: Ry(YAW) * DIST_petes + MOVE, as the PS3 "Skate 2 Background" mod placed it (fitted from the two
# models' vertices to ~1e-5 m). old_cas.py writes the same move into cas-skate2/cas.json "rooms".
YAW = 0.7870465
MOVE = (-1.41272, 200.00017, -6.7415)
# i_monoLightmap_Dot = SELECT[u16 at +6 of the material's lightmap record & 3] (s2.bin 0x82BCEACC, table 0x82E654D0).
SELECT = np.array([(1, 0, 0), (0, 1, 0), (0, 0, 1), (.33, .34, .33)])


def move(points, offset=MOVE):
    c, s = np.cos(YAW), np.sin(YAW)
    x, y, z = np.asarray(points, np.float64).T
    return np.stack([c * x + s * z + offset[0], y + offset[1], c * z - s * x + offset[2]], 1).astype(np.float32)


def layers(raw, table):
    """Chromaticity page GUID -> the layer its materials read from their layer page (one layer per page here)."""
    out = {}
    for section in table.entries:
        if section.type_id != 0x00eb0005:
            continue
        o = section.f0
        h = struct.unpack_from('>8I', raw, o)
        groups = []
        for j in range(h[1]):
            at = o + h[3] + j * 32
            v = struct.unpack_from('>8I', raw, at)
            kind = raw[o + v[0]:raw.index(b'\0', o + v[0])]
            if kind == b'Name':
                groups.append({})
            elif kind == b'lightmap' and groups:
                groups[-1]['layer'] = struct.unpack_from('>H', raw, at + 6)[0] & 3
            elif kind == b'chromaticity' and groups:
                groups[-1]['chroma'] = (v[4] << 32) | v[5]
        for g in groups:
            if 'layer' in g and 'chroma' in g and out.setdefault(g['chroma'], g['layer']) != g['layer']:
                raise ValueError(f"chromaticity page {g['chroma']:016X} is read with two layers")
    return out


def bake(rgba, width, height, chroma, chroma_width, chroma_height, layer):
    """Skate 2's environment shaders light by 4.5 * L^2 * C: L = dot(layer page, SELECT[layer]) and C the
    chromaticity page. The runtime reads 1.5 * page^2, so the page holds sqrt(3 * L^2 * C) (as old_sea.bake)."""
    from PIL import Image
    l = np.frombuffer(rgba, np.uint8).reshape(height, width, 4)[..., :3] / 255 @ SELECT[layer]
    c = Image.fromarray(np.frombuffer(chroma, np.uint8).reshape(chroma_height, chroma_width, 4)[..., :3])
    c = np.asarray(c.resize((width, height), Image.BILINEAR), np.float64) / 255
    out = np.full((height, width, 4), 255, np.uint8)
    out[..., :3] = np.round(np.clip(l[..., None] * np.sqrt(3 * c), 0, 1) * 255)
    return out.tobytes()


def main(repo, game, out):
    sys.path[:0] = [str(repo / 'tools/pipeline/skate2port'), str(repo)]
    from s2big import Big
    from tools.asset_pipeline import backdrop
    backdrop._vendor()
    import mdl_parser
    import rx2_parser
    import prepare_hawaiian_dream as prep
    # Skate 2's global models bind materials by import order (no external GUID table), as the PS3 mods do.
    bind = prep._bind_material_groups_by_guid
    prep._bind_material_groups_by_guid = lambda *a, **kw: bind(*a, **{**kw, 'allow_import_order_fallback': True})
    parse = mdl_parser.parse_rx2

    def moved(data):
        model = parse(data)
        for mesh in model.meshes:
            mesh.vertices = move(mesh.vertices)
            if mesh.normals is not None:
                mesh.normals = move(mesh.normals, (0, 0, 0))
        return model
    mdl_parser.parse_rx2 = moved
    archive = Big(game / 'data/big/miscboot.big')
    entries = {e.path: e for e in archive.entries}
    raw = archive.read(entries['data/content/world/models/DIST_petes.rx2'])
    table = rx2_parser.RX2File(raw)
    table.parse()
    layer_of = layers(raw, table)
    texture, fetched = backdrop._texture, []

    def remember(textures, guid):
        fetched.append(guid)
        return texture(textures, guid)
    backdrop._texture = remember
    # _export fetches the chromaticity page right before it calls the bake hook.
    lightmap = lambda *pages: bake(*pages, layer_of[fetched[-1]])
    out.mkdir(parents=True, exist_ok=True)
    if not backdrop._export(archive, entries, 'world/models/DIST_petes', 'world/models/DIST_petes_Textures',
                            'CreateSkater_Skate2', 'DIST_CreateSakter', out, None, lightmap):
        sys.exit("Skate 2's DIST_petes has no meshes")
    path = out / 'CreateSkater_Skate2.skate'
    print('written', path, path.stat().st_size, 'bytes')


if __name__ == '__main__':
    main(*map(Path, sys.argv[1:4]))
