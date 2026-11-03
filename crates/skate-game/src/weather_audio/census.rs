//! What the rain is falling on around the camera, and what is over it: a ring of down rays sorted into the five
//! rain surfaces, per side, plus an up ray and four slanted ones for cover (plan Task 6, map.md §5).
use super::mixer::Surface;
use crate::camera::world_query::line;
use bevy::math::Vec3;
use skate_core::physics::board_world::BoardWorld;
use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_4, TAU};

/// Audio material (`surface & 0x7f`) -> rain surface. From the Task 0 dump of the 95 AudioSurfaceMap entries:
/// grain 5 -> Wood, grain 9 -> Metal, the named soft-natural set -> Foliage, water -> Water; the material
/// category field (1 metal, 3 soft natural, 4 wood) settles the grainless ones. 95-142 (skater/board) are Hard.
const CLASS: [Surface; 143] = {
    use Surface::*;
    let mut t = [Hard; 143];
    t[5] = Wood; // wood_ramp
    t[6] = Wood;
    t[7] = Foliage; // dirt
    t[8] = Metal;
    t[9] = Foliage; // grass
    let mut i = 10;
    while i <= 39 {
        t[i] = Metal; // 30 = metal_sheet; 36 by category (grain 13)
        i += 1;
    }
    while i <= 49 {
        t[i] = Wood; // 40-45 wood category, 46-49 wood grain
        i += 1;
    }
    t[54] = Foliage; // leaves
    t[55] = Foliage; // bush
    t[57] = Wood; // paper (wood grain)
    t[58] = Wood; // cardboard (wood grain)
    t[66] = Metal; // category 1
    t[67] = Metal; // category 1
    t[68] = Metal; // category 1
    t[69] = Foliage; // grass
    t[70] = Foliage; // grass
    t[71] = Foliage; // grass
    t[72] = Metal; // category 1
    t[81] = Metal; // category 1
    t[83] = Metal;
    t[84] = Metal;
    t[88] = Metal;
    t[89] = Wood; // category 4
    t[90] = Metal;
    t[92] = Water; // water
    t[93] = Wood;
    t
};

pub(crate) fn class_of(audio_material: u8) -> Surface {
    CLASS.get(usize::from(audio_material)).copied().unwrap_or(Surface::Hard)
}

const RADII: [f32; 3] = [2., 5., 10.];
/// Hits this far above the camera are a roof, not ground.
const ROOF_ABOVE: f32 = 2.;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Census {
    left: [f32; 5],
    right: [f32; 5],
    pub cover: f32,
    pub roof_metal: bool,
}

/// A face this steep (|normal.y| under it) is a wall: a slanted ray hitting it is not cover.
const WALL: f32 = 0.5;

#[cfg(test)]
thread_local! {
    /// Rays cast on this thread (the budget test).
    pub(crate) static RAYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The hit's audio class, position and face normal's y, or None.
pub(super) fn cast(world: &BoardWorld, start: Vec3, end: Vec3) -> Option<(Surface, Vec3, f32)> {
    #[cfg(test)]
    RAYS.with(|r| r.set(r.get() + 1));
    let hit = line(world, [start.x, start.y, start.z, 0.], [end.x, end.y, end.z, 0.], 0.).ok()?;
    (hit.hit != 0).then(|| {
        let p = Vec3::new(hit.position[0], hit.position[1], hit.position[2]);
        (class_of((hit.surface & 0x7f) as u8), p, hit.normal[1])
    })
}

impl Census {
    /// Parts a census is cast in, one per call (the rays spread over frames): the three rings, then cover.
    pub(crate) const PARTS: usize = 4;

    /// A whole census at once (tests; the game casts it a part at a time, [`Census::sample_part`]).
    #[cfg(test)]
    pub(crate) fn sample(world: &BoardWorld, camera: Vec3, right: Vec3, phase: f32) -> Census {
        let mut c = Census::default();
        (0..Self::PARTS).for_each(|part| c.sample_part(world, camera, right, phase, part));
        c
    }

    /// Parts 0-2: the ring at 2, 5 or 10 m, 8 down rays (the ring sits half a step off `right` and turns by
    /// `phase`) from 30 m above to 30 m below the camera. Part 3: one up ray (40 m) and four at 45° (20 m) for
    /// cover. A census starts from `Census::default()`.
    pub(crate) fn sample_part(&mut self, world: &BoardWorld, camera: Vec3, right: Vec3, phase: f32, part: usize) {
        let right = Vec3::new(right.x, 0., right.z).try_normalize().unwrap_or(Vec3::X);
        let forward = Vec3::Y.cross(right);
        if let Some(&r) = RADII.get(part) {
            for k in 0..8 {
                let a = phase + (k as f32 + 0.5) * FRAC_PI_4;
                let at = camera + (right * a.cos() + forward * a.sin()) * r;
                let mut top = at + Vec3::Y * 30.;
                let bottom = at - Vec3::Y * 30.;
                // A roof over this spot: look again from just under it, so the ground beneath still counts.
                let mut hit = cast(world, top, bottom);
                if let Some((_, p, _)) = hit.filter(|(_, p, _)| p.y > camera.y + ROOF_ABOVE) {
                    top = Vec3::new(at.x, p.y - 0.05, at.z);
                    hit = cast(world, top, bottom).filter(|(_, p, _)| p.y <= camera.y + ROOF_ABOVE);
                }
                if let Some((class, p, _)) = hit {
                    let side = if (p - camera).dot(right) >= 0. { &mut self.right } else { &mut self.left };
                    side[class as usize] += 1. / (1. + r / 3.);
                }
            }
            return;
        }
        let up = cast(world, camera, camera + Vec3::Y * 40.);
        // Walls are not cover: a street of building fronts would otherwise muffle the open rain.
        let slanted = (0..4)
            .filter(|&k| {
                let a = phase + k as f32 * TAU / 4.;
                let dir = (right * a.cos() + forward * a.sin()) * FRAC_1_SQRT_2 + Vec3::Y * FRAC_1_SQRT_2;
                cast(world, camera, camera + dir * 20.).is_some_and(|(_, _, ny)| ny.abs() >= WALL)
            })
            .count();
        self.cover = (0.6 * f32::from(u8::from(up.is_some())) + 0.1 * slanted as f32).clamp(0., 1.);
        self.roof_metal = matches!(up, Some((Surface::Metal, _, _)));
    }

    /// Per surface, [left, right]; each side sums to 1 (no hits: all Hard).
    pub(crate) fn surfaces(&self) -> [[f32; 2]; 5] {
        let norm = |side: [f32; 5]| {
            let sum: f32 = side.iter().sum();
            if sum > 0. { side.map(|w| w / sum) } else { [1., 0., 0., 0., 0.] }
        };
        let (l, r) = (norm(self.left), norm(self.right));
        std::array::from_fn(|i| [l[i], r[i]])
    }
}

/// Something solid within 40 m straight above `at` (the skater's cover: dry wheels under it).
pub(crate) fn covered(world: &BoardWorld, at: Vec3) -> bool {
    cast(world, at + Vec3::Y * 0.5, at + Vec3::Y * 40.).is_some()
}

/// Rain on hard ground makes puddles: `water += hard · (0.35 heavy | 0.15 otherwise)`, then each side renormalised.
pub(crate) fn water_boost(mut surfaces: [[f32; 2]; 5], rain: f32) -> [[f32; 2]; 5] {
    let k = if rain >= 1. { 0.35 } else { 0.15 };
    for side in 0..2 {
        surfaces[Surface::Water as usize][side] += surfaces[Surface::Hard as usize][side] * k;
        let sum: f32 = surfaces.iter().map(|s| s[side]).sum();
        if sum > 0. {
            surfaces.iter_mut().for_each(|s| s[side] /= sum);
        }
    }
    surfaces
}
