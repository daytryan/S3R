//! Not retail (user 10-19): the season's layout snapped to the park's real collision. Each entry finds its surface
//! below its authored point or is dropped (and logged); repeated items are spaced evenly along their paths. Only the
//! map and the tables decide it, so every player on a map sees the same park. Not tested in play.

use super::layout::{Kind, Layout, Palette, Snap, Style, Zone};
use super::{Pumpkin, carving, random, ring, seed_at};
use bevy::prelude::*;
use skate_core::{math::Vector3, physics::board_world::BoardWorld};

/// How far below its authored point an entry looks for its surface (the search starts 1 m above it).
pub(super) const DROP: f32 = 6.;
/// How near a surface a hung entry, a strand's support or a web's anchor must be.
pub(super) const TOUCH: f32 = 0.6;

/// A piece where it landed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placed {
    pub zone: Zone,
    pub kind: Kind,
    pub at: Vec3,
    pub yaw: f32,
    pub scale: f32,
    pub seed: u32,
}

/// A strand of lights along `path`.
#[derive(Clone, Debug)]
pub(crate) struct Strand {
    pub zone: Zone,
    pub path: Vec<Vec3>,
    pub style: Style,
    pub palette: Palette,
    pub garland: bool,
}

/// A giant web strung between `frame`'s anchors.
#[derive(Clone, Debug)]
pub(crate) struct Web {
    pub zone: Zone,
    pub frame: Vec<Vec3>,
    pub spider: bool,
    pub seed: u32,
    /// With its spider, the little ones dropping on threads from its lowest edge: (where each is tied, its length).
    pub danglers: Vec<(Vec3, f32)>,
}

/// The season's park as it lands.
#[derive(Default, Debug)]
pub(crate) struct Plan {
    pub pieces: Vec<Placed>,
    pub pumpkins: Vec<Pumpkin>,
    pub strands: Vec<Strand>,
    pub webs: Vec<Web>,
    pub flocks: Vec<Vec3>,
    pub roosts: Vec<Vec3>,
    /// The glows on the ground as laid ([`glows`]): where each lies, its colour.
    pub glows: Vec<(Transform, [f32; 3])>,
    /// What did not land, and why.
    pub dropped: Vec<String>,
}

pub(crate) fn resolve(world: &BoardWorld, layout: &Layout) -> Plan {
    let probe = Probe(world);
    let mut plan = Plan::default();
    let name = |zone: Zone, what: &str, at: Vec3| format!("{zone:?} {what} at ({:.1}, {:.1}, {:.1})", at.x, at.y, at.z);
    for (i, piece) in layout.pieces.iter().enumerate() {
        let at = Vec3::from(piece.at);
        match land(&probe, piece.kind, at, piece.snap, piece.scale) {
            Ok(at) => add(&mut plan, Placed { zone: piece.zone, kind: piece.kind, at, yaw: piece.yaw, scale: piece.scale, seed: seed_at(at, i as u32) }),
            Err(why) => plan.dropped.push(format!("{}: {why}", name(piece.zone, &format!("{:?}", piece.kind), at))),
        }
    }
    for (i, row) in layout.rows.iter().enumerate() {
        let points: Vec<Vec3> = row.points.iter().map(|&p| Vec3::from(p)).collect();
        for (k, (at, ahead)) in along(&points, row.spacing).into_iter().enumerate() {
            // Facing the path's right-hand side.
            let right = ahead.cross(Vec3::Y);
            let yaw = right.x.atan2(right.z);
            match land(&probe, row.kind, at, row.snap, row.scale) {
                Ok(at) => {
                    let seed = seed_at(at, (1000 * (i + 1) + k) as u32);
                    add(&mut plan, Placed { zone: row.zone, kind: row.kind, at, yaw, scale: row.scale, seed });
                }
                Err(why) => plan.dropped.push(format!("{}: {why}", name(row.zone, &format!("{:?}", row.kind), at))),
            }
        }
    }
    for route in layout.routes {
        let path: Vec<Vec3> = route.points.iter().map(|&p| Vec3::from(p)).collect();
        // A wound strand is held by its turns against the tower; any other by its points.
        let supports = match route.style {
            Style::Spiral { .. } => super::strung::wire(&path, route.style).into_iter().step_by(20).collect(),
            _ => path.clone(),
        };
        if supports.iter().filter(|&&p| probe.touches(p)).count() >= 2 {
            plan.strands.push(Strand { zone: route.zone, path, style: route.style, palette: route.palette, garland: route.garland });
        } else {
            plan.dropped.push(format!("{}: touches nothing", name(route.zone, "strand", path[0])));
        }
    }
    for (i, span) in layout.webs.iter().enumerate() {
        let frame: Vec<Vec3> = span.frame.iter().map(|&p| Vec3::from(p)).collect();
        if frame.iter().filter(|&&p| probe.touches(p)).count() >= 2 {
            let seed = seed_at(frame[0], i as u32);
            let danglers = if span.spider && frame.len() >= 3 { danglers(&probe, &frame, seed) } else { Vec::new() };
            plan.webs.push(Web { zone: span.zone, seed, frame, spider: span.spider, danglers });
        } else {
            plan.dropped.push(format!("{}: touches nothing", name(span.zone, "web", frame[0])));
        }
    }
    plan.flocks = layout.flocks.iter().map(|&p| Vec3::from(p)).collect();
    for &p in layout.roosts {
        let p = Vec3::from(p);
        if probe.touches(p) {
            plan.roosts.push(p);
        } else {
            plan.dropped.push(format!("roost bat at ({:.1}, {:.1}, {:.1}): touches nothing", p.x, p.y, p.z));
        }
    }
    for p in &mut plan.pumpkins {
        p.spill = p.carving.is_some() && lies_flat(&probe, p.pool(), p.at.y);
    }
    plan
}

/// The pool's middle and its four edges (most of the way out) each have a surface within 30 cm of `y` under them.
fn lies_flat(probe: &Probe, pool: Transform, y: f32) -> bool {
    [Vec3::ZERO, Vec3::new(0., 0., 0.35), Vec3::new(0., 0., -0.35), Vec3::new(-0.35, 0., 0.), Vec3::new(0.35, 0., 0.)]
        .iter()
        .all(|&o| probe.below(pool.transform_point(o)).is_some_and(|(hit, _)| (hit.y - y).abs() < 0.3))
}

/// Three little spiders on threads from the web's lowest edge: (where each is tied, its length). Not retail (user
/// 10-20): a thread stops short of what is under it, so at the bottom of its bob the spider still hangs 30 cm over it;
/// one left with under 30 cm of thread is left out (they hung into the floor).
fn danglers(probe: &Probe, frame: &[Vec3], seed: u32) -> Vec<(Vec3, f32)> {
    let n = frame.len();
    let edge = (0..n).min_by(|&a, &b| (frame[a].y + frame[(a + 1) % n].y).total_cmp(&(frame[b].y + frame[(b + 1) % n].y))).unwrap_or(0);
    let (a, b) = (frame[edge], frame[(edge + 1) % n]);
    [0.25f32, 0.5, 0.75]
        .into_iter()
        .enumerate()
        .filter_map(|(j, t)| {
            let top = a.lerp(b, t) - Vec3::Y * (4. * super::strung::SAG * a.distance(b) * t * (1. - t));
            let drop = 0.8 + 1.2 * random(seed ^ (j as u32 * 13 + 5));
            let bob = super::halloween::BOB;
            let room = probe.hit(top, top - Vec3::Y * (drop + bob + 0.3)).map_or(drop, |(under, _)| top.y - under.y - 0.3 - bob);
            let length = drop.min(room);
            (length >= 0.3).then_some((top, length))
        })
        .collect()
}

/// Each piece's glows on the ground ([`super::ground_glows`]) where they lie flat: as authored, else cut to three
/// quarters or half across, else left out. Not retail (user 10-20): a pool or a mist patch past an edge hung in the air.
pub(crate) fn glows(world: &BoardWorld, plan: &Plan, season: super::Season) -> Vec<(Transform, [f32; 3])> {
    let probe = Probe(world);
    let mut out = Vec::new();
    for piece in &plan.pieces {
        for (at, colour) in super::ground_glows(piece, season) {
            let laid = [1., 0.75, 0.5].into_iter().map(|f| at.with_scale(at.scale * Vec3::new(f, 1., f))).find(|&t| lies_flat(&probe, t, piece.at.y));
            out.extend(laid.map(|t| (t, colour)));
        }
    }
    out
}

/// Where an entry of `kind` authored at `at` lands by `snap`, or why it does not.
fn land(probe: &Probe, kind: Kind, at: Vec3, snap: Snap, scale: f32) -> Result<Vec3, &'static str> {
    if super::outside(at.x, at.z) > super::MARGIN {
        return Err("outside the park");
    }
    match snap {
        Snap::Ground => {
            let (p, n) = probe.below(at).ok_or("nothing below")?;
            if n.y < 0.97 {
                return Err("no level ground");
            }
            let (radius, height) = kind.room();
            if probe.clear(p, radius * scale, height * scale) { Ok(p) } else { Err("no room") }
        }
        Snap::Top => probe.below(at).map(|(p, _)| p).ok_or("nothing below"),
        Snap::Hung => {
            if probe.touches(at) {
                Ok(at)
            } else {
                Err("touches nothing")
            }
        }
    }
}

/// A placed piece, and the pumpkins a hay stack carries; a pumpkin is only its pumpkin.
fn add(plan: &mut Plan, piece: Placed) {
    if piece.kind == Kind::Pumpkin {
        let s = piece.seed;
        let carving = (random(s) < 0.75).then(|| carving(s ^ 3));
        plan.pumpkins.push(Pumpkin { at: piece.at, yaw: piece.yaw + 0.5 * (random(s ^ 5) - 0.5), size: 0.3 * piece.scale, light: false, carving, spill: false });
        return;
    }
    if piece.kind == Kind::PumpkinTower {
        let turn = Quat::from_rotation_y(piece.yaw);
        for (k, (rest, size, lit)) in super::halloween::pumpkin_tower(piece.seed).into_iter().enumerate() {
            let s = piece.seed ^ (k as u32 * 131);
            plan.pumpkins.push(Pumpkin {
                at: piece.at + turn * (rest * piece.scale),
                yaw: piece.yaw + 0.6 * (random(s ^ 5) - 0.5),
                size: size * piece.scale,
                light: lit,
                carving: Some(carving(s ^ 3)),
                spill: false,
            });
        }
        return;
    }
    if piece.kind == Kind::Hay {
        let turn = Quat::from_rotation_y(piece.yaw);
        for (k, (rest, size, lit)) in super::halloween::hay_stack(piece.seed).1.into_iter().enumerate() {
            let s = piece.seed ^ (k as u32 * 131);
            let face = if lit || random(s) < 0.5 { Some(carving(s ^ 3)) } else { None };
            plan.pumpkins.push(Pumpkin {
                at: piece.at + turn * (rest * piece.scale),
                yaw: piece.yaw + 0.6 * (random(s ^ 5) - 0.5),
                size: size * piece.scale,
                light: lit,
                carving: face,
                spill: false,
            });
        }
    }
    plan.pieces.push(piece);
}

/// Points evenly spaced along the path `points` (with its unit direction there): the first at its start, the last at
/// its end, as many gaps as bring them nearest `spacing` apart.
pub(crate) fn along(points: &[Vec3], spacing: f32) -> Vec<(Vec3, Vec3)> {
    let total: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    if points.len() < 2 || total <= 0. {
        return points.first().map(|&p| vec![(p, Vec3::Z)]).unwrap_or_default();
    }
    let gaps = (total / spacing).round().max(1.) as usize;
    let (mut seg, mut start) = (0, 0.);
    (0..=gaps)
        .map(|k| {
            let s = total * k as f32 / gaps as f32;
            while seg + 2 < points.len() && start + points[seg].distance(points[seg + 1]) < s {
                start += points[seg].distance(points[seg + 1]);
                seg += 1;
            }
            let (a, b) = (points[seg], points[seg + 1]);
            let d = a.distance(b);
            (a.lerp(b, if d > 0. { ((s - start) / d).min(1.) } else { 0. }), (b - a).normalize_or(Vec3::Z))
        })
        .collect()
}

pub(super) struct Probe<'a>(pub(super) &'a BoardWorld);

impl Probe<'_> {
    /// The first surface on the segment: the point and its normal, turned to face back along the segment.
    pub(super) fn hit(&self, from: Vec3, to: Vec3) -> Option<(Vec3, Vec3)> {
        let v = |p: Vec3| Vector3::new(p.x, p.y, p.z);
        let hit = self.0.query_thin_line(v(from), v(to)).ok()??;
        let (p, n) = (hit.geometry.position, hit.geometry.normal);
        let n = Vec3::new(n.x, n.y, n.z).normalize_or_zero();
        Some((Vec3::new(p.x, p.y, p.z), if n.dot(to - from) > 0. { -n } else { n }))
    }
    /// Level ground at (x, z) within 8 cm of `y`.
    pub(super) fn level(&self, x: f32, z: f32, y: f32) -> bool {
        self.hit(Vec3::new(x, y + 0.6, z), Vec3::new(x, y - 0.6, z)).is_some_and(|(p, n)| n.y > 0.97 && (p.y - y).abs() < 0.08)
    }
    /// Nothing within `radius` of `at` from the ground to `height`: room to stand a tree.
    pub(super) fn clear(&self, at: Vec3, radius: f32, height: f32) -> bool {
        let level = (0..8).all(|k| {
            let p = at + ring(k, 8) * radius;
            self.level(p.x, p.z, at.y)
        });
        level
            && self.hit(at + Vec3::Y * 0.2, at + Vec3::Y * (height + 1.)).is_none()
            && [0.4, 1.6, 3.].iter().all(|&h| (0..8).all(|k| self.hit(at + Vec3::Y * h, at + Vec3::Y * h + ring(k, 8) * (radius + 0.3)).is_none()))
    }
    /// The first surface below `at`, from 1 m over it to [`DROP`] under: the point and its normal.
    pub(super) fn below(&self, at: Vec3) -> Option<(Vec3, Vec3)> {
        self.hit(at + Vec3::Y, at - Vec3::Y * DROP)
    }
    /// A surface within [`TOUCH`] of `at` along any of the six axes.
    pub(super) fn touches(&self, at: Vec3) -> bool {
        [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].iter().any(|&d| self.hit(at, at + d * TOUCH).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::{Piece, Route, Row, WebSpan};
    use super::*;
    use skate_core::physics::{board_world::WorldTriangle, contact::RetailContactMaterial};

    fn triangle(v: [[f32; 3]; 3]) -> WorldTriangle {
        let material = RetailContactMaterial { static_friction: 0., dynamic_friction: 0., restitution: 0. };
        WorldTriangle::from_vertices(v.map(|p| Vector3::new(p[0], p[1], p[2])), material, 0, 0, [0.; 3], 0.).unwrap()
    }

    /// A 40 x 40 m floor at y 100 about (300, -700), inside the park, and a 6 m wall along x = 310 (both faces).
    fn world() -> BoardWorld {
        let (x0, x1, z0, z1, y) = (280., 320., -720., -680., 100.);
        let mut t = vec![triangle([[x0, y, z0], [x0, y, z1], [x1, y, z0]]), triangle([[x1, y, z1], [x1, y, z0], [x0, y, z1]])];
        let (a, b, c, d) = ([310., y, z0], [310., y, z1], [310., y + 6., z1], [310., y + 6., z0]);
        for [p, q, r] in [[a, b, c], [a, c, d], [a, c, b], [a, d, c]] {
            t.push(triangle([p, q, r]));
        }
        BoardWorld::new(t)
    }

    fn piece(kind: Kind, at: [f32; 3], snap: Snap) -> Piece {
        Piece { zone: Zone::MainFloor, kind, at, yaw: 0., scale: 1., snap }
    }

    fn route(points: &'static [[f32; 3]]) -> Route {
        Route { zone: Zone::MainFloor, points, style: Style::Swag, palette: Palette::Halloween, garland: false }
    }

    fn sample() -> Layout<'static> {
        static PIECES: [Piece; 3] = [
            Piece { zone: Zone::MainFloor, kind: Kind::Hay, at: [300., 103., -700.], yaw: 0., scale: 1., snap: Snap::Ground },
            Piece { zone: Zone::MainFloor, kind: Kind::Witch, at: [290., 101., -690.], yaw: 1., scale: 1., snap: Snap::Ground },
            Piece { zone: Zone::MainFloor, kind: Kind::HangingGhost, at: [309.7, 104., -700.], yaw: 0., scale: 1., snap: Snap::Hung },
        ];
        static ROUTES: [Route; 1] = [Route {
            zone: Zone::MainFloor,
            points: &[[309.8, 105., -710.], [309.8, 105., -690.]],
            style: Style::Swag,
            palette: Palette::Halloween,
            garland: true,
        }];
        static ROWS: [Row; 1] = [Row {
            zone: Zone::MainFloor,
            kind: Kind::Pumpkin,
            points: &[[285., 100.5, -715.], [305., 100.5, -715.]],
            spacing: 2.,
            snap: Snap::Ground,
            scale: 1.,
        }];
        static WEBS: [WebSpan; 1] = [WebSpan { zone: Zone::MainFloor, frame: &[[309.8, 100.2, -705.], [309.8, 105.8, -705.], [309.8, 100.2, -695.]], spider: true }];
        Layout { pieces: &PIECES, routes: &ROUTES, rows: &ROWS, webs: &WEBS, flocks: &[[300., 115., -700.]], roosts: &[[309.7, 105., -702.]] }
    }

    #[test]
    fn a_ground_piece_lands_on_the_floor_and_a_blocked_one_is_dropped() {
        let pieces = [piece(Kind::Hay, [300., 103., -700.], Snap::Ground), piece(Kind::House, [309.5, 103., -700.], Snap::Ground)];
        let plan = resolve(&world(), &Layout { pieces: &pieces, ..Layout::EMPTY });
        assert_eq!(plan.pieces.len(), 1);
        assert!((plan.pieces[0].at.y - 100.).abs() < 0.01);
        assert!(!plan.pumpkins.is_empty(), "the hay's pumpkins come with it");
        assert!(plan.dropped.len() == 1 && plan.dropped[0].contains("House"), "{:?}", plan.dropped);
    }

    #[test]
    fn items_along_a_route_are_evenly_spaced() {
        let pts = along(&[Vec3::ZERO, Vec3::new(10., 0., 0.)], 3.);
        assert_eq!(pts.len(), 4); // three gaps of 3.33
        assert!(pts.windows(2).all(|w| (w[0].0.distance(w[1].0) - 10. / 3.).abs() < 1e-3));
    }

    #[test]
    fn a_route_that_touches_nothing_is_dropped() {
        let routes = [route(&[[295., 105., -700.], [305., 105., -700.]])]; // 5 m over the floor
        let plan = resolve(&world(), &Layout { routes: &routes, ..Layout::EMPTY });
        assert!(plan.strands.is_empty() && plan.dropped.len() == 1, "{:?}", plan.dropped);
    }

    #[test]
    fn resolving_twice_gives_the_same_park() {
        let (a, b) = (resolve(&world(), &sample()), resolve(&world(), &sample()));
        assert!(!a.pieces.is_empty() && !a.pumpkins.is_empty() && !a.strands.is_empty() && !a.webs.is_empty(), "{:?}", a.dropped);
        assert_eq!(format!("{:?}", a.pieces), format!("{:?}", b.pieces));
        assert_eq!(format!("{:?}", a.pumpkins), format!("{:?}", b.pumpkins));
    }

    #[test]
    fn a_pumpkin_tower_stacks_carved_jack_o_lanterns() {
        let pieces = [piece(Kind::PumpkinTower, [300., 100.5, -700.], Snap::Ground)];
        let plan = resolve(&world(), &Layout { pieces: &pieces, ..Layout::EMPTY });
        assert!((3..=5).contains(&plan.pumpkins.len()), "{:?} {:?}", plan.pumpkins, plan.dropped);
        assert!(plan.pumpkins.iter().all(|p| p.carving.is_some()));
        assert_eq!(plan.pumpkins.iter().filter(|p| p.light).count(), 1);
        assert!(plan.pumpkins.windows(2).all(|w| w[1].at.y > w[0].at.y && w[1].size < w[0].size));
    }

    #[test]
    fn a_candle_glow_lies_only_on_a_surface() {
        // A tower mid-floor; a pumpkin mid-floor; one on the floor's east edge (x 320) facing out over the drop.
        let pieces = [
            piece(Kind::PumpkinTower, [290., 100.5, -700.], Snap::Ground),
            Piece { yaw: 0., ..piece(Kind::Pumpkin, [300., 100.5, -690.], Snap::Top) },
            Piece { yaw: std::f32::consts::FRAC_PI_2, ..piece(Kind::Pumpkin, [319.6, 100.5, -690.], Snap::Top) },
        ];
        let plan = resolve(&world(), &Layout { pieces: &pieces, ..Layout::EMPTY });
        let (tower, singles) = plan.pumpkins.split_at(plan.pumpkins.len() - 2);
        assert!(tower.len() >= 3, "{:?}", plan.dropped);
        // Only the tower's bottom pumpkin stands on the floor; the rest sit on the pumpkin below, in the air.
        assert_eq!(tower.iter().map(|p| p.spill).collect::<Vec<_>>(), [vec![true], vec![false; tower.len() - 1]].concat());
        let carved = |p: &Pumpkin| p.carving.is_some();
        assert_eq!(singles.iter().map(|p| p.spill).collect::<Vec<_>>(), [carved(&singles[0]), false]);
    }

    #[test]
    fn a_ground_glow_lies_on_the_floor_or_is_cut_back() {
        use super::super::{Season, ground_glows};
        // A witch and a mist mid-floor; a mist at the floor's east edge (x 320, nothing below beyond it).
        let pieces = [
            piece(Kind::Witch, [290., 100.5, -690.], Snap::Ground),
            piece(Kind::Mist, [292., 100.5, -700.], Snap::Ground),
            piece(Kind::Mist, [319.3, 100.5, -700.], Snap::Ground),
        ];
        let world = world();
        let plan = resolve(&world, &Layout { pieces: &pieces, ..Layout::EMPTY });
        assert_eq!(plan.pieces.len(), 3, "{:?}", plan.dropped);
        let laid = glows(&world, &plan, Season::Halloween);
        let authored = |k: usize| -> Vec<Transform> { ground_glows(&plan.pieces[k], Season::Halloween).into_iter().map(|g| g.0).collect() };
        // Mid-floor the witch's flood pool and the mist's three patches lie as authored.
        let mid = [authored(0), authored(1)].concat();
        assert!(mid.len() == 4 && mid.iter().all(|t| laid.iter().any(|g| g.0 == *t)), "{laid:?}");
        // At the edge the patch past it is left out; whatever is laid lies on the floor.
        assert!(laid.iter().filter(|g| g.0.translation.x > 312.).count() < 3, "{laid:?}");
        let probe = Probe(&world);
        assert!(laid.iter().all(|g| lies_flat(&probe, g.0, 100.)), "{laid:?}");
    }

    #[test]
    fn a_little_spider_stops_short_of_the_floor() {
        // A web with its spider on the wall's face, its lowest edge 2 m over the floor (sagging to about 1 m).
        static WEBS: [WebSpan; 1] =
            [WebSpan { zone: Zone::MainFloor, frame: &[[309.7, 102., -706.], [309.7, 102., -694.], [309.7, 106., -700.]], spider: true }];
        let plan = resolve(&world(), &Layout { webs: &WEBS, ..Layout::EMPTY });
        let danglers = &plan.webs[0].danglers;
        assert!(!danglers.is_empty(), "{:?}", plan.dropped);
        // At the bottom of its bob each spider still hangs 30 cm over the floor.
        assert!(danglers.iter().all(|&(top, length)| top.y - length - super::super::halloween::BOB >= 100.3 - 1e-4), "{danglers:?}");
    }
}
