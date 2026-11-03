"""Write reference.json from the user's verified Skate Source Files (plus San Van copy A). Hashes and names only."""
import json, os, sys
from pathlib import Path
from tools.pipeline.xdvdfs import Iso
from tools.pipeline.sources import ITEMS, sha256, stfs_header
import hashlib

# Dev-only: the verified source files folder and the San Van Party Pack copy A package, from the environment.
SSF = Path(os.environ.get('SKATE_SOURCES') or sys.exit('set SKATE_SOURCES to the verified source files folder'))
EXTRA = [Path(os.environ.get('SKATE_SANVAN_A') or sys.exit('set SKATE_SANVAN_A to the San Van Party Pack copy A package'))]
PACKAGES = {  # STFS file name (first 8 characters) -> item
    '20CA6487': 's3-dlc-sanvan', '8576AE23': 's3-dlc-sanvan', 'E8CB22B7': 's3-dlc-hawaii',
    '0165E5B9': 's3-dlc-maloof', 'B6B90663': 's3-dlc-afterdark', 'E33EB431': 's3-dlc-create',
    'D1472E11': 's3-dlc-blackbox', '9E0AE017': 's3-dlc-filmer', 'D0F772D8': 's3-dlc-share',
    '8B9253EB': 's3-dlc-money', 'EF8A3514': 's2-dlc-maloof', 'B6E4D01C': 's2-dlc-filmer',
    'AC4907A5': 's2-dlc-throwback', 'TU_12K22': 's3-tu3',
}


def main():
    for extra_file in EXTRA:
        assert extra_file.is_file(), f'EXTRA file not found: {extra_file}'
    items = {}
    for item_id, folder, _, kind, title in ITEMS:
        top = next(SSF.glob(folder + ' - *'))
        if kind == 'iso':
            with Iso(next(top.rglob('*.iso'))) as iso:
                files = {n: [s, hashlib.sha256(iso.read(n)).hexdigest()] for n, (_, s) in iso.files().items()}
                items[item_id] = {'kind': 'iso', 'xex': files['default.xex'][1], 'files': files}
        elif kind == 'mod':
            files = {p.relative_to(top).as_posix(): [p.stat().st_size, sha256(p)] for p in top.rglob('*') if p.is_file()}
            marker = max(files, key=lambda n: (files[n][0], n))
            items[item_id] = {'kind': 'mod', 'marker': marker, 'files': files}
        else:
            items[item_id] = {'kind': 'stfs', 'title': title, 'headers': {}}
    for path in [*SSF.rglob('*'), *EXTRA]:
        item_id = PACKAGES.get(path.name[:8])
        if path.is_file() and item_id:
            title, digest = stfs_header(path)
            assert title == items[item_id]['title'], path
            items[item_id]['headers'][digest] = path.stat().st_size
    assert all(v['kind'] != 'stfs' or v['headers'] for v in items.values()), 'a package is missing'
    assert len(items['s3-dlc-sanvan']['headers']) == 2, 'San Van should have two headers (copy A and B)'
    out = Path(__file__).with_name('reference.json')
    out.write_text(json.dumps({'version': 1, 'items': items}, indent=0, sort_keys=True), encoding='utf-8')
    print('wrote', out, len(items), 'items')


if __name__ == '__main__':
    sys.exit(main())
