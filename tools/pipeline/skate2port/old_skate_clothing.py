"""Port Skate 1 and Skate 2 Create-a-Skater merchandise into the installed Skate 3 customiser set:
hats, tops (tees, shirts, hoodies, jackets, sweaters), pants, shoes, socks, glasses, necklaces,
wristwear, rings, belts, decks, trucks and wheels, with their wear styles and tile pictures.
Each game's tiles carry its title logo ("skate." / "skate2") bottom-right.

Skate 2 uses the same cas_db v6.0 catalog, RW4 meshes/textures, skeleton and shaders as Skate 3;
its archives are classic BIGF/BIG4 (s2big.py). Its DLC (Throwback, First Gear) items are on the
disc, unlocked by cas.SponsorUnlock: they are included with the DLC tile badge. Skate 2
designs that Skate 3 already ships (same model, near-identical texture) are skipped.

Skate 1 (skate1.py) has an older recipe schema (FE fields), an older mesh layout (s1mesh.py),
male skaters only, no belt slot in Skate 3 (belts go to Accessories > Rings) and no item
pictures: cas_tiles.py draws the clothing on the Skate 3 tile template (Skate 2's clothing too, its
own pictures being framed differently) and the decks, trucks and wheels on retail's board-part template. Its three tee fits (loose, tight,
long) become one tee with Edit Clothing options.

Every token of a ported model name gets the game's prefix (track_jacket_closed ->
s2track_s2jacket_s2closed): thumbnail stems (<model>_<material>) never collide with Skate 3's,
and the menu's wear-style grouping (name-token subsets, closed/open pairs) stays within a game.

usage (numpy + Pillow): py old_skate_clothing.py REPO GAME_ROOT DLC_EXTRACTED SKATE2_ROOT SKATE1_ROOT ASSETS
Rerun after dlc_clothing.py or a setup --refresh; backups in <set>/pre-skate2/.
"""
import functools, json, re, shutil, struct, sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import numpy as np
from PIL import Image

repo, game, dlc, s2game, s1game, assets = map(Path, sys.argv[1:7])
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/mixamo_to_skate'), str(Path(__file__).resolve().parent)]
from tools.owned_game import big
from tools.asset_pipeline import customisation_catalog, customisation_library, customiser_lighting, native_roster, retail_character
from tools.asset_pipeline import customiser_cache as cache
from tools.extract_default_skater import import_rx2_parser
from tools.asset_pipeline.customisation_library import decode_png
from vendor.skate3_ui.rw4 import extract_textures
from s2big import Big
from s2catalog import sanitize
from s2lang import strings
from families import crossings, families, origin
from s2badge import badge, logo, logo_skate1, stamp
import skate1, s1mesh, cas_tiles, fit_clothing

MERCH = {'Hat', 'OuterTorso', 'Pants', 'Feet', 'Sock', 'Glasses', 'Jewellery', 'WristItem', 'Accessory',
         'SkateBoard', 'SkateTruck', 'SkateWheel'}
# garment requirement flag -> (body slot, flag the body part provides)
BODY = {'cas.ArmModelRequired': ('Arm', 'cas.ArmModelType'), 'cas.InnerCutTypeRequired': ('InnerTorso', 'cas.InnerCutType'),
        'cas.LegModelRequired': ('Leg', 'cas.LegModelType')}
# Skate 3 renamed the male leg cuts; its own legs keep skin tones consistent with its arms and head.
ALIASES = {'knee down': 'knee', 'thigh down': 'thigh'}
DLC = {'cac_throwback_pack_inventory': 'skate2_throwback', 'cac_dlc_first_gear_pack_inventory': 'skate2_first_gear'}
WEAR = ('cas.WearStyle', 'cas.HatOrientation', 'cas.IsDefaultWearStyle')
ORIENTATION = {'flippedup': 'flippedup', '45left': '45left', '45right': '45right', 'backward': 'backwards',
               'headphone': 'headphone', 'pulldown': 'pulleddown'}
# Skate 3 authors the cap option order in the names (_1_flippedup, _2_45left, ...); the menu sorts by name.
ORDER = {'flippedup': 1, '45left': 2, '45right': 3, 'backwards': 4, 'headphone': 5, 'pulleddown': 6}
DRAWN = f'cas_tiles {cas_tiles.TEMPLATE_VERSION}'  # clothing tiles' marker: cas_tiles.TEMPLATE_VERSION redraws them all

DiscArchive = big.BigArchive
dlc_bigs = [DiscArchive(p) for p in sorted(dlc.glob('*/*.big'))]
s2cac = Big(s2game/'data/content/createacharacter.big')
s1cac = skate1.Archive(s1game)


class LayeredArchive:
    """Disc archive with Skate 3 DLC same-kind entries (and Skate 1/2 CAC meshes/textures) on top."""
    def __init__(self, path):
        kind = '/' + Path(path).stem.lower() + '/'
        extra = [s2cac, s1cac] if Path(path).name.lower() == 'createacharacter.big' else []
        self.owner, by_path = {}, {}
        for archive in (DiscArchive(path), *dlc_bigs, *extra):
            for entry in archive.entries:
                p = entry.path.lower()
                if archive in dlc_bigs and kind not in p or archive in extra and not p.startswith('data/content/createacharacter/'):
                    continue
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


def material_data(model, collections, parser, specular=None):
    """Skate 1 models name a shader (model_normalspeccloth, ...) instead of Skate 2/3's family.key."""
    shaders = {p.value for p in parser.parse_rx2(model).materials if p.kind == 'AttribulatorMaterialName'}
    name = next(iter(shaders)) if len(shaders) == 1 else ''
    if name and '.' not in name:
        shader = skate1.SHADERS.get(name, 'character.cloth')
        family, key = shader.split('.', 1)
        fields, _ = collections.resolve('material_' + family, key)
        rows = fields[customiser_lighting.key_hash('m_params')]['array']['items']
        return dict(shader=shader, params=[struct.unpack('>4f', bytes.fromhex(r)) for r in rows], specular=specular)
    return skate3_material_data(model, collections, parser, specular)


customiser_lighting.material_data = material_data

base = assets/'private/customisation'
directory = base/'sets'/json.loads((base/'current.json').read_text())['set']
backup = directory/'pre-skate2'
if not (backup/'catalog.json').exists():
    backup.mkdir(exist_ok=True)
    for name in ('catalog.json', 'library-base.json', 'library-v3.json', 'native-lighting.json',
                 'catalog-complete.json', 'library-complete.json', 'lighting-complete.json'):
        shutil.copy2(directory/name, backup/name)

# ---- Skate 2 catalog -------------------------------------------------------------------------
xml = s2cac.read(next(e for e in s2cac.entries if e.path.lower() == 'data/content/recipe/createacharacter/cas_db_000.xml'))
s2 = customisation_catalog.parse_catalog(sanitize(xml), {e.path for e in s2cac.entries})
for model in (m for c in s2['components'] for m in c['models']):
    if model['flags'].get('cas.IsDefaultWearStyle') == '1':
        model['flags']['cas.IsDefaultWearStyle'] = 'yes'
# Skate 2 leaves HatOrientation/IsDefaultWearStyle blank on some cap variants (every female one,
# the male fitted cap, the headphone hats) where Skate 3 authors them. Fill them from the names
# so the variants become the cap's Edit Clothing options instead of tiles of their own.
hats = [m for c in s2['components'] if c['slot'] == 'Hat' for m in c['models']]
for model in hats:
    found = next((ORIENTATION[t] for t in model['name'].split('_') if t in ORIENTATION), None)
    if found and not model['flags'].get('cas.HatOrientation'):
        model['flags']['cas.HatOrientation'] = found
for model in hats:
    flags = model['flags']
    if (flags.get('cas.HatOrientation') or 'normal') != 'normal' or flags.get('cas.IsDefaultWearStyle') == 'yes':
        continue
    base = set(model['name'].split('_'))
    styles = [m for m in hats if m['flags'].get('cas.Gender') == flags.get('cas.Gender')
              and (m['flags'].get('cas.HatOrientation') or 'normal') != 'normal' and set(m['name'].split('_')) - set(ORIENTATION) == base]
    if styles:
        flags['cas.HatOrientation'], flags['cas.IsDefaultWearStyle'] = 'normal', 'yes'
# A few closed/open and sleeve pairs have no default (the female leather jacket flags the open
# one). Skate 3 authors the closed/plain model as the default: do that for tops no family claims.
# A shared name token keeps the menu's token-subset match to the pair (not a same-sized garment).
tops = [m for c in s2['components'] if c['slot'] == 'OuterTorso' for m in c['models']]
grouped = {n for d, ms in families({'components': [{'models': tops}]}).items() for n in (d, *ms)}
orphans = [m for m in tops if m['flags'].get('cas.WearStyle') and m['name'] not in grouped]
count = lambda m: sum(len(g) for g in m['lods'][0]['material_instances'])
pair_tag = {}
for base in orphans:
    alts = [m for m in orphans if m is not base and m['id'] not in pair_tag
            and all(m['flags'].get(k) == base['flags'].get(k) for k in ('cas.Gender', 'cas.TopType')) and count(m) == count(base)
            and (set(base['name'].split('_')) < set(m['name'].split('_')) or base['name'].replace('closed', 'open') == m['name'])]
    if alts and base['id'] not in pair_tag:
        tag = f'p{len(set(pair_tag.values()))}'
        for m in (base, *alts):
            pair_tag[m['id']] = tag
            m['flags']['cas.IsDefaultWearStyle'] = 'yes' if m is base else ''
        print('Wear-style pair:', base['name'], '->', [m['name'] for m in alts])

# ---- Skate 1 catalog -------------------------------------------------------------------------
s1 = customisation_catalog.parse_catalog(skate1.catalog_xml(s1game), {e.path for e in s1cac.entries})
# One StyleGroup sharing its designs = one garment in several fits (tees: loose/tight/long):
# the first model is the default, the rest its Edit Clothing options with the same design list.
# Each fit has its own material ids for a design, so designs match by name; the menu then pairs
# the fits by name tokens: the default is named after the group's common tokens plus a fit tag.
style_groups, fit_names, label_as = {}, {}, {}
for component in s1['components']:
    for model in component['models']:
        if (group := model['flags'].get('cas.Skate1StyleGroup')):
            style_groups.setdefault((component['slot'], group), []).append(model)
for (slot, group), models in style_groups.items():
    designs = lambda m: [v['name'] for v in m['lods'][0]['material_instances'][0]]
    shared = [n for n in dict.fromkeys(designs(models[0])) if all(n in designs(m) for m in models[1:])]
    if len(models) < 2 or len(shared) < 0.8 * len(designs(models[0])):
        continue  # cargo shorts vs cargo pants: different garments that share a group name
    tag = f'f{len(set(t for t, _ in fit_names.values()))}'
    common = [t for t in models[0]['name'].split('_') if all(t in m['name'].split('_') for m in models[1:])]
    for model in models:
        by_name = {}
        for v in model['lods'][0]['material_instances'][0]:
            by_name.setdefault(v['name'], v)
        model['lods'][0]['material_instances'][0] = [by_name[n] for n in shared]
        model['flags']['cas.IsDefaultWearStyle'] = 'yes' if model is models[0] else ''
        model['flags']['cas.WearStyle'] = ' '.join(t for t in model['name'].lower().split('_') if t != 'lod')
        fit_names[model['id']] = (tag, common if model is models[0] else model['name'].split('_'))
        label_as[model['id']] = models[0]['name']  # one design label across the fits (Edit Clothing keeps the design)
    print('Skate 1 wear styles:', [m['name'] for m in models], len(shared), 'designs')

# ---- merge -----------------------------------------------------------------------------------
catalog = json.loads((directory/'catalog.json').read_text())
for component in catalog['components']:
    component['models'] = [m for m in component['models'] if not origin(m['name'])]
for mid in (*s2['materials'], *s1['materials']):
    catalog['materials'].pop(mid, None)
library = json.loads((directory/'library-v3.json').read_text())
skate3 = {m['id']: (c['slot'], m) for c in catalog['components'] for m in c['models'] if m['id'] in library['models']}

provided = {(slot, m['flags'].get(flag, ''), m['flags'].get('cas.Gender', ''))
            for slot, m in skate3.values() for slot_, flag in BODY.values() if slot == slot_}


def has(slot, value, gender):
    return any((slot, value, g) in provided for g in (gender, 'unisex', ''))


def keep(source, gamename, extra_body):
    """(slot, model) of a ported catalog's merchandise, with requirements Skate 3 can meet."""
    body = [(c['slot'], m) for c in source['components'] if c['slot'] in {s for s, _ in BODY.values()} for m in c['models']]
    result = []
    for component in source['components']:
        if component['slot'] not in MERCH:
            continue
        for model in component['models']:
            if component['slot'] == 'OuterTorso' and not model['flags'].get('cas.TopType'):
                continue  # the bare torso (no top): Skate 3's own is the one worn (a second listed a second Remove tile)
            flags, gender = model['flags'], model['flags'].get('cas.Gender', '')
            for need, (slot, provides) in BODY.items():
                value = flags.get(need, '')
                if not value or has(slot, value, gender):
                    continue
                if value in ALIASES and has(slot, ALIASES[value], gender):
                    flags[need] = ALIASES[value]
                    continue
                parts = [m for s, m in body if s == slot and m['flags'].get(provides) == value and m['flags'].get('cas.Gender') in (gender, 'unisex')]
                if not parts:
                    raise SystemExit(f'No {slot} for {gamename} {model["name"]} ({need}={value}, {gender})')
                extra_body.update({m['id']: (slot, m) for m in parts})
            result.append((component['slot'], model))
    return result


extra_body, notes = {}, []
kept = keep(s2, 'Skate 2', extra_body)
for slot, model in extra_body.values():
    notes.append(f'{slot} {model["name"]} (needed by Skate 2 garments)')
    kept.append((slot, model))
kept1 = keep(s1, 'Skate 1', {})

# Skip Skate 2 designs Skate 3 already ships: same slot and model name, near-identical diffuse texture.
parser = import_rx2_parser(repo/'tools/vendor/utt')
decoded = directory/'decoded'
decoded.mkdir(exist_ok=True)
s2textures = {e.path.lower(): e for e in s2cac.entries}


def diffuse(cat, mid):
    tid = next((t['id'] for t in cat['materials'][mid]['textures'] if t['channel'] == 'diffuse'), None)
    if tid and cat is s2 and not (decoded/(tid+'.png')).exists():
        decode_png(parser, s2cac.read(s2textures[f'data/content/createacharacter/texture/0x{tid}.rx2']), decoded/(tid+'.png'), tid+'.rx2')
    return decoded/(tid+'.png') if tid else None


@functools.cache
def thumb(path):
    return np.asarray(Image.open(path).convert('RGB').resize((128, 128), Image.Resampling.BOX))


def pixels(path):
    return thumb(path).astype(np.int16)


def twin(path, theirs):
    """Whether one of the pictures `theirs` shows the design at `path`: under 1% of its pixels visibly different."""
    if (lib := cas_tiles._native()) is not None:  # tiles_native.rs: the same test on the bytes
        if not theirs:
            return False
        mine, stack = np.ascontiguousarray(thumb(path)), np.ascontiguousarray(np.stack([thumb(p) for p in theirs]))
        assert mine.dtype == stack.dtype == np.uint8 and mine.shape == stack.shape[1:] and mine.shape[2] == 3
        return bool(lib.tiles_twin(mine.ctypes.data, stack.ctypes.data, len(theirs), mine.shape[0] * mine.shape[1]))
    return any(float((np.abs(pixels(path)-pixels(t)).max(axis=2) > 40).mean()) < .01 for t in theirs)


twins = {}
for slot, model in skate3.values():
    twins.setdefault((slot, model['name'].lower()), []).append(model)


def compared(slot, model):
    """Whether the model's designs are checked against Skate 3's same-named model."""
    lod = model['lods'][0]
    # ponytail: wear-style families and multi-loop models are kept whole; pruning a design
    # would misalign the family's design lists (create_skater.rs wear_families).
    return (slot, model['name'].lower()) in twins and len(lod['material_instances']) == 1 and not any(model['flags'].get(k) for k in WEAR)


# the Skate 2 diffuses the check below decodes (diffuse), decoded first on threads (the decode and PNG encode let go of the
# GIL), one design per texture so no two threads write one file
by_texture = {next((t['id'] for t in s2['materials'][v['id']]['textures'] if t['channel'] == 'diffuse'), None): v['id']
              for slot, model in kept if compared(slot, model) for v in model['lods'][0]['material_instances'][0]}
with ThreadPoolExecutor(8) as pool:
    list(pool.map(lambda mid: diffuse(s2, mid), [mid for tid, mid in by_texture.items() if tid]))


def preload(path):
    try:
        thumb(path)
    except Exception:
        pass  # the check below meets it again


# and their thumbnails (and the Skate 3 twins'), read and shrunk on threads too
pictures = [diffuse(s2, mid) for tid, mid in by_texture.items() if tid]
pictures += [diffuse(catalog, v['id']) for slot, model in kept if compared(slot, model)
             for t in twins[(slot, model['name'].lower())] for v in t['lods'][0]['material_instances'][0] if v['id'] in library['materials']]
with ThreadPoolExecutor(8) as pool:
    list(pool.map(preload, dict.fromkeys(p for p in pictures if p and p.exists())))
duplicates = 0
for slot, model in list(kept):
    lod = model['lods'][0]
    if not compared(slot, model):
        continue
    theirs = [p for t in twins[(slot, model['name'].lower())] for v in t['lods'][0]['material_instances'][0]
              if v['id'] in library['materials'] and (p := diffuse(catalog, v['id'])) and p.exists()]
    fresh = []
    for variant in lod['material_instances'][0]:
        path = diffuse(s2, variant['id'])
        # share of visibly different pixels; a changed logo moves it well past 1%
        if path and twin(path, theirs):
            duplicates += 1
        else:
            fresh.append(variant)
    lod['material_instances'][0] = fresh
    if not fresh:
        kept.remove((slot, model))
print('Skipped', duplicates, 'Skate 2 designs Skate 3 already has')

s1_materials = set()
for gamename, source, models in (('s2', s2, kept), ('s1', s1, kept1)):
    used = {v['id'] for _, m in models for lod in m['lods'] for group in lod['material_instances'] for v in group}
    for mid in used:
        material = source['materials'][mid]
        if (pack := DLC.get(material['flags'].get('cas.SponsorUnlock', ''))):
            material['flags']['cas.DlcID'] = pack
        catalog['materials'][mid] = material
    if gamename == 's1':
        s1_materials = used
    print(gamename, 'models', len(models), 'designs', len(used))
original, ported = {}, []
for gamename, models in (('s2', kept), ('s1', kept1)):
    for slot, model in models:
        original[model['id']] = model['name']
        tag, tokens = fit_names.get(model['id'], (None, model['name'].split('_'))) if gamename == 's1' else (None, model['name'].split('_'))
        model['name'] = '_'.join(gamename + token for token in ([tag] if tag else []) + tokens)
        if gamename == 's2' and slot == 'Hat' and (n := ORDER.get(model['flags'].get('cas.HatOrientation'))):
            model['name'] = f's2{n}_{model["name"]}'
        if gamename == 's2' and model['id'] in pair_tag:
            model['name'] = f's2{pair_tag[model["id"]]}_{model["name"]}'
        component = next((c for c in catalog['components'] if c['slot'] == slot), None)
        if component is None:
            component = {'slot': slot, 'models': []}
            catalog['components'].append(component)
        component['models'].append(model)
        ported.append((slot, model))
assert not crossings(catalog), crossings(catalog)
bare = sorted(m['flags'].get('cas.Gender', '') for c in catalog['components'] if c['slot'] == 'OuterTorso'
              for m in c['models'] if not m['flags'].get('cas.TopType'))
assert bare == ['female', 'male'], bare  # one bare torso per gender (customiser.rs lists each as a Remove tile)
grouped = {n for d, ms in families(catalog).items() for n in (d, *ms)}
alternates = {n for d, ms in families(catalog).items() for n in ms}
for gamename in ('s2', 's1'):
    print(gamename, 'wear-style families:', sum(origin(d) == gamename for d in families(catalog)))
print('styled Skate 2 models without a family:',
      sorted(m['name'] for _, m in kept if any(m['flags'].get(k) for k in WEAR) and m['name'] not in grouped))
(directory/'catalog.json').write_text(json.dumps(catalog, indent=2) + '\n', encoding='utf-8')
print(*notes, sep='\n  ')

# Skate 1 normal maps are plain RGB tangent normals (Skate 2/3 pack X in alpha, Y in green):
# write their converted form where prepare() caches it so it is used as is.
normals = directory/'library/textures'
normals.mkdir(parents=True, exist_ok=True)
s1textures = {e.path.lower(): e for e in s1cac.entries}
s1normals = {}  # texture id -> its disc path, once each (designs share normal maps)
for mid in s1_materials:
    tid = next((t['id'] for t in catalog['materials'][mid]['textures'] if t['channel'] == 'normal'), None)
    path = f'data/content/createacharacter/texture/0x{tid}.rx2'
    if not tid or (normals/(tid+'_normal.png')).exists() or path not in s1textures:
        continue
    s1normals[tid] = path


def s1_normal(tid, path):
    if not (decoded/(tid+'.png')).exists():
        image = decode_png(parser, s1cac.read(s1textures[path]), decoded/(tid+'.png'), tid+'.rx2')
    else:
        image = Image.open(decoded/(tid+'.png'))
    image.convert('RGB').save(normals/(tid+'_normal.png'))


with ThreadPoolExecutor(8) as pool:  # the decode and the PNG encodes let go of the GIL
    list(pool.map(s1_normal, s1normals, s1normals.values()))

# Accessory items keep their authored skin weights (the fit below skips them): reconvert any an earlier run fitted.
for slot, model in ported:
    glb = directory/'library'/f"{model['id']}_v4.glb"
    if slot == 'Accessory' and glb.exists() and fit_clothing.fitted_already(glb):
        glb.unlink()
library = customisation_library.prepare(dict(assets=str(assets), game_root=str(game), directory=str(directory),
                                             library_index='library-base.json'))
print(len(library['errors']), 'variants unavailable', [e for e in library['errors'] if e.get('model') in original][:20])
# Names from the unprefixed authoring names, as prepare() words them for Skate 3 items.
labelled = set()
for slot, model in ported:
    if model['id'] not in library['models']:
        continue
    library['models'][model['id']]['name'] = customisation_library.friendly(original[model['id']])
    for variant in (v for group in model['lods'][0]['material_instances'] for v in group):
        if variant['id'] in library['materials'] and variant['id'] not in labelled:
            labelled.add(variant['id'])
            library['materials'][variant['id']]['name'] = customisation_library.variant_label(
                slot, label_as.get(model['id'], original[model['id']]), variant['name'])
# Same authoring-prefix cleanup dlc_clothing.py applies ("Dlc Bloodshed", "Dw Dw Planb Dw Mega").
for material in library['materials'].values():
    material['name'] = re.sub(r'^(?:(?:Dlc|Dw) )+', '', material['name']).replace(' Dw ', ' ')
(directory/'library-base.json').write_text(json.dumps(library, separators=(',', ':')))
customiser_lighting.prepare(game, assets, directory, library)
library = json.loads((directory/'library-v3.json').read_text())
# the 10-04 skin-through-clothes fit, before the receipts below and before any tile is drawn from these models
# (not the Accessory slot: see fit_clothing.apply).
fitted = fit_clothing.apply(assets, library)
print('fitted', len(fitted), 'models')

for name in ('catalog', 'library', 'lighting'):
    marker = directory/(name+'-complete.json')
    saved = json.loads(marker.read_text())
    paths = []
    for relative in cache.OUTPUTS[name]:
        path = directory/relative
        paths.extend(path.rglob('*') if path.is_dir() else [path])
    saved['files'] = cache.receipt(directory, paths, marker)  # unchanged files keep the marker's hashes
    marker.write_text(json.dumps(saved))

# ---- Edit Skater tiles -----------------------------------------------------------------------
# The grid lists a part only with a thumbnail (data/fe/cas/<gender>/<slot>/<model>_<material>).
screen = assets/'private/create-skater'
manifest_path = screen/'runtime/createskater.json'
if not (screen/'runtime/createskater.json.pre-skate2').exists():
    shutil.copy2(manifest_path, screen/'runtime/createskater.json.pre-skate2')
manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
thumb_root = screen/'thumbs/assets/data/fe/cas'
# A previous run's entries (Skate 3 has no s1/s2-prefixed stems; checked 2026-09-28).
previous_keys = [k for k in manifest['thumbnails'] if k.rsplit('/', 1)[-1][:2] in ('s1', 's2')]
previous = {k.rsplit('/', 1)[-1] for k in previous_keys}
manifest['thumbnails'] = {k: v for k, v in manifest['thumbnails'].items() if k.rsplit('/', 1)[-1] not in previous}
manifest['language'] = {k: v for k, v in manifest['language'].items()
                        if not (k.startswith('ID_CAC_PART_') and k[len('ID_CAC_PART_'):].lower() in previous)}


def register(key, folder):
    texture = next(t for t in json.loads((folder/'manifest.json').read_text())['textures'] if t.get('rgba_file'))
    manifest['thumbnails'][key] = {'rgba': (folder/texture['rgba_file']).relative_to(screen).as_posix(),
                                   'width': texture['width'], 'height': texture['height']}


def register_drawn(key, stem):
    """register() for a tile cas_tiles.save_tile just wrote (its manifest names the RGBA file, 128x128)."""
    manifest['thumbnails'][key] = {'rgba': (thumb_root/key/f'0000_{stem}.Texture.rgba').relative_to(screen).as_posix(),
                                   'width': 128, 'height': 128}


def drawn(key, model_id):
    """The tile is this template's drawing, and a fitted model's tile was drawn from the current fit."""
    marker = thumb_root/key/'manifest.json'
    saved = json.loads(marker.read_text()) if marker.exists() else {}
    return saved.get('drawn') == DRAWN and (model_id not in fitted or saved.get('fit') == fit_clothing.VERSION)


# Skate 2: the designs it pictured, with its English part names and the skate2 logo. Clothing, decks,
# trucks and wheels are drawn on the Skate 3 tile templates (cas_tiles): Skate 2's own pictures use its
# framing (black brush frame, other cameras); other folders keep Skate 2's thumbnails.
stems = {f'{original[m["id"]]}_{v["name"]}'.lower(): f'{m["name"]}_{v["name"]}'.lower() for _, m in kept if m['id'] in library['models']
         for group in m['lods'][0]['material_instances'] for v in group if v['id'] in library['materials']}
ids = {f'{m["name"]}_{v["name"]}'.lower(): (m['id'], v['id']) for _, m in kept if m['id'] in library['models']
       for group in m['lods'][0]['material_instances'] for v in group if v['id'] in library['materials']}
assert not any(k.rsplit('/', 1)[-1] in stems.values() for k in manifest['thumbnails']), 'stem collides with a Skate 3 thumbnail'
names = strings(s2game)
mark = badge(logo(s2game))
thumbs = Big(s2game/'data/fe/cas.big')
count, todo = 0, {}
for entry in thumbs.entries:
    path = entry.path.lower()
    if not (path.startswith('data/fe/cas/') and path.endswith('.rx2')):
        continue
    *folders, stem = path[len('data/fe/cas/'):-len('.rx2')].split('/')
    if stem not in stems:
        continue
    key = '/'.join([*folders, stems[stem]])
    folder = thumb_root/key
    clothing = folders[0] in ('male', 'female') or folders[0] in cas_tiles.BOARD_FOLDERS  # round 2: boards on retail's board template
    if clothing and not drawn(key, ids[stems[stem]][0]):  # drawn below
        model_id, mid = ids[stems[stem]]
        todo.setdefault(('/'.join(folders), model_id), []).append((mid, key, stems[stem]))
    elif not clothing and (not (folder/'manifest.json').exists() or not json.loads((folder/'manifest.json').read_text()).get('skate2_badge')):
        shutil.rmtree(folder, ignore_errors=True)
        folder.mkdir(parents=True)
        (folder/'source.rx2').write_bytes(thumbs.read(entry))
        extracted = extract_textures(folder/'source.rx2', folder, force=True, decode_png=True)
        for t in (t for t in extracted['textures'] if t.get('rgba_file')):
            stamp(folder/t['rgba_file'], t['width'], t['height'], mark).save(folder/Path(t['rgba_file']).with_suffix('.png').name)
        extracted['skate2_badge'] = 1
        (folder/'manifest.json').write_text(json.dumps(extracted))
        (folder/'source.rx2').unlink()
    if (folder/'manifest.json').exists():
        register(key, folder)
    if (text := names.get('ID_CAC_PART_' + stem.upper(), '').strip()):
        manifest['language']['ID_CAC_PART_' + stems[stem].upper()] = text
    count += 1
drawn_s2 = 0
# one raster per model and folder (cas_tiles), drawn and saved on several processes (draw_tiles)
cas_tiles.draw_tiles(assets, directory/'library-v3.json', [(tile_folder, model_id, designs) for (tile_folder, model_id), designs in todo.items()],
                     thumb_root, mark, {'drawn': DRAWN, 'skate2_badge': 1, 'fit': fit_clothing.VERSION})
for designs in todo.values():
    for mid, key, stem in designs:
        register_drawn(key, stem)
        drawn_s2 += 1
print('Skate 2 clothing tiles drawn', drawn_s2)
shown = {k.rsplit('/', 1)[-1] for k in manifest['thumbnails']}
print('Skate 2 thumbnails', count, '| designs without a Skate 2 thumbnail (unlisted, as in Skate 2):',
      sum(s not in shown for s in stems.values()))

# Skate 1 (no item pictures): clothing and board parts drawn on the Skate 3 tile templates (cas_tiles);
# stamped with the "skate." logo, generated names.
mark = badge(logo_skate1(s2game))

def stem_of(model, names_by_id, mid):
    return f'{model["name"]}_{names_by_id[mid]}'.lower()


rendered = 0
parts = []  # every part first, so all their tiles are drawn together
for slot, model in kept1:
    part = library['models'].get(model['id'])
    if not part or model['name'] in alternates:
        continue  # alternate fits are Edit Clothing options of the default's tiles
    prefix = slot.lower() if slot.startswith('Skate') else f'male/{slot.lower()}'
    names_by_id = {v['id']: v['name'] for v in model['lods'][0]['material_instances'][0]}
    # round 2: decks, trucks and wheels too, on retail's board-part template (cas_tiles BOARD)
    parts.append((slot, model, part, prefix, names_by_id,
                  [mid for mid in part['materials'] if not drawn(f'{prefix}/{stem_of(model, names_by_id, mid)}', model['id'])]))
jobs = [(prefix, model['id'], [(mid, f'{prefix}/{stem}', stem) for mid in todo for stem in [stem_of(model, names_by_id, mid)]])
        for _, model, _, prefix, names_by_id, todo in parts if todo]
cas_tiles.draw_tiles(assets, directory/'library-v3.json', jobs, thumb_root, mark, {'drawn': DRAWN, 'skate1_badge': 1, 'fit': fit_clothing.VERSION})
for slot, model, part, prefix, names_by_id, todo in parts:
    for mid in part['materials']:
        stem = stem_of(model, names_by_id, mid)
        key = f'{prefix}/{stem}'
        folder = thumb_root/key
        if mid in todo and mid in library['materials']:  # drawn above (render_model skips designs the library lacks)
            register_drawn(key, stem)
            rendered += 1
        else:
            register(key, folder)
        manifest['language']['ID_CAC_PART_' + stem.upper()] = skate1.part_name(slot, original[model['id']], names_by_id[mid])
print('Skate 1 thumbnails drawn', rendered)

manifest_path.write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
for key in previous_keys:
    if key not in manifest['thumbnails']:
        shutil.rmtree(thumb_root/key, ignore_errors=True)
print('OLD_SKATE_CLOTHING_READY', len(library['models']), 'models', len(library['materials']), 'materials')
