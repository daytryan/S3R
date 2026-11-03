"""#11 (player build review): packages on another drive are copied, so the disk check counts them."""
import tempfile, types, unittest
from pathlib import Path
from tools.pipeline import runner
from tools.pipeline.stages import S3_DLC


class PackageCopies(unittest.TestCase):
    def ctx(self, base, folder):
        found = {}
        for item in S3_DLC:
            (folder/item).write_bytes(b'x' * 10)
            found[item] = {'path': str(folder/item)}
        return types.SimpleNamespace(base=base, found=found)

    def test_another_drive_counts_one_copy_per_folder_that_holds_them(self):
        with tempfile.TemporaryDirectory() as t:
            here = Path(t)
            other = 'Z:' if not here.drive.upper().startswith('Z') else 'Y:'
            ctx = self.ctx(Path(other + '/game'), here)
            nine = 10 * len(S3_DLC)
            self.assertEqual(runner.package_copies(ctx, ['online-challenge-map', 'online-skate3-export', 'online-skate3']), 2 * nine)
            self.assertEqual(runner.package_copies(ctx, ['skate3-base']), 0, 'no stage that holds them')

    def test_the_same_drive_costs_nothing(self):
        with tempfile.TemporaryDirectory() as t:
            ctx = self.ctx(Path(t, 'game'), Path(t))
            self.assertEqual(runner.package_copies(ctx, ['online-challenge-map', 'online-skate3']), 0)


if __name__ == '__main__':
    unittest.main()
