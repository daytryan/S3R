"""Adds the "Everyone Is Welcome Here" deck: the six-stripe pride rainbow on the retail flag-deck layout.

Usage: rainbow_deck.py REPO ASSETS

Ships no assets: the texture is derived from the player's own Canada deck texture (decoded/2c7f38180017028e.png),
its flag area (x 0..463, y 256..511) repainted as six equal stripes, top to bottom red, orange, yellow, green, blue,
violet, laid along the same axis as the flag decks. The grip half, edge rows, wood strip and alpha stay Canada's.
The material copies Canada (brand skate, shader, lighting, normal map) but costs 0, has no unlock and no worn twin.

Writes the current customisation set (catalog.json, library-base.json, library-v3.json, decoded/<texture>.png and
the catalog/library/lighting receipts) and Edit Skater's tile + name (createskater.json, thumbs/). Idempotent: a
rerun replaces its own entries. Backups once, before the first change: <set>/pre-rainbow/, createskater.json.pre-rainbow.
Rerun after any customiser refresh (dlc/old/pro clothing regenerate the set)."""
import copy, json, shutil, sys
from pathlib import Path
import numpy as np
from PIL import Image

repo, assets = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/mixamo_to_skate'), str(repo/'tools/pipeline/skate2port')]
from tools.asset_pipeline.setup_state import atomic_json, receipt
import cas_tiles

MODEL = 'f2c517ee8b8359da'                      # the deck model ("Skateboard") every flag deck sits on
CANADA, CANADA_TEXTURE = '2c7f3817001702f6', '2c7f38180017028e'
MID, TID = 'ffff000000000001', 'ffff000000000002'  # custom ids: retail ids start 0000/2c7f, none start ffff
NAME, INSTANCE = 'Everyone Is Welcome Here', 'skateboard_everyone_is_welcome_here_material'
STRIPES = ('#E40303', '#FF8C00', '#FFED00', '#008026', '#24408E', '#732982')
ART_X, ART_Y = (0, 464), (256, 512)             # the flag decks' art area on the 512x512 deck texture

base_dir = assets/'private/customisation'
folder = base_dir/'sets'/json.loads((base_dir/'current.json').read_text())['set']
rel = folder.relative_to(assets).as_posix()
screen = assets/'private/create-skater'
manifest_path = screen/'runtime/createskater.json'

backup = folder/'pre-rainbow'
if not backup.exists():
    backup.mkdir()
    for name in ('catalog.json', 'library-base.json', 'library-v3.json',
                 'catalog-complete.json', 'library-complete.json', 'lighting-complete.json'):
        shutil.copy2(folder/name, backup/name)
    shutil.copy2(manifest_path, manifest_path.with_name(manifest_path.name + '.pre-rainbow'))


def paint(canada):
    a = np.asarray(canada.convert('RGBA')).copy()
    assert a.shape[:2] == (512, 512), a.shape
    (x0, x1), (y0, y1) = ART_X, ART_Y
    band = (np.arange(y1 - y0) * len(STRIPES)) // (y1 - y0)
    colours = np.array([[int(c[i:i + 2], 16) for i in (1, 3, 5)] for c in STRIPES], np.uint8)
    a[y0:y1, x0:x1, :3] = colours[band][:, None, :]
    return Image.fromarray(a, 'RGBA')


paint(Image.open(folder/'decoded'/f'{CANADA_TEXTURE}.png')).save(folder/'decoded'/f'{TID}.png')
diffuse = f'{rel}/decoded/{TID}.png'

for name in ('library-base.json', 'library-v3.json'):
    library = json.loads((folder/name).read_text(encoding='utf-8'))
    m = copy.deepcopy(library['materials'][CANADA])
    m.update(name=NAME, diffuse=diffuse, flags={'Cost': '0', 'ManufacturerID': 'skate'})
    library['materials'][MID] = m
    model = library['models'][MODEL]
    for ids in (model['materials'], *model['groups'][:1]):
        if MID not in ids:
            ids.append(MID)
    atomic_json(folder/name, library)
    if name == 'library-v3.json':
        v3 = library

catalog = json.loads((folder/'catalog.json').read_text(encoding='utf-8'))
m = copy.deepcopy(catalog['materials'][CANADA])
for t in m['textures']:
    if t['channel'] == 'diffuse':
        t.update(id=TID, path=f'decoded/{TID}.png', in_archive=False)
values = {'cas.Cost': '0', 'cas.AlternateMaterialName': ''}
m['flags'].update(values)
m['authored']['attributes']['id'] = '0x' + MID
for child in (c for ud in m['authored'].get('children', []) for c in ud.get('children', [])):
    if child['tag'] in values:
        child['attributes']['value'] = values[child['tag']]
catalog['materials'][MID] = m
deck = next(md for c in catalog['components'] for md in c['models'] if md['id'] == MODEL)
for lod in deck['lods']:
    group = [v for v in lod['material_instances'][0] if v['id'] != MID]
    group.append({'id': MID, 'name': INSTANCE, 'authored': {'tag': 'matvar', 'attributes': {'id': '0x' + MID, 'n': INSTANCE}}})
    lod['material_instances'][0] = group
temporary = folder/'catalog.json.tmp'
temporary.write_text(json.dumps(catalog, indent=2) + '\n', encoding='utf-8')
temporary.replace(folder/'catalog.json')

for stage, files in (('catalog', ['catalog.json']), ('library', ['library-base.json', f'decoded/{TID}.png']),
                     ('lighting', ['library-v3.json'])):
    marker = folder/f'{stage}-complete.json'
    saved = json.loads(marker.read_text())
    saved['files'].update(receipt(folder, [folder/f for f in files]))
    atomic_json(marker, saved)

# Edit Skater: the tile (drawn on retail's board template, as every ported board is) and the part name
stem = f"{deck['name']}_{INSTANCE}".lower()
key = f'skateboard/{stem}'
tile = dict(cas_tiles.render_model(assets, v3, 'skateboard', MODEL, [MID]))[MID]
out = screen/'thumbs/assets/data/fe/cas'/key
shutil.rmtree(out, ignore_errors=True)
out.mkdir(parents=True)
rgba = out/f'0000_{stem}.Texture.rgba'
rgba.write_bytes(np.asarray(tile.convert('RGBA'), np.uint8).tobytes())
tile.save(rgba.with_suffix('.png'))
(out/'manifest.json').write_text(json.dumps({'textures': [{'rgba_file': rgba.name, 'width': 128, 'height': 128}],
                                             'drawn': f'cas_tiles {cas_tiles.TEMPLATE_VERSION}'}))
manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
manifest['thumbnails'][key] = {'rgba': rgba.relative_to(screen).as_posix(), 'width': 128, 'height': 128}
manifest['language']['ID_CAC_PART_' + stem.upper()] = NAME
temporary = manifest_path.with_name(manifest_path.name + '.tmp')
temporary.write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
temporary.replace(manifest_path)
print('RAINBOW_DECK_READY', MID, key)
