"""Skate 2's Freeskate Activities on New_San_Vanelona: ChallengeType 42, FreeskateActivityType 1 = Gap, 2 = Tag.

Writes into ASSETS/private/skate2-activities/ (an export dir; there is no --install yet, the runtime is not built):
  activities.json  every online activity row (AvailableOnline, not DebugOnly: old_online.py's `freeskate_activities`)
                   with what its graph reads: title, description, time_limit, goal_adjustment (FreeskateGoalAdjustment,
                   the raw Int32), the central message, the location {name, matrix, slots} (globallocators.big, else the
                   row's mission), the highlight definition, and per objective (ChallengeCore Objectives, in order):
                   key, text, its snippet (Lua source, what the objectives manager runs), the trigger volumes it names
                   (name, instance id, box: the row's mission cSim_Global.xsf; the snippet's id must be the volume's),
                   the trick groups it names, its other natives, its visual indicators (placed by the mission's
                   locators), DisplayInHUD / TriggerActivationMode / Triggers / MasterObjective, PointsValid, the
                   intro / complete / fail events and FailDefinition. Plus `trick_groups` (members, old_tutorial's
                   trick_groups) and `strings` (Skate 2's English text of every id the rows, snippets and graphs name).
                   Each objective also carries `def`, an `ObjectiveDef` as crates/skate-game/src/skate3_objectives.rs loads it (snippets as
                   {src, tree, natives} from skate3_online.snippet_tree, the 20 trigger bytes, the activation in Skate 3's numbering:
                   activity_convert.py); each row a `group` (a `GroupDef`, None when a snippet was rejected), its `trigger_volumes`
                   and a `conversion` count; the top level `snippet_sources` (src -> Lua text, the oracle's input).
  scripts/<row>.lua  each row's ChallengeCore handlers, decompiled by unluac (the oracle; not installed)
  steps.json       the oracle: the step shape the rows share, each handler's variants (by normalised source) and the
                   rows that differ from the most common variant

Nothing is approximated: a value the data does not give is listed under `unrecovered`.
usage (java on PATH or SKATE_JAVA; SKATE_UNLUAC; SKATE_TABLES): py -3 old_activities.py REPO SKATE2_ROOT ASSETS
"""
import collections, json, os, re, struct, sys, tempfile, time
from pathlib import Path

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
import s2lang
from tools.online_challenge_map import global_locators, compact_json
from tools.asset_pipeline.vlt import hash64
import activity_convert as convert


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
OUT = assets/'private/skate2-activities'
AVAILABLE_ONLINE, DEBUG_ONLY = 'Hash_F9A9A1DDED84A643', 'Hash_FD40E4A31027C984'
ACTIVITY = 42
KINDS = {1: 'gap', 2: 'tag'}   # FreeskateActivityType (eFreeskateType)

# ---------------------------------------------------------------- vault
vaults = T['db_vaults']()
T['vaults'] = vaults           # challenge() reads the skater vault for a tutorial movie
v = vaults['ChallengeCore']
text = s2lang.strings(s2game)
sources = {}                   # unluac output by function name (h_<handler key>), filled by decompile below
decompiled = {}


def decompile(chunk, _plain=T['decompile']):
    """old_tutorial.decompile, once per distinct chunk, retried when Windows still holds its temp .luac (WinError 32,
    as old_online.py); keeps each handler's own source for the oracle."""
    if chunk not in decompiled:
        for attempt in range(5):
            try:
                decompiled[chunk] = _plain(chunk)
                break
            except PermissionError:
                if attempt == 4:
                    raise
                time.sleep(2)
    function, source = decompiled[chunk]
    sources[function] = source
    return function, source


T['decompile'] = decompile     # challenge() looks it up in old_tutorial's globals


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


def raw(m, name):
    return m.get(name) or m.get('Hash_%016X' % hash64(name))


def int16(m, name):
    """An EA::Reflection::Int16 field: the top half of its word (FreeskateGoalAdjustment "000F0000" = 15, as
    skate3_online.decode reads Int16; old_tutorial's Vault.scalar reads the whole word, 983040)."""
    f = raw(m, name)
    assert f is None or f['type'] == 'EA::Reflection::Int16', (name, f and f['type'])
    return f and struct.unpack('>h', bytes.fromhex(f['data'])[:2])[0]


challenges = {r['key']: r for r in v.rows.values() if r['class'] == 'challenges'}
parents = {r['parent'] for r in challenges.values()}
keys = [k for k in sorted(challenges) if k not in parents and get(merged(k), 'ChallengeType') == ACTIVITY
        and get(merged(k), AVAILABLE_ONLINE) and not get(merged(k), DEBUG_ONLY)]

# ---------------------------------------------------------------- placement
glb = {}
gbig = Big(s2game/'data/big/globallocators.big')
for e in gbig.entries:
    if e.path.lower().startswith('data/content/global_locators/'):
        for k, loc in global_locators(gbig.read(e)).items():
            glb.setdefault(k, loc)
missions = Big(s2game/'data/content/missions.big')
mission_files = {}
for e in missions.entries:
    parts = e.path.lower().split('/')
    if len(parts) >= 4 and parts[-3] == 'missions':
        mission_files.setdefault(parts[-2], {})[Path(e.path).name] = e


def mission(key):
    """(locators {name lower: {matrix, slots}}, volumes {name lower: {id, matrix, min, max}}) of missions/<key>: as
    old_online.mission, plus each volume's instance id (@0xB8, old_tutorial.mission_placement). A missions/ folder name
    is at most 30 characters (ol_fa_megapark_02_gap_woodenramps streams missions/ol_fa_megapark_02_gap_woodenra/)."""
    files = mission_files.get(key.lower()) or mission_files.get(key.lower()[:30])
    if not files:
        return {}, {}
    sim = next(n for n in files if n.lower().endswith('_sim.xst'))
    locators, volumes = {}, {}
    with tempfile.TemporaryDirectory(prefix='skate2-activities-') as work:
        work = Path(work)
        for n, e in files.items():
            (work/n).write_bytes(missions.read(e))
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
                        volumes[full.split('_0x', 1)[0].lower()] = {
                            'id': struct.unpack_from('>Q', d, o + 0xB8)[0], 'matrix': T['matrix'](d, o),
                            'min': list(struct.unpack_from('>3f', d, o + 0x40)),
                            'max': list(struct.unpack_from('>3f', d, o + 0x50))}
    return locators, volumes


def placed(name, local):
    loc = glb.get(name.lower()) or local.get(name.lower()) if name else None
    return loc and {'name': name, 'matrix': loc['matrix'], 'slots': loc['slots'],
                    'source': 'globallocators' if name.lower() in glb else 'mission'}


# ---------------------------------------------------------------- snippets
ENTERED = re.compile(r'ObjectiveTrigger\.(\w+)\(\s*(\d+)\s*,\s*"([^"|]*)\|([^"]+)"\s*\)')
NATIVE = re.compile(r'\b([A-Z]\w*)\.(\w+)\s*\(')


def snippet(src, volumes, unrecovered, where):
    """The volumes, trick groups and other natives a snippet names; each volume placed by name, its id checked."""
    out = {'volumes': [], 'trick_groups': sorted(set(re.findall(r'DidSpecificTrickGroup\w*\("(\w+)"\)', src))),
           'natives': sorted({f'{a}.{b}' for a, b in NATIVE.findall(src)})}
    for call, ident, district, name in ENTERED.findall(src):
        box = volumes.get(name.lower())
        entry = {'call': call, 'name': name, 'district': district, 'id': int(ident),
                 **({k: box[k] for k in ('matrix', 'min', 'max')} if box else {})}
        if box is None:
            unrecovered.append(f'{where}: volume {name} (not in the row\'s mission)')
        elif box['id'] != int(ident):
            # Both are retail (ol_fa_soho_11_gap_planters: the snippet's 0x0005CF1A03E38706 is no volume of its
            # mission; the mission's volume_1 is 0x0005D1E103E38706). Whether EnteredVolume matches the id or the
            # name is not traced.
            entry['mission_id'] = box['id']
            unrecovered.append(f'{where}: {name}: the snippet\'s instance id {int(ident):#018x} is not the mission '
                               f'volume\'s {box["id"]:#018x} (which one EnteredVolume matches is untraced)')
        out['volumes'].append(entry)
    return out


# ---------------------------------------------------------------- rows
rows, unrecovered, scripts, steps_of = [], [], {}, {}
snippets = convert.Snippets()
for key in keys:
    c, lua = T['challenge'](v, key)
    s = c['self']
    # The missions the row streams: its RequiredChallengeHull's (ol_fa_waterfront_05_gap_circlestairs:
    # ol_fa_waterfront_03_gap_nob), else its own.
    hulls = [h for hull in s.get('RequiredChallengeHull') or [] for h in hull if h] or [key]
    local, volumes = {}, {}
    for h in hulls:
        more_locators, more_volumes = mission(h)
        if not (more_locators or more_volumes):
            unrecovered.append(f'{key}: mission {h} (no missions/ folder)')
        local.update(more_locators)
        volumes.update(more_volumes)
    scripts[key] = lua
    steps_of[key] = c['steps']
    row = {'id': key, 'missions': hulls, 'activity': s['FreeskateActivityType'], 'kind': KINDS.get(s['FreeskateActivityType']),
           'title': s['Title'], 'description': s.get('Description'), 'time_limit': s['TimeLimit'],
           # setup_challenge: TimeLimit * (1 + (NumSkaters - 2) * FreeskateGoalAdjustment / 100).
           'goal_adjustment': int16(merged(key), 'FreeskateGoalAdjustment'),
           'objective_complete_message': s.get('ModifiedCentralMessageHALDIDObjectiveComplete'),
           'location': placed(s.get('Location'), local), 'highlight': s.get('HighlightDefinition'),
           'disable_hom': s.get('DisableHOM'), 'allow_session_markers': s.get('AllowSessionMarkersToBeSet'),
           # setup_common adds self.VisualIndicators (the row's own; each objective has its own too).
           'visual_indicators': [vi['locator'] for vi in s['VisualIndicators']] if 'VisualIndicators' in s else None}
    if row['visual_indicators'] is None:
        unrecovered.append(f'{key}: row VisualIndicators unset in every record of its chain (setup_common iterates '
                           'self.VisualIndicators.Count: the default of an unset field is not traced)')
    if not row['location']:
        unrecovered.append(f'{key}: location {s.get("Location")}')
    row['objectives'] = []
    raw_objectives = [v.fields(r) for r in v.refs(merged(key)['Objectives'])]
    assert len(raw_objectives) == len(c['objectives']), key
    for i, o in enumerate(c['objectives']):
        where = f'{key} objective {i}'
        definition = o.get('Definition') or {}
        entry = {'key': o['Key'], 'text': definition.get('text') or None, 'source': definition.get('source'),
                 **snippet(definition.get('source') or '', volumes, unrecovered, where),
                 'visual_indicators': [], 'display_in_hud': o.get('DisplayInHUD'),
                 'activation': o.get('TriggerActivationMode'), 'triggers': o.get('Triggers'),
                 'master': o.get('MasterObjective'), 'requires_highlight': o.get('RequiresHighlight'),
                 'points_valid': (o.get('PointsValid') or [None])[0],
                 'intro_events': o.get('IntroEvents'), 'complete_events': o.get('CompleteEvents'),
                 'fail_events': o.get('FailEvents'), 'fail_definition': o.get('FailDefinition'),
                 'highlight': o.get('HighlightDefinition'), 'hints': o.get('Hints')}
        for vi in o.get('VisualIndicators') or []:
            loc = local.get(vi['locator'].lower()) if vi['locator'] else None
            if vi['locator'] and not loc:
                unrecovered.append(f'{where}: visual indicator {vi["locator"]}')
            entry['visual_indicators'].append({'locator': vi['locator'], 'matrix': loc and loc['matrix']})
        entry['def'] = convert.objective_def(entry, raw_objectives[i], snippets, where)
        row['objectives'].append(entry)
    # Everything the runtime loads, or none of it: a rejected snippet must not become the empty handler.
    mine = [r for r in snippets.rejected if r[0].startswith(key + ' ')]
    defs = [o['def'] for o in row['objectives']]
    by_id = {str(b['id']): dict(name=n, **b) for n, b in volumes.items()}
    row['group'] = None if mine else {'PointRequirement': None, 'objectives': defs}
    row['trigger_volumes'] = convert.trigger_volumes(defs, by_id)
    row['conversion'] = {'snippets': sum(1 for o in row['objectives'] for t in (o['source'], o['points_valid']) if (t or '').strip()),
                         'converted': sum(1 for d in defs for t in (d['obj'], d['points_valid']) if t),
                         'rejected': [{'where': w, 'error': e} for w, e in mine]}
    for w, e in mine:
        unrecovered.append(f'{w}: the Skate 3 snippet parser rejects it ({e})')
    rows.append(row)

joined = '\n'.join(o['source'] or '' for r in rows for o in r['objectives'])
groups = T['trick_groups'](v, joined)
# A text field holds a string id, or retail's literal text after '#' (ol_fa_ot_17_gap_cathedralexit's title).
ids = {x for r in rows for x in (r['title'], r['description'], r['objective_complete_message']) if x and x.startswith('ID_')}
ids |= {o['text'] for r in rows for o in r['objectives'] if o['text'] and o['text'].startswith('ID_')}
ids |= set(re.findall(r'"(ID_[A-Z0-9_]+)"', '\n'.join(scripts.values()) + joined))
unrecovered += [f'string {i} (no Skate 2 English text)' for i in sorted(ids) if i not in text]


# ---------------------------------------------------------------- oracle (steps.json)
def normalised(src, key):
    """A handler's source with its function name and its row's own names masked."""
    src = re.sub(r'^function h_\w+\(', 'function h(', src.strip(), flags=re.M)
    return re.sub(re.escape(key), '<row>', src, flags=re.I)


shapes = collections.Counter(json.dumps({s: sorted(h) for s, h in st.items()}, sort_keys=True) for st in steps_of.values())
variants = {}
for key, st in steps_of.items():
    for step, handlers in st.items():
        for handler, function in handlers.items():
            variants.setdefault(f'{step}.{handler}', collections.defaultdict(list))[normalised(sources[function], key)].append(key)
oracle = {'rows': len(steps_of), 'shapes': [{'rows': n, 'steps': json.loads(s)} for s, n in shapes.most_common()],
          'step_order': {k: list(st) for k, st in steps_of.items()}, 'handlers': {}}
for name, by_source in sorted(variants.items()):
    ranked = sorted(by_source.items(), key=lambda kv: -len(kv[1]))
    oracle['handlers'][name] = {'variants': len(ranked), 'common_rows': len(ranked[0][1]), 'common': ranked[0][0],
                                'others': [{'rows': sorted(r), 'source': s} for s, r in ranked[1:]]}

# ---------------------------------------------------------------- write
(OUT/'scripts').mkdir(parents=True, exist_ok=True)
for key, lua in scripts.items():
    (OUT/f'scripts/{key}.lua').write_text(lua, encoding='utf-8', newline='\n')
(OUT/'steps.json').write_text(json.dumps(oracle, indent=1) + '\n', encoding='utf-8', newline='\n')
(OUT/'activities.json').write_text(compact_json({
    'map': 'New_San_Vanelona', 'source': 'Skate 2 ChallengeCore (ChallengeType 42, AvailableOnline, not DebugOnly)',
    'kinds': KINDS, 'rows': rows, 'trick_groups': dict(sorted(groups.items())), 'snippet_sources': snippets.sources, 'strings': {i: text[i] for i in sorted(ids) if i in text},
    'unrecovered': unrecovered}) + '\n', encoding='utf-8', newline='\n')
print(len(rows), 'activities', dict(collections.Counter(r['kind'] for r in rows)), ';', len(shapes), 'step shapes;',
      sum(1 for h in oracle['handlers'].values() if h['variants'] > 1), 'handlers with per-row variants;',
      len(unrecovered), 'unrecovered')
