"""Drop Skate 2's trigger-volume collision from a map convert_ps3.py made before it left them out (build_archive's
drop_triggers): retail rides through them (round2-research R7). Matches by the untouched section bytes.

usage: py strip_trigger_collision.py REPO SIM_DIR MAP.skate [--write]
  SIM_DIR  the district stream folder the map's Sim stream came from (stages.py MOD_MAPS: --sim, else --pres)
  --write  replace MAP.skate (the original kept as MAP.skate.pre-triggers) and refresh its sha256 in maps.json
           (kept as maps.json.pre-triggers); without it, only lists what would go.
"""
import hashlib, json, os, struct, sys, zlib
from pathlib import Path

repo, sim, target = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
write = '--write' in sys.argv[4:]
sys.path[:0] = [str(Path(__file__).resolve().parent), str(repo / 'tools/vendor/utt'),
                str(repo / 'tools/vendor/university/tools/vanilla_map_extraction/tools')]
import ps3_streams
from build_retail_collision_archive import MAGIC, collision_sections, trigger_sections
from embed_retail_collision_archive import embed_archive
from export_skate2_skybox import _find_extension_offset, _read_extensions

triggers = set()
for asset in ps3_streams.load_district_stream(sim, 'Sim', sim.name):
    if asset.record.asset_type == 0x1:  # PS3 simulation; convert_ps3 gives it the Xbox RW4 header
        rx2 = b'\x89RW4xb2' + asset.data[7:]
        sections = collision_sections(rx2)
        triggers.update(sections[i] for i in trigger_sections(rx2))

(record,) = [r for tag, r in _read_extensions(target, _find_extension_offset(target)) if tag == b'RWCM']
method, size = struct.unpack_from('<II', record, 12)
archive = record[20:20 + size]
archive = zlib.decompress(archive) if method == 1 else archive
assert archive[:8] == MAGIC
count, at, kept, dropped = struct.unpack_from('<I', archive, 8)[0], 12, [], []
for index in range(count):
    (n,) = struct.unpack_from('<I', archive, at)
    name = archive[at + 4:at + 4 + n].decode()
    (m,) = struct.unpack_from('<I', archive, at + 4 + n)
    mesh = archive[at + 8 + n:at + 8 + n + m]
    at += 8 + n + m
    (dropped if mesh in triggers else kept).append((index, name, mesh))
assert at == len(archive)
print(f'{len(triggers)} trigger sections in {sim.name}; {len(dropped)} of {count} archive meshes are triggers:')
for index, name, _ in dropped:
    print(f'  #{index} {name}')
if not write or not dropped:
    sys.exit(0)

out = bytearray(MAGIC + struct.pack('<I', len(kept)))
for _, name, mesh in kept:
    out += struct.pack('<I', len(name.encode())) + name.encode() + struct.pack('<I', len(mesh)) + mesh
staged = target.with_name(target.name + '.rwcmset')
staged.write_bytes(out)
fresh = target.with_name(target.name + '.new')
print(json.dumps(embed_archive(target, staged, fresh)))
staged.unlink()
backup = target.with_name(target.name + '.pre-triggers')
if backup.exists():
    os.replace(fresh, target)  # the earlier run's backup stays the original
else:
    os.replace(target, backup)
    os.replace(fresh, target)
catalog = target.parent.parent / 'maps.json'
text = catalog.read_bytes()
if not catalog.with_name('maps.json.pre-triggers').exists():
    catalog.with_name('maps.json.pre-triggers').write_bytes(text)
maps = json.loads(text)
old = next(e for e in (maps if isinstance(maps, list) else maps['maps']) if e.get('path') == f'maps/{target.name}')['sha256']
digest = hashlib.sha256()
with target.open('rb') as f:
    for block in iter(lambda: f.read(1 << 24), b''):
        digest.update(block)
assert text.count(old.encode()) == 1
catalog.write_bytes(text.replace(old.encode(), digest.hexdigest().encode()))
print(f'{target.name}: {len(dropped)} trigger meshes dropped, sha256 {digest.hexdigest()}')
