"""Skate 2's tutorial and challenge screens (screens.big) for the engine's apt_screen runtime.

Writes assets/private/skate2-tutorial/screens/:
  runtime/<stem>.json   the hud_runtime manifest old_hud.movie() makes (characters, shapes, fonts, action blocks), with
                        imported controls merged in as tools/prepare_core_menu.py does (offset ids, init_order, exports)
  assets/...            the RGBA payloads those reference
  natives.json          per movie, the native objects/methods its ActionScript names (what apt_screen must answer)
  trick_guides.json     {prompt gesture (presentation event 0x14): [gesture, button, connector, ...]}, what
                        HUDComponents.GetTutorialGesturesButtons hands hudtutorial's FillTrickData

usage (numpy, PIL): py old_screens.py REPO SKATE2_ROOT ASSETS
"""
import json, re, sys
from pathlib import Path

import old_hud  # same argv: REPO SKATE2_ROOT ASSETS
from old_hud import Big, S2Archive, project, AssetCache, SceneFlattener, Actions, font_mapping, collections, s2lang, payloads
from prepare_core_menu import _bundles, _init_order, _parameters, BUNDLE_STRIDE, ACTION_SHIFT

S = 'data/fe/source/screens/'
MOVIES = {
    'hudtutorial': S + 'hud2/hudtutorial',
    'tutorial': S + 'hud2/tutorial',
    'hudobjective2': S + 'hud2/hudobjective2',
    # HUD2.ShowCentralMessage (CentralHUD): "Challenge Complete!" and the other big centre lines.
    'hudcentral': S + 'hud2/hudcentral',
    'hudtimer': S + 'hud2/hudtimer',
    'huddistance': S + 'hud2/huddistance',
    'challenge_objectives': S + 'challenge/challenge_objectives',
    'challenge_signup': S + 'challenge/challenge_signup',
    'challengefailed_small': S + 'hudComponents/challengefailed_small',
    'endchallenge_small': S + 'hudComponents/endchallenge_small',
    'endchallengeoptions': S + 'hudComponents/endchallengeoptions',
    'subtitle': S + 'main/subtitle',
    'letterbox': S + 'main/letterbox',
    'nis_overlay': S + 'nis/nis_overlay',
    # The OVERLAY cutscene track's TutorialGesture_OlliePt1/Pt2 (ids 80/81): SetState(1) olliedown, SetState(2) ollieup.
    'gestollie': S + 'nis/gestollie',
    'skate101': S + 'tutorials/skate101',
    'sk8popup': S + 'popup/sk8popup',
    'small_popup': S + 'popup/small_popup',
    'unlock': S + 'hudComponents/unlock',
    'unlockoverlay': S + 'unlock/unlockoverlay',
    # Photo challenges, by default.xex's screen table: PhotoSelect (FrontEnd.OpenPhotoSelection), PhotoTreatment,
    # CountdownHUD, HighlightReelHUD, SpotKilled (challenge/killedit; read as the OVERLAY KilledIt (70) screen).
    'photoselect': S + 'challenge/photoselect',
    'phototreatment': S + 'hudComponents/phototreatment',
    'hudcountdown': S + 'hud2/hudcountdown',
    'highlight_reel_overlay': S + 'hud2/highlight_reel_overlay',
    'killedit': S + 'challenge/killedit',
    # SignUpHUD (screen 91): the "Sign up for: <title>" prompt in a lesson's ±3 m sign-up box.
    'actionoverlay2': S + 'hud2/actionoverlay2',
}
# Every control bundle a screen may import (button menus, pickers, icons) and the button art they show.
SHARED = ('data/fe/source/controls/', 'data/fe/source/images/buttons/')
OUT = old_hud.assets/'private/skate2-tutorial/screens'
WORK = old_hud.WORK_ROOT/'screens-cache'
# old_hud's helpers read and write under its WORK/OUT.
old_hud.WORK, old_hud.OUT = WORK, OUT
# button_item2 names its glyph in `_test`; the engine draws manifest `buttons` (Skate 2's xbox360 button bank) there.
BUTTONS = Path('assets/data/fe/source/images/buttons/xbox360/buttons')
_payloads = old_hud.payloads
old_hud.payloads = lambda m: _payloads(m) | {t['rgba'] for t in m.get('buttons', {}).values()}


def buttons():
    art = json.loads((WORK/BUTTONS/'manifest.json').read_text())['textures']
    # Each button is a 32 px disc centred in the 64 px texture's top-left 48 px cell (a pad's direction arrows reach
    # its edges): button_item2's 32 px quad is that disc, the arrows drawn past it.
    return {t['name'].removeprefix('button_').removesuffix('.Texture'):
            {'rgba': (BUTTONS/t['rgba_file']).as_posix(), 'width': t['width'], 'height': t['height'],
             'frame': [8, 8, 40, 40]} for t in art}


def cache():
    project.BigArchive = S2Archive
    project.find_big_directory = lambda root: Path(root)/'data'
    project.CORE_ARCHIVES = ('fe/screens.big', 'big/miscboot.big')
    project.extract_project(old_hud.s2game, WORK, prefixes=(*MOVIES.values(), *SHARED), force=True)
    return AssetCache(WORK)


# ponytail: Skate 1-era font names some Skate 2 screens still name, with no bitmap bank on the Skate 2 disc
# (its FontManager has none); Skate 2's nearest faces stand in.
# Name -> (font family, the FontManager AptName whose native layout it borrows).
STAND_INS = {'TradeBld': ('Helvetica LT Std', 'Helvetica LT Std'), 'TradeBld Stroke': ('Helvetica LT Std', 'Helvetica LT Std'),
             'NewCourierBlot': ('markerstreak', 'Marker_streak'), 'NewCourierBlotStroke': ('markerstreak', 'Marker_streak'),
             # actionoverlay2 names Marker_streak in lower case.
             'marker_streak': ('markerstreak', 'Marker_streak')}


class Present:
    """The cache with imports from bundles the disc doesn't ship (controls/game_icons) left out."""
    def __init__(self, c):
        self.c = c

    def __getattr__(self, name):
        return getattr(self.c, name)

    def absent(self, file):
        try:
            self.c.load_bundle(file)
            return False
        except Exception:
            return True

    def load_bundle(self, key):
        b = self.c.load_bundle(key)
        return b | {'imports': {i: v for i, v in b['imports'].items() if not self.absent(v['file'])}}


def movie(c, name, mappings, language):
    """old_hud.movie's manifest with `name`'s imports merged in (prepare_core_menu.prepare's scheme): each bundle's
    ids offset by BUNDLE_STRIDE, imported placeholders resolved to the exporter, action blocks keyed
    (bundle << ACTION_SHIFT) | offset, construct-only clip events turned into placement `init` values."""
    present = Present(c)
    bundles = _bundles(present, name)
    index = {key: i for i, key in enumerate(bundles)}
    stubs = set()

    def gid(key, local):
        imported = c.load_bundle(key)['imports'].get(local)
        if imported and present.absent(imported['file']):
            # ponytail: Skate 2's disc has no such bundle anywhere; the placement shows nothing (an empty clip).
            stubs.add(index[key] * BUNDLE_STRIDE + local)
            return index[key] * BUNDLE_STRIDE + local
        owner, ch = c.resolve_character(key, local)
        return index[owner['key']] * BUNDLE_STRIDE + ch['id']

    characters, shapes, fonts, blocks, init, exports, scoped, dropped = [], {}, {}, {}, {}, {}, {}, []
    for key in bundles:
        bundle = c.load_bundle(key)
        for export, local in bundle['exports'].items():
            exports.setdefault(export, gid(key, local))
            scoped.setdefault(str(index[key]), {})[export] = gid(key, local)
        raw = WORK/'raw'/(key + '.apt')
        apt = raw.read_bytes()
        acts = Actions(apt, raw.with_suffix('.const').read_bytes())
        for local, ch in bundle['characters'].items():
            ch = json.loads(json.dumps(ch))
            ch['id'] = index[key] * BUNDLE_STRIDE + local
            for f in ch.get('frames', []):
                for control in f['controls']:
                    place = control['type_name'].startswith('place_object')
                    if place and control.get('flags', 0) & 2:
                        control['character_id'] = gid(key, control['character_id'])
                    if place and control.get('flags', 0) & 0x80:
                        values = _parameters(apt, acts, control['actions_offset'])
                        if values is None:
                            dropped.append({'bundle': key, 'character': ch['id'], 'depth': control.get('depth')})
                        else:
                            control['init'] = values
                        control['flags'] &= ~0x80
                        control['actions_offset'] = 0
                        continue
                    offset = control.get('actions_offset', 0)
                    if control['type_name'] in ('do_action', 'do_init_action') and offset:
                        control['actions_offset'] = (index[key] << ACTION_SHIFT) | offset
                        blocks[str(control['actions_offset'])] = acts.stream(offset)
                        if control['type_name'] == 'do_init_action':
                            init.setdefault(key, []).append(control['actions_offset'])
            if ch.get('text') and ch['text'].get('font_id') is not None:
                ch['text']['font_id'] = gid(key, ch['text']['font_id'])
            if ch['type_name'] == 'shape':
                scene = SceneFlattener(c, lambda *_: {}).flatten(key, local)
                if scene['unresolved']:
                    raise ValueError(f'{key}: unresolved shape {scene["unresolved"]}')
                for p in scene['primitives']:
                    if not p.get('texture'):  # solid fill: a white texel keeps its colour (as old_hud.movie)
                        p['texture'] = {'width': 1, 'height': 1, 'rgba': old_hud.WHITE}
                        for tri in p['triangles']:
                            for v in tri:
                                v['uv'] = [0.5, 0.5]
                shapes[str(ch['id'])] = scene['primitives']
            elif ch['type_name'] == 'font':
                fonts[ch['font']['name']] = c.font_asset(ch['font']['name'])
            characters.append(ch)
    characters += [{'id': i, 'type': 5, 'type_name': 'sprite', 'movie': {'frame_count': 0}, 'frames': []} for i in sorted(stubs)]
    if stubs:
        print(name, 'imports absent from the disc, left empty:', sorted(stubs))
    missing = [f for f, asset in fonts.items() if asset is None]
    if missing:
        raise ValueError(f'{name}: fonts without a Skate 2 bitmap bank: {missing}')
    if dropped:
        print(name, 'dropped clip events (not construct parameters):', dropped)
    return {'format': 'skate3-scoring-hud', 'version': 1, 'source': {'bundles': bundles, 'game': 'skate2'},
            'bundles': bundles, 'characters': characters, 'shapes': shapes, 'fonts': fonts, 'actions': blocks,
            'init_order': [o for key in _init_order(present, bundles) for o in init.get(key, [])],
            'exports': exports, 'scoped_exports': scoped, 'dropped_clip_actions': dropped, 'buttons': buttons(),
            'font_mappings': mappings, 'language': language, 'unresolved_fonts': []}


# Sk8::FE::tTrickGuideInputSlot ids -> the names hudtutorial's gesture_item / button_item2 take (default.xex .data
# 0x82d79368 buttons and 0x82d793e0 gestures; connectors are .rdata "Plus", "Arrow", "Or").
GUIDE_BUTTONS = ('', 'Back', 'Start', 'A', 'B', 'Y', 'X', 'R_Trigger', 'L_Trigger', 'L_Bumper', 'R_Bumper', 'L3_Click',
                 'R3_Click', 'Stick_Right_Up', 'Stick_Right_Down', 'Stick_Right_L', 'Stick_Right_R', 'Stick_Right',
                 'DPad_Up', 'DPad_Down', 'DPad_Left', 'DPad_Right', 'Stick_Left_Up', 'Stick_Left_Down', 'Stick_Left_L',
                 'Stick_Left_R', 'Stick_Left', 'DPad')
GUIDE_GESTURES = ('', 'grind_5_0_Grind', 'grind_fs_5_0', 'grind_fs_bluntslide', 'grind_fs_boardslide', 'grind_fs_crooks',
                  'grind_fs_lipslide', 'grind_fs_noseblunt', 'grind_fs_overcrooks', 'grind_fs_salad', 'grind_fs_noseslide',
                  'grind_fs_tailslide', 'grind_nose_grind', 'grind_fs_feeble', 'grind_fs_smith', 'grind_fs_willie',
                  'grind_fs_overwillie', 'nollie', 'nollie_360_flip', 'nollie_360_hardflip', 'nollie_360_heelflip',
                  'nollie_360_inward_heel', 'nollie_360popshuvit', 'nollie_fs_360popshuvit', 'nollie_fs_popshuvit',
                  'nollie_hardflip', 'nollie_heelflip', 'nollie_inward_heelflip', 'nollie_kickflip', 'nollie_popshuvit',
                  'nollie_varial_flip', 'nollie_varial_heelflip', 'ollie', 'ollie_360_flip', 'ollie_360_hardflip',
                  'ollie_360_heelflip', 'ollie_360_inward_heelflip', 'ollie_360popshuvit', 'ollie_fs_360popshuvit',
                  'ollie_fs_popshuvit', 'ollie_hardflip', 'ollie_heelflip', 'ollie_inward_heelflip', 'ollie_kickflip',
                  'ollie_popshuvit', 'ollie_varial_heelflip', 'ollie_varial_kickflip')
GUIDE_CONNECTORS = ('', 'Plus', 'Arrow', 'Or')
GUIDE_FIELD = 'Hash_03ACC9F200807B87'  # tTrickGuideInputSlot[] on the vault's trick rows


def trick_guides(vault):
    """The guide slots of every gesture the tutorial's prompts name, as hudtutorial's flat name list."""
    from tools.asset_pipeline.vlt import hash64
    rows = {}
    for c in json.loads(Path(vault).read_text(encoding='utf-8'))['collections']:
        if GUIDE_FIELD in c['fields']:
            rows.setdefault(c['key'], c)
    tutorial = (OUT.parent/'tutorial.json').read_text(encoding='utf-8')
    guides = {}
    for name in sorted(set(re.findall(r'"type": 20, "a": "([^"]+)"', re.sub(r'\s+', ' ', tutorial)))):
        row = rows.get('Hash_%016X' % hash64(name))
        if row is None:
            continue
        flat = []
        for item in row['fields'][GUIDE_FIELD]['array']['items']:
            gesture, button, connector = (int(item[i:i + 8], 16) for i in (0, 8, 16))
            if gesture or button:
                flat += [GUIDE_GESTURES[gesture], GUIDE_BUTTONS[button], GUIDE_CONNECTORS[connector]]
        guides[name] = flat
    OUT.mkdir(parents=True, exist_ok=True)  # old_tutorial.py's rmtree takes screens/ with it
    (OUT/'trick_guides.json').write_text(json.dumps(guides, indent=1), encoding='utf-8')
    return guides


IDENT = re.compile(r'^[A-Za-z_][A-Za-z0-9_]*$')


def natives(manifest):
    """Identifiers the movie's action blocks push: candidates for the native objects and methods it calls."""
    names = set()

    def walk(v):
        if isinstance(v, str):
            if IDENT.match(v) and len(v) > 2:
                names.add(v)
        elif isinstance(v, dict):
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)
    walk(manifest['actions'])
    return sorted(names)


if __name__ == '__main__':
    c = cache()
    vault = collections()
    mappings = font_mapping(vault, c)
    trick_guides(vault)
    for name, (family, apt) in STAND_INS.items():
        c.add_font_alias(name, family, mappings[apt])
        mappings[name] = mappings[apt] | {'stand_in': apt}
    language = s2lang.strings(old_hud.s2game)
    manifests, failed = {}, {}
    for stem, name in MOVIES.items():
        try:
            manifests[stem] = movie(c, name, mappings, language)
        except Exception as error:  # one broken movie must not stop the rest
            failed[stem] = repr(error)
            print('FAILED', stem, error)
    old_hud.install(manifests)
    (OUT/'natives.json').write_text(json.dumps({s: natives(m) for s, m in manifests.items()}, indent=1), encoding='utf-8')
    for stem, manifest in manifests.items():
        sizes = {p['texture']['rgba']: [p['texture']['width'], p['texture']['height']]
                 for prims in manifest['shapes'].values() for p in prims if p.get('texture')}
        for rel, (w, h) in sizes.items():
            assert (OUT/rel).stat().st_size == w*h*4, f'{stem}: {rel} is not {w}x{h} RGBA'
    print(len(manifests), 'screens ->', OUT, '; failed:', failed or 'none')
