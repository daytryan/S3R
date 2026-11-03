from xref import *
# find "lbz rX, 0x2f0(rY)" followed within 4 by "addi rZ, rY, 0x130"
for i, w in enumerate(words):
    if w >> 26 == 34 and w & 0xffff == 0x2f0:
        ry = (w >> 16) & 31
        for j in range(i + 1, i + 6):
            v = words[j]
            if v >> 26 == 14 and v & 0xffff == 0x130:
                regs = {(v >> 21) & 31, ry}
                out = []
                for k in range(j + 1, min(j + 300, len(words))):
                    u = words[k]; op = u >> 26; ra = (u >> 16) & 31; disp = u & 0xffff
                    if op in (48, 32) and ra in regs and 0xf0 <= disp <= 0x114:
                        out.append((hex(TEXT_VA + 4 * k), hex(disp)))
                    if u == 0x4e800020: break
                print(hex(TEXT_VA + 4 * i), out)
                break
