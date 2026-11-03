"""Skate 1 .abin reader: hierarchy (type 4), clips (type 2) and poses (type 3), codec word 1; export_clip writes one clip
as the tutorial scenes play it (anim1/<clip>.bin, see write_clip).

Port of s1.pe (research run 3, .superpowers/sdd/2026-10-14-s1-tutorial/research.md 4.3a for the spec and addresses):
  block chain        : blocks from 0x30, size@0 type@4 codec@8 FastString name@0x10; data = (blk+0x37)&~0xF (0x8224cd90)
  hierarchy          : 0x82253d18 (36-byte part records at skel+8+8n) / 0x82253788 (part-id fixup of clip and pose tables)
  bind compose       : 0x82c14948 (AnimCmdSys SQT op 8), see add_bind
  clip sampler       : 0x8225ec98 (frame = fps*t, A/B pair, part walk); pose sampler 0x8225eac8
  codec-1 part decode: 0x822621d8 (scalar) == 0x82260218 (VMX, the "AnimFetch" job body)
  packed quaternion  : 0x82262038, sin/cos pair 0x82261f48
Usage: py -3 s1abin.py FILE (block, part and clip dump). Tests: test_s1abin.py.
"""
import contextlib, io, math, struct, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]/"vendor"/"skate3_anim"))  # repo/tools/vendor/skate3_anim
with contextlib.redirect_stdout(io.StringIO()):
    from abin_importer import _decode6  # FastString36, same encoding as Skate 3

u32 = lambda d, o: struct.unpack_from(">I", d, o)[0]
f32s = lambda d, o, n: struct.unpack_from(">%df" % n, d, o)
TWO_PI_2047 = 0.003069460391998291    # 0x821912dc
PI_1023 = 0.0030709607526659966       # 0x821912e0
INV_2047 = 0.0004885197849944234      # 0x821912d8
HALF_PI = 1.5707963705062866          # 0x8215b144
INV_8191 = 0.00012208521366119385     # s16 translation scale (0x822621d8 / 0x82260218)


def fs(d, o, words):
    return "".join(_decode6(u32(d, o + 4 * i)) for i in range(words))


def aligned(blk):
    return (blk + 0x34 + 3) & ~0xF


def blocks(d):
    out, p = [], 0x30
    while p + 0x30 <= len(d):
        size = u32(d, p)
        if size == 0 or size > len(d) - p:
            break
        out.append(dict(off=p, size=size, type=u32(d, p + 4), codec=u32(d, p + 8), name=fs(d, p + 0x10, 6)))
        p += size
    return out


def hierarchy(d, blk):
    s = aligned(blk)
    n, nparts = struct.unpack_from(">H", d, s)[0], u32(d, s + 4)
    parents = list(struct.unpack_from(">%di" % n, d, s + 8))
    rec = s + 8 + 8 * n                       # 36-byte part records (0x82253d18: skel + 8*(n+1) + 36*i)
    parts = []
    for i in range(nparts):
        r = rec + 36 * i
        parts.append(dict(name=fs(d, r, 5), count=u32(d, r + 24), sqt=u32(d, r + 28),
                          clip=struct.unpack_from(">H", d, r + 32)[0], id=struct.unpack_from(">H", d, r + 34)[0]))
    nb = rec + 36 * nparts                    # bone names, FastString 20 bytes each
    names = [fs(d, nb + 20 * i, 5) for i in range(n)]
    return dict(n=n, parents=parents, parts=parts, names=names)


def part_header(d, h):
    m = struct.unpack_from(">6I", d, h)
    w6, dataoff = u32(d, h + 0x18), u32(d, h + 0x1C)
    return dict(at=h, scale=m[0] | m[1] << 32, rot=m[2] | m[3] << 32, trans=m[4] | m[5] << 32,
                rot_f32=bool(w6 >> 31 & 1), trans_s16=bool(w6 >> 30 & 1), skel_part=w6 >> 25 & 0x1F,
                frame_size=w6 >> 7 & 0x3FFFF, bones=(w6 >> 1 & 0x3F) + 1, data=h + dataoff, w6=w6)


def table(d, ad, at, count):
    return [part_header(d, ad + (u32(d, at + 4 * i) & 0xFFFFFF)) for i in range(count)]


def clip(d, blk):
    ad = aligned(blk)
    root = f32s(d, ad, 8)
    fps, frames = f32s(d, ad + 0x20, 2)
    flags = u32(d, ad + 0x34)
    count = flags >> 8 & 0x1F                 # 0x8225ec98 / 0x8225387c
    return dict(ad=ad, root=root, fps=fps, frames=int(frames), flags=flags, parts=table(d, ad, ad + 0x38, count))


def pose(d, blk):
    ad = aligned(blk)
    return dict(ad=ad, frames=1, parts=table(d, ad, ad + 4, u32(d, ad)))


def unpack_quat(q):
    """0x82262038: 11/10/11 bits -> (azimuth, elevation, sin(half angle)); unit by construction."""
    a, b, c = q >> 21 & 0x7FF, q >> 11 & 0x3FF, q & 0x7FF
    al, be, t = a * TWO_PI_2047 - math.pi, b * PI_1023 - HALF_PI, c * INV_2047
    k = 1.0 - t * t
    w = math.sqrt(k) if k > 0 else 0.0
    return (t * math.cos(be) * math.cos(al), t * math.cos(be) * math.sin(al), t * math.sin(be), w)


def frame_size(p, sqt0):
    size = 0
    for i in range(p["bones"]):
        size += 12 * (p["scale"] >> i & 1)
        size += (16 if p["rot_f32"] else 4) * (p["rot"] >> i & 1)
        size += (6 if p["trans_s16"] and sqt0 + i != 0 else 12) * (p["trans"] >> i & 1)
    return size


def decode(d, p, f, sqt0):
    """One frame of one part -> [(scale, quat xyzw, trans)] for bones sqt0.. (every frame stored, no keys)."""
    o, out = p["data"] + f * p["frame_size"], []
    for i in range(p["bones"]):
        s, q, t = (1.0, 1.0, 1.0), (0.0, 0.0, 0.0, 1.0), (0.0, 0.0, 0.0)
        if p["scale"] >> i & 1:
            s = f32s(d, o, 3); o += 12
        if p["rot"] >> i & 1:
            if p["rot_f32"]:
                q = f32s(d, o, 4); o += 16
            else:
                q = unpack_quat(u32(d, o)); o += 4
        if p["trans"] >> i & 1:
            if p["trans_s16"] and sqt0 + i != 0:
                t = tuple(v * INV_8191 for v in struct.unpack_from(">3h", d, o)); o += 6
            else:
                t = f32s(d, o, 3); o += 12
        out.append((s, q, t))
    return out


def sample(d, hier, anim, f):
    """Clip/pose frame f -> {bone index: sqt}. Part k of the table drives hierarchy part w6[25:30]
    (0x82253788 rewrites that field and the entry's high byte to the runtime part id; the sampler matches ids)."""
    res = {}
    for p in anim["parts"]:
        if not p["bones"] or not p["frame_size"] and not (p["scale"] | p["rot"] | p["trans"]):
            continue
        hp = hier["parts"][p["skel_part"]]
        for i, sqt in enumerate(decode(d, p, f, hp["sqt"])):
            res[hp["sqt"] + i] = sqt
    return res


def qmul(a, b):
    ax, ay, az, aw = a; bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def qrot(q, v):
    x, y, z, w = qmul(qmul(q, (*v, 0.0)), (-q[0], -q[1], -q[2], q[3]))
    return (x, y, z)


def add_bind(bind, clip_sqts, n):
    """Clip values are deltas on the bank's reference pose (the file's type-3 pose): q = qb*qc, t = tb + qb.tc,
    s = sb*sc, t.w from the clip. Kernel RETAIL: AnimCmdSys SQT-engine op 8 (switch 0x82c19eb8, case 0x82c19fcc)
    -> 0x82c14948, out = B o A. That A = clip and B = reference is INFERRED (Skate 2's traced rule, faces.md 9.1, and
    the data: the other order sinks the wheels 0.3 m under the trajectory, research.md 4.3a)."""
    I, out = ((1.0, 1.0, 1.0), (0.0, 0.0, 0.0, 1.0), (0.0, 0.0, 0.0)), {}
    for i in range(n):
        (bs, bq, bt), (cs, cq, ct) = bind.get(i, I), clip_sqts.get(i, I)
        out[i] = (tuple(a * b for a, b in zip(bs, cs)), qmul(bq, cq), tuple(a + b for a, b in zip(bt, qrot(bq, ct))))
    return out


def world(hier, local):
    """Forward kinematics, positions only (scale ignored: only leaf lip/cheek bones carry scale, see check)."""
    out = {}
    def get(i):
        if i not in out:
            _, q, t = local.get(i, ((1, 1, 1), (0, 0, 0, 1), (0, 0, 0)))
            par = hier["parents"][i]
            if par < 0:
                out[i] = (q, t)
            else:
                pq, pt = get(par)
                out[i] = (qmul(pq, q), tuple(a + b for a, b in zip(pt, qrot(pq, t))))
        return out[i]
    for i in range(hier["n"]):
        get(i)
    return out


def load(src):
    """A bank (path or bytes) -> (data, blocks, hierarchy, clips by name, poses by name)."""
    d = bytes(src) if isinstance(src, (bytes, bytearray, memoryview)) else Path(src).read_bytes()
    bl = blocks(d)
    hier = hierarchy(d, next(b for b in bl if b["type"] == 4)["off"])
    # RETAIL 4.3a: no codec switch; nothing reads blk+8. Codec word 0 (NIS_Jason_Dill_ProTutorial_S02_01, its clip and
    # pose, all f32 rotations) decodes like 1. Codec 3 (animationDB / CACAnimationDB) is another layout: UNTRACED, skipped.
    clips = {b["name"]: clip(d, b["off"]) for b in bl if b["type"] == 2 and b["codec"] in (0, 1)}
    poses = {b["name"]: pose(d, b["off"]) for b in bl if b["type"] == 3 and b["codec"] in (0, 1)}
    return d, bl, hier, clips, poses


def check(src):
    """The run 3 checks on one skater-rig bank (path or bytes): rig layout, part sizes, unit rotations, scale only on
    lip/cheek bones, bind o clip on the ground, the root header = the trajectory's last frame in frame-0 space."""
    d, bl, hier, clips, poses = load(src)
    assert hier["n"] == 155 and len(hier["parts"]) == 19
    assert [p["count"] for p in hier["parts"]] == [1, 5, 3, 8, 8, 1, 6, 17, 17, 4, 8, 2, 2, 15, 1, 1, 27, 21, 8]
    assert sum(p["count"] for p in hier["parts"]) == 155
    for anim in list(clips.values()) + list(poses.values()):
        for p in anim["parts"]:
            hp = hier["parts"][p["skel_part"]]
            assert hp["clip"] == 1 or anim in poses.values(), "a clip part drives a procedural hierarchy part"
            if p["frame_size"]:
                assert p["bones"] == hp["count"]
                assert frame_size(p, hp["sqt"]) == p["frame_size"], hex(p["w6"])
                assert p["data"] + anim["frames"] * p["frame_size"] <= len(d)
        for f in range(0, anim["frames"], max(1, anim["frames"] // 40)):
            for i, (s, q, t) in sample(d, hier, anim, f).items():
                assert abs(sum(c * c for c in q) - 1.0) < 1e-5
                assert s == (1.0, 1.0, 1.0) or i >= 78 and 0.7 < min(s) <= max(s) < 1.1  # scale: lip/cheek only
    name, c = next(iter(clips.items()))
    bind, f0 = sample(d, hier, next(iter(poses.values())), 0), sample(d, hier, c, 0)
    w = world(hier, add_bind(bind, f0, hier["n"]))
    wheel = hier["names"].index("RIGHT_WHEELFRONT")
    tq, tt = f0.get(0, ((), (0.0, 0.0, 0.0, 1.0), (0.0, 0.0, 0.0)))[1:]
    assert 0.0 < qrot(tq, w[wheel][1])[1] < 0.8, "bind o clip puts the wheels on or near the ground"
    if 0 in f0:  # header +0/+0x10 = trajectory end minus start, in the start frame (0x8225f988 loop)
        q0, t0 = f0[0][1:]; q1, t1 = sample(d, hier, c, c["frames"] - 1)[0][1:]
        inv = (-q0[0], -q0[1], -q0[2], q0[3])
        dt, dq = qrot(inv, [a - b for a, b in zip(t1, t0)]), qmul(inv, q1)
        assert max(abs(a - b) for a, b in zip(dt + dq, c["root"][:3] + c["root"][4:])) < 1e-3


def write_clip(out, name, frames):
    """frames[f][bone] = (scale xyz, rotation xyzw, translation xyz) -> out/<name>.bin, frames x bones x (rotation,
    translation, scale): 10 little-endian f32."""
    out.mkdir(parents=True, exist_ok=True)
    path = out/f"{name}.bin"
    path.write_bytes(b"".join(struct.pack("<10f", *q, *t, *s) for frame in frames for s, q, t in frame))
    return path


def export_clip(src, clip_name, out):
    """One clip of a bank (path or bytes) -> out/<clip>.bin: every frame's local (rotation, translation, scale) per
    hierarchy bone = the bank's reference pose o the clip (add_bind; the lip / cheek leaves carry scale, research 4.3a). Bones of the parts a clip never
    keys (record clip flag 0: twists, hair, hem) hold the pose and are `animated: false`."""
    d, _, hier, clips, poses = load(src)
    (ref,) = poses.values()  # one reference pose per bank (the rig's own: TUTORIAL_SKATER, IP_SHINGO, ...)
    c, bind, n = clips[clip_name], sample(d, hier, ref, 0), hier["n"]
    frames = []
    for f in range(c["frames"]):
        local = add_bind(bind, sample(d, hier, c, f), n)
        frames.append([local[i] for i in range(n)])
    path = write_clip(Path(out), clip_name, frames)
    animated = [False] * n
    for p in hier["parts"]:
        for i in range(p["sqt"], p["sqt"] + p["count"]):
            animated[i] = p["clip"] == 1
    return {"clip": clip_name, "file": f"{path.parent.name}/{path.name}", "fps": c["fps"], "frames": c["frames"],
            "bones": hier["names"], "parents": hier["parents"], "animated": animated}


if __name__ == "__main__":
    d, bl, hier, clips, poses = load(sys.argv[1])
    for b in bl:
        print("block 0x%x size 0x%x type %d codec 0x%x %s" % (b["off"], b["size"], b["type"], b["codec"], b["name"]))
    for i, p in enumerate(hier["parts"]):
        print("part %2d %-22s count %2d sqt %3d clip %d id %d" % (i, p["name"], p["count"], p["sqt"], p["clip"], p["id"]))
    for name, a in list(clips.items()) + list(poses.items()):
        print(name, "fps", a.get("fps"), "frames", a["frames"], "root", a.get("root"), "flags %#x" % a.get("flags", 0))
        for p in a["parts"]:
            print("  skel part %2d bones %2d fsz %4d rotf32 %d ts16 %d s %x r %x t %x" % (
                p["skel_part"], p["bones"], p["frame_size"], p["rot_f32"], p["trans_s16"], p["scale"], p["rot"], p["trans"]))
