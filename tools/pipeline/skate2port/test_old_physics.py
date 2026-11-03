"""The skate2-physics stage (old_physics.py, physics_big.py): the entry list is well formed, a cut-down archive reads back
byte for byte, and a set with an archive missing is refused by name. No disc needed: the archives are made here."""
import json, os, sys, tempfile, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here)]
import old_physics  # noqa: E402
import physics_big  # noqa: E402

# The nine archives the sim reads: the spike's minimal set (<MIN>).
NINE = {'data\\big\\anim.big', 'data\\big\\miscboot.big', 'data\\big\\miscload.big', 'data\\big\\ropa2.big',
        'data\\content\\livingworld.big', 'data\\content\\marquee.big', 'data\\content\\worldbam.big',
        'data\\content\\worldmisc.big', 'data\\db\\db.big'}


class EntryList(unittest.TestCase):
    def test_the_entry_list_is_well_formed(self):
        entries = old_physics.read_entries(here/'old_physics_entries.tsv')
        cells = [e for e in entries if old_physics.CELL.search(e.name)]
        self.assertEqual(len(cells), 284)
        self.assertEqual(len({(e.archive, e.key) for e in entries}), len(entries), 'a duplicate (archive, key)')
        self.assertLessEqual({e.archive for e in entries}, NINE)
        for e in entries:
            self.assertTrue(e.stored >= 0 and e.name and e.key, e)
        # four archives are copied whole (fix round 1): every entry of their tables is listed
        self.assertEqual(set(old_physics.WHOLE), {'data\\content\\livingworld.big', 'data\\big\\miscload.big',
                                                  'data\\big\\ropa2.big', 'data\\db\\db.big'})
        for archive, count in WHOLE_ENTRIES.items():
            self.assertEqual(sum(1 for e in entries if e.archive == archive), count, archive)

    def test_the_whole_archives_are_the_discs_tables(self):
        # needs the disc image: S2PHYS_TEST_ISO is the path of the Skate 2 .iso (skipped without it)
        iso = os.environ.get('S2PHYS_TEST_ISO')
        if not iso:
            self.skipTest('set S2PHYS_TEST_ISO to the Skate 2 .iso')
        disc = old_physics.IsoDisc(here.parents[2], iso)
        try:
            entries = old_physics.read_entries(here/'old_physics_entries.tsv')
            for archive in old_physics.WHOLE:
                want = old_physics.whole_entries(disc, archive)
                # the list is sorted by key, the table is in the disc's order
                self.assertEqual(sorted(e for e in entries if e.archive == archive), sorted(want), archive)
        finally:
            disc.close()


# The entries of the four whole archives' tables on the disc (livingworld, miscload, ropa2, db).
WHOLE_ENTRIES = {'data\\content\\livingworld.big': 580, 'data\\big\\miscload.big': 926, 'data\\big\\ropa2.big': 126,
                 'data\\db\\db.big': 6}


class Archive(unittest.TestCase):
    def test_a_written_archive_reads_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            # a named table (BIG4): two entries, the second shaped like a RefPack stream
            named = [('data\\a\\x.bin', b'hello world'), ('data\\a\\Y.BIN', b'\x10\xfb\x00\x00\x20' + bytes(range(150)))]
            path = Path(tmp)/'named.big'
            physics_big.write(path, physics_big.Table(b'BIG4', 4, ()), named)
            data = path.read_bytes()
            table = physics_big.read_table(data)
            self.assertEqual((table.magic, table.width), (b'BIG4', 4))
            self.assertEqual([r.key for r in table.rows], [k for k, _ in named])
            for row, (key, payload) in zip(table.rows, named):
                self.assertEqual(data[row.offset:row.offset + row.stored], payload, key)
            self.assertFalse(path.with_suffix('.bh').exists(), 'a named archive with no twin writes no .bh')

            # a hashed table (Viv4): the .bh holds the table, the .big the bytes; data\config\input.cfg is the port's
            # own reference hash (djb2_matches_reference_miscboot_entry)
            self.assertEqual(physics_big.djb2(b'data\\config\\input.cfg'), 0x5efcbd5b)
            keys = [physics_big.djb2(b'data\\config\\input.cfg'), physics_big.djb2(b'data\\config\\other.cfg')]
            template = physics_big.Table(b'Viv4', 0, tuple(physics_big.Row(k, 0, 0, u) for k, u in zip(keys, (2000, 32))))
            hashed = [(keys[0], b'\x10\xfb\x00\x07\xd0' + b'\x07' * 40), (keys[1], bytes(range(32)))]
            path = Path(tmp)/'hashed.big'
            physics_big.write(path, template, hashed)
            table = physics_big.read_table(path.with_suffix('.bh').read_bytes())
            self.assertEqual(table.magic, b'Viv4')
            self.assertEqual({r.key for r in table.rows}, set(keys))
            big = path.read_bytes()
            for key, payload in hashed:
                row = next(r for r in table.rows if r.key == key)
                self.assertEqual(big[row.offset:row.offset + row.stored], payload)
                self.assertEqual(row.unpacked, 2000 if key == keys[0] else 32)

            # a named archive with a hashed twin: the .bh follows the names (djb2) and the template's unpacked sizes
            twin = physics_big.Table(b'Viv4', 0, tuple(physics_big.Row(physics_big.djb2(k.encode()), 0, 0, 999) for k, _ in named))
            path = Path(tmp)/'both.big'
            physics_big.write(path, physics_big.Table(b'BIGF', 4, ()), named, bh=twin)
            names = physics_big.read_table(path.read_bytes())
            hashes = physics_big.read_table(path.with_suffix('.bh').read_bytes())
            self.assertEqual(names.magic, b'BIGF')
            self.assertEqual({(r.offset, r.stored) for r in names.rows}, {(r.offset, r.stored) for r in hashes.rows})
            self.assertEqual({r.key for r in hashes.rows}, {physics_big.djb2(k.encode()) for k, _ in named})
            self.assertEqual({r.unpacked for r in hashes.rows}, {999})

    def test_a_table_that_is_not_one_is_refused(self):
        for bad in (b'', b'BIG4' + bytes(8), b'Q' * 16, b'BIG4' + bytes(4) + (1).to_bytes(4, 'big') + (99).to_bytes(4, 'big')):
            with self.assertRaises(ValueError, msg=bad):
                physics_big.read_table(bad)


def make_disc(root):
    """A tiny disc: a.big (named, with a hashed .bh twin) and b.big (named), and a default.xex."""
    root = Path(root)
    (root/'default.xex').write_bytes(b'XEX2 test')
    a = [('data\\a\\one.bin', b'one' * 40), ('data\\a\\two.bin', b'\x10\xfb\x00\x00\x30' + b'2' * 20), ('data\\a\\three.bin', b'3' * 7)]
    b = [('data\\b\\four.bin', b'4' * 100), ('data\\b\\five.bin', b'5' * 9)]
    twin = physics_big.Table(b'Viv4', 0, tuple(physics_big.Row(physics_big.djb2(k.encode()), 0, 0, 48 if k.endswith('two.bin') else len(v))
                                               for k, v in a))
    (root/'data/big').mkdir(parents=True)
    (root/'data/content').mkdir(parents=True)
    physics_big.write(root/'data/big/a.big', physics_big.Table(b'BIG4', 4, ()), a, bh=twin)
    physics_big.write(root/'data/content/b.big', physics_big.Table(b'BIGF', 4, ()), b)
    return [old_physics.Entry('data\\big\\a.big', f'0x{physics_big.djb2(k.encode()):08x}', k, len(v)) for k, v in a[:2]] + \
           [old_physics.Entry('data\\content\\b.big', k, k, len(v)) for k, v in b]


class Set(unittest.TestCase):
    def test_a_partial_set_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            disc, out = Path(tmp)/'disc', Path(tmp)/'out'
            disc.mkdir()
            entries = make_disc(disc)
            count, size = old_physics.build(old_physics.FolderDisc(disc), entries, out)
            self.assertEqual((count, size), (4, sum(e.stored for e in entries)))
            manifest = json.loads((out/'manifest.json').read_text(encoding='utf-8'))
            self.assertEqual(manifest['version'], 1)
            self.assertEqual(len(manifest['disc']), 64)
            self.assertEqual([e[:2] for e in manifest['entries']], [[e.archive, e.key] for e in entries])
            self.assertTrue((out/'draw').is_dir() and not any((out/'draw').iterdir()))
            self.assertTrue((out/'data/big/a.bh').is_file(), 'the .bh of a.big is kept')
            self.assertEqual(old_physics.verify(out), 4)
            (out/'data/content/b.big').unlink()
            with self.assertRaises(ValueError) as why:
                old_physics.verify(out)
            self.assertEqual(str(why.exception), 'missing data\\content\\b.big data\\b\\four.bin')

    def test_a_whole_archive_set_equals_the_archive_table(self):
        with tempfile.TemporaryDirectory() as tmp:
            disc, out = Path(tmp)/'disc', Path(tmp)/'out'
            disc.mkdir()
            make_disc(disc)
            source = old_physics.FolderDisc(disc)
            # a.big is read through its hashed .bh, b.big by name
            entries = old_physics.whole_entries(source, 'data\\big\\a.big', True) + \
                old_physics.whole_entries(source, 'data\\content\\b.big', False)
            self.assertEqual(len(entries), 5)
            for archive, hashed in (('data/big/a.big', True), ('data/content/b.big', False)):
                table = old_physics.table_of(source, archive)
                listed = [e for e in entries if e.archive == archive.replace('/', '\\')]
                self.assertEqual([e.name for e in listed], [r.key for r in table.rows])
                self.assertEqual([e.stored for e in listed], [r.stored for r in table.rows])
                keys = [f'0x{physics_big.djb2(r.key.encode()):08x}' if hashed else r.key for r in table.rows]
                self.assertEqual([e.key for e in listed], keys)
            self.assertEqual(old_physics.build(source, entries, out), (5, sum(e.stored for e in entries)))
            self.assertEqual(old_physics.verify(out), 5)
            for archive in ('data/big/a.big', 'data/content/b.big'):
                before, after = old_physics.table_of(source, archive), old_physics.table_of(old_physics.FolderDisc(out), archive)
                self.assertEqual([(r.key, r.stored) for r in after.rows], [(r.key, r.stored) for r in before.rows])
                for b, a in zip(before.rows, after.rows):
                    self.assertEqual(source.read(archive, b.offset, b.stored), (out/archive).read_bytes()[a.offset:a.offset + a.stored])
            # the hashed twin keeps every row, with the disc's unpacked sizes
            twin = physics_big.read_table((out/'data/big/a.bh').read_bytes())
            self.assertEqual({(r.key, r.unpacked) for r in twin.rows},
                             {(r.key, r.unpacked) for r in physics_big.read_table((disc/'data/big/a.bh').read_bytes()).rows})

    def test_a_whole_archive_with_a_name_twice_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            disc = Path(tmp)/'disc'
            disc.mkdir()
            physics_big.write(disc/'data/big/twice.big', physics_big.Table(b'BIG4', 4, ()), [('data\\x.bin', b'1'), ('data\\x.bin', b'2')])
            with self.assertRaises(ValueError):
                old_physics.whole_entries(old_physics.FolderDisc(disc), 'data\\big\\twice.big', False)

    def test_a_disc_that_differs_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            disc = Path(tmp)/'disc'
            disc.mkdir()
            entries = make_disc(disc)
            entries[0] = entries[0]._replace(stored=entries[0].stored + 1)
            with self.assertRaises(ValueError):
                old_physics.build(old_physics.FolderDisc(disc), entries, Path(tmp)/'out')


if __name__ == '__main__':
    unittest.main()
