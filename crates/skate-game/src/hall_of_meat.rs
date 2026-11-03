//! Hall Of Meat in free skate, the map's game's own version ([`free_game`]): Skate 3's here behind Control Settings >
//! "Hall Of Meat Mode" (Off by default, as Skate 3's own); Skate 2's in skate2_hall_of_meat.rs, always on as Skate 2's
//! free roam had no setting; none on Skate 1 maps (Skate 1 had no Hall of Meat). Evidence, and what is inferred:
//! skate2port/HOM_FREESKATE_REPORT.md.
//!
//! On a Skate 3 map, with the toggle on and no match or lesson running ([`gate`]), as TU3 runs it:
//! - the world conditioner's capability mask is HoM's 0xffff_f7f7 (AirCollector 0x82DA7888, animation_phase.rs): the
//!   bail slow-motion, the camera's BrokenBone shot and the score panel need it;
//! - [`Scorer`] is Score::HoM::WipeoutScorer (per tick 0x82dae4f0) over the stock vault row Sk8::Score::HoM
//!   'default', with its manager's panel selector (0x82da35ac);
//! - the slow-motion is SimController's Fill 0x82599e2c ([`Scorer::time_scale`]);
//! - the panel is Skate 3's hud2/homscoring (skate2port/old_hall_of_meat3.py) on the online-ui layer ([`panel`]).
use crate::{
    apt_screen::{ONLINE, Screens},
    physics::{PlayerControls, SkaterRuntime},
    skate2_hall_of_meat::Curve,
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use skate_core::camera::SimulationRateRequest;
use skate_data::collections::Collections;

/// Sk8::Score::HoM (vault class 9E4C471BC6EAB615), row 'default'.
const CLASS: &str = "Hash_9E4C471BC6EAB615";
const BONES: usize = 25;
/// The scorer's hard-coded frame (0x820849c8); nothing in it reads a real dt.
const DT: f32 = 1. / 60.;
/// A share below this stops spreading (0x82dae6f0).
const SPREAD_MIN: f32 = 0.1;
const DEG: f32 = 57.29578;
/// Rotation trackers +112, +192, +272 and their axes (ctor 0x82daceb0).
const AXES: [usize; 3] = [2, 0, 1];
/// The engine's FilteredCategory (= the scorer's owner+64).
const GROUND: u32 = 1;
const AIR: u32 = 2;
const GRIND: u32 = 3;
const WIPEOUT: u32 = 4;
const OFFBOARD: u32 = 6;
const OFFBOARD_AIR: u32 = 7;
const NAME: &str = "0-homscoring";

// ---------------------------------------------------------------- the vault row
struct Bone {
    /// (bone, share) it passes force on to; -1 = none.
    links: [(i32, f32); 4],
    /// Scrape, Bruise, Sprain, Dislocate, BreakPart, BreakAll: (points, threshold; 0 = no such level).
    levels: [(i64, f32); 6],
}
struct Tweak {
    id: usize,
    stick: [f32; 2],
    frames: u32,
    points: i64,
}
pub(crate) struct Rules {
    bones: Vec<Bone>,
    tweak_hold: Curve,
    impact: Curve,
    rotation_tracked: Curve,
    tweak_hits: Curve,
    air_time: Curve,
    drop: Curve,
    tweak_spins_0: Curve,
    tweak_spins_3: Curve,
    speed: Curve,
    rotation: Curve,
    bail_time: Curve,
    force_scale: f32,
    tweak_stick: f32,
    tweaks: Vec<Tweak>,
}

impl Rules {
    /// The stock row (skater-collections.json). ponytail: leg_impact / toe_share (wipe start's foot impulse), the
    /// hurt threshold and display decay feed what isn't ported (see the report); entity tables likewise.
    pub(crate) fn load(data: &Collections) -> Result<Self, String> {
        let curve = |name: &str| -> Result<Curve, String> {
            let w = data.words::<20>(CLASS, "default", name)?;
            // PointNegGraphData8: a four-word head, then x[8], y[8].
            Ok(Curve { x: std::array::from_fn(|i| f32::from_bits(w[4 + i])), y: std::array::from_fn(|i| f32::from_bits(w[12 + i])) })
        };
        let set = data.words::<{ BONES * 21 }>(CLASS, "default", "Hash_1F9B5EA147CBEEE9")?;
        let mut bones = Vec::with_capacity(BONES);
        for (b, w) in set.chunks(21).enumerate() {
            let bone = Bone {
                links: std::array::from_fn(|k| (w[1 + 2 * k] as i32, f32::from_bits(w[2 + 2 * k]))),
                levels: std::array::from_fn(|k| (i64::from(w[9 + 2 * k]), f32::from_bits(w[10 + 2 * k]))),
            };
            if w[0] as usize != b || bone.links.iter().any(|&(l, _)| l < -1 || l >= BONES as i32) {
                return Err(format!("HoM bone {b}: bad record"));
            }
            bones.push(bone);
        }
        let field = data.field(CLASS, "default", "Hash_5717CFCCCA59BD47")?;
        let items = field.array.as_ref().ok_or("HoM tweaks: not an array")?;
        let tweaks = (items.items.iter())
            .map(|hex| {
                let w = words(hex)?;
                let &[id, _, x, y, frames, points] = w.as_slice() else { return Err("HoM tweak: not 24 bytes".to_owned()) };
                if id >= 4 {
                    return Err("HoM tweak: id past 3".to_owned());
                }
                Ok(Tweak { id: id as usize, stick: [f32::from_bits(x), f32::from_bits(y)], frames, points: i64::from(points) })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            bones,
            tweak_hold: curve("Hash_A9B81D5BF44762E4")?,
            impact: curve("Hash_95AE883C533FC1EB")?,
            rotation_tracked: curve("Hash_2E61C1BB9D8A8A95")?,
            tweak_hits: curve("Hash_05B6EAA1D6A19CCE")?,
            air_time: curve("Hash_96CDCC590988982E")?,
            drop: curve("Hash_32B1A3C2961F4E6E")?,
            tweak_spins_0: curve("Hash_36274FBAE087E832")?,
            tweak_spins_3: curve("Hash_1D49983B40C59E5A")?,
            speed: curve("Hash_B2A997F8579B1998")?,
            rotation: curve("Hash_6CD9F0CD7675949C")?,
            bail_time: curve("Hash_2451424AD330B08F")?,
            force_scale: data.float(CLASS, "default", "Hash_B7514DE7EBDE9D01")?,
            tweak_stick: data.float(CLASS, "default", "Hash_9C494E9614FA5ED6")?,
            tweaks,
        })
    }
}

fn words(hex: &str) -> Result<Vec<u32>, String> {
    let hex: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    (0..hex.len() / 8)
        .map(|i| hex.get(i * 8..i * 8 + 8).and_then(|w| u32::from_str_radix(w, 16).ok()).ok_or_else(|| "bad hex".to_owned()))
        .collect()
}

/// Every call site truncates the curve (fctidz).
fn pts(curve: &Curve, x: f32) -> i64 {
    curve.at(x) as i64
}

// ---------------------------------------------------------------- one tick, as the scorer reads it
pub(crate) struct Frame {
    /// FilteredCategory (owner+64).
    category: u32,
    /// State68 (the respawn countdown) / State69 (teleport requested).
    countdown: bool,
    teleport: bool,
    /// Air+452: while set nothing time-based accumulates (the wipeout holds its velocity and clock).
    frozen: bool,
    /// Reckoning +16 / +64: the system's velocity and height.
    velocity: Vec3,
    height: f32,
    /// Skeleton+599 (the wipeout is over) and +580 (its response strength, the impact curve's input).
    settled: bool,
    impact: f32,
    /// Per scorer bone: (force, resting on an up-facing world contact).
    contacts: [(f32, bool); BONES],
    /// The ragdoll root's basis rows as the trackers keep them. ponytail: the hips body's [Y, X, Z] columns; which
    /// rows retail's +96/+48/+80 are wasn't traced (the report).
    basis: [Vec3; 3],
}

impl Frame {
    fn read(skater: &SkaterRuntime) -> Self {
        let p = &skater.player_input.physical;
        let flags = &skater.player_state.state_flags;
        let feedback = &skater.collision_feedback;
        let [x, y, z] = skater.skeleton.bodies()[23].rates.basis.columns;
        let v = p.reckoning.vector_16.map(f32::from_bits);
        Self {
            category: p.filtered_state_0,
            countdown: flags[68 - 52],
            teleport: flags[69 - 52],
            frozen: p.air.use_air_reckoning_452 != 0,
            velocity: Vec3::new(v[0], v[1], v[2]),
            height: f32::from_bits(p.reckoning.vector_64[1]),
            settled: p.skeleton.over_599 != 0,
            impact: p.skeleton.response_strength_580,
            // 0x82dae9a8: scorer bone b reads collision part b + 1; FACE (23) part 1's and GROIN (24) part 23's
            // specific contact.
            contacts: std::array::from_fn(|b| {
                let part = &feedback.bones[match b {
                    23 => 1,
                    24 => 23,
                    b => b + 1,
                }];
                let (normal, force) = if b >= 23 { (part.specific_normal, part.specific_force) } else { (part.normal, part.force) };
                (force, part.groups[3] && normal[1] > 0.33)
            }),
            basis: [Vec3::from(y), Vec3::from(x), Vec3::from(z)],
        }
    }
}

// ---------------------------------------------------------------- the scorer
#[derive(Clone, Copy, Default)]
struct Record {
    level: Option<usize>,
    previous: Option<usize>,
    peak: f32,
    /// Decays x0.8 a frame.
    force: f32,
}
#[derive(Clone, Copy, Default)]
struct Tracker {
    rows: [Vec3; 3],
    angle: f32,
}

/// The scorer's per-bail fields (all reset by 0x82dad0a8).
#[derive(Clone, Default)]
struct Bail {
    records: [Record; BONES],
    trackers: [Tracker; 3],
    velocity: Vec3,
    captured: bool,
    speed_pts: i64,
    top: f32,
    airborne: bool,
    air_frames: u32,
    air_pts: i64,
    drop_pts: i64,
    drop: f32,
    max_air_frames: u32,
    max_drop: f32,
    air_pts_sum: i64,
    drop_pts_sum: i64,
    bail_frames: u32,
    impact_sum: f32,
    break_hold: u32,
    banked_rotation: f32,
    rotation: f32,
    max_rotation: f32,
    rotation_pts_max: i64,
    tweak: Option<usize>,
    tweak_frames: u32,
    snapshot: f32,
    held: [f32; 4],
    degrees: [f32; 4],
    tweak_sum: i64,
    tweak_now: i64,
    tweak_done: bool,
}

impl Bail {
    fn new(basis: [Vec3; 3]) -> Self {
        Self { trackers: [Tracker { rows: basis, angle: 0. }; 3], ..default() }
    }
    fn takeoff(&mut self, f: &Frame) {
        self.top = f.height;
        (self.air_frames, self.air_pts, self.drop_pts, self.drop) = (0, 0, 0, 0.);
        self.snapshot = self.banked_rotation;
        self.tweak = None;
        (self.tweak_done, self.tweak_frames) = (false, 0);
    }
    /// 0x82daecb8.
    fn air_update(&mut self, r: &Rules, f: &Frame, stick: [f32; 2]) {
        self.top = self.top.max(f.height);
        if !f.frozen {
            self.air_frames += 1;
        }
        self.drop = self.top - f.height;
        self.air_pts = pts(&r.air_time, self.air_frames as f32 * DT);
        self.drop_pts = pts(&r.drop, self.drop);
        self.body_tweak(r, f.frozen, stick);
    }
    /// 0x82daebc0: a landing banks the air.
    fn bank_air(&mut self) {
        self.air_pts_sum += self.air_pts;
        self.drop_pts_sum += self.drop_pts;
        self.max_air_frames = self.max_air_frames.max(self.air_frames);
        self.max_drop = self.max_drop.max(self.drop);
        (self.air_frames, self.air_pts, self.drop_pts, self.drop) = (0, 0, 0, 0.);
        (self.tweak_done, self.tweak_frames) = (false, 0);
        self.tweak = None;
        self.snapshot = self.banked_rotation;
    }
    /// 0x82dad728: the stick held past tweak_stick within pi/n of a tweak's direction; its frames held score it once.
    fn body_tweak(&mut self, r: &Rules, frozen: bool, [x, y]: [f32; 2]) {
        (self.tweak_done, self.tweak_now) = (false, 0);
        let previous = self.tweak.take();
        let n = r.tweaks.len() as f32;
        if x * x + y * y > r.tweak_stick * r.tweak_stick && n > 0. {
            let stick = Vec2::new(x, y);
            // 0x8296ea60: the unsigned angle between them.
            let found = r.tweaks.iter().find(|t| stick.perp_dot(Vec2::from(t.stick)).atan2(stick.dot(Vec2::from(t.stick))).abs() < std::f32::consts::PI / n);
            if let Some(t) = found {
                self.tweak = Some(t.id);
                if previous != Some(t.id) {
                    (self.snapshot, self.tweak_frames) = (self.rotation, 0);
                    self.tweak_done = frozen;
                } else {
                    let turned = self.rotation - self.snapshot;
                    self.snapshot = self.rotation;
                    if !frozen {
                        self.tweak_frames += 1;
                        self.degrees[t.id] += turned;
                        self.held[t.id] += DT;
                        if self.tweak_frames == t.frames {
                            self.tweak_sum += t.points;
                            (self.tweak_done, self.tweak_now) = (true, t.points);
                        }
                    }
                }
                return;
            }
        }
        self.tweak_frames = 0;
    }
    fn trackers(&mut self, basis: [Vec3; 3], copy_only: bool) {
        for (tracker, axis) in self.trackers.iter_mut().zip(AXES) {
            if copy_only {
                tracker.rows = basis;
            } else {
                self.banked_rotation += track(tracker, axis, basis);
            }
        }
    }
    /// 0x82dad640.
    fn totals(&mut self, r: &Rules, tracking: bool) {
        self.rotation = self.banked_rotation + self.trackers.iter().map(|t| (t.angle * DEG).abs()).sum::<f32>();
        let curve = if tracking { &r.rotation_tracked } else { &r.rotation };
        self.max_rotation = self.max_rotation.max(self.rotation);
        self.rotation_pts_max = self.rotation_pts_max.max(pts(curve, self.rotation));
    }
    fn bail_pts(&self, r: &Rules) -> i64 {
        pts(&r.bail_time, self.bail_frames as f32 * DT)
    }
    /// 0x82dad448 + 0x82dada30. There is no multiplier.
    fn total(&self, r: &Rules) -> i64 {
        let damage: i64 = (self.records.iter().zip(&r.bones)).map(|(rec, bone)| rec.level.map_or(0, |k| bone.levels[k].0)).sum();
        let spins = |degrees: f32| ((degrees + 90.) / 360.).floor();
        // 0x82dad528: the four curves summed as floats, truncated once. Entity hits aren't scored (no signal): 0.
        let tweak_bonus = (r.tweak_hits.at(0.)
            + r.tweak_hold.at(self.held[1])
            + r.tweak_spins_3.at(spins(self.degrees[3]))
            + r.tweak_spins_0.at(spins(self.degrees[0]))) as i64
            + self.tweak_sum;
        damage
            + tweak_bonus
            + self.rotation_pts_max
            + self.speed_pts
            + self.drop_pts
            + self.air_pts
            + self.drop_pts_sum
            + self.air_pts_sum
            + self.bail_pts(r)
            + pts(&r.impact, self.impact_sum)
    }
}

/// 0x82dae6f0: no depth guard in retail; every stock cycle passes on less than it got. ponytail: the 32 only stops a
/// modded vault's full-share cycle from overflowing the stack.
fn spread(r: &Rules, records: &mut [Record; BONES], bone: usize, force: f32, from: usize, depth: u8) {
    let record = &mut records[bone];
    record.force += force;
    record.peak = record.peak.max(record.force);
    for &(link, share) in &r.bones[bone].links {
        let f = share * force;
        if link >= 0 && link as usize != from && f > SPREAD_MIN && depth < 32 {
            spread(r, records, link as usize, f, bone, depth + 1);
        }
    }
}

/// 0x82dac930 / 0x82dac880: one tracker's turn this frame; a reversal after more than 30 degrees banks them.
fn track(tracker: &mut Tracker, axis: usize, basis: [Vec3; 3]) -> f32 {
    let old = tracker.angle;
    let old_degrees = (old * DEG).abs();
    tracker.angle += step(axis, &tracker.rows, &basis);
    tracker.rows = basis;
    if (tracker.angle * DEG).abs() < old_degrees && old_degrees > 30. {
        tracker.angle -= old;
        return old_degrees;
    }
    0.
}

/// 0x82dac9d0: the new row projected into the plane normal to `u`, its angle from the old one about `u`, wrapped to
/// (-pi, pi]; more than 90 degrees in one frame is ignored.
fn step(axis: usize, old: &[Vec3; 3], new: &[Vec3; 3]) -> f32 {
    let (u, from, to) = match axis {
        0 => (old[1], old[2], new[2]),
        1 => (old[2], old[1], new[1]),
        _ => (old[0], old[1], new[1]),
    };
    let t = u.cross(to).normalize_or_zero().cross(u).normalize_or_zero();
    let cos = t.dot(from);
    if cos < 0. || t == Vec3::ZERO || from == Vec3::ZERO {
        return 0.;
    }
    // 0x8296ec98: acos in [0, 2pi), 2pi - a when the cross product points against u.
    let a = cos.min(1.).acos();
    if from.cross(t).dot(u) < 0. { -a } else { a }
}

/// What the panel does next, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Panel {
    /// The manager's show pulse (S2+14630): SetState 1.
    Show,
    /// The hide pulse (S2+14643): SetState 2.
    Hide,
    /// A publish (HOMScoringHUD feeder 0x82774828): SetState 3 with this data and the tweak completed this frame.
    Feed(Feed, Option<i64>),
}

// No body damage screen after a bail (user 10-17 #20, reverting feature #8): retail Skate 3 has none (no movie on
// the disc, TU3 or DLC; its Lua OpenHOMResults, native table 0x8230F304, is a stub `li r3,0; blr` at 0x82A248A0).
// The bones (Scorer::shown_bones) and homscoring stay.

/// The HUD data the natives answer (HOMScoringHUD +12..+56).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Feed {
    score: i64,
    /// [speed, drop, air time, bail time, rotation] points (HOM_GetBonusScores).
    points: [i64; 5],
    /// Bail speed m/s, best landed drop m, best landed air s, bail s, most rotation degrees (HOM_GetBonusData).
    metrics: [f32; 5],
}

/// Hall Of Meat Mode's per-tick state (on [`SkaterRuntime`]: physics feeds it mid-tick).
#[derive(Default)]
pub(crate) struct Scorer {
    rules: Option<Rules>,
    /// On for this tick ([`gate`]).
    pub enabled: bool,
    /// Scoring2+200: 1 for 30 frames after a new break, else 0 (the PhysOut reset zeroes it every frame).
    pub broken_bone_200: f32,
    stick: [f32; 2],
    /// The Fill's WipeoutGround trigger latch.
    trigger_latch: bool,
    /// The manager's selector: last tick's show condition, and the hide latch.
    three: bool,
    hidden: bool,
    /// The scorer's own +2228 (wiping), +2229 (tracking the air before a bail), +2236 (seconds since that air).
    wiping: bool,
    tracking: bool,
    since_air: f32,
    bail: Bail,
    feed: Feed,
    tweak_shown: bool,
    pub panel: Vec<Panel>,
    /// The vault's bone colours ([`Scorer::bone_colours`]).
    bone_colours: [Vec3; 2],
}

impl Scorer {
    pub(crate) fn load(data: &Collections) -> Self {
        // HOMSkaterPresEntity's colours (0x82762348): sprained / dislocated, then broken.
        let colour = |name: &str| data.words::<4>("Hash_1FFDC8E3ACA07C1F", "hall_of_meat_skeleton", name).map(|w| Vec3::new(f32::from_bits(w[0]), f32::from_bits(w[1]), f32::from_bits(w[2])));
        let bone_colours = [colour("Hash_F1C8F879829F12A0"), colour("Hash_9C9AB482CDC3CF48")];
        let bone_colours = bone_colours.map(|c| c.inspect_err(|e| warn!("Hall of Meat bones: {e}")).unwrap_or(Vec3::splat(0.8)));
        Self { rules: Rules::load(data).inspect_err(|e| warn!("Hall of Meat: {e}")).ok(), bone_colours, ..default() }
    }
    /// The bones the skater shows (hom_bones), per vault bone: 0 none, 1 sprained or dislocated, 2 broken. Retail
    /// 0x82763A40 compares each level against 2 and 4. Only during a bail (INFERRED: retail's gate is untraced).
    pub(crate) fn shown_bones(&self) -> Option<[u8; BONES]> {
        (self.enabled && self.wiping).then(|| self.bail.records.map(|r| r.level.map_or(0, |l| u8::from(l >= 2) + u8::from(l >= 4))))
    }
    pub(crate) fn bone_colours(&self) -> [Vec3; 2] {
        self.bone_colours
    }

    /// The last published bail score (the panel's HOM_GetScore).
    pub(crate) fn score(&self) -> i64 {
        self.feed.score
    }

    /// The scorer's own wiping (+2228) while on. Its fall posts cMsgWipeoutOver (0x82dae6a0 sets player+0x3936,
    /// 0x82df14a4 posts it): at the reset countdown or teleport, before the Wipeout category ends.
    pub(crate) fn wiping(&self) -> Option<bool> {
        self.enabled.then_some(self.wiping)
    }

    /// Off clears everything but the rules.
    fn set_enabled(&mut self, on: bool) {
        let on = on && self.rules.is_some();
        if self.enabled && !on {
            *self = Self { rules: self.rules.take(), ..default() };
        }
        self.enabled = on;
    }

    /// SimController Fill 0x82599e2c, from the last tick's mask, S2+200 and state and this tick's left trigger: 0.35x
    /// while a new break holds S2+200, and down to 0.5x with the trigger while the ragdoll is on the ground (after it
    /// was let go there). Bug 10-16 #21: online the Fill skips the whole block, the trigger latch included
    /// (0x82599E08: TheSimulation [0x830cfd94] vfunc 0 = 0x8285A870, the same flag Lua's IsOnline 0x82846248 reads).
    /// ponytail: its other gate (r22+4 vfunc 60, 0x825921B0, looks like "the sim's own player") is taken as passing:
    /// this runs only for the local skater.
    pub(crate) fn time_scale(&mut self, online: bool, capabilities: u32, state: u32, trigger: f32) -> f32 {
        if online {
            return 1.;
        }
        let mut scale = 1.;
        if capabilities & 0x10 != 0 {
            scale = (-self.broken_bone_200).mul_add(0.65, 1.);
        }
        if capabilities & 0x20 != 0 {
            if state == skate_core::player::state::PhysicalStateId::WipeoutGround as u32 {
                if trigger <= 0.2 {
                    self.trigger_latch = true;
                }
                if self.trigger_latch {
                    let s = 1. - (trigger - 0.2) * 1.25 * 0.5;
                    scale = if scale - s >= 0. { s } else { scale };
                }
            } else {
                self.trigger_latch = false;
            }
        }
        scale
    }

    /// One completed tick: the manager's panel selector, then the scorer (0x82dae4f0).
    fn step(&mut self, f: &Frame) {
        self.broken_bone_200 = 0.;
        let Some(r) = self.rules.take() else { return };
        // Manager: show when entering a wipeout or a requested teleport (reset to 0); hide once back on the ground, a
        // rail or the feet. ponytail: retail clears the hide latch only in the manager's reset; entering the show
        // condition again stands in for that (the report).
        let three = f.teleport || f.category == WIPEOUT;
        if three {
            if !self.three {
                self.feed.score = 0;
                self.panel.push(Panel::Show);
            }
            self.hidden = false;
        } else if matches!(f.category, GROUND | GRIND | OFFBOARD) && !self.hidden {
            self.hidden = true;
            self.panel.push(Panel::Hide);
        }
        self.three = three;

        let pre = matches!(f.category, AIR | OFFBOARD_AIR);
        let (was_wiping, was_tracking) = (self.wiping, self.tracking);
        self.wiping = f.category == WIPEOUT && !f.teleport && !f.countdown;
        self.since_air = if pre { 0. } else { self.since_air + DT };
        self.tracking = pre || (was_tracking && self.since_air < 0.1);
        let cold = !was_tracking && !was_wiping && self.wiping;
        if (!was_tracking && self.tracking) || cold {
            self.bail = Bail::new(f.basis);
            if cold {
                let b = &mut self.bail;
                (b.velocity, b.speed_pts, b.captured) = (f.velocity, pts(&r.speed, f.velocity.length()), true);
            }
        }
        if was_wiping && !self.wiping {
            self.publish(&r);
        }
        if self.wiping {
            self.wipe_frame(&r, f);
        } else {
            self.not_wiping(&r, f);
        }
        self.rules = Some(r);
    }

    /// 0x82daf1d8.
    fn wipe_frame(&mut self, r: &Rules, f: &Frame) {
        let b = &mut self.bail;
        let was_airborne = b.airborne;
        for record in &mut b.records {
            record.previous = record.level;
        }
        // 0x82dae9a8: airborne unless some bone rests on an up-facing world contact.
        b.airborne = true;
        for (bone, &(force, grounded)) in f.contacts.iter().enumerate() {
            let force = force * 2. * r.force_scale;
            if force > SPREAD_MIN {
                spread(r, &mut b.records, bone, force, bone, 0);
            }
            b.airborne &= !grounded;
        }
        for (record, bone) in b.records.iter_mut().zip(&r.bones) {
            record.level = (0..6).rev().find(|&k| bone.levels[k].1 > 0. && record.peak > bone.levels[k].1);
            record.force *= 0.8;
        }
        if b.airborne && !was_airborne {
            b.takeoff(f);
        } else if was_airborne && !b.airborne {
            b.air_update(r, f, self.stick);
            b.bank_air();
        }
        if b.airborne {
            b.air_update(r, f, self.stick);
        }
        b.trackers(f.basis, f.frozen || f.settled);
        b.totals(r, self.tracking);
        if !f.frozen && !f.settled {
            b.bail_frames += 1;
        }
        if f.impact > 0. {
            b.impact_sum += f.impact;
        }
        let breaks = b.records.iter().any(|rec| rec.level.is_some_and(|k| k >= 4) && !rec.previous.is_some_and(|k| k >= 4));
        b.break_hold = if breaks { 30 } else { b.break_hold.saturating_sub(1) };
        self.broken_bone_200 = if b.break_hold > 0 { 1. } else { 0. };
        self.publish(r);
    }

    /// 0x82daedc8: the air before a bail.
    fn not_wiping(&mut self, r: &Rules, f: &Frame) {
        if !self.tracking {
            return;
        }
        let b = &mut self.bail;
        let was_airborne = b.airborne;
        b.airborne = matches!(f.category, AIR | OFFBOARD_AIR);
        if b.airborne && !was_airborne {
            b.takeoff(f);
        } else if was_airborne && !b.airborne {
            b.air_update(r, f, self.stick);
            b.bank_air();
        }
        if b.airborne {
            b.air_update(r, f, self.stick);
            if !b.captured || f.velocity.length() > b.velocity.length() {
                b.velocity = f.velocity;
            }
            (b.speed_pts, b.captured) = (pts(&r.speed, b.velocity.length()), true);
        }
        b.trackers(f.basis, false);
        b.totals(r, self.tracking);
    }

    /// 0x82dadc48 and the feeder: the panel's data, and its tweak sticker once per completed tweak.
    fn publish(&mut self, r: &Rules) {
        let b = &self.bail;
        self.feed = Feed {
            score: b.total(r),
            points: [b.speed_pts, b.drop_pts_sum, b.air_pts_sum, b.bail_pts(r), b.rotation_pts_max],
            metrics: [b.velocity.length(), b.max_drop, b.max_air_frames as f32 * DT, b.bail_frames as f32 * DT, b.max_rotation],
        };
        let tweak = (b.tweak_done && !self.tweak_shown).then_some(b.tweak_now);
        self.tweak_shown = b.tweak_done;
        match self.panel.last_mut() {
            Some(Panel::Feed(last, None)) => *last = self.feed,
            _ => self.panel.push(Panel::Feed(self.feed, None)),
        }
        if let (Some(t), Some(Panel::Feed(_, slot))) = (tweak, self.panel.last_mut()) {
            *slot = Some(t);
        }
    }
}

/// Runs the scorer on the completed tick (from publish_feedback, before the camera reads S2+200).
pub(crate) fn score(skater: &mut SkaterRuntime) {
    if !skater.hall_of_meat.enabled {
        skater.hall_of_meat.broken_bone_200 = 0.;
        return;
    }
    let frame = Frame::read(skater);
    skater.hall_of_meat.step(&frame);
}

// ---------------------------------------------------------------- systems
/// The game whose free-skate Hall of Meat runs now: 0 = none (a match, a lesson, or Skate 3 with the toggle off), else
/// the map's game as Settings > HUD Style picks it (skate2_hud::hud_game).
pub(crate) fn free_game(world: &World) -> u8 {
    let menu = world.get_resource::<crate::graphics_menu::Menu>();
    let idle = world.get_resource::<crate::match_core::Match>().is_none_or(|m| m.mode.is_none() && m.phase == crate::match_core::Phase::Idle);
    let lesson = world.get_resource::<crate::challenge::LessonMenu>().is_some_and(|l| l.active);
    match world.get_resource::<crate::map_transition::CurrentMap>() {
        Some(map) if idle && !lesson => match crate::skate2_hud::hud_game(menu, map) {
            // Only Skate 3 has the setting; Skate 2's free roam shows its panel with no option (IsHOMAvailable 0x82313b18).
            3 if !menu.is_some_and(|m| m.hall_of_meat()) => 0,
            game => game,
        },
        _ => 0,
    }
}

pub(crate) struct HallOfMeatPlugin;
impl Plugin for HallOfMeatPlugin {
    fn build(&self, app: &mut App) {
        use crate::app::SimulationSet;
        // In the physics set, so it shares the tick's run conditions (Start menu, replay, map load): a request queued
        // while no tick drains it piled up every FixedUpdate the online Start menu stayed open.
        app.add_systems(FixedUpdate, gate.in_set(SimulationSet::Physics).before(crate::physics::advance))
            .add_systems(Update, panel.after(crate::app::FrameSet::Physics));
    }
}

/// Whether this tick's `physics::advance` skips the engine's sim, so nothing drains the camera's rate queue: physics
/// failed, a vehicle is occupied, or Skate 2 physics has the local skater (Task 16).
pub(crate) fn physics_skipped(world: &World) -> bool {
    world.get_resource::<crate::physics::GamePhysics>().is_none_or(|p| p.failed)
        || world.get_resource::<crate::modding::vehicles::Vehicles>().is_some_and(|v| v.occupied())
        || !crate::skate2_physics::step::engine_runs_local(world.get_resource::<crate::skate2_physics::Skate2Physics>())
}

/// Before each tick: on or off, the tweak stick, and the Fill's slow-motion request, dispatched at the tick's start
/// ahead of the camera's own (SimController 0x8285C968).
fn gate(world: &mut World) {
    // Also on through the local skater's Skate 3 HOM Battle turn (skate3_uniform).
    let on = free_game(world) == 3 || world.get_resource::<crate::skate3_uniform::UniformGame>().is_some_and(|g| g.hom_live);
    let skips = physics_skipped(world);
    let online = world.get_resource::<crate::multiplayer::Multiplayer>().is_some_and(|n| n.active());
    let Some(words) = world.get_resource::<PlayerControls>().map(|c| *c.controller.words()) else { return };
    let Some(mut skater) = world.get_resource_mut::<SkaterRuntime>() else { return };
    let caps = skater.player_input.physical.scoring.capabilities_204;
    let state = skater.player_input.physical.state.state_16;
    let hom = &mut skater.hall_of_meat;
    hom.set_enabled(on);
    hom.stick = [f32::from_bits(words[7]), f32::from_bits(words[8])];
    let scale = hom.time_scale(online, caps, state, f32::from_bits(words[11]));
    // The queue is drained only by a tick that runs: don't feed it while physics is skipped.
    if let Some(mut camera) = world.get_resource_mut::<crate::camera::CameraRuntime>()
        && scale < 1.
        && !skips
    {
        let request = SimulationRateRequest { timestep: f32::from_bits(0x3c88_8889) / scale, ticks: 1 };
        camera.simulation_rate_requests.insert(0, request);
    }
}

/// The score panel: kept loaded while the mode is on (no load hitch at a bail); hidden with Settings > HUD.
fn panel(
    skater: Option<ResMut<SkaterRuntime>>,
    screens: Option<ResMut<Screens>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    let Some(mut skater) = skater else { return };
    let hom = &mut skater.hall_of_meat;
    let events = std::mem::take(&mut hom.panel);
    let Some(mut screens) = screens else { return };
    let layer = screens.layer(ONLINE);
    if !hom.enabled || menu.is_some_and(|m| !m.hud_visible()) {
        layer.close(NAME);
        return;
    }
    let result = (|| -> Result<(), String> {
        if layer.get(NAME).is_none() {
            time_unit(layer.open(NAME, "homscoring", &[])?);
        }
        let screen = layer.get(NAME).ok_or("homscoring closed")?;
        for event in events {
            match event {
                Panel::Show => {
                    screen.call("SetState", &[json!(1)])?;
                }
                Panel::Hide => {
                    screen.call("SetState", &[json!(2)])?;
                }
                Panel::Feed(feed, tweak) => {
                    let text = |id: &str| screen.bindings.movie.text_assets.localize(id);
                    for (key, value) in answers(&feed, tweak, crate::hud_runtime::imperial(), text) {
                        screen.answer(&key, value);
                    }
                    screen.call("SetState", &[json!(3)])?;
                }
            }
        }
        screen.take_calls();
        Ok(())
    })();
    if let Err(error) = result {
        warn!("Hall of Meat: homscoring: {error}");
        layer.close(NAME);
    }
}

/// The "s" UpdateScoring puts after each time: the movie's own ID_COMMON_METRIC_TIME.
fn time_unit(screen: &mut crate::apt_screen::Screen) {
    let s = screen.bindings.movie.text_assets.localize("ID_COMMON_METRIC_TIME");
    screen.answer("FELanguage.GetLocaleText(ID_COMMON_METRIC_TIME)", json!(s));
}

/// What homscoring's UpdateScoring asks (HUDComponents.HOM_*; natives 0x8265fe70..0x826601e8). Speed and drop come as
/// the text the exe's {0:speed} / {0:distance} make of them (the movie's ID_HOM_SPEED_VALUE / ID_COMMON_METRIC_METERS
/// are '%s' in the port), with Settings > Units and the movie's unit text. ponytail: one place. A placeholder without
/// ".N" keeps its slot's last precision (0x8290DA38 writes the global table 0x830BCCC8, 3 at boot); slot 0's is 1 after
/// any big-trick text (TrickDisplay_GetAllTrickData's "{0:distance.1}" / "{0:f.1} s").
fn answers(feed: &Feed, tweak: Option<i64>, imperial: bool, text: impl Fn(&str) -> String) -> Vec<(String, Json)> {
    use crate::hud_runtime::{distance, speed};
    let [mps, drop, air, bail, rotation] = feed.metrics;
    let time = |s: f32| format!("{s:.2}");
    let (speed, drop) = (speed(mps, 1, imperial, &text), distance(drop, 1, imperial, &text));
    let mut answers = vec![
        ("HUDComponents.HOM_GetScore".to_owned(), json!(feed.score)),
        (
            "HUDComponents.HOM_GetBonusData".to_owned(),
            json!([speed, drop, time(air), time(bail), rotation, false, false, false, false, false]),
        ),
        ("HUDComponents.HOM_GetBonusScores".to_owned(), json!(feed.points)),
        // Cars, objects, pedestrians and skaters hit aren't scored (no signal): only the body tweak.
        ("HUDComponents.HOM_GetBonusCollisionData".to_owned(), json!([0, 0, tweak.is_some(), 0, 0])),
        ("HUDComponents.HOM_GetBonusCollisionScore".to_owned(), json!([[" "], [" "], tweak.unwrap_or(0), [" "], [" "]])),
    ];
    for t in [time(air), time(bail)] {
        answers.push((format!("FELanguage.GetLocalizedTime({t})"), json!(format!("#{t}"))));
    }
    answers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(x: [f32; 8], y: [f32; 8]) -> Curve {
        Curve { x, y }
    }
    fn flat() -> Curve {
        curve([0., 1., 2., 3., 4., 5., 6., 7.], [0.; 8])
    }
    fn none() -> Bone {
        Bone { links: [(-1, 0.); 4], levels: [(0, 0.); 6] }
    }

    /// Bone 0 passes half to bone 1, bone 1 a quarter back (never back where it came from).
    fn rules() -> Rules {
        let mut bones: Vec<Bone> = (0..BONES).map(|_| none()).collect();
        bones[0] = Bone {
            links: [(1, 0.5), (-1, 0.), (-1, 0.), (-1, 0.)],
            levels: [(100, 5.), (200, 10.), (0, 0.), (0, 0.), (1000, 30.), (0, 0.)],
        };
        bones[1] = Bone { links: [(0, 0.25), (-1, 0.), (-1, 0.), (-1, 0.)], levels: [(50, 2.), (0, 0.), (0, 0.), (0, 0.), (0, 0.), (0, 0.)] };
        Rules {
            bones,
            tweak_hold: flat(),
            impact: flat(),
            rotation_tracked: flat(),
            tweak_hits: flat(),
            air_time: flat(),
            drop: flat(),
            tweak_spins_0: flat(),
            tweak_spins_3: flat(),
            speed: flat(),
            rotation: flat(),
            bail_time: flat(),
            force_scale: 0.75,
            tweak_stick: 0.6,
            tweaks: vec![Tweak { id: 0, stick: [0., 1.], frames: 15, points: 500 }],
        }
    }
    fn scorer(rules: Rules) -> Scorer {
        Scorer { rules: Some(rules), enabled: true, ..default() }
    }
    fn frame(category: u32) -> Frame {
        Frame {
            category,
            countdown: false,
            teleport: false,
            frozen: false,
            velocity: Vec3::ZERO,
            height: 0.,
            settled: false,
            impact: 0.,
            contacts: [(0., true); BONES],
            basis: [Vec3::Y, Vec3::X, Vec3::Z],
        }
    }

    #[test]
    fn the_wipe_is_over_at_the_reset_countdown_not_the_category_end() {
        let mut s = scorer(rules());
        let mut f = frame(WIPEOUT);
        s.step(&f);
        assert_eq!(s.wiping(), Some(true));
        f.countdown = true;
        s.step(&f);
        assert_eq!(s.wiping(), Some(false), "over while the category is still Wipeout");
        s.enabled = false;
        assert_eq!(s.wiping(), None, "off: the caller keeps the category's edge");
    }

    #[test]
    fn a_scored_bail_shows_the_bones_and_only_homscoring() {
        let mut s = scorer(rules());
        let mut f = frame(WIPEOUT);
        f.contacts[0].0 = 10.;
        for _ in 0..3 {
            s.step(&f);
        }
        // The skater shows the broken skull (BreakPart) and not the scraped neck.
        assert_eq!(s.shown_bones().map(|b| (b[0], b[1])), Some((2, 0)));
        s.step(&frame(0));
        assert_eq!(s.shown_bones(), None, "only during the bail");
        // Bug 10-17 #20: the bail's score goes to homscoring only; no body damage screen follows.
        assert!(s.feed.score > 0);
        assert!(s.panel.iter().any(|p| matches!(p, Panel::Feed(..))), "{:?}", s.panel);
    }

    #[test]
    fn damage_decays_and_grades_on_the_peak() {
        let mut s = scorer(rules());
        let mut f = frame(WIPEOUT);
        f.contacts[0].0 = 10.; // x2 x0.75 = 15 into bone 0, 7.5 on to bone 1
        s.step(&f);
        let r = &s.bail.records;
        assert_eq!((r[0].peak, r[1].peak), (15., 7.5));
        assert_eq!((r[0].level, r[1].level), (Some(1), Some(0)), "Bruise (10) and Scrape (2)");
        assert_eq!(r[0].force, 15. * 0.8, "the accumulator decays after grading");
        assert_eq!(s.panel, vec![Panel::Show, Panel::Feed(s.feed, None)]);
        assert_eq!(s.feed.score, 200 + 50);
        // A second hit stacks on the decayed accumulator: 12 + 15 = 27, under BreakPart (30).
        s.step(&f);
        assert_eq!(s.bail.records[0].peak, 27.);
        assert_eq!(s.broken_bone_200, 0.);
        // 27 * 0.8 + 15 = 36.6 passes it: a new break holds S2+200 for 30 frames.
        s.step(&f);
        assert_eq!(s.bail.records[0].level, Some(4));
        assert_eq!(s.broken_bone_200, 1.);
        assert_eq!(s.feed.score, 1000 + 50);
        f.contacts[0].0 = 0.;
        for _ in 0..29 {
            s.step(&f);
        }
        assert_eq!(s.broken_bone_200, 1.);
        s.step(&f);
        assert_eq!(s.broken_bone_200, 0.);
        assert_eq!(s.feed.score, 1000 + 50, "levels come from the peak, which stays");
    }

    #[test]
    fn the_air_before_a_bail_counts_and_the_panel_hides_back_on_the_ground() {
        let mut rules = rules();
        rules.air_time = curve([0., 0.5, 0.5, 10., 11., 12., 13., 14.], [0., 0., 100., 100., 100., 100., 100., 100.]);
        rules.speed = curve([0., 5., 10., 20., 30., 40., 50., 60.], [0., 0., 750., 1000., 1000., 1000., 1000., 1000.]);
        let mut s = scorer(rules);
        let mut f = frame(AIR);
        f.velocity = Vec3::new(12., 0., 0.);
        for _ in 0..60 {
            s.step(&f);
        }
        assert!(s.panel.is_empty(), "the air alone shows nothing");
        assert_eq!(s.bail.speed_pts, 800, "the air's top speed: 750 + 250 * 2 / 10");
        // The bail from that air keeps it: airborne ragdoll (no grounded bone), then down.
        f.category = WIPEOUT;
        f.contacts = [(0., false); BONES];
        s.step(&f);
        assert_eq!(s.bail.air_frames, 61, "the takeoff came before the bail");
        f.contacts = [(0., true); BONES];
        s.step(&f);
        assert_eq!((s.bail.air_pts_sum, s.bail.max_air_frames), (100, 62), "landing banks the air");
        assert_eq!(s.feed.points, [800, 0, 100, 0, 0]);
        assert_eq!(s.feed.score, 800 + 100);
        // The respawn countdown ends the wipe (final publish); the panel stays up until the ground.
        f.countdown = true;
        s.step(&f);
        assert!(!s.wiping);
        f = frame(5);
        s.step(&f);
        assert!(!matches!(s.panel.last(), Some(Panel::Hide)));
        s.step(&frame(GROUND));
        assert_eq!(s.panel.first(), Some(&Panel::Show));
        assert_eq!(s.panel.last(), Some(&Panel::Hide));
        s.panel.clear();
        s.step(&frame(GROUND));
        assert!(s.panel.is_empty(), "the hide latch holds");
    }

    #[test]
    fn tweaks_score_once_per_hold_while_airborne() {
        let mut s = scorer(rules());
        s.stick = [0., 1.];
        let mut f = frame(WIPEOUT);
        f.contacts = [(0., false); BONES];
        for _ in 0..17 {
            s.step(&f);
        }
        assert_eq!((s.bail.tweak_sum, s.bail.tweak_frames), (500, 16), "the award lands on the 16th frame held");
        let tweaks: Vec<_> = s.panel.iter().filter_map(|p| if let Panel::Feed(_, t) = p { *t } else { None }).collect();
        assert_eq!(tweaks, vec![500]);
        for _ in 0..30 {
            s.step(&f);
        }
        assert_eq!(s.bail.tweak_sum, 500);
    }

    #[test]
    fn trackers_bank_a_reversal_past_30_degrees() {
        let mut tracker = Tracker { rows: [Vec3::Y, Vec3::X, Vec3::Z], angle: 0. };
        let about_y = |deg: f32| {
            let q = Quat::from_rotation_y(deg.to_radians());
            [q * Vec3::Y, q * Vec3::X, q * Vec3::Z]
        };
        let mut banked = 0.;
        for d in (5..=40).step_by(5) {
            banked += track(&mut tracker, 2, about_y(d as f32));
        }
        assert!(((tracker.angle * DEG).abs() - 40.).abs() < 1e-3 && banked == 0.);
        banked += track(&mut tracker, 2, about_y(35.));
        assert!((banked - 40.).abs() < 1e-3, "{banked}");
        assert!(((tracker.angle * DEG).abs() - 5.).abs() < 1e-3);
    }

    /// The Fill's slow-motion request is drained only by a physics tick. Online the Start menu stops the simulation
    /// sets (gameplay_active) but not FixedUpdate: a gate outside them queued one request per tick (Vec::insert(0, ..),
    /// O(n)) for as long as the menu stayed open over a slow-motion bail, so the frame rate fell the longer it stayed.
    #[test]
    fn slow_motion_gate_runs_only_with_the_simulation() {
        use crate::app::SimulationSet;
        use bevy::ecs::schedule::{ScheduleLabel, SystemSet};
        let mut app = App::new();
        app.add_plugins(HallOfMeatPlugin)
            .configure_sets(FixedUpdate, (SimulationSet::Input, SimulationSet::Controls, SimulationSet::Physics).chain());
        let world = app.world_mut();
        let mut schedule = world.resource_mut::<Schedules>().remove(FixedUpdate.intern()).unwrap();
        schedule.initialize(world).unwrap();
        let gated = schedule.graph().systems_in_set(SimulationSet::Physics.intern()).unwrap().clone();
        let gate = schedule.systems().unwrap().find(|(_, s)| s.name().to_string().ends_with("hall_of_meat::gate")).map(|(k, _)| k);
        assert!(gate.is_some_and(|k| gated.contains(&k)), "the slow-motion gate must share the physics tick's run conditions");
    }

    #[test]
    fn slow_motion_follows_the_fill() {
        let mut s = scorer(rules());
        let ground = skate_core::player::state::PhysicalStateId::WipeoutGround as u32;
        assert_eq!(s.time_scale(false, 0x1c0, ground, 1.), 1., "no HoM bits");
        s.broken_bone_200 = 1.;
        assert!((s.time_scale(false, 0xffff_f7f7, 0, 0.) - 0.35).abs() < 1e-6);
        assert_eq!(s.time_scale(true, 0xffff_f7f7, 0, 0.), 1., "online: no slow-motion (0x82599E08)");
        s.broken_bone_200 = 0.;
        assert_eq!(s.time_scale(false, 0xffff_f7f7, ground, 1.), 1., "the trigger must be let go first");
        assert_eq!(s.time_scale(false, 0xffff_f7f7, ground, 0.), 1.);
        // Online leaves the latch alone (retail skips its stb 0x92): an online tick off the ground doesn't drop it.
        assert_eq!(s.time_scale(true, 0xffff_f7f7, 0, 1.), 1.);
        assert!((s.time_scale(false, 0xffff_f7f7, ground, 1.) - 0.5).abs() < 1e-6);
        assert_eq!(s.time_scale(false, 0xffff_f7f7, 0, 1.), 1., "off the ground the latch drops");
        assert_eq!(s.time_scale(false, 0xffff_f7f7, ground, 1.), 1.);
    }

    /// The installed stock row (SKATE3_ASSET_ROOT; these tests are not in CI) and Skate 3's homscoring, run with every
    /// native it asks answered.
    #[test]
    fn installed_rules_and_homscoring() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let root = std::path::PathBuf::from(assets);
        let r = Rules::load(&Collections::load(&root).unwrap()).unwrap();
        assert_eq!((r.force_scale, r.tweak_stick, r.tweaks.len()), (0.75, 0.6, 4));
        assert_eq!(pts(&r.speed, 10.), 750);
        assert_eq!(r.bones[24].links, [(-1, 0.), (-1, 0.), (22, 1.), (-1, 0.)], "GROIN passes all to HIPS");
        let mut screen = crate::apt_screen::Screen::load(&root.join("private/online-ui/runtime/homscoring.json"), &[]).unwrap();
        time_unit(&mut screen);
        assert_eq!(screen.bindings.movie.text_assets.localize("ID_COMMON_METRIC_TIME"), "s");
        screen.call("SetState", &[json!(1)]).unwrap();
        let feed = Feed { score: 15250, points: [750, 1250, 2500, 750, 1000], metrics: [11.2, 9.4, 2.53, 5.1, 812.] };
        // Speed and drop in the movie's own unit text, Settings > Units both ways.
        let stats = |imperial| {
            let answers = answers(&feed, None, imperial, |id| screen.bindings.movie.text_assets.localize(id));
            let data = answers.into_iter().find(|(k, _)| k == "HUDComponents.HOM_GetBonusData").unwrap().1;
            (data[0].clone(), data[1].clone())
        };
        assert_eq!(stats(false), (json!("40.3 Km/h"), json!("9.4 m")));
        assert_eq!(stats(true), (json!("25.2 MPH"), json!("28.2 ft")));
        for tweak in [Some(500), None] {
            for (key, value) in answers(&feed, tweak, false, |id| screen.bindings.movie.text_assets.localize(id)) {
                screen.answer(&key, value);
            }
            screen.call("SetState", &[json!(3)]).unwrap();
        }
        for _ in 0..90 {
            screen.advance().unwrap();
        }
        assert!(!screen.draws().unwrap().is_empty());
        assert!(screen.bindings.unanswered.is_empty(), "homscoring asks {:?}", screen.bindings.unanswered);
        if let Ok(dir) = std::env::var("HOM_SNAPS") {
            let png = std::path::PathBuf::from(dir).join("homscoring3.png");
            crate::core_menu_runtime::tests::raster_draws(&screen.draws().unwrap(), &root.join("private/online-ui"), &png);
        }
    }
}
