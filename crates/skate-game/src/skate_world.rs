//! Adapter for authored .skate world geometry. Does not replace controllers.
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use skate_core::{
    math::Vector3,
    physics::{
        board_world::{
            BoardWorld, WorldTriangle,
            query_metadata::{Bounds, QueryMesh, QueryMetadata, QueryPool},
        },
        collision::TriangleFeature,
        contact::RetailContactMaterial,
        drive_frames::RetailAffineTransform,
    },
};
use skate_data::skate_map::SkateMap;
use std::collections::HashMap;

#[path = "lightmap_atlas.rs"]
mod lightmap_atlas;

#[path = "retail_shadow_geometry.rs"]
pub(crate) mod shadow_geometry;

pub(crate) fn validate_runtime(map: &SkateMap) -> Result<(), String> {
    let _span = info_span!("validate_map").entered();
    let archive = retail_archive(map)?;
    if map.geometry.collision.is_empty() && archive.is_none() {
        return Err("SKATE map has no collision geometry".into());
    }
    for triangle in map.geometry.collision.iter().filter(|_| archive.is_none()) {
        if let Some(edges) = triangle.native_edges {
            decode_native_edges(edges)?;
        }
    }
    if !map.doors.is_empty() {
        return Err(format!(
            "Map '{}' contains {} hinged doors. This imported game has no door body/controller adapter yet; refusing to drop their geometry or turn them into static walls.",
            map.name,
            map.doors.len()
        ));
    }
    for extension in &map.extensions {
        let tag = String::from_utf8_lossy(&extension.tag);
        if extension.tag == *b"RWCM" {
            continue;
        }
        if extension.tag == *b"MOBJ" {
            skate_data::skate_map::validate_static_objects(map, extension)?;
            continue;
        }
        if extension.tag == *b"SKYB" && extension.schema == 1 {
            eprintln!(
                "SKATE LIMITATION: SKYB retail sky retained; using the map horizon until its shader adapter is available."
            );
            continue;
        }
        if extension.tag != *b"WMET" && extension.tag != *b"WCFG" && extension.tag != *b"BMAT" {
            return Err(format!(
                "SKATE extension {tag} schema {} is decoded but its runtime adapter is not implemented. Refusing to silently omit potentially required world geometry.",
                extension.schema
            ));
        }
        eprintln!(
            "SKATE LIMITATION: {tag} extension retained; its runtime behavior is not connected."
        );
    }
    if map.materials.iter().any(|m| m.retail_definition.is_some()) {
        eprintln!(
            "SKATE_RENDER: retail world material adapter enabled; unsupported families retain portable PBR rendering."
        );
    }
    if map.textures.iter().any(|t| t.width == 0) {
        return Err(
            "SKATE contains external texture placeholders; supply a package with embedded textures"
                .into(),
        );
    }
    if !map.routes.is_empty() {
        eprintln!(
            "SKATE LIMITATION: {} NPC routes parsed; supplied game has no NPC controller.",
            map.routes.len()
        );
    }
    if map.lights.iter().any(|l| l.kind == 2) {
        eprintln!(
            "SKATE LIMITATION: area-light records retained; Bevy adapter currently renders point and spot lights only."
        );
    }
    eprintln!(
        "SKATE LIMITATION: native frame-lighting/day-night controller is not connected; using package lighting and retail district sky where available."
    );
    eprintln!(
        "SKATE_MAP_LOADED name={:?} version={} render_triangles={} collision_triangles={} textures={} spawn={:?}",
        map.name,
        map.version,
        map.geometry.indices.len() / 3,
        map.geometry.collision.len(),
        map.textures.len(),
        map.spawn
    );
    Ok(())
}

/// TU3 ClusteredMesh::GetUnitVolumes (82AC8A68): fdivs then fsubs,
/// using the pi-squared word at 822F88D0. This is not acos/angle decoding.
/// Bit 7 denotes an unmatched compiler edge and is not a triangle flag.
pub(crate) fn decode_native_edges(edges: [u8; 3]) -> Result<(u32, [f32; 3]), String> {
    let mut flags = 1 | TriangleFeature::ONE_SIDED | TriangleFeature::USE_EDGE_COSINES;
    let mut cosines = [0.; 3];
    for (i, code) in edges.into_iter().enumerate() {
        let exponent = code & 0x1f;
        // The native signed 32-bit shift becomes negative at 28 and zero
        // above it. Reject those malformed codes instead of producing NaNs.
        if exponent >= 28 {
            return Err(format!(
                "Invalid SKATE native edge angle code {exponent} at corner {i}"
            ));
        }
        cosines[i] = 1.0 - f32::from_bits(0x411d_e9e7) / ((8_u32 << exponent) as f32);
        flags |= u32::from(code & 0x20) << i;
        flags |= u32::from(code & 0x40) << (i + 3);
    }
    Ok((flags, cosines))
}

fn retail_archive(map: &SkateMap) -> Result<Option<&[u8]>, String> {
    let mut archives = map.extensions.iter().filter(|e| e.tag == *b"RWCM");
    let Some(archive) = archives.next() else {
        return Ok(None);
    };
    if archive.schema != 1 || archives.next().is_some() {
        return Err("SKATE requires one RWCM extension with schema 1".into());
    }
    Ok(Some(&archive.payload))
}

fn retail_collision_world(
    archive: &[u8],
    material: RetailContactMaterial,
) -> Result<BoardWorld, String> {
    // User 10-27 (Bump Riser's giant invisible wall): retail's trigger volumes are no collision.
    let stripped = skate_data::retail_collision::without_triggers(archive)?;
    let (archive, triggers) = stripped.as_ref().map_or((archive, 0), |(kept, n)| (kept.as_slice(), *n));
    // perf 10-18: clusters convert on the load workers a batch at a time. SKATE_PARALLEL_COLLISION=0: one by one.
    let batch = if crate::config::perf_on("SKATE_PARALLEL_COLLISION") { COLLISION_BATCH } else { 0 };
    let (built, count) = retail_collision_parts(archive, material, batch)?;
    let sealed = sealed_faces(&built.triangles);
    let dropped = sealed.iter().filter(|&&s| s).count();
    let built = if dropped == 0 { built } else { built.without(&sealed) };
    eprintln!(
        "SKATE_RWCM_READY triangles={count} sealed={dropped} triggers={triggers} query_clusters={} source=embedded",
        built.meshes.len()
    );
    BoardWorld::with_query_metadata(
        built.triangles,
        QueryMetadata {
            packed_surfaces: built.packed_surfaces,
            meshes: built.meshes,
            static_edges: vec![],
            island_flags: 0,
        },
    )
    .map_err(str::to_owned)
}

/// Bug #25/#26 (10-25): faces that face into solid geometry. A one-sided wall whose top edge is shared with a floor
/// lying on the wall's FRONT side, the wall below that floor, and every point just in front of the wall sealed under
/// a floor (straight up meets a floor's underside, straight down meets no floor). Nothing reaches such a face from
/// its front without going through the floor, yet a truck capsule riding that floor gets a speculative face contact
/// at the shared edge whenever its axis is a little off the edge (the refine in `contact_points` then loses to the
/// face normal): the invisible wall beside University's binoculars and along flush grind edges.
/// INFERRED, collision-report.md: retail keeps these faces in its archive; how retail avoids the stop is untraced.
fn sealed_faces(triangles: &[WorldTriangle]) -> Vec<bool> {
    use bevy::platform::collections::{HashMap, HashSet};
    const FLOOR: f32 = 0.7; // normal y of a floor
    const GAP: f32 = 0.05; // metres a floor must lie in front of the wall, and the wall below the floor
    const CELL: f32 = 4.0;
    let key = |p: Vector3| [p.x, p.y, p.z].map(|v| (v * 500.).round() as i32); // 2 mm
    let edge = |a: Vector3, b: Vector3| {
        let (a, b) = (key(a), key(b));
        if a <= b { (a, b) } else { (b, a) }
    };
    let dot = |a: Vector3, b: Vector3| a.x * b.x + a.y * b.y + a.z * b.z;
    let sub = |a: Vector3, b: Vector3| Vector3::new(a.x - b.x, a.y - b.y, a.z - b.z);
    let mut sealed = vec![false; triangles.len()];
    // perf: the passes over every triangle run on the load workers, a run of indices each, in order.
    let runs: Vec<_> = (0..triangles.len()).step_by(1 << 16).map(|s| s..(s + (1 << 16)).min(triangles.len())).collect();
    // Top edges of one-sided walls (the opposite corner lies below the edge), sorted for lookups: a hash map's
    // single-threaded build cost University 90 ms, the sort 20.
    let mut tops = on_workers(&runs, |run| {
        let mut found = Vec::new();
        for i in run.clone() {
            let (v, f) = (triangles[i].vertices, triangles[i].triangle().feature);
            if f.flags & TriangleFeature::ONE_SIDED == 0 || f.normal.y > FLOOR {
                continue;
            }
            for o in 0..3 {
                let (a, b) = (v[(o + 1) % 3], v[(o + 2) % 3]);
                if v[o].y < a.y.min(b.y) - GAP {
                    found.push((edge(a, b), i, o));
                }
            }
        }
        found
    })
    .concat();
    tops.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut candidates = on_workers(&runs, |run| {
        let mut found = Vec::new();
        for t in &triangles[run.clone()] {
            let (v, n) = (t.vertices, t.triangle().feature.normal);
            if n.y <= FLOOR {
                continue;
            }
            for o in 0..3 {
                let a = v[(o + 1) % 3];
                let key = edge(a, v[(o + 2) % 3]);
                let first = tops.partition_point(|w| w.0 < key);
                for &(_, i, wo) in tops[first..].iter().take_while(|w| w.0 == key) {
                    let wall = triangles[i].triangle();
                    if dot(sub(v[o], a), wall.feature.normal) > GAP && dot(sub(wall.vertices[wo], a), n) < -GAP {
                        found.push(i);
                    }
                }
            }
        }
        found
    })
    .concat();
    candidates.sort_unstable();
    candidates.dedup();
    if candidates.is_empty() {
        return sealed;
    }
    // Points 2 cm in front of each candidate, and the triangles over their 4 m cells.
    let cell = |p: Vector3| ((p.x / CELL).floor() as i32, (p.z / CELL).floor() as i32);
    let samples: Vec<[Vector3; 6]> = candidates
        .iter()
        .map(|&i| {
            let ([p, q, r], n) = (triangles[i].vertices, triangles[i].triangle().feature.normal);
            let (e, f) = (sub(q, p), sub(r, p));
            [(0.15, 0.15), (0.15, 0.4), (0.15, 0.7), (0.4, 0.15), (0.4, 0.4), (0.7, 0.15)].map(|(u, w)| {
                Vector3::new(
                    p.x + u * e.x + w * f.x + 0.02 * n.x,
                    p.y + u * e.y + w * f.y + 0.02 * n.y,
                    p.z + u * e.z + w * f.z + 0.02 * n.z,
                )
            })
        })
        .collect();
    let cells: HashSet<(i32, i32)> = samples.iter().flatten().map(|&p| cell(p)).collect();
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    let found = on_workers(&runs, |run| {
        let mut found = Vec::new();
        for i in run.clone() {
            let [a, b, c] = triangles[i].vertices;
            let lo = cell(Vector3::new(a.x.min(b.x).min(c.x), 0., a.z.min(b.z).min(c.z)));
            let hi = cell(Vector3::new(a.x.max(b.x).max(c.x), 0., a.z.max(b.z).max(c.z)));
            for x in lo.0..=hi.0 {
                for z in lo.1..=hi.1 {
                    if cells.contains(&(x, z)) {
                        found.push(((x, z), i));
                    }
                }
            }
        }
        found
    });
    for (c, i) in found.into_iter().flatten() {
        grid.entry(c).or_default().push(i);
    }
    // Normal y of the nearest face straight above (up) or below p, if any.
    let nearest = |p: Vector3, up: bool| {
        let mut best: Option<(f32, f32)> = None;
        for &i in grid.get(&cell(p)).into_iter().flatten() {
            let ([a, b, c], n) = (triangles[i].vertices, triangles[i].triangle().feature.normal);
            let (e, f, g) = (sub(b, a), sub(c, a), sub(p, a));
            let den = e.x * f.z - f.x * e.z;
            if den.abs() < 1e-9 {
                continue;
            }
            let (u, w) = ((g.x * f.z - f.x * g.z) / den, (e.x * g.z - g.x * e.z) / den);
            if u < -1e-6 || w < -1e-6 || u + w > 1. + 1e-6 {
                continue;
            }
            let height = a.y + u * e.y + w * f.y - p.y;
            let distance = if up { height } else { -height };
            if distance > 1e-4 && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, n.y));
            }
        }
        best.map(|(_, y)| y)
    };
    for (&i, points) in candidates.iter().zip(&samples) {
        sealed[i] = points.iter().all(|&p| {
            nearest(p, true).is_some_and(|ny| ny > 0.05) && !nearest(p, false).is_some_and(|ny| ny > 0.05)
        });
    }
    sealed
}

/// perf 10-18: source triangles queued before a parallel convert (about 12 MB of them).
const COLLISION_BATCH: usize = 1 << 18;

/// The RWCM archive's collision triangles, their surfaces and query meshes, in archive order.
struct BuiltCollision {
    triangles: Vec<WorldTriangle>,
    packed_surfaces: Vec<u16>,
    meshes: Vec<QueryMesh>,
}

/// A cluster's triangles and its group runs (triangle count, bounds), in order.
type ConvertedCluster = (Vec<WorldTriangle>, Vec<(usize, Bounds)>);

impl BuiltCollision {
    fn push(&mut self, cluster: &[skate_data::retail_collision::RetailTriangle], (triangles, runs): ConvertedCluster) {
        self.packed_surfaces.extend(cluster.iter().map(|t| t.surface));
        let mut start = self.triangles.len();
        self.triangles.extend(triangles);
        for (len, bounds) in runs {
            self.meshes.push(QueryMesh {
                geometry: 0,
                rejection_flags: 0,
                triangle_range: start..start + len,
                local_to_world: RetailAffineTransform::IDENTITY,
                world_to_local: RetailAffineTransform::IDENTITY,
                local_bounds: bounds,
                // Static registration `sub_82776B58` hands the mesh-record ctor
                // `sub_8276CB18` a hardcoded `li r8,-1` for matchingID (+180),
                // alongside the `li r7,0` / `li r6,0` for the rejection mask and
                // support identity that the two fields above already reproduce.
                // Every call site of that ctor passes a constant -1 except one,
                // and that one reads its id from a registered actor/unit object
                // (+40) -- never from cluster data. The cluster's packed unit
                // group is a *per-triangle* attribute in retail: `sub_82AC8A68`
                // stores it at triangle+84, right next to the surface code at
                // +88 that `packed_surfaces` above carries. Filing it on the
                // mesh made `matches()` reject every actor whose id was not the
                // cluster's, silently hiding static geometry from offboard
                // ground and line queries.
                matching_group: -1,
                pool: QueryPool::Ground,
            });
            start += len;
        }
    }

    /// The same collision without the `drop` triangles: each query mesh keeps its order, its bounds shrink to what is
    /// left, and a mesh left empty goes. Compacted in place (the meshes' ranges ascend and touch, so the write never
    /// passes the read): a second copy of New San Vanelona's 4.5M triangles would double the load's peak.
    fn without(mut self, drop: &[bool]) -> Self {
        let mut write = 0;
        let mut meshes = Vec::with_capacity(self.meshes.len());
        for mesh in std::mem::take(&mut self.meshes) {
            let start = write;
            for i in mesh.triangle_range.clone().filter(|&i| !drop[i]) {
                self.triangles[write] = self.triangles[i];
                self.packed_surfaces[write] = self.packed_surfaces[i];
                write += 1;
            }
            let Some(local_bounds) = Bounds::from_points(self.triangles[start..write].iter().flat_map(|t| t.vertices))
            else {
                continue;
            };
            meshes.push(QueryMesh { triangle_range: start..write, local_bounds, ..mesh });
        }
        self.triangles.truncate(write);
        self.packed_surfaces.truncate(write);
        self.meshes = meshes;
        self
    }
}

/// `archive` built with clusters converted `batch` source triangles at a time on the load workers (0: each as it
/// is decoded). Either way the output and the first error are the same.
fn retail_collision_parts(
    archive: &[u8],
    material: RetailContactMaterial,
    batch: usize,
) -> Result<(BuiltCollision, usize), String> {
    // Sized up front: grown by doubling, New San Vanelona's 4.5M triangles peaked at 1.6 GB during the copy.
    let hint = skate_data::retail_collision::triangle_count(archive).unwrap_or(0);
    let mut built = BuiltCollision {
        triangles: Vec::with_capacity(hint),
        packed_surfaces: Vec::with_capacity(hint),
        meshes: Vec::new(),
    };
    let mut pending: Vec<Vec<skate_data::retail_collision::RetailTriangle>> = Vec::new();
    let mut queued = 0;
    let flush = |built: &mut BuiltCollision, pending: &mut Vec<Vec<skate_data::retail_collision::RetailTriangle>>| -> Result<(), String> {
        let converted = on_workers(pending, |cluster: &Vec<_>| convert_cluster(cluster, material));
        for (cluster, converted) in pending.drain(..).zip(converted) {
            built.push(&cluster, converted?);
        }
        Ok(())
    };
    let visited = skate_data::retail_collision::visit_clusters(archive, |_, cluster| {
        if batch == 0 {
            built.push(cluster, convert_cluster(cluster, material)?);
            return Ok(());
        }
        pending.push(cluster.to_vec());
        queued += cluster.len();
        if queued >= batch {
            queued = 0;
            flush(&mut built, &mut pending)?;
        }
        Ok(())
    });
    // Queued clusters precede the visit's error, as they would one by one.
    flush(&mut built, &mut pending)?;
    Ok((built, visited?))
}

fn convert_cluster(
    cluster: &[skate_data::retail_collision::RetailTriangle],
    material: RetailContactMaterial,
) -> Result<ConvertedCluster, String> {
    let mut triangles = Vec::with_capacity(cluster.len());
    let mut runs = Vec::new();
    // Preserve cluster order and partition further only for group filters.
    let mut cursor = 0;
    while cursor < cluster.len() {
        let group = cluster[cursor].group;
        let start = triangles.len();
        while cursor < cluster.len() && cluster[cursor].group == group {
            let source = cluster[cursor];
            let (flags, cosines) = match source.edges {
                Some(edges) => {
                    let (mut flags, cosines) = decode_native_edges(edges)?;
                    flags &= !TriangleFeature::ONE_SIDED;
                    if source.one_sided {
                        flags |= TriangleFeature::ONE_SIDED;
                    }
                    (flags, cosines)
                }
                // TriangleVolume ctor82AC7770 retains these defaults when
                // the unit has no edge data; the mesh sidedness is not read.
                None => (0x1e1, [-1.; 3]),
            };
            triangles.push(
                WorldTriangle::from_vertices(
                    source.points.map(|p| Vector3::new(p[0], p[1], p[2])),
                    material,
                    u32::from(source.surface),
                    flags,
                    cosines,
                    0.,
                )
                .ok_or("Invalid RWCM collision triangle")?,
            );
            cursor += 1;
        }
        let bounds = Bounds::from_points(triangles[start..].iter().flat_map(|t| t.vertices))
            .ok_or("Invalid RWCM cluster bounds")?;
        runs.push((triangles.len() - start, bounds));
    }
    Ok((triangles, runs))
}

pub(crate) fn collision_world(
    map: &SkateMap,
    material: RetailContactMaterial,
) -> Result<BoardWorld, String> {
    if let Some(archive) = retail_archive(map)? {
        return retail_collision_world(archive, material);
    }
    let (positions, vertices) = weld(
        map.geometry
            .collision
            .iter()
            .map(|t| t.points.map(Vec3::from_array)),
    );
    let mut normals = Vec::with_capacity(vertices.len());
    for tri in &map.geometry.collision {
        let [a, b, c] = tri.points.map(Vec3::from_array);
        let normal = (b - a)
            .cross(c - a)
            .try_normalize()
            .ok_or("Invalid SKATE collision triangle normal")?;
        normals.push(normal);
    }
    // Fully native maps need no reconstructed adjacency. Mixed maps still
    // include every face when finding neighbors for their authored geometry.
    let (mut flags, mut cosines) = if map
        .geometry
        .collision
        .iter()
        .any(|t| t.native_edges.is_none())
    {
        edge_features(&positions, &vertices, &normals)
    } else {
        (vec![0; vertices.len()], vec![[1.; 3]; vertices.len()])
    };
    let mut triangles = Vec::with_capacity(vertices.len());
    let mut packed_surfaces = Vec::with_capacity(vertices.len());
    for (i, source) in map.geometry.collision.iter().enumerate() {
        if let Some(edges) = source.native_edges {
            (flags[i], cosines[i]) = decode_native_edges(edges)?;
        }
        let m = &map.materials[source.material as usize - 1];
        // Exact EncodeRwSurfaceId mapping from the reference native adapter.
        packed_surfaces.push((m.audio | (m.physics << 7) | (m.pattern << 12)) as u16);
        let points = vertices[i].map(|id| {
            let p = positions[id];
            Vector3::new(p.x, p.y, p.z)
        });
        // Keep the supplied game's original static-world contact combine values.
        // The native map bridge supplies packed surfaces, not a guessed split of
        // the package's single friction scalar into static/dynamic coefficients.
        triangles.push(
            WorldTriangle::from_vertices(
                points,
                material,
                source.surface,
                flags[i],
                cosines[i],
                0.,
            )
            .ok_or("Invalid SKATE collision volume")?,
        );
    }
    // Portable maps have no native cluster hierarchy. Bound contiguous ranges
    // once at load time so the existing BVH can reject distant geometry. Keep
    // triangle order and mesh identity/filter values: contact tie-breaking,
    // packed surfaces and adjacency must not change with this acceleration.
    let mut meshes = Vec::new();
    for start in (0..triangles.len()).step_by(64) {
        let end = (start + 64).min(triangles.len());
        let bounds = Bounds::from_points(
            triangles[start..end]
                .iter()
                .flat_map(|t| t.vertices),
        )
        .ok_or("SKATE collision bounds empty")?;
        meshes.push(QueryMesh {
            geometry: 0,
            rejection_flags: 0,
            triangle_range: start..end,
            local_to_world: RetailAffineTransform::IDENTITY,
            world_to_local: RetailAffineTransform::IDENTITY,
            local_bounds: bounds,
            matching_group: -1,
            pool: QueryPool::Ground,
        });
    }
    let metadata = QueryMetadata {
        packed_surfaces,
        meshes,
        static_edges: vec![],
        island_flags: 0,
    };
    BoardWorld::with_query_metadata(triangles, metadata).map_err(str::to_owned)
}

/// Retail teleport clearance: `horizontal_spawn` (TU3 0x82DB8998) places the
/// skater 0.1 m above the locator, and the University locators sit on their face
/// (campusstart/stadium/observatory 0.001 m, practice 0.117 m).
const SPAWN_CLEARANCE: f32 = 0.1;

/// The height of the first face under `p`: a line from 1 m above it to 10 m below.
pub(crate) fn face_under(world: &BoardWorld, [x, y, z]: [f32; 3]) -> Option<f32> {
    let hit = world.query_thin_line(Vector3::new(x, y + 1., z), Vector3::new(x, y - 10., z)).ok()??;
    Some(hit.geometry.position.y)
}

/// Retail's teleport ground probe (S3 0x82598428, S2 0x824e3290, S1 0x82419070): a line from 0.75 m above the target
/// to 500 m below it; on a hit the translation becomes the hit + 0.15 m, the basis kept. Every game teleport runs it
/// (TeleportPlayer 0x82864038 -> actor 0x825926f8 with probe 1, and the command-3 job). `horizontal_spawn` then adds
/// its 0.1 m, so the board origin sits 0.25 m above the face. The bail reset (0x82592518) and the spawn-point
/// respawn (0x8245d7c8) pass probe 0. Returns the probed target and the face it hit.
pub(crate) fn probe_ground(world: &BoardWorld, mut at: [[f32; 4]; 4]) -> ([[f32; 4]; 4], Option<f32>) {
    let start = Vector3::new(at[3][0], at[3][1] + 0.75, at[3][2]);
    let Ok(Some(hit)) = world.query_thin_line(start, Vector3::new(start.x, start.y - 500., start.z)) else {
        return (at, None);
    };
    let p = hit.geometry.position;
    at[3][..3].copy_from_slice(&[p.x, p.y + 0.15, p.z]);
    (at, Some(p.y))
}

/// [`probe_ground`] for a game teleport, which must land on something. Not retail: over a hole in our map (the Skate 1
/// city mod lacks some of retail's ground, so a lobby slot 5 m on from its locator can hang over nothing), the ground
/// nearest to it ([`nearest_ground`]), else the nearest walkable face as a package spawn takes ([`supported_spawn`]),
/// at the probe's 0.15 m, instead of a fall forever; and on the board, a spot the board doesn't fit on moves or turns
/// to one it does ([`fitting_spot`]). A skater on foot (Edit Skater stopping it where it stands) stays put.
pub(crate) fn travel_ground(world: &BoardWorld, at: [[f32; 4]; 4], on_board: bool) -> [[f32; 4]; 4] {
    let mut at = match probe_ground(world, at) {
        (probed, Some(_)) => probed,
        (mut at, None) => {
            let target = [at[3][0], at[3][1], at[3][2]];
            let [x, face, z] = nearest_ground(world, target).unwrap_or_else(|| {
                let [x, y, z] = supported_spawn(world, target);
                [x, y - SPAWN_CLEARANCE, z]
            });
            at[3][..3].copy_from_slice(&[x, face + 0.15, z]);
            at
        }
    };
    if on_board && let Some(([x, face, z], turn)) = fitting_spot(world, &at, at[3][1] - 0.15) {
        for row in &mut at[..3] {
            let turned = Quat::from_rotation_y(turn) * Vec3::new(row[0], row[1], row[2]);
            row[..3].copy_from_slice(&turned.to_array());
        }
        at[3][..3].copy_from_slice(&[x, face + 0.15, z]);
    }
    at
}

/// Not retail: where the board fits, for a spot on `face` it doesn't, and the turn of its heading there (radians about
/// +y); None: it fits, or nothing near does. Retail puts the board on the locator whatever stands beside it:
/// DownTown's 1up_dwtn_04 4th waiting spot lies in a 0.32 m gap between two walls, 1 cm from one, and New San
/// Vanelona's kotm_4 Death Race 3rd start 0.1 m from a post, so the board lands against them, tips and the skater
/// bails. The deck fits where no wall (a face steeper than 45°) is within its half length ahead or behind or its half
/// width to the sides, at the wheels' and the deck's height as it rests and as the teleport drops it in (0.3 m:
/// DownTown's walls stand on a 0.15 m base). The spot is the nearest within 1.5 m on the same ground (0.1 m), dry
/// ([`nearest_ground`]'s surfaces) and reached without passing a wall, at the locator's heading if one fits, else
/// turned the least (15° steps): a gap narrower than the board is long, like DownTown's, only takes it lengthwise.
fn fitting_spot(world: &BoardWorld, at: &[[f32; 4]; 4], face: f32) -> Option<([f32; 3], f32)> {
    let flat = |c: [f32; 4]| Vec3::new(c[0], 0., c[2]).try_normalize();
    let (ahead, side) = (flat(at[2])?, flat(at[0])?);
    let v = |p: Vec3| Vector3::new(p.x, p.y, p.z);
    // A steep face between the two points, whichever way it is wound: the thin line meets only faces wound towards its
    // start, and a walkable slope wound downwards reports a normal y of about -0.9.
    let wall = |a: Vec3, b: Vec3| {
        [(a, b), (b, a)].into_iter().any(|(from, to)| matches!(world.query_thin_line(v(from), v(to)), Ok(Some(hit)) if hit.geometry.normal.y.abs() < 0.7))
    };
    let fits = |p: Vec3, turn: f32| {
        let (ahead, side) = (Quat::from_rotation_y(turn) * ahead, Quat::from_rotation_y(turn) * side);
        [0.04, 0.12, 0.3].into_iter().all(|up| {
            let c = p + Vec3::Y * up;
            [(ahead, 0.42), (-ahead, 0.42), (side, 0.12), (-side, 0.12)].into_iter().all(|(d, reach)| !wall(c, c + d * reach))
        })
    };
    let spot = Vec3::new(at[3][0], face, at[3][2]);
    if fits(spot, 0.) {
        return None;
    }
    // The places it may go, nearest first: the spot, then rings 5 cm apart out to 0.5 m (a narrow gap leaves
    // centimetres) and 10 cm apart out to 1.5 m.
    let up = Vec3::Y * 0.3;
    let rings = (0..=10).map(|i| i as f32 * 0.05).chain((6..=15).map(|i| i as f32 * 0.1));
    let places: Vec<Vec3> = rings
        .flat_map(|r| {
            let points = if r == 0. { 1 } else { 16 };
            (0..points).map(move |k| (r, std::f32::consts::TAU * k as f32 / points as f32))
        })
        .filter_map(|(r, angle)| {
            let (x, z) = (spot.x + r * angle.cos(), spot.z + r * angle.sin());
            let hit = world.query_thin_line(Vector3::new(x, face + 0.5, z), Vector3::new(x, face - 0.5, z)).ok()??;
            let p = Vec3::new(x, hit.geometry.position.y, z);
            let dry = !matches!((hit.tag >> 7) & 31, 6 | 9 | 12);
            ((p.y - face).abs() <= 0.1 && dry && !wall(spot + up, p + up)).then_some(p)
        })
        .collect();
    // shortcut: where nothing within 1.5 m fits at any turn, every fit test runs (up to ~90k thin lines, once per
    // teleport; c1 meets none); budget it if a teleport there hitches.
    [0., 15., -15., 30., -30., 45., -45., 60., -60., 75., -75., 90.].into_iter().map(f32::to_radians).find_map(|turn| {
        places.iter().find(|p| fits(**p, turn)).map(|p| (p.to_array(), turn))
    })
}

/// [`travel_ground`]'s spot over a hole: the flat face near `target` that costs least in metres moved plus metres up or
/// down, probed on rings 0.5 m apart out to 10 m, each from 2 m over the target to 3 m under it. Flat: ground 0.6 m out
/// all round, within 0.1 m of the spot's, so the board isn't left on the hole's edge (a skater put on the nearest ground
/// teetered there and fell), and nothing over it for 2 m. A walkable face's centre (supported_spawn) can sit on a roof
/// over the hole, which the skater rolls off and falls 70 m. Never water, Unrideable or InstantBail (physics surfaces 12,
/// 6 and 9: 82DB80C8 bails or teleports a skater standing on them; Danny Way's nearest flat face was a pool).
fn nearest_ground(world: &BoardWorld, [x, y, z]: [f32; 3]) -> Option<[f32; 3]> {
    let face_at = |px: f32, pz: f32| match world.query_thin_line(Vector3::new(px, y + 2., pz), Vector3::new(px, y - 3., pz)) {
        Ok(Some(hit)) if !matches!((hit.tag >> 7) & 31, 6 | 9 | 12) => Some(hit.geometry.position.y),
        _ => None,
    };
    // Flat, with room to stand: no face over the spot for 2 m.
    let flat = |px: f32, pz: f32, face: f32| {
        (0..8).all(|k| {
            let angle = std::f32::consts::TAU * k as f32 / 8.;
            face_at(px + 0.6 * angle.cos(), pz + 0.6 * angle.sin()).is_some_and(|f| (f - face).abs() <= 0.1)
        }) && world.query_thin_line(Vector3::new(px, face + 0.05, pz), Vector3::new(px, face + 2., pz)).is_ok_and(|hit| hit.is_none())
    };
    let mut best: Option<(f32, [f32; 3])> = None;
    for ring in 0..=20u32 {
        let r = ring as f32 * 0.5;
        let points = (8 * ring).max(1);
        for k in 0..points {
            let angle = std::f32::consts::TAU * k as f32 / points as f32;
            let (px, pz) = (x + r * angle.cos(), z + r * angle.sin());
            let Some(face) = face_at(px, pz) else { continue };
            let cost = r + (face - y).abs();
            if best.is_none_or(|(c, _)| cost < c) && flat(px, pz, face) {
                best = Some((cost, [px, face, pz]));
            }
        }
        // Every spot further out costs more than its ring's radius.
        if best.is_some_and(|(c, _)| c <= r) {
            break;
        }
    }
    best.map(|(_, spot)| spot)
}

/// The package spawn (a wheel-ground anchor), lowered to the retail 0.1 m above
/// the face under it, or, if the runtime collision has nothing under it, 0.1 m
/// above the walkable face nearest to it. The converter places spawns 1 m above
/// the face (`surface_height + 1.0`), so startup fell 1 m and landed; a few
/// (StartPark, MegaPark) hang over empty space, so the skater fell forever.
pub(crate) fn supported_spawn(world: &BoardWorld, spawn: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = spawn;
    if let Some(face) = face_under(world, spawn) {
        // ponytail: min keeps spawns authored on (or within 0.1 m of) the face as-is.
        return [x, y.min(face + SPAWN_CLEARANCE), z];
    }
    let at = Vec2::new(spawn[0], spawn[2]);
    let nearest = world
        .triangles()
        .iter()
        .filter_map(|t| {
            let [a, b, c] = t.vertices.map(|v| Vec3::new(v.x, v.y, v.z));
            // The converter's walkable face: at least 2 m² and facing up.
            let cross = (b - a).cross(c - a);
            let centre = (a + b + c) / 3.;
            (cross.length() >= 4. && cross.y >= 0.9 * cross.length())
                .then(|| [centre.x, centre.y + SPAWN_CLEARANCE, centre.z])
        })
        .min_by(|p, q| {
            at.distance_squared(Vec2::new(p[0], p[2]))
                .total_cmp(&at.distance_squared(Vec2::new(q[0], q[2])))
        });
    match nearest {
        Some(p) => {
            eprintln!("SKATE_SPAWN_UNSUPPORTED package={spawn:?} using={p:?}");
            p
        }
        None => spawn,
    }
}

/// Reuse identical render resources without changing authored triangles.
fn render_texture_ids(textures: &[skate_data::skate_map::Texture]) -> Vec<u32> {
    use std::hash::{Hash, Hasher};
    let mut buckets: HashMap<(u32, u32, u32, u64), Vec<usize>> = HashMap::new();
    let mut ids = vec![0];
    // G3: hashing every texel was ~0.5 s of New San Vanelona's load on one thread.
    let hashes = on_workers(textures, |t| {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        t.rgba.hash(&mut hash);
        hash.finish()
    });
    for (i, (t, hash)) in textures.iter().zip(hashes).enumerate() {
        let bucket = buckets
            .entry((t.width, t.height, t.color_space, hash))
            .or_default();
        let same = bucket.iter().copied().find(|&j| textures[j].rgba == t.rgba);
        let canonical = same.unwrap_or_else(|| {
            bucket.push(i);
            i
        });
        ids.push(canonical as u32 + 1);
    }
    ids
}

#[cfg(test)]
fn render_material_ids(
    materials: &[skate_data::skate_map::Material],
    texture_ids: &[u32],
) -> Vec<usize> {
    material_ids(materials, texture_ids, true)
}

#[cfg(test)]
fn material_ids(
    materials: &[skate_data::skate_map::Material],
    texture_ids: &[u32],
    include_lightmap: bool,
) -> Vec<usize> {
    material_ids_with(materials, texture_ids, include_lightmap, &Definitions::parse(materials).ids)
}

/// `definitions[i]`: material i's retail definition identity (Definitions::ids).
fn material_ids_with(
    materials: &[skate_data::skate_map::Material],
    texture_ids: &[u32],
    include_lightmap: bool,
    definitions: &[u32],
) -> Vec<usize> {
    let mut unique = HashMap::new();
    materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            *unique
                .entry((pbr_key(m, texture_ids, include_lightmap), definitions[i]))
                .or_insert(i)
        })
        .collect()
}

/// Every material's retail definition, parsed once per load, and its identity: 0 without one, else equal for equal
/// definitions. G3: material_ids (twice), the lightmap atlas and the batches each parsed every definition again
/// (~1 s a pass on New San Vanelona), before any map texture could start converting.
struct Definitions {
    parsed: Vec<Option<crate::retail_render::Definition>>,
    ids: Vec<u32>,
}
impl Definitions {
    fn parse(materials: &[skate_data::skate_map::Material]) -> Self {
        let parse = |m: &skate_data::skate_map::Material| {
            m.retail_definition.as_deref().and_then(crate::retail_render::Definition::parse)
        };
        let parsed = on_workers(materials, parse);
        let mut unique = HashMap::new();
        let ids = parsed
            .iter()
            .map(|d| d.as_ref().map_or(0, |d| { let next = unique.len() as u32 + 1; *unique.entry(d).or_insert(next) }))
            .collect();
        Self { parsed, ids }
    }
}

/// Load worker threads: up to 8, leaving two cores for the running game.
fn load_workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2)).clamp(1, 8)
}

/// `items` mapped by `f` in one run per load worker, in `items`' order.
fn on_workers<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let run = items.len().div_ceil(load_workers()).max(1);
    let f = &f;
    std::thread::scope(|scope| {
        let runs: Vec<_> = items
            .chunks(run)
            .map(|run| (run, std::thread::Builder::new().name("map-load-worker".into()).spawn_scoped(scope, move || run.iter().map(f).collect::<Vec<_>>())))
            .collect();
        // A run whose thread could not start is mapped here.
        runs.into_iter()
            .flat_map(|(run, worker)| match worker {
                Ok(worker) => worker.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                Err(_) => run.iter().map(f).collect(),
            })
            .collect()
    })
}

/// Every field the PBR material reads. Source names and collision surface
/// metadata do not change a PBR material.
fn pbr_key(
    m: &skate_data::skate_map::Material,
    texture_ids: &[u32],
    include_lightmap: bool,
) -> [u32; 13] {
    let textures = m.textures.map(|id| texture_ids[id as usize]);
    [
        textures[0],
        if include_lightmap { textures[1] } else { 0 },
        textures[2],
        textures[3],
        textures[4],
        m.color[0].to_bits(),
        m.color[1].to_bits(),
        m.color[2].to_bits(),
        m.roughness.to_bits(),
        m.emissive.to_bits(),
        m.indirect_strength.to_bits(),
        m.alpha_mode,
        m.alpha_cutoff.to_bits(),
    ]
}

fn render_groups(
    geometry: &skate_data::skate_map::Geometry,
    material_ids: &[usize],
) -> Vec<(usize, Vec<u32>)> {
    let mut lookup = HashMap::new();
    let mut groups: Vec<(usize, Vec<u32>)> = Vec::new();
    for tri in geometry.indices.chunks_exact(3) {
        let material = material_ids[geometry.vertices[tri[0] as usize].material as usize - 1];
        let group = *lookup.entry(material).or_insert_with(|| {
            let index = groups.len();
            groups.push((material, Vec::new()));
            index
        });
        groups[group].1.extend_from_slice(tri);
    }
    groups
}

struct RenderGroup {
    material: usize,
    indices: Vec<u32>,
    retail: Option<crate::retail_render::RetailWorldMaterial>,
    /// The tangent mode it was batched by: 0 authored frames, 1 generated, 2 none (consolidated_groups_with).
    tangents: u8,
}

/// perf 10-19: SKATE_MERGE_BLEND=1 merges blended batches of one material per [`BLEND_CELL`] cell (off by default:
/// draw order inside a cell can change, see consolidated_groups_with).
fn merge_blend() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SKATE_MERGE_BLEND").is_ok_and(|v| v == "1"))
}
/// perf 10-19: the cell blended batches merge within under SKATE_MERGE_BLEND.
const BLEND_CELL: f32 = 64.;

#[cfg(test)]
fn consolidated_groups(
    map: &SkateMap,
    texture_ids: &[u32],
    tuning: &crate::retail_render::MaterialTuning,
    texture: &mut impl FnMut(u32, u8) -> Option<Handle<Image>>,
    atlas: &lightmap_atlas::Atlas,
    pages: &[Handle<Image>],
) -> Vec<RenderGroup> {
    let definitions = Definitions::parse(&map.materials);
    let unified = crate::config::perf_on("SKATE_UNIFIED_LAYOUT");
    consolidated_groups_with(map, texture_ids, tuning, texture, atlas, pages, &definitions, unified, merge_blend())
}
/// `unified`: SKATE_UNIFIED_LAYOUT (see spawn_instances); `merge_blend`: [`merge_blend`].
#[allow(clippy::too_many_arguments)]
fn consolidated_groups_with(
    map: &SkateMap,
    texture_ids: &[u32],
    tuning: &crate::retail_render::MaterialTuning,
    texture: &mut impl FnMut(u32, u8) -> Option<Handle<Image>>,
    atlas: &lightmap_atlas::Atlas,
    pages: &[Handle<Image>],
    definitions: &Definitions,
    unified: bool,
    merge_blend: bool,
) -> Vec<RenderGroup> {
    let ids = material_ids_with(&map.materials, texture_ids, true, &definitions.ids);
    let mut texture = |id: u32, role: u8| match id.checked_sub(lightmap_atlas::FIRST_PAGE) {
        Some(page) => pages.get(page as usize).cloned(),
        None => texture(id, role),
    };
    let mut lookup = HashMap::new();
    let mut pbr_lookup = HashMap::new();
    let mut blend_lookup = HashMap::new();
    let mut skate2_map = None;
    let mut groups: Vec<RenderGroup> = Vec::new();
    for (material, indices) in render_groups(&map.geometry, &ids) {
        let m = &map.materials[material];
        let retail = definitions.parsed[material]
            .as_ref()
            .filter(|d| d.supported(tuning))
            .cloned()
            .map(|mut d| {
                // Atlased lightmap: batches no longer split by lightmap alone.
                if let Some(page) = atlas.page(material) {
                    let uv = d.bindings.get("lightmap").map_or(1, |b| b.uv);
                    d.bindings.insert(
                        "lightmap".into(),
                        crate::retail_render::Binding {
                            texture: lightmap_atlas::FIRST_PAGE + page,
                            uv,
                            u: 1,
                            v: 1,
                        },
                    );
                }
                let mut built = d.build(m, tuning, &mut texture);
                // Skate 2 proxyworld m_params.y (captures); proxies carry no lightmap
                // evidence of their own, the map's other materials do.
                if d.family == 11
                    && *skate2_map.get_or_insert_with(|| {
                        definitions.parsed.iter().flatten().any(|d| d.skate2_lighting())
                    })
                {
                    built.params.family.w = 0.4;
                }
                built
            });
        // Tangent construction must not switch paths when batches are combined.
        let explicit = indices
            .iter()
            .all(|&i| map.geometry.vertices[i as usize].tangent_frame.is_some());
        let tangent_mode = if explicit {
            0
        } else if m.textures[2] != 0 {
            1
        } else {
            2
        };
        // perf 10-19 (SKATE_UNIFIED_LAYOUT): every retail batch carries tangents (zero without a frame) and colours,
        // so one without any frame batches with its material's framed ones: zero tangents take retail_world.wgsl's
        // derivative frame, as no tangents did. Batches with only some frames stay apart (their meshes use frames
        // only when every vertex has one).
        let tangent_mode = if unified
            && tangent_mode == 2
            && retail.is_some()
            && indices.iter().all(|&i| map.geometry.vertices[i as usize].tangent_frame.is_none())
        {
            0
        } else {
            tangent_mode
        };
        // perf 10-19 (SKATE_MERGE_BLEND=1, off): blended batches of one material merge per BLEND_CELL cell. The
        // sorted pass then orders a cell's batch by its one centre, and inside it by source order, where separate
        // batches sorted by their own centres. Alpha blending isn't commutative, so where two of them overlap on
        // screen they can draw in another order than before.
        if let Some(blended) = retail
            .as_ref()
            .filter(|m| merge_blend && tangent_mode != 1 && m.batch_key().is_none())
        {
            let position = |i: u32| Vec3::from_array(map.geometry.vertices[i as usize].position);
            let cells: &mut HashMap<(i32, i32), usize> =
                blend_lookup.entry((blended.material_key(), tangent_mode)).or_default();
            for tri in indices.chunks_exact(3) {
                let c = (position(tri[0]) + position(tri[1]) + position(tri[2])) / (3. * BLEND_CELL);
                let slot = *cells.entry((c.x.floor() as i32, c.z.floor() as i32)).or_insert_with(|| {
                    groups.push(RenderGroup {
                        material,
                        indices: Vec::new(),
                        retail: retail.clone(),
                        tangents: tangent_mode,
                    });
                    groups.len() - 1
                });
                groups[slot].indices.extend_from_slice(tri);
            }
            continue;
        }
        // Tangent generation can average shared vertices across old boundaries.
        // Keep those batches separate rather than changing their shading.
        if let Some(key) = retail
            .as_ref()
            .and_then(|m| m.batch_key())
            .filter(|_| tangent_mode != 1)
        {
            let destination = *lookup.entry((key, tangent_mode)).or_insert(groups.len());
            if destination < groups.len() {
                groups[destination].indices.extend(indices);
                continue;
            }
        }
        // PBR fallbacks whose lightmaps share an atlas page differ only in
        // their UV_1 remap, which is per vertex. Blended ones keep their
        // sorting groups, as retail ones do.
        if let Some(page) = atlas
            .page(material)
            .filter(|_| retail.is_none() && m.alpha_mode != 2 && tangent_mode != 1)
        {
            let destination = *pbr_lookup
                .entry((pbr_key(m, texture_ids, false), page, tangent_mode))
                .or_insert(groups.len());
            if destination < groups.len() {
                groups[destination].indices.extend(indices);
                continue;
            }
        }
        groups.push(RenderGroup {
            material,
            indices,
            retail,
            tangents: tangent_mode,
        });
    }
    split_large(map, groups)
}

/// Culling works per mesh, so a merged batch spanning a city is drawn in full
/// by every view, including the skater's 24 m shadow cascade (4.5 of ~11 GPU
/// ms on Skate 1 San Vanelona). Batches wider than a cell are cut into cells by
/// triangle centroid. Blended and tangent-generating batches keep their groups.
/// ponytail: fixed cell from an RTX 2050 A/B (Skate 1 San Vanelona 97 -> 153 FPS);
/// 64-256 m cost more in draw count than they saved (New San Vanelona 60k batches).
const CELL: f32 = 512.;
fn split_large(map: &SkateMap, groups: Vec<RenderGroup>) -> Vec<RenderGroup> {
    let position = |i: u32| Vec3::from_array(map.geometry.vertices[i as usize].position);
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        let m = &map.materials[group.material];
        let blended = group
            .retail
            .as_ref()
            .map_or(m.alpha_mode == 2, |r| r.batch_key().is_none());
        // perf 10-19: by its batching mode (the old normal-map-and-not-all-framed test, until SKATE_UNIFIED_LAYOUT
        // batched frameless sources with framed ones: those generate nothing).
        let generated_tangents = group.tangents == 1;
        let (min, max) = group
            .indices
            .iter()
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &i| {
                (lo.min(position(i)), hi.max(position(i)))
            });
        if blended || generated_tangents || (max.x - min.x).max(max.z - min.z) <= CELL {
            out.push(group);
            continue;
        }
        let mut cells = HashMap::new();
        for tri in group.indices.chunks_exact(3) {
            let c = (position(tri[0]) + position(tri[1]) + position(tri[2])) / (3. * CELL);
            let slot = *cells
                .entry((c.x.floor() as i32, c.z.floor() as i32))
                .or_insert_with(|| {
                    out.push(RenderGroup {
                        material: group.material,
                        indices: Vec::new(),
                        retail: group.retail.clone(),
                        tangents: group.tangents,
                    });
                    out.len() - 1
                });
            out[slot].indices.extend_from_slice(tri);
        }
    }
    out
}

/// The reference RW mesh compiler's 1 mm vertex welding: shared positions and
/// per-triangle position ids.
pub(crate) fn weld(tris: impl Iterator<Item = [Vec3; 3]>) -> (Vec<Vec3>, Vec<[usize; 3]>) {
    let mut welded = HashMap::<[i64; 3], usize>::new();
    let mut positions = Vec::<Vec3>::new();
    let vertices = tris
        .map(|points| {
            points.map(|p| {
                let inverse = 1.0 / f64::from(0.001_f32);
                let key = p.to_array().map(|v| (f64::from(v) * inverse).round() as i64);
                *welded.entry(key).or_insert_with(|| {
                    positions.push(p);
                    positions.len() - 1
                })
            })
        })
        .collect();
    (positions, vertices)
}

/// Edge flags and cosines from the reference compiler's reversed edge pairing
/// over welded triangles. Triangle diagonals are adjacency, never authored ledges.
pub(crate) fn edge_features(
    positions: &[Vec3],
    vertices: &[[usize; 3]],
    normals: &[Vec3],
) -> (Vec<u32>, Vec<[f32; 3]>) {
    let mut cosines = vec![[1.; 3]; vertices.len()];
    let mut flags =
        vec![TriangleFeature::ONE_SIDED | TriangleFeature::USE_EDGE_COSINES | 0xe0; vertices.len()];
    let mut open = HashMap::<(usize, usize), (usize, usize)>::new();
    for (i, ids) in vertices.iter().enumerate() {
        for edge in 0..3 {
            let (a, b) = (ids[edge], ids[(edge + 1) % 3]);
            if let Some((other, oe)) = open.remove(&(b, a)) {
                let cosine = normals[i].dot(normals[other]).clamp(-1., 1.);
                let orientation =
                    (positions[b] - positions[a]).dot(normals[i].cross(normals[other]));
                // ExtendedEdgeCosine / MakeEdgeCode in rw_collision_mesh.cpp:
                // orientation >= -1e-6 is convex; flat edges have no convex bit.
                for (ti, e) in [(i, edge), (other, oe)] {
                    cosines[ti][e] = cosine;
                    if orientation <= -1.0e-6 || cosine >= 1. {
                        flags[ti] &= !(0x20 << e);
                    }
                }
            } else {
                open.entry((a, b)).or_insert((i, edge));
            }
        }
    }
    let mut adjacent = vec![Vec::new(); positions.len()];
    for (i, ids) in vertices.iter().enumerate() {
        for &v in ids {
            adjacent[v].push(i);
        }
    }
    for (v, faces) in adjacent.iter().enumerate() {
        let Some(&first) = faces.first() else {
            continue;
        };
        let reference = normals[first];
        if faces
            .iter()
            .all(|&i| (reference.dot(normals[i]) - 1.).abs() <= 0.01)
        {
            for &i in faces {
                for corner in 0..3 {
                    if vertices[i][corner] == v {
                        flags[i] |= 0x200 << corner;
                    }
                }
            }
        }
    }
    (flags, cosines)
}

/// Movable props move as whole entities, so no batch may span two of them.
/// Batches without a movable prop pass through unchanged.
fn split_instances(
    map: &SkateMap,
    groups: Vec<RenderGroup>,
    instances: &[Option<u32>],
) -> Vec<(Option<u32>, RenderGroup)> {
    let instance = |i: u32| {
        instances
            .get((map.geometry.vertices[i as usize].material as usize).checked_sub(1)?)
            .copied()
            .flatten()
    };
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        if instances.is_empty() {
            out.push((None, group));
            continue;
        }
        let mut parts = HashMap::new();
        for tri in group.indices.chunks_exact(3) {
            let key = instance(tri[0]);
            let slot = *parts.entry(key).or_insert_with(|| {
                out.push((
                    key,
                    RenderGroup {
                        material: group.material,
                        indices: Vec::new(),
                        retail: group.retail.clone(),
                        tangents: group.tangents,
                    },
                ));
                out.len() - 1
            });
            out[slot].1.indices.extend_from_slice(tri);
        }
    }
    out
}

#[cfg(test)]
mod performance_inventory {
    use super::*;
    #[test]
    #[ignore = "diagnostic: SKATE_MAP_INVENTORY, SKATE_ASSET_ROOT"]
    fn texels_by_role() {
        let map = SkateMap::load(std::path::Path::new(&std::env::var_os("SKATE_MAP_INVENTORY").unwrap())).unwrap();
        let tuning = crate::retail_render::MaterialTuning::load(std::path::Path::new(&std::env::var_os("SKATE_ASSET_ROOT").unwrap()));
        let textures = render_texture_ids(&map.textures);
        let seen = std::cell::RefCell::new(std::collections::BTreeSet::new());
        let mut texture = |id: u32, role: u8| {
            let id = textures[id as usize];
            if id != 0 {
                seen.borrow_mut().insert((id, role));
            }
            None
        };
        let _ = consolidated_groups(&map, &textures, &tuning, &mut texture, &Default::default(), &[]);
        let mut by_role = std::collections::BTreeMap::<u8, (usize, u64, u32)>::new();
        for &(id, role) in seen.borrow().iter() {
            let t = &map.textures[id as usize - 1];
            let e = by_role.entry(role).or_default();
            e.0 += 1;
            e.1 += u64::from(t.width) * u64::from(t.height);
            e.2 = e.2.max(t.width.max(t.height));
        }
        eprintln!("ROLES {by_role:?}");
    }
    #[test]
    #[ignore = "requires SKATE_MAP_INVENTORY, SKATE_ASSET_ROOT and SKATE_MAP_INVENTORY_OUT"]
    fn consolidated_batches() {
        let map = SkateMap::load(std::path::Path::new(
            &std::env::var_os("SKATE_MAP_INVENTORY").unwrap(),
        ))
        .unwrap();
        let tuning = crate::retail_render::MaterialTuning::load(std::path::Path::new(
            &std::env::var_os("SKATE_ASSET_ROOT").unwrap(),
        ));
        let textures = render_texture_ids(&map.textures);
        let baseline = render_groups(
            &map.geometry,
            &render_material_ids(&map.materials, &textures),
        );
        let mut texture = |id: u32, role: u8| {
            let id = textures[id as usize];
            if id == 0
                || (role == 5
                    && map.textures[id as usize - 1].height
                        != map.textures[id as usize - 1].width * 6)
            {
                return None;
            }
            Some(Handle::Uuid(
                bevy::asset::uuid::Uuid::from_u128((u128::from(id) << 8) | u128::from(role)),
                default(),
            ))
        };
        let groups = consolidated_groups(
            &map,
            &textures,
            &tuning,
            &mut texture,
            &Default::default(),
            &[],
        );
        let mut before: Vec<[u32; 3]> = map
            .geometry
            .indices
            .chunks_exact(3)
            .map(|t| t.try_into().unwrap())
            .collect();
        let mut after: Vec<[u32; 3]> = groups
            .iter()
            .flat_map(|g| g.indices.chunks_exact(3).map(|t| t.try_into().unwrap()))
            .collect();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(before, after);
        let mut opaque = 0;
        let mut masked = 0;
        let mut blended = 0;
        let mut fallback = 0;
        for group in &groups {
            match group.retail.as_ref().map(|m| m.alpha) {
                Some(AlphaMode::Opaque) => opaque += 1,
                Some(AlphaMode::Mask(_)) => masked += 1,
                Some(_) => blended += 1,
                None => fallback += 1,
            }
        }
        let mut shadows = shadow_geometry::ShadowGeometry::default();
        let mut source_shadow_triangles: Vec<[u32; 3]> = Vec::new();
        let mut replaced_casters = 0;
        for group in &groups {
            if let Some(material) = group
                .retail
                .as_ref()
                .filter(|m| shadow_geometry::eligible(m))
            {
                shadows.add(&map.geometry, &group.indices, &Handle::default(), material.two_sided);
                source_shadow_triangles
                    .extend(group.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]));
                replaced_casters += 1;
            }
        }
        let mut batched_shadow_triangles: Vec<[u32; 3]> = shadows
            .batches
            .iter()
            .flat_map(|(_, indices)| indices.chunks_exact(3).map(|t| t.try_into().unwrap()))
            .collect();
        source_shadow_triangles.sort_unstable();
        batched_shadow_triangles.sort_unstable();
        assert_eq!(source_shadow_triangles, batched_shadow_triangles);
        let proxy_vertices: usize = shadows
            .batches
            .iter()
            .map(|(_, indices)| {
                indices
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
            })
            .sum();
        let bounds = groups
            .iter()
            .filter(|g| g.retail.is_some())
            .map(|g| {
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = -min;
                for &i in &g.indices {
                    let p = Vec3::from_array(map.geometry.vertices[i as usize].position);
                    min = min.min(p);
                    max = max.max(p);
                }
                bevy::camera::primitives::Aabb::from_min_max(min, max)
            })
            .collect();
        let shadow_index = crate::retail_render::shadow_visibility::benchmark_bounds(bounds);
        let result = serde_json::json!({"shadow_index":shadow_index,"baseline_batches":baseline.len(),"consolidated_batches":groups.len(),"opaque":opaque,"masked":masked,"blended":blended,"fallback":fallback,"triangles":after.len(),"map_lights":map.lights.len(),"triangle_multiset_preserved":true,"replaced_opaque_shadow_casters":replaced_casters,"opaque_shadow_batches":shadows.batches.len(),"shadow_triangle_multiset_preserved":true,"opaque_shadow_triangles":source_shadow_triangles.len(),"proxy_vertices":proxy_vertices,"proxy_vertex_index_bytes":proxy_vertices*24+source_shadow_triangles.len()*12});
        std::fs::write(
            std::env::var_os("SKATE_MAP_INVENTORY_OUT").unwrap(),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }
    /// FPS pass 10-23: vertex-shader runs per triangle (FIFO-32 cache misses) of the map batches in their own
    /// triangle order, as built (one vertex per source index) and welded (one per distinct attribute set; shadow
    /// proxies by position, all a depth pass reads). Prints counts only.
    #[test]
    #[ignore = "diagnostic: SKATE_MAP_INVENTORY, SKATE_ASSET_ROOT"]
    fn vertex_reuse() {
        fn misses(ids: impl Iterator<Item = u64>) -> usize {
            let mut fifo = std::collections::VecDeque::with_capacity(32);
            let mut n = 0;
            for id in ids {
                if !fifo.contains(&id) {
                    n += 1;
                    if fifo.len() == 32 {
                        fifo.pop_front();
                    }
                    fifo.push_back(id);
                }
            }
            n
        }
        fn welded<K: std::hash::Hash + Eq>(keys: impl Iterator<Item = K>) -> (Vec<u64>, usize) {
            let mut ids = HashMap::new();
            let out = keys.map(|k| { let n = ids.len() as u64; *ids.entry(k).or_insert(n) }).collect();
            (out, ids.len())
        }
        let map = SkateMap::load(std::path::Path::new(&std::env::var_os("SKATE_MAP_INVENTORY").unwrap())).unwrap();
        let tuning = crate::retail_render::MaterialTuning::load(std::path::Path::new(&std::env::var_os("SKATE_ASSET_ROOT").unwrap()));
        let textures = render_texture_ids(&map.textures);
        let mut texture = |id: u32, role: u8| {
            let id = textures[id as usize];
            (id != 0).then(|| Handle::Uuid(bevy::asset::uuid::Uuid::from_u128((u128::from(id) << 8) | u128::from(role)), default()))
        };
        let groups = consolidated_groups(&map, &textures, &tuning, &mut texture, &Default::default(), &[]);
        let vertex = |i: u32| &map.geometry.vertices[i as usize];
        let full = |i: u32| {
            let v = vertex(i);
            (v.position.map(f32::to_bits), v.normal.map(f32::to_bits), v.uv.map(f32::to_bits), v.lightmap_uv.map(f32::to_bits),
                v.material, v.decal_uv.map(|d| d.map(f32::to_bits)), v.tangent_frame)
        };
        // [triangles, vertices as built, welded, misses as built, misses welded]
        let mut main = [0usize; 5];
        let mut generated = [0usize; 5];
        for g in &groups {
            let row = if g.tangents == 1 { &mut generated } else { &mut main };
            let (ids, distinct) = welded(g.indices.iter().map(|&i| full(i)));
            row[0] += g.indices.len() / 3;
            row[1] += g.indices.iter().collect::<std::collections::HashSet<_>>().len();
            row[2] += distinct;
            row[3] += misses(g.indices.iter().map(|&i| u64::from(i)));
            row[4] += misses(ids.into_iter());
        }
        for (name, r) in [("main", main), ("generated-tangents", generated)] {
            let t = r[0].max(1) as f32;
            eprintln!("REUSE {name}: triangles {} vertices {} welded {} | runs/triangle as built {:.3} welded {:.3}",
                r[0], r[1], r[2], r[3] as f32 / t, r[4] as f32 / t);
        }
    }
    /// Explicit read-only inventory, never initializes Bevy or a GPU. Asset path
    /// stays in the environment; output contains counts/bounds only.
    #[test]
    #[ignore = "requires SKATE_MAP_INVENTORY and SKATE_MAP_INVENTORY_OUT"]
    fn runtime_batches() {
        let map = SkateMap::load(std::path::Path::new(
            &std::env::var_os("SKATE_MAP_INVENTORY").unwrap(),
        ))
        .unwrap();
        let textures = render_texture_ids(&map.textures);
        let materials = render_material_ids(&map.materials, &textures);
        let groups = render_groups(&map.geometry, &materials);
        let mut spans = [0usize; 4];
        let mut triangles = [0usize; 4];
        let mut cells = std::collections::HashSet::new();
        for (material, indices) in &groups {
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for &i in indices {
                let p = Vec3::from_array(map.geometry.vertices[i as usize].position);
                min = min.min(p);
                max = max.max(p);
            }
            let extent = (max.x - min.x).max(max.z - min.z);
            for (i, threshold) in [50., 100., 250., 500.].iter().enumerate() {
                if extent > *threshold {
                    spans[i] += 1;
                    triangles[i] += indices.len() / 3;
                }
            }
            for tri in indices.chunks_exact(3) {
                let p = tri
                    .iter()
                    .map(|&i| Vec3::from_array(map.geometry.vertices[i as usize].position) / 3.)
                    .sum::<Vec3>();
                cells.insert((
                    *material,
                    (p.x / 32.).floor() as i32,
                    (p.z / 32.).floor() as i32,
                ));
            }
        }
        let result = serde_json::json!({"schema":1,"runtime_material_batches":groups.len(),"extent_threshold_metres":[50,100,250,500],"batches_over_threshold":spans,"triangles_in_batches_over_threshold":triangles,"centroid_32m_cells":cells.len(),"note":"Exact current material canonicalization and grouping; no frustum, occlusion or runtime frame measurement"});
        std::fs::write(
            std::env::var_os("SKATE_MAP_INVENTORY_OUT").unwrap(),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }
}

/// One map texture in the format its role samples: colour (0) is sRGB for
/// colour-managed exports, details (3), specular (6) and cube maps (5) carry mip
/// chains. Lightmaps (4, clamped) stay as authored for both renderers, which
/// decode x*x*4 after filtering, so one upload (or atlas tile) serves both.
/// Whether the GPU samples BC formats; set before any scene is prepared.
static BC7: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn detect_texture_compression(world: &World) {
    let Some(device) = world.get_resource::<bevy::render::renderer::RenderDevice>() else {
        return;
    };
    let bc = device
        .features()
        .contains(bevy::render::render_resource::WgpuFeatures::TEXTURE_COMPRESSION_BC);
    if !BC7.swap(bc, std::sync::atomic::Ordering::Relaxed) && bc {
        info!("SKATE_TEXTURES bc7=true");
    }
}
/// Load timings without a GPU: encode as a BC-capable one does.
#[cfg(test)]
pub(crate) fn assume_bc7() {
    BC7.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// An RGBA8 texture (mip levels in sequence). Uploaded as BC7 when the GPU
/// supports it: a quarter of the memory and bandwidth (New San Vanelona's
/// ~1.2 GB of textures overflowed a 4 GB card). Cube maps and sizes that are
/// not 4-texel aligned stay RGBA8.
/// Encoded with the fastest BC7 profile, kept on disk by [`bc7_cached`] (perf 10-18).
fn texture_image(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    layers: u32,
    levels: u32,
    format: TextureFormat,
) -> Image {
    let (data, format) = match (compresses(width, height, layers), format) {
        (true, TextureFormat::Rgba8UnormSrgb) => (
            bc7_cached(&rgba, width, height, levels),
            TextureFormat::Bc7RgbaUnormSrgb,
        ),
        (true, TextureFormat::Rgba8Unorm) => (
            bc7_cached(&rgba, width, height, levels),
            TextureFormat::Bc7RgbaUnorm,
        ),
        _ => (rgba, format),
    };
    uploaded(data, width, height, layers, levels, format)
}
/// Whether texture_image encodes this size as BC7.
fn compresses(width: u32, height: u32, layers: u32) -> bool {
    BC7.load(std::sync::atomic::Ordering::Relaxed) && layers == 1 && width % 4 == 0 && height % 4 == 0
}
/// The image for `data` already in `format` (mip levels in sequence).
fn uploaded(data: Vec<u8>, width: u32, height: u32, layers: u32, levels: u32, format: TextureFormat) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Image::new checks the data against one mip level; set the chain after.
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image
}

fn bc7_levels(rgba: &[u8], width: u32, height: u32, levels: u32) -> Vec<u8> {
    use intel_tex_2::bc7;
    let settings = if rgba.chunks_exact(4).all(|p| p[3] == 255) {
        bc7::opaque_ultra_fast_settings()
    } else {
        bc7::alpha_ultra_fast_settings()
    };
    let mut out = Vec::with_capacity(rgba.len() / 4 + 16 * levels as usize);
    let mut offset = 0;
    for level in 0..levels {
        let (w, h) = ((width >> level).max(1), (height >> level).max(1));
        let (bw, bh) = (w.next_multiple_of(4), h.next_multiple_of(4));
        let pixels = &rgba[offset..offset + (w * h * 4) as usize];
        offset += pixels.len();
        // Small mips fill their partial blocks by edge replication.
        let padded: Vec<u8>;
        let data = if (bw, bh) == (w, h) {
            pixels
        } else {
            padded = (0..bh)
                .flat_map(|y| {
                    (0..bw).flat_map(move |x| {
                        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
                        pixels[i..i + 4].iter().copied()
                    })
                })
                .collect();
            &padded
        };
        out.extend(bc7::compress_blocks(
            &settings,
            &intel_tex_2::RgbaSurface {
                data,
                width: bw,
                height: bh,
                stride: bw * 4,
            },
        ));
    }
    out
}

/// perf 10-18: what the BC7 encode makes of these texels (the encoder and its profile choice); a change makes new keys.
const BC7_CACHE_VERSION: &[u8] = b"bc7-cache 1; intel_tex_2 0.5 ultra_fast opaque/alpha";
/// The disk cache's cap; past it the least recently used files go (New San Vanelona's world takes about 300 MB).
const BC7_CACHE_BYTES: u64 = 1 << 30;
/// %LOCALAPPDATA%\Skate3RustEngine\bc7-cache (beside the audio PCM cache), never the installation.
/// SKATE_BC7_CACHE=0: none.
fn bc7_cache_dir() -> Option<&'static std::path::Path> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::var_os("LOCALAPPDATA")
            .filter(|_| crate::config::perf_on("SKATE_BC7_CACHE"))
            .map(|root| std::path::PathBuf::from(root).join("Skate3RustEngine/bc7-cache"))?;
        std::fs::create_dir_all(&dir).ok()?;
        // Once per run, off the load: the cache back under its cap.
        let pruned = dir.clone();
        let _ = std::thread::Builder::new().name("bc7-cache-prune".into()).spawn(move || prune_bc7_cache(&pruned, BC7_CACHE_BYTES));
        Some(dir)
    })
    .as_deref()
}
/// [`bc7_levels`], read from the disk cache when it holds these texels' blocks; encoded and kept otherwise.
fn bc7_cached(rgba: &[u8], width: u32, height: u32, levels: u32) -> Vec<u8> {
    bc7_cached_in(bc7_cache_dir(), rgba, width, height, levels)
}
fn bc7_cached_in(dir: Option<&std::path::Path>, rgba: &[u8], width: u32, height: u32, levels: u32) -> Vec<u8> {
    let Some(dir) = dir else { return bc7_levels(rgba, width, height, levels) };
    let mut key = blake3::Hasher::new();
    key.update(BC7_CACHE_VERSION);
    for n in [width, height, levels] {
        key.update(&n.to_le_bytes());
    }
    key.update(rgba);
    let path = dir.join(format!("{}.bc7", key.finalize().to_hex()));
    let size: usize = (0..levels).map(|l| (((width >> l).max(1).div_ceil(4)) * ((height >> l).max(1).div_ceil(4)) * 16) as usize).sum();
    // A file is its blocks and their hash: a short or damaged one is encoded again.
    if let Ok(file) = std::fs::read(&path)
        && file.len() == size + 32
        && blake3::hash(&file[..size]).as_bytes()[..] == file[size..]
    {
        // The least recently used go first when the cache is pruned.
        let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
        let mut blocks = file;
        blocks.truncate(size);
        return blocks;
    }
    let mut blocks = bc7_levels(rgba, width, height, levels);
    if blocks.len() == size {
        // Written whole under a name of its own, then renamed: a reader never sees half a file.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pending = path.with_extension(format!("{}-{n}.tmp", std::process::id()));
        let hash = blake3::hash(&blocks);
        blocks.extend_from_slice(hash.as_bytes());
        if std::fs::write(&pending, &blocks).and_then(|()| std::fs::rename(&pending, &path)).is_err() {
            let _ = std::fs::remove_file(&pending);
        }
        blocks.truncate(size);
    }
    blocks
}
/// Removes the least recently used blocks until the cache is within `cap` bytes, and temporary files a crashed run left.
fn prune_bc7_cache(dir: &std::path::Path, cap: u64) {
    let now = std::time::SystemTime::now();
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.len(), meta.modified().ok()?))
        })
        .filter(|(path, _, modified)| {
            let stale = path.extension().is_some_and(|e| e == "tmp")
                && now.duration_since(*modified).is_ok_and(|age| age.as_secs() > 3600);
            !(stale && std::fs::remove_file(path).is_ok())
        })
        .filter(|(path, ..)| path.extension().is_some_and(|e| e == "bc7"))
        .collect();
    files.sort_by_key(|f| f.2);
    let mut used: u64 = files.iter().map(|f| f.1).sum();
    for (path, len, _) in files {
        if used <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            used -= len;
        }
    }
}

/// A cutout texture: some texels clear, under 5% part-transparent (Skate 3's TRAFFIC_TIRE_Worlds 2%, its glass 78-94%).
/// Bug 10-17 #7: blended, a cutout writes no depth, so a parked car's black hub cards drew over its tyres and arches.
/// ponytail: a texel count, not retail's state (Skate 3's environment.transparent alpha state is UNTRACED).
fn cutout(texture: &skate_data::skate_map::Texture) -> bool {
    let alpha = texture.rgba.iter().skip(3).step_by(4);
    let (clear, partial) = alpha.fold((0usize, 0usize), |(c, p), &a| (c + usize::from(a <= 25), p + usize::from((26..230).contains(&a))));
    clear > 0 && partial * 20 < texture.rgba.len() / 4
}

fn role_image(source: &skate_data::skate_map::Texture, role: u8, map_version: u8) -> Image {
    let cube = role == 5;
    let height = if cube {
        source.height / 6
    } else {
        source.height
    };
    let layers = if cube { 6 } else { 1 };
    let srgb = role == 0 && source.color_space == 1 && !(2..=3).contains(&map_version);
    let format = if srgb {
        TextureFormat::Rgba8UnormSrgb
    } else {
        TextureFormat::Rgba8Unorm
    };
    // Texture Quality (read at load, like the anisotropy): Low and Medium halve the world's textures (the same texels
    // as dropping the top mips), the clamped ones too: on University they are 1268 of 1960 and most of the texels.
    // The PBR fallback's (role 0) are the converted maps' own and stay.
    // Not retail (user 10-19): Performance mode shrinks every role, under 128 a side (texture_steps); the BC7 cache
    // keys by the shrunk texels and size, so each level keeps its own blocks.
    let (rgba, width, height) = if role == 0 && !crate::graphics_menu::performance() {
        (source.rgba.clone(), source.width, height)
    } else {
        crate::retail_render::shrink(&source.rgba, source.width, height, layers, crate::graphics_menu::texture_steps())
    };
    let (bytes, levels) = if role == 3 || role == 6 || cube {
        crate::retail_render::mip_chain(&rgba, width, height, layers)
    } else {
        (rgba, 1)
    };
    let mut image = texture_image(bytes, width, height, layers, levels, format);
    if cube {
        image.texture_view_descriptor =
            Some(bevy::render::render_resource::TextureViewDescriptor {
                dimension: Some(bevy::render::render_resource::TextureViewDimension::Cube),
                ..default()
            });
    }
    let mut sampler = bevy::image::ImageSamplerDescriptor::linear();
    // ponytail: read at load, so the setting reaches world textures on the next map load.
    sampler.anisotropy_clamp = crate::graphics_menu::ANISOTROPY.load(std::sync::atomic::Ordering::Relaxed);
    if role != 4 && role != 6 && !cube {
        sampler.address_mode_u = bevy::image::ImageAddressMode::Repeat;
        sampler.address_mode_v = bevy::image::ImageAddressMode::Repeat;
    }
    image.sampler = ImageSampler::Descriptor(sampler);
    image
}

/// Converts map textures on worker threads while the caller builds batches. A
/// request reserves the handle and queues the work; finished images reach the
/// sink on the caller's thread at `pump`/`finish`, so a streaming sink uploads
/// them during the load instead of after the last batch.
struct TextureQueue<'m, 's, S: crate::map_render::AssetSink<Image>> {
    map: &'m SkateMap,
    ids: &'m [u32],
    images: &'s mut S,
    cache: HashMap<(u32, u8), Handle<Image>>,
    handles: Vec<Handle<Image>>,
    /// None without worker threads: requests convert on this thread.
    work: Option<std::sync::mpsc::Sender<(usize, ImageJob<'m>)>>,
    done: std::sync::mpsc::Receiver<(usize, Image)>,
}
/// An image built on a worker: a map texture's conversion or a lightmap atlas page.
type ImageJob<'m> = Box<dyn FnOnce() -> Image + Send + 'm>;
impl<'m, 's, S: crate::map_render::AssetSink<Image>> TextureQueue<'m, 's, S> {
    /// Up to 8 workers, leaving two cores for the running game.
    fn start<'scope>(
        scope: &'scope std::thread::Scope<'scope, 'm>,
        map: &'m SkateMap,
        ids: &'m [u32],
        images: &'s mut S,
    ) -> Self {
        let (work, jobs) = std::sync::mpsc::channel::<(usize, ImageJob<'m>)>();
        let jobs = std::sync::Arc::new(std::sync::Mutex::new(jobs));
        let (finished, done) = std::sync::mpsc::channel();
        let workers = load_workers();
        let mut spawned = 0;
        for _ in 0..workers {
            let (jobs, finished) = (jobs.clone(), finished.clone());
            let worker = std::thread::Builder::new()
                .name("map-texture-convert".into())
                .spawn_scoped(scope, move || {
                    loop {
                        // Ends once the queue is closed and empty.
                        let job = jobs
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        let Ok((index, job)) = job else { break };
                        if finished.send((index, job())).is_err() {
                            break;
                        }
                    }
                });
            match worker {
                Ok(_) => spawned += 1,
                Err(error) => warn!("Texture conversion worker: {error}"),
            }
        }
        Self {
            map,
            ids,
            images,
            cache: HashMap::new(),
            handles: Vec::new(),
            work: (spawned > 0).then_some(work),
            done,
        }
    }
    /// Material slot `id`'s texture is a cutout (`cutout`).
    fn cutout(&self, id: u32) -> bool {
        let id = self.ids[id as usize];
        id != 0 && cutout(&self.map.textures[id as usize - 1])
    }
    /// Texture record `id` (a material slot) in `role`'s format.
    fn request(&mut self, id: u32, role: u8) -> Option<Handle<Image>> {
        let id = self.ids[id as usize];
        if id == 0 {
            return None;
        }
        if role == 5
            && self.map.textures[id as usize - 1].height
                != self.map.textures[id as usize - 1].width * 6
        {
            // Older .skate exports contain only face zero. Never treat it as a cube.
            return None;
        }
        if let Some(handle) = self.cache.get(&(id, role)) {
            return Some(handle.clone());
        }
        let map = self.map;
        let handle = self.submit(Box::new(move || role_image(&map.textures[id as usize - 1], role, map.version)));
        self.cache.insert((id, role), handle.clone());
        Some(handle)
    }
    /// Reserves the image's handle and queues its build (on this thread without workers).
    fn submit(&mut self, job: ImageJob<'m>) -> Handle<Image> {
        let handle = self.images.reserve();
        self.handles.push(handle.clone());
        let job = (self.handles.len() - 1, job);
        let unsent = match &self.work {
            Some(work) => work
                .send(job)
                .err()
                .map(|std::sync::mpsc::SendError(job)| job),
            None => Some(job),
        };
        if let Some((index, job)) = unsent {
            self.images.insert(self.handles[index].clone(), job());
        }
        handle
    }
    /// Hand finished images to the sink.
    fn pump(&mut self) {
        while let Ok((index, image)) = self.done.try_recv() {
            self.images.insert(self.handles[index].clone(), image);
        }
    }
    /// Close the queue and hand over the rest as the workers finish.
    fn finish(mut self) {
        self.work = None;
        while let Ok((index, image)) = self.done.recv() {
            self.images.insert(self.handles[index].clone(), image);
        }
    }
}

/// A map batch vertex's identity: its source index, or (SKATE_FPS_WELD) the bits of every field a batch attribute is
/// built from (position, normal, both UVs, the lightmap page's material, decal UV, tangent frame).
#[derive(Hash, PartialEq, Eq)]
enum Corner {
    Index(u32),
    Bits(([u32; 10], u32, Option<[u32; 2]>, Option<[u8; 4]>)),
}
fn corner_bits(v: &skate_data::skate_map::Vertex) -> ([u32; 10], u32, Option<[u32; 2]>, Option<[u8; 4]>) {
    let [p, n, uv, lm] = [&v.position[..], &v.normal[..], &v.uv[..], &v.lightmap_uv[..]];
    let mut bits = [0; 10];
    for (b, f) in bits.iter_mut().zip(p.iter().chain(n).chain(uv).chain(lm)) {
        *b = f.to_bits();
    }
    (bits, v.material, v.decal_uv.map(|d| d.map(f32::to_bits)), v.tangent_frame)
}

/// perf 11-01 (SKATE_PACK_VERTICES): a retail batch's normals and tangents as Snorm16x4 and its colour (decal UVs, zero
/// elsewhere) as Float32x2, 52 bytes a vertex instead of 72: the Skate 1/2 cities' ~11 M vertices take ~220 MB less
/// video memory and every pass fetches less. The attribute ids stay, so Bevy's pipelines bind them where they did; the
/// GPU widens them to the shaders' vec3/vec4 (a colour's missing z, w read 0, 1, the decal UVs' own z, w). Directions are
/// normalized first: the shaders normalize them anyway, and a Snorm16 step is 1/32767.
fn pack_retail_vertices(mesh: &mut Mesh) {
    use bevy::mesh::{MeshVertexAttribute, VertexAttributeValues as V, VertexFormat};
    const NORMAL: MeshVertexAttribute = MeshVertexAttribute { format: VertexFormat::Snorm16x4, ..Mesh::ATTRIBUTE_NORMAL };
    const TANGENT: MeshVertexAttribute = MeshVertexAttribute { format: VertexFormat::Snorm16x4, ..Mesh::ATTRIBUTE_TANGENT };
    const COLOR: MeshVertexAttribute = MeshVertexAttribute { format: VertexFormat::Float32x2, ..Mesh::ATTRIBUTE_COLOR };
    let snorm = |v: f32| (v.clamp(-1., 1.) * 32767.).round() as i16;
    if let Some(V::Float32x3(normals)) = mesh.remove_attribute(Mesh::ATTRIBUTE_NORMAL) {
        let packed = normals.iter().map(|&n| Vec3::from_array(n).normalize_or_zero()).map(|n| [snorm(n.x), snorm(n.y), snorm(n.z), 0]);
        mesh.insert_attribute(NORMAL, V::Snorm16x4(packed.collect()));
    }
    if let Some(V::Float32x4(tangents)) = mesh.remove_attribute(Mesh::ATTRIBUTE_TANGENT) {
        let packed = tangents.iter().map(|&[x, y, z, w]| {
            let t = Vec3::new(x, y, z).normalize_or_zero();
            [snorm(t.x), snorm(t.y), snorm(t.z), snorm(w)]
        });
        mesh.insert_attribute(TANGENT, V::Snorm16x4(packed.collect()));
    }
    if let Some(V::Float32x4(colours)) = mesh.remove_attribute(Mesh::ATTRIBUTE_COLOR) {
        mesh.insert_attribute(COLOR, V::Float32x2(colours.iter().map(|&[u, v, ..]| [u, v]).collect()));
    }
}

pub(crate) fn spawn(
    map: &SkateMap,
    commands: &mut crate::map_render::SceneCommands,
    meshes: &mut impl crate::map_render::AssetSink<Mesh>,
    materials: &mut impl crate::map_render::AssetSink<crate::retail_render::PropMaterial>,
    retail_materials: &mut impl crate::map_render::AssetSink<crate::retail_render::RetailWorldMaterial>,
    blend_materials: &mut impl crate::map_render::AssetSink<crate::retail_render::RetailBlendMaterial>,
    images: &mut impl crate::map_render::AssetSink<Image>,
    tuning: &crate::retail_render::MaterialTuning,
) {
    spawn_instances(
        map,
        &[],
        commands,
        meshes,
        materials,
        retail_materials,
        blend_materials,
        images,
        tuning,
    );
}

/// `instances[material - 1]` names the movable prop a package material draws;
/// those batches are cut per prop and tagged `PropPiece` so they can move.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_instances(
    map: &SkateMap,
    instances: &[Option<u32>],
    commands: &mut crate::map_render::SceneCommands,
    meshes: &mut impl crate::map_render::AssetSink<Mesh>,
    materials: &mut impl crate::map_render::AssetSink<crate::retail_render::PropMaterial>,
    retail_materials: &mut impl crate::map_render::AssetSink<crate::retail_render::RetailWorldMaterial>,
    blend_materials: &mut impl crate::map_render::AssetSink<crate::retail_render::RetailBlendMaterial>,
    images: &mut impl crate::map_render::AssetSink<Image>,
    tuning: &crate::retail_render::MaterialTuning,
) {
    // Texture roles have different transfer functions even when sharing a record.
    let _span = info_span!("prepare_map_geometry_and_textures").entered();
    let texture_ids = info_span!("texture_ids").in_scope(|| render_texture_ids(&map.textures));
    let definitions = info_span!("material_definitions").in_scope(|| Definitions::parse(&map.materials));
    let mut atlas = info_span!("lightmap_atlas").in_scope(|| {
        let ids = material_ids_with(&map.materials, &texture_ids, true, &definitions.ids);
        lightmap_atlas::build_with(map, &texture_ids, &ids, tuning, &definitions.parsed)
    });
    let atlas_pages = std::mem::take(&mut atlas.pages);
    // perf 10-19: SKATE_UNIFIED_LAYOUT=0 gives retail batches only the tangents and colours their shading reads.
    let unified_layout = crate::config::perf_on("SKATE_UNIFIED_LAYOUT");
    // Texture conversion was most of this function's time; it runs on worker
    // threads alongside batch building (TextureQueue).
    std::thread::scope(|scope| {
        let mut textures = TextureQueue::start(scope, map, &texture_ids, images);
        // The atlas pages, the biggest images, are composed and compressed first in the workers' queue (perf 10-12:
        // they took ~1 s on this thread before any texture started). Their handles come first, as before.
        let pages: Vec<_> = atlas_pages.into_iter().map(|page| textures.submit(Box::new(move || page.image(map)))).collect();
        // Lightmaps belong to mesh entities, not StandardMaterial. Distinct baked
        // lighting still needs separate geometry batches but can share a PBR material.
        let pbr_ids = info_span!("pbr_ids").in_scope(|| material_ids_with(&map.materials, &texture_ids, false, &definitions.ids));
        let grouping = info_span!("group_batches").entered();
        let groups = consolidated_groups_with(
            map,
            &texture_ids,
            tuning,
            &mut |id, role| textures.request(id, role),
            &atlas,
            &pages,
            &definitions,
            unified_layout,
            merge_blend(),
        );
        let groups = split_instances(map, groups, instances);
        drop(grouping);
        let building = info_span!("build_batches").entered();
        eprintln!(
            "SKATE_RENDER_BATCHES count={} triangles={}",
            groups.len(),
            map.geometry.indices.len() / 3
        );
        let mut material_handles: Vec<Option<Handle<crate::retail_render::PropMaterial>>> =
            vec![None; map.materials.len()];
        let mut retail_handles = HashMap::new();
        let mut blend_handles = HashMap::new();
        // perf 11-01 (SKATE_SHADOW_PROXIES=1, off by default): opaque static batches cast through 64 m position-only
        // proxies (retail_shadow_geometry.rs).
        let mut shadows = shadow_geometry::enabled().then(shadow_geometry::ShadowGeometry::default);
        for (
            instance,
            RenderGroup {
                material: material_index,
                indices,
                retail,
                tangents: tangent_mode,
            },
        ) in groups
        {
            textures.pump();
            let m = &map.materials[material_index];
            // Bug 10-25 #36: Skate 2's godray.spotlight beams add in their own pass (retail_godray.rs), not as a batch.
            if let Some(d) = definitions.parsed[material_index].as_ref().filter(|d| d.shader == crate::retail_render::godray::SHADER) {
                let texture = d.bindings.get("diffuse").map_or(m.textures[0], |b| b.texture);
                // Role 3: the retail diffuse's (linear, mipped, repeating), as Definition::build fetches it.
                if let Some(diffuse) = textures.request(texture, 3) {
                    let corners = crate::retail_render::godray::corners(indices.iter().map(|&i| &map.geometry.vertices[i as usize]));
                    commands.spawn((
                        Name::new(m.name.clone()),
                        crate::retail_render::godray::Godray { corners, diffuse },
                        Transform::default(),
                        Visibility::default(),
                    ));
                }
                continue;
            }
            // Reindex each batch, preserving authored normals and both UV sets. FPS pass 10-23 (SKATE_FPS_WELD): one
            // vertex per distinct set of attribute bits, so a corner written once per triangle (Skate 1 San Vanelona
            // ran ~2.6 vertex shaders a triangle) is shaded once; triangle order and every corner's values stay.
            // Generated tangents keep one vertex per source index: mikktspace's output depends on the sharing.
            let weld = crate::config::perf_on("SKATE_FPS_WELD") && tangent_mode != 1;
            let mut remap = HashMap::new();
            let mut vertices = Vec::new();
            let local: Vec<u32> = indices
                .iter()
                .map(|&index| {
                    let v = &map.geometry.vertices[index as usize];
                    let key = if weld { Corner::Bits(corner_bits(v)) } else { Corner::Index(index) };
                    *remap.entry(key).or_insert_with(|| {
                        let id = vertices.len() as u32;
                        vertices.push(v);
                        id
                    })
                })
                .collect();
            // Static batches never change their mesh, so they carry their own bounds (NoAutoAabb). A per-frame mesh
            // write elsewhere (Skate 1's minimap) then no longer has calculate_bounds and material specialization
            // re-check every map batch (perf 10-10 B2a, ~0.85 ms a frame on Skate 1).
            let bounds = bevy::camera::primitives::Aabb::enclosing(vertices.iter().map(|v| Vec3::from_array(v.position)))
                .unwrap_or_default();
            let mut mesh = Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vertices.iter().map(|v| v.position).collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                vertices.iter().map(|v| v.normal).collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_UV_0,
                vertices.iter().map(|v| v.uv).collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_UV_1,
                vertices
                    .iter()
                    .map(|v| atlas.uv((v.material as usize).saturating_sub(1), v.lightmap_uv))
                    .collect::<Vec<_>>(),
            )
            .with_inserted_indices(bevy::mesh::Indices::U32(local));
            // perf 10-19 (SKATE_UNIFIED_LAYOUT): every retail batch has one vertex layout (position, normal, both UVs,
            // tangent, colour), so the mesh allocator keeps them in one slab class and their draws multi-draw
            // together. retail_world.wgsl reads the colour only under samples_decal_uv and takes a zero tangent's
            // derivative frame (mesh_tangent_local_to_world passes zero through). PBR fallbacks keep theirs: Bevy's
            // pbr_fragment.wgsl replaces base colour with a vertex colour, and its VERTEX_TANGENTS skips the
            // back-face normal flip (pbr_functions.wgsl prepare_world_normal).
            let unified = unified_layout && retail.is_some();
            let zeros = || vec![[0f32; 4]; vertices.len()];
            // Vertex colours carry retail decal coordinates, never PBR tint.
            let decal = retail.as_ref().is_some_and(|m| m.samples_decal_uv());
            if unified && !decal {
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, zeros());
            } else if decal {
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_COLOR,
                    vertices
                        .iter()
                        .map(|v| {
                            let uv = v.decal_uv.unwrap_or(v.uv);
                            [uv[0], uv[1], 0., 1.]
                        })
                        .collect::<Vec<_>>(),
                );
            }
            // A unified framed batch may hold frameless sources (consolidated_groups_with): zero where none.
            if vertices.iter().all(|v| v.tangent_frame.is_some()) || (unified && tangent_mode == 0) {
                let tangents: Vec<[f32; 4]> = vertices
                    .iter()
                    .map(|v| {
                        v.tangent_frame.map_or([0.; 4], |frame| {
                            let frame = frame.map(|b| (b as i8 as f32 / 127.).max(-1.));
                            let binormal = Vec3::new(frame[0], frame[1], frame[2]);
                            let tangent = binormal.cross(Vec3::from_array(v.normal)) * frame[3];
                            [tangent.x, tangent.y, tangent.z, frame[3]]
                        })
                    })
                    .collect();
                mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
            } else if m.textures[2] != 0 {
                if let Err(error) = mesh.generate_tangents() {
                    warn!("SKATE material {} tangent generation: {error}", m.name);
                    if unified {
                        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, zeros());
                    }
                }
            } else if unified {
                mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, zeros());
            }
            if let Some(material) = retail {
                if crate::config::perf_on("SKATE_PACK_VERTICES") {
                    pack_retail_vertices(&mut mesh);
                }
                // Cells of one split batch (and equal merged batches) share a material; equal blended ones too, so
                // the sorted transparent pass rebinds less.
                let mesh = Mesh3d(meshes.add(mesh));
                let screen = material.diffuse.as_ref().is_some_and(|d| d.id() == crate::retail_render::FEED.id());
                let mut entity = if material.batch_key().is_some() {
                    let no_prepass = material.camera_prepass_unsafe();
                    let proxied = instance.is_none() && shadow_geometry::eligible(&material);
                    let two_sided = material.two_sided;
                    let material = retail_handles
                        .entry(material.material_key())
                        .or_insert_with(|| retail_materials.add(material))
                        .clone();
                    let mut entity = commands.spawn((Name::new(m.name.clone()), mesh, MeshMaterial3d(material.clone()), Transform::default()));
                    if let Some(shadows) = shadows.as_mut().filter(|_| proxied) {
                        shadows.add(&map.geometry, &indices, &material, two_sided);
                        entity.insert(bevy::light::NotShadowCaster);
                    }
                    if no_prepass {
                        entity.insert(crate::retail_render::shadow_visibility::NoCameraPrepass);
                    }
                    entity
                } else {
                    let material = blend_handles
                        .entry(material.material_key())
                        .or_insert_with(|| blend_materials.add(crate::retail_render::RetailBlendMaterial(material)))
                        .clone();
                    commands.spawn((Name::new(m.name.clone()), mesh, MeshMaterial3d(material), Transform::default()))
                };
                if screen {
                    entity.insert(crate::retail_render::JumbotronScreen); // #33
                }
                if let Some(instance) = instance {
                    entity.insert((crate::physics::props::PropPiece(instance), Visibility::Hidden)); // until props::present places it (G8 M2)
                } else {
                    // Static: the shadow lights find it through the retained index (perf 10-10 #3); props move, so
                    // they stay on the per-frame walk.
                    entity.insert((
                        crate::retail_render::shadow_visibility::StaticShadowCaster,
                        bounds,
                        bevy::camera::visibility::NoAutoAabb,
                    ));
                }
                continue;
            }
            let material = material_handles[pbr_ids[material_index]]
                .get_or_insert_with(|| {
                    let orm = textures.request(m.textures[3], 2);
                    // PBR fallbacks and props: StandardMaterial plus the weather snow layer.
                    materials.add(crate::retail_render::PropMaterial {
                        base: StandardMaterial {
                            base_color: Color::linear_rgb(m.color[0], m.color[1], m.color[2]),
                            base_color_texture: textures.request(m.textures[0], 0),
                            normal_map_texture: textures.request(m.textures[2], 2),
                            metallic_roughness_texture: orm.clone(),
                            occlusion_texture: orm,
                            metallic: if m.textures[3] != 0 { 1. } else { 0. },
                            perceptual_roughness: m.roughness,
                            emissive: LinearRgba::rgb(
                                m.color[0] * m.emissive,
                                m.color[1] * m.emissive,
                                m.color[2] * m.emissive,
                            ),
                            emissive_texture: textures.request(m.textures[4], 0),
                            alpha_mode: match m.alpha_mode {
                                1 => AlphaMode::Mask(m.alpha_cutoff),
                                2 if textures.cutout(m.textures[0]) => AlphaMode::Mask(0.5),
                                2 => AlphaMode::Blend,
                                _ => AlphaMode::Opaque,
                            },
                            lightmap_exposure: m.indirect_strength,
                            ..default()
                        },
                        extension: default(),
                    })
                })
                .clone();
            let mut entity = commands.spawn((
                Name::new(m.name.clone()),
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material),
                Transform::default(),
            ));
            if let Some(instance) = instance {
                entity.insert((crate::physics::props::PropPiece(instance), Visibility::Hidden)); // until props::present places it (G8 M2)
            } else {
                entity.insert((
                    crate::retail_render::shadow_visibility::StaticShadowCaster,
                    bounds,
                    bevy::camera::visibility::NoAutoAabb,
                ));
            }
            // UV_1 above is already in atlas space when the lightmap has a tile.
            let lightmap = match atlas.page(material_index) {
                Some(page) => pages.get(page as usize).cloned(),
                None => textures.request(m.textures[1], 4),
            };
            if let Some(image) = lightmap {
                entity.insert(bevy::pbr::Lightmap {
                    image,
                    uv_rect: Rect::new(0., 0., 1., 1.),
                    bicubic_sampling: false,
                });
            }
        }
        if let Some(shadows) = shadows {
            info_span!("shadow_proxies").in_scope(|| shadows.spawn(&map.geometry, commands, meshes));
        }
        drop(building);
        info_span!("finish_textures").in_scope(|| textures.finish());
    });
    for light in &map.lights {
        let color = Color::linear_rgb(light.color[0], light.color[1], light.color[2]);
        let transform = Transform::from_translation(Vec3::from_array(light.position));
        match light.kind {
            0 => {
                commands.spawn((
                    Name::new(light.name.clone()),
                    PointLight {
                        color,
                        intensity: light.intensity,
                        range: light.range,
                        radius: light.radius,
                        ..default()
                    },
                    transform,
                ));
            }
            1 => {
                commands.spawn((
                    Name::new(light.name.clone()),
                    SpotLight {
                        color,
                        intensity: light.intensity,
                        range: light.range,
                        radius: light.radius,
                        inner_angle: light.inner_cos.acos(),
                        outer_angle: light.outer_cos.acos(),
                        ..default()
                    },
                    transform.looking_to(Vec3::from_array(light.direction), Vec3::Y),
                ));
            }
            _ => {}
        }
    }
    commands.insert_resource(ClearColor(Color::linear_rgb(
        map.environment[3],
        map.environment[4],
        map.environment[5],
    )));
    info!(
        "SKATE_WORLD_READY name={:?} render_triangles={} collision_triangles={}",
        map.name,
        map.geometry.indices.len() / 3,
        map.geometry.collision.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug 10-17 #7: a cutout (a tyre's clear hub card) is alpha-tested; glass, mostly part-transparent, still blends.
    #[test]
    fn cutouts_are_told_from_glass() {
        let texture = |alphas: &[u8]| skate_data::skate_map::Texture {
            name: String::new(),
            width: alphas.len() as u32,
            height: 1,
            color_space: 0,
            rgba: alphas.iter().flat_map(|&a| [0, 0, 0, a]).collect(),
        };
        let mut tyre = vec![255; 70];
        tyre.extend([0; 28]);
        tyre.extend([128; 2]);
        assert!(cutout(&texture(&tyre)));
        assert!(!cutout(&texture(&[100; 100])), "glass");
        assert!(!cutout(&texture(&[255; 100])), "opaque: nothing to cut");
    }

    /// C1: retail's teleport probe (0x82598428) grounds a target that sits inside the face, or above it, at the
    /// face + 0.15 m, keeps the basis, and leaves a target with nothing within 500 m under it alone.
    #[test]
    fn the_teleport_probe_puts_a_target_0_15_m_over_the_face_under_it() {
        let material = skate_core::physics::contact::RetailContactMaterial {
            static_friction: 0.5,
            dynamic_friction: 0.5,
            restitution: 0.,
        };
        let world = crate::physics::ground::flat_world(material);
        let floor = crate::physics::ground::HEIGHT;
        let at = |y: f32| [[0., 0., -1., 0.], [0., 1., 0., 0.], [1., 0., 0., 0.], [3., y, -4., 0.]];
        for (y, label) in [(floor - 0.235, "inside the face (ots_univ_04_ol)"), (floor + 0.6, "above it")] {
            let (probed, face) = probe_ground(&world, at(y));
            assert!(face.is_some_and(|f| (f - floor).abs() < 1e-4), "{label}: {face:?}");
            let want = [3., floor + 0.15, -4., 0.];
            assert!(probed[3].iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-4), "{label}: {:?}", probed[3]);
            assert_eq!(probed[..3], at(y)[..3], "{label}: basis kept");
        }
        // 0.75 m is the reach above the target: a face 0.8 m over it is not its ground.
        assert_eq!(probe_ground(&world, at(floor - 0.8)), (at(floor - 0.8), None));
        assert_eq!(probe_ground(&world, at(floor + 501.)), (at(floor + 501.), None));
    }

    /// G8 I5: the Skate 2 tutorial's borrowed DMOs keep only their own vertices and lightmaps (whole source package vs the cut).
    #[test]
    #[ignore = "requires SKATE_ASSET_ROOT with private/skate2-tutorial/dmos.json"]
    fn staged_tutorial_props_pack_only_their_own_lightmaps() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE_ASSET_ROOT").unwrap());
        let tuning = crate::retail_render::MaterialTuning::load(&root);
        let atlas = |map: &SkateMap| {
            let ids = render_texture_ids(&map.textures);
            let atlas = lightmap_atlas::build(map, &ids, &render_material_ids(&map.materials, &ids), &tuning);
            (atlas.pages.len(), atlas.pages.iter().map(|p| p.pixels(map).len()).sum::<usize>())
        };
        let started = std::time::Instant::now();
        let (staged, _) = crate::physics::props::staged_package(&root, "New_San_Vanelona").unwrap();
        let cut = started.elapsed();
        let path = root.join("private/native-props/DownTown.skate");
        let whole = SkateMap::parse_render_only(&std::fs::read(path).unwrap()).unwrap();
        let (before, after) = (atlas(&whole), atlas(&staged));
        println!(
            "I5 atlas (pages, bytes) whole={before:?} staged={after:?}; vertices whole={} staged={}; staged_package {cut:?}",
            whole.geometry.vertices.len(),
            staged.geometry.vertices.len()
        );
        // ponytail: today's DownTown DMOs carry no lightmap (both atlases are empty); this guards the cut if one ever does.
        assert!(after.1 <= before.1 && staged.geometry.vertices.len() < whole.geometry.vertices.len());
    }

    fn compare_cluster_queries(world: &BoardWorld) -> (usize, usize) {
        let mut metadata = world.query_metadata().unwrap().clone();
        let mut single = metadata.meshes[0].clone();
        single.triangle_range = 0..world.triangles().len();
        single.local_bounds =
            Bounds::from_points(world.triangles().iter().flat_map(|t| t.vertices))
                .unwrap();
        metadata.meshes = vec![single];
        let baseline =
            BoardWorld::with_query_metadata(world.triangles().to_vec(), metadata).unwrap();
        let mut visited = [0usize; 2];
        let mut times = [std::time::Duration::ZERO; 2];
        for triangle in world
            .triangles()
            .iter()
            .step_by((world.triangles().len() / 256).max(1))
        {
            let p = triangle.vertices[0];
            let start = Vector3::new(p.x, p.y + 1., p.z);
            let end = Vector3::new(p.x, p.y - 1., p.z);
            let mut candidates = Vec::new();
            for (i, scene) in [&baseline, world].into_iter().enumerate() {
                let bounds = scene.line_candidate_bounds(start, end, 0.1);
                visited[i] += scene
                    .candidate_ranges(bounds)
                    .iter()
                    .map(|r| r.len())
                    .sum::<usize>();
                let now = std::time::Instant::now();
                candidates.push(
                    scene
                        .line_candidates(start, end, 0.1)
                        .map(|(id, _)| id)
                        .collect::<Vec<_>>(),
                );
                times[i] += now.elapsed();
            }
            assert_eq!(
                candidates[0], candidates[1],
                "candidate identity/order changed"
            );
        }
        eprintln!(
            "CUSTOM_QUERY_BENCH baseline_triangles={} clustered_triangles={} baseline_ms={} clustered_ms={} clusters={}",
            visited[0],
            visited[1],
            times[0].as_secs_f64() * 1000.,
            times[1].as_secs_f64() * 1000.,
            world.query_metadata().unwrap().meshes.len()
        );
        (visited[0], visited[1])
    }

    #[test]
    fn portable_collision_clusters_preserve_queries_and_cull_distant_geometry() {
        let mut map = demo();
        let source = map.geometry.collision.remove(0);
        map.geometry.collision.clear();
        for i in 0..256 {
            map.geometry
                .collision
                .push(skate_data::skate_map::Collision {
                    points: source.points.map(|p| [p[0] + i as f32 * 100., p[1], p[2]]),
                    surface: source.surface,
                    material: source.material,
                    native_edges: source.native_edges,
                });
        }
        let world = collision_world(&map, material()).unwrap();
        assert_eq!(world.query_metadata().unwrap().meshes.len(), 4);
        let (before, after) = compare_cluster_queries(&world);
        assert!(after < before / 2);
    }

    #[test]
    #[ignore = "requires a supplied map; static collision-query benchmark only"]
    fn supplied_custom_map_collision_query_benchmark() {
        let path = std::env::var("SKATE_TEST_CUSTOM_MAP").unwrap();
        let map = SkateMap::load(std::path::Path::new(&path)).unwrap();
        let world = collision_world(&map, material()).unwrap();
        let (before, after) = compare_cluster_queries(&world);
        assert!(after < before);
    }
    fn demo() -> SkateMap {
        SkateMap::parse(include_bytes!("../../../maps/format-demo.skate")).unwrap()
    }
    fn retail_demo() -> SkateMap {
        fn definition(unused: &str) -> Vec<u8> {
            fn text(bytes: &mut Vec<u8>, value: &str) {
                bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
                bytes.extend_from_slice(value.as_bytes());
            }
            let mut bytes = vec![0; 16];
            text(&mut bytes, "world.default");
            bytes.extend_from_slice(&1u32.to_le_bytes()); // supported family
            bytes.extend_from_slice(&0u32.to_le_bytes()); // one-sided
            bytes.extend_from_slice(&0u32.to_le_bytes()); // no explicit bindings
            bytes.extend_from_slice(&1u32.to_le_bytes());
            text(&mut bytes, "unused_source_property");
            bytes.extend_from_slice(&1u32.to_le_bytes());
            text(&mut bytes, unused);
            text(&mut bytes, "");
            bytes
        }
        let mut map = demo();
        map.materials[0].retail_definition = Some(definition("a"));
        map.materials[0].alpha_mode = 0;
        let mut second = demo().materials.remove(0);
        second.retail_definition = Some(definition("b"));
        second.alpha_mode = 0;
        second.roughness = 0.12345; // ignored by the retail shader
        map.materials.push(second);
        let original_vertices = map.geometry.vertices.len() as u32;
        let duplicates: Vec<_> = map
            .geometry
            .vertices
            .iter()
            .map(|v| {
                let mut v = v.clone();
                v.material = 2;
                v.position[0] += 3.;
                v
            })
            .collect();
        map.geometry.vertices.extend(duplicates);
        map.geometry.indices.extend(
            map.geometry
                .indices
                .clone()
                .into_iter()
                .map(|i| i + original_vertices),
        );
        for v in &mut map.geometry.vertices {
            v.tangent_frame = Some([0, 127, 0, 127]);
        }
        map
    }

    #[test]
    fn effective_batches_ignore_unused_metadata_but_preserve_lighting_and_blend_groups() {
        let mut map = retail_demo();
        let textures = render_texture_ids(&map.textures);
        let tuning = crate::retail_render::MaterialTuning::default();
        let mut texture = |id: u32, role: u8| {
            (id != 0).then(|| {
                Handle::Uuid(
                    bevy::asset::uuid::Uuid::from_u128(
                        (u128::from(textures[id as usize]) << 8) | u128::from(role),
                    ),
                    default(),
                )
            })
        };
        assert_eq!(
            render_groups(
                &map.geometry,
                &render_material_ids(&map.materials, &textures)
            )
            .len(),
            2
        );
        let groups = consolidated_groups(
            &map,
            &textures,
            &tuning,
            &mut texture,
            &Default::default(),
            &[],
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].indices.len(), map.geometry.indices.len());
        let original_lightmap = map.materials[1].textures[1];
        map.materials[1].textures[1] = if original_lightmap == 0 {
            map.materials[0].textures[0]
        } else {
            0
        };
        assert_eq!(
            consolidated_groups(
                &map,
                &textures,
                &tuning,
                &mut texture,
                &Default::default(),
                &[]
            )
            .len(),
            2
        );
        map.materials[1].textures[1] = original_lightmap;
        for m in &mut map.materials {
            m.alpha_mode = 2;
        }
        let groups = consolidated_groups(
            &map,
            &textures,
            &tuning,
            &mut texture,
            &Default::default(),
            &[],
        );
        assert_eq!(groups.len(), 2);
        assert!(
            groups
                .iter()
                .all(|g| !shadow_geometry::eligible(g.retail.as_ref().unwrap()))
        );
    }

    #[test]
    fn pbr_fallbacks_on_one_atlas_page_draw_as_one_batch_unless_blended() {
        let mut map = retail_demo();
        for m in &mut map.materials {
            // Family 0: unsupported by the retail shader, so both render through PBR.
            m.retail_definition.as_mut().unwrap()[33..37].copy_from_slice(&0u32.to_le_bytes());
            m.roughness = 0.5;
        }
        let textures = render_texture_ids(&map.textures);
        let tuning = crate::retail_render::MaterialTuning::default();
        let groups = |map: &SkateMap| {
            let atlas = lightmap_atlas::build(
                map,
                &textures,
                &render_material_ids(&map.materials, &textures),
                &tuning,
            );
            let pages: Vec<Handle<Image>> = atlas.pages.iter().map(|_| Handle::default()).collect();
            consolidated_groups(map, &textures, &tuning, &mut |_, _| None, &atlas, &pages)
        };
        // Different (unused) retail definitions keep them apart until batching.
        assert_eq!(
            render_groups(
                &map.geometry,
                &render_material_ids(&map.materials, &textures)
            )
            .len(),
            2
        );
        let merged = groups(&map);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].indices.len(), map.geometry.indices.len());
        for m in &mut map.materials {
            m.alpha_mode = 2;
        }
        assert_eq!(
            groups(&map).len(),
            2,
            "blended batches keep their sorting groups"
        );
    }
    #[test]
    fn bc7_levels_match_the_gpu_block_layout() {
        // 12x8 with its full chain: 6x4, 3x2 and 1x1 need padded blocks.
        let (rgba, levels) = crate::retail_render::mip_chain(&vec![200u8; 12 * 8 * 4], 12, 8, 1);
        let blocks: u32 = (0..levels)
            .map(|l| ((12u32 >> l).max(1).div_ceil(4)) * ((8u32 >> l).max(1).div_ceil(4)))
            .sum();
        assert_eq!(bc7_levels(&rgba, 12, 8, levels).len(), blocks as usize * 16);
    }
    /// perf 10-18: the BC7 disk cache gives the encoder's blocks, reads them back, re-encodes a damaged file and
    /// prunes the least recently used past its cap.
    #[test]
    fn bc7_cache_round_trips_and_survives_damage() {
        let dir = std::env::temp_dir().join(format!("s3r-bc7-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rgba: Vec<u8> = (0..16 * 8 * 4).map(|i| (i * 7) as u8).collect();
        let encoded = bc7_levels(&rgba, 16, 8, 1);
        let files = || std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect::<Vec<_>>();
        assert_eq!(bc7_cached_in(Some(&dir), &rgba, 16, 8, 1), encoded);
        let [file] = &files()[..] else { panic!("one file: {:?}", files()) };
        assert_eq!(std::fs::metadata(file).unwrap().len(), encoded.len() as u64 + 32);
        // A read: the file's (hash-checked) blocks, not an encode.
        let mut stored = std::fs::read(file).unwrap();
        stored[0] ^= 1;
        let hash = blake3::hash(&stored[..encoded.len()]);
        stored[encoded.len()..].copy_from_slice(hash.as_bytes());
        std::fs::write(file, &stored).unwrap();
        assert_eq!(bc7_cached_in(Some(&dir), &rgba, 16, 8, 1), stored[..encoded.len()]);
        // Damaged: encoded again and rewritten.
        std::fs::write(file, &encoded[..10]).unwrap();
        assert_eq!(bc7_cached_in(Some(&dir), &rgba, 16, 8, 1), encoded);
        assert_eq!(std::fs::metadata(file).unwrap().len(), encoded.len() as u64 + 32);
        // Another texture, then a cap of one file: the older goes.
        let other: Vec<u8> = rgba.iter().map(|b| b.wrapping_add(1)).collect();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(file).unwrap().set_modified(old).unwrap();
        bc7_cached_in(Some(&dir), &other, 16, 8, 1);
        prune_bc7_cache(&dir, encoded.len() as u64 + 32);
        assert!(!file.exists() && files().len() == 1, "{:?}", files());
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn wide_batches_split_into_cells_keeping_every_triangle() {
        let mut map = demo();
        for v in &mut map.geometry.vertices {
            v.position = v.position.map(|p| p * 1e5);
        }
        map.materials[0].textures[2] = 0;
        let whole = || {
            vec![RenderGroup {
                material: 0,
                indices: map.geometry.indices.clone(),
                retail: None,
                tangents: 2, // no frames, no normal map: none
            }]
        };
        let split = split_large(&map, whole());
        assert!(split.len() > 1);
        let mut before: Vec<_> = map
            .geometry
            .indices
            .chunks_exact(3)
            .map(|t| t.to_vec())
            .collect();
        let mut after: Vec<_> = split
            .iter()
            .flat_map(|g| g.indices.chunks_exact(3).map(|t| t.to_vec()))
            .collect();
        before.sort();
        after.sort();
        assert_eq!(before, after);
        map.materials[0].alpha_mode = 2;
        assert_eq!(
            split_large(&map, whole()).len(),
            1,
            "blended batches keep their sorting groups"
        );
    }
    /// perf 10-19 (SKATE_UNIFIED_LAYOUT): a material's frameless batch merges with its framed one, keeping every
    /// triangle once; every static retail batch then has one vertex layout, frameless vertices zero tangents.
    #[test]
    fn unified_layout_merges_frameless_batches_and_shares_one_vertex_layout() {
        let mut map = retail_demo();
        for m in &mut map.materials {
            m.textures[2] = 0;
        }
        for v in map.geometry.vertices.iter_mut().filter(|v| v.material == 2) {
            v.tangent_frame = None;
        }
        let textures = render_texture_ids(&map.textures);
        let tuning = crate::retail_render::MaterialTuning::default();
        let definitions = Definitions::parse(&map.materials);
        let groups = |unified| {
            let atlas = Default::default();
            consolidated_groups_with(&map, &textures, &tuning, &mut |_, _| None, &atlas, &[], &definitions, unified, false)
        };
        let triangles = |groups: Vec<RenderGroup>| {
            let mut t: Vec<Vec<u32>> =
                groups.iter().flat_map(|g| g.indices.chunks_exact(3).map(<[u32]>::to_vec)).collect();
            t.sort();
            t
        };
        assert_eq!((groups(false).len(), groups(true).len()), (2, 1));
        assert_eq!(triangles(groups(false)), triangles(groups(true)));
        if !crate::config::perf_on("SKATE_UNIFIED_LAYOUT") {
            return;
        }
        // A decal (colours) without frames beside a framed batch (tangents): two layouts before.
        map.materials[1].retail_definition = Some(decal_definition());
        let mut world = published(&map);
        let handles: Vec<Handle<Mesh>> = world
            .query_filtered::<&Mesh3d, (With<crate::retail_render::shadow_visibility::StaticShadowCaster>, Without<shadow_geometry::ShadowProxy>)>()
            .iter(&world)
            .map(|m| m.0.clone())
            .collect();
        let meshes = world.resource::<Assets<Mesh>>();
        let mut interned = bevy::mesh::MeshVertexBufferLayouts::default();
        let layouts: Vec<_> =
            handles.iter().map(|h| meshes.get(h).unwrap().get_mesh_vertex_buffer_layout(&mut interned)).collect();
        assert_eq!(layouts.len(), 2);
        assert!(layouts.iter().all(|l| *l.0 == *layouts[0].0 && l.0.contains(Mesh::ATTRIBUTE_TANGENT)));
        let zero_tangents = handles.iter().any(|h| match meshes.get(h).unwrap().attribute(Mesh::ATTRIBUTE_TANGENT) {
            Some(bevy::mesh::VertexAttributeValues::Float32x4(t)) => t.iter().all(|t| *t == [0.; 4]),
            Some(bevy::mesh::VertexAttributeValues::Snorm16x4(t)) => t.iter().all(|t| *t == [0; 4]),
            _ => false,
        });
        assert!(zero_tangents, "the frameless decal's tangents are zero");
    }
    /// perf 10-19 (SKATE_MERGE_BLEND): blended batches of one material merge within a 64 m cell, not across cells.
    #[test]
    fn merged_blended_batches_stay_within_one_cell() {
        let mut map = retail_demo();
        for m in &mut map.materials {
            m.alpha_mode = 2;
        }
        for v in &mut map.geometry.vertices {
            v.position = v.position.map(|p| p * 1e-3 + 10.); // all in cell (0, 0)
        }
        let textures = render_texture_ids(&map.textures);
        let tuning = crate::retail_render::MaterialTuning::default();
        let groups = |map: &SkateMap, merge| {
            let (atlas, definitions) = (Default::default(), Definitions::parse(&map.materials));
            consolidated_groups_with(map, &textures, &tuning, &mut |_, _| None, &atlas, &[], &definitions, true, merge)
        };
        assert_eq!(groups(&map, false).len(), 2, "blended batches keep their sorting groups");
        let merged = groups(&map, true);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].indices.len(), map.geometry.indices.len());
        for v in map.geometry.vertices.iter_mut().filter(|v| v.material == 2) {
            v.position[0] += 100.;
        }
        assert_eq!(groups(&map, true).len(), 2, "another cell");
    }
    #[test]
    fn retail_batches_are_owned_and_retired_with_the_map() {
        let map = retail_demo();
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
        world.init_resource::<Assets<Image>>();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        scene.prepare(Some(&map), std::path::Path::new("unused"));
        scene.publish(&mut world);
        // perf 11-01: the batches that cast no shadow are exactly the ones the proxies cast for, triangle for triangle
        // (opaque_batches_cast_through_welded_position_proxies matches them by position); both retire with the map.
        fn triangles<F: bevy::ecs::query::QueryFilter>(world: &mut World) -> usize {
            let handles: Vec<Handle<Mesh>> = world.query_filtered::<&Mesh3d, F>().iter(world).map(|m| m.0.clone()).collect();
            handles.iter().map(|h| world.resource::<Assets<Mesh>>().get(h).unwrap().indices().unwrap().len() / 3).sum()
        }
        let off = triangles::<(With<bevy::light::NotShadowCaster>, Without<shadow_geometry::ShadowProxy>)>(&mut world);
        assert_eq!(off, triangles::<With<shadow_geometry::ShadowProxy>>(&mut world));
        assert_eq!(off > 0, shadow_geometry::enabled(), "every batch here is opaque");
        crate::map_render::MapAssets::retire(&mut world);
        assert_eq!(
            world
                .query_filtered::<Entity, With<crate::map_render::MapEntity>>()
                .iter(&world)
                .count(),
            0
        );
        assert_eq!(world.resource::<Assets<Mesh>>().len(), 0);
        assert_eq!(world.resource::<Assets<StandardMaterial>>().len(), 0);
        assert_eq!(
            world
                .resource::<Assets<crate::retail_render::PropMaterial>>()
                .len(),
            0
        );
        assert_eq!(
            world
                .resource::<Assets<crate::retail_render::RetailWorldMaterial>>()
                .len(),
            0
        );
        assert_eq!(world.resource::<Assets<crate::retail_render::RetailBlendMaterial>>().len(), 0);
    }
    #[test]
    fn render_batches_preserve_complete_triangles_and_materials() {
        let map = demo();
        let material_ids = render_material_ids(&map.materials, &render_texture_ids(&map.textures));
        let chunks = render_groups(&map.geometry, &material_ids);
        assert_eq!(chunks.len(), 1);
        let mut original: Vec<_> = map
            .geometry
            .indices
            .chunks_exact(3)
            .map(|t| t.to_vec())
            .collect();
        let mut partitioned = Vec::new();
        for (material, indices) in chunks {
            for tri in indices.chunks_exact(3) {
                assert_eq!(
                    material + 1,
                    map.geometry.vertices[tri[0] as usize].material as usize
                );
                partitioned.push(tri.to_vec());
            }
        }
        original.sort();
        partitioned.sort();
        assert_eq!(partitioned, original);
    }
    #[test]
    fn render_material_sharing_requires_identical_rendered_fields() {
        let mut map = demo();
        let mut copy = demo().materials.remove(0);
        copy.name = "another source object".into();
        copy.audio = 42;
        map.materials.push(copy);
        let texture_ids = render_texture_ids(&map.textures);
        assert_eq!(render_material_ids(&map.materials, &texture_ids), [0, 0]);
        map.materials[1].roughness = 0.1234;
        assert_eq!(render_material_ids(&map.materials, &texture_ids), [0, 1]);
        map.materials[1].roughness = map.materials[0].roughness;
        map.materials[1].textures[1] = 0;
        assert_eq!(render_material_ids(&map.materials, &texture_ids), [0, 1]);
        assert_eq!(material_ids(&map.materials, &texture_ids, false), [0, 0]);
        map.materials[1].indirect_strength += 1.;
        assert_eq!(material_ids(&map.materials, &texture_ids, false), [0, 1]);
    }
    #[test]
    fn texture_sharing_preserves_pixels_dimensions_and_color_space() {
        let texture = |name: &str, color_space, pixel| skate_data::skate_map::Texture {
            name: name.into(),
            width: 1,
            height: 1,
            color_space,
            rgba: vec![pixel, 0, 0, 255],
        };
        let textures = vec![
            texture("first", 1, 30),
            texture("duplicate", 1, 30),
            texture("linear", 0, 30),
            texture("different", 1, 31),
        ];
        assert_eq!(render_texture_ids(&textures), [0, 1, 1, 3, 4]);
    }
    fn material() -> RetailContactMaterial {
        RetailContactMaterial {
            static_friction: 0.,
            dynamic_friction: 0.,
            restitution: 1.,
        }
    }
    #[test]
    fn native_edges_override_generated_adjacency() {
        let mut map = demo();
        map.geometry.collision[0].native_edges = Some([0x20, 0x42, 0x9a]);
        validate_runtime(&map).unwrap();
        let world = collision_world(&map, material()).unwrap();
        let f = world.triangles()[0].triangle().feature;
        assert!(f.edge_convex(0));
        assert!(!f.edge_convex(1));
        assert!(!f.edge_convex(2));
        assert!(!f.vertex_disabled(0));
        assert!(f.vertex_disabled(1));
        assert!(!f.vertex_disabled(2));
        assert_eq!(f.edge_cosines[0].to_bits(), 0xbe6f4f38);
        assert_eq!(f.edge_cosines[1].to_bits(), 0x3f310b0c);
        assert_eq!(f.edge_cosines[2], 1.);
        assert!(decode_native_edges([31, 0, 0]).is_err());
        assert_eq!(decode_native_edges([0, 2, 26]).unwrap().1, f.edge_cosines);
    }
    #[test]
    fn embedded_archive_is_authoritative_and_preserves_cluster_metadata() {
        let mut map = demo();
        map.geometry.collision.clear();
        map.extensions.push(skate_data::skate_map::Extension {
            tag: *b"RWCM",
            schema: 1,
            payload: include_bytes!("../../skate-data/tests/fixtures/retail-collision.rwcmset")
                .to_vec(),
        });
        validate_runtime(&map).unwrap();
        let world = collision_world(&map, material()).unwrap();
        assert_eq!(world.triangles().len(), 3);
        let metadata = world.query_metadata().unwrap();
        assert_eq!(metadata.meshes.len(), 3);
        // The cluster group still partitions the clusters into meshes (three
        // here), but it must not land on the mesh's matchingID: retail's static
        // registration writes -1 there. See the comment at the construction
        // site. This is what `embedded_static_rwcm_hits_distinct_actor_query_ids`
        // depends on, and the two tests disagreed until 2026-09-20.
        assert!(metadata.meshes.iter().all(|m| m.matching_group == -1));
        assert_eq!(metadata.packed_surfaces, vec![0x4321; 3]);
        assert_eq!(world.triangles()[1].vertices[0].x, 10.);
    }
    #[test]
    fn native_unit_without_edge_data_keeps_constructor_defaults() {
        let mut archive =
            include_bytes!("../../skate-data/tests/fixtures/retail-collision.rwcmset").to_vec();
        let name_len = u32::from_le_bytes(archive[12..16].try_into().unwrap()) as usize;
        let cluster = 16 + name_len + 4 + 160;
        archive[cluster + 2..cluster + 4].copy_from_slice(&8_u16.to_be_bytes());
        archive[cluster + 80] = 0xc1;
        archive[cluster + 84..cluster + 88].copy_from_slice(&[0x34, 0x12, 0x21, 0x43]);
        let world = retail_collision_world(&archive, material()).unwrap();
        let f = world.triangles()[0].triangle().feature;
        assert_eq!(f.flags, 0x1e1);
        assert_eq!(f.edge_cosines, [-1.; 3]);
    }
    /// perf 10-18: the batched, parallel collision build gives what the one-by-one build gives.
    #[test]
    fn parallel_collision_build_matches_the_serial_one() {
        let archive = include_bytes!("../../skate-data/tests/fixtures/retail-collision.rwcmset");
        let built = |batch| {
            let (b, count) = retail_collision_parts(archive, material(), batch).unwrap();
            format!("{:?} {:?} {:?} {count}", b.triangles, b.packed_surfaces, b.meshes)
        };
        let serial = built(0);
        assert!(serial.contains("triangle_range: 2..3"), "{serial}");
        for batch in [1, 2, COLLISION_BATCH] {
            assert_eq!(built(batch), serial, "batch {batch}");
        }
    }

    /// A truck-like capsule (radius 8.5 mm, half length 64 mm) at `center`, its axis `axis`, moving at `velocity`:
    /// the normals and tags of the static-world contacts the board query gives it.
    fn truck_contacts(world: &mut BoardWorld, center: [f32; 3], axis: [f32; 3], velocity: [f32; 3]) -> Vec<(Vector3, u32)> {
        use skate_core::physics::{
            board::BodyId, board_step::CollisionBody, board_world::{BoardWorldVolume, ContactRetentionSettings},
            collision::WorldContactSettings, world_contact::ContactPrimitive,
        };
        let v = |[x, y, z]: [f32; 3]| Vector3::new(x, y, z);
        let length = axis.iter().map(|a| a * a).sum::<f32>().sqrt();
        let volume = BoardWorldVolume {
            body: CollisionBody::Board(BodyId::FrontTruck),
            primitive: ContactPrimitive::Capsule {
                center: v(center),
                axis: v(axis.map(|a| a / length)),
                half_length: 0.064,
                radius: 0.0085,
            },
            linear_velocity: v(velocity),
            material: material(),
        };
        // The board query's settings (physics::ground::query_settings).
        let query = WorldContactSettings {
            volume_padding: 0.05,
            maximum_separating_distance: 0.5,
            edge_cos_bend_normal_threshold: 0.999,
            convexity_epsilon: 0.01,
            is_object: false,
        };
        let retention = ContactRetentionSettings { capacity: u32::MAX, duplicate_distance_squared: -1.0, deferred_reduction: true };
        world.query_primitives(&[volume], query, retention).iter().map(|k| (k.contact.normal, k.contact.tag)).collect()
    }

    /// Bug #25/#26 (10-25): a wall buried under the floor in front of it (its top edge flush with that floor) is
    /// dropped; the floors, a step wall and a wall facing a cave under a slab stay. Before, a truck capsule riding that
    /// floor towards the buried wall's edge, its axis 1.3 degrees off the edge, got a face contact from the wall.
    #[test]
    fn a_face_sealed_under_the_floor_in_front_of_it_is_dropped() {
        let flags = 1 | TriangleFeature::ONE_SIDED | TriangleFeature::USE_EDGE_COSINES | 0xe00; // edge code 90 x3
        let tri = |p: [[f32; 3]; 3]| {
            WorldTriangle::from_vertices(p.map(|[x, y, z]| Vector3::new(x, y, z)), material(), 7, flags, [1.; 3], 0.)
                .unwrap()
        };
        // An up-facing quad over x0..x1, z -3..3, at height y.
        let floor = |x0: f32, x1: f32, y: f32| [tri([[x0, y, -3.], [x0, y, 3.], [x1, y, -3.]]), tri([[x1, y, -3.], [x0, y, 3.], [x1, y, 3.]])];
        let mut triangles = Vec::new();
        triangles.extend(floor(-2., 0., 0.));
        triangles.extend(floor(0., 2., 0.));
        let buried = triangles.len();
        triangles.push(tri([[0., 0., -3.], [0., 0., 3.], [0., -3., 0.]])); // faces +x, under the floor east of it
        triangles.extend(floor(3., 5., 0.));
        triangles.extend(floor(5., 7., 1.));
        triangles.push(tri([[5., 1., 3.], [5., 1., -3.], [5., 0., 0.]])); // a step: faces -x, the top floor behind it
        triangles.extend(floor(8., 10., 0.));
        triangles.extend(floor(10., 12., 0.)); // a slab top in front of the cave wall
        triangles.push(tri([[10., -1., -3.], [12., -1., -3.], [10., -1., 3.]])); // the slab's underside
        triangles.push(tri([[12., -1., -3.], [12., -1., 3.], [10., -1., 3.]]));
        triangles.extend(floor(10., 12., -3.)); // the cave floor
        triangles.push(tri([[10., 0., -3.], [10., 0., 3.], [10., -3., 0.]])); // faces +x into the cave
        let sealed = sealed_faces(&triangles);
        assert_eq!(sealed.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| i).collect::<Vec<_>>(), [buried]);

        let probe = |triangles: Vec<WorldTriangle>| {
            truck_contacts(&mut BoardWorld::new(triangles), [0.0685, 0.0307, 0.], [0.023, 0., 1.], [-4., 0., 0.])
        };
        let ghost = |contacts: &[(Vector3, u32)]| contacts.iter().any(|(n, _)| n.x > 0.9);
        assert!(ghost(&probe(triangles.clone())), "the buried wall's face contact (the bug)");
        let kept: Vec<_> = triangles.iter().zip(&sealed).filter(|(_, s)| !**s).map(|(t, _)| *t).collect();
        let contacts = probe(kept);
        assert!(!ghost(&contacts), "{contacts:?}");
        assert!(contacts.iter().any(|(n, _)| n.y > 0.9), "the floor still holds the truck: {contacts:?}");
    }

    /// Bug #25/#26 on the installed University (SKATE3_ASSET_ROOT = the install's assets; returns early without it):
    /// the binocular plaza's buried wall and a grind edge's flush fin give a truck no face contact any more, while
    /// the plaza floor, the wall facing the cave under the viewing slab and a binocular post still do.
    #[test]
    fn university_binoculars_and_flush_grind_edges_have_no_invisible_wall() {
        let Ok(root) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let map = SkateMap::load(&std::path::Path::new(&root).parent().unwrap().join("maps/University.skate")).unwrap();
        let (built, count) = retail_collision_parts(retail_archive(&map).unwrap().unwrap(), material(), COLLISION_BATCH).unwrap();
        let started = std::time::Instant::now();
        let sealed = sealed_faces(&built.triangles).iter().filter(|&&s| s).count();
        eprintln!("University: {sealed} of {count} faces sealed, found in {:?}", started.elapsed());
        assert!((1..500).contains(&sealed), "{sealed} sealed faces");
        let mut world = collision_world(&map, material()).unwrap();
        let toward = |contacts: &[(Vector3, u32)], [x, y, z]: [f32; 3]| contacts.iter().any(|(n, _)| n.x * x + n.y * y + n.z * z > 0.9);
        // Riding west across the plaza 7 m past the last binoculars: the wall under it at x -408.499 faces east.
        let plaza = truck_contacts(&mut world, [-408.4305, 222.6127, -875.909], [0.023, 0., 1.], [-4., 0., 0.]);
        assert!(!toward(&plaza, [1., 0., 0.]), "binocular plaza: {plaza:?}");
        assert!(toward(&plaza, [0., 1., 0.]), "binocular plaza floor: {plaza:?}");
        // The fin under the grind spline at (40.7, 133.48, -625.7), the floor flush on both sides.
        let fin = truck_contacts(&mut world, [40.744, 133.5117, -625.6914], [-0.4343, 0., 0.9007], [-3.562, 0., -1.819]);
        assert!(!toward(&fin, [0.891, 0., 0.455]), "flush grind edge: {fin:?}");
        assert!(toward(&fin, [0., 1., 0.]), "grind edge floor: {fin:?}");
        // The same wall where it faces the cave under the viewing slab still stops a truck.
        let cave = truck_contacts(&mut world, [-408.4305, 220.6, -879.5], [0.023, 0., 1.], [-4., 0., 0.]);
        assert!(toward(&cave, [1., 0., 0.]), "cave wall: {cave:?}");
        // So does a binocular post (surface 145; at this height its east side is at x -400.974).
        let post = truck_contacts(&mut world, [-400.925, 223.0, -877.23], [0., 0., 1.], [-4., 0., 0.]);
        assert!(post.iter().any(|&(_, tag)| tag == 145), "binocular post: {post:?}");
    }

    /// User 10-27: Bump Riser (ots_artg_02_ol on the installed Art Gallery; SKATE3_ASSET_ROOT = the install's assets,
    /// returns early without it) started 12.9 m in front of an invisible wall, the face x + z = 100.04 of a 145 m
    /// trigger box (cSim_250_-50_high.xsf#22) retail never collides with. A truck crossing it on the floor (y 42.12;
    /// the nearest world wall 1.8 m off) touches the floor alone.
    #[test]
    fn bump_risers_trigger_box_is_no_wall() {
        let Ok(root) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let map = SkateMap::load(&std::path::Path::new(&root).parent().unwrap().join("maps/Art_Gallery.skate")).unwrap();
        let mut world = collision_world(&map, material()).unwrap();
        let contacts = truck_contacts(&mut world, [225.128, 42.14, -125.083], [0.7071, 0., -0.7071], [-2.83, 0., -2.83]);
        let along = |[x, y, z]: [f32; 3]| contacts.iter().map(move |(n, _)| n.x * x + n.y * y + n.z * z);
        assert!(!along([0.7071, 0., 0.7071]).any(|d| d.abs() > 0.9), "the trigger box's face: {contacts:?}");
        assert!(along([0., 1., 0.]).any(|d| d > 0.9), "the floor: {contacts:?}");
    }
    #[test]
    #[ignore = "requires SKATE_MAP_TEST_PATH pointing to a private map"]
    fn private_map_builds_collision_world() {
        let path = std::env::var("SKATE_MAP_TEST_PATH").unwrap();
        let map = SkateMap::load(std::path::Path::new(&path)).unwrap();
        validate_runtime(&map).unwrap();
        let world = collision_world(&map, material()).unwrap();
        let [x, y, z] = map.spawn;
        let hit = world
            .query_thin_line(Vector3::new(x, y + 1., z), Vector3::new(x, y - 10., z))
            .unwrap();
        assert!(hit.is_some(), "spawn has no supporting collision");
        eprintln!(
            "Private map world: {} triangles, spawn hit {:?}",
            world.triangles().len(),
            hit
        );
    }
    /// Diagnostic (user 10-13): a "giant invisible barrier" where Follow Slappy ends; the skater walked into it at about
    /// (1420.7, 85, -252.4), bailed and crashed. Prints what the map's collision holds across that spot.
    #[test]
    #[ignore = "requires SKATE_MAP_TEST_PATH pointing to New_San_Vanelona.skate"]
    fn skate2_follow_slappy_end_collision() {
        let path = std::env::var("SKATE_MAP_TEST_PATH").unwrap();
        let map = SkateMap::load(std::path::Path::new(&path)).unwrap();
        let world = collision_world(&map, material()).unwrap();
        for (dx, dz) in [(1., 0.), (-1., 0.), (0., 1.), (0., -1.), (0.7, 0.7), (-0.7, 0.7), (0.7, -0.7), (-0.7, -0.7)] {
            for y in [85.2f32, 86.5, 89.] {
                let from = Vector3::new(1420.7, y, -252.4);
                let hit = world.query_thin_line(from, Vector3::new(from.x + dx * 12., y, from.z + dz * 12.)).unwrap();
                eprintln!("dir ({dx}, {dz}) y {y}: {:?}", hit.map(|h| (h.geometry.position, h.geometry.normal, h.tag)));
            }
        }
    }
    #[test]
    fn unsupported_spawn_moves_to_the_nearest_walkable_face() {
        let world = collision_world(&demo(), material()).unwrap();
        // The demo floor is y = 0 over x, z in [-30, 30].
        // 1 m up drops to the retail 0.1 m clearance; a spawn on the face stays.
        let [_, y, _] = supported_spawn(&world, [5., 1., -5.]);
        assert!((y - 0.1).abs() < 1e-5, "{y}");
        assert_eq!(supported_spawn(&world, [5., 0., -5.]), [5., 0., -5.]);
        let [x, y, z] = supported_spawn(&world, [100., 1., 100.]);
        assert!(
            (-30.0..=30.0).contains(&x) && (-30.0..=30.0).contains(&z),
            "{x} {z}"
        );
        assert!((y - 0.1).abs() < 1e-5, "{y}");
        assert!(
            world
                .query_thin_line(Vector3::new(x, y + 1., z), Vector3::new(x, y - 10., z))
                .unwrap()
                .is_some()
        );
    }
    /// v12 #15: a teleport over nothing (a lobby slot past the Skate 1 city mod's ground) lands on the nearest walkable
    /// face at the probe's 0.15 m; over ground it is retail's probe, untouched.
    #[test]
    fn a_teleport_over_a_hole_lands_on_the_nearest_ground() {
        let world = collision_world(&demo(), material()).unwrap();
        let at = |x: f32, y: f32, z: f32| {
            let mut m = Mat4::IDENTITY.to_cols_array_2d();
            m[3][..3].copy_from_slice(&[x, y, z]);
            m
        };
        assert_eq!(travel_ground(&world, at(5., 2., -5.), true), probe_ground(&world, at(5., 2., -5.)).0);
        let landed = travel_ground(&world, at(100., 2., 100.), true);
        let [x, y, z] = [landed[3][0], landed[3][1], landed[3][2]];
        assert!((-30.0..=30.0).contains(&x) && (-30.0..=30.0).contains(&z), "{x} {z}");
        assert!((y - 0.15).abs() < 1e-5, "{y}");
        assert_eq!(landed[0], at(0., 0., 0.)[0], "the basis is kept");
    }
    /// A teleport 0.75-1 m under a face with nothing under it (the probe starts 0.75 m up, so it misses that face; a lobby
    /// slot at University's film_univ_02 fell 73 m): the skater stands on the face over the target.
    #[test]
    fn a_teleport_just_under_a_face_stands_on_it() {
        let world = collision_world(&demo(), material()).unwrap();
        let mut at = Mat4::IDENTITY.to_cols_array_2d();
        at[3][..3].copy_from_slice(&[5., -0.9, 5.]);
        assert_eq!(probe_ground(&world, at).1, None, "the probe starts under the face");
        let landed = travel_ground(&world, at, true);
        assert_eq!([landed[3][0], landed[3][2]], [5., 5.]);
        assert!((landed[3][1] - 0.15).abs() < 1e-5, "{}", landed[3][1]);
    }
    /// A teleport over a hole with a roof over it and ground 2 m to the side (Skate 1 San Vanelona's ownspot_07, player 2:
    /// the roof's centre was the nearest walkable face, and the skater fell 75 m off it): it lands on the ground, clear
    /// of the hole's edge.
    #[test]
    fn a_teleport_over_a_hole_under_a_roof_lands_on_the_ground_beside_it() {
        let mut map = demo();
        let (surface, material_id) = (map.geometry.collision[0].surface, map.geometry.collision[0].material);
        let ground = [[2., 0., -30.], [2., 0., 30.], [30., 0., -30.]];
        let roof = [[-1., 3.3, -1.5], [-1., 3.3, 1.5], [2.5, 3.3, -1.5]];
        map.geometry.collision = [ground, roof]
            .map(|points| skate_data::skate_map::Collision { points, surface, material: material_id, native_edges: None })
            .into();
        let world = collision_world(&map, material()).unwrap();
        assert!(world.triangles().iter().all(|t| t.normal.y > 0.99), "both face up");
        let at = |x: f32, y: f32, z: f32| {
            let mut m = Mat4::IDENTITY.to_cols_array_2d();
            m[3][..3].copy_from_slice(&[x, y, z]);
            m
        };
        assert!(probe_ground(&world, at(10., 0., 0.)).1.is_some_and(|f| f.abs() < 1e-5), "the ground is probed");
        assert_eq!(probe_ground(&world, at(0., 0., 0.)).1, None, "the hole under the roof");
        let landed = travel_ground(&world, at(0., 0., 0.), true);
        let [x, y, z] = [landed[3][0], landed[3][1], landed[3][2]];
        assert!((y - 0.15).abs() < 1e-5, "on the ground, not the roof: {y}");
        assert!((2.6..=3.5).contains(&x) && z.abs() <= 1.0, "beside the hole, 0.6 m clear of its edge: {x} {z}");
    }
    /// A teleport over a hole with water nearer than the ground (Danny Way's 1up_dwmc_01 waiting slots 4 and 5: the
    /// nearest flat face was a pool, and standing on water is a bail): it lands on the ground.
    #[test]
    fn a_teleport_over_a_hole_lands_on_ground_not_water() {
        let mut map = demo();
        let (surface, material_id) = (map.geometry.collision[0].surface, map.geometry.collision[0].material);
        let water = [[1., 0., -30.], [1., 0., 30.], [30., 0., -30.]];
        let ground = [[-3., 0., -30.], [-30., 0., -30.], [-3., 0., 30.]];
        map.geometry.collision = [(water, skate_core::player::water::WATER << 7), (ground, surface)]
            .map(|(points, surface)| skate_data::skate_map::Collision { points, surface, material: material_id, native_edges: None })
            .into();
        let world = collision_world(&map, material()).unwrap();
        let category = |x: f32| world.query_thin_line(Vector3::new(x, 1., 0.), Vector3::new(x, -1., 0.)).unwrap().map(|hit| (hit.tag >> 7) & 31);
        assert_eq!(category(2.), Some(skate_core::player::water::WATER), "the water is probed");
        assert!(category(-4.).is_some_and(|c| c != skate_core::player::water::WATER), "the ground is probed");
        let mut at = Mat4::IDENTITY.to_cols_array_2d();
        assert_eq!(probe_ground(&world, at).1, None, "the hole");
        at = travel_ground(&world, at, true);
        let [x, y, z] = [at[3][0], at[3][1], at[3][2]];
        // The nearest ground 0.6 m in from its edge is 4 m off (the ring that reaches it), whichever way round.
        assert!((y - 0.15).abs() < 1e-5 && x <= -3.6 && x.hypot(z) <= 4.01, "on the ground beside the hole: {x} {y} {z}");
    }
    /// A teleport 1 cm from a wall that stands on a base (DownTown's 1up_dwtn_04 4th waiting spot: the board, dropped
    /// in, met it, tipped and bailed) moves along the ground until the board clears it, on the near side; one clear of
    /// walls is retail's probe.
    #[test]
    fn a_teleport_against_a_wall_moves_until_the_board_fits() {
        let mut map = demo();
        let (surface, material_id) = (map.geometry.collision[0].surface, map.geometry.collision[0].material);
        let ground = [[[-30., 0., -30.], [-30., 0., 30.], [30., 0., -30.]], [[30., 0., -30.], [-30., 0., 30.], [30., 0., 30.]]];
        // At x 0.01, z -5..5, from 0.2 m (nothing at the resting board's height) to 2 m, its winding turned away from the
        // spot (+x) as the archive turns DownTown's.
        let wall = [[[0.01, 0.2, -5.], [0.01, 2., -5.], [0.01, 0.2, 5.]], [[0.01, 2., -5.], [0.01, 2., 5.], [0.01, 0.2, 5.]]];
        map.geometry.collision = ground
            .into_iter()
            .chain(wall)
            .map(|points| skate_data::skate_map::Collision { points, surface, material: material_id, native_edges: None })
            .collect();
        let world = collision_world(&map, material()).unwrap();
        let at = |x: f32| {
            let mut m = Mat4::IDENTITY.to_cols_array_2d();
            m[3][..3].copy_from_slice(&[x, 1., 0.]);
            m
        };
        assert_eq!(travel_ground(&world, at(-3.), true), probe_ground(&world, at(-3.)).0, "clear: the probe's spot");
        let probed = probe_ground(&world, at(0.)).0;
        let moved = travel_ground(&world, at(0.), true);
        let [x, y, z] = [moved[3][0], moved[3][1], moved[3][2]];
        assert!((y - 0.15).abs() < 1e-5, "on the ground: {y}");
        assert!(x <= 0.01 - 0.12 && x.hypot(z) <= 0.3, "the deck's side clear of the wall, the near side, close by: {x} {z}");
        assert_eq!(moved[..3], probed[..3], "the heading is kept");
        assert_eq!(travel_ground(&world, at(0.), false), probed, "on foot: no board to fit, the probe's spot");
    }
    /// Review M3 (online fixes 11-01): a walkable slope just ahead is no wall, whichever way the archive winds it.
    #[test]
    fn a_ramp_wound_downwards_is_no_wall() {
        let mut map = demo();
        let (surface, material_id) = (map.geometry.collision[0].surface, map.geometry.collision[0].material);
        let ground = [[[-30., 0., -30.], [-30., 0., 30.], [30., 0., -30.]], [[30., 0., -30.], [-30., 0., 30.], [30., 0., 30.]]];
        // Rising 24° from z 0.2 (0.29 m ahead it is at the wheels' 0.04 m), its normal down (0, -0.91, 0.41).
        let ramp = [[[-5., 0., 0.2], [5., 0., 0.2], [-5., 0.8, 2.]], [[5., 0., 0.2], [5., 0.8, 2.], [-5., 0.8, 2.]]];
        map.geometry.collision = ground
            .into_iter()
            .chain(ramp)
            .map(|points| skate_data::skate_map::Collision { points, surface, material: material_id, native_edges: None })
            .collect();
        let world = collision_world(&map, material()).unwrap();
        assert!(world.triangles()[2].triangle().feature.normal.y < -0.9, "wound downwards");
        let mut at = Mat4::IDENTITY.to_cols_array_2d();
        at[3][..3].copy_from_slice(&[0., 1., 0.]);
        assert_eq!(travel_ground(&world, at, true), probe_ground(&world, at).0);
    }
    /// A teleport into a gap narrower than the board is long, heading across it (DownTown's 4th waiting spot: 0.32 m
    /// between two walls, its heading 52° off the gap): no spot nearby fits at that heading, so the board turns to lie
    /// along the gap, in it.
    #[test]
    fn a_teleport_into_a_narrow_gap_turns_the_board_along_it() {
        let mut map = demo();
        let (surface, material_id) = (map.geometry.collision[0].surface, map.geometry.collision[0].material);
        let ground = [[[-30., 0., -30.], [-30., 0., 30.], [30., 0., -30.]], [[30., 0., -30.], [-30., 0., 30.], [30., 0., 30.]]];
        // x -0.16 facing +x and x 0.16 facing -x, both 2 m tall, z -5..5.
        let west = [[[-0.16, 0., -5.], [-0.16, 2., -5.], [-0.16, 0., 5.]], [[-0.16, 2., -5.], [-0.16, 2., 5.], [-0.16, 0., 5.]]];
        let east = [[[0.16, 0., -5.], [0.16, 0., 5.], [0.16, 2., -5.]], [[0.16, 2., -5.], [0.16, 0., 5.], [0.16, 2., 5.]]];
        map.geometry.collision = ground
            .into_iter()
            .chain(west)
            .chain(east)
            .map(|points| skate_data::skate_map::Collision { points, surface, material: material_id, native_edges: None })
            .collect();
        let world = collision_world(&map, material()).unwrap();
        let mut at = Mat4::from_rotation_y(52f32.to_radians()).to_cols_array_2d();
        at[3][..3].copy_from_slice(&[0., 1., 0.]);
        let landed = travel_ground(&world, at, true);
        let ahead = Vec3::new(landed[2][0], 0., landed[2][2]).normalize();
        // Lengthwise: 0.42 m of half length stays inside the 0.16 m either side only within about 22° of the gap's line.
        assert!(ahead.z.abs() >= 23f32.to_radians().cos(), "along the gap: {ahead}");
        assert!(landed[3][0].abs() <= 0.04 && (landed[3][1] - 0.15).abs() < 1e-5, "in the gap, on the ground: {:?}", landed[3]);
        assert!((Vec3::from_slice(&landed[1][..3]) - Vec3::Y).length() < 1e-5, "still upright");
    }
    #[test]
    fn map_collision_uses_separate_geometry_and_surface_metadata() {
        let mut map = demo();
        map.geometry.vertices[0].position = [1000.; 3];
        map.materials[0].audio = 42;
        map.materials[0].physics = 4;
        map.materials[0].pattern = 7;
        let world = collision_world(&map, material()).unwrap();
        assert_eq!(world.triangles().len(), 2);
        assert_eq!(
            world.triangles()[0].vertices[0],
            Vector3::new(-30., 0., -30.)
        );
        assert!(world.triangles()[0].triangle().feature.normal.y > 0.999);
        assert_eq!(
            world.query_metadata().unwrap().packed_surfaces,
            vec![42 | (4 << 7) | (7 << 12); 2]
        );
        assert!(world.query_metadata().unwrap().static_edges.is_empty());
    }
    #[test]
    fn shared_flat_diagonal_is_not_a_convex_contact_edge() {
        let world = collision_world(&demo(), material()).unwrap();
        let a = world.triangles()[0].triangle().feature;
        let b = world.triangles()[1].triangle().feature;
        assert!(!a.edge_convex(2));
        assert!(!b.edge_convex(0));
        assert!(a.edge_convex(0));
        assert_eq!(a.edge_cosines[2], 1.);
        assert!(a.vertex_disabled(0));
    }
    #[test]
    fn render_adapter_creates_mesh_and_decoded_lightmap_without_a_window() {
        let map = demo();
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
        world.init_resource::<Assets<Image>>();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        scene.prepare(Some(&map), std::path::Path::new("unused"));
        scene.publish(&mut world);
        // Plus the custom-map sun and moon: one disc, two textures and materials.
        assert_eq!(world.resource::<Assets<Mesh>>().len(), 2);
        assert_eq!(world.resource::<Assets<StandardMaterial>>().len(), 2);
        assert_eq!(
            world
                .resource::<Assets<crate::retail_render::PropMaterial>>()
                .len(),
            1
        );
        assert_eq!(world.resource::<Assets<Image>>().len(), 4);
        assert_eq!(world.query::<&Mesh3d>().iter(&world).count(), 3);
        let lightmap = world
            .query::<&bevy::pbr::Lightmap>()
            .single(&world)
            .unwrap();
        let image = world
            .resource::<Assets<Image>>()
            .get(&lightmap.image)
            .unwrap();
        // Stored as authored; the lightmap shader decodes x*x*4 after filtering.
        assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm);
        assert_eq!(image.data.as_ref().unwrap()[0], 64);
    }
    /// A family 3 retail definition that samples a decal through vertex colours.
    fn decal_definition() -> Vec<u8> {
        fn text(bytes: &mut Vec<u8>, value: &str) {
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
        let mut decal = vec![0; 16];
        text(&mut decal, "world.decal");
        for value in [3u32, 0, 1] {
            decal.extend_from_slice(&value.to_le_bytes());
        } // family 3, one-sided, one binding
        text(&mut decal, "decal");
        for value in [1u32, 0, 0, 0] {
            decal.extend_from_slice(&value.to_le_bytes());
        } // texture 1
        decal.extend_from_slice(&0u32.to_le_bytes()); // no parameters
        text(&mut decal, "");
        decal
    }
    /// perf 11-01 (SKATE_PACK_VERTICES, on by default): retail batches are 52 bytes a vertex. Normals and tangents are
    /// Snorm16x4 within half a step of their normalized source, the tangent sign exact; decal UVs are Float32x2, exact.
    #[test]
    fn retail_batches_pack_normals_tangents_and_decal_uvs() {
        use bevy::mesh::VertexAttributeValues::{Float32x2, Float32x3, Snorm16x4};
        let mut map = retail_demo();
        map.materials[1].retail_definition = Some(decal_definition());
        // Per position, so the source of a batch vertex is the first one found there.
        for v in &mut map.geometry.vertices {
            let [x, _, z] = v.position;
            v.normal = [0.3, 2.0, -0.7 - x * 0.01]; // not unit: packing normalizes
            v.decal_uv = Some([0.123 + x, z - 4.5]);
        }
        let mut world = published(&map);
        let meshes: Vec<Handle<Mesh>> = world
            .query_filtered::<&Mesh3d, (With<MeshMaterial3d<crate::retail_render::RetailWorldMaterial>>, Without<shadow_geometry::ShadowProxy>)>()
            .iter(&world)
            .map(|m| m.0.clone())
            .collect();
        assert_eq!(meshes.len(), 2);
        let decode = |v: i16| (f32::from(v) / 32767.).max(-1.);
        let mut interned = bevy::mesh::MeshVertexBufferLayouts::default();
        let mut decal_uvs = 0;
        for handle in &meshes {
            let mesh = world.resource::<Assets<Mesh>>().get(handle).unwrap();
            assert_eq!(mesh.get_mesh_vertex_buffer_layout(&mut interned).0.layout().array_stride, 52);
            let (Some(Float32x3(p)), Some(Snorm16x4(n)), Some(Snorm16x4(t)), Some(Float32x2(c))) = (
                mesh.attribute(Mesh::ATTRIBUTE_POSITION),
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL),
                mesh.attribute(Mesh::ATTRIBUTE_TANGENT),
                mesh.attribute(Mesh::ATTRIBUTE_COLOR),
            ) else {
                panic!("packed retail batch attributes")
            };
            for (k, position) in p.iter().enumerate() {
                let source = map.geometry.vertices.iter().find(|v| v.position == *position).unwrap();
                let want = Vec3::from_array(source.normal).normalize();
                let got = Vec3::new(decode(n[k][0]), decode(n[k][1]), decode(n[k][2]));
                assert!((got - want).abs().max_element() <= 0.5 / 32767. + 1e-7, "normal {got} for {want}");
                assert!(t[k][3].abs() == 32767 || t[k] == [0; 4], "tangent sign {:?}", t[k]);
                if c[k] != [0.; 2] {
                    assert_eq!(c[k], source.decal_uv.unwrap(), "decal UVs pass through exactly");
                    decal_uvs += 1;
                }
            }
        }
        assert!(decal_uvs > 0, "the decal batch carries its UVs");
    }
    #[test]
    fn only_decal_batches_upload_vertex_colours() {
        let mut map = retail_demo();
        map.materials[1].retail_definition = Some(decal_definition());
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
        world.init_resource::<Assets<Image>>();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        scene.prepare(Some(&map), std::path::Path::new("unused"));
        scene.publish(&mut world);
        // Some(whether any colour is non-zero); perf 10-19: SKATE_UNIFIED_LAYOUT gives the other batch zeros.
        let mut colours: Vec<Option<bool>> = world
            .query_filtered::<&Mesh3d, (With<MeshMaterial3d<crate::retail_render::RetailWorldMaterial>>, Without<shadow_geometry::ShadowProxy>)>()
            .iter(&world)
            .map(|mesh| {
                world.resource::<Assets<Mesh>>().get(&mesh.0).unwrap().attribute(Mesh::ATTRIBUTE_COLOR).map(|c| match c {
                    bevy::mesh::VertexAttributeValues::Float32x4(c) => c.iter().any(|c| *c != [0.; 4]),
                    // perf 11-01 (SKATE_PACK_VERTICES): the decal UVs alone.
                    bevy::mesh::VertexAttributeValues::Float32x2(c) => c.iter().any(|c| *c != [0.; 2]),
                    _ => true,
                })
            })
            .collect();
        colours.sort();
        let unified = crate::config::perf_on("SKATE_UNIFIED_LAYOUT");
        assert_eq!(colours, [if unified { Some(false) } else { None }, Some(true)]);
        // Map batches enter the retained shadow index, so lights skip them by BVH and layer (perf 10-10 #3).
        let indexed: Vec<bool> = world
            .query_filtered::<Has<crate::retail_render::shadow_visibility::StaticShadowCaster>, (With<MeshMaterial3d<crate::retail_render::RetailWorldMaterial>>, Without<shadow_geometry::ShadowProxy>)>()
            .iter(&world)
            .collect();
        assert_eq!(indexed, [true, true]);
    }
    fn published(map: &SkateMap) -> World {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
        world.init_resource::<Assets<Image>>();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        scene.prepare(Some(map), std::path::Path::new("unused"));
        scene.publish(&mut world);
        world
    }
    /// FPS pass 10-23 (SKATE_FPS_WELD): a source that writes a shared corner once per triangle (Skate 1 San Vanelona
    /// ran ~2.6 vertex shaders a triangle) gets one vertex per distinct attribute set, so the post-transform cache
    /// reuses it; every corner keeps its exact attributes.
    #[test]
    fn map_batches_share_bit_identical_corners() {
        let mut map = retail_demo();
        let corners: Vec<_> = map.geometry.indices.iter().map(|&i| map.geometry.vertices[i as usize].clone()).collect();
        map.geometry.indices = (0..corners.len() as u32).collect();
        map.geometry.vertices = corners.clone();
        let key = |p: [f32; 3], n: [f32; 3], uv: [f32; 2]| (p.map(f32::to_bits), n.map(f32::to_bits), uv.map(f32::to_bits));
        let distinct = corners.iter().map(|v| key(v.position, v.normal, v.uv)).collect::<std::collections::HashSet<_>>().len();
        assert!(distinct < corners.len());
        // perf 11-01: batches carry packed normals (SKATE_PACK_VERTICES), so corners compare by them.
        let packed = |n: [f32; 3]| {
            let n = Vec3::from_array(n).normalize_or_zero().to_array();
            n.map(|v| (v.clamp(-1., 1.) * 32767.).round() as i16)
        };
        let key = |p: [f32; 3], n: [i16; 3], uv: [f32; 2]| (p.map(f32::to_bits), n, uv.map(f32::to_bits));
        let mut source: Vec<_> = corners.chunks_exact(3).map(|t| [0, 1, 2].map(|c| key(t[c].position, packed(t[c].normal), t[c].uv))).collect();
        let mut world = published(&map);
        let meshes: Vec<Handle<Mesh>> = world
            .query_filtered::<&Mesh3d, (With<MeshMaterial3d<crate::retail_render::RetailWorldMaterial>>, Without<shadow_geometry::ShadowProxy>)>()
            .iter(&world)
            .map(|m| m.0.clone())
            .collect();
        let (mut vertices, mut drawn) = (0, Vec::new());
        for mesh in &meshes {
            let mesh = world.resource::<Assets<Mesh>>().get(mesh).unwrap();
            use bevy::mesh::VertexAttributeValues::{Float32x2, Float32x3, Snorm16x4};
            let (Some(Float32x3(p)), Some(normals), Some(Float32x2(uv))) =
                (mesh.attribute(Mesh::ATTRIBUTE_POSITION), mesh.attribute(Mesh::ATTRIBUTE_NORMAL), mesh.attribute(Mesh::ATTRIBUTE_UV_0))
            else {
                panic!("retail batch attributes")
            };
            let n: Vec<[i16; 3]> = match normals {
                Snorm16x4(n) => n.iter().map(|&[x, y, z, _]| [x, y, z]).collect(),
                Float32x3(n) => n.iter().map(|&n| packed(n)).collect(),
                other => panic!("retail batch normals {other:?}"),
            };
            vertices += p.len();
            let indices: Vec<usize> = mesh.indices().unwrap().iter().collect();
            drawn.extend(indices.chunks_exact(3).map(|t| [0, 1, 2].map(|c| key(p[t[c]], n[t[c]], uv[t[c]]))));
        }
        assert_eq!(vertices, distinct);
        source.sort_unstable();
        drawn.sort_unstable();
        assert_eq!(drawn, source);
    }
    #[test]
    fn static_map_batches_carry_the_bounds_bevy_would_compute() {
        let mut map = retail_demo();
        // Family 0 renders through the PBR fallback: both kinds of static batch.
        map.materials[1].retail_definition.as_mut().unwrap()[33..37].copy_from_slice(&0u32.to_le_bytes());
        let mut world = published(&map);
        let batches: Vec<(Handle<Mesh>, Option<bevy::camera::primitives::Aabb>, bool)> = world
            .query_filtered::<(&Mesh3d, Option<&bevy::camera::primitives::Aabb>, Has<bevy::camera::visibility::NoAutoAabb>), (With<crate::retail_render::shadow_visibility::StaticShadowCaster>, Without<shadow_geometry::ShadowProxy>)>()
            .iter(&world)
            .map(|(m, a, fixed)| (m.0.clone(), a.copied(), fixed))
            .collect();
        assert_eq!(batches.len(), 2);
        for (mesh, aabb, fixed) in batches {
            let computed = bevy::camera::primitives::MeshAabb::compute_aabb(world.resource::<Assets<Mesh>>().get(&mesh).unwrap());
            assert!(fixed && aabb.is_some() && aabb == computed, "{aabb:?} vs {computed:?}");
        }
    }
    /// perf 11-01 (SKATE_SHADOW_PROXIES=1): opaque static batches leave the world lights to 64 m cells of welded
    /// positions on layer 27, which no camera draws. Every triangle that stopped casting is in them, by its positions.
    /// Off (the default): no proxies, and every batch casts.
    #[test]
    fn opaque_batches_cast_through_welded_position_proxies() {
        use super::shadow_geometry::{LAYER, ShadowProxy};
        use bevy::{camera::visibility::RenderLayers, mesh::VertexAttributeValues::Float32x3};
        let map = retail_demo();
        let mut world = published(&map);
        let triangles = |mesh: &Mesh| {
            let Some(Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { panic!("positions") };
            let i: Vec<usize> = mesh.indices().unwrap().iter().collect();
            i.chunks_exact(3).map(|t| [0, 1, 2].map(|c| p[t[c]].map(f32::to_bits))).collect::<Vec<_>>()
        };
        let originals: Vec<(Handle<Mesh>, bool)> = world
            .query_filtered::<(&Mesh3d, Has<bevy::light::NotShadowCaster>), (With<MeshMaterial3d<crate::retail_render::RetailWorldMaterial>>, Without<ShadowProxy>)>()
            .iter(&world)
            .map(|(m, off)| (m.0.clone(), off))
            .collect();
        let proxies: Vec<(Handle<Mesh>, RenderLayers, bool)> = world
            .query_filtered::<(&Mesh3d, &RenderLayers, Has<crate::retail_render::shadow_visibility::StaticShadowCaster>), With<ShadowProxy>>()
            .iter(&world)
            .map(|(m, l, indexed)| (m.0.clone(), l.clone(), indexed))
            .collect();
        if !super::shadow_geometry::enabled() {
            assert!(proxies.is_empty() && !originals.is_empty() && originals.iter().all(|(_, off)| !*off));
            return;
        }
        assert!(!originals.is_empty() && originals.iter().all(|(_, off)| *off), "opaque batches cast no more");
        assert!(!proxies.is_empty());
        let meshes = world.resource::<Assets<Mesh>>();
        let (mut cast, mut left) = (Vec::new(), Vec::new());
        for (handle, layers, indexed) in &proxies {
            assert!(*layers == RenderLayers::layer(LAYER) && *indexed);
            let mesh = meshes.get(handle).unwrap();
            assert_eq!(mesh.attributes().count(), 1, "positions alone");
            assert!(matches!(mesh.indices(), Some(bevy::mesh::Indices::U16(_))));
            let Some(Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { panic!() };
            let distinct: std::collections::HashSet<_> = p.iter().map(|v| v.map(f32::to_bits)).collect();
            assert_eq!(distinct.len(), p.len(), "one vertex per position");
            cast.extend(triangles(mesh));
        }
        for (handle, _) in &originals {
            left.extend(triangles(meshes.get(handle).unwrap()));
        }
        cast.sort();
        left.sort();
        assert_eq!(cast, left);
    }
    #[test]
    fn pbr_fallback_samples_the_retail_lightmap_atlas() {
        let mut map = retail_demo();
        // Family 0 is unsupported by the retail shader, so it renders through PBR.
        map.materials[1].retail_definition.as_mut().unwrap()[33..37]
            .copy_from_slice(&0u32.to_le_bytes());
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
        world.init_resource::<Assets<Image>>();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        scene.prepare(Some(&map), std::path::Path::new("unused"));
        scene.publish(&mut world);
        let pbr = world
            .query::<&bevy::pbr::Lightmap>()
            .single(&world)
            .unwrap()
            .image
            .id();
        let retail = world
            .query_filtered::<&MeshMaterial3d<crate::retail_render::RetailWorldMaterial>, Without<shadow_geometry::ShadowProxy>>()
            .single(&world)
            .unwrap()
            .0
            .id();
        let retail = world
            .resource::<Assets<crate::retail_render::RetailWorldMaterial>>()
            .get(retail)
            .unwrap();
        // One upload of the shared lightmap: the atlas page both materials sample.
        assert_eq!(retail.lightmap.as_ref().map(Handle::id), Some(pbr));
        assert_eq!(
            world.resource::<Assets<Image>>().get(pbr).unwrap().width(),
            4096
        );
    }
}
