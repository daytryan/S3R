"""Skate 2's EA BIG archive tables for the skate2-physics stage: read a table, write a cut-down archive.

Not named s2*.py on purpose: OLD_GAMES globs s2*.py, and every stage that hashes that glob would change its fingerprint.

The tables are the ones the Skate 2 Rust port reads (skate2-formats big.rs, `BigArchive::parse`):
- named: `BIGF` / `BIG4` (or `BIG1`..`BIG9`, the digit being the integer width). 16 bytes: magic, LE u32 archive size,
  BE u32 count, BE u32 header size. Then per entry: offset (width bytes, BE), stored size (min(width, 4) bytes, BE), the
  name and a NUL. Lookup ignores ASCII case.
- hashed: `Viv4`, in a `.bh` beside the `.big`: the same 16 bytes (the LE u32 is the table's size), then per entry 16 bytes,
  all BE u32: offset, stored size, unpacked size (0: the entry is not packed), djb2 hash of the entry's path.
  Lookup is the case-sensitive djb2 of the path. The offsets are in the `.big`.
An entry's bytes are stored as they are on the disc (RefPack included) and are copied verbatim.
"""
import struct
from dataclasses import dataclass
from pathlib import Path

ALIGN = 64  # the disc starts every entry on a 64-byte boundary


def djb2(name):
    """rw::core::Big::Locate's hash (0x82bb8680) of a path as bytes."""
    h = 5381
    for b in name:
        h = (h * 33 + b) & 0xFFFFFFFF
    return h


@dataclass(frozen=True)
class Row:
    key: object    # the name (str, latin-1) of a named table, the hash (int) of a hashed one
    offset: int
    stored: int
    unpacked: int  # the unpacked size; a hashed table's 0 reads as the stored size, as the port reads it


@dataclass(frozen=True)
class Table:
    magic: bytes   # b'BIGF', b'BIG4' ... or b'Viv4'
    width: int     # integer width of a named table (0 for a hashed one)
    rows: tuple    # of Row, in the order the file lists them

    @property
    def hashed(self):
        return self.magic == b'Viv4'


def read_table(header):
    """The table at the start of `header` (the whole file is fine). Raises ValueError for anything the port refuses."""
    if len(header) < 16:
        raise ValueError('truncated header')
    magic = bytes(header[:4])
    count, size = struct.unpack('>II', header[8:16])
    if size < 16 or size > len(header):
        raise ValueError(f'header size {size} is outside the {len(header)} bytes given')
    rows = []
    if magic == b'Viv4':
        if 16 + count * 16 > size:
            raise ValueError('entry table overflows the header')
        for i in range(count):
            offset, stored, unpacked, h = struct.unpack_from('>IIII', header, 16 + 16 * i)
            rows.append(Row(h, offset, stored, unpacked or stored))
        return Table(magic, 0, tuple(rows))
    if magic[:3] != b'BIG':
        raise ValueError(f'unknown magic {magic!r}')
    variant = magic[3:4]
    if variant in (b'F', b'4'):
        width = 4
    elif variant in (b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9'):
        width = int(variant)
    else:
        raise ValueError(f'unsupported width {variant!r}')
    at = 16
    for _ in range(count):
        if at + width + min(width, 4) >= size:
            raise ValueError('entry table overflows the header')
        offset = int.from_bytes(header[at:at + width], 'big')
        stored = int.from_bytes(header[at + width:at + width + min(width, 4)], 'big')
        at += width + min(width, 4)
        end = header.find(b'\0', at, size)
        if end < 0:
            raise ValueError('a name has no end')
        rows.append(Row(bytes(header[at:end]).decode('latin-1'), offset, stored, stored))
        at = end + 1
    return Table(magic, width, tuple(rows))


def _aligned(n):
    return (n + ALIGN - 1) // ALIGN * ALIGN


def _hashed_table(rows):
    rows = sorted(rows, key=lambda r: r.key)  # the disc lists them by hash
    size = 16 + 16 * len(rows)
    out = bytearray(b'Viv4' + struct.pack('<I', size) + struct.pack('>II', len(rows), size))
    for r in rows:
        out += struct.pack('>IIII', r.offset, r.stored, r.unpacked, r.key)
    return bytes(out)


def write(path, table, entries, bh=None):
    """Writes `entries`, a list of (key, stored bytes), as the archive `path` (the .big), bytes verbatim.

    `table` is the template: its magic and width, and for a hashed template the unpacked size of each key. A named
    template gives a .big that holds its own named table. A hashed template gives the .bh beside `path` and a .big
    that holds only the bytes (the port reads such an archive through its .bh). With `bh` (a hashed template) a named
    archive also gets the .bh: a row for each entry, keyed djb2(name), with the unpacked size `bh` holds for that hash
    (an entry that `bh` does not list is left out of the .bh, as the disc leaves it out).
    Offsets are fresh, and the file lists the entries in the order given.
    """
    path = Path(path)
    unpacked = {r.key: r.unpacked for r in table.rows}
    if table.hashed:
        if bh is not None:
            raise ValueError('a hashed archive has no second table')
        header_size = 256  # no table of its own: the bytes start after a blank header, as the disc's first entry does
    else:
        width = table.width
        header_size = 16 + sum(width + min(width, 4) + len(k.encode('latin-1')) + 1 for k, _ in entries)
    placed, at = [], _aligned(header_size)
    for key, payload in entries:
        placed.append((key, at, len(payload)))
        at = _aligned(at + len(payload))
    end = placed[-1][1] + placed[-1][2] if placed else header_size
    limit = 1 << (8 * (table.width or 4))
    if end >= limit:
        raise ValueError(f'{path.name} is {end} bytes: past the {table.width}-byte offsets of its table')
    head = bytearray(header_size)
    if not table.hashed:
        head[:16] = table.magic + struct.pack('<I', end) + struct.pack('>II', len(entries), header_size)
        cursor = 16
        for key, offset, stored in placed:
            head[cursor:cursor + width] = offset.to_bytes(width, 'big'); cursor += width
            head[cursor:cursor + min(width, 4)] = stored.to_bytes(min(width, 4), 'big'); cursor += min(width, 4)
            name = key.encode('latin-1')
            head[cursor:cursor + len(name)] = name; cursor += len(name) + 1  # the NUL is already there
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, 'wb') as f:
        f.write(head)
        for (key, offset, stored), (_, payload) in zip(placed, entries):
            f.write(bytes(offset - f.tell()))
            f.write(payload)
    rows = None
    if table.hashed:
        missing = [k for k, _ in entries if k not in unpacked]
        if missing:
            raise ValueError(f'the template holds no unpacked size for {missing[0]:#010x}')
        rows = [Row(key, offset, stored, unpacked[key]) for key, offset, stored in placed]
    elif bh is not None:
        by_hash = {r.key: r.unpacked for r in bh.rows}
        rows = []
        for key, offset, stored in placed:
            h = djb2(key.encode('latin-1'))
            if h in by_hash:
                rows.append(Row(h, offset, stored, by_hash[h]))
    if rows is not None:
        path.with_suffix('.bh').write_bytes(_hashed_table(rows))
