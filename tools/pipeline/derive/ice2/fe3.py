from xref import *
import capstone, re
md = capstone.Cs(capstone.CS_ARCH_PPC, capstone.CS_MODE_32 | capstone.CS_MODE_BIG_ENDIAN)
R30 = 0x822083c8
start = 0x8269c270
o = va2off(start)
last6 = None; last4=None
for ins in md.disasm(d[o:o + 4 * 4000], start):
    s = ins.op_str
    m = re.match(r'r6, r31, (-?0x[0-9a-f]+|\d+)', s)
    if ins.mnemonic == 'addi' and m: last6 = 'r31+' + m.group(1)
    if ins.mnemonic == 'mr' and s.startswith('r6, '): last6 = s
    m = re.match(r'r5, r30, (-?0x[0-9a-f]+|\d+)', s)
    if ins.mnemonic == 'addi' and m:
        va = R30 + int(m.group(1), 16 if 'x' in m.group(1) else 10)
        name = d[va2off(va):va2off(va)+40].split(b'\0')[0]
        if b'Fisheye' in name or b'Vignette' in name or b'Noise' in name:
            print(hex(ins.address), name, last6)
    if ins.mnemonic == 'blr': print('blr', hex(ins.address)); break
