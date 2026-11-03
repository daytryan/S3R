import pickle, struct, json
from rx import M, E, sections
def parse(d, out, src):
    for at, _, size, _, _, kind in sections(d):
        if kind not in (0xEB000D, 0xEB0019):
            continue
        _, n, _, first = struct.unpack_from('>4I', d, at)
        rs, name_at = (0xA0, 0x88) if kind == 0xEB000D else (0xF0, 0xE0)
        for r in range(n):
            o = at + first + rs * r
            ptr = at + struct.unpack_from('>I', d, o + name_at)[0]
            full = d[ptr:d.index(b'\0', ptr)].decode('latin-1')
            m = struct.unpack_from('>16f', d, o)
            out.setdefault(full.split('_0x', 1)[0].lower(), {'src': src, 'kind': hex(kind), 'matrix': [list(m[i:i+4]) for i in (0,4,8,12)],
                'min': list(struct.unpack_from('>3f', d, o + 0x40)), 'max': list(struct.unpack_from('>3f', d, o + 0x50))})
vols = {}
for n, e in E.items():
    if n.endswith('.rx2') and not n.endswith('_textures.rx2'):
        try: parse(M.read(e), vols, n)
        except Exception as ex: print('skip', n, ex)
for aid, d in pickle.load(open('bam_sims.pkl', 'rb')):
    parse(d, vols, 'BAM:%016x' % aid)
print(len(vols))
L = json.load(open('lessons.json'))
AO, DO = 'Hash_F9A9A1DDED84A643', 'Hash_FD40E4A31027C984'
par = {v['_parent'] for v in L.values()}
on = [k for k, v in L.items() if v.get(AO) and not v.get(DO) and k not in par and v.get('ChallengeType') != 29]
F = ('ScoringBoundary', 'SpotVolumes', 'OTSTriggerBoundary', 'OTSSpotVolumes', 'OTSScoringBoundary', 'BestTrickRegions', 'BestTrickSpotVolumes', 'CheckPoints', 'StartLine', 'FinishLine')
miss, hit = [], 0
for k in on:
    for f in F:
        val = L[k].get(f)
        for x in (val if isinstance(val, list) else [val] if val else []):
            if x.lower() in vols: hit += 1
            else: miss.append((k, f, x))
print('hit', hit, 'miss', len(miss)); print(miss[:30])
non = [k for k, v in vols.items() if v['matrix'] != [[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]]
print('non-identity', len(non), non[:5])
