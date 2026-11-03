"""Skate 1's cameraman tutorial (Skate_1_San_Vanelona): the data crates/skate-game/src/skate1_challenge.rs runs.

Writes assets/private/challenges-skate1/:
  tutorial.json         the lesson chain (miscgame.big tutorial_progress.sp), per lesson the merged vault fields
                        (`self`, field names resolved, places as names), steps -> events -> handler functions, the
                        speech IDs its scripts play; places: locators (BAM.loc) and trigger volumes (the lessons'
                        missions.big .rx2, world AABBs; Skate 1 world space = the converted map's space)
  scripts/<lesson>.lua  every handler decompiled by unluac: `function h_<KEY>(self, message, vars)`
  strings.json          every ID_ label the lessons and the tutorial screens use (English)
  progress.json         the career's progress.sp records (progress()): prerequisites, unlocks, slot actions, points
  speech/<id>_<n>.wav   every take of every cameraman line the scripts play (vgmstream, EAAC XMA -> 16-bit PCM)
  movies/<name>.vp6 + <name>.wav   Game_Intro_1 and _3 as-is, English audio downmixed to stereo PCM

usage (numpy; java on PATH): py -3.12 old_challenges.py REPO SKATE1_ROOT ASSETS
"""
import functools, json, os, re, shutil, struct, subprocess, sys, tempfile, wave
from pathlib import Path
import numpy as np

repo, s1game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(here)]
from s2big import Big
from s2lang import table
from tools.asset_pipeline.vlt import convert, hash64

OUT = assets/'private/challenges-skate1'
UNLUAC = Path(os.environ.get('SKATE_UNLUAC') or here/'tools/unluac.jar')
VGMSTREAM = Path(os.environ.get('SKATE_VGMSTREAM') or here/'tools/vgmstream/vgmstream-cli.exe')
LESSON_CLASS = 'Hash_8876E0B556740E36'
INTROS = ['Game_Intro_1', 'Game_Intro_3']
# Field hashes named by the handlers that read them or by their values.
ALIAS = {'Hash_3CB7BA2AFC9BDE42': 'Steps', 'Hash_6B0BD2BAE508B742': 'Description',
         'Hash_04D29EE3D05C88E0': 'CompleteDescription', 'Hash_05BD602B1F690E99': 'CompleteDescription2'}
# The career missions' fields the C++ reads (research.md 1.1 "Type 3", 4.1); kept out of ALIAS, which old_online1 shares.
MISSION_ALIAS = {'Hash_8022BA64CB8E431D': 'SignupRadius', 'Hash_555C505A36C3353A': 'GlobalSpawn',
                 'Hash_122DB7B0F026DBEC': 'GlobalType', 'Hash_C410489AC117B97E': 'AssetLoadType',
                 'Hash_16E7E79B28332502': 'ClearOtherNPCs', 'Hash_F2AE60AD0ED9D4BC': 'PosterFromCode'}
# The window (research 1.2) and, at the user's ask (10-15), the career's next record: Community Spirit (progress.sp record
# 5, the best trick vs Cuz), after which Danny Way's pro tutorial unlocks.
MISSIONS = ['he_u1_film01', 'he_u1_skate01', 'he_u2_follow01', 'he_u2_photo01', 'he_u3_film01', 'he_u4_besttrick01']
PRO_TUTORIALS = ['fp_u3_tutorial02', 'fp_u3_tutorial01', 'fp_u3_tutorial05', 'fp_u3_tutorial04', 'fp_u3_tutorial03']
# Labels the tutorial screens (hud_tutorial_challenge_overlay, tutorial_end, challenge_signup) ask for themselves.
SCREEN_STRINGS = ['ID_COMMON_MISSION_LEFT_AREA_WARNING', 'ID_TUTORIAL_LESSON_COMPLETE', 'ID_TUTORIAL_LESSON_FAILED',
                  'ID_TUTORIAL_TUTORIAL_COMPLETE', 'ID_COMMON_CONTINUE_CAPS', 'ID_COMMON_RETRY_CAPS', 'ID_COMMON_QUIT_CAPS',
                  # the missions' objective HUD (P5 L10)
                  'ID_OBJECTIVE_TITLE_GET_POINTS', 'ID_OBJECTIVE_BEATPOINTS',
                  *(f'ID_OBJECTIVE_TITLE_TRICK_{t}{s}' for t in ['FLIP', 'GAP', 'GRAB', 'GRIND', 'MANUAL', 'SPIN', 'REVERT']
                    for s in ['', 'S']),
                  'ID_CHALLENGE_FILM_HE_U1_FILM01_OBJ', 'ID_PRO_TUTORIAL_NOLLIE_OBJ1', 'ID_PRO_TUTORIAL_NOLLIE_OBJ1_FEET',
                  'ID_OBJECTIVE_NOSKATEZONE', 'ID_OBJECTIVE_GETLINE', 'ID_COMMON_NUMBER_OVER_NUMBER']
# "{0:d} m" / "{0:d} ft", the follow distance formats: no ID name, keyed by label hash as the subtitles are (P5 L10).
UNIT_FORMATS = ('92099686', '413c56da')
# No bank member has these: 8195 is lesson 05's NPC branch (only outside tutorial mode, never in this port), 1943 the
# Air lesson's spin-bail line (C1942 and C1944 exist; the line was cut). The engine plays nothing for them.
KNOWN_MISSING_SPEECH = {1943, 8195}
# Bytes per integer type, as the schema declares them (field size n); every enum (e*) is declared 4. vlt.convert exports
# an inline field as its whole 4-byte slot with the value left-aligned (all 1185 sub-4-byte slots pad with zeros).
INT_BYTES = {'Int8': 1, 'UInt8': 1, 'Int16': 2, 'UInt16': 2, 'Int32': 4, 'UInt32': 4, 'Int64': 8, 'UInt64': 8}


def label_key(label):
    x = 0xFFFFFFFF
    for c in label.encode('latin-1'):
        x = (x * 33 + c) & 0xFFFFFFFF
    return x


# ---------------------------------------------------------------- vault
class Vault:
    def __init__(self):
        names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
        stem = s1game/'data/db/skater'
        data = convert(stem, stem, names)
        self.bin = (s1game/'data/db/skater.bin').read_bytes()
        self.by_key = {}
        for r in data['collections']:
            self.by_key.setdefault(r['key'].removeprefix('Hash_').upper() if r['key'].startswith('Hash_')
                                   else '%016X' % hash64(r['key']), []).append(r)
        self.lessons = {r['key']: r for r in data['collections'] if r['class'] == LESSON_CLASS}

    def merged(self, key):
        out = {}
        while key:
            row = self.lessons[key]
            for k, v in row['fields'].items():
                out.setdefault(k, v)
            key = row['parent']
        return out

    def rows(self, field):
        items = field['array']['items'] if 'array' in field else [field['data']]
        return [r for i in items for r in self.by_key.get(i[16:32].upper(), [])]

    def string(self, ptr):
        if 0 < ptr < len(self.bin) and self.bin[ptr - 1] == 0:
            raw = self.bin[ptr:self.bin.index(b'\0', ptr)]
            if raw and all(32 <= b < 127 for b in raw):
                return raw.decode('latin-1')
        return None

    def scalar(self, t, data):
        if t == 'Text':
            return data
        raw = bytes.fromhex(data)
        if t == 'Bool':
            return raw[:1] != b'\0'
        if t == 'Float':
            return struct.unpack('>f', raw[:4])[0]
        if t.startswith(('Int', 'UInt', 'e')):
            return int.from_bytes(raw[:INT_BYTES.get(t, 4)], 'big', signed=t.startswith('Int'))
        w = struct.unpack('>%dI' % (len(raw) // 4), raw[:len(raw) // 4 * 4])
        if t == 'tLocationID':
            return self.string(w[0])
        if t == 'tTriggerVolumeInstanceID':  # "<district>|<volume>|0x<id>"
            s = self.string(w[0])
            return s.split('|')[1] if s else None
        if t == 'tVisualEditorData':  # the indicator's locator is its one string word
            return next((s for s in map(self.string, w) if s), None)
        if t in ('tSpeechInfo', 'tContextSpeechInfo', 'tFollowTheLeaderWarningInfo'):  # words; a line is s16 << 16
            return list(w)
        if t == 'tGlobalSpawnCharacterInfo':
            return spawn_row(raw, self.string)
        return data

    def value(self, field):
        t = field['type'].rsplit('::', 1)[-1]
        if 'array' in field:  # a Text array's items are pool offsets; the converter resolves them (NISFlyThroughScenes)
            return field['array']['text_items'] if t == 'Text' else [self.scalar(t, i) for i in field['array']['items']]
        return self.scalar(t, field['data'])


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


def spawn_row(raw, string):
    """Sk8::Challenge::tGlobalSpawnCharacterInfo (40 bytes, research 4.1): entity class +0 and key +8 (u64), +16/+20 0,
    locator +24, start mode +28 (0 removed / 1 stays / 2 moves to `second` while its challenge runs), second locator
    +32 (pool offset 8 = the empty string), +36 0."""
    cls, key, _, _, loc, mode, second, _ = struct.unpack_from('>QQIIIIII', raw)
    return {'class': '%016X' % cls, 'key': '%016X' % key, 'locator': string(loc), 'mode': mode,
            'second': string(second)}


def challenge(v, name, lua=True):
    """A challenge row by name (lesson, career mission or pro tutorial; all are LESSON_CLASS rows): `self` = the merged
    fields (named by ALIAS / MISSION_ALIAS), `steps` = step -> event -> handler function; lua=False skips unluac."""
    merged = v.merged(name)
    me = {}
    for k, field in merged.items():
        k = ALIAS.get(k) or MISSION_ALIAS.get(k, k)
        if k != 'Steps':
            me[k] = v.value(field)
    me['Name'] = name
    steps, lua_out, handlers = {}, [f'-- {name}: Skate 1 challenge handlers, decompiled by unluac'], []
    for step in v.rows(merged['Hash_3CB7BA2AFC9BDE42']):
        sname = step['fields']['Name']['data']
        steps[sname] = own = {}
        for handler in v.rows(step['fields'].get('Hash_810394ECEFE7E48B', {'data': ''}) or {'data': ''}):
            hname = handler['fields']['Name']['data']
            if 'Hash_C3CD4B4183274819' not in handler['fields']:
                # Three stateenter rows (class 038FA43A; S.H.I.N.G.O. new_state, Follow leaving / leader_warning) have
                # a tLuaScript pool offset instead of a blob; all point at the same empty `do end` text: no-ops.
                assert 'Hash_6391ACEAB5A03CD2' in handler['fields'], (name, sname, hname)
                own[hname] = None
                lua_out.append(f'-- [{sname}] {hname}: empty (no chunk)\n')
                continue
            size, offset = struct.unpack('>II', bytes.fromhex(handler['fields']['Hash_C3CD4B4183274819']['data']))
            handlers.append((sname, own, hname, v.bin[offset:offset + size]))
    if not lua:
        return {'self': me, 'steps': steps}, None
    prefetch([chunk for *_, chunk in handlers])  # as old_tutorial.challenge
    for sname, own, hname, chunk in handlers:
        function, source = decompile(chunk)
        own[hname] = function
        lua_out.append(f'-- [{sname}] {hname}\n{source.strip()}\n')
    return {'self': me, 'steps': steps}, '\n'.join(lua_out)


lesson = challenge


def chain():
    """tutorial_progress.sp: 0x400-byte records, each the lesson name then the lesson it unlocks after."""
    g = Big(s1game/'data/big/miscgame.big')
    d = g.read(next(e for e in g.entries if e.path.endswith('tutorial_progress.sp')))
    order = []
    for at in range(0, len(d) - 0x3FF, 0x400):
        names = re.findall(rb'fp_\w+', d[at:at + 0x400])
        if names:
            after = names[1].decode() if len(names) > 1 else None
            assert after == (order[-1] if order else None), f'chain out of order: {names}'
            order.append(names[0].decode())
    return order


def progress(data):
    """The career's progress.sp (miscgame.big data/progression/): u32 count, u32 record size, then records (research
    1.1). Per record: name +0, prerequisites +0x20/+0x40/+0x60, +0x80 (`hero_am`..., UNTRACED), +0xA0 (UNTRACED),
    pre-actions +0xC0 (appended to the completing record's actions when this one becomes available, 0x823fb850),
    unlocks +0xE0, actions in 0x20 slots from +0x2E0 run in slot order (executor 0x823fe948), words +0x3C0..+0x3EC
    with +0x3C8 points required and +0x3D0 points awarded (INFERRED from the running totals)."""
    n, size = struct.unpack_from('>II', data, 0)
    text = lambda r, at, end: r[at:end].split(b'\0')[0].decode('latin-1')
    split = lambda s: [x for x in s.split(';') if x]
    out = []
    for i in range(n):
        r = data[8 + i * size:8 + (i + 1) * size]
        words = {hex(at): struct.unpack_from('>I', r, at)[0] for at in range(0x3C0, 0x3F0, 4)}
        out.append({'name': text(r, 0, 0x20),
                    'after': [s for s in (text(r, at, at + 0x20) for at in (0x20, 0x40, 0x60)) if s],
                    'class': text(r, 0x80, 0xA0) or None, 'a0': text(r, 0xA0, 0xC0) or None,
                    'pre_actions': split(text(r, 0xC0, 0xE0)), 'unlocks': split(text(r, 0xE0, 0x2E0)),
                    'actions': [s for s in (text(r, at, at + 0x20) for at in range(0x2E0, 0x3C0, 0x20)) if s],
                    'words': {k: v for k, v in words.items() if v},
                    'points_required': words['0x3c8'], 'points_awarded': words['0x3d0']})
    return out


# ---------------------------------------------------------------- places
def world_locators():
    text = (s1game/'data/content/world/stream/BAM.loc').read_text(encoding='latin-1')
    out = {}
    for m in re.finditer(r'<Name>([^<]*)</Name>.*?<Transform>([^<]*)</Transform>', text, re.S):
        f = [float(x) for x in m.group(2).split(',')]
        out[m.group(1)] = {'matrix': [f[0:4], f[4:8], f[8:12], f[12:16]], 'position': f[12:15]}
    return out


def volumes(names):
    """Trigger volume table (RW4 object 0xEB000D) of the named challenges' missions.big .rx2: 0xA0-byte records (an
    identity matrix, then the world AABB at +0x40/+0x50; research 5.2 #17)."""
    missions = Big(s1game/'data/big/missions.big')
    out = {}
    # INFERRED (P6 D1): the disc spells two pros' arenas fp_u3_tutorial_04/_05.rx2, so match ignoring underscores;
    # the assert below keeps it to one arena per name.
    wanted = {n.replace('_', '') for n in names}
    hits = []
    for e in missions.entries:
        if Path(e.path).suffix != '.rx2' or Path(e.path).stem.replace('_', '') not in wanted:
            continue
        hits.append(Path(e.path).stem.replace('_', ''))
        d = missions.read(e)
        count, table_at = struct.unpack_from('>I', d, 0x20)[0], struct.unpack_from('>I', d, 0x30)[0]
        for i in range(count):
            at, _, size, _, _, kind = struct.unpack_from('>6I', d, table_at + 24 * i)
            if kind != 0xEB000D:
                continue
            _, n, _, first = struct.unpack_from('>4I', d, at)
            for r in range(n):
                o = at + first + 0xA0 * r
                ptr = at + struct.unpack_from('>I', d, o + 0x88)[0]
                full = d[ptr:d.index(b'\0', ptr)].decode('latin-1')
                out[full.split('_0x', 1)[0]] = {
                    'id': '0x%016x' % struct.unpack_from('>Q', d, o + 0x70)[0],
                    'min': list(struct.unpack_from('>3f', d, o + 0x40)),
                    'max': list(struct.unpack_from('>3f', d, o + 0x50))}
    assert len(hits) == len(set(hits)), f'an arena name matched twice: {hits}'
    return out


# ---------------------------------------------------------------- audio
def Big_from_bytes(data):
    tmp = Path(tempfile.mkdtemp(prefix='skate1-big-'))/'x.big'
    tmp.write_bytes(data)
    return Big(tmp)


def eaac_wav(header, body, out):
    """An EAAC (XMA) sound as 16-bit WAV: vgmstream reads the header and the block chain as an .snr/.sns pair."""
    with tempfile.TemporaryDirectory(prefix='skate1-eaac-') as work:
        snr = Path(work)/'a.snr'
        snr.write_bytes(header)
        snr.with_suffix('.sns').write_bytes(body)
        subprocess.run([str(VGMSTREAM), '-o', str(out), str(snr)], check=True, capture_output=True)
    with wave.open(str(out)) as w:
        return w.getnframes() / w.getframerate()


# ---------------------------------------------------------------- speech (research 4.6)
# Event id = bank << 13 | n; data/audio/english/<bank>speech.big holds <bank>Events.evt, <bank>hdr.big (sample id = a
# .hdr's first u16), <bank>sth.big (12 bytes per take) and the .dat audio (4.6 step 4).
SPEECH_BANKS = ['maincast', 'livingworld', 'cameraman', 'announcer']
# tMCSpeechTuning (4.6 step 5): per bank, the RefSpec table (row `default`, field 6921FD8E: 32-byte {u32 event, pad, u64
# class, u64 row}) and the class whose `default` row an unlisted event uses; the chance % is the row's 75E0969A +24.
TUNING_TABLES = ['Hash_62BD79E5134EAD89', 'Hash_939145C7117673A2', 'Hash_58028E80FA4BB2F1', 'Hash_8500343A0B83D289']
TUNING_DEFAULTS = ['A4830AAFC03D0B5C', 'A14B6CB341536B48', '1425B6C2AE523E7F', '3A2867260B238277']
MC_SPEECH_MAP = ('Hash_B29C3B2C13D96482', 'Hash_2FF5AA5D715E3824')  # {tag, mask, variant, ..} rows (lookup 0x822a5ee0)
# Exe tables (4.6 steps 2-3): {u32 line, u32 event} pairs per path, the speaker -> tag words, the NIS {u32 TT, speaker}.
CAMERAMAN_LINES, PHOTOGRAPHER_LINES, GENERIC_LINES = (0x82158668, 180), (0x82158C08, 8), (0x82158D78, 49)
# The announcer (speakers 32 / 33, 0x822c39f0 reads neither): its pairs, then player 0x822c6508's switch on event - 0x6004
# (table 0x822c663c): these hand the leaf evaluator the call's flags (0x822c66f8), every other event nothing (0x822c6734).
ANNOUNCER_LINES, ANNOUNCER_FLAG_EVENTS = (0x82158C48, 38), {0x6015, 0x6021, 0x6024}
SPEAKER_TAGS, NIS_SPEAKERS, NIS_LINE = 0x82156928, (0x82158F00, 42), 5000  # lines from 5000 play from the NIS table
# What the cameraman switch 0x822c6b44 hands the leaf evaluator: the call's flags (0x400e turns 0 into 0x800000 first,
# 0x822c6c94), the rank condition (0x822c6cd8: 1 << max(mgr+1556, mgr+1560), 2 -> 3, 4 and up -> 7), or nothing.
FLAG_EVENTS = {0x400E, 0x400F, 0x401B, 0x401E, 0x4054, 0x407E, 0x4131, 0x4138, 0x4139, 0x413A, 0x4147, 0x41B6, 0x41C8}
RANK_EVENTS = {0x4021, 0x4023, 0x402F, 0x4030, 0x4031, 0x4062, 0x40DE}
# The generic path rolls the event's own chance (0x822c7224), then plays 0x75 or 0xdb, 0x79 or 0x7a, 50/50 (0x822c758c).
SWAPS = {0x75: 0xDB, 0xDB: 0x75, 0x79: 0x7A, 0x7A: 0x79}


@functools.cache
def exe1():
    """Skate 1's memory image (base 0x82000000), rebuilt in memory from the player's own default.xex; never written."""
    from tools.build_skate2_image import build
    return build(s1game/'default.xex')


def exe_pairs(at, count):
    """{a: b} over `count` big-endian u32 pairs at an exe address; the lookups walk them in order, so the first wins."""
    out = {}
    for i in range(count):
        a, b = struct.unpack_from('>II', exe1(), at - 0x82000000 + 8 * i)
        out.setdefault(a, b)
    return out


def evt_events(data):
    """<bank>Events.evt: {event id: [(leaf masks, sample id)]}. Header u32 0x0315B007, .., u16 count @16, u16 event
    offsets x4 @24; event: u16 id, u16, u16, u8 leaves, u8, u8 flags, u8, u16 conditions, u16 leaf offsets x4 (from the
    event); leaf: 39 64 04 00, u32 LE n, u32 LE words, n u32 masks, u16 sample id."""
    assert struct.unpack_from('>I', data)[0] == 0x0315B007, data[:4].hex()
    count, = struct.unpack_from('>H', data, 16)
    out = {}
    for at in struct.unpack_from('>%dH' % count, data, 24):
        p = 4 * at
        eid, _, _, leaves = struct.unpack_from('>HHHB', data, p)
        out[eid] = []
        for off in struct.unpack_from('>%dH' % leaves, data, p + 12):
            r = p + 4 * off
            assert data[r:r + 4] == b'\x39\x64\x04\x00', (hex(eid), data[r:r + 4].hex())
            n, = struct.unpack_from('<I', data, r + 4)
            *masks, sid = struct.unpack_from('>%dIH' % n, data, r + 12)
            out[eid].append((tuple(masks), sid))
    return out


def leaf_matches(masks, condition):
    """INFERRED (4.6 step 6; the evaluator behind [0x83170c50] is UNTRACED): every mask meets its condition (AND non-zero,
    or both 0). An event the switch hands no condition matches every leaf."""
    return all(m & c or m == c == 0 for m, c in zip(masks, condition))


@functools.cache
def speech_tables(v):
    """The bank events, sample names (and where each lives), NIS lines, play chances, MCSpeechMap and exe tables."""
    events, samples, where = {}, {}, {}
    for bank in SPEECH_BANKS:
        big = Big(s1game/f'data/audio/english/{bank}speech.big')
        members = {Path(e.path).name: e for e in big.entries}
        events.update(evt_events(big.read(members[f'{bank}Events.evt'])))
        hdr = Big_from_bytes(big.read(members[f'{bank}hdr.big']))
        for e in hdr.entries:
            sid, = struct.unpack_from('>H', hdr.read(e))
            assert sid not in samples, (bank, e.path)
            samples[sid] = Path(e.path).stem
            where[samples[sid]] = bank
    nis = Big(s1game/'data/audio/english/nis.big')
    resident = Big_from_bytes(nis.read(next(e for e in nis.entries if e.path.endswith('nisresident.big'))))
    speakers, nis_lines = exe_pairs(*NIS_SPEAKERS) | {99: 34}, {}  # 0x822c4b78: TT 99 is speaker 34, before the table
    atoi = lambda s: int(re.match(r'\d*', s).group() or 0)
    for e in resident.entries:  # TT_NNNN_<who>: atoi of chars 0-1 and of chars 3-6 (0x822c4b70, 0x822c4bd8)
        stem = Path(e.path).stem
        tt, line = atoi(stem[:2]), atoi(stem[3:7])
        if tt in speakers:  # UNTRACED: the speaker a record with no pair keeps
            nis_lines.setdefault((speakers[tt], line), stem)  # lookup 0x822c4c70 walks the records in order
            where[stem] = 'nis'

    default = '%016X' % hash64('default')

    def row(cls, key):
        return next(r for r in v.by_key.get(key, []) if r['class'] == cls)

    listed = {}
    for table in TUNING_TABLES:
        for item in row(table, default)['fields']['Hash_6921FD8EB71E5E8F']['array']['items']:
            b = bytes.fromhex(item)
            listed.setdefault(struct.unpack_from('>I', b)[0], (b[8:16].hex().upper(), b[16:24].hex().upper()))

    def chance(event):
        cls, key = listed.get(event, (TUNING_DEFAULTS[event >> 13], default))
        tuning = bytes.fromhex(row('Hash_' + cls, key)['fields']['Hash_75E0969A8CEDB8EB']['data'])
        return round(struct.unpack_from('>f', tuning, 24)[0], 4)

    mcmap = {}
    for item in row(MC_SPEECH_MAP[0], default)['fields'][MC_SPEECH_MAP[1]]['array']['items']:
        tag, mask, variant = struct.unpack_from('>3I', bytes.fromhex(item))
        mcmap.setdefault(tag, (mask, variant))
    return {'events': events, 'samples': samples, 'where': where, 'nis': nis_lines, 'chance': chance,
            'mcmap': mcmap, 'cameraman': exe_pairs(*CAMERAMAN_LINES), 'photographer': exe_pairs(*PHOTOGRAPHER_LINES),
            'generic': exe_pairs(*GENERIC_LINES), 'announcer': exe_pairs(*ANNOUNCER_LINES),
            'tag': lambda speaker: struct.unpack_from('>I', exe1(), SPEAKER_TAGS - 0x82000000 + 4 * speaker)[0]}


def resolve(v, speaker, line, condition=0, rank=1):
    """A native's (speaker, line, flags) -> {'files': [sample], 'chance': %}, or None when nothing plays (dispatch
    0x822c37c8: a line missing from its table plays nothing, no fallback by number). `rank` (1 / 3 / 7) is the condition
    the cameraman's RANK_EVENTS read instead of the flags. Which take of a sample plays: UNTRACED."""
    t = speech_tables(v)
    if line >= NIS_LINE:  # the NIS line table: lookup 0x822c4c70 searches speaker 30 as 28; no chance roll
        stem = t['nis'].get((28 if speaker == 30 else speaker, line))
        return {'files': [stem], 'chance': 100.0} if stem else None
    assert speaker != 35, f'speaker {speaker}: its path (0x82308c58) is UNTRACED'
    if speaker in (32, 33):  # the announcer; it rolls its bank's chance too (0x822c6600 -> 0x822c6480)
        event = t['announcer'].get(line)
        condition = (condition,) if event in ANNOUNCER_FLAG_EVENTS else ()
    elif speaker == 31:
        event = t['cameraman'].get(line)
        condition = ((rank,) if event in RANK_EVENTS else (condition or 0x800000,) if event == 0x400E
                     else (condition,) if event in FLAG_EVENTS else ())
    elif speaker in (29, 30):  # Pho1 / Pho2; INFERRED: the photographer NPC must be spawned (0x8230a4a0)
        event, condition = t['photographer'].get(line), (0x10000, 1 if speaker == 29 else 2)
    else:  # the speaker's tag -> MCSpeechMap (mask, variant); UNTRACED: what 0x822a5ee0 gives a tag with no row
        event, condition = t['generic'].get(line), t['mcmap'].get(t['tag'](speaker), (0, 0))
    if event is None:
        return None
    swap = [SWAPS[event]] if speaker not in (29, 30, 31, 32, 33) and event in SWAPS else []
    files = [t['samples'][sid] for e in [event] + swap for masks, sid in t['events'][e] if leaf_matches(masks, condition)]
    return {'files': files, 'chance': t['chance'](event)} if files else None


SPEECH_TYPES = ('tSpeechInfo', 'tContextSpeechInfo', 'tFollowTheLeaderWarningInfo')


def speech_fields(v, name):
    """A challenge's speech fields by their vault type, named or not (he_u1_film01 / he_u3_film01 hold line-low 702 and
    time-warning 705 in fields known only by hash): [(type, value)]."""
    typed = ((f['type'].rsplit('::', 1)[-1], f) for f in v.merged(name).values())
    return [(t, v.value(f)) for t, f in typed if t in SPEECH_TYPES]


def speech_calls(sources, rows, career):
    """Every (speaker, line, flags) the scripts, their rows' speech fields and the progress slots can play (4.6 step 1):
    PlayCameramanSpeech(N) and the lessons' SuccessSpeech / speechID = N -> (31, N, 0); tSpeechInfo {char, s16 line} ->
    (char, line, 0); tContextSpeechInfo {char, line, context}, picked by type (speech_fields); FollowTheLeader warning rows (SetWarningInfo 0x827c22b8:
    +8 char, +12 line, +16 followee; recovery +20 char, +24 line, flags 0); `m=N` slot actions -> (31, N, 0)."""
    calls = set()
    for src in sources:
        assert not re.search(r'Play(?!Cameraman)\w*Speech\(\d', src), 'a literal speech call this reader skips'
        calls |= {(31, int(x), 0) for x in re.findall(r'PlayCameramanSpeech\((\d+)\)', src)}
        calls |= {(31, int(x), 0) for x in re.findall(r'\b(?:SuccessSpeech|speechID)\s*=\s*(\d+)', src) if int(x)}
    for fields in rows:  # speech_fields()
        for t, val in fields:
            if t == 'tFollowTheLeaderWarningInfo':
                for w in val:
                    calls |= {(w[2], w[3] >> 16, w[4]), (w[5], w[6] >> 16, 0)}
            else:  # one row, or an array of them (he_u4_besttrick01's AudioAnnouncerRoundIntro)
                for row in val if isinstance(val[0], list) else [val]:
                    calls.add((row[0], row[1] >> 16, row[2] if t == 'tContextSpeechInfo' else 0))
    for rec in career:
        calls |= {(31, int(a[2:]), 0) for a in rec['pre_actions'] + rec['actions'] if a.startswith('m=')}
    return calls


def speech_key(v, speaker, line, flags):
    """speech_table keys: "speaker:line:flags"; a rank line is keyed by its rank condition instead (speech_rank_lines)."""
    if speaker == 31 and line < NIS_LINE and speech_tables(v)['cameraman'].get(line) in RANK_EVENTS:
        return [(f'{speaker}:{line}:{rank}', resolve(v, speaker, line, rank=rank)) for rank in (1, 3, 7)]
    return [(f'{speaker}:{line}:{flags}', resolve(v, speaker, line, flags))]


def speech_files(v, stems):
    """Every take of each sample as speech/<sample>_<n>.wav: a bank sample's .dat split by its <bank>sth.big record (12
    bytes per take: u32 start, 8-byte header), a NIS line's .snr + .sns (one take). -> {sample: [{'audio', 'seconds'}]}."""
    where, out, jobs, opened = speech_tables(v)['where'], {}, [], {}
    for stem in sorted(stems):
        bank = where[stem]
        if bank not in opened:
            big = Big(s1game/f'data/audio/english/{bank}.big' if bank == 'nis' else
                      s1game/f'data/audio/english/{bank}speech.big')
            members = {Path(e.path).name: e for e in big.entries}
            inner = Big_from_bytes(big.read(members['nisresident.big' if bank == 'nis' else f'{bank}sth.big']))
            opened[bank] = big, members, {Path(e.path).stem: e for e in inner.entries}, inner
        big, members, headers, inner = opened[bank]
        rec = inner.read(headers[stem])
        if bank == 'nis':
            parts = [(rec, big.read(members[f'{stem}.sns']))]
        else:
            dat = big.read(members[f'{stem}.dat'])
            starts = [struct.unpack_from('>I', rec, k)[0] for k in range(0, len(rec) - 11, 12)]
            parts = [(rec[12 * n + 4:12 * n + 12], dat[s:(starts[n + 1] if n + 1 < len(starts) else len(dat))])
                     for n, s in enumerate(starts)]
        out[stem] = []
        for n, (header, body) in enumerate(parts):
            out[stem].append({'audio': f'speech/{stem}_{n}.wav', 'seconds': None})
            jobs.append((out[stem][-1], (header, body, OUT/out[stem][-1]['audio'])))
    for (take, _), seconds in zip(jobs, side_by_side(lambda job: eaac_wav(*job[1]), jobs)):  # vgmstream side by side
        take['seconds'] = seconds
    return out


def movie(name):
    src = s1game/f'data/movies/{name}_english_ntsc.vp6'
    shutil.copyfile(src, OUT/f'movies/{name}.vp6')
    meta = subprocess.run([str(VGMSTREAM), '-m', str(src)], capture_output=True, text=True).stdout
    samples = int(re.search(r'play duration: (\d+) samples', meta).group(1))
    with tempfile.TemporaryDirectory(prefix='skate1-movie-') as work:
        wav_path = Path(work)/'a.wav'
        subprocess.run([str(VGMSTREAM), '-o', str(wav_path), str(src)], check=True, capture_output=True)
        with wave.open(str(wav_path)) as w:
            channels, rate = w.getnchannels(), w.getframerate()
            pcm = np.frombuffer(w.readframes(w.getnframes()), '<i2').reshape(-1, channels).astype(np.float32)
    assert len(pcm) == samples, f'{name}: decoded {len(pcm)} frames, header says {samples}'
    if channels == 6:  # ponytail: assumes L R C LFE Ls Rs (as the Skate 2 port does); swap if the centre sounds wrong
        left = pcm[:, 0] + 0.707 * (pcm[:, 2] + pcm[:, 4])
        right = pcm[:, 1] + 0.707 * (pcm[:, 2] + pcm[:, 5])
        stereo = np.stack([left, right], 1)
        stereo *= min(1.0, 32767 / max(1.0, np.abs(stereo).max()))
    else:
        stereo = pcm[:, :2] if channels >= 2 else np.repeat(pcm, 2, 1)
    with wave.open(str(OUT/f'movies/{name}.wav'), 'wb') as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(rate)
        w.writeframes(stereo.astype('<i2').tobytes())
    video = video_seconds(src)
    return {'video': f'movies/{name}.vp6', 'audio': f'movies/{name}.wav', 'seconds': samples / rate,
            'video_seconds': video}


def video_seconds(path):
    """EA MVhd: frame count u32 LE @+0x10, fps = u32 LE @+0x18 / u32 LE @+0x1C (982047/32767 = 29.97)."""
    with open(path, 'rb') as f:
        d = f.read(0x20)
    assert d[:4] == b'MVhd', path
    frames, = struct.unpack_from('<I', d, 0x10)
    rate, scale = struct.unpack_from('<II', d, 0x18)
    return frames * scale / rate


# ---------------------------------------------------------------- subtitles
# r=2 in a progress.sp slot plays ROP table row 2 (exe 0x830a2ad0, research 1.2 step 16): this in-engine NIS.
ROP_SCENES = {2: 'NIS_ROP_2002_shingo_S01_0'}


def subtitles1(data, names, text):
    """miscboot subtitle.bin: 'subt', version, count, cue bytes, (label_key(name), offset) x count, then per name
    8-byte records (u16 tenths, u16 unread, u32 label; 0x001a20ba = clear) up to time 0xFFFF. A cue is the time its
    line appears (S1 subtitler 0x8248fcc0: cue x 100 <= elapsed ms; Skate 2's .sub stores END times); the line runs to
    the next record. The subtitler walks the records in file order, one per frame, so a cue never fires before the one
    ahead of it (research5.md 3). Labels are the hex keys strings.json gets (Skate 1 ships no label-name table). The
    u16 after the time is never read (research5.md 3b).
    ponytail: a cue held behind a later-timed one fires a frame after it; that one blank frame is dropped."""
    assert data[:4] == b'subt', data[:4]
    count, = struct.unpack_from('>I', data, 8)
    base, at = 16 + 8 * count, dict(struct.unpack_from('>II', data, 16 + 8 * i) for i in range(count))
    out = {}
    for name in names:
        if label_key(name) not in at:
            continue
        p, cues = base + at[label_key(name)], []
        while (t := struct.unpack_from('>H', data, p)[0]) != 0xFFFF:
            cues.append((max(t / 10, cues[-1][0] if cues else 0.), struct.unpack_from('>I', data, p + 4)[0]))
            p += 8
        lines = []
        for (start, label), (end, _) in zip(cues, cues[1:] + [(None, None)]):
            if label == 0x001A20BA:
                continue
            # NIS_DWay_ProTutorial_S01_0's 20.1 s cue (07d5a57e) names no string english.big holds. UNTRACED: what the
            # subtitler shows for a label its string table lacks; fallback: no line.
            if label not in text and (name, label) == ('NIS_DWay_ProTutorial_S01_0', 0x07D5A57E):
                continue
            assert label in text and end is not None, f'{name}: cue {label:08x} at {start} s'
            lines.append({'start': start, 'end': end, 'label': f'{label:08x}'})
        out[name] = lines
    return out



def english():
    """english.big's string tables: label_key(label) -> English text."""
    eng, text = Big(s1game/'data/big/english.big'), {}
    for e in eng.entries:
        if e.path.lower().endswith('.bin') and 'histogram' not in e.path.lower():
            text.update(table(eng.read(e)))
    return text

# ---------------------------------------------------------------- main
if OUT.exists():
    shutil.rmtree(OUT)
for sub in ('scripts', 'speech', 'movies'):
    (OUT/sub).mkdir(parents=True)

v = Vault()
order = chain()
lessons = {}
for name in order:
    lessons[name], lua = lesson(v, name)
    (OUT/f'scripts/{name}.lua').write_text(lua, encoding='utf-8')
sources = {name: (OUT/f'scripts/{name}.lua').read_text(encoding='utf-8') for name in order}
missions, pro_tutorials = {}, {}  # the career's next five (research 1.2 steps 12-23) and the in-window pro lessons (P6)
for name in MISSIONS + PRO_TUTORIALS:
    (missions if name in MISSIONS else pro_tutorials)[name], lua = challenge(v, name)
    (OUT/f'scripts/{name}.lua').write_text(lua, encoding='utf-8')
mission_sources = {n: (OUT/f'scripts/{n}.lua').read_text(encoding='utf-8') for n in MISSIONS + PRO_TUTORIALS}

world, vols = world_locators(), volumes(order)
locator_refs, volume_refs = set(), set()
for c in lessons.values():
    me = c['self']
    for k in ('Location', 'MapStartLocation', 'BestTrickStartLocations', 'VisualIndicators'):
        val = me.get(k)
        locator_refs |= {x for x in (val if isinstance(val, list) else [val]) if isinstance(x, str) and x}
    for k in ('OTSTriggerBoundary', 'OTSSpotVolumes'):
        val = me.get(k)
        volume_refs |= {x for x in (val if isinstance(val, list) else [val]) if isinstance(x, str) and x}
for c in list(missions.values()) + list(pro_tutorials.values()):  # every field value that names a BAM.loc locator
    for val in c['self'].values():
        for x in val if isinstance(val, list) else [val]:
            names = [x.get('locator'), x.get('second')] if isinstance(x, dict) else [x]
            locator_refs |= {n for n in names if isinstance(n, str) and n in world}
places = {'locators': {n: world[n] for n in sorted(locator_refs) if n in world},
          'volumes': {n: vols[n] for n in sorted(volume_refs) if n in vols} | volumes(MISSIONS + PRO_TUTORIALS)}
# A locator the challenges' own missions.big streams also carry gets their child spawns as `slots` (empty when it has
# none): RETAIL S1 0x823d9088 places RepositionAI's arg 3 / SpawnAIAtLocation's id at child[slot], past the last child
# at child 0 (no children: the locator) moved -5 m in z per slot; a BAM.loc-only locator is its matrix (0x823d8df0).
# overnight-1015 C4: without them Shingo and the player stood on one point.
from tools.online_challenge_map import global_locators
_streams = {n.replace('_', '') for n in order + MISSIONS + PRO_TUTORIALS}
_missions_big = Big(s1game/'data/big/missions.big')
for e in _missions_big.entries:
    if Path(e.path).suffix == '.rx2' and Path(e.path).stem.replace('_', '') in _streams:
        for k, loc in global_locators(_missions_big.read(e)).items():
            hit = next((n for n in places['locators'] if n.lower() == k), None)
            if hit:
                places['locators'][hit]['slots'] = loc['slots']

speech_ids = set()
for name, src in sources.items():
    ids = {int(x) for x in re.findall(r'PlayCameramanSpeech\((\d+)\)', src)}
    ids |= {int(x) for x in re.findall(r'\b(?:SuccessSpeech|speechID)\s*=\s*(\d+)', src)} - {0}
    lessons[name]['speech'] = sorted(ids)
    speech_ids |= ids
game = Big(s1game/'data/big/miscgame.big')
career = progress(game.read(next(e for e in game.entries if e.path.endswith('/progress.sp'))))
rows = {**lessons, **missions, **pro_tutorials}
calls = speech_calls([*sources.values(), *mission_sources.values()], [speech_fields(v, n) for n in rows],
                     [r for r in career if r['name'] in rows])
# PlayAnnouncerSpeech(N) (0x827bbb18) plays (32, N, 0), skipping N == 0; PlayContextedAnnouncerSpeech(N, context)
# (0x827bbb60) plays (32, N, context). Community Spirit hands them int-list fields and a literal context var.
# UNTRACED: the competitors' own lines (AI.GetCharacterAnnouncerWinningIndex); fallback: not exported.
for name, src in mission_sources.items():
    me, ctx = rows[name]['self'], {k: int(n) for k, n in re.findall(r'vars\.(\w+) = (\d+)\n', src)}
    calls |= {(32, n, 0) for f in re.findall(r'PlayAnnouncerSpeech\(self\.(\w+)\[', src) for n in me.get(f, []) if n}
    calls |= {(32, n, ctx[c]) for f, c in re.findall(r'PlayContextedAnnouncerSpeech\(self\.(\w+)\[[^]]*\], vars\.(\w+)\)', src)
              for n in me.get(f, []) if n}  # an unset field (AudioAnnouncerRoundStart) plays nothing
speech_table = dict(kv for call in sorted(calls) for kv in speech_key(v, *call))  # None = nothing plays (kept, 4.6)
rank_lines = sorted({l for s, l, _ in calls if len(speech_key(v, s, l, 0)) > 1})
speech_audio = speech_files(v, {f for e in speech_table.values() if e for f in e['files']})
# The lessons' cameraman lines by number, as skate1_challenge.rs reads them today (P2 moves it to speech_table).
lines = {str(i): [{'member': f, **take} for f in speech_table[f'31:{i}:0']['files'] for take in speech_audio[f]]
         for i in sorted(speech_ids) if speech_table.get(f'31:{i}:0')}

text = english()
referenced = set(re.findall(r'\bID_[A-Z0-9_]+', json.dumps([lessons, missions, pro_tutorials]) + ''.join(sources.values())
                            + ''.join(mission_sources.values()))) | set(SCREEN_STRINGS)
for name in order:  # every objective line, title and description of the chained lessons
    n = name[-2:]
    referenced |= {f'ID_CAMERAMAN_TUTORIAL_{n}_{s}' for s in ['TITLE', 'DESC', 'COMPLETEDESC'] + [f'OBJ{k}' for k in range(1, 10)]}
strings = {k: text[label_key(k)] for k in sorted(referenced) if label_key(k) in text}
scene_names = set(INTROS) | set(ROP_SCENES.values()) | {
    c['self'][k] for c in [*lessons.values(), *missions.values(), *pro_tutorials.values()]
    for k in ('NISFlyThroughScene', 'NISIntroOfferScene', 'NISIntroDeclineScene', 'NISIntroAcceptScene', 'NISOutroScene')
    if c['self'].get(k)}
boot = Big(s1game/'data/big/miscboot.big')
subs = subtitles1(boot.read(next(e for e in boot.entries if e.path.endswith('/subtitle.bin'))), sorted(scene_names), text)
strings |= {c['label']: text[int(c['label'], 16)] for cues in subs.values() for c in cues}
strings |= {k: text[int(k, 16)] for k in UNIT_FORMATS}

movies = dict(zip(INTROS, side_by_side(movie, INTROS, gb_each=0.6)))  # ~0.5 GB of PCM each

tutorial = {'map': 'Skate_1_San_Vanelona', 'chain': order, 'intros': INTROS, 'lessons': lessons, 'places': places,
            'speech': lines, 'movies': movies, 'missions': missions, 'pro_tutorials': pro_tutorials,
            'speech_table': speech_table, 'speech_rank_lines': rank_lines, 'speech_files': speech_audio}
(OUT/'tutorial.json').write_text(json.dumps(tutorial, indent=1), encoding='utf-8')
(OUT/'strings.json').write_text(json.dumps(strings, indent=1, ensure_ascii=False), encoding='utf-8')
(OUT/'subtitles.json').write_text(json.dumps(subs, indent=1), encoding='utf-8')
(OUT/'progress.json').write_text(json.dumps(career, indent=1), encoding='utf-8')

# ---------------------------------------------------------------- self-check
assert order == ['fp_u2_tutorial01', 'fp_u2_tutorial02', 'fp_u2_tutorial04', 'fp_u2_tutorial05'], order
assert [r['name'] for r in career[100:104]] == order and career[0]['after'] == [order[-1]], 'progress.sp lessons'
assert [r['name'] for r in career[:6]] == MISSIONS, 'progress.sp records 0-5: the window and Community Spirit'
shingo = missions['he_u1_skate01']['self']
spoken = {*INTROS, *(lessons[n]['self']['NISFlyThroughScene'] for n in order), *ROP_SCENES.values(),
          shingo['NISIntroOfferScene'], shingo['NISIntroDeclineScene']}
assert spoken <= {n for n, cues in subs.items() if cues}, f'no subtitles for {sorted(spoken - set(subs))}'
for name, c in list(missions.items()) + list(pro_tutorials.items()):
    me = c['self']
    for event in (f for events in c['steps'].values() for f in events.values() if f):
        assert f'function {event}(' in mission_sources[name], (name, event)
    for k in ('Location', 'MapStartLocation'):
        assert not me.get(k) or me[k] in places['locators'], f'{name}: {k} {me.get(k)} not in BAM.loc'
    for spawn in me.get('GlobalSpawn') or []:
        assert spawn['locator'] in places['locators'], f'{name}: spawn {spawn}'
    assert me.get('Title') in strings, f'{name}: title {me.get("Title")}'
for name, c in lessons.items():
    for step, events in c['steps'].items():
        for event, function in events.items():
            assert f'function {function}(' in sources[name], (name, step, event)
    luac = subprocess.run(['luac', '-p', '-'], input=sources[name], capture_output=True, text=True) \
        if shutil.which('luac') else None
    assert luac is None or luac.returncode == 0, f'{name}: {luac.stderr}'
    me = c['self']
    start = (me.get('BestTrickStartLocations') or [None])[0]
    assert start in places['locators'], f'{name}: start {start} not in BAM.loc'
    box = places['volumes'].get(me['OTSTriggerBoundary'])
    assert box, f'{name}: boundary {me["OTSTriggerBoundary"]} not in the rx2 volumes'
    p = places['locators'][start]['position']
    assert all(box['min'][k] <= p[k] <= box['max'][k] for k in (0, 2)), f'{name}: start {p} outside {box}'
    if 'OTSSpotVolumes' in sources[name]:  # 05 names a Test_Ph_Grinds volume no stream has, but never reads it
        for spot in me.get('OTSSpotVolumes') or []:
            assert spot in places['volumes'], f'{name}: spot volume {spot}'
    for vi in me.get('VisualIndicators') or []:
        assert vi in places['locators'], f'{name}: indicator {vi}'
    assert me.get('NISFlyThroughScene'), name
    for k in ('Title', 'Description'):
        assert me.get(k) in strings, f'{name}: {k} {me.get(k)}'
missing_speech = speech_ids - {int(k) for k in lines}
assert missing_speech <= KNOWN_MISSING_SPEECH, f'speech not in the bank: {sorted(missing_speech)}'
assert all(k in speech_table for call in calls for k, _ in speech_key(v, *call)), 'a speech call without a table entry'
for line in (101, 104, 1003, 5037):  # Seb's photo lines (research 4.6 "Lines in the window")
    assert speech_table[f'29:{line}:0'], line
assert speech_table['31:1401:1'] and speech_table['24:308:0'] and speech_table['31:314:0'], 'follow / S.H.I.N.G.O. lines'
for m in movies.values():
    # the audio runs a few seconds past the last frame (Game_Intro_1: 231.2 s vs 228.5 s); the player holds the last frame
    assert 0 <= m['seconds'] - m['video_seconds'] < 5.0, m
missing_strings = sorted(k for k in re.findall(r'"(ID_[A-Z0-9_]+)"', ''.join(sources.values())) if k not in strings)
assert not missing_strings, missing_strings
print(f'{len(lessons)} lessons, {len(places["locators"])} locators, {len(places["volumes"])} volumes, '
      f'{sum(map(len, lines.values()))} speech takes of {len(lines)} lines, {len(speech_table)} speech calls '
      f'({sum(1 for e in speech_table.values() if not e)} silent), {sum(map(len, speech_audio.values()))} takes of '
      f'{len(speech_audio)} samples, {len(strings)} strings, '
      f'{len(movies)} movies -> {OUT}')
