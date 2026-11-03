//! Original APT HUD rendered independently of the world's resolution scale.
use crate::{apt_scene, config::Config, hud_runtime, physics::SkaterRuntime};
use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
    camera::{RenderTarget, ScalingMode, visibility::RenderLayers},
    prelude::*,
    render::render_resource::{
        AsBindGroup, BlendState, Extent3d, RenderPipelineDescriptor, ShaderType, TextureDimension,
        TextureFormat,
    },
    shader::ShaderRef,
    sprite_render::{AlphaMode2d, Material2d, Material2dPlugin},
    ui_render::UiMaterialPlugin,
    window::PrimaryWindow,
};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, ShaderType)]
pub(crate) struct ColorTransform {
    pub multiply: Vec4,
    pub add: Vec4,
}
/// Shared by every original APT overlay (scoring HUD, Start-button crossbar).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct HudMaterial {
    #[uniform(0)]
    pub color: ColorTransform,
    #[texture(1)]
    #[sampler(2)]
    pub atlas: Handle<Image>,
}
impl Material2d for HudMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://skate3rust/hud_render.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}
/// The offscreen target already contains RGB multiplied by coverage. Applying
/// ImageNode's straight-alpha blend again suppresses the original soft glow.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct HudComposite {
    #[texture(0)]
    #[sampler(1)]
    pub image: Handle<Image>,
}
impl UiMaterial for HudComposite {
    fn fragment_shader() -> ShaderRef {
        "embedded://skate3rust/hud_composite.wgsl".into()
    }
    fn specialize(descriptor: &mut RenderPipelineDescriptor, _: UiMaterialKey<Self>) {
        if let Some(fragment) = &mut descriptor.fragment {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING);
            }
        }
    }
}
#[derive(Resource)]
struct Hud {
    target: Handle<Image>,
    composite: Handle<HudComposite>,
    /// Offscreen camera and the full-screen node compositing its target.
    camera: Entity,
    overlay: Entity,
    rebind_after_resize: bool,
    runtime: hud_runtime::Runtime,
    source: serde_json::Value,
    shapes: apt_scene::Shapes,
    textures: BTreeMap<String, Handle<Image>>,
    slots: crate::apt_canvas::Slots,
    generation: u64,
    failed: bool,
    /// perf 10-18: the draw key of what the slots hold.
    drawn: Option<u64>,
    /// perf 10-18: the script stopped while another game's HUD shows; `reset` reloads it when the Skate 3 one is back.
    idle: bool,
}
/// SKATE_HUD_IDLE_ADVANCE=1: the Skate 3 HUD's script runs every tick even while another game's HUD replaces it.
fn idle_advance() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SKATE_HUD_IDLE_ADVANCE").is_ok_and(|v| v == "1"))
}
pub(crate) struct ScoringHudPlugin;
impl Plugin for ScoringHudPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "hud_render.wgsl");
        embedded_asset!(app, "hud_composite.wgsl");
        app.add_plugins((
            Material2dPlugin::<HudMaterial>::default(),
            UiMaterialPlugin::<HudComposite>::default(),
        ))
        .add_systems(
            PostStartup,
            setup.after(crate::graphics_menu::PresentationSetup),
        )
        .add_systems(
            FixedUpdate,
            (advance, multiplier_sounds)
                .after(crate::app::SimulationSet::Physics)
                .run_if(crate::graphics_menu::gameplay_active),
        )
        .add_systems(
            Update,
            (resize_target, reset, render)
                .chain()
                .after(crate::app::FrameSet::Physics),
        );
    }
}
fn root(asset_root: &std::path::Path) -> PathBuf {
    std::env::var_os("SKATE_SCORING_HUD_ROOT").map(PathBuf::from).unwrap_or_else(|| asset_root.join("private/hud"))
}
/// What setup reads, for the startup prefetch (apt_canvas::prefetch).
pub(crate) fn manifest(asset_root: &std::path::Path) -> PathBuf {
    root(asset_root).join("runtime/trickdisplay.json")
}
fn setup(
    mut commands: Commands,
    config: Res<Config>,
    skater: Res<SkaterRuntime>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    // The startup dependency also applies the presentation system's deferred
    // camera spawn before this query. Without it, setup silently lost the HUD.
    let Ok(output) = cameras.single() else {
        error!("Original HUD requires the presentation camera");
        return;
    };
    let (root, file) = (root(&config.asset_root), manifest(&config.asset_root));
    let result = (|| -> Result<Hud, String> {
        // perf 10-18: main() prefetches it.
        let source: serde_json::Value = match crate::apt_canvas::prefetched_json(&file) {
            Some(json) => json,
            None => serde_json::from_slice(&std::fs::read(&file).map_err(|e| format!("{}: {e}", root.display()))?)
                .map_err(|e| e.to_string())?,
        };
        let runtime = hud_runtime::Runtime::load(&source, skater.scoring.hud_input())?;
        let shapes: apt_scene::Shapes =
            serde::Deserialize::deserialize(&source["shapes"]).map_err(|e| e.to_string())?;
        let mut files = BTreeMap::new();
        for shape in shapes.values().flatten() {
            files.insert(
                shape.texture.rgba.clone(),
                [shape.texture.width, shape.texture.height],
            );
        }
        for font in runtime.bindings.movie.text_assets.fonts.values() {
            files.insert(font.texture.clone(), font.size);
        }
        let mut textures = BTreeMap::new();
        for (path, size) in files {
            let bytes = std::fs::read(root.join(&path)).map_err(|e| format!("HUD {path}: {e}"))?;
            if bytes.len() != size[0] as usize * size[1] as usize * 4 {
                return Err(format!("Invalid HUD texture size {path}"));
            }
            let image = Image::new(
                Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                bytes,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            );
            textures.insert(path, images.add(image));
        }
        Ok(Hud {
            target: Handle::default(),
            composite: Handle::default(),
            camera: Entity::PLACEHOLDER,
            overlay: Entity::PLACEHOLDER,
            rebind_after_resize: false,
            runtime,
            source,
            shapes,
            textures,
            slots: default(),
            generation: 0,
            failed: false,
            drawn: None,
            idle: false,
        })
    })();
    match result {
        Ok(mut hud) => {
            let target = images.add(Image::new_target_texture(
                window.physical_width().max(1),
                window.physical_height().max(1),
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
            hud.target = target.clone();
            hud.camera = commands
                .spawn((
                    Camera2d,
                    Projection::Orthographic(OrthographicProjection {
                        scaling_mode: ScalingMode::AutoMin {
                            min_width: 1280.,
                            min_height: 720.,
                        },
                        ..OrthographicProjection::default_2d()
                    }),
                    Camera {
                        order: -1,
                        clear_color: ClearColorConfig::Custom(Color::NONE),
                        ..default()
                    },
                    RenderTarget::Image(target.clone().into()),
                    RenderLayers::layer(31),
                    Msaa::Off,
                ))
                .id();
            hud.composite = composites.add(HudComposite { image: target });
            hud.overlay = commands
                .spawn((
                    MaterialNode(hud.composite.clone()),
                    UiTargetCamera(output),
                    GlobalZIndex(1),
                    Pickable::IGNORE,
                    Node {
                        position_type: PositionType::Absolute,
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                ))
                .id();
            commands.insert_resource(hud);
            info!("Original scoring HUD loaded from {}", root.display());
        }
        Err(error) => error!(
            "Original scoring HUD could not load from {}: {error}. See docs/hud-installation.md",
            root.display()
        ),
    }
}
// Rasterize at output pixel resolution; retain the original 1280x720 APT
// coordinate space. This avoids a second enlargement of every glyph/glow.
fn resize_target(
    hud: Option<ResMut<Hud>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    let Some(mut hud) = hud else {
        return;
    };
    // Bevy 0.18's UI material preparation has no dependency on GpuImage
    // preparation. Retry once on the following frame, when the replacement
    // image is available regardless of preparation order during the resize.
    if hud.rebind_after_resize {
        if let Some(material) = composites.get_mut(&hud.composite) {
            material.image = hud.target.clone();
        }
    }
    let size = Extent3d {
        width: window.physical_width().max(1),
        height: window.physical_height().max(1),
        depth_or_array_layers: 1,
    };
    hud.rebind_after_resize = resize_image(
        &mut images,
        &mut composites,
        &hud.target,
        &hud.composite,
        size,
    );
}
fn resize_image(
    images: &mut Assets<Image>,
    composites: &mut Assets<HudComposite>,
    target: &Handle<Image>,
    composite: &Handle<HudComposite>,
    size: Extent3d,
) -> bool {
    if images
        .get(target)
        .is_some_and(|image| image.texture_descriptor.size != size)
    {
        // Assets::get_mut emits Modified even without a write. Doing that
        // every frame recreates the GPU target behind the compositor's
        // cached bind group. Only invalidate the image on a real resize.
        if let Some(image) = images.get_mut(target) {
            image.resize(size);
        }
        // The UI material retains its bind group. A real target resize must
        // reprepare it so it samples the replacement GPU texture view.
        if let Some(material) = composites.get_mut(composite) {
            material.image = target.clone();
        }
        return true;
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_target_changes_only_on_resize_and_refreshes_composite() {
        // Asset scheduling only: no render plugin, window or gameplay systems.
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<HudComposite>();
        let target =
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(
                    1280,
                    720,
                    TextureFormat::Rgba8UnormSrgb,
                    None,
                ));
        let composite = app
            .world_mut()
            .resource_mut::<Assets<HudComposite>>()
            .add(HudComposite {
                image: target.clone(),
            });
        app.update();
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<Image>>>()
            .clear();
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<HudComposite>>>()
            .clear();
        for (width, height, changed) in
            [(1280, 720, false), (1920, 1080, true), (1920, 1080, false)]
        {
            app.world_mut()
                .resource_scope(|world, mut images: Mut<Assets<Image>>| {
                    let mut composites = world.resource_mut::<Assets<HudComposite>>();
                    resize_image(
                        &mut images,
                        &mut composites,
                        &target,
                        &composite,
                        Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                    );
                });
            app.update();
            let images: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<AssetEvent<Image>>>()
                .drain()
                .collect();
            let materials: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<AssetEvent<HudComposite>>>()
                .drain()
                .collect();
            assert_eq!(
                images
                    .iter()
                    .any(|e| matches!(e, AssetEvent::Modified { id } if *id == target.id())),
                changed
            );
            assert_eq!(
                materials
                    .iter()
                    .any(|e| matches!(e, AssetEvent::Modified { id } if *id == composite.id())),
                changed
            );
        }
    }

    /// Test build 1 (user 10-21): switching from Skate 3 to Skate 2 physics mid-trick left the Skate 3 trick on
    /// screen. Not native Skate 2 (user 10-20): the switch restarts the trick display's script, as a new map does.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the install's assets, read only: the HUD movie and the stock skater)"]
    fn a_physics_switch_clears_the_trick_display() {
        use crate::skate2_physics::Skate2Physics;
        use crate::skate2_physics::sim::{HostState, Link};
        let assets = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
        let mut app = crate::skate2_physics::step::tests::harness();
        let world = app.world_mut();
        let source: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest(&assets)).unwrap()).unwrap();
        let shapes = || -> apt_scene::Shapes { serde::Deserialize::deserialize(&source["shapes"]).unwrap() };
        let input = world.resource::<SkaterRuntime>().scoring.hud_input();
        let runtime = hud_runtime::Runtime::load(&source, input).unwrap();
        let blank = apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes()).unwrap();
        world.insert_resource(Hud {
            target: Handle::default(),
            composite: Handle::default(),
            camera: Entity::PLACEHOLDER,
            overlay: Entity::PLACEHOLDER,
            rebind_after_resize: false,
            runtime,
            source: source.clone(),
            shapes: shapes(),
            textures: BTreeMap::new(),
            slots: default(),
            generation: 0,
            failed: false,
            drawn: None,
            idle: false,
        });
        world.insert_resource(crate::map_transition::CurrentMap {
            path: None,
            name: "Test world".into(),
            spawn: [0.0; 3],
            heading: 0.0,
            generation: 0,
        });
        let drawn = |world: &World| {
            let hud = world.resource::<Hud>();
            apt_scene::draw(&hud.runtime.bindings.movie, &hud.runtime.vm, &hud.shapes).unwrap()
        };
        // Skate 3 physics, mid-trick: a trick on the display (named through the port's adapter, which needs no sim).
        let trick = skate2_physics_protocol::Tricks { begun: vec![2], multiplier: 1.0, ..Default::default() };
        let still = skate2_physics_protocol::Tricks { multiplier: 1.0, ..Default::default() };
        world.resource_mut::<SkaterRuntime>().scoring.apply_port_frame(&trick);
        for n in 0..30 {
            world.run_system_cached(advance).unwrap();
            world.run_system_cached(reset).unwrap();
            if n == 0 {
                world.resource_mut::<SkaterRuntime>().scoring.apply_port_frame(&still);
            }
        }
        assert_ne!(drawn(world), blank, "the trick shows");
        // The switch: Skate 2 drives, and the engine's scorer starts over (switch_on).
        let mut skater = world.resource_mut::<SkaterRuntime>();
        skater.scoring = skater.scoring.fresh();
        world.resource_mut::<Skate2Physics>().state =
            HostState::Driving(Link::from_channels(std::sync::mpsc::channel().0, std::sync::mpsc::channel().1));
        world.run_system_cached(reset).unwrap();
        assert_eq!(drawn(world), blank, "the switch cleared the display");
        // And Skate 2's tricks show on it.
        world.resource_mut::<SkaterRuntime>().scoring.apply_port_frame(&trick);
        for n in 0..30 {
            world.run_system_cached(advance).unwrap();
            world.run_system_cached(reset).unwrap();
            if n == 0 {
                world.resource_mut::<SkaterRuntime>().scoring.apply_port_frame(&still);
            }
        }
        assert_ne!(drawn(world), blank, "the port's trick shows under the switch");
    }
}
fn reset(
    hud: Option<ResMut<Hud>>,
    skater: Res<SkaterRuntime>,
    map: Res<crate::map_transition::CurrentMap>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    skate2: Option<Res<crate::skate2_physics::Skate2Physics>>,
    mut drove: Local<bool>,
) {
    let Some(mut hud) = hud else {
        return;
    };
    // An idle script restarts as on a new map, before the HUD draws again.
    let woken = hud.idle && crate::skate2_hud::hud_game(menu.as_deref(), &map) == 3;
    // Not native Skate 2 (user 10-20): a switch between Skate 3 and Skate 2 physics, either way, restarts the
    // script too, so a trick the other physics named (mid-trick at the switch) does not stay on screen.
    // Not tested in play.
    let drives = skate2_drives(skate2.as_deref());
    let switched = std::mem::replace(&mut *drove, drives) != drives;
    if hud.generation == map.generation && !woken && !switched {
        return;
    }
    match hud_runtime::Runtime::load(&hud.source, skater.scoring.hud_input()) {
        Ok(runtime) => {
            hud.runtime = runtime;
            hud.generation = map.generation;
            hud.failed = false;
            hud.idle = false;
        }
        Err(error) => {
            error!("Original scoring HUD reset: {error}");
            hud.failed = true;
        }
    }
}
fn skate2_drives(skate2: Option<&crate::skate2_physics::Skate2Physics>) -> bool {
    skate2.is_some_and(|s| s.drives_local())
}
fn advance(
    hud: Option<ResMut<Hud>>,
    skater: Res<SkaterRuntime>,
    map: Res<crate::map_transition::CurrentMap>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    let Some(mut hud) = hud else {
        return;
    };
    if hud.failed || hud.generation != map.generation {
        return;
    }
    // perf 10-18: render draws nothing while another game's HUD replaces this one (Skate 1/2 maps, HUD Style), so
    // its script needn't run; it waits for `reset`'s reload once it shows again.
    if !idle_advance() && (hud.idle || crate::skate2_hud::hud_game(menu.as_deref(), &map) != 3) {
        hud.idle = true;
        return;
    }
    if let Err(error) = hud.runtime.update(
        skater.scoring.hud_input(),
        skater.scoring.new_trick,
        skater.scoring.modified_trick,
        skater.scoring.close_tricks,
    ) {
        error!("Original scoring HUD stopped: {error}");
        hud.failed = true;
    }
}
fn render(
    mut commands: Commands,
    hud: Option<ResMut<Hud>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    map: Res<crate::map_transition::CurrentMap>,
    matched: Option<Res<crate::match_core::Match>>,
    mut cameras: Query<&mut Camera>,
    mut visibility: Query<&mut Visibility>,
    replay: Option<Res<crate::replay::hud::ReplayHud>>,
) {
    let Some(mut hud) = hud else {
        return;
    };
    // Trick text must not draw over the Edit Skater screen; Game Settings > HUD hides it, and Skate 2's
    // own trick display replaces it on the Skate 2 maps, and Skate 1's on its map (Settings > HUD Style). A match's
    // mode hides it while its Lua has HUD2.Show(TrickDisplayHUD, false) in force. Retail shows none over the Replay
    // Editor (user 10-27).
    let hidden = customiser.is_some_and(|c| c.open)
        || replay.is_some_and(|r| r.open())
        // A mode's intro cutscene shows none either.
        || matched.is_some_and(|m| {
            m.trick_display_hidden && m.phase != crate::match_core::Phase::Idle || matches!(m.phase, crate::match_core::Phase::Intro { .. })
        })
        || menu.as_deref().is_some_and(|m| !m.hud_visible())
        || crate::skate2_hud::hud_game(menu.as_deref(), &map) != 3;
    // perf 10-18: last frame's slots stand while nothing they show changed.
    let key = apt_scene::hash_of((apt_scene::draw_key(&hud.runtime.bindings.movie, &hud.runtime.vm), hud.failed || hidden));
    if hud.drawn == Some(key) && !apt_scene::redraw_always() {
        return;
    }
    hud.drawn = Some(key);
    let draws = if hud.failed || hidden {
        Vec::new()
    } else {
        match apt_scene::draw(&hud.runtime.bindings.movie, &hud.runtime.vm, &hud.shapes) {
            Ok(draws) => draws,
            Err(error) => {
                error!("Original HUD geometry: {error}");
                hud.failed = true;
                Vec::new()
            }
        }
    };
    let hud = &mut *hud;
    let textured: Vec<_> = draws
        .iter()
        .filter_map(|d| Some((d, hud.textures.get(&d.texture)?.clone())))
        .collect();
    // No trick text: skip the offscreen pass and its full-screen composite. The
    // overlay only reappears on a frame whose camera (order -1) redraws it first.
    let active = !textured.is_empty();
    if let Ok(mut camera) = cameras.get_mut(hud.camera)
        && camera.is_active != active
    {
        camera.is_active = active;
    }
    if let Ok(mut shown) = visibility.get_mut(hud.overlay) {
        shown.set_if_neq(if active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    hud.slots
        .draw(textured, 31, &mut commands, &mut meshes, &mut materials);
}

/// Retail plays the score multiplier's sounds from this module, not from the ScoreModule:
/// `sub_82666BC0` polls the published sequence multiplier against its own cached copy and, on
/// any change, picks a sound by exact float equality. The scoring runtime keeps the cache and
/// reports the change edge; the selection lives with the sounds.
///
/// This runs with the simulation rather than with the frame so a change cannot be missed when
/// several fixed ticks fall inside one rendered frame. It deliberately does not depend on the
/// HUD resource: the sound is retail's whether or not the display itself loaded. The exception is
/// Skate 2's trick display, whose own UpdateMultiplier plays Skate 2's sounds (skate2_hud).
fn multiplier_sounds(
    skater: Res<SkaterRuntime>,
    mut sounds: MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
    skate2_hud: Option<Res<crate::skate2_hud::Skate2Hud>>,
) {
    let Some(multiplier) = skater.scoring.multiplier_changed else {
        return;
    };
    if skate2_hud.is_some_and(|hud| hud.active()) {
        return;
    }
    if let Some(id) = crate::skate_audio::player::multiplier_sound_id(multiplier) {
        sounds.write(crate::skate_audio::FrontEndSoundRequest(id));
    }
}
