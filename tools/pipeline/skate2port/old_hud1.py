"""Skate 1's HUD for its converted map: trick display, minimap (+icons) and session marker panel.

Writes assets/private/hud-skate1/ (the engine picks it on Skate_1_San_Vanelona when Settings > Skate 1 HUD is on).
Same movie/manifest pipeline as old_hud.py (Skate 2): Skate 1's `data/fe/screens.big` holds apt/const/geo/rx2 movies
the vendored skate3_ui extractor reads through the s2big adapter.

usage (numpy, PIL): py old_hud1.py REPO SKATE1_ROOT ASSETS [--probe | --minimap | --trickdisplay]
  --minimap       rebuild only runtime/minimap.json and its pages (the first run scans the world stream for the
                  street-plan tiles, ~11 min, then caches them under work/hud1-cache)
  --trickdisplay  rebuild only runtime/trickdisplay.json (its personal-best data)
  --marker        rebuild only the session marker panel (hud-skate1/session-marker and its tab)
"""
import hashlib, json, os, shutil, struct, sys
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/vendor/utt'), str(here)]
from s2big import Big
import vendor.skate3_ui.project as project
from vendor.skate3_ui.big import BigArchive
from vendor.skate3_ui.scene_graph import AssetCache, SceneFlattener
from vendor.skate3_ui.actions import Actions

OUT = assets/'private/hud-skate1'
WORK = WORK_ROOT/'hud1-cache'
CANDIDATES = {
    'inktrick': 'data/fe/source/screens/hudComponents/inktrick',
    'inkaction': 'data/fe/source/screens/hudComponents/inkaction',
    'inkbottomright': 'data/fe/source/screens/hudComponents/inkbottomright',
    'hudink': 'data/fe/source/screens/hud/hudink',
    'trickdisplay': 'data/fe/source/screens/hudComponents/trickdisplay',
    'skatetrick': 'data/fe/source/screens/hudComponents/skatetrick',
    'trick': 'data/fe/source/screens/hudComponents/trick',
    'filmtreatment': 'data/fe/source/screens/hudComponents/filmtreatment',
    'centralhud': 'data/fe/source/screens/hudComponents/centralhud',
    'minimap': 'data/fe/source/screens/hud/hud_minimap',
    'hudbase': 'data/fe/source/screens/hud/hudbase',
    'actionoverlay': 'data/fe/source/screens/hudComponents/actionoverlay',
}
# Chosen from --probe against the Xenia capture: trickdisplay has the trick name / points / line / modifier fields
# (103 root frames); the ink-smear sprites (inktrick, inkaction, inkbottomright) carry no text. The district name
# field lives in hud_minimap ("_DISTRICT NAME"), so the district plate is part of the minimap, not centralhud.
MOVIES = {'trickdisplay': CANDIDATES['trickdisplay']}
MAPSCREEN = 'data/fe/source/screens/mapping/mapscreen'
MAINMAP = 'data/fe/source/screens/mapping/maps/mainmap'
BUTTON_ITEM = 'data/fe/source/controls/button_item'
EXTRA = (MAPSCREEN, MAINMAP, BUTTON_ITEM)
# Skate 1's minimap is drawn natively (default.xex 0x82468e60: tiles 0x824692d0, north needle 0x82469a18, icons
# 0x82469960 -> 0x8246b7b0); hud_minimap only places it (mcMiniMap) and its district plate. Its numbers, by address
# (the engine side, old_minimap.rs, holds the zoom/heading/needle code constants):
SCALE = 1.0562  # px per metre (0x82136700)
PIN = 71.2811  # icons further than this (zoomed metres) sit on it, 75.3 px out (0x82136858)
FADE = [60.0, 350.0, 10000.0]  # 0x8246adc8: past the pin alpha 1 - (m - 60)/(350 - 60); none past 10000
DISC = 62.5968, 67.5968  # 0x824692d0's shader params: alpha 1 - smoothstep(62.5968, 67.5968, px from the centre)
# A 128 m tile is 135.1936 px (0x82190d98) drawn from (corner - player)*1.0562 + 128 - 135.1936: the disc centre
# shows the point 6.81 m +x +z of the skater (the game's own quirk, kept).
TILE_METRES, TILE_SHIFT = 128.0, 128.0 - 135.1936
STREAM = 'data/content/world/stream/BAM'  # .xmm: tile guid + world corner; .xst kinds; .xsf refpacked RW4 arenas
MINIMAP_RX2 = 'data/maps/minimap.rx2'  # miscboot: the mini_icon_* textures (0x82136798.. names them)
# Career challenges (missions.big: <magazine>_<tier>_<kind>NN) -> their minimap icon, mini_icon_chal_<kind>_<family>.
# The magazine prefix picks the family: he = The Skateboard Mag (red, the Xenia capture's S.K.A.T.E tally),
# ah = Thrasher (green), fp = Nopath (blue); Thrasher's art where a family has none. An own-the-spot is a spot.
FAMILY = {'he': 'smag', 'ah': 'thrash', 'fp': 'nopath'}
# ponytail: downhill -> race is by name (the exe's kind -> icon switch wasn't traced); racegate is a gate, not a start.
KINDS = (('tutorial', 'tutorial'), ('ownspot', None), ('spotrace', 'spotrace'), ('deathrace', 'deathrace'),
         ('demophoto', 'demo'), ('photo', 'photo'), ('prochallenge', 'pro'), ('besttrick', 'besttrick'),
         ('jam', 'jam'), ('skate', 'skate'), ('follow', 'followme'), ('downhill', 'race'))
MISSIONS = Big(s1game/'data/big/missions.big')
# Global_Teleporters locators with a minimap icon (Slappy's and the Regs have none in minimap.rx2).
PLACES = {'PlanB_Entrance_Global_Locator': 'mini_icon_planb', 'Xgames_Entrance_Global_Locator': 'mini_icon_chal_xgames'}
# BAM.loc's map locations: the train stations (the Xenia capture shows one pinned on the rim) and the MegaPark.
# ponytail: the two skate shops' maplocs are left out: which of shop_adidas/smag/thrash each is wasn't traced.
MAPLOCS = (('subway', 'mini_icon_trainstation'), ('megapark', 'mini_icon_megapark'))
# hud_minimap's mDistrictArray: the key GetMiniMapDistrict returns -> its label (english.big).
DISTRICTS = (('financial', 'ID_DISTRICT_FINANCIAL'), ('suburbs', 'ID_DISTRICT_SUBURBS'), ('oldTown', 'ID_DISTRICT_OLDTOWN'),
             ('urbanres', 'ID_DISTRICT_URBAN'), ('industry', 'ID_DISTRICT_INDUSTRY'), ('outskirts', 'ID_DISTRICT_OUTSKIRTS'))
# GetMiniMapDistrict (0x82456840) reads an index the exe latches at world entry (0x824642a4) from a zone query
# (0x82323b18 -> 0x8285a058); its zone hashes map only to suburbs/urbanres/financial/oldTown/industry (zones 5 and 6
# and 'no world' -> industry), so the plate never reads 'outskirts'.
# ponytail: the zone volumes weren't found (no FNV-1/1a hit over BAM.loc names); the nearest BAM.loc locator's
# district category stands in (as old_grading.py's Skate 1 zones), Outskirts -> industry (inferred: the only zone left).
DISTRICT_OF = {'DIST_financial': 'financial', 'DIST_Suburbs': 'suburbs', 'DIST_Old_Town': 'oldTown',
               'DIST_UrbanRes': 'urbanres', 'DIST_Outskirts': 'industry', 'Financial_XGames': 'financial',
               'Old_Town_PlanB': 'oldTown'}


class S1Archive:
    """The vendored extractor's BigArchive interface over a Skate 1 BIGF archive."""
    _safe_relative = staticmethod(BigArchive._safe_relative)

    def __init__(self, source):
        self.big = Big(source)
        self.entries = self.big.entries

    def read(self, entry):
        return self.big.read(entry)


def cache(prefixes):
    project.BigArchive = S1Archive
    project.find_big_directory = lambda root: Path(root)/'data'
    project.CORE_ARCHIVES = ('fe/screens.big', 'big/miscboot.big')
    project.extract_project(s1game, WORK, prefixes=prefixes, force=True)
    name_textures(MAPSCREEN)
    return AssetCache(WORK)


def name_textures(key):
    """Some Skate 1 bundles' texture dictionaries have no names (mapscreen, endchallengeoptions); their textures are
    stored in their bitmap ids' string order."""
    manifest_path = WORK/'assets'/key/'manifest.json'
    manifest = json.loads(manifest_path.read_text())
    if manifest['textures'] and not manifest['textures'][0]['name']:
        ids = sorted(str(ch['id']) for ch in json.loads((WORK/'metadata'/(key + '.apt.json')).read_text())['characters']
                     if ch['type_name'] == 'bitmap')
        assert len(ids) == len(manifest['textures']), (key, len(ids), len(manifest['textures']))
        for t, i in zip(manifest['textures'], ids):
            t['name'] = f'{key.rsplit("/", 1)[-1]}_textures\\{i}.Texture'
        manifest_path.write_text(json.dumps(manifest))


WHITE = 'assets/white.rgba'
# Skate 1's APT font names -> its miscboot data/fonts/*.gfn (the exe's (gfn, apt name) table, default.xex 0x82118b88).
GFN = {'NewCourierBlotStroke': 'indytype_stroke', 'Skate Monospaced Numbers': 'skatemonospaced',
       'TradeBld Stroke': 'tradebldstroke', 'NewCourierBlot': 'indytype', 'TradeBld': 'tradebld', 'Bit KitBold': 'bitkitbold'}
# ponytail: em size in glyph pixels, default ascent+descent (fits "LINE"); the trick name and score faces
# are sized by eye to the Xenia capture's "OLLIE 10".
EM = {'indytype_stroke': 39, 'skatemonospaced': 32}


def gfn_font(name):
    """A Skate 1 FntG font as the engine's bitmap-font definition plus its RGBA atlas under WORK.
    Layout: BE header (u16 ascent<<8|descent at 0x12, atlas texel size 1/w, 1/h floats at 0x40), 12-byte glyph
    records from 0x80 (u16 codepoint, u8 w, h, u16 x, y, u8 advance, s8 x offset, s8 y offset (top from baseline),
    pad), atlas 16 bytes after the texture header offset at 0x1c, stored as 4x4 (2 bytes/px: coverage, brightness)
    or 8x4 (1 byte/px coverage, 0..0x7d) pixel blocks."""
    import numpy as np
    from PIL import Image
    misc = Big(s1game/'data/big/miscboot.big')
    d = misc.read(next(e for e in misc.entries if e.path.endswith(f'/{name}.gfn')))
    ascent, descent = d[0x12], d[0x13]
    n = struct.unpack('>H', d[10:12])[0]
    w, h = (round(1/v) for v in struct.unpack('>2f', d[0x40:0x48]))
    start = struct.unpack('>I', d[0x1c:0x20])[0] + 0x10
    bpp = (len(d) - start)//(w*h)
    bw = 4 if bpp == 2 else 8
    px = np.frombuffer(d[start:start + w*h*bpp], np.uint8).reshape(h//4, w//bw, 4, bw, bpp).transpose(0, 2, 1, 3, 4).reshape(h, w, bpp)
    if bpp == 2:
        # Coverage has a floor of 15 over each whole glyph cell (drawn, it is a faint box round every letter).
        floor = 15
        coverage = ((np.maximum(px[..., 0].astype(np.int32), floor) - floor)*255//(255 - floor)).astype(np.uint8)
        rgba = np.dstack([px[..., 1]]*3 + [coverage])
    else:
        rgba = np.dstack([np.full((h, w), 255, np.uint8)]*3 + [(np.minimum(px[..., 0], 0x7d).astype(np.uint16)*255//0x7d).astype(np.uint8)])
    # Transparent texels keep stray brightness; black stops a light fringe round the dark outline when filtered.
    rgba[rgba[..., 3] == 0, :3] = 0
    # The disc packs glyphs edge to edge, so filtering at a quad's edge picked up the neighbour's outline (a box
    # round every letter). Repack each glyph with a transparent gutter.
    records = [struct.unpack('>HBBHHBbbB', d[0x80 + 12*i:0x80 + 12*i + 12]) for i in range(n)]
    gap, cx, cy, row, placed = 2, 2, 2, 0, []
    for _, gw, gh, *_ in records:
        if cx + gw + gap > w:
            cx, cy, row = 2, cy + row + gap, 0
        placed.append((cx, cy))
        cx, row = cx + gw + gap, max(row, gh)
    ph = 1 << (cy + row + gap - 1).bit_length()
    packed = np.zeros((ph, w, 4), np.uint8)
    for (_, gw, gh, x, y, *_), (px_, py_) in zip(records, placed):
        packed[py_:py_ + gh, px_:px_ + gw] = rgba[y:y + gh, x:x + gw]
    rgba, h = packed, ph
    rel = f'assets/fonts/{name}.rgba'
    (WORK/rel).parent.mkdir(parents=True, exist_ok=True)
    (WORK/rel).write_bytes(rgba.tobytes())
    Image.fromarray(rgba).save(WORK/f'assets/fonts/{name}.png')
    glyphs, chars = [], []
    for i, ((cp, gw, gh, _, _, adv, xo, yo, _), (x, y)) in enumerate(zip(records, placed)):
        glyphs.append({'glyph_index': i, 'width': gw, 'height': gh, 'x_offset': xo, 'y_offset': -yo, 'x_advance': adv,
                       'atlas_bounds': [x, y, x + gw, y + gh]})
        chars.append({'codepoint': cp, 'glyph_index': i})
    scale = 1/EM.get(name, ascent + descent)
    font = {'texture': rel, 'definition': {'format': 'skate1-gfn', 'glyphs': glyphs, 'characters': chars,
                                           'metrics': {'Ascent': ascent}, 'textures': [{'width': w, 'height': h}]}}
    return font, {'file_name': name, 'native_layout': {'ScaleX': scale, 'ScaleY': scale, 'OffsetX': 0.0, 'OffsetY': 0.0}}


def skate3_hud():
    """The installed Skate 3 HUD manifest: its font banks, font mappings and string table."""
    return json.loads((assets/'private/hud/runtime/trickdisplay.json').read_text(encoding='utf-8'))


def movie(c, name, s3):
    """prepare_hud.prepare's manifest for one movie: characters, flattened shapes, fonts, action blocks."""
    bundle = c.load_bundle(name)
    apt_path = WORK/'raw'/(name + '.apt')
    actions = Actions(apt_path.read_bytes(), apt_path.with_suffix('.const').read_bytes())
    blocks, shapes, fonts, mappings = {}, {}, {}, {}
    for ch in bundle['characters'].values():
        for f in ch.get('frames', []):
            for control in f['controls']:
                if control['type_name'] in ('do_action', 'do_init_action') and control.get('actions_offset', 0):
                    blocks[str(control['actions_offset'])] = actions.stream(control['actions_offset'])
        if ch['type_name'] == 'shape':
            scene = SceneFlattener(c, lambda *_: {}).flatten(name, ch['id'])
            if scene['unresolved']:
                raise ValueError(f'{name}: unresolved shape {scene["unresolved"]}')
            for p in scene['primitives']:
                # The engine draws every shape textured, so a solid fill samples a white texel and keeps its colour.
                if not p.get('texture'):
                    p['texture'] = {'width': 1, 'height': 1, 'rgba': WHITE}
                    for tri in p['triangles']:
                        for v in tri:
                            v['uv'] = [0.5, 0.5]
            shapes[str(ch['id'])] = scene['primitives']
        elif ch['type_name'] == 'font':
            apt_name = ch['font']['name']
            fonts[apt_name], mappings[apt_name] = gfn_font(GFN[apt_name])
    # Skate 1's HUD text is all capitals ("OLLIE" in the Xenia capture); trick names arrive as string IDs, so the
    # strings they resolve to are capitalised here.
    import re
    caps = lambda s: ''.join(p if p.startswith(('{', '<')) else p.upper() for p in re.split(r'(\{[^}]*\}|<[^>]*>)', s))
    language = {k: caps(v) for k, v in s3['language'].items()}
    for t in {(p['texture']['rgba'], p['texture']['width'], p['texture']['height']) for prims in shapes.values() for p in prims}:
        if t[0] != WHITE:
            bleed(*t)
    return {'format': 'skate3-scoring-hud', 'version': 1,
            'source': {'bundle': name, 'game': 'skate1', 'apt_sha256': hashlib.sha256(apt_path.read_bytes()).hexdigest()},
            'characters': list(bundle['characters'].values()), 'shapes': shapes, 'fonts': fonts, 'actions': blocks,
            'font_mappings': mappings, 'language': language, 'unresolved_fonts': []}


def bleed(rel, w, h, faint=32):
    """Skate 1's ink textures pad transparency with chroma green (2, 255, 2), which DXT also smears into the faint
    edge; filtering then draws a green rim round the ink. Give every texel below `faint` alpha its nearest visible
    neighbours' colour (alpha untouched), in place in the cache.
    Each pass grows the known area by one texel (4-neighbour mean, summed up, down, left, right in float32), for at
    most max(w, h) passes; only the frontier is computed, in padded arrays where unknown texels hold 0."""
    import numpy as np
    from vendor.skate3_ui.binary import read_file, write_file
    a = np.frombuffer(read_file(WORK/rel), np.uint8).reshape(h, w, 4).copy()
    known = a[..., 3] >= faint
    if known.all() or not known.any():
        return
    rgb = np.zeros((h + 2, w + 2, 3), np.float32)
    rgb[1:-1, 1:-1] = a[..., :3].astype(np.float32)*known[..., None]
    k = np.zeros((h + 2, w + 2), bool)
    k[1:-1, 1:-1] = known
    ys, xs = np.nonzero(~known & (k[:-2, 1:-1] | k[2:, 1:-1] | k[1:-1, :-2] | k[1:-1, 2:]))
    ys, xs = ys + 1, xs + 1
    for _ in range(max(w, h)):
        if not ys.size:
            break
        total = ((rgb[ys - 1, xs] + rgb[ys + 1, xs]) + rgb[ys, xs - 1]) + rgb[ys, xs + 1]
        count = ((k[ys - 1, xs].astype(np.float32) + k[ys + 1, xs]) + k[ys, xs - 1]) + k[ys, xs + 1]
        rgb[ys, xs] = total/count[:, None]
        k[ys, xs] = True
        cy, cx = np.concatenate([ys - 1, ys + 1, ys, ys]), np.concatenate([xs, xs, xs - 1, xs + 1])
        inside = (cy >= 1) & (cy <= h) & (cx >= 1) & (cx <= w)
        cy, cx = cy[inside], cx[inside]
        fresh = ~k[cy, cx]
        cell = np.unique(cy[fresh]*(w + 2) + cx[fresh])
        ys, xs = cell // (w + 2), cell % (w + 2)
    a[..., :3] = rgb[1:-1, 1:-1].round().astype(np.uint8)
    write_file(WORK/rel, a.tobytes())


def minimap_pages(manifest):
    """{file: [w, h]} of every page the minimap manifest draws (tiles are PNGs)."""
    pages = [manifest['arrow'], manifest['north'], *manifest['icons'], *manifest['skaters'], manifest['district'],
             *manifest.get('markers', {}).values()]
    sizes = {p['texture']: p['size'] for p in pages}
    return sizes | {t['texture']: manifest['tiles']['size'] for t in manifest['tiles']['list']}


def payloads(manifest):
    """Every RGBA file a manifest references (shape textures, font pages, minimap pages), relative to its cache root."""
    if 'centre' in manifest:
        return set(minimap_pages(manifest))
    files = {p['texture']['rgba'] for prims in manifest['shapes'].values() for p in prims if p.get('texture')}
    return files | {f['texture'] for f in manifest['fonts'].values()}


def source_of(rel):
    """A payload lives in this script's cache, or (Skate 3's font pages) in the installed Skate 3 HUD."""
    for root in (WORK, assets/'private/hud'):
        if (root/rel).is_file():
            return root/rel
    raise FileNotFoundError(rel)


def install(manifests):
    OUT.mkdir(parents=True, exist_ok=True)
    (WORK/WHITE).parent.mkdir(parents=True, exist_ok=True)
    (WORK/WHITE).write_bytes(b'\xff'*4)
    for stem, manifest in manifests.items():
        for rel in payloads(manifest):
            (OUT/rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_of(rel), OUT/rel)
        (OUT/'runtime').mkdir(exist_ok=True)
        (OUT/'runtime'/(stem + '.json')).write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
        print(stem, len(payloads(manifest)), 'payloads')


def check(stems):
    """Re-read what was installed: every referenced payload exists at its declared size."""
    for stem in stems:
        manifest = json.loads((OUT/'runtime'/(stem + '.json')).read_text(encoding='utf-8'))
        if 'centre' in manifest:
            sizes = {rel: size for rel, size in minimap_pages(manifest).items() if not rel.endswith('.png')}
            for rel in set(minimap_pages(manifest)) - set(sizes):
                assert (OUT/rel).read_bytes()[:8] == b'\x89PNG\r\n\x1a\n', f'{stem}: {rel} is not a PNG'
            d = manifest['district']
            assert len(d['plates']) == len(d['names']) and all(0 <= p[2] < len(d['plates']) for p in d['points'])
            assert all(d['plates'][p[2]] for p in d['points']), f'{stem}: a district point has no plate'
            assert d['intro'][-1] == [1.0, 0.0], f'{stem}: the plate intro does not end at rest'
        else:
            sizes = {p['texture']['rgba']: [p['texture']['width'], p['texture']['height']]
                     for prims in manifest['shapes'].values() for p in prims if p.get('texture')}
            sizes |= {f['texture']: [f['definition']['textures'][0]['width'], f['definition']['textures'][0]['height']]
                      for f in manifest['fonts'].values()}
        for rel, (w, h) in sizes.items():
            assert (OUT/rel).stat().st_size == w*h*4, f'{stem}: {rel} is not {w}x{h} RGBA'
    print('check ok:', OUT)


def locators(entry):
    """(name, world x, z) of each locator in one missions.big RW4 file: names in order, 4x4 BE matrices from 0x200."""
    import re
    data = MISSIONS.read(entry)
    names = [m.group().decode() for m in re.finditer(rb'[A-Za-z_][A-Za-z0-9_]{5,}', data)][1:]  # [0] is 'RW4xb2'
    out = []
    for i, name in enumerate(names):
        o = 0x200 + 0x80*i
        if o + 64 > len(data):
            break
        f = struct.unpack('>16f', data[o:o + 64])
        if f[15] != 1.0:
            break
        out.append((name, f[12], f[14]))
    return out


def first_transform(data):
    """World (x, z) of a challenge file's first placed locator: the first rigid BE 4x4 with a translation. The file
    leads with its models (identity-positioned meshes), so the locators come after them."""
    for o in range(0, len(data) - 63, 16):
        f = struct.unpack('>16f', data[o:o + 64])
        if f[15] == 1.0 and f[3] == f[7] == f[11] == 0 and all(abs(sum(v*v for v in f[r:r + 3]) - 1) < 1e-3 for r in (0, 4, 8)) \
                and 1 < abs(f[12]) + abs(f[14]) < 1e4:
            return f[12], f[14]
    return None


def bam_locations():
    """(name, world x, z, category) of every BAM.loc locator."""
    import re
    text = (s1game/(STREAM + '.loc')).read_text(encoding='latin-1')
    for m in re.finditer(r'<Location>(.*?)</Location>', text, re.S):
        b = m.group(1)
        n, t, cat = (re.search(rf'<{k}>(.*?)</{k}>', b) for k in ('Name', 'Transform', 'Category'))
        if n and t:
            v = [float(x) for x in t.group(1).split(',')]
            yield n.group(1), v[12], v[14], cat and cat.group(1)


def career():
    """The challenges Skate 1's career offers: the names of miscgame.big progress.sp's 0x400-byte records (count and
    record size first), underscores dropped (the disc has fp_u3_tutorial_04 for the career's fp_u3_tutorial04)."""
    g = Big(s1game/'data/big/miscgame.big')
    d = g.read(next(e for e in g.entries if e.path.endswith('/progress.sp')))
    count, size = struct.unpack_from('>II', d)
    return {d[8 + i*size:8 + i*size + 32].split(b'\0')[0].decode().lower().replace('_', '') for i in range(count)}


def places(icons):
    """(icon, world x, z) for every career challenge start and map place Skate 1 marks on its minimap. Challenge files
    the career never offers (the demo build's demophoto05s, cut ah_a1_photo01 / ah_p1_deathrace05 / he_p1_photo03)
    are left out (the Xenia capture shows neither of the two in its range: he_a1_demophoto05, ah_a1_photo01).
    ponytail: every offered challenge shows, as if all were unlocked: retail shows the unlocked ones of the save's
    career (progress.sp's prerequisites), and the port has no Skate 1 career."""
    out, offered = [], career()
    for e in MISSIONS.entries:
        stem = Path(e.path).stem
        mag, _, rest = stem.partition('_')
        if mag not in FAMILY or stem.endswith('_Textures') or '_challengeLocator' in stem:  # a jam's extra locator file
            continue
        if stem.lower().replace('_', '') not in offered:
            continue
        kind = rest.partition('_')[2]
        match = next((s for k, s in KINDS if kind.startswith(k)), False)
        if match is False:
            continue
        icon = 'mini_icon_spot' if match is None else next(
            (n for n in (f'mini_icon_chal_{match}_{FAMILY[mag]}', f'mini_icon_chal_{match}_thrash') if n in icons), None)
        start = icon and first_transform(MISSIONS.read(e))
        if start:
            out.append((icon, *start))
    teleporters = next(e for e in MISSIONS.entries if e.path.endswith('/Global_Teleporters.rx2'))
    out += [(PLACES[n], x, z) for n, x, z in locators(teleporters) if n in PLACES]
    out += [(icon, x, z) for n, x, z, _ in bam_locations() for key, icon in MAPLOCS
            if key in n.lower() and (n.lower().endswith('_maploc') or n == key)]
    return out


def mini_icons():
    """miscboot's minimap.rx2 mini_icon_* textures as RGBA pages: {name: {texture, size}}."""
    from vendor.skate3_ui.rw4 import read_textures
    from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant
    misc = Big(s1game/'data/big/miscboot.big')
    entry = next(e for e in misc.entries if e.path.replace('\\', '/').lower().endswith(MINIMAP_RX2))
    folder = WORK/'assets/minimap/icons'
    folder.mkdir(parents=True, exist_ok=True)
    (folder/'minimap.rx2').write_bytes(misc.read(entry))
    out = {}
    for t in read_textures(folder/'minimap.rx2')[1]:
        name, m = t.name.split('\\')[-1].removesuffix('.Texture'), t.metadata
        rgba = decode_rgba(t.data, parse_fetch_constant(bytes.fromhex(m['fetch_constant_hex'])))
        assert len(rgba) == m['width']*m['height']*4, name
        (folder/(name + '.rgba')).write_bytes(rgba)
        out[name] = {'texture': f'assets/minimap/icons/{name}.rgba', 'size': [m['width'], m['height']]}
    return out


def tiles():
    """The street-plan tiles: BAM.xmm lists each 128 m tile's texture guid and world (x, z) corner; the textures are
    in the stream's kind-4096 RW4 arenas (refpacked in .xsf). The decoded PNGs and their index are cached under WORK
    (delete assets/minimap/tiles to rescan).
    The stream holds each arena ~7 times (34,597 records of 4,798 assets): a record whose packed bytes were already
    seen holds the same textures, every corner one already found, so it is skipped unread (the scan took ~5 min)."""
    from PIL import Image
    from tools.owned_game.refpack import decompress
    from vendor.skate3_ui.rw4 import parse_textures
    from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant
    folder = WORK/'assets/minimap/tiles'
    index = folder/'index.json'
    if index.is_file():
        return json.loads(index.read_text(encoding='utf-8'))
    stream = s1game/STREAM
    xmm = stream.with_suffix('.xmm').read_bytes()
    corners = {struct.unpack_from('>Q', xmm, 24 + 16*i)[0]: struct.unpack_from('>ff', xmm, 32 + 16*i)
               for i in range((len(xmm) - 24)//16)}
    xst = stream.with_suffix('.xst').read_bytes()
    arenas = {struct.unpack_from('>Q', xst, 24 + 64*i)[0] for i in range(struct.unpack_from('>I', xst, 16)[0])
              if struct.unpack_from('>I', xst, 24 + 64*i + 36)[0] == 4096}
    folder.mkdir(parents=True, exist_ok=True)
    found, seen = {}, set()
    with open(stream.with_suffix('.xsf'), 'rb') as xsf:
        size, at = xsf.seek(0, 2), 0x80
        while at < size:  # 128-byte record headers: asset id, stored size, header size, stride
            xsf.seek(at)
            asset, stored, header, stride = struct.unpack('>QIII', xsf.read(20))
            if not asset:
                break
            if asset in arenas:
                xsf.seek(at + header)
                packed = xsf.read(stored)
                digest = hashlib.blake2b(packed, digest_size=16).digest()
                if (asset, digest) in seen:
                    at += stride
                    continue
                seen.add((asset, digest))
                arena = decompress(packed)
                try:
                    textures = parse_textures(arena, 'arena.rx2')[1]
                except Exception:
                    textures = []  # not a texture arena
                for t in textures:
                    if t.guid in corners and t.guid not in found:
                        m = t.metadata
                        rgba = decode_rgba(t.data, parse_fetch_constant(bytes.fromhex(m['fetch_constant_hex'])))
                        Image.frombytes('RGBA', (m['width'], m['height']), rgba).save(folder/f'{t.guid:016x}.png')
                        found[t.guid] = [m['width'], m['height']]
            at += stride
    assert {tuple(s) for s in found.values()} == {(256, 256)}, set(map(tuple, found.values()))
    listed = [{'texture': f'assets/minimap/tiles/{g:016x}.png', 'world': list(corners[g])} for g in sorted(found)]
    missing = [{'guid': f'{g:016x}', 'world': list(corners[g])} for g in sorted(set(corners) - set(found))]
    out = {'list': listed, 'missing': missing}
    index.write_text(json.dumps(out, indent=1), encoding='utf-8')
    return out


def district(c, strings):
    """hud_minimap's district plate: its text field (districtText in mcDistrictName) laid out for each district name
    as the engine lays out Skate 1 text, at the field's resting place (after the intro), plus the BAM.loc points the
    engine picks the nearest of."""
    bundle = c.load_bundle(CANDIDATES['minimap'])
    chars = {ch['id']: ch for ch in bundle['characters'].values()}

    def compose(m, n):  # m after n (APT 2x3: a b c d tx ty)
        a, b, cc, d, tx, ty = n
        return [m[0]*a + m[2]*b, m[1]*a + m[3]*b, m[0]*cc + m[2]*d, m[1]*cc + m[3]*d,
                m[0]*tx + m[2]*ty + m[4], m[1]*tx + m[3]*ty + m[5]]

    # The root's depth-11 sprite (mcDistrictName) up to its resting frame 9: placements with a matrix (flag 4) move it,
    # with a colour transform (flag 8, alpha first) fade it. Frames 1-9 are the 'intro' label SetState(1) plays.
    root, plate, matrix, alpha, steps = chars[0], None, None, 1.0, []
    for f in root['frames'][:10]:
        for p in f['controls']:
            if p['type_name'] == 'place_object2' and p['depth'] == 11:
                plate = p['character_id'] if p['character_id'] >= 0 else plate
                matrix = p['matrix'] if p['flags'] & 4 else matrix
                alpha = p['color_transform'][0]/255 if p['flags'] & 8 else alpha
        steps.append((alpha, matrix[5]))
    intro = [[a, y - steps[-1][1]] for a, y in steps[1:]]  # [alpha, y px from rest] per 60 Hz frame (16 ms/frame)
    assert intro[-1] == [1.0, 0.0] and intro[0][0] < 0.25, intro
    field = next(p for p in chars[plate]['frames'][0]['controls'] if p.get('name') == 'districtText')
    matrix = compose(matrix, field['matrix'])
    text = chars[field['character_id']]
    t = text['text']
    font, layout = gfn_font(GFN[chars[t['font_id']]['font']['name']])
    d = font['definition']
    glyphs = {g['glyph_index']: g for g in d['glyphs']}
    by_char = {ch['codepoint']: glyphs[ch['glyph_index']] for ch in d['characters']}
    atlas = d['textures'][0]
    s = t['font_height']*layout['native_layout']['ScaleX']
    x0b, y0b, x1b, _ = text['bounds']
    assert t['alignment'] == 2, t  # centred
    plates, names = [], []
    for key, label in DISTRICTS:
        value = strings.get(label)
        names.append(value or '')
        if not value:  # outskirts has no string in Skate 1
            plates.append([])
            continue
        width = sum(by_char[ord(ch)]['x_advance']*s for ch in value)
        assert width <= x1b - x0b - 4, (value, width)  # one line: the field word-wraps
        pen, quads = x0b + (x1b - x0b - width)/2, []
        for ch in value:
            g = by_char[ord(ch)]
            gx, gy = pen + g['x_offset']*s, y0b + (d['metrics']['Ascent'] - g['y_offset'])*s
            u0, v0, u1, v1 = g['atlas_bounds']
            corners = [(gx, gy, u0, v0), (gx + g['width']*s, gy, u1, v0), (gx + g['width']*s, gy + g['height']*s, u1, v1),
                       (gx, gy + g['height']*s, u0, v1)]
            for i in (0, 1, 2, 0, 2, 3):
                x, y, u, v = corners[i]
                quads.append({'position': [matrix[0]*x + matrix[2]*y + matrix[4], matrix[1]*x + matrix[3]*y + matrix[5]],
                              'uv': [u/atlas['width'], v/atlas['height']]})
            pen += g['x_advance']*s
        plates.append(quads)
    argb = int(t['color_argb'].lstrip('#'), 16)
    keys = [k for k, _ in DISTRICTS]
    points = [[x, z, keys.index(DISTRICT_OF[cat])] for _, x, z, cat in bam_locations() if cat in DISTRICT_OF]
    return {'texture': font['texture'], 'size': [atlas['width'], atlas['height']],
            'colour': [((argb >> n) & 255)/255 for n in (16, 8, 0, 24)], 'names': names, 'plates': plates, 'points': points,
            'intro': intro}


def english(labels):
    """Skate 1's English text of each label (english.big tables, keyed by the label's x*33+c hash)."""
    from s2lang import table
    rows, big = {}, Big(s1game/'data/big/english.big')
    for e in big.entries:
        if e.path.lower().endswith('.bin') and 'histogram' not in e.path.lower():
            rows.update(table(big.read(e)))

    def key(label):
        if isinstance(label, int):  # a string the game looks up by its hash alone
            return label
        x = 0xFFFFFFFF
        for ch in label.encode('latin-1'):
            x = (x*33 + ch) & 0xFFFFFFFF
        return x
    return {label: rows[key(label)] for label in labels if key(label) in rows}


# Skate 1's personal bests (default.xex): 16 rows of 24 bytes at 0x830a37a8 (best f32, minimum f32, title label ->
# ID_RECORD_BESTS_01..16), each value's text by 0x82330718's jump table at 0x823307b4. Engine side: trick_stats.rs.
BESTS = ((500.0, 'number'), (9.0, 'time'), (6.0, 'number'), (6.0, 'time'), (2000.0, 'number'), (35.0, 'time'),
         (4.0, 'number'), (12.0, 'time'), (10.0, 'time'), (2.0, 'time'), (4.0, 'distance'), (15.0, 'distance'),
         (5.0, 'distance'), (2.0, 'distance'), (2.0, 'distance'), (2.0, 'number'))
DISTANCE_FORMAT = 0x7f263aa8  # 0x82330910: the metric distance text's string hash ('{0:f.1} m')
DISTANCE_FEET = 0xbe745eef  # the imperial one ('{0:f.1} ft'), when Units is on Feet (settings +18904 == 1)


def trick_stats():
    """trickdisplay's personal-best callout (UpdatePersonalBests): the table and the strings its value text uses."""
    text = english(['ID_COMMON_METRIC_TIME', 'ID_COMMON_HYPHEN', DISTANCE_FORMAT, DISTANCE_FEET])
    return {'bests': [{'label': f'ID_RECORD_BESTS_{i + 1:02d}', 'minimum': m, 'format': f} for i, (m, f) in enumerate(BESTS)],
            'time': text['ID_COMMON_METRIC_TIME'], 'hyphen': text['ID_COMMON_HYPHEN'], 'distance': text[DISTANCE_FORMAT],
            'distance_feet': text[DISTANCE_FEET]}


def minimap(c):
    """Skate 1's native minimap as data for the engine (old_minimap.rs): the disc of street-plan tiles centred on
    hud_minimap's mcMiniMap, the player arrow, the north needle, the icons of every career challenge and map place,
    the online skaters' dots and the district plate under it."""
    icons = mini_icons()
    bundle = c.load_bundle(CANDIDATES['minimap'])
    root = next(ch for ch in bundle['characters'].values() if ch['id'] == 0)
    centre = next(p['matrix'][4:6] for p in root['frames'][0]['controls'] if p.get('name') == 'mcMiniMap')
    marks = places(icons)
    grid = tiles()
    print(len(marks), 'minimap icons;', len(grid['list']), 'tiles,', len(grid['missing']), 'missing from the disc', grid['missing'])
    return {'centre': centre, 'scale': SCALE, 'pin': PIN, 'fade': FADE, 'zoom': True,
            'tiles': {'feather': DISC[0], 'radius': DISC[1], 'metres': TILE_METRES, 'shift': TILE_SHIFT, 'size': [256, 256],
                      'list': grid['list']},
            'arrow': icons['mini_icon_arrow'], 'north': icons['mini_icon_north'],
            'icons': [{**icons[icon], 'world': [x, z]} for icon, x, z in marks],
            'skaters': [icons[f'mini_icon_skater{i:02}'] for i in range(1, 9)],
            # The race gates skate1_online shows (mini_icon_racegate, table 0x830065b8), placed at run time.
            'markers': {'mini_icon_racegate': icons['mini_icon_racegate']},
            'district': district(c, english([label for _, label in DISTRICTS]))}


def session_marker(c, s3):
    """Skate 1's D-pad marker panel (actionoverlay's mcDpadoverlay), flattened into the session-marker overlay format
    the engine shows while the marker button is held. Skate 1's game code drove it natively (the movie only hides
    it on frame 0): label 'two' (frame 1) = Go To Marker on D-pad up over Place Marker on D-pad down. Its 'one'
    label (frame 14, Place alone, for before a marker exists) is left out: the user wants both rows always, with
    no jump when a marker is placed. Its Button Item slots get button_item's
    Xbox_DPad_Up / Xbox_DPad_Down art centred on the placeholder, and its TradeBld rows use Skate 1's tradebld.gfn.
    Not in Skate 1: a third row, B Player List (role "players", in `player_list`, shown only online as Skate 3's), made
    of the panel's own pieces one row lower: the bottom paper strip, a smaller copy of the velcro patch, button_item's
    Xbox_B and a TradeBld row."""
    ao = CANDIDATES['actionoverlay'].lower()  # the flattener's bundle keys are lower case
    buttons = {'/up/': 104, '/down/': 89}  # button_item exports Xbox_DPad_Up, Xbox_DPad_Down
    language = s3['language']
    font, layout = gfn_font('tradebld')
    d = font['definition']
    glyphs = {g['glyph_index']: g for g in d['glyphs']}
    by_char = {ch['codepoint']: glyphs[ch['glyph_index']] for ch in d['characters']}
    atlas = d['textures'][0]
    em = 1/layout['native_layout']['ScaleY']  # glyph pixels per em (gfn_font's default: ascent + descent)
    ascent = d['metrics']['Ascent']
    exports = c.load_bundle(BUTTON_ITEM)['exports']
    buttons['/b/'] = exports['Xbox_B']
    art = {}
    for key, char in buttons.items():
        flat = SceneFlattener(c, lambda *_: {'frame': 0}).flatten(BUTTON_ITEM, char)
        assert flat['primitives'] and not flat['unresolved'], (key, flat['unresolved'])
        art[key] = flat['primitives']
    # The panel (mcDpadoverlay, sprite 22) and the tab: main_mc (39), the "SPOT FOUND!" sign, rests after its outro
    # (frame 53, a stop) almost wholly off the top-left corner; the edge of its tan tape stays on screen as the tab
    # the Xenia captures show while the marker panel is closed.
    for folder, shown, frame in (('session-marker', 22, 1), ('session-marker-tab', 39, 53)):
        def state(path, owner, ch):
            if ch['type_name'] in ('sprite', 'animation'):
                if owner['key'] == ao and ch['id'] in (22, 39):
                    return {'frame': frame} if ch['id'] == shown else {'visible': False}
                return {'frame': 0}
            return {}
        flat = SceneFlattener(c, state).flatten(CANDIDATES['actionoverlay'], 0)
        if shown == 39:  # only what shows on screen: the sign's text and challenge icon are off it
            flat['primitives'] = [p for p in flat['primitives']
                                  if max(v['position'][0] for t in p['triangles'] for v in t) > 0
                                  and max(v['position'][1] for t in p['triangles'] for v in t) > 0]
            flat['text'], flat['unresolved'] = [], []
        assert all('TradeBld' in u['reason'] for u in flat['unresolved']), flat['unresolved']
        out = OUT/folder
        out.mkdir(parents=True, exist_ok=True)
        textures, ids, meshes = [], {}, []

        def texture(rel, width, height):
            if rel not in ids:
                data = (WORK/rel).read_bytes()
                assert len(data) == width*height*4, rel
                ids[rel] = len(textures)
                (out/f'texture-{len(textures)}.rgba').write_bytes(data)
                textures.append({'file': f'texture-{len(textures)}.rgba', 'width': width, 'height': height, 'source': str(rel)})
            return ids[rel]

        def quad(x0, y0, x1, y1, u0=0., v0=0., u1=1., v1=1.):
            q = [((x0, y0), (u0, v0)), ((x1, y0), (u1, v0)), ((x1, y1), (u1, v1)), ((x0, y1), (u0, v1))]
            return [{'position': list(q[i][0]), 'uv': list(q[i][1])} for i in (0, 1, 2, 0, 2, 3)]

        # Each name has two Button Items (depths 12/14 and 25/27); the game's by-name lookup reached the later pair,
        # which is the one the 'one' layout moves into the patch. The earlier pair stays empty.
        live = {k: max((p['draw_order'] for p in flat['primitives'] if k in p['path'] and 'placeholder' in p['path']),
                       default=None) for k in buttons}
        for p in flat['primitives']:
            vertices = [{'position': v['position'], 'uv': v.get('uv', [0, 0])} for tri in p['triangles'] for v in tri]
            if max(v['position'][1] for v in vertices) < 0:
                continue  # the 'one' layout parks the Go To row above the screen
            key = next((k for k in buttons if k in p['path']), None)
            if key and 'placeholder' in p['path'] and p['draw_order'] != live[key]:
                continue
            if key and 'placeholder' in p['path']:
                xs, ys = [v['position'][0] for v in vertices], [v['position'][1] for v in vertices]
                cx, cy = (min(xs) + max(xs))/2, (min(ys) + max(ys))/2
                bx = [v['position'][0] for a in art[key] for t in a['triangles'] for v in t]
                by = [v['position'][1] for a in art[key] for t in a['triangles'] for v in t]
                dx, dy = cx - (min(bx) + max(bx))/2, cy - (min(by) + max(by))/2
                for a in art[key]:
                    t = a['texture']
                    meshes.append({'texture': texture(t['rgba'], t['width'], t['height']), 'color': [1, 1, 1, p['color'][3]],
                                   'vertices': [{'position': [v['position'][0] + dx, v['position'][1] + dy], 'uv': v['uv']}
                                                for tri in a['triangles'] for v in tri],
                                   'order': p['draw_order'], 'source': p['path'] + ' <- button_item ' + str(buttons[key])})
                continue
            if not p.get('texture'):
                continue
            t = p['texture']
            meshes.append({'texture': texture(t['rgba'], t['width'], t['height']), 'color': p['color'], 'vertices': vertices,
                           'order': p['draw_order'], 'source': p['path']})
        # The rows: the flattener has no packaged TradeBld, so they are laid out here from the gfn exactly as the
        # engine's apt_scene lays out Skate 1 text: from the box's top-left, glyph top = ascent - y_offset.
        players = []
        texts = list(flat['text'])
        if shown == 22:
            # The B row: one row below Place, as the Place row sits below Go To.
            top = next(t for t in texts if t['path'].endswith('/topText'))
            bottom = next(t for t in texts if t['path'].endswith('/bottomText'))
            row = bottom['matrix'][5] - top['matrix'][5]
            texts.append(dict(bottom, value='#Player List', path=bottom['path'].replace('bottomText', 'playersText'),
                              matrix=bottom['matrix'][:5] + [bottom['matrix'][5] + row], players=True))
        for t in texts:
            a, b, cc, dd, tx, ty = t['matrix']
            value = language.get(t['value'], t['value']).lstrip('#').upper()  # Skate 1's HUD text is capitals
            s = t['font_height']/em
            pen, base = t['bounds'][0], t['bounds'][1] + ascent*s
            if ty + base < 0:
                continue  # parked above the screen, as the 'one' layout's Go To row
            vertices = []
            for letter in value:
                g = by_char.get(ord(letter))
                if g is None:
                    raise ValueError(f'tradebld.gfn has no {letter!r} for {value!r}')
                x0, y0 = pen + g['x_offset']*s, base - g['y_offset']*s
                x1, y1 = x0 + g['width']*s, y0 + g['height']*s
                u0, v0, u1, v1 = g['atlas_bounds']
                for v in quad(x0, y0, x1, y1, u0/atlas['width'], v0/atlas['height'], u1/atlas['width'], v1/atlas['height']):
                    x, y = v['position']
                    v['position'] = [a*x + cc*y + tx, b*x + dd*y + ty]
                    vertices.append(v)
                pen += g['x_advance']*s
            argb = int(t['color_argb'].lstrip('#'), 16)
            colour = [((argb >> n) & 255)/255 for n in (16, 8, 0)] + [t['alpha']*((argb >> 24) & 255)/255]
            (players if t.get('players') else meshes).append(
                {'texture': texture(font['texture'], atlas['width'], atlas['height']), 'color': colour, 'vertices': vertices,
                 'order': t['draw_order'], 'source': t['path'] + ' ' + value})
        if shown == 22:
            players[:0] = b_row(meshes, art['/b/'], players[0], texture)
            if '--check' in sys.argv:
                print('players', [(m['source'][-40:], round(min(v['position'][1] for v in m['vertices']))) for m in players])
        manifest = {'version': 1, 'canvas': [1280, 720], 'textures': textures,
                    'meshes': sorted(meshes, key=lambda m: m['order']),
                    'player_list': [dict(m, role='players') for m in players],
                    'notes': f"Skate 1's actionoverlay sprite {shown} (frame {frame}), flattened by skate2port/old_hud1.py."}
        (out/'hud.json').write_text(json.dumps(manifest, indent=1) + '\n', encoding='utf-8')
        print(folder, len(meshes), 'meshes,', len(textures), 'textures ->', out/'hud.json')


def b_row(meshes, b_art, text, texture):
    """The B row's art under its text: the bottom paper strip one strip lower, then a copy of the velcro patch scaled to
    one row (drawn after the strip, as the patch is over the strips) with button_item's Xbox_B centred beside the text."""
    def ys(m):
        return [v['position'][1] for v in m['vertices']]

    def moved(m, f, tag):
        return dict(m, vertices=[{'position': [v['position'][0], f(v['position'][1])], 'uv': v['uv']} for v in m['vertices']],
                    source=m['source'] + tag)
    upper = next(m for m in meshes if m['source'].endswith('/depth_3/depth_1'))
    lower = next(m for m in meshes if m['source'].endswith('/depth_5/depth_1'))
    step = min(ys(lower)) - min(ys(upper))
    strip = moved(lower, lambda y: y + step, ' (B row)')
    patch = [m for m in meshes if '/depth_7/' in m['source']]
    down = [m for m in meshes if '/down/' in m['source'] and 'button_item' in m['source']]
    # The B row draws after the panel, so the strip's left end (under the patches in Skate 1's layering) is cut away
    # short of the D-pad art, and the new patch starts just under the down arrow.
    cut = max(v['position'][0] for m in patch for v in m['vertices']) - 14
    xs = [v['position'][0] for v in strip['vertices']]
    sx0, sx1 = min(xs), max(xs)
    u0 = next(v['uv'][0] for v in strip['vertices'] if v['position'][0] == sx0)
    u1 = next(v['uv'][0] for v in strip['vertices'] if v['position'][0] == sx1)
    cut_u = u0 + (cut - sx0)/(sx1 - sx0)*(u1 - u0)
    strip['vertices'] = [{'position': [cut, v['position'][1]], 'uv': [cut_u, v['uv'][1]]} if v['position'][0] < cut else v
                         for v in strip['vertices']]
    p0, p1 = min(min(ys(m)) for m in patch), max(max(ys(m)) for m in patch)
    t0 = max(max(ys(m)) for m in down) + 1
    t1 = max(ys(strip)) + 8
    patch = [moved(m, lambda y: t0 + (y - p0)*(t1 - t0)/(p1 - p0), ' (B row patch)') for m in patch]
    cx = (min(v['position'][0] for m in down for v in m['vertices']) + max(v['position'][0] for m in down for v in m['vertices']))/2
    cy = (min(ys(text)) + max(ys(text)))/2
    bx = [v['position'][0] for a in b_art for t in a['triangles'] for v in t]
    by = [v['position'][1] for a in b_art for t in a['triangles'] for v in t]
    dx, dy = cx - (min(bx) + max(bx))/2, cy - (min(by) + max(by))/2
    button = [{'texture': texture(a['texture']['rgba'], a['texture']['width'], a['texture']['height']), 'color': [1, 1, 1, 1],
               'vertices': [{'position': [v['position'][0] + dx, v['position'][1] + dy], 'uv': v['uv']}
                            for tri in a['triangles'] for v in tri],
               'order': text['order'], 'source': 'button_item Xbox_B (B row)'} for a in b_art]
    return [strip] + patch + button


def build(c):
    if '--marker' in sys.argv:  # just hud-skate1/session-marker(-tab)
        session_marker(c, skate3_hud())
        return
    if '--minimap' in sys.argv:  # just the minimap (runtime/minimap.json and its pages)
        install({'minimap': minimap(c)})
        check(['minimap'])
        return
    s3 = skate3_hud()
    manifests = {stem: movie(c, name, s3) for stem, name in MOVIES.items()}
    manifests['trickdisplay']['trick_stats'] = trick_stats()
    if '--trickdisplay' in sys.argv:  # just runtime/trickdisplay.json; the old one is kept as .pre-bests
        installed = OUT/'runtime/trickdisplay.json'
        backup = installed.with_suffix('.json.pre-bests')
        if installed.is_file() and not backup.exists():
            shutil.copyfile(installed, backup)
        install({'trickdisplay': manifests['trickdisplay']})
        check(['trickdisplay'])
        return
    manifests['minimap'] = minimap(c)
    install(manifests)
    check(list(manifests))
    session_marker(c, s3)


def probe(c):
    """One summary per candidate movie: size, root frames, and every text field (to match against the Xenia capture)."""
    for stem, name in CANDIDATES.items():
        try:
            bundle = c.load_bundle(name)
        except Exception as error:
            print(f'{stem:15s} FAILED {type(error).__name__}: {error}')
            continue
        chars = list(bundle['characters'].values())
        texts = [ch for ch in chars if ch['type_name'] == 'text']
        root = next((ch for ch in chars if ch['id'] == 0), {})
        kinds = {}
        for ch in chars:
            kinds[ch['type_name']] = kinds.get(ch['type_name'], 0) + 1
        print(f"{stem:15s} chars={len(chars):3d} root_frames={len(root.get('frames', [])):3d} {kinds}")
        for t in texts[:14]:
            print('    text', t.get('id'), repr(t.get('value') or t.get('initial_text') or t.get('text') or ''))


if __name__ == '__main__':
    c = cache(tuple(CANDIDATES.values()) + EXTRA)
    if '--probe' in sys.argv:
        probe(c)
        sys.exit(0)
    build(c)
