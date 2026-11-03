//! Opt-in startup verification, with no in-game tool UI.
use crate::{
    animation::AnimationStatus,
    app::FrameSet,
    config::Config,
    input::ControllerInput,
    physics::{GamePhysics, PlayerControls, SkaterRuntime},
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    time::TimeUpdateStrategy,
};
use std::time::Duration;

#[derive(Resource, Default)]
struct Verification {
    elapsed: f32,
    requested: bool,
    captured: bool,
}

pub(crate) struct VerificationPlugin;
impl Plugin for VerificationPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(list) = std::env::var("SKATE_CAPTURE_SPOTS") {
            let list = list.split(',').filter_map(|i| i.trim().parse().ok()).collect();
            let toggle = std::env::var_os("SKATE_CAPTURE_TOGGLE").is_some_and(|v| v == "1");
            if std::env::var_os("SKATE_CAPTURE_FREEZE").is_some_and(|v| v == "1") {
                app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
            }
            app.insert_resource(Spots { list, toggle, frame: 0, saved: 0, look: None })
                .add_systems(Update, capture_spots.in_set(FrameSet::Verification));
            return;
        }
        app.init_resource::<Verification>()
            .add_systems(Update, verify.in_set(FrameSet::Verification));
    }
}

/// FPS pass look check (10-23, diagnostics, with --verify): SKATE_CAPTURE_SPOTS=i,j,.. shoots the map's Travel
/// destinations i, j, .. as <capture>-<n>.png instead of the one startup shot, each SETTLE frames after the skater is
/// moved there, then exits. Frames count once the skater is ready and no map change is loading, from 0 again after
/// each map change. SKATE_CAPTURE_FREEZE=1 advances the clock one fixed tick per counted frame and not at all on the
/// others (as hero_clip does), so two runs of one build draw the same frames. SKATE_CAPTURE_TOGGLE=1 switches to
/// Performance mode and back before the first spot, so a pass the switch leaves stale shows in the shots.
#[derive(Resource)]
struct Spots {
    list: Vec<usize>,
    toggle: bool,
    frame: u32,
    saved: usize,
    look: Option<crate::graphics_menu::Look>,
}
const FIRST: u32 = 300;
const SETTLE: u32 = 180;
const PERIOD: u32 = 240;
#[derive(Debug, PartialEq)]
enum Step {
    Wait,
    Performance(bool),
    Move(usize),
    Shoot(usize),
    Done,
}
fn step(frame: u32, spots: usize, toggle: bool) -> Step {
    match frame {
        30 if toggle => Step::Performance(true),
        120 if toggle => Step::Performance(false),
        f if f >= FIRST + spots as u32 * PERIOD => Step::Done,
        f if f >= FIRST && (f - FIRST) % PERIOD == 0 => Step::Move(((f - FIRST) / PERIOD) as usize),
        f if f >= FIRST && (f - FIRST) % PERIOD == SETTLE => Step::Shoot(((f - FIRST) / PERIOD) as usize),
        _ => Step::Wait,
    }
}
#[allow(clippy::too_many_arguments)]
fn capture_spots(
    mut commands: Commands,
    (config, animation, fixed): (Res<Config>, Res<AnimationStatus>, Res<Time<Fixed>>),
    (transition, travel): (Res<crate::map_transition::MapTransition>, Res<crate::teleport_menu::Travel>),
    mut changed: MessageReader<crate::map_transition::WorldChanged>,
    mut skater: ResMut<SkaterRuntime>,
    mut menu: ResMut<crate::graphics_menu::Menu>,
    mut spots: ResMut<Spots>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = &config.verification_capture else {
        return;
    };
    if changed.read().count() > 0 {
        spots.frame = 0;
    }
    let counting = animation.ready && !transition.busy();
    if std::env::var_os("SKATE_CAPTURE_FREEZE").is_some_and(|v| v == "1") {
        let tick = if counting { fixed.timestep() } else { Duration::ZERO };
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(tick));
    }
    if !counting {
        return;
    }
    match step(spots.frame, spots.list.len(), spots.toggle) {
        Step::Wait => {}
        Step::Performance(on) => {
            let back = spots.look.take().filter(|_| !on);
            let replaced = menu.diagnostic_look(back);
            if on {
                spots.look = Some(replaced);
            } else {
                spots.toggle = false;
            }
        }
        Step::Move(n) => {
            let destinations = travel.destinations();
            let spot = destinations.get(spots.list[n] % destinations.len().max(1)).and_then(|d| d.matrix);
            match spot.map(|m| skater.travel_to(m)) {
                Some(Ok(())) => {}
                other => error!("SKATE_CAPTURE_SPOTS: spot {} not reached ({other:?})", spots.list[n]),
            }
        }
        Step::Shoot(n) => {
            let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path.with_file_name(format!("{stem}-{n}.png"))))
                .observe(|_: On<ScreenshotCaptured>, mut spots: ResMut<Spots>| spots.saved += 1);
        }
        Step::Done => {
            if spots.saved < spots.list.len() {
                return;
            }
            info!("GAME_VERIFY_OK");
            exit.write(AppExit::Success);
        }
    }
    spots.frame += 1;
}
fn verify(
    mut commands: Commands,
    time: Res<Time<Real>>,
    config: Res<Config>,
    animation: Res<AnimationStatus>,
    input: Res<ControllerInput>,
    physics: Res<GamePhysics>,
    controls: Res<PlayerControls>,
    skater: Res<SkaterRuntime>,
    camera: Res<crate::camera::CameraRuntime>,
    mut replay: ResMut<crate::replay::Replay>,
    mut state: ResMut<Verification>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = &config.verification_capture else {
        return;
    };
    state.elapsed += time.delta_secs();
    // Opt-in visual smoke check of the replay HUD and presentation endpoints.
    if animation.ready
        && state.elapsed > 2.0
        && !replay.active
        && std::env::var("SKATE_VERIFY_REPLAY").as_deref() == Ok("1")
    {
        replay.enter();
    }
    // SKATE_VERIFY_SECONDS waits longer for a big map to stream in.
    let delay = std::env::var("SKATE_VERIFY_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(4f32);
    if animation.ready && state.elapsed > delay && !state.requested {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()))
            .observe(
                |_: On<ScreenshotCaptured>, mut state: ResMut<Verification>| {
                    state.captured = true;
                },
            );
        state.requested = true;
    }
    if state.captured && state.elapsed > delay + 2. {
        let input_report = format!(
            "Game integration capture (not a Skate 3 parity verdict)\nExecutable: {}\nPolls: {}\nConsumed batches: {}\nStatus: {:?}\nPacket numbers: {:?}\nActions 64..81 by device: {:?}\nDerived ticks: {}\nIntents: {:?}\nDifficulty index: {}\nPhysics ticks: {}\nContacts: {}\nBody positions: {:?}\nPhysical pose publications: {}\nGraph animation ticks: {}\nTruck targets: {:?}\nGround speed: {}\nGameplay camera shot: {}\nCamera frame: {:?}\n",
            std::env::current_exe()
                .map_or_else(|_| "unavailable".into(), |p| p.display().to_string()),
            input.publications,
            input.consumed_batches,
            input.status,
            input.packet_numbers,
            input.mapped_actions,
            controls.ticks,
            controls.intents,
            physics.difficulty_index(),
            physics.ticks,
            physics.contact_count,
            physics.board.bodies().map(|body| body.rates.position),
            skater.pose_generation,
            skater.animation.ticks,
            skater.ground.steering.targets,
            physics.riding.motion.ground_speed,
            camera.selected_shot(),
            camera.frame,
        );
        let input_report = format!(
            "{input_report}Native OnBoard/OffBoard clips available: {}\n",
            skater.animation.evaluator.frames.clip_count()
        );
        if let Err(error) = std::fs::write(path.with_extension("input.txt"), input_report) {
            eprintln!("Cannot save controller verification: {error}");
            exit.write(AppExit::error());
            return;
        }
        info!("GAME_VERIFY_OK");
        exit.write(AppExit::Success);
    } else if state.elapsed > delay + 41. {
        error!("Game startup/capture timed out");
        exit.write(AppExit::error());
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, step};

    /// The look check's schedule (FPS pass 10-23): the Performance round trip first, then each spot moved to and shot
    /// SETTLE frames later, then done.
    #[test]
    fn spots_are_shot_after_settling_and_the_toggle_comes_first() {
        let at = |frame| step(frame, 2, true);
        assert_eq!(at(0), Step::Wait);
        assert_eq!(at(30), Step::Performance(true));
        assert_eq!(at(120), Step::Performance(false));
        assert_eq!(at(300), Step::Move(0));
        assert_eq!(at(300 + 180), Step::Shoot(0));
        assert_eq!(at(300 + 240), Step::Move(1));
        assert_eq!(at(300 + 240 + 180), Step::Shoot(1));
        assert_eq!(at(300 + 480), Step::Done);
        assert_eq!(step(30, 2, false), Step::Wait, "no toggle unless asked");
    }
}
