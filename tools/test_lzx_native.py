"""lzx_native.rs against build_audio_image.lzx: random streams from a small encoder (verbatim, aligned and uncompressed
blocks, matches that overlap, wrap the window and reach into a delta reference), byte for byte. No discs needed.
py -3 tools/test_lzx_native.py  (skips when lzx.dll is not built: scripts/Build-Native.ps1)"""
import random
import struct
import sys
import unittest
from pathlib import Path

sys.path[:0] = [str(Path(__file__).resolve().parents[1]), str(Path(__file__).resolve().parent)]
import build_audio_image as b  # noqa: E402

MAIN_BITS, LENGTH_BITS, PRE_BITS = 10, 8, 5  # every code the same length, so a symbol's code is the symbol


class Writer:
    def __init__(self):
        self.out, self.v, self.n = bytearray(), 0, 0

    def bits(self, value, k):
        self.v, self.n = self.v << k | value, self.n + k
        while self.n >= 16:
            self.n -= 16
            self.out += struct.pack('<H', self.v >> self.n)
            self.v &= (1 << self.n) - 1

    def align(self):
        if self.n:
            self.bits(0, 16 - self.n)


def lengths(w, prev, length):
    for _ in range(20):
        w.bits(PRE_BITS, 4)
    for x in range(len(prev)):
        w.bits((prev[x] - length) % 17, PRE_BITS)
        prev[x] = length


def stream(rng, out_len, window_bits, ref_len):
    size, slots = 1 << window_bits, {15: 30, 16: 32, 17: 34}[window_bits]
    main_prev, length_prev = [0] * (256 + slots * 8), [0] * 249
    w, pos, r = Writer(), 0, [1, 1, 1]
    w.bits(0, 1)  # no E8 translation
    while pos < out_len:
        kind = rng.choice((1, 2, 3))
        n = min(rng.randint(1, 50000), out_len - pos)
        w.bits(kind, 3)
        w.bits(n >> 8, 16)
        w.bits(n & 0xFF, 8)
        if kind == 3:
            w.bits(0, 16) if not w.n else w.align()  # lzxd skips a whole word when the header ends on a boundary
            r = [rng.randint(1, size) for _ in range(3)]
            w.out += struct.pack('<III', *r) + rng.randbytes(n) + bytes(n & 1)
            pos += n
            continue
        if kind == 2:
            for _ in range(8):
                w.bits(3, 3)
        lengths(w, main_prev[:256], MAIN_BITS)
        lengths(w, main_prev[256:], MAIN_BITS)
        main_prev[:] = [MAIN_BITS] * len(main_prev)
        lengths(w, length_prev, LENGTH_BITS)
        end = pos + n
        while pos < end:
            room = min(end, (pos // 0x8000 + 1) * 0x8000) - pos  # a match never crosses a block or frame end
            if room < 2 or rng.random() < 0.3:
                w.bits(rng.randrange(256), MAIN_BITS)
                pos += 1
            else:
                length = rng.randint(2, min(room, 257))
                slot = rng.choice((0, 1, 2, 3, 4, 4))
                if slot == 4:
                    offset = rng.choice((rng.randint(2, 40), rng.randint(2, size - 3)))  # slots reach size - 3
                    slot = max(s for s in range(4, slots) if b.BASE[s] <= offset + 2)
                    v, extra = offset + 2 - b.BASE[slot], b.EXTRA[slot]
                    r = [offset, r[0], r[1]]
                elif slot == 1:
                    r = [r[1], r[0], r[2]]
                elif slot == 2:
                    r = [r[2], r[1], r[0]]
                elif slot == 3:
                    r = [1, r[0], r[1]]
                head = min(length - 2, 7)
                w.bits(256 + slot * 8 + head, MAIN_BITS)
                if head == 7:
                    w.bits(length - 9, LENGTH_BITS)
                if slot >= 4 and kind == 2 and extra >= 3:
                    w.bits(v >> 3, extra - 3)
                    w.bits(v & 7, 3)
                elif slot >= 4:
                    w.bits(v, extra)
                pos += length
            if pos % 0x8000 == 0:
                w.align()
    w.align()
    return bytes(w.out)


class Lzx(unittest.TestCase):
    @unittest.skipUnless(b.LZX, "lzx.dll not built")
    def test_same_bytes_as_python(self):
        rng = random.Random(5)
        for case in range(40):
            window_bits = rng.choice((15, 16, 17))
            out_len = rng.choice((1, 0x8000, rng.randint(2, 200000)))
            reference = rng.randbytes(rng.choice((0, 100, 1 << window_bits)))
            data = stream(rng, out_len, window_bits, len(reference))
            native = b.lzx(data, out_len, window_bits, reference)
            saved, b.LZX = b.LZX, None
            try:
                self.assertEqual(native, b.lzx(data, out_len, window_bits, reference), case)
            finally:
                b.LZX = saved

    @unittest.skipUnless(b.LZX, "lzx.dll not built")
    def test_malformed_input_raises_the_pythons_error(self):
        with self.assertRaisesRegex(ValueError, "bad LZX block type 0"):
            b.lzx(bytes(64), 100, 15)


if __name__ == "__main__":
    unittest.main()
