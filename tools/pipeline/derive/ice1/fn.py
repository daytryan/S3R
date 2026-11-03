from x1 import *
import sys
def fn(start, maxn=600):
    a = start; out = []; n = 0
    while n < maxn:
        o = va2off(a)
        got = False
        for ins in md.disasm(d[o:o+4*maxn], a):
            if ins.address != start and ins.mnemonic == 'mflr' and ins.op_str == 'r12':
                return out
            out.append(f'{ins.address:x} {ins.mnemonic} {ins.op_str}'); n += 1; got = True
            a = ins.address + 4
        if not got:
            a += 4
    return out
if __name__ == '__main__':
    for x in sys.argv[1:]:
        print('====', x); print('\n'.join(fn(int(x, 16))))
