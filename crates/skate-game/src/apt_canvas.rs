//! Offscreen presentation of an original APT screen: a 1280x720 orthographic
//! camera on its own render layer draws the movie's triangles into a window-sized
//! target, composited by a UI node. Textures load on first use, so screens with
//! large optional image sets (part thumbnails) only pay for what they show.
use crate::{
    apt_scene::{Draw, Vertex},
    scoring_hud::{ColorTransform, HudComposite, HudMaterial},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::{RenderTarget, ScalingMode, visibility::RenderLayers},
    prelude::*,
    render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
    sprite_render::MeshMaterial2d,
};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

struct Slot {
    entity: Entity,
    mesh: Handle<Mesh>,
    material: Handle<HudMaterial>,
    /// What the mesh and material currently hold: vertices, multiply, add, texture.
    drawn: (Vec<Vertex>, [f32; 4], [f32; 4], Handle<Image>),
}
/// One reusable mesh/material entity per APT draw (scoring HUD and every screen).
#[derive(Default)]
pub(crate) struct Slots {
    slots: Vec<Slot>,
    /// Slots drawn last frame.
    visible: usize,
}
impl Slots {
    /// Replace last frame's triangles (movie coordinates, 1280x720) on `layer`.
    /// Assets::get_mut re-uploads a mesh and rebuilds a material's bind group, so
    /// a slot's assets are only touched when its draw differs from last frame.
    pub fn draw<'a>(
        &mut self,
        draws: impl IntoIterator<Item = (&'a Draw, Handle<Image>)>,
        layer: usize,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<HudMaterial>,
    ) {
        let mut used = 0;
        for (draw, texture) in draws {
            let extended;
            let draw = match reach_window_edges(draw) {
                Some(d) => {
                    extended = d;
                    &extended
                }
                None => draw,
            };
            // The mesh and the material change apart: a minimap turning with the skater moves only vertices, and a
            // material get_mut would rebuild its bind group every frame for nothing (perf 10-10 #6).
            let (same_mesh, same_material) = self.slots.get(used).map_or((false, false), |s| {
                (s.drawn.0 == draw.vertices, s.drawn.1 == draw.multiply && s.drawn.2 == draw.add && s.drawn.3 == texture)
            });
            if !(same_mesh && same_material) {
                let mesh = || {
                    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, draw.vertices.iter().map(|v| [v.position[0] - 640., 360. - v.position[1], 0.]).collect::<Vec<_>>());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, draw.vertices.iter().map(|v| v.uv).collect::<Vec<_>>());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 0., 1.]; draw.vertices.len()]);
                    mesh
                };
                let material = || HudMaterial { color: ColorTransform { multiply: draw.multiply.into(), add: draw.add.into() }, atlas: texture.clone() };
                if let Some(slot) = self.slots.get_mut(used) {
                    if !same_mesh {
                        if let Some(old) = meshes.get_mut(&slot.mesh) {
                            *old = mesh();
                        }
                        slot.drawn.0 = draw.vertices.clone();
                    }
                    if !same_material {
                        if let Some(old) = materials.get_mut(&slot.material) {
                            *old = material();
                        }
                        (slot.drawn.1, slot.drawn.2, slot.drawn.3) = (draw.multiply, draw.add, texture.clone());
                    }
                } else {
                    let (mesh, material) = (mesh(), material());
                    let drawn = (draw.vertices.clone(), draw.multiply, draw.add, texture.clone());
                    let mesh = meshes.add(mesh);
                    let material = materials.add(material);
                    let entity = commands
                        .spawn((
                            Mesh2d(mesh.clone()),
                            MeshMaterial2d(material.clone()),
                            Transform::from_xyz(0., 0., used as f32 * 0.01),
                            RenderLayers::layer(layer),
                        ))
                        .id();
                    self.slots.push(Slot { entity, mesh, material, drawn });
                }
            }
            // Slots within last frame's count are already visible.
            if used >= self.visible {
                commands.entity(self.slots[used].entity).insert(Visibility::Visible);
            }
            used += 1;
        }
        // Only slots that were drawn last frame need hiding; a closed screen costs nothing.
        for slot in self.slots.iter().take(self.visible).skip(used) {
            commands.entity(slot.entity).insert(Visibility::Hidden);
        }
        self.visible = used;
    }
}
/// A solid-colour fill (1x1 texture) covering the whole 1280x720 stage, with its stage-edge vertices pushed
/// far past it: the stage keeps 16:9 on any window, and a full-screen fade or dimmer (sk8popup's black
/// shape 1) must also cover the letterbox margins around it, as it covers the whole screen on retail.
fn reach_window_edges(draw: &Draw) -> Option<Draw> {
    const STAGE: [f32; 2] = [1280., 720.];
    const PAST: f32 = 4096.;
    // Half a stage unit of slack: the fill's corners went through the APT matrices, so 1280 can land as 1279.9999.
    const EDGE: f32 = 0.5;
    if draw.size != [1, 1] || draw.vertices.is_empty() {
        return None;
    }
    let covers = (0..2).all(|i| {
        let (lo, hi) = draw.vertices.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v.position[i]), hi.max(v.position[i])));
        lo <= EDGE && hi >= STAGE[i] - EDGE
    });
    covers.then(|| Draw {
        vertices: draw
            .vertices
            .iter()
            .map(|v| Vertex {
                position: std::array::from_fn(|i| match v.position[i] {
                    p if p <= EDGE => -PAST,
                    p if p >= STAGE[i] - EDGE => STAGE[i] + PAST,
                    p => p,
                }),
                uv: v.uv,
            })
            .collect(),
        ..draw.clone()
    })
}
/// System-font glyph textures one canvas keeps (~16 KB of GPU memory each).
const GLYPH_TEXTURES: usize = 1024;
/// C1: system glyphs (other players' text) hold at most [`GLYPH_TEXTURES`] of a canvas's textures; loading one more
/// drops them all, to reload as drawn.
// ponytail: clear-all, not LRU; a screen drawing more distinct glyphs than that at once reloads them each frame.
fn make_room<T>(textures: &mut BTreeMap<String, T>, path: &str) {
    use crate::text_fallback::is_glyph;
    if is_glyph(path) && textures.keys().filter(|k| is_glyph(k)).count() >= GLYPH_TEXTURES {
        textures.retain(|k, _| !is_glyph(k));
    }
}
pub(crate) struct Canvas {
    root: PathBuf,
    layer: usize,
    textures: BTreeMap<String, Option<Handle<Image>>>,
    pub target: Handle<Image>,
    pub composite: Handle<HudComposite>,
    camera: Entity,
    active: bool,
    rebind_after_resize: bool,
    slots: Slots,
    /// perf 10-18: the key of what the slots hold ([`Self::drawn_as`]); None after any other change.
    drawn: Option<u64>,
}
impl Canvas {
    pub fn new(
        commands: &mut Commands,
        images: &mut Assets<Image>,
        composites: &mut Assets<HudComposite>,
        window: &Window,
        root: PathBuf,
        layer: usize,
        order: isize,
    ) -> Self {
        let mut image = Image::new_target_texture(
            window.physical_width().max(1),
            window.physical_height().max(1),
            TextureFormat::Rgba8UnormSrgb,
            None,
        );
        // Perf 10-18: the GPU draws it; a zeroed CPU copy only cost RAM (~8 MB each at 1080p, 22+ canvases) and its
        // upload at startup and on every resize.
        image.data = None;
        // new_target_texture asks for a GPU copy on resize, but the texture lacks COPY_SRC: with data it was never
        // taken (a resize re-uploaded zeros); without data wgpu rejects it. Redrawn each frame, so no copy.
        image.copy_on_resize = false;
        let target = images.add(image);
        let camera = commands
            .spawn((
                Camera2d,
                Projection::Orthographic(OrthographicProjection {
                    // AutoMin keeps the 16:9 stage undistorted on any window shape (Fixed stretched it).
                    scaling_mode: ScalingMode::AutoMin { min_width: 1280., min_height: 720. },
                    ..OrthographicProjection::default_2d()
                }),
                // Inactive until shown: no GPU cost during gameplay.
                Camera { order, is_active: false, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
                RenderTarget::Image(target.clone().into()),
                RenderLayers::layer(layer),
                Msaa::Off,
            ))
            .id();
        let composite = composites.add(HudComposite { image: target.clone() });
        Self {
            root,
            layer,
            textures: BTreeMap::new(),
            target,
            composite,
            camera,
            active: false,
            rebind_after_resize: false,
            slots: Slots::default(),
            drawn: None,
        }
    }
    /// Scale the stage to cover the whole window (cropping its overflow) instead of fitting inside it:
    /// full-screen art such as the loading screen, which retail shows edge to edge.
    pub fn cover(&self, commands: &mut Commands) {
        commands.entity(self.camera).insert(Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::AutoMax { max_width: 1280., max_height: 720. },
            ..OrthographicProjection::default_2d()
        }));
    }
    /// Eagerly load (and validate) a texture, e.g. at setup.
    pub fn preload(&mut self, images: &mut Assets<Image>, path: &str, size: [u32; 2]) -> Result<Handle<Image>, String> {
        let (bytes, size) = texture_bytes(&self.root, path, size)?;
        if size[0] == 0 || size[1] == 0 || bytes.len() != size[0] as usize * size[1] as usize * 4 {
            return Err(format!("Invalid texture size {path}"));
        }
        let handle = images.add(Image::new(
            Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            TextureDimension::D2,
            bytes,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        ));
        self.textures.insert(path.into(), Some(handle.clone()));
        self.drawn = None;
        Ok(handle)
    }
    /// A texture the host fills itself (a video frame), drawn wherever a draw names `key`.
    pub fn insert_texture(&mut self, key: &str, handle: Handle<Image>) {
        self.textures.insert(key.into(), Some(handle));
        self.drawn = None;
    }
    fn texture(&mut self, images: &mut Assets<Image>, path: &str, size: [u32; 2]) -> Option<Handle<Image>> {
        if let Some(known) = self.textures.get(path) {
            return known.clone();
        }
        make_room(&mut self.textures, path);
        let loaded = self.preload(images, path, size).map_err(|e| warn!("APT screen texture: {e}")).ok();
        // A bad file is reported once, then skipped.
        self.textures.insert(path.into(), loaded.clone());
        loaded
    }
    pub fn camera(&self) -> Entity {
        self.camera
    }
    /// Turning off, the camera renders one more frame, empty and transparent: an overlay that stays
    /// on screen (the Skate 1/2 HUDs) would otherwise keep showing the last image drawn, over the next map.
    pub fn set_active(&mut self, active: bool, cameras: &mut Query<&mut Camera>) {
        let camera_on = active || self.active;
        self.active = active;
        if let Ok(mut camera) = cameras.get_mut(self.camera) {
            if camera.is_active != camera_on {
                camera.is_active = camera_on;
            }
        }
    }
    pub fn resize(&mut self, window: &Window, images: &mut Assets<Image>, composites: &mut Assets<HudComposite>) {
        if !self.active {
            return;
        }
        // Same deferred rebind as the scoring HUD: UI materials do not wait for GpuImage.
        if self.rebind_after_resize {
            if let Some(material) = composites.get_mut(&self.composite) {
                material.image = self.target.clone();
            }
        }
        let size = Extent3d { width: window.physical_width().max(1), height: window.physical_height().max(1), depth_or_array_layers: 1 };
        self.rebind_after_resize = false;
        if images.get(&self.target).is_some_and(|image| image.texture_descriptor.size != size) {
            if let Some(image) = images.get_mut(&self.target) {
                image.resize(size);
            }
            if let Some(material) = composites.get_mut(&self.composite) {
                material.image = self.target.clone();
            }
            self.rebind_after_resize = true;
            self.drawn = None;
        }
    }
    /// Replace last frame's triangles with `draws` (movie coordinates, 1280x720).
    pub fn draw(
        &mut self,
        draws: &[Draw],
        commands: &mut Commands,
        images: &mut Assets<Image>,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<HudMaterial>,
    ) {
        let textured: Vec<_> = draws.iter().filter_map(|d| Some((d, self.texture(images, &d.texture, d.size)?))).collect();
        self.slots.draw(textured, self.layer, commands, meshes, materials);
        self.drawn = None;
    }
    /// perf 10-18: the slots hold the draws of `key` (apt_scene::draw_key and whatever else the host's draws depend
    /// on): the host can skip building and drawing them again.
    pub fn up_to_date(&self, key: u64) -> bool {
        self.drawn == Some(key) && !crate::apt_scene::redraw_always()
    }
    /// What the [`Self::draw`] just made is `key`'s.
    pub fn drawn_as(&mut self, key: u64) {
        self.drawn = Some(key);
    }
    /// The next [`Self::up_to_date`] is false whatever its key.
    pub fn forget_drawn(&mut self) {
        self.drawn = None;
    }
}

/// On an always-present full-screen node compositing a canvas: names that canvas's camera.
#[derive(Component)]
pub(crate) struct CompositeOf(pub Entity);

/// Hide each such node while its camera is off. Idle canvases otherwise each blend a transparent
/// full-screen texture every frame: 10 of them made the UI pass shade ~11x the screen (PERF_AUDIT_2026-10-04).
pub(crate) fn hide_idle_composites(cameras: Query<&Camera>, mut nodes: Query<(&CompositeOf, &mut Visibility)>) {
    for (of, mut visibility) in &mut nodes {
        let wanted = if cameras.get(of.0).is_ok_and(|c| c.is_active) { Visibility::Inherited } else { Visibility::Hidden };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Every texture the movie itself references (shapes and fonts), with sizes.
/// A texture's RGBA8 pixels and size, from the startup prefetch or else the disk.
/// A 1x1 white texture made here, not read: a draw of it is a plain fill in its `multiply` colour (the chat box).
pub(crate) const SOLID: &str = "s3r_solid.rgba";
pub(crate) fn texture_bytes(root: &Path, path: &str, size: [u32; 2]) -> Result<(Vec<u8>, [u32; 2]), String> {
    if path.rsplit(['/', '\\']).next() == Some(SOLID) {
        return Ok((vec![255; 4], [1, 1]));
    }
    if let Some(art) = crate::button_icons::replace(path, size) {
        return Ok((art.to_vec(), size));
    }
    if let Some(glyph) = crate::text_fallback::pixels(path) {
        return glyph.map(|px| (px, size));
    }
    match prefetched(|store| store.textures.remove(&root.join(path))) {
        Some(read) => Ok(read),
        None => read_texture(root, path, size),
    }
}
/// A texture file's RGBA8 pixels and size: a PNG decoded, anything else raw at the size its manifest gives.
fn read_texture(root: &Path, path: &str, size: [u32; 2]) -> Result<(Vec<u8>, [u32; 2]), String> {
    let bytes = std::fs::read(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
    Ok(if path.ends_with(".png") {
        let decoded = image::load_from_memory(&bytes).map_err(|e| format!("{path}: {e}"))?.into_rgba8();
        let size = [decoded.width(), decoded.height()];
        (decoded.into_raw(), size)
    } else {
        (bytes, size)
    })
}

/// Perf 10-12: the menus' manifests (parsed) and their textures, read on a thread from launch while main() and the
/// map scene load. Their PostStartup setups take them from here instead of reading the disk before the first frame
/// (~0.35 s warm, ~2.5 s from a cold disk). A file that failed to read or parse isn't kept: its setup reads it again
/// and reports the error as before.
enum Prefetch {
    Idle,
    Running(std::thread::JoinHandle<Prefetched>),
    Ready(Prefetched),
}
#[derive(Default)]
struct Prefetched {
    json: HashMap<PathBuf, serde_json::Value>,
    textures: HashMap<PathBuf, (Vec<u8>, [u32; 2])>,
}
static PREFETCH: Mutex<Prefetch> = Mutex::new(Prefetch::Idle);

/// Reads `manifests` (each `<root>/runtime/<name>.json`) and the textures each draws (relative to its root).
pub(crate) fn prefetch(manifests: Vec<PathBuf>) {
    let job = std::thread::Builder::new().name("menu-prefetch".into()).spawn(move || {
        let mut store = Prefetched::default();
        for path in manifests {
            let Some(json) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
                continue;
            };
            let root = path.parent().and_then(Path::parent).unwrap_or(Path::new(""));
            for (texture, size) in movie_textures(&json).unwrap_or_default() {
                if let Ok(read) = read_texture(root, &texture, size) {
                    store.textures.insert(root.join(texture), read);
                }
            }
            store.json.insert(path, json);
        }
        store
    });
    if let Ok(job) = job {
        *PREFETCH.lock().unwrap_or_else(PoisonError::into_inner) = Prefetch::Running(job);
    }
}
/// `take` on the prefetched files, once the prefetch has finished (waiting for it). None without one.
fn prefetched<T>(take: impl FnOnce(&mut Prefetched) -> Option<T>) -> Option<T> {
    let mut prefetch = PREFETCH.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(*prefetch, Prefetch::Running(_)) {
        let Prefetch::Running(job) = std::mem::replace(&mut *prefetch, Prefetch::Idle) else { unreachable!() };
        if let Ok(store) = job.join() {
            *prefetch = Prefetch::Ready(store);
        }
    }
    match &mut *prefetch {
        Prefetch::Ready(store) => take(store),
        _ => None,
    }
}
/// The parsed manifest at `path`, if the prefetch read it.
pub(crate) fn prefetched_json(path: &Path) -> Option<serde_json::Value> {
    prefetched(|store| store.json.remove(path))
}
/// After the menus' setups: frees what none of them took (a running prefetch finishes and frees on its own).
pub(crate) fn drop_prefetch() {
    *PREFETCH.lock().unwrap_or_else(PoisonError::into_inner) = Prefetch::Idle;
}

pub(crate) fn movie_textures(source: &serde_json::Value) -> Result<BTreeMap<String, [u32; 2]>, String> {
    let size = |t: &serde_json::Value| [t["width"].as_u64().unwrap_or(0) as u32, t["height"].as_u64().unwrap_or(0) as u32];
    let mut files = BTreeMap::new();
    for shape in source["shapes"].as_object().ok_or("Screen shapes missing")?.values() {
        for primitive in shape.as_array().ok_or("Invalid screen shape")? {
            let t = &primitive["texture"];
            files.insert(t["rgba"].as_str().ok_or("Shape texture")?.to_owned(), size(t));
        }
    }
    for font in source["fonts"].as_object().ok_or("Screen fonts missing")?.values() {
        files.insert(font["texture"].as_str().ok_or("Font texture")?.to_owned(), size(&font["definition"]["textures"][0]));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[derive(Resource)]
    struct Frame(Slots, Vec<Draw>, Handle<Image>);

    /// C1: a canvas keeps a bounded number of system-glyph textures; the movie's own stay; a glyph name that this
    /// session never drew is an error, never a disk read.
    #[test]
    fn glyph_textures_are_bounded() {
        let glyph = |i: usize| std::env::temp_dir().join("skate3-glyphs").join(format!("{i:x}.rgba")).to_string_lossy().into_owned();
        let mut textures: BTreeMap<String, ()> = BTreeMap::from([("runtime/futura.rgba".into(), ())]);
        for i in 0..GLYPH_TEXTURES * 3 {
            make_room(&mut textures, &glyph(i));
            textures.insert(glyph(i), ());
            assert!(textures.len() <= GLYPH_TEXTURES + 1);
        }
        assert!(textures.contains_key("runtime/futura.rgba"));
        assert!(texture_bytes(Path::new("."), &glyph(0x10FFFB), [4, 4]).is_err());
    }

    #[test]
    fn idle_canvas_composites_are_hidden() {
        let mut world = World::new();
        let idle = world.spawn(Camera { is_active: false, ..default() }).id();
        let live = world.spawn(Camera::default()).id();
        let a = world.spawn((CompositeOf(idle), Visibility::Inherited)).id();
        let b = world.spawn((CompositeOf(live), Visibility::Hidden)).id();
        world.run_system_once(hide_idle_composites).unwrap();
        assert_eq!(world.get::<Visibility>(a), Some(&Visibility::Hidden));
        assert_eq!(world.get::<Visibility>(b), Some(&Visibility::Inherited));
    }

    #[test]
    fn full_stage_fills_reach_the_window_edges() {
        let quad = |size, [x0, y0, x1, y1]: [f32; 4]| Draw {
            texture: String::new(), size, multiply: [1.; 4], add: [0.; 4],
            vertices: [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]].map(|position| Vertex { position, uv: [0., 0.] }).to_vec(),
        };
        let wide = reach_window_edges(&quad([1, 1], [0., 0., 1280., 720.])).unwrap();
        assert_eq!(wide.vertices[2].position, [1280. + 4096., 720. + 4096.]);
        assert_eq!(wide.vertices[5].position, [-4096., 720. + 4096.]);
        // Matrix rounding near the stage edges still counts as full-stage.
        let rounded = reach_window_edges(&quad([1, 1], [0.1, 0.1, 1279.9, 719.9])).unwrap();
        assert_eq!(rounded.vertices[2].position, [1280. + 4096., 720. + 4096.]);
        // Partial fills and textured full-stage art stay on the stage.
        assert!(reach_window_edges(&quad([1, 1], [0., 0., 640., 720.])).is_none());
        assert!(reach_window_edges(&quad([1280, 720], [0., 0., 1280., 720.])).is_none());
    }

    #[test]
    fn only_changed_draws_touch_slot_assets() {
        // Asset scheduling only: no render plugin or window.
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<HudMaterial>();
        let texture = app.world_mut().resource_mut::<Assets<Image>>().add(Image::default());
        app.world_mut().insert_resource(Frame(Slots::default(), Vec::new(), texture));
        let draw = |x: f32| Draw {
            texture: String::new(), size: [1, 1], multiply: [1.; 4], add: [0.; 4],
            vertices: vec![Vertex { position: [x, 0.], uv: [0., 0.] }; 3],
        };
        // Modified (mesh, material) events caused by drawing `draws` for one frame.
        let modified = |app: &mut App, draws: Vec<Draw>| {
            app.world_mut().resource_mut::<Frame>().1 = draws;
            app.world_mut()
                .run_system_once(|mut frame: ResMut<Frame>, mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<HudMaterial>>| {
                    let Frame(slots, draws, texture) = &mut *frame;
                    slots.draw(draws.iter().map(|d| (d, texture.clone())), 5, &mut commands, &mut meshes, &mut materials);
                })
                .unwrap();
            app.update();
            let world = app.world_mut();
            let meshes = world.resource_mut::<Messages<AssetEvent<Mesh>>>().drain().filter(|e| matches!(e, AssetEvent::Modified { .. })).count();
            let materials = world.resource_mut::<Messages<AssetEvent<HudMaterial>>>().drain().filter(|e| matches!(e, AssetEvent::Modified { .. })).count();
            (meshes, materials)
        };
        modified(&mut app, vec![draw(0.)]);
        assert_eq!(modified(&mut app, vec![draw(0.)]), (0, 0));
        // Moved vertices re-upload the mesh only; a colour change rebuilds only the material (perf 10-10 #6).
        assert_eq!(modified(&mut app, vec![draw(1.)]), (1, 0));
        assert_eq!(modified(&mut app, vec![Draw { multiply: [0.5; 4], ..draw(1.) }]), (0, 1));
        assert_eq!(modified(&mut app, vec![draw(1.)]), (0, 1));
        // Hidden, then shown again unchanged: no re-upload.
        modified(&mut app, Vec::new());
        assert_eq!(modified(&mut app, vec![draw(1.)]), (0, 0));
    }
}
