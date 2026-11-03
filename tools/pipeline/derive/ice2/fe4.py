from xref import *
for i, w in enumerate(words):
    op = w >> 26
    disp = w & 0xffff
    if op in (36, 52, 14, 48, 32) and 0x4f0 <= disp <= 0x4fc and 0x82690000 <= TEXT_VA + 4*i < 0x826b0000:
        print(hex(TEXT_VA + 4 * i), op, hex(disp), (w >> 16) & 31)
