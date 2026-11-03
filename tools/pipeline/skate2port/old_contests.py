"""Skate 2's Spot Battle, Best Trick, Jam and 1-Up on New_San_Vanelona: what skate2_contests.rs needs beyond rows.json.

Writes OUT (install it as assets/private/skate2-online/contests/; never point OUT at the install):
  contests.json   every online row of the four modes (ChallengeType 9 Spot Battle, 8 Best Trick, 15 Jam, 32 1-Up;
                  AvailableOnline, not DebugOnly, as old_online.py picks them), from the ChallengeCore vault:
                  - Spot Battle: NumRounds, TimeLimit, NumSequencesToEndTurn, ContestCumulativeScoring, the spot volumes
                    and scoring boundary (world boxes), attempt / waiting spawns, indicators, the flythrough scene;
                  - Best Trick / Jam: ContestCumulativeScoring / Teams / Elimination and per round ContestVariant,
                    RunType, RoundTime, CumulativeMinScore, BonusPoints, RestartOnWipeout, AudioOutOfBoundsWarningTime,
                    round boundary + spot volumes, PathEndVolume, RestartLastChanceVolume, start (+ slots), indicators
                    and intro scene;
                  - 1-Up: TimeLimit, NumToWin, the start volume, attempt / waiting spawns, every OneUpObject (volumes,
                    start, indicators);
                  - each row's CentralMessageHALID* / ModifiedCentralMessageHALID* ids, and the English strings.
                  RoundTime / AudioOutOfBoundsWarningTime are Int16 seconds (floats here), NumToWin an Int8;
                  the Int32 CumulativeMinScore / BonusPoints are still divided by 65536 (0 on every disc row).
  indicator/indicators.json  old_indicators' format for skate2_arrows.rs's Arrows.more (shapes, styles, locators).
The flow (turns, rounds, voting, timings) is the templates' Lua (rows.json `scripts`; skate2-online/scripts/*.lua from
old_online.py) and the exe's VotingManager; skate2_contests.rs ports it and cites it.

With --voting HUD_DIR (old_online.py's hud-skate2/online, read only) it also writes OUT/hud/: runtime/contests_voting.json
(hudonlinevoting through old_screens.movie: imports merged, construct clip events as placement values) + its payloads;
copy that tree's missing files into assets/private/hud-skate2/online/ (the shared payloads are already there).

usage: py -3 old_contests.py REPO SKATE2_ROOT OUT [--voting HUD_DIR]
"""
import json, os, re, struct, sys, tempfile
from pathlib import Path

repo, s2game, OUT = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
import s2lang
from tools.online_challenge_map import global_locators

MODES = {9: 'spot_battle', 8: 'best_trick', 15: 'jam', 32: 'one_up'}
AVAILABLE_ONLINE, DEBUG_ONLY = 'Hash_F9A9A1DDED84A643', 'Hash_FD40E4A31027C984'
INDICATOR_TYPE, INDICATOR_MODEL = 'Hash_3297556BECB605E2', 'Hash_FB5FDAB5DC2BD228'
STYLE, STYLE_TEXTURE, STYLE_BACKGROUND, STYLE_TINT = ('Hash_F0DB8767D611AF94', 'Hash_8E38A3CD0186F8DA',
                                                      'Hash_FF4DCA217E05B9BC', 'Hash_E4F3418597AACAA5')
ORANGE = 'D7EDBD362D7D2152'  # the tutorial file's orange (a row authored with style 0 gets it)
FIXED = 65536.
STRING_PREFIXES = ('ID_CHALLENGE_CENTRAL', 'ID_CHALLENGE_MODIFIED', 'ID_CONTEST_', 'ID_CHALLENGE_OL_1UP',
                   'ID_CHALLENGE_1UP', 'ID_MISSION_TEMPLATE_ONLINE', 'ID_COMMON_RANK', 'ID_COMMON_NAME',
                   'ID_COMMON_SCORE', 'ID_COMMON_STATUS', 'ID_CHALLENGE_COMMON_CONTEST')


def tutorial_lib():
    """old_tutorial.py's library half (below `vaults = db_vaults()` it runs and rmtrees its own output)."""
    src = (here/'old_tutorial.py').read_text(encoding='utf-8')
    ns = {'__name__': 'old_tutorial_lib', '__file__': str(here/'old_tutorial.py')}
    argv, sys.argv = sys.argv, ['old_tutorial.py', str(repo), str(s2game), str(OUT/'x')]
    try:
        exec(compile(src.split('\nvaults = db_vaults()')[0], str(here/'old_tutorial.py'), 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


T = tutorial_lib()
vaults = T['db_vaults']()
v, sk = vaults['ChallengeCore'], vaults['skater']
text = s2lang.strings(s2game)

# ---------------------------------------------------------------- placement (as old_online.py)
glb = {}
gbig = Big(s2game/'data/big/globallocators.big')
for e in gbig.entries:
    if e.path.lower().startswith('data/content/global_locators/'):
        for k, loc in global_locators(gbig.read(e)).items():
            glb.setdefault(k, loc)
world = {k.lower(): {'matrix': m, 'slots': []} for k, m in T['world_locators']().items()}
missions = Big(s2game/'data/content/missions.big')
mission_files = {}
for e in missions.entries:
    parts = e.path.lower().split('/')
    if len(parts) >= 4 and parts[-3] == 'missions':
        mission_files.setdefault(parts[-2], {})[Path(e.path).name] = e
streams = {}


def stream(name):
    """(locators, volumes) of one missions/<name> stream (cSim_Global.xsf RW4 assets): locators as global_locators,
    volumes = oriented trigger boxes (kind 0xEB0019; every one on the disc has an identity matrix)."""
    if name in streams:
        return streams[name]
    files = mission_files.get(name, {})
    sim = next((n for n in files if n.lower().endswith('_sim.xst')), None)
    locators, volumes = {}, {}
    if sim and 'cSim_Global.xsf' in files:
        with tempfile.TemporaryDirectory(prefix='skate2-contests-') as work:
            work = Path(work)
            T['write_streams'](work, files, missions.read, sim, 'cSim_Global.xsf')  # the two read below
            for asset in T['read_sfil'](work/'cSim_Global.xsf', T['read_atoc'](work/sim)):
                d = asset.data
                if d[:4] != b'\x89RW4':
                    continue
                locators.update(global_locators(d))
                for kind, at, _ in T['rw4_objects'](d):
                    if kind == 0xEB0019:
                        _, count, _, first = struct.unpack_from('>4I', d, at)
                        for r in range(count):
                            o = at + first + 0xF0 * r
                            full = T['cstr'](d, at + struct.unpack_from('>I', d, o + 0xE0)[0])
                            m = T['matrix'](d, o)
                            assert all(abs(m[i][j] - (i == j)) < 1e-4 for i in range(4) for j in range(4)), full
                            volumes[full.split('_0x', 1)[0].lower()] = {
                                'min': [round(x, 3) for x in struct.unpack_from('>3f', d, o + 0x40)],
                                'max': [round(x, 3) for x in struct.unpack_from('>3f', d, o + 0x50)]}
    streams[name] = locators, volumes
    return streams[name]


def merged(name):
    m = {}
    for c in T['SELF_CLASSES']:
        row = v.row(c, name)
        if row:
            for k, val in v.fields(row).items():
                m.setdefault(k, val)
    return m


rows_by_key = {r['key']: r for r in v.rows.values() if r['class'] == 'challenges'}
parents = {r['parent'] for r in rows_by_key.values()}
missing = {'locators': [], 'volumes': [], 'indicators': []}


class Row:
    """One row's lookups: its mission stream (the key, else without _ol), then every stream for volumes it lacks."""
    def __init__(self, key):
        self.key = key
        name = next((n for n in (key.lower(), re.sub(r'_ol$', '', key.lower())) if n in mission_files), None)
        self.locators, self.volumes = stream(name) if name else ({}, {})

    def place(self, name, slots=False):
        if not isinstance(name, str) or not name:
            return None
        loc = glb.get(name.lower()) or self.locators.get(name.lower()) or world.get(name.lower())
        if not loc:  # some rows' streams are named otherwise (c_1a_05_besttrick_street: besttrick_test01)
            loc = next((s[0][name.lower()] for s in map(stream, mission_files) if name.lower() in s[0]), None)
        if not loc:
            missing['locators'].append(f'{self.key}: {name}')
            return None
        return {'locator': name, 'matrix': loc['matrix'], **({'slots': loc['slots']} if slots else {})}

    def box(self, field):
        """A tTriggerVolumeInstanceID ('DIST|name|id') -> {name, min, max}; a volume no stream has is None."""
        if not (isinstance(field, dict) and field.get('volume', '').count('|') == 2):
            return None
        name = field['volume'].split('|')[1]
        found = self.volumes.get(name.lower())
        if not found:  # authored in another row's stream (shared park furniture)
            found = next((s[1][name.lower()] for s in map(stream, mission_files) if name.lower() in s[1]), None)
        if not found:
            missing['volumes'].append(f'{self.key}: {name}')
            return None
        return {'name': name, **found}

    def boxes(self, fields):
        return [b for b in (self.box(f) for f in fields or []) if b]

    def indicators(self, fields):
        out = []
        for r in fields or []:
            w = r['words']
            shape, style = indicator_shape('%08X%08X' % (w[10], w[11])), '%08X%08X' % (w[16], w[17])
            at = self.place(r['locator'])
            if at and shape:
                out.append({**at, 'shape': shape, 'style': ORANGE if style == '0' * 16 else style})
            else:
                missing['indicators'].append(f'{self.key}: {r["locator"]}')
        return out


def inherited(cls, key, name):
    row = sk.row(cls, key)
    while row:
        if name in row['fields']:
            return row['fields'][name]['data']
        row = sk.row(cls, row['parent']) if row['parent'] else None


def indicator_shape(kind):
    """VisualIndicators type key -> the outline asset id (type -> model -> asset), as old_hall_of_meat."""
    model = inherited(INDICATOR_TYPE, int(kind, 16), 'Hash_8D090C8B5931547A')
    return model and inherited(INDICATOR_MODEL, int(model[16:32], 16), 'Hash_1AFE1B981FB92727')


def style_of(key):
    """An indicator style (old_indicators.style): its colour texture (the tutorial folder's <colour>.png) and tint."""
    # ponytail: the tutorial folder has the visual_indicator_<colour> textures only; volume_indicator_<colour> (the
    # spot-volume outlines' style) borrows its colour's.
    png = lambda t: t and re.sub(r'^(visual|volume)_indicator_', '', t.removesuffix('.Texture')) + '.png'
    names = {T['key_hash'](n): n for n in ('default', 'orange', 'red', 'yellow', 'blue', 'green')}
    k = names.get(int(key, 16), int(key, 16))
    tint = inherited(STYLE, k, STYLE_TINT)
    return {'texture': png(inherited(STYLE, k, STYLE_TEXTURE)), 'background': png(inherited(STYLE, k, STYLE_BACKGROUND)),
            'tint': list(struct.unpack('>4f', bytes.fromhex(tint))) if tint else [1, 1, 1, 1]}


def scene_of(nis):
    return nis[0] if isinstance(nis, list) and nis and isinstance(nis[0], str) and nis[0] else None


def fixed(x):  # ponytail: only the Int32 CumulativeMinScore / BonusPoints still pass here; 0 on every disc row
    return round(x / FIXED, 3) if isinstance(x, int) else x


def seconds(x):
    """An Int16 count of seconds (RoundTime, AudioOutOfBoundsWarningTime), as a float."""
    return float(x) if isinstance(x, int) else x


def messages(s):
    return {k: val for k, val in s.items() if k.startswith(('CentralMessageHALID', 'ModifiedCentralMessageHALID'))
            and isinstance(val, str) and val}


rows, variants = {}, {}
for key in sorted(k for k in rows_by_key if k not in parents):
    m = merged(key)
    get = lambda k, d=None: v.value(m[k]) if k in m else d
    kind = get('ChallengeType')
    if kind not in MODES or not get(AVAILABLE_ONLINE) or get(DEBUG_ONLY):
        continue
    s = v.named(m, skip=('Steps', 'Objectives', 'ContestRounds', 'OneUpObject'))
    r = Row(key)
    row = {'mode': MODES[kind], 'title': s.get('Title'), 'time_limit': s.get('TimeLimit'), 'messages': messages(s),
           'attempt': r.place(s.get('TurnBasedAttemptSpawnPoint'), True),
           'waiting': r.place(s.get('TurnBasedWaitingLocation'), True),
           'start': r.place(s.get('StartLocation'), True)}
    if kind == 9:
        row |= {'num_rounds': s.get('NumRounds'), 'sequences_to_end_turn': s.get('NumSequencesToEndTurn'),
                'cumulative': bool(s.get('ContestCumulativeScoring')), 'spot_volumes': r.boxes(s.get('SpotVolumes')),
                'scoring_boundary': r.box(s.get('ScoringBoundary')),
                'indicators': r.indicators(s.get('VisualIndicators')),
                'intro': scene_of(s.get('NISFlythroughDefinition'))}
    elif kind in (8, 15):
        intro = [e['a'] for e in s.get('IntroPresentationEvents') or [] if e['type'] == 5]
        row |= {k: bool(s.get('Contest' + k.title().replace('_', ''))) for k in ('cumulative_scoring', 'teams', 'elimination')}
        row['cumulative'] = row.pop('cumulative_scoring')
        row['rounds'] = []
        for i, rr in enumerate(v.refs(m['ContestRounds'])):
            f = v.named(v.fields(rr))
            scenes = [e['a'] for e in f.get('IntroPresentationEvents') or [] if e['type'] == 5]
            if i == 0 and not scenes:  # round 1's intro is the row's on three rows
                scenes = intro
            variants[f.get('ContestVariant')] = variants.get(f.get('ContestVariant'), 0) + 1
            row['rounds'].append({
                'variant': f.get('ContestVariant'), 'run_type': f.get('RunType'), 'time': seconds(f.get('RoundTime')),
                'cumulative_min': fixed(f.get('CumulativeMinScore', 0)), 'bonus_points': fixed(f.get('BonusPoints', 0)),
                'restart_on_wipeout': bool(f.get('RestartOnWipeout')), 'boundary': r.box(f.get('RoundBoundary')),
                # run_round: out of the boundary AudioOutOfBoundsWarningTime + 3 s puts the skater back; TimedRunMulti
                # (RunType 4) sends it to the next start slot at PathEndVolume; RestartLastChanceVolume arms the restart.
                'oob_warning': seconds(f.get('AudioOutOfBoundsWarningTime', 1)),
                'path_end': r.box(f.get('PathEndVolume')), 'restart_volumes': r.boxes(f.get('RestartLastChanceVolume')),
                'spot_volumes': r.boxes(f.get('SpotVolumes')), 'start': r.place(f.get('StartLocation'), True),
                'indicators': r.indicators(f.get('VisualIndicators')), 'intro': scenes[0] if scenes else None})
    else:
        # NumToWin is an Int8 (3 rounds; the Lua compares roundsWon to it).
        row |= {'num_to_win': s.get('NumToWin'), 'start_volume': r.box(s.get('TurnBasedStartVolume')),
                'intro': ([e['a'] for e in s.get('IntroPresentationEvents') or [] if e['type'] == 5] or [None])[0],
                'objects': []}
        for o in v.refs(m['OneUpObject']):
            f = v.named(v.fields(o))
            row['objects'].append({'volumes': r.boxes(f.get('ObjectVolume')), 'start': r.place(f.get('StartLocation')),
                                   'indicators': r.indicators(f.get('VisualIndicators'))})
    rows[key] = row

# ---------------------------------------------------------------- checks + write
by_mode = {mode: sorted(k for k, r in rows.items() if r['mode'] == mode) for mode in MODES.values()}
print({k: len(x) for k, x in by_mode.items()}, 'variants', variants)
print('teams', [k for k, r in rows.items() if r.get('teams')], 'elimination', [k for k, r in rows.items() if r.get('elimination')])
for what, items in missing.items():
    print(len(items), 'unresolved', what, items[:12])
assert all(r['spot_volumes'] for r in rows.values() if r['mode'] == 'spot_battle'), 'a spot battle without spots'
assert all(o['volumes'] for r in rows.values() if r['mode'] == 'one_up' for o in r['objects']), 'a 1-Up object without volumes'

looks = {i['locator']: {'shape': i['shape'], 'style': i['style']} for i in
         (i for r in rows.values() for i in r.get('indicators', []) + [i for rd in r.get('rounds', []) for i in rd['indicators']]
          + [i for o in r.get('objects', []) for i in o['indicators']])}
import old_indicators  # noqa: E402  (argv-free since it only reads sys.argv at import: REPO SKATE2_ROOT ASSETS)
misc = Big(s2game/'data/big/miscboot.big')
shapes = old_indicators.outlines(misc.read(next(e for e in misc.entries if e.path.endswith('visual_indicators_sk82.rx2'))))
styles = {k: style_of(k) for k in sorted({l['style'] for l in looks.values()})}
print('indicator styles', {k: s['texture'] for k, s in styles.items()})
(OUT/'indicator').mkdir(parents=True, exist_ok=True)
(OUT/'indicator/indicators.json').write_text(json.dumps({
    'shapes': {k: shapes[k] for k in sorted({l['shape'] for l in looks.values()})}, 'styles': styles, 'locators': looks},
    separators=(',', ':')), encoding='utf-8')
ids = {i for r in rows.values() for i in [r['title'], *r['messages'].values()]}
(OUT/'contests.json').write_text(json.dumps({
    'source': 'Skate 2 (Europe) db.big ChallengeCore vault + missions.big streams; old_contests.py',
    'rows': rows,
    'strings': {k: val for k, val in sorted(text.items()) if k.startswith(STRING_PREFIXES) or k in ids},
}, indent=1), encoding='utf-8')
print(len(rows), 'rows,', len(looks), 'indicators ->', OUT)

# ---------------------------------------------------------------- the voting HUD (--voting HUD_DIR)
# old_online.py exports hudonlinevoting in old_hud's manifest: its imported button component (controls/, character
# 28) is missing and its placements keep their clip events, both of which the runtime refuses. old_screens.movie is the
# importing export (bundles merged, construct-only clip events -> placement `init`, the button bank): the same movie
# through it, with old_online's font mappings and language (read from HUD_DIR's hudonlinevoting.json).
if '--voting' in sys.argv:
    hud = Path(sys.argv[sys.argv.index('--voting') + 1])
    stock = json.loads((hud/'runtime/hudonlinevoting.json').read_text(encoding='utf-8'))
    argv, sys.argv = sys.argv, ['old_hud.py', str(repo), str(s2game), str(OUT/'hud-unused')]
    try:
        import old_hud, old_screens
    finally:
        sys.argv = argv
    old_screens.WORK = old_hud.WORK = WORK_ROOT/'contests-voting-cache'
    old_hud.OUT = OUT/'hud'
    old_screens.MOVIES = {'contests_voting': 'data/fe/source/screens/hud2/hudonlinevoting'}
    cache = old_screens.cache()
    mappings = old_screens.font_mapping(old_screens.collections(), cache)  # registers the FontManager aliases too
    # Its "CURRENT VOTE:" label is authored in Arial, which resolves to the debug font bank but has no FontManager
    # row: the debug font's own layout (its row is "NOT FOR USE IN APT", file debug).
    mappings.setdefault('Arial', stock['font_mappings']['NOT FOR USE IN APT'])
    manifest = old_screens.movie(cache, old_screens.MOVIES['contests_voting'], mappings, stock['language'])
    old_hud.install({'contests_voting': manifest})
    print('voting HUD ->', OUT/'hud', '(install its contents into hud-skate2/online/, new files only)')
