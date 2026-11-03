"""Skate 1's scripted AI in the tutorial window (research.md 3): Shingo's recorded paths for S.H.I.N.G.O. and Follow
Shingo, the rider profiles his Competitors rows name, and the S.H.I.N.G.O. round filters.

Writes ASSETS/private/challenges-skate1/ai.json (old_challenges.py, which rmtrees challenges-skate1/, runs first):
  paths     {mission: [path]} aipaths.big AIPATH_Challenge_he_u1_skate01 / _he_u2_followleader01 (decode) in the rider
            driver's shape (tools/skate3_ai_paths.decode, Skate 2's ai_segments): name, pos, step, ticks, state,
            board, skater, widths, flags, branches, tricks
  lift      -0.087: every rolling node sits 0.087 m above the retail Sim collision, as in Skate 2 (3.3, RETAIL run 2;
            measured by scratch/r2/s1pathground.py); positions stay as recorded
  rounds    {mission: [[path name]]} RoundPaths, S.H.I.N.G.O.'s 4 rounds (FilterPaths keeps the loaded paths named, 3.9)
  riders    {mission: [profile]} the mission's Competitors rows in order (`shingo_skate|0` -> shingo_skate): retail
            AI.SelectCharacters (0x827c1538 -> 0x823f2e90) stores a kind-0 row's character (0x823f3278) in slot
            0x218 + 8n, n in row order
  profiles  {profile: fields} (profile)
  tiers     {trick: difficulty tier} class 6918469984A8C596 Hash_183878B4AFE58B7F, the landing roll's base (3.2)

usage: py -3 old_ai1.py REPO SKATE1_ROOT ASSETS
"""
import json, struct, sys
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(here)]
from s2big import Big
from tools.asset_pipeline.vlt import hash64
from tools.skate3_ai_paths import f32, quat, trick_table

MAIN = '# ' + '-' * 64 + ' main'  # the marker, built: this file is split on it too
FILES = {'he_u1_skate01': 'he_u1_skate01', 'he_u2_follow01': 'he_u2_followleader01',
         'fp_u3_tutorial02': 'fp_u3_tutorial02',  # P6: P-Rod's AI leader  # mission -> AIPATH_Challenge_<file>
         'he_u4_besttrick01': 'he_u4_besttrick01'}  # user 10-15: Community Spirit's three competitors
PATH_DATA, HEADER, NODE, EXT = 0x00EB0014, 0x50, 40, 56  # Skate 3: 0x60 / 44 / 40
S1_TRICKS = 139  # s1.pe 0x820cf940 == Skate 3's IDENTIFIERS[:139] (D23)
PROFILE, CHARACTER, SCORABLE = 'E9407D230E506420', 'E57478F796EBC28C', '6918469984A8C596'
ABD, TIER = 'Hash_CCD014669DD09FCB', 'Hash_183878B4AFE58B7F'


def lib(script):
    """A sibling script's library half (its main body rmtrees its own output)."""
    ns = {'__name__': script[:-3] + '_lib', '__file__': str(here/script)}
    argv, sys.argv = sys.argv, [script, str(repo), str(s1game), str(assets)]
    try:
        exec(compile((here/script).read_text(encoding='utf-8').split(MAIN)[0], str(here/script), 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


def decode(d, stem, tricks):
    """One AIPATH .rx2 (3.3): RW4 section 0xEB0014 {u32 count, u32 header offset}; 0x50-byte headers {16-byte key,
    node offset +0x10 / count +0x14, bbox +0x20 / +0x30, ext offset +0x40, branch table +0x44 / count +0x48}; 40-byte
    nodes {pos, step, board +24 / skater +28 quats, ext +32, ticks +36, width +37 (x 0.01 m), event +38 (1 trick start,
    2 trick end), flag bits +39 (0x01 flipped, 0x02 crouched)}; 56-byte ext records {start pos, vel, gravity, offset,
    s16 yaw (degrees) +48, s16 pitch +50 (unit UNTRACED), u16 trick +52, u8 type +54, u8 valid trajectory +55}, laid out
    in node order from the ext offset (asserted). Branch triples as Skate 3's; their weights are never read (3.9)."""
    u = lambda o: struct.unpack_from('>I', d, o)[0]
    index, count = u(0x30), u(0x20)
    base = next(s[0] for s in (struct.unpack_from('>6I', d, index + 24 * i) for i in range(count)) if s[5] == PATH_DATA)
    first = base + u(base + 4)

    def name(key):
        """[0..4] are the file's (1, 0, unlock, kind, row), [5..11] the letters, [12..15] a u32 index: the RoundPaths
        spelling, <file>[_<letters>]_<index>."""
        assert key[:5] == d[first:first + 5], key.hex()
        letters = key[5:12].split(b'\0')[0].decode('latin-1')
        return f"{stem}{'_' + letters if letters else ''}_{struct.unpack('>I', key[12:16])[0]}"

    paths = []
    for i in range(u(base)):
        h = first + HEADER * i
        no, nc = struct.unpack_from('>2I', d, h + 0x10)
        eo, bo, bc = struct.unpack_from('>3I', d, h + 0x40)
        p = {'name': name(d[h:h + 16]), **{k: [] for k in ('pos', 'step', 'ticks', 'state', 'board', 'skater', 'widths',
                                                             'flags', 'tricks', 'branches')}}
        for k in range(nc):
            q = h + no + NODE * k
            p['pos'].append([f32(x) for x in struct.unpack_from('>3f', d, q)])
            p['step'].append([f32(x) for x in struct.unpack_from('>3f', d, q + 12)])
            p['board'].append(quat(d[q + 24:q + 28]))
            p['skater'].append(quat(d[q + 28:q + 32]))
            p['ticks'].append(d[q + 36])
            # One width (Skate 2: left and right, 0.02 m) as both sides in the driver's unit. INFERRED: the corridor
            # test is Skate 2's (distance < width); whether Skate 1's byte is a half or full width is UNTRACED.
            p['widths'].append([d[q + 37] // 2] * 2)
            p['state'].append(d[q + 38])
            p['flags'].append(d[q + 39])
            if ext := u(q + 32):
                x = q + ext
                assert x - h == eo + EXT * len(p['tricks']), (p['name'], k)
                v = [f32(f) for f in struct.unpack_from('>12f', d, x)]
                yaw, pitch, trick = struct.unpack_from('>hhH', d, x + 48)
                assert trick < S1_TRICKS, (p['name'], k, trick)
                p['tricks'].append({'node': k, 'trick': trick, 'name': tricks[trick], 'type': d[x + 54],
                                    'launch': d[x + 55] == 1, 'pos': v[0:3], 'vel': v[3:6], 'gravity': v[6:9],
                                    'offset': v[9:12], 'yaw': yaw, 'pitch': pitch})
        for j in range(bc):
            at = h + bo + 12 * j
            off, n, node = struct.unpack_from('>3I', d, at)
            p['branches'].append({'node': node, 'to': [{'path': name(d[e:e + 16]), 'node': u(e + 16)}
                                                       for e in (at + off + 24 * m for m in range(n))]})
        paths.append(p)
    sizes = {p['name']: len(p['pos']) for p in paths}
    for p in paths:
        for b in p['branches']:
            assert b['node'] < len(p['pos']) and all(t['node'] < sizes[t['path']] for t in b['to']), (p['name'], b)
    return paths


def hex_key(key):
    return key.removeprefix('Hash_').upper() if key.startswith('Hash_') else '%016X' % hash64(key)


def merged(v, cls, key):
    """A row of class `cls` and its parents' fields (old_challenges.Vault rows)."""
    out = {}
    while key:
        row = next(r for r in v.by_key[hex_key(key)] if hex_key(r['class']) == cls)
        for k, f in row['fields'].items():
            out.setdefault(k, f)
        key = row['parent']
    return out


def profile(v, key):
    """Class E9407D230E506420 row `key`, the fields the trick chooser at controller +2736 reads by offset (3.2, RETAIL):
    gesture +372 (6F97: % a positive gesture plays); land chance % by the trick's difficulty tier 0..3, [913D +384,
    F83A +388, 0602 +408, F83A +388]; decay +392 (1DD7: the chance x decay^n, n UNTRACED); ollie +396 / nollie +380
    (the flip re-roll's regular / nollie shares); grabs +400 (6DBA: off, the grab handler does nothing); flip lists
    [[trick, weight]] regular +212 (E580) and nollie +4 (BB90); abd: the character row's % (CCD01466) that the Lua
    rolls against for StartSkatePath's used-trick avoid (3.4). 782E +376 and E518 +404 have no reader found (UNKNOWN)."""
    f = merged(v, PROFILE, key)
    num = lambda h: v.value(f['Hash_' + h] if 'Hash_' + h in f else f[h])
    flips = lambda h: [[t, f32(w)] for t, w in (struct.unpack('>If', bytes.fromhex(i)[:8])
                                                 for i in f['Hash_' + h]['array']['items'])]
    land = [f32(num(h)) for h in ('913D2B5F07880189', 'F83A843573435E6C', '06025FDC568AA983', 'F83A843573435E6C')]
    return {'gesture': f32(num('6F970C90412AD652')), 'land': land, 'decay': f32(num('1DD7BEA0203DD57B')),
            'ollie': f32(num('ollie')), 'nollie': f32(num('nollie')), 'grabs': num('6DBAF7AD6CDE6A16'),
            'regular': flips('E580B6284639E03F'), 'nollie_flips': flips('BB901D68361E9833'),
            'abd': v.value(merged(v, CHARACTER, key)[ABD]),
            'unknown': {'782E': num('782E7345CDF39DFD'), 'E518': f32(num('E5183166A978C33A'))}}


def tiers(v, names):
    """Trick name -> its scorable row's difficulty tier (an enum word; the class rows hold the defaults)."""
    out = {}
    for n in names:
        if any(hex_key(r['class']) == SCORABLE for r in v.by_key.get(hex_key(n), [])):
            out[n] = int(merged(v, SCORABLE, n)[TIER]['data'], 16)
    return out


def rounds(v, raw):
    """RoundPaths: per round 208 bytes, 50 u32 name offsets into skater.bin (8 = the empty string), then `all` (+0xc8:
    1 = no filter, RETAIL FilterPaths 0x823f2db0; 0 on every window row) and a flag (+0xcc) FilterPaths copies to
    filter+0xf4 (getter 0x823f2e60, no caller found: UNTRACED; Community Spirit's round 1 sets it). Not exported."""
    out = []
    for item in raw:
        words = struct.unpack('>52I', bytes.fromhex(item))
        assert words[50] == 0, words[50:]
        out.append([s for s in map(v.string, words[:50]) if s])
    return out


# ---------------------------------------------------------------- main
C = lib('old_challenges.py')
v = C['Vault']()
tricks = [n for n, _, _ in trick_table()]
big = Big(s1game/'data/big/aipaths.big')
ai = {'lift': -0.087, 'paths': {}, 'rounds': {}, 'riders': {}, 'profiles': {}}
for mission, stem in FILES.items():
    entry = next(e for e in big.entries if e.path.endswith(f'/AIPATH_Challenge_{stem}.rx2'))
    ai['paths'][mission] = decode(big.read(entry), stem, tricks)
    me = C['challenge'](v, mission, lua=False)[0]['self']
    # tCompetitorInfo {u32 name, u32 kind}; kind 1 is the random pick (0x823f3b00) no window row uses.
    assert all(c[8:16] == '00000000' for c in me['Competitors']), (mission, me['Competitors'])
    ai['riders'][mission] = [v.string(int(c[:8], 16)).split('|')[0] for c in me['Competitors']]
    for rider in ai['riders'][mission]:
        ai['profiles'][rider] = profile(v, rider)
    if 'RoundPaths' in me:
        ai['rounds'][mission] = rounds(v, me['RoundPaths'])
ai['tiers'] = tiers(v, tricks[:S1_TRICKS])
out = assets/'private/challenges-skate1'
out.mkdir(parents=True, exist_ok=True)
(out/'ai.json').write_text(json.dumps(ai, separators=(',', ':')), encoding='utf-8')
print(f"{sum(len(p) for p in ai['paths'].values())} paths, {len(ai['profiles'])} profiles, {len(ai['tiers'])} tiers")
