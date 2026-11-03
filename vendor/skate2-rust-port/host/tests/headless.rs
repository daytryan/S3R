//! The host in process on the user's Skate 2 disc data: boot, the spike's run, the minimal set, the camera, the
//! placements, the skater's pose and the trick events. Each test needs the disc, so each is ignored by default: run
//! them with `--ignored` and `S2HOST_TEST_DISC`, `S2HOST_TEST_MIN` and `S2HOST_TEST_READS` set.

mod support;

use std::f32::consts::{PI, TAU};
use std::time::{Duration, Instant};

use skate2_physics_host::Host;
use skate2_physics_protocol::{Boot, Frame, PORT_BONES, Pad, Place, Profile, TICK_SECONDS, Tricks};
use support::scorables::SCORABLES;
use support::{host, host_with, names, push, record_reads, right_point, slide_script, spike_script};

/// The spike's run: 15 s at 60 Hz.
const TICKS: u32 = 900;

/// The frames of the spike's run on the data folder `root_var` names, one for each of its ticks.
fn spike_run(root_var: &str) -> Vec<Frame> {
    let mut host = host(root_var);
    (0..TICKS)
        .map(|tick| host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{root_var}, spike tick {tick}: {why}")))
        .collect()
}

/// The frame the spike printed at `seconds`: it prints its tick `t`, counted from 0, as `t / 60` s.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn at(frames: &[Frame], seconds: f64) -> &Frame {
    &frames[(seconds * 60.0).round() as usize]
}

fn name<T: PartialEq + Copy>(table: &[(T, &'static str)], id: T) -> &'static str {
    table.iter().find(|(key, _)| *key == id).map_or("?", |(_, name)| name)
}

/// The physics, action and motion states, as the spike printed them.
fn states(frame: &Frame) -> String {
    let (action, motion, physics) = frame.state;
    format!("{} / {} / {}", name(names::PHYSICS, physics), name(names::ACTION, action), name(names::MOTION, motion))
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(&b).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt()
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn boots_within_10_seconds() {
    record_reads("boots_within_10_seconds");
    let started = Instant::now();
    let _host = host("S2HOST_TEST_DISC");
    let took = started.elapsed();
    println!("boot took {took:?} (the spike: 2.5 s)");
    assert!(took < Duration::from_secs(10), "boot took {took:?}");
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_spike_run_is_reproduced() {
    record_reads("the_spike_run_is_reproduced");
    let frames = spike_run("S2HOST_TEST_DISC");

    // The wall crash ends in the native respawn.
    let respawn = at(&frames, 14.25);
    println!("14.25 s: deck {:?}  {}", respawn.deck, states(respawn));
    assert_eq!(name(names::PHYSICS, respawn.state.2), "TELEPORTING", "14.25 s: {}", states(respawn));

    let landed = at(&frames, 14.75);
    let off = distance(landed.deck, [818.57, -61.30, 953.17]);
    println!("14.75 s: deck {:?} ({off:.4} m from the spike's)  {}", landed.deck, states(landed));
    assert_eq!(name(names::PHYSICS, landed.state.2), "PHYSICS_GROUND", "14.75 s: {}", states(landed));
    assert!(off <= 0.05, "14.75 s: the deck is {off} m from the spike's");
}

#[test]
#[ignore = "needs the Skate 2 disc and its minimal set: S2HOST_TEST_DISC, S2HOST_TEST_MIN, S2HOST_TEST_READS"]
fn the_minimal_set_runs_the_same() {
    record_reads("the_minimal_set_runs_the_same");
    let disc = spike_run("S2HOST_TEST_DISC");
    let min = spike_run("S2HOST_TEST_MIN");
    let (tick, worst) = disc
        .iter()
        .zip(&min)
        .map(|(disc, min)| distance(disc.deck, min.deck))
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("a run has ticks");
    println!("minimal set: over {TICKS} ticks the deck is at most {worst} m from the disc run's");
    assert!(worst <= 1e-4, "spike tick {tick}: the deck is {worst} m from the disc run's");
}

/// Row 4 of the spec's unconfirmed list: the port's camera without its renderer.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_camera_runs_headless() {
    record_reads("the_camera_runs_headless");
    let frames = spike_run("S2HOST_TEST_DISC");
    // A tick changes the camera when its position differs from the tick before's.
    let changed = frames.windows(2).filter(|pair| pair[0].camera.position != pair[1].camera.position).count();
    let (tick, farthest) = frames
        .iter()
        .map(|frame| distance(frame.camera.position, frame.deck))
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("a run has ticks");
    let steps = TICKS - 1;
    println!("camera: moved on {changed} of the {steps} ticks after the first");
    println!("camera: at most {farthest:.2} m from the deck (spike tick {tick})");
    assert!(changed >= 800, "the camera moved on only {changed} ticks");
    assert!(farthest <= 10.0, "spike tick {tick}: the camera is {farthest} m from the deck");
}

// ---- placements ------------------------------------------------------------------------------------------------

/// The spike's skater by the wall it crashed into: its deck at 14.25 s, before the respawn.
const TARGET: [f32; 3] = [817.62, -61.34, 946.58];
const YAW: f32 = 1.0;
/// The spike ticks run before the first two tests place the skater.
const BEFORE_PLACING: u32 = 120;
/// The spike tick after which the trail test's skater is put back: it still stands at its spawn, before its first
/// push at 1.0 s.
const STANDING: u32 = 30;
/// The spike ticks the trail test rides before its placement: four pushes from the spawn, still rolling at 5 s.
const RIDDEN: u32 = 300;
/// The ticks the trail test may run past the spike's run, with the pad left alone, for the respawn.
const AFTER_THE_RUN: u32 = 600;
/// The spike's spawn `F_WF_Waterfront` (`BAM_Sim.loc`): the x and z of its authored forward axis.
const SPAWN_FORWARD: [f32; 3] = [0.347_115, 0.0, 0.937_823];
/// The ticks a placement may take to settle.
const SETTLE_TICKS: u32 = 6;
/// `frame.category` for a wipeout (frame.rs).
const WIPEOUT: u8 = 4;

/// `speed` m/s along `yaw` (radians about +y from +z).
fn along(yaw: f32, speed: f32) -> [f32; 3] {
    [speed * yaw.sin(), 0.0, speed * yaw.cos()]
}

/// The heading of `direction`, in radians about +y from +z, as a placement's yaw.
fn yaw_of(direction: [f32; 3]) -> f32 {
    direction[0].atan2(direction[2])
}

/// `a - b` wrapped into `-PI..PI`. NaN stays NaN.
fn angle_between(a: f32, b: f32) -> f32 {
    (a - b + PI).rem_euclid(TAU) - PI
}

/// `a` is within `limit` m of `b` in x and in z. False when either is NaN.
fn within_xz(a: [f32; 3], b: [f32; 3], limit: f32) -> bool {
    (a[0] - b[0]).abs() <= limit && (a[2] - b[2]).abs() <= limit
}

/// What `settle` saw.
struct Settled {
    /// The frame that said settled.
    frame: Frame,
    /// Its `n`.
    ticks: u32,
    /// The arrival: the first frame with the deck at the placement in x and z, the one on which physics moved the
    /// bodies there. Its `n`, the frame and the deck's nose after it.
    arrival: Option<(u32, Frame, [f32; 3])>,
}

/// Ticks `host` with `pad(n)` for its `n`th tick after placing `generation` at `position`, until a frame says the
/// placement has settled.
fn settle(test: &str, host: &mut Host, generation: u32, position: [f32; 3], pad: impl Fn(u32) -> Pad) -> Settled {
    let mut arrival = None;
    for after in 1..=SETTLE_TICKS {
        let frame = host.tick(pad(after)).unwrap_or_else(|why| panic!("{test}, placed tick {after}: {why}"));
        let nose = host.deck_nose().unwrap_or_else(|| panic!("{test}, placed tick {after}: no deck nose"));
        println!(
            "{test}: placed tick {after}: placed {:?}  deck {:?}  nose {nose:?}  speed {:.3}  wheels down {}  \
             category {}  {}",
            frame.placed,
            frame.deck,
            frame.speed,
            frame.audio.num_wheels_in_contact,
            frame.category,
            states(&frame)
        );
        if arrival.is_none() && within_xz(frame.deck, position, 0.15) {
            arrival = Some((after, frame.clone(), nose));
        }
        if frame.placed == Some((generation, true)) {
            return Settled { frame, ticks: after, arrival };
        }
        assert_eq!(frame.placed, Some((generation, false)), "{test}, placed tick {after}");
    }
    panic!("{test}: the placement {generation} did not settle within {SETTLE_TICKS} ticks");
}

/// A host after the spike's first `BEFORE_PLACING` ticks, then placed by `place` with the pad left alone: the host,
/// the placement's generation and what settling it saw.
fn placed(test: &str, place: &Place) -> (Host, u32, Settled) {
    let mut host = host("S2HOST_TEST_DISC");
    for tick in 0..BEFORE_PLACING {
        host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{test}, spike tick {tick}: {why}"));
    }
    let generation = host.place(place).unwrap_or_else(|why| panic!("{test}: {why}"));
    let settled = settle(test, &mut host, generation, place.position, |_| Pad::default());
    (host, generation, settled)
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn a_placement_settles_at_its_target() {
    const TEST: &str = "a_placement_settles_at_its_target";
    record_reads(TEST);
    let place = Place { position: TARGET, yaw: YAW, velocity: [0.0; 3], end_line: false };
    let (host, generation, settled) = placed(TEST, &place);
    let frame = &settled.frame;
    // The port raises the spawn by 0.1 m and adjusts its height, so only x and z are the target's.
    let off = [frame.deck[0] - TARGET[0], frame.deck[2] - TARGET[2]];
    println!("placement {generation} settled after {} ticks: deck {:?}", settled.ticks, frame.deck);
    println!(
        "  off the target by {:.4} m in x and {:.4} m in z (y {:.4} m)",
        off[0],
        off[1],
        frame.deck[1] - TARGET[1]
    );
    // The heading is the arrival's: from the next tick the idle stance turns the deck, natively, as it does after
    // the boot spawn (VENDORED.md, S1 finding for row 3).
    let (arrived, _, nose) = settled.arrival.as_ref().expect("the deck reaches the target in x and z");
    let heading = yaw_of(*nose);
    let error = angle_between(heading, YAW);
    println!(
        "  arrival (placed tick {arrived}): deck nose {nose:?}: heading {heading:.4} rad, {error:.4} rad from {YAW}"
    );
    let settled_nose = host.deck_nose().expect("the deck's nose after a tick");
    let drift = angle_between(yaw_of(settled_nose), YAW);
    println!(
        "  settled (placed tick {}): deck nose {settled_nose:?}: {drift:.4} rad from {YAW} (the native idle drift)",
        settled.ticks
    );
    assert!(off.iter().all(|off| off.abs() <= 0.15), "the deck is {off:?} m off the target in x and z");
    assert!(error.abs() <= 0.02, "the heading on arrival is {error} rad off");
}

/// Row 3 of the spec's unconfirmed list: a large board velocity right after a placement.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn a_placement_carries_its_speed() {
    const TEST: &str = "a_placement_carries_its_speed";
    record_reads(TEST);
    let place = Place { position: TARGET, yaw: YAW, velocity: along(YAW, 5.0), end_line: false };
    let (mut host, generation, settled) = placed(TEST, &place);
    let frame = &settled.frame;
    println!(
        "placement {generation} settled after {} ticks at {:.3} m/s, category {}",
        settled.ticks, frame.speed, frame.category
    );
    let after: Vec<Frame> = (1..=60)
        .map(|tick| host.tick(Pad::default()).unwrap_or_else(|why| panic!("tick {tick} after settling: {why}")))
        .collect();
    let mut seen: Vec<u8> = after.iter().map(|frame| frame.category).collect();
    seen.dedup();
    println!("categories over the next 60 ticks, in order: {seen:?}");
    for (tick, frame) in (1..).zip(&after).take(12).chain((1..).zip(&after).skip(12).step_by(12)) {
        println!(
            "  +{tick}: deck {:?}  speed {:.3}  wheels down {}  category {}  {}",
            frame.deck,
            frame.speed,
            frame.audio.num_wheels_in_contact,
            frame.category,
            states(frame)
        );
    }
    assert!((frame.speed - 5.0).abs() <= 0.5, "speed {} on the settling tick", frame.speed);
    for (tick, frame) in (1..).zip(&after) {
        assert_ne!(frame.category, 0, "tick {tick} after settling: no category");
        assert_ne!(frame.category, WIPEOUT, "tick {tick} after settling: a wipeout, {}", states(frame));
    }
    // The speed holds past the touchdown from the port's raised spawn.
    let touchdown = after
        .iter()
        .position(|frame| frame.audio.num_wheels_in_contact > 0)
        .expect("the wheels land within 60 ticks of settling");
    let later = after.get(touchdown + 30).expect("60 ticks reach 30 past the touchdown");
    println!(
        "touchdown at +{}: speed {:.3}; 30 ticks later (+{}): speed {:.3}",
        touchdown + 1,
        after[touchdown].speed,
        touchdown + 31,
        later.speed
    );
    assert!(later.speed >= 4.0, "speed {} 30 ticks after the touchdown", later.speed);
}

fn dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// User 10-22: pushing sounded like a powerslide. The sound takes its slide from the board's own axes, so the frames
/// carry them: up and nose are a right angle of unit axes, a straight roll goes along the nose, and the push the pad
/// asks for reaches the pushing flags.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_sound_gets_the_boards_axes() {
    const TEST: &str = "the_sound_gets_the_boards_axes";
    record_reads(TEST);
    let place = Place { position: TARGET, yaw: YAW, velocity: along(YAW, 5.0), end_line: false };
    let (mut host, _, _) = placed(TEST, &place);
    let rolling: Vec<Frame> = (1..=60)
        .map(|tick| host.tick(Pad::default()).unwrap_or_else(|why| panic!("roll tick {tick}: {why}")))
        .collect();
    let heading = along(YAW, 1.0);
    for (tick, frame) in (1..).zip(&rolling).skip(30) {
        let audio = &frame.audio;
        let (up, nose) = (audio.deck_up, audio.deck_nose);
        let along_nose = dot(nose, [heading[0], heading[1], heading[2], 0.0]);
        println!("+{tick}: up {up:?} nose {nose:?} nose along the roll {along_nose:.3}");
        assert!((dot(up, up) - 1.0).abs() < 1e-3 && (dot(nose, nose) - 1.0).abs() < 1e-3, "+{tick}: not unit");
        assert!(dot(up, nose).abs() < 1e-3, "+{tick}: up and nose not at a right angle");
        assert!(up[1] > 0.9, "+{tick}: up {up:?} on flat ground");
        assert!(along_nose.abs() > 0.95, "+{tick}: the roll is not along the nose ({along_nose})");
    }
    let pushing: Vec<Frame> = (1..=120)
        .map(|tick| host.tick(if tick % 40 < 12 { push() } else { Pad::default() }))
        .collect::<Result<_, _>>()
        .unwrap_or_else(|why| panic!("push: {why}"));
    assert!(pushing.iter().any(|frame| frame.audio.is_generally_pushing), "no push in 120 ticks of pushes");
    for (tick, frame) in (1..).zip(&pushing) {
        let audio = &frame.audio;
        assert!(!audio.is_physically_pushing || audio.is_generally_pushing, "+{tick}: a physical push outside a push");
    }
}

/// The engine's NSV spawn with its skater at rest there (the game's `rides_on_nsv` run, its root after 120 engine
/// ticks): far from the port's boot spawn, so the port's high collections there are not in when the placement settles.
const NSV_REST: [f32; 3] = [765.8626, -62.339_645, 1241.0247];
/// The board's creep at that rest, which the switch carries over.
const NSV_CREEP: [f32; 3] = [0.013_725_603, -0.004_552_042_6, 0.121_166_77];
/// Below this the deck has gone into the ground there: at rest it sits at -62.343, sunk with its wheels through it at
/// -62.43, and fallen through it below -64.
const NSV_FLOOR: f32 = -62.40;

/// User 10-22: switching Skate 2 physics on right after the sim booted dropped the skater through the map. One tick
/// after the boot, a placement far from the boot spawn settled before native Skate 2's `IsAreaLoaded` held there, and
/// the board fell through ground that streamed in after it (tick 1: fell through on every run; tick 20: sunk on one of
/// two). The placement waits for that ground, as native Skate 2's teleport waits behind its load screen.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn a_placement_waits_for_its_ground() {
    const TEST: &str = "a_placement_waits_for_its_ground";
    record_reads(TEST);
    for (run, before) in [1u32, 1, 2, 3, 8, 20].into_iter().enumerate() {
        let mut host = host_with("S2HOST_TEST_DISC", &Boot { mode: 1, ..Boot::default() });
        host.warm_up().unwrap_or_else(|why| panic!("{TEST}, run {run}: warm-up: {why}"));
        for tick in 1..before {
            host.tick(Pad::default()).unwrap_or_else(|why| panic!("{TEST}, run {run}: tick {tick}: {why}"));
        }
        let place = Place { position: NSV_REST, yaw: 0.0, velocity: NSV_CREEP, end_line: false };
        let generation = host.place(&place).unwrap_or_else(|why| panic!("{TEST}, run {run}: {why}"));
        let mut settled = None;
        let (mut lowest, mut contact) = (f32::MAX, None);
        for after in 1..=300u32 {
            let frame = host.tick(Pad::default()).unwrap_or_else(|why| panic!("{TEST}, run {run}: +{after}: {why}"));
            match frame.placed {
                Some((placed, true)) if placed == generation => {
                    settled.get_or_insert(after);
                }
                Some((placed, false)) if placed == generation && settled.is_none() => {}
                other => panic!("{TEST}, run {run}: +{after}: placed {other:?}, after settling at {settled:?}"),
            }
            let Some(from) = settled else { continue };
            lowest = lowest.min(frame.deck[1]);
            if contact.is_none() && frame.audio.num_wheels_in_contact > 0 {
                contact = Some(after - from);
            }
            if after - from >= 90 {
                break;
            }
        }
        println!(
            "{TEST}, run {run} ({before} ticks after the boot): settled at +{settled:?}, first wheel contact {contact:?} \
             ticks later, lowest deck {lowest:.3}"
        );
        assert!(settled.is_some(), "run {run}: the placement never settled");
        assert!(contact.is_some(), "run {run}: no wheel touched the ground in 90 ticks after settling");
        assert!(lowest >= NSV_FLOOR, "run {run}: the deck went down to {lowest} (the ground is at -62.427)");
    }
}

/// The breadcrumb trail starts over at a placement, as after a map travel: the bail after it respawns by the bail,
/// not back on the trail from before the placement. Native Skate 2's respawn goes first to the bail's own spot, so the
/// placement's crumb itself is checked, not where the bail lands.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn a_placement_starts_the_respawn_trail() {
    const TEST: &str = "a_placement_starts_the_respawn_trail";
    record_reads(TEST);
    let mut host = host("S2HOST_TEST_DISC");
    let mut standing = None;
    for tick in 0..RIDDEN {
        let frame = host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("spike tick {tick}: {why}"));
        if tick == STANDING - 1 {
            standing = Some(frame.deck);
        }
        if tick == RIDDEN - 1 {
            println!("{TEST}: spike tick {tick}: deck {:?}  speed {:.3}  {}", frame.deck, frame.speed, states(&frame));
        }
    }
    let standing = standing.expect("the spike's first ticks");
    let (before, _) = host.breadcrumbs().expect("a trail before the placement");
    println!("{TEST}: the trail before the placement, newest first: {} crumbs {before:?}", before.len());
    // The ride left crumbs away from where the skater is put back, so the trail's starting over shows.
    assert!(
        before.iter().any(|crumb| !within_xz(*crumb, standing, 1.0)),
        "the ride left no crumb away from the placement"
    );

    // The skater put back where it stood, facing its spawn's way, then the spike's run from there: pushes, ollie,
    // kickflip and the wall crash at about 9.9 s.
    let yaw = yaw_of(SPAWN_FORWARD);
    let place = Place { position: standing, yaw, velocity: [0.0; 3], end_line: false };
    let generation = host.place(&place).unwrap_or_else(|why| panic!("{TEST}: {why}"));
    println!("{TEST}: placed at {:?} facing {yaw:.4} rad after spike tick {}", place.position, RIDDEN - 1);
    let settled = settle(TEST, &mut host, generation, place.position, |after| spike_script(STANDING - 1 + after));

    // (i) The trail is the placement's one crumb.
    let (trail, spot) = host.breadcrumbs().expect("a trail after the placement");
    println!("{TEST}: the trail once settled, newest first: {} crumbs {trail:?}, wipeout spot {spot:?}", trail.len());
    let newest = *trail.first().expect("the placement drops a crumb");
    assert!(
        within_xz(newest, place.position, 0.15),
        "the newest crumb {newest:?} is not the placement {:?} in x and z",
        place.position
    );
    assert_eq!(trail.len(), 1, "crumbs from before the placement remain: {trail:?}");
    assert!(
        trail.iter().all(|crumb| within_xz(*crumb, place.position, 0.15)),
        "a crumb away from the placement remains: {trail:?}"
    );

    // (ii) The wall crash respawns by the crash, far from the trail before the placement.
    let mut wiped = None;
    let mut teleported = None;
    let mut landed = None;
    let mut last_spot = spot;
    for tick in STANDING + settled.ticks..TICKS + AFTER_THE_RUN {
        let frame = host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("spike tick {tick}: {why}"));
        let physics = name(names::PHYSICS, frame.state.2);
        if tick % 60 == 0 {
            println!("spike tick {tick}: deck {:?}  speed {:.3}  {}", frame.deck, frame.speed, states(&frame));
        }
        if wiped.is_none() && frame.category == WIPEOUT {
            println!("spike tick {tick}: wipeout, deck {:?}  {}", frame.deck, states(&frame));
            wiped = Some((tick, frame.deck));
        } else if wiped.is_some() && teleported.is_none() && physics == "TELEPORTING" {
            println!("spike tick {tick}: the respawn, deck {:?}; the wipeout spot before it {last_spot:?}", frame.deck);
            teleported = Some((tick, last_spot));
        } else if teleported.is_some() && physics == "PHYSICS_GROUND" {
            landed = Some((tick, frame));
            break;
        }
        if teleported.is_none() {
            last_spot = host.breadcrumbs().expect("a trail during the run").1;
        }
    }
    let (wiped, wiped_deck) =
        wiped.unwrap_or_else(|| panic!("no wipeout in the spike's run after placement {generation}"));
    let (_, spot) = teleported.unwrap_or_else(|| panic!("no respawn after the wipeout at spike tick {wiped}"));
    let (tick, frame) = landed.expect("the respawned skater lands within the spike's run");
    let from_spot = distance(frame.deck, spot);
    let from_wipeout = distance(frame.deck, wiped_deck);
    let nearest = before.iter().map(|crumb| distance(frame.deck, *crumb)).fold(f32::INFINITY, f32::min);
    println!(
        "spike tick {tick}: landed at {:?}: {from_spot:.3} m from the wipeout spot {spot:?} ({:.3} m in x and z), \
         {from_wipeout:.3} m from the deck at the wipeout, {nearest:.3} m from the nearest crumb before the placement, \
         {:.3} m from the placement",
        frame.deck,
        ((frame.deck[0] - spot[0]).powi(2) + (frame.deck[2] - spot[2]).powi(2)).sqrt(),
        distance(frame.deck, place.position)
    );
    assert!(from_spot <= 1.0, "the respawn landed {from_spot} m from the wipeout spot");
    assert!(
        before.iter().all(|crumb| distance(frame.deck, *crumb) > 20.0),
        "the respawn landed {nearest} m from a crumb from before the placement"
    );
}

// ---- the pose --------------------------------------------------------------------------------------------------

/// `Frame::bones` indices (`PORT_BONES`).
const HIPS: usize = 1;
const BOARD: usize = 25;

/// The translation of a frame matrix: row 3 (frame.rs).
fn translation(matrix: &[f32; 16]) -> [f32; 3] {
    [matrix[12], matrix[13], matrix[14]]
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_pose_has_our_first_32_bones() {
    record_reads("the_pose_has_our_first_32_bones");
    let mut host = host("S2HOST_TEST_DISC");
    host.tick(spike_script(0)).unwrap_or_else(|why| panic!("spike tick 0: {why}"));
    let names = host.bone_names();
    println!("the presentation pose has {} bones", names.len());
    for (index, ours) in PORT_BONES.iter().enumerate() {
        println!("  {index:2}: {:<18} ours {ours}", names.get(index).copied().unwrap_or("-"));
    }
    assert!(names.len() >= PORT_BONES.len(), "the pose has only {} bones", names.len());
    for (index, (name, ours)) in names.iter().zip(PORT_BONES).enumerate() {
        assert!(name.eq_ignore_ascii_case(ours), "bone {index} is {name}, ours is {ours}");
    }
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn bones_are_world_matrices() {
    record_reads("bones_are_world_matrices");
    let mut host = host("S2HOST_TEST_DISC");
    // The worst distance and its spike tick; NaN counts as worst.
    let mut worst_hips = (0, 0.0_f32);
    let mut worst_board = (0, 0.0_f32);
    let mut hips_moved = 0;
    let mut last_hips = None;
    for tick in 0..TICKS {
        let frame = host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("spike tick {tick}: {why}"));
        let hips = host.hips_position().unwrap_or_else(|| panic!("spike tick {tick}: no skater_hips_position"));
        assert!(frame.root.iter().all(|value| value.is_finite()), "spike tick {tick}: root {:?}", frame.root);
        assert_eq!(frame.root[15], 1.0, "spike tick {tick}: root {:?}", frame.root);
        for (index, bone) in frame.bones.iter().enumerate() {
            assert!(bone.iter().all(|value| value.is_finite()), "spike tick {tick}: bones[{index}] {bone:?}");
            assert_eq!(bone[15], 1.0, "spike tick {tick}: bones[{index}] {bone:?}");
        }
        let off_hips = distance(translation(&frame.bones[HIPS]), hips);
        let off_board = distance(translation(&frame.bones[BOARD]), frame.deck);
        if !(off_hips <= worst_hips.1) {
            worst_hips = (tick, off_hips);
        }
        if !(off_board <= worst_board.1) {
            worst_board = (tick, off_board);
        }
        if last_hips.is_some_and(|last| last != frame.bones[HIPS]) {
            hips_moved += 1;
        }
        last_hips = Some(frame.bones[HIPS]);
        assert!(off_hips <= 0.3, "spike tick {tick}: the hips bone is {off_hips} m from skater_hips_position {hips:?}");
        assert!(off_board <= 0.1, "spike tick {tick}: skateboard_root is {off_board} m from the deck {:?}", frame.deck);
    }
    println!("pose: the hips bone moved on {hips_moved} of the {} ticks after the first", TICKS - 1);
    println!("pose: hips at most {:.4} m from skater_hips_position (spike tick {})", worst_hips.1, worst_hips.0);
    println!("pose: skateboard_root at most {:.4} m from the deck (spike tick {})", worst_board.1, worst_board.0);
}

// ---- trick events ----------------------------------------------------------------------------------------------

/// The neutral ticks after a pattern's points, for its trick to begin.
const AFTER_POINTS: usize = 90;
/// The speed the pattern tricks are placed at, in m/s.
const ROLLING: f32 = 4.0;
/// Scorable ids (`support::scorables`).
const FSPOWERSLIDE: u16 = 0;
const BSPOWERSLIDE: u16 = 1;
const NOSEMANUAL: u16 = 2;
const OLLIE: u16 = 128;
/// The spike's ollie at 6.0 s: the ticks before it are pushes and coasting.
const BEFORE_THE_OLLIE: u32 = 360;
/// The spike's wall bail: the ticks from the kickflip's landing (8.72 s) past the respawn (14.25 s).
const THE_WALL: std::ops::Range<usize> = 524..860;

/// Whether `tricks` has a trick event or an active trick.
fn any_trick(tricks: &Tricks) -> bool {
    !(tricks.begun.is_empty() && tricks.converted.is_empty() && tricks.ended.is_empty() && tricks.active.is_empty())
}

/// The events of `tricks` that are set, for a trace line.
fn events(tricks: &Tricks) -> String {
    let mut out = Vec::new();
    for (name, ids) in [("begun", &tricks.begun), ("converted", &tricks.converted), ("active", &tricks.active)] {
        if !ids.is_empty() {
            out.push(format!("{name} {ids:?}"));
        }
    }
    if !tricks.ended.is_empty() {
        out.push(format!("ended {:?}", tricks.ended));
    }
    for (name, set) in [
        ("sequence_begun", tricks.sequence_begun),
        ("sequence_completed", tricks.sequence_completed),
        ("completed_successfully", tricks.completed_successfully),
        ("interrupted", tricks.interrupted),
        ("just_landed", tricks.just_landed),
        ("grinding", tricks.grinding),
    ] {
        if set {
            out.push(name.to_owned());
        }
    }
    out.join(", ")
}

/// The line meter of `tricks`, for a trace line.
fn line(tricks: &Tricks) -> String {
    format!(
        "line_score {} line_points {}/{} multiplier {:.2}",
        tricks.line_score, tricks.line_points, tricks.line_max_points, tricks.multiplier
    )
}

/// One trace line for `frame`, the `tick`th of the run `run`.
fn trace(test: &str, run: &str, tick: impl std::fmt::Display, frame: &Frame) {
    println!(
        "{test}: {run} tick {tick}: {}; {}  speed {:.2}  category {}  {}",
        events(&frame.tricks),
        line(&frame.tricks),
        frame.speed,
        frame.category,
        states(frame)
    );
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn hud_frames_drain_on_each_call() {
    const TEST: &str = "hud_frames_drain_on_each_call";
    record_reads(TEST);
    let mut host = host("S2HOST_TEST_DISC");
    let tick = |host: &mut Host, n: u32| {
        host.tick_port(Pad::default()).unwrap_or_else(|why| panic!("{TEST}, port tick {n}: {why}"));
    };
    // Before the first snapshot, nothing gathers.
    tick(&mut host, 1);
    tick(&mut host, 2);
    let unsubscribed = host.port().take_scoring_hud_frames().len();
    let snapshot = host.port().try_visual_snapshot().map(|_| ()).map_err(|why| why.to_string());
    tick(&mut host, 3);
    tick(&mut host, 4);
    let first = host.port().take_scoring_hud_frames().len();
    let second = host.port().take_scoring_hud_frames().len();
    // A snapshot drains only its own list, which has gathered every tick since the first snapshot (3, 4 and 5).
    tick(&mut host, 5);
    let snapshot_took = host
        .port()
        .try_visual_snapshot()
        .map(|visual| visual.scoring_hud_frames.len())
        .map_err(|why| why.to_string());
    let after_the_snapshot = host.port().take_scoring_hud_frames().len();
    println!(
        "{TEST}: 2 ticks before the first snapshot: {unsubscribed} frames; the snapshot: {snapshot:?}; 2 ticks after \
         it: {first} frames, then {second}; 1 more tick: the snapshot took {snapshot_took:?}, then this call \
         {after_the_snapshot}"
    );
    assert_eq!(unsubscribed, 0, "frames gathered before the first snapshot");
    snapshot.unwrap_or_else(|why| panic!("{TEST}: the first snapshot: {why}"));
    assert_eq!(first, 2, "the frames of the 2 ticks after the first snapshot");
    assert_eq!(second, 0, "the second call");
    assert_eq!(snapshot_took, Ok(3), "the snapshot's own list, ticks 3-5");
    assert_eq!(after_the_snapshot, 1, "the frame of the tick before a snapshot");
}

/// The most neutral ticks `play` waits, after a placement settles, for the landing bump to end.
const BUMP_TICKS: u32 = 90;

/// What one playing of a pattern trick saw: the neutral ticks it waited for the landing bump, the `(tick, ids)` of each
/// frame with a trick begun (ticks counted from the first point), and the categories in order.
struct Played {
    waited: u32,
    begun: Vec<(u32, Vec<u16>)>,
    categories: Vec<u8>,
}

/// Places `host`'s skater at the Task 4 spot rolling at `ROLLING`, waits with the pad neutral until the landing bump
/// has ended, then ticks `pads` and `AFTER_POINTS` neutral ticks.
///
/// The placement drops the board from the port's raised spawn, and the Motion graph rides the landing out in
/// `RidingIdle/Bumped` (natively, about 35 ticks). There, a shallow antic does not anticipate: the kickflip's and the
/// heelflip's (0.69 and 0.69 of full stick, against the ollie's 0.97) go to `TrickFromSquare` and no trick begins.
fn play(test: &str, host: &mut Host, scorable: &str, pads: &[Pad]) -> Played {
    let place = Place { position: TARGET, yaw: YAW, velocity: along(YAW, ROLLING), end_line: true };
    let generation = host.place(&place).unwrap_or_else(|why| panic!("{test}, {scorable}: {why}"));
    let settled = settle(test, host, generation, TARGET, |_| Pad::default());
    assert!((settled.frame.speed - ROLLING).abs() <= 0.5, "{test}, {scorable}: settled at {} m/s", settled.frame.speed);
    // The settled frame is still in the air (no wheel down): the bump begins at the touchdown, a tick later. Wait
    // until it has begun and ended.
    let mut waited = 0;
    let mut bumped = false;
    loop {
        assert!(waited < BUMP_TICKS, "{test}, {scorable}: {BUMP_TICKS} ticks after settling, bumped {bumped}");
        let frame = host.tick(Pad::default()).unwrap_or_else(|why| panic!("{test}, {scorable}, bump tick: {why}"));
        waited += 1;
        let in_bump = name(names::MOTION, frame.state.1).ends_with("/Bumped");
        if bumped && !in_bump {
            break;
        }
        bumped |= in_bump;
    }
    let mut played = Played { waited, begun: Vec::new(), categories: Vec::new() };
    for (n, pad) in (1_u32..).zip(pads.iter().copied().chain(std::iter::repeat_n(Pad::default(), AFTER_POINTS))) {
        let frame = host.tick(pad).unwrap_or_else(|why| panic!("{test}, {scorable}, tick {n}: {why}"));
        if !frame.tricks.begun.is_empty() {
            played.begun.push((n, frame.tricks.begun.clone()));
        }
        if played.categories.last() != Some(&frame.category) {
            played.categories.push(frame.category);
        }
    }
    played
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn every_pattern_trick_reports_its_own_id() {
    const TEST: &str = "every_pattern_trick_reports_its_own_id";
    record_reads(TEST);
    let mut host = host("S2HOST_TEST_DISC");
    for tick in 0..BEFORE_PLACING {
        host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, spike tick {tick}: {why}"));
    }
    let patterns: Vec<(String, Vec<[f32; 2]>)> =
        host.port().host_right_patterns().into_iter().map(|(name, points)| (name.to_owned(), points)).collect();
    println!(
        "{TEST}: {} right-stick patterns (name, points): {:?}",
        patterns.len(),
        patterns.iter().map(|(name, points)| (name.as_str(), points.len())).collect::<Vec<_>>()
    );
    let mut tested = Vec::new();
    let mut without = Vec::new();
    let mut failing = Vec::new();
    for (id, scorable) in (0_u16..).zip(SCORABLES) {
        // A scorable's pattern has its name, ignoring case: the scorable `kickflip` is the pattern `Kickflip`.
        let Some((pattern, points)) =
            patterns.iter().find(|(name, points)| name.eq_ignore_ascii_case(scorable) && !points.is_empty())
        else {
            without.push(id);
            continue;
        };
        tested.push(id);
        // The pattern's points as right-stick samples, one per tick.
        let pads: Vec<Pad> = points.iter().map(|point| right_point(*point)).collect();
        let played = play(TEST, &mut host, scorable, &pads);
        println!(
            "{TEST}: {id} {scorable} (pattern {pattern}, {} points, one per tick, after {} bump ticks): begun (tick, \
             ids) {:?}; categories {:?}; logs {:?}",
            points.len(),
            played.waited,
            played.begun,
            played.categories,
            host.take_logs()
        );
        if !played.begun.iter().any(|(_, ids)| ids.contains(&id)) {
            failing.push((id, scorable));
        }
    }
    let unmatched: Vec<&str> = patterns
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !SCORABLES.iter().any(|scorable| name.eq_ignore_ascii_case(scorable)))
        .collect();
    println!("{TEST}: {} ids tested: {tested:?}", tested.len());
    println!("{TEST}: {} ids without a pattern: {without:?}", without.len());
    println!("{TEST}: {} patterns named after no scorable: {unmatched:?}", unmatched.len());
    println!("{TEST}: {} ids that did not report themselves: {failing:?}", failing.len());
    assert!(!tested.is_empty(), "no scorable has a pattern of its name");
    assert!(
        failing.is_empty(),
        "{} of the {} pattern tricks did not begin as their own id: {failing:?}",
        failing.len(),
        tested.len()
    );
}

/// The right stick of a nose manual: pushed up, gently (a pattern point's `y` is down).
const MANUAL: [f32; 2] = [0.0, -0.5];
/// The nose manual of `manual_script`, in ticks: 4.5 s to 5.3 s, after the spike's four pushes.
const FIRST_MANUAL: std::ops::Range<u32> = 270..318;
/// The nose manual test's ticks: to 6.0 s.
const MANUAL_TICKS: u32 = 360;
/// The spike tick from which the runs after the kickflip hold a nose manual: 8.9 s, after its landing (8.72 s), in
/// place of the spike's pushes at 9.0 s and 9.7 s.
const AFTER_THE_KICKFLIP: u32 = 534;
/// The tick on which the line test ends the line, 11 ticks into its nose manual's scorable: 9.58 s.
const END_LINE_AT: u32 = 575;
/// The tick on which the line tests release their nose manual, before the wall: 9.83 s.
const MANUAL_RELEASED: u32 = 590;
/// The spike tick from which `slide_into_the_wall` holds the spike's powerslide stick: 9.53 s.
const SLIDE_INTO_THE_WALL: u32 = 572;

/// The nose manual test's script, for its tick `tick` (counted from 0): the spike's four pushes, then the right stick
/// held at `MANUAL` through `FIRST_MANUAL`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn manual_script(tick: u32) -> Pad {
    let s = |sec: f64| (sec * 60.0) as u32;
    if [1.0, 1.9, 2.8, 3.7].iter().any(|&at| (s(at)..s(at) + 12).contains(&tick)) {
        return push();
    }
    if FIRST_MANUAL.contains(&tick) {
        return right_point(MANUAL);
    }
    Pad::default()
}

/// The spike's run to its kickflip, a coast without the pushes at 9.0 s and 9.7 s, then the powerslide of
/// `slide_script` held from `SLIDE_INTO_THE_WALL` into the wall.
fn slide_into_the_wall(tick: u32) -> Pad {
    const SLIDE: u32 = 300;
    if tick < AFTER_THE_KICKFLIP {
        spike_script(tick)
    } else if tick < SLIDE_INTO_THE_WALL {
        Pad::default()
    } else {
        slide_script(SLIDE)
    }
}

/// The spike's run to its kickflip, then a nose manual held from `AFTER_THE_KICKFLIP` to `until`.
fn manual_after_the_kickflip(tick: u32, until: u32) -> Pad {
    if tick < AFTER_THE_KICKFLIP {
        spike_script(tick)
    } else if tick < until {
        right_point(MANUAL)
    } else {
        Pad::default()
    }
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_basic_moves_report_their_ids() {
    const TEST: &str = "the_basic_moves_report_their_ids";
    record_reads(TEST);
    let mut failures = Vec::new();

    // The spike's run: four pushes and a coast until the ollie at 6.0 s, the kickflip, then the wall bail.
    let mut host = host("S2HOST_TEST_DISC");
    let mut took = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    let mut frames = Vec::new();
    for tick in 0..TICKS {
        let started = Instant::now();
        let frame = host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, spike tick {tick}: {why}"));
        let elapsed = started.elapsed();
        took += elapsed;
        slowest = slowest.max(elapsed);
        frames.push(frame);
    }
    println!(
        "{TEST}: Host::tick (port tick, snapshot, trick frames, audio, pose, frame) over the spike's {TICKS} ticks: \
         {:.3} ms on average, {:.3} ms at most",
        took.as_secs_f64() * 1000.0 / f64::from(TICKS),
        slowest.as_secs_f64() * 1000.0
    );
    for (tick, frame) in frames.iter().enumerate() {
        if !frame.tricks.begun.is_empty()
            || !frame.tricks.ended.is_empty()
            || frame.tricks.sequence_completed
            || frame.tricks.interrupted
            || (590..600).contains(&tick)
        {
            trace(TEST, "spike", tick, frame);
        }
    }
    println!("{TEST}: the spike run's logs: {:?}", host.take_logs());
    let coast: Vec<u32> = (0..BEFORE_THE_OLLIE).filter(|&tick| any_trick(&frames[tick as usize].tricks)).collect();
    if !coast.is_empty() {
        failures.push(format!("a trick while pushing and coasting, spike ticks {coast:?}"));
    }
    let ollie: Vec<u16> = frames.iter().flat_map(|frame| frame.tricks.begun.clone()).collect();
    if !ollie.contains(&OLLIE) {
        failures.push(format!("the spike's ollie: begun {ollie:?}"));
    }
    // The spike's own bail comes while coasting, with no trick to interrupt: native Skate 2 sets
    // `wipeout_interrupted_trick` only when the wipeout cuts off a sequence-worthy activity (`module_dispatch.rs`
    // `manage_collector`, `ground.rs` `activity_sequence_worthy`).
    let wipeout = THE_WALL.clone().find(|&tick| frames[tick].category == WIPEOUT);
    let interrupted = THE_WALL.clone().find(|&tick| frames[tick].tricks.interrupted);
    println!("{TEST}: the coasting wall bail: wipeout at spike tick {wipeout:?}, interrupted at {interrupted:?}");
    if wipeout.is_none() || interrupted.is_some() {
        failures.push(format!("the coasting wall bail: wipeout at {wipeout:?}, interrupted at {interrupted:?}"));
    }

    // The spike's wall bail with its powerslide held into the wall: the bail cuts the slide off.
    let mut host = support::host("S2HOST_TEST_DISC");
    let mut bail = None;
    let mut cut = None;
    for tick in 0..TICKS {
        let frame =
            host.tick(slide_into_the_wall(tick)).unwrap_or_else(|why| panic!("{TEST}, slide bail tick {tick}: {why}"));
        if tick >= SLIDE_INTO_THE_WALL && bail.is_none() {
            trace(TEST, "slide bail", tick, &frame);
        }
        if tick >= SLIDE_INTO_THE_WALL && bail.is_none() && frame.category == WIPEOUT {
            bail = Some(tick);
        }
        if tick >= SLIDE_INTO_THE_WALL && cut.is_none() && frame.tricks.interrupted {
            cut = Some(tick);
        }
    }
    println!("{TEST}: the wall bail in a powerslide: wipeout at spike tick {bail:?}, interrupted at {cut:?}");
    if bail.is_none() || cut != bail {
        failures.push(format!("the wall bail in a powerslide: wipeout at {bail:?}, interrupted at {cut:?}"));
    }

    // The spike's powerslide (variant c).
    let mut host = support::host("S2HOST_TEST_DISC");
    let mut slid = Vec::new();
    for tick in 0..BEFORE_THE_OLLIE + 60 {
        let frame = host.tick(slide_script(tick)).unwrap_or_else(|why| panic!("{TEST}, slide tick {tick}: {why}"));
        if any_trick(&frame.tricks) || frame.tricks.sequence_completed {
            trace(TEST, "slide", tick, &frame);
        }
        slid.extend(frame.tricks.begun);
    }
    println!("{TEST}: the powerslide began {slid:?}");
    if !slid.iter().any(|id| [FSPOWERSLIDE, BSPOWERSLIDE].contains(id)) {
        failures.push(format!("the powerslide began {slid:?}, not {FSPOWERSLIDE} or {BSPOWERSLIDE}"));
    }

    // A nose manual after four pushes.
    let mut host = support::host("S2HOST_TEST_DISC");
    let mut manual = Vec::new();
    for tick in 0..MANUAL_TICKS {
        let frame = host.tick(manual_script(tick)).unwrap_or_else(|why| panic!("{TEST}, manual tick {tick}: {why}"));
        if !frame.tricks.begun.is_empty() || !frame.tricks.ended.is_empty() || frame.tricks.sequence_completed {
            trace(TEST, "manual", tick, &frame);
        }
        manual.push(frame.tricks);
    }
    let begun: Vec<u16> = manual.iter().flat_map(|tricks| tricks.begun.clone()).collect();
    let ended: Vec<(u16, u32)> = manual.iter().flat_map(|tricks| tricks.ended.clone()).collect();
    println!("{TEST}: the nose manual: begun {begun:?}, ended {ended:?}");
    if !begun.contains(&NOSEMANUAL) {
        failures.push(format!("the nose manual began {begun:?}"));
    }
    if !ended.iter().any(|&(id, score)| id == NOSEMANUAL && score > 0) {
        failures.push(format!("the nose manual ended {ended:?}, with no ({NOSEMANUAL}, score > 0)"));
    }

    // The spike's kickflip starts a line (line_score 100), which a nose manual after it keeps until the wall.
    let mut host = support::host("S2HOST_TEST_DISC");
    let mut kept = Vec::new();
    for tick in 0..MANUAL_RELEASED {
        let frame = host
            .tick(manual_after_the_kickflip(tick, MANUAL_RELEASED))
            .unwrap_or_else(|why| panic!("{TEST}, kept line tick {tick}: {why}"));
        if tick >= END_LINE_AT {
            kept.push((tick, frame.tricks.line_score));
        }
    }
    let lost: Vec<(u32, u32)> = kept.iter().copied().filter(|&(_, score)| score == 0).collect();
    println!(
        "{TEST}: the line, not ended: line_score {:?} from tick {END_LINE_AT} to {MANUAL_RELEASED}; zero on {lost:?}",
        kept.first().map(|&(_, score)| score)
    );
    if kept.is_empty() || !lost.is_empty() {
        failures.push(format!("the line, not ended, has no score on (tick, score) {lost:?}"));
    }

    // The same run, with host_end_line in the nose manual.
    let mut host = support::host("S2HOST_TEST_DISC");
    let mut before_the_end = None;
    let mut after_the_end = Vec::new();
    for tick in 0..TICKS {
        if tick == END_LINE_AT {
            if !host.port().host_end_line() {
                failures.push("host_end_line found no local player".to_owned());
            }
            println!("{TEST}: before line tick {tick}: host_end_line");
        }
        let frame = host
            .tick(manual_after_the_kickflip(tick, MANUAL_RELEASED))
            .unwrap_or_else(|why| panic!("{TEST}, line tick {tick}: {why}"));
        if tick >= AFTER_THE_KICKFLIP
            && (tick < MANUAL_RELEASED + 30 || any_trick(&frame.tricks) || frame.tricks.sequence_completed)
        {
            trace(TEST, "line", tick, &frame);
        }
        if tick + 1 == END_LINE_AT {
            before_the_end = Some((frame.tricks.line_score, frame.tricks.active.clone()));
        }
        if tick >= END_LINE_AT {
            after_the_end.push((tick, frame.tricks.line_score));
        }
    }
    let (line_before, active_before) = before_the_end.expect("the tick before the line ends");
    let scored: Vec<(u32, u32)> = after_the_end.iter().copied().filter(|&(_, score)| score != 0).collect();
    println!(
        "{TEST}: the line: on the tick before host_end_line line_score {line_before}, active {active_before:?}; from \
         tick {END_LINE_AT} to {TICKS}, nonzero on {scored:?}"
    );
    if line_before == 0 || !active_before.contains(&NOSEMANUAL) {
        failures.push(format!(
            "before host_end_line: line_score {line_before}, active {active_before:?}: no line, or not in a nose manual"
        ));
    }
    if !scored.is_empty() {
        failures.push(format!("line_score after host_end_line at tick {END_LINE_AT}: (tick, score) {scored:?}"));
    }
    assert!(failures.is_empty(), "{TEST}: {failures:#?}");
}

// ---- seed, gestures, game mode and profile (Task 8) ------------------------------------------------------------

/// The seed the repeatable run uses.
const SEED: u64 = 0x5EED;

/// What a seeded run saw: the deck at every tick of the spike's run, the snapshot's pedestrian and vehicle counts at
/// every 60th, and the process-wide generators' state right after the boot, after the first tick, after each 60th
/// tick and at the end.
struct Seeded {
    decks: Vec<[f32; 3]>,
    counts: Vec<(usize, usize)>,
    rng: Vec<([u32; 6], u32)>,
    /// `Host::rng_at_seed`.
    at_seed: Option<([u32; 6], u32)>,
}

/// The spike's run on a host booted with `seed` and exact delivery.
fn seeded_run(seed: u64) -> Seeded {
    let boot = Boot { seed: Some(seed), exact_delivery: Some(true), ..Boot::default() };
    let mut host = host_with("S2HOST_TEST_DISC", &boot);
    let mut decks = Vec::new();
    let mut counts = Vec::new();
    let at_seed = host.rng_at_seed();
    let mut rng = vec![host.rng_state()];
    for tick in 0..TICKS {
        let frame = host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("seeded spike tick {tick}: {why}"));
        decks.push(frame.deck);
        if tick == 0 || tick % 60 == 59 {
            rng.push(host.rng_state());
        }
        if tick % 60 == 0 {
            let counted = host
                .port()
                .try_visual_snapshot()
                .map(|visual| (visual.pedestrians.len(), visual.vehicles.len()))
                .unwrap_or_else(|why| panic!("seeded spike tick {tick}: the snapshot: {why}"));
            counts.push(counted);
        }
    }
    Seeded { decks, counts, rng, at_seed }
}

/// The state `Boot::seed` leaves the generators in: `global_next`'s six words start at the port's initial words plus
/// the seed's 32-bit fold, and the CRT `rand`'s `holdrand` is that fold.
fn state_of_seed(seed: u64) -> ([u32; 6], u32) {
    const INITIAL: [u32; 6] = [0xf22d_0e56, 0x8831_26e9, 0xc624_dd2f, 0x0702_c49c, 0x9e35_3f7d, 0x6fdf_3b64];
    let word = (seed ^ (seed >> 32)) as u32;
    (INITIAL.map(|initial| initial.wrapping_add(word)), word)
}

/// How many words the generator `global_next` has drawn since it was seeded with `seed`: its last word starts at
/// `0x6fdf3b64 + seed` and counts up once per draw.
fn draws(state: &([u32; 6], u32), seed: u64) -> u32 {
    let word = (seed ^ (seed >> 32)) as u32;
    state.0[5].wrapping_sub(0x6fdf_3b64).wrapping_sub(word)
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_same_seed_gives_the_same_run() {
    const TEST: &str = "the_same_seed_gives_the_same_run";
    record_reads(TEST);
    // One after the other: the port's generators are process-wide.
    let Seeded { decks: first, counts: first_counts, rng: first_rng, at_seed: first_seed } = seeded_run(SEED);
    let Seeded { decks: second, counts: second_counts, rng: second_rng, at_seed: second_seed } = seeded_run(SEED);
    // The skater's run does not draw from the generators, so the decks alone cannot show that the seed arrived (they
    // are equal unseeded too). The generators' state right after the seed was applied can: it is the seed's, the same
    // for the same seed and not for another. Past that the draw counts depend on timing (printed below), whatever the
    // seed.
    let other = seeded_run(SEED + 1);
    for (what, rng, seed) in [("first", &first_rng, SEED), ("second", &second_rng, SEED), ("other seed", &other.rng, SEED + 1)] {
        println!(
            "{TEST}: {what}: draws since the seed: after the boot {}, after tick 1 {}, then after each 60th tick {:?}; \
             the state after the boot {:x?}",
            draws(&rng[0], seed),
            draws(&rng[1], seed),
            rng[2..].iter().map(|state| draws(state, seed)).collect::<Vec<_>>(),
            rng[0]
        );
    }
    assert_eq!(first_seed, Some(state_of_seed(SEED)), "the first run's generators after the seed");
    assert_eq!(second_seed, Some(state_of_seed(SEED)), "the second run's generators after the seed");
    assert_eq!(other.at_seed, Some(state_of_seed(SEED + 1)), "the other seed's generators after the seed");
    assert_ne!(state_of_seed(SEED), state_of_seed(SEED + 1));
    let different: Vec<usize> =
        (0..first.len()).filter(|&tick| (0..3).any(|axis| first[tick][axis].to_bits() != second[tick][axis].to_bits())).collect();
    println!(
        "{TEST}: seed {SEED:#x}: {} of {} decks differ bit for bit (first at spike tick {:?}); (pedestrians, vehicles) \
         at every 60th tick: first {first_counts:?}, second {second_counts:?}",
        different.len(),
        first.len(),
        different.first()
    );
    assert!(different.is_empty(), "{} decks differ, the first at spike tick {:?}", different.len(), different.first());
    assert_eq!(first_counts, second_counts, "the pedestrian and vehicle counts differ");
}

/// XInput's D-pad and face buttons.
const DPAD_UP: u16 = 0x0001;
const DPAD_DOWN: u16 = 0x0002;
const DPAD_LEFT: u16 = 0x0004;
const DPAD_RIGHT: u16 = 0x0008;
const FACE_Y: u16 = 0x8000;

fn pad_of(buttons: u16) -> Pad {
    Pad { buttons, ..Pad::default() }
}

/// Ticks `host` `ticks` times with `pad` held; the last frame.
fn hold(host: &mut Host, pad: Pad, ticks: u32) -> Frame {
    let mut last = None;
    for tick in 0..ticks {
        last = Some(host.tick(pad).unwrap_or_else(|why| panic!("held tick {tick}: {why}")));
    }
    last.expect("at least one tick")
}

/// Presses Y (off board toggle), then waits with the pad neutral until the Motion graph has left the dismount of
/// `OffBoard/OBGround` for a state that stays (its locomotion or its idle), and the camera has had time to settle
/// behind the skater. The frame after that.
fn get_off_the_board(test: &str, host: &mut Host) -> Frame {
    hold(host, pad_of(FACE_Y), 3);
    let mut last = String::new();
    let mut since = 0;
    for waited in 0..600 {
        let frame = host.tick(Pad::default()).unwrap_or_else(|why| panic!("{test}, off-board wait {waited}: {why}"));
        let motion = name(names::MOTION, frame.state.1);
        if motion != last {
            println!("{test}: off-board wait tick {waited}: {motion}");
            last = motion.to_owned();
            since = 0;
        }
        since += 1;
        if last.starts_with("OffBoard/OBGround") && !last.contains("Dismount") && since >= 30 {
            println!("{test}: off the board after {} ticks: {}", waited + 1, states(&frame));
            return hold(host, Pad::default(), 120);
        }
    }
    panic!("{test}: no settled off-board state 600 ticks after Y, last {last}");
}

/// The 35 gestures in `eAnimGestureNames` order (world-objects 8).
const GESTURES: [&str; 35] = [
    "sky", "what", "snap", "thumbdown", "pumphi", "fonz", "bruce", "shrug", "pumplo", "devil", "fu", "throwarms",
    "doublegun", "peace", "shaka", "spock", "airguitar", "airplane", "flex", "boxing", "clothesline", "paper", "rock",
    "scissors", "dunno", "fingerpoint", "fingerwag", "fists", "fliptable", "freedom", "getouttahere", "push",
    "raisetheroof", "wings", "axe",
];

/// Presses `button` (a rising edge, neither bumper held) and answers the gestures the port started in the next
/// ticks, then waits for the gesture to end.
fn press_gesture(test: &str, host: &mut Host, button: u16) -> Vec<u32> {
    host.take_started_gestures();
    hold(host, pad_of(button), 3);
    let mut started = host.take_started_gestures();
    for _ in 0..10 {
        started.extend(host.take_started_gestures());
        host.tick(Pad::default()).unwrap_or_else(|why| panic!("{test}, after the press: {why}"));
    }
    started.extend(host.take_started_gestures());
    // The gesture's channel must end before the next press can start one.
    hold(host, Pad::default(), 360);
    started
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn each_slot_fires_its_gesture() {
    const TEST: &str = "each_slot_fires_its_gesture";
    record_reads(TEST);
    const SLOTS: [u32; 4] = [4, 5, 6, 34];
    // The slots are Up, Down, Left and Right, in that order (world-objects 8). The test presses Up, Right, Down, Left.
    let presses = [("up", DPAD_UP, 0), ("right", DPAD_RIGHT, 3), ("down", DPAD_DOWN, 1), ("left", DPAD_LEFT, 2)];

    let mut host = host("S2HOST_TEST_DISC");
    hold(&mut host, Pad::default(), 30);
    get_off_the_board(TEST, &mut host);
    host.set_gestures(SLOTS).unwrap_or_else(|why| panic!("{TEST}: {why}"));
    assert_eq!(host.local_gestures(), Some(SLOTS), "the slots the actor reads");
    let mut failures = Vec::new();
    for (direction, button, slot) in presses {
        let started = press_gesture(TEST, &mut host, button);
        let wanted = SLOTS[slot];
        println!(
            "{TEST}: D-pad {direction} (slot {slot}): started {:?}, wanted {wanted} ({})",
            started.iter().map(|&index| GESTURES.get(index as usize).copied().unwrap_or("?")).collect::<Vec<_>>(),
            GESTURES[wanted as usize]
        );
        if started != [wanted] {
            failures.push(format!("D-pad {direction}: started {started:?}, wanted [{wanted}]"));
        }
    }

    // The character reload that a respawn travel runs keeps the slots.
    assert!(host.reload_character_data(), "{TEST}: the reload found no local actor");
    if host.local_gestures() != Some(SLOTS) {
        failures.push(format!("after the character reload the slots are {:?}", host.local_gestures()));
    }
    let started = press_gesture(TEST, &mut host, DPAD_RIGHT);
    println!("{TEST}: after the character reload, D-pad right started {started:?}");
    if started != [SLOTS[3]] {
        failures.push(format!("after the character reload, D-pad right started {started:?}, wanted [{}]", SLOTS[3]));
    }

    // The wipeout respawn of the spike's run keeps the slots.
    let mut host = support::host("S2HOST_TEST_DISC");
    host.set_gestures(SLOTS).unwrap_or_else(|why| panic!("{TEST}: {why}"));
    for tick in 0..TICKS {
        host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, spike tick {tick}: {why}"));
    }
    println!("{TEST}: after the wipeout respawn the slots are {:?}", host.local_gestures());
    if host.local_gestures() != Some(SLOTS) {
        failures.push(format!("after the wipeout respawn the slots are {:?}", host.local_gestures()));
    }
    get_off_the_board(TEST, &mut host);
    let started = press_gesture(TEST, &mut host, DPAD_LEFT);
    println!("{TEST}: after the wipeout respawn, D-pad left started {started:?}");
    if started != [SLOTS[2]] {
        failures.push(format!("after the wipeout respawn, D-pad left started {started:?}, wanted [{}]", SLOTS[2]));
    }
    assert!(failures.is_empty(), "{TEST}: {failures:#?}");
}

/// The frames of the spike's wall bail on a host booted with `boot` and then given `mode` (if any).
fn wall_bail(test: &str, boot: &Boot, mode: Option<u8>) -> Vec<Frame> {
    let mut host = host_with("S2HOST_TEST_DISC", boot);
    if let Some(mode) = mode {
        host.set_mode(mode).unwrap_or_else(|why| panic!("{test}: {why}"));
    }
    (0..700)
        .map(|tick| host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{test}, spike tick {tick}: {why}")))
        .collect()
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn mode_3_turns_the_hom_effect_off() {
    const TEST: &str = "mode_3_turns_the_hom_effect_off";
    record_reads(TEST);
    let on_boot = |mode| Boot { mode, ..Boot::default() };
    let mut failures = Vec::new();
    // (what, frames): the effect's frames are those with `hom.effect_enabled`.
    for (what, frames, effect_wanted) in [
        ("mode 1 at boot", wall_bail(TEST, &on_boot(1), None), true),
        ("mode 3 at boot", wall_bail(TEST, &on_boot(3), None), false),
        ("mode 3 after boot", wall_bail(TEST, &Boot::default(), Some(3)), false),
        ("mode 1 after boot", wall_bail(TEST, &Boot::default(), Some(1)), true),
    ] {
        let effect: Vec<usize> = (0..frames.len()).filter(|&tick| frames[tick].hom.effect_enabled).collect();
        let bailing: Vec<usize> = (0..frames.len()).filter(|&tick| frames[tick].hom.bail_in_progress).collect();
        let stretched = frames.iter().filter(|frame| frame.sim_period_s != TICK_SECONDS).count();
        println!(
            "{TEST}: {what}: wipeout category from tick {:?}; hom.bail_in_progress on {} ticks (first {:?}); \
             hom.effect_enabled on {} ticks (first {:?}); {stretched} ticks with sim_period_s off {TICK_SECONDS}",
            frames.iter().position(|frame| frame.category == WIPEOUT),
            bailing.len(),
            bailing.first(),
            effect.len(),
            effect.first()
        );
        if effect.is_empty() == effect_wanted {
            failures.push(format!("{what}: hom.effect_enabled on {} ticks, wanted it {}", effect.len(), if effect_wanted { "set" } else { "never set" }));
        }
        if stretched != 0 {
            failures.push(format!("{what}: sim_period_s is not TICK_SECONDS on {stretched} ticks"));
        }
    }
    assert!(failures.is_empty(), "{TEST}: {failures:#?}");
}

/// The camera's height above the deck, per frame.
fn camera_heights(frames: &[Frame]) -> Vec<f32> {
    frames.iter().map(|frame| frame.camera.position[1] - frame.deck[1]).collect()
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

/// The camera's heading about +y from +z, in radians.
fn camera_yaw(frame: &Frame) -> f32 {
    frame.camera.forward[0].atan2(frame.camera.forward[2])
}

/// What the off-board camera did while the right stick was held at the top right.
#[derive(Debug)]
#[allow(dead_code, reason = "`turn` is only printed, to show the camera does not follow the invert")]
struct Look {
    /// The free camera's look stick: the stick after `invert_x` and `invert_y` (the port's
    /// `CameraSubject::look_stick`).
    stick: [f32; 2],
    /// The free camera's elevation offset.
    elevation: f32,
    /// How far the camera rose (negative: sank) over the hold, in metres.
    rise: f32,
    /// How far it turned over the hold, in radians.
    turn: f32,
}

impl Look {
    /// The look stick points `(x, y)` signs' way, each axis more than half way: the stick at the top right is the
    /// pad's, after the port's radial dead zone (about 0.7 on each axis).
    fn points(&self, x: f32, y: f32) -> bool {
        self.stick[0] * x > 0.5 && self.stick[1] * y > 0.5
    }
}

/// With the skater off the board, holds the right stick up and to the right for `LOOK_TICKS`.
fn look_up_right(test: &str, host: &mut Host) -> Look {
    const LOOK_TICKS: u32 = 60;
    let start = get_off_the_board(test, host);
    let end = hold(host, Pad { right: [i16::MAX, i16::MAX], ..Pad::default() }, LOOK_TICKS);
    let free = host.port().diagnostic_camera_free_look().expect("the camera is constructed");
    assert!(free.use_free_stick, "{test}: the off-board shot does not take the free stick");
    Look {
        stick: free.look_stick,
        elevation: free.elevation_offset,
        rise: end.camera.position[1] - start.camera.position[1],
        turn: angle_between(camera_yaw(&end), camera_yaw(&start)),
    }
}

/// A host booted with `profile` that has run the spike's run, wall bail and respawn included.
fn after_the_respawn(profile: Profile) -> (Host, Vec<Frame>) {
    let mut host = host_with("S2HOST_TEST_DISC", &Boot { profile, ..Boot::default() });
    let frames: Vec<Frame> = (0..TICKS)
        .map(|tick| host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("spike tick {tick}: {why}")))
        .collect();
    (host, frames)
}

#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_profile_reaches_the_camera() {
    const TEST: &str = "the_profile_reaches_the_camera";
    record_reads(TEST);
    let with = |camera_angle, invert_x, invert_y| Profile { camera_angle, invert_x, invert_y, ..Profile::default() };
    let mut failures = Vec::new();

    // The camera angle: Low (0) against High (1) over 300 spike ticks.
    let run = |profile: Profile| -> Vec<Frame> {
        let mut host = host_with("S2HOST_TEST_DISC", &Boot { profile, ..Boot::default() });
        (0..300)
            .map(|tick| host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, spike tick {tick}: {why}")))
            .collect()
    };
    let low = run(with(0, false, false));
    let high = run(with(1, false, false));
    let (low_heights, high_heights) = (camera_heights(&low), camera_heights(&high));
    let differing = (0..300).filter(|&tick| low[tick].camera.position != high[tick].camera.position).count();
    // Settled: the second half of the run.
    let (low_mean, high_mean) = (mean(&low_heights[150..]), mean(&high_heights[150..]));
    println!(
        "{TEST}: camera height above the deck over 300 ticks: Low mean {:.3} m, High mean {:.3} m; the camera \
         positions differ on {differing} ticks; over the last 150 ticks Low {low_mean:.3} m, High {high_mean:.3} m",
        mean(&low_heights),
        mean(&high_heights)
    );
    if differing == 0 {
        failures.push("the two camera paths are the same".to_owned());
    }
    if high_mean <= low_mean + 0.05 {
        failures.push(format!("the High camera ({high_mean:.3} m) does not sit higher than Low ({low_mean:.3} m)"));
    }

    // The inverts. Off the board the right stick drives the free camera, whose input is the stick after the inverts
    // (`look_stick`). In Skate 2's camera data the free camera's heading offset is 0, so x moves nothing of the camera
    // but the stick itself (the right stick also turns the skater, which the camera follows, with or without the
    // invert); y raises or lowers the camera (`elevation_offset`). So invert_x shows in the look stick's x, invert_y in
    // its y and in the camera's height.
    let turn = |profile: Profile, after_boot: bool| -> Look {
        let boot = if after_boot { Boot::default() } else { Boot { profile, ..Boot::default() } };
        let mut host = host_with("S2HOST_TEST_DISC", &boot);
        if after_boot {
            host.set_profile(profile);
        }
        hold(&mut host, Pad::default(), 30);
        look_up_right(TEST, &mut host)
    };
    let plain = turn(with(0, false, false), false);
    let invert_x = turn(with(0, true, false), false);
    let invert_y = turn(with(0, false, true), false);
    let both_after_boot = turn(with(0, true, true), true);
    for (what, look) in [("plain", &plain), ("invert_x", &invert_x), ("invert_y", &invert_y), ("both, set_profile", &both_after_boot)] {
        println!("{TEST}: {what}: {look:?}");
    }
    if !plain.points(1.0, 1.0) || plain.elevation < 0.1 {
        failures.push(format!("plain: the look stick {:?}, elevation {}", plain.stick, plain.elevation));
    }
    if !invert_x.points(-1.0, 1.0) || invert_x.elevation < 0.1 {
        failures.push(format!("invert_x by Boot.profile: the look stick is {:?}, not (-, +)", invert_x.stick));
    }
    if !invert_y.points(1.0, -1.0) || invert_y.elevation > -0.1 {
        failures.push(format!("invert_y by Boot.profile: the look stick {:?}, elevation {}", invert_y.stick, invert_y.elevation));
    }
    if (plain.rise - invert_y.rise).abs() < 0.1 {
        failures.push(format!("invert_y by Boot.profile: the camera rose {} m, plain {} m", invert_y.rise, plain.rise));
    }
    if !both_after_boot.points(-1.0, -1.0) || both_after_boot.elevation > -0.1 {
        failures.push(format!(
            "both by set_profile: the look stick {:?}, elevation {}",
            both_after_boot.stick, both_after_boot.elevation
        ));
    }
    if (plain.rise - both_after_boot.rise).abs() < 0.1 {
        failures.push(format!("both by set_profile: the camera rose {} m, plain {} m", both_after_boot.rise, plain.rise));
    }

    // After the wipeout respawn the settings still hold: the High camera is higher, and the inverts still reach the
    // look stick and the camera's height.
    let (mut low_host, low_frames) = after_the_respawn(with(0, false, false));
    let (mut high_host, high_frames) = after_the_respawn(with(1, true, true));
    let (low_after, high_after) = (mean(&camera_heights(&low_frames[870..])), mean(&camera_heights(&high_frames[870..])));
    println!("{TEST}: after the wipeout respawn (ticks 870-900): Low {low_after:.3} m, High {high_after:.3} m");
    if high_after <= low_after + 0.05 {
        failures.push(format!("after the respawn the High camera ({high_after:.3} m) is not higher than Low ({low_after:.3})"));
    }
    let plain_after = look_up_right(TEST, &mut low_host);
    let inverted_after = look_up_right(TEST, &mut high_host);
    println!("{TEST}: after the wipeout respawn: plain {plain_after:?}, inverted {inverted_after:?}");
    if !plain_after.points(1.0, 1.0) || !inverted_after.points(-1.0, -1.0) || inverted_after.elevation > -0.1 {
        failures.push(format!("after the respawn: plain look stick {:?}, inverted {:?}", plain_after.stick, inverted_after.stick));
    }
    if (plain_after.rise - inverted_after.rise).abs() < 0.1 {
        failures.push(format!("after the respawn the camera rose {} m inverted, {} m plain", inverted_after.rise, plain_after.rise));
    }
    assert!(failures.is_empty(), "{TEST}: {failures:#?}");
}
