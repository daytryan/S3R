//! Raw D-pad -> listener gesture intents -> CharacterGesture channels -> pose.
use super::*;
use skate_core::animation::output::Sqt;

const CHANNELS: [&str; 3] = ["GestureBoth", "GestureRight", "GestureLeft"];
// XInput: DPAD_UP, DPAD_DOWN, LEFT_SHOULDER, Y.
const UP: u16 = 0x0001;
const DOWN: u16 = 0x0002;
const LB: u16 = 0x0100;
const Y: u16 = 0x8000;

struct Trace {
    /// A gesture channel is alive.
    active: Vec<bool>,
    /// The skater is off the board (native category500).
    offboard: Vec<bool>,
    /// Gesture table index CharacterGesture published.
    published: Vec<Option<u32>>,
    pose: Vec<Sqt>,
}

fn run(selections: Option<[u32; 4]>, buttons: impl Fn(u32) -> u16, ticks: u32, sample: u32) -> Trace {
    run_held(selections, buttons, |_| None, ticks, sample)
}

/// [`run`] under a match hold where `hold` says (physics.rs's force brake; Some(true): one that lets the emotes through).
fn run_held(
    selections: Option<[u32; 4]>,
    buttons: impl Fn(u32) -> u16,
    hold: impl Fn(u32) -> Option<bool>,
    ticks: u32,
    sample: u32,
) -> Trace {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    let assets = skate_data::GameAssets::load(root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
    let mut physics = GamePhysics::load(root).unwrap();
    physics.set_gesture_preferences(selections);
    let mut skater = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
    let mut controls = PlayerControls::load(root).unwrap();
    let mut input = crate::input::ControllerInput::default();
    let mut camera = crate::camera::CameraRuntime::load(root).unwrap();
    let mut trace = Trace { active: Vec::new(), offboard: Vec::new(), published: Vec::new(), pose: Vec::new() };
    for tick in 0..ticks {
        input.sample_raw_for_test(skate_core::input::xbox::XboxState {
            buttons: buttons(tick),
            triggers: [0; 2],
            left: [0; 2],
            right: [0; 2],
        });
        let mut actions = input.player_actions();
        controls.update(
            &mut actions,
            physics.settings.step.simulation.time_step,
            physics.settings.input_magnitude_threshold,
            skater.player_input.physical.scoring.capabilities_204,
        );
        controls.publish_gestures(
            physics.animation_profile.physics_mode,
            skater.player_input.physical.state.state_16,
        );
        let held = hold(tick);
        physics.animation_profile.force_braking = held.is_some();
        physics.animation_profile.gesture_force_brake_bypass = held == Some(true);
        frame::advance(&mut physics, &mut skater, &mut controls, &graphs, &mut actions, true, &mut camera)
            .unwrap_or_else(|error| panic!("Gesture tick={tick}: {error}"));
        let motion = &skater.animation.motion;
        trace.active.push(CHANNELS.iter().any(|c| motion.animation.channels.has(c)));
        trace.offboard.push(skater.player_input.physical.state.category_12 == 500);
        trace.published.push(motion.gesture_publication.map(|p| p.gesture));
        if tick == sample {
            trace.pose = skater.animation.pose.clone();
        }
    }
    trace
}

#[test]
#[ignore = "requires private stock graph/animation/physics assets"]
fn dpad_plays_the_selected_gesture_and_releases_it() {
    let held = 60..150;
    let t = run(None, |t| if held.contains(&t) { UP } else { 0 }, 420, 130);
    let first = t.active.iter().position(|a| *a);
    let last = t.active.iter().rposition(|a| *a);
    eprintln!("gesture channel alive ticks {first:?}..={last:?}");
    assert!(!t.active[..60].iter().any(|a| *a), "no gesture before the press");
    assert!(t.active[70..150].iter().all(|a| *a), "the gesture keeps playing while Up is held");
    assert!(!t.active[419], "the gesture plays its exit and ends after release");
    // Uncustomised skaters use ResetGestureSet: Up is table entry 0 (air guitar).
    assert_eq!(t.published[100], Some(0));

    // The emote visibly moves the skater compared with an idle run.
    let idle = run(None, |_| 0, 131, 130);
    assert!(!idle.active.iter().any(|a| *a));
    let moved = t
        .pose
        .iter()
        .zip(&idle.pose)
        .flat_map(|(a, b)| (0..4).map(move |i| (a.rotation[i] - b.rotation[i]).abs()))
        .fold(0.0f32, f32::max);
    eprintln!("largest local rotation difference vs idle: {moved}");
    assert!(moved > 0.05, "gesture did not change the pose ({moved})");

    // LB + D-pad is the stock session-marker binding, not a gesture.
    let marker = run(None, |t| if held.contains(&t) { UP | LB } else { 0 }, 160, 0);
    assert!(!marker.active.iter().any(|a| *a), "LB + Up must not gesture");
}

#[test]
#[ignore = "requires private stock graph/animation/physics assets"]
fn tapped_custom_gesture_plays_through_on_and_off_the_board() {
    // Down = Yard sale (36), as chosen in Create-a-Skater.
    let chosen = Some([0, 36, 2, 3]);
    let tap = run(chosen, |t| if (60..63).contains(&t) { DOWN } else { 0 }, 400, 0);
    let alive = tap.active.iter().filter(|a| **a).count();
    eprintln!("tapped gesture alive for {alive} ticks");
    assert!(alive > 30, "a tap plays into, cycle and out ({alive} ticks)");
    assert!(tap.published.contains(&Some(36)));
    assert!(!tap.active[399]);

    // Y steps off the board; the gesture then plays on foot.
    let walk = run(chosen, |t| match t {
        20..24 => Y,
        200..260 => DOWN,
        _ => 0,
    }, 320, 0);
    let off = walk.offboard.iter().position(|o| *o);
    eprintln!("offboard from tick {off:?}; gesture alive on foot: {}", walk.active[200..260].iter().filter(|a| **a).count());
    assert!(walk.offboard[190], "Y should have left the board");
    assert!(walk.active[210..260].iter().all(|a| *a), "the gesture plays while walking");
}

/// Review I1 (online fixes 11-01): a hold that lets the player emote (a mode's intro and countdown, bug #44/#45) lets
/// the D-pad gestures past its force brake, on the board and on foot; a hold that doesn't keeps them out.
#[test]
#[ignore = "requires private stock graph/animation/physics assets"]
fn a_hold_that_allows_emotes_lets_them_past_its_force_brake() {
    let up = |t: u32| if (60..150).contains(&t) { UP } else { 0 };
    let emoting = run_held(None, up, |_| Some(true), 150, 0);
    assert!(emoting.active[70..150].iter().all(|a| *a), "the emote plays on the board under the brake");
    let blocked = run_held(None, up, |_| Some(false), 150, 0);
    assert!(!blocked.active.iter().any(|a| *a), "a hold without emotes keeps them out");
    // Y steps off the board unheld; then held from tick 190, Down (Yard sale, as chosen in Create-a-Skater) on foot.
    let chosen = Some([0, 36, 2, 3]);
    let buttons = |t: u32| match t {
        20..24 => Y,
        200..260 => DOWN,
        _ => 0,
    };
    let walk = run_held(chosen, buttons, |t| (t >= 190).then_some(true), 260, 0);
    assert!(walk.offboard[190], "Y should have left the board");
    assert!(walk.active[210..260].iter().all(|a| *a), "the emote plays on foot under the brake");
    let still = run_held(chosen, buttons, |t| (t >= 190).then_some(false), 260, 0);
    assert!(!still.active[200..260].iter().any(|a| *a), "on foot, a hold without emotes keeps them out");
}
