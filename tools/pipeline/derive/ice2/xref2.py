from xref import *
def xrefs2(lo, hi, window=200):
    """Any computed address (lis + addi chains, or base+disp loads) in [lo, hi)."""
    out = []
    val = {}
    for i, w in enumerate(words):
        op = w >> 26
        rd, ra = (w >> 21) & 31, (w >> 16) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if op == 18 and (w & 1) == 0 and (w & 2) == 0:   # unconditional b: keep
            pass
        if op == 15 and ra == 0:
            val[rd] = (imm << 16, i); t = None
        elif op == 15 and ra in val and i - val[ra][1] < window:
            val[rd] = ((val[ra][0] + (imm << 16)) & 0xffffffff, i)
        elif op == 14 and ra in val and i - val[ra][1] < window:
            t = (val[ra][0] + simm) & 0xffffffff
            val[rd] = (t, i)
            if lo <= t < hi: out.append((TEXT_VA + 4 * i, t))
        elif op in (32, 34, 40, 42, 48, 50) and ra in val and i - val[ra][1] < window:
            t = (val[ra][0] + simm) & 0xffffffff
            if lo <= t < hi: out.append((TEXT_VA + 4 * i, t))
            val.pop(rd, None)
        elif op in (14, 15, 32, 34, 40, 42, 31, 21, 24, 25, 28, 29) :
            val.pop(rd, None) if op not in (24, 25, 28, 29) else val.pop(ra, None)
    return out
if __name__ == '__main__':
    lo, hi = int(sys.argv[1], 16), int(sys.argv[2], 16)
    for a, t in xrefs2(lo, hi):
        o = va2off(t); s = d[o:o+40].split(b'\0')[0]
        print(hex(a), hex(t), s)
