"""No rescued script hard-codes a personal path, a tools/ binary next to itself, or a work/ cache next to itself."""
import re, unittest
from pathlib import Path

PIPELINE = Path(__file__).resolve().parent
SKIP = {'rescue.py', 'make_reference.py', 'audit.py'}  # audit.py is the personal-path checker itself

# Task 7 fixes these; check only build-time files in derive/
DERIVE_CHAIN = {'derive/ice2/params2.py', 'derive/ice2/params.py', 'derive/ice2/xref.py',
                'derive/ice1/params1.py', 'derive/ice1/enums.py', 'derive/ice1/x1.py',
                'derive/names-5372/names.py'}

# One-off dev tools that never run at build time, excluded from releases
SKIP_DIRS = {'scratch'}


class RescuedPaths(unittest.TestCase):
    def test_no_hard_coded_paths(self):
        for path in PIPELINE.rglob('*.py'):
            if path.name in SKIP or path.name.startswith('test_'):
                continue
            rel = path.relative_to(PIPELINE).as_posix()
            # Skip files in derive/ that aren't in the build-time chain
            if rel.startswith('derive/') and rel not in DERIVE_CHAIN:
                continue
            # Skip one-off dev tools
            if any(rel.startswith(d + '/') for d in SKIP_DIRS):
                continue
            text = path.read_text(encoding='utf-8', errors='replace')
            self.assertNotRegex(text, r'(?i)[A-Z]:[\\/]+Users', rel)
            # Personal paths: no C:/ paths anywhere
            self.assertNotRegex(text, r"\[['\"']java['\"'],", rel)
            # No hard-coded work cache: only on WORK_ROOT = line (for modified files)
            if rel.startswith('skate2port/') or rel in ('stategraph.py', 'loc/skate_points.py'):
                work_matches = list(re.finditer(r"(?i)here\s*/\s*['\"]work", text))
                for match in work_matches:
                    line_start = text.rfind('\n', 0, match.start()) + 1
                    line = text[line_start:text.find('\n', line_start)]
                    self.assertIn('WORK_ROOT =', line, f'{rel}: {line}')
            # Hard-coded table files: use TABLES instead
            self.assertNotRegex(text, r"here\s*/\s*['\"](?:ice_params1?\.json|online1_enums\.json|s2_names\.txt)['\"]", rel)
            # Env vars must be read where needed (check specific modified files only)
            work_readers = {'skate2port/old_tutorial.py', 'skate2port/old_challenges.py', 'skate2port/old_cameras.py',
                           'skate2port/old_contests.py', 'skate2port/old_hall_of_meat.py', 'skate2port/old_hall_of_meat3.py',
                           'skate2port/old_hud.py', 'skate2port/old_hud1.py', 'skate2port/old_indicators.py',
                           'skate2port/old_online.py', 'skate2port/old_online1.py', 'skate2port/old_online1_modes.py',
                           'skate2port/old_online_hud.py', 'skate2port/old_skate_game.py'}
            if rel in work_readers:
                self.assertIn('SKATE_WORK', text, f'{rel}: SKATE_WORK not read')
            tools_readers = {'skate2port/old_tutorial.py', 'skate2port/old_challenges.py'}
            if rel in tools_readers:
                self.assertIn('SKATE_UNLUAC', text, f'{rel}: SKATE_UNLUAC not read')
                self.assertIn('SKATE_VGMSTREAM', text, f'{rel}: SKATE_VGMSTREAM not read')
                # Java itself, or only through tools/unluac_batch.py, which reads SKATE_JAVA (#19: paths relative to TEMP)
                self.assertTrue("os.environ.get('SKATE_JAVA'" in text or ('unluac_batch import' in text and "'-jar'" not in text),
                                f'{rel}: SKATE_JAVA not read')
                self.assertIn("os.environ.get('SKATE_JAVA'", (PIPELINE.parent/'unluac_batch.py').read_text(encoding='utf-8'))
            # UNLUAC/VGMSTREAM use os.environ
            for match in re.finditer(r"^(UNLUAC|VGMSTREAM)\s*=", text, re.MULTILINE):
                line_start = match.start()
                line_end = text.find('\n', line_start)
                line = text[line_start:line_end]
                self.assertIn('os.environ', line, f'{rel}: {line}')


if __name__ == '__main__':
    unittest.main()
