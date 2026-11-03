//! Bug #25 (10-25) end to end: the stock skater pushing west across University's binocular plaza stopped dead
//! (7.9 to 0.07 m/s, then rolled back) at deck x -408.25, where a wall buried under the plaza meets it: the front
//! truck got a face contact from that wall (`skate_world::sealed_faces`). Install assets, read-only.
use super::mode_harness::{Harness, Matrix, Spawn, pads};

fn level(p: [f32; 3], heading: f32) -> Matrix {
    let (s, c) = heading.to_radians().sin_cos();
    [[c, 0., -s, 0.], [0., 1., 0., 0.], [s, 0., c, 0.], [p[0], p[1], p[2], 1.]]
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn pushing_west_past_the_binoculars_does_not_stop_at_the_buried_wall() {
    if std::env::var_os("SKATE3_ASSET_ROOT").is_none() {
        return;
    }
    let mut h = Harness::load("University", Spawn::MapLoad(level([-394., 222.62, -874.], -90.)));
    for _ in 0..90 {
        h.tick(pads::idle());
    }
    let (mut slowest, mut west) = (f32::MAX, f32::MAX);
    for t in 0..300 {
        let s = h.tick(pads::push(t));
        assert!(!s.bailing(), "bailed at tick {t}, deck {:?}", s.deck);
        if (-409.5..=-407.5).contains(&s.deck[0]) {
            slowest = slowest.min(s.speed);
        }
        west = west.min(s.deck[0]);
    }
    // Before the fix: 0.07 m/s at x -408.26, and the board never got past -408.3 before pushing again.
    assert!(slowest > 6.0, "slowest {slowest} m/s across the buried wall");
    assert!(west < -412.0, "reached x {west}");
}
