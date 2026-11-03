from xref import *
import capstone
md = capstone.Cs(capstone.CS_ARCH_PPC, capstone.CS_MODE_32 | capstone.CS_MODE_BIG_ENDIAN)
def dis(start, count):
    o = va2off(start)
    for ins in md.disasm(d[o:o + 4 * count], start):
        print(hex(ins.address), ins.mnemonic, ins.op_str)
if __name__ == '__main__':
    dis(int(sys.argv[1], 16), int(sys.argv[2]))
