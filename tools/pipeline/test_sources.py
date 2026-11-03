"""Item identification against the user's Skate Source Files (read-only; layouts are rebuilt with hard links)."""
import os, tempfile, unittest
from pathlib import Path
from unittest import mock
from tools.pipeline import sources

# The developer's verified source files and San Van copy A folder (unset: those tests skip)
SSF = Path(os.environ.get('SKATE_SOURCES') or 'unset')
SANVAN_A = Path(os.environ.get('SKATE_SANVAN_A_DIR') or 'unset')


class WrongRegion(unittest.TestCase):
    def test_wrong_region_disc_is_a_mismatch(self):
        names = sorted(sources.REFERENCE['items']['s1-disc']['files'])

        class OtherRegion:  # a Skate 1 disc whose default.xex differs, holding just over half its files
            def __init__(self, path): pass
            def __enter__(self): return self
            def __exit__(self, *exc): pass
            def read(self, name): return b'another region'
            def files(self): return dict.fromkeys(n.upper() for n in names[:len(names) // 2 + 1])

        with tempfile.TemporaryDirectory() as t, mock.patch.object(sources, 'Iso', OtherRegion):
            Path(t, 'Skate (USA).iso').touch()
            found = sources.scan([Path(t)])
        self.assertEqual((found['s1-disc']['status'], found['s1-disc']['detail']),
                         ('mismatch', 'a different region or revision (default.xex differs)'))
        self.assertEqual(found['s2-disc']['status'], 'missing')


class VerifiedOnce(unittest.TestCase):
    def test_an_unchanged_item_is_not_rehashed_and_a_changed_one_is(self):
        import hashlib, json
        body = b'mod file'
        ref = {'kind': 'mod', 'files': {'a.bin': [len(body), hashlib.sha256(body).hexdigest()]}}
        with tempfile.TemporaryDirectory() as t, \
                mock.patch.dict(sources.REFERENCE['items'], {'mod-x': ref}), \
                mock.patch.object(sources, 'ITEMS', [('mod-x', 'x', 'Mod X', 'mod', None)]):
            folder, cache = Path(t, 'mod'), Path(t, 'sources-verified.json')
            folder.mkdir(); (folder/'a.bin').write_bytes(body)
            found = {'mod-x': {'status': 'ok', 'path': str(folder), 'identity': None, 'detail': ''}}
            calls = []
            real = sources.verify_one
            with mock.patch.object(sources, 'verify_one', lambda *a: (calls.append(a[0]), real(*a))):
                self.assertEqual(sources.verify(found, cache), [])
                self.assertIn('mod-x', json.loads(cache.read_text()))
                self.assertEqual(sources.verify(found, cache), [])
                self.assertEqual(calls, ['mod-x'], 'the second check trusts the unchanged file')
                (folder/'a.bin').write_bytes(b'mod fil!')  # same size, new content and modified time
                os.utime(folder/'a.bin', ns=(1, 1))
                self.assertTrue(sources.verify(found, cache), 'a changed file is hashed again and fails')
                self.assertEqual(calls, ['mod-x', 'mod-x'])
                self.assertNotIn('mod-x', json.loads(cache.read_text()), 'a failed item is not remembered')


@unittest.skipUnless(SSF.is_dir(), 'needs Skate Source Files')
class Sources(unittest.TestCase):
    def test_all_18_found_in_skate_source_files(self):
        found = sources.scan([SSF])
        self.assertEqual(len(sources.ITEMS), 18)
        self.assertEqual({k: v['status'] for k, v in found.items()}, {i[0]: 'ok' for i in sources.ITEMS})

    def test_shuffled_layout_with_odd_names(self):
        with tempfile.TemporaryDirectory(dir=Path.home()/'AppData/Local/Temp') as t:
            root = Path(t, "Skate & Co' Ünï")
            for i, path in enumerate(p for p in SSF.rglob('*') if p.is_file()):
                out = root/('deep/' * (i % 3))/path.relative_to(SSF).as_posix().replace('/', '_')
                if 'RPCS3' in str(path) or 'Mod' in str(path):     # mods are folders: keep their tree
                    out = root/'mods'/path.relative_to(SSF)
                out.parent.mkdir(parents=True, exist_ok=True)
                os.link(path, out)                                  # same volume: instant, no copy
            found = sources.scan([root])
            self.assertTrue(all(v['status'] == 'ok' for v in found.values()), found)

    def test_truncated_package_is_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            src = next(p for p in (SSF/'05 - Skate 2 DLC (Xbox 360)').rglob('*') if p.is_file() and p.stat().st_size > 1 << 20)
            cut = Path(t, src.name)
            with open(src, 'rb') as r, open(cut, 'wb') as w:
                w.write(r.read(src.stat().st_size // 2))
            found = sources.scan([Path(t)])
            self.assertTrue(any(v['status'] == 'mismatch' and 'incomplete' in v['detail'] for v in found.values()), found)

    @unittest.skipUnless(SANVAN_A.is_dir(), 'needs San Van copy A')
    def test_sanvan_a_alone_is_found(self):
        found = sources.scan([SANVAN_A])
        self.assertEqual(found['s3-dlc-sanvan']['status'], 'ok')
        self.assertTrue(found['s3-dlc-sanvan']['identity'].lower().startswith('20ca6487'))

    @unittest.skipUnless(SANVAN_A.is_dir(), 'needs San Van copy A')
    def test_sanvan_b_preferred_over_a_when_both_present(self):
        found = sources.scan([SANVAN_A, SSF])
        self.assertEqual(found['s3-dlc-sanvan']['status'], 'ok')
        self.assertTrue(found['s3-dlc-sanvan']['identity'].lower().startswith('8576ae23'))

    @unittest.skipUnless(SANVAN_A.is_dir(), 'needs San Van copy A')
    def test_sanvan_b_preferred_when_ssf_first(self):
        found = sources.scan([SSF, SANVAN_A])
        self.assertEqual(found['s3-dlc-sanvan']['status'], 'ok')
        self.assertTrue(found['s3-dlc-sanvan']['identity'].lower().startswith('8576ae23'))

    def test_verify_with_missing_slot_returns_problem_line(self):
        found = sources.scan([SSF])
        # Manually remove one item to mark it missing
        missing_id = 's3-dlc-sanvan'
        found[missing_id]['status'] = 'missing'
        found[missing_id]['path'] = None
        problems = sources.verify(found)
        self.assertTrue(any('San Van' in p and 'missing' in p for p in problems), problems)

    def test_verify_of_truncated_iso_returns_problem_line(self):
        with tempfile.TemporaryDirectory() as t:
            src = next(SSF.rglob('Skate 3 (USA, Europe)*.iso'))
            cut = Path(t, src.name)
            with open(src, 'rb') as r, open(cut, 'wb') as w:
                w.write(r.read(src.stat().st_size // 2))
            # Manually populate a found dict with the truncated ISO
            found = {item[0]: {'status': 'missing', 'path': None, 'identity': None, 'detail': ''} for item in sources.ITEMS}
            found['s3-disc'] = {'status': 'ok', 'path': str(cut), 'identity': None, 'detail': ''}
            problems = sources.verify(found)
            self.assertTrue(any('Skate 3' in p and ('truncated' in p or 'files differ' in p) for p in problems), problems)

    def test_unreadable_package_is_skipped(self):
        with tempfile.TemporaryDirectory() as t:
            # Create a real package file and a junk file with hex-code name
            src_pkg = next(p for p in (SSF/'03 - Skate 3 DLC (Xbox 360)').rglob('*') if p.is_file() and len(p.name) > 30)
            pkg_copy = Path(t, src_pkg.name)
            os.link(src_pkg, pkg_copy)
            junk = Path(t, '12345678_junk')
            junk.write_bytes(b'not an STFS file')
            # Mock stfs_header to raise PermissionError for the junk file only
            orig_stfs_header = sources.stfs_header
            def mock_stfs_header(path):
                if path.name.startswith('12345678'):
                    raise PermissionError('Access denied')
                return orig_stfs_header(path)
            with mock.patch('tools.pipeline.sources.stfs_header', side_effect=mock_stfs_header):
                found = sources.scan([Path(t)])
            # Real package should be found; junk should be skipped
            dlc_items = [k for k in found if k.startswith('s3-dlc')]
            self.assertTrue(any(found[k]['status'] == 'ok' for k in dlc_items), f'Expected at least one DLC found, got {found}')


class ScanCache(unittest.TestCase):
    def test_a_repeat_scan_reads_no_unchanged_file_and_skips_cloud_files_and_system_folders(self):
        """A3/B7: answers reused while size and modified time hold (a hit and a miss alike); a changed file is read
        again; online-only (cloud) files and folders such as node_modules are never opened."""
        hit = ('s3-tu3', 'digest', None)
        with tempfile.TemporaryDirectory() as t:
            top, cache = Path(t, 'src'), Path(t, 'sources-identified.json')
            (top/'node_modules').mkdir(parents=True)
            (top/'node_modules/TU_hidden').write_bytes(b'x')
            for name in ('TU_good', 'other.iso', 'TU_cloud'):
                (top/name).write_bytes(b'x')
            if os.name == 'nt':  # FILE_ATTRIBUTE_OFFLINE, as an online-only OneDrive file carries
                import ctypes
                ctypes.windll.kernel32.SetFileAttributesW(str(top/'TU_cloud'), 0x1000)
            ident = lambda path: hit if path.name == 'TU_good' else None
            with mock.patch.object(sources, 'identify_file', side_effect=ident) as read:
                first = sources.scan([top], cache)
                opened = sorted(c.args[0].name for c in read.call_args_list)
                self.assertEqual(opened, ['TU_good', 'other.iso'] + ([] if os.name == 'nt' else ['TU_cloud']))
                read.reset_mock()
                self.assertEqual(sources.scan([top], cache), first)
                read.assert_not_called()
                (top/'other.iso').write_bytes(b'longer')
                sources.scan([top], cache)
                self.assertEqual([c.args[0].name for c in read.call_args_list], ['other.iso'])
            self.assertEqual(first['s3-tu3']['path'], str(top/'TU_good'))


if __name__ == '__main__':
    unittest.main()
