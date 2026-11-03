"""The skin-through-clothes fit of the ported Edit Skater items (Skate 1, Skate 2 + its DLC, Skate 3 DLC, the Free Play
pros' items), applied from fit_corrections.npz by the wardrobe stages that make the models (old_skate_clothing.py and
pro_clothing.py call apply() before they draw their tiles, so each tile is drawn once, from the fitted model).

The fit itself (10-04, .superpowers/sdd/2026-10-04-clothing/fit-report.md and round 2) tests every item on the body
parts the engine shows with it, in bind, extreme and retail poses with the Build morphs, and pushes garment points out
of the skin, transfers missing fat/thin targets and, in round 2, re-derives skin weights. It takes hours of ray tests,
so it ran once; fit_corrections.npz holds what it changed per model, as changes to the model the wardrobe stages write:
  <mid>/check     SHA-256 of the unfitted POSITION bytes the changes belong to (another model: left as it is)
  <mid>/moved     indices of the points the fit moved, <mid>/delta their change (float64: the float32 result is exact)
  <mid>/joints, <mid>/weights   the round-2 skin weights (when the fit re-derived them)
  <mid>/target<t>/{delta,ndelta}  fat (0) / thin (1) target changes (position, normal), float64
  <mid>/meta      the GLB's extras.skin_fit
Each fitted model is rewritten in place as the fit wrote it (write_fit: new accessors, the source's others kept). A
generator rewrites its models from the discs on every run, so there is nothing to undo.

usage: fit_clothing.py --extract FITTED_SET UNFITTED_SET OUT  the corrections from a fitted set (its pre-fit/ and
                                                              pre-round2/ hold the originals) and a fresh one
"""
import hashlib, json, struct, sys
from pathlib import Path
import numpy as np

DATA = Path(__file__).resolve().with_name('fit_corrections.npz')
# in the tile markers of fitted items: new data or a new rule, new tiles
VERSION = hashlib.sha256(DATA.read_bytes() + b'waist 0.95').hexdigest()[:12]
TARGETS = (0, 1)  # library morph slots: fat, thin
# Pants keep their own waist (10-11, item 3): the fit pushed waistbands out by ~1-2 cm at y 0.9-1.1, through the tops
# over them (S2 pants poked out of 40/117 S1 and 14/153 S2 tops, none before). Points the fit moved from above WAIST go
# back where the disc put them, blended over WAIST_BLEND below it; under the tops' hems (median 0.86-0.90) the fit stays.
# Swept on set a5a3 (scratchpad me/pants/sweep.py): male 1218 poking pairs vs 1195 unfitted and 1384 fitted; female 3/3/5.
WAIST, WAIST_BLEND = 0.95, 0.03


def document(path):
    from glb import Document
    return Document(path)


def geometry(path):
    """(POSITION, JOINTS_0, WEIGHTS_0 and the fat/thin targets' POSITION/NORMAL of a one-primitive GLB, its skin_fit)."""
    d = document(path)
    prims = d.doc['meshes'][0]['primitives']
    assert len(prims) == 1, path
    a, targets = prims[0]['attributes'], prims[0].get('targets', [])
    out = {k: d.accessor(a[k]) for k in ('POSITION', 'JOINTS_0', 'WEIGHTS_0')}
    for t in TARGETS:
        for k in ('POSITION', 'NORMAL'):
            out[f'{t}{k}'] = d.accessor(targets[t][k]) if t < len(targets) and k in targets[t] else np.zeros_like(out['POSITION'])
    return out, d.doc.get('extras', {}).get('skin_fit')


def check(positions):
    return np.frombuffer(hashlib.sha256(np.ascontiguousarray(positions, '<f4').tobytes()).digest(), np.uint8)


# ---- writing a fitted GLB (the 10-04 fit's write_fit) -------------------------------------------------------
def append_accessor(doc, data, array, kind, component, target, bounds=False):
    data.extend(b'\0' * (-len(data) % 4))
    doc.doc['bufferViews'].append({'buffer': 0, 'byteOffset': len(data), 'byteLength': array.nbytes, 'target': target})
    data.extend(array.tobytes())
    accessor = {'bufferView': len(doc.doc['bufferViews']) - 1, 'componentType': component, 'count': len(array), 'type': kind}
    if bounds:
        accessor.update(min=array.min(0).tolist(), max=array.max(0).tolist())
    doc.doc['accessors'].append(accessor)
    return len(doc.doc['accessors']) - 1


def save_glb(doc, data, path):
    data.extend(b'\0' * (-len(data) % 4))
    doc.doc['buffers'] = [{'byteLength': len(data)}]
    text = json.dumps(doc.doc, separators=(',', ':')).encode()
    text += b' ' * (-len(text) % 4)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(struct.pack('<III', 0x46546c67, 2, 28 + len(text) + len(data)) + struct.pack('<I4s', len(text), b'JSON')
                     + text + struct.pack('<I4s', len(data), b'BIN\0') + bytes(data))


def write_fit(source, target, fit):
    """The source GLB with fitted positions, skin weights and targets (new accessors: zero targets are shared)."""
    doc = document(source)
    data = bytearray(doc.data)
    prim = doc.doc['meshes'][0]['primitives'][0]
    prim['attributes']['POSITION'] = append_accessor(doc, data, fit['pos'].astype('<f4'), 'VEC3', 5126, 34962, True)
    if fit['weights'] is not None:
        component = doc.doc['accessors'][prim['attributes']['JOINTS_0']]['componentType']
        prim['attributes']['JOINTS_0'] = append_accessor(doc, data, fit['weights'][0].astype('<u2' if component == 5123 else '<u1'), 'VEC4', component, 34962)
        prim['attributes']['WEIGHTS_0'] = append_accessor(doc, data, fit['weights'][1].astype('<f4'), 'VEC4', 5126, 34962)
    for t, (dp, dn) in sorted(fit['added'].items()):
        prim['targets'][t] = dict(prim['targets'][t], POSITION=append_accessor(doc, data, dp.astype('<f4'), 'VEC3', 5126, 34962, True),
                                  NORMAL=append_accessor(doc, data, dn.astype('<f4'), 'VEC3', 5126, 34962))
    doc.doc.setdefault('extras', {})['skin_fit'] = fit['meta']
    save_glb(doc, data, Path(target))


def corrected(g, data, mid, slot=None):
    """The fit for an unfitted model's geometry() from the corrections, or None when they belong to another model."""
    if not np.array_equal(check(g['POSITION']), data[f'{mid}/check']):
        return None
    pos = g['POSITION'].astype(np.float64)
    moved, delta = data[f'{mid}/moved'], data[f'{mid}/delta']
    if slot == 'Pants':
        delta = delta * np.clip((WAIST - pos[moved, 1]) / WAIST_BLEND, 0, 1)[:, None]
    pos[moved] += delta
    added = {t: (g[f'{t}POSITION'] + data[f'{mid}/target{t}/delta'], g[f'{t}NORMAL'] + data[f'{mid}/target{t}/ndelta'])
             for t in TARGETS if f'{mid}/target{t}/delta' in data}
    weights = (data[f'{mid}/joints'], data[f'{mid}/weights']) if f'{mid}/joints' in data else None
    return dict(pos=pos, added=added, weights=weights, meta=json.loads(str(data[f'{mid}/meta'])))


def models(data):
    return sorted({k.split('/')[0] for k in data.files})


def extract(fitted_set, fresh_set, out):
    """fit_corrections.npz from a set the fit was applied to and a fresh set of the same wardrobe: every changed
    array as a change to the fresh model, checked to rebuild the fitted one exactly."""
    arrays = {}
    for path in sorted((fitted_set/'library').glob('*_v4.glb')):
        f, meta = geometry(path)
        if not meta:
            continue
        mid = path.name[:-len('_v4.glb')]
        o, _ = geometry(next(p for p in (fitted_set/'pre-fit'/path.name, fitted_set/'pre-round2'/path.name) if p.exists()))
        assert np.array_equal(o['POSITION'], geometry(fresh_set/'library'/path.name)[0]['POSITION']), mid
        delta = f['POSITION'].astype(np.float64) - o['POSITION'].astype(np.float64)
        moved = np.nonzero(delta.any(1))[0]
        arrays.update({f'{mid}/check': check(o['POSITION']), f'{mid}/moved': moved.astype(np.uint32),
                       f'{mid}/delta': delta[moved], f'{mid}/meta': np.array(json.dumps(meta))})
        if not (np.array_equal(f['JOINTS_0'], o['JOINTS_0']) and np.array_equal(f['WEIGHTS_0'], o['WEIGHTS_0'])):
            arrays[f'{mid}/joints'], arrays[f'{mid}/weights'] = f['JOINTS_0'], f['WEIGHTS_0']
        for t in TARGETS:
            if not (np.array_equal(f[f'{t}POSITION'], o[f'{t}POSITION']) and np.array_equal(f[f'{t}NORMAL'], o[f'{t}NORMAL'])):
                arrays[f'{mid}/target{t}/delta'] = f[f'{t}POSITION'].astype(np.float64) - o[f'{t}POSITION']
                arrays[f'{mid}/target{t}/ndelta'] = f[f'{t}NORMAL'].astype(np.float64) - o[f'{t}NORMAL']
    np.savez_compressed(out, **arrays)
    data = np.load(out)
    for mid in models(data):  # the round trip rebuilds every fitted array exactly
        fit = corrected(geometry(fresh_set/'library'/f'{mid}_v4.glb')[0], data, mid)
        f, _ = geometry(fitted_set/'library'/f'{mid}_v4.glb')
        assert np.array_equal(fit['pos'].astype('<f4'), f['POSITION']), mid
        for t, (dp, dn) in fit['added'].items():
            assert np.array_equal(dp.astype('<f4'), f[f'{t}POSITION']) and np.array_equal(dn.astype('<f4'), f[f'{t}NORMAL']), mid
        if fit['weights'] is not None:
            assert np.array_equal(fit['weights'][0], f['JOINTS_0']) and np.array_equal(fit['weights'][1], f['WEIGHTS_0']), mid
    print('fit corrections:', len(models(data)), 'models ->', out)


def fitted_already(path):
    """The GLB's JSON chunk names a skin_fit (read without the binary chunk: pro_clothing.py checks all 365 again)."""
    with open(path, 'rb') as glb:
        length = struct.unpack('<12xI', glb.read(16))[0]
        return 'skin_fit' in json.loads(glb.read(4 + length)[4:]).get('extras', {})


def apply(assets, library):
    """Fit every model of the set the corrections hold (present, not fitted yet, and the model they were made for), in
    place. Returns the ids of the models now fitted, for the tile markers (`VERSION`).
    Never the Accessory slot: its round-2 skin weights moved every belt and Isaac's back plates wholly onto
    RIGHTHAND_REPARENTED and a third of Mike's backpack onto bones it does not hang from (10-11), and it moved none
    of their points. Their authored weights are kept."""
    fitted = set()
    with np.load(DATA) as data:
        for mid in models(data):
            model = library['models'].get(mid)
            target = assets/model['scene'] if model and model.get('slot') != 'Accessory' else None
            if target is None or not target.exists():
                continue  # not in this wardrobe (yet: pro_clothing.py adds the pros' items after old_skate_clothing.py)
            if not fitted_already(target):
                fit = corrected(geometry(target)[0], data, mid, model.get('slot'))
                if fit is None:
                    continue  # not the model the fit was made for
                write_fit(target, target, fit)
            fitted.add(mid)
    return fitted


if __name__ == '__main__':
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]/'mixamo_to_skate'))  # glb
    assert sys.argv[1:2] == ['--extract'], __doc__
    extract(Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]))
