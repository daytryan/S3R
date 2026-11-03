"""Skate 1 tutorial cast (research.md 4.2, 4.3a, 4.5): the scene actors, Shingo, Seb and the four pro tutorials' pros
as GLBs on Skate 3's skeleton, the pedestrian clips Shingo and Seb play, and their vault rows.

Skate 1's marquee.big has the recipe/marquee/<name>.xml + model/<recipe>/<slot>/*.rx2 layout native_roster reads; s1mesh
reads the older mesh layout. Seb (Deran_Chung) is livingworld.big's, in the same layout under livingworld/: one Rostral
part, seb_photographer, is his whole body (research5.md B2). Two recipe differences are handled here:
- a part's LOD lists one material instance per mesh in its file (Shingo's Leg: trousers, then the arms): instance i
  becomes part `<slot>~<i>`, which reads mesh i (the order INFERRED: the shared 1475-vertex arms mesh is mesh 1 under
  the skin material on Shingo, Haslam and Carroll);
- a hair material's cut map is `transparency`, its alpha in A; hair.psh / hairspec.psh have no diffuse sampler and
  light the map's RGB as the colour (research5.md B1). UNTRACED: the hair tint (the material-class rows of skater.vlt
  class D590F15C91FBB4A1 do not resolve from the recipe type names), hairspec's card_blend, and alpha test vs blend.
The models keep Skate 1's scene rig bones (the 48 face joints 78-125 with their parents, fingers, twists) as extra
joints, named by the tutorial's Basics bank (research 4.3a).

Writes ASSETS/private/challenges-skate1/ (rmtrees only cast/ and anim1/; old_challenges.py, which rmtrees all of
challenges-skate1/, runs first):
  cast/<model>.glb (+ .png)  native_roster.prepare's model and preview
  anim1/<clip>.bin           pedestriansDB.abin's clips (s1abin.export_clip) the entities' remap rows and Seb's
                             photo state name (PHOT_*); PHOTO_SHOOT_BLEND is a type-6 block, not a clip, and is left out
  cast.json                  {'models': {model: 'cast/<model>.glb'}, 'clips': {clip: export_clip meta + looping,
                             channel}, 'entities': {vault key: {name, model, remap: {node: [clip, ...]}}}}

usage: py old_challenges_cast.py REPO S1ROOT ASSETS [SOURCE]   (SOURCE, default ASSETS, read only: private/stock)
"""
import functools, json, re, shutil, sys, tempfile
import xml.etree.ElementTree as ET
from copy import deepcopy
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
source = Path(sys.argv[4]) if len(sys.argv) > 4 else assets
sys.path[:0] = [str(repo), str(repo/'tools'), str(Path(__file__).resolve().parent)]
from PIL import Image
from s2big import Big
from tools.asset_pipeline import character_glb, native_roster, retail_character
from tools.asset_pipeline.marquee_assets import MissingMarqueeAsset
from tools.asset_pipeline.vlt import convert, hash64
import s1abin, s1mesh

# The scene actors (scene.bin: tutorial_skater, IP_shingo), Seb's model and the pros of the four pro tutorials inside
# the window (research 4.5: NIS_CHaslam / JasonDill / MCarrol / PRod _ProTutorial tracks).
CAST = ['tutorial_skater', 'IP_shingo', 'Deran_Chung', 'Chris_haslam', 'jason_dill', 'mike_carroll', 'Paul_rodriguez',
        # User 10-15: Community Spirit's host (its GlobalSpawn; a livingworld model) and two of its competitors
        # (Competitors eric_unknown / fabio_unknown: character rows ip_eric / ip_fabio), then Danny Way.
        'Cuz', 'ip_eric', 'ip_fabio', 'danny_way']
# Class 60D703C77CAB1631 rows (research 4.2): Shingo ('ip_shingo') and Seb ("Photographer").
# P6: + P-Rod, Dill, Carroll, Haslam (research6 2: GlobalSpawn class 60D703C77CAB1631, marquee, remap 66F08967).
ENTITIES = ['0D0BDC7925D47161', '7E5E359D6CE96AEC',
            'B033A0ADE0933A58', '0DED833242F2F857', '146D3B7D88385267', '5B87C6981CA2AE86',
            '8C0D2AE0C53B259C', '42545B0938E341C9']  # he_u4_besttrick01's Cuz, fp_u3_tutorial03's Danny Way
ENTITY, MODEL, REMAP = '60D703C77CAB1631', '4E740A6670F241A7', '53E332F67B8124B2'
# Entity fields (read off skater.vlt, names UNKNOWN): display name, model RefSpec, remap RefSpec; the model row's
# recipe name and its eLivingWorldModelCategory (1 marquee: a whole recipe; 0 a living-world pedestrian).
NAME, MODEL_REF, REMAP_REF = 'BD8F02BA1986215A', '7DFFA6187E2DE132', '00367B8F33E79C43'
MODEL_NAME, CATEGORY = '62F7C585686F3371', 'F6897CC7B637850C'
# Remap fields named by the motion graph nodes that read them (marquee.xml / MotionGraph_Photographer.xml).
NODES = {'%016X' % hash64(n): n for n in ('IdleBasicCyc', 'CollisionBackStanding', 'CollisionFwdStanding',
                                          'CollisionLeftStanding', 'CollisionRightStanding')}
RECIPE_FIELD = 'Hash_6C9F05D8DBE7A492'
SPLIT, CUT = '~', '~a'  # part <slot>~<i>; texture <id>~a = a transparency map's alpha


# ---------------------------------------------------------------- vault
def hex_key(k):
    return k.removeprefix('Hash_').upper() if k.startswith('Hash_') else '%016X' % hash64(k)


stem = s1game/'data/db/skater'
ROWS = {(hex_key(r['class']), hex_key(r['key'])): r
        for r in convert(stem, stem, (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines())['collections']}
BIN = (s1game/'data/db/skater.bin').read_bytes()


def merged(cls, key):
    """A row's fields over its parent chain (the class's 'default' row last)."""
    out = {}
    while key and (cls, key) in ROWS:
        row = ROWS[cls, key]
        for k, v in row['fields'].items():
            out.setdefault(hex_key(k), v)
        key = hex_key(row['parent']) if row['parent'] else None
    return out


def texts(field):
    """A Text field's strings; an array's items are offsets of NUL-terminated strings in skater.bin."""
    if 'array' not in field:
        return [field['data']]
    return [BIN[int(p, 16):BIN.index(b'\0', int(p, 16))].decode('latin-1') for p in field['array']['items']]


def ref(field):
    """An Attrib::RefSpec -> (class, key)."""
    return field['data'][:16].upper(), field['data'][16:32].upper()


# ---------------------------------------------------------------- models
def split(xml):
    """Skate 1's recipe in native_roster's shape: one material instance per part (SPLIT) and a transparency map as
    the cut map (CUT) and, without a diffuse, the colour."""
    root = ET.fromstring(xml)
    for comp in root.findall('comp'):
        lod = next(l for l in comp.find('mod').findall('lod') if l.get('idx') == '0')
        for i in range(1, len(lod.findall('matinst'))):
            part = deepcopy(comp)
            part.set('n', f"{comp.get('n')}{SPLIT}{i}")
            own = next(l for l in part.find('mod').findall('lod') if l.get('idx') == '0')
            for j, inst in enumerate(own.findall('matinst')):
                if j != i:
                    own.remove(inst)
            root.append(part)
        for inst in lod.findall('matinst')[1:]:
            lod.remove(inst)
    for mat in root.findall('mat'):
        channels = {sp.get('chn'): sp.get('id') for sp in mat.findall('sp')}
        if 'transparency' in channels:
            if 'diffuse' not in channels:
                ET.SubElement(mat, 'sp', id=channels['transparency'], chn='diffuse')
            ET.SubElement(mat, 'sp', id=channels['transparency'] + CUT, chn='alpha')
    return ET.tostring(root)


LIVING = Big(s1game/'data/content/livingworld.big')
LIVING_FILES = native_roster.Resources(LIVING)


class Resources(native_roster.Resources):
    def resolve(self, path):  # a split part's and a cut texture's file is the original's; Seb's are livingworld.big's
        path = re.sub(rf'{SPLIT}\d+/', '/', path).replace(CUT + '.rx2', '.rx2')
        try:
            return super().resolve(path)
        except MissingMarqueeAsset:
            return LIVING_FILES.resolve(path.replace('/marquee/', '/livingworld/'))

    def read(self, path):
        entry = self.resolve(path)
        data = (LIVING if entry in LIVING.entries else self.archive).read(entry)
        return split(data) if path.endswith('.xml') else data


s1mesh.install(retail_character.RX2, retail_character)
parse = retail_character.RX2.parse_rx2


def parse_rx2(path, geometry=True):
    """Part <slot>~<i> reads mesh i of its file, <slot> mesh 0 (see split)."""
    result = parse(path, geometry)
    meshes = [m for m in result['meshes'] if m.get('positions')] if geometry else result['meshes']
    if len(meshes) > 1:
        slot = Path(path).parent.name
        i = int(slot.split(SPLIT)[1]) if SPLIT in slot else 0
        result['meshes'] = meshes[i:i + 1]
    return result


decode = native_roster.decode_png


def decode_png(parser, src, dest):
    """A CUT texture: the transparency map's alpha as grey (native_roster reads a cut map's red)."""
    decode(parser, src, dest)
    if Path(dest).stem.endswith(CUT):
        alpha = Image.open(dest).getchannel('A')
        Image.merge('RGB', (alpha, alpha, alpha)).save(dest)


retail_character.RX2.parse_rx2 = parse_rx2
# native_roster and character_glb bound decode_dense_morphs before s1mesh.install replaced it.
native_roster.decode_dense_morphs = character_glb.decode_dense_morphs = retail_character.decode_dense_morphs
native_roster.decode_png = decode_png
native_roster.BigArchive = Big
native_roster.Resources = Resources
scene = Big(s1game/'data/big/scene.big')
bank = next(e for e in scene.entries if e.path.endswith('/Basic_Basics_Tutorial_S01_01.abin'))
BANK, _, HIER, _, POSES = s1abin.load(scene.read(bank))
(REF,) = POSES.values()
REF = s1abin.sample(BANK, HIER, REF, 0)
FACE = range(78, 126)  # FACE .. UPPERLIP (research 4.3a)


class SkeletonSet(retail_character.SkeletonSet):
    """The marquee rx2 rigs hang the face leaves straight off HEAD (JAW, LOWERLIP, LEFTEYE ...) and lack FACE and the
    17 OFFSET_* joints the scene clips key. The face takes the bank's parents; a joint the model lacks binds at its
    parent's bind o the bank's reference local. It skins nothing, so the rest pose is the model's own (the rebuilt
    chain lands on the models' leaf binds to 5e-4 in rotation, 0-3.5 cm in position: each face's proportions)."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        names, parents = HIER['names'], HIER['parents']
        for i in FACE:
            if names[i] not in self.bind:
                _, q, t = REF.get(i, ((1, 1, 1), (0, 0, 0, 1), (0, 0, 0)))
                local = retail_character.quat_matrix(q)
                local[3, :3] = t
                self.bind[names[i]] = local @ self.bind[names[parents[i]]]  # parents come first (78 hangs off HEAD)

    def _rx2_parent_names(self):
        out = super()._rx2_parent_names()
        out.update({HIER['names'][i]: HIER['names'][HIER['parents'][i]] for i in FACE})
        return out


character_glb.SkeletonSet = SkeletonSet
native_roster.convert = functools.partial(native_roster.convert, extra_bones=set(HIER['names']), stock=source/'private/stock')


# ---------------------------------------------------------------- main
OUT = assets/'private/challenges-skate1'
for folder in ('cast', 'anim1'):
    shutil.rmtree(OUT/folder, ignore_errors=True)
(OUT/'cast').mkdir(parents=True)
entities = {}
for key in ENTITIES:
    e, model = merged(ENTITY, key), merged(*ref(merged(ENTITY, key)[MODEL_REF]))
    name = model[MODEL_NAME]['data']
    # The remap row's own fields: its class default row holds the pedestrian graph's nodes (NPC_*, SG_*), which
    # motiongraph_marquee.xml and MotionGraph_Photographer.xml do not have (research 4.2).
    remap = {hex_key(k): v for k, v in ROWS[ref(e[REMAP_REF])]['fields'].items()}
    entities[key] = {'name': e[NAME]['data'], 'model': name, 'category': int(model[CATEGORY]['data'], 16),
                     'draw': name,
                     'remap': {NODES.get(k, 'Hash_' + k): texts(v) for k, v in remap.items()
                               if v['type'] == 'EA::Reflection::Text'}}
    assert entities[key]['draw'] in CAST, entities[key]
anim = Big(s1game/'data/big/anim.big')
peds = anim.read(next(e for e in anim.entries if e.path.endswith('/pedestriansDB.abin')))
clips = s1abin.load(peds)[3]
for e in entities.values():
    # The remap's clip lists stay `remap`; its other text fields (the motion graph the NPC runs, flags: research 4.2)
    # go to `texts`. A list naming some clips but not all would be a clip pedestriansDB lacks: fail.
    hits = {k: [c in clips for c in v] for k, v in e['remap'].items()}
    assert all(all(h) or not any(h) for h in hits.values()), (e['name'], hits)
    e['texts'] = {k: v for k, v in e['remap'].items() if not any(hits[k])}
    e['remap'] = {k: v for k, v in e['remap'].items() if any(hits[k])}
wanted = sorted({c for e in entities.values() for v in e['remap'].values() for c in v} |
                {c for c in clips if c.startswith('PHOT_')})
cast = {'models': {k: f'cast/{k}.glb' for k in CAST}, 'clips': {}, 'entities': entities}
for name in wanted:
    flags = clips[name]['flags']
    # Bit 28 looping (INFERRED, research 4.3a: every *_CYC). Bit 29 channel animation: the translation w comes from the
    # header (0x822628e8), which s1abin does not apply (PRO_PR_POSS1/2_R_0_N; the node reading them is UNTRACED).
    cast['clips'][name] = {**s1abin.export_clip(peds, name, OUT/'anim1'), 'looping': bool(flags >> 28 & 1),
                           'channel': bool(flags >> 29 & 1)}
with tempfile.TemporaryDirectory(prefix='skate1-cast-') as work:
    work = Path(work)
    rows = [{'class': 'characters_marquee', 'key': 'ip', 'parent': '', 'fields': {}}]
    rows += [{'class': 'characters_marquee', 'key': k, 'parent': 'ip', 'fields': {RECIPE_FIELD: {'data': k}}} for k in CAST]
    (work/'collections.json').write_text(json.dumps({'collections': rows}))
    library = work/'library'
    for item in native_roster.prepare(s1game, assets, library, work/'collections.json', work/'build'):
        if item['status'] != 'ready':
            raise SystemExit(f"{item['key']}: {item.get('error')}")
        entry = library/'entries'/item['id']
        shutil.copyfile(entry/'character.glb', OUT/f"cast/{item['key']}.glb")
        shutil.copyfile(entry/'preview.png', OUT/f"cast/{item['key']}.png")
        print('cast', item['key'])
(OUT/'cast.json').write_text(json.dumps(cast, indent=1), encoding='utf-8')
print(f"{len(CAST)} models, {len(cast['clips'])} clips, {len(entities)} entities")
