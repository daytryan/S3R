"""Export authored district DMO locators and their referenced model geometry.

This preserves initial placement, not DMO simulation. Native 0x825876D0 reads
the locator matrix and template ID at +112; RX2 EB001D records are 128 bytes.
No name matching, random placement, ground snapping or collision synthesis.
"""
import argparse
import copy
import hashlib
import json
import struct
import sys
import tempfile
from pathlib import Path

import numpy as np
from .map_writer import write
from tools.owned_game.big import BigArchive


def sections(raw):
    if raw[:7] != b'\x89RW4xb2':
        raise ValueError('Expected Xbox RX2')
    count = struct.unpack_from('>I', raw, 32)[0]
    table = struct.unpack_from('>I', raw, 48)[0]
    if table + count*24 > len(raw):
        raise ValueError('Truncated RX2 section table')
    return [struct.unpack_from('>6I', raw, table+i*24) for i in range(count)]


def matrix(raw, at):
    value = np.array(struct.unpack_from('>16f', raw, at)).reshape(4, 4)
    if not np.isfinite(value).all() or not np.allclose(value[:, 3], [0, 0, 0, 1], atol=1e-5):
        raise ValueError('Invalid DMO affine matrix')
    if abs(np.linalg.det(value[:3, :3])) < 1e-8:
        raise ValueError('Singular DMO matrix')
    return value


def records(raw, kind, stride):
    for offset, _, size, _, _, type_id in sections(raw):
        if type_id != kind:
            continue
        if offset+size > len(raw) or size < 32:
            raise ValueError('Invalid DMO section extent')
        count = struct.unpack_from('>I', raw, offset+4)[0]
        start, strings = struct.unpack_from('>2I', raw, offset+12)
        if start != 32 or strings != start+count*stride or strings > size:
            raise ValueError('Unexpected DMO record layout')
        for index in range(count):
            yield offset, offset+start+index*stride, size


def locators(raw):
    result = []
    for base, at, size in records(raw, 0xEB001D, 128):
        name_offset = struct.unpack_from('>I', raw, at+124)[0]
        if not 32 <= name_offset < size:
            raise ValueError('Invalid DMO locator name offset')
        end = raw.index(b'\0', base+name_offset, base+size)
        result.append(dict(instance_id=f'{struct.unpack_from(">Q", raw, at+96)[0]:016X}',
            locator_id=f'{struct.unpack_from(">Q", raw, at+104)[0]:016X}',
            template_id=f'{struct.unpack_from(">Q", raw, at+112)[0]:016X}',
            matrix=matrix(raw, at).tolist(), name=raw[base+name_offset:end].decode('utf-8'),
            source_offset=at, bounds=np.array(struct.unpack_from('>8f', raw, at+64)).reshape(2, 4)[:, :3].tolist()))
    return result


def template_meshes(raw):
    """Resolve EB000D tInstance -> EB0001 model -> EB0023 mesh -> declaration."""
    table = sections(raw)
    result = {}
    for _, at, _ in records(raw, 0xEB000D, 160):
        key = f'{struct.unpack_from(">Q", raw, at+104)[0]:016X}'
        index = struct.unpack_from('>I', raw, at+128)[0]
        if index >= len(table) or table[index][5] != 0xEB0001:
            raise ValueError('Unresolved DMO model reference')
        base, _, size, _, _, _ = table[index]
        start = struct.unpack_from('>I', raw, base+36)[0]
        count = struct.unpack_from('>H', raw, base+48)[0]
        if base+size > len(raw) or start+count*8 > size:
            raise ValueError('Invalid DMO model mesh table')
        mesh_info = []
        for j in range(count):
            mesh_index = struct.unpack_from('>I', raw, base+start+j*8)[0]
            if mesh_index >= len(table) or table[mesh_index][5] != 0xEB0023:
                raise ValueError('Unresolved DMO mesh reference')
            descriptor = struct.unpack_from('>I', raw, table[mesh_index][0]+40)[0]
            if descriptor >= len(table) or table[descriptor][5] != 0x200E9:
                raise ValueError('Unresolved DMO vertex declaration')
            mesh_info.append(table[descriptor][0])
        if key in result:
            raise ValueError('Duplicate DMO template ID')
        result[key] = dict(mesh_info=mesh_info, matrix=matrix(raw, at),
                          model_matrix=matrix(raw, base+struct.unpack_from('>I', raw, base+32)[0]))
    return result


# A box's corners (x, y, z in -1/+1, x slowest) and its faces, wound outward.
BOX_CORNERS = np.array([[x, y, z] for x in (-1, 1) for y in (-1, 1) for z in (-1, 1)], dtype=np.float64)
BOX_FACES = np.array([(0, 1, 3), (0, 3, 2), (4, 6, 7), (4, 7, 5), (0, 4, 5), (0, 5, 1),
                      (2, 3, 7), (2, 7, 6), (0, 2, 6), (0, 6, 4), (1, 5, 7), (1, 7, 3)])


def capsule_rings(half_height, radius):
    """A capsule along Z as (radius, z) rings: hemisphere rings at 0/30/60/90 degrees, each half moved out by half_height."""
    rings = [(radius*np.sin(a), radius*np.cos(a)+half_height) for a in np.radians((0, 30, 60, 90))]
    return rings + [(rho, -z) for rho, z in reversed(rings)]


def lathe_triangles(rings, segments=8):
    """(radius, z) rings, top to bottom, turned about Z into triangles."""
    turn = 2*np.pi*np.arange(segments+1)/segments
    points = [np.stack([rho*np.cos(turn), rho*np.sin(turn), np.full(segments+1, z)], 1) for rho, z in rings]
    tris = np.array([q for a, b in zip(points, points[1:]) for k in range(segments)
                     for q in ((a[k], b[k], b[k+1]), (a[k], b[k+1], a[k+1]))])
    return tris[np.linalg.norm(np.cross(tris[:, 1]-tris[:, 0], tris[:, 2]-tris[:, 0]), axis=1) > 1e-12]


def volume_triangles(raw, table, at, decode_mesh, parent=np.eye(4)):
    """A RenderWare collision Volume (96 B at `at`: row-vector matrix, type +0x40, radius +0x50) as (n, 3, 3) model
    triangles. Types: 2 capsule along local Z (half height +0x44); 5 cylinder along local Z (half height +0x44, radius
    +0x48, grown by +0x50: oil drum); 4 box (half extents +0x44, grown by the radius);
    6 aggregate (+0x44 a record: ClusteredMesh 0x80006, via `decode_mesh`, or a volume list 0x80002, count +0x28,
    its 96 B volumes from +0x30). Anything else is refused. Capsule axis: primitive_mass.rs; field order: the RW
    Volume layout, INFERRED from the disc values (legs 2.6 cm thick)."""
    m = np.array(struct.unpack_from('>16f', raw, at), dtype=np.float64).reshape(4, 4)
    m[:, 3] = (0, 0, 0, 1)
    world = m @ parent
    kind, ref = struct.unpack_from('>2I', raw, at+0x40)
    radius, = struct.unpack_from('>f', raw, at+0x50)
    if kind == 2:
        tris = lathe_triangles(capsule_rings(struct.unpack_from('>f', raw, at+0x44)[0], radius))
    elif kind == 5:
        # ponytail: a fat cylinder becomes a plain one grown by its fatness; rounded rims stick out by r(sqrt2-1).
        half_height, rho = np.array(struct.unpack_from('>2f', raw, at+0x44))+radius
        tris = lathe_triangles([(0, half_height), (rho, half_height), (rho, -half_height), (0, -half_height)])
    elif kind == 4:
        # ponytail: a rounded box (radius > 0) becomes a box grown by the radius; corners stick out by r(sqrt3-1).
        tris = (BOX_CORNERS*(np.array(struct.unpack_from('>3f', raw, at+0x44))+radius))[BOX_FACES]
    elif kind == 6 and ref < len(table) and table[ref][5] == 0x80006:
        tris = np.asarray(decode_mesh(raw[table[ref][0]:table[ref][0]+table[ref][2]]), dtype=np.float64).reshape(-1, 3, 3)
    elif kind == 6 and ref < len(table) and table[ref][5] == 0x80002:
        base = table[ref][0]
        count, = struct.unpack_from('>I', raw, base+40)
        first, = struct.unpack_from('>I', raw, base+48)
        return np.concatenate([volume_triangles(raw, table, base+first+96*k, decode_mesh, world) for k in range(count)])
    else:
        raise ValueError(f'Unsupported DMO collision volume type {kind}')
    if kind != 6:
        # Wind each face away from the primitive's centre (both shapes are convex about their origin).
        n = np.cross(tris[:, 1]-tris[:, 0], tris[:, 2]-tris[:, 0])
        flip = np.einsum('ij,ij->i', n, tris.mean(1)) < 0
        tris[flip] = tris[flip][:, [0, 2, 1]]
    return tris @ world[:3, :3] + world[3, :3]


def template_collision(raw, decode_mesh, wanted=None):
    """Per DMO template id (those in `wanted`, default all), its collision as (n, 3, 3) model triangles: the EB000D
    tInstance row (template +104, model EB0001 +128) names its EB000A at +132, whose body (+8, 0x40006) holds its
    Volume (+0x70, 0x80001). Skate 2 cPres_Global, round2 T4."""
    table = sections(raw)
    result = {}
    for _, at, _ in records(raw, 0xEB000D, 160):
        template = f'{struct.unpack_from(">Q", raw, at+104)[0]:016X}'
        if wanted is None or template in wanted:
            index, = struct.unpack_from('>I', raw, at+132)
            if index >= len(table) or table[index][5] != 0xEB000A:
                raise ValueError('Unresolved DMO collision reference')
            body, = struct.unpack_from('>I', raw, table[index][0]+8)
            if body >= len(table) or table[body][5] != 0x40006:
                raise ValueError('Unresolved DMO collision body')
            volume, = struct.unpack_from('>I', raw, table[body][0]+0x70)
            if volume >= len(table) or table[volume][5] != 0x80001:
                raise ValueError('Unresolved DMO collision volume')
            if template in result:
                raise ValueError('Two collisions for DMO template '+template)
            result[template] = volume_triangles(raw, table, table[volume][0], decode_mesh)
    return result


def template_grabs(raw):
    """Template ids with GRABDATA: an EB001F among the records from the EB000D row's model (+128) to its EB000A (+132).
    INFERRED: grouping by that span; Skate 2's wood panel (no EB001F) is the one prop retail never lets you grab."""
    table = sections(raw)
    found = set()
    for _, at, _ in records(raw, 0xEB000D, 160):
        model, collision = struct.unpack_from('>2I', raw, at+128)
        if any(table[i][5] == 0xEB001F for i in range(model, min(collision, len(table)))):
            found.add(f'{struct.unpack_from(">Q", raw, at+104)[0]:016X}')
    return found


def transform_mesh(arrays, index, transform):
    # NpzFile.items() reads every array before the filter runs.
    values = {key: np.array(arrays[key], copy=True) for key in arrays if key.endswith('_'+str(index))}
    key = f'vertices_{index}'
    values[key] = (values[key] @ transform[:3, :3] + transform[3, :3]).astype('f4')
    normal_matrix = np.linalg.inv(transform[:3, :3]).T
    for prefix in ('normals', 'retail_normals'):
        key = f'{prefix}_{index}'
        if key in values:
            n = values[key] @ normal_matrix
            values[key] = (n/np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-20)).astype('f4')
    if np.linalg.det(transform[:3, :3]) < 0:
        values[f'faces_{index}'] = values[f'faces_{index}'][:, [0, 2, 1]]
    return values


def catalog(cache_roots):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'vendor/utt'))
    import rx2_parser
    from .backdrop import texture_groups
    templates, textures = {}, {}
    for root in cache_roots:
        m = json.loads((root/'manifest.json').read_text())
        for key, value in m['textures'].items():
            entry = dict(value)
            for path_key in ('rgba', 'png'):
                if path_key in entry:
                    entry[path_key] = str((root/entry[path_key]).resolve())
            if key in textures and Path(textures[key]['rgba']).read_bytes() != Path(entry['rgba']).read_bytes():
                raise ValueError('Conflicting DMO texture '+key)
            textures[key] = entry
        for model in m['models']:
            raw = (root/model['rx2']).read_bytes()
            table = rx2_parser.RX2File(raw)
            table.parse()
            roles = set(role for mesh in model['meshes'] for role in mesh['retail_texture_ids'])
            bindings = texture_groups(raw, table, roles)
            for key, definition in template_meshes(raw).items():
                meshes = [copy.deepcopy(mesh) for mesh in model['meshes'] if mesh['source_offsets']['mesh_info'] in definition['mesh_info']]
                for mesh in meshes:
                    # DMO resources have a high-bit namespace that display names
                    # omit. Bind the binary channel GUID; never strip that bit.
                    group = bindings[mesh['retail_material_group_index']]
                    mesh['retail_texture_ids'] = {role:f'0x{group[role]:016x}' for role in mesh['retail_texture_ids']}
                    mesh['texture_id'] = mesh['retail_texture_ids'].get('diffuse', mesh['retail_texture_ids'].get('transparent'))
                if len(meshes) != len(definition['mesh_info']):
                    raise ValueError('DMO model geometry omitted '+key)
                if key in templates:
                    raise ValueError('Conflicting DMO template '+key)
                templates[key] = dict(definition, meshes=meshes, npz=root/model['npz'], asset_id=model['asset_id'])
    return templates, textures


def save_catalog(cache_roots, output):
    """Build once per installation; JSON keeps this local cache non-executable."""
    templates, textures = catalog(cache_roots)
    serialised = {key: dict(value, matrix=value['matrix'].tolist(),
        model_matrix=value['model_matrix'].tolist(), npz=str(value['npz'].resolve()))
        for key, value in templates.items()}
    temporary = output.with_suffix('.new')
    temporary.write_text(json.dumps(dict(version=1, templates=serialised, textures=textures)), encoding='utf-8')
    temporary.replace(output)


def load_catalog(path):
    data = json.loads(path.read_text(encoding='utf-8'))
    if data['version'] != 1:
        raise ValueError('Unsupported DMO catalog version')
    for value in data['templates'].values():
        value['matrix'] = np.array(value['matrix'], dtype=np.float64)
        value['model_matrix'] = np.array(value['model_matrix'], dtype=np.float64)
        value['npz'] = Path(value['npz'])
    return data['templates'], data['textures']


def export(manifest_path, cache_roots, output, *, catalog_path=None):
    district = json.loads(manifest_path.read_text())
    templates, textures = catalog(cache_roots) if catalog_path is None else load_catalog(catalog_path)
    placements = {}
    for source in district['simulation_assets']:
        raw = (manifest_path.parent/source['rx2']).read_bytes()
        for item in locators(raw):
            item['source_asset'] = source['asset_id']
            item['source_sha256'] = hashlib.sha256(raw).hexdigest()
            key = item['instance_id']
            if key in placements and placements[key] != item:
                raise ValueError('Conflicting DMO locator '+key)
            placements[key] = item
    return write_package(district['map_name'], district['district_name'], placements.values(), templates, textures,
                         output)


def write_package(map_name, district_name, placements, templates, textures, output, *, annotate=None):
    """Render-only `output` .skate of each placement's template model at its matrix, plus `output`.json listing them
    (each with `annotate(item, transform)`'s fields when given). Returns (placed, unresolved)."""
    report = dict(map=map_name, instances=[], unresolved=[], simulation='initial placement only')
    with tempfile.TemporaryDirectory(prefix='skate-dmo-') as work:
        root = Path(work); models = []; used = set()
        for number, item in enumerate(placements):
            template = templates.get(item['template_id'])
            if template is None:
                report['unresolved'].append(item)
                continue
            transform = template['model_matrix'] @ template['matrix'] @ np.array(item['matrix'])
            arrays = {}; meshes = copy.deepcopy(template['meshes'])
            with np.load(template['npz'], allow_pickle=False) as original:
                for mesh in meshes:
                    arrays.update(transform_mesh(original, mesh['index'], transform))
                    used.update(mesh['retail_texture_ids'].values())
            path = f'{number}.npz'; np.savez(root/path, **arrays)
            models.append(dict(asset_id=item['instance_id'], meshes=meshes, npz=path))
            report['instances'].append(dict(item, model_asset=template['asset_id'], meshes=len(meshes),
                                            **(annotate(item, transform) if annotate else {})))
        if models:
            manifest = dict(map_name=map_name, district_name=district_name,
                models=models, textures={key:textures[key] for key in sorted(used)},
                normal_texture_policy=dict(excluded_texture_ids=[]), grind_splines=[])
            (root/'manifest.json').write_text(json.dumps(manifest))
            # Publish only a complete package; preserve the previous one on failure.
            temporary = output.with_suffix('.skate.new')
            try:
                write(root/'manifest.json', temporary, None, render_only=True)
                temporary.replace(output)
            finally:
                temporary.unlink(missing_ok=True)
        else:
            output.unlink(missing_ok=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.with_suffix('.json').write_text(json.dumps(report, indent=2))
    return len(report['instances']), len(report['unresolved'])


def prepare_catalog(game_root, work):
    vendor = Path(__file__).resolve().parents[1]/'vendor'
    sys.path.insert(0, str(vendor/'university/tools/vanilla_map_extraction/tools'))
    from prepare_hawaiian_dream import prepare
    archive = BigArchive(game_root/'data/content/worlddmo.big')
    archive.extract_entries(archive.entries, work/'raw')
    roots = []
    for stream in (work/'raw/data/content/world/dmo').iterdir():
        root = work/'cache'/stream.name
        prepare(stream_directory=stream, output_root=root, utt_root=vendor/'utt',
                district_name=stream.name, map_name=stream.name, raw_texture_cache=True)
        roots.append(root)
    save_catalog(roots, work/'catalog.json')
    return roots


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--manifest', type=Path, required=True)
    p.add_argument('--cache', type=Path, nargs='+', required=True)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    print('DMO placed/unresolved:', export(a.manifest, a.cache, a.output))
