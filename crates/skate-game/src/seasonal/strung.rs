//! Not retail (user 10-19): things strung between supports: strands of lights (swagged, taut, icicles or wound round
//! a tower) and the giant cord webs. Not tested in play.

use super::layout::{Palette, Style};
use super::model::*;
use super::random;
use bevy::prelude::*;
use std::f32::consts::TAU;

/// The longest swag between two supports (m).
pub(super) const SWAG: f32 = 2.5;
/// Every swag's sag, as a share of its span.
pub(super) const SAG: f32 = 0.08;
/// A giant web cord's radius (m).
pub(super) const CORD_RADIUS: f32 = 0.03;

/// The wire's path through `points`, sampled every 5 cm or finer.
pub(super) fn wire(points: &[Vec3], style: Style) -> Vec<Vec3> {
    const STEP: f32 = 0.05;
    let Some(&start) = points.first() else { return Vec::new() };
    let mut out = vec![start];
    match style {
        Style::Spiral { radius, top, turns } => {
            let head = points[points.len() - 1];
            let axis = (head - start).normalize_or(Vec3::Y);
            let side = Vec3::Y.cross(axis).try_normalize().unwrap_or(Vec3::X);
            let ahead = axis.cross(side);
            let reach = turns * TAU * radius.max(top) + start.distance(head);
            let n = ((reach / STEP).ceil() as usize).max(2);
            out = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    let a = TAU * turns * t;
                    start.lerp(head, t) + (side * a.cos() + ahead * a.sin()) * (radius + (top - radius) * t)
                })
                .collect();
        }
        Style::Swag => {
            for w in points.windows(2) {
                let swags = (w[0].distance(w[1]) / SWAG).ceil().max(1.) as usize;
                for s in 0..swags {
                    let (p, q) = (w[0].lerp(w[1], s as f32 / swags as f32), w[0].lerp(w[1], (s + 1) as f32 / swags as f32));
                    let span = p.distance(q);
                    let n = ((span / STEP).ceil() as usize).max(2);
                    out.extend((1..=n).map(|i| {
                        let t = i as f32 / n as f32;
                        p.lerp(q, t) - Vec3::Y * (4. * SAG * span * t * (1. - t))
                    }));
                }
            }
        }
        Style::Taut | Style::Icicles => {
            for w in points.windows(2) {
                let n = ((w[0].distance(w[1]) / STEP).ceil() as usize).max(1);
                out.extend((1..=n).map(|i| w[0].lerp(w[1], i as f32 / n as f32)));
            }
        }
    }
    out
}

/// How far apart a palette's bulbs sit (m): the multicoloured ones are big C9s.
pub(super) fn spacing(palette: Palette) -> f32 {
    match palette {
        Palette::Halloween | Palette::Warm => 0.3,
        Palette::Multi => 0.4,
        Palette::Ice => 0.25,
    }
}

/// Bulbs evenly spaced along `wire` by its length, hanging just below it.
pub(super) fn bulbs(wire: &[Vec3], spacing: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    let (mut next, mut run) = (spacing * 0.5, 0.);
    for w in wire.windows(2) {
        let d = w[0].distance(w[1]);
        while d > 0. && next <= run + d {
            out.push(w[0].lerp(w[1], (next - run) / d) - Vec3::Y * 0.045);
            next += spacing;
        }
        run += d;
    }
    out
}

/// A strand of lights along `points` laid into `m`: its wire, a pine garland round it with `garland`, and its bulbs
/// (`first` picks where a cycling palette starts).
pub(super) fn lights(m: &mut Model, points: &[Vec3], style: Style, palette: Palette, garland: bool, first: usize) {
    let w = wire(points, style);
    if w.len() < 2 {
        return;
    }
    let cable = if palette == Palette::Ice { WHITE } else { BLACK };
    tube(m.part(cable), &w, |_| 0.006, 4);
    let seed = |k: usize| (first as u32).wrapping_mul(7919) ^ (k as u32).wrapping_mul(104_729);
    if garland {
        let length: f32 = w.windows(2).map(|p| p[0].distance(p[1])).sum();
        tube(m.part(PINE), &w, |t| 0.06 * (0.85 + 0.15 * (t * length * 31.).sin()), 6);
        for (k, p) in bulbs(&w, 0.12).into_iter().enumerate() {
            let s = seed(k);
            let out = Vec3::new(random(s) - 0.5, random(s ^ 1) - 0.5, random(s ^ 2) - 0.5).normalize_or(Vec3::Y) * 0.05;
            surface(m.part(NEEDLE), 5, 3, ball(p + Vec3::Y * 0.045 + out, Vec3::new(0.05, 0.04, 0.05)));
        }
    }
    let colour = |k: usize| match palette {
        Palette::Halloween => [BULB_ORANGE, BULB_PURPLE, BULB_GREEN][(first + k) % 3],
        Palette::Warm => BULB_WARM,
        Palette::Ice => BULB_ICE,
        Palette::Multi => MULTI + (first + k) % 5,
    };
    let size = if palette == Palette::Multi { Vec3::new(0.03, 0.045, 0.03) } else { Vec3::splat(0.025) };
    if style == Style::Icicles {
        // A drop every 30 cm, 20-90 cm long, a bulb every 10 cm down it.
        for (k, p) in bulbs(&w, 0.3).into_iter().enumerate() {
            let top = p + Vec3::Y * 0.045;
            let length = 0.2 + 0.7 * random(seed(k));
            tube(m.part(cable), &[top, top - Vec3::Y * length], |_| 0.004, 3);
            for j in 1..=(length / 0.1) as usize {
                surface(m.part(colour(k + j)), 6, 4, ball(top - Vec3::Y * (0.1 * j as f32), size));
            }
        }
    } else {
        for (k, p) in bulbs(&w, spacing(palette)).into_iter().enumerate() {
            surface(m.part(colour(k)), 6, 4, ball(p, size));
        }
    }
}

/// A giant web of cord strung between the anchors of `frame` laid into `m`: its frame, spokes from a hub and a spiral
/// sagging between them, a few threads broken. Returns the hub, where its spider sits.
pub(super) fn web(m: &mut Model, frame: &[Vec3], seed: u32) -> Vec3 {
    let n = frame.len();
    if n < 2 {
        return frame.first().copied().unwrap_or_default();
    }
    // A cord from a to b, its middle sagging `sag` of its length.
    let cord = |m: &mut Model, a: Vec3, b: Vec3, sag: f32| {
        let c = (a + b) * 0.5 - Vec3::Y * (2. * sag * a.distance(b));
        let path = curve(8, |t| bend(a, c, b, t));
        tube(m.part(CORD), &path, |_| CORD_RADIUS, 5);
        path
    };
    let centre = frame.iter().copied().sum::<Vec3>() / n as f32;
    let size = 2. * frame.iter().map(|p| p.distance(centre)).fold(0., f32::max);
    let hub = centre - Vec3::Y * (0.03 * size);
    // The frame, and its sagged edges as one loop for the spokes to meet.
    let mut rim: Vec<Vec3> = Vec::new();
    for i in 0..n {
        let path = cord(m, frame[i], frame[(i + 1) % n], SAG);
        rim.extend(&path[..path.len() - 1]);
    }
    rim.push(rim[0]);
    let spokes = 12 + (random(seed) * 5.) as usize % 5;
    let total: f32 = rim.windows(2).map(|w| w[0].distance(w[1])).sum();
    let ends: Vec<Vec3> = super::place::along(&rim, total / spokes as f32).into_iter().take(spokes).map(|(p, _)| p).collect();
    // A web too small to space them has fewer.
    let spokes = ends.len();
    for &end in &ends {
        cord(m, hub, end, 0.01);
    }
    let rings = 10 + (random(seed ^ 1) * 5.) as usize % 5;
    for j in 0..rings {
        let f = 0.08 + 0.84 * (j + 1) as f32 / rings as f32;
        for k in 0..spokes {
            if random(seed ^ ((j * 31 + k) as u32 + 7)) < 0.06 {
                continue;
            }
            cord(m, hub.lerp(ends[k], f), hub.lerp(ends[(k + 1) % spokes], f), 0.05);
        }
    }
    hub
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swags_are_even_and_sag_alike() {
        let w = wire(&[Vec3::new(0., 3., 0.), Vec3::new(10., 3., 0.)], Style::Swag);
        // 4 swags of 2.5 m, each bottoming at 3 - 0.08 * 2.5, and back up to 3 at each support.
        let low = w.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        assert!((low - (3. - SAG * 2.5)).abs() < 0.01);
        assert_eq!(w.iter().filter(|p| (p.y - 3.).abs() < 1e-4).count(), 5);
        let b = bulbs(&w, 0.3);
        assert!(b.len() > 30);
        assert!(b.windows(2).all(|p| (p[0].distance(p[1]) - 0.3).abs() < 0.03));
    }

    #[test]
    fn a_spiral_climbs_its_tower() {
        let w = wire(&[Vec3::ZERO, Vec3::Y * 10.], Style::Spiral { radius: 1., top: 0.5, turns: 4. });
        let (first, last) = (w[0], w[w.len() - 1]);
        assert!(first.y.abs() < 1e-3 && (last.y - 10.).abs() < 1e-3);
        assert!((Vec2::new(first.x, first.z).length() - 1.).abs() < 0.01 && (Vec2::new(last.x, last.z).length() - 0.5).abs() < 0.05);
    }

    #[test]
    fn icicles_hang_below_their_route() {
        let mut m = Model::new();
        lights(&mut m, &[Vec3::new(0., 5., 0.), Vec3::new(6., 5., 0.)], Style::Icicles, Palette::Ice, false, 0);
        assert!(!m.0[BULB_ICE].positions.is_empty());
        assert!(m.0[BULB_ICE].positions.iter().all(|p| p[1] < 5. - 0.02 && p[1] > 5. - 0.95));
    }

    #[test]
    fn a_giant_web_reaches_its_anchors() {
        let mut m = Model::new();
        let hub = web(&mut m, &[Vec3::ZERO, Vec3::new(10., 0., 0.), Vec3::new(5., 8., 0.)], 1);
        let xs = m.0[CORD].positions.iter().map(|p| p[0]);
        assert!(xs.clone().fold(f32::MAX, f32::min) < 0.1 && xs.fold(f32::MIN, f32::max) > 9.9);
        assert!(m.0[CORD].indices.len() > 3000 && hub.y > 1. && hub.y < 7.);
    }
}
