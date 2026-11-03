//! The local skater under Skate 2 physics (Task 16). Not tested in play.
//!
//! While `Skate2Physics::drives_local`, `physics::advance` returns before the engine's sim (`frame::advance_as`) and
//! `step`, right after it in `SimulationSet::Physics`, runs the port instead, once per fixed tick:
//! - the pad (`port_pad`): the engine's polled pad (`input::pad`, neutral while the engine gates gameplay), and in a
//!   match intro only what the engine's own intro lets through;
//! - `Skate2Physics::tick`, then the frame on the skater (`apply`): the pose (`pose`), the root, the filtered category,
//!   the camera (`camera`) and the tick period.
//!
//! A failure restarts the sim (Task 14, Amendment F). The skater is then held where the last frame put it
//! (`Skate2Physics::last_good`): nothing ticks, the engine's sim included, until the switch places it back (Task 19).
//! When the retries stop (`HostState::Failed`) `drives_local` is false and the engine's sim has the skater again on the
//! next tick. Only the local player is bypassed: the riders (`physics::riders`) keep the engine's sim.

use std::time::Duration;

use bevy::prelude::*;
use skate2_physics_protocol::{Frame, Pad};
use skate_core::camera::CameraFrame;
use skate_core::physics::filtered_state::FilteredCategory;

use super::{Skate2Physics, camera, input, pose, sim::HostState};
use crate::camera::CameraRuntime;
use crate::input::ControllerInput;
use crate::physics::SkaterRuntime;

/// The port's filtered categories by the byte it sends (`host/src/frame.rs` `category`: PhysOut's
/// `FilteredPhysicsState`, in the port's order), each with the engine's `FilteredCategory` of the same name.
pub(crate) const CATEGORIES: [(&str, FilteredCategory); 8] = [
    ("Invalid", FilteredCategory::Invalid),
    ("Ground", FilteredCategory::Ground),
    ("Air", FilteredCategory::Air),
    ("Grind", FilteredCategory::Grind),
    ("Wipeout", FilteredCategory::Wipeout),
    ("Teleport", FilteredCategory::Teleport),
    ("OffBoard", FilteredCategory::Offboard),
    ("OffBoardAir", FilteredCategory::OffboardAir),
];

/// The engine's category for the port's byte; `Invalid` for a byte the port never sends.
pub(crate) fn category(byte: u8) -> FilteredCategory {
    CATEGORIES.get(usize::from(byte)).map_or(FilteredCategory::Invalid, |(_, category)| *category)
}

/// The D-pad and the shoulders (LB, RB): what a match intro with emotes lets through.
const EMOTES: u16 = 0x030F;

/// The pad the port gets this tick: `input::pad` of the engine's polled pad (neutral while the engine gates gameplay).
/// In a match intro (`match_hold`) it gets what the engine's own intro lets through (`tick_input`): nothing, or with
/// the emotes on (`match_gestures`) only the D-pad and the shoulders, with the sticks and triggers at rest. Not native
/// Skate 2 (user 10-20): the engine's modes pin the skater through their intro and countdown; Skate 2 would read the
/// whole pad.
pub(crate) fn port_pad(input: &ControllerInput) -> Pad {
    let pad = input::pad(input.raw_input(), input.gated());
    match (input.match_hold, input.match_gestures) {
        (false, _) => pad,
        (true, false) => Pad::default(),
        (true, true) => Pad { buttons: pad.buttons & EMOTES, ..Pad::default() },
    }
}

/// The fixed clock follows the host's period for this tick (slow motion stretches it). A period no clock can run
/// (not above 0, or not finite) keeps the last one.
pub(crate) fn follow_period(cadence: &mut Time<Fixed>, period_s: f32) {
    match Duration::try_from_secs_f32(period_s) {
        Ok(period) if !period.is_zero() => cadence.set_timestep(period),
        _ => {}
    }
}

/// Whether the engine's own sim runs the local skater this tick: not while Skate 2 physics drives it, nor while it holds
/// it through a restart after driving (`Skate2Physics::drives_local`; Amendment F: no Skate 3 physics in between). With
/// no resource (an app without the switch's plugin) it always runs.
pub(crate) fn engine_runs_local(skate2: Option<&Skate2Physics>) -> bool {
    !skate2.is_some_and(Skate2Physics::drives_local)
}

/// One tick of the port while it drives the skater: its frame, or `None` while a restart holds the skater (nothing is
/// sent) or when this tick failed (which restarts the sim, Task 14).
pub(crate) fn drive_tick(skate2: &mut Skate2Physics, pad: Pad) -> Option<Box<Frame>> {
    if !matches!(skate2.state, HostState::Driving(_)) {
        return None;
    }
    skate2.tick(pad)
}

/// The start of a hold (a restart after driving): one new pose generation with the pose unchanged, so presentation
/// captures the held pose as both its snapshots and stops blending between the last two frames. Later held ticks add
/// none. Not native Skate 2 (user 10-20): Skate 2 never restarts its sim.
pub(crate) fn hold(held: &mut bool, pose_generation: &mut u64) {
    if !std::mem::replace(held, true) {
        *pose_generation = pose_generation.wrapping_add(1);
    }
}

/// The port's frame on the local skater: the pose by bone name, the root, the category, the tricks (the engine's
/// scorer, Task 17), the view (`view` is `None` while the player watches a rider, whose own sim drives the camera) and
/// the clock.
pub(crate) fn apply(
    frame: &Frame,
    skater: &mut SkaterRuntime,
    view: Option<&mut CameraRuntime>,
    cadence: &mut Time<Fixed>,
) {
    let names: Vec<&str> = skater.animation.evaluator.frames.bone_names.iter().map(String::as_str).collect();
    skater.render_pose = pose::retarget(frame, &names);
    skater.pose_generation = skater.pose_generation.wrapping_add(1);
    let to_world = pose::animation_to_world(frame);
    skater.animated_skeleton.roots.animation_to_world = to_world;
    skater.animated_skeleton.roots.world_to_animation = pose::inverse_rigid(&to_world);
    let category = category(frame.category);
    let teleport = FilteredCategory::Teleport as u32;
    // The port's teleport (a respawn) moves the skater in one tick somewhere inside it: the view cuts on every tick
    // in it and on the tick that leaves it, as presentation cuts for the engine's own teleports (the protocol has no
    // `hard_cut`).
    let cut = category as u32 == teleport || skater.player_input.physical.filtered_state_0 == teleport;
    skater.player_input.physical.filtered_state_0 = category as u32;
    // Not native Skate 2 (user 10-20): bit 10 is the engine's teleport flag, written only by its own sim; left set
    // from before the switch, presentation would cut every frame of the drive.
    skater.player_input.processed.flags_2472 &= !(1 << 10);
    // The port's tricks in place of the engine's `advance` (Task 17), which does not run under the switch.
    skater.scoring.apply_port_frame(&frame.tricks);
    if let Some(view) = view {
        let previous = view.frame.unwrap_or_else(camera::neutral);
        view.frame = Some(CameraFrame { discontinuity: cut, ..camera::frame(&frame.camera, &previous) });
    }
    follow_period(cadence, frame.sim_period_s);
}

/// The fixed-tick system (`PhysicsPlugin`, after `advance`): returns at once unless Skate 2 physics has the local
/// skater; while it does, ticks the port with the pad and applies its frame, or holds the skater.
pub(crate) fn step(
    skate2: Option<ResMut<Skate2Physics>>,
    input: Option<Res<ControllerInput>>,
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    riders: Res<crate::physics::riders::Riders>,
    mut skater: ResMut<SkaterRuntime>,
    mut view: ResMut<CameraRuntime>,
    mut cadence: ResMut<Time<Fixed>>,
    mut held: Local<bool>,
) {
    let Some(mut skate2) = skate2 else {
        return;
    };
    if engine_runs_local(Some(&skate2)) {
        *held = false;
        return;
    }
    // `advance` skips a skater who is driving a vehicle; so does the port, which then waits where it was. Not native
    // Skate 2 (user 10-20): the engine's vehicles are its own.
    if vehicles.occupied() {
        return;
    }
    let pad = input.as_deref().map_or_else(Pad::default, port_pad);
    match drive_tick(&mut skate2, pad) {
        Some(frame) => {
            *held = false;
            let view = (!riders.watching()).then_some(&mut *view);
            apply(&frame, &mut skater, view, &mut cadence);
        }
        // A failure that stopped the retries gives the skater back to the engine's sim, which draws it next tick.
        None if skate2.drives_local() => hold(&mut held, &mut skater.pose_generation),
        None => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use bevy::ecs::system::RunSystemOnce;
    use skate2_physics_protocol::{Boot, EngineMessage, Frame, HostMessage, Pad};
    use skate_core::input::xbox::XboxState;

    use super::*;
    use crate::skate2_physics::input;
    use crate::skate2_physics::sim::{HostState, Link, Refused};

    /// The names the host gives its categories (`host/src/frame.rs` `category`), in its byte order.
    const PORT_CATEGORIES: [&str; 8] = ["Invalid", "Ground", "Air", "Grind", "Wipeout", "Teleport", "OffBoard", "OffBoardAir"];

    #[test]
    fn the_category_maps_by_name() {
        for (byte, port) in PORT_CATEGORIES.iter().enumerate() {
            let ours = category(byte as u8);
            assert!(format!("{ours:?}").eq_ignore_ascii_case(port), "byte {byte}: the port's {port} is our {ours:?}");
        }
        assert_eq!(category(8), FilteredCategory::Invalid, "a byte the port never sends");
        assert_eq!(category(255), FilteredCategory::Invalid);
    }

    #[test]
    fn the_tick_period_follows_the_host() {
        let mut cadence = Time::<Fixed>::from_seconds(1.0 / 60.0);
        follow_period(&mut cadence, 1.0 / 15.0);
        let ms = cadence.timestep().as_secs_f64() * 1000.0;
        assert!((ms - 1000.0 / 15.0).abs() < 0.001, "{ms} ms");
        follow_period(&mut cadence, 1.0 / 60.0);
        let ms = cadence.timestep().as_secs_f64() * 1000.0;
        assert!((ms - 1000.0 / 60.0).abs() < 0.001, "{ms} ms");
        // A period no clock can run keeps the last one.
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            follow_period(&mut cadence, bad);
            assert!((cadence.timestep().as_secs_f64() * 1000.0 - 1000.0 / 60.0).abs() < 0.001, "{bad}");
        }
    }

    /// A pad with every kind of input: A, RB and D-pad left held, both sticks and both triggers off centre.
    fn held() -> ControllerInput {
        let mut input = ControllerInput::default();
        input.sample_raw_for_test(XboxState {
            buttons: 0x1000 | 0x0200 | 0x0004,
            triggers: [200, 30],
            left: [12000, -9000],
            right: [-3000, 32767],
        });
        input
    }

    #[test]
    fn the_match_intro_withholds_the_pad() {
        let mut input = held();
        let live = input::pad(input.raw_input(), false);
        assert_ne!(live, Pad::default());
        assert_eq!(port_pad(&input), live, "no match intro: the whole pad");
        input.match_hold = true;
        assert_eq!(port_pad(&input), Pad::default(), "a match intro with no emotes: a neutral pad");
        input.match_gestures = true;
        assert_eq!(
            port_pad(&input),
            Pad { buttons: 0x0200 | 0x0004, ..Pad::default() },
            "a match intro with emotes: only the D-pad and the shoulders, no sticks or triggers"
        );
        input.pinned = true;
        assert_eq!(port_pad(&input), Pad::default(), "a gate still wins");
    }

    /// A resource driving over a channel link (no thread), whose restarts never get a slot.
    fn driving() -> (Skate2Physics, std::sync::mpsc::Sender<HostMessage>, std::sync::mpsc::Receiver<EngineMessage>) {
        let (to_host, from_engine) = channel();
        let (to_engine, from_host) = channel();
        let mut skate2 = Skate2Physics::with(Box::new(|_: &Path, _: &Boot| Err(Refused::Busy)), Box::new(Instant::now));
        skate2.state = HostState::Driving(Link::from_channels(to_host, from_host));
        (skate2, to_engine, from_engine)
    }

    #[test]
    fn a_restart_holds_the_skater() {
        let (mut skate2, to_engine, from_engine) = driving();
        let pad = Pad { buttons: 0x1000, ..Pad::default() };
        to_engine.send(HostMessage::Frame(Box::new(Frame { tick: 7, ..Frame::default() }))).unwrap();
        assert_eq!(drive_tick(&mut skate2, pad).map(|frame| frame.tick), Some(7));
        assert_eq!(from_engine.try_recv(), Ok(EngineMessage::Tick(pad)), "the pad reached the sim");

        // The sim ends: the tick fails, the sim restarts, and the skater is held where the last frame put it.
        drop(to_engine);
        assert!(drive_tick(&mut skate2, pad).is_none());
        assert!(matches!(skate2.state, HostState::Restarting(_)));
        assert!(skate2.drives_local(), "the engine's sim does not take over while Skate 2 reloads");
        assert_eq!(skate2.last_good.as_ref().map(|frame| frame.tick), Some(7));
        assert_eq!(from_engine.try_iter().collect::<Vec<_>>(), [EngineMessage::Tick(pad), EngineMessage::Shutdown]);
        for _ in 0..5 {
            assert!(drive_tick(&mut skate2, pad).is_none(), "nothing ticks while held");
        }
        assert!(skate2.drives_local());
    }

    #[test]
    fn a_hold_bumps_the_pose_generation_once() {
        let (mut held, mut generation) = (false, 41);
        hold(&mut held, &mut generation);
        assert_eq!((held, generation), (true, 42), "the hold begins: one new generation");
        for _ in 0..5 {
            hold(&mut held, &mut generation);
        }
        assert_eq!(generation, 42, "no more while it lasts");
        held = false; // a frame was applied
        hold(&mut held, &mut generation);
        assert_eq!(generation, 43, "the next hold bumps it again");
    }

    #[test]
    fn the_engine_sim_runs_only_without_a_skate2_skater() {
        assert!(engine_runs_local(None), "no switch plugin");
        let mut skate2 = Skate2Physics::with(Box::new(|_: &Path, _: &Boot| Err(Refused::Busy)), Box::new(Instant::now));
        assert!(matches!(skate2.state, HostState::Off));
        assert!(engine_runs_local(Some(&skate2)), "Off");
        // Starting: the slot is busy, so the first attempt waits; Skate 3 physics drives while Skate 2 loads.
        skate2.start(Path::new("D:/s2"), &Boot::default());
        assert!(matches!(skate2.state, HostState::Starting(_)));
        assert!(engine_runs_local(Some(&skate2)), "Starting");
        let link = || Link::from_channels(channel().0, channel().1);
        for replace_due in [false, true] {
            skate2.replace_due = replace_due;
            skate2.state = HostState::Ready(link());
            assert_eq!(engine_runs_local(Some(&skate2)), !replace_due, "Ready, replace_due {replace_due}");
            skate2.state = HostState::Driving(link());
            assert!(!engine_runs_local(Some(&skate2)), "Driving, replace_due {replace_due}");
            skate2.state = HostState::Failed("it closed".into());
            assert!(engine_runs_local(Some(&skate2)), "Failed, replace_due {replace_due}");
        }
        // Restarting after driving holds the skater; a restart of a first start (nothing driven yet) does not.
        let (mut skate2, to_engine, _from_engine) = driving();
        drop(to_engine);
        assert!(drive_tick(&mut skate2, Pad::default()).is_none());
        assert!(matches!(skate2.state, HostState::Restarting(_)));
        assert!(!engine_runs_local(Some(&skate2)), "Restarting after driving");
        skate2.replace_due = false;
        assert!(engine_runs_local(Some(&skate2)), "Restarting a first start");
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn switch_off_runs_the_engine_sim_unchanged() {
        let mut app = harness();
        let world = app.world_mut();

        // Off: the engine's sim, as today.
        let before = ticks(world);
        let generation = world.resource::<SkaterRuntime>().pose_generation;
        tick(world);
        assert_eq!(ticks(world), before + 1, "one advance is one engine tick");
        assert_eq!(world.resource::<SkaterRuntime>().pose_generation, generation + 1);

        // Driving: the port's frame, and no engine tick.
        let (to_engine, _from_engine) = drive(world);
        let frame = port(0, 2);
        to_engine.send(HostMessage::Frame(Box::new(frame.clone()))).unwrap();
        let (before, generation) = (ticks(world), world.resource::<SkaterRuntime>().pose_generation);
        tick(world);
        assert_eq!(ticks(world), before, "the engine's sim did not run");
        let skater = world.resource::<SkaterRuntime>();
        assert_eq!(skater.pose_generation, generation + 1);
        let to_world = skater.animated_skeleton.roots.animation_to_world;
        let board = pose::then(&skater.render_pose[25], &to_world);
        for (i, want) in frame.bones[25].iter().enumerate().filter(|(i, _)| i % 4 != 3) {
            assert!((board[i / 4][i % 4] - want).abs() < 1e-4, "the board: [{i}] {} != {want}", board[i / 4][i % 4]);
        }
        assert_eq!(skater.player_input.physical.filtered_state_0, FilteredCategory::Air as u32);
        assert_eq!(world.resource::<Time<Fixed>>().timestep(), Duration::from_secs_f32(1.0 / 60.0));

        // The sim dies: the tick fails and the sim restarts. No Skate 3 tick runs in between (Amendment F), and the
        // skater keeps the last frame's pose and root.
        drop(to_engine);
        let skater = world.resource::<SkaterRuntime>();
        let held = (skater.render_pose.clone(), skater.animated_skeleton.roots.animation_to_world);
        let before = ticks(world);
        for n in 0..5 {
            tick(world);
            assert!(world.resource::<Skate2Physics>().drives_local(), "tick {n}: Skate 2 still has the skater");
            assert_eq!(ticks(world), before, "tick {n}: no engine tick while Skate 2 reloads");
            let skater = world.resource::<SkaterRuntime>();
            assert_eq!(skater.render_pose, held.0, "tick {n}: the pose is held");
            assert_eq!(skater.animated_skeleton.roots.animation_to_world, held.1, "tick {n}: the root is held");
        }
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Restarting(_)));
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn a_held_skater_stands_still() {
        let mut app = harness();
        let world = app.world_mut();
        tick(world);
        let (to_engine, _from_engine) = drive(world);
        for n in 0..2 {
            to_engine.send(HostMessage::Frame(Box::new(port(n, 2)))).unwrap();
            tick(world);
        }
        assert!(!still(world), "two port frames in a row are blended");
        drop(to_engine);
        let generation = world.resource::<SkaterRuntime>().pose_generation;
        for n in 0..5 {
            tick(world);
            assert!(world.resource::<Skate2Physics>().drives_local(), "tick {n}: held");
            assert_eq!(
                world.resource::<SkaterRuntime>().pose_generation,
                generation + 1,
                "tick {n}: one new generation when the hold begins, and no more"
            );
            assert!(still(world), "tick {n}: the held skater and view stand still");
        }
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn the_port_teleport_cuts_the_view() {
        let mut app = harness();
        let world = app.world_mut();
        tick(world);
        // The engine's teleport bit, left set from before the switch.
        world.resource_mut::<SkaterRuntime>().player_input.processed.flags_2472 |= 1 << 10;
        let (to_engine, _from_engine) = drive(world);
        let cut = |world: &World| world.resource::<CameraRuntime>().frame.expect("the port's view").discontinuity;
        to_engine.send(HostMessage::Frame(Box::new(port(0, 2)))).unwrap();
        tick(world);
        let flags = world.resource::<SkaterRuntime>().player_input.processed.flags_2472;
        assert_eq!(flags & (1 << 10), 0, "the engine's stale teleport bit is cleared");
        // A cut while the port is in its teleport (a respawn) and on the tick that leaves it, then blending again.
        for (n, category, want) in [(1, 2, false), (2, 5, true), (3, 5, true), (4, 2, true), (5, 2, false), (6, 2, false)] {
            to_engine.send(HostMessage::Frame(Box::new(port(n, category)))).unwrap();
            tick(world);
            assert_eq!(cut(world), want, "frame {n}, category {category}");
        }
        assert!(!still(world), "blending again after the teleport");
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn the_hall_of_meat_gate_skips_under_the_switch() {
        let mut app = harness();
        let world = app.world_mut();
        assert!(!crate::hall_of_meat::physics_skipped(world), "the gate feeds a tick that runs");
        let (to_engine, _from_engine) = drive(world);
        assert!(crate::hall_of_meat::physics_skipped(world), "nothing drains the rate queue while Skate 2 drives");
        drop(to_engine);
        assert!(drive_tick(&mut world.resource_mut::<Skate2Physics>(), Pad::default()).is_none());
        assert!(crate::hall_of_meat::physics_skipped(world), "nor while it holds the skater through a restart");
    }

    /// A mode that follows the switch scores Skate 2 tricks (Task 19's test, moved to Task 17 by Amendment E): while
    /// Skate 2 physics drives, a frame that ends `nosemanual` reaches the reads the modes make of the engine's scorer
    /// (`completed` for Skate 1's challenges; `announced`, `sequence_landed` and `sequence_score` for Skate 2's contests
    /// and lessons).
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn a_followed_mode_scores_skate_2_tricks() {
        use skate2_physics_protocol::Tricks;
        let mut app = harness();
        let world = app.world_mut();
        tick(world);
        let (to_engine, _from_engine) = drive(world);
        let mut begun = port(0, 1);
        begun.tricks = Tricks { begun: vec![2], multiplier: 1.0, ..Tricks::default() };
        to_engine.send(HostMessage::Frame(Box::new(begun))).unwrap();
        tick(world);
        let s = &world.resource::<SkaterRuntime>().scoring;
        assert_eq!(s.announced, ["nosemanual"]);
        assert!(s.new_trick, "a new trick on the display");
        let mut ended = port(1, 1);
        ended.tricks = Tricks {
            ended: vec![(2, 300)],
            line_score: 300,
            multiplier: 1.0,
            sequence_completed: true,
            completed_successfully: true,
            ..Tricks::default()
        };
        to_engine.send(HostMessage::Frame(Box::new(ended))).unwrap();
        let before = ticks(world);
        tick(world);
        assert_eq!(ticks(world), before, "the engine's sim (and its scorer's advance) did not run");
        let s = &world.resource::<SkaterRuntime>().scoring;
        assert_eq!(s.completed, ["nosemanual"]);
        assert!(s.sequence_landed);
        assert_eq!(s.sequence_score, 300.0);
    }

    /// The engine's local skater, sim, camera and presentation, with the switch `Off`.
    pub(crate) fn harness() -> App {
        use crate::physics::{GamePhysics, PlayerControls};
        let root = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        let physics = GamePhysics::load(&root).unwrap();
        let skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
        let mut app = App::new();
        app.insert_resource(crate::modding::vehicles::Vehicles::default())
            .insert_resource(CameraRuntime::load(&root).unwrap())
            .insert_resource(PlayerControls::load(&root).unwrap())
            .insert_resource(graphs)
            .insert_resource(physics)
            .insert_resource(skater)
            .insert_resource(Time::<Fixed>::from_seconds(1.0 / 60.0))
            .init_resource::<crate::input::PublishedTickInput>()
            .init_resource::<crate::physics::riders::Riders>()
            .init_resource::<ControllerInput>()
            .init_resource::<crate::presentation::Presentation>()
            .init_resource::<crate::replay::Replay>()
            .init_resource::<Skate2Physics>();
        app
    }

    /// One fixed tick of the local skater: the engine's `advance`, then `step`, then presentation's capture.
    pub(crate) fn tick(world: &mut World) {
        world.run_system_once(crate::physics::advance).unwrap();
        // Cached, as the app runs it: `step`'s `Local` (the hold) lives across ticks.
        world.run_system_cached(step).unwrap();
        world.run_system_once(crate::presentation::capture).unwrap();
    }

    fn ticks(world: &World) -> u64 {
        world.resource::<crate::physics::GamePhysics>().ticks
    }

    /// Whether presentation's two snapshots are the same, so the drawn skater and view stand still between them.
    fn still(world: &World) -> bool {
        let (previous, current) = world.resource::<crate::presentation::Presentation>().pair().expect("captured");
        previous.bones == current.bones && previous.root == current.root && previous.camera == current.camera
    }

    /// Puts the switch in `Driving` over a channel link: the host's ends, to send the frames and to keep the link's
    /// messages deliverable (a tick to a closed end fails).
    fn drive(world: &mut World) -> (std::sync::mpsc::Sender<HostMessage>, std::sync::mpsc::Receiver<EngineMessage>) {
        let (to_host, from_engine) = channel();
        let (to_engine, from_host) = channel();
        world.resource_mut::<Skate2Physics>().state = HostState::Driving(Link::from_channels(to_host, from_host));
        (to_engine, from_engine)
    }

    /// The port's frame `n`, moved `n` cm along x, in `category`.
    fn port(n: u8, category: u8) -> Frame {
        let mut frame = Frame { sim_period_s: 1.0 / 60.0, category, ..crate::skate2_physics::pose::tests::posed() };
        for bone in &mut frame.bones {
            bone[12] += f32::from(n) * 0.01;
        }
        frame
    }
}
