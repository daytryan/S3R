from xref import *
def f32_at(va):
    o = va2off(va)
    return struct.unpack_from('>f', d, o)[0] if o is not None else None
def s_at(va):
    o = va2off(va)
    if o is None: return None
    s = d[o:o + 48].split(b'\0')[0]
    return s.decode('latin-1') if s and all(32 <= c < 127 for c in s) else None
def run(start, end, table_reg=27):
    r = {}; f = {}; objs = []; cur = None; table = []
    i0, i1 = (start - TEXT_VA) // 4, (end - TEXT_VA) // 4
    for i in range(i0, i1):
        w = words[i]; op = w >> 26; rd, ra = (w >> 21) & 31, (w >> 16) & 31
        imm = w & 0xffff; simm = imm - 0x10000 if imm & 0x8000 else imm
        if op == 14:  # addi / li
            r[rd] = simm & 0xffffffff if ra == 0 else ((r[ra] + simm) & 0xffffffff if r.get(ra) is not None else None)
        elif op == 15:
            r[rd] = (imm << 16) if ra == 0 else ((r[ra] + (imm << 16)) & 0xffffffff if r.get(ra) is not None else None)
        elif op == 24:
            if rd in r and r[rd] is not None: r[ra] = r[rd] | imm
        elif op == 48:  # lfs
            if ra in r and r[ra] is not None: f[rd] = f32_at((r[ra] + simm) & 0xffffffff)
        elif op == 36 and ra == 3:  # stw rS, d(r3)
            if cur is None: cur = {}
            cur[simm] = r.get(rd)
        elif op == 52 and ra == 3:  # stfs
            if cur is None: cur = {}
            cur[('f', simm)] = f.get(rd)
        elif op == 38 and ra == 3:
            if cur is None: cur = {}
            cur[('b', simm)] = r.get(rd)
        elif op == 36 and ra == table_reg and rd == 3:
            table.append((simm, cur)); cur = None
        elif op == 18 and (w & 1):  # bl -> call clobbers r3 etc.
            pass
        elif op == 31:
            xo = (w >> 1) & 0x3ff
            if xo == 444 and rd == ((w >> 11) & 31):  # mr ra, rs
                r[ra] = r.get(rd)
    return table
if __name__ == '__main__':
    st, en = int(sys.argv[1], 16), int(sys.argv[2], 16)
    for off, o in run(st, en):
        if o is None: print(hex(off), None); continue
        name = s_at(o.get(8) or 0); ui = s_at(o.get(0xc) or 0)
        ints = {hex(k): v for k, v in o.items() if isinstance(k, int) and k not in (0, 4, 8, 0xc, 0x10)}
        fl = {hex(k[1]): (round(v, 4) if v is not None else None) for k, v in o.items() if isinstance(k, tuple) and k[0] == 'f'}
        print('slot', (off - 0x10) // 4, hex(off), name, '|', ui, ints, fl)
