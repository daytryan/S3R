"""Skate 2 tutorial cast (Slappy, Reda, the arriving pros) as GLBs on Skate 3's skeleton.

Skate 2's marquee.big has Skate 3's recipe/marquee/<name>.xml + model/<recipe>/<slot>/*.rx2 layout, so
tools/asset_pipeline/native_roster.prepare builds them unchanged once it reads the classic BIG archive.
Output: ASSETS/private/skate2-tutorial/cast/<key>.glb (+ preview.png).
The models keep the scene rig's face, finger and twist bones as extra joints (the cutscene clips animate them), named
by SOURCE/private/skate2-tutorial/scene/anim/*.abin. SOURCE (default ASSETS, read only) also gives private/stock.

usage: py old_tutorial_cast.py REPO S2ROOT ASSETS [SOURCE]
"""
import functools, json, re, shutil, struct, sys, tempfile
from pathlib import Path

repo, s2game, assets = map(Path, sys.argv[1:4])
source = Path(sys.argv[4]) if len(sys.argv) > 4 else assets
sys.path[:0] = [str(repo), str(repo/'tools'), str(Path(__file__).resolve().parent)]
from s2big import Big
from tools.asset_pipeline import character_glb, customiser_lighting, native_roster, retail_character
from tools.asset_pipeline.environment import Collections
from tools.asset_pipeline.retail_character import ABIN
from tools.extract_default_skater import import_rx2_parser
sys.path.insert(0, str(repo/'tools/vendor/utt'))
import mdl_parser


def scene_bones(path):
    """A bank's bone names (FastString, 5 words each, after the hierarchy's part records)."""
    data = path.read_bytes(); h = ABIN.AbinFile(data).hierarchy
    base = h.skel_base + 8 + 8 * h.num_bones + 36 * h.num_parts
    return [''.join(ABIN._decode6(struct.unpack_from('>I', data, base + 20 * i + 4 * w)[0]) for w in range(5))
            for i in range(h.num_bones)]

# Spawned by the tutorial: GlobalSpawnCharacterInfo (slappy, giovanni_reda), the ProsArrival spawns, and the
# scene-only actors (rob_dyrdek, ai_skater_06; ai_skater_01/02 in goto_m_1b_01_goto_school).
# attiba_jefferson_prop: Atiba's livingworld_models recipe and his scenes' (field 0x62F7C585686F3371): his camera.
# giovanni_reda_prop: Reda's scenes' recipe (his video camera; skate2_cast shows it only in scenes).
CAST = ['slappy', 'giovanni_reda', 'giovanni_reda_prop', 'attiba_jefferson', 'attiba_jefferson_prop', 'mike_carroll',
        'eric_koston', 'rob_dyrdek', 'ai_skater_06', 'ai_skater_01', 'ai_skater_02']
RECIPE_FIELD = 'Hash_6C9F05D8DBE7A492'



class Resources(native_roster.Resources):
    """Skate 2 recipes also list LOD-less <mod> blocks naming alternate material instances; the LOD's own
    matinst decides, so they are dropped."""
    def read(self, path):
        data = super().read(path)
        if path.endswith('.xml'):
            # ai_skater_01's Pants spells the LOD-less one `<matinst />`.
            data = re.sub(rb'<mod>\s*<matinst(?:\s*/>|>.*?</matinst>)\s*</mod>', b'', data, flags=re.S)
            data = self.borrow_diffuse(data)
        return data

    def borrow_diffuse(self, data):
        """Reda's default shirt diffuse (0x00000bd8) is not on the disc; an alternate shirt material with the same
        normal map is, so a material missing its diffuse takes that one's."""
        mats = re.findall(rb'<mat id="[^"]+"[^>]*>.*?</mat>', data, flags=re.S)
        texture = lambda m, chn: (re.search(rb'<sp id="(0x[0-9a-f]+)" chn="' + chn + rb'"', m) or [None, None])[1]
        present = lambda t: t and f"data/content/marquee/texture/{t.decode()}.rx2" in self.entries
        for mat in mats:
            diffuse, normal = texture(mat, b'diffuse'), texture(mat, b'normal')
            if diffuse and not present(diffuse):
                donor = next((texture(m, b'diffuse') for m in mats if m is not mat and normal and texture(m, b'normal') == normal
                              and present(texture(m, b'diffuse'))), None)
                if donor:
                    data = data.replace(mat, mat.replace(diffuse, donor))
        return data


class SkeletonSet(retail_character.SkeletonSet):
    """The marquee rig's Left_Prop / Right_Prop are roots, and character_glb keeps an extra joint only under a parent it
    already has: hung off TRAJECTORY, Atiba's camera (skinned to Right_Prop) keeps its own joint for skate2_cast to
    pose (user 10-29) instead of folding into SKATEBOARD_ROOT. Here, not in character_glb.py: three other stages
    fingerprint that file."""
    def _rx2_parent_names(self):
        out = dict(super()._rx2_parent_names())
        out.update({n: 'TRAJECTORY' for n in ('LEFT_PROP', 'RIGHT_PROP') if n in out and out[n] is None})
        return out


character_glb.SkeletonSet = SkeletonSet
native_roster.BigArchive = Big
native_roster.Resources = Resources
SCENE_BONES = {b for p in (source/'private/skate2-tutorial/scene/anim').glob('*.abin') for b in scene_bones(p)}
if not SCENE_BONES:
    raise SystemExit('no scene banks under ' + str(source/'private/skate2-tutorial/scene/anim'))
native_roster.convert = functools.partial(native_roster.convert, extra_bones=SCENE_BONES, stock=source/'private/stock')
OUT = assets/'private/skate2-tutorial/cast'
OUT.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='skate2-cast-') as work:
    work = Path(work)
    rows = [{'class': 'characters_marquee', 'key': 'ip', 'parent': '', 'fields': {}}]
    rows += [{'class': 'characters_marquee', 'key': k, 'parent': 'ip', 'fields': {RECIPE_FIELD: {'data': k}}} for k in CAST]
    (work/'collections.json').write_text(json.dumps({'collections': rows}))
    library = work/'library'
    report = native_roster.prepare(s2game, assets, library, work/'collections.json', work/'build')
    for item in report:
        if item['status'] != 'ready':
            raise SystemExit(f"{item['key']}: {item.get('error')}")
        entry = library/'entries'/item['id']
        shutil.copyfile(entry/'character.glb', OUT/f"{item['key']}.glb")
        shutil.copyfile(entry/'preview.png', OUT/f"{item['key']}.png")
        print('cast', item['key'])


def lighting(resources, vault, rx2, key):
    """retail_character.rs's rows for one cast model (user 10-29 bug 1: without them the cast kept the glTF's PBR
    material, lit by the flat ambient only): each slot's character shader rows from Skate 3's vault, as
    old_skate_clothing.py lights Skate 2's clothing; no specular map (no Skate 2 marquee material has one); a hair's
    alpha map as its coverage, cut on the second UV set (customiser_lighting.prepare's mask)."""
    root = resources.recipe(key)
    materials = {m.attrib['id']: m for m in root.findall('mat')}
    rows = {}
    for comp in root.findall('comp'):
        slot = comp.attrib['n']
        lod = next(l for l in comp.find('mod').findall('lod') if l.get('idx') == '0')
        raw = resources.read(f"data/content/marquee/model/{key}/{slot}/{lod.attrib['arenaid']}.rx2")
        textures = {s.attrib['chn']: s.attrib['id'] for s in materials[lod.find('matinst/matvar').attrib['id']].findall('sp')}
        data = customiser_lighting.material_data(raw, vault, mdl_parser)
        if 'alpha' in textures:
            (texture,) = rx2.parse_rx2(resources.read(f"data/content/marquee/texture/{textures['alpha']}.rx2")).textures
            mask = OUT/f'{key}_{slot}_alpha.png'
            texture.to_pil().convert('RGB').getchannel('R').save(mask)
            data['opacity'] = mask.relative_to(assets).as_posix()
        rows['Retail_' + slot] = data
    return rows


# skate2_cast gives each member's root the key "skate2-tutorial/<model>"; retail_character::load files these under it.
resources = Resources(Big(s2game/'data/content/marquee.big'))
vault = Collections(json.loads((source/'private/stock/skater-collections.json').read_text()))
rx2 = import_rx2_parser(repo/'tools/vendor/utt')
(OUT/'lighting.json').write_text(json.dumps({key: lighting(resources, vault, rx2, key) for key in CAST}))
print('cast lighting', len(CAST))
