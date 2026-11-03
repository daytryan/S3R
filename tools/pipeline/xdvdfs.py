# tools/pipeline/xdvdfs.py
"""Xbox 360 disc images (XDVDFS): list and copy files out. Replaces extract-xiso."""
import struct
from pathlib import Path

SECTOR = 2048
MAGIC = b'MICROSOFT*XBOX*MEDIA'
PARTITIONS = (0, 0x2080000, 0xFD90000, 0x18300000)  # plain image, XGD3, XGD2, XGD1 game partition


class Iso:
    def __init__(self, path):
        self.path = Path(path)
        self.file = open(self.path, 'rb')
        self._files = None
        self.end = self.file.seek(0, 2)
        for base in PARTITIONS:
            self.file.seek(base + 32 * SECTOR)
            head = self.file.read(28)
            if len(head) == 28 and head[:20] == MAGIC:
                self.base, self.root = base, struct.unpack_from('<II', head, 20)
                return
        self.file.close()
        raise ValueError(f'{self.path.name} is not an Xbox 360 disc image')

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.file.close()

    def files(self):
        if self._files is None:
            out, dirs, seen = {}, [('', *self.root)], set()
            while dirs:
                prefix, sector, size = dirs.pop()
                if not size or sector in seen:
                    continue
                seen.add(sector)
                if self.base + sector * SECTOR + size > self.end:  # before reading: a corrupt size must not allocate GiBs
                    raise ValueError(f'{self.path.name} is truncated in a directory')
                self.file.seek(self.base + sector * SECTOR)
                table, todo, visited = self.file.read(size), [0], set()
                if len(table) != size:
                    raise ValueError(f'{self.path.name} is truncated in a directory')
                while todo:
                    at = todo.pop()
                    if at in visited or at + 14 > len(table) or table[at:at + 2] == b'\xff\xff':
                        continue
                    visited.add(at)
                    left, right, start, length, attr, n = struct.unpack_from('<HHIIBB', table, at)
                    name = table[at + 14:at + 14 + n].decode('latin-1')
                    if not name or name in ('.', '..') or any(c in name for c in '/\\:'):
                        raise ValueError(f'{self.path.name} has an unsafe file name')
                    todo += [o * 4 for o in (left, right) if o]
                    if attr & 0x10:
                        dirs.append((prefix + name + '/', start, length))
                    else:
                        out[prefix + name] = (start, length)
            self._files = out
        return self._files

    def read(self, name):
        sector, size = self.files()[name]
        self.file.seek(self.base + sector * SECTOR)
        data = self.file.read(size)
        if len(data) != size:
            raise ValueError(f'{self.path.name} is truncated at {name}')
        return data

    def chunks(self, name):
        """Yield file in 8 MB blocks, raising ValueError if truncated. Do not read from the same Iso while iterating."""
        sector, size = self.files()[name]
        self.file.seek(self.base + sector * SECTOR)
        left = size
        while left:
            block = self.file.read(min(left, 8 << 20))
            if not block:
                raise ValueError(f'{self.path.name} is truncated at {name}')
            yield block
            left -= len(block)

    def extract(self, dest, names=None):
        for name in (self.files() if names is None else names):
            self.files()[name]  # Raises KeyError if name not found
            out = Path(dest)/name
            out.parent.mkdir(parents=True, exist_ok=True)
            with open(out, 'wb') as w:
                for block in self.chunks(name):
                    w.write(block)
