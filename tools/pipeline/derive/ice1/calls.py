from x1 import *
import sys
def callers(t):
    out=[]
    for i,w in enumerate(words):
        if w>>26==18 and (w&3)==1:
            off=w&0x3fffffc
            if off&0x2000000: off-=0x4000000
            if TEXT_VA+4*i+off==t: out.append(TEXT_VA+4*i)
    return out
if __name__=='__main__':
    for x in sys.argv[1:]:
        print(x,[hex(c) for c in callers(int(x,16))])
