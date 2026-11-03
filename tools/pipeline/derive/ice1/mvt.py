from x1 import *
import json
regs = {}; vt = {}; res = {}
start = (0x8233bfa0 - TEXT_VA) // 4; end = (0x8233cf00 - TEXT_VA) // 4
for i in range(start, end):
    w = words[i]; va = TEXT_VA + 4 * i
    op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
    imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
    if op == 15 and ra == 0: regs[rd] = (imm << 16) & 0xffffffff
    elif op == 14 and ra and ra in regs: regs[rd] = (regs[ra] + simm) & 0xffffffff
    elif op == 36 and simm == 0 and rd in regs and 0x82000000 <= regs[rd] < 0x83200000:
        vt[ra] = regs[rd]
    elif op == 36 and ra == 20 and rd in vt:
        res[simm // 4] = hex(vt[rd])
print(res)
for t, v in sorted(res.items()):
    v = int(v, 16)
    o = va2off(v)
    print(t, hex(v), [hex(x) for x in struct.unpack_from('>16I', d, o)])
