from xref import *
import math
fl = lambda va: struct.unpack_from('>f', d, va2off(va))[0]
def run(start, end, r, f, value, sinu16=lambda x: math.sin(x * 2 * math.pi / 65536)):
    mem = {}
    pc = start
    while pc < end:
        w = words[(pc - TEXT_VA) // 4]; op = w >> 26
        D, A, B, C = (w >> 21) & 31, (w >> 16) & 31, (w >> 11) & 31, (w >> 6) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if op == 59:
            xo = (w >> 1) & 31
            g = lambda k: f.get(k, 0.)
            f[D] = {25: lambda: g(A) * g(C), 18: lambda: g(A) / g(B), 20: lambda: g(A) - g(B), 21: lambda: g(A) + g(B),
                    22: lambda: math.sqrt(g(B)), 29: lambda: g(A) * g(C) + g(B), 30: lambda: -(g(A) * g(C) - g(B)),
                    28: lambda: g(A) * g(C) - g(B), 31: lambda: -(g(A) * g(C) + g(B))}[xo]()
        elif op == 63:
            xo = (w >> 1) & 0x3ff
            if xo == 72: f[D] = f.get(B, 0.)
            elif xo == 40: f[D] = -f.get(B, 0.)
            else: raise Exception(hex(pc) + ' fop ' + str(xo))
        elif op == 48:
            f[D] = fl((r[A] + simm) & 0xffffffff) if A != 1 else mem[simm]
        elif op == 52:
            assert A == 1; mem[simm] = f[D]
        elif op == 14: r[D] = simm if A == 0 else (r.get(A, 0) + simm) & 0xffffffff
        elif op == 15: r[D] = (imm << 16) & 0xffffffff if A == 0 else 0
        elif op == 21: r[A] = r.get(D, 0) & 0xffff  # clrlwi 16 only
        elif op == 31: 
            xo = (w >> 1) & 0x3ff
            if xo == 444: r[A] = r.get(D, 0)
        elif op == 32: pass
        elif op == 18:
            tgt = (pc + ((w & 0x3fffffc) - (0x4000000 if w & 0x2000000 else 0))) & 0xffffffff
            if tgt == 0x8283e7a0: f[1] = value(r[4], r[5])
            elif tgt == 0x8283cf38: f[1] = sinu16(r[3])
            elif tgt == 0x8283ceb0: return mem
            else: raise Exception('call ' + hex(tgt))
        else: raise Exception(hex(pc) + ' op ' + str(op))
        pc += 4
    return mem
