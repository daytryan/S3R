//! `--hero-clip OUT.webm`: the launcher's Play-screen clip. The test world in a hidden window, pad 0 driven by a
//! trick script built from the installed skater.pat, the run recorded into the replay ring, then played back frame by
//! frame through a transparent camera (skater and board only) into ffmpeg. Silent, no Discord or Steam presence, no
//! saved settings. Exit codes: 0 ok, 1 bad args (config), 3 no usable patterns, 4 no landed trick, 5 encode failed, 6 watchdog (110 s).
pub(crate) mod encode;
pub(crate) mod script;
pub(crate) mod trim;

use crate::{app::SimulationSet, graphic_tile, replay::Replay, world::PlayerRoot};
use bevy::{
    camera::{
        ClearColorConfig, RenderTarget,
        visibility::{NoFrustumCulling, RenderLayers},
    },
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    prelude::*,
    render::{
        render_resource::Extent3d,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    time::TimeUpdateStrategy,
};
use skate_core::input::xbox::XboxState;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

/// The clip camera's private render layer: only the player's meshes and the clip's own lights are on it.
const LAYER: usize = 53;
/// Drawn 2x and averaged down by alpha (team_photo's soft edges, no MSAA): straight alpha for VP9.
const SUPER: u32 = 2;
/// The camera stands this far to the side of the line of travel, looking level...
const DISTANCE: f32 = 4.2;
/// ...where the frame's height covers this many metres...
const SPAN: f32 = 3.2;
/// ...with the floor this far down the frame (the launcher's coping row).
const GROUND_ROW: f32 = 0.86;
/// The loop starts this long after the skater reaches the flat: the ramp's bottom curve is behind him.
const LOOP_IN: f32 = 0.4;
/// Captures of the first frame before it is taken as it is (new pipelines and textures settling).
const WARM_TRIES: u32 = 30;

fn fov() -> f32 {
    2. * (SPAN / 2. / DISTANCE).atan()
}

/// Eye height above the floor: looking level, the floor lands GROUND_ROW down the frame.
fn eye_height() -> f32 {
    (GROUND_ROW - 0.5) * SPAN
}

/// This process is a clip run: player files (settings, personal bests) are never written.
pub(crate) fn running() -> bool {
    PAD.lock().unwrap_or_else(PoisonError::into_inner).is_some()
}

pub(crate) fn is_active(config: &crate::config::Config) -> bool {
    config.hero_clip.is_some()
}

/// For plugins deciding in `build` (Config is inserted before them in app.rs).
pub(crate) fn active(app: &App) -> bool {
    app.world().get_resource::<crate::config::Config>().is_some_and(is_active)
}

/// Pad 0's script and the sim tick it is on. Static because the pad is polled outside the ECS (input/platform.rs).
static PAD: Mutex<Option<(Vec<script::Step>, usize)>> = Mutex::new(None);

/// Pad `index` in clip mode, as (packet number, state); `Some(None)` = disconnected (every pad but 0).
/// `None` outside clip mode.
pub(crate) fn pad(index: usize) -> Option<Option<(u32, XboxState)>> {
    let guard = PAD.lock().unwrap_or_else(PoisonError::into_inner);
    let (steps, tick) = guard.as_ref()?;
    if index != 0 {
        return Some(None);
    }
    let s = steps[(*tick).min(steps.len() - 1)];
    // input/script.rs's scale: -100..100 -> the XInput axis range
    let axis = |v: i16| (i32::from(v.clamp(-100, 100)) * 32767 / 100) as i16;
    let buttons = u16::from(s.push) * 0x1000 | u16::from(s.brake) * 0x2000; // A pushes, B brakes
    Some(Some((*tick as u32, XboxState { buttons, triggers: [0; 2], left: [0; 2], right: [axis(s.rx), axis(s.ry)] })))
}

pub(crate) struct HeroClipPlugin;

#[derive(Resource)]
struct Hero {
    out: PathBuf,
    /// The active member's look file (character.json for the boss) as it was read: its stance drove the script, its
    /// hash goes in hero.json.
    character: Option<Vec<u8>>,
    render: Option<Render>,
}

struct Render {
    /// The clip's first frame on the recording clock, and how many 30 fps frames it has.
    start: f64,
    frames: usize,
    next: usize,
    speed: f32,
    still: f32,
    target: Handle<Image>,
    camera: Entity,
    captured: Arc<Mutex<Option<Image>>>,
    stage: Stage,
    tries: u32,
    previous: Option<image::RgbaImage>,
    encoder: Option<encode::Encoder>,
}

#[derive(PartialEq)]
enum Stage {
    /// Move the playhead and the camera; the pose shows next frame.
    Seek,
    Shoot,
    Wait,
}

impl Plugin for HeroClipPlugin {
    fn build(&self, app: &mut App) {
        // its own thread, so a hung main thread still exits 6; under the launcher's 120 s kill, and roomy enough for a
        // first run after a build compiling every pipeline cold
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(110));
            std::process::exit(6);
        });
        app.insert_resource(bevy::audio::GlobalVolume::new(bevy::audio::Volume::SILENT))
            .add_systems(Startup, start)
            .add_systems(FixedUpdate, advance.in_set(SimulationSet::Input))
            .add_systems(Update, (end_run, render).chain());
    }
}

fn fail(code: i32, error: impl std::fmt::Display) -> ! {
    error!("hero clip: {error}");
    std::process::exit(code);
}

fn start(mut commands: Commands, config: Res<crate::config::Config>, fixed: Res<Time<Fixed>>) {
    let Some(out) = config.hero_clip.clone() else { return };
    let patterns = skate_data::gesture_patterns::load(&config.asset_root.join("private/stock/data/joystick/skater.pat"))
        .unwrap_or_else(|e| fail(3, e));
    let patterns: Vec<_> = patterns.into_iter().map(|p| script::Pattern { name: p.name, coords: p.points }).collect();
    // the look the player skates as (Edit Skaters' Select Skater), which the customiser dresses the run in too
    let file = crate::team_data::active_look_path(&crate::saves::settings_dir(&config.asset_root));  // Not retail (user 10-13)
    let character = std::fs::read(file).ok();
    // the customiser's default stance is 1 (goofy), which mirrors the flicks sideways
    let stance = character
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .and_then(|look| look["stance"].as_f64())
        .unwrap_or(1.);
    let tricks = std::env::var("SKATE_HERO_TRICKS").unwrap_or_else(|_| "Ollie,Kickflip,360Flip".into());
    let tricks: Vec<&str> = tricks.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
    let steps = script::plan(&patterns, &tricks, stance == 1.).unwrap_or_else(|e| fail(3, e));
    *PAD.lock().unwrap_or_else(PoisonError::into_inner) = Some((steps, 0));
    // exactly one sim tick per frame however fast the hidden window draws, so every flick point lands on its tick
    commands.insert_resource(TimeUpdateStrategy::ManualDuration(fixed.timestep()));
    commands.insert_resource(Hero { out, character, render: None });
}

fn advance() {
    if let Some((_, tick)) = PAD.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        *tick += 1;
    }
}

/// The script has run: pick the clip from the recording, then set up its camera, lights and encoder.
#[allow(clippy::too_many_arguments)]
fn end_run(
    mut commands: Commands,
    hero: Option<ResMut<Hero>>,
    mut replay: ResMut<Replay>,
    mut images: ResMut<Assets<Image>>,
    players: Query<Entity, With<PlayerRoot>>,
    children: Query<&Children>,
    meshes: Query<Option<&RenderLayers>, With<Mesh3d>>,
) {
    let Some(mut hero) = hero.filter(|h| h.render.is_none()) else { return };
    let ran = PAD.lock().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|(steps, tick)| *tick >= steps.len());
    if !ran {
        return;
    }
    let ring = replay.frames();
    let frames: Vec<_> = (0..ring.len()).filter_map(|i| ring.get(i)).collect();
    let roots: Vec<Vec3> = frames.iter().map(|f| f.actors.first().map_or(Vec3::ZERO, |a| a.root.translation)).collect();
    // the start box is 1.5 m above the floor (physics/ground.rs)
    let floor = roots.first().map_or(0., |r| r.y) - 1.4;
    let ticks: Vec<trim::Tick> = frames
        .iter()
        .zip(&roots)
        .map(|(f, r)| trim::Tick {
            time: f.time as f32,
            on_flat: r.y < floor,
            bailed: f.audio.as_ref().is_some_and(|a| a.wiping_out),
            landed: f.audio.as_ref().is_some_and(|a| a.landed),
        })
        .collect();
    let Some((flat, last)) = trim::window(&ticks) else { fail(4, "no landed trick") };
    // a seamless loop: from rolling on the flat (past the ramp's bottom curve) to the pose after the last landing
    // that matches it best, before the closing brake
    // ...opening just before the first push on the flat (ring frame 0 is the first sim tick)
    let first_push = frames.first().map_or(0., |f| f.time as f32) + script::FLAT_PUSH_AT as f32 / 60.;
    let start = (flat + LOOP_IN).max(first_push - LOOP_IN);
    let pose = |t: f32| ticks.iter().position(|k| k.time >= t).map(|i| frames[i].actors.first().map(|a| a.bones.clone()).unwrap_or_default());
    let first = pose(start).unwrap_or_else(|| fail(4, "the run ended on the ramp"));
    let (from, to) = trim::seam_span(&ticks).unwrap_or((last, last));
    let later: Vec<(f32, Vec<Mat4>)> = ticks
        .iter()
        .zip(&frames)
        .filter(|(k, _)| k.time >= from && k.time <= to)
        .map(|(k, f)| (k.time, f.actors.first().map(|a| a.bones.clone()).unwrap_or_default()))
        .collect();
    let end = trim::seam(&first, &later).unwrap_or(last);
    let at = |t: f32| ticks.iter().position(|k| k.time >= t).map_or(roots[roots.len() - 1], |i| roots[i]);
    let (a, b) = (at(start), at(end));
    let speed = Vec2::new(b.x - a.x, b.z - a.z).length() / (end - start).max(0.1);
    // the launcher's rest frame: just after the first landing, rolling on the board
    let landed = ticks.iter().find(|k| k.landed && k.time >= start).map_or(start, |k| k.time);
    let still = (landed - start + 0.25).min(end - start);
    // the frame at `end` repeats the first one, so it is left out
    let frames = (((end - start) * encode::FPS as f32) as usize).max(1);
    replay.script(f64::from(start));

    for player in &players {
        for entity in children.iter_descendants(player) {
            if let Ok(layers) = meshes.get(entity) {
                commands.entity(entity).insert((layers.cloned().unwrap_or_default().with(LAYER), NoFrustumCulling));
            }
        }
    }
    let mut image = graphic_tile::target(1);
    image.resize(Extent3d { width: encode::W * SUPER, height: encode::H * SUPER, depth_or_array_layers: 1 });
    let target = images.add(image);
    // side view from -x: travel (+z) runs to the right of the frame
    let rotation = Transform::IDENTITY.looking_to(Vec3::X, Vec3::Y).rotation;
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera { order: -2, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
            RenderTarget::Image(target.clone().into()),
            Projection::Perspective(PerspectiveProjection { fov: fov(), ..default() }),
            Transform { rotation, ..default() },
            RenderLayers::layer(LAYER),
            Msaa::Off,
            Tonemapping::None,
            DebandDither::Disabled,
            bevy::light::AmbientLight { color: Color::WHITE, brightness: graphic_tile::AMBIENT, ..default() },
        ))
        .id();
    for (lux, at) in graphic_tile::lights(rotation) {
        commands.spawn((DirectionalLight { illuminance: lux, shadows_enabled: false, ..default() }, at, RenderLayers::layer(LAYER)));
    }
    let encoder = encode::Encoder::spawn(&skate_data::audio::ffmpeg::program(), &hero.out).unwrap_or_else(|e| fail(5, e));
    hero.render = Some(Render {
        start: f64::from(start),
        frames,
        next: 0,
        speed,
        still,
        target,
        camera,
        captured: Arc::default(),
        stage: Stage::Seek,
        tries: 0,
        previous: None,
        encoder: Some(encoder),
    });
}

/// One clip frame at a time, as the replay export does it (replay/hud.rs): playhead, render, capture, encode.
fn render(
    mut commands: Commands,
    hero: Option<ResMut<Hero>>,
    mut replay: ResMut<Replay>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    let Some(mut hero) = hero else { return };
    let Hero { out, character, render } = &mut *hero;
    let Some(r) = render.as_mut() else { return };
    match r.stage {
        Stage::Seek => {
            replay.seek(r.start + r.next as f64 / f64::from(encode::FPS));
            // follow the skater along the floor, never up and down: the floor stays on the coping row
            if let (Some((a, b, alpha)), Ok(mut camera)) = (replay.sample(), cameras.get_mut(r.camera)) {
                let root = a.root.translation.lerp(b.root.translation, alpha);
                let floor = crate::physics::ground::FLOOR_HEIGHT;
                camera.translation = Vec3::new(root.x - DISTANCE, floor + eye_height(), root.z);
            }
            r.stage = Stage::Shoot;
        }
        Stage::Shoot => {
            let slot = r.captured.clone();
            commands.spawn(Screenshot::image(r.target.clone())).observe(move |captured: On<ScreenshotCaptured>| {
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(captured.image.clone());
                }
            });
            r.stage = Stage::Wait;
        }
        Stage::Wait => {
            let Some(image) = r.captured.lock().ok().and_then(|mut c| c.take()) else { return };
            let shot = graphic_tile::rgba(&image)
                .map(|big| graphic_tile::downsample(&big, SUPER))
                .unwrap_or_else(|| fail(5, "unreadable capture"));
            if r.next == 0 {
                // the first frame twice the same before it counts: pipelines and textures have settled
                let verdict = graphic_tile::judge(r.previous.as_ref(), &shot);
                if verdict != graphic_tile::Verdict::Done && r.tries < WARM_TRIES {
                    r.tries += 1;
                    r.previous = Some(shot);
                    r.stage = Stage::Shoot;
                    return;
                }
                if verdict == graphic_tile::Verdict::Empty {
                    fail(5, "the skater never showed in the clip camera");
                }
            }
            let encoder = r.encoder.as_mut().expect("encoder lives until the last frame");
            encoder.push(shot.as_raw()).unwrap_or_else(|e| fail(5, e));
            r.next += 1;
            r.stage = Stage::Seek;
            if r.next < r.frames {
                return;
            }
            r.encoder.take().expect("checked above").finish().unwrap_or_else(|e| fail(5, e));
            let character = character.as_deref().map_or("none".into(), |bytes| {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(bytes))
            });
            let json = serde_json::json!({ "speed": r.speed, "character": character, "still": r.still });
            std::fs::write(out.with_extension("json"), json.to_string()).unwrap_or_else(|e| fail(5, e));
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_sits_on_the_coping_row_and_the_frame_spans_three_point_two_metres() {
        use bevy::camera::CameraProjection;
        let projection = PerspectiveProjection { fov: fov(), aspect_ratio: encode::W as f32 / encode::H as f32, ..default() };
        let eye = Transform::from_xyz(-DISTANCE, eye_height(), 0.).looking_to(Vec3::X, Vec3::Y);
        let ndc = |p: Vec3| {
            let clip = projection.get_clip_from_view() * eye.to_matrix().inverse() * p.extend(1.);
            clip.truncate().truncate() / clip.w
        };
        let row = |y: f32| (1. - ndc(Vec3::new(0., y, 0.)).y) / 2.;
        assert!((row(0.) - GROUND_ROW).abs() < 0.01, "floor row {}", row(0.));
        assert!(((row(0.) - row(1.)) * SPAN - 1.).abs() < 0.01, "one metre = {} of the frame", row(0.) - row(1.));
        assert!(ndc(Vec3::new(0., 1., 1.)).x > 0., "travel (+z) runs right");
    }
}
