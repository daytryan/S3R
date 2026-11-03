//! Skate 1's tutorial AI riders (P4): Shingo in S.H.I.N.G.O. (S.K.A.T.E.) and Follow Shingo on P1's ai.json
//! (old_ai1.py), as full skaters in physics/riders.rs's `Riders`, keyed `<model>#s1ai{id}` (rider_model). Skate 2's driver
//! (skate2_rider.rs) is the template; research.md 3.2-3.9 hold Skate 1's rules and deltas. skate1_challenge.rs calls
//! the [`Skate1Ai`] natives by Lua AI id.
use crate::{
    physics::{SkaterRuntime, riders::Riders},
    skate1_cast::{FOLDER, M4},
    skate2_rider::{
        AIR_UP, Body, Path, Player, Sample, Step, advance, band, category, is_nollie, lerp3, link, nearest, next_start, place,
        point, signed_angle, slerp4, turn,
    },
};
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use skate_core::{animation::skeleton_input::name::encode, scoring::catalog::IDENTIFIERS};
use std::{
    collections::BTreeMap,
    f32::consts::{FRAC_PI_4, PI},
};

/// A trick-start node's ext record (research 3.3, 56 bytes): trick index +52, type +54, yaw +48 in degrees.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Ext {
    pub node: usize,
    pub trick: i32,
    #[serde(rename = "type")]
    pub kind: i32,
    pub yaw: i32,
}

/// A Skate 1 AI profile (class E9407D23, research 3.2 table).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Profile1 {
    /// Land chance % by tier 0..3: 913D, F83A, 0602, F83A.
    pub land: [f32; 4],
    /// 1DD7: land-chance decay, to the power n.
    pub decay: f32,
    /// The regular / nollie shares of the flip re-roll.
    pub ollie: f32,
    pub nollie: f32,
    /// 6DBA: grabs enabled.
    pub grabs: bool,
    /// E580 / BB90 (trick, weight).
    pub regular: Vec<(i32, f32)>,
    pub nollie_flips: Vec<(i32, f32)>,
    /// CCD0 (character row): GetCharacterPercentageWillDoABDTrick.
    pub abd: i32,
}

#[derive(Deserialize)]
struct File {
    paths: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    rounds: BTreeMap<String, Vec<Vec<String>>>,
    /// The mission's Competitors rows in order (old_ai1.py).
    riders: BTreeMap<String, Vec<String>>,
    profiles: BTreeMap<String, Profile1>,
    /// Trick name -> difficulty tier (Hash_183878B4AFE58B7F).
    tiers: BTreeMap<String, usize>,
}

/// One challenge's paths, as AI.LoadPaths loads them.
pub(crate) struct Paths {
    pub paths: Vec<Path>,
    pub ext: Vec<Vec<Ext>>,
    /// RoundPaths: each round's path names.
    pub rounds: Vec<Vec<String>>,
    /// Trick index (S1 table = IDENTIFIERS[0..138], research 3.2) -> tier.
    pub tiers: BTreeMap<i32, usize>,
}

impl File {
    fn challenge(&self, name: &str) -> Result<Paths, String> {
        let list = self.paths.get(name).ok_or_else(|| format!("no paths for {name}"))?;
        let value = |v: &Value| serde_json::from_value::<Path>(v.clone()).map_err(|e| format!("{name}: {e}"));
        let paths: Vec<Path> = list.iter().map(value).collect::<Result<_, _>>()?;
        for p in &paths {
            let n = p.pos.len();
            let lens = [p.step.len(), p.ticks.len(), p.state.len(), p.board.len(), p.skater.len(), p.widths.len(), p.flags.len()];
            if n < 2 || lens.iter().any(|&l| l != n) || p.tricks.iter().any(|t| t.node >= n) {
                return Err(format!("{name}: path {} is malformed", p.name));
            }
        }
        let ext = list
            .iter()
            .map(|v| {
                let ext = serde_json::from_value::<Option<Vec<Ext>>>(v["tricks"].clone()).map_err(|e| format!("{name}: {e}"))?;
                Ok(ext.unwrap_or_default())
            })
            .collect::<Result<Vec<_>, String>>()?;
        if paths.iter().zip(&ext).any(|(p, ext)| ext.iter().any(|x| x.node >= p.pos.len())) {
            return Err(format!("{name}: a trick start past its path"));
        }
        let tiers = self.tiers.iter().filter_map(|(n, &t)| Some((IDENTIFIERS.iter().position(|i| i.0 == n.as_str())? as i32, t))).collect();
        Ok(Paths { paths, ext, rounds: self.rounds.get(name).cloned().unwrap_or_default(), tiers })
    }
}

impl Paths {
    /// UNTRACED R18: a trick with no tier row. Fallback: tier 0.
    fn tier(&self, trick: i32) -> usize {
        self.tiers.get(&trick).copied().unwrap_or(0)
    }
}

/// A step's path filter (research 3.9 "Filters"): the single key PF+4 (UNTRACED R1: its writer; fallback the
/// wildcard), one path (FollowTrickPattern's match), or the round list PF+20, by path index.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Filter {
    #[default]
    Any,
    Only(usize),
    List(Vec<usize>),
}
impl Filter {
    fn pass(&self, path: usize) -> bool {
        match self {
            Filter::Any => true,
            Filter::Only(p) => *p == path,
            Filter::List(list) => list.contains(&path),
        }
    }
}

/// The behaviour step (ctor 0x82245cf8): mode 1, branching on, no filter, empty avoid list; `pattern` is
/// FollowTrickPattern's trick (+32), spent once performed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Setup {
    pub mode: u8,
    pub branching: bool,
    pub filter: Filter,
    pub avoid: Vec<i32>,
    pub pattern: Option<i32>,
}
impl Default for Setup {
    fn default() -> Self {
        Self { mode: 1, branching: true, filter: Filter::Any, avoid: Vec::new(), pattern: None }
    }
}

/// PF: the round list (Some = PF+248 clear). Its mode flag PF+244 is round +204, 0 in all four S.H.I.N.G.O. rounds,
/// and round +200 is 0 too (research 3.3): neither is exported, so the list is on and its mode 0.
/// UNTRACED R2: PF+248 on a fresh boot. Fallback: list off.
#[derive(Default)]
struct Filters {
    list: Option<Vec<usize>>,
}
impl Filters {
    /// FilterReset (0x827c0558).
    fn reset(&mut self) {
        self.list = None;
    }
    /// FilterPaths(self, round) (0x823f2db0): each name of the row becomes a key; a name with no path matches none.
    fn round(&mut self, paths: &Paths, round: usize) {
        let names = paths.rounds.get(round).into_iter().flatten();
        self.list = Some(names.filter_map(|n| paths.paths.iter().position(|p| &p.name == n)).collect());
    }
    /// StartSkatePath / StartPath on `setup`: the PF+4 key, replaced by the list with mode 0 while the list is on.
    fn apply(&self, setup: &mut Setup) {
        setup.filter = Filter::Any;
        if let Some(list) = &self.list {
            (setup.filter, setup.mode) = (Filter::List(list.clone()), 0);
        }
    }
}

/// Start pick radius (tunables +160 4F87 = 4.0 m) and reject angle (0x8219159c, 50 deg).
const START_RADIUS: f32 = 4.;
const START_ANGLE: f32 = 0.872_664_6;

/// 0x82232108's random term: (int)((r % 100000) x 1e-5 x 400), 0..399 (400.0 at 0x830c398c).
fn rnd(r: u32) -> u32 {
    ((r % 100_000) as f32 * 1e-5 * 400.) as u32
}

/// 0x82232108 for path `i` joined at `node`. None (0xFFFFFFFF) when there is no next node, when the heading to it
/// is 50 deg or more off the skater's, or when one of its trick starts is in the avoid list (0x82238110 arg 0,
/// INFERRED by trick id). Otherwise rnd plus tenths of a degree (572.958 at 0x82191598). The angle (0x82230a90) is
/// INFERRED as Skate 2's SignedAngle about up.
/// ponytail: crowd (1024 x other AIs within 5 nodes) is 0 (R31): one AI per tutorial challenge.
fn score(paths: &Paths, i: usize, node: usize, at: Vec3, forward: Vec3, avoid: &[i32], r: u32) -> Option<u32> {
    let path = &paths.paths[i];
    let next = Vec3::from_array(*path.pos.get(node + 1)?);
    let angle = signed_angle(forward, next - at, Vec3::Y).abs();
    let avoided = paths.ext[i].iter().any(|e| path.state[e.node] == 1 && avoid.contains(&e.trick));
    (angle < START_ANGLE && !avoided).then(|| rnd(r) + (angle * 572.958) as u32)
}

/// 0x82232090: the lowest score (unsigned), the first on a tie; all rejected: the first.
fn lowest(scores: impl Iterator<Item = Option<u32>>) -> usize {
    let mut best = (u32::MAX, 0);
    for (i, s) in scores.enumerate() {
        if let Some(s) = s
            && s < best.0
        {
            best = (s, i);
        }
    }
    best.1
}

/// The start pick (0x82232340): up to 16 paths that pass the filter, with node 0 within 4 m, are scored; the lowest
/// wins. If none is that near: the nearest node 0 among the filtered paths (0x8223a460). UNTRACED R3: whether that
/// honours the filter. Fallback: filtered first, else any path.
fn pick_start(paths: &Paths, setup: &Setup, at: Vec3, forward: Vec3, mut rand: impl FnMut() -> u32) -> Option<usize> {
    let start = |i: usize| Vec3::from_array(paths.paths[i].pos[0]);
    let near: Vec<usize> =
        (0..paths.paths.len()).filter(|&i| setup.filter.pass(i) && start(i).distance(at) <= START_RADIUS).take(16).collect();
    if !near.is_empty() {
        return Some(near[lowest(near.iter().map(|&i| score(paths, i, 0, at, forward, &setup.avoid, rand())))]);
    }
    let closest = |pass: &dyn Fn(usize) -> bool| {
        (0..paths.paths.len()).filter(|&i| pass(i)).min_by(|&a, &b| start(a).distance(at).total_cmp(&start(b).distance(at)))
    };
    closest(&|i| setup.filter.pass(i)).or_else(|| closest(&|_| true))
}

/// Branching (0x82231dd0) on reaching a node with a branch row (UNTRACED R5: +3286; fallback once per node change).
/// The caller skips it in the air (UNTRACED R6: byte +374; fallback airborne) and during an ollie, flip or grind.
/// Candidates are "stay" plus each link whose path is loaded and passes the filter, scored as the start pick from
/// the link's node; the entry weight is never read. A switch returns (path, node), with node the new path's node
/// nearest the skater among n-3..n (0x82237078).
fn branch(paths: &Paths, path: usize, node: usize, setup: &Setup, body: &Body, mut rand: impl FnMut() -> u32) -> Option<(usize, usize)> {
    let row = paths.paths[path].branches.iter().find(|b| b.node == node)?;
    let mut options = vec![(path, node)];
    let links = row.to.iter().filter_map(|l| Some((paths.paths.iter().position(|p| p.name == l.path)?, l.node)));
    options.extend(links.filter(|&(p, n)| setup.filter.pass(p) && n < paths.paths[p].pos.len()));
    let scores: Vec<_> = options.iter().map(|&(p, n)| score(paths, p, n, body.position, body.forward, &setup.avoid, rand())).collect();
    let pick = lowest(scores.into_iter());
    let (p, n) = options[pick];
    let pos = &paths.paths[p].pos;
    let d = |i: usize| Vec3::from_array(pos[i]).distance(body.position);
    let join = (n.saturating_sub(3)..=n).min_by(|&a, &b| d(a).total_cmp(&d(b)))?;
    (pick > 0).then_some((p, join))
}

/// Skate 1's trick table rows (s1.pe 0x820cf940 = IDENTIFIERS[0..138], research 3.2).
fn valid(trick: i32) -> bool {
    (0..=138).contains(&trick)
}

/// UNTRACED R9 (0x8222f4f0): when a pattern trick suits the action. Fallback: its type (IDENTIFIERS .2) is the
/// action's, ollie (1) and flip (2) counting as one group.
fn suits(trick: i32, kind: i32) -> bool {
    let t = category(trick);
    t == kind || (matches!(t, 1 | 2) && matches!(kind, 1 | 2))
}

/// The trick a start node performs (0x82230928), by its type and the step's mode; None clears the action:
/// - 1 ollie / 2 flip: mode 0 the recorded trick, mode 1 the re-roll when due (`rolled`), mode 2 the pattern trick;
/// - 4 grab: only with profile 6DBA; 5 grind, 8 manual, 9 powerslide: modes 0/1 recorded, mode 2 the pattern trick;
/// - 3, 6, 7, > 9 (no handler), a mode >= 3, a pattern trick that doesn't suit, or an index past the table: None.
///
/// The pattern trick (0x82230370) is spent once used (+2780 steps past its one element).
fn dispatch(kind: i32, recorded: i32, mode: u8, pattern: &mut Option<i32>, grabs: bool, rolled: Option<i32>) -> Option<i32> {
    if mode >= 3 || !matches!(kind, 1 | 2 | 4 | 5 | 8 | 9) || (kind == 4 && !grabs) {
        return None;
    }
    let trick = match mode {
        2 => {
            let p = pattern.filter(|&p| suits(p, kind))?;
            *pattern = None;
            p
        }
        1 if matches!(kind, 1 | 2) => rolled.unwrap_or(recorded),
        _ => recorded,
    };
    valid(trick).then_some(trick)
}

/// Mode 1's re-roll gate (0x82230238 -> 0x822376b0): the next event node within 600 is the trick's end more than 50
/// on, or a start whose link (table +4) is this trick. UNTRACED R15: the units of 600 / 50. Fallback: ticks.
fn reroll_due(path: &Path, n: usize, trick: i32) -> bool {
    let mut ticks = 0;
    for i in n + 1..path.pos.len() {
        ticks += path.ticks[i];
        if ticks > 600 {
            return false;
        }
        match path.state[i] {
            2 => return ticks > 50,
            1 => return path.tricks.iter().find(|t| t.node == i).is_some_and(|t| link(t.trick) == trick),
            _ => {}
        }
    }
    false
}

/// Mode 1's draw (0x8223e6c0 -> 0x8223e450): the regular list (E580) when r1 x (ollie + nollie) < ollie, else the
/// nollie list (BB90); r2 walked against the weights normalised to 1 (0x8223e1c8); the first entry is the fallback.
/// None: an empty list (the recorded trick stays).
fn reroll(profile: &Profile1, r1: f32, r2: f32) -> Option<i32> {
    let list = if r1 * (profile.ollie + profile.nollie) < profile.ollie { &profile.regular } else { &profile.nollie_flips };
    let sum: f32 = list.iter().map(|e| e.1).sum();
    let scale = if sum > 0. { 1. / sum } else { 1. };
    let mut c = 0.;
    list.iter()
        .find(|e| {
            c += e.1 * scale;
            r2 < c
        })
        .or(list.first())
        .map(|e| e.0)
}

/// The landing roll (0x8223e7f0): bail when rand()/32767 x 100 > land[tier] x decay^n.
fn bails(profile: &Profile1, tier: usize, n: u32, r: f32) -> bool {
    r * 100. > profile.land[tier.min(3)] * profile.decay.powi(n as i32)
}

/// 180s in a yaw, degrees (0x82238250): (int)((|yaw| + tol) / 180). UNTRACED R7: tol ([[pathdb+0xB314]+1088]).
/// Fallback: 90.
fn halves(yaw: f32) -> u32 {
    ((yaw.abs() + 90.) / 180.) as u32
}

/// FollowTrickPattern's matcher (0x8223ae80) under the PF+4 wildcard (R1): paths with a trick start of `trick`
/// whose first trick start has `spins` 180s; up to 40. UNTRACED R8: the trick equivalence 0x82235118. Fallback: the
/// exact index.
fn pattern_paths(paths: &Paths, trick: i32, spins: u32) -> Vec<usize> {
    (0..paths.paths.len())
        .filter(|&i| {
            let starts = || paths.ext[i].iter().filter(|e| paths.paths[i].state[e.node] == 1);
            starts().any(|e| e.trick == trick) && starts().min_by_key(|e| e.node).is_some_and(|e| halves(e.yaw as f32) == spins)
        })
        .take(40)
        .collect()
}

const DEG: f32 = PI / 180.;
/// Skate 1's controller tunables (class CB59C2182C87F89F `default` row, research 3.5 items 2 / 3).
const TURN_FULL: f32 = 18. * DEG; // +28 FED2
const PUSH_ERR: f32 = 40. * DEG; // +108 8692
const BRAKE_RAMP: f32 = 0.1; // +132 7E70
const BRAKE_ERR: f32 = 40. * DEG; // +148 8AC7
/// +116 DCD5 (Skate 2: 0.25). UNTRACED R20: the target formula (0x82231a20 matched, not read). Fallback: Skate 2's.
const BAND_WEIGHT: f32 = 0.3;
const MANUAL: f32 = 0.5; // +24 742E

/// ponytail: retail's CRT rand() (0x82e42190, 15-bit) can't be matched (research 3.9); xorshift with its spread.
#[derive(Clone)]
pub(crate) struct Rng(u32);
impl Default for Rng {
    fn default() -> Self {
        Rng(0x9E37_79B9)
    }
}
impl Rng {
    pub(crate) fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    /// rand() / 32767.
    fn unit(&mut self) -> f32 {
        (self.next() & 0x7FFF) as f32 / 32767.
    }
}

/// What the lead's Lua hears about a rider.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AiEvent {
    /// A popped ollie or flip landed (or bailed by the roll): its trick, its 180s, switch (UNTRACED R26; fallback
    /// false, as skate1_online).
    Landed { trick: i32, spins: u32, switched: bool, bail: bool },
    Wipeout,
}

/// Skate 1's path controller (research 3.5: Skate 2's per-frame shape with Skate 1's tunables and deltas).
struct Driver1 {
    path: usize,
    setup: Setup,
    /// The follow band (+3304): set at reset, cleared by SwitchOffFollowMeRubberBanding.
    band: bool,
    at: Sample,
    prev: Option<Vec3>,
    wiped: u32,
    wiping: bool,
    /// A landing's bail was posted as its wipeout (`landing`): the fall it starts isn't posted again.
    bailed: bool,
    /// +3188: the trick type in progress, 0 none; +3192 a manual's balance.
    action: i32,
    manual: f32,
    /// +3284. UNTRACED R21: its rule. Fallback: Skate 2's lock.
    locked: bool,
    /// The popped ollie or flip and the facing turned since, in degrees (R16); the last tick's state.
    air: Option<(i32, f32)>,
    airborne: bool,
    facing: Vec3,
}

impl Driver1 {
    fn new(path: usize, setup: Setup, band: bool) -> Self {
        let at = Sample { node: 0, frac: 0. };
        Self { path, setup, band, at, prev: None, wiped: 0, wiping: false, bailed: false, action: 0, manual: 0., locked: false, air: None, airborne: false, facing: Vec3::Z }
    }
    /// UNTRACED R4: the join at node 0 (0x8222f1a8) moving the skater. Fallback: onto node 0 facing along it
    /// (Skate 2's StartPath).
    fn start(&self, data: &Paths) -> M4 {
        place(&data.paths[self.path], 0)
    }
    fn join(&mut self, path: usize, node: usize) {
        (self.path, self.at, self.action, self.locked) = (path, Sample { node, frac: 0. }, 0, false);
    }

    fn step(&mut self, data: &Paths, profile: &Profile1, body: Body, player: Option<&Player>, rng: &mut Rng, events: &mut Vec<AiEvent>) -> Step {
        let mut out = Step::default();
        // UNTRACED R13: off-board handling. Fallback: no corridor watchdog (research5 1); a wipeout is reported once,
        // and down over 2 s he goes back on his path (Skate 2's INFERRED reset).
        if body.wiping_out && !self.wiping && !std::mem::take(&mut self.bailed) {
            events.push(AiEvent::Wipeout);
        }
        self.wiping = body.wiping_out;
        self.wiped = if body.wiping_out { self.wiped + 1 } else { 0 };
        if self.wiped > 120 {
            (self.wiped, self.action, self.air) = (0, 0, None);
            out.reposition = Some(place(&data.paths[self.path], self.at.node));
            return out;
        }
        // Speed (0x82231c20, read: Skate 2's 0x823f9210).
        let moved = self.prev.map(|p| body.position - p).filter(|d| d.abs().max_element() > 1.192_092_9e-7);
        let vel = moved.map_or(body.velocity, |d| d * 60.);
        self.prev = Some(body.position);
        let mut speed = vel.length();
        if speed > 0.01 && body.forward.dot(vel) < 0. {
            speed = -speed;
        }
        // Nearest point. UNTRACED R31: the search's far end. Fallback: Skate 2's 12 nodes.
        let old = self.at.node;
        self.at = nearest(&data.paths[self.path], body.position, old.saturating_sub(3), old + 12);
        if self.at.node + 1 >= data.paths[self.path].pos.len() {
            // UNTRACED R14: a recording's end. Fallback: Skate 2's rule (the nearest filtered path starting near the
            // end) within Skate 1's start radius. INFERRED: he_u2_follow01's first recordings end 2.3-3.9 m short of
            // the next ones' starts; Skate 2's 2.5 m left A_8's end (2.502 m from A_15) joined to nothing and Shingo
            // stopped there for good (user 10-15 run15).
            let end = Vec3::from_array(*data.paths[self.path].pos.last().unwrap());
            let starts = (0..data.paths.len()).filter(|&i| self.setup.filter.pass(i));
            let next = starts.map(|i| (i, end.distance(Vec3::from_array(data.paths[i].pos[0])))).filter(|&(_, d)| d < START_RADIUS);
            if let Some((next, _)) = next.min_by(|a, b| a.1.total_cmp(&b.1)) {
                self.join(next, 0);
            }
        } else if self.at.node > old {
            for n in old + 1..=self.at.node {
                self.dispatch_node(&mut out, data, profile, n, rng);
            }
            if self.setup.branching
                && !body.airborne
                && !matches!(self.action, 1 | 2 | 5)
                && let Some((p, n)) = branch(data, self.path, self.at.node, &self.setup, &body, || rng.next())
            {
                self.join(p, n);
            }
        }
        let path = &data.paths[self.path];
        let here = point(path, self.at);
        let (s2, s30, s45) = (advance(path, self.at, 2.), advance(path, self.at, 30.), advance(path, self.at, 45.));
        let dist = body.position.distance(here);
        // Skate 1's one width byte: the corridor's half width (research5 1), exported in 0.02 m.
        let width = path.widths[self.at.node][0] as f32 * 0.02;
        if !self.locked && (self.at.node > 0 || self.at.frac > 1e-4) && (dist < width || dist < 0.5) {
            self.locked = true;
        }
        let rb = match player {
            Some(p) if self.band => band(&body, p),
            _ => 0.,
        };
        // Heading error (0x82230de8 = Skate 2's 0x823f7ed0).
        let fwd = body.forward;
        let mut path_fwd = Quat::from_array(slerp4(&path.board, s2)) * Vec3::Z;
        if path_fwd.dot(fwd) < 0. {
            path_fwd = -path_fwd;
        }
        let a1 = signed_angle(fwd, path_fwd, body.up);
        let mut d = point(path, s45) - body.position;
        d -= body.up * d.dot(body.up) / body.up.dot(body.up);
        let a2 = if d.length_squared() > 1e-5 { signed_angle(fwd, d, body.up) } else { a1 };
        let e = if dist < 1. { (a1 + a2) * 0.5 } else { a2 };
        // Lookahead: the next start within 60 frames, within 3 m of the path (Skate 2's 0x823f6688).
        let ahead = if dist > 3. {
            None
        } else {
            next_start(path, self.at, 60).and_then(|(n, f)| Some((data.ext[self.path].iter().find(|x| x.node == n)?, f)))
        };
        self.landing(&mut out, data, profile, &body, rng, events);
        if body.airborne {
            // UNTRACED R12: the air turn. Fallback: Skate 2's (0x823fdf08), against the recording's skater facing:
            // 2 deg dead zone, full at 10. About world up (skate2_rider's AIR_UP).
            let want = Quat::from_array(slerp4(&path.skater, s2)) * Vec3::Z;
            let a = signed_angle(body.facing, want, AIR_UP);
            let x = a.abs() - 2. * DEG;
            if x > 0. {
                turn(&mut out, (if a > 0. { -1. } else { 1. }) * (x / (8. * DEG)).min(1.));
            }
            self.signals(&mut out, path, e, false, ahead);
            return out;
        }
        // Ground turn (0x82231498).
        let m = (e.abs() / TURN_FULL).min(1.);
        turn(&mut out, if e > 0. { -m } else { m });
        // Locomotion.
        let mut vref = Vec3::from_array(lerp3(&path.step, s30)).length() * 60.;
        let p30 = point(path, s30);
        let flat = Vec2::new(p30.x - here.x, p30.z - here.z).length();
        let slope = if flat > 0.001 { (p30.y - here.y) / flat } else { 0. };
        // UNTRACED: leaving a standing start. `advance` never moves off node 0 at its start (0x823e90d8), and the
        // nollie path 0 opens on 64 still ticks, so the target stayed 0 and P-Rod never pushed (playtest 10-15).
        // Fallback: the off-path floor below.
        let standing = self.at.node == 0 && self.at.frac <= 0.001;
        if dist > 2. || standing {
            vref = vref.max(4.);
        }
        let target = vref + BAND_WEIGHT * vref * rb;
        let mut pushed = false;
        if speed < target {
            // Push (0x822315e0): Skate 2's trick-soon and steep rules; no action skip, no corridor gate.
            let mut k = if ahead.is_some_and(|(_, f)| f < 40) && speed > 2. { 0.3 } else { 0.95 };
            if slope < -0.22 {
                k *= 0.3;
            }
            let path_vel = Vec3::from_array(lerp3(&path.step, self.at));
            if path_vel.dot(fwd) >= 0. && speed < k * target && e.abs() < PUSH_ERR {
                // UNTRACED R19: Skate 1's four push signals (0x831a76e0..7728). Fallback: Skate 2's three.
                out.intents.extend([("LeftPush", 1.), ("NewPush", 1.), ("Pushing", 1.)]);
                pushed = true;
            }
        } else if self.at.node >= 1 {
            // Brake (0x82231848): k 0.3, 0.12 up a slope over 0.22.
            let k = if slope > 0.22 { 0.12 } else { 0.3 };
            let s = speed * k;
            if s > target && e.abs() < BRAKE_ERR {
                out.intents.push((if self.action == 8 { "ManualBrake" } else { "Brake" }, ((s - target) / BRAKE_RAMP).min(1.)));
            }
        }
        self.signals(&mut out, path, e, pushed, ahead);
        out
    }

    /// Trick dispatch (0x82230928) on node `n`: a start takes its trick by type and mode; an end clears the action.
    fn dispatch_node(&mut self, out: &mut Step, data: &Paths, profile: &Profile1, n: usize, rng: &mut Rng) {
        let path = &data.paths[self.path];
        match path.state[n] {
            1 => {
                let Some(x) = data.ext[self.path].iter().find(|x| x.node == n) else {
                    self.action = 0;
                    return;
                };
                self.action = x.kind;
                let due = self.setup.mode == 1 && matches!(x.kind, 1 | 2) && reroll_due(path, n, x.trick);
                let rolled = if due { reroll(profile, rng.unit(), rng.unit()) } else { None };
                match dispatch(x.kind, x.trick, self.setup.mode, &mut self.setup.pattern, profile.grabs, rolled) {
                    Some(trick) => self.begin(out, trick),
                    None => self.action = 0,
                }
            }
            2 => {
                (self.action, self.manual) = (0, 0.);
                if !self.airborne {
                    self.air = None;
                }
            }
            _ => {}
        }
    }

    /// A trick's start. An ollie or flip pops with Skate 2's 0x823f6938 signals (UNTRACED R10: 0x8222f558's own;
    /// fallback Skate 2's). A manual takes its balance. UNTRACED R11: grind, grab and slide start signals. Fallback:
    /// none at the start (the physics grinds; no grabs on the disc paths); a locked slide writes its side each tick.
    fn begin(&mut self, out: &mut Step, trick: i32) {
        match self.action {
            1 | 2 => {
                let name = IDENTIFIERS[trick as usize].0;
                out.intents.extend([("Trick", 1.), (name, 1.), ("GestureSpeed", 1.), ("TrickHeight", 1.), ("DontMirrorTrick", 1.)]);
                (self.air, self.bailed) = (Some((trick, 0.)), false);
            }
            8 => self.manual = if trick == 3 { -MANUAL } else { MANUAL },
            _ => {}
        }
    }

    /// Per-tick signals:
    /// - UNTRACED R12: Antic before an ollie or flip, and Crouch where the recording crouched (flag bit 1).
    ///   Fallback: Skate 2's;
    /// - a manual's balance;
    /// - a locked powerslide's side (research 3.5 item 4: turn > 0, i.e. e < 0, is Right).
    fn signals(&self, out: &mut Step, path: &Path, e: f32, pushed: bool, ahead: Option<(&Ext, i32)>) {
        let antic = ahead.filter(|(x, _)| matches!(x.kind, 1 | 2) && !pushed && e.abs() < FRAC_PI_4 && self.action != 8);
        if let Some((x, _)) = antic {
            out.intents.extend([("AnticMag", 1.), ("AnticAngle", if is_nollie(x.trick) { PI } else { 0. })]);
        }
        let node = if self.at.frac < 0.5 { self.at.node } else { (self.at.node + 1).min(path.flags.len() - 1) };
        if path.flags[node] & 0x02 != 0 && !pushed && antic.is_none() && self.action == 0 {
            out.intents.push(("Crouch", 1.));
        }
        match self.action {
            8 => out.intents.push(("Manual", self.manual)),
            9 if self.locked => {
                out.intents.extend(if e < 0. { [("RightSlideStart", 1.), ("RightSlide", 0.4)] } else { [("LeftSlideStart", 1.), ("LeftSlide", 0.4)] })
            }
            _ => {}
        }
    }

    /// Air tracking. UNTRACED R16: the landing's n (skater sim +152). Fallback: the 180s of the facing turned in the
    /// air. UNTRACED R17: which landings roll. Fallback: the landing after a popped ollie or flip rolls the bail
    /// (0x82230af0 -> 0x8223e7f0) and is reported. UNTRACED R30: the reported trick. Fallback: the skater's scored
    /// trick, else the popped one.
    fn landing(&mut self, out: &mut Step, data: &Paths, profile: &Profile1, body: &Body, rng: &mut Rng, events: &mut Vec<AiEvent>) {
        if body.airborne
            && let Some((_, yaw)) = self.air.as_mut()
        {
            *yaw += signed_angle(self.facing, body.facing, AIR_UP).to_degrees();
        }
        let landed = self.airborne && !body.airborne;
        (self.airborne, self.facing) = (body.airborne, body.facing);
        let Some((popped, yaw)) = self.air.take_if(|_| landed) else { return };
        let scored = (0..=138).find(|&i| body.trick == Some(encode(IDENTIFIERS[i as usize].0.as_bytes())));
        let trick = scored.unwrap_or(popped);
        let spins = halves(yaw);
        let bail = bails(profile, data.tier(trick), spins, rng.unit());
        if bail {
            out.intents.push(("WipeOutRequest", 1.));
            // INFERRED: the bail's wipeout goes out before the landing's aircompleted, in its tick, as the player's
            // (skate1_challenge::observe; the air finalised wiped, 0x827a38c8 -> 0x8279e640). he_u1_skate01's leader
            // reads `Wipedout ~= true` against the number, so only the wipeout's LockoutNPC stops a bail setting the
            // trick; posted a tick after it, Shingo set every trick and never lost the lead (playtest run10).
            events.push(AiEvent::Wipeout);
            self.bailed = true;
        }
        events.push(AiEvent::Landed { trick, spins, switched: false, bail });
    }
}

/// The model a rider (ai.json `riders` value) draws.
/// - INFERRED R28 (research 3.2): shingo_skate / shingo_follow inherit ip_shingo's model.
/// - INFERRED (P6 R3, ledger U5): P-Rod's competitor string "paul_rodriguez" draws his marquee model.
/// - INFERRED (user 10-15, Community Spirit): eric_unknown / fabio_unknown / shingo_unknown draw their profile rows'
///   ip_eric / ip_fabio / ip_shingo models (old_ai1.py profile rows; the cast exports those three).
/// - UNTRACED: the character row (class E57478F796EBC28C) behind 0x827c1538. Fallback: this table; cost: a wrong
///   leader model.
fn rider_model(rider: &str) -> &'static str {
    match rider {
        "paul_rodriguez" => "Paul_rodriguez",
        "eric_unknown" => "ip_eric",
        "fabio_unknown" => "ip_fabio",
        _ => "IP_shingo",
    }
}

/// The rider's name on the S.K.A.T.E. HUD (SkateMission.GetPlayerName). RETAIL 0x82319110 gives an AI player (record
/// +0x18 == 2) the localized text attribute 0xB95F05E301B01FFB of its character record (0x823f4030 -> 0x823f00c0;
/// "AI Player %d" only when it is empty or ###GENERIC###, 0x823f3ef0): shingo_skate (via ip_shingo) and shingo_follow
/// ID_MARQUEE_SHINGO, paul_rodriguez (via rodriguez) ID_CHEAT_PROSKATER17_SHORT, eric_unknown / fabio_unknown
/// ID_MARQUEE_ERIC / ID_MARQUEE_FABIO (their profile rows' marquee ids). The text is english.big's.
/// ponytail: English only; strings.json lacks these labels (old_challenges.py SCREEN_STRINGS would carry them).
fn rider_name(rider: &str) -> &'static str {
    match rider {
        "paul_rodriguez" => "P-ROD",
        "eric_unknown" => "ERIC",
        "fabio_unknown" => "FABIO",
        _ => "SHINGO",
    }
}

/// One AI skater by Lua id.
struct Ai {
    key: String,
    /// [`rider_name`].
    name: &'static str,
    /// The spawn matrix the cast keeps (a change would respawn the model); teleports go to `place`.
    at: Option<M4>,
    place: Option<M4>,
    spawned: bool,
    profile: Profile1,
    band: bool,
    /// UNTRACED R27: SpawnSkaterRegular's effect on the rider. Fallback: stored and reported only.
    regular: bool,
    pinned: bool,
    setup: Setup,
    /// A path step to start on the next tick (the start pick needs the rider's body).
    start: bool,
    driver: Option<Driver1>,
}

/// The Lua natives' AI state (research 3.4) for skate1_challenge.rs; `drive` runs it on the fixed tick.
#[derive(Resource, Default)]
pub(crate) struct Skate1Ai {
    file: Option<File>,
    paths: Option<Paths>,
    filters: Filters,
    ais: BTreeMap<i64, Ai>,
    events: Vec<(i64, AiEvent)>,
    rng: Rng,
}

impl Skate1Ai {
    /// The installed ai.json (old_ai1.py), read when the tutorial data loads.
    pub(crate) fn open(&mut self, root: &std::path::Path) -> Result<(), String> {
        let json = std::fs::read_to_string(root.join(FOLDER).join("ai.json")).map_err(|e| format!("ai.json: {e}"))?;
        self.parse(&json)
    }
    fn parse(&mut self, json: &str) -> Result<(), String> {
        self.file = Some(serde_json::from_str(json).map_err(|e| format!("ai.json: {e}"))?);
        Ok(())
    }
    /// AI.LoadPaths(self) (0x827c0440).
    pub(crate) fn load_paths(&mut self, challenge: &str) -> bool {
        self.paths = self.file.as_ref().and_then(|f| f.challenge(challenge).map_err(|e| warn!("Skate 1 AI: {e}")).ok());
        self.paths.is_some()
    }
    /// AI.UnloadPaths: every driver stops.
    pub(crate) fn unload_paths(&mut self) {
        self.paths = None;
        for a in self.ais.values_mut() {
            (a.driver, a.start) = (None, false);
        }
    }
    /// AI.SelectCharacters(self) (0x827c1538): competitor `id` rides the challenge's Competitors row id - 1 (ai.json
    /// `riders`; RETAIL 0x823f2e90 fills slot 0x218 + 8n in row order; INFERRED: Lua id n + 1 is slot n, as Community
    /// Spirit's CompetitorID[i] = i - 1 with the player 0). A one-row mission's every id rides its row.
    /// `band` = ChallengeType 23 (the follow band, research 3.5 item 7).
    pub(crate) fn select(&mut self, id: i64, challenge: &str, band: bool) -> bool {
        let Some((model, name, profile)) = self.file.as_ref().and_then(|f| {
            let rows = f.riders.get(challenge)?;
            let rider = rows.get(usize::try_from(id - 1).unwrap_or(0).min(rows.len().checked_sub(1)?))?;
            Some((rider_model(rider), rider_name(rider), f.profiles.get(rider)?.clone()))
        }) else {
            return false;
        };
        let key = format!("{model}#s1ai{id}");
        let ai = Ai { key, name, at: None, place: None, spawned: false, profile, band, regular: true, pinned: false, setup: Setup::default(), start: false, driver: None };
        self.ais.insert(id, ai);
        true
    }
    /// AI.SpawnAIAtLocation (0x827c0340).
    pub(crate) fn spawn(&mut self, id: i64, at: M4) {
        if let Some(a) = self.ais.get_mut(&id) {
            (a.at, a.place) = (Some(at), Some(at));
        }
    }
    /// AI.RepositionAI: a teleport that drops the path step (Skate 2's Reposition).
    pub(crate) fn reposition(&mut self, id: i64, at: M4) {
        if let Some(a) = self.ais.get_mut(&id) {
            (a.place, a.driver, a.start) = (Some(at), None, false);
        }
    }
    /// AI.UnspawnAI: the rider leaves on the next tick. (CAIBackToAmbient is not this: retail 0x827c1648 is
    /// `li r3,0; blr`, a no-op.)
    pub(crate) fn unspawn(&mut self, id: i64) {
        self.ais.remove(&id);
    }
    /// Tutorial stop, map change, session join.
    pub(crate) fn clear(&mut self) {
        self.ais.clear();
        self.events.clear();
        self.filters.reset();
        self.paths = None;
    }
    /// AI.PinAI / AI.UnpinAI. UNTRACED R24: pinning's effect on the rider. Fallback: no signals while pinned.
    pub(crate) fn pin(&mut self, id: i64, pinned: bool) {
        if let Some(a) = self.ais.get_mut(&id) {
            a.pinned = pinned;
        }
    }
    /// AI.SpawnSkaterRegular (0x827c1e08).
    pub(crate) fn set_regular(&mut self, id: i64, regular: bool) {
        if let Some(a) = self.ais.get_mut(&id) {
            a.regular = regular;
        }
    }
    /// AI.GetIfSkaterIsRegularStance.
    pub(crate) fn regular(&self, id: i64) -> bool {
        self.ais.get(&id).is_none_or(|a| a.regular)
    }
    /// AI.GetCharacterPercentageWillDoABDTrick (0x827c1c90): the CCD0 row value.
    pub(crate) fn abd(&self, id: i64) -> i32 {
        self.ais.get(&id).map_or(0, |a| a.profile.abd)
    }
    /// AI.FilterReset (0x827c0558).
    pub(crate) fn filter_reset(&mut self) {
        self.filters.reset();
    }
    /// AI.FilterPaths(self, round) (0x827c0510), round 0-based.
    pub(crate) fn filter_paths(&mut self, round: usize) {
        if let Some(paths) = &self.paths {
            self.filters.round(paths, round);
        }
    }
    /// Skate.StartSkatePath(id, branching, avoid used) (0x827bf078); AI.StartPath(id) (0x827c0588) is
    /// (id, true, []). `avoid` = the used-trick ring when arg 3 is true (research 3.4).
    pub(crate) fn start_path(&mut self, id: i64, branching: bool, avoid: Vec<i32>) {
        let Some(a) = self.ais.get_mut(&id) else { return };
        (a.setup.branching, a.setup.avoid) = (branching, avoid);
        self.filters.apply(&mut a.setup);
        (a.start, a.driver) = (true, None);
    }
    /// AI.FollowTrickPattern(id, Stored180SpinCount) (0x827c13a8, research 3.4): on a match, mode 2 with the
    /// player's trick on that path, branching off; without one, false, the wildcard filter and the step as it was.
    /// INFERRED R29: either way he sets off.
    pub(crate) fn follow_trick_pattern(&mut self, id: i64, trick: i32, spins: u32) -> bool {
        let (Some(a), Some(paths)) = (self.ais.get_mut(&id), &self.paths) else { return false };
        let found = pattern_paths(paths, trick, spins);
        a.setup.filter = Filter::Any;
        if !found.is_empty() {
            let path = found[self.rng.next() as usize % found.len()];
            (a.setup.pattern, a.setup.branching, a.setup.mode, a.setup.filter) = (Some(trick), false, 2, Filter::Only(path));
        }
        (a.start, a.driver) = (true, None);
        !found.is_empty()
    }
    /// AI.ClearBehavior: he stops riding. UNTRACED R25: a fresh step after it. Fallback: yes.
    pub(crate) fn clear_behavior(&mut self, id: i64) {
        if let Some(a) = self.ais.get_mut(&id) {
            (a.setup, a.start, a.driver) = (Setup::default(), false, None);
        }
    }
    /// AI.SwitchOffFollowMeRubberBanding (0x827c1090 -> 0x82234f20).
    pub(crate) fn band_off(&mut self, id: i64) {
        if let Some(a) = self.ais.get_mut(&id) {
            a.band = false;
            if let Some(d) = &mut a.driver {
                d.band = false;
            }
        }
    }
    /// The rider's key: `Riders::get(key)` gives its body (position, camera target).
    pub(crate) fn key(&self, id: i64) -> Option<&str> {
        self.ais.get(&id).map(|a| a.key.as_str())
    }
    /// The rider's HUD name ([`rider_name`]).
    pub(crate) fn name(&self, id: i64) -> Option<&'static str> {
        self.ais.get(&id).map(|a| a.name)
    }
    /// The spawned riders' cast keys and spawn matrices (skate1_cast).
    pub(crate) fn riders(&self) -> Vec<(String, M4)> {
        self.ais.values().filter_map(|a| Some((a.key.clone(), a.at?))).collect()
    }
    /// This frame's landings and wipeouts, by Lua id.
    pub(crate) fn drain(&mut self) -> Vec<(i64, AiEvent)> {
        std::mem::take(&mut self.events)
    }
}

/// Riders the fixed tick keeps: every one not Skate 1's, and Skate 1's whose AI is still selected.
fn keeps(ai: &Skate1Ai, key: &str) -> bool {
    !key.contains("#s1ai") || ai.ais.values().any(|a| a.key == key)
}

/// The riders' fixed tick: after the player's, before riders::advance. It applies spawns and teleports, starts
/// paths (the start pick on the rider's body, then the join, R4), and writes each driver's step as its rider's
/// signals. Riders whose AI left are removed (Skate 1 keys only).
pub(crate) fn drive(
    mut ai: ResMut<Skate1Ai>,
    riders: Option<ResMut<Riders>>,
    skater: Res<SkaterRuntime>,
    (map, online): (Option<Res<crate::map_transition::CurrentMap>>, Option<Res<crate::multiplayer::Online>>),
) {
    let Some(mut riders) = riders else { return };
    riders.retain(|key| keeps(&ai, key));
    // D4: the tutorial's AI may spawn (its map, offline, as skate1_challenge loads it): keep a rider's load ready.
    riders.warm |= ai.file.is_some() && map.is_some_and(|m| crate::skate1_hud::skate1_map(&m)) && !online.is_some_and(|o| o.0);
    let m = &skater.animated_skeleton.roots.animation_to_world;
    let player = Player { position: Vec3::new(m[3][0], m[3][1], m[3][2]), wiping_out: skater.wiping_out() };
    let Skate1Ai { ais, paths, rng, events, .. } = &mut *ai;
    for (&id, a) in ais.iter_mut() {
        if let Some(at) = a.place.take() {
            if std::mem::replace(&mut a.spawned, true) {
                riders.teleport(&a.key, at);
            } else {
                riders.spawn(&a.key, at);
            }
        }
        let Some(rider) = riders.get_mut(&a.key).filter(|r| !r.failed()) else { continue };
        let (Some(data), false) = (paths.as_ref(), a.pinned) else {
            rider.intents.clear();
            continue;
        };
        // RepositionAI then StartPath in one frame (the nollie lesson's leader_ready): the start waits for the
        // teleport's tick, so it is picked where he now stands and its own teleport isn't refused, which left him at
        // his spawn (playtest 10-15). INFERRED: retail's RepositionAI moves him at once.
        if a.start && rider.teleporting() {
            rider.intents.clear();
            continue;
        }
        let body = rider.body();
        if std::mem::take(&mut a.start) {
            let pick = pick_start(data, &a.setup, body.position, body.forward, || rng.next());
            a.driver = pick.map(|p| Driver1::new(p, a.setup.clone(), a.band));
            if let Some(d) = &a.driver {
                rider.travel_to(d.start(data));
            }
            rider.intents.clear();
            continue;
        }
        let Some(d) = a.driver.as_mut() else {
            rider.intents.clear();
            continue;
        };
        let mut out = Vec::new();
        let step = d.step(data, &a.profile, body, Some(&player), rng, &mut out);
        rider.intents = step.intents;
        if let Some(at) = step.reposition {
            rider.travel_to(at);
        }
        events.extend(out.into_iter().map(|e| (id, e)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate2_rider::{Branch, Link, TrickNode};
    use serde_json::json;

    /// A straight path along +Z at x = `x`: six nodes 1 m / 10 ticks apart; `start` puts one trick start
    /// (node, trick, type, yaw) as old_ai1.py writes it.
    fn path_json(name: &str, x: f32, start: Option<(usize, i32, i32, i32)>) -> Value {
        let n = 6;
        let mut state = vec![0u8; n];
        let mut tricks = Vec::new();
        if let Some((node, trick, kind, yaw)) = start {
            state[node] = 1;
            tricks.push(json!({"node": node, "trick": trick, "name": "?", "type": kind, "launch": true, "yaw": yaw, "pitch": 0}));
        }
        json!({
            "name": name,
            "pos": (0..n).map(|i| [x, 0., i as f32]).collect::<Vec<_>>(),
            "step": vec![[0., 0., 0.1]; n],
            "ticks": (0..n).map(|i| if i == 0 { 0 } else { 10 }).collect::<Vec<_>>(),
            "state": state,
            "board": vec![[0., 0., 0., 1.]; n],
            "skater": vec![[0., 0., 0., 1.]; n],
            "widths": vec![[75, 75]; n],
            "flags": vec![0; n],
            "tricks": tricks,
            "branches": [],
        })
    }

    /// old_ai1.py's shape: S.H.I.N.G.O. with an ollie 222 deg (one 180), a kickflip and a varial path; Follow with one.
    fn ai_json() -> String {
        let profile = |abd: i32, land: [f32; 4], decay: f32| {
            json!({
                "gesture": 90., "land": land, "decay": decay, "ollie": 9., "nollie": 1., "grabs": true,
                "regular": [[92, 5.], [96, 5.]], "nollie_flips": [[113, 5.]], "abd": abd, "unknown": {},
            })
        };
        json!({
            "lift": -0.087,
            "paths": {
                "he_u1_skate01": [path_json("s_0", 0., Some((2, 128, 1, 222))), path_json("s_1", 3., Some((2, 96, 2, 0))), path_json("s_2", 6., Some((2, 96, 2, 356)))],
                "he_u2_follow01": [path_json("f_0", 0., None)],
            },
            "rounds": {"he_u1_skate01": [["s_0", "s_missing", "s_2"], ["s_1"]]},
            "riders": {"he_u1_skate01": ["shingo_skate"], "he_u2_follow01": ["shingo_follow"]},
            "profiles": {"shingo_skate": profile(35, [57., 10., 0., 10.], 0.8), "shingo_follow": profile(30, [100., 75., 0., 75.], 1.)},
            "tiers": {"ollie": 0, "kickflip": 0, "360flip": 2},
        })
        .to_string()
    }

    fn file() -> File {
        serde_json::from_str(&ai_json()).unwrap()
    }

    /// old_ai1.py output: one challenge's paths with their ext records (type, yaw) and RoundPaths; tiers by index.
    #[test]
    fn ai_json_loads_one_challenge() {
        let p = file().challenge("he_u1_skate01").unwrap();
        assert_eq!(p.paths.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["s_0", "s_1", "s_2"]);
        assert_eq!((p.ext[0][0].trick, p.ext[0][0].kind, p.ext[0][0].yaw), (128, 1, 222));
        assert_eq!(p.paths[0].tricks[0].node, 2);
        assert_eq!((p.tier(96), p.tier(85), p.tier(7)), (0, 2, 0), "kickflip 0, 360flip 2, no row: 0 (R18)");
        assert_eq!(file().profiles["shingo_skate"].abd, 35);
        assert!(file().challenge("he_u3_film01").is_err());
    }

    /// Review Focus 5 / research 3.3: RoundPaths names O_2, which the disc lacks: a name with no path matches nothing.
    #[test]
    fn a_round_naming_a_missing_path_skips_it() {
        let p = file().challenge("he_u1_skate01").unwrap();
        let mut f = Filters::default();
        f.round(&p, 0);
        let mut s = Setup::default();
        f.apply(&mut s);
        assert_eq!(s.filter, Filter::List(vec![0, 2]));
        f.round(&p, 9);
        f.apply(&mut s);
        assert_eq!(s.filter, Filter::List(vec![]), "a round past the table: nothing passes");
    }

    /// Review Focus 5: a path whose arrays disagree in length, or a trick start past its path, is refused, not
    /// indexed out of bounds later.
    #[test]
    fn a_malformed_path_is_refused() {
        let mut bad = path_json("b", 0., None);
        bad["ticks"] = json!([0, 10]);
        let mut f = file();
        f.paths.insert("x".into(), vec![bad]);
        assert!(f.challenge("x").is_err());
        let mut past = path_json("p", 0., Some((2, 96, 2, 0)));
        past["tricks"][0]["node"] = json!(9);
        f.paths.insert("y".into(), vec![past]);
        assert!(f.challenge("y").is_err());
    }

    /// research 3.9 "Filters" / "Modes": StartPath takes the PF+4 key (R1: any), then with the list on (after
    /// FilterPaths) the round list and mode 0; FilterReset turns the list off and the step keeps its mode (1).
    #[test]
    fn start_path_takes_the_round_list_in_mode_0() {
        let p = file().challenge("he_u1_skate01").unwrap();
        let mut f = Filters::default();
        let mut s = Setup::default();
        f.apply(&mut s);
        assert_eq!((s.mode, s.branching, &s.filter), (1, true, &Filter::Any), "R2: list off at boot");
        f.round(&p, 1);
        f.apply(&mut s);
        assert_eq!((s.mode, &s.filter), (0, &Filter::List(vec![1])));
        f.reset();
        let mut s = Setup::default();
        f.apply(&mut s);
        assert_eq!((s.mode, &s.filter), (1, &Filter::Any));
        assert!(Filter::Only(2).pass(2) && !Filter::Only(2).pass(1) && Filter::Any.pass(7));
    }

    /// The shipped data (research 3.3): 29 / 18 paths, four rounds, profiles 35 % ABD; land 57 / 100 % at tier 0.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn the_exported_paths_load() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let f: File = serde_json::from_slice(&std::fs::read(root.join(FOLDER).join("ai.json")).unwrap()).unwrap();
        let skate = f.challenge("he_u1_skate01").unwrap();
        assert_eq!((skate.paths.len(), skate.rounds.len()), (29, 4));
        assert_eq!(f.challenge("he_u2_follow01").unwrap().paths.len(), 18);
        assert_eq!((f.profiles["shingo_skate"].abd, f.profiles["shingo_skate"].land[0]), (35, 57.));
        assert_eq!(f.profiles["shingo_follow"].land[0], 100.);
    }

    /// Skate 2's test path: `n` nodes 10 frames apart at `speed` m/s along `dir` from `from`, board facing `dir`.
    fn line(name: &str, n: usize, from: Vec3, dir: Vec3, speed: f32) -> Path {
        let q = Quat::from_rotation_arc(Vec3::Z, dir.normalize());
        let step = dir.normalize() * speed / 60.;
        Path {
            name: name.into(),
            pos: (0..n).map(|i| (from + step * 10. * i as f32).to_array()).collect(),
            step: vec![step.to_array(); n],
            ticks: (0..n).map(|i| if i == 0 { 0 } else { 10 }).collect(),
            state: vec![0; n],
            board: vec![q.to_array(); n],
            skater: vec![q.to_array(); n],
            widths: vec![[50, 50]; n],
            flags: vec![0; n],
            h44: 0,
            branches: Vec::new(),
            tricks: Vec::new(),
        }
    }
    fn paths(list: Vec<Path>) -> Paths {
        let n = list.len();
        Paths { paths: list, ext: vec![Vec::new(); n], rounds: Vec::new(), tiers: BTreeMap::new() }
    }
    /// A trick start on node `node` of path `i`.
    fn put(p: &mut Paths, i: usize, node: usize, trick: i32, kind: i32, yaw: i32) {
        p.paths[i].state[node] = 1;
        p.paths[i].tricks.push(TrickNode { node, trick });
        p.ext[i].push(Ext { node, trick, kind, yaw });
    }
    fn body(position: Vec3, forward: Vec3, velocity: Vec3) -> Body {
        Body { position, velocity, forward, facing: forward, up: Vec3::Y, airborne: false, wiping_out: false, trick: None }
    }

    /// research 3.9 "Start pick": paths starting within 4 m, scored rnd (0..399) + tenths of a degree to node 1, lowest
    /// wins; 50 deg off or an avoided trick rejects; all rejected: the first; none within 4 m: the nearest start.
    #[test]
    fn the_start_pick_takes_the_lowest_score_within_4_m() {
        let mut p = paths(vec![
            line("a", 5, Vec3::Z, Vec3::Z, 6.),
            line("b", 5, Vec3::X * 0.5, Vec3::Z, 6.),
            line("c", 5, -Vec3::Z, -Vec3::Z, 6.),
        ]);
        put(&mut p, 0, 2, 96, 2, 0);
        let s = Setup::default();
        let zero = || 0u32;
        assert_eq!((rnd(0), rnd(99_999), rnd(100_000)), (0, 399, 0));
        assert_eq!(pick_start(&p, &s, Vec3::ZERO, Vec3::Z, zero), Some(0), "0 deg beats 26.6 deg (265)");
        let mut r = [99_999u32, 0, 0].into_iter();
        assert_eq!(pick_start(&p, &s, Vec3::ZERO, Vec3::Z, || r.next().unwrap()), Some(1), "399 loses to 265");
        let avoid = Setup { avoid: vec![96], ..Setup::default() };
        assert_eq!(pick_start(&p, &avoid, Vec3::ZERO, Vec3::Z, zero), Some(1), "a's kickflip is used");
        assert_eq!(pick_start(&p, &s, Vec3::ZERO, -Vec3::X, zero), Some(0), "all 50 deg off: the first");
        let far = Vec3::new(0., 0., 10.);
        assert_eq!(pick_start(&p, &s, far, Vec3::Z, zero), Some(0), "none within 4 m: the nearest start");
        let only_c = Setup { filter: Filter::Only(2), ..Setup::default() };
        assert_eq!(pick_start(&p, &only_c, far, Vec3::Z, zero), Some(2));
        let none = Setup { filter: Filter::List(vec![]), ..Setup::default() };
        assert_eq!(pick_start(&p, &none, far, Vec3::Z, zero), Some(0), "R3: nothing passes: any path");
    }

    /// research 3.9 "Branching": stay against each link the filter passes, scored as the start pick; a switch joins
    /// the new path at its node nearest the skater among n-3..n.
    #[test]
    fn a_branch_scores_stay_against_its_links_through_the_filter() {
        let mut a = line("a", 10, Vec3::ZERO, Vec3::Z, 6.);
        let link = |path: &str, node| Link { path: path.into(), node };
        a.branches = vec![Branch { node: 3, to: vec![link("b", 5), link("c", 2), link("gone", 0)] }];
        let p = paths(vec![a, line("b", 10, Vec3::new(0.3, 0., -2.), Vec3::Z, 6.), line("c", 10, Vec3::Z, -Vec3::Z, 6.)]);
        let here = body(Vec3::new(0., 0., 3.), Vec3::Z, Vec3::Z * 6.);
        let s = Setup::default();
        assert_eq!(branch(&p, 0, 2, &s, &here, || 0), None, "no branch row on node 2");
        let mut r = [99_999u32, 0, 0].into_iter();
        assert_eq!(branch(&p, 0, 3, &s, &here, || r.next().unwrap()), Some((1, 5)), "stay 399 vs b 166 (16.7 deg): b at z = 3");
        assert_eq!(branch(&p, 0, 3, &s, &here, || 0), None, "straight on (0) beats b (166); c heads back");
        let only_a = Setup { filter: Filter::Only(0), ..Setup::default() };
        let mut r = [99_999u32, 0, 0].into_iter();
        assert_eq!(branch(&p, 0, 3, &only_a, &here, || r.next().unwrap()), None, "b fails the filter");
    }

    /// research 3.9 "Trick dispatch": by the node's type and the step's mode. bs_50_50 = 7 (type 5), bsgrab = 54
    /// (4), heelflip 92 / kickflip 96 (2), ollie 128 (1); 140 is past the 138-row table.
    #[test]
    fn a_trick_start_takes_its_trick_by_type_and_mode() {
        let mut none = None;
        assert_eq!(dispatch(2, 96, 0, &mut none, true, Some(92)), Some(96), "mode 0: the recorded trick");
        assert_eq!(dispatch(2, 96, 1, &mut none, true, Some(92)), Some(92), "mode 1: a due re-roll");
        assert_eq!(dispatch(1, 128, 1, &mut none, true, None), Some(128));
        assert_eq!(dispatch(5, 7, 1, &mut none, true, Some(92)), Some(7), "only ollies and flips re-roll");
        assert_eq!(dispatch(8, 3, 1, &mut none, true, None), Some(3));
        assert_eq!(dispatch(4, 54, 0, &mut none, false, None), None, "grabs need 6DBA");
        assert_eq!(dispatch(4, 54, 0, &mut none, true, None), Some(54));
        for kind in [3, 6, 7, 11] {
            assert_eq!(dispatch(kind, 4, 0, &mut none, true, None), None, "no handler for {kind}");
        }
        assert_eq!(dispatch(2, 96, 3, &mut none, true, None), None, "mode 3+");
        assert_eq!(dispatch(2, 140, 0, &mut none, true, None), None, "index > 138");
        let mut pattern = Some(92);
        assert_eq!(dispatch(5, 7, 2, &mut pattern, true, None), None, "R9: a heelflip doesn't suit a grind");
        assert_eq!(pattern, Some(92));
        assert_eq!(dispatch(1, 128, 2, &mut pattern, true, None), Some(92), "R9: ollie and flip are one group");
        assert_eq!(dispatch(2, 96, 2, &mut pattern, true, None), None, "the one-element pattern is spent");
    }

    /// research 3.9 "Mode 1 flip re-roll": kept unless the next event within 600 ticks (R15) is its end more than 50
    /// on, or a start whose link (table +4) is this trick.
    #[test]
    fn mode_1_rerolls_before_a_long_trick_end_or_a_linked_start() {
        let mut p = line("a", 12, Vec3::ZERO, Vec3::Z, 6.);
        p.state[1] = 1;
        p.state[7] = 2;
        assert!(reroll_due(&p, 1, 92), "its end 60 ticks on");
        (p.state[7], p.state[5]) = (0, 2);
        assert!(!reroll_due(&p, 1, 92), "its end 40 ticks on");
        p.state[5] = 1;
        p.tricks.push(TrickNode { node: 5, trick: 93 });
        assert!(reroll_due(&p, 1, 92), "heelflip2 links to heelflip");
        p.state[5] = 0;
        assert!(!reroll_due(&p, 1, 92), "nothing ahead: kept");
    }

    /// 0x8223e6c0 / 0x8223e450: the regular list when r1 x (ollie + nollie) < ollie, then r2 against the normalised
    /// running sum; the first entry is the fallback.
    #[test]
    fn the_flip_reroll_draws_by_share_then_weight() {
        let p = Profile1 { ollie: 9., nollie: 1., regular: vec![(85, 0.), (92, 5.), (96, 5.)], nollie_flips: vec![(113, 1.)], ..Profile1::default() };
        assert_eq!(reroll(&p, 0.89, 0.), Some(92));
        assert_eq!(reroll(&p, 0.9, 0.), Some(113));
        assert_eq!((reroll(&p, 0., 0.49), reroll(&p, 0., 0.5), reroll(&p, 0., 1.)), (Some(92), Some(96), Some(85)));
        assert_eq!(reroll(&Profile1::default(), 0., 0.), None, "an empty list keeps the recorded trick");
    }

    /// research 3.2 "Landing roll": bail when r x 100 > land[tier] x decay^n.
    #[test]
    fn the_landing_roll_uses_tier_and_decay() {
        let skate = Profile1 { land: [57., 10., 0., 10.], decay: 0.8, ..Profile1::default() };
        assert!(!bails(&skate, 0, 0, 0.56) && bails(&skate, 0, 0, 0.58));
        assert!(bails(&skate, 0, 1, 0.5), "57 x 0.8 = 45.6");
        assert!(bails(&skate, 2, 0, 0.01), "0602 = 0: a tier-2 trick always bails");
        assert!(!bails(&skate, 3, 0, 0.09) && bails(&skate, 3, 0, 0.11));
        let follow = Profile1 { land: [100., 75., 0., 75.], decay: 1., ..Profile1::default() };
        assert!(!bails(&follow, 0, 3, 1.), "100 % at tier 0");
    }

    /// 0x82238250: (int)((|yaw| + tol) / 180), R7 tol = 90.
    #[test]
    fn spin_halves_round_to_the_nearest_180() {
        assert_eq!([222., 356., -13., 89., 90.].map(halves), [1, 2, 0, 0, 1]);
    }

    /// research 3.4 FollowTrickPattern: paths with a start of the player's trick (R8: exact index) whose FIRST start
    /// has the player's 180 count.
    #[test]
    fn follow_patterns_match_trick_and_first_spin() {
        let mut p = paths(vec![line("a", 6, Vec3::ZERO, Vec3::Z, 6.), line("b", 6, Vec3::X, Vec3::Z, 6.), line("c", 6, Vec3::X * 2., Vec3::Z, 6.)]);
        put(&mut p, 0, 2, 128, 1, 222);
        put(&mut p, 1, 2, 96, 2, 0);
        put(&mut p, 1, 4, 128, 1, 356);
        put(&mut p, 2, 1, 128, 1, 356);
        assert_eq!(pattern_paths(&p, 128, 1), [0]);
        assert_eq!(pattern_paths(&p, 128, 0), [1], "b's first start (the kickflip) has no spin");
        assert_eq!(pattern_paths(&p, 128, 2), [2]);
        assert!(pattern_paths(&p, 85, 0).is_empty(), "Review Focus 1: no 360 flip recorded");
    }

    /// The shipped S.K.A.T.E. paths (checked in Python on P1's ai.json): ollie 1 -> _10 (yaw 222), ollie 2 -> _11
    /// (356), kickflip 0 -> nine paths, kickflip 1 -> none.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn the_exported_paths_match_follow_patterns() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let f: File = serde_json::from_slice(&std::fs::read(root.join(FOLDER).join("ai.json")).unwrap()).unwrap();
        let p = f.challenge("he_u1_skate01").unwrap();
        let names = |v: Vec<usize>| v.into_iter().map(|i| p.paths[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(pattern_paths(&p, 128, 1)), ["he_u1_skate01_10"]);
        assert_eq!(names(pattern_paths(&p, 128, 2)), ["he_u1_skate01_11"]);
        assert_eq!(pattern_paths(&p, 96, 0).len(), 9);
        assert!(pattern_paths(&p, 96, 1).is_empty());
    }

    fn value(step: &Step, name: &str) -> Option<f32> {
        step.intents.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
    fn go(d: &mut Driver1, p: &Paths, prof: &Profile1, b: Body) -> Step {
        d.step(p, prof, b, None, &mut Rng::default(), &mut Vec::new())
    }

    /// research 3.5 table: the same ground turn (18 deg to full), push (below 0.95 of the path's speed) and brake
    /// (0.3 of the speed past it, node >= 1) as Skate 2.
    #[test]
    fn a_driver_turns_pushes_and_brakes_as_skate_2_does() {
        let p = paths(vec![line("a", 20, Vec3::ZERO, Vec3::Z, 6.)]);
        let prof = Profile1::default();
        let fresh = || Driver1::new(0, Setup::default(), false);
        let off = Quat::from_rotation_y(9f32.to_radians()) * Vec3::Z;
        assert!((value(&go(&mut fresh(), &p, &prof, body(Vec3::ZERO, off, Vec3::ZERO)), "Turn").unwrap() - 0.5).abs() < 1e-3);
        let at = Vec3::new(0., 0., 5.);
        let push = go(&mut fresh(), &p, &prof, body(at, Vec3::Z, Vec3::ZERO));
        assert_eq!((value(&push, "LeftPush"), value(&push, "NewPush"), value(&push, "Pushing")), (Some(1.), Some(1.), Some(1.)));
        assert_eq!(value(&go(&mut fresh(), &p, &prof, body(at, Vec3::Z, Vec3::Z * 25.)), "Brake"), Some(1.));
        let coast = go(&mut fresh(), &p, &prof, body(at, Vec3::Z, Vec3::Z * 7.));
        assert_eq!((value(&coast, "Brake"), value(&coast, "LeftPush")), (None, None));
    }

    /// research 3.5 table 3: Skate 1's rubber-band weight DCD5 is 0.3 (Skate 2: 0.25). With the player 15 m ahead the
    /// target is 7.8 m/s, so at 7.2 he pushes (Skate 2 would coast). SwitchOffFollowMeRubberBanding drops it.
    #[test]
    fn the_follow_band_lifts_the_target_by_dcd5() {
        let p = paths(vec![line("a", 40, Vec3::ZERO, Vec3::Z, 6.)]);
        let at = Vec3::new(0., 0., 5.);
        let player = Player { position: at + Vec3::Z * 15., wiping_out: false };
        let ride = |band: bool| {
            let mut d = Driver1::new(0, Setup::default(), band);
            d.step(&p, &Profile1::default(), body(at, Vec3::Z, Vec3::Z * 7.2), Some(&player), &mut Rng::default(), &mut Vec::new())
        };
        assert_eq!(value(&ride(true), "Pushing"), Some(1.), "7.2 < 0.95 x 7.8");
        assert_eq!(value(&ride(false), "Pushing"), None);
    }

    /// research 3.9: mode 0 pops the recorded kickflip on passing its node, with Skate 2's trick signals (R10);
    /// Antic leads it within 60 frames (R12).
    #[test]
    fn a_trick_node_pops_its_flip_and_antic_leads_it() {
        let mut p = paths(vec![line("a", 20, Vec3::ZERO, Vec3::Z, 6.)]);
        put(&mut p, 0, 6, 96, 2, 0);
        let prof = Profile1::default();
        let mut d = Driver1::new(0, Setup { mode: 0, ..Setup::default() }, false);
        let before = go(&mut d, &p, &prof, body(Vec3::new(0., 0., 5.5), Vec3::Z, Vec3::Z * 6.));
        assert_eq!((value(&before, "AnticMag"), value(&before, "AnticAngle"), value(&before, "Trick")), (Some(1.), Some(0.), None));
        let pop = go(&mut d, &p, &prof, body(Vec3::new(0., 0., 6.05), Vec3::Z, Vec3::Z * 6.));
        for name in ["Trick", "kickflip", "GestureSpeed", "TrickHeight", "DontMirrorTrick"] {
            assert_eq!(value(&pop, name), Some(1.), "{name}");
        }
        assert_eq!(value(&go(&mut d, &p, &prof, body(Vec3::new(0., 0., 6.2), Vec3::Z, Vec3::Z * 6.)), "Trick"), None, "once");
    }

    /// research 3.5 item 4: a locked powerslide writes the side its turn points (turn > 0, i.e. e < 0: Right, 0.4);
    /// research 3.9: a manual balances at +24 742E = 0.5, negated for tailmanual (3).
    #[test]
    fn a_locked_powerslide_slides_toward_its_turn_and_a_tailmanual_balances_back() {
        let mut p = paths(vec![line("a", 20, Vec3::ZERO, Vec3::Z, 6.)]);
        put(&mut p, 0, 3, 0, 9, 0);
        put(&mut p, 0, 10, 3, 8, 0);
        let prof = Profile1::default();
        let mut d = Driver1::new(0, Setup::default(), false);
        let slide = go(&mut d, &p, &prof, body(Vec3::new(0., 0., 3.05), Quat::from_rotation_y(0.2) * Vec3::Z, Vec3::Z * 6.));
        assert_eq!((value(&slide, "RightSlideStart"), value(&slide, "RightSlide")), (Some(1.), Some(0.4)));
        let manual = go(&mut d, &p, &prof, body(Vec3::new(0., 0., 10.05), Vec3::Z, Vec3::Z * 6.));
        assert_eq!(value(&manual, "Manual"), Some(-0.5));
    }

    /// research 3.2 landing roll: landing a popped flip rolls the bail with n = the 180s turned in the air (R16):
    /// 200 deg -> 1. A bail writes WipeOutRequest and is posted as the wipeout, before the landing; the landing goes to
    /// the lead with its trick (R30) and spin count.
    #[test]
    fn a_landing_rolls_its_bail_and_reports_the_trick() {
        let mut p = paths(vec![line("a", 20, Vec3::ZERO, Vec3::Z, 6.)]);
        put(&mut p, 0, 3, 96, 2, 0);
        let fly = |land: f32| {
            let prof = Profile1 { land: [land; 4], decay: 1., ..Profile1::default() };
            let mut d = Driver1::new(0, Setup { mode: 0, ..Setup::default() }, false);
            let (mut rng, mut events) = (Rng::default(), Vec::new());
            let mut b = body(Vec3::new(0., 0., 3.05), Vec3::Z, Vec3::Z * 6.);
            d.step(&p, &prof, b, None, &mut rng, &mut events);
            b.airborne = true;
            for i in 1..=10 {
                b.facing = Quat::from_rotation_y((20. * i as f32).to_radians()) * Vec3::Z;
                d.step(&p, &prof, b, None, &mut rng, &mut events);
            }
            b.airborne = false;
            let s = d.step(&p, &prof, b, None, &mut rng, &mut events);
            // The fall the bail starts was posted with it.
            b.wiping_out = land == 0.;
            d.step(&p, &prof, b, None, &mut rng, &mut events);
            (value(&s, "WipeOutRequest"), events)
        };
        let (bail, events) = fly(0.);
        assert_eq!(bail, Some(1.));
        assert_eq!(events, [AiEvent::Wipeout, AiEvent::Landed { trick: 96, spins: 1, switched: false, bail: true }]);
        let (bail, events) = fly(100.);
        assert_eq!((bail, events), (None, vec![AiEvent::Landed { trick: 96, spins: 1, switched: false, bail: false }]));
    }

    /// Review Focus 4 / R13: down for over 2 s, he goes back on his path at his node, and the wipeout is reported
    /// once; a rider merely off his path is never pulled back (Skate 1 has no corridor watchdog, research5 1).
    #[test]
    fn a_wiped_rider_resets_after_two_seconds_and_an_off_path_one_never_does() {
        let p = paths(vec![line("a", 20, Vec3::ZERO, Vec3::Z, 6.)]);
        let prof = Profile1::default();
        let mut d = Driver1::new(0, Setup::default(), false);
        let (mut rng, mut events) = (Rng::default(), Vec::new());
        d.step(&p, &prof, body(Vec3::new(0., 0., 4.), Vec3::Z, Vec3::Z * 6.), None, &mut rng, &mut events);
        let lost = body(Vec3::new(20., 0., 4.), Vec3::Z, Vec3::ZERO);
        assert!((0..600).all(|_| d.step(&p, &prof, lost, None, &mut rng, &mut events).reposition.is_none()));
        let mut down = body(Vec3::new(0.5, 0., 4.), Vec3::Z, Vec3::ZERO);
        down.wiping_out = true;
        let back: Vec<_> = (0..121).filter_map(|_| d.step(&p, &prof, down, None, &mut rng, &mut events).reposition).collect();
        assert_eq!(back.len(), 1, "on frame 121");
        assert!(Vec3::from_slice(&back[0][3][..3]).distance(Vec3::from_array(p.paths[0].pos[4])) < 1e-4);
        assert_eq!(events, [AiEvent::Wipeout]);
    }

    /// UNTRACED R14 (Skate 2's rule, Skate 1's 4 m start radius): at a recording's end he goes on along the nearest
    /// path starting within 4 m that his filter passes.
    #[test]
    fn a_recording_end_chains_onto_a_filtered_path() {
        let p = paths(vec![line("a", 5, Vec3::ZERO, Vec3::Z, 6.), line("b", 5, Vec3::Z * 5., Vec3::Z, 6.), line("c", 5, Vec3::Z * 4.5, Vec3::Z, 6.)]);
        let end = body(Vec3::new(0., 0., 4.), Vec3::Z, Vec3::Z * 6.);
        let mut d = Driver1::new(0, Setup::default(), false);
        go(&mut d, &p, &Profile1::default(), end);
        assert_eq!(d.path, 2, "c starts 0.5 m from a's end");
        let mut d = Driver1::new(0, Setup { filter: Filter::List(vec![0, 1]), ..Setup::default() }, false);
        go(&mut d, &p, &Profile1::default(), end);
        assert_eq!(d.path, 1, "c is not in the round");
    }

    fn m(x: f32) -> M4 {
        [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]]
    }
    fn loaded() -> Skate1Ai {
        let mut ai = Skate1Ai::default();
        ai.parse(&ai_json()).unwrap();
        ai
    }

    /// Playtest 10-15: P-Rod stood on his first node all lesson. From a standstill at node 0 of the shipped nollie
    /// path 0 (as StartPath places him), he pushes off.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn prod_pushes_off_his_first_node() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let f: File = serde_json::from_slice(&std::fs::read(root.join(FOLDER).join("ai.json")).unwrap()).unwrap();
        let p = f.challenge("fp_u3_tutorial02").unwrap();
        let prof = f.profiles["paul_rodriguez"].clone();
        let at = place(&p.paths[0], 0);
        let b = body(Vec3::from_slice(&at[3][..3]), Vec3::from_slice(&at[2][..3]), Vec3::ZERO);
        let mut d = Driver1::new(0, Setup::default(), true);
        let player = Player { position: b.position - b.forward * 3., wiping_out: false };
        let step = d.step(&p, &prof, b, Some(&player), &mut Rng::default(), &mut Vec::new());
        assert_eq!(value(&step, "Pushing"), Some(1.), "{:?}", step.intents);
    }

    /// User 10-15 run12: P-Rod turned back up his path or stood off it. Driven by his own controller on his own
    /// physics on San Vanelona from the shipped nollie path 0, he rides on past path 0's shove-it (node 21), path 1's
    /// (node 14) and path 1's nollie (node 40, rolled to a flip: its deck turned over, the air turn spun him round
    /// about the deck's up and he landed facing back) to path 1's next shove-it (node 61).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (an installation's assets, its maps/Skate_1_San_Vanelona.skate beside them)"]
    fn prod_rides_on_past_his_shove_its() {
        use crate::physics::{GamePhysics, riders::advance_all};
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let f: File = serde_json::from_slice(&std::fs::read(root.join(FOLDER).join("ai.json")).unwrap()).unwrap();
        let p = f.challenge("fp_u3_tutorial02").unwrap();
        let prof = f.profiles["paul_rodriguez"].clone();
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        // His paths are San Vanelona's: on the flat test floor he drops off them.
        let map = root.parent().unwrap().join("maps/Skate_1_San_Vanelona.skate");
        let map = skate_data::skate_map::SkateMap::load(&map).unwrap();
        let mut physics = GamePhysics::load_with_map(&root, Some(&map)).unwrap();
        let player = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
        let mut riders = Riders::default();
        riders.spawn("Paul_rodriguez#s1ai1", place(&p.paths[0], 0));
        let (mut d, mut rng, mut furthest) = (Driver1::new(0, Setup::default(), false), Rng::default(), (0, 0));
        for i in 0..40 * 60 {
            advance_all(&mut riders, &mut physics, &player, &graphs, &root, None);
            let rider = riders.get_mut("Paul_rodriguez#s1ai1").expect("spawned");
            assert!(!rider.failed());
            let step = d.step(&p, &prof, rider.body(), None, &mut rng, &mut Vec::new());
            rider.intents = step.intents;
            if let Some(at) = step.reposition {
                rider.travel_to(at);
            }
            furthest = furthest.max((d.path, d.at.node));
            if i % 60 == 0 {
                let b = riders.get("Paul_rodriguez#s1ai1").unwrap().body();
                eprintln!("{}s path {} node {} at {:.1?} v {:.1} fwd {:.2?} wipe {}", i / 60, d.path, d.at.node,
                    b.position, b.velocity.length(), b.forward, b.wiping_out);
            }
        }
        assert!(furthest >= (1, 61), "P-Rod got only to path {} node {}", furthest.0, furthest.1);
    }

    /// User 10-15 run15: in S.K.A.T.E. Shingo copied the player's heelflip on he_u1_skate01_7 and never set off (placed
    /// at its node 0, no air for the attempt's whole timer). On San Vanelona, set up as FollowTrickPattern's match
    /// sets him (mode 2 with the path's first trick, that path only, no branching) at each path's node 0 as
    /// travel_to places him, he reaches the path's first trick start within 4 s (the 8 recordings opening on a
    /// reversed board took 5-10 s, heading off the wrong way first, or never got there).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (an installation's assets, its maps/Skate_1_San_Vanelona.skate beside them)"]
    fn shingo_sets_off_on_every_skate_path() {
        use crate::physics::riders::advance_all;
        let (root, f, graphs, mut physics, player) = on_san_vanelona();
        let p = f.challenge("he_u1_skate01").unwrap();
        let prof = f.profiles[&f.riders["he_u1_skate01"][0]].clone();
        let mut riders = Riders::default();
        let mut stuck = Vec::new();
        for i in 0..p.paths.len() {
            let first = p.ext[i].iter().filter(|x| p.paths[i].state[x.node] == 1).min_by_key(|x| x.node).unwrap();
            let setup = if matches!(first.kind, 1 | 2) {
                Setup { mode: 2, branching: false, filter: Filter::Only(i), avoid: Vec::new(), pattern: Some(first.trick) }
            } else {
                Setup::default()
            };
            let key = format!("IP_shingo#s1ai{}", i + 1);
            riders.spawn(&key, place(&p.paths[i], 0));
            let (mut d, mut rng, mut events) = (Driver1::new(i, setup, false), Rng::default(), Vec::new());
            let mut reached = None;
            for t in 0..15 * 60 {
                advance_all(&mut riders, &mut physics, &player, &graphs, &root, None);
                let rider = riders.get_mut(&key).expect("spawned");
                assert!(!rider.failed());
                let step = d.step(&p, &prof, rider.body(), None, &mut rng, &mut events);
                rider.intents = step.intents;
                if let Some(at) = step.reposition {
                    rider.travel_to(at);
                }
                if d.path != i || d.at.node >= first.node {
                    reached = Some(t);
                    break;
                }
            }
            let b = riders.get(&key).unwrap().body();
            eprintln!("path {i:2} {:22} trick {} at node {}: {} (node {} v {:.1} wipe {})", p.paths[i].name,
                IDENTIFIERS[first.trick as usize].0, first.node, reached.map_or("STUCK".into(), |t| format!("{t} ticks")),
                d.at.node, b.velocity.length(), b.wiping_out);
            if reached.is_none_or(|t| t > 4 * 60) {
                stuck.push(p.paths[i].name.clone());
            }
            riders.remove(&key);
        }
        assert!(stuck.is_empty(), "Shingo didn't reach his trick within 4 s on {stuck:?}");
    }

    /// The Skate 1 AI's world: San Vanelona's physics (his paths are its; on the flat test floor he drops off them).
    fn on_san_vanelona() -> (std::path::PathBuf, File, crate::graph_runtime::StockGraphs, crate::physics::GamePhysics, SkaterRuntime) {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let f: File = serde_json::from_slice(&std::fs::read(root.join(FOLDER).join("ai.json")).unwrap()).unwrap();
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        let map = root.parent().unwrap().join("maps/Skate_1_San_Vanelona.skate");
        let map = skate_data::skate_map::SkateMap::load(&map).unwrap();
        let physics = crate::physics::GamePhysics::load_with_map(&root, Some(&map)).unwrap();
        let player = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
        (root, f, graphs, physics, player)
    }

    /// User 10-15 run15 (he_u2_follow01, "wrong route / off the path"): StartPath put Shingo on A_6's node 0 and he
    /// stopped for good at (-402.1, -2006.1), past the end of the first recordings (A_3 / A_4 / A_6 / A_8, which loop
    /// west to x -455 and back east). From A_6's node 0, as StartPath sets him (mode 1, branching), he sets off along
    /// it at once (placed on its reversed board he stood facing back for 3 s) and rides on past the second group (A_0,
    /// A_15, A_18, A_19, which end ~110 m from the start).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (an installation's assets, its maps/Skate_1_San_Vanelona.skate beside them)"]
    fn shingo_leads_the_follow_route() {
        use crate::physics::riders::advance_all;
        let (root, f, graphs, mut physics, player) = on_san_vanelona();
        let p = f.challenge("he_u2_follow01").unwrap();
        let prof = f.profiles[&f.riders["he_u2_follow01"][0]].clone();
        let i = p.paths.iter().position(|x| x.name == "he_u2_followleader01_A_6").unwrap();
        let start = Vec3::from_array(p.paths[i].pos[0]);
        let mut riders = Riders::default();
        riders.spawn("IP_shingo#s1ai1", place(&p.paths[i], 0));
        let (mut d, mut rng, mut far, mut off) = (Driver1::new(i, Setup::default(), true), Rng::default(), 0f32, 0f32);
        for t in 0..60 * 60 {
            advance_all(&mut riders, &mut physics, &player, &graphs, &root, None);
            let rider = riders.get_mut("IP_shingo#s1ai1").expect("spawned");
            assert!(!rider.failed());
            let step = d.step(&p, &prof, rider.body(), None, &mut rng, &mut Vec::new());
            rider.intents = step.intents;
            if let Some(at) = step.reposition {
                rider.travel_to(at);
            }
            let b = riders.get("IP_shingo#s1ai1").unwrap().body();
            far = far.max(b.position.distance(start));
            if t == 2 * 60 {
                off = b.position.distance(start);
            }
            if t % 60 == 0 {
                eprintln!("{}s {} node {} at {:.1?} v {:.1} wipe {}", t / 60, p.paths[d.path].name, d.at.node, b.position,
                    b.velocity.length(), b.wiping_out);
            }
        }
        assert!(off > 2., "after 2 s he is {off:.1} m from A_6's start");
        assert!(far > 90., "he got only {far:.0} m from the start");
    }

    /// P6 R3 (INFERRED): P-Rod's rider draws Paul_rodriguez; Shingo's riders keep IP_shingo.
    #[test]
    fn prods_rider_draws_his_own_model() {
        let mut json: serde_json::Value = serde_json::from_str(&ai_json()).unwrap();
        json["riders"]["fp_u3_tutorial02"] = serde_json::json!(["paul_rodriguez"]);
        json["profiles"]["paul_rodriguez"] = json["profiles"]["shingo_skate"].clone();
        let mut ai = Skate1Ai::default();
        ai.parse(&json.to_string()).unwrap();
        assert!(ai.select(1, "fp_u3_tutorial02", false) && ai.select(2, "he_u1_skate01", false));
        assert_eq!((ai.key(1), ai.key(2)), (Some("Paul_rodriguez#s1ai1"), Some("IP_shingo#s1ai2")));
        // Run12 (no names by the S.K.A.T.E. letters): RETAIL 0x82319110 names an AI by its character's short name.
        assert_eq!((ai.name(1), ai.name(2), ai.name(3)), (Some("P-ROD"), Some("SHINGO"), None));
    }

    /// User 10-15 (Community Spirit): competitor n + 1 rides Competitors row n, with that row's model and name.
    #[test]
    fn each_competitor_rides_its_own_row() {
        let mut json: serde_json::Value = serde_json::from_str(&ai_json()).unwrap();
        json["riders"]["he_u4_besttrick01"] = serde_json::json!(["eric_unknown", "fabio_unknown", "shingo_unknown"]);
        for rider in ["eric_unknown", "fabio_unknown", "shingo_unknown"] {
            json["profiles"][rider] = json["profiles"]["shingo_skate"].clone();
        }
        let mut ai = Skate1Ai::default();
        ai.parse(&json.to_string()).unwrap();
        assert!((1..=3).all(|id| ai.select(id, "he_u4_besttrick01", false)));
        assert_eq!((ai.key(1), ai.key(2), ai.key(3)), (Some("ip_eric#s1ai1"), Some("ip_fabio#s1ai2"), Some("IP_shingo#s1ai3")));
        assert_eq!((ai.name(1), ai.name(2), ai.name(3)), (Some("ERIC"), Some("FABIO"), Some("SHINGO")));
    }

    /// research 3.4 natives by Lua id: the rider's key, profile, stance, ABD, spawn and the step StartSkatePath builds.
    #[test]
    fn the_natives_drive_one_rider_by_lua_id() {
        let mut ai = loaded();
        assert!(ai.select(2, "he_u1_skate01", false));
        assert_eq!((ai.key(2), ai.abd(2), ai.regular(2)), (Some("IP_shingo#s1ai2"), 35, true));
        ai.set_regular(2, false);
        assert!(!ai.regular(2));
        assert!(ai.riders().is_empty(), "not spawned yet");
        ai.spawn(2, m(1.));
        ai.reposition(2, m(2.));
        assert_eq!(ai.riders(), [("IP_shingo#s1ai2".to_string(), m(1.))], "the cast keeps its spawn matrix");
        assert!(ai.load_paths("he_u1_skate01"));
        ai.filter_reset();
        ai.filter_paths(0);
        ai.start_path(2, false, vec![96]);
        let a = &ai.ais[&2];
        assert_eq!((a.setup.mode, a.setup.branching, a.setup.avoid.clone(), a.start), (0, false, vec![96], true));
        ai.clear_behavior(2);
        assert_eq!((ai.ais[&2].setup.clone(), ai.ais[&2].start), (Setup::default(), false), "R25");
        ai.unspawn(2);
        assert!(ai.key(2).is_none());
    }

    /// Review Focus 1 / research 3.4 FollowTrickPattern: a match rides mode 2, branching off, on that path only, with
    /// the player's trick; no match answers false but still rides (wildcard; the step's own mode) - R29.
    #[test]
    fn an_unmatched_pattern_still_rides() {
        let mut ai = loaded();
        ai.select(1, "he_u1_skate01", false);
        ai.load_paths("he_u1_skate01");
        assert!(!ai.follow_trick_pattern(1, 85, 0));
        let a = &ai.ais[&1];
        assert_eq!((a.setup.mode, a.setup.filter.clone(), a.setup.pattern, a.start), (1, Filter::Any, None, true));
        assert!(ai.follow_trick_pattern(1, 128, 1));
        let a = &ai.ais[&1];
        assert_eq!((a.setup.mode, a.setup.branching, a.setup.filter.clone(), a.setup.pattern), (2, false, Filter::Only(0), Some(128)));
    }

    /// Review Focus 5: with no ai.json, or no paths for a challenge, the natives answer false and change nothing.
    #[test]
    fn natives_without_paths_do_nothing() {
        let mut ai = Skate1Ai::default();
        assert!(!ai.select(1, "he_u1_skate01", false) && !ai.load_paths("he_u1_skate01"));
        ai.spawn(1, m(0.));
        ai.start_path(1, true, Vec::new());
        assert!(!ai.follow_trick_pattern(1, 128, 0));
        assert!(ai.riders().is_empty() && ai.key(1).is_none() && ai.drain().is_empty());
        let mut ai = loaded();
        assert!(!ai.load_paths("he_u3_film01"), "Filming 101 has no recordings");
        assert!(!ai.select(1, "he_u3_film01", false), "nor a rider");
    }

    /// Review Focus 2: the fixed tick keeps every rider that isn't Skate 1's (Skate 2's `#ai` competitors) and only
    /// the Skate 1 riders whose AI is still selected.
    #[test]
    fn drive_keeps_other_riders() {
        let mut ai = loaded();
        ai.select(1, "he_u1_skate01", false);
        assert!(keeps(&ai, "slappy#ai1") && keeps(&ai, "IP_shingo#s1ai1") && !keeps(&ai, "IP_shingo#s1ai2"));
        ai.clear();
        assert!(!keeps(&ai, "IP_shingo#s1ai1") && keeps(&ai, "slappy#ai1"));
    }
}
