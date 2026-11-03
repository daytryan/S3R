from x1 import *
# find stw rS, off(rB) where rB = 0x830fcb18 (tracked lis/addi), record off and the function start
regs = {}; start = None; out = []
for i, w in enumerate(words):
    va = TEXT_VA + 4 * i
    op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
    imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
    if w == 0x7d8802a6: regs = {}; start = va
    if op == 15 and ra == 0: regs[rd] = (imm << 16) & 0xffffffff
    elif op == 14 and ra and ra in regs: regs[rd] = (regs[ra] + simm) & 0xffffffff
    elif op == 36 and ra in regs and regs[ra] == 0x830fcb18:
        out.append((hex(va), hex(start or 0), simm // 4))
    elif op == 18 and w & 1:
        for r in range(3, 13): regs.pop(r, None)
for o in out: print(o)
