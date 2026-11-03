//! Skate 3's Own the Lot on the match core: the online activities of the 12 lots (challenge graphs
//! freeskate_ol_gap_tag / _accumulation / _simultaneous_trick / _survival / _trick_list < freeskate_ol < default), the
//! online non-team path. Data: tools/skate3_online.py `--modes otl`, installed at `private/skate3-online/otl/`:
//! rules.json (per activity row its graph, TimeLimit, CountdownDelay, the Freeskate* goals, ChallengeBoundary,
//! SpotVolumes, Location with its 6 slots, the Objectives / ObjectivesSolo groups as data; `lots`: each lot's
//! activities), strings.json, scripts/<graph>.lua (the decompiled graphs: the rules' source and the tests' oracle).
//!
//! An Own the Lot lobby is a Career-tab row of the Online Challenge Map (the lot); its crossbar has Select Activity
//! (ID_CROSSBAR_ONLINE_OTL_SELECT_ACTIVITY, retail item 42) in place of Start Challenge, and the host's pick starts the
//! activity (match_core: the game's mode is `own_the_lot:<activity>`).
//!
//! [`Otl`] is the five graphs handler by handler (each activity graph's own section, as the tool writes it):
//! - Gap / Tag (gap_tag): everyone races through the row's objectives; a skater's Score is
//!   Objectives.GetNumCompletedObjectivesForScoring, its Time when all are done; done when everyone still here is.
//! - Accumulation: one shared points total (Trick.GetCompletedSequenceScore of each completing sequence) toward
//!   FreeskateRequiredPoints x CalcAdjustment(skaters).
//! - Simultaneous Trick: every skater completes the objective within FreeskateTolerance of the others.
//! - Survival (Points): once everyone moves, on board, inside a SpotVolume, sequences add to a shared total; a bail,
//!   leaving the board, standing still or leaving the spot (past the tolerance) starts it over.
//! - Trick List: a shared list; the first to land an objective takes it for everyone (ForceObjectiveComplete).
//!
//! Each console evaluates its own skater's objective group ([`skate3_objectives::Group`] with skate3_film's skater
//! natives) and relays what the graphs hear of it (`Match::outbox`): objectives done, sequences begun and completed,
//! bails, the board, moving, the SpotVolumes. The graph keeps every skater's group as a [`Mirror`] (status by
//! objective), so Reset / Force / SetEvaluationStatus on another skater's group act on the mirror, and on this
//! console's real group. ponytail: retail evaluates every skater's group on each console from the replicated skaters.
//! Solo (`SKATE_MODE=own_the_lot:<activity>`) is a one-skater activity on the online rules.
use crate::{
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable, career},
    multiplayer::Multiplayer,
    online_roster::OnlineRoster,
    physics::SkaterRuntime,
    skate3_film::{self, Frame, SENSE_NATIVES, Sense},
    skate3_modes::{
        self, Component, Effect, EndSync, Graph, Handled, HudDriver, Locator, ModeData, Msg, Place, ResultsSetup, Rt, View, Volume,
        WHITE,
    },
    skate3_objectives::{Group, GroupDef, ObjectiveDef, Out},
    skate3_race::RaceVolume,
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Online.eFinishReason (the enum's string order, 0x82644xxx registration; not traced value by value): NOT_FINISHED 0,
/// NORMAL_COMPLETION 1, TIME_UP 2, PLAYER_DISCONNECT 3.
const NOT_FINISHED: i64 = 0;
const NORMAL: i64 = 1;
const TIME_UP: i64 = 2;
const DISCONNECT: i64 = 3;
/// Challenge.EActivityCompletionType: Abandoned 0, Owned 1 (as the oracle's driver).
const ABANDONED: i64 = 0;
const OWNED: i64 = 1;
/// HUD2.eDPOFormat: Ints 0, Points 1, Players 4.
const DPO_INTS: i64 = 0;
const DPO_POINTS: i64 = 1;
const DPO_PLAYERS: i64 = 4;
/// FrontEnd.eResultsLBType (endchallenge_online): PlaceNameInt 0, PlaceNameScoreTime 6.
const PLACE_NAME_INT: i64 = 0;
const PLACE_NAME_SCORE_TIME: i64 = 6;
/// The graphs' fixed seconds: vars.EndDelay (setup_common), the load poll (wait_for_load), the reposition delay
/// (skaterteleportcomplete), the countdown.
const END_DELAY: f64 = 2.;
const REPOSITION_DELAY: f64 = 1.;
const COUNTDOWN: f64 = 3.;
/// wait_for_load's poll (each graph's AddCountdownTimer(0.5)).
const LOAD_POLL: f64 = 0.5;
/// Not retail (user 10-12): wait_for_load gives up on a skater that never appears after this long (retail waits on).
const LOAD_MAX: f64 = 30.;
/// Objectives.MakeObjectives' group pointer for a skater (the oracle's driver: 100 + id).
pub(crate) const BASE: u32 = 100;
/// The volume index a Trigger message gives the ChallengeBoundary (SpotVolumes are 0..).
pub(crate) const BOUNDARY: i64 = -1;
const LOCATION: &str = "Location";
/// Skater moving / stopped (cMsgSkaterMoving). ponytail: the message's speeds are not traced; Skate 2's port's.
const MOVING: f32 = 0.5;
const STOPPED: f32 = 0.3;

const YOU_FINISHED_FIRST: &str = "ID_CHALLENGE_FREESKATEACTIVITY_GAP_YOUFINISHEDFIRST";
const YOU_FINISHED: &str = "ID_CHALLENGE_FREESKATEACTIVITY_GAP_YOUFINISHED";
const FINISHED_FIRST: &str = "ID_CHALLENGE_FREESKATEACTIVITY_GAP_GAMERTAGFINISHEDFIRST";
const FINISHED: &str = "ID_CHALLENGE_FREESKATEACTIVITY_GAP_GAMERTAGFINISHED";
const HURRY_UP: &str = "ID_CHALLENGE_FREESKATEACTIVITY_GAP_HURRYUP";
const SURV: &str = "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_";
pub(crate) const SELECT_ACTIVITY: &str = "ID_CROSSBAR_ONLINE_OTL_SELECT_ACTIVITY";

// ---------------------------------------------------------------- data
#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Groups {
    #[serde(rename = "Objectives", default)]
    pub objectives: Vec<Option<GroupDef>>,
    #[serde(rename = "ObjectivesSolo", default)]
    pub solo: Vec<Option<GroupDef>>,
}

/// An activity row (rules.json).
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub title: String,
    pub online: bool,
    /// The activity's graph (tools/skate3_online.py: by its template).
    #[serde(default)]
    pub graph: String,
    #[serde(rename = "TimeLimit")]
    pub time_limit: f64,
    #[serde(rename = "CountdownDelay")]
    pub countdown_delay: f64,
    #[serde(rename = "OutOfBoundsTime", default)]
    pub oob_time: f64,
    #[serde(rename = "ChallengeBoundary")]
    pub boundary: Option<RaceVolume>,
    #[serde(rename = "CentralMessageHALIDReturnToChallengeArea", default)]
    pub return_message: String,
    #[serde(rename = "FreeskateGoalAdjustment", default)]
    pub adjustment: f64,
    #[serde(rename = "FreeskateAccumulationType", default)]
    pub accumulation_type: i64,
    #[serde(rename = "FreeskateSurvivalType", default)]
    pub survival_type: i64,
    #[serde(rename = "FreeskateRequiredPoints")]
    pub required_points: Option<f64>,
    #[serde(rename = "FreeskateTolerance")]
    pub tolerance: Option<f64>,
    #[serde(rename = "SpotVolumes", default)]
    pub spots: Vec<Option<RaceVolume>>,
    #[serde(rename = "Location")]
    pub location: Option<Locator>,
    #[serde(rename = "objective_list", default)]
    pub objective_list: Vec<serde_json::Value>,
    #[serde(default)]
    pub groups: Groups,
    #[serde(default)]
    pub trigger_volumes: BTreeMap<String, Option<Volume>>,
    #[serde(default)]
    pub unique_trick_group: Option<String>,
}

/// MakeObjectives' group from a list of one-objective groups (each Objectives entry is one, as Domination's).
fn flatten(list: &[Option<GroupDef>]) -> Option<GroupDef> {
    let objectives: Option<Vec<_>> = list.iter().map(|g| g.as_ref().filter(|g| g.objectives.len() == 1).map(|g| g.objectives[0].clone())).collect();
    objectives.filter(|o| !o.is_empty()).map(|objectives| GroupDef { point_requirement: None, objectives })
}

impl Row {
    pub fn kind(&self) -> Option<Kind> {
        Kind::of(&self.graph)
    }
    /// Objectives.MakeObjectives(id[, "ObjectivesSolo"]).
    pub fn group(&self, solo: bool) -> Option<GroupDef> {
        flatten(if solo { &self.groups.solo } else { &self.groups.objectives })
    }
    /// Playable: online, a ported graph and goal type, its 6 start slots, its volumes placed, its objective groups
    /// whole and ported, on a known map.
    pub fn playable(&self, id: &str, points: &ObjectiveDef) -> bool {
        let Some(kind) = self.kind() else { return false };
        let ported = |g: Option<GroupDef>| g.is_some_and(|g| Group::supported(&g, points, SENSE_NATIVES).is_ok());
        let goal = match kind {
            // ponytail: the accumulation types other than Points (Grind / Air measures, one row: Grind_Distance) and
            // Survival's Time type (no row) are not ported.
            Kind::Accumulation => self.accumulation_type == 0 && self.required_points.is_some(),
            Kind::Survival => self.survival_type == 0 && self.required_points.is_some() && self.tolerance.is_some() && !self.spots.is_empty() && self.spots.iter().all(Option::is_some),
            // One skater plays ObjectivesSolo.
            Kind::Simul => self.tolerance.is_some() && ported(self.group(true)),
            Kind::GapTag | Kind::TrickList => true,
        };
        self.online
            && goal
            && self.location.as_ref().is_some_and(|l| l.slots.len() == 6)
            && ported(self.group(false))
            && self.trigger_volumes.values().all(Option::is_some)
            && map_of(id).is_some()
    }
}

/// The map an activity is on (Black Box and the Maloof Money Cup park are their own maps: skate3_modes' table).
pub(crate) use skate3_modes::map_of;

// ---------------------------------------------------------------- the graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    GapTag,
    Accumulation,
    Simul,
    Survival,
    TrickList,
}
impl Kind {
    pub fn of(graph: &str) -> Option<Self> {
        Some(match graph {
            "freeskate_ol_gap_tag" => Self::GapTag,
            "freeskate_ol_accumulation" => Self::Accumulation,
            "freeskate_ol_simultaneous_trick" => Self::Simul,
            "freeskate_ol_survival" => Self::Survival,
            "freeskate_ol_trick_list" => Self::TrickList,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    ShowNis,
    SetupCommon,
    SetupChallenge,
    WaitForLoad,
    Countdown,
    RunChallenge,
    Highlight,
    Results,
    CleanupChallenge,
    Complete,
}

/// A skater's objective group as the graph sees it: each objective done, evaluation on, and what
/// GetNumCompletedObjectivesForScoring last read (relayed with each completion).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Mirror {
    pub done: Vec<bool>,
    pub on: bool,
    pub scoring: i64,
}

/// The graph's Objectives.* calls on a skater's group (the glue applies this console's).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    Make { player: usize, solo: bool },
    Reset(usize),
    Evaluate(usize, bool),
    Force(usize, usize),
    FailAll(usize),
    Delete(usize),
}

/// Survival's ToleranceReason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Reason {
    #[default]
    Na,
    NotMoving,
    ExitBoundary,
}

/// vars.SkaterData[i] (ID i - 1).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Sk {
    pub disconnected: bool,
    finish: i64,
    pub score: f64,
    pub time: f64,
    complete: bool,
    result: i64,
    /// ToleranceTimerID (None: -1).
    tolerance: Option<u32>,
    moving: bool,
    onboard: bool,
    volumes: i64,
    reason: Reason,
}

/// vars.Standings[i] (or SkaterData order for Survival): the results' rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Standing {
    pub player: usize,
    pub score: f64,
    pub time: f64,
}

#[derive(Clone)]
pub(crate) struct Otl {
    kind: Kind,
    row: Row,
    me: usize,
    n: usize,
    pub sk: Vec<Sk>,
    pub groups: Vec<Option<Mirror>>,
    /// Trick.GetCompletedSequenceScore / IsInManual / IsInSequence per skater, as last relayed.
    pub seq: Vec<i64>,
    pub manual: Vec<bool>,
    pub in_sequence: Vec<bool>,
    pub cmds: Vec<Cmd>,
    setup_complete: bool,
    closed_loading: bool,
    num_active: i64,
    num_done: i64,
    pub finish_reason: i64,
    pub finish_time: f64,
    pub time_limit: f64,
    result: i64,
    reposition_delay: Option<u32>,
    countdown_delay: Option<u32>,
    countdown_timer: Option<u32>,
    challenge_timer: Option<u32>,
    delay_timer: Option<u32>,
    oob_timer: Option<u32>,
    /// Accumulation / Survival: the shared total and its requirement.
    pub total: f64,
    pub required: f64,
    /// Simultaneous Trick.
    simultaneous: bool,
    finished: bool,
    /// Trick List: the objectives taken, and how many.
    list_done: Vec<bool>,
    list_count: i64,
    pub surviving: bool,
    /// Survival's run_challenge reset the SpotVolumes' trigger state (the glue re-enters them).
    pub reset_spots: bool,
    pub standings: Vec<Standing>,
    /// The results were reported (Online.OnlineChallengeComplete).
    pub reported: bool,
    /// Entity.DoesSkaterExist for every skater still in the activity (the glue sets it; offline and the oracle: true).
    pub loaded: bool,
    load_poll: Option<u32>,
    load_since: f64,
    /// Online: when each skater's last objective landed (its own console's challenge clock).
    done_at: Vec<f64>,
    /// Online Trick List: who landed each objective earliest (challenge clock, id).
    claims: Vec<Option<(f64, usize)>>,
    /// Online Accumulation: every objective counted (its skater's challenge clock, skater, points).
    landed: Vec<(f64, usize, f64)>,
    /// Online Simultaneous Trick / Survival: this console judges, or follows the judge ([`Relay::Ask`]).
    pub role: Role,
    /// The judge: the tolerance windows that ran out here (the glue logs them as verdicts).
    pub tols: Vec<usize>,
    /// A verdict being applied: the judge's challenge clock when it judged it.
    pub stamp: Option<f64>,
}

/// Not retail (user 10-12): who decides an online Simultaneous Trick / Survival ([`Relay::Ask`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Role {
    /// Every event applied as it comes (offline, the other activities).
    #[default]
    Own,
    Judge,
    /// The judge's verdicts only: the tolerance windows here do not run out by themselves.
    Follower,
}

/// The judged part of [`Otl`], as a judge promoted after a leave sends it ([`Relay::Sync`]), compact (end review I2: a
/// match event record holds 1024 bytes; 10 skaters fit). Per skater (flags, finish, score, time, result, volumes,
/// sequence score, group scoring, group objectives): flags 1 disconnected, 2 complete, 4 tolerance window, 8 moving,
/// 16 on board, 32 manual, 64 in sequence, 128 group, 256 group on, the tolerance reason from 512; the objectives one
/// bit each under a leading 1. Then (total, finish time, finish reason, active skaters, 1 surviving | 2 finished).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Judged {
    s: Vec<(u32, i64, f64, f64, i64, i64, i64, i64, u64)>,
    g: (f64, f64, i64, i64, u8),
}

impl Otl {
    pub fn new(row: Row, players: usize, me: usize) -> Option<Self> {
        let kind = row.kind()?;
        Some(Otl {
            kind,
            row,
            me,
            n: players,
            sk: vec![Sk::default(); players],
            groups: vec![None; players],
            seq: vec![0; players],
            manual: vec![false; players],
            in_sequence: vec![false; players],
            cmds: Vec::new(),
            setup_complete: false,
            closed_loading: false,
            num_active: 0,
            num_done: 0,
            finish_reason: NOT_FINISHED,
            finish_time: -1.,
            time_limit: 0.,
            result: ABANDONED,
            reposition_delay: None,
            countdown_delay: None,
            countdown_timer: None,
            challenge_timer: None,
            delay_timer: None,
            oob_timer: None,
            total: 0.,
            required: 0.,
            simultaneous: false,
            finished: false,
            list_done: Vec::new(),
            list_count: 0,
            surviving: false,
            reset_spots: false,
            standings: Vec::new(),
            reported: false,
            loaded: true,
            load_poll: None,
            load_since: 0.,
            done_at: vec![0.; players],
            claims: Vec::new(),
            landed: Vec::new(),
            role: Role::Own,
            tols: Vec::new(),
            stamp: None,
        })
    }
    pub fn kind(&self) -> Kind {
        self.kind
    }

    fn name(p: usize) -> String {
        format!("P{p}")
    }
    /// vars:CalcAdjustment(numSkaters, inverse).
    fn adjust(&self, skaters: usize, inverse: bool) -> f64 {
        let m = if inverse { -1. } else { 1. };
        1. + m * (skaters as f64 - 1.) * (self.row.adjustment / 100.)
    }
    fn objectives(&self, solo: bool) -> usize {
        self.row.group(solo).map_or(0, |g| g.objectives.len())
    }

    // ------------------------------------------------------------ Objectives.* on the mirrors
    fn make(&mut self, rt: &mut Rt<St>, p: usize, solo: bool) {
        if solo {
            rt.note(format!("objectives {p} ObjectivesSolo"));
        }
        self.groups[p] = Some(Mirror { done: vec![false; self.objectives(solo)], on: true, scoring: 0 });
        self.cmds.push(Cmd::Make { player: p, solo });
    }
    fn reset(&mut self, rt: &mut Rt<St>, p: usize) {
        rt.note(format!("reset {}", BASE + p as u32));
        if let Some(g) = self.groups[p].as_mut() {
            g.done.iter_mut().for_each(|d| *d = false);
        }
        self.cmds.push(Cmd::Reset(p));
    }
    /// vars.EnableAllObjectives (Objectives.SetEvaluationStatus on every group).
    fn enable_all(&mut self, rt: &mut Rt<St>, on: bool) {
        for p in 0..self.n {
            rt.note(format!("eval {} {on}", BASE + p as u32));
            if let Some(g) = self.groups[p].as_mut() {
                g.on = on;
            }
            self.cmds.push(Cmd::Evaluate(p, on));
        }
    }
    /// Objectives.ForceObjectiveComplete (0x82c61838): objectivecomplete, then allobjectivecomplete when it was the last.
    fn force(&mut self, rt: &mut Rt<St>, p: usize, k: usize) {
        rt.note(format!("force {} {k}", BASE + p as u32));
        self.cmds.push(Cmd::Force(p, k));
        let Some(g) = self.groups[p].as_mut().filter(|g| g.done.get(k) == Some(&false)) else { return };
        g.done[k] = true;
        let all = g.done.iter().all(|d| *d);
        rt.post(Msg::GroupObjective { group: BASE + p as u32, index: k, points: false, accumulated: 0 });
        if all {
            rt.post(Msg::GroupAll { group: BASE + p as u32 });
        }
    }
    fn fail_all(&mut self, rt: &mut Rt<St>, p: usize) {
        rt.note(format!("failall {}", BASE + p as u32));
        self.cmds.push(Cmd::FailAll(p));
    }
    fn delete(&mut self, rt: &mut Rt<St>, p: usize) {
        rt.note(format!("delete {}", BASE + p as u32));
        self.groups[p] = None;
        self.cmds.push(Cmd::Delete(p));
    }
    /// A skater's group completed objective `k` (its own console's evaluation, relayed): objectivecomplete, and
    /// allobjectivecomplete when it was the last; `scoring` is GetNumCompletedObjectivesForScoring after it, `seq` the
    /// completing sequence's score. Nothing while the group is off or already has it.
    pub fn objective_done(&mut self, rt: &mut Rt<St>, p: usize, k: usize, scoring: i64, seq: i64) {
        self.objective_at(rt, p, k, scoring, seq, None);
    }
    /// The challenge clock: seconds since run_challenge began.
    pub fn elapsed(&self, rt: &Rt<St>) -> f64 {
        self.time_limit - self.challenge_timer.map_or(0., |t| rt.countdown_time(t))
    }
    /// The challenge clock of what is being handled: a verdict's is the judge's.
    fn clock(&self, rt: &Rt<St>) -> f64 {
        self.stamp.unwrap_or_else(|| self.elapsed(rt))
    }
    /// The judged state ([`Relay::Sync`]).
    pub fn judged(&self) -> Judged {
        let s = (0..self.n)
            .map(|p| {
                let (k, g) = (&self.sk[p], self.groups[p].as_ref());
                let bits = [k.disconnected, k.complete, k.tolerance.is_some(), k.moving, k.onboard, self.manual[p], self.in_sequence[p], g.is_some(), g.is_some_and(|g| g.on)];
                let flags = bits.iter().enumerate().fold((k.reason as u32) << 9, |f, (i, &b)| f | u32::from(b) << i);
                // ponytail: a group's first 63 objectives (the rows have a handful).
                let done = g.map_or(1, |g| g.done.iter().take(63).rev().fold(1u64, |m, &d| m << 1 | u64::from(d)));
                (flags, k.finish, k.score, k.time, k.result, k.volumes, self.seq[p], g.map_or(0, |g| g.scoring), done)
            })
            .collect();
        let state = u8::from(self.surviving) | u8::from(self.finished) << 1;
        Judged { s, g: (self.total, self.finish_time, self.finish_reason, self.num_active, state) }
    }
    /// A promoted judge's state replaces this console's: its tolerance windows run here (a window this console had
    /// not started starts in full: ponytail, the judge's remaining time is not sent), its finish starts or stops the
    /// end delay. ponytail: this skater's real objective group is not re-synced to the mirror (a verdict the old judge
    /// sent only here stays applied to it).
    pub fn restore(&mut self, rt: &mut Rt<St>, j: Judged) {
        // End review I4: a Sync for another number of skaters (a modded or mismatched console) is not this activity's.
        if j.s.len() != self.n || j.s.iter().any(|s| s.8 == 0) {
            return;
        }
        let tolerance = self.row.tolerance.unwrap_or(0.);
        for (p, &(f, finish, score, time, result, volumes, seq, scoring, done)) in j.s.iter().enumerate() {
            let bit = |i: u32| f >> i & 1 == 1;
            let s = &mut self.sk[p];
            s.tolerance = match (bit(2), s.tolerance) {
                (true, Some(t)) => Some(t),
                (true, None) => Some(rt.add_timer(tolerance)),
                (false, Some(t)) => {
                    rt.remove_timer(t);
                    None
                }
                (false, None) => None,
            };
            (s.disconnected, s.complete, s.moving, s.onboard) = (bit(0), bit(1), bit(3), bit(4));
            (s.finish, s.score, s.time, s.result, s.volumes) = (finish, score, time, result, volumes);
            s.reason = match f >> 9 & 3 {
                1 => Reason::NotMoving,
                2 => Reason::ExitBoundary,
                _ => Reason::Na,
            };
            (self.manual[p], self.in_sequence[p], self.seq[p]) = (bit(5), bit(6), seq);
            let len = 63 - done.leading_zeros() as usize;
            self.groups[p] = bit(7).then(|| Mirror { done: (0..len).map(|i| done >> i & 1 == 1).collect(), on: bit(8), scoring });
        }
        let (total, finish_time, finish_reason, num_active, state) = j.g;
        (self.total, self.num_active, self.surviving, self.finished) = (total, num_active.clamp(0, self.n as i64), state & 1 != 0, state & 2 != 0); // the judge's count: no MIN to underflow
        let (was, now) = (self.finish_reason == NORMAL, finish_reason == NORMAL);
        (self.finish_reason, self.finish_time) = (finish_reason, finish_time);
        if now && !was {
            self.delay_timer = Some(rt.add_timer(END_DELAY));
        } else if was && !now {
            if let Some(t) = self.delay_timer.take() {
                rt.remove_timer(t);
            }
        }
        if self.kind != Kind::Simul {
            rt.dpo_params(self.total as i64, Some(self.required as i64));
        }
    }
    /// [`Self::objective_done`] landed at `at` (the skater's own challenge clock; None: now here).
    pub fn objective_at(&mut self, rt: &mut Rt<St>, p: usize, k: usize, scoring: i64, seq: i64, at: Option<f64>) {
        let at = at.unwrap_or_else(|| self.elapsed(rt));
        let online = rt.online && rt.state == St::RunChallenge && p < self.n;
        // Not retail (user 10-12): online a Trick List objective goes to whoever landed it earliest (challenge clock,
        // then id), not to the relay that reached this console first, so every console credits the same skater.
        if online && self.kind == Kind::TrickList && self.list_done.get(k) == Some(&true) {
            if let Some(Some(c)) = self.claims.get(k).copied()
                && (at, p) < c
            {
                if c.1 != p {
                    self.sk[c.1].score -= 1.;
                    self.sk[p].score += 1.;
                }
                self.claims[k] = Some((at, p));
            }
            return;
        }
        // Not retail (user 10-12): online an Accumulation objective another console relays counts though this
        // console's groups are off (the target was met here first): it landed before its console knew, and counts there.
        let late = online && self.kind == Kind::Accumulation && p != self.me;
        // End review I4: an objective out of range (a modded client, an objective-count mismatch) is nobody's.
        let Some(g) = self.groups.get_mut(p).and_then(Option::as_mut).filter(|g| g.done.get(k).is_some_and(|&d| (g.on || late) && (late || !d))) else { return };
        if let Some(d) = self.done_at.get_mut(p) {
            *d = at;
        }
        if online && self.kind == Kind::Accumulation {
            self.landed.push((at, p, seq as f64));
        }
        if self.kind == Kind::TrickList {
            self.claims.resize(self.claims.len().max(k + 1), None);
            if self.claims[k].is_none_or(|c| (at, p) < c) {
                self.claims[k] = Some((at, p));
            }
        }
        g.done[k] = true;
        g.scoring = scoring;
        let all = g.done.iter().all(|d| *d);
        self.seq[p] = seq;
        rt.post(Msg::GroupObjective { group: BASE + p as u32, index: k, points: false, accumulated: 0 });
        if all {
            rt.post(Msg::GroupAll { group: BASE + p as u32 });
        }
    }
    /// [wait_for_load] vars.AreSkatersLoaded: the countdown once every skater exists, else poll again.
    fn poll_load(&mut self, rt: &mut Rt<St>) {
        if self.loaded || rt.now - self.load_since >= LOAD_MAX {
            rt.change(St::Countdown);
        } else {
            self.load_poll = Some(rt.add_timer(LOAD_POLL));
        }
    }
    /// The skater whose group a message names.
    fn owner(&self, group: u32) -> Option<usize> {
        let p = group.checked_sub(BASE)? as usize;
        self.groups.get(p)?.as_ref().map(|_| p)
    }

    // ------------------------------------------------------------ the handlers
    /// [setup_common] stateenter, the online non-team path.
    fn setup_common(&mut self, rt: &mut Rt<St>) {
        self.closed_loading = false;
        self.num_active = self.n as i64;
        for (i, s) in self.sk.iter_mut().enumerate() {
            *s = Sk { disconnected: rt.gone.get(i).copied().unwrap_or(true), finish: NOT_FINISHED, ..Sk::default() };
        }
        self.finish_reason = NOT_FINISHED;
        self.finish_time = -1.;
        self.setup_complete = true;
        self.time_limit = self.row.time_limit * self.adjust(self.n, false);
        // HUD2.CreateGroup: hudloaded.
        rt.post(Msg::HudLoaded);
        for p in 0..self.n {
            self.make(rt, p, false);
        }
    }

    /// [setup_challenge] stateenter of each graph.
    fn setup_challenge(&mut self, rt: &mut Rt<St>) {
        let n = self.n;
        match self.kind {
            Kind::GapTag => {
                for s in &mut self.sk {
                    (s.score, s.time, s.complete, s.result) = (0., -1., false, ABANDONED);
                }
                (0..n).for_each(|p| self.reset(rt, p));
                self.enable_all(rt, true);
                self.result = ABANDONED;
                self.num_done = 0;
                rt.show(Component::ObjectiveHUD, true);
                rt.dpo_format(DPO_PLAYERS);
                rt.dpo_params(self.num_done, Some(self.num_active));
                rt.dpo_title("ID_DPO_PLAYERSFINISHED");
                rt.show(Component::DPOHUD, true);
            }
            Kind::Accumulation => {
                self.time_limit = self.row.time_limit;
                self.sk.iter_mut().for_each(|s| s.score = 0.);
                self.total = 0.;
                (0..n).for_each(|p| self.reset(rt, p));
                self.enable_all(rt, true);
                self.result = ABANDONED;
                rt.show(Component::ObjectiveHUD, true);
                rt.dpo_format(DPO_POINTS);
                self.required = self.row.required_points.unwrap_or(0.) * self.adjust(n, false);
                rt.dpo_params(self.total as i64, Some(self.required as i64));
                rt.dpo_title("ID_FSA_ACCUMULATION_TARGET");
                rt.show(Component::DPOHUD, true);
            }
            Kind::Simul => {
                for s in &mut self.sk {
                    (s.complete, s.tolerance, s.score) = (false, None, 0.);
                }
                self.finished = false;
                self.simultaneous = 1 < n;
                if !self.simultaneous {
                    for p in 0..n {
                        self.delete(rt, p);
                        self.make(rt, p, true);
                    }
                }
                (0..n).for_each(|p| self.reset(rt, p));
                self.enable_all(rt, true);
                self.result = ABANDONED;
                rt.show(Component::ObjectiveHUD, true);
            }
            Kind::TrickList => {
                self.time_limit = self.row.time_limit * self.adjust(n, true);
                self.sk.iter_mut().for_each(|s| s.score = 0.);
                let count = self.row.objective_list.len();
                (self.list_count, self.list_done) = (0, vec![false; count]);
                (0..n).for_each(|p| self.reset(rt, p));
                self.enable_all(rt, true);
                rt.show(Component::ObjectiveHUD, true);
                rt.dpo_format(DPO_INTS);
                rt.dpo_title("ID_DPO_OBJECTIVESCOMPLETED");
                rt.dpo_params(self.list_count, Some(count as i64));
                rt.show(Component::DPOHUD, true);
            }
            Kind::Survival => {
                self.required = self.row.required_points.unwrap_or(0.);
                for s in &mut self.sk {
                    (s.score, s.moving, s.onboard, s.volumes, s.reason, s.tolerance) = (0., false, true, 0, Reason::Na, None);
                }
                self.total = 0.;
                self.surviving = false;
                (0..n).for_each(|p| self.reset(rt, p));
                self.enable_all(rt, true);
                rt.show(Component::ObjectiveHUD, true);
                rt.dpo_format(DPO_POINTS);
                rt.dpo_params(self.total as i64, Some(self.required as i64));
                rt.dpo_title("ID_FSA_SURVIVAL_TARGET");
                rt.show(Component::DPOHUD, true);
            }
        }
        rt.change(St::WaitForLoad);
    }

    /// [run_challenge] timerexpired of each graph.
    fn run_timer(&mut self, rt: &mut Rt<St>, id: u32) {
        let id = Some(id);
        let to_start = |rt: &mut Rt<St>, me: usize| rt.reposition(me, Place { field: LOCATION, index: 0, slot: me }, 1.);
        match self.kind {
            Kind::Survival => {
                if id == self.challenge_timer {
                    if self.finish_reason != NORMAL {
                        self.finish_reason = TIME_UP;
                        self.fail_all(rt, self.me);
                        self.reset_tolerance();
                        rt.pin(self.me, true);
                        self.delay_timer = Some(rt.add_timer(END_DELAY));
                    }
                } else if id == self.delay_timer {
                    rt.show(Component::TimerHUD, false);
                    rt.change(if self.finish_reason == NORMAL { St::Highlight } else { St::Results });
                } else if id == self.oob_timer {
                    to_start(rt, self.me);
                } else if let Some(i) = (0..self.n).find(|&i| self.sk[i].tolerance == id) {
                    self.tolerance_timer(rt, i);
                }
            }
            // (Timer ids are unique: the graphs' differing test orders are the same.)
            _ => {
                if id == self.delay_timer {
                    rt.show(Component::TimerHUD, false);
                    rt.change(St::Highlight);
                } else if id == self.challenge_timer {
                    self.finish_reason = TIME_UP;
                    rt.show(Component::TimerHUD, false);
                    rt.change(St::Results);
                } else if id == self.oob_timer {
                    to_start(rt, self.me);
                } else if self.kind == Kind::Simul
                    && self.simultaneous
                    && let Some(i) = (0..self.n).find(|&i| self.sk[i].tolerance == id)
                {
                    self.tolerance_timer(rt, i);
                }
            }
        }
    }

    /// A skater's tolerance window ran out here. Not retail (user 10-12): online the judge decides when ([`Role`]).
    fn tolerance_timer(&mut self, rt: &mut Rt<St>, i: usize) {
        match self.role {
            Role::Follower => {}
            Role::Judge => {
                self.tols.push(i);
                self.tolerance_out(rt, i);
            }
            Role::Own => self.tolerance_out(rt, i),
        }
    }
    /// [run_challenge] timerexpired, a skater's ToleranceTimerID.
    pub fn tolerance_out(&mut self, rt: &mut Rt<St>, i: usize) {
        if i >= self.n {
            return;
        }
        if let Some(t) = self.sk[i].tolerance.take() {
            rt.remove_timer(t);
        }
        if self.kind != Kind::Survival {
            return;
        }
        if self.survivable(i) {
            self.sk[i].reason = Reason::Na;
            return;
        }
        let (mine, theirs) = match self.sk[i].reason {
            Reason::NotMoving => ("YOUSKATEDSLOW", "TOOSLOW"),
            Reason::ExitBoundary => ("YOULEFTAREA", "LEFTAREA"),
            Reason::Na => ("", ""),
        };
        if !mine.is_empty() {
            self.tell(rt, i, mine, theirs);
        }
        self.stop_survival(rt);
    }

    /// [run_challenge] objectivecomplete.
    fn objective(&mut self, rt: &mut Rt<St>, group: u32, k: usize) {
        let Some(p) = self.owner(group) else { return };
        match self.kind {
            Kind::GapTag => self.sk[p].score = self.groups[p].as_ref().map_or(0, |g| g.scoring) as f64,
            Kind::Accumulation => {
                let points = self.seq[p] as f64;
                self.sk[p].score += points;
                self.total += points;
                rt.dpo_params(self.total as i64, Some(self.required as i64));
                if self.finish_reason == NORMAL {
                    // (Online: a late relayed objective after the target, see objective_at.)
                } else if self.total >= self.required {
                    self.finish_reason = NORMAL;
                    self.finish_time = self.time_limit - self.challenge_timer.map_or(0., |t| rt.countdown_time(t));
                    self.delay_timer = Some(rt.add_timer(END_DELAY));
                    self.enable_all(rt, false);
                } else {
                    self.reset(rt, p);
                }
            }
            Kind::TrickList => {
                let already = self.list_done.get(k).copied().unwrap_or(false);
                if let Some(d) = self.list_done.get_mut(k) {
                    *d = true;
                }
                if !already {
                    let p = if rt.online { self.claims.get(k).copied().flatten().map_or(p, |c| c.1) } else { p };
                    self.sk[p].score += 1.;
                    self.list_count += 1;
                    rt.dpo_params(self.list_count, Some(self.list_done.len() as i64));
                    for j in 0..self.n {
                        self.force(rt, j, k);
                    }
                }
            }
            Kind::Simul | Kind::Survival => {}
        }
    }

    /// [run_challenge] allobjectivecomplete.
    fn all_done(&mut self, rt: &mut Rt<St>, group: u32) {
        match self.kind {
            Kind::GapTag => {
                let Some(p) = self.owner(group) else { return };
                let s = &mut self.sk[p];
                (s.complete, s.result) = (true, OWNED);
                // Not retail (user 10-12): online the time is the skater's own console's (relayed), not this one's.
                s.time = if rt.online { self.done_at[p] } else { self.time_limit - self.challenge_timer.map_or(0., |t| rt.countdown_time(t)) };
                self.num_done += 1;
                if p == self.me {
                    rt.central(if self.num_done == 1 { YOU_FINISHED_FIRST } else { YOU_FINISHED }, WHITE, &[], 0);
                } else {
                    rt.modified(if self.num_done == 1 { FINISHED_FIRST } else { FINISHED }, WHITE, &[Self::name(p)], false);
                    if !self.sk[self.me].complete && self.num_done == self.num_active - 1 {
                        rt.central(HURRY_UP, WHITE, &[], 0);
                    }
                }
                rt.dpo_params(self.num_done, Some(self.num_active));
                if self.num_done >= self.num_active {
                    self.finish_reason = NORMAL;
                    self.delay_timer = Some(rt.add_timer(END_DELAY));
                    self.enable_all(rt, false);
                }
            }
            Kind::Simul => {
                if let Some(p) = self.owner(group) {
                    self.sk[p].complete = true;
                    self.sk[p].score = self.seq[p] as f64;
                    if self.simultaneous {
                        let t = rt.add_timer(self.row.tolerance.unwrap_or(0.));
                        self.sk[p].tolerance = Some(t);
                    }
                }
                let all = !self.simultaneous || self.sk.iter().all(|s| s.disconnected || s.complete);
                if all {
                    self.finish_reason = NORMAL;
                    self.finish_time = self.clock(rt);
                    self.delay_timer = Some(rt.add_timer(END_DELAY));
                    self.enable_all(rt, false);
                    self.finished = true;
                }
            }
            // No group filter: any group done ends it (every group is forced alike).
            Kind::TrickList => {
                self.finish_reason = NORMAL;
                self.finish_time = self.time_limit - self.challenge_timer.map_or(0., |t| rt.countdown_time(t));
                self.delay_timer = Some(rt.add_timer(END_DELAY));
                self.enable_all(rt, false);
            }
            Kind::Accumulation | Kind::Survival => {}
        }
    }

    /// Simultaneous Trick's [run_challenge] sequencebegin: a skater's sequence clears the others' completions whose
    /// tolerance ran out.
    fn sequence_begin(&mut self, rt: &mut Rt<St>, p: usize) {
        if self.finished || p >= self.n || !self.simultaneous {
            return;
        }
        for i in 0..self.n {
            if self.sk[i].complete && self.sk[i].tolerance.is_none() {
                self.reset(rt, i);
                self.sk[i].score = 0.;
                self.sk[i].complete = false;
            }
        }
    }

    /// [run_challenge] triggerentercollision / triggerexitcollision: Survival's SpotVolumes, then this skater leaving or
    /// re-entering the ChallengeBoundary.
    fn trigger(&mut self, rt: &mut Rt<St>, p: usize, volume: i64, inside: bool) {
        if self.kind == Kind::Survival && p < self.n && (0..self.row.spots.len() as i64).contains(&volume) {
            let s = &mut self.sk[p];
            if inside {
                s.volumes = s.volumes.saturating_add(1); // a Sync can set any count
                if s.volumes == 1 {
                    if self.surviving {
                        if s.reason == Reason::ExitBoundary {
                            (s.reason, s.tolerance) = (Reason::Na, None);
                        }
                    } else {
                        self.start_survival();
                    }
                }
            } else {
                s.volumes = s.volumes.saturating_sub(1);
                if s.volumes == 0 && self.surviving {
                    s.reason = Reason::ExitBoundary;
                    let t = rt.add_timer(self.row.tolerance.unwrap_or(0.));
                    self.sk[p].tolerance = Some(t);
                }
            }
        }
        if p == self.me && self.row.boundary.is_some() && volume == BOUNDARY {
            if inside {
                if let Some(t) = self.oob_timer {
                    rt.remove_timer(t);
                }
                rt.reset_effect(Effect::OOB);
            } else {
                self.oob_timer = Some(rt.add_timer(self.row.oob_time));
                let id = self.row.return_message.clone();
                rt.central(&id, WHITE, &[], 0);
                rt.start_effect(Effect::OOB, self.row.oob_time);
            }
        }
    }

    // ------------------------------------------------------------ Survival (setup_challenge's functions)
    fn reset_tolerance(&mut self) {
        for s in &mut self.sk {
            (s.reason, s.tolerance) = (Reason::Na, None);
        }
    }
    /// vars.PlayerSurvivable.
    fn survivable(&self, i: usize) -> bool {
        let s = &self.sk[i];
        s.disconnected || s.tolerance.is_some() || (s.moving && s.volumes > 0 && s.onboard)
    }
    /// vars.StopSurvival, the non-team Points path.
    fn stop_survival(&mut self, rt: &mut Rt<St>) {
        if self.finish_reason == NORMAL {
            return;
        }
        self.surviving = false;
        self.reset_tolerance();
        self.sk.iter_mut().for_each(|s| s.score = 0.);
        self.total = 0.;
        rt.dpo_params(self.total as i64, Some(self.required as i64));
    }
    /// vars.StartSurvivalIfSatisfied.
    fn start_survival(&mut self) {
        self.reset_tolerance();
        if !self.surviving && (0..self.n).all(|i| self.survivable(i)) {
            self.surviving = true;
        }
    }
    /// A central message for this skater, else the others' (modified, with the name).
    fn tell(&self, rt: &mut Rt<St>, i: usize, mine: &str, theirs: &str) {
        if i == self.me {
            rt.central(&format!("{SURV}{mine}"), WHITE, &[], 0);
        } else {
            rt.modified(&format!("{SURV}{theirs}"), WHITE, &[Self::name(i)], false);
        }
    }
    fn survival(&mut self, rt: &mut Rt<St>, msg: &Msg) {
        match *msg {
            Msg::Moving { player: p, moving } if p < self.n => {
                self.sk[p].moving = moving;
                if moving {
                    if self.surviving {
                        if self.sk[p].reason == Reason::NotMoving {
                            (self.sk[p].reason, self.sk[p].tolerance) = (Reason::Na, None);
                        }
                    } else {
                        self.start_survival();
                    }
                } else if self.surviving && (self.manual[p] || !self.in_sequence[p]) {
                    self.sk[p].reason = Reason::NotMoving;
                    let t = rt.add_timer(self.row.tolerance.unwrap_or(0.));
                    self.sk[p].tolerance = Some(t);
                }
            }
            Msg::Wipeout(p) if p < self.n && self.surviving => {
                self.tell(rt, p, "YOUBAILED", "BAIL");
                self.stop_survival(rt);
            }
            Msg::Board { player: p, on } if p < self.n => {
                self.sk[p].onboard = on;
                if on {
                    self.start_survival();
                } else if self.surviving {
                    self.tell(rt, p, "YOUWENTOFFBOARD", "OFFBOARD");
                    self.stop_survival(rt);
                }
            }
            Msg::Sequence { player: p, score, .. } if p < self.n && self.surviving => {
                self.sk[p].score += score as f64;
                self.total += score as f64;
                let meets = self.total >= self.required;
                rt.dpo_params(self.total as i64, Some(self.required as i64));
                if meets {
                    self.finish_reason = NORMAL;
                    self.finish_time = self.clock(rt);
                    self.force(rt, self.me, 0);
                    self.reset_tolerance();
                    rt.pin(self.me, true);
                    self.delay_timer = Some(rt.add_timer(END_DELAY));
                }
                if !self.sk[p].moving {
                    self.tell(rt, p, "YOUSKATEDSLOW", "TOOSLOW");
                    self.stop_survival(rt);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ results
    /// [results] stateenter of each graph, the online non-team path; then the report (OnlineChallengeComplete).
    fn results(&mut self, rt: &mut Rt<St>) {
        let finish = |s: &Sk, global: i64| if s.finish != NOT_FINISHED { s.finish } else { global };
        rt.hud.results.clear();
        let mut report = Vec::new();
        match self.kind {
            Kind::GapTag => {
                self.result = self.sk[self.me].result;
                self.finish_time = self.sk[self.me].time;
                let mut st: Vec<(Standing, i64)> =
                    (0..self.n).map(|i| (Standing { player: i, score: self.sk[i].score, time: self.sk[i].time }, finish(&self.sk[i], self.finish_reason))).collect();
                skate3_modes::lua_sort(&mut st, |a, b| a.0.score > b.0.score || (a.0.score == b.0.score && a.0.time < b.0.time));
                for (i, (s, _)) in st.iter().enumerate() {
                    rt.result(s.player, i as i64 + 1, s.score as i64);
                    rt.note(format!("time {} {:.3}", s.player, s.time));
                }
                report.extend(st.iter().enumerate().map(|(i, (s, f))| (s.player, i + 1, s.score, s.time, *f)));
                self.standings = st.into_iter().map(|s| s.0).collect();
            }
            Kind::Accumulation | Kind::TrickList | Kind::Simul => {
                // Not retail (user 10-12): online the list's time is when its last objective was landed (its
                // claimer's clock), not when that relay reached this console.
                if rt.online && self.kind == Kind::TrickList && self.finish_reason == NORMAL {
                    self.finish_time = self.claims.iter().flatten().map(|c| c.0).fold(0., f64::max);
                }
                // Not retail (user 10-12): online Accumulation's time is when the objectives, in the order their
                // skaters landed them, reached the target (every console counts the same ones).
                if rt.online && self.kind == Kind::Accumulation && self.finish_reason == NORMAL {
                    let mut landed = self.landed.clone();
                    landed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    let mut total = 0.;
                    for (at, _, points) in landed {
                        total += points;
                        if total >= self.required {
                            self.finish_time = at;
                            break;
                        }
                    }
                }
                if self.kind == Kind::Simul && self.finish_reason != NORMAL {
                    self.sk.iter_mut().for_each(|s| s.score = -1.);
                }
                let mut st: Vec<(Standing, i64)> = (0..self.n)
                    .map(|i| {
                        let f = if self.kind == Kind::Simul { self.finish_reason } else { finish(&self.sk[i], self.finish_reason) };
                        (Standing { player: i, score: self.sk[i].score, time: self.finish_time }, f)
                    })
                    .collect();
                skate3_modes::lua_sort(&mut st, |a, b| a.0.score > b.0.score);
                // Simultaneous Trick sets no player data (Online.OnlineChallengeComplete's report only).
                if self.kind != Kind::Simul {
                    for (i, (s, _)) in st.iter().enumerate() {
                        rt.result(s.player, i as i64 + 1, s.score as i64);
                    }
                }
                report.extend(st.iter().enumerate().map(|(i, (s, f))| (s.player, i + 1, s.score, s.time, *f)));
                self.standings = st.into_iter().map(|s| s.0).collect();
            }
            // In SkaterData order, unsorted.
            Kind::Survival => {
                for i in 0..self.n {
                    rt.result(i, i as i64 + 1, self.sk[i].score as i64);
                }
                report.extend((0..self.n).map(|i| (i, i + 1, self.sk[i].score, self.finish_time, finish(&self.sk[i], self.finish_reason))));
                self.standings = (0..self.n).map(|i| Standing { player: i, score: self.sk[i].score, time: self.finish_time }).collect();
            }
        }
        for (id, rank, score, time, f) in report {
            rt.note(format!("report {id} {rank} {} {time:.3} {f} {}", score as i64, self.result));
        }
        self.reported = true;
        rt.change(St::CleanupChallenge);
    }
}

impl Graph for Otl {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::ShowNis => "show_nis",
            St::SetupCommon => "setup_common",
            St::SetupChallenge => "setup_challenge",
            St::WaitForLoad => "wait_for_load",
            St::Countdown => "countdown",
            St::RunChallenge => "run_challenge",
            St::Highlight => "highlight",
            St::Results => "results",
            St::CleanupChallenge => "cleanup_challenge",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        // [global] playerdisconnected.
        if let Msg::Left(p) = *msg
            && self.setup_complete
            && self.sk.get(p).is_some_and(|s| !s.disconnected)
        {
            let s = &mut self.sk[p];
            (s.finish, s.disconnected, s.score) = (DISCONNECT, true, -1.);
            self.num_active -= 1;
            if p == self.me {
                rt.leave();
                rt.change(if self.kind == Kind::GapTag { St::CleanupChallenge } else { St::Complete });
            } else if self.kind == Kind::GapTag {
                self.num_done = self.sk.iter().filter(|s| !s.disconnected && s.complete).count() as i64;
                rt.dpo_params(self.num_done, Some(self.num_active));
                if self.num_done >= self.num_active {
                    self.finish_reason = NORMAL;
                    rt.change(St::Highlight);
                }
            }
        }
        match (rt.state, msg) {
            (St::Initial, Msg::Enter) => {
                self.setup_complete = false;
                rt.change(St::ShowNis);
            }
            // Online: no signup presentation (it plays offline only).
            (St::ShowNis, Msg::Enter) => rt.change(St::SetupCommon),
            (St::SetupCommon, Msg::Enter) => self.setup_common(rt),
            (St::SetupCommon, Msg::HudLoaded) => {
                rt.show(Component::TrickDisplayHUD, true);
                rt.reposition(self.me, Place { field: LOCATION, index: 0, slot: self.me }, 0.);
            }
            (St::SetupCommon, Msg::Teleported(_)) => self.reposition_delay = Some(rt.add_timer(REPOSITION_DELAY)),
            (St::SetupCommon, Msg::Timer(id)) if self.reposition_delay == Some(*id) => rt.change(St::SetupChallenge),
            (St::SetupChallenge, Msg::Enter) => self.setup_challenge(rt),
            // Entity.DoesSkaterExist: every skater is loaded here.
            (St::WaitForLoad, Msg::Enter) => {
                self.load_since = rt.now;
                self.poll_load(rt);
            }
            (St::WaitForLoad, Msg::Timer(id)) if self.load_poll == Some(*id) => self.poll_load(rt),
            (St::Countdown, Msg::Enter) => {
                rt.pin(self.me, true);
                if !self.closed_loading {
                    rt.close_loading();
                    self.closed_loading = true;
                }
                self.countdown_delay = Some(rt.add_timer(self.row.countdown_delay));
            }
            (St::Countdown, Msg::Timer(id)) if self.countdown_delay == Some(*id) => {
                let t = rt.add_timer(COUNTDOWN);
                self.countdown_timer = Some(t);
                rt.countdown(t);
            }
            (St::Countdown, Msg::Timer(id)) if self.countdown_timer == Some(*id) => rt.change(St::RunChallenge),
            (St::RunChallenge, Msg::Enter) => {
                rt.pin(self.me, false);
                self.reset_spots = self.kind == Kind::Survival;
                let t = rt.add_timer(self.time_limit);
                self.challenge_timer = Some(t);
                rt.timer(t);
                if self.kind == Kind::Accumulation {
                    (0..self.n).for_each(|p| self.reset(rt, p));
                }
            }
            (St::RunChallenge, Msg::Timer(id)) => self.run_timer(rt, *id),
            (St::RunChallenge, Msg::GroupObjective { group, index, .. }) => self.objective(rt, *group, *index),
            (St::RunChallenge, Msg::GroupAll { group }) => self.all_done(rt, *group),
            (St::RunChallenge, Msg::SequenceBegin(p)) if self.kind == Kind::Simul => self.sequence_begin(rt, *p),
            (St::RunChallenge, Msg::Trigger { player, volume, inside }) => self.trigger(rt, *player, *volume, *inside),
            (St::RunChallenge, m) if self.kind == Kind::Survival => self.survival(rt, m),
            (St::Highlight, Msg::Enter) => rt.reel(),
            (St::Highlight, Msg::Reel) => rt.change(St::Results),
            (St::Results, Msg::Enter) => self.results(rt),
            (St::CleanupChallenge, Msg::Enter) => rt.change(St::Complete),
            (St::Complete, Msg::Enter) => {
                for p in 0..self.n {
                    if self.groups[p].is_some() {
                        self.delete(rt, p);
                    }
                }
                rt.pin(self.me, false);
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// What this console's skater did that the graphs hear, relayed to the others (only what the activity's graph
/// handles).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Relay {
    /// objectivecomplete: the objective, GetNumCompletedObjectivesForScoring after it, the sequence's score, when (the
    /// skater's challenge clock).
    Obj { k: usize, scoring: i64, seq: i64, at: f64 },
    /// [`skate3_modes::EndSync`]'s marker.
    End(skate3_modes::EndMark),
    /// sequencebegin (Simultaneous Trick).
    Begin,
    /// Survival: sequencecomplete, wipeout, skateronboard / skateroffboard, skatermoving (with Trick.IsInManual and
    /// IsInSequence as they were), a SpotVolumes edge.
    Sequence { score: i64 },
    Wipe,
    Board(bool),
    Moving { moving: bool, manual: bool, sequence: bool },
    Spot { volume: i64, inside: bool },
    /// Not retail (user 10-12): online Simultaneous Trick and Survival are judged by one console, the lowest slot still
    /// here (the host first). The others send their events to it (`Ask`, with the sender's count); it applies every
    /// event in its own order and relays each as a verdict (`Log`: its number, whose event and count, the judge's
    /// challenge clock), with its tolerance windows running out (`Tol`) and skaters leaving (`Gone`). Every console
    /// applies the verdicts in order, so all show the judge's outcome. A judge promoted after a leave first sends its
    /// state and how far it got (`Sync`); the others take it and send again what it lacks of theirs.
    Ask(u32, Box<Relay>),
    Log { k: u64, from: usize, n: u32, at: f64, ev: Box<Relay> },
    Tol(usize),
    Gone(usize),
    Sync { k: u64, state: Box<Judged>, applied: Vec<u32> },
}

/// End review M8: the most entries a verdict queue keeps (a lost leave or Log must not grow it without end); a knob.
const QUEUE_MAX: usize = 256;

/// `item` at the end of `queue`, its oldest dropped past [`QUEUE_MAX`].
fn capped<T>(queue: &mut Vec<T>, item: T) {
    if queue.len() >= QUEUE_MAX {
        queue.remove(0);
    }
    queue.push(item);
}

/// Not retail (user 10-12): the judged activities' verdicts as this console keeps them ([`Relay::Ask`]).
#[derive(Default)]
struct Verdicts {
    /// The judge as this console sees it.
    judge: Option<usize>,
    /// The next verdict's number (the judge: to give; the others: to take).
    next: u64,
    /// Each skater's last event (count) in the verdicts.
    applied: Vec<u32>,
    /// This skater's events: the last count, and all of them (a new judge may lack some).
    sent: u32,
    unlogged: Vec<(u32, Relay)>,
    /// Verdicts from a console this one does not take for the judge yet (it has not seen the leave).
    held: Vec<(usize, Relay)>,
    /// Skaters gone while run_challenge runs: the judge's `Gone` applies them (or, after it, this console does).
    lefts: Vec<usize>,
}

#[derive(Default)]
struct Probe {
    spots: Vec<bool>,
    boundary: Option<bool>,
    /// Volume edges: (SpotVolumes index or BOUNDARY, inside).
    seen: Vec<(i64, bool)>,
    frames: Vec<Frame>,
}

/// The last tick as the relay's edges read it: sequence open, wiping, off board, moving.
#[derive(Default, Clone, Copy)]
struct Was {
    open: bool,
    wiping: bool,
    off: bool,
    moving: bool,
}

struct Play {
    id: String,
    row: Row,
    g: Otl,
    rt: Rt<St>,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    placed: u32,
    /// This skater's objective group (by the graph's pointer), and its natives.
    groups: Vec<(u32, Group)>,
    sense: Sense,
    was: Was,
    /// Other consoles' events waiting for this one's run_challenge.
    pending: Vec<(usize, Relay)>,
    sync: EndSync,
    verdicts: Verdicts,
}

impl Play {
    /// This console's skater plays (its events count).
    fn live(&self) -> bool {
        self.rt.state == St::RunChallenge && !self.sync.holding() && self.g.sk.get(self.me).is_some_and(|s| !s.disconnected)
    }
    /// Delivers what is due and applies the graph's objective calls (online, the challenge's end held for the other
    /// consoles): what to relay (the judge's tolerance verdicts, then the End marker).
    fn tick(&mut self, now: f64, points: Option<&ObjectiveDef>) -> Vec<String> {
        let mut out = Vec::new();
        self.rejudge(&mut out);
        let mut end = None;
        for _ in 0..64 {
            apply(self, points);
            end = end.or(skate3_modes::advance_synced(&mut self.g, &mut self.rt, now, St::RunChallenge, self.me, &mut self.sync));
            self.log_tolerances(&mut out);
            if self.rt.due().is_none_or(|d| d > now) || self.sync.holding() {
                break;
            }
        }
        // A skater gone in run_challenge whose leave the judge did not decide: here, once it is over.
        if self.rt.state != St::RunChallenge {
            for p in std::mem::take(&mut self.verdicts.lefts) {
                skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Left(p));
            }
        }
        apply(self, points);
        out.extend(end.map(Relay::End));
        self.texts(out)
    }
    fn texts(&self, out: Vec<Relay>) -> Vec<String> {
        if !self.online {
            return Vec::new();
        }
        out.iter().map(|r| serde_json::to_string(r).expect("plain data")).collect()
    }
    /// Online Simultaneous Trick / Survival with others: judged ([`Relay::Ask`]).
    fn judged(&self) -> bool {
        self.online && self.names.len() > 1 && matches!(self.g.kind(), Kind::Simul | Kind::Survival)
    }
    /// The judge is the lowest slot still here; a console promoted to it sends its state and puts its own events
    /// not judged yet in; a console that takes another for the judge applies what that one sent meanwhile.
    fn rejudge(&mut self, out: &mut Vec<Relay>) {
        if !self.judged() {
            return;
        }
        let n = self.rt.gone.len();
        self.verdicts.applied.resize(n, 0);
        let judge = (0..n).find(|&p| !self.rt.gone[p]).unwrap_or(self.me);
        let mine = judge == self.me;
        self.g.role = if mine { Role::Judge } else { Role::Follower };
        // The judge relays its End after its last verdict; the others wait for it longer.
        (self.sync.defer, self.sync.max) = if mine { (true, 0.) } else { (false, 2. * skate3_modes::END_HOLD_MAX) };
        self.sync.judge = Some(judge);
        let was = self.verdicts.judge.replace(judge);
        if was.is_none_or(|w| w == judge) {
            return;
        }
        if mine {
            // End review M8: promoted during an end hold, its own hold and cap from now; its (judge's) End marker
            // goes out after its verdicts.
            self.sync.rearm();
            self.verdicts.held.clear();
            out.push(Relay::Sync { k: self.verdicts.next, state: Box::new(self.g.judged()), applied: self.verdicts.applied.clone() });
            for (n, r) in std::mem::take(&mut self.verdicts.unlogged) {
                self.judge_one(self.me, n, r, out);
            }
        } else {
            let held = std::mem::take(&mut self.verdicts.held);
            let mut theirs: Vec<(usize, Relay)> = held.into_iter().filter(|(p, _)| *p == judge).collect();
            theirs.append(&mut self.pending);
            self.pending = theirs;
        }
    }
    /// The judge's tolerance windows that ran out: verdicts.
    fn log_tolerances(&mut self, out: &mut Vec<Relay>) {
        for i in std::mem::take(&mut self.g.tols) {
            if self.g.role == Role::Judge {
                let at = self.g.elapsed(&self.rt);
                out.push(Relay::Log { k: self.verdicts.next, from: self.me, n: 0, at, ev: Box::new(Relay::Tol(i)) });
                self.verdicts.next = self.verdicts.next.saturating_add(1);
            }
        }
    }
    /// The judge takes `from`'s event (count `n`; 0: the judge's own verdict) now and relays the verdict.
    fn judge_one(&mut self, from: usize, n: u32, r: Relay, out: &mut Vec<Relay>) {
        if n != 0 && self.verdicts.applied.get(from).is_some_and(|&a| n <= a) {
            return;
        }
        let at = self.g.elapsed(&self.rt);
        self.verdict(from, n, at, &r);
        out.push(Relay::Log { k: self.verdicts.next, from, n, at, ev: Box::new(r) });
        self.verdicts.next = self.verdicts.next.saturating_add(1);
        self.log_tolerances(out);
    }
    /// A verdict applied here: `from`'s event at the judge's challenge clock `at`, with what it posted.
    fn verdict(&mut self, from: usize, n: u32, at: f64, r: &Relay) {
        self.g.stamp = Some(at);
        match *r {
            Relay::Tol(i) => self.g.tolerance_out(&mut self.rt, i),
            Relay::Gone(p) => {
                self.verdicts.lefts.retain(|&q| q != p);
                skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Left(p));
            }
            ref r => deliver(self, from, r),
        }
        let now = self.rt.now;
        skate3_modes::advance(&mut self.g, &mut self.rt, now);
        self.g.stamp = None;
        if let Some(a) = self.verdicts.applied.get_mut(from) {
            *a = (*a).max(n);
        }
    }
    /// A judged message from another console, in run_challenge.
    fn judged_in(&mut self, from: usize, r: Relay, out: &mut Vec<Relay>) {
        match r {
            Relay::Ask(n, ev) => {
                // Tol / Gone are the judge's own verdicts: a peer asking for one would end another's window or drop them.
                if self.g.role == Role::Judge && !matches!(*ev, Relay::Tol(_) | Relay::Gone(_)) {
                    self.judge_one(from, n, *ev, out);
                }
            }
            Relay::Log { .. } | Relay::Sync { .. } if self.verdicts.judge != Some(from) => capped(&mut self.verdicts.held, (from, r)),
            Relay::Log { k, from: p, n, at, ev } => {
                if k >= self.verdicts.next {
                    self.verdicts.next = k.saturating_add(1); // a judge's k can be u64::MAX
                    self.verdict(p, n, at, &ev);
                }
            }
            Relay::Sync { k, state, applied } => {
                self.g.restore(&mut self.rt, *state);
                self.verdicts.next = k;
                self.verdicts.applied = applied;
                self.verdicts.applied.resize(self.rt.gone.len(), 0);
                // What the new judge lacks of this skater's: again.
                let have = self.verdicts.applied[self.me];
                self.verdicts.unlogged.retain(|(n, _)| *n > have);
                out.extend(self.verdicts.unlogged.iter().map(|(n, r)| Relay::Ask(*n, Box::new(r.clone()))));
            }
            _ => {}
        }
    }
    /// This skater's event: delivered here (judged: to the judge), and what to relay.
    fn local(&mut self, r: Relay) -> Vec<String> {
        let mut out = Vec::new();
        if self.judged() {
            self.verdicts.sent += 1;
            let n = self.verdicts.sent;
            if self.g.role == Role::Judge {
                self.judge_one(self.me, n, r, &mut out);
            } else {
                capped(&mut self.verdicts.unlogged, (n, r.clone()));
                out.push(Relay::Ask(n, Box::new(r)));
            }
        } else {
            let me = self.me;
            deliver(self, me, &r);
            out.push(r);
        }
        self.texts(out)
    }
    /// Skater `p` left the match. Judged, in run_challenge: the judge decides when (`Gone`).
    fn left(&mut self, p: usize) -> Vec<String> {
        let mut out = Vec::new();
        if p >= self.rt.gone.len() || self.rt.gone[p] {
            return Vec::new();
        }
        self.rt.gone[p] = true;
        if self.judged() && self.rt.state == St::RunChallenge {
            self.verdicts.lefts.push(p);
            self.rejudge(&mut out);
            if self.g.role == Role::Judge {
                self.judge_one(self.me, 0, Relay::Gone(p), &mut out);
            }
        } else {
            skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Left(p));
        }
        self.texts(out)
    }
    /// The other consoles' events: in run_challenge (an early one waits; after it, dropped). What to relay (the
    /// judge's verdicts).
    fn receive(&mut self, received: Vec<(usize, Relay)>) -> Vec<String> {
        let mut out = Vec::new();
        for (from, r) in received {
            match r {
                Relay::End(phase) => self.sync.ended(from, phase),
                r if from != self.me => self.pending.push((from, r)),
                _ => {}
            }
        }
        match self.rt.state {
            St::RunChallenge => {
                let judged = self.judged();
                for (from, r) in std::mem::take(&mut self.pending) {
                    if judged {
                        self.judged_in(from, r, &mut out);
                    } else {
                        deliver(self, from, &r);
                    }
                }
            }
            St::Highlight | St::Results | St::CleanupChallenge | St::Complete => self.pending.clear(),
            _ => {}
        }
        self.texts(out)
    }
}

#[derive(Resource, Default)]
pub(crate) struct OtlGame {
    data: Option<ModeData>,
    points: Option<ObjectiveDef>,
    tricks: HashMap<String, Vec<String>>,
    play: Option<Play>,
    probe: Probe,
    spots: Vec<RaceVolume>,
    boundary: Option<RaceVolume>,
    triggers: Vec<(u64, Volume)>,
    live: bool,
    warned: String,
}

/// Each lot's playable activities, (activity, title text) in the lot's order: the crossbar's Select Activity list.
#[derive(Resource, Default)]
pub(crate) struct OtlLots(pub BTreeMap<String, Vec<(String, String)>>);

pub(crate) struct Skate3OtlPlugin;
impl Plugin for Skate3OtlPlugin {
    fn build(&self, app: &mut App) {
        let data = ModeData::install(app, "otl", "Own the Lot");
        let rules = data.as_ref().and_then(|d| skate3_film::rules(d).inspect_err(|e| warn!("Skate 3 Own the Lot: {e}")).ok());
        let (points, tricks) = rules.map_or((None, HashMap::new()), |r| (Some(r.points_objective), r.trick_groups));
        let mut lots = OtlLots::default();
        if let (Some(data), Some(points)) = (&data, &points) {
            #[derive(Deserialize)]
            struct Lots {
                #[serde(default)]
                lots: BTreeMap<String, Vec<String>>,
            }
            let all = std::fs::read_to_string(data.root.join("rules.json")).ok().and_then(|t| serde_json::from_str::<Lots>(&t).ok()).map_or_else(BTreeMap::new, |l| l.lots);
            for (lot, activities) in all {
                let ok: Vec<(String, String)> = (activities.into_iter())
                    .filter_map(|a| data.row::<Row>(&a).ok().filter(|r| r.playable(&a, points)).map(|r| (a, data.text(&r.title))))
                    .collect();
                if !ok.is_empty() {
                    lots.0.insert(lot, ok);
                }
            }
            // The lots and every playable activity (also the ones no lot lists, as Survival's, for SKATE_MODE solo).
            let activities = data.rows.keys().filter(|a| data.row::<Row>(a).is_ok_and(|r| r.playable(a, points))).cloned();
            let ids: Vec<String> = lots.0.keys().cloned().chain(activities).collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                // The lot is a Career-tab row of the Online Challenge Map; its activities start from it (Select Activity).
                playable.0.extend(lots.0.keys().map(|lot| career(lot)));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
            info!("Skate 3 Own the Lot: {} lots, {} activities playable", lots.0.len(), lots.0.values().map(Vec::len).sum::<usize>());
        }
        app.insert_resource(OtlGame { data, points, tricks, ..default() })
            .insert_resource(lots)
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            // After online_boundary's: it clears the session marker's OOB static each frame; ours is the activity's.
            .add_systems(Update, run.after(crate::app::FrameSet::Physics).after(crate::online_boundary::update));
    }
}

/// Every physics tick while the activity runs: this skater's SpotVolumes and ChallengeBoundary edges, and its
/// [`Frame`] for the objective group and the relay.
fn probe(
    mut game: ResMut<OtlGame>,
    skater: Res<SkaterRuntime>,
    marker: Option<Res<crate::session_marker::SessionMarker>>,
) {
    let OtlGame { probe, spots, boundary, triggers, live, .. } = &mut *game;
    if !*live {
        *probe = Probe::default();
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let pos = Vec3::new(at[0], at[1], at[2]);
    probe.spots.resize(spots.len(), false);
    for (j, (was, v)) in probe.spots.iter_mut().zip(spots.iter()).enumerate() {
        let inside = v.contains(pos);
        if inside != *was {
            *was = inside;
            probe.seen.push((j as i64, inside));
        }
    }
    if let Some(b) = boundary {
        let inside = b.contains(pos);
        // The run starts inside: only a change is an edge.
        if probe.boundary.is_some_and(|was| was != inside) {
            probe.seen.push((BOUNDARY, inside));
        }
        probe.boundary = Some(inside);
    }
    probe.frames.push(skate3_film::frame_of(&skater, marker.as_deref(), triggers, None));
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<OtlGame>| {
        let Some(data) = game.data.take() else { return };
        step(world, &mut game, &data);
        game.data = Some(data);
    });
}

/// The graph's Objectives.* calls on this console's group.
fn apply(play: &mut Play, points: Option<&ObjectiveDef>) {
    for c in std::mem::take(&mut play.g.cmds) {
        match c {
            Cmd::Make { player, solo } if player == play.me => {
                play.groups.clear();
                match (play.row.group(solo), points) {
                    (Some(def), Some(points)) => match Group::new(player, &def, points) {
                        Ok(mut g) => {
                            g.unique_group = play.row.unique_trick_group.clone();
                            play.groups.push((BASE + player as u32, g));
                        }
                        Err(e) => warn!("Skate 3 Own the Lot: {}: {e}", play.id),
                    },
                    _ => warn!("Skate 3 Own the Lot: {}: no objective data", play.id),
                }
            }
            Cmd::Reset(p) if p == play.me => play.groups.iter_mut().for_each(|g| g.1.reset()),
            Cmd::Evaluate(p, on) if p == play.me => play.groups.iter_mut().for_each(|g| g.1.enabled = on),
            // Its messages are the mirror's (Otl::force).
            Cmd::Force(p, k) if p == play.me => play.groups.iter_mut().for_each(|g| drop(g.1.force_complete(k))),
            Cmd::FailAll(p) if p == play.me => play.groups.iter_mut().for_each(|g| g.1.fail_all()),
            Cmd::Delete(p) if p == play.me => play.groups.clear(),
            _ => {}
        }
    }
}

/// One skater's event into the graph.
fn deliver(play: &mut Play, from: usize, r: &Relay) {
    let msg = match *r {
        Relay::Obj { k, scoring, seq, at } => {
            play.g.objective_at(&mut play.rt, from, k, scoring, seq, Some(at));
            return;
        }
        Relay::End(_) | Relay::Ask(..) | Relay::Log { .. } | Relay::Tol(_) | Relay::Gone(_) | Relay::Sync { .. } => return,
        Relay::Begin => Msg::SequenceBegin(from),
        Relay::Sequence { score } => Msg::Sequence { player: from, score, wiped: false },
        Relay::Wipe => Msg::Wipeout(from),
        Relay::Board(on) => Msg::Board { player: from, on },
        Relay::Moving { moving, manual, sequence } => {
            if let (Some(m), Some(s)) = (play.g.manual.get_mut(from), play.g.in_sequence.get_mut(from)) {
                (*m, *s) = (manual, sequence);
            }
            Msg::Moving { player: from, moving }
        }
        Relay::Spot { volume, inside } => Msg::Trigger { player: from, volume, inside },
    };
    skate3_modes::dispatch(&mut play.g, &mut play.rt, msg);
}

/// [wait_for_load] vars.AreSkatersLoaded online: every slot still in the activity has its skater on this map (a slot
/// gone, or past the activity's players, does not count).
fn all_loaded(slots: &[u64], gone: &[bool], exists: impl Fn(u64) -> bool) -> bool {
    slots.iter().enumerate().all(|(p, &a)| gone.get(p).copied().unwrap_or(true) || exists(a))
}

/// This skater's event: relayed, then delivered here.
fn send(world: &mut World, play: &mut Play, r: Relay) {
    let texts = play.local(r);
    world.resource_mut::<Match>().outbox.extend(texts);
}

fn step(world: &mut World, game: &mut OtlGame, data: &ModeData) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && mode == Some(Mode::OwnTheLot) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 Own the Lot: {id} is on {:?}, not this map ({map:?})", map_of(&id));
            game.warned = id;
        }
        return;
    }
    let row = row.expect("on_map");
    if game.play.is_none() {
        if !matches!(phase, Phase::Running { .. }) {
            world.resource_mut::<Match>().hold = true;
            return;
        }
        let Some(play) = begin(world, &id, row, now, game.tricks.clone()) else {
            game.warned = id;
            return;
        };
        game.play = Some(play);
    }
    let points = game.points.clone();
    let play = game.play.as_mut().expect("begun");

    if play.online {
        let m = world.resource::<Match>();
        let left: Vec<usize> = (0..play.names.len()).filter(|&p| !play.rt.gone[p] && m.players.get(p).is_none_or(String::is_empty)).collect();
        for p in left {
            let texts = play.left(p);
            world.resource_mut::<Match>().outbox.extend(texts);
        }
    }
    if play.online {
        let (roster, net) = (world.get_resource::<OnlineRoster>(), world.get_resource::<Multiplayer>());
        let slots = roster.and_then(|r| r.state.game.as_ref()).map(|g| g.slots.clone()).unwrap_or_default();
        play.g.loaded = net.is_none_or(|net| all_loaded(&slots, &play.rt.gone, |a| net.skater_exists(a)));
    }
    let texts = play.tick(now, points.as_ref());
    world.resource_mut::<Match>().outbox.extend(texts);
    game.live = play.live();
    game.spots = play.row.spots.iter().flatten().cloned().collect();
    game.boundary = play.row.boundary.clone();
    game.triggers = play.row.trigger_volumes.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()?))).collect();
    // Survival's run_challenge reset the SpotVolumes' trigger state: a skater already inside enters again.
    if std::mem::take(&mut play.g.reset_spots) {
        game.probe.spots.clear();
    }
    let kind = play.g.kind();
    let survival = kind == Kind::Survival;
    for (volume, inside) in std::mem::take(&mut game.probe.seen) {
        if !game.live {
            continue;
        }
        if volume == BOUNDARY {
            // Only this skater's boundary is handled (the handlers test PlayerID == LocalPlayerID).
            let me = play.me;
            skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Trigger { player: me, volume, inside });
        } else if survival {
            send(world, play, Relay::Spot { volume, inside });
        }
    }
    for f in std::mem::take(&mut game.probe.frames) {
        if !game.live {
            continue;
        }
        let was = play.was;
        let open = f.sequence_open();
        if kind == Kind::Simul && open && !was.open {
            send(world, play, Relay::Begin);
        }
        if survival {
            if let Some((score, wiped)) = f.sequence_end(was.open) {
                // A bail's sequence banks nothing.
                send(world, play, Relay::Sequence { score: if wiped { 0 } else { score } });
            }
            if f.wiping() && !was.wiping {
                send(world, play, Relay::Wipe);
            }
            if f.off_board() != was.off {
                send(world, play, Relay::Board(!f.off_board()));
            }
            let moving = if was.moving { f.speed() >= STOPPED } else { f.speed() > MOVING };
            if moving != was.moving {
                send(world, play, Relay::Moving { moving, manual: f.manual(), sequence: open });
            }
            play.was.moving = moving;
        }
        (play.was.open, play.was.wiping, play.was.off) = (open, f.wiping(), f.off_board());
        let outs = skate3_film::feed(&mut play.groups, &mut play.sense, f);
        let scoring = play.groups.first().map_or(0, |g| i64::from(g.1.completed_for_scoring(true)));
        let seq = play.sense.finished_score();
        for (_, _, o) in outs {
            if let Out::Complete { index, points: false, .. } = o {
                let at = play.g.elapsed(&play.rt);
                send(world, play, Relay::Obj { k: index, scoring, seq, at });
            }
        }
    }
    if play.online {
        let received: Vec<(usize, Relay)> =
            world.resource_mut::<Match>().inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect();
        let texts = play.receive(received);
        world.resource_mut::<Match>().outbox.extend(texts);
    }
    let texts = play.tick(now, points.as_ref());
    world.resource_mut::<Match>().outbox.extend(texts);

    // RepositionAI(id, Location, id): this skater to its start slot.
    if let Some((seq, place)) = play.rt.landed(play.me)
        && play.placed != seq
    {
        play.placed = seq;
        if let Some(at) = play.row.location.as_ref().map(|l| l.slot(place.slot)) {
            world.resource_mut::<Match>().teleport = Some(at);
        }
    }
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = play.rt.covered;
    }
    // G8: the OOB static (freeskate_ol 203 / 212) on the session marker's noise, as free skate's; none once it is over.
    let oob = if play.g.reported || play.rt.left { 0. } else { play.rt.oob_static(now) };
    if let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>()
        && marker.oob != oob
    {
        marker.oob = oob;
    }
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = play.rt.pinned.get(play.me).copied().unwrap_or(true) || play.rt.presenting_now();
        m.gestures = skate3_modes::gestures(&m, play.g.reported);
        m.watch = None;
        m.score = play.g.sk.get(play.me).map_or(0, |s| s.score as i64);
        m.finished = play.g.reported || play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
    }
    let title = data.text(&play.row.title);
    let view = View { names: &play.names, me: play.me, online: play.online, title: title.clone(), label: "Own the Lot" };
    let mut hud = play.rt.hud.clone();
    // The results (each graph's [results]): Simultaneous Trick sets no rows (its report fills retail's), so its
    // standings are the rows here. ponytail: Survival's own column titles are "#col1".."#col3" (unfinished on the
    // disc); Rank / Skater / Score here.
    if kind == Kind::Simul && play.g.reported && hud.results.is_empty() {
        hud.results = play.g.standings.iter().enumerate().map(|(i, s)| (s.player, i as i64 + 1, s.score as i64)).collect();
    }
    let (layout, third) = match kind {
        Kind::GapTag => (PLACE_NAME_SCORE_TIME, "ID_LEADERBOARD_HEADER_OBJECTIVES"),
        Kind::TrickList => (PLACE_NAME_INT, "ID_LEADERBOARD_HEADER_OBJECTIVES"),
        _ => (PLACE_NAME_INT, "ID_COMMON_SCORE"),
    };
    let mut columns = vec!["ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER", third];
    if kind == Kind::GapTag {
        columns.push("ID_LEADERBOARD_HEADER_TIME");
    }
    let results = play.g.reported.then(|| ResultsSetup {
        title,
        layout,
        columns: columns.into_iter().map(|c| data.text(c)).collect(),
        throwdown: 0,
        choice: None,
        times: if kind == Kind::GapTag {
            (play.g.standings.iter()).filter(|s| s.time >= 0.).map(|s| (s.player, format!("#{}", Mode::DeathRace.show((s.time * 1000.).round() as i64)))).collect()
        } else {
            Vec::new()
        },
        floats: Vec::new(),
        strings: Vec::new(),
    });
    play.hud.drive(world, data, &hud, &view, results.as_ref(), now);
}

fn begin(world: &mut World, id: &str, row: Row, now: f64, tricks: HashMap<String, Vec<String>>) -> Option<Play> {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (vec!["Player 1".to_owned()], 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let Some(mut g) = Otl::new(row.clone(), names.len(), me) else {
        warn!("Skate 3 Own the Lot: {id}: graph {:?} is not ported", row.graph);
        return None;
    };
    info!("Skate 3 Own the Lot: {id} ({:?}, {}), {} skaters, this one {me}", g.kind(), if online { "online" } else { "solo" }, names.len());
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    rt.online = online;
    skate3_modes::start(&mut g, &mut rt);
    Some(Play {
        id: id.to_owned(),
        row,
        g,
        rt,
        names,
        me,
        online,
        hud: HudDriver::default(),
        placed: 0,
        groups: Vec::new(),
        sense: Sense::new(tricks),
        was: Was::default(),
        pending: Vec::new(),
        sync: EndSync::default(),
        verdicts: Verdicts::default(),
    })
}

fn end(world: &mut World, game: &mut OtlGame) {
    if let Some(play) = game.play.take() {
        info!("Skate 3 Own the Lot: {} ended", play.id);
    }
    game.live = false;
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = false;
    }
    if let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>() {
        marker.oob = 0.;
    }
    HudDriver::close(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};
    use crate::skate3_objectives::tests::spot;

    /// A test row: `k` objectives (`solo` in ObjectivesSolo), `spots` SpotVolumes.
    #[allow(clippy::too_many_arguments)]
    fn row(kind: Kind, k: usize, solo: usize, spots: usize, boundary: bool, time: f64, adjustment: f64, required: f64) -> Row {
        let graph = match kind {
            Kind::GapTag => "freeskate_ol_gap_tag",
            Kind::Accumulation => "freeskate_ol_accumulation",
            Kind::Simul => "freeskate_ol_simultaneous_trick",
            Kind::Survival => "freeskate_ol_survival",
            Kind::TrickList => "freeskate_ol_trick_list",
        };
        let groups = |n: usize| (0..n).map(|i| Some(GroupDef { point_requirement: None, objectives: vec![Some(spot(i as u64 + 1))] })).collect();
        let vol = |name: &str| RaceVolume { name: name.into(), min: [0.; 3], max: [1.; 3] };
        Row {
            title: "ID_T".into(),
            online: true,
            graph: graph.into(),
            time_limit: time,
            countdown_delay: 1.5,
            oob_time: 5.,
            boundary: boundary.then(|| vol("b")),
            return_message: "ID_RETURN".into(),
            adjustment,
            accumulation_type: 0,
            survival_type: 0,
            required_points: Some(required),
            tolerance: Some(2.),
            spots: (0..spots).map(|j| Some(vol(&format!("s{j}")))).collect(),
            location: Some(Locator { matrix: [[0.; 4]; 4], slots: vec![[[0.; 4]; 4]; 6] }),
            objective_list: vec![serde_json::Value::Null; k],
            groups: Groups { objectives: groups(k), solo: groups(solo) },
            trigger_volumes: BTreeMap::new(),
            unique_trick_group: None,
        }
    }

    /// One run's events: objectives done (a skater's k-th, with its scoring count and sequence score), Simultaneous
    /// Trick's sequences begun, Survival's moving / board / bails / spot volumes / sequences, this skater leaving and
    /// re-entering the ChallengeBoundary, and rarely a skater leaving.
    #[allow(clippy::too_many_arguments)]
    fn plans(seed: u64, kind: Kind, n: usize, me: usize, k: usize, spots: usize, time: f64, leaves: bool) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        let mut run = Vec::new();
        let leave = (leaves && n > 1).then(|| (next(n as u64) as usize, 2. + next((time * 6.) as u64) as f64 / 10.));
        let obj = |p: usize, k: usize, scoring: i64, seq: i64| Ev::Other("obj", seq * 65536 + (k as i64) * 256 + p as i64, scoring as f64);
        for p in 0..n {
            let mut t = 0.05 * (p + 1) as f64;
            let mut done = 0;
            while t < time + 3. {
                t += match kind {
                    Kind::Survival | Kind::Simul => 0.3 + next(30) as f64 / 10.,
                    _ => 1. + next(80) as f64 / 10.,
                };
                if leave.is_some_and(|(q, at)| q == p && t >= at) {
                    break;
                }
                let seq = 100 * (1 + next(30) as i64);
                match kind {
                    Kind::GapTag | Kind::TrickList => {
                        done += 1;
                        run.push((t, obj(p, next(k as u64) as usize, done + next(2) as i64, seq)));
                    }
                    Kind::Accumulation => run.push((t, obj(p, 0, 0, seq))),
                    Kind::Simul => {
                        run.push((t, Ev::Other("seqbegin", p as i64, 0.)));
                        if next(2) == 0 {
                            run.push((t + 0.1 + next(20) as f64 / 10., obj(p, 0, 0, seq)));
                        }
                    }
                    Kind::Survival => {
                        let ev = match next(9) {
                            0 | 1 => Ev::Other("moving", p as i64, next(8) as f64),
                            2 | 3 => Ev::Other("trig", p as i64 + 256 * (1 + next(spots as u64) as i64), (next(3) > 0) as i64 as f64),
                            4 => Ev::Other("board", p as i64, (next(4) > 0) as i64 as f64),
                            5 if next(3) == 0 => Ev::Other("wipe", p as i64, 0.),
                            _ => Ev::Other("seq", p as i64, seq as f64),
                        };
                        run.push((t, ev));
                    }
                }
                if p == me && next(6) == 0 {
                    run.push((t + 0.05, Ev::Other("trig", p as i64, 0.)));
                    run.push((t + 0.5 + next(70) as f64 / 10., Ev::Other("trig", p as i64, 1.)));
                }
            }
        }
        if let Some((p, at)) = leave {
            run.push((at, Ev::Leave(p)));
        }
        vec![run]
    }

    const SETUP: &str = r#"
Online.eFinishReason = {NOT_FINISHED = 0, NORMAL_COMPLETION = 1, TIME_UP = 2, PLAYER_DISCONNECT = 3, KNOCKED_OUT = 4, INACTIVE = 5}
Online.eFreeskateType = {Accumulation = 0, Gap = 1, Tag = 2, Simultaneous_Trick = 3, Survival = 4, Trick_List = 5}
Online.eFreeskateAccumulationType = {Points = 0, Grind_Only = 1, Grind_Distance = 2, Air_Only = 3, Air_Time = 4,
  Air_Horizontal_Distance = 5, Air_Vertical_Distance = 6, HallOfMeat_Points = 7}
Online.eFreeskateSurvivalType = {Points = 0, Time = 1}
Online.IsOnline = function() return true end
Effects.eChallengeEffect.OOB = "OOB"
ChallengeManager.GetSkaterCount = function() return N end
Entity.DoesSkaterExist = function() return true end
ChallengeStatus.GetHighScore = function() return 0 end
OTL.GetOTLChallengeParent = function() return {Name = "otl_lot", OnlineFreeSkateActivities = {Count = 8}} end
FrontEnd.Results_SetPlayerTime = function(id, t) log_line(string.format("time %d %.3f", id, t)) end
Online.OnlineChallengeComplete = function(r)
  for _, x in ipairs(r) do log_line(string.format("report %d %d %d %.3f %d %d", x[1], x[2], x[3], x[4], x[7], x[8])) end
end
local spots = {Count = S}
for j = 1, S do spots[j] = "s" .. (j - 1) end
SELF = setmetatable({Name = "otl_x", TimeLimit = TIME, CountdownDelay = 1.5, OutOfBoundsTime = 5, ChallengeBoundary = BOUNDARY,
  CentralMessageHALIDReturnToChallengeArea = "ID_RETURN", FreeskateGoalAdjustment = ADJ, FreeskateAccumulationType = 0,
  FreeskateSurvivalType = 0, FreeskateRequiredPoints = REQ, FreeskateTolerance = 2, SpotVolumes = spots,
  VisualIndicators = {Count = 0}, Location = "Location0", Objectives = {Count = K}, FreeskateActivityType = 1,
  DisableHOM = true}, {__index = function() return nil end})
GROUPS, SEQ, MAN, INSEQ = {}, {}, {}, {}
local function all(x) for k = 0, x.k - 1 do if not x.done[k] then return false end end return true end
Objectives.MakeObjectives = function(id, set)
  if set then log_line("objectives " .. id .. " " .. set) end
  local k = K
  if set == "ObjectivesSolo" then k = KSOLO end
  GROUPS[100 + id] = {done = {}, on = true, scoring = 0, k = k}
  return 100 + id
end
Objectives.ResetObjectives = function(g) log_line("reset " .. g); local x = GROUPS[g]; if x then x.done = {} end end
Objectives.SetEvaluationStatus = function(g, b) log_line("eval " .. g .. " " .. tostring(b)); local x = GROUPS[g]; if x then x.on = b end end
Objectives.DeleteObjectiveGroup = function(g) log_line("delete " .. g); GROUPS[g] = nil end
Objectives.FailAllObjectiveStatus = function(g) log_line("failall " .. g) end
Objectives.GetNumCompletedObjectivesForScoring = function(g) return GROUPS[g].scoring end
Objectives.ForceObjectiveComplete = function(g, k)
  log_line("force " .. g .. " " .. k)
  local x = GROUPS[g]
  if x and k < x.k and not x.done[k] then
    x.done[k] = true
    POST("objectivecomplete", {ObjectiveGroup = g, ObjectiveIndex = k})
    if all(x) then POST("allobjectivecomplete", {ObjectiveGroup = g}) end
  end
end
Trick.GetCompletedSequenceScore = function(id) return SEQ[id] or 0 end
Trick.IsInManual = function(id) return MAN[id] == true end
Trick.IsInSequence = function(id) return INSEQ[id] == true end
function ACTOR() return nil end
function DELIVER(kind, v, w)
  local p = v % 256
  if kind == "obj" then
    local k, seq = (v // 256) % 256, v // 65536
    local g = GROUPS[100 + p]
    if g and g.on and k < g.k and not g.done[k] then
      g.done[k] = true
      g.scoring = math.tointeger(w)
      SEQ[p] = seq
      POST("objectivecomplete", {ObjectiveGroup = 100 + p, ObjectiveIndex = k})
      if all(g) then POST("allobjectivecomplete", {ObjectiveGroup = 100 + p}) end
    end
    return true
  end
  if kind == "seqbegin" then SEND("sequencebegin", {PlayerID = p}); return true end
  if kind == "moving" then
    local f = math.tointeger(w)
    MAN[p] = f & 2 ~= 0
    INSEQ[p] = f & 4 ~= 0
    SEND("skatermoving", {PlayerID = p, IsMoving = f & 1 ~= 0})
    return true
  end
  if kind == "trig" then
    local vol = v // 256 - 1
    local id = "b"
    if vol >= 0 then id = "s" .. vol end
    if w > 0 then SEND("triggerentercollision", {PlayerID = p, VolumeID = id}) else SEND("triggerexitcollision", {PlayerID = p, VolumeID = id}) end
    return true
  end
  if kind == "wipe" then SEND("wipeout", {PlayerID = p}); return true end
  if kind == "board" then
    if w > 0 then SEND("skateronboard", {PlayerID = p}) else SEND("skateroffboard", {PlayerID = p}) end
    return true
  end
  if kind == "seq" then SEND("sequencecomplete", {PlayerID = p, SequenceScore = math.tointeger(w), Wipedout = false}); return true end
  return false
end
"#;

    /// The model's side of the plan events (the setup's DELIVER).
    fn other(g: &mut Otl, rt: &mut Rt<St>, kind: &str, v: i64, w: f64) -> Option<Msg> {
        let p = (v % 256) as usize;
        Some(match kind {
            "obj" => {
                g.objective_done(rt, p, ((v / 256) % 256) as usize, w as i64, v / 65536);
                return None;
            }
            "seqbegin" => Msg::SequenceBegin(p),
            "moving" => {
                let f = w as i64;
                if let (Some(m), Some(s)) = (g.manual.get_mut(p), g.in_sequence.get_mut(p)) {
                    (*m, *s) = (f & 2 != 0, f & 4 != 0);
                }
                Msg::Moving { player: p, moving: f & 1 != 0 }
            }
            "trig" => Msg::Trigger { player: p, volume: v / 256 - 1, inside: w > 0. },
            "wipe" => Msg::Wipeout(p),
            "board" => Msg::Board { player: p, on: w > 0. },
            "seq" => Msg::Sequence { player: p, score: w as i64, wiped: false },
            _ => return None,
        })
    }

    /// Each activity graph (S3_OTL_DIR = tools/skate3_online.py's `--out`/otl, with scripts/): its own section (the
    /// graph's full handler set) decides every game the model runs the same way: states, timers, the HUD2 calls, the
    /// objective groups' resets / evaluation / forces, central messages, the DPO, the out-of-bounds return, results and
    /// report; 1 to 6 skaters, skaters leaving.
    #[test]
    #[ignore = "requires the decompiled freeskate_ol_*.lua (S3_OTL_DIR)"]
    fn skate3_otl_matches_the_freeskate_ol_graphs() {
        let root = std::path::PathBuf::from(std::env::var("S3_OTL_DIR").unwrap());
        let (mut games, mut lines) = (0, 0);
        for (kind, graph) in [
            (Kind::GapTag, "freeskate_ol_gap_tag"),
            (Kind::Accumulation, "freeskate_ol_accumulation"),
            (Kind::Simul, "freeskate_ol_simultaneous_trick"),
            (Kind::Survival, "freeskate_ol_survival"),
            (Kind::TrickList, "freeskate_ol_trick_list"),
        ] {
            let text = std::fs::read_to_string(root.join(format!("scripts/{graph}.lua"))).unwrap();
            // The activity graph's own section: its states replace freeskate_ol's.
            let own = &text[text.find(&format!("-- graph {graph} parent")).unwrap()..];
            let (mut reported, mut finished) = (0, 0);
            for seed in 1..=40u64 {
                for n in 1..=6usize {
                    let s = seed as usize;
                    let me = (s * 7 + n) % n;
                    let k = match kind {
                        Kind::Accumulation | Kind::Simul | Kind::Survival => 1,
                        _ => 1 + (s * 3 + n) % 9,
                    };
                    let spots = 1 + s % 3;
                    let time = [40., 60., 90.][(s + n) % 3];
                    let adjustment = [0., 25., 50.][s % 3];
                    let required = [2000., 6500., 15000.][(s / 3) % 3];
                    let boundary = s % 5 != 0;
                    let r = row(kind, k, 1, spots, boundary, time, adjustment, required);
                    let plans = plans(seed * 977 + n as u64 * 31 + k as u64, kind, n, me, k, spots, time, seed % 3 == 0);
                    let setup = format!(
                        "N, ME, FIRST = {n}, {me}, 0\nK, KSOLO, S, TIME, ADJ, REQ = {k}, 1, {spots}, {time}, {adjustment}, {required}\nBOUNDARY = {}\n{SETUP}",
                        if boundary { "\"b\"" } else { "nil" }
                    );
                    let (lua, errs) = lua_log(own, &setup, "initial", "run_challenge", &plans);
                    let mut g = Otl::new(r, n, me).unwrap();
                    let mut rt = Rt::new(St::Initial, n, 0.);
                    let model = model_log(&mut g, &mut rt, St::RunChallenge, &plans, |_| None, |rt| rt.state == St::Complete, other);
                    compare(&format!("{graph} seed {seed} n {n} me {me} k {k}"), &lua, &model, &errs);
                    assert!(errs.is_empty(), "{graph} seed {seed} n {n}: {errs:?}");
                    games += 1;
                    lines += lua.len();
                    reported += usize::from(lua.iter().any(|l| l.contains("report")));
                    finished += usize::from(g.finish_reason == NORMAL);
                }
            }
            eprintln!("{graph}: 240 games, {reported} reported, {finished} completed");
            assert!(reported >= 200, "{graph}: {reported} games reported");
            assert!(finished >= 20, "{graph}: {finished} games completed");
        }
        eprintln!("freeskate_ol oracle: {games} games, {lines} log lines identical");
        assert_eq!(games, 1200);
    }

    /// Online audit 10-12: `n` consoles play one `kind` activity (`k` objectives) through the match relay
    /// ([`skate3_modes::tests::Wire`]), started up to 0.3 s apart; each skater lands its `plan`'s (challenge clock,
    /// objective) on its own console (one its mirror already has is skipped: it was forced there). `leaver` (player,
    /// challenge clock) quits then (what it still had on the way is lost); the others notice it 0.5 s + 0.4 s per slot
    /// later. Every staying console's finish reason and standings (player, score, time in ms).
    #[allow(clippy::type_complexity)]
    fn otl_over_the_wire(
        kind: Kind,
        k: usize,
        n: usize,
        seed: u64,
        loss: f64,
        plan: &dyn Fn(usize) -> Vec<(f64, usize)>,
        leaver: Option<(usize, f64)>,
    ) -> Vec<(i64, Vec<(usize, i64, i64)>)> {
        let r = row(kind, k, 1, 1, false, 60., 0., if kind == Kind::Accumulation { 3000. } else { 0. });
        let mut wire = skate3_modes::tests::Wire::new(seed, (0.15, 0.3), loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        let mut fed = vec![0; n];
        let plans: Vec<Vec<(f64, usize)>> = (0..n).map(plan).collect();
        let mut left_at = f64::INFINITY;
        let staying = |i: &usize| leaver.is_none_or(|(p, _)| p != *i);
        for frame in 0..60 * 200 {
            let t = frame as f64 / 60.;
            for i in 0..n {
                if t < starts[i] || !staying(&i) && t >= left_at {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| {
                    let g = Otl::new(r.clone(), n, i).unwrap();
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    let mut g = g;
                    skate3_modes::start(&mut g, &mut rt);
                    let names = (0..n).map(|p| format!("P{p}")).collect();
                    Play {
                        id: "o".into(),
                        row: r.clone(),
                        g,
                        rt,
                        names,
                        me: i,
                        online: true,
                        hud: HudDriver::default(),
                        placed: 0,
                        groups: Vec::new(),
                        sense: Sense::new(HashMap::new()),
                        was: Was::default(),
                        pending: Vec::new(),
                        sync: EndSync::default(),
                        verdicts: Verdicts::default(),
                    }
                });
                if let Some((q, clock)) = leaver
                    && q == i
                    && play.rt.state == St::RunChallenge
                    && play.g.elapsed(&play.rt) >= clock
                {
                    left_at = t;
                    (0..n).for_each(|to| wire.cut(i, to));
                    continue;
                }
                let mut out: Vec<String> = Vec::new();
                if let Some((q, _)) = leaver
                    && t >= left_at + 0.5 + 0.4 * i as f64
                {
                    out.extend(play.left(q));
                }
                out.extend(play.tick(t, None));
                if play.live() {
                    let at = play.g.elapsed(&play.rt);
                    while fed[i] < plans[i].len() && at >= plans[i][fed[i]].0 {
                        let k = plans[i][fed[i]].1;
                        fed[i] += 1;
                        let g = play.g.groups[i].as_ref().unwrap();
                        if g.done[k] {
                            continue;
                        }
                        let scoring = g.done.iter().filter(|d| **d).count() as i64 + 1;
                        out.extend(play.local(Relay::Obj { k, scoring, seq: 500, at }));
                    }
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                out.extend(play.receive(received));
                out.extend(play.tick(t, None));
                wire.send(t, i, n, out);
            }
            if (0..n).filter(staying).all(|i| plays[i].as_ref().is_some_and(|p| p.rt.state == St::Complete)) {
                break;
            }
        }
        (0..n)
            .filter(staying)
            .map(|i| {
                let p = plays[i].as_ref().unwrap();
                assert_eq!(p.rt.state, St::Complete, "seed {seed}: console {i} finished");
                (p.g.finish_reason, p.g.standings.iter().map(|s| (s.player, s.score as i64, (s.time * 1000.).round() as i64)).collect())
            })
            .collect()
    }

    /// Online audit 10-12 (Critical): every console shows the same Gap / Tag and Trick List results. Before the fix a
    /// Gap / Tag skater's time was its relay's arrival on each console (each ranked its own skater first on a tie of
    /// objectives), a Trick List objective two skaters landed within the relay's latency went to each console's own
    /// skater, and an objective in the last moments was dropped on consoles whose timer had ended the activity.
    #[test]
    fn every_console_shows_the_same_lot_results() {
        // Gap / Tag: both skaters finish all 3, 0.1 s apart: player 1 first everywhere.
        let gap = |p: usize| vec![(5., 0), (10., 1), (if p == 1 { 20. } else { 20.1 }, 2)];
        let two = otl_over_the_wire(Kind::GapTag, 3, 2, 1, 0., &gap, None);
        assert_eq!(two[0], two[1], "gap: both consoles agree");
        assert_eq!(two[0].1[0].0, 1, "gap: player 1 finished first");
        // Trick List: both land objective 0, 50 ms apart: player 0 takes it everywhere.
        let list = |p: usize| vec![(10. + 0.05 * p as f64, 0)];
        let two = otl_over_the_wire(Kind::TrickList, 3, 2, 1, 0., &list, None);
        assert_eq!(two[0], two[1], "list: both consoles agree");
        assert_eq!(two[0].1[0].0, 0, "list: player 0 took it");
        // Six skaters, 10% loss: near-simultaneous landings and one in the last 50 ms.
        for seed in 1..=6u64 {
            let plan = move |p: usize| {
                let mut v: Vec<(f64, usize)> = (0..4).map(|j| (5. + 12. * j as f64 + 0.02 * p as f64, (seed as usize + p * 3 + j * 5) % 8)).collect();
                v.push((59.95, (seed as usize + p) % 8));
                v
            };
            for kind in [Kind::GapTag, Kind::TrickList, Kind::Accumulation] {
                let all = otl_over_the_wire(kind, 8, 6, seed, 0.1, &plan, None);
                for c in &all[1..] {
                    assert_eq!(*c, all[0], "{kind:?} seed {seed}: every console the same");
                }
            }
        }
    }

    /// Matrix G2 (10-12): six skaters, 150-300 ms, 5-10 % loss, a guest (player 2) or the host (player 0) quitting. By
    /// hand (freeskate_ol playerdisconnected: Score -1, NumActive - 1; Gap / Tag's sort: Score desc, then Time asc;
    /// Trick List's: Score desc). Gap / Tag, 4 objectives: the timer ends it (three skaters never finish) or the leave
    /// does (everyone else had finished: NumDone >= NumActive). Trick List, 15 objectives a skater claims in turn, the
    /// last claim ending it: the leaver's claims stay taken, it still ranks last.
    #[test]
    fn six_skaters_and_a_leaver_end_own_the_lot_with_the_retail_standings() {
        let order = |all: &[(i64, Vec<(usize, i64, i64)>)], what: &str| -> Vec<usize> {
            for c in &all[1..] {
                assert_eq!(*c, all[0], "{what}: every staying console the same");
            }
            all[0].1.iter().map(|s| s.0).collect()
        };
        for seed in 1..=4u64 {
            let loss = 0.05 + 0.0125 * seed as f64;
            let q = if seed % 2 == 0 { 0 } else { 2 };
            let stayers = |want: &[usize]| want.iter().copied().filter(|&p| p != q).chain([q]).collect::<Vec<_>>();
            // Gap / Tag, the timer ends it: player 1 done at 18, 0 at 20, 5 at 25; 2 has 3, 3 has 2, 4 has 1. The
            // leaver quits at 12 with 2.
            let timed = |p: usize| -> Vec<(f64, usize)> {
                match p {
                    0 => vec![(5., 0), (10., 1), (15., 2), (20., 3)],
                    1 => vec![(5., 0), (10., 1), (15., 2), (18., 3)],
                    2 => vec![(5., 0), (10., 1), (15., 2)],
                    3 => vec![(6., 0), (11., 1)],
                    4 => vec![(7., 0)],
                    _ => vec![(5., 0), (10., 1), (15., 2), (25., 3)],
                }
            };
            let all = otl_over_the_wire(Kind::GapTag, 4, 6, seed, loss, &timed, Some((q, 12.)));
            let what = format!("gap timed seed {seed} leaver {q}");
            let want = if q == 2 { vec![1, 0, 5, 3, 4, 2] } else { vec![1, 5, 2, 3, 4, 0] };
            assert_eq!(order(&all, &what), want, "{what}: {:?}", all[0]);
            assert_eq!(all[0].1[5].1, -1, "{what}: the leaver's score");
            // Gap / Tag, the leave ends it: everyone else done at 20 + p, the leaver (3 of 4) quits at 30.
            let done = move |p: usize| -> Vec<(f64, usize)> {
                let mut v = vec![(5., 0), (10., 1), (15., 2)];
                if p != q {
                    v.push((20. + p as f64, 3));
                }
                v
            };
            let all = otl_over_the_wire(Kind::GapTag, 4, 6, seed, loss, &done, Some((q, 30.)));
            let what = format!("gap left seed {seed} leaver {q}");
            assert_eq!(order(&all, &what), stayers(&[0, 1, 2, 3, 4, 5]), "{what}: {:?}", all[0]);
            assert!(all.iter().all(|c| c.0 == NORMAL), "{what}: finished normally");
            // Trick List: claims 5, 0, 4, 1, 3, 2 (15 objectives, in blocks); the leaver's by 11, then it quits at 14;
            // the others' from 16 on.
            let counts = [5usize, 0, 4, 1, 3, 2];
            let list = move |p: usize| -> Vec<(f64, usize)> {
                let first: usize = counts[..p].iter().sum();
                (0..counts[p]).map(|j| (if p == q { 3. + 2. * j as f64 } else { 16. + 4. * j as f64 + 0.3 * p as f64 }, first + j)).collect()
            };
            let all = otl_over_the_wire(Kind::TrickList, 15, 6, seed, loss, &list, Some((q, 14.)));
            let what = format!("list seed {seed} leaver {q}");
            assert_eq!(order(&all, &what), stayers(&[0, 2, 4, 5, 3, 1]), "{what}: {:?}", all[0]);
            assert!(all.iter().all(|c| c.0 == NORMAL), "{what}: every objective claimed");
            assert_eq!(all[0].1.iter().map(|s| s.1).collect::<Vec<_>>(), stayers(&[0, 2, 4, 5, 3, 1]).iter().map(|&p| if p == q { -1 } else { counts[p] as i64 }).collect::<Vec<_>>(), "{what}: the scores");
        }
    }

    /// A console's online Own the Lot activity (`row`, `n` skaters, this one `me`), started at `t`.
    fn judged_play(r: &Row, n: usize, me: usize, t: f64) -> Play {
        let mut g = Otl::new(r.clone(), n, me).unwrap();
        let mut rt = Rt::new(St::Initial, n, t);
        rt.online = true;
        skate3_modes::start(&mut g, &mut rt);
        let names = (0..n).map(|p| format!("P{p}")).collect();
        Play {
            id: "j".into(),
            row: r.clone(),
            g,
            rt,
            names,
            me,
            online: true,
            hud: HudDriver::default(),
            placed: 0,
            groups: Vec::new(),
            sense: Sense::new(HashMap::new()),
            was: Was::default(),
            pending: Vec::new(),
            sync: EndSync::default(),
            verdicts: Verdicts::default(),
        }
    }

    /// Online audit 10-12: `n` consoles play a judged activity (Simultaneous Trick / Survival, one objective, one
    /// spot, `required` points) through the match relay ([`skate3_modes::tests::Wire`]), started up to 0.3 s apart;
    /// each skater does its `plan`'s (seconds into its run_challenge, event) on its own console (an objective its
    /// mirror already has is skipped). `leaver` (player, seconds into its run_challenge, reversed) vanishes then: what
    /// it still had on the way to the odd consoles is lost, and the others notice it at different moments (in slot
    /// order, or reversed). Every remaining console's finish reason and standings (player, score, time in ms).
    #[allow(clippy::type_complexity)]
    fn judged_over_the_wire(
        kind: Kind,
        n: usize,
        seed: u64,
        loss: f64,
        required: f64,
        plan: &dyn Fn(usize) -> Vec<(f64, Relay)>,
        leaver: Option<(usize, f64, bool)>,
    ) -> Vec<(i64, Vec<(usize, i64, i64)>)> {
        let r = row(kind, 1, 1, 1, false, 60., 0., required);
        let mut wire = skate3_modes::tests::Wire::new(seed, (0.15, 0.3), loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        let mut fed = vec![0; n];
        let plans: Vec<Vec<(f64, Relay)>> = (0..n).map(plan).collect();
        // When the leaver left, and when each console's run_challenge began (global time): a plan's clock runs on
        // past the time limit (Survival's END_DELAY), unlike the challenge clock.
        let mut left_at = f64::INFINITY;
        let mut run_at = vec![f64::INFINITY; n];
        let staying = |i: &usize| leaver.is_none_or(|(p, ..)| p != *i);
        for frame in 0..60 * 200 {
            let t = frame as f64 / 60.;
            if let Some((q, at, _)) = leaver
                && left_at.is_infinite()
                && plays[q].as_ref().is_some_and(|p| p.rt.state == St::RunChallenge)
                && t - run_at[q] >= at
            {
                left_at = t;
                for j in (1..n).step_by(2) {
                    wire.cut(q, j);
                }
            }
            for i in 0..n {
                if t < starts[i] || !staying(&i) && t >= left_at {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| judged_play(&r, n, i, t));
                let mut out = Vec::new();
                if let Some((q, _, reversed)) = leaver
                    && t >= left_at + 0.5 + 0.4 * if reversed { n - 1 - i } else { i } as f64
                {
                    out.extend(play.left(q));
                }
                out.extend(play.tick(t, None));
                if play.rt.state == St::RunChallenge && run_at[i].is_infinite() {
                    run_at[i] = t;
                }
                if play.live() {
                    let at = play.g.elapsed(&play.rt);
                    while fed[i] < plans[i].len() && t - run_at[i] >= plans[i][fed[i]].0 {
                        let mut ev = plans[i][fed[i]].1.clone();
                        fed[i] += 1;
                        if let Relay::Obj { at: ref mut when, .. } = ev {
                            if play.g.groups[i].as_ref().is_some_and(|g| g.done[0]) {
                                continue;
                            }
                            *when = at;
                        }
                        out.extend(play.local(ev));
                    }
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                out.extend(play.receive(received));
                out.extend(play.tick(t, None));
                wire.send(t, i, n, out);
            }
            if (0..n).filter(staying).all(|i| plays[i].as_ref().is_some_and(|p| p.rt.state == St::Complete)) {
                break;
            }
        }
        (0..n)
            .filter(staying)
            .map(|i| {
                let p = plays[i].as_ref().unwrap();
                assert_eq!(p.rt.state, St::Complete, "console {i} finished");
                (p.g.finish_reason, p.g.standings.iter().map(|s| (s.player, s.score as i64, (s.time * 1000.).round() as i64)).collect())
            })
            .collect()
    }

    /// Online audit 10-12 (Important; user 10-12: the host decides): every console shows the judge's Simultaneous
    /// Trick and Survival. Before the fix each console timed the tolerance windows on its own clock and applied the
    /// skaters' events in the order they reached it: a sequence begun near the end of another skater's window reset
    /// that skater on one console and not on another, and a bail racing the sequence that met the target ended the
    /// streak on one console and finished the activity on another.
    #[test]
    fn every_console_shows_the_judges_verdict() {
        let obj = |seq: i64| Relay::Obj { k: 0, scoring: 1, seq, at: 0. };
        let agree = |all: &[(i64, Vec<(usize, i64, i64)>)], what: &str| {
            for c in &all[1..] {
                assert_eq!(*c, all[0], "{what}: every console the same");
            }
        };
        // Simultaneous Trick: player 1 begins a sequence as player 0's window runs out, then completes.
        for seed in 1..=6u64 {
            let plan = |p: usize| if p == 0 { vec![(10., obj(1000))] } else { vec![(11.95, Relay::Begin), (12.5, obj(1200))] };
            agree(&judged_over_the_wire(Kind::Simul, 2, seed, 0., 0., &plan, None), &format!("simul seed {seed}"));
        }
        // Survival: player 1 bails 50 ms after player 0's sequence meets the target.
        let start = || vec![(0.5, Relay::Moving { moving: true, manual: false, sequence: false }), (0.6, Relay::Spot { volume: 0, inside: true })];
        for seed in 1..=6u64 {
            let plan = |p: usize| {
                let mut v = start();
                v.extend(if p == 0 {
                    vec![(5., Relay::Sequence { score: 1000 }), (12., Relay::Sequence { score: 1000 })]
                } else {
                    vec![(8., Relay::Sequence { score: 1000 }), (12.05, Relay::Wipe)]
                });
                v
            };
            agree(&judged_over_the_wire(Kind::Survival, 2, seed, 0., 3000., &plan, None), &format!("survival seed {seed}"));
        }
        // Six skaters, 10% loss: sequences begun and completed, banked, bails, close together.
        for seed in 1..=6u64 {
            let simul = move |p: usize| {
                (0..5)
                    .flat_map(|j| {
                        let t = 4. + 10. * j as f64 + 0.37 * ((seed as usize + p * 3 + j) % 7) as f64;
                        [(t, Relay::Begin), (t + 0.4 + 0.1 * p as f64, obj(100 * (1 + p as i64 + j as i64)))]
                    })
                    .collect::<Vec<_>>()
            };
            agree(&judged_over_the_wire(Kind::Simul, 6, seed, 0.1, 0., &simul, None), &format!("simul six seed {seed}"));
            let survival = move |p: usize| {
                let mut v = start();
                for j in 0..8 {
                    let t = 3. + 6. * j as f64 + 0.05 * p as f64;
                    v.push((t, if (seed as usize + p + j) % 9 == 0 { Relay::Wipe } else { Relay::Sequence { score: 100 * (1 + ((p + j) % 5) as i64) } }));
                }
                v
            };
            agree(&judged_over_the_wire(Kind::Survival, 6, seed, 0.1, 9000., &survival, None), &format!("survival six seed {seed}"));
        }
    }

    /// Online audit 10-12 (user 10-12): the judge (the host, slot 0) vanishes while a verdict is pending (a tolerance
    /// window running, a streak short of the target), part of what it sent lost on the way: the next slot takes over
    /// judging and every remaining console still shows the same outcome.
    #[test]
    fn a_new_judge_takes_over_when_the_host_leaves() {
        let obj = |seq: i64| Relay::Obj { k: 0, scoring: 1, seq, at: 0. };
        for seed in 1..=4u64 {
            let simul = |p: usize| match p {
                0 => vec![],
                1 => vec![(10., obj(1000))],
                2 => vec![(11., obj(1100))],
                _ => vec![(11.4, obj(1200))],
            };
            let all = judged_over_the_wire(Kind::Simul, 4, seed, 0.05, 0., &simul, Some((0, 10.5, false)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "simul seed {seed}: every remaining console the same");
            }
            let survival = |p: usize| {
                let mut v = vec![(0.5, Relay::Moving { moving: true, manual: false, sequence: false }), (0.6, Relay::Spot { volume: 0, inside: true })];
                v.extend((0..6).map(|j| (4. + 2. * j as f64 + 0.1 * p as f64, Relay::Sequence { score: 400 })));
                v
            };
            let all = judged_over_the_wire(Kind::Survival, 4, seed, 0.05, 5000., &survival, Some((0, 9., false)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "survival seed {seed}: every remaining console the same");
            }
        }
    }

    /// Matrix G2 (10-12): six skaters, 150-300 ms, 5-10 % loss, the host (the judge, slot 0) or a guest (slot 3)
    /// quitting before anyone scores. By hand (playerdisconnected: Score -1; Simultaneous Trick done once every
    /// connected skater completed inside the tolerance, sorted by Score desc; Survival's streak ends at the target,
    /// SkaterData order): Simultaneous Trick, the five land 1000 + 100 p within 1 s: theirs by score, the leaver last;
    /// Survival, the five bank 3 x 400 = the 6000 target: 1200 each, the leaver -1, in slot order.
    #[test]
    fn six_skaters_and_a_leaver_end_the_judged_activities_with_the_retail_standings() {
        let obj = |seq: i64| Relay::Obj { k: 0, scoring: 1, seq, at: 0. };
        for seed in 1..=4u64 {
            let loss = 0.05 + 0.0125 * seed as f64;
            let q = if seed % 2 == 0 { 3 } else { 0 };
            let simul = move |p: usize| if p == q { vec![] } else { vec![(10. + 0.2 * p as f64, obj(1000 + 100 * p as i64))] };
            let all = judged_over_the_wire(Kind::Simul, 6, seed, loss, 0., &simul, Some((q, 8., seed > 2)));
            let what = format!("simul seed {seed} leaver {q}");
            for c in &all[1..] {
                assert_eq!(*c, all[0], "{what}: every staying console the same");
            }
            assert_eq!(all[0].0, NORMAL, "{what}: finished");
            let mut want: Vec<(usize, i64)> = (0..6).rev().filter(|&p| p != q).map(|p| (p, 1000 + 100 * p as i64)).collect();
            want.push((q, -1));
            assert_eq!(all[0].1.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), want, "{what}");
            let survival = move |p: usize| {
                let mut v = vec![(0.5, Relay::Moving { moving: true, manual: false, sequence: false }), (0.6, Relay::Spot { volume: 0, inside: true })];
                if p != q {
                    v.extend((0..3).map(|j| (6. + 2. * j as f64 + 0.1 * p as f64, Relay::Sequence { score: 400 })));
                }
                v
            };
            let all = judged_over_the_wire(Kind::Survival, 6, seed, loss, 6000., &survival, Some((q, 3., seed > 2)));
            let what = format!("survival seed {seed} leaver {q}");
            for c in &all[1..] {
                assert_eq!(*c, all[0], "{what}: every staying console the same");
            }
            assert_eq!(all[0].0, NORMAL, "{what}: met the target");
            let want: Vec<(usize, i64)> = (0..6).map(|p| (p, if p == q { -1 } else { 1200 })).collect();
            assert_eq!(all[0].1.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), want, "{what}");
        }
    }

    /// Online audit 10-12 (end review M8): the judge leaves 0.4 s before the Survival ends (time up, then END_DELAY);
    /// the next slot banks a sequence 50 ms before the end (the old judge never saw it) and, already holding at the
    /// end, is promoted and decides it. The last slot noticed the leave first: before, it took the new judge's End
    /// marker (sent as a follower, before its verdicts) for the judge's and ended without that sequence: the new judge
    /// met the target, it timed out.
    #[test]
    fn a_judge_promoted_at_the_end_decides_before_anyone_ends() {
        let start = || vec![(0.5, Relay::Moving { moving: true, manual: false, sequence: false }), (0.6, Relay::Spot { volume: 0, inside: true })];
        for seed in 1..=4u64 {
            let plan = |p: usize| {
                let mut v = start();
                if p == 1 {
                    v.push((61.95, Relay::Sequence { score: 1000 }));
                }
                v
            };
            let all = judged_over_the_wire(Kind::Survival, 3, seed, 0., 1000., &plan, Some((0, 61.6, true)));
            assert_eq!(all[1], all[0], "seed {seed}: both remaining consoles the same");
            assert_eq!(all[0].0, NORMAL, "seed {seed}: the banked sequence met the target");
        }
    }

    /// Online audit 10-12 (end review I2): a promoted judge's Sync fits one match event record (skate_net
    /// Events::fitted, 1024 bytes) with 10 skaters. Before, its per-skater fields by name overflowed it at about 4
    /// skaters and match_core dropped it: the followers kept their old state and verdict numbers.
    #[test]
    fn a_judges_sync_fits_a_record_at_ten_skaters() {
        for kind in [Kind::Simul, Kind::Survival] {
            let mut g = Otl::new(row(kind, 1, 1, 1, false, 60., 0., 9000.), 10, 0).unwrap();
            let mut rt = Rt::new(St::Initial, 10, 0.);
            rt.online = true;
            skate3_modes::start(&mut g, &mut rt);
            while rt.state != St::RunChallenge {
                let t = rt.due().unwrap();
                skate3_modes::advance(&mut g, &mut rt, t);
            }
            // Long numbers wherever a number goes.
            for (p, s) in g.sk.iter_mut().enumerate() {
                (s.score, s.time, s.finish, s.result, s.volumes, s.tolerance) = (123_456., 59.983_333_333_333_33, -1, 3, 2, Some(99));
                (g.seq[p], g.manual[p], g.in_sequence[p]) = (123_456, true, true);
            }
            (g.total, g.finish_time) = (1_234_567., 59.983_333_333_333_33);
            let sync = Relay::Sync { k: 1234, state: Box::new(g.judged()), applied: vec![9999; 10] };
            let text = serde_json::to_string(&sync).unwrap();
            // Online safety 10-26: signed, and sent on as a leaver's.
            let events = skate_net::roster::Events { token: u64::MAX, first: u32::MAX, items: vec![crate::match_core::at_its_biggest(&text)] };
            assert_eq!(events.fitted().0, 1, "{kind:?}: {} bytes", text.len());
        }
    }

    /// Online audit 10-12 (end review I4): a relayed objective out of range (a modded client, an objective-count
    /// mismatch) and a Sync for another number of skaters are ignored. Before, the first panicked every console (an
    /// Accumulation objective from another console skipped the bounds check) and the second the next Moving.
    #[test]
    fn a_bad_relay_is_ignored() {
        let running = |kind: Kind, n: usize| {
            let r = row(kind, 2, 1, 1, false, 60., 0., 3000.);
            let mut g = Otl::new(r, n, 0).unwrap();
            let mut rt = Rt::new(St::Initial, n, 0.);
            rt.online = true;
            skate3_modes::start(&mut g, &mut rt);
            while rt.state != St::RunChallenge {
                let t = rt.due().unwrap();
                skate3_modes::advance(&mut g, &mut rt, t);
            }
            (g, rt)
        };
        let (mut g, mut rt) = running(Kind::Accumulation, 2);
        g.objective_at(&mut rt, 1, 99, 1, 500, Some(1.));
        assert_eq!(g.total, 0., "nothing counted");
        let (mut g, mut rt) = running(Kind::Survival, 2);
        let (one, _) = running(Kind::Survival, 1);
        g.restore(&mut rt, one.judged());
        g.surviving = true;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Moving { player: 1, moving: false });
        assert_eq!((g.seq.len(), g.manual.len(), g.in_sequence.len()), (2, 2, 2), "the short Sync left alone");
    }

    /// A hostile judge's Sync (i64::MIN skaters and volumes, verdict number u64::MAX) and Log overflow nothing; a
    /// follower's Ask for a Gone verdict (another skater out) is refused by the judge.
    #[test]
    fn a_hostile_relay_cannot_overflow_or_spoof() {
        let r = row(Kind::Survival, 1, 1, 1, false, 60., 0., 9000.);
        let mut play = judged_play(&r, 4, 1, 0.);
        let mut t = 0.;
        while play.rt.state != St::RunChallenge {
            t += 1. / 60.;
            play.tick(t, None);
        }
        let mut j = play.g.judged();
        j.g.3 = i64::MIN;
        j.s.iter_mut().for_each(|s| s.5 = i64::MIN);
        play.receive(vec![(0, Relay::Sync { k: u64::MAX, state: Box::new(j), applied: Vec::new() })]);
        play.receive(vec![(0, Relay::Log { k: u64::MAX, from: 2, n: 1, at: 1., ev: Box::new(Relay::Spot { volume: 0, inside: false }) })]);
        play.left(0);
        assert_eq!(play.g.role, Role::Judge, "this console judges now");
        play.receive(vec![(3, Relay::Ask(1, Box::new(Relay::Gone(2))))]);
        assert!(!play.g.sk[2].disconnected, "a peer cannot drop another skater");
    }

    /// Online audit 10-12 (end review M8): the verdict queues are capped (256): verdicts from a console not taken for
    /// the judge (a lost leave), and this skater's own events.
    #[test]
    fn the_verdict_queues_are_capped() {
        let r = row(Kind::Simul, 1, 1, 1, false, 60., 0., 0.);
        let mut play = judged_play(&r, 3, 1, 0.);
        let mut t = 0.;
        while play.rt.state != St::RunChallenge {
            t += 1. / 60.;
            play.tick(t, None);
        }
        for k in 0..1000 {
            play.receive(vec![(2, Relay::Log { k, from: 2, n: 1, at: 1., ev: Box::new(Relay::Begin) })]);
            play.local(Relay::Begin);
        }
        assert!(play.verdicts.held.len() <= 256, "{} held", play.verdicts.held.len());
        assert!(play.verdicts.unlogged.len() <= 256, "{} own", play.verdicts.unlogged.len());
    }

    /// G8 (online audit 10-28): leaving the ChallengeBoundary starts the OOB static (StartEffect(OOB, OutOfBoundsTime),
    /// freeskate_ol_gap_tag 212), full as the skater is sent back; back inside it goes (203). Before, it was only logged.
    #[test]
    fn leaving_the_lot_raises_the_static_until_back_inside() {
        let mut g = Otl::new(row(Kind::GapTag, 2, 1, 1, true, 60., 0., 0.), 2, 0).unwrap();
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::RunChallenge {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        let out = rt.now;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: BOUNDARY, inside: false });
        skate3_modes::advance(&mut g, &mut rt, out + 2.5);
        assert!((rt.oob_static(rt.now) - 0.5).abs() < 1e-6, "2.5 s of OutOfBoundsTime 5: {}", rt.oob_static(rt.now));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: BOUNDARY, inside: true });
        assert_eq!(rt.oob_static(rt.now), 0., "back inside");
    }

    /// Online audit 10-12 (Important, mixed loading): wait_for_load polls every 0.5 s until every skater still in the
    /// activity exists here (each graph's AreSkatersLoaded); before, the countdown began at once, so a console still
    /// loading another skater started its clock early. A skater that never appears holds no one past LOAD_MAX.
    #[test]
    fn wait_for_load_waits_for_every_skater() {
        let r = row(Kind::GapTag, 2, 1, 1, false, 60., 0., 0.);
        let mut g = Otl::new(r, 2, 0).unwrap();
        let mut rt = Rt::new(St::Initial, 2, 0.);
        g.loaded = false;
        skate3_modes::start(&mut g, &mut rt);
        let step = |g: &mut Otl, rt: &mut Rt<St>| {
            let t = rt.due().unwrap();
            skate3_modes::advance(g, rt, t);
        };
        while rt.state != St::WaitForLoad {
            step(&mut g, &mut rt);
        }
        let since = rt.now;
        for _ in 0..4 {
            step(&mut g, &mut rt);
        }
        assert_eq!(rt.state, St::WaitForLoad, "the other skater is not here yet");
        assert!((rt.now - since - 2.).abs() < 1e-9, "polled every 0.5 s");
        g.loaded = true;
        step(&mut g, &mut rt);
        assert_eq!(rt.state, St::Countdown, "here: the countdown");
        // Never here: given up on after LOAD_MAX.
        let mut g = Otl::new(row(Kind::GapTag, 2, 1, 1, false, 60., 0., 0.), 2, 0).unwrap();
        let mut rt = Rt::new(St::Initial, 2, 0.);
        g.loaded = false;
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::Countdown {
            step(&mut g, &mut rt);
        }
        assert!(rt.now >= LOAD_MAX, "{}", rt.now);
        // The glue's answer (end review M11): every slot still in the activity has its skater here.
        let here = |a: u64| a != 30;
        assert!(all_loaded(&[10, 20], &[false, false], here));
        assert!(!all_loaded(&[10, 30], &[false, false], here), "30's skater is not here yet");
        assert!(all_loaded(&[10, 30], &[false, true], here), "30 left: not waited for");
        assert!(all_loaded(&[10, 20, 30], &[false, false], here), "a slot past the activity's players");
    }

    /// By hand: a Trick List objective taken by one skater is forced on every group (their mirrors post it again, a
    /// no-op), the last one ends the activity; the real group's force is a command for the glue.
    #[test]
    fn a_trick_list_objective_is_taken_for_everyone() {
        let r = row(Kind::TrickList, 2, 1, 1, true, 60., 0., 0.);
        let mut g = Otl::new(r, 2, 0).unwrap();
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        for _ in 0..100 {
            if rt.state == St::RunChallenge {
                break;
            }
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert_eq!(rt.state, St::RunChallenge);
        g.cmds.clear();
        g.objective_done(&mut rt, 1, 0, 1, 500);
        let t = rt.now;
        skate3_modes::advance(&mut g, &mut rt, t);
        assert_eq!(g.sk[1].score, 1.);
        assert!(g.cmds.contains(&Cmd::Force(0, 0)), "{:?}", g.cmds);
        assert!(g.groups[0].as_ref().unwrap().done[0], "skater 0's mirror has it too");
        g.objective_done(&mut rt, 0, 0, 1, 500);
        let t = rt.now;
        skate3_modes::advance(&mut g, &mut rt, t);
        assert_eq!(g.sk[0].score, 0., "already taken");
        g.objective_done(&mut rt, 0, 1, 1, 500);
        let t = rt.now + 2.5;
        skate3_modes::advance(&mut g, &mut rt, t);
        assert_eq!(g.finish_reason, NORMAL);
        assert!(g.reported && rt.state == St::Complete, "{:?}", rt.state);
        assert_eq!(g.standings.iter().map(|s| (s.player, s.score)).collect::<Vec<_>>(), [(0, 1.), (1, 1.)]);
    }

    /// The tool's output (S3_OTL_DIR): the activity rows load, their graphs are the five ported ones, most are playable;
    /// every lot lists playable activities; on a Gap / Tag row a sequence through an objective's volume completes it.
    #[test]
    #[ignore = "requires the Skate 3 Own the Lot data (S3_OTL_DIR)"]
    fn skate3_otl_data() {
        let root = std::path::PathBuf::from(std::env::var("S3_OTL_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let rules = skate3_film::rules(&data).unwrap();
        let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let (mut tried, mut completed) = (0, 0);
        for id in data.rows.keys() {
            let r: Row = data.row(id).unwrap();
            assert!(r.kind().is_some(), "{id}: {}", r.graph);
            let ok = r.playable(id, &rules.points_objective);
            let e = by_kind.entry(r.graph.clone()).or_default();
            (e.0, e.1) = (e.0 + 1, e.1 + usize::from(ok));
            if !ok {
                eprintln!("not playable: {id}");
                continue;
            }
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            if r.kind() == Some(Kind::GapTag) {
                let def = r.group(false).unwrap();
                for k in 0..def.objectives.len() {
                    let text = data.rows[id.as_str()]["groups"]["Objectives"][k].to_string();
                    // Its volumes in the snippet's order, one sequence through them all.
                    let mut volumes: Vec<(usize, u64)> =
                        r.trigger_volumes.keys().filter_map(|v| text.find(&format!("\"{v}\"")).map(|at| (at, v.parse().unwrap()))).collect();
                    if volumes.is_empty() {
                        continue;
                    }
                    volumes.sort();
                    let volume: Vec<u64> = volumes.into_iter().map(|(_, v)| v).collect();
                    let mut groups = vec![(BASE, Group::new(0, &def, &rules.points_objective).unwrap())];
                    let mut sense = Sense::new(rules.trick_groups.clone());
                    // A big line (gaps ask GetSequencePoints >= up to a few thousand).
                    let done: Vec<usize> = skate3_film::sequence_through_all(&volume, 20_000.)
                        .into_iter()
                        .flat_map(|f| skate3_film::feed(&mut groups, &mut sense, f))
                        .filter_map(|(_, _, o)| if let Out::Complete { index, .. } = o { Some(index) } else { None })
                        .collect();
                    tried += 1;
                    completed += usize::from(done.contains(&k));
                }
            }
        }
        eprintln!("otl rows by graph (rows, playable): {by_kind:?}; gap/tag objectives completed through their volumes: {completed} of {tried}");
        let playable: usize = by_kind.values().map(|v| v.1).sum();
        assert!(playable >= 100, "{playable}");
        // 194 of 250 on the disc data: the rest also ask a trick group (fliptricks, handplant), a manual or an air,
        // which the test's grind line lacks.
        assert!(completed * 10 >= tried * 7, "{completed} of {tried}");
        for s in [SELECT_ACTIVITY, YOU_FINISHED, HURRY_UP, "ID_DPO_PLAYERSFINISHED", "ID_FSA_ACCUMULATION_TARGET", "ID_FSA_SURVIVAL_TARGET",
                  "ID_DPO_OBJECTIVESCOMPLETED", "ID_LEADERBOARD_HEADER_OBJECTIVES", "ID_LEADERBOARD_HEADER_TIME", "ID_COMMON_SCORE"] {
            assert!(!data.text(s).starts_with("ID_"), "{s}");
        }
    }
}
