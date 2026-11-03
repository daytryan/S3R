"""Skate 1 default.xex memory image (SKATE_XEX_IMAGE) helpers: va<->off, words, xrefs, disasm."""
import os, struct, sys, re
d = open(os.environ['SKATE_XEX_IMAGE'], 'rb').read()  # derive.py writes it from the player's Skate 1 default.xex
SECS = [(0x82000000, 0x1250000, 0)]  # s1.img is the memory image
TEXT_VA, TEXT_SIZE, TEXT_RAW = 0x821f0000, 0xe6994c, 0x1f0000
def va2off(va):
    for v, n, r in SECS:
        if v <= va < v + n: return va - v + r
    return None
words = struct.unpack_from('>%dI' % (TEXT_SIZE // 4), d, TEXT_RAW)
def xrefs(targets):
    targets = set(targets); out = []; lis = {}
    for i, w in enumerate(words):
        op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if op == 15 and ra == 0: lis[rd] = (imm << 16, i)
        elif op in (14, 24) and ra in lis and i - lis[ra][1] < 64:
            t = (lis[ra][0] + (simm if op == 14 else imm)) & 0xffffffff
            if t in targets: out.append((TEXT_VA + 4 * i, t))
        elif op in (32, 48, 34, 40, 36, 52, 38) and ra in lis and i - lis[ra][1] < 64:
            t = (lis[ra][0] + simm) & 0xffffffff
            if t in targets: out.append((TEXT_VA + 4 * i, t))
    return out
def s_at(va):
    o = va2off(va)
    if o is None: return None
    s = d[o:o + 48].split(b'\0')[0]
    return s.decode('latin-1') if s and all(32 <= c < 127 for c in s) else None
def f32_at(va):
    o = va2off(va); return struct.unpack_from('>f', d, o)[0] if o is not None else None
try:  # disassembly helpers only: the build (params1.py, enums.py) runs without capstone
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_PPC, capstone.CS_MODE_32 | capstone.CS_MODE_BIG_ENDIAN)
except ImportError:
    md = None
def dis(start, count):
    o = va2off(start)
    for ins in md.disasm(d[o:o + 4 * count], start):
        print(hex(ins.address), ins.mnemonic, ins.op_str)
