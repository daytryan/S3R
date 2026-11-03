//! Perf 10-12 (Mission 1D): process RAM by stage, as the game loads a map at launch: the file, the decoded map,
//! main()'s loads, the Startup scene (BC7 as on the player's GPU), the render-only assets leaving for the render world,
//! the map dropped, the audio game side's boot; then each part dropped in turn, so its delta is its size.
//! Heap = the process heap's live bytes (Rust and the CRT); private = commit; ws = working set (touched).
//! `SKATE3_ASSET_ROOT=… [SKATE_MAP_TEST_PATH=X.skate] cargo test … memory_cost -- --ignored --nocapture`
use super::*;
use crate::performance::{ProcessMemory, image_cpu_bytes, mesh_cpu_bytes, process_memory};
use skate_data::skate_map::SkateMap;

struct Stages(ProcessMemory);
impl Stages {
    fn mark(&mut self, stage: &str) {
        let (now, was) = (process_memory(true), self.0);
        let mb = |b: usize| b as f64 / 1048576.;
        let d = |a: usize, b: usize| (a as f64 - b as f64) / 1048576.;
        eprintln!(
            "SKATE_MEMORY {stage:<44} heap {:>6.0} ({:>+6.0}) private {:>6.0} ({:>+6.0}) ws {:>6.0} ({:>+6.0}) heap_committed {:>6.0} peak_private {:>6.0}",
            mb(now.heap_live),
            d(now.heap_live, was.heap_live),
            mb(now.private),
            d(now.private, was.private),
            mb(now.working_set),
            d(now.working_set, was.working_set),
            mb(now.heap_committed),
            mb(now.peak_private),
        );
        self.0 = now;
    }
}

fn rows(title: &str, mut rows: Vec<(String, usize)>) {
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let total: usize = rows.iter().map(|r| r.1).sum();
    eprintln!("SKATE_MEMORY_PARTS {title}: {:.1} MB", total as f64 / 1048576.);
    for (name, bytes) in rows.into_iter().filter(|r| r.1 >= 1 << 20) {
        eprintln!("SKATE_MEMORY_PARTS   {:>8.1} MB {name}", bytes as f64 / 1048576.);
    }
}

fn map_parts(map: &SkateMap) {
    use skate_data::skate_map::{Collision, Geometry, Vertex};
    let geometry = |g: &Geometry| {
        g.vertices.capacity() * size_of::<Vertex>() + g.indices.capacity() * 4 + g.collision.capacity() * size_of::<Collision>()
    };
    let mut parts = vec![
        ("textures (decoded rgba)".to_string(), map.textures.iter().map(|t| t.rgba.capacity()).sum()),
        (format!("vertices ({} x {} B)", map.geometry.vertices.len(), size_of::<Vertex>()), map.geometry.vertices.capacity() * size_of::<Vertex>()),
        ("indices".into(), map.geometry.indices.capacity() * 4),
        (format!("collision ({} tris)", map.geometry.collision.len()), map.geometry.collision.capacity() * size_of::<Collision>()),
        ("rails".into(), map.rails.iter().map(|r| r.points.capacity() * 12 + r.native.as_ref().map_or(0, Vec::capacity)).sum()),
        ("doors".into(), map.doors.iter().map(|d| geometry(&d.geometry)).sum()),
        ("materials (retail definitions)".into(), map.materials.iter().map(|m| m.retail_definition.as_ref().map_or(0, Vec::capacity)).sum()),
    ];
    let mut tags = std::collections::BTreeMap::<String, usize>::new();
    for e in &map.extensions {
        *tags.entry(format!("extension {}", String::from_utf8_lossy(&e.tag))).or_default() += e.payload.capacity();
    }
    parts.extend(tags);
    rows(&format!("SkateMap {:?}", map.name), parts);
}

/// Main-world CPU copies in the scene's Assets, split by whether they stay (MAIN_WORLD) or leave with extraction.
fn scene_parts(world: &World) {
    use bevy::asset::RenderAssetUsages as U;
    let (mut kept, mut leaving) = ([0usize; 2], [0usize; 2]);
    for (_, image) in world.resource::<Assets<Image>>().iter() {
        let slot = if image.asset_usage.contains(U::MAIN_WORLD) { &mut kept } else { &mut leaving };
        slot[0] += image_cpu_bytes(image);
    }
    for (_, mesh) in world.resource::<Assets<Mesh>>().iter() {
        let slot = if mesh.asset_usage.contains(U::MAIN_WORLD) { &mut kept } else { &mut leaving };
        slot[1] += mesh_cpu_bytes(mesh);
    }
    rows(
        "scene assets",
        vec![
            ("images, render world only (leave at extraction)".into(), leaving[0]),
            ("meshes, render world only (leave at extraction)".into(), leaving[1]),
            ("images kept in the main world".into(), kept[0]),
            ("meshes kept in the main world".into(), kept[1]),
        ],
    );
}

fn physics_parts(physics: &GamePhysics) {
    use skate_core::physics::board_world::{WorldTriangle, query_metadata::{Bounds, QueryMesh}};
    let triangles = physics.world.triangles().len();
    let mut parts = vec![
        (format!("collision triangles ({triangles} x {} B)", size_of::<WorldTriangle>()), triangles * size_of::<WorldTriangle>()),
        (format!("triangle bounds ({} B)", size_of::<Bounds>()), triangles * size_of::<Bounds>()),
    ];
    if let Ok(m) = physics.world.query_metadata() {
        parts.push(("packed surfaces".into(), m.packed_surfaces.capacity() * 2));
        parts.push((format!("query meshes ({} x {} B)", m.meshes.len(), size_of::<QueryMesh>()), m.meshes.capacity() * size_of::<QueryMesh>()));
    }
    rows("physics world (listed parts)", parts);
}

/// What extraction does to a RENDER_WORLD-only asset: its data goes to the render world, which frees it after upload.
fn hand_off_render_only(world: &mut World) {
    use bevy::asset::RenderAssetUsages as U;
    for (_, image) in world.resource_mut::<Assets<Image>>().iter_mut() {
        if !image.asset_usage.contains(U::MAIN_WORLD) {
            image.data = None;
        }
    }
    for (_, mesh) in world.resource_mut::<Assets<Mesh>>().iter_mut() {
        if !mesh.asset_usage.contains(U::MAIN_WORLD) {
            drop(mesh.take_gpu_data());
        }
    }
}

/// The audio worker's game side as player_audio::run_game_side boots it (GameSide::build's public calls; its title
/// bed is left out).
fn boot_audio(
    root: &std::path::Path,
) -> (skate_audio_core::authored::AuthoredRuntime, skate_audio_core::game::Skater, Option<skate_data::audio::world::WorldAudio>) {
    use skate_data::audio::title;
    let guest = skate_data::audio::catalog::load_guest_image(&root.join("private/stock/audio-runtime-image")).unwrap();
    let mut runtime = skate_audio_core::authored::AuthoredRuntime::new(guest, Vec::new(), Vec::new()).unwrap();
    let db = root.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let files = title::with_traffic(title::files(title::roots(root)), root, Default::default());
    runtime.wire_environment().unwrap();
    runtime.game_boot(vault, files).0.unwrap();
    let skater = runtime.game_skater().unwrap();
    let world = skate_data::audio::world::WorldAudio::load(root).ok();
    (runtime, skater, world)
}

#[test]
#[ignore = "requires the installation and a map; measurement, run alone with --nocapture"]
fn memory_cost() {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let root = root.canonicalize().unwrap();
    let path = std::env::var_os("SKATE_MAP_TEST_PATH")
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::map_library::default_map(&root).unwrap().expect("a default map"));
    let mut s = Stages(process_memory(true));
    s.mark("start");
    // Probes: a touched block counts in heap, private and ws; a zeroed one never written only in heap and private.
    let probe = std::hint::black_box(vec![1u8; 64 << 20]);
    s.mark("probe: 64 MB written");
    drop(probe);
    let probe = std::hint::black_box(vec![0u8; 64 << 20]);
    s.mark("probe: 64 MB zeroed, untouched");
    drop(probe);
    s.mark("probes freed");

    let bytes = std::fs::read(&path).unwrap();
    s.mark("the .skate file read whole");
    drop(bytes);
    s.mark("file bytes dropped");
    let (map, _) = crate::config::load_map(&path).unwrap();
    s.mark("load_map (SkateMap::load + fingerprint)");
    map_parts(&map);
    let mut config = crate::config::Config {
        asset_root: root.clone(),
        verification_capture: None,
        map: Some(map),
        map_path: Some(path),
        difficulty: crate::difficulty::Difficulty::default(),
        check_assets: false,
        start_paused: false,
        multiplayer: crate::multiplayer::Options::default(),
        map_fingerprint: 0,
        teleport: None,
        hero_clip: None,
        skate2_physics_dir: None,
        physics_override: None,
    };
    let loaded = crate::load_startup(&config).unwrap();
    s.mark("load_startup (physics, manifest, graphs, skater)");
    physics_parts(&loaded.physics);

    crate::skate_world::assume_bc7();
    let mut world = World::new();
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<StandardMaterial>>();
    world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
    world.init_resource::<Assets<Image>>();
    let mut scene = crate::map_render::PreparedScene::new(&world);
    scene.prepare(config.map.as_ref(), &root);
    s.mark("PreparedScene::prepare");
    scene.publish(&mut world);
    s.mark("PreparedScene::publish");
    scene_parts(&world);
    hand_off_render_only(&mut world);
    s.mark("render-only scene assets handed off");
    drop(config.map.take());
    s.mark("map dropped (world::spawn)");
    let audio = boot_audio(&root);
    s.mark("audio game side booted");
    // The menus' parsed movies (each screen keeps its Movie; the JSON is freed after the load).
    let mut movies = Vec::new();
    for manifest in [crate::core_menu::manifest(&root), crate::challenge_map::manifest(&root)]
        .into_iter()
        .chain(crate::create_skater::manifests(&root))
    {
        let json = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&manifest).unwrap()).unwrap();
        if json.get("format").is_some_and(|f| f != "skate3-boot" && f != "skate3-popup") {
            movies.push(crate::apt_movie::Movie::load(&json).unwrap());
        }
        drop(json);
        s.mark(&format!("movie {}", manifest.file_name().unwrap().to_string_lossy()));
    }

    // Each part's size: what dropping it frees.
    drop(movies);
    s.mark("drop: menu movies");
    drop(audio);
    s.mark("drop: audio game side");
    drop(world);
    s.mark("drop: scene world");
    let crate::Loaded { manifest, graphs, physics, skater, controls } = loaded;
    drop(physics);
    s.mark("drop: physics (collision, grind, props)");
    drop(skater);
    s.mark("drop: skater runtime");
    drop(graphs);
    s.mark("drop: stock graphs");
    drop(manifest);
    s.mark("drop: asset manifest");
    drop(controls);
    s.mark("drop: controls");
}

/// Does VRAM count in the process's private commit? Headless wgpu on Vulkan (as Bevy runs it), no game or window:
/// 4 x 64 MB device-local textures created, written through queue.write_texture (staging) and dropped, then a
/// 256 MB vertex buffer as the mesh allocator's slabs are.
#[test]
#[ignore = "explicit headless GPU probe: holds 256 MB of VRAM for a moment"]
fn vram_commit_probe() {
    bevy::tasks::block_on(async {
        let mut s = Stages(process_memory(true));
        s.mark("start");
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::VULKAN, ..Default::default() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        eprintln!("PROBE adapter={:?}", adapter.get_info());
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let wait = || device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        s.mark("vulkan device");
        let size = wgpu::Extent3d { width: 4096, height: 4096, depth_or_array_layers: 1 };
        let textures: Vec<_> = (0..4)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();
        wait();
        s.mark("4 x 64 MB textures created, no data");
        let pixels = vec![7u8; 64 << 20];
        s.mark("64 MB of source pixels");
        for texture in &textures {
            let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4096 * 4), rows_per_image: None };
            queue.write_texture(texture.as_image_copy(), &pixels, layout, size);
            queue.submit([]);
            wait();
        }
        s.mark("each written via write_texture");
        drop(pixels);
        drop(textures);
        wait();
        s.mark("textures + source dropped");
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 << 20,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        wait();
        s.mark("256 MB vertex buffer");
        drop(buffer);
        wait();
        s.mark("buffer dropped");
    });
}

/// The ambience beds' PCM over a tour of the Skate 3 districts (Beds::prefetch as the map transition asks it, then
/// Beds::enter as the worker calls it on the map change), through the PCM cache.
#[test]
#[ignore = "requires the installation; measurement, run alone with --nocapture"]
fn bed_memory() {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let root = root.canonicalize().unwrap();
    let mut s = Stages(process_memory(true));
    let (mut runtime, _skater, world) = boot_audio(&root);
    s.mark("audio game side booted");
    let world = world.unwrap();
    let mut beds = skate_data::audio::title::Beds::default();
    for map in ["", "University", "DownTown", "Industrial", "MegaPark", "University", "Skate_1_San_Vanelona", "DownTown"] {
        // As the map transition announces it: the loader decodes while the map loads (~5-12 s; 2 s here).
        beds.prefetch(&world, &root, map);
        std::thread::sleep(std::time::Duration::from_secs(2));
        s.mark(&format!("beds: prefetch {map:?}"));
        let started = std::time::Instant::now();
        let old = skate_data::audio::title::OldGame::of(map).is_some();
        let added = beds.enter(&mut runtime, &world, &root, map, old).unwrap();
        s.mark(&format!("beds: enter {map:?} +{added} in {:.2}s, {} installed", started.elapsed().as_secs_f32(), beds.len()));
    }
}

/// Highest private commit a 2 ms sampler saw since the last `take`.
struct PeakSampler {
    max: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl PeakSampler {
    fn start() -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        let max = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (m, s) = (max.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Relaxed) {
                m.fetch_max(process_memory(false).private, Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        Self { max, stop }
    }
    fn take(&self) -> usize {
        let now = process_memory(false).private;
        self.max.swap(now, std::sync::atomic::Ordering::Relaxed).max(now)
    }
}
impl Drop for PeakSampler {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn scene_world() -> World {
    let mut world = World::new();
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<StandardMaterial>>();
    world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
    world.init_resource::<Assets<Image>>();
    world
}

/// One map switch's CPU side as map_transition runs it (load + validate, then the scene prepared on one thread while
/// physics builds on this one, the images/meshes held in the stream until the loader returns, the map dropped, then
/// the stream drained and handed to the render world). Prints the peak private commit above `base` per phase.
fn switch_to(root: &std::path::Path, path: &std::path::Path, sampler: &PeakSampler) -> (World, GamePhysics) {
    let base = process_memory(false).private;
    let mb = |b: usize| (b as f64 - base as f64) / 1048576.;
    sampler.take();
    let started = std::time::Instant::now();
    let (map, _) = crate::config::load_map(path).unwrap();
    let read = sampler.take();
    let read_ms = started.elapsed().as_millis();
    if std::env::var_os("SKATE_LOAD_PEAK_PARTS").is_some() {
        map_parts(&map);
        let spare: usize = map.textures.iter().map(|t| t.rgba.capacity() - t.rgba.len()).sum();
        eprintln!("SKATE_MEMORY_PARTS   texture spare capacity {:.1} MB", spare as f64 / 1048576.);
        let mut seen = std::collections::HashSet::new();
        let duplicate: usize = map.textures.iter().filter(|t| !seen.insert((t.width, t.height, t.color_space, &t.rgba))).map(|t| t.rgba.len()).sum();
        eprintln!("SKATE_MEMORY_PARTS   duplicate texture pixels {:.1} MB", duplicate as f64 / 1048576.);
    }
    crate::skate_world::validate_runtime(&map).unwrap();
    if std::env::var_os("SKATE_LOAD_PEAK_PHYSICS").is_some() {
        // Physics alone: its own transient over what it keeps.
        let before = process_memory(false).private;
        sampler.take();
        let physics = GamePhysics::load_at(root, Some(&map), crate::difficulty::Difficulty::default(), None).unwrap();
        let (peak, kept) = (sampler.take(), process_memory(false).private);
        physics_parts(&physics);
        drop(physics);
        eprintln!(
            "SKATE_LOAD_PHYSICS peak {:.0} kept {:.0} MB above the map",
            (peak - before) as f64 / 1048576.,
            (kept as f64 - before as f64) / 1048576.
        );
    }
    let mut world = scene_world();
    let mut scene = crate::map_render::PreparedScene::new(&world);
    let mut streamed = scene.stream();
    let started = std::time::Instant::now();
    let timeline = std::sync::Mutex::new(Vec::new());
    let done = std::sync::atomic::AtomicBool::new(false);
    let physics = std::thread::scope(|scope| {
        let render = scope.spawn(|| {
            scene.prepare(Some(&map), root);
            timeline.lock().unwrap().push(format!("render done {} ms", started.elapsed().as_millis()));
        });
        if std::env::var_os("SKATE_LOAD_PEAK_PARTS").is_some() {
            scope.spawn(|| {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let mb = (process_memory(false).private as f64 - base as f64) / 1048576.;
                    timeline.lock().unwrap().push(format!("{}:{mb:.0}", started.elapsed().as_millis() / 100 * 100));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            });
        }
        let physics = GamePhysics::load_at(root, Some(&map), crate::difficulty::Difficulty::default(), None).unwrap();
        timeline.lock().unwrap().push(format!("physics done {} ms", started.elapsed().as_millis()));
        render.join().unwrap();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        physics
    });
    if std::env::var_os("SKATE_LOAD_PEAK_PARTS").is_some() {
        eprintln!("SKATE_LOAD_TIMELINE {}", timeline.lock().unwrap().join(" "));
    }
    let prepare = sampler.take();
    let prepare_ms = started.elapsed().as_millis();
    let held = process_memory(false).private;
    drop(map);
    let dropped = process_memory(false).private;
    while !streamed.drain(&mut world) {}
    scene.publish(&mut world);
    drop(streamed);
    hand_off_render_only(&mut world);
    let upload = sampler.take();
    eprintln!(
        "SKATE_LOAD_PEAK {:<22} read {:>6.0} ({read_ms} ms) prepare {:>6.0} ({prepare_ms} ms) loader done {:>6.0} map dropped {:>6.0} upload {:>6.0} settled {:>6.0} MB above the pre-load private {:.0} MB",
        path.file_stem().unwrap().to_string_lossy(),
        mb(read),
        mb(prepare),
        mb(held),
        mb(dropped),
        mb(upload),
        mb(process_memory(false).private),
        base as f64 / 1048576.
    );
    (world, physics)
}

/// The load peak per map, then University <-> Skate 1 SV x4 (the old world alive while the next loads, as in game)
/// to see whether the CPU side returns each cycle. `SKATE_LOAD_PEAK_MAPS=a,b` (installation map stems) overrides the
/// maps. `SKATE3_ASSET_ROOT=… cargo test … load_peak -- --ignored --nocapture --test-threads 1`
#[test]
#[ignore = "requires the installation; measurement, run alone with --nocapture"]
fn load_peak() {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let root = root.canonicalize().unwrap();
    let maps = root.parent().unwrap().join("maps");
    let names = std::env::var("SKATE_LOAD_PEAK_MAPS")
        .unwrap_or_else(|_| "New_San_Vanelona,Skate_1_San_Vanelona,University".into());
    crate::skate_world::assume_bc7();
    let sampler = PeakSampler::start();
    let mut s = Stages(process_memory(true));
    s.mark("start");
    for name in names.split(',') {
        let loaded = switch_to(&root, &maps.join(format!("{name}.skate")), &sampler);
        drop(loaded);
        s.mark(&format!("{name} dropped"));
    }
    let mut live = None;
    for cycle in 0..4 {
        for name in ["Skate_1_San_Vanelona", "University"] {
            let next = switch_to(&root, &maps.join(format!("{name}.skate")), &sampler);
            drop(live.replace(next));
            s.mark(&format!("cycle {cycle}: {name} live, old dropped"));
        }
    }
    drop(live);
    s.mark("all dropped");
}

/// Every byte the decoded package carries (Vec capacity aside).
fn map_digest(map: &SkateMap) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    let floats = |h: &mut std::hash::DefaultHasher, v: &[f32]| v.iter().for_each(|x| h.write_u32(x.to_bits()));
    h.write_u8(map.version);
    map.name.hash(&mut h);
    floats(&mut h, &map.spawn);
    floats(&mut h, &[map.heading]);
    floats(&mut h, &map.environment);
    for m in &map.materials {
        format!("{m:?}").hash(&mut h);
    }
    for t in &map.textures {
        (&t.name, t.width, t.height, t.color_space, &t.rgba).hash(&mut h);
    }
    for v in &map.geometry.vertices {
        floats(&mut h, &v.position);
        floats(&mut h, &v.normal);
        floats(&mut h, &v.uv);
        floats(&mut h, &v.lightmap_uv);
        h.write_u32(v.material);
        v.decal_uv.map(|uv| uv.map(f32::to_bits)).hash(&mut h);
        v.tangent_frame.hash(&mut h);
    }
    map.geometry.indices.hash(&mut h);
    for c in &map.geometry.collision {
        c.points.iter().for_each(|p| floats(&mut h, p));
        (c.surface, c.material, c.native_edges).hash(&mut h);
    }
    format!("{:?}{:?}{:?}{:?}", map.rails, map.doors, map.lights, map.routes).hash(&mut h);
    for e in &map.extensions {
        (e.tag, e.schema, &e.payload).hash(&mut h);
    }
    h.finish()
}

/// The prepared scene as it commits: every image (bytes, format, size, mips, sampler, view) and mesh (each attribute's
/// bytes, indices) in handle order, every material, and every entity in spawn order (name, mesh/material handles,
/// transform, lightmap, visibility, bounds, prop piece).
fn scene_digest(world: &mut World) -> u64 {
    use crate::retail_render::{PropMaterial, RetailBlendMaterial, RetailSkyMaterial, RetailWorldMaterial};
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    for (id, image) in world.resource::<Assets<Image>>().iter() {
        format!("{id:?}{:?}{:?}{:?}", image.texture_descriptor, image.sampler, image.texture_view_descriptor).hash(&mut h);
        image.data.hash(&mut h);
    }
    for (id, mesh) in world.resource::<Assets<Mesh>>().iter() {
        format!("{id:?}{:?}", mesh.primitive_topology()).hash(&mut h);
        for (attribute, values) in mesh.attributes() {
            format!("{:?}", attribute.id).hash(&mut h);
            values.get_bytes().hash(&mut h);
        }
        match mesh.indices() {
            Some(bevy::mesh::Indices::U32(i)) => i.hash(&mut h),
            Some(bevy::mesh::Indices::U16(i)) => i.hash(&mut h),
            None => {}
        }
    }
    fn materials<M: Asset + std::fmt::Debug>(world: &World, h: &mut std::hash::DefaultHasher) {
        for (id, m) in world.resource::<Assets<M>>().iter() {
            format!("{id:?}{m:?}").hash(h);
        }
    }
    materials::<StandardMaterial>(world, &mut h);
    materials::<PropMaterial>(world, &mut h);
    materials::<RetailWorldMaterial>(world, &mut h);
    materials::<RetailBlendMaterial>(world, &mut h);
    materials::<RetailSkyMaterial>(world, &mut h);
    let mut entities: Vec<(Entity, String)> = world
        .query_filtered::<(
            Entity,
            Option<&Name>,
            Option<&Mesh3d>,
            (
                Option<&MeshMaterial3d<RetailWorldMaterial>>,
                Option<&MeshMaterial3d<RetailBlendMaterial>>,
                Option<&MeshMaterial3d<PropMaterial>>,
                Option<&MeshMaterial3d<StandardMaterial>>,
                Option<&MeshMaterial3d<RetailSkyMaterial>>,
            ),
            Option<&Transform>,
            Option<&bevy::pbr::Lightmap>,
            Option<&Visibility>,
            Option<&bevy::camera::primitives::Aabb>,
            Option<&crate::physics::props::PropPiece>,
        ), With<crate::map_render::MapEntity>>()
        .iter(world)
        .map(|(e, name, mesh, (a, b, c, d, f), transform, lightmap, visibility, aabb, piece)| {
            let lightmap = lightmap.map(|l| (l.image.id(), l.uv_rect, l.bicubic_sampling));
            let ids = (a.map(|m| m.id()), b.map(|m| m.id()), c.map(|m| m.id()), d.map(|m| m.id()), f.map(|m| m.id()));
            (e, format!("{name:?}{:?}{ids:?}{transform:?}{lightmap:?}{visibility:?}{aabb:?}{:?}", mesh.map(|m| m.id()), piece.map(|p| p.0)))
        })
        .collect();
    entities.sort_by_key(|e| e.0);
    entities.len().hash(&mut h);
    for (_, line) in entities {
        line.hash(&mut h);
    }
    h.finish()
}

fn physics_digest(physics: &GamePhysics) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    for t in physics.world.triangles() {
        format!("{t:?}").hash(&mut h);
    }
    format!("{:?}", physics.world.query_metadata().map(|m| (m.packed_surfaces.len(), m.meshes.len()))).hash(&mut h);
    h.finish()
}

/// Oracle for the load-path memory work: the decoded package, the prepared scene and the collision world, digested on
/// University, New San Vanelona and Skate 1 SV. With `SKATE_LOAD_ORACLE=<file>`: lines already in the file must match
/// (the "before" run wrote them), new ones are appended.
#[test]
#[ignore = "requires the installation; ~1 min per city"]
fn load_oracle() {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let root = root.canonicalize().unwrap();
    let file = std::env::var_os("SKATE_LOAD_ORACLE").map(std::path::PathBuf::from);
    let known = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
    crate::skate_world::assume_bc7();
    for name in ["University", "New_San_Vanelona", "Skate_1_San_Vanelona"] {
        let path = root.parent().unwrap().join("maps").join(format!("{name}.skate"));
        let (map, _) = crate::config::load_map(&path).unwrap();
        let decoded = map_digest(&map);
        let mut world = scene_world();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        let mut streamed = scene.stream();
        let physics = std::thread::scope(|scope| {
            let render = scope.spawn(|| scene.prepare(Some(&map), &root));
            let physics = GamePhysics::load_at(&root, Some(&map), crate::difficulty::Difficulty::default(), None).unwrap();
            render.join().unwrap();
            physics
        });
        drop(map);
        while !streamed.drain(&mut world) {}
        scene.publish(&mut world);
        let line = format!("{name} map={decoded:016x} scene={:016x} physics={:016x}", scene_digest(&mut world), physics_digest(&physics));
        eprintln!("SKATE_LOAD_ORACLE {line}");
        match known.lines().find(|l| l.split(' ').next() == Some(name)) {
            Some(before) => assert_eq!(before, line, "{name}: the load path changed what it builds"),
            None => {
                if let Some(file) = &file {
                    use std::io::Write;
                    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(file).unwrap();
                    writeln!(out, "{line}").unwrap();
                }
            }
        }
    }
}

/// The steady-level creep across map switches, without the game: Skate 1 SV's and University's real prepared images
/// (BC7) and mesh bytes uploaded to a headless Vulkan device the way the switch does it (old map's GPU assets dropped
/// first, then the new ones created with their data), x4, private / ws after each switch. `SKATE_PROBE_HINTS=memory`
/// uses wgpu's MemoryUsage allocator hints instead of Performance (Bevy's default).
#[test]
#[ignore = "explicit headless GPU probe: uploads two maps' images and meshes four times"]
fn gpu_switch_probe() {
    use wgpu::util::DeviceExt;
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let root = root.canonicalize().unwrap();
    crate::skate_world::assume_bc7();
    // Each map's images (descriptor, bytes) and mesh bytes, as the render world receives them.
    let scenes: Vec<(&str, Vec<(wgpu::TextureDescriptor<'static>, Vec<u8>)>, usize)> = ["Skate_1_San_Vanelona", "University"]
        .into_iter()
        .map(|name| {
            let (map, _) = crate::config::load_map(&root.parent().unwrap().join("maps").join(format!("{name}.skate"))).unwrap();
            let mut world = scene_world();
            let mut scene = crate::map_render::PreparedScene::new(&world);
            scene.prepare(Some(&map), &root);
            scene.publish(&mut world);
            let images = world
                .resource::<Assets<Image>>()
                .iter()
                .filter_map(|(_, image)| {
                    let d = &image.texture_descriptor;
                    Some((
                        wgpu::TextureDescriptor {
                            label: None,
                            size: d.size,
                            mip_level_count: d.mip_level_count,
                            sample_count: 1,
                            dimension: d.dimension,
                            format: d.format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        },
                        image.data.clone()?,
                    ))
                })
                .collect();
            let meshes = world.resource::<Assets<Mesh>>().iter().map(|(_, m)| mesh_cpu_bytes(m)).sum();
            (name, images, meshes)
        })
        .collect();
    bevy::tasks::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::VULKAN, ..Default::default() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let memory_hints = match std::env::var("SKATE_PROBE_HINTS").as_deref() {
            Ok("memory") => wgpu::MemoryHints::MemoryUsage,
            _ => wgpu::MemoryHints::Performance,
        };
        eprintln!("PROBE adapter={:?} hints={memory_hints:?}", adapter.get_info().name);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                memory_hints,
                ..Default::default()
            })
            .await
            .unwrap();
        let wait = || device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mut s = Stages(process_memory(false));
        s.mark("device");
        let mut live: (Vec<wgpu::Texture>, Vec<wgpu::Buffer>) = Default::default();
        for cycle in 0..4 {
            for (name, images, meshes) in &scenes {
                drop(std::mem::take(&mut live));
                queue.submit([]);
                wait();
                // Bevy's mesh allocator packs vertex data into slabs (up to 512 MB); 256 MB buffers here.
                let mut left = *meshes;
                while left > 0 {
                    let size = left.min(256 << 20) as u64;
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: size.next_multiple_of(4),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    queue.write_buffer(&buffer, 0, &vec![1u8; size.next_multiple_of(4) as usize]);
                    live.1.push(buffer);
                    left -= size as usize;
                }
                for (i, (desc, data)) in images.iter().enumerate() {
                    live.0.push(device.create_texture_with_data(&queue, desc, wgpu::util::TextureDataOrder::LayerMajor, data));
                    // The switch streams 64 MB a frame; a frame's submit and the allocator's cleanup follow.
                    if i % 64 == 0 {
                        queue.submit([]);
                        wait();
                    }
                }
                queue.submit([]);
                wait();
                s.mark(&format!("cycle {cycle}: {name} uploaded ({} images)", images.len()));
            }
        }
        drop(live);
        queue.submit([]);
        wait();
        s.mark("all dropped");
    });
}
