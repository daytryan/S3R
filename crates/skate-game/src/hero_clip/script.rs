//! The clip's pad 0, one Step per 60 Hz sim tick, generated from the installed skater.pat (no coordinates in the repo).
pub struct Pattern { pub name: String, pub coords: Vec<[f32; 2]> }
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step { pub push: bool, pub brake: bool, pub rx: i16, pub ry: i16 }

pub const SETTLE: usize = 120;  // > the 90-frame spawn pin (physics.rs:5-13)
pub const PUSHES: usize = 2;    // A held 10 ticks, released 20, on the start box
pub const FLAT_PUSHES: usize = 2; // the same again on the flat, inside the clip
pub const ROLL: usize = 200;    // down the test world's 12 m ramp and onto the flat
/// The tick the first push on the flat starts: the clip opens just before it.
pub const FLAT_PUSH_AT: usize = SETTLE + PUSHES * 30 + ROLL;
pub const GAP: usize = 70;      // air, landing and a little roll between tricks

pub fn plan(patterns: &[Pattern], tricks: &[&str], mirrored: bool) -> Result<Vec<Step>, String> {
    let mut steps = vec![Step::default(); SETTLE];
    for _ in 0..PUSHES {
        steps.extend(std::iter::repeat_n(Step { push: true, ..Step::default() }, 10));
        steps.extend(std::iter::repeat_n(Step::default(), 20));
    }
    steps.extend(std::iter::repeat_n(Step::default(), ROLL));
    for _ in 0..FLAT_PUSHES {
        steps.extend(std::iter::repeat_n(Step { push: true, ..Step::default() }, 10));
        steps.extend(std::iter::repeat_n(Step::default(), 20));
    }
    // PAT -> stick as tests/flip_playback.rs does it; goofy mirrors sideways
    let stick = |p: [f32; 2]| Step {
        rx: (p[0] * 100.0).round() as i16 * if mirrored { -1 } else { 1 },
        ry: (-p[1] * 100.0).round() as i16,
        ..Step::default()
    };
    let mut any = false;
    for name in tricks {
        let Some(p) = patterns.iter().find(|p| p.name == *name && p.coords.len() >= 2) else { continue };
        any = true;
        // hold point 0, one tick per point, hold the last briefly (longer turns into doubles), let go
        steps.extend(std::iter::repeat_n(stick(p.coords[0]), 12));
        steps.extend(p.coords[1..p.coords.len() - 1].iter().map(|&c| stick(c)));
        steps.extend(std::iter::repeat_n(stick(p.coords[p.coords.len() - 1]), 4));
        steps.extend(std::iter::repeat_n(Step::default(), GAP));
    }
    if !any {
        return Err(format!("none of {tricks:?} is in skater.pat"));
    }
    steps.extend(std::iter::repeat_n(Step { brake: true, ..Step::default() }, 60));
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pat(name: &str, coords: &[[f32; 2]]) -> Pattern { Pattern { name: name.into(), coords: coords.to_vec() } }
    // 360Flip is the in-repo fixture (skate-core/src/input/gesture.rs:193-201); the others are made up for the test
    fn pats() -> Vec<Pattern> { vec![
        pat("Ollie", &[[0.0, -0.9], [0.0, 0.9]]),
        pat("Kickflip", &[[0.0, -0.9], [-0.6, 0.7]]),
        pat("360Flip", &[[-0.965714, 0.268571], [-0.497143, 0.84], [0.211429, 0.988571], [0.908571, -0.405714]]),
    ] }

    #[test]
    fn settles_then_pushes_then_rolls_before_the_first_flick() {
        let steps = plan(&pats(), &["Ollie"], false).unwrap();
        assert!(steps[..SETTLE].iter().all(|s| !s.push && s.rx == 0 && s.ry == 0));
        assert_eq!(steps[SETTLE..SETTLE + PUSHES * 30].iter().filter(|s| s.push).count(), PUSHES * 10);
        let first_flick = steps.iter().position(|s| s.ry != 0 || s.rx != 0).unwrap();
        assert_eq!(first_flick, SETTLE + PUSHES * 30 + ROLL + FLAT_PUSHES * 30);
    }

    #[test]
    fn pushes_again_on_the_flat_where_the_clip_sees_it() {
        let steps = plan(&pats(), &["Ollie"], false).unwrap();
        let flat = SETTLE + PUSHES * 30 + ROLL;
        assert!(steps[flat - ROLL..flat].iter().all(|s| !s.push), "rolls down the ramp without pushing");
        assert_eq!(steps[flat..flat + FLAT_PUSHES * 30].iter().filter(|s| s.push).count(), FLAT_PUSHES * 10);
    }

    #[test]
    fn a_flick_holds_the_first_point_steps_each_point_and_lets_go() {
        let steps = plan(&pats(), &["360Flip"], false).unwrap();
        let at = SETTLE + PUSHES * 30 + ROLL + FLAT_PUSHES * 30;
        assert!(steps[at..at + 12].iter().all(|s| (s.rx, s.ry) == (-97, -27)));
        assert_eq!((steps[at + 12].rx, steps[at + 12].ry), (-50, -84));
        assert_eq!((steps[at + 13].rx, steps[at + 13].ry), (21, -99));
        assert!(steps[at + 14..at + 18].iter().all(|s| (s.rx, s.ry) == (91, 41)));
        assert_eq!((steps[at + 18].rx, steps[at + 18].ry), (0, 0));
    }

    #[test]
    fn goofy_mirrors_the_stick_sideways_only() {
        let reg = plan(&pats(), &["Kickflip"], false).unwrap();
        let goofy = plan(&pats(), &["Kickflip"], true).unwrap();
        assert!(reg.iter().zip(&goofy).all(|(a, b)| a.rx == -b.rx && a.ry == b.ry && a.push == b.push));
    }

    #[test]
    fn missing_tricks_are_skipped_and_none_at_all_is_an_error() {
        let one = plan(&pats(), &["Nollie", "Ollie"], false).unwrap();
        let both = plan(&pats(), &["Ollie"], false).unwrap();
        assert_eq!(one.len(), both.len());
        assert!(plan(&pats(), &["Nollie"], false).is_err());
    }

    #[test]
    fn ends_with_a_brake_after_the_last_trick() {
        let steps = plan(&pats(), &["Ollie", "Kickflip", "360Flip"], false).unwrap();
        assert!(steps.last().unwrap().brake);
    }
}
