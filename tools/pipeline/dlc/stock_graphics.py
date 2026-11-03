"""Add Skate 3's stock brand/team logos ("graphics") to an installed customiser set.

The pipeline (customisation_library.py) now indexes them itself; this fills a set made before that,
without regenerating it (which would drop the DLC / Skate 1+2 / pro clothing merged into it).
Decodes each logo decal into <set>/decoded, adds `graphics` to library-base.json and library-v3.json,
and rewrites those files' receipts. Backups in <set>/pre-graphics/. Rerun after pro_clothing.py only if
that ran with an older customisation_library.py (it regenerates the library).

usage (Pillow): py -3 stock_graphics.py REPO GAME_ROOT SET_DIR
"""
import json, shutil, sys
from pathlib import Path

repo, game, folder = map(Path, sys.argv[1:4])
sys.path[:0] = [str(repo)]
from tools.asset_pipeline.customisation_library import graphic_kind, graphic_name
from tools.asset_pipeline.customisation_catalog import write_private
from tools.asset_pipeline.setup_state import atomic_json, receipt
from tools.extract_default_skater import import_rx2_parser, decode_texture
from tools.owned_game.big import BigArchive

assets = folder.parents[3]
catalog = json.loads((folder/'catalog.json').read_text())
archive = BigArchive(game/'data/content/createacharacter.big')
entries = {e.path.lower(): e for e in archive.entries}
parser = import_rx2_parser(repo/'tools/vendor/utt')
written = []


def texture(tid):
    png = folder/'decoded'/(tid+'.png')
    if not png.exists():
        path = 'data/content/createacharacter/texture/0x'+tid+'.rx2'
        source = folder/'source'/path
        if not source.exists():
            write_private(source, archive.read(entries[path.lower()]))
            written.append(source)
        decode_texture(parser, source, png)
        written.append(png)
    return png.relative_to(assets).as_posix()


graphics = {}
misc = next(p for p in catalog['components'] if p['slot'] == 'Misc')
for variant in (v for m in misc['models'] for lod in m['lods'] for g in lod['material_instances'] for v in g):
    mat = catalog['materials'][variant['id']]
    kind = graphic_kind(variant['name'])
    if kind and not mat['flags'].get('cas.TattooCategory') and variant['id'] not in graphics:
        decal = {t['channel']: t['id'] for t in mat['textures']}['decal']
        graphics[variant['id']] = dict(name=graphic_name(variant['name']), kind=kind, texture=texture(decal),
                                       bounds=[float(v) for v in mat['flags']['cas.StampBorderConstraint'].split(',')])

backup = folder/'pre-graphics'
backup.mkdir(exist_ok=True)
for name in ('library-base.json', 'library-v3.json', 'library-complete.json', 'lighting-complete.json'):
    if not (backup/name).exists():
        shutil.copyfile(folder/name, backup/name)
for name in ('library-base.json', 'library-v3.json'):
    library = json.loads((folder/name).read_text())
    library['graphics'] = graphics
    (folder/name).write_text(json.dumps(library, separators=(',', ':')))
for stage, extra in (('library', ['library-base.json', *written]), ('lighting', ['library-v3.json'])):
    path = folder/(stage+'-complete.json')
    saved = json.loads(path.read_text())
    saved['files'].update(receipt(folder, [folder/p for p in extra]))
    atomic_json(path, saved)
print('GRAPHICS', len(graphics), 'brand', sum(g['kind'] == 'brand' for g in graphics.values()),
      'team', sum(g['kind'] == 'team' for g in graphics.values()), 'decoded', sum(p.suffix == '.png' for p in written))
