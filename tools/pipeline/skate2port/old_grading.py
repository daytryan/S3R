"""Skate 1 / Skate 2 colour grading for the converted old-skate maps.

Writes assets/private/colour-grading/<map>.{json,cubes,zones}; the engine (retail_grading.rs) applies it.

Cubes: both games ship 32^3 RGB colour cubes (Skate 2 data/big/colourcubes.big, Skate 1 miscboot.big
data/colourcubes/) stored in the Xbox 3D-texture tile order. Skate 2's cc_neutral is the identity cube, so
its texels name the grid cell every file offset holds; that one permutation unscrambles every cube.
Output: RGBA8, red fastest, then green, then blue.

How the games apply them (their final pixel shaders, disassembled from Xenia's shader cache; constants
read from the live GPU command buffer of a Skate 2 frame):
  Skate 2  c = sqrt(tm/2) * 1.41 + bloom; lookup at c*31/32 + 0.5/32 (identity colour matrix).
  Skate 1  lookup at c directly (tint pass, g_bTint).
Zones: both games switch preset by area and blend over 1.8 s (Skate 2 live state). The zone volumes
weren't found, so each map is split by nearest district locator: Skate 2 = the names each
data/big/globallocators.big DIST_* file lists, placed by BAM_Sim.loc; Skate 1 = BAM.loc categories.
A Skate 2 preset is (fog, cc, sky, exp, bloom) vault rows; the ones marked live are what the running
game used there, the rest are the district's own named rows. Sub-spots (Mega Compound, Library,
Matrix, Slappy's) are circles round their locators. Per zone it carries the cube, bloom, exposure
(rendering exp_*), fog (material_fog, as the Skate 3 pipeline derives its ramp) and sky (material_sky
sun scale + multiplier relative to 'default', which the map's sky dome was built with).

Bloom (Skate 2 only; per zone): bright pass saturate((x - t)*k), saturation matrix s, then
b1*blur(L0) + b2*L1, with k = 1/(1 - t) and s = 1 + bloom_saturation (live captures: Mega Compound
t .5 -> k 2, s 2; Waterfront t .65 -> k 2.857, bloom_saturation 2 -> s 3).
Film (Skate 1 only): see skate1_film.
Lens (Skate 2 only): its last pass upscales through a fisheye, uv' = .5 + (uv - .5)*(0.9 + 0.72 r^4)
(g_Fisheye_Warp, live), times miscboot.big globaltextures.rx2's 512x2 vignette gradient sampled at
1 - (aspect-corrected distance from the centre).

usage (numpy): py old_grading.py REPO SKATE2_ROOT SKATE1_ROOT ASSETS [s2|s1]
"""
import json, re, struct, sys, tempfile
from pathlib import Path
import numpy as np

repo, s2game, s1game, assets = map(Path, sys.argv[1:5])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools/vendor/utt'), str(here)]
from s2big import Big
from tools.asset_pipeline.vlt import convert, hash64
from tools.asset_pipeline.environment import Collections, field, floats, fog_parameters, sky_parameters

OUT = assets/'private/colour-grading'
CELL = 8.0  # metres per zone-grid cell
S2_DLC = ['Skate_2_Center', 'Skate_2_Factory', 'Skate_2_Maloof', 'Skate_2_Parkade', 'Skate_2_School']
# globallocators.big file prefix -> (fog, cc, sky, exp, bloom, district title). Titles are each vault district
# row's label (class 8B82A26453CFE8E2: district_financial -> ID_DISTRICT_FINANCIAL, ...); the engine's Skate 2 HUD
# shows the title card when the zone's title changes.
S2_DISTRICTS = {
    'DIST_Downtow': ('fog_financial', 'cc_financial', 'sky_financial', 'exp_lessbright', 'bloom_financial', 'ID_DISTRICT_FINANCIAL'),  # live
    'DIST_Old_Tow': ('fog_oldtown', 'cc_oldtown', 'sky_oldtown', 'exp_oldtown', 'bloom_oldtown', 'ID_DISTRICT_OLDTOWN'),  # live (memory)
    'DIST_Project': ('fog_projects', 'cc_projects', 'sky_projects', 'exp_bkup2', 'bloom_projects', 'ID_DISTRICT_PROJECTS'),  # live (memory)
    'DIST_UrbanRe': ('fog_rez', 'cc_urbanrez', 'sky_rez', 'exp_bkup1', 'bloom_res', 'ID_DISTRICT_URBAN_RED'),  # live (memory)
    'DIST_SanVanM': ('fog_default', 'cc_svm', 'sky_svm', 'exp_default', 'bloom_svm', 'ID_DISTRICT_MOUNTAIN'),
    'DIST_SoHo': ('fog_soho', 'cc_soho', 'sky_soho', 'exp_soho', 'bloom_soho', 'ID_DISTRICT_SOHO'),
    'DIST_Waterfr': ('fog_waterfront', 'cc_waterfront', 'sky_waterfront', 'exp_bright', 'bloom_waterfront', 'ID_DISTRICT_WATERFRONT'),  # live
    'DIST_BigEven': ('fog_bigevent', 'cc_bigevent', 'default', 'exp_bigevent', 'bloom_bigevent', 'ID_DISTRICT_X_GAMES')}
# (centre x, z), radius, preset. ponytail: radii eyeballed from each spot's size; the real volumes are unknown.
S2_SPOTS = [((1136.7, -1158.8), 150.0, ('fog_default', 'cc_danny', 'sky_danny', 'exp_default', 'bloom_danny', 'ID_DISTRICT_MOUNTAIN')),  # live
            ((60.0, 760.0), 130.0, ('fog_financial', 'cc_bkup4', 'sky_financial', 'exp_lessbright', 'bloom_financial', 'ID_DISTRICT_FINANCIAL')),  # Library, live
            ((430.0, 700.0), 110.0, ('fog_financial', 'cc_bkup4', 'sky_financial', 'exp_matrix', 'bloom_exp5', 'ID_DISTRICT_FINANCIAL')),  # Matrix, live
            ((1665.0, -545.0), 90.0, ('fog_slappy', 'cc_slappy', 'sky_slappy', 'exp_slappy', 'bloom_slappy', 'ID_DISTRICT_PROJECTS'))]
S1_DISTRICTS = {'DIST_financial': 'cc_financial', 'DIST_Suburbs': 'cc_suburbs', 'DIST_Old_Town': 'cc_oldtown',
                'DIST_UrbanRes': 'cc_urbanrez', 'DIST_Outskirts': 'cc_outskirts', 'Financial_XGames': 'cc_xgames',
                'Old_Town_PlanB': 'cc_planb'}


def untile(neutral):
    """File texel -> (r, g, b) grid cell, from the identity cube."""
    q = np.rint(np.frombuffer(neutral, np.uint8).reshape(-1, 3).astype(int) * 31 / 255).astype(int)
    cell = q[:, 0] + 32*q[:, 1] + 1024*q[:, 2]
    assert len(np.unique(cell)) == 32**3, 'cc_neutral is not a permutation of the grid'
    return cell


def cube(raw, cell):
    out = np.zeros((32**3, 4), np.uint8)
    out[cell, :3] = np.frombuffer(raw, np.uint8).reshape(-1, 3)
    out[:, 3] = 255
    return out.tobytes()


def locations(xml):
    for m in re.finditer(r'<Location>(.*?)</Location>', xml, re.S):
        b = m.group(1)
        n, t = re.search(r'<Name>(.*?)</Name>', b), re.search(r'<Transform>(.*?)</Transform>', b)
        c = re.search(r'<Category>(.*?)</Category>', b)
        if n and t:
            v = [float(x) for x in t.group(1).split(',')]
            yield n.group(1), (v[12], v[14]), c.group(1) if c else None


def grid(points, labels, overrides=()):
    """Nearest-labelled-point zone index per CELL-sized square (x, z), 300 m past the outermost points."""
    p = np.array(points, np.float64)
    x0, z0 = p.min(0) - 300
    w, h = np.ceil((p.max(0) + 300 - (x0, z0))/CELL).astype(int)
    xs, zs = x0 + (np.arange(w) + 0.5)*CELL, z0 + (np.arange(h) + 0.5)*CELL
    out = np.zeros((h, w), np.uint8)
    lab = np.array(labels, np.uint8)
    for j, z in enumerate(zs):
        d = (xs[:, None] - p[None, :, 0])**2 + (z - p[None, :, 1])**2
        out[j] = lab[d.argmin(1)]
    for (cx, cz), r, zone in overrides:
        X, Z = np.meshgrid(xs, zs)
        out[(X - cx)**2 + (Z - cz)**2 < r*r] = zone
    return {'x0': float(x0), 'z0': float(z0), 'cell': CELL, 'width': int(w), 'height': int(h)}, out.tobytes()


def write(name, cubes, zones, lookup, bloom, zone_grid=None, lens=None, film=None):
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT/(name + '.cubes')).write_bytes(b''.join(c for _, c in cubes))
    meta = {'cubes': [n for n, _ in cubes], 'lookup': lookup, 'bloom': bloom, 'zones': zones, 'grid': None, 'lens': lens,
            'film': film}
    if zone_grid:
        meta['grid'], data = zone_grid
        (OUT/(name + '.zones')).write_bytes(data)
    (OUT/(name + '.json')).write_text(json.dumps(meta, indent=1), encoding='utf-8')
    print(name, len(cubes), 'cubes,', len(zones), 'zones', '' if not zone_grid else '%dx%d grid' % (meta['grid']['width'], meta['grid']['height']))


def vignette():
    """Row 0 RGB of the 512x2 lens vignette gradient (globaltextures.rx2), the row the fisheye pass samples."""
    import rx2_parser
    boot = Big(s2game/'data/big/miscboot.big')
    table = rx2_parser.parse_rx2(boot.read(next(e for e in boot.entries if e.path.endswith('textures/globaltextures.rx2'))))
    tex, = [t for t in table.textures if (t.width, t.height) == (512, 2)]
    return np.frombuffer(bytes(tex.rgba), np.uint8).reshape(2, 512, 4)[0, :, :3].tobytes()


def skate2_bloom(rows, key):
    f, _ = rows.resolve('Hash_C57D67ABB410B966', key)
    v = {n: struct.unpack('>f', bytes.fromhex(f[hash64(n)]['data']))[0]
         for n in ('bloom_threshold', 'bloom_1', 'bloom_2', 'bloom_saturation')}
    t = v['bloom_threshold']
    return {'threshold': t, 'scale': 1/(1 - t), 'level0': v['bloom_1'], 'level1': v['bloom_2'], 'saturation': 1 + v['bloom_saturation']}


def skate2_preset(rows, fog, sky, exp, sky_default):
    """Exposure (target, min, max, damping), fog ramp/colour, sky (sun scale, multiplier / default's)."""
    f, _ = rows.resolve('Hash_1FFDC8E3ACA07C1F', exp)
    auto, _ = rows.resolve('Hash_1FFDC8E3ACA07C1F', 'auto_exposure')
    exposure = [struct.unpack('>f', bytes.fromhex(f.get(hash64(k), auto[hash64(k)])['data']))[0]
                for k in ('auto_exposure_' + n for n in ('target_luminance', 'min', 'max', 'damping'))]
    f, _ = rows.resolve('material_fog', fog)
    fog = fog_parameters({n: floats(field(f, n), 4 if n == 'fog_colour' else 1)
                          for n in ('fog_near', 'fog_far', 'fog_colour', 'fog_power', 'fog_max')})
    s = sky_parameters(rows, 'sky.' + sky)
    return {'exposure': exposure, 'fog': fog, 'sky': [s['sun_scale'], s['multiplier']/sky_default]}


def skate2():
    big = Big(s2game/'data/big/colourcubes.big')
    raw = {Path(e.path).stem: big.read(e) for e in big.entries}
    cell = untile(raw['cc_neutral'])
    db = Big(s2game/'data/db/db.big')
    with tempfile.TemporaryDirectory(prefix='skate2-db-') as work:
        for e in db.entries:
            if Path(e.path).stem == 'skater':
                (Path(work)/Path(e.path).name).write_bytes(db.read(e))
        names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
        rows = Collections(convert(Path(work)/'skater', Path(work)/'skater', names))

    presets = list(dict.fromkeys([*S2_DISTRICTS.values(), *(p for _, _, p in S2_SPOTS)]))
    cubes = list(dict.fromkeys(c for _, c, _, _, _, _ in presets))
    sky_default = sky_parameters(rows, 'sky.default')['multiplier']
    zones = [{'name': '/'.join((c, e, b)), 'cube': cubes.index(c), 'bloom': skate2_bloom(rows, b), 'district': d,
              **skate2_preset(rows, f, s, e, sky_default)} for f, c, s, e, b, d in presets]
    world = Big(s2game/'data/content/worldbam.big')
    placed = {n: xz for n, xz, _ in locations(world.read(next(e for e in world.entries if e.path.endswith('BAM_Sim.loc'))).decode('latin-1'))}
    glob = Big(s2game/'data/big/globallocators.big')
    points, labels = [], []
    for e in glob.entries:
        district = Path(e.path).name.split('_[')[0]
        if district not in S2_DISTRICTS:
            continue
        for n in sorted({m.group().decode() for m in re.finditer(rb'[A-Za-z0-9_:]{6,}', glob.read(e))} & placed.keys()):
            points.append(placed[n]); labels.append(presets.index(S2_DISTRICTS[district]))
    city = grid(points, labels, [(xz, r, presets.index(p)) for xz, r, p in S2_SPOTS])
    lookup = {'scale': 31/32, 'offset': 0.5/32}
    lens = {'warp': [0.9, 0.72], 'vignette': vignette().hex()}
    write('New_San_Vanelona', [(c, cube(raw[c], cell)) for c in cubes], zones, lookup, True, city, lens)
    # The DLC parks use render location dlc_default: colour_cube default (cc_financial), the 'bloom' row, and
    # auto_exposure 'auto_exposure', material_fog 'default', material_sky 'default' (its child Hash_01009DA3D8B0B8B2
    # names the same three).
    default = [{'name': 'default', 'cube': 0, 'bloom': skate2_bloom(rows, 'bloom'),
                **skate2_preset(rows, 'default', 'default', 'auto_exposure', sky_default)}]
    for name in S2_DLC:
        write(name, [('cc_financial', cube(raw['cc_financial'], cell))], default, lookup, True, lens=lens)
    return cell


def skate1(cell):
    big = Big(s1game/'data/big/miscboot.big')
    raw = {Path(e.path).stem: big.read(e) for e in big.entries if '/colourcubes/' in e.path}
    cubes = list(dict.fromkeys(S1_DISTRICTS.values()))
    points, labels = [], []
    for n, xz, cat in locations((s1game/'data/content/world/stream/BAM.loc').read_text(encoding='latin-1')):
        if cat in S1_DISTRICTS:
            points.append(xz); labels.append(cubes.index(S1_DISTRICTS[cat]))
    zones = [{'name': c.removeprefix('cc_'), 'cube': i, 'bloom': S1_BLOOM} for i, c in enumerate(cubes)]
    write('Skate_1_San_Vanelona', [(c, cube(raw[c], cell)) for c in cubes], zones, {'scale': 1.0, 'offset': 0.0}, True,
          grid(points, labels), film=skate1_film('Skate_1_San_Vanelona'))


# Skate 1's final pass (shader B699240D, disassembled in Xenia's cache) draws the frame through a fisheye it
# upscales to fill the screen, a radial vignette, a grunge vignette texture fading in toward the edges and film
# noise. Its numbers are vault rows (data/db/skater.vlt). The 'fisheye' row (class 1FFDC8E3ACA07C1F) holds ten
# floats for the disc shader's ten constants (final/postfx.psh.o: adjust_h/v_ratio, h/v_upscaler,
# h/v_warp_multiplier, warp_power, warp_multiplier, vignette_power, vignette_multiplier); the field names are
# stripped, so each hash is matched by pairing and value. ponytail: the pairing is inferred -- warp 0.3 * r^3
# upscaled 0.9 lands the corners exactly on the screen edge, which is what the upscaler is for; a live capture
# of c0-c15 would confirm it.
S1_FISHEYE = {'h_warp': 0xAF65240EB61442FD, 'v_warp': 0x59F7C644DEF5B7A8, 'warp_power': 0x4C6FDC39C7C08995,
              'h_upscaler': 0x5905218EC008C9CA, 'v_upscaler': 0xBD4FD11D8686A431,
              'vignette_multiplier': 0x13B56EFC09EF308F, 'vignette_power': 0x715478E1C89B25BF}
# postfx 'main' row: (1, 1, 1, 0.288), the grunge vignette's tint and strength; depth of field (its composite
# E8DC writes a depth blur amount the final pass mixes toward the blurred frame by): (-1, 0.0001, 60, 300) = no
# near blur, far blur from 60 m to 300 m, next to maxCoC 2 (blur radius, quarter-resolution pixels).
S1_GRUNGE = 0x3FB0A443D6663EA6
S1_DOF = 0xF53954F62983A1C0
S1_GRAIN = 0.02  # ponytail: per-pixel grain; the Xenia frames' pixel noise is ~0.4%, their visible mottling is the grunge
# ponytail: vignette 1 - m*r^p (r = 1 in the corners) fitted to three Xenia frames' brightness by distance from the
# centre (0.84 at r .5, 0.56 at .7, 0.15 at .9, grunge included): the vault's 0.226 / 4 read as that undershoots
# badly, so the pairing of those two fields (or the game's r scale) is not known yet. A live c0-c15 capture fixes it.
S1_VIGNETTE = [1.1, 3.0]
# Skate 1 has no eye adaptation (its world shader multiplies by the 'default' material's exposure, 1.0), but that
# is in Skate 1's light units: the converted map is lit on Skate 3's scale, and pinning the engine's meter at 2.5
# overexposed every sunny scene (user, 2026-09-30). None keeps the engine's auto meter; set a number to fix it.
S1_EXPOSURE = None
# Skate 1's bloom: its world shaders write luminance/8 to alpha and its bright pass (C3FA8514, run as such by
# retail_grading.wgsl when the film flag is set) keeps colour * saturate((alpha - bloom_threshold)/luminance), with
# the postfx 'main' row's bloom_threshold 0.05: at most ~0.075 even for white. The two blurred levels are summed
# with weights level0 + level1 = its bloom multiply. ponytail: that multiply was never captured; 4.0 (one of the
# 'main' row's unnamed floats) gives white +0.3. Raise both levels for more glow, lower for less.
S1_BLOOM = {'threshold': 0.05, 'scale': 1.0, 'level0': 2.0, 'level1': 2.0, 'saturation': 1.0}
# The values above stay in the JSON for builds before the 2026-10-03 audit; current builds read film['s1'] (below).

# Skate 1's post-processing 1:1 (GRADING_AUDIT_S1.md has every source). Its exe (TU0 image) packs these vault fields
# into the shaders each frame; the engine (retail_grading.rs skate1_*) redoes that packing, so the data stays raw.
# Final pass B699240D (setter 0x827409a8): adjust_hv_ratio = 2 x (B98D, E1A0); warp_vignette_power = 0.5 x (4C6F, 7154);
# hv_upscaler = 0.5 x (5905, BD4F); hv_warp_multiplier = (AF65, 59F7) x 13B5; vignette_multiplier = 74AE ('fisheye' row of
# class 1FFDC8E3ACA07C1F; 2.0 at 0x820d209c, 0.5 at 0x8211e1f4).
S1_FINAL = {'adjust': (0xB98D4668E5DF201E, 0xE1A08E911A064E88), 'power': (0x4C6FDC39C7C08995, 0x715478E1C89B25BF),
            'upscale': (0x5905218EC008C9CA, 0xBD4FD11D8686A431), 'warp': (0xAF65240EB61442FD, 0x59F7C644DEF5B7A8)}
S1_WARP_MULTIPLIER, S1_VIGNETTE_MULTIPLIER = 0x13B56EFC09EF308F, 0x74AE1570D9E0CABB
# Gameplay grade: class 716E979EFF5B29C8 row 'game' (parent 'base'), PointNegGraphData4 curves of the VisualDirector
# intensity t (0x82743600). One set's curves in 0x8273fc10's packing order: offset, contrast, pivot, desaturation,
# saturate intensity, saturate colour r/g/b, post modulate, post brightness, post quantise. E8DC lerps near (set B,
# struct +0x00) to far (set A, +0x40) between the NIS distances.
S1_SETS = {'near': (0xB2AD114543471483, 0x297A6D21445AE451, 0xF50A2257B30758CA, 0x060A0DFAF00DE957, 0x771E9E6A75AC3413,
                    0x555742F19F0199B3, 0x6BC74C41CD946D1B, 0x2D5CE9663AF5EF4B, 0x6AEA1694D86D6AB8, 0x812E840C4E569B29,
                    0xEB0E01CFF160E6DD),
           'far': (0x6F34C5C9C36250DC, 0xF7B10B6E4ABB612C, 0xAB5872D5385F99B2, 0xB44B5CE8B1CC054A, 0xFF3471276AAABFA0,
                   0xF039D55FAD7A134B, 0x3044DC7EA57EA1DB, 0x251D306AE3662431, 0x91558D416A6FF9CD, 0xE417151AB37CBA2E,
                   0x7FF3C1AB2E1EFF13)}
# Grunge radius, feather (the exe uploads 1/max(x, 1e-4)), centre x, y; NIS near, far (metres); bloom threshold (C3FA
# coefficients.w); g_BloomMultiply x, y (B3DF's level weights).
S1_CURVES = (0x243C209107E37FFF, 0x21CD1C1EC0DC365A, 0x393B9DC1D5EB9743, 0x368CA5B61D7AD150, 0x635DDCB474A62BD2,
             0xB5B4F93006F0CCF3, 0x1C226B5FDD1A430D, 0x33582B68E8733D61, 0x5CA249B4F0C60D81)
# Curves the engine leaves out because gameplay never turns them on (checked for every t below): hue flags (> 0.5
# enables), quantise radius (its mask needs |2(uv - centre)| above it; at most 1.562), noise modulate (0) and brightness
# (1) make the noise term 1, stencil lerp 0, post quantise > 0.
S1_OFF = {'hue': (0xB298FD4C2A71578A, 0xC00F882BB64E2B0A), 'quantise': (0xBC7263966D44EB65,),
          'noise_modulate': (0x1AF88762AA1E82B9, 0x79E7B661E4E17CC0, 0x0402CB68E7EF57EF),
          'noise_brightness': (0x36421D45F7ED66F7, 0x40FA90E4D133BD43, 0x075EC1A46BA03C4D), 'stencil': (0xB7806D9E1BAF15C9,)}
# VisualDirector (class 733F1FE94323D441 'default', 0x826f7618): t = g16(speed), smoothed t += (g16 - t)*(1 - k) per
# 1/60 s, k at layout +0x58 (1AB2).
S1_SPEED, S1_SMOOTHING = 0x29D1BFA11A3F8F1C, 0x1AB2D6FB82A20F88
# Auto exposure (0x8273ef80, rendering row 'tonemap', or 5133EA95 inside the volumes below; 'default' material):
# target, min, max, rate.
S1_METER = (0x418C16548A4A19D7, 0xBB6A331328CD6322, 0x9D2C1C81C9021AA1, 0xC6A7451BC264D690)
# postfx 'main': depth of field (-1, 1e-4, 60, 300): far blur from z to w; maxCoC (static, x 0.5 = bDOFLerpScale);
# the per-frame maxCoC ramp AAC7 + (E90B - AAC7)*clamp((exposure - D2B1)/(6360 - D2B1)) (update 0x82741fe0).
S1_DECK_AO = (0x196AED84AB3EF932, 0xC80DF1088E67A19B, 0xDAD1D10DA3226BA2, 0xC5415DD3823B6B29, 0xEB5F62DA0D8ECB01)
S1_DOF_KEYS = (0xF53954F62983A1C0, 0x368B9887963ED632, (0xAAC73C007F343E21, 0xE90BC76A36076ACC, 0xD2B132C5F4BD450B, 0x636016E024460B93))


def graph(fields, key):
    """PointNegGraphData<n>: a 16-byte header, then n x and n y (big-endian floats), returned as [x..., y...]."""
    raw = bytes.fromhex(fields[key]['data'])
    return list(struct.unpack('>%df' % ((len(raw) - 16)//4), raw[16:]))


def curve(c, t):
    """0x823a0df8: linear between the points, flat outside them."""
    n = len(c)//2
    x, y = c[:n], c[n:]
    if t < x[0]:
        return y[0]
    for i in range(1, n):
        if t < x[i]:
            return y[i] if x[i] <= x[i - 1] else y[i - 1] + (y[i] - y[i - 1])*(t - x[i - 1])/(x[i] - x[i - 1])
    return y[-1]


def skate1_film(name):
    import rx2_parser
    with tempfile.TemporaryDirectory(prefix='skate1-db-') as work:
        for ext in ('bin', 'vlt'):
            (Path(work)/('skater.' + ext)).write_bytes((s1game/'data/db'/('skater.' + ext)).read_bytes())
        names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
        rows = Collections(convert(Path(work)/'skater', Path(work)/'skater', names))
    fish, _ = rows.resolve(0x1FFDC8E3ACA07C1F, 'fisheye')
    v = {k: struct.unpack('>f', bytes.fromhex(fish[h]['data']))[0] for k, h in S1_FISHEYE.items()}
    main, _ = rows.resolve('postfx', 'main')
    tint = struct.unpack('>4f', bytes.fromhex(main[S1_GRUNGE]['data']))
    dof = struct.unpack('>4f', bytes.fromhex(main[S1_DOF]['data']))
    max_coc = struct.unpack('>f', bytes.fromhex(main[hash64('maxCoC')]['data']))[0]
    table = rx2_parser.parse_rx2((s1game/'data/content/textures/globaltextures.rx2').read_bytes())
    grunge, = [t for t in table.textures if (t.width, t.height) == (1024, 512)]
    (OUT/(name + '.grunge.rgba')).write_bytes(bytes(grunge.rgba))
    one = lambda f, k: struct.unpack('>f', bytes.fromhex(f[k]['data'][:8]))[0]
    final = {n: [one(fish, k) for k in keys] for n, keys in S1_FINAL.items()}
    game, _ = rows.resolve(0x716E979EFF5B29C8, 'game')
    vd, _ = rows.resolve(0x733F1FE94323D441, 'default')
    # 0x8273ede4 reads 'tonemap' unless 0x822ada80 (camera inside an audio volume that is eVolumeType 1 with
    # eNotifyType & 1; 0x822be6f8) picks 5133EA95. The volumes are skate.ems records (56 bytes: centre, extents, fade,
    # forward, key), tested at the camera by 0x822bd6b8.
    meter, _ = rows.resolve(0x1FFDC8E3ACA07C1F, 'tonemap')
    inside, _ = rows.resolve(0x1FFDC8E3ACA07C1F, 0x5133EA95720CD990)
    ems = (s1game/'data/audio/skate.ems').read_bytes()
    volumes = []
    for i in range(struct.unpack('>I', ems[:4])[0]):
        r = ems[4 + 56*i:60 + 56*i]
        key = struct.unpack('>Q', r[0x30:0x38])[0]
        if (0xF0CEF367088EFFF8, key) not in rows.rows:  # no row: the defaults at 0x831abfe0 are 0
            continue
        row, _ = rows.resolve(0xF0CEF367088EFFF8, key)
        word = lambda h: struct.unpack('>I', bytes.fromhex(row[h]['data']))[0] if h in row else 0
        if word(0x6D18B8674D7E5337) == 1 and word(0xF28CC0255D7D4898) & 1:
            f = struct.unpack('>11f', r[4:0x30])
            volumes.append([*f[:6], *f[7:10]])
    sets = {n: [graph(game, k) for k in keys] for n, keys in S1_SETS.items()}
    curves = [graph(game, k) for k in S1_CURVES]
    focus = struct.unpack('>4f', bytes.fromhex(main[S1_DOF_KEYS[0]]['data']))
    knots = sorted({0., 1., *(x for c in [*sets['near'], *sets['far'], *curves] for x in c[:4])} | {i/256 for i in range(257)})
    for t in knots:
        assert all(curve(graph(game, k), t) <= 0.5 for k in S1_OFF['hue'])
        assert all(curve(graph(game, k), t) > 1.5621 for k in S1_OFF['quantise'])
        assert all(curve(graph(game, k), t) == 0 for k in (*S1_OFF['noise_modulate'], *S1_OFF['stencil']))
        assert all(curve(graph(game, k), t) == 1 for k in S1_OFF['noise_brightness'])
        assert all(curve(s[10], t) > 0 for s in sets.values())
    s1 = {'adjust': [2*a for a in final['adjust']], 'power': [0.5*p for p in final['power']],
          'upscale': [0.5*u for u in final['upscale']], 'warp': [w*one(fish, S1_WARP_MULTIPLIER) for w in final['warp']],
          'vignette': one(fish, S1_VIGNETTE_MULTIPLIER), **sets, 'curves': curves,
          'speed': graph(vd, S1_SPEED), 'smoothing': one(vd, S1_SMOOTHING),
          'dof': [focus[2], focus[3], 0.5*one(main, S1_DOF_KEYS[1])], 'coc': [one(main, k) for k in S1_DOF_KEYS[2]],
          'meter': [one(meter, k) for k in S1_METER], 'meter_volume': [one(inside, k) for k in S1_METER], 'volumes': volumes}
    assert [round(x, 4) for x in s1['meter']] == [0.3, 0.89, 1.31, 0.2] and [round(x, 4) for x in s1['meter_volume']] == [0.3, 0.29, 0.9, 0.2]
    assert len(volumes) == 1 and [round(x, 2) for x in volumes[0][:3]] == [-39.42, 0.36, 1967.19]
    # E8DC's deck AO (0x82740190 reads row 'deck_ao' via 0x8272ee40..eff8): XLen, ZLen, Radius, HeightFalloff, Colour rgb.
    ao, _ = rows.resolve(0x1FFDC8E3ACA07C1F, 'deck_ao')
    s1['deck_ao'] = [one(ao, k) for k in S1_DECK_AO[:4]] + list(struct.unpack('>4f', bytes.fromhex(ao[S1_DECK_AO[4]]['data'])))[:3]
    # g_MotionBlurIntensity, and the enable that feeds g_bDOF.
    s1['motion_blur'] = [one(main, hash64('motion_blur_intensity')), float(main[hash64('motion_blur_enable')]['data'][:2] != '00')]
    assert abs(curve(s1['speed'], 0) - 0.3554) < 1e-4 and s1['dof'][:2] == [60, 300] and focus[0] == -1
    assert [round(x, 4) for x in s1['deck_ao']] == [0, 0.3, 0.17, 3, 0.1064, 0.1449, 0.19] and s1['motion_blur'] == [1, 1]
    return {'warp': [v['h_warp'], v['v_warp'], v['warp_power']], 'upscale': [v['h_upscaler'], v['v_upscaler']],
            'vignette': S1_VIGNETTE, 'grunge': tint[3], 'grain': S1_GRAIN,
            'texture': name + '.grunge.rgba', 'size': [grunge.width, grunge.height], 'dof': [dof[2], dof[3], max_coc], 'exposure': S1_EXPOSURE,
            's1': s1}


def smoothness(lut):
    """Mean step between neighbouring cells; a correctly unscrambled cube is smooth (neutral: ~8)."""
    a = np.frombuffer(lut, np.uint8).reshape(32, 32, 32, 4)[..., :3].astype(int)
    return float(np.mean([np.abs(np.diff(a, axis=k)).mean() for k in range(3)]))


if __name__ == '__main__':
    # Optional 5th arg s2|s1 rewrites one game's maps only (the identity check below still runs).
    only = sys.argv[5] if len(sys.argv) > 5 else ''
    if only != 's1':
        cell = skate2()
    else:
        big = Big(s2game/'data/big/colourcubes.big')
        cell = untile(big.read(next(e for e in big.entries if e.path.endswith('cc_neutral.rgb'))))
    if only != 's2':
        skate1(cell)
    # Check: the unscrambled identity cube is the identity (to 8-bit floor rounding), and every cube is smooth.
    big = Big(s2game/'data/big/colourcubes.big')
    neutral = cube(big.read(next(e for e in big.entries if e.path.endswith('cc_neutral.rgb'))), cell)
    a = np.frombuffer(neutral, np.uint8).reshape(32, 32, 32, 4)
    ramp = np.floor(np.arange(32)*255/31)
    assert np.abs(a[0, 0, :, 0] - ramp).max() <= 1 and np.abs(a[0, :, 0, 1] - ramp).max() <= 1 and np.abs(a[:, 0, 0, 2] - ramp).max() <= 1
    for f in sorted(OUT.glob('*.cubes')):
        d = f.read_bytes()
        print(f.stem, 'smoothness', [round(smoothness(d[i:i + 32**3*4]), 1) for i in range(0, len(d), 32**3*4)])
