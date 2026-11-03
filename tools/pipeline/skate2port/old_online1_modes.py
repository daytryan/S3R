"""Skate 1's online modes at run time: what skate1_online.rs reads besides old_online1.py's rows and intros.

Writes into ASSETS/private/ (an export dir, the one old_online1.py wrote; --install copies into an installation):
  hud-skate1/online/runtime/centralhud.json + payloads  hudbase's central messenger (round / countdown / congrats /
                                 bottom messages), old_hud1's manifest format; the mode overlays drive it through hudbase
  hud-skate1/online/runtime/skatetrick.json + payloads  the S.K.A.T.E. overlay's trick component (set / copied trick,
                                 the already-been-done list), with the overlay's labels
  hud-skate1/online/runtime/{leaderboard,racehud,spectator_mode}.json  old_online1's (and skatetrick), rebuilt with
                                 their imported controls merged in (menupicker, button_item, game_icons, trick_gestures;
                                 ids, exports, init_order as prepare_core_menu); clip events become placement init values
  hud-skate1/online/sfx/<Name>.wav  every Audio.Play<Name>() of the online movies that plays a 1000 / 2000 menu
                                 sound (old_screens1.audio, the tutorial's rule)
  skate1-online/gates/           gates.json + <model>.png: the RaceGate (rg000) and SpotGate (sg000) models the races
                                 draw on their way points (old_online1_gates.py, from the disc's visual_treatments.rx2)
  skate1-online/modes.json       the Skate 1 English skate1_online.rs formats itself: the mode titles
                                 (ID_MISSION_TEMPLATE_*, Mission.GetRaceHUDTitle), the event-log lines
                                 HUD.AddChallengeStart/Finish/WinnerMessage add, the timer and leaderboard timer formats,
                                 the race HUDs' lap fields and Spot Race's objectives (strings default.xex looks up by
                                 hash or by its tables), plus the eSk8HUD* enums the Lua reads
--install INST_ASSETS also copies skate1-online/scripts/*.lua (old_online1.py's decompiled handlers, which the engine
  runs) and the outputs above into the installation. A file that is already there and differs is backed up once as
  <file>.pre-online1modes before it is replaced. None has an installation.json receipt.

Evidence: ONLINE1_MODES_REPORT.md.
usage (numpy, PIL): py -3 old_online1_modes.py REPO SKATE1_ROOT ASSETS [--install INST_ASSETS]
"""
import json, os, re, shutil, sys
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
install_to = Path(sys.argv[sys.argv.index('--install') + 1]) if '--install' in sys.argv else None
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(here)]
argv, sys.argv = sys.argv, ['old_hud1.py', str(repo), str(s1game), str(install_to or assets)]
try:
    import old_hud1 as hud1
finally:
    sys.argv = argv
from s2big import Big
from s2lang import table

OUT = assets/'private/skate1-online'
HUD_OUT = assets/'private/hud-skate1/online'
SCREENS = 'data/fe/source/screens/'
CENTRAL = SCREENS + 'hudComponents/centralhud'
# The overlays whose SetState skate1_online ports (their labels go into centralhud's language: hudbase shows them there).
OVERLAYS = ['hud/hud_besttrick_challenge_overlay', 'hud/hud_jam_challenge_overlay', 'hud/hud_spot_challenge_overlay',
            'hud/hud_deathrace/hud_deathrace_challenge_overlay', 'hud/hud_spotrace_challenge_overlay',
            'hud/hud_skate/hud_skate_challenge_overlay', 'hudComponents/onlinehud', 'hudComponents/racehud',
            'hudComponents/skatehud', 'hudComponents/spectator_mode']
GFN = {'TradeBld': 'tradebld', 'NewCourierBlot': 'indytype', 'Bit KitBold': 'bitkitbold',
       'Raven Stroke': 'tradebldstroke',  # old_online1's
       # menupicker's Times New Roman "M" is not in the exe's table: 0x827535b8 falls back to index 0, Bit KitBold.
       'Times New Roman': 'bitkitbold'}
# Mission.GetRaceHUDTitle's title per mode (0x825f4b40 / 0x82371050) and the mode names the event log fills in.
TITLES = {'skate': 'ID_MISSION_TEMPLATE_SKATE_TITLE', 'best_trick': 'ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK',
          'spot_battle': 'ID_MISSION_TEMPLATE_ONLINE_SPOTBATTLE', 'jam': 'ID_MISSION_TEMPLATE_ONLINE_JAM',
          'spot_race': 'ID_MISSION_TEMPLATE_SPOT_RACE_TITLE', 'death_race': 'ID_MISSION_TEMPLATE_ONLINE_DEATHRACE',
          'freeskate': 'ID_SIDEKICK_ONLINE_MENU_FREESKATE'}
# Strings default.xex formats by hash (no label in its tables): key -> label_key hash.
HASHED = {'started': 0xe2e8f5c0,      # HUD.AddChallengeStartMessage (0x827b9..): "{0:s} has started!"
          'finished': 0x20f48ad3,     # HUD.AddChallengeFinishMessage
          'won': 0x489c5105,          # HUD.AddChallengeWinnerMessage(index)
          'seconds_short': 0xfcd52836,  # GetTimerHUDText (0x82883d40): under a minute, seconds < 10
          'seconds': 0x0b896f95,      # under a minute
          'lobby': 0xfba8fa98,        # FELeaderboard.GetTimerText: the post-match countdown
          'moving': 0x2a8b6f15,       # the same while the session is moving
          'gates_missed': 0x0d7be47c,  # Death Race's GetRaceHUDLapField (0x82342de8): missed / MaxAllowed + 1
          'spots': 0x95e1e19a,        # Spot Race's (0x82346d18): next way point / way points - 1
          'rounds': 0xfb0771ae,       # Spot Battle's (0x82342308): current round / vault NumRounds
          'finish_line': 0xfae5932b,  # Mission.GetSpotRaceObjective (0x82346ed0) on trick type None
          'lobby_title': 0xdbb3ef87}  # the free-skate mission's GetRaceHUDTitle in a challenge session (0x82347148)
# GetSpotRaceObjective's other lines: points > 0 "Get {0:d} Points", else the trick type's singular objective title
# (0x830a3f68's {category, title, titles} rows, by Trick.eTrickType) with 1; types without a title are null.
POINTS = 'ID_OBJECTIVE_TITLE_GET_POINTS'
TRICK_TITLES = [None, None, 'FLIP', 'GAP', 'GRAB', 'GRIND', None, None, 'MANUAL', None, 'SPIN', 'REVERT']
LABELS = ['ID_MISSION_ROUND_NOTIFICATION', 'ID_MISSION_COUNTDOWN_GO', 'ID_MISSION_ROUND_COMPLETE',
          'ID_MISSION_CONTEST_FAILED', 'ID_MISSION_CONTEST_COMPLETE', 'ID_MISSION_CHALLENGE_FORFEIT',
          'ID_COMMON_MISSION_LEFT_AREA_WARNING']

text = {}
english_big = Big(s1game/'data/big/english.big')
for e in english_big.entries:
    if e.path.lower().endswith('.bin') and 'histogram' not in e.path.lower():
        text.update(table(english_big.read(e)))


def label_key(s):
    x = 0xFFFFFFFF
    for c in s.encode():
        x = (x*33 + c) & 0xFFFFFFFF
    return x


def english(label):
    return text.get(label_key(label))


# ---------------------------------------------------------------- centralhud (old_hud1's pipeline)
hud1.WORK, hud1.OUT = WORK_ROOT/'online1-central-cache', HUD_OUT
hud1.GFN.update(GFN)
cache = hud1.cache((CENTRAL,) + hud1.EXTRA)
ids = set(LABELS)
for name in [CENTRAL] + [SCREENS + o for o in OVERLAYS]:
    for root in (hud1.WORK, WORK_ROOT/'online1-hud-cache'):
        for suffix in ('.apt', '.const'):
            raw = root/'raw'/(name + suffix)
            if raw.is_file():
                ids |= {m.decode() for m in re.findall(rb'ID_[A-Z0-9_]{3,}', raw.read_bytes())}
missing = sorted(i for i in ids if not english(i))
s3 = {'language': {i: english(i) for i in sorted(ids) if english(i)}}
manifest = hud1.movie(cache, CENTRAL, s3)
hud1.install({'centralhud': manifest})
hud1.check(['centralhud'])
print('centralhud fonts:', sorted(manifest['fonts']), '; labels without English:', missing)

# ---------------------------------------------------------------- the Skate overlay's components
# hud_skate_challenge_overlay registers skatehud (old_online1's) and skatetrick (the set / copied trick and the
# already-been-done list) as its UI components; skatetrick is exported here, with the overlay's labels.
SKATETRICK = SCREENS + 'hudComponents/skatetrick'
hud1.WORK = WORK_ROOT/'online1-skatetrick-cache'
skate_cache = hud1.cache((SKATETRICK,) + hud1.EXTRA)
for name in [SKATETRICK, SCREENS + 'hud/hud_skate/hud_skate_challenge_overlay']:
    for root in (hud1.WORK, WORK_ROOT/'online1-hud-cache'):
        raw = root/'raw'/(name + '.apt')
        if raw.is_file():
            ids |= {m.decode() for m in re.findall(rb'ID_[A-Z0-9_]{3,}', raw.read_bytes() + raw.with_suffix('.const').read_bytes())}
# Its language (capitals) and fonts; its characters are merged with its imports below.
skatetrick = hud1.movie(skate_cache, SKATETRICK, {'language': {i: english(i) for i in sorted(ids) if english(i)}})


# ---------------------------------------------------------------- clip events
# apt_display places no clip events (it refuses a placement that has them). The online movies' are all
# construct / initialize-time `name = value` lists (leaderboard's game icon and menu picker, racehud's badge,
# spectator_mode's pad buttons): turned into the placement's `init` values, as prepare_core_menu does for Skate 3's
# menus. Values are text (the engine's init is text): a number becomes its decimal text.
# ponytail: numbers set as text; nothing in these movies adds to them (the menu picker's are read by a picker the
# online leaderboard never shows).
from vendor.skate3_ui.actions import Actions
import struct
CONSTRUCT, INITIALIZE = 0x40000, 0x200   # APT's clip event flags (SWF ClipEventFlags bytes, little-endian)


def clip_values(apt, acts, offset):
    """{name: text} of a clip event list of only construct / initialize-time sets, else None."""
    count, pointer = struct.unpack_from('>II', apt, offset)
    out = {}
    for i in range(count):
        flags, _, at = struct.unpack_from('>III', apt, pointer + 12*i)
        if flags & ~(CONSTRUCT | INITIALIZE):
            return None
        stack = []
        for row in acts.stream(at):
            op, value = row['opcode'], row.get('operand')
            if op in (0xA1, 0xB5):              # push string / byte
                stack.append(value)
            elif op == 0xB4:                    # push float (its bits)
                stack.append(round(struct.unpack('>f', struct.pack('>I', value))[0], 6))
            elif op == 0xA6:                    # set the pushed name to a constant string
                out[stack.pop()] = value
            elif op == 0x1D:                    # setVariable
                value = stack.pop()
                out[stack.pop()] = str(value)
            elif op != 0:
                return None
    return out


# ---------------------------------------------------------------- imported controls
# leaderboard (MenuPicker, Button Item, Game Icons), racehud (Game Icons), spectator_mode (Button Item) and skatetrick
# (Trick Gesture) place characters of shared control bundles, which old_hud1.movie leaves out (apt_movie refuses an
# unknown character, and attachMovie finds no export). These four are rebuilt with their imports merged, as
# old_screens.movie / tools/prepare_core_menu.py do: each bundle's ids offset by BUNDLE_STRIDE, imported placeholders
# resolved to the exporter, action blocks keyed (bundle << ACTION_SHIFT) | offset, init_order, exports. Clip events
# become placement init values (clip_values); any that aren't plain sets are dropped and printed.
from prepare_core_menu import _bundles, _init_order, BUNDLE_STRIDE, ACTION_SHIFT
IMPORTING = {'leaderboard': SCREENS + 'hudComponents/leaderboard', 'racehud': SCREENS + 'hudComponents/racehud',
             'spectator_mode': SCREENS + 'hudComponents/spectator_mode', 'skatetrick': SKATETRICK}
hud1.WORK = WORK_ROOT/'online1-merge-cache'
merge_cache = hud1.cache(tuple(IMPORTING.values()) + ('data/fe/source/controls/', 'data/fe/source/images/trick_gestures/')
                         + hud1.EXTRA)


# Skate 1's root movie (main.apt, loaded before every screen) sets the platform globals button_item and leaderboard
# read: _global.Platform_XBOX360 = 0, Platform_PS3 = 1, PlatformCurrent = Platform_XBOX360. These screens run without
# it (undefined == undefined takes button_item's PS3 glyphs), so those three assignments of its root frame action, as
# shipped, run first: its constant pool, then the rows from `_global.Platform_XBOX360` to PlatformCurrent's set.
def platform_block(at=928):
    main = Big(s1game/'data/fe/screens.big')
    raw = {e.path.rsplit('.', 1)[1]: main.read(e) for e in main.entries
           if e.path in ('data/fe/source/screens/main.apt', 'data/fe/source/screens/main.const')}
    rows = Actions(raw['apt'], raw['const']).stream(at)
    pool = [v['value'] for v in rows[0]['values']]
    pushed = [i for i, r in enumerate(rows) if r['opcode'] == 0xA2 and isinstance(r.get('operand'), int)]
    first = next(i for i in pushed if pool[rows[i]['operand']] == 'Platform_XBOX360') - 1
    current = next(i for i in pushed if i > first and pool[rows[i]['operand']] == 'PlatformCurrent')
    last = next(i for i in range(current, len(rows)) if rows[i]['opcode'] == 0x4F)
    block = rows[first:last + 1]
    assert rows[0]['opcode'] == 0x88 and rows[first]['opcode'] == 0x71 and not any(r.get('target') for r in block)
    return at, [rows[0]] + block + [{'offset': rows[last]['next'], 'opcode': 0}]


PLATFORM = platform_block()
caps = lambda s: ''.join(p if p.startswith(('{', '<')) else p.upper() for p in re.split(r'(\{[^}]*\}|<[^>]*>)', s))  # old_hud1.movie's


def merged(c, name, base):
    """`base` (old_hud1's manifest: language, fonts) with `name`'s characters and its imports' merged in, and the
    imports' string IDs added to its language."""
    bundles = _bundles(c, name)
    index = {key: i for i, key in enumerate(bundles)}
    # main.apt's platform block, keyed as one bundle past the merged ones.
    platform = (len(bundles) << ACTION_SHIFT) | PLATFORM[0]
    language = dict(base['language'])

    def gid(key, local):
        owner, ch = c.resolve_character(key, local)
        return index[owner['key']]*BUNDLE_STRIDE + ch['id']

    characters, shapes, fonts, mappings, blocks, init, exports, scoped, dropped = [], {}, {}, {}, {}, {}, {}, {}, []
    for key in bundles:
        bundle = c.load_bundle(key)
        for export, local in bundle['exports'].items():
            exports.setdefault(export, gid(key, local))
            scoped.setdefault(str(index[key]), {})[export] = gid(key, local)
        raw = hud1.WORK/'raw'/(key + '.apt')
        apt = raw.read_bytes()
        acts = Actions(apt, raw.with_suffix('.const').read_bytes())
        for i in {m.decode() for m in re.findall(rb'ID_[A-Z0-9_]{3,}', apt + raw.with_suffix('.const').read_bytes())}:
            if i not in language and english(i):
                language[i] = caps(english(i))
        for local, ch in bundle['characters'].items():
            ch = json.loads(json.dumps(ch))
            ch['id'] = index[key]*BUNDLE_STRIDE + local
            for f in ch.get('frames', []):
                for control in f['controls']:
                    place = control['type_name'].startswith('place_object')
                    if place and control.get('flags', 0) & 2:
                        control['character_id'] = gid(key, control['character_id'])
                    if place and control.get('flags', 0) & 0x80:
                        values = clip_values(apt, acts, control['actions_offset']) if control.get('actions_offset') else {}
                        if values is None:
                            dropped.append((key, local, control.get('name')))
                        else:
                            control['init'] = {**control.get('init', {}), **values}
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
                scene = hud1.SceneFlattener(c, lambda *_: {}).flatten(key, local)
                if scene['unresolved']:
                    raise ValueError(f'{key}: unresolved shape {scene["unresolved"]}')
                for p in scene['primitives']:
                    if not p.get('texture'):  # solid fill: a white texel keeps its colour (as old_hud1.movie)
                        p['texture'] = {'width': 1, 'height': 1, 'rgba': hud1.WHITE}
                        for tri in p['triangles']:
                            for v in tri:
                                v['uv'] = [0.5, 0.5]
                shapes[str(ch['id'])] = scene['primitives']
            elif ch['type_name'] == 'font':
                n = ch['font']['name']
                fonts[n], mappings[n] = hud1.gfn_font(hud1.GFN[n])
            characters.append(ch)
    for t in {(p['texture']['rgba'], p['texture']['width'], p['texture']['height']) for prims in shapes.values() for p in prims}:
        if t[0] != hud1.WHITE:
            hud1.bleed(*t)
    if dropped:
        print(name, 'dropped clip events (not plain sets):', dropped)
    blocks[str(platform)] = PLATFORM[1]
    return base | {'source': {'bundles': bundles, 'game': 'skate1'}, 'bundles': bundles, 'characters': characters,
                   'shapes': shapes, 'fonts': fonts, 'font_mappings': mappings, 'actions': blocks,
                   'init_order': [platform] + [o for key in _init_order(c, bundles) for o in init.get(key, [])],
                   'exports': exports, 'scoped_exports': scoped, 'language': language}


imported = {}
for stem, name in IMPORTING.items():
    base = skatetrick if stem == 'skatetrick' else json.loads((HUD_OUT/'runtime'/(stem + '.json')).read_text(encoding='utf-8'))
    imported[stem] = merged(merge_cache, name, base)
    print(stem, 'bundles:', imported[stem]['bundles'])
hud1.install(imported)
hud1.check(list(imported))

# ---------------------------------------------------------------- modes.json
enums = json.loads((TABLES/'online1_enums.json').read_text(encoding='utf-8'))
modes = {
    'source': 'old_online1_modes.py: Skate 1 english.big (by label and by default.xex hash), default.xex HUD enums',
    'titles': {m: english(l) for m, l in TITLES.items()},
    'strings': {k: text[h] for k, h in HASHED.items()},
    'labels': {l: english(l) for l in LABELS},
    'objectives': {'points': english(POINTS),
                   'tricks': [english('ID_OBJECTIVE_TITLE_TRICK_' + t) if t else None for t in TRICK_TITLES]},
    'enums': enums,
}
assert all(modes['titles'].values()) and all(modes['labels'].values()), modes
assert modes['objectives']['points'] and all(modes['objectives']['tricks'][i] for i, t in enumerate(TRICK_TITLES) if t)
OUT.mkdir(parents=True, exist_ok=True)
(OUT/'modes.json').write_text(json.dumps(modes, indent=1, ensure_ascii=False) + '\n', encoding='utf-8')
print('modes.json:', modes['titles'], modes['strings'])

# ---------------------------------------------------------------- race gate models (old_online1_gates)
sys.path.insert(0, str(repo/'tools/vendor/skate3_anim'))
import rx2_skeleton
import old_online1_gates
gates = old_online1_gates.export(s1game, OUT/'gates', rx2_skeleton)
print('gates:', {k: (len(m['positions']), m['material']) for k, m in gates['models'].items()})


# ---------------------------------------------------------------- sounds (old_screens1.audio)
# Audit 10-28 GAP-2: sfx/<Name>.wav for every Audio.Play<Name>() these movies call that plays a 1000 / 2000 menu sound
# (S.K.A.T.E.'s letters, the leaderboard, the race HUD, the timer), as the tutorial's; apt_screen plays them by name.
work, out = hud1.WORK, hud1.OUT
import old_screens, old_screens1
hud1.WORK, hud1.OUT = work, out  # its import aims old_hud1 at the tutorial's folders
old_screens1.OUT = HUD_OUT  # audio() writes <OUT>/sfx
used = {n for f in sorted((HUD_OUT/'runtime').glob('*.json')) for n in old_screens.natives(json.loads(f.read_text(encoding='utf-8')))}
# Bug 10-31: hudbase's PlayCountDownSound (the 3-2-1 and its GO, online every mode) isn't in an exported movie's
# ActionScript: skate1_online_hud's messenger plays it by these names.
COUNTDOWN = {'PlayCountDownGenSound', 'PlayCountDownGenGoSound'}
sounds = old_screens1.audio(used | COUNTDOWN)
assert {'PlayGetLetter', 'PlayPlayerEliminated', 'PlayLeaderboardIn'} | COUNTDOWN <= sounds.keys(), sounds
print('sfx:', sounds)
# Bug 10-31: the countdown's samples are near full scale (loudest 400 ms -4.9 / -8.0 dB), far over the HUD's other
# sounds. Not retail (HUD.abk's own playback level isn't traced): both play at -15 dB (apt_screen::fe_gain).
(HUD_OUT/'sfx'/'gains.json').write_text(json.dumps({'PlayCountDownGenSound': 0.31, 'PlayCountDownGenGoSound': 0.45}, indent=1) + '\n', encoding='utf-8')


# ---------------------------------------------------------------- install
def place(src, dest):
    """Copy src to dest; an existing dest that differs is kept once as <dest>.pre-online1modes."""
    keep = dest.with_name(dest.name + '.pre-online1modes')
    if dest.is_file() and not keep.exists() and dest.read_bytes() != src.read_bytes():
        shutil.copy2(dest, keep)
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dest)


if install_to:
    scripts = sorted((OUT/'scripts').glob('*.lua'))
    assert scripts, f'no decompiled scripts in {OUT/"scripts"}: run old_online1.py first'
    for lua in scripts:
        place(lua, install_to/'private/skate1-online/scripts'/lua.name)
    place(OUT/'modes.json', install_to/'private/skate1-online/modes.json')
    for f in sorted((OUT/'gates').iterdir()):
        place(f, install_to/'private/skate1-online/gates'/f.name)
    runtime = ['runtime/centralhud.json', 'runtime/skatetrick.json', 'runtime/leaderboard.json', 'runtime/racehud.json',
               'runtime/spectator_mode.json']
    for rel in sorted(set().union(hud1.payloads(manifest), *map(hud1.payloads, imported.values()))) + runtime:
        place(HUD_OUT/rel, install_to/'private/hud-skate1/online'/rel)
    for wav in sorted((HUD_OUT/'sfx').glob('*.wav')):
        place(wav, install_to/'private/hud-skate1/online/sfx'/wav.name)
    print('installed', len(scripts), 'scripts, modes.json, centralhud and the import-merged', sorted(imported), 'into', install_to)
