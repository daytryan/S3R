"""Skate 1 ICE parameter registration (default.xex 0x82c3e6d8..0x82c436dc) emulated linearly -> ice_params1.json.
Objects (0x6c bytes): +8 name, +0xc UI name, +0x10 track, +0x14 per-key flag, +0x18 type, +0x1c bits, +0x20/24/28
default/min/max (f32 for the float ctor 0x82c2a260, ints for 0x82c2a2f0 and most inline ones), +0x3c cubic pos,
+0x40 tension pos. Registered by 0x82c29ec0(table, pos, obj) or inline `stw r3, 0x10 + 4 pos(r28)`."""
from x1 import *
import json, struct
START, END = 0x82c3e6d8, 0x82c436dc
FCTOR, ICTOR, REG = 0x82c2a260, 0x82c2a2f0, 0x82c29ec0
def emulate():
    r = {1: 0x7f000000}; f = {}; stack = {}; fstack = {}; cur = {}; table = {}; last = None
    for i in range((START - TEXT_VA) // 4, (END - TEXT_VA) // 4):
        w = words[i]; op = w >> 26; rd, ra, rb = (w >> 21) & 31, (w >> 16) & 31, (w >> 11) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        g = r.get
        if op == 14:
            r[rd] = simm & 0xffffffff if ra == 0 else ((g(ra) + simm) & 0xffffffff if g(ra) is not None else None)
        elif op == 15:
            r[rd] = (imm << 16) & 0xffffffff if ra == 0 else None
        elif op == 24:
            r[ra] = (g(rd) | imm) if g(rd) is not None else None
        elif op == 48:  # lfs
            if ra == 1: f[rd] = fstack.get(simm)
            else: f[rd] = f32_at((g(ra) + simm) & 0xffffffff) if g(ra) is not None else None
        elif op == 52:  # stfs
            if ra == 1: fstack[simm] = f.get(rd)
            elif ra == 3: cur[('f', simm)] = f.get(rd)
        elif op == 36:  # stw
            if ra == 1: stack[simm] = g(rd)
            elif ra == 3 and rd != 3: cur[simm] = g(rd)
            elif rd == 3 and ra == 28: table[(simm - 0x10) // 4] = last or cur; cur = {}; last = None
        elif op == 38:
            if ra == 3: cur[('b', simm)] = g(rd)
        elif op == 32:
            r[rd] = stack.get(simm) if ra == 1 else None
        elif op == 63 and ((w >> 1) & 0x3ff) == 72: f[rd] = f.get(rb)
        elif op == 31 and ((w >> 1) & 0x3ff) == 444: r[ra] = g(rd) if rd == rb else None
        elif op == 31: r[rd] = None
        elif op == 7: r[rd] = None
        elif w == 0x4e800421: cur = {}
        elif op == 18 and (w & 1):
            tgt = (TEXT_VA + 4 * i + ((w & 0x3fffffc) - (0x4000000 if w & 0x2000000 else 0))) & 0xffffffff
            s = lambda o: stack.get(o)
            if tgt == FCTOR:
                last = {8: g(4), 0xc: g(5), 0x10: g(6), 0x14: g(7), 0x18: g(8), 0x1c: g(9),
                        ('f', 0x20): f.get(1), ('f', 0x24): f.get(2), ('f', 0x28): f.get(3), 0x3c: s(0x64), 0x40: s(0x6c)}
            elif tgt == ICTOR:
                last = {8: g(4), 0xc: g(5), 0x10: g(6), 0x14: g(7), 0x18: g(8), 0x1c: g(9),
                        0x20: g(10), 0x24: s(0x54), 0x28: s(0x5c), 0x3c: s(0x64), 0x40: s(0x6c)}
            elif tgt == REG:
                table[g(4)] = last; last = None
            for x in (3, 4, 5, 6, 7, 8, 9, 10, 11, 12): r[x] = None
    return table
def dump():
    t = emulate()
    out = []
    for pos in sorted(t):
        o = t[pos]
        typ = o.get(0x18)
        get = lambda k: o.get(('f', k)) if ('f', k) in o and typ in (3, 4) else o.get(k)
        out.append(dict(pos=pos, name=s_at(o.get(8) or 0), track=o.get(0x10), flag=o.get(0x14), type=typ, bits=o.get(0x1c),
                        default=get(0x20), min=get(0x24), max=get(0x28), cubic=o.get(0x3c), tension=o.get(0x40)))
    return out
if __name__ == '__main__':
    out = dump()
    for x in out: print(x)
    json.dump(out, open('ice_params1.json', 'w'), indent=0)
    print(len(out), 'params; positions', min(x['pos'] for x in out), max(x['pos'] for x in out))
