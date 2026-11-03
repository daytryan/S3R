"""Skate 2's visual indicators (the neon outline icons: guide chevrons, the red "!" triangle, ...) for skate2_arrows.rs.

The disc draws each indicator as a ribbon along a 2D spline outline (render.big shaders/spline.fx: a "darken" pass with
visual_indicator_background, then an additive pass with the style's colour texture). Sources:
  miscboot.big content/world/models/visual_indicators_sk82.rx2  object 0xEB0021: 36 outlines keyed by asset id, each
      a run of 64-byte cubic segments [a, b, c, d] (float4 each, P(t) = a t^3 + b t^2 + c t + d, x/y in metres, y up)
  miscboot.big content/textures/vitextures.rx2                    the 32x32 ribbon cross-sections per colour
  db.big skater vault: indicator type (class 3297556BECB605E2) -> model key -> asset id (class FB5FDAB5DC2BD228);
      style (class F0DB8767D611AF94) -> texture + tint
  tutorial.json VisualIndicators rows: words[10..11] type key, words[16..17] style key

Writes assets/private/skate2-tutorial/indicator/{indicators.json,signup.json,<colour>.png,background.png}.
usage (numpy, PIL): py old_indicators.py REPO SKATE2_ROOT ASSETS   (after old_screens.py, which caches the vault)
"""
import json, os, struct, sys
from pathlib import Path

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/vendor/utt'), str(here)]
import numpy as np
from PIL import Image
from s2big import Big
from vendor.skate3_ui.rw4 import read_textures
from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant, xbox360_tiled_offset

OUT = assets/'private/skate2-tutorial/indicator'
VAULT = WORK_ROOT/'screens-cache/skater-collections.json'
TYPE, MODEL, STYLE = 'Hash_3297556BECB605E2', 'Hash_FB5FDAB5DC2BD228', 'Hash_F0DB8767D611AF94'
STEPS = 10  # samples per segment (the outlines are near-straight runs)
SIGNUP_STYLE = 'default'  # the sign-up indicators' colour style (screens.md "Trace results" f)


def rgba(texture):
    """RGBA for A8R8G8B8 (the vendored decoder) and 5:6:5 (format 4, big-endian words, the extra colours)."""
    t = texture.metadata
    if t['format_code'] != 4:
        return np.frombuffer(decode_rgba(texture.data, parse_fetch_constant(bytes.fromhex(t['fetch_constant_hex']))),
                             np.uint8).reshape(t['height'], t['width'], 4)
    out = np.zeros((t['height'], t['width'], 4), np.uint8)
    for y in range(t['height']):
        for x in range(t['width']):
            at = xbox360_tiled_offset(x, y, max(32, t['width']), 1) * 2
            v, = struct.unpack_from('>H', texture.data, at)
            out[y, x] = ((v >> 11) * 255 // 31, ((v >> 5) & 63) * 255 // 63, (v & 31) * 255 // 31, 255)
    return out


def outlines(d):
    """{asset id hex: [[x, y], ...]} from the model pack (object 0xEB0021)."""
    count, table = struct.unpack_from('>I', d, 0x20)[0], struct.unpack_from('>I', d, 0x30)[0]
    pack = next(d[at:at + size] for at, _, size, _, _, kind in
                (struct.unpack_from('>6I', d, table + 24 * i) for i in range(count)) if kind == 0xEB0021)
    shapes = {}
    for i in range(struct.unpack_from('>I', pack, 0)[0]):
        e = 0x10 + 64 * i
        asset = pack[e + 8:e + 16].hex().upper()
        _, n, offset, _ = struct.unpack_from('>4I', pack, e + 48)
        points = []
        for j in range(n):
            r = struct.unpack_from('>16f', pack, offset + 64 * j)
            a, b, c, p = (np.array(r[k:k + 2]) for k in (0, 4, 8, 12))
            points += [(((a * t + b) * t + c) * t + p).round(4).tolist() for t in np.arange(STEPS) / STEPS]
            if j == n - 1:
                points.append((a + b + c + p).round(4).tolist())
        shapes[asset] = points
    return shapes


def main():
    misc = Big(s2game/'data/big/miscboot.big')
    files = {Path(e.path).name: e for e in misc.entries}
    OUT.mkdir(parents=True, exist_ok=True)
    work = WORK_ROOT/'vi'
    work.mkdir(parents=True, exist_ok=True)
    (work/'vitextures.rx2').write_bytes(misc.read(files['vitextures.rx2']))
    textures = {}
    for t in read_textures(work/'vitextures.rx2')[1]:
        name = t.name.removesuffix('.Texture')
        if name.startswith('visual_indicator_'):
            file = name.removeprefix('visual_indicator_') + '.png'
            pixels = rgba(t).copy()
            if file != 'background.png':
                pixels[..., 3] = 255  # the colour pass adds ONE/ONE: alpha unused, so the engine's Add mode must not dim
            Image.fromarray(pixels).save(OUT/file)
            textures[t.name] = file
    shapes = outlines(misc.read(files['visual_indicators_sk82.rx2']))

    rows = {}
    for c in json.loads(VAULT.read_text(encoding='utf-8'))['collections']:
        rows[(c['class'], c['key'])] = c

    def field(cls, key, name):
        """A vault field with parent inheritance."""
        while (c := rows.get((cls, key))) is not None:
            if name in c['fields']:
                return c['fields'][name]['data']
            key = c['parent']
        return None

    from tools.asset_pipeline.vlt import hash64
    names = {'%016X' % hash64(k): k for k in ('default', 'orange', 'red', 'yellow', 'blue', 'green', 'tutorial', 'signup')}

    def style(key):
        key = names.get(key, 'Hash_' + key)
        tint = field(STYLE, key, 'Hash_E4F3418597AACAA5')
        return {'texture': textures[field(STYLE, key, 'Hash_8E38A3CD0186F8DA')],
                'background': textures.get(field(STYLE, key, 'Hash_FF4DCA217E05B9BC')),
                'tint': list(struct.unpack('>4f', bytes.fromhex(tint))) if tint else [1, 1, 1, 1]}

    def shape(key):
        model = field(TYPE, names.get(key, 'Hash_' + key), 'Hash_8D090C8B5931547A')[16:32]
        return field(MODEL, 'Hash_' + model, 'Hash_1AFE1B981FB92727')

    tutorial = json.loads((OUT.parent/'tutorial.json').read_text(encoding='utf-8'))
    locators, used = {}, set()

    def walk(o):
        if isinstance(o, dict):
            for r in o.get('VisualIndicators') or []:
                w = r['words']
                kind, look = '%08X%08X' % (w[10], w[11]), '%08X%08X' % (w[16], w[17])
                locators[r['locator']] = {'shape': shape(kind), 'style': look}
                used.add(look)
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)
    walk(tutorial['challenges'])
    wanted = {l['shape'] for l in locators.values()}
    out = {'shapes': {k: v for k, v in shapes.items() if k in wanted},
           'styles': {k: style(k) for k in used},
           'locators': locators}
    (OUT/'indicators.json').write_text(json.dumps(out, separators=(',', ':')), encoding='utf-8')
    # Sign-up indicators (each challenge's SignUpIndicator type row) over the lesson's Location; the binding raises them
    # 2 m (the `signup` row's offset) and shows them while the lesson is open.
    look = '%016X' % hash64(SIGNUP_STYLE)
    signup = {}
    for c in tutorial['challenges'].values():
        ref, location = (c['self'].get('SignUpIndicator') or [None])[0], c['self'].get('Location')
        if ref and location:
            signup['signup:' + location] = {'shape': shape(ref[16:]), 'style': look}
    assert all(l['shape'] in shapes for l in signup.values()), signup
    (OUT/'signup.json').write_text(json.dumps({
        'shapes': {l['shape']: shapes[l['shape']] for l in signup.values()},
        'styles': {look: style(look)},
        'locators': signup}, separators=(',', ':')), encoding='utf-8')
    print(len(signup), 'sign-up indicators;', sorted({l['shape'] for l in signup.values()}))
    print(len(locators), 'indicators;', {k: sum(1 for l in locators.values() if l['shape'] == k) for k in wanted},
          {k: v['texture'] for k, v in out['styles'].items()})


if __name__ == '__main__':
    main()
