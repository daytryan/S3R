# tools/pipeline/test_dlc_location_image.py
"""B2: the location pictures go through one dlc_location_image.py start per stage, not one per picture."""
import json, runpy, sys, tempfile, types, unittest
from pathlib import Path
from unittest import mock
from tools.pipeline import stages

SCRIPT = Path(stages.__file__).with_name('dlc')/'dlc_location_image.py'
REPO = SCRIPT.parents[3]


class FakeBig:
    opened = []

    def __init__(self, path):
        FakeBig.opened.append(Path(path).name)
        self.entries = [types.SimpleNamespace(path=f'data/fe/source/images/locations/{n}.rx2') for n in ('One', 'Two')]

    def read(self, entry):
        return entry.path.encode()


def fake_textures(rx2, folder, force=False, decode_png=False):
    folder.mkdir(parents=True, exist_ok=True)
    (folder/'t.rgba').write_bytes(Path(rx2).read_bytes()[:8].ljust(8, b'\0'))
    (folder/'t.png').write_bytes(b'png')  # removed: only the .rgba stays
    return {'textures': [{'rgba_file': 't.rgba', 'width': 2, 'height': 1}]}


class LocationImages(unittest.TestCase):
    def run_script(self, *args):
        sys.path.insert(0, str(REPO/'tools'))
        from tools.owned_game import big
        from vendor.skate3_ui import rw4
        with mock.patch.object(big, 'BigArchive', FakeBig), mock.patch.object(rw4, 'extract_textures', fake_textures), \
                mock.patch.object(sys, 'argv', [str(SCRIPT), str(REPO), *map(str, args)]), \
                mock.patch.object(sys, 'path', list(sys.path)):
            runpy.run_path(str(SCRIPT), run_name='__main__')

    def test_many_pictures_in_one_start_open_each_archive_once(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)/'challenge-map'
            (root/'runtime').mkdir(parents=True)
            (root/'runtime/challengemap.json').write_text(json.dumps({'thumbnails': {'old.rx2': {}}}))
            FakeBig.opened.clear()
            photo = 'data/fe/source/images/locations/{}.rx2'
            self.run_script('--each', root, 'a.big', photo.format('One'), 'a.big', photo.format('two'), 'b.big', photo.format('Two'))
            self.assertEqual(FakeBig.opened, ['a.big', 'b.big'])
            thumbs = json.loads((root/'runtime/challengemap.json').read_text())['thumbnails']
            self.assertEqual(sorted(thumbs), ['old.rx2', 'one.rx2', 'two.rx2'])
            self.assertEqual(thumbs['one.rx2'], {'rgba': 'images/assets/data/fe/source/images/locations/One/t.rgba', 'width': 2, 'height': 1})
            self.assertEqual(sorted(p.name for p in (root/'images/assets/data/fe/source/images/locations/One').iterdir()), ['t.rgba'])
            # the one-picture form still works
            self.run_script('c.big', photo.format('One'), root)
            self.assertEqual(FakeBig.opened[-1], 'c.big')

    def test_the_stages_start_it_once(self):
        calls = []
        ctx = types.SimpleNamespace(install=Path('I'), disc=lambda i: Path(i), package=lambda i: Path(i),
                                    script=lambda stage, rel, *a: calls.append(rel))
        stages.locations(ctx)
        self.assertEqual(calls.count('pipeline/dlc/dlc_location_image.py'), 1)


if __name__ == '__main__':
    unittest.main()
