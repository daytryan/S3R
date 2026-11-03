//! Skate 3's Film and Photo challenges online (challenge graphs `objectives_shared` < objectives < default, and
//! `photo_skate2` < objectives_shared), the Career tab's SimulPlay QuantifiedCompletion: every skater plays the same
//! film or photo challenge at once, and the session succeeds only when every skater still there owns or kills it.
//! Data: tools/skate3_online.py `--modes film,photo`, installed at `private/skate3-online/{film,photo}/`: rules.json
//! (per row the start, ChallengeBoundary, the film timer, the Owned / Killed It objective groups with their snippets'
//! trees, the volumes they name; `points_objective` and the skatercollections `trick_groups` beside the rows),
//! strings.json, scripts/ (the graph: the rules' source and the oracle), the intro camera.
//!
//! [`Film`] is the graph handler by handler for a SimulPlay session (line numbers are objectives_shared.lua's;
//! photo_skate2's overrides say so). The objectives run on [`skate3_objectives::Group`] (one per Owned / Killed It
//! group): the graph's Objectives.* calls are [`Cmd`]s the glue applies; the groups' messages come back as
//! [`Msg::GroupObjective`] etc. [`Quantified`] is the SimulPlay manager's QuantifiedCompletion table (RE below); a
//! skater's completion travels as [`Relay::Done`] over `Match::outbox` (net message 24's fields; no new relay kind).
//!
//! SimulPlay manager, as traced (TU3): net message 24 (0x825656a8) fills the player's slot once: +8 time, +16 tier,
//! +20 = 1 for Owned / Killed, 2 for Abandoned; the update 0x82564590 counts the present players (in the session, not
//! left), the completed (+20) and the succeeded (+16), sets the registered DPO to (succeeded, present) and, once
//! every present player has completed, posts SimulPlayComplete with Success = every present player's +20 is 1
//! (0x82564800). RegisterHUD (0x82564c10) makes that DPO format 4 (Players), titled ID_DPO_PLAYERSFINISHED.
use crate::{
    challenge::cutscene::Cutscene,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate3_modes::{self, ChallengeFade, Component, Graph, Handled, HudDriver, Locator, ModeData, Msg, Place, ResultsSetup, Rt, View, Volume, WHITE},
    skate3_objectives::{Group, GroupDef, ObjectiveDef, Out, Skater, Value},
    skate3_ots::Groups,
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Challenge.EActivityCompletionType (0x8283e078): Abandoned 0, Owned 1, Killed 2.
const ABANDONED: i64 = 0;
const OWNED: i64 = 1;
const KILLED: i64 = 2;
/// Challenge.eChallengeTypes: the online film rows are Film (7), the photo rows SpotPhoto (38).
pub(crate) const FILM: i64 = 7;
pub(crate) const SPOT_PHOTO: i64 = 38;
/// FrontEnd.eResultsLBType.PlaceNameStringTime (0x82853a7c: 15).
const PLACE_NAME_STRING_TIME: i64 = 15;
/// HUD2.eDPOFormat.Players, RegisterHUD's DPO (0x82564c10).
const DPO_PLAYERS: i64 = 4;
const START: &str = "StartLocation";
/// The pointers Objectives.MakeObjectivesFromGroup hands the Lua: Owned group i at 100 + i, Killed It at 200 + i.
const OWNED_BASE: u32 = 100;
const KILLED_BASE: u32 = 200;

// ---------------------------------------------------------------- data
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub title: String,
    pub online: bool,
    pub intro: Option<String>,
    pub challenge_type: Option<i64>,
    #[serde(rename = "StartLocation")]
    pub start: Option<Locator>,
    /// RepositionAI's 7th argument at the countdown: the skater starts on foot (film_univ_09, photo_indu_01).
    #[serde(rename = "SpawnOffboard", default)]
    pub spawn_offboard: bool,
    #[serde(rename = "ChallengeBoundary")]
    pub boundary: Option<Volume>,
    #[serde(rename = "CountdownDelay", default)]
    pub countdown_delay: f64,
    #[serde(rename = "PrefilmTime", default)]
    pub prefilm: f64,
    #[serde(rename = "TotalFilmTime", default)]
    pub total: f64,
    #[serde(rename = "FilmTimerLowLimit")]
    pub low_limit: Option<f64>,
    #[serde(rename = "OwnedItHUDDisplayTime", default)]
    pub owned_time: f64,
    #[serde(rename = "KilledItHUDDisplayTime", default)]
    pub killed_time: f64,
    #[serde(rename = "OutOfBoundsTime", default)]
    pub out_time: f64,
    #[serde(rename = "AllowPlayerBail")]
    pub allow_bail: Option<bool>,
    #[serde(rename = "AllowObjectiveFailure")]
    pub allow_failure: Option<bool>,
    #[serde(rename = "ResetObjectivesOnPlayerUseSessionMarker")]
    pub marker_resets: Option<bool>,
    /// [setup]'s `self.AllowSessionMarkersToBeSet == true` (absent: false).
    #[serde(rename = "AllowSessionMarkersToBeSet", default)]
    pub allow_markers: bool,
    #[serde(rename = "ModifiedCentralMessageHALIDTimeUp")]
    pub time_up: Option<String>,
    #[serde(rename = "ModifiedCentralMessageHALIDOutOfBounds")]
    pub out_of_bounds: Option<String>,
    #[serde(rename = "ModifiedCentralMessageHALIDScoredLow")]
    pub scored_low: Option<String>,
    #[serde(rename = "CentralMessageHALIDReturnToChallengeArea")]
    pub return_msg: Option<String>,
    #[serde(rename = "CentralMessageHALIDChallengeFailed")]
    pub failed_msg: Option<String>,
    #[serde(default)]
    pub groups: Groups,
    #[serde(default)]
    pub trigger_volumes: BTreeMap<String, Option<Volume>>,
    /// tObjectiveFlow {type, count}.
    pub flow: Option<[u32; 2]>,
    /// The local data's TrackUniqueTrickGroup (a skatercollections trick group, `trick_groups`' key): set,
    /// Objectives.IsTrackedUniqueTrickGroup is true (0x82c60630) and every group tracks it (group+120).
    pub unique_trick_group: Option<String>,
    /// G7: the row's VisualIndicators with their look (tools/skate3_online.py chevrons; none in an older export).
    #[serde(default)]
    pub visual_indicators: Vec<crate::skate3_race::Chevron>,
}

impl Row {
    /// A group's PointRequirement as the Lua reads it (an unset field is the attribute's 0).
    fn requirement(g: &Option<GroupDef>) -> i64 {
        g.as_ref().and_then(|g| g.point_requirement).map_or(0, i64::from)
    }
    /// Playable: online, Film or SpotPhoto, a start, flow type 0 (activate all, 0x82c6d168), every group whole and
    /// ported (skate3_objectives with [`Sense`]'s natives), every volume its snippets name placed, on a known map.
    pub fn playable(&self, id: &str, points: &ObjectiveDef) -> bool {
        let groups = self.groups.owned.iter().chain(&self.groups.killed);
        self.online
            && matches!(self.challenge_type, Some(FILM | SPOT_PHOTO))
            && self.start.is_some()
            && self.flow.is_none_or(|f| f[0] == 0)
            && !self.groups.owned.is_empty()
            && groups.clone().all(|g| g.as_ref().is_some_and(|g| Group::supported(g, points, SENSE_NATIVES).is_ok()))
            && self.trigger_volumes.values().all(Option::is_some)
            && skate3_modes::map_of(id).is_some()
    }
}

// ---------------------------------------------------------------- SimulPlay QuantifiedCompletion
/// The SimulPlay manager's slots for QuantifiedCompletion (see the module notes).
#[derive(Clone, Debug, Default)]
pub(crate) struct Quantified {
    /// +20: 0 not yet, 1 Owned / Killed, 2 Abandoned.
    pub done: Vec<u8>,
    pub tier: Vec<i64>,
    pub time: Vec<f64>,
    pub present: Vec<bool>,
    /// This console's completion for the others (net message 24).
    pub sent: Vec<Relay>,
}
impl Quantified {
    pub fn new(players: usize) -> Self {
        Quantified { done: vec![0; players], tier: vec![0; players], time: vec![0.; players], present: vec![true; players], sent: Vec::new() }
    }
    /// 0x825656a8: a player's completion, kept once.
    pub fn record(&mut self, player: usize, time: f64, tier: i64) {
        if self.done.get(player) == Some(&0) {
            (self.done[player], self.tier[player], self.time[player]) = (if tier != 0 { 1 } else { 2 }, tier, time);
        }
    }
    /// The DPO's numbers: (succeeded, present) (0x82564590).
    pub fn dpo(&self) -> (i64, i64) {
        let present = (0..self.done.len()).filter(|&p| self.present[p]);
        (present.clone().filter(|&p| self.tier[p] != 0 && self.done[p] != 0).count() as i64, present.count() as i64)
    }
    /// 0x82564590's tail: once every present player has completed, SimulPlayComplete with Success (0x82564800).
    fn check(&self) -> Option<bool> {
        let present: Vec<usize> = (0..self.done.len()).filter(|&p| self.present[p]).collect();
        present.iter().all(|&p| self.done[p] != 0).then(|| present.iter().all(|&p| self.done[p] == 1))
    }
}

/// What one console tells the others: its completion (net message 24: time, tier).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "film")]
pub(crate) enum Relay {
    Done { time: f64, tier: i64 },
}

// ---------------------------------------------------------------- the graph
/// HUD2.ShowModifiedCentralMessage(text) / ShowCentralMessage(text). ponytail: a row without the text passes nil to
/// the native (not traced); nothing is shown.
fn modified(rt: &mut Rt<St>, text: &Option<String>) {
    if let Some(t) = text {
        rt.modified(t, WHITE, &[], false);
    }
}
fn central(rt: &mut Rt<St>, text: &Option<String>) {
    if let Some(t) = text {
        rt.central(t, WHITE, &[], 0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    PinPlayer,
    MakeObjectives,
    PrefilmDisplay,
    NisOverview,
    Setup,
    WaitForAssetload,
    SpawnPlayer,
    Countdown,
    ChallengeRunning,
    ChallengeCleanup,
    KilledIt,
    HighlightReel,
    EndOptions,
    Complete,
    DisplayPhoto,
}

/// Challenge.eChallengeFlow (0x8283e1d0): Win 2, Lose 3, Kill 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Win,
    Lose,
    Kill,
}

/// What the graph asks of its objective groups (Objectives.*), applied by the glue in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    /// MakeObjectivesFromGroup(LocalPlayerID, i, "ObjectivesOwned" / "ObjectivesKilledIt", ...): the group's pointer.
    Make(u32),
    Evaluate(u32, bool),
    Reset(u32),
    FailAll(u32),
    /// ActivateChallengeStartObjectives.
    Start(u32),
    Delete(u32),
    /// AddLastUniqueTrickToList.
    AddUnique(u32),
}

pub(crate) struct Film {
    row: Row,
    photo: bool,
    me: usize,
    /// GetSimulPlayCompletionType (0x82564428).
    stored: i64,
    pub sp: Quantified,
    pub cmds: Vec<Cmd>,
    owned: Vec<u32>,
    killed: Vec<u32>,
    use_dpo: bool,
    dpo_req: i64,
    locked_out: bool,
    film_expired: bool,
    out_of_bound: bool,
    wipe_out: bool,
    all_complete: bool,
    result: Flow,
    current: i64,
    complete_result: i64,
    pub score: i64,
    film_timer: Option<u32>,
    countdown_timer: Option<u32>,
    countdown_delay: Option<u32>,
    warning_timer: Option<u32>,
    oob_timer: Option<u32>,
    not_enough_timer: Option<u32>,
    owned_pending: Option<u32>,
    wipeout_timer: Option<u32>,
    failed_timer: Option<u32>,
    session_reset_timer: Option<u32>,
    dpo_timer: Option<u32>,
    wait_hud_timer: Option<u32>,
    central_delay: Option<u32>,
    replay_wait: Option<u32>,
    intro_len: f64,
    /// The intro NIS while its presentation runs (the glue plays it).
    pub scene: Option<String>,
    /// G7: the row's VisualIndicators are up ([countdown] AddIndicator until [complete]'s teardown).
    pub indicators: bool,
}

impl Film {
    pub fn new(row: Row, intro_len: f64, players: usize, me: usize, stored: i64) -> Self {
        let photo = row.challenge_type == Some(SPOT_PHOTO);
        Film {
            row,
            photo,
            me,
            stored,
            sp: Quantified::new(players),
            cmds: Vec::new(),
            owned: Vec::new(),
            killed: Vec::new(),
            use_dpo: false,
            dpo_req: 0,
            locked_out: false,
            film_expired: false,
            out_of_bound: false,
            wipe_out: false,
            all_complete: false,
            result: Flow::Lose,
            current: ABANDONED,
            complete_result: ABANDONED,
            score: 0,
            film_timer: None,
            countdown_timer: None,
            countdown_delay: None,
            warning_timer: None,
            oob_timer: None,
            not_enough_timer: None,
            owned_pending: None,
            wipeout_timer: None,
            failed_timer: None,
            session_reset_timer: None,
            dpo_timer: None,
            wait_hud_timer: None,
            central_delay: None,
            replay_wait: None,
            intro_len,
            scene: None,
            indicators: false,
        }
    }

    pub fn groups(&self) -> impl Iterator<Item = u32> + '_ {
        self.owned.iter().chain(&self.killed).copied()
    }

    fn cmd(&mut self, rt: &mut Rt<St>, c: Cmd) {
        rt.note(match &c {
            Cmd::Make(g) => format!("make {g}"),
            Cmd::Evaluate(g, b) => format!("eval {g} {b}"),
            Cmd::Reset(g) => format!("reset {g}"),
            Cmd::FailAll(g) => format!("failall {g}"),
            Cmd::Start(g) => format!("start {g}"),
            Cmd::Delete(g) => format!("delete {g}"),
            Cmd::AddUnique(g) => format!("addunique {g}"),
        });
        self.cmds.push(c);
    }
    /// The same call on every Owned group, then every Killed It group (the Lua's paired loops).
    fn each(&mut self, rt: &mut Rt<St>, c: impl Fn(u32) -> Cmd) {
        for g in self.groups().collect::<Vec<_>>() {
            self.cmd(rt, c(g));
        }
    }

    fn start_place(&self) -> Place {
        Place { field: START, index: 0, slot: self.me }
    }

    /// vars:ChallengeResultSelector (81-115) for the Film / SpotPhoto types (the boundary rule's ChallengeHasNoBoundary
    /// is false for both). ponytail: vars.NotEnoughPoints is never set by the graph, so its branch is left out.
    fn selector(&mut self, rt: &mut Rt<St>) -> bool {
        if self.all_complete {
            true
        } else if self.out_of_bound {
            // AI.RepositionAI(LocalPlayerID, StartLocation): slot and fade at their defaults (the player, 1).
            let place = self.start_place();
            rt.reposition(self.me, place, 1.);
            self.out_of_bound = false;
            false
        } else if self.wipe_out {
            self.wipe_out = false;
            false
        } else {
            self.film_expired
        }
    }

    /// The Owned / Killed It group a pointer is: (is Killed It, its 1-based index).
    fn which(&self, group: u32) -> Option<bool> {
        if self.owned.contains(&group) {
            Some(false)
        } else if self.killed.contains(&group) {
            Some(true)
        } else {
            None
        }
    }

    /// The SimulPlay manager's update after a slot or the session changed: the DPO, then the completion.
    fn simul_update(&mut self, rt: &mut Rt<St>) {
        (rt.hud.dpo.a, rt.hud.dpo.b) = self.sp.dpo();
        if let Some(success) = self.sp.check() {
            rt.post(Msg::SimulDone(success));
        }
    }

    /// Another console's completion (net message 24).
    pub fn remote_done(&mut self, rt: &mut Rt<St>, player: usize, time: f64, tier: i64) {
        if rt.gone.get(player).copied().unwrap_or(true) {
            return;
        }
        self.sp.record(player, time, tier);
        self.simul_update(rt);
    }

    /// SimulPlay.SimulPlayPlayerComplete(time, tier) (0x82838d48): this skater's slot, and the others told.
    fn player_complete(&mut self, rt: &mut Rt<St>, time: f64) {
        let tier = match self.result {
            Flow::Win => OWNED,
            Flow::Kill => KILLED,
            Flow::Lose => ABANDONED,
        };
        rt.note(format!("playercomplete {time:.2} {tier}"));
        self.sp.record(self.me, time, tier);
        self.sp.sent.push(Relay::Done { time, tier });
        self.simul_update(rt);
        let t = rt.add_timer(self.row.owned_time.max(self.row.killed_time));
        self.wait_hud_timer = Some(t);
    }

    /// The treatment shown for the result, hidden (killed_it / challenge_cleanup / display_photo's timers).
    fn hide_treatment(&mut self, rt: &mut Rt<St>) {
        match self.result {
            Flow::Win => rt.show(Component::OwnedItHUD, false),
            Flow::Kill => rt.show(Component::KilledItHUD, false),
            Flow::Lose => {}
        }
    }

    /// [challenge_running] timerexpired (381-500).
    fn running_timer(&mut self, rt: &mut Rt<St>, id: u32) {
        let complete = self.selector(rt);
        let is = |t: Option<u32>| t == Some(id);
        if is(self.film_timer) && !self.locked_out {
            modified(rt, &self.row.time_up);
            self.result = Flow::Lose;
            self.film_expired = true;
            rt.change(St::ChallengeCleanup);
        } else if is(self.dpo_timer) {
            rt.dpo_params(0, Some(self.dpo_req));
        } else if is(self.oob_timer) {
            modified(rt, &self.row.out_of_bounds);
            self.result = Flow::Lose;
            self.out_of_bound = true;
            if complete {
                rt.change(St::ChallengeCleanup);
            } else {
                // RepositionAI(id, StartLocation, id, true) (401): the 4th sets the session marker there too.
                let place = self.start_place();
                rt.reposition_marked(self.me, place, 1.);
            }
        } else if is(self.warning_timer) {
            // ChallengeSpeech only.
        } else if is(self.not_enough_timer) {
            self.each(rt, Cmd::FailAll);
            modified(rt, &self.row.scored_low);
            if !self.photo {
                self.result = Flow::Lose;
                for g in self.groups().collect::<Vec<_>>() {
                    self.cmd(rt, Cmd::Reset(g));
                    self.cmd(rt, Cmd::Evaluate(g, false));
                }
            } else {
                for g in self.groups().collect::<Vec<_>>() {
                    self.cmd(rt, Cmd::Reset(g));
                    self.cmd(rt, Cmd::Evaluate(g, true));
                }
                self.locked_out = false;
            }
            if complete {
                rt.change(St::ChallengeCleanup);
            }
        } else if is(self.wipeout_timer) {
            rt.pause(false);
            self.each(rt, Cmd::Reset);
            if self.row.allow_bail != Some(true) && complete {
                rt.change(St::ChallengeCleanup);
            }
            if !self.photo {
                let place = self.start_place();
                rt.reposition(self.me, place, 1.);
            }
        } else if is(self.failed_timer) {
            self.each(rt, Cmd::Reset);
            if self.row.allow_failure != Some(true) && complete {
                rt.change(St::ChallengeCleanup);
            }
        } else if is(self.owned_pending) {
            self.owned_pending = None;
            rt.show(Component::OwnedItHUD, true);
            rt.change(St::ChallengeCleanup);
        } else if is(self.session_reset_timer) {
            self.each(rt, Cmd::Reset);
        }
    }

    /// [challenge_running] allobjectivecomplete (585-655).
    fn all_objectives(&mut self, rt: &mut Rt<St>, group: u32) {
        if self.locked_out && self.owned_pending.is_none() {
            return;
        }
        let can_kill = !self.killed.is_empty();
        let Some(killed) = (if self.killed.contains(&group) { Some(true) } else if self.owned.contains(&group) { Some(false) } else { None }) else {
            self.current = ABANDONED;
            return;
        };
        self.current = if killed { KILLED } else { OWNED };
        if !(self.stored == ABANDONED || !can_kill || killed) {
            return;
        }
        self.locked_out = true;
        rt.pause(true);
        rt.disable_use_marker(true);
        if killed {
            self.owned_pending = None;
            self.result = Flow::Kill;
        } else {
            self.result = Flow::Win;
        }
        self.all_complete = true;
        if self.use_dpo && let Some(t) = self.dpo_timer.take() {
            rt.remove_timer(t);
        }
        if self.selector(rt) {
            if killed {
                rt.show(Component::KilledItHUD, true);
                rt.change(St::ChallengeCleanup);
            } else {
                self.owned_pending = Some(rt.add_timer(0.5));
            }
        }
    }

    /// [challenge_running] objectivecomplete (657-813). ponytail: the highlight reel keeps no replay here
    /// (HighlightReel.* only builds the reel the graph plays), and no online row gives an objective BonusTime.
    fn objective(&mut self, rt: &mut Rt<St>, group: u32, points: bool, accumulated: i64) {
        if self.locked_out && self.owned_pending.is_none() {
            return;
        }
        if self.which(group).is_none() || points {
            return;
        }
        self.score = accumulated;
        if self.row.unique_trick_group.is_some() {
            // ponytail: HUD2.ABDHUDRefreshFilmTrickList (the ABD HUD's unique trick list) is not drawn.
            self.cmd(rt, Cmd::AddUnique(group));
        }
        if self.use_dpo {
            rt.dpo_params(accumulated, Some(self.dpo_req));
            self.dpo_timer = Some(rt.add_timer(1.));
        }
    }

    /// The group a failure / shortfall message names, as the handlers pick it (851-911): the Owned group while the
    /// challenge was never completed, else the Killed It group (`lone`: an Owned group too when there is no Killed It).
    fn failing(&self, group: u32, lone: bool) -> Option<u32> {
        let killed = self.which(group)?;
        match (self.stored == ABANDONED, killed) {
            (true, false) | (false, true) => Some(group),
            (false, false) if lone && self.killed.is_empty() => Some(group),
            _ => None,
        }
    }

    /// [challenge_running] objectivefailed (885-933). ponytail: Objectives.GetObjectiveFailureHALText's text (the
    /// failure definition's HAL word) is not exported; no message is shown.
    fn failed(&mut self, rt: &mut Rt<St>, group: u32) {
        if self.locked_out {
            return;
        }
        let Some(valid) = self.failing(group, true) else { return };
        if self.row.allow_failure == Some(false) {
            self.cmd(rt, Cmd::FailAll(valid));
        }
        self.result = Flow::Lose;
        let complete = self.selector(rt);
        if self.photo {
            self.failed_timer = Some(rt.add_timer(1.5));
        } else if complete && self.row.allow_failure != Some(true) {
            rt.change(St::ChallengeCleanup);
        }
    }
}

impl Graph for Film {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::PinPlayer => "pin_player",
            St::MakeObjectives => "make_objectives",
            St::PrefilmDisplay => "prefilm_display",
            St::NisOverview => "nis_overview",
            St::Setup => "setup",
            St::WaitForAssetload => "wait_for_assetload",
            St::SpawnPlayer => "spawn_player",
            St::Countdown => "countdown",
            St::ChallengeRunning => "challenge_running",
            St::ChallengeCleanup => "challenge_cleanup",
            St::KilledIt => "killed_it",
            St::HighlightReel => "highlight_reel",
            St::EndOptions => "end_options",
            St::Complete => "complete",
            St::DisplayPhoto => "display_photo",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        // A skater leaving: the SimulPlay manager's update (the graph has no playerdisconnected handler).
        if let Msg::Left(p) = msg {
            if let Some(x) = self.sp.present.get_mut(*p) {
                *x = false;
            }
            self.simul_update(rt);
        }
        let me = self.me;
        match (rt.state, msg) {
            (St::Initial, Msg::Enter) => rt.change(St::PinPlayer),
            // [pin_player] stateenter (10-18).
            (St::PinPlayer, Msg::Enter) => {
                rt.pin(me, true);
                rt.pause(true);
                rt.disable_use_marker(true);
                rt.change(St::MakeObjectives);
            }
            // [make_objectives] stateenter (21-133): SimulPlay, so no competitors.
            (St::MakeObjectives, Msg::Enter) => {
                rt.pin(me, true);
                (self.locked_out, self.use_dpo, self.dpo_req) = (false, false, 0);
                self.owned = (1..=self.row.groups.owned.len() as u32).map(|i| OWNED_BASE + i).collect();
                for g in self.owned.clone() {
                    self.cmd(rt, Cmd::Make(g));
                }
                for g in self.owned.clone() {
                    self.cmd(rt, Cmd::Evaluate(g, false));
                }
                self.killed = (1..=self.row.groups.killed.len() as u32).map(|i| KILLED_BASE + i).collect();
                for (i, g) in self.killed.clone().into_iter().enumerate() {
                    self.cmd(rt, Cmd::Make(g));
                    let req = Row::requirement(&self.row.groups.killed[i]);
                    if 0 < req {
                        (self.use_dpo, self.dpo_req) = (true, req);
                    }
                }
                for g in self.killed.clone() {
                    self.cmd(rt, Cmd::Evaluate(g, false));
                }
                rt.change(St::PrefilmDisplay);
            }
            // [prefilm_display] stateenter (136-164): HUD2.CreateGroup, then hudloaded.
            (St::PrefilmDisplay, Msg::Enter) => {
                rt.post(Msg::HudLoaded);
                (self.film_expired, self.out_of_bound, self.film_timer) = (false, false, None);
            }
            // [prefilm_display] hudloaded (167-189).
            (St::PrefilmDisplay, Msg::HudLoaded) => {
                let shown = match self.stored {
                    ABANDONED => self.owned.first(),
                    OWNED | KILLED => self.killed.first().or(self.owned.first()),
                    _ => None,
                };
                if let Some(g) = shown {
                    rt.note(format!("objhud {g}"));
                }
                rt.note("simulbegin 0");
                rt.note(format!("registerhud {}", self.use_dpo));
                // RegisterHUD (0x82564c10): the DPO up as Players, ID_DPO_PLAYERSFINISHED. ponytail: with the
                // challenge's own DPO in use the manager takes the second one; no online row has one (UseDPOHUD).
                (rt.hud.dpo.visible, rt.hud.dpo.format, rt.hud.dpo.title) = (true, DPO_PLAYERS, "ID_DPO_PLAYERSFINISHED".into());
                (rt.hud.dpo.a, rt.hud.dpo.b) = self.sp.dpo();
                rt.change(St::NisOverview);
            }
            // [nis_overview] (192-209).
            (St::NisOverview, Msg::Enter) => {
                if self.row.intro.is_some() {
                    self.scene = self.row.intro.clone();
                    rt.present(self.intro_len);
                } else {
                    rt.change(St::Setup);
                }
            }
            (St::NisOverview, Msg::Presented) => {
                self.scene = None;
                rt.change(St::Setup);
            }
            // [setup] stateenter (1330-1406).
            (St::Setup, Msg::Enter) => {
                (self.countdown_timer, self.oob_timer, self.warning_timer, self.central_delay, self.replay_wait) = (None, None, None, None, None);
                (self.not_enough_timer, self.owned_pending) = (None, None);
                self.result = Flow::Lose;
                self.score = 0;
                (self.wipe_out, self.film_expired, self.all_complete) = (false, false, false);
                self.film_timer = None;
                self.complete_result = ABANDONED;
                if self.row.allow_markers {
                    rt.disable_set_marker(false);
                }
                rt.change(St::WaitForAssetload);
            }
            // [wait_for_assetload] stateenter (212-218). ponytail: the challenge's assets are always loaded here.
            (St::WaitForAssetload, Msg::Enter) => rt.change(St::SpawnPlayer),
            (St::SpawnPlayer, Msg::Enter) => rt.change(St::Countdown),
            // [countdown] stateenter (258-304): RepositionAI(id, StartLocation, id, true, 1, false, SpawnOffboard).
            // The 4th posts cMsgSetSessionMarker 0x9A96B5F7 {Transform, FootForward, IsOnBoard = !SpawnOffboard} for the
            // local skater (0x82d550a8): the session marker goes to the start. The glue's teleport goes on foot when the
            // row says so (Match::teleport_on_foot). The move fades over 1 s.
            (St::Countdown, Msg::Enter) => {
                self.indicators = true;
                rt.pause(false);
                self.locked_out = false;
                rt.show(Component::ObjectiveHUD, true);
                let place = self.start_place();
                rt.reposition_marked(me, place, 1.);
            }
            // [countdown] teleporttransitionfinished (340-371).
            (St::Countdown, Msg::TransitionDone) => {
                // ponytail: with unique-trick tracking, the ABD HUD (its gesture and list) and
                // HUD2.ShowTrickDisplayContext(false) are not drawn; only the component's visibility is kept.
                if self.row.unique_trick_group.is_some() {
                    rt.show(Component::ABDHUD, true);
                }
                if self.use_dpo {
                    rt.dpo_params(0, Some(self.dpo_req));
                    rt.show(Component::DPOHUD, true);
                }
                rt.show(Component::ObjectiveHUD, true);
                rt.close_loading();
                self.countdown_delay = Some(rt.add_timer(self.row.countdown_delay));
                self.each(rt, |g| Cmd::Evaluate(g, true));
            }
            // [countdown] timerexpired (307-337).
            (St::Countdown, Msg::Timer(id)) => {
                if self.countdown_delay == Some(*id) {
                    let t = rt.add_timer(self.row.prefilm);
                    self.countdown_timer = Some(t);
                    rt.countdown(t);
                } else if self.countdown_timer == Some(*id) {
                    if self.row.total > 0. {
                        let t = rt.add_timer(self.row.total);
                        self.film_timer = Some(t);
                        rt.timer(t);
                    }
                    if let Some(low) = self.row.low_limit.filter(|&l| self.row.total > l) {
                        self.warning_timer = Some(rt.add_timer(self.row.total - low));
                    }
                    rt.pin(me, false);
                    for g in self.groups().collect::<Vec<_>>() {
                        self.cmd(rt, Cmd::Start(g));
                        self.cmd(rt, Cmd::Evaluate(g, true));
                    }
                    rt.change(St::ChallengeRunning);
                }
            }
            // [challenge_running] stateenter (374-379).
            (St::ChallengeRunning, Msg::Enter) => rt.disable_use_marker(false),
            (St::ChallengeRunning, Msg::Timer(id)) => self.running_timer(rt, *id),
            // [challenge_running] wipeout (503-526).
            (St::ChallengeRunning, Msg::Wipeout(p)) if *p == me && !self.locked_out && !self.film_expired && self.row.allow_bail != Some(true) => {
                self.each(rt, Cmd::FailAll);
                self.wipe_out = true;
                self.result = Flow::Lose;
                rt.pause(true);
                self.wipeout_timer = Some(rt.add_timer(1.5));
            }
            // [challenge_running] triggerentercollision / triggerexitcollision (552-583): volume 0 is the
            // ChallengeBoundary.
            (St::ChallengeRunning, Msg::Trigger { player, volume: 0, inside: true }) if *player == me && !self.locked_out => {
                if let Some(t) = self.oob_timer {
                    rt.remove_timer(t);
                }
            }
            (St::ChallengeRunning, Msg::Trigger { player, volume: 0, inside: false }) if *player == me && !self.locked_out => {
                self.oob_timer = Some(rt.add_timer(self.row.out_time));
                central(rt, &self.row.return_msg);
            }
            (St::ChallengeRunning, Msg::GroupAll { group }) => self.all_objectives(rt, *group),
            (St::ChallengeRunning, Msg::GroupObjective { group, points, accumulated, .. }) => self.objective(rt, *group, *points, *accumulated),
            // [challenge_running] didnotgetenoughpoints (852-883).
            (St::ChallengeRunning, Msg::GroupLow { player, group }) if *player == me && !self.locked_out => {
                if let Some(valid) = self.failing(*group, false) {
                    self.locked_out = true;
                    self.not_enough_timer = Some(rt.add_timer(0.));
                    self.cmd(rt, Cmd::FailAll(valid));
                    self.cmd(rt, Cmd::Evaluate(valid, false));
                }
            }
            (St::ChallengeRunning, Msg::GroupFailed { group, .. }) => self.failed(rt, *group),
            // [challenge_running] usemarker (944-967).
            (St::ChallengeRunning, Msg::UseMarker(p)) if *p == me && !self.locked_out => {
                if self.row.marker_resets == Some(true) {
                    self.each(rt, Cmd::FailAll);
                    self.session_reset_timer = Some(rt.add_timer(1.));
                }
            }
            // [challenge_cleanup] stateenter (970-1012; photo_skate2 1432-1454).
            (St::ChallengeCleanup, Msg::Enter) => {
                if self.photo {
                    if self.result == Flow::Lose {
                        central(rt, &self.row.failed_msg);
                    }
                    let time = match self.film_timer.filter(|_| self.row.total > 0.) {
                        Some(t) => self.row.total - rt.countdown_time(t),
                        None => 0.,
                    };
                    self.player_complete(rt, time);
                } else {
                    let left = self.film_timer.filter(|_| self.row.total > 0.).map_or(0., |t| rt.countdown_time(t));
                    if let Some(t) = self.film_timer {
                        rt.remove_timer(t);
                    }
                    for g in self.groups().collect::<Vec<_>>() {
                        self.cmd(rt, Cmd::Evaluate(g, false));
                    }
                    if self.result == Flow::Lose {
                        central(rt, &self.row.failed_msg);
                    }
                    rt.disable_set_marker(true);
                    self.player_complete(rt, self.row.total - left);
                }
            }
            // [challenge_cleanup] simulplaycomplete (1015-1026; photo_skate2 1457-1471).
            (St::ChallengeCleanup, Msg::SimulDone(success)) => {
                // photo_skate2's (its stateenter's own sits in the not-SimulPlay branch).
                if self.photo {
                    rt.disable_set_marker(true);
                }
                if !success {
                    self.result = Flow::Lose;
                }
                rt.change(match (self.photo, success) {
                    (false, _) => St::KilledIt,
                    (true, true) => St::DisplayPhoto,
                    (true, false) => St::EndOptions,
                });
            }
            (St::ChallengeCleanup, Msg::Timer(id)) if self.wait_hud_timer == Some(*id) => self.hide_treatment(rt),
            // [killed_it] (1042-1082).
            (St::KilledIt, Msg::Enter) => {
                rt.pause(true);
                rt.pin(me, true);
                if self.result == Flow::Lose {
                    rt.pause(true);
                    self.central_delay = Some(rt.add_timer(1.5));
                } else {
                    self.replay_wait = Some(rt.add_timer(self.row.owned_time.max(self.row.killed_time)));
                }
            }
            (St::KilledIt, Msg::Timer(id)) if self.central_delay == Some(*id) => {
                self.hide_treatment(rt);
                rt.change(St::EndOptions);
            }
            (St::KilledIt, Msg::Timer(id)) if self.replay_wait == Some(*id) => {
                self.hide_treatment(rt);
                rt.change(St::HighlightReel);
            }
            // [highlight_reel] (1097-1141).
            (St::HighlightReel, Msg::Enter) => rt.reel(),
            (St::HighlightReel, Msg::Reel) => rt.change(St::EndOptions),
            // photo_skate2 [display_photo] (1355-1429). ponytail: no photo scene or photo selection is shown; the
            // reel's nischallengenotify and niscomplete come at once.
            (St::DisplayPhoto, Msg::Enter) => {
                rt.pin(me, true);
                self.replay_wait = Some(rt.add_timer(self.row.owned_time.max(self.row.killed_time)));
            }
            (St::DisplayPhoto, Msg::Timer(id)) if self.central_delay == Some(*id) => rt.change(St::EndOptions),
            (St::DisplayPhoto, Msg::Timer(id)) if self.replay_wait == Some(*id) => {
                self.hide_treatment(rt);
                rt.show(Component::ObjectiveHUD, false);
                if self.use_dpo {
                    rt.show(Component::DPOHUD, false);
                }
                rt.show(Component::DPOHUD, false);
                rt.show(Component::TimerHUD, false);
                rt.reel();
                rt.post(Msg::Nis);
                rt.post(Msg::NisDone);
            }
            (St::DisplayPhoto, Msg::NisDone) => {
                rt.pause(false);
                rt.change(St::EndOptions);
            }
            // [end_options] stateenter (1144-1200), SimulPlay.
            (St::EndOptions, Msg::Enter) => {
                rt.pause(false);
                rt.note(format!("resultsmenu {PLACE_NAME_STRING_TIME}"));
                self.complete_result = match self.result {
                    Flow::Win => OWNED,
                    Flow::Kill => KILLED,
                    Flow::Lose => ABANDONED,
                };
                rt.change(St::Complete);
            }
            // [complete] stateenter (1286-1327).
            (St::Complete, Msg::Enter) => {
                self.indicators = false;
                rt.pin(me, false);
                for g in self.groups().collect::<Vec<_>>() {
                    self.cmd(rt, Cmd::Delete(g));
                }
                if let Some(t) = self.film_timer {
                    rt.remove_timer(t);
                }
                // HUD2.DeleteGroup: every component down.
                (rt.hud.dpo.visible, rt.hud.objective, rt.hud.timer, rt.hud.countdown) = (false, false, None, None);
                (rt.hud.owned_it, rt.hud.killed_it) = (None, None);
                rt.disable_set_marker(false);
                rt.pause(false);
                rt.note(format!("complete {}", self.complete_result));
                rt.leave();
            }
            // No [restarted] (1232-1266, its DisableUseSessionMarker included): SimulPlay never reaches it. Its way in is
            // [end_options] hudretry (OpenContinueRetryDialog, opened only with SimulPlay false, 1193) or [global]
            // restartchallenge (the crossbar's Restart Challenge, item 6, in none of the online lists 0x8261FF58 picks:
            // 0x82205DEC, 0x82205E10.., 0x82205F0C).
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the skater, as the snippets see it
/// PhysicalStateId categories (GetStateCategory).
const GROUND: u32 = 100;
const AIR: u32 = 200;
const GRIND: u32 = 400;
const OFFBOARD: u32 = 500;
/// The manual scorables (catalog 2, 3).
const MANUALS: [&str; 2] = ["nosemanual", "tailmanual"];

/// The skater natives the online Film / Photo snippets call that [`Sense`] answers (the group's own are
/// skate3_objectives::GROUP_NATIVES).
pub(crate) const SENSE_NATIVES: &[&str] = &[
    "Stance.IsFakie",
    "Grind.IsGrindingGUID",
    "Wipeout.IsWipingOut",
    "ObjectiveLocomotion.IsInSequence",
    "ObjectiveLocomotion.SequenceCompleted",
    "ObjectiveLocomotion.WentOffBoard",
    "ObjectiveLocomotion.IsOnGround",
    "ObjectiveLocomotion.IsOffBoard",
    "Grind.IsGrinding",
    "Manual.IsManualing",
    "Manual.IsNoseManualing",
    "Manual.IsTailManualing",
    "Manual.ManualDistance",
    "Grind.GetMaxGrindLengthInSequence",
    "ObjectiveAir.GetCurrentDegreesOfSpin",
    "ObjectiveAir.GetDegreesOfSpin",
    "ObjectiveAir.AirCompleted",
    "ObjectivePoints.GetSequencePoints",
    "ObjectivePoints.GetFinishedSequenceScore",
    "ObjectiveTrigger.IsInVolume",
    "Tricks.DidSpecificTrickThisFrame",
    "Tricks.DidSpecificTrickGroupThisFrame",
    "Tricks.BeganTrick",
    "Tricks.BeganTrickGroup",
];

/// One physics tick of this console's skater.
#[derive(Clone, Default)]
pub(crate) struct Frame {
    category: u32,
    wiping: bool,
    /// The animation output's riding-fakie byte (+155).
    fakie: bool,
    /// While grinding (the filtered state's flag, +80): the grind spline's two GUIDs (+64 / +72).
    grind_guids: Option<[u64; 2]>,
    /// The scorer's sequence: open, banked this tick, its score.
    open: bool,
    landed: bool,
    score: f32,
    holding: Vec<&'static str>,
    manual_distance: f32,
    grind_length: f32,
    /// |82DA8BE0's rotation total| this air.
    spin: f32,
    announced: Vec<&'static str>,
    /// The row's volumes the skater is in (instance ids), and the ChallengeBoundary.
    inside: Vec<u64>,
    boundary: Option<bool>,
    /// The session marker's sets / uses so far.
    markers: (u64, u64),
    /// The board's speed (physical skateboard +0x80).
    speed: f32,
}

/// What Own the Lot's relay reads of a tick (skate3_otl).
impl Frame {
    pub fn sequence_open(&self) -> bool {
        self.open
    }
    /// The sequence that ended this tick: its score and whether it was aborted (banked: `landed`).
    pub fn sequence_end(&self, was_open: bool) -> Option<(i64, bool)> {
        (self.landed || (was_open && !self.open)).then(|| (self.score.round() as i64, !self.landed))
    }
    pub fn wiping(&self) -> bool {
        self.wiping
    }
    pub fn off_board(&self) -> bool {
        self.category == OFFBOARD
    }
    pub fn manual(&self) -> bool {
        self.holding.iter().any(|h| MANUALS.contains(h))
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }
}

/// The skater natives' answers on one tick: this tick, the one before, and what the last air / sequence left.
/// ponytail: the natives are read off the Rust scorer's taps (scoring_runtime.rs); retail's objective natives are
/// not traced one by one: an air "completes" when the state leaves the air category, a manual is a held nose / tail
/// manual, ManualDistance is that carrier's distance in the scorer's units.
#[derive(Default)]
pub(crate) struct Sense {
    now: Frame,
    was: Frame,
    last_spin: f32,
    finished: f32,
    /// The longest grind in the open (or just banked) sequence.
    grind: f32,
    /// skatercollections: trick group -> its scorables' identifiers (rules.json `trick_groups`).
    groups: HashMap<String, Vec<String>>,
}

impl Sense {
    /// Trick.GetCompletedSequenceScore: the last banked sequence's score.
    pub fn finished_score(&self) -> i64 {
        self.finished.round() as i64
    }
    /// A marker set / used this tick (the first tick has no counts before it).
    fn marker_set(&self) -> bool {
        self.was.category != 0 && self.now.markers.0 > self.was.markers.0
    }
    fn marker_used(&self) -> bool {
        self.was.category != 0 && self.now.markers.1 > self.was.markers.1
    }
    pub fn new(groups: HashMap<String, Vec<String>>) -> Self {
        Sense { groups, ..default() }
    }
    fn step(&mut self, frame: Frame) {
        self.was = std::mem::replace(&mut self.now, frame);
        if self.now.open && !self.was.open {
            self.grind = 0.;
        }
        self.grind = self.grind.max(self.now.grind_length);
        if self.was.category == AIR && self.now.category != AIR {
            self.last_spin = self.was.spin;
        }
        if self.now.landed {
            self.finished = self.now.score;
        }
    }
}

fn first_str(args: &[Value]) -> String {
    match args.first() {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

impl Skater for Sense {
    fn native(&mut self, name: &str, args: &[Value]) -> Option<Value> {
        let (f, was) = (&self.now, &self.was);
        let held = |names: &[&str]| f.holding.iter().any(|h| names.contains(h));
        Some(match name {
            "Wipeout.IsWipingOut" => Value::Bool(f.wiping),
            // 0x82c63cb0: the skater's [56] +155, the animation output's riding fakie.
            "Stance.IsFakie" => Value::Bool(f.fakie),
            // 0x82c65d88: argument 1 parsed as a u64 (0x829254f0 / 0x82ae8f78) against the filtered state's grind GUIDs
            // ([64] +64 / +72) while its grinding flag (+80) is set.
            "Grind.IsGrindingGUID" => {
                let guid = first_str(args).parse::<u64>().ok();
                Value::Bool(f.grind_guids.zip(guid).is_some_and(|(g, id)| g.contains(&id)))
            }
            "ObjectiveLocomotion.IsInSequence" => Value::Bool(f.open),
            "ObjectiveLocomotion.SequenceCompleted" => Value::Bool(f.landed),
            "ObjectiveLocomotion.WentOffBoard" => Value::Bool(f.category == OFFBOARD && was.category != OFFBOARD),
            "ObjectiveLocomotion.IsOnGround" => Value::Bool(f.category == GROUND),
            "ObjectiveLocomotion.IsOffBoard" => Value::Bool(f.category == OFFBOARD),
            "Grind.IsGrinding" => Value::Bool(f.category == GRIND),
            "Manual.IsManualing" => Value::Bool(held(&MANUALS)),
            "Manual.IsNoseManualing" => Value::Bool(held(&MANUALS[..1])),
            "Manual.IsTailManualing" => Value::Bool(held(&MANUALS[1..])),
            "Manual.ManualDistance" => Value::Num(f.manual_distance),
            "Grind.GetMaxGrindLengthInSequence" => Value::Num(self.grind),
            "ObjectiveAir.GetCurrentDegreesOfSpin" => Value::Num(if f.category == AIR { f.spin } else { 0. }),
            "ObjectiveAir.GetDegreesOfSpin" => Value::Num(self.last_spin),
            "ObjectiveAir.AirCompleted" => Value::Bool(was.category == AIR && f.category != AIR),
            "ObjectivePoints.GetSequencePoints" => Value::Num(f.score),
            "ObjectivePoints.GetFinishedSequenceScore" => Value::Num(self.finished),
            "ObjectiveTrigger.IsInVolume" => Value::Bool(first_str(args).parse::<u64>().is_ok_and(|v| f.inside.contains(&v))),
            // ponytail: BeganTrick(Group) is read as the announcement (retail's start-of-trick event is not traced).
            "Tricks.DidSpecificTrickThisFrame" | "Tricks.BeganTrick" => Value::Bool(f.announced.contains(&first_str(args).as_str())),
            "Tricks.DidSpecificTrickGroupThisFrame" | "Tricks.BeganTrickGroup" => {
                let group = first_str(args);
                Value::Bool(f.announced.iter().any(|t| self.in_group(&group, t)))
            }
            _ => return None,
        })
    }
    fn in_group(&self, group: &str, trick: &str) -> bool {
        self.groups.get(group).is_some_and(|m| m.iter().any(|t| t == trick))
    }
}

// ---------------------------------------------------------------- the glue
#[derive(Deserialize)]
pub(crate) struct Rules {
    pub points_objective: ObjectiveDef,
    #[serde(default)]
    pub trick_groups: HashMap<String, Vec<String>>,
}

/// One installed data set (film or photo).
struct Set {
    mode: Mode,
    label: &'static str,
    data: ModeData,
    points: ObjectiveDef,
    tricks: HashMap<String, Vec<String>>,
}

pub(crate) fn rules(data: &ModeData) -> Result<Rules, String> {
    let text = std::fs::read_to_string(data.root.join("rules.json")).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[derive(Default)]
struct Probe {
    frames: Vec<Frame>,
}

struct Play {
    id: String,
    mode: Mode,
    row: Row,
    g: Film,
    rt: Rt<St>,
    /// The objective groups by the graph's pointer.
    groups: Vec<(u32, Group)>,
    sense: Sense,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    placed: u32,
    playing: Option<String>,
    /// G7: the row's VisualIndicators as skate3_indicators::show items (none without the install's looks).
    markers: Vec<(String, skate3_modes::Matrix)>,
}

/// G7: Film's and Photo's looks (tools/skate3_online.py look_file).
const FILM_INDICATORS: &str = "private/skate3-online/film/indicators.json";
const PHOTO_INDICATORS: &str = "private/skate3-online/photo/indicators.json";

#[derive(Resource, Default)]
pub(crate) struct FilmGame {
    sets: Vec<Set>,
    play: Option<Play>,
    probe: Probe,
    volumes: Vec<(u64, Volume)>,
    boundary: Option<Volume>,
    live: bool,
    warned: String,
}

fn load(app: &App, mode: Mode, dir: &str, label: &'static str) -> Option<Set> {
    let data = ModeData::install(app, dir, label)?;
    let rules = rules(&data).inspect_err(|e| warn!("Skate 3 {label}: {e}")).ok()?;
    Some(Set { mode, label, data, points: rules.points_objective, tricks: rules.trick_groups })
}

pub(crate) struct Skate3FilmPlugin;
impl Plugin for Skate3FilmPlugin {
    fn build(&self, app: &mut App) {
        let sets: Vec<Set> = [(Mode::Film, "film", "Film"), (Mode::Photo, "photo", "Photo")].into_iter().filter_map(|(m, d, l)| load(app, m, d, l)).collect();
        for s in &sets {
            let ids: Vec<String> = s.data.rows.keys().filter(|id| s.data.row::<Row>(id).is_ok_and(|r| r.playable(id, &s.points))).cloned().collect();
            info!("Skate 3 {}: {} playable rows", s.label, ids.len());
            // Film and Photo are the Online Challenge Map's Career tab (SimulPlay).
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                playable.0.extend(ids.iter().map(|id| crate::match_core::career(id)));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        app.insert_resource(FilmGame { sets, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick while the challenge runs: this skater's [`Frame`].
fn probe(mut game: ResMut<FilmGame>, skater: Res<SkaterRuntime>, marker: Option<Res<crate::session_marker::SessionMarker>>) {
    let FilmGame { probe, volumes, boundary, live, .. } = &mut *game;
    if !*live {
        probe.frames.clear();
        return;
    }
    probe.frames.push(frame_of(&skater, marker.as_deref(), volumes, boundary.as_ref()));
}

/// This tick's [`Frame`]: the skater against a row's volumes (instance ids) and boundary.
pub(crate) fn frame_of(
    skater: &SkaterRuntime,
    marker: Option<&crate::session_marker::SessionMarker>,
    volumes: &[(u64, Volume)],
    boundary: Option<&Volume>,
) -> Frame {
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let at = Vec3::new(at[0], at[1], at[2]);
    let s = &skater.scoring;
    let carriers = s.carrier_metrics();
    let category = skater.player_state.current().category();
    Frame {
        category,
        wiping: skater.wiping_out(),
        fakie: skater.animation.packet.riding_fakie,
        grind_guids: skater.player_state.filtered_output.filter(|f| f.grinding).map(|f| [f.grind.pathed_guid, f.grind.local_guid]),
        open: s.sequence_open(),
        landed: s.sequence_landed,
        score: s.sequence_score,
        holding: s.holding().collect(),
        manual_distance: carriers.iter().flatten().filter(|c| c.0 == 2 || c.0 == 3).map(|c| c.3).fold(0., f32::max),
        // ponytail: the grind's length is the longest carrier distance while grinding (the grind collector's own
        // length is not tapped); online only `> 0` is asked.
        grind_length: if category == GRIND { carriers.iter().flatten().map(|c| c.3).fold(f32::EPSILON, f32::max) } else { 0. },
        spin: s.spin_degrees().0.abs(),
        announced: s.announced.clone(),
        inside: volumes.iter().filter(|(_, v)| v.contains(at)).map(|(id, _)| *id).collect(),
        boundary: boundary.map(|b| b.contains(at)),
        markers: marker.map_or((0, 0), |m| (m.sets, m.uses)),
        speed: {
            let v = skater.player_input.physical.skateboard.vector_80.map(f32::from_bits);
            Vec3::new(v[0], v[1], v[2]).length()
        },
    }
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<FilmGame>| step(world, &mut game));
}

fn group(groups: &mut [(u32, Group)], p: u32) -> Option<&mut Group> {
    groups.iter_mut().find(|g| g.0 == p).map(|g| &mut g.1)
}

/// The graph's Objectives.* calls on the groups.
fn apply(play: &mut Play, points: &ObjectiveDef) {
    for c in std::mem::take(&mut play.g.cmds) {
        match c {
            Cmd::Make(p) => {
                let defs = if p > KILLED_BASE { &play.row.groups.killed } else { &play.row.groups.owned };
                match defs.get((p % 100) as usize - 1).cloned().flatten().map(|d| Group::new(play.me, &d, points)) {
                    Some(Ok(mut g)) => {
                        g.unique_group = play.row.unique_trick_group.clone();
                        play.groups.push((p, g));
                    }
                    Some(Err(e)) => warn!("Skate 3 Film / Photo: {}: {e}", play.id),
                    None => warn!("Skate 3 Film / Photo: {}: objective group {p} missing", play.id),
                }
            }
            Cmd::Evaluate(p, b) => group(&mut play.groups, p).into_iter().for_each(|g| g.enabled = b),
            Cmd::Reset(p) => group(&mut play.groups, p).into_iter().for_each(Group::reset),
            Cmd::FailAll(p) => group(&mut play.groups, p).into_iter().for_each(Group::fail_all),
            Cmd::Start(p) => group(&mut play.groups, p).into_iter().for_each(Group::activate_start),
            Cmd::Delete(p) => play.groups.retain(|g| g.0 != p),
            Cmd::AddUnique(p) => {
                if let Some(g) = group(&mut play.groups, p) {
                    g.add_last_unique();
                }
            }
        }
    }
}

/// Delivers what is due and applies the graph's objective calls until nothing is left.
fn pump(play: &mut Play, points: &ObjectiveDef, now: f64) {
    for _ in 0..64 {
        apply(play, points);
        if play.rt.due().is_none_or(|d| d > now) {
            break;
        }
        skate3_modes::advance(&mut play.g, &mut play.rt, now);
    }
    apply(play, points);
}

/// A test skater's ticks: on the ground, a sequence opening, grinding through a volume (instance id), landing `score`.
#[cfg(test)]
pub(crate) fn sequence_through(volume: u64, score: f32) -> Vec<Frame> {
    sequence_through_all(&[volume], score)
}

/// As [`sequence_through`], through each volume in turn (a gap's volumes in one sequence).
#[cfg(test)]
pub(crate) fn sequence_through_all(volumes: &[u64], score: f32) -> Vec<Frame> {
    let ground = Frame { category: GROUND, ..default() };
    let open = Frame { category: GROUND, open: true, score: score / 2., ..default() };
    let grind = |v| Frame { category: GRIND, open: true, grind_length: 2., score: score / 2., inside: vec![v], announced: vec!["bs_50_50"], ..default() };
    let landed = Frame { category: GROUND, landed: true, score, ..default() };
    let through = volumes.iter().flat_map(|&v| [grind(v), grind(v)]);
    [ground.clone(), open].into_iter().chain(through).chain([landed, ground]).collect()
}

/// One event on every group in order, its messages kept with the group's pointer and points.
fn each(groups: &mut [(u32, Group)], sense: &mut Sense, out: &mut Vec<(u32, i64, Out)>, mut f: impl FnMut(&mut Group, &mut Sense) -> Vec<Out>) {
    for (p, g) in groups.iter_mut() {
        let outs = f(g, sense);
        let accumulated = i64::from(g.accumulated());
        out.extend(outs.into_iter().map(|o| (*p, accumulated, o)));
    }
}

/// One tick of the skater through objective groups: their messages (pointer, accumulated points, message).
/// ponytail: retail's message order within a frame is not traced; here sequencebegin, the volumes entered, the
/// tricks, sequencecomplete, wipeoutover, the marker, then the per-frame update (0x82c6f998).
pub(crate) fn feed(groups: &mut [(u32, Group)], sense: &mut Sense, f: Frame) -> Vec<(u32, i64, Out)> {
    sense.step(f);
    let (s, w) = (sense.now.clone(), sense.was.clone());
    let mut out = Vec::new();
    if s.open && !w.open {
        groups.iter_mut().for_each(|g| g.1.sequence_begin());
    }
    for &v in s.inside.iter().filter(|v| !w.inside.contains(v)) {
        each(groups, sense, &mut out, |g, sense| g.trigger_enter(v, sense));
    }
    for t in &s.announced {
        groups.iter_mut().for_each(|g| g.1.trick(t));
    }
    if s.landed || (w.open && !s.open) {
        let score = s.score.round() as i32;
        each(groups, sense, &mut out, |g, sense| g.sequence_complete(score, !s.landed, sense));
    }
    if !s.wiping && w.wiping {
        groups.iter_mut().for_each(|g| g.1.wipeout_over());
    }
    if sense.marker_set() {
        each(groups, sense, &mut out, |g, sense| g.marker(false, sense));
    }
    if sense.marker_used() {
        each(groups, sense, &mut out, |g, sense| g.marker(true, sense));
    }
    each(groups, sense, &mut out, |g, sense| g.tick(sense));
    out
}

/// One tick of the skater through the groups and the graph: the groups' messages, then the graph's own (wipeout,
/// the boundary, usemarker).
fn frame(play: &mut Play, points: &ObjectiveDef, f: Frame, now: f64) {
    let me = play.me;
    for (group, accumulated, o) in feed(&mut play.groups, &mut play.sense, f) {
        play.rt.post(match o {
            Out::Complete { index, points, .. } => Msg::GroupObjective { group, index, points, accumulated },
            Out::Failed { index, failed } => Msg::GroupFailed { group, index, failed },
            Out::AllComplete => Msg::GroupAll { group },
            Out::NotEnoughPoints => Msg::GroupLow { player: me, group },
            Out::PointsAdded { .. } => continue,
        });
    }
    pump(play, points, now);
    let (s, w) = (&play.sense.now, &play.sense.was);
    let mut msgs = Vec::new();
    if s.wiping && !w.wiping {
        msgs.push(Msg::Wipeout(me));
    }
    if let (Some(inside), Some(was)) = (s.boundary, w.boundary)
        && inside != was
    {
        msgs.push(Msg::Trigger { player: me, volume: 0, inside });
    }
    if play.sense.marker_used() {
        msgs.push(Msg::UseMarker(me));
    }
    for m in msgs {
        skate3_modes::dispatch(&mut play.g, &mut play.rt, m);
        pump(play, points, now);
    }
}

fn step(world: &mut World, game: &mut FilmGame) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let set = (owned && phase != Phase::Idle).then(|| game.sets.iter().position(|s| Some(s.mode) == mode)).flatten();
    let row = set.and_then(|i| game.sets[i].data.row::<Row>(&id).ok());
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 Film / Photo: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
            game.warned = id;
        }
        return;
    }
    let (row, set) = (row.expect("on_map"), set.expect("row"));
    if game.play.is_none() {
        if !matches!(phase, Phase::Running { .. }) {
            world.resource_mut::<Match>().hold = true;
            return;
        }
        game.play = Some(begin(world, &game.sets[set], &id, row, now));
    }
    let FilmGame { sets, play, probe, volumes, boundary, live, .. } = game;
    let set = &sets[set];
    let (data, points) = (&set.data, &set.points);
    let play = play.as_mut().expect("begun");

    if play.online {
        let left: Vec<usize> = {
            let m = world.resource::<Match>();
            (0..play.names.len()).filter(|&p| !play.rt.gone[p] && m.players.get(p).is_none_or(String::is_empty)).collect()
        };
        for p in left {
            play.rt.gone[p] = true;
            skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
        }
    }
    pump(play, points, now);

    // This console's skater through the objective groups.
    *live = !play.groups.is_empty() && !play.rt.left;
    *volumes = play.row.trigger_volumes.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()?))).collect();
    *boundary = play.row.boundary.clone();
    for f in std::mem::take(&mut probe.frames) {
        if *live {
            frame(play, points, f, now);
        }
    }

    // SimulPlay: this console's completion out (net message 24), the others' in.
    let out = std::mem::take(&mut play.g.sp.sent);
    if play.online {
        let received: Vec<(usize, Relay)> = {
            let mut m = world.resource_mut::<Match>();
            m.outbox.extend(out.iter().map(|r| serde_json::to_string(r).expect("plain data")));
            m.inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect()
        };
        for (from, Relay::Done { time, tier }) in received {
            if from != play.me {
                play.g.remote_done(&mut play.rt, from, time, tier);
                pump(play, points, now);
            }
        }
    }

    // RepositionAI(id, StartLocation, ...): this skater to the start.
    if let Some((seq, place)) = play.rt.landed(play.me)
        && play.placed != seq
    {
        play.placed = seq;
        if let Some(at) = play.row.start.as_ref().map(|l| l.slot(place.slot)) {
            let mut m = world.resource_mut::<Match>();
            (m.teleport, m.teleport_on_foot, m.teleport_marks) = (Some(at), play.row.spawn_offboard, play.rt.marked == Some(seq));
        }
    }

    // The intro NIS.
    let want = play.rt.presenting.filter(|&end| now < end).and(play.g.scene.clone()).filter(|_| play.rt.state == St::NisOverview);
    if want != play.playing {
        if want.is_some() {
            world.resource_mut::<Cutscene>().start(&data.root, want.as_deref().expect("some"));
        } else {
            world.resource_mut::<Cutscene>().stop();
        }
        world.resource_mut::<Match>().intro = want.is_some();
        play.playing = want;
    }
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = play.rt.covered && play.playing.is_none();
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        let alpha = if play.playing.is_some() { 0. } else { play.rt.fade_alpha(now) as f32 };
        if fade.0 != alpha {
            fade.0 = alpha;
        }
    }
    let file = if play.mode == Mode::Photo { PHOTO_INDICATORS } else { FILM_INDICATORS };
    crate::skate3_indicators::show(world, file, if play.g.indicators { play.markers.clone() } else { Vec::new() });
    {
        let mut m = world.resource_mut::<Match>();
        // AI.PinAI holds the skater (the countdown, the treatment); the loading screen too.
        m.hold = play.rt.covered || play.rt.pinned.get(play.me).copied().unwrap_or(false) || play.rt.presenting_now();
        m.gestures = skate3_modes::gestures(&m, play.rt.left);
        m.watch = None;
        m.score = play.g.score;
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
        (m.marker_set_disabled, m.marker_use_disabled) = (play.rt.marker_set_disabled, play.rt.marker_use_disabled);
    }
    let view = View { names: &play.names, me: play.me, online: play.online, title: data.text(&play.row.title), label: set.label };
    // The results (end_options, 1162-1171): Rank, Skater, Completion, Time over PlaceNameStringTime. ponytail: the
    // rows are the SimulPlay manager's slots (CompleteChallenge's native fill is not traced): Killed, then Owned, then
    // the rest, each by time; the completion as ID_CELEBRATE_KILLED / ID_COMMON_OWNED.
    let sp = &play.g.sp;
    if play.rt.left && play.rt.hud.results.is_empty() {
        for (rank, p) in results_order(sp).into_iter().enumerate() {
            play.rt.result(p, rank as i64 + 1, (sp.time[p] * 1000.).round() as i64);
        }
    }
    let results = play.rt.left.then(|| ResultsSetup {
        title: data.text(&play.row.title),
        layout: PLACE_NAME_STRING_TIME,
        columns: ["ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_HEADER_COMPLETION", "ID_LEADERBOARD_HEADER_TIME"].map(|c| data.text(c)).to_vec(),
        throwdown: 0,
        choice: None,
        times: (0..play.names.len()).filter(|&p| sp.done[p] != 0).map(|p| (p, format!("#{}", Mode::DeathRace.show((sp.time[p] * 1000.).round() as i64)))).collect(),
        floats: Vec::new(),
        strings: (0..play.names.len())
            .map(|p| {
                let id = match sp.tier[p] {
                    KILLED => "ID_CELEBRATE_KILLED",
                    OWNED => "ID_COMMON_OWNED",
                    _ => "",
                };
                (p, if id.is_empty() { String::new() } else { data.text(id) })
            })
            .collect(),
    });
    let hud = play.rt.hud.clone();
    play.hud.drive(world, data, &hud, &view, results.as_ref(), now);
}

/// The results' order (the ponytail above): Killed, then Owned, then the rest, each by time.
fn results_order(sp: &Quantified) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sp.tier.len()).collect();
    order.sort_by(|&a, &b| sp.tier[b].cmp(&sp.tier[a]).then(sp.time[a].total_cmp(&sp.time[b])));
    order
}

fn begin(world: &mut World, set: &Set, id: &str, row: Row, now: f64) -> Play {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (vec!["Player 1".to_owned()], 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let intro = row.intro.as_ref().and_then(|s| set.data.intros.get(s)).map_or(0., |&l| f64::from(l));
    info!("Skate 3 {}: {id} ({}), {} skaters, this one {me}", set.label, if online { "SimulPlay" } else { "solo" }, names.len());
    // ponytail: GetSimulPlayCompletionType (0x82564428) is the lowest career completion of the skaters there; Skate 3's
    // career save is not kept here, so every row reads Abandoned (the Owned groups first, as a first play).
    let mut g = Film::new(row.clone(), intro, names.len(), me, ABANDONED);
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    g.sp.present = rt.gone.iter().map(|g| !g).collect();
    skate3_modes::start(&mut g, &mut rt);
    let sense = Sense::new(set.tricks.clone());
    // G7: an export from before G7 has no looks: nothing is drawn.
    let markers = if set.data.root.join("indicators.json").is_file() {
        crate::skate3_indicators::looks(&row.visual_indicators)
    } else {
        Vec::new()
    };
    Play { id: id.to_owned(), mode: set.mode, row, g, rt, groups: Vec::new(), sense, names, me, online, hud: HudDriver::default(), placed: 0, playing: None, markers }
}

fn end(world: &mut World, game: &mut FilmGame) {
    for file in [FILM_INDICATORS, PHOTO_INDICATORS] {
        crate::skate3_indicators::show(world, file, Vec::new());
    }
    if let Some(play) = game.play.take() {
        info!("Skate 3 {:?}: {} ended", play.mode, play.id);
        if play.playing.is_some() {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
    }
    game.live = false;
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = false;
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        fade.0 = 0.;
    }
    HudDriver::close(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};
    use serde_json::json;

    /// One oracle session's row settings.
    #[derive(Clone, Copy, Debug)]
    struct Case {
        ctype: i64,
        stored: i64,
        owned: usize,
        killed: usize,
        kreq: i32,
        bail: Option<bool>,
        fail_ok: Option<bool>,
        marker: Option<bool>,
        low: Option<f64>,
        total: f64,
        prefilm: f64,
        /// TrackUniqueTrickGroup set.
        track: bool,
    }

    fn lua_opt<T: std::fmt::Display>(v: Option<T>) -> String {
        v.map_or("nil".to_owned(), |v| v.to_string())
    }

    fn row(c: &Case) -> Row {
        let group = |req: Option<i32>| json!({"PointRequirement": req, "objectives": []});
        serde_json::from_value(json!({
            "title": "ID_T", "online": true, "intro": "film_x", "challenge_type": c.ctype,
            "StartLocation": {"matrix": ([[0.0f32; 4]; 4])}, "ChallengeBoundary": {"min": ([0.0f32; 3]), "max": ([1.0f32; 3])},
            "CountdownDelay": 1.5, "PrefilmTime": c.prefilm, "TotalFilmTime": c.total, "FilmTimerLowLimit": c.low,
            "OwnedItHUDDisplayTime": 3.1, "KilledItHUDDisplayTime": 3.3, "OutOfBoundsTime": 5,
            "AllowPlayerBail": c.bail, "AllowObjectiveFailure": c.fail_ok, "ResetObjectivesOnPlayerUseSessionMarker": c.marker,
            "ModifiedCentralMessageHALIDTimeUp": "ID_TIMEUP", "ModifiedCentralMessageHALIDOutOfBounds": "ID_OOB",
            "ModifiedCentralMessageHALIDScoredLow": "ID_LOW", "CentralMessageHALIDReturnToChallengeArea": "ID_RETURN",
            "CentralMessageHALIDChallengeFailed": "ID_FAILED",
            "groups": {
                "ObjectivesOwned": (0..c.owned).map(|_| group(None)).collect::<Vec<_>>(),
                "ObjectivesKilledIt": (0..c.killed).map(|_| group((c.kreq > 0).then_some(c.kreq))).collect::<Vec<_>>(),
            },
            "flow": [0, 1],
            "unique_trick_group": c.track.then_some("grinds"),
        }))
        .unwrap()
    }

    const SETUP: &str = r#"
local function grp(req) return {PointRequirement = req, Objectives = {{HighlightDefinition = "hl"}}} end
local owned, killed = {Count = OWNED_N}, {Count = KILLED_N}
for i = 1, OWNED_N do owned[i] = grp(0) end
for i = 1, KILLED_N do killed[i] = grp(KREQ) end
SELF = setmetatable({Name = "film", Title = "ID_T", StartLocation = "StartLocation0", ChallengeBoundary = "boundary",
  CountdownDelay = 1.5, PrefilmTime = PREFILM, TotalFilmTime = TOTAL, FilmTimerLowLimit = ROW.low,
  OwnedItHUDDisplayTime = 3.1, KilledItHUDDisplayTime = 3.3, OutOfBoundsTime = 5, AllowPlayerBail = ROW.bail,
  AllowObjectiveFailure = ROW.failok, ResetObjectivesOnPlayerUseSessionMarker = ROW.marker,
  ModifiedCentralMessageHALIDTimeUp = "ID_TIMEUP", ModifiedCentralMessageHALIDOutOfBounds = "ID_OOB",
  ModifiedCentralMessageHALIDScoredLow = "ID_LOW", CentralMessageHALIDReturnToChallengeArea = "ID_RETURN",
  CentralMessageHALIDChallengeFailed = "ID_FAILED", AudioDefineChallengeWinLowTime = 10,
  ObjectivesOwned = owned, ObjectivesKilledIt = killed, VisualIndicators = {Count = 0}, IntroPresentationEvents = {Count = 1}},
  {__index = function() return nil end})
Challenge.eChallengeTypes = {Film = 7, SpotPhoto = 38, AnywherePhoto = 39, SpotFilm = 40, AnywhereFilm = 41, Follow = 42}
Challenge.GetChallengeType = function() return CTYPE end
Challenge.eChallengeFlow = {Exit = 0, Restart = 1, Win = 2, Lose = 3, Kill = 4}
Challenge.CompleteChallenge = function(_, r) log_line("complete " .. r); log_line("leave") end
SessionMarker = setmetatable({DisableUseSessionMarker = function() log_line("use marker disabled true") end,
  EnableUseSessionMarker = function() log_line("use marker disabled false") end,
  DisableSetSessionMarker = function() log_line("set marker disabled true") end,
  EnableSetSessionMarker = function() log_line("set marker disabled false") end}, getmetatable(Challenge))
Objectives.EObjectiveGroupType = {Owned = 0, Killed = 1}
Objectives.MakeObjectivesFromGroup = function(_, i, name)
  local g = 200 + i
  if name == "ObjectivesOwned" then g = 100 + i end
  log_line("make " .. g); return g
end
Objectives.SetEvaluationStatus = function(g, b) log_line("eval " .. g .. " " .. tostring(b)) end
Objectives.ResetObjectives = function(g) log_line("reset " .. g) end
Objectives.FailAllObjectiveStatus = function(g) log_line("failall " .. g) end
Objectives.ActivateChallengeStartObjectives = function(g) log_line("start " .. g) end
Objectives.DeleteObjectiveGroup = function(g) log_line("delete " .. g) end
Objectives.GetAccumulatedPoints = function() return SCORE end
Objectives.GetTotalNumObjectives = function() return 2 end
Objectives.GetCurrentNumCompletedObjectives = function() return 1 end
Objectives.IsTrackedUniqueTrickGroup = function() return TRACK end
Objectives.AddLastUniqueTrickToList = function(g) log_line("addunique " .. g) end
HUD2.SetupObjectiveHUDData = function(_, g) log_line("objhud " .. g) end
ChallengeAsset.IsChallengeAssetLoadComplete = function() return true end
NIS.GetPhotoNISCount = function() return 3 end
FrontEnd.eResultsLBType.PlaceNameStringTime = 15
FrontEnd.Results_SetResultsMenuType = function(t) log_line("resultsmenu " .. t) end
-- photo_skate2's display_photo: the reel, then the photo scene's notify and its end.
if CTYPE == 38 then
  HighlightReel.PlayHighlightReel = function() log_line("reel"); POST("highlightreelcomplete"); POST("nischallengenotify"); POST("niscomplete") end
end
-- The SimulPlay manager's QuantifiedCompletion slots (0x825656a8, 0x82564590, 0x82564800).
DONE = {}
function CHECK()
  local all, ok = true, true
  for i = 0, N - 1 do
    if not GONE[i] then
      if not DONE[i] then all = false elseif DONE[i] ~= 1 then ok = false end
    end
  end
  if all then POST("simulplaycomplete", {Success = ok}) end
end
local function record(p, tier)
  if not DONE[p] then if tier ~= 0 then DONE[p] = 1 else DONE[p] = 2 end end
  CHECK()
end
SimulPlay = setmetatable({eSimulPlayType = {QuantifiedCompletion = 0},
  IsInSimulPlay = function() return true end,
  GetSimulPlayCompletionType = function() return STORED end,
  BeginSimulPlay = function(t) log_line("simulbegin " .. t) end,
  RegisterHUD = function(_, d) log_line("registerhud " .. tostring(d)) end,
  SimulPlayPlayerComplete = function(t, tier) log_line(string.format("playercomplete %.2f %d", t, tier)); record(ME, tier) end},
  getmetatable(Challenge))
function ACTOR() return ME end
function DELIVER(kind, v, w)
  local g = math.floor(w)
  if kind == "all" then SEND("allobjectivecomplete", {ObjectiveGroup = g}); return true end
  if kind == "obj" then
    SCORE = v
    SEND("objectivecomplete", {ObjectiveGroup = g, ObjectiveIndex = 0, IsPointObjective = false, RequiresHighlight = false})
    return true
  end
  if kind == "low" then SEND("didnotgetenoughpoints", {PlayerID = ME, ObjectiveGroup = g}); return true end
  if kind == "fail" then SEND("objectivefailed", {ObjectiveGroup = g, ObjectiveIndex = 0, FailedIndex = 0}); return true end
  if kind == "marker" then SEND("usemarker", {PlayerID = ME}); return true end
  if kind == "exit" then SEND("triggerexitcollision", {PlayerID = ME, VolumeID = "boundary"}); return true end
  if kind == "enter" then SEND("triggerentercollision", {PlayerID = ME, VolumeID = "boundary"}); return true end
  if kind == "done" then if not GONE[g] then record(g, v) end; return true end
  if kind == "leave" then if not GONE[v] then GONE[v] = true; log_line("left " .. v); CHECK() end; return true end
  return false
end
"#;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, k: u64) -> u64 {
            let x = &mut self.0;
            *x ^= *x << 13;
            *x ^= *x >> 7;
            *x ^= *x << 17;
            *x % k
        }
    }

    /// One run of challenge_running: objective messages for real and stray groups, failures, shortfalls, bails, the
    /// boundary, the marker, the others completing or leaving (then, past the film time, the rest completing).
    fn plan(seed: u64, n: usize, me: usize, c: &Case) -> Vec<Vec<(f64, Ev)>> {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let groups: Vec<f64> = (1..=c.owned).map(|i| 100. + i as f64).chain((1..=c.killed).map(|i| 200. + i as f64)).chain([999.]).collect();
        let mut run = Vec::new();
        let mut t = 0.3;
        let (mut gone, mut done) = (vec![false; n], vec![false; n]);
        for _ in 0..r.next(9) {
            t += 0.4 + r.next(40) as f64 / 10.;
            let g = groups[r.next(groups.len() as u64) as usize];
            let other = (n > 1).then(|| (me + 1 + r.next(n as u64 - 1) as usize) % n);
            match r.next(13) {
                0 | 1 => run.push((t, Ev::Other("obj", 100 * r.next(20) as i64, g))),
                2 | 3 => run.push((t, Ev::Other("all", 0, g))),
                4 => run.push((t, Ev::Other("fail", 0, g))),
                5 => run.push((t, Ev::Other("low", 0, g))),
                6 => run.push((t, Ev::Bail)),
                7 => run.push((t, Ev::Other("marker", 0, 0.))),
                8 => {
                    run.push((t, Ev::Other("exit", 0, 0.)));
                    if r.next(2) == 0 {
                        run.push((t + 1. + r.next(60) as f64 / 10., Ev::Other("enter", 0, 0.)));
                    }
                }
                9 | 10 if other.is_some() => {
                    let p = other.expect("checked");
                    done[p] = true;
                    run.push((t, Ev::Other("done", r.next(3) as i64, p as f64)));
                }
                11 if other.is_some() => {
                    let p = other.expect("checked");
                    gone[p] = true;
                    run.push((t, Ev::Leave(p)));
                }
                _ => {}
            }
        }
        // The rest of the session completes after the film time.
        let end = c.total.max(t) + 2.;
        for p in (0..n).filter(|&p| p != me && !gone[p] && !done[p]) {
            run.push((end + p as f64, Ev::Other("done", 1 + r.next(2) as i64, p as f64)));
        }
        vec![run]
    }

    fn oracle(src: &str, n: usize, me: usize, c: &Case, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let setup = format!(
            "N, ME, FIRST, CTYPE, STORED = {n}, {me}, 0, {}, {}\nOWNED_N, KILLED_N, KREQ, TOTAL, PREFILM = {}, {}, {}, {:?}, {:?}\n\
             ROW = {{low = {}, bail = {}, failok = {}, marker = {}}}\nTRACK = {}\n{SETUP}",
            c.ctype,
            c.stored,
            c.owned,
            c.killed,
            c.kreq,
            c.total,
            c.prefilm,
            lua_opt(c.low),
            lua_opt(c.bail),
            lua_opt(c.fail_ok),
            lua_opt(c.marker),
            c.track
        );
        let (lua, errors) = lua_log(src, &setup, "initial", "challenge_running", plans);
        let mut g = Film::new(row(c), 0., n, me, c.stored);
        let mut rt = Rt::new(St::Initial, n, 0.);
        let other = |g: &mut Film, rt: &mut Rt<St>, kind: &str, v: i64, w: f64| {
            let group = w as u32;
            Some(match kind {
                "all" => Msg::GroupAll { group },
                "obj" => Msg::GroupObjective { group, index: 0, points: false, accumulated: v },
                "low" => Msg::GroupLow { player: me, group },
                "fail" => Msg::GroupFailed { group, index: 0, failed: 0 },
                "marker" => Msg::UseMarker(me),
                "exit" => Msg::Trigger { player: me, volume: 0, inside: false },
                "enter" => Msg::Trigger { player: me, volume: 0, inside: true },
                "done" => {
                    g.remote_done(rt, w as usize, 12.5, v);
                    return None;
                }
                _ => return None,
            })
        };
        let model = model_log(&mut g, &mut rt, St::ChallengeRunning, plans, |_| Some(me), |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    fn case(s: usize, n: usize, photo: bool) -> Case {
        let pick = |k: usize, m: usize| (s * 7 + n * 3 + k) % m;
        Case {
            ctype: if photo { SPOT_PHOTO } else { FILM },
            stored: [ABANDONED, ABANDONED, OWNED, KILLED][pick(0, 4)],
            owned: 1 + pick(1, 2),
            killed: pick(2, 3),
            kreq: [0, 0, 500][pick(3, 3)],
            bail: [None, Some(false), Some(true)][pick(4, 3)],
            fail_ok: [None, Some(false), Some(true)][pick(5, 3)],
            marker: [None, Some(true)][pick(6, 2)],
            low: [None, Some(5.)][pick(7, 2)],
            total: [20., 45.][pick(8, 2)],
            prefilm: [0., 2.][pick(9, 2)],
            track: pick(10, 2) == 1,
        }
    }

    fn run_oracle(src: &str, photo: bool) {
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for seed in 1..=60u64 {
            for n in 1..=4usize {
                let s = seed as usize;
                let me = s % n;
                let c = case(s, n, photo);
                let plans = plan(seed * 131 + n as u64, n, me, &c);
                let (lua, model, errs) = oracle(src, n, me, &c, &plans);
                compare(&format!("seed {seed} n {n} me {me} {c:?}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                let keys = [
                    "OwnedItHUD true", "KilledItHUD true", "modified ID_TIMEUP", "modified ID_OOB", "modified ID_LOW", "central ID_FAILED", "failall",
                    "reset 1", "dpo ", "move 0", "move 1", "playercomplete", "left", "complete 0", "complete 1", "complete 2", "reel", "enter killed_it",
                    "enter display_photo", "enter end_options", "leave",
                    "addunique", "show ABDHUD true",
                ];
                for key in keys {
                    *seen.entry(key).or_default() += lua.iter().filter(|l| l.contains(key)).count();
                }
            }
        }
        eprintln!("covered: {seen:?}");
        let skip: &[&str] = if photo { &["enter killed_it", "modified ID_TIMEUP"] } else { &["enter display_photo"] };
        assert!(seen.iter().all(|(k, &n)| n > 0 || skip.contains(k)), "every branch exercised: {seen:?}");
        eprintln!("{} oracle: {games} sessions, {lines} log lines identical ({errors} Lua errors replayed)", if photo { "photo_skate2" } else { "objectives_shared" });
    }

    /// objectives_shared.lua itself (S3_FILM_DIR = tools/skate3_online.py's `--out`/film, with scripts/) decides every
    /// Film SimulPlay session the model runs the same way: the groups' calls, Owned / Killed It, the film timer, bails,
    /// the boundary, the marker, failures and shortfalls, the SimulPlay completion with 1 to 4 skaters, the results.
    #[test]
    #[ignore = "requires the decompiled objectives_shared.lua (S3_FILM_DIR)"]
    fn skate3_film_matches_objectives_shared_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_FILM_DIR").unwrap());
        run_oracle(&std::fs::read_to_string(root.join("scripts/objectives_shared.lua")).unwrap(), false);
    }

    /// photo_skate2.lua (S3_PHOTO_DIR): the same for Photo (its challenge_cleanup and display_photo).
    #[test]
    #[ignore = "requires the decompiled photo_skate2.lua (S3_PHOTO_DIR)"]
    fn skate3_photo_matches_photo_skate2_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_PHOTO_DIR").unwrap());
        run_oracle(&std::fs::read_to_string(root.join("scripts/photo_skate2.lua")).unwrap(), true);
    }

    /// The data (S3_FILM_DIR, S3_PHOTO_DIR): the playable rows, their volumes, intros and strings; every objective
    /// group builds; no playable row starts at its AltStartLocation (UseFilmSetupTime with AllowAltStartLocation).
    #[test]
    #[ignore = "requires the Skate 3 film / photo data (S3_FILM_DIR, S3_PHOTO_DIR)"]
    fn skate3_film_data() {
        for (var, ctype) in [("S3_FILM_DIR", FILM), ("S3_PHOTO_DIR", SPOT_PHOTO)] {
            let data = ModeData::load(std::path::Path::new(&std::env::var(var).unwrap())).unwrap();
            let rules = rules(&data).unwrap();
            let rows: Vec<(String, Row)> = data.rows.keys().map(|id| (id.clone(), data.row::<Row>(id).unwrap())).collect();
            let online = rows.iter().filter(|r| r.1.online).count();
            let playable: Vec<&String> = rows.iter().filter(|(id, r)| r.playable(id, &rules.points_objective)).map(|r| &r.0).collect();
            eprintln!("{var}: {} of {online} online rows playable: {playable:?}", playable.len());
            for (id, r) in rows.iter().filter(|(id, _)| playable.contains(&id)) {
                assert_eq!(r.challenge_type, Some(ctype), "{id}");
                assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
                assert!(r.intro.as_ref().is_some_and(|s| data.intros.contains_key(s)), "{id}");
                assert!(r.boundary.is_some() && r.total > 0., "{id}");
                let raw = &data.rows[id.as_str()];
                assert!(!(raw["UseFilmSetupTime"] == true && raw["AllowAltStartLocation"] == true), "{id}");
                for d in r.groups.owned.iter().chain(&r.groups.killed) {
                    Group::new(0, d.as_ref().unwrap(), &rules.points_objective).unwrap();
                }
                // Every volume a snippet names is placed.
                let text = serde_json::to_string(&raw["groups"]).unwrap();
                for v in r.trigger_volumes.keys() {
                    assert!(text.contains(v.as_str()), "{id}: {v}");
                }
                for native in ["\"ObjectiveTrigger.IsInVolume\",[[\"str\",\"", "\"ObjectiveTrigger.EnteredVolume\",[[\"str\",\""] {
                    for part in text.split(native).skip(1) {
                        let v = part.split('"').next().unwrap();
                        assert!(r.trigger_volumes.contains_key(v), "{id}: volume {v} not exported");
                    }
                }
            }
            assert!(!playable.is_empty());
            for (group, members) in &rules.trick_groups {
                assert!(!members.is_empty(), "{group}");
            }
            for s in ["ID_DPO_PLAYERSFINISHED", "ID_LEADERBOARD_HEADER_COMPLETION", "ID_LEADERBOARD_HEADER_TIME", "ID_COMMON_OWNED", "ID_CELEBRATE_KILLED"] {
                assert!(!data.text(s).starts_with("ID_"), "{s}");
            }
        }
    }

    /// The skater through a row's objective group (pointer 1), the graph's AddLastUniqueTrickToList on each objective
    /// completion (objectives_shared 689-690): every message.
    fn drive(groups: &mut [(u32, Group)], sense: &mut Sense, frames: Vec<Frame>) -> Vec<Out> {
        let mut outs = Vec::new();
        for f in frames {
            for (p, _, o) in feed(groups, sense, f) {
                if let (Out::Complete { points: false, .. }, Some(g)) = (&o, group(groups, p))
                    && g.unique_group.is_some()
                {
                    g.add_last_unique();
                }
                outs.push(o);
            }
        }
        outs
    }

    fn ground() -> Frame {
        Frame { category: GROUND, ..default() }
    }

    /// The rows the engine natives of stage 4 made playable (Stance.IsFakie, Grind.IsGrindingGUID, NumUniqueTrickDone,
    /// Flow.Activate/DeactivateObjective, Reset.ResetAllObjectives), run on their disc snippets: each completes on the
    /// run it asks for and not on a near miss (S3_FILM_DIR, S3_PHOTO_DIR).
    #[test]
    #[ignore = "requires the Skate 3 film / photo data (S3_FILM_DIR, S3_PHOTO_DIR)"]
    fn the_new_natives_rows_complete() {
        let load = |var: &str| {
            let data = ModeData::load(std::path::Path::new(&std::env::var(var).unwrap())).unwrap();
            let rules = rules(&data).unwrap();
            (data, rules)
        };
        let make = |rules: &Rules, row: &Row, def: &Option<GroupDef>| {
            let mut g = Group::new(0, def.as_ref().unwrap(), &rules.points_objective).unwrap();
            g.unique_group = row.unique_trick_group.clone();
            (vec![(1, g)], Sense::new(rules.trick_groups.clone()))
        };
        let all = |outs: &[Out]| outs.contains(&Out::AllComplete);
        let (film, film_rules) = load("S3_FILM_DIR");

        // film_dwtn_03 Owned: the counter (objective 0) activates the manual spots one by one (Flow.Activate /
        // DeactivateObjective); a wipeout resets the group (Reset.ResetAllObjectives).
        let row: Row = film.row("film_dwtn_03").unwrap();
        assert!(row.playable("film_dwtn_03", &film_rules.points_objective));
        let manny = |v: u64| vec![ground(), Frame { open: true, holding: vec!["nosemanual"], inside: vec![v], ..ground() }, Frame { landed: true, ..ground() }, ground()];
        let spots = [3202084649600222420u64, 3202084649600222421, 3202084649600222422, 3202084649600222423];
        let (mut gs, mut sense) = make(&film_rules, &row, &row.groups.owned[0]);
        assert!(drive(&mut gs, &mut sense, manny(spots[1])).is_empty(), "spot 2 is not active before spot 1");
        let outs = drive(&mut gs, &mut sense, manny(spots[0]));
        assert!(matches!(outs[..], [Out::Complete { index: 1, points: false, .. }]), "{outs:?}");
        assert_eq!(gs[0].1.status(2), crate::skate3_objectives::ACTIVE, "the counter activated spot 2");
        drive(&mut gs, &mut sense, vec![Frame { wiping: true, ..ground() }, ground()]);
        assert_eq!((gs[0].1.completed(), gs[0].1.status(1), gs[0].1.status(2)), (0, crate::skate3_objectives::ACTIVE, crate::skate3_objectives::DISABLED), "the wipeout reset the group");
        let outs: Vec<Out> = spots.iter().flat_map(|&v| drive(&mut gs, &mut sense, manny(v))).collect();
        assert!(all(&outs), "{outs:?}");

        // film_univ_01 Killed It (TrackUniqueTrickGroup grinds): each spot's grind needs a grind not done before.
        let row: Row = film.row("film_univ_01").unwrap();
        assert!(row.playable("film_univ_01", &film_rules.points_objective));
        let tracked = row.unique_trick_group.clone().expect("film_univ_01 tracks a trick group");
        let tricks: Vec<&'static str> = film_rules.trick_groups[&tracked].iter().take(6).map(|t| &*Box::leak(t.clone().into_boxed_str())).collect();
        let grind = |v: u64, t: &'static str| {
            let g = Frame { category: GRIND, open: true, inside: vec![v], announced: vec![t], ..default() };
            vec![ground(), Frame { open: true, ..ground() }, g, Frame { landed: true, ..ground() }, ground(), ground(), ground(), ground()]
        };
        let spots = [2415451017807622u64, 2432922944767750, 2432927239735046, 2432931534702342, 3202084649599893978];
        let (mut gs, mut sense) = make(&film_rules, &row, &row.groups.killed[0]);
        assert_eq!(drive(&mut gs, &mut sense, grind(spots[0], tricks[0])).len(), 1);
        assert!(drive(&mut gs, &mut sense, grind(spots[1], tricks[0])).is_empty(), "the same grind again is not unique");
        let outs: Vec<Out> = spots[1..].iter().zip(&tricks[1..]).flat_map(|(&v, &t)| drive(&mut gs, &mut sense, grind(v, t))).collect();
        assert!(all(&outs), "{outs:?}");
        assert_eq!(gs[0].1.unique_done(), 5);

        let (photo, photo_rules) = load("S3_PHOTO_DIR");
        // photo_indu_05 Owned: a grind on the rail GUID 3202084653893748088 between its volumes.
        let row: Row = photo.row("photo_indu_05").unwrap();
        assert!(row.playable("photo_indu_05", &photo_rules.points_objective));
        let rail = |guid: u64| {
            let on = Frame { category: GRIND, open: true, inside: vec![2413200454944518, 3202084649600090389], grind_guids: Some([guid, 7]), ..default() };
            vec![Frame { inside: vec![2413200454944518], ..ground() }, on, Frame { landed: true, ..ground() }, ground()]
        };
        let (mut gs, mut sense) = make(&photo_rules, &row, &row.groups.owned[0]);
        assert!(drive(&mut gs, &mut sense, rail(3202084653893748089)).is_empty(), "another rail");
        assert!(all(&drive(&mut gs, &mut sense, rail(3202084653893748088))));

        // photo_artg_01 Killed It: fakie on the ground through its seven volumes in order.
        let row: Row = photo.row("photo_artg_01").unwrap();
        assert!(row.playable("photo_artg_01", &photo_rules.points_objective));
        let loop_ = |fakie: bool| {
            [3202084649601667841u64, 3202084649601667844, 3202084649601667845, 3202084649601667846, 3202084649601667847, 3202084649601667848, 3202084649601667849]
                .map(|v| Frame { inside: vec![v], fakie, ..ground() })
                .to_vec()
        };
        let (mut gs, mut sense) = make(&photo_rules, &row, &row.groups.killed[0]);
        assert!(drive(&mut gs, &mut sense, loop_(false)).is_empty(), "regular");
        assert!(all(&drive(&mut gs, &mut sense, loop_(true))));
        for (id, rules, data) in [("photo_indu_03", &photo_rules, &photo), ("photo_mmcn_01", &photo_rules, &photo), ("film_artg_03", &film_rules, &film)] {
            assert!(data.row::<Row>(id).unwrap().playable(id, &rules.points_objective), "{id}");
        }
    }

    /// The SimulPlay manager by hand: a slot is kept once; the DPO counts the present players and those who owned or
    /// killed it; SimulPlayComplete once every present player has completed, a success only when all of them did.
    #[test]
    fn quantified_completion() {
        let mut q = Quantified::new(3);
        q.record(0, 10., OWNED);
        assert_eq!((q.dpo(), q.check()), ((1, 3), None));
        q.record(0, 20., ABANDONED);
        assert_eq!((q.done[0], q.time[0]), (1, 10.), "a slot is kept once");
        q.record(1, 12., ABANDONED);
        q.present[2] = false;
        assert_eq!((q.dpo(), q.check()), ((1, 2), Some(false)), "an abandoned run fails the session");
        let mut q = Quantified::new(2);
        q.record(0, 1., KILLED);
        q.record(1, 2., OWNED);
        assert_eq!((q.dpo(), q.check()), ((2, 2), Some(true)));
    }

    /// G7: the row's markers are up from the countdown through the run and the clean-up wait until [complete].
    #[test]
    fn the_markers_show_from_the_countdown_until_complete() {
        let c = Case { stored: ABANDONED, owned: 1, killed: 1, kreq: 0, ..case(1, 2, false) };
        let mut g = Film::new(row(&c), 0., 2, 0, ABANDONED);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        assert!(!g.indicators, "none before the start");
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::ChallengeRunning {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert!(g.indicators, "up while the run goes");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::GroupAll { group: OWNED_BASE + 1 });
        let t = rt.due().unwrap();
        skate3_modes::advance(&mut g, &mut rt, t);
        assert_eq!(rt.state, St::ChallengeCleanup);
        assert!(g.indicators, "still up while the others finish");
        g.remote_done(&mut rt, 1, 30., KILLED);
        for _ in 0..50 {
            if rt.left {
                break;
            }
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert!(rt.left && !g.indicators, "[complete] takes them down");
    }

    /// The graph by hand: owning it waits 0.5 s for a kill, then cleans up and tells the others; the session's
    /// completion takes it to the reel and the results.
    #[test]
    fn owning_then_the_session_completes() {
        let c = case(1, 2, false);
        let c = Case { stored: ABANDONED, owned: 1, killed: 1, kreq: 0, ..c };
        let mut g = Film::new(row(&c), 0., 2, 0, ABANDONED);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::ChallengeRunning {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert_eq!(g.cmds.iter().filter(|c| matches!(c, Cmd::Make(_))).count(), 2);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::GroupAll { group: OWNED_BASE + 1 });
        let t = rt.due().unwrap();
        skate3_modes::advance(&mut g, &mut rt, t);
        assert_eq!(rt.state, St::ChallengeCleanup);
        assert_eq!(g.sp.sent.len(), 1, "{:?}", g.sp.sent);
        assert!(matches!(g.sp.sent[0], Relay::Done { tier: OWNED, .. }));
        assert_eq!((rt.hud.dpo.a, rt.hud.dpo.b), (1, 2));
        g.remote_done(&mut rt, 1, 30., KILLED);
        for _ in 0..50 {
            if rt.left {
                break;
            }
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert!(rt.left);
        assert_eq!(g.complete_result, OWNED);
    }

    /// Matrix Step 2 (10-12): `n` consoles play one Film (or Photo) row (an Owned and a Killed group, 45 s) through the
    /// match relay ([`skate3_modes::tests::Wire`]), started up to 0.3 s apart; skater p completes `plan(p)` (seconds
    /// into its challenge_running, killed?) on its own console; `leaver` (player, seconds) quits then, the others
    /// noticing it 0.5 s + 0.4 s per slot later. Every staying console's results rows (player, rank, ms).
    fn film_over_the_wire(n: usize, seed: u64, loss: f64, photo: bool, plan: &dyn Fn(usize) -> (f64, bool), leaver: Option<(usize, f64)>) -> Vec<Vec<(usize, i64, i64)>> {
        let c = Case { stored: ABANDONED, owned: 1, killed: 1, kreq: 0, low: None, prefilm: 0., total: 45., ..case(1, 2, photo) };
        let r = row(&c);
        let mut wire = skate3_modes::tests::Wire::new(seed, (0.15, 0.3), loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut games: Vec<Option<(Film, Rt<St>)>> = (0..n).map(|_| None).collect();
        let mut running: Vec<Option<f64>> = vec![None; n];
        let mut done = vec![false; n];
        let mut left_at = f64::INFINITY;
        let staying = |i: &usize| leaver.is_none_or(|(p, _)| p != *i);
        for frame in 0..60 * 300 {
            let t = frame as f64 / 60.;
            for i in 0..n {
                if t < starts[i] || !staying(&i) && t >= left_at {
                    continue;
                }
                let (g, rt) = games[i].get_or_insert_with(|| {
                    let mut g = Film::new(r.clone(), 0., n, i, ABANDONED);
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    skate3_modes::start(&mut g, &mut rt);
                    (g, rt)
                });
                if let Some((q, _)) = leaver
                    && t >= left_at + 0.5 + 0.4 * i as f64
                    && !rt.gone[q]
                {
                    rt.gone[q] = true;
                    skate3_modes::dispatch(g, rt, Msg::Left(q));
                }
                skate3_modes::advance(g, rt, t);
                if rt.state == St::ChallengeRunning && running[i].is_none() {
                    running[i] = Some(t);
                }
                if let Some(at) = running[i] {
                    if let Some((q, clock)) = leaver
                        && q == i
                        && t - at >= clock
                    {
                        left_at = t;
                        (0..n).for_each(|to| wire.cut(i, to));
                        continue;
                    }
                    let (clock, killed) = plan(i);
                    if !done[i] && t - at >= clock {
                        done[i] = true;
                        skate3_modes::dispatch(g, rt, Msg::GroupAll { group: if killed { KILLED_BASE } else { OWNED_BASE } + 1 });
                    }
                }
                for (from, text) in wire.take(t, i) {
                    let Relay::Done { time, tier } = serde_json::from_str(&text).unwrap();
                    g.remote_done(rt, from, time, tier);
                }
                skate3_modes::advance(g, rt, t);
                let out: Vec<String> = g.sp.sent.drain(..).map(|r| serde_json::to_string(&r).unwrap()).collect();
                wire.send(t, i, n, out);
            }
            if (0..n).filter(staying).all(|i| games[i].as_ref().is_some_and(|(_, rt)| rt.left)) {
                break;
            }
        }
        (0..n)
            .filter(staying)
            .map(|i| {
                let (g, rt) = games[i].as_ref().unwrap();
                assert!(rt.left, "seed {seed}: console {i} finished ({:?})", rt.state);
                results_order(&g.sp).into_iter().enumerate().map(|(rank, p)| (p, rank as i64 + 1, (g.sp.time[p] * 1000.).round() as i64)).collect()
            })
            .collect()
    }

    /// Matrix Step 2 (10-12): six skaters, 150-300 ms, 5-10 % loss, Film and Photo, no leaver, the host (0) or a guest
    /// (4) quitting 2 s in. By hand (the SimulPlay slots; our ponytail order, CompleteChallenge's fill UNTRACED: Killed,
    /// Owned, the rest, each by time): players 0 and 3 kill it, the rest own it, player p at 5 + p s: 0 3 1 2 4 5; a
    /// leaver never completes (Abandoned, no time): last; the session still completes on every staying console.
    #[test]
    fn six_skaters_and_a_leaver_end_film_and_photo_the_same() {
        let plan = |p: usize| (5. + p as f64, p % 3 == 0);
        for seed in 1..=6u64 {
            let photo = seed % 2 == 0;
            let leaver = match seed % 3 {
                0 => None,
                1 => Some((0, 2.)),
                _ => Some((4, 2.)),
            };
            let all = film_over_the_wire(6, seed, 0.05 + 0.01 * (seed - 1) as f64, photo, &plan, leaver);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} photo {photo}: every staying console the same");
            }
            let quit = leaver.map(|l| l.0);
            let mut want: Vec<usize> = [0, 3, 1, 2, 4, 5].into_iter().filter(|&p| Some(p) != quit).collect();
            want.extend(quit);
            assert_eq!(all[0].iter().map(|r| r.0).collect::<Vec<_>>(), want, "seed {seed} photo {photo} leaver {leaver:?}: {:?}", all[0]);
        }
    }

    #[test]
    fn relays_round_trip() {
        let r = Relay::Done { time: 12.5, tier: KILLED };
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Relay>(&text).unwrap(), r, "{text}");
    }

    /// The skater natives on edges: WentOffBoard and AirCompleted for one tick, the last air's spin kept, trick groups.
    #[test]
    fn sense_edges() {
        let mut s = Sense { groups: [("grabs".to_owned(), vec!["indy".to_owned()])].into(), ..default() };
        let ask = |s: &mut Sense, n: &str, a: &[Value]| s.native(n, a).unwrap();
        s.step(Frame { category: AIR, spin: 360., ..default() });
        assert_eq!(ask(&mut s, "ObjectiveAir.GetCurrentDegreesOfSpin", &[]), Value::Num(360.));
        s.step(Frame { category: OFFBOARD, announced: vec!["indy"], ..default() });
        assert_eq!(ask(&mut s, "ObjectiveAir.AirCompleted", &[]), Value::Bool(true));
        assert_eq!(ask(&mut s, "ObjectiveLocomotion.WentOffBoard", &[]), Value::Bool(true));
        assert_eq!(ask(&mut s, "ObjectiveAir.GetDegreesOfSpin", &[]), Value::Num(360.));
        assert_eq!(ask(&mut s, "Tricks.DidSpecificTrickGroupThisFrame", &[Value::Str("grabs".into())]), Value::Bool(true));
        s.step(Frame { category: OFFBOARD, ..default() });
        assert_eq!(ask(&mut s, "ObjectiveLocomotion.WentOffBoard", &[]), Value::Bool(false));
        assert_eq!(ask(&mut s, "ObjectiveAir.AirCompleted", &[]), Value::Bool(false));
        assert_eq!(ask(&mut s, "Stance.IsFakie", &[]), Value::Bool(false));
        s.step(Frame { category: GROUND, fakie: true, grind_guids: None, ..default() });
        assert_eq!(ask(&mut s, "Stance.IsFakie", &[]), Value::Bool(true));
        s.step(Frame { category: GRIND, grind_guids: Some([7, 8]), ..default() });
        assert_eq!(ask(&mut s, "Grind.IsGrindingGUID", &[Value::Str("8".into())]), Value::Bool(true));
        assert_eq!(ask(&mut s, "Grind.IsGrindingGUID", &[Value::Str("9".into())]), Value::Bool(false));
        assert!(s.native("ObjectiveAir.GetNumBodyFlips", &[]).is_none());
    }
}
