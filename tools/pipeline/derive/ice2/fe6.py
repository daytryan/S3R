from xref import *
for i, w in enumerate(words):
    if w >> 26 == 14 and w & 0xffff == 0x7940:
        rx = (w >> 21) & 31
        regs = {rx}
        out = []
        for j in range(i + 1, min(i + 400, len(words))):
            v = words[j]; op = v >> 26; rd, ra = (v >> 21) & 31, (v >> 16) & 31; disp = v & 0xffff
            if op == 14 and ra in regs and disp == 0x130: regs.add(rd)
            if op == 31 and ((v >> 1) & 0x3ff) == 444 and ra in regs: regs.add(rd)  # mr
            if op in (48, 32, 34, 40) and ra in regs and 0xe0 <= disp <= 0x12f:
                out.append((hex(TEXT_VA + 4 * j), op, hex(disp)))
            if v == 0x4e800020: break
        print(hex(TEXT_VA + 4 * i), out)
