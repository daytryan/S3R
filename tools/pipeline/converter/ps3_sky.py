"""Write Skate3Rust native skies for converted Skate 2 maps from the mod's PS3 San Van sky.

Dome: miscload.big world/models/DIST_skybox (sky.default); textures: DIST_svsky_Textures, which
provides the same GUIDs with the San Vanelona gradient. Sun/fog/exposure parameters come from an
existing sky exported for the same sky.default material and default render location.

usage: python ps3_sky.py REPO MISCLOAD_BIG ASSETS TEMPLATE_SKY_JSON NAME...
"""
import hashlib, json, sys
from pathlib import Path

repo, miscload, assets, template = map(Path, sys.argv[1:5])
names = sys.argv[5:]
sys.path[:0] = [str(Path(__file__).parent), str(repo), str(repo / 'tools/vendor/utt')]
import mdl_parser, ps3_textures
from tools.owned_game.big import BigArchive
from tools.asset_pipeline.sky import _material, _texture

if miscload.is_dir():
    # An extracted world/models folder with its own dome, e.g. Skate 1 San Van's
    # DIST_skybox_sanvanelona.psg + DIST_skybox_sanvanelona_Textures.psg.
    textures_path, = miscload.glob('DIST_skybox*_Textures.psg')
    raw = textures_path.with_name(textures_path.name.replace('_Textures', '')).read_bytes()
    textures_raw = textures_path.read_bytes()
else:
    archive = BigArchive(miscload)
    entries = {e.path: e for e in archive.entries}
    raw = archive.read(entries['data/content/world/models/DIST_skybox.psg'])
    textures_raw = archive.read(entries['data/content/world/models/DIST_svsky_Textures.psg'])
model = mdl_parser.parse_model(raw)
table = ps3_textures.PS3Texture(raw)
table.parse()
channels = _material(raw, table)
textures = ps3_textures.parse(textures_raw)
diffuse, sun = _texture(textures, channels['diffuse']), _texture(textures, channels['specular'])
mesh, = model.meshes
shader = next(p.value for p in model.materials if p.kind == 'AttribulatorMaterialName')
environment = json.loads(template.read_text())['environment']
assert shader == 'sky.default' and sun.height == 16 and mesh.uvs is not None
output = assets / 'private/native-skies'
for name in names:
    (output / (name + '.rgba')).write_bytes(diffuse.rgba)
    (output / (name + '.sun.rgba')).write_bytes(sun.rgba)
    (output / (name + '.json')).write_text(json.dumps({
        'width': diffuse.width, 'height': diffuse.height,
        'positions': mesh.vertices.tolist(), 'uvs': mesh.uvs.tolist(), 'indices': mesh.faces.reshape(-1).tolist(),
        'environment': environment, 'sun_width': sun.width, 'sun_height': sun.height,
        'source_sha256': hashlib.sha256(textures_raw).hexdigest(),
    }, separators=(',', ':')), encoding='utf-8')
    print(name, f'{diffuse.width}x{diffuse.height} sky, {sun.width}x16 sun')
