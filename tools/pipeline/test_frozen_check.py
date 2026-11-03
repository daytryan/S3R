import sys, tempfile, unittest
from pathlib import Path
from unittest import mock
from tools import native
from tools.pipeline import frozen_check


class FrozenCheck(unittest.TestCase):
    def test_a_onefile_bundle_is_refused(self):
        """B1: the setup tool must be a folder; a --onefile build unpacks ~113 MB to %TEMP% on every start."""
        exe = Path('C:/game/support/skate3setup/skate3setup.exe')
        with mock.patch.object(sys, 'executable', str(exe)), mock.patch.object(sys, '_MEIPASS', str(exe.parent/'_internal'), create=True):
            self.assertTrue(frozen_check.onedir())
        with mock.patch.object(sys, 'executable', str(exe)), mock.patch.object(sys, '_MEIPASS', 'C:/Temp/_MEI1234', create=True):
            self.assertFalse(frozen_check.onedir())

    def test_a_missing_native_helper_is_named(self):
        """F4: a helper DLL that doesn't load is reported (the release check fails on it), not silently replaced by Python."""
        with tempfile.TemporaryDirectory() as tools:
            for rs in ('lzx_native.rs', 'deep/rx2_native.rs'):
                (Path(tools)/rs).parent.mkdir(exist_ok=True)
                (Path(tools)/rs).write_text('')
            with mock.patch.object(native, 'load', lambda name, beside: object() if name == 'lzx' else None):
                self.assertEqual(native.missing(tools), ['rx2'])
            with mock.patch.object(native, 'load', side_effect=OSError('bad DLL')):
                self.assertEqual(native.missing(tools), ['lzx', 'rx2'])

    def test_every_engine_file_the_career_stage_reads_is_bundled(self):
        """The skate3-career stage's scripts quote engine files (the trick catalog, the speech id tables); the release must ship each one
        (Build-Release.ps1 `$engineSources`), or frozen_check refuses the build."""
        import re, shutil
        from tools.pipeline import stages
        root = Path(frozen_check.__file__).resolve().parents[2]
        script = (root/'scripts/Build-Release.ps1').read_text(encoding='utf-8')
        block = script[script.index('$engineSources ='):script.index('$engineData =')]
        bundled = set(re.findall(r"'(crates/[\w./-]+)'", block))
        bundled |= {f'crates/skate-game/src/{name}' for name in re.findall(r"'(\w+\.rs)'", block)}
        self.assertIn('crates/skate-core/src/scoring/catalog.rs', bundled)   # the parse found the list
        with tempfile.TemporaryDirectory() as scripts:
            for rel in stages.SKATE3_CAREER:
                if '/' not in rel and rel.endswith('.py'):
                    shutil.copy(root/'tools'/rel, Path(scripts)/rel)
            quoted = frozen_check.engine_paths(Path(scripts))
        self.assertIn('crates/skate-game/src/skate3_career/speech_tables.json', quoted)   # the scan sees the quote
        self.assertEqual([p for p in quoted if p not in bundled], [])


if __name__ == '__main__':
    unittest.main()
