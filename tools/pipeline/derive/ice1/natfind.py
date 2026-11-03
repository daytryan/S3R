from x1 import *
import sys
names = sys.argv[1:]
for n in names:
    for m in re.finditer(re.escape(n.encode()) + b'\0', d):
        va = 0x82000000 + m.start()
        if d[m.start()-1] != 0: continue
        # find pointers to it in data
        pat = struct.pack('>I', va)
        hits = [0x82000000 + h.start() for h in re.finditer(re.escape(pat), d)]
        xr = xrefs([va])
        print(n, hex(va), 'ptrs', [hex(h) for h in hits[:5]], 'code', [hex(a) for a, _ in xr[:5]])
        for h in hits[:3]:
            o = h - 0x82000000
            print('   around', [hex(x) for x in struct.unpack_from('>6I', d, o - 8)])
