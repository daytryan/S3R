"""Build the TU3 guest image the player-audio runtime reads (assets/private/stock/audio-runtime-image).

Usage: build_audio_image.py <disc default.xex> <title update default.xexp> <out dir>

The runtime reads rodata (class descriptors, constants, tables) from the game image as loaded at
0x82000000. That image only exists decrypted and decompressed, with Title Update 3 applied, so
this rebuilds it from the owner's own files: XEX2 AES decrypt, LZX decompress, then the xexp's
LZX-delta patches, the same steps Xenia's XexModule::ApplyPatch takes. The output is written in
the 2 MB g_XXXX.bin pieces (named by address >> 16) that the PR's gdb dump produced.
"""
import ctypes
import functools
import hashlib
import os
import re
import struct
import sys
from pathlib import Path

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

try:
    from tools import native
    LZX = native.load('lzx', __file__)  # lzx_native.rs: lzx() below in native code, the same bytes
except ImportError:  # run without the repo on sys.path
    LZX = None
if LZX:
    LZX.skate_lzx.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_char_p,
                              ctypes.c_size_t, ctypes.c_void_p]

RETAIL_KEY_SHA256 = "a0bd3f5f56adf2b0238a2702c0c0cd8548fb34b3dc19345943dc011010723fe4"
FILE_FORMAT, DELTA_PATCH, IMAGE_BASE = 0x3FF, 0x5FF, 0x10201


def key_from_source(text):
    """The retail XEX2 key as Xenia's xex_module.cc writes it (`xe_xex2_retail_key[16] = {0x.., ...}`), or b''."""
    m = re.search(r"xe_xex2_retail_key\[16\]\s*=\s*\{([^}]*)\}", text)
    return bytes(int(x, 16) for x in re.findall(r"0x([0-9A-Fa-f]{2})", m.group(1))) if m else b""


@functools.cache
def retail_key():
    """The key is never in this repo: it is read from Xenia's public source (fetch_tools 'xenia-key', the file's
    SHA-256 pinned), which the pipeline downloads with its other tools and names in SKATE_XEX_KEY_SOURCE. Run by
    hand, the file is downloaded into out/tools. Either way the key must match its own SHA-256 above."""
    source = os.environ.get("SKATE_XEX_KEY_SOURCE")
    if not source:
        sys.path.insert(0, str(Path(__file__).resolve().parents[1]))  # run by hand: the repo root, for tools.pipeline
        from tools.pipeline.fetch_tools import tool
        source = tool(Path(__file__).resolve().parents[1]/"out"/"tools", "xenia-key", lambda text: None)
    key = key_from_source(Path(source).read_text(encoding="utf-8"))
    if hashlib.sha256(key).hexdigest() != RETAIL_KEY_SHA256:
        raise ValueError(f"{source} does not hold the retail XEX key; delete it and build again")
    return key


def u32(b, o):
    return struct.unpack_from(">I", b, o)[0]


def aes(key, data, cbc):
    mode = modes.CBC(bytes(16)) if cbc else modes.ECB()
    d = Cipher(algorithms.AES(key), mode).decryptor()
    whole = len(data) & ~15
    return d.update(bytes(data[:whole])) + d.finalize() + bytes(data[whole:])


class Header:
    def __init__(self, raw):
        assert raw[:4] == b"XEX2", "not an XEX2 file"
        self.raw = raw
        self.pe_offset, self.security = u32(raw, 8), u32(raw, 16)
        self.opts = dict(struct.unpack_from(">II", raw, 24 + i * 8) for i in range(u32(raw, 20)))

    def image_size(self):
        return u32(self.raw, self.security + 4)

    def session_key(self):
        return aes(retail_key(), self.raw[self.security + 0x150:self.security + 0x160], cbc=False)

    def file_format(self):
        o = self.opts[FILE_FORMAT]
        encryption, compression = struct.unpack_from(">HH", self.raw, o + 4)
        window, first = u32(self.raw, o + 8), u32(self.raw, o + 12)
        return encryption, compression, window, first, self.raw[o + 16:o + 36]


def blocks(body, first, digest):
    """Walk the SHA-1-chained blocks; each starts with the next block's size and hash."""
    p, size = 0, first
    while size:
        block = body[p:p + size]
        if hashlib.sha1(block).digest() != digest:
            raise ValueError(f"block at {p:#x} fails its SHA-1 (wrong key?)")
        size, digest = u32(block, 0), block[4:24]
        yield block[24:]
        p += len(block)


def deblock(body, first, digest):
    """Normal compression: each block is u16-sized chunks of LZX data, a 0 size ends it."""
    out = bytearray()
    for block in blocks(body, first, digest):
        q = 0
        while n := struct.unpack_from(">H", block, q)[0]:
            out += block[q + 2:q + 2 + n]
            q += 2 + n
    return bytes(out)


# LZX (libmspack lzxd semantics, as Xenia calls it: no reset interval, not LZX DELTA framing)
EXTRA = [0] * 51
BASE = [0] * 51
for i in range(51):
    EXTRA[i] = min(max(i // 2 - 1, 0), 17)
for i in range(1, 51):
    BASE[i] = BASE[i - 1] + (1 << EXTRA[i - 1])


class Bits:
    def __init__(self, data):
        self.d, self.p, self.buf, self.n = data, 0, 0, 0

    def ensure(self, k):
        while self.n < k:
            lo = self.d[self.p] if self.p < len(self.d) else 0
            hi = self.d[self.p + 1] if self.p + 1 < len(self.d) else 0
            self.p += 2
            self.buf, self.n = (self.buf << 16) | (hi << 8) | lo, self.n + 16

    def read(self, k):
        if not k:
            return 0
        self.ensure(k)
        self.n -= k
        v = self.buf >> self.n
        self.buf &= (1 << self.n) - 1
        return v

    def symbol(self, table):
        self.ensure(16)
        sym, length = table[(self.buf >> (self.n - 16)) & 0xFFFF]
        if length == 0:
            raise ValueError("bad huffman code")
        self.n -= length
        self.buf &= (1 << self.n) - 1
        return sym


def huffman(lens):
    """Canonical MSB-first codes -> 16-bit lookup of (symbol, length); None if every length is 0."""
    table, code = [(0, 0)] * 65536, 0
    for length in range(1, 17):
        for sym, l in enumerate(lens):
            if l == length:
                span = 1 << (16 - length)
                table[code * span:(code + 1) * span] = [(sym, length)] * span
                code += 1
        code <<= 1
    return table if any(lens) else None


def read_lengths(bits, lens, first, last):
    pre = huffman([bits.read(4) for _ in range(20)])
    x = first
    while x < last:
        z = bits.symbol(pre)
        if z == 17:
            run, value = bits.read(4) + 4, 0
        elif z == 18:
            run, value = bits.read(5) + 20, 0
        elif z == 19:
            run = bits.read(1) + 4
            value = (lens[x] - bits.symbol(pre)) % 17
        else:
            run, value = 1, (lens[x] - z) % 17
        lens[x:x + run] = [value] * run
        x += run


def lzx(data, out_len, window_bits, reference=b""):
    if LZX:
        data, reference, out = bytes(data), bytes(reference), ctypes.create_string_buffer(out_len)
        if not LZX.skate_lzx(data, len(data), out_len, window_bits, reference, len(reference), out):
            return out.raw
        # anything it isn't sure of (malformed input) runs below, so errors and edge cases stay the Python's
    size = 1 << window_bits
    if len(reference) > size:
        raise ValueError("delta reference larger than the window")
    window = bytearray(size)
    window[size - len(reference):] = reference
    slots = {15: 30, 16: 32, 17: 34, 18: 36, 19: 38, 20: 42, 21: 50}[window_bits]
    # A run may overshoot the table end; like mspack's LENTABLE_SAFETY, pad instead of growing.
    main_count = 256 + slots * 8
    main_lens, length_lens, aligned = [0] * (main_count + 64), [0] * (249 + 64), None
    main = lengths = None
    bits, out = Bits(data), bytearray()
    r0 = r1 = r2 = 1
    posn = frame_posn = 0
    block_type = block_length = block_remaining = 0
    intel_size = (bits.read(16) << 16 | bits.read(16)) if bits.read(1) else 0
    while len(out) < out_len:
        frame_size = min(0x8000, out_len - len(out))
        todo = frame_posn + frame_size - posn
        while todo > 0:
            if block_remaining == 0:
                if block_type == 3 and block_length & 1:
                    bits.p += 1
                block_type = bits.read(3)
                block_remaining = block_length = bits.read(16) << 8 | bits.read(8)
                if block_type == 2:
                    aligned = huffman([bits.read(3) for _ in range(8)])
                if block_type in (1, 2):
                    read_lengths(bits, main_lens, 0, 256)
                    read_lengths(bits, main_lens, 256, main_count)
                    main = huffman(main_lens[:main_count])
                    read_lengths(bits, length_lens, 0, 249)
                    lengths = huffman(length_lens[:249])
                elif block_type == 3:
                    if bits.n == 0:
                        bits.ensure(16)
                    bits.n = bits.buf = 0
                    r0, r1, r2 = struct.unpack_from("<III", bits.d, bits.p)
                    bits.p += 12
                else:
                    raise ValueError(f"bad LZX block type {block_type}")
            run = min(block_remaining, todo)
            todo -= run
            block_remaining -= run
            if block_type == 3:
                window[posn:posn + run] = bits.d[bits.p:bits.p + run]
                bits.p += run
                posn += run
                continue
            while run > 0:
                element = bits.symbol(main)
                if element < 256:
                    window[posn] = element
                    posn += 1
                    run -= 1
                    continue
                element -= 256
                length = element & 7
                if length == 7:
                    length += bits.symbol(lengths)
                length += 2
                slot = element >> 3
                if slot == 0:
                    offset = r0
                elif slot == 1:
                    offset, r1 = r1, r0
                    r0 = offset
                elif slot == 2:
                    offset, r2 = r2, r0
                    r0 = offset
                elif slot == 3:
                    offset, r2, r1, r0 = 1, r1, r0, 1
                else:
                    extra = EXTRA[slot]
                    offset = BASE[slot] - 2
                    if block_type == 2 and extra >= 3:
                        offset += (bits.read(extra - 3) << 3) + bits.symbol(aligned)
                    elif block_type == 2 and extra == 0:
                        offset = 1
                    else:
                        offset += bits.read(extra)
                    r2, r1, r0 = r1, r0, offset
                if posn + length > size:
                    raise ValueError("LZX match runs past the window")
                src = posn - offset
                if src < 0:
                    src += size  # wraps into the reference data at the window's end
                # offset < length overlaps the match's own output (across the wrap too): byte by byte.
                if offset >= length and (src + length <= posn or src >= posn + length):
                    if src + length <= size:
                        window[posn:posn + length] = window[src:src + length]
                    else:
                        cut = size - src
                        window[posn:posn + cut] = window[src:]
                        window[posn + cut:posn + length] = window[:length - cut]
                else:
                    for k in range(length):
                        window[posn + k] = window[(src + k) % size]
                posn += length
                run -= length
            if run < 0:
                block_remaining += run
        if posn - frame_posn != frame_size:
            raise ValueError("LZX frame size mismatch")
        if bits.n > 0:
            bits.ensure(16)
        bits.read(bits.n & 15)
        frame = window[frame_posn:frame_posn + frame_size]
        if intel_size:
            raise ValueError("E8 translation is not expected in an Xbox 360 image")
        out += frame
        frame_posn += frame_size
        posn %= size
        frame_posn %= size
    return bytes(out)


def apply_delta(records, dest, window_bits):
    """xex2_delta_patch records: old, new (u32), uncompressed, compressed (u16), then data."""
    p = 0
    while p + 12 <= len(records):
        old, new, plain, packed = struct.unpack_from(">IIHH", records, p)
        if not (old or new or plain or packed):
            break
        if packed == 0:
            dest[new:new + plain] = bytes(plain)
        elif packed == 1:
            dest[new:new + plain] = dest[old:old + plain]
        else:
            dest[new:new + plain] = lzx(records[p + 12:p + 12 + packed], plain, window_bits,
                                        bytes(dest[old:old + plain]))
        p += 12 + (packed if packed > 1 else 0)


def window_bits(window):
    return window.bit_length() - 1


def build(xex_path, xexp_path):
    base_raw, patch_raw = Path(xex_path).read_bytes(), Path(xexp_path).read_bytes()
    base, patch = Header(base_raw), Header(patch_raw)
    encryption, compression, window, first, digest = base.file_format()
    if (encryption, compression) != (1, 2):
        raise ValueError(f"unexpected base format {encryption}/{compression}")
    body = aes(base.session_key(), base_raw[base.pe_offset:], cbc=True)
    image = lzx(deblock(body, first, digest), base.image_size(), window_bits(window))
    print(f"base image {len(image):#x} bytes")

    d = patch.opts[DELTA_PATCH]
    target_headers, src_off, src_size, dst_off, img_src_off, img_src_size, img_dst_off = \
        struct.unpack_from(">7I", patch_raw, d + 0x30)
    p_encryption, p_compression, p_window, p_first, p_digest = patch.file_format()
    if (p_encryption, p_compression) != (1, 3):
        raise ValueError(f"unexpected patch format {p_encryption}/{p_compression}")
    headers = bytearray(max(target_headers, base.pe_offset))
    headers[:base.pe_offset] = base_raw[:base.pe_offset]
    if src_off:
        headers[dst_off:dst_off + src_size] = headers[src_off:src_off + src_size]
    apply_delta(patch_raw[d + 0x4C:d + u32(patch_raw, d)], headers, window_bits(p_window))
    target = Header(bytes(headers))
    print(f"patched headers: image {target.image_size():#x} bytes at {target.opts[IMAGE_BASE]:#x}")

    patched = bytearray(target.image_size())
    patched[img_dst_off:img_dst_off + img_src_size] = image[img_src_off:img_src_off + img_src_size]
    # The xexp's own image key is wrapped with the patched (target) image's session key.
    key = aes(target.session_key(), patch_raw[patch.security + 0x150:patch.security + 0x160], cbc=False)
    for records in blocks(aes(key, patch_raw[patch.pe_offset:], cbc=True), p_first, p_digest):
        apply_delta(records, patched, window_bits(p_window))
    return target.opts[IMAGE_BASE], bytes(patched)


def main(xex_path, xexp_path, out):
    base, image = build(xex_path, xexp_path)
    # Known TU3 rodata (docs/player-audio-retail-drivers.md): m/s -> km/h and the pitch scale.
    for address, value in ((0x822F8628, 3.6), (0x822F889C, 4096.0)):
        got = struct.unpack_from(">f", image, address - base)[0]
        if abs(got - value) > 1e-5:
            raise SystemExit(f"{address:#x} holds {got}, not {value}: this is not the TU3 image")
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    chunk = 0x200000
    for start in range(0, len(image), chunk):
        (out / f"g_{(base + start) >> 16:04X}.bin").write_bytes(image[start:start + chunk])
    print(f"wrote {len(image):#x} bytes to {out}")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    main(*sys.argv[1:])
