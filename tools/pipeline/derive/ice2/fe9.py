from xref import *
for i, w in enumerate(words):
    op = w >> 26
    if op in (34, 38) and w & 0xffff == 0x7c30:
        print(hex(TEXT_VA + 4 * i), 'stb' if op == 38 else 'lbz', (w >> 16) & 31)
