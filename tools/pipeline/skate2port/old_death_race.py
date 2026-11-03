"""Skate 2's Death Races on New_San_Vanelona: the data crates/skate-game/src/skate2_death_race.rs runs.

Every ChallengeType 19 race of the downhill (RaceType 0) and trick-for-time (RaceType 1) kinds, single-player and online
(`_ol`; an online row id is its Online Challenge Map row id). Their rules are the shared race template's Lua (handlers
decompiled by unluac; read in skate2_death_race.rs's notes), not C++: the engine ports those rules over this data.

Writes OUT (assets/private/skate2-online/death-race/), never deleting anything there:
  death_race.json   per race: title, kind, gate rules, its one heat (time limit, start + its per-racer slots, legs ->
                    gates [volume, time bonus, indicator type, ribbon style] + split triggers in order, music triggers), the chevrons, the online
                    end camera, the online intro (scene per player count, placed by the heat's NIS definition), and
                    the race's locators and trigger volumes (world AABBs; Skate 2 world space = NSV map space;
                    `missing`: names no mission folder has, retail gaps such as kotm_3c's gates 2-5);
                    plus the indicator outlines and styles, and the strings the race shows
  tutorial.json     the intro scenes in challenge::cutscene's format (it reads <root>/tutorial.json): one per race,
                    scene and local slot ("<row>|<scene>|<slot>", the player on its track) and a camera-only one
                    ("<row>|<scene>")
  scene/camera/<scene>.json  the ICE cameras, evaluated by old_tutorial.py's scene_camera
  scene/anim/<bank>.abin     the scenes' actor clip banks, as on the disc
  indicator/<texture>.png    vitextures.rx2's visual_indicator_* ribbon cross-sections and volume_indicator_* textures

Write to a scratch OUT, then copy the new files into the install (none of them has an installation.json receipt).

usage (numpy, PIL; after old_screens.py, which caches the skater vault):
  py -3 old_death_race.py REPO SKATE2_ROOT OUT [TELEPORTS_JSON]
TELEPORTS_JSON (the install's private/teleports.json) checks that BAM_Sim.loc locators are the engine's NSV frame.
"""
import json, math, struct, sys, tempfile
from pathlib import Path

repo, s2game, OUT = sys.argv[1], sys.argv[2], Path(sys.argv[3])
teleports_json = Path(sys.argv[4]) if len(sys.argv) > 4 else None
here = Path(__file__).resolve().parent


def load(script, name):
    """A sibling export's helpers, without running its main (old_tutorial's main rmtree's its own OUT)."""
    src = (here/script).read_text(encoding='utf-8')
    cut = src.find('# ---------------------------------------------------------------- main')
    g = {'__file__': str(here/script), '__name__': name}
    argv, sys.argv = sys.argv, ['x', repo, s2game, str(OUT/'_unused')]
    try:
        exec(compile(src[:cut] if cut >= 0 else src, script, 'exec'), g)
    finally:
        sys.argv = argv
    return g


T = load('old_tutorial.py', '__old_tutorial__')
I = load('old_indicators.py', '__old_indicators__')
T['OUT'] = OUT  # scenes() writes scene/camera/<scene>.json under it
Big = T['Big']
S2 = Path(s2game)

TYPE, MODEL, STYLE = I['TYPE'], I['MODEL'], I['STYLE']
# eChallengeIndicators, read from default.xex's enum registration (0x82b76228); the race uses 2..4.
INDICATORS = {'None': 0, 'Obstacle': 1, 'RaceNextGate': 2, 'RaceGates': 3, 'RaceFinish': 4}
STRINGS = ['ID_RACE_SECONDS_ADDED', 'ID_RACE_FINISHED', 'ID_MISSION_CHALLENGE_FAILED', 'ID_RACE_VARIANT_DOWNHILL',
           'ID_RACE_VARIANT_TRICKFORTIME', 'ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE']


def f32(w):
    return struct.unpack('>f', struct.pack('>I', w))[0]


def hexkey(hi, lo):
    return '%08X%08X' % (hi, lo)


def definition(words):
    """A NIS definition row {scene, flags 256, 0, 0, position xyz + pad, direction xyz + pad, 4 x 0}: the scene and
    where NIS.Enter*UsingDefinition places it. Empty ones stay non-nil in retail (the online intro reads them)."""
    if not words:
        return {'scene': '', 'at': [0, 0, 0], 'dir': [0, 0, 1]}
    return {'scene': words[0], 'at': [round(f32(w), 4) for w in words[4:7]], 'dir': [round(f32(w), 4) for w in words[8:11]]}


def yaw(d):
    return math.atan2(d[0], d[2])


def main():
    vaults = T['db_vaults']()
    v = vaults['ChallengeCore']
    rows = lambda refs: [v.named(v.fields(v.row(v.classes[(w[0] << 32) | w[1]], (w[2] << 32) | w[3]))) for w in refs]
    # 'DIST_<district>|<volume>|<id>' -> the volume's name; an unset reference is empty.
    volume = lambda t: t['volume'].split('|')[1] if '|' in t['volume'] else None
    names = sorted({r['key'] for (c, _), r in v.rows.items() if c == 'challenges'})

    world = T['world_locators']()
    if teleports_json:
        # The engine's NSV teleports are BAM_Sim.loc locators placed as they are: the same frame as everything here.
        nsv = [t for t in json.loads(teleports_json.read_text(encoding='utf-8'))['destinations'] if t['map'] == 'New_San_Vanelona']
        checked = [t for t in nsv if t['locator'] in world]
        assert checked, 'no NSV teleport is a BAM_Sim.loc locator'
        for t in checked:
            assert all(abs(a - b) < 1e-3 for ra, rb in zip(t['matrix'], world[t['locator']]) for a, b in zip(ra, rb)), t['locator']
        print(f'frame: {len(checked)} NSV teleports equal their BAM_Sim.loc locators')

    races, scene_names, styles, shapes_wanted = {}, set(), set(), set()
    placements = {}
    for name in names:
        m = {}
        for row in filter(None, [v.row(c, name) for c in T['SELF_CLASSES']]):
            for k, val in v.fields(row).items():
                m.setdefault(k, val)
        s = v.named(m, skip=('Steps', 'Objectives'))
        # race_shared & co. are templates; the races are r_* (and bl_*).
        if not name.startswith(('r_', 'bl_')) or s.get('ChallengeType') != 19 or s.get('RaceType') not in (0, 1) or 'RaceHeats' not in s:
            continue
        heats = rows(s['RaceHeats'])
        assert len(heats) == 1, f'{name}: {len(heats)} heats'
        heat = heats[0]
        legs = []
        for leg in rows(heat['Legs']):
            gates = []
            for gate in rows(leg['RandomGates']):
                r = gate['RibbonInstance']
                styles.add(hexkey(r[8], r[9]))
                shapes_wanted.add(hexkey(r[2], r[3]))
                gates.append({'volume': volume(gate['GateVolume']), 'bonus': gate['Time_Bonus'], 'indicator': gate['IndicatorType'],
                              'ribbon': hexkey(r[2], r[3]), 'style': hexkey(r[8], r[9]),
                              'audio': gate['Hash_DC41AFD60FDAD913']})  # eRaceGateAudio (0 Standard)
            legs.append({'gates': gates, 'splits': [volume(t) for t in leg['SplitTimeTriggers']], 'end_bonus': leg['EndTimeBonus']})
        ribbons = {}
        for key, field in (('grey', 'RaceGateGreyRibbon'), ('final', 'RaceFinalGateRibbon'), ('leg_finish', 'RaceLegFinishGateRibbon')):
            r = s[field]
            ribbons[key] = {'ribbon': hexkey(r[2], r[3]), 'style': hexkey(r[8], r[9])}
            styles.add(ribbons[key]['style'])
            shapes_wanted.add(ribbons[key]['ribbon'])
        chevrons = []
        for row in s.get('VisualIndicators') or []:
            w = row['words']
            chevrons.append({'locator': row['locator'], 'type': hexkey(w[10], w[11]), 'style': hexkey(w[16], w[17])})
            styles.add(chevrons[-1]['style'])
            shapes_wanted.add(chevrons[-1]['type'])
        sp, online = (definition(heat[k]) for k in ('NISFlythroughDefinition', 'NISFlythroughDefinitionOnline'))
        online_nis = [v.string(int(p, 16)) if isinstance(p, str) else p for p in heat['OnlineNISs']]
        scene_names |= set(online_nis)
        races[name] = {
            'online': name.endswith('_ol'),
            'title': s.get('Title'), 'description': s.get('Description'),
            'race_type': s['RaceType'], 'skipable': bool(s.get('RaceGateSkipable')),
            'trick_multiplier': s.get('RaceTrickForTimeMultiplier') or 0,  # Int16
            'start': s.get('StartLocation'), 'end_camera': s.get('OnlineEndCameraLocation') or None,
            'grey_icon': s.get('RaceGateGreyIcon'), 'ribbons': ribbons, 'chevrons': chevrons,
            # end_heat: placed within PlaceHigherToWin wins; cleanup_heat shows ShowChallengeComplete/FailedMessage.
            'place_to_win': s.get('PlaceHigherToWin') or 1,  # Int16 (1 on every race)
            'complete': s.get('CentralMessageHALIDChallengeComplete') or None,
            'failed': s.get('CentralMessageHALIDChallengeFailed') or None,
            'heat': {'time_limit': heat['TimeLimit'], 'include321': bool(heat['Include321']), 'start': heat['StartLocation'],
                     'knockout': heat['KnockoutPerLeg'], 'no_pushing': bool(heat['NoPushing']),
                     'wipeout_limit': heat['WipeOutLimit'], 'legs': legs,
                     'terminate': [volume(t) for t in heat['RaceTerminateVolumes']],
                     'music': [{'trigger': volume(p['Trigger']), 'progress': p['Progress']} for p in rows(heat['MusicPace'])],
                     'nis': sp, 'nis_online': online, 'online_nis': online_nis},
            'hulls': [h[0] for h in s.get('RequiredChallengeHull') or []],
        }

    # Placement: each race's mission streams (folder names are cut to 30 characters), then the world's locators, then
    # any other mission folder (a few races name a locator or gate that lives in a sibling race's stream).
    def placement(hull):
        if hull not in placements:
            placements[hull] = T['mission_placement'](hull)[:2]
            if not any(placements[hull]) and len(hull) > 30:
                placements[hull] = T['mission_placement'](hull[:30])[:2]
        return placements[hull]

    def others(wanted):
        missions = Big(S2/'data/content/missions.big')
        folders = sorted({e.path.split('/missions/', 1)[1].split('/', 1)[0] for e in missions.entries if '/missions/' in e.path})
        found = {}
        for f in sorted(folders, key=lambda f: not f.startswith(('r_', 'kotm', 'c_'))):
            if wanted <= set(found):
                break
            loc, vol = placement(f)
            for n in wanted - set(found):
                if n in loc or n in vol:
                    found[n] = (f, loc.get(n), vol.get(n))
        return found

    def resolve(race, extra):
        locators, volumes = {}, {}
        for hull in race['hulls']:
            loc, vol = placement(hull)
            locators.update(loc)
            volumes.update(vol)
        for n, (_, loc, vol) in extra.items():
            locators.setdefault(n, loc) if loc else volumes.setdefault(n, vol)
        return locators, volumes

    for pass_, extra in (('own', {}), ('others', None)):
        if pass_ == 'others':
            wanted = {n for race in races.values() for n in race['missing']}
            extra = others(wanted) if wanted else {}
            for n, (f, *_) in sorted(extra.items()):
                print(f'placement: {n} from missions/{f}')
        for name, race in races.items():
            if pass_ == 'others' and not race['missing']:
                continue
            locators, volumes = resolve(race, extra)
            heat = race['heat']
            want_loc = {race['start'], heat['start'], race['end_camera']} | {c['locator'] for c in race['chevrons']}
            want_vol = {g['volume'] for leg in heat['legs'] for g in leg['gates']} | {t for leg in heat['legs'] for t in leg['splits']}
            want_vol |= set(heat['terminate']) | {m['trigger'] for m in heat['music']}
            if None in want_vol and pass_ == 'own':
                print(f'{name}: an unset volume reference')
            want_vol.discard(None)
            race['locators'], missing = {}, []
            for n in sorted(x for x in want_loc if x):
                at = locators.get(n) or world.get(n)
                (missing.append(n) if at is None else race['locators'].__setitem__(n, at))
            race['volumes'] = {n: {'min': volumes[n]['min'], 'max': volumes[n]['max']} for n in sorted(want_vol) if n in volumes}
            missing += sorted(want_vol - set(race['volumes']))
            race['missing'] = missing
            if missing and pass_ == 'others':
                print(f'{name}: missing {missing}')

    # The heat start's per-player children: the heat's teleport (AI.RepositionAI(id, CurrentHeatData.StartLocation, id,
    # false, 0) after the fade-out) puts competitor id on child id. Mission locator records are global_locators' records.
    from tools.online_challenge_map import global_locators
    missions = Big(S2/'data/content/missions.big')
    children = {}

    def slots_in(folder):
        if folder not in children:
            children[folder] = {}
            for name in (folder, folder[:30]):
                files = {Path(e.path).name: e for e in missions.entries if f'/missions/{name}/'.lower() in e.path.lower()}
                sim = next((n for n in files if n.lower().endswith('_sim.xst')), None)
                if not sim:
                    continue
                with tempfile.TemporaryDirectory(prefix='skate2-dr-') as work:
                    work = Path(work)
                    T['write_streams'](work, files, missions.read, sim, 'cSim_Global.xsf')  # the two read below
                    for asset in T['read_sfil'](work/'cSim_Global.xsf', T['read_atoc'](work/sim)):
                        if asset.data[:4] == b'\x89RW4':
                            children[folder].update(global_locators(asset.data))
                break
        return children[folder]
    for race in races.values():
        start = race['heat']['start']
        folders = race['hulls'] + [f for n, (f, *_) in extra.items() if n == start]
        race['heat']['slots'] = next((loc['slots'] for f in folders if (loc := slots_in(f).get(start.lower()))), [])
    for race in races.values():
        del race['hulls']

    # The online intro (setup_heat): scene OnlineNISs[clamp(NumCompetitors - 1, 1, n)], placed by the online
    # definition, or the single-player one when the online one is empty (default.xex NIS.EnterOnlineNISUsingDefinition
    # 0x826dac08: it copies the online definition, falls back to the other when its name is empty, then names the scene).
    table = T['scenes'](scene_names)
    present = [n for n in races[next(iter(races))]['heat']['online_nis'] if n in table]
    scenes, banks = {}, set()
    for name, race in races.items():
        heat = race['heat']
        place = heat['nis_online'] if heat['nis_online']['scene'] else heat['nis']
        intro = []
        for wanted in heat['online_nis']:
            # ponytail: Online_RaceStarts_07/08 (7 and 8 players) are not in scene_000.bin; the 6-player one stands in.
            scene = wanted if wanted in table else present[-1]
            data = table[scene]
            # The scene's tracks and camera are local (racers on identity TRAJECTORYs at the origin, facing +z); the
            # definition puts that frame at `at`, +z along `dir` (the start slots face it too, within ~0.1 rad). The
            # scenes' own dir (yaw -0.726) is RaceStarts_1's world facing at kotm_2, copied, and not applied.
            # ponytail: pitch dropped; the exe's placement maths (past 0x826dac08) wasn't traced.
            turn = yaw(place['dir'])
            key = f'{name}|{scene}'
            base = {'origin': place['at'], 'turn': round(turn, 6), 'camera': data.get('camera')}
            scenes[key] = dict(base, tracks=[])
            for slot, track in enumerate(data['tracks']):
                scenes[f'{key}|{slot}'] = dict(base, tracks=[{'character': 'current_player', 'anim': track['anim']}])
                banks.add(track['anim'])
            intro.append({'scene': scene, 'seconds': data['duration'], 'tracks': len(data['tracks'])})
        race['intro'] = intro if place['scene'] else []
    scene_big = Big(S2/'data/big/scene.big')
    anims = {e.path.removeprefix('data/'): e for e in scene_big.entries if e.path.startswith('data/scene/anim/')}
    (OUT/'scene/anim').mkdir(parents=True, exist_ok=True)
    for a in sorted(banks):
        (OUT/a).write_bytes(scene_big.read(anims[a]))
    (OUT/'tutorial.json').write_text(json.dumps({'scenes': scenes}, separators=(',', ':')), encoding='utf-8')

    # Indicators: outlines, styles and the ribbon textures (old_indicators.py's sources).
    misc = Big(S2/'data/big/miscboot.big')
    files = {Path(e.path).name: e for e in misc.entries}
    work = OUT/'_work'
    work.mkdir(parents=True, exist_ok=True)
    (work/'vitextures.rx2').write_bytes(misc.read(files['vitextures.rx2']))
    textures = {}
    (OUT/'indicator').mkdir(parents=True, exist_ok=True)
    for t in I['read_textures'](work/'vitextures.rx2')[1]:
        tname = t.name.removesuffix('.Texture')
        if tname.startswith(('visual_indicator_', 'volume_indicator_')):
            file = tname + '.png'
            pixels = I['rgba'](t).copy()
            # The colour swatches are tints (alpha unused); background and volume_* keep theirs.
            if tname.startswith('visual_indicator_') and tname != 'visual_indicator_background':
                pixels[..., 3] = 255
            I['Image'].fromarray(pixels).save(OUT/'indicator'/file)
            textures[t.name] = file
            print(f"texture {tname}: {t.metadata['width']}x{t.metadata['height']} format {t.metadata['format_code']}")
    (work/'vitextures.rx2').unlink()
    work.rmdir()
    outlines = I['outlines'](misc.read(files['visual_indicators_sk82.rx2']))
    vrows = {(c['class'], c['key']): c for c in json.loads(I['VAULT'].read_text(encoding='utf-8'))['collections']}

    def field(cls, key, fname):
        while (c := vrows.get((cls, key))) is not None:
            if fname in c['fields']:
                return c['fields'][fname]['data']
            key = c['parent']
        return None
    from tools.asset_pipeline.vlt import hash64
    named = {'%016X' % hash64(k): k for k in ('default', 'orange', 'red', 'yellow', 'blue', 'green', 'grey', 'white')}

    def style(key):
        k = named.get(key, 'Hash_' + key)
        tint = field(STYLE, k, 'Hash_E4F3418597AACAA5')
        return {'name': named.get(key), 'texture': textures[field(STYLE, k, 'Hash_8E38A3CD0186F8DA')],
                'background': textures.get(field(STYLE, k, 'Hash_FF4DCA217E05B9BC')),
                'tint': list(struct.unpack('>4f', bytes.fromhex(tint))) if tint else [1, 1, 1, 1]}

    def shape(key):
        model = field(TYPE, 'Hash_' + key, 'Hash_8D090C8B5931547A')[16:32]
        return field(MODEL, 'Hash_' + model, 'Hash_1AFE1B981FB92727')
    shape_of = {k: shape(k) for k in sorted(shapes_wanted)}
    assert all(a in outlines for a in shape_of.values()), shape_of

    # Strings
    boot = {e.path.lower(): e for e in misc.entries}
    labels = T['table'](misc.read(boot['data/fe/languages/labels/language_labels_global_skate2ng.bin']))
    values = T['table'](misc.read(boot['data/fe/languages/english/language_english_global_skate2ng.bin']))
    text = {labels[k]: values[k] for k in labels if k in values}
    wanted = set(STRINGS) | {r[k] for r in races.values() for k in ('title', 'description', 'complete', 'failed') if r.get(k)}
    strings = {k: text[k] for k in sorted(wanted) if k in text}
    print('strings missing:', sorted(wanted - set(strings)))

    out = {'map': 'New_San_Vanelona', 'indicators': INDICATORS,
           'shapes': {k: outlines[a] for k, a in shape_of.items()},
           'styles': {k: style(k) for k in sorted(styles)},
           'strings': strings, 'races': races}
    (OUT/'death_race.json').write_text(json.dumps(out, separators=(',', ':')), encoding='utf-8')

    # ---- self-checks
    online = [n for n, r in races.items() if r['online']]
    for n in online:
        r = races[n]
        # Retail data gaps (no mission folder has them): kotm_3c gates 2-5, kotm_4 MPchev_01 chevrons, kotm_5a gate 5.
        # Gates only add time and draw a ribbon (the splits drive progress), so the race runs without them, as on disc.
        needed = {r['heat']['start']} | {t for leg in r['heat']['legs'] for t in leg['splits']}
        assert not needed & set(r['missing']), (n, r['missing'])
        if r['missing']:
            print(f'{n}: retail gap {r["missing"]}')
        assert r['heat']['knockout'] == 0 and not r['heat']['no_pushing'] and r['heat']['wipeout_limit'] == 0, n
        assert r['intro'], f'{n}: no online intro'
        assert all(scenes[f"{n}|{i['scene']}"]['camera'] for i in r['intro']), n
        # The start and every gate sit in the NSV world (ground heights 0..300 m, inside the map's +-2 km).
        p = r['locators'][r['heat']['start']][3]
        assert all(abs(c) < 2500 for c in p[:3]), (n, p)
        # The racers' slots (world matrices; some sets stand 20-45 m off their parent locator, as authored).
        assert r['heat']['slots'] and all(math.dist(s[3][:3], p[:3]) < 50 for s in r['heat']['slots']), n
    print('heat start slots:', {n: len(races[n]['heat']['slots']) for n in online})
    print('place to win / end messages:', {(races[n]['place_to_win'], races[n]['complete'], races[n]['failed']) for n in online})
    # The online end camera: which locator row points at the finish (the engine's camera looks along it).
    for n in online:
        r = races[n]
        cam = r['locators'].get(r['end_camera'] or '')
        if not cam:
            continue
        fin = r['volumes'][r['heat']['legs'][-1]['splits'][-1]]
        c = [(a + b) / 2 - cam[3][i] for i, (a, b) in enumerate(zip(fin['min'], fin['max']))]
        length = math.sqrt(sum(x * x for x in c)) or 1
        print(f'{n}: end camera rows . finish = ' + ' '.join(f'{sum(cam[k][i] * c[i] for i in range(3)) / length:+.2f}' for k in range(3)))
    print(f'{len(races)} races ({len(online)} online), {len(scenes)} intro scene entries, {len(banks)} clip banks, '
          f'styles {[(k, s["name"], s["texture"]) for k, s in out["styles"].items()]}, shapes {shape_of}')


if __name__ == '__main__':
    main()
