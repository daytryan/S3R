"""Tables rebuilt from the user's discs equal the ones the current install was built with (read-only oracle)."""
import json, os, shutil, sys, tempfile, unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock
from tools.pipeline import derive
from tools.pipeline.derive import derive_tables

# The oracle: the original tables folder and the extracted discs (unset: the oracle test skips)
OLD = Path(os.environ.get('SKATE_TABLES') or 'unset')
S2 = Path(os.environ.get('SKATE2_DISC') or 'unset')
S1 = Path(os.environ.get('SKATE1_DISC') or 'unset')
TABLES = ['ice_params.json', 'ice_params1.json', 'online1_enums.json', 's2_names.txt']


class FakeBig:  # db.big holding the two tables names.py reads, each with one compiled Lua chunk
    def __init__(self, path):
        self.entries = [SimpleNamespace(path=f'db/{n}') for n in derive.DB_TABLES]

    def read(self, entry):
        return b'\x1bLuaQ' + bytes(64)


class Failures(unittest.TestCase):
    """No discs needed: a bad generator run fails loudly and leaves nothing behind."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        (self.tmp/'unluac.jar').touch()
        self.env = {'SKATE_UNLUAC': str(self.tmp/'unluac.jar'), 'SKATE_JAVA': sys.executable}
        self.out = self.tmp/'tables'
        self.out.mkdir()
        self.build = mock.patch('tools.build_skate2_image.build', return_value=b'img Foo_bar\x00')
        self.build.start()
        self.addCleanup(self.build.stop)
        mock.patch('tools.pipeline.skate2port.s2big.Big', FakeBig).start()
        self.addCleanup(mock.patch.stopall)

    def script(self, name, code):
        (self.tmp/name).write_text(code)
        return str(self.tmp/name)

    def derive(self, generators):
        with mock.patch.object(derive, 'GENERATORS', generators):
            derive_tables(self.tmp, self.tmp, self.out, env=self.env)

    def test_failing_generator_publishes_no_tables(self):
        ok = self.script('ok.py', "open('a.json', 'w').write('{\"x\": 1}')")
        bad = self.script('bad.py', 'raise SystemExit(3)')
        with self.assertRaisesRegex(RuntimeError, 'bad.py exited with code 3'):
            self.derive([(ok, 's2', 'a.json', 'a.json'), (bad, 's1', None, 'b.json')])
        self.assertEqual(list(self.out.iterdir()), [])  # no partial set, no scratch

    def test_empty_output_is_an_error(self):
        empty = self.script('empty.py', "print('{}')")
        with self.assertRaisesRegex(RuntimeError, 'empty.py wrote an empty b.json'):
            self.derive([(empty, 's1', None, 'b.json')])
        self.assertEqual(list(self.out.iterdir()), [])

    def test_missing_unluac_fails_before_any_work(self):
        self.env['SKATE_UNLUAC'] = str(self.tmp/'nope.jar')
        with self.assertRaisesRegex(RuntimeError, 'unluac'):
            self.derive(derive.GENERATORS)
        tools_build = sys.modules['tools.build_skate2_image'].build
        tools_build.assert_not_called()

    def test_unluac_failure_is_an_error(self):  # "java" here is python, which rejects -jar: no chunk decompiles
        with self.assertRaisesRegex(RuntimeError, 'names.py exited with code 1: unluac decompiled none'):
            self.derive([g for g in derive.GENERATORS if g[0].endswith('names.py')])
        self.assertEqual(list(self.out.iterdir()), [])


@unittest.skipUnless(OLD.is_dir() and S2.is_dir() and S1.is_dir() and (OLD/'tools/unluac.jar').is_file()
                     and shutil.which('java'), 'needs the user disc copies, unluac.jar and java')
class Derive(unittest.TestCase):
    def test_tables_match_the_ones_in_use(self):
        with tempfile.TemporaryDirectory() as t:
            derive_tables(S2, S1, Path(t), env={'SKATE_UNLUAC': str(OLD/'tools/unluac.jar')})
            for name in ('ice_params.json', 'ice_params1.json', 'online1_enums.json'):
                self.assertEqual(json.loads(Path(t, name).read_text(encoding='utf-8')),
                                 json.loads((OLD/name).read_text(encoding='utf-8')), name)
            self.assertEqual(Path(t, 's2_names.txt').read_text(encoding='utf-8').split(),
                             (OLD/'s2_names.txt').read_text(encoding='utf-8').split())
            # memory images and db dumps are game content: only the tables may be left behind
            self.assertEqual(sorted(p.name for p in Path(t).iterdir()), TABLES)


if __name__ == '__main__':
    unittest.main()
