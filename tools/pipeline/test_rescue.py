"""Every rescued file is still there, and no game-derived data sits anywhere in tools/pipeline.
Byte-identity with the originals was checked once at rescue time (Task 1); Tasks 6 and 7 edit the copies since."""
import re, unittest
from pathlib import Path

PIPELINE = Path(__file__).resolve().parent
BANNED = ('ice_params', 'online1_enums', 's2_names', '.img', '.jar', '.exe', '.dll')


class Rescue(unittest.TestCase):
    def test_rescued_files_present(self):
        origins = (PIPELINE/'ORIGINS.md').read_text(encoding='utf-8')
        rows = re.findall(r'^\| `(.+?)` \| `(.+?)` \| `([0-9a-f]{16})` \|$', origins, re.M)
        self.assertGreater(len(rows), 60)
        repo = PIPELINE.parents[1]
        for copy, _original, _digest in rows:
            self.assertTrue((repo/copy).is_file(), copy)

    def test_no_game_data(self):
        for path in PIPELINE.rglob('*'):
            if path.is_file() and '__pycache__' not in path.parts:
                self.assertFalse(any(b in path.name for b in BANNED), path)


if __name__ == '__main__':
    unittest.main()
