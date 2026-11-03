//! Skate 1's and Skate 2's minimaps, drawn as their games drew them natively. skate2port/old_hud1.py (Skate 1) and
//! old_hud.py (Skate 2) export each game's runtime/minimap.json; skate2port/MINIMAP_REPORT.md lists where every
//! number comes from. Skate 1 (default.xex 0x82468e60): a disc of street-plan tiles turned with the skater's
//! heading and zoomed out with speed, a north needle, icons pinned to the rim, the player arrow and the district
//! plate. Skate 2 (0x82353a48 / 0x823763a0): hudminimap's frame (disc, rings and arrow in one picture) with markers.
use crate::{
    apt_scene::{Draw, Vertex},
    multiplayer::appearance::RemoteCharacter,
    online_roster::OnlineRoster,
    physics::SkaterRuntime,
};
use bevy::prelude::*;
use std::{collections::HashMap, f32::consts::PI};

/// ponytail: the games ran their HUD at 30 Hz; their per-frame easing below is converted with it.
const RETAIL_HZ: f32 = 30.;
/// Heading: each frame it keeps 0.85 and takes 0.15 of the travel direction (faster than 1 m/s, renormalised)
/// or of the facing (0x82468e60; Skate 2's 0x82353a48 the same).
const HEADING_KEEP: f32 = 0.85;
const HEADING_TRAVEL: f32 = 1.;
/// Skate 1's zoom (0x8246a7c8): 1 - 0.2 (1 - cos(pi t)), t = (speed - 2.8) / 4.7 clamped, so 1 standing and 0.6
/// from 7.5 m/s; each frame it moves 0.016 of the way there (0x8246a680).
const ZOOM_FROM: f32 = 2.8;
const ZOOM_SPAN: f32 = 4.7;
const ZOOM_DEPTH: f32 = 0.2;
const ZOOM_EASE: f32 = 0.016;
/// A jump faster than this (m/s) is a teleport, not travel.
const TELEPORT: f32 = 60.;
/// Skate 1's north needle (0x82469a18): it aims at world (0, -4500); its quad (-21.12, -21.12)..(0, 0) is turned
/// 2.1627 rad, moved (46, 46), then turned to north's direction plus pi about the disc centre (not zoomed).
const NORTH: Vec2 = Vec2::new(0., -4500.);
const NEEDLE: f32 = 21.12;
const NEEDLE_TURN: f32 = 2.1627;
const NEEDLE_OUT: Vec2 = Vec2::splat(46.);
/// The disc's feathered rim as bands of falling opacity (no per-vertex colour here): ~1 px each at 1080p.
const FEATHER_RINGS: usize = 8;
const SEGMENTS: usize = 48;
/// The marker Skate 2's presentation event 0x1E draws: its first argument, "mini_teleport" in every tutorial use.
const WAYPOINT_ICON: &str = "mini_teleport";
/// Skate 1's race gate marker (mini_icon_racegate): skate1_online's way points shown ahead.
const GATE_ICON: &str = "mini_icon_racegate";

#[derive(serde::Deserialize)]
pub(crate) struct Page {
    pub(crate) texture: String,
    pub(crate) size: [u32; 2],
}
/// Skate 2's hudminimap frame: `extent` px of its page drawn at `position`.
#[derive(serde::Deserialize)]
struct Frame {
    #[serde(flatten)]
    page: Page,
    position: [f32; 2],
    extent: f32,
}
/// Skate 1's street plan: `metres`-square tiles by world (x, z) corner, drawn in a disc of alpha
/// 1 - smoothstep(feather, radius, px from the centre); a tile's corner sits `shift` px (before zoom) off its spot.
#[derive(serde::Deserialize)]
struct Tiles {
    feather: f32,
    radius: f32,
    metres: f32,
    shift: f32,
    size: [u32; 2],
    list: Vec<Tile>,
}
#[derive(serde::Deserialize)]
struct Tile {
    texture: String,
    world: [f32; 2],
}
#[derive(serde::Deserialize)]
struct Icon {
    #[serde(flatten)]
    page: Page,
    world: [f32; 2],
}
/// Skate 1's district plate: hudminimap's text laid out per district (`plates`, by index; empty = no name), for the
/// district of the nearest of `points` (world x, z, index). Retail latches it at world entry (0x824641f8 writes the
/// index GetMiniMapDistrict reads, then ShowMiniMap(true) plays the 'intro'), so it does not change while riding.
#[derive(serde::Deserialize)]
struct District {
    texture: String,
    size: [u32; 2],
    colour: [f32; 4],
    plates: Vec<Vec<Vertex>>,
    points: Vec<[f32; 3]>,
    /// hud_minimap's 'intro' frames (60 Hz) for the plate: [alpha, y px from its rest]; the last is at rest.
    #[serde(default)]
    intro: Vec<[f32; 2]>,
}

#[derive(serde::Deserialize)]
pub(crate) struct Minimap {
    #[serde(default)]
    frame: Option<Frame>,
    centre: [f32; 2],
    /// Px per metre.
    scale: f32,
    /// Markers further than this (metres times zoom) sit on it, in their direction.
    pin: f32,
    /// Past the pin a marker fades from 1 at `[0]` to 0 at `[1]`, and is gone past `[2]` (Skate 1).
    #[serde(default)]
    fade: Option<[f32; 3]>,
    /// Zooms out with speed (Skate 1).
    #[serde(default)]
    zoom: bool,
    #[serde(default)]
    tiles: Option<Tiles>,
    #[serde(default)]
    arrow: Option<Page>,
    #[serde(default)]
    north: Option<Page>,
    #[serde(default)]
    icons: Vec<Icon>,
    /// Skate 2's native marker icons by name, for markers the game places at run time.
    #[serde(default)]
    markers: HashMap<String, Page>,
    /// Other players' dots, by roster slot.
    #[serde(default)]
    skaters: Vec<Page>,
    #[serde(default)]
    district: Option<District>,
    #[serde(skip)]
    view: View,
}

#[derive(Default)]
struct View {
    at: Option<Vec3>,
    heading: Vec2,
    zoom: f32,
    /// The district plate latched at world entry, and 60 Hz frames since its intro began.
    district: Option<usize>,
    entered: f32,
    /// Skate 1's race gates shown (world x, z): [`Minimap::set_gates`].
    gates: Vec<[f32; 2]>,
    /// Named markers (icon, world x, z): [`Minimap::set_markers`].
    markers: Vec<(String, [f32; 2])>,
    /// Skate 1's offered challenges (icon, world x, z) in place of the static challenge icons; None: the static set
    /// ([`Minimap::set_challenges`]).
    challenges: Option<Vec<(String, [f32; 2])>>,
}

/// The skater's world position and facing (world x, z).
pub(crate) fn skater_pose(skater: &SkaterRuntime) -> (Vec3, Vec2) {
    let m = skater.animated_skeleton.roots.animation_to_world;
    (Vec3::new(m[3][0], m[3][1], m[3][2]), Vec2::new(m[2][0], m[2][2]))
}
/// Each other player's roster slot (host first, as every client sees it) and world (x, z).
pub(crate) fn others<'a>(
    roster: Option<&OnlineRoster>,
    remotes: impl Iterator<Item = (&'a RemoteCharacter, &'a GlobalTransform)>,
) -> Vec<(usize, Vec2)> {
    let Some(roster) = roster else { return Vec::new() };
    remotes
        .filter_map(|(remote, at)| {
            let slot = roster.members.iter().position(|m| m.actor == remote.0)?;
            Some((slot, xz(at.translation())))
        })
        .collect()
}

fn xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

fn quad(texture: &str, size: [u32; 2], centre: Vec2, half: Vec2, alpha: f32) -> Draw {
    let v = |x: f32, y: f32, u: f32, w: f32| Vertex { position: [centre.x + x * half.x, centre.y + y * half.y], uv: [u, w] };
    Draw {
        texture: texture.into(),
        size,
        vertices: vec![v(-1., -1., 0., 0.), v(1., -1., 1., 0.), v(1., 1., 1., 1.), v(-1., -1., 0., 0.), v(1., 1., 1., 1.), v(-1., 1., 0., 1.)],
        multiply: [1., 1., 1., alpha],
        add: [0.; 4],
    }
}

/// `poly` (convex) clipped to the box `lo`..`hi` (Sutherland-Hodgman).
fn clip(mut poly: Vec<Vec2>, lo: Vec2, hi: Vec2) -> Vec<Vec2> {
    for (axis, bound, below) in [(0, lo.x, false), (0, hi.x, true), (1, lo.y, false), (1, hi.y, true)] {
        let inside = |p: Vec2| if below { p[axis] <= bound } else { p[axis] >= bound };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for (i, &a) in poly.iter().enumerate() {
            let b = poly[(i + 1) % poly.len()];
            if inside(a) {
                out.push(a);
            }
            if inside(a) != inside(b) {
                out.push(a + (b - a) * ((bound - a[axis]) / (b[axis] - a[axis])));
            }
        }
        poly = out;
        if poly.len() < 3 {
            return Vec::new();
        }
    }
    poly
}

/// A page's texture name without folder and extension (`assets/minimap/icons/mini_icon_spot.rgba` -> mini_icon_spot).
fn stem(page: &Page) -> &str {
    let file = page.texture.rsplit('/').next().unwrap_or_default();
    file.split('.').next().unwrap_or_default()
}
/// A challenge marker old_hud1 places (mini_icon_chal_<kind>_<family>, mini_icon_spot); the X Games entrance is a place.
fn challenge_page(page: &Page) -> bool {
    let s = stem(page);
    (s.starts_with("mini_icon_chal_") && s != "mini_icon_chal_xgames") || s == "mini_icon_spot"
}

impl Minimap {
    /// Follow the skater at `at` facing `facing` (world x, z), `dt` seconds on: the game's heading and zoom easing.
    pub(crate) fn update(&mut self, at: Vec3, facing: Vec2, dt: f32) {
        let facing = facing.normalize_or(Vec2::NEG_Y);
        let v = &mut self.view;
        let Some(last) = v.at.filter(|_| dt > 0.) else {
            if v.at.is_none() {
                *v = View { at: Some(at), heading: facing, zoom: 1., ..default() };
                self.enter();
            }
            return;
        };
        v.at = Some(at);
        v.entered += dt * 60.;
        let velocity = (at - last) / dt;
        let teleport = velocity.length() > TELEPORT;
        let velocity = if teleport { Vec3::ZERO } else { velocity };
        let (travel, speed) = (xz(velocity), velocity.length());
        let keep = HEADING_KEEP.powf(dt * RETAIL_HZ);
        v.heading = if travel.length() > HEADING_TRAVEL {
            (v.heading * keep + travel.normalize() * (1. - keep)).normalize_or(facing)
        } else {
            v.heading * keep + facing * (1. - keep)
        };
        if self.zoom {
            let t = ((speed - ZOOM_FROM) / ZOOM_SPAN).clamp(0., 1.);
            let target = (1. - ZOOM_DEPTH * (1. - (PI * t).cos())).max(0.01);
            v.zoom = target + (v.zoom - target) * (1. - ZOOM_EASE).powf(dt * RETAIL_HZ);
        }
        if teleport {
            self.enter();
        }
    }

    /// World entry: latch the plate's district and, when it changed, play its intro.
    // ponytail: retail replays the intro on every world entry; a teleport within the district (or a bail's respawn
    // jump) doesn't here, as whether Skate 1's marker go-to re-enters the world wasn't traced.
    fn enter(&mut self) {
        let at = self.at();
        let Some(d) = &self.district else { return };
        let near = d.points.iter().min_by(|a, b| at.distance_squared(Vec2::new(a[0], a[1])).total_cmp(&at.distance_squared(Vec2::new(b[0], b[1]))));
        let near = near.map(|p| p[2] as usize);
        if near != self.view.district {
            (self.view.district, self.view.entered) = (near, 0.);
        }
    }

    fn at(&self) -> Vec2 {
        xz(self.view.at.unwrap_or_default())
    }
    /// Stage right and the skater's heading (up on the disc), in world (x, z).
    fn basis(&self) -> (Vec2, Vec2) {
        let forward = self.view.heading.normalize_or(Vec2::NEG_Y);
        (Vec2::new(-forward.y, forward.x), forward)
    }
    /// World-aligned px from the centre -> stage px.
    fn stage(&self, q: Vec2) -> Vec2 {
        let (right, forward) = self.basis();
        Vec2::from(self.centre) + Vec2::new(q.dot(right), -q.dot(forward))
    }

    /// Skate 1's race gates on the map until the next call (world x, z), each a mini_icon_racegate marker.
    pub(crate) fn set_gates(&mut self, gates: Vec<[f32; 2]>) {
        self.view.gates = gates;
    }

    /// Skate 2's tutorial lesson markers until the next call (icon in `markers`, world x, z).
    pub(crate) fn set_markers(&mut self, markers: Vec<(String, [f32; 2])>) {
        self.view.markers = markers;
    }

    /// Skate 1's offered challenges until the next call: their icons (old_hud1's mini_icon_chal_* / mini_icon_spot) at
    /// world (x, z) replace the static challenge icons; None draws the static set.
    pub(crate) fn set_challenges(&mut self, challenges: Option<Vec<(String, [f32; 2])>>) {
        self.view.challenges = challenges;
    }

    /// The static icons' page named `icon`, else its Thrasher art (old_hud1's fallback); None when neither is exported.
    fn icon_page(&self, icon: &str) -> Option<&Page> {
        let find = |name: &str| self.icons.iter().map(|i| &i.page).find(|p| stem(p) == name);
        find(icon).or_else(|| find(&format!("{}_thrash", icon.rsplit_once('_')?.0)))
    }

    /// Everything the minimap draws this frame, back to front. `others`: other players' (roster slot, world x, z);
    /// `waypoint`: Skate 2's tutorial marker (world x, z).
    pub(crate) fn draws(&self, others: &[(usize, Vec2)], waypoint: Option<[f32; 2]>) -> Vec<Draw> {
        let mut out = Vec::new();
        if let Some(f) = &self.frame {
            let [x, y] = f.position;
            let (u, v) = (f.extent / f.page.size[0] as f32, f.extent / f.page.size[1] as f32);
            let mut frame = quad(&f.page.texture, f.page.size, Vec2::new(x, y) + f.extent / 2., Vec2::splat(f.extent / 2.), 1.);
            for p in &mut frame.vertices {
                p.uv = [p.uv[0] * u, p.uv[1] * v];
            }
            out.push(frame);
        }
        if let Some(tiles) = &self.tiles {
            self.tile_draws(tiles, &mut out);
        }
        out.extend(self.north.as_ref().map(|page| self.needle(page)));
        let offered = self.view.challenges.as_ref();
        let icons = self.icons.iter().filter(|i| offered.is_none() || !challenge_page(&i.page));
        out.extend(icons.filter_map(|i| self.marker(Vec2::from(i.world), &i.page, true)));
        for (icon, at) in offered.into_iter().flatten() {
            out.extend(self.icon_page(icon).and_then(|page| self.marker(Vec2::from(*at), page, true)));
        }
        if !self.skaters.is_empty() {
            out.extend(others.iter().filter_map(|&(slot, at)| self.marker(at, &self.skaters[slot % self.skaters.len()], true)));
        }
        // UNTRACED: the lesson markers' order against the waypoint (they never share a spot in the tutorial).
        out.extend(self.view.markers.iter().filter_map(|(icon, at)| self.marker(Vec2::from(*at), self.markers.get(icon)?, false)));
        out.extend(waypoint.zip(self.markers.get(WAYPOINT_ICON)).and_then(|(w, page)| self.marker(Vec2::from(w), page, false)));
        if let Some(page) = self.markers.get(GATE_ICON) {
            out.extend(self.view.gates.iter().filter_map(|&g| self.marker(Vec2::from(g), page, true)));
        }
        out.extend(self.arrow.as_ref().map(|a| {
            let half = Vec2::new(a.size[0] as f32, a.size[1] as f32) / 2.;
            quad(&a.texture, a.size, Vec2::from(self.centre), half, 1.)
        }));
        out.extend(self.district.as_ref().and_then(|d| self.plate(d)));
        out
    }

    /// A marker at `world` (0x8246b7b0 / Skate 2 0x823763a0): its direction turned with the heading, `scale` px per
    /// zoomed metre out to the pin, there beyond it; drawn upright at its texture's size. Skate 1 fades it past the pin.
    fn marker(&self, world: Vec2, page: &Page, fades: bool) -> Option<Draw> {
        let zoom = if self.zoom { self.view.zoom } else { 1. };
        let d = world - self.at();
        let m = d.length() * zoom;
        let alpha = match self.fade {
            Some([start, end, hide]) if fades && m > self.pin => {
                if m > hide { 0. } else if m > start { 1. - (m - start) / (end - start) } else { 1. }
            }
            _ => 1.,
        };
        if alpha <= 0. {
            return None;
        }
        let q = if m > self.pin { d.normalize_or_zero() * self.pin } else { d * zoom } * self.scale;
        let half = Vec2::new(page.size[0] as f32, page.size[1] as f32) / 2.;
        Some(quad(&page.texture, page.size, self.stage(q), half, alpha))
    }

    /// The north needle: off the disc at north's direction less ~55 degrees, pointing out.
    fn needle(&self, page: &Page) -> Draw {
        let (right, forward) = self.basis();
        let north = (NORTH - self.at()).normalize_or(Vec2::NEG_Y);
        let s = Vec2::new(north.dot(right), -north.dot(forward));
        let turn = Vec2::from_angle(s.x.atan2(-s.y) + PI);
        let c = Vec2::from(self.centre);
        let p = |x: f32, y: f32, u: f32, v: f32| {
            let at = c + turn.rotate(Vec2::from_angle(NEEDLE_TURN).rotate(Vec2::new(x, y)) + NEEDLE_OUT);
            Vertex { position: at.into(), uv: [u, v] }
        };
        let (a, b, cc, d) = (p(-NEEDLE, -NEEDLE, 0., 0.), p(0., -NEEDLE, 1., 0.), p(0., 0., 1., 1.), p(-NEEDLE, 0., 0., 1.));
        Draw { texture: page.texture.clone(), size: page.size, vertices: vec![a.clone(), b, cc.clone(), a, cc, d], multiply: [1.; 4], add: [0.; 4] }
    }

    /// The street-plan tiles inside the disc: its alpha-1 core and feather bands (polygons about the centre),
    /// each clipped to every tile square it meets. One draw per tile and band.
    // ponytail: tile pages load on first sight and stay (apt_canvas caches every texture): 454 x 256 KB = 116 MB
    // of VRAM if every tile of the city is seen in one session; evict far tiles if that ever matters.
    fn tile_draws(&self, t: &Tiles, out: &mut Vec<Draw>) {
        let zoom = if self.zoom { self.view.zoom } else { 1. };
        let side = t.metres * self.scale * zoom;
        let (right, forward) = self.basis();
        // Stage offset -> world-aligned px (the inverse of `stage`).
        let world_px = |angle: f32, r: f32| {
            let s = Vec2::from_angle(angle) * r;
            right * s.x - forward * s.y
        };
        let step = std::f32::consts::TAU / SEGMENTS as f32;
        // Cells as (point count, points): a frame clips ~430 cells against each tile in view, so no Vec per cell.
        let ring = |r0: f32, r1: f32| -> Vec<(usize, [Vec2; 4])> {
            (0..SEGMENTS)
                .map(|i| {
                    let (a, b) = (i as f32 * step, (i + 1) as f32 * step);
                    if r0 == 0. {
                        (3, [Vec2::ZERO, world_px(a, r1), world_px(b, r1), Vec2::ZERO])
                    } else {
                        (4, [world_px(a, r0), world_px(a, r1), world_px(b, r1), world_px(b, r0)])
                    }
                })
                .collect()
        };
        let width = (t.radius - t.feather) / FEATHER_RINGS as f32;
        let bands: Vec<(f32, Vec<(usize, [Vec2; 4])>)> = std::iter::once((1., ring(0., t.feather)))
            .chain((0..FEATHER_RINGS).map(|i| {
                let r = t.feather + (i as f32 + 0.5) * width;
                let s = ((r - t.feather) / (t.radius - t.feather)).clamp(0., 1.);
                (1. - s * s * (3. - 2. * s), ring(t.feather + i as f32 * width, t.feather + (i + 1) as f32 * width))
            }))
            .collect();
        let at = self.at();
        for tile in &t.list {
            let lo = ((Vec2::from(tile.world) - at) * self.scale + t.shift) * zoom;
            let hi = lo + side;
            if Vec2::ZERO.clamp(lo, hi).length() >= t.radius {
                continue;
            }
            let v = |q: Vec2| Vertex { position: self.stage(q).into(), uv: ((q - lo) / side).into() };
            // `clip` keeps a cell wholly inside the tile as it is, and empties one wholly past an edge. The margin
            // covers the rounding of the intersection points an earlier edge adds (~1e-4 px on a ~100 px disc), so
            // both shortcuts are exact; cells near an edge take the full clip.
            const MARGIN: f32 = 0.01;
            let inside = |p: &Vec2| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
            let outside = |pts: &[Vec2]| {
                pts.iter().all(|p| p.x < lo.x - MARGIN)
                    || pts.iter().all(|p| p.x > hi.x + MARGIN)
                    || pts.iter().all(|p| p.y < lo.y - MARGIN)
                    || pts.iter().all(|p| p.y > hi.y + MARGIN)
            };
            for (alpha, cells) in &bands {
                let mut vertices = Vec::new();
                for (n, cell) in cells {
                    let cell = &cell[..*n];
                    if outside(cell) {
                        continue;
                    }
                    let clipped;
                    let poly = if cell.iter().all(inside) {
                        cell
                    } else {
                        clipped = clip(cell.to_vec(), lo, hi);
                        &clipped[..]
                    };
                    for i in 1..poly.len().saturating_sub(1) {
                        vertices.extend([v(poly[0]), v(poly[i]), v(poly[i + 1])]);
                    }
                }
                if !vertices.is_empty() {
                    out.push(Draw { texture: tile.texture.clone(), size: t.size, vertices, multiply: [1., 1., 1., *alpha], add: [0.; 4] });
                }
            }
        }
    }

    /// The latched district's plate, at its intro frame (fading in, settling 1 px down).
    // ponytail: only the plate fades; the disc is native (MiniMapComponent, constant tint) and shows from intro frame 1.
    fn plate(&self, d: &District) -> Option<Draw> {
        let vertices = d.plates.get(self.view.district?).filter(|v| !v.is_empty())?;
        let [alpha, dy] = d.intro.get(self.view.entered as usize).or(d.intro.last()).copied().unwrap_or([1., 0.]);
        let vertices = vertices.iter().map(|v| Vertex { position: [v.position[0], v.position[1] + dy], uv: v.uv }).collect();
        let [r, g, b, a] = d.colour;
        Some(Draw { texture: d.texture.clone(), size: d.size, vertices, multiply: [r, g, b, a * alpha], add: [0.; 4] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(name: &str, size: u32) -> Page {
        Page { texture: name.into(), size: [size; 2] }
    }
    fn centre(d: &Draw) -> Vec2 {
        d.vertices.iter().map(|v| Vec2::from(v.position)).sum::<Vec2>() / d.vertices.len() as f32
    }
    /// Skate 2's: frame and markers, 1 px per metre, pinned at 71.2811.
    fn skate2() -> Minimap {
        Minimap {
            frame: Some(Frame { page: page("frame", 256), position: [986.8, 64.9], extent: 179. }),
            centre: [1076., 153.],
            scale: 1.,
            pin: 71.2811,
            fade: None,
            zoom: false,
            tiles: None,
            arrow: None,
            north: None,
            icons: Vec::new(),
            markers: [(WAYPOINT_ICON.to_string(), page("teleport", 32))].into(),
            skaters: vec![page("skater", 32)],
            district: None,
            view: View::default(),
        }
    }
    /// Skate 1's numbers (old_hud1.py), one tile and one icon.
    fn skate1() -> Minimap {
        Minimap {
            frame: None,
            centre: [1088.3, 144.4],
            scale: 1.0562,
            pin: 71.2811,
            fade: Some([60., 350., 10000.]),
            zoom: true,
            tiles: Some(Tiles {
                feather: 62.5968,
                radius: 67.5968,
                metres: 128.,
                shift: 128. - 135.1936,
                size: [256, 256],
                list: vec![Tile { texture: "tile".into(), world: [0., 0.] }],
            }),
            arrow: Some(page("arrow", 16)),
            north: Some(page("north", 32)),
            icons: Vec::new(),
            markers: HashMap::new(),
            skaters: (1..=8).map(|i| page(&format!("skater{i:02}"), 16)).collect(),
            district: Some(District {
                texture: "font".into(),
                size: [8, 8],
                colour: [1.; 4],
                plates: vec![vec![Vertex { position: [1., 1.], uv: [0., 0.] }; 3], Vec::new()],
                points: vec![[0., 0., 0.], [1000., 0., 1.]],
                intro: Vec::new(),
            }),
            view: View::default(),
        }
    }

    /// P5: a Skate 1 offer list replaces the static challenge icons (not the places); an icon a family lacks draws its
    /// Thrasher art (old_hud1's fallback); a name with neither draws nothing; None brings the static set back.
    #[test]
    fn skate1_challenge_icons_follow_the_offer_list() {
        let mut m = skate1();
        let icon = |name: &str, world| Icon { page: Page { texture: format!("assets/minimap/icons/{name}.rgba"), size: [32; 2] }, world };
        m.icons = vec![
            icon("mini_icon_chal_tutorial_smag", [64., 74.]),
            icon("mini_icon_chal_jam_thrash", [74., 64.]),
            icon("mini_icon_planb", [54., 64.]),
        ];
        m.update(Vec3::new(64., 0., 64.), Vec2::NEG_Y, 0.);
        let names = |m: &Minimap| -> Vec<String> {
            m.draws(&[], None).into_iter().map(|d| d.texture).filter(|t| t.contains("mini_icon_")).collect()
        };
        assert_eq!(names(&m).len(), 3, "the static set");
        m.set_challenges(Some(vec![("mini_icon_chal_jam_nopath".into(), [60., 60.])]));
        assert_eq!(names(&m), ["assets/minimap/icons/mini_icon_planb.rgba", "assets/minimap/icons/mini_icon_chal_jam_thrash.rgba"]);
        m.set_challenges(Some(vec![("mini_icon_chal_unknown_smag".into(), [60., 60.])]));
        assert_eq!(names(&m), ["assets/minimap/icons/mini_icon_planb.rgba"], "no page, no marker");
        m.set_challenges(None);
        assert_eq!(names(&m).len(), 3);
    }

    #[test]
    fn skate2_markers_sit_a_metre_a_pixel_out_and_pin_at_its_radius() {
        let mut m = skate2();
        m.update(Vec3::new(155., 0., 424.), Vec2::NEG_Y, 0.);
        let draws = m.draws(&[], Some([155. + 30., 424.]));
        // No street map: the frame, then the waypoint.
        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].texture, "frame");
        let w = centre(&draws[1]);
        assert!((w - Vec2::new(1076. + 30., 153.)).length() < 1e-3, "{w}");
        assert!((draws[1].vertices[2].position[0] - draws[1].vertices[0].position[0] - 32.).abs() < 1e-3, "32 px");
        // 1 km north, facing north: pinned straight up, unfaded.
        let far = m.draws(&[], Some([155., 424. - 1000.]));
        assert!((centre(&far[1]) - Vec2::new(1076., 153. - 71.2811)).length() < 1e-3);
        assert_eq!(far[1].multiply[3], 1.);
        // Facing east, north is to the left.
        m.view.heading = Vec2::X;
        assert!(centre(&m.draws(&[], Some([155., 424. - 1000.]))[1]).x < 1076. - 70.);
        // Other players by slot.
        assert_eq!(m.draws(&[(3, Vec2::new(155., 400.))], None)[1].texture, "skater");
    }

    /// Skate 1's race gates (online audit 10-12): a mini_icon_racegate marker at each gate shown, under the arrow,
    /// faded past the pin as the other icons; none without the page (an install before the marker was exported).
    #[test]
    fn skate1_race_gates_are_racegate_markers() {
        let mut m = skate1();
        m.update(Vec3::new(64., 0., 64.), Vec2::NEG_Y, 0.);
        m.set_gates(vec![[64. + 20., 64.], [64., 64. - 200.]]);
        assert!(!m.draws(&[], None).iter().any(|d| d.texture == "gate"), "no page, no marker");
        m.markers.insert(GATE_ICON.into(), page("gate", 16));
        let draws = m.draws(&[], None);
        let gates: Vec<&Draw> = draws.iter().filter(|d| d.texture == "gate").collect();
        assert_eq!(gates.len(), 2);
        assert!(gates[1].multiply[3] < 1., "past the pin it fades");
        let arrow = draws.iter().position(|d| d.texture == "arrow").unwrap();
        assert!(draws.iter().rposition(|d| d.texture == "gate").unwrap() < arrow, "under the arrow");
        m.set_gates(Vec::new());
        assert!(!m.draws(&[], None).iter().any(|d| d.texture == "gate"));
    }

    /// User 10-13 (bug 8): the tutorial's lesson markers draw their named icon as the waypoint does (pinned, unfaded),
    /// under the waypoint; a name with no page draws nothing.
    #[test]
    fn skate2_lesson_markers_draw_their_icon() {
        let mut m = skate2();
        m.markers.insert("mini_smag".into(), page("smag", 32));
        m.update(Vec3::new(155., 0., 424.), Vec2::NEG_Y, 0.);
        m.set_markers(vec![("mini_smag".into(), [155. + 30., 424.]), ("mini_unknown".into(), [155., 400.])]);
        let draws = m.draws(&[], Some([155., 424. - 1000.]));
        let textures: Vec<&str> = draws.iter().map(|d| d.texture.as_str()).collect();
        assert_eq!(textures, ["frame", "smag", "teleport"]);
        assert!((centre(&draws[1]) - Vec2::new(1076. + 30., 153.)).length() < 1e-3);
        m.set_markers(Vec::new());
        assert_eq!(m.draws(&[], None).len(), 1);
    }

    #[test]
    fn skate1_tiles_fill_the_disc_offset_as_the_game_draws_them() {
        let mut m = skate1();
        m.update(Vec3::new(64., 0., 64.), Vec2::NEG_Y, 0.);
        let draws = m.draws(&[], None);
        let tiles: Vec<&Draw> = draws.iter().filter(|d| d.texture == "tile").collect();
        assert_eq!(tiles.len(), 1 + FEATHER_RINGS, "one draw per band");
        let c = Vec2::from(m.centre);
        for v in tiles.iter().flat_map(|d| &d.vertices) {
            assert!(Vec2::from(v.position).distance(c) <= 67.5968 + 1e-3);
        }
        // The disc centre shows the point 6.81 m +x +z of the skater: uv (64 + 6.81) / 128.
        let middle = tiles[0].vertices.iter().find(|v| Vec2::from(v.position).distance(c) < 1e-3).unwrap();
        let expected = (64. * 1.0562 + 135.1936 - 128.) / 135.1936;
        assert!((Vec2::from(middle.uv) - Vec2::splat(expected)).length() < 1e-4, "{:?}", middle.uv);
        // The core is opaque, the rim fades.
        assert_eq!(tiles[0].multiply[3], 1.);
        assert!(tiles[FEATHER_RINGS].multiply[3] < 0.05);
        // Standing at the tile's corner the disc covers only its +x +z quarter.
        m.view.at = Some(Vec3::new(-10., 0., -10.));
        for v in m.draws(&[], None).iter().filter(|d| d.texture == "tile").flat_map(|d| &d.vertices) {
            assert!(v.uv[0] >= -1e-5 && v.uv[1] >= -1e-5);
        }
        // Far from every tile, none draws.
        m.view.at = Some(Vec3::new(5000., 0., 5000.));
        assert!(!m.draws(&[], None).iter().any(|d| d.texture == "tile"));
    }

    /// `tile_draws` as it was before perf 10-12 (every cell clipped, fresh Vecs throughout): the oracle the faster
    /// one must match bit for bit.
    fn reference_tile_draws(m: &Minimap, t: &Tiles) -> Vec<Draw> {
        let mut out = Vec::new();
        let zoom = if m.zoom { m.view.zoom } else { 1. };
        let side = t.metres * m.scale * zoom;
        let (right, forward) = m.basis();
        let world_px = |angle: f32, r: f32| {
            let s = Vec2::from_angle(angle) * r;
            right * s.x - forward * s.y
        };
        let step = std::f32::consts::TAU / SEGMENTS as f32;
        let ring = |r0: f32, r1: f32| -> Vec<Vec<Vec2>> {
            (0..SEGMENTS)
                .map(|i| {
                    let (a, b) = (i as f32 * step, (i + 1) as f32 * step);
                    if r0 == 0. {
                        vec![Vec2::ZERO, world_px(a, r1), world_px(b, r1)]
                    } else {
                        vec![world_px(a, r0), world_px(a, r1), world_px(b, r1), world_px(b, r0)]
                    }
                })
                .collect()
        };
        let width = (t.radius - t.feather) / FEATHER_RINGS as f32;
        let bands: Vec<(f32, Vec<Vec<Vec2>>)> = std::iter::once((1., ring(0., t.feather)))
            .chain((0..FEATHER_RINGS).map(|i| {
                let r = t.feather + (i as f32 + 0.5) * width;
                let s = ((r - t.feather) / (t.radius - t.feather)).clamp(0., 1.);
                (1. - s * s * (3. - 2. * s), ring(t.feather + i as f32 * width, t.feather + (i + 1) as f32 * width))
            }))
            .collect();
        let at = m.at();
        for tile in &t.list {
            let lo = ((Vec2::from(tile.world) - at) * m.scale + t.shift) * zoom;
            let hi = lo + side;
            if Vec2::ZERO.clamp(lo, hi).length() >= t.radius {
                continue;
            }
            for (alpha, cells) in &bands {
                let vertices: Vec<Vertex> = cells
                    .iter()
                    .flat_map(|cell| {
                        let poly = clip(cell.clone(), lo, hi);
                        let v = |q: Vec2| Vertex { position: m.stage(q).into(), uv: ((q - lo) / side).into() };
                        (1..poly.len().saturating_sub(1)).flat_map(move |i| [v(poly[0]), v(poly[i]), v(poly[i + 1])]).collect::<Vec<_>>()
                    })
                    .collect();
                if !vertices.is_empty() {
                    out.push(Draw { texture: tile.texture.clone(), size: t.size, vertices, multiply: [1., 1., 1., *alpha], add: [0.; 4] });
                }
            }
        }
        out
    }

    fn same_draws(a: &[Draw], b: &[Draw]) -> bool {
        let bits = |d: &Draw| -> Vec<u32> { d.vertices.iter().flat_map(|v| [v.position[0], v.position[1], v.uv[0], v.uv[1]]).map(f32::to_bits).collect() };
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| {
                a.texture == b.texture && a.size == b.size && a.multiply.map(f32::to_bits) == b.multiply.map(f32::to_bits) && a.add.map(f32::to_bits) == b.add.map(f32::to_bits) && bits(a) == bits(b)
            })
    }

    /// Perf 10-12: the street-plan tiles are the same draws, bit for bit, as the clip-every-cell original, over a
    /// city of tiles at many places, headings and zooms (tile corners and edges included).
    #[test]
    fn tile_draws_match_the_reference_bit_for_bit() {
        let mut m = skate1();
        let tiles = m.tiles.as_mut().unwrap();
        tiles.list = (-4..4).flat_map(|x| (-4..4).map(move |z| Tile { texture: format!("t{x}_{z}"), world: [x as f32 * 128., z as f32 * 128.] })).collect();
        let mut compared = 0;
        for i in 0..2000 {
            let f = i as f32;
            // Positions over the grid (exact tile corners and edges every 50th), headings all round, zoom 0.6..1.
            let at = if i % 50 == 0 { Vec3::new(((i / 50) % 8) as f32 * 128. - 512., 0., 0.) } else { Vec3::new((f * 7.31).sin() * 400., 0., (f * 3.17).cos() * 400.) };
            m.view = View { at: Some(at), heading: Vec2::from_angle(f * 0.737), zoom: 0.6 + 0.4 * (f * 0.11).sin().abs(), ..View::default() };
            if i % 7 == 0 {
                m.view.heading = Vec2::NEG_Y; // axis-aligned cells meet tile edges exactly
            }
            let t = m.tiles.as_ref().unwrap();
            let mut fast = Vec::new();
            m.tile_draws(t, &mut fast);
            let reference = reference_tile_draws(&m, t);
            assert!(same_draws(&fast, &reference), "pose {i}: {} draws vs {}", fast.len(), reference.len());
            compared += reference.len();
        }
        assert!(compared > 2000 * (1 + FEATHER_RINGS), "the poses must show tiles");
    }

    /// Perf 10-12: the Skate 1 minimap's per-frame cost (update + draws) riding round Slappy's, as installed.
    /// SKATE3_ASSET_ROOT = the install's assets. `cargo test ... skate1_minimap_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires the installed Skate 1 HUD"]
    fn skate1_minimap_frame_cost() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private/hud-skate1");
        let mut m: Minimap = serde_json::from_slice(&std::fs::read(root.join("runtime/minimap.json")).unwrap()).unwrap();
        let frames = 3000;
        let (mut draws, mut vertices) = (0, 0);
        let t = std::time::Instant::now();
        for i in 0..frames {
            // 8 m/s round a 60 m circle near Slappy's.
            let a = i as f32 / 60. * 8. / 60.;
            let at = Vec3::new(1418. + 60. * a.cos(), 0., 1046. + 60. * a.sin());
            m.update(at, Vec2::new(-a.sin(), a.cos()), 1. / 60.);
            let d = m.draws(&[], None);
            draws += d.len();
            vertices += d.iter().map(|d| d.vertices.len()).sum::<usize>();
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / frames as f64;
        eprintln!("skate1 minimap: {us:.1} us/frame, {} draws, {} vertices per frame", draws / frames, vertices / frames);
        // tile_draws alone, against the pre-10-12 copy, on the same poses.
        let (mut fast, mut reference) = (0., 0.);
        for i in 0..frames {
            let a = i as f32 / 60. * 8. / 60.;
            m.update(Vec3::new(1418. + 60. * a.cos(), 0., 1046. + 60. * a.sin()), Vec2::new(-a.sin(), a.cos()), 1. / 60.);
            let tiles = m.tiles.as_ref().unwrap();
            let t = std::time::Instant::now();
            let mut out = Vec::new();
            m.tile_draws(tiles, &mut out);
            fast += t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let old = reference_tile_draws(&m, tiles);
            reference += t.elapsed().as_secs_f64();
            assert!(same_draws(&out, &old), "frame {i}");
        }
        eprintln!("tile_draws {:.1} us/frame (was {:.1})", fast * 1e6 / frames as f64, reference * 1e6 / frames as f64);
    }

    #[test]
    fn skate1_zooms_out_with_speed_and_turns_toward_travel() {
        let mut m = skate1();
        let mut at = Vec3::ZERO;
        m.update(at, Vec2::NEG_Y, 0.);
        assert_eq!(m.view.zoom, 1.);
        // 10 m/s east for 10 s (facing north): zoom eases to 0.6, heading turns east.
        for _ in 0..600 {
            at.x += 10. / 60.;
            m.update(at, Vec2::NEG_Y, 1. / 60.);
        }
        assert!((m.view.zoom - 0.6).abs() < 0.01, "{}", m.view.zoom);
        assert!(m.view.heading.normalize().dot(Vec2::X) > 0.999);
        // At 30 Hz the same easing is the game's per-frame step.
        let mut n = skate1();
        n.update(Vec3::ZERO, Vec2::NEG_Y, 0.);
        n.update(Vec3::new(1., 0., 0.), Vec2::NEG_Y, 1. / 30.);
        assert!((n.view.zoom - (1. + (0.6 - 1.) * 0.016)).abs() < 1e-5, "{}", n.view.zoom);
        // Standing still it settles back and the heading follows the facing.
        for _ in 0..1200 {
            m.update(at, Vec2::Y, 1. / 60.);
        }
        assert!((m.view.zoom - 1.).abs() < 0.01 && m.view.heading.normalize().dot(Vec2::Y) > 0.999);
        // A teleport is not speed.
        let zoom = m.view.zoom;
        m.update(at + Vec3::new(3000., 0., 0.), Vec2::Y, 1. / 60.);
        assert!(m.view.zoom >= zoom - 1e-6);
    }

    #[test]
    fn skate1_needle_and_icons_follow_the_game() {
        let mut m = skate1();
        m.update(Vec3::new(0., 0., 1000.), Vec2::NEG_Y, 0.);
        let draws = m.draws(&[], None);
        let c = Vec2::from(m.centre);
        // Facing north: the needle's centre is 74.42 px out at 54.6 degrees left of up, as 0x82469a18 puts it.
        let needle = centre(draws.iter().find(|d| d.texture == "north").unwrap()) - c;
        assert!((needle.length() - 74.42).abs() < 0.05, "{needle}");
        assert!((needle.x.atan2(-needle.y).to_degrees() + 54.6).abs() < 0.2, "{needle}");
        // Facing east the needle turns with north (to the left): 90 degrees counter-clockwise.
        m.view.heading = Vec2::X;
        let east = centre(m.draws(&[], None).iter().find(|d| d.texture == "north").unwrap()) - c;
        assert!((east.x.atan2(-east.y).to_degrees() + 144.6).abs() < 0.2, "{east}");
        // An icon 200 m north (zoom 1) sits on the pin, 75.29 px up, faded to 1 - 140/290.
        m.view.heading = Vec2::NEG_Y;
        m.icons = vec![Icon { page: page("icon", 32), world: [0., 800.] }, Icon { page: page("icon", 32), world: [0., 990.] }];
        let icons: Vec<Draw> = m.draws(&[], None).into_iter().filter(|d| d.texture == "icon").collect();
        assert!((centre(&icons[0]) - (c - Vec2::Y * 71.2811 * 1.0562)).length() < 1e-3);
        assert!((icons[0].multiply[3] - (1. - 140. / 290.)).abs() < 1e-4);
        // 10 m north: 10.562 px up, opaque, upright 32 px.
        assert!((centre(&icons[1]) - (c - Vec2::Y * 10.562)).length() < 1e-3 && icons[1].multiply[3] == 1.);
        // Past 350 m an icon is gone.
        m.icons = vec![Icon { page: page("icon", 32), world: [0., 600.] }];
        assert!(!m.draws(&[], None).iter().any(|d| d.texture == "icon"));
        // The arrow comes after the map and icons; the plate shows the nearest district (none for an empty name).
        let draws = m.draws(&[(2, Vec2::new(0., 990.))], None);
        assert_eq!(draws.iter().rev().nth(1).unwrap().texture, "arrow");
        assert_eq!(draws.last().unwrap().texture, "font");
        assert!(draws.iter().any(|d| d.texture == "skater03"));
        m.update(Vec3::new(990., 0., 0.), Vec2::NEG_Y, 1. / 60.); // a teleport: re-entry latches the empty one
        assert_ne!(m.draws(&[], None).last().unwrap().texture, "font");
    }

    #[test]
    fn skate1_district_plate_latches_at_entry_and_fades_in_like_hud_minimap() {
        let mut m = skate1();
        let d = m.district.as_mut().unwrap();
        d.plates[1] = vec![Vertex { position: [5., 5.], uv: [0., 0.] }; 3];
        d.intro = vec![[0.2, -0.8], [0.6, -0.3], [1., 0.]];
        // (alpha, first vertex) of the plate.
        let plate = |m: &Minimap| {
            let d = m.draws(&[], None).into_iter().find(|d| d.texture == "font").unwrap();
            Vec3::new(d.multiply[3], d.vertices[0].position[0], d.vertices[0].position[1])
        };
        let is = |m: &Minimap, want: [f32; 3]| assert!(plate(m).distance(Vec3::from(want)) < 1e-4, "{} != {want:?}", plate(m));
        // Entering the world at district 0: intro frame 0, then one per 60 Hz frame, then at rest.
        m.update(Vec3::ZERO, Vec2::NEG_Y, 0.);
        is(&m, [0.2, 1., 0.2]);
        m.update(Vec3::new(0.1, 0., 0.), Vec2::NEG_Y, 1. / 60.);
        is(&m, [0.6, 1., 0.7]);
        m.update(Vec3::new(0.2, 0., 0.), Vec2::NEG_Y, 1. / 60.);
        is(&m, [1., 1., 1.]);
        // Riding into district 1's ground keeps the latched name (retail reads it only at world entry).
        let mut at = Vec3::new(0.2, 0., 0.);
        while at.x < 1990. {
            at.x += 50. / 60.;
            m.update(at, Vec2::X, 1. / 60.);
        }
        is(&m, [1., 1., 1.]);
        // A teleport re-enters the world there: district 1, its intro again, timed by 60 Hz frames at any frame rate
        // (at 144 Hz frame 1 starts on the third render after).
        let there = Vec3::new(1000., 0., 500.);
        m.update(there, Vec2::X, 1. / 144.);
        is(&m, [0.2, 5., 4.2]);
        m.update(there, Vec2::X, 1. / 144.);
        m.update(there, Vec2::X, 1. / 144.);
        is(&m, [0.2, 5., 4.2]);
        m.update(there, Vec2::X, 1. / 144.);
        is(&m, [0.6, 5., 4.7]);
        // A teleport within the same district keeps it at rest.
        for _ in 0..10 {
            m.update(there, Vec2::X, 1. / 144.);
        }
        m.update(Vec3::new(1200., 0., 0.), Vec2::X, 1. / 144.);
        is(&m, [1., 5., 5.]);
    }

    /// Offline renders of the installed minimaps (SKATE1_HUD_ROOT / SKATE2_HUD_ROOT) into MINIMAP_RENDER_DIR: the
    /// engine's draws rasterised in software, 3x, over a flat backdrop, for side-by-side checks with retail frames.
    #[test]
    #[ignore = "requires the prepared Skate 1 and Skate 2 HUDs"]
    fn minimap_offline_renders() {
        let out = std::path::PathBuf::from(std::env::var("MINIMAP_RENDER_DIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into()));
        std::fs::create_dir_all(&out).unwrap();
        let load = |var: &str| {
            let root = std::path::PathBuf::from(std::env::var(var).unwrap());
            let m: Minimap = serde_json::from_slice(&std::fs::read(root.join("runtime/minimap.json")).unwrap()).unwrap();
            (root, m)
        };
        // (name, position, facing, settled speed m/s, others, waypoint)
        type Pose = (&'static str, Vec3, Vec2, f32, Vec<(usize, Vec2)>, Option<[f32; 2]>);
        let (root1, mut s1) = load("SKATE1_HUD_ROOT");
        assert!(s1.tiles.as_ref().is_some_and(|t| t.list.len() > 400) && s1.icons.len() > 50);
        let poses1: Vec<Pose> = vec![
            // The pose the Xenia frame (s1_disc.png) best matched: map turned 183 degrees, zoom 0.6.
            ("s1_xenia_pose", Vec3::new(-456.8, 0., -1908.8), Vec2::new(0.0523, 0.9986), 8., Vec::new(), None),
            ("s1_suburbs_station_standing", Vec3::new(-477.5, 0., -2000.), Vec2::NEG_Y, 0., vec![(1, Vec2::new(-460., -2030.))], None),
            ("s1_downtown_east_fast", Vec3::new(-40., 0., 300.), Vec2::X, 10., Vec::new(), None),
            ("s1_old_town_standing", Vec3::new(1163., 0., 140.), Vec2::NEG_Y, 0., Vec::new(), None),
            // The district plate 3 frames into its intro (alpha 176/255, 0.3 px above its rest).
            ("s1_old_town_intro", Vec3::new(1163., 0., 140.), Vec2::NEG_Y, 0., Vec::new(), None),
        ];
        let (root2, mut s2) = load("SKATE2_HUD_ROOT");
        let poses2: Vec<Pose> = vec![
            ("s2_waypoint_near", Vec3::new(155., 0., 424.), Vec2::NEG_Y, 0., Vec::new(), Some([175., 380.])),
            ("s2_waypoint_far_east", Vec3::new(155., 0., 424.), Vec2::X, 0., vec![(1, Vec2::new(140., 410.))], Some([155., -600.])),
        ];
        for (root, m, poses) in [(&root1, &mut s1, poses1), (&root2, &mut s2, poses2)] {
            for (name, at, facing, speed, others, waypoint) in poses {
                m.view = View::default();
                m.update(at, facing, 0.);
                m.view.heading = facing;
                m.view.entered = if name.ends_with("_intro") { 3. } else { f32::MAX };
                if m.zoom {
                    let t = ((speed - ZOOM_FROM) / ZOOM_SPAN).clamp(0., 1.);
                    m.view.zoom = 1. - ZOOM_DEPTH * (1. - (PI * t).cos());
                }
                let draws = m.draws(&others, waypoint);
                let window = [960., 40., 240., 280.];
                raster::render(root, &draws, window, 3, image::Rgba([128, 138, 148, 255])).save(out.join(format!("{name}.png"))).unwrap();
                // The same over nothing, to lay over a screenshot.
                raster::render(root, &draws, window, 3, image::Rgba([0; 4])).save(out.join(format!("{name}_layer.png"))).unwrap();
            }
        }
    }

    mod raster {
        use super::*;
        use image::{Rgba, RgbaImage};

        fn texture(root: &std::path::Path, draw: &Draw) -> RgbaImage {
            let path = root.join(&draw.texture);
            if draw.texture.ends_with(".png") {
                return image::open(&path).unwrap().into_rgba8();
            }
            RgbaImage::from_raw(draw.size[0], draw.size[1], std::fs::read(&path).unwrap()).unwrap()
        }
        fn sample(t: &RgbaImage, uv: Vec2) -> Vec4 {
            let (w, h) = (t.width() as f32, t.height() as f32);
            let (x, y) = (uv.x * w - 0.5, uv.y * h - 0.5);
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let px = |x: f32, y: f32| {
                let p = t.get_pixel(x.clamp(0., w - 1.) as u32, y.clamp(0., h - 1.) as u32).0;
                Vec4::new(p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32) / 255.
            };
            px(x0, y0).lerp(px(x0 + 1., y0), fx).lerp(px(x0, y0 + 1.).lerp(px(x0 + 1., y0 + 1.), fx), fy)
        }
        /// `draws` (stage px) inside `crop` (x, y, w, h), `k` output px per stage px, alpha-blended in order over `backdrop`.
        pub(super) fn render(root: &std::path::Path, draws: &[Draw], crop: [f32; 4], k: u32, backdrop: Rgba<u8>) -> RgbaImage {
            let [cx, cy, cw, ch] = crop;
            let (w, h) = ((cw as u32) * k, (ch as u32) * k);
            let mut out = RgbaImage::from_pixel(w, h, backdrop);
            let mut cache: HashMap<String, RgbaImage> = HashMap::new();
            for d in draws {
                let t = cache.entry(d.texture.clone()).or_insert_with(|| texture(root, d));
                for tri in d.vertices.chunks(3) {
                    let p: Vec<Vec2> = tri.iter().map(|v| (Vec2::from(v.position) - Vec2::new(cx, cy)) * k as f32).collect();
                    let area = (p[1] - p[0]).perp_dot(p[2] - p[0]);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    let lo = p[0].min(p[1]).min(p[2]).max(Vec2::ZERO).floor();
                    let hi = p[0].max(p[1]).max(p[2]).min(Vec2::new(w as f32, h as f32)).ceil();
                    for y in lo.y as u32..hi.y as u32 {
                        for x in lo.x as u32..hi.x as u32 {
                            let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                            let b = [(p[2] - p[1]).perp_dot(q - p[1]) / area, (p[0] - p[2]).perp_dot(q - p[2]) / area];
                            let b = [b[0], b[1], 1. - b[0] - b[1]];
                            if b.iter().any(|&v| v < -1e-4) {
                                continue;
                            }
                            let uv = Vec2::from(tri[0].uv) * b[0] + Vec2::from(tri[1].uv) * b[1] + Vec2::from(tri[2].uv) * b[2];
                            let c = sample(t, uv) * Vec4::from(d.multiply) + Vec4::from(d.add);
                            // Straight-alpha "over".
                            let dst = out.get_pixel_mut(x, y);
                            let (a, below) = (c.w.clamp(0., 1.), dst.0[3] as f32 / 255.);
                            let total = a + below * (1. - a);
                            if total > 0. {
                                for i in 0..3 {
                                    let value = c[i].clamp(0., 1.) * 255. * a + dst.0[i] as f32 * below * (1. - a);
                                    dst.0[i] = (value / total).round() as u8;
                                }
                                dst.0[3] = (total * 255.).round() as u8;
                            }
                        }
                    }
                }
            }
            out
        }
    }
}
