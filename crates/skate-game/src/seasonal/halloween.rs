//! Not retail (user 10-19): the Halloween mode's bigger decorations, which seasonal.rs places about the Super Ultra
//! Mega Park: inflatables (a haunted witch house, witches, giant spiders, ghosts), graveyards, hay bales, the plaza's
//! light pole and pumpkin towers, cauldrons, skeletons, crypt lanterns, bats and spiders hanging on threads. All built
//! here in code; the inflatables' nylon glows a little from the lamps inside them. Not tested in play.

use super::model::*;
use super::{Decor, Kind, random};
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

// ---------------------------------------------------------------------------------------------------------------
// Inflatables.

/// A haunted witch's house about 3.8 m wide and 5.4 m to its tower's tip: puffy purple walls on a black skirt, an
/// orange eave, a crooked black roof and chimney, a green tower with its own crooked roof, and lit arched windows,
/// barred, and a door left ajar. Pegged down at its corners.
pub(super) fn witch_house() -> Model {
    let mut m = Model::new();
    surface(m.part(BLACK), 40, 12, pillow(Vec3::new(0., 0.13, 0.), Vec3::new(1.9, 0.15, 1.5), 0.4, 0.3));
    surface(m.part(PURPLE), 48, 24, pillow(Vec3::new(0., 1.3, 0.), Vec3::new(1.7, 1.2, 1.3), 0.3, 0.3));
    surface(m.part(ORANGE), 48, 10, pillow(Vec3::new(0., 2.5, 0.), Vec3::new(1.95, 0.14, 1.55), 0.4, 0.3));
    // The roof: a rounded pyramid, stitched in puffy rings, its tip bent over.
    surface(m.part(BLACK), 40, 40, |u, v| {
        let s = v.max(1e-3);
        let (so, co) = (TAU * u).sin_cos();
        let puff = 1. + 0.035 * (s * PI * 6.).sin().powi(2);
        let w = (0.06 * s + 0.94 * s.powf(0.85)) * puff;
        let lean = (1. - s).powf(2.2);
        Vec3::new(1.75 * w * spow(co, 0.5) - 0.6 * lean, 2.55 + 2.3 * (1. - s) - 0.25 * lean * lean, 1.38 * w * spow(so, 0.5) + 0.1 * lean)
    });
    // The tower at the front right, and its roof.
    let tower = Vec3::new(1.4, 0., 1.);
    surface(m.part(GREEN), 32, 20, pillow(tower + Vec3::Y * 2., Vec3::new(0.5, 1.95, 0.5), 0.25, 1.));
    surface(m.part(ORANGE), 32, 8, pillow(tower + Vec3::Y * 3.9, Vec3::new(0.66, 0.11, 0.66), 0.4, 1.));
    surface(m.part(BLACK), 32, 30, |u, v| {
        let s = v.max(1e-3);
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.7 * s.powf(0.9) * (1. + 0.04 * (s * PI * 5.).sin().powi(2)) + 0.02 * s;
        let lean = (1. - s).powf(2.);
        tower + Vec3::new(r * co + 0.45 * lean, 3.95 + 1.55 * (1. - s), r * so - 0.15 * lean)
    });
    // A crooked chimney behind the ridge.
    let chimney = curve(10, |t| bend(Vec3::new(-0.8, 2.9, -0.5), Vec3::new(-0.75, 3.6, -0.55), Vec3::new(-1.05, 4.1, -0.45), t));
    tube(m.part(BROWN), &chimney, |t| 0.17 - 0.03 * t, 12);
    // The front: the door in the middle, a window either side, one in the tower.
    let front = 1.3 + 0.012;
    let window = arch(0.62, 0.78);
    for x in [-1.0, 0.45] {
        decal(m.part(GREEN), &window, Vec3::new(x, 0.88, front - 0.004), Vec2::splat(1.18));
        decal(m.part(WINDOW), &window, Vec3::new(x, 0.88, front), Vec2::ONE);
        // Bars across the glow.
        decal(m.part(BLACK), &rect(0.05, 0.74), Vec3::new(x, 1.26, front + 0.004), Vec2::ONE);
        decal(m.part(BLACK), &rect(0.6, 0.05), Vec3::new(x, 1.2, front + 0.004), Vec2::ONE);
    }
    let door = arch(0.62, 1.15);
    decal(m.part(GREEN), &door, Vec3::new(-0.27, 0.3, front - 0.004), Vec2::new(1.2, 1.1));
    decal(m.part(WINDOW), &door, Vec3::new(-0.27, 0.3, front - 0.002), Vec2::ONE);
    // Ajar: the dark door covers all but a strip of light down its left edge.
    decal(m.part(BROWN), &arch(0.5, 1.12), Vec3::new(-0.21, 0.3, front), Vec2::ONE);
    decal(m.part(GOLD), &rect(0.05, 0.05), Vec3::new(-0.05, 0.75, front + 0.004), Vec2::ONE);
    let tower_window = arch(0.26, 0.42);
    decal(m.part(BLACK), &tower_window, Vec3::new(tower.x, 2.55, tower.z + 0.5), Vec2::splat(1.25));
    decal(m.part(WINDOW), &tower_window, Vec3::new(tower.x, 2.6, tower.z + 0.505), Vec2::ONE);
    // A round attic window under the roof's front.
    let round: Vec<Vec2> = (0..16).map(|k| Vec2::from_angle(TAU * k as f32 / 16.) * 0.2).collect();
    decal(m.part(WINDOW), &round, Vec3::new(-0.35, 1.95, front), Vec2::ONE);
    decal(m.part(BLACK), &rect(0.4, 0.035), Vec3::new(-0.35, 1.95, front + 0.004), Vec2::ONE);
    decal(m.part(BLACK), &rect(0.035, 0.4), Vec3::new(-0.35, 1.95, front + 0.004), Vec2::ONE);
    // Tethers from the eave's corners to pegs.
    for (x, z) in [(-1.8, 1.4), (1.8, -1.4), (-1.8, -1.4)] {
        let top = Vec3::new(x, 2.45, z);
        let peg = Vec3::new(x * 1.35, 0., z * 1.45);
        tube(m.part(BLACK), &[top, peg], |_| 0.012, 5);
    }
    m
}

/// An inflatable witch about 2.9 m tall to her hat's tip: purple dress with a jagged hem, a green face with a long
/// nose, orange hair, a crooked black hat, one hand waving and the other holding her broom.
pub(super) fn witch() -> Model {
    let mut m = Model::new();
    surface(m.part(BLACK), 24, 6, pillow(Vec3::new(0., 0.05, 0.), Vec3::new(0.6, 0.06, 0.6), 0.5, 1.));
    // The dress: flared, puffy in panels, hemmed in points.
    surface(m.part(PURPLE), 48, 30, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let points = ((u * 9.).fract() - 0.5).abs() * 2.;
        let (r, y) = if v < 0.88 {
            let t = v / 0.88;
            let r = 0.27 + 0.4 * t.powf(1.4);
            (r * (1. + 0.05 * (TAU * u * 6.).sin().abs() * t), 1.5 - 1.32 * t - 0.09 * points * t.powi(6))
        } else {
            let k = (v - 0.88) / 0.12;
            (0.67 * (1. - k) * 1.0, 0.18 - 0.09 * points * (1. - k) + 0.04 * k)
        };
        Vec3::new(r * co, y, r * so)
    });
    surface(m.part(BLACK), 32, 8, pillow(Vec3::new(0., 1.48, 0.), Vec3::new(0.3, 0.07, 0.27), 0.4, 1.));
    decal(m.part(GOLD), &rect(0.12, 0.1), Vec3::new(0., 1.48, 0.275), Vec2::ONE);
    surface(m.part(PURPLE), 32, 16, ball(Vec3::new(0., 1.7, 0.), Vec3::new(0.3, 0.3, 0.24)));
    // The face.
    let head = Vec3::new(0., 2.18, 0.02);
    surface(m.part(SKIN), 32, 20, ball(head, Vec3::new(0.27, 0.3, 0.26)));
    surface(m.part(SKIN), 16, 10, ball(Vec3::new(0., 1.98, 0.13), Vec3::new(0.11, 0.12, 0.1)));
    let nose = curve(8, |t| bend(head + Vec3::new(0., -0.02, 0.22), head + Vec3::new(0., 0.0, 0.45), head + Vec3::new(0., -0.13, 0.5), t));
    tube(m.part(SKIN), &nose, |t| 0.065 * (1. - t) + 0.016, 10);
    surface(m.part(SKIN), 10, 6, ball(head + Vec3::new(0.045, -0.06, 0.43), Vec3::splat(0.025)));
    for x in [-0.1, 0.1] {
        surface(m.part(WHITE), 16, 10, ball(head + Vec3::new(x, 0.07, 0.215), Vec3::new(0.075, 0.09, 0.05)));
        surface(m.part(BLACK), 12, 8, ball(head + Vec3::new(x * 0.9, 0.06, 0.255), Vec3::splat(0.035)));
        // Brows, angled down to the nose.
        let brow = [head + Vec3::new(x * 1.7, 0.2, 0.19), head + Vec3::new(x * 0.4, 0.15, 0.25)];
        tube(m.part(BLACK), &brow, |_| 0.016, 6);
    }
    // A grin, on the face.
    let skin_z = |x: f32, y: f32| head.z + 0.26 * (1. - (x / 0.27).powi(2) - ((y - head.y) / 0.3).powi(2)).max(0.).sqrt() + 0.012;
    let grin = curve(10, |t| {
        let x = -0.13 + 0.26 * t;
        let y = head.y - 0.17 + 0.05 * (x / 0.13).powi(2);
        Vec3::new(x, y, skin_z(x, y))
    });
    tube(m.part(BLACK), &grin, |_| 0.017, 6);
    // Orange hair falling from under the brim.
    for k in 0..7 {
        let a = PI * (0.32 + 1.36 * k as f32 / 6.);
        let out = Vec3::new(a.sin(), 0., a.cos());
        let root = head + out * 0.25 + Vec3::Y * 0.14;
        let lock = curve(8, |t| root + out * (0.12 * t) - Vec3::Y * (0.5 * t) + Vec3::new(0.03, 0., 0.03) * (t * 9.).sin());
        tube(m.part(ORANGE), &lock, |t| 0.055 - 0.03 * t, 8);
    }
    // The hat: a wide brim, a crooked crown, a band and buckle.
    let brim = head + Vec3::Y * 0.21;
    surface(m.part(BLACK), 40, 6, pillow(brim, Vec3::new(0.62, 0.04, 0.62), 0.6, 1.));
    surface(m.part(BLACK), 32, 26, |u, v| {
        let s = v.max(1e-3);
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.31 * s.powf(0.8) * (1. + 0.05 * (s * PI * 4.).sin().powi(2)) + 0.01 * s;
        let lean = (1. - s).powf(2.4);
        brim + Vec3::new(r * co - 0.18 * lean, 1.05 * (1. - s) - 0.2 * lean * lean, r * so - 0.32 * lean)
    });
    surface(m.part(PURPLE), 32, 6, pillow(brim + Vec3::Y * 0.1, Vec3::new(0.3, 0.065, 0.3), 0.4, 1.));
    decal(m.part(GOLD), &rect(0.13, 0.11), brim + Vec3::new(0., 0.1, 0.305), Vec2::ONE);
    // Arms: the left waving, the right holding the broom.
    let shoulder = |side: f32| Vec3::new(0.29 * side, 1.85, 0.);
    let left = curve(10, |t| bend(shoulder(-1.), Vec3::new(-0.62, 1.95, 0.1), Vec3::new(-0.7, 2.38, 0.18), t));
    let right = curve(10, |t| bend(shoulder(1.), Vec3::new(0.56, 1.55, 0.12), Vec3::new(0.6, 1.5, 0.34), t));
    for arm in [&left, &right] {
        tube(m.part(PURPLE), arm, |t| 0.11 - 0.03 * t, 12);
        surface(m.part(SKIN), 14, 10, ball(*arm.last().unwrap(), Vec3::splat(0.09)));
    }
    let (foot, top) = (Vec3::new(0.66, 0.62, 0.38), Vec3::new(0.57, 2.45, 0.3));
    tube(m.part(BROWN), &[foot, top], |_| 0.035, 10);
    surface(m.part(STRAW), 24, 12, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.05 + 0.2 * v.powf(1.2) * (1. + 0.08 * (TAU * u * 11.).sin());
        Vec3::new(foot.x + r * co, 0.66 - 0.6 * v, foot.z + r * so)
    });
    surface(m.part(ORANGE), 16, 4, pillow(foot + Vec3::Y * 0.02, Vec3::new(0.075, 0.04, 0.075), 0.5, 1.));
    m
}

/// A giant inflatable spider about 3 m across: a black body striped orange, purple-banded legs bent at orange knees,
/// red eyes glowing and white fangs. Its front feet (+Z) stand `front` below the floor it sits on and its back feet
/// `back`: 0 on the ground; sat on a deck's edge its front legs hang down the face while its back legs stand on the
/// deck; on a car's roof every leg grips over the roof's edges.
pub(super) fn spider(front: f32, back: f32) -> Model {
    let mut m = Model::new();
    let abdomen = Vec3::new(0., 0.8, -0.6);
    let r = Vec3::new(0.62, 0.52, 0.75);
    surface(m.part(BLACK), 40, 24, ball(abdomen, r));
    // Orange stripes over its back.
    for z in [-0.3f32, 0., 0.3] {
        let k = (1. - (z / 0.75).powi(2)).sqrt() * 1.012;
        let stripe = curve(16, |t| {
            let a = PI * (-0.08 + 1.16 * t);
            abdomen + Vec3::new(a.cos() * r.x * k, a.sin() * r.y * k, z * r.z / 0.75 * 1.0)
        });
        tube(m.part(ORANGE), &stripe, |_| 0.05, 8);
    }
    let head = Vec3::new(0., 0.62, 0.22);
    surface(m.part(BLACK), 32, 20, ball(head, Vec3::new(0.42, 0.36, 0.4)));
    for (x, y, z, size) in [(0.15, 0.74, 0.55, 0.1), (0.07, 0.86, 0.5, 0.055), (0.27, 0.64, 0.5, 0.055)] {
        for side in [-1., 1.] {
            surface(m.part(RED_EYE), 12, 8, ball(Vec3::new(x * side, y, z), Vec3::splat(size)));
        }
    }
    for side in [-1., 1.] {
        let fang = curve(6, |t| bend(Vec3::new(0.09 * side, 0.45, 0.55), Vec3::new(0.1 * side, 0.32, 0.63), Vec3::new(0.06 * side, 0.22, 0.6), t));
        tube(m.part(WHITE), &fang, |t| 0.045 * (1. - t) + 0.008, 8);
        for (k, a) in [0.55f32, 1.05, 1.6, 2.15].into_iter().enumerate() {
            let out = Vec3::new(a.sin() * side, 0., a.cos());
            let hip = head + Vec3::new(out.x * 0.33, 0.04, out.z * 0.3);
            let reach = 1.35 + 0.1 * (k == 1 || k == 2) as u32 as f32;
            let knee = hip + out * (reach * 0.48) + Vec3::Y * 0.62;
            let foot = Vec3::new(hip.x, if out.z > 0. { -front } else { -back }, hip.z) + out * reach;
            let thigh = curve(8, |t| bend(hip, hip + out * 0.25 + Vec3::Y * 0.5, knee, t));
            let shin = curve(8, |t| bend(knee, knee + out * 0.45 + Vec3::Y * 0.05, foot, t));
            tube(m.part(BLACK), &thigh, |t| 0.09 - 0.015 * t, 10);
            tube(m.part(PURPLE), &shin, |t| 0.075 - 0.035 * t, 10);
            surface(m.part(ORANGE), 14, 10, ball(knee, Vec3::splat(0.1)));
        }
    }
    m
}

/// A ghost 2.4 m tall, glowing pale blue-white: a sheet flaring to a wavy hem, arms out to say boo, black eyes and an
/// open mouth. It floats on a pole (its root bobs it).
pub(super) fn ghost() -> Model {
    let mut m = Model::new();
    ghost_sheet(&mut m);
    tube(m.part(BLACK), &[Vec3::new(0., -0.4, 0.), Vec3::new(0., 0.7, 0.)], |_| 0.025, 8);
    m
}

/// The ghost without its pole: the sheet, its arms and face, the top of its head 2.3 m up.
fn ghost_sheet(m: &mut Model) {
    let waves = |u: f32| (TAU * u * 6.).sin();
    surface(m.part(GHOST), 48, 36, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let (r, y) = if v < 0.3 {
            let a = v / 0.3 * FRAC_PI_2;
            (0.45 * a.sin(), 1.85 + 0.45 * a.cos())
        } else if v < 0.9 {
            let t = (v - 0.3) / 0.6;
            let r = (0.45 + 0.25 * t.powf(1.3)) * (1. + 0.07 * waves(u + 0.1 * t) * t);
            (r, 1.85 - 1.45 * t + 0.13 * waves(u) * t.powi(4))
        } else {
            let k = (v - 0.9) / 0.1;
            let r = 0.7 * (1. + 0.07 * waves(u + 0.1));
            (r * (1. - k), 0.4 + 0.13 * waves(u) * (1. - k) + 0.25 * k)
        };
        Vec3::new(r * co, y, r * so)
    });
    for side in [-1., 1.] {
        let arm = curve(10, |t| bend(Vec3::new(0.42 * side, 1.5, 0.), Vec3::new(0.8 * side, 1.62, 0.2), Vec3::new(0.82 * side, 1.85, 0.55), t));
        tube(m.part(GHOST), &arm, |t| 0.15 - 0.07 * t, 12);
        surface(m.part(BLACK), 14, 10, ball(Vec3::new(0.15 * side, 2.0, 0.405), Vec3::new(0.08, 0.12, 0.04)));
    }
    surface(m.part(BLACK), 16, 10, ball(Vec3::new(0., 1.72, 0.44), Vec3::new(0.1, 0.13, 0.04)));
}

// ---------------------------------------------------------------------------------------------------------------
// Graveyards and hay.

/// A graveyard plot about 4 m wide: a row or two of tilted headstones of four shapes (RIP carved on the stone
/// texture), a mound before each, a skeleton's hand clawing out of one, and a skull. Its fence is
/// [`fence_panel`].
pub(super) fn graveyard(seed: u32) -> Model {
    let mut m = Model::new();
    let shapes: [Vec<Vec2>; 3] = [
        {
            let mut o = vec![Vec2::new(-0.28, 0.), Vec2::new(0.28, 0.)];
            o.extend((0..=8).map(|k| Vec2::new(0., 0.56) + Vec2::from_angle(PI * k as f32 / 8.) * 0.28));
            o
        },
        vec![Vec2::new(-0.25, 0.), Vec2::new(0.25, 0.), Vec2::new(0.25, 0.6), Vec2::new(0., 0.82), Vec2::new(-0.25, 0.6)],
        vec![Vec2::new(-0.3, 0.), Vec2::new(0.3, 0.), Vec2::new(0.3, 0.55), Vec2::new(0.22, 0.63), Vec2::new(-0.22, 0.63), Vec2::new(-0.3, 0.55)],
    ];
    let rows = if random(seed) < 0.5 { 1 } else { 2 };
    let mut k = 0;
    for row in 0..rows {
        for col in 0..3 {
            k += 1;
            let s = seed.wrapping_add(k * 97);
            let x = -1.3 + 1.3 * col as f32 + 0.25 * (random(s) - 0.5);
            let z = -0.9 * row as f32;
            let tilt = Quat::from_euler(EulerRot::YXZ, 0.3 * (random(s ^ 1) - 0.5), -0.18 * random(s ^ 2), 0.2 * (random(s ^ 3) - 0.5));
            let at = Transform::from_translation(Vec3::new(x, -0.05, z)).with_rotation(tilt).with_scale(Vec3::splat(0.9 + 0.35 * random(s ^ 4)));
            let shape = (random(s ^ 5) * 4.) as usize;
            if shape == 3 {
                // A cross.
                slab(m.part(STONE), &rect(0.13, 0.95).iter().map(|p| *p + Vec2::Y * 0.475).collect::<Vec<_>>(), 0.1, at, false);
                slab(m.part(STONE), &rect(0.55, 0.13).iter().map(|p| *p + Vec2::Y * 0.68).collect::<Vec<_>>(), 0.1, at, false);
            } else {
                slab(m.part(STONE), &shapes[shape], 0.13, at, true);
            }
            surface(m.part(DIRT), 20, 8, pillow(Vec3::new(x, 0., z + 0.6), Vec3::new(0.32, 0.13, 0.55), 1., 1.));
            if k == 2 {
                skeleton_hand(&mut m, Vec3::new(x + 0.05, 0.08, z + 0.55));
            }
        }
    }
    // A skull at the corner, looking out.
    let skull = Vec3::new(1.55, 0.12, 0.75);
    surface(m.part(BONE), 20, 14, ball(skull, Vec3::new(0.11, 0.1, 0.12)));
    surface(m.part(BONE), 14, 8, ball(skull + Vec3::new(0., -0.06, 0.06), Vec3::new(0.07, 0.05, 0.07)));
    for x in [-0.045, 0.045] {
        surface(m.part(BLACK), 10, 6, ball(skull + Vec3::new(x, 0.01, 0.1), Vec3::new(0.03, 0.033, 0.03)));
    }
    m
}

/// Bony fingers clawing up out of a grave at `at`.
fn skeleton_hand(m: &mut Model, at: Vec3) {
    tube(m.part(BONE), &[at - Vec3::new(0., 0.1, 0.1), at + Vec3::new(0., 0.12, 0.02)], |_| 0.025, 8);
    let palm = at + Vec3::new(0., 0.18, 0.03);
    surface(m.part(BONE), 12, 8, ball(palm, Vec3::new(0.055, 0.06, 0.025)));
    for (k, x) in [-0.04f32, -0.013, 0.013, 0.04].into_iter().enumerate() {
        let length = 0.11 + 0.025 * (k == 1 || k == 2) as u32 as f32;
        let finger = curve(6, |t| bend(palm + Vec3::new(x, 0.05, 0.), palm + Vec3::new(x * 1.2, 0.05 + length, 0.02), palm + Vec3::new(x * 1.3, 0.04 + length, 0.09), t));
        tube(m.part(BONE), &finger, |_| 0.011, 6);
    }
    let thumb = curve(5, |t| bend(palm + Vec3::new(-0.05, 0., 0.), palm + Vec3::new(-0.1, 0.05, 0.02), palm + Vec3::new(-0.11, 0.1, 0.06), t));
    tube(m.part(BONE), &thumb, |_| 0.012, 6);
}

/// A stack of bales: where each stands, and where pumpkins sit on and before it (rest point, radius, lit).
pub(super) fn hay_stack(seed: u32) -> (Vec<Transform>, Vec<(Vec3, f32, bool)>) {
    let turn = |k: u32| Quat::from_rotation_y(0.2 * (random(seed ^ k) - 0.5));
    if random(seed ^ 40) < 0.6 {
        // Two side by side, one on top.
        let bales = vec![
            Transform::from_xyz(-0.57, 0., 0.).with_rotation(turn(1)),
            Transform::from_xyz(0.57, 0., 0.06).with_rotation(turn(2)),
            Transform::from_xyz(0.05, 0.48, 0.02).with_rotation(turn(3)),
        ];
        let pumpkins = vec![
            (Vec3::new(-0.2, 0.96, 0.02), 0.27, true),
            (Vec3::new(0.38, 0.96, 0.05), 0.17, false),
            (Vec3::new(-0.92, 0.48, 0.), 0.2, false),
            (Vec3::new(0.95, 0.48, 0.06), 0.15, false),
            (Vec3::new(-0.45, 0., 0.62), 0.22, false),
            (Vec3::new(0.42, 0., 0.7), 0.3, true),
        ];
        (bales, pumpkins)
    } else {
        let bales = vec![Transform::from_rotation(turn(1))];
        let pumpkins = vec![(Vec3::new(-0.1, 0.48, 0.), 0.28, true), (Vec3::new(0.36, 0.48, 0.02), 0.16, false), (Vec3::new(-0.75, 0., 0.5), 0.22, false)];
        (bales, pumpkins)
    }
}

/// The stack's bales.
pub(super) fn hay(seed: u32) -> Model {
    let mut m = Model::new();
    for at in hay_stack(seed).0 {
        hay_bale(&mut m, at);
    }
    m
}

/// A rectangular straw bale 1.1 x 0.48 x 0.55 m standing on the ground, tied with two strings.
fn hay_bale(m: &mut Model, at: Transform) {
    let r = Vec3::new(0.55, 0.24, 0.28);
    let shape = pillow(Vec3::Y * 0.24, r, 0.25, 0.25);
    surface(m.part(STRAW), 32, 16, |u, v| at.transform_point(shape(u, v)));
    for x in [-0.27f32, 0.27] {
        let string = curve(24, |t| {
            let a = TAU * t;
            let (s, c) = a.sin_cos();
            Vec3::new(x, 0.24 + r.y * 1.03 * spow(s, 0.25), r.z * 1.03 * spow(c, 0.25))
        });
        let string: Vec<Vec3> = string.into_iter().map(|p| at.transform_point(p)).collect();
        tube(m.part(BROWN), &string, |_| 0.01, 5);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Bats and hanging spiders.

/// A bat's body, nose along +Z: furry black with red pin eyes and pointed ears.
pub(super) fn bat_body() -> Model {
    let mut m = Model::new();
    surface(m.part(WING), 14, 10, ball(Vec3::ZERO, Vec3::new(0.06, 0.06, 0.14)));
    surface(m.part(WING), 12, 8, ball(Vec3::new(0., 0.02, 0.15), Vec3::splat(0.055)));
    for side in [-1., 1.] {
        tube(m.part(WING), &[Vec3::new(0.03 * side, 0.06, 0.15), Vec3::new(0.05 * side, 0.13, 0.13)], |t| 0.02 * (1. - t) + 0.002, 4);
        surface(m.part(RED_EYE), 6, 4, ball(Vec3::new(0.025 * side, 0.035, 0.2), Vec3::splat(0.012)));
    }
    m
}

/// One wing, out along +X from the shoulder (its pivot): a membrane between finger bones, scalloped behind.
pub(super) fn bat_wing() -> Builder {
    let outline = [
        Vec2::new(0., 0.06),
        Vec2::new(0.2, 0.1),
        Vec2::new(0.42, 0.05),
        Vec2::new(0.52, -0.02),
        Vec2::new(0.4, -0.08),
        Vec2::new(0.34, -0.17),
        Vec2::new(0.24, -0.11),
        Vec2::new(0.16, -0.2),
        Vec2::new(0.08, -0.1),
        Vec2::new(0., -0.12),
    ];
    let mut b = Builder::default();
    let root = b.vertex(Vec3::new(0.04, 0., -0.02), Vec3::Y, Vec2::ZERO);
    let rim: Vec<u32> = outline.iter().map(|p| b.vertex(Vec3::new(p.x, 0.02 * p.x, p.y), Vec3::Y, Vec2::ZERO)).collect();
    for k in 0..rim.len() - 1 {
        b.triangle(root, rim[k], rim[k + 1]);
    }
    b
}

/// A small black spider (about 12 cm) hanging from its thread, which runs up +Y from its back.
pub(super) fn little_spider() -> Model {
    let mut m = Model::new();
    surface(m.part(WING), 12, 8, ball(Vec3::new(0., 0., -0.04), Vec3::new(0.045, 0.04, 0.055)));
    surface(m.part(WING), 10, 6, ball(Vec3::new(0., -0.005, 0.03), Vec3::splat(0.028)));
    for side in [-1., 1.] {
        surface(m.part(RED_EYE), 6, 4, ball(Vec3::new(0.012 * side, 0.005, 0.056), Vec3::splat(0.007)));
        for a in [0.5f32, 1.1, 1.8, 2.5] {
            let out = Vec3::new(a.sin() * side, 0., a.cos());
            let hip = Vec3::new(0., -0.005, 0.02) + out * 0.02;
            let leg = curve(6, |t| bend(hip, hip + out * 0.06 + Vec3::Y * 0.04, hip + out * 0.11 - Vec3::Y * 0.05, t));
            tube(m.part(WING), &leg, |_| 0.005, 4);
        }
    }
    m
}

// ---------------------------------------------------------------------------------------------------------------
// The park's other pieces.

/// The model of a Halloween piece of `kind` (seeded by `seed`), or None for kinds with none here (pumpkins, mist,
/// uplights and Christmas's).
pub(super) fn model(kind: Kind, seed: u32) -> Option<Model> {
    Some(match kind {
        Kind::House => witch_house(),
        Kind::Witch => witch(),
        Kind::Spider => spider(0., 0.),
        Kind::DrapedSpider => spider(1.2, 0.),
        Kind::CarSpider => spider(0.8, 0.8),
        Kind::Ghost => ghost(),
        Kind::HangingGhost => hanging_ghost(),
        Kind::Graveyard => graveyard(seed),
        Kind::Hay => hay(seed),
        Kind::Cauldron => cauldron(),
        Kind::Broom => broom(),
        Kind::Skeleton => skeleton(seed),
        Kind::EggSac => egg_sac(),
        Kind::LightPole => light_pole(9.),
        Kind::BatBanner => bat_banner(),
        Kind::CryptLantern => crypt_lantern(),
        Kind::Fence => fence_panel(4.),
        _ => return None,
    })
}

/// A low iron fence `length` long along X about the origin, its front +Z: posts with spear tips, two rails.
pub(super) fn fence_panel(length: f32) -> Model {
    let mut m = Model::new();
    let (posts, half) = (((length / 0.27).round() as usize).max(1), length * 0.5);
    for p in 0..=posts {
        let x = -half + length * p as f32 / posts as f32;
        tube(m.part(BLACK), &[Vec3::new(x, 0., 0.), Vec3::new(x, 0.62, 0.)], |_| 0.012, 5);
        tube(m.part(BLACK), &[Vec3::new(x, 0.6, 0.), Vec3::new(x, 0.7, 0.)], |t| 0.03 * (1. - t) + 0.002, 5);
    }
    for y in [0.15, 0.52] {
        tube(m.part(BLACK), &[Vec3::new(-half, y, 0.), Vec3::new(half, y, 0.)], |_| 0.012, 5);
    }
    m
}

/// A ghost hung on a cord from its tie at the origin (from a span or a beam), its head a metre below, no pole.
pub(super) fn hanging_ghost() -> Model {
    let mut m = Model::new();
    let mut sheet = Model::new();
    ghost_sheet(&mut sheet);
    m.append(sheet, Transform::from_xyz(0., -3.3, 0.));
    tube(m.part(CORD), &[Vec3::ZERO, Vec3::new(0., -1.02, 0.)], |_| 0.008, 5);
    m
}

/// A witch's cauldron 1.4 m across on three legs over a fire: a black iron pot, its rim rolled, brimming with a
/// glowing green brew and its bubbles.
pub(super) fn cauldron() -> Model {
    let mut m = Model::new();
    surface(m.part(BLACK), 40, 20, pillow(Vec3::Y * 0.75, Vec3::new(0.7, 0.5, 0.7), 0.8, 1.));
    let rim = curve(32, |t| Vec3::new((TAU * t).cos() * 0.64, 1.17, (TAU * t).sin() * 0.64));
    tube(m.part(BLACK), &rim, |_| 0.07, 8);
    surface(m.part(CAULDRON), 32, 8, pillow(Vec3::Y * 1.2, Vec3::new(0.6, 0.07, 0.6), 0.5, 1.));
    for (x, z, r) in [(0.2f32, 0.1f32, 0.08f32), (-0.25, 0.2, 0.06), (0.05, -0.3, 0.07), (-0.15, -0.1, 0.045), (0.32, -0.15, 0.04), (-0.38, -0.05, 0.05)] {
        surface(m.part(CAULDRON), 10, 6, ball(Vec3::new(x, 1.25 + r * 0.4, z), Vec3::splat(r)));
    }
    for k in 0..3 {
        let a = TAU * k as f32 / 3. + 0.4;
        let out = Vec3::new(a.cos(), 0., a.sin());
        let leg = curve(6, |t| bend(out * 0.42 + Vec3::Y * 0.45, out * 0.62 + Vec3::Y * 0.25, out * 0.66, t));
        tube(m.part(BLACK), &leg, |t| 0.05 - 0.02 * t, 8);
    }
    // The fire under it: crossed logs and licking flames.
    for a in [0.3f32, 1.9] {
        let d = Vec3::new(a.cos(), 0., a.sin()) * 0.45;
        tube(m.part(BROWN), &[Vec3::Y * 0.06 - d, Vec3::Y * 0.06 + d], |_| 0.06, 8);
    }
    for k in 0..5 {
        let a = TAU * k as f32 / 5.;
        let foot = Vec3::new(a.cos() * 0.15, 0.08, a.sin() * 0.15);
        let lick = curve(6, |t| bend(foot, foot * 1.6 + Vec3::Y * 0.12, foot * 0.4 + Vec3::Y * 0.34, t));
        tube(m.part(FLAME), &lick, |t| 0.07 * (1. - t) + 0.01, 6);
    }
    m
}

/// A witch's broom 1.6 m tall standing on its bristles.
pub(super) fn broom() -> Model {
    let mut m = Model::new();
    tube(m.part(BROWN), &[Vec3::Y * 0.4, Vec3::new(0.04, 1.6, -0.08)], |_| 0.03, 8);
    surface(m.part(STRAW), 24, 12, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.04 + 0.2 * v.powf(1.2) * (1. + 0.08 * (TAU * u * 11.).sin());
        Vec3::new(r * co, 0.46 - 0.46 * v, r * so)
    });
    surface(m.part(ORANGE), 16, 4, pillow(Vec3::Y * 0.43, Vec3::new(0.06, 0.035, 0.06), 0.5, 1.));
    m
}

/// A skeleton climbing out of its grave, about 1.1 m out of the ground: a mound of earth, its spine and ribs rising
/// from it, its skull tilted, one arm reaching up and the other clawing at the ground (which, by seed).
pub(super) fn skeleton(seed: u32) -> Model {
    let mut m = Model::new();
    surface(m.part(DIRT), 24, 8, pillow(Vec3::new(0., 0., -0.1), Vec3::new(0.75, 0.2, 0.95), 1., 1.));
    let lean = 0.2 + 0.2 * random(seed);
    let spine = |t: f32| Vec3::new(0., -0.1 + 0.85 * t, -0.25 + lean * t * t);
    for k in 0..14 {
        let t = k as f32 / 13.;
        surface(m.part(BONE), 8, 6, ball(spine(t), Vec3::new(0.035, 0.025, 0.03)));
    }
    // Ribs curving round from the spine to the breastbone.
    for k in 0..6 {
        let t = 0.42 + 0.075 * k as f32;
        let back = spine(t);
        let width = 0.17 - 0.012 * (k as f32 - 2.).powi(2).min(9.);
        for side in [-1., 1.] {
            let rib = curve(10, |s| {
                let a = PI * s;
                back + Vec3::new(side * width * a.sin(), -0.04 * s, 0.2 * (1. - a.cos()) * 0.5 + 0.01)
            });
            tube(m.part(BONE), &rib, |_| 0.013, 5);
        }
    }
    let neck = spine(1.);
    let tilt = 0.4 * (random(seed ^ 1) - 0.5);
    let skull = neck + Vec3::new(tilt * 0.1, 0.15, 0.04);
    surface(m.part(BONE), 20, 14, ball(skull, Vec3::new(0.11, 0.12, 0.13)));
    surface(m.part(BONE), 14, 8, ball(skull + Vec3::new(0., -0.08, 0.06), Vec3::new(0.075, 0.05, 0.07)));
    for x in [-0.045, 0.045] {
        surface(m.part(BLACK), 10, 6, ball(skull + Vec3::new(x + tilt * 0.02, 0.01, 0.11), Vec3::new(0.032, 0.036, 0.03)));
    }
    surface(m.part(BLACK), 8, 6, ball(skull + Vec3::new(0., -0.04, 0.125), Vec3::new(0.015, 0.02, 0.012)));
    let reaching = if random(seed ^ 2) < 0.5 { -1. } else { 1. };
    for side in [-1., 1.] {
        let shoulder = spine(0.85) + Vec3::new(0.19 * side, 0., 0.);
        let hand = if side == reaching {
            shoulder + Vec3::new(0.15 * side, 0.55, 0.3)
        } else {
            Vec3::new(0.35 * side, 0.15, 0.55)
        };
        let elbow = (shoulder + hand) * 0.5 + Vec3::new(0.14 * side, -0.05, -0.05);
        tube(m.part(BONE), &[shoulder, elbow], |_| 0.024, 6);
        tube(m.part(BONE), &[elbow, hand], |_| 0.02, 6);
        skeleton_hand(&mut m, hand - Vec3::Y * 0.12);
    }
    m
}

/// A spider's egg sac 0.7 m long: a teardrop of silk hanging from its tie at the origin.
pub(super) fn egg_sac() -> Model {
    let mut m = Model::new();
    surface(m.part(CORD), 24, 16, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.26 * (PI * v).sin().max(0.).powf(0.7) * (0.55 + 0.45 * v) * (1. + 0.04 * (TAU * u * 7. + v * 9.).sin());
        Vec3::new(r * co, -0.1 - 0.7 * v, r * so)
    });
    tube(m.part(CORD), &[Vec3::ZERO, Vec3::new(0., -0.14, 0.)], |_| 0.012, 5);
    m
}

/// A fairground light pole `height` m tall: striped purple and black on a flared foot, a crown of hooks near its top
/// (the canopy's strands run from there) and a glowing jack-o'-lantern on top.
pub(super) fn light_pole(height: f32) -> Model {
    let mut m = Model::new();
    surface(m.part(BLACK), 24, 8, pillow(Vec3::Y * 0.15, Vec3::new(0.45, 0.15, 0.45), 0.5, 1.));
    let bands = ((height / 0.45).round() as usize).max(2);
    for k in 0..bands {
        let (a, b) = (height * k as f32 / bands as f32, height * (k + 1) as f32 / bands as f32);
        let r = |y: f32| 0.13 - 0.04 * y / height;
        tube(m.part(if k % 2 == 0 { PURPLE } else { BLACK }), &[Vec3::Y * a, Vec3::Y * b], |t| r(a + (b - a) * t), 12);
    }
    let crown = height - 0.3;
    tube(m.part(BLACK), &curve(32, |t| Vec3::new((TAU * t).cos() * 0.55, crown, (TAU * t).sin() * 0.55)), |_| 0.025, 6);
    for k in 0..8 {
        let a = TAU * k as f32 / 8.;
        tube(m.part(BLACK), &[Vec3::Y * (crown - 0.25), Vec3::new(a.cos() * 0.55, crown, a.sin() * 0.55)], |_| 0.02, 5);
    }
    let head = Vec3::Y * (height + 0.38);
    surface(m.part(ORANGE), 40, 16, |u, v| {
        let p = ball(Vec3::ZERO, Vec3::new(0.45, 0.36, 0.45))(u, v);
        head + p * Vec3::new(1. + 0.06 * (TAU * u * 10.).cos(), 1., 1. + 0.06 * (TAU * u * 10.).cos())
    });
    tube(m.part(GREEN), &[head + Vec3::Y * 0.3, head + Vec3::new(0.04, 0.46, 0.)], |_| 0.04, 6);
    let front = head.z + 0.47;
    for x in [-0.15f32, 0.15] {
        decal(m.part(WINDOW), &[Vec2::new(-0.07, -0.05), Vec2::new(0.07, -0.05), Vec2::new(0., 0.07)], Vec3::new(x, head.y + 0.08, front), Vec2::ONE);
    }
    let grin: Vec<Vec2> = (0..=10).map(|k| Vec2::new(-0.2 + 0.04 * k as f32, if k % 2 == 0 { -0.03 } else { 0.01 })).chain([Vec2::new(0.14, -0.1), Vec2::new(-0.14, -0.1)]).collect();
    decal(m.part(WINDOW), &grin, Vec3::new(0., head.y - 0.1, front - 0.01), Vec2::ONE);
    m
}

/// Where each jack-o'-lantern of a stack of 3 to 5, about 2.5 m tall, rests (rest point, radius, lit): the biggest at
/// the bottom, each sitting on the one below, a little turned and off-centre (the same as [`hay_stack`]'s).
pub(super) fn pumpkin_tower(seed: u32) -> Vec<(Vec3, f32, bool)> {
    let count = 3 + (random(seed) * 3.) as usize % 3;
    let sizes: Vec<f32> = [0.6, 0.5, 0.42, 0.35, 0.28][..count].to_vec();
    // A pumpkin of radius r stands 1.26 r tall (pumpkin_point) and nestles 5% into the one below.
    let fit = 2.5 / sizes.iter().map(|r| 1.21 * r).sum::<f32>();
    let mut y = 0.;
    sizes
        .iter()
        .enumerate()
        .map(|(k, &r)| {
            let r = r * fit;
            let s = seed ^ (k as u32 * 37 + 3);
            let at = Vec3::new(0.06 * r * (random(s) - 0.5), y, 0.06 * r * (random(s ^ 1) - 0.5));
            y += 1.21 * r;
            (at, r, k == 0)
        })
        .collect()
}

/// A banner of purple cloth 1 x 2.5 m with a black bat on both faces, on a bracket standing 1.25 m out (+Z) from a
/// wall or post at the origin, 3 m up.
pub(super) fn bat_banner() -> Model {
    let mut m = Model::new();
    let top = 3.;
    tube(m.part(BLACK), &[Vec3::new(0., top, 0.), Vec3::new(0., top, 1.25)], |_| 0.025, 8);
    tube(m.part(BLACK), &[Vec3::new(0., top - 0.6, 0.), Vec3::new(0., top, 0.7)], |_| 0.015, 6);
    surface(m.part(BLACK), 10, 6, ball(Vec3::new(0., top, 1.28), Vec3::splat(0.045)));
    // The cloth hangs across the bracket (local X along it), its tail cut in a swallowtail.
    let hang = Transform::from_xyz(0., top - 0.03, 0.72).with_rotation(Quat::from_rotation_y(-FRAC_PI_2));
    let cloth = [Vec2::new(-0.5, 0.), Vec2::new(-0.5, -2.5), Vec2::new(0., -2.15), Vec2::new(0.5, -2.5), Vec2::new(0.5, 0.)];
    slab(m.part(PURPLE), &cloth, 0.02, hang, false);
    let right = [(0.06, -0.02), (0.14, -0.04), (0.22, 0.04), (0.3, -0.02), (0.38, 0.05), (0.42, 0.2), (0.12, 0.13), (0.06, 0.12), (0.045, 0.2), (0.015, 0.14)];
    let mut bat: Vec<Vec2> = vec![Vec2::new(0., -0.12)];
    bat.extend(right.iter().map(|&(x, y)| Vec2::new(x, y) * 1.1 + Vec2::Y * -1.));
    bat.extend(right.iter().rev().map(|&(x, y)| Vec2::new(-x, y) * 1.1 + Vec2::Y * -1.));
    bat[0] = Vec2::new(0., -0.12 * 1.1 - 1.);
    slab(m.part(BLACK), &bat, 0.03, hang, false);
    m
}

/// A crypt lantern 1.85 m tall: an iron post on a stone foot, a glass lantern lit by a candle, a black cap.
pub(super) fn crypt_lantern() -> Model {
    let mut m = Model::new();
    surface(m.part(STONE), 16, 6, pillow(Vec3::Y * 0.12, Vec3::new(0.18, 0.12, 0.18), 0.3, 0.3));
    tube(m.part(BLACK), &[Vec3::Y * 0.2, Vec3::Y * 1.45], |t| 0.04 - 0.012 * t, 10);
    surface(m.part(BLACK), 16, 4, pillow(Vec3::Y * 1.45, Vec3::new(0.14, 0.03, 0.14), 0.4, 0.3));
    surface(m.part(FLAME), 16, 8, pillow(Vec3::Y * 1.6, Vec3::new(0.1, 0.13, 0.1), 0.2, 0.2));
    for (x, z) in [(1., 1.), (-1., 1.), (1., -1.), (-1., -1.)] {
        tube(m.part(BLACK), &[Vec3::new(0.11 * x, 1.45, 0.11 * z), Vec3::new(0.11 * x, 1.75, 0.11 * z)], |_| 0.012, 5);
    }
    surface(m.part(BLACK), 16, 8, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.19 * spow(v, 0.9);
        Vec3::new(r * spow(co, 0.4), 1.92 - 0.17 * v, r * spow(so, 0.4))
    });
    surface(m.part(BLACK), 8, 6, ball(Vec3::Y * 1.95, Vec3::splat(0.03)));
    m
}

// ---------------------------------------------------------------------------------------------------------------
// Spawning and motion.

/// An inflatable leaning in the breeze about its foot (a ghost also bobbing up and down on its pole).
#[derive(Component)]
pub(super) struct Sway {
    pub(super) at: Vec3,
    pub(super) base: Quat,
    pub(super) seed: f32,
    pub(super) lean: f32,
    pub(super) bob: f32,
}

/// A bat circling `centre` `radius` out, `speed` radians a second (the sign its way round).
#[derive(Component)]
pub(super) struct Bat {
    centre: Vec3,
    radius: f32,
    speed: f32,
    phase: f32,
}

#[derive(Component)]
pub(super) struct Wing {
    side: f32,
    seed: f32,
}

/// A spider on a thread tied at `top` (`thread`: the thread itself), going slowly up and down.
#[derive(Component)]
pub(super) struct Dangle {
    top: Vec3,
    length: f32,
    seed: f32,
    thread: bool,
}

/// Each kind's coloured floodlights (in front of it, model space: position, linear colour, lumens, range) and the
/// glow each throws on the ground (diameter).
pub(super) fn floods(kind: Kind) -> Vec<(Vec3, [f32; 3], f32, f32, f32)> {
    match kind {
        Kind::House => vec![
            (Vec3::new(-1.7, 0.35, 3.1), [0.55, 0.12, 1.], 520_000., 12., 4.5),
            (Vec3::new(1.9, 0.35, 3.), [0.3, 1., 0.12], 420_000., 11., 4.),
            (Vec3::new(-0.27, 0.8, 2.), [1., 0.5, 0.12], 160_000., 6., 3.),
        ],
        Kind::Witch => vec![(Vec3::new(0., 0.3, 1.9), [0.35, 1., 0.18], 320_000., 9., 3.5)],
        Kind::Spider => vec![(Vec3::new(0., 0.3, 2.5), [1., 0.1, 0.06], 340_000., 9., 4.)],
        Kind::Ghost => vec![(Vec3::new(0., 0.3, 1.8), [0.35, 0.55, 1.], 280_000., 9., 3.5)],
        Kind::Graveyard => vec![(Vec3::new(0., 0.45, 0.6), [0.35, 1., 0.3], 220_000., 7., 5.5)],
        Kind::Cauldron => vec![(Vec3::new(0., 1.6, 0.), [0.3, 1., 0.15], 200_000., 7., 3.5)],
        Kind::CryptLantern => vec![(Vec3::new(0., 1.6, 0.), [1., 0.5, 0.12], 60_000., 5., 2.)],
        _ => vec![],
    }
}

/// A roosting bat's wing, wrapped round its body, twitching now and then.
#[derive(Component)]
pub(super) struct Twitch {
    side: f32,
    seed: f32,
}

/// How far a roosting bat's wings wrap round under its belly (radians).
const FOLD: f32 = 1.35;

/// How far a hanging spider goes up and down on its thread (m).
pub(super) const BOB: f32 = 0.15;

/// Spiders hanging on threads tied at each `(top, length)`, going slowly up and down.
pub(super) fn spawn_danglers(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &[Finish], danglers: &[(Vec3, f32)]) {
    if danglers.is_empty() {
        return;
    }
    let little = upload(meshes, little_spider());
    let mut thread = Builder::default();
    tube(&mut thread, &[Vec3::ZERO, Vec3::NEG_Y], |_| 0.0025, 4);
    let thread = meshes.add(thread.mesh());
    let silk = match &materials[GHOST] {
        Finish::Lit(m) => m.clone(),
        Finish::Glow(_) => unreachable!("ghost nylon is lit"),
    };
    for (k, &(top, length)) in danglers.iter().enumerate() {
        let seed = k as f32 * 1.7;
        commands.spawn((
            Decor,
            crate::map_render::MapEntity,
            Dangle { top, length, seed, thread: true },
            Mesh3d(thread.clone()),
            MeshMaterial3d(silk.clone()),
            Transform::from_translation(top).with_scale(Vec3::new(1., length, 1.)),
            NotShadowCaster,
        ));
        commands
            .spawn((
                Name::new("Halloween hanging spider"),
                Decor,
                crate::map_render::MapEntity,
                Dangle { top, length, seed, thread: false },
                Transform::from_translation(top - Vec3::Y * length).with_scale(Vec3::splat(1.6)),
                Visibility::default(),
            ))
            .with_children(|c| spawn_parts(c, &little, materials));
    }
}

/// Bats: eight to a flock circling each of `flocks`, and one hanging upside down by its feet at each of `roosts`,
/// its wings wrapped round it.
pub(super) fn spawn_bats(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &[Finish], flocks: &[Vec3], roosts: &[Vec3]) {
    if flocks.is_empty() && roosts.is_empty() {
        return;
    }
    let body = upload(meshes, bat_body());
    let wings = [1., -1.].map(|side: f32| {
        let mut b = bat_wing();
        for p in &mut b.positions {
            p[0] *= side;
        }
        meshes.add(b.mesh())
    });
    let wing_material = match &materials[WING] {
        Finish::Lit(m) => m.clone(),
        Finish::Glow(_) => unreachable!("wings are lit"),
    };
    for (f, &centre) in flocks.iter().enumerate() {
        for k in 0..8 {
            let s = (f * 8 + k) as u32 * 977 + 5;
            let bat = Bat {
                centre: centre + Vec3::new(4. * (random(s) - 0.5), 3. * (random(s ^ 1) - 0.5), 4. * (random(s ^ 2) - 0.5)),
                radius: 4. + 6. * random(s ^ 3),
                speed: (0.5 + 0.5 * random(s ^ 4)) * if random(s ^ 5) < 0.3 { -1. } else { 1. },
                phase: random(s ^ 6) * TAU,
            };
            let seed = random(s ^ 7) * TAU;
            commands
                .spawn((
                    Name::new("Halloween bat"),
                    Decor,
                    crate::map_render::MapEntity,
                    Transform::from_translation(centre).with_scale(Vec3::splat(1.3)),
                    Visibility::default(),
                    bat,
                ))
                .with_children(|c| {
                    spawn_parts(c, &body, materials);
                    for (w, side) in [1., -1.].into_iter().enumerate() {
                        c.spawn((Wing { side, seed }, Mesh3d(wings[w].clone()), MeshMaterial3d(wing_material.clone()), Transform::from_xyz(0.05 * side, 0.02, 0.)));
                    }
                });
        }
    }
    for (k, &roost) in roosts.iter().enumerate() {
        let s = k as u32 * 613 + 11;
        let seed = random(s) * TAU;
        // Nose down, its tail (0.14 m behind its middle, at scale 1.3) at the roost.
        let hang = Quat::from_rotation_y(seed) * Quat::from_rotation_x(FRAC_PI_2);
        commands
            .spawn((
                Name::new("Halloween roosting bat"),
                Decor,
                crate::map_render::MapEntity,
                Transform::from_translation(roost - Vec3::Y * 0.18).with_rotation(hang).with_scale(Vec3::splat(1.3)),
                Visibility::default(),
            ))
            .with_children(|c| {
                spawn_parts(c, &body, materials);
                for (w, side) in [1., -1.].into_iter().enumerate() {
                    c.spawn((
                        Twitch { side, seed },
                        Mesh3d(wings[w].clone()),
                        MeshMaterial3d(wing_material.clone()),
                        Transform::from_xyz(0.05 * side, 0.02, 0.).with_rotation(Quat::from_rotation_z(-FOLD * side)),
                    ));
                }
            });
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn animate(
    time: Res<Time>,
    mut sways: Query<(&Sway, &mut Transform), (Without<Bat>, Without<Wing>, Without<Dangle>)>,
    mut bats: Query<(&Bat, &mut Transform), (Without<Sway>, Without<Wing>, Without<Dangle>)>,
    mut wings: Query<(&Wing, &mut Transform), (Without<Sway>, Without<Bat>, Without<Dangle>)>,
    mut danglers: Query<(&Dangle, &mut Transform), (Without<Sway>, Without<Bat>, Without<Wing>)>,
    mut twitches: Query<(&Twitch, &mut Transform), (Without<Sway>, Without<Bat>, Without<Wing>, Without<Dangle>)>,
) {
    let t = time.elapsed_secs();
    for (s, mut at) in &mut sways {
        let wind = Quat::from_euler(EulerRot::XYZ, s.lean * (t * 0.9 + s.seed).sin(), 0., s.lean * 0.7 * (t * 0.65 + s.seed * 1.7).sin());
        at.rotation = s.base * wind;
        at.translation = s.at + Vec3::Y * (s.bob * (0.5 + 0.5 * (t * 1.2 + s.seed).sin()));
    }
    for (bat, mut at) in &mut bats {
        let a = bat.phase + bat.speed * t;
        let place = |a: f32| bat.centre + Vec3::new(a.cos() * bat.radius, 0.8 * (2. * a + bat.phase).sin(), a.sin() * bat.radius);
        let here = place(a);
        let ahead = (place(a + 0.05 * bat.speed.signum()) - here).normalize_or(Vec3::Z);
        // Nose along +Z, banked into the turn.
        let look = Transform::from_translation(here).looking_to(-ahead, Vec3::Y).rotation;
        at.translation = here;
        at.rotation = Quat::from_axis_angle(ahead, -0.4 * bat.speed.signum()) * look;
    }
    for (wing, mut at) in &mut wings {
        let flap = 0.15 + 0.8 * (t * 13. + wing.seed).sin();
        at.rotation = Quat::from_rotation_z(flap * wing.side);
    }
    for (d, mut at) in &mut danglers {
        let length = d.length + BOB * (t * 0.7 + d.seed).sin();
        if d.thread {
            at.scale.y = length;
        } else {
            at.translation = d.top - Vec3::Y * length;
            at.rotation = Quat::from_rotation_y(0.4 * (t * 0.3 + d.seed).sin());
        }
    }
    for (w, mut at) in &mut twitches {
        // Still mostly; every few seconds a quick shrug.
        let shrug = (t * 0.8 + w.seed).sin().max(0.).powi(12);
        at.rotation = Quat::from_rotation_z((-FOLD + 0.3 * shrug) * w.side);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every part's triangles face away from the model's middle on average: the finite-difference normals and the
    /// winding agree, so nothing renders inside out.
    #[test]
    fn the_inflatables_are_closed_and_face_out() {
        for (name, model, look, middle) in [
            ("house", witch_house(), PURPLE, Vec3::new(0., 1.6, 0.)),
            ("witch", witch(), PURPLE, Vec3::new(0., 1.3, 0.)),
            ("spider", spider(0., 0.), BLACK, Vec3::new(0., 0.7, -0.1)),
            ("ghost", ghost(), GHOST, Vec3::new(0., 1.4, 0.)),
            ("cauldron", cauldron(), BLACK, Vec3::new(0., 0.75, 0.)),
            ("egg sac", egg_sac(), CORD, Vec3::new(0., -0.45, 0.)),
            ("pole", light_pole(12.), PURPLE, Vec3::new(0., 6., 0.)),
        ] {
            let body = &model.0[look];
            assert!(body.indices.len() > 300, "{name}");
            assert!(outward(body, middle) > 0., "{name} faces in");
            assert!(body.normals.iter().all(|n| Vec3::from(*n).is_finite()), "{name}");
        }
    }

    #[test]
    fn a_draped_spider_hangs_its_legs_below_the_beam() {
        let low = spider(1.2, 0.).0[PURPLE].positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!((low + 1.2).abs() < 0.1, "{low}");
    }

    /// The west gate's spider sits on the deck over the tunnel: its front legs hang down the face, its back legs stand.
    #[test]
    fn a_spider_over_an_edge_drapes_only_its_front_legs() {
        let model = model(Kind::DrapedSpider, 0).unwrap();
        let shins = &model.0[PURPLE].positions;
        let low = |keep: fn(f32) -> bool| shins.iter().filter(|p| keep(p[2])).map(|p| p[1]).fold(f32::MAX, f32::min);
        let (front, back) = (low(|z| z > 0.6), low(|z| z < 0.4));
        assert!((front + 1.2).abs() < 0.1, "front feet at {front}");
        assert!(back.abs() < 0.1, "back feet at {back}");
    }

    /// The car spider sits on the red SUV's roof (user 10-20): every foot hangs over the roof's edges, gripping them.
    #[test]
    fn a_car_spider_grips_the_roof_edges_with_every_leg() {
        let model = model(Kind::CarSpider, 0).unwrap();
        let shins = &model.0[PURPLE].positions;
        let low = |keep: fn(f32) -> bool| shins.iter().filter(|p| keep(p[2])).map(|p| p[1]).fold(f32::MAX, f32::min);
        let (front, back) = (low(|z| z > 0.6), low(|z| z < 0.4));
        assert!((front + 0.8).abs() < 0.1, "front feet at {front}");
        assert!((back + 0.8).abs() < 0.1, "back feet at {back}");
    }

    #[test]
    fn every_halloween_kind_with_a_model_builds_one() {
        for kind in [
            Kind::House,
            Kind::Witch,
            Kind::Spider,
            Kind::DrapedSpider,
            Kind::CarSpider,
            Kind::Ghost,
            Kind::HangingGhost,
            Kind::Graveyard,
            Kind::Hay,
            Kind::Cauldron,
            Kind::Broom,
            Kind::Skeleton,
            Kind::EggSac,
            Kind::LightPole,
            Kind::BatBanner,
            Kind::CryptLantern,
            Kind::Fence,
        ] {
            assert!(model(kind, 7).is_some_and(|m| m.0.iter().any(|b| !b.indices.is_empty())), "{kind:?}");
        }
    }

    #[test]
    fn graveyards_vary_with_their_seed() {
        let (a, b) = (graveyard(1), graveyard(9));
        assert!(a.0[STONE].positions.len() > 100);
        assert_ne!(a.0[STONE].positions.len() + a.0[DIRT].positions.len(), 0);
        assert!(a.0[STONE].positions != b.0[STONE].positions);
    }
}
