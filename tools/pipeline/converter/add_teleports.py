"""Add a converted map's authored destinations to the teleport catalog (Escape menu and Locations tab).

Locators come from a district .loc XML (PS3 mod), a DLC .big's global_locators RX2s, or
'-': one destination at the map's current spawn. Only named locators inside the map's
collision are kept. SPAWN_LOCATOR, if given, also becomes the .skate spawn. maps.json and
teleports.json receipts in data/installation.json are refreshed so the game does not ask
for an asset refresh.

usage (DLC .big sources need numpy + Pillow):
  python add_teleports.py DATA_DIR MAP_NAME SOURCE [SPAWN_LOCATOR]
"""
import hashlib, json, mmap, os, re, struct, sys, zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))  # the repo root: tools/pipeline/converter/<this file>
from tools.asset_pipeline.setup_state import atomic_json

NAMES = {
    # New San Vanelona (DIST_NewSanVan_Sim.loc)
    'A_StartWaterFront_SeaWall': 'Waterfront Sea Wall (Skate 2 start)',
    'A_RobsPark': "Rob's Park",
    'old_pool_teleport_locator': 'Old Pool',
    'pink_pool_teleport_locator1': 'Pink Pool',
    'party_pool_teleport_locator': 'Party Pool',
    'community_pool_teleport_locator': 'Community Pool',
    'school_pool_teleport_locator': 'School Pool',
    'poop_deck_teleport_locator': 'Poop Deck',
    'do_drop_inn_teleport_locator': 'Do Drop Inn',
    'freeskate_megacompound_locator': 'Mega Compound',
    'freeskate_oldspillway': 'Old Spillway',
    'Z_SVM01_DAMStart': 'The Dam',
    'Z_PJ03_BackAlleyStart': 'Back Alley',
    'Z_DT4_SkatelineStart': 'Downtown Skate Line',
    'Z_TrainingStart': 'Training Facility',
    'megaRampClassicStart': 'Mega Ramp (Classic)',
    'megaRampTechStart': 'Mega Ramp (Tech)',
    'HipRampStart': 'Big Event Hip Ramp',
    'SideRampStart': 'Big Event Side Ramp',
    # Skate 2 online DLC parks (DIST_<park>_Sim.loc)
    'freeskate_online_DLC_communitycenter': 'Community Center',
    'freeskate_online_DLC_dyrdek': "Rob's Warehouse",
    'freeskate_online_DLC_parkade': 'Parkade',
    'freeskate_online_DLC_School': 'The School',
    # Xbox DLC (global_locators) and '-' spawn destinations, keyed by map name
    'Z_MaloofMCNYC_Start': 'Maloof Money Cup 2010 NYC',
    'freeskate_artgallery_locator': 'Art Gallery',
    'freeskate_zenplazapark_locator': 'Zen Plaza',
    'freeskate_dannyway_megacompound_locator': "Danny Way's Hawaiian Dream",
    'Skate_2_Maloof': 'Maloof Money Cup (Skate 2)',
    'Z_Sanitarium_Start': 'The Sanatorium',
    'freeskate_backlotpark_locator': 'Back Lot Park',
    'freeskate_downtownpark_night_locator': 'Downtown Night Park',
    'teleport_dwtn_skpk_01_ToDwtn_locator_01': 'Downtown Night Park',
    # Skate 1 San Vanelona (PS3 custom map; global_locators/DLC_BAM/DIST_SanVanelona/*.loc,
    # names from its LANGUAGE_English_custom_maps table)
    'freeskate_sanvanelona_art_gallery_locator': 'Art Gallery',
    'freeskate_sanvanelona_cathedral_locator': 'Cathedral',
    'freeskate_sanvanelona_city_hall_locator': 'City Hall',
    'freeskate_sanvanelona_community_center_locator': 'Community Center',
    'freeskate_sanvanelona_diy_park_locator': 'DIY Park',
    'freeskate_sanvanelona_dyrdek_locator': 'Dyrdek',
    'freeskate_sanvanelona_elementary_school_locator': 'Elementary School',
    'freeskate_sanvanelona_industrial_warehouse_locator': 'Industrial Warehouse',
    'freeskate_sanvanelona_library_locator': 'Library',
    'freeskate_sanvanelona_matrix_plaza_locator': 'Matrix Plaza',
    'freeskate_sanvanelona_mega_park_locator': 'Mega Park',
    'freeskate_sanvanelona_monument_locator': 'Monument',
    'freeskate_sanvanelona_parkade_locator': 'Parkade',
    'freeskate_sanvanelona_pj_mansion_locator': 'PJ Mansion',
    'freeskate_sanvanelona_regs_locator': 'Regs',
    'freeskate_sanvanelona_skate_plaza_locator': 'Skate Plaza',
    'freeskate_sanvanelona_slappys_locator': 'Slappys',
    'freeskate_sanvanelona_urban_res_top_locator': 'Urban Res Top',
    'freeskate_sanvanelona_zen_plaza_locator': 'Zen Plaza',
}

data, name, source = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
spawn_name = sys.argv[4] if len(sys.argv) > 4 else None
record = json.loads((data / 'installation.json').read_text(encoding='utf-8'))
install = data / record['directory']
skate = install / 'maps' / f'{name}.skate'


def sha(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 22), b''):
            h.update(block)
    return h.hexdigest()


def spawn_offset():
    """Header: SKATE14\\0, u32, u32-length name, then spawn xyz + heading (floats)."""
    with open(skate, 'rb') as f:
        f.seek(12)
        return 16 + struct.unpack('<I', f.read(4))[0]


# Collision mesh bounds from the manifest the map writer embeds as the WMET extension.
with open(skate, 'rb') as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as mm:
    at = mm.rfind(b'WMET')
    _, _, method, packed = struct.unpack_from('<IIII', mm, at + 4)
    raw = mm[at + 20:at + 20 + packed]
    manifest = json.loads(zlib.decompress(raw) if method == 1 else raw)
    spawn = struct.unpack_from('<3f', mm, spawn_offset())
boxes = [mesh['bounds'] for a in manifest['simulation_assets'] for mesh in a.get('collision_meshes', [])]


def inside(p):
    return any(b['minimum'][0] - 1 <= p[0] <= b['maximum'][0] + 1 and b['minimum'][2] - 1 <= p[2] <= b['maximum'][2] + 1
               and b['minimum'][1] - 3 <= p[1] <= b['maximum'][1] + 3 for b in boxes)


if source == '-':
    # The spawn sits 1 m above its ground point; teleports take the ground point.
    locators = {name: [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [spawn[0], spawn[1] - 1., spawn[2], 1]]}
elif source.lower().endswith('.big'):
    from tools.owned_game.big import BigArchive
    from tools.asset_pipeline.teleports import location_records
    archive = BigArchive(Path(source))
    # One DLC can hold several districts' locators (…/global_locators/<pack>/<district>/*.rx2).
    district = manifest['district_name'].casefold()
    locators = {r['locator']: r['matrix'] for e in archive.entries
                if e.path.startswith('data/content/global_locators/') and e.path.endswith('.rx2')
                and e.path.split('/')[-2].casefold() == district
                for r in location_records(archive.read(e))}
else:
    # One .loc XML, or a folder of them (a DLC's extracted global_locators district).
    files = sorted(Path(source).glob('*.loc')) if Path(source).is_dir() else [Path(source)]
    locators = {n: [[float(v) for v in t.split(',')][i:i + 4] for i in (0, 4, 8, 12)] for f in files for n, t in
                re.findall(r'<Name>(.*?)</Name>.*?<Transform>(.*?)</Transform>', f.read_text(), re.S)}

destinations = []
for locator, m in locators.items():
    if locator not in NAMES:
        continue
    if not inside(m[3][:3]):
        print('skipped (outside collision)', locator)
        continue
    destinations.append({'id': f'{name.lower()}_{locator.lower()}', 'name': NAMES[locator], 'map': name,
                         'locator': locator, 'matrix': m, 'source': Path(source).name})
if not destinations:
    sys.exit(f'No named destinations for {name} in {source}')

teleports = install / 'assets/private/teleports.json'
catalog = json.loads(teleports.read_text(encoding='utf-8'))
catalog['destinations'] = sorted([d for d in catalog['destinations'] if d['map'] != name] + destinations,
                                 key=lambda d: (d['map'].casefold(), d['name'].casefold()))
atomic_json(teleports, catalog)

if spawn_name:
    x, y, z = locators[spawn_name][3][:3]
    with open(skate, 'r+b') as f:
        f.seek(spawn_offset())
        f.write(struct.pack('<3f', x, y + 1., z))
# The map may also have been reconverted: always record its current checksum.
maps_path = install / 'maps.json'
maps = json.loads(maps_path.read_text())
for entry in maps:
    if entry['path'] == f'maps/{name}.skate':
        entry['sha256'] = sha(skate)
atomic_json(maps_path, maps)

for group, path in (('maps', 'maps.json'), ('environment', 'assets/private/teleports.json')):
    if path in record['outputs'].get(group, {}):
        record['outputs'][group][path].update(size=(install / path).stat().st_size, sha256=sha(install / path))
atomic_json(data / 'installation.json', record)
print(f'{name}: {", ".join(d["name"] for d in destinations)}' + (f'; spawn at {spawn_name}' if spawn_name else ''))
