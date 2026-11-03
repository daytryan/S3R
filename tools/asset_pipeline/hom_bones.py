"""Skate 3's Hall of Meat bone model (marquee dem_bones_hom): one mesh per bone, skinned to the skater's skeleton.

Retail draws it on the skater as HOMSkaterPresEntity (create 0x827637D0) and shows only the damaged bones
(update 0x82763A40). Output: <out>/hom_bones.glb (one mesh per Bones_* component) and hom_bones.json (the slot names).
"""
import argparse, json, shutil, sys, tempfile
import xml.etree.ElementTree as ET
from pathlib import Path
if __name__ == '__main__':sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.owned_game.big import BigArchive
from tools.asset_pipeline.character_glb import convert
from tools.asset_pipeline.marquee_assets import Resources
from tools.asset_pipeline.native_roster import finalize_glb

RECIPE = 'dem_bones_hom'


def export(marquee, private, out):
    resources = Resources(BigArchive(marquee))
    root = ET.fromstring(resources.read(f'data/content/recipe/marquee/{RECIPE}.xml'))
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        models = Path(tmp) / 'models'
        recipe = {'components': [], 'morph_assembly': {'expected_targets': {}, 'face_targets': []}, 'preset': {'body_mods': {}}}
        for component in root.findall('comp'):
            slot = component.attrib['n']
            (mod,) = component.findall('mod')
            lod = next(l for l in mod.findall('lod') if l.get('idx') == '0')
            dest = models / slot / (lod.attrib['arenaid'] + '.rx2')
            dest.parent.mkdir(parents=True)
            dest.write_bytes(resources.read(f"data/content/marquee/model/{RECIPE}/{slot}/{lod.attrib['arenaid']}.rx2"))
            # The marquee_hom material is drawn by the game (hom_bones.rs): the glb carries geometry and skin only.
            recipe['components'].append({'slot': slot, 'tint': [1, 1, 1], 'textures': {}, 'alpha_mode': 'OPAQUE', 'coverage': False})
            recipe['morph_assembly']['expected_targets'][slot] = []
        convert(models, private, recipe, output=out / 'hom_bones.glb', geometry_only=True)
        finalize_glb(out / 'hom_bones.glb')
    (out / 'hom_bones.json').write_text(json.dumps({'slots': [c['slot'] for c in recipe['components']]}, indent=2))


if __name__ == '__main__':
    p = argparse.ArgumentParser()
    p.add_argument('--marquee', type=Path, required=True, help='the Skate 3 data/content/marquee.big')
    p.add_argument('--private', type=Path, required=True, help="the install's assets/private (stock OnBoard.abin)")
    p.add_argument('--out', type=Path, required=True)
    a = p.parse_args()
    export(a.marquee, a.private, a.out)
