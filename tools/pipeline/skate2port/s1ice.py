"""Skate 1's ICE takes (scene.big data/scene/ice.bin) for old_tutorial's evaluator: the same chunk/record layout as
Skate 2's with 34 tracks per take, read with Skate 1's parameter table (ice_params1.json, re-derived from default.xex).
`T` is old_tutorial's library namespace; use(T, params) points its evaluator (ice_value, scene_camera, edit_shots) at
the Skate 1 table."""
import math, struct

import numpy as np

NT = 34  # Skate 1 takes have 34 tracks (Skate 2: 37); the edit track (ASSEMBLY) is 32, the shake data 33
# Skate 1 has no sun override: ice_frame's SUN_DIRECTION_* read their default (0, off) from an empty pseudo-track.
PHANTOM = [{'pos': 10000 + i, 'name': n, 'track': NT, 'flag': int(n.endswith('OVERRIDE')), 'type': 3, 'bits': 0,
            'default': 0.0, 'min': 0.0, 'max': 1.0, 'cubic': 65535, 'tension': 65535}
           for i, n in enumerate(('SUN_DIRECTION_X', 'SUN_DIRECTION_Y', 'SUN_DIRECTION_Z', 'SUN_DIRECTION_OVERRIDE'))]

# SHAKE_TYPE names (default.xex 0x82081e20: Default, Jog, Still, WalkFast, WalkSlow); 1..4 are ice.bin records hashed
# by name. 0 (Default) has no record.
SHAKE_NAMES = ['Jog', 'Still', 'WalkFast', 'WalkSlow']


def use(T, params):
    """old_tutorial's ICE tables -> Skate 1's (params sorted by pos), plus the phantom sun track."""
    T['ICE_PARAMS'], T['ICE'], T['ICE_POS'] = params, {p['name']: p for p in params + PHANTOM}, {p['pos']: p for p in params}


def ice_take(T, t):
    """old_tutorial.ice_take with Skate 1's layout: 34 (key count, value count) tracks at 0x14, the index and time
    arrays at 0x9c, then each parameter's values in ice_params1.json order (T['ICE_PARAMS'], after use)."""
    tracks = [struct.unpack_from('<HH', t, 0x14 + 4 * i) for i in range(NT)]
    base = 0x14 + 4 * NT
    n_index, n_time = sum(max(k - 2, 0) for k, _ in tracks), sum(max(k - 1, 0) for k, _ in tracks)
    index = struct.unpack_from(f'<{n_index}H', t, base)
    times = struct.unpack_from(f'<{n_time}H', t, base + 2 * n_index)
    keys, i, j = [], 0, 0
    for k, n in tracks:
        keys.append(([0] + list(index[i:i + max(k - 2, 0)]) + [n - 2] if k > 1 else [0] * k,
                     [0.] + [x / 65535 for x in times[j:j + max(k - 1, 0)]] + [1.]))
        i, j = i + max(k - 2, 0), j + max(k - 1, 0)
    keys.append(([], [0., 1.]))  # the phantom track
    o = (base + 2 * (n_index + n_time) + 3) & ~3
    values = {p['name']: [] for p in PHANTOM}
    for p in T['ICE_PARAMS']:
        k, n = tracks[p['track']]
        n = k if p['flag'] else n
        size = ((n * p['bits'] + 31) >> 3) & ~3
        block = t[o:o + size]
        if p['type'] == 4:
            values[p['name']] = list(struct.unpack_from(f'<{n}f', block))
        else:
            bits, mask = int.from_bytes(block, 'big'), (1 << p['bits']) - 1
            raw = [(bits >> (8 * size - (x + 1) * p['bits'])) & mask for x in range(n)]
            values[p['name']] = [T['ice_dequant'](p, x) for x in raw] if p['type'] == 3 else raw
        o += size
    return {'duration': struct.unpack_from('<f', t, 0xc)[0], 'keys': keys, 'values': values, 'size': o, 'tracks': tracks}


def ice_cameras(T, e):
    """scene name hash -> takes, as old_tutorial.ice_cameras (same chunk/record layout); every record must decode to
    exactly its size."""
    out = {}
    o, end = 8, 8 + struct.unpack_from('<I', e, 4)[0]
    while o < end:
        chunk_end = o + 8 + struct.unpack_from('<I', e, o + 4)[0]
        r = o + 8
        while r + 8 <= chunk_end:
            _, size, h, n = struct.unpack_from('<4I', e, r)
            takes, at = [], r + 0x10
            for _ in range(n):
                takes.append(ice_take(T, e[at:r + 8 + size]))
                at = (at + takes[-1]['size'] + 3) & ~3
            assert at == r + 8 + size, f'ice record {h:#x} decodes to {at - r - 8} of {size} bytes'
            out[h] = takes
            r += 8 + size
        o = chunk_end
    return out


def use_shakes(T, cameras, label_key):
    """old_tutorial's shake curves -> Skate 1's: ice.bin records 290-293 (cameras from ice_cameras), hashed by name like
    the scenes (label_key), as old_tutorial.ice_shakes' (frame count, rows); T's ice_shake becomes Skate 1's."""
    shakes = [None]
    for name in SHAKE_NAMES:
        take, = cameras[label_key(name)]
        cols = [take['values'][n] for n in ('SHAKE_QUAT_X', 'SHAKE_QUAT_Y', 'SHAKE_QUAT_Z', 'SHAKE_POS_X', 'SHAKE_POS_Y', 'SHAKE_POS_Z')]
        shakes.append((take['tracks'][33][0], list(zip(*cols))))
    T['ice_shakes'] = lambda: shakes
    T['ice_shake'] = lambda take, u, left, fov: ice_shake(T, take, u, left, fov)


def noise(n):
    """0x82c26220's lattice value at integer n (Hugo Elias): int32 arithmetic, only the low 31 bits survive the mask."""
    m = ((n << 13) ^ n) & 0xffffffff
    return 1 - ((m * (m * m * 15731 + 789221) + 1376312589) & 0x7fffffff) / 1073741824


def vnoise(x):
    """0x82c26220: lerp between the lattice values either side of x."""
    i = math.floor(x)
    return noise(i) + (noise(i + 1) - noise(i)) * (x - i)


def fbm(x):
    """0x82c26328: six octaves, each twice the rate and half the weight of the last (range about +-1.97)."""
    return sum(vnoise(x * 2 ** o) * 0.5 ** o for o in range(6))


def ice_shake(T, take, u, left, fov):
    """Skate 1's camera shake (applier 0x82c24eb0), as (turn, move) of the view's rows (forward, left, up), or None;
    ice_frame applies it as old_tutorial.ice_shake's (camera = S . camera). T = take seconds x SHAKE_FREQUENCY.
    INFERRED (research5.md 2): u is the applier's take fraction [blk+4]; the lens is the camera's half fov; the rows
    are (forward, left, up) as Skate 2's. ponytail: float angles; retail's u16 angles round to 0.0055 deg."""
    v = lambda n: T['ice_value'](take, n, u, left)
    amplitude = v('SHAKE_AMPLITUDE')
    if not amplitude:
        return None
    t = u * take['duration'] * v('SHAKE_FREQUENCY')
    kind = int(v('SHAKE_TYPE'))
    if kind:
        # 0x82c24f60-0x82c25104: one frame, no blend, scaled by the amplitude alone (Skate 2 adds a lens factor).
        frames, values = T['ice_shakes']()[kind]
        a = values[int(t * 30) % frames]
        qa, qb, qc = (c * amplitude for c in (a[2], a[0], -a[1]))  # quaternion (QZ, QX, -QY), w = sqrt(1 - |xyz|)
        w = math.sqrt(1 - math.sqrt(qa * qa + qb * qb + qc * qc))
        # The exe's matrix, as Skate 2's: the rotation of the quaternion, transposed.
        turn = np.array([[1 - 2 * (qb * qb + qc * qc), 2 * (qa * qb + qc * w), 2 * (qa * qc - qb * w)],
                         [2 * (qa * qb - qc * w), 1 - 2 * (qa * qa + qc * qc), 2 * (qb * qc + qa * w)],
                         [2 * (qa * qc + qb * w), 2 * (qb * qc - qa * w), 1 - 2 * (qa * qa + qb * qb)]])
        return turn, np.array([-a[5], -a[3], a[4]]) * amplitude
    # SHAKE_TYPE 0 (Default), 0x82c25108-0x82c252c8: fbm noise on four channels at golden-ratio rates.
    k = math.sin(fov / 2) / math.sin(math.pi / 6)
    rz = math.asin(math.sin(math.radians(6 * amplitude * fbm(1.618 * t))) * k)
    ry = math.asin(math.sin(math.radians(4 * amplitude * fbm(1.382 * t))) * k)
    cz, sz, cy, sy = math.cos(rz), math.sin(rz), math.cos(ry), math.sin(ry)
    turn = np.array([[cy, 0, -sy], [0, 1, 0], [sy, 0, cy]]) @ np.array([[cz, sz, 0], [-sz, cz, 0], [0, 0, 1]])
    return turn, np.array([0., 0.3 * amplitude * fbm(0.618 * t), 0.2 * amplitude * fbm(0.382 * t)])
