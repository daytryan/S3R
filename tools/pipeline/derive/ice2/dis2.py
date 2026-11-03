from xref import *
import capstone, sys
md = capstone.Cs(capstone.CS_ARCH_PPC, capstone.CS_MODE_32 | capstone.CS_MODE_BIG_ENDIAN)
def dis(start, count):
    for k in range(count):
        va = start + 4 * k; o = va2off(va)
        ins = next(md.disasm(d[o:o + 4], va), None)
        print(hex(va), *( (ins.mnemonic, ins.op_str) if ins else ('.word', d[o:o+4].hex()) ))
if __name__ == '__main__':
    dis(int(sys.argv[1], 16), int(sys.argv[2]))
