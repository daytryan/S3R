"""Put what the Free Play roster (pros and specials) wear into the Edit Skater merchandise:
hats, tops, pants/shorts, shoes, wristwear, rings, pendants, accessories, and each pro's
board split into deck, trucks and wheels. Plus Mike's backpack from Skate 2 (Accessories), the Skate 2
pros' own outfits (Skate 2's marquee recipes, skate2 badge) and the gloves the marquee characters wear
(Deer Man; Danny Way's vert outfit from the Danny Way pack) under Accessories > Gloves.

The roster's marquee recipes (data/content/recipe/marquee/<recipe>.xml, base game + DLC)
name one model per component; this script serves those meshes and textures under virtual
createacharacter paths and catalogs them like CAS items, so customisation_library and
customiser_lighting convert and light them unchanged. Lighting is each mesh's own authored
character shader (as the pro renders in Free Play); skin shaders on garments (rings and
Mike's bag sample the arm-skin atlas) become the cloth shader. Boards are cut into deck, trucks
and wheels (keep_part); tops get their collar pushed clear of the CAS head's neck yoke (fit_neck).

Left out: heads, eyes, hair (unique marquee meshes, not CAS hair styles with full/hat pairs),
arms and legs (body, except a gloved character's 'Arm', which is the gloves), and the costumes that replace the body (Dem Bones' skeleton, Meat Man,
the Dr Pepper can and legs).

Names: every token carries 's2mq' (tile stems never collide; old_skate_clothing.py strips
s1/s2 models on its rerun, so it drops these instead of failing on paths only this script
serves; rerun this script after it). Ids are derived from the source identity, so reruns
reuse the converted GLBs and drawn tiles.

usage (numpy + Pillow): py -3 pro_clothing.py REPO GAME_ROOT DLC_EXTRACTED SKATE2_ROOT SKATE1_ROOT ASSETS
Run after dlc_clothing.py and old_skate_clothing.py; backups in <set>/pre-pros/.
"""
import hashlib, json, re, shutil, struct, sys, tempfile
import xml.etree.ElementTree as ET
from pathlib import Path
import numpy as np
from PIL import Image

repo, game, dlc, s2game, s1game, assets = map(Path, sys.argv[1:7])
helpers = repo/'tools/pipeline/skate2port'  # s2big, s2badge, skate1, s1mesh, cas_tiles (rescued into the repo)
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/mixamo_to_skate'), str(helpers)]
from tools.owned_game import big
from tools.asset_pipeline import customisation_library, customiser_lighting, native_roster, retail_character
from tools.asset_pipeline import customiser_cache as cache
from tools.asset_pipeline.marquee_assets import Resources
from tools.asset_pipeline.optional_content import CONTENT_ERRORS
from glb import Document
from s2big import Big
from s2badge import badge, logo, stamp
import skate1, s1mesh, cas_tiles, fit_clothing

PREFIX = 's2mq'  # Skate 2's own ported pro items already start 's2pro_'
SKIN_TOP = {'wrist': 1.065, 'forearm': 1.191, 'bicept': 1.346, 'shoulder': 1.485}  # male CAS arms, max y (measured 10-02)
LEG_TOP = {'legs_tight': 0.217, 'knee': 0.606, 'thigh': 0.914}                    # male CAS legs, max y
DLC_IDS = {'creator_00000000.big': '15', 'dway_park_00000000.big': '10'}  # the packs' CAS items' cas.DlcID (checked 10-02, 10-30)
RENDER = 3  # bump to redraw every tile (cas_tiles.TEMPLATE_VERSION is in the marker too)
# fit_clothing's corrections for the Skate 2 pros' garments (offsets/weights only, SHA-checked against the unfitted points)
PRO_FIT = Path(__file__).resolve().with_name('pro_fit_corrections.npz')
PRO_FIT_VERSION = hashlib.sha256(PRO_FIT.read_bytes()).hexdigest()[:12]
BARE = 2  # bump to cut the gloves' bare-skin triangles again (reconvert them first: delete their library GLBs)

# recipe component -> CAS slot. Body (Arm, Leg, Hair, Rostral, Organ, Misc, Equipment) is left out.
SLOTS = {'Hat': 'Hat', 'Torso': 'OuterTorso', 'OuterTorso': 'OuterTorso', 'Pants': 'Pants', 'Feet': 'Feet',
         'WristItem': 'WristItem', 'Jewellery': 'Accessory', 'Accessory': 'Accessory', 'SkateBoard': 'SkateBoard',
         'Glasses': 'Glasses'}
# (character, component) -> (CAS slot, item noun): pieces whose recipe component is not what they are.
# Isaac's 'Misc' (the RIG suit's back plates) is not listed: it is part of his suit, not a CAS accessory (10-11).
# Gloves are the 'Arm' meshes of the characters whose hands are gloved (every marquee Arm diffuse checked, 10-30:
# only these two; Skate 1 and 2 have none. Isaac's RIG gauntlets stay part of his suit: cut out, their fingers do not
# line up with the skater's, 57 hand points through by up to 11 mm).
SPECIAL = {('isaak', 'Feet'): ('Hat', 'Helmet'), ('isaak', 'InnerTorso'): ('InnerTorso', 'RIG'),
           ('isaak', 'Accessory'): ('Jewellery', 'RIG Light'),
           ('deerman', 'Hair'): ('Hat', 'Balaclava'), ('terry_kennedy', 'Accessory'): ('Jewellery', 'Pendant'),
           ('shingo', 'Accessory'): ('WristItem', 'Bracelet'), ('dan_drehobl', 'Accessory'): ('Accessory', 'Cigarette'),
           ('deerman', 'Arm'): ('Accessory', 'Gloves'), ('danny_way_vert', 'Arm'): ('Accessory', 'Gloves'),
           ('s2_terry_kennedy', 'Accessory'): ('Jewellery', 'Pendant'), ('s2_chris_cole', 'Hat'): ('WristItem', 'Wristbands'),
           ('s2_chris_cole', 'WristItem'): ('Accessory', 'Wallet Chain'),
           ('s2_danny_way', 'InnerTorso'): ('InnerTorso', 'Undershirt'), ('s2_lucas_puig', 'InnerTorso'): ('InnerTorso', 'Undershirt'),
           ('s2_eric_koston', 'Torso'): ('InnerTorso', 'Undershirt')}  # Koston's 'Torso' is only the tee under his shirt
# layered outfits: the top's cas.InnerCutTypeRequired = its own InnerTorso's cas.InnerCutType (as Isaac's RIG)
INNER = {'isaak': 'isaac rig', 's2_danny_way': 's2 danny way', 's2_lucas_puig': 's2 lucas puig', 's2_eric_koston': 's2 eric koston'}
# Skate 2's pros (its marquee recipes; the cast, ambient and prop recipes are not pros)
S2_PROS = ('alex_chalmers', 'brayden_szafranski', 'chris_cole', 'chris_haslam', 'colin_mckay', 'danny_way', 'darren_navarette',
           'dennis_busenitz', 'eric_koston', 'jake_brown', 'jason_dill', 'jerry_hsu', 'john_cardiel', 'john_rattray',
           'lucas_puig', 'marc_johnson', 'mark_appleyard', 'mike_carroll', 'pat_duffy', 'pj_ladd', 'ray_barbee',
           'rob_dyrdek', 'ryan_gallant', 'ryan_smith', 'terry_kennedy')
# Skate 2 pros whose shoes Skate 2's own CAS already sells (its pro shoe models, ported by old_skate_clothing.py): the
# marquee pair is the same shoe (diffuse twin under 1%, or by eye on the tiles where the UV layout differs, 10-30)
S2_CAS_SHOES = {'brayden_szafranski', 'chris_cole', 'chris_haslam', 'colin_mckay', 'danny_way', 'darren_navarette',
                'dennis_busenitz', 'eric_koston', 'jason_dill', 'john_cardiel', 'john_rattray', 'lucas_puig', 'marc_johnson',
                'mark_appleyard', 'mike_carroll', 'pat_duffy', 'pj_ladd', 'ray_barbee', 'rob_dyrdek', 'terry_kennedy'}
COSTUME = {'dem_bones': {'Torso', 'Leg', 'Accessory', 'Equipment'}}  # bone body, not clothes
# Tops by eye from the drawn tiles (the meshes carry no garment type). Unlisted: by extent, see top_type().
TOPS = {'coach_frank': 'buttonshirt', 'colin_mckay': 'hoody', 'dan_drehobl': 'buttonshirt', 'danny_way': 'ls_tshirt',
        'eric_koston': 'buttonshirt', 'jerry_hsu': 'buttonshirt', 'john_cardiel': 'buttonshirt', 'lizard_king': 'ls_tshirt',
        'pat_duffy': 'ls_tshirt', 'pj_ladd': 'hoody', 'ryan_smith': 'buttonshirt', 'shingo': 'ls_tshirt',
        'skate_ambassador': 'buttonshirt',
        's2_colin_mckay': 'hoody', 's2_dennis_busenitz': 'sweater', 's2_eric_koston': 'buttonshirt', 's2_jerry_hsu': 'buttonshirt',
        's2_lucas_puig': 'buttonshirt', 's2_marc_johnson': 'jacket', 's2_pat_duffy': 'ls_tshirt', 's2_pj_ladd': 'hoody',
        's2_rob_dyrdek': 'hoody', 's2_ryan_smith': 'buttonshirt'}
# Hats by eye: (noun, RequiresHairModelType). Unlisted: by extent, see requirements().
HATS = {'joey_brezinski': ('Cap', 'hat'), 'dennis_busenitz': ('Cap', 'hat'), 'darren_navarette': ('Fedora', 'hat'),
        'reda': ('Flat Cap', 'hat'), 'slappy': ('Headband', 'none'), 's2_darren_navarette': ('Fedora', 'hat')}
TOP_NOUN = {'tshirt': 'Tee', 'ls_tshirt': 'Long-Sleeve Tee', 'tanktop': 'Tank Top', 'buttonshirt': 'Shirt',
            'hoody': 'Hoodie', 'jacket': 'Jacket', 'sweater': 'Sweater'}
# Accessory-slot noun -> cas.AccessoryType: the engine's Accessories tab (cas_menu ACCESSORY_TABS); rings carry none
ACCESSORY_TYPES = {'Backpack': 'backpack', 'Cigarette': 'other', 'Gloves': 'gloves', 'Wallet Chain': 'belt'}
NOUNS = {('isaak', 'Torso'): 'RIG Suit'}  # (character, component) -> noun, for names the rules below get wrong
BOARD = {'SkateBoard': 'Deck', 'SkateTruck': 'Trucks', 'SkateWheel': 'Wheels'}

DiscArchive = big.BigArchive
dlc_bigs = [DiscArchive(p) for p in sorted(dlc.glob('*/*.big'))]
s2cac = Big(s2game/'data/content/createacharacter.big')
s1cac = skate1.Archive(s1game)


class Entry:
    def __init__(self, path, read):
        self.path, self.reader = path, read


class Virtual:
    """Marquee meshes/textures under the createacharacter paths this script catalogs them at."""
    def __init__(self):
        self.entries, self.paths = [], set()

    def add(self, path, read):
        if path.lower() not in self.paths:
            self.paths.add(path.lower())
            self.entries.append(Entry(path, read))

    def read(self, entry):
        return entry.reader()


virtual = Virtual()


class LayeredArchive:
    """As old_skate_clothing.py (disc + Skate 3 DLC same-kind entries + Skate 1/2 CAC), plus the virtual entries."""
    def __init__(self, path):
        kind = '/' + Path(path).stem.lower() + '/'
        extra = [s2cac, s1cac, virtual] if Path(path).name.lower() == 'createacharacter.big' else []
        self.owner, by_path = {}, {}
        for archive in (DiscArchive(path), *dlc_bigs, *extra):
            for entry in archive.entries:
                p = entry.path.lower()
                if archive in dlc_bigs and kind not in p or archive in extra and not p.startswith('data/content/createacharacter/'):
                    continue
                if archive is virtual and p in by_path:
                    raise SystemExit('virtual path collides with an archive entry: ' + p)
                self.owner[id(entry)] = archive
                by_path[p] = entry
        self.entries = list(by_path.values())

    def read(self, entry):
        return self.owner[id(entry)].read(entry)


def open_archive(path):
    return LayeredArchive(path) if Path(path).name.lower() in ('createacharacter.big', 'marquee.big') else DiscArchive(path)


customisation_library.BigArchive = customiser_lighting.BigArchive = native_roster.BigArchive = open_archive
s1mesh.install(retail_character.RX2, retail_character)
skate3_material_data = customiser_lighting.material_data
ours = {}  # sha1 of a virtual model -> (character, component)


def shader_rows(shader, collections, specular):
    family, key = shader.split('.', 1)
    fields, _ = collections.resolve('material_' + family, key)
    rows = fields[customiser_lighting.key_hash('m_params')]['array']['items']
    return dict(shader=shader, params=[struct.unpack('>4f', bytes.fromhex(r)) for r in rows], specular=specular)


shaders_used = {}


def material_data(model, collections, parser, specular=None):
    shaders = {p.value for p in parser.parse_rx2(model).materials if p.kind == 'AttribulatorMaterialName'}
    name = next(iter(shaders)) if len(shaders) == 1 else ''
    if name and '.' not in name:  # Skate 1 shader names, as old_skate_clothing.py
        return shader_rows(skate1.SHADERS.get(name, 'character.cloth'), collections, specular)
    owner = ours.get(hashlib.sha1(model).hexdigest())
    if owner is None:
        return skate3_material_data(model, collections, parser, specular)
    # ponytail: a garment drawn with a skin shader (it samples the arm-skin atlas) gets the cloth one
    shader = 'character.default_cloth' if not name or 'skin' in name else name
    try:
        data = shader_rows(shader, collections, specular)
    except CONTENT_ERRORS:
        data = shader_rows('character.default_cloth', collections, specular)
    shaders_used[owner] = (name, data['shader'])
    return data


customiser_lighting.material_data = material_data

base = assets/'private/customisation'
directory = base/'sets'/json.loads((base/'current.json').read_text())['set']
backup = directory/'pre-pros'
if not (backup/'catalog.json').exists():
    backup.mkdir(exist_ok=True)
    for name in ('catalog.json', 'library-base.json', 'library-v3.json', 'native-lighting.json',
                 'catalog-complete.json', 'library-complete.json', 'lighting-complete.json'):
        shutil.copy2(directory/name, backup/name)
screen = assets/'private/create-skater'
manifest_path = screen/'runtime/createskater.json'
if not (screen/'runtime/createskater.json.pre-pros').exists():
    shutil.copy2(manifest_path, screen/'runtime/createskater.json.pre-pros')
manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
language = manifest['language']


def ours_by_name(name):
    tokens = name.split('_')
    return tokens[0] == PREFIX and all(t.startswith(PREFIX) for t in tokens)


def hid(*parts):
    return hashlib.sha1('|'.join(parts).encode()).hexdigest()[:16]


def vid(tag, ident):
    """A virtual file id for a marquee id: this script's tag (per source game) + a hash of the id."""
    return tag + hashlib.sha1(f'{tag}:{ident.lower()}'.encode()).hexdigest()[:12]


class S2Resources(Resources):
    """Skate 2 recipes also list LOD-less <mod> blocks (alternate material instances); the LOD's own decides."""
    def read(self, path):
        data = super().read(path)
        return re.sub(rb'<mod>\s*<matinst>.*?</matinst>\s*</mod>', b'', data, flags=re.S) if path.endswith('.xml') else data

    def recipe(self, name):
        # Mike's recipe lists an unused material whose diffuse is not on the disc; textures are checked per item
        return ET.fromstring(self.read('data/content/recipe/marquee/' + name + '.xml'))


s3_marquee = LayeredArchive(game/'data/content/marquee.big')
s3res = Resources(s3_marquee)
s2res = S2Resources(Big(s2game/'data/content/marquee.big'))
rows = json.loads((directory/'database/collections.json').read_text())['collections']
work = Path(tempfile.mkdtemp(prefix='pro-clothing-'))


def mesh_points(data):
    path = work/'mesh.rx2'
    path.write_bytes(data)
    parsed = retail_character.RX2.parse_rx2(str(path))
    return np.concatenate([np.asarray(m['positions']) for m in parsed['meshes'] if m.get('positions') and m.get('indices')])


def components(res, recipe):
    """(component, mod name, arena id, material element, all material elements) of a recipe's LOD 0 models."""
    root = res.recipe(recipe)
    mats = {m.attrib['id']: m for m in root.findall('mat')}
    for comp in root.findall('comp'):
        mod = comp.find('mod')
        lod = next((l for l in mod.findall('lod') if l.get('idx') == '0'), None)
        if lod is None:  # Skate 2 lists a few components with no LOD 0 (Jake Brown's shoes): not drawn in Skate 2 either
            continue
        variants = lod.findall('matinst/matvar')
        yield comp.attrib['n'], mod.attrib['n'], lod.attrib['arenaid'], [mats[v.attrib['id']] for v in variants]


# ---- what each character wears -------------------------------------------------------------
def dlc_of(recipe):
    pack = s3_marquee.owner[id(s3res.resolve(f'data/content/recipe/marquee/{recipe}.xml'))]
    return DLC_IDS[pack.path.name.lower()] if pack in dlc_bigs else ''


characters = []
for row in native_roster.roster(rows):
    fields = next(r for r in rows if r['class'] == 'characters_marquee' and r['key'] == row['key'])['fields']
    string = fields.get('Hash_174B301910A902C9', {}).get('data', '')
    name = language.get(string, '').strip() or row['name']
    characters.append(dict(key=row['key'], recipe=row['recipe'], name=name, game='s3', res=s3res, tag='fa30', dlc=dlc_of(row['recipe'])))
# Danny Way's vert outfit (Danny Way pack, not a roster character): his gloves only.
characters.append(dict(key='danny_way_vert', recipe='danny_way_vert', name='Danny Way', game='s3', res=s3res, tag='fa30',
                       dlc=dlc_of('danny_way_vert'), only={'Arm'}))
# Skate 2's Mike (the photographer): his backpack only.
characters.append(dict(key='mike', recipe='mike', name='Mike', game='s2', res=s2res, tag='fa20', dlc='', only={'Accessory'}))
# Skate 2's pros, keyed apart from the Skate 3 pros of the same name (ids, names and the rules above), named as Skate 3 names them.
s3_names = {ch['recipe']: ch['name'] for ch in characters}
for recipe in S2_PROS:
    characters.append(dict(key='s2_' + recipe, recipe=recipe, name=s3_names.get(recipe, recipe.replace('_', ' ').title()),
                           game='s2', res=s2res, tag='fa20', dlc=''))

items, coverage = [], {}
for ch in characters:
    report = coverage.setdefault(ch['name'], dict(added=[], skipped=[]))
    for comp, mod, arena, materials in components(ch['res'], ch['recipe']):
        if ch.get('only') and comp not in ch['only']:
            continue
        slot, noun = SPECIAL.get((ch['key'], comp), (SLOTS.get(comp), None))
        if ch['key'] == 'mike':
            slot, noun = 'Accessory', 'Backpack'
        if slot is None or comp in COSTUME.get(ch['key'], ()):
            if comp not in ('Rostral', 'Organ', 'Misc'):
                report['skipped'].append(f'{comp} {mod}: ' + ('hair (not a CAS hair style)' if comp == 'Hair' else 'body'))
            continue
        if len(materials) != 1:
            report['skipped'].append(f'{comp} {mod}: {len(materials)} materials')
            continue
        if ch['game'] == 's2' and comp == 'Feet' and ch['recipe'] in S2_CAS_SHOES:
            report['skipped'].append(f'{comp} {mod}: already in Skate 2\'s own Create-a-Skater (old_skate_clothing.py)')
            continue
        res, recipe = ch['res'], ch['recipe']
        model_path = f'data/content/marquee/model/{recipe}/{comp}/{arena}.rx2'
        data = res.read(model_path)
        textures = []
        for sp in materials[0].findall('sp'):
            source = f"data/content/marquee/texture/{sp.attrib['id'].lower()}.rx2"
            try:
                res.resolve(source)
            except CONTENT_ERRORS:
                continue
            tid = vid(ch['tag'], sp.attrib['id'])
            virtual.add(f'data/content/createacharacter/texture/0x{tid}.rx2', lambda r=res, s=source: r.read(s))
            textures.append(dict(channel=sp.attrib['chn'], id=tid, path=f'data/content/createacharacter/texture/0x{tid}.rx2', in_archive=True))
        if not any(t['channel'] == 'diffuse' for t in textures):
            report['skipped'].append(f'{comp} {mod}: diffuse texture not on the disc')
            continue
        path = f'data/content/createacharacter/model/cas_db/{slot}/0x{vid(ch["tag"], arena)}.rx2'
        virtual.add(path, lambda r=res, s=model_path: r.read(s))
        ours[hashlib.sha1(data).hexdigest()] = (ch['key'], comp)
        points = mesh_points(data)
        for part_slot in (('SkateBoard', 'SkateTruck', 'SkateWheel') if slot == 'SkateBoard' else (slot,)):
            items.append(dict(ch=ch, comp=comp, mod=mod, slot=part_slot, noun=noun, path=path, arena=arena,
                              material=materials[0], textures=textures, lo=points.min(0), hi=points.max(0)))

# ---- requirements and names ----------------------------------------------------------------
parts_of = {}  # (character, component) -> points of the body parts the garment was made over
for ch in characters:
    for comp, mod, arena, materials in components(ch['res'], ch['recipe']):
        if comp in ('Arm', 'Leg') and ch.get('only') is None:
            parts_of[(ch['key'], comp)] = mesh_points(ch['res'].read(f"data/content/marquee/model/{ch['recipe']}/{comp}/{arena}.rx2"))


def cover(tops, height):
    """The shortest CAS body part reaching the pro's own (so no gap shows between garment and skin)."""
    return min(((top, name) for name, top in tops.items() if top >= height - 0.012), default=(0, ''))[1]


def top_type(item):
    if item['ch']['key'] in TOPS:
        return TOPS[item['ch']['key']]
    lo, hi = item['lo'], item['hi']
    # ponytail: extent heuristic (half-width = sleeve length, height = hood up, depth = hood down);
    # tiles checked by eye and exceptions listed in TOPS
    if hi[0] < 0.25:
        return 'tanktop'
    if hi[0] < 0.42:
        return 'tshirt'
    return 'hoody' if hi[1] > 1.7 or lo[2] < -0.2 else 'jacket'


def requirements(item):
    """Synthesised cas.* flags: the body parts, hair and items the garment needs or hides."""
    key, slot, lo, hi = item['ch']['key'], item['slot'], item['lo'], item['hi']
    flags = {'cas.Gender': 'unisex' if slot in ('Feet', 'SkateBoard', 'SkateTruck', 'SkateWheel') else 'male'}
    if slot == 'OuterTorso':
        arm = parts_of.get((key, 'Arm'))
        flags['cas.TopType'] = top = top_type(item)
        flags['cas.ArmModelRequired'] = cover(SKIN_TOP, arm[:, 1].max()) if arm is not None else ''
        flags['cas.InnerCutTypeRequired'] = INNER.get(key, '')
        flags['cas.IsNecklaceRemoved'] = 'yes' if top in ('hoody', 'jacket', 'sweater', 'buttonshirt') and key != 'isaak' else ''
        if hi[1] > 1.7:  # hood up, as Skate 3's "hood up no hat" hoodies
            flags['cas.RequiresHairModelType'], flags['cas.RequiresRemovalOfComponents'] = 'none', 'Hat,Hair'
    elif slot == 'InnerTorso':
        flags['cas.InnerCutType'], flags['cas.IsLayeredType'] = INNER[key], 'True'
    elif slot == 'Pants':
        leg = parts_of.get((key, 'Leg'))
        flags['cas.LegModelRequired'] = cover(LEG_TOP, leg[:, 1].max()) if leg is not None else ''
        if key == 'isaak':  # the RIG legs include the boots
            flags['cas.RequiresRemovalOfComponents'] = 'Feet,Sock'
    elif slot == 'Hat':
        if item['noun'] in ('Helmet', 'Balaclava'):
            flags['cas.RequiresHairModelType'], flags['cas.RequiresRemovalOfComponents'] = 'none', 'Glasses'
        elif key in HATS:
            flags['cas.RequiresHairModelType'] = HATS[key][1]
        elif 'headband' in item['mod'].lower():
            flags['cas.RequiresHairModelType'] = 'full'
        else:  # a brim (front or back) is a cap over "hat" hair; brimless is a beanie that hides the hair
            flags['cas.RequiresHairModelType'] = 'hat' if hi[2] > 0.14 or lo[2] < -0.15 else 'none'
    elif slot == 'Accessory':  # retail's Accessory items are rings; ported others get their own tab
        flags['cas.AccessoryType'] = ACCESSORY_TYPES.get(item['noun'], '')
    return flags


def noun(item):
    key, slot, mod = item['ch']['key'], item['slot'], item['mod'].lower()
    if (key, item['comp']) in NOUNS:
        return NOUNS[(key, item['comp'])]
    if slot in BOARD:
        return BOARD[slot]
    if item['noun']:
        return item['noun']
    if slot == 'OuterTorso':
        return 'Polo Shirt' if 'polo' in mod else 'Jersey' if 'jersey' in mod else TOP_NOUN[top_type(item)]
    if slot == 'Hat':
        if key in HATS:
            return HATS[key][0]
        return 'Headband' if 'headband' in mod else 'Cap' if requirements(item)['cas.RequiresHairModelType'] == 'hat' else 'Beanie'
    if slot == 'Pants':
        return 'Shorts' if (key, 'Leg') in parts_of and 'jeans' not in mod else 'Jeans' if 'jeans' in mod else 'Pants'
    if slot == 'Feet':
        return 'Shoes'
    if slot == 'WristItem':
        return 'Watch' if 'watch' in mod else 'Leather Wristband' if 'leather' in mod else 'Wristbands' if mod.endswith('s') else 'Wristband'
    if slot == 'Accessory':
        return 'Ring' if 'ring' in mod else mod.replace('_', ' ').title()
    return mod.replace('_', ' ').title()


catalog = json.loads((directory/'catalog.json').read_text())
previous_materials = set()
for component in catalog['components']:
    previous_materials |= {v['id'] for m in component['models'] if ours_by_name(m['name'])
                           for l in m['lods'] for g in l['material_instances'] for v in g}
    component['models'] = [m for m in component['models'] if not ours_by_name(m['name'])]
for mid in previous_materials:
    catalog['materials'].pop(mid, None)
existing_models = {m['id'] for c in catalog['components'] for m in c['models']}
blank_model = {k: '' for c in catalog['components'] for m in c['models'] for k in m['flags']}
blank_material = {k: '' for m in catalog['materials'].values() for k in m['flags']}

added = []
for item in items:
    ch = item['ch']
    item['id'] = hid('pro-model', ch['game'], ch['key'], item['comp'], item['slot'])
    item['mid'] = hid('pro-material', ch['game'], ch['key'], item['comp'], item['slot'], item['material'].attrib['id'])
    # ours from an earlier run: a rerun of old-clothing alone drops our models (origin() takes every s2 name) but not
    # their materials, so the name sweep above misses them (10-08)
    catalog['materials'].pop(item['mid'], None)
    assert item['id'] not in existing_models, item['id']
    item['label'] = f"{ch['name']}'s {noun(item)}" if ch['game'] == 's2' else f"{ch['name']} {noun(item)}"
    tokens = ['_'.join([ch['key'], item['comp'], item['slot']]).lower().split('_')][0]
    item['name'] = '_'.join([PREFIX] + [PREFIX + t for t in tokens if t])
    item['variant'] = PREFIX + 'original'
    material = {'shader': item['material'].attrib.get('type', ''), 'textures': item['textures'],
                'flags': {**blank_material, 'cas.DlcID': ch['dlc']}}
    catalog['materials'][item['mid']] = material
    model = {'id': item['id'], 'name': item['name'], 'flags': {**blank_model, **requirements(item)},
             'lods': [{'index': 0, 'arena_asset_id': item['arena'].removeprefix('0x'), 'model_id': item['path'].rsplit('0x', 1)[1][:-4],
                       'path': item['path'], 'in_archive': True,
                       'material_instances': [[{'id': item['mid'], 'name': item['variant']}]]}]}
    component = next((c for c in catalog['components'] if c['slot'] == item['slot']), None)
    if component is None:
        component = {'slot': item['slot'], 'models': []}
        catalog['components'].append(component)
    component['models'].append(model)
    added.append(item)
(directory/'catalog.json').write_text(json.dumps(catalog, indent=2) + '\n', encoding='utf-8')
print('catalogued', len(added), 'pro items')

# ---- convert ---------------------------------------------------------------------------------
# Accessory items keep their authored skin weights (the fit below skips them): reconvert any an earlier run fitted.
for item in added:
    glb = directory/'library'/f"{item['id']}_v4.glb"
    if item['slot'] == 'Accessory' and glb.exists() and fit_clothing.fitted_already(glb):
        glb.unlink()
library = customisation_library.prepare(dict(assets=str(assets), game_root=str(game), directory=str(directory),
                                             library_index='library-base.json'))
mine = {i['id'] for i in added}
failed = [e for e in library['errors'] if e.get('model') in mine]
if failed:
    print('UNAVAILABLE', failed)


def welded_islands(tris, pos):
    """Island id per triangle: triangles joined through shared (welded) vertex positions."""
    weld = np.unique(pos.round(4), axis=0, return_inverse=True)[1].reshape(-1)
    parent = list(range(int(weld.max()) + 1))

    def find(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i
    for a, b, c in weld[tris]:
        root = find(a)
        parent[find(b)] = root
        parent[find(c)] = root
    return np.array([find(v) for v in weld[tris[:, 0]]])


def append_accessor(doc, data, array, kind, component, target):
    data.extend(b'\0' * (-len(data) % 4))
    doc.doc['bufferViews'].append({'buffer': 0, 'byteOffset': len(data), 'byteLength': array.nbytes, 'target': target})
    data.extend(array.tobytes())
    doc.doc['accessors'].append({'bufferView': len(doc.doc['bufferViews']) - 1, 'componentType': component,
                                 'count': len(array), 'type': kind})
    return len(doc.doc['accessors']) - 1


def save_glb(doc, data, path):
    data.extend(b'\0' * (-len(data) % 4))
    doc.doc['buffers'] = [{'byteLength': len(data)}]
    text = json.dumps(doc.doc, separators=(',', ':')).encode()
    text += b' ' * (-len(text) % 4)
    path.write_bytes(struct.pack('<III', 0x46546c67, 2, 28 + len(text) + len(data)) + struct.pack('<I4s', len(text), b'JSON') + text
                     + struct.pack('<I4s', len(data), b'BIN\0') + bytes(data))


def keep_part(path, slot):
    """Cut a whole marquee board GLB down to its deck, trucks or wheels (by each vertex's main joint;
    small deck-joint islands under the deck are the truck baseplates and risers). Returns the triangles kept."""
    doc = Document(path)
    prims = doc.doc['meshes'][0]['primitives']  # a deck's grip is a second one on the same vertices (character_glb.py)
    if doc.doc.get('extras', {}).get('pro_part') == slot:
        return sum(doc.doc['accessors'][p['indices']]['count'] for p in prims) // 3
    a = prims[0]['attributes']
    skin = doc.doc['skins'][0]
    names = [doc.doc['nodes'][j].get('name', '').upper() for j in skin['joints']]
    pos = doc.accessor(a['POSITION'])
    joints, weights = doc.accessor(a['JOINTS_0']).astype(int), doc.accessor(a['WEIGHTS_0']).astype(np.float32)
    pieces = [doc.accessor(p['indices']).reshape(-1, 3) for p in prims]
    tris = np.concatenate(pieces)
    island = welded_islands(tris, pos)
    box = {k: (pos[tris[island == k]].reshape(-1, 3).min(0), pos[tris[island == k]].reshape(-1, 3).max(0)) for k in set(island)}
    data = bytearray(doc.data)
    append = lambda array, kind, component, target: append_accessor(doc, data, array, kind, component, target)
    main = joints[np.arange(len(joints)), weights.argmax(1)]
    if not any('WHEEL' in names[j] or 'TRUCK' in names[j] for j in set(main)):
        # ponytail: a rigid board (Deerman's: every vertex on the root) gets its wheel and truck islands
        # rigged to the nearest wheel/truck bone, so they spin and steer like the retail parts
        bind = np.linalg.inv(doc.accessor(skin['inverseBindMatrices']).reshape(-1, 4, 4))[:, 3, :3]
        for k, (lo, hi) in box.items():
            wheel = (lo[0] > 0.07 or hi[0] < -0.07) and hi[1] < 0.07 and 0.2 < min(abs(lo[2]), abs(hi[2])) and max(abs(lo[2]), abs(hi[2])) < 0.29
            if not wheel and not (hi[1] < 0.09 and hi[2] - lo[2] < 0.5):
                continue
            bones = [j for j, n in enumerate(names) if ('WHEEL' if wheel else 'TRUCK') in n]
            bone = min(bones, key=lambda j: float(np.linalg.norm(bind[j] - (lo + hi) / 2)))
            verts = np.unique(tris[island == k])
            joints[verts] = [bone, 0, 0, 0]
            weights[verts] = [1, 0, 0, 0]
        rig = dict(JOINTS_0=append(joints.astype('<u2'), 'VEC4', 5123, 34962), WEIGHTS_0=append(weights.astype('<f4'), 'VEC4', 5126, 34962))
        for p in prims:
            p['attributes'].update(rig)
        main = joints[:, 0]
    main = np.array([names[j] for j in main])
    group = np.where(np.char.find(main, 'WHEEL') >= 0, 'SkateWheel', np.where(np.char.find(main, 'TRUCK') >= 0, 'SkateTruck', 'SkateBoard'))
    votes = group[tris]
    tri_group = np.where(votes[:, 1] == votes[:, 2], votes[:, 1], votes[:, 0])
    deck = tri_group == 'SkateBoard'
    if any(hi[2] - lo[2] > 0.5 for k, (lo, hi) in box.items() if (deck & (island == k)).any()):
        small = np.array([box[k][1][2] - box[k][0][2] < 0.5 and box[k][0][1] < 0.079 for k in island])
        tri_group[deck & small] = 'SkateTruck'
    else:  # ponytail: no one island spans the deck (Isaac's board): what hangs below the deck's underside
        tri_group[deck & (pos[tris][:, :, 1].mean(1) < 0.079)] = 'SkateTruck'
    keep, start, kept = tri_group == slot, 0, []
    for p, piece in zip(prims, pieces):
        mine = piece[keep[start:start + len(piece)]].astype('<u4').reshape(-1)
        start += len(piece)
        if mine.size:
            p['indices'] = append(mine, 'SCALAR', 5125, 34963)
            kept.append(p)
    prims[:] = kept or prims[:1]
    if not kept:
        prims[0]['indices'] = append(np.zeros(0, '<u4'), 'SCALAR', 5125, 34963)
    doc.doc.setdefault('extras', {})['pro_part'] = slot
    save_glb(doc, data, path)
    return int(keep.sum())


FIT = 1  # bump to refit the tops (reconvert them first: delete their library GLBs)
rostral = library['models'][library['defaults']['male']['selections']['Rostral']['asset_id']]
head_doc = Document(assets/rostral['scene'])
head_attributes = head_doc.doc['meshes'][0]['primitives'][0]['attributes']
head_pos, head_nrm = head_doc.accessor(head_attributes['POSITION']).astype(float), head_doc.accessor(head_attributes['NORMAL']).astype(float)
bib = head_pos[:, 1] < 1.52  # the CAS head's neck and the chest yoke under the collar
head_skin_pos, head_skin_nrm = head_pos[bib], head_nrm[bib] / np.linalg.norm(head_nrm[bib], axis=1, keepdims=True)


def hand_skin():
    """Points and unit normals of every male CAS arm part's hands (the skater's rig has no finger bones, so a hand is
    rigid on its HAND joint and a bind-pose fit holds in every pose)."""
    points, normals = [], []
    for part in library['models'].values():
        if part['slot'] != 'Arm' or part['flags'].get('Gender') not in ('male', 'unisex'):
            continue
        doc = Document(assets/part['scene'])
        a = doc.doc['meshes'][0]['primitives'][0]['attributes']
        names = [doc.doc['nodes'][j].get('name', '') for j in doc.doc['skins'][0]['joints']]
        joints, weights = doc.accessor(a['JOINTS_0']).astype(int), doc.accessor(a['WEIGHTS_0'])
        hand = np.array(['HAND' in names[j] for j in joints[np.arange(len(joints)), weights.argmax(1)]])
        points.append(doc.accessor(a['POSITION']).astype(float)[hand])
        normals.append(doc.accessor(a['NORMAL']).astype(float)[hand])
    points, normals = np.concatenate(points), np.concatenate(normals)
    return points, normals / np.linalg.norm(normals, axis=1, keepdims=True)


def fit_neck(path, margin=0.006, reach=0.06, spread=0.03, skin=None, above=1.25):
    """Move a top's collar region out of the CAS head's neck and chest yoke. The marquee tops were made over
    each pro's own head, whose yoke is smaller, so the CAS one showed through the collar as a ragged skin patch.
    skin=(points, unit normals) fits another body surface (the gloves: the CAS hands), `above` the lowest y moved."""
    head_pos, head_nrm = skin or (head_skin_pos, head_skin_nrm)
    doc = Document(path)
    if doc.doc.get('extras', {}).get('pro_fit') == FIT:
        return
    a = doc.doc['meshes'][0]['primitives'][0]['attributes']
    v = doc.accessor(a['POSITION']).astype(float)
    weld = np.unique(v.round(4), axis=0, return_inverse=True)[1].reshape(-1)  # seam copies move together
    pos = np.zeros((weld.max() + 1, 3))
    pos[weld] = v
    sel = np.where(pos[:, 1] > above)[0]
    pairwise = lambda p, q: np.sqrt(np.maximum((p * p).sum(1)[:, None] + (q * q).sum(1)[None] - 2 * p @ q.T, 0))
    dist = pairwise(pos[sel], head_pos)
    near = dist.argmin(1)
    inside = ((pos[sel] - head_pos[near]) * head_nrm[near]).sum(1)  # < 0: behind the nearest head surface
    need = np.where((dist[np.arange(len(sel)), near] < reach) & (inside < margin), margin - inside, 0)
    # ponytail: each vertex takes its neighbours' largest need with a linear falloff (a smooth bulge, not spikes);
    # fitted to the default head at zero body weight (these meshes have no fat/thin morphs)
    falloff = np.clip(1 - pairwise(pos[sel], pos[sel]) / spread, 0, 1) * need[None, :]
    best = falloff.argmax(1)
    pos[sel] += head_nrm[near[best]] * falloff.max(1)[:, None]
    data = bytearray(doc.data)
    moved = pos[weld].astype('<f4')
    a['POSITION'] = append_accessor(doc, data, moved, 'VEC3', 5126, 34962)
    doc.doc['accessors'][a['POSITION']].update(min=moved.min(0).tolist(), max=moved.max(0).tolist())
    doc.doc.setdefault('extras', {})['pro_fit'] = FIT
    save_glb(doc, data, path)


LIPS = 1  # bump to move the cigarette again (reconvert it first: delete its library GLB)
cas_face = retail_character.RX2.parse_rx2(str(next((directory/'library/reference/Rostral').glob('*.rx2'))))
cas_bind = {b['name']: np.array(b['bind_matrix']).reshape(4, 4) for b in cas_face['bones'] if b.get('bind_matrix') is not None}


def onto_cas_lips(item, path):
    """Dan's cigarette is skinned to his lip bones, which the skater's 35-joint rig lacks, so it converts onto HEAD
    where Dan's lips are (8 mm above and 19 mm in front of the CAS head's). Each point follows its lip bones from
    Dan's bind to the CAS reference head's (the same bone names), so it sits on the CAS lips."""
    doc = Document(path)
    if doc.doc.get('extras', {}).get('pro_lips') == LIPS:
        return
    source = work/'lips.rx2'
    source.write_bytes(item['ch']['res'].read(f"data/content/marquee/model/{item['ch']['recipe']}/{item['comp']}/{item['arena']}.rx2"))
    parsed = retail_character.RX2.parse_rx2(str(source))
    bind = {b['index']: (b['name'], np.array(b['bind_matrix']).reshape(4, 4)) for b in parsed['bones'] if b.get('bind_matrix') is not None}
    mesh = next(m for m in parsed['meshes'] if m.get('positions') and m.get('indices'))
    a = doc.doc['meshes'][0]['primitives'][0]['attributes']
    pos = doc.accessor(a['POSITION']).astype(float)
    assert np.allclose(pos, mesh['positions'], atol=1e-5), path  # the conversion keeps the disc's points, in order
    moved = np.zeros_like(pos)
    for i, v in enumerate(np.c_[pos, np.ones(len(pos))]):
        skin = [(bind[mesh['used_bone_indices'][int(p)]], w) for p, w in zip(mesh['skin_indices'][i], mesh['skin_weights'][i]) if w > 1e-4]
        total = sum(w for _, w in skin)
        for (name, b), w in skin:
            moved[i] += w / total * (v @ np.linalg.inv(b) @ cas_bind[name])[:3]
    # ponytail: normals kept (the lip binds differ by a translation; rotations by < 0.002)
    data = bytearray(doc.data)
    moved = moved.astype('<f4')
    a['POSITION'] = append_accessor(doc, data, moved, 'VEC3', 5126, 34962)
    doc.doc['accessors'][a['POSITION']].update(min=moved.min(0).tolist(), max=moved.max(0).tolist())
    doc.doc.setdefault('extras', {})['pro_lips'] = LIPS
    save_glb(doc, data, path)


def bare_skin(rgb):
    """Texels of a character's own skin, shaded or not (warm, not near black), not grey leather, cloth or white trim."""
    return (rgb.mean(-1) > 60) & (rgb[..., 0] - rgb[..., 2] > 30)


def cut_bare_skin(item, path):
    """Drop a glove's triangles textured with its character's own skin (Deer Man's fingerless gloves: the fingers and
    the opening on the back), so the skater's own hands show there in the skater's own skin tone."""
    doc = Document(path)
    if doc.doc.get('extras', {}).get('pro_bare') == BARE:
        return
    prims = doc.doc['meshes'][0]['primitives']
    uv = doc.accessor(prims[0]['attributes']['TEXCOORD_0'])
    image = np.asarray(Image.open(assets/library['materials'][item['mid']]['diffuse']).convert('RGB')).astype(float)
    h, w = image.shape[:2]
    data = bytearray(doc.data)
    for p in prims:
        tris = doc.accessor(p['indices']).reshape(-1, 3)
        # most of 10 texels inside the triangle (not its corners: the back opening's wide triangles end on its leather rim)
        at = (SAMPLES[None, :, :, None] * uv[tris][:, None]).sum(2) % 1
        skin = bare_skin(image[np.minimum((at[..., 1] * h).astype(int), h - 1), np.minimum((at[..., 0] * w).astype(int), w - 1)])
        p['indices'] = append_accessor(doc, data, tris[skin.mean(1) < 0.5].astype('<u4').reshape(-1), 'SCALAR', 5125, 34963)
    doc.doc.setdefault('extras', {})['pro_bare'] = BARE
    save_glb(doc, data, path)


SAMPLES = np.array([[1, 1, 1], [8, 1, 1], [1, 8, 1], [1, 1, 8], [4, 4, 2], [4, 2, 4], [2, 4, 4], [6, 3, 1], [1, 6, 3], [3, 1, 6]])
SAMPLES = SAMPLES / SAMPLES.sum(1, keepdims=True)  # barycentric points inside a triangle
assert bare_skin(np.array([[200, 150, 120], [120, 70, 50], [40, 40, 42], [230, 230, 230], [90, 85, 80], [50, 20, 10]])).tolist() ==     [True, True, False, False, False, False]
empty = set()  # board parts a pro's board does not have (no triangles left)
hands = hand_skin() if any(i['noun'] == 'Gloves' for i in added) else None
for item in added:
    if item['id'] not in library['models']:
        continue
    if item['slot'] in BOARD and not keep_part(assets/library['models'][item['id']]['scene'], item['slot']):
        empty.add(item['id'])
    if item['slot'] in ('OuterTorso', 'InnerTorso'):
        fit_neck(assets/library['models'][item['id']]['scene'])
    if item['noun'] == 'Cigarette':
        onto_cas_lips(item, assets/library['models'][item['id']]['scene'])
    if item['noun'] == 'Gloves':  # made as the character's hands, so the CAS hands sat level with them (z-fighting, up to 4 mm out)
        fit_neck(assets/library['models'][item['id']]['scene'], margin=0.0025, reach=0.02, spread=0.01, skin=hands, above=-1)
        cut_bare_skin(item, assets/library['models'][item['id']]['scene'])
# the 10-04 skin-through-clothes fit (made over the collars fit_neck moves), before any tile is drawn from these models
# (not the Accessory slot: see fit_clothing.apply). The Skate 2 pros' garments came later: the same fit, run 10-30 over
# them alone, is kept in its own file so the 10-04 one and every tile marked with its VERSION stay as they are.
fitted = fit_clothing.apply(assets, library)
fit_clothing.DATA, data_1004 = PRO_FIT, fit_clothing.DATA
pro_fitted = fit_clothing.apply(assets, library)
fit_clothing.DATA = data_1004
print('fitted', len(fitted), '+', len(pro_fitted), 'models')

# ---- Edit Skater tiles -------------------------------------------------------------------------
thumb_root = screen/'thumbs/assets/data/fe/cas'
previous_keys = [k for k in manifest['thumbnails'] if ours_by_name(k.rsplit('/', 1)[-1].rsplit('_', 1)[0])]
manifest['thumbnails'] = {k: v for k, v in manifest['thumbnails'].items() if k not in previous_keys}
manifest['language'] = language = {k: v for k, v in language.items()
                                   if not (k.startswith('ID_CAC_PART_') and ours_by_name(k[len('ID_CAC_PART_'):].lower().rsplit('_', 1)[0]))}
tile_camera = cas_tiles.tile_camera


def one_glove_camera(folder, points, back=False):
    """cas_tiles.tile_camera, but an item on both hands (gloves; retail has no glove tile) is framed on its left one, the
    hand the ring tiles show, by the ring tiles' framed rule turned onto the back of the hand (framing both, a body
    apart, drew each glove a few pixels wide)."""
    if folder == 'male/accessory' and points[:, 0].min() < -0.3 and points[:, 0].max() > 0.3:  # (tops span both arms too)
        rule = dict(cas_tiles.TEMPLATES['male/accessory']['framed'], yaw=250, pitch=25, roll=70, span=0.75)
        return cas_tiles.framed_cam(points[points[:, 0] > 0], rule), {}
    return tile_camera(folder, points, back)


cas_tiles.tile_camera = one_glove_camera


def draw(item):
    """The item's tile (RGBA 128x128) on the Skate 3 tile template (cas_tiles): clothing on the mannequin, a deck,
    truck or wheel on the assembled board (retail's board-part tiles, round 2)."""
    folder = tile_key(item)[1].rsplit('/', 1)[0]
    return dict(cas_tiles.render_model(assets, library, folder, item['id'], [item['mid']]))[item['mid']]


def tile_key(item):
    stem = f"{item['name']}_{item['variant']}".lower()
    return stem, (item['slot'].lower() if item['slot'] in BOARD else f"male/{item['slot'].lower()}") + '/' + stem


mark = badge(logo(s2game))
images, drawn = {}, 0
for item in added:
    if item['id'] not in library['models'] or item['id'] in empty or item['slot'] == 'InnerTorso':
        continue
    stem, key = tile_key(item)
    folder = thumb_root/key
    flags = library['models'][item['id']]['flags']
    marker = f"{RENDER}.{FIT}.{cas_tiles.TEMPLATE_VERSION}:{flags.get('ArmModelRequired', '')}:{flags.get('LegModelRequired', '')}:{flags.get('RequiresHairModelType', '')}"
    marker += f":fit {fit_clothing.VERSION}" if item['id'] in fitted else f":fit {PRO_FIT_VERSION}" if item['id'] in pro_fitted else ''
    marker += f":bare {BARE}" if item['noun'] == 'Gloves' else ''
    marker += f":lips {LIPS}" if item['noun'] == 'Cigarette' else ''
    saved = folder/'manifest.json'
    rgba = folder/f'0000_{stem}.Texture.rgba'
    if saved.exists() and json.loads(saved.read_text()).get('pro_render') == marker and rgba.exists():
        images[item['id']] = np.frombuffer(rgba.read_bytes(), np.uint8).reshape(128, 128, 4)
        continue
    image = draw(item)
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    rgba.write_bytes(image.tobytes())
    if item['ch']['game'] == 's2':
        image = stamp(rgba, 128, 128, mark)
    image.save(rgba.with_suffix('.png'))
    saved.write_text(json.dumps({'textures': [{'rgba_file': rgba.name, 'width': 128, 'height': 128}], 'pro_render': marker,
                                 **({'skate2_badge': 1} if item['ch']['game'] == 's2' else {})}))
    images[item['id']] = np.asarray(image)
    drawn += 1
print('tiles drawn', drawn)

# Trucks and wheels many pros share (same look on a different atlas) are listed once.
shared = {}
for slot in BOARD:
    kept = []
    for item in (i for i in added if i['slot'] == slot and i['id'] in images):
        a = images[item['id']].astype(np.int16)
        twin = next((k for k in kept if float((np.abs(a - images[k['id']].astype(np.int16)).max(axis=2) > 40).mean()) < .01), None)
        if twin:
            shared.setdefault(twin['id'], [twin]).append(item)
        else:
            kept.append(item)
dropped = {i['id'] for group in shared.values() for i in group[1:]} | empty
for component in catalog['components']:
    component['models'] = [m for m in component['models'] if m['id'] not in dropped]
for item in added:
    if item['id'] in dropped:
        catalog['materials'].pop(item['mid'], None)
        (assets/library['models'].pop(item['id'])['scene']).unlink(missing_ok=True)
        library['materials'].pop(item['mid'], None)
added = [i for i in added if i['id'] not in dropped]
(directory/'catalog.json').write_text(json.dumps(catalog, indent=2) + '\n', encoding='utf-8')

for item in added:
    if item['id'] in library['models']:
        # the label is also the board grid's grouping key (customiser.rs wear_pair): unique per item
        library['models'][item['id']]['name'] = item['label']
        library['materials'][item['mid']]['name'] = item['label']
(directory/'library-base.json').write_text(json.dumps(library, separators=(',', ':')))
customiser_lighting.prepare(game, assets, directory, library)

for name in ('catalog', 'library', 'lighting'):
    marker = directory/(name+'-complete.json')
    saved = json.loads(marker.read_text())
    paths = []
    for relative in cache.OUTPUTS[name]:
        path = directory/relative
        paths.extend(path.rglob('*') if path.is_dir() else [path])
    saved['files'] = cache.receipt(directory, paths, marker)  # unchanged files keep the marker's hashes
    marker.write_text(json.dumps(saved))

registered = set()
for item in added:
    if item['id'] not in library['models'] or item['slot'] == 'InnerTorso':
        continue
    stem, key = tile_key(item)
    manifest['thumbnails'][key] = {'rgba': (thumb_root/key/f'0000_{stem}.Texture.rgba').relative_to(screen).as_posix(),
                                   'width': 128, 'height': 128}
    language['ID_CAC_PART_' + stem.upper()] = item['label']
    registered.add(key)
manifest_path.write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
for item in (i for i in items if i['id'] in dropped):
    shutil.rmtree(thumb_root/tile_key(item)[1], ignore_errors=True)
for key in previous_keys:
    if key not in registered:
        shutil.rmtree(thumb_root/key, ignore_errors=True)
shutil.rmtree(work, ignore_errors=True)

# ---- report ----------------------------------------------------------------------------------
for item in items:
    report = coverage[item['ch']['name']]
    if item['id'] in empty:
        report['skipped'].append(f"{BOARD[item['slot']]}: none on this board")
    elif item['id'] in dropped:
        twin = next(g[0] for g in shared.values() if item in g)
        report['skipped'].append(f"{BOARD[item['slot']]}: same look as {twin['label']}")
    elif item['id'] in library['models']:
        report['added'].append(f"{item['slot']}: {item['label']}")
    else:
        report['skipped'].append(f"{item['slot']} {item['mod']}: conversion failed")
counts = {}
for item in added:
    if item['id'] in library['models']:
        counts[item['slot']] = counts.get(item['slot'], 0) + 1
print('REPORT', json.dumps(dict(counts=counts, coverage=coverage, shaders={f'{k} {c}': v for (k, c), v in shaders_used.items()},
    tops={i['label']: library['models'][i['id']]['flags'].get('TopType') for i in added if i['slot'] == 'OuterTorso' and i['id'] in library['models']})))
print('PRO_CLOTHING_READY', len(registered), 'tiles', len(library['models']), 'models')
