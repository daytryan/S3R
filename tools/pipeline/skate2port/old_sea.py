"""New San Vanelona's sea (brief 10-29): Skate 2 draws it from the global model DIST_Water in miscboot.big (two
ocean.default meshes at y -64.63, out to the horizon), which the mod's district stream lacks. Written as the map's
render-only backdrop, its layer pages baked with their chromaticity pages into the page the runtime reads.

usage: old_sea.py REPO SKATE2_GAME ASSETS
"""
import sys
from pathlib import Path

import numpy as np


def bake(rgba, width, height, chroma, chroma_width, chroma_height):
    """Skate 2's ocean_defaultPS lights by 4.5 * L^2 * C: L = dot(layer page, the draw's layer select) and C the
    chromaticity page (summing to 1). Installed Skate 2 pages hold sqrt(3 * L^2 * C) (retail_world.wgsl: 1.5 * page^2,
    and L^2 = mean(page^2) for the ocean's specular). shortcut: L is the green layer, the only one DIST_Water's pages
    fill (red and blue 0, alpha 1); the select constant (VS c8) was not captured."""
    from PIL import Image
    layer = np.frombuffer(rgba, np.uint8).reshape(height, width, 4)[..., 1] / 255
    c = Image.fromarray(np.frombuffer(chroma, np.uint8).reshape(chroma_height, chroma_width, 4)[..., :3])
    c = np.asarray(c.resize((width, height), Image.BILINEAR), np.float64) / 255
    out = np.full((height, width, 4), 255, np.uint8)
    out[..., :3] = np.round(np.clip(layer[..., None] * np.sqrt(3 * c), 0, 1) * 255)
    return out.tobytes()


def main(repo, game, assets):
    sys.path[:0] = [str(repo / 'tools/pipeline/skate2port'), str(repo)]
    from s2big import Big
    from tools.asset_pipeline import backdrop
    backdrop._vendor()
    import prepare_hawaiian_dream as prep
    # Skate 2's global models bind materials by import order (no external GUID table), as the PS3 mods do.
    bind = prep._bind_material_groups_by_guid
    prep._bind_material_groups_by_guid = lambda *a, **kw: bind(*a, **{**kw, 'allow_import_order_fallback': True})
    archive = Big(game / 'data/big/miscboot.big')
    entries = {e.path: e for e in archive.entries}
    out = assets / 'private/native-backdrops'
    out.mkdir(parents=True, exist_ok=True)
    if not backdrop._export(archive, entries, 'world/models/DIST_Water', 'world/models/DIST_Water_Textures',
                            'New_San_Vanelona', 'DIST_NewSanVan', out, ('ocean.default',), bake):
        sys.exit("Skate 2's DIST_Water has no ocean.default")
    path = out / 'New_San_Vanelona.skate'
    print('written', path, path.stat().st_size, 'bytes')


if __name__ == '__main__':
    main(*map(Path, sys.argv[1:4]))
