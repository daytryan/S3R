"""Skate 2's online modes on New_San_Vanelona: rows, spawns, online areas, intros and the online player HUD.

Writes into ASSETS/private/ (an export dir; --install copies it into an installation):
  skate2-online/rows.json        every Skate 2 online row (AvailableOnline, not DebugOnly) by mode: title, description,
                                 start {matrix, slots} / boundary / oob_time (online.json's shape) and what the mode
                                 scripts read: time limit, rounds (each round's start, boundary volume, times, intro
                                 scenes), waiting/attempt spawns, trigger volumes, intro scenes, Lua template
  skate2-online/intros/          tutorial.json + scene/camera/<scene>.json (+ scene/anim/ banks of actor tracks): the
                                 pre-match NIS cameras of the modes this script owns (Spot Battle flythroughs, Best
                                 Trick/Jam round intros, 1-Up overviews), evaluated by old_tutorial's ICE code. Same
                                 format and file name as skate2-tutorial's scenes, so challenge::cutscene's
                                 Cutscene::start(<assets>/private/skate2-online/intros, scene) plays one as it is.
  skate2-online/scripts/<template>.lua  every mode template's handlers, decompiled (reference; not installed)
  hud-skate2/online/runtime/<movie>.json + payloads  the online player HUD movies, old_hud's manifest format

--install INST_ASSETS (an installation's assets/ dir):
  - merges the rows into private/challenge-map/online/online.json (backup online.json.pre-skate2online, once):
    challenges into the Versus tab's ID_MAP_FILTER_CHALLENGETYPES_<MODE> folders (match_core::Mode::from_filter),
    freeskate locations at the top of the Freeskate tab's "#New San Vanelona" folder. Every entry it adds carries
    "game": "skate2", so a rerun replaces them and leaves the Skate 3 rows as they are. Then game_tabs.apply moves
    every Skate 2 row into the "#Skate 2" tab (user's design: Skate 2 and Skate 1 get their own tabs after Versus).
  - adds ID_MAP_FILTER_CHALLENGETYPES_BESTTRICK / _JAM (no retail Skate 3 text) to the `language` of the menu and
    online HUD manifests that carry the challenge type names (backup <file>.pre-skate2online, once);
  - copies the exports above (skate2-online/rows.json, skate2-online/intros/, hud-skate2/online/) as new files.
  Nothing is deleted; none of these files has an installation.json receipt.

Evidence and decisions: ONLINE_REPORT.md.
usage (java on PATH for unluac): py -3 old_online.py REPO SKATE2_ROOT ASSETS [--install INST_ASSETS]
"""
import json, os, re, shutil, struct, sys, tempfile, time
from pathlib import Path

repo, s2game, assets = map(Path, sys.argv[1:4])
install_to = Path(sys.argv[sys.argv.index('--install') + 1]) if '--install' in sys.argv else None
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
import game_tabs
import s2lang
from tools.online_challenge_map import global_locators, boundary, compact_json


def tutorial_lib():
    """old_tutorial.py's library half (it runs and rmtrees its own output below `vaults = db_vaults()`)."""
    src = (here/'old_tutorial.py').read_text(encoding='utf-8')
    ns = {'__name__': 'old_tutorial_lib', '__file__': str(here/'old_tutorial.py')}
    argv, sys.argv = sys.argv, ['old_tutorial.py', str(repo), str(s2game), str(assets)]
    try:
        exec(compile(src.split('\nvaults = db_vaults()')[0], str(here/'old_tutorial.py'), 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


T = tutorial_lib()
OUT = assets/'private/skate2-online'
HUD_OUT = assets/'private/hud-skate2/online'
MAP = 'New_San_Vanelona'
AVAILABLE_ONLINE, DEBUG_ONLY, STEPS = 'Hash_F9A9A1DDED84A643', 'Hash_FD40E4A31027C984', 'Hash_3CB7BA2AFC9BDE42'
# eChallengeTypes (Skate 3 keeps the numbering) -> (match_core mode, Versus folder filter id, icon). 1-Up has no
# match_core mode: its rows go in the retail 1UP folder (from_filter -> None). Icons are Skate 2's own minimap
# markers (old_hud's minimap icons), registered in online.json as skate2_<name> beside Skate 3's.
MODES = {9: ('spot_battle', 'SPOTBATTLE', 'mini_spotfound'), 8: ('best_trick', 'BESTTRICK', 'mini_besttrick'),
         15: ('jam', 'JAM', 'mini_jam'), 19: ('death_race', 'DEATHRACE', 'mini_deathrace'),
         31: ('hall_of_meat', 'HOMBATTLE', 'mini_hom'), 4: ('skate', 'SKATE', 'mini_skate'),
         32: ('one_up', '1UP', 'mini_1up')}
S2_ICONS = assets/'private/hud-skate2/assets/data/fe/source/images/icons/minimap'
FREESKATE, ACTIVITY = 22, 42
FILTER = 'ID_MAP_FILTER_CHALLENGETYPES_'
# Folders Skate 3 lacks: their label (Skate 2's own mode name) and help (its mode description), added to the language
# tables at install. Order: after the retail folders.
NEW_FOLDERS = {'BESTTRICK': ('ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK', 'ID_MISSION_TEMPLATE_BEST_TRICK_DESCRIPTION'),
               'JAM': ('ID_MISSION_TEMPLATE_ONLINE_JAM', 'ID_MISSION_TEMPLATE_JAM_DESCRIPTION')}
# The online player HUD (spot_battle/hall_of_meat `setup`: HUD2.eHUDComponent lists when Online.IsOnline, plus the
# session-wide event log, chyron, spectating and round results). trickdisplay/central/minimap are old_hud's already.
HUD_MOVIES = {m: f'data/fe/source/screens/hud2/{m}' for m in
              ('hudtimer', 'hudcountdown', 'leaderboardhud', 'hudonlinevoting', 'dpo', 'onlineotsstat',
               'hud_online_eventlog2', 'online_chyron', 'onlineobserving')}
HUD_MOVIES['roundresults'] = 'data/fe/source/screens/hudComponents/roundresults'
FIXED = ('RoundTime', 'AudioLeadEarlyTime', 'AudioLeadLateTime', 'AudioNewLeaderTime', 'AudioOutOfBoundsWarningTime',
         'AudioRoundEndWarningTime')  # Int16 seconds in the round rows, exported as floats


# ---------------------------------------------------------------- vault
vaults = T['db_vaults']()
v = vaults['ChallengeCore']
text = s2lang.strings(s2game)


def merged(name):
    m = {}
    for c in T['SELF_CLASSES']:
        row = v.row(c, name)
        if row:
            for k, val in v.fields(row).items():
                m.setdefault(k, val)
    return m


def get(m, k, default=None):
    return v.value(m[k]) if k in m else default


def say(id_):
    """A row label: Skate 2's English text as a literal (Skate 3's tables lack Skate 2's ids)."""
    return '#' + text[id_] if id_ in text else '#' + id_


def scene_of(nis):
    return nis[0] if isinstance(nis, list) and nis and isinstance(nis[0], str) and nis[0] else None


rows_by_key = {r['key']: r for r in v.rows.values() if r['class'] == 'challenges'}
parents = {r['parent'] for r in rows_by_key.values()}


def template(name):
    """The row (itself or an ancestor) whose own fields hold the Steps: the mode script."""
    r = rows_by_key[name]
    while r and STEPS not in r['fields']:
        r = rows_by_key.get(r['parent'])
    return r and r['key']


# ---------------------------------------------------------------- placement
glb = {}
gbig = Big(s2game/'data/big/globallocators.big')
for e in gbig.entries:
    if e.path.lower().startswith('data/content/global_locators/'):
        for k, loc in global_locators(gbig.read(e)).items():
            glb.setdefault(k, dict(loc, stream=Path(e.path).name))
misc = Big(s2game/'data/big/miscload.big')
polygons = {Path(e.path).stem.lower(): boundary(misc.read(e).decode('latin-1')) for e in misc.entries
            if e.path.lower().startswith('data/boundary/') and e.path.lower().endswith('.xml')}
missions = Big(s2game/'data/content/missions.big')
mission_files = {}
for e in missions.entries:
    parts = e.path.lower().split('/')
    if len(parts) >= 4 and parts[-3] == 'missions':
        mission_files.setdefault(parts[-2], {})[Path(e.path).name] = e


def mission(key):
    """(locators {name lower: {matrix, slots}}, volumes {name: box}) of the row's mission stream (missions/<key>,
    else <key> without its _ol suffix). Locator records match global_locators' (children = per-slot spawns)."""
    name = next((n for n in (key.lower(), re.sub(r'_ol$', '', key.lower())) if n in mission_files), None)
    if not name:
        return {}, {}, None
    files = mission_files[name]
    sim = next((n for n in files if n.lower().endswith('_sim.xst')), None)
    locators, volumes = {}, {}
    with tempfile.TemporaryDirectory(prefix='skate2-online-') as work:
        work = Path(work)
        T['write_streams'](work, files, missions.read, *filter(None, [sim]), 'cSim_Global.xsf')  # the two read below
        for asset in T['read_sfil'](work/'cSim_Global.xsf', T['read_atoc'](work/sim)):
            d = asset.data
            if d[:4] != b'\x89RW4':
                continue
            locators.update(global_locators(d))
            for kind, at, _ in T['rw4_objects'](d):
                if kind == 0xEB0019:  # as old_tutorial.mission_placement: oriented trigger volumes
                    _, count, _, first = struct.unpack_from('>4I', d, at)
                    for r in range(count):
                        o = at + first + 0xF0 * r
                        full = T['cstr'](d, at + struct.unpack_from('>I', d, o + 0xE0)[0])
                        volumes[full.split('_0x', 1)[0].lower()] = {
                            'matrix': T['matrix'](d, o), 'min': list(struct.unpack_from('>3f', d, o + 0x40)),
                            'max': list(struct.unpack_from('>3f', d, o + 0x50))}
    return locators, volumes, name


def locate(names, local):
    """The first name that resolves (global locators, then the mission's), preferring one with spawns for a lobby
    (2+ slots), then any slot."""
    found = [(n, glb.get(n.lower()) or local.get(n.lower())) for n in names if isinstance(n, str) and n]
    found = [(n, loc) for n, loc in found if loc]
    best = next(((n, l) for n, l in found if len(l['slots']) > 1),
                next(((n, l) for n, l in found if l['slots']), found[0] if found else (None, None)))
    return best[0], best[1] and {'matrix': best[1]['matrix'], 'slots': best[1]['slots']}


def volume(field, vols):
    name = field['volume'].split('|')[1] if isinstance(field, dict) and '|' in field.get('volume', '') else None
    return name and {'name': name, **(vols.get(name.lower()) or {'missing': True})}


# ---------------------------------------------------------------- rows
rows, activities, scenes_wanted, templates = {}, [], {}, {}
for key in sorted(k for k in rows_by_key if k not in parents):
    m = merged(key)
    kind = get(m, 'ChallengeType')
    if not get(m, AVAILABLE_ONLINE) or get(m, DEBUG_ONLY) or kind not in (*MODES, FREESKATE, ACTIVITY):
        continue
    local, vols, mission_name = mission(key)
    tpl = template(key)
    templates[tpl] = kind
    row = {'id': key, 'type': kind, 'title': get(m, 'Title'), 'template': tpl, 'mission': mission_name}
    row['name'] = say(row['title'])
    desc = get(m, 'Description', '')
    desc = desc if isinstance(desc, str) and desc.startswith('ID_') else (row['title'] or '') + '_DESC'
    row['description'] = say(desc) if desc in text else ''
    if kind == ACTIVITY:
        row.update(activity=get(m, 'FreeskateActivityType'), time_limit=get(m, 'TimeLimit'))
        row['location'], start = locate([get(m, 'Location')], local)
        row['start'] = start
        activities.append(row)
        continue
    rounds = []
    if 'ContestRounds' in m:
        for r in v.refs(m['ContestRounds']):
            f = v.named(v.fields(r))
            loc_name, start = locate([f.get('StartLocation')], local)
            rounds.append({'start_locator': loc_name, 'start': start, 'boundary_volume': volume(f.get('RoundBoundary'), vols),
                           **{k: float(f[k]) for k in FIXED if isinstance(f.get(k), int)},
                           'run_type': f.get('RunType'),
                           'intro_scenes': [e['a'] for e in f.get('IntroPresentationEvents') or [] if e['type'] == 5],
                           'round_scenes': [e['a'] for e in f.get('RoundPresentationEvents') or [] if e['type'] == 5]})
    # Contest competitors spawn on the round's StartLocation slots; everyone else on the row's locator. Turn-based
    # rows (S.K.A.T.E., 1-Up) whose locator has no per-player children start the lobby on the waiting locator.
    order = [rounds[0]['start_locator']] if rounds else []
    order += [get(m, k) for k in ('MapStartLocation', 'Location', 'StartLocation', 'TurnBasedWaitingLocation')]
    row['start_locator'], row['start'] = locate(order, local)
    row['boundary'] = polygons.get(key.lower()) or (mission_name and polygons.get(mission_name)) or None
    oob = get(m, 'OutOfBoundsTime')
    row['oob_time'] = oob if isinstance(oob, int) and oob > 0 else None
    row['time_limit'], row['num_rounds'] = get(m, 'TimeLimit'), get(m, 'NumRounds')
    for k in ('TurnBasedWaitingLocation', 'TurnBasedAttemptSpawnPoint'):
        if get(m, k):
            name, loc = locate([get(m, k)], local)
            row[k] = {'locator': get(m, k), **(loc or {'missing': True})}
    for k in ('ChallengeBoundary', 'ScoringBoundary', 'TurnBasedStartVolume'):
        if volume(get(m, k), vols):
            row[k] = volume(get(m, k), vols)
    if get(m, 'SpotVolumes'):
        row['SpotVolumes'] = [volume(s, vols) for s in get(m, 'SpotVolumes')]
    intro = [scene_of(get(m, 'NISFlythroughDefinition')), scene_of(get(m, 'IntroNIS'))]
    intro += [e['a'] for e in get(m, 'IntroPresentationEvents') or [] if e['type'] == 5]
    row['intro_scenes'] = [s for s in intro if s]
    if rounds:
        row['rounds'] = rounds
    if kind in (9, 8, 15, 32):  # the pre-match intros this script owns (DR/HoM/S.K.A.T.E. agents own theirs)
        for s in row['intro_scenes'] + [s for r in rounds for s in r['intro_scenes'] + r['round_scenes']]:
            scenes_wanted.setdefault(s, []).append(key)
    rows[key] = row

by_mode = {}
for r in rows.values():
    by_mode.setdefault('freeskate' if r['type'] == FREESKATE else MODES[r['type']][0], []).append(r)
missing = sorted(k for k, r in rows.items() if not r['start'])
assert not missing, f'rows without a start: {missing}'

# ---------------------------------------------------------------- intros (cameras; actor tracks get their clip banks)
T['OUT'] = OUT/'intros'
intros, broken = {}, []
scene_big = Big(s2game/'data/big/scene.big')
anims = {e.path.removeprefix('data/'): e for e in scene_big.entries if e.path.startswith('data/scene/anim/')}
(OUT/'intros/scene/anim').mkdir(parents=True, exist_ok=True)
for name in sorted(scenes_wanted):
    try:
        got = T['scenes']({name})
    except AssertionError:
        broken.append(name)  # an ICE take without camera keys (old_tutorial asserts on it)
        continue
    for s in got.values():
        s['rows'] = scenes_wanted[name]
        for tr in s['tracks']:  # as old_tutorial: the bank as-is, plus the female skater's where the disc has one
            female = tr['anim'].replace('_Player1.abin', '_Female1.abin')
            if female != tr['anim'] and female in anims:
                tr['female_anim'] = female
            for a in {tr['anim'], tr.get('female_anim', tr['anim'])}:
                (OUT/'intros'/a).write_bytes(scene_big.read(anims[a]))
    intros.update(got)
T['post_audio'](intros, s2game/'data/audio/post.big', OUT/'intros')
(OUT/'intros').mkdir(parents=True, exist_ok=True)
(OUT/'intros/tutorial.json').write_text(json.dumps({  # the name Cutscene::start reads
    'kinds': {2: 'Flythrough', 12: 'SpotIntro', 24: '1Up', 25: 'BestTrick'},
    'space': '1 world, 2 avatar, 3 scene origin (old_tutorial.scene_camera)',
    'scenes': intros, 'missing': sorted(set(scenes_wanted) - set(intros)), 'no_camera': broken}, indent=1), encoding='utf-8')

# ---------------------------------------------------------------- scripts (reference)
(OUT/'scripts').mkdir(parents=True, exist_ok=True)
lua_index, decompiled = {}, {}


def decompile(chunk):
    """old_tutorial.decompile, once per distinct chunk (row templates repeat them), retried when Windows still holds
    its temp .luac at cleanup (WinError 32)."""
    if chunk not in decompiled:
        for attempt in range(5):
            try:
                decompiled[chunk] = T['decompile'](chunk)
                break
            except PermissionError:
                if attempt == 4:
                    raise
                time.sleep(2)
    return decompiled[chunk]


def chunks(tpl):
    """The handler chunks the loop below decompiles for a template, in its order."""
    for step in v.refs(rows_by_key[tpl]['fields'][STEPS]):
        if 'unresolved' in step:
            continue
        sf = v.fields(step)
        for h in v.refs(sf['Hash_810394ECEFE7E48B']) if 'Hash_810394ECEFE7E48B' in sf else []:
            size, offset = struct.unpack('>II', bytes.fromhex(v.fields(h)['Hash_C3CD4B4183274819']['data']))
            yield v.bin[offset:offset + size]


T['prefetch']([c for tpl in sorted(templates) for c in chunks(tpl)])  # unluac runs side by side (old_tutorial.prefetch)
for tpl in sorted(templates):
    lua = [f'-- {tpl}: Skate 2 ChallengeCore handlers, decompiled by unluac']
    steps = {}
    for step in v.refs(rows_by_key[tpl]['fields'][STEPS]):
        if 'unresolved' in step:
            continue
        sf = v.fields(step)
        handlers = v.refs(sf['Hash_810394ECEFE7E48B']) if 'Hash_810394ECEFE7E48B' in sf else []
        steps[sf['Name']['data']] = {}
        for h in handlers:
            hf = v.fields(h)
            size, offset = struct.unpack('>II', bytes.fromhex(hf['Hash_C3CD4B4183274819']['data']))
            fn, source = decompile(v.bin[offset:offset + size])
            steps[sf['Name']['data']][hf['Name']['data']] = fn
            lua.append(f'-- [{sf["Name"]["data"]}] {hf["Name"]["data"]}\n{source.strip()}\n')
    (OUT/f'scripts/{tpl}.lua').write_text('\n'.join(lua), encoding='utf-8')
    lua_index[tpl] = {'type': templates[tpl], 'steps': steps}

(OUT/'rows.json').write_text(compact_json({
    'map': MAP, 'source': 'Skate 2 ChallengeCore (AvailableOnline, not DebugOnly); see ONLINE_REPORT.md',
    'modes': by_mode, 'freeskate_activities': activities, 'scripts': lua_index}) + '\n', encoding='utf-8')
print({k: len(x) for k, x in by_mode.items()}, len(activities), 'activities;', len(intros), 'intros;', len(lua_index), 'templates')

# ---------------------------------------------------------------- online player HUD
argv, sys.argv = sys.argv, ['old_hud.py', str(repo), str(s2game), str(assets)]
try:
    import old_hud
    from old_screens import STAND_INS  # Skate 1-era font names (roundresults' TradeBld) -> Skate 2 stand-ins
finally:
    sys.argv = argv
from prepare_hud import font_mapping
old_hud.OUT, old_hud.WORK, old_hud.MOVIES = HUD_OUT, WORK_ROOT/'online-hud-cache', HUD_MOVIES
cache = old_hud.cache()
stock = install_to/'private/hud/runtime/trickdisplay.json' if install_to else None
language = (json.loads(stock.read_text(encoding='utf-8'))['language'] if stock and stock.is_file() else {}) | text
mappings = font_mapping(old_hud.collections(), cache)
for name, (family, apt) in STAND_INS.items():  # as old_screens does
    cache.add_font_alias(name, family, mappings[apt])
    mappings[name] = mappings[apt] | {'stand_in': apt}
manifests = {stem: old_hud.movie(cache, path, mappings, language) for stem, path in HUD_MOVIES.items()}
old_hud.install(manifests)
for stem, manifest in manifests.items():  # old_hud.check's payload test, for these movies
    for p in [p for prims in manifest['shapes'].values() for p in prims if p.get('texture')]:
        assert (HUD_OUT/p['texture']['rgba']).stat().st_size == p['texture']['width'] * p['texture']['height'] * 4
    for f in manifest['fonts'].values():
        t = f['definition']['textures'][0]
        assert (HUD_OUT/f['texture']).stat().st_size == t['width'] * t['height'] * 4
print('hud fonts:', {s: sorted(m['fonts']) for s, m in manifests.items()})


# ---------------------------------------------------------------- install
def backup(path):
    keep = path.with_name(path.name + '.pre-skate2online')
    if not keep.exists():
        shutil.copy2(path, keep)


def entry(r):
    spawn = {'start': r['start'], 'boundary': r['boundary'], 'oob_time': r['oob_time']}
    if r['type'] == FREESKATE:
        return {'name': r['name'], 'game': 'skate2', 'location': {'map': MAP, 'teleport': None, **spawn, 'id': r['id']}}
    at = r['start']['matrix'][3][:3]
    return {'name': r['name'], 'game': 'skate2', 'challenge': {
        'id': r['id'], 'map': MAP, 'teleport': None, 'position': at, **spawn, 'description': r['description'],
        'objectives': [], 'bonus': 0, 'icon': 'skate2_' + MODES[r['type']][2], 'map_image': None,
        'mode': MODES[r['type']][0]}}


def strip(rows_):
    return [dict(e, rows=strip(e['rows'])) if e.get('rows') else e for e in rows_ if e.get('game') != 'skate2']


if install_to:
    online_path = install_to/'private/challenge-map/online/online.json'
    online = json.loads(online_path.read_text(encoding='utf-8'))
    backup(online_path)
    tabs = {t['name']: t for t in online['tabs']}
    for t in online['tabs']:
        t['rows'] = strip(t['rows'])
    versus, freeskate = tabs['ID_MAP_FILTER_VERSUS'], tabs['ID_FREESKATEOPTION_START']
    header = {'name': '#New San Vanelona', 'header': True, 'game': 'skate2'}
    for kind, (_, flt, _) in MODES.items():
        name = FILTER + flt
        folder = next((e for e in versus['rows'] if e.get('folder') and e['name'] == name), None)
        if folder is None:  # laid out as the retail folders: Options header + Quick Match, then the rows
            label, help_ = NEW_FOLDERS[flt]
            retail = next(e for e in versus['rows'] if e.get('folder') and e['name'].startswith(FILTER))
            folder = {'name': name, 'help': say(help_), 'folder': True, 'game': 'skate2', 'rows': retail['rows'][:2]}
            versus['rows'].append(folder)
        mine = sorted((r for r in rows.values() if r['type'] == kind), key=lambda r: r['name'].lower())
        folder['rows'] += [header] + [entry(r) for r in mine]
    # After game_tabs the map's freeskate folder is the Skate 2 tab's Freeskate folder.
    nsv = next(e for t in (freeskate, tabs.get('#Skate 2', {'rows': []})) for e in t['rows']
               if e.get('folder') and e['name'] in ('#New San Vanelona', game_tabs.FREESKATE))
    locations = sorted((r for r in rows.values() if r['type'] == FREESKATE), key=lambda r: r['name'].lower())
    nsv['rows'] = ([{'name': '#Skate 2 Online', 'header': True, 'game': 'skate2'}] + [entry(r) for r in locations] +
                   [{'name': '#Teleport Spots', 'header': True, 'game': 'skate2'}] + nsv['rows'])
    for _, _, icon in MODES.values():
        rel = f'online/icons/skate2_{icon}.rgba'
        assert (S2_ICONS/f'{icon}.rgba').stat().st_size == 32 * 32 * 4, icon
        shutil.copy2(S2_ICONS/f'{icon}.rgba', online_path.parent.parent/rel)
        online['icons']['skate2_' + icon] = {'rgba': rel, 'width': 32, 'height': 32}
    online = game_tabs.apply(online)
    online_path.write_text(compact_json(online) + '\n', encoding='utf-8')
    # Folder labels for the two Skate 2-only filters, wherever the type names are shown (map menu, online HUDs).
    words = {FILTER + flt: text[label] for flt, (label, _) in NEW_FOLDERS.items()}
    for path in [install_to/'private/challenge-map/runtime/challengemap.json',
                 *sorted((install_to/'private/online-ui/runtime').glob('*.json'))]:
        data = json.loads(path.read_text(encoding='utf-8'))
        if FILTER + 'SKATE' in data.get('language', {}) and any(data['language'].get(k) != w for k, w in words.items()):
            backup(path)
            data['language'].update(words)
            path.write_text(json.dumps(data, separators=(',', ':'), ensure_ascii=False) + '\n', encoding='utf-8')
            print('language +', path.relative_to(install_to))
    dest = install_to/'private/skate2-online'
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copy2(OUT/'rows.json', dest/'rows.json')
    shutil.copytree(OUT/'intros', dest/'intros', dirs_exist_ok=True)
    shutil.copytree(HUD_OUT, install_to/'private/hud-skate2/online', dirs_exist_ok=True)
    # Self-check: what the engine will read back.
    check = json.loads(online_path.read_text(encoding='utf-8'))
    added = [r for t in check['tabs'] for e in t['rows'] for r in e.get('rows', []) if r.get('game') == 'skate2'
             and ('challenge' in r or 'location' in r)]
    assert len(added) == len(rows), (len(added), len(rows))
    lone = [r['name'] for r in added if len((r.get('challenge') or r.get('location'))['start']['slots']) < 2]
    print('installed', len(added), 'rows into', online_path, '; < 2 spawn slots (engine stacks on the locator):', lone)
