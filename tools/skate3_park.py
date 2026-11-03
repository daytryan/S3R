"""Read a Skate 3 skate park layout (`.park`) exactly as TU3 reads it, and write it as JSON.

py -3 tools/skate3_park.py PARK.park --out DIR

Sources (TU3 addresses, base 0x82000000; docs/superpowers/specs/skate3-story-mode-reference/stage2_park.md Q1):
- File reader 0x82512c00 (called by 0x82514318 with a stream {buf, size, pos, fail u8}; read helper 0x828dfb28).
  Writer 0x825129d0 writes version 12 (0x822440cc).
- Terrain block parser 0x82868318 (highest version 2 at [0x82ffed20]); DMO block parser 0x82884600 (version 1 at
  [0x82fdfe84]); model lists read by 0x8287f9a8 (from 0x8287f5a0); teleporters read by load step 5 at 0x828a2474
  with 0x8286d410 per slot (4 slots, matrix helper 0x828e1af0 reads 4 rows of 3 f32).

Output DIR/<park stem>.json:
  {"version", "world_key", "block_sizes": [terrain, dmo, terrain models, dmo models],
   "terrain": {"version", "pieces": [{"id", "pos", "quat", "matrix", "colour"}], "unread"},
   "dmo": {"version", "items": [{"id", "pos", "quat", "matrix", "extra"}], "unread"},
   "terrain_models": [id], "dmo_models": [id], "spawn": matrix or null, "set_70": [u32], "unlock_8c": u32 or null,
   "unlock_8c_from_world": bool, "strings_90": [str], "mode_a0", "name", "owner_1a8", "kind_1b0",
   "teleporters": {"block_size", "count", "slots": [{"a": matrix, "a_flag", "b": matrix, "b_flag"}], "unread"},
   "read_end", "unread_tail"}
Ids are 16 hex digits. A matrix is 4 rows of 4 f32, row-vector convention (world = local * M): rows 0-2 are the
rotation transposed, row 3 the position (stage2_park.md Q1 "Terrain block"). The w lanes the file does not carry
are written 0 (1 for row 3). "unread" counts the bytes of a block retail never reads (the writer leaves its
count*64+8 / count*48+8 / 0x20c allocations partly unwritten, 0x82867f48 / 0x82884f18 / 0x825122f4).

Refusals (Refused, exit 2): retail keeps going on all of these with a sticky fail flag or a skipped block; this
tool refuses so nothing is silently dropped.
- "truncated": a read past the end of the file or of a block (0x828dfb28 sets stream+0xc and reads nothing).
- "version": a file version outside 2..12 (the writer's 12 is the newest; v1 has no spawn, none exists on disc),
  a terrain block above 2 (0x82868394 reads nothing), a DMO block other than 1 (0x82884664 reads nothing).
- "teleporters": a count above the 4 slots step 5 reads (0x828a2504 stops at slot 4).
"""
import argparse
import json
import math
import struct
import sys
from pathlib import Path

NEWEST = 12  # 0x822440cc, the writer's version
TERRAIN_NEWEST = 2  # [0x82ffed20]
DMO_VERSION = 1  # [0x82fdfe84]
TELEPORTER_SLOTS = 4  # 0x828a24a4: 4 slots of 0x150 at *(0x830cfd94)+0x11c


class Refused(Exception):
    """The read stops: `reason` is truncated, version or teleporters."""

    def __init__(self, reason, detail):
        super().__init__(f"{reason}: {detail}")
        self.reason = reason


class Stream:
    """0x828dfb28: copy n bytes or fail. Retail sets a sticky flag and reads nothing; this refuses."""

    def __init__(self, data, what):
        self.data, self.pos, self.what = data, 0, what

    def take(self, n):
        if len(self.data) - self.pos < n:
            raise Refused("truncated", f"{self.what}: {n} bytes wanted at {self.pos:#x}, {len(self.data) - self.pos} left")
        out = self.data[self.pos:self.pos + n]
        self.pos += n
        return out

    def u8(self):
        return self.take(1)[0]

    def u32(self):
        return struct.unpack(">I", self.take(4))[0]

    def u64(self):
        return struct.unpack(">Q", self.take(8))[0]

    def f32(self, n):
        return list(struct.unpack(">%df" % n, self.take(4 * n)))


def hex64(v):
    return "%016X" % v


def quat_matrix(pos, quat):
    """Rows of R transposed then pos: the matrix 0x82868318 builds from a record (sqrt2 constant 0x82139a00)."""
    x, y, z, w = quat
    r = [[1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
         [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
         [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)]]
    return [[r[0][i], r[1][i], r[2][i], 0.0] for i in range(3)] + [list(pos) + [1.0]]


def terrain(block):
    """0x82868318: u32 version, u32 count, records of pos f32x3, quat f32x4, u64 id, colour f32x3 (v>=2)."""
    s = Stream(block, "terrain block")
    version, count = s.u32(), s.u32()
    if version > TERRAIN_NEWEST:
        raise Refused("version", f"terrain block version {version} (retail reads nothing above {TERRAIN_NEWEST})")
    pieces = []
    for _ in range(count):
        pos, quat, pid = s.f32(3), s.f32(4), s.u64()
        colour = s.f32(3) if version >= 2 else None  # 0x82868468; v1 takes [0x830bd5e0]
        # 0x82868604: id [0x830bbdd8] (0) is not added; kept here with the flag so nothing is dropped unseen
        pieces.append({"id": hex64(pid), "pos": pos, "quat": quat, "matrix": quat_matrix(pos, quat), "colour": colour,
                       "skipped_by_retail": pid == 0})
    return {"version": version, "pieces": pieces, "unread": len(block) - s.pos}


def dmos(block):
    """0x82884600: u32 version, u32 count, records of pos f32x3, quat f32x4, u64 id, u32 extra."""
    s = Stream(block, "DMO block")
    version, count = s.u32(), s.u32()
    if version != DMO_VERSION:
        raise Refused("version", f"DMO block version {version} (retail reads only {DMO_VERSION})")
    items = []
    for _ in range(count):
        pos, quat, did, extra = s.f32(3), s.f32(4), s.u64(), s.u32()
        items.append({"id": hex64(did), "pos": pos, "quat": quat, "matrix": quat_matrix(pos, quat), "extra": extra,
                      "skipped_by_retail": did == 0})  # 0x828846d0
    # shortcut: the trim at 0x82884738 (0x82743fc0 true: drop items past 0x1d or with 0x828844a0 cost > 0x2710) is not
    # applied, its condition is untraced (stage2_park.md open questions); trace it before trusting DMO counts above 29.
    return {"version": version, "items": items, "unread": len(block) - s.pos}


def model_list(block, what):
    """0x8287f9a8: u32 count, count x u64 piece id."""
    s = Stream(block, what)
    ids = [hex64(s.u64()) for _ in range(s.u32())]
    return ids, len(block) - s.pos


def teleporters(block):
    """Load step 5 (0x828a2474): u32 count, then up to 4 slots read by 0x8286d410: matrix, u8, matrix, u8."""
    s = Stream(block, "teleporter block")
    count = s.u32()
    if count > TELEPORTER_SLOTS:
        raise Refused("teleporters", f"count {count}, retail reads {TELEPORTER_SLOTS} slots")
    slots = []
    for _ in range(count):
        a = [s.f32(3) for _ in range(4)]  # 0x828e1af0: 4 rows of 12 bytes
        a_flag = s.u8()
        b = [s.f32(3) for _ in range(4)]
        b_flag = s.u8()
        slots.append({"a": [r + [0.0] for r in a[:3]] + [a[3] + [1.0]], "a_flag": a_flag,
                      "b": [r + [0.0] for r in b[:3]] + [b[3] + [1.0]], "b_flag": b_flag})
    return {"block_size": len(block), "count": count, "slots": slots, "unread": len(block) - s.pos}


def read_park(data):
    """0x82512c00 field by field, with each version gate where TU3 has it."""
    s = Stream(data, "file")
    version = s.u32()
    if not 2 <= version <= NEWEST:
        raise Refused("version", f"file version {version} (TU3 writes {NEWEST}; this reader takes 2..{NEWEST})")
    sizes = [s.u32() for _ in range(4)]  # terrain, DMO, terrain model list, DMO model list
    out = {"version": version, "world_key": hex64(s.u64()), "block_sizes": sizes}
    blocks = [s.take(n) for n in sizes]  # each is read only when its size is non-zero (0x82512cc0..0x82512db4)
    out["terrain"] = terrain(blocks[0]) if sizes[0] else None
    out["dmo"] = dmos(blocks[1]) if sizes[1] else None
    out["terrain_models"], out["terrain_models_unread"] = model_list(blocks[2], "terrain model list") if sizes[2] else ([], 0)
    out["dmo_models"], out["dmo_models_unread"] = model_list(blocks[3], "DMO model list") if sizes[3] else ([], 0)
    out["spawn"] = None
    if version >= 2:  # 0x82512dbc
        rows = [s.f32(4) for _ in range(4)]
        out["spawn"] = rows
    out["set_70"], out["unlock_8c"], out["strings_90"] = [], None, []
    if version >= 5:  # 0x82512e18
        out["set_70"] = [s.u32() for _ in range(s.u32())]
        if version >= 7:  # 0x82512e74
            out["unlock_8c"] = s.u32()
        out["strings_90"] = [s.take(s.u32()).decode("latin-1") for _ in range(s.u32())]
    # 0x82513064: below 10, data+0x8c is replaced by the low word of the world's Unlock field 3A3029D4DCA2E714
    out["unlock_8c_from_world"] = version < 10
    out["mode_a0"], out["name"] = None, None
    if version >= 6:  # 0x82513110
        out["mode_a0"] = s.u32()
        out["name"] = s.take(0x100).split(b"\0", 1)[0].decode("latin-1")
    out["owner_1a8"] = None
    if version >= 12:  # 0x825131ac
        out["owner_1a8"] = hex64(s.u64())
    elif version >= 9:  # 0x825131c8: a u32 is read and dropped, the field becomes -1
        s.u32()
        out["owner_1a8"] = "FFFFFFFFFFFFFFFF"
    out["kind_1b0"] = s.u32() if version >= 11 else None  # 0x825131dc
    out["teleporters"] = None
    if version >= 8:  # 0x82513234
        out["teleporters"] = teleporters(s.take(s.u32()))
    out["read_end"] = s.pos
    out["unread_tail"] = len(data) - s.pos  # 0x825132cc returns here; old v2/v3 files carry a tail it never reads
    return out


def yaw_degrees(matrix):
    """Yaw about +Y of a row-vector matrix (for summaries only)."""
    return math.degrees(math.atan2(-matrix[0][2], matrix[0][0]))


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("park", type=Path, help="a .park file (read only)")
    p.add_argument("--out", type=Path, required=True, help="folder that receives <stem>.json (keep it outside the repo)")
    a = p.parse_args(argv)
    try:
        park = read_park(a.park.read_bytes())
    except Refused as e:
        print(f"refused ({e})", file=sys.stderr)
        return 2
    a.out.mkdir(parents=True, exist_ok=True)
    dest = a.out / (a.park.stem + ".json")
    dest.write_text(json.dumps(park, indent=1), encoding="utf-8", newline="\n")
    t, d = park["terrain"], park["dmo"]
    print(f"wrote {dest}: version {park['version']}, world {park['world_key']}, "
          f"{len(t['pieces']) if t else 0} pieces, {len(d['items']) if d else 0} DMOs, "
          f"spawn {'none' if park['spawn'] is None else [round(v, 3) for v in park['spawn'][3][:3]]}, "
          f"unread tail {park['unread_tail']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
