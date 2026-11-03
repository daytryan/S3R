"""build_audio_image's retail key: read from Xenia's source text and checked against its SHA-256, never kept in the repo.
No network or discs needed. py -3 tools/test_xex_key.py"""
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path[:0] = [str(Path(__file__).resolve().parents[1]), str(Path(__file__).resolve().parent)]
import build_audio_image as b  # noqa: E402


class XexKey(unittest.TestCase):
    def setUp(self):
        b.retail_key.cache_clear()

    def test_the_key_is_read_from_xenias_array(self):
        text = "static const uint8_t xe_xex2_devkit_key[16] = {0x00};\n" \
               "static constexpr uint8_t xe_xex2_retail_key[16] = {\n    0x01, 0xAB, 0xff,\n    0x10};\n"
        self.assertEqual(b.key_from_source(text), bytes([0x01, 0xAB, 0xFF, 0x10]))
        self.assertEqual(b.key_from_source("no key here"), b"")

    def test_a_file_without_the_right_key_is_refused(self):
        with tempfile.TemporaryDirectory() as t:
            fake = Path(t, "xex_module.cc")
            fake.write_text("uint8_t xe_xex2_retail_key[16] = {" + ", ".join(["0x00"] * 16) + "};", encoding="utf-8")
            with mock.patch.dict(os.environ, {"SKATE_XEX_KEY_SOURCE": str(fake)}):
                with self.assertRaisesRegex(ValueError, "does not hold the retail XEX key"):
                    b.retail_key()

    def test_the_key_is_not_in_the_source(self):
        text = Path(b.__file__).read_text(encoding="utf-8")
        self.assertNotIn("RETAIL_KEY = bytes", text)
        self.assertNotRegex(text, r'fromhex\("[0-9A-Fa-f]{32}"\)')


if __name__ == "__main__":
    unittest.main()
