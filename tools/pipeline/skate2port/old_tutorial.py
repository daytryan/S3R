"""Skate 2's tutorial (New_San_Vanelona): the data crates/skate-game/src/skate2_challenge.rs runs.

Writes assets/private/skate2-tutorial/:
  tutorial.json         frontend order, the progression slice (triggers -> states -> handler functions), the
                        progression messages they send, per challenge the merged lesson fields (`self`), steps ->
                        events -> handler functions, objectives with every field decoded, placement (locators,
                        oriented volumes in Skate 2 world space = NSV map space), scenes (actor tracks, audio,
                        subtitles), trick groups, speech line -> bank member, grind ids -> rails, the photo
                        subjects' AI paths, the riders' recordings (ai_segments), trick rows (ai_tricks) and AI
                        profiles (ai_profiles)
  scripts/<challenge>.lua  every handler decompiled by unluac: `function h_<KEY>(self, message, vars)`
  scripts/progression.lua  every progression handler (plain Lua source in the vault), wrapped as
                        `function p_<KEY>(self, message, vars)`
  strings.json          every ID_ label the above references (English)
  scene/camera/<scene>.json  the scene's ICE camera (scene.big data/scene/ice_000.bin) and camera shake
                        (ICELite.bin) evaluated as default.xex does, sampled at CAMERA_HZ:
                        {space, samples: [t, eye xyz, look xyz, roll, fov, time scale, fisheye, tonemap, bloom threshold, bloom intensity, sun xyz], overlay?: [[t, id]]}
  movies/<name>.vp6 + <name>.wav   movies as-is, English audio (vgmstream subsong 1, 48 kHz 6ch) downmixed to stereo
  reference/RECOVERY.md tutorial_recovery.md (evidence) + the generated walkthrough of every step

Speech and cutscene audio are XMA EAAC (the Skate 3 format); the engine decodes them from the disc, so this only
records bank + member. See tutorial_recovery.md for every inferred meaning.

usage (numpy; java on PATH): py old_tutorial.py REPO SKATE2_ROOT ASSETS
"""
import functools, json, math, os, re, shutil, struct, subprocess, sys, tempfile, wave
from pathlib import Path
import numpy as np

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
from s2lang import table
from tools.asset_pipeline.vlt import convert, hash64
from skate3_streams import read_atoc, read_sfil
from tools.asset_pipeline.dynamic_props import locators as dmo_locators

OUT = assets/'private/skate2-tutorial'
UNLUAC = Path(os.environ.get('SKATE_UNLUAC') or here/'tools/unluac.jar')
VGMSTREAM = Path(os.environ.get('SKATE_VGMSTREAM') or here/'tools/vgmstream/vgmstream-cli.exe')

# Lessons in scope. m_1a_07/08 have progression entries but no trigger activates them in retail (RECOVERY.md).
CHALLENGES = ['m_1a_01_goto_slappys', 'm_1a_03_tutorial_fliptricks', 'm_1a_04_tutorial_manuals',
              'm_1a_07_tutorial_airtricks', 'm_1a_08_tutorial_grinds', 'm_2a_01_tutorial_slappy',
              'm_1a_05_photo_rail', 'm_1a_06_photo_puzzle', 'm_1a_07_follow_slappy']
TRIGGERS = ['Start State Machine', 'Trigger_m_1a_01_goto_slappys', 'Trigger_m_1a_03_tutorial_fliptricks',
            'Trigger_m_1a_04_tutorail_manual', 'Trigger_m_2a_01_tutorial_slappy', 'Trigger_m_2a_01_tutorial_Slappy',
            'Trigger_m_1a_05_photo_rail', 'Trigger_m_1a_06_photo_puzzle', 'Trigger_m_1a_07_follow',
            'Trigger_m_1a_07_follow_fail', 'Trigger_m_1a_07_follow_accept']
# Hard-coded in the frontend (the exe names data\movies\RedaCarRide_english_ntsc.vp6); not in any vault.
FRONTEND = [{'movie': 'intro_movie'}, {'movie': 'RedaCarRide'}, {'customiser': True}, {'trigger': 'Start State Machine'}]
# The lesson's movie row (TutorialMovie) where the vault has one; the other two by their subtitle group
# (Tutorial_Skate03 = ID_NISSUBTUTS_GRABS*, Tutorial_Skate02 = ID_NISSUBTUTS_GRINDS*).
MOVIE_FALLBACK = {'m_1a_07_tutorial_airtricks': 'Tutorial_Skate03', 'm_1a_08_tutorial_grinds': 'Tutorial_Skate02'}
# Scene -> nis.big stream(s); matched by name and length against nisresident.big headers (RECOVERY.md).
# The second stream is the take's AUDIO_POST_ID (track 33): `99_<id>_*` in post.big, the scene's SFX bed (camera
# whooshes and zooms). 01/02/03 have only a post stream, the full mix, in nis.big.
SCENE_AUDIO = {
    'tutorial_m_1a_goto_slappys': ['40_5011_SlapTut.sns', '99_5011_SlapTutSFX_GH4.sns'],
    'replay_tutorial_m_1a_goto_slappys': ['40_5018_TutReply.sns', '99_5018_TutReplaySFX.sns'],
    'Goto_m_1a_01_goto_slappys_outro_1': ['40_5008_SlapOut1.sns', '99_5008_SlapOut1SFX_GH2.sns'],
    'Goto_m_1a_01_goto_slappys_outro_2': ['40_5009_SlapOut2.sns', '99_5009_SlapOut2SFX_GH1.sns'],
    'Goto_m_1a_01_goto_slappys_outro_3': ['40_5010_SlapOut3.sns', '99_5010_SlapOut3SFX_GH2.sns'],
    'Goto_m_1a_01_goto_slappys_outro_4': ['40_5012_SlapOut4_ED.sns', '99_5012_SlapOut4SFX_GH3.sns'],
    '01_Welcome_Back': ['99_5001_WlcmBk_GH4.sns'],
    '02_Pros_Arrival': ['99_5002_ProArrv_GH5.sns'],
    '03_Pros_Leave': ['99_5003_ProLv_GH4.sns'],
    # Photo scenes: the take's AUDIO_SPEECH_ID is the stream number; Atiba (38) is the photographer.
    'tutorial_m_1a_05_photo_rail': ['38_5000_1a_05.sns', '99_5013_PhtoRail_GH2.sns'],
    'photo_in_m_1a_06_photo_puzzle': ['38_5002_1a_06_int.sns', '99_5014_PhtoPzlSFX_GH1.sns'],
    # Follow Slappy's win: ICE AUDIO_SPEECH_CHARACTER 40 / AUDIO_SPEECH_ID 5016, AUDIO_POST_ID 5016 (round2-research R6).
    'goto_m_1b_01_goto_school': ['40_5016_SchlFlythr.sns', '99_5016_SchlFlySFX_GH1.sns'],
}
# Speaker -> (number in bank members <line>_<number>_<label>.dat and in Audio* attributes [number, line << 16], bank).
SPEAKERS = {'CameraMan': (40, 'english/cameramanspeech.big'), 'Slappy': (29, 'english/maincastspeech.big'),
            'Atiba': (38, 'english/maincastspeech.big')}
SPEAKER_ALIAS = {'Abita': 'Atiba'}  # m_1a_05's hint speech event spells him so; normalised to the speech key

# Vault field names (hash -> name). Named by the handlers that read them or by their values; RECOVERY.md has the evidence.
ALIAS = {
    'Hash_3CB7BA2AFC9BDE42': 'Steps', 'Hash_810394ECEFE7E48B': 'Handlers', 'Hash_C3CD4B4183274819': 'Chunk',
    'Hash_48427451AEF34906': 'TutorialMovie', 'Hash_59CD0B9449D19C3C': 'TutorialMovieLabel',
    'Hash_CE53161A0E754F51': 'ObjectiveFlow', 'Hash_915C7BF8B06D5A58': 'DMOResetInfo',
    'Hash_555C505A36C3353A': 'GlobalSpawnCharacterInfo', 'Hash_876BF0819AA58858': 'RequiredChallengeHull',
    'Hash_E13D9C326E4D98D5': 'Definition', 'Hash_40D40861DD9A6CC6': 'FailDefinition',
    'Hash_E73C9CF0F75A6FCD': 'IntroEvents', 'Hash_D404BA3319E2C389': 'CompleteEvents',
    'Hash_607EC72F932997C1': 'FailEvents', 'Hash_B69AF085F7A6D275': 'Hints', 'Hash_BD96B775B855E129': 'Events',
    'Hash_D4965ED3BD43EC44': 'Triggers', 'Hash_D7332063BE5CC0EB': 'TriggerActivationMode',
    # A second hint list: same row class, rows numbered in the one <objective>_<n> series; m_1a_06 has only this.
    'Hash_ECAEB8E619A68AF3': 'Hints',
    'Hash_A595E54979341D05': 'PhotoOutScene', 'Hash_A5FC99F67C810807': 'PhotoKilledScene',
}
# The classes one challenge key spans; `self` is their merged effective fields, earlier class wins.
SELF_CLASSES = ['challenges', 'Hash_46A56ADDF284C381', 'Hash_A4F35B159D7432B5']
OBJECTIVE_CLASS, GROUP_CLASS = 'Hash_59A34A6C5074F88A', 'Hash_5B47469D2D895DDE'
TRIGGER_CLASS, MESSAGE_CLASS, ENTRY_CLASS = 'Hash_2500715BAA0F4D9C', 'Hash_B9E254F192F0721A', 'Hash_62A8C2523DA41D76'
MESSAGE_TYPES = {0x2: 'nis', 0x9: 'player_spawn', 0xA: 'hud_component', 0xF: 'fmv', 0x12: 'dialog'}
EMPTY = 0x1ED  # ChallengeCore.bin offset of ""
# Bytes per integer type, as the schemas declare them (field size n); every enum (e*) is declared 4. vlt.convert exports
# an inline field as its whole 4-byte slot with the value left-aligned (all 3133 sub-4-byte slots pad with zeros).
INT_BYTES = {'Int8': 1, 'UInt8': 1, 'Int16': 2, 'UInt16': 2, 'Int32': 4, 'UInt32': 4, 'Int64': 8, 'UInt64': 8}


def key_hash(name):
    return int(name[5:], 16) if name.startswith('Hash_') else hash64(name)


class Vault:
    """A converted vault plus its .bin, with per-class field inheritance (child row's field wins)."""
    def __init__(self, data, binary):
        self.bin = binary
        self.rows = {(r['class'], key_hash(r['key'])): r for r in data['collections']}
        self.classes = {key_hash(r['class']): r['class'] for r in data['collections']}
        self.by_key = {}  # refs whose class hash names no class (progression properties) resolve by key alone
        for r in data['collections']:
            self.by_key.setdefault(key_hash(r['key']), r)

    def row(self, cls, key):
        return self.rows.get((cls, key if isinstance(key, int) else key_hash(key)))

    def fields(self, row):
        merged = {}
        while row:
            for k, v in row['fields'].items():
                merged.setdefault(k, v)
            row = self.row(row['class'], row['parent']) if row['parent'] else None
        return merged

    def refs(self, field):
        items = field['array']['items'] if 'array' in field else [field['data']]
        out = []
        for i in items:
            if int(i[:16], 16):
                key = int(i[16:32], 16)
                out.append(self.row(self.classes.get(int(i[:16], 16), ''), key) or self.by_key.get(key) or
                           {'unresolved': i[:32]})
        return out

    def string(self, ptr):
        if ptr == EMPTY:
            return ''
        if 0 < ptr < len(self.bin) and self.bin[ptr - 1] == 0:
            raw = self.bin[ptr:self.bin.index(b'\0', ptr)]
            if raw and all(b >= 32 or b in (9, 10, 13) for b in raw):
                return raw.decode('latin-1')
        return None

    def word(self, w):
        s = self.string(w)
        return w if s is None else s

    def value(self, v):
        t = v['type'].rsplit('::', 1)[-1]
        if 'array' in v:
            return [self.scalar(t, i) for i in v['array']['items']]
        return self.scalar(t, v['data'])

    def scalar(self, t, data):
        if t == 'Text':
            return data
        if t == 'RefSpec':
            return [r.get('key', r.get('unresolved')) for r in self.refs({'data': data})]
        raw = bytes.fromhex(data)
        if t == 'Bool':
            return raw != b'\0' * len(raw)
        if t == 'Float':
            return struct.unpack('>f', raw)[0]
        if t.startswith(('Int', 'UInt', 'e')):  # ints and enums
            return int.from_bytes(raw[:INT_BYTES.get(t, 4)], 'big', signed=t.startswith('Int'))
        w = struct.unpack('>%dI' % (len(raw) // 4), raw[:len(raw) // 4 * 4])
        if t == 'tChallengePresentationEvent':
            return {'type': w[0], 'a': self.word(w[1]), 'b': self.word(w[2])}
        if t == 'tObjectiveDefinition':
            return {'source': self.word(w[0]), 'text': self.word(w[3]), 'words': [self.word(x) for x in w]}
        if t == 'tLocationID':
            return self.word(w[0])
        if t == 'tVisualEditorData':
            return {'locator': self.word(w[6]), 'words': list(w)}
        if t == 'tTriggerVolumeInstanceID':
            return {'volume': self.word(w[0]), 'id': '0x%08x%08x' % (w[2], w[3])}
        return [self.word(x) for x in w]

    def named(self, fields, skip=()):
        return {ALIAS.get(k, k): self.value(v) for k, v in fields.items() if ALIAS.get(k, k) not in skip}


# round2 T5 tricks: what the AI's PathController reads per trick and per rider, from the skater vault.
SCORABLE, PROFILE = 0x6918469984A8C596, 0xE9407D230E506420
S2_TRICKS = 269  # rows in Skate 2's trick table 0x820614c8: Skate 3's IDENTIFIERS[:269], name for name


def items(fields, key):
    return fields.get(key, {}).get('array', {}).get('items', [])


def ai_tricks(v, names):
    """Per Skate 2 trick index (`names`: the table's spellings) its scorable row, None without one (the AI then pops
    nothing): `lead`, frames ahead of its node it starts (Hash_2702723D1A7F0ABF), and `recipe`, the AI signals
    0x823fdcc0 writes, [signal, value, from frame, to frame or -1] (Hash_F216FED54DCBEB50: 6 slots of {UInt32, Float,
    Int32, Int32}, signal 0 unused)."""
    cls = v.classes.get(SCORABLE)
    out = []
    for name in names:
        row = v.row(cls, name) if cls else None
        if row is None:
            out.append(None)
            continue
        f = v.fields(row)
        slots = (struct.unpack('>Ifii', bytes.fromhex(i)[:16]) for i in items(f, 'Hash_F216FED54DCBEB50')[:6])
        out.append({'lead': v.value(f['Hash_2702723D1A7F0ABF']) if 'Hash_2702723D1A7F0ABF' in f else 0,
                    'recipe': [list(s) for s in slots if s[0]]})
    return out


def ai_profiles(v, keys):
    """The AI profiles (class 0xE9407D230E506420) named `keys`: `grabs` (+400, Hash_6DBAF7AD6CDE6A16: categories 3, 4
    and 12 perform their recipe) and the flips mode 1 draws from by weight, [trick, weight] (Hash_E580B6284639E03F,
    for an N... trick Hash_BB901D68361E9833; items {UInt32, Float})."""
    cls = v.classes.get(PROFILE)
    out = {}
    for key in keys:
        row = v.row(cls, key) if cls else None
        if row is None:
            continue
        f = v.fields(row)
        flips = {k: [list(struct.unpack('>If', bytes.fromhex(i)[:8])) for i in items(f, h)]
                 for k, h in (('regular', 'Hash_E580B6284639E03F'), ('nollie', 'Hash_BB901D68361E9833'))}
        out[key] = {'grabs': 'Hash_6DBAF7AD6CDE6A16' in f and v.value(f['Hash_6DBAF7AD6CDE6A16']), **flips}
    return out


def db_vaults():
    db = Big(s2game/'data/db/db.big')
    names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
    names += (TABLES/'s2_names.txt').read_text(encoding='utf-8').splitlines()
    with tempfile.TemporaryDirectory(prefix='skate2-tut-') as work:
        work = Path(work)
        for e in db.entries:
            if Path(e.path).stem in ('skater', 'ChallengeCore', 'progressioncore'):
                (work/Path(e.path).name).write_bytes(db.read(e))
        return {stem: Vault(convert(work/'skater', work/stem, names), (work/f'{stem}.bin').read_bytes())
                for stem in ('ChallengeCore', 'progressioncore', 'skater')}


def _unluac(chunk):
    from tools.unluac_batch import run, text  # Java gets paths relative to TEMP: a non-ASCII folder name works (#19)
    stdout, code = run([chunk], unluac=UNLUAC)[chunk]
    stdout = text(stdout)
    function = re.search(r'^function (h_\w+)\(self, message, vars\)', stdout, re.M)
    assert code == 0 and function, f'unluac failed: {stdout[:300]}'
    return function.group(1), stdout


def decompile(chunk):
    return _luac[chunk] if chunk in _luac else _unluac(chunk)


_luac = {}  # chunk -> _unluac(chunk), filled by prefetch


def prefetch(chunks):
    """Decompile the chunks a loop is about to ask decompile() for, in one unluac batch (tools/unluac_batch.py: one JVM
    start-up, not one per chunk), so the loop then gets the same results from memory, in its own order. A chunk that
    fails here is left to the loop's own call (which fails, or retries, as it always did)."""
    from tools.unluac_batch import run, text
    for chunk, (stdout, code) in run([c for c in dict.fromkeys(chunks) if c not in _luac], unluac=UNLUAC).items():
        function = re.search(r'^function (h_\w+)\(self, message, vars\)', text(stdout), re.M)
        if code == 0 and function:
            _luac[chunk] = function.group(1), text(stdout)


def side_by_side(fn, items, gb_each=0.1):
    """[fn(x) for x in items], workers() threads at once: for jobs that mostly wait on their own subprocess (unluac,
    vgmstream) and write only their own files, so the results and files are a loop's."""
    from concurrent.futures import ThreadPoolExecutor
    from tools.pipeline.fanout import workers
    with ThreadPoolExecutor(workers(gb_each)) as pool:
        return list(pool.map(fn, items))


# ---------------------------------------------------------------- challenges
def challenge(v, name):
    rows = [v.row(c, name) for c in SELF_CLASSES]
    assert rows[0], f'no challenge row {name}'
    merged = {}
    for row in filter(None, rows):
        for k, val in v.fields(row).items():
            merged.setdefault(k, val)
    self_ = v.named(merged, skip=('Steps', 'Objectives'))
    self_['Name'] = name
    steps, lua, handlers = {}, [f'-- {name}: Skate 2 ChallengeCore handlers, decompiled by unluac'], []
    for step in v.refs(merged['Hash_3CB7BA2AFC9BDE42']):
        step = v.fields(step)
        sname = step['Name']['data']
        steps[sname] = own = {}
        # m_1a_07_follow_slappy has handler-less steps.
        for handler in v.refs(step['Hash_810394ECEFE7E48B']) if 'Hash_810394ECEFE7E48B' in step else ():
            handler = v.fields(handler)
            size, offset = struct.unpack('>II', bytes.fromhex(handler['Hash_C3CD4B4183274819']['data']))
            handlers.append((sname, own, handler['Name']['data'], v.bin[offset:offset + size]))
    prefetch([chunk for *_, chunk in handlers])
    for sname, own, hname, chunk in handlers:
        function, source = decompile(chunk)
        own[hname] = function
        lua.append(f'-- [{sname}] {hname}\n{source.strip()}\n')
    objectives = []
    for row in v.refs(merged['Objectives']):
        assert 'unresolved' not in row, f'{name}: objective {row}'
        fields = v.named(v.fields(row), skip=('Hints',))
        fields['Key'] = row['key']
        own = v.fields(row)
        fields['Hints'] = [dict(v.named(v.fields(h)), Key=h['key']) for k in ('Hash_B69AF085F7A6D275', 'Hash_ECAEB8E619A68AF3')
                           if k in own for h in v.refs(own[k])]
        objectives.append(fields)
    movie = merged.get('Hash_48427451AEF34906')
    if movie:  # a skater-vault movie row (class 18B27535); FileName is the movie
        row = vaults['skater'].row('Hash_18B275357481FB52', int(movie['data'][16:32], 16))
        self_['TutorialMovie'] = row['fields']['FileName']['data'] if row else None
    self_['TutorialMovie'] = self_.get('TutorialMovie') or MOVIE_FALLBACK.get(name)
    if 'RaceHeats' in self_:
        self_['RaceHeats'] = race_heats(v, self_['RaceHeats'])
    return {'self': self_, 'steps': steps, 'objectives': objectives}, '\n'.join(lua)


def race_heats(v, heats):
    """RaceHeats -> Legs -> RandomGates as the race template reads them: rows of named fields; trigger volumes as
    their names (what the port's triggerentercollision VolumeID carries); empty NIS definitions nil."""
    rows = lambda refs: [v.named(v.fields(v.row(v.classes[(w[0] << 32) | w[1]], (w[2] << 32) | w[3]))) for w in refs]
    volume = lambda t: t['volume'].split('|')[1]
    out = rows(heats)
    for heat in out:
        heat['Legs'] = rows(heat['Legs'])
        for leg in heat['Legs']:
            leg['SplitTimeTriggers'] = [volume(t) for t in leg['SplitTimeTriggers']]
            leg['RandomGates'] = [dict(g, GateVolume=volume(g['GateVolume']), RibbonInstance=None) for g in rows(leg['RandomGates'])]
        for k, d in heat.items():
            if k.startswith('NIS') and d[:1] == ['']:
                heat[k] = None
        heat['AIPaths'] = [p for p in heat['AIPaths'] if p]
        # [min, max, warning speaker, line << 16, followee, recovery speaker, line << 16, delay, repeat]; the last
        # word reads as a stray string.
        heat['FollowTheLeaderWarningInfo'] = [[w if isinstance(w, int) else 0 for w in i] for i in heat['FollowTheLeaderWarningInfo']]
    return out


# ---------------------------------------------------------------- progression
def minimap_icon(skater, ref):
    """A challenge's minimap icon: its E616ACDF refspec ([class hi, lo, key hi, lo, ..]) names a skater-vault row
    (class F788504A8D922CED) whose FCFE787C is the hudminimap marker (0x82472dc4 -> 0x8246bd10). Types 0/38/23 never
    swap to an owned/killed variant (round2-research2 Q4)."""
    row = skater.row(skater.classes[ref[0] << 32 | ref[1]], ref[2] << 32 | ref[3])
    return skater.value(row['fields']['Hash_FCFE787CCF2F775E'])


def entry_bindings(v, names):
    """Per entry name in `names`: its [trigger, mode] binding rows (class F5C8EFA0) in vault order. CompleteChallenge
    (0x824cd1b8) starts the bindings whose mode (Hash_20C0) the recorded outcome allows: win {0, 3, 5}, lose
    {1, 3, 4, 5}, abandon {2, 4, 5} (round2-research2 Q5). Rows join on serials: trigger Hash_30FE, entry Hash_2AEB."""
    def name_of(row):
        return row['fields'].get('Name', {}).get('data', row['key'])
    def serial(row, h):
        return v.value(row['fields'][h]) if h in row['fields'] else None
    triggers = {serial(r, 'Hash_30FEF85A4E39B759'): name_of(r) for (c, _), r in v.rows.items() if c == TRIGGER_CLASS}
    entries = {}
    for (c, _), r in v.rows.items():
        if c == ENTRY_CLASS:
            entries.setdefault(serial(r, 'Hash_2AEBDDA55A479FDE'), []).append(name_of(r))
    out = {n: [] for n in names}
    for (c, _), r in v.rows.items():
        if c == 'Hash_F5C8EFA024EE3925':
            for e in entries.get(serial(r, 'Hash_2AEBDDA55A479FDE'), []):
                if e in out:
                    out[e].append([triggers[serial(r, 'Hash_30FEF85A4E39B759')], serial(r, 'Hash_20C0A525C304BB07')])
    return out


def progression(v):
    def name_of(row):
        return row['fields'].get('Name', {}).get('data', row['key'])
    by_name = {}
    for (cls, _), row in v.rows.items():
        by_name.setdefault((cls, name_of(row)), row)
    triggers, lua = {}, ['-- Skate 2 progressioncore handlers (plain Lua source in the vault)']
    for t in TRIGGERS:
        row = by_name[(TRIGGER_CLASS, t)]
        states = {}
        for state in v.refs(row['fields']['Hash_8EFBBC13E8126ED3']):
            handlers = {}
            for h in v.refs(state['fields']['Hash_DEB2096FE58B1805']):
                ptr = struct.unpack('>I', bytes.fromhex(h['fields']['Hash_6391ACEAB5A03CD2']['data'])[:4])[0]
                source = v.string(ptr)
                function = 'p_%016X' % key_hash(h['key'])
                handlers[name_of(h)] = function
                lua.append(f'-- {t} [{name_of(state)}] {name_of(h)}\nfunction {function}(self, message, vars)\n{source}\nend\n')
            states[name_of(state)] = handlers
        initial = v.refs(row['fields']['InitialState'])[0]
        triggers[t] = {'initial': name_of(initial), 'states': states}
    source = '\n'.join(lua)
    sent = set(re.findall(r'Progression\.SendMessage\(\s*"([^"]+)"', source))
    messages = {}
    for m in sorted(sent):
        row = by_name.get((MESSAGE_CLASS, m))
        assert row, f'progression message {m} not found'
        kind = struct.unpack('>i', bytes.fromhex(row['fields']['Hash_1CC1319BADBA0454']['data']))[0]
        refs = v.refs(row['fields']['Properties']) if 'Properties' in row['fields'] else []
        props = [p['fields'].get('Hash_54A390B7ED8DCF1D', {}).get('data', '') if 'fields' in p else None for p in refs]
        # The property names (class 55970EF6 rows, paired with the properties by index Hash_B0D770EA): kind
        # Hash_3C32F163 1 is an int, 2 text. SendMessage's handlers read them by name (0x824cc6a8).
        names = {n['fields']['Hash_B0D770EAAD4A229F']['data']: n for n in v.refs(row['fields']['Hash_69BDC77C3309E1D1']) if 'fields' in n} \
            if 'Hash_69BDC77C3309E1D1' in row['fields'] else {}
        named = {}
        for p in refs:
            f = p.get('fields', {})
            n = names.get(f.get('Hash_B0D770EAAD4A229F', {}).get('data'))
            if n is not None:
                named[name_of(n)] = struct.unpack('>i', bytes.fromhex(f['Hash_0E9CE6A6267BA806']['data']))[0] \
                    if n['fields']['Hash_3C32F163E20008CD']['data'] == '00000001' else f.get('Hash_54A390B7ED8DCF1D', {}).get('data', '')
        ident = struct.unpack('>i', bytes.fromhex(row['fields']['Hash_6931057318D2132E']['data']))[0]
        settings = [bytes.fromhex(s['fields']['Hash_95300B5DDA3A0A01']['data']) != b'\0'
                    for s in v.refs(row['fields']['Settings'])] if 'Settings' in row['fields'] else []
        messages[m] = {'type': MESSAGE_TYPES.get(kind, kind), 'properties': props, 'named': named, 'id': ident,
                       'settings': settings}
    entries = {}
    for (cls, _), row in v.rows.items():
        if cls == ENTRY_CLASS and name_of(row) in CHALLENGES:
            entries[name_of(row)] = {'title': row['fields'].get('Title', {}).get('data'),
                                     'triggers': [name_of(t) for t in v.refs(row['fields']['Hash_D102E7411533FAA8'])]
                                     if 'Hash_D102E7411533FAA8' in row['fields'] else []}
    for name, bound in entry_bindings(v, entries).items():
        entries[name]['bindings'] = bound
    activated = set(re.findall(r'Progression\.(?:ActivateChallenge(?:ByTitle)?)\(\s*"([^"]+)"', source))
    return {'triggers': triggers, 'messages': messages, 'entries': entries,
            'activated': sorted(activated & set(CHALLENGES))}, source


# ---------------------------------------------------------------- placement
def rw4_objects(d):
    count, = struct.unpack_from('>I', d, 0x20)
    table_at, = struct.unpack_from('>I', d, 0x30)
    for i in range(count):
        at, _, size, _, _, kind = struct.unpack_from('>6I', d, table_at + 24 * i)
        yield kind, at, size


def cstr(d, at):
    return d[at:d.index(b'\0', at)].decode('latin-1')


def matrix(d, at):
    return [list(struct.unpack_from('>4f', d, at + 16 * k)) for k in range(4)]


@functools.cache
def _missions():
    """missions.big and its entries with their paths in lower case (the online scripts place hundreds of missions)."""
    missions = Big(s2game/'data/content/missions.big')
    return missions, [(e.path.lower(), e) for e in missions.entries]


def write_streams(work, files, read, *names):
    """Write the folder's `files` ({name: entry}) that the stream readers open, `names` in any case, into work: what
    writing every file there gives those reads (of names differing only in case, the last written is the one read)."""
    wanted = {n.lower() for n in names}
    for n, e in files.items():
        if n.lower() in wanted:
            (work/n).write_bytes(read(e))


def mission_placement(name):
    """Locator (0xEB0009), trigger volume (0xEB0019) and DMO (0xEB001D, read by the repo's dynamic_props.locators)
    tables of missions/<name>/cSim_Global.xsf."""
    missions, entries = _missions()
    files = {Path(e.path).name: e for path, e in entries if f'/missions/{name}/'.lower() in path}
    if not files:
        return {}, {}, []
    locators, volumes, dmos = {}, {}, []
    with tempfile.TemporaryDirectory(prefix='skate2-mission-') as work:
        work = Path(work)
        write_streams(work, files, missions.read, f'{name}_Sim.xst', 'cSim_Global.xsf')
        records = read_atoc(work/f'{name}_Sim.xst')
        for asset in read_sfil(work/'cSim_Global.xsf', records):
            d = asset.data
            if d[:4] != b'\x89RW4':
                continue
            for kind, at, size in rw4_objects(d):
                if kind == 0xEB0009:   # hdr (count, 1, 0, first); 0x80-byte records; name ptr @+0x68
                    count, _, _, first = struct.unpack_from('>4I', d, at)
                    for r in range(count):
                        o = at + first + 0x80 * r
                        locators[cstr(d, at + struct.unpack_from('>I', d, o + 0x68)[0])] = matrix(d, o)
                elif kind == 0xEB0019:  # hdr (magic, count, count, first); 0xF0-byte records; name ptr @+0xE0
                    _, count, _, first = struct.unpack_from('>4I', d, at)
                    for r in range(count):
                        o = at + first + 0xF0 * r
                        full = cstr(d, at + struct.unpack_from('>I', d, o + 0xE0)[0])
                        volumes[full.split('_0x', 1)[0]] = {  # name_0x<id path>::<mission>
                            'id': '0x%016x' % struct.unpack_from('>Q', d, o + 0xB8)[0],
                            'matrix': matrix(d, o),
                            'min': list(struct.unpack_from('>3f', d, o + 0x40)),
                            'max': list(struct.unpack_from('>3f', d, o + 0x50)),
                            'corners': [list(struct.unpack_from('>3f', d, o + 0x70 + 16 * k)) for k in range(4)]}
            for p in dmo_locators(d):
                m = p['matrix']
                # Models: worldmisc.big data/content/world/dmo/DMO/cPres_Global.xsf, keyed by template_id;
                # dynamic_props draws model_matrix @ template matrix @ `matrix`. yaw: Bevy Quat::from_rotation_y.
                dmos.append({'name': p['name'].split('_0x', 1)[0], 'template': re.sub(r'(_DMO)?_\d+$', '', p['name'].split('_0x', 1)[0]),
                             'template_id': p['template_id'], 'instance_id': p['instance_id'], 'matrix': m, 'at': m[3][:3],
                             'yaw': math.degrees(math.atan2(-m[0][2], m[0][0])), 'bounds': p['bounds']})
    return locators, volumes, dmos


def world_locators():
    world = Big(s2game/'data/content/worldbam.big')
    text = world.read(next(e for e in world.entries if e.path.endswith('BAM_Sim.loc'))).decode('latin-1')
    out = {}
    for m in re.finditer(r'<Name>([^<]*)</Name>.*?<Transform>([^<]*)</Transform>', text, re.S):
        v = [float(x) for x in m.group(2).split(',')]
        out[m.group(1)] = [v[0:4], v[4:8], v[8:12], v[12:16]]
    return out


# ---------------------------------------------------------------- scenes, subtitles
def scenes(wanted):
    cameras = ice_cameras()
    boot = Big(s2game/'data/big/miscboot.big')
    d = boot.read(next(e for e in boot.entries if e.path == 'data/scene/scene_000.bin'))
    magic, _, n_scenes, _ = struct.unpack_from('>4sIII', d, 0)
    assert magic == b'scen'
    tracks_at = 0x10 + n_scenes * 0x68
    out = {}
    for i in range(n_scenes):
        o = 0x10 + i * 0x68
        name = d[o + 0x28:o + 0x68].split(b'\0')[0].decode('latin-1')
        if name not in wanted:
            continue
        f = struct.unpack_from('>7f', d, o + 8)
        count, first = struct.unpack_from('>HH', d, o + 0x24)
        tracks = []
        for t in range(first, first + count):
            r = tracks_at + 180 * t
            text = lambda a, b: d[r + a:r + b].split(b'\0')[0].decode('latin-1')
            tracks.append({'character': text(0, 32), 'package': text(32, 48), 'anim': text(48, 112), 'clip': text(112, 176)})
        out[name] = {'kind': d[o + 4], 'origin': list(f[0:3]), 'duration': f[6], 'tracks': tracks,
                     'audio': SCENE_AUDIO.get(name, [])}
        takes = cameras.get(struct.unpack_from('>I', d, o)[0])
        if takes:
            (OUT/'scene/camera').mkdir(parents=True, exist_ok=True)
            camera = scene_camera(takes)
            (OUT/f'scene/camera/{name}.json').write_text(json.dumps(camera, separators=(',', ':')), encoding='utf-8')
            out[name]['camera'] = f'scene/camera/{name}.json'
            out[name]['posts'] = post_ids(takes)  # post_audio turns them into audio; the tutorial's own drops them
            if 'freeze' in camera:
                # ponytail: NIS.GetPhotoNISCount read as the freeze key count (8 in photo_out/killed); not traced in the exe.
                out[name]['photos'] = len(camera['freeze'])
    return out


# ---------------------------------------------------------------- ICE cameras
# The parameter table (name, track, per-key flag, packing, range, cubic/tension links) is default.xex's ICE parameter
# registration, dumped to ice_params.json. Decoding and evaluation follow the exe (tutorial_recovery.md, "Cameras").
ICE_PARAMS = sorted(json.loads((TABLES/'ice_params.json').read_text(encoding='utf-8')), key=lambda p: p['pos'])
ICE = {p['name']: p for p in ICE_PARAMS}
ICE_POS = {p['pos']: p for p in ICE_PARAMS}
CAMERA_HZ = 60
CAMERA_TRACKS = (0, 1, 5)  # eye/look/dutch/lens, aux lens + override, time scale: their key times are the cuts


@functools.cache  # scenes() is called once per scene by the online scripts; nothing changes the takes
def ice_cameras():
    """scene name hash -> takes. ice_000.bin: LE (type, size) chunks of records (1, size, hash, take count) + takes."""
    big = Big(s2game/'data/big/scene.big')
    e = big.read(next(x for x in big.entries if x.path == 'data/scene/ice_000.bin'))
    out = {}
    o, end = 8, 8 + struct.unpack_from('<I', e, 4)[0]
    while o < end:
        chunk_end = o + 8 + struct.unpack_from('<I', e, o + 4)[0]
        r = o + 8
        while r + 8 <= chunk_end:
            _, size, h, n = struct.unpack_from('<4I', e, r)
            takes, at = [], r + 0x10
            for _ in range(n):
                takes.append(ice_take(e[at:r + 8 + size]))
                at = (at + takes[-1]['size'] + 3) & ~3
            out[h] = takes
            r += 8 + size
        o = chunk_end
    return out


def ice_take(t):
    """37 tracks of (key count, value count); u16 value indices of the inner keys, u16 key times (65535 = the take's
    end), then each parameter's values in table order (one per key when flagged, else per value), packed MSB first:
    f32, raw unsigned, or quantized about the default (ice_dequant)."""
    tracks = [struct.unpack_from('<HH', t, 0x14 + 4 * i) for i in range(37)]
    n_index, n_time = sum(max(k - 2, 0) for k, _ in tracks), sum(max(k - 1, 0) for k, _ in tracks)
    index = struct.unpack_from(f'<{n_index}H', t, 0xa8)
    times = struct.unpack_from(f'<{n_time}H', t, 0xa8 + 2 * n_index)
    keys, i, j = [], 0, 0
    for k, v in tracks:
        # Key n uses values index[n] and index[n] + 1 (two past the previous key's = a cut).
        keys.append(([0] + list(index[i:i + max(k - 2, 0)]) + [v - 2] if k > 1 else [0] * k,
                     [0.] + [x / 65535 for x in times[j:j + max(k - 1, 0)]] + [1.]))
        i, j = i + max(k - 2, 0), j + max(k - 1, 0)
    o = (0xa8 + 2 * (n_index + n_time) + 3) & ~3
    values = {}
    for p in ICE_PARAMS:
        k, v = tracks[p['track']]
        n = k if p['flag'] else v
        size = ((n * p['bits'] + 31) >> 3) & ~3
        block = t[o:o + size]
        assert p['type'] in (1, 2, 3, 4), p
        if p['type'] == 4:
            values[p['name']] = list(struct.unpack_from(f'<{n}f', block))
        else:
            bits, mask = int.from_bytes(block, 'big'), (1 << p['bits']) - 1
            raw = [(bits >> (8 * size - (x + 1) * p['bits'])) & mask for x in range(n)]
            values[p['name']] = [ice_dequant(p, x) for x in raw] if p['type'] == 3 else raw
        o += size
    return {'duration': struct.unpack_from('<f', t, 0xc)[0], 'keys': keys, 'values': values, 'size': o}


def ice_dequant(p, raw):
    """0x828456d8 with the zero point of 0x82867cf0: raw Z is exactly the default, linear to min below and max above."""
    lo, de, hi = p['min'], p['default'], p['max']
    q = (1 << p['bits']) - 1
    n = float(int((hi - de) * q / (hi - lo) + 0.5))
    z = q - n
    d = raw - z
    if d < 0 and z > 0:
        return de + (de - lo) * d / z
    if d > 0 and n > 0:
        return de + (hi - de) * d / n
    return de


def ice_value(take, name, u, left=False, keys=None):
    """A parameter at fraction u of the take. Per-key parameters hold through their key; the rest are Hermite
    between the key's two values (0x828408a8): a tangent is the key's delta, or with the key's cubic flag set, blended
    with a continuous neighbour's delta by duration and scaled by the tension parameter. `left`: the limit from
    before u (the last frame before a cut). `keys`: one frame's {track: key}, shared by its parameters (the same u and
    left find the same key on a track); the arithmetic is the closures' it replaced, operation for operation."""
    p = ICE[name]
    track = p['track']
    index, times = take['keys'][track]
    if not index:
        return p['default']
    k = None if keys is None else keys.get(track)
    if k is None:
        k = 0
        for n in range(1, len(index)):
            if times[n] < u or times[n] == u and not left:
                k = n
        if keys is not None:
            keys[track] = k
    vals = take['values'][name]
    if p['flag']:
        return vals[k]
    p0, p1 = vals[index[k]], vals[index[k] + 1]
    span = times[k + 1] - times[k]
    d = p1 - p0
    m0 = m1 = d
    cubic = ICE_POS.get(p['cubic'])
    if cubic is not None and take['values'][cubic['name']][k]:
        tension = ICE_POS.get(p['tension'])
        m = []
        for side in (0, 1):
            t = p1 - p0
            n, e = k - 1 + 2 * side, index[k] + side
            if 0 <= n < len(index) and index[n] + 1 - side == e and span + (times[n + 1] - times[n]) > 0:
                dk, dn = span, times[n + 1] - times[n]
                f8, f9 = dn / (dk + dn), dk / (dk + dn)
                if dn > 1e-6:
                    f9 *= dk / dn
                t = (vals[index[n] + 1] - vals[index[n]]) * f9 + f8 * t
            m.append(t * (take['values'][tension['name']][e] if tension else 1.))
        m0, m1 = m
    s = min(max((u - times[k]) / span, 0.), 1.) if span > 0 else 0.
    a, b = m0 + m1 - 2 * d, 3 * d - 2 * m0 - m1
    return ((a * s + b) * s + m0) * s + p0


def ice_frame(take, u, left=False, shake=True):
    """[eye xyz, look xyz, roll, fov, time scale, fisheye, tonemap, bloom threshold, bloom intensity,
    sun xyz] in engine axes. ICE is Z up: (x, y, z) -> engine (y, z, x).
    0x82843160 builds the view's rows (forward, left, up; Z up) and rolls the up row toward the camera's right by
    DUTCH turns; then the camera shake (ice_shake) moves and turns the view in its own frame. roll is the turn about
    Bevy's local Z from looking_at(look, Y) to that up (-DUTCH turns without shake). The lens override (per key,
    track 1) swaps in the aux lens; fov is 2 atan(15.96 / lens), as the exe's.
    fisheye scales the screen fisheye's edge warp. With FISHEYE_OVERRIDE, 0x8259ffd0 sets the lens multiplier to
    FISHEYE_MULTIPLIER x 4.7619 (1 / 0.21; the default is 1) and scales the lens's two strength settings (0.3, 0.26)
    by it, leaving its centre scale (0.9) alone. FISHEYE_HORIZONTAL/VERTICAL are 1, the defaults, in every take.
    tonemap is TONEMAP_VALUE under TONEMAP_OVERRIDE (0x8259ec28 -> render settings +0x88): while > 0 the exposure
    meter is set to it (0x8232c53c). bloom is BLOOM_THRESHOLD, BLOOM_INTENSITY under BLOOM_OVERRIDE (0x8259f460 ->
    +0x94, +0x98). sun is SUN_DIRECTION under SUN_DIRECTION_OVERRIDE (ice_sun), in the sky data's own axes as the
    engine's map sun is. All are 0 without their override."""
    keys = {}
    v = lambda n: ice_value(take, n, u, left, keys)
    lens = v('LENS_LENGTH_AUX') if v('LENS_OVERRIDE') else v('LENS_LENGTH')
    fov = 2 * math.atan(15.96 / lens)
    eye, look = np.array([v('EYE_X'), v('EYE_Y'), v('EYE_Z')]), np.array([v('LOOK_X'), v('LOOK_Y'), v('LOOK_Z')])
    unit = lambda x: x / np.linalg.norm(x)
    f = unit(look - eye)
    left_ = unit(cross([0., 0., 1.], f))
    up = cross(f, left_)
    a = v('DUTCH') * 2 * math.pi
    rows = np.array([f, math.cos(a) * left_ + math.sin(a) * up, -math.sin(a) * left_ + math.cos(a) * up])
    distance = np.linalg.norm(look - eye)
    if shake and (moved := ice_shake(take, u, left, fov)):
        turn, move = moved
        eye, rows = eye + move @ rows, turn @ rows
    engine = lambda x: np.array([x[1], x[2], x[0]])
    eye, f, up = engine(eye), unit(engine(rows[0])), engine(rows[2])
    right0 = unit(cross([0., 1., 0.], -f))
    roll = math.atan2(-(up @ right0), up @ cross(-f, right0))
    fisheye = v('FISHEYE_MULTIPLIER') / 0.21 if v('FISHEYE_OVERRIDE') else 1.
    tonemap = v('TONEMAP_VALUE') if v('TONEMAP_OVERRIDE') else 0.
    bloom = (v('BLOOM_THRESHOLD'), v('BLOOM_INTENSITY')) if v('BLOOM_OVERRIDE') else (0., 0.)
    sun = ice_sun(v('SUN_DIRECTION_X'), v('SUN_DIRECTION_Y'), v('SUN_DIRECTION_Z')) if v('SUN_DIRECTION_OVERRIDE') else (0., 0., 0.)
    return [float(x) for x in (*eye, *(eye + f * distance), roll, fov, v('TIME_SCALE') / 100, fisheye, tonemap, *bloom, *sun)]


def cross(a, b):
    """np.cross of two 3-vectors without its axis handling (most of ice_frame's time): the same float64 products and
    differences, so the same bits."""
    a0, a1, a2 = a
    b0, b1, b2 = b
    return np.array([a1 * b2 - a2 * b1, a2 * b0 - a0 * b2, a0 * b1 - a1 * b0], dtype=np.float64)


def ice_sun(a, b, c):
    """0x825a3784: degrees -> radians; the rows (X, Y, Z) turn by sincos (0x8237f858: sin, cos) of a in the (Z, Y)
    pair, of b in the (X, Y) pair, of c in the (Z, Y) pair again; the light direction (+0xa0 of the light state) is
    (1, 0, 0) through them, the final Z row."""
    a, b, c = map(math.radians, (a, b, c))
    x, y, z = np.eye(3)
    z1, y1 = math.sin(a) * z + math.cos(a) * y, math.cos(a) * z - math.sin(a) * y
    y2 = math.cos(b) * x - math.sin(b) * y1
    return math.sin(c) * z1 + math.cos(c) * y2


@functools.cache
def ice_shakes():
    """SHAKE_TYPE n -> (frame count, per frame SHAKE_QUAT_X/Y/Z, SHAKE_POS_X/Y/Z): take n of scene.big
    data/scene/ICELite.bin in file order (take 0 has none). Its takes have 34 tracks; the last holds those six
    parameters, 16-bit about 0 in [-1, 1], one value per frame."""
    big = Big(s2game/'data/big/scene.big')
    e = big.read(next(x for x in big.entries if x.path == 'data/scene/ICELite.bin'))
    out = []
    o, end = 8, 8 + struct.unpack_from('<I', e, 4)[0]
    while o < end:
        chunk_end = o + 8 + struct.unpack_from('<I', e, o + 4)[0]
        r = o + 8
        while r + 8 <= chunk_end:
            _, size, _, n = struct.unpack_from('<4I', e, r)
            t = e[r + 0x10:r + 8 + size]
            tracks = [struct.unpack_from('<HH', t, 0x14 + 4 * i) for i in range(34)]
            k, v = tracks[33]
            assert n == 1 and (k == 0 or not any(x != (0, 0) for x in tracks[:33])), tracks
            at = (0x9c + 2 * (max(k - 2, 0) + max(k - 1, 0)) + 3) & ~3
            block = ((v * 16 + 31) >> 3) & ~3
            assert k == 0 or at + 6 * block == size - 8
            cols = []
            for j in range(6):
                bits = int.from_bytes(t[at + j * block:at + (j + 1) * block], 'big')
                cols.append([ice_dequant(ICE['SHAKE_QUAT_X'], bits >> (8 * block - (x + 1) * 16) & 0xffff) for x in range(v)])
            out.append((k, list(zip(*cols))))
            r += 8 + size
        o = chunk_end
    return out


def ice_shake(take, u, left, fov):
    """Camera shake (0x828420f8), as (turn, move) of the view's rows, or None. SHAKE_TYPE's ICELite take plays at
    30 frames a second x SHAKE_FREQUENCY, wrapping; it is scaled by SHAKE_AMPLITUDE x sin(fov / 2) / sin(30 deg).
    The exe's own quirks are kept: w = sqrt(1 - |xyz|), and the move reads frame a for both ends of its lerp."""
    v = lambda n: ice_value(take, n, u, left)
    amplitude = v('SHAKE_AMPLITUDE')
    if not amplitude:
        return None
    kind = v('SHAKE_TYPE')
    assert kind, 'shake type 0 (procedural, 0x82842580) is not ported'
    # ponytail: frame count = the shake track's key count (the take object's u16 at 0x498, read as keys).
    frames, values = ice_shakes()[kind]
    s = amplitude * math.sin(fov / 2) / math.sin(math.pi / 6)
    at = u * take['duration'] * v('SHAKE_FREQUENCY') * 30
    a, b, f = values[int(at) % frames], values[(int(at) + 1) % frames], at % 1

    def quat(x):
        q = [c * s for c in x[:3]]
        return q + [math.sqrt(1 - math.sqrt(sum(c * c for c in q)))]
    x, y, z, w = (p + (q - p) * f for p, q in zip(quat(a), quat(b)))
    # The exe's matrix, fitted exactly: the rotation of quaternion (z, x, -y, w), transposed.
    qa, qb, qc = z, x, -y
    turn = np.array([[1 - 2 * (qb * qb + qc * qc), 2 * (qa * qb + qc * w), 2 * (qa * qc - qb * w)],
                     [2 * (qa * qb - qc * w), 1 - 2 * (qa * qa + qc * qc), 2 * (qb * qc + qa * w)],
                     [2 * (qa * qc + qb * w), 2 * (qb * qc - qa * w), 1 - 2 * (qa * qa + qb * qb)]])
    return turn, np.array([-a[5], -a[3], a[4]]) * s


def ice_rows(take, a, b, shift):
    """Samples of take seconds [a, b] at scene time t + shift; a cut is two samples at one time (before, after)."""
    # In fractions of the take, so a cut lands exactly on its key time.
    dur = take['duration']
    cuts = {x for tr in CAMERA_TRACKS for x in take['keys'][tr][1][1:-1] if a < x * dur < b}
    grid = {min(a + i / CAMERA_HZ, b) / dur for i in range(int((b - a) * CAMERA_HZ) + 2)}
    rows, end = [], b / dur
    # `left` only matters at a parameter's own inner key time; anywhere else the limit from before is the same frame.
    keyed = {x for _, times in take['keys'] for x in times[1:-1]}
    for u in sorted(grid | cuts):
        after = [round(x, 5) for x in ice_frame(take, u, False)]
        before = [round(x, 5) for x in ice_frame(take, u, True)] if u in keyed else after
        if u == end or u in cuts and before != after:
            rows.append([u * dur + shift] + before)
        if u != end:
            rows.append([u * dur + shift] + after)
    return rows


def edit_shots(edit, takes):
    """An edit's shots (take number, subtake start s, edit t0, t1): each take-switch key (CONTAINS_SUBTAKE's
    track: Skate 2's 35, Skate 1's 32) with CONTAINS_SUBTAKE plays TAKE_NUMBER from TAKE_START, a fraction of that take
    (nis-clock.md 2, RETAIL); the scene stops at the tail key after the last shot."""
    index, times = edit['keys'][ICE['CONTAINS_SUBTAKE']['track']]
    v = edit['values']
    return [(v['TAKE_NUMBER'][j], v['TAKE_START'][j] * takes[v['TAKE_NUMBER'][j]]['duration'],
             times[j] * edit['duration'], times[j + 1] * edit['duration'])
            for j in range(len(index)) if v['CONTAINS_SUBTAKE'][j]]


def scene_camera(takes):
    """One take plays as is. Several: the one with take-switch keys (CONTAINS_SUBTAKE) is the edit (edit_shots)."""
    edit = next((t for t in takes if any(t['values']['CONTAINS_SUBTAKE'])), None)
    overlay, freeze, shots = [], [], []
    if edit is None:
        used, rows = takes[:1], ice_rows(takes[0], 0., takes[0]['duration'], 0.)
        overlay = ice_steps(takes[0], 'OVERLAY', 0., takes[0]['duration'], 0.)
        # FREEZE_CMD/FREEZE_TIME (track 34): a key per photo snap (photo_out/photo_killed); single-take scenes only.
        freeze = [[t, c, ice_value(takes[0], 'FREEZE_TIME', t / takes[0]['duration'])]
                  for t, c in ice_steps(takes[0], 'FREEZE_CMD', 0., takes[0]['duration'], 0.) if c]
    else:
        used, rows = [], []
        shots = edit_shots(edit, takes)
        for n, start, t0, t1 in shots:
            take = takes[n]
            used.append(take)
            rows += ice_rows(take, start, start + t1 - t0, t0 - start)
            overlay += ice_steps(take, 'OVERLAY', start, start + t1 - t0, t0 - start)
    # Space (0x8284f228): 1 world, 2 the avatar's position (world axes), 3 the scene (its origin, ground snapped).
    # 0 "Player" (name table Player/World/Hybrid/Scene): the avatar's position and rotation (Skate 1 0x82c38130: its
    # vfunc +4 rows and +8 position via 0x82c1be28); only Skate 1's photo-win cameras look in it.
    eye, look = ({x for t in used for x in t['values'][k]} for k in ('SPACE_EYE', 'SPACE_LOOK'))
    assert len(eye) == len(look) == 1 and eye | look <= {0, 1, 2, 3}, (eye, look)
    out = {'space': eye.pop(), 'samples': [[round(x, 5) for x in row] for row in rows]}
    # The look's own space where it differs (m_1a_06's photo cameras: eye in the world, look on the avatar).
    if look != {out['space']}:
        out['look_space'] = look.pop()
    # OVERLAY: the HUD overlay enum (0 Clear, 80/81 TutorialGesture_OlliePt1/Pt2 = screen nis/gestollie).
    steps = [s for i, s in enumerate(overlay) if i == 0 or s[1] != overlay[i - 1][1]]
    if any(v for _, v in steps):
        out['overlay'] = [[round(t, 5), v] for t, v in steps]
    if freeze:
        out['freeze'] = [[round(t, 5), c, f] for t, c, f in freeze]  # [t, FREEZE_CMD, FREEZE_TIME]
    if shots:
        # [edit t0, subtake start]: each cut sets the actors to its shot's subtake time (nis-clock.md 5).
        out['shots'] = [[round(t0, 5), round(start, 5)] for _, start, t0, _ in shots]
    return out


def ice_steps(take, name, a, b, shift):
    """A per-key (step) parameter over take seconds [a, b] as [scene time, value] at each change."""
    dur, times = take['duration'], take['keys'][ICE[name]['track']][1]
    return [[a + shift, ice_value(take, name, a / dur)]] + \
        [[u * dur + shift, ice_value(take, name, u)] for u in times[1:-1] if a < u * dur < b]


def subtitles(names, labels, values):
    """English .sub cues: (key u32 LE, END time u16 in 1/10 s); each line runs from the previous record's time."""
    boot = Big(s2game/'data/big/miscboot.big')
    sub = Big_from_bytes(boot.read(next(e for e in boot.entries if e.path.endswith('subtitles.big'))))
    files = {Path(e.path).stem: e for e in sub.entries if '/SubTitles_English/' in e.path}
    out = {}
    for name in names:
        e = files.get(f'XBOX2_{name}') or files.get(name)
        if not e:
            continue
        d = sub.read(e)
        cues, start = [], 0.0
        for i in range((len(d) - 4) // 8):
            key, end, _ = struct.unpack_from('<IHH', d, 4 + 8 * i)
            if key != 0x1A20BA and end != 0xFFFF:
                assert key in labels, f'{name}: subtitle key {key:#x} has no label'
                cues.append({'start': start, 'end': end / 10, 'label': labels[key]})
            start = end / 10
        out[name] = cues
    return out


def Big_from_bytes(data):
    tmp = Path(tempfile.mkdtemp(prefix='skate2-big-'))/'x.big'
    tmp.write_bytes(data)
    return Big(tmp)


# ---------------------------------------------------------------- movies, speech
def movie(name):
    src = s2game/f'data/movies/{name}_english_ntsc.vp6'
    shutil.copyfile(src, OUT/f'movies/{name}.vp6')
    meta = subprocess.run([str(VGMSTREAM), '-m', '-s', '1', str(src)], capture_output=True, text=True).stdout
    samples = int(re.search(r'play duration: (\d+) samples', meta).group(1))
    with tempfile.TemporaryDirectory(prefix='skate2-movie-') as work:
        wav_path = Path(work)/'a.wav'
        subprocess.run([str(VGMSTREAM), '-s', '1', '-o', str(wav_path), str(src)], check=True, capture_output=True)
        with wave.open(str(wav_path)) as w:
            channels, rate = w.getnchannels(), w.getframerate()
            pcm = np.frombuffer(w.readframes(w.getnframes()), '<i2').reshape(-1, channels).astype(np.float32)
    assert len(pcm) == samples, f'{name}: decoded {len(pcm)} frames, header says {samples}'
    if channels == 6:  # ponytail: assumes L R C LFE Ls Rs; swap if the preview's centre sounds wrong
        left = pcm[:, 0] + 0.707 * (pcm[:, 2] + pcm[:, 4])
        right = pcm[:, 1] + 0.707 * (pcm[:, 2] + pcm[:, 5])
        stereo = np.stack([left, right], 1)
        stereo *= min(1.0, 32767 / max(1.0, np.abs(stereo).max()))
    else:
        stereo = pcm[:, :2]
    with wave.open(str(OUT/f'movies/{name}.wav'), 'wb') as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(rate)
        w.writeframes(stereo.astype('<i2').tobytes())
    return {'video': f'movies/{name}.vp6', 'audio': f'movies/{name}.wav', 'seconds': samples / rate}


def eaac_wav(header, body, out):
    """An EAAC (XMA) sound as 16-bit WAV: vgmstream reads the header and the block chain as an .snr/.sns pair."""
    with tempfile.TemporaryDirectory(prefix='skate2-eaac-') as work:
        snr = Path(work)/'a.snr'
        snr.write_bytes(header)
        snr.with_suffix('.sns').write_bytes(body)
        subprocess.run([str(VGMSTREAM), '-o', str(out), str(snr)], check=True, capture_output=True)
    with wave.open(str(out)) as w:
        channels, rate, n = w.getnchannels(), w.getframerate(), w.getnframes()
        pcm = np.frombuffer(w.readframes(n), '<i2').reshape(-1, channels).astype(np.float32)
    if channels in (3, 6):
        if channels == 3:  # NIS scenes: L, dialogue centre, R (the centre is alone in speech-only stretches)
            stereo = np.stack([pcm[:, 0] + 0.707 * pcm[:, 1], pcm[:, 2] + 0.707 * pcm[:, 1]], 1)
        else:  # ponytail: SlapTutSFX; the movies' L R C LFE Ls Rs is assumed, unverified for EAAC
            stereo = np.stack([pcm[:, 0] + 0.707 * (pcm[:, 2] + pcm[:, 4]), pcm[:, 1] + 0.707 * (pcm[:, 2] + pcm[:, 5])], 1)
        stereo *= min(1.0, 32767 / max(1.0, np.abs(stereo).max()))
        with wave.open(str(out), 'wb') as w:
            w.setnchannels(2); w.setsampwidth(2); w.setframerate(rate)
            w.writeframes(stereo.astype('<i2').tobytes())
    return n / rate


def post_ids(takes):
    """A scene's AUDIO_POST streams: the takes' AUDIO_POST_ID at each AUDIO_POST_CMD 2 key at t=0, the `99_<id>_*`
    SFX bed SCENE_AUDIO names by hand (bug 10-09 #10). ponytail: every online take starts its post at 0 (Skate 2 and
    Skate 1 probed); a later one is left out, as it would need a delay. tools/skate3_online.py scene_posts is Skate 3's.
    An id several takes key (Skate 1's lesson edits: take copies and the first shot's take) plays once."""
    out = []
    track = ICE['AUDIO_POST_CMD']['track']
    for t in takes:
        cmd, ids = t['values'].get('AUDIO_POST_CMD', []), t['values'].get('AUDIO_POST_ID', [])
        times = t['keys'][track][1] if track < len(t['keys']) else []
        out += [ids[j] for j, c in enumerate(cmd) if c == 2 and j < len(ids) and (times[j] if j < len(times) else 0.) == 0.]
    return list(dict.fromkeys(out))


def fold_stereo(pcm):
    """RwAudioCore's 5.1 -> stereo fold-down (prepare_boot.FOLD; channels L C R [Ls Rs [LFE]], measured on post.big as
    on the movies): LFE dropped, centre and surrounds -3 dB into the fronts, the mix a further -3 dB. Two channels or
    fewer stay as they are. (eaac_wav's own downmix stays the tutorial's.)"""
    f = 0.70710678
    if pcm.shape[1] < 3:
        return pcm
    left, right = pcm[:, 0] + f * pcm[:, 1], pcm[:, 2] + f * pcm[:, 1]
    if pcm.shape[1] >= 5:
        left, right = left + f * pcm[:, 3], right + f * pcm[:, 4]
    return np.stack([left, right], 1) * f


def post_wavs(bank_path, ids, out):
    """Each id's `99_<id>_*.sns` in a bank (Skate 2 data/audio/post.big, Skate 1 data/audio/english/nis.big; its .snr
    header in the bank's *resident.big) decoded by vgmstream and folded to stereo: out/nis/<stream>.wav.
    Returns {id: {'audio', 'seconds'}}; an id without a stream is reported and left out."""
    bank = Big(bank_path)
    resident = Big_from_bytes(bank.read(next(e for e in bank.entries if e.path.endswith('resident.big'))))
    heads = {Path(e.path).name: e for e in resident.entries}
    streams = {int(Path(e.path).name.split('_')[1]): e for e in bank.entries
               if Path(e.path).name.startswith('99_') and e.path.endswith('.sns')}
    got = {}
    for i in sorted(ids):
        e = streams.get(i)
        if e is None:
            print(f'AUDIO_POST_ID {i}: no 99_{i}_*.sns in {Path(bank_path).name} - not exported')
            continue
        stem = Path(e.path).stem
        with tempfile.TemporaryDirectory(prefix='skate2-post-') as work:
            snr, raw = Path(work)/'a.snr', Path(work)/'a.wav'
            snr.write_bytes(resident.read(heads[stem + '.snr']))
            snr.with_suffix('.sns').write_bytes(bank.read(e))
            subprocess.run([str(VGMSTREAM), '-o', str(raw), str(snr)], check=True, capture_output=True)
            with wave.open(str(raw)) as w:
                channels, rate, n = w.getnchannels(), w.getframerate(), w.getnframes()
                pcm = fold_stereo(np.frombuffer(w.readframes(n), '<i2').reshape(-1, channels).astype(np.float32))
        (Path(out)/'nis').mkdir(parents=True, exist_ok=True)
        with wave.open(str(Path(out)/f'nis/{stem}.wav'), 'wb') as w:
            w.setnchannels(pcm.shape[1]); w.setsampwidth(2); w.setframerate(rate)
            w.writeframes(np.clip(np.round(pcm), -32768, 32767).astype('<i2').tobytes())
        got[i] = {'audio': f'nis/{stem}.wav', 'seconds': round(n / rate, 3)}
    return got


def post_audio(scenes, bank_path, out):
    """Each scene's `posts` (from scenes()) as its audio: the streams post_wavs writes under out (the scenes' root)."""
    wavs = post_wavs(bank_path, {i for s in scenes.values() for i in s.get('posts', [])}, out)
    for s in scenes.values():
        s['audio'] = [wavs[i] for i in s.pop('posts', []) if i in wavs]


def speech_lines(challenges, progression_lua, messages):
    """Line N of a speaker = bank member N_<number>_*.dat; its first take (sub-sound 0 of the matching .sth) ->
    speech/. Lines come from speech events, Audio* attributes (line 0 = none) and progression reminders."""
    lines = {}
    by_number = {n: s for s, (n, _) in SPEAKERS.items()}
    for data in challenges.values():
        for e in all_events(data):
            if e['type'] in (2, 3, 13) and e['a'] in SPEAKERS:
                lines.setdefault(e['a'], set()).add(str(e['b']))
        for k, a in data['self'].items():
            if k.startswith('Audio') and isinstance(a, list) and len(a) == 2 and a[1] >> 16:
                lines.setdefault(by_number[a[0]], set()).add(str(a[1] >> 16))
    # ponytail: a reminder names no speaker; 4004 is only in the CameraMan bank (4004_40_Rmd_Atiba).
    lines.setdefault('CameraMan', set()).update(re.findall(r'PlayProgressionReminder\((\d+)\)', progression_lua))
    # Type 4 (SponsorMeStart) plays its FileID through the native PlayProgressionReminder uses (0x824d32c8 -> speech
    # vtable[15]); CharacterID 40 is CameraMan, the only speaker a tutorial type 4 names.
    fours = [m['named'] for m in messages.values() if m['type'] == 4]
    assert all(n['CharacterID'] == 40 for n in fours), fours
    lines['CameraMan'].update(str(n['FileID']) for n in fours)
    out, jobs = {}, []
    for speaker, ids in lines.items():
        number, bank_path = SPEAKERS[speaker]
        bank = Big(s2game/'data/audio'/bank_path)
        members = {Path(e.path).name: e for e in bank.entries}
        sth = Big_from_bytes(bank.read(next(e for n, e in members.items() if n.endswith('sth.big'))))
        headers = {Path(e.path).stem: e for e in sth.entries}
        out[speaker] = {'bank': bank_path, 'lines': {}}
        for i in sorted(ids, key=int):
            found = sorted(n for n in members if n.endswith('.dat') and n.split('_')[:2] == [i, str(number)])
            if not found:
                out[speaker]['lines'][i] = None
                continue
            dat, rec = bank.read(members[found[0]]), sth.read(headers[Path(found[0]).stem])
            starts = [struct.unpack_from('>I', rec, k)[0] for k in range(0, len(rec), 12)] + [len(dat)]
            # ponytail: a line holds several takes; the first is always played. Retail's pick wasn't recovered.
            path = f'speech/{speaker}_{i}.wav'
            out[speaker]['lines'][i] = line = {'member': found[0], 'audio': path, 'seconds': None}
            jobs.append((line, (rec[4:12], dat[starts[0]:starts[1]], OUT/path)))
    for (line, _), seconds in zip(jobs, side_by_side(lambda job: eaac_wav(*job[1]), jobs)):
        line['seconds'] = seconds
    return out


def all_events(data):
    for k in ('IntroPresentationEvents', 'OutroPresentationEvents', 'GotoPresentationEvents'):
        yield from data['self'].get(k) or []
    for o in data['objectives']:
        for k in ('IntroEvents', 'CompleteEvents', 'FailEvents'):
            yield from o.get(k) or []
        for h in o['Hints']:
            yield from h.get('Events') or []


def trick_groups(v, sources):
    wanted = set(re.findall(r'DidSpecificTrickGroup\w*\("(\w+)"\)', sources))
    names = set(re.findall(r'\("([a-z0-9_]+)",', (repo/'crates/skate-core/src/scoring/catalog.rs').read_text()))
    names |= set((TABLES/'s2_names.txt').read_text(encoding='utf-8').split())
    trick = {hash64(n): n for n in names}
    groups = {r['key']: r for (c, _), r in v.rows.items() if c == GROUP_CLASS}
    children = {}
    for k, r in groups.items():
        children.setdefault(r['parent'], []).append(k)
    def members(k):
        own = groups[k]['fields'].get('Hash_033C23A65A34839D')
        items = (own['array']['items'] if 'array' in own else [own['data']]) if own else []
        out = [trick.get(int(i[16:32], 16), f'?{i[16:32]}') for i in items]
        for c in children.get(k, []):
            out += members(c)
        return out
    out = {g: sorted(set(members(g))) for g in wanted}
    for g, m in out.items():
        assert m and not any(t.startswith('?') for t in m), f'trick group {g}: {m[:5]}'
    return out


# ---------------------------------------------------------------- AI paths, grinds
# Grind.DidSpecificGrind ids are rail definition ids (the 0x..03e38707 id a BAM grind spline carries), not trick names;
# both are the photo rail's, worldbam cSim_1700_-500_high.xsf at (1666, 102, -523) (RECOVERY.md, "Photo challenges").
GRIND_RAILS = {'450138407667463': {'rail_definition': '0x0001996603E38707', 'spline': '0x073AB37A1C12BF78'},
               '450370335901447': {'rail_definition': '0x0001999C03E38707', 'spline': '0x655FC9C6D3B2CD8C'}}


def ai_paths():
    """aipaths.big AIPATH_Challenge_<m_1a_0N>_photo.rx2: the photo subject's recorded runs, per path
    [[t, x, y, z, state], ...] in Skate 2 world space. Path headers at 0x250/0x2b0 (u32 0x0101610F; samples at
    header - 0x20 + w[4], count w[5]); 44-byte samples: pos, vel per frame, 3 words, q (top byte = frames since the
    previous sample, 60 Hz), state (top byte). AIPATH_Challenge_m_1a_07_follow.rx2: segment headers (u32 0x01016107,
    count at 0x220) at 0x250 + 0x60 k, two routes of chained segments; the leader rides the first."""
    def samples(d, h, magic):
        w = struct.unpack_from('>16I', d, h)
        assert w[0] == magic, f'path header {w[0]:#x}'
        t, rows = 0., []
        for k in range(w[5]):
            at = h - 0x20 + w[4] + 44 * k
            q, state = struct.unpack_from('>2I', d, at + 36)
            t += (q >> 24) / 60
            rows.append([round(t, 4)] + [round(x, 4) for x in struct.unpack_from('>3f', d, at)] + [state >> 24])
        return rows
    big = Big(s2game/'data/big/aipaths.big')
    out = {}
    for name in CHALLENGES:
        kind = 'follow' if 'follow' in name else 'photo'
        e = next((e for e in big.entries if f'AIPATH_Challenge_{"_".join(name.split("_")[:3])}_{kind}' in e.path), None)
        if e is None:
            continue
        d = big.read(e)
        if kind == 'photo':
            out[name] = [samples(d, h, 0x0101610F) for h in (0x250, 0x2b0)]
            continue
        segments = [samples(d, 0x250 + 0x60 * k, 0x01016107) for k in range(struct.unpack_from('>I', d, 0x220)[0])]
        route, left = segments[0], segments[1:]
        while nxt := next((s for s in left if math.dist(s[0][1:4], route[-1][1:4]) < 1), None):
            left.remove(nxt)
            route += [[round(r[0] + route[-1][0], 4)] + r[1:] for r in nxt]
        out[name] = [route]
    return out


def ai_segments():
    """The same aipaths.big files, whole (round2 T5, full-skater riders): per lesson the file's paths as
    tools/skate3_ai_paths.decode reads them (one layout in Skate 2 and 3: segment headers 0x60 from 0x230, nodes 44,
    ExtendedNodeData 40, branch triples), the per-tick displacement as "step", the decoded quats, widths, flags and
    events; branch weights dropped (written 0, never read: round2-research4 Q2). Skate 2 trick indices stay raw
    ("trick": Skate 2's table 0x820614c8, row for row Skate 3's first 269; `ai_tricks` has their rows)."""
    from tools import skate3_ai_paths
    big = Big(s2game/'data/big/aipaths.big')
    out = {}
    for name in CHALLENGES:
        kind = 'follow' if 'follow' in name else 'photo'
        e = next((e for e in big.entries if f'AIPATH_Challenge_{"_".join(name.split("_")[:3])}_{kind}' in e.path), None)
        if e is None:
            continue
        paths = skate3_ai_paths.decode(big.read(e), [])
        for p in paths:
            assert not p['anomalies'], (name, p['anomalies'])
            p['tricks'] = [{k: t[k] for k in ('node', 'trick', 'launch', 'pos', 'vel', 'aux')} for t in p['tricks']]
            p['branches'] = [{'node': b['node'], 'to': [{'path': t['path'], 'node': t['node']} for t in b['to']]}
                             for b in p['branches']]
            for k in ('raw', 'anomalies', 'key'):
                del p[k]
        out[name] = paths
    return out


# ---------------------------------------------------------------- RECOVERY.md walkthrough
def walkthrough(challenges, flow, text):
    say = lambda s: text.get(s, s) if isinstance(s, str) else s
    lines = ['\n## Generated walkthrough\n', 'Frontend: ' + ' -> '.join(
        f.get('movie') or ('Create-a-Skater' if f.get('customiser') else f['trigger']) for f in FRONTEND) + '\n']
    for t, trig in flow['triggers'].items():
        lines.append(f'### Trigger `{t}` (initial `{trig["initial"]}`)\n')
        for s, handlers in trig['states'].items():
            lines.append(f'- **{s}**: ' + ', '.join(f'`{e}`' for e in handlers))
        lines.append('')
    for name, data in challenges.items():
        me = data['self']
        lines += [f'### `{name}`: {say(me.get("Title"))}\n', f'{say(me.get("Description"))}\n',
                  f'Start `{me.get("StartLocation")}`; video `{me.get("TutorialMovie")}`.\n']
        for i, o in enumerate(data['objectives'], 1):
            d = o.get('Definition') or {}
            lines.append(f'{i}. `{o["Key"]}`: {say(d.get("text"))}')
            for e in o.get('IntroEvents') or []:
                lines.append(f'   - intro event {e["type"]:#x} `{e["a"]}` `{e["b"]}` {say(e["b"]) if str(e["b"]).startswith("ID_") else ""}')
            vol = re.findall(r'\|(\w+)"', d.get('source') or '')
            if vol:
                lines.append(f'   - volumes: {", ".join(sorted(set(vol)))}')
            for h in o['Hints']:
                lines.append(f'   - hint `{h["Key"]}`: ' + '; '.join(f'{e["type"]:#x} {e["a"]} {e["b"]}' for e in h.get('Events') or []))
        lines.append('')
    return '\n'.join(lines)


# ---------------------------------------------------------------- main
vaults = db_vaults()
if OUT.exists():
    shutil.rmtree(OUT)
for sub in ('scripts', 'movies', 'reference', 'speech', 'nis'):
    (OUT/sub).mkdir(parents=True)

challenges = {}
for name in CHALLENGES:
    challenges[name], lua = challenge(vaults['ChallengeCore'], name)
    challenges[name]['icon'] = minimap_icon(vaults['skater'], challenges[name]['self']['Hash_E616ACDF5A36BFAC'])
    (OUT/f'scripts/{name}.lua').write_text(lua, encoding='utf-8')
for c in challenges.values():
    for e in all_events(c):
        if e['type'] in (2, 3, 13):  # speech events: `a` is the speaker, a `speech` key
            e['a'] = SPEAKER_ALIAS.get(e['a'], e['a'])
            assert e['a'] in SPEAKERS, f'speech event speaker {e["a"]}'
flow, progression_lua = progression(vaults['progressioncore'])
(OUT/'scripts/progression.lua').write_text(progression_lua, encoding='utf-8')
dump = json.dumps(challenges)
objective_sources = '\n'.join(o['Definition']['source'] for c in challenges.values() for o in c['objectives']
                              if isinstance(o.get('Definition'), dict) and isinstance(o['Definition'].get('source'), str))
objective_sources += '\n'.join(h['Definition']['source'] for c in challenges.values() for o in c['objectives']
                               for h in o['Hints'] if isinstance(h.get('Definition'), dict) and isinstance(h['Definition'].get('source'), str))

# placement: every in-scope mission stream + the world locators the data names
locators, volumes, dmos = {}, {}, {}
for name in CHALLENGES:
    loc, vol, dmo = mission_placement(name)
    locators.update(loc); volumes.update(vol)
    if dmo:
        dmos[name] = dmo
world = world_locators()
locator_refs = {c['self'].get(k) for c in challenges.values() for k in ('StartLocation', 'MapStartLocation')} - {None}
locator_refs |= {o['VisualIndicators'][i]['locator'] for c in challenges.values() for o in [c['self']] + c['objectives']
                 for i in range(len(o.get('VisualIndicators') or []))}
locator_refs |= {e[{0x8: 'a', 0x1E: 'b'}[e['type']]] for c in challenges.values() for e in all_events(c) if e['type'] in (0x8, 0x1E)}  # teleports
locator_refs |= {m['properties'][0] for m in flow['messages'].values() if m['type'] == 'player_spawn'}
locator_refs |= {n for n in world if n.startswith('m_1a_01_postchallenge')}
locator_refs = {n for n in locator_refs if isinstance(n, str) and n}
for n in locator_refs:
    if n not in locators and n in world:
        locators[n] = world[n]
# Pumping's four indicators are world props (vi_pumping_tutorial_pumparea_1002..1005) that no stream on the disc
# carries; stand them on the corners of the pump-area floor volume (RECOVERY.md).
for i, corner in enumerate(volumes['m_2a_01_tutorial_slappy_vol05']['corners'], 1):
    locators.setdefault(f'm_2a_01_tutorial_slappy_vi0{i}', [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], corner + [1]])
missing_locators = sorted(locator_refs - set(locators))
volume_refs = set(re.findall(r'DIST_\w+\|(\w+)\|', objective_sources))
missing_volumes = sorted(volume_refs - set(volumes))

scene_names = {e['a'] for c in challenges.values() for e in all_events(c) if e['type'] in (0x5, 0x1D) and e['a']}  # 0x1D preload, 0x5 play
scene_names |= {e['b'] for c in challenges.values() for e in all_events(c) if e['type'] == 0x13}  # highlight cameras
scene_names |= {m['properties'][0] for m in flow['messages'].values() if m['type'] == 'nis'}
# The race template inherits the photo scenes' fields (highlight_film_killed, on no disc) and never plays them.
scene_names |= {c['self'][k] for c in challenges.values() for k in ('PhotoOutScene', 'PhotoKilledScene')
                if c['self'].get(k) and 'RaceHeats' not in c['self']}
scene_table = scenes(scene_names)
# Each track's clip bank, as-is from scene.big data/scene/anim/ (a track's `anim` names it). A Player track's
# female-skater bank is <scene>_Female1 (ProsArrival_Female1, ...): `female_anim` where the disc has one.
scene_big = Big(s2game/'data/big/scene.big')
anims = {e.path.removeprefix('data/'): e for e in scene_big.entries if e.path.startswith('data/scene/anim/')}
(OUT/'scene/anim').mkdir(parents=True, exist_ok=True)
for s in scene_table.values():
    for tr in s['tracks']:
        assert tr['anim'] in anims, f'scene bank {tr["anim"]} not in scene.big'
        female = tr['anim'].replace('_Player1.abin', '_Female1.abin')
        if female != tr['anim'] and female in anims:
            tr['female_anim'] = female
        for a in {tr['anim'], tr.get('female_anim', tr['anim'])}:
            (OUT/a).write_bytes(scene_big.read(anims[a]))
# Stream -> (bank, its resident headers big, header entry, body entry); headers are <stream>.snr.
streams = {}
for bank, res in (('english/nis.big', 'nisresident.big'), ('post.big', 'postresident.big')):
    b = Big(s2game/'data/audio'/bank)
    rb = Big_from_bytes(b.read(next(e for e in b.entries if e.path.endswith(res))))
    heads = {Path(e.path).name: e for e in rb.entries}
    for e in b.entries:
        if e.path.endswith('.sns'):
            streams[Path(e.path).name] = (b, rb, heads[Path(e.path).stem + '.snr'], e)
jobs = []
for name, s in scene_table.items():
    s.pop('posts', None)  # ponytail: the tutorial's SFX beds are SCENE_AUDIO's, named by hand
    assert all(a in streams for a in s['audio']), s['audio']
    s['audio'] = [{'stream': a, 'audio': f'nis/{name}{"_post" * i}.wav', 'seconds': None} for i, a in enumerate(s['audio'])]
    jobs += [(e, (streams[e['stream']][1].read(streams[e['stream']][2]), streams[e['stream']][0].read(streams[e['stream']][3]),
                  OUT/e['audio'])) for e in s['audio']]
for (e, _), seconds in zip(jobs, side_by_side(lambda job: eaac_wav(*job[1]), jobs)):  # vgmstream runs side by side
    e['seconds'] = seconds

boot = Big(s2game/'data/big/miscboot.big')
read = {e.path.lower(): e for e in boot.entries}
labels = table(boot.read(read['data/fe/languages/labels/language_labels_global_skate2ng.bin']))
values = table(boot.read(read['data/fe/languages/english/language_english_global_skate2ng.bin']))
movie_names = [f['movie'] for f in FRONTEND if 'movie' in f]
movie_names += [m['properties'][0] for m in flow['messages'].values() if m['type'] == 'fmv']
movie_names += [c['self']['TutorialMovie'] for c in challenges.values() if c['self'].get('TutorialMovie')]
movie_names = list(dict.fromkeys(movie_names))
cues = subtitles(movie_names + sorted(scene_table), labels, values)
for name, s in scene_table.items():
    s['subtitles'] = cues.get(name, [])
movies = {}
for name, m in zip(movie_names, side_by_side(movie, movie_names, gb_each=0.6)):  # ~0.6 GB of PCM for the intro movie
    movies[name] = m
    movies[name]['subtitles'] = cues.get(name, [])

speech = speech_lines(challenges, progression_lua, flow['messages'])
missing_speech = sorted(f'{spk} {i}' for spk, s in speech.items() for i, m in s['lines'].items() if not m)
groups = trick_groups(vaults['ChallengeCore'], objective_sources)
paths = ai_paths()
segments = ai_segments()
from tools.skate3_ai_paths import trick_table
ai_rows = ai_tricks(vaults['skater'], [n for n, _, _ in trick_table()[:S2_TRICKS]])
ai_riders = ai_profiles(vaults['skater'], sorted({c[0] for name in segments
                                                  for c in challenges[name]['self'].get('Competitors') or []}))

text = {labels[k]: values[k] for k in labels if k in values}
# The handler Lua isn't in the vault dump: it carries the template's ID_MISSION_CHALLENGE_* keys.
lua = ''.join(p.read_text(encoding='utf-8') for p in (OUT/'scripts').glob('*.lua'))
referenced = set(re.findall(r'\bID_[A-Z0-9_]+', dump + json.dumps(flow) + json.dumps(cues) + lua))
# The port's popup and result screens: Yes/No for the dialogs, Continue for the unlocks screen, Continue/Retry after a race.
referenced |= {'ID_COMMON_YES', 'ID_COMMON_NO', 'ID_COMMON_CONTINUE', 'ID_COMMON_RETRY'}
strings = {k: text[k] for k in sorted(referenced) if k in text}
missing_strings = sorted(referenced - set(strings))

tutorial = {'map': 'New_San_Vanelona', 'frontend': FRONTEND, 'progression': flow, 'challenges': challenges,
            'placement': {'locators': locators, 'volumes': volumes}, 'dmos': dmos, 'scenes': scene_table, 'movies': movies,
            # ponytail: grinds = any grind (the "grinds" group) per rail id; the spot volume pins it to the photo rail.
            # Exact would match the engine's rail to grind_rails' spline.
            'speech': speech, 'trick_groups': groups, 'grinds': {i: groups['grinds'] for i in GRIND_RAILS},
            'grind_rails': GRIND_RAILS, 'ai_paths': paths, 'ai_segments': segments,
            'ai_tricks': ai_rows, 'ai_profiles': ai_riders,
            'missing': {'locators': missing_locators, 'volumes': missing_volumes, 'speech': missing_speech,
                        'scenes': sorted(scene_names - set(scene_table)),
                        'strings': missing_strings}}
(OUT/'tutorial.json').write_text(json.dumps(tutorial, indent=1), encoding='utf-8')
(OUT/'strings.json').write_text(json.dumps(strings, indent=1, ensure_ascii=False), encoding='utf-8')
(OUT/'reference/RECOVERY.md').write_text((here/'tutorial_recovery.md').read_text(encoding='utf-8') +
                                         walkthrough(challenges, flow, text), encoding='utf-8')

# Self-check. Names the data references but the disc lacks are listed in tutorial.json "missing" and must stay
# within what RECOVERY.md accounts for (cut content), so a new gap fails here instead of in the game.
KNOWN_MISSING = {'speech': ['Atiba 1008', 'Atiba 1015', 'CameraMan 936'],'scenes': ['tutorial_m_2a_01_tutorial_slappy']}
for kind, names in tutorial['missing'].items():
    assert set(names) <= set(KNOWN_MISSING.get(kind, [])), f'unresolved {kind}: {names}'
for name, c in challenges.items():
    for step, events in c['steps'].items():
        for event, function in events.items():
            assert f'function {function}(' in (OUT/f'scripts/{name}.lua').read_text(encoding='utf-8')
# Every scene has its camera; Welcome Back's opens on its first key and cuts seven times.
assert all('camera' in s for s in scene_table.values()), [n for n, s in scene_table.items() if 'camera' not in s]
welcome = json.loads((OUT/scene_table['01_Welcome_Back']['camera']).read_text(encoding='utf-8'))['samples']
# The unshaken first eye is (0.59578, 0.58361, 0.05285); the shake moves it by centimetres.
assert len(welcome[0]) == 17 and math.dist(welcome[0][1:4], [0.59578, 0.58361, 0.05285]) < 0.1 and sum(x[0] == y[0] for x, y in zip(welcome, welcome[1:])) == 7
assert flow['activated'] == sorted({'m_1a_01_goto_slappys', 'm_1a_03_tutorial_fliptricks', 'm_1a_04_tutorial_manuals',
                                    'm_2a_01_tutorial_slappy', 'm_1a_05_photo_rail', 'm_1a_06_photo_puzzle',
                                    'm_1a_07_follow_slappy'}), flow['activated']
# Slappy's route chains six segments and ends in the finish volume the heat's one leg names.
follow, heat = paths['m_1a_07_follow_slappy'][0], challenges['m_1a_07_follow_slappy']['self']['RaceHeats'][0]
finish = volumes[heat['Legs'][0]['SplitTimeTriggers'][-1]]
assert len(follow) == 262 and round(follow[-1][0], 1) == 46.4 and all(finish['min'][i] <= follow[-1][1 + i] <= finish['max'][i] for i in range(3))
assert heat['StartLocation'] in locators
# Photo scenes snap 8 times; the puzzle's hints keep Hash_ECAEB8E619A68AF3's order; the rail's runs loop at 41.1/35.9 s.
assert all(scene_table[f'photo_{k}_{c}']['photos'] == 8 for k in ('out', 'killed') for c in ('m_1a_05_photo_rail', 'm_1a_06_photo_puzzle'))
assert [h['Key'][-3:] for h in challenges['m_1a_06_photo_puzzle']['objectives'][0]['Hints']] == ['0_5', '0_0', '0_2', '0_1', '0_6', '0_3', '0_4']
assert [round(p[-1][0], 2) for p in paths['m_1a_05_photo_rail']] == [41.08, 35.85]
# The puzzle's wedge is the DMO its tester names (AddDMOVolumeTester's argument is its instance id).
wedge = next(d for d in dmos['m_1a_06_photo_puzzle'] if d['template'] == 'WedgeRamp_large')
assert int(wedge['instance_id'], 16) == 11020444654411825821 and len(dmos['m_1a_06_photo_puzzle']) == 8
print(f'{len(challenges)} challenges, {sum(len(c["objectives"]) for c in challenges.values())} objectives, '
      f'{len(locators)} locators, {len(volumes)} volumes, {len(scene_table)} scenes, {len(movies)} movies, '
      f'{len(strings)} strings -> {OUT}')

# G8: the lessons' DMOs (old_tutorial_dmos.py) live in OUT too, which this export just replaced; regenerate them so
# a re-export never silently drops dmos.json (the engine then spawns no tutorial DMOs).
task = [sys.executable, '--task'] if getattr(sys, 'frozen', False) else [sys.executable]  # as derive.py / runner.script
subprocess.run([*task, str(here/'old_tutorial_dmos.py'), str(repo), str(s2game), str(assets)], check=True)
