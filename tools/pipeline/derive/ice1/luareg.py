"""Lua natives: {char *name, lua_CFunction fn} pairs in data. Prints name -> fn for every such pair."""
from x1 import *
import json
out = {}
TEXT_END = TEXT_VA + TEXT_SIZE
for off in range(0, len(d) - 8, 4):
    p, f = struct.unpack_from('>II', d, off)
    if TEXT_VA <= f < TEXT_END and 0x82000000 <= p < 0x821f0000:
        n = s_at(p)
        if n and n[0].isupper() and n.isidentifier() and len(n) > 3:
            out.setdefault(n, []).append(hex(f))
json.dump(out, open('luareg.json', 'w'), indent=0)
print(len(out))
