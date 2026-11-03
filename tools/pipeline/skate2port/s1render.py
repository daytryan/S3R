"""CPU thumbnails for Skate 1 parts (Skate 1 had no per-item pictures), styled like the retail
Skate 3 ones: the item on the default skater, slot framing, soft light, transparent background.

A scene is rasterised once per model into a G-buffer (mesh id, UV, normal); every design
of the model is then only a texture lookup, so hundreds of tee designs cost one raster."""
import functools, json
from pathlib import Path
import numpy as np
from PIL import Image

SIZE, SS = 128, 4  # output size, supersampling
ARM_DROP = np.radians(38)  # A-pose bind arms -> hanging, for torso shots
ARM_CHAIN = ('Arm', 'ArmTwist', 'ForeArm', 'ForeArmTwist', 'ForeArmTwist1', 'Hand')


def load_glb(path):
    from glb import Document
    doc = Document(path)
    j = doc.doc
    prim = j['meshes'][0]['primitives'][0]
    a = prim['attributes']
    mesh = dict(pos=doc.accessor(a['POSITION']).astype(np.float64), nrm=doc.accessor(a['NORMAL']).astype(np.float64),
                uv=doc.accessor(a['TEXCOORD_0']).astype(np.float64), idx=np.concatenate([doc.accessor(p['indices']) for p in j['meshes'][0]['primitives']]).reshape(-1, 3).astype(np.int64))  # a deck's grip: character_glb.py
    if j.get('skins') and 'JOINTS_0' in a:
        skin = j['skins'][0]
        names = [j['nodes'][i].get('name', '') for i in skin['joints']]
        ibm = doc.accessor(skin['inverseBindMatrices']).reshape(-1, 4, 4)
        joints = doc.accessor(a['JOINTS_0']).astype(np.int64)
        weights = doc.accessor(a['WEIGHTS_0']).astype(np.float64)
        pose_arms(mesh, names, ibm, joints, weights)
    return mesh


def pose_arms(mesh, names, ibm, joints, weights):
    """Rotate each arm chain down about its shoulder-ball joint (linear blend skinning)."""
    upper = [n.upper() for n in names]  # converted rigs name joints LEFTARM, LEFTHAND_REPARENTED, ...
    for side, sign in (('LEFT', -1), ('RIGHT', 1)):
        if side + 'ARM' not in upper:
            continue
        pivot = np.linalg.inv(ibm[upper.index(side + 'ARM')].T)[:3, 3]
        # arm hangs toward -Y: rotate about Z (left arm is at +X)
        c, s = np.cos(sign * ARM_DROP), np.sin(sign * ARM_DROP)
        rot = np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]])
        chain = np.array([n in {side + p.upper() for p in ARM_CHAIN} or n.startswith(side + 'HAND') for n in upper])
        w = (weights * chain[joints]).sum(1)[:, None]
        mesh['pos'] = mesh['pos'] + w * (((mesh['pos'] - pivot) @ rot.T + pivot) - mesh['pos'])
        mesh['nrm'] = mesh['nrm'] + w * (mesh['nrm'] @ rot.T - mesh['nrm'])


def camera(yaw, pitch):
    y, p = np.radians(yaw), np.radians(pitch)
    ry = np.array([[np.cos(y), 0, np.sin(y)], [0, 1, 0], [-np.sin(y), 0, np.cos(y)]])
    rx = np.array([[1, 0, 0], [0, np.cos(p), -np.sin(p)], [0, np.sin(p), np.cos(p)]])
    return rx @ ry  # world -> view (view looks down -Z, character faces +Z)


def one_band(points):
    """A pair of wristbands (one per wrist): the left one."""
    return points[points[:, 0] > 0] if points[:, 0].min() < -0.2 and points[:, 0].max() > 0.2 else points


def ring_view(points, tilt=np.radians(40)):
    """View a band (bracelet, watch) 40 degrees off its axis: an open ellipse, face toward +Y."""
    centred = points - points.mean(0)
    _, vectors = np.linalg.eigh(centred.T @ centred)
    axis = vectors[:, 0]  # least spread = the band's axis
    side = np.cross(axis, [0, 1, 0]) if abs(axis[1]) < 0.9 else np.cross(axis, [1, 0, 0])
    side /= np.linalg.norm(side)
    toward = axis * np.cos(tilt) + np.cross(side, axis) * np.sin(tilt)  # camera direction
    toward /= np.linalg.norm(toward)
    if toward[1] < 0:
        toward = -toward
    right = np.cross([0, 1, 0], toward) if abs(toward[1]) < 0.99 else np.array([1., 0, 0])
    right /= np.linalg.norm(right)
    return np.array([right, np.cross(toward, right), toward])


@functools.cache
def _native():
    """tiles_native.rs (tiles.dll, shared with cas_tiles): rasterise's triangle loop in Rust; None keeps Python."""
    import ctypes, sys
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools import native
    lib = native.load('tiles', __file__)
    if lib is not None:
        P, I = ctypes.c_void_p, ctypes.c_int64
        lib.tiles_orthographic.argtypes = [I, P, P, P, P, P, I, P, I, ctypes.c_int16, P, P, P, P]
        lib.tiles_orthographic.restype = None
        lib.tiles_sample.argtypes = [P, I, I, P, I, P]
        lib.tiles_sample.restype = None
    return lib


def rasterise(meshes, view, frame, roll=0.0):
    """G-buffer for meshes (list of dict pos/nrm/uv/idx) framed on world-space points (Nx3)."""
    n = SIZE * SS
    if roll:
        cr, sr = np.cos(np.radians(roll)), np.sin(np.radians(roll))
        view = np.array([[cr, -sr, 0], [sr, cr, 0], [0, 0, 1]]) @ view
    projected = frame @ view.T
    lo, hi = projected.min(0), projected.max(0)
    scale = 0.9 * n / max(hi[0] - lo[0], hi[1] - lo[1])
    centre = (lo + hi) / 2
    depth = np.full((n, n), -np.inf)
    mid = np.full((n, n), -1, np.int16)
    uv = np.zeros((n, n, 2))
    nrm = np.zeros((n, n, 3))
    lib = _native()
    for m_i, mesh in enumerate(meshes):
        v = mesh['pos'] @ view.T
        sx = (v[:, 0] - centre[0]) * scale + n / 2
        sy = n / 2 - (v[:, 1] - centre[1]) * scale
        vn = mesh['nrm'] @ view.T
        if lib is not None:  # the loop below in Rust (tiles_native.rs), bit for bit
            tris = np.ascontiguousarray(mesh['idx'], np.int64)
            arrays = [np.ascontiguousarray(a, np.float64) for a in (sx, sy, v, vn, mesh['uv'])]
            lib.tiles_orthographic(n, *(a.ctypes.data for a in arrays), len(sx), tris.ctypes.data, len(tris), m_i,
                                   depth.ctypes.data, mid.ctypes.data, uv.ctypes.data, nrm.ctypes.data)
            continue
        for tri in mesh['idx']:
            xs, ys = sx[tri], sy[tri]
            x0, x1 = max(0, int(xs.min())), min(n - 1, int(np.ceil(xs.max())))
            y0, y1 = max(0, int(ys.min())), min(n - 1, int(np.ceil(ys.max())))
            if x0 > x1 or y0 > y1:
                continue
            d = (ys[1] - ys[2]) * (xs[0] - xs[2]) + (xs[2] - xs[1]) * (ys[0] - ys[2])
            if abs(d) < 1e-9:
                continue
            yy, xx = np.mgrid[y0:y1 + 1, x0:x1 + 1] + 0.5
            w0 = ((ys[1] - ys[2]) * (xx - xs[2]) + (xs[2] - xs[1]) * (yy - ys[2])) / d
            w1 = ((ys[2] - ys[0]) * (xx - xs[2]) + (xs[0] - xs[2]) * (yy - ys[2])) / d
            w2 = 1 - w0 - w1
            inside = (w0 >= 0) & (w1 >= 0) & (w2 >= 0)
            if not inside.any():
                continue
            z = w0 * v[tri[0], 2] + w1 * v[tri[1], 2] + w2 * v[tri[2], 2]
            region = depth[y0:y1 + 1, x0:x1 + 1]
            win = inside & (z > region)
            if not win.any():
                continue
            region[win] = z[win]
            mid[y0:y1 + 1, x0:x1 + 1][win] = m_i
            bw = np.stack([w0[win], w1[win], w2[win]], 1)
            uv[y0:y1 + 1, x0:x1 + 1][win] = bw @ mesh['uv'][tri]
            nrm[y0:y1 + 1, x0:x1 + 1][win] = bw @ vn[tri]
    return dict(mid=mid, uv=uv, nrm=nrm)


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


def shade(gbuf, materials):
    """materials[mesh id] = (texture HxWx4 float 0..1, tint rgb, alpha-tested)."""
    n = SIZE * SS
    out = np.zeros((n, n, 4))
    key = np.array([-0.45, 0.55, 0.7]); key /= np.linalg.norm(key)
    fill = np.array([0.6, 0.1, 0.5]); fill /= np.linalg.norm(fill)
    for m_i, (texture, tint, alpha_test) in enumerate(materials):
        sel = gbuf['mid'] == m_i
        if not sel.any():
            continue
        colour = sample(texture, gbuf['uv'][sel])
        normal = gbuf['nrm'][sel]
        normal /= np.maximum(np.linalg.norm(normal, axis=1, keepdims=True), 1e-9)
        light = 0.42 + 0.62 * np.clip(normal @ key, 0, None) + 0.22 * np.clip(normal @ fill, 0, None)
        rim = 0.18 * (1 - np.abs(normal[:, 2])) ** 3
        rgb = np.clip(colour[:, :3] * np.asarray(tint) * light[:, None] + rim[:, None], 0, 1)
        alpha = (colour[:, 3] >= 0.5).astype(float) if alpha_test else np.ones(len(rgb))
        out[sel] = np.concatenate([rgb * alpha[:, None], alpha[:, None]], 1)
    # premultiplied box downsample
    small = out.reshape(SIZE, SS, SIZE, SS, 4).mean((1, 3))
    a = small[..., 3:4]
    rgb = np.where(a > 0, small[..., :3] / np.maximum(a, 1e-6), 0)
    return Image.fromarray(np.uint8(np.round(np.concatenate([rgb, a], 2) * 255)), 'RGBA')


_textures = {}


def texture(path):
    if path not in _textures:
        _textures[path] = np.asarray(Image.open(path).convert('RGBA'), dtype=np.float32) / 255
        if len(_textures) > 64:
            _textures.pop(next(iter(_textures)))
    return _textures[path]


# slot -> (yaw, pitch, context parts, framing)
VIEWS = {
    'Hat': (28, -12, ('Rostral', 'Organ'), 'item+head'),
    'Glasses': (24, -6, ('Rostral', 'Organ', 'Hair'), 'face'),
    'OuterTorso': (22, -4, ('Rostral', 'Organ', 'Hair', 'Arm', 'Pants'), 'torso'),
    'Pants': (22, -4, ('Feet', 'Leg', 'OuterTorso'), 'legs'),
    'Feet': (58, -16, ('Pants',), 'left foot'),
    'Accessory': (18, -28, (), 'item'),  # Skate 1 belts, alone like the retail ring tiles
    'WristItem': (0, 0, (), 'ring'),  # thin bands hide behind the hand: alone, seen at an angle to the band
    'SkateBoard': (0, 0, (), 'item'),  # special camera below
    'SkateTruck': (35, 22, (), 'front truck'),
    'SkateWheel': (62, -10, (), 'one wheel'),
}


def framing(kind, item, context):
    """World-space points the camera frames."""
    pts = item['pos']
    everything = np.concatenate([m['pos'] for m in [item, *context]]) if context else pts
    corners = lambda lo, hi: np.array([[x, y, z] for x in (lo[0], hi[0]) for y in (lo[1], hi[1]) for z in (lo[2], hi[2])])
    if kind == 'ring':
        return one_band(pts)
    if kind == 'item':
        return pts
    if kind == 'front truck':  # the model holds both trucks; the retail tile shows one, axle ends cropped
        return pts[(pts[:, 2] > 0) & (np.abs(pts[:, 0]) < 0.075)]
    if kind == 'one wheel':
        return pts[(pts[:, 2] > 0) & (pts[:, 0] > 0)]
    if kind in ('item+head', 'face'):  # head down to the chin, like the retail hat/glasses tiles
        head = context[0]['pos']
        return np.concatenate([pts, head[head[:, 1] > head[:, 1].max() - 0.24]])
    if kind == 'torso':
        return everything[(everything[:, 1] > 0.9) & (everything[:, 1] < 1.54) & (np.abs(everything[:, 0]) < 0.3)]
    if kind == 'legs':
        return everything[(everything[:, 1] > 0.0) & (everything[:, 1] < 1.12)]
    if kind == 'left foot':
        p = pts[pts[:, 0] > 0]
        return np.concatenate([p, p + [0, 0.08, 0]])
    if kind == 'waist':
        mid = (pts.min(0) + pts.max(0)) / 2
        return corners(mid - [0.22, 0.12, 0.15], mid + [0.22, 0.12, 0.15])
    if kind == 'wrist':  # the item's side of the body (left is +X)
        side = pts[:, 0] > 0 if pts[:, 0].mean() > 0 else pts[:, 0] < 0
        p = pts[side]
        mid = (p.min(0) + p.max(0)) / 2
        return corners(mid - 0.09, mid + 0.09)
    raise ValueError(kind)


def render_model(assets, library, slot, model, context_parts):
    """Yield (material id, RGBA thumbnail) for every design of a library model."""
    yaw, pitch, parts, kind = VIEWS[slot]
    item = load_glb(assets/model['scene'])
    context, mats = [], []
    for part in parts:
        if part in context_parts:
            mesh_part, material = context_parts[part]
            context.append(load_glb(assets/mesh_part['scene']))
            mats.append((texture(assets/material['diffuse']), material['tint'], material.get('alpha') or bool(material.get('opacity'))))
    view = camera(yaw, pitch) if kind != 'ring' else ring_view(one_band(item['pos']))
    roll = 0.0
    if slot == 'SkateBoard':  # deck graphic faces down: look up at it, diagonal like the retail tiles
        view = np.array([[0, 0, 1], [-1, 0, 0], [0, -1, 0]], dtype=float)  # screen x = deck length, depth = -Y
        roll = 45
    gbuf = rasterise([item, *context], view, framing(kind, item, context), roll)
    for mid in model['materials']:
        material = library['materials'][mid]
        yield mid, shade(gbuf, [(texture(assets/material['diffuse']), material['tint'], material.get('alpha')), *mats])
