import os, struct, sys, re
IMG = os.environ['SKATE_XEX_IMAGE']  # derive.py writes it from the player's Skate 2 default.xex
d = open(IMG, 'rb').read()
TEXT_RAW, TEXT_VA, TEXT_SIZE = 0x309800, 0x82310000, 0x9f9958
def va2off(va):
    if 0x82000400 <= va < 0x82000400 + 0x2d0a00: return va - 0x82000000
    if TEXT_VA <= va < TEXT_VA + TEXT_SIZE: return va - TEXT_VA + TEXT_RAW
    if 0x82d10000 <= va < 0x82d10000 + 0x7ec00: return va - 0x82d10000 + 0xd03200
    return None
def off2va(off):
    if TEXT_RAW <= off < TEXT_RAW + TEXT_SIZE: return off - TEXT_RAW + TEXT_VA
    return off + 0x82000000
words = struct.unpack_from('>%dI' % (TEXT_SIZE // 4), d, TEXT_RAW)
def xrefs(targets):
    targets = set(targets); out = []
    lis = {}
    for i, w in enumerate(words):
        op = w >> 26
        rd, ra = (w >> 21) & 31, (w >> 16) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if op == 15 and ra == 0:
            lis[rd] = (imm << 16, i)
        elif op in (14, 24) and ra in lis and i - lis[ra][1] < 64:
            t = (lis[ra][0] + (simm if op == 14 else imm)) & 0xffffffff
            if t in targets: out.append((TEXT_VA + 4 * i, t))
        elif op in (32, 48, 34, 40, 36, 52, 38) and ra in lis and i - lis[ra][1] < 64:  # lwz/lfs/lbz/lhz/stw/stfs/stb d(ra)
            t = (lis[ra][0] + simm) & 0xffffffff
            if t in targets: out.append((TEXT_VA + 4 * i, t))
    return out
def cstr(va):
    o = va2off(va); return d[o:d.index(b'\0', o)]
if __name__ == '__main__':
    pats = [s.encode() for s in sys.argv[1:]]
    tg = {}
    for p in pats:
        for m in re.finditer(re.escape(p), d[:0x2d0e00]):
            st = d.rfind(b'\0', 0, m.start()) + 1
            tg[0x82000000 + st] = d[st:d.index(b'\0', st)]
    for a, t in xrefs(tg):
        print(hex(a), hex(t), tg[t][:60])
