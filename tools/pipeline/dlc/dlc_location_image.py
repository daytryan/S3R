"""Install a DLC FE picture (Locations map data/fe/source/images/map/<name>.rx2, or location photo
data/fe/source/images/locations/<name>.rx2) into the prepared Challenge Map screen, decoded exactly
as prepare_challenge_map.py decodes the disc maps. The screen finds it by lower-case file name.
usage: python dlc_location_image.py REPO DLC_BIG ARCHIVE_PATH CHALLENGE_MAP_DIR [PNG_PREVIEW]
       python dlc_location_image.py REPO --each CHALLENGE_MAP_DIR DLC_BIG ARCHIVE_PATH [DLC_BIG ARCHIVE_PATH ...]
The --each form installs many pictures in one start (B2: the frozen setup's start-up was paid once per picture).
"""
import json, sys, tempfile
from pathlib import PurePosixPath, Path

repo = Path(sys.argv[1])
sys.path.insert(0, str(repo))
sys.path.insert(0, str(repo/'tools'))
from tools.owned_game.big import BigArchive
from vendor.skate3_ui.rw4 import extract_textures

if sys.argv[2] == '--each':
    root, rest, preview = Path(sys.argv[3]), sys.argv[4:], None
    jobs = list(zip(rest[::2], rest[1::2]))
else:
    jobs, root, preview = [(sys.argv[2], sys.argv[3])], Path(sys.argv[4]), sys.argv[5] if len(sys.argv) > 5 else None
manifest = root/'runtime/challengemap.json'
data = json.loads(manifest.read_text(encoding='utf-8'))
archives = {}
for big, path in jobs:
    archive = archives[big] = archives.get(big) or BigArchive(Path(big))
    entry, = (e for e in archive.entries if e.path.lower() == path.lower())
    name = PurePosixPath(entry.path).stem
    folder = root/'images/assets'/PurePosixPath(entry.path).with_suffix('')
    with tempfile.TemporaryDirectory() as work:
        rx2 = Path(work)/f'{name}.rx2'
        rx2.write_bytes(archive.read(entry))
        textures = extract_textures(rx2, folder, force=True, decode_png=True)['textures']
    t = next(t for t in textures if t.get('rgba_file'))
    rgba = folder/t['rgba_file']
    assert rgba.stat().st_size == t['width']*t['height']*4
    for extra in folder.glob('*'):
        if extra != rgba: extra.unlink()
    data['thumbnails'][f'{name.lower()}.rx2'] = {'rgba': rgba.relative_to(root).as_posix(), 'width': t['width'], 'height': t['height']}
    print(f"{name.lower()}.rx2: {t['width']}x{t['height']} -> {rgba.relative_to(root).as_posix()}")
manifest.write_text(json.dumps(data, separators=(',', ':')) + '\n', encoding='utf-8')
if preview:
    from PIL import Image
    Image.frombytes('RGBA', (t['width'], t['height']), rgba.read_bytes()).save(preview)
