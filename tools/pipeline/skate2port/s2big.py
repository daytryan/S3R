"""Reader for the classic EA BIGF/BIG4 archives used by Skate 2 (Skate 3 uses EB BIG v3)."""
import struct
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Entry:
    path: str
    offset: int
    size: int


class Big:
    def __init__(self, path):
        self.path = Path(path)
        with self.path.open('rb') as f:
            head = f.read(16)
            assert head[:4] in (b'BIGF', b'BIG4'), (path, head[:4])
            count, header_size = struct.unpack('>II', head[8:16])
            data = head + f.read(header_size - 16)
        self.entries, cursor = [], 16
        for _ in range(count):
            offset, size = struct.unpack_from('>II', data, cursor)
            end = data.index(b'\0', cursor + 8)
            self.entries.append(Entry(data[cursor + 8:end].decode('latin-1').replace('\\', '/'), offset, size))
            cursor = end + 1

    def read(self, entry):
        with self.path.open('rb') as f:
            f.seek(entry.offset)
            data = f.read(entry.size)
        # ponytail: refpack only; add other codecs if an entry turns up with a different magic
        if data[:2] in (b'\x10\xfb', b'\x90\xfb'):
            from tools.owned_game.refpack import decompress
            return decompress(data)  # reads its own size header
        return data


if __name__ == '__main__':
    import sys
    for e in Big(sys.argv[1]).entries:
        print(e.size, e.path)
