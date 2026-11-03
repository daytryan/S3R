"""Add owned Xbox DLC Create-a-Skater items (clothes, hair, boards, wheels) and DLC
characters (Skate.Create: Slappy, Fabio) to an installed customiser set.

The newest DLC cas_db_*.xml is a cumulative superset of the disc catalog. This
reruns the catalog/library/lighting stages of the current set with the DLC BIGs
layered over createacharacter.big and marquee.big, builds the DLC roster
characters, then rewrites those stage receipts.
Items whose files belong to DLC packs that are not present are pruned by the library stage.
usage (needs numpy + Pillow): python dlc_clothing.py REPO GAME_ROOT DLC_EXTRACTED ASSETS
"""
import json, re, shutil, sys, tempfile
from pathlib import Path

repo, game, dlc, assets = map(Path, sys.argv[1:5])
sys.path.insert(0, str(repo))
from tools.owned_game import big
from tools.asset_pipeline import customisation_catalog, customisation_library, customiser_lighting, native_roster
from tools.asset_pipeline import customiser_cache as cache
from tools.asset_pipeline.vlt import convert
from tools.asset_pipeline.environment import NAMES

DiscArchive = big.BigArchive
dlc_bigs = [DiscArchive(p) for p in sorted(dlc.glob('*/*.big'))]


class LayeredArchive:
    """A disc archive with the DLC's same-kind entries on top (later archives win on a path clash)."""
    def __init__(self, path):
        kind = '/' + Path(path).stem.lower() + '/'  # data/content/<kind>/, data/content/recipe/<kind>/
        self.owner = {}
        by_path = {}
        for archive in (DiscArchive(path), *dlc_bigs):
            for entry in archive.entries:
                if archive.path != Path(path) and kind not in entry.path.lower():
                    continue
                self.owner[id(entry)] = archive
                by_path[entry.path.lower()] = entry
        self.entries = list(by_path.values())

    def read(self, entry):
        return self.owner[id(entry)].read(entry)


def open_archive(path):
    return LayeredArchive(path) if Path(path).name.lower() in ('createacharacter.big', 'marquee.big') else DiscArchive(path)


customisation_library.BigArchive = customiser_lighting.BigArchive = native_roster.BigArchive = open_archive

base = assets/'private/customisation'
directory = base/'sets'/json.loads((base/'current.json').read_text())['set']
backup = directory/'pre-dlc'
if not (backup/'collections.json').exists():
    backup.mkdir(exist_ok=True)
    for name in ('catalog.json', 'library-base.json', 'library-v3.json', 'native-lighting.json', 'database/collections.json',
                 'native-roster/complete.json', 'catalog-complete.json', 'library-complete.json', 'lighting-complete.json',
                 'roster-complete.json'):
        if not (backup/Path(name).name).exists():
            shutil.copy2(directory/name, backup/Path(name).name)

# Newest cumulative DLC catalog.
catalogs = [(a, e) for a in dlc_bigs for e in a.entries
            if re.fullmatch(r'data/content/recipe/createacharacter/cas_db_xfd1ca62f_\d+\.xml', e.path.lower())]
archive, entry = max(catalogs, key=lambda c: c[1].path.lower())
print('DLC catalog:', entry.path)
layered = LayeredArchive(game/'data/content/createacharacter.big')
catalog = customisation_catalog.parse_catalog(archive.read(entry), {e.path for e in layered.entries})
(directory/'catalog.json').write_text(json.dumps(catalog, indent=2) + '\n', encoding='utf-8')

# DLC roster rows live in the DLC's dlc_*.vlt, keyed by unnamed hashes: name them from
# their marquee string (ID_COMMON_DLC_SLAPPY_MARQUEE -> slappy).
names = [*(repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines(), *NAMES, 'characters_marquee', 'ip', 'pro_skaters']
collections_path = directory/'database/collections.json'
collections = json.loads(collections_path.read_text())
added = []
with tempfile.TemporaryDirectory(prefix='dlc-db-') as work:
    work = Path(work)
    db = DiscArchive(game/'data/big/db.big')
    db.extract_entries([e for e in db.entries if Path(e.path).name.lower() in {'skaterschema.bin', 'skaterschema.vlt'}], work)
    for i, a in enumerate(dlc_bigs):
        a.extract_entries([e for e in a.entries if e.path.startswith('data/db/dlc_') and e.path.endswith(('.vlt', '.bin'))], work/str(i))
        for vlt in (work/str(i)/'data/db').glob('*.vlt'):
            for row in convert(work/'data/db/skaterschema', vlt.with_suffix(''), names)['collections']:
                label = re.fullmatch(r'ID_COMMON_DLC_(\w+)_MARQUEE', row['fields'].get('Hash_174B301910A902C9', {}).get('data', ''))
                if row['class'] != 'characters_marquee' or not label:
                    continue
                row['key'] = label[1].lower()
                collections['collections'] = [r for r in collections['collections']
                                              if (r['class'], r['key']) != ('characters_marquee', row['key'])] + [row]
                added.append(row['key'])
collections_path.write_text(json.dumps(collections))
print('DLC characters:', added)

library = customisation_library.prepare(dict(assets=str(assets), game_root=str(game), directory=str(directory),
                                             library_index='library-base.json'))
print(len(library['errors']), 'variants unavailable (their textures are in DLC packs that are not present)')
# DLC authoring names carry pack prefixes: "Dlc Bloodshed", "Dw Dw Planb Dw Mega".
for material in library['materials'].values():
    material['name'] = re.sub(r'^(?:(?:Dlc|Dw) )+', '', material['name']).replace(' Dw ', ' ')
(directory/'library-base.json').write_text(json.dumps(library, separators=(',', ':')))
customiser_lighting.prepare(game, assets, directory, library)

roster = directory/'native-roster'
report = native_roster.prepare(game, assets, roster, collections_path, directory/'roster-work', only=added) if added else []
shutil.rmtree(directory/'roster-work', ignore_errors=True)
for item in report:
    print('Character', item['name'], item['status'], item.get('error', ''))
complete = json.loads((roster/'complete.json').read_text())
complete['characters'] = sum(1 for p in (roster/'entries').iterdir() if (p/'character.glb').is_file())
(roster/'complete.json').write_text(json.dumps(complete))

for name in ('catalog', 'library', 'lighting', 'roster'):
    marker = directory/(name+'-complete.json')
    saved = json.loads(marker.read_text())
    paths = []
    for relative in cache.OUTPUTS[name]:
        path = directory/relative
        paths.extend(path.rglob('*') if path.is_dir() else [path])
    saved['files'] = cache.receipt(directory, paths, marker)  # unchanged files keep the marker's hashes
    marker.write_text(json.dumps(saved))
# Edit Skater only lists parts with a retail thumbnail (data/fe/cas/<gender>/<slot>/<model>_<material>);
# the DLC ships its own. Decode them next to the disc ones and register them.
sys.path.insert(0, str(repo/'tools'))
from vendor.skate3_ui.rw4 import extract_textures
screen = assets/'private/create-skater'
manifest_path = screen/'runtime/createskater.json'
if not (screen/'runtime/createskater.json.pre-dlc').exists():
    shutil.copy2(manifest_path, screen/'runtime/createskater.json.pre-dlc')
manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
thumbs = 0
for a in dlc_bigs:
    for e in a.entries:
        path = e.path.replace('\\', '/')
        if not (path.lower().startswith('data/fe/cas/') and path.lower().endswith('.rx2')):
            continue
        key = path[len('data/fe/cas/'):-len('.rx2')].lower()
        folder = screen/'thumbs/assets/data/fe/cas'/key
        if not (folder/'manifest.json').exists():
            folder.mkdir(parents=True, exist_ok=True)
            (folder/'source.rx2').write_bytes(a.read(e))
            extracted = json.loads(json.dumps(extract_textures(folder/'source.rx2', folder, force=True, decode_png=True)))
            (folder/'manifest.json').write_text(json.dumps(extracted))
            (folder/'source.rx2').unlink()
        else:
            extracted = json.loads((folder/'manifest.json').read_text())
        texture = next(t for t in extracted['textures'] if t.get('rgba_file'))
        manifest['thumbnails'][key] = {'rgba': (folder/texture['rgba_file']).relative_to(screen).as_posix(),
                                       'width': texture['width'], 'height': texture['height']}
        thumbs += 1
manifest_path.write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
print('DLC thumbnails:', thumbs)
print('DLC_CLOTHING_READY', len(library['models']), 'models', len(library['materials']), 'materials',
      complete['characters'], 'characters')
