from xref import *
def vals(lo, hi):
    val = {}; out = []
    for i in range(lo, hi):
        w = words[i]; op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if op == 15 and ra == 0: val[rd] = imm << 16
        elif op == 14 and ra in val:
            val[rd] = (val[ra] + simm) & 0xffffffff; out.append((i, val[rd]))
        elif op == 24 and rd in val:  # ori ra, rs(rd field), imm
            out.append((i, ('ori', (val[rd] | imm) & 0xffffffff)))
    return out
for i, w in enumerate(words):
    if w >> 26 == 24 and (w & 0xffff) < 0x40:
        rs = (w >> 21) & 31
        ok = any(words[j] >> 26 == 15 and (words[j] >> 16) & 31 == 0 and (words[j] >> 21) & 31 == rs and words[j] & 0xffff == 0x8000 for j in range(max(0, i - 6), i))
        if not ok: continue
        names = []
        for k, v in vals(max(0, i - 10), i + 4):
            if isinstance(v, int):
                o = va2off(v)
                if o and o < 0x2d0e00:
                    s = d[o:o + 40].split(b'\0')[0]
                    if s and all(32 <= c < 127 for c in s): names.append(s.decode())
        print(hex(TEXT_VA + 4 * i), hex(0x80000000 | (w & 0xffff)), names)
