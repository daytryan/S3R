"""Skate 1's online modes on Skate_1_San_Vanelona: rows, spawns, online areas, intros and the online player HUD.

Writes into ASSETS/private/ (an export dir; --install copies it into an installation):
  skate1-online/rows.json        every Skate 1 online row (skater.vlt challenge rows: AvailableOnline, not DebugOnly; the
                                 six type-29 teleporters left out) by mode: English name, description, start {matrix,
                                 slots} / boundary / oob_time (online.json's shape), the row's named vault fields (`self`,
                                 string-pool refs resolved), every place they name (`places`: locators with their spawn
                                 slots, trigger volumes), intro scenes, Lua template, each VisualIndicators record's type
                                 (`indicator_types`)
  skate1-online/intros/          tutorial.json + scene/camera/<scene>.json: every pre-match flythrough the Skate 1 mode
                                 Lua plays (NISFlyThroughScene; Best Trick's per-round NISFlyThroughScenes). Skate 1's
                                 ICE takes (scene.big ice.bin) evaluated by old_tutorial's ICE code with Skate 1's own
                                 parameter table (ice_params1.json, from default.xex) and its shake curves (ice.bin's
                                 Jog/Still/WalkFast/WalkSlow records). Same format as skate2-online/intros, so
                                 challenge::cutscene's Cutscene::start(<assets>/private/skate1-online/intros, scene) plays one.
  skate1-online/scripts/<template>.lua  every mode template's handlers, decompiled (reference; not installed)
  hud-skate1/online/runtime/<movie>.json + payloads  the online HUD movies (default.xex's Online* screens, the mode
                                 overlays and the HUD components they load, the intros' spotfound card), old_hud1's
                                 manifest format
  hud-skate1/online/sfx/spot_found.wav  the spotfound card's sound (Audio.PlaySpotFound), fe.big HUD.abk sample 38

--install INST_ASSETS (an installation's assets/ dir):
  - merges the rows into private/challenge-map/online/online.json (backup online.json.pre-skate1online, once):
    challenges into the Versus tab's ID_MAP_FILTER_CHALLENGETYPES_<MODE> folders (a missing one is made as the retail
    folders are: Options header + Quick Match), freeskate locations at the top of Skate 1's freeskate folder. Every entry
    it adds carries "game": "skate1", so a rerun replaces them. game_tabs.apply then moves them into the "#Skate 1" tab.
  - adds ID_MAP_FILTER_CHALLENGETYPES_SPOTRACE (Skate 1 only; no Skate 3 or Skate 2 text) to the `language` of the menu
    and online HUD manifests that carry the challenge type names (backup <file>.pre-skate1online, once);
  - copies the exports above (skate1-online/rows.json, skate1-online/intros/, hud-skate1/online/) as new files.
  Nothing is deleted; none of these files has an installation.json receipt.

Evidence and decisions: ONLINE1_REPORT.md.
usage (java on PATH for unluac; numpy, PIL): py -3 old_online1.py REPO SKATE1_ROOT ASSETS [--install INST_ASSETS]
"""
import json, math, os, re, shutil, struct, sys, time
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
install_to = Path(sys.argv[sys.argv.index('--install') + 1]) if '--install' in sys.argv else None
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
from s2lang import table
import game_tabs
import s1ice
from skate3_streams import decompress_refpack
from tools.online_challenge_map import global_locators, boundary, compact_json


def lib(script, marker):
    """A sibling script's library half (each runs, and rmtrees its own output, below `marker`)."""
    src = (here/script).read_text(encoding='utf-8')
    ns = {'__name__': script[:-3] + '_lib', '__file__': str(here/script)}
    argv, sys.argv = sys.argv, [script, str(repo), str(s1game), str(assets)]
    try:
        exec(compile(src.split(marker)[0], str(here/script), 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


C = lib('old_challenges.py', '# ---------------------------------------------------------------- main')  # S1 vault
T = lib('old_tutorial.py', '\nvaults = db_vaults()')  # ICE camera evaluation
MAP = 'Skate_1_San_Vanelona'
OUT = assets/'private/skate1-online'
HUD_OUT = assets/'private/hud-skate1/online'
FILTER = 'ID_MAP_FILTER_CHALLENGETYPES_'
STEPS, HANDLERS, CHUNK = 'Hash_3CB7BA2AFC9BDE42', 'Hash_810394ECEFE7E48B', 'Hash_C3CD4B4183274819'
# Challenge row fields named by the Lua that reads them (self.<Name>) or by their values. Other unnamed fields are left
# out of rows.json, except the row-level scene ref list (string-pool offsets of scene names).
FIELDS = {'Hash_F9A9A1DDED84A643': 'AvailableOnline', 'Hash_FD40E4A31027C984': 'DebugOnly',
          'Hash_C95DCFB60ACF8393': 'LocationID', 'Hash_0199F910CC5DB28B': 'AvailableDemo',
          'Hash_931D177C12BEA04C': 'NISIntro', 'Hash_493547658FEBD9A3': 'MapIconLocation'}
SCENE_REFS = 'Hash_6266AB67D97AC208'
# eChallengeType -> (match_core mode name, Versus folder filter, icon, the mode's description). Skate 1 has no online map
# icons (its online menu is a list); the Skate 2 minimap markers old_online registered stand in (Spot Race: Death Race's).
MODES = {4: ('skate', 'SKATE', 'mini_skate', 'ID_MISSION_TEMPLATE_SKATE_DESCRIPTION'),
         8: ('best_trick', 'BESTTRICK', 'mini_besttrick', 'ID_MISSION_TEMPLATE_BEST_TRICK_DESCRIPTION'),
         9: ('spot_battle', 'SPOTBATTLE', 'mini_spotfound', 'ID_MISSION_TEMPLATE_ONLINE_SPOTBATTLE_DESCRIPTION'),
         15: ('jam', 'JAM', 'mini_jam', 'ID_MISSION_TEMPLATE_JAM_DESCRIPTION'),
         19: ('death_race', 'DEATHRACE', 'mini_deathrace', 'ID_MISSION_TEMPLATE_ONLINE_DEATH_RACE_DESCRIPTION'),
         20: ('spot_race', 'SPOTRACE', 'mini_deathrace', 'ID_MISSION_TEMPLATE_SPOT_RACE_DESCRIPTION')}
FREESKATE, TELEPORTER = 22, 29
# Versus folders that may be missing: (label, help), Skate 1's own mode name and description.
NEW_FOLDERS = {'BESTTRICK': ('ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK', 'ID_MISSION_TEMPLATE_BEST_TRICK_DESCRIPTION'),
               'JAM': ('ID_MISSION_TEMPLATE_ONLINE_JAM', 'ID_MISSION_TEMPLATE_JAM_DESCRIPTION'),
               'SPOTRACE': ('ID_MISSION_TEMPLATE_SPOT_RACE_TITLE', 'ID_MISSION_TEMPLATE_SPOT_RACE_DESCRIPTION')}
# Lua's Trick.eTrickType: default.xex registers the names in this order (None at 0x820ca840 .. Revert), read as 0..11.
TRICK_TYPES = ['None', 'Air', 'Flip', 'Gap', 'Grab', 'Grind', 'Invert', 'Lip', 'Manual', 'Slide', 'Spin', 'Revert']
FIXED = ('ForfeitTimer', 'CompetitorNumber')  # Int16 (ForfeitTimer in seconds), exported as floats
# Scene kinds (default.xex's scene kind names at 0x820d08fc, in enum order).
KINDS = ['Invalid', 'Flythrough', 'ChallengeWin', 'SkateIntroOffer', 'SkateIntroAccept', 'SkateIntroDecline',
         'SkateOutroWin', 'SkateOutroLose', 'SpotIntro', 'SpotOutro', 'ProIntro', 'ProChallengeIntro',
         'ProChallengeOutro', 'RightOfPassage', 'TutorialIntro', 'TutorialOutroSuccess', 'TutorialOutroFailure']
SCREENS = 'data/fe/source/screens/'
# default.xex's screen table (0x82138bd4..): OnlineTimer, OnlineEventLog, OnlinePlayersList, OnlineSpectatorOverlay,
# HUD_Online_Freeskate_Overlay, DeathRace_Challenge_LeaderBoard and the HUD_<Mode>_Challenge_Overlay screens the mode
# Lua's HUD.ChangeHUDState drives; onlinehud/racehud/timerhud/skatehud are the components those overlays load.
# challenge/spotfound is screen 93, the one an intro's OVERLAY 2-5 (SpotFound_<corner>) opens through the
# StateManagerBackEnd (0x82352018; bug-list item10-flashes.md D1).
HUD_MOVIES = {m.rsplit('/', 1)[-1]: SCREENS + m for m in (
    'hud/hud_online_timer', 'hud/hud_online_eventlog', 'hud/hud_online_players', 'hud/hud_online_freeskate_overlay',
    'hudComponents/spectator_mode', 'hudComponents/leaderboard', 'hudComponents/onlinehud', 'hudComponents/racehud',
    'hudComponents/timerhud', 'hudComponents/skatehud', 'hud/hud_skate/hud_skate_challenge_overlay',
    'hud/hud_besttrick_challenge_overlay', 'hud/hud_jam_challenge_overlay', 'hud/hud_spot_challenge_overlay',
    'hud/hud_deathrace/hud_deathrace_challenge_overlay', 'hud/hud_spotrace_challenge_overlay', 'challenge/spotfound')}
# APT font names -> miscboot data/fonts/<name>.gfn: default.xex's font table (0x82118b88). "Raven Stroke"
# (hud_online_players) is not in it.
GFN = {'TradeBld': 'tradebld', 'NewCourierBlot': 'indytype', 'Bit KitBold': 'bitkitbold',
       'Raven Stroke': 'tradebldstroke'}  # ponytail: Raven Stroke's fallback in the game isn't traced; TradeBld Stroke stands in

# ---------------------------------------------------------------- vault, text
v = C['Vault']()
label_key = C['label_key']
text = {}
english_big = Big(s1game/'data/big/english.big')
for e in english_big.entries:
    if e.path.lower().endswith('.bin') and 'histogram' not in e.path.lower():
        text.update(table(english_big.read(e)))


def english(label):
    return text.get(label_key(label)) if isinstance(label, str) and label else None


def say(label):
    """A row label: Skate 1's English text as a literal (Skate 3's tables lack Skate 1's ids)."""
    return '#' + (english(label) or label)


def resolve(x):
    """Unknown-typed vault values are hex words; a string-pool offset among them is a name (scene refs)."""
    if isinstance(x, list):
        return [resolve(i) for i in x]
    if isinstance(x, str) and re.fullmatch(r'[0-9A-F]{8}', x) and int(x, 16) >= 0x10000:
        return v.string(int(x, 16)) or x
    return x


def fields(key):
    out = {}
    for k, f in v.merged(key).items():
        k = C['ALIAS'].get(k, FIELDS.get(k, k))
        if k == 'Steps' or k.startswith('Hash_') and k != SCENE_REFS:
            continue
        try:
            out[k] = resolve(v.value(f))
        except IndexError:  # contest_challenge's default ScoringBoundary: a bare volume name, no "<district>|"
            out[k] = v.string(int(f['data'][:8], 16))
    if 'SpotTrickTypes' in out:
        out['SpotTrickTypes'] = [TRICK_TYPES[int(x, 16)] for x in out['SpotTrickTypes']]
    for k in FIXED:
        if isinstance(out.get(k), int):
            out[k] = float(out[k])
    return out


lessons = v.lessons
parents = {r['parent'] for r in lessons.values()}


def template(key):
    """The row (itself or an ancestor) whose own fields hold the Steps: the mode script."""
    while key and STEPS not in lessons[key]['fields']:
        key = lessons[key]['parent']
    return key


# ---------------------------------------------------------------- places
# Locators: the mission streams' (missions.big *.rx2: global_locators records, children = per-slot spawns), the world
# stream's Sim assets (BAM.xsf, RefPack'd RW4), then BAM.loc (no spawns). Trigger volumes (0xEB000D 0xA0-byte and
# 0xEB0019 0xF0-byte records: matrix, min, max, name) from the same RW4s. All are in the converted map's frame (checked
# against teleports.json below).
missions = Big(s1game/'data/big/missions.big')
stream = s1game/'data/content/world/stream/BAM'


def rw4_sections(d):
    count, table_at = struct.unpack_from('>I', d, 0x20)[0], struct.unpack_from('>I', d, 0x30)[0]
    return [struct.unpack_from('>6I', d, table_at + 24 * i) for i in range(count)]


def rw4_volumes(d, out, source):
    for at, _, _, _, _, kind in rw4_sections(d):
        if kind not in (0xEB000D, 0xEB0019):
            continue
        _, n, _, first = struct.unpack_from('>4I', d, at)
        size, name_at = (0xA0, 0x88) if kind == 0xEB000D else (0xF0, 0xE0)
        for r in range(n):
            o = at + first + size * r
            ptr = at + struct.unpack_from('>I', d, o + name_at)[0]
            name = d[ptr:d.index(b'\0', ptr)].decode('latin-1').split('_0x', 1)[0]
            m = struct.unpack_from('>16f', d, o)
            out.setdefault(name.lower(), {'name': name, 'source': source, 'matrix': [list(m[i:i + 4]) for i in (0, 4, 8, 12)],
                                          'min': list(struct.unpack_from('>3f', d, o + 0x40)),
                                          'max': list(struct.unpack_from('>3f', d, o + 0x50))})


def world_sims(kind=1):
    """BAM.xsf's assets of one ATOC type (1 the Sims; 4 more RW4s, whose trigger volumes are the only copy of 13 Spot
    Battles' boundaries and spot volumes): each RefPack'd after its 128-byte header. Asset headers chain by their
    stride (u32 @16), so the 2 GB stream is walked without reading its textures."""
    xst = stream.with_suffix('.xst').read_bytes()
    kinds = {struct.unpack_from('>Q', xst, 24 + 64 * i)[0]: struct.unpack_from('>I', xst, 24 + 64 * i + 36)[0]
             for i in range(struct.unpack_from('>I', xst, 16)[0])}
    with open(stream.with_suffix('.xsf'), 'rb') as f:
        size, at = f.seek(0, 2), 0x80
        while at < size:
            f.seek(at)
            asset, stored, header, stride = struct.unpack('>QIII', f.read(20))
            if not asset:
                break
            if kinds[asset] == kind:
                f.seek(at + header)
                d = f.read(stored)
                yield asset, decompress_refpack(d) if d[:2] == b'\x10\xfb' else d
            at += stride


mission_locs, world_locs, volumes, unread = {}, {}, {}, []
for e in missions.entries:
    name = e.path.rsplit('/', 1)[-1].lower()
    if name.endswith('.rx2') and not name.endswith('_textures.rx2'):
        d = missions.read(e)
        try:
            for k, loc in global_locators(d).items():
                mission_locs.setdefault(k, dict(loc, source=name))
            rw4_volumes(d, volumes, name)
        except (struct.error, ValueError) as x:
            unread.append((name, str(x)))
print(len(mission_locs), 'mission locators;', len(unread), 'mission streams unread', unread[:5])
for asset, d in world_sims():
    for k, loc in global_locators(d).items():
        world_locs.setdefault(k, dict(loc, source='BAM.xsf %016x' % asset))
    rw4_volumes(d, volumes, 'BAM.xsf %016x' % asset)
for asset, d in world_sims(4):
    try:
        rw4_volumes(d, volumes, 'BAM.xsf %016x' % asset)
    except (struct.error, ValueError) as x:
        unread.append(('BAM.xsf %016x' % asset, str(x)))
bam_loc = {k.lower(): {'matrix': loc['matrix'], 'slots': [], 'source': 'BAM.loc'} for k, loc in C['world_locators']().items()}


def locator(name):
    """The name's locator from the source with the most spawn slots (mission stream, world stream, BAM.loc in a tie)."""
    found = [t[name.lower()] for t in (mission_locs, world_locs, bam_loc) if name.lower() in t]
    return max(found, key=lambda loc: len(loc['slots'])) if found else None


def locate(names):
    """The first name that resolves, preferring one with spawns for a lobby (2+ slots), then any slot (old_online's)."""
    found = [(n, locator(n)) for n in names if isinstance(n, str) and n]
    found = [(n, loc) for n, loc in found if loc]
    best = next(((n, l) for n, l in found if len(l['slots']) > 1),
                next(((n, l) for n, l in found if l['slots']), found[0] if found else (None, None)))
    return best[0], best[1] and {'matrix': best[1]['matrix'], 'slots': best[1]['slots']}


misc = Big(s1game/'data/big/miscgame.big')
polygons = {Path(e.path).stem.lower(): boundary(misc.read(e).decode('latin-1')) for e in misc.entries
            if e.path.lower().startswith('data/boundary/') and e.path.lower().endswith('.xml')}


def inside(polygon, x, z):
    """online_boundary.rs's even-odd test on world X/Z."""
    hit, j = False, len(polygon) - 1
    for i, (xi, zi) in enumerate(polygon):
        xj, zj = polygon[j]
        if (zi > z) != (zj > z) and x < (xj - xi) * (z - zi) / (zj - zi) + xi:
            hit = not hit
        j = i
    return hit


assert inside([[0, 0], [2, 0], [2, 2], [0, 2]], 1, 1) and not inside([[0, 0], [2, 0], [2, 2], [0, 2]], 3, 1)

# ---------------------------------------------------------------- rows
PLACE_FIELDS = ('Location', 'SkateWaitingLocation', 'SkateAttemptSpawnPoint', 'StartLocation', 'MapStartLocation',
                'EndLocation', 'BestTrickStartLocations', 'VisualIndicators', 'ScoringBoundary', 'SpotVolumes',
                'OTSTriggerBoundary', 'OTSSpotVolumes', 'OTSScoringBoundary', 'BestTrickRegions', 'BestTrickSpotVolumes',
                'CheckPoints', 'StartLine', 'FinishLine')
rows, templates, scenes_wanted = {}, {}, {}
for key in sorted(k for k in lessons if k not in parents):
    me = fields(key)
    kind = me.get('ChallengeType')
    if not me.get('AvailableOnline') or me.get('DebugOnly') or kind not in (*MODES, FREESKATE):
        continue  # (type 29: the X Games elevators/teleporters, not lobbies)
    tpl = template(key)
    templates[tpl] = kind
    row = {'id': key, 'type': kind, 'mode': MODES[kind][0] if kind in MODES else 'freeskate', 'template': tpl,
           'location_id': me.get('LocationID'), 'name': say(me.get('LocationID') or me.get('Title') or key)}
    # The online menu lists rows by their location name; the description is the mode's (Skate 1 online text). A row's
    # own Description (its career copy, often another row's) stays in `self`.
    row['description'] = english(MODES[kind][3]) if kind in MODES else ''
    order = (me.get('BestTrickStartLocations') or [])[:1]
    order += [me.get(k) for k in ('MapStartLocation', 'StartLocation', 'Location', 'SkateWaitingLocation', 'SkateAttemptSpawnPoint')]
    row['start_locator'], row['start'] = locate(order)
    row['boundary'] = polygons.get(key.lower())
    oob = me.get('OutOfBoundsTime')
    row['oob_time'] = oob if isinstance(oob, int) and oob > 0 else None
    places, missing = {}, []
    for f in PLACE_FIELDS:
        for name in (me.get(f) if isinstance(me.get(f), list) else [me.get(f)]):
            if isinstance(name, str) and name:
                loc, vol = locator(name), volumes.get(name.lower())
                if loc or vol:
                    places[name] = {'locator': loc} if loc else {'volume': vol}
                else:
                    missing.append(name)
    row['places'], row['missing_places'] = places, missing
    # (an unresolved word, e.g. best_trick_shared's 00000008 placeholders, is no scene)
    scene = lambda s: s if isinstance(s, str) and s and not re.fullmatch(r'[0-9A-F]{8}', s) else None
    if kind == 8:
        # Best Trick's setup_round plays NISFlyThroughScenes[round] before each round; its Lua never plays the row's
        # NISFlyThroughScene (Jam's setup_challenge, S.K.A.T.E.'s show_nis, OTS's intro_flyby and the races'
        # flythrough_nis play that one, once).
        row['round_intro_scenes'] = [scene(s) for s in me.get('NISFlyThroughScenes') or []]
        row['intro_scenes'] = [s for s in row['round_intro_scenes'] if s]
    else:
        row['intro_scenes'] = [s for s in [scene(me.get('NISFlyThroughScene'))] if s]
    row['self'] = me
    # Each VisualIndicators record's type (tVisualEditorData word 6, read by 0x823df6f0): 4 the obstacle indicator oi000;
    # 0 is never placed (audit 10-28 P7: ol_fp_u1_ownspot01's second record).
    vis = v.merged(key).get('VisualIndicators')
    items = (vis['array']['items'] if 'array' in vis else [vis['data']]) if vis else []
    row['indicator_types'] = [int(i[48:56], 16) for i in items]
    assert not isinstance(me.get('VisualIndicators'), list) or len(items) == len(me['VisualIndicators']), key
    for s in row['intro_scenes']:
        scenes_wanted.setdefault(s, []).append(key)
    rows[key] = row

by_mode = {}
for r in rows.values():
    by_mode.setdefault(r['mode'], []).append(r)
no_start = sorted(k for k, r in rows.items() if not r['start'])
assert not no_start, f'rows without a start: {no_start}'
assert all(r['boundary'] for r in rows.values()), 'every Skate 1 online row has data/boundary/<row>.xml'

# ---------------------------------------------------------------- intros (Skate 1 ICE through old_tutorial's evaluator)
scene_big = Big(s1game/'data/big/scene.big')
scene_bin = scene_big.read(next(e for e in scene_big.entries if e.path == 'data/scene/scene.bin'))
ice_bin = scene_big.read(next(e for e in scene_big.entries if e.path == 'data/scene/ice.bin'))
ICE1 = sorted(json.loads((TABLES/'ice_params1.json').read_text(encoding='utf-8')), key=lambda p: p['pos'])
s1ice.use(T, ICE1)
cameras = s1ice.ice_cameras(T, ice_bin)
s1ice.use_shakes(T, cameras, label_key)
n_scenes = struct.unpack_from('>I', scene_bin, 8)[0]
assert scene_bin[:4] == b'scen'
scene_records = {}
for i in range(n_scenes):
    o = 0x10 + i * 0x68
    f = struct.unpack_from('>7f', scene_bin, o + 8)
    scene_records[scene_bin[o + 0x28:o + 0x68].split(b'\0')[0].decode('latin-1')] = {
        'hash': struct.unpack_from('>I', scene_bin, o)[0], 'kind': scene_bin[o + 4], 'origin': list(f[0:3]),
        'duration': f[6], 'tracks': struct.unpack_from('>H', scene_bin, o + 0x24)[0]}
(OUT/'intros/scene/camera').mkdir(parents=True, exist_ok=True)
intros, no_camera, looks = {}, [], []
for name in sorted(scenes_wanted):
    s, takes = scene_records.get(name), None
    takes = s and cameras.get(s['hash'])
    if not takes:
        continue  # in `missing`: no scene record (Best Trick's placeholder "test") or no ICE record
    assert not s['tracks'], f'{name}: actor tracks (none of the online scenes has any)'
    # No online scene has an edit (CONTAINS_SUBTAKE keys; old_scenes1's tutorial scenes do).
    assert not any(any(t['values']['CONTAINS_SUBTAKE']) for t in takes), f'{name}: subtakes'
    try:
        camera = T['scene_camera'](takes)
    except AssertionError:
        no_camera.append(name)
        continue
    entry = {'kind': s['kind'], 'origin': s['origin'], 'duration': s['duration'], 'tracks': [], 'audio': [],
             'camera': f'scene/camera/{name}.json', 'rows': scenes_wanted[name]}
    # OVERLAY is Skate 1's HUD overlay enum (default.xex's name table 0x821375b0: 2-5 SpotFound_BottomLeft/BottomRight/
    # TopLeft/TopRight, 6.. SpotOwned_*); kept raw, out of the camera, as its ids and states differ from Skate 2's.
    if 'overlay' in camera:
        entry['s1_overlay'] = camera.pop('overlay')
    take = takes[0]
    audio = {n: [st for st in T['ice_steps'](take, n, 0., take['duration'], 0.) if st[1]]
             for n in ('AUDIO_POST_ID', 'AUDIO_SPEECH_ID', 'AUDIO_MUSIC_ID', 'AUDIO_EFFECT_ID')}
    if any(audio.values()):
        entry['s1_audio_ids'] = {n: st for n, st in audio.items() if st}  # the post is exported (below); the rest not
    entry['posts'] = [st[1] for st in audio['AUDIO_POST_ID'] if st[0] == 0.]  # ponytail: a later post needs a delay
    if len(takes) > 1:
        # ponytail: several takes and no ASSEMBLY (CONTAINS_SUBTAKE) keys: which one Skate 1 plays isn't traced; take 0.
        entry['takes'] = len(takes)
    (OUT/f'intros/scene/camera/{name}.json').write_text(json.dumps(camera, separators=(',', ':')), encoding='utf-8')
    intros[name] = entry
    for key in scenes_wanted[name]:
        # The frame check (axes as Skate 2's): how close the camera's eye passes the row's places (start, its grid,
        # locators, volume centres). A flythrough's look target can be far off (Spot Battle's open on a city view).
        r = rows[key]
        at = [m[3][:3] for m in [r['start']['matrix'], *r['start']['slots']]]
        at += [p['locator']['matrix'][3][:3] if 'locator' in p else
               [(a + b) / 2 for a, b in zip(p['volume']['min'], p['volume']['max'])] for p in r['places'].values()]
        looks.append((min(math.dist(x[1:4], a) for x in camera['samples'] for a in at), name, key))
T['post_audio'](intros, s1game/'data/audio/english/nis.big', OUT/'intros')  # Skate 1 has no post.big
(OUT/'intros/tutorial.json').write_text(json.dumps({  # the name Cutscene::start reads
    'kinds': dict(enumerate(KINDS)),
    'space': '1 world, 2 avatar, 3 scene origin (old_tutorial.scene_camera)',
    'scenes': intros, 'missing': sorted(set(scenes_wanted) - set(intros) - set(no_camera)), 'no_camera': no_camera},
    indent=1), encoding='utf-8')
looks.sort()
print(len(intros), 'intros; closest camera eye -> row place (m): median', round(looks[len(looks) // 2][0], 1),
      'max', [(round(d), n) for d, n, _ in looks[-3:]])

# ---------------------------------------------------------------- scripts (reference)
(OUT/'scripts').mkdir(parents=True, exist_ok=True)
lua_index, decompiled = {}, {}


def decompile(chunk):
    """old_challenges.decompile, once per distinct chunk (row templates repeat them), retried when Windows still holds
    its temp .luac at cleanup (WinError 32)."""
    if chunk not in decompiled:
        for attempt in range(5):
            try:
                decompiled[chunk] = C['decompile'](chunk)
                break
            except PermissionError:
                if attempt == 4:
                    raise
                time.sleep(2)
    return decompiled[chunk]


C['prefetch']([v.bin[offset:offset + size]  # every chunk the loop below asks for, unluac runs side by side
                 for tpl in sorted(templates) for step in v.rows(lessons[tpl]['fields'][STEPS])
                 for h in v.rows(step['fields'].get(HANDLERS, {'data': ''}) or {'data': ''})
                 for size, offset in [struct.unpack('>II', bytes.fromhex(h['fields'][CHUNK]['data']))]])
for tpl in sorted(templates):
    lua, steps = [f'-- {tpl}: Skate 1 challenge handlers, decompiled by unluac'], {}
    for step in v.rows(lessons[tpl]['fields'][STEPS]):
        sname = step['fields']['Name']['data']
        steps[sname] = {}
        for h in v.rows(step['fields'].get(HANDLERS, {'data': ''}) or {'data': ''}):
            size, offset = struct.unpack('>II', bytes.fromhex(h['fields'][CHUNK]['data']))
            fn, source = decompile(v.bin[offset:offset + size])
            steps[sname][h['fields']['Name']['data']] = fn
            lua.append(f'-- [{sname}] {h["fields"]["Name"]["data"]}\n{source.strip()}\n')
    (OUT/f'scripts/{tpl}.lua').write_text('\n'.join(lua), encoding='utf-8')
    lua_index[tpl] = {'type': templates[tpl], 'steps': steps}

(OUT/'rows.json').write_text(compact_json({
    'map': MAP, 'source': 'Skate 1 skater.vlt challenge rows (AvailableOnline, not DebugOnly); see ONLINE1_REPORT.md',
    'trick_types': TRICK_TYPES, 'modes': by_mode, 'scripts': lua_index}) + '\n', encoding='utf-8')
print({k: len(x) for k, x in by_mode.items()}, len(intros), 'intros;', len(lua_index), 'templates;',
      sum(len(r['missing_places']) for r in rows.values()), 'unresolved places')

# ---------------------------------------------------------------- online HUD (old_hud1's pipeline)
argv, sys.argv = sys.argv, ['old_hud1.py', str(repo), str(s1game), str(install_to or assets)]
try:
    import old_hud1 as hud1
finally:
    sys.argv = argv
hud1.WORK, hud1.OUT = WORK_ROOT/'online1-hud-cache', HUD_OUT
hud1.GFN.update(GFN)
cache = hud1.cache(tuple(HUD_MOVIES.values()) + hud1.EXTRA)
# The language: Skate 1's own labels these movies (and the mode Lua's leaving-area warning) ask for; no trick names.
ids = {'ID_COMMON_MISSION_LEFT_AREA_WARNING'}
for name in HUD_MOVIES.values():
    for suffix in ('.apt', '.const'):
        raw = hud1.WORK/'raw'/(name + suffix)
        if raw.is_file():
            ids |= {m.decode() for m in re.findall(rb'ID_[A-Z0-9_]{3,}', raw.read_bytes())}
s3 = {'language': {i: english(i) for i in sorted(ids) if english(i)}}
manifests = {stem: hud1.movie(cache, name, s3) for stem, name in HUD_MOVIES.items()}
hud1.install(manifests)
hud1.check(list(manifests))
print('hud fonts:', {s: sorted(m['fonts']) for s, m in manifests.items() if m['fonts']})
# spotfound's intro frames call Audio.PlaySpotFound(): message 236 -> skater.vlt FEMessageToSoundTrigger -> menu sound
# 2038 -> the PlayHudSample patch with index 2038 - 2000, which only fe.big's HUD.abk exports (item10-flashes.md D2).
# ponytail: index 38 -> HUD.abk sample 38 is inferred (the patch graph isn't decoded), and the game's per-index volume
# list and its scale aren't traced, so this is unity gain.
fe = Big(s1game/'data/audio/fe.big')
abk = fe.read(next(e for e in fe.entries if Path(e.path).name == 'HUD.abk'))
bank, bank_size = struct.unpack_from('>I', abk, 0x18)[0], struct.unpack_from('>I', abk, 0x24)[0]  # S10A sample bank
starts = struct.unpack_from(f'>{struct.unpack_from(">I", abk, bank + 8)[0]}I', abk, bank + 12)
end = starts[39] if starts[39] != 0xFFFFFFFF else bank_size  # 0xFFFFFFFF: unused slot (banks.rs SAMPLE_ABSENT)
sample = abk[bank + starts[38]:bank + end]
h1, h2 = struct.unpack_from('>II', sample)
assert h2 >> 29 == 0, f'HUD.abk sample 38: stream type/loop bits {h2 >> 29}, want a plain RAM sound (8-byte header)'
# ponytail: a RAM sound (stream type 0) is relabelled streamed (1) so vgmstream reads it as an .snr/.sns pair; the
# block layout is the same, and the frame count is checked against the header below.
(HUD_OUT/'sfx').mkdir(parents=True, exist_ok=True)
seconds = C['eaac_wav'](struct.pack('>II', h1, h2 | 1 << 30), sample[8:], HUD_OUT/'sfx/spot_found.wav')
assert round(seconds * (h1 & 0x3FFFF)) == h2 & 0x1FFFFFFF, (seconds, hex(h1), hex(h2))
print('sfx/spot_found.wav', round(seconds, 3), 's')


# ---------------------------------------------------------------- install
def backup(path):
    keep = path.with_name(path.name + '.pre-skate1online')
    if not keep.exists():
        shutil.copy2(path, keep)


def entry(r):
    spawn = {'start': r['start'], 'boundary': r['boundary'], 'oob_time': r['oob_time']}
    if r['type'] == FREESKATE:
        return {'name': r['name'], 'game': 'skate1', 'location': {'map': MAP, 'teleport': None, **spawn, 'id': r['id']}}
    return {'name': r['name'], 'game': 'skate1', 'challenge': {
        'id': r['id'], 'map': MAP, 'teleport': None, 'position': r['start']['matrix'][3][:3], **spawn,
        'description': r['description'], 'objectives': [], 'bonus': 0, 'icon': 'skate2_' + MODES[r['type']][2],
        'map_image': None, 'mode': MODES[r['type']][0]}}


def strip(rows_):
    return [dict(e, rows=strip(e['rows'])) if e.get('rows') else e for e in rows_ if e.get('game') != 'skate1']


def entries(online):
    return [r for t in online['tabs'] for e in t['rows'] for r in [e, *e.get('rows', [])]
            if 'challenge' in r or 'location' in r]


if install_to:
    # The map frame: every Skate 1 teleport spot (teleports.json, what the converted map spawns at) is a BAM.loc locator.
    teleports = json.loads((install_to/'private/teleports.json').read_text(encoding='utf-8'))['destinations']
    spots = [t for t in teleports if t['map'] == MAP]
    off = max(min(math.dist(t['matrix'][3][:3], loc['matrix'][3][:3]) for loc in bam_loc.values()) for t in spots)
    assert spots and off < 0.01, f'Skate 1 locators are not in the converted map frame (off by {off} m)'
    online_path = install_to/'private/challenge-map/online/online.json'
    online = json.loads(online_path.read_text(encoding='utf-8'))
    backup(online_path)
    for t in online['tabs']:
        t['rows'] = strip(t['rows'])
    taken = {(r.get('challenge') or r.get('location')).get('id') for r in entries(online)}
    assert not taken & set(rows), f'row ids already used: {sorted(taken & set(rows))}'
    tabs = {t['name']: t for t in online['tabs']}
    versus, freeskate = tabs['ID_MAP_FILTER_VERSUS'], tabs['ID_FREESKATEOPTION_START']
    for kind, (_, flt, icon, _) in MODES.items():
        assert 'skate2_' + icon in online['icons'], f'old_online registers skate2_{icon}; run it first'
        name = FILTER + flt
        folder = next((e for e in versus['rows'] if e.get('folder') and e['name'] == name), None)
        if folder is None:  # laid out as the retail folders: Options header + Quick Match, then the rows
            retail = next(e for e in versus['rows'] if e.get('folder') and e['name'].startswith(FILTER))
            folder = {'name': name, 'help': say(NEW_FOLDERS[flt][1]), 'folder': True, 'game': 'skate1',
                      'rows': retail['rows'][:2]}
            versus['rows'].append(folder)
        folder['rows'] += [entry(r) for r in sorted((r for r in rows.values() if r['type'] == kind),
                                                    key=lambda r: r['name'].lower())]
    # Skate 1's freeskate folder: on the Freeskate tab before game_tabs has run, the "#Skate 1" tab's after.
    s1 = next((e for e in freeskate['rows'] if e.get('folder') and (inner := [r for r in e['rows'] if 'location' in r])
               and all(r['location']['map'] == MAP for r in inner)), None) or \
        next(e for e in tabs['#Skate 1']['rows'] if e.get('folder') and e['name'] == game_tabs.FREESKATE)
    locations = sorted((r for r in rows.values() if r['type'] == FREESKATE), key=lambda r: r['name'].lower())
    s1['rows'] = ([{'name': '#Skate 1 Online', 'header': True, 'game': 'skate1'}] + [entry(r) for r in locations] +
                  [{'name': '#Teleport Spots', 'header': True, 'game': 'skate1'}] + s1['rows'])
    online = game_tabs.apply(online)
    online_path.write_text(compact_json(online) + '\n', encoding='utf-8')
    # The Spot Race folder's label wherever the type names are shown (map menu, online HUDs); Best Trick and Jam are
    # old_online's already (same text in Skate 1).
    for path in [install_to/'private/challenge-map/runtime/challengemap.json',
                 *sorted((install_to/'private/online-ui/runtime').glob('*.json'))]:
        data = json.loads(path.read_text(encoding='utf-8'))
        language = data.get('language', {})
        words = {FILTER + flt: english(label) for flt, (label, _) in NEW_FOLDERS.items() if FILTER + flt not in language}
        if FILTER + 'SKATE' in language and words:
            backup(path)
            language.update(words)
            path.write_text(json.dumps(data, separators=(',', ':'), ensure_ascii=False) + '\n', encoding='utf-8')
            print('language +', sorted(words), path.relative_to(install_to))
    dest = install_to/'private/skate1-online'
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copy2(OUT/'rows.json', dest/'rows.json')
    shutil.copytree(OUT/'intros', dest/'intros', dirs_exist_ok=True)
    shutil.copytree(HUD_OUT, install_to/'private/hud-skate1/online', dirs_exist_ok=True)
    # Self-check: what the engine will read back.
    check = json.loads(online_path.read_text(encoding='utf-8'))
    added = [r for r in entries(check) if r.get('game') == 'skate1']
    assert len(added) == len(rows), (len(added), len(rows))
    tab = next(t for t in check['tabs'] if t['name'] == '#Skate 1')
    print('#Skate 1 tab:', [(f['name'], len(game_tabs.rows_of(f['rows']))) for f in tab['rows']])
    target = lambda r: r.get('challenge') or r.get('location')
    lone = [r['name'] for r in added if len(target(r)['start']['slots']) < 2]
    # Where players appear: the slots (a locator's own matrix is the challenge marker; a race's 8 children are its
    # starting grid, up to 630 m away), else the locator.
    spawns = lambda s: [m[3] for m in s['slots']] or [s['matrix'][3]]
    # teleport_menu::valid_matrix, which online_map::prune applies to every start and slot (else the start is dropped)
    valid = lambda m: (all(math.isfinite(x) for row in m for x in row) and all(abs(m[i][3]) < 1e-5 for i in range(3))
                       and abs(m[3][3] - 1) < 1e-5 and m[2][0] ** 2 + m[2][2] ** 2 > 1e-6)
    starts = [target(r)['start'] for r in added]
    assert all(valid(m) for s in starts for m in [s['matrix'], *s['slots']]), 'a start prune would drop'
    outside = [r['name'] for r in added
               if not all(inside(target(r)['boundary'], p[0], p[2]) for p in spawns(target(r)['start']))]
    print('installed', len(added), 'rows into', online_path, '; < 2 spawn slots (engine stacks on the locator):', lone,
          '; a spawn outside its boundary:', outside)
