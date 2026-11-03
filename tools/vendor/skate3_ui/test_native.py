"""xenos_native.rs / refpack.dll against the Python they replace: random textures and streams, byte for byte.
py -3 tools/vendor/skate3_ui/test_native.py  (skips when the DLLs are not built: scripts/Build-Native.ps1)"""
import random
import sys
import unittest
from pathlib import Path

sys.path[:0] = [str(Path(__file__).resolve().parents[2]), str(Path(__file__).resolve().parents[3])]
from vendor.skate3_ui import refpack, xenos  # noqa: E402
from vendor.skate3_ui.binary import FormatError  # noqa: E402


def python(call, *args):
    saved = xenos._native, refpack._native
    xenos._native = refpack._native = None
    try:
        return call(*args)
    except FormatError:
        return FormatError
    finally:
        xenos._native, refpack._native = saved


def native(call, *args):
    try:
        return call(*args)
    except FormatError:
        return FormatError


class Native(unittest.TestCase):
    @unittest.skipUnless(xenos._native, "xenos.dll not built")
    def test_xenos(self):
        rng = random.Random(7)
        for case in range(3000):
            fmt = rng.choice((6, 18, 19, 20, 7))
            w, h = rng.choice((1, 3, 4, 17, 64, 100, 128, 256)), rng.choice((1, 2, 4, 9, 32, 64, 130))
            tiled, pitch = rng.random() < 0.6, rng.choice((0, 0, w, w + 32, 512))
            size = rng.choice((w * h * 4, w * h, 0, 1 << 16, 300000))
            data = rng.randbytes(size)
            fetch = xenos.FetchConstant(w, h, fmt, "", 1, tiled, 0, pitch)
            self.assertEqual(native(xenos.decode_rgba, data, fetch), python(xenos.decode_rgba, data, fetch), (case, fetch))

    @unittest.skipUnless(refpack._native, "refpack.dll not built")
    def test_refpack(self):
        rng = random.Random(9)
        for case in range(3000):
            body, size = stream(rng)
            if rng.random() < 0.5:  # corrupt one byte, or cut the stream short
                at = rng.randrange(len(body))
                body = body[:at] + bytes([rng.randrange(256)]) + body[at + 1:] if rng.random() < 0.7 else body[:at]
            for data, expected in ((b"\x10\xfb" + size.to_bytes(3, "big") + body, None), (body, size),
                                   (body, size + rng.choice((-1, 1))), (body, None)):
                self.assertEqual(native(refpack.decompress, data, expected), python(refpack.decompress, data, expected), case)


def stream(rng):
    """A valid RefPack body (every command kind, overlapping back-references) and its output size."""
    out, written = bytearray(), 0
    for _ in range(rng.randrange(1, 40)):
        kind = rng.randrange(4) if written else 3
        lit = rng.randrange(4)
        if kind == 0:
            d, n = rng.randrange(1, min(written + lit, 1024) + 1), rng.randrange(3, 11)
            out += bytes([((d - 1) >> 3) & 0x60 | (n - 3) << 2 | lit, (d - 1) & 0xFF]) + rng.randbytes(lit)
        elif kind == 1:
            d, n = rng.randrange(1, min(written + lit, 16384) + 1), rng.randrange(4, 68)
            out += bytes([0x80 | (n - 4), lit << 6 | (d - 1) >> 8, (d - 1) & 0xFF]) + rng.randbytes(lit)
        elif kind == 2:
            d, n = rng.randrange(1, written + lit + 1), rng.randrange(5, 1029)
            out += bytes([0xC0 | ((d - 1) >> 12) & 0x10 | ((n - 5) >> 6) & 0x0C | lit, ((d - 1) >> 8) & 0xFF,
                          (d - 1) & 0xFF, (n - 5) & 0xFF]) + rng.randbytes(lit)
        else:
            lit, n = rng.randrange(1, 29) * 4, 0
            out += bytes([0xE0 | (lit - 4) >> 2]) + rng.randbytes(lit)
        written += lit + n
    lit = rng.randrange(4)
    return bytes(out) + bytes([0xFC | lit]) + rng.randbytes(lit), written + lit


if __name__ == "__main__":
    unittest.main()
