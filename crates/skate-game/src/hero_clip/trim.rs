//! Which part of the run becomes the clip: from the first tick on the flat to just after the last landing,
//! cut short before any bail, at most MAX seconds.
use bevy::math::Mat4;

pub struct Tick { pub time: f32, pub on_flat: bool, pub bailed: bool, pub landed: bool }
pub const TAIL: f32 = 0.8; // roll-out after the last landing
pub const LEAD: f32 = 0.2; // margin before a bail
pub const MAX: f32 = 9.0; // the flat pushes and all three tricks

pub fn window(ticks: &[Tick]) -> Option<(f32, f32)> {
    let (start, last_land, cut) = landings(ticks)?;
    Some((start, (last_land + TAIL).min(cut).min(start + MAX)))
}

/// Where the loop may close: rolling after the real last landing (0.4 .. 1.1 s on), never into a bail.
pub fn seam_span(ticks: &[Tick]) -> Option<(f32, f32)> {
    let (_, last_land, cut) = landings(ticks)?;
    Some((last_land + 0.4, (last_land + 1.1).min(cut)))
}

/// The first tick on the flat, the last landing on it, and the bail cut (infinite without a bail).
fn landings(ticks: &[Tick]) -> Option<(f32, f32, f32)> {
    let start = ticks.iter().find(|t| t.on_flat)?.time;
    let cut = ticks.iter().find(|t| t.bailed).map_or(f32::INFINITY, |t| t.time - LEAD);
    // a landing inside the bail margin is the start of the bail, not a landing
    let last_land = ticks.iter().filter(|t| t.landed && t.time >= start && t.time < cut).last()?.time;
    Some((start, last_land, cut))
}

/// Where a seamless loop closes: of the `later` frames (time, animation-space bones), the one whose pose is closest
/// to the loop's `first` frame. Root position is left out: the camera follows the skater.
pub fn seam(first: &[Mat4], later: &[(f32, Vec<Mat4>)]) -> Option<f32> {
    let distance = |pose: &[Mat4]| first.iter().zip(pose).map(|(a, b)| a.w_axis.distance(b.w_axis)).sum::<f32>();
    later
        .iter()
        .filter(|(_, pose)| pose.len() == first.len())
        .min_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)))
        .map(|(time, _)| *time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;
    fn run(spec: &str) -> Vec<Tick> { // one char per tick at 10 Hz for readability: r ramp, f flat, L landed on flat, B bail
        spec.chars().enumerate().map(|(i, c)| Tick { time: i as f32 / 10.0, on_flat: c != 'r', bailed: c == 'B', landed: c == 'L' }).collect()
    }

    #[test]
    fn starts_on_the_flat_and_ends_a_little_after_the_last_landing() {
        let (a, b) = window(&run("rrrrffffLffffLffffffffff")).unwrap();
        assert!((a - 0.4).abs() < 1e-4 && (b - (1.3 + TAIL)).abs() < 1e-4);
    }

    #[test]
    fn a_bail_cuts_the_clip_before_it() {
        let (_, b) = window(&run("rrffffLffffBBBB")).unwrap();
        assert!(b <= 1.1 - LEAD && b >= 0.6);
    }

    #[test]
    fn no_landing_means_no_clip() {
        assert!(window(&run("rrffffBBB")).is_none());
        assert!(window(&run("rrrr")).is_none());
    }

    #[test]
    fn never_longer_than_max() {
        let long = format!("rr{}", "fffffffffL".repeat(20));
        let (a, b) = window(&run(&long)).unwrap();
        assert!(b - a <= MAX + 1e-4);
    }

    #[test]
    fn a_landing_inside_the_bail_margin_does_not_count() {
        // landed at 0.2, bailed at 0.3: a clip cut at 0.1 would not show the landing
        assert!(window(&run("ffLB")).is_none());
    }

    #[test]
    fn the_loop_ends_on_the_pose_closest_to_its_first_frame() {
        let pose = |x: f32| vec![Mat4::from_translation(Vec3::new(x, 0., 0.)), Mat4::IDENTITY];
        let first = pose(1.0);
        let later = [(5.0, pose(3.0)), (5.1, pose(1.1)), (5.2, pose(0.5))];
        assert_eq!(seam(&first, &later), Some(5.1));
        // a frame with another bone count (a cut) never matches
        assert_eq!(seam(&first, &[(6.0, vec![Mat4::IDENTITY])]), None);
    }

    #[test]
    fn the_seam_is_searched_after_the_real_last_landing_and_before_any_bail() {
        // landed at 1.0, bailed at 2.0: the search ends at the bail cut (1.8), not 1.0 + 1.1
        let (a, b) = seam_span(&run("rrffffffffLfffffffffBBBB")).unwrap();
        assert!((a - 1.4).abs() < 1e-4 && (b - 1.8).abs() < 1e-4, "{a} {b}");
        // no bail: 0.4 .. 1.1 s after the landing
        let (a, b) = seam_span(&run("rrffLffffffffffffffff")).unwrap();
        assert!((a - 0.8).abs() < 1e-4 && (b - 1.5).abs() < 1e-4, "{a} {b}");
    }
}
