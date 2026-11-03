"""Skate 1 / Skate 2 skies and lighting environments for the converted old-skate maps.

Writes assets/private/native-skies/<map>.{json,rgba,sun.rgba} (backups in native-skies/pre-oldsky/);
the engine loads them per map (sky dome, sun direction for the character light and shadows, fog).

Skate 2 (New San Vanelona and the Skate_2_* maps): the disc's own dome (miscboot.big
world/models/DIST_skybox), render location 'default' (sun_position, material_fog) and
material_sky 'default' (m_params). Its sky_defaultPS (render.big sky_defaultPS.updb) is
    linear = (lerp(luma(g), g, p1.w)*p1.rgb + detail - 0.5)^2 + sun^2/(sun.a+0.01), times p0.y
so gradient + detail - 0.5 is baked into the one panorama the engine's sky shader squares.

Skate 1 (Skate_1_San_Vanelona): the disc's dome, whose UVs wrap the panorama twice (unrolled
here to one copy per direction), the 'sky' material's sun-side brightening
pow(max(dot(L, v), 0), p.x)*p.y + p.z and the additive sun sprite (transparentincandescent:
rgb*a, One+One). Skate 1 renders in gamma space with no tone curve, so its display colours go
through the inverse of the engine's tone curve at exposure E_REF. Sun = the sprite's direction;
fog = the 'default' material's fog_near_taper_max / fog_colour.

usage (numpy + Pillow): py old_skies.py REPO SKATE2_ROOT SKATE1_ROOT ASSETS
"""
import hashlib, json, math, shutil, struct, sys, tempfile
from pathlib import Path
import numpy as np

repo, s2game, s1game, assets = map(Path, sys.argv[1:5])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools/vendor/utt'), str(here)]
import mdl_parser, rx2_parser
from s2big import Big
from tools.asset_pipeline.vlt import convert, hash64
from tools.asset_pipeline.environment import NAMES, Collections, fog_parameters

SKATE2_MAPS = ['New_San_Vanelona', 'Skate_2_Center', 'Skate_2_Factory', 'Skate_2_Maloof',
               'Skate_2_Parkade', 'Skate_2_School']
# Both old games centre the dome on the camera; the engine anchors it at a fixed height, so use
# each map's spawn height (<map>-load.log) plus the camera's.
ANCHOR = {'New_San_Vanelona': 49.7, 'Skate_2_Center': 373.8, 'Skate_2_Factory': 650.5,
          'Skate_2_Maloof': 634.7, 'Skate_2_Parkade': -49.0, 'Skate_2_School': 407.3,
          'Skate_1_San_Vanelona': 49.8}
CAMERA = 1.7
# ponytail: Skate 1 is matched to the engine's baseline exposure; the meter adapts around it.
# If the Skate 1 sky reads too bright/dark in game, this is the knob (multiplier = X / E_REF).
E_REF = 2.5
OUT = assets/'private/native-skies'


def vault(stem):
    names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
    return Collections(convert(stem, stem, [*names, *NAMES]))


def floats(raw):
    raw = bytes.fromhex(raw)
    return list(struct.unpack('>%df' % (len(raw)//4), raw[:len(raw)//4*4]))


def sky_model(raw, textures_raw):
    """(dome mesh, other meshes, {shader: {channel: texture}}): textures resolved by the GUID each
    material names (the parser's mesh<->material pairing is unreliable for these files)."""
    model = mdl_parser.parse_rx2(raw)
    table = rx2_parser.parse_rx2(textures_raw)
    guid = {}
    for section in table.entries:
        if section.type_id == 0x00eb000b:
            n, start = struct.unpack_from('>2I', table.data, section.f0)
            for i in range(n):
                _, _, hi, lo, _, index = struct.unpack_from('>6I', table.data, section.f0+start+i*24)
                guid[(hi << 32) | lo] = next(t for t in table.textures if t.index == index)
    materials, current = {}, None
    for p in model.materials:
        if p.kind == 'AttribulatorMaterialName':
            current = materials.setdefault(p.value, {})
        elif current is not None and p.kind != 'Name' and '_0x' in p.value:
            current[p.kind] = guid[int(p.value.rsplit('_0x', 1)[1], 16)]
    dome = max(model.meshes, key=lambda m: np.abs(m.vertices).max())
    assert np.abs(dome.vertices[:, [0, 2]].mean(0)).max() < 0.05*np.abs(dome.vertices).max(), 'dome not centred'
    return dome, [m for m in model.meshes if m is not dome], materials


def rgba(texture):
    return np.frombuffer(bytes(texture.rgba), np.uint8).reshape(texture.height, texture.width, 4).astype(np.float64)/255


def sample(img, u, v, wrap_u):
    """Bilinear, U wrapped or clamped, V clamped (texel centres at (i+0.5)/size)."""
    h, w = img.shape[:2]
    x, y = u*w - 0.5, np.clip(v*h - 0.5, 0, h - 1)
    if not wrap_u:
        x = np.clip(x, 0, w - 1)
    x0, y0 = np.floor(x).astype(int), np.floor(y).astype(int)
    fx, fy = (x - x0)[..., None], (y - y0)[..., None]
    x1, y1 = x0 + 1, np.minimum(y0 + 1, h - 1)
    x0, x1 = (x0 % w, x1 % w) if wrap_u else (np.clip(x0, 0, w - 1), np.clip(x1, 0, w - 1))
    top = img[y0, x0]*(1 - fx) + img[y0, x1]*fx
    return top*(1 - fy) + (img[y1, x0]*(1 - fx) + img[y1, x1]*fx)*fy


def texel_directions(mesh, uv, w, h):
    """Unit dome direction behind every texel: the mesh rasterised in UV space (per-triangle
    affine, as the GPU interpolates). Uncovered texels (never sampled) point straight up."""
    out = np.zeros((h, w, 3))
    out[..., 1] = 1
    px, py = uv[:, 0]*w - 0.5, uv[:, 1]*h - 0.5
    for a, b, c in mesh.faces:
        xs, ys = px[[a, b, c]], py[[a, b, c]]
        det = (ys[1] - ys[2])*(xs[0] - xs[2]) + (xs[2] - xs[1])*(ys[0] - ys[2])
        if abs(det) < 1e-9:
            continue
        x0, x1 = max(0, math.floor(xs.min())), min(w - 1, math.ceil(xs.max()))
        y0, y1 = max(0, math.floor(ys.min())), min(h - 1, math.ceil(ys.max()))
        X, Y = np.meshgrid(np.arange(x0, x1 + 1), np.arange(y0, y1 + 1))
        l0 = ((ys[1] - ys[2])*(X - xs[2]) + (xs[2] - xs[1])*(Y - ys[2]))/det
        l1 = ((ys[2] - ys[0])*(X - xs[2]) + (xs[0] - xs[2])*(Y - ys[2]))/det
        inside = (l0 >= -1e-6) & (l1 >= -1e-6) & (1 - l0 - l1 >= -1e-6)
        l0, l1 = l0[inside, None], l1[inside, None]
        p = l0*mesh.vertices[a] + l1*mesh.vertices[b] + (1 - l0 - l1)*mesh.vertices[c]
        out[Y[inside], X[inside]] = p/np.linalg.norm(p, axis=1, keepdims=True)
    return out


def sprite(directions, mesh, texture):
    """Additive rgb*a of a textured quad seen along each direction from the dome centre."""
    img, total = rgba(texture), np.zeros(directions.shape[:2] + (3,))
    d = directions.reshape(-1, 3)
    for a, b, c in mesh.faces:
        A, B, C = mesh.vertices[[a, b, c]].astype(np.float64)
        n = np.cross(B - A, C - A)
        with np.errstate(divide='ignore', invalid='ignore'):
            t = (n @ A)/(d @ n)
        X = t[:, None]*d
        v0, v1, v2 = B - A, C - A, X - A
        d00, d01, d11 = v0 @ v0, v0 @ v1, v1 @ v1
        d20, d21 = v2 @ v0, v2 @ v1
        den = d00*d11 - d01*d01
        s1, s2 = (d11*d20 - d01*d21)/den, (d00*d21 - d01*d20)/den
        inside = (t > 0) & (s1 >= 0) & (s2 >= 0) & (s1 + s2 <= 1)
        uv = (1 - s1 - s2)[:, None]*mesh.uvs[a] + s1[:, None]*mesh.uvs[b] + s2[:, None]*mesh.uvs[c]
        texel = sample(img, uv[inside, 0], uv[inside, 1], False)
        flat = total.reshape(-1, 3)
        flat[inside] = texel[:, :3]*texel[:, 3:]
    return total


def inverse_tone(display):
    """Engine tone pass: display = sqrt(2x - x^2) for x <= 1 (retail_tone.wgsl), x = exposed linear."""
    return 1 - np.sqrt(np.clip(1 - np.square(np.clip(display, 0, 1)), 0, 1))


def write(name, width, height, panorama, sun, dome, uvs, environment, source):
    for suffix in ('.json', '.rgba', '.sun.rgba'):
        old = OUT/(name + suffix)
        if old.is_file() and not (OUT/'pre-oldsky'/old.name).is_file():
            (OUT/'pre-oldsky').mkdir(exist_ok=True)
            shutil.copy2(old, OUT/'pre-oldsky'/old.name)
    (OUT/(name + '.rgba')).write_bytes(panorama.tobytes())
    (OUT/(name + '.sun.rgba')).write_bytes(sun.tobytes())
    (OUT/(name + '.json')).write_text(json.dumps({
        'width': width, 'height': height, 'positions': dome.vertices.tolist(), 'uvs': uvs.tolist(),
        'indices': dome.faces.reshape(-1).tolist(), 'environment': environment,
        'sun_width': sun.shape[1], 'sun_height': sun.shape[0], 'source_sha256': source,
    }, separators=(',', ':')), encoding='utf-8')
    print(name, f'{width}x{height} sky, sun {environment["sun_direction"]}, multiplier {environment["multiplier"]:.3f},',
          f'anchor {environment["anchor_height"]}, fog {environment["fog_frame"]}')


def unit(v):
    v = np.asarray(v, np.float64)
    return (v/np.linalg.norm(v)).tolist()


def as8(x):
    return np.clip(np.round(x*255), 0, 255).astype(np.uint8)


def skate2():
    boot = Big(s2game/'data/big/miscboot.big')
    entry = {e.path: e for e in boot.entries}
    raw = boot.read(entry['data/content/world/models/DIST_skybox.rx2'])
    textures_raw = boot.read(entry['data/content/world/models/DIST_skybox_Textures.rx2'])
    dome, _, materials = sky_model(raw, textures_raw)
    sky = materials['sky.default']
    db = Big(s2game/'data/db/db.big')
    with tempfile.TemporaryDirectory(prefix='skate2-db-') as work:
        for e in db.entries:
            if Path(e.path).stem == 'skater':
                (Path(work)/Path(e.path).name).write_bytes(db.read(e))
        rows = vault(Path(work)/'skater')
    location, location_chain = rows.resolve('render_locations', 'default')
    sky_cls, sky_key, _ = struct.unpack('>3Q', bytes.fromhex(location[hash64('material_sky')]['data']))
    material, material_chain = rows.resolve(sky_cls, sky_key)
    p0, p1 = (floats(i) for i in material[hash64('m_params')]['array']['items'][:2])
    fog_cls, fog_key, _ = struct.unpack('>3Q', bytes.fromhex(location[hash64('material_fog')]['data']))
    fog_row, fog_chain = rows.resolve(fog_cls, fog_key)
    fog = {'chain': fog_chain, **{n: floats(fog_row[hash64(n)]['data'])[:4 if n == 'fog_colour' else 1]
           for n in ('fog_near', 'fog_far', 'fog_colour', 'fog_power', 'fog_max')}}

    detail = rgba(sky['detail'])[..., :3]
    h, w = detail.shape[:2]
    U, V = np.meshgrid((np.arange(w) + 0.5)/w, (np.arange(h) + 0.5)/h)
    gradient = sample(rgba(sky['diffuse'])[..., :3], U, V, True)
    luma = gradient @ np.array([.3, .59, .11])
    gradient = (luma[..., None] + (gradient - luma[..., None])*p1[3])*np.array(p1[:3])
    colour = np.abs(gradient + detail - 0.5)  # squared by the shader, so the sign drops out
    scale = max(1.0, colour.max())  # keep > 1 values: store /scale, multiplier *scale^2
    panorama = np.dstack([as8(colour/scale), np.full((h, w), 255, np.uint8)])
    sun = np.frombuffer(bytes(sky['specular'].rgba), np.uint8).reshape(sky['specular'].height, -1, 4)
    assert sun.shape[0] == 16
    for name in SKATE2_MAPS:
        write(name, w, h, panorama, sun, dome, dome.uvs, {
            'world_chain': ['skate2_san_vanelona'], 'location_chain': location_chain,
            'sun_direction': unit(floats(location[hash64('sun_position')]['data'])[:3]),
            'anchor_height': ANCHOR[name] + CAMERA,
            'sky_model': 'skate2 miscboot.big world/models/DIST_skybox',
            'sky_textures': 'skate2 miscboot.big world/models/DIST_skybox_Textures',
            'fog': fog, 'material_chain': material_chain, 'sun_scale': p0[0],
            'multiplier': p0[1]*scale*scale, 'sky_tint': p1, 'fog_frame': fog_parameters(fog),
        }, hashlib.sha256(textures_raw).hexdigest())


def skate1():
    models = s1game/'data/content/world/models'
    raw, textures_raw = (models/'DIST_skybox.rx2').read_bytes(), (models/'DIST_skybox_Textures.rx2').read_bytes()
    dome, others, materials = sky_model(raw, textures_raw)
    sun_quad, = others
    rows = vault(s1game/'data/db/skater')
    MATERIAL = 'Hash_D590F15C91FBB4A1'  # the shader-constant class ('default', 'sky', 'sun', ...)
    value = lambda row, name: floats(row[hash64(name)]['data'])
    sky_row, _ = rows.resolve(MATERIAL, 'sky')
    default, _ = rows.resolve(MATERIAL, 'default')
    bias = value(sky_row, 'sky_bloom_power_multiplier_base')
    exposure = value(sky_row, 'exposure')[0]
    near, taper, maximum, power = value(default, 'fog_near_taper_max')
    fog_display = np.array(value(default, 'fog_colour')[:3])

    library = rgba(materials['model_sky']['diffuse'])[..., :3]
    library = np.concatenate([library, library], axis=1)  # the dome's u spans [-1, 1]: two copies
    h, w = library.shape[:2]
    uvs = dome.uvs.copy()
    uvs[:, 0] = (uvs[:, 0] + 1)/2
    directions = texel_directions(dome, uvs, w, h)
    light = np.array(unit(sun_quad.vertices.mean(0)))
    brightness = np.power(np.clip(directions @ light, 0, None), bias[0])*bias[1] + bias[2]
    display = library*brightness[..., None]*exposure
    display += sprite(directions, sun_quad, materials['model_transparentincandescent']['diffuse'])
    x = inverse_tone(display)
    peak = x.max()
    panorama = np.dstack([as8(np.sqrt(x/peak)), np.full((h, w), 255, np.uint8)])
    sun = np.zeros((16, 512, 4), np.uint8)  # the sprite is in the panorama
    sun[..., 3] = 255
    # Skate 1: min(saturate((d - near)*taper)^power, max). Engine: max*saturate((d - near)/(far - near))^power,
    # equal when far - near = max^(1/power)/taper.
    fog = {'chain': ['default'], 'fog_near': [near], 'fog_far': [near + maximum**(1/power)/taper],
           'fog_colour': [*(inverse_tone(fog_display)/E_REF).tolist(), 1.0],
           'fog_power': [power], 'fog_max': [maximum]}
    write('Skate_1_San_Vanelona', w, h, panorama, sun, dome, uvs, {
        'world_chain': ['skate1_san_vanelona'], 'location_chain': ['default'],
        'sun_direction': light.tolist(), 'anchor_height': ANCHOR['Skate_1_San_Vanelona'] + CAMERA,
        'sky_model': 'skate1 world/models/DIST_skybox', 'sky_textures': 'skate1 world/models/DIST_skybox_Textures',
        'fog': fog, 'material_chain': ['sky', 'default'], 'sun_scale': 1.0, 'multiplier': peak/E_REF,
        'sky_bias': bias, 'fog_frame': fog_parameters(fog),
    }, hashlib.sha256(textures_raw).hexdigest())


if __name__ == '__main__':
    assert inverse_tone(np.array(1.0)) == 1 and abs(np.sqrt(2*inverse_tone(np.array(.5)) - inverse_tone(np.array(.5))**2) - .5) < 1e-9
    skate2()
    skate1()
