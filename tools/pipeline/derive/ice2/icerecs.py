import struct
def scene_names():
    d = open('scene_000.bin', 'rb').read()
    n = struct.unpack_from('>I', d, 8)[0]
    out = {}
    for i in range(n):
        o = 0x10 + i * 0x68
        out[struct.unpack_from('>I', d, o)[0]] = (d[o + 0x28:o + 0x68].split(b'\0')[0].decode('latin-1'), d[o + 4])
    return out
def records(path='ice_000.bin'):
    e = open(path, 'rb').read()
    tag, size = struct.unpack_from('<II', e, 0)
    o = 8
    while o < 8 + size:
        t, s = struct.unpack_from('<II', e, o)
        c = e[o + 8:o + 8 + s]
        r = 0
        while r + 8 <= len(c):
            one, rs, h = struct.unpack_from('<3I', c, r)
            yield t, h, c[r:r + 8 + rs]
            r += 8 + rs
        o += 8 + s
if __name__ == '__main__':
    names = scene_names()
    for t, h, rec in records():
        dur = struct.unpack_from('<f', rec, 0x1c)[0]
        print(hex(t), hex(h), len(rec), round(dur, 3), names.get(h, '?'))
