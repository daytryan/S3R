from params import f32_at, s_at
from xref import *
import json
def emulate(start, end, table_reg=21):
    r = {1: 0x7f000000}; f = {}; mem = {}; cur = None; table = {}; last = None
    for i in range((start - TEXT_VA) // 4, (end - TEXT_VA) // 4):
        w = words[i]; op = w >> 26; rd, ra, rb = (w >> 21) & 31, (w >> 16) & 31, (w >> 11) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        g = lambda x: r.get(x)
        if op == 14:
            r[rd] = simm & 0xffffffff if ra == 0 else ((g(ra) + simm) & 0xffffffff if g(ra) is not None else None)
        elif op == 15:
            r[rd] = (imm << 16) if ra == 0 else ((g(ra) + (imm << 16)) & 0xffffffff if g(ra) is not None else None)
        elif op == 24:
            r[ra] = (g(rd) | imm) if g(rd) is not None else None
        elif op == 48:  # lfs
            f[rd] = f32_at((g(ra) + simm) & 0xffffffff) if g(ra) is not None and ra != 1 else mem.get(('f', simm))
        elif op == 52 and ra == 1:  # stfs to stack
            mem[('f', simm)] = f.get(rd)
        elif op == 36:  # stw
            if ra == 1: mem[simm] = g(rd)
            elif ra == 3:
                cur = cur or {}; cur[simm] = g(rd)
            elif ra == table_reg and rd == 3:
                table[(simm - 0x10) // 4] = cur; cur = None
        elif op == 52 and ra == 3:
            cur = cur or {}; cur[('f', simm)] = f.get(rd)
        elif op == 32 and ra == 1:  # lwz from stack
            r[rd] = mem.get(simm)
        elif op == 32:
            r[rd] = None
        elif op == 63 and ((w >> 1) & 0x3ff) == 72:  # fmr
            f[rd] = f.get(rb)
        elif op == 31 and ((w >> 1) & 0x3ff) == 444:  # or / mr
            r[ra] = g(rd) if rd == rb else None
        elif op == 18 and (w & 1):
            tgt = (TEXT_VA + 4 * i + ((w & 0x3fffffc) - (0x4000000 if w & 0x2000000 else 0))) & 0xffffffff
            if tgt == 0x82867b38:
                last = {8: g(4), 0xc: g(5), 0x10: g(6), 0x14: g(7), 0x18: g(8), 0x1c: g(9), ('f', 0x20): f.get(1), ('f', 0x24): f.get(2), ('f', 0x28): f.get(3)}
            elif tgt == 0x82867bd8:
                last = {8: g(4), 0xc: g(5), 0x10: g(6), 0x14: g(7), 0x18: g(8), 0x1c: g(9), 0x20: g(10), 0x24: mem.get(0x54), 0x28: mem.get(0x5c)}
            elif tgt == 0x82845350:
                table[g(4)] = last; last = None
            r[3] = 0x7e000000
    return table
if __name__ == '__main__':
    i = (0x828533a8 - TEXT_VA) // 4
    while words[i] != 0x4e800020: i += 1
    t = emulate(0x828533a8, TEXT_VA + 4 * i)
    out = []
    for pos in sorted(k for k in t if k is not None):
        o = t[pos]
        if not o: print(pos, None); continue
        row = dict(pos=pos, name=s_at(o.get(8) or 0), track=o.get(0x10), flag=o.get(0x14), type=o.get(0x18), bits=o.get(0x1c),
                   default=o.get(('f', 0x20), o.get(0x20)), min=o.get(('f', 0x24), o.get(0x24)), max=o.get(('f', 0x28), o.get(0x28)),
                   cubic=o.get(0x3c, 0xffff), tension=o.get(0x40, 0xffff))  # pos links; 0xffff = none (was a one-off step, 10-01)
        out.append(row)
        print(row)
    json.dump(out, open('ice_params.json', 'w'), indent=0)
