"""Slappy's gate (round2 10-14): Skate 2's DynamicHull "slappys", for crates/skate-game/src/physics/props.rs.

RETAIL (.superpowers/sdd/2026-10-13-s2-tutorial/round2-progress.md, "10-14 Slappy's gate"): vault ChallengeCore
challenges/slappys defaults the hull to "Locked" (Hash_4C91113947CBBBED) and maps "Locked" to the stream
missions.big data/content/missions/PJ03_SlappysGate (Hash_0F2FE38E547C5960); "Unlock" maps to none. Loading the stream
(0x823a8be8) draws its model (cPres) and adds its collision (cSim): SlappysGateDoor_1000 (tUnlockableInstanceID
0x0006764c03e38706), a double-sided chain-link door panel already in world space.

Runs old_tutorial_dmos.py (same arguments) with the gate added to its package (report row: retail collision, surface,
grab false), then adds to dmos.json
  hulls: {slappys: {default: "Locked", states: {Locked: [{name, instance_id, template_id, matrix}]}}}
Run this instead of old_tutorial_dmos.py, or after it: that one writes the package and dmos.json without the gate.
The skate2-tutorial stage runs it after the export (user 10-28: until then only a hand run made the gate).

usage: py old_tutorial_gate.py REPO SKATE2_ROOT ASSETS
"""
import json, runpy, struct, sys, tempfile
from pathlib import Path
import numpy as np

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
from s2big import Big
from prepare_hawaiian_dream import prepare
import retail_collision_mesh as rcm
import tools.asset_pipeline.dynamic_props as dp

OUT = assets/'private/skate2-tutorial'
STREAM = 'PJ03_SlappysGate'
# The vault rows (RETAIL): hull, its default state, the state whose stream is PJ03_SlappysGate, the door instance.
HULL, DEFAULT, LOCKED = 'slappys', 'Locked', 'Locked'
NAME, INSTANCE = 'SlappysGateDoor_1000', '0006764C03E38706'

with tempfile.TemporaryDirectory(prefix='skate2-gate-') as work:
    work = Path(work)
    (work/'raw').mkdir()
    missions = Big(s2game/'data/content/missions.big')
    for e in missions.entries:
        if f'/missions/{STREAM}/'.lower() in e.path.replace(chr(92), '/').lower():
            (work/'raw'/Path(e.path).name).write_bytes(missions.read(e))
    root = work/'cache'
    prepare(stream_directory=work/'raw', output_root=root, utt_root=repo/'tools/vendor/utt', district_name=STREAM,
            map_name=STREAM, raw_texture_cache=True, allow_material_import_order_fallback=True)
    manifest = json.loads((root/'manifest.json').read_text())
    [model], [sim] = manifest['models'], manifest['simulation_assets']
    assert all(f'0x{INSTANCE}'.lower() in m['name'].lower() for m in model['meshes']), 'not the door model'

    # cSim: the EB000D tInstance (+132) names its EB000A, whose +12 is the Volume (INFERRED: the record's one section
    # reference; this stream has no 0x40006 body, unlike the DMO templates' cPres_Global, dp.template_collision).
    raw = (root/sim['rx2']).read_bytes()
    table = dp.sections(raw)
    [(_, at, _)] = dp.records(raw, 0xEB000D, 160)
    index, = struct.unpack_from('>I', raw, at+132)
    assert table[index][5] == 0xEB000A, 'unresolved collision'
    volume, = struct.unpack_from('>I', raw, table[index][0]+12)
    assert table[volume][5] == 0x80001, 'unresolved volume'
    assert not any(t[5] == 0xEB001F for t in table), 'GRABDATA on the gate'
    triangles = lambda mesh: rcm.decode_clustered_mesh(mesh).triangles
    shape = dp.volume_triangles(raw, table, table[volume][0], lambda m: [(t.a, t.b, t.c) for t in triangles(m)])
    [surface] = {t.surface for s in table if s[5] == 0x80006 for t in triangles(raw[s[0]:s[0]+s[2]])}

    # The model is in world space (identity, as catalog() reads its template); its texture ids are the manifest's.
    key = model['asset_id'][2:].upper()   # template id: the door model (the tInstance's template field is -1)
    template = dict(matrix=np.eye(4), model_matrix=np.eye(4), meshes=model['meshes'], npz=root/model['npz'],
                    asset_id=model['asset_id'])
    textures = {k: dict(v, **{p: str((root/v[p]).resolve()) for p in ('rgba', 'png') if p in v})
                for k, v in manifest['textures'].items()}
    gate = dict(name=NAME, instance_id=INSTANCE, template_id=key, matrix=np.eye(4).tolist())
    note = dict(collision=np.round(shape, 4).tolist(), grab=False, surface=surface)

    real = dp.write_package
    def write_package(map_name, district, rows, templates, dmo_textures, output, *, annotate):
        assert key not in templates, 'gate template id clashes with a DMO'
        # A shared texture id (the default cubemap) must be the same texture, as catalog() requires.
        assert all(Path(dmo_textures[k]['rgba']).read_bytes() == Path(textures[k]['rgba']).read_bytes()
                   for k in dmo_textures.keys() & textures.keys()), 'conflicting gate texture'
        placed = real(map_name, district, rows + [gate], {**templates, key: template}, {**textures, **dmo_textures},
                      output, annotate=lambda item, transform: note if item is gate else annotate(item, transform))
        assert placed == (len(rows) + 1, 0), placed
        return len(rows), 0   # what old_tutorial_dmos.py checks of its own rows
    dp.write_package = write_package
    sys.argv = [str(here/'old_tutorial_dmos.py'), str(repo), str(s2game), str(assets)]
    runpy.run_path(str(here/'old_tutorial_dmos.py'), run_name='__main__')

dmos = json.loads((OUT/'dmos.json').read_text(encoding='utf-8'))
dmos['hulls'] = {HULL: {'default': DEFAULT, 'states': {LOCKED: [gate]}}}
(OUT/'dmos.json').write_text(json.dumps(dmos, indent=1), encoding='utf-8')
# Self-check: two quads 1.5 cm apart (one per face), 3.53 m wide and 3.32 m tall at Slappy's, retail surface 929.
assert len(shape) == 4 and surface == 929, (len(shape), surface)
lo, hi = shape.reshape(-1, 3).min(0), shape.reshape(-1, 3).max(0)
assert np.allclose(lo, [1595.208, 100.181, -511.073], atol=2e-3) and np.allclose(hi, [1597.496, 103.501, -508.353], atol=2e-3)
report = json.loads((assets/'private'/(dmos['source'] + '.json')).read_text())
assert [i['instance_id'] for i in report['instances']].count(INSTANCE) == 1
print(f'{HULL}: {DEFAULT} -> {NAME} ({len(shape)} triangles, surface {surface}) ->', OUT/'dmos.json')
