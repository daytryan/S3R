import hashlib, tempfile, unittest, zipfile
from unittest import mock
from pathlib import Path
from tools.pipeline.audit import audit


def make_zip(path, files):
    with zipfile.ZipFile(path, 'w') as z:
        for name, data in files.items():
            z.writestr(name, data)
    return path


class Audit(unittest.TestCase):
    def test_clean_release_passes(self):
        with tempfile.TemporaryDirectory() as t:
            z = make_zip(Path(t, 'game.zip'), {'game/skate3rust.exe': b'MZ program'})
            self.assertEqual(audit([z], set()), [])

    def test_planted_disc_file_is_caught(self):
        disc_file = b'\x00EA BIG archive bytes from the disc'
        with tempfile.TemporaryDirectory() as t:
            z = make_zip(Path(t, 'game.zip'), {'game/data/x.bin': disc_file})
            problems = audit([z], {hashlib.sha256(disc_file).hexdigest()})
            self.assertTrue(any('game file' in p for p in problems), problems)

    def test_banned_extension_and_personal_path(self):
        with tempfile.TemporaryDirectory() as t:
            z = make_zip(Path(t, 'game.zip'), {'game/a.big': b'x', 'game/b.txt': b'built in C:\\Users\\Someone\\x'})
            problems = audit([z], set())
            self.assertTrue(any('.big' in p for p in problems) and any('personal path' in p for p in problems), problems)

    def test_escaped_and_forward_slash_paths_are_personal_too(self):
        with tempfile.TemporaryDirectory() as t:
            z = make_zip(Path(t, 'game.zip'), {'game/a.json': b'"C:\\\\Users\\\\x"', 'game/b.py': b"'c:/users/x'"})
            self.assertEqual(len([p for p in audit([z], set()) if 'personal path' in p]), 2)

    def test_setup_bundle_judges_only_our_own_entries(self):
        inner = [('support/skate3setup.exe', b'MZ'),
                 ('support/skate3setup.exe!python312.dll', b'C:\\Users\\runneradmin\\build'),  # third party: fine
                 ('support/skate3setup.exe!PYZ.pyz!tools.owned_game.big', b'code'),  # module name, not a .big
                 ('support/skate3setup.exe!PYZ.pyz!tools.leak', b"'C:/Users/Someone/x'")]  # ours: caught
        with tempfile.TemporaryDirectory() as t, mock.patch('tools.pipeline.audit.members', return_value=inner):
            z = make_zip(Path(t, 'game.zip'), {'game/support/skate3setup.exe': b'MZ'})
            problems = audit([z], set())
            self.assertEqual(len(problems), 1, problems)
            self.assertIn('tools.leak', problems[0])

    def test_the_setup_folder_judges_only_our_own_files(self):
        """B1: the setup tool ships as support/skate3setup/ (--onedir); its third-party files carry CI build paths."""
        files = {'game/support/skate3setup/_internal/numpy.libs/openblas.dll': rb'C:\Users\runneradmin\x',
                 'game/support/skate3setup/_internal/python312.dll': b'C:/Users/runneradmin/x',
                 'game/support/skate3setup/_internal/tools/leak.py': b"'C:/Users/Someone/x'",
                 'game/support/skate3setup/_internal/tools/a.big': b'x'}
        with tempfile.TemporaryDirectory() as t:
            problems = audit([make_zip(Path(t, 'game.zip'), files)], set())
        self.assertEqual(len(problems), 2, problems)
        self.assertTrue(any('leak.py' in p and 'personal path' in p for p in problems), problems)
        self.assertTrue(any('a.big' in p and 'banned' in p for p in problems), problems)


if __name__ == '__main__':
    unittest.main()
