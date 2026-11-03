from fn import fn
import sys
def run(start, n=3000):
    regs={}
    for l in fn(start,n):
        a,m,*o=l.split(' ',2); o=o[0] if o else ''
        ops=[x.strip() for x in o.split(',')]
        if m=='li': regs[ops[0]]=int(ops[1],16)
        elif m=='mr' and ops[1] in regs: regs[ops[0]]=regs[ops[1]]
        elif m=='mr': regs.pop(ops[0],None)
        elif m in ('stw',) and '(' in ops[1]:
            off=int(ops[1].split('(')[0],16)
            if 0x138<=off<=0x150 or off in (0x20,0x24):
                print(a, m, ops[1], ops[0], '=', regs.get(ops[0]))
            if '(r20)' in ops[1]: print('   SLOT', off//4)
        elif m=='bl':
            for r in ('r3','r4','r5','r6','r7','r8','r9','r10','r11','r12'): regs.pop(r,None)
        elif m in ('lwz','addi','lis','add','slwi','ori','lbz','rlwinm','clrlwi','mulli'):
            regs.pop(ops[0],None)
for x in sys.argv[1:]:
    print('==',x); run(int(x,16))
