"""Skate 2's HUD for its converted maps: trick display, district title card and minimap.

Writes assets/private/hud-skate2/ (the engine picks it on the Skate 2 maps when Settings > Skate 2 HUD is on):
  runtime/trickdisplay.json, runtime/central.json  the same manifest tools/prepare_hud.py makes for Skate 3's
                                                   trickdisplay2, from Skate 2's hud2/trickdisplay2 and hud2/hudcentral
  session-marker/hud.json                          the phone list (hud2/hudphonelist) with its marker rows, flattened
                                                   for the engine's marker overlay (held marker button)
  runtime/minimap.json                             the minimap frame (hud2/hudminimap), its marker radius/pin and
                                                   the native marker icons (images/icons/minimap.rx2 mini_*, by name)
  assets/...                                       the RGBA payloads those reference

Skate 2's archives are classic BIGF (s2big), so they're fed to the vendored Skate 3 UI extractor through an
adapter; the movies, fonts (miscboot.big data/fe/fonts) and RW4 textures share Skate 3's formats. Text labels
come from Skate 2's own string table (s2lang), font aliases from its FontManager vault rows.

usage (numpy, PIL): py old_hud.py REPO SKATE2_ROOT ASSETS [--minimap | --trickdisplay]
  --minimap       rebuild only runtime/minimap.json and its pages
  --trickdisplay  rebuild only runtime/trickdisplay.json (its big-trick data); the old one is kept as .pre-bigtricks
  --phonelist     rebuild only session-marker/hud.json (the phone list)
"""
import hashlib, json, os, shutil, struct, sys, tempfile
from pathlib import Path

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/vendor/utt'), str(here)]
from s2big import Big
import s2lang
import vendor.skate3_ui.project as project
from vendor.skate3_ui.big import BigArchive
from vendor.skate3_ui.scene_graph import AssetCache, SceneFlattener
from vendor.skate3_ui.actions import Actions
from prepare_hud import font_mapping

OUT = assets/'private/hud-skate2'
WORK = WORK_ROOT/'hud-cache'
MOVIES = {'trickdisplay': 'data/fe/source/screens/hud2/trickdisplay2', 'central': 'data/fe/source/screens/hud2/hudcentral'}
PHONELIST = 'data/fe/source/screens/hud2/hudphonelist'
MINIMAP = 'data/fe/source/screens/hud2/hudminimap'
MARKERS = 'data/fe/source/images/icons/minimap'
WHITE = 'assets/white.rgba'
# Retail draws the phone list 1:1 (user 10-14, bug #3: the 0.6 chosen by eye looked wrong). Written to the manifest
# as authored_scale; session_marker/hud.rs undoes any other value.
PHONE_SCALE = 1.0


class S2Archive:
    """The vendored extractor's BigArchive interface over a Skate 2 BIGF archive."""
    _safe_relative = staticmethod(BigArchive._safe_relative)

    def __init__(self, source):
        self.big = Big(source)
        self.entries = self.big.entries

    def read(self, entry):
        return self.big.read(entry)


def cache():
    project.BigArchive = S2Archive
    project.find_big_directory = lambda root: Path(root)/'data'
    project.CORE_ARCHIVES = ('fe/screens.big', 'big/miscboot.big')
    prefixes = (*MOVIES.values(), PHONELIST, MINIMAP, 'data/fe/source/controls/button_item2', 'data/fe/source/images/buttons/xbox360')
    project.extract_project(s2game, WORK, prefixes=prefixes, force=True)
    return AssetCache(WORK)


def collections():
    """Skate 2's skater vault as the collections JSON prepare_hud.font_mapping reads (FontManager rows)."""
    from tools.asset_pipeline.vlt import convert
    db = Big(s2game/'data/db/db.big')
    with tempfile.TemporaryDirectory(prefix='skate2-db-') as work:
        for e in db.entries:
            if Path(e.path).stem == 'skater':
                (Path(work)/Path(e.path).name).write_bytes(db.read(e))
        names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
        data = convert(Path(work)/'skater', Path(work)/'skater', names)
    target = WORK/'skater-collections.json'
    target.write_text(json.dumps(data), encoding='utf-8')
    return target


# eScoreThresholdType_<name> in its value order: the "%d" globals the game registers for the movie (0x8250dd78), the
# order of the vault's threshold rows.
THRESHOLD_TYPES = ('ManualLength', 'GrindLength', 'PowerslideLength', 'HandplantHeight', 'GrabTime', 'AirTime',
                   'AirHorizontalDistance', 'AirHeight', 'AirStepUp', 'AirStepDown', 'SequenceSize', 'GrassGap',
                   'StairGap', 'HighGap', 'VeryHighGap')


def big_tricks(collections_json):
    """trickdisplay2's big-trick callouts (UpdateBigTrickDisplay): the threshold enums as movie globals, and the skater
    vault's Sk8::Score::ScoreThresholdDataSet (15 big-endian rows of u32 type, f32 big, f32 huge) as
    trick_stats.thresholds, [big, huge] by type. The engine's trick_stats.rs scores its measurements against them."""
    data = json.loads(Path(collections_json).read_text(encoding='utf-8'))
    rows = [f['data'] for c in data['collections'] for f in c['fields'].values()
            if isinstance(f, dict) and f.get('type') == 'Sk8::Score::ScoreThresholdDataSet']
    assert len(rows) == 1, f'{len(rows)} threshold tables'
    table = {t: [big, huge] for t, big, huge in struct.iter_unpack('>Iff', bytes.fromhex(rows[0]))}
    assert sorted(table) == list(range(len(THRESHOLD_TYPES))), sorted(table)
    names = {f'eScoreThresholdType_{n}': str(i) for i, n in enumerate(THRESHOLD_TYPES)}
    return {'globals': names | {'eScoreThreshold_Big': '0', 'eScoreThreshold_Huge': '1'},
            'trick_stats': {'thresholds': [table[t] for t in sorted(table)]}}


def movie(c, name, mappings, language):
    """prepare_hud.prepare's manifest for one movie: characters, flattened shapes, fonts, action blocks."""
    bundle = c.load_bundle(name)
    apt_path = WORK/'raw'/(name + '.apt')
    actions = Actions(apt_path.read_bytes(), apt_path.with_suffix('.const').read_bytes())
    blocks, shapes, fonts = {}, {}, {}
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
                # Skate 3's HUD has no solid fills; the engine draws every shape textured, so a solid one
                # samples a white texel and keeps its colour.
                if not p.get('texture'):
                    p['texture'] = {'width': 1, 'height': 1, 'rgba': WHITE}
                    for tri in p['triangles']:
                        for v in tri:
                            v['uv'] = [0.5, 0.5]
            shapes[str(ch['id'])] = scene['primitives']
        elif ch['type_name'] == 'font':
            fonts[ch['font']['name']] = c.font_asset(ch['font']['name'])
    missing = [f for f, asset in fonts.items() if asset is None]
    if missing:
        raise ValueError(f'{name}: fonts without a Skate 2 bitmap bank: {missing}')
    return {'format': 'skate3-scoring-hud', 'version': 1,
            'source': {'bundle': name, 'game': 'skate2', 'apt_sha256': hashlib.sha256(apt_path.read_bytes()).hexdigest()},
            'characters': list(bundle['characters'].values()), 'shapes': shapes, 'fonts': fonts, 'actions': blocks,
            'font_mappings': mappings, 'language': language, 'unresolved_fonts': []}


def payloads(manifest):
    """Every RGBA file a manifest references (shape textures, font pages, minimap pages), relative to the cache root."""
    if 'frame' in manifest:
        return {manifest['frame']['texture']} | {m['texture'] for m in manifest['markers'].values()}
    files = {p['texture']['rgba'] for prims in manifest['shapes'].values() for p in prims if p.get('texture')}
    return files | {f['texture'] for f in manifest['fonts'].values()}


def minimap(c):
    """Skate 2's minimap: hudminimap's mMiniMap frame (its 179 px shape over a 256 px page: the translucent disc, rings
    and arrow) and the native marker icons the game draws over it. Skate 2 draws no street map. Its marker code
    (default.xex 0x823763a0) puts a marker 1 px per metre from the centre (0x82353a48 turns the world with the
    skater's heading) and pins one further than 71.2811 (0x822671a8) on that radius; its icon is 32 px
    (0x8215ea74) times the marker's scale."""
    bundle = c.load_bundle(MINIMAP)
    root = bundle['characters'][next(k for k, ch in bundle['characters'].items() if ch['id'] == 0)]
    places = {p.get('name') or p['character_id']: p['matrix'] for p in root['frames'][0]['controls'] if p['type_name'] == 'place_object2'}
    textures = {Path(t['name']).name: t for t in json.loads((WORK/'assets'/MINIMAP/'manifest.json').read_text())['textures']}
    frame = textures['1.Texture']
    icons = markers()
    return {'frame': {'texture': f"assets/{MINIMAP}/{frame['rgba_file']}", 'size': [frame['width'], frame['height']],
                      'position': places['mMiniMap'][4:6], 'extent': 179.0},
            'centre': places[9][4:6],
            'scale': 1.0, 'pin': 71.2811,
            # ponytail: a marker fades 60 -> 350 m and hides past 1000 only when its record's flag (+60) says so;
            # which markers set it wasn't traced, so none fade (as before).
            'markers': icons,
            # ponytail: other players' dot; Skate 2's online minimap use of mini_skater wasn't traced.
            'skaters': [icons['mini_skater']]}


def markers():
    """The native minimap icons (screens.big images/icons/minimap.rx2, the 32 px mini_* cells; presentation event 0x1E
    names one, e.g. mini_teleport) as RGBA payloads: {name: {texture, size}}."""
    import numpy as np
    from vendor.skate3_ui.rw4 import read_textures
    from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant
    screens = Big(s2game/'data/fe/screens.big')
    entry = next(e for e in screens.entries if e.path.replace('\\', '/').endswith('images/icons/minimap.rx2'))
    folder = WORK/'assets'/MARKERS
    folder.mkdir(parents=True, exist_ok=True)
    (folder/'minimap.rx2').write_bytes(screens.read(entry))
    out = {}
    for t in read_textures(folder/'minimap.rx2')[1]:
        name, m = t.name.removesuffix('.Texture'), t.metadata
        if not name.startswith('mini_'):
            continue
        rgba = decode_rgba(t.data, parse_fetch_constant(bytes.fromhex(m['fetch_constant_hex'])))
        assert len(rgba) == m['width']*m['height']*4, name
        (folder/(name + '.rgba')).write_bytes(rgba)
        out[name] = {'texture': f'assets/{MARKERS}/{name}.rgba', 'size': [m['width'], m['height']]}
    return out


def phonelist(c, language):
    """Skate 2's phone list (hud2/hudphonelist) open on its main page, flattened into the session-marker overlay
    format (tools/extract_session_marker.py) the engine shows while the marker button is held. Rows as its
    UpdateMainMenu fills them: header ID_PHONELIST_TITLE, Go To Marker (D-pad up), Place Marker (D-pad down); the
    Contacts row is career-only, so its slot becomes the B row as Player List (role "players", in `player_list`, which
    the engine shows only online, as Skate 3's). The imported Button Item placeholders get the Skate 2 button art
    (their clip actions picked the button at runtime), centred on each placeholder."""
    from vendor.skate3_ui.scene_graph import transform_point
    from vendor.skate3_ui.bitmap_font import measure_bitmap_text
    name = PHONELIST
    frames = {0: 23, 76: 42, 75: 0, 47: 0, 48: 0}  # root after hudintro, mcQuickMenu maximized, main page, rows enabled
    texts = {'/mcTextHeader/': 'ID_PHONELIST_TITLE', '/mTextHelp0/': 'ID_PHONELIST_GOTO_MARKER', '/mTextHelp1/': 'ID_PHONELIST_PLACE_MARKER',
             '/mTextHelp2/': '#Player List'}
    buttons = {'/depth_44/': 'button_L_Bumper.Texture', '/mButton0/': 'button_DPad_Up.Texture', '/mButton1/': 'button_DPad_Down.Texture',
               '/depth_28/': 'button_B.Texture'}
    players_row = ('/mTextHelp2/', '/depth_28/')
    players = []

    def state(path, owner, ch):
        if ch['type_name'] == 'text':
            key = next((k for k in texts if k in path), None)
            return {'text': texts[key]} if key else {}
        if ch['type_name'] in ('sprite', 'animation'):
            if owner['key'] == name and ch['id'] in frames:
                return {'frame': frames[ch['id']]}
            if ch['movie']['frame_count'] <= 1:
                return {'frame': 0}
        return {}

    flat = SceneFlattener(c, state).flatten(name, 0)
    if flat['unresolved']:
        raise ValueError(f'phone list: {flat["unresolved"]}')
    out = OUT/'session-marker'
    out.mkdir(parents=True, exist_ok=True)
    textures, ids, meshes = [], {}, []

    def texture(path, width, height):
        if path not in ids:
            data = (WORK/path).read_bytes()
            assert len(data) == width*height*4, path
            ids[path] = len(textures)
            (out/f'texture-{len(textures)}.rgba').write_bytes(data)
            textures.append({'file': f'texture-{len(textures)}.rgba', 'width': width, 'height': height, 'source': str(path)})
        return ids[path]

    bank = Path('assets/data/fe/source/images/buttons/xbox360/buttons')
    art = {t['name']: t for t in json.loads((WORK/bank/'manifest.json').read_text())['textures']}
    for p in flat['primitives']:
        path = p['path']
        if not p.get('texture') and 'mButtonRender' not in path:
            continue  # a solid debug rect at the root
        vertices = [dict(position=v['position'], uv=v.get('uv', [0, 0])) for tri in p['triangles'] for v in tri]
        index = texture(p['texture']['rgba'], p['texture']['width'], p['texture']['height']) if p.get('texture') else None
        role = 'art'
        if 'mButtonRender' in path:
            key = next(k for k in buttons if k in path)
            item = art[buttons[key]]
            index = texture(bank/item['rgba_file'], item['width'], item['height'])
            # As Skate 3's RenderButton: the whole texture, centred on the placeholder.
            xs, ys = [v['position'][0] for v in vertices], [v['position'][1] for v in vertices]
            cx, cy = (min(xs) + max(xs))/2, (min(ys) + max(ys))/2
            x0, y0, x1, y1 = cx - item['width']/2, cy - item['height']/2, cx + item['width']/2, cy + item['height']/2
            q = [((x0, y0), (0, 0)), ((x1, y0), (1, 0)), ((x1, y1), (1, 1)), ((x0, y1), (0, 1))]
            vertices = [dict(position=list(q[i][0]), uv=list(q[i][1])) for i in (0, 1, 2, 0, 2, 3)]
            p['color'] = [1, 1, 1, p['color'][3]]
            role = {'/mButton0/': 'return', '/mButton1/': 'place'}.get(key, 'art')
        row = any(d in path for d in players_row)
        (players if row else meshes).append({'texture': index, 'role': 'players' if row else role, 'color': p['color'],
                                             'vertices': vertices, 'order': p['draw_order'], 'source': path})
    for t in flat['text']:
        value = language.get(t['value'], t['value']).lstrip('#')
        font = t['font_asset']
        d = font['definition']
        size = d['textures'][0]
        index = texture(font['texture'], size['width'], size['height'])
        glyphs = {g['glyph_index']: g for g in d['glyphs']}
        metrics = measure_bitmap_text(d, value, t['font_height'])
        vertices = []
        for used in metrics['glyphs']:
            g = glyphs[used['glyph_index']]
            s = metrics['scale']
            x = t['bounds'][0] + 2 + used['left']
            y = t['bounds'][1] + 2 + t['font_height'] - g['y_offset'] * s
            w, h = g['width'] * s, g['height'] * s
            a, b, cc, dd = g['atlas_bounds']
            corners = [(x, y, a, b), (x + w, y, cc, b), (x + w, y + h, cc, dd), (x, y + h, a, dd)]
            for i in (0, 1, 2, 0, 2, 3):
                px, py, u, v = corners[i]
                vertices.append({'position': transform_point(t['matrix'], [px, py]), 'uv': [u/size['width'], v/size['height']]})
        argb = int(t['color_argb'].lstrip('#'), 16)
        colour = [((argb >> n) & 255)/255 for n in (16, 8, 0)] + [t['alpha']*((argb >> 24) & 255)/255]
        role = 'return' if '/mTextHelp0/' in t['path'] else 'place' if '/mTextHelp1/' in t['path'] else 'art'
        row = any(d in t['path'] for d in players_row)
        (players if row else meshes).append({'texture': index, 'role': 'players' if row else role, 'color': colour, 'vertices': vertices,
                                             'order': t['draw_order'], 'source': t['path']})
    if not any(m['texture'] is not None for m in players):
        raise ValueError('phone list: the Contacts row (the Player List slot) drew nothing')
    for m in meshes + players:
        for v in m['vertices']:
            v['position'] = [v['position'][0] * PHONE_SCALE, v['position'][1] * PHONE_SCALE]
    manifest = {'version': 1, 'canvas': [1280, 720], 'authored_scale': PHONE_SCALE, 'textures': textures, 'meshes': sorted(meshes, key=lambda m: m['order']),
                'player_list': sorted(players, key=lambda m: m['order']),
                'notes': "Skate 2's hudphonelist main page, flattened by skate2port/old_hud.py."}
    (out/'hud.json').write_text(json.dumps(manifest, indent=1) + '\n', encoding='utf-8')
    print('phone list', len(meshes), 'meshes,', len(textures), 'textures ->', out/'hud.json')


def install(manifests):
    OUT.mkdir(parents=True, exist_ok=True)
    (WORK/WHITE).write_bytes(b'\xff'*4)
    for stem, manifest in manifests.items():
        for rel in payloads(manifest):
            (OUT/rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(WORK/rel, OUT/rel)
        (OUT/'runtime').mkdir(exist_ok=True)
        (OUT/'runtime'/(stem + '.json')).write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
        print(stem, len(payloads(manifest)), 'payloads')


def check(stems=(*MOVIES, 'minimap')):
    """Re-read what was installed: every referenced payload exists at its declared size (as install_prepared_hud does)."""
    for stem in stems:
        manifest = json.loads((OUT/'runtime'/(stem + '.json')).read_text(encoding='utf-8'))
        sizes = {}
        if stem == 'minimap':
            sizes = {manifest['frame']['texture']: manifest['frame']['size']}
            sizes |= {m['texture']: m['size'] for m in (*manifest['markers'].values(), *manifest['skaters'])}
        else:
            sizes = {p['texture']['rgba']: [p['texture']['width'], p['texture']['height']]
                     for prims in manifest['shapes'].values() for p in prims if p.get('texture')}
            sizes |= {f['texture']: [f['definition']['textures'][0]['width'], f['definition']['textures'][0]['height']]
                      for f in manifest['fonts'].values()}
        for rel, (w, h) in sizes.items():
            assert (OUT/rel).stat().st_size == w*h*4, f'{stem}: {rel} is not {w}x{h} RGBA'
    print('check ok:', OUT)


if __name__ == '__main__':
    c = cache()
    if '--minimap' in sys.argv:
        install({'minimap': minimap(c)})
        check(('minimap',))
        sys.exit(0)
    vault = collections()
    mappings = font_mapping(vault, c)
    # Tricks Skate 2 never had (darkcatches, caveman, acid drop...) still reach its HUD as Skate 3 labels, so
    # its strings fall back to the installed Skate 3 HUD's; Skate 2's own wording wins where both have one.
    stock = assets/'private/hud/runtime/trickdisplay.json'
    fallback = json.loads(stock.read_text(encoding='utf-8'))['language'] if stock.is_file() else {}
    if not fallback:
        print('warning: no Skate 3 HUD strings at', stock, '- Skate 3-only tricks will show their labels')
    language = fallback | s2lang.strings(s2game)
    if '--phonelist' in sys.argv:
        phonelist(c, language)
        sys.exit(0)
    manifests = {stem: movie(c, name, mappings, language) for stem, name in MOVIES.items()}
    manifests['trickdisplay'] |= big_tricks(vault)
    if '--trickdisplay' in sys.argv:
        installed = OUT/'runtime/trickdisplay.json'
        backup = installed.with_suffix('.json.pre-bigtricks')
        if installed.is_file() and not backup.exists():
            shutil.copyfile(installed, backup)
        install({'trickdisplay': manifests['trickdisplay']})
        check(('trickdisplay',))
        sys.exit(0)
    manifests['minimap'] = minimap(c)
    install(manifests)
    phonelist(c, language)
    check()
