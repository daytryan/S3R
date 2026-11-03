//! Skate 2's Hall of Meat on New_San_Vanelona, on the match core (`match_core.rs`; spec:
//! docs/superpowers/specs/2026-10-02-skate2-online-modes-plan.md). Data: skate2port/old_hall_of_meat.py, installed at
//! `private/skate2-online/hall-of-meat/` (hom.json, the screens, the intro cameras, sfx, indicator/). Also free skate's
//! bail panel on Skate 2 maps, always on as Skate 2 had no setting ([`free_skate`]; Skate 3's own is hall_of_meat.rs).
//!
//! The rules around a bail are the disc's `hall_of_meat` Lua template (scripts/hall_of_meat.lua), followed here:
//! - Intro: the row's NISFlythroughDefinition camera (tutorial.json + scene/camera, played by challenge::cutscene),
//!   held with `Match::intro`.
//! - Countdown: "<name>, your turn!" + "Round n" (CentralHUD) and the 3-2-1 (CountdownHUD).
//! - Each of NumRounds attempts: a TimeLimit timer (TimerHUD) and the row's neon indicators. A bail stops the timer and
//!   scores; running out of time ("Out of time!") leaves 2 s in which a bail that is over by then still scores, except
//!   on the match's last run (Skate 3's rule, user 10-28).
//! - wipeoutalmostover fades the screen out over TimeToTeleport; wipeoutover scores the attempt; the best attempt is
//!   the player's score (`Match::score`). Then back to the attempt spawn for the next round.
//! - Online it is turn-based (ActivePlayerID / PlayerOrderMap): each round every player takes one turn in slot order
//!   from a first player drawn at random (Skate 3's rule, user 10-28; [`schedule`]). The active player is repositioned on the attempt spawn and pinned through the countdown, then
//!   unpinned for its attempt; the others wait pinned on their TurnBasedWaitingLocation slots, watching it. Its game
//!   sends [`TurnOver`] when its turn ends and every game moves on.
//! - Online with 3+ players the waiting players vote on each attempt (hudonlinevoting; setup 162, challenge_running 12,
//!   end_turn 684, votingcomplete 706-731): each [`Vote`] goes to every game, the attempter's game ends the vote with its
//!   turn and relays the mean in its TurnOver, and a mean over 1 that beats the best so far is the Player's Choice
//!   (`Run::best_voted`; it never changes scores or ranks).
//!
//! The bail score itself is C++ in Skate 2 (Score::HoM::WipeoutScorer: 0x82bb74d8 at a wipeout's start, 0x82bb8398
//! every 60 Hz frame of it, both driven by 0x82337f88). [`Bail`] is its 1:1 port over the vault row in hom.json (every
//! rule with its address: skate2port/HOM_S2_SCORER_REPORT.md), fed every physics tick from the engine ([`Frame::read`]):
//! - wiping: the category (`filtered_state_0`) is Wipeout (4);
//! - per scorer bone (bone b reads collision part b + 1; FACE part 1's and GROIN part 23's specific contact): |Δv · n|,
//!   the part's velocity change (`skeleton.record.velocity_changes`) along its contact normal, and whether it rests on
//!   an up-facing world contact (group 3, n.y > 0.33); the body is in the air while no bone does;
//! - speed at the bail's start: the reckoning velocity (`reckoning.vector_16`); air and drop: its height (`vector_64`);
//! - body rotation: three trackers on the hips' basis rows (`skeleton.record.pose[23]`); bail time: frames until the
//!   wipeout settles (`skeleton.over_599`); body tweaks: the left stick (derived controller words 7/8).
//! The match's fade: `teleport_pending` / `time_until_teleport` (the engine's TeleportAutoResetFadeOutTime countdown).
//!
//! The HUD is Skate 2's own movies: homscoring (the score panel, its metric icons and the tweak sticker; layer
//! [`HALL_OF_MEAT`]) and leaderboardhud on this game's layer, and the tutorial port's hudcountdown / hudtimer /
//! hudcentral ([`TutorialScreens`]).
use crate::{
    apt_screen::{HALL_OF_MEAT, Screens},
    challenge::cutscene::Cutscene,
    config::Config,
    match_core::{Match, Mode, Phase, Playable},
    physics::{PlayerControls, SkaterRuntime},
    skate2_contests::{MOVE, REPOSITION_FADE, reposition_black, teleport},
    skate2_screens::{Central, TutorialScreens, View},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use skate_core::physics::reciprocal_square_root_estimate;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    f32::consts::{PI, TAU},
    path::PathBuf,
    sync::Arc,
};

const ROOT: &str = "private/skate2-online/hall-of-meat";
/// Every row's map.
const MAP: &str = "New_San_Vanelona";
/// Bones in the scorer's record order (hom.json `rules.bones`).
const BONES: usize = 25;
/// One 60 Hz frame (0x82060140, 0x3C888889: the scorer's frames-to-seconds factor; also the engine's tick).
const DT: f32 = 1. / 60.;
/// The skater's Wipeout category (`filtered_state_0`; 0x82337f88 tests 4).
const WIPEOUT: u32 = 4;
/// Countdown between attempts (the template's 3 s CountdownTimer).
const COUNTDOWN: f32 = 3.;
/// After a timed-out attempt, before the next one (end_turn's 2 s; 0.01 s after a wipeout, whose fade and score panel
/// already took their time).
const BETWEEN: f32 = 2.;
/// Skate 3's rule (user 10-28): EndRunTimerID, the turn running on after its time is up (S3 hall_of_meat.lua 887-901).
const END_RUN: f32 = 2.;
/// challenge_finished's 2 s ("Challenge Complete") before the results.
const ENDING: f32 = 2.;
/// How long a central message stays (hudcentral animates itself out).
const MESSAGE: f32 = 2.;
/// The match's time cap per attempt beyond its TimeLimit (countdown, the bail, the fade, the score panel). A knob.
const ATTEMPT_OVERHEAD: f32 = 20.;
/// The score panel refreshes this often while the score climbs (each refresh can play its scroll sound).
const PANEL_EVERY: f32 = 0.1;
const SCORING: &str = "1-homscoring";
const LEADERBOARD: &str = "2-leaderboard";

// ---------------------------------------------------------------- data (hom.json)
#[derive(Deserialize)]
pub(crate) struct Data {
    pub rules: Rules,
    rows: HashMap<String, Row>,
    strings: HashMap<String, String>,
}
/// The skater vault's Sk8::Score::HoM row (exact f32s; old_hall_of_meat.py `rules`).
#[derive(Deserialize)]
pub(crate) struct Rules {
    bones: Vec<Bone>,
    curves: Curves,
    force_scale: f32,
    toe_share: f32,
    tweak_stick: f32,
    tweaks: Vec<Tweak>,
    /// The vehicle scorer's price list (0x82bb8be0): (HoM type, points), else vehicle_points.
    vehicle_points: u32,
    vehicles: Vec<(i32, u32)>,
}
#[derive(Deserialize)]
struct Bone {
    /// (bone, share of the force) it passes on, in vault order (its -1 slots are not exported).
    links: Vec<(usize, f32)>,
    /// Scrape, Bruise, Sprain, Dislocate, BreakPart, BreakAll: (points, force threshold; 0 = no such level).
    levels: Vec<(u32, f32)>,
}
/// Sk8::PointNegGraphData8: eight points, evaluated by 0x8238da38.
#[derive(Deserialize, Clone, Copy)]
pub(crate) struct Curve {
    pub(crate) x: [f32; 8],
    pub(crate) y: [f32; 8],
}
#[derive(Deserialize)]
struct Curves {
    leg_impact: Curve,
    air_time: Curve,
    drop: Curve,
    speed: Curve,
    rotation: Curve,
    bail_time: Curve,
}
#[derive(Deserialize)]
struct Tweak {
    id: i32,
    stick: [f32; 2],
    frames: u32,
    points: u32,
}
#[derive(Deserialize)]
struct Row {
    title: Option<String>,
    time_limit: f32,
    num_rounds: u32,
    attempt: Option<Place>,
    /// TurnBasedWaitingLocation: online, the players not on their turn wait (and watch) here, each on its slot.
    waiting: Option<Place>,
    intro: Option<String>,
    intro_seconds: Option<f32>,
    #[serde(default)]
    indicators: Vec<Place>,
}
#[derive(Deserialize, Clone)]
struct Place {
    locator: String,
    matrix: [[f32; 4]; 4],
    #[serde(default)]
    slots: Vec<[[f32; 4]; 4]>,
}
impl Place {
    /// AI.RepositionAI's spot for slot `i` (online_map::locator_slot).
    fn slot(&self, i: usize) -> [[f32; 4]; 4] {
        crate::online_map::locator_slot(&self.matrix, &self.slots, i)
    }
}

/// The turn after `k` (inclusive) that someone still here takes: (turn, round, player). Turns go round by round, each
/// round in slot order from `first` (hall_of_meat.lua's PlayerOrderMap / CurrentPlayerIndex, skipping the
/// disconnected): turn k is player k % n, the match's turns are first .. first + n x rounds. Skate 3's rule (user
/// 10-28): `first` is drawn at random and every round starts again from it (S3 hall_of_meat.lua 143-144, 263).
fn schedule(k: usize, first: usize, players: usize, rounds: u32, gone: impl Fn(usize) -> bool) -> Option<(usize, u32, usize)> {
    let n = players.max(1);
    (k.max(first)..first + n * rounds as usize).find(|&k| !gone(k % n)).map(|k| (k, ((k - first) / n) as u32 + 1, k % n))
}

/// Online: the active player's turn is over (its `@ev` record; every game then moves on to the next turn).
#[derive(Serialize, Deserialize)]
struct TurnOver {
    hom_turn: usize,
    /// The attempter's best bail so far (its leaderboard score), for every game's results order ([`placing`]).
    #[serde(default)]
    best: i64,
    /// The turn's mean vote (Online.StopVoting on the attempter's game, 3+ players), relayed so every game names the
    /// same Player's Choice. Not retail (user 10-12, as skate3_uniform): retail's VotingManager sync is native, untraced.
    #[serde(default)]
    voted: Option<f32>,
}

/// Online: a waiting player's vote on turn `hom_vote`'s attempt (hudonlinevoting: 0 weak, 1 ok, 2 sweet, 3 gnarly).
#[derive(Serialize, Deserialize)]
struct Vote {
    hom_vote: usize,
    level: u8,
}

/// Online, the results order ([`Match::order`]): each player's best bail as the turns' TurnOver records gave it, in
/// turn order, a tie keeping the earlier scorer (hall_of_meat.lua 72-84: strict `>`, as `promote`); the players who
/// left (`left`, in the order they left: `Match::left`, one order on every console) last at -1, the later leaver above
/// the earlier (playerdisconnected 558-580 moves a leaver down past every Score > -1 only).
fn placing(players: usize, turns: &BTreeMap<usize, i64>, left: &[usize]) -> Vec<(usize, i64)> {
    let mut order: Vec<usize> = (0..players).collect();
    let mut best = vec![0i64; players];
    for (&k, &b) in turns {
        let p = k % players;
        if b > best[p] {
            best[p] = b;
            // ponytail: promote compares f32; bail scores stay far under 2^24.
            crate::skate2_contests::promote(&mut order, |i| best[i] as f32, p);
        }
    }
    order.retain(|p| !left.contains(p));
    order.into_iter().map(|p| (p, best[p])).chain(left.iter().rev().map(|&p| (p, -1))).collect()
}

impl Data {
    fn load(root: &std::path::Path) -> Result<Self, String> {
        let path = root.join("hom.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let data: Self = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let bones = &data.rules.bones;
        if bones.len() != BONES || bones.iter().any(|b| b.levels.len() != 6 || b.links.iter().any(|&(l, _)| l >= BONES)) {
            return Err(format!("{}: expected {BONES} bones of 6 levels", path.display()));
        }
        Ok(data)
    }
    fn text(&self, id: &str) -> String {
        self.strings.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }
}

impl Curve {
    /// 0x8238da38 (Skate 3's 0x82481e10 is the same): below the first x the first y, from the last x (or NaN) the last
    /// y, else the first segment ending past `v`, linear by one fused multiply-add (a zero-width one takes its upper y).
    pub(crate) fn at(&self, v: f32) -> f32 {
        if v < self.x[0] {
            return self.y[0];
        }
        if !(v < self.x[7]) {
            return self.y[7];
        }
        (1..8).find(|&i| v < self.x[i]).map_or(self.y[0], |i| {
            let width = self.x[i] - self.x[i - 1];
            if width > 0. { ((self.y[i] - self.y[i - 1]) / width).mul_add(v - self.x[i - 1], self.y[i - 1]) } else { self.y[i] }
        })
    }
    /// The scorer's whole points: fctidz then the low 32 bits.
    fn points(&self, v: f32) -> u32 {
        self.at(v) as i64 as u32
    }
}

// ---------------------------------------------------------------- the bail scorer
type V = [f32; 4];
/// 0x8205fe28 (57.29578).
const DEGREES: f32 = 57.295_78;

/// One frame of a wipeout as the scorer reads it (0x82bb8398's inputs).
#[derive(Clone, Copy, Default)]
pub(crate) struct Frame {
    /// The reckoning velocity (scorer object +36 +16), read at the bail's start.
    pub velocity: V,
    /// The reckoning height (+36 +48 .y).
    pub height: f32,
    /// Per scorer bone: (|Δv · n|, not resting on an up-facing world contact) (0x82bb7f48 over 0x8238c110's records).
    pub contacts: [(f32, bool); BONES],
    /// The hips' basis rows as the trackers keep them (+20 +96, +48, +80, +64).
    pub rows: [V; 4],
    /// The wipeout has settled (+20 +579).
    pub settled: bool,
    /// The left stick (+48 +116/+120).
    pub stick: [f32; 2],
    /// The cars hit this frame within 8 m (0x82322018's list, flagged by veh4368): (id, HoM type). traffic.rs.
    pub vehicles: [Option<(u64, i32)>; 8],
}

impl Frame {
    fn read(skater: &SkaterRuntime, controls: &PlayerControls, vehicles: [Option<(u64, i32)>; 8]) -> Self {
        let p = &skater.player_input.physical;
        let record = &skater.skeleton.record;
        let hips = record.pose[23];
        let words = controls.controller.words();
        Self {
            velocity: p.reckoning.vector_16.map(f32::from_bits),
            height: f32::from_bits(p.reckoning.vector_64[1]),
            contacts: std::array::from_fn(|b| {
                let part = match b {
                    23 => 1,
                    24 => 23,
                    b => b + 1,
                };
                let c = &skater.collision_feedback.bones[part];
                contact(record.velocity_changes[part], if b >= 23 { c.specific_normal } else { c.normal }, c.groups[3])
            }),
            rows: [hips[2], hips[0], hips[1], hips[3]],
            settled: p.skeleton.over_599 != 0,
            stick: [f32::from_bits(words[7]), f32::from_bits(words[8])],
            vehicles,
        }
    }
}

/// 0x8238c110's force |Δv · n| and 0x82bb7f48's "not on the ground" (a world contact, group 3, with n.y > 0.33).
fn contact(velocity_change: V, normal: V, world: bool) -> (f32, bool) {
    (dot3(velocity_change, normal).abs(), !(world && normal[1] > 0.33))
}

/// One air's metrics (scorer +2088: the air under way, +2104: the best of each, +2120: the landed airs' sums).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Air {
    frames: u32,
    air_points: u32,
    drop_points: u32,
    drop: f32,
}

/// A body-rotation tracker (scorer +112 / +192 / +272): the rows it last saw and the angle turned about its axis.
#[derive(Clone, Copy, Default)]
struct Tracker {
    rows: [V; 4],
    angle: f32,
    axis: usize,
}

/// One bail's score (Score::HoM::WipeoutScorer).
#[derive(Clone, Default)]
pub(crate) struct Bail {
    damage: [f32; BONES],
    level: [Option<usize>; BONES],
    /// |velocity| at the start (m/s) and its points.
    speed: f32,
    speed_points: u32,
    airborne: bool,
    /// The air under way's top height.
    top: f32,
    air: Air,
    best: Air,
    landed: Air,
    /// The tweak held (its id) and the frames it has been held past the first.
    tweak: Option<i32>,
    hold: u32,
    tweak_points: u32,
    tweaks: u32,
    trackers: [Tracker; 3],
    /// Degrees banked by the trackers, the most turned at once and the most rotation points.
    banked: f32,
    max_rotation: f32,
    max_rotation_points: u32,
    /// Frames before the wipeout settled.
    unsettled: u32,
    /// The vehicle scorer (0x82bb8a90, scorer +20): the cars hit as (id, HoM type, points), 8 at most (0x82bb86f0),
    /// and their points (+124).
    vehicles: Vec<(u64, i32, u32)>,
    vehicle_points: u32,
}

impl Bail {
    /// 0x82bb74d8 (after 0x82bb6d60's reset): trackers on the current rows (axes 2, 0, 1: 0x82bb6aa8), the feet's
    /// last impacts, the speed and its points.
    pub(crate) fn start(rules: &Rules, f: &Frame) -> Self {
        let mut bail = Self { trackers: [2, 0, 1].map(|axis| Tracker { rows: f.rows, angle: 0., axis }), ..default() };
        // ponytail: the engine has no foot-impact signal (Skate 2's 0x823944d0 staging), so both feet read 0.
        bail.feet(rules, 0., 0.);
        bail.speed = length(f.velocity);
        bail.speed_points = rules.curves.speed.points(bail.speed);
        bail
    }

    /// 0x82bb8398: one frame of the wipeout (the start's frame included).
    pub(crate) fn frame(&mut self, rules: &Rules, f: &Frame) {
        let was_airborne = self.airborne;
        // 0x82bb7f48: each bone's scaled force over 0.1 spreads; in the air unless some bone rests on the ground.
        self.airborne = true;
        for (bone, &(force, free)) in f.contacts.iter().enumerate() {
            let force = force * rules.force_scale;
            if force > 0.1 {
                self.spread(rules, bone, force, bone);
            }
            self.airborne &= free;
        }
        for (b, bone) in rules.bones.iter().enumerate() {
            self.level[b] = (0..6).rev().find(|&k| bone.levels[k].1 > 0. && self.damage[b] > bone.levels[k].1);
        }
        if self.airborne && !was_airborne {
            (self.top, self.air, self.tweak, self.hold) = (f.height, Air::default(), None, 0);
        } else if was_airborne && !self.airborne {
            self.air_frame(rules, f);
            self.bank_air();
        }
        // 0x82bb84c8's entity scorers: the vehicle scorer 0x82bb8a90 (the DMO and pedestrian ones have no signal: 0).
        self.cars(rules, &f.vehicles);
        if self.airborne {
            self.air_frame(rules, f);
        }
        if f.settled {
            for t in &mut self.trackers {
                t.rows = f.rows;
            }
        } else {
            for t in &mut self.trackers {
                self.banked += t.track(&f.rows);
            }
        }
        // 0x82bb70e0
        let deg = |t: &Tracker| (t.angle * DEGREES).abs();
        let rotation = deg(&self.trackers[1]) + deg(&self.trackers[0]) + deg(&self.trackers[2]) + self.banked;
        self.max_rotation = if rotation - self.max_rotation >= 0. { rotation } else { self.max_rotation };
        self.max_rotation_points = self.max_rotation_points.max(rules.curves.rotation.points(rotation));
        if !f.settled {
            self.unsettled += 1;
        }
    }

    /// 0x82bb8a90: each car hit not yet on the list goes on it (while it has room) with its type's points (0x82bb8be0:
    /// the `vehicles` entry for the type, else vehicle_points).
    fn cars(&mut self, rules: &Rules, hits: &[Option<(u64, i32)>; 8]) {
        for &(id, kind) in hits.iter().flatten() {
            if self.vehicles.len() < 8 && !self.vehicles.iter().any(|v| v.0 == id) {
                let points = rules.vehicles.iter().find(|v| v.0 == kind).map_or(rules.vehicle_points, |v| v.1);
                self.vehicles.push((id, kind, points));
                self.vehicle_points += points;
            }
        }
    }

    /// 0x82bb7ca8: adds `force` to `bone` and passes each link its share over 0.1, never back to `from`.
    fn spread(&mut self, rules: &Rules, bone: usize, force: f32, from: usize) {
        self.damage[bone] = force + self.damage[bone];
        for &(link, share) in &rules.bones[bone].links {
            let passed = share * force;
            if link != from && passed > 0.1 {
                self.spread(rules, link, passed, bone);
            }
        }
    }

    /// 0x82bb75b0: the last foot impacts before the bail hurt the feet and, by toe_share, the toes.
    fn feet(&mut self, rules: &Rules, left: f32, right: f32) {
        let curve = &rules.curves.leg_impact;
        let (left, right) = (rules.force_scale * curve.at(left), rules.force_scale * curve.at(right));
        self.spread(rules, 15, left, 15);
        self.spread(rules, 14, rules.toe_share * left, 14);
        self.spread(rules, 19, right, 19);
        self.spread(rules, 18, rules.toe_share * right, 18);
    }

    /// 0x82bb8288: an airborne frame: the top, the drop below it, their points, then the body tweak.
    fn air_frame(&mut self, rules: &Rules, f: &Frame) {
        if f.height > self.top {
            self.top = f.height;
        }
        self.air.frames += 1;
        self.air.drop = self.top - f.height;
        self.air.air_points = rules.curves.air_time.points(self.air.frames as f32 * DT);
        self.air.drop_points = rules.curves.drop.points(self.air.drop);
        self.body_tweak(rules, f.stick);
    }

    /// 0x82bb81a0: a landing adds the air to the sums and keeps the best of each.
    fn bank_air(&mut self) {
        let (air, sum, best) = (std::mem::take(&mut self.air), self.landed, self.best);
        self.landed = Air {
            frames: air.frames + sum.frames,
            air_points: air.air_points + sum.air_points,
            drop_points: air.drop_points + sum.drop_points,
            drop: air.drop + sum.drop,
        };
        self.best = Air {
            frames: best.frames.max(air.frames),
            air_points: best.air_points.max(air.air_points),
            drop_points: best.drop_points.max(air.drop_points),
            drop: if best.drop - air.drop >= 0. { best.drop } else { air.drop },
        };
        (self.tweak, self.hold) = (None, 0);
    }

    /// 0x82bb71b0: the stick past tweak_stick within π / (tweaks) of a tweak's direction holds it; it scores when held
    /// `frames` frames past the first (a new or re-taken one starts again).
    fn body_tweak(&mut self, rules: &Rules, [x, y]: [f32; 2]) {
        let before = self.tweak.take();
        let half = PI / rules.tweaks.len() as f32;
        let held = (x * x + y * y > rules.tweak_stick * rules.tweak_stick)
            .then(|| rules.tweaks.iter().find(|t| angle_2d(t.stick, [x, y]) < half))
            .flatten();
        let Some(t) = held else {
            self.hold = 0;
            return;
        };
        self.tweak = Some(t.id);
        self.hold = if before == Some(t.id) { self.hold + 1 } else { 0 };
        if self.hold == t.frames {
            self.tweak_points += t.points;
            self.tweaks += 1;
        }
    }

    /// 0x82bb7008: Σ over bones of their level's points.
    fn damage_points(&self, rules: &Rules) -> u32 {
        (rules.bones.iter().zip(self.level)).filter_map(|(bone, level)| level.map(|k| bone.levels[k].0)).sum()
    }

    fn bail_time_points(&self, rules: &Rules) -> u32 {
        rules.curves.bail_time.points(self.unsettled as f32 * DT)
    }

    /// 0x82bb7410: the tweaks, the best rotation, speed, the air under way and the landed airs, bail time, the cars.
    fn bonus_points(&self, rules: &Rules) -> u32 {
        self.vehicle_points
            + self.tweak_points
            + self.max_rotation_points
            + self.speed_points
            + self.air.drop_points
            + self.air.air_points
            + self.landed.drop_points
            + self.landed.air_points
            + self.bail_time_points(rules)
    }

    /// 0x82bb7790's score: damage + bonus (no multiplier).
    pub(crate) fn total(&self, rules: &Rules) -> i64 {
        i64::from(self.damage_points(rules) + self.bonus_points(rules))
    }

    /// The panel's metric points as 0x823c6af8 feeds them: [speed, drop, air time, bail time, rotation] (the landed
    /// airs' sums; the air under way only counts in the total).
    fn bonus(&self, rules: &Rules) -> [u32; 5] {
        [self.speed_points, self.landed.drop_points, self.landed.air_points, self.bail_time_points(rules), self.max_rotation_points]
    }
}

impl Tracker {
    /// 0x82bb6718 / 0x82bb6668: turn by the step to `rows` (wrapped into ±π). Falling back under its last |angle| from
    /// past 30° banks those degrees (returned) and keeps only this step.
    fn track(&mut self, rows: &[V; 4]) -> f32 {
        let before = self.angle;
        let degrees = (before * DEGREES).abs();
        let mut step = self.step(rows);
        if step > PI {
            step -= TAU;
        } else if step < -PI {
            step += TAU;
        }
        self.angle = step + self.angle;
        self.rows = *rows;
        if (self.angle * DEGREES).abs() < degrees && degrees > 30. {
            self.angle -= before;
            degrees
        } else {
            0.
        }
    }

    /// 0x82bb67b8: how far (0..2π) the tracked row turned about the axis row: the new row put in the plane across the
    /// axis, measured from the old one (0 when over 90° away).
    fn step(&self, new: &[V; 4]) -> f32 {
        let (axis, reference, row) = match self.axis {
            0 => (self.rows[1], self.rows[2], new[2]),
            1 => (self.rows[2], self.rows[1], new[1]),
            2 => (self.rows[0], self.rows[1], new[1]),
            _ => return 0.,
        };
        let c = cross(axis, row);
        let u = scale(c, rsqrt(dot3(c, c), 2));
        let c = cross(u, axis);
        let w = scale(c, rsqrt(dot3(c, c), 2));
        if 0. > dot3(w, reference) {
            return 0.;
        }
        angle_about(w, reference, axis)
    }
}

/// 0x8238ce50: the angle from `p` to `q` (0..2π) turning about `axis`; 0 when either is shorter than 0.01.
fn angle_about(p: V, q: V, axis: V) -> f32 {
    let (pp, qq) = (dot3(p, p), dot3(q, q));
    if !(pp > 1e-4 && qq > 1e-4) {
        return 0.;
    }
    let (p, q) = (scale(p, rsqrt(pp, 1)), scale(q, rsqrt(qq, 1)));
    let angle = acos(dot3(p, q).max(-1.).min(1.));
    if 0. > dot3(cross(p, q), axis) { TAU - angle } else { angle }
}

/// 0x827d1fd8: the angle between two 2D directions; 0 when either is shorter than 0.01.
fn angle_2d(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (aa, bb) = (a[0] * a[0] + a[1] * a[1], b[0] * b[0] + b[1] * b[1]);
    if !(aa > 1e-4 && bb > 1e-4) {
        return 0.;
    }
    let (ra, rb) = (rsqrt(aa, 2), rsqrt(bb, 2));
    acos(((a[0] * ra) * (b[0] * rb) + (a[1] * ra) * (b[1] * rb)).max(-1.).min(1.))
}

/// 0x82377d98: Skate 2's vector arccos (a polynomial in |x| and one Newton step on 1/√(1.0000001 − |x|)).
fn acos(x: f32) -> f32 {
    const C1: [u32; 4] = [0x3F1D_D7B6, 0x4089_80BD, 0xBF98_3F2F, 0xC0D1_360E];
    const C2: [u32; 4] = [0xBD6D_D42D, 0xBED6_5553, 0x3E66_3246, 0x400B_1889];
    const C3: [u32; 4] = [0xBFAF_4418, 0xC08F_6AD9, 0x3FB5_8485, 0x40AF_6AD8];
    let a = x.abs();
    let near = f32::from_bits(0x3F80_0001) - a;
    let cubic = x * x * a;
    let odd = (-a).mul_add(x, x);
    let p = |i: usize| {
        let c = |t: [u32; 4]| f32::from_bits(t[i]);
        c(C2).mul_add(a, c(C1)).mul_add(a, c(C3))
    };
    let (high, low) = (p(2).mul_add(cubic, p(3)), p(0).mul_add(cubic, p(1)));
    let e = reciprocal_square_root_estimate(near);
    let r = e.mul_add((-(near * 0.5)).mul_add(e * e, 0.5), e);
    PI * 0.5 - (odd * low).mul_add(r, x * high)
}

/// vrsqrtefp and `steps` Newton steps as the scorer's callers write them (the estimate is the engine's exact one).
fn rsqrt(d: f32, steps: usize) -> f32 {
    let mut r = reciprocal_square_root_estimate(d);
    for _ in 0..steps {
        r = (r * 0.5).mul_add((-d).mul_add(r * r, 1.), r);
    }
    r
}

/// |v| as 0x82bb74d8 / 0x82bb7790 take it: d · rsqrt(d), 0 for d = 0.
fn length(v: V) -> f32 {
    let d = dot3(v, v);
    if d == 0. { 0. } else { d * rsqrt(d, 2) }
}

/// vmsum3fp. ponytail: taken as two rounded adds in order (the engine's dot3); VMX128's internal rounding isn't known.
fn dot3(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn scale(v: V, s: f32) -> V {
    v.map(|x| x * s)
}

/// a × b as the scorer's VMX128 builds it: (a · b.yzx − a.yzx · b).yzx, the subtraction fused (vnmsubfp).
fn cross(a: V, b: V) -> V {
    let lane = |k: usize| (-a[(k + 1) % 3]).mul_add(b[k], a[k] * b[(k + 1) % 3]);
    [lane(1), lane(2), lane(0), 0.]
}

// ---------------------------------------------------------------- the run
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Intro { ends: f32 },
    /// countdown stateenter until teleporttransitionfinished: AI.RepositionAI's fade from `from` (the turn's player to
    /// the attempt spawn, the last one back to its waiting slot). Round 0: the last attempter's move after the last
    /// round; `news`: the round message follows (RoundNumberChanged, or solo).
    Move { round: u32, from: f32, news: bool },
    /// Riding: the timer runs until `ends`.
    Turn { round: u32, ends: f32 },
    Bailing { round: u32 },
    /// Between attempts: the next starts at `at`.
    Next { round: u32, at: f32 },
    Countdown { round: u32, ends: f32 },
    /// Online: someone else's turn, watched from the waiting spot until its game says it is over.
    Watch { round: u32 },
    /// challenge_finished: "Challenge Complete" until `until`.
    Ending { until: f32 },
    Done,
}

struct Run {
    row: String,
    stage: Stage,
    best: i64,
    /// Central messages queued: (text, shown from).
    messages: Vec<(String, f32)>,
    /// This console's own skater's next RepositionAI move (made at the fade's black; retried while the engine's or
    /// the core's own teleport is pending).
    teleport: Option<[[f32; 4]; 4]>,
    /// The turn ([`schedule`]) and whose it is; this player's index; the players (1 solo).
    k: usize,
    active: usize,
    me: usize,
    players: usize,
    online: bool,
    /// The first player ([`schedule`]): Skate 3's rule (user 10-28), drawn from the match token on every console.
    first: usize,
    /// Skate 3's rule (user 10-28): this console's attempt is past its time: the EndRunTimerID's end (S3
    /// hall_of_meat.lua 887-901; [`overtime`]).
    buzzer: Option<f32>,
    /// Online: turns whose end the active player's game has sent.
    over: BTreeSet<usize>,
    /// The wipeout's FadeOut has gone black: it stays black until the next RepositionAI's black.
    dark: bool,
    /// The camera's player (Online.SwitchCamera; None: this one), switched at a turn's fade's black (bool6).
    watching: Option<usize>,
    /// Pause.EnablePauseMenuAndBackpack has run (the first countdown's teleporttransitionfinished).
    pause_ok: bool,
    /// The score panel's last refresh and what it last showed.
    panel: Option<Shown>,
    standings: Vec<(String, i64, bool)>,
    /// Online: every turn's best so far (its TurnOver), by turn.
    turns: BTreeMap<usize, i64>,
    /// Online with 3+ players (setup 162: EnableVoting): the waiting players vote on each attempt.
    voting: bool,
    /// VotingManager's votes on this turn's attempt, by player (1, ok, until changed; StopVoting resets them).
    votes: Vec<u8>,
    /// What hudonlinevoting last showed (this player's vote); None while hidden.
    vote_shown: Option<u8>,
    /// HighestVote (votingcomplete 706-712): the Player's Choice so far, (player, mean vote).
    best_voted: Option<(usize, f32)>,
    /// Party Play (Skate 3's party path): its players take the turns on this one skater (`me` follows the turn), by
    /// these names; the turn whose Press-A card was opened, and the one whose card was answered.
    party: bool,
    names: Vec<String>,
    carded: Option<usize>,
    answered: Option<usize>,
}

/// The per-tick side ([`tick`]) and the frame side ([`update`]) share this.
#[derive(Resource, Default)]
pub(crate) struct HallOfMeat {
    data: Option<Arc<Data>>,
    root: PathBuf,
    /// Score a bail that starts now (an attempt is under way).
    armed: bool,
    /// The live (or last) bail and whether the wipeout is still on.
    bail: Option<Bail>,
    bailing: bool,
    /// Under the Skate 2 physics switch: the port's live (or last) bail (`Frame.hom`), scored and shown in place of
    /// `bail` (Task 18).
    port: Option<skate2_physics_protocol::Hom>,
    /// The wipeout ended since the frame side last looked.
    ended: bool,
    /// The engine's teleport fade: (time left, its length) while pending.
    fade_out: Option<(f32, f32)>,
    run: Option<Run>,
    /// Free skate on a Skate 2 map (hall_of_meat::free_game): the score panel's last
    /// refresh while it is up (skate2_hud hides the minimap meanwhile).
    pub(crate) free: Option<Option<Shown>>,
    screens: TutorialScreens,
    veil: Option<Entity>,
    /// The row last warned about as missing (logged once, not every frame).
    warned: String,
}

#[derive(Component)]
struct Veil;

pub(crate) struct Skate2HallOfMeatPlugin;
impl Plugin for Skate2HallOfMeatPlugin {
    fn build(&self, app: &mut App) {
        let root = app.world().resource::<Config>().asset_root.join(ROOT);
        let data = Data::load(&root).inspect_err(|e| warn!("Skate 2 Hall of Meat: {e}")).ok().map(Arc::new);
        if let (Some(data), Some(mut playable)) = (&data, app.world_mut().get_resource_mut::<Playable>()) {
            playable.0.extend(data.rows.keys().cloned());
        }
        app.insert_resource(HallOfMeat { data, root, ..default() })
            .add_systems(FixedUpdate, tick.after(crate::app::SimulationSet::Physics).after(crate::traffic::drive))
            .add_systems(Update, update.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick, as 0x82337f88 drives the scorer: entering the Wipeout category starts a bail (when one is to be
/// scored), every tick of it (the first included) is a scorer frame, and leaving it ends the bail. While Skate 2
/// physics has the skater the engine's sim (and so its scorer's input) does not run: the port's own bail is taken
/// instead ([`port_tick`], Task 18).
fn tick(
    mut hom: ResMut<HallOfMeat>,
    skater: Res<SkaterRuntime>,
    controls: Res<PlayerControls>,
    traffic: Option<Res<crate::traffic::Traffic>>,
    skate2: Option<Res<crate::skate2_physics::Skate2Physics>>,
) {
    let Some(data) = hom.data.clone() else { return };
    if let Some(skate2) = skate2.as_deref().filter(|s| s.drives_local()) {
        if let Some(frame) = skate2.last_good.as_deref() {
            port_tick(&mut hom, &frame.hom);
        }
        return;
    }
    let state = &skater.wipeout_state.state;
    // wipeoutalmostover: the bail's reset countdown, only inside a bail. I3 (online audit 10-06): `teleport_pending`
    // stays up after every reset (only the next bail's `enter` clears it) and a free board raises the board
    // manager's flag, so a dismount after any bail read as this fade and blacked the turn out; the Wipeout category
    // is the bail (as skate2_contests' and skate3_uniform's probes gate it).
    hom.fade_out = match hom.fade_out {
        _ if !(skater.wiping_out() && state.teleport_pending) => None,
        Some((_, length)) => Some((state.time_until_teleport, length)),
        None => Some((state.time_until_teleport, state.time_until_teleport.max(DT))),
    };
    if skater.player_input.physical.filtered_state_0 != WIPEOUT {
        if std::mem::take(&mut hom.bailing) {
            hom.ended = true;
        }
        return;
    }
    let frame = Frame::read(&skater, &controls, traffic.map_or([None; 8], |t| t.hom_hits()));
    if !hom.bailing {
        if !hom.armed {
            return;
        }
        hom.bail = Some(Bail::start(&data.rules, &frame));
        hom.port = None;
        hom.bailing = true;
    }
    if let Some(bail) = hom.bail.as_mut() {
        bail.frame(&data.rules, &frame);
    }
}

/// One tick of the port's bail under the Skate 2 physics switch (Task 18), as [`tick`] runs the engine's: the port's
/// `bail_in_progress` starts a bail (when one is to be scored), its numbers are the bail's while it lasts, and
/// `bail_completed` (or the bail no longer in progress) ends it, as leaving the Wipeout category ends the engine's. An
/// engine bail still on when the switch took the skater ends there, with the engine's score. Not native Skate 2 (user
/// 10-20): no fade before the reset (the protocol carries no reset countdown; the port respawns the skater itself).
/// Not tested in play.
fn port_tick(hom: &mut HallOfMeat, frame: &skate2_physics_protocol::Hom) {
    hom.fade_out = None;
    let live = frame.bail_in_progress && !frame.bail_completed;
    if !hom.bailing {
        if !(live && hom.armed) {
            return;
        }
        hom.bail = None;
        hom.bailing = true;
    }
    if hom.bail.is_none() {
        hom.port = Some(frame.clone());
    }
    if !live {
        hom.bailing = false;
        hom.ended = true;
    }
}

fn update(world: &mut World) {
    world.resource_scope(|world, mut hom: Mut<HallOfMeat>| {
        let now = crate::match_core::match_now(world);
        let (playing, phase) = {
            let m = world.resource::<Match>();
            (plays(m), m.phase.clone())
        };
        let free = !playing && hom.run.is_none() && crate::hall_of_meat::free_game(world) == 2;
        match (free, hom.data.clone()) {
            (true, Some(data)) => free_skate(world, &mut hom, &data, now),
            _ => free_skate_off(world, &mut hom),
        }
        match (playing, hom.data.clone()) {
            (true, Some(data)) => step(world, &mut hom, &data, &phase, now),
            _ if hom.run.is_some() => finish(world, &mut hom),
            _ => {}
        }
    });
}

/// Skate 2's Hall of Meat script runs the match: not an `Owned` row (S3 HOM Battle, S1), which its own mode plays.
fn plays(m: &Match) -> bool {
    !m.owned && m.mode == Some(Mode::HallOfMeat) && m.phase != Phase::Idle
}

/// Skate 2's career free roam (IsHOMAvailable 0x82313b18 passes): the panel shows at each wipeout's start (0x82650028;
/// SetState 1, no skin change: free roam never sends 999/1000), climbs with the score, and goes at the first frame back
/// on the ground, a rail or the feet after the wipe (0x823d4090 / 0x8264fe40). ponytail: hidden with Settings > HUD
/// (Skate 2's own HUD toggle reader wasn't traced); the minimap hide the panel does is skate2_hud's.
fn free_skate(world: &mut World, hom: &mut HallOfMeat, data: &Data, now: f32) {
    hom.armed = true;
    hom.ended = false;
    let mut shown = hom.free.take().flatten();
    let hud = world.get_resource::<crate::graphics_menu::Menu>().is_none_or(|m| m.hud_visible());
    let category = world.resource::<SkaterRuntime>().player_input.physical.filtered_state_0;
    if !hud {
        if shown.take().is_some()
            && let Some(mut screens) = world.get_resource_mut::<Screens>()
        {
            screens.layer(HALL_OF_MEAT).close(SCORING);
        }
    } else if hom.bailing {
        let show = shown.is_none();
        panel(world, hom, data, &mut shown, now, show, false);
    } else if shown.is_some() {
        panel(world, hom, data, &mut shown, now, false, false);
        if matches!(category, 1 | 3 | 6) {
            hide_panel(world, &mut shown);
        }
    }
    hom.free = Some(shown);
}

fn free_skate_off(world: &mut World, hom: &mut HallOfMeat) {
    if hom.free.take().is_some() {
        hom.armed = false;
        (hom.bail, hom.port) = (None, None);
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            screens.layer(HALL_OF_MEAT).close(SCORING);
        }
    }
}

/// Ends the run: screens, scene, indicators and veil off.
fn finish(world: &mut World, hom: &mut HallOfMeat) {
    hom.run = None;
    hom.armed = false;
    (hom.bail, hom.port) = (None, None);
    hom.screens.clear(world);
    if let Some(mut screens) = world.get_resource_mut::<Screens>() {
        let layer = screens.layer(HALL_OF_MEAT);
        layer.close(SCORING);
        layer.close(LEADERBOARD);
        screens.layer(crate::apt_screen::SKATE2_ONLINE).close(crate::skate2_contests::VOTING);
    }
    world.resource_mut::<Cutscene>().stop();
    let mut arrows = world.resource_mut::<crate::skate2_arrows::Arrows>();
    arrows.wanted.clear();
    arrows.more.clear();
    veil(world, hom, 0.);
}

fn step(world: &mut World, hom: &mut HallOfMeat, data: &Data, phase: &Phase, now: f32) {
    let (token, slot, players) = {
        let m = world.resource::<Match>();
        (m.token, m.slot, m.players.clone())
    };
    let online = token != 0 && !players.is_empty();
    // playerdisconnected: the core blanks the name of a slot whose player left.
    let gone = |p: usize| online && players.get(p).is_none_or(String::is_empty);
    if hom.run.is_none() {
        // Bug 11-01 (as skate2_contests'): the flythrough waits for the start's trip (Party Play's map load, online the
        // start slot's cover), or it plays out behind the loading screen; the core's intro waits meanwhile.
        if !crate::skate1_online::trip_over(world, MAP) {
            let mut m = world.resource_mut::<Match>();
            (m.intro, m.no_countdown) = (true, true);
            return;
        }
        let row_id = world.resource::<Match>().row.clone();
        let Some(row) = data.rows.get(&row_id) else {
            if hom.warned != row_id {
                warn!("Skate 2 Hall of Meat: no row {row_id:?} in hom.json");
                hom.warned = row_id;
            }
            world.resource_mut::<Match>().finished = true;
            return;
        };
        // Party Play: its players take turns on this one skater (Skate 3's party path).
        let party = !online && crate::party_play::in_session(world);
        let names = if party { crate::party_play::names(world) } else { Vec::new() };
        let (n, me) = if online { (players.len(), slot) } else { (names.len().max(1), 0) };
        // Skate 3's rule (user 10-28): a random first player, the same on every console: the match token, as
        // skate3_uniform's draw (S3 hall_of_meat.lua 143-144); Party Play from the clock.
        let seed = if party { u64::from(now.to_bits()) } else { token };
        let first = (seed % n.max(1) as u64) as usize;
        let (k, active) = schedule(first, first, n, row.num_rounds, gone).map_or((first, me), |(k, _, p)| (k, p));
        let me = if party { active } else { me };
        let mut m = world.resource_mut::<Match>();
        // Party Play has no cap: its cards and its results dialog wait on the players.
        m.time_limit = (!party).then(|| n as f32 * row.num_rounds as f32 * (row.time_limit + ATTEMPT_OVERHEAD));
        // The flythrough goes straight to wait_for_players: the countdowns are the Lua's own.
        m.no_countdown = true;
        // It runs to its camera's end on its clock (nis-clock.md 10), not its record's length.
        let real = |scene: &String, s: f32| crate::challenge::cutscene::real_length(&hom.root, scene).unwrap_or(s);
        let intro = row.intro.as_ref().zip(row.intro_seconds).map(|(scene, s)| (scene, real(scene, s)));
        m.intro = intro.is_some();
        if let Some((scene, _)) = intro {
            world.resource_mut::<Cutscene>().start(&hom.root, scene);
        }
        let ends = now + intro.map_or(0., |(_, s)| s);
        hom.run = Some(Run {
            row: row_id,
            stage: Stage::Intro { ends },
            best: 0,
            messages: Vec::new(),
            teleport: None,
            k,
            active,
            me,
            players: n,
            online,
            first,
            buzzer: None,
            over: BTreeSet::new(),
            dark: false,
            watching: None,
            pause_ok: false,
            panel: None,
            standings: Vec::new(),
            turns: BTreeMap::new(),
            voting: online && n >= 3,
            votes: vec![1; n],
            vote_shown: None,
            best_voted: None,
            party,
            names,
            carded: None,
            answered: None,
        });
        world.resource_mut::<crate::skate2_arrows::Arrows>().more = vec![format!("{ROOT}/indicator/indicators.json")];
    }
    let Some(mut run) = hom.run.take() else { return };
    let row = &data.rows[&run.row];
    if run.online {
        hear(&mut run, std::mem::take(&mut world.resource_mut::<Match>().inbox));
        let mut m = world.resource_mut::<Match>();
        m.order = placing(run.players, &run.turns, &m.left);
    }
    if at_black(&mut run, now)
        && let Some(at) = run.teleport
        && teleport(world, at)
    {
        run.teleport = None;
    }
    let own = run_name(world);
    let names = if run.party { &run.names } else { &players };
    let name = if run.online || run.party { names.get(run.active).cloned().unwrap_or_default() } else { own.clone() };
    let mine = run.active == run.me;
    let mut countdown = None;
    let mut timer = None;
    match (run.stage, phase) {
        (Stage::Intro { ends }, Phase::Intro { .. }) => {
            if now >= ends && world.resource::<Match>().intro {
                world.resource_mut::<Cutscene>().stop();
                world.resource_mut::<Match>().intro = false;
            }
        }
        // wait_for_players (hall_of_meat.lua 734-757): everyone to their waiting slot; countdown stateenter (304-385):
        // the first player to the attempt spawn's slot 0, in the same fade.
        (Stage::Intro { .. }, Phase::Running { .. }) => {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
            run.teleport = if mine { row.attempt.as_ref().map(|p| p.slot(0)) } else { row.waiting.as_ref().map(|p| p.slot(run.me)) };
            run.stage = Stage::Move { round: 1, from: now, news: true };
        }
        (Stage::Move { .. }, Phase::Running { .. }) => after_move(&mut run, data, &name, now),
        (Stage::Ending { until }, Phase::Running { .. }) if now >= until => {
            // Party Play's match ends at its results dialog, not at the core's results.
            world.resource_mut::<Match>().finished |= !run.party;
            run.stage = Stage::Done;
        }
        (Stage::Turn { round, ends }, Phase::Running { .. }) => {
            if run.buzzer.is_none() && now - ends + row.time_limit < 1. {
                countdown = Some((i64::from(round), 0));
            }
            if hom.bailing || hom.ended {
                run.stage = Stage::Bailing { round };
                hom.armed = false;
                panel(world, hom, data, &mut run.panel, now, true, true);
            } else if now >= ends {
                if run.buzzer.is_none() {
                    run.messages.push((data.text("ID_CHALLENGE_CENTRALMESSAGE_RANOUTOFTIME"), now));
                }
                let last = schedule(run.k + 1, run.first, run.players, row.num_rounds, gone).is_none();
                if !overtime(&mut run, round, last, now) {
                    hom.armed = false;
                    world.resource_mut::<crate::skate2_arrows::Arrows>().wanted.clear();
                }
            } else if run.buzzer.is_none() {
                timer = Some(f64::from(ends - now));
            }
        }
        // Skate 3's rule (user 10-28): the 2 s after time-up run out mid-bail: the turn ends unscored (S3 end_turn has no
        // wipeoutover handler; hall_of_meat.lua 897-900).
        (Stage::Bailing { round }, Phase::Running { .. }) if !hom.ended && run.buzzer.is_some_and(|b| now >= b) => {
            world.resource_mut::<crate::skate2_arrows::Arrows>().wanted.clear();
            run.stage = Stage::Next { round, at: now + BETWEEN };
        }
        (Stage::Bailing { round }, Phase::Running { .. }) => {
            panel(world, hom, data, &mut run.panel, now, false, true);
            if std::mem::take(&mut hom.ended) {
                let score = match (&hom.port, &hom.bail) {
                    (Some(port), _) => i64::from(port.hud.score),
                    (None, bail) => bail.as_ref().map_or(0, |b| b.total(&data.rules)),
                };
                info!("Skate 2 Hall of Meat: round {round} scored {score}");
                run.best = run.best.max(score);
                world.resource_mut::<Match>().score = run.best;
                world.resource_mut::<crate::skate2_arrows::Arrows>().wanted.clear();
                run.stage = Stage::Next { round, at: now };
            }
        }
        (Stage::Next { round, at }, Phase::Running { .. }) if now >= at => {
            // Our turn is over: the others move on when they hear it.
            let over = turn_over(&mut run, &gone);
            if run.online
                && let Ok(text) = serde_json::to_string(&over)
            {
                world.resource_mut::<Match>().outbox.push(text);
            }
            next_turn(&mut run, row, data, gone, round, now);
        }
        (Stage::Watch { round }, Phase::Running { .. }) if run.over.contains(&run.k) || gone(run.active) => {
            left_turn(&mut run, &gone);
            next_turn(&mut run, row, data, gone, round, now);
        }
        (Stage::Countdown { round, ends }, Phase::Running { .. }) => {
            if now >= ends {
                hide_panel(world, &mut run.panel);
                run.buzzer = None;
                run.stage = if mine { start_turn(world, hom, row, round, now) } else { Stage::Watch { round } };
            } else {
                countdown = Some((i64::from(round), (ends - now).ceil() as i64));
            }
        }
        _ => {}
    }
    let wipe = hom.fade_out.filter(|_| mine && matches!(run.stage, Stage::Turn { .. } | Stage::Bailing { .. }));
    let fade = screen(&mut run, wipe, now);
    veil(world, hom, fade);
    run.messages.retain(|(_, from)| now < from + MESSAGE);
    let message = run.messages.iter().filter(|(_, from)| *from <= now).next_back().map(|(m, _)| m.clone());
    let title = row.title.as_deref().map(|t| data.text(t)).unwrap_or_default();
    let view = View { countdown, timer, result: message.map(|m| (m, title.clone(), Central::Plain)), ..default() };
    hom.screens.sync(world, &view);
    let show = !matches!(run.stage, Stage::Intro { .. });
    leaderboard(world, &mut run, &title, &own, show);
    // The waiting players' vote on the attempt (OnlineVotingHUD): each change goes to every game.
    let pressed = world.get_resource::<crate::customiser::Navigation>().map_or(0, |n| n.pressed);
    if let Some(vote) = cast_vote(&mut run, pressed)
        && let Ok(text) = serde_json::to_string(&vote)
    {
        world.resource_mut::<Match>().outbox.push(text);
    }
    let want = voting_hud(&run);
    crate::skate2_contests::vote_screen(world, &mut run.vote_shown, want);
    // PinAI on everyone but the active player from its challenge_running UnpinAI (hall_of_meat.lua 7) to the next
    // countdown's PinAI (352, 377-380; end_turn pins no one); the others watch the camera's player.
    if !matches!(phase, Phase::Results { .. }) {
        let riding = mine && matches!(run.stage, Stage::Turn { .. } | Stage::Bailing { .. } | Stage::Next { .. });
        let mut m = world.resource_mut::<Match>();
        (m.hold, m.watch) = (!riding, run.watching.filter(|&p| run.online && p != run.me));
        // Skate 3's rule (user 10-28, s3rules S2 item 5): the D-pad emotes go through this mode's own waits, as
        // skate3_uniform's HOM Battle (skate3_modes::gestures); its results are the core's.
        m.gestures = crate::skate3_modes::gestures(&m, false);
    }
    {
        let results = matches!(phase, Phase::Results { .. });
        let mut m = world.resource_mut::<Match>();
        m.pause_disabled = pause_disabled(&run, results);
        m.trick_display_hidden = trick_display_hidden(run.stage, results);
    }
    let again = run.party && party_exchange(world, &mut run, row, &title, now);
    hom.run = Some(run);
    if again {
        info!("Skate 2 Hall of Meat: {} again", row.title.as_deref().unwrap_or_default());
        finish(world, hom);
    }
}

/// Party Play: the turn's fade is over and its Press-A card is due (its teleporttransitionfinished waits for the
/// card's A): (player 1-based, round).
fn card_due(run: &Run, now: f32) -> Option<(usize, i64)> {
    match run.stage {
        Stage::Move { round, from, .. } if run.party && round > 0 && now - from >= MOVE && run.answered != Some(run.k) => {
            Some((run.active + 1, i64::from(round)))
        }
        _ => None,
    }
}

/// Party Play (party_play::exchange), as Skate 3's party HOM Battle: the card as each turn's fade ends, the pad on the
/// player the turn's fade switched to (at its black), the others on their TurnBasedWaitingLocation slots (slot = player
/// id), the results dialog once the match is done. True for Rematch / Restart Challenge: the match again on this row.
fn party_exchange(world: &mut World, run: &mut Run, row: &Row, title: &str, now: f32) -> bool {
    let card = card_due(run, now).filter(|_| run.carded != Some(run.k));
    if card.is_some() {
        run.carded = Some(run.k);
    }
    let controlled = run.watching.unwrap_or(run.active);
    let waiting = (0..run.players).map(|p| row.waiting.as_ref().filter(|_| p != controlled).map(|w| w.slot(p))).collect();
    let results = matches!(run.stage, Stage::Done).then(|| {
        let rows: Vec<_> = (placing(run.players, &run.turns, &[]).into_iter().enumerate())
            .map(|(i, (p, best))| (p, run.names.get(p).cloned().unwrap_or_default(), i as i64 + 1, best))
            .collect();
        crate::party_play::standings(title, &rows, "ID_LEADERBOARD_POINTS_HEADER", 0)
    });
    let answers = crate::party_play::exchange(world, crate::party_play::Turn { controlled, waiting, card, results });
    if answers.hud_complete {
        run.answered = Some(run.k);
    }
    answers.retry || answers.restart
}

/// Online: the turns whose end their own player's game has sent (with its mean vote), and the votes on this turn's
/// attempt.
fn hear(run: &mut Run, items: Vec<(usize, String)>) {
    for (from, text) in items {
        // Only the turn's own player ends it and names its mean (a peer's word: a mean is 0..=3).
        if let Ok(TurnOver { hom_turn, best, voted }) = serde_json::from_str(&text) {
            if hom_turn % run.players == from {
                run.over.insert(hom_turn);
                run.turns.insert(hom_turn, best);
                if let Some(score) = voted.filter(|s| (0.0_f32..=3.0).contains(s)) {
                    players_choice(run, from, score);
                }
            }
        // The attempter doesn't vote; a vote on another turn is too late or too early.
        } else if let Ok(Vote { hom_vote, level }) = serde_json::from_str(&text)
            && hom_vote == run.k
            && from != run.active
            && let Some(vote) = run.votes.get_mut(from)
        {
            *vote = level.min(3);
        }
    }
}

/// end_turn's votingcomplete (hall_of_meat.lua 706-712): a mean over 1 that beats the best so far names the turn's
/// attempter the Player's Choice.
fn players_choice(run: &mut Run, player: usize, score: f32) {
    if score > 1. && run.best_voted.is_none_or(|(_, best)| score > best) {
        run.best_voted = Some((player, score));
    }
}

/// end_turn's timer on the attempter's own game: its best into the results order, StopVoting (684: the mean of the
/// others' votes, its votingcomplete here) and the TurnOver every other game moves on with.
fn turn_over(run: &mut Run, gone: impl Fn(usize) -> bool) -> TurnOver {
    let me = run.me;
    let voted = run.voting.then(|| crate::skate2_contests::tally(&run.votes, me, gone));
    if let Some(score) = voted {
        players_choice(run, me, score);
    }
    run.turns.insert(run.k, run.best);
    TurnOver { hom_turn: run.k, best: run.best, voted }
}

/// A turn that ends with its attempter gone and no TurnOver: StopVoting on this game's own votes. Not retail (user
/// 10-12, as skate3_uniform): retail's VotingManager names one mean on every console natively.
fn left_turn(run: &mut Run, gone: impl Fn(usize) -> bool) {
    if run.voting && !run.over.contains(&run.k) {
        let active = run.active;
        let score = crate::skate2_contests::tally(&run.votes, active, gone);
        players_choice(run, active, score);
    }
}

/// What hudonlinevoting shows this player: its vote while it waits through another's attempt with voting on (Watch:
/// that attempt's StartVoting, challenge_running 12, to its StopVoting); None hides it.
fn voting_hud(run: &Run) -> Option<u8> {
    let on = run.voting && run.active != run.me && matches!(run.stage, Stage::Watch { .. });
    on.then(|| run.votes.get(run.me).copied().unwrap_or(1))
}

/// A waiting player's press while hudonlinevoting is up (A up, B down, as skate2_contests): its new vote, to send to
/// every game, when it changed.
fn cast_vote(run: &mut Run, pressed: u16) -> Option<Vote> {
    voting_hud(run)?;
    let (k, me) = (run.k, run.me);
    let vote = run.votes.get_mut(me)?;
    let level = crate::skate2_contests::next_vote(pressed, *vote);
    if level == *vote {
        return None;
    }
    *vote = level;
    Some(Vote { hom_vote: k, level })
}

/// RepositionAI at black: true once this console's own move is due (no fade running, or its black), when a turn's
/// camera switch (bool6) and the wipeout's black are handed over.
fn at_black(run: &mut Run, now: f32) -> bool {
    let cut = match run.stage {
        Stage::Move { round, from, .. } => Some((round, from + REPOSITION_FADE / 2.)),
        _ => None,
    };
    if let Some((round, cut)) = cut
        && now >= cut
    {
        run.dark = false;
        if round > 0 {
            run.watching = Some(run.active);
        }
    }
    cut.is_none_or(|(_, cut)| now >= cut)
}

/// teleporttransitionfinished (hall_of_meat.lua 406-427): the pause menu back, "<name>, your turn" (+ the round), the
/// 3 s countdown; after the last round, challenge_finished (582-593) and its 2 s.
fn after_move(run: &mut Run, data: &Data, name: &str, now: f32) {
    let Stage::Move { round, from, news } = run.stage else { return };
    if now - from < MOVE || card_due(run, now).is_some() {
        return;
    }
    if round == 0 {
        run.messages.push((data.text("ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE"), now));
        run.stage = Stage::Ending { until: now + ENDING };
        return;
    }
    run.pause_ok = true;
    run.messages.push((data.text("ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN").replace("{0:s}", name), now));
    if news {
        let text = data.text("ID_CONTEST_CENTRALMESSAGE_ROUNDNUMBER").replace("{0:s}", &round.to_string());
        run.messages.push((text, now + MESSAGE / 2.));
    }
    run.stage = Stage::Countdown { round, ends: now + COUNTDOWN };
}

/// The screen: wipeoutalmostover's FadeOut (challenge_running, the attempter's; 112-118) over the engine's reset
/// countdown (`wipe`: time left, length), black from then until the next RepositionAI's black, and each RepositionAI's
/// own fade.
fn screen(run: &mut Run, wipe: Option<(f32, f32)>, now: f32) -> f32 {
    run.dark |= wipe.is_some();
    match (wipe, run.dark, run.stage) {
        (Some((left, length)), ..) => 1. - (left / length).clamp(0., 1.),
        (None, true, _) => 1.,
        (None, false, Stage::Move { from, .. }) => reposition_black(now - from),
        _ => 0.,
    }
}

/// Pause.DisablePauseMenuAndBackpack at initial (hall_of_meat.lua 156) and challenge_finished (587); Enable at each
/// countdown's teleporttransitionfinished (411) and at complete (504).
fn pause_disabled(run: &Run, results: bool) -> bool {
    !results && (!run.pause_ok || matches!(run.stage, Stage::Ending { .. } | Stage::Done))
}

/// reset (hall_of_meat.lua 266-268): a Hall of Meat row hides the trick display, HUD2.Show(TrickDisplayHUD, false), from
/// the flythrough's end until challenge_cleanup's HUD2.DeleteGroup gives free skate's group back (online: as the results
/// begin).
fn trick_display_hidden(stage: Stage, results: bool) -> bool {
    !results && !matches!(stage, Stage::Intro { .. } | Stage::Done)
}

/// countdown stateenter (hall_of_meat.lua 304-385) after a turn: the next player to the attempt spawn's slot 0 and the
/// last one back to its waiting slot in one fade (each console moves its own skater); the messages and the countdown
/// follow the fade (the round message when it changes; solo always). After the last round the last attempter goes back
/// to the attempt spawn (its own slot) and challenge_finished follows the fade; gone, the camera goes to the next
/// player (Online.SwitchCamera) and it finishes at once.
fn next_turn(run: &mut Run, row: &Row, data: &Data, gone: impl Fn(usize) -> bool, round: u32, now: f32) {
    // StopVoting put every vote back to ok.
    run.votes.fill(1);
    let was_mine = run.active == run.me;
    match schedule(run.k + 1, run.first, run.players, row.num_rounds, &gone) {
        Some((k, next_round, active)) => {
            (run.k, run.active) = (k, active);
            if run.party {
                // Party Play: the one skater is the next player's, with that player's best so far.
                run.me = active;
                run.best = (run.turns.iter()).filter(|&(&t, _)| t % run.players == active).map(|(_, &b)| b).max().unwrap_or(0);
            }
            if active == run.me {
                run.teleport = row.attempt.as_ref().map(|p| p.slot(0));
            } else if was_mine {
                run.teleport = row.waiting.as_ref().map(|p| p.slot(run.me));
            }
            run.stage = Stage::Move { round: next_round, from: now, news: next_round != round || !run.online };
        }
        None if !gone(run.active) => {
            if was_mine {
                run.teleport = row.attempt.as_ref().map(|p| p.slot(run.me));
            }
            run.stage = Stage::Move { round: 0, from: now, news: false };
        }
        None => {
            run.watching = (0..run.players).find(|&p| !gone(p));
            run.messages.push((data.text("ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE"), now));
            run.stage = Stage::Ending { until: now + ENDING };
        }
    }
}

/// challenge_running's timerexpired on the attempter's game, Skate 3's rule (user 10-28; S3 hall_of_meat.lua
/// 887-901): the first time up, unless this is the match's last run (LastRun), the turn runs on END_RUN more seconds
/// (EndRunTimerID: still armed, so a bail over by then scores) and this returns true; else the attempt is over:
/// end_turn's BETWEEN, false.
fn overtime(run: &mut Run, round: u32, last: bool, now: f32) -> bool {
    if run.buzzer.is_none() && !last {
        run.buzzer = Some(now + END_RUN);
        run.stage = Stage::Turn { round, ends: now + END_RUN };
        return true;
    }
    run.stage = Stage::Next { round, at: now + BETWEEN };
    false
}

fn run_name(world: &World) -> String {
    world.get_resource::<crate::presence::Presence>().map_or_else(|| "The Legend".into(), |p| p.display_name())
}

fn start_turn(world: &mut World, hom: &mut HallOfMeat, row: &Row, round: u32, now: f32) -> Stage {
    hom.armed = true;
    (hom.bail, hom.port) = (None, None);
    hom.ended = false;
    world.resource_mut::<crate::skate2_arrows::Arrows>().wanted =
        row.indicators.iter().map(|p| (p.locator.clone(), p.matrix)).collect();
    Stage::Turn { round, ends: now + row.time_limit }
}

/// The bail score panel (homscoring): shown at the bail (SetState 1; a match also sets the normal skin, 1000, and plays
/// its transition sting, reloading the movie), refreshed while the score climbs (SetState 3: UpdateScoring reads the
/// HOM_* natives answered here). `panel` is its last refresh.
fn panel(world: &mut World, hom: &HallOfMeat, data: &Data, panel: &mut Option<Shown>, now: f32, open: bool, intro: bool) {
    // Under the Skate 2 physics switch the port's bail (Task 18), else the engine's: (total, tweaks, cars listed).
    let (total, tweaks, listed) = match (&hom.port, &hom.bail) {
        (Some(port), _) => (i64::from(port.hud.score), u32::from(port.hud.new_body_tweak), port.hud.new_car_collisions as usize),
        (None, Some(bail)) => (bail.total(&data.rules), bail.tweaks, bail.vehicles.len()),
        (None, None) => return,
    };
    let shown = panel.map_or((f32::MIN, -1, 0, 0), |p| p);
    if !open && (now - shown.0 < PANEL_EVERY || (total, tweaks, listed) == (shown.1, shown.2, shown.3)) {
        return;
    }
    let new_tweak = tweaks > shown.2;
    let cars = shown.3;
    *panel = Some((now, total, tweaks, listed));
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    let layer = screens.layer(HALL_OF_MEAT);
    let result = (|| -> Result<(), String> {
        let fresh = layer.get(SCORING).is_none() || (open && intro);
        if fresh {
            layer.open(SCORING, "homscoring", &[])?;
        }
        if open || fresh {
            let screen = layer.get(SCORING).ok_or("homscoring closed")?;
            if intro {
                screen.call("SetState", &[json!(1000)])?;
            }
            screen.call("SetState", &[json!(1)])?;
            if intro {
                screen.call("PlayIntroAudio", &[])?;
            }
        }
        let screen = layer.get(SCORING).ok_or("homscoring closed")?;
        let text = |id: &str| screen.bindings.movie.text_assets.localize(id);
        let imperial = crate::hud_runtime::imperial();
        let answers = match (&hom.port, &hom.bail) {
            (Some(port), _) => {
                let mut answers = port_answers(port, imperial, &text);
                answers.push(("FELanguage.GetLocaleText(ID_COMMON_METRIC_TIME)".to_owned(), json!(data.text("ID_COMMON_METRIC_TIME"))));
                answers
            }
            (None, Some(bail)) => panel_answers(data, bail, (new_tweak, cars), imperial, text),
            (None, None) => return Ok(()),
        };
        update_panel(screen, answers)
    })();
    if let Err(error) = result {
        warn!("Skate 2 Hall of Meat: homscoring: {error}");
        layer.close(SCORING);
    }
}

/// What homscoring's UpdateScoring asks (HUDComponents.HOM_*), for `bail` so far. Speed and drop come as the text its
/// ID_HOM_SPEED_VALUE '{0:speed}' / ID_COMMON_METRIC_METERS '{0:distance}' make ('%s' in the port): the exe's formatters
/// (0x8246F620 x3.6 Km/h or x2.25 MPH, 0x8246F4F0 m or x3.0 ft; one place) with Settings > Units and the movie's text.
/// The panel's last refresh: (when, total, tweaks, cars on the list).
type Shown = (f32, i64, u32, usize);

/// The car hits past the first `shown` of the list: how many, and each one's points (a popup apiece).
fn car_popups(bail: &Bail, shown: usize) -> (usize, Vec<u32>) {
    let new: Vec<u32> = bail.vehicles.iter().skip(shown).map(|v| v.2).collect();
    (new.len(), new)
}

/// `(new_tweak, cars)`: a tweak completed since the last refresh, and the cars on the list then.
fn panel_answers(data: &Data, bail: &Bail, (new_tweak, cars): (bool, usize), imperial: bool, text: impl Fn(&str) -> String) -> Vec<(String, Json)> {
    let rules = &data.rules;
    let new_cars = car_popups(bail, cars);
    let time = |s: f32| format!("{s:.2}");
    // 0x823c6af8: the start's speed, the best air's drop and time, the bail time, the most rotation.
    let data_block = [
        json!(crate::hud_runtime::speed(bail.speed, 1, imperial, &text)),
        json!(crate::hud_runtime::distance(bail.best.drop, 1, imperial, &text)),
        json!(time(bail.best.frames as f32 * DT)),
        json!(time(bail.unsettled as f32 * DT)),
        json!(format!("{:.0}", bail.max_rotation)),
    ];
    let mut answers = vec![
        ("HUDComponents.HOM_GetScore".to_owned(), json!(bail.total(rules))),
        ("HUDComponents.HOM_GetBonusData".to_owned(), json!(data_block)),
        ("HUDComponents.HOM_GetBonusScores".to_owned(), json!(bail.bonus(rules))),
        // [cars, DMOs, tweak, pedestrians, skaters]. ponytail: objects, pedestrians and skaters hit aren't scored (no
        // signal); the cars (traffic.rs) and the body tweak are.
        ("HUDComponents.HOM_GetBonusCollisionData".to_owned(), json!([new_cars.0, 0, new_tweak, 0, 0])),
        ("HUDComponents.HOM_GetBonusCollisionScore".to_owned(), json!([new_cars.1, [], rules_tweak_points(rules), [], []])),
        ("FELanguage.GetLocaleText(ID_COMMON_METRIC_TIME)".to_owned(), json!(data.text("ID_COMMON_METRIC_TIME"))),
    ];
    for t in [&data_block[2], &data_block[3]] {
        let t = t.as_str().unwrap_or_default();
        answers.push((format!("FELanguage.GetLocalizedTime({t})"), json!(format!("#{t}"))));
    }
    answers
}

/// What homscoring's UpdateScoring asks (HUDComponents.HOM_*, as [`panel_answers`]) from the port's own bail numbers
/// (`Frame.hom.hud`, the port's HOMScoringHUDData, field for field: spec 3.3) while Skate 2 physics has the skater
/// (Task 18): the metrics in the port's order (HomHudData::bonus_data / bonus_scores, 0x826ba760 / 0x826ba688; the
/// scores truncated to APT integers), and per entity this tick's new hits with their points (the front of the port's
/// arrays). The metric-time text is the caller's ([`panel`]). Not tested in play. shortcut: a hit or tweak in a tick
/// the 0.1 s refresh skips gets no popup (its points are in the score); accumulate them in [`port_tick`] if it shows in
/// play.
pub(crate) fn port_answers(hom: &skate2_physics_protocol::Hom, imperial: bool, text: impl Fn(&str) -> String) -> Vec<(String, Json)> {
    let hud = &hom.hud;
    let time = |s: f32| format!("{s:.2}");
    let data_block = [
        json!(crate::hud_runtime::speed(hud.wipeout_speed, 1, imperial, &text)),
        json!(crate::hud_runtime::distance(hud.max_air_drop, 1, imperial, &text)),
        json!(time(hud.max_air_time)),
        json!(time(hud.wipeout_duration)),
        json!(format!("{:.0}", hud.rotation)),
    ];
    let scores = [hud.wipeout_speed_score, hud.max_air_drop_score, hud.max_air_time_score, hud.wipeout_duration_score, hud.rotation_score]
        .map(|s| s as i32);
    let new = |count: u32, points: &[f32; 10]| points[..count.min(10) as usize].iter().map(|&p| p as i32).collect::<Vec<_>>();
    let mut answers = vec![
        ("HUDComponents.HOM_GetScore".to_owned(), json!(hud.score)),
        ("HUDComponents.HOM_GetBonusData".to_owned(), json!(data_block)),
        ("HUDComponents.HOM_GetBonusScores".to_owned(), json!(scores)),
        // [cars, DMOs, tweak, pedestrians, skaters], as panel_answers.
        (
            "HUDComponents.HOM_GetBonusCollisionData".to_owned(),
            json!([hud.new_car_collisions, hud.new_dmo_collisions, hud.new_body_tweak, hud.new_ped_collisions, hud.new_skater_collisions]),
        ),
        (
            "HUDComponents.HOM_GetBonusCollisionScore".to_owned(),
            json!([
                new(hud.new_car_collisions, &hud.car_scores),
                new(hud.new_dmo_collisions, &hud.dmo_scores),
                hud.tweak as i32,
                new(hud.new_ped_collisions, &hud.ped_scores),
                new(hud.new_skater_collisions, &hud.skater_scores)
            ]),
        ),
    ];
    for t in [&data_block[2], &data_block[3]] {
        let t = t.as_str().unwrap_or_default();
        answers.push((format!("FELanguage.GetLocalizedTime({t})"), json!(format!("#{t}"))));
    }
    answers
}

/// SetState 3 (UpdateScoring) twice: a metric the first call turns on only gets its score text from the next
/// (the C++ HUD calls it every frame); the second call sees no new tweak, so the sticker isn't doubled.
fn update_panel(screen: &mut crate::apt_screen::Screen, answers: Vec<(String, Json)>) -> Result<(), String> {
    for (key, value) in answers {
        screen.answer(&key, value);
    }
    screen.call("SetState", &[json!(3)])?;
    screen.answer("HUDComponents.HOM_GetBonusCollisionData", json!([0, 0, false, 0, 0]));
    screen.call("SetState", &[json!(3)]).map(drop)
}

/// The sticker's points (0x823c6af8 shows the completed tweak's; ponytail: the first's, as every disc tweak is 500).
fn rules_tweak_points(rules: &Rules) -> u32 {
    rules.tweaks.first().map_or(0, |t| t.points)
}

fn hide_panel(world: &mut World, panel: &mut Option<Shown>) {
    *panel = None;
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    if let Some(screen) = screens.layer(HALL_OF_MEAT).get(SCORING) {
        // ponytail: SetState 2 plays the panel's outro; it is closed with the run (or reopened at the next bail).
        let _ = screen.call("SetState", &[json!(2)]);
    }
}

/// LeaderboardHUD (HUD2.LeaderboardHUDInit(name, 0, players): layout IntStringInt): the match's standings online
/// (the core's), else the player's own best.
fn leaderboard(world: &mut World, run: &mut Run, title: &str, name: &str, show: bool) {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let standings: Vec<(String, i64, bool)> = if online && !m.standings.is_empty() {
        m.standings.iter().map(|s| (s.name.clone(), s.score, s.local)).collect()
    } else if run.party {
        // Every player's best, the turn under way's included; the one on the spot highlighted.
        let mut turns = run.turns.clone();
        turns.insert(run.k, run.best);
        let names = |p: usize| run.names.get(p).cloned().unwrap_or_default();
        placing(run.players, &turns, &[]).into_iter().map(|(p, best)| (names(p), best, p == run.active)).collect()
    } else {
        vec![(name.to_owned(), run.best, true)]
    };
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    let layer = screens.layer(HALL_OF_MEAT);
    if !show {
        return;
    }
    let reopen = layer.get(LEADERBOARD).is_none() || standings.len() != run.standings.len();
    if !reopen && standings == run.standings {
        return;
    }
    let answers = leaderboard_answers(title, online, &standings);
    let result = (|| -> Result<(), String> {
        if reopen {
            let screen = layer.open(LEADERBOARD, "leaderboardhud", &answers)?;
            screen.call("SetState", &[json!(3)])?;
            screen.call("SetState", &[json!(1)]).map(drop)
        } else {
            crate::skate2_contests::refresh_leaderboard(layer.get(LEADERBOARD).ok_or("leaderboardhud closed")?, answers)
        }
    })();
    if let Err(error) = result {
        warn!("Skate 2 Hall of Meat: leaderboardhud: {error}");
        layer.close(LEADERBOARD);
    }
    run.standings = standings;
}

/// What leaderboardhud's SetupScreen / RefreshAll ask, for (name, score, local) rows in rank order.
fn leaderboard_answers(title: &str, online: bool, standings: &[(String, i64, bool)]) -> [(&'static str, Json); 9] {
    use crate::skate2_contests::score_text;
    let rows: Vec<_> = standings.iter().map(|(name, score, local)| (name.clone(), *score, score_text(0, *score as f64), *local)).collect();
    crate::skate2_contests::leaderboard_answers(&format!("#{title}"), online, 0, "ID_COMMON_UNKNOWN", &rows, None)
}

/// Effects.StartEffect(FadeOut, TimeToTeleport) at wipeoutalmostover, and back in after the teleport: a black veil
/// under the HUD. ponytail: the effect slot (0x82b7faf0) is taken as a linear fade to black; not traced further.
fn veil(world: &mut World, hom: &mut HallOfMeat, alpha: f32) {
    let entity = match hom.veil {
        Some(e) if world.get_entity(e).is_ok() => e,
        _ if alpha <= 0. => return,
        _ => {
            let e = world
                .spawn((
                    Veil,
                    Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
                    BackgroundColor(Color::NONE),
                    GlobalZIndex(0),
                    Pickable::IGNORE,
                ))
                .id();
            hom.veil = Some(e);
            e
        }
    };
    if let Some(mut color) = world.get_mut::<BackgroundColor>(entity) {
        let next = BackgroundColor(Color::BLACK.with_alpha(alpha));
        color.set_if_neq(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C2: an S3 HOM Battle row is `Owned` (skate3_uniform plays it); Skate 2's script must leave it alone, or its
    /// missing-row path finishes the match every frame.
    #[test]
    fn an_owned_hom_row_is_not_skate_2s() {
        let mut m = Match { mode: Some(Mode::HallOfMeat), phase: Phase::Running { started: 0., ends: None }, ..default() };
        assert!(plays(&m), "a Skate 2 row");
        m.owned = true;
        assert!(!plays(&m), "an S3 (or S1) row its own mode owns");
        m.owned = false;
        m.phase = Phase::Idle;
        assert!(!plays(&m), "no match");
    }

    /// Bug 11-01: Party Play's start travels to New_San_Vanelona behind the loading screen, and the flythrough, started
    /// with the match, played out behind it. The run starts once the trip is over, the core's intro waiting for it.
    #[test]
    fn the_flythrough_waits_for_the_trip_to_the_map() {
        let row = Row {
            title: None,
            time_limit: 30.,
            num_rounds: 1,
            attempt: None,
            waiting: None,
            intro: Some("hom".into()),
            intro_seconds: Some(5.),
            indicators: vec![],
        };
        let data = Data { rules: rules(), rows: [("hom".to_owned(), row)].into(), strings: HashMap::new() };
        let mut world = World::new();
        world.insert_resource(Match { mode: Some(Mode::HallOfMeat), row: "hom".into(), phase: Phase::Intro { since: 0. }, ..default() });
        world.init_resource::<crate::skate2_arrows::Arrows>();
        world.init_resource::<Cutscene>();
        // The rest of the step: its screens read the asset root and the clock.
        world.init_resource::<Time<Real>>();
        world.insert_resource(Config {
            asset_root: PathBuf::new(),
            verification_capture: None,
            map: None,
            map_path: None,
            difficulty: Default::default(),
            check_assets: false,
            start_paused: false,
            multiplayer: Default::default(),
            map_fingerprint: 0,
            teleport: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
        });
        let current = |map: &str| crate::map_transition::CurrentMap {
            path: Some(format!("maps/{map}.skate").into()),
            name: String::new(),
            spawn: [0.; 3],
            heading: 0.,
            generation: 0,
        };
        world.insert_resource(current("Skate_1_San_Vanelona"));
        let (mut hom, phase) = (HallOfMeat::default(), Phase::Intro { since: 0. });
        step(&mut world, &mut hom, &data, &phase, 0.);
        assert!(hom.run.is_none() && !world.resource::<Cutscene>().active(), "no flythrough behind the loading screen");
        assert!(world.resource::<Match>().intro, "the core's intro waits for it");
        world.insert_resource(current("New_San_Vanelona"));
        step(&mut world, &mut hom, &data, &phase, 0.);
        assert!(hom.run.is_some() && world.resource::<Cutscene>().active(), "the flythrough starts on the map");
    }

    #[test]
    fn turns_go_round_by_round_in_slot_order_past_who_left() {
        assert_eq!(schedule(0, 0, 3, 2, |_| false), Some((0, 1, 0)));
        assert_eq!(schedule(3, 0, 3, 2, |_| false), Some((3, 2, 0)), "round 2 starts with slot 0 again");
        assert_eq!(schedule(1, 0, 3, 2, |p| p == 1), Some((2, 1, 2)), "a player who left is skipped");
        assert_eq!(schedule(6, 0, 3, 2, |_| false), None, "every turn taken");
        assert_eq!(schedule(2, 0, 1, 3, |_| false), Some((2, 3, 0)), "solo: one turn a round");
        assert_eq!(schedule(0, 0, 2, 3, |_| true), None, "no one left");
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 3): the first player is drawn (`first`, from the match token) and
    /// every round starts again from it (S3 hall_of_meat.lua 143-144, 263): first 2 of 3 plays 2, 0, 1, 2, 0, 1.
    #[test]
    fn turns_start_from_the_drawn_first_player_every_round() {
        let mut k = 2;
        let mut turns = Vec::new();
        while let Some((at, round, p)) = schedule(k, 2, 3, 2, |_| false) {
            turns.push((round, p));
            k = at + 1;
        }
        assert_eq!(turns, [(1, 2), (1, 0), (1, 1), (2, 2), (2, 0), (2, 1)]);
        assert_eq!(schedule(2, 2, 3, 2, |p| p == 2), Some((3, 1, 0)), "the drawn player gone: the next one starts");
        assert_eq!(schedule(0, 2, 3, 2, |_| false), Some((2, 1, 2)), "never before the first");
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 6): time up, the turn runs on 2 s more (EndRunTimerID), still armed;
    /// then end_turn. On the match's last run (LastRun) end_turn comes at once (S3 hall_of_meat.lua 887-901).
    #[test]
    fn time_up_runs_on_two_seconds_except_on_the_last_run() {
        let mut run = online_run(Stage::Turn { round: 1, ends: 30. });
        assert!(overtime(&mut run, 1, false, 30.), "still armed");
        assert_eq!((run.stage, run.buzzer), (Stage::Turn { round: 1, ends: 30. + END_RUN }, Some(30. + END_RUN)));
        assert!(!overtime(&mut run, 1, false, 30. + END_RUN));
        assert_eq!(run.stage, Stage::Next { round: 1, at: 30. + END_RUN + BETWEEN });
        let mut run = online_run(Stage::Turn { round: 2, ends: 30. });
        assert!(!overtime(&mut run, 2, true, 30.), "the last run");
        assert_eq!((run.stage, run.buzzer), (Stage::Next { round: 2, at: 30. + BETWEEN }, None));
    }

    /// countdown stateenter (hall_of_meat.lua 304-385): the next player to the attempt spawn's slot 0 and the last one
    /// back to its waiting slot in one RepositionAI fade (the messages and countdown wait for it); after the last round
    /// the last attempter goes back to its own attempt slot, or, gone, the camera goes to the next player and it ends.
    #[test]
    fn turns_change_behind_a_reposition_fade() {
        let m = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let place = |x: f32| Place { locator: String::new(), matrix: m(x), slots: (1..=4).map(|k| m(x + k as f32)).collect() };
        let (attempt, waiting) = (Some(place(100.)), Some(place(200.)));
        let row = Row { title: None, time_limit: 30., num_rounds: 1, attempt, waiting, intro: None, intro_seconds: None, indicators: vec![] };
        let data = Data { rules: rules(), rows: HashMap::new(), strings: HashMap::new() };
        let mut run = online_run(Stage::Next { round: 1, at: 0. });
        next_turn(&mut run, &row, &data, |_| false, 1, 5.);
        assert_eq!((run.active, run.stage, run.teleport), (1, Stage::Move { round: 1, from: 5., news: false }, Some(m(201.))));
        assert!(run.messages.is_empty(), "the turn's messages wait for the fade");
        run.me = 2;
        next_turn(&mut run, &row, &data, |_| false, 1, 9.);
        assert_eq!((run.active, run.teleport), (2, Some(m(101.))), "the attempt spawn's child 0");
        next_turn(&mut run, &row, &data, |_| false, 1, 13.);
        assert_eq!((run.stage, run.teleport), (Stage::Move { round: 0, from: 13., news: false }, Some(m(103.))), "its own slot");
        run.me = 0;
        next_turn(&mut run, &row, &data, |p| p == 2, 1, 13.);
        assert_eq!((run.stage, run.watching), (Stage::Ending { until: 13. + ENDING }, Some(0)));
    }

    /// Three players online, this console player 0, the pause menu still off (initial).
    fn online_run(stage: Stage) -> Run {
        Run {
            row: String::new(),
            stage,
            best: 0,
            messages: vec![],
            teleport: None,
            k: 0,
            active: 0,
            me: 0,
            players: 3,
            online: true,
            first: 0,
            buzzer: None,
            over: BTreeSet::new(),
            dark: false,
            watching: None,
            pause_ok: false,
            panel: None,
            standings: vec![],
            turns: BTreeMap::new(),
            voting: true,
            votes: vec![1; 3],
            vote_shown: None,
            best_voted: None,
            party: false,
            names: vec![],
            carded: None,
            answered: None,
        }
    }

    /// Party Play: one skater, three players; every turn's fade ends on its Press-A card (the countdown waits for its
    /// A), the next player takes the skater to the attempt spawn with that player's own best, and the results go by
    /// each player's best bail.
    #[test]
    fn party_turns_hand_over_behind_their_card() {
        let m = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let place = |x: f32| Place { locator: String::new(), matrix: m(x), slots: (1..=4).map(|k| m(x + k as f32)).collect() };
        let row = Row { title: None, time_limit: 30., num_rounds: 2, attempt: Some(place(100.)), waiting: Some(place(200.)), intro: None, intro_seconds: None, indicators: vec![] };
        let data = Data { rules: rules(), rows: HashMap::new(), strings: HashMap::new() };
        let names = vec!["Player 1".to_owned(), "Player 2".to_owned(), "Player 3".to_owned()];
        let mut run = Run { online: false, voting: false, party: true, names, ..online_run(Stage::Move { round: 1, from: 0., news: true }) };
        assert_eq!(card_due(&run, MOVE - 0.1), None, "not before the fade is over");
        assert_eq!(card_due(&run, MOVE), Some((1, 1)));
        after_move(&mut run, &data, "Player 1", MOVE + 5.);
        assert!(matches!(run.stage, Stage::Move { .. }), "the countdown waits for the card's A");
        run.answered = Some(run.k);
        after_move(&mut run, &data, "Player 1", MOVE + 5.);
        assert!(matches!(run.stage, Stage::Countdown { round: 1, .. }));
        // Player 1 bails for 700; the next turn is Player 2's, on the attempt spawn, from 0.
        run.best = 700;
        turn_over(&mut run, |_| false);
        next_turn(&mut run, &row, &data, |_| false, 1, 50.);
        assert_eq!((run.k, run.active, run.me, run.best, run.teleport), (1, 1, 1, 0, Some(m(101.))));
        assert_eq!(card_due(&run, 50. + MOVE), Some((2, 1)), "a new card for the new turn");
        run.best = 900;
        turn_over(&mut run, |_| false);
        next_turn(&mut run, &row, &data, |_| false, 1, 60.);
        turn_over(&mut run, |_| false);
        next_turn(&mut run, &row, &data, |_| false, 1, 70.);
        assert_eq!((run.k, run.active, run.best), (3, 0, 700), "round 2: Player 1 again, with its best");
        assert_eq!(card_due(&run, 70. + MOVE), Some((1, 2)));
        assert_eq!(placing(3, &run.turns, &[]), [(1, 900), (0, 700), (2, 0)]);
    }

    /// GAP-9 (online audit 10-28): reset hides the trick display for the whole Hall of Meat (hall_of_meat.lua 266-268)
    /// until challenge_cleanup's DeleteGroup: the flythrough before reset and the results after cleanup show it.
    #[test]
    fn hall_of_meat_hides_the_trick_display_from_reset_to_cleanup() {
        for stage in [
            Stage::Move { round: 1, from: 0., news: true },
            Stage::Countdown { round: 1, ends: 3. },
            Stage::Turn { round: 1, ends: 30. },
            Stage::Bailing { round: 1 },
            Stage::Next { round: 1, at: 2. },
            Stage::Watch { round: 2 },
            Stage::Ending { until: 2. },
        ] {
            assert!(trick_display_hidden(stage, false), "{stage:?}");
        }
        assert!(!trick_display_hidden(Stage::Intro { ends: 5. }, false), "the flythrough comes before reset");
        assert!(!trick_display_hidden(Stage::Done, false), "cleanup's DeleteGroup");
        assert!(!trick_display_hidden(Stage::Turn { round: 1, ends: 30. }, true), "the results");
    }

    /// GAP-6 (online audit 10-28): online with 3+ players the waiting players vote on each attempt (hall_of_meat.lua
    /// 162, 12, 684, 706-731): hudonlinevoting is up for them only, each vote reaches the attempter's game, whose
    /// StopVoting relays the mean in its TurnOver, and a mean over 1 that beats the best so far is every game's Player's
    /// Choice.
    #[test]
    fn three_players_vote_and_every_game_names_the_players_choice() {
        const A: u16 = 0x1000;
        let mut runs: Vec<Run> = (0..3).map(|me| Run { me, ..online_run(Stage::Watch { round: 1 }) }).collect();
        runs[0].stage = Stage::Turn { round: 1, ends: 30. };
        assert_eq!(runs.iter().map(voting_hud).collect::<Vec<_>>(), [None, Some(1), Some(1)], "the attempter doesn't vote");
        assert!(cast_vote(&mut runs[0], A).is_none());
        // Player 1 votes gnarly (A twice), player 2 sweet (A once); the attempter's own and another turn's are dropped.
        let mut wire = Vec::new();
        for (me, presses) in [(1, 2), (2, 1)] {
            for _ in 0..presses {
                let vote = cast_vote(&mut runs[me], A).unwrap();
                wire.push((me, serde_json::to_string(&vote).unwrap()));
            }
        }
        assert!(cast_vote(&mut runs[1], 0).is_none(), "no press, no vote");
        assert_eq!(voting_hud(&runs[1]), Some(3));
        wire.push((0, serde_json::to_string(&Vote { hom_vote: 0, level: 0 }).unwrap()));
        wire.push((2, serde_json::to_string(&Vote { hom_vote: 3, level: 0 }).unwrap()));
        hear(&mut runs[0], wire);
        assert_eq!(runs[0].votes, [1, 3, 2]);
        let over = turn_over(&mut runs[0], |_| false);
        assert_eq!((over.voted, runs[0].best_voted), (Some(2.5), Some((0, 2.5))));
        let relay = serde_json::to_string(&over).unwrap();
        for c in [1, 2] {
            hear(&mut runs[c], vec![(0, relay.clone())]);
            assert_eq!(runs[c].best_voted, Some((0, 2.5)), "game {c}");
        }
        // Later turns: a mean of 1, or one no better, names no one new; a better one does; a mean past 3 is no one's.
        let turn = |k: usize, voted: f32| (k % 3, serde_json::to_string(&TurnOver { hom_turn: k, best: 0, voted: Some(voted) }).unwrap());
        hear(&mut runs[2], vec![turn(1, 1.), turn(2, 2.5), turn(4, 9.)]);
        assert_eq!(runs[2].best_voted, Some((0, 2.5)));
        hear(&mut runs[2], vec![turn(4, 3.)]);
        assert_eq!(runs[2].best_voted, Some((1, 3.)));
        // An attempter who left sends no TurnOver: a waiting game ends the vote on its own votes.
        let mut run = Run { me: 2, k: 4, active: 1, best_voted: Some((0, 2.5)), ..online_run(Stage::Watch { round: 2 }) };
        run.votes = vec![3, 1, 3];
        left_turn(&mut run, |p| p == 1);
        assert_eq!(run.best_voted, Some((1, 3.)), "the gone attempter's mean, from this game's votes");
        run.best_voted = None;
        run.over.insert(4);
        left_turn(&mut run, |p| p == 1);
        assert_eq!(run.best_voted, None, "its TurnOver came: the relayed mean stands");
    }

    /// RepositionAI's fade in a turn change (hall_of_meat.lua 112-118, 304-427, 582-593): the camera switches to the
    /// next player at black; its messages, the pause menu and the countdown come at teleporttransitionfinished; the
    /// attempter's wipeout FadeOut stays black until that black; after the last round, challenge_finished's 2 s with the
    /// pause menu off.
    #[test]
    fn a_turn_starts_when_its_fade_is_over() {
        let data = Data { rules: rules(), rows: HashMap::new(), strings: HashMap::new() };
        let mut run = online_run(Stage::Turn { round: 1, ends: 30. });
        assert_eq!(screen(&mut run, Some((0.25, 1.)), 4.), 0.75, "wipeoutalmostover's FadeOut");
        run.stage = Stage::Move { round: 2, from: 5., news: true };
        run.active = 1;
        assert_eq!(screen(&mut run, None, 5.25), 1., "black held into the next fade");
        assert!(!at_black(&mut run, 5.4) && run.watching.is_none());
        assert!(at_black(&mut run, 5.5) && run.watching == Some(1) && !run.dark, "the cut");
        assert_eq!(screen(&mut run, None, 6.75), 0.5, "fading in");
        after_move(&mut run, &data, "Bo", 5. + MOVE - 0.5);
        assert!(run.messages.is_empty() && pause_disabled(&run, false));
        after_move(&mut run, &data, "Bo", 5. + MOVE);
        assert_eq!(run.stage, Stage::Countdown { round: 2, ends: 5. + MOVE + COUNTDOWN });
        assert_eq!(run.messages.len(), 2, "your turn and the round");
        assert!(!pause_disabled(&run, false));
        run.stage = Stage::Move { round: 0, from: 20., news: false };
        after_move(&mut run, &data, "Bo", 20. + MOVE);
        assert_eq!(run.stage, Stage::Ending { until: 20. + MOVE + ENDING });
        assert!(pause_disabled(&run, false) && !pause_disabled(&run, true));
    }

    /// I3 (online audit 10-06): a dismount after a full bail. The bail's reset leaves `teleport_pending` up (only the
    /// next bail's `enter` clears it) and a free board raises the board manager's flag, which the old gate read as
    /// wipeoutalmostover: the FadeOut went black for the rest of the turn. Only a bail's own reset countdown fades.
    /// `tick` runs on the engine's own skater (the mode harness).
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
    fn a_dismount_after_a_bail_does_not_fade_the_turn_out() {
        use crate::physics::{
            GamePhysics,
            mode_harness::{Harness, Spawn, asset_root, pads},
        };
        use bevy::ecs::system::RunSystemOnce;
        use skate_core::input::xbox::XboxState;
        /// Ticks the harness and runs `tick` after each tick: per tick (the board manager's flag, the reset countdown's
        /// flag, a FadeOut asked for).
        fn run(mut h: Harness, w: &mut World, ticks: u64, pad: impl Fn(u64) -> XboxState) -> (Harness, Vec<[bool; 3]>) {
            let mut out = Vec::new();
            for t in 0..ticks {
                let s = h.tick(pad(t));
                w.insert_resource(h.physics);
                w.insert_resource(h.skater);
                w.insert_resource(h.controls);
                w.run_system_once(tick).unwrap();
                h.physics = w.remove_resource::<GamePhysics>().unwrap();
                h.skater = w.remove_resource::<SkaterRuntime>().unwrap();
                h.controls = w.remove_resource::<PlayerControls>().unwrap();
                out.push([s.board_wiping_out, s.teleport_pending, w.resource::<HallOfMeat>().fade_out.is_some()]);
            }
            (h, out)
        }
        let mut world = World::new();
        let data = Data::load(&asset_root().join(ROOT)).unwrap();
        world.insert_resource(HallOfMeat { data: Some(Arc::new(data)), ..default() });
        let mut h = Harness::load("University", Spawn::Package);
        h.run(60, |_| pads::idle());
        let (h, bail) = run(h, &mut world, 600, |t| if t < 30 { pads::bail() } else { pads::idle() });
        let last = *bail.last().unwrap();
        assert!(bail.iter().any(|s| s[2]), "the bail's reset countdown fades out (wipeoutalmostover)");
        assert!(last[1] && !last[2], "after the reset: the flag stays up, no fade");
        let (_, dismount) = run(h, &mut world, 500, |t| if t == 20 || t == 300 { pads::buttons(pads::Y) } else { pads::idle() });
        assert!(dismount.iter().any(|s| s[0] && s[1]), "the old gate's case: a free board with the stale flag");
        assert!(dismount.iter().all(|s| !s[2]), "a dismount after a bail faded the turn out");
    }

    /// Every installed online row has the attempt spawn and a waiting slot for each of a match's 10 players.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/skate2-online/hall-of-meat"]
    fn installed_rows_have_their_attempt_and_waiting_slots() {
        let root = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join(ROOT);
        let data = Data::load(&root).unwrap();
        assert!(!data.rows.is_empty());
        for (id, row) in &data.rows {
            assert!(row.attempt.is_some(), "{id}: no attempt spawn");
            let waiting = row.waiting.as_ref().unwrap_or_else(|| panic!("{id}: no waiting spot"));
            assert!(!waiting.slots.is_empty(), "{id}: no waiting slots");
            assert!((0..10).all(|i| waiting.slot(i)[3][1] != 0. || waiting.matrix[3][1] == 0.), "{id}");
        }
    }

    fn curve(x: [f32; 8], y: [f32; 8]) -> Curve {
        Curve { x, y }
    }

    /// Two bones: 0 passes half to 1, 1 passes a quarter back (never back to where it came from).
    fn rules() -> Rules {
        let flat = curve([0., 1., 2., 3., 4., 5., 6., 7.], [0.; 8]);
        let mut bones: Vec<Bone> = (0..BONES).map(|_| Bone { links: vec![], levels: vec![(0, 0.); 6] }).collect();
        bones[0] = Bone { links: vec![(1, 0.5)], levels: vec![(100, 5.), (200, 10.), (0, 0.), (0, 0.), (1000, 30.), (0, 0.)] };
        bones[1] = Bone { links: vec![(0, 0.25), (2, 0.)], levels: vec![(50, 2.), (0, 0.), (0, 0.), (0, 0.), (0, 0.), (0, 0.)] };
        Rules {
            bones,
            curves: Curves { leg_impact: flat, air_time: flat, drop: flat, speed: flat, rotation: flat, bail_time: flat },
            force_scale: 0.75,
            toe_share: 0.4,
            tweak_stick: 0.6,
            tweaks: vec![
                Tweak { id: 0, stick: [0., 1.], frames: 15, points: 500 },
                Tweak { id: 2, stick: [1., 0.], frames: 15, points: 500 },
            ],
            vehicle_points: 1500,
            vehicles: vec![(12, 2000), (13, 2000)],
        }
    }

    #[test]
    fn each_car_hit_scores_once_by_its_type_and_eight_at_most() {
        let rules = rules();
        let mut f = frame(false);
        let mut bail = Bail::start(&rules, &f);
        bail.frame(&rules, &f);
        assert_eq!(bail.vehicle_points, 0);
        // 0x82bb8a90: a Truck (type 12) 2000, any other car vehicle_points.
        f.vehicles[0] = Some((7, 12));
        f.vehicles[1] = Some((8, -1));
        bail.frame(&rules, &f);
        assert_eq!(bail.vehicle_points, 3500);
        // Still touching next frame: each car scores once.
        bail.frame(&rules, &f);
        assert_eq!(bail.vehicle_points, 3500);
        // 0x82bb7410: the cars' points are in the bonus, so in the total.
        let mut without = bail.clone();
        without.vehicle_points = 0;
        assert_eq!(bail.total(&rules) - without.total(&rules), 3500);
        // 0x82bb86f0: eight cars at most.
        for id in 100..110 {
            f.vehicles = [None; 8];
            f.vehicles[0] = Some((id, 9));
            bail.frame(&rules, &f);
        }
        assert_eq!(bail.vehicles.len(), 8);
        assert_eq!(bail.vehicle_points, 3500 + 6 * 1500);
    }

    #[test]
    fn the_panel_shows_the_ports_numbers() {
        use skate2_physics_protocol::{Hom, HomHud};
        let mut car_scores = [0.; 10];
        car_scores[0] = 500.;
        let hud = HomHud { score: 12345, car_scores, new_car_collisions: 1, ..default() };
        let port = Hom { bail_in_progress: true, hud, ..default() };
        let answers: HashMap<String, Json> = port_answers(&port, false, |id| id.to_owned()).into_iter().collect();
        assert_eq!(answers["HUDComponents.HOM_GetScore"], json!(12345));
        assert_eq!(answers["HUDComponents.HOM_GetBonusCollisionScore"][0][0], json!(500), "the car hit's points first");
        assert_eq!(answers["HUDComponents.HOM_GetBonusCollisionData"][0], json!(1));

        // As the engine's own bail: bail_in_progress starts it (the panel shows while `bailing`), bail_completed ends
        // it (`ended`: the panel goes and the match scores it), with the port's score.
        let mut hom = HallOfMeat { armed: true, ..default() };
        port_tick(&mut hom, &port);
        assert!(hom.bailing && !hom.ended, "the port's bail is on");
        assert!(hom.bail.is_none());
        port_tick(&mut hom, &port);
        assert!(hom.bailing, "still on");
        let done = Hom { bail_completed: true, hud: HomHud { score: 20000, ..port.hud.clone() }, ..default() };
        port_tick(&mut hom, &done);
        assert!(!hom.bailing && hom.ended, "the port's bail is over");
        assert_eq!(hom.port.as_ref().map(|p| p.hud.score), Some(20000), "the port's final score");
        // Nothing to score: no bail starts.
        let mut idle = HallOfMeat::default();
        port_tick(&mut idle, &port);
        assert!(!idle.bailing && idle.port.is_none());
    }

    #[test]
    fn the_panel_pops_each_new_car_hit_once() {
        let rules = rules();
        let mut f = frame(false);
        let mut bail = Bail::start(&rules, &f);
        f.vehicles[0] = Some((7, 13));
        bail.frame(&rules, &f);
        // homscoring's CollisionData / CollisionScore: [cars, DMOs, tweak, pedestrians, skaters].
        assert_eq!(car_popups(&bail, 0), (1, vec![2000]));
        assert_eq!(car_popups(&bail, 1), (0, vec![]));
    }

    /// A frame with every bone free of the ground (in the air) or not.
    fn frame(airborne: bool) -> Frame {
        Frame { contacts: [(0., airborne); BONES], rows: turned(0.), ..default() }
    }

    /// The trackers' rows ([Z, X, Y, T]) of a body turned `theta` about world Y.
    fn turned(theta: f32) -> [V; 4] {
        let (s, c) = theta.sin_cos();
        [[s, 0., c, 0.], [c, 0., -s, 0.], [0., 1., 0., 0.], [0., 0., 0., 1.]]
    }

    #[test]
    fn constants_are_the_exes() {
        assert_eq!(DT.to_bits(), 0x3C88_8889);
        assert_eq!(DEGREES.to_bits(), 0x4265_2EE1);
        assert_eq!((PI.to_bits(), TAU.to_bits()), (0x4049_0FDB, 0x40C9_0FDB));
    }

    #[test]
    fn curves_are_the_exes_evaluator() {
        // The disc's bail-time curve: nothing under 5 s, then a step to 750.
        let c = curve([0., 4.999, 5., 5.25, 5.5, 6., 8., 9.], [0., 0., 750., 1250., 2500., 5000., 10000., 15000.]);
        assert_eq!(c.at(-1.), 0.);
        assert_eq!(c.at(4.), 0.);
        assert_eq!(c.at(5.), 750.);
        assert_eq!(c.at(5.125), 1000.);
        assert_eq!(c.at(9.), 15000.);
        assert_eq!(c.at(100.), 15000.);
        assert_eq!(c.at(f32::NAN), 15000., "NaN fails `< last x`: the last y");
        assert_eq!(c.points(5.1), 949, "5.1f32 is 5.0999999: 949.9998, truncated");
        let step = curve([0., 1., 1., 2., 3., 4., 5., 6.], [0., 0., 750., 1000., 1000., 1000., 1000., 1000.]);
        assert_eq!(step.at(1.), 750., "a zero-width segment takes its upper value");
        // fmadds: one rounding (the disc's drop curve; an unfused multiply-add gives 1869.2937).
        let drop = curve([0., 6.999_99, 7., 10.5, 17.5, 30., 40., 45.], [0., 0., 750., 1250., 2500., 5000., 10000., 10000.]);
        assert_eq!(drop.at(f32::from_bits(0x415F_7D1D)).to_bits(), 0x44E9_A967);
    }

    #[test]
    fn acos_and_lengths_are_the_exes_polynomial_and_newton_steps() {
        for (x, bits) in [(0.5, 0x3F86_0A92), (-0.5, 0x4006_0A92), (0., 0x3FC9_0FDB), (1., 0x3500_0000), (-1., 0x4049_0FD9)] {
            assert_eq!(acos(x).to_bits(), bits, "acos({x})");
        }
        for i in -100..=100 {
            let x = i as f32 / 100.;
            assert!((acos(x) - x.acos()).abs() < 3e-6, "acos({x})");
        }
        assert_eq!(length([10., 0., 0., 0.]), 10., "exactly 10 m/s: the speed curve's 750, not 9.9999995's 746");
        assert_eq!(length([1., 2., 2., 9.]), 3., "xyz only");
        assert_eq!(length([0.; 4]), 0.);
        assert_eq!(angle_2d([0., 1.], [1., 0.]), PI * 0.5);
        assert_eq!(angle_2d([0., 1.], [0., 0.009]), 0., "shorter than 0.01");
    }

    #[test]
    fn force_is_the_velocity_change_along_the_normal_and_the_ground_is_an_up_world_contact() {
        assert_eq!(contact([1., -2., 3., 7.], [0., 1., 0., 0.], true), (2., false));
        assert_eq!(contact([1., -2., 3., 7.], [0., 1., 0., 0.], false), (2., true), "not a world contact");
        assert_eq!(contact([1., -2., 3., 7.], [0., 0.33, 0., 0.], true), (0.66, true), "not up enough");
        let rules = rules();
        let mut bail = Bail::start(&rules, &frame(true));
        let mut f = frame(true);
        bail.frame(&rules, &f);
        assert!(bail.airborne);
        f.contacts[7].1 = false;
        bail.frame(&rules, &f);
        assert!(!bail.airborne, "one bone on the ground");
    }

    #[test]
    fn damage_spreads_from_the_bone_and_grades_every_frame() {
        let rules = rules();
        let mut bail = Bail::start(&rules, &frame(false));
        let mut f = frame(false);
        f.contacts[0].0 = 16.; // 12 after the 0.75 force scale; 6 to bone 1, whose share back to 0 is not passed
        bail.frame(&rules, &f);
        assert_eq!((bail.damage[0], bail.damage[1]), (12., 6.));
        assert_eq!(bail.level[..2], [Some(1), Some(0)]);
        assert_eq!(bail.damage_points(&rules), 200 + 50, "bone 0 past Bruise (10), bone 1 past Scrape (2)");
        // 0.15 on bone 1: its quarter to bone 0 (0.0375) is under 0.1.
        f.contacts[0].0 = 0.;
        f.contacts[1].0 = 0.2;
        bail.frame(&rules, &f);
        assert_eq!(bail.damage[0], 12.);
        assert_eq!(bail.damage[1], 0.2 * 0.75 + 6.);
        // 3 on bone 1 passes 0.75 to bone 0, which can't pass it back.
        f.contacts[1].0 = 4.;
        bail.frame(&rules, &f);
        assert_eq!((bail.damage[0], bail.damage[1]), (12.75, 3. + (0.2 * 0.75 + 6.)));
        // The running sum crosses BreakPart (30); Sprain/Dislocate (0 thresholds) are skipped.
        f.contacts[1].0 = 0.;
        f.contacts[0].0 = 30.;
        bail.frame(&rules, &f);
        assert_eq!(bail.damage_points(&rules), 1000 + 50);
        // A threshold is passed only when exceeded.
        let mut rules = rules;
        rules.force_scale = 0.5;
        let mut bail = Bail::start(&rules, &frame(false));
        let mut f = frame(false);
        f.contacts[0].0 = 20.;
        bail.frame(&rules, &f);
        assert_eq!((bail.damage[0], bail.level[0]), (10., Some(0)));
    }

    #[test]
    fn the_feet_take_their_impacts_and_the_toes_toe_share() {
        let mut rules = rules();
        rules.curves.leg_impact = curve([0., 10., 20., 30., 40., 50., 60., 70.], [0., 10., 20., 30., 40., 50., 60., 70.]);
        let mut bail = Bail::default();
        bail.feet(&rules, 20., 0.);
        assert_eq!((bail.damage[15], bail.damage[14]), (15., 0.4 * 15.));
        assert_eq!((bail.damage[19], bail.damage[18]), (0., 0.));
        let start = Bail::start(&rules, &frame(false));
        assert!(start.damage.iter().all(|&d| d == 0.), "no foot signal: the start adds nothing");
    }

    #[test]
    fn airs_score_live_then_bank_into_sums_and_bests() {
        let mut rules = rules();
        rules.curves.air_time = curve([0., 0.5, 0.5, 1., 2., 3., 4., 5.], [0., 0., 100., 200., 300., 400., 500., 600.]);
        rules.curves.drop = curve([0., 1., 1., 2., 3., 4., 5., 6.], [0., 0., 10., 20., 30., 40., 50., 60.]);
        let mut bail = Bail::start(&rules, &frame(false));
        let mut f = Frame { stick: [0., 1.], ..frame(true) };
        for i in 0..60 {
            f.height = 5. - i as f32 * 0.05;
            bail.frame(&rules, &f);
        }
        // 60 frames = 1 s: 200; 2.95 m: 29; the tweak held up scored at its 16th frame.
        assert_eq!((bail.air.frames, bail.air.air_points, bail.air.drop_points), (60, 200, 29));
        assert_eq!((bail.tweaks, bail.tweak_points), (1, 500));
        assert_eq!(bail.bonus(&rules), [0; 5], "the panel's metrics show landed airs only");
        assert_eq!(bail.total(&rules), 200 + 29 + 500, "the total counts the air under way");
        // Landing: one more air frame (61 frames, 3 m), banked.
        f = Frame { height: 2., stick: [0.7, 0.], ..frame(false) };
        bail.frame(&rules, &f);
        let first = Air { frames: 61, air_points: 201, drop_points: 30, drop: 3. };
        assert_eq!((bail.air, bail.landed, bail.best), (Air::default(), first, first));
        for _ in 0..20 {
            bail.frame(&rules, &f);
        }
        assert_eq!(bail.tweaks, 1, "tweaks only score in the air");
        assert_eq!(bail.bonus(&rules), [0, 30, 201, 0, 0]);
        // A second, shorter air: 30 frames in the air and the landing's (0.52 s: 103), no drop.
        f = Frame { height: 1., ..frame(true) };
        for _ in 0..30 {
            bail.frame(&rules, &f);
        }
        bail.frame(&rules, &Frame { height: 1., ..frame(false) });
        let second = Air { frames: 31, air_points: 103, drop_points: 0, drop: 0. };
        assert_eq!(bail.landed, Air { frames: 92, air_points: 304, drop_points: 30, drop: 3. });
        assert_eq!(bail.best, first, "{second:?} beats it at nothing");
        assert_eq!(bail.total(&rules), 500 + 30 + 304);
    }

    #[test]
    fn tweaks_score_once_per_hold_and_re_arm() {
        let rules = rules();
        let mut bail = Bail::start(&rules, &frame(false));
        let up = Frame { stick: [0., 1.], ..frame(true) };
        for _ in 0..15 {
            bail.frame(&rules, &up);
        }
        assert_eq!(bail.tweaks, 0, "15 frames: held 14 past the first");
        bail.frame(&rules, &up);
        assert_eq!(bail.tweaks, 1);
        for _ in 0..30 {
            bail.frame(&rules, &up);
        }
        assert_eq!(bail.tweaks, 1, "once per hold");
        bail.frame(&rules, &Frame { stick: [0., 0.5], ..frame(true) }); // under tweak_stick: let go
        for _ in 0..16 {
            bail.frame(&rules, &up);
        }
        assert_eq!(bail.tweaks, 2, "re-taken");
        // Two tweaks: each takes ±90°, the first listed wins; 96° from up is right's.
        let right = Frame { stick: [0.9, -0.1], ..frame(true) };
        for _ in 0..16 {
            bail.frame(&rules, &right);
        }
        assert_eq!((bail.tweaks, bail.tweak, bail.tweak_points), (3, Some(2), 1500), "another direction starts again");
    }

    #[test]
    fn rotation_tracks_turns_banks_reversals_and_scores_its_best() {
        let mut rules = rules();
        rules.curves.rotation =
            curve([0., 719.9, 720., 1080., 1800., 2520., 3240., 3500.], [0., 0., 750., 1250., 2500., 5000., 10000., 15000.]);
        // Trackers in scorer order (+112 axis 2, +192 axis 0, +272 axis 1); a turn about the hips' Y is +272's.
        let deg = |t: &Tracker| (t.angle * DEGREES).abs();
        let total = |b: &Bail| deg(&b.trackers[1]) + deg(&b.trackers[0]) + deg(&b.trackers[2]) + b.banked;
        // The still trackers creep: acos(1) is 4.8e-7, and a normalised dot an ulp under 1 gives 3.5e-4.
        let still = |b: &Bail| format!("{} {}", deg(&b.trackers[0]), deg(&b.trackers[1]));
        let mut bail = Bail::start(&rules, &frame(false));
        let mut f = frame(false);
        for i in 1..=20 {
            f.rows = turned(i as f32 * 0.1);
            bail.frame(&rules, &f);
        }
        assert!((bail.trackers[2].angle.abs() - 2.).abs() < 1e-4, "{}", bail.trackers[2].angle);
        assert!(deg(&bail.trackers[0]) < 0.1 && deg(&bail.trackers[1]) < 0.1, "{}", still(&bail));
        assert_eq!(bail.max_rotation, total(&bail), "0x82bb70e0's sum, still climbing");
        // Turning back from past 30° banks those degrees and keeps counting from the step back.
        let before = deg(&bail.trackers[2]);
        f.rows = turned(1.9);
        bail.frame(&rules, &f);
        assert_eq!(bail.banked, before);
        assert!((bail.trackers[2].angle.abs() - 0.1).abs() < 1e-4, "{}", bail.trackers[2].angle);
        assert_eq!(bail.max_rotation, total(&bail));
        // Settled: the trackers follow the body without counting.
        let (max, angles) = (bail.max_rotation, bail.trackers.map(|t| t.angle));
        f = Frame { rows: turned(1.), settled: true, ..f };
        bail.frame(&rules, &f);
        assert_eq!((bail.max_rotation, bail.trackers.map(|t| t.angle), bail.trackers[0].rows), (max, angles, turned(1.)));
        assert_eq!(bail.unsettled, 21);
        // Over 90° in one frame isn't a turn (axis-exact rows: only +192's acos(1) creep is left).
        let mut bail = Bail::start(&rules, &frame(false));
        bail.frame(&rules, &Frame { rows: turned(2.), ..frame(false) });
        assert_eq!(bail.trackers.map(|t| t.angle), [0., acos(1.), 0.]);
        assert_eq!(bail.max_rotation, acos(1.) * DEGREES);
        // Two full turns and a bit: 802.1° (+ the creep), 750 + 500 / 360 x 82.1.
        let mut bail = Bail::start(&rules, &frame(false));
        for i in 1..=70 {
            bail.frame(&rules, &Frame { rows: turned(i as f32 * 0.2), ..frame(false) });
        }
        assert!((bail.trackers[2].angle.abs() - 14.).abs() < 1e-3, "{}", bail.trackers[2].angle);
        assert!(deg(&bail.trackers[0]) < 0.5 && deg(&bail.trackers[1]) < 0.5, "{}", still(&bail));
        assert_eq!(bail.max_rotation, total(&bail));
        assert_eq!(bail.max_rotation_points, rules.curves.rotation.points(bail.max_rotation));
        assert_eq!(bail.max_rotation_points, 864, "{}", bail.max_rotation);
    }

    #[test]
    fn bail_time_counts_unsettled_frames_and_speed_is_the_starts() {
        let mut rules = rules();
        rules.curves.bail_time = curve([0., 4.999, 5., 5.25, 5.5, 6., 8., 9.], [0., 0., 750., 1250., 2500., 5000., 10000., 15000.]);
        rules.curves.speed = curve([0., 9.9999, 10., 12.5, 17.5, 20., 45., 50.], [0., 0., 750., 2500., 5000., 10000., 10000., 10000.]);
        let mut bail = Bail::start(&rules, &Frame { velocity: [0., 0., 12., 0.], ..frame(false) });
        assert_eq!((bail.speed, bail.speed_points), (12., 2150));
        for _ in 0..299 {
            bail.frame(&rules, &frame(false));
        }
        assert_eq!(bail.bonus(&rules)[3], 0, "4.98 s");
        bail.frame(&rules, &frame(false));
        assert_eq!(bail.bonus(&rules)[3], 750, "5 s");
        bail.frame(&rules, &Frame { settled: true, ..frame(false) });
        assert_eq!((bail.unsettled, bail.total(&rules)), (300, 2150 + 750));
    }

    /// The installed export (SKATE3_ASSET_ROOT): every row has an attempt spawn and an intro scene the cutscene file
    /// carries. Skipped without the asset root (these tests are not in CI).
    #[test]
    fn installed_hom_json_loads_with_spawns_and_intros() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let root = PathBuf::from(assets).join(ROOT);
        let data = Data::load(&root).unwrap();
        let scenes: Json = serde_json::from_slice(&std::fs::read(root.join("tutorial.json")).unwrap()).unwrap();
        assert!(data.rows.len() >= 20);
        for (id, row) in &data.rows {
            assert!(row.attempt.is_some() && row.num_rounds > 0 && row.time_limit > 0., "{id}");
            let scene = row.intro.as_deref().unwrap();
            assert!(scenes["scenes"][scene]["camera"].is_string(), "{id}: {scene}");
        }
        assert_eq!(data.rules.curves.speed.points(10.), 750);
        let r = &data.rules;
        assert_eq!(
            (r.curves.bail_time.x[1].to_bits(), r.tweak_stick.to_bits(), r.toe_share.to_bits()),
            (0x409F_F7CF, 0x3F19_999A, 0x3ECC_CCCD),
            "exact f32s (4.999, 0.6, 0.4)"
        );
        assert_eq!(r.tweaks.iter().map(|t| t.id).collect::<Vec<_>>(), [0, 2, 1, 3]);
    }

    /// The installed homscoring and leaderboardhud run with every native they ask answered (SKATE3_ASSET_ROOT).
    #[test]
    fn installed_screens_have_every_native_answered() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let root = PathBuf::from(assets).join(ROOT);
        let data = Data::load(&root).unwrap();
        let mut bail = Bail::start(&data.rules, &Frame { velocity: [12., 0., 0., 0.], ..frame(false) });
        for i in 0..90 {
            let mut f = Frame { height: 9. - i as f32 * 0.1, rows: turned(i as f32 * 0.15), ..frame(true) };
            f.contacts[3].0 = 40.;
            bail.frame(&data.rules, &f);
        }
        bail.frame(&data.rules, &frame(false));
        assert!(bail.total(&data.rules) > 0 && bail.bonus(&data.rules)[1] > 0);
        let mut screen = crate::apt_screen::Screen::load(&root.join("runtime/homscoring.json"), &[]).unwrap();
        for state in [1000, 1] {
            screen.call("SetState", &[json!(state)]).unwrap();
        }
        screen.call("PlayIntroAudio", &[]).unwrap();
        let answers = panel_answers(&data, &bail, (true, 0), false, |id| screen.bindings.movie.text_assets.localize(id));
        assert_eq!(answers[1].1[0], json!("43.2 Km/h"), "12 m/s");
        update_panel(&mut screen, answers).unwrap();
        for _ in 0..120 {
            screen.advance().unwrap();
        }
        assert!(!screen.draws().unwrap().is_empty());
        assert!(screen.bindings.unanswered.is_empty(), "homscoring asks {:?}", screen.bindings.unanswered);
        // HOM_SNAPS=<dir>: rasterise both screens there, to look at.
        let snap = |screen: &crate::apt_screen::Screen, name: &str| {
            if let Ok(dir) = std::env::var("HOM_SNAPS") {
                let png = PathBuf::from(dir).join(format!("{name}.png"));
                crate::core_menu_runtime::tests::raster_draws(&screen.draws().unwrap(), &root, &png);
            }
        };
        snap(&screen, "homscoring");
        let standings = [("Ryan".to_owned(), 15250, true), ("Danny".to_owned(), 900, false)];
        let answers = leaderboard_answers("Fence Drop", true, &standings);
        let mut screen = crate::apt_screen::Screen::load(&root.join("runtime/leaderboardhud.json"), &answers).unwrap();
        for state in [3, 1, 5] {
            screen.call("SetState", &[json!(state)]).unwrap();
        }
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        assert!(!screen.draws().unwrap().is_empty());
        assert!(screen.bindings.unanswered.is_empty(), "leaderboardhud asks {:?}", screen.bindings.unanswered);
        snap(&screen, "leaderboardhud");
    }

    /// Each player's bail score in rounds 1 and 2 (no ties: `results_keep_the_earlier_scorer_and_list_leavers_last` has
    /// them).
    const BAILS: [[i64; 2]; 6] = [[300, 1200], [900, 400], [2500, 100], [50, 700], [1800, 2000], [600, 1500]];

    /// Six consoles over the wire (150-300 ms, 8 % loss as resends), two rounds: each runs its own turns through the
    /// real schedule, next_turn, after_move and TurnOver records; a rider bails 5 s into its turn for BAILS'. `leaver`
    /// leaves at `leave_at` (its records still on the way are lost; the others hear it 150-300 ms later). Returns each
    /// console's (turns it rode, best, its results order by slot), None for the leaver, once every one left is Done.
    fn hom_over_the_wire(seed: u64, leaver: usize, leave_at: f32) -> Vec<Option<(u32, i64, Vec<usize>)>> {
        use crate::skate3_modes::tests::Wire;
        let row = Row { title: None, time_limit: 30., num_rounds: 2, attempt: None, waiting: None, intro: None, intro_seconds: None, indicators: vec![] };
        let data = Data { rules: rules(), rows: HashMap::new(), strings: HashMap::new() };
        let mut wire = Wire::new(seed, (0.15, 0.3), 0.08);
        // wait_for_players: every console's first Move (the intro is over).
        let mut runs: Vec<Option<Run>> =
            (0..6).map(|me| Some(Run { me, players: 6, ..online_run(Stage::Move { round: 1, from: 0., news: true }) })).collect();
        let mut rode = [0u32; 6];
        let mut heard = [f32::INFINITY; 6];
        let mut now = 0f32;
        while runs.iter().flatten().any(|r| r.stage != Stage::Done) {
            now += 1. / 60.;
            assert!(now < 600., "stuck: {:?}", runs.iter().map(|r| r.as_ref().map(|r| (r.k, r.stage))).collect::<Vec<_>>());
            if runs[leaver].is_some() && now >= leave_at {
                runs[leaver] = None;
                for c in 0..6 {
                    wire.cut(leaver, c);
                    heard[c] = now + 0.15 + 0.15 * wire.draw() as f32;
                }
            }
            for c in 0..6 {
                let Some(run) = runs[c].as_mut() else { continue };
                let gone = |p: usize| p == leaver && now >= heard[c];
                hear(run, wire.take(f64::from(now), c));
                at_black(run, now);
                match run.stage {
                    Stage::Move { .. } => after_move(run, &data, "x", now),
                    Stage::Countdown { round, ends } if now >= ends => {
                        run.stage = if run.active == run.me {
                            rode[c] += 1;
                            Stage::Turn { round, ends: now + row.time_limit }
                        } else {
                            Stage::Watch { round }
                        };
                    }
                    // The bail's wipeout is over: Bailing -> Next at once.
                    Stage::Turn { round, ends } if now >= ends - row.time_limit + 5. => {
                        run.best = run.best.max(BAILS[c][round as usize - 1]);
                        run.stage = Stage::Next { round, at: now };
                    }
                    Stage::Next { round, at } if now >= at => {
                        run.turns.insert(run.k, run.best);
                        wire.send(f64::from(now), c, 6, vec![serde_json::to_string(&TurnOver { hom_turn: run.k, best: run.best, voted: None }).unwrap()]);
                        next_turn(run, &row, &data, gone, round, now);
                    }
                    Stage::Watch { round } if run.over.contains(&run.k) || gone(run.active) => next_turn(run, &row, &data, gone, round, now),
                    Stage::Ending { until } if now >= until => run.stage = Stage::Done,
                    _ => {}
                }
            }
        }
        let placed = |r: &Run| placing(6, &r.turns, &[leaver]).into_iter().map(|(p, _)| p).collect();
        runs.iter().enumerate().map(|(c, r)| r.as_ref().map(|r| (rode[c], r.best, placed(r)))).collect()
    }

    /// The results order online: each turn's best in turn order, a tie keeping the earlier scorer (hall_of_meat.lua
    /// 72-84, strict `>`): Bo's 700 (turn 1) stays ahead of Ann's (turn 4); the players who left last at -1, the later
    /// leaver above the earlier (558-580). Four players, two rounds: turn k is player k % 4.
    #[test]
    fn results_keep_the_earlier_scorer_and_list_leavers_last() {
        let turns = BTreeMap::from([(0, 500), (1, 700), (2, 0), (3, 300), (4, 700), (5, 700), (6, 50)]);
        assert_eq!(placing(4, &turns, &[3, 2]), [(1, 700), (0, 700), (2, -1), (3, -1)]);
        assert_eq!(placing(4, &turns, &[2, 3]), [(1, 700), (0, 700), (3, -1), (2, -1)]);
        assert_eq!(placing(4, &turns, &[]), [(1, 700), (0, 700), (3, 300), (2, 50)]);
    }

    /// Six online, a guest (slot 3) leaves in round 1 or the host (slot 0) in its first turn: every console gets to
    /// the end, everyone left rides both rounds, and every console's results go by the best bail (hall_of_meat.lua
    /// 72-84), the leaver last (558-580).
    #[test]
    fn six_over_the_wire_every_console_ends_and_the_results_go_by_the_best_bail() {
        for (seed, leaver, leave_at, want) in [(1, 3, 20., [2, 4, 5, 0, 1, 3]), (2, 0, 7., [2, 4, 5, 1, 3, 0])] {
            let ends = hom_over_the_wire(seed, leaver, leave_at);
            for (c, end) in ends.iter().enumerate().filter(|(c, _)| *c != leaver) {
                assert_eq!(end.as_ref().map(|e| e.0), Some(2), "seed {seed}: console {c} rode both rounds");
                assert_eq!(end.as_ref().map(|e| e.2.clone()), Some(want.to_vec()), "seed {seed}: console {c}'s results");
            }
        }
    }
}
