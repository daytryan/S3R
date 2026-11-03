"""Regression fixtures for padded Xbox tiles and BC colour palettes."""
import ctypes
import importlib.util
from pathlib import Path
import struct
import sys
import unittest
from unittest.mock import patch

UTT = Path(__file__).resolve().parents[1] / 'vendor/utt'
sys.path.insert(0, str(UTT))
import rx2_fast

# Exercise the standalone fallback as well as the NumPy implementation.
spec = importlib.util.spec_from_file_location('rx2_scalar_test', UTT / 'rx2_parser.py')
scalar = importlib.util.module_from_spec(spec)
with patch.dict(sys.modules, {'rx2_fast': None}):
    spec.loader.exec_module(scalar)


class TextureDecodeTests(unittest.TestCase):
    def test_padding_cannot_overwrite_visible_rows(self):
        for width, pitch in ((w, p) for w in (4, 8, 16, 32) for p in (2, 4, 8, 16)):
            with self.subTest(width=width, pitch=pitch):
                units = max(32*32, 4096//pitch)
                raw = bytearray(units*pitch)
                for offset in range(units):
                    x = scalar._x360_tiled_x(offset, width, pitch)
                    y = scalar._x360_tiled_y(offset, width, pitch)
                    if x < width and y < 32:
                        raw[offset*pitch:offset*pitch+pitch] = bytes([1+y])*pitch
                expected = b''.join(bytes([1+y])*(width*pitch) for y in range(32))
                for decoder in (scalar, rx2_fast):
                    self.assertEqual(decoder._untile360(raw, width, pitch)[:len(expected)], expected)

    def test_bc1_keeps_transparent_palette(self):
        colour = struct.pack('>HH', 0x001f, 0xf800) + b'\xff'*4
        self.assertEqual(scalar._dxt1_block(colour, True), bytes(64))
        self.assertEqual(rx2_fast._decode(colour, 4, 4, 1, False), bytes(64))

    def test_bc2_bc3_use_four_colours_with_reversed_endpoints(self):
        colour = struct.pack('>HH', 0x001f, 0xf800) + b'\xff'*4
        for kind, alpha, block_decoder in ((3, b'\xaa'*8, scalar._dxt3_block),
                                           (5, b'\x00\xaa'+bytes(6), scalar._dxt5_block)):
            expected = bytes((170, 0, 85, 170))*16
            self.assertEqual(block_decoder(alpha+colour), expected)
            self.assertEqual(rx2_fast._decode(alpha+colour, 4, 4, kind, False), expected)

    @unittest.skipUnless(rx2_fast._native, 'rx2.dll not built (scripts/Build-Native.ps1)')
    def test_the_native_decoder_matches_numpy_byte_for_byte(self):
        import numpy as np
        rng, native = np.random.default_rng(1), rx2_fast._native
        try:
            for kind in (1, 2, 3, 5):
                for w, h in ((1, 1), (3, 7), (32, 32), (100, 60), (5, 300), (256, 64)):
                    for tiled in (True, False):
                        blocks = ((w+3)//4)*((h+3)//4)*(8 if kind == 1 else 16)
                        for size in (blocks, blocks//2, blocks+37):  # whole, truncated, trailing bytes
                            data = rng.integers(0, 256, size, dtype=np.uint8).tobytes()
                            rx2_fast._native = native
                            fast = rx2_fast._decode(data, w, h, kind, tiled)
                            rx2_fast._native = None
                            self.assertEqual(fast, rx2_fast._decode(data, w, h, kind, tiled), (kind, w, h, tiled, size))
        finally:
            rx2_fast._native = native

    def test_the_native_lightmap_filter_matches_numpy(self):
        import numpy as np
        from tools import native
        lib = native.load('lightmap', Path(__file__).parents[1]/'pipeline/converter/convert_ps3.py')
        if not lib:
            self.skipTest('lightmap.dll not built (scripts/Build-Native.ps1)')
        rng = np.random.default_rng(2)
        for w, h, f in ((4, 6, 2), (7, 9, 3), (64, 32, 2), (101, 77, 4)):
            px = rng.integers(0, 256, w*h*4, dtype=np.uint8)
            w2, h2 = w//f, h//f
            out = np.empty(w2*h2*4, np.uint8)
            self.assertEqual(lib.skate_box_downscale(ctypes.c_void_p(px.ctypes.data), ctypes.c_size_t(w), ctypes.c_size_t(h),
                                                     ctypes.c_size_t(f), ctypes.c_void_p(out.ctypes.data)), 0)
            expected = px.reshape(h, w, 4).astype(np.uint16)[:h2*f, :w2*f].reshape(h2, f, w2, f, 4).mean((1, 3))
            self.assertEqual(out.tobytes(), np.round(expected).astype(np.uint8).tobytes(), (w, h, f))
