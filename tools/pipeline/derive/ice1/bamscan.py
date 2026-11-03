import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import struct, sys, time
sys.path[:0] = [REPO_ROOT, REPO_ROOT + r'\tools\vendor\university\tools\vanilla_map_extraction\tools']
from skate3_streams import decompress_refpack
S = env('SKATE1_DISC', 'the extracted Skate 1 disc folder') + r'\data\content\world\stream\BAM'
x = open(S + '.xst', 'rb').read()
n = struct.unpack_from('>I', x, 16)[0]
recs = {}
for i in range(n):
    r = x[24 + 64 * i:88 + 64 * i]
    recs[struct.unpack_from('>Q', r, 0)[0]] = (struct.unpack_from('>I', r, 12)[0], struct.unpack_from('>I', r, 36)[0])
import collections
print(n, collections.Counter(t for _, t in recs.values()))
f = open(S + '.xsf', 'rb')
size = f.seek(0, 2)
cur = 0x80; seen = 0; t0 = time.time(); kinds = collections.Counter()
while cur < size and seen < 20:
    f.seek(cur); h = f.read(208)
    aid, stored, hsz, stride = struct.unpack_from('>QIII', h, 0)
    if aid == 0: break
    tot, typ = recs.get(aid, (None, None))
    print(hex(cur), hex(aid), stored, hsz, stride, 'type', typ, 'tot', tot, struct.unpack_from('>11I', h, 128))
    cur += stride if stride else ((hsz + stored + 127) & ~127)
    seen += 1

cur = 0x80; sims = []; t0 = time.time(); heads = collections.Counter()
while cur < size:
    f.seek(cur); h = f.read(208)
    if len(h) < 24: break
    aid, stored, hsz, stride = struct.unpack_from('>QIII', h, 0)
    if aid == 0: break
    tot, typ = recs[aid]
    if typ == 1:
        f.seek(cur + hsz); p = f.read(stored)
        heads[p[:2].hex()] += 1
        sims.append((cur, aid, stored, tot, p))
    cur += stride
print('walked', hex(cur), size, len(sims), 'sims', heads, round(time.time() - t0, 1), 's')
import pickle
found = 0
out = []
for cur, aid, stored, tot, p in sims:
    try:
        d = decompress_refpack(p) if p[:2] == b'\x10\xfb' else p
    except Exception as e:
        print('fail', hex(aid), e, p[:8].hex()); continue
    out.append((aid, d))
    if b'fp_u1_ownspot03_challengeboundary' in d:
        print('HIT', hex(aid), len(d), d[:4]); found += 1
pickle.dump(out, open('bam_sims.pkl', 'wb'))
print('found', found, 'decoded', len(out), 'magic', collections.Counter(d[:4] for _, d in out))
