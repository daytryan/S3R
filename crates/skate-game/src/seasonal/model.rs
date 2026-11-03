//! Not retail (user 10-19): the seasonal decorations' model kit: a mesh builder, the shapes every piece is made of,
//! the looks (materials) they are drawn in and the textures those use. Not tested in play.

use super::random;
use crate::retail_render::PropMaterial;
use bevy::{
    asset::RenderAssetUsages,
    light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

// ---------------------------------------------------------------------------------------------------------------
// Meshes.

/// Builds a triangle mesh; each triangle is wound to face the way its first vertex's normal points.
#[derive(Default)]
pub(super) struct Builder {
    pub(super) positions: Vec<[f32; 3]>,
    pub(super) normals: Vec<[f32; 3]>,
    pub(super) uvs: Vec<[f32; 2]>,
    pub(super) indices: Vec<u32>,
}

impl Builder {
    pub(super) fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2) -> u32 {
        self.positions.push(p.into());
        self.normals.push(n.normalize_or(Vec3::Y).into());
        self.uvs.push(uv.into());
        self.positions.len() as u32 - 1
    }
    pub(super) fn triangle(&mut self, a: u32, b: u32, c: u32) {
        let p = |i: u32| Vec3::from(self.positions[i as usize]);
        let n = Vec3::from(self.normals[a as usize]) + Vec3::from(self.normals[b as usize]) + Vec3::from(self.normals[c as usize]);
        if (p(b) - p(a)).cross(p(c) - p(a)).dot(n) < 0. {
            self.indices.extend([a, c, b]);
        } else {
            self.indices.extend([a, b, c]);
        }
    }
    pub(super) fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }
    pub(super) fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Shapes. Every one is laid into a Builder already placed (model space: +Z the front, Y up, the ground at 0).

/// A closed surface `f(u, v)`: u once round (0..1, from +X toward +Z), v from the top (0) to the bottom (1). Normals
/// come from finite differences; that orientation makes ∂u × ∂v point outward.
pub(super) fn surface(b: &mut Builder, segments: usize, rings: usize, f: impl Fn(f32, f32) -> Vec3) {
    const E: f32 = 1e-3;
    let first = b.positions.len() as u32;
    for j in 0..=rings {
        let v = j as f32 / rings as f32;
        let w = v.clamp(4. * E, 1. - 4. * E);
        for i in 0..=segments {
            let u = i as f32 / segments as f32;
            let n = (f(u + E, w) - f(u - E, w)).cross(f(u, w + E) - f(u, w - E));
            b.vertex(f(u, v), n, Vec2::new(u, v));
        }
    }
    let at = |j: usize, i: usize| first + (j * (segments + 1) + i) as u32;
    for j in 0..rings {
        for i in 0..segments {
            b.quad(at(j, i), at(j + 1, i), at(j + 1, i + 1), at(j, i + 1));
        }
    }
}

pub(super) fn spow(x: f32, e: f32) -> f32 {
    x.signum() * x.abs().powf(e)
}

/// A puffy box (a superellipsoid) of half-sizes `r` about `at`: squareness `e` up and down and `round` about the
/// middle (1 an ellipse, toward 0 a square).
pub(super) fn pillow(at: Vec3, r: Vec3, e: f32, round: f32) -> impl Fn(f32, f32) -> Vec3 {
    move |u, v| {
        let (se, ce) = (FRAC_PI_2 - PI * v).sin_cos();
        let (so, co) = (TAU * u).sin_cos();
        let ring = spow(ce, e);
        at + Vec3::new(r.x * ring * spow(co, round), r.y * spow(se, e), r.z * ring * spow(so, round))
    }
}

pub(super) fn ball(at: Vec3, r: Vec3) -> impl Fn(f32, f32) -> Vec3 {
    pillow(at, r, 1., 1.)
}

/// A round tube through `path` (at least two points), `r(t)` thick at t (0..1) along it, its ends domed.
pub(super) fn tube(b: &mut Builder, path: &[Vec3], r: impl Fn(f32) -> f32, sides: usize) {
    let n = path.len();
    let tangent = |i: usize| (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec3::Y);
    // (centre, across, radius, along, tilt toward along): a frame carried down the path without twisting.
    let mut rings: Vec<(Vec3, Vec3, Vec3, f32, Vec3, f32)> = Vec::new();
    let mut side = tangent(0).any_orthonormal_vector();
    for i in 0..n {
        let t = tangent(i);
        side = (side - t * side.dot(t)).normalize_or(t.any_orthonormal_vector());
        rings.push((path[i], side, t.cross(side), r(i as f32 / (n - 1) as f32), t, 0.));
    }
    let dome = |end: (Vec3, Vec3, Vec3, f32, Vec3, f32), out: Vec3| {
        (1..=3).map(move |k| {
            let a = k as f32 * PI / 8.;
            (end.0 + out * end.3 * a.sin(), end.1, end.2, end.3 * a.cos(), out, a)
        })
    };
    let (start, end) = (rings[0], rings[n - 1]);
    let mut all: Vec<_> = dome(start, -start.4).collect();
    all.reverse();
    all.extend(rings);
    all.extend(dome(end, end.4));
    let first = b.positions.len() as u32;
    for &(centre, across, up, radius, along, tilt) in &all {
        for k in 0..=sides {
            let (s, c) = (TAU * k as f32 / sides as f32).sin_cos();
            let radial = across * c + up * s;
            b.vertex(centre + radial * radius, radial * tilt.cos() + along * tilt.sin(), Vec2::new(k as f32 / sides as f32, 0.));
        }
    }
    let at = |j: usize, k: usize| first + (j * (sides + 1) + k) as u32;
    for j in 0..all.len() - 1 {
        for k in 0..sides {
            b.quad(at(j, k), at(j + 1, k), at(j + 1, k + 1), at(j, k + 1));
        }
    }
}

/// `n + 1` points along `f` from t = 0 to 1.
pub(super) fn curve(n: usize, f: impl Fn(f32) -> Vec3) -> Vec<Vec3> {
    (0..=n).map(|i| f(i as f32 / n as f32)).collect()
}

/// The quadratic Bézier from `a` toward `c` to `b`.
pub(super) fn bend(a: Vec3, c: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.lerp(c, t).lerp(c.lerp(b, t), t)
}

/// A slab: the convex `outline` (x, y, anticlockwise from the front) `depth` thick along Z, placed by `at`. A
/// `carved` front maps the outline's box onto the texture; every other face takes its plain corner.
pub(super) fn slab(b: &mut Builder, outline: &[Vec2], depth: f32, at: Transform, carved: bool) {
    let (lo, hi) = outline.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    let plain = Vec2::splat(0.02);
    let face = |p: Vec2, side: f32| {
        if carved && side > 0. { Vec2::new((p.x - lo.x) / (hi.x - lo.x), 1. - (p.y - lo.y) / (hi.y - lo.y)) } else { plain }
    };
    let point = |p: Vec2, z: f32| at.transform_point(Vec3::new(p.x, p.y, z));
    let middle = outline.iter().copied().sum::<Vec2>() / outline.len() as f32;
    for side in [1., -1.] {
        let n = at.rotation * (Vec3::Z * side);
        let c = b.vertex(point(middle, depth * 0.5 * side), n, face(middle, side));
        let rim: Vec<u32> = outline.iter().map(|&p| b.vertex(point(p, depth * 0.5 * side), n, face(p, side))).collect();
        for k in 0..rim.len() {
            b.triangle(c, rim[k], rim[(k + 1) % rim.len()]);
        }
    }
    for k in 0..outline.len() {
        let (p, q) = (outline[k], outline[(k + 1) % outline.len()]);
        let e = (q - p).normalize_or_zero();
        let n = at.rotation * Vec3::new(e.y, -e.x, 0.);
        let v: Vec<u32> = [(p, 0.5), (q, 0.5), (q, -0.5), (p, -0.5)]
            .iter()
            .map(|&(o, z)| b.vertex(point(o, depth * z), n, plain))
            .collect();
        b.quad(v[0], v[1], v[2], v[3]);
    }
}

/// An arch: a rectangle `w` across and `h` tall with a round top, from its foot's middle.
pub(super) fn arch(w: f32, h: f32) -> Vec<Vec2> {
    let r = w * 0.5;
    let mut out = vec![Vec2::new(-r, 0.), Vec2::new(r, 0.)];
    out.extend((0..=10).map(|k| Vec2::new(0., h - r) + Vec2::from_angle(PI * k as f32 / 10.) * r));
    out
}

/// A flat shape on a front face at depth `z`, facing +Z (no thickness).
pub(super) fn decal(b: &mut Builder, outline: &[Vec2], at: Vec3, scale: Vec2) {
    let middle = outline.iter().copied().sum::<Vec2>() / outline.len() as f32;
    let p = |q: Vec2| at + (q * scale).extend(0.);
    let c = b.vertex(p(middle), Vec3::Z, Vec2::splat(0.5));
    let rim: Vec<u32> = outline.iter().map(|&q| b.vertex(p(q), Vec3::Z, Vec2::splat(0.5))).collect();
    for k in 0..rim.len() {
        b.triangle(c, rim[k], rim[(k + 1) % rim.len()]);
    }
}

pub(super) fn rect(w: f32, h: f32) -> Vec<Vec2> {
    vec![Vec2::new(-w * 0.5, -h * 0.5), Vec2::new(w * 0.5, -h * 0.5), Vec2::new(w * 0.5, h * 0.5), Vec2::new(-w * 0.5, h * 0.5)]
}

// ---------------------------------------------------------------------------------------------------------------
// Looks: what every part is made of.

/// A part's surface: lit (sRGB colour, roughness, the share of its colour the lamp inside adds), metal (sRGB colour,
/// roughness), see-through (sRGB colour, alpha) or an unlit glow (linear HDR colour).
#[derive(Clone, Copy)]
pub(super) enum Look {
    Lit([f32; 3], f32, f32),
    Metal([f32; 3], f32),
    Clear([f32; 3], f32),
    Glow([f32; 3]),
}

pub(super) const BLACK: usize = 0;
pub(super) const PURPLE: usize = 1;
pub(super) const GREEN: usize = 2;
pub(super) const ORANGE: usize = 3;
pub(super) const WHITE: usize = 4;
pub(super) const BROWN: usize = 5;
pub(super) const STRAW: usize = 6;
pub(super) const STONE: usize = 7;
pub(super) const DIRT: usize = 8;
pub(super) const BONE: usize = 9;
pub(super) const GOLD: usize = 10;
pub(super) const SKIN: usize = 11;
pub(super) const WING: usize = 12;
pub(super) const WINDOW: usize = 13;
pub(super) const RED_EYE: usize = 14;
pub(super) const BULB_ORANGE: usize = 15;
pub(super) const BULB_PURPLE: usize = 16;
pub(super) const BULB_GREEN: usize = 17;
pub(super) const GHOST: usize = 18;
pub(super) const RED: usize = 19;
pub(super) const PINE: usize = 20;
pub(super) const NEEDLE: usize = 21;
pub(super) const SNOW: usize = 22;
pub(super) const COAL: usize = 23;
pub(super) const CARROT: usize = 24;
pub(super) const GINGER: usize = 25;
pub(super) const ICING: usize = 26;
pub(super) const WRAP_RED: usize = 27;
pub(super) const WRAP_GREEN: usize = 28;
pub(super) const WRAP_BLUE: usize = 29;
pub(super) const WRAP_WHITE: usize = 30;
pub(super) const RIBBON: usize = 31;
pub(super) const SILVER: usize = 32;
pub(super) const BAUBLE_RED: usize = 33;
pub(super) const BAUBLE_GOLD: usize = 34;
pub(super) const GLASS: usize = 35;
pub(super) const CORD: usize = 36;
pub(super) const CAULDRON: usize = 37;
pub(super) const BULB_WARM: usize = 38;
pub(super) const BULB_ICE: usize = 39;
pub(super) const STAR: usize = 40;
/// The first of five twinkling Christmas bulb colours (`MULTI..MULTI + 5`), stepped through seasonal.rs's PALETTE.
pub(super) const MULTI: usize = 41;
pub(super) const FLAME: usize = 46;

pub(super) const LOOKS: [Look; 47] = [
    Look::Lit([0.05, 0.04, 0.06], 0.45, 0.5),
    Look::Lit([0.4, 0.1, 0.62], 0.4, 0.45),
    Look::Lit([0.3, 0.68, 0.12], 0.4, 0.45),
    Look::Lit([1.0, 0.42, 0.02], 0.4, 0.5),
    Look::Lit([0.93, 0.94, 0.96], 0.45, 0.4),
    Look::Lit([0.3, 0.17, 0.08], 0.75, 0.1),
    Look::Lit([1., 1., 1.], 0.9, 0.),
    Look::Lit([1., 1., 1.], 0.95, 0.),
    Look::Lit([0.17, 0.11, 0.07], 1., 0.),
    Look::Lit([0.86, 0.83, 0.72], 0.6, 0.),
    Look::Lit([0.95, 0.72, 0.25], 0.3, 0.2),
    Look::Lit([0.42, 0.8, 0.25], 0.5, 0.35),
    Look::Lit([0.06, 0.05, 0.07], 0.6, 0.),
    Look::Glow([4., 2.1, 0.35]),
    Look::Glow([7., 0.25, 0.08]),
    Look::Glow([5., 1.6, 0.1]),
    Look::Glow([2.4, 0.35, 5.]),
    Look::Glow([0.9, 4.5, 0.35]),
    Look::Lit([0.86, 0.92, 1.0], 0.5, 0.0),
    // RED: Santa red, painted wood, nutcracker coats.
    Look::Lit([0.72, 0.04, 0.05], 0.45, 0.),
    // PINE: garland green.
    Look::Lit([0.07, 0.3, 0.12], 0.8, 0.),
    // NEEDLE: a deep blue-green fir.
    Look::Lit([0.03, 0.2, 0.16], 0.85, 0.),
    // SNOW: snowmen and snow on roofs.
    Look::Lit([0.95, 0.97, 1.0], 0.7, 0.),
    Look::Lit([0.04, 0.04, 0.045], 0.6, 0.),
    Look::Lit([1.0, 0.45, 0.08], 0.5, 0.),
    Look::Lit([0.62, 0.34, 0.14], 0.75, 0.),
    Look::Lit([1.0, 0.98, 0.95], 0.5, 0.),
    Look::Lit([0.75, 0.05, 0.06], 0.35, 0.),
    Look::Lit([0.05, 0.42, 0.14], 0.35, 0.),
    Look::Lit([0.08, 0.2, 0.62], 0.35, 0.),
    Look::Lit([0.95, 0.95, 0.92], 0.35, 0.),
    // RIBBON: gold ribbon and bows.
    Look::Metal([0.95, 0.75, 0.3], 0.3),
    Look::Metal([0.9, 0.92, 0.95], 0.2),
    Look::Metal([0.8, 0.06, 0.08], 0.15),
    Look::Metal([0.95, 0.72, 0.25], 0.15),
    // GLASS: snow globes.
    Look::Clear([0.85, 0.92, 1.0], 0.22),
    // CORD: giant webs, off-white and lit from within so they read at night.
    Look::Lit([0.9, 0.9, 0.86], 0.7, 0.6),
    // CAULDRON: a witch's brew, glowing green.
    Look::Glow([0.6, 3.4, 0.35]),
    Look::Glow([5., 3.2, 1.2]),
    Look::Glow([2.2, 3.6, 6.]),
    Look::Glow([7., 5.6, 1.6]),
    Look::Glow([4., 0.24, 0.16]),
    Look::Glow([0.32, 3.6, 0.48]),
    Look::Glow([0.4, 1., 4.]),
    Look::Glow([4., 2.2, 0.2]),
    Look::Glow([4., 3.4, 2.2]),
    Look::Glow([6., 2.8, 0.5]),
];

/// A model in parts, one Builder per look.
pub(super) struct Model(pub Vec<Builder>);

impl Model {
    pub(super) fn new() -> Self {
        Self((0..LOOKS.len()).map(|_| Builder::default()).collect())
    }
    pub(super) fn part(&mut self, look: usize) -> &mut Builder {
        &mut self.0[look]
    }
    /// Lays `other` into this model, placed by `at`, each of its parts into the same part here.
    pub(super) fn append(&mut self, other: Model, at: Transform) {
        for (to, from) in self.0.iter_mut().zip(other.0) {
            let first = to.positions.len() as u32;
            to.positions.extend(from.positions.iter().map(|&p| at.transform_point(p.into()).to_array()));
            to.normals.extend(from.normals.iter().map(|&n| (at.rotation * Vec3::from(n)).to_array()));
            to.uvs.extend(from.uvs);
            to.indices.extend(from.indices.iter().map(|&i| first + i));
        }
    }
}

/// How much a part's triangles face away from `middle`: their areas, signed by which way each faces. Positive when
/// the winding and the finite-difference normals agree that the part faces out.
#[cfg(test)]
pub(super) fn outward(b: &Builder, middle: Vec3) -> f32 {
    b.indices
        .chunks(3)
        .map(|t| {
            let p = |i: u32| Vec3::from(b.positions[i as usize]);
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            n.dot((p(t[0]) + p(t[1]) + p(t[2])) / 3. - middle).signum() * n.length()
        })
        .sum()
}

// ---------------------------------------------------------------------------------------------------------------
// Materials.

/// The share of a nylon part's colour its inner lamp adds (emissive, unaffected by exposure like the unlit glows).
const INNER_GLOW: f32 = 0.35;

pub(super) enum Finish {
    Lit(Handle<PropMaterial>),
    Glow(Handle<StandardMaterial>),
}

pub(super) fn materials(unlit: &mut Assets<StandardMaterial>, lit: &mut Assets<PropMaterial>, images: &mut Assets<Image>) -> Vec<Finish> {
    let (straw, stone) = (images.add(straw_texture()), images.add(stone_texture()));
    LOOKS
        .iter()
        .enumerate()
        .map(|(k, look)| match *look {
            Look::Glow(c) => Finish::Glow(unlit.add(StandardMaterial { base_color: Color::linear_rgb(c[0], c[1], c[2]), unlit: true, ..default() })),
            Look::Lit(srgb, roughness, inner) => {
                let colour = Color::srgb(srgb[0], srgb[1], srgb[2]);
                let mut base = StandardMaterial {
                    base_color: colour,
                    base_color_texture: match k {
                        STRAW => Some(straw.clone()),
                        STONE => Some(stone.clone()),
                        _ => None,
                    },
                    perceptual_roughness: roughness,
                    emissive: colour.to_linear() * (inner * INNER_GLOW),
                    ..default()
                };
                if k == WING {
                    // Bat wings are one sheet.
                    (base.double_sided, base.cull_mode) = (true, None);
                }
                Finish::Lit(lit.add(PropMaterial { base, extension: default() }))
            }
            Look::Metal(srgb, roughness) => Finish::Lit(lit_material(lit, srgb, roughness, 1.)),
            Look::Clear(srgb, alpha) => {
                let base = StandardMaterial {
                    base_color: Color::srgba(srgb[0], srgb[1], srgb[2], alpha),
                    perceptual_roughness: 0.05,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                };
                Finish::Lit(lit.add(PropMaterial { base, extension: default() }))
            }
        })
        .collect()
}

/// A model's meshes, one per look it uses.
pub(super) fn upload(meshes: &mut Assets<Mesh>, model: Model) -> Vec<(usize, Handle<Mesh>)> {
    model.0.into_iter().enumerate().filter(|(_, b)| !b.indices.is_empty()).map(|(k, b)| (k, meshes.add(b.mesh()))).collect()
}

pub(super) fn spawn_parts(c: &mut ChildSpawnerCommands, parts: &[(usize, Handle<Mesh>)], materials: &[Finish]) {
    for (look, mesh) in parts {
        match &materials[*look] {
            Finish::Lit(m) => {
                c.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(m.clone())));
            }
            Finish::Glow(m) => {
                c.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(m.clone()), NotShadowCaster));
            }
        }
    }
}

/// An unlit, glowing material of `colour` times `strength` (HDR; unlit ignores emissive).
pub(super) fn glow(colour: [f32; 3], strength: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::linear_rgb(colour[0] * strength, colour[1] * strength, colour[2] * strength),
        unlit: true,
        ..default()
    }
}

/// Lit like the world's props (PropMaterial: its snow, fog and time-of-day handling).
pub(super) fn lit_material(materials: &mut Assets<PropMaterial>, srgb: [f32; 3], roughness: f32, metallic: f32) -> Handle<PropMaterial> {
    materials.add(PropMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(srgb[0], srgb[1], srgb[2]),
            perceptual_roughness: roughness,
            metallic,
            ..default()
        },
        extension: default(),
    })
}

/// A soft round glow for the ground (white, alpha falling off from the middle), drawn additively.
pub(super) fn pool_texture() -> Image {
    const SIZE: u32 = 64;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / SIZE as f32 * 2. - 1.;
            let a = (1. - d.length()).clamp(0., 1.);
            rgba.extend([255, 255, 255, (a * a * 255.) as u8]);
        }
    }
    crate::custom_graphic::texture(rgba, (SIZE, SIZE))
}

/// A flat glow on the ground, `size` across, of an additive material.
pub(super) fn pool(meshes: &mut Assets<Mesh>, size: f32) -> Handle<Mesh> {
    meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(size * 0.5)).mesh())
}

pub(super) fn pool_material(unlit: &mut Assets<StandardMaterial>, texture: Handle<Image>) -> Handle<StandardMaterial> {
    unlit.add(StandardMaterial {
        base_color_texture: Some(texture),
        unlit: true,
        alpha_mode: AlphaMode::Add,
        ..default()
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Textures.

/// Straw: short strands of light and dark gold on gold, mostly running one way.
pub(super) fn straw_texture() -> Image {
    use tiny_skia::{LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
    const SIZE: u32 = 256;
    let mut canvas = Pixmap::new(SIZE, SIZE).expect("straw canvas");
    canvas.fill(tiny_skia::Color::from_rgba8(190, 145, 62, 255));
    let mut paint = Paint::default();
    paint.anti_alias = true;
    for k in 0..2600u32 {
        let (x, y) = (random(k * 3 + 1) * SIZE as f32, random(k * 3 + 2) * SIZE as f32);
        let length = 6. + 22. * random(k * 5 + 3);
        let a = (random(k * 7 + 4) - 0.5) * 0.7;
        let shade = random(k * 11 + 5);
        let mix = |dark: f32, light: f32| (dark + (light - dark) * shade) as u8;
        paint.set_color_rgba8(mix(120., 245.), mix(82., 212.), mix(30., 130.), 210);
        let mut path = PathBuilder::new();
        path.move_to(x, y);
        path.line_to(x + a.cos() * length, y + a.sin() * length);
        if let Some(path) = path.finish() {
            let stroke = Stroke { width: 0.9 + 1.3 * random(k * 13 + 6), line_cap: LineCap::Round, ..Default::default() };
            canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    let rgba = canvas.pixels().iter().flat_map(|p| [p.red(), p.green(), p.blue(), 255]).collect();
    crate::custom_graphic::texture(rgba, (SIZE, SIZE))
}

/// Weathered grey stone, mottled and patched with lichen, with RIP cut into its upper middle and a few worn lines
/// of epitaph below.
pub(super) fn stone_texture() -> Image {
    use tiny_skia::{LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
    const SIZE: u32 = 256;
    let noise = |x: f32, y: f32, cell: f32, salt: u32| {
        let (gx, gy) = (x / cell, y / cell);
        let (ix, iy) = (gx.floor() as i32, gy.floor() as i32);
        let (fx, fy) = (gx - ix as f32, gy - iy as f32);
        let h = |i: i32, j: i32| random((i as u32).wrapping_mul(73_856_093) ^ (j as u32).wrapping_mul(19_349_663) ^ salt);
        let (sx, sy) = (fx * fx * (3. - 2. * fx), fy * fy * (3. - 2. * fy));
        let top = h(ix, iy) + (h(ix + 1, iy) - h(ix, iy)) * sx;
        let bottom = h(ix, iy + 1) + (h(ix + 1, iy + 1) - h(ix, iy + 1)) * sx;
        top + (bottom - top) * sy
    };
    let mut canvas = Pixmap::new(SIZE, SIZE).expect("stone canvas");
    for (i, px) in canvas.data_mut().chunks_exact_mut(4).enumerate() {
        let (x, y) = ((i as u32 % SIZE) as f32, (i as u32 / SIZE) as f32);
        let grey = 0.36 + 0.14 * noise(x, y, 40., 1) + 0.08 * noise(x, y, 11., 2) + 0.05 * noise(x, y, 3., 3);
        let lichen = ((noise(x, y, 25., 4) - 0.68) * 5.).clamp(0., 1.) * 0.6;
        let rgb = [grey * (1. - 0.25 * lichen), grey * (1. + 0.15 * lichen), grey * (1. - 0.4 * lichen) + 0.02];
        px.copy_from_slice(&[(rgb[0] * 255.) as u8, (rgb[1] * 255.) as u8, (rgb[2] * 255.) as u8, 255]);
    }
    let s = SIZE as f32;
    let mut paint = Paint::default();
    paint.anti_alias = true;
    let mut cut = |strokes: &[&[(f32, f32)]], width: f32, shade: u8| {
        paint.set_color_rgba8(shade, shade, shade + 4, 255);
        for line in strokes {
            let mut path = PathBuilder::new();
            path.move_to(line[0].0 * s, line[0].1 * s);
            for p in &line[1..] {
                path.line_to(p.0 * s, p.1 * s);
            }
            if let Some(path) = path.finish() {
                let stroke = Stroke { width, line_cap: LineCap::Round, ..Default::default() };
                canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    };
    let bowl = |x: f32| -> Vec<(f32, f32)> {
        let mut b = vec![(x, 0.22), (x + 0.06, 0.22)];
        b.extend((0..=8).map(|k| {
            let a = -FRAC_PI_2 + PI * k as f32 / 8.;
            (x + 0.06 + 0.055 * a.cos(), 0.2775 + 0.0575 * a.sin())
        }));
        b.push((x, 0.335));
        b
    };
    let (r_bowl, p_bowl) = (bowl(0.27), bowl(0.6));
    let letters: [&[(f32, f32)]; 6] =
        [&[(0.27, 0.22), (0.27, 0.45)], &r_bowl, &[(0.32, 0.335), (0.4, 0.45)], &[(0.5, 0.22), (0.5, 0.45)], &[(0.6, 0.22), (0.6, 0.45)], &p_bowl];
    // A dark cut with a lighter lip below it.
    cut(&letters, 11., 58);
    cut(&letters, 6., 34);
    for (k, y) in [0.56f32, 0.63, 0.7].into_iter().enumerate() {
        let words: Vec<[(f32, f32); 2]> = (0..4)
            .filter(|w| random(k as u32 * 7 + w) < 0.8)
            .map(|w| {
                let x = 0.26 + 0.12 * w as f32;
                [(x, y), (x + 0.08 + 0.03 * random(k as u32 * 11 + w), y)]
            })
            .collect();
        let refs: Vec<&[(f32, f32)]> = words.iter().map(|w| &w[..]).collect();
        cut(&refs, 3.5, 70);
    }
    let rgba = canvas.pixels().iter().flat_map(|p| [p.red(), p.green(), p.blue(), 255]).collect();
    crate::custom_graphic::texture(rgba, (SIZE, SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appending_a_model_moves_its_parts() {
        let mut piece = Model::new();
        surface(piece.part(RED), 8, 6, ball(Vec3::ZERO, Vec3::splat(0.5)));
        let count = piece.0[RED].positions.len();
        let mut zone = Model::new();
        zone.append(piece, Transform::from_xyz(10., 0., 0.).with_rotation(Quat::from_rotation_y(FRAC_PI_2)));
        assert_eq!(zone.0[RED].positions.len(), count);
        assert!(zone.0[RED].positions.iter().all(|p| (Vec3::from(*p) - Vec3::X * 10.).length() < 0.51));
        assert!(zone.0[RED].indices.iter().all(|&i| (i as usize) < count));
    }
}
