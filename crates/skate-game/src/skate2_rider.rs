//! Skate 2's AI PathController (round2 T5, round2-research4): drives a tutorial rider (physics/riders.rs) along the
//! lesson's recorded paths (old_tutorial.py `ai_segments`, aipaths.big) by writing the ActionGraph signals retail's
//! controller writes: Turn/BodySpin/KickTurn toward the path, Push below the recording's speed, Brake far above it,
//! Crouch where the recording crouched, branch choice by retail's score, and the off-path reposition; and (round2 T5
//! tricks) the trick half: the lookahead (0x823f6688), node events (0x823f77a0) and each category's signals
//! (0x823f6bf0), Antic, air BoardAdjust, the push gate's trickSoon and the branch block. The recordings' trick
//! indices are Skate 2's table 0x820614c8, row for row Skate 3's (skate_core IDENTIFIERS[..269]).
//! Not ported: the ObstacleAvoider, off-board motion, the WipeOutRequest watchdogs (+584 > 300, +588 > 150) and
//! landing bail roll (0x823ef058), landing BoardAdjust (+634/+635), path mode 2 (0x823f72a8).
pub(crate) use crate::physics::riders::Body;
use bevy::prelude::*;
use serde::Deserialize;
use skate_core::{animation::skeleton_input::name::encode, scoring::catalog::IDENTIFIERS};

/// One recorded segment (tools/skate3_ai_paths.py decode, per node).
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Path {
    pub name: String,
    pub pos: Vec<[f32; 3]>,
    /// Displacement per tick (x 60 = m/s).
    pub step: Vec<[f32; 3]>,
    /// Ticks since the previous node.
    pub ticks: Vec<u32>,
    /// Node event (+39): 0 plain, 1 trick start, 2 trick end, 4 inert.
    pub state: Vec<u8>,
    /// Packed orientations, xyzw.
    pub board: Vec<[f32; 4]>,
    pub skater: Vec<[f32; 4]>,
    /// Corridor half-widths in 0.02 m (+97 left, +98 right).
    pub widths: Vec<[u8; 2]>,
    /// 0x01 flipped, 0x02 crouch, 0x04 airborne, 0x08 off-board.
    pub flags: Vec<u8>,
    #[serde(default)]
    pub h44: u32,
    #[serde(default)]
    pub branches: Vec<Branch>,
    /// Trick nodes (ExtendedNodeData +36, Skate 2's trick index): `state` 1 starts it, 2 ends it.
    #[serde(default)]
    pub tricks: Vec<TrickNode>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Branch {
    pub node: usize,
    pub to: Vec<Link>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Link {
    pub path: String,
    pub node: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TrickNode {
    pub node: usize,
    pub trick: i32,
}

/// A trick's scorable row (Skate 2 skater vault class 0x6918469984A8C596; old_tutorial.py `ai_tricks`).
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Trick {
    /// Frames ahead of its node it starts (row +264, Hash_2702723D1A7F0ABF).
    #[serde(default)]
    pub lead: u32,
    /// What 0x823fdcc0 writes: (signal, value, from frame, to frame or -1), signals indexing SIGNALS.
    #[serde(default)]
    pub recipe: Vec<(u8, f32, i32, i32)>,
}

/// An AI profile (class 0xE9407D230E506420; old_tutorial.py `ai_profiles`).
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Profile {
    /// +400 (Hash_6DBAF7AD6CDE6A16): categories 3, 4 and 12 perform their recipe.
    #[serde(default)]
    pub grabs: bool,
    /// The flips mode 1 draws from by weight (Hash_E580B6284639E03F), and for an N... trick (Hash_BB901D68361E9833).
    #[serde(default)]
    pub regular: Vec<(i32, f32)>,
    #[serde(default)]
    pub nollie: Vec<(i32, f32)>,
}

/// The player, as the score and the follow band read it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Player {
    pub position: Vec3,
    pub wiping_out: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Step {
    pub intents: Vec<(&'static str, f32)>,
    /// A game teleport onto the path (RepositionAI rows: right, up, at, position).
    pub reposition: Option<[[f32; 4]; 4]>,
}

/// A point on a path: segment `node` -> `node + 1` at `frac`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    pub node: usize,
    pub frac: f32,
}

pub(crate) struct Driver {
    path: usize,
    at: Sample,
    /// +484, furthest node reached.
    furthest: usize,
    prev: Option<Vec3>,
    /// +584
    off_frames: u32,
    wiped_frames: u32,
    /// The follow lesson's rubber band (mode 23).
    pub band: bool,
    rng: u32,
    /// Skate 2's scorable rows by trick index (None: no row) and the rider's profile, from StartPath.
    pub tricks: Vec<Option<Trick>>,
    pub profile: Profile,
    t: Tricks,
}

/// The trick half's state by AIController offset; reset (0x823f6210) on StartPath, a branch pick and a path's end.
struct Tricks {
    /// +500: the category in progress, 0 none.
    action: i32,
    /// +504: a manual's value (0x823f6828).
    manual: f32,
    /// +508: the last start node taken.
    last_event: i32,
    /// +512 / +516 / +528 / +532: the next start within 60 frames, the frames to it, its category and trick.
    next: i32,
    next_frames: i32,
    next_cat: i32,
    next_trick: i32,
    /// +520 / +524: AnticMag, AnticAngle.
    antic: (f32, f32),
    /// +536: the trick performed, -1 none.
    current: i32,
    /// +612: frames since its start.
    frames: i32,
    /// +620: locked onto the path (0x823fb9b8).
    locked: bool,
    /// +623: a start lies ahead; +624: an ollie or flip.
    upcoming: bool,
    next_flip: bool,
    /// +625: this trick started early; +626: start the next one now (its lead).
    early: bool,
    start_early: bool,
    /// +632: airborne; +608: frames since the last landing.
    air: bool,
    since_land: u32,
    /// +639: a variant popped as its +8 row, held until it scores.
    hold: bool,
    /// +156 / +488: the event scan's node, this frame and last (0x823fbab8).
    scan: usize,
    scan_prev: usize,
}

impl Default for Tricks {
    fn default() -> Self {
        Self {
            action: 0,
            manual: 0.,
            last_event: -1,
            next: -1,
            next_frames: 0,
            next_cat: 0,
            next_trick: -1,
            antic: (0., 0.),
            current: -1,
            frames: 0,
            locked: false,
            upcoming: false,
            next_flip: false,
            early: false,
            start_early: false,
            air: false,
            since_land: 7200,
            hold: false,
            scan: 0,
            scan_prev: 0,
        }
    }
}

const DEG: f32 = std::f32::consts::PI / 180.;

impl Driver {
    pub(crate) fn new(path: usize) -> Self {
        Self {
            path,
            at: Sample { node: 0, frac: 0. },
            furthest: 0,
            prev: None,
            off_frames: 0,
            wiped_frames: 0,
            band: false,
            // ponytail: retail draws from the global RNG it shares with audio (research4: any uniform RNG is faithful).
            rng: 0x9E37_79B9 ^ (path as u32).wrapping_mul(0x85EB_CA6B),
            tricks: Vec::new(),
            profile: Profile::default(),
            t: Tricks::default(),
        }
    }
    pub(crate) fn path(&self) -> usize {
        self.path
    }
    pub(crate) fn node(&self) -> usize {
        self.at.node
    }
    /// The teleport onto node 0 of this driver's path, facing along it (StartPath).
    pub(crate) fn start(&mut self, paths: &[Path]) -> [[f32; 4]; 4] {
        self.at = Sample { node: 0, frac: 0. };
        self.furthest = 0;
        self.prev = None;
        place(&paths[self.path], 0)
    }

    pub(crate) fn step(&mut self, paths: &[Path], body: Body, others: &[(usize, usize)], player: Option<&Player>) -> Step {
        let mut out = Step::default();
        let path = &paths[self.path];
        // Speed (0x823f9210).
        let moved = self.prev.map(|p| body.position - p).filter(|d| d.abs().max_element() > 1.192_092_9e-7);
        let vel = moved.map_or(body.velocity, |d| d * 60.);
        self.prev = Some(body.position);
        let mut speed = vel.length();
        if speed > 0.01 && body.forward.dot(vel) < 0. {
            speed = -speed;
        }
        // Nearest point (0x823e8608 from max(node - 3, 0)). UNTRACED: the search's far end; 12 nodes.
        let old = self.at.node;
        self.at = nearest(path, body.position, old.saturating_sub(3), old + 12);
        self.furthest = self.furthest.max(self.at.node);
        // UNTRACED rule: at a recording's end, the path that starts within 2.5 m of it (follow chains, photo loops).
        if self.at.node + 1 >= path.pos.len() {
            let end = Vec3::from_array(path.pos[path.pos.len() - 1]);
            if let Some(next) = (0..paths.len())
                .map(|i| (i, end.distance(Vec3::from_array(paths[i].pos[0]))))
                .filter(|&(_, d)| d < 2.5)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
            {
                self.path = next;
                self.at = Sample { node: 0, frac: 0. };
                self.furthest = 0;
                // INFERRED: the reset StartPath and a branch pick run (0x823f6210).
                self.t = Tricks::default();
            }
        } else if self.at.node != old {
            self.branch(paths, &body, others, player);
        }
        let path = &paths[self.path];
        let here = point(path, self.at);
        let s2 = advance(path, self.at, 2.);
        let s30 = advance(path, self.at, 30.);
        let s45 = advance(path, self.at, 45.);
        // Corridor (0x823fa4a0).
        let dist = body.position.distance(here);
        let dir = Vec3::from_array(lerp3(&path.step, self.at)).normalize_or_zero();
        let side = (body.position - here).dot(dir.cross(Vec3::Y));
        let width = path.widths[self.at.node][if side < 0. { 0 } else { 1 }] as f32 * 0.02;
        if dist < width || dist <= 1e-4 {
            self.off_frames = 0;
        } else {
            self.off_frames += 1;
        }
        // Watchdog: reposition at the furthest node (0x823f4568).
        if self.off_frames > 420 {
            self.off_frames = 0;
            self.at = Sample { node: self.furthest, frac: 0. };
            out.reposition = Some(place(path, self.furthest));
            return out;
        }
        // INFERRED: a wipeout resets onto the path after 2.0 s.
        self.wiped_frames = if body.wiping_out { self.wiped_frames + 1 } else { 0 };
        if self.wiped_frames > 120 {
            self.wiped_frames = 0;
            self.at = Sample { node: self.furthest, frac: 0. };
            out.reposition = Some(place(path, self.furthest));
            return out;
        }
        // The trick half's bookkeeping (round2 T5 tricks).
        let rb = match player {
            Some(player) if self.band => band(&body, player),
            _ => 0.,
        };
        let t = &mut self.t;
        // Lock (0x823fb9b8): once past the path's first point, in its corridor (+636) or within 0.5 m.
        if !t.locked && (self.at.node > 0 || self.at.frac > 1e-4) && (dist < width || dist < 0.5) {
            (t.scan, t.locked) = (self.at.node, true);
        }
        // Event scan (0x823fbab8): a frame ahead, stretched by the band (0x823fd8e0: 0.25 x speed x band; its
        // 0.3 x speed x Influence+52 term INFERRED 0).
        t.scan_prev = t.scan;
        let v = speed.abs();
        let adjust = 0.25 * v * rb;
        let ahead = if v > 0.001 && adjust.abs() > 0.01 { (v + adjust) / v } else { 1. };
        t.scan = if ahead > 1e-5 { advance(path, self.at, ahead).node } else { self.at.node };
        // Lookahead (0x823f6688): the next start within 60 frames, within 3 m of the path.
        if dist > 3. {
            t.upcoming = false;
        } else if let Some((n, frames)) = next_start(path, self.at, 60) {
            let trick = trick_at(path, n);
            (t.upcoming, t.next, t.next_frames, t.next_trick, t.next_cat) = (true, n as i32, frames, trick, category(trick));
            t.next_flip = matches!(t.next_cat, 1 | 2);
            // Path mode < 2 (RETAIL: every tutorial rider is mode 1, see flip()): pi for an N... trick.
            t.antic = match (t.next_flip, is_nollie(trick)) {
                (true, true) => (1., std::f32::consts::PI),
                (true, false) => (1., 0.),
                _ => (0., 0.),
            };
        } else {
            (t.next, t.start_early, t.upcoming) = (-1, false, false);
        }
        // Air tracking (0x823f7bd8): a landing ends a grab or flip (classes 2-4 bar coffin, 60) and the hold.
        // Not ported: its bail roll (0x823ef058 -> WipeOutRequest).
        let landed = t.air && !body.airborne;
        t.air = body.airborne;
        t.since_land = if landed { 0 } else { t.since_land + 1 };
        if landed {
            if valid(t.current) && matches!(IDENTIFIERS[t.current as usize].1, 2..=4) && t.current != 60 {
                t.current = -1;
            }
            t.hold = false;
        }
        // Heading error (0x823f7ed0).
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
        if body.airborne {
            // Air turn (0x823fdf08): against the recording's skater facing, 2 deg dead zone, full at 10.
            let want = Quat::from_array(slerp4(&path.skater, s2)) * Vec3::Z;
            let a = signed_angle(body.facing, want, AIR_UP);
            let x = a.abs() - 2. * DEG;
            if x > 0. {
                turn(&mut out, (if a > 0. { -1. } else { 1. }) * (x / (8. * DEG)).min(1.));
            }
            // BoardAdjust (0x823f8850): the board set for a manual ahead.
            let t = &self.t;
            if t.upcoming && t.action != 8 && t.next_cat == 8 {
                let v = manual_value(t.next_trick);
                out.intents.extend([("BoardAdjustAngle", if v > 0. { -2.677 } else { 0.257 }), ("BoardAdjustMag", v.abs())]);
            }
            // INFERRED: no push in the air (+630 clear).
            self.events(&mut out, path, &body, e, false);
            return out;
        }
        // Ground turn (0x823fddf0).
        let m = (e.abs() / (18. * DEG)).min(1.);
        turn(&mut out, if e > 0. { -m } else { m });
        // Locomotion (0x823f8fa8).
        let mut vref = Vec3::from_array(lerp3(&path.step, s30)).length() * 60.;
        let p30 = point(path, s30);
        let flat = Vec2::new(p30.x - here.x, p30.z - here.z).length();
        let slope = if flat > 0.001 { (p30.y - here.y) / flat } else { 0. };
        if dist > 2. {
            vref = vref.max(4.);
        }
        let target = vref + 0.25 * vref * rb;
        let mut pushed = false;
        if speed < target {
            let path_vel = Vec3::from_array(lerp3(&path.step, self.at));
            // +646 (41 < off-path frames < 211) skips the push.
            let calm = !(42..211).contains(&self.off_frames);
            // trickSoon: 0.3 with a start under 40 frames ahead above 2 m/s; none during categories 4, 7, 10, 12.
            let t = &self.t;
            let mut k = if t.upcoming && t.next_frames < 40 && speed > 2. { 0.3 } else { 0.95 };
            if slope < -0.22 {
                k *= 0.3;
            }
            let tricking = matches!(t.action, 4 | 7 | 10 | 12);
            if !tricking && path_vel.dot(fwd) >= 0. && calm && speed < k * target && e.abs() < 40. * DEG {
                // UNTRACED: the push foot (0x823e6810 vf28 != vf12); always the left.
                out.intents.extend([("LeftPush", 1.), ("NewPush", 1.), ("Pushing", 1.)]);
                pushed = true;
            }
        } else if self.at.node >= 1 {
            let k = if slope > 0.22 { 0.3 * 0.4 } else { 0.3 };
            let s = speed * k;
            if s > target && e.abs() < 40. * DEG {
                out.intents.push((if self.t.action == 8 { "ManualBrake" } else { "Brake" }, ((s - target) / 0.1).min(1.)));
            }
        }
        self.events(&mut out, path, &body, e, pushed);
        out
    }

    /// Branch choice on a node change (0x823f97e0): entry 0 stays, then each link from the furthest node; lowest
    /// score wins, ties to the first, all rejected stays.
    fn branch(&mut self, paths: &[Path], body: &Body, others: &[(usize, usize)], player: Option<&Player>) {
        // 0x823f93f8: none in the air, during an ollie, flip or grind, or with an ollie or flip ahead (the previous
        // frame's lookahead: the branch runs before it).
        if body.airborne || matches!(self.t.action, 1 | 2 | 5) || (self.t.upcoming && self.t.next_flip) {
            return;
        }
        let path = &paths[self.path];
        let Some(record) = path.branches.iter().find(|b| b.node == self.furthest) else { return };
        let mut candidates = vec![(self.path, self.at.node)];
        candidates.extend(record.to.iter().filter_map(|l| Some((paths.iter().position(|p| p.name == l.path)?, l.node))));
        let mut best: Option<(u32, usize, usize)> = None;
        for (p, node) in candidates {
            let rand = (((self.next_rand() % 100_000) as f32) * 1e-5 * 400.) as u32;
            if let Some(s) = score(paths, p, node, body, others, player, rand)
                && best.is_none_or(|b| s < b.0)
            {
                best = Some((s, p, node));
            }
        }
        if let Some((_, p, node)) = best.filter(|b| b.1 != self.path || b.2 != self.at.node) {
            self.path = p;
            self.at = nearest(&paths[p], body.position, node.saturating_sub(3), node + 12);
            self.furthest = self.at.node;
            self.t = Tricks::default();
        }
    }

    /// Node events (0x823f77a0): the newest start or end node the scan passed since last frame (under 4 nodes on,
    /// under 2 from node 0), else the early start a lead asked for; then the action's signals and its frame count.
    fn events(&mut self, out: &mut Step, path: &Path, body: &Body, e: f32, pushed: bool) {
        let (scan, prev) = (self.t.scan, self.t.scan_prev);
        let d = scan as i32 - prev as i32;
        let mut hit = if d > 0 && ((prev > 0 && d < 4) || (prev == 0 && d < 2)) {
            (prev + 1..=scan).rev().find(|&n| n as i32 > self.t.last_event && matches!(path.state[n], 1 | 2))
        } else {
            None
        };
        if self.t.start_early && hit.is_none() {
            hit = usize::try_from(self.t.next).ok();
        }
        match hit.map(|n| (n, path.state[n])) {
            Some((n, 1)) => {
                self.t.frames = 0;
                self.t.early = std::mem::take(&mut self.t.start_early);
                self.t.last_event = n as i32;
                let trick = trick_at(path, n);
                let known = valid(trick);
                self.t.action = category(trick);
                let ok = match self.t.action {
                    1 | 2 => self.flip(out, path, n, trick),
                    // Without the profile's +400 these start with nothing to perform.
                    3 | 4 | 12 if !self.profile.grabs => true,
                    // 0x823f7388 (its own 0x823fdcc0 call for 12 writes what 0x823f6bf0 writes this frame).
                    3 | 4 | 5 | 7 | 10 | 12 | 13 => {
                        if known {
                            self.t.current = trick;
                        }
                        known
                    }
                    6 | 9 => known,
                    8 => {
                        if known {
                            self.t.manual = manual_value(trick);
                        }
                        known
                    }
                    // 0x823f76a0.
                    11 => {
                        if known {
                            out.intents.push(("Trick", 1.));
                            match trick {
                                4 => out.intents.push(("SlideFs180", 1.)),
                                5 => out.intents.push(("SlideBs180", 1.)),
                                _ => {}
                            }
                        }
                        known
                    }
                    _ => true,
                };
                if !ok {
                    (self.t.hold, self.t.action) = (false, 0);
                }
            }
            Some((_, 2)) => {
                (self.t.early, self.t.hold, self.t.action) = (false, false, 0);
            }
            _ => {}
        }
        self.signals(out, path, body, e, pushed);
        self.t.frames += 1;
    }

    /// The action's signals (0x823f6bf0): Antic for an ollie or flip ahead, Crouch where the recording crouched, a
    /// manual's value ahead (airborne or within 5 frames of landing), a lead's early start (+626), then by category.
    fn signals(&mut self, out: &mut Step, path: &Path, body: &Body, e: f32, pushed: bool) {
        let t = &self.t;
        let antic = t.upcoming && t.antic.0 > 0. && !pushed && e.abs() < std::f32::consts::FRAC_PI_4 && t.action != 8;
        if antic {
            out.intents.extend([("AnticMag", t.antic.0), ("AnticAngle", t.antic.1)]);
        }
        // The sample's flags (+100) are its nearer node's.
        let node = if self.at.frac < 0.5 { self.at.node } else { (self.at.node + 1).min(path.flags.len() - 1) };
        if path.flags[node] & 0x02 != 0 && !pushed && !antic && t.action == 0 {
            out.intents.push(("Crouch", 1.));
        }
        if t.upcoming {
            if (t.air || t.since_land < 5) && t.action != 8 && t.next_cat == 8 {
                out.intents.push(("Manual", manual_value(t.next_trick)));
            }
            let lead = self.row(t.next_trick).map_or(0, |r| r.lead);
            if !t.early && lead > 0 && t.next_frames < lead as i32 {
                self.t.start_early = true;
            }
        }
        match self.t.action {
            1 | 2 => self.hold(out, body),
            3 | 4 | 5 | 7 | 10 | 12 | 13 => self.recipe(out, self.t.current),
            6 => out.intents.push(("GrabWorld", 1.)),
            8 => out.intents.push(("Manual", self.t.manual)),
            9 if self.t.locked => out.intents.extend(if e < 0. {
                [("RightSlideStart", 1.), ("RightSlide", 0.4)]
            } else {
                [("LeftSlideStart", 1.), ("LeftSlide", 0.4)]
            }),
            _ => {}
        }
    }

    /// 0x823f6ad8: a variant popped as its +8 row holds (HoldPattern) until the skater scores it, bar a frame that
    /// writes Trick. INFERRED: the skater's trick list (+6320) is the port's published trick name.
    fn hold(&mut self, out: &mut Step, body: &Body) {
        if !self.t.hold {
            return;
        }
        if valid(self.t.current) && body.trick == Some(encode(IDENTIFIERS[self.t.current as usize].0.as_bytes())) {
            self.t.hold = false;
        } else if !out.intents.iter().any(|(n, _)| *n == "Trick") {
            out.intents.push(("HoldPattern", 1.));
        }
    }

    /// 0x823fdcc0: the trick's recipe, each signal inside its frame window.
    fn recipe(&self, out: &mut Step, trick: i32) {
        let Some(row) = self.row(trick) else { return };
        for &(signal, value, from, to) in &row.recipe {
            if signal != 0 && self.t.frames >= from && (to < 0 || self.t.frames <= to) {
                out.intents.extend(SIGNALS.get(signal as usize).map(|&name| (name, value)));
            }
        }
    }

    /// An ollie or flip start (0x823f7528): a recorded variant (its +8 row another flip) pops nothing; mode 1 may vary
    /// the trick (0x823f70d8 -> 0x823f6f18), then 0x823f6938 pops it. Mode 1 is RETAIL for every tutorial rider: the
    /// mode (Influence+60) is 1 from 0x823f3fb4 and only rewritten by the Move task's 0x823b6590 from task+72, which
    /// is 1 (0x823ff088) unless StartPath runs under an active path filter (0x826dbf6c); the photo lessons never
    /// filter and Follow Slappy's tRoundPaths +800 = 1 leaves the filter off (0x824b50c8). Mode 0's 0x823f7448 pops
    /// the recorded trick (its chain loop returns its argument).
    fn flip(&mut self, out: &mut Step, path: &Path, n: usize, trick: i32) -> bool {
        let base = if valid(alt(trick)) { alt(trick) } else { trick };
        if base != trick && base != 128 {
            return true;
        }
        let trick = if vary(path, n, trick) { self.pick(trick) } else { trick };
        if !valid(trick) {
            return false;
        }
        self.trigger(out, trick);
        true
    }

    /// 0x823f6f18 for an ollie or flip: a weighted draw (0x823eede0) from the profile's list (the nollie list for an
    /// N... trick); the trick itself without a scorable row. UNTRACED: the global group check (0x82e57390),
    /// INFERRED to pass.
    fn pick(&mut self, trick: i32) -> i32 {
        let nollie = is_nollie(trick);
        // ponytail: an empty list keeps the recorded trick (retail would read entry 0 of nothing).
        if self.row(trick).is_none() || (if nollie { &self.profile.nollie } else { &self.profile.regular }).is_empty() {
            return trick;
        }
        let r = (self.next_rand() & 0x7FFF) as f32 * 3.051_850_9e-5;
        let list = if nollie { &self.profile.nollie } else { &self.profile.regular };
        // 0x823eec28 keeps each weight times 1 / their f32 sum.
        let sum: f32 = list.iter().map(|e| e.1).sum();
        let scale = if sum > 0. { 1. / sum } else { 1. };
        let mut c = 0.;
        list.iter()
            .find(|e| {
                c += e.1 * scale;
                r < c
            })
            .unwrap_or(&list[0])
            .0
    }

    /// 0x823f6938: pop `trick` as its +8 row (a variant's plain flip, a late flip's ollie) when that row is scorable
    /// and its TrickType (its category) matches the action, ollie and flip counting as one.
    fn trigger(&mut self, out: &mut Step, trick: i32) {
        let base = if valid(alt(trick)) { alt(trick) } else { trick };
        let (cat, action) = (category(base), self.t.action);
        if !valid(base) || self.row(base).is_none() || (cat != action && !(matches!(cat, 1 | 2) && matches!(action, 1 | 2))) {
            return;
        }
        out.intents.extend([("Trick", 1.), (IDENTIFIERS[base as usize].0, 1.), ("GestureSpeed", 1.)]);
        if matches!(action, 1 | 2) {
            out.intents.extend([("TrickHeight", 1.), ("DontMirrorTrick", 1.)]);
        }
        (self.t.hold, self.t.current) = (base != trick && base != 128, trick);
    }

    fn row(&self, trick: i32) -> Option<&Trick> {
        self.tricks.get(usize::try_from(trick).ok()?)?.as_ref()
    }

    fn next_rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }
}

/// Retail's branch score (0x823f9850); None = rejected.
pub(crate) fn score(
    paths: &[Path],
    path: usize,
    node: usize,
    body: &Body,
    others: &[(usize, usize)],
    player: Option<&Player>,
    rand_term: u32,
) -> Option<u32> {
    let p = &paths[path];
    let n1 = node + 1;
    if n1 >= p.pos.len() {
        return None;
    }
    let d = Vec3::from_array(p.pos[n1]) - body.position;
    // ponytail: retail measures from the board's raw At; Body::forward is negated riding fakie.
    let a = signed_angle(body.forward, d, body.up).abs();
    if a >= 0.872_664_63 {
        return None;
    }
    let mut score = (a * 572.9578) as u32;
    if node > 0 && (node - 1..=n1.min(p.pos.len() - 1)).any(|i| p.flags[i] & 0x04 != 0 || p.state[i] != 0) {
        return None;
    }
    let crowd = others.iter().filter(|&&(op, on)| op == path && on.abs_diff(node) <= 5).count() as u32;
    score += (crowd << 10) + rand_term;
    if let Some(player) = player {
        let md = (node..p.pos.len()).step_by(2).map(|i| Vec3::from_array(p.pos[i]).distance(player.position)).fold(f32::MAX, f32::min);
        score += ((md * 30.) as u32).min(1500);
    }
    if p.h44 & 7 == 7 {
        score += 1000;
    }
    Some(score)
}

/// The follow lesson's rubber band (0x823fca08, mode 23): the AI waits for a player behind and runs from one ahead.
/// INFERRED: mode 23 for Follow Slappy.
pub(crate) fn band(ai: &Body, player: &Player) -> f32 {
    if player.wiping_out {
        return -1.;
    }
    let dist = player.position.distance(ai.position);
    let rb = if dist > 5. { ((dist - 5.) / 10.).min(1.) } else { 0. };
    if (player.position - ai.position).dot(ai.forward) < 0. { -rb } else { rb }
}

/// The air turn's axis: world up. INFERRED: the deck's up (the ground turn's) turns over in a flip, which flipped the
/// air turn's sign mid-flip and spun the rider round (user 10-15 run12: P-Rod's nollie flips on fp_u3_tutorial02
/// path 1 landed him facing back up his path).
pub(crate) const AIR_UP: Vec3 = Vec3::Y;

pub(crate) fn turn(out: &mut Step, m: f32) {
    // INFERRED: PhysBodySpin rides with BodySpin, as Skate 3's listener pairs them.
    out.intents.extend([("Turn", m), ("BodySpin", m), ("KickTurn", m), ("PhysBodySpin", m)]);
}

/// SignedAngle: 0 for a near-zero vector, else the angle signed by `axis`, wrapped to (-pi, pi].
pub(crate) fn signed_angle(a: Vec3, b: Vec3, axis: Vec3) -> f32 {
    if a.length_squared() <= 1e-4 || b.length_squared() <= 1e-4 {
        return 0.;
    }
    let mut angle = a.normalize().dot(b.normalize()).clamp(-1., 1.).acos();
    if a.cross(b).dot(axis) < 0. {
        angle = std::f32::consts::TAU - angle;
    }
    if angle > std::f32::consts::PI { angle - std::f32::consts::TAU } else { angle }
}

pub(crate) fn nearest(path: &Path, at: Vec3, from: usize, to: usize) -> Sample {
    if path.pos.len() < 2 {
        return Sample { node: 0, frac: 0. };
    }
    let last = path.pos.len().saturating_sub(2);
    let mut best = (f32::MAX, Sample { node: from.min(last), frac: 0. });
    for i in from.min(last)..=to.min(last) {
        let (a, b) = (Vec3::from_array(path.pos[i]), Vec3::from_array(path.pos[i + 1]));
        let ab = b - a;
        let t = if ab.length_squared() > 1e-12 { ((at - a).dot(ab) / ab.length_squared()).clamp(0., 1.) } else { 0. };
        let d = at.distance_squared(a + ab * t);
        // Ties to the later segment: a node is its own segment's start.
        if d <= best.0 {
            best = (d, Sample { node: i, frac: t });
        }
    }
    let mut s = best.1;
    if s.frac >= 1. {
        s = Sample { node: s.node + 1, frac: 0. };
    }
    s
}

/// The sample `frames` ahead (0x823e90d8, node +36 ticks); never past the end, never off node 0 at its start.
pub(crate) fn advance(path: &Path, from: Sample, frames: f32) -> Sample {
    if from.node == 0 && from.frac <= 0.001 {
        return from;
    }
    let (mut node, mut frac, mut left) = (from.node, from.frac, frames);
    while node + 1 < path.pos.len() {
        let len = path.ticks[node + 1].max(1) as f32;
        let rest = (1. - frac) * len;
        if left < rest {
            return Sample { node, frac: frac + left / len };
        }
        left -= rest;
        node += 1;
        frac = 0.;
    }
    Sample { node, frac: 0. }
}

pub(crate) fn point(path: &Path, s: Sample) -> Vec3 {
    Vec3::from_array(lerp3(&path.pos, s))
}

pub(crate) fn lerp3(v: &[[f32; 3]], s: Sample) -> [f32; 3] {
    let a = Vec3::from_array(v[s.node]);
    let b = Vec3::from_array(v[(s.node + 1).min(v.len() - 1)]);
    a.lerp(b, s.frac).to_array()
}

pub(crate) fn slerp4(v: &[[f32; 4]], s: Sample) -> [f32; 4] {
    let a = Quat::from_array(v[s.node]).normalize();
    let b = Quat::from_array(v[(s.node + 1).min(v.len() - 1)]).normalize();
    a.slerp(b, s.frac).to_array()
}

/// Rows right, up, at, position: the recording's board orientation at `node`, turned end for end when its At points
/// back along the path. INFERRED (the start's fallback, "onto node 0 facing along it"; Skate 1's R4 is UNTRACED):
/// user 10-15 run15, 8 of he_u1_skate01's 29 recordings and he_u2_follow01's A_4 / A_6 open on a board 180 deg from
/// their travel under a skater facing on (a shove-it's swapped ends, skate1_rider::forward). Placed by it, Shingo
/// faced back up the path: stuck at node 0 in S.K.A.T.E., the wrong way first on the follow route.
pub(crate) fn place(path: &Path, node: usize) -> [[f32; 4]; 4] {
    let mut q = Quat::from_array(path.board[node]).normalize();
    let here = Vec3::from_array(path.pos[node]);
    let ahead = path.pos[node + 1..].iter().map(|p| Vec3::from_array(*p) - here).find(|d| d.length() > 0.5);
    if ahead.is_some_and(|d| (q * Vec3::Z).dot(d) < 0.) {
        q *= Quat::from_rotation_y(std::f32::consts::PI);
    }
    let row = |v: Vec3, w: f32| [v.x, v.y, v.z, w];
    [row(q * Vec3::X, 0.), row(q * Vec3::Y, 0.), row(q * Vec3::Z, 0.), row(Vec3::from_array(path.pos[node]), 1.)]
}

/// Skate 2's AI signal enum (names at 0x82d4bc90), a recipe's signal index. The S3 port reads none of MarkGonzales,
/// QuickOllie, GestureDown, GestureUp, NewLeftGroundGrab, NewRightGroundGrab, FingerFlipVar, AI_ForceOffBoard.
const SIGNALS: [&str; 80] = [
    "INVALID", "MarkGonzales", "DannyWay", "QuickOllie", "ForceBrake", "WipeOutRecover", "WipeOutRequest",
    "GestureDown", "GestureLeft", "GestureRight", "GestureUp", "RightPush", "LeftPush", "NewPush", "Pushing",
    "BodySpin", "LeftGroundGrab", "RightGroundGrab", "NewLeftGroundGrab", "NewRightGroundGrab", "LeftAirGrab",
    "RightAirGrab", "NewLeftAirGrab", "NewRightAirGrab", "NewDismount", "Dismount", "TweakX", "TweakY",
    "BoardAdjustAngle", "BoardAdjustMag", "AnticAngle", "AnticMag", "KickTurn", "Turn", "HardTurn", "GrindBalanceX",
    "Brake", "WipeoutGestureX", "WipeoutGestureY", "WipeoutControlX", "WipeoutControlY", "Slide", "LeftSlideStart",
    "RightSlideStart", "LeftSlide", "RightSlide", "Manual", "ManualBrake", "HardTurnCrouch", "Crouch", "HoldPattern",
    "GestureSpeed", "FrontFlip", "BackFlip", "NoInput", "Trick", "TrickHeight", "DontMirrorTrick", "SlideFs180",
    "SlideBs180", "AutoPumpAngle", "AutoPumpMag", "ExitGrind", "GrabWorld", "PhysGrindTranslation",
    "PhysGrindStabilityNudge", "PhysGrindUpDown", "PhysBodySpin", "HandPlantTweakX", "HandPlantTweakY",
    "HandPlantDismount", "HandPlantOneFootRight", "HandPlantOneFootLeft", "FingerFlip", "FingerFlipShuv",
    "FingerFlipVar", "Ollie", "ToggleOffBoardState", "NewToggleOffBoardState", "AI_ForceOffBoard",
];

/// A Skate 2 trick index: table 0x820614c8's 269 rows, the same as Skate 3's first 269.
fn valid(trick: i32) -> bool {
    (0..269).contains(&trick)
}

/// Table +16: the action category (IDENTIFIERS' score type), 0 for no trick.
pub(crate) fn category(trick: i32) -> i32 {
    if valid(trick) { IDENTIFIERS[trick as usize].2 as i32 } else { 0 }
}

/// Table +8: the row a variant pops as (its plain flip; a late flip's ollie), -1 none.
fn alt(trick: i32) -> i32 {
    match trick {
        93 | 94 | 134 => 92,
        97 | 98 | 135 => 96,
        114 | 115 | 136 => 113,
        118 | 119 | 137 => 117,
        100..=105 => 128,
        _ => -1,
    }
}

/// Table +4: the combo link (the flip a variant follows), -1 none.
pub(crate) fn link(trick: i32) -> i32 {
    match trick {
        93 => 92,
        94 => 93,
        134 => 94,
        97 => 96,
        98 => 97,
        135 => 98,
        114 => 113,
        115 => 114,
        136 => 115,
        118 => 117,
        119 => 118,
        137 => 119,
        _ => -1,
    }
}

/// 0x823f6530: an N... trick (nollie, n_kickflip).
pub(crate) fn is_nollie(trick: i32) -> bool {
    valid(trick) && matches!(IDENTIFIERS[trick as usize].0.as_bytes()[0], b'n' | b'N')
}

/// 0x823f6828: nosemanual 0.5, tailmanual -0.5 (attribute 742E1EE3337D654E), else 0.
fn manual_value(trick: i32) -> f32 {
    match trick {
        2 => 0.5,
        3 => -0.5,
        _ => 0.,
    }
}

/// The trick on node `n`, -1 none.
fn trick_at(path: &Path, n: usize) -> i32 {
    path.tricks.iter().find(|t| t.node == n).map_or(-1, |t| t.trick)
}

/// 0x823e9000 for event 1: the next start node within `max` frames, and the frames to it.
pub(crate) fn next_start(path: &Path, at: Sample, max: i32) -> Option<(usize, i32)> {
    let mut i = at.node + 1;
    let mut frames = (*path.ticks.get(i)? as f32 * (1. - at.frac)) as i32;
    while i < path.pos.len() && frames < max {
        if path.state[i] == 1 {
            return Some((i, frames));
        }
        i += 1;
        frames += path.ticks.get(i).map_or(0, |&t| t as i32);
    }
    None
}

/// 0x823f70d8 (mode 1): may the flip on node `n` vary? Yes 300 frames on or at the path's end; at the next start only
/// if it is an ollie or flip linked (table +4) to this one; at an air run's end only past 50 frames and with no
/// start on the node after.
fn vary(path: &Path, n: usize, trick: i32) -> bool {
    let (mut frames, mut air) = (0, false);
    for i in n + 1..path.pos.len() {
        if frames >= 300 {
            return true;
        }
        frames += path.ticks[i];
        let here = path.flags[i] & 0x04 != 0;
        if path.state[i] == 1 {
            let next = trick_at(path, i);
            return matches!(category(next), 1 | 2) && (if valid(next) { link(next) } else { next }) == trick;
        }
        if air && !here {
            return path.state.get(i + 1) != Some(&1) && frames > 50;
        }
        air |= here;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// A straight recording along `dir` from `from`: `n` nodes 10 frames apart at `speed` m/s, board facing `dir`.
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
            h44: 7,
            branches: Vec::new(),
            tricks: Vec::new(),
        }
    }

    fn body(position: Vec3, forward: Vec3, velocity: Vec3) -> Body {
        Body { position, velocity, forward, facing: forward, up: Vec3::Y, airborne: false, wiping_out: false, trick: None }
    }

    fn value(step: &Step, name: &str) -> Option<f32> {
        step.intents.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }

    /// round2-research4 Q1 (0x823fddf0): Turn = BodySpin = KickTurn, full at 18 deg of heading error, linear below it,
    /// positive when the path lies clockwise of the board (SignedAngle(fwd, path, up) < 0).
    #[test]
    fn a_driver_turns_toward_its_path_with_retail_sign_and_ramp() {
        let paths = [line("a", 20, Vec3::ZERO, Vec3::Z, 6.)];
        let off = |degrees: f32| Quat::from_rotation_y(degrees.to_radians()) * Vec3::Z;
        let mut driver = Driver::new(0);
        let step = driver.step(&paths, body(Vec3::ZERO, off(9.), Vec3::ZERO), &[], None);
        for name in ["Turn", "BodySpin", "KickTurn"] {
            assert!((value(&step, name).unwrap() - 0.5).abs() < 1e-3, "{name}: {:?}", step.intents);
        }
        let mut driver = Driver::new(0);
        let step = driver.step(&paths, body(Vec3::ZERO, off(-60.), Vec3::ZERO), &[], None);
        assert_eq!(value(&step, "Turn"), Some(-1.), "full turn the other way past 18 deg");
    }

    /// Q1 0x823f89d8 / 0x823f8d18: push while under 0.95 of the path's speed 30 frames ahead; brake only once 0.3 of
    /// the speed passes it (and never on node 0); in between, coast.
    #[test]
    fn a_driver_pushes_below_its_path_speed_and_brakes_far_above_it() {
        let paths = [line("a", 20, Vec3::ZERO, Vec3::Z, 6.)];
        let at = Vec3::new(0., 0., 5.);
        let mut driver = Driver::new(0);
        let step = driver.step(&paths, body(at, Vec3::Z, Vec3::ZERO), &[], None);
        assert_eq!((value(&step, "LeftPush"), value(&step, "NewPush"), value(&step, "Pushing")), (Some(1.), Some(1.), Some(1.)));
        assert_eq!(value(&step, "Brake"), None);
        let mut driver = Driver::new(0);
        let step = driver.step(&paths, body(at, Vec3::Z, Vec3::Z * 25.), &[], None);
        assert_eq!(value(&step, "Brake"), Some(1.), "25 m/s x 0.3 = 7.5 > 6: full brake");
        assert_eq!(value(&step, "LeftPush"), None);
        let mut driver = Driver::new(0);
        let step = driver.step(&paths, body(at, Vec3::Z, Vec3::Z * 7.), &[], None);
        assert_eq!((value(&step, "Brake"), value(&step, "LeftPush")), (None, None), "coasting");
    }

    /// UNTRACED rule (round2 T5): at a recording's end the driver goes on along the one that starts within 2.5 m
    /// (Follow Slappy's segments chain 0-5 and 6-10 that way; a photo lap ends 1-2.1 m from its own start and loops).
    #[test]
    fn a_driver_follows_its_path_onto_the_next_and_loops() {
        let a = line("a", 6, Vec3::ZERO, Vec3::Z, 6.);
        let b = line("b", 6, Vec3::new(0., 0., 5.2), Vec3::X, 6.);
        let paths = [a, b];
        let mut driver = Driver::new(0);
        for z in [0., 2., 4., 5., 5.1] {
            driver.step(&paths, body(Vec3::new(0., 0., z), Vec3::Z, Vec3::Z * 6.), &[], None);
        }
        assert_eq!(driver.path(), 1, "on b");
        let mut lap = line("lap", 6, Vec3::ZERO, Vec3::Z, 6.);
        lap.pos[5] = [0.5, 0., 1.];
        let paths = [lap];
        let mut driver = Driver::new(0);
        for z in [0., 2., 4.] {
            driver.step(&paths, body(Vec3::new(0., 0., z), Vec3::Z, Vec3::Z * 6.), &[], None);
        }
        // The last node, beside the start: nearer than the lap's own nodes 1-2 (0.5 m off).
        driver.step(&paths, body(Vec3::new(0.5, 0., 1.), Vec3::Z, Vec3::Z * 6.), &[], None);
        assert_eq!((driver.path(), driver.node() < 3), (0, true), "looped to its start");
    }

    /// Q2 0x823f9850: a candidate scores its angle to the next node in 0.1 deg (none at 50 deg or more), 1024 per
    /// other AI within 5 nodes, the random term and +1000 for segment flags 0x7; never next to an airborne or event
    /// node. Retail takes the lowest.
    #[test]
    fn a_branch_scores_retail_terms_and_never_passes_an_airborne_node() {
        let mut a = line("a", 10, Vec3::ZERO, Vec3::Z, 6.);
        a.h44 = 0;
        let paths = [a];
        let here = body(Vec3::new(0., 0., 1.), Vec3::Z, Vec3::ZERO);
        assert_eq!(score(&paths, 0, 1, &here, &[], None, 0), Some(0));
        assert_eq!(score(&paths, 0, 1, &here, &[(0, 4), (0, 7)], None, 399), Some(1024 + 399), "one AI within 5 nodes");
        let turned = body(Vec3::new(0., 0., 1.), Quat::from_rotation_y(0.5) * Vec3::Z, Vec3::ZERO);
        assert_eq!(score(&paths, 0, 1, &turned, &[], None, 0), Some((0.5 * 572.9578f32) as u32));
        let away = body(Vec3::new(0., 0., 1.), Quat::from_rotation_y(PI * 0.3) * Vec3::Z, Vec3::ZERO);
        assert_eq!(score(&paths, 0, 1, &away, &[], None, 0), None, "54 deg");
        let mut paths = paths;
        paths[0].flags[2] = 0x04;
        assert_eq!(score(&paths, 0, 1, &here, &[], None, 0), None, "airborne node beside it");
        paths[0].flags[2] = 0;
        paths[0].h44 = 7;
        assert_eq!(score(&paths, 0, 1, &here, &[], None, 0), Some(1000));
        let player = Player { position: Vec3::new(10., 0., 1.), wiping_out: false };
        assert_eq!(score(&paths, 0, 1, &here, &[], Some(&player), 0), Some(1000 + 300), "player term 30 per metre");
    }

    /// Q1 0x823fca08 (mode 23): no band within 5 m, then a linear ramp to 1 at 15 m, negative with the player behind,
    /// -1 while the player wipes out.
    #[test]
    fn the_follow_band_waits_for_a_player_behind_and_runs_from_one_ahead() {
        let leader = body(Vec3::ZERO, Vec3::Z, Vec3::ZERO);
        let at = |z: f32| Player { position: Vec3::new(0., 0., z), wiping_out: false };
        assert_eq!(band(&leader, &at(-3.)), 0.);
        assert!((band(&leader, &at(-10.)) + 0.5).abs() < 1e-6);
        assert_eq!(band(&leader, &at(-40.)), -1.);
        assert!((band(&leader, &at(10.)) - 0.5).abs() < 1e-6);
        assert_eq!(band(&leader, &Player { wiping_out: true, ..at(10.) }), -1.);
    }

    /// Q1 watchdog (fceb0): more than 420 frames off its path's corridor and the AI is put back on it (0x823f4568),
    /// at the furthest node it reached.
    #[test]
    fn a_rider_off_its_path_too_long_goes_back_to_it() {
        let paths = [line("a", 20, Vec3::ZERO, Vec3::Z, 6.)];
        let mut driver = Driver::new(0);
        driver.step(&paths, body(Vec3::new(0., 0., 3.), Vec3::Z, Vec3::Z * 6.), &[], None);
        let lost = body(Vec3::new(20., 0., 3.), Vec3::Z, Vec3::ZERO);
        let back: Vec<_> = (0..421).filter_map(|_| driver.step(&paths, lost, &[], None).reposition).collect();
        assert_eq!(back.len(), 1, "once, on frame 421");
        let to = Vec3::from_slice(&back[0][3][..3]);
        assert!(to.distance(Vec3::from_array(paths[0].pos[driver.node()])) < 1e-4 && to.z >= 2., "{to:?}");
        assert!(Vec3::from_slice(&back[0][2][..3]).abs_diff_eq(Vec3::Z, 1e-4), "facing along it");
    }

    // round2 T5 tricks. The rides below run a `line` along +Z at 6 m/s (nodes 1 m, 10 frames apart) from z = 0.05:
    // the event scan, one frame ahead of the rider (0x823fbab8), passes node k on frame 10k - 1, and the lookahead
    // (0x823f6688, 0x823e9000) sees a start on node k from frame 10k - 60.

    /// Every Skate 2 trick with a scorable row and an empty recipe.
    fn rows() -> Vec<Option<Trick>> {
        vec![Some(Trick::default()); 269]
    }

    /// `trick` starts on node `n` (event 1) and, with `end`, ends on that node (event 2).
    fn put_trick(path: &mut Path, n: usize, trick: i32, end: Option<usize>) {
        path.state[n] = 1;
        path.tricks.push(TrickNode { node: n, trick });
        if let Some(end) = end {
            path.state[end] = 2;
        }
    }

    fn rider(tricks: Vec<Option<Trick>>, profile: Profile) -> Driver {
        let mut driver = Driver::new(0);
        (driver.tricks, driver.profile) = (tricks, profile);
        driver
    }

    fn at(frame: usize) -> Vec3 {
        Vec3::new(0., 0., 0.05 + 0.1 * frame as f32)
    }

    fn ride(driver: &mut Driver, paths: &[Path], frames: std::ops::Range<usize>, forward: Vec3) -> Vec<Step> {
        frames.map(|f| driver.step(paths, body(at(f), forward, Vec3::Z * 6.), &[], None)).collect()
    }

    /// 0x823f6688: a start within 60 frames arms Antic (AnticMag 1, AnticAngle 0, pi for a nollie trick); 0x823f77a0
    /// pops it the frame the scan passes its node (0x823f7528 -> 0x823f6938: Trick, the row's name, GestureSpeed,
    /// TrickHeight, DontMirrorTrick). Its air run ends 40 frames on (not over 50, 0x823f70d8): not varied.
    #[test]
    fn a_flip_ahead_anticipates_then_pops_on_the_frame_its_node_is_passed() {
        let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
        put_trick(&mut a, 10, 128, None);
        for i in 11..14 {
            a.flags[i] = 0x04;
        }
        let paths = [a];
        let mut driver = rider(rows(), Profile { regular: vec![(96, 1.)], ..Profile::default() });
        let steps = ride(&mut driver, &paths, 0..110, Vec3::Z);
        assert_eq!(value(&steps[39], "AnticMag"), None, "60 frames out");
        assert_eq!((value(&steps[40], "AnticMag"), value(&steps[40], "AnticAngle")), (Some(1.), Some(0.)));
        assert_eq!(value(&steps[98], "Trick"), None);
        let pop = &steps[99];
        for name in ["Trick", "ollie", "GestureSpeed", "TrickHeight", "DontMirrorTrick", "AnticMag"] {
            assert_eq!(value(pop, name), Some(1.), "{name}: {:?}", pop.intents);
        }
        assert_eq!(value(pop, "kickflip"), None, "not varied");
        assert!(steps[100..].iter().all(|s| value(s, "Trick").is_none() && value(s, "AnticMag").is_none()));
        let mut n = line("n", 30, Vec3::ZERO, Vec3::Z, 6.);
        put_trick(&mut n, 10, 117, None);
        let paths = [n];
        let mut driver = rider(rows(), Profile::default());
        let steps = ride(&mut driver, &paths, 0..51, Vec3::Z);
        assert_eq!(value(&steps[50], "AnticAngle"), Some(PI), "n_kickflip");
    }

    /// A grab (category 4, profile +400) starts its row's lead (15) frames early (+626) and writes its recipe
    /// (0x823fdcc0) until its end node (event 2); without +400 it writes nothing. No push while grabbing (0x823f89d8).
    #[test]
    fn a_grab_starts_its_lead_early_and_its_end_event_stops_it() {
        let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
        put_trick(&mut a, 10, 67, Some(14));
        let paths = [a];
        let mut tricks = rows();
        tricks[67] = Some(Trick { lead: 15, recipe: vec![(21, 1., 0, -1), (17, 1., 0, -1)] });
        let grabs = Profile { grabs: true, ..Profile::default() };
        let grabbing = |s: &Step| (value(s, "RightAirGrab"), value(s, "RightGroundGrab"));
        let mut driver = rider(tricks.clone(), grabs.clone());
        let steps = ride(&mut driver, &paths, 0..150, Vec3::Z);
        assert_eq!(grabbing(&steps[85]), (None, None), "14 frames out");
        for f in 86..139 {
            assert_eq!(grabbing(&steps[f]), (Some(1.), Some(1.)), "frame {f}");
        }
        assert!(steps[139..].iter().all(|s| grabbing(s) == (None, None)), "ended");
        let mut driver = rider(tricks.clone(), Profile::default());
        assert!(ride(&mut driver, &paths, 0..150, Vec3::Z).iter().all(|s| grabbing(s) == (None, None)), "no +400");
        let mut driver = rider(tricks.clone(), grabs.clone());
        ride(&mut driver, &paths, 0..120, Vec3::Z);
        let slow = driver.step(&paths, body(at(119) + Vec3::Z * 0.04, Vec3::Z, Vec3::Z * 2.4), &[], None);
        assert_eq!(value(&slow, "LeftPush"), None, "grabbing");
        let mut driver = rider(tricks, grabs);
        ride(&mut driver, &paths, 0..150, Vec3::Z);
        let slow = driver.step(&paths, body(at(149) + Vec3::Z * 0.04, Vec3::Z, Vec3::Z * 2.4), &[], None);
        assert_eq!(value(&slow, "LeftPush"), Some(1.), "ended");
    }

    /// Manuals (category 8, 0x823f6828: nosemanual 0.5, tailmanual -0.5) write Manual from start to end node; ahead of
    /// one, airborne or within 5 frames of landing, Manual early and in the air BoardAdjust (0x823f8850: -2.677 for a
    /// positive value, else 0.257; magnitude |value|). A manual brakes with ManualBrake (0x823f8d18).
    #[test]
    fn manuals_write_their_value_set_the_board_in_the_air_and_brake_with_manual_brake() {
        let path = |trick| {
            let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
            put_trick(&mut a, 10, trick, Some(14));
            [a]
        };
        let adjust = |s: &Step| (value(s, "Manual"), value(s, "BoardAdjustAngle"), value(s, "BoardAdjustMag"));
        let paths = path(2);
        let mut driver = rider(rows(), Profile::default());
        let steps = ride(&mut driver, &paths, 0..95, Vec3::Z);
        assert_eq!(value(&steps[90], "Manual"), None, "grounded, long since landing");
        let air = driver.step(&paths, Body { airborne: true, ..body(at(95), Vec3::Z, Vec3::Z * 6.) }, &[], None);
        assert_eq!(adjust(&air), (Some(0.5), Some(-2.677), Some(0.5)));
        let steps = ride(&mut driver, &paths, 96..140, Vec3::Z);
        for f in 96..139 {
            assert_eq!(value(&steps[f - 96], "Manual"), Some(0.5), "frame {f}");
        }
        assert_eq!(value(&steps[139 - 96], "Manual"), None, "ended");
        let paths = path(3);
        let mut driver = rider(rows(), Profile::default());
        ride(&mut driver, &paths, 0..95, Vec3::Z);
        let air = driver.step(&paths, Body { airborne: true, ..body(at(95), Vec3::Z, Vec3::Z * 6.) }, &[], None);
        assert_eq!(adjust(&air), (Some(-0.5), Some(0.257), Some(0.5)));
        let paths = path(2);
        let mut driver = rider(rows(), Profile::default());
        ride(&mut driver, &paths, 0..120, Vec3::Z);
        let fast = driver.step(&paths, body(at(119) + Vec3::Z * 0.5, Vec3::Z, Vec3::Z * 30.), &[], None);
        assert_eq!((value(&fast, "ManualBrake"), value(&fast, "Brake")), (Some(1.), None));
    }

    /// 0x823f6bf0: powerslides (9) write RightSlideStart 1 / RightSlide 0.4 with the path clockwise of the board
    /// (+492 < 0), else the Left pair, until their end node; reverts (11, 0x823f76a0) write Trick and SlideBs180 on
    /// their start frame; grinds (5) their recipe until their end node.
    #[test]
    fn slides_reverts_and_grinds_write_their_category_signals() {
        let off = |degrees: f32| Quat::from_rotation_y(degrees.to_radians()) * Vec3::Z;
        let path = |trick, end| {
            let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
            put_trick(&mut a, 10, trick, end);
            [a]
        };
        let paths = path(1, Some(14));
        for (degrees, start, slide) in [(9., "RightSlideStart", "RightSlide"), (-9., "LeftSlideStart", "LeftSlide")] {
            let mut driver = rider(rows(), Profile::default());
            let steps = ride(&mut driver, &paths, 0..140, off(degrees));
            assert_eq!(value(&steps[98], start), None);
            for f in 99..139 {
                assert_eq!((value(&steps[f], start), value(&steps[f], slide)), (Some(1.), Some(0.4)), "{degrees} frame {f}");
            }
            assert_eq!(value(&steps[139], slide), None, "ended");
        }
        let paths = path(5, None);
        let mut driver = rider(rows(), Profile::default());
        let steps = ride(&mut driver, &paths, 0..101, Vec3::Z);
        assert_eq!((value(&steps[99], "Trick"), value(&steps[99], "SlideBs180")), (Some(1.), Some(1.)));
        assert_eq!((value(&steps[100], "Trick"), value(&steps[100], "SlideBs180")), (None, None));
        let paths = path(6, Some(14));
        let mut tricks = rows();
        tricks[6] = Some(Trick { lead: 0, recipe: vec![(66, -1., 0, -1)] });
        let mut driver = rider(tricks, Profile::default());
        let steps = ride(&mut driver, &paths, 0..140, Vec3::Z);
        assert_eq!(value(&steps[98], "PhysGrindUpDown"), None);
        assert!((99..139).all(|f| value(&steps[f], "PhysGrindUpDown") == Some(-1.)));
        assert_eq!(value(&steps[139], "PhysGrindUpDown"), None, "ended");
    }

    /// 0x823f89d8: push under 0.95 of the path's speed, under 0.3 of it with a start under 40 frames ahead and the
    /// rider over 2 m/s (trickSoon).
    #[test]
    fn the_push_gate_drops_to_0_3_before_a_trick() {
        let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
        put_trick(&mut a, 10, 128, None);
        let paths = [a];
        let push = |z: f32, speed: f32| {
            let mut driver = rider(rows(), Profile::default());
            value(&driver.step(&paths, body(Vec3::new(0., 0., z), Vec3::Z, Vec3::Z * speed), &[], None), "LeftPush")
        };
        assert_eq!(push(2.55, 3.), Some(1.), "74 frames out: 3 < 0.95 x 6");
        assert_eq!(push(8.55, 3.), None, "14 frames out: 3 > 0.3 x 6");
        assert_eq!(push(8.55, 1.5), Some(1.), "not over 2 m/s");
    }

    /// 0x823f93f8: no branch choice during an ollie, flip or grind (+500 in 1, 2, 5), nor with an ollie or flip ahead
    /// (the previous frame's +623 and +624).
    #[test]
    fn a_branch_waits_for_a_flip_or_grind() {
        let branching = |setup: &dyn Fn(&mut Path)| {
            let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
            a.branches = vec![Branch { node: 5, to: vec![Link { path: "b".into(), node: 0 }] }];
            setup(&mut a);
            let mut b = line("b", 30, Vec3::new(0., 0., 5.), Vec3::Z, 6.);
            b.h44 = 0;
            let paths = [a, b];
            let mut driver = rider(rows(), Profile::default());
            ride(&mut driver, &paths, 0..60, Vec3::Z);
            driver.path()
        };
        assert_eq!(branching(&|_| {}), 1, "takes b, the lower score");
        assert_eq!(branching(&|a| put_trick(a, 10, 128, None)), 0, "an ollie 50 frames ahead");
        assert_eq!(branching(&|a| put_trick(a, 3, 6, Some(9))), 0, "grinding");
    }

    /// Mode 1 varies a flip (0x823f70d8: no linked start, path's end) by the profile's weights (0x823eede0); a variant
    /// pops as its +8 row (kickflip2 -> kickflip) and holds (HoldPattern, 0x823f6ad8) from the next frame until the
    /// skater scores it or lands (0x823f7bd8).
    #[test]
    fn a_varied_flip_pops_its_base_and_holds_until_the_variant_scores() {
        let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
        put_trick(&mut a, 10, 96, None);
        let paths = [a];
        let varied = || rider(rows(), Profile { regular: vec![(97, 1.)], ..Profile::default() });
        let mut driver = varied();
        let steps = ride(&mut driver, &paths, 0..101, Vec3::Z);
        for name in ["Trick", "kickflip", "TrickHeight", "DontMirrorTrick"] {
            assert_eq!(value(&steps[99], name), Some(1.), "{name}");
        }
        assert_eq!((value(&steps[99], "kickflip2"), value(&steps[99], "HoldPattern")), (None, None));
        assert_eq!(value(&steps[100], "HoldPattern"), Some(1.));
        let scored = skate_core::animation::skeleton_input::name::encode(b"kickflip2");
        let step = driver.step(&paths, Body { trick: Some(scored), ..body(at(101), Vec3::Z, Vec3::Z * 6.) }, &[], None);
        assert_eq!(value(&step, "HoldPattern"), None, "scored");
        let mut driver = varied();
        ride(&mut driver, &paths, 0..101, Vec3::Z);
        let air = driver.step(&paths, Body { airborne: true, ..body(at(101), Vec3::Z, Vec3::Z * 6.) }, &[], None);
        assert_eq!(value(&air, "HoldPattern"), Some(1.), "airborne");
        assert_eq!(value(&ride(&mut driver, &paths, 102..103, Vec3::Z)[0], "HoldPattern"), None, "landed");
    }

    /// A recorded variant pops nothing (0x823f7528: its +8 row is another flip), nor a flip without a scorable row
    /// (0x823f6938).
    #[test]
    fn a_variant_node_or_missing_row_pops_nothing() {
        for (trick, missing) in [(97, None), (128, Some(128))] {
            let mut a = line("a", 30, Vec3::ZERO, Vec3::Z, 6.);
            put_trick(&mut a, 10, trick, None);
            let paths = [a];
            let mut tricks = rows();
            if let Some(i) = missing {
                tricks[i] = None;
            }
            let mut driver = rider(tricks, Profile::default());
            let steps = ride(&mut driver, &paths, 0..101, Vec3::Z);
            assert!(steps.iter().all(|s| value(s, "Trick").is_none()), "{trick}");
        }
    }
}
