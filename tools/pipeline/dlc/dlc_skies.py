"""Export authored skies for converted DLC maps, like tools/asset_pipeline/sky.py does for disc maps.

DLC world rows live in the DLC's own dlc_*.vlt and inherit from disc rows, so
disc + DLC collections are merged. Sky models are read from the DLC archive
first, then the disc's miscload.big.
usage (needs numpy + Pillow): python dlc_skies.py REPO GAME_ROOT DLC_EXTRACTED ASSETS
"""
import json, shutil, sys, tempfile
from pathlib import Path

repo, game, dlc, assets = map(Path, sys.argv[1:5])
sys.path.insert(0, str(repo))
from tools.asset_pipeline import backdrop
backdrop._vendor()
import mdl_parser, rx2_parser
import prepare_hawaiian_dream as prep
from tools.owned_game.big import BigArchive
from tools.asset_pipeline.vlt import convert
from tools.asset_pipeline.environment import Collections, NAMES, key_hash, sky_parameters, world_environment, fog_parameters
from tools.asset_pipeline.sky import _texture

# Converted DLC map internal name -> installed file stem (character lighting reads the stem).
STEMS = {'MaloofMoneyCupDLC': 'Maloof_Money_Cup_NYC', 'DLC_DW_MegaCompund': 'Danny_Way_Hawaiian_Dream',
         'AGPark': 'Art_Gallery', 'Dist_Zen': 'Zen_Plaza', 'SanitariumDLC': 'Sanatorium',
         'BackLotParkDLC': 'Back_Lot_Park', 'DownTownSkateParkNightDLC': 'Downtown_Night_Park'}

names = [*(repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines(), *NAMES]
dlc_bigs = [BigArchive(p) for p in dlc.glob('*/*.big')]
misc = BigArchive(game/'data/big/miscload.big')
archives = [(a, {e.path: e for e in a.entries}) for a in (*dlc_bigs, misc)]

with tempfile.TemporaryDirectory(prefix='dlc-sky-') as work:
    work = Path(work)
    db = BigArchive(game/'data/big/db.big')
    need = {'skaterschema.bin', 'skaterschema.vlt', 'skatercollections.bin', 'skatercollections.vlt'}
    db.extract_entries([e for e in db.entries if Path(e.path).name.lower() in need], work)
    schema = work/'data/db/skaterschema'
    rows = convert(schema, work/'data/db/skatercollections', names)['collections']
    for i, a in enumerate(dlc_bigs):
        a.extract_entries([e for e in a.entries if e.path.startswith('data/db/dlc_') and e.path.endswith(('.vlt', '.bin'))], work/str(i))
        for vlt in (work/str(i)/'data/db').glob('*.vlt'):
            rows += convert(schema, vlt.with_suffix(''), names)['collections']
collections = Collections({'collections': rows})  # later (DLC) rows override disc rows
# ponytail: After Dark's night render location (0x659C8FB5B9C99EF2) is in no disc or DLC
# table we have; borrow the disc 'dlc' row for sun + fog. Sky dome/textures stay the DLC's night ones.
_loc = key_hash('render_locations')
# Skate.Create parks' locations are missing too: Back Lot Park (day sky) borrows 'default',
# Downtown Night Park borrows 'dlc' like After Dark.
for _key, _row in ((0x659C8FB5B9C99EF2, 'dlc'), (0x62B4C4FC5F0D37DE, 'default'), (0x61DD44C934E01952, 'dlc')):
    collections.rows.setdefault((_loc, _key), collections.rows[_loc, key_hash(_row)])

def source(path):
    for a, entries in archives:
        if path in entries:
            return a, entries
    raise FileNotFoundError(path)

def read(path):
    a, entries = source(path)
    return a.read(entries[path])

output = assets/'private/native-skies'
done = set()
for (cls, _), row in collections.rows.items():
    if cls != key_hash('world'):
        continue
    stream = next((v['data'] for k, v in row['fields'].items() if key_hash(k) == key_hash('WorldStream')), None)
    name = (stream or '').removeprefix('DIST_')
    if name not in STEMS or name in done:
        continue
    env = world_environment(collections, row['key'])
    raw = read('data/content/'+env['sky_model']+'.rx2')
    model = mdl_parser.parse_rx2(raw)
    table = rx2_parser.RX2File(raw)
    table.parse()
    # Hawaiian Dream's sky model also carries world-space mountains/mist/trees:
    # bind meshes by GUID, take the sky.* mesh as the dome, the rest become a backdrop.
    groups, bindings = prep._bind_material_groups_by_guid(
        raw, prep._group_material_parameters(model.materials), len(model.meshes), allow_import_order_fallback=False)
    shaders = [prep._material_metadata(groups, i)['shader_name'] for i in range(len(model.meshes))]
    dome, = (i for i, s in enumerate(shaders) if s.startswith('sky.'))
    channels = backdrop.texture_groups(raw, table, ('diffuse', 'specular'))[bindings[dome]['group_index']]
    textures = rx2_parser.parse_rx2(read('data/content/'+env['sky_textures']+'.rx2'))
    diffuse = _texture(textures, channels['diffuse'])
    sun = _texture(textures, channels['specular'])
    env.update(sky_parameters(collections, shaders[dome]))
    env['fog_frame'] = fog_parameters(env['fog'])
    mesh = model.meshes[dome]
    assert mesh.uvs is not None and sun.height == 16 and len(sun.rgba) == sun.width*sun.height*4
    scenery = set(shaders) - {shaders[dome]}
    if scenery:
        a, entries = source('data/content/'+env['sky_model']+'.rx2')
        (assets/'private/native-backdrops').mkdir(parents=True, exist_ok=True)
        backdrop._export(a, entries, env['sky_model'], env['sky_textures'], name, stream,
                         assets/'private/native-backdrops', scenery)
        print(f'{name}: backdrop {sorted(scenery)}')
    (output/(name+'.rgba')).write_bytes(diffuse.rgba)
    (output/(name+'.sun.rgba')).write_bytes(sun.rgba)
    (output/(name+'.json')).write_text(json.dumps({
        'width': diffuse.width, 'height': diffuse.height,
        'positions': mesh.vertices.tolist(), 'uvs': mesh.uvs.tolist(),
        'indices': mesh.faces.reshape(-1).tolist(),
        'environment': env, 'sun_width': sun.width, 'sun_height': sun.height,
        'source_sha256': row['sha256'],
    }, separators=(',', ':')), encoding='utf-8')
    shutil.copyfile(output/(name+'.json'), output/(STEMS[name]+'.json'))
    done.add(name)
    print(f"{name}: {env['sky_model']} {diffuse.width}x{diffuse.height}, sun {sun.width}x16, location {env['location_chain']}")

missing = set(STEMS) - done
if missing:
    sys.exit(f'No sky resolved for: {sorted(missing)}')
