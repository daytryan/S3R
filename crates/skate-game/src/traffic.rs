//! Street traffic (items 24b/24c): only on `New_San_Vanelona`, `Skate_1_San_Vanelona` and the Skate 3
//! districts with roads (`DownTown`, `Industrial`, `University`), only offline. A port of the Skate 2 RAIN
//! traffic agent (s2.pe; addresses are s2.pe unless marked). Skate 1 and Skate 3 run the same agent on their
//! own data (tu3.pe: LW driver 0x826B71F0, vehicles circle 0x826B7760, census spawn 0x826B8B88). Data:
//! `assets/private/traffic/{skate2,skate1,skate3/<map>}/` from tools/pipeline/skate2port/old_traffic.py. Mission B: each car is
//! a vehicle body in the skater's solve ([`collide`], collision group 8: the wipeout's vehicle rules), its contacts
//! run the contact callback 0x82b0a6b8 ([`Sim::hit`]: alarm, brake, the Hall of Meat's hit mask, [`Traffic::hom_hits`]).
//!
//! Field names keep the retail vehicle offsets (`f3420` = veh+3420 ...). `kind` is veh4376 (what the car
//! is stopping for), `plan` veh4380 (0 none, 1 lane change, 3 pull over), `near` veh3736.
use std::collections::{HashMap, VecDeque};
use std::path::Path;

use bevy::{light::NotShadowCaster, prelude::*, scene::SceneInstance};
use serde::Deserialize;
use serde_json::Value;
use skate_data::audio::title::{SKATE1_MAP, SKATE2_MAP};
use skate_data::audio::world::{self as painter, Tile};

use crate::modding::vehicles::network::CollisionShape;

/// The RAIN step, 1/60 s (0x82060140); every callback here runs once per step.
const DT: f32 = 1.0 / 60.0;
const MAX: f32 = f32::MAX;
/// Census circle C779 (vehicles), layout (80, 100, 110, 0, 0) [0x825cbfc0]: spawn ring and despawn radius. tu3
/// 0x826B7760 centres it on the skater plus dir(velocity) * layout[3] (0 here); neither 826B9B90 (spawn) nor
/// 826BAAB8 (despawn, |centre - pos|^2 - r^2 >= 0) looks at the camera.
const RING: (f32, f32) = (80.0, 100.0);
const DESPAWN: f32 = 110.0;
/// Not retail (user 10-14, bug #8: cars popping in): no ring spawn or despawn inside the camera's view cone
/// (half-angle 60 degrees, wider than any gameplay FOV), unless a car in view is past this far.
/// ponytail: a cone, not the frustum, and nothing tests occlusion; a car hidden behind a building still waits.
const VIEW_COS: f32 = 0.5;
const DESPAWN_SEEN: f32 = 160.0;
/// Skate 3's budget A maximum (tu3 0x826BD9C8, `li 0x17`): bug 10-17 #42, far too many cars on Skate 3 maps.
const SKATE3_LIVE_CARS: usize = 23;

// ---------------------------------------------------------------- exported data

#[derive(Deserialize)]
struct JRoads {
    superroads: Vec<JSuper>,
    roads: Vec<JRoad>,
    intersections: Vec<JInter>,
}
#[derive(Deserialize)]
struct JSuper {
    id: String,
    a: String,
    a_n: usize,
    b: String,
    length: f32,
    speed: f32,
    lanes_ab: usize,
    lanes_ba: u32,
}
#[derive(Deserialize)]
struct JEnd {
    id: String,
    a: u32,
    lanes: Vec<Vec<usize>>,
}
#[derive(Deserialize)]
#[allow(dead_code)] // `ends`: read by the tests
struct JRoad {
    id: String,
    ends: [JEnd; 2],
    #[serde(rename = "super")]
    sup: String,
    w128: [u32; 6],
    pieces: Vec<JPiece>,
}
#[derive(Deserialize)]
struct JPiece {
    p0: [f32; 3],
    p1: [f32; 3],
    t0: [f32; 3],
    t1: [f32; 3],
    arc: Vec<f32>,
    length: f32,
    edges: [[f32; 3]; 4],
    edge_length: f32,
}
#[derive(Deserialize)]
struct JInter {
    id: String,
    speed: f32,
    w580: u32,
    inner: [[f32; 3]; 4],
    entries: [JEnd; 4],
    exits: [JEnd; 4],
    lanes: Vec<JLane>,
}
#[derive(Deserialize)]
struct JLane {
    p0: [f32; 3],
    p1: [f32; 3],
    t0: [f32; 3],
    t1: [f32; 3],
    arc: Vec<f32>,
    speed: f32,
    entry: usize,
    w88: usize,
    exit: usize,
    w96: usize,
}
#[derive(Deserialize)]
struct JCensus {
    painter: Vec<JTile>,
    census: HashMap<String, JRow>,
    groups: HashMap<String, Vec<JGroup>>,
    entities: HashMap<String, JEntity>,
    group_painter: Option<Vec<JTile>>,
    livingworld: HashMap<String, HashMap<String, Value>>,
}
#[derive(Deserialize)]
struct JTile {
    cx: f32,
    cz: f32,
    hx: f32,
    hz: f32,
    nodes: String,
    palette: Vec<String>,
    high: bool,
}
#[derive(Deserialize)]
struct JRow {
    count: u32,
    group: Option<String>,
}
#[derive(Deserialize)]
struct JGroup {
    weight: f32,
    entities: Vec<String>,
}
#[derive(Deserialize)]
struct JEntity {
    variants: Vec<JVariant>,
    profile: HashMap<String, Value>,
    behaviour: HashMap<String, Value>,
    /// Hall of Meat vehicle type (old_traffic.py hom_type); -1 = none.
    #[serde(default = "no_hom")]
    hom: i32,
    /// The Skate 3 audio row (vault class 259095163B974174) whose engine type picks the car's engine bank
    /// (old_traffic.py; Skate 3's own entity row on its maps), hex; absent = silent.
    #[serde(default)]
    audio: Option<String>,
}
fn no_hom() -> i32 {
    -1
}
#[derive(Deserialize)]
struct JVariant {
    model: String,
    #[serde(default)]
    appearance: HashMap<String, Value>,
}

/// A variant's colorize lists (census appearance): DF76D7D773857EDB the body colours (10 per Skate 1 model),
/// 12026E2EED18CC8D the second colours ([[0,0,1,1]]: blue stays blue). INFERRED: which feeds colorize_red and which
/// colorize_blue, from the values (the native that sets the constants is UNTRACED).
#[derive(Clone, Default)]
struct Palette {
    red: Vec<[f32; 3]>,
    blue: Vec<[f32; 3]>,
}

fn palette(appearance: &HashMap<String, Value>) -> Palette {
    let list = |key: &str| -> Vec<[f32; 3]> {
        appearance.get(key).and_then(Value::as_array).into_iter().flatten()
            .filter_map(|c| Some([c[0].as_f64()? as f32, c[1].as_f64()? as f32, c[2].as_f64()? as f32]))
            .collect()
    };
    Palette { red: list("DF76D7D773857EDB"), blue: list("12026E2EED18CC8D") }
}

/// Skate 1's vehicle pixel shader (render.big vehiclematerial.psh): a diffuse texel with green under 0.035 is paint,
/// its red and blue the weights of the two colours. Applied to the texture's stored (sRGB) bytes. INFERRED: the
/// shader's diffuse is the sampled value without a gamma conversion.
pub(crate) fn colorize(rgba: &mut [u8], red: [f32; 3], blue: [f32; 3]) {
    for px in rgba.chunks_exact_mut(4) {
        let [r, g, b] = [px[0], px[1], px[2]].map(|v| f32::from(v) / 255.0);
        if g < 0.035 {
            for k in 0..3 {
                px[k] = ((r * red[k] + b * blue[k]).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s, 16).unwrap_or(0)
}

fn tile(t: &JTile) -> Tile {
    let bytes: Vec<u8> = (0..t.nodes.len() / 2).filter_map(|i| u8::from_str_radix(&t.nodes[2 * i..2 * i + 2], 16).ok()).collect();
    Tile {
        centre: [t.cx, t.cz],
        half: [t.hx, t.hz],
        nodes: bytes.chunks_exact(10).map(|n| std::array::from_fn(|k| u16::from_be_bytes([n[2 * k], n[2 * k + 1]]))).collect(),
        // "" (an unpainted palette slot) reads as key 0, which no census row has.
        palette: t.palette.iter().map(|p| hex(p)).collect(),
        high: t.high,
    }
}

// ---------------------------------------------------------------- road network

/// A cubic Hermite with its arc-length table (lane 112 B / piece 224 B).
#[derive(Clone)]
struct Curve {
    p0: Vec3,
    p1: Vec3,
    t0: Vec3,
    t1: Vec3,
    arc: Vec<f32>,
}

impl Curve {
    fn new(p0: [f32; 3], p1: [f32; 3], t0: [f32; 3], t1: [f32; 3], arc: Vec<f32>) -> Self {
        Self { p0: p0.into(), p1: p1.into(), t0: t0.into(), t1: t1.into(), arc }
    }
    /// 0x82b0bfe8's table: chord sums over `n` evenly spaced parameters.
    fn build(p0: Vec3, p1: Vec3, t0: Vec3, t1: Vec3, n: usize) -> Self {
        let mut c = Self { p0, p1, t0, t1, arc: vec![0.0] };
        for k in 1..n {
            let step = c.at(k as f32 / (n - 1) as f32).distance(c.at((k - 1) as f32 / (n - 1) as f32));
            c.arc.push(c.arc[k - 1] + step);
        }
        c
    }
    fn len(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }
    /// 0x8235bcf0: distance -> parameter through the arc table.
    fn param(&self, s: f32) -> f32 {
        let n = self.arc.len();
        let Some(i) = self.arc.iter().position(|&a| a > s) else { return 1.0 };
        if i == 0 {
            return 0.0; // ponytail: s < arc[0] = 0 never reaches retail; clamp instead of reading arc[-1]
        }
        ((i - 1) as f32 + (s - self.arc[i - 1]) / (self.arc[i] - self.arc[i - 1])) / (n - 1) as f32
    }
    /// 0x8235b678: rows [2 -2 1 1; -3 3 -2 -1; 0 0 1 0; 1 0 0 0].
    fn at(&self, t: f32) -> Vec3 {
        let (t2, t3) = (t * t, t * t * t);
        self.p0 * (2.0 * t3 - 3.0 * t2 + 1.0) + self.p1 * (3.0 * t2 - 2.0 * t3) + self.t0 * (t3 - 2.0 * t2 + t) + self.t1 * (t3 - t2)
    }
    /// 0x8235bfd0 (UNTRACED body; the analytic derivative of the same basis).
    fn tangent(&self, t: f32) -> Vec3 {
        let t2 = t * t;
        self.p0 * (6.0 * t2 - 6.0 * t) + self.p1 * (6.0 * t - 6.0 * t2) + self.t0 * (3.0 * t2 - 4.0 * t + 1.0) + self.t1 * (3.0 * t2 - 2.0 * t)
    }
}

struct Piece {
    c: Curve,
    len: f32,
    /// Cumulative end within the road (piece +208).
    end: f32,
    /// start-left, start-right, end-left, end-right.
    e: [Vec3; 4],
    up: Vec3,
}

struct Road {
    sup: usize,
    start: f32,
    end: f32,
    pieces: Vec<Piece>,
}

struct Super {
    ahead: Option<usize>,
    /// Entry slot of `ahead` this superroad feeds (a_n).
    slot: usize,
    behind: Option<usize>,
    len: f32,
    speed: f32,
    lanes: usize,
    /// 1 parking allowed, 2 lane change allowed (superroad +56).
    flags: u32,
    roads: Vec<usize>,
}

struct Slot {
    sup: usize,
    start: f32,
    /// Per road lane: the intersection lane records it feeds (entry) or is fed by (exit).
    lanes: Vec<Vec<usize>>,
}

struct ILane {
    c: Curve,
    speed: f32,
    entry: usize,
    entry_lane: usize,
    exit: usize,
    exit_lane: usize,
}

struct Inter {
    lights: bool,
    speed: f32,
    up: Vec3,
    entries: [Option<Slot>; 4],
    exits: [Option<Slot>; 4],
    lanes: Vec<ILane>,
}

#[derive(Clone, Copy, Default, Debug)]
struct Pose {
    pos: Vec3,
    fwd: Vec3,
    up: Vec3,
    left: Vec3,
}

fn frame(pos: Vec3, tangent: Vec3, up: Vec3) -> Pose {
    let fwd = tangent.try_normalize().unwrap_or(Vec3::Z);
    let left = up.cross(fwd).try_normalize().unwrap_or(Vec3::X);
    Pose { pos, fwd, up: fwd.cross(left), left }
}

const CELL: f32 = 10.0;

struct Network {
    sups: Vec<Super>,
    roads: Vec<Road>,
    inters: Vec<Inter>,
    /// Spawn placement points (two per piece) by 10 m cell.
    grid: HashMap<(i32, i32), Vec<(Vec3, usize, usize)>>,
}

impl Network {
    fn new(j: &JRoads) -> Self {
        let inter_ix: HashMap<&str, usize> = j.intersections.iter().enumerate().map(|(i, x)| (x.id.as_str(), i)).collect();
        let sup_ix: HashMap<&str, usize> = j.superroads.iter().enumerate().map(|(i, x)| (x.id.as_str(), i)).collect();
        let road_ix: HashMap<&str, usize> = j.roads.iter().enumerate().map(|(i, x)| (x.id.as_str(), i)).collect();
        let mut sups: Vec<Super> = j
            .superroads
            .iter()
            .map(|s| Super {
                ahead: inter_ix.get(s.a.as_str()).copied(),
                slot: s.a_n,
                behind: inter_ix.get(s.b.as_str()).copied(),
                len: s.length,
                speed: s.speed,
                lanes: s.lanes_ab,
                flags: s.lanes_ba,
                roads: Vec::new(),
            })
            .collect();
        let mut roads = Vec::new();
        let mut grid: HashMap<(i32, i32), Vec<(Vec3, usize, usize)>> = HashMap::new();
        for (ri, r) in j.roads.iter().enumerate() {
            let sup = sup_ix.get(r.sup.as_str()).copied().unwrap_or(usize::MAX);
            let (len, end) = (f32::from_bits(r.w128[3]), f32::from_bits(r.w128[4]));
            let pieces: Vec<Piece> = r
                .pieces
                .iter()
                .map(|p| {
                    let e = p.edges.map(Vec3::from);
                    // 0x8235c240: up = cross(e3 - e0, e2 - e1).
                    let up = (e[3] - e[0]).cross(e[2] - e[1]).try_normalize().unwrap_or(Vec3::Y);
                    Piece { c: Curve::new(p.p0, p.p1, p.t0, p.t1, p.arc.clone()), len: p.length, end: p.edge_length, e, up }
                })
                .collect();
            for (pi, p) in pieces.iter().enumerate() {
                // 0x82B04A08: two candidate points per piece.
                for q in [(p.e[0] + p.e[1] + p.e[2]) / 3.0, (p.e[1] + p.e[2] + p.e[3]) / 3.0] {
                    grid.entry(cell(q)).or_default().push((q, ri, pi));
                }
            }
            if let Some(s) = sups.get_mut(sup) {
                s.roads.push(ri);
            }
            roads.push(Road { sup, start: end - len, end, pieces });
        }
        for s in &mut sups {
            s.roads.sort_by(|a, b| roads[*a].start.total_cmp(&roads[*b].start));
        }
        let slot = |e: &JEnd| {
            let r = *road_ix.get(e.id.as_str()).filter(|_| e.a == 1)?;
            let road: &Road = &roads[r];
            (road.sup != usize::MAX).then(|| Slot { sup: road.sup, start: road.start, lanes: e.lanes.clone() })
        };
        let inters = j
            .intersections
            .iter()
            .map(|x| {
                let c = x.inner.map(Vec3::from);
                Inter {
                    lights: x.w580 != 0,
                    speed: x.speed,
                    // 0x82b0be78: up from the inner corners.
                    up: (c[3] - c[1]).cross(c[2] - c[0]).try_normalize().unwrap_or(Vec3::Y),
                    entries: std::array::from_fn(|k| slot(&x.entries[k])),
                    exits: std::array::from_fn(|k| slot(&x.exits[k])),
                    lanes: x
                        .lanes
                        .iter()
                        .map(|l| ILane {
                            c: Curve::new(l.p0, l.p1, l.t0, l.t1, l.arc.clone()),
                            speed: l.speed,
                            entry: l.entry,
                            entry_lane: l.w88,
                            exit: l.exit,
                            exit_lane: l.w96,
                        })
                        .collect(),
                }
            })
            .collect();
        Self { sups, roads, inters, grid }
    }

    /// 0x8235b7b8: the road of `sup` holding `d` (start <= d < end).
    fn road_at(&self, sup: usize, d: f32) -> Option<&Road> {
        self.sups[sup].roads.iter().map(|&r| &self.roads[r]).find(|r| r.start <= d && d < r.end)
    }

    fn piece_at(&self, sup: usize, d: f32) -> Option<(&Piece, f32)> {
        let road = self.road_at(sup, d)?;
        let local = d - road.start;
        let p = road.pieces.iter().find(|p| p.end > local).or(road.pieces.last())?;
        Some((p, local - (p.end - p.len)))
    }

    /// 0x8235c240: a lane position on a superroad. Lane 0 is the inner lane; lane `lanes` is parking.
    fn road_pose(&self, sup: usize, d: f32, lane: usize) -> Option<Pose> {
        let (p, s) = self.piece_at(sup, d)?;
        let t = p.c.param(s);
        let u = if p.len > 0.0 { s / p.len } else { 0.0 };
        let width = (p.e[1] + (p.e[3] - p.e[1]) * u).distance(p.e[0] + (p.e[2] - p.e[0]) * u);
        let f = frame(p.c.at(t), p.c.tangent(t), p.up);
        let frac = (2 * lane + 1) as f32 / (2 * self.sups[sup].lanes.max(1)) as f32;
        Some(Pose { pos: f.pos - f.left * width * (frac - 0.5), ..f })
    }

    fn nearest(&self, at: Vec3) -> Option<(usize, usize)> {
        let (cx, cz) = cell(at);
        let mut best = (CELL, None);
        for x in cx - 1..=cx + 1 {
            for z in cz - 1..=cz + 1 {
                for &(q, r, p) in self.grid.get(&(x, z)).into_iter().flatten() {
                    let d = q.distance(at);
                    if d <= best.0 {
                        best = (d, Some((r, p)));
                    }
                }
            }
        }
        best.1
    }
}

fn cell(q: Vec3) -> (i32, i32) {
    ((q.x / CELL).floor() as i32, (q.z / CELL).floor() as i32)
}

// ---------------------------------------------------------------- lights

/// One of the four light controllers (320 B, ctor 0x825c7250, program 0x825c77c8, tick 0x82330490).
/// Controller `i` serves entry slot `i`; modes 0 red, 1 amber, 2 green.
#[derive(Clone)]
struct Light {
    phases: Vec<(u8, f32)>,
    cur: usize,
    rem: f32,
}

impl Light {
    fn new(i: usize, green: f32, amber: f32, all_red: f32) -> Self {
        let phases = if i & 1 == 0 {
            vec![(0, all_red), (0, green + amber), (0, all_red), (2, green), (1, amber)]
        } else {
            vec![(0, all_red), (2, green), (1, amber), (0, all_red), (0, green + amber)]
        };
        Self { rem: phases[0].1, phases, cur: 0 }
    }
    fn tick(&mut self) {
        self.rem -= DT;
        if self.rem < 0.0 {
            self.cur = (self.cur + 1) % self.phases.len();
            // The overshoot carries into the next phase, except at the cycle wrap.
            let carry = if self.cur == 0 { 0.0 } else { self.rem };
            self.rem = self.phases[self.cur].1 + carry;
        }
    }
    fn mode(&self) -> u8 {
        self.phases[self.cur].0
    }
}

// ---------------------------------------------------------------- census

fn num(m: &HashMap<String, Value>, k: &str) -> f32 {
    m.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

/// Per-entity tuning: driver profile 62B1C1B37ED2E2CA and behaviour 02AA3F538D6C188A. Missing values read 0
/// (0x82e57998).
#[derive(Clone, Copy, Default)]
struct Tune {
    slow: f32,
    fast: f32,
    chg_k: f32,
    chg_f: f32,
    stop_k: f32,
    odo: f32,
    amin: f32,
    amax: f32,
    ramp: f32,
    follow_kmh: f32,
    follow_dv: f32,
    follow_dist: f32,
    follow_brake: f32,
    hard: f32,
    alarm: f32,
    sk_k: f32,
    gap_k: f32,
    blink: f32,
    p_fast: f32,
    p_slow: f32,
    p_normal: f32,
    list_b: f32,
    runner: f32,
    smart_exit: f32,
    smart_lane: f32,
    change: f32,
    status_on: f32,
    aware: f32,
    b80: f32,
    slow_kmh: f32,
    pull: f32,
    aware_kmh: f32,
    runner_kmh: f32,
    honk_slow: f32,
    honk_skater: f32,
    parked: f32,
    alarm_hit: f32,
    /// The car's audio row key (record `+24`, read by SFXObj_TrafficEngine 0x824D6110).
    audio: u64,
}

impl Tune {
    fn new(p: &HashMap<String, Value>, b: &HashMap<String, Value>) -> Self {
        Self {
            slow: num(p, "3DF4F2D4CA631369"),        // +24
            fast: num(p, "E0C564403296C5F8"),        // +32
            chg_k: num(p, "758229215579C6D1"),       // +36 -> 3656
            chg_f: num(p, "328B9F4685A14018"),       // +40 -> 3676
            stop_k: num(p, "2ACE3FEE16D6F7A2"),      // +44 -> 3644 STOP
            odo: num(p, "90AB56A5DDCF2A3A"),         // +48
            amin: num(p, "5EFF87155A4EABE1"),        // +52
            amax: num(p, "CA49D85FA629F7E7"),        // +56
            ramp: num(p, "03F46E22E52C0002"),        // +60, per tick
            follow_kmh: num(p, "D20826F15FB15A2E"),  // 0x82b0c210
            follow_dv: num(p, "256A412E350A2659"),
            follow_dist: num(p, "33466832D8178EAF"), // also 0x82b0d6f0
            follow_brake: num(p, "3AB7FC7CF7A17C81"),
            hard: num(p, "DD49E3C2B833A723"),
            alarm: num(p, "E199FC7CEA222809"),       // 0x82b08988
            sk_k: num(p, "30313EA6F282C874"),        // -> 3652
            gap_k: num(p, "73A1989BB90C93BA"),       // -> 3648 GAP
            blink: num(p, "7A525925B4B604BA"),       // 0x82b0cbb8
            p_fast: num(b, "3C32BAC5E516710F"),      // +0
            p_slow: num(b, "85C11600C2914F96"),      // +4
            p_normal: num(b, "90FC5AB44797501B"),    // +8
            list_b: num(b, "B5C60C1D43899F74"),      // +12 -> 4385&4
            runner: num(b, "962D641F23FE1E20"),      // +16 -> 4385&0x10
            smart_exit: num(b, "20E9C6487FDDBDE8"),  // +20 -> 4385&2
            smart_lane: num(b, "7C6B48BD9ADF8E6E"),  // +24
            change: num(b, "52CF2CF346699CA1"),      // +28
            status_on: num(b, "50E084076390A573"),   // +36 -> 4385&1
            aware: num(b, "C423B7E0449F8D0E"),       // -> 4385&8
            b80: num(b, "99083122A1B7116A"),         // -> 4386&0x80
            slow_kmh: num(b, "40540E1A0A5D447A"),    // 0x82b028a0
            pull: num(b, "559BA807F95FF93E"),
            aware_kmh: num(b, "E939890E1F14DC0B"),   // 0x82b0cdb8
            runner_kmh: num(b, "F142ABBFBEDA71E2"),  // 0x82b02bf8
            honk_slow: num(b, "BC1A827C21919C3B"),   // 0x82b0c8b0
            honk_skater: num(b, "FE83E2E0A19A9AFE"),
            parked: num(b, "988BB0F6F043EB3D"),      // 0x82b08860
            alarm_hit: num(p, "543475921FD9E04A"),   // 0x82b0a6b8
            audio: 0,
        }
    }
}

struct Kind {
    /// Index into `Sim::models` per variant; `None` when the model was not exported.
    variants: Vec<Option<usize>>,
    palettes: Vec<Palette>,
    tune: Tune,
    hom: i32,
}

struct Census {
    tiles: Vec<Tile>,
    /// Skate 1 paints the group on its own layer (4); Skate 2 names it in the census row.
    group_tiles: Option<Vec<Tile>>,
    rows: HashMap<u64, (u32, Option<u64>)>,
    groups: HashMap<u64, Vec<(f32, Vec<u64>)>>,
    kinds: HashMap<u64, Kind>,
}

struct Model {
    path: String,
    len: f32,
    half_width: f32,
    wheel_radius: f32,
    /// The bounding box in the car's frame: centre and half extents.
    centre: Vec3,
    half: Vec3,
    /// The collision boxes in the car's frame (centre, half extents), front to back or back to front along Z: one
    /// per stretch of the body with one roof height, so the hood and trunk stand as low as they are drawn.
    boxes: Vec<(Vec3, Vec3)>,
}

/// Slices along the car's length that [`body_boxes`] measures.
const SLICES: usize = 10;
/// Neighbouring slices whose tops differ by less than this share a box.
const STEP: f32 = 0.12;

/// Length (Z extent), half width, wheel radius, the (min, max) box from the GLB's accessor bounds (0x8235c7f0:
/// L = body extents.x, which is the length axis) and the body's collision boxes ([`body_boxes`]). ponytail: assumes
/// the exporter's flat Vehicle_Root -> parts tree.
fn glb_metrics(path: &Path) -> Option<(f32, f32, f32, Vec3, Vec3, Vec<(Vec3, Vec3)>)> {
    let b = std::fs::read(path).ok()?;
    let n = u32::from_le_bytes(b.get(12..16)?.try_into().ok()?) as usize;
    let j: Value = serde_json::from_slice(b.get(20..20 + n)?).ok()?;
    // The BIN chunk follows the JSON chunk's 8-byte header and data.
    let bin = b.get(20 + n + 8..).unwrap_or_default();
    let v3 = |v: &Value| Vec3::new(v[0].as_f64().unwrap_or(0.0) as f32, v[1].as_f64().unwrap_or(0.0) as f32, v[2].as_f64().unwrap_or(0.0) as f32);
    let (mut lo, mut hi, mut radius) = (Vec3::MAX, Vec3::MIN, 0.0f32);
    let mut body = Vec::new();
    for node in j["nodes"].as_array()? {
        let Some(m) = node["mesh"].as_u64() else { continue };
        let t = if node["translation"].is_array() { v3(&node["translation"]) } else { Vec3::ZERO };
        for prim in j["meshes"][m as usize]["primitives"].as_array()? {
            let a = &j["accessors"][prim["attributes"]["POSITION"].as_u64()? as usize];
            let (min, max) = (v3(&a["min"]), v3(&a["max"]));
            let wheel = node["name"].as_str().is_some_and(|s| s.to_ascii_lowercase().contains("wheel"));
            // The body box stops at the chassis floor: with the wheels it reached the ground, so a hippie jump's
            // board couldn't pass under (user 10-14, bug #6). INFERRED: retail's vehicle hull (UNTRACED) is the body.
            lo = if wheel { Vec3::new(lo.x.min(t.x + min.x), lo.y, lo.z.min(t.z + min.z)) } else { lo.min(t + min) };
            hi = hi.max(t + max);
            if wheel {
                radius = radius.max(0.5 * (max.y - min.y));
            } else if let Some(points) = positions(&j, bin, a) {
                // Unreadable: no slice gets a top, and the body is the one bounding box.
                body.extend(points.into_iter().map(|p| p + t));
            }
        }
    }
    (hi.z > lo.z).then(|| (hi.z - lo.z, 0.5 * (hi.x - lo.x), radius, lo, hi, body_boxes(&body, lo, hi)))
}

/// A float VEC3 accessor's values (the exporter writes no other position type).
fn positions(j: &Value, bin: &[u8], a: &Value) -> Option<Vec<Vec3>> {
    if a["componentType"].as_u64()? != 5126 {
        return None;
    }
    let view = &j["bufferViews"][a["bufferView"].as_u64()? as usize];
    let stride = view["byteStride"].as_u64().unwrap_or(12) as usize;
    let start = (view["byteOffset"].as_u64().unwrap_or(0) + a["byteOffset"].as_u64().unwrap_or(0)) as usize;
    (0..a["count"].as_u64()? as usize)
        .map(|i| {
            let f = |k: usize| Some(f32::from_le_bytes(bin.get(start + i * stride + 4 * k..)?.get(..4)?.try_into().ok()?));
            Some(Vec3::new(f(0)?, f(1)?, f(2)?))
        })
        .collect()
}

/// The body as boxes along its length: the drawn body's top in each of [`SLICES`] slices, neighbours within [`STEP`]
/// merged, each box from the chassis floor to its stretch's top, the car's full width. One box to the roof put an
/// invisible wall over the hood and trunk (0.3-0.6 m on the Skate 3 cars). INFERRED: retail's hull is UNTRACED.
fn body_boxes(body: &[Vec3], lo: Vec3, hi: Vec3) -> Vec<(Vec3, Vec3)> {
    let slice = |z: f32| (((z - lo.z) / (hi.z - lo.z) * SLICES as f32) as usize).min(SLICES - 1);
    let mut tops = [f32::MIN; SLICES];
    for p in body {
        let i = slice(p.z);
        tops[i] = tops[i].max(p.y);
    }
    let mut runs: Vec<(usize, usize, f32)> = Vec::new();
    for (i, &top) in tops.iter().enumerate() {
        match runs.last_mut() {
            // An empty slice (no body vertex) joins the stretch it's in.
            Some((_, end, t)) if top == f32::MIN || (top - *t).abs() < STEP || *t == f32::MIN => {
                *end = i;
                *t = t.max(top);
            }
            _ => runs.push((i, i, top)),
        }
    }
    let edge = |i: usize| lo.z + (hi.z - lo.z) * i as f32 / SLICES as f32;
    runs.into_iter()
        .map(|(start, end, top)| {
            let top = if top == f32::MIN { hi.y } else { top };
            let (a, b) = (Vec3::new(lo.x, lo.y, edge(start)), Vec3::new(hi.x, top, edge(end + 1)));
            (0.5 * (a + b), 0.5 * (b - a))
        })
        .collect()
}

// ---------------------------------------------------------------- the agent

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Following,
    Changing,
    PullingOver,
    Parked,
    PullingOut,
    Passing,
}

/// veh4356: on a road (1) or an intersection lane (0).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum On {
    Road,
    Inter,
}

/// A lane change, pull-over or pull-out (0x82b0bfe8): d0 -> d1 from lane `from` to `to`; `s` is veh3632.
struct Change {
    d0: f32,
    d1: f32,
    from: usize,
    to: usize,
    c: Curve,
    len: f32,
    s: f32,
}

struct Car {
    serial: u64,
    /// Skate 1 colorize (red, blue) colours ([`colorize`]); None: the texture as exported.
    paint: Option<([f32; 3], [f32; 3])>,
    model: usize,
    len: f32,
    hw: f32,
    t: Tune,
    state: State,
    on: On,
    sup: usize,
    lane: usize,
    d: f32,
    s: f32,
    /// veh4372 with the intersection it indexes.
    rec: Option<(usize, usize)>,
    /// veh4128: intersection registered with.
    reg: Option<usize>,
    speed: f32,
    accel: f32,
    f3420: u32,
    lamps: u32,
    f4385: u32,
    f4386: u32,
    f4387: u32,
    status: u32,
    over: f32,
    odo: f32,
    park_at: f32,
    target: usize,
    plan: u32,
    near: f32,
    kind: u32,
    t3688: f32,
    t3692: f32,
    t3696: f32,
    t3700: f32,
    brake_t: f32,
    blink_t: f32,
    amax: f32,
    speed_off: f32,
    stop: f32,
    gap: f32,
    sk_gap: f32,
    chg: f32,
    change: Option<Change>,
    /// veh4337: still moving along `change`.
    changing: bool,
    seen: Vec3,
    sk_dist: f32,
    sk_speed: f32,
    pose: Pose,
    travelled: f32,
    /// Hall of Meat vehicle type (entity row -> 8B670C504A363A7C, 0x82bb8a90).
    hom: i32,
    /// veh4368: the players that hit the car this frame (bit 0 = the local skater; 0x823ad3f0 clears it).
    hit: u32,
}

impl Car {
    /// 0x82b02b58: lane occupancy per car.
    fn occ(&self) -> f32 {
        self.len + self.gap
    }
    /// 0x82b02bf8 (4387&0x80 setter UNTRACED, never set).
    fn ignores_lights(&self) -> bool {
        self.f4385 & 0x10 != 0 && self.speed * 3.6 > self.t.runner_kmh
    }
    /// nav.vfunc96 0x82b02cb0 (4386&2 setter UNTRACED, never set).
    fn waiting(&self) -> bool {
        self.f3420 & 4 != 0
    }
    /// 0x82b0cdb8 / 0x82b0dd08: inside the quad from the car centre (± half width) `reach` metres ahead, in xz.
    fn in_quad(&self, p: Vec3, reach: f32) -> bool {
        let v = (p - self.pose.pos).with_y(0.0);
        let (f, l) = (self.pose.fwd.with_y(0.0).normalize_or_zero(), self.pose.left.with_y(0.0).normalize_or_zero());
        let along = v.dot(f);
        (0.0..=reach).contains(&along) && v.dot(l).abs() <= self.hw
    }
}

/// 0x82b0d380.
fn obstacle(c: &mut Car, dist: f32, gap: f32, kind: u32, best: &mut f32, best_gap: &mut f32) {
    if dist < c.near {
        c.near = dist;
    }
    if dist <= c.speed + gap {
        if dist <= gap {
            c.f4386 &= !4;
        }
        if *best > dist {
            *best = dist;
            *best_gap = gap;
            c.kind = if kind == 1 || kind == 5 { 1 } else { 3 };
        }
    }
    if c.f4386 & 4 != 0 {
        c.t3688 = 0.0;
    } else if c.kind == 3 && c.speed * 3.6 < c.t.slow_kmh {
        c.t3688 += DT;
    }
}

/// 0x82b0d530 (list-A radius 3584 = 0).
fn skater_obstacle(c: &mut Car, best: &mut f32, best_gap: &mut f32) {
    if c.f4386 & 8 != 0 {
        let d = c.seen.distance(c.pose.pos) - 0.5 * c.len;
        if *best > d {
            *best = d;
            *best_gap = c.sk_gap;
            c.kind = 4;
        }
    }
    if c.kind == 4 && c.f4386 & 4 != 0 && c.speed * 3.6 < c.t.slow_kmh {
        c.t3692 += DT;
    } else {
        c.t3692 = 0.0;
    }
}

/// 0x82b0c210.
fn speed_ctl(c: &mut Car, best: f32, best_gap: f32) {
    if c.f3420 & 8 != 0 {
        return;
    }
    if c.kind == 0 {
        c.accel = if c.accel < 0.0 { 0.0 } else { c.accel + c.t.ramp };
        let mut amax = c.amax;
        if c.f4387 & 0x10 != 0 && c.speed * 3.6 > c.t.follow_kmh && c.sk_speed * 3.6 > c.t.follow_kmh {
            amax = 0.0;
            let tv = (c.sk_speed - c.t.follow_dv * 0.277_777_8).max(0.0);
            // ponytail: veh3712 is always -1 (its 4386&1 setter is UNTRACED) and 4387&8 (the skater flag) 0.
            if c.sk_dist < c.t.follow_dist {
                c.accel = (tv * tv - c.speed * c.speed) / (2.0 * c.t.follow_brake + 0.001);
            }
        }
        if c.accel > amax {
            c.accel = amax;
        }
    }
    if best_gap < MAX && best <= c.speed + best_gap {
        if best <= best_gap {
            if c.kind == 4 {
                c.accel = -c.speed;
                if c.accel < -c.t.hard {
                    c.f3420 |= 8;
                }
            } else {
                c.accel = 0.0;
                c.speed = 0.0;
            }
        } else {
            c.accel = -(c.speed * c.speed) / ((best - best_gap) * 2.0 + 0.001);
        }
    }
    // A hit ahead of the car (4385&0x20, set by the contact callback 0x82b0a6b8): take all the speed off.
    if c.f4385 & 0x20 != 0 && c.accel > -c.speed {
        c.accel = -c.speed;
        if c.accel < -c.t.hard {
            c.f3420 |= 8;
        }
    }
    if c.kind == 2 {
        c.f4387 |= 0x20;
    }
}

/// 0x82b0c740. The limit is soft: past it accel drops to 0 and the per-tick ramp re-adds, as retail.
fn integrate_speed(c: &mut Car, road_speed: f32) {
    let limit = (c.speed_off + 1.0) * road_speed; // + 3668 only with 4386&2 (never set)
    c.speed += c.accel * DT;
    if c.accel >= 0.0 && c.speed > limit {
        c.accel = 0.0;
    }
    if c.accel <= 0.001 && c.speed <= 0.01 {
        c.accel = 0.0;
        c.speed = 0.0;
        c.f3420 &= !8;
        c.f4385 &= !0x20;
    }
}

/// 0x82b0c8b0: veh3416, what the horn/voice layer (Mission B) reads.
fn status(c: &mut Car, skater: Vec3) {
    c.status = if c.f3420 & 0x80 != 0 {
        if c.f3420 & 0x10 != 0 { 6 } else { 0 }
    } else if c.f4385 & 1 == 0 {
        0
    } else if c.kind == 5 {
        3
    } else if c.t3688 > c.t.honk_slow {
        4 | u32::from(c.f4385 & 2 == 0)
    } else if c.f4386 & 8 != 0 {
        if c.t3692 > c.t.honk_skater {
            2
        } else if 2.5 * c.len > skater.distance(c.pose.pos) && c.speed * 3.6 > c.t.slow_kmh {
            1
        } else {
            0
        }
    } else {
        0
    };
}

/// 0x82b0cbb8: veh4384. 0x02 brake, 0x04 reverse, 0x08 left, 0x10 right, 0x80 blink phase.
fn lamps(c: &mut Car) {
    if c.f3420 & 0x80 == 0 {
        c.lamps &= !4; // reverse = 3420&0x40, never set
        if c.speed > 0.0 && c.accel < 0.0 {
            c.brake_t += DT;
        } else if c.accel > 0.0 {
            c.brake_t = 0.0;
        }
        c.lamps = if c.brake_t > 0.5 { c.lamps | 2 } else { c.lamps & !2 };
    } else {
        c.lamps = (c.lamps & 0x80) | if c.f3420 & 0x10 != 0 { 0x18 } else { 0 };
    }
    if c.lamps & 0x18 == 0 {
        c.blink_t = 0.0;
        c.lamps &= !0x80;
    } else {
        c.blink_t += DT;
        if c.blink_t > c.t.blink {
            c.blink_t = 0.0;
            c.lamps ^= 0x80;
        }
    }
}

pub(crate) struct Sim {
    net: Network,
    census: Census,
    models: Vec<Model>,
    cars: Vec<Option<Car>>,
    /// Superroad lane lists sorted by d at insert (0x82bf3a70): first = rearmost, last = front.
    lists: Vec<Vec<Vec<usize>>>,
    occ: Vec<Vec<f32>>,
    /// Parking reservations (spot, car), ascending (0x82bf3c40).
    parking: Vec<Vec<(f32, usize)>>,
    /// Intersection lane-record lists: front = newest registration.
    recs: Vec<Vec<VecDeque<usize>>>,
    lights: [Light; 4],
    rng: u32,
    frame: u64,
    filled: bool,
    serial: u64,
    /// The audio handles' base: each simulation's own, so a map's cars never reuse the last map's
    /// (CSTATEMGR_TrafficCar keeps a car's sound state by handle).
    handles: u32,
    skater: Vec3,
    skater_vel: Vec3,
    /// The gameplay camera (eye, forward), set before each tick; None: no camera, spawn anywhere (retail).
    pub(crate) view: Option<(Vec3, Vec3)>,
    /// Most live cars: Skate 3's budget A (tu3 0x826B83C8 spawns only while A > the live vehicle count; the ramp
    /// 0x826BD9C8 caps A at 23). Unlimited on the Skate 1 / 2 maps (their exes' cap is untraced).
    live_cap: usize,
    /// The vehicles layer's density (+0x8C, 0x826B7010): Free Play's Traffic setting, else 1.0.
    pub(crate) density: f32,
}

/// The census row's live-car cap: its count x the density, floored (tu3 0x826B8A28), within budget A.
fn census_cap(count: u32, density: f32, live_cap: usize) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let scaled = (count as f32 * density).floor().max(0.) as usize;
    scaled.min(live_cap)
}

impl Sim {
    /// `(data, vehicles)`: see [`dirs`].
    pub(crate) fn load(asset_root: &Path, (dir, vehicles): (&str, &str)) -> Result<Self, String> {
        let read = |name: &str| std::fs::read(asset_root.join(dir).join(name)).map_err(|e| format!("{dir}/{name}: {e}"));
        let roads: JRoads = serde_json::from_slice(&read("roads.json")?).map_err(|e| e.to_string())?;
        let census: JCensus = serde_json::from_slice(&read("census.json")?).map_err(|e| e.to_string())?;
        let mut sim = Self::new(&roads, &census, |m| {
            let path = format!("{vehicles}/{m}.glb");
            glb_metrics(&asset_root.join(&path)).map(|(len, half_width, wheel_radius, lo, hi, boxes)| Model {
                path,
                len,
                half_width,
                wheel_radius,
                centre: 0.5 * (lo + hi),
                half: 0.5 * (hi - lo),
                boxes,
            })
        });
        if dir.starts_with("private/traffic/skate3/") {
            sim.live_cap = SKATE3_LIVE_CARS;
        }
        Ok(sim)
    }

    fn new(roads: &JRoads, j: &JCensus, mut model: impl FnMut(&str) -> Option<Model>) -> Self {
        let net = Network::new(roads);
        let mut models = Vec::new();
        let mut by_name: HashMap<String, Option<usize>> = HashMap::new();
        let kinds = j
            .entities
            .iter()
            .map(|(k, e)| {
                let variants = e
                    .variants
                    .iter()
                    .map(|v| {
                        *by_name.entry(v.model.clone()).or_insert_with(|| {
                            model(&v.model).map(|m| {
                                models.push(m);
                                models.len() - 1
                            })
                        })
                    })
                    .collect();
                (hex(k), Kind { variants, palettes: e.variants.iter().map(|v| palette(&v.appearance)).collect(), tune: Tune { audio: e.audio.as_deref().map_or(0, hex), ..Tune::new(&e.profile, &e.behaviour) }, hom: e.hom })
            })
            .collect();
        let census = Census {
            tiles: j.painter.iter().map(tile).collect(),
            group_tiles: j.group_painter.as_ref().map(|g| g.iter().map(tile).collect()),
            rows: j.census.iter().map(|(k, r)| (hex(k), (r.count, r.group.as_deref().map(hex)))).collect(),
            groups: j.groups.iter().map(|(k, g)| (hex(k), g.iter().map(|e| (e.weight, e.entities.iter().map(|x| hex(x)).collect())).collect())).collect(),
            kinds,
        };
        // Livingworld row 835587ECAD20014E (0x825c77c8): green A, amber B, all-red C.
        let lw = j.livingworld.get("Hash_835587ECAD20014E");
        let lv = |k: &str| lw.map_or(0.0, |m| num(m, k));
        let (a, b, c) = (lv("3836077DEFB6F008"), lv("CFCA94D67E0B234E"), lv("2C0CE3766A0074CD"));
        Self {
            lists: net.sups.iter().map(|s| vec![Vec::new(); s.lanes + 1]).collect(),
            occ: net.sups.iter().map(|s| vec![0.0; s.lanes + 1]).collect(),
            parking: net.sups.iter().map(|_| Vec::new()).collect(),
            recs: net.inters.iter().map(|i| vec![VecDeque::new(); i.lanes.len()]).collect(),
            lights: std::array::from_fn(|i| Light::new(i, a, b, c)),
            net,
            census,
            models,
            cars: Vec::new(),
            rng: 0x5EED_2009, // Ruling: one seeded LCG for every draw (retail rng 0x82333840 state UNTRACED).
            frame: 0,
            filled: false,
            live_cap: usize::MAX,
            density: 1.0,
            serial: 0,
            handles: {
                static SIMS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                SIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) << 20
            },
            skater: Vec3::ZERO,
            skater_vel: Vec3::ZERO,
            view: None,
        }
    }

    fn next(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.rng
    }
    /// int(rng * 2.3283064e-08) % n: the retail list pick (0x823335DC).
    fn pick(&mut self, n: usize) -> usize {
        ((f64::from(self.next()) / 4_294_967_296.0 * 100.0) as usize) % n.max(1)
    }
    /// 0x825e4900: chance*100 >= rand%100 + 1.
    fn roll(&mut self, chance: f32) -> bool {
        chance * 100.0 >= (self.next() % 100 + 1) as f32
    }

    fn car(&self, i: usize) -> Option<&Car> {
        self.cars.get(i)?.as_ref()
    }
    pub(crate) fn count(&self) -> usize {
        self.cars.iter().flatten().count()
    }

    fn insert(&mut self, sup: usize, lane: usize, me: usize, d: f32, occ: f32) {
        let at = self.lists[sup][lane].iter().position(|&v| self.car(v).is_some_and(|o| o.d > d)).unwrap_or(self.lists[sup][lane].len());
        self.lists[sup][lane].insert(at, me);
        self.occ[sup][lane] += occ;
    }
    fn remove(&mut self, sup: usize, lane: usize, me: usize, occ: f32) {
        if let Some(p) = self.lists[sup][lane].iter().position(|&v| v == me) {
            self.lists[sup][lane].remove(p);
            self.occ[sup][lane] -= occ;
        }
    }
    /// 0x82bf3990: the car ahead in a lane list (none for the front car).
    fn ahead_in(&self, sup: usize, lane: usize, me: usize) -> Option<&Car> {
        let l = &self.lists[sup][lane];
        let p = l.iter().position(|&v| v == me)?;
        self.car(*l.get(p + 1)?)
    }

    /// Clearing veh4372 also takes the car off that record (retail leaves the handle; see the rulings).
    fn drop_rec(&mut self, me: usize, c: &mut Car) {
        if let Some((n, r)) = c.rec.take() {
            self.recs[n][r].retain(|&v| v != me);
        }
        c.reg = None;
        c.f4386 &= !0x40;
    }

    fn forget(&mut self, me: usize, c: &Car) {
        for lane in 0..self.lists[c.sup].len() {
            self.remove(c.sup, lane, me, c.occ());
        }
        self.parking[c.sup].retain(|&(_, v)| v != me);
        if let Some((n, r)) = c.rec {
            self.recs[n][r].retain(|&v| v != me);
        }
    }

    /// 0x82bf33d0: room for a car of length `len` at `d` in `lane`.
    fn gap(&self, sup: usize, d: f32, len: f32, speed: f32, lane: usize, strict: bool) -> bool {
        let s = &self.net.sups[sup];
        if s.lanes == 0 || d < len || d > s.len - len || (strict && s.lanes < 2) {
            return false;
        }
        let cars: Vec<&Car> = self.lists[sup][lane].iter().filter_map(|&v| self.car(v)).collect();
        let a = cars.iter().position(|o| o.d > d);
        if let Some(o) = a.map(|i| cars[i]) {
            if d + len + speed > o.d - 0.5 * o.len {
                return false;
            }
        }
        let b = match a {
            Some(0) => None,
            Some(i) => Some(cars[i - 1]),
            None => cars.last().copied(),
        };
        b.is_none_or(|o| o.d + o.speed + o.len <= d - 0.5 * len)
    }

    /// 0x82bf35e0: the lane a car moves to.
    fn pick_lane(&mut self, sup: usize, lane: usize, smart: bool) -> usize {
        let lanes = self.net.sups[sup].lanes;
        let cands: Vec<usize> = [lane.checked_sub(1), Some(lane + 1)].into_iter().flatten().filter(|&l| l < lanes).collect();
        if cands.is_empty() {
            return lane;
        }
        if smart {
            let mut best = (self.occ[sup][lane], lane);
            for &l in &cands {
                if self.occ[sup][l] < best.0 {
                    best = (self.occ[sup][l], l);
                }
            }
            best.1
        } else {
            let i = self.pick(cands.len());
            cands[i]
        }
    }

    /// 0x82bf37a8: a free parking spot ahead, or -1.
    fn find_spot(&self, sup: usize, d: f32, len: f32, speed: f32) -> f32 {
        let s = &self.net.sups[sup];
        let mut far = 0.75 * s.len;
        if self.parking[sup].is_empty() {
            return 0.5 * s.len;
        }
        let min = d + 1.5 * len + speed;
        for &(spot, v) in &self.parking[sup] {
            let ol = self.car(v).map_or(0.0, |o| o.len);
            let front = spot + 0.5 * ol;
            if front >= far {
                continue;
            }
            let cand = (front + far) * 0.5;
            if cand > min && far - front > len + 5.0 {
                return cand;
            }
            far = front - ol;
        }
        -1.0
    }

    fn change_curve(&self, sup: usize, d0: f32, d1: f32, from: usize, to: usize) -> Option<Change> {
        let (a, b) = (self.net.road_pose(sup, d0, from)?, self.net.road_pose(sup, d1, to)?);
        let k = (d1 - d0) * 1.3;
        let c = Curve::build(a.pos, b.pos, a.fwd * k, b.fwd * k, 10);
        Some(Change { d0, d1, from, to, len: c.len(), c, s: 0.0 })
    }

    // ------------------------------------------------------------ intersection query

    /// 0x82bf10b0 -> (0 go, 1 light, 2 too fast, 3 yield, 4 no room; outKind 2 = yielding to a waiting car).
    fn query(&self, me: usize, c: &Car, n: usize, r: usize, check_light: bool) -> (u32, u32) {
        let it = &self.net.inters[n];
        let ln = &it.lanes[r];
        let (e, el, x, xl) = (ln.entry, ln.entry_lane, ln.exit, ln.exit_lane);
        let (Some(en), Some(ex)) = (&it.entries[e % 4], &it.exits[x % 4]) else { return (0, 0) };
        let remaining = self.net.sups[en.sup].len - (c.d + 0.5 * c.len);
        let mut vmax = ln.speed;
        if check_light {
            if it.lights {
                let l = &self.lights[e % 4];
                let reach = l.mode() == 2 && c.speed * l.rem >= remaining - 0.5 * c.len;
                if !reach && x != (e + 3) % 4 {
                    return (1, 0); // red; right on red allowed
                }
            } else {
                vmax = 0.1;
            }
        }
        if c.speed > vmax && remaining >= c.stop {
            return (2, 0);
        }
        if let Some(o) = self.lists[ex.sup].get(xl).and_then(|l| l.first()).and_then(|&o| self.car(o)) {
            if c.len > o.d + o.speed - 0.5 * o.len {
                return (4, 0);
            }
        }
        let recs = &self.recs[n];
        if en.lanes.get(el).is_some_and(|js| js.iter().any(|&j| j != r && !recs[j].is_empty())) {
            return (3, 0);
        }
        if let Some(v) = recs[r].iter().filter(|&&v| v != me).filter_map(|&v| self.car(v)).find(|v| v.on == On::Inter) {
            if (v.s - 0.5 * v.len) + remaining < c.gap {
                return (4, 0);
            }
        }
        let m = |k: usize| (e + k) % 4; // 1 left, 2 opposite, 3 right
        let xlanes = self.net.sups[ex.sup].lanes;
        let nl = !it.lights;
        let e88 = |from: usize, l: usize| self.e88(n, from, x, Some(l), None);
        let b88 = |from: usize, to: usize, consider: bool| self.e88(n, from, to, None, consider.then_some((c, remaining)));
        let hit = if x == m(1) {
            (0..=xl).rev().find_map(|l| e88(m(3), l).or_else(|| e88(m(2), l)))
                .or_else(|| b88(m(1), m(3), nl))
                .or_else(|| b88(m(1), m(2), nl))
                .or_else(|| b88(m(3), e, nl))
                .or_else(|| b88(m(2), e, true))
                .or_else(|| b88(m(2), m(3), true))
        } else if x == m(3) {
            if xl >= xlanes {
                return (0, 0);
            }
            (xl..xlanes).find_map(|l| e88(m(1), l).or_else(|| e88(m(2), l)))
        } else {
            (xl..xlanes).find_map(|l| e88(m(1), l))
                .or_else(|| (0..=xl).rev().find_map(|l| e88(m(3), l)))
                .or_else(|| b88(m(2), m(3), true))
                .or_else(|| b88(m(1), m(3), nl))
                .or_else(|| b88(m(3), m(1), nl))
                .or_else(|| b88(m(3), e, nl))
        };
        hit.map_or((0, 0), |k| (3, k))
    }

    /// 0x82bf0e88 (one exit lane) / 0x82bf0b88 (`lane` None; `approach` = also registered cars still on their
    /// road that reach the line first, ties to the lower handle).
    fn e88(&self, n: usize, from: usize, to: usize, lane: Option<usize>, approach: Option<(&Car, f32)>) -> Option<u32> {
        let it = &self.net.inters[n];
        for (j, ln) in it.lanes.iter().enumerate() {
            if ln.entry != from || ln.exit != to || lane.is_some_and(|l| l != ln.exit_lane) {
                continue;
            }
            for v in self.recs[n][j].iter().filter_map(|&v| self.car(v)) {
                let hit = v.on == On::Inter
                    || approach.is_some_and(|(me, mine)| {
                        let s = &self.net.sups[v.sup];
                        let f = s.len - v.d - 0.5 * v.len;
                        v.on == On::Road && s.ahead == Some(n) && (f < mine || (f == mine && me.serial > v.serial))
                    });
                if hit {
                    return Some(if v.waiting() { 2 } else { 0 });
                }
            }
        }
        None
    }

    // ------------------------------------------------------------ Vehicle.xml graph

    /// 0x8233aaa0 over Vehicle.xml: the first true transition (XML order) fires its hook.
    fn transitions(&mut self, me: usize, c: &mut Car) {
        match c.state {
            State::Following => {
                if c.on == On::Inter {
                    // ToPassingIntersection 0x82b09600
                    if c.f4386 & 0x20 != 0 {
                        self.parking[c.sup].retain(|&(_, v)| v != me);
                        c.f4386 &= !0x20;
                    }
                    c.state = State::Passing;
                } else if self.need_register(me, c) {
                    self.register(me, c);
                } else if self.need_unregister(c) {
                    // UnregisterFromIntersection 0x82b09b78
                    self.drop_rec(me, c);
                    c.f3420 &= !4;
                } else if self.need_change(c) {
                    self.to_changing(me, c);
                } else if self.need_pull_over(c) {
                    // ToPullingOver 0x82b09118
                    let to = self.net.sups[c.sup].lanes;
                    if let Some(k) = self.change_curve(c.sup, c.d, c.park_at, c.lane, to) {
                        c.change = Some(k);
                        c.changing = true;
                        self.insert(c.sup, to, me, c.d, c.occ());
                        c.lamps = (c.lamps & !0x18) | 0x10;
                        c.state = State::PullingOver;
                    }
                }
            }
            // IsLaneChangePassageComplete 0x82b08a60 -> FromChangingLaneToFollowingLane 0x82b096b0
            State::Changing | State::PullingOut => {
                if c.change.as_ref().is_some_and(|k| k.s >= k.len) {
                    if let Some(k) = c.change.take() {
                        self.remove(c.sup, k.from, me, c.occ());
                        c.lane = k.to;
                    }
                    c.changing = false;
                    c.odo = 0.0;
                    c.plan = 0;
                    c.park_at = -1.0;
                    self.drop_rec(me, c);
                    c.f4387 &= !0x20;
                    c.lamps &= !0x18;
                    c.state = State::Following;
                }
            }
            // -> ToStayingParked 0x82b09298 (the car stays in the parking lane list and the reservation)
            State::PullingOver => {
                if c.change.as_ref().is_some_and(|k| k.s >= k.len) {
                    if let Some(k) = c.change.take() {
                        self.remove(c.sup, k.from, me, c.occ());
                        c.lane = k.to;
                    }
                    c.changing = false;
                    c.t3696 = 0.0;
                    self.drop_rec(me, c);
                    c.lamps = 0;
                    c.park_at = -1.0;
                    c.f3420 |= 0x80; // state enter
                    c.state = State::Parked;
                }
            }
            State::Parked => {
                if c.t3700 > c.t.alarm {
                    c.f3420 &= !0x10; // StopAlarming 0x82b08988 / 0x82b099e8
                } else if c.f3420 & 0x10 == 0 && c.t3696 > c.t.parked && self.net.road_at(c.sup, c.d + c.chg).is_some() {
                    // IsRequiredToPullOut 0x82b08860 -> ToPullingOut 0x82b09400
                    let to = c.lane.saturating_sub(1);
                    if let Some(k) = self.change_curve(c.sup, c.d, c.d + c.chg, c.lane, to) {
                        c.f3420 &= !0x80; // state exit
                        c.change = Some(k);
                        c.changing = true;
                        self.parking[c.sup].retain(|&(_, v)| v != me);
                        c.f4386 &= !0x20;
                        self.insert(c.sup, to, me, c.d, c.occ());
                        c.lamps = (c.lamps & !0x18) | 0x08;
                        c.state = State::PullingOut;
                    }
                }
            }
            State::Passing => {
                if c.on == On::Road {
                    // IsIntersectionPassageComplete 0x82b08aa8 -> FromPassing 0x82b098c8; exit 0x82b08190
                    c.over = 0.0;
                    c.park_at = -1.0;
                    c.f4385 |= 0x40;
                    c.plan = 0;
                    c.odo = 0.0;
                    c.lamps &= !0x18;
                    c.state = State::Following;
                }
            }
        }
    }

    /// 0x82b08ae8: the front car of its lane, within a stopping distance of the line.
    fn need_register(&self, me: usize, c: &Car) -> bool {
        c.f4386 & 0x40 == 0 && self.lists[c.sup][c.lane].last() == Some(&me) && c.d + 0.5 * c.len + c.speed >= self.net.sups[c.sup].len - c.stop
    }

    /// RegisterToIntersection 0x82b09a00.
    fn register(&mut self, me: usize, c: &mut Car) {
        let Some((n, r)) = c.rec else { return };
        if self.net.sups[c.sup].ahead != Some(n) {
            return; // stale record (see the rulings); despawned at the road end
        }
        c.reg = Some(n);
        c.f4386 |= 0x40;
        self.recs[n][r].push_front(me);
        if c.ignores_lights() && self.query(me, c, n, r, true).0 == 1 {
            c.f3420 |= 4; // 0x82b09b1c
        }
    }

    /// 0x82b08d08.
    fn need_unregister(&self, c: &Car) -> bool {
        let s = &self.net.sups[c.sup];
        c.f4386 & 0x40 != 0 && c.reg == s.behind && c.d >= c.stop && c.d + c.stop < s.len
    }

    /// IsRequiredToChangeLane 0x82b08430.
    fn need_change(&self, c: &Car) -> bool {
        let lc = c.len * c.t.chg_f + c.speed;
        c.plan == 1
            && c.speed > 0.5 * lc
            && c.near > 0.5 * lc
            && c.d > c.len
            && self.net.sups[c.sup].len - c.len > c.d + lc
            && self.gap(c.sup, c.d, c.len, c.speed, c.target, true)
            && self.net.road_at(c.sup, c.d + lc).is_some()
    }

    /// ToChangingLane 0x82b08e98.
    fn to_changing(&mut self, me: usize, c: &mut Car) {
        let d1 = c.d + c.len * c.t.chg_f + c.speed;
        let Some(k) = self.change_curve(c.sup, c.d, d1, c.lane, c.target) else { return };
        c.change = Some(k);
        c.changing = true;
        self.insert(c.sup, c.target, me, c.d, c.occ());
        c.lamps = if c.target < c.lane { (c.lamps | 0x08) & !0x10 } else { (c.lamps | 0x10) & !0x08 };
        c.state = State::Changing;
    }

    /// IsRequiredToPullOver 0x82b08728.
    fn need_pull_over(&self, c: &Car) -> bool {
        c.plan == 3
            && c.park_at >= 0.0
            && c.park_at > c.d
            && c.park_at - c.d <= c.chg
            && c.speed < 1.0
            && self.net.road_at(c.sup, c.park_at).is_some()
    }

    // ------------------------------------------------------------ behaviours

    /// ContinueFollowingLane 0x82b05ab0.
    fn cont_following(&mut self, me: usize, c: &mut Car) {
        let (len, speed, ahead, slot, behind) = {
            let s = &self.net.sups[c.sup];
            (s.len, s.speed, s.ahead, s.slot, s.behind)
        };
        // Exit choice (0x82b05e60).
        if c.rec.is_none() && c.reg != behind {
            if let Some(n) = ahead {
                let opts: Vec<usize> = self.net.inters[n].entries[slot % 4].as_ref().and_then(|e| e.lanes.get(c.lane)).cloned().unwrap_or_default();
                if !opts.is_empty() {
                    let i = if c.f4385 & 2 != 0 {
                        let load = |j: usize| {
                            let ln = &self.net.inters[n].lanes[j];
                            self.net.inters[n].exits[ln.exit % 4].as_ref().map_or(MAX, |x| {
                                self.occ[x.sup].get(ln.exit_lane).copied().unwrap_or(MAX) / self.net.sups[x.sup].len
                            })
                        };
                        (0..opts.len()).fold(0, |b, k| if load(opts[k]) < load(opts[b]) { k } else { b })
                    } else {
                        self.pick(opts.len())
                    };
                    c.rec = Some((n, opts[i]));
                }
            }
        }
        c.near = MAX;
        c.kind = 0;
        c.f4386 |= 4;
        let (mut best, mut bg) = (MAX, MAX);
        if let Some((n, r)) = c.rec.filter(|&(n, _)| c.reg != behind && ahead == Some(n)) {
            let to_line = len - c.d - 0.5 * c.len;
            if c.stop + c.speed >= to_line {
                let (q, k) = self.query(me, c, n, r, !c.ignores_lights());
                if q != 0 {
                    if to_line <= c.stop {
                        c.f4386 &= !4;
                    }
                    if best > to_line {
                        best = to_line;
                        bg = c.stop;
                        c.kind = if q == 3 && k == 2 { 5 } else { 1 };
                    }
                }
            }
            let it = &self.net.inters[n];
            let (e, el, x) = (it.lanes[r].entry, it.lanes[r].entry_lane, it.lanes[r].exit);
            let inside: Vec<(f32, f32, u32)> = it.entries[e % 4]
                .as_ref()
                .and_then(|en| en.lanes.get(el))
                .into_iter()
                .flatten()
                .flat_map(|&j| self.recs[n][j].iter())
                .filter(|&&v| v != me)
                .filter_map(|&v| self.car(v))
                .filter(|v| v.on == On::Inter)
                .map(|v| (v.s, v.len, v.kind))
                .collect();
            for (vs, vl, vk) in inside {
                obstacle(c, (len - c.d + vs) - 0.5 * (c.len + vl), c.gap, vk, &mut best, &mut bg);
            }
            if c.stop * 10.0 + c.speed >= to_line {
                if x == (e + 1) % 4 {
                    c.lamps |= 0x08;
                } else if x == (e + 3) % 4 {
                    c.lamps |= 0x10;
                }
            }
        }
        if c.park_at >= 0.0 {
            let t = c.park_at - c.d + 0.1;
            if t > 0.0 && c.chg + c.speed >= t {
                if t <= c.chg {
                    c.f4386 &= !4;
                }
                if best > t {
                    best = t;
                    bg = c.chg;
                    c.kind = 2;
                }
            }
        }
        match self.ahead_in(c.sup, c.lane, me).map(|o| (o.d, o.len, o.kind)) {
            Some((od, ol, ok)) => obstacle(c, od - c.d - 0.5 * (c.len + ol), c.gap, ok, &mut best, &mut bg),
            None => c.t3688 = 0.0,
        }
        skater_obstacle(c, &mut best, &mut bg);
        speed_ctl(c, best, bg);
        integrate_speed(c, speed);
    }

    /// ContinueChangingLane 0x82b06b60.
    fn cont_changing(&mut self, me: usize, c: &mut Car) {
        c.kind = 0;
        c.f4386 |= 4;
        c.near = MAX;
        let (mut best, mut bg) = (MAX, MAX);
        let Some((from, to, s, len)) = c.change.as_ref().map(|k| (k.from, k.to, k.s, k.len)) else { return };
        let mut a0 = None;
        if c.lane == from {
            if s - 0.5 * c.len > 0.5 * len {
                self.remove(c.sup, from, me, c.occ());
                c.lane = to;
            } else {
                a0 = self.ahead_in(c.sup, from, me).map(|o| (o.d, o.len, o.kind, o.changing));
            }
        }
        let a1 = self.ahead_in(c.sup, to, me).map(|o| (o.d, o.len, o.kind, true));
        if a0.is_none() && a1.is_none() {
            c.t3688 = 0.0;
        } else {
            let (mut g, mut k) = (MAX, 0);
            for (od, ol, ok, _) in [a0, a1].into_iter().flatten().filter(|o| o.3) {
                let dist = od - c.d - 0.5 * (c.len + ol);
                if dist < g {
                    (g, k) = (dist, ok);
                }
            }
            obstacle(c, g, c.gap * 0.5, k, &mut best, &mut bg);
        }
        skater_obstacle(c, &mut best, &mut bg);
        speed_ctl(c, best, bg);
        integrate_speed(c, self.net.sups[c.sup].speed);
    }

    /// ContinuePullingOver 0x82b070b8.
    fn cont_pulling_over(&mut self, me: usize, c: &mut Car) {
        c.kind = 0;
        c.f4386 |= 4;
        c.near = MAX;
        let (mut best, mut bg) = (MAX, MAX);
        let Some((from, to, s, len)) = c.change.as_ref().map(|k| (k.from, k.to, k.s, k.len)) else { return };
        let rem = len - s;
        if c.speed > rem {
            best = rem;
            bg = 0.0;
            c.kind = 2;
        }
        if c.lane == from {
            if s - 0.5 * c.len > 0.5 * len {
                self.remove(c.sup, from, me, c.occ());
                c.t3688 = 0.0;
                c.lane = to;
            } else {
                match self.ahead_in(c.sup, from, me).map(|o| (o.d, o.len, o.kind, o.changing)) {
                    None => c.t3688 = 0.0,
                    Some((od, ol, ok, ch)) => {
                        let dist = if ch { od - c.d - 0.5 * (c.len + ol) } else { MAX };
                        obstacle(c, dist, c.gap * 0.25, ok, &mut best, &mut bg);
                    }
                }
            }
        } else {
            c.t3688 = 0.0;
        }
        skater_obstacle(c, &mut best, &mut bg);
        speed_ctl(c, best, bg);
        integrate_speed(c, self.net.sups[c.sup].speed);
    }

    /// ContinuePullingOut 0x82b07680.
    fn cont_pulling_out(&mut self, me: usize, c: &mut Car) {
        c.f4386 |= 4;
        c.near = MAX;
        c.kind = 0;
        let (mut best, mut bg) = (MAX, MAX);
        let to = c.change.as_ref().map_or(c.lane, |k| k.to);
        match self.ahead_in(c.sup, to, me).map(|o| (o.d, o.len, o.kind)) {
            None => c.t3688 = 0.0,
            Some((od, ol, ok)) => obstacle(c, od - c.d - 0.5 * (c.len + ol), c.gap * 0.25, ok, &mut best, &mut bg),
        }
        skater_obstacle(c, &mut best, &mut bg);
        speed_ctl(c, best, bg);
        integrate_speed(c, self.net.sups[c.sup].speed);
    }

    /// ContinueStayingParked 0x82b075b0.
    fn cont_parked(c: &mut Car) {
        c.f4386 &= !4;
        c.kind = 2;
        c.near = MAX;
        c.accel = 0.0;
        c.speed = 0.0;
        if c.f3420 & 0x10 != 0 {
            c.t3700 += DT;
            c.t3696 = 0.0;
        } else {
            c.t3696 += DT;
            c.t3700 = 0.0;
        }
    }

    /// ContinuePassingIntersection 0x82b07928.
    fn cont_passing(&mut self, me: usize, c: &mut Car) {
        c.kind = 0;
        c.f4386 |= 4;
        c.near = MAX;
        let (mut best, mut bg) = (MAX, MAX);
        let Some((n, r)) = c.rec else { return };
        let it = &self.net.inters[n];
        let ln = &it.lanes[r];
        let my_front = (ln.c.len() - c.s) - 0.5 * c.len;
        // Merge pass: other records feeding my exit lane.
        let merging: Vec<(f32, u32)> = it.exits[ln.exit % 4]
            .as_ref()
            .and_then(|x| x.lanes.get(ln.exit_lane))
            .into_iter()
            .flatten()
            .filter(|&&j| j != r)
            .flat_map(|&j| self.recs[n][j].iter().filter_map(|&v| self.car(v)).filter(|v| v.on == On::Inter).map(move |v| (j, v)))
            .map(|(j, v)| ((it.lanes[j].c.len() - v.s) - 0.5 * v.len, v.kind))
            .collect();
        for (of, ok) in merging {
            if my_front > of {
                obstacle(c, my_front - (of + c.len), c.gap, ok, &mut best, &mut bg);
            }
        }
        let list = &self.recs[n][r];
        let a = list.iter().position(|&v| v == me).and_then(|p| list.get(p + 1)).and_then(|&v| self.car(v));
        let b = it.exits[ln.exit % 4].as_ref().and_then(|x| self.lists[x.sup].get(ln.exit_lane)).and_then(|l| l.first()).and_then(|&v| self.car(v));
        if a.is_none() && b.is_none() {
            c.t3688 = 0.0;
        } else {
            let (mut g, mut k) = (MAX, 0);
            if let Some(a) = a.filter(|a| a.on == On::Inter) {
                let ga = (a.s - c.s) - 0.5 * (c.len + a.len);
                if ga < g {
                    (g, k) = (ga, a.kind);
                }
            }
            if let Some(b) = b.filter(|b| b.d >= 0.0) {
                let gb = (b.d + ln.c.len() - c.s) - 0.5 * (c.len + b.len);
                if gb < g {
                    (g, k) = (gb, b.kind);
                }
            }
            obstacle(c, g, c.gap * 0.5, k, &mut best, &mut bg);
        }
        let (lane_speed, inter_speed) = (ln.speed, it.speed);
        skater_obstacle(c, &mut best, &mut bg);
        speed_ctl(c, best, bg);
        integrate_speed(c, inter_speed);
        c.over = if c.speed > lane_speed { (c.speed - lane_speed) / lane_speed } else { 0.0 };
    }

    // ------------------------------------------------------------ per-tick systems

    /// 0x82b0dfc0: lane-change / pull-over decision every 90AB metres.
    fn decide(&mut self, me: usize, c: &mut Car) {
        if c.f4386 & 0x40 != 0 || c.on != On::Road || c.plan != 0 || c.odo <= c.t.odo {
            return;
        }
        c.odo = 0.0;
        let (lanes, flags) = (self.net.sups[c.sup].lanes, self.net.sups[c.sup].flags);
        if self.roll(c.t.change) {
            let smart = self.roll(c.t.smart_lane);
            if flags & 2 != 0 {
                let t = self.pick_lane(c.sup, c.lane, smart);
                if t != c.lane && self.gap(c.sup, c.d, c.len, c.speed, t, true) {
                    c.target = t;
                    c.plan = 1;
                    return;
                }
            }
        }
        // ponytail: the chance is zeroed only under 4386&2, whose setter is UNTRACED (never set).
        if self.roll(c.t.pull) {
            c.park_at = -1.0;
            if flags & 1 != 0 && c.lane + 1 == lanes {
                let spot = self.find_spot(c.sup, c.d, c.len, c.speed);
                if spot >= c.d + c.chg {
                    c.park_at = spot;
                    let p = &mut self.parking[c.sup];
                    let at = p.iter().position(|&(s, _)| s > spot).unwrap_or(p.len());
                    p.insert(at, (spot, me));
                    c.f4386 |= 0x20;
                    c.plan = 3;
                    return;
                }
            }
        }
        c.plan = 0;
    }

    /// 0x82b0e638 (while 4385&0x40): the behaviour bits.
    fn roll_behaviour(&mut self, c: &mut Car) {
        let t = c.t;
        for (bit, chance) in [(2, t.smart_exit), (4, t.list_b), (8, t.aware), (0x10, t.runner), (1, t.status_on)] {
            c.f4385 = if self.roll(chance) { c.f4385 | bit } else { c.f4385 & !bit };
        }
        c.f4386 = if self.roll(t.b80) { c.f4386 | 0x80 } else { c.f4386 & !0x80 };
        c.f4385 &= !0x40;
    }

    /// 0x82b0cdb8 (list A = the local skater, radius 0, flag 0; list B, pedestrians, skipped).
    fn aware(&self, c: &mut Car) {
        c.f4386 &= !8;
        if !(c.speed * 3.6 > c.t.aware_kmh || (c.f4385 & 8 != 0 && !c.ignores_lights())) {
            return;
        }
        let p = self.skater;
        if p.distance(c.pose.pos) - 0.5 * c.len < c.sk_gap + c.speed && c.in_quad(p, c.speed + c.sk_gap) {
            c.f4386 |= 8;
            c.seen = p;
        }
    }

    /// 0x82b0d6f0: the skater ahead the car follows (Skate 1 has no 33466832, so never there).
    fn follow(&self, c: &mut Car) {
        c.sk_speed = 0.0;
        c.sk_dist = -1.0;
        c.f4387 &= !0x18;
        let p = self.skater;
        let dist = p.distance(c.pose.pos) - 0.5 * c.len;
        if dist < c.t.follow_dist && c.in_quad(p, c.speed + c.sk_gap) {
            c.sk_dist = dist;
            c.sk_speed = self.skater_vel.length();
            if c.pose.fwd.dot(self.skater_vel) > 0.0 {
                c.f4387 |= 0x10;
            }
        }
    }

    /// 0x82371378 + pose 0x8235c578. False = despawn (0x825d5798).
    fn advance(&mut self, me: usize, c: &mut Car) -> bool {
        let ds = c.speed * DT;
        match c.on {
            On::Road => {
                if let Some(k) = c.change.as_mut() {
                    k.s += ds;
                    c.d = k.d0 + if k.len > 0.0 { k.s / k.len } else { 1.0 } * (k.d1 - k.d0);
                    if k.s >= k.len {
                        c.lane = k.to;
                        c.changing = false;
                    }
                    if self.net.road_at(c.sup, c.d).is_none() {
                        return false;
                    }
                } else if c.state != State::Parked {
                    c.d += ds;
                    let s = &self.net.sups[c.sup];
                    if c.d >= s.len {
                        let over = c.d - s.len;
                        // End kind 0: the intersection, only with a record chosen for it.
                        let (Some(n), Some((rn, _))) = (s.ahead, c.rec) else { return false };
                        if rn != n {
                            return false;
                        }
                        self.remove(c.sup, c.lane, me, c.occ());
                        c.d = -1.0;
                        c.on = On::Inter;
                        c.s = over;
                    } else if self.net.road_at(c.sup, c.d).is_none() {
                        return false; // a kind-2 end inside the superroad (Skate 1 data)
                    }
                }
            }
            On::Inter => {
                c.s += ds;
                let Some((n, r)) = c.rec else { return false };
                let ln = &self.net.inters[n].lanes[r];
                if c.s > ln.c.len() {
                    let over = c.s - ln.c.len();
                    let Some(x) = self.net.inters[n].exits[ln.exit % 4].as_ref() else { return false };
                    let (sup, xl, d) = (x.sup, ln.exit_lane, x.start + over);
                    if xl >= self.net.sups[sup].lanes {
                        return false;
                    }
                    c.sup = sup;
                    c.lane = xl;
                    c.d = d;
                    c.on = On::Road;
                    c.s = over;
                    self.insert(sup, xl, me, d, c.occ());
                }
            }
        }
        let before = c.pose.pos;
        self.place(c);
        let moved = c.pose.pos.distance(before);
        c.odo += moved;
        c.travelled += moved;
        true
    }

    fn place(&self, c: &mut Car) {
        let pose = match (c.on, &c.change) {
            (On::Inter, _) => c.rec.map(|(n, r)| {
                let it = &self.net.inters[n];
                let curve = &it.lanes[r].c;
                let t = curve.param(c.s);
                frame(curve.at(t), curve.tangent(t), it.up)
            }),
            (On::Road, Some(k)) => {
                // Ruling: the change curve takes the road piece's up.
                let up = self.net.piece_at(c.sup, c.d).map_or(Vec3::Y, |(p, _)| p.up);
                let t = k.c.param(k.s);
                Some(frame(k.c.at(t), k.c.tangent(t), up))
            }
            (On::Road, None) => self.net.road_pose(c.sup, c.d, c.lane),
        };
        if let Some(p) = pose {
            c.pose = p;
        }
    }

    /// 0x82b0b790: one agent tick.
    fn step(&mut self, me: usize, c: &mut Car) -> bool {
        self.transitions(me, c);
        match c.state {
            State::Following => self.cont_following(me, c),
            State::Changing => self.cont_changing(me, c),
            State::PullingOver => self.cont_pulling_over(me, c),
            State::Parked => Self::cont_parked(c),
            State::PullingOut => self.cont_pulling_out(me, c),
            State::Passing => self.cont_passing(me, c),
        }
        self.decide(me, c);
        if c.f4385 & 0x40 != 0 {
            self.roll_behaviour(c);
        }
        self.aware(c);
        self.follow(c);
        status(c, self.skater);
        lamps(c);
        self.advance(me, c)
    }

    // ------------------------------------------------------------ living world

    /// 0x82333308 census pick at a point: (model, tune), or None.
    fn census_pick(&mut self, at: Vec3) -> Option<(usize, Tune, i32, Palette)> {
        let key = painter::sample(&self.census.tiles, at.x, at.z)?;
        let &(count, group) = self.census.rows.get(&key)?;
        // tu3 0x826B8A28: the vehicles layer's count x density (+0x8C: 1.0 offline, 0x826B7010); 0x826B83C8: budget A.
        if self.count() >= census_cap(count, self.density, self.live_cap) {
            return None;
        }
        let group = match &self.census.group_tiles {
            Some(tiles) => painter::sample(tiles, at.x, at.z)?,
            None => group?,
        };
        let r = (self.next() % 100 + 1) as f32;
        let mut cum = 0.0;
        let mut ents = None;
        for (w, e) in self.census.groups.get(&group)? {
            cum += w;
            if r <= cum * 100.0 {
                ents = Some(e.clone());
                break;
            }
        }
        let ents = ents?;
        let i = self.pick(ents.len());
        let n = self.census.kinds.get(ents.get(i)?)?.variants.len();
        let v = self.pick(n);
        let kind = self.census.kinds.get(&ents[i])?;
        Some((kind.variants.get(v).copied().flatten()?, kind.tune, kind.hom, kind.palettes.get(v).cloned().unwrap_or_default()))
    }

    /// 0x825cd1c0: `tries` attempts for `spawns` cars in the ring around the skater.
    fn spawn_pass(&mut self, init: bool) {
        // 0x825cd268: the first fill uses 6000/600 in [8, 80].
        let (mut tries, mut left, inner, outer) = if init { (6000, 600, 8.0, 80.0) } else { (2, 1, RING.0, RING.1) };
        while tries > 0 && left > 0 {
            tries -= 1;
            // 0x82333718
            let (u1, u2, u3) = (self.unit(), self.unit(), self.unit());
            let dir = Vec3::new(u2 - 0.5, 0.0, u1 - 0.5).normalize_or_zero();
            let at = self.skater + dir * (inner + (outer - inner) * u3);
            if (init || !self.in_view(at)) && self.spawn_at(at) {
                left -= 1;
            }
        }
    }

    fn in_view(&self, p: Vec3) -> bool {
        self.view.is_some_and(|(eye, fwd)| (p - eye).normalize_or_zero().dot(fwd) > VIEW_COS)
    }

    fn unit(&mut self) -> f32 {
        (f64::from(self.next()) / 4_294_967_296.0) as f32
    }

    /// Census pick + placement 0x82B04A08 + ctor 0x82b09cc8 / profile 0x82b0ac18.
    fn spawn_at(&mut self, at: Vec3) -> bool {
        let Some((model, t, hom, palette)) = self.census_pick(at) else { return false };
        let Some((ri, pi)) = self.net.nearest(at) else { return false };
        let road = &self.net.roads[ri];
        let (sup, d) = (road.sup, road.start + road.pieces[pi].end - road.pieces[pi].len);
        if sup == usize::MAX {
            return false;
        }
        let free: Vec<usize> = (0..self.net.sups[sup].lanes).filter(|&l| self.gap(sup, d, 15.0, 0.0, l, false)).collect();
        if free.is_empty() {
            return false;
        }
        let lane = free[self.pick(free.len())];
        let (len, hw) = (self.models[model].len, self.models[model].half_width);
        // Driver type: rand%100+1 <= cumulative * 100 over fast, slow, normal.
        let r = (self.next() % 100 + 1) as f32;
        let mut cum = 0.0;
        let mut speed_off = 0.0;
        for (p, v) in [(t.p_fast, t.fast), (t.p_slow, -t.slow), (t.p_normal, 0.0)] {
            cum += p;
            if r <= cum * 100.0 {
                speed_off = v;
                break;
            }
        }
        let amax = t.amin + (self.next() % 101) as f32 * 0.01 * (t.amax - t.amin);
        self.serial += 1;
        // Picked from the serial, not the traffic RNG, so the retail spawn sequence is unchanged. INFERRED: one random
        // entry of each list per car.
        let pick = |list: &[[f32; 3]], salt: u64| {
            let h = (self.serial ^ salt).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32;
            (!list.is_empty()).then(|| list[h as usize % list.len()])
        };
        let paint = pick(&palette.red, 0).map(|red| (red, pick(&palette.blue, 0xB1).unwrap_or([0.0, 0.0, 1.0])));
        let mut c = Car {
            serial: self.serial,
            paint,
            model,
            len,
            hw,
            t,
            state: State::Following,
            on: On::Road,
            sup,
            lane,
            d,
            s: 0.0,
            rec: None,
            reg: None,
            speed: 0.0,
            accel: 0.0,
            f3420: 0,
            lamps: 0,
            f4385: 0xCE,
            f4386: 4,
            f4387: 0,
            status: 0,
            over: 0.0,
            odo: 0.0,
            park_at: -1.0,
            target: lane,
            plan: 0,
            near: MAX,
            kind: 0,
            t3688: 0.0,
            t3692: 0.0,
            t3696: 0.0,
            t3700: 0.0,
            brake_t: 0.0,
            blink_t: 0.0,
            amax,
            speed_off,
            stop: len * t.stop_k,
            gap: len * t.gap_k,
            sk_gap: len * t.sk_k,
            chg: len * t.chg_k,
            change: None,
            changing: false,
            seen: Vec3::ZERO,
            sk_dist: -1.0,
            sk_speed: 0.0,
            pose: Pose::default(),
            travelled: 0.0,
            hom,
            hit: 0,
        };
        self.place(&mut c);
        let me = self.cars.iter().position(Option::is_none).unwrap_or(self.cars.len());
        if me == self.cars.len() {
            self.cars.push(None);
        }
        self.insert(sup, lane, me, d, c.occ());
        self.cars[me] = Some(c);
        true
    }

    /// One 60 Hz step: lights, every agent, then (every 4th frame, 0x823d5458) despawn and spawn.
    pub(crate) fn tick(&mut self, skater: Vec3) {
        self.skater_vel = if self.frame == 0 { Vec3::ZERO } else { (skater - self.skater) / DT };
        self.skater = skater;
        for l in &mut self.lights {
            l.tick();
        }
        if !self.filled {
            self.filled = true;
            self.spawn_pass(true);
        }
        for i in 0..self.cars.len() {
            let Some(mut c) = self.cars[i].take() else { continue };
            if self.step(i, &mut c) {
                self.cars[i] = Some(c);
            } else {
                self.forget(i, &c);
            }
        }
        if self.frame % 4 == 1 {
            for i in 0..self.cars.len() {
                let gone = |c: &Car| {
                    let far = c.pose.pos.distance(skater);
                    far > DESPAWN && (far > DESPAWN_SEEN || !self.in_view(c.pose.pos))
                };
                if self.cars[i].as_ref().is_some_and(gone) {
                    let c = self.cars[i].take().unwrap();
                    self.forget(i, &c);
                }
            }
            self.spawn_pass(false);
        }
        self.frame += 1;
    }

    /// Start a car's alarm as 0x82b0a6b8 does: veh3420 |= 0x10 with both parked timers reset (hazards, status 6,
    /// StopAlarming 0x82b099e8 after E199 s).
    pub(crate) fn raise_alarm(&mut self, serial: u64) {
        if let Some(c) = self.cars.iter_mut().flatten().find(|c| c.serial == serial) {
            c.t3696 = 0.0;
            c.t3700 = 0.0;
            c.f3420 |= 0x10;
        }
    }

    /// The vehicle contact callback 0x82b0a6b8 for the local skater touching car `serial` at `point` while moving at
    /// `velocity`. A parked car whose contact is past profile 543475921FD9E04A sounds its alarm; the skater's bit
    /// goes in the hit mask (veh4368, for the Hall of Meat); a contact ahead of the car's centre along its heading
    /// brakes it (4385&0x20, [`speed_ctl`]). Ruling: the contact vector (callback arg +48) is taken as the skater's
    /// velocity relative to the car (its meaning is UNTRACED).
    pub(crate) fn hit(&mut self, serial: u64, point: Vec3, velocity: Vec3) {
        let Some(c) = self.cars.iter_mut().flatten().find(|c| c.serial == serial) else { return };
        let alarm = c.f3420 & 0x80 != 0 && (velocity - c.pose.fwd * c.speed).length() > c.t.alarm_hit;
        c.hit |= 1;
        if (point - c.pose.pos).dot(c.pose.fwd) > 0.0 {
            c.f4385 |= 0x20;
        }
        if alarm {
            self.raise_alarm(serial);
        }
    }

    /// 0x823ad3f0: every car's hit mask clears each frame.
    pub(crate) fn clear_hits(&mut self) {
        for c in self.cars.iter_mut().flatten() {
            c.hit = 0;
        }
    }

    /// Bug 10-25 #24, the user's account of retail (not traced): a session marker return takes away a car standing
    /// where the marker is, as the one whose roof it was set on. `at`: the marker's translation (the board or feet,
    /// up to 1.5 m over the box), with half a metre round the sides.
    pub(crate) fn clear_at(&mut self, at: Vec3) {
        for i in 0..self.cars.len() {
            let under = |c: &Car| {
                let m = &self.models[c.model];
                let d = at - (c.pose.pos + c.pose.left * m.centre.x + c.pose.up * m.centre.y + c.pose.fwd * m.centre.z);
                d.dot(c.pose.left).abs() <= m.half.x + 0.5 && d.dot(c.pose.fwd).abs() <= m.half.z + 0.5 && d.dot(c.pose.up).abs() <= m.half.y + 1.5
            };
            if self.cars[i].as_ref().is_some_and(under) {
                let c = self.cars[i].take().unwrap();
                self.forget(i, &c);
            }
        }
    }

    /// Whether `point` (a contact on car `serial`) is on the top of one of its boxes: the roof, hood or trunk, not a side.
    pub(crate) fn on_top(&self, serial: u64, point: Vec3) -> bool {
        let Some(c) = self.cars.iter().flatten().find(|c| c.serial == serial) else { return false };
        let d = point - c.pose.pos;
        let local = Vec3::new(d.dot(c.pose.left), d.dot(c.pose.up), d.dot(c.pose.fwd));
        self.models[c.model].boxes.iter().any(|&(centre, half)| {
            (local.z - centre.z).abs() <= half.z && local.y >= centre.y + half.y - ROOF_BAND
        })
    }

    /// Each car's collision boxes (its model's [`Model::boxes`]) on its pose, moving with it.
    pub(crate) fn shapes(&self) -> Vec<(u64, CollisionShape)> {
        self.cars
            .iter()
            .flatten()
            .flat_map(|c| {
                self.models[c.model].boxes.iter().map(move |&(centre, half)| {
                    let shape = CollisionShape {
                        position: c.pose.pos.to_array(),
                        rotation: pose_transform(&c.pose).rotation.to_array(),
                        velocity: (c.pose.fwd * c.speed).to_array(),
                        angular: [0.0; 3],
                        half: half.to_array(),
                        offset: centre.to_array(),
                        rounding: ROUNDING,
                        mass: MASS,
                    };
                    (c.serial, shape)
                })
            })
            .collect()
    }

    /// The cars the title's traffic audio hears (collector 0x827A3790 into the game frame `+48`/`+52`, which
    /// CSTATEMGR_TrafficCar 0x824F2460 and SFXCTL 0x824B2A28 read): those within [`VOICE_RANGE`] of the skater,
    /// nearest first, [`skate_audio_core::game::TRAFFIC_CARS`] at most. Record `+16`: status (0x82b0c8b0) in bits
    /// 28..31, veh3420 bit 3 (horn) as bit 27.
    pub(crate) fn voices(&self) -> Vec<skate_audio_core::game::TrafficCar> {
        let mut near: Vec<(f32, &Car)> = self
            .cars
            .iter()
            .flatten()
            .map(|c| (c.pose.pos.distance(self.skater), c))
            .filter(|(d, _)| *d <= VOICE_RANGE)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        near.iter()
            .take(skate_audio_core::game::TRAFFIC_CARS)
            .map(|&(distance, c)| skate_audio_core::game::TrafficCar {
                handle: self.handles.wrapping_add(c.serial as u32),
                accel: c.accel,
                speed: c.speed,
                distance,
                flags: (c.status & 0xF) << 28 | u32::from(c.f3420 & 8 != 0) << 27,
                key: c.t.audio,
                forward: c.pose.fwd.to_array(),
                position: c.pose.pos.to_array(),
            })
            .collect()
    }

    /// The Hall of Meat's hit list (0x82322018): of the cars within 8 m of the skater (8 at most), those the skater
    /// hit this frame, as (id, HoM type).
    pub(crate) fn hom_hits(&self) -> [Option<(u64, i32)>; 8] {
        let mut out = [None; 8];
        let near = self.cars.iter().flatten().filter(|c| c.pose.pos.distance(self.skater) <= 8.0).take(8);
        for (o, c) in out.iter_mut().zip(near.filter(|c| c.hit & 1 != 0)) {
            *o = Some((c.serial, c.hom));
        }
        out
    }
}

/// Ruling: how far the collector 0x827A3790 reaches, and its order, are UNTRACED; nearest first within this.
const VOICE_RANGE: f32 = 40.0;

/// Ruling: the retail car body (VehiclePool) is UNTRACED; a car is its model's bounding box with these.
/// Traced so far (tu3): 0x82C37458 builds the car's CollisionBody in group 8 with mass 500 * x*y*z of
/// (this+0x10)->vfunc 0x10 (0x82C37630); the shape is (this+0x10)->vfunc 0x50, whose source is UNTRACED.
const ROUNDING: f32 = 0.05;
/// How far under a box's top a contact still counts as on it ([`Sim::on_top`]): its rounding and a wheel's give.
const ROOF_BAND: f32 = 0.15;
const MASS: f32 = 1500.0;

// ---------------------------------------------------------------- Bevy

pub(crate) struct TrafficPlugin;

impl Plugin for TrafficPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Traffic>()
            .add_systems(
                FixedUpdate,
                collide.after(crate::multiplayer::prepare).before(crate::app::SimulationSet::Physics).run_if(crate::graphics_menu::gameplay_active),
            )
            .add_systems(FixedUpdate, drive.after(crate::app::SimulationSet::Physics).run_if(crate::graphics_menu::gameplay_active))
            .add_systems(Update, (gate, show).chain());
    }
}

#[derive(Resource, Default)]
pub(crate) struct Traffic {
    pub(crate) sim: Option<Sim>,
    /// (map, generation) traffic was built for; None = no traffic (other map, or online).
    key: Option<(String, u64)>,
    shown: HashMap<u64, Shown>,
    /// Painted copies of a car material, by (source material, colours), with the serial of the last car that wore
    /// each ([`paint_copy`]).
    painted: HashMap<PaintKey, (Handle<StandardMaterial>, u64)>,
    /// Each car model's scene, by model, held while the map's traffic lives: a model whose last car left unloaded, and
    /// its next car reloaded it and repainted every colour (a 4-13 ms hitch about once a second, perf 11-01).
    scenes: HashMap<usize, Handle<Scene>>,
}

type PaintKey = (AssetId<StandardMaterial>, [u32; 6]);

/// Painted copies kept once no car wears them: each is a whole atlas on the GPU (0.7-1.3 MB with its mips).
const PAINT_CACHE: usize = 64;

struct Shown {
    root: Entity,
    scene: Entity,
    wheels: Option<Vec<(Entity, Transform)>>,
}

impl Traffic {
    /// The cars the skater hit this frame, for the Hall of Meat (none without traffic: other maps, online).
    pub(crate) fn hom_hits(&self) -> [Option<(u64, i32)>; 8] {
        self.sim.as_ref().map_or([None; 8], Sim::hom_hits)
    }

    /// The cars the traffic audio hears ([`Sim::voices`]); None without traffic (other maps, online).
    pub(crate) fn voices(&self) -> Option<Vec<skate_audio_core::game::TrafficCar>> {
        self.sim.as_ref().map(Sim::voices)
    }
}

/// Mission B hook: one per traffic car on screen, refreshed every frame.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
#[allow(dead_code)]
pub(crate) struct TrafficCar {
    pub serial: u64,
    /// veh3416 (0x82b0c8b0): 0 quiet, 1 skater close, 2 skater blocking, 3 yielding, 4/5 stuck behind, 6 alarm.
    pub status: u32,
    /// veh4384 lamp bits.
    pub lamps: u32,
    /// veh3412: fraction over the intersection lane speed.
    pub overspeed: f32,
    pub speed: f32,
    pub length: f32,
    pub half_width: f32,
}

/// A traffic map's data folder (roads.json, census.json) and vehicle GLB folder under the asset root; None = no
/// traffic. Skate 3: the districts whose world stream has a road network (SkateSchool is census-painted but has no
/// roads; the parks have neither), sharing one vehicles folder.
fn dirs(map: &str) -> Option<(&'static str, &'static str)> {
    Some(match map {
        SKATE2_MAP => ("private/traffic/skate2", "private/traffic/skate2/vehicles"),
        SKATE1_MAP => ("private/traffic/skate1", "private/traffic/skate1/vehicles"),
        "DownTown" => ("private/traffic/skate3/DownTown", "private/traffic/skate3/vehicles"),
        "Industrial" => ("private/traffic/skate3/Industrial", "private/traffic/skate3/vehicles"),
        "University" => ("private/traffic/skate3/University", "private/traffic/skate3/vehicles"),
        _ => return None,
    })
}

/// Only the traffic maps, only offline (the brief: never online, any map, any game).
#[allow(clippy::type_complexity)]
fn wanted(map: Option<&crate::map_transition::CurrentMap>, online: bool) -> Option<((&'static str, &'static str), (String, u64))> {
    let map = map.filter(|_| !online)?;
    Some((dirs(&map.name)?, (map.name.clone(), map.generation)))
}

fn gate(
    mut commands: Commands,
    mut traffic: ResMut<Traffic>,
    online: Option<Res<crate::multiplayer::Online>>,
    map: Option<Res<crate::map_transition::CurrentMap>>,
    config: Option<Res<crate::config::Config>>,
    skate2: Option<Res<crate::skate2_physics::Skate2Physics>>,
    cars: Query<Entity, With<TrafficCar>>,
    (mode, free_play): (Option<Res<crate::skate3_career::mode::GameMode>>, Option<Res<crate::skate3_career::freeplay::FreePlay>>),
) {
    // Story mode Task 37 (not tested in play): Free Play's Traffic setting scales the census; 0 clears the cars.
    let density = match (mode, free_play) {
        (Some(mode), Some(fp)) => crate::skate3_career::freeplay::density(*mode, &fp.settings),
        _ => 1.0,
    };
    // Amendment C: no engine cars while Skate 2 physics has the skater (its skater cannot touch them); they come back
    // when the switch gives the skater back.
    let engine_skater = crate::skate2_physics::step::engine_runs_local(skate2.as_deref());
    let want = wanted(map.as_deref(), online.is_some_and(|o| o.0)).filter(|_| engine_skater && density > 0.);
    if let Some(sim) = traffic.sim.as_mut() {
        sim.density = density;
    }
    let key = want.as_ref().map(|w| w.1.clone());
    if traffic.key == key {
        return;
    }
    for e in &cars {
        commands.entity(e).despawn();
    }
    traffic.shown.clear();
    traffic.scenes.clear();
    traffic.key = key;
    // Ruling: roads.json is read synchronously when the map is entered.
    traffic.sim = match (want, config) {
        (Some((dirs, _)), Some(config)) => {
            Sim::load(&config.asset_root, dirs).inspect_err(|e| warn!("traffic: {e}")).ok().map(|sim| Sim { density, ..sim })
        }
        _ => None,
    };
}

/// The car bodies for this physics step: none online or in a replay (or without traffic).
fn car_bodies(traffic: &Traffic, online: bool, replay: bool) -> Vec<(u64, CollisionShape)> {
    match &traffic.sim {
        Some(sim) if !online && !replay => sim.shapes(),
        _ => Vec::new(),
    }
}

/// Before the physics step (after multiplayer::prepare refilled the proxies): the cars near the skater join the
/// solve as vehicle bodies (SkaterVehicleCollisionPipeline; collision group 8 for the wipeout's vehicle rules).
fn collide(
    traffic: Res<Traffic>,
    mut physics: ResMut<crate::physics::GamePhysics>,
    skater: Res<crate::physics::SkaterRuntime>,
    online: Option<Res<crate::multiplayer::Online>>,
    replay: Option<Res<crate::replay::Replay>>,
) {
    let bodies = car_bodies(&traffic, online.is_some_and(|o| o.0), replay.is_some_and(|r| r.active));
    let mut proxies = std::mem::take(&mut physics.network_proxies);
    for (serial, shape) in &bodies {
        if let Some(body) = proxies.append_vehicle(shape, &physics, &skater) {
            proxies.cars.push((body, *serial));
        }
    }
    physics.network_proxies = proxies;
}

pub(crate) fn drive(
    mut traffic: ResMut<Traffic>,
    mut skater: ResMut<crate::physics::SkaterRuntime>,
    mut physics: ResMut<crate::physics::GamePhysics>,
    online: Option<Res<crate::multiplayer::Online>>,
    replay: Option<Res<crate::replay::Replay>>,
    camera: Query<&GlobalTransform, With<crate::camera::GameplayCamera>>,
) {
    if online.is_some_and(|o| o.0) || replay.is_some_and(|r| r.active) {
        return;
    }
    let Some(sim) = traffic.sim.as_mut() else { return };
    sim.view = camera.iter().next().map(|t| (t.translation(), t.forward().as_vec3()));
    // This step's contacts on the cars (the solve's touches on their bodies), then the agents.
    sim.clear_hits();
    let touched = std::mem::take(&mut physics.network_proxies.touched);
    let on_board = skater.player_input.physical.state.category_12 != 500;
    for &(body, point, velocity, board) in &touched {
        if let Some(&(_, serial)) = physics.network_proxies.cars.iter().find(|c| c.0 == body) {
            sim.hit(serial, point, velocity);
            // Retail: riding onto a car's top bails (the vehicle contact wipeout, reason 7); on foot it can be stood
            // on (its PB_STAND_ON_CAR pose). UNTRACED: retail's trigger; the user's account of the real game.
            if board && on_board && sim.on_top(serial, point) {
                skater.wipeout.state.request(7, 0.0);
            }
        }
    }
    sim.tick(crate::animation::native_matrix(skater.animated_skeleton.roots.animation_to_world).w_axis.truncate());
}

/// `source` (a [`painted`] car material) in these colours: the cached copy, else a new one ([`paint`]). A new copy
/// takes the colorized atlas of a cached copy with the same atlas and colours (Skate 1's body and light materials
/// share one atlas) and the shine mask of one of the same material, so each texture is made once. Past
/// [`PAINT_CACHE`] the copies worn longest ago go.
fn paint_copy(
    cache: &mut HashMap<PaintKey, (Handle<StandardMaterial>, u64)>,
    source: &Handle<StandardMaterial>,
    shine: bool,
    (red, blue): ([f32; 3], [f32; 3]),
    serial: u64,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Option<Handle<StandardMaterial>> {
    let [r, b] = [red, blue].map(|c| c.map(f32::to_bits));
    let key = (source.id(), [r[0], r[1], r[2], b[0], b[1], b[2]]);
    if let Some((h, worn)) = cache.get_mut(&key) {
        *worn = (*worn).max(serial);
        return Some(h.clone());
    }
    let atlas = |m: AssetId<StandardMaterial>| materials.get(m)?.base_color_texture.as_ref().map(Handle::id);
    let mine = atlas(key.0)?;
    let (mut colorized, mut mask) = (None, None);
    for ((m, colours), (h, _)) in cache.iter() {
        let Some(copy) = materials.get(h).filter(|_| atlas(*m) == Some(mine)) else { continue };
        if *colours == key.1 {
            colorized = copy.base_color_texture.clone();
        }
        if shine && *m == key.0 {
            mask = copy.metallic_roughness_texture.clone();
        }
    }
    let h = paint(source, shine, red, blue, (colorized, mask), materials, images)?;
    cache.insert(key, (h.clone(), serial));
    if cache.len() > PAINT_CACHE {
        let mut worn: Vec<(u64, PaintKey)> = cache.iter().map(|(k, (_, t))| (*t, *k)).collect();
        worn.sort_unstable_by_key(|w| w.0);
        for (_, k) in &worn[..cache.len() - PAINT_CACHE] {
            cache.remove(k);
        }
    }
    Some(h)
}

/// A copy of `source` whose texture is colorized; None until its image has loaded. `colorized` and `mask`: textures a
/// cached copy already made ([`paint_copy`]).
fn paint(
    source: &Handle<StandardMaterial>,
    shine: bool,
    red: [f32; 3],
    blue: [f32; 3],
    (colorized, mask): (Option<Handle<Image>>, Option<Handle<Image>>),
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Option<Handle<StandardMaterial>> {
    let mut m = materials.get(source)?.clone();
    let atlas = images.get(m.base_color_texture.as_ref()?)?;
    use bevy::render::render_resource::TextureFormat;
    if !matches!(atlas.texture_descriptor.format, TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm) {
        return None;
    }
    // Both from the unpainted atlas.
    let new_mask = if shine && mask.is_none() { Some(shine_mask(atlas)?) } else { None };
    let new_atlas = colorized.is_none().then(|| atlas.clone());
    if shine {
        m.metallic_roughness_texture = mask.or_else(|| new_mask.map(|mask| images.add(mask)));
        (m.perceptual_roughness, m.metallic) = (1., 0.);
    }
    if let Some(h) = colorized {
        m.base_color_texture = Some(h);
        return Some(materials.add(m));
    }
    let mut image = new_atlas?;
    colorize(image.data.as_mut()?, red, blue);
    // perf 10-19: SKATE_TRAFFIC_GPU_ONLY: the colorized copy is only drawn (paint reads the unpainted source, and no
    // pixel reader takes a painted material's texture), so it keeps no RAM copy once uploaded.
    if crate::config::perf_on("SKATE_TRAFFIC_GPU_ONLY") {
        image.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    }
    // Not retail (user 10-19): Performance mode's low-resolution paint (its source is capped as it loads).
    crate::graphics_menu::performance_texture(&mut image);
    m.base_color_texture = Some(images.add(image));
    // The source's own alpha mode: Skate 1's cutout (set in `show` first), Skate 3's opaque chassis.
    Some(materials.add(m))
}

/// Skate 1's car materials (vehicleMaterialClass, vehicleLightMaterialClass) come exported as BLEND, but their one
/// atlas's alpha is 0/255 cutout: blended, the bodies drew see-through and out of order (user 10-14, bug #5). Skate 3's
/// vehicle_glass stays blended. Retail draws them opaque, with no alpha test either: Skate 1 render.big's
/// vehiclematerial.fxml / vehiclelightmaterial.fxml are AlphaBlendDisabled_AlphaTestDisabled, and their pixel shaders
/// never clip, so the atlas's alpha-0 texels draw solid (bug 10-18 #5; a 0.5 cutout cut holes retail doesn't).
fn cutout(material: &str) -> bool {
    matches!(material, "vehicleMaterialClass" | "vehicleLightMaterialClass")
}

/// The car materials [`colorize`] paints. Skate 3's vehicle_defaultPS (shaders_final.big) reads i_colorize_red and
/// i_colorize_blue too, and its chassis atlas carries the same paint texels (green 0, blue 151 on the body), so its
/// cars take their census colours (bug 10-17 #42: every Skate 3 car drew the atlas's own colour).
fn painted(material: &str) -> bool {
    cutout(material) || material == "vehicle_chassis"
}

/// A car material's (perceptual roughness, reflectance) over the export's fully matte one: glass only. A body's
/// shine is masked texel by texel ([`shine_mask`]); a uniform gloss made every car glossy and grey (v12 list 2).
/// INFERRED: the glass's values (vehicle_glass's shader constants are per material data, not traced).
fn finish(material: &str) -> Option<(f32, f32)> {
    (material == "vehicle_glass").then_some((0.08, 0.5))
}

/// Skate 3's chassis (vehicle_defaultPS, decoded): every specular and environment term is scaled by the diffuse
/// alpha squared, so only the texels its atlas marks (trim, chrome, lights) shine and the rest is matte paint. Skate 1's
/// atlases use alpha as a 0/255 cutout instead (`cutout`), so theirs stay as exported.
fn shine_masked(material: &str) -> bool {
    material == "vehicle_chassis"
}

/// Least roughness of a fully marked texel. INFERRED: retail's spec power (m_params[2]) is per material data.
const SHINE_ROUGHNESS: f32 = 0.25;

/// The glTF metallic-roughness map (green roughness, blue metallic) for a chassis atlas: roughness 1 (matte) where its
/// alpha is 0, down to [`SHINE_ROUGHNESS`] at alpha 1, along alpha squared as the shader scales the shine.
fn shine_mask(diffuse: &Image) -> Option<Image> {
    let size = diffuse.texture_descriptor.size;
    // The top level only (a chain's smaller levels follow it).
    let data = diffuse.data.as_ref()?.get(..(size.width * size.height * 4) as usize)?;
    let rgba: Vec<u8> = data
        .chunks_exact(4)
        .flat_map(|px| {
            let a = f32::from(px[3]) / 255.;
            let roughness = 1. + (SHINE_ROUGHNESS - 1.) * a * a;
            [0, (roughness * 255.).round() as u8, 0, 255]
        })
        .collect();
    Some(Image::new(
        size,
        bevy::render::render_resource::TextureDimension::D2,
        rgba,
        bevy::render::render_resource::TextureFormat::Rgba8Unorm,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    ))
}

fn pose_transform(p: &Pose) -> Transform {
    // The GLBs are in the retail car basis: +X left, +Y up, +Z forward.
    Transform { translation: p.pos, rotation: Quat::from_mat3(&Mat3::from_cols(p.left, p.up, p.fwd)), scale: Vec3::ONE }
}

#[allow(clippy::too_many_arguments)]
fn show(
    mut commands: Commands,
    mut traffic: ResMut<Traffic>,
    server: Res<AssetServer>,
    instances: Query<&SceneInstance>,
    spawner: Res<SceneSpawner>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    mut nodes: Query<(Option<&Name>, &mut Transform)>,
    mut tags: Query<&mut TrafficCar>,
    (named, mut materials, mut images): (
        Query<(&MeshMaterial3d<StandardMaterial>, &bevy::gltf::GltfMaterialName)>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<Image>>,
    ),
) {
    let traffic = &mut *traffic;
    // A model unloads on a map change (`gate` lets go of its scene) and reloads under new material ids: the paint
    // copies of the old ones, each a whole recoloured atlas, go with them.
    traffic.painted.retain(|(source, _), _| materials.contains(*source));
    let Some(sim) = &traffic.sim else { return };
    traffic.shown.retain(|serial, s| {
        let keep = sim.cars.iter().flatten().any(|c| c.serial == *serial);
        if !keep {
            commands.entity(s.root).despawn();
        }
        keep
    });
    for c in sim.cars.iter().flatten() {
        let model = &sim.models[c.model];
        let tag = TrafficCar {
            serial: c.serial,
            status: c.status,
            lamps: c.lamps,
            overspeed: c.over,
            speed: c.speed,
            length: c.len,
            half_width: c.hw,
        };
        let s = traffic.shown.entry(c.serial).or_insert_with(|| {
            let root = commands.spawn((Name::new("Traffic car"), pose_transform(&c.pose), Visibility::default(), tag)).id();
            let model_scene = traffic.scenes.entry(c.model).or_insert_with(|| server.load(GltfAssetLabel::Scene(0).from_asset(model.path.clone())));
            let scene = commands.spawn((SceneRoot(model_scene.clone()), ChildOf(root))).id();
            Shown { root, scene, wheels: None }
        });
        // Written only when they change: a car standing still (lights, parked, braked) keeps its subtree out of
        // transform propagation and its meshes out of extraction.
        if let Ok((_, mut t)) = nodes.get_mut(s.root) {
            t.set_if_neq(pose_transform(&c.pose));
        }
        if let Ok(mut t) = tags.get_mut(s.root) {
            t.set_if_neq(tag);
        }
        if s.wheels.is_none() && instances.get(s.scene).is_ok_and(|i| spawner.instance_is_ready(**i)) {
            let mut wheels = Vec::new();
            for e in children.iter_descendants(s.scene) {
                if meshes.contains(e) {
                    // Ruling: no car shadows (the retail traffic shadow path is UNTRACED).
                    commands.entity(e).insert(NotShadowCaster);
                }
                // Shared by every car of the model: changed once (get_mut re-prepares the material).
                if let Ok((material, name)) = named.get(e) {
                    let blended = materials.get(&material.0).is_some_and(|m| matches!(m.alpha_mode, AlphaMode::Blend));
                    if cutout(&name.0) && blended {
                        if let Some(mut m) = materials.get_mut(&material.0) {
                            m.alpha_mode = AlphaMode::Opaque;
                        }
                    }
                    // Before the paint copies, which clone it: the exported material is fully matte (roughness 1).
                    if let Some((roughness, reflectance)) = finish(&name.0)
                        && materials.get(&material.0).is_some_and(|m| m.perceptual_roughness != roughness)
                        && let Some(mut m) = materials.get_mut(&material.0)
                    {
                        m.perceptual_roughness = roughness;
                        m.reflectance = reflectance;
                    }
                    if let (true, Some(colours)) = (painted(&name.0), c.paint)
                        && let Some(h) =
                            paint_copy(&mut traffic.painted, &material.0, shine_masked(&name.0), colours, c.serial, &mut materials, &mut images)
                    {
                        commands.entity(e).insert(MeshMaterial3d(h));
                    }
                }
                if let Ok((Some(name), t)) = nodes.get(e) {
                    if name.as_str().to_ascii_lowercase().contains("wheel") {
                        wheels.push((e, *t));
                    }
                }
            }
            s.wheels = Some(wheels);
        }
        // Wheels roll by distance travelled over the wheel radius about their axle (local X).
        let angle = if model.wheel_radius > 0.0 { c.travelled / model.wheel_radius } else { 0.0 };
        for (e, rest) in s.wheels.iter().flatten() {
            if let Ok((_, mut t)) = nodes.get_mut(*e) {
                t.set_if_neq(*rest * Transform::from_rotation(Quat::from_rotation_x(angle)));
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/traffic.rs"]
mod tests;
