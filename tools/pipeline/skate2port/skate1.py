"""Skate 1 (2007) Create-a-Skater data in Skate 3's catalog terms.

Skate 1's big_cac.xml is recipe schema 5.5 with "FE" authoring fields where Skate 3 has
"cas" ones; meshes live under model/big_cac/<slot> (older RW4 mesh layout: s1mesh.py).
Skate 1 has male skaters only and no item thumbnails (s1render.py draws them)."""
import re
import xml.etree.ElementTree as ET
from s2big import Big
from s2catalog import sanitize

# Skate 1 slot -> Skate 3 slot. Skate 3 has no belt slot: belts go to Accessory, tagged cas.AccessoryType
# 'belt' for their own Accessories > Belts tab (retail's unused ID_CAC_BELTS_TITLE).
SLOTS = {'Hat': 'Hat', 'OuterTorso': 'OuterTorso', 'Pants': 'Pants', 'Feet': 'Feet', 'Glasses': 'Glasses',
         'Wristband': 'WristItem', 'Belt': 'Accessory', 'SkateBoard': 'SkateBoard', 'SkateTruck': 'SkateTruck',
         'SkateWheel': 'SkateWheel'}
ARMS = {'male_wrist_lod': 'wrist', 'male_bicep_lod': 'bicept', 'male_shoulders_lod': 'shoulder'}
LEGS = {'male_thigh_lod': 'thigh', 'male_calf_lod': 'knee'}
# Skate 1 shaders -> the Skate 3 character shader with the same role (customiser_lighting)
SHADERS = {'model_ropanormalspecmapcloth': 'character.cloth_ropa', 'model_normalspeccloth': 'character.cloth',
           'model_clothlight': 'character.default_cloth', 'model_normalspecskin': 'character.default_skin'}
SKIP_MODELS = {'male_full_torso_LOD'}  # the bare torso (Skate 3's "No top" is its own)


class Archive:
    """Skate 1 createacharacter.big with model paths moved to Skate 3's cas_db/<slot> layout."""
    def __init__(self, game):
        self.big = Big(f'{game}/data/content/createacharacter.big')
        self.entries, self._source = [], {}
        for e in self.big.entries:
            path = e.path
            m = re.match(r'(data/content/createacharacter/model/)big_cac/([^/]+)/(.*)', path, re.I)
            if m:
                path = f'{m[1]}cas_db/{SLOTS.get(m[2], m[2])}/{m[3]}'
            elif not path.lower().startswith('data/content/createacharacter/texture/'):
                continue
            entry = type(e)(path, e.offset, e.size)
            self._source[id(entry)] = e
            self.entries.append(entry)

    def read(self, entry):
        return self.big.read(self._source.get(id(entry), entry))


def top_type(name):
    n = name.lower()
    for key, kind in (('hoodie', 'hoody'), ('jacket', 'jacket'), ('windbreaker', 'jacket'), ('sweater', 'sweater'),
                      ('shirt_button', 'buttonshirt'), ('collared', 'buttonshirt'), ('tanktop', 'tanktop'),
                      ('slvl', 'ls_tshirt'), ('tshirt', 'tshirt')):
        if key in n:
            return kind
    return 'tshirt'


def catalog_xml(game):
    """big_cac.xml rewritten as a Skate 3 cas_db v6.0 document (FE fields -> cas fields)."""
    big = Big(f'{game}/data/content/createacharacter.big')
    root = ET.fromstring(sanitize(big.read(next(e for e in big.entries if e.path.lower().endswith('recipe/createacharacter/big_cac.xml')))))
    root.attrib.update(n='cas_db', schemaversion='6.0')

    def fields(node):
        return {f.tag[3:]: f.get('value', '') for g in node.findall('ud') if g.get('n') == 'FE' for f in g}

    def replace(node, values):
        for g in node.findall('ud'):
            node.remove(g)
        ud = ET.Element('ud', n='cas')
        for k, v in values.items():
            ET.SubElement(ud, 'cas.' + k, value=v)
        node.insert(0, ud)

    for mat in root.findall('mat'):
        fe = fields(mat)
        brand = fe.get('sponsorshipID') or re.sub(r'^ID_SPONSORSHIPBROWSER_', '', fe.get('manufID', '')).lower()
        replace(mat, {'ManufacturerID': brand if brand not in ('', 'generic') else 'no_brand',
                      'Skate1LockGroup': fe.get('lockGroup', '')})
    for comp in list(root.findall('comp')):
        slot = comp.get('n')
        if slot not in SLOTS:
            root.remove(comp)
            continue
        comp.set('n', SLOTS[slot])
        for mod in list(comp.findall('mod')):
            fe = fields(mod)
            name = mod.get('n')
            if name in SKIP_MODELS:
                comp.remove(mod)
                continue
            flags = {'Gender': 'unisex' if slot.startswith('Skate') else ('female' if fe.get('female') == '1' else 'male')}
            if slot == 'OuterTorso':
                flags['TopType'] = top_type(name)
                if (arm := ARMS.get(fe.get('skinModel1', '').lower())):
                    flags['ArmModelRequired'] = arm
            if slot == 'Pants':
                if (leg := LEGS.get(fe.get('skinModel1', '').lower())):
                    flags['LegModelRequired'] = leg
                flags['WearStyle'] = 'low waist pant' if '_lo_' in name.lower() else 'high waist pant'
            if slot == 'Belt':
                flags['AccessoryType'] = 'belt'
            if slot == 'Hat':
                flags['RequiresHairModelType'] = 'none' if 'helmet' in name.lower() else 'hat'
            flags['Skate1StyleGroup'] = fe.get('StyleGroup', '')
            replace(mod, flags)
    return ET.tostring(root)


COLOURS = {'bk': 'Black', 'wh': 'White', 'wt': 'White', 'gy': 'Grey', 'gr': 'Grey', 'bl': 'Blue', 'lb': 'Light Blue',
           'rd': 'Red', 'br': 'Brown', 'bn': 'Brown', 'gn': 'Green', 'ol': 'Olive', 'be': 'Beige', 'bg': 'Beige',
           'yl': 'Yellow', 'yw': 'Yellow', 'pu': 'Purple', 'pi': 'Pink', 'go': 'Gold', 'or': 'Orange', 'mg': 'Magenta',
           'mr': 'Maroon'}
BRANDS = {'4star': 'Fourstar', 'alienworkshop': 'Alien Workshop', 'blacklabel': 'Black Label', 'planb': 'Plan B',
          'dc': 'DC', 'es': 'éS', 'dvs': 'DVS', 'c1rca': 'C1RCA', 'ipath': 'IPath', 'zoo': 'Zoo York',
          'toyMachine': 'Toy Machine', 'toymachine': 'Toy Machine', 'thrasher': 'Thrasher', 'faa': 'FA',
          'jrattray': 'John Rattray', 'rdyrdek': 'Rob Dyrdek', 'jdill': 'Jason Dill'}
NOISE = {'material', 'lod', 'hip', 'lo', 'combined', 'comobined', 'nko', 'pko', 'pkx', 'pky', 'zpo', 'zpx', 'zpy', 'fc',
         'slvs', 'slvl', 'slvx', 'slvu', 'ul', 'loose', 'loose02', 'tight', 'long', 'down', 'medium', 'regular', 'cac',
         'color', 'eccen', 'board', 'truck', 'wheel', 'trim', 'shoes', 'shoe', 'hats', 'hat', 'cap', 'belts', 'belt',
         'bracelets', 'watches', 'glasses', 'tshirt', 'hoodie', 'shirt', 'buttonup', 'jacket', 'sweater', 'blinn',
         'baseball'}


def part_name(slot, model, material):
    """Readable Skate 1 part name from authoring names: tshirt_loose_slvs_nko_chocolate_wh01_material -> Chocolate (White)."""
    words, colour = [], ''
    for token in re.split(r'[_ ]+', material.lower()):
        m = re.fullmatch(r'([a-z]{2})(\d\d)[a-z]?', token)
        if m and m[1] in COLOURS:
            colour = colour or COLOURS[m[1]]
            continue
        if token in NOISE or not token or token.isdigit() or re.fullmatch(r'(?:blinn|lambert|phong)\d*', token):
            continue
        words.append(BRANDS.get(token, token.title()))
    if not words:  # e.g. jeans_regular_hip_bk01 -> model words
        words = [BRANDS.get(t, t.title()) for t in re.split(r'_+', model.lower()) if t not in NOISE and not t.isdigit()]
    words = [w for i, w in enumerate(words) if w not in words[:i]]
    name = ' '.join(words) or model
    return f'{name} ({colour})' if colour else name
