"""Edit Skater tiles for ported parts (Skate 1, Skate 2, the Free Play roster's clothes), drawn on the
retail Skate 3 template: per tile folder (male/hat, female/feet, ...) one fixed perspective camera and
pose on the thumbnail mannequin, measured from the native Skate 3 tiles (silhouette + colour fits; the
evidence is in .superpowers/sdd/2026-10-04-clothing/previews-report.md).

A scene is rasterised once per model into a G-buffer (mesh id, UVs, normal); every design of the model
is then only a texture lookup, so hundreds of tee designs cost one raster."""
import ctypes, functools
from pathlib import Path
import numpy as np
from PIL import Image

SIZE, SS, BORDER = 128, 3, 4  # tile size, supersampling, transparent border every native tile keeps (rows/cols 4..123)
BIAS = {'Sock': 0.004}  # slot -> depth pull (m): socks lie inside the socked legs' surface
SKIN = [1.21, 2.097, 2.597]  # skin balance (character.skin / character.face), calibrated on the natives
TEMPLATE_VERSION = 6  # part of every drawn tile's marker: bump to redraw all ported tiles (4: round 2 board, sock, glasses, hat templates; 5: the sock rule mirrored from customiser_parts.rs, decks drawn with their grip primitive; 6: the face turns with the head)

# The mannequin every native tile wears (identified on the native tiles: clean-shaven light head with a
# brown buzz cut / shoulder-length dark brown hair, the plain grey stampable tee, charcoal chinos / blue jeans,
# adidas campus vulc shoes). (model id, material id) from the retail catalog.
MANNEQUIN = {
    'male': {'Rostral': ('9461df2eb3d482dc', '2c7f3817001715ad'), 'Hair': ('8a283505bb37b70a', '2c7f3817000204f6'),
             'Organ': ('b278a4bfb3d6c4bc', '00000c6203e38817'), 'OuterTorso': ('801095192c838106', '0000129103e38817'),
             'Pants': ('d92b14a037b78985', '2c7f38170017358c'), 'Feet': ('85b73da337acc9cd', '2c7f381700171a9e')},
    'female': {'Rostral': ('f2fdb9aa8ec6271c', '2c7f38170017020f'), 'Hair': ('a54de70b338bac30', '0000231103e38817'),
               'Organ': ('dbe5ecdf58eb9a5a', '0000239a03e38817'), 'OuterTorso': ('862d97f973760883', '000022c703e38817'),
               'Pants': ('bdf44cfe4be1561c', '0000438d03e38817'), 'Feet': ('85b73da337acc9cd', '2c7f381700171a9e')},
}
# Board-part tiles (data/fe/cas/skateboard, skatetruck, skatewheel): retail draws the item on the assembled board, one
# fixed camera per folder (every native deck tile has the same silhouette, cover 0.53; trucks 0.76; wheels 0.72). The
# rest of the board is the default deck in White 1 (the plain grey underside every native truck/wheel tile shows),
# the default Generic Truck and Trim 113 wheels.
BOARD = {'SkateBoard': ('a841a65d5c4dd593', '000044df03e38817'), 'SkateTruck': ('c6f01c93097b7fb5', '000048e603e38817'),
         'SkateWheel': ('87d0fa0ea0366b23', '00004acc03e38817')}
BOARD_FOLDERS = {'skateboard', 'skatetruck', 'skatewheel'}
# the mannequins' hair colour (hair takes the profile's colour): measured on the native glasses / hat tiles (hairbal)
HAIR_TINT = {'male': [0.74, 0.60, 0.51], 'female': [0.22, 0.14, 0.10]}
# tile folder -> what the native tiles show besides the item (None: the whole mannequin)
SHOWN = {'wristitem': {'Arm'}, 'watch': {'Arm'}, 'wristband': {'Arm'}, 'accessory': set(), 'jewellery': set()}
# tile folder -> camera (orbit about `target`: yaw, pitch, roll, dist (m), fov (deg)) + pose; fitted on the native
# tiles (silhouette IoU + colour NCC), see previews-report.md. 'framed': the rule for items the fixed camera misses.
TEMPLATES = {'male/hat': {'target': [-0.018, 1.6839, 0.0595],
              'yaw': -4.5155,
              'pitch': 16.5162,
              'dist': 1.446,
              'fov': 11.7256,
              'roll': 0.0,
              'pose': {'head_bow': 10.2077, 'head_turn': -12.0329, 'head_tilt': -0.3525, 'arm_drop': 50.5}},
 'male/glasses': {'target': [-0.0471, 1.6431, 0.0821],
                  'yaw': 7.7772,
                  'pitch': 10.857,
                  'dist': 0.6364,
                  'fov': 19.641,
                  'roll': 0.0,
                  'pose': {'head_bow': 3.1359, 'head_turn': -23.7477, 'head_tilt': 6.7609, 'arm_drop': 31.4687}},
 'male/outertorso': {'target': [0.0335, 1.1846, 0.0122],
                     'yaw': -0.1887,
                     'pitch': 9.1504,
                     'dist': 2.9296,
                     'fov': 15.4518,
                     'roll': 0.0,
                     'pose': {'arm_drop': 33.1208, 'arm_fwd': 1.2533}},
 'male/pants': {'target': [-0.0293, 0.62, 0.0327],
                'yaw': 25.5339,
                'pitch': 0.3548,
                'dist': 2.458,
                'fov': 14.0376,
                'roll': 0.0,
                'pose': {'arm_drop': 41.3041, 'arm_fwd': -11.1992, 'leg_spread': -0.2485, 'toe_out': 10.6973}},
 'male/feet': {'target': [0.1663, 0.0739, 0.0774],
               'yaw': -62.3095,
               'pitch': 15.6551,
               'dist': 1.461,
               'fov': 13.4696,
               'roll': 0.0,
               'pose': {}},
 'male/sock': {'target': [0.0526, 0.2376, 0.0311],
               'yaw': -51.4736,
               'pitch': 17.7571,
               'dist': 2.2074,
               'fov': 15.7701,
               'roll': 0.0,
               'pose': {'leg_spread': 2.5, 'toe_out': 8.8224}},
 'male/wristitem': {'target': [0.4934, 1.0935, 0.0834],
                    'yaw': -57.0,
                    'pitch': 56.4,
                    'dist': 0.927,
                    'fov': 6.3,
                    'roll': -86.0,
                    'pose': {}},
 'male/accessory': {'target': [0.6054, 0.9885, 0.1278],
                    'yaw': 175.2422,
                    'pitch': -8.1425,
                    'dist': 1.0112,
                    'fov': 3.0598,
                    'roll': -0.5917,
                    'pose': {},
                    'item_centre': [0.6045, 0.9909, 0.1351],
                    'framed': {'yaw': 0.0,
                               'pitch': 17.5,
                               'roll': -45.0,
                               'fov': 3.0598,
                               'ox': 0.0,
                               'oy': 0.0,
                               'span': 0.6},
                    'framed_back': {'yaw': 200.0,
                                    'pitch': 8.0,
                                    'roll': 0.0,
                                    'fov': 3.0598,
                                    'ox': 0.0,
                                    'oy': 0.0,
                                    'span': 0.6}},
 'female/hat': {'target': [-0.0463, 1.6713, 0.0284],
                'yaw': -7.7037,
                'pitch': 15.8069,
                'dist': 1.3849,
                'fov': 12.7188,
                'roll': 0.0,
                'pose': {'head_bow': 15.8806, 'head_turn': -15.7176, 'head_tilt': 4.8987, 'arm_drop': 53.631}},
 'female/outertorso': {'target': [0.0311, 1.1691, -0.0241],
                       'yaw': 8.7049,
                       'pitch': 6.4705,
                       'dist': 3.0241,
                       'fov': 15.2131,
                       'roll': 0.0,
                       'pose': {'arm_drop': 31.5691, 'arm_fwd': -14.2691}},
 'female/pants': {'target': [-0.0293, 0.62, 0.0327],
                  'yaw': 25.5339,
                  'pitch': 0.3548,
                  'dist': 2.458,
                  'fov': 14.0376,
                  'roll': 0.0,
                  'pose': {'arm_drop': 41.3041, 'arm_fwd': -11.1992, 'leg_spread': -0.2485, 'toe_out': 10.6973}},
 'female/feet': {'target': [0.1565, 0.0768, 0.0698],
                 'yaw': -58.4038,
                 'pitch': 13.9765,
                 'dist': 1.4927,
                 'fov': 14.0743,
                 'roll': 0.0,
                 'pose': {}},
 'female/sock': {'target': [0.0442, 0.2414, 0.0277],
                 'yaw': -49.6189,
                 'pitch': 21.7029,
                 'dist': 2.0824,
                 'fov': 16.3937,
                 'roll': 0.0,
                 'pose': {'leg_spread': 1.5, 'toe_out': 6.8396}},
 'female/wristitem': {'target': [0.5, 1.0837, 0.0887],
                      'yaw': -56.0,
                      'pitch': 55.7,
                      'dist': 0.995,
                      'fov': 6.3,
                      'roll': -86.0,
                      'pose': {}},
 'skateboard': {'target': [0.0081, 0.0456, 0.0442],
                'yaw': -16.12,
                'pitch': -95.7473,
                'roll': 23.1301,
                'dist': 0.7908,
                'fov': 30.8624,
                'pose': {}},
 'skatetruck': {'target': [-0.0164, 0.0748, 0.2163],
                'yaw': -20.0192,
                'pitch': -30.437,
                'roll': -12.3106,
                'dist': 0.3854,
                'fov': 27.3141,
                'pose': {}},
 'skatewheel': {'target': [-0.0926, 0.0363, 0.2491],
                'yaw': 114.2075,
                'pitch': -21.6331,
                'dist': 0.2447,
                'fov': 20.9006,
                'roll': -76.4705,
                'pose': {}}}
ALIAS = {'watch': 'wristitem', 'wristband': 'wristitem'}
# Item-only folders frame each item (the natives' camera direction, their item-to-tile size ratio).
FRAMED = {'male/jewellery': {'yaw': 0.0, 'pitch': 16.13, 'span': 1.625, 'ox': 0.0, 'oy': -0.402, 'fov': 8.0},
 'female/jewellery': {'yaw': 0.0, 'pitch': 16.13, 'span': 1.625, 'ox': 0.0, 'oy': -0.402, 'fov': 8.0}}


def rx(a): c, s = np.cos(a), np.sin(a); return np.array([[1, 0, 0], [0, c, -s], [0, s, c]])
def ry(a): c, s = np.cos(a), np.sin(a); return np.array([[c, 0, s], [0, 1, 0], [-s, 0, c]])
def rz(a): c, s = np.cos(a), np.sin(a); return np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]])

def indices(doc):
    """Every primitive's triangles: a deck's grip is a second primitive on the same vertices (character_glb.py)."""
    prims = doc.doc['meshes'][0]['primitives']
    assert all(p['attributes'] == prims[0]['attributes'] for p in prims)
    return np.concatenate([doc.accessor(p['indices']) for p in prims]).reshape(-1, 3)


@functools.lru_cache(maxsize=256)
def load_glb(path):
    from glb import Document
    doc = Document(path)
    j = doc.doc
    prim = j['meshes'][0]['primitives'][0]
    a = prim['attributes']
    m = dict(pos=doc.accessor(a['POSITION']).astype(np.float64), nrm=doc.accessor(a['NORMAL']).astype(np.float64),
             uv=doc.accessor(a['TEXCOORD_0']).astype(np.float64), idx=indices(doc).astype(np.int64))
    m['uv1'] = doc.accessor(a['TEXCOORD_1']).astype(np.float64) if 'TEXCOORD_1' in a else m['uv']
    if j.get('skins') and 'JOINTS_0' in a:
        skin = j['skins'][0]
        names = [j['nodes'][i].get('name', '').upper() for i in skin['joints']]  # converted rigs: LEFTARM, LEFTHAND_REPARENTED, ...
        ibm = doc.accessor(skin['inverseBindMatrices']).reshape(-1, 4, 4)
        m['joint_pos'] = {n: np.linalg.inv(ibm[i].T)[:3, 3] for i, n in enumerate(names)}
        m['names'], m['joints'] = names, doc.accessor(a['JOINTS_0']).astype(np.int64)
        m['weights'] = doc.accessor(a['WEIGHTS_0']).astype(np.float64)
    return m


ARM = ('ARM', 'ARMTWIST', 'FOREARM', 'FOREARMTWIST', 'FOREARMTWIST1', 'HAND')
# The head's own bones: HEAD and the face bones under it (asset_pipeline/customisation_library.py FACE_JOINTS), which the
# mannequin's head (Rostral: jaw, lips, nose, brows) and eyes (Organ) are skinned to. Bug 10-17 #46: left out, the face
# stayed put while the skull turned (hat and glasses tiles).
HEAD = ('HEAD', 'FACE', 'OFFSET_JAW', 'JAW', 'OFFSET_CHIN', 'CHIN', 'TONGUE', 'TONGUETIP', 'LEFTEYE', 'RIGHTEYE', 'UPPERLIP',
        'LOWERLIP', *(s + p for s in ('LEFT', 'RIGHT') for p in ('UPEYELID', 'LOWEYELID', 'INNEREYEBROW', 'OUTEREYEBROW',
        'CHEEK', 'UPCHEEK', 'NOSE', 'CREASE', 'MOUTH', 'UPPERLIP', 'LOWERLIP')))


def chain(m, members):
    """Per-vertex weight on joints; members: name -> factor (a name ending in '*' is a prefix)."""
    f = np.array([next((v for k, v in members.items() if n == k or (k.endswith('*') and n.startswith(k[:-1]))), 0.0)
                  for n in m['names']])
    return (m['weights'] * f[m['joints']]).sum(1)[:, None]


def posed(m, pose):
    """Bind pose (A-pose) -> tile pose by linear blend skinning of three chains: each arm about its
    shoulder ball (arm_drop down, arm_fwd forward), the head about the neck (head_bow, head_turn, head_tilt), each leg about its hip (leg_spread, toe_out)."""
    pos, nrm = m['pos'], m['nrm']
    if 'names' not in m:
        return pos, nrm
    moves = []
    for side, sign in (('LEFT', -1), ('RIGHT', 1)):  # the left arm is at +X
        R = rx(np.radians(-pose.get('arm_fwd', 0))) @ rz(np.radians(sign * pose.get('arm_drop', 0)))
        members = {side + p + ('*' if p == 'HAND' else ''): 1.0 for p in ARM}
        if side + 'ARM' in m['joint_pos']:
            moves.append((members, m['joint_pos'][side + 'ARM'], R))
    for side, sign in (('LEFT', 1), ('RIGHT', -1)):  # legs about the hip: spread apart, toes out
        R = rz(np.radians(sign * pose.get('leg_spread', 0))) @ ry(np.radians(sign * pose.get('toe_out', 0)))
        if side + 'UPLEG' in m['joint_pos']:
            moves.append(({side + p: 1.0 for p in ('UPLEG', 'LEG', 'FOOT', 'TOEBASE*')}, m['joint_pos'][side + 'UPLEG'], R))
    H = ry(np.radians(pose.get('head_turn', 0))) @ rx(np.radians(pose.get('head_bow', 0))) @ rz(np.radians(pose.get('head_tilt', 0)))
    if 'NECK' in m['joint_pos']:
        moves.append(({'NECK': 0.5, 'NECK1': 1.0, **dict.fromkeys(HEAD, 1.0)}, m['joint_pos']['NECK'], H))
    out_p, out_n = pos.copy(), nrm.copy()
    for members, pivot, R in moves:
        if np.allclose(R, np.eye(3)):
            continue
        w = chain(m, members)
        if w.any():
            out_p += w * (((pos - pivot) @ R.T + pivot) - pos)
            out_n += w * (nrm @ R.T - nrm)
    return out_p, out_n


def camera(yaw, pitch, roll=0.0):
    """World -> view (view looks down -Z; the skater faces +Z, its left is +X)."""
    y, p = np.radians(yaw), np.radians(pitch)
    ry_ = np.array([[np.cos(y), 0, np.sin(y)], [0, 1, 0], [-np.sin(y), 0, np.cos(y)]])
    rx_ = np.array([[1, 0, 0], [0, np.cos(p), -np.sin(p)], [0, np.sin(p), np.cos(p)]])
    return rz(np.radians(roll)) @ rx_ @ ry_


def eye_from(target, yaw, pitch, dist, roll=0.0):
    return np.asarray(target, float) + camera(yaw, pitch, roll).T @ np.array([0, 0, dist])


@functools.cache
def _native():
    """tiles_native.rs (tiles.dll): the triangle loops below in Rust, bit for bit; None runs them in Python."""
    import sys
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools import native
    lib = native.load('tiles', __file__)
    if lib is not None:
        P, I, F = ctypes.c_void_p, ctypes.c_int64, ctypes.c_double
        lib.tiles_perspective.argtypes = [I, P, P, P, P, P, P, I, P, I, ctypes.c_int32, F, P, I, I, I, ctypes.c_int32,
                                          ctypes.c_int32, P, P, P, P, P]
        lib.tiles_perspective.restype = None
        lib.tiles_orthographic.argtypes = [I, P, P, P, P, P, I, P, I, ctypes.c_int16, P, P, P, P]
        lib.tiles_orthographic.restype = None
        lib.tiles_sample.argtypes = [P, I, I, P, I, P]
        lib.tiles_sample.restype = None
        lib.tiles_twin.argtypes = [P, P, I, I]
        lib.tiles_twin.restype = ctypes.c_int32
    return lib


def _c(array, dtype=np.float64):
    return np.ascontiguousarray(array, dtype)


def rasterise(meshes, cam, pose, coverage=None, bias=None, cull=True):
    """G-buffer (mesh id, uv, uv1, view normal) of posed meshes through a perspective camera:
    cam = dict(eye, yaw, pitch, fov[, roll]). coverage[mesh id] = f(uv, uv1) -> bool (alpha test) or None:
    a cut-out pixel is discarded before the depth test, so what is behind it shows (hair over the forehead).
    bias[mesh id]: metres the mesh is pulled toward the camera in the depth test (a sock inside the leg)."""
    n = SIZE * SS
    R = camera(cam['yaw'], cam['pitch'], cam.get('roll', 0))
    f = 1 / np.tan(np.radians(cam['fov']) / 2)
    depth = np.full((n, n), np.inf)
    mid = np.full((n, n), -1, np.int32)
    uv, uv1, nrm = np.zeros((n, n, 2)), np.zeros((n, n, 2)), np.zeros((n, n, 3))
    for m_i, m in enumerate(meshes):
        cover = coverage[m_i] if coverage else None
        pos, normals = posed(m, pose)
        v = (pos - np.asarray(cam['eye'], float)) @ R.T
        z = -v[:, 2]
        sx = (0.5 + 0.5 * f * v[:, 0] / np.maximum(z, 1e-6)) * n
        sy = (0.5 - 0.5 * f * v[:, 1] / np.maximum(z, 1e-6)) * n
        vn = normals @ R.T
        tris = m['idx']
        tx, ty = sx[tris], sy[tris]
        keep = (z[tris] > 0.02).all(1) & (tx.max(1) >= 0) & (tx.min(1) < n) & (ty.max(1) >= 0) & (ty.min(1) < n)
        if (lib := _native()) is not None and (cover is None or isinstance(cover, Cover)):
            kept, arrays = _c(tris[keep], np.int64), [_c(a) for a in (sx, sy, z, vn, m['uv'], m['uv1'])]
            tex = _c(cover.texture, np.float32) if cover is not None else None
            lib.tiles_perspective(n, *(a.ctypes.data for a in arrays), len(sx), kept.ctypes.data, len(kept), int(bool(cull)),
                                  float(bias[m_i] if bias else 0.0), None if tex is None else tex.ctypes.data,
                                  *((tex.shape[0], tex.shape[1], cover.channel, int(cover.second)) if tex is not None else (0, 0, 0, 0)),
                                  m_i, depth.ctypes.data, mid.ctypes.data, uv.ctypes.data, uv1.ctypes.data, nrm.ctypes.data)
            continue
        for tri in tris[keep]:
            xs, ys = sx[tri], sy[tri]
            x0, x1 = max(0, int(xs.min())), min(n - 1, int(np.ceil(xs.max())))
            y0, y1 = max(0, int(ys.min())), min(n - 1, int(np.ceil(ys.max())))
            if x0 > x1 or y0 > y1:
                continue
            d = (ys[1] - ys[2]) * (xs[0] - xs[2]) + (xs[2] - xs[1]) * (ys[0] - ys[2])
            if abs(d) < 1e-9 or (cull and d > 0 and cover is None):  # back faces culled (the mouth's inside shows through
                continue                                             # the cheek otherwise); cut-out cards (hair) two-sided
            yy, xx = np.mgrid[y0:y1 + 1, x0:x1 + 1] + 0.5
            w0 = ((ys[1] - ys[2]) * (xx - xs[2]) + (xs[2] - xs[1]) * (yy - ys[2])) / d
            w1 = ((ys[2] - ys[0]) * (xx - xs[2]) + (xs[0] - xs[2]) * (yy - ys[2])) / d
            w2 = 1 - w0 - w1
            inside = (w0 >= 0) & (w1 >= 0) & (w2 >= 0)
            if not inside.any():
                continue
            iz = 1 / z[tri]  # perspective-correct interpolation
            p0, p1, p2 = w0 * iz[0], w1 * iz[1], w2 * iz[2]
            s = p0 + p1 + p2
            zz = 1 / s - (bias[m_i] if bias else 0.0)
            region = depth[y0:y1 + 1, x0:x1 + 1]
            win = inside & (zz < region)
            if not win.any():
                continue
            bw = np.stack([p0[win], p1[win], p2[win]], 1) / s[win][:, None]
            t_uv, t_uv1 = bw @ m['uv'][tri], bw @ m['uv1'][tri]
            if cover is not None:
                keep_px = cover(t_uv, t_uv1)
                if not keep_px.any():
                    continue
                win[win] = keep_px
                bw, t_uv, t_uv1 = bw[keep_px], t_uv[keep_px], t_uv1[keep_px]
            region[win] = zz[win]
            mid[y0:y1 + 1, x0:x1 + 1][win] = m_i
            uv[y0:y1 + 1, x0:x1 + 1][win] = t_uv
            uv1[y0:y1 + 1, x0:x1 + 1][win] = t_uv1
            nrm[y0:y1 + 1, x0:x1 + 1][win] = bw @ vn[tri]
    return dict(mid=mid, uv=uv, uv1=uv1, nrm=nrm)


def sample(texture, uv):
    """Bilinear, wrapping; texture HxWx4 float."""
    if (lib := _native()) is not None and texture.dtype == np.float32 and texture.ndim == 3 and texture.shape[2] == 4             and uv.dtype == np.float64 and uv.ndim == 2:  # tiles_native.rs, bit for bit
        tex, uv = np.ascontiguousarray(texture), np.ascontiguousarray(uv)
        out = np.empty((len(uv), 4))
        lib.tiles_sample(tex.ctypes.data, tex.shape[0], tex.shape[1], uv.ctypes.data, len(uv), out.ctypes.data)
        return out
    h, w = texture.shape[:2]
    x = (uv[:, 0] % 1) * w - 0.5
    y = (uv[:, 1] % 1) * h - 0.5
    x0, y0 = np.floor(x).astype(int), np.floor(y).astype(int)
    fx, fy = (x - x0)[:, None], (y - y0)[:, None]
    g = lambda yy, xx: texture[yy % h, xx % w]
    return (g(y0, x0) * (1 - fx) * (1 - fy) + g(y0, x0 + 1) * fx * (1 - fy)
            + g(y0 + 1, x0) * (1 - fx) * fy + g(y0 + 1, x0 + 1) * fx * fy)


def to_linear(c):
    c = np.asarray(c, float)
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_srgb(c):
    c = np.clip(c, 0, 1)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


# Light rig (view space; the camera looks down -Z). Calibrated on native tiles: see previews-report.md.
LIGHT = dict(key=np.array([-0.45, 0.55, 0.7]), fill=np.array([0.6, 0.1, 0.5]), ambient=0.1444, key_gain=0.0519, fill_gain=0.6941, rim=0.0012, exposure=2.1177)


def shade(gbuf, materials, light=None):
    """materials[mesh id] = dict(texture, tint, opacity texture or None, alpha_test)."""
    lt = {**LIGHT, **(light or {})}
    key = lt['key'] / np.linalg.norm(lt['key'])
    fill = lt['fill'] / np.linalg.norm(lt['fill'])
    n = SIZE * SS
    out = np.zeros((n, n, 4))
    for m_i, mat in enumerate(materials):
        sel = gbuf['mid'] == m_i
        if not sel.any():
            continue
        colour = sample(mat['texture'], gbuf['uv'][sel])
        normal = gbuf['nrm'][sel]
        normal = normal / np.maximum(np.linalg.norm(normal, axis=1, keepdims=True), 1e-9)
        # retail albedo (customiser_stamp.wgsl / character_albedo): diffuse^2 in its own space, times the linear tint
        albedo = colour[:, :3] ** 2 * to_linear(mat['tint']) * (np.asarray(SKIN) if mat.get('skin') else 1.0)
        lit = lt['ambient'] + lt['key_gain'] * np.clip(normal @ key, 0, None) + lt['fill_gain'] * np.clip(normal @ fill, 0, None)
        rim = lt['rim'] * (1 - np.abs(normal[:, 2])) ** 3
        rgb = np.clip(to_srgb(albedo * lit[:, None] * lt['exposure']) + rim[:, None], 0, 1)
        alpha = np.ones(len(rgb))  # cut-outs were discarded in rasterise (coverage)
        out[sel] = np.concatenate([rgb * alpha[:, None], alpha[:, None]], 1)
    small = out.reshape(SIZE, SS, SIZE, SS, 4).mean((1, 3))  # premultiplied box downsample
    b = BORDER
    small[:b] = small[-b:] = 0
    small[:, :b] = small[:, -b:] = 0
    a = small[..., 3:4]
    rgb = np.where(a > 0, small[..., :3] / np.maximum(a, 1e-6), 0)
    return Image.fromarray(np.uint8(np.round(np.concatenate([rgb, a], 2) * 255)), 'RGBA')


class Cover:
    """An alpha test (rasterise's coverage callback): channel `channel` of `texture` sampled at uv (uv1 if `second`)."""
    def __init__(self, texture, channel, second):
        self.texture, self.channel, self.second = texture, channel, second

    def __call__(self, uv, uv1):
        return sample(self.texture, uv1 if self.second else uv)[:, self.channel] >= 0.5


def coverage_of(mat):
    """The alpha test a material needs, as rasterise's coverage callback (None: opaque)."""
    if mat.get('opacity') is not None:  # hair: red-channel opacity on the second UVs (customiser_stamp.wgsl)
        return Cover(mat['opacity'], 0, True)
    if mat.get('alpha_test'):
        return Cover(mat['texture'], 3, False)
    return None


_textures = {}


def texture(path):
    if path not in _textures:
        _textures[path] = np.asarray(Image.open(path).convert('RGBA'), dtype=np.float32) / 255
        if len(_textures) > 64:
            _textures.pop(next(iter(_textures)))
    return _textures[path]


def angles_from(back, right):
    """yaw, pitch, roll of the camera whose view +Z (toward the eye) is `back` and screen-right is `right`."""
    b = np.asarray(back, float) / np.linalg.norm(back)
    p = np.degrees(np.arcsin(np.clip(b[1], -1, 1)))
    y = np.degrees(np.arctan2(-b[0], b[2]))
    M = camera(y, p)
    r0 = np.asarray(right, float) - b * (np.asarray(right, float) @ b)
    r0 /= np.linalg.norm(r0)
    roll = np.degrees(np.arctan2(-(r0 @ M[1]), r0 @ M[0]))
    return float(y), float(p), float(roll)


# ---- outfit (customiser_parts.rs, the parts a tile shows) ------------------------------------------
def flag(m, k):
    return (m or {}).get('flags', {}).get(k, '')


def resolve(library, gender, sel):
    """Python mirror of customiser_parts.rs's outfit resolution (body parts a garment needs, hair under
    a hat, removals, skin tone). sel: slot -> (model id, material id)."""
    sel, M, mats = dict(sel), library['models'], library['materials']
    model = lambda s: M.get(sel[s][0]) if s in sel else None
    first = lambda p: next((i for i in p['materials'] if flag(mats.get(i), 'IsDefault') == 'yes'),
                           next((i for i in p['materials'] if i in mats), None))

    def choose(slot, key, value):
        if not value or value == 'none':
            sel.pop(slot, None)
        elif not (model(slot) and flag(model(slot), key) == value):
            c = sorted((i, p) for i, p in M.items() if p['slot'] == slot and flag(p, 'Gender') in ('', gender, 'unisex') and flag(p, key) == value)
            if c:
                sel[slot] = (c[0][0], first(c[0][1]))
    if (o := model('OuterTorso')):
        choose('Arm', 'ArmModelType', flag(o, 'ArmModelRequired'))
        choose('InnerTorso', 'InnerCutType', flag(o, 'InnerCutTypeRequired'))
        if flag(o, 'SupportsWristItem') == 'no':
            sel.pop('WristItem', None)
        if flag(o, 'IsNecklaceRemoved') == 'yes':
            sel.pop('Jewellery', None)
    if (p := model('Pants')):
        choose('Leg', 'LegModelType', flag(p, 'LegModelRequired'))
    if (leg := model('Leg')):
        style = flag(mats.get(sel['Sock'][1]), 'SockStyle') if 'Sock' in sel else ''
        desired = '' if style in ('', 'none') else style
        have = flag(leg, 'RequiresSockStyle')
        if have != desired and not (desired == '' and have == 'none'):
            c = sorted((i, q) for i, q in M.items() if q['slot'] == 'Leg' and flag(q, 'Gender') in ('', gender, 'unisex')
                       and flag(q, 'LegModelType') == flag(leg, 'LegModelType')
                       and (flag(q, 'RequiresSockStyle') == desired or (desired == '' and flag(q, 'RequiresSockStyle') == 'none')))
            # tight pants over socks: retail (CACPartDB 0x82ddccb8) falls back to the unisex tight legs for "tiny" socks
            c = c or (desired and flag(leg, 'LegModelType') == 'legs_tight' and sorted(
                (i, q) for i, q in M.items() if q['slot'] == 'Leg' and flag(q, 'Gender') == 'unisex'
                and flag(q, 'LegModelType') == 'legs_tight' and flag(q, 'RequiresSockStyle') == 'tiny'))
            if c:
                sel['Leg'] = (c[0][0], c[0][1]['materials'][0])
    if 'Sock' in sel:  # socks follow the legs (retail CASSimManager 0x82540d28): none without bare legs, else the
        need = flag(model('Leg'), 'RequiresSockStyle')  # length the legs are cut for; tiny socks take the legs off
        if not need or flag(mats.get(sel['Sock'][1]), 'SockStyle') != need:
            c = sorted((i, m) for i, q in M.items() if need and q['slot'] == 'Sock' for m in q['materials']
                       if flag(mats.get(m), 'SockStyle') == need)
            if c:
                sel['Sock'] = c[0]
            else:
                sel.pop('Sock')
        if 'Sock' in sel and flag(mats.get(sel['Sock'][1]), 'SockStyle') == 'tiny':
            sel.pop('Leg', None)
    hair = model('Hair')
    if 'Hat' not in sel and hair and flag(hair, 'HairModelType') != 'full':
        c = sorted((i, q) for i, q in M.items() if q['slot'] == 'Hair' and flag(q, 'HairStyle') == flag(hair, 'HairStyle')
                   and flag(q, 'Gender') == flag(hair, 'Gender') and flag(q, 'HairModelType') == 'full')
        if c:
            sel['Hair'] = (c[0][0], c[0][1]['materials'][0])
    if (hat := model('Hat')):
        need = flag(hat, 'RequiresHairModelType')
        if need == 'none':
            sel.pop('Hair', None)
        elif (hair := model('Hair')) and need and flag(hair, 'HairModelType') != need:
            c = sorted((i, q) for i, q in M.items() if q['slot'] == 'Hair' and flag(q, 'HairStyle') == flag(hair, 'HairStyle')
                       and flag(q, 'HairModelType') == need and flag(q, 'Gender') in ('', gender, 'unisex'))
            if c:
                sel['Hair'] = (c[0][0], c[0][1]['materials'][0])
            else:
                sel.pop('Hair', None)
    for s in {s.strip() for k in list(sel) for s in flag(model(k), 'RequiresRemovalOfComponents').split(',') if s.strip()}:
        sel.pop(s, None)
    tone = flag(mats.get(sel['Rostral'][1]), 'SkinTone') if 'Rostral' in sel else ''
    for slot, (a, mt) in list(sel.items()):  # every skin part takes the head's tone, fewest other flags changed
        old = mats.get(mt)
        if tone and flag(old, 'SkinTone') and slot != 'Rostral' and a in M:
            same = [(sum(old['flags'].get(k) != v for k, v in mats[i]['flags'].items() if k != 'SkinTone'), i)
                    for i in M[a]['materials'] if i in mats and flag(mats[i], 'SkinTone') == tone]
            if same:
                sel[slot] = (a, min(same)[1])
    return {s: v for s, v in sel.items() if v[0] in M and v[1] in mats}


BARE_LEG = {'male': 'thigh', 'female': 'thigh down'}  # the native sock tiles show bare legs, no pants
SOCK_FEET = ('85b73da337acc9ce', '2c7f381700171aa7')  # and the default skater's white shoes (library defaults Feet model, cream-sole design), not the campus vulc (round 2)


def outfit(library, folder, slot, model_id, material_id):
    """The parts a tile in `folder` (e.g. 'male/hat') shows: the mannequin wearing the item."""
    gender, kind = folder.rsplit('/', 1) if '/' in folder else ('male', folder)
    if kind in BOARD_FOLDERS:
        return {**BOARD, slot: (model_id, material_id)}
    sel = dict(MANNEQUIN[gender])
    sel[slot] = (model_id, material_id)
    if kind == 'sock':
        sel.pop('Pants', None)
        sel['Feet'] = SOCK_FEET
        legs = sorted(i for i, p in library['models'].items() if p['slot'] == 'Leg' and flag(p, 'Gender') in (gender, 'unisex')
                      and flag(p, 'LegModelType') == BARE_LEG[gender] and flag(p, 'RequiresSockStyle') in ('', 'none'))
        if legs:
            sel['Leg'] = (legs[0], library['models'][legs[0]]['materials'][0])
    sel = resolve(library, gender, sel)
    shown = SHOWN.get(ALIAS.get(kind, kind))
    return sel if shown is None else {s: v for s, v in sel.items() if s == slot or s in shown}


# ---- cameras -------------------------------------------------------------------------------------
def orbit_cam(t):
    return dict(eye=eye_from(t['target'], t['yaw'], t['pitch'], t['dist'], t.get('roll', 0)), yaw=t['yaw'], pitch=t['pitch'],
                roll=t.get('roll', 0), fov=t['fov'])


def framed_cam(points, rule):
    """Item-framed tile: look along the rule's direction at the item's bbox centre (shifted by ox/oy of
    its size), its largest extent spanning `span` of the tile."""
    lo, hi = points.min(0), points.max(0)
    size = max(float((hi - lo).max()), 1e-3)
    R = camera(rule['yaw'], rule['pitch'], rule.get('roll', 0))
    centre = (lo + hi) / 2 + R[0] * rule.get('ox', 0) * size + R[1] * rule.get('oy', 0) * size
    dist = size / rule['span'] / (2 * np.tan(np.radians(rule['fov']) / 2))
    return dict(eye=eye_from(centre, rule['yaw'], rule['pitch'], dist, rule.get('roll', 0)), yaw=rule['yaw'], pitch=rule['pitch'],
                roll=rule.get('roll', 0), fov=rule['fov'])


def projected_box(points, cam):
    R = camera(cam['yaw'], cam['pitch'], cam.get('roll', 0))
    v = (points - np.asarray(cam['eye'])) @ R.T
    f = 1 / np.tan(np.radians(cam['fov']) / 2)
    z = np.maximum(-v[:, 2], 1e-6)
    x, y = (0.5 + 0.5 * f * v[:, 0] / z) * SIZE, (0.5 - 0.5 * f * v[:, 1] / z) * SIZE
    return x.min(), y.min(), x.max(), y.max()


def template_key(folder):
    gender, kind = folder.rsplit('/', 1) if '/' in folder else ('', folder)
    kind = ALIAS.get(kind, kind)
    return f'{gender}/{kind}' if gender else kind


def tile_camera(folder, item_points, back=False):
    """The folder's camera and pose. An item the fixed ring camera does not show whole (a Skate 1 belt
    or a pro's cigarette under Accessories) is framed on its own by the template's 'framed' rule; a backpack
    (back=True) by 'framed_back', from behind."""
    key = template_key(folder)
    if key in FRAMED:
        return framed_cam(item_points, FRAMED[key]), {}
    t = TEMPLATES[key]
    if 'framed' in t:  # item-only folder (rings): the fixed native camera; an item elsewhere (another finger) is aimed at alike
        centre = (item_points.min(0) + item_points.max(0)) / 2
        home = np.asarray(t['item_centre'])
        cam = orbit_cam(t if np.linalg.norm(centre - home) < 0.03 else {**t, 'target': centre + np.asarray(t['target']) - home})
        x0, y0, x1, y1 = projected_box(item_points, cam)
        if x0 < BORDER or y0 < BORDER or x1 > SIZE - BORDER or y1 > SIZE - BORDER:
            return framed_cam(item_points, t['framed_back' if back else 'framed']), {}
        return cam, {}
    return orbit_cam(t), t.get('pose', {})


# ---- drawing -------------------------------------------------------------------------------------
# Colourizable designs: every native tile shows the palette's Red (library 'colours': 0.5, 0.1, 0.1). Short and tiny
# socks are white: their grey texture under the palette's White (0.9) draws grey, the natives' are ~2.4x brighter
# (linear, measured on the native regular / tiny sock tiles), so their tint is 1.45 (sRGB-encoded).
RED, WHITE = [0.5, 0.1, 0.1], [1.45, 1.45, 1.45]


def tile_tint(slot, m, gender='male'):
    if slot == 'Hair':
        return HAIR_TINT[gender]
    if flag(m, 'IsColourizable').lower() == 'true' and not slot.startswith('Skate'):  # native stampable decks show white
        return WHITE if slot == 'Sock' and flag(m, 'SockStyle') in ('short', 'tiny') else RED
    return m['tint']


def material(assets, library, slot, mid, item=True, gender='male'):
    m = library['materials'][mid]
    return dict(texture=texture(assets/m['diffuse']), tint=tile_tint(slot, m, gender) if item or slot == 'Hair' else m['tint'],
                opacity=texture(assets/m['opacity']) if m.get('opacity') else None, alpha_test=bool(m.get('alpha')),
                skin=(m.get('lighting') or {}).get('shader') in ('character.skin', 'character.face'))


def swap_sides(name):
    return name.replace('LEFT', '#').replace('RIGHT', 'LEFT').replace('#', 'RIGHT')


def mirrored(m):
    """The mesh reflected left <-> right (x -> -x, winding and joint sides swapped)."""
    out = {**m, 'pos': m['pos'] * [-1, 1, 1], 'nrm': m['nrm'] * [-1, 1, 1], 'idx': m['idx'][:, ::-1]}
    if 'names' in m:
        out['names'] = [swap_sides(n) for n in m['names']]
        out['joint_pos'] = {swap_sides(k): v * [-1, 1, 1] for k, v in m['joint_pos'].items()}
    return out


def board_mask():
    """The native board tiles' frame (union of every native deck/truck/wheel tile's alpha): content in px 5..122 with
    5 px rounded corners."""
    k = SIZE / 128
    y, x = (np.mgrid[0:SIZE, 0:SIZE] + 0.5) / k
    lo, hi, r = 5, 123, 5
    d = np.hypot(x - np.clip(x, lo + r, hi - r), y - np.clip(y, lo + r, hi - r))
    return np.clip((r + 0.5 / k - d) * k, 0, 1)


def render_model(assets, library, folder, model_id, materials=None):
    """Yield (material id, RGBA 128x128 tile) for a library model's designs, drawn on `folder`'s template
    (folder = the tile's data/fe/cas folder: 'male/hat', 'female/outertorso', ...)."""
    part = library['models'][model_id]
    materials = [m for m in (materials or part['materials']) if m in library['materials']]
    if not materials:
        return
    slot = part['slot']
    sel = outfit(library, folder, slot, model_id, materials[0])
    slots = list(sel)
    meshes = [load_glb(str(assets/library['models'][sel[s][0]]['scene'])) for s in slots]
    i = slots.index(slot)
    # a right-wrist-only item: draw it as the template draws the left wrist, then mirror the tile back
    mirror = template_key(folder).endswith('wristitem') and meshes[i]['pos'][:, 0].mean() < 0
    if mirror:
        meshes = [mirrored(m) for m in meshes]
    cam, pose = tile_camera(folder, meshes[i]['pos'], flag(part, 'AccessoryType') == 'backpack')
    context = [material(assets, library, s, sel[s][1], s == slot, folder.split('/')[0]) for s in slots]
    covers = [coverage_of(m) for m in context]
    bias = [BIAS.get(s, 0.0) for s in slots]
    cull = SHOWN.get(ALIAS.get(folder.rsplit('/', 1)[-1], folder.rsplit('/', 1)[-1])) != set()  # an item alone: two-sided
    # one raster per model, unless the item's designs cut out (their alpha test differs per design: coverage_of(material))
    cuts_out = lambda mid: bool(library['materials'][mid].get('opacity') or library['materials'][mid].get('alpha'))  # noqa: E731
    shared = None if any(map(cuts_out, materials)) else rasterise(meshes, cam, pose, covers[:i] + [None] + covers[i + 1:], bias, cull)
    for mid in materials:  # a design's texture loaded when drawn: a model's 300 designs at once were ~1 GB a lane
        design = material(assets, library, slot, mid)
        gbuf = shared or rasterise(meshes, cam, pose, covers[:i] + [coverage_of(design)] + covers[i + 1:], bias, cull)
        image = shade(gbuf, context[:i] + [design] + context[i + 1:])
        if folder in BOARD_FOLDERS:
            a = np.asarray(image).copy()
            a[..., 3] = np.round(a[..., 3] * board_mask()).astype(np.uint8)
            image = Image.fromarray(a, 'RGBA')
        yield mid, image.transpose(Image.Transpose.FLIP_LEFT_RIGHT) if mirror else image


def save_tile(folder, stem, image, mark, marker):
    """A drawn 128x128 Edit Skater tile: raw RGBA and PNG with the game's logo badge stamped in, and the screen's manifest."""
    import json, shutil
    from s2badge import stamped
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    rgba = folder/f'0000_{stem}.Texture.rgba'
    image = stamped(image, mark)
    rgba.write_bytes(image.tobytes())
    image.save(rgba.with_suffix('.png'))
    (folder/'manifest.json').write_text(json.dumps({'textures': [{'rgba_file': rgba.name, 'width': 128, 'height': 128}], **marker}))


def draw_tiles(assets, library_path, jobs, thumb_root, mark, marker, count=None):
    """render_model for many (folder, model id, [(design id, tile key, stem), ...]) jobs, drawn by fanout workers on
    several processes and saved straight to thumb_root/<tile key> (save_tile, `mark` the badge, `marker` the manifest's
    extra fields). Each job is drawn exactly as render_model draws it (one process, one model)."""
    import json, tempfile
    from tools.pipeline import fanout
    if not jobs:
        return
    # a tile key twice: its last drawn design wins, as when the tiles are saved one after another
    materials = json.loads(Path(library_path).read_text())['materials']
    owner = {key: (j, d) for j, (_, _, designs) in enumerate(jobs) for d, (mid, key, _) in enumerate(designs) if mid in materials}
    jobs = [(folder, model_id, [(mid, key, stem, owner.get(key) == (j, d)) for d, (mid, key, stem) in enumerate(designs)])
            for j, (folder, model_id, designs) in enumerate(jobs)]
    with tempfile.TemporaryDirectory(prefix='s3r-badge-') as temp:
        badge = Path(temp)/'badge.png'
        mark.save(badge)  # PNG keeps every RGBA byte
        with fanout.fan_out(__file__, jobs, (assets, library_path, thumb_root, badge, json.dumps(marker)),
                            size=lambda job: 4 + len(job[2]), count=count or min(4, fanout.workers(0.8))):  # 0.72 GB a lane (10-08)
            pass


if __name__ == '__main__':  # a draw_tiles worker: cas_tiles.py --lane PLAN --out DIR ASSETS LIBRARY THUMBS BADGE MARKER
    import json, sys
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]/'mixamo_to_skate'))  # glb
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))  # tools.pipeline.fanout
    from tools.pipeline import fanout
    jobs, out, (assets, library, thumbs, badge, marker) = fanout.lane(sys.argv[1:])
    library, marker = json.loads(Path(library).read_text()), json.loads(marker)
    with Image.open(badge) as image:
        mark = image.copy()
    for index, (folder, model_id, designs) in jobs:
        tiles = dict(render_model(Path(assets), library, folder, model_id, [mid for mid, *_ in designs]))
        for mid, key, stem, last in designs:
            if last:  # drawn (in the library), and the key's last
                save_tile(Path(thumbs)/key, stem, tiles[mid], mark, marker)
