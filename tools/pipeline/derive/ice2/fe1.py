from xref import *
import struct
f = lambda va: struct.unpack('>f', d[va2off(va):va2off(va)+4])[0]
print('0x822d0084', f(0x822d0084), '0x82000a10', f(0x82000a10))
# any load/store with displacement in the fisheye blocks
for i, w in enumerate(words):
    op = w >> 26
    if op in (32, 36, 48, 52, 14, 50, 54):
        disp = w & 0xffff
        if 0x7a30 <= disp <= 0x7a58 or 0x7b60 <= disp <= 0x7b88:
            print(hex(TEXT_VA + 4 * i), op, hex(disp), (w >> 16) & 31)
