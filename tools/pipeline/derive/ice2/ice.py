import struct, sys
def parse(c):
    ver, size, h, one = struct.unpack_from('<4I', c, 0)
    dur = struct.unpack_from('<f', c, 0x1c)[0]
    # descriptor slots: u16 pairs from 0x20 until the index stream; find slot count by consistency
    for nslots in range(1, 200):
        descs = [struct.unpack_from('<HH', c, 0x20 + 4 * k) for k in range(nslots)]
        nt = sum(k - 1 for k, v in descs if k > 1)
        ni = sum(k - 2 for k, v in descs if k > 2)
        nv = sum(v for k, v in descs)
        at = 0x20 + 4 * nslots
        idx_at = at
        t_at = idx_at + 2 * ni
        v_at = t_at + 2 * nt
        v_at += (-v_at) % 4
        end = v_at + 4 * nv
        if end == len(c) or (nslots > 30 and end <= len(c) and descs[-1] == (0, 0) and False):
            break
    else:
        return None
    return dur, nslots, descs, idx_at, t_at, v_at, end
def curves(c):
    r = parse(c)
    dur, nslots, descs, idx_at, t_at, v_at, end = r
    out = []
    ip, tp, vp = idx_at, t_at, v_at
    for slot, (k, v) in enumerate(descs):
        if k == 0:
            continue
        times = [0] + list(struct.unpack_from('<%dH' % (k - 1), c, tp)) if k > 1 else [0]
        tp += 2 * max(k - 1, 0)
        offs = [0] + (list(struct.unpack_from('<%dH' % (k - 2), c, ip)) if k > 2 else [])
        ip += 2 * max(k - 2, 0)
        vals = list(struct.unpack_from('<%df' % v, c, vp)); vp += 4 * v
        if k > 1:
            offs.append(offs[-1] + (1 if v - offs[-1] - (1 if False else 0) else 0))
        out.append((slot, k, v, [t / 65535 * dur for t in times], offs, vals))
    return dur, nslots, out
if __name__ == '__main__':
    c = open(sys.argv[1], 'rb').read()
    print(parse(c)[:2], len(c))
