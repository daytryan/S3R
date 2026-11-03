//! Opt-in repeatable frame/physics timing: SKATE_PERF_REPORT=path.json.
use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};
use std::sync::{Arc, Mutex};
use std::{collections::BTreeMap, sync::OnceLock};
use std::{path::PathBuf, time::Instant};

static EPOCH: OnceLock<Instant> = OnceLock::new();
static SLOW_SECTIONS: Mutex<Vec<(&'static str, f64, f64)>> = Mutex::new(Vec::new());
fn timestamp(now: Instant) -> f64 {
    EPOCH
        .get()
        .map_or(0., |epoch| now.duration_since(*epoch).as_secs_f64() * 1000.)
}

/// Process memory in bytes (zero off Windows): private commit, its peak since launch, the working set, and the process
/// heap's live and committed bytes (Rust's System allocator and the CRT both allocate there). Diagnostics only:
/// SKATE_PERF_REPORT's memory samples and tests/memory_cost.rs. The heap figures only with `heap`: HeapSummary walks
/// the whole heap under its lock (~0.5 s while a map loads, every allocating thread waiting on it).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProcessMemory {
    pub private: usize,
    pub peak_private: usize,
    pub working_set: usize,
    pub heap_live: usize,
    pub heap_committed: usize,
}
pub(crate) fn process_memory(heap_summary: bool) -> ProcessMemory {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::{Memory, Threading::GetCurrentProcess};
        // PROCESS_MEMORY_COUNTERS_EX: windows-sys has it behind a feature this crate doesn't enable.
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            page_faults: u32,
            peak_working_set: usize,
            working_set: usize,
            pools: [usize; 4],
            pagefile: usize,
            peak_pagefile: usize,
            private: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn K32GetProcessMemoryInfo(process: *mut std::ffi::c_void, counters: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters { cb: size_of::<Counters>() as u32, ..Default::default() };
        let mut heap = Memory::HEAP_SUMMARY { cb: size_of::<Memory::HEAP_SUMMARY>() as u32, ..Default::default() };
        // SAFETY: both structs are sized as the calls expect; the handles are this process's own.
        unsafe {
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
            if heap_summary {
                Memory::HeapSummary(Memory::GetProcessHeap(), 0, &mut heap);
            }
        }
        ProcessMemory {
            private: c.private,
            peak_private: c.peak_pagefile,
            working_set: c.working_set,
            heap_live: heap.cbAllocated,
            heap_committed: heap.cbCommitted,
        }
    }
    #[cfg(not(windows))]
    ProcessMemory::default()
}

/// The CPU copy Bevy keeps of an asset whose usage includes MAIN_WORLD (none once extraction took it).
pub(crate) fn image_cpu_bytes(image: &Image) -> usize {
    image.data.as_ref().map_or(0, Vec::len)
}
pub(crate) fn mesh_cpu_bytes(mesh: &Mesh) -> usize {
    let indices = mesh.try_indices().map_or(0, |i| i.len() * if matches!(i, bevy::mesh::Indices::U16(_)) { 2 } else { 4 });
    mesh.try_attributes().map_or(0, |a| a.map(|(_, v)| v.get_bytes().len()).sum::<usize>()) + indices
}
/// Once a second: [elapsed_ms, private, working set, heap live, heap committed (0 without SKATE_PERF_HEAP), main-world image
/// and mesh copies] MB.
static MEMORY: Mutex<Vec<[f64; 7]>> = Mutex::new(Vec::new());
fn memory_samples(images: Res<Assets<Image>>, meshes: Res<Assets<Mesh>>, time: Res<Time<Real>>, mut next: Local<f32>) {
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 1.;
    static HEAP: OnceLock<bool> = OnceLock::new();
    let m = process_memory(*HEAP.get_or_init(|| std::env::var_os("SKATE_PERF_HEAP").is_some()));
    let mb = |b: usize| b as f64 / 1048576.;
    let image_bytes = images.iter().map(|(_, i)| image_cpu_bytes(i)).sum();
    let mesh_bytes = meshes.iter().map(|(_, m)| mesh_cpu_bytes(m)).sum();
    let row = [timestamp(Instant::now()), mb(m.private), mb(m.working_set), mb(m.heap_live), mb(m.heap_committed), mb(image_bytes), mb(mesh_bytes)];
    MEMORY.lock().unwrap().push(row);
}
/// At the report: the largest main-world CPU copies, with their asset paths.
fn log_cpu_assets(images: &Assets<Image>, meshes: &Assets<Mesh>, server: &AssetServer) {
    let mut rows: Vec<(usize, String)> = images
        .iter()
        .map(|(id, i)| (image_cpu_bytes(i), format!("image {:?} {:?}", i.size(), server.get_path(id))))
        .chain(meshes.iter().map(|(id, m)| (mesh_cpu_bytes(m), format!("mesh {:?}", server.get_path(id)))))
        .filter(|r| r.0 > 0)
        .collect();
    let total: usize = rows.iter().map(|r| r.0).sum();
    eprintln!("SKATE_PERF_MEMORY_CPU_ASSETS count={} mb={:.1}", rows.len(), total as f64 / 1048576.);
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    for (bytes, what) in rows.into_iter().take(20) {
        eprintln!("SKATE_PERF_MEMORY_CPU_ASSETS {:>8.2} MB {what}", bytes as f64 / 1048576.);
    }
}

static SECTIONS: Mutex<BTreeMap<&'static str, (f64, u64)>> = Mutex::new(BTreeMap::new());
pub(crate) struct Scope {
    name: &'static str,
    start: Option<Instant>,
}
impl Scope {
    pub(crate) fn new(name: &'static str) -> Self {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        Self {
            name,
            start: ENABLED
                .get_or_init(|| std::env::var_os("SKATE_PERF_REPORT").is_some())
                .then(Instant::now),
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            if elapsed >= 2. {
                let mut slow = SLOW_SECTIONS.lock().unwrap();
                if slow.len() < 256 {
                    slow.push((self.name, timestamp(start), elapsed));
                }
            }
            let mut sections = SECTIONS.lock().unwrap();
            let entry = sections.entry(self.name).or_default();
            entry.0 += elapsed;
            entry.1 += 1;
        }
    }
}

#[derive(Resource)]
pub(crate) struct Performance {
    path: PathBuf,
    start: Option<Instant>,
    frame_start: Instant,
    previous: Instant,
    physics_ms: f64,
    ticks: u32,
    samples: Vec<[f64; 4]>,
    sample_times: Vec<f64>,
    render: Arc<Mutex<Vec<[f64; 7]>>>,
}
impl Performance {
    pub(crate) fn physics(&mut self, elapsed: std::time::Duration) {
        self.physics_ms += elapsed.as_secs_f64() * 1000.;
        self.ticks += 1;
    }
}

pub(crate) struct PerformancePlugin;
impl Plugin for PerformancePlugin {
    fn build(&self, app: &mut App) {
        let Some(path) = std::env::var_os("SKATE_PERF_REPORT") else {
            return;
        };
        let now = Instant::now();
        let _ = EPOCH.set(now);
        let render = Arc::new(Mutex::new(Vec::with_capacity(16384)));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(RenderPerformance {
                    start: None,
                    frame: now,
                    prepared: now,
                    mark: now,
                    phases: [0.; 3],
                    samples: render.clone(),
                })
                .add_systems(Render, render_begin.before(RenderSystems::ExtractCommands))
                .add_systems(Render, phase_counts.after(RenderSystems::PhaseSort).before(RenderSystems::Render))
                .add_systems(
                    Render,
                    render_assets_done
                        .after(RenderSystems::PrepareMeshes)
                        .after(RenderSystems::PrepareAssets)
                        .before(RenderSystems::ManageViews),
                )
                .add_systems(
                    Render,
                    render_views_done
                        .after(RenderSystems::ManageViews)
                        .before(RenderSystems::Queue),
                )
                .add_systems(
                    Render,
                    render_queue_done
                        .after(RenderSystems::Queue)
                        .before(RenderSystems::PhaseSort),
                )
                .add_systems(
                    Render,
                    render_prepared
                        .after(RenderSystems::Prepare)
                        .before(RenderSystems::Render),
                )
                .add_systems(Render, render_finish.after(RenderSystems::PostCleanup));
            render_app
                .init_resource::<MeshBindingTimer>()
                .add_systems(
                    Render,
                    mesh_binding_begin
                        .in_set(RenderSystems::PrepareBindGroups)
                        .before(bevy::pbr::prepare_mesh_bind_groups),
                )
                .add_systems(
                    Render,
                    mesh_binding_end
                        .in_set(RenderSystems::PrepareBindGroups)
                        .after(bevy::pbr::prepare_mesh_bind_groups),
                );
        }
        // Benchmark measurements must not depend on whether the window has focus.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
        app.add_systems(Startup, report_adapter);
        if std::env::var_os("SKATE_PERF_SWITCH_EVERY").is_some() {
            app.add_plugins(bevy::render::diagnostic::MeshAllocatorDiagnosticPlugin)
                .add_systems(Update, cycle_maps);
            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                render_app.add_systems(bevy::render::ExtractSchedule, (gpu_image_census, slab_census, view_census));
                // Experiment knob: cap mesh slab size (MB) to see what the slab high-water costs and saves.
                if let Some(mb) = std::env::var("SKATE_PERF_MAX_SLAB_MB").ok().and_then(|v| v.parse::<u64>().ok()) {
                    render_app.insert_resource(bevy::render::mesh::allocator::MeshAllocatorSettings {
                        max_slab_size: mb << 20,
                        ..default()
                    });
                }
            }
        } else if std::env::var_os("SKATE_PERF_SWITCH_MAP").is_some() {
            app.add_systems(Update, switch_map);
        }
        if std::env::var_os("SKATE_PERF_MODES").is_some() {
            app.add_systems(Update, cycle_modes);
        }
        if std::env::var_os("SKATE_PERF_CAMERA_SWEEP").is_some() {
            app.add_systems(
                Update,
                sweep_camera.after(crate::app::FrameSet::Verification),
            );
        }
        if std::env::var_os("SKATE_PERF_TOUR").is_some() {
            app.add_systems(Update, tour);
        }
        if std::env::var_os("SKATE_PERF_EDIT_SKATER").is_some() {
            app.add_systems(Update, edit_skater_in_and_out);
        }
        if std::env::var_os("SKATE_PERF_CENSUS").is_some() {
            app.add_systems(Last, census);
        }
        if std::env::var_os("SKATE_PERF_FLY").is_some() {
            app.add_systems(
                Update,
                fly_camera
                    .after(crate::app::FrameSet::Verification)
                    .after(sweep_camera),
            );
        }
        app.insert_resource(Performance {
            path: path.into(),
            start: None,
            frame_start: now,
            previous: now,
            physics_ms: 0.,
            ticks: 0,
            samples: Vec::with_capacity(16384),
            sample_times: Vec::with_capacity(16384),
            render,
        })
        .add_systems(First, begin)
        .add_systems(Last, (finish, image_events, visible_batches, memory_samples));
        // SKATE_PERF_HEAVY: the heaviest visible meshes every 0.5 s (~10 lines). Always on, it put a 40-60 ms hitch in every
        // report whose stderr went to a file (~6.7 ms a line on Windows; bug 10-25 #6), so it runs only when asked for.
        if std::env::var_os("SKATE_PERF_HEAVY").is_some() {
            app.add_systems(Last, heavy_meshes);
            if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
                render_app.add_systems(Render, mesh_triangles.after(RenderSystems::PhaseSort).before(RenderSystems::Render));
            }
        }
    }
}

/// Every 300 frames, the average world batches drawn per frame (SKATE_PERF_VISIBLE): opaque (binned, multi-drawn)
/// against blended (depth sorted, one draw each).
fn visible_batches(
    opaque: Query<&ViewVisibility, (With<MeshMaterial3d<crate::retail_render::RetailWorldMaterial>>, Without<crate::skate_world::shadow_geometry::ShadowProxy>)>,
    blended: Query<&ViewVisibility, With<MeshMaterial3d<crate::retail_render::RetailBlendMaterial>>>,
    mut sums: Local<(u32, usize, usize)>,
) {
    let (frames, o, b) = &mut *sums;
    *frames += 1;
    *o += opaque.iter().filter(|v| v.get()).count();
    *b += blended.iter().filter(|v| v.get()).count();
    if *frames == 300 {
        eprintln!("SKATE_PERF_VISIBLE opaque={} blended={} of {}/{}", *o / 300, *b / 300, opaque.iter().len(), blended.iter().len());
        *sums = default();
    }
}

/// Perf 10-10: Skate 1 Slappys' prepass jumped 0.6 -> 7 GPU ms facing one way (geometry-bound). Every 0.5 s, the
/// main camera's heading, visible triangles and its heaviest meshes.
/// Map meshes keep no main-world copy: the render world publishes their triangle counts.
static MESH_TRIANGLES: std::sync::LazyLock<Mutex<std::collections::HashMap<AssetId<Mesh>, u32>>> = std::sync::LazyLock::new(default);
fn mesh_triangles(meshes: Res<bevy::render::render_asset::RenderAssets<bevy::render::mesh::RenderMesh>>, mut frames: Local<u32>) {
    *frames += 1;
    if *frames % 60 != 1 {
        return;
    }
    let mut out = MESH_TRIANGLES.lock().unwrap();
    for (id, mesh) in meshes.iter() {
        let count = match mesh.buffer_info {
            bevy::render::mesh::RenderMeshBufferInfo::Indexed { count, .. } => count,
            bevy::render::mesh::RenderMeshBufferInfo::NonIndexed => mesh.vertex_count,
        };
        out.insert(id, count / 3);
    }
}
fn heavy_meshes(
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::visibility::VisibleEntities), With<Camera3d>>,
    meshes_q: Query<(&Mesh3d, Option<&Name>, Option<&bevy::camera::primitives::Aabb>)>,
    time: Res<Time<Real>>,
    mut next: Local<f32>,
) {
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 0.5;
    let Some((_, transform, visible)) = cameras.iter().find(|(c, ..)| c.is_active) else { return };
    let eye = transform.translation();
    // Visible triangles whose nearest point lies past each Draw Distance far plane (400/650/1000 m) and 2 km.
    let mut past = [0usize; 4];
    for (mesh, _, aabb) in visible.get(std::any::TypeId::of::<Mesh3d>()).iter().filter_map(|&e| meshes_q.get(e).ok()) {
        let Some(aabb) = aabb else { continue };
        let tris = MESH_TRIANGLES.lock().unwrap().get(&mesh.0.id()).copied().unwrap_or(0) as usize;
        let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
        let d = ((eye - c).abs() - h).max(Vec3::ZERO).length();
        for (slot, limit) in past.iter_mut().zip([400., 650., 1000., 2000.]) {
            if d > limit {
                *slot += tris;
            }
        }
    }
    eprintln!("SKATE_PERF_FAR past400={} past650={} past1000={} past2000={}", past[0], past[1], past[2], past[3]);
    let mut top: Vec<(usize, String)> = visible
        .get(std::any::TypeId::of::<Mesh3d>())
        .iter()
        .filter_map(|&e| meshes_q.get(e).ok())
        .map(|(mesh, name, aabb)| {
            let tris = MESH_TRIANGLES.lock().unwrap().get(&mesh.0.id()).copied().unwrap_or(0) as usize;
            let extent = aabb.map_or(Vec3::ZERO, |a| Vec3::from(a.half_extents) * 2.);
            let center = aabb.map_or(Vec3::ZERO, |a| Vec3::from(a.center));
            (tris, format!("{} c={:.0},{:.0},{:.0} size={:.0}x{:.0}x{:.0}", name.map_or("?", |n| n.as_str()), center.x, center.y, center.z, extent.x, extent.y, extent.z))
        })
        .collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.0));
    let f = transform.forward();
    eprintln!(
        "SKATE_PERF_HEAVY t={:.1} yaw={:.0} pos={:.0},{:.0},{:.0} meshes={} tris={}",
        time.elapsed_secs(),
        f.x.atan2(f.z).to_degrees(),
        transform.translation().x,
        transform.translation().y,
        transform.translation().z,
        top.len(),
        top.iter().map(|t| t.0).sum::<usize>()
    );
    for (tris, label) in top.iter().take(8) {
        eprintln!("SKATE_PERF_HEAVY   {tris} {label}");
    }
}

/// Perf 10-10: GpuImage extract+prepare cost ~1 ms every frame on NSV, so some image is changed every frame. Every
/// 300 frames, logs the images modified or added most often (SKATE_PERF_IMAGE_EVENTS), with size and asset path.
fn image_events(
    mut events: MessageReader<AssetEvent<Image>>,
    images: Res<Assets<Image>>,
    server: Res<AssetServer>,
    mut counts: Local<std::collections::HashMap<AssetId<Image>, (u32, u32)>>,
    mut frames: Local<u32>,
) {
    for event in events.read() {
        match *event {
            AssetEvent::Modified { id } => counts.entry(id).or_default().0 += 1,
            AssetEvent::Added { id } => counts.entry(id).or_default().1 += 1,
            _ => {}
        }
    }
    *frames += 1;
    if *frames < 300 {
        return;
    }
    let mut top: Vec<_> = counts.drain().collect();
    top.sort_by_key(|(_, (m, a))| std::cmp::Reverse(m + a));
    for (id, (modified, added)) in top.into_iter().take(5) {
        let size = images.get(id).map(|i| i.size());
        let path = server.get_path(id.untyped());
        eprintln!("SKATE_PERF_IMAGE_EVENTS frames={} modified={modified} added={added} id={id:?} size={size:?} path={path:?}", *frames);
    }
    *frames = 0;
}

#[derive(Resource, Default)]
struct MeshBindingTimer(Option<Scope>);
fn mesh_binding_begin(mut timer: ResMut<MeshBindingTimer>) {
    timer.0 = Some(Scope::new("mesh_bind_groups"));
}
fn mesh_binding_end(mut timer: ResMut<MeshBindingTimer>) {
    timer.0.take();
}
fn report_adapter(
    device: Res<bevy::render::renderer::RenderDevice>,
    adapter: Res<bevy::render::renderer::RenderAdapterInfo>,
) {
    eprintln!(
        "SKATE_GPU adapter={:?} features={:?} limits={:?}",
        &**adapter,
        device.features(),
        device.limits()
    );
}

// Rendering-only benchmark: exercise changing visibility without steering the
// skater or feeding synthetic inputs into the simulation camera.
fn sweep_camera(time: Res<Time<Real>>, mut cameras: Query<&mut Transform, With<Camera3d>>, mut logged: Local<bool>) {
    // Level with the horizon (3 degrees down), as a player looks across a map: the chase camera's own tilt aimed
    // the sweep at the ground and hid the view the benchmark is for.
    // perf 11-01: SKATE_PERF_YAW=<degrees> holds one heading (a trace of one view) instead of turning.
    static YAW: OnceLock<Option<f32>> = OnceLock::new();
    let fixed = *YAW.get_or_init(|| std::env::var("SKATE_PERF_YAW").ok().and_then(|v| v.parse::<f32>().ok()));
    let turn = fixed.map_or(time.elapsed_secs() * std::f32::consts::TAU / 12., f32::to_radians);
    if !*logged {
        *logged = true;
        // A report sample's heading is (elapsed_s + (sample_ms - report_ms) / 1000) x 30 degrees.
        eprintln!("SKATE_PERF_SWEEP report_ms={:.0} elapsed_s={:.3}", timestamp(Instant::now()), time.elapsed_secs());
    }
    for mut transform in &mut cameras {
        transform.look_to(Vec3::new(turn.sin(), -0.05, turn.cos()), Vec3::Y);
    }
}
/// SKATE_PERF_FLY=<m/s>: carry the camera round a 200 m-radius loop from the
/// spawn, facing along it, so visibility and texture use keep changing as when
/// skating fast across a map. Rendering only, like the sweep.
fn fly_camera(time: Res<Time<Real>>, mut cameras: Query<&mut Transform, With<Camera3d>>) {
    const RADIUS: f32 = 200.;
    let speed: f32 = std::env::var("SKATE_PERF_FLY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30.);
    let angle = time.elapsed_secs() * speed / RADIUS;
    for mut transform in &mut cameras {
        transform.translation += Vec3::new(RADIUS * (angle.cos() - 1.), 0., RADIUS * angle.sin());
        transform.look_to(Vec3::new(-angle.sin(), 0., angle.cos()), Vec3::Y);
    }
}
/// SKATE_PERF_CENSUS=<s>: every <s> seconds (default 5) while playing normally, one line of what the world holds and
/// draws, to find what a session leaves behind (v12 list 2: frames slower after visiting Edit Skater twice): the last
/// interval's mean frame time, entities, active cameras, 3D meshes (visible / all), skinned meshes, open screens.
#[allow(clippy::type_complexity)]
fn census(
    time: Res<Time<Real>>,
    mut next: Local<f32>,
    mut frames: Local<(u32, f32)>,
    entities: Query<()>,
    cameras: Query<&Camera>,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
    skinned: Query<(), With<bevy::mesh::skinning::SkinnedMesh>>,
    screens: Option<Res<crate::apt_screen::Screens>>,
) {
    *frames = (frames.0 + 1, frames.1 + time.delta_secs());
    let every = std::env::var("SKATE_PERF_CENSUS").ok().and_then(|v| v.parse().ok()).unwrap_or(5.);
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + every;
    let visible = meshes.iter().filter(|v| v.get()).count();
    eprintln!(
        "SKATE_PERF_CENSUS at_s={:.0} frame_ms={:.2} entities={} cameras_active={} meshes={visible}/{} skinned={} screens={:?}",
        time.elapsed_secs(),
        frames.1 * 1000. / frames.0.max(1) as f32,
        entities.iter().count(),
        cameras.iter().filter(|c| c.is_active).count(),
        meshes.iter().count(),
        skinned.iter().count(),
        screens.map(|s| s.census()).unwrap_or_default(),
    );
    *frames = (0, 0.);
}
/// SKATE_PERF_EDIT_SKATER=<s>: open Edit Skater, then close it without saving, every <s> seconds (default 8),
/// so the report's frames show whether a visit leaves the game slower. Logs "SKATE_PERF_EDIT_SKATER <ms> open|close"
/// on the report's clock.
fn edit_skater_in_and_out(
    time: Res<Time<Real>>,
    customiser: Option<ResMut<crate::customiser::Customiser>>,
    mut next: Local<f32>,
) {
    let Some(mut customiser) = customiser else { return };
    let every = std::env::var("SKATE_PERF_EDIT_SKATER").ok().and_then(|v| v.parse().ok()).unwrap_or(8.);
    if *next == 0. {
        *next = warmup() as f32 + every;
    }
    if time.elapsed_secs() < *next {
        return;
    }
    *next += every;
    let open = customiser.open;
    if open {
        customiser.close_without_saving();
    } else {
        customiser.begin();
    }
    eprintln!("SKATE_PERF_EDIT_SKATER {:.0} {}", timestamp(Instant::now()), if open { "close" } else { "open" });
}
/// SKATE_PERF_TOUR=<s>: teleport the skater to each Travel destination on the map in turn, <s> seconds apiece
/// (default 8), so a run covers the whole map rather than the spawn. Pair with SKATE_INPUT_SCRIPT to keep skating.
/// Logs "SKATE_PERF_TOUR <ms> <name>" on the report's sample_elapsed_ms clock for per-spot splits.
fn tour(
    time: Res<Time<Real>>,
    travel: Res<crate::teleport_menu::Travel>,
    mut skater: ResMut<crate::physics::SkaterRuntime>,
    mut next: Local<(usize, f32)>,
) {
    let spots = travel.destinations();
    let every = std::env::var("SKATE_PERF_TOUR").ok().and_then(|v| v.parse().ok()).unwrap_or(8.);
    let now = time.elapsed_secs();
    if spots.is_empty() || now < next.1 {
        return;
    }
    let spot = &spots[next.0 % spots.len()];
    if spot.matrix.is_some_and(|m| skater.travel_to(m).is_ok()) {
        eprintln!("SKATE_PERF_TOUR {:.0} {}", timestamp(Instant::now()), spot.name);
    }
    *next = (next.0 + 1, now + every);
}
/// SKATE_PERF_SWITCH_MAP=X.skate: switch maps in-game 5 s in, as the Change
/// Location menu does, and measure only the world that the switch commits.
/// Note the switch saves X as the default map, like the menu.
fn switch_map(
    time: Res<Time<Real>>,
    mut switched: Local<bool>,
    mut requested: Local<bool>,
    mut transition: ResMut<crate::map_transition::MapTransition>,
    mut changed: MessageReader<crate::map_transition::WorldChanged>,
    mut p: ResMut<Performance>,
) {
    if changed.read().count() > 0 {
        *switched = true;
        eprintln!("SKATE_PERF_SWITCH committed");
    }
    if *switched {
        return;
    }
    if time.elapsed_secs() > 5. && !*requested && !transition.busy() {
        *requested = true;
        let path = std::path::PathBuf::from(std::env::var_os("SKATE_PERF_SWITCH_MAP").unwrap());
        let label = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        transition.request(crate::map_library::Entry {
            label,
            path: Some(path),
        });
    }
    // Restart the measurement window until the new world is in.
    p.start = None;
    p.samples.clear();
    p.sample_times.clear();
    p.render.lock().unwrap().clear();
}
/// The render world's uploaded images, and those whose main-world Image is gone (by size/format), for cycle_maps.
static GPU_IMAGES: Mutex<String> = Mutex::new(String::new());
/// Every live GPU image by size/format/mips (cycle_maps diffs it per switch), and the mesh slab census.
static GPU_IMAGE_KEYS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
static SLABS: Mutex<String> = Mutex::new(String::new());
/// Item 29: per mesh slab (8 MB or more), its buffer size, live meshes and bytes, the end of its highest allocation,
/// and how many of its meshes still have a main-world copy, to see what holds a slab open after a map leaves.
fn slab_census(
    allocator: Res<bevy::render::mesh::allocator::MeshAllocator>,
    meshes: Res<bevy::render::render_asset::RenderAssets<bevy::render::mesh::RenderMesh>>,
    main: bevy::render::Extract<Res<Assets<Mesh>>>,
    mut ids: Local<Vec<bevy::render::render_resource::BufferId>>,
    mut frame: Local<u32>,
) {
    *frame += 1;
    if *frame % 30 != 0 {
        return;
    }
    // slab (by buffer) -> (buffer bytes, meshes, live bytes, highest end bytes, main-world copies)
    let mut slabs = BTreeMap::<usize, (u64, usize, u64, u64, usize)>::new();
    let mut add = |slice: Option<bevy::render::mesh::allocator::MeshBufferSlice>, element: u64, main: bool| {
        let Some(slice) = slice else { return };
        let id = slice.buffer.id();
        let key = ids.iter().position(|&b| b == id).unwrap_or_else(|| {
            ids.push(id);
            ids.len() - 1
        });
        let e = slabs.entry(key).or_default();
        e.0 = slice.buffer.size();
        e.1 += 1;
        e.2 += u64::from(slice.range.end - slice.range.start) * element;
        e.3 = e.3.max(u64::from(slice.range.end) * element);
        e.4 += main as usize;
    };
    for (id, mesh) in meshes.iter() {
        let main = main.contains(id);
        add(allocator.mesh_vertex_slice(&id), mesh.layout.0.layout().array_stride, main);
        if let bevy::render::mesh::RenderMeshBufferInfo::Indexed { index_format, .. } = mesh.buffer_info {
            let size = if index_format == bevy::render::render_resource::IndexFormat::Uint16 { 2 } else { 4 };
            add(allocator.mesh_index_slice(&id), size, main);
        }
    }
    let mb = |b: u64| b as f64 / 1048576.;
    let mut out = format!("count={} total_mb={:.0}", allocator.slab_count(), mb(allocator.slabs_size()));
    for (slab, (size, n, live, end, main)) in slabs {
        if size >= 8 << 20 {
            out.push_str(&format!(" [#{slab} buf={:.0} meshes={n} live={:.0} end={:.0} main={main}]", mb(size), mb(live), mb(end)));
        }
    }
    *SLABS.lock().unwrap() = out;
}
static VIEWS: Mutex<String> = Mutex::new(String::new());
/// Render-world cameras still holding view textures (main a/b, depth, prepass): "live" ones were extracted last frame,
/// "stale" ones were not (an inactive camera keeps whatever it had when it went off).
fn view_census(
    views: Query<(
        Has<bevy::render::camera::ExtractedCamera>,
        Option<&bevy::render::view::ViewTarget>,
        Option<&bevy::render::view::ViewDepthTexture>,
        Option<&bevy::core_pipeline::prepass::ViewPrepassTextures>,
    )>,
    mut frame: Local<u32>,
) {
    *frame += 1;
    if *frame % 30 != 0 {
        return;
    }
    let bytes = |t: &bevy::render::render_resource::Texture| {
        let size = t.size();
        u64::from(t.format().block_copy_size(None).unwrap_or(4)) * u64::from(size.width) * u64::from(size.height)
            * u64::from(size.depth_or_array_layers)
    };
    let (mut live, mut stale, mut stale_bytes, mut sizes) = (0, 0, 0u64, Vec::new());
    for (extracted, target, depth, prepass) in &views {
        if target.is_none() && depth.is_none() && prepass.is_none() {
            continue;
        }
        if extracted {
            live += 1;
            continue;
        }
        stale += 1;
        let mut b = 0;
        if let Some(t) = target {
            b += bytes(t.main_texture()) + bytes(t.main_texture_other());
        }
        if let Some(d) = depth {
            b += bytes(&d.texture);
        }
        if let Some(p) = prepass {
            for a in [&p.depth, &p.normal, &p.motion_vectors, &p.deferred].into_iter().flatten() {
                b += bytes(&a.texture.texture);
            }
        }
        stale_bytes += b;
        sizes.push(format!("{}x{}:{:.1}", target.map_or(0, |t| t.main_texture().width()), target.map_or(0, |t| t.main_texture().height()), b as f64 / 1048576.));
    }
    *VIEWS.lock().unwrap() = format!("live={live} stale={stale} stale_mb={:.1} {sizes:?}", stale_bytes as f64 / 1048576.);
}
fn gpu_image_census(
    images: bevy::render::Extract<Res<Assets<Image>>>,
    gpu: Res<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>,
    materials: bevy::render::Extract<(
        Res<Assets<crate::retail_render::RetailWorldMaterial>>,
        Res<Assets<crate::retail_render::RetailBlendMaterial>>,
        Res<Assets<crate::retail_render::RetailSkyMaterial>>,
        Res<Assets<crate::retail_render::PropMaterial>>,
        Res<Assets<StandardMaterial>>,
    )>,
    prepared: Res<bevy::render::erased_render_asset::ErasedRenderAssets<bevy::pbr::PreparedMaterial>>,
    allocators: Res<bevy::pbr::MaterialBindGroupAllocators>,
    mut frame: Local<u32>,
) {
    *frame += 1;
    if *frame % 30 != 0 {
        return;
    }
    // Prepared materials (their bind group slots hold texture views) whose main-world asset is gone, per type.
    fn orphans<M: Asset>(id: bevy::asset::UntypedAssetId, assets: &Assets<M>, counts: &mut [usize; 2]) -> bool {
        let Ok(id) = id.try_typed::<M>() else { return false };
        counts[0] += 1;
        counts[1] += !assets.contains(id) as usize;
        true
    }
    let (world, blend, sky, props, standard) = &*materials;
    let mut counts = [[0usize; 2]; 6];
    for (id, _) in prepared.iter() {
        let [w, b, s, p, st, other] = &mut counts;
        if !(orphans(id, world, w) || orphans(id, blend, b) || orphans(id, sky, s) || orphans(id, props, p) || orphans(id, standard, st)) {
            other[0] += 1;
        }
    }
    let mut material_census = format!("prepared[total,orphan] world={:?} blend={:?} sky={:?} props={:?} standard={:?} other={}", counts[0], counts[1], counts[2], counts[3], counts[4], counts[5][0]);
    // Bindless slabs and the resources resident in them, per material type.
    for (name, id) in [
        ("world", std::any::TypeId::of::<crate::retail_render::RetailWorldMaterial>()),
        ("props", std::any::TypeId::of::<crate::retail_render::PropMaterial>()),
        ("standard", std::any::TypeId::of::<StandardMaterial>()),
    ] {
        if let Some(a) = allocators.get(&id) {
            material_census.push_str(&format!(" slabs_{name}={}/{}", a.slab_count(), a.allocations()));
        }
    }
    let rest: (usize, u64) = allocators.values().map(|a| (a.slab_count(), a.allocations())).fold((0, 0), |s, a| (s.0 + a.0, s.1 + a.1));
    material_census.push_str(&format!(" slabs_all={}/{}", rest.0, rest.1));
    let mut orphans = BTreeMap::<String, usize>::new();
    // Live images of 1M texels or more, to diff between visits to the same map.
    let mut big = BTreeMap::<String, usize>::new();
    let mut total = 0;
    let mut all = BTreeMap::<String, usize>::new();
    for (id, image) in gpu.iter() {
        total += 1;
        let s = image.size;
        let key = format!("{}x{}x{} {:?} m{}", s.width, s.height, s.depth_or_array_layers, image.texture_format, image.mip_level_count);
        *all.entry(key.clone()).or_default() += 1;
        if u64::from(s.width) * u64::from(s.height) * u64::from(s.depth_or_array_layers) >= 1 << 20 {
            *big.entry(key.clone()).or_default() += 1;
        }
        if !images.contains(id) {
            *orphans.entry(key).or_default() += 1;
        }
    }
    material_census.push_str(&format!(" big={big:?}"));
    let mut top: Vec<_> = orphans.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let count: usize = top.iter().map(|(_, n)| n).sum();
    top.truncate(8);
    *GPU_IMAGES.lock().unwrap() = format!("gpu_images={total} orphans={count} {top:?} {material_census}");
    *GPU_IMAGE_KEYS.lock().unwrap() = all;
}
/// SKATE_PERF_SWITCH_EVERY=<s> with SKATE_PERF_SWITCH_MAP: alternate between the
/// start map and that one, <s> seconds after each load, to expose per-switch
/// leaks in process memory. Each switch saves the default map, like the menu.
fn cycle_maps(
    time: Res<Time<Real>>,
    config: Res<crate::config::Config>,
    mut transition: ResMut<crate::map_transition::MapTransition>,
    mut state: Local<(f32, u32)>,
    // A switch updates Config's map, so remember where the run started.
    mut start: Local<Option<Option<std::path::PathBuf>>>,
    counts: (
        Res<Assets<Mesh>>,
        Res<Assets<Image>>,
        Res<Assets<StandardMaterial>>,
        Res<Assets<crate::retail_render::PropMaterial>>,
        Res<Assets<crate::retail_render::RetailWorldMaterial>>,
        Query<Entity>,
    ),
    device: Option<Res<bevy::render::renderer::RenderDevice>>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    mut previous_keys: Local<BTreeMap<String, usize>>,
    // Images rewritten in place (same handle, new GPU texture) since the last switch: "WxH fmt" -> count.
    (mut image_events, mut modified): (MessageReader<AssetEvent<Image>>, Local<BTreeMap<String, usize>>),
) {
    for event in image_events.read() {
        if let AssetEvent::Modified { id } = event
            && let Some(image) = counts.1.get(*id)
        {
            let size = image.texture_descriptor.size;
            *modified.entry(format!("{}x{}x{} {:?}", size.width, size.height, size.depth_or_array_layers, image.texture_descriptor.format)).or_default() += 1;
        }
    }
    let every: f32 = std::env::var("SKATE_PERF_SWITCH_EVERY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.);
    let start = start.get_or_insert_with(|| config.map_path.clone()).clone();
    let now = time.elapsed_secs();
    if let Some(device) = &device
        && (now * 2.) as u32 != ((now - time.delta_secs()) * 2.) as u32
    {
        let h = device.wgpu_device().get_internal_counters().hal;
        eprintln!(
            "SKATE_PERF_GPU_TICK at_s={now:.1} busy={} textures={} views={} bind_groups={} buffers={} encoders={} samplers={} images={}",
            transition.busy(),
            h.textures.read(),
            h.texture_views.read(),
            h.bind_groups.read(),
            h.buffers.read(),
            h.command_encoders.read(),
            h.samplers.read(),
            counts.1.len()
        );
    }
    if transition.busy() || state.0 == 0. {
        state.0 = now + every;
    }
    if now < state.0 || transition.busy() {
        return;
    }
    state.1 += 1;
    let path = if state.1 % 2 == 1 {
        std::env::var_os("SKATE_PERF_SWITCH_MAP").map(std::path::PathBuf::from)
    } else {
        start
    };
    let label = path
        .as_deref()
        .and_then(|p| p.file_stem())
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let (meshes, images, standard, props, retail, entities) = counts;
    eprintln!(
        "SKATE_PERF_CYCLE switch={} to={label} at_s={now:.1} meshes={} images={} standard={} props={} retail={} entities={}",
        state.1,
        meshes.len(),
        images.len(),
        standard.len(),
        props.len(),
        retail.len(),
        entities.iter().count()
    );
    use bevy::render::diagnostic::MeshAllocatorDiagnosticPlugin as Slabs;
    let slab = |path| diagnostics.get(path).and_then(|d| d.value()).unwrap_or(0.);
    eprintln!(
        "SKATE_PERF_SLABS switch={} slabs={} slab_mb={:.0} meshes={}",
        state.1,
        slab(Slabs::slabs_diagnostic_path()),
        slab(Slabs::slabs_size_diagnostic_path()) / 1048576.,
        slab(Slabs::allocations_diagnostic_path())
    );
    // wgpu's live objects and bytes; all zero unless built with `--features wgpu/counters`.
    if let Some(device) = device {
        let gpu = device.wgpu_device().get_internal_counters().hal;
        eprintln!(
            "SKATE_PERF_GPU switch={} buffers={} textures={} views={} bind_groups={} buffer_mb={} texture_mb={} chunks={}",
            state.1,
            gpu.buffers.read(),
            gpu.textures.read(),
            gpu.texture_views.read(),
            gpu.bind_groups.read(),
            gpu.buffer_memory.read() >> 20,
            gpu.texture_memory.read() >> 20,
            gpu.memory_allocations.read()
        );
    }
    eprintln!("SKATE_PERF_IMAGES switch={} {}", state.1, GPU_IMAGES.lock().unwrap());
    eprintln!("SKATE_PERF_MESH_SLABS switch={} {}", state.1, SLABS.lock().unwrap());
    eprintln!("SKATE_PERF_VIEWS switch={} {}", state.1, VIEWS.lock().unwrap());
    eprintln!("SKATE_PERF_IMAGES_MODIFIED switch={} {:?}", state.1, std::mem::take(&mut *modified));
    // Live GPU images by size/format against the last switch: what each trip adds or drops.
    let keys = GPU_IMAGE_KEYS.lock().unwrap().clone();
    let mut diff = Vec::new();
    for key in keys.keys().chain(previous_keys.keys()).collect::<std::collections::BTreeSet<_>>() {
        let (now, was) = (keys.get(key).copied().unwrap_or(0), previous_keys.get(key).copied().unwrap_or(0));
        if now != was {
            diff.push(format!("{key}:{}", now as i64 - was as i64));
        }
    }
    eprintln!("SKATE_PERF_IMAGE_DIFF switch={} {diff:?}", state.1);
    *previous_keys = keys;
    transition.request(crate::map_library::Entry { label, path });
    state.0 = now + every;
}
/// SKATE_PERF_MODES=<s>: play one online challenge row per mode on this map (as SKATE_MODE starts one) for <s>
/// seconds, then free skate <s> seconds; logs "SKATE_PERF_MODE" with the free-skate frame time and the census after
/// each, so a mode that leaves entities, assets or open screens behind shows as growth. Pair with SKATE_PERF_SECONDS.
#[allow(clippy::type_complexity)]
fn cycle_modes(
    time: Res<Time<Real>>,
    mut m: ResMut<crate::match_core::Match>,
    (owned, playable): (Res<crate::match_core::Owned>, Res<crate::match_core::Playable>),
    (challenge, menu, config): (Option<Res<crate::challenge_map::ChallengeMap>>, Res<crate::graphics_menu::Menu>, Res<crate::config::Config>),
    screens: Option<Res<crate::apt_screen::Screens>>,
    counts: (Res<Assets<Mesh>>, Res<Assets<Image>>, Res<Assets<StandardMaterial>>, Res<Assets<crate::scoring_hud::HudMaterial>>, Query<Entity>),
    // Rows to play, the next, when the current step ends, playing, free-skate (frames, seconds).
    mut state: Local<Option<(Vec<(crate::match_core::Mode, String)>, usize, f32, bool, (u32, f32))>>,
) {
    let every: f32 = std::env::var("SKATE_PERF_MODES").ok().and_then(|v| v.parse().ok()).unwrap_or(20.);
    let now = time.elapsed_secs();
    let Some(challenge) = challenge else { return };
    let (rows, next, due, playing, free) = state.get_or_insert_with(|| {
        let map = config.map_path.as_deref().and_then(|p| p.file_stem()).map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let mut rows: Vec<(crate::match_core::Mode, String)> = Vec::new();
        let online = challenge.online_map(&menu.map_labels()).map(|o| o.tabs).unwrap_or_default();
        for (tab, t) in online.iter().enumerate() {
            for folder in &t.rows {
                let Some(mode) = crate::match_core::Mode::from_filter(&folder.name) else { continue };
                let row = folder.rows.iter().filter_map(|e| e.challenge.as_ref()).find(|c| {
                    crate::map_library::same_name(&c.map, &map) && playable.plays(tab, &c.id)
                });
                if let (Some(c), false) = (row, rows.iter().any(|(m, _)| *m == mode)) {
                    rows.push((mode, c.id.clone()));
                }
            }
        }
        eprintln!("SKATE_PERF_MODES map={map} rows={rows:?}");
        (rows, 0, now + every, false, (0, 0.))
    });
    if !*playing {
        free.0 += 1;
        free.1 += time.delta_secs();
    }
    if now < *due {
        return;
    }
    if *playing {
        crate::match_core::drop_match(&mut m);
        *playing = false;
        *free = (0, 0.);
    } else {
        let (meshes, images, standard, hud, entities) = &counts;
        let open = screens.map(|s| s.census()).unwrap_or_default();
        eprintln!(
            "SKATE_PERF_MODE after={} at_s={now:.1} free_ms={:.2} entities={} meshes={} images={} standard={} hud={} screens={open:?}",
            next.checked_sub(1).and_then(|i| rows.get(i)).map_or("none", |(_, r)| r.as_str()),
            free.1 * 1000. / free.0.max(1) as f32,
            entities.iter().count(),
            meshes.len(),
            images.len(),
            standard.len(),
            hud.len(),
        );
        if let Some((mode, row)) = rows.get(*next % rows.len().max(1)).cloned() {
            m.start(mode, &row, now);
            m.owned = owned.0.contains(&row);
            *playing = true;
        }
        *next += 1;
    }
    *due = now + every;
}
/// SKATE_PERF_WARMUP: seconds left out at the start (default 10: initialization and shader warmup); 0 keeps a first
/// load's own frames (bug 10-25 #6: the first seconds on a map).
fn warmup() -> f64 {
    static WARMUP: OnceLock<f64> = OnceLock::new();
    *WARMUP.get_or_init(|| std::env::var("SKATE_PERF_WARMUP").ok().and_then(|v| v.parse().ok()).unwrap_or(10.))
}
/// SKATE_PERF_SECONDS: measurement length before the report and exit (default 25).
fn report_after() -> f64 {
    std::env::var("SKATE_PERF_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25.)
}
fn begin(mut p: ResMut<Performance>) {
    p.frame_start = Instant::now();
    p.physics_ms = 0.;
    p.ticks = 0;
}
fn finish(
    mut p: ResMut<Performance>,
    mut exit: MessageWriter<AppExit>,
    (images, meshes, server): (Res<Assets<Image>>, Res<Assets<Mesh>>, Res<AssetServer>),
) {
    let now = Instant::now();
    let elapsed = now
        .duration_since(*p.start.get_or_insert(now))
        .as_secs_f64();
    let frame = now.duration_since(p.previous).as_secs_f64() * 1000.;
    p.previous = now;
    // Exclude initialization and shader warmup. Frame interval includes render
    // synchronization; CPU schedule time is First..Last of the main world.
    if elapsed > warmup() {
        let sample = [
            frame,
            now.duration_since(p.frame_start).as_secs_f64() * 1000.,
            p.physics_ms,
            f64::from(p.ticks),
        ];
        p.samples.push(sample);
        p.sample_times.push(timestamp(now));
    }
    if elapsed > report_after() && !p.samples.is_empty() {
        let mean = |column: usize| {
            p.samples.iter().map(|s| s[column]).sum::<f64>() / p.samples.len() as f64
        };
        let mut frames: Vec<_> = p.samples.iter().map(|s| s[0]).collect();
        frames.sort_by(f64::total_cmp);
        let render = p.render.lock().unwrap();
        let render_mean = |column: usize| {
            render.iter().map(|s| s[column]).sum::<f64>() / render.len().max(1) as f64
        };
        let report = serde_json::json!({
            "frames": frames.len(), "fps": 1000. / mean(0),
            "frame_ms_mean": mean(0), "frame_ms_median": frames[frames.len()/2],
            "frame_ms_p95": frames[(frames.len()-1)*95/100],
            "main_schedule_ms_mean": mean(1), "physics_ms_per_frame": mean(2),
            "physics_ticks_per_frame": mean(3),
            "physics_ms_per_tick": mean(2) / mean(3).max(f64::EPSILON),
            "render_prepare_ms_mean": render_mean(0), "render_submit_ms_mean": render_mean(1),
            "render_assets_ms_mean": render_mean(2), "render_views_ms_mean": render_mean(3), "render_queue_ms_mean": render_mean(4),
            "sections_ms_per_call": SECTIONS.lock().unwrap().iter().map(|(&k, &(ms, n))| (k, ms / n as f64)).collect::<BTreeMap<_,_>>(),
            "samples": p.samples,
            "sample_elapsed_ms": p.sample_times,
            "render_samples": &*render,
            "render_sample_columns": ["prepare_ms", "submit_ms", "assets_ms", "views_ms", "queue_ms", "elapsed_ms", "waiting_pipelines"],
            "slow_sections": &*SLOW_SECTIONS.lock().unwrap(),
            "slow_section_columns": ["section", "start_elapsed_ms", "duration_ms"],
            "frame_ms_max": frames[frames.len()-1],
            "frame_ms_p99": frames[(frames.len()-1)*99/100],
            "frames_over_8ms": frames.iter().filter(|&&ms| ms > 8.).count(),
            "memory_samples": &*MEMORY.lock().unwrap(),
            "memory_sample_columns": ["elapsed_ms", "private_mb", "working_set_mb", "heap_live_mb", "heap_committed_mb", "main_world_image_mb", "main_world_mesh_mb"],
        });
        log_cpu_assets(&images, &meshes, &server);
        match std::fs::write(&p.path, serde_json::to_vec_pretty(&report).unwrap()) {
            Ok(()) => {
                eprintln!("SKATE_PERF_REPORT {}", p.path.display());
                exit.write(AppExit::Success);
            }
            Err(e) => {
                eprintln!("Performance report: {e}");
                exit.write(AppExit::error());
            }
        }
    }
}

#[derive(Resource)]
struct RenderPerformance {
    start: Option<Instant>,
    frame: Instant,
    prepared: Instant,
    mark: Instant,
    phases: [f64; 3],
    samples: Arc<Mutex<Vec<[f64; 7]>>>,
}
/// Every 2 s: each 3D view's transparent phase size (its encoding dominates NSV's render submit).
fn phase_counts(
    transparent: Res<bevy::render::render_phase::ViewSortedRenderPhases<bevy::core_pipeline::core_3d::Transparent3d>>,
    opaque: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::core_3d::Opaque3d>>,
    prepass: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::prepass::Opaque3dPrepass>>,
    mut last: Local<Option<Instant>>,
) {
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 2.) {
        return;
    }
    *last = Some(Instant::now());
    // Binned passes: a multidraw set is one draw call; batchable and unbatchable bins draw one by one.
    macro_rules! binned {
        ($name:literal, $phases:expr) => {
            for phase in $phases.values() {
                let keys = || phase.multidrawable_meshes.keys();
                eprintln!(
                    "SKATE_PERF_3D {} multidraw_sets={} pipelines={} bind_groups={} vertex_slabs={} index_slabs={} multidraw_bins={} batchable_bins={} unbatchable={}",
                    $name,
                    phase.multidrawable_meshes.len(),
                    keys().map(|k| k.pipeline).collect::<std::collections::HashSet<_>>().len(),
                    keys().map(|k| k.material_bind_group_index).collect::<std::collections::HashSet<_>>().len(),
                    keys().map(|k| k.vertex_slab).collect::<std::collections::HashSet<_>>().len(),
                    keys().map(|k| k.index_slab).collect::<std::collections::HashSet<_>>().len(),
                    phase.multidrawable_meshes.values().map(|b| b.len()).sum::<usize>(),
                    phase.batchable_meshes.len(),
                    phase.unbatchable_meshes.values().map(|u| u.entities.len()).sum::<usize>(),
                );
            }
        };
    }
    binned!("opaque", opaque.0);
    binned!("prepass", prepass.0);
    for (view, phase) in transparent.iter() {
        let draws = phase.items.iter().filter(|i| !i.batch_range.is_empty()).count();
        let pipelines: std::collections::HashSet<_> = phase.items.iter().map(|i| i.pipeline).collect();
        let switches = phase.items.windows(2).filter(|w| w[0].pipeline != w[1].pipeline).count();
        eprintln!(
            "SKATE_PERF_3D transparent view={view:?} items={} draws={draws} pipelines={} pipeline_switches={switches}",
            phase.items.len(),
            pipelines.len()
        );
    }
}
fn render_begin(mut p: ResMut<RenderPerformance>) {
    p.frame = Instant::now();
    p.mark = p.frame;
}
fn render_assets_done(mut p: ResMut<RenderPerformance>) {
    let now = Instant::now();
    p.phases[0] = now.duration_since(p.mark).as_secs_f64() * 1000.;
    p.mark = now;
}
fn render_views_done(mut p: ResMut<RenderPerformance>) {
    let now = Instant::now();
    p.phases[1] = now.duration_since(p.mark).as_secs_f64() * 1000.;
    p.mark = now;
}
fn render_queue_done(mut p: ResMut<RenderPerformance>) {
    p.phases[2] = p.mark.elapsed().as_secs_f64() * 1000.;
}
fn render_prepared(mut p: ResMut<RenderPerformance>) {
    p.prepared = Instant::now();
}
fn render_finish(
    mut p: ResMut<RenderPerformance>,
    pipelines: Res<bevy::render::render_resource::PipelineCache>,
) {
    let now = Instant::now();
    if now
        .duration_since(*p.start.get_or_insert(now))
        .as_secs_f64()
        > warmup()
    {
        p.samples.lock().unwrap().push([
            p.prepared.duration_since(p.frame).as_secs_f64() * 1000.,
            now.duration_since(p.prepared).as_secs_f64() * 1000.,
            p.phases[0],
            p.phases[1],
            p.phases[2],
            timestamp(now),
            pipelines.waiting_pipelines().count() as f64,
        ]);
    }
}
