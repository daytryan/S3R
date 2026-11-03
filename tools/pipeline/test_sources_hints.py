"""#18 (public release review): a missing item says why when the scan saw something close (no discs needed)."""
import hashlib, os, tempfile, types, unittest
from pathlib import Path
from unittest import mock
from tools.pipeline import sources


def package(path, title, content, digest=b'\x07' * 20):
    """An STFS header the scan reads: magic, header digest at 0x32C, content type at 0x344, title ID at 0x360."""
    h = bytearray(0x364)
    h[:4] = b'LIVE'
    h[0x32C:0x340] = digest
    h[0x344:0x348] = content.to_bytes(4, 'big')
    h[0x360:0x364] = bytes.fromhex(title)
    path.write_bytes(bytes(h))


class Hints(unittest.TestCase):
    def scan(self, make):
        with tempfile.TemporaryDirectory() as t:
            make(Path(t))
            return sources.scan([Path(t)])

    def test_an_unreadable_iso_explains_the_missing_discs(self):
        found = self.scan(lambda t: (t/'Skate 3.iso').write_bytes(b'not a disc' * 100))
        for disc in ('s3-disc', 's2-disc', 's1-disc'):
            self.assertEqual(found[disc]['status'], 'missing')
            self.assertIn("can't be read as an Xbox 360 disc image (Skate 3.iso)", found[disc]['detail'])
        self.assertEqual(found['s3-tu3']['detail'], '', 'a package is not explained by a disc')

    def test_an_unknown_dlc_release_says_so_for_that_games_packs_only(self):
        found = self.scan(lambda t: package(t/'0123456789ABCDEF', '454108E6', 2))
        self.assertIn('different release (0123456789ABCDEF)', found['s3-dlc-afterdark']['detail'])
        self.assertEqual(found['s2-dlc-maloof']['detail'], '')

    def test_another_title_update_is_named(self):
        found = self.scan(lambda t: package(t/'TU_12345678', '454108E6', sources.INSTALLER))
        self.assertEqual((found['s3-tu3']['status'], found['s3-tu3']['detail']),
                         ('mismatch', 'a different title update: Title Update 3 is needed'))

    def test_a_games_on_demand_copy_is_named(self):
        found = self.scan(lambda t: package(t/'ABCDEF0123456789', '4541087F', sources.GOD))
        self.assertEqual((found['s2-disc']['status'], found['s2-disc']['detail']), ('mismatch', sources.GAMES_ON_DEMAND))

    def test_an_extracted_disc_folder_is_named(self):
        body = b'an extracted default.xex'
        ref = {**sources.REFERENCE['items']['s1-disc'], 'xex': hashlib.sha256(body).hexdigest()}
        with mock.patch.dict(sources.REFERENCE['items'], {'s1-disc': ref}):
            found = self.scan(lambda t: ((t/'Skate').mkdir(), (t/'Skate'/'default.xex').write_bytes(body)))
        self.assertEqual((found['s1-disc']['status'], found['s1-disc']['detail']), ('mismatch', sources.EXTRACTED))

    def test_online_only_files_are_mentioned(self):
        real = os.stat

        def cloud(path, *a, **k):
            if str(path).lower().endswith('.iso'):
                return types.SimpleNamespace(st_size=1, st_mtime_ns=1, st_mode=0o100644, st_file_attributes=sources.CLOUD)
            return real(path, *a, **k)
        with mock.patch.object(sources.os, 'stat', cloud):
            found = self.scan(lambda t: (t/'Skate 2.iso').write_bytes(b'x'))
        self.assertIn('online-only (OneDrive or Dropbox)', found['s2-disc']['detail'])
        self.assertIn('online-only', found['mod-newsanvan']['detail'])


if __name__ == '__main__':
    unittest.main()
