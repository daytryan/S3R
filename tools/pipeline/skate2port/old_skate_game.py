"""Skate 2's game of S.K.A.T.E. (ChallengeCore template "skate") for crates/skate-game/src/skate2_skate_game.rs.

Writes OUT (a scratch folder; copy it to assets/private/skate2-online/skate/ -- nothing here touches an install):
  rules.json         the template's numbers and every S.K.A.T.E. row: spot / start volumes, attempt spawn, waiting and
                     indicator locators (missions/<row>/cSim_Global.xsf; `indicator_locators` names each indicator's
                     look), its intro scene, failure line, time limit
  indicator/indicators.json  old_indicators' format for skate2_arrows.rs's Arrows.more: each VisualIndicators locator's
                     type outline (type -> model -> asset) and style, and those outlines (as old_hall_of_meat)
  scripts/skate.lua  the template's step handlers, decompiled (the rules' source: setup_skate .. show_results)
  strings.json       the English strings the mode and its screens show
  gestures.json      scorable identifier -> [Skate 2 label, gesture_item export ('none': no art), TrickType]: the skater
                     vault trick row's first tTrickGuideInputSlot gesture, what hudabd's gesture_item attaches as g_<name>
  tutorial.json      {"scenes": ...} + scene/camera/<scene>.json: each row's IntroPresentationEvents type 5 flythrough
                     (scene_000.bin + ice_000.bin, baked by old_tutorial.py's ICE evaluation) for challenge::cutscene
  screens/runtime/   hudabd (gesture_item merged in), leaderboardhud, hudthrowmes: old_screens.movie manifests whose
  screens/assets/    payloads are named from the Skate 2 tutorial layer's root (private/skate2-tutorial/screens), the
                     apt_screen canvas they draw on: '../../skate2-online/skate/screens/assets/...'
The timer, countdown and central message are the tutorial's own hudtimer / hudcountdown / hudcentral.

usage (numpy, PIL, java for unluac): py -3 old_skate_game.py REPO SKATE2_ROOT OUT
then the sounds:  cargo run -p skate-data --example s2_fe_sounds -- SKATE2_ROOT work/skate-cache/skater-collections.json OUT/screens
"""
import json, os, re, struct, sys
from pathlib import Path

REPO, S2, OUT = (Path(a) for a in sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
OUT = OUT.resolve()
# old_hud / old_screens / old_tutorial read argv REPO SKATE2_ROOT ASSETS: their default outputs land in scratch.
sys.argv[3] = str(OUT/'_unused')
sys.path.insert(0, str(here))
import old_hud  # noqa: E402
import old_screens  # noqa: E402

S = 'data/fe/source/screens/'
MOVIES = {'hudabd': S + 'hud2/hudabd', 'leaderboardhud': S + 'hud2/leaderboardhud', 'hudthrowmes': S + 'hud2/hudthrowmes'}
WORK = WORK_ROOT/'skate-cache'
SCREENS = OUT/'screens'
# Payload paths as the Skate 2 layer's canvas (root private/skate2-tutorial/screens) resolves them.
PREFIX = '../../skate2-online/skate/screens/'
old_hud.WORK = old_screens.WORK = WORK
old_hud.OUT = old_screens.OUT = SCREENS

src = (here/'old_tutorial.py').read_text(encoding='utf-8')
tut = {'__file__': str(here/'old_tutorial.py'), '__name__': 'old_tutorial'}
exec(compile(src[:src.index('# ---------------------------------------------------------------- main')], 'old_tutorial.py', 'exec'), tut)
tut['OUT'] = OUT  # scenes() writes scene/camera/<scene>.json under it

TEMPLATE = 'skate'
# Row fields the port reads (named by old_tutorial's ALIAS / name lists).
GUIDE = 'Hash_03ACC9F200807B87'   # tTrickGuideInputSlot[] (old_screens.GUIDE_FIELD)
LABEL = 'Hash_843613E915014627'   # the trick's name string
INDICATOR_TYPE, INDICATOR_MODEL = 'Hash_3297556BECB605E2', 'Hash_FB5FDAB5DC2BD228'  # skater vault classes, as old_contests
ORANGE = 'D7EDBD362D7D2152'  # the tutorial file's "default" orange style (a row authored with style 0 gets it)


def write(rel, data):
    path = OUT/rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=1) if not isinstance(data, str) else data, encoding='utf-8')


def skate_rows(v):
    """The template and every row under it (career, online and test rows alike)."""
    root = tut['key_hash'](TEMPLATE)
    out = {}
    for (cls, _), r in v.rows.items():
        row, chain = r, []
        while cls == 'challenges' and row and row['parent']:
            chain.append(tut['key_hash'](row['parent']))
            row = v.row(cls, row['parent'])
        if root in chain or (cls == 'challenges' and r['key'] == TEMPLATE):
            merged = {}
            for c in tut['SELF_CLASSES']:
                part = v.row(c, r['key'])
                for k, val in (v.fields(part) if part else {}).items():
                    merged.setdefault(k, val)
            out[r['key']] = merged
    return out


def lua(v, fields):
    text, handlers = [f'-- {TEMPLATE}: Skate 2 ChallengeCore handlers, decompiled by unluac'], []
    for step in v.refs(fields['Hash_3CB7BA2AFC9BDE42']):
        step = v.fields(step)
        for handler in v.refs(step['Hash_810394ECEFE7E48B']) if 'Hash_810394ECEFE7E48B' in step else ():
            handler = v.fields(handler)
            size, offset = struct.unpack('>II', bytes.fromhex(handler['Hash_C3CD4B4183274819']['data']))
            handlers.append((f'-- [{step["Name"]["data"]}] {handler["Name"]["data"]}', v.bin[offset:offset + size]))
    tut['prefetch']([chunk for _, chunk in handlers])  # unluac runs side by side (old_tutorial.prefetch)
    for head, chunk in handlers:
        _, source = tut['decompile'](chunk)
        text.append(f'{head}\n{source.strip()}\n')
    return '\n'.join(text)


def volume(placement, ref):
    """A tTriggerVolumeInstanceID ("DIST_x|name|0xid") as its world box."""
    name = ref['volume'].split('|')[1] if ref and ref.get('volume') else ''
    box = placement[1].get(name)
    return box and {'name': name, 'min': box['min'], 'max': box['max']}


def rows_json(v, rows):
    template = v.named(rows[TEMPLATE], skip=('Steps', 'Objectives'))
    out = {}
    for name, fields in sorted(rows.items()):
        f = v.named(fields, skip=('Steps', 'Objectives'))
        if name == TEMPLATE or not f.get('SpotVolumes'):
            continue  # skate2_multi / skate2_online / ... are mode templates, no spot
        placement = tut['mission_placement'](name)
        if not placement[0] and name.endswith('_ol'):  # the career pro row's online copy plays on its mission's placement
            placement = tut['mission_placement'](name[:-3])
        if not placement[0]:
            print(name, ': no mission placement, skipped')
            continue
        intro = [e['a'] for e in f.get('IntroPresentationEvents', []) if e['type'] == 5]
        out[name] = {
            'title': f['Title'], 'description': f['Description'],
            # S2's online S.K.A.T.E. list (skate2-online/rows.json): the ol_ and t_ rows and the career row's _ol copy.
            'online': not name.startswith('m_') or name.endswith('_ol'),
            'time_limit': f['TimeLimit'],
            'failure': f['ModifiedCentralMessageHALIDSkateFailureExplanation'],
            'intro': intro[0] if intro else None,
            'spawn': placement[0].get(f['TurnBasedAttemptSpawnPoint']),
            'wait': placement[0].get(f['TurnBasedWaitingLocation']),
            'indicators': [placement[0].get(i['locator']) for i in f.get('VisualIndicators', [])],
            'indicator_locators': [i['locator'] for i in f.get('VisualIndicators', [])],
            'start_volume': volume(placement, f['TurnBasedStartVolume']),
            'spot_volumes': [volume(placement, s) for s in f['SpotVolumes']],
            'boundary': volume(placement, f.get('ChallengeBoundary')),
        }
        missing = [k for k, val in out[name].items() if val is None and k not in ('intro', 'boundary')] + \
                  [k for k in ('spot_volumes', 'indicators') if None in out[name][k]]
        if missing:  # the career / test rows are bonus; the four online rows must place
            print(name, ': unplaced', missing, '- skipped')
            assert not out.pop(name)['online'], name
    return {
        'template': {k: template[k] for k in sorted(template) if k.startswith(('CentralMessage', 'ModifiedCentral'))},
        # ABDListItemLimit is an Int8, 5 on the disc: 5 letters, and 5 tricks in the list.
        'max_letters': template['ABDListItemLimit'],
        # skate.lua's timers: wait_for_leader / wait_for_follower 5 s before the attempt runs anyway, the follower's 0.25 s
        # settle, 2 s after a set / copy / failed set, 3 s + 2.5 s + 2 s around a letter, 2 s before the results.
        'timings': {'pre_attempt': 5.0, 'follower_settle': 0.25, 'after_result': 2.0, 'letter_wait': 3.0,
                    'letter_show': 2.5, 'letter_after': 2.0, 'winner': 2.0},
        # aircompleted: Trick.GetTrickTypeFromScorableID(FirstScorableId) must be Flip (2) or Air (1).
        'valid_trick_types': [1, 2],
        'rows': out,
    }


def indicators(v, sk, rows, rules):
    """indicator/indicators.json in old_indicators' format, for skate2_arrows.rs's Arrows.more: each exported row's
    VisualIndicators locator with its type's outline (type -> model -> asset, as old_hall_of_meat) and style. Styles
    stay the tutorial file's: every online row's is ORANGE, and the career row's copy of its _ol twin's locator (style
    "orange", the same texture) yields to the online row's."""
    import old_indicators

    def field(cls, key, name):
        row = sk.row(cls, key)
        while row:
            if name in row['fields']:
                return row['fields'][name]['data']
            row = sk.row(cls, row['parent']) if row['parent'] else None
    looks = {}
    for name in sorted(rules['rows'], key=lambda n: rules['rows'][n]['online']):  # online rows last: theirs win
        for i in v.named(rows[name], skip=('Steps', 'Objectives')).get('VisualIndicators', []):
            w = i['words']
            model = field(INDICATOR_TYPE, int('%08X%08X' % (w[10], w[11]), 16), 'Hash_8D090C8B5931547A')
            shape = model and field(INDICATOR_MODEL, int(model[16:32], 16), 'Hash_1AFE1B981FB92727')
            assert shape, f'{name}: {i["locator"]} has no indicator type'
            style = '%08X%08X' % (w[16], w[17])
            looks[i['locator']] = {'shape': shape, 'style': ORANGE if style == '0' * 16 else style}
    assert {l['style'] for l in looks.values()} == {ORANGE}, 'a S.K.A.T.E. indicator style the tutorial file lacks'
    misc = tut['Big'](S2/'data/big/miscboot.big')
    shapes = old_indicators.outlines(misc.read(next(e for e in misc.entries if e.path.endswith('visual_indicators_sk82.rx2'))))
    write('indicator/indicators.json', json.dumps({
        'shapes': {k: shapes[k] for k in sorted({l['shape'] for l in looks.values()})}, 'styles': {}, 'locators': looks},
        separators=(',', ':')))
    return looks


def gestures(v):
    """Every engine scorable identifier the Skate 2 vault has a trick row for."""
    hash64 = tut['hash64']
    catalog = (REPO/'crates/skate-core/src/scoring/catalog.rs').read_text(encoding='utf-8')
    names = re.findall(r'^\s*\("([a-z0-9_]+)", \d+, \d+\),', catalog, re.M)
    rows = {}
    for (cls, key), r in v.rows.items():
        f = v.fields(r)
        if GUIDE in f:
            rows.setdefault(key, f)
    out = {}
    for name in names:
        f = rows.get(hash64(name))
        if f is None:
            continue
        slots = [int(i[:8], 16) for i in f[GUIDE]['array']['items']]
        # ponytail: slots past old_screens' table (0x30: boneless / fastplant) name no g_ export we know: no art.
        table = old_screens.GUIDE_GESTURES
        gesture = next((table[s] if s < len(table) else 'none' for s in slots if s), 'none')
        kind = next((f[k] for k in ('TrickType', 'Type') if k in f), None)
        kind = v.value(kind) if kind else None
        out[name] = [v.value(f[LABEL]) if LABEL in f else None, gesture, kind[0] if isinstance(kind, list) else kind]
    return out


def screens():
    project = old_hud.project
    project.BigArchive = old_hud.S2Archive
    project.find_big_directory = lambda root: Path(root)/'data'
    project.CORE_ARCHIVES = ('fe/screens.big', 'big/miscboot.big')
    project.extract_project(old_hud.s2game, WORK, prefixes=(*MOVIES.values(), *old_screens.SHARED), force=True)
    c = old_hud.AssetCache(WORK)
    mappings = old_hud.font_mapping(old_hud.collections(), c)
    for name, (family, apt) in old_screens.STAND_INS.items():
        c.add_font_alias(name, family, mappings[apt])
        mappings[name] = mappings[apt] | {'stand_in': apt}
    language = old_hud.s2lang.strings(old_hud.s2game)
    manifests = {stem: old_screens.movie(c, name, mappings, language) for stem, name in MOVIES.items()}
    old_hud.install(manifests)  # runtime/<stem>.json + payloads under SCREENS
    for stem, manifest in manifests.items():
        text = json.dumps(manifest, separators=(',', ':'))
        for rel in old_hud.payloads(manifest):
            text = text.replace(json.dumps(rel), json.dumps(PREFIX + rel))
        (SCREENS/'runtime'/(stem + '.json')).write_text(text + '\n', encoding='utf-8')
    return language


if __name__ == '__main__':
    vaults = tut['db_vaults']()
    tut['vaults'] = vaults
    cc = vaults['ChallengeCore']
    rows = skate_rows(cc)
    write('scripts/skate.lua', lua(cc, rows[TEMPLATE]))
    rules = rows_json(cc, rows)
    print(len(rules['rows']), 'rows:', ', '.join(rules['rows']))
    scenes = tut['scenes']({r['intro'] for r in rules['rows'].values() if r['intro']})
    for name, scene in list(scenes.items()):
        if scene['tracks'] or 'camera' not in scene:  # ponytail: actor intros (career pro rows) would need old_tutorial's anim export
            print('intro', name, 'has actors or no camera - dropped')
            (OUT/'scene/camera'/(name + '.json')).unlink(missing_ok=True)
            del scenes[name]
    for row in rules['rows'].values():
        row['intro'] = row['intro'] if row['intro'] in scenes else None
    tut['post_audio'](scenes, old_hud.s2game/'data/audio/post.big', OUT)
    write('tutorial.json', {'scenes': scenes})
    write('rules.json', rules)
    print(len(scenes), 'intro scenes;', len(indicators(cc, vaults['skater'], rows, rules)), 'indicator looks')
    g = gestures(vaults['skater'])
    write('gestures.json', g)
    print(len(g), 'trick gestures; kickflip', g.get('kickflip'), 'heelflip', g.get('heelflip'))
    language = screens()
    keep = re.compile(r'^ID_(SKATE_|LEADERBOARD_SKATE|CHALLENGE_CENTRAL_MESSAGE_|CHALLENGE_OL_SKATE|CHALLENGE_MPCM_SKATE|'
                      r'WRNGTRK_|TRICK_|CHALLENGE_[A-Z0-9_]*SKATE|MISSION_COUNTDOWN|COMMON_UNKNOWN)')
    write('strings.json', {k: v for k, v in sorted(language.items()) if keep.match(k) and not k.startswith(('ID_SKATE_REEL', 'ID_SKATE_PROFILE'))})
    print('done ->', OUT)
