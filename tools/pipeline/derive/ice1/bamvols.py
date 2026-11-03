import pickle, struct, collections, re, json
sims = pickle.load(open('bam_sims.pkl', 'rb'))
def sections(d):
    count, table_at = struct.unpack_from('>I', d, 0x20)[0], struct.unpack_from('>I', d, 0x30)[0]
    return [struct.unpack_from('>6I', d, table_at + 24 * i) for i in range(count)]
kinds = collections.Counter()
vols = {}
names_seen = collections.Counter()
for aid, d in sims:
    for at, _, size, _, _, kind in sections(d):
        kinds[hex(kind)] += 1
        if kind == 0xEB000D:
            _, n, _, first = struct.unpack_from('>4I', d, at)
            for r in range(n):
                o = at + first + 0xA0 * r
                ptr = at + struct.unpack_from('>I', d, o + 0x88)[0]
                full = d[ptr:d.index(b'\0', ptr)].decode('latin-1')
                m = struct.unpack_from('>16f', d, o)
                vols.setdefault(full.split('_0x', 1)[0].lower(), {'full': full, 'kind': 'aabb', 'identity': m == (1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1),
                    'min': struct.unpack_from('>3f', d, o + 0x40), 'max': struct.unpack_from('>3f', d, o + 0x50)})
print(kinds)
print(len(vols), sum(1 for v in vols.values() if not v['identity']), 'non-identity')
for k in ['fp_u1_ownspot03_challengeboundary_1001', 'he_p1_skate02_scoreboundary', 'ol_besttrick_financial_library_scoreboundary_trick01', 'ol_jam_suburbs_school_scoreboundary1_1000', 'fp_u1_ownspot03_spotvolume_1000']:
    print(k, vols.get(k))
pickle.dump(vols, open('bam_vols.pkl', 'wb'))
