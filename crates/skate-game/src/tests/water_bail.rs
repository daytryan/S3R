//! Water (brief 2026-10-26) end to end on DownTown's pond at x -297.5 z -198.7: water at y 46.18, its bed 1.73 m
//! under it. Before the fix the skater stood on the water for good (the user's report); now the board's water vote
//! requests the bail (82DB8120), the wipeout takes it as a water bail (special surface: ragdoll request 10, which
//! with the surfcull table sinks through the water) and the wipeout's reset puts the skater back on dry ground.
//! `water` off is the code before the fix. Install assets, read-only.
use super::mode_harness::{Harness, Matrix, Sample, Spawn, pads};
use skate_core::math::Vector3;

const POND: [f32; 3] = [-297.5, 46.18, -198.7];
/// physics_wipeout's longest wipeout is 20 s; the reset follows it.
const TICKS: u64 = 25 * 60;

/// One tick: the sample, Wipeout486 (special surface) and 487 (below it, the graph's IsInWater).
type Tick = (Sample, bool, bool);

/// Idles on the package spawn (the respawn history's first entry), is teleported onto the pond and idles there.
fn onto_the_pond(water: bool) -> (Harness, Vec<Tick>) {
    let mut h = Harness::load("DownTown", Spawn::Package);
    h.physics.water = water;
    h.run(150, |_| pads::idle());
    let at: Matrix = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [POND[0], POND[1] + 1., POND[2], 1.]];
    h.teleport(at);
    let s = (0..TICKS)
        .map(|_| {
            let s = h.tick(pads::idle());
            let w = &h.skater.wipeout_state.state;
            (s, w.special_surface, w.below_surface)
        })
        .collect();
    (h, s)
}

fn on_pond(s: &Sample) -> bool {
    (s.deck[0] - POND[0]).hypot(s.deck[2] - POND[2]) < 10.
}

/// The physics surface of the first face under a point.
fn surface_under(h: &Harness, [x, y, z]: [f32; 3]) -> Option<u32> {
    let hit = h.physics.world().query_thin_line(Vector3::new(x, y + 1., z), Vector3::new(x, y - 10., z)).ok()??;
    Some((hit.tag >> 7) & 31)
}

/// Each change of state, category or water flags, for the log.
fn trace(s: &[Tick]) -> String {
    let mut out = String::new();
    for (i, (x, special, below)) in s.iter().enumerate() {
        let changed = i == 0 || {
            let (p, ps, pb) = &s[i - 1];
            p.state != x.state || p.category != x.category || ps != special || pb != below
        };
        if changed {
            out += &format!(
                "\n  t{i} {:?} cat {} special {special} below {below} deck {:.2?} lowest body {:.2}",
                x.state, x.category, x.deck, x.lowest_body
            );
        }
    }
    out
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn a_skater_on_the_water_bails_and_is_put_back_on_dry_ground() {
    let (h, s) = onto_the_pond(true);
    eprintln!("WATER on:{}", trace(&s));
    let landed = s.iter().position(|t| on_pond(&t.0)).expect("the teleport put the skater on the pond");
    let bail = s.iter().position(|t| t.0.bailing()).expect("never bailed on the water");
    assert!(bail < landed + 30, "bailed at t{bail}, landed at t{landed}");
    // The reset is a teleport: the deck jumps.
    let reset = (bail + 1..s.len())
        .find(|&t| (s[t].0.deck[0] - s[t - 1].0.deck[0]).hypot(s[t].0.deck[2] - s[t - 1].0.deck[2]) > 3.)
        .expect("never reset after the bail");
    let wipeout = &s[bail..reset];
    let lowest = wipeout.iter().map(|t| t.0.lowest_body).fold(f32::INFINITY, f32::min);
    let below = wipeout.iter().filter(|t| t.2).count();
    let end = &s.last().unwrap().0;
    eprintln!(
        "WATER landed t{landed}, bailed t{bail}, reset t{reset} ({:.2} s after the bail) to {:.2?}; lowest body {lowest:.2}, \
         below the surface (IsInWater) {below} ticks",
        (reset - bail) as f32 / 60.,
        s[reset].0.deck
    );
    assert!(wipeout.iter().any(|t| t.1), "the wipeout never took the water (special surface)");
    assert!(!s[reset..].iter().any(|t| t.0.bailing()), "bailed again after the reset");
    assert!(!on_pond(end), "ended on the pond at {:?}", end.deck);
    assert_ne!(surface_under(&h, end.deck), Some(12), "reset onto water at {:?}", end.deck);
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn without_the_water_fix_the_skater_stands_on_the_water() {
    let (_, s) = onto_the_pond(false);
    eprintln!("WATER off:{}", trace(&s));
    assert!(!s.iter().any(|t| t.0.bailing()), "bailed");
    let end = &s.last().unwrap().0;
    assert!(on_pond(end) && (end.wheel_bottom - POND[1]).abs() < 0.1, "ended at {:?}", end.deck);
}

/// Brief 10-29, the sea at the map's edge: open water whose collision is one Unrideable (surface 6) plane, with nothing
/// else within 100 m. Industrial's lies at y -4 over water drawn at -7.71 (no surface 12 anywhere); New San Vanelona's
/// at -65.4 (tag 0x337) north of the Waterfront sea wall. The board votes 6 and 82DB80C8 (Skate 2: ConsiderSurface,
/// s2.bin 82332F80) requests the teleport (State69): straight back to dry ground, no bail.
const INDUSTRIAL_SEA: (&str, [f32; 3]) = ("Industrial", [-1500., -4., 900.]);
const NEW_SAN_VANELONA_SEA: (&str, [f32; 3]) = ("New_San_Vanelona", [500., -65.4, 1500.]);

/// Idles on the package spawn, is set down 1 m over the sea and idles 10 s; `fix` off is the code before the fix.
fn onto_the_sea((map, sea): (&str, [f32; 3]), fix: bool) -> (Harness, Vec<Sample>) {
    let mut h = Harness::load(map, Spawn::Package);
    h.physics.unrideable = fix;
    h.run(150, |_| pads::idle());
    let at: Matrix = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [sea[0], sea[1] + 1., sea[2], 1.]];
    h.teleport(at);
    let s = h.run(10 * 60, |_| pads::idle());
    (h, s)
}

fn at_sea(sea: [f32; 3], s: &Sample) -> bool {
    (s.deck[0] - sea[0]).hypot(s.deck[2] - sea[2]) < 10.
}

fn teleported_back_from((map, sea): (&str, [f32; 3])) {
    let (h, s) = onto_the_sea((map, sea), true);
    assert_eq!(surface_under(&h, sea), Some(6), "the fixture spot is {map}'s Unrideable sea");
    let landed = s.iter().position(|t| at_sea(sea, t)).expect("the teleport put the skater on the sea");
    let reset = (landed + 1..s.len())
        .find(|&t| !at_sea(sea, &s[t]) && (s[t].deck[0] - s[t - 1].deck[0]).hypot(s[t].deck[2] - s[t - 1].deck[2]) > 3.)
        .expect("never teleported off the sea");
    let end = s.last().unwrap();
    eprintln!(
        "SEA {map}: landed t{landed}, reset t{reset} ({:.2} s) to {:.2?} ({:?}); bailed on the way {}",
        (reset - landed) as f32 / 60.,
        s[reset].deck,
        s[reset].state,
        s[landed..reset].iter().any(Sample::bailing)
    );
    assert!(reset < landed + 120, "teleported only at t{reset}, landed t{landed}");
    assert!(!at_sea(sea, end), "ended on the sea at {:?}", end.deck);
    assert_ne!(surface_under(&h, end.deck), Some(6), "reset onto Unrideable at {:?}", end.deck);
}

fn stays_on((map, sea): (&str, [f32; 3])) {
    let (_, s) = onto_the_sea((map, sea), false);
    let end = s.last().unwrap();
    assert!(at_sea(sea, end) && (end.wheel_bottom - sea[1]).abs() < 0.1, "{map}: ended at {:?}", end.deck);
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn a_skater_on_industrials_sea_is_teleported_back_to_dry_ground() {
    teleported_back_from(INDUSTRIAL_SEA);
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn a_skater_on_new_san_vanelonas_sea_is_teleported_back_to_dry_ground() {
    teleported_back_from(NEW_SAN_VANELONA_SEA);
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn without_the_fix_the_skater_rides_the_sea() {
    stays_on(INDUSTRIAL_SEA);
    stays_on(NEW_SAN_VANELONA_SEA);
}
