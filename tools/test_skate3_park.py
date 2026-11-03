"""py -3 -m unittest tools.test_skate3_park (from the repo root). The stock-park tests need SKATE3_ASSETS, the install's
assets folder (read only); the rest build small parks in memory."""
import os
import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import skate3_park as p  # noqa: E402

STOCK = ["Downtown_Empty", "Downtown_Fullpark", "Industrial_Empty", "Industrial_Fullpark", "MegaPark_Empty",
         "MegaPark_Fullpark", "Starter_Fullpark", "Startpark_Empty", "startpark_progression", "startpark_tutorial1",
         "startpark_tutorial2", "startpark_tutorialsignup"]


def park(version=12, pieces=(), terrain_version=2, dmo_version=1, teleporters=4, tail=b""):
    """A file laid out as the writer 0x825129d0 lays it out."""
    t = struct.pack(">II", terrain_version, len(pieces))
    for pos, quat, pid in pieces:
        t += struct.pack(">3f4fQ", *pos, *quat, pid) + (struct.pack(">3f", 1, 1, 1) if terrain_version >= 2 else b"")
    d = struct.pack(">II", dmo_version, 0)
    tm = struct.pack(">I", 0)
    dm = struct.pack(">I", 0)
    out = struct.pack(">I4IQ", version, len(t), len(d), len(tm), len(dm), 0x830E6DE0597149E5) + t + d + tm + dm
    if version >= 2:
        out += struct.pack(">16f", 0, 0, -1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 5, 0, 6, 1)
    if version >= 5:
        out += struct.pack(">II", 1, 0xB28ED8C9) + (struct.pack(">I", 7) if version >= 7 else b"") + struct.pack(">I", 0)
    if version >= 6:
        out += struct.pack(">I", 0) + b"park\0".ljust(0x100, b"\0")
    if version >= 12:
        out += struct.pack(">Q", 0xFFFFFFFFFFFFFFFF)
    elif version >= 9:
        out += struct.pack(">I", 0)
    if version >= 11:
        out += struct.pack(">I", 2)
    if version >= 8:
        tp = struct.pack(">I", teleporters) + (b"\0" * 98) * teleporters
        out += struct.pack(">I", len(tp)) + tp
    return out + tail


class Synthetic(unittest.TestCase):
    def test_round_trip_of_the_writer_layout(self):
        q = (0, 0.70710678, 0, 0.70710678)
        r = p.read_park(park(pieces=[((1, 2, 3), q, 0x2C70170300030006)]))
        piece = r["terrain"]["pieces"][0]
        self.assertEqual(piece["id"], "2C70170300030006")
        m = piece["matrix"]
        for got, want in zip(m[0][:3] + m[2][:3] + m[3][:3], (0, 0, -1, 1, 0, 0, 1, 2, 3)):
            self.assertAlmostEqual(got, want, places=5)
        self.assertEqual((r["unread_tail"], r["unlock_8c"], r["unlock_8c_from_world"]), (0, 7, False))
        self.assertEqual(r["teleporters"]["count"], 4)

    def test_every_version_gate_reads_to_the_end(self):
        for v in range(2, 13):
            with self.subTest(version=v):
                self.assertEqual(p.read_park(park(version=v))["unread_tail"], 0)

    def test_terrain_v1_has_no_colour(self):
        r = p.read_park(park(pieces=[((0, 0, 0), (0, 0, 0, 1), 5)], terrain_version=1))
        self.assertIsNone(r["terrain"]["pieces"][0]["colour"])

    def test_truncated_refuses(self):
        whole = park(pieces=[((0, 0, 0), (0, 0, 0, 1), 5)])
        for cut in (3, 30, 60, len(whole) - 1):
            with self.subTest(cut=cut), self.assertRaises(p.Refused) as e:
                p.read_park(whole[:cut])
            self.assertEqual(e.exception.reason, "truncated")

    def test_unknown_versions_refuse(self):
        for kw in ({"version": 13}, {"version": 1}, {"terrain_version": 3}, {"dmo_version": 2}):
            with self.subTest(**kw), self.assertRaises(p.Refused) as e:
                p.read_park(park(**kw))
            self.assertEqual(e.exception.reason, "version")

    def test_more_than_four_teleporters_refuses(self):
        with self.assertRaises(p.Refused) as e:
            p.read_park(park(teleporters=5))
        self.assertEqual(e.exception.reason, "teleporters")

    def test_cli_writes_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            f = Path(tmp) / "x.park"
            f.write_bytes(park())
            self.assertEqual(p.main([str(f), "--out", tmp]), 0)
            self.assertTrue((Path(tmp) / "x.json").exists())
            f.write_bytes(park()[:10])
            self.assertEqual(p.main([str(f), "--out", tmp]), 2)


@unittest.skipUnless(os.environ.get("SKATE3_ASSETS"), "set SKATE3_ASSETS to the install's assets folder (read only)")
class Stock(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = Path(os.environ["SKATE3_ASSETS"]) / "private/stock/data/skatepark"

    def test_every_stock_park_parses(self):
        names = sorted(f.stem for f in self.dir.glob("*.park"))
        self.assertEqual(names, sorted(STOCK))
        for name in names:
            with self.subTest(park=name):
                p.read_park((self.dir / f"{name}.park").read_bytes())

    def test_progression(self):
        r = p.read_park((self.dir / "startpark_progression.park").read_bytes())
        self.assertEqual((r["version"], r["world_key"], r["unread_tail"]), (12, "830E6DE0597149E5", 0))
        self.assertEqual(len(r["terrain"]["pieces"]), 251)
        self.assertEqual(len(r["terrain_models"]), 15)
        self.assertEqual(r["dmo"]["items"], [])
        pos = r["spawn"][3][:3]
        for got, want in zip(pos, (-112.540, 0, 11.246)):
            self.assertAlmostEqual(got, want, places=3)
        self.assertAlmostEqual(p.yaw_degrees(r["spawn"]), 90, places=3)
        self.assertEqual(r["terrain"]["unread"], 4016)  # stage2_park.md: bytes 0x2F34..0x3EE4 are writer junk
        self.assertEqual((r["teleporters"]["block_size"], r["teleporters"]["count"]), (0x20C, 4))

    def test_old_files_keep_their_unread_tail(self):
        for name, tail in (("Startpark_Empty", 512), ("startpark_tutorial1", 448)):
            with self.subTest(park=name):
                self.assertEqual(p.read_park((self.dir / f"{name}.park").read_bytes())["unread_tail"], tail)


if __name__ == "__main__":
    unittest.main()
