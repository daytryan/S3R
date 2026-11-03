//! Not retail (user 10-19): the Christmas mode's decorations, which seasonal.rs places about the Super Ultra Mega
//! Park: the big tree, snowmen, candy canes, nutcrackers, wreaths, presents, a gingerbread house, Santa's sleigh,
//! reindeer of light, snow globes, sleds and lanterns. All built here in code. Not tested in play.

use super::layout::{Kind, Palette, Style};
use super::model::*;
use super::random;
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// The model of a Christmas piece of `kind` (seeded by `seed`), or None for kinds that are not Christmas's.
pub(super) fn model(kind: Kind, seed: u32) -> Option<Model> {
    Some(match kind {
        Kind::BigTree => big_tree(14.),
        Kind::Snowman => snowman(seed),
        Kind::CandyCane => candy_cane(),
        Kind::Nutcracker => nutcracker(seed),
        Kind::Wreath => wreath(),
        Kind::Presents => presents(seed),
        Kind::Gingerbread => gingerbread_house(seed),
        Kind::Sleigh => sleigh(),
        Kind::Reindeer => reindeer(seed),
        Kind::SnowGlobe => snow_globe(),
        Kind::Sled => sled(),
        Kind::Lantern => lantern(),
        _ => return None,
    })
}

/// Each kind's floodlights, as halloween.rs's: (spot in front of it, linear colour, lumens, range, glow diameter).
pub(super) fn floods(kind: Kind) -> Vec<(Vec3, [f32; 3], f32, f32, f32)> {
    match kind {
        Kind::BigTree => vec![
            (Vec3::new(-4., 1., 7.5), [1., 0.72, 0.4], 700_000., 18., 9.),
            (Vec3::new(4.5, 1., -7.), [1., 0.72, 0.4], 600_000., 18., 9.),
        ],
        Kind::Gingerbread => vec![(Vec3::new(0., 0.4, 2.6), [1., 0.6, 0.3], 300_000., 9., 4.5)],
        Kind::SnowGlobe => vec![(Vec3::new(0., 0.4, 2.2), [0.5, 0.7, 1.], 260_000., 8., 4.)],
        Kind::Nutcracker => vec![(Vec3::new(0., 0.2, 1.3), [1., 0.7, 0.4], 240_000., 8., 3.)],
        // Not retail (user 10-20): the lanterns light the pit and the south rim, which had no lamp.
        Kind::Lantern => vec![(Vec3::new(0., 1.6, 0.), [1., 0.72, 0.4], 60_000., 5., 2.)],
        _ => vec![],
    }
}

/// The big tree's outline: its radius at height `y` (a cone from its lowest boughs to its tip).
fn outline(height: f32, y: f32) -> f32 {
    let foot = 0.06 * height;
    0.46 * height * (1. - ((y - foot) / (height - foot)).clamp(0., 1.))
}

/// The centrepiece tree, `height` m to its tip (its star spins on top, spawned apart): a trunk, a dense cone of
/// needles inside so no sky shows through, 14 tiers of drooping fronds tufted with needles, about 80 baubles, a gold
/// bead garland and a spiral of big multicoloured bulbs fitted to the cone.
pub(super) fn big_tree(height: f32) -> Model {
    let mut m = Model::new();
    let foot = 0.06 * height;
    tube(m.part(BROWN), &[Vec3::ZERO, Vec3::Y * (foot + 1.)], |_| 0.03 * height, 10);
    // The inner cone, closed underneath.
    surface(m.part(NEEDLE), 40, 24, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let (r, y) = if v < 0.92 {
            let y = height - 0.4 - (height - 0.4 - foot) * v / 0.92;
            (outline(height, y) * 0.82, y)
        } else {
            (outline(height, foot) * 0.82 * (1. - v) / 0.08, foot + 0.4 * (v - 0.92) / 0.08)
        };
        Vec3::new(r * co, y, r * so)
    });
    const TIERS: usize = 14;
    for k in 0..TIERS {
        let y = foot + (height - foot - 1.2) * k as f32 / TIERS as f32;
        let fronds = 14 - k * 5 / (TIERS - 1);
        for j in 0..fronds {
            let a = TAU * (j as f32 + 0.5 * (k % 2) as f32) / fronds as f32 + 0.3 * k as f32;
            let out = Vec3::new(a.cos(), 0., a.sin());
            let droop = 0.12 * outline(height, y);
            let tip = out * outline(height, y - droop) + Vec3::Y * (y - droop);
            let root = out * 0.3 + Vec3::Y * (y + 0.25);
            let frond = curve(6, |t| bend(root, (root + tip) * 0.5 + Vec3::Y * (0.35 * droop), tip, t));
            tube(m.part(NEEDLE), &frond, |t| 0.1 * (1. - t) + 0.03, 6);
            // Needle tufts every 35 cm down it, fuller toward its tip.
            let length = root.distance(tip);
            let tufts = (length / 0.35) as usize;
            for i in 1..=tufts {
                let t = i as f32 / tufts as f32;
                let p = bend(root, (root + tip) * 0.5 + Vec3::Y * (0.35 * droop), tip, t);
                let size = 0.18 + 0.14 * t;
                surface(m.part(NEEDLE), 8, 5, ball(p, Vec3::new(size, size * 0.45, size)));
            }
        }
    }
    // Baubles, more toward the bottom, just outside the boughs.
    let looks = [BAUBLE_RED, BAUBLE_GOLD, SILVER];
    for i in 0..80 {
        let t = ((i as f32 + 0.5) / 80.).sqrt();
        let y = height - 1.5 - (height - 1.5 - foot - 0.3) * t;
        let a = i as f32 * 2.399_963;
        let at = Vec3::new(a.cos(), 0., a.sin()) * (outline(height, y) + 0.12) + Vec3::Y * y;
        surface(m.part(looks[i % 3]), 12, 8, ball(at, Vec3::splat(0.17 + 0.05 * (i % 2) as f32)));
    }
    // The bead garland and the bulbs, wound round the cone.
    let (low, high) = (foot + 0.6, height - 0.8);
    let beads = super::strung::wire(
        &[Vec3::Y * low, Vec3::Y * high],
        Style::Spiral { radius: outline(height, low) + 0.25, top: outline(height, high) + 0.25, turns: 4.5 },
    );
    for p in super::strung::bulbs(&beads, 0.14) {
        surface(m.part(RIBBON), 6, 4, ball(p, Vec3::splat(0.045)));
    }
    let spiral = Style::Spiral { radius: outline(height, low + 0.3) + 0.2, top: outline(height, high) + 0.2, turns: 7. };
    super::strung::lights(&mut m, &[Vec3::Y * (low + 0.3), Vec3::Y * high], spiral, Palette::Multi, false, 0);
    m
}

/// A five-pointed star, `r` to its points, `depth` thick, facing +Z.
pub(super) fn star(r: f32, depth: f32) -> Mesh {
    let mut b = Builder::default();
    let points: Vec<Vec3> = (0..10)
        .map(|k| {
            let a = PI / 2. + TAU * k as f32 / 10.;
            let reach = if k % 2 == 0 { r } else { r * 0.42 };
            Vec3::new(a.cos() * reach, a.sin() * reach, 0.)
        })
        .collect();
    for side in [1., -1.] {
        let centre = b.vertex(Vec3::Z * depth * 0.5 * side, Vec3::Z * side, Vec2::ZERO);
        let rim: Vec<u32> = points.iter().map(|&p| b.vertex(p, Vec3::Z * side, Vec2::ZERO)).collect();
        for k in 0..10 {
            b.triangle(centre, rim[k], rim[(k + 1) % 10]);
        }
    }
    b.mesh()
}

/// A snowman 1.8 m tall: three balls of snow, coal eyes, smile and buttons, a carrot nose, stick arms, and by seed a
/// top hat or a bobble hat and a scarf in red, green or blue.
pub(super) fn snowman(seed: u32) -> Model {
    let mut m = Model::new();
    let (bottom, middle, head) = (Vec3::Y * 0.42, Vec3::Y * 1.02, Vec3::Y * 1.47);
    surface(m.part(SNOW), 32, 18, ball(bottom, Vec3::new(0.5, 0.44, 0.5)));
    surface(m.part(SNOW), 28, 16, ball(middle, Vec3::splat(0.34)));
    surface(m.part(SNOW), 24, 14, ball(head, Vec3::splat(0.24)));
    for x in [-0.08, 0.08] {
        surface(m.part(COAL), 8, 6, ball(head + Vec3::new(x, 0.06, 0.215), Vec3::splat(0.03)));
    }
    for k in 0..5 {
        let a = -0.5 + 0.25 * k as f32;
        surface(m.part(COAL), 6, 4, ball(head + Vec3::new(a.sin() * 0.13, -0.08 + 0.03 * a.abs(), 0.2), Vec3::splat(0.018)));
    }
    for y in [0.9, 1.04, 1.18] {
        let z = (0.34f32.powi(2) - (y - middle.y).powi(2)).sqrt();
        surface(m.part(COAL), 8, 6, ball(Vec3::new(0., y, z + 0.005), Vec3::splat(0.035)));
    }
    tube(m.part(CARROT), &[head + Vec3::new(0., 0., 0.2), head + Vec3::new(0., -0.02, 0.45)], |t| 0.04 * (1. - t) + 0.004, 8);
    for side in [-1., 1.] {
        let shoulder = middle + Vec3::new(0.3 * side, 0.08, 0.);
        let hand = shoulder + Vec3::new(0.55 * side, 0.3 + 0.1 * side, 0.05);
        tube(m.part(BROWN), &[shoulder, hand], |t| 0.025 - 0.01 * t, 6);
        let fork = shoulder.lerp(hand, 0.7);
        tube(m.part(BROWN), &[fork, fork + Vec3::new(0.12 * side, 0.15, 0.)], |_| 0.012, 5);
    }
    let colour = [RED, WRAP_GREEN, WRAP_BLUE][(random(seed) * 3.) as usize % 3];
    let neck = Vec3::Y * 1.27;
    tube(m.part(colour), &curve(24, |t| neck + Vec3::new((TAU * t).cos() * 0.22, 0.02 * (TAU * t * 3.).sin(), (TAU * t).sin() * 0.22)), |_| 0.06, 8);
    tube(m.part(colour), &curve(6, |t| neck + Vec3::new(0.12 + 0.06 * t, -0.42 * t, 0.17 + 0.05 * t)), |_| 0.05, 8);
    if random(seed ^ 1) < 0.5 {
        let brim = head + Vec3::Y * 0.2;
        surface(m.part(COAL), 24, 6, pillow(brim, Vec3::new(0.3, 0.025, 0.3), 0.5, 1.));
        surface(m.part(COAL), 24, 10, pillow(brim + Vec3::Y * 0.2, Vec3::new(0.18, 0.2, 0.18), 0.15, 1.));
        surface(m.part(colour), 24, 4, pillow(brim + Vec3::Y * 0.06, Vec3::new(0.185, 0.035, 0.185), 0.3, 1.));
    } else {
        surface(m.part(colour), 24, 12, pillow(head + Vec3::Y * 0.12, Vec3::new(0.25, 0.2, 0.25), 0.8, 1.));
        surface(m.part(WHITE), 24, 4, pillow(head + Vec3::Y * 0.07, Vec3::new(0.255, 0.05, 0.255), 0.4, 1.));
        surface(m.part(WHITE), 12, 8, ball(head + Vec3::Y * 0.34, Vec3::splat(0.08)));
    }
    m
}

/// A candy cane 2.4 m tall, striped red and white, its hook to +X.
pub(super) fn candy_cane() -> Model {
    let mut m = Model::new();
    let (r, top) = (0.3, 2.1);
    let path: Vec<Vec3> = curve(80, |t| {
        if t < 0.6 {
            Vec3::Y * (top * t / 0.6)
        } else {
            let a = PI * (t - 0.6) / 0.4;
            Vec3::new(r - r * a.cos(), top + r * a.sin(), 0.)
        }
    });
    // Stripes: alternate red and white lengths of about 12 cm.
    let mut run = 0.;
    for w in path.windows(2) {
        tube(m.part(if (run / 0.12) as usize % 2 == 0 { RED } else { WHITE }), w, |_| 0.07, 10);
        run += w[0].distance(w[1]);
    }
    m
}

/// A nutcracker soldier 4.2 m tall: black boots and busby, white trousers, a red coat with gold buttons and belt,
/// white gloves, a painted face with rosy cheeks, a white beard and a plume.
pub(super) fn nutcracker(seed: u32) -> Model {
    let mut m = Model::new();
    surface(m.part(GOLD), 32, 8, pillow(Vec3::Y * 0.12, Vec3::new(0.8, 0.12, 0.6), 0.3, 0.4));
    for side in [-1., 1.] {
        let x = 0.2 * side;
        surface(m.part(BLACK), 16, 10, pillow(Vec3::new(x, 0.55, 0.05), Vec3::new(0.17, 0.32, 0.22), 0.4, 0.8));
        tube(m.part(WHITE), &[Vec3::new(x, 0.85, 0.), Vec3::new(x, 1.55, 0.)], |_| 0.15, 12);
    }
    let coat = Vec3::Y * 2.1;
    surface(m.part(RED), 32, 18, pillow(coat, Vec3::new(0.48, 0.62, 0.36), 0.5, 0.8));
    surface(m.part(GOLD), 32, 4, pillow(coat - Vec3::Y * 0.42, Vec3::new(0.5, 0.06, 0.38), 0.5, 0.8));
    for k in 0..4 {
        for x in [-0.13, 0.13] {
            surface(m.part(GOLD), 8, 6, ball(Vec3::new(x, 1.85 + 0.17 * k as f32, 0.35), Vec3::splat(0.035)));
        }
    }
    for side in [-1., 1.] {
        let shoulder = coat + Vec3::new(0.5 * side, 0.42, 0.);
        let hand = coat + Vec3::new(0.62 * side, -0.25, 0.18);
        tube(m.part(RED), &[shoulder, hand], |t| 0.13 - 0.03 * t, 12);
        surface(m.part(GOLD), 12, 6, pillow(shoulder + Vec3::Y * 0.08, Vec3::new(0.16, 0.06, 0.14), 0.5, 1.));
        surface(m.part(WHITE), 12, 8, ball(hand, Vec3::splat(0.12)));
    }
    let head = Vec3::Y * 3.05;
    surface(m.part(BONE), 24, 14, ball(head, Vec3::new(0.3, 0.33, 0.3)));
    for x in [-0.15, 0.15] {
        surface(m.part(RED), 10, 6, ball(head + Vec3::new(x, -0.06, 0.24), Vec3::new(0.06, 0.05, 0.03)));
        surface(m.part(COAL), 8, 6, ball(head + Vec3::new(x * 0.7, 0.07, 0.27), Vec3::splat(0.035)));
    }
    surface(m.part(WHITE), 20, 10, pillow(head + Vec3::new(0., -0.25, 0.12), Vec3::new(0.26, 0.2, 0.18), 0.7, 1.));
    tube(m.part(BONE), &[head + Vec3::new(0., 0.0, 0.28), head + Vec3::new(0., -0.03, 0.36)], |t| 0.05 * (1. - t) + 0.02, 8);
    let busby = head + Vec3::Y * 0.5;
    surface(m.part(BLACK), 24, 12, pillow(busby, Vec3::new(0.32, 0.38, 0.32), 0.25, 1.));
    surface(m.part(GOLD), 24, 4, pillow(busby - Vec3::Y * 0.3, Vec3::new(0.33, 0.04, 0.33), 0.3, 1.));
    let plume = [WHITE, RED, GOLD][(random(seed) * 3.) as usize % 3];
    tube(m.part(plume), &curve(6, |t| bend(busby + Vec3::new(0.2, 0.2, 0.15), busby + Vec3::new(0.3, 0.55, 0.15), busby + Vec3::new(0.1, 0.75, 0.1), t)), |t| 0.07 * (1. - t) + 0.02, 8);
    m
}

/// A wreath 4 m across facing +Z about the origin (its middle): pine clumps round a ring, warm lights through them,
/// red berries and a red bow with tails at the bottom.
pub(super) fn wreath() -> Model {
    let mut m = Model::new();
    let ring = |a: f32, r: f32| Vec3::new(a.cos() * r, a.sin() * r, 0.);
    tube(m.part(PINE), &curve(48, |t| ring(TAU * t, 1.6)), |_| 0.3, 10);
    for k in 0..72 {
        let a = TAU * k as f32 / 72.;
        let s = k as u32 * 31 + 5;
        let at = ring(a, 1.6 + 0.25 * (random(s) - 0.5)) + Vec3::Z * (0.15 * random(s ^ 1));
        surface(m.part(if k % 3 == 0 { NEEDLE } else { PINE }), 8, 5, ball(at, Vec3::new(0.26, 0.26, 0.16)));
        if k % 4 == 1 {
            for b in 0..3 {
                let p = at + Vec3::new(0.05 * b as f32, 0.04 * (b % 2) as f32, 0.18);
                surface(m.part(RED), 6, 4, ball(p, Vec3::splat(0.04)));
            }
        }
    }
    let lights: Vec<Vec3> = curve(40, |t| ring(TAU * t, 1.62) + Vec3::Z * 0.32);
    super::strung::lights(&mut m, &lights, Style::Taut, Palette::Warm, false, 0);
    let knot = ring(-FRAC_PI_2, 1.6) + Vec3::Z * 0.35;
    surface(m.part(RED), 12, 8, ball(knot, Vec3::new(0.16, 0.13, 0.08)));
    for side in [-1., 1.] {
        let lp = curve(12, |t| {
            let a = TAU * t;
            knot + Vec3::new(side * (0.35 - 0.35 * a.cos()), 0.18 * a.sin(), 0.)
        });
        tube(m.part(RED), &lp, |_| 0.07, 8);
        let tail = curve(6, |t| knot + Vec3::new(side * 0.25 * t, -0.7 * t, 0.05 * (t * 6.).sin()));
        tube(m.part(RED), &tail, |_| 0.06, 8);
    }
    m
}

/// A heap of 3 to 6 wrapped presents, about 1.5 m across, each tied with ribbon and a bow.
pub(super) fn presents(seed: u32) -> Model {
    let mut m = Model::new();
    let count = 3 + (random(seed) * 4.) as usize % 4;
    let wraps = [WRAP_RED, WRAP_GREEN, WRAP_BLUE, WRAP_WHITE];
    for k in 0..count {
        let s = seed ^ (k as u32 * 53 + 9);
        let size = Vec3::new(0.25 + 0.2 * random(s), 0.18 + 0.2 * random(s ^ 1), 0.25 + 0.2 * random(s ^ 2));
        let a = TAU * k as f32 / count as f32 + random(s ^ 3);
        let (at, y) = if k == count - 1 && count > 3 { (Vec3::ZERO, 0.45) } else { (Vec3::new(a.cos(), 0., a.sin()) * 0.45, 0.) };
        let turn = Transform::from_translation(at + Vec3::Y * (y + size.y)).with_rotation(Quat::from_rotation_y(random(s ^ 4) * PI));
        let mut gift = Model::new();
        surface(gift.part(wraps[(s as usize) % 4]), 24, 12, pillow(Vec3::ZERO, size, 0.12, 0.12));
        let ribbon = if wraps[(s as usize) % 4] == WRAP_RED { GOLD } else { RIBBON };
        surface(gift.part(ribbon), 24, 12, pillow(Vec3::ZERO, Vec3::new(size.x + 0.008, size.y + 0.008, 0.035), 0.12, 0.12));
        surface(gift.part(ribbon), 24, 12, pillow(Vec3::ZERO, Vec3::new(0.035, size.y + 0.01, size.z + 0.01), 0.12, 0.12));
        for side in [-1., 1.] {
            let lp = curve(10, |t| {
                let a = TAU * t;
                Vec3::new(side * (0.1 - 0.1 * a.cos()), size.y + 0.02 + 0.07 * a.sin().abs(), 0.)
            });
            tube(gift.part(ribbon), &lp, |_| 0.025, 6);
        }
        m.append(gift, turn);
    }
    m
}

/// A gingerbread house 3.5 m wide, its front +Z: gingerbread walls and a pitched roof under snow, white icing along
/// every edge, candy studs, gumdrops round its foot, lit windows and a door.
pub(super) fn gingerbread_house(seed: u32) -> Model {
    let mut m = Model::new();
    let (w, d, wall) = (1.75, 1.3, 2.);
    surface(m.part(GINGER), 32, 16, pillow(Vec3::Y * (wall * 0.5), Vec3::new(w, wall * 0.5, d), 0.12, 0.08));
    // The gables and the roof, slabs leaning to the ridge.
    let ridge = wall + 1.3;
    let gable = [Vec2::new(-w, wall - 0.05), Vec2::new(w, wall - 0.05), Vec2::new(0., ridge)];
    for z in [d - 0.05, -d + 0.05] {
        slab(m.part(GINGER), &gable, 0.12, Transform::from_xyz(0., 0., z), false);
    }
    let pitch = (1.3f32).atan2(w);
    let span = (w * w + 1.3 * 1.3).sqrt() + 0.3;
    for side in [-1., 1.] {
        let at = Transform::from_xyz(side * w * 0.5, wall + 0.65, 0.).with_rotation(Quat::from_rotation_z(-side * pitch) * Quat::from_rotation_x(FRAC_PI_2));
        slab(m.part(GINGER), &rect(span, 2. * d + 0.4), 0.12, at, false);
        let snow = Transform::from_xyz(side * (w * 0.5 - 0.04), wall + 0.72, 0.).with_rotation(Quat::from_rotation_z(-side * pitch) * Quat::from_rotation_x(FRAC_PI_2));
        slab(m.part(SNOW), &rect(span - 0.05, 2. * d + 0.45), 0.1, snow, false);
        // Icing scallops along the eave.
        let eave = curve(40, |t| Vec3::new(side * (w + 0.12), wall - 0.02 - 0.06 * (t * 20. * PI).sin().abs(), -d - 0.2 + (2. * d + 0.4) * t));
        tube(m.part(ICING), &eave, |_| 0.05, 6);
    }
    for z in [d + 0.03, -d - 0.03] {
        for side in [-1., 1.] {
            tube(m.part(ICING), &[Vec3::new(side * (w + 0.1), wall, z), Vec3::new(0., ridge + 0.06, z)], |_| 0.06, 6);
        }
    }
    // Candy studs under the eaves and gumdrops round the foot.
    let candy = [RED, WRAP_GREEN, WRAP_BLUE, WHITE];
    for k in 0..14 {
        let x = -w + 2. * w * (k as f32 + 0.5) / 14.;
        surface(m.part(candy[k % 4]), 8, 6, ball(Vec3::new(x, wall - 0.2, d + 0.02), Vec3::splat(0.06)));
    }
    for k in 0..18 {
        let s = seed ^ (k as u32 * 17 + 1);
        let a = TAU * k as f32 / 18.;
        let at = Vec3::new(a.cos() * (w + 0.25), 0., a.sin() * (d + 0.25));
        surface(m.part(candy[(s as usize) % 4]), 12, 8, pillow(at + Vec3::Y * 0.1, Vec3::new(0.14, 0.12, 0.14), 0.6, 1.));
    }
    // Windows with icing frames, and the door.
    let front = d + 0.012;
    for x in [-1.05, 1.05] {
        decal(m.part(ICING), &rect(0.62, 0.62), Vec3::new(x, 1.2, front), Vec2::ONE);
        decal(m.part(WINDOW), &rect(0.5, 0.5), Vec3::new(x, 1.2, front + 0.004), Vec2::ONE);
        decal(m.part(ICING), &rect(0.04, 0.5), Vec3::new(x, 1.2, front + 0.008), Vec2::ONE);
        decal(m.part(ICING), &rect(0.5, 0.04), Vec3::new(x, 1.2, front + 0.008), Vec2::ONE);
    }
    decal(m.part(ICING), &arch(0.82, 1.42), Vec3::new(0., 0., front), Vec2::ONE);
    decal(m.part(RED), &arch(0.7, 1.35), Vec3::new(0., 0., front + 0.004), Vec2::ONE);
    surface(m.part(GOLD), 8, 6, ball(Vec3::new(0.22, 0.7, front + 0.03), Vec3::splat(0.04)));
    tube(m.part(RED), &[Vec3::new(0.9, ridge - 0.6, -0.4), Vec3::new(0.9, ridge + 0.3, -0.4)], |_| 0.18, 10);
    m
}

/// Santa's sleigh 3 m long, its front +Z: a red body swept up at the front, gold runners curling over, a seat and a
/// sack of presents behind it.
pub(super) fn sleigh() -> Model {
    let mut m = Model::new();
    surface(m.part(RED), 40, 16, |u, v| {
        let p = pillow(Vec3::ZERO, Vec3::new(0.75, 0.45, 1.3), 0.25, 0.25)(u, v);
        // Swept up toward the front.
        let lift = ((p.z / 1.3 + 0.2).max(0.) / 1.2).powi(2) * 0.5;
        Vec3::new(p.x, 0.95 + p.y + lift, p.z)
    });
    surface(m.part(RED), 24, 10, pillow(Vec3::new(0., 1.55, -0.5), Vec3::new(0.65, 0.35, 0.18), 0.3, 0.3));
    surface(m.part(GOLD), 40, 4, pillow(Vec3::Y * 1.42, Vec3::new(0.78, 0.04, 1.33), 0.3, 0.25));
    for side in [-1., 1.] {
        let x = 0.6 * side;
        let runner = curve(30, |t| {
            if t < 0.7 {
                Vec3::new(x, 0.06, -1.4 + 2.9 * t / 0.7)
            } else {
                let a = PI * 1.3 * (t - 0.7) / 0.3;
                Vec3::new(x, 0.06 + 0.35 * (1. - a.cos()), 1.5 + 0.35 * a.sin())
            }
        });
        tube(m.part(GOLD), &runner, |_| 0.05, 8);
        for z in [-0.9, 0., 0.8] {
            tube(m.part(GOLD), &[Vec3::new(x, 0.06, z), Vec3::new(x * 0.9, 0.6, z)], |_| 0.035, 6);
        }
    }
    surface(m.part(BROWN), 24, 14, ball(Vec3::new(0., 1.85, -0.95), Vec3::new(0.5, 0.6, 0.42)));
    tube(m.part(GOLD), &curve(16, |t| Vec3::new((TAU * t).cos() * 0.2, 2.35, -0.95 + (TAU * t).sin() * 0.18)), |_| 0.03, 6);
    m.append(presents(11), Transform::from_xyz(0., 1.2, 0.3).with_scale(Vec3::splat(0.55)));
    m
}

/// A reindeer of light 2.2 m tall, its nose +Z: its outline in warm bulbs on a wire frame, its head up or grazing by
/// seed.
pub(super) fn reindeer(seed: u32) -> Model {
    let mut m = Model::new();
    let grazing = random(seed) < 0.35;
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let mut outline = |points: &[Vec3]| super::strung::lights(&mut m, points, Style::Taut, Palette::Warm, false, 0);
    let back = [p(0., 1.25, -0.7), p(0., 1.35, -0.2), p(0., 1.32, 0.35), p(0., 1.25, 0.55)];
    outline(&back);
    outline(&[p(0., 1.2, -0.75), p(0., 0.95, -0.7), p(0., 0.85, -0.2), p(0., 0.85, 0.3), p(0., 0.95, 0.55)]);
    let (neck, nose) = if grazing { (p(0., 0.9, 0.95), p(0., 0.35, 1.1)) } else { (p(0., 1.7, 0.85), p(0., 1.75, 1.2)) };
    outline(&[back[3], neck, nose]);
    outline(&[p(0., 0.95, 0.55), neck.lerp(nose, 0.3) - Vec3::Y * 0.15, nose]);
    let crown = neck.lerp(nose, 0.25) + Vec3::Y * 0.12;
    for side in [-1., 1.] {
        let x = 0.1 * side;
        outline(&[crown + p(x, 0., 0.), crown + p(x * 2., 0.35, -0.15), crown + p(x * 3., 0.6, -0.05), crown + p(x * 3.5, 0.75, 0.15)]);
        outline(&[crown + p(x * 2., 0.35, -0.15), crown + p(x * 4., 0.42, -0.3)]);
        for z in [-0.6f32, 0.45] {
            outline(&[p(x * 1.3, 0.88, z), p(x * 1.4, 0.45, z + 0.05), p(x * 1.4, 0., z)]);
        }
    }
    outline(&[p(0., 1.25, -0.7), p(0., 1.4, -0.85)]);
    m
}

/// A snow globe 3 m tall: a red and gold base, a glass dome over a snowy mound with a little snowman and a fir.
pub(super) fn snow_globe() -> Model {
    let mut m = Model::new();
    surface(m.part(RED), 32, 10, pillow(Vec3::Y * 0.3, Vec3::new(1.15, 0.3, 1.15), 0.3, 1.));
    surface(m.part(GOLD), 32, 4, pillow(Vec3::Y * 0.58, Vec3::new(1.18, 0.05, 1.18), 0.4, 1.));
    surface(m.part(GLASS), 40, 24, ball(Vec3::Y * 1.75, Vec3::splat(1.25)));
    surface(m.part(SNOW), 32, 8, pillow(Vec3::Y * 0.7, Vec3::new(1.05, 0.18, 1.05), 0.7, 1.));
    m.append(snowman(5), Transform::from_xyz(-0.25, 0.8, 0.2).with_scale(Vec3::splat(0.5)));
    surface(m.part(NEEDLE), 16, 10, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.35 * v * (1. + 0.15 * (v * 18.).sin().abs());
        Vec3::new(0.4 + r * co, 2.2 - 1.3 * v, -0.2 + r * so)
    });
    m
}

/// A child's sled 1.2 m long, its front +Z: wooden slats on red runners curling up at the front, a tow rope.
pub(super) fn sled() -> Model {
    let mut m = Model::new();
    for k in 0..5 {
        let z = -0.5 + 0.22 * k as f32;
        surface(m.part(BROWN), 16, 4, pillow(Vec3::new(0., 0.24, z), Vec3::new(0.3, 0.025, 0.09), 0.3, 0.2));
    }
    for side in [-1., 1.] {
        let x = 0.26 * side;
        let runner = curve(16, |t| {
            if t < 0.7 {
                Vec3::new(x, 0.03, -0.6 + 1.1 * t / 0.7)
            } else {
                let a = PI * (t - 0.7) / 0.3;
                Vec3::new(x, 0.03 + 0.12 * (1. - a.cos()), 0.5 + 0.12 * a.sin())
            }
        });
        tube(m.part(RED), &runner, |_| 0.025, 6);
        for z in [-0.4, 0.3] {
            tube(m.part(RED), &[Vec3::new(x, 0.03, z), Vec3::new(x, 0.22, z)], |_| 0.02, 5);
        }
    }
    tube(m.part(BROWN), &curve(10, |t| bend(Vec3::new(-0.24, 0.27, 0.62), Vec3::new(0., 0.1, 1.2), Vec3::new(0.24, 0.27, 0.62), t)), |_| 0.012, 5);
    m
}

/// A lantern on a post 1.8 m tall: an iron post, warm glass, a black cap, and a red bow on a pine tuft below it.
pub(super) fn lantern() -> Model {
    let mut m = Model::new();
    surface(m.part(BLACK), 16, 6, pillow(Vec3::Y * 0.08, Vec3::new(0.16, 0.08, 0.16), 0.4, 1.));
    tube(m.part(BLACK), &[Vec3::Y * 0.1, Vec3::Y * 1.45], |t| 0.035 - 0.01 * t, 10);
    surface(m.part(BLACK), 16, 4, pillow(Vec3::Y * 1.45, Vec3::new(0.13, 0.03, 0.13), 0.4, 0.3));
    surface(m.part(BULB_WARM), 16, 8, pillow(Vec3::Y * 1.6, Vec3::new(0.1, 0.13, 0.1), 0.2, 0.2));
    surface(m.part(BLACK), 16, 8, |u, v| {
        let (so, co) = (TAU * u).sin_cos();
        let r = 0.18 * spow(v, 0.9);
        Vec3::new(r * spow(co, 0.4), 1.9 - 0.16 * v, r * spow(so, 0.4))
    });
    surface(m.part(PINE), 12, 8, ball(Vec3::new(0., 1.3, 0.05), Vec3::new(0.16, 0.1, 0.14)));
    surface(m.part(RED), 10, 6, ball(Vec3::new(0., 1.3, 0.17), Vec3::new(0.06, 0.05, 0.04)));
    for side in [-1., 1.] {
        tube(m.part(RED), &curve(8, |t| Vec3::new(0., 1.3, 0.17) + Vec3::new(side * 0.1 * (1. - (TAU * t).cos()), 0.05 * (TAU * t).sin(), 0.)), |_| 0.025, 6);
        tube(m.part(RED), &[Vec3::new(0., 1.3, 0.17), Vec3::new(side * 0.08, 1.12, 0.19)], |_| 0.02, 5);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_big_tree_is_full_and_dressed() {
        let t = big_tree(14.);
        assert!(t.0[NEEDLE].indices.len() > 20_000);
        assert!(t.0[BAUBLE_RED].positions.len() + t.0[BAUBLE_GOLD].positions.len() + t.0[SILVER].positions.len() > 0);
        let bulbs: usize = (MULTI..MULTI + 5).map(|k| t.0[k].positions.len()).sum();
        assert!(bulbs > 0);
        // Full: at mid-height the needles reach at least a quarter of the height out.
        let reach = t.0[NEEDLE].positions.iter().filter(|p| (p[1] - 7.).abs() < 0.5).map(|p| Vec2::new(p[0], p[2]).length()).fold(0., f32::max);
        assert!(reach > 3.5, "{reach}");
    }

    #[test]
    fn the_big_tree_has_room_for_its_lowest_boughs() {
        let t = big_tree(14.);
        let reach = t.0.iter().flat_map(|b| b.positions.iter()).map(|p| Vec2::new(p[0], p[2]).length()).fold(0., f32::max);
        assert!(Kind::BigTree.room().0 >= reach, "the boughs reach {reach} m");
    }

    #[test]
    fn snowmen_have_faces_hats_and_arms() {
        let s = snowman(3);
        assert!([SNOW, COAL, CARROT, BROWN].iter().all(|&k| !s.0[k].indices.is_empty()));
    }

    #[test]
    fn every_christmas_kind_builds_a_model() {
        for kind in [
            Kind::BigTree,
            Kind::Snowman,
            Kind::CandyCane,
            Kind::Nutcracker,
            Kind::Wreath,
            Kind::Presents,
            Kind::Gingerbread,
            Kind::Sleigh,
            Kind::Reindeer,
            Kind::SnowGlobe,
            Kind::Sled,
            Kind::Lantern,
        ] {
            assert!(model(kind, 7).is_some_and(|m| m.0.iter().any(|b| !b.indices.is_empty())), "{kind:?}");
        }
    }

    #[test]
    fn the_round_pieces_are_closed_and_face_out() {
        for (name, model, look, middle) in [
            ("snowman", snowman(1), SNOW, Vec3::Y * 0.9),
            ("snow globe", snow_globe(), GLASS, Vec3::Y * 1.75),
            ("nutcracker", nutcracker(1), RED, Vec3::Y * 2.1),
        ] {
            let body = &model.0[look];
            assert!(body.indices.len() > 300, "{name}");
            assert!(outward(body, middle) > 0., "{name} faces in");
            assert!(body.normals.iter().all(|n| Vec3::from(*n).is_finite()), "{name}");
        }
    }
}
