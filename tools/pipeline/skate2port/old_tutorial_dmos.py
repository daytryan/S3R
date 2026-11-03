"""Skate 2 tutorial DMOs (G8): the movable objects the lessons place or test, for crates/skate-game/src/physics/props.rs.

Writes assets/private/skate2-tutorial/dmos.json:
  {map, source, challenges: {<challenge>: [{name, instance_id, template_id, matrix, world}]}}
- Mission DMOs: each lesson's missions/<name>/cSim_Global.xsf EB001D table (old_tutorial.mission_placement).
- Testers: every DMOs.AddDMOVolumeTester(<instance id>) in a lesson's objectives that its mission doesn't place is a
  world DMO; it is found in worldbam.big BAM/cSim_*.xsf and listed under that lesson with world: true.
`matrix` is the locator's row-vector matrix in Skate 2 world space (= NSV map space).

Also writes the `source` package, assets/private/<source>.skate + .json (round2 T4): every listed DMO drawn with Skate
2's own model (worldmisc.big world/dmo/DMO through Skate 3's prepare + catalog), its report instance carrying the
template's retail collision in world space (`collision`, triangles) and whether retail lets you grab it (`grab`).

usage: py old_tutorial_dmos.py REPO SKATE2_ROOT ASSETS
"""
import ast, functools, json, math, re, struct, sys, tempfile
from pathlib import Path
import numpy as np

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
from skate3_streams import read_atoc, read_sfil
from tools.asset_pipeline.dynamic_props import locators as dmo_locators, catalog, template_collision, template_grabs,     write_package
import tools.asset_pipeline.dynamic_props as dp

OUT = assets/'private/skate2-tutorial'
# Under assets/private/ (props.rs resolves a source with a '/' there).
SOURCE = 'skate2-tutorial/props/Skate2Tutorial'

# old_tutorial.py's mission reader, reused as-is (that script runs its whole export at import).
g = dict(globals())
for node in ast.parse((here/'old_tutorial.py').read_text(encoding='utf-8')).body:
    if isinstance(node, ast.FunctionDef) and node.name in ('cstr', 'matrix', 'rw4_objects', '_missions', 'write_streams', 'mission_placement'):
        exec(compile(ast.Module([node], []), 'old_tutorial.py', 'exec'), g)

tutorial = json.loads((OUT/'tutorial.json').read_text(encoding='utf-8'))
row = lambda d, world: {'name': d['name'].split('_0x', 1)[0], 'instance_id': d['instance_id'],
                        'template_id': d['template_id'], 'matrix': d['matrix'], 'world': world}
dmos, wanted = {}, {}
for name, challenge in tutorial['challenges'].items():
    placed = [row(d, False) for d in g['mission_placement'](name)[2]]
    sources = json.dumps(challenge['objectives'])
    for tester in re.findall(r'AddDMOVolumeTester\(\s*(\d+)', sources):
        key = f'{int(tester):016X}'
        if all(d['instance_id'] != key for d in placed):
            wanted[key] = name
    if placed:
        dmos[name] = placed

# The world testers: scan the streamed world simulation files for their locators.
big = Big(s2game/'data/content/worldbam.big')
with tempfile.TemporaryDirectory(prefix='skate2-dmos-') as work:
    work = Path(work)
    records = []
    for e in big.entries:
        if e.path.endswith('.xst') and 'Sim' in e.path:
            p = work/Path(e.path).name
            p.write_bytes(big.read(e))
            records += read_atoc(p)
    index = {r.asset_id: r for r in records}
    for e in big.entries:
        if not wanted or not (e.path.endswith('.xsf') and 'cSim' in e.path):
            continue
        p = work/'x.xsf'
        p.write_bytes(big.read(e))
        for asset in read_sfil(p, records, require_all_records=False, record_index=index):
            if asset.data[:4] != b'\x89RW4':
                continue
            try:
                found = dmo_locators(asset.data)
            except ValueError:
                continue
            for d in found:
                if d['instance_id'] in wanted:
                    dmos.setdefault(wanted.pop(d['instance_id']), []).append(row(d, True))
assert not wanted, f'testers not found in the world: {wanted}'

# The package: Skate 2's DMO stream through Skate 3's prepare/catalog, collision from the same RW4 asset.
from prepare_hawaiian_dream import prepare
import retail_collision_mesh as rcm
used = {d['template_id'] for rows in dmos.values() for d in rows}
misc = Big(s2game/'data/content/worldmisc.big')
with tempfile.TemporaryDirectory(prefix='skate2-dmo-models-') as work:
    work = Path(work)
    for e in misc.entries:
        p = e.path.replace(chr(92), '/')
        if '/world/dmo/' in p.lower():
            (work/'raw'/p).parent.mkdir(parents=True, exist_ok=True)
            (work/'raw'/p).write_bytes(misc.read(e))
    root = work/'cache'
    prepare(stream_directory=work/'raw/data/content/world/dmo/DMO', output_root=root, utt_root=repo/'tools/vendor/utt',
            district_name='DMO', map_name='DMO', raw_texture_cache=True, allow_material_import_order_fallback=True)
    templates, textures = catalog([root])
    # Skate 2's texture assets carry no DMO high bit; its bindings do (Skate 3's carry it on both).
    textures.update({f'0x{int(k, 16) | 1 << 63:016x}': v for k, v in list(textures.items())})
    shapes, grabs, edges = {}, set(), {}
    # The skater rides each template's ClusteredMesh alone: retail's SkaterDMOCollisionJob (s2.pe 0x827cf140) tests only
    # GetClusteredMesh; the primitives beside it are the rigid body's proxy (user 10-28: the wedge's base-plate box stood
    # at axle height ahead of its skirt and stopped the board). template_collision walks dp.volume_triangles, so the
    # primitives are left out here; dynamic_props.py itself stays as it is (the maps group fingerprints it).
    whole = dp.volume_triangles
    def ride_surface(raw, table, at, decode_mesh, parent=np.eye(4)):
        if struct.unpack_from('>I', raw, at+0x40)[0] != 6:
            return np.zeros((0, 3, 3))
        return whole(raw, table, at, decode_mesh, parent)
    dp.volume_triangles = ride_surface
    try:
        for model in json.loads((root/'manifest.json').read_text())['models']:
            raw = (root/model['rx2']).read_bytes()
            # Each triangle's retail edge codes ride along in triangle order (10-29 bug 7): props.rs decodes them as the
            # map does (decode_native_edges); its welded recompute mis-pairs the wedge's three-triangle crease at x -0.28.
            for template in sorted(used):
                codes = []
                def decode(mesh, codes=codes):
                    tris = rcm.decode_clustered_mesh(mesh).triangles
                    codes.extend(t.edge_codes for t in tris)
                    return [(t.a, t.b, t.c) for t in tris]
                if found := template_collision(raw, decode, {template}):
                    shapes.update(found)
                    edges[template] = [list(c) for c in codes] if all(c is not None for c in codes) else []
            grabs |= template_grabs(raw)
    finally:
        dp.volume_triangles = whole
    assert used <= shapes.keys(), f'no collision for {used - shapes.keys()}'
    assert all(len(shapes[t]) for t in used), f'no ride mesh for {[t for t in used if not len(shapes[t])]}'
    package = assets/'private'/(SOURCE + '.skate')
    package.parent.mkdir(parents=True, exist_ok=True)
    rows = [d for v in dmos.values() for d in v]
    placed = write_package(tutorial['map'], 'Skate2Tutorial', rows, templates, textures, package, annotate=lambda item, transform: dict(
        collision=np.round(shapes[item['template_id']] @ transform[:3, :3] + transform[3, :3], 4).tolist(),
        collision_edges=edges.get(item['template_id'], []),
        grab=item['template_id'] in grabs))
    assert placed == (len(rows), 0), placed
# Self-check (round2 T4, 10-28): the wedge rides its 118 mesh triangles (its 12 boxes are the rigid body's); the wood
# panel is the one prop with no GRABDATA.
assert len(shapes['00016EBB03E38707']) == 118, len(shapes['00016EBB03E38707'])
assert used - grabs == {'000196B203E38707'}, used - grabs
# 10-29 bug 7: one code per triangle, and the toe crease is retail's (concave 0x5a / near-flat 0x6c), not a knife edge.
assert all(len(edges[t]) in (0, len(shapes[t])) for t in used), {t: (len(edges[t]), len(shapes[t])) for t in used}
assert edges['00016EBB03E38707'][109][0] == 0x5a and edges['00016EBB03E38707'][110] == [0x65, 0x59, 0x6c], edges['00016EBB03E38707'][109:111]

(OUT/'dmos.json').write_text(json.dumps({'map': tutorial['map'], 'source': SOURCE, 'challenges': dmos}, indent=1),
                             encoding='utf-8')
# Self-check: Meet Slappy's tester is the dumpster at the gap (yaw 125 degrees), the puzzle's is the wedge.
slappy = {d['instance_id']: d for d in dmos['m_1a_01_goto_slappys']}
gap = slappy['FDA727257834E7A3']
assert gap['world'] and gap['name'] == 'DumpsterGrunge_DMO_1058' and gap['template_id'] == '000187A003E38707'
assert [round(v, 2) for v in gap['matrix'][3][:3]] == [1697.16, 108.18, -539.22] and abs(gap['matrix'][0][0] + 0.5736) < 1e-3
assert any(d['instance_id'] == f'{11020444654411825821:016X}' for d in dmos['m_1a_06_photo_puzzle'])
print({k: len(v) for k, v in dmos.items()}, '->', OUT/'dmos.json')


def spawn_points(name):
    """GetSpawnLocation's rows (s2.pe 0x82879950, 10-29 bugs 2 and 6): each locator's sub-locations. tLocationDescData
    (0xEB0009) header: locations, sub-locations, strings, then their offsets; a 0x80-byte location keeps its sub count
    at +0x60, their offset (from the header) at +0x64 and its name at +0x68; a sub-location is 0x70 bytes, matrix first."""
    missions, entries = g['_missions']()
    files = {Path(e.path).name: e for path, e in entries if f'/missions/{name}/'.lower() in path}
    out = {}
    with tempfile.TemporaryDirectory(prefix='skate2-spawn-') as work:
        work = Path(work)
        g['write_streams'](work, files, missions.read, f'{name}_Sim.xst', 'cSim_Global.xsf')
        if not (work/f'{name}_Sim.xst').exists():
            return out
        for asset in read_sfil(work/'cSim_Global.xsf', read_atoc(work/f'{name}_Sim.xst')):
            d = asset.data
            if d[:4] != b'\x89RW4':
                continue
            for kind, at, _ in g['rw4_objects'](d):
                if kind == 0xEB0009:
                    count, _, _, first = struct.unpack_from('>4I', d, at)
                    for r in range(count):
                        o = at + first + 0x80 * r
                        subs, offset, label = struct.unpack_from('>3I', d, o + 0x60)
                        if subs:
                            out[g['cstr'](d, at + label)] = [g['matrix'](d, at + offset + 0x70 * k) for k in range(subs)]
    return out


tutorial['placement']['spawn_points'] = {k: v for name in tutorial['challenges'] for k, v in spawn_points(name).items()}
(OUT/'tutorial.json').write_text(json.dumps(tutorial, indent=1), encoding='utf-8')
# Self-check: Follow Slappy's player starts beside Slappy, No Mugshots' player at spawnpoint01.
_sp = tutorial['placement']['spawn_points']
assert [round(v, 3) for v in _sp['m_1a_07_follow_slappy_startlocator01'][0][3][:3]] == [1599.753, 100.538, -512.164]
assert [round(v, 3) for v in _sp['m_1a_05_photo_rail_startlocator01'][0][3][:3]] == [1677.243, 101.63, -503.559]
print('spawn points', len(_sp))
