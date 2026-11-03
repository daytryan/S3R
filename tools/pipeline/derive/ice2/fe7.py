from xref import *
for i, w in enumerate(words):
    op = w >> 26
    disp = w & 0xffff
    if op in (36, 52, 14) and 0x4f0 <= disp <= 0x4fc and 0x82680000 <= TEXT_VA + 4*i < 0x826c0000:
        print(hex(TEXT_VA + 4 * i), op, hex(disp), (w >> 16) & 31)
