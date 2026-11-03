from x1 import *
import sys
def table(sw):
    o = va2off(sw)
    ins = list(md.disasm(d[o:o+4*12], sw))
    n = int(ins[0].op_str.split(',')[-1], 16) + 1
    hi = lo = None
    for i in ins:
        if i.mnemonic == 'lis' and i.op_str.startswith('r12'): hi = int(i.op_str.split(',')[-1], 16) & 0xffff
        if i.mnemonic == 'addi' and i.op_str.startswith('r12, r12'):
            lo = int(i.op_str.split(',')[-1], 16); lo = lo & 0xffff; lo = lo - 0x10000 if lo >= 0x8000 else lo
    base = ((hi << 16) + lo) & 0xffffffff
    return n, base, [struct.unpack_from('>I', d, va2off(base) + 4*i)[0] for i in range(n)]
def body(t, limit=60, stop=('b', 'blr', 'bctr')):
    o = va2off(t); out = []
    for ins in md.disasm(d[o:o+4*limit], t):
        out.append(f'{ins.mnemonic} {ins.op_str}')
        if ins.mnemonic in stop: break
    return out
if __name__ == '__main__':
    sw = int(sys.argv[1], 16)
    n, base, ts = table(sw)
    print('cases', n, 'table', hex(base))
    for i, t in enumerate(ts):
        print(i, hex(t), ' | '.join(body(t)))
