//! Edit Skaters' 2D photos: each member's card portrait (retail's photo object 0x82640eb0 rendering
//! `FrontEndState_TeamManagement_Profile_%d` with vault photo setup `team_profile_0k`) and Import Skater's preview
//! (the same `team_profile_0k` setup, k the member imported into: 0x82653858). Drawn by the engine like the Custom
//! Graphic tiles (graphic_tile.rs): the look on its own hidden skater on a private render layer, its own camera,
//! captured until two captures agree, then laid over the setup's background art and colour and written as raw RGBA
//! for the APT canvas.
//!
//! Retail (TU3; research in .superpowers/sdd/2026-10-05-team-photos): the photographer 0x8278f4c0 poses the CAS
//! skater in the setup's cac_body clip advanced by 100000 s (0x8278f528 -> 0x82532098: the clip's held end), lights
//! it with the setup's render location (0x8278fb90: cas_photo, its sun given in the camera's right/up/forward basis,
//! the cas SH) and shoots a full-body 128 x 256 portrait (0x8278e960) with a 45 degree vertical lens, near 0.01, far
//! 200 (0x8278f788). The card keeps that picture (the member's profile +284) until the look changes; we keep it in a
//! cache file named by the look, setup and this renderer's version, so a known look is never drawn twice.
//!
//! The frame the setup's camera is given in: 0x8278f788 builds view = inverse(W) x R(90 deg) x lookAt(camera,
//! target, up (0,1,0)), W the CAS character's vt[10] (vtable 0x82310d88 +40 = 0x82dfd800: its world matrix, +352),
//! which its animation sets through vt[11] (0x827a68b0, from 0x825325d8: a placement and a yaw). Where the CAS
//! stage holds that placement is run-time state, not data. Ruling: origin at the skater's HIPS joint, X right, Y
//! towards the camera, Z up: the setups' target lies 0.16 m off the origin, and retail's own portraits
//! (fedynamic.big fe/source/images/demo/demoskater01-05, drawn with these setups' backgrounds) centre the picture at
//! hip height. The picture holds the skater over the setup's colour and art only: no logo or name (no such setup
//! field; the cards draw both as their own APT elements).
//! Without the editor bank or the stock rig the skater stands in its bind pose with the arms lowered (the tiles').
use crate::{customiser_material::SkaterMaterial, customiser_parts::Parts, team_select::PhotoSetup};
use bevy::{
    camera::{ClearColorConfig, RenderTarget, visibility::{NoFrustumCulling, RenderLayers}},
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    mesh::morph::MorphWeights,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    scene::{SceneInstance, SceneSpawner},
};
use serde_json::Value;
use std::{collections::{HashMap, VecDeque}, path::PathBuf};

/// The photo skater's, camera's and lights' private layer.
pub(crate) const PHOTO_LAYER: usize = 49;
/// Drawn 2x2 and averaged down.
const SUPER: u32 = 2;
const TIMEOUT: f64 = 30.;
const SETTLE: u32 = 4;
const TRIES: u32 = 12;
/// Arms lowered as in the tiles (s1render's ARM_DROP).
const ARM_DROP: f32 = 38.;
/// The world's character exposure (retail_character.rs), the light's w. cas's exposure record exp_festart is a meter
/// (auto_exposure target 0.25, 0.75..1.5, damping 0.5; x2.5 here): its value on a photo is the front end's meter state
/// when it is drawn, not data; the world's 2.5 (exposure 1.0) lies inside it. Its bloom_femain (threshold 1.0) is not
/// drawn.
const EXPOSURE: f32 = 2.5;
/// Part of every cache file's name: bump it when the drawing changes, so old pictures are not reused. 3: no capture
/// while a pipeline compiles ([`ready`]), and the sock / grip / alpha fixes of the CAS waves (pictures drawn before
/// them showed socks under long pants and pieces missing, cached under the look for good).
const VERSION: u32 = 3;
/// Frames of Shooting a capture waits for compiling pipelines at most (one that never finishes, e.g. a shader that
/// never loads, must not keep every photo from being drawn: past this the capture goes ahead as before).
pub(crate) const PIPELINE_WAIT: u32 = 600;

/// A photo to draw.
#[derive(Clone, Debug)]
pub(crate) struct PhotoJob {
    /// Asset name the screen loads it by.
    pub key: String,
    /// The look, resolved (Parts::resolve).
    pub profile: Value,
    pub setup: PhotoSetup,
    /// The background art: its raw RGBA file and size.
    pub background: Option<(PathBuf, [u32; 2])>,
    /// Where the picture goes ([`cache_file`]): if it is there already, it is not drawn again.
    pub file: PathBuf,
}

/// The cache file of a look in a setup: `dir/<hash>.rgba`, the hash over everything that changes the picture: this
/// renderer's version, the look as saved (resolving it is a function of the library, which `inputs` covers), the
/// setup, the background, and `inputs`, the fingerprints of the clothing library (Parts::fingerprint) and of the
/// boot data's art (TeamFile::fingerprint). Re-running a stage or a clothing script changes them and redraws.
pub(crate) fn cache_file(dir: &std::path::Path, look: &Value, setup: &PhotoSetup, background: Option<&(PathBuf, [u32; 2])>, inputs: &str) -> PathBuf {
    let mut hash = blake3::Hasher::new();
    hash.update(&VERSION.to_le_bytes());
    hash.update(&serde_json::to_vec(look).unwrap_or_default());
    hash.update(format!("{setup:?}{background:?}{inputs}").as_bytes());
    dir.join(format!("{}.rgba", &hash.finalize().to_hex()[..32]))
}
/// A finished photo: the RGBA file written (None: it could not be drawn).
#[derive(Clone, Debug)]
pub(crate) struct PhotoDone {
    pub key: String,
    pub file: Option<(PathBuf, [u32; 2])>,
}

#[derive(PartialEq)]
enum Stage {
    Loading,
    Posing,
    Framing,
    Shooting,
}
struct Active {
    id: u64,
    job: PhotoJob,
    started: f64,
    stage: Stage,
    frames: u32,
    root: Entity,
    scenes: Vec<(Entity, String, String)>,
    target: Handle<Image>,
    camera: Option<Entity>,
    lights: Vec<Entity>,
    asked: bool,
    previous: Option<image::RgbaImage>,
    attempts: u32,
    /// The pieces' materials (their sun is set once the camera is placed).
    materials: Vec<Handle<SkaterMaterial>>,
}
#[derive(Resource, Default)]
pub(crate) struct Photos {
    queue: VecDeque<PhotoJob>,
    active: Option<Active>,
    next_id: u64,
    captured: Option<(u64, Image)>,
    /// The editor bank per gender (None: it would not load).
    posers: HashMap<String, Option<crate::cas_pose::Poser>>,
    /// The stock rig's bones when there is no gameplay skater (OffBoard.abin; None: it would not load).
    bones: Option<Option<Vec<String>>>,
    pub finished: Vec<PhotoDone>,
}
impl Photos {
    /// Queue a photo; an older request for the same key is replaced. One already in its cache file is done at once.
    pub(crate) fn request(&mut self, job: PhotoJob) {
        self.queue.retain(|j| j.key != job.key);
        if cached(&job.file, &job.setup) {
            let size = [job.setup.width.max(1), job.setup.height.max(1)];
            self.finished.push(PhotoDone { key: job.key, file: Some((job.file, size)) });
            return;
        }
        self.queue.push_back(job);
    }
    /// Nothing queued or being drawn.
    pub(crate) fn idle(&self) -> bool {
        self.queue.is_empty() && self.active.is_none()
    }
}
impl Active {
    fn finish(self, commands: &mut Commands) {
        commands.entity(self.root).despawn();
        for entity in self.camera.into_iter().chain(self.lights) {
            commands.entity(entity).despawn();
        }
    }
}

/// Render pipelines still compiling after the last render frame (PipelineCache::waiting_pipelines, shared with the
/// render world). Bevy compiles them in the background and skips every mesh whose pipeline is not ready, so a capture
/// taken meanwhile lacks those pieces; two such captures agree and the picture is cached under the look for good.
/// The photo camera's own settings (no MSAA, no tonemapping) make pipelines of their own, so its first frames always
/// wait for some; while the world loads, those queue behind the world's.
/// Custom Graphic tiles (graphic_tile.rs) wait on it too.
#[derive(Resource, Clone, Default)]
pub(crate) struct PipelinesWaiting(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl PipelinesWaiting {
    pub(crate) fn count(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}
fn count_waiting(cache: Res<bevy::render::render_resource::PipelineCache>, waiting: Res<PipelinesWaiting>) {
    waiting.0.store(cache.waiting_pipelines().count(), std::sync::atomic::Ordering::Relaxed);
}
/// Shares [`PipelinesWaiting`] with the render world, once however many plugins ask.
pub(crate) fn track_pipelines(app: &mut App) {
    if app.world().contains_resource::<PipelinesWaiting>() {
        return;
    }
    let waiting = PipelinesWaiting::default();
    app.insert_resource(waiting.clone());
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render.insert_resource(waiting).add_systems(bevy::render::Render, count_waiting.after(bevy::render::RenderSystems::Render));
    }
}

/// No pipeline is still compiling, or one has kept compiling for PIPELINE_WAIT frames.
pub(crate) fn compiled(frames: u32, waiting: usize) -> bool {
    waiting == 0 || frames >= PIPELINE_WAIT
}

/// Whether to ask for a capture: the picture has had SETTLE frames and its pipelines are [`compiled`].
fn ready(frames: u32, asked: bool, waiting: usize) -> bool {
    !asked && frames >= SETTLE && compiled(frames, waiting)
}

pub(crate) struct PhotoPlugin;
impl Plugin for PhotoPlugin {
    fn build(&self, app: &mut App) {
        track_pipelines(app);
        app.init_resource::<Photos>().add_systems(Update, (begin, load, shoot).chain());
    }
}

/// Import Skater's previews' cache (the cards' is in the settings folder): one folder in the temp directory.
pub(crate) fn preview_folder() -> PathBuf {
    std::env::temp_dir().join("skate3-team-photos")
}

/// The setup's picture is in `file` whole (a torn or missing one is drawn again).
pub(crate) fn cached(file: &std::path::Path, setup: &PhotoSetup) -> bool {
    std::fs::metadata(file).is_ok_and(|m| m.len() == u64::from(setup.width.max(1) * setup.height.max(1) * 4))
}

/// A cache folder's housekeeping (off the main thread): a failed write's `.part` older than a minute (never one
/// being written: a write takes milliseconds), and, given `unused`, any picture written longer ago than that.
pub(crate) fn sweep(dir: &std::path::Path, unused: Option<std::time::Duration>) {
    let now = std::time::SystemTime::now();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let limit = if path.extension().is_some_and(|e| e == "part") { Some(std::time::Duration::from_secs(60)) } else { unused };
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok());
        if age.zip(limit).is_some_and(|(age, limit)| age > limit) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The setup's camera and target in skater space, from the hips joint at `hips` (see the module ruling).
pub(crate) fn placement(setup: &PhotoSetup, hips: Vec3) -> (Vec3, Vec3) {
    let at = |p: [f32; 3]| hips + Vec3::new(p[0], p[2], p[1]);
    (at(setup.camera), at(setup.target))
}

/// The render location's sun (given in the camera's right, up, forward basis, 0x8296efe0) in the world.
pub(crate) fn sun(camera: Quat, sun: [f32; 3]) -> Vec3 {
    (camera * Vec3::X * sun[0] + camera * Vec3::Y * sun[1] + camera * Vec3::NEG_Z * sun[2]).normalize_or(Vec3::Y)
}

/// The capture over the setup's colour and background art (all straight alpha), as raw RGBA.
pub(crate) fn composite(photo: &image::RgbaImage, colour: [f32; 4], background: Option<&image::RgbaImage>) -> image::RgbaImage {
    let [_, r, g, b] = colour.map(|c| (c.clamp(0., 1.) * 255.).round() as u8);
    let mut out = image::RgbaImage::from_pixel(photo.width(), photo.height(), image::Rgba([r, g, b, 255]));
    let over = |dst: &mut image::Rgba<u8>, src: image::Rgba<u8>| {
        let a = f32::from(src.0[3]) / 255.;
        for c in 0..3 {
            dst.0[c] = (f32::from(src.0[c]) * a + f32::from(dst.0[c]) * (1. - a)).round() as u8;
        }
    };
    if let Some(background) = background.filter(|b| b.dimensions() == photo.dimensions()) {
        for (dst, src) in out.pixels_mut().zip(background.pixels()) {
            over(dst, *src);
        }
    }
    for (dst, src) in out.pixels_mut().zip(photo.pixels()) {
        over(dst, *src);
    }
    out
}

fn begin(mut commands: Commands, mut photos: ResMut<Photos>, parts: Res<Parts>, server: Res<AssetServer>, time: Res<Time<Real>>) {
    let (photos, now) = (&mut *photos, time.elapsed_secs_f64());
    if let Some(started) = photos.active.as_ref().map(|a| a.started) {
        if now - started > TIMEOUT {
            if let Some(active) = photos.active.take() {
                warn!("Team photo {} not drawn: gave up after {TIMEOUT} s", active.job.key);
                photos.finished.push(PhotoDone { key: active.job.key.clone(), file: None });
                active.finish(&mut commands);
            }
        }
        return;
    }
    let Some(job) = photos.queue.pop_front() else { return };
    let root = commands.spawn((Name::new("Team photo skater"), Transform::default(), Visibility::Hidden, RenderLayers::layer(PHOTO_LAYER))).id();
    let mut scenes = Vec::new();
    for value in job.profile["selections"].as_object().into_iter().flat_map(|s| s.values()) {
        let (Some(id), Some(mid)) = (value["asset_id"].as_str(), value["material_id"].as_str()) else { continue };
        let Some(part) = parts.library.models.get(id) else { continue };
        // perf 10-19: through Part::load (SKATE_PARTS_GPU_ONLY), like every loader of a part.
        let scene = commands.spawn((SceneRoot(part.load(&server, Some(GltfAssetLabel::Scene(0)))), ChildOf(root))).id();
        scenes.push((scene, id.to_owned(), mid.to_owned()));
    }
    photos.next_id += 1;
    photos.active = Some(Active {
        id: photos.next_id,
        job,
        started: now,
        stage: Stage::Loading,
        frames: 0,
        root,
        scenes,
        target: Handle::default(),
        camera: None,
        lights: Vec::new(),
        asked: false,
        previous: None,
        attempts: 0,
        materials: Vec::new(),
    });
}

#[allow(clippy::too_many_arguments)]
fn load(
    mut commands: Commands,
    mut photos: ResMut<Photos>,
    mut parts: ResMut<Parts>,
    server: Res<AssetServer>,
    spawner: Res<SceneSpawner>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<SkaterMaterial>>,
    instances: Query<&SceneInstance>,
    meshes: Query<Entity, With<Mesh3d>>,
    parents: Query<&ChildOf>,
    mut morphs: Query<(Entity, &mut MorphWeights)>,
    names: Query<&bevy::gltf::GltfMaterialName>,
) {
    let Some(active) = photos.active.as_mut().filter(|a| a.stage == Stage::Loading) else { return };
    if active.scenes.iter().any(|(scene, ..)| !instances.get(*scene).is_ok_and(|i| spawner.instance_is_ready(**i))) {
        return;
    }
    for (_, _, mid) in &active.scenes {
        parts.warm(mid, &server, &mut materials);
    }
    let profile = &active.job.profile;
    let ready = parts.graphics_ready(profile, &server, &mut images, crate::custom_graphic::REMOTE_SIZE)
        & parts.tattoos_ready(profile, &server)
        & active.scenes.iter().all(|(_, _, mid)| parts.material_ready(mid, &server));
    if !ready {
        return;
    }
    // The render location's SH and sun (its direction once the camera is placed); older boot data has neither, and
    // the pieces fall back to the tiles' lights. key.w < 0 keeps the world's lighting (retail_character.rs) off them.
    let sh = active.job.setup.sh.as_ref().filter(|sh| sh.len() == 9 && active.job.setup.sun.is_some());
    for (scene, id, mid) in &active.scenes {
        let Some(mut material) = parts.profile_material(id, mid, profile, &materials) else { continue };
        let retail = &mut material.extension.retail;
        match sh {
            Some(sh) if retail.tint.w > 0. => {
                retail.sh = std::array::from_fn(|i| Vec3::from_array(sh[i]).extend(0.));
                retail.key = Vec4::new(1., 1., 1., -1.);
                retail.light = Vec4::new(0., 1., 0., EXPOSURE);
            }
            _ => (retail.light, retail.tint) = (Vec4::ZERO, Vec4::ZERO),
        }
        // The deck's grip (customiser_parts::GRIP) wears the profile's Grip Color.
        let grip = parts.grip_material(id, mid, &material, profile).map(|m| materials.add(m));
        let handle = materials.add(material);
        active.materials.push(handle.clone());
        active.materials.extend(grip.clone());
        for mesh in &meshes {
            if parents.iter_ancestors(mesh).any(|p| p == *scene) {
                let grip = grip.as_ref().filter(|_| names.get(mesh).is_ok_and(|n| n.0 == crate::customiser_parts::GRIP));
                let handle = grip.unwrap_or(&handle);
                commands
                    .entity(mesh)
                    .remove::<MeshMaterial3d<StandardMaterial>>()
                    .insert((MeshMaterial3d(handle.clone()), RenderLayers::layer(PHOTO_LAYER), NoFrustumCulling));
            }
        }
    }
    let weights: Vec<f32> = parts
        .library
        .morphs
        .iter()
        .enumerate()
        .map(|(i, name)| profile["morphs"][name].as_f64().unwrap_or(if (2..19).contains(&i) { 0.25 } else { 0. }).clamp(0., 0.5) as f32)
        .collect();
    for (entity, mut morph) in &mut morphs {
        if morph.weights().len() == weights.len() && parents.iter_ancestors(entity).any(|p| p == active.root) {
            morph.weights_mut().copy_from_slice(&weights);
        }
    }
    active.stage = Stage::Posing;
    active.frames = 0;
}

#[allow(clippy::too_many_arguments)]
fn shoot(
    mut commands: Commands,
    mut photos: ResMut<Photos>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<SkaterMaterial>>,
    mut joints: ParamSet<(Query<(&Name, &Transform)>, Query<(Entity, &Name, &mut Transform)>)>,
    skins: Query<(Entity, &bevy::mesh::skinning::SkinnedMesh)>,
    globals: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    (config, skater, waiting): (Res<crate::config::Config>, Option<Res<crate::physics::SkaterRuntime>>, Res<PipelinesWaiting>),
) {
    let photos = &mut *photos;
    let Some(active) = photos.active.as_mut() else { return };
    match active.stage {
        Stage::Loading => {}
        Stage::Posing => {
            active.frames += 1;
            if active.frames < 2 {
                return;
            }
            // The setup's pose on the stock rig (bone names: the skater's, else OffBoard.abin's), held at its end as
            // retail's 100000 s step leaves it; else the arms lowered.
            let bones = match skater.as_ref() {
                Some(skater) => Some(skater.animation.evaluator.frames.bone_names.clone()),
                None => photos
                    .bones
                    .get_or_insert_with(|| {
                        let path = config.asset_root.join("private/stock/data/anim/OffBoard.abin");
                        skate_data::abin::Bank::load(&path).ok().and_then(|b| b.hierarchy().map(|h| h.bone_names.clone()))
                    })
                    .clone(),
            };
            let posed = active.job.setup.pose.as_deref().zip(bones.as_ref()).and_then(|(clip, bones)| {
                let gender = active.job.profile["gender"].as_str().unwrap_or("male").to_owned();
                let poser = photos.posers.entry(gender.clone()).or_insert_with(|| {
                    crate::cas_pose::Poser::editor(&config.asset_root, &gender, bones).map_err(|e| warn!("Team photo poses: {e}")).ok()
                });
                let poser = poser.as_ref()?;
                let clip = clip.to_uppercase();
                let pose = poser.pose(&clip, poser.length(&clip).unwrap_or(0.), false, &[])?;
                let rig = crate::animation::AnimationStatus::for_scene(active.root, &crate::animation::with_face(bones), &skins, &joints.p0(), &parents)
                    .map_err(|e| warn!("Team photo {}: {e}", active.job.key))
                    .ok()?;
                Some(rig.pose_transforms(&pose))
            });
            let mut nodes = joints.p1();
            if let Some(posed) = posed {
                for (entity, transform) in posed {
                    if let Ok((_, _, mut node)) = nodes.get_mut(entity) {
                        *node = transform;
                    }
                }
                commands.entity(active.root).insert(Visibility::Inherited);
                active.stage = Stage::Framing;
                active.frames = 0;
                return;
            }
            for (entity, name, mut transform) in &mut nodes {
                let degrees = if name.eq_ignore_ascii_case("LEFTARM") {
                    -ARM_DROP
                } else if name.eq_ignore_ascii_case("RIGHTARM") {
                    ARM_DROP
                } else {
                    continue;
                };
                if !parents.iter_ancestors(entity).any(|p| p == active.root) {
                    continue;
                }
                let Some(parent) = parents.get(entity).ok().and_then(|p| globals.get(p.parent()).ok()) else { continue };
                transform.rotation = crate::graphic_tile::dropped(parent.to_scale_rotation_translation().1, transform.rotation, degrees);
            }
            commands.entity(active.root).insert(Visibility::Inherited);
            active.stage = Stage::Framing;
            active.frames = 0;
        }
        Stage::Framing => {
            active.frames += 1;
            if active.frames < 2 {
                return;
            }
            // The hips joint of the photo skater (the first piece that has one), else a standing hip height.
            let hips = joints
                .p1()
                .iter()
                .filter(|(_, name, _)| name.eq_ignore_ascii_case("HIPS"))
                .find(|(entity, ..)| parents.iter_ancestors(*entity).any(|p| p == active.root))
                .and_then(|(entity, ..)| globals.get(entity).ok())
                .map_or(Vec3::new(0., 0.95, 0.), |g| g.translation());
            let setup = &active.job.setup;
            let (eye, target) = placement(setup, hips);
            let [w, h] = [setup.width.max(1), setup.height.max(1)];
            let mut image = crate::graphic_tile::target(1);
            image.resize(bevy::render::render_resource::Extent3d { width: w * SUPER, height: h * SUPER, depth_or_array_layers: 1 });
            active.target = images.add(image);
            let rotation = Transform::from_translation(eye).looking_at(target, Vec3::Y).rotation;
            if let Some(light) = setup.sun.map(|s| sun(rotation, s).extend(EXPOSURE)) {
                for handle in &active.materials {
                    if let Some(material) = materials.get_mut(handle).filter(|m| m.extension.retail.light.w > 0.) {
                        material.extension.retail.light = light;
                    }
                }
            }
            let projection = PerspectiveProjection { fov: setup.fov.to_radians(), near: setup.near, far: setup.far, ..default() };
            active.camera = Some(
                commands
                    .spawn((
                        Camera3d::default(),
                        Camera { order: -51, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
                        RenderTarget::Image(active.target.clone().into()),
                        Projection::Perspective(projection),
                        Transform { translation: eye, rotation, scale: Vec3::ONE },
                        RenderLayers::layer(PHOTO_LAYER),
                        Msaa::Off,
                        Tonemapping::None,
                        DebandDither::Disabled,
                        bevy::light::AmbientLight { color: Color::WHITE, brightness: 419., ..default() },
                    ))
                    .id(),
            );
            for (lux, at) in crate::graphic_tile::lights(rotation) {
                active.lights.push(
                    commands.spawn((DirectionalLight { illuminance: lux, shadows_enabled: false, ..default() }, at, RenderLayers::layer(PHOTO_LAYER))).id(),
                );
            }
            active.stage = Stage::Shooting;
            active.frames = 0;
        }
        Stage::Shooting => {
            active.frames += 1;
            if let Some((_, image)) = photos.captured.take_if(|(id, _)| *id == active.id) {
                active.asked = false;
                active.frames = 0;
                active.attempts += 1;
                let shot = crate::graphic_tile::rgba(&image).map(|big| crate::graphic_tile::downsample(&big, SUPER));
                let verdict = shot.as_ref().map(|s| crate::graphic_tile::judge(active.previous.as_ref(), s));
                let file = match verdict {
                    Some(crate::graphic_tile::Verdict::Done) => shot.and_then(|shot| {
                        let background = active.job.background.as_ref().and_then(|(path, [w, h])| {
                            image::RgbaImage::from_raw(*w, *h, std::fs::read(path).ok()?)
                        });
                        let photo = composite(&shot, active.job.setup.colour, background.as_ref());
                        let path = active.job.file.clone();
                        // Written whole, then renamed: a half-written file is never taken for a cached one.
                        let partial = path.with_extension(format!("{}.part", std::process::id()));
                        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&partial, photo.as_raw()));
                        written.and_then(|_| std::fs::rename(&partial, &path)).map_err(|e| warn!("Team photo {}: {e}", active.job.key)).ok()?;
                        Some((path, [photo.width(), photo.height()]))
                    }),
                    Some(_) if active.attempts < TRIES => {
                        active.previous = shot;
                        return;
                    }
                    _ => {
                        // Why: no picture, an empty one (with or without colour under its zero alpha), or how many
                        // pixels its last two shots differ in.
                        let changed = shot.as_ref().zip(active.previous.as_ref()).map(|(a, b)| a.pixels().zip(b.pixels()).filter(|(p, q)| p != q).count());
                        let coloured = crate::graphic_tile::rgba(&image).map(|big| big.pixels().filter(|p| p[0] > 0 || p[1] > 0 || p[2] > 0).count());
                        warn!(
                            "Team photo {} not drawn: nothing steady came back (last {verdict:?}, pixels changed {changed:?}, coloured {coloured:?})",
                            active.job.key
                        );
                        None
                    }
                };
                if let Some(active) = photos.active.take() {
                    photos.finished.push(PhotoDone { key: active.job.key.clone(), file });
                    active.finish(&mut commands);
                }
            } else if ready(active.frames, active.asked, waiting.count()) {
                active.asked = true;
                let id = active.id;
                commands.spawn(Screenshot::image(active.target.clone())).observe(move |captured: On<ScreenshotCaptured>, mut photos: ResMut<Photos>| {
                    photos.captured = Some((id, captured.image.clone()));
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> PhotoSetup {
        PhotoSetup {
            camera: [-0.062, 2.8, 0.2],
            target: [-0.06, 2.1, 0.16],
            colour: [1., 0.2, 0.4, 0.6],
            background: None,
            width: 4,
            height: 2,
            pose: None,
            fov: 45.,
            near: 0.01,
            far: 200.,
            sun: Some([-5., 1.1, 0.]),
            sh: None,
        }
    }

    /// team_profile_00 stands the camera 2.8 m in front of the hips, 20 cm above them, looking a little down.
    #[test]
    fn the_setup_places_the_camera_in_front_of_the_hips() {
        let (eye, target) = placement(&setup(), Vec3::new(0., 0.9, 0.));
        assert!((eye - Vec3::new(-0.062, 1.1, 2.8)).length() < 1e-5, "{eye}");
        assert!((target - Vec3::new(-0.06, 1.06, 2.1)).length() < 1e-5, "{target}");
    }

    /// No capture while a pipeline compiles: its meshes would be missing from it (and from the cached picture).
    #[test]
    fn no_capture_while_pipelines_compile() {
        assert!(!ready(SETTLE - 1, false, 0), "the picture settles first");
        assert!(ready(SETTLE, false, 0));
        assert!(!ready(SETTLE, true, 0), "one capture at a time");
        assert!(!ready(SETTLE, false, 2), "a piece's pipeline is still compiling");
        assert!(ready(PIPELINE_WAIT, false, 2), "one that never finishes does not hold the photo for good");
        assert!(PIPELINE_WAIT as f64 / 60. < TIMEOUT, "it gives up before the photo does");
    }

    /// cas_photo's sun (-5, 1.1, 0) is camera-relative: from the picture's left, a little above, whichever way the
    /// camera looks.
    #[test]
    fn the_sun_comes_from_the_cameras_left() {
        for yaw in [0f32, 1.2, -2.5] {
            let camera = Quat::from_rotation_y(yaw);
            let light = sun(camera, [-5., 1.1, 0.]);
            assert!((light.length() - 1.).abs() < 1e-5);
            assert!(light.dot(camera * Vec3::X) < -0.97 && light.y > 0.2 && light.dot(camera * Vec3::NEG_Z).abs() < 1e-5, "{light}");
        }
    }

    /// The cache name follows the look, the setup and the background, and nothing else.
    #[test]
    fn a_cache_file_is_named_by_what_it_shows() {
        let dir = std::path::Path::new("cache");
        let look = serde_json::json!({"gender": "male", "selections": {"Hat": {"asset_id": "a", "material_id": "m"}}});
        let a = cache_file(dir, &look, &setup(), None, "lib");
        assert_eq!(a, cache_file(dir, &look, &setup(), None, "lib"), "the same picture, the same file");
        assert!(a.starts_with(dir) && a.extension().is_some_and(|e| e == "rgba"));
        let other = serde_json::json!({"gender": "female", "selections": {}});
        assert_ne!(a, cache_file(dir, &other, &setup(), None, "lib"), "another look");
        let mut moved = setup();
        moved.camera[1] = 2.7;
        assert_ne!(a, cache_file(dir, &look, &moved, None, "lib"), "another setup");
        assert_ne!(a, cache_file(dir, &look, &setup(), Some(&(PathBuf::from("bg.rgba"), [4, 2])), "lib"), "another background");
        assert_ne!(a, cache_file(dir, &look, &setup(), None, "lib2"), "a re-run clothing script or boot stage");
    }

    /// A photo already in its cache file is finished at once, never drawn; one of the wrong size is drawn again.
    #[test]
    fn a_cached_photo_is_ready_without_drawing() {
        let dir = std::env::temp_dir().join(format!("skate-team-photo-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let job = |key: &str, file: PathBuf| PhotoJob { key: key.into(), profile: Value::Null, setup: setup(), background: None, file };
        let (hit, short) = (dir.join("hit.rgba"), dir.join("short.rgba"));
        std::fs::write(&hit, [7u8; 4 * 2 * 4]).unwrap();
        std::fs::write(&short, [7u8; 3]).unwrap();
        let mut photos = Photos::default();
        photos.request(job("cached", hit.clone()));
        assert!(photos.idle(), "nothing to draw");
        assert_eq!(photos.finished.len(), 1);
        assert_eq!((photos.finished[0].key.as_str(), photos.finished[0].file.clone()), ("cached", Some((hit, [4, 2]))));
        photos.request(job("torn", short));
        photos.request(job("new", dir.join("missing.rgba")));
        assert!(!photos.idle() && photos.finished.len() == 1, "a torn or missing file is drawn");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A failed write's leftover goes after a minute; a picture only once it is older than `unused`.
    #[test]
    fn the_sweep_drops_leftovers_and_old_pictures() {
        let dir = std::env::temp_dir().join(format!("skate-team-photo-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ago = |s: u64| std::time::SystemTime::now() - std::time::Duration::from_secs(s);
        for (name, age) in [("old.1.part", 120), ("new.1.part", 0), ("old.rgba", 3 * 86400), ("new.rgba", 0)] {
            let file = std::fs::File::create(dir.join(name)).unwrap();
            file.set_modified(ago(age)).unwrap();
        }
        let left = || {
            let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            names.sort();
            names
        };
        sweep(&dir, None);
        assert_eq!(left(), ["new.1.part", "new.rgba", "old.rgba"], "a card picture stays however old");
        sweep(&dir, Some(std::time::Duration::from_secs(86400)));
        assert_eq!(left(), ["new.1.part", "new.rgba"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Colour, then the background art, then the skater, each over the last by its alpha.
    #[test]
    fn a_photo_lies_over_its_background_and_colour() {
        let mut shot = image::RgbaImage::new(4, 2);
        shot.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut background = image::RgbaImage::new(4, 2);
        background.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        let out = composite(&shot, setup().colour, Some(&background));
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [0, 255, 0, 255]);
        assert_eq!(out.get_pixel(2, 0).0, [51, 102, 153, 255]);
    }

    /// The five card photos drawn by the engine path, headless, on the installed assets: presets 1-5 in
    /// team_profile_00-04 (SKATE3_ASSET_ROOT = the install's assets, SKATE_BOOT_ROOT = prepared boot data with the
    /// lens/sun/SH fields, a GPU adapter; nothing of the game is started). Each is a 128 x 256 full-body portrait
    /// over its background; asked again, every one comes from the cache at once. SKATE_PHOTO_OUT=dir keeps them
    /// (`photo-k.png` and the raw `photo-k.rgba` the screen render takes).
    #[test]
    #[ignore]
    fn the_card_photos_draw_full_body_and_come_back_from_the_cache() {
        use bevy::{
            render::{
                RenderPlugin,
                pipelined_rendering::PipelinedRenderingPlugin,
                settings::{Backends, RenderCreation, WgpuSettings},
            },
            window::{ExitCondition, WindowPlugin},
            winit::WinitPlugin,
        };
        // A photo that is not drawn says why with warn!; LogPlugin is off here, so a plain subscriber shows it.
        let _ = bevy::log::tracing_subscriber::fmt().with_writer(std::io::stderr).try_init();
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let boot = PathBuf::from(std::env::var("SKATE_BOOT_ROOT").expect("SKATE_BOOT_ROOT"));
        let team: crate::team_select::TeamFile = serde_json::from_slice(&std::fs::read(boot.join("runtime/team.json")).unwrap()).unwrap();
        let library_path = crate::customiser_parts::asset_directory(&assets).join("library-v3.json");
        let library: crate::customiser_parts::Library = serde_json::from_slice(&std::fs::read(&library_path).unwrap()).unwrap();
        let mut app = App::new();
        app.add_plugins(
            crate::app::without_dlss(DefaultPlugins
                .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, close_when_requested: false, ..default() })
                .set(RenderPlugin {
                    render_creation: RenderCreation::Automatic(WgpuSettings { backends: Some(Backends::VULKAN | Backends::DX12), ..default() }),
                    ..default()
                })
                .set(AssetPlugin { file_path: assets.to_string_lossy().into_owned(), ..default() })
                .disable::<WinitPlugin>()
                .disable::<bevy::log::LogPlugin>()
                .disable::<PipelinedRenderingPlugin>()
                .disable::<bevy::gilrs::GilrsPlugin>()),
        );
        crate::customiser_material::register(&mut app);
        crate::retail_character::lighting_for_tests(&mut app);
        #[derive(Resource)]
        struct Keep(#[allow(dead_code)] Handle<Shader>);
        let common = Shader::from_wgsl(include_str!("retail_character_common.wgsl"), "retail_character_common.wgsl");
        let handle = app.world_mut().resource_mut::<Assets<Shader>>().add(common);
        app.insert_resource(Keep(handle));
        let parts = Parts::for_assets(library, assets.clone());
        let config = crate::config::Config {
            asset_root: assets.clone(),
            verification_capture: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
            map: None,
            map_path: None,
            difficulty: default(),
            check_assets: false,
            start_paused: false,
            multiplayer: default(),
            map_fingerprint: 0,
            teleport: None,
        };
        // The presets as the library resolves them (the boot data's converted recipes, preset 03's female-flagged
        // socks included; one it refuses is reported, not drawn).
        let looks: Vec<Value> = team
            .presets
            .iter()
            .filter_map(|p| parts.resolve(&p.profile).map_err(|e| eprintln!("{}: {e}", p.name)).ok())
            .collect();
        assert!(looks.len() >= 5, "five presets to draw");
        let jobs: Vec<PhotoJob> = (0..5)
            .map(|k| {
                let setup = team.photos[&format!("team_profile_{k:02}")].clone();
                let profile = looks[k].clone();
                let background = setup.background.as_ref().and_then(|b| team.thumbnails.get(b)).map(|t| (boot.join(&t.rgba), [t.width, t.height]));
                let dir = std::env::temp_dir().join(format!("skate-team-photo-gpu-{}", std::process::id()));
                let file = cache_file(&dir, &profile, &setup, background.as_ref(), "");
                PhotoJob { key: format!("photo-{k}"), profile, setup, background, file }
            })
            .collect();
        let dir = jobs[0].file.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(&dir);
        app.insert_resource(parts).insert_resource(config).add_plugins(PhotoPlugin);
        app.finish();
        app.cleanup();
        let started = std::time::Instant::now();
        for job in &jobs {
            app.world_mut().resource_mut::<Photos>().request(job.clone());
        }
        while !app.world().resource::<Photos>().idle() && started.elapsed().as_secs() < 300 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let cold = started.elapsed();
        let done = std::mem::take(&mut app.world_mut().resource_mut::<Photos>().finished);
        assert_eq!(done.len(), 5, "five photos");
        let undrawn: Vec<&str> = done.iter().filter(|d| d.file.is_none()).map(|d| d.key.as_str()).collect();
        assert!(undrawn.is_empty(), "not drawn: {undrawn:?}");
        // Asked again (the screen opening, the same looks): every one at once from its file, nothing drawn.
        let started = std::time::Instant::now();
        for job in &jobs {
            app.world_mut().resource_mut::<Photos>().request(job.clone());
        }
        let cached = started.elapsed();
        let photos = app.world().resource::<Photos>();
        assert!(photos.idle() && photos.finished.len() == 5, "all from the cache");
        eprintln!("5 card photos: drawn in {cold:?}, from the cache in {cached:?}");
        let out = std::env::var("SKATE_PHOTO_OUT").ok().map(PathBuf::from);
        for (k, job) in jobs.iter().enumerate() {
            let done = done.iter().find(|d| d.key == job.key).unwrap();
            let (path, [w, h]) = done.file.clone().unwrap_or_else(|| panic!("{} not drawn", job.key));
            assert_eq!((w, h), (128, 256), "team_profile is a 128 x 256 portrait");
            let photo = image::RgbaImage::from_raw(w, h, std::fs::read(&path).unwrap()).unwrap();
            let (bg, [bw, bh]) = job.background.clone().unwrap();
            let bg = image::RgbaImage::from_raw(bw, bh, std::fs::read(bg).unwrap()).unwrap();
            // The skater: pixels the photo changed from its background art.
            let changed: Vec<(u32, u32)> = photo
                .enumerate_pixels()
                .filter(|(x, y, p)| p.0.iter().zip(bg.get_pixel(*x, *y).0).take(3).any(|(a, b)| a.abs_diff(b) > 40))
                .map(|(x, y, _)| (x, y))
                .collect();
            let top = changed.iter().map(|p| p.1).min().unwrap();
            let bottom = changed.iter().map(|p| p.1).max().unwrap();
            eprintln!("photo {k}: skater rows {top}..{bottom}, {} px", changed.len());
            if let Some(out) = &out {
                std::fs::create_dir_all(out).unwrap();
                photo.save(out.join(format!("photo-{k}.png"))).unwrap();
                std::fs::copy(&path, out.join(format!("photo-{k}.rgba"))).unwrap();
            }
            assert!(photo.pixels().all(|p| p.0[3] == 255), "opaque");
            assert!(top < 40 && bottom > 200, "photo {k}: head to feet fill the picture ({top}..{bottom})");
        }
        app.world_mut().write_message(bevy::app::AppExit::Success);
        let _ = std::fs::remove_dir_all(dir);
    }
}
