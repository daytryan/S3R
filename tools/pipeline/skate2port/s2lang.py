"""Skate 2 English string table (label -> text) from miscboot.big.

Header: u32 count @8, u32 header size (28) @12, u32 pool offset @16. After the header,
`count` (key, offset) u32 LE pairs; the offset points into the NUL-separated pool that
follows the 8-byte pool descriptor. Labels and values share keys, not pool positions
(the pools differ by one string, which is why the Skate 3 positional pairing fails)."""
import struct
from s2big import Big


def table(data):
    count, _, pool = struct.unpack_from('<III', data, 8)
    base = pool + 8
    rows = {}
    for i in range(count):
        key, offset = struct.unpack_from('<II', data, 28 + i*8)
        end = data.index(b'\0', base + offset)
        rows[key] = data[base + offset:end].decode('latin-1')
    return rows


def strings(game):
    big = Big(f'{game}/data/big/miscboot.big')
    read = {e.path.lower(): e for e in big.entries}
    labels = table(big.read(read['data/fe/languages/labels/language_labels_global_skate2ng.bin']))
    values = table(big.read(read['data/fe/languages/english/language_english_global_skate2ng.bin']))
    return {labels[k]: values[k] for k in labels if k in values}


if __name__ == '__main__':
    t = strings('../Skate 2 (Europe)')
    parts = {k: v for k, v in t.items() if k.startswith('ID_CAC_PART_')}
    print(len(t), len(parts), list(parts.items())[:12])
    assert t.get('ID_COMMON_ACCEPT'), 'pairing broken'
