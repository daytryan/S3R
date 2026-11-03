"""Decode RW4 PS3 textures (CellGcmTexture descriptors) into rx2_parser.Texture objects.

Same TOC as Xbox RX2: a 0x000200E8 record points at the GCM descriptor; the
record before it gives the RSX data offset (relative to the 0x44 header size) and size.
RSX DXT is linear little-endian; swapping 16-bit words gives the Xbox byte order the
shared decoders expect, with tiling disabled.
"""
import struct
import numpy as np
import rx2_parser as rx
import rx2_fast

MAGIC_PS3 = b'\x89RW4ps3'
GCM_LINEAR = 0x20
# Not rx.FMT_B5G6R5: the preparer re-decodes that id with the Xbox 565 decoder.
FMT_PS3_R5G6B5 = 0x184
# GCM base format -> (Xbox format id reused for naming/downstream, bytes per 4x4 block or per texel, block?)
GCM = {
    0x86: (rx.FMT_DXT1, 8, True),
    0x87: (rx.FMT_DXT3, 16, True),
    0x88: (rx.FMT_DXT5, 16, True),
    0x85: (rx.FMT_A8R8G8B8, 4, False),
    0x84: (FMT_PS3_R5G6B5, 2, False),
    0x81: (rx.FMT_A8, 1, False),
}


def _level_size(w, h, unit, block):
    return ((w + 3) // 4) * ((h + 3) // 4) * unit if block else w * h * unit


def _deswizzle(raw, w, h, unit):
    """RSX swizzled (Morton, x in even bits) surface -> linear rows. Power-of-two sizes only."""
    y, x = np.mgrid[0:h, 0:w]
    index = np.zeros_like(x)
    bit, shift = 1, 0
    bx, by = w, h
    while bx > 1 or by > 1:
        if bx > 1:
            index |= ((x & bit) != 0) << shift; shift += 1; bx >>= 1
        if by > 1:
            index |= ((y & bit) != 0) << shift; shift += 1; by >>= 1
        bit <<= 1
    src = np.frombuffer(raw, np.uint8, count=w * h * unit).reshape(-1, unit)
    return src[index.reshape(-1)].tobytes()


def _decode_level(raw, w, h, gcm):
    fmt, unit, block = GCM[gcm & ~(GCM_LINEAR | 0x40)]
    if block:
        swapped = np.frombuffer(raw, np.uint8).reshape(-1, 2)[:, ::-1].tobytes()
        kind = {rx.FMT_DXT1: 1, rx.FMT_DXT3: 3, rx.FMT_DXT5: 5}[fmt]
        return rx2_fast._decode(swapped, w, h, kind, tiled=False)
    if not gcm & GCM_LINEAR:
        raw = _deswizzle(raw, w, h, unit)
    if fmt == rx.FMT_A8R8G8B8:
        return np.frombuffer(raw, np.uint8, count=w * h * 4).reshape(-1, 4)[:, [1, 2, 3, 0]].tobytes()
    if fmt == FMT_PS3_R5G6B5:
        v = np.frombuffer(raw, '>u2', count=w * h).astype(np.uint32)
        return np.stack((((v >> 11) & 31) * 255 // 31, ((v >> 5) & 63) * 255 // 63, (v & 31) * 255 // 31,
                         np.full_like(v, 255)), axis=1).astype(np.uint8).tobytes()
    a = np.frombuffer(raw, np.uint8, count=w * h)
    return np.stack((np.zeros_like(a),) * 3 + (a,), axis=1).tobytes()


class PS3Texture(rx.RX2File):
    """Drop-in for rx2_parser.parse_rx2 results: .textures, .warnings, .entries, .data, .data_base."""

    def parse(self):
        data = self.data
        if data[:7] != MAGIC_PS3:
            raise rx.RX2ParseError('not a PS3 RW4 arena')
        self.file_count, = struct.unpack_from('>I', data, 0x20)
        self.file_table_offset, = struct.unpack_from('>I', data, 0x30)
        self.data_base, = struct.unpack_from('>I', data, 0x44)
        self.type_id, = struct.unpack_from('>I', data, 0x70)
        self.entries, prev = [], None
        for i in range(self.file_count):
            f0, f1, f2, f3, f4, t = struct.unpack_from('>iiiiiI', data, self.file_table_offset + i * 24)
            e = rx.TOCEntry(i, f0, f1, f2, f3, f4, t)
            if prev:
                e.data_offset, e.buffer_size = prev
            self.entries.append(e)
            prev = (f0, f2)
        return self

    def _decode_texture(self, entry):
        gcm, mips, dim, cube, _, w, h = struct.unpack_from('>BBBBIHH', self.data, entry.info_offset)
        if gcm & ~(GCM_LINEAR | 0x40) not in GCM or dim != 2:
            raise rx.RX2ParseError(f'unsupported PS3 texture format 0x{gcm:02X} dim {dim}')
        fmt, unit, block = GCM[gcm & ~(GCM_LINEAR | 0x40)]
        base = self.data_base + entry.data_offset
        raw = self.data[base:base + entry.buffer_size]
        chain = sum(_level_size(max(w >> m, 1), max(h >> m, 1), unit, block) for m in range(max(mips, 1)))
        faces = 6 if cube else 1
        stride = (chain + 127) & ~127 if cube else chain
        if len(raw) < stride * (faces - 1) + _level_size(w, h, unit, block):
            raise rx.RX2ParseError(f'truncated PS3 texture {w}x{h} fmt 0x{gcm:02X}')
        level = _level_size(w, h, unit, block)
        rgba = b''.join(_decode_level(raw[f * stride:f * stride + level], w, h, gcm) for f in range(faces))
        t = rx.Texture(entry.index, w, h * faces, fmt, rgba, base, entry.buffer_size)
        t.cube_faces, t.face_stride = faces, stride
        return t


def parse(source):
    t = PS3Texture(source)
    t.parse()
    t.decode_textures()
    return t
