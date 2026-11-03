"""Skate 2's Hall of Meat for the engine's skate2_hall_of_meat.rs (New_San_Vanelona).

Writes OUT (install it as assets/private/skate2-online/hall-of-meat/; never point OUT at the install, it overwrites):
  runtime/<stem>.json   the HoM Flash screens as apt_screen runs them (old_screens.movie's manifest): homscoring (the
                        bail score HUD, C++'s HOMScoringHUD), hom_results (the body damage results screen, SP only),
                        leaderboardhud (the online LeaderboardHUD the HoM template shows)
  assets/...            their RGBA payloads (shapes, Skate 2 font pages)
  natives.json          per movie, the identifiers its ActionScript names (what apt_screen must answer)
  hom.json              the rules: the skater vault's Sk8::Score::HoM row (bones, damage levels, bonus curves, entity
                        hits, body tweaks) decoded, the hall_of_meat template's fields, every online row (ol_hom_*) with
                        its attempt / waiting / start locators in NSV world space, its intro scene and its neon
                        indicators, and the HoM strings (English)
  tutorial.json         {"scenes": ...} the rows' intro flythroughs (NISFlythroughDefinition) as challenge/cutscene.rs
  scene/camera/*.json   reads them (the file name is the one cutscene.rs loads), cameras baked by old_tutorial's ICE code
  scripts/hall_of_meat.lua  the template's Lua (decompiled), for reference: the rules around a bail are Lua, the bail
                        score itself is C++ (Trick.GetHOMScore -> Score::HoM::WipeoutScorer)
sfx/<key>.wav is written separately by `cargo run -p skate-data --example s2_fe_sounds -- SKATE2_ROOT
work/hom-cache/skater-collections.json OUT` (it scans OUT/runtime for the movies' FE_SOUND_ keys).

Evidence: HALL_OF_MEAT_REPORT.md.
usage (numpy, PIL; java on PATH): py -3 old_hall_of_meat.py REPO SKATE2_ROOT OUT
"""
import json, os, struct, sys
from pathlib import Path

repo, s2game, OUT = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
# old_hud / old_screens parse argv as REPO SKATE2_ROOT ASSETS; their OUT/WORK are redirected below before any use.
sys.argv[1:4] = [str(repo), str(s2game), str(OUT)]
sys.path[:0] = [str(repo), str(here)]
import old_hud, old_screens

S = 'data/fe/source/screens/'
MOVIES = {
    'homscoring': S + 'hud2/homscoring',
    'hom_results': S + 'hudComponents/hom_results',
    'leaderboardhud': S + 'hud2/leaderboardhud',
}
WORK = WORK_ROOT/'hom-cache'
old_screens.MOVIES, old_screens.WORK, old_screens.OUT = MOVIES, WORK, OUT
old_hud.WORK, old_hud.OUT = WORK, OUT

# Sk8::Score::HoM row (skater vault class 9E4C471BC6EAB615 'default'), decoded against the exe's WipeoutScorer
# (0x82bb8398 per frame while wiped out; see the report). Bone record index = the scorer's bone enum.
HOM_CLASS = 0x9E4C471BC6EAB615
BONES = ['NECK1', 'NECK', 'LEFTHAND', 'LEFTFOREARM', 'LEFTARM', 'LEFTSHOULDER', 'RIGHTHAND', 'RIGHTFOREARM', 'RIGHTARM',
         'RIGHTSHOULDER', 'SPINE3', 'SPINE2', 'SPINE1', 'SPINE', 'LEFTTOEBASE', 'LEFTFOOT', 'LEFTLEG', 'LEFTUPLEG',
         'RIGHTTOEBASE', 'RIGHTFOOT', 'RIGHTLEG', 'RIGHTUPLEG', 'HIPS', 'FACE', 'GROIN']
LEVELS = ['Scrape', 'Bruise', 'Sprain', 'Dislocate', 'BreakPart', 'BreakAll']
CURVES = {0xC2D28EFB: 'leg_impact', 0x96CDCC59: 'air_time', 0x32B1A3C2: 'drop', 0xB2A997F8: 'speed',
          0x6CD9F0CD: 'rotation', 0x2451424A: 'bail_time'}
SCALARS = {0xB7514DE7: 'force_scale', 0x72A3CCC3: 'toe_share', 0x9C494E96: 'tweak_stick', 0x8FB03ECD: 'hurt_damage',
           0xF54CF357: 'dmo_points', 0x96276C72: 'ped_points', 0xFC8A190E: 'vehicle_points', 0xD6CB5A92: 'decay'}
BONE_SET, TWEAKS, PEDS, VEHICLES = 0x1F9B5EA1, 0x5717CFCC, 0x9E620977, 0xFF1CED9B
HOM_TYPE, AVAILABLE_ONLINE, DEBUG_ONLY = 31, 'Hash_F9A9A1DDED84A643', 'Hash_FD40E4A31027C984'
INDICATOR_TYPE, INDICATOR_MODEL = 'Hash_3297556BECB605E2', 'Hash_FB5FDAB5DC2BD228'
STRING_PREFIXES = ('ID_HOM', 'ID_CHALLENGE_CENTRAL', 'ID_CONTEST_CENTRALMESSAGE', 'ID_COMMON_METRIC', 'ID_CHALLENGE_OL_HOM',
                   'ID_COMMON_SCORE', 'ID_MISSION_TEMPLATE_HALL', 'ID_MISSION_TEMPLATE_ONLINE_HALL', 'ID_MISSION_TEMPLATE_HOM')


def screens():
    c = old_screens.cache()
    vault = old_hud.collections()
    mappings = old_hud.font_mapping(vault, c)
    for name, (family, apt) in old_screens.STAND_INS.items():
        c.add_font_alias(name, family, mappings[apt])
        mappings[name] = mappings[apt] | {'stand_in': apt}
    language = old_hud.s2lang.strings(s2game)
    manifests = {stem: old_screens.movie(c, name, mappings, language) for stem, name in MOVIES.items()}
    # The engine's FELanguage.GetLocalizedFormatStringCustom fills printf-style % args; Skate 2's typed {0:speed} /
    # {0:distance} formatters are the plugin's, which hands the movie the formatted text (HOM_GetBonusData).
    manifests['homscoring']['language'].update(ID_HOM_SPEED_VALUE='%s', ID_COMMON_METRIC_METERS='%s')
    old_hud.install(manifests)
    (OUT/'natives.json').write_text(json.dumps({s: old_screens.natives(m) for s, m in manifests.items()}, indent=1), encoding='utf-8')
    return manifests, language


def tutorial_lib():
    """old_tutorial.py's library half (below `vaults = db_vaults()` it runs and rmtrees its own output)."""
    src = (here/'old_tutorial.py').read_text(encoding='utf-8')
    ns = {'__name__': 'old_tutorial_lib', '__file__': str(here/'old_tutorial.py')}
    exec(compile(src.split('\nvaults = db_vaults()\n')[0], str(here/'old_tutorial.py'), 'exec'), ns)
    ns['OUT'] = OUT  # scenes() writes scene/camera/ under it
    return ns


def by_top(fields):
    """Vault fields by the top 32 bits of their key hash (how the scorer's offsets were matched to keys)."""
    return {int(k[5:13], 16): f for k, f in fields.items() if k.startswith('Hash_')}


def rules(T, v):
    # Exact f32 values (no rounding): the scorer port (skate2_hall_of_meat.rs) is bit-exact on them.
    row = next(r for r in v.rows.values() if T['key_hash'](r['class']) == HOM_CLASS and r['key'] == 'default')
    f = by_top(row['fields'])
    raw = bytes.fromhex(f[BONE_SET]['data'])
    bones = []
    for i in range(len(raw) // 84):
        r = struct.unpack_from('>i' + 'if' * 4 + 'If' * 6, raw, i * 84)
        assert r[0] == i, f'bone record {i} names bone {r[0]}'
        bones.append({'name': BONES[i],
                      'links': [[r[1 + 2 * k], r[2 + 2 * k]] for k in range(4) if r[1 + 2 * k] >= 0],
                      'levels': [[r[9 + 2 * k], r[10 + 2 * k]] for k in range(6)]})
    curves = {}
    for top, name in CURVES.items():
        c = struct.unpack('>20f', bytes.fromhex(f[top]['data']))
        curves[name] = {'x': list(c[4:12]), 'y': list(c[12:20]), 'max': [c[2], c[3]]}
    scalars = {}
    for top, name in SCALARS.items():
        t, data = f[top]['type'], bytes.fromhex(f[top]['data'])
        scalars[name] = struct.unpack('>f', data)[0] if t.endswith('Float') else int.from_bytes(data, 'big')
    tweaks = []
    for item in f[TWEAKS]['array']['items']:
        i, _, x, y, frames, points = struct.unpack('>iIffII', bytes.fromhex(item))
        tweaks.append({'id': i, 'stick': [x, y], 'frames': frames, 'points': points})
    entities = lambda top: [list(struct.unpack('>II', bytes.fromhex(i))) for i in f[top]['array']['items']]
    return {'levels': LEVELS, 'bones': bones, 'curves': curves, **scalars, 'tweaks': tweaks,
            'peds': entities(PEDS), 'vehicles': entities(VEHICLES)}


def indicator_shape(T, sk, kind):
    """A VisualIndicators type key -> the outline asset id old_indicators exports (type -> model -> asset)."""
    def field(cls, key, name):
        row = sk.row(cls, key)
        while row:
            if name in row['fields']:
                return row['fields'][name]['data']
            row = sk.row(cls, row['parent']) if row['parent'] else None
    model = field(INDICATOR_TYPE, int(kind, 16), 'Hash_8D090C8B5931547A')
    return model and field(INDICATOR_MODEL, int(model[16:32], 16), 'Hash_1AFE1B981FB92727')


def mission_locators(T, key):
    """{name lower: {matrix, slots}} of the row's mission stream (missions.big missions/<key>/, its cSim_Global.xsf RW4
    assets), where the observer and indicator locators live (as old_online.mission)."""
    from tools.online_challenge_map import global_locators
    import tempfile
    big = MISSIONS.setdefault('big', T['Big'](s2game/'data/content/missions.big'))
    files = {Path(e.path).name: e for e in big.entries if e.path.lower().split('/')[-2:-1] == [key.lower()]}
    sim = next((n for n in files if n.lower().endswith('_sim.xst')), None)
    if not sim or 'cSim_Global.xsf' not in files:
        return {}
    out = {}
    with tempfile.TemporaryDirectory(prefix='skate2-hom-') as work:
        work = Path(work)
        for n, e in files.items():
            (work/n).write_bytes(big.read(e))
        for asset in T['read_sfil'](work/'cSim_Global.xsf', T['read_atoc'](work/sim)):
            if asset.data[1:4] == b'RW4':
                out.update(global_locators(asset.data))
    return out


MISSIONS = {}


def rows(T, vaults, locators):
    v = vaults['ChallengeCore']
    challenges = {r['key']: r for r in v.rows.values() if r['class'] == 'challenges'}
    parents = {r['parent'] for r in challenges.values()}
    out, scenes = {}, set()
    for key in sorted(k for k in challenges if k not in parents):
        m = {}
        for c in T['SELF_CLASSES']:
            row = v.row(c, key)
            if row:
                for k, val in v.fields(row).items():
                    m.setdefault(k, val)
        get = lambda k: v.value(m[k]) if k in m else None
        try:
            s = v.named(m, skip=('Steps', 'Objectives'))
        except (KeyError, ValueError, struct.error):  # rows of other modes with fields named() can't read
            continue
        if s.get('ChallengeType') != HOM_TYPE or not get(AVAILABLE_ONLINE) or get(DEBUG_ONLY):
            continue
        # The mission stream is the row's key, or (03, 13, 15, 16) the stem its locators share.
        stem = (s.get('TurnBasedAttemptSpawnPoint') or '').lower().removesuffix('_challengelocator')
        local = mission_locators(T, key) or mission_locators(T, stem)
        find = lambda name: name and (locators.get(name) or locators.get(name.lower()) or local.get(name.lower()))
        place = lambda name: find(name) and {'locator': name, **find(name)}
        nis = s.get('NISFlythroughDefinition')
        intro = nis[0] if isinstance(nis, list) and nis and isinstance(nis[0], str) and nis[0] else None
        if intro:
            scenes.add(intro)
        indicators = []
        for r in s.get('VisualIndicators') or []:
            w = r['words']
            shape = indicator_shape(T, vaults['skater'], '%08X%08X' % (w[10], w[11]))
            if find(r['locator']) and shape:  # (one row in 14_alley is authored empty: type and style 0)
                indicators.append({'locator': r['locator'], 'matrix': find(r['locator'])['matrix'], 'shape': shape,
                                   'style': '%08X%08X' % (w[16], w[17])})
        out[key] = {'title': s.get('Title'), 'time_limit': s.get('TimeLimit'), 'num_rounds': s.get('NumRounds'),
                    'attempt': place(s.get('TurnBasedAttemptSpawnPoint')) or place(s.get('StartLocation')),
                    'waiting': place(s.get('TurnBasedWaitingLocation')),
                    'start': place(s.get('StartLocation')) or place(s.get('TurnBasedAttemptSpawnPoint')),
                    'intro': intro, 'indicators': indicators,
                    'messages': {k: s.get('CentralMessageHALID' + k) for k in ('YourTurn', 'OutOfTime', 'ChallengeComplete', 'RoundOver')},
                    'cumulative': s.get('ContestCumulativeScoring')}
        missing = [k for k in ('attempt', 'waiting') if not out[key][k]]
        if missing:
            print(key, 'unresolved', missing, s.get('TurnBasedAttemptSpawnPoint'), s.get('TurnBasedWaitingLocation'))
    return out, scenes


def indicators(T, hom_rows):
    """indicator/indicators.json in old_indicators' format, for skate2_arrows.rs's Arrows.more: the outlines the rows
    use and each indicator locator's look. Styles stay the tutorial file's (every HoM style is D7EDBD362D7D2152, its
    orange; a row authored with style 0 gets it too)."""
    import old_indicators
    misc = T['Big'](s2game/'data/big/miscboot.big')
    shapes = old_indicators.outlines(misc.read(next(e for e in misc.entries if e.path.endswith('visual_indicators_sk82.rx2'))))
    looks = {i['locator']: {'shape': i['shape'], 'style': ORANGE if i['style'] == '0' * 16 else i['style']}
             for r in hom_rows.values() for i in r['indicators']}
    assert {l['style'] for l in looks.values()} == {ORANGE}, 'a HoM indicator style the tutorial file lacks'
    (OUT/'indicator').mkdir(exist_ok=True)
    (OUT/'indicator/indicators.json').write_text(json.dumps({
        'shapes': {k: shapes[k] for k in sorted({l['shape'] for l in looks.values()})}, 'styles': {}, 'locators': looks},
        separators=(',', ':')), encoding='utf-8')


ORANGE = 'D7EDBD362D7D2152'


def rest(language):
    T = tutorial_lib()
    vaults = T['db_vaults']()
    T['vaults'] = vaults  # challenge() reads it as a global
    # globallocators.big's RX2 locators (lower case, with per-slot spawns), then BAM_Sim.loc (world) for the rest: the
    # observer (waiting) locators are only in the former, and BAM_Sim.loc's copy of a challengelocator is the same root
    # without its children (every attempt locator has 6-9 on the disc; RepositionAI resolves child[slot], S2
    # 0x82879950), so the global one wins, as in old_contests. (Until 10-12 the world copy won: no attempt slots.)
    from tools.online_challenge_map import global_locators
    gbig = T['Big'](s2game/'data/big/globallocators.big')
    locators = {}
    for e in gbig.entries:
        if e.path.lower().startswith('data/content/global_locators/'):
            for k, loc in global_locators(gbig.read(e)).items():
                locators.setdefault(k, loc)
    locators = {k: {'matrix': m, 'slots': []} for k, m in T['world_locators']().items()} | locators
    hom_rows, wanted = rows(T, vaults, locators)
    scenes = T['scenes'](wanted)
    for name, s in scenes.items():
        assert not s['tracks'], f'{name}: an intro with actors (cutscene.rs would need their scene/anim banks)'
    for r in hom_rows.values():
        if r['intro'] and r['intro'] not in scenes:
            print(r['intro'], 'has no scene; the row plays without an intro')
            r['intro'] = None
        r['intro_seconds'] = r['intro'] and scenes[r['intro']]['duration']
    template, lua = T['challenge'](vaults['ChallengeCore'], 'hall_of_meat')
    t = template['self']
    hom = {
        'source': 'Skate 2 (Europe) db.big skater + ChallengeCore vaults; see HALL_OF_MEAT_REPORT.md',
        'rules': rules(T, vaults['skater']),
        'template': {k: t.get(k) for k in ('TimeLimit', 'NumRounds', 'ContestCumulativeScoring')} | {
            'messages': {k: t.get('CentralMessageHALID' + k) for k in ('YourTurn', 'OutOfTime', 'ChallengeComplete', 'RoundOver')}},
        'rows': hom_rows,
        'strings': {k: val for k, val in sorted(language.items()) if k.startswith(STRING_PREFIXES)
                    or any(k == r['title'] for r in hom_rows.values())},
    }
    (OUT/'hom.json').write_text(json.dumps(hom, indent=1), encoding='utf-8')
    indicators(T, hom_rows)
    (OUT/'tutorial.json').write_text(json.dumps({'scenes': scenes}, indent=1), encoding='utf-8')
    (OUT/'scripts').mkdir(exist_ok=True)
    (OUT/'scripts/hall_of_meat.lua').write_text(lua, encoding='utf-8')
    return hom_rows, scenes


if __name__ == '__main__':
    OUT.mkdir(parents=True, exist_ok=True)
    manifests, language = screens()
    print(len(manifests), 'screens ->', OUT)
    hom_rows, scenes = rest(language)
    print(len(hom_rows), 'rows,', len(scenes), 'intros ->', OUT)
