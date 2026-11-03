import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, struct, json
from pathlib import Path
HERE = Path(env('SKATE_TABLES', 'the skate2port tables folder'))
REPO = Path(REPO_ROOT)
S2 = Path(env('SKATE2_DISC', 'the extracted Skate 2 disc folder'))
sys.path[:0] = [str(REPO), str(REPO/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(HERE)]
src = (HERE/'old_tutorial.py').read_text(encoding='utf-8')
T = {'__name__': 'old_tutorial_lib', '__file__': str(HERE/'old_tutorial.py')}
argv, sys.argv = sys.argv, ['old_tutorial.py', str(REPO), str(S2), str(Path('.').resolve()/'t2')]
exec(compile(src.split('\nvaults = db_vaults()')[0], str(HERE/'old_tutorial.py'), 'exec'), T)
sys.argv = argv
P = sorted(json.load(open(HERE/'ice_params1.json')), key=lambda p: p['pos'])
NT = 34
def ice_take(t):
    tracks = [struct.unpack_from('<HH', t, 0x14 + 4 * i) for i in range(NT)]
    base = 0x14 + 4 * NT
    n_index, n_time = sum(max(k - 2, 0) for k, _ in tracks), sum(max(k - 1, 0) for k, _ in tracks)
    index = struct.unpack_from(f'<{n_index}H', t, base)
    times = struct.unpack_from(f'<{n_time}H', t, base + 2 * n_index)
    keys, i, j = [], 0, 0
    for k, v in tracks:
        keys.append(([0] + list(index[i:i + max(k - 2, 0)]) + [v - 2] if k > 1 else [0] * k,
                     [0.] + [x / 65535 for x in times[j:j + max(k - 1, 0)]] + [1.]))
        i, j = i + max(k - 2, 0), j + max(k - 1, 0)
    o = (base + 2 * (n_index + n_time) + 3) & ~3
    values = {}
    for p in P:
        k, v = tracks[p['track']]
        n = k if p['flag'] else v
        size = ((n * p['bits'] + 31) >> 3) & ~3
        block = t[o:o + size]
        if p['type'] == 4:
            values[p['name']] = list(struct.unpack_from(f'<{n}f', block))
        else:
            bits, mask = int.from_bytes(block, 'big'), (1 << p['bits']) - 1
            raw = [(bits >> (8 * size - (x + 1) * p['bits'])) & mask for x in range(n)]
            values[p['name']] = [T['ice_dequant'](p, x) for x in raw] if p['type'] == 3 else raw
        o += size
    return {'duration': struct.unpack_from('<f', t, 0xc)[0], 'keys': keys, 'values': values, 'size': o, 'tracks': tracks}
e = open('ice.bin', 'rb').read()
out = {}; bad = 0
o, end = 8, 8 + struct.unpack_from('<I', e, 4)[0]
while o < end:
    ce = o + 8 + struct.unpack_from('<I', e, o + 4)[0]; r = o + 8
    while r + 8 <= ce:
        _, size, h, n = struct.unpack_from('<4I', e, r)
        takes, at = [], r + 0x10
        for _ in range(n):
            tk = ice_take(e[at:r + 8 + size]); takes.append(tk)
            at = (at + tk['size'] + 3) & ~3
        if at != r + 8 + size: bad += 1
        out[h] = takes; r += 8 + size
    o = ce
print(len(out), 'records; size mismatches', bad)
d = open('scene.bin', 'rb').read()
n = struct.unpack_from('>I', d, 8)[0]
names = {}
for i in range(n):
    q = 0x10 + i * 0x68
    names[struct.unpack_from('>I', d, q)[0]] = (d[q + 0x28:q + 0x68].split(b'\0')[0].decode(), d[q+4], struct.unpack_from('>7f', d, q+8), struct.unpack_from('>HH', d, q+0x24))
multi = [names.get(h, ('?',))[0] for h, t in out.items() if len(t) > 1]
print('multi-take', multi)
import collections
sp = collections.Counter()
used = collections.Counter()
for h, ts in out.items():
    for t in ts:
        sp[(tuple(set(t['values']['SPACE_EYE'])), tuple(set(t['values']['SPACE_LOOK'])))] += 1
        for tr, (k, v) in enumerate(t['tracks']):
            if k: used[tr] += 1
        if any(t['values']['SHAKE_AMPLITUDE']): used['shake'] += 1
print(sp); print(sorted(used.items(), key=str))
json.dump({names[h][0] if h in names else hex(h): [{'duration': t['duration'], 'tracks': t['tracks']} for t in ts] for h, ts in out.items()}, open('ice_summary.json', 'w'), indent=0)

want = [n for h, n in ((h, names[h][0]) for h in out if h in names) if n.startswith(('OL_', 'Fp_u1_ownspot_')) or '_besttrick0' in n or n in ('he_u5_demo01', 'ah_p1_deathrace06', 'ah_p1_jam02', 'he_a1_jam01')]
for n in []:
    h = next(k for k, v in names.items() if v[0] == n)
    ts = out.get(h, [])
    print(n, 'kind', names[h][1], 'takes', len(ts), [(round(t['duration'], 2), sorted(set(t['values']['SPACE_EYE'])), sorted(set(t['values']['SPACE_LOOK'])), 'shake' if any(t['values']['SHAKE_AMPLITUDE']) else '', 'sub' if any(t['values']['CONTAINS_SUBTAKE']) else '', [i for i,(k,v) in enumerate(t['tracks']) if k]) for t in ts])
for n in []:
    h = next(k for k, v in names.items() if v[0] == n)
    for t in out.get(h, []):
        vals = {k: sorted(set(round(x, 3) if isinstance(x, float) else x for x in t['values'][k])) for k in ('OVERLAY', 'SHAKE_TYPE', 'SHAKE_AMPLITUDE', 'AUDIO_POST_ID', 'AUDIO_POST_CMD', 'AUDIO_SPEECH_ID', 'FADE', 'FADE_TO_COLOR', 'LENS_OVERRIDE', 'FISHEYE_OVERRIDE', 'TONEMAP_OVERRIDE', 'BLOOM_OVERRIDE')}
        print(n, {k: v for k, v in vals.items() if v and v != [0]})
order = list(out)
for i, (h, ts) in enumerate(out.items()):
    for t in ts:
        if t['tracks'][33][0]:
            print('shake record', i, hex(h), names.get(h, ('?',))[0], 'tracks', [(j, kv) for j, kv in enumerate(t['tracks']) if kv[0]], 'dur', t['duration'])
print('first records', [(i, hex(h), names.get(h, ('?',))[0]) for i, h in enumerate(order[:8])])
