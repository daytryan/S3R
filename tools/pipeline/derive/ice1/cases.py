from x1 import *
import sys
def cases(table, n, exit_addrs, limit=80):
    ts=[struct.unpack_from('>I',d,va2off(table+4*i))[0] for i in range(n)]
    for i,t in enumerate(ts):
        out=[]; a=t
        for k in range(limit):
            o=va2off(a); ins=list(md.disasm(d[o:o+4],a))
            if not ins: a+=4; continue
            x=ins[0]; s=f'{x.mnemonic} {x.op_str}'
            out.append(s)
            if x.mnemonic=='b' and int(x.op_str,16) in exit_addrs: break
            if x.mnemonic in ('blr',) : break
            if x.mnemonic=='b' and (int(x.op_str,16)<t or int(x.op_str,16)>t+0x400): out.append('->'); break
            a+=4
        print(f'case {i} @{t:x}: ' + ' | '.join(out))
if __name__=='__main__':
    cases(int(sys.argv[1],16), int(sys.argv[2]), {int(x,16) for x in sys.argv[3:]})
