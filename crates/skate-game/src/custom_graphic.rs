//! Edit Skater graphics, after Skate 3 and skate.ea.com's Graphics Creator (2010-2013):
//! stock brand and team logos, or a creator design, at one of the four Graphic Locations of a
//! customizable tee, hoodie or windbreaker (front, back, sleeves), a customizable hat's
//! front and the customizable deck's bottom, each moved, sized and turned with Graphic
//! Adjust. Tattoos use the same adjust.
//!
//! Other players' designs are data, never images: a bounded list of built-in shapes,
//! stock logos and short text, decoded strictly and drawn here at a small fixed size,
//! a few per frame. Settings hide everyone's designs; the Player List hides one player's.
use base64::Engine;
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};
use tiny_skia::{FillRule, FilterQuality, Paint, Path as Shape, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};

/// Graphic Location: profile key, the model's stamp rectangle, menu label.
pub(crate) const ZONES: [(&str, &str, &str); 6] = [
    ("front", "StampUVConstraintQ1", "ID_CAC_CUSTOMGRAPHIC_SHIRT_FRONT"),
    ("back", "StampUVConstraintQ2", "ID_CAC_CUSTOMGRAPHIC_SHIRT_BACK"),
    ("left", "StampUVConstraintQ3", "ID_CAC_CUSTOMGRAPHIC_SHIRT_LEFTARM"),
    ("right", "StampUVConstraintQ4", "ID_CAC_CUSTOMGRAPHIC_SHIRT_RIGHTARM"),
    ("deck", "", "ID_CAC_BOARDS_TITLE"),
    ("hat", "", "ID_CAC_HATS_TITLE"),
];
/// A top's Graphic Locations. Retail keeps one graphic and one adjust per category (0x82536c40:
/// shirt, upper tattoo, lower tattoo, board, hat), so a top wears one graphic at one of these.
pub(crate) const SHIRT: [&str; 4] = ["front", "back", "left", "right"];
/// The customizable deck's bottom in its secondary UVs ([top, bottom, left, right], V up),
/// measured from the retail mesh (every other face of the deck collapses to one UV point).
const DECK: [f32; 4] = [0.98, 0.515, 0.19, 0.305];
/// A logoable hat's front (retail HatGraphicAdjust). cas_db leaves hats' rectangle flags
/// empty, so these are measured the same way: the Customizable Hat's crown front panel
/// (New Era, every wear style, both genders; the brim top below it in the same quadrant is
/// left out so a logo sits on the crown), then the other logoable hats' stamp faces.
const HAT: [f32; 4] = [0.972, 0.714, 0.067, 0.465];
const HATS: [(&str, [f32; 4]); 6] = [
    ("d7f3d08dde36bc61", [0.965, 0.509, 0.014, 0.468]), // Beanie Hat (DLC)
    ("bb98f8bbcc3da70d", [0.965, 0.509, 0.014, 0.468]),
    ("d7f3d08dde36bc67", [0.927, 0.505, 0.001, 0.489]), // Beanie Fluffball (DLC)
    ("bb98f8bbcc3da70a", [0.94, 0.505, 0.001, 0.472]),
    ("c2ce7955e7ba5142", [0.857, 0.582, 0.003, 0.495]), // Helmet
    ("ed034a24374c8d35", [0.809, 0.566, 0.008, 0.49]),
];
/// The Customizable Windbreaker (closed, open): an IsLogoable material and the tees' four-
/// quadrant stamp layout, but empty StampUVConstraintQ1-4 in cas_db. Its stamp faces,
/// measured from the retail mesh: front, back, left and right sleeve.
const WINDBREAKER: [(&str, [[f32; 4]; 4]); 2] = [
    ("c620cb8d10a788ec", [[0.992, 0.507, 0.101, 0.398], [0.998, 0.516, 0.607, 0.896], [0.488, 0.009, 0.107, 0.394], [0.49, 0.008, 0.605, 0.893]]),
    ("c620cb8d10a788ef", [[0.992, 0.509, 0.111, 0.406], [0.991, 0.512, 0.608, 0.891], [0.488, 0.009, 0.106, 0.393], [0.491, 0.009, 0.609, 0.896]]),
];

/// A retail rectangle flag ("top,bottom,left,right").
pub(crate) fn rectangle(flag: &str) -> Option<[f32; 4]> {
    let v: Vec<f32> = flag.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    <[f32; 4]>::try_from(v).ok()
}
/// The Graphic Locations a worn model offers and their rectangles, in shader zone order:
/// a customizable (IsLogoable) top's front, back and sleeves, the customizable deck's
/// bottom, a logoable hat's front. The Edit Skater rows and the stamped material both
/// come from here; a location without a rectangle (a top with no stamp layout) is not offered.
pub(crate) fn locations(id: &str, part: &crate::customiser_parts::Part, logoable: bool) -> Vec<(&'static str, [f32; 4])> {
    let zones = match part.slot.as_str() {
        _ if !logoable => &ZONES[..0],
        "OuterTorso" if !part.flag("TopType").is_empty() => &ZONES[..4],
        "SkateBoard" => &ZONES[4..5],
        "Hat" => &ZONES[5..],
        _ => &ZONES[..0],
    };
    let measured = |i: usize| WINDBREAKER.iter().find(|w| w.0 == id).map(|w| w.1[i]);
    let rect = |i: usize, zone: &str, key: &str| match zone {
        "deck" => Some(DECK),
        "hat" => Some(HATS.iter().find(|h| h.0 == id).map_or(HAT, |h| h.1)),
        _ => rectangle(part.flag(key)).or_else(|| measured(i)),
    };
    zones.iter().enumerate().filter_map(|(i, (zone, key, _))| Some((*zone, rect(i, zone, key)?))).collect()
}

/// Graphic Adjust: (profile key, low, high, step, initial). Moves are secondary UVs from the
/// retail start (right, up positive; retail keeps the offset itself), size a factor on the start
/// scale, angle degrees counter-clockwise (V up). Retail has no flip; an older profile's "flip"
/// is dropped. Retail's fit (`fit`) bounds a local adjust; these ranges bound peer data (size 8:
/// the fit lets the smallest stock art box grow to 7.6 on a sleeve).
pub(crate) const ADJUST: [(&str, f64, f64, f64, f64); 4] =
    [("x", -0.5, 0.5, 0.025, 0.), ("y", -0.5, 0.5, 0.025, 0.), ("size", 0.25, 8., 0.05, 1.), ("angle", -180., 180., 5., 0.)];
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Adjust {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub angle: f32,
}
impl Default for Adjust {
    fn default() -> Self {
        Self { x: 0., y: 0., size: 1., angle: 0. }
    }
}
impl Adjust {
    /// Missing or out-of-range values (peer data included) take the nearest valid one.
    pub fn of(v: &Value) -> Self {
        let f = |i: usize| {
            let (key, lo, hi, _, initial) = ADJUST[i];
            v[key].as_f64().unwrap_or(initial).clamp(lo, hi) as f32
        };
        Self { x: f(0), y: f(1), size: f(2), angle: f(3) }
    }
    fn write(self, out: &mut Map<String, Value>) {
        for (key, value, initial) in [("x", self.x, 0.), ("y", self.y, 0.), ("size", self.size, 1.), ("angle", self.angle, 0.)] {
            if value != initial {
                out.insert(key.into(), json!((value as f64 * 1000.).round() / 1000.));
            }
        }
    }
}

/// Retail's Graphic Adjust starts (TU3 table 0x830442b8, copied in by 0x82536fa0 whenever a
/// location or tattoo spot is chosen): [turn, scale, offset u, offset v] for a male skater, then
/// a female one (0x82ddc438's gender picks the column). The start centre is 0.5 minus the offset,
/// in secondary UVs, V down. Scale s shows the 256x128 graphic 1/s of the UVs across and 1/(2s)
/// down (vertex shader 0x82c2e3a8, skin and cloth alike), so its texels stay square.
const START: [[[f32; 4]; 2]; 12] = [
    [[0., 5., 0.25, 0.25]; 2],                          // shirt front
    [[0., 5., 0.25, 0.25], [0., 5., 0.25, 0.21]],       // chest tattoo
    [[1.571, 7., 0.25, 0.25]; 2],                       // deck
    [[0., 5., 0.25, 0.32]; 2],                          // hat
    [[0., 5., -0.25, 0.25]; 2],                         // shirt back
    [[0., 5., -0.25, 0.3], [0., 5., -0.25, 0.1]],       // back tattoo
    [[0., 7., 0.25, -0.15]; 2],                         // left sleeve
    [[0., 7., 0.234, -0.15], [0., 7., 0.25, -0.15]],    // left arm tattoo
    [[0., 10., 0.17, -0.35], [0., 10., 0.19, -0.35]],   // left leg tattoo
    [[0., 7., -0.234, -0.15], [0., 7., -0.25, -0.15]],  // right sleeve
    [[0., 7., -0.25, -0.15]; 2],                        // right arm tattoo
    [[0., 10., -0.17, -0.35], [0., 10., -0.19, -0.35]], // right leg tattoo
];
/// Start entry per Graphic Location, in ZONES order (0x825fbae8's shirt, board and hat tables).
const ENTRY: [usize; 6] = [0, 4, 6, 9, 2, 3];
/// Retail's smallest graphic (0x82c2e698 stops shrinking at this scale).
const MAX_SCALE: f32 = 15.;
/// The art box of a design: the whole canvas.
pub(crate) const CANVAS: [f32; 4] = [1., 0., 0., 1.];

/// A Graphic Location's start entry.
pub(crate) fn entry(zone: &str) -> usize {
    ENTRY[ZONES.iter().position(|z| z.0 == zone).unwrap_or(0)]
}
/// A tattoo spot's start entry and the skin model's rectangle flag for it: the profile's
/// "Arm" sides 0-3 are the left arm, right arm, chest, back; "Leg" 0-1 the left and right leg.
pub(crate) fn tattoo(slot: &str, side: u64) -> Option<(usize, &'static str)> {
    let q = |e: usize| ["StampUVConstraintQ1", "StampUVConstraintQ2", "StampUVConstraintQ3", "StampUVConstraintQ4"][quadrant(e)];
    let e = match (slot, side) {
        ("Arm", 0) => 7,
        ("Arm", 1) => 10,
        ("Arm", 2) => 1,
        ("Arm", 3) => 5,
        ("Leg", 0) => 8,
        ("Leg", 1) => 11,
        _ => return None,
    };
    Some((e, q(e)))
}
/// Entries 0-3 fit in quadrant Q1, 4-5 Q2, 6-8 Q3, 9-11 Q4 (0x82536c40).
fn quadrant(entry: usize) -> usize {
    [0, 0, 0, 0, 1, 1, 2, 2, 2, 3, 3, 3][entry.min(11)]
}
/// Retail's rectangle for a start entry when the model's flag is empty: its whole quadrant
/// of the secondary UVs ([top, bottom, left, right], V up).
pub(crate) fn quarter(entry: usize) -> [f32; 4] {
    [[1., 0.5, 0., 0.5], [1., 0.5, 0.5, 1.], [0.5, 0., 0., 0.5], [0.5, 0., 0.5, 1.]][quadrant(entry)]
}

/// Retail's stamp state for an Adjust: turn, scale, centre (secondary UVs, V down).
fn state(entry: usize, female: bool, adjust: Adjust) -> (f32, f32, Vec2) {
    let [turn, start, u, v] = START[entry.min(11)][usize::from(female)];
    (turn + adjust.angle.to_radians(), start / adjust.size, Vec2::new(0.5 - u + adjust.x, 0.5 - v - adjust.y))
}

/// A logo, design or tattoo as retail draws it, its start entry moved, scaled and turned by
/// Adjust, exactly as stored (no fit at draw time: a wide logo's start overflows a hoodie
/// front, as on the 360): the two rows texture = 0.5 + diag(s, 2s) R(turn) (uv - centre).
pub(crate) fn stamp(entry: usize, female: bool, adjust: Adjust) -> [Vec4; 2] {
    let (turn, s, centre) = state(entry, female, adjust);
    let (sin, cos) = turn.sin_cos();
    let (u, v) = (Vec2::new(s * cos, -s * sin), Vec2::new(2. * s * sin, 2. * s * cos));
    [Vec4::new(u.x, u.y, 0.5 - u.dot(centre), 0.), Vec4::new(v.x, v.y, 0.5 - v.dot(centre), 0.)]
}

/// Retail's fit after each Graphic Adjust step (0x82c2e698): if the art box (`art`, its
/// StampBorderConstraint [top, bottom, left, right], read unflipped as texture UVs) leaves the
/// zone (`target`, [top, bottom, left, right] V up), each axis that overflows on one side only
/// slides back by the first such corner's overshoot plus 0.001; then, while any corner is still
/// out, the scale grows by 0.1% (0x82c2ebd8 tests, edges inclusive). Returns the Adjust to store.
pub(crate) fn fit(entry: usize, female: bool, target: [f32; 4], art: [f32; 4], adjust: Adjust) -> Adjust {
    let (turn, scale, mut centre) = state(entry, female, adjust);
    let mut s = scale.min(MAX_SCALE);
    let (min, max) = (Vec2::new(target[2], 1. - target[0]), Vec2::new(target[3], 1. - target[1]));
    let (sin, cos) = turn.sin_cos();
    // Corners in retail's order: left-bottom, left-top, right-bottom, right-top.
    let corners = |s: f32, centre: Vec2| {
        [(art[2], art[1]), (art[2], art[0]), (art[3], art[1]), (art[3], art[0])].map(|(u, v)| {
            let (p, q) = ((u - 0.5) / s, (v - 0.5) / (2. * s));
            centre + Vec2::new(cos * p + sin * q, cos * q - sin * p)
        })
    };
    let fits = |s: f32, centre: Vec2| corners(s, centre).iter().all(|c| c.cmpge(min).all() && c.cmple(max).all());
    if s == scale && fits(s, centre) {
        return adjust;
    }
    for axis in 0..2 {
        let out: Vec<f32> = corners(s, centre).iter().map(|c| (c[axis] - min[axis]).min(0.) + (c[axis] - max[axis]).max(0.)).collect();
        if out.iter().any(|&d| d < 0.) && out.iter().any(|&d| d > 0.) {
            continue;
        }
        if let Some(&d) = out.iter().find(|&&d| d != 0.) {
            centre[axis] -= d + 0.001f32.copysign(d);
        }
    }
    // Retail keeps s and the centre; we store start / s and the move, so test what they read back as.
    let [_, start, u, v] = START[entry.min(11)][usize::from(female)];
    let home = Vec2::new(0.5 - u, 0.5 - v);
    let shown = |s: f32, centre: Vec2| fits(start / (start / s), home + (centre - home));
    while !shown(s, centre) && s < MAX_SCALE {
        s = ((f64::from(s) * 1.001) as f32).min(MAX_SCALE);
    }
    // ponytail: retail's 0.005 steps keep the centre well inside the zone, so its growth ends
    // before 15; an older profile or another garment's zone can leave it at or past an edge, where
    // retail would grow past 15 or forever: there its smallest slides in, keeping the 0.001 margin.
    if !shown(s, centre) {
        let offsets = corners(s, Vec2::ZERO);
        let lo = min - offsets.into_iter().reduce(Vec2::min).unwrap_or_default() + 0.001;
        let hi = max - offsets.into_iter().reduce(Vec2::max).unwrap_or_default() - 0.001;
        for axis in 0..2 {
            centre[axis] = if lo[axis] <= hi[axis] { centre[axis].clamp(lo[axis], hi[axis]) } else { (min[axis] + max[axis]) / 2. };
        }
    }
    let moved = centre - home;
    Adjust { x: moved.x, y: -moved.y, size: start / s, ..adjust }
}

// ---- Graphic Creator designs ----

/// Layers on one side (version 3; versions 1 and 2 held `OLD_LAYERS`).
pub(crate) const MAX_LAYERS: usize = 64;
const OLD_LAYERS: usize = 32;
pub(crate) const MAX_TEXT: usize = 24;
const LAYER_BYTES: usize = 16;
/// "G", version 3, background RGBA, layer count (version 1: no background).
const HEADER: usize = 7;
const VERSION: u8 = 3;
/// The longest a layer's extra bytes get: text (length byte + letters).
const MAX_BYTES: usize = HEADER + MAX_LAYERS * (LAYER_BYTES + 1 + MAX_TEXT);
/// Layer centre limit and size range, thousandths of a canvas unit. A side's canvas is as tall and
/// wide as its rectangle, its LONG side 4 units (`canvas`), the centre the origin. Version 1 and 2
/// designs were drawn on a 2:1 canvas 4 units wide (the same rule), over a stamp (`upgrade`).
pub(crate) const REACH: i16 = 2500;
pub(crate) const SIZES: (u16, u16) = (40, 4000);
/// A side texture's width: 256 px on other players, the local skater's and the creator's 512.
/// Tall sides (the tee front, the deck) are as tall as their aspect asks, at most four times that (2048 local, 1024 for others).
pub(crate) const REMOTE_SIZE: u32 = 256;
pub(crate) const LOCAL_SIZE: u32 = 512;

/// The shape library: our own shapes (the website's was never archived). A `Shape(u8)` index is
/// one byte: the first fifteen are the original simple ones, then `graphic_shapes`' hand-made
/// pictures (about 150), and indices `SHAPES.len()..180` stay reserved for any added later; a design
/// naming an index nothing draws is rejected.
pub(crate) const SHAPES: [&str; OLD_SHAPES + crate::graphic_shapes::COUNT] = {
    const FIRST: [&str; OLD_SHAPES] = [
        "Square", "Rounded", "Circle", "Ring", "Triangle", "Diamond", "Hexagon", "Star", "Burst", "Heart", "Bolt", "Arrow",
        "Chevron", "Cross", "Stripe",
    ];
    let mut names = [""; OLD_SHAPES + crate::graphic_shapes::COUNT];
    let mut i = 0;
    while i < names.len() {
        names[i] = if i < OLD_SHAPES { FIRST[i] } else { crate::graphic_shapes::NAMES[i - OLD_SHAPES] };
        i += 1;
    }
    names
};
/// Shapes version 1 and 2 designs may name, and the ones drawn with the even-odd rule.
const OLD_SHAPES: usize = 15;
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Art {
    Shape(u8),
    /// A stock graphic (library material id, 16 hex digits).
    Logo(u64),
    /// A tattoo or decal from the installed library (its 16 hex digit id). Every player draws it
    /// from their own files; an id their install lacks draws nothing.
    GameArt(u64),
    /// Printable ASCII, 1 to MAX_TEXT characters.
    Text(String),
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layer {
    pub art: Art,
    /// RGBA; a logo keeps its own colours and uses the alpha as opacity.
    pub colour: [u8; 4],
    /// Centre, thousandths of a canvas unit from the middle (y down).
    pub x: i16,
    pub y: i16,
    /// Width and height, thousandths of a canvas unit.
    pub w: u16,
    pub h: u16,
    /// Degrees, clockwise.
    pub angle: i16,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Moves, scales and recolours with the layer below (flags bit 2). Never set on the bottom layer.
    pub grouped: bool,
}
impl Layer {
    pub fn new(art: Art, colour: [u8; 4]) -> Self {
        let mut layer = Self { art, colour, x: 0, y: 0, w: 1000, h: 1000, angle: 0, flip_x: false, flip_y: false, grouped: false };
        match layer.art {
            Art::Text(_) => {
                layer.h = 400;
                layer.fit_text();
            }
            // Stock graphics are 2:1, half the canvas.
            Art::Logo(_) | Art::GameArt(_) => layer.w = 2000,
            Art::Shape(_) => {}
        }
        layer
    }
    /// Text keeps its letters' shape: the built-in face's characters are half as wide as tall.
    pub fn fit_text(&mut self) {
        if let Art::Text(t) = &self.art {
            let natural = self.h as usize * t.len().max(1) / 2;
            self.w = natural.clamp(SIZES.0 as usize, SIZES.1 as usize) as u16;
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Doc {
    /// Bottom first.
    pub layers: Vec<Layer>,
    /// The website's background colour and opacity; transparent is none.
    pub background: [u8; 4],
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let s = self.bytes.get(self.at..self.at + n).ok_or("Graphic truncated")?;
        self.at += n;
        Ok(s)
    }
}
fn printable(text: &str) -> bool {
    (1..=MAX_TEXT).contains(&text.len()) && text.bytes().all(|b| (0x20..0x7f).contains(&b))
}
impl Doc {
    pub fn encode(&self) -> String {
        let mut b = vec![b'G', VERSION];
        b.extend(self.background);
        b.push(self.layers.len() as u8);
        for l in &self.layers {
            let (kind, extra) = match &l.art {
                Art::Shape(s) => (0, vec![*s]),
                Art::Logo(id) => (1, id.to_le_bytes().to_vec()),
                Art::Text(t) => (2, [&[t.len() as u8][..], t.as_bytes()].concat()),
                Art::GameArt(id) => (3, id.to_le_bytes().to_vec()),
            };
            b.push(kind);
            b.extend(l.colour);
            for v in [l.x as u16, l.y as u16, l.w, l.h, l.angle as u16] {
                b.extend(v.to_le_bytes());
            }
            b.push(u8::from(l.flip_x) | u8::from(l.flip_y) << 1 | u8::from(l.grouped) << 2);
            b.extend(extra);
        }
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(b)
    }
    /// Strict version 3: anything unknown, out of range, oversized or trailing rejects the design.
    pub fn decode(text: &str) -> Result<Self, &'static str> {
        match parse(text)? {
            (VERSION, doc) => Ok(doc),
            _ => Err("Unsupported graphic"),
        }
    }
    /// A version 1 or 2 design (its layers sit on the old 2:1 canvas): `upgrade` it before wearing.
    pub fn decode_old(text: &str) -> Result<Self, &'static str> {
        match parse(text)? {
            (VERSION, _) => Err("Unsupported graphic"),
            (_, doc) => Ok(doc),
        }
    }
    /// A design of any version, its layers as stored (for a tile of an older design).
    pub fn decode_any(text: &str) -> Result<Self, &'static str> {
        parse(text).map(|(_, doc)| doc)
    }
    /// Library images the design draws: stock logos and game art (ids in one library namespace).
    pub fn images(&self) -> impl Iterator<Item = u64> + '_ {
        self.layers.iter().filter_map(|l| match l.art {
            Art::Logo(id) | Art::GameArt(id) => Some(id),
            _ => None,
        })
    }
}
/// A design of any version that decodes strictly.
pub(crate) fn valid(text: &str) -> bool {
    parse(text).is_ok()
}
/// Version 1 and 2 designs have at most `OLD_LAYERS` layers and the first `OLD_SHAPES` shapes and
/// no game art; version 3 the full set. The layer bytes are the same in every version.
fn parse(text: &str) -> Result<(u8, Doc), &'static str> {
    if text.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("Graphic too large");
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(text).map_err(|_| "Graphic unreadable")?;
    let mut reader = Reader { bytes: &bytes, at: 0 };
    let mut take = |n| reader.take(n);
    let version = match take(2)? {
        [b'G', v @ 1..=VERSION] => *v,
        _ => return Err("Unsupported graphic"),
    };
    let background = if version == 1 { [0; 4] } else { <[u8; 4]>::try_from(take(4)?).unwrap_or_default() };
    let (max_layers, shapes) = if version == VERSION { (MAX_LAYERS, SHAPES.len()) } else { (OLD_LAYERS, OLD_SHAPES) };
    let count = take(1)?[0];
    if count as usize > max_layers {
        return Err("Unsupported graphic");
    }
    let mut layers = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let b = take(LAYER_BYTES)?;
        let word = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let (x, y, w, h, angle) = (word(5) as i16, word(7) as i16, word(9), word(11), word(13) as i16);
        // unsigned_abs: i16::MIN has no i16 magnitude (abs() overflows; wrapped, it would pass the check).
        if x.unsigned_abs() > REACH.unsigned_abs() || y.unsigned_abs() > REACH.unsigned_abs() || !(SIZES.0..=SIZES.1).contains(&w) || !(SIZES.0..=SIZES.1).contains(&h)
            || !(-180..=180).contains(&angle) || b[15] > if version == VERSION { 7 } else { 3 }
        {
            return Err("Graphic layer out of range");
        }
        let (kind, colour, flags) = (b[0], [b[1], b[2], b[3], b[4]], b[15]);
        let art = match kind {
            0 => {
                let s = take(1)?[0];
                if s as usize >= shapes {
                    return Err("Unknown shape");
                }
                Art::Shape(s)
            }
            1 => Art::Logo(u64::from_le_bytes(take(8)?.try_into().unwrap())),
            2 => {
                let n = take(1)?[0] as usize;
                let text = String::from_utf8(take(n)?.to_vec()).map_err(|_| "Graphic text unreadable")?;
                if !printable(&text) {
                    return Err("Graphic text not allowed");
                }
                Art::Text(text)
            }
            3 if version == VERSION => Art::GameArt(u64::from_le_bytes(take(8)?.try_into().unwrap())),
            _ => return Err("Unknown graphic layer"),
        };
        layers.push(Layer { art, colour, x, y, w, h, angle, flip_x: flags & 1 != 0, flip_y: flags & 2 != 0, grouped: flags & 4 != 0 });
    }
    if reader.at != bytes.len() {
        return Err("Graphic has trailing data");
    }
    Ok((version, Doc { layers, background }))
}

/// A side's canvas in pixels: `size` wide and as tall as the side's rectangle `[top, bottom, left,
/// right]` is (so a texel is square), but at most `4 * size` tall (the width gives way).
pub(crate) fn canvas(rect: [f32; 4], size: u32) -> (u32, u32) {
    let [top, bottom, left, right] = rect;
    let aspect = (right - left) / (top - bottom);
    if !(aspect.is_finite() && aspect > 0.) {
        return (size, size / 2);
    }
    let (mut w, mut h) = (size as f32, size as f32 / aspect);
    if h > 4. * size as f32 {
        (w, h) = (4. * size as f32 * aspect, 4. * size as f32);
    }
    (w.round().max(1.) as u32, h.round().max(1.) as u32)
}
/// The stamp rows that lay a side's whole canvas exactly over its rectangle `[top, bottom, left,
/// right]` (V up) in the secondary UVs: `tex = (uv - corner) / extent`, V down. No retail start.
pub(crate) fn side_stamp(rect: [f32; 4]) -> [Vec4; 2] {
    let [top, bottom, left, right] = rect;
    let (w, h) = (right - left, top - bottom);
    [Vec4::new(1. / w, 0., -left / w, 0.), Vec4::new(0., 1. / h, -(1. - top) / h, 0.)]
}
/// A version 1 or 2 design, drawn on the 2:1 canvas that retail's stamp (`entry`, `female`,
/// `adjust`) showed over the side `rect`, as a version 3 design on that side's own canvas: each
/// layer where, how big and how turned the stamp showed it, so the side looks the same (what the
/// stamp showed past the rectangle is cut, as the side now cuts it). The old background colour
/// covered the stamp's canvas only, so it becomes the bottom layer. At a big adjust the stamp's canvas
/// is far larger than the side and a layer can outgrow the 4 unit limit: a layer that does is drawn at
/// the limit with its shape kept (not squashed), and a background that covers the whole side becomes
/// one the size of the side.
pub(crate) fn upgrade(old: &Doc, entry: usize, female: bool, adjust: Adjust, rect: [f32; 4]) -> Doc {
    let [r0, r1] = stamp(entry, female, adjust);
    let (a, b) = (Mat2::from_cols_array(&[r0.x, r1.x, r0.y, r1.y]), Vec2::new(r0.z, r1.z));
    let inverse = a.inverse();
    // Old canvas units (y down, 4 x 2) to texture to garment UVs (V down).
    let to_uv = |c: Vec2| inverse * (Vec2::new(0.5 + c.x / 4., 0.5 + c.y / 2.) - b);
    let axis = inverse * Vec2::new(0.25, 0.);
    let (old_unit, turn) = (axis.length(), axis.y.atan2(axis.x).to_degrees());
    let [top, bottom, left, right] = rect;
    let (width, height) = (right - left, top - bottom);
    let (centre, unit) = (Vec2::new((left + right) / 2., 1. - (top + bottom) / 2.), width.max(height) / 4.);
    let (half_w, half_h) = (width / unit / 2., height / unit / 2.);
    let ground = (old.background[3] > 0).then(|| Layer { w: 4000, h: 2000, ..Layer::new(Art::Shape(0), old.background) });
    let layers = ground.iter().map(|l| (l, true)).chain(old.layers.iter().map(|l| (l, false))).filter_map(|(l, is_ground)| {
        let at = (to_uv(Vec2::new(l.x as f32, l.y as f32) / 1000.) - centre) / unit;
        let scale = old_unit / unit;
        let (mut w, mut h) = (l.w as f32 * scale, l.h as f32 * scale);
        let reach = 0.5 * w.hypot(h) / 1000.;
        if at.x.abs() - reach > half_w || at.y.abs() - reach > half_h {
            return None; // wholly off the side
        }
        let size = |v: f32| v.round().clamp(SIZES.0 as f32, SIZES.1 as f32) as u16;
        let place = |v: f32| (v * 1000.).round().clamp(-f32::from(REACH), f32::from(REACH)) as i16;
        let degrees = (l.angle as f32 + turn + 180.).rem_euclid(360.) - 180.;
        if is_ground {
            // The side's corners, in the background's own frame: all inside means it covers the side.
            let into_layer = Mat2::from_angle(-degrees.to_radians());
            let covers = [(-half_w, -half_h), (half_w, -half_h), (-half_w, half_h), (half_w, half_h)].iter().all(|&(x, y)| {
                let p = into_layer * (Vec2::new(x, y) - at) * 1000.;
                p.x.abs() <= w / 2. && p.y.abs() <= h / 2.
            });
            if covers {
                return Some(Layer { x: 0, y: 0, w: size(half_w * 2000.), h: size(half_h * 2000.), angle: 0, ..l.clone() });
            }
        } else {
            let keep = (SIZES.1 as f32 / w.max(h)).min(1.);
            (w, h) = (w * keep, h * keep);
        }
        Some(Layer { x: place(at.x), y: place(at.y), w: size(w), h: size(h), angle: degrees.round() as i16, ..l.clone() })
    });
    Doc { layers: layers.collect(), background: [0; 4] }
}
pub(crate) fn logo_key(id: &str) -> Option<u64> {
    (id.len() == 16).then(|| u64::from_str_radix(id, 16).ok()).flatten()
}
pub(crate) fn logo_id(key: u64) -> String {
    format!("{key:016x}")
}

fn polygon(b: &mut PathBuilder, points: &[(f32, f32)]) {
    b.move_to(points[0].0, points[0].1);
    for p in &points[1..] {
        b.line_to(p.0, p.1);
    }
    b.close();
}
/// A star or polygon: `n` points at radius 0.5 (alternating with `inner` when given), the first straight up.
fn radial(n: usize, inner: Option<f32>) -> Vec<(f32, f32)> {
    let count = if inner.is_some() { n * 2 } else { n };
    (0..count)
        .map(|i| {
            let r = if i % 2 == 1 { inner.unwrap_or(0.5) } else { 0.5 };
            let a = std::f32::consts::TAU * i as f32 / count as f32 - std::f32::consts::FRAC_PI_2;
            (r * a.cos(), r * a.sin())
        })
        .collect()
}
/// A library shape in the unit box (-0.5..0.5, y down).
fn shape(index: u8) -> Option<Shape> {
    let mut b = PathBuilder::new();
    let unit = Rect::from_xywh(-0.5, -0.5, 1., 1.)?;
    match index {
        0 => b.push_rect(unit),
        1 => {
            let (r, k) = (0.18, 0.18 * 0.45);
            b.move_to(-0.5 + r, -0.5);
            b.line_to(0.5 - r, -0.5);
            b.cubic_to(0.5 - k, -0.5, 0.5, -0.5 + k, 0.5, -0.5 + r);
            b.line_to(0.5, 0.5 - r);
            b.cubic_to(0.5, 0.5 - k, 0.5 - k, 0.5, 0.5 - r, 0.5);
            b.line_to(-0.5 + r, 0.5);
            b.cubic_to(-0.5 + k, 0.5, -0.5, 0.5 - k, -0.5, 0.5 - r);
            b.line_to(-0.5, -0.5 + r);
            b.cubic_to(-0.5, -0.5 + k, -0.5 + k, -0.5, -0.5 + r, -0.5);
            b.close();
        }
        2 => b.push_oval(unit),
        3 => {
            b.push_oval(unit);
            b.push_oval(Rect::from_xywh(-0.32, -0.32, 0.64, 0.64)?);
        }
        4 => polygon(&mut b, &[(0., -0.5), (0.5, 0.5), (-0.5, 0.5)]),
        5 => polygon(&mut b, &[(0., -0.5), (0.5, 0.), (0., 0.5), (-0.5, 0.)]),
        6 => polygon(&mut b, &radial(6, None)),
        7 => polygon(&mut b, &radial(5, Some(0.2))),
        8 => polygon(&mut b, &radial(12, Some(0.34))),
        9 => {
            b.move_to(0., 0.5);
            b.cubic_to(-0.2, 0.32, -0.5, 0.1, -0.5, -0.18);
            b.cubic_to(-0.5, -0.4, -0.32, -0.5, -0.24, -0.5);
            b.cubic_to(-0.1, -0.5, 0., -0.4, 0., -0.28);
            b.cubic_to(0., -0.4, 0.1, -0.5, 0.24, -0.5);
            b.cubic_to(0.32, -0.5, 0.5, -0.4, 0.5, -0.18);
            b.cubic_to(0.5, 0.1, 0.2, 0.32, 0., 0.5);
            b.close();
        }
        10 => polygon(&mut b, &[(0.12, -0.5), (-0.3, 0.06), (-0.02, 0.06), (-0.12, 0.5), (0.3, -0.06), (0.02, -0.06)]),
        11 => polygon(&mut b, &[(-0.5, -0.14), (0.1, -0.14), (0.1, -0.4), (0.5, 0.), (0.1, 0.4), (0.1, 0.14), (-0.5, 0.14)]),
        12 => polygon(&mut b, &[(-0.5, -0.3), (0., 0.15), (0.5, -0.3), (0.5, 0.05), (0., 0.5), (-0.5, 0.05)]),
        13 => polygon(&mut b, &[
            (-0.15, -0.5), (0.15, -0.5), (0.15, -0.15), (0.5, -0.15), (0.5, 0.15), (0.15, 0.15),
            (0.15, 0.5), (-0.15, 0.5), (-0.15, 0.15), (-0.5, 0.15), (-0.5, -0.15), (-0.15, -0.15),
        ]),
        14 => b.push_rect(Rect::from_xywh(-0.5, -0.12, 1., 0.24)?),
        _ => return crate::graphic_shapes::path(usize::from(index).checked_sub(OLD_SHAPES)?),
    }
    b.finish()
}
struct Outline<'a> {
    path: &'a mut PathBuilder,
    pen: f32,
    sx: f32,
    sy: f32,
    top: f32,
}
impl Outline<'_> {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        ((self.pen + x) * self.sx - 0.5, (self.top - y) * self.sy - 0.5)
    }
}
impl ttf_parser::OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.move_to(p.0, p.1);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.line_to(p.0, p.1);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, p) = (self.at(x1, y1), self.at(x, y));
        self.path.quad_to(a.0, a.1, p.0, p.1);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, c, p) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.path.cubic_to(a.0, a.1, c.0, c.1, p.0, p.1);
    }
    fn close(&mut self) {
        self.path.close();
    }
}
/// Text filling the unit box, in the engine's built-in face.
// ponytail: one face (Fira Mono, built into Bevy); the website offered several.
fn text(text: &str) -> Option<Shape> {
    let face = ttf_parser::Face::parse(bevy::text::DEFAULT_FONT_DATA, 0).ok()?;
    let glyphs: Vec<_> = text.chars().filter_map(|c| face.glyph_index(c)).collect();
    let width: f32 = glyphs.iter().map(|g| face.glyph_hor_advance(*g).unwrap_or(0) as f32).sum();
    let (top, bottom) = (face.ascender() as f32, face.descender() as f32);
    if width <= 0. || top <= bottom {
        return None;
    }
    let mut path = PathBuilder::new();
    let mut outline = Outline { path: &mut path, pen: 0., sx: 1. / width, sy: 1. / (top - bottom), top };
    for g in glyphs {
        face.outline_glyph(g, &mut outline);
        outline.pen += face.glyph_hor_advance(g).unwrap_or(0) as f32;
    }
    path.finish()
}

/// A stock logo drawn over a finished side: library image `logo`, placed by retail's stamp `rows`
/// over the side's rectangle `rect` (`[top, bottom, left, right]`, V up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Overlay {
    pub logo: u64,
    pub rows: [Vec4; 2],
    pub rect: [f32; 4],
}
/// Draw a design on a `size` canvas (`canvas()` for a side), its long side 4 units, to
/// straight-alpha RGBA. Images missing from `images` are left out. `over`: a stock logo drawn on
/// top of everything, where its retail stamp puts it on the side (cut at the side's edge).
pub(crate) fn render(doc: &Doc, (width, height): (u32, u32), images: &HashMap<u64, Pixmap>, over: Option<Overlay>) -> Vec<u8> {
    let mut canvas = Pixmap::new(width, height).expect("design size");
    let [r, g, b, a] = doc.background;
    canvas.fill(tiny_skia::Color::from_rgba8(r, g, b, a));
    let unit = width.max(height) as f32 / 4.;
    let (cx, cy) = (width as f32 / 2., height as f32 / 2.);
    for l in &doc.layers {
        let flip = |f: bool| if f { -1. } else { 1. };
        let place = Transform::from_translate(cx + l.x as f32 / 1000. * unit, cy + l.y as f32 / 1000. * unit)
            .pre_rotate(l.angle as f32)
            .pre_scale(l.w as f32 / 1000. * unit * flip(l.flip_x), l.h as f32 / 1000. * unit * flip(l.flip_y));
        let [r, g, b, a] = l.colour;
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        match &l.art {
            Art::Shape(s) => {
                if let Some(path) = shape(*s) {
                    // The first fifteen were drawn even-odd; the later ones are built for the non-zero rule.
                    let rule = if usize::from(*s) < OLD_SHAPES { FillRule::EvenOdd } else { FillRule::Winding };
                    canvas.fill_path(&path, &paint, rule, place, None);
                }
            }
            Art::Text(t) => {
                if let Some(path) = text(t) {
                    canvas.fill_path(&path, &paint, FillRule::Winding, place, None);
                }
            }
            Art::Logo(id) | Art::GameArt(id) => {
                if let Some(image) = images.get(id) {
                    let place = place.pre_translate(-0.5, -0.5).pre_scale(1. / image.width() as f32, 1. / image.height() as f32);
                    let paint = PixmapPaint { opacity: a as f32 / 255., quality: FilterQuality::Bilinear, ..default() };
                    canvas.draw_pixmap(0, 0, image.as_ref(), &paint, place, None);
                }
            }
        }
    }
    if let Some(over) = over {
        let [row_u, row_v] = over.rows;
        let stamp = Mat2::from_cols_array(&[row_u.x, row_v.x, row_u.y, row_v.y]);
        if let (Some(logo), true) = (images.get(&over.logo), stamp.determinant().abs() > 1e-9) {
            let [top, bottom, left, right] = over.rect;
            let (inverse, shift) = (stamp.inverse(), Vec2::new(row_u.z, row_v.z));
            // A logo pixel to its texture UV, to the garment's UV, to a pixel of this side.
            let at = |p: Vec2| {
                let uv = inverse * (p / Vec2::new(logo.width() as f32, logo.height() as f32) - shift);
                Vec2::new((uv.x - left) / (right - left) * width as f32, (uv.y - (1. - top)) / (top - bottom) * height as f32)
            };
            let origin = at(Vec2::ZERO);
            let (x_axis, y_axis) = (at(Vec2::X) - origin, at(Vec2::Y) - origin);
            let place = Transform::from_row(x_axis.x, x_axis.y, y_axis.x, y_axis.y, origin.x, origin.y);
            let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..default() };
            canvas.draw_pixmap(0, 0, logo.as_ref(), &paint, place, None);
        }
    }
    canvas.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}
/// A stock graphic for drawing into designs, at most 256 px.
fn load_logo(path: &Path) -> Option<Pixmap> {
    let mut image = image::open(path).ok()?.into_rgba8();
    if image.width().max(image.height()) > 256 {
        image = image::imageops::thumbnail(&image, 256.min(image.width()), 256.min(image.height()));
    }
    let (w, h) = image.dimensions();
    let mut data = image.into_raw();
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u16;
        for c in &mut p[..3] {
            *c = ((*c as u16 * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}
/// A drawn side, mipped like retail's decals.
pub(crate) fn texture(rgba: Vec<u8>, (width, height): (u32, u32)) -> Image {
    let size = Extent3d { width, height, depth_or_array_layers: 1 };
    let mut image = Image::new_uninit(size, TextureDimension::D2, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    let (data, levels) = mips(&rgba, width, height);
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image
}
/// Bytes of a side texture with its full mip chain.
pub(crate) fn texture_bytes((mut w, mut h): (u32, u32)) -> usize {
    let mut total = 0;
    loop {
        total += w as usize * h as usize * 4;
        if w == 1 && h == 1 {
            return total;
        }
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
}
/// Retail's stamp textures (every stock logo and tattoo on the disc) carry a full mip chain, 9
/// levels at 256x128. Ours: RGBA8 `rgba` followed by each level down to 1x1, each the 2x2 box
/// average of the last in the stored sRGB, straight-alpha bytes (the stock logos' own level 1
/// matches that within DXT5 error). Returns the data and the level count.
pub(crate) fn mips(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32) {
    let mut out = rgba.to_vec();
    let (mut w, mut h, mut at, mut levels) = (width as usize, height as usize, 0, 1);
    while (w > 1 || h > 1) && out.len() >= at + w * h * 4 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let level = &out[at..at + w * h * 4];
        let px = |x: usize, y: usize, c: usize| u32::from(level[(y.min(h - 1) * w + x.min(w - 1)) * 4 + c]);
        let next: Vec<u8> = (0..nh * nw * 4)
            .map(|i| {
                let (x, y, c) = (i / 4 % nw * 2, i / 4 / nw * 2, i % 4);
                ((px(x, y, c) + px(x + 1, y, c) + px(x, y + 1, c) + px(x + 1, y + 1, c) + 2) / 4) as u8
            })
            .collect();
        at += w * h * 4;
        out.extend(next);
        (w, h, levels) = (nw, nh, levels + 1);
    }
    (out, levels)
}

/// What a side wears: its design on its rectangle, and a stock logo drawn over it (a side with a
/// logo and no design is not a `Side`: it stays on retail's own path).
pub(crate) struct Side<'a> {
    pub design: &'a str,
    pub rect: [f32; 4],
    pub over: Option<(u64, [Vec4; 2])>,
}
impl Side<'_> {
    fn overlay(&self) -> Option<Overlay> {
        self.over.map(|(logo, rows)| Overlay { logo, rows, rect: self.rect })
    }
    /// The cache key: the design, the side's shape, the logo and where it sits, and the size.
    fn key(&self, size: u32) -> [u8; 32] {
        let mut hash = blake3::Hasher::new();
        hash.update(self.design.as_bytes());
        hash.update(&size.to_le_bytes());
        for v in self.rect {
            hash.update(&v.to_le_bytes());
        }
        if let Some((logo, rows)) = &self.over {
            hash.update(&logo.to_le_bytes());
            for v in rows.iter().flat_map(|r| r.to_array()) {
                hash.update(&v.to_le_bytes());
            }
        }
        *hash.finalize().as_bytes()
    }
}

/// One drawn side in the cache.
struct Drawn {
    image: Handle<Image>,
    used: Instant,
    /// Texture bytes with mips.
    bytes: usize,
    /// The size it was drawn for and the rectangle it covers (bits), to find its predecessor.
    size: u32,
    rect: [u32; 4],
}
/// Drawn sides, shared by everyone wearing the same one.
#[derive(Default)]
pub(crate) struct Designs {
    drawn: HashMap<[u8; 32], Drawn>,
    logos: HashMap<u64, Option<Pixmap>>,
    window: Option<Instant>,
    spent: u32,
}
/// Sides drawn per frame; a lobby of 50 arriving at once spreads over a second or so.
const PER_FRAME: u32 = 4;
/// Library images decoded per frame, over all the sides drawn: a side of 64 distinct images (a
/// peer's) loads over several frames instead of stalling one.
const LOADS_PER_FRAME: usize = 8;
/// Drawn sides kept for reuse. A full lobby wears 50 x 6 of them at once (4 top sides, a hat and
/// a deck each), and an entry that falls out while worn would be drawn again every frame.
const KEEP: usize = 400;
/// ...and at most this many texture bytes (50 players' sides are about 200 MB); the least recently
/// used go first, so the editor's many drafts cannot pile up.
const BUDGET: usize = 512 << 20;
impl Designs {
    pub fn get(&self, side: &Side, size: u32) -> Option<Handle<Image>> {
        self.drawn.get(&side.key(size)).map(|e| e.image.clone())
    }
    /// Drop a drawn side (a preview tile's: nothing else wears it).
    pub fn forget(&mut self, side: &Side, size: u32) {
        self.drawn.remove(&side.key(size));
    }
    /// Drop the sides of this size that no material wears any more: LOCAL_SIZE when Edit Skater
    /// closes (the drafts and previews it walked through), REMOTE_SIZE once out of a lobby.
    pub fn forget_unworn(&mut self, size: u32) {
        // A handle only the cache holds is a texture no material shows.
        let worn = |image: &Handle<Image>| matches!(image, Handle::Strong(strong) if std::sync::Arc::strong_count(strong) > 1);
        self.drawn.retain(|_, e| e.size != size || worn(&e.image));
    }
    /// Sides cached and their texture bytes.
    #[cfg(test)]
    pub fn resident(&self) -> (usize, usize) {
        (self.drawn.len(), self.drawn.values().map(|e| e.bytes).sum())
    }
    /// Draw the side if this frame's budget allows. False until it is drawn; an invalid
    /// design is ready at once (as nothing).
    pub fn prepare(&mut self, side: &Side, size: u32, images: &mut Assets<Image>, image_file: impl Fn(u64) -> Option<PathBuf>) -> bool {
        let key = side.key(size);
        if let Some(entry) = self.drawn.get_mut(&key) {
            entry.used = Instant::now();
            return true;
        }
        if self.window.is_none_or(|w| w.elapsed() > Duration::from_millis(15)) {
            self.window = Some(Instant::now());
            self.spent = 0;
        }
        if self.spent >= PER_FRAME {
            return false;
        }
        let Ok(doc) = Doc::decode(side.design) else { return true };
        // Its library images first, a few a frame; the side itself once they are all in.
        let mut missing: Vec<u64> = doc.images().chain(side.over.map(|o| o.0)).filter(|id| !self.logos.contains_key(id)).collect();
        missing.sort_unstable();
        missing.dedup();
        for &id in missing.iter().take(LOADS_PER_FRAME) {
            self.logos.insert(id, image_file(id).and_then(|p| load_logo(&p)));
        }
        if missing.len() > LOADS_PER_FRAME {
            self.spent = PER_FRAME;
            return false;
        }
        self.spent += 1;
        let canvas = canvas(side.rect, size);
        let bytes = texture_bytes(canvas);
        let handle = images.add(texture(self.draw(&doc, canvas, side.overlay(), image_file), canvas));
        // The local skater wears one design per side at a time, so a new one (an edit, a preview, a
        // logo adjust step) replaces the last for that rectangle instead of piling up beside it.
        let rect = side.rect.map(f32::to_bits);
        if size == LOCAL_SIZE {
            self.drawn.retain(|_, e| e.size != size || e.rect != rect);
        }
        while self.drawn.len() >= KEEP || (self.drawn.values().map(|e| e.bytes).sum::<usize>() + bytes > BUDGET && !self.drawn.is_empty()) {
            let Some(oldest) = self.drawn.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| *k) else { break };
            self.drawn.remove(&oldest);
        }
        self.drawn.insert(key, Drawn { image: handle, used: Instant::now(), bytes, size, rect });
        true
    }
    /// Draw a design, loading the library images it uses once.
    pub fn draw(&mut self, doc: &Doc, canvas: (u32, u32), over: Option<Overlay>, image_file: impl Fn(u64) -> Option<PathBuf>) -> Vec<u8> {
        let wanted: Vec<u64> = doc.images().chain(over.map(|o| o.logo)).collect();
        for &id in &wanted {
            self.logos.entry(id).or_insert_with(|| image_file(id).and_then(|p| load_logo(&p)));
        }
        let images: HashMap<u64, Pixmap> = wanted.iter().filter_map(|&id| Some((id, self.logos.get(&id)?.clone()?))).collect();
        render(doc, canvas, &images, over)
    }
}

// ---- profile data ----

/// A side's profile entry (`graphics.<zone>`) holds a stock logo (`logo`, placed by retail's adjust
/// `x`, `y`, `size`, `angle`) and/or a creator design (`design`, a whole-side canvas with no adjust).
pub(crate) fn logo(entry: &Value) -> Option<&str> {
    entry["logo"].as_str()
}
pub(crate) fn design(entry: &Value) -> Option<&str> {
    entry["design"].as_str()
}
fn clear_adjust(entry: &mut Map<String, Value>) {
    for (key, ..) in ADJUST {
        entry.remove(key);
    }
}
/// Keep only well-formed graphics and tattoo adjusts: unknown zones, keys, stock ids and
/// malformed designs are dropped, adjusts clamped. A peer's bad data costs only its own graphic.
/// Every side keeps its design; the top keeps one stock logo (retail 0x82536c40's category 0).
pub(crate) fn sanitize(profile: &mut Value, stock: impl Fn(&str) -> bool) {
    let Some(profile) = profile.as_object_mut() else { return };
    if let Some(graphics) = profile.get("graphics").cloned() {
        let mut clean = Map::new();
        for (zone, entry) in graphics.as_object().into_iter().flatten() {
            if !ZONES.iter().any(|z| z.0 == zone) {
                continue;
            }
            let (logo, design) = (logo(entry).filter(|id| stock(id)), design(entry).filter(|t| valid(t)));
            if logo.is_none() && design.is_none() {
                continue;
            }
            let mut out = Map::new();
            if let Some(id) = logo {
                out.insert("logo".into(), json!(id));
            }
            if let Some(text) = design {
                out.insert("design".into(), json!(text));
            }
            // The adjust is the logo's; an older design keeps it for `migrate`, which uses it once.
            if logo.is_some() || design.is_some_and(|t| Doc::decode(t).is_err()) {
                Adjust::of(entry).write(&mut out);
            }
            clean.insert(zone.clone(), Value::Object(out));
        }
        // Retail keeps one shirt graphic (0x82536c40's category 0: one location, one state), so an
        // older profile's or peer's several stock logos keep one. ponytail: INFERRED (no retail
        // profile held two): the lowest retail location index (0x83044438: front, back, left arm, right arm).
        let keep = SHIRT.iter().find(|z| clean.get(**z).is_some_and(|e| e.get("logo").is_some())).copied();
        for zone in SHIRT.into_iter().filter(|z| Some(*z) != keep) {
            let Some(side) = clean.get_mut(zone).and_then(Value::as_object_mut) else { continue };
            side.remove("logo");
            if side.get("design").and_then(Value::as_str).is_none_or(|t| Doc::decode(t).is_ok()) {
                clear_adjust(side);
            }
            if !side.contains_key("design") {
                clean.remove(zone);
            }
        }
        profile.insert("graphics".into(), Value::Object(clean));
    }
    if let Some(tattoos) = profile.get_mut("tattoos").and_then(Value::as_object_mut) {
        for entry in tattoos.values_mut() {
            let mut out = Map::new();
            for key in ["id", "side"] {
                if let Some(v) = entry.get(key) {
                    out.insert(key.into(), v.clone());
                }
            }
            Adjust::of(entry).write(&mut out);
            *entry = Value::Object(out);
        }
    }
}
/// Make the profile's version 1 and 2 designs version 3 (`upgrade`) on the sides `rect_of` knows,
/// the worn item's rectangles; one whose item is not worn waits until it is.
pub(crate) fn migrate(profile: &mut Value, rect_of: impl Fn(&str) -> Option<[f32; 4]>) {
    let female = profile["gender"].as_str() == Some("female");
    let Some(graphics) = profile.get_mut("graphics").and_then(Value::as_object_mut) else { return };
    for (zone, side) in graphics.iter_mut() {
        let Some(old) = design(side).and_then(|t| Doc::decode_old(t).ok()) else { continue };
        let Some(rect) = rect_of(zone) else { continue };
        let adjust = if logo(side).is_some() { Adjust::default() } else { Adjust::of(side) };
        side["design"] = json!(upgrade(&old, entry(zone), female, adjust, rect).encode());
        if logo(side).is_none() {
            if let Some(side) = side.as_object_mut() {
                clear_adjust(side);
            }
        }
    }
}
/// Remove creator designs (stock logos stay): Settings or the Player List hid them.
pub(crate) fn without_designs(profile: &mut Value) {
    if let Some(graphics) = profile.get_mut("graphics").and_then(Value::as_object_mut) {
        graphics.retain(|_, side| {
            let Some(side) = side.as_object_mut() else { return true };
            side.remove("design").is_none() || side.contains_key("logo")
        });
    }
}

/// Settings > Gameplay > Player Graphics: other players' creator designs.
pub(crate) static SHOW_OTHERS: AtomicBool = AtomicBool::new(true);
/// Session players whose designs are hidden from the Player List, by actor id.
static HIDDEN: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());
pub(crate) fn hidden(actor: u64) -> bool {
    HIDDEN.lock().is_ok_and(|h| h.contains(&actor))
}
pub(crate) fn toggle_hidden(actor: u64) {
    if let Ok(mut h) = HIDDEN.lock() {
        if !h.remove(&actor) {
            h.insert(actor);
        }
    }
}
/// Actor ids are per session.
pub(crate) fn forget_hidden() {
    if let Ok(mut h) = HIDDEN.lock() {
        h.clear();
    }
}

/// My Graphics: saved designs beside the skater profile.
pub(crate) const SLOTS: usize = 8;
pub(crate) fn slots_file(profile: &Path) -> PathBuf {
    profile.with_file_name("my-graphics.json")
}
pub(crate) fn load_slots(profile: &Path) -> Vec<Option<String>> {
    let saved: Vec<Option<String>> = std::fs::read(slots_file(profile))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    (0..SLOTS).map(|i| saved.get(i).cloned().flatten().filter(|t| valid(t))).collect()
}
#[cfg(test)]
pub(crate) fn save_slots(profile: &Path, slots: &[Option<String>]) -> std::io::Result<()> {
    let path = slots_file(profile);
    let pending = path.with_extension("tmp");
    std::fs::write(&pending, serde_json::to_vec(slots)?)?;
    std::fs::rename(pending, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Doc {
        Doc {
            layers: vec![
                Layer::new(Art::Shape(2), [255, 0, 0, 255]),
                Layer { angle: -45, flip_x: true, ..Layer::new(Art::Text("SKATE 3".into()), [255, 255, 255, 255]) },
                Layer { x: 700, y: -REACH, ..Layer::new(Art::Logo(0x2c7f3817000903e4), [0, 0, 0, 128]) },
            ],
            ..default()
        }
    }
    fn raw(doc: &Doc) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(doc.encode()).unwrap()
    }
    fn enc(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(b)
    }
    #[test]
    fn designs_round_trip_and_reject_anything_malformed() {
        let doc = Doc { background: [10, 20, 30, 200], layers: [sample().layers, vec![Layer::new(Art::GameArt(0x2c7f3817000100aa), [9; 4])]].concat() };
        assert_eq!(Doc::decode(&doc.encode()).unwrap(), doc);
        assert_eq!(raw(&doc)[..2], [b'G', 3]);
        let good = raw(&doc);
        // Versions 1 and 2 (the old 2:1 canvas) read as old designs, never as version 3.
        let old = Doc { layers: sample().layers, ..doc.clone() };
        let v2 = [&[b'G', 2][..], &raw(&old)[2..]].concat();
        assert_eq!(Doc::decode_old(&enc(&v2)).unwrap(), old);
        assert!(Doc::decode(&enc(&v2)).is_err() && valid(&enc(&v2)));
        let v1 = [&[b'G', 1, raw(&old)[HEADER - 1]][..], &raw(&old)[HEADER..]].concat();
        assert_eq!(Doc::decode_old(&enc(&v1)).unwrap(), Doc { background: [0; 4], ..old.clone() });
        assert!(Doc::decode_old(&doc.encode()).is_err(), "a version 3 design is not old");
        // Old versions hold 32 layers, the first 15 shapes and no game art.
        let old_layers = |n: usize, art: Art| {
            let d = Doc { layers: vec![Layer::new(art, [0; 4]); n], ..default() };
            [&[b'G', 2][..], &raw(&d)[2..]].concat()
        };
        assert!(Doc::decode_old(&enc(&old_layers(OLD_LAYERS, Art::Shape(0)))).is_ok());
        assert!(Doc::decode_old(&enc(&old_layers(OLD_LAYERS + 1, Art::Shape(0)))).is_err());
        assert!(Doc::decode_old(&enc(&old_layers(1, Art::GameArt(1)))).is_err());
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(Doc::decode(&enc(&trailing)).is_err());
        assert!(Doc::decode(&enc(&good[..good.len() - 1])).is_err());
        let mut version = good.clone();
        version[1] = 4;
        assert!(Doc::decode(&enc(&version)).is_err() && !valid(&enc(&version)));
        let mut kind = good.clone();
        kind[HEADER] = 9;
        assert!(Doc::decode(&enc(&kind)).is_err());
        let mut shape = good.clone();
        shape[HEADER + LAYER_BYTES] = SHAPES.len() as u8;
        assert!(Doc::decode(&enc(&shape)).is_err());
        let mut far = good.clone();
        far[HEADER + 5..HEADER + 7].copy_from_slice(&(REACH + 1).to_le_bytes());
        assert!(Doc::decode(&enc(&far)).is_err());
        let mut small = good.clone();
        small[HEADER + 9..HEADER + 11].copy_from_slice(&(SIZES.0 - 1).to_le_bytes());
        assert!(Doc::decode(&enc(&small)).is_err());
        let mut turned = good.clone();
        turned[HEADER + 13..HEADER + 15].copy_from_slice(&181i16.to_le_bytes());
        assert!(Doc::decode(&enc(&turned)).is_err());
        let mut flags = good.clone();
        flags[HEADER + 15] = 8;
        assert!(Doc::decode(&enc(&flags)).is_err());
        for bad in ["", "\u{e9}t\u{e9}", "line\nbreak", &"x".repeat(MAX_TEXT + 1)] {
            let doc = Doc { layers: vec![Layer::new(Art::Text(bad.into()), [0; 4])], ..default() };
            assert!(Doc::decode(&doc.encode()).is_err(), "{bad:?}");
        }
        let over = Doc { layers: vec![Layer::new(Art::Shape(0), [0; 4]); MAX_LAYERS + 1], ..default() };
        assert!(Doc::decode(&over.encode()).is_err());
        assert!(Doc::decode(&"A".repeat(MAX_BYTES.div_ceil(3) * 4 + 1)).is_err());
        assert!(Doc::decode("not base64!").is_err());
    }
    #[test]
    fn grouped_round_trips_and_unknown_flag_bits_still_reject() {
        let doc = Doc { layers: vec![Layer::new(Art::Shape(0), [255; 4]), Layer { grouped: true, flip_y: true, ..Layer::new(Art::Shape(2), [255; 4]) }], ..default() };
        let back = Doc::decode(&doc.encode()).unwrap();
        assert_eq!(back, doc);
        assert!(!back.layers[0].grouped && back.layers[1].grouped);
        let good = raw(&doc);
        // The second layer's flags byte: bit 2 is grouped; bits 3 to 7 still reject.
        let at = HEADER + LAYER_BYTES + 1 + 15;
        assert_eq!(good[at], 6);
        for bit in 3..8 {
            let mut bad = good.clone();
            bad[at] |= 1 << bit;
            assert!(Doc::decode(&enc(&bad)).is_err(), "bit {bit}");
        }
        // Designs from before the bit decode ungrouped; old versions never carry it.
        let old = Doc { layers: vec![Layer::new(Art::Shape(1), [9; 4])], ..default() };
        assert!(!Doc::decode(&old.encode()).unwrap().layers[0].grouped);
        let mut v2 = [&[b'G', 2][..], &raw(&old)[2..]].concat();
        v2[HEADER + 15] = 4;
        assert!(Doc::decode_old(&enc(&v2)).is_err());
    }
    #[test]
    fn a_maximum_design_is_64_longest_text_layers() {
        let full = Doc { layers: vec![Layer::new(Art::Text("W".repeat(MAX_TEXT)), [255; 4]); MAX_LAYERS], background: [255; 4] };
        let text = full.encode();
        assert_eq!(Doc::decode(&text).unwrap(), full);
        assert_eq!(MAX_LAYERS, 64);
        assert_eq!(raw(&full).len(), HEADER + MAX_LAYERS * (LAYER_BYTES + 1 + MAX_TEXT), "7 + 64 x 41 bytes");
        assert!(text.len() <= MAX_BYTES.div_ceil(3) * 4, "{} chars", text.len());
        // Anything the strict decode takes draws without trouble at both side sizes.
        let rgba = render(&full, canvas(DECK, LOCAL_SIZE), &HashMap::new(), None);
        assert_eq!(rgba.len() as u32, canvas(DECK, LOCAL_SIZE).0 * canvas(DECK, LOCAL_SIZE).1 * 4);
    }
    #[test]
    fn designs_draw_shapes_and_text_and_skip_missing_logos() {
        let rgba = render(&sample(), (64, 32), &HashMap::new(), None);
        assert_eq!(rgba.len(), 64 * 32 * 4, "2:1");
        let centre = (16 * 64 + 32) * 4;
        assert!(rgba[centre + 3] > 200, "the circle covers the middle");
        let corner_alpha = rgba[3];
        assert_eq!(corner_alpha, 0, "background stays clear");
        let filled = render(&Doc { background: [0, 0, 255, 255], ..sample() }, (64, 32), &HashMap::new(), None);
        assert_eq!(filled[..4], [0, 0, 255, 255], "background colour under the layers");
        assert!(text("Ab").is_some());
        assert!((0..SHAPES.len() as u8).all(|s| shape(s).is_some()));
    }
    /// One shape alone, filling a `size` square canvas (the layer 3400 thousandths across, 85% of it).
    fn one_shape(index: u8, size: u32) -> Vec<u8> {
        let doc = Doc { layers: vec![Layer { w: 3400, h: 3400, ..Layer::new(Art::Shape(index), [255; 4]) }], background: [0; 4] };
        render(&doc, (size, size), &HashMap::new(), None)
    }
    #[test]
    fn the_hand_made_shapes_all_draw_something_and_keep_their_holes() {
        assert!(SHAPES.len() >= 150 && SHAPES.len() <= 180, "about 150 more, inside the reserved range");
        for (i, name) in SHAPES.iter().enumerate() {
            let rgba = one_shape(i as u8, 64);
            let covered = rgba.chunks(4).filter(|p| p[3] > 128).count();
            assert!(covered > 64 * 64 / 50, "{i} {name} draws almost nothing ({covered} px)");
            assert!(covered <= 64 * 64 * 3 / 4, "{i} {name} fills the whole canvas");
        }
        // The frames are open in the middle, the solid shapes are not.
        let middle = |i: usize| one_shape(i as u8, 64)[(32 * 64 + 32) * 4 + 3];
        let at = |name: &str| SHAPES.iter().position(|n| *n == name).unwrap_or_else(|| panic!("{name}"));
        for name in ["Square Frame", "Round Badge", "Hex Frame", "Bearing", "Honeycomb"] {
            assert_eq!(middle(at(name)), 0, "{name} is open in the middle");
        }
        for name in ["Pentagon", "Cat", "Owl", "Shield"] {
            assert_eq!(middle(at(name)), 255, "{name} is solid in the middle");
        }
        // A hole in one shape does not turn another layer's art transparent: two overlapping layers join.
        let doc = Doc {
            layers: vec![
                Layer { x: -200, w: 2000, h: 2000, ..Layer::new(Art::Shape(at("Pentagon") as u8), [255, 0, 0, 255]) },
                Layer { x: 200, w: 2000, h: 2000, ..Layer::new(Art::Shape(at("Pentagon") as u8), [0, 0, 255, 255]) },
            ],
            background: [0; 4],
        };
        let rgba = render(&doc, (64, 64), &HashMap::new(), None);
        assert_eq!(&rgba[(32 * 64 + 32) * 4..][..4], [0, 0, 255, 255], "the later layer is on top");
        // Every index decodes and re-encodes, and the first one past the end is refused.
        for i in [15u8, 100, (SHAPES.len() - 1) as u8] {
            let doc = Doc { layers: vec![Layer::new(Art::Shape(i), [9, 9, 9, 255])], background: [0; 4] };
            assert_eq!(Doc::decode(&doc.encode()).unwrap(), doc, "shape {i}");
        }
        let past = Doc { layers: vec![Layer::new(Art::Shape(SHAPES.len() as u8), [9, 9, 9, 255])], background: [0; 4] };
        assert!(Doc::decode(&past.encode()).is_err(), "an index nothing draws is refused");
        // For a look at them: SKATE_SHAPES_PNG=path writes a contact sheet of every shape.
        if let Some(path) = std::env::var_os("SKATE_SHAPES_PNG") {
            let (cols, cell) = (14usize, 72u32);
            let rows = SHAPES.len().div_ceil(cols) as u32;
            let mut sheet = image::RgbaImage::from_pixel(cols as u32 * cell, rows * cell, image::Rgba([60, 66, 84, 255]));
            for i in 0..SHAPES.len() {
                let rgba = one_shape(i as u8, cell);
                let (ox, oy) = ((i % cols) as u32 * cell, (i / cols) as u32 * cell);
                for (n, p) in rgba.chunks(4).enumerate() {
                    let (x, y) = (n as u32 % cell, n as u32 / cell);
                    let a = f32::from(p[3]) / 255.;
                    let bg = sheet.get_pixel(ox + x, oy + y).0;
                    let mixed = [0, 1, 2].map(|k| (255. * a + f32::from(bg[k]) * (1. - a)) as u8);
                    sheet.put_pixel(ox + x, oy + y, image::Rgba([mixed[0], mixed[1], mixed[2], 255]));
                }
            }
            sheet.save(path).unwrap();
        }
    }
    type Rows = [Vec4; 2];
    fn garment([a, b]: Rows, t: Vec2) -> Vec2 {
        Mat2::from_cols_array(&[a.x, b.x, a.y, b.y]).inverse() * (t - Vec2::new(a.z, b.z))
    }
    /// The art box's corners (read as texture UVs, unflipped, as retail does) in `zone`.
    fn inside(rows: Rows, art: [f32; 4], zone: [f32; 4]) -> bool {
        [(art[2], art[0]), (art[3], art[0]), (art[2], art[1]), (art[3], art[1])].into_iter().all(|(u, v)| {
            let p = garment(rows, Vec2::new(u, v));
            p.x >= zone[2] - 1e-4 && p.x <= zone[3] + 1e-4 && p.y >= 1. - zone[0] - 1e-4 && p.y <= 1. - zone[1] + 1e-4
        })
    }
    #[test]
    fn graphics_and_tattoos_start_where_retail_puts_them() {
        let centre = |rows: Rows| garment(rows, Vec2::splat(0.5));
        // Shirt front: centred on (0.25, 0.25), scale 5 across and 10 down (square texels).
        let front = stamp(entry("front"), false, Adjust::default());
        assert!(centre(front).abs_diff_eq(Vec2::splat(0.25), 1e-5));
        assert!((front[0].truncate().truncate().length() - 5.).abs() < 1e-4 && (front[1].truncate().truncate().length() - 10.).abs() < 1e-4);
        // The deck's start is retail's quarter turn (1.571: the graphic runs down the deck).
        let deck = stamp(entry("deck"), false, Adjust::default());
        assert!(deck[0].x.abs() < 2e-3 && (deck[0].y.abs() - 7.).abs() < 1e-4 && (deck[1].x - 14.).abs() < 1e-4);
        // The second column is the female one: her right sleeve sits 0.016 further out.
        let right = |female| centre(stamp(entry("right"), female, Adjust::default()));
        assert!(right(false).abs_diff_eq(Vec2::new(0.734, 0.65), 1e-5) && right(true).abs_diff_eq(Vec2::new(0.75, 0.65), 1e-5));
        assert!(centre(stamp(entry("hat"), true, Adjust::default())).abs_diff_eq(Vec2::new(0.25, 0.18), 1e-5));
        // Tattoos: the same table (chest, back, arms, legs), in their skin model's quadrant.
        assert_eq!(tattoo("Arm", 2), Some((1, "StampUVConstraintQ1")));
        assert_eq!(tattoo("Arm", 0), Some((7, "StampUVConstraintQ3")));
        assert_eq!(tattoo("Leg", 1), Some((11, "StampUVConstraintQ4")));
        assert_eq!(tattoo("Leg", 2), None);
        let chest = stamp(1, true, Adjust::default());
        assert!(centre(chest).abs_diff_eq(Vec2::new(0.25, 0.29), 1e-5) && (chest[0].x - 5.).abs() < 1e-4);
        let leg = stamp(8, false, Adjust::default());
        assert!(centre(leg).abs_diff_eq(Vec2::new(0.33, 0.85), 1e-5) && (leg[0].x - 10.).abs() < 1e-4);
        assert_eq!(quarter(8), [0.5, 0., 0., 0.5]);
        // Moves are UVs (right, up), half size doubles the scale, and a positive angle turns
        // the art counter-clockwise (V up): its top leans left (-u).
        let moved = stamp(0, false, Adjust { x: 0.05, y: 0.02, size: 0.5, ..default() });
        assert!(centre(moved).abs_diff_eq(Vec2::new(0.3, 0.23), 1e-5) && (moved[0].x - 10.).abs() < 1e-4);
        let top = |rows: Rows| garment(rows, Vec2::new(0.5, 0.)) - centre(rows);
        assert!(top(stamp(0, false, Adjust { angle: 30., ..default() })).x < -1e-3);
        assert!(top(stamp(7, false, Adjust { angle: -30., ..default() })).x > 1e-3);
    }
    #[test]
    fn adjust_steps_fit_like_retail() {
        // Male tee front (retail StampUVConstraintQ1) and Cat Skate's art box.
        let (front, cat) = ([0.98, 0.48, 0.13, 0.37], [0.992126, 0., 0.2784314, 0.7176471]);
        let near = |a: Adjust, b: Adjust| (a.x - b.x).abs() + (a.y - b.y).abs() + (a.size - b.size).abs() + (a.angle - b.angle).abs() < 1e-4;
        // A start that fits is stored as is.
        assert!(near(fit(0, false, front, cat, Adjust::default()), Adjust::default()));
        // A wide art box over a narrow front overflows both sides at the start (drawn so, no fit
        // at draw time); the first step's fit can't slide it, so it grows by 0.1% until it fits.
        let (narrow, wide) = ([0.98, 0.48, 0.17, 0.33], [0.8, 0.2, 0., 1.]);
        assert!(!inside(stamp(0, false, Adjust::default()), wide, narrow));
        let grown = fit(0, false, narrow, wide, Adjust::default());
        let s = 5. / grown.size;
        assert!(s > 6.25 && s < 6.25 * 1.0011 && grown.x == 0. && grown.y == 0., "{grown:?}");
        assert!(inside(stamp(0, false, grown), wide, narrow));
        // Moved past one edge: slid back by the overshoot plus 0.001, size kept.
        let slid = fit(0, false, front, cat, Adjust { x: 0.1, ..default() });
        let right = 0.25 + slid.x + (cat[3] - 0.5) / 5.;
        assert!((right - (0.37 - 0.001)).abs() < 1e-5 && slid.size == 1., "{slid:?}");
        // Grown, moved or turned past the zone: what is stored fits, and stays put.
        for adjust in [
            Adjust { size: 4., ..default() },
            Adjust { x: 0.5, y: -0.5, ..default() },
            Adjust { x: -0.5, y: 0.5, size: 3., angle: 45., ..default() },
            Adjust { angle: 90., size: 2., ..default() },
        ] {
            let stored = fit(0, false, front, cat, adjust);
            assert!(inside(stamp(0, false, stored), cat, front), "{adjust:?} {stored:?}");
            assert!(stored.size <= adjust.size + 1e-5 && stored.angle == adjust.angle, "{adjust:?} {stored:?}");
            let again = fit(0, false, front, cat, stored);
            assert!(near(again, stored), "a fitted adjust is a fixed point: {adjust:?} {stored:?} {again:?}");
        }
        // Retail's smallest.
        assert!((fit(0, false, front, cat, Adjust { size: 0.25, ..default() }).size - 5. / MAX_SCALE).abs() < 1e-5);
    }
    #[test]
    fn decals_get_retails_full_mip_chain() {
        // 256x128 -> 9 levels, as every stock logo and tattoo on the disc.
        let (data, levels) = mips(&vec![0; 256 * 128 * 4], 256, 128);
        assert_eq!((levels, data.len()), (9, (0..9).map(|l| (256 >> l) * (128 >> l).max(1) * 4).sum()));
        // Each level is the 2x2 box average of the last.
        let (data, levels) = mips(&[0, 0, 0, 0, 255, 255, 255, 255, 100, 0, 0, 255, 0, 100, 0, 255], 4, 1);
        assert_eq!(levels, 3);
        assert_eq!(data[16..], [128, 128, 128, 128, 50, 50, 0, 255, 89, 89, 64, 192]);
        let design = texture(vec![0; 256 * 128 * 4], (256, 128));
        assert_eq!(design.texture_descriptor.mip_level_count, 9);
    }
    #[test]
    fn sanitize_keeps_only_valid_graphics() {
        // A grouped layer (flags bit 2) is a valid design like any other.
        let mut grouped = sample();
        if let Some(l) = grouped.layers.get_mut(1) {
            l.grouped = true;
        }
        let design = grouped.encode();
        assert!(valid(&design));
        let mut p = json!({
            "graphics": {
                "front": {"logo": "aaaa", "x": 9, "size": 0.5, "junk": [1, 2, 3]},
                "back": {"design": design, "x": 0.3},
                "left": {"design": "!!"},
                "right": {"logo": "missing"},
                "collar": {"logo": "aaaa"},
                "hat": {"design": design, "angle": 10},
            },
            "tattoos": {"Arm": {"id": "ink", "side": 2, "angle": 999, "extra": "x"}},
        });
        sanitize(&mut p, |id| id == "aaaa");
        // A design on every side it was on (no adjust: the side is its canvas), one stock logo.
        assert_eq!(p["graphics"], json!({"front": {"logo": "aaaa", "x": 0.5, "size": 0.5}, "back": {"design": design}, "hat": {"design": design}}));
        assert_eq!(p["tattoos"], json!({"Arm": {"id": "ink", "side": 2, "angle": 180.0}}));
        without_designs(&mut p);
        assert!(p["graphics"].get("hat").is_none() && p["graphics"].get("back").is_none() && p["graphics"].get("front").is_some());
        // Four designs at once, and a logo over one of them keeps its adjust; hiding designs leaves the logo.
        let mut four = json!({"graphics": {
            "front": {"design": design}, "back": {"design": design}, "left": {"design": design},
            "right": {"design": design, "logo": "aaaa", "x": 0.1}, "deck": {"design": design, "logo": "aaaa"},
        }});
        sanitize(&mut four, |id| id == "aaaa");
        assert_eq!(four["graphics"].as_object().unwrap().len(), 5);
        assert_eq!(four["graphics"]["right"], json!({"design": design, "logo": "aaaa", "x": 0.1}));
        without_designs(&mut four);
        assert_eq!(four["graphics"], json!({"right": {"logo": "aaaa", "x": 0.1}, "deck": {"logo": "aaaa"}}));
        // An older profile's several stock logos: the first valid one in retail's location order; the
        // others lose the logo (and its adjust) but keep their designs.
        let mut old = json!({"graphics": {"right": {"logo": "aaaa", "design": design, "x": 0.2}, "front": {"logo": "missing"}, "left": {"logo": "aaaa", "x": 0.1}, "deck": {"logo": "aaaa"}}});
        sanitize(&mut old, |id| id == "aaaa");
        assert_eq!(old["graphics"], json!({"left": {"logo": "aaaa", "x": 0.1}, "right": {"design": design}, "deck": {"logo": "aaaa"}}));
        // An older (version 2) design keeps its adjust until it is migrated.
        let v2 = enc(&[&[b'G', 2][..], &raw(&sample())[2..]].concat());
        let mut older = json!({"graphics": {"front": {"design": v2, "x": 0.1, "size": 2.0}}});
        sanitize(&mut older, |_| false);
        assert_eq!(older["graphics"], json!({"front": {"design": v2, "x": 0.1, "size": 2.0}}));
    }
    fn to_tex([a, b]: Rows, uv: Vec2) -> Vec2 {
        Vec2::new(a.x * uv.x + a.y * uv.y + a.z, b.x * uv.x + b.y * uv.y + b.z)
    }
    fn texel(rgba: &[u8], (w, h): (u32, u32), t: Vec2) -> Option<[u8; 4]> {
        if !(0. ..1.).contains(&t.x) || !(0. ..1.).contains(&t.y) {
            return None;
        }
        let i = ((t.y * h as f32) as usize * w as usize + (t.x * w as f32) as usize) * 4;
        Some([rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]])
    }
    /// A garment UV (V down) on the side rectangle's canvas.
    fn on_side(rect: [f32; 4], uv: Vec2) -> Vec2 {
        to_tex(side_stamp(rect), uv)
    }
    #[test]
    fn old_designs_become_whole_side_designs_that_look_the_same() {
        let old = Doc {
            layers: vec![
                Layer { w: 1200, h: 1200, ..Layer::new(Art::Shape(2), [255, 0, 0, 255]) },
                Layer { x: 1200, y: 400, w: 600, h: 300, ..Layer::new(Art::Shape(0), [0, 0, 255, 255]) },
                Layer { x: -1000, y: -300, angle: 25, ..Layer::new(Art::Text("AB".into()), [255, 255, 255, 255]) },
                Layer { x: 500, y: -400, w: 800, h: 600, angle: -40, flip_x: true, ..Layer::new(Art::Shape(4), [0, 200, 0, 255]) },
            ],
            ..default()
        };
        let tilted = Adjust { x: 0.02, y: -0.01, size: 0.8, angle: 20. };
        let cases: [(&str, bool, Adjust, [f32; 4]); 5] = [
            ("front", false, Adjust::default(), [0.98, 0.48, 0.13, 0.37]),
            ("left", true, Adjust::default(), [0.47, 0., 0.01, 0.47]),
            ("deck", false, Adjust::default(), DECK),
            ("hat", true, Adjust::default(), HAT),
            ("right", false, tilted, WINDBREAKER[0].1[3]),
        ];
        for (zone, female, adjust, rect) in cases {
            for background in [[0; 4], [30, 60, 90, 255]] {
                let old = Doc { background, ..old.clone() };
                let new = upgrade(&old, entry(zone), female, adjust, rect);
                assert_eq!(new.background, [0; 4], "{zone}: the old background is a layer");
                assert!(Doc::decode(&new.encode()).is_ok(), "{zone}: a valid version 3 design");
                let (old_rgba, old_size) = (render(&old, (256, 128), &HashMap::new(), None), (256, 128));
                let new_size = canvas(rect, LOCAL_SIZE);
                let new_rgba = render(&new, new_size, &HashMap::new(), None);
                let old_rows = stamp(entry(zone), female, adjust);
                let [top, bottom, left, right] = rect;
                let (mut union, mut differ) = (0, 0);
                const N: usize = 240;
                for i in 0..N {
                    for j in 0..N {
                        let uv = Vec2::new(left + (right - left) * (i as f32 + 0.5) / N as f32, 1. - top + (top - bottom) * (j as f32 + 0.5) / N as f32);
                        let a = texel(&old_rgba, old_size, to_tex(old_rows, uv));
                        let b = texel(&new_rgba, new_size, on_side(rect, uv));
                        let covered = |p: Option<[u8; 4]>| p.is_some_and(|c| c[3] > 127);
                        if covered(a) || covered(b) {
                            union += 1;
                            let far = |a: [u8; 4], b: [u8; 4]| (0..3).any(|c| a[c].abs_diff(b[c]) > 90);
                            if covered(a) != covered(b) || a.zip(b).is_some_and(|(a, b)| far(a, b)) {
                                differ += 1;
                            }
                        }
                    }
                }
                assert!(union > 400, "{zone}: the old design shows on the side ({union} points)");
                assert!((differ as f32) < union as f32 * 0.06, "{zone} {background:?}: {differ} of {union} points differ");
            }
        }
        // One wholly off the side is dropped; the rest keep their order; a design in a profile is
        // migrated on the worn item's rectangle and loses its adjust.
        let far = Doc { layers: vec![Layer { x: REACH, y: REACH, ..Layer::new(Art::Shape(0), [1; 4]) }, Layer::new(Art::Shape(2), [2; 4])], ..default() };
        assert_eq!(upgrade(&far, 0, false, Adjust::default(), DECK).layers.len(), 1);
        let v2 = enc(&[&[b'G', 2][..], &raw(&old)[2..]].concat());
        let mut profile = json!({"gender": "male", "graphics": {"front": {"design": v2, "size": 2.0}, "hat": {"design": v2}}});
        migrate(&mut profile, |zone| (zone == "front").then_some([0.98, 0.48, 0.13, 0.37]));
        let front = design(&profile["graphics"]["front"]).unwrap();
        assert!(Doc::decode(front).is_ok() && profile["graphics"]["front"].get("size").is_none());
        assert_eq!(design(&profile["graphics"]["hat"]), Some(v2.as_str()), "no hat worn: waits");
        let again = profile.clone();
        migrate(&mut profile, |_| Some(HAT));
        assert_eq!(profile["graphics"]["front"], again["graphics"]["front"], "a version 3 design is left alone");
    }
    #[test]
    fn an_old_design_at_a_big_adjust_keeps_its_shapes_and_its_background_keeps_covering_the_side() {
        let rect = [0.98, 0.48, 0.13, 0.37];
        // A wide ellipse as big as the old canvas, a small one, and a background colour.
        let old = Doc {
            layers: vec![
                Layer { w: 4000, h: 2000, ..Layer::new(Art::Shape(2), [255, 0, 0, 255]) },
                Layer { w: 600, h: 300, ..Layer::new(Art::Shape(2), [0, 255, 0, 255]) },
            ],
            background: [10, 20, 30, 255],
        };
        let size_of = canvas(rect, LOCAL_SIZE);
        let mut outgrew = 0;
        for size in [1., 2., 4., 8.] {
            let adjust = Adjust { size, ..Adjust::default() };
            let new = upgrade(&old, entry("front"), false, adjust, rect);
            assert!(Doc::decode(&new.encode()).is_ok(), "size {size}: a valid design");
            // Every layer keeps its 2:1 shape, whether or not it hit the limit on the way.
            for layer in new.layers.iter().filter(|l| matches!(l.art, Art::Shape(2))) {
                assert!(((layer.w as f32 / layer.h as f32) / 2. - 1.).abs() < 0.03, "size {size}: {}x{} was squashed", layer.w, layer.h);
                outgrew += usize::from(layer.w == SIZES.1);
            }
            // Where the old background covered the whole side, the new one still does, however big.
            let old_rgba = render(&Doc { layers: vec![], ..old.clone() }, (256, 128), &HashMap::new(), None);
            let new_rgba = render(&Doc { layers: new.layers.iter().filter(|l| l.colour == [10, 20, 30, 255]).cloned().collect(), ..default() }, size_of, &HashMap::new(), None);
            let rows = stamp(entry("front"), false, adjust);
            let [top, bottom, left, right] = rect;
            let (mut old_covers, mut lost) = (0, 0);
            for i in 0..40 {
                for j in 0..40 {
                    let uv = Vec2::new(left + (right - left) * (i as f32 + 0.5) / 40., 1. - top + (top - bottom) * (j as f32 + 0.5) / 40.);
                    if texel(&old_rgba, (256, 128), to_tex(rows, uv)).is_some_and(|c| c[3] > 127) {
                        old_covers += 1;
                        lost += usize::from(!texel(&new_rgba, size_of, on_side(rect, uv)).is_some_and(|c| c[3] > 127));
                    }
                }
            }
            assert!(lost * 50 <= old_covers.max(1), "size {size}: the background lost {lost} of {old_covers} points");
        }
        assert!(outgrew > 0, "the test reaches the size limit");
    }
    #[test]
    fn a_side_canvas_has_square_texels_and_maps_exactly_onto_its_rectangle() {
        let tee = |i: usize| rectangle(["0.98,0.48,0.13,0.37", "0.98,0.48,0.64,0.86", "0.48,0.25,0.05,0.45", "0.48,0.25,0.55,0.95"][i]).unwrap();
        let mut rects: Vec<[f32; 4]> = (0..4).map(tee).collect();
        rects.extend([DECK, HAT]);
        rects.extend(HATS.iter().map(|h| h.1));
        rects.extend(WINDBREAKER.iter().flat_map(|w| w.1));
        for rect in rects {
            let [top, bottom, left, right] = rect;
            for size in [LOCAL_SIZE, REMOTE_SIZE] {
                let (w, h) = canvas(rect, size);
                let (rw, rh) = (right - left, top - bottom);
                assert!(w.max(h) <= 4 * size && w <= size.max(h), "{rect:?} {size}: {w}x{h}");
                assert!(((w as f32 / h as f32) / (rw / rh) - 1.).abs() < 0.01, "{rect:?} {size}: texels are square, {w}x{h}");
                // Exactly onto the rectangle: its corners are the canvas corners.
                let stamp = side_stamp(rect);
                assert!(to_tex(stamp, Vec2::new(left, 1. - top)).abs_diff_eq(Vec2::ZERO, 1e-4), "{rect:?}");
                assert!(to_tex(stamp, Vec2::new(right, 1. - bottom)).abs_diff_eq(Vec2::ONE, 1e-4), "{rect:?}");
                // A layer at each canvas corner lands on the matching rectangle corner.
                let unit = w.max(h) as f32 / 4.;
                let (half_w, half_h) = (w as f32 / unit * 500., h as f32 / unit * 500.);
                let corner = |sx: f32, sy: f32| Layer { x: (sx * (half_w - 100.)) as i16, y: (sy * (half_h - 100.)) as i16, w: 200, h: 200, ..Layer::new(Art::Shape(0), [255, 0, 0, 255]) };
                let doc = Doc { layers: vec![corner(-1., -1.), corner(1., 1.)], ..default() };
                let rgba = render(&doc, (w, h), &HashMap::new(), None);
                let lit = |x: u32, y: u32| rgba[(y * w + x) as usize * 4 + 3] > 200;
                assert!(lit(1, 1) && lit(w - 2, h - 2), "{rect:?} {size}: the layers sit in the canvas corners");
                assert!(!lit(w - 2, 1) && !lit(1, h - 2), "{rect:?} {size}: and only there");
                // The first lit pixel row and column are the canvas edge (to within a pixel).
                let first = (0..w).find(|&x| lit(x, 1)).unwrap();
                let top_row = (0..h).find(|&y| lit(1, y)).unwrap();
                assert!(first <= 1 && top_row <= 1, "{rect:?} {size}: starts at {first},{top_row}");
                let last = (0..w).rev().find(|&x| lit(x, h - 2)).unwrap();
                let bottom_row = (0..h).rev().find(|&y| lit(w - 2, y)).unwrap();
                assert!(last + 2 >= w && bottom_row + 2 >= h, "{rect:?} {size}: ends at {last},{bottom_row} of {w}x{h}");
            }
        }
        // Tall sides give way in width, wide ones keep the width.
        assert_eq!(canvas(DECK, LOCAL_SIZE), (506, 2048));
        assert_eq!(canvas(DECK, REMOTE_SIZE), (253, 1024));
        assert_eq!(canvas(tee(0), LOCAL_SIZE), (512, 1067));
        assert_eq!(canvas(tee(2), LOCAL_SIZE).0, LOCAL_SIZE);
        assert_eq!(canvas([0.5, 0.5, 0.1, 0.2], LOCAL_SIZE), (LOCAL_SIZE, LOCAL_SIZE / 2), "a degenerate rectangle");
    }
    #[test]
    fn a_stock_logo_is_drawn_over_a_sides_design() {
        let rect = [0.98, 0.48, 0.13, 0.37];
        let red = Doc { layers: vec![Layer { w: 4000, h: 4000, ..Layer::new(Art::Shape(0), [255, 0, 0, 255]) }], ..default() };
        let mut green = Pixmap::new(256, 128).unwrap();
        green.fill(tiny_skia::Color::from_rgba8(0, 255, 0, 255));
        let images = HashMap::from([(7, green)]);
        let rows = stamp(entry("front"), false, Adjust::default());
        let over = Overlay { logo: 7, rows, rect };
        let size = canvas(rect, LOCAL_SIZE);
        let (with, without) = (render(&red, size, &images, Some(over)), render(&red, size, &images, None));
        let at = |rgba: &[u8], uv: Vec2| texel(rgba, size, on_side(rect, uv)).unwrap();
        // Retail's start puts the logo's centre at (0.25, 0.25), 0.2 wide and 0.1 tall (scale 5).
        assert_eq!(at(&with, Vec2::new(0.25, 0.25)), [0, 255, 0, 255], "the logo is on top");
        assert_eq!(at(&without, Vec2::new(0.25, 0.25)), [255, 0, 0, 255], "and the design is under it");
        assert_eq!(at(&with, Vec2::new(0.16, 0.22)), [0, 255, 0, 255]);
        assert_eq!(at(&with, Vec2::new(0.14, 0.25)), [255, 0, 0, 255], "the design shows round the logo");
        assert_eq!(at(&with, Vec2::new(0.25, 0.32)), [255, 0, 0, 255]);
        assert_eq!(at(&with, Vec2::new(0.25, 0.5)), [255, 0, 0, 255]);
        // The logo footprint is where the retail stamp shows it, and no further.
        let inside = |uv: Vec2| to_tex(rows, uv).cmpge(Vec2::ZERO).all() && to_tex(rows, uv).cmple(Vec2::ONE).all();
        let [top, bottom, left, right] = rect;
        let (mut shown, mut agree) = (0, 0);
        for i in 0..200 {
            for j in 0..200 {
                let uv = Vec2::new(left + (right - left) * (i as f32 + 0.5) / 200., 1. - top + (top - bottom) * (j as f32 + 0.5) / 200.);
                let green = at(&with, uv)[1] == 255;
                shown += usize::from(inside(uv));
                agree += usize::from(inside(uv) == green);
            }
        }
        assert!(shown > 1000 && agree >= 200 * 200 - 500, "{agree} of {} points agree with the stamp", 200 * 200);
        // A logo with no design under it draws too.
        let blank = render(&Doc::default(), size, &images, Some(over));
        assert_eq!(at(&blank, Vec2::new(0.25, 0.25)), [0, 255, 0, 255]);
        assert_eq!(at(&blank, Vec2::new(0.14, 0.25))[3], 0);
        // A missing logo draws nothing; the cache tells the sides apart.
        assert_eq!(render(&red, size, &HashMap::new(), Some(over)), without);
        let side = |rect, over| Side { design: "AAAA", rect, over };
        let base = side(rect, None).key(LOCAL_SIZE);
        assert_ne!(base, side(rect, None).key(REMOTE_SIZE));
        assert_ne!(base, side(DECK, None).key(LOCAL_SIZE));
        assert_ne!(base, side(rect, Some((7, rows))).key(LOCAL_SIZE));
        assert_ne!(side(rect, Some((7, rows))).key(LOCAL_SIZE), side(rect, Some((8, rows))).key(LOCAL_SIZE));
        assert_eq!(base, side(rect, None).key(LOCAL_SIZE));
    }
    #[test]
    fn fifty_players_wearing_six_full_sides_fit_in_video_memory() {
        let bytes = texture_bytes;
        // The tallest/biggest customizable item: a tee's four sides, a hat, a deck.
        let sides = [[0.98, 0.48, 0.13, 0.37], [0.98, 0.48, 0.64, 0.86], [0.48, 0.25, 0.05, 0.45], [0.48, 0.25, 0.55, 0.95], HAT, DECK];
        let player = |size| sides.iter().map(|r| bytes(canvas(*r, size))).sum::<usize>();
        let (local, remote) = (player(LOCAL_SIZE), player(REMOTE_SIZE));
        let lobby = local + 49 * remote;
        eprintln!("VRAM: one local skater {} KiB, one other player {} KiB, 50 players {} MiB", local / 1024, remote / 1024, lobby >> 20);
        assert!(lobby < 512 << 20, "{} MiB", lobby >> 20);
        // The cache keeps every worn side (KEEP of them) inside its byte budget.
        assert!(KEEP >= 50 * sides.len(), "every worn side stays cached");
        assert!(lobby * 2 < BUDGET, "a lobby of worn sides is well inside the cache's byte budget");
    }
    /// Draw `side` now, whatever this frame's budget says.
    fn draw_now(designs: &mut Designs, side: &Side, size: u32, images: &mut Assets<Image>) {
        designs.window = None;
        while !designs.prepare(side, size, images, |_| None) {
            designs.window = None;
        }
    }
    #[test]
    fn editing_keeps_one_local_texture_per_side_however_many_steps_and_the_rest_goes_when_unworn() {
        let (rect, other) = ([0.98, 0.48, 0.13, 0.37], [0.98, 0.48, 0.64, 0.86]);
        let one = texture_bytes(canvas(rect, LOCAL_SIZE));
        let text = |n: i16| Doc { layers: vec![Layer { x: n, ..Layer::new(Art::Shape(2), [255, 0, 0, 255]) }], ..default() }.encode();
        let (mut images, mut designs) = (Assets::<Image>::default(), Designs::default());
        // Two hundred edit steps (the creator wears the design every 0.25 s): always one texture.
        for n in 0..200 {
            let text = text(n);
            draw_now(&mut designs, &Side { design: &text, rect, over: None }, LOCAL_SIZE, &mut images);
            assert_eq!(designs.resident(), (1, one), "edit step {n}");
        }
        // Two hundred Graphic Adjust steps of a stock logo over a design: the same.
        let design = text(7);
        for n in 0..200 {
            let rows = [Vec4::new(1. + n as f32 / 50., 0., 0., 0.), Vec4::Y];
            draw_now(&mut designs, &Side { design: &design, rect, over: Some((9, rows)) }, LOCAL_SIZE, &mut images);
            assert_eq!(designs.resident(), (1, one), "adjust step {n}");
        }
        // Another side of the item is its own texture; other players' sides are not replaced by edits.
        draw_now(&mut designs, &Side { design: &design, rect: other, over: None }, LOCAL_SIZE, &mut images);
        assert_eq!(designs.resident().0, 2);
        for n in 0..5 {
            let text = text(n);
            draw_now(&mut designs, &Side { design: &text, rect, over: None }, REMOTE_SIZE, &mut images);
        }
        assert_eq!(designs.resident().0, 2 + 5, "remote sides are shared by everyone wearing them: each is kept");
        // Edit Skater closes: only the local sides a material still holds stay.
        let worn = designs.get(&Side { design: &design, rect: other, over: None }, LOCAL_SIZE).expect("drawn");
        designs.forget_unworn(LOCAL_SIZE);
        assert_eq!(designs.resident().0, 1 + 5, "the unworn local side went, the worn one and the remote ones stayed");
        drop(worn);
        designs.forget_unworn(LOCAL_SIZE);
        assert_eq!(designs.resident().0, 5, "once nothing wears it, it goes");
        // A preview tile's sides are forgotten when it is drawn.
        let text = text(3);
        let side = Side { design: &text, rect, over: None };
        designs.forget(&side, REMOTE_SIZE);
        assert_eq!(designs.resident().0, 4);
        // Out of the lobby: other players' sides nothing wears go too.
        designs.forget_unworn(REMOTE_SIZE);
        assert_eq!(designs.resident().0, 0, "a left lobby's sides do not stay in video memory");
    }
    #[test]
    fn a_side_of_many_library_images_loads_them_over_frames() {
        let art = |n: u64| Layer::new(Art::GameArt(n), [255; 4]);
        let text = Doc { layers: (1..=40).map(art).collect(), ..default() }.encode();
        let (mut images, mut designs, loaded) = (Assets::<Image>::default(), Designs::default(), std::cell::Cell::new(0));
        let side = Side { design: &text, rect: DECK, over: None };
        let mut calls = 0;
        loop {
            designs.window = None;
            loaded.set(0);
            let ready = designs.prepare(&side, REMOTE_SIZE, &mut images, |_| {
                loaded.set(loaded.get() + 1);
                None
            });
            assert!(loaded.get() <= LOADS_PER_FRAME, "{} images in one frame", loaded.get());
            calls += 1;
            if ready {
                break;
            }
            assert!(calls < 20);
        }
        assert_eq!(calls, 40usize.div_ceil(LOADS_PER_FRAME), "40 images, 8 a frame, the side with the last of them");
        assert_eq!(designs.resident().0, 1);
    }
    #[test]
    fn every_customizable_item_gets_its_locations() {
        use crate::customiser_parts::Part;
        let part = |slot: &str, flags: &[(&str, &str)]| Part {
            slot: slot.into(),
            name: String::new(),
            flags: flags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            materials: Vec::new(),
            scene: String::new(),
        };
        let zones = |l: Vec<(&'static str, [f32; 4])>| l.into_iter().map(|z| z.0).collect::<Vec<_>>();
        // Tee: the retail flags, in shader zone order.
        let q = [("StampUVConstraintQ1", "0.98,0.48,0.13,0.37"), ("StampUVConstraintQ2", "0.98,0.48,0.64,0.86"),
            ("StampUVConstraintQ3", "0.48,0.25,0.05,0.45"), ("StampUVConstraintQ4", "0.48,0.25,0.55,0.95"), ("TopType", "tshirt")];
        let tee = locations("801095192c838106", &part("OuterTorso", &q), true);
        assert_eq!(zones(tee.clone()), ["front", "back", "left", "right"]);
        assert_eq!(tee[1].1, [0.98, 0.48, 0.64, 0.86]);
        assert!(locations("801095192c838106", &part("OuterTorso", &q), false).is_empty(), "not IsLogoable");
        // Windbreaker: no retail flags, measured rectangles; a top without either (the Skate 2 tee) offers none.
        let bare = part("OuterTorso", &[("TopType", "jacket")]);
        assert_eq!(zones(locations("c620cb8d10a788ec", &bare, true)), ["front", "back", "left", "right"]);
        assert!(locations("99d51597a9cd41a6", &bare, true).is_empty());
        // Bare torso (tattoos), hat, deck, other slots.
        assert!(locations("x", &part("OuterTorso", &[]), true).is_empty());
        assert_eq!(locations("dcbfd26f04c7744e", &part("Hat", &[]), true), [("hat", HAT)]);
        assert_eq!(locations("c2ce7955e7ba5142", &part("Hat", &[]), true), [("hat", [0.857, 0.582, 0.003, 0.495])]);
        assert_eq!(locations("a841a65d5c4dd593", &part("SkateBoard", &[]), true), [("deck", DECK)]);
        assert!(locations("x", &part("Pants", &[]), true).is_empty());
        // Every rectangle sits in its own secondary-UV quadrant (Q1 top left, V up; retail's
        // own flags overlap the middle by up to 0.025).
        for (id, model) in [("c620cb8d10a788ec", &bare), ("801095192c838106", &part("OuterTorso", &q))] {
            for (i, (_, [top, bottom, left, right])) in locations(id, model, true).into_iter().enumerate() {
                let (u0, v0, e) = ((i % 2) as f32 * 0.5, if i < 2 { 0.5 } else { 0. }, 0.03);
                assert!(left >= u0 - e && right <= u0 + 0.5 + e && bottom >= v0 - e && top <= v0 + 0.5 + e && top > bottom && right > left, "{id} {i}");
            }
        }
    }
}
