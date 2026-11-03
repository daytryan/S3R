"""Every native registration (name -> fn): lis/addi tracked; bl 0x829591b8 makes the fn object from r3, bl 0x82974f60
registers it under the string in r4 (also other register calls: record any bl whose r4 is a string)."""
from x1 import *
import json
regs = {}; fn = None; out = {}
MAKE, REG = 0x829591b8, 0x82974f60
for i, w in enumerate(words):
    va = TEXT_VA + 4 * i
    op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
    imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
    if w == 0x7d8802a6:  # mflr r12: new function
        regs = {}; fn = None
    if op == 15:
        regs[rd] = ((imm << 16) + (regs[ra] if ra else 0)) & 0xffffffff if (ra == 0 or regs.get(ra) is not None) else None
    elif op == 14:
        if ra == 0: regs[rd] = simm & 0xffffffff
        elif regs.get(ra) is not None: regs[rd] = (regs[ra] + simm) & 0xffffffff
        else: regs[rd] = None
    elif op == 18 and w & 1:
        t = (va + ((w & 0x3fffffc) - (0x4000000 if w & 0x2000000 else 0))) & 0xffffffff
        if t == MAKE: fn = regs.get(3)
        elif t == REG and regs.get(4) and fn:
            n = s_at(regs[4])
            if n: out.setdefault(n, []).append((hex(fn), hex(va)))
        for r in (3, 4, 5, 6, 7, 8, 9, 10, 11, 12): regs.pop(r, None)
    elif op == 31 and ((w >> 1) & 0x3ff) == 444 and rd == (w >> 11) & 31:  # mr
        regs[ra] = regs.get(rd)
json.dump(out, open('natreg.json', 'w'), indent=0)
print(len(out))
