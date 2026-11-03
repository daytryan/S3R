"""Minimal STFS (LIVE/PIRS/CON) extractor. Read-only ("female") packages, with one or two hash tables per level."""
import os, struct, sys

def extract(pkg, out):
    f = open(pkg, 'rb')
    hdr = f.read(0x1000)
    assert hdr[:4] in (b'LIVE', b'PIRS', b'CON '), 'not STFS'
    base = (struct.unpack('>I', hdr[0x340:0x344])[0] + 0xFFF) & 0xF000
    vd = hdr[0x379:0x379 + 0x24]
    # ponytail: female (1 hash table per level) only; male packages need active-table selection
    assert vd[2] & 1, 'male STFS package not supported'
    ft_count = struct.unpack('<H', vd[3:5])[0]
    ft_block = int.from_bytes(vd[5:8], 'little')

    # Tables per hash level: 1 for the LIVE/PIRS packages; the CON "Time is Money" pack stores 2 (a stale and an active
    # copy) with the same descriptor flag, so the layout is taken from where the file table actually parses.
    k = 1

    def backing(b):
        r = b + k * (b // 0xAA + 1)
        if b >= 0xAA:
            r += k * (b // 0x70E4 + 1)
            if b >= 0x70E4:
                r += k
        return r

    def addr(b):
        return (backing(b) << 12) + base

    def hash_entry(b):
        n = 0 if b < 0xAA else (b // 0xAA) * 0xAB + b // 0x70E4 + 1 + (1 if b >= 0x70E4 else 0)
        f.seek((n << 12) + base + (b % 0xAA) * 0x18)
        return f.read(0x18)

    def next_block(b):
        # ponytail: two-table packages would need the active table picked per level; none we read chains blocks
        if k != 1:
            raise NotImplementedError('block chain in a two-table STFS package')
        return int.from_bytes(hash_entry(b)[0x15:0x18], 'big')

    def read_blocks(start, count, consecutive):
        b = start
        for i in range(count):
            f.seek(addr(b))
            yield f.read(0x1000)
            if i + 1 < count:
                b = b + 1 if consecutive else next_block(b)

    def file_table():
        table = b''.join(read_blocks(ft_block, ft_count, False))
        entries = []
        for i in range(0, len(table), 0x40):
            e = table[i:i + 0x40]
            n = e[0x28] & 0x3F
            if n == 0:
                break
            if not all(0x20 <= c < 0x7F for c in e[:n]):
                return None
            entries.append(e)
        return entries or None  # a package always holds a file; an empty table is the wrong layout

    entries = file_table()
    if entries is None:
        k = 2
        entries = file_table()
    assert entries is not None, 'STFS file table not found'
    paths = {0xFFFF: out}
    for idx, e in enumerate(entries):
        name = e[:e[0x28] & 0x3F].decode('ascii')
        parent = struct.unpack('>H', e[0x32:0x34])[0]
        p = os.path.join(paths[parent], name)
        if e[0x28] & 0x80:
            paths[idx] = p
            os.makedirs(p, exist_ok=True)
            continue
        os.makedirs(os.path.dirname(p), exist_ok=True)
        blocks = int.from_bytes(e[0x29:0x2C], 'little')
        start = int.from_bytes(e[0x2F:0x32], 'little')
        size = struct.unpack('>I', e[0x34:0x38])[0]
        left = size
        with open(p, 'wb') as o:
            for chunk in read_blocks(start, blocks, bool(e[0x28] & 0x40)):
                o.write(chunk[:left])
                left -= min(left, 0x1000)
        assert os.path.getsize(p) == size, p
        print(f'{size:>12}  {os.path.relpath(p, out)}')

if __name__ == '__main__':
    extract(sys.argv[1], sys.argv[2])
