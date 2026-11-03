from xref import *
for name in (b'g_Fisheye_Warp\0', b'g_VignetteUVOffset\0', b'Vignette_texture\0'):
    o = d.find(name)
    while o >= 0:
        va = off2va(o)
        print(name, hex(o), hex(va), [(hex(a), hex(t)) for a, t in xrefs([va])])
        o = d.find(name, o + 1)
