import struct, json
from icerecs import records, scene_names
P = json.load(open('ice_params.json'))
P.sort(key=lambda p: p['pos'])

def dequant(p, r):
    # 0x828456d8 with the zero point from 0x82867cf0: raw Z is exactly the default,
    # linear to min below it and to max above it.
    lo, de, hi = p['min'], p['default'], p['max']
    q = (1 << p['bits']) - 1
    up = (hi - de) * q / (hi - lo)
    n = float(int(up + 0.5))
    z = q - n
    d = r - z
    if d < 0 and z > 0: return de + (de - lo) * d / z
    if d > 0 and n > 0: return de + (hi - de) * d / n
    return de

def bits_le(block, n, bits, msb):
    # block as little-endian u32 words
    # big-endian bit stream, MSB first
    big = int.from_bytes(block, 'big')
    tot = len(block) * 8
    return [(big >> (tot - (i + 1) * bits)) & ((1 << bits) - 1) for i in range(n)]

def take(rec, msb=True, off=0x10):
    t = rec[off:]
    dur = struct.unpack_from('<f', t, 0xc)[0]
    tr = [struct.unpack_from('<HH', t, 0x14 + 4 * i) for i in range(37)]
    nidx = sum(max(k - 2, 0) for k, v in tr)
    ntim = sum(max(k - 1, 0) for k, v in tr)
    o = 0xa8
    idx = struct.unpack_from('<%dH' % nidx, t, o); o += 2 * nidx
    tim = struct.unpack_from('<%dH' % ntim, t, o); o += 2 * ntim
    o = (o + 3) & ~3
    vals = {}
    for p in P:
        k, v = tr[p['track']]
        n = v if p['flag'] == 0 else k
        size = ((n * p['bits'] + 31) >> 3) & ~3
        block = t[o:o + size]
        if p['type'] == 4:
            x = list(struct.unpack('<%df' % n, block[:4 * n]))
        else:
            raw = bits_le(block, n, p['bits'], msb)
            if p['type'] == 3:
                x = [dequant(p, r) for r in raw]
            else:
                x = raw
        vals[p['name']] = x
        o += size
    # per-track index/time slices
    ii = ti = 0
    keys = []
    for k, v in tr:
        keys.append((idx[ii:ii + max(k - 2, 0)], tim[ti:ti + max(k - 1, 0)]))
        ii += max(k - 2, 0); ti += max(k - 1, 0)
    return dict(dur=dur, tracks=tr, keys=keys, vals=vals, end=o + off, size=len(rec))

if __name__ == '__main__':
    import sys
    names = scene_names()
    want = sys.argv[1] if len(sys.argv) > 1 else '01_Welcome_Back'
    for tag, h, rec in records():
        nm = names.get(h, ('?',))[0]
        if want in nm:
            tk = take(rec)
            print(nm, hex(tag), 'takes', struct.unpack_from('<I', rec, 12)[0], 'end', tk['end'], 'size', tk['size'], 'dur', tk['dur'])
            print('tracks', [(i, k, v) for i, (k, v) in enumerate(tk['tracks']) if k or v])
