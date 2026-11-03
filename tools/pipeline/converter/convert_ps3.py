"""Convert a Skate 3 PS3 district stream (e.g. the RPCS3 New San Van mod) into a Skate3Rust .skate map.

Runs the engine's own Xbox map pipeline (prepare -> collision archive -> map writer ->
game load check) with the PS3 readers patched in:
  * .pst/.psf streams with 72-byte ATOC records and PS3 payload packing (ps3_streams)
  * CellGcm textures (ps3_textures)
  * RW4ps3 models via the vendored mdl_parser PS3 path
  * RSX vertex elements translated to the Xenos descriptors the UV/frame decoders read
  * simulation arenas relabelled RW4xb2: their TOC and big-endian collision/grind data match

usage (needs numpy + Pillow):
  python convert_ps3.py --repo REPO --pres DIR [--sim DIR] --name NAME --install INSTALL_DIR --game-exe EXE
  then the same with --check: the game's load check of the written map
"""
import argparse, dataclasses, hashlib, json, shutil, struct, subprocess, sys, time
from collections import Counter, namedtuple
from pathlib import Path

HERE = Path(__file__).resolve().parent
p = argparse.ArgumentParser()
p.add_argument('--repo', type=Path, required=True)
p.add_argument('--pres', type=Path, required=True, help='district stream folder with the presentation (Pres) stream')
p.add_argument('--sim', type=Path, help='district stream folder with the Sim stream (default: --pres)')
p.add_argument('--name', required=True, help='output map name, e.g. New_San_Vanelona')
p.add_argument('--install', type=Path, required=True, help='data/installations/<id> folder')
p.add_argument('--game-exe', type=Path, required=True)
p.add_argument('--work', type=Path, default=HERE / 'work')
p.add_argument('--lightmap-downscale', type=int, default=1,
               help='divide lightmap page resolution (the runtime uploads them as RGBA16F: 8 bytes/pixel)')
p.add_argument('--check', action='store_true',
               help="only the game's load check, in its own process: the converter's memory is gone by then")
a = p.parse_args()
a.sim = a.sim or a.pres

sys.path.insert(0, str(a.repo))
tools = a.repo / 'tools/vendor/university/tools/vanilla_map_extraction/tools'
sys.path[:0] = [str(HERE), str(a.repo / 'tools/vendor/utt'), str(tools)]
import mdl_parser, rx2_parser, prepare_hawaiian_dream as prep, retail_lightmap_uv as uv
import ps3_streams, ps3_textures
from skate3_streams import ASSET_TYPE_MODEL, ASSET_TYPE_SIMULATION, ASSET_TYPE_TEXTURE, StreamAsset
from build_retail_collision_archive import build_archive
from tools.asset_pipeline.map_writer import write as write_map
from tools.asset_pipeline.backdrop import texture_groups
from tools.asset_pipeline.retail_material import TERRAIN_CHANNELS, TERRAIN_FRAME_SHADERS, TERRAIN_SCALARS, terrain_scalar
import numpy as np
import retail_collision_mesh as rcm
import ctypes
from concurrent.futures import ThreadPoolExecutor
from tools import native

LIGHTMAP = native.load('lightmap', __file__)  # lightmap_native.rs: downscale_lightmaps' filter, the same bytes
if LIGHTMAP:
    LIGHTMAP.skate_box_downscale.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t,
                                             ctypes.c_void_p]

# --- collision: 16-bit cluster offsets are unsigned (runtime GetVertex, docs/SKATE-MAPS.md).
# The vendored decoder sign-extends them, displacing vertices by 65536 * granularity; the
# spawn search reads these triangles, so decode them the way the game does.
_decode_vertices = rcm._decode_vertices


def decode_vertices(cluster, vertex_count, compression, granularity, vertex_data_end):
    if compression != 1:
        return _decode_vertices(cluster, vertex_count, compression, granularity, vertex_data_end)
    base = np.array(struct.unpack_from('>3i', cluster, rcm.CLUSTER_HEADER_SIZE), np.int64)
    if rcm.CLUSTER_HEADER_SIZE + 12 + vertex_count * 6 > vertex_data_end:
        raise ValueError('16-bit compressed vertex extends into the unit stream')
    delta = np.frombuffer(cluster, '>u2', vertex_count * 3, rcm.CLUSTER_HEADER_SIZE + 12).reshape(-1, 3)
    values = np.clip(base + delta, -2**31, 2**31 - 1).astype(np.float32) * np.float32(granularity)
    return [tuple(float(v) for v in row) for row in values]


rcm._decode_vertices = decode_vertices
rcm.UNSIGNED_16BIT_OFFSETS = True  # the same rule for the native decoder


class CentreSpawn:
    """Upward face nearest the centre of the map's collision (retail parks sit km from the origin)."""

    def __init__(self):
        self.triangles = []

    def consider(self, meshes):
        for mesh in meshes:
            if mesh.triangles:
                self.triangles.append(mesh.triangles.points)  # prepare decodes points_only

    def result(self, name):
        if not self.triangles:
            raise ValueError('No collision in ' + name)
        t = np.concatenate(self.triangles)
        a, b, c = t[:, 0], t[:, 1], t[:, 2]
        cross = np.cross(b - a, c - a)
        length = np.linalg.norm(cross, axis=1)
        points = ((a + b + c) / 3)[(length >= 4) & (cross[:, 1] >= .9 * length)]
        if not len(points):
            raise ValueError('No supported spawn surface in ' + name)
        flat = t.reshape(-1, 3)
        centre = (flat.min(0) + flat.max(0)) / 2
        point = points[np.argmin((points[:, 0] - centre[0]) ** 2 + (points[:, 2] - centre[2]) ** 2)].copy()
        point[1] += 1.
        return tuple(point)

# --- streams: PS3 RW4 types -> the Xbox ATOC type codes the preparer switches on
PS3_TYPES = {0x80: ASSET_TYPE_TEXTURE, 0x10: ASSET_TYPE_MODEL, 0x1: ASSET_TYPE_SIMULATION}
SOURCES = {'Pres': a.pres, 'Sim': a.sim, 'Tex': a.pres}  # Tex: separate texture stream (Skate 1 San Van)


def load_district_stream(_directory, stream_name, _district):
    root = SOURCES.get(stream_name)
    if root is None:
        return []
    out = []
    for asset in ps3_streams.load_district_stream(root, stream_name, root.name):
        kind = PS3_TYPES.get(asset.record.asset_type, asset.record.asset_type)
        data = asset.data
        if kind == ASSET_TYPE_SIMULATION:
            data = b'\x89RW4xb2' + data[7:]
        out.append(StreamAsset(dataclasses.replace(asset.record, asset_type=kind),
                               asset.source_path, asset.source_offset, asset.stored_size, data))
    index_textures([x for x in out if x.record.asset_type == ASSET_TYPE_TEXTURE])
    return out


prep.load_district_stream = load_district_stream

# --- texture binding by GUID. Materials reference textures by the GUID each texture arena
# exports (0x00EB000B), as the game does. The preparer matches by name instead, and the mod
# ships distinct lightmap pages under one name ([0x..]/[Rx..]/[Bx..]); name matching bound
# about half of them to the wrong (often black) page.
GUID_TEXTURE = {}    # exported texture GUID -> manifest texture id
UNIQUE_TEXTURE = {}  # asset id -> texture id, for names shared by different textures
_texture_id = prep._texture_id


def exported_guids(data):
    table = ps3_textures.PS3Texture(data)
    table.parse()
    guids = []
    for entry in table.entries:
        if entry.type_id == 0x00EB000B:
            count, start = struct.unpack_from('>2I', data, entry.f0)
            for i in range(count):
                _, _, hi, lo, _, _ = struct.unpack_from('>6I', data, entry.f0 + start + i * 24)
                guids.append(hi << 32 | lo)
    return guids


def index_textures(assets):
    by_name = {}
    for asset in assets:
        by_name.setdefault(_texture_id(asset.data, asset.record.asset_id), {}).setdefault(
            hashlib.sha256(asset.data).digest(), []).append(asset.record.asset_id)
    for name, versions in by_name.items():
        if len(versions) > 1:  # same name, different pixels: keep each one addressable
            for ids in versions.values():
                for aid in ids:
                    UNIQUE_TEXTURE[aid] = f'{name}_{ids[0]:016x}'
    for asset in assets:
        for guid in exported_guids(asset.data):
            GUID_TEXTURE[guid] = texture_id(asset.data, asset.record.asset_id)
    shared = sum(len(v) > 1 for v in by_name.values())
    print(f'indexed {len(GUID_TEXTURE)} texture GUIDs; {shared} names shared by different textures', flush=True)


def texture_id(data, asset_id):
    return UNIQUE_TEXTURE.get(asset_id) or _texture_id(data, asset_id)


prep._texture_id = texture_id

GROUP_GUIDS = {}  # id(bound material group) -> {role: texture GUID}
_bind = prep._bind_material_groups_by_guid


def bind(data, groups, mesh_count, **kw):
    bound, bindings = _bind(data, groups, mesh_count, **kw)
    GROUP_GUIDS.clear()
    if bytes(data[:7]) == ps3_textures.MAGIC_PS3:
        table = ps3_textures.PS3Texture(data)
        table.parse()
        channels = texture_groups(data, table, prep.RETAIL_TEXTURE_CHANNELS + TERRAIN_CHANNELS)
        for group, binding in zip(bound, bindings):
            GROUP_GUIDS[id(group)] = channels[binding['group_index']]
    return bound, bindings


prep._bind_material_groups_by_guid = bind
_metadata = prep._material_metadata


def material_metadata(groups, index):
    meta = _metadata(groups, index)
    guids = GROUP_GUIDS.get(id(groups[index])) if index < len(groups) else None
    for role, guid in (guids or {}).items():
        if role in TERRAIN_SCALARS:
            meta['retail_parameters'][role] = [terrain_scalar(guid)]
        elif guid in GUID_TEXTURE:
            meta['retail_texture_ids'][role] = GUID_TEXTURE[guid]
    channel = meta['texture_channel']
    if guids and guids.get(channel) in GUID_TEXTURE:
        meta['texture_id'] = GUID_TEXTURE[guids[channel]]
    return meta


prep._material_metadata = material_metadata
# Skate 2 terrain carries the exact retail normal and tangent frame too (retail_material.TERRAIN_FRAME_SHADERS).
prep.RETAIL_WORLD_FRAME_SHADER_PREFIXES += TERRAIN_FRAME_SHADERS

# --- textures and models
_parse_rx2 = rx2_parser.parse_rx2
rx2_parser.parse_rx2 = lambda d: ps3_textures.parse(d) if bytes(d[:7]) == ps3_textures.MAGIC_PS3 else _parse_rx2(d)
mdl_parser.parse_rx2 = mdl_parser.parse_model

from ps3_vertex import xenos

for fn in ('decode_lightmap_uvs', 'decode_decal_uvs', 'decode_retail_world_frame'):
    original = getattr(uv, fn)
    setattr(prep, fn, lambda data, *, attributes, _f=original, **kw: _f(data, attributes=xenos(attributes), **kw))


def prune_missing(m):
    """Drop texture references the mod does not ship (the base PS3 game supplies them).

    Mostly Skate 2 chromaticity pages, which the runtime never samples; a few shared
    detail/specular/diffuse textures. A mesh without its diffuse texture renders untextured.
    """
    dropped = Counter()
    for model in m['models']:
        for mesh in model['meshes']:
            roles = mesh.get('retail_texture_ids') or {}
            for role in [r for r, t in roles.items() if t and t not in m['textures']]:
                del roles[role]; dropped[role] += 1
            if mesh.get('texture_id') and mesh['texture_id'] not in m['textures']:
                mesh['texture_id'] = None; dropped['untextured mesh'] += 1
    print(f'dropped texture references not in the mod: {dict(dropped)}', flush=True)


def downscale_lightmaps(m, root, factor):
    """Box-filter lightmap pages so a whole-city map fits a 4 GB GPU (New San Van: ~2.4 GB of lightmaps at 1x)."""
    if factor <= 1:
        return
    pages = {t for model in m['models'] for mesh in model['meshes']
             for role, t in (mesh.get('retail_texture_ids') or {}).items() if role == 'lightmap'}
    entries = [e for t in pages if (e := m['textures'].get(t))
               and e.get('cube_faces', 1) == 1 and min(e['width'], e['height']) >= 2 * factor]

    def shrink(e):
        w, h = e['width'], e['height']
        w2, h2 = w // factor, h // factor
        path = root / e['rgba']
        px = np.frombuffer(path.read_bytes(), np.uint8).reshape(h, w, 4)
        out = np.empty(w2 * h2 * 4, np.uint8)
        if not LIGHTMAP or LIGHTMAP.skate_box_downscale(px.ctypes.data, w, h, factor, out.ctypes.data):
            out = np.round(px.astype(np.uint16)[:h2 * factor, :w2 * factor].reshape(h2, factor, w2, factor, 4).mean((1, 3)))
        path.write_bytes(out.astype(np.uint8).tobytes())

    with ThreadPoolExecutor(4) as pool:  # file reads, writes and the native filter all let go of the GIL
        list(pool.map(shrink, entries))
    before = after = 0
    for e in entries:
        w, h = e['width'], e['height']
        e['width'], e['height'] = w // factor, h // factor
        before += w * h; after += (w // factor) * (h // factor)
    print(f'lightmaps downscaled {factor}x: {before * 8 / 2**30:.2f} GB -> {after * 8 / 2**30:.2f} GB as RGBA16F', flush=True)


def run():
    work = a.work / a.name
    final = a.install / 'maps' / (a.name + '.skate')
    if a.check:
        return check(work, final)
    shutil.rmtree(work, ignore_errors=True)
    t0 = time.perf_counter()
    district = a.pres.name
    spawn = CentreSpawn()
    manifest = prep.prepare(
        stream_directory=a.pres, output_root=work / 'intermediate', utt_root=a.repo / 'tools/vendor/utt',
        district_name=district, map_name=a.name, package_name='Skate 3 PS3 stream', cache_format='skate3-rust-map-v1',
        allow_material_import_order_fallback=True, raw_texture_cache=True,
        texture_stream_names=('Tex',) if any(a.pres.glob('cTex_*.psf')) else (),
        collision_consumer=spawn.consider, write_render_sources=False)
    print(f'prepare {time.perf_counter() - t0:.1f}s', flush=True)
    m = json.loads(manifest.read_text())  # one read and one write of the (large) manifest for both edits
    prune_missing(m)
    downscale_lightmaps(m, manifest.parent, a.lightmap_downscale)
    manifest.write_text(json.dumps(m))
    del m  # build_archive and write_map read the file, not this dict
    collision = work / 'collision.rwcmset'
    # Skate 2's trigger volumes are not solid (retail rides through them).
    print(json.dumps(build_archive(manifest, collision, drop_triggers=True)), flush=True)
    start = spawn.result(a.name)
    del spawn  # its collision points: ~300 MB on Skate 1, freed before write_map's buffers
    write_map(manifest, final, collision, lambda s: print(s, flush=True), prepared_spawn=start)
    print(f'written {final} {final.stat().st_size} bytes, {time.perf_counter() - t0:.1f}s', flush=True)


def check(work, final):
    check = subprocess.run([str(a.game_exe), '--assets', str(a.install / 'assets'), '--map', str(final), '--check-assets'],
                           capture_output=True, text=True, errors='replace')
    log = check.stdout + check.stderr
    (a.install / f'{a.name}-load.log').write_text(log, encoding='utf-8')
    print('\n'.join(l for l in log.splitlines() if 'SKATE_MAP_LOADED' in l or 'READY' in l or 'rror' in l))
    if check.returncode:
        sys.exit(f'game load check failed ({check.returncode}); see {a.name}-load.log')
    shutil.rmtree(work, ignore_errors=True)


if __name__ == '__main__':
    run()
