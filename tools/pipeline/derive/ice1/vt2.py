from x1 import *
from fn import fn
import re
def vstores(start, n=3000):
    regs={}; out=[]
    for l in fn(start,n):
        a,m,*o=l.split(' ',2); o=o[0] if o else ''
        ops=[x.strip() for x in o.split(',')]
        if m=='lis': regs[ops[0]]=(int(ops[1],16)<<16)&0xffffffff
        elif m=='addi' and ops[1] in regs: regs[ops[0]]=(regs[ops[1]]+int(ops[2],16))&0xffffffff
        elif m=='stw' and ops[1].startswith('0(') and ops[0] in regs and 0x82100000<regs[ops[0]]<0x821f0000:
            out.append((a,ops[1],hex(regs[ops[0]])))
        elif m=='stw' and '(r20)' in ops[1]:
            out.append((a,'SLOT',int(ops[1].split('(')[0],16)//4))
        elif m=='bl': out.append((a,'BL',o))
    return out
if __name__=='__main__':
    import sys
    for x in vstores(int(sys.argv[1],16)):
        if x[1]!='BL' or len(sys.argv)>2: print(x)
