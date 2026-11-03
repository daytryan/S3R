from xref import *
# functions that load the render globals pointer (lwz rX, 0x2478(lis 0x82df))
hits = [a for a, t in xrefs([0x82df2478])]
print(len(hits))
for a in hits:
    i = (a - TEXT_VA) // 4
    rg = (words[i] >> 21) & 31
    regs = {rg}
    uses = []
    for j in range(i + 1, min(i + 80, len(words))):
        w = words[j]; op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
        disp = w & 0xffff
        if ra in regs and op in (14, 15, 32, 48, 52, 36):
            uses.append((op, hex(disp)))
        if w == 0x4e800020: break
    sel = [u for u in uses if u[0] in (14, 15) or 0x7a00 <= int(u[1], 16) <= 0x7c00]
    if sel: print(hex(a), sel[:12])
