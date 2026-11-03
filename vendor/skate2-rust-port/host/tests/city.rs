//! The ambient gate (Task 22): with the census off, no pedestrian and no vehicle of Skate 2's city is in the sim. The
//! user wants Skate 2's skating and tricks, not its city (Amendment C), so the game always boots with the gate off.
//! Each test needs the disc, so each is ignored by default: run them with `--ignored` and `S2HOST_TEST_DISC` and
//! `S2HOST_TEST_READS` set. Not tested in play.

#[allow(dead_code, reason = "shared with the other disc tests, which use the rest")]
mod support;

use skate2_physics_host::Host;
use skate2_physics_protocol::Boot;
use support::{host_with, record_reads, spike_script};

/// The seed the repeatable run uses (the same as `headless.rs`).
const SEED: u64 = 0x5EED;
/// The ticks the long run lasts: the spike's 900-tick run, then 900 more with the pad left alone.
const TICKS: u32 = 1800;

/// What a snapshot holds of Skate 2's city: its pedestrians and its vehicles.
fn counts(test: &str, host: &mut Host, tick: u32) -> (usize, usize) {
    host.port()
        .try_visual_snapshot()
        .map(|visual| (visual.pedestrians.len(), visual.vehicles.len()))
        .unwrap_or_else(|why| panic!("{test}, tick {tick}: the snapshot: {why}"))
}

/// A host booted on the NSV disc with the repeatable seed and exact delivery, and `ambient` as asked.
fn booted(ambient: bool) -> Host {
    let boot = Boot { seed: Some(SEED), exact_delivery: Some(true), ambient, ..Boot::default() };
    host_with("S2HOST_TEST_DISC", &boot)
}

/// Row 2 of the spec's unconfirmed list: what the boot census leaves. With the gate off, the first tick's snapshot
/// holds no pedestrian and no vehicle.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_ambient_gate_empties_the_city_on_the_first_tick() {
    const TEST: &str = "the_ambient_gate_empties_the_city_on_the_first_tick";
    record_reads(TEST);
    let mut host = booted(false);
    host.tick(spike_script(0)).unwrap_or_else(|why| panic!("{TEST}, tick 0: {why}"));
    let (pedestrians, vehicles) = counts(TEST, &mut host, 0);
    println!("{TEST}: after the first tick the snapshot holds {pedestrians} pedestrians and {vehicles} vehicles");
    assert_eq!((pedestrians, vehicles), (0, 0), "the first tick's snapshot holds Skate 2's city");
}

/// The gate holds: over 1800 ticks of the spike's script (then the pad left alone), the snapshot never holds a
/// pedestrian or a vehicle.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_ambient_gate_holds_on_nsv() {
    const TEST: &str = "the_ambient_gate_holds_on_nsv";
    record_reads(TEST);
    let mut host = booted(false);
    let mut most = (0, 0);
    let mut first_seen = None;
    for tick in 0..TICKS {
        host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, tick {tick}: {why}"));
        let (pedestrians, vehicles) = counts(TEST, &mut host, tick);
        most = (most.0.max(pedestrians), most.1.max(vehicles));
        if (pedestrians, vehicles) != (0, 0) && first_seen.is_none() {
            first_seen = Some((tick, pedestrians, vehicles));
        }
    }
    println!("{TEST}: over {TICKS} ticks at most {} pedestrians and {} vehicles; first seen {first_seen:?}", most.0, most.1);
    assert_eq!(first_seen, None, "the gate is off, yet a pedestrian or a vehicle was in the snapshot");
}

/// The two tests above pass for a reason that is not the gate if the host's city is empty anyway (this host streams
/// no NSV terrain, so the census may find nothing to spawn: VENDORED.md, row 2). So this one checks the
/// gate itself, where the census reads it: the flags its cull radius and its spawn test use, after the boot and after
/// a tick, with the gate closed, then open, then closed and open again through `set_ambient`.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_gate_reaches_the_census() {
    const TEST: &str = "the_gate_reaches_the_census";
    record_reads(TEST);
    // One after the other: the port's generators are process-wide.
    let mut closed = booted(false);
    assert!(closed.gated_before_census(), "the gate was not closed before the boot's initial census");
    let after_boot = closed.census_banned();
    closed.tick(spike_script(0)).unwrap_or_else(|why| panic!("{TEST}, closed, tick 0: {why}"));
    let after_tick = closed.census_banned();
    println!(
        "{TEST}: gate closed at boot (before the boot's initial census: {}): banned (pedestrians, vehicles) after the \
         boot {after_boot:?}, after a tick {after_tick:?}",
        closed.gated_before_census()
    );
    assert_eq!(after_boot, Some((true, true)), "closed at boot: the census flags after the boot");
    assert_eq!(after_tick, Some((true, true)), "closed at boot: the census flags after a tick");

    let mut open = booted(true);
    let after_boot = open.census_banned();
    open.tick(spike_script(0)).unwrap_or_else(|why| panic!("{TEST}, open, tick 0: {why}"));
    let after_tick = open.census_banned();
    println!("{TEST}: gate open at boot: banned after the boot {after_boot:?}, after a tick {after_tick:?}");
    assert_eq!(after_boot, Some((false, false)), "open at boot: the census flags after the boot");
    assert_eq!(after_tick, Some((false, false)), "open at boot: the census flags after a tick");

    // The game's SetAmbient: closing an open gate and opening it again take hold on the next census update.
    open.set_ambient(false).unwrap_or_else(|why| panic!("{TEST}, set_ambient(false): {why}"));
    assert_eq!(open.census_banned(), Some((true, true)), "SetAmbient(false): the flags at once");
    for tick in 1..=8 {
        open.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, closed again, tick {tick}: {why}"));
        assert_eq!(open.census_banned(), Some((true, true)), "SetAmbient(false): the flags after tick {tick}");
    }
    open.set_ambient(true).unwrap_or_else(|why| panic!("{TEST}, set_ambient(true): {why}"));
    assert_eq!(open.census_banned(), Some((false, false)), "SetAmbient(true): the flags at once");
    for tick in 9..=16 {
        open.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, open again, tick {tick}: {why}"));
        assert_eq!(open.census_banned(), Some((false, false)), "SetAmbient(true): the flags after tick {tick}");
    }
}

/// What a snapshot holds of Skate 2's props: its dynamic objects (`LWDynamicObjectMan`'s DMOs). The skater and its board
/// are not among them, so every one is a non-skater object.
fn props(test: &str, host: &mut Host, tick: u32) -> usize {
    host.port()
        .try_visual_snapshot()
        .map(|visual| visual.dynamic_objects.len())
        .unwrap_or_else(|why| panic!("{test}, tick {tick}: the snapshot: {why}"))
}

/// Amendment C and fix round 1: the user wants Skate 2's skating and tricks, not its world, and a Skate 2 prop would be
/// an invisible obstacle in the game's own map. So with the gate closed no dynamic object is in the sim on any of 1800
/// ticks, and with it open the same run grows them (so the zero is the gate's and not an empty map's).
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn the_ambient_gate_holds_the_props_on_nsv() {
    const TEST: &str = "the_ambient_gate_holds_the_props_on_nsv";
    record_reads(TEST);
    // One after the other: the port's generators are process-wide.
    let run = |ambient: bool| {
        let mut host = booted(ambient);
        let mut every_60th = Vec::new();
        let mut most = 0;
        let mut first_seen = None;
        for tick in 0..TICKS {
            host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, ambient {ambient}, tick {tick}: {why}"));
            let count = props(TEST, &mut host, tick);
            most = most.max(count);
            if count != 0 && first_seen.is_none() {
                first_seen = Some((tick, count));
            }
            if tick == 0 || tick % 60 == 59 {
                every_60th.push((tick, count));
            }
        }
        println!("{TEST}: ambient {ambient}: most {most} dynamic objects, first seen {first_seen:?}; (tick, count) {every_60th:?}");
        (most, first_seen)
    };
    let (closed_most, closed_first) = run(false);
    let (open_most, _) = run(true);
    assert_eq!(closed_first, None, "the gate is closed, yet a dynamic object was in the snapshot (most {closed_most})");
    assert!(open_most >= 9, "the gate is open and the props should grow to 9 or more on NSV, the most was {open_most}");
}

/// Closing the gate on a running city: the props there leave (the cull radius is 0), and none comes back. Unlike the
/// tests above this one starts with the gate open, so the props it finds were spawned and have to be culled.
#[test]
#[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC, S2HOST_TEST_READS"]
fn closing_the_gate_culls_the_props() {
    const TEST: &str = "closing_the_gate_culls_the_props";
    /// The tick the gate closes, when the open run has 9 or 10 props.
    const CLOSE_AT: u32 = 700;
    record_reads(TEST);
    let mut host = booted(true);
    for tick in 0..CLOSE_AT {
        host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, tick {tick}: {why}"));
    }
    let before = props(TEST, &mut host, CLOSE_AT);
    host.set_ambient(false).unwrap_or_else(|why| panic!("{TEST}, set_ambient(false): {why}"));
    let mut gone_at = None;
    let mut most_after = 0;
    for tick in CLOSE_AT..TICKS {
        host.tick(spike_script(tick)).unwrap_or_else(|why| panic!("{TEST}, tick {tick}: {why}"));
        let count = props(TEST, &mut host, tick);
        if count == 0 && gone_at.is_none() {
            gone_at = Some(tick - CLOSE_AT);
        }
        if gone_at.is_some() {
            most_after = most_after.max(count);
        }
    }
    println!("{TEST}: {before} props when the gate closed at tick {CLOSE_AT}; none left after {gone_at:?} ticks; most after that {most_after}");
    assert!(before >= 9, "the open gate should have grown 9 or more props by tick {CLOSE_AT}, it had {before}");
    assert!(gone_at.is_some(), "the closed gate left props in the snapshot at the end");
    assert_eq!(most_after, 0, "a prop came back after the gate culled them");
}
