"""Decrypt and decompress the Skate 2 disc `default.xex` into its memory image (base 0x82000000).

Usage: py -3 tools/build_skate2_image.py <Skate 2 disc default.xex> <out .bin>

The output is the test-only oracle image for `crates/skate-core/src/physics/skate2` (env `SKATE2_IMAGE`).
It is built from the user's own disc at test time and is never committed or shipped.

Uses tools/build_audio_image.py's XEX code. Its LZX decoder once copied stale window bytes for a wrapping
match with offset < length (on Skate 2: 2,031 bytes of .rdata, the tail of .data and all of .bss).
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import build_audio_image as b  # noqa: E402


def build(xex_path):
    raw = Path(xex_path).read_bytes()
    h = b.Header(raw)
    encryption, compression, window, first, digest = h.file_format()
    if (encryption, compression) != (1, 2):
        raise ValueError(f"unexpected format {encryption}/{compression}")
    body = b.aes(h.session_key(), raw[h.pe_offset:], cbc=True)
    return b.lzx(b.deblock(body, first, digest), h.image_size(), b.window_bits(window))


if __name__ == "__main__":
    image = build(sys.argv[1])
    Path(sys.argv[2]).write_bytes(image)
    print(f"{len(image):#x} bytes -> {sys.argv[2]}")
