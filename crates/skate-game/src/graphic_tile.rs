//! Preview tiles for saved Custom Graphics: the item with the saved design worn, framed like Skate 3's
//! clothing tiles (the reference is tools/pipeline/skate2port/s1render.py, which matches retail's
//! per-slot framing offline), drawn by the engine into an off-screen image and kept as a PNG
//! (`graphic_store::tile_file`), drawn again if it is missing or `TILE_VERSION` changed.
//!
//! A tile is drawn on its own hidden default skater, never on the one the player is editing: the
//! default outfit of the player's gender (head, eyes, hair, arms, pants as the slot's context, as
//! s1render) with the item and the design on it, in the model's own pose with the arms lowered 38
//! degrees (s1render's `pose_arms`), on a private render layer with its own camera and its own key
//! and fill lights (s1render's), drawn 4x4 supersampled and box-filtered down to 128 px. Nothing of
//! it touches the player's draft, `Parts::applied` (which is what other players are sent) or the
//! player's materials. A capture is only kept once the skater wears the design with every asset
//! loaded, and the same picture came back twice.
use crate::{
    custom_graphic::{self as graphic, REMOTE_SIZE, Side},
    customiser::Customiser,
    customiser_material::SkaterMaterial,
    customiser_parts::Parts,
    graphic_store::Saved,
};
use bevy::{
    asset::RenderAssetUsages,
    camera::{
        ClearColorConfig, RenderTarget, ScalingMode,
        primitives::Aabb,
        visibility::{NoFrustumCulling, RenderLayers},
    },
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    mesh::morph::MorphWeights,
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    scene::{SceneInstance, SceneSpawner},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

/// Retail's clothing tile size.
pub(crate) const SIZE: u32 = 128;
/// s1render.py: drawn 4x4 and averaged down.
pub(crate) const SUPER: u32 = 4;
/// s1render.py: the framed points fill 0.9 of the tile along their longer extent.
pub(crate) const FILL: f32 = 0.9;
/// The private render layer of the tile skater, its camera and its lights (no other camera or light
/// uses it, so nothing of the world lights or shows in a tile).
pub(crate) const TILE_LAYER: usize = 41;
/// s1render's ARM_DROP: the bind pose's arms hang this many degrees lower for a top's tile.
const ARM_DROP: f32 = 38.;
/// Seconds a tile may take from start to capture before it is given up (nothing is kept).
const TIMEOUT: f64 = 30.;
/// Frames for the camera's new pipelines and the skater's textures to be on screen before a capture; past them it
/// still waits while a pipeline compiles (team_photo::compiled): Bevy skips a mesh whose pipeline is not ready, two
/// such captures agree and the tile is kept without that piece.
const SETTLE: u32 = 4;
/// Captures before a tile is given up: one has to match the one before it.
const TRIES: u32 = 12;
/// Bevy's default exposure (Blender's EV100 9.7) turns lux into output at 1 / (2^9.7 * 1.2); s1render's
/// lighting is 0.42 ambient + 0.62 key + 0.22 fill of the albedo, so (a Lambert surface is albedo / pi):
/// key 0.62 pi / 0.0010017 and fill 0.22 pi / 0.0010017 lux, ambient 0.42 / 0.0010017 cd/m2.
const KEY_LUX: f32 = 1944.;
const FILL_LUX: f32 = 690.;
pub(crate) const AMBIENT: f32 = 419.;
/// s1render's key and fill, the directions towards the lights in view space (x right, y up, z towards the viewer).
const KEY: Vec3 = Vec3::new(-0.45, 0.55, 0.7);
const FILL_DIRECTION: Vec3 = Vec3::new(0.6, 0.1, 0.5);

/// A matrix written row by row, as in the numpy source.
fn rows(r: [[f32; 3]; 3]) -> Mat3 {
    Mat3::from_cols_array_2d(&r).transpose()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Slot {
    Top,
    Hat,
    Deck,
}
impl Slot {
    pub fn of(part_slot: &str) -> Option<Self> {
        Some(match part_slot {
            "OuterTorso" => Self::Top,
            "Hat" => Self::Hat,
            "SkateBoard" => Self::Deck,
            _ => return None,
        })
    }
    /// World (skater space, y up, facing +Z) to view rotation, as s1render's `VIEWS`: tops yaw 22
    /// pitch -4, hats yaw 28 pitch -12 (`camera(yaw, pitch)` = rx @ ry), decks looked at with
    /// screen x along the board, depth -Y, then rolled 45 degrees.
    pub fn view(self) -> Mat3 {
        self.facing("front")
    }
    /// `view`, a top turned to the side `zone` (`shown_zone`): retail's tiles only ever show the front.
    pub fn facing(self, zone: &str) -> Mat3 {
        let camera = |yaw: f32, pitch: f32| {
            let ((sy, cy), (sp, cp)) = (yaw.to_radians().sin_cos(), pitch.to_radians().sin_cos());
            rows([[1., 0., 0.], [0., cp, -sp], [0., sp, cp]]) * rows([[cy, 0., sy], [0., 1., 0.], [-sy, 0., cy]])
        };
        match self {
            // Not retail (user 10-19): a design on the back shows the back (the same 22 degree three-quarter), one
            // on a sleeve that sleeve straight on, turned as Edit Skater turns the skater for the side
            // (`cas_menu::Frame::of_zone`).
            Self::Top => camera(
                match zone {
                    "back" => 202.,
                    "left" => 90.,
                    "right" => -90.,
                    _ => 22.,
                },
                -4.,
            ),
            Self::Hat => camera(28., -12.),
            Self::Deck => {
                let (s, c) = 45f32.to_radians().sin_cos();
                rows([[c, -s, 0.], [s, c, 0.], [0., 0., 1.]]) * rows([[0., 0., 1.], [-1., 0., 0.], [0., -1., 0.]])
            }
        }
    }
    /// The slots s1render draws with the item as its context (the rest of the default outfit is left out).
    fn context(self) -> &'static [&'static str] {
        match self {
            Self::Top => &["Rostral", "Organ", "Hair", "Arm", "Pants"],
            Self::Hat => &["Rostral", "Organ"],
            Self::Deck => &[],
        }
    }
}

/// Where the tile camera stands, in skater space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shot {
    /// Camera orientation (looks down its -Z).
    pub rotation: Quat,
    /// The point it looks at: the middle of the framed points.
    pub centre: Vec3,
    /// Side of the square the orthographic view shows: the longer extent over `FILL`.
    pub size: f32,
}
/// Not retail (user 10-19): the side a saved top's tile faces: the one with the most layers, the first of
/// front, back, left, right on a tie (a side of one colour counts above a blank one).
pub(crate) fn shown_zone(saved: &Saved) -> &'static str {
    let weight = |zone: &str| saved.sides.get(zone).map_or((0, 0), |t| (graphic::Doc::decode_any(t).map_or(0, |d| d.layers.len()), 1));
    graphic::SHIRT.iter().rev().copied().max_by_key(|z| weight(z)).unwrap_or("front")
}
/// s1render's `rasterise`: project the framed points (`view`: `Slot::facing`), centre their extents, fill 0.9 of the tile.
pub(crate) fn shot(view: Mat3, points: &[Vec3]) -> Shot {
    let projected: Vec<Vec3> = points.iter().map(|p| view * *p).collect();
    let lo = projected.iter().copied().reduce(Vec3::min).unwrap_or_default();
    let hi = projected.iter().copied().reduce(Vec3::max).unwrap_or_default();
    let extent = (hi.x - lo.x).max(hi.y - lo.y).max(1e-3);
    // View space back to skater space is the transpose of a rotation.
    let back = view.transpose();
    Shot { rotation: Quat::from_mat3(&back), centre: back * ((lo + hi) / 2.), size: extent / FILL }
}
/// The points a slot frames, from the item's bounding box in skater space (`min`, `max`).
// ponytail: s1render frames mesh vertices; the engine has boxes. A top frames the torso slab it uses
// (y 0.9 to 1.54, |x| < 0.3, chest depth taken as 0.4); a hat frames itself plus the top 0.24 m of
// head under it; a deck frames its box. Tune against a preview if a tile looks off.
pub(crate) fn frame_points(slot: Slot, min: Vec3, max: Vec3) -> Vec<Vec3> {
    let (min, max) = match slot {
        Slot::Top => (Vec3::new(-0.3, 0.9, -0.2), Vec3::new(0.3, 1.54, 0.2)),
        Slot::Hat => (min - Vec3::Y * 0.24, max),
        Slot::Deck => (min, max),
    };
    (0..8)
        .map(|i| Vec3::new(if i & 1 == 0 { min.x } else { max.x }, if i & 2 == 0 { min.y } else { max.y }, if i & 4 == 0 { min.z } else { max.z }))
        .collect()
}
/// s1render's key and fill for a camera turned `rotation` (view to skater space): how bright each
/// light is and where it stands. A directional light shines along its -Z, so it looks away from the
/// direction it comes from.
pub(crate) fn lights(rotation: Quat) -> [(f32, Transform); 2] {
    [(KEY_LUX, KEY), (FILL_LUX, FILL_DIRECTION)].map(|(lux, from)| (lux, Transform::IDENTITY.looking_to(-(rotation * from.normalize()), Vec3::Y)))
}
/// A joint's new local rotation when the arm it heads turns `degrees` about the skater's Z axis around
/// its own position (s1render's `pose_arms`, left arm -, right arm +), given its parent's world
/// rotation and its own local one. The joints below it follow through the hierarchy.
pub(crate) fn dropped(parent: Quat, local: Quat, degrees: f32) -> Quat {
    parent.inverse() * Quat::from_rotation_z(degrees.to_radians()) * parent * local
}

/// An empty RGBA target image the tile camera draws into and a screenshot reads back.
pub(crate) fn target(size: u32) -> Image {
    let mut image = Image::new_fill(
        Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0; 4],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    image
}
/// A captured image as straight-alpha RGBA bytes.
pub(crate) fn rgba(image: &Image) -> Option<image::RgbaImage> {
    Some(image.clone().try_into_dynamic().ok()?.to_rgba8())
}
/// s1render's downsample: the average of each `factor` x `factor` block, weighted by alpha so the
/// transparent background does not darken the edge.
pub(crate) fn downsample(big: &image::RgbaImage, factor: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(big.width() / factor, big.height() / factor, |x, y| {
        let (mut sum, mut alpha) = ([0f32; 3], 0f32);
        for j in 0..factor {
            for i in 0..factor {
                let p = big.get_pixel(x * factor + i, y * factor + j).0;
                let a = f32::from(p[3]) / 255.;
                for c in 0..3 {
                    sum[c] += f32::from(p[c]) * a;
                }
                alpha += a;
            }
        }
        let channel = |c: usize| if alpha > 0. { (sum[c] / alpha).round().clamp(0., 255.) as u8 } else { 0 };
        image::Rgba([channel(0), channel(1), channel(2), (alpha / (factor * factor) as f32 * 255.).round() as u8])
    })
}
/// Write a tile as a straight-alpha RGBA PNG.
pub(crate) fn write_png(image: &image::RgbaImage, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let pending = path.with_extension("tmp");
    image.save_with_format(&pending, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(pending, path).map_err(|e| e.to_string())
}
/// What a capture means: nothing drawn yet, a picture that changed since the last (still loading),
/// or the same picture twice (done).
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Empty,
    Changed,
    Done,
}
pub(crate) fn judge(previous: Option<&image::RgbaImage>, now: &image::RgbaImage) -> Verdict {
    if now.pixels().all(|p| p.0[3] == 0) {
        Verdict::Empty
    } else if previous == Some(now) {
        Verdict::Done
    } else {
        Verdict::Changed
    }
}

/// A tile to draw: the item model it is for, the saved design to wear, and the PNG to write.
#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub item: String,
    pub saved: Saved,
    pub path: PathBuf,
}
#[derive(PartialEq)]
enum Stage {
    /// The scenes and textures load; then the tile's own materials go on.
    Loading,
    /// The arms come down once the hierarchy has its world transforms.
    Posing,
    /// The item's bounds are known: camera and lights.
    Framing,
    /// Captures until two in a row agree.
    Shooting,
}
struct Active {
    id: u64,
    job: Job,
    started: f64,
    stage: Stage,
    frames: u32,
    /// The default skater's outfit with the item and the saved designs on it.
    profile: Value,
    root: Entity,
    /// Each piece's scene, item id and material id.
    scenes: Vec<(Entity, String, String)>,
    /// The tile's own materials (the player's are never touched).
    materials: Vec<Handle<SkaterMaterial>>,
    target: Handle<Image>,
    camera: Option<Entity>,
    lights: Vec<Entity>,
    /// A screenshot is on its way.
    asked: bool,
    previous: Option<image::RgbaImage>,
    attempts: u32,
}
#[derive(Resource, Default)]
pub(crate) struct Tiles {
    queue: VecDeque<Job>,
    active: Option<Active>,
    next_id: u64,
    /// The latest screenshot, with the id of the tile it was taken for.
    captured: Option<(u64, Image)>,
    /// Finished PNGs, newest last, for the menu to refresh its tiles.
    pub finished: Vec<PathBuf>,
    /// The Graphic Creator is open: no new tile starts (it would load assets while the player edits).
    pub hold: bool,
}
impl Tiles {
    /// Ask for a tile unless its PNG exists or it is already waiting.
    pub fn request(&mut self, job: Job) {
        let waiting = self.queue.iter().chain(self.active.iter().map(|a| &a.job)).any(|j| j.path == job.path);
        if !job.path.exists() && !waiting {
            self.queue.push_back(job);
        }
    }
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }
}
impl Active {
    /// Take everything the tile spawned or drew away again.
    fn finish(self, commands: &mut Commands, parts: &mut Parts) {
        commands.entity(self.root).despawn();
        for entity in self.camera.into_iter().chain(self.lights) {
            commands.entity(entity).despawn();
        }
        // Sides drawn for the tile are worn by nobody else.
        for (zone, entry) in self.profile["graphics"].as_object().into_iter().flatten() {
            let rect = parts.side_rect(&self.profile["selections"], zone);
            if let (Some(design), Some(rect)) = (graphic::design(entry), rect) {
                parts.designs.forget(&Side { design, rect, over: None }, REMOTE_SIZE);
            }
        }
    }
}

pub(crate) struct TilePlugin;
impl Plugin for TilePlugin {
    fn build(&self, app: &mut App) {
        crate::team_photo::track_pipelines(app);
        app.init_resource::<Tiles>().add_systems(Update, (begin, load, shoot).chain());
    }
}

/// The patch that puts a saved Custom Graphic on an outfit: its designs on its sides, no design
/// on the other sides and no stock logo (the tile shows the saved graphic alone).
pub(crate) fn wear(saved: &Saved) -> Value {
    let sides: serde_json::Map<String, Value> = graphic::ZONES
        .iter()
        .map(|z| (z.0.to_owned(), json!({ "design": saved.sides.get(z.0), "logo": Value::Null })))
        .collect();
    json!({ "graphics": sides })
}
/// The outfit a tile draws: the default skater of `gender` in the item's first logoable material (the
/// authored colour; the player's own colours do not enter a tile), its slot's context, and `saved`'s
/// designs on it. None when the library lacks the item or the default.
pub(crate) fn outfit(parts: &Parts, gender: &str, item: &str, saved: &Saved) -> Option<Value> {
    let lib = &parts.library;
    let model = lib.models.get(item)?;
    let slot = Slot::of(&model.slot)?;
    let logoable = |id: &&String| lib.materials.get(*id).is_some_and(|m| m.flag("IsLogoable").eq_ignore_ascii_case("true"));
    let material = model.materials.iter().find(logoable).or(model.materials.first())?;
    let mut profile = lib.defaults.get(gender)?.clone();
    profile["selections"][&model.slot] = json!({ "asset_id": item, "material_id": material });
    profile["graphics"] = wear(saved)["graphics"].clone();
    // The resolved outfit brings the arms, skin and hair the item needs; then only the context stays.
    let mut profile = parts.resolve(&profile).ok()?;
    let keep: Vec<&str> = slot.context().iter().copied().chain([model.slot.as_str()]).collect();
    profile["selections"].as_object_mut()?.retain(|name, _| keep.contains(&name.as_str()));
    Some(profile)
}

/// Pick the next tile and spawn its skater: hidden, on the private layer, at the origin.
fn begin(
    mut commands: Commands,
    mut tiles: ResMut<Tiles>,
    customiser: Res<Customiser>,
    mut parts: ResMut<Parts>,
    server: Res<AssetServer>,
    time: Res<Time<Real>>,
) {
    let (tiles, now) = (&mut *tiles, time.elapsed_secs_f64());
    if let Some(started) = tiles.active.as_ref().map(|a| a.started) {
        if now - started > TIMEOUT {
            if let Some(active) = tiles.active.take() {
                warn!("Custom Graphic tile {} not drawn: gave up after {TIMEOUT} s", active.job.path.display());
                active.finish(&mut commands, &mut parts);
            }
        }
        return;
    }
    // Only while Edit Skater is open on the item the job is for, and not under the Graphic Creator.
    if !customiser.open || tiles.hold {
        return;
    }
    while let Some(job) = tiles.queue.pop_front() {
        if job.path.exists() {
            continue;
        }
        let worn = customiser.draft["selections"].as_object().into_iter().flat_map(|s| s.values()).any(|w| w["asset_id"].as_str() == Some(job.item.as_str()));
        if !worn {
            continue; // asked again when the item is worn
        }
        let Some(profile) = outfit(&parts, customiser.gender(), &job.item, &job.saved) else {
            warn!("Custom Graphic tile for {}: the default skater cannot wear it", job.item);
            continue;
        };
        let root = commands.spawn((Name::new("Custom Graphic tile skater"), Transform::default(), Visibility::Hidden, RenderLayers::layer(TILE_LAYER))).id();
        let mut scenes = Vec::new();
        for value in profile["selections"].as_object().into_iter().flat_map(|s| s.values()) {
            let (Some(id), Some(mid)) = (value["asset_id"].as_str(), value["material_id"].as_str()) else { continue };
            let Some(part) = parts.library.models.get(id) else { continue };
            // perf 10-19: through Part::load (SKATE_PARTS_GPU_ONLY), like every loader of a part.
            let scene = commands.spawn((SceneRoot(part.load(&server, Some(GltfAssetLabel::Scene(0)))), ChildOf(root))).id();
            scenes.push((scene, id.to_owned(), mid.to_owned()));
        }
        tiles.next_id += 1;
        tiles.active = Some(Active {
            id: tiles.next_id,
            job,
            started: now,
            stage: Stage::Loading,
            frames: 0,
            profile,
            root,
            scenes,
            materials: Vec::new(),
            target: Handle::default(),
            camera: None,
            lights: Vec::new(),
            asked: false,
            previous: None,
            attempts: 0,
        });
        return;
    }
}

/// Wait for the pieces and their textures and designs, then give each piece its own material.
#[allow(clippy::too_many_arguments)]
fn load(
    mut commands: Commands,
    mut tiles: ResMut<Tiles>,
    mut parts: ResMut<Parts>,
    server: Res<AssetServer>,
    spawner: Res<SceneSpawner>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<SkaterMaterial>>,
    instances: Query<&SceneInstance>,
    meshes: Query<Entity, With<Mesh3d>>,
    parents: Query<&ChildOf>,
    mut morphs: Query<(Entity, &mut MorphWeights)>,
) {
    let Some(active) = tiles.active.as_mut().filter(|a| a.stage == Stage::Loading) else { return };
    if active.scenes.iter().any(|(scene, ..)| !instances.get(*scene).is_ok_and(|i| spawner.instance_is_ready(**i))) {
        return;
    }
    for (_, _, mid) in &active.scenes {
        parts.warm(mid, &server, &mut materials);
    }
    // All three advance their loading each frame (no short circuit); the designs are drawn here, at
    // the size other players see.
    let ready = parts.graphics_ready(&active.profile, &server, &mut images, REMOTE_SIZE)
        & parts.tattoos_ready(&active.profile, &server)
        & active.scenes.iter().all(|(_, _, mid)| parts.material_ready(mid, &server));
    if !ready {
        return;
    }
    for (scene, id, mid) in &active.scenes {
        let Some(mut material) = parts.profile_material(id, mid, &active.profile, &materials) else { continue };
        // Lit by the tile's own lights through Bevy's PBR path, not by the world's retail light (which
        // changes with the time of day and the map, and would make a tile differ from run to run).
        material.extension.retail.light = Vec4::ZERO;
        material.extension.retail.tint = Vec4::ZERO;
        let handle = materials.add(material);
        for mesh in &meshes {
            if parents.iter_ancestors(mesh).any(|p| p == *scene) {
                commands
                    .entity(mesh)
                    .remove::<MeshMaterial3d<StandardMaterial>>()
                    .insert((MeshMaterial3d(handle.clone()), RenderLayers::layer(TILE_LAYER), NoFrustumCulling));
            }
        }
        active.materials.push(handle);
    }
    // The body the way the game shows it (the same default face and body morphs as a remote skater).
    let weights: Vec<f32> = parts
        .library
        .morphs
        .iter()
        .enumerate()
        .map(|(i, name)| active.profile["morphs"][name].as_f64().unwrap_or(if (2..19).contains(&i) { 0.25 } else { 0. }).clamp(0., 0.5) as f32)
        .collect();
    for (entity, mut morph) in &mut morphs {
        if morph.weights().len() == weights.len() && parents.iter_ancestors(entity).any(|p| p == active.root) {
            morph.weights_mut().copy_from_slice(&weights);
        }
    }
    active.stage = Stage::Posing;
    active.frames = 0;
}

/// Pose, frame, light and capture. A capture counts when it is not empty and equals the one before.
#[allow(clippy::too_many_arguments)]
fn shoot(
    mut commands: Commands,
    mut tiles: ResMut<Tiles>,
    mut parts: ResMut<Parts>,
    mut images: ResMut<Assets<Image>>,
    mut joints: Query<(Entity, &Name, &mut Transform)>,
    globals: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    bounds: Query<(&Aabb, &GlobalTransform)>,
    waiting: Res<crate::team_photo::PipelinesWaiting>,
) {
    let tiles = &mut *tiles;
    let Some(active) = tiles.active.as_mut() else { return };
    match active.stage {
        Stage::Loading => {}
        Stage::Posing => {
            active.frames += 1;
            if active.frames < 2 {
                return; // the scenes' world transforms are in
            }
            for (entity, name, mut transform) in &mut joints {
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
                transform.rotation = dropped(parent.to_scale_rotation_translation().1, transform.rotation, degrees);
            }
            commands.entity(active.root).insert(Visibility::Inherited);
            active.stage = Stage::Framing;
            active.frames = 0;
        }
        Stage::Framing => {
            active.frames += 1;
            if active.frames < 2 {
                return; // the lowered arms' world transforms and the meshes' bounds are in
            }
            let slot = parts.library.models.get(&active.job.item).and_then(|p| Slot::of(&p.slot)).unwrap_or(Slot::Top);
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let mut stack: Vec<Entity> = active.scenes.iter().filter(|s| s.1 == active.job.item).map(|s| s.0).collect();
            while let Some(entity) = stack.pop() {
                if let Ok((aabb, global)) = bounds.get(entity) {
                    let (centre, half) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
                    for i in 0..8 {
                        let sign = Vec3::new(if i & 1 == 0 { -1. } else { 1. }, if i & 2 == 0 { -1. } else { 1. }, if i & 4 == 0 { -1. } else { 1. });
                        let p = global.transform_point(centre + half * sign);
                        (lo, hi) = (lo.min(p), hi.max(p));
                    }
                }
                stack.extend(children.get(entity).into_iter().flatten());
            }
            let shot = shot(slot.facing(shown_zone(&active.job.saved)), &frame_points(slot, lo, hi));
            active.target = images.add(target(SIZE * SUPER));
            // Orthographic: how far back hardly matters, so stand 3 m off with room in front and behind.
            let translation = shot.centre + shot.rotation * (Vec3::Z * 3.);
            active.camera = Some(
                commands
                    .spawn((
                        Camera3d::default(),
                        Camera { order: -50, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
                        RenderTarget::Image(active.target.clone().into()),
                        Projection::Orthographic(OrthographicProjection {
                            near: 0.1,
                            far: 6.,
                            scaling_mode: ScalingMode::Fixed { width: shot.size, height: shot.size },
                            ..OrthographicProjection::default_3d()
                        }),
                        Transform { translation, rotation: shot.rotation, scale: Vec3::ONE },
                        RenderLayers::layer(TILE_LAYER),
                        Msaa::Off,
                        Tonemapping::None,
                        DebandDither::Disabled,
                        bevy::light::AmbientLight { color: Color::WHITE, brightness: AMBIENT, ..default() },
                    ))
                    .id(),
            );
            for (lux, at) in lights(shot.rotation) {
                active.lights.push(
                    commands
                        .spawn((DirectionalLight { illuminance: lux, shadows_enabled: false, ..default() }, at, RenderLayers::layer(TILE_LAYER)))
                        .id(),
                );
            }
            active.stage = Stage::Shooting;
            active.frames = 0;
        }
        Stage::Shooting => {
            active.frames += 1;
            if let Some((_, image)) = tiles.captured.take_if(|(id, _)| *id == active.id) {
                active.asked = false;
                active.frames = 0;
                active.attempts += 1;
                let tile = rgba(&image).map(|big| downsample(&big, SUPER));
                match tile.as_ref().map(|t| judge(active.previous.as_ref(), t)) {
                    Some(Verdict::Done) => {
                        if let Some(tile) = tile {
                            match write_png(&tile, &active.job.path) {
                                Ok(()) => tiles.finished.push(active.job.path.clone()),
                                Err(error) => warn!("Custom Graphic tile {}: {error}", active.job.path.display()),
                            }
                        }
                    }
                    Some(_) if active.attempts < TRIES => {
                        active.previous = tile;
                        return;
                    }
                    _ => warn!("Custom Graphic tile {} not drawn: nothing steady came back", active.job.path.display()),
                }
                if let Some(active) = tiles.active.take() {
                    active.finish(&mut commands, &mut parts);
                }
            } else if active.frames >= SETTLE && !active.asked && crate::team_photo::compiled(active.frames, waiting.count()) {
                active.asked = true;
                let id = active.id;
                commands.spawn(Screenshot::image(active.target.clone())).observe(move |captured: On<ScreenshotCaptured>, mut tiles: ResMut<Tiles>| {
                    tiles.captured = Some((id, captured.image.clone()));
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::customiser_parts::Library;
    use bevy::{
        app::AppExit,
        render::{
            RenderPlugin,
            pipelined_rendering::PipelinedRenderingPlugin,
            settings::{Backends, RenderCreation, WgpuSettings},
        },
        window::{ExitCondition, WindowPlugin},
        winit::WinitPlugin,
    };
    use std::sync::{Arc, Mutex};

    /// The framing numbers of s1render.py: slot views, 0.9 fill, a longer extent that sets the scale.
    #[test]
    fn framing_follows_s1renders_numbers() {
        // s1render's camera(22, -4) applied to the character's forward axis, worked by hand:
        // ry*(0,0,1) = (sin22, 0, cos22); rx(-4) then gives y = 0.0647, z = 0.9250.
        let forward = Slot::Top.view() * Vec3::Z;
        assert!((forward - Vec3::new(0.3746, 0.0647, 0.9250)).abs().max_element() < 1e-3, "{forward}");
        // The hat's view faces the character's front toward the camera too.
        assert!((Slot::Hat.view() * Vec3::Z).z > 0.85);
        // The deck is looked at from below: depth is -Y (before the roll that leaves depth alone).
        assert!(((Slot::Deck.view() * Vec3::Y).z + 1.).abs() < 1e-5);
        // Each view is a proper rotation.
        for slot in [Slot::Top, Slot::Hat, Slot::Deck] {
            assert!((slot.view().determinant() - 1.).abs() < 1e-5 && (slot.view() * slot.view().transpose()).abs_diff_eq(Mat3::IDENTITY, 1e-5));
        }
        // Fill: the longer extent is 0.9 of the square, the framed points are centred, whatever the slot.
        for slot in [Slot::Top, Slot::Hat, Slot::Deck] {
            let points = frame_points(slot, Vec3::new(-0.15, 1.6, -0.12), Vec3::new(0.15, 1.8, 0.2));
            let s = shot(slot.view(), &points);
            let view = Mat3::from_quat(s.rotation).transpose();
            let local: Vec<Vec3> = points.iter().map(|p| view * (*p - s.centre)).collect();
            let lo = local.iter().copied().reduce(Vec3::min).unwrap();
            let hi = local.iter().copied().reduce(Vec3::max).unwrap();
            let extent = (hi.x - lo.x).max(hi.y - lo.y);
            assert!((extent / s.size - FILL).abs() < 1e-4, "{slot:?}: fills {}", extent / s.size);
            assert!((lo.x + hi.x).abs() < 1e-4 && (lo.y + hi.y).abs() < 1e-4, "{slot:?}: centred");
            assert!(local.iter().all(|p| p.x.abs() <= s.size / 2. && p.y.abs() <= s.size / 2.));
        }
        // A top frames the torso slab whatever the item's own box; a hat adds 0.24 m below.
        assert_eq!(frame_points(Slot::Top, Vec3::ZERO, Vec3::ONE)[0], Vec3::new(-0.3, 0.9, -0.2));
        assert!((frame_points(Slot::Hat, Vec3::new(0., 1.6, 0.), Vec3::ONE)[0].y - 1.36).abs() < 1e-6);
        assert_eq!((SIZE, SUPER, FILL), (128, 4, 0.9));
        assert_eq!(Slot::of("OuterTorso"), Some(Slot::Top));
        assert_eq!(Slot::of("Pants"), None);
        // The same inputs give the same shot every time (the tile is deterministic).
        let a = shot(Slot::Hat.view(), &frame_points(Slot::Hat, Vec3::new(-0.1, 1.7, -0.1), Vec3::new(0.1, 1.85, 0.15)));
        let b = shot(Slot::Hat.view(), &frame_points(Slot::Hat, Vec3::new(-0.1, 1.7, -0.1), Vec3::new(0.1, 1.85, 0.15)));
        assert_eq!((a.rotation, a.centre, a.size), (b.rotation, b.centre, b.size));
    }

    /// User 10-19: a top's tile faces the side its design is on (the most layers), as Edit Skater turns the
    /// skater to that side; a hat or deck keeps its one view.
    #[test]
    fn a_tops_tile_faces_the_side_with_the_design() {
        use crate::custom_graphic::{Art, Doc, Layer};
        let design = |n: usize| Doc { layers: vec![Layer::new(Art::Shape(2), [255; 4]); n], ..default() }.encode();
        let saved = |sides: &[(&str, usize)]| Saved { sides: sides.iter().map(|(z, n)| ((*z).to_owned(), design(*n))).collect() };
        assert_eq!(shown_zone(&saved(&[("back", 1)])), "back", "a back print shows the back");
        assert_eq!(shown_zone(&saved(&[("front", 1), ("left", 3)])), "left", "the most layers");
        assert_eq!(shown_zone(&saved(&[("right", 2), ("back", 2)])), "back", "a tie: front, back, left, right");
        assert_eq!(shown_zone(&saved(&[("right", 0)])), "right", "one colour beats blank");
        assert_eq!(shown_zone(&Saved::default()), "front");
        for zone in crate::custom_graphic::SHIRT {
            // The skater-space direction that faces the tile camera, level...
            let toward = (Slot::Top.facing(zone).transpose() * Vec3::Z * Vec3::new(1., 0., 1.)).normalize();
            // ...is the one Edit Skater turns toward its camera for the side (cas_pose: yaw - turn).
            let turn = crate::cas_menu::Frame::of_zone(zone).view().2;
            assert!(toward.dot(Quat::from_rotation_y(turn) * Vec3::Z) > 0.9, "{zone}: {toward}");
        }
        assert_eq!(Slot::Top.facing("front"), Slot::Top.view());
        assert_eq!(Slot::Hat.facing("back"), Slot::Hat.view(), "a hat has one side");
    }

    #[test]
    fn the_key_and_fill_come_from_where_s1render_puts_them_as_the_camera_sees() {
        for slot in [Slot::Top, Slot::Hat, Slot::Deck] {
            let rotation = shot(slot.view(), &frame_points(slot, Vec3::ZERO, Vec3::ONE)).rotation;
            let [(key_lux, key), (fill_lux, fill)] = lights(rotation);
            assert!(key_lux > fill_lux && fill_lux > 0.);
            // Back in view space: the key stands upper left in front, the fill right and a little up.
            let view = rotation.inverse();
            let (key_from, fill_from) = (view * -key.forward().as_vec3(), view * -fill.forward().as_vec3());
            assert!(key_from.abs_diff_eq(KEY.normalize(), 1e-4), "{slot:?} {key_from}");
            assert!(fill_from.abs_diff_eq(FILL_DIRECTION.normalize(), 1e-4), "{slot:?} {fill_from}");
            // Both shine into the picture, away from the viewer.
            assert!(key.forward().dot(rotation * Vec3::Z) < 0. && fill.forward().dot(rotation * Vec3::Z) < 0.);
        }
    }

    #[test]
    fn lowering_an_arm_turns_it_about_the_skaters_z_axis_whatever_its_parents_turn() {
        let parent = Quat::from_euler(EulerRot::XYZ, 0.4, -1.1, 2.0);
        let local = Quat::from_euler(EulerRot::XYZ, -0.3, 0.2, 0.9);
        for degrees in [-ARM_DROP, ARM_DROP] {
            let after = parent * dropped(parent, local, degrees);
            let wanted = Quat::from_rotation_z(degrees.to_radians()) * parent * local;
            assert!(after.abs_diff_eq(wanted, 1e-5) || after.abs_diff_eq(-wanted, 1e-5), "{degrees}");
        }
        // The right arm (+) turns the opposite way to the left (-): a hand at +X goes up, at -X comes down.
        let hand = |degrees: f32| dropped(Quat::IDENTITY, Quat::IDENTITY, degrees) * Vec3::X;
        assert!(hand(ARM_DROP).y > 0. && hand(-ARM_DROP).y < 0.);
    }

    #[test]
    fn a_big_picture_is_averaged_down_by_alpha_so_edges_keep_their_colour() {
        // A 4x4 block: the left half opaque red, the right half clear: red at half alpha, not dark red.
        let big = image::RgbaImage::from_fn(8, 4, |x, _| if x < 2 { image::Rgba([200, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) });
        let small = downsample(&big, 4);
        assert_eq!((small.width(), small.height()), (2, 1));
        assert_eq!(small.get_pixel(0, 0).0, [200, 0, 0, 128]);
        assert_eq!(small.get_pixel(1, 0).0, [0, 0, 0, 0]);
        // Opaque stays opaque and the same input gives the same bytes.
        let solid = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        assert_eq!(downsample(&solid, 4), downsample(&solid, 4));
        assert_eq!(downsample(&solid, 4).get_pixel(1, 1).0, [10, 20, 30, 255]);
    }

    #[test]
    fn a_tile_waits_for_compiling_pipelines_but_not_past_its_timeout() {
        use crate::team_photo::{compiled, PIPELINE_WAIT};
        assert!(!compiled(SETTLE, 1), "a piece's pipeline is still compiling");
        assert!(compiled(SETTLE, 0));
        assert!(compiled(PIPELINE_WAIT, 1), "one that never finishes does not hold the tile for good");
        assert!(PIPELINE_WAIT as f64 / 60. < TIMEOUT, "it gives up before the tile does");
    }

    #[test]
    fn a_capture_counts_only_when_something_is_drawn_and_it_matches_the_one_before() {
        let clear = image::RgbaImage::new(4, 4);
        let a = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let mut b = a.clone();
        b.put_pixel(0, 0, image::Rgba([9, 9, 9, 255]));
        assert_eq!(judge(None, &clear), Verdict::Empty, "pipelines still compiling: nothing drawn");
        assert_eq!(judge(Some(&clear), &clear), Verdict::Empty, "two empty captures are not a tile");
        assert_eq!(judge(None, &a), Verdict::Changed, "the first capture has nothing to match");
        assert_eq!(judge(Some(&a), &b), Verdict::Changed, "a piece arrived between captures");
        assert_eq!(judge(Some(&a), &a), Verdict::Done);
    }

    #[test]
    fn a_wear_patch_names_every_side_and_drops_the_logo() {
        let saved = Saved { sides: [("front".to_owned(), "AAA".to_owned())].into() };
        let patch = wear(&saved);
        assert_eq!(patch["graphics"]["front"], json!({"design": "AAA", "logo": null}));
        assert_eq!(patch["graphics"]["back"], json!({"design": null, "logo": null}));
        assert_eq!(patch["graphics"].as_object().unwrap().len(), 6);
    }

    #[test]
    fn requests_skip_tiles_that_exist_or_wait() {
        let dir = std::env::temp_dir().join(format!("skate-tile-request-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let job = |name: &str| Job { item: "x".into(), saved: Saved::default(), path: dir.join(name) };
        let mut tiles = Tiles::default();
        tiles.request(job("a.png"));
        tiles.request(job("a.png"));
        std::fs::write(dir.join("b.png"), b"png").unwrap();
        tiles.request(job("b.png"));
        assert_eq!(tiles.queue.len(), 1);
        assert!(tiles.busy());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A small library: a default skater (head, eyes, hair, pants, a tank top) and a tee, a hat and a deck to put on it.
    fn library() -> Library {
        let model = |slot: &str, flags: Value, material: &str| json!({"slot": slot, "name": slot, "flags": flags, "materials": [material], "scene": format!("{slot}.glb")});
        let material = |logoable: bool| json!({"name": "m", "flags": {"IsLogoable": logoable.to_string()}, "diffuse": "x.png", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1});
        let tee_flags = json!({"TopType": "tshirt", "StampUVConstraintQ1": "0.98,0.48,0.13,0.37", "StampUVConstraintQ2": "0.98,0.48,0.64,0.86", "StampUVConstraintQ3": "0.48,0.25,0.05,0.45", "StampUVConstraintQ4": "0.48,0.25,0.55,0.95"});
        serde_json::from_value(json!({
            "models": {
                "head": model("Rostral", json!({}), "plain"), "eyes": model("Organ", json!({}), "plain"), "hair": model("Hair", json!({}), "plain"),
                "pants": model("Pants", json!({}), "plain"), "tank": model("OuterTorso", json!({"TopType": "tshirt"}), "plain"),
                "tee": model("OuterTorso", tee_flags, "print"), "cap": model("Hat", json!({}), "print"), "deck": model("SkateBoard", json!({}), "print"),
                "shoes": model("Feet", json!({}), "plain"),
            },
            "materials": {"plain": material(false), "print": material(true)},
            "defaults": {"male": {"gender": "male", "selections": {
                "Rostral": {"asset_id": "head", "material_id": "plain"}, "Organ": {"asset_id": "eyes", "material_id": "plain"},
                "Hair": {"asset_id": "hair", "material_id": "plain"}, "Pants": {"asset_id": "pants", "material_id": "plain"},
                "OuterTorso": {"asset_id": "tank", "material_id": "plain"}, "Feet": {"asset_id": "shoes", "material_id": "plain"},
                "SkateBoard": {"asset_id": "deck", "material_id": "print"}}}},
            "morphs": [],
        }))
        .unwrap()
    }

    #[test]
    fn a_tile_dresses_the_default_skater_in_the_item_and_its_slots_context_only() {
        let parts = Parts::for_test(library());
        let saved = Saved { sides: [("front".to_owned(), crate::graphic_editor::design(&graphic::Doc { layers: vec![graphic::Layer::new(graphic::Art::Shape(2), [255, 0, 0, 255])], ..default() }).unwrap())].into() };
        let slots = |item: &str| -> Vec<String> {
            let profile = outfit(&parts, "male", item, &saved).unwrap_or_else(|| panic!("{item}"));
            let mut slots: Vec<String> = profile["selections"].as_object().unwrap().keys().cloned().collect();
            slots.sort();
            slots
        };
        // A top: the head, eyes, hair and pants as context, the tee instead of the tank top; no shoes, no deck.
        assert_eq!(slots("tee"), ["Hair", "Organ", "OuterTorso", "Pants", "Rostral"]);
        let tee = outfit(&parts, "male", "tee", &saved).unwrap();
        assert_eq!(tee["selections"]["OuterTorso"], json!({"asset_id": "tee", "material_id": "print"}), "the item, in its logoable material");
        assert!(tee["graphics"]["front"]["design"].is_string() && tee["graphics"].get("back").is_none(), "the saved design is worn, nothing else");
        // A hat: the head and eyes under it, no hair to poke through. A deck: alone.
        assert_eq!(slots("cap"), ["Hat", "Organ", "Rostral"]);
        assert_eq!(slots("deck"), ["SkateBoard"]);
        // Nothing of the player's own look enters a tile, and it is deterministic.
        assert!(tee.get("colours").is_none() && tee.get("tattoos").is_none() && tee.get("skin_tint").is_none());
        assert_eq!(outfit(&parts, "male", "tee", &saved), Some(tee));
        // What a tile cannot show is refused.
        assert!(outfit(&parts, "male", "pants", &saved).is_none(), "pants are not a tile slot");
        assert!(outfit(&parts, "male", "nothing", &saved).is_none());
        assert!(outfit(&parts, "female", "tee", &saved).is_none(), "no default skater of that gender");
    }

    /// The engine path itself, headless: cameras into off-screen images, screenshots of those images
    /// read back, a known colour and a clear transparent pixel found in the bytes, and the PNG written.
    /// Needs a GPU adapter (no window, nothing of the game is started).
    #[test]
    fn an_offscreen_target_reads_back_the_colour_drawn_into_it() {
        let mut app = App::new();
        app.add_plugins(
            crate::app::without_dlss(DefaultPlugins
                .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, close_when_requested: false, ..default() })
                .set(RenderPlugin {
                    render_creation: RenderCreation::Automatic(WgpuSettings { backends: Some(Backends::VULKAN | Backends::DX12), ..default() }),
                    ..default()
                })
                .disable::<WinitPlugin>()
                .disable::<bevy::log::LogPlugin>()
                .disable::<PipelinedRenderingPlugin>()
                .disable::<bevy::gilrs::GilrsPlugin>()),
        );
        app.finish();
        app.cleanup();
        let (solid, clear) = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            (images.add(target(SIZE)), images.add(target(SIZE)))
        };
        for (image, colour, order) in [(&solid, Color::srgb_u8(200, 30, 60), 0), (&clear, Color::NONE, 1)] {
            app.world_mut().spawn((
                Camera3d::default(),
                Camera { order, clear_color: ClearColorConfig::Custom(colour), ..default() },
                RenderTarget::Image(image.clone().into()),
                Msaa::Off,
            ));
        }
        // Let the cameras draw a few frames first, as the tile job does (SETTLE) before it captures.
        for _ in 0..SETTLE {
            app.update();
        }
        let seen: Arc<Mutex<Vec<(usize, Image)>>> = Arc::default();
        for (n, image) in [&solid, &clear].into_iter().enumerate() {
            let seen = seen.clone();
            app.world_mut().spawn(Screenshot::image(image.clone())).observe(move |captured: On<ScreenshotCaptured>| {
                seen.lock().unwrap().push((n, captured.image.clone()));
            });
        }
        for _ in 0..600 {
            app.update();
            if seen.lock().unwrap().len() == 2 {
                break;
            }
        }
        app.world_mut().write_message(AppExit::Success);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "both screenshots came back");
        for (n, image) in seen.iter() {
            assert_eq!((image.width(), image.height()), (SIZE, SIZE));
            let data = image.data.as_ref().expect("pixels");
            assert_eq!(data.len(), (SIZE * SIZE * 4) as usize);
            let pixel = &data[(64 * SIZE as usize + 64) * 4..][..4];
            if *n == 0 {
                assert!(pixel[..3].iter().zip([200u8, 30, 60]).all(|(a, b)| a.abs_diff(b) <= 2) && pixel[3] == 255, "solid: {pixel:?}");
                let path = std::env::temp_dir().join(format!("skate-tile-{}.png", std::process::id()));
                write_png(&rgba(image).unwrap(), &path).unwrap();
                let back = image::open(&path).unwrap().to_rgba8();
                assert_eq!(back.get_pixel(64, 64).0[..3], pixel[..3]);
                let _ = std::fs::remove_file(path);
            } else {
                assert_eq!(pixel[3], 0, "clear: {pixel:?}");
            }
        }
    }

    /// The whole tile path on the installed models (needs SKATE_CAC_TEST_LIBRARY, a GPU adapter and
    /// the install; nothing of the game is started): the default skater wears the tee with a design,
    /// is drawn by the tile camera and written, twice with the same bytes; another design gives
    /// another tile; the player's own draft and `Parts::applied` are never touched.
    /// SKATE_TILE_PNG=dir keeps the pictures.
    #[test]
    #[ignore]
    fn a_tile_of_the_installed_tee_draws_the_same_twice_and_leaves_the_players_outfit_alone() {
        use crate::custom_graphic::{Art, Doc, Layer};
        let library_path = PathBuf::from(std::env::var("SKATE_CAC_TEST_LIBRARY").expect("SKATE_CAC_TEST_LIBRARY"));
        let asset_root = library_path.ancestors().nth(5).expect("assets/private/customisation/sets/<set>/library-v3.json").to_path_buf();
        let library: Library = serde_json::from_slice(&std::fs::read(&library_path).unwrap()).unwrap();
        let mut app = App::new();
        app.add_plugins(
            crate::app::without_dlss(DefaultPlugins
                .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, close_when_requested: false, ..default() })
                .set(RenderPlugin {
                    render_creation: RenderCreation::Automatic(WgpuSettings { backends: Some(Backends::VULKAN | Backends::DX12), ..default() }),
                    ..default()
                })
                .set(AssetPlugin { file_path: asset_root.to_string_lossy().into_owned(), ..default() })
                .disable::<WinitPlugin>()
                .disable::<bevy::log::LogPlugin>()
                .disable::<PipelinedRenderingPlugin>()
                .disable::<bevy::gilrs::GilrsPlugin>()),
        );
        crate::customiser_material::register(&mut app);
        crate::retail_character::lighting_for_tests(&mut app);
        // The stamp shader imports the character lighting module.
        #[derive(Resource)]
        struct Keep(#[allow(dead_code)] Handle<Shader>);
        let common = Shader::from_wgsl(include_str!("retail_character_common.wgsl"), "retail_character_common.wgsl");
        let handle = app.world_mut().resource_mut::<Assets<Shader>>().add(common);
        app.insert_resource(Keep(handle));
        let parts = Parts::for_assets(library, asset_root);
        let customiser = Customiser::for_test(&parts, Vec::new(), "male");
        app.insert_resource(parts).insert_resource(customiser).add_plugins(TilePlugin);
        app.finish();
        app.cleanup();
        let dir = std::env::temp_dir().join(format!("skate-tile-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let design = |shape: u8, rgb: [u8; 3]| {
            let doc = Doc { layers: vec![Layer { w: 2600, h: 2600, ..Layer::new(Art::Shape(shape), [rgb[0], rgb[1], rgb[2], 255]) }], ..default() };
            Saved { sides: [("front".to_owned(), doc.encode())].into() }
        };
        // The player wears a tee (the item the tiles are for) in the draft.
        let tee = "801095192c838106";
        app.world_mut().resource_mut::<Customiser>().draft["selections"]["OuterTorso"] = json!({"asset_id": tee, "material_id": "0000129103e38817"});
        let before = (app.world().resource::<Customiser>().draft.clone(), app.world().resource::<Parts>().applied.clone());
        let jobs = [("a.png", design(2, [220, 30, 30])), ("b.png", design(2, [220, 30, 30])), ("c.png", design(20, [30, 60, 220]))];
        for (name, saved) in jobs {
            app.world_mut().resource_mut::<Tiles>().request(Job { item: tee.into(), saved, path: dir.join(name) });
        }
        for frame in 0..6000 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
            if !app.world().resource::<Tiles>().busy() {
                eprintln!("three tiles after {frame} frames");
                break;
            }
        }
        let read = |name: &str| std::fs::read(dir.join(name)).unwrap_or_else(|_| panic!("{name} was not drawn"));
        let (a, b, c) = (read("a.png"), read("b.png"), read("c.png"));
        assert_eq!(a, b, "the same design draws the same tile");
        assert_ne!(a, c, "another design draws another tile");
        let tile = image::load_from_memory(&a).unwrap().to_rgba8();
        assert_eq!((tile.width(), tile.height()), (SIZE, SIZE));
        let drawn = tile.pixels().filter(|p| p.0[3] > 0).count();
        assert!(drawn > 1500 && drawn < (SIZE * SIZE) as usize, "the tee fills the tile and not all of it: {drawn} px");
        let reddish = tile.pixels().filter(|p| p.0[3] > 200 && p.0[0] > 150 && p.0[1] < 90 && p.0[2] < 90).count();
        assert!(reddish > 60, "the red design shows on the tee: {reddish} px");
        // The player's outfit and what other players are sent were never touched.
        let after = (app.world().resource::<Customiser>().draft.clone(), app.world().resource::<Parts>().applied.clone());
        assert_eq!(after, before);
        if let Ok(keep) = std::env::var("SKATE_TILE_PNG") {
            std::fs::create_dir_all(&keep).unwrap();
            for name in ["a.png", "c.png"] {
                std::fs::copy(dir.join(name), Path::new(&keep).join(name)).unwrap();
            }
        }
        app.world_mut().write_message(AppExit::Success);
        let _ = std::fs::remove_dir_all(dir);
    }
}
