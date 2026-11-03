"""Skate 1 default.xex Lua enum tables: the {name, register fn, 0, 3} table at 0x8217d5xx; each register fn pushes
(name string, float value) pairs. Prints {enum: {member: value}} as JSON."""
import json, struct, sys
from x1 import *  # the sibling helpers, reading SKATE_XEX_IMAGE

def word(va):
    return struct.unpack_from('>I', d, va2off(va))[0]

def members(fn, limit=4000):
    regs, out, name = {}, [], None
    va = fn
    for _ in range(limit):
        w = word(va)
        op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if w == 0x4e800020 or (w >> 26 == 18 and not w & 1):  # blr / tail b (restgprlr)
            break
        if op == 15:  # lis / addis
            regs[rd] = ((imm << 16) + (regs.get(ra, 0) if ra else 0)) & 0xffffffff
        elif op == 14:  # addi / li
            if ra == 0:
                regs[rd] = simm & 0xffffffff
            else:
                regs[rd] = (regs.get(ra, 0) + simm) & 0xffffffff
            if rd == 4 and ra != 0:
                name = s_at(regs[4])
        elif op in (32, 33, 34, 35, 40, 41, 42, 43) or (op == 31 and ((w >> 1) & 0x3ff) in (444, 23, 266, 40)):
            if op == 31 and ((w >> 1) & 0x3ff) == 444:
                regs.pop(ra, None)
            else:
                regs.pop(rd, None)
        elif op == 48 and rd == 0:  # lfs f0, off(ra)
            if name is not None and ra in regs:
                v = f32_at((regs[ra] + simm) & 0xffffffff)
                out.append((name, v))
                name = None
        va += 4
    return out

table = 0x8217d5cc - 0x30
# walk back to the table start
while s_at(word(table - 16)) and word(table - 4) in (3,):
    table -= 16
res = {}
va = table
while True:
    n = s_at(word(va))
    if not n or word(va + 12) != 3:
        break
    res[n] = dict(members(word(va + 4)))
    va += 16
print(json.dumps(res, indent=1))
