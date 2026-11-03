"""stfs_extract on synthetic packages: the one-table LIVE layout and the two-table CON layout ("Time is Money")."""
import struct, sys, tempfile, unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent/'dlc'))
from stfs_extract import extract  # noqa: E402


def package(tables, name=b'unlockall_00000000.big', data=b'EB\x00\x03' + b'x' * 5000):
    """A female package: header (data at 0xB000), `tables` hash tables, file table block 0, then the file's blocks 1..n."""
    blocks = (len(data) + 0xFFF) // 0x1000
    head = bytearray(0xB000)
    head[:4] = b'LIVE' if tables == 1 else b'CON '
    head[0x340:0x344] = struct.pack('>I', 0xAD0E)
    head[0x379:0x37D] = bytes([0x24, 0, 1, 1])  # same descriptor flag in both layouts, as on the disc packages
    # stale/active hash tables: hash bytes, never a valid entry name
    hashes = b''.join(bytes([0xED] * 0x14) + b'\x80\xff\xff\xff' for _ in range(0xAA)).ljust(0x1000, b'\0')
    entry = bytearray(0x40)
    entry[:len(name)] = name
    entry[0x28] = 0x40 | len(name)  # consecutive blocks
    entry[0x29:0x2C] = blocks.to_bytes(3, 'little')
    entry[0x2F:0x32] = (1).to_bytes(3, 'little')
    entry[0x32:0x34] = b'\xff\xff'
    entry[0x34:0x38] = struct.pack('>I', len(data))
    body = hashes * tables + bytes(entry).ljust(0x1000, b'\0') + data.ljust(blocks * 0x1000, b'\0')
    return bytes(head) + body, name.decode(), data


class StfsLayoutTest(unittest.TestCase):
    def check(self, tables):
        raw, name, data = package(tables)
        with tempfile.TemporaryDirectory() as tmp:
            pkg = Path(tmp, 'pkg')
            pkg.write_bytes(raw)
            extract(str(pkg), str(Path(tmp, 'out')))
            self.assertEqual(Path(tmp, 'out', name).read_bytes(), data)

    def test_one_table_live_package(self):
        self.check(1)

    def test_two_table_con_package(self):
        self.check(2)


if __name__ == '__main__':
    unittest.main()
