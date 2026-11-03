# tools/pipeline/test_xdvdfs.py
"""XDVDFS reading on a synthetic image, plus the user's real Skate 3 ISO when present (read-only)."""
import os, struct, tempfile, unittest
from pathlib import Path
from tools.pipeline.xdvdfs import Iso, MAGIC, SECTOR

SOURCE = Path(os.environ.get('SKATE_SOURCES') or 'unset', '01 - Skate 3 (USA, Europe) - Xbox 360 ISO')
EXTRACTED = Path(__file__).resolve().parents[2]/'Skate 3 (USA, Europe) (En,Fr,De,Es,It,Nl)'


def entry(name, sector, size, attr, right=0):
    raw = struct.pack('<HHIIBB', 0, right, sector, size, attr, len(name)) + name.encode()
    return raw + b'\xff' * (-len(raw) % 4)


def image(truncate=0):
    first = entry('a.txt', 40, 5, 0x20, right=0)
    root = entry('a.txt', 40, 5, 0x20, right=len(first) // 4) + entry('sub', 41, 20, 0x10)
    sub = entry('b.bin', 42, 3, 0x20)
    data = bytearray(43 * SECTOR)
    data[32 * SECTOR:32 * SECTOR + 28] = MAGIC + struct.pack('<II', 39, len(root))
    data[39 * SECTOR:39 * SECTOR + len(root)] = root
    data[40 * SECTOR:40 * SECTOR + 5] = b'hello'
    data[41 * SECTOR:41 * SECTOR + len(sub)] = sub
    data[42 * SECTOR:42 * SECTOR + 3] = b'xyz'
    return bytes(data[:len(data) - truncate])


def disc(root, claimed=None, sector=39, cut=0):
    """Image whose root table (`root` bytes) sits at sector 39; the descriptor points at `sector` and claims `claimed` bytes."""
    data = bytearray(39 * SECTOR + len(root))
    data[32 * SECTOR:32 * SECTOR + 28] = MAGIC + struct.pack('<II', sector, len(root) if claimed is None else claimed)
    data[39 * SECTOR:] = root
    return bytes(data[:len(data) - cut])


class Xdvdfs(unittest.TestCase):
    def test_synthetic_list_read_extract(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t, 'game & co.iso'); p.write_bytes(image())
            with Iso(p) as iso:
                self.assertEqual(iso.files(), {'a.txt': (40, 5), 'sub/b.bin': (42, 3)})
                self.assertEqual(iso.read('a.txt'), b'hello')
                iso.extract(Path(t, 'out'))
            self.assertEqual(Path(t, 'out/sub/b.bin').read_bytes(), b'xyz')

    def test_truncated_and_not_an_image(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t, 'cut.iso'); p.write_bytes(image(truncate=SECTOR - 1))
            with Iso(p) as iso, self.assertRaisesRegex(ValueError, 'truncated'):
                iso.extract(Path(t, 'out'))
            q = Path(t, 'x.iso'); q.write_bytes(b'\0' * 100000)
            with self.assertRaisesRegex(ValueError, 'not an Xbox 360 disc image'):
                Iso(q)

    def test_unsafe_names_rejected(self):
        for name in ('..', '.', 'a/b', 'a:b', 'a' + chr(92) + 'b'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as t:
                p = Path(t, 'bad.iso'); p.write_bytes(disc(entry(name, 40, 5, 0x20)))
                with Iso(p) as iso, self.assertRaisesRegex(ValueError, 'unsafe'):
                    iso.files()

    def test_truncated_directory_table_rejected(self):
        first = entry('a.txt', 40, 5, 0x20)
        root = entry('a.txt', 40, 5, 0x20, right=len(first) // 4) + entry('sub', 41, 20, 0x10)
        cases = {'cut mid-table': disc(root, cut=10),
                 'root sector past EOF': disc(root, sector=1000),
                 'absurd table size': disc(root, claimed=0xFFFFFFFF)}
        for what, data in cases.items():
            with self.subTest(what), tempfile.TemporaryDirectory() as t:
                p = Path(t, 'cut.iso'); p.write_bytes(data)
                with Iso(p) as iso, self.assertRaisesRegex(ValueError, 'truncated in a directory'):
                    iso.files()

    def test_read_of_cut_off_payload_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t, 'cut.iso'); p.write_bytes(image(truncate=SECTOR - 1))
            with Iso(p) as iso, self.assertRaisesRegex(ValueError, 'truncated'):
                iso.read('sub/b.bin')

    def test_file_ending_inside_root_descriptor(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t, 'short.iso'); p.write_bytes(bytes(32 * SECTOR) + MAGIC + bytes(4))
            with self.assertRaisesRegex(ValueError, 'not an Xbox 360 disc image'):
                Iso(p)

    @unittest.skipUnless(SOURCE.is_dir() and EXTRACTED.is_dir(), 'needs the user disc files')
    def test_real_skate3_iso_matches_extracted_copy(self):
        isos = sorted(SOURCE.glob('*.iso'))
        if not isos:
            self.skipTest('no .iso in the Skate 3 source folder')
        with Iso(isos[0]) as iso:
            files = iso.files()
            self.assertEqual(iso.read('default.xex'), (EXTRACTED/'default.xex').read_bytes())
            for path in EXTRACTED.rglob('*'):
                if path.is_file():
                    rel = path.relative_to(EXTRACTED).as_posix()
                    key = next(k for k in files if k.lower() == rel.lower())
                    self.assertEqual(files[key][1], path.stat().st_size, rel)


if __name__ == '__main__':
    unittest.main()
