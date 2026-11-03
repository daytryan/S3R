//! Normal gameplay camera rendering endpoint. Simulation owns the cadence of
//! CameraRuntime::advance after its complete physical/animation publication.
mod collision;
mod graph;
mod graph_conditions;
mod graph_subject;
mod publication;
mod runtime;
mod settings;
mod shake_data;
mod shot_data;
mod stock_names;
mod subject;
mod trajectory;
pub(crate) mod world_query;
pub(crate) use graph_subject::CameraGraphEnvironment;
pub(crate) use publication::{
    CameraAirOutput, CameraAnimationOutput, CameraEventsOutput, CameraGrindOutput,
    CameraOffboardOutput, CameraPreferences, CameraPublicationInputs, CameraStateOutput,
    snapshot as publish_camera_subject,
};
pub(crate) use runtime::{CameraRuntime, SHAKE};
#[cfg(test)]
pub(crate) fn runtime_style_for_test(style: usize) {
    runtime::STYLE.store(style, std::sync::atomic::Ordering::Relaxed);
}
use crate::{app::FrameSet, config::Config};
use bevy::prelude::*;

#[derive(Component)]
pub(crate) struct GameplayCamera;

pub(crate) struct CameraPlugin;
impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn).add_systems(
            Update,
            (
                follow_style,
                present
                    .after(FrameSet::Animation)
                    .before(FrameSet::Verification),
            ),
        );
        if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render.add_systems(bevy::render::Render, pipelines_ready.in_set(bevy::render::RenderSystems::Cleanup));
        }
    }
}
/// Camera Style setting: rebuild the camera from the chosen game's data. The next
/// completed physics tick starts it fresh, as after a map load.
fn follow_style(
    menu: Option<Res<crate::graphics_menu::Menu>>,
    config: Res<Config>,
    runtime: Option<ResMut<CameraRuntime>>,
) {
    let (Some(menu), Some(mut runtime)) = (menu, runtime) else {
        return;
    };
    if runtime.style != menu.camera_style() {
        runtime.switch_style(menu.camera_style(), &config.asset_root);
    }
}
fn spawn(
    mut commands: Commands,
    config: Res<Config>,
    retail: Res<crate::retail_render::RetailScene>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    if let Some(menu) = menu {
        runtime::STYLE.store(menu.camera_style(), std::sync::atomic::Ordering::Relaxed);
    }
    let runtime = CameraRuntime::load(&config.asset_root)
        .unwrap_or_else(|error| panic!("Cannot initialize normal gameplay camera: {error}"));
    commands.insert_resource(runtime);
    let mut camera = commands.spawn((
        GameplayCamera,
        // The retail tone pass reads the depth (Skate 1's depth of field).
        Camera3d {
            depth_texture_usages: (bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT
                | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING)
                .into(),
            ..default()
        },
        Camera {
            is_active: false,
            ..default()
        },
        Transform::default(),
    ));
    if retail.0 {
        camera.insert((
            bevy::render::view::Hdr,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            crate::retail_render::RetailTone::default(),
        ));
    }
}

/// No render pipeline is still compiling (the render world's last frame): a setting that needs new pipelines (MSAA,
/// shadows) draws its meshes only once they are ready, so the world must not be held before then.
static PIPELINES_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn pipelines_ready(cache: Res<bevy::render::render_resource::PipelineCache>) {
    PIPELINES_READY.store(cache.waiting_pipelines().next().is_none(), std::sync::atomic::Ordering::Relaxed);
}
/// Frames the world draws, once the menu holds it still or a setting or the window size changes under it, before its
/// image is kept (pipelined rendering has one in flight).
const FRESH_FRAMES: u32 = 3;
/// Holds the world camera's last image under a still Start menu.
#[derive(Default)]
pub(crate) struct Hold {
    key: (u64, UVec2),
    fresh: u32,
}
impl Hold {
    /// Whether to stop drawing the world: `still` for [`FRESH_FRAMES`] drawn frames since it began or `key` (settings
    /// revision, window size) last changed, with every pipeline compiled.
    fn hold(&mut self, still: bool, key: (u64, UVec2), compiled: bool) -> bool {
        if !still || key != self.key {
            (self.key, self.fresh) = (key, 0);
        }
        if !still {
            return false;
        }
        if self.fresh >= FRESH_FRAMES && compiled {
            return true;
        }
        self.fresh += 1;
        false
    }
}

#[derive(Default)]
pub(crate) struct VehicleCameraBlend {
    active: bool,
    previous: Option<Transform>,
    from: Option<Transform>,
    elapsed: f32,
}

/// The world holds its last image under the Start menu (`crossbar`) only while time is paused (offline): nothing
/// under it moves, and drawing it again each frame was a whole map render for nothing (C1). Online time runs on, and
/// retail blurs each frame's live scene behind the menu (0x827F7270; user 10-27: ours had held the last frame since
/// the blur shook with the moving camera). Screens over the menu that show the live world (the CAS, a replay) draw it.
fn held_still(crossbar: bool, paused: bool, customiser: bool, replay: bool) -> bool {
    crossbar && paused && !customiser && !replay
}

pub(crate) fn present(
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    mut runtime: ResMut<CameraRuntime>,
    windows: Query<&Window>,
    history: Res<crate::presentation::Presentation>,
    time: Res<Time<Fixed>>,
    replay: Res<crate::replay::Replay>,
    virtual_time: Res<Time<Virtual>>,
    mut vehicle_blend: Local<VehicleCameraBlend>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    real: Res<Time<Real>>,
    mut editor_view: Local<Option<(f32, f32, f32)>>,
    mut skate2_view: Local<Option<(Vec3, Vec3, Vec3, f32)>>,
    mut cameras: Query<(&mut Camera, &mut Transform, &mut Projection), With<GameplayCamera>>,
    (menu, creator, replay_hud, skate2): (
        Option<Res<crate::graphics_menu::Menu>>,
        Option<Res<crate::graphic_creator::Creator>>,
        Option<Res<crate::replay::hud::ReplayHud>>,
        Option<Res<crate::cas_room::Skate2Cas>>,
    ),
    mut hold: Local<Hold>,
) {
    if customiser.as_ref().is_none_or(|c| !c.open) {
        *editor_view = None;
        *skate2_view = None;
    }
    if let Ok(window) = windows.single() {
        // A minimised window reports zero height, and 0/0 is NaN. That NaN
        // reaches the field of view, the frame fails the finite check in
        // `runtime`, and the game exits with "Normal gameplay camera produced
        // a non-finite frame" -- so minimising the window killed the running
        // game. Keep the last good ratio instead: there is nothing to draw at
        // zero area anyway, and it is restored the moment the window is.
        let (width, height) = (window.width(), window.height());
        if width > 0. && height > 0. {
            runtime.set_aspect_ratio(width / height);
        }
    }
    let Some((previous, current, alpha)) = history.view(&replay, time.overstep_fraction()) else {
        return;
    };
    let still = held_still(
        menu.as_ref().is_some_and(|m| m.crossbar),
        virtual_time.is_paused(),
        customiser.as_ref().is_some_and(|c| c.open),
        replay.active,
    );
    let key = (menu.as_ref().map_or(0, |m| m.revision), windows.single().map_or(UVec2::ZERO, Window::physical_size));
    let held = hold.hold(still, key, PIPELINES_READY.load(std::sync::atomic::Ordering::Relaxed));
    // Render speed (10-27): an export's own camera draws the same view (replay/hud.rs shows its frames meanwhile).
    let draw = !held && !replay_hud.is_some_and(|h| h.exporting());
    // Field of View setting; the customiser and cutscenes still set their own.
    let fov_offset = menu.as_ref().map_or(0., |m| m.fov_offset());
    for (mut camera, mut transform, mut projection) in &mut cameras {
        *transform = crate::presentation::blend(previous.camera, current.camera, alpha);
        if replay.active {
            if let Some(free) = replay.free_camera {
                *transform = free;
            }
        }
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = (previous.fov + (current.fov - previous.fov) * alpha + fov_offset).max(10_f32.to_radians());
            // Settings > Graphics > Draw Distance: the frustum's far plane (the projection itself is infinite).
            p.far = crate::graphics_menu::draw_distance().1;
            // The Replay Editor's lens (replay/cameras.rs).
            if let Some(fov) = replay.camera_fov.filter(|_| replay.active) {
                p.fov = fov;
            }
        }
        if let Some(customiser) = customiser.as_ref().filter(|c| c.open) {
            // Ease between editor framings (real time: gameplay is paused).
            let target = customiser.preview_camera();
            let eased = editor_view.get_or_insert(target);
            let k = 1. - (-real.delta_secs() * 6.).exp();
            let lerp = |a: f32, b: f32| a + (b - a) * k;
            *eased = (
                lerp(eased.0, target.0),
                lerp(eased.1, target.1),
                lerp(eased.2, target.2),
            );
            let (height, distance, yaw) = *eased;
            let (heading, _, _) = current.root.rotation.to_euler(EulerRot::YXZ);
            // As retail: the camera stays square to the room's back wall, shifted
            // sideways so the skater stands right of the menu, and level except
            // for subjects below 0.45 m (shoes are looked down on). Rotate Skater
            // turns the skater (cas_pose); only the classic panel orbits.
            let orbit = if customiser.original || !customiser.fallback { 0. } else { yaw };
            let basis = current.root.translation;
            let turn = Quat::from_rotation_y(heading + orbit);
            // The Graphic Creator's two cards: the skater stands further right.
            let side = -distance * if creator.as_ref().is_some_and(|c| c.is_open()) { 0.54 } else { 0.265625 };
            let eye = basis + turn * Vec3::new(side, height.max(0.45), distance);
            *transform = Transform::from_translation(eye)
                .looking_at(basis + turn * Vec3::new(side, height, 0.), Vec3::Y);
            if let Projection::Perspective(p) = &mut *projection {
                p.fov = 50_f32.to_radians();
            }
            // A Skate 2 room: Skate 2's own camera for the page (cas_room::Skate2Cas), eased the same way.
            let shot = crate::cas_room::skate2_shot_now(customiser, creator.as_ref().is_some_and(|c| c.is_open()));
            let stem = menu.as_ref().map_or(crate::graphics_menu::ROOMS[0].1, |m| m.edit_skater_room());
            let view = skate2.as_ref()
                .filter(|_| customiser.original || !customiser.fallback)
                .and_then(|cas| cas.view(stem, &current.root, shot));
            if let Some(view) = view {
                let target = (view.eye, view.look, view.up, view.fov);
                let eased = skate2_view.get_or_insert(target);
                *eased = (eased.0.lerp(target.0, k), eased.1.lerp(target.1, k), eased.2.lerp(target.2, k), lerp(eased.3, target.3));
                *transform = Transform::from_translation(eased.0).looking_at(eased.1, eased.2);
                if let Projection::Perspective(p) = &mut *projection {
                    p.fov = eased.3;
                }
            } else {
                *skate2_view = None;
            }
        }
        let vehicle = vehicles.camera();
        let active = vehicle.is_some();
        if active != vehicle_blend.active {
            vehicle_blend.from = vehicle_blend.previous;
            vehicle_blend.elapsed = 0.;
            vehicle_blend.active = active;
        }
        if let Some(pose) = vehicle {
            *transform = pose;
        }
        vehicle_blend.elapsed += virtual_time.delta_secs();
        if let Some(from) = vehicle_blend.from {
            if vehicle_blend.elapsed < 0.5 && from.translation.distance(transform.translation) < 50.
            {
                let t = (vehicle_blend.elapsed / 0.5).clamp(0., 1.);
                *transform = crate::presentation::blend(from, *transform, t * t * (3. - 2. * t));
            } else {
                vehicle_blend.from = None;
            }
        }
        // Vehicle motion is already interpolated with the rider/chassis. A separate
        // follow filter introduces relative motion and makes fixed ticks visible.
        vehicle_blend.previous = Some(*transform);
        if camera.is_active != draw {
            camera.is_active = draw;
        }
    }
}

/// Keep the presentation camera/target/settings while replacing world-specific
/// postprocessing. A procedural scene must not inherit native HDR/tone state.
pub(crate) fn set_world_environment(world: &mut World, retail: bool) {
    let cameras: Vec<_> = world
        .query_filtered::<Entity, With<GameplayCamera>>()
        .iter(world)
        .collect();
    for id in cameras {
        let mut camera = world.entity_mut(id);
        if retail {
            camera.insert((
                bevy::render::view::Hdr,
                bevy::core_pipeline::tonemapping::Tonemapping::None,
                crate::retail_render::RetailTone::default(),
            ));
        } else {
            camera.remove::<(bevy::render::view::Hdr, crate::retail_render::RetailTone)>();
            camera.insert(bevy::core_pipeline::tonemapping::Tonemapping::default());
        }
        camera.get_mut::<Camera>().unwrap().is_active = false;
        *camera.get_mut::<Transform>().unwrap() = Transform::default();
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;
    /// A still menu holds the world after FRESH_FRAMES drawn frames; a setting or resize under it, or a pipeline still
    /// compiling, draws again first; leaving the menu draws at once.
    /// User 10-27 (retail blurs the live scene each frame): the Start menu holds the world still only offline, where
    /// time is paused; online the world keeps drawing behind it.
    #[test]
    fn only_a_paused_world_holds_under_the_start_menu() {
        assert!(held_still(true, true, false, false), "offline: time paused");
        assert!(!held_still(true, false, false, false), "online: the world runs on behind the menu");
        assert!(!held_still(true, true, true, false), "the CAS shows the live world");
        assert!(!held_still(true, true, false, true), "a replay too");
        assert!(!held_still(false, true, false, false), "no Start menu");
    }

    #[test]
    fn the_paused_world_holds_after_fresh_frames_and_redraws_on_changes() {
        let mut hold = Hold::default();
        let key = (0, UVec2::new(1920, 1080));
        assert!(!hold.hold(false, key, true));
        let drawn = |hold: &mut Hold, key, compiled| (0..10).take_while(|_| !hold.hold(true, key, compiled)).count();
        assert_eq!(drawn(&mut hold, key, true), FRESH_FRAMES as usize);
        assert!(hold.hold(true, key, true), "held");
        assert_eq!(drawn(&mut hold, (1, key.1), true), FRESH_FRAMES as usize, "a setting changed");
        assert_eq!(drawn(&mut hold, (1, UVec2::new(1280, 720)), true), FRESH_FRAMES as usize, "resized");
        assert_eq!(drawn(&mut hold, (2, key.1), false), 10, "pipelines compiling");
        assert!(hold.hold(true, (2, key.1), true));
        assert!(!hold.hold(false, (2, key.1), true), "menu closed");
        assert_eq!(drawn(&mut hold, (2, key.1), true), FRESH_FRAMES as usize, "reopened");
    }
}

#[cfg(test)]
mod environment_tests {
    use super::*;
    #[test]
    fn native_procedural_switch_clears_tone_hdr_and_retains_camera_settings() {
        let mut world = World::new();
        let id = world
            .spawn((
                GameplayCamera,
                Camera3d::default(),
                Msaa::Sample4,
                bevy::camera::RenderTarget::default(),
            ))
            .id();
        for retail in [true, false, true, false] {
            set_world_environment(&mut world, retail);
            let entity = world.entity(id);
            assert_eq!(entity.contains::<bevy::render::view::Hdr>(), retail);
            assert_eq!(
                entity.contains::<crate::retail_render::RetailTone>(),
                retail
            );
            assert_eq!(*entity.get::<Msaa>().unwrap(), Msaa::Sample4);
            assert!(!entity.get::<Camera>().unwrap().is_active);
            assert_eq!(
                *entity
                    .get::<bevy::core_pipeline::tonemapping::Tonemapping>()
                    .unwrap(),
                if retail {
                    bevy::core_pipeline::tonemapping::Tonemapping::None
                } else {
                    bevy::core_pipeline::tonemapping::Tonemapping::default()
                }
            );
        }
    }
}
