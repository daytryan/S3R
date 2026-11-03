"""Skate 2's own Edit Skater presentation for the Skate 2 room (graphics_menu.rs ROOMS): its editor clips, where the
skater is in each editor state and each page's camera.

Writes assets/private/cas-skate2/ (cas_room.rs reads it; without it the Skate 2 rooms keep Skate 3's presentation):
  cac_edit_male.abin, cac_edit_female.abin  data/anim/ in anim.big: CAS_STAND, CAS_SITTING (the couch), CAS_ONTABLE
                                            (Back Alley Pete's table), CAS_GESTURESTAND_CYCLE
  cas.json  bodies   skater.vlt cac_body rows that are not gestures (CASCharacterEntity::SetBodyState 8259e8c8):
                     position, rotation (degrees about +Y), animation
            cameras  cac_camera rows, parents resolved (CasCamera::SetTargetState 825874f0): position, look, up, fov
                     (degrees, vertical: GetFOVY 827f64b0)
            rooms    the Skate 2 room package: the move (yaw radians about +Y, then translation) taking Skate 2's room
                     space, which the vault positions are in, onto the room old_room.py writes (its YAW and MOVE).
The vault rows' layout offsets (position @0, look @16, fov @32; body position @0, animation @16, rotation @20,
gesture @24) are asserted against the vault's class definitions, so a field swap fails here instead of in the game.
Existing files are never replaced with different content.

usage: py -3 old_cas.py REPO SKATE2_ROOT ASSETS
"""
import json, math, struct, sys, tempfile
from pathlib import Path

repo, s2game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools/pipeline/converter'), str(here)]
from s2big import Big
from tools.asset_pipeline import vlt
from tools.asset_pipeline.environment import NAMES
from old_room import MOVE, YAW

OUT = assets/'private/cas-skate2'
# Row keys as the game hashes them (vlt.hash64). Unresolved ones stay Hash_<key>; cas_room.rs uses only these.
KEYS = ('default', 'standing', 'slappys', 'back_alley_petes', 'idle', 'face', 'hat', 'head', 'body', 'chest', 'legs',
        'feet', 'rings', 'wrist_left', 'wrist_right', 'skateboard', 'skatetruck', 'skatewheel', 'necklace',
        'earring_left', 'earring_right', 'lipring', 'brow', 'mouth', 'jaw', 'nose', 'eyes', 'chin')
FIELDS = {  # (class, field hash): (name, layout offset or None for a non-layout field)
    ('cac_camera', 0xF323B5D55CA55A89): ('position', 0), ('cac_camera', 0xF0FC2B540F11109E): ('look', 16),
    ('cac_camera', 0xEFB7F7EF35801C19): ('fov', 32), ('cac_camera', 0x513070D9CBBE801A): ('up', None),
    ('cac_body', 0x6D2BEA1EFEA33425): ('position', 0), ('cac_body', 0x249D0D4624998D68): ('animation', 16),
    ('cac_body', 0x4591287983616615): ('rotation', 20), ('cac_body', 0x9C04CB22EB98D1CC): ('gesture', 24),
}


def vault_rows():
    db = Big(s2game/'data/db/db.big')
    with tempfile.TemporaryDirectory(prefix='skate2-db-') as work:
        for e in db.entries:
            if Path(e.path).stem == 'skater':
                (Path(work)/Path(e.path).name).write_bytes(db.read(e))
        stem = Path(work)/'skater'
        sv, sb, se = vlt.vault(stem)
        for _, kind, _, at in se:  # the class definitions: check the offsets FIELDS relies on
            if kind != 0x2A7895AC4A876152:
                continue
            key, _, count, defs, *_ = vlt.unpack('QIIIIII', sv, at)
            cls = {vlt.hash64('cac_camera'): 'cac_camera', vlt.hash64('cac_body'): 'cac_body'}.get(key)
            for i in range(count if cls else 0):
                field, _, offset, _, _, flags, _ = vlt.unpack('QQHHHBB', sb, defs + i*24)
                name, want = FIELDS.get((cls, field), (None, None))
                if name and want is not None:
                    assert flags & 2 and offset == want, (cls, name, offset, flags)
        names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
        rows = vlt.convert(stem, stem, [*names, *NAMES, 'cac_camera', 'cac_body', *KEYS])['collections']
    return [r for r in rows if r['class'] in ('cac_camera', 'cac_body')]


def value(field, data):
    raw = bytes.fromhex(data['data']) if data['type'] != 'EA::Reflection::Text' else data['data']
    if data['type'] == 'Math::Vector3':
        return [round(v, 6) for v in struct.unpack('>3f', raw[:12])]
    if data['type'] == 'EA::Reflection::Float':
        return round(struct.unpack('>f', raw)[0], 6)
    if data['type'] == 'EA::Reflection::Bool':
        return raw[0] != 0
    return raw


def presentation():
    rows = vault_rows()
    by_key = {(r['class'], r['key']): r for r in rows}
    hashes = {f'Hash_{vlt.hash64(k):016X}': k for k in KEYS}
    out = {'bodies': {}, 'cameras': {}}
    for r in rows:
        fields, cur = {}, r
        while cur:  # a row inherits what it leaves out from its parents
            for h, data in cur['fields'].items():
                name = FIELDS.get((r['class'], int(h[5:], 16) if h.startswith('Hash_') else vlt.hash64(h)), (None,))[0]
                if name:
                    fields.setdefault(name, value(name, data))
            cur = by_key.get((r['class'], cur['parent'])) if cur['parent'] else None
        key = hashes.get(r['key'], r['key'])
        if r['class'] == 'cac_body' and not fields.get('gesture'):
            out['bodies'][key] = {k: fields[k] for k in ('position', 'rotation', 'animation')}
        elif r['class'] == 'cac_camera':
            out['cameras'][key] = {'position': fields['position'], 'look': fields['look'],
                                   'up': fields.get('up', [0.0, 1.0, 0.0]), 'fov': fields['fov']}
    for state in ('standing', 'slappys', 'back_alley_petes', 'idle'):
        assert state in out['bodies'], state
    assert out['bodies']['slappys']['animation'] == 'CAS_Sitting', out['bodies']['slappys']
    return out


def write(path, data):
    if path.exists() and path.read_bytes() != data:
        sys.exit(f'{path} exists with other content; not replaced')
    path.write_bytes(data)


def main():
    out = presentation()
    out['rooms'] = {'CreateSkater_Skate2': {'yaw': YAW, 'translation': list(MOVE)}}
    OUT.mkdir(parents=True, exist_ok=True)
    anim = Big(s2game/'data/big/anim.big')
    for gender in ('male', 'female'):
        entry = next(e for e in anim.entries if e.path == f'data/anim/cac_edit_{gender}.abin')
        write(OUT/entry.path.rsplit('/', 1)[1], anim.read(entry))
    write(OUT/'cas.json', json.dumps(out, indent=1, sort_keys=True).encode())
    print('cas-skate2:', len(out['bodies']), 'bodies,', len(out['cameras']), 'cameras, rooms',
          {k: round(math.degrees(v['yaw']), 2) for k, v in out['rooms'].items()})


if __name__ == '__main__':
    main()
