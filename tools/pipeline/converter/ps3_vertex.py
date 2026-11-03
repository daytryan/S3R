"""RSX vertex element (type, count, offset, stride, usage, _) -> 16-byte Xenos descriptor,
so the Xbox UV/frame decoders read PS3 vertex streams."""
import struct
from collections import namedtuple

Attr = namedtuple('Attr', 'offset descriptor semantic data_type components')
XENOS = {(1, 2): 25, (1, 4): 26, (3, 2): 31, (3, 4): 32, (2, 2): 37, (2, 4): 38, (6, 1): 16}


def xenos(attributes):
    out = []
    for at in attributes:
        d = bytes(at.descriptor)
        if len(d) == 16:
            return attributes
        kind, count, _, _, usage = struct.unpack_from('>BBHHB', d)
        fake = bytearray(16)
        struct.pack_into('>I', fake, 4, XENOS.get((kind, count), 63))
        if 8 <= usage <= 13:
            fake[9], fake[10] = 5, usage - 8  # TEXCOORD n
        elif usage == 14 and kind == 6:
            fake[9] = 6  # packed 11:11:10 tangent
        out.append(Attr(at.offset, bytes(fake), at.semantic, at.data_type, at.components))
    return out
