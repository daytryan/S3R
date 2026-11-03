//! Skate 3's Own the Spot online (challenge graph `ownthespot_shared_mlui` < objectives < default), the Career tab's
//! SimulPlay session: every skater plays the spot at once, as often as it likes, until the host ends the session from
//! the pause menu (ID_CROSSBAR_OTS_RETURN_TO_LOBBY). Data: tools/skate3_online.py `--modes ots`, installed at
//! `private/skate3-online/ots/`: rules.json (per row StartLocation, the OTSTriggerBoundary, the Owned / Killed It
//! objective groups as data with their snippets' trees, the spot volumes by instance id, the HUD times, the intro NIS;
//! `points_objective` beside the rows), strings.json, scripts/ownthespot_shared_mlui.lua (the graph: the rules' source
//! and the oracle), the intro camera.
//!
//! [`Ots`] is the graph handler by handler (line numbers are ownthespot_shared_mlui.lua's as the tool writes it). The
//! objectives (a sequence through the spot, its score against the target) run on [`skate3_objectives::Group`]: the
//! graph's Objectives.* calls are [`Cmd`]s the glue applies, the group's allobjectivecomplete / didnotgetenoughpoints
//! come back as [`Msg`]s. [`SimulPlay`] is the SimulPlay manager's score table (natives 0x82838bd8..0x82839000, net
//! message 49 relayed through `Match::outbox`; message 48, the host's end, likewise): no new relay kinds.
use crate::{
    challenge::cutscene::Cutscene,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate3_modes::{self, Abort, ChallengeFade, Component, Graph, Handled, HudDriver, Locator, ModeData, Msg, Place, ResultsSetup, Rt, View, Volume, GREEN, WHITE},
    skate3_objectives::{Group, GroupDef, ObjectiveDef, Out},
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Challenge.EActivityCompletionType (0x8283e078): Abandoned 0, Owned 1, Killed 2.
const ABANDONED: i64 = 0;
const OWNED: i64 = 1;
const KILLED: i64 = 2;
/// ChallengeStatus.eHighScoreTier (0x827d0808): Owned 0, Killed 1, PersonalBest 2.
const TIER_OWNED: i64 = 0;
const TIER_KILLED: i64 = 1;
const TIER_PB: i64 = 2;
/// HUD2.eDPOFormat.Ints.
const DPO_INTS: i64 = 0;
/// FrontEnd.eResultsLBType.PlaceNameStringScore (0x82853b34: 17).
const PLACE_NAME_STRING_SCORE: i64 = 17;
/// The didnotgetenoughpoints freeze (110).
const POINT_FREEZE: f64 = 0.5;
/// The host's end (msg 48, 0x82565988): 600 ticks (0x82d5dd38, at 60 a second) of
/// ID_SIMULPLAY_OTS_ENDING_SESSION, then the forced completion.
const ENDING_FOR: f64 = 10.;
const START: &str = "StartLocation";
const ENDING: &str = "ID_SIMULPLAY_OTS_ENDING_SESSION";

// ---------------------------------------------------------------- data
#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Groups {
    #[serde(rename = "ObjectivesOwned", default)]
    pub owned: Vec<Option<GroupDef>>,
    #[serde(rename = "ObjectivesKilledIt", default)]
    pub killed: Vec<Option<GroupDef>>,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub title: String,
    pub online: bool,
    pub intro: Option<String>,
    #[serde(rename = "StartLocation")]
    pub start: Option<Locator>,
    #[serde(rename = "OTSTriggerBoundary")]
    pub boundary: Option<Volume>,
    #[serde(rename = "OwnedItHUDDisplayTime")]
    pub owned_time: f64,
    #[serde(rename = "KilledItHUDDisplayTime")]
    pub killed_time: f64,
    #[serde(rename = "OutOfBoundsTime")]
    pub out_time: f64,
    #[serde(rename = "CentralMessageHALIDReturnToChallengeArea")]
    pub return_msg: String,
    #[serde(default)]
    pub groups: Groups,
    /// The volumes the snippets name (ObjectiveTrigger.EnteredVolume's decimal id), placed by the row's mission.
    #[serde(default)]
    pub trigger_volumes: BTreeMap<String, Option<Volume>>,
    /// G7: the row's VisualIndicators with their look (tools/skate3_online.py chevrons; none in an older export).
    #[serde(default)]
    pub visual_indicators: Vec<crate::skate3_race::Chevron>,
}
impl Row {
    /// self.ObjectivesOwned[1].PointRequirement / ObjectivesKilledIt[1] (0 for an empty list, as UpdateScoreTargets).
    fn requirement(groups: &[Option<GroupDef>]) -> Option<i64> {
        groups.first().map(|g| g.as_ref().and_then(|g| g.point_requirement).map_or(0, i64::from))
    }
    /// Playable: online, a start, its Owned objective group whole and ported (skate3_objectives), every volume its
    /// snippets name placed, on a known map.
    pub fn playable(&self, id: &str, points: &ObjectiveDef) -> bool {
        self.online
            && self.start.is_some()
            && self.groups.owned.first().and_then(Option::as_ref).is_some_and(|g| Group::supported(g, points, &[]).is_ok())
            && self.trigger_volumes.values().all(Option::is_some)
            && skate3_modes::map_of(id).is_some()
    }
}

// ---------------------------------------------------------------- SimulPlay
/// A SimulPlay score record (the manager's per-player entry, stride 32: +12 score, +16 level, +29 new best).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Entry {
    pub score: i64,
    pub level: i64,
}

/// The SimulPlay manager's score table as the natives read it.
#[derive(Clone, Debug, Default)]
pub(crate) struct SimulPlay {
    /// SimulPlay.IsInSimulPlay.
    pub on: bool,
    /// BeginSimulPlay's minimum best score (mgr+328).
    min: i64,
    pub entries: Vec<Entry>,
    /// The session's players (0x825655d0 skips an empty or departed slot).
    pub present: Vec<bool>,
    me: usize,
    /// UpdateSimulPlayScore's net message 49 for the other consoles.
    pub sent: Vec<Relay>,
}
impl SimulPlay {
    pub fn new(on: bool, players: usize, me: usize) -> Self {
        SimulPlay { on, min: 0, entries: vec![Entry::default(); players], present: vec![true; players], me, sent: Vec::new() }
    }
    /// 0x825655d0: the first player with the strictly highest score (above 0), if that reaches the minimum.
    fn best(&self) -> Option<usize> {
        let mut best: Option<(usize, i64)> = None;
        for (i, e) in self.entries.iter().enumerate() {
            if self.present.get(i).copied().unwrap_or(false) && e.score > best.map_or(0, |b| b.1) {
                best = Some((i, e.score));
            }
        }
        best.filter(|b| b.1 >= self.min).map(|b| b.0)
    }
    /// GetBestSimulPlayScore (0x82838e40): the best score, at least the minimum.
    pub fn best_score(&self) -> i64 {
        self.best().map_or(0, |i| self.entries[i].score).max(self.min)
    }
    /// UpdateSimulPlayScore (0x82839000): this player's entry (unless a setup score), and the message to the others.
    fn update(&mut self, rt: &mut Rt<St>, score: i64, level: i64, pre: i64, post: i64, best: bool, setup: bool) {
        rt.note(format!("simul {score} {level} {pre} {post} {best} {setup}"));
        if !setup && let Some(e) = self.entries.get_mut(self.me) {
            *e = Entry { score, level };
        }
        self.sent.push(Relay::Score { score, level, pre, post, setup });
    }
    /// Message 49 from another console (0x825657c8): its entry, unless a setup score.
    pub fn receive(&mut self, from: usize, score: i64, level: i64, setup: bool) {
        if !setup && let Some(e) = self.entries.get_mut(from) {
            *e = Entry { score, level };
        }
    }
}

// ---------------------------------------------------------------- the graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    Setup,
    Functions,
    MakeObjective,
    ViewNis,
    WaitForTrigger,
    SetupHud,
    DisplayHud,
    ChallengeSetup,
    Teleport,
    ChallengeRunning,
    ChallengeCleanup,
    HighlightReel,
    EndOptions,
    Restart,
    Complete,
}

/// Challenge.eChallengeFlow (0x8283e1d0): Exit 0, Restart 1, Win 2, Lose 3, Kill 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Exit,
    Restart,
    Win,
    Lose,
    Kill,
}

/// What the graph asks of its objective group (Objectives.*), applied by the glue in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    /// MakeObjectivesFromGroup(LocalPlayerID, 1, "ObjectivesOwned", Owned).
    Make,
    Reset,
    Evaluate(bool),
    Requirement(i64),
    Delete,
}

pub(crate) struct Ots {
    row: Row,
    intro_len: f64,
    me: usize,
    names: Vec<String>,
    /// Localize.ConvertText's strings (the two the graph formats itself).
    texts: HashMap<String, String>,
    pub sp: SimulPlay,
    /// ChallengeStatus.GetHighScore(self.Name).
    pub high: i64,
    pub cmds: Vec<Cmd>,
    // vars
    simul: bool,
    session_timer: Option<u32>,
    starting_best: i64,
    pub best: i64,
    owned: i64,
    killed: i64,
    leader: i64,
    pub session: i64,
    target: i64,
    tier: i64,
    seen_intro: bool,
    out_of_boundary: bool,
    result: Flow,
    complete_result: i64,
    simul_complete_result: i64,
    completed: bool,
    changed_stage: bool,
    locked_out: bool,
    wipe_out: bool,
    group: bool,
    hud_created: bool,
    treatment_timer: Option<u32>,
    challenge_timer: Option<u32>,
    freeze_timer: Option<u32>,
    replay_timer: Option<u32>,
    challenge_score: Option<i64>,
    objective_text: &'static str,
    target_text: String,
    /// The intro NIS while its presentation runs (the glue plays it).
    pub scene: Option<String>,
    /// G7: the row's VisualIndicators are up ([setup] AddIndicator until ChallengeCleanUp's teardown).
    pub indicators: bool,
}

impl Ots {
    pub fn new(row: Row, intro_len: f64, names: Vec<String>, me: usize, simul: bool, high: i64, texts: HashMap<String, String>) -> Self {
        let n = names.len();
        Ots {
            row,
            intro_len,
            me,
            names,
            texts,
            sp: SimulPlay::new(simul, n, me),
            high,
            cmds: Vec::new(),
            simul,
            session_timer: None,
            starting_best: 0,
            best: 0,
            owned: 0,
            killed: 0,
            leader: 0,
            session: 0,
            target: 0,
            tier: TIER_OWNED,
            seen_intro: false,
            out_of_boundary: false,
            result: Flow::Lose,
            complete_result: ABANDONED,
            simul_complete_result: ABANDONED,
            completed: false,
            changed_stage: false,
            locked_out: false,
            wipe_out: false,
            group: false,
            hud_created: false,
            treatment_timer: None,
            challenge_timer: None,
            freeze_timer: None,
            replay_timer: None,
            challenge_score: None,
            objective_text: "ID_CHALLENGE_OBJECTIVE_BEAT_PERSONALSCORE",
            target_text: "ID_COMMON_TARGET".into(),
            scene: None,
            indicators: false,
        }
    }

    fn text(&self, id: &str) -> String {
        self.texts.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }

    fn cmd(&mut self, rt: &mut Rt<St>, c: Cmd) {
        rt.note(match &c {
            Cmd::Make => "make".to_owned(),
            Cmd::Reset => "reset".into(),
            Cmd::Evaluate(b) => format!("eval {b}"),
            Cmd::Requirement(n) => format!("req {n}"),
            Cmd::Delete => "delete".into(),
        });
        self.cmds.push(c);
    }

    /// ChallengeStatus.SetHighScore(self.Name, score).
    fn set_high(&mut self, rt: &mut Rt<St>, score: i64) {
        rt.note(format!("highscore {score}"));
        self.high = score;
    }

    /// vars:UpdateScoreTargets (585-604). ponytail: Online.GetTopFriendScore reads a friends leaderboard that is
    /// not kept here: 0, the native's answer with no friend scores.
    fn update_targets(&mut self) {
        self.best = self.high;
        self.owned = Row::requirement(&self.row.groups.owned).unwrap_or(0);
        self.killed = Row::requirement(&self.row.groups.killed).unwrap_or(0);
        self.leader = self.sp.best_score();
    }

    /// vars:SetupTargetScore (605-648), the friend branch dropped with GetTopFriendScore's 0.
    fn setup_target(&mut self) {
        self.update_targets();
        self.target = self.best;
        self.tier = TIER_PB;
        self.objective_text = "ID_CHALLENGE_OBJECTIVE_BEAT_PERSONALSCORE";
        self.target_text = "ID_COMMON_TARGET".into();
        if self.owned != 0 && self.best < self.owned {
            self.target = self.owned;
            self.tier = TIER_OWNED;
            self.objective_text = "ID_CHALLENGE_OBJECTIVE_BEAT_OWNSPOT";
        } else if self.killed != 0 && self.best < self.killed {
            self.target = self.killed;
            self.tier = TIER_KILLED;
            self.objective_text = "ID_CHALLENGE_OBJECTIVE_BEAT_KILLSPOT";
        } else if self.simul {
            if self.leader != 0 {
                // GetBestSimulPlayScoreOwner (0x82838ed0): the best's gamertag, nil without one.
                let owner = self.sp.best().and_then(|i| self.names.get(i).cloned()).unwrap_or_else(|| self.text("ID_CHALLENGE_OTS_TARGET_NO_LEADER"));
                self.target_text = format!("{}: {owner}", self.text("ID_CHALLENGE_OTS_TARGET_SESSION_BEST"));
                self.target = self.leader;
                // IsBestSimulPlayScoreOwner (0x82838f98).
                self.objective_text = if self.sp.best() == Some(self.me) {
                    "ID_CHALLENGE_OBJECTIVE_BEAT_PERSONALSCORE"
                } else {
                    "ID_CHALLENGE_OBJECTIVE_BEAT_SESSION_BEST"
                };
            } else {
                self.target = self.best;
                self.objective_text = "ID_CHALLENGE_OBJECTIVE_BEAT_PERSONALSCORE";
            }
        } else {
            self.target = self.best;
            self.objective_text = "ID_CHALLENGE_OBJECTIVE_BEAT_PERSONALSCORE";
        }
    }

    /// vars:UpdateHud (649-656).
    fn update_hud(&mut self, rt: &mut Rt<St>, score: i64) {
        rt.dpo_at(0, score, self.target);
        rt.dpo_title_at(0, &self.target_text);
        rt.dpo_at(1, score, self.best);
        rt.dpo_title_at(1, "ID_DPO_PERSONALBEST");
        rt.objective_text(self.objective_text);
        rt.dpo_format(DPO_INTS);
    }

    /// vars:UpdateObjectivePointRequirement (657-659).
    fn update_requirement(&mut self, rt: &mut Rt<St>) {
        let t = self.target;
        self.cmd(rt, Cmd::Requirement(t));
    }

    /// vars:fTriggerOutOfBound (660-671), and the same lines in [challenge_running] triggerexitcollision (70-84).
    fn out_of_bound(&mut self, rt: &mut Rt<St>) {
        self.result = Flow::Restart;
        let id = self.row.return_msg.clone();
        rt.modified(&id, WHITE, &[], false);
        let t = rt.add_timer(self.row.out_time);
        self.challenge_timer = Some(t);
        rt.timer(t);
        self.update_hud(rt, 0);
        rt.show(Component::ObjectiveHUD, false);
        rt.show(Component::DPOHUD, false);
        self.cmd(rt, Cmd::Evaluate(false));
    }

    /// vars:ChallengeCleanUp (559-584): HUD2.DeleteGroup takes every component down.
    fn cleanup(&mut self, rt: &mut Rt<St>) {
        self.completed = true;
        self.indicators = false;
        if self.group {
            self.cmd(rt, Cmd::Delete);
            self.group = false;
        }
        rt.hud.dpo.visible = false;
        rt.hud.objective = false;
        rt.hud.timer = None;
        (rt.hud.owned_it, rt.hud.killed_it) = (None, None);
        self.hud_created = false;
        rt.pause(false);
        rt.disable_set_marker(false);
        if let Some(t) = self.session_timer.take() {
            rt.remove_countup(t);
        }
    }

    /// Challenge.CompleteChallenge: the challenge is over.
    fn complete(&mut self, rt: &mut Rt<St>, result: i64) {
        rt.note(format!("complete {result}"));
        rt.leave();
    }

    /// [challenge_running] allobjectivecomplete (35-58).
    fn all_complete(&mut self, rt: &mut Rt<St>, score: i64) {
        if self.wipe_out || self.locked_out {
            return;
        }
        self.locked_out = true;
        // The Kill / Win treatments take each other's display time, as the Lua has them.
        if score >= self.killed {
            self.treatment_timer = Some(rt.add_timer(self.row.owned_time));
            rt.show(Component::KilledItHUD, true);
            self.result = Flow::Kill;
        } else {
            self.treatment_timer = Some(rt.add_timer(self.row.killed_time));
            rt.show(Component::OwnedItHUD, true);
            self.result = Flow::Win;
        }
        self.update_hud(rt, score);
        self.session = score;
        self.challenge_score = Some(score);
        // ponytail: HighlightReel.AddHighlightUsingDefinition keeps no replay here (the reel is skipped in SimulPlay).
        rt.change(St::ChallengeCleanup);
    }

    /// [challenge_running] didnotgetenoughpoints (86-112).
    fn not_enough(&mut self, rt: &mut Rt<St>, score: i64) {
        if self.locked_out {
            return;
        }
        self.update_hud(rt, score);
        let mut new_best = false;
        if score > self.best {
            self.set_high(rt, score);
            self.best = score;
            new_best = true;
            rt.central("ID_LEADERBOARD_NEW_PERSONAL_BEST", GREEN, &[], 0);
        }
        let level = if score >= self.killed {
            KILLED
        } else if score >= self.owned {
            OWNED
        } else {
            ABANDONED
        };
        if self.simul && score > self.session {
            self.session = score;
            let tier = self.tier;
            self.sp.update(rt, score, level, tier, tier, new_best, false);
        }
        self.cmd(rt, Cmd::Reset);
        self.cmd(rt, Cmd::Evaluate(false));
        self.freeze_timer = Some(rt.add_timer(POINT_FREEZE));
    }

    /// [global] updatesimulplayscore (333-355).
    fn simul_score(&mut self, rt: &mut Rt<St>, player: usize, score: i64, pre: i64, post: i64, setup: bool) {
        if !self.sp.on || setup {
            return;
        }
        let name = self.names.get(player).cloned().unwrap_or_default();
        if self.leader != 0 && post == TIER_PB && score > self.leader {
            rt.central("ID_SIMULPLAY_OTS_HIGHSCORE", GREEN, &[name], 0);
        } else if pre != post {
            if pre == TIER_OWNED {
                let id = if score >= self.killed { "ID_SIMULPLAY_OTS_KILLED" } else { "ID_SIMULPLAY_OTS_OWNED" };
                rt.central(id, GREEN, &[name], 0);
            } else if pre == TIER_KILLED {
                rt.central("ID_SIMULPLAY_OTS_KILLED", GREEN, &[name], 0);
            }
        }
        if self.hud_created {
            self.setup_target();
            self.update_requirement(rt);
            self.update_hud(rt, 0);
        }
    }

    /// [end_options] stateenter (245-295).
    fn end_options(&mut self, rt: &mut Rt<St>) -> Handled {
        self.changed_stage = false;
        rt.pause(false);
        if matches!(self.result, Flow::Win | Flow::Kill) {
            let old = self.tier;
            let mut new_best = false;
            let score = self.challenge_score.ok_or(Abort)?;
            if score > self.best {
                self.set_high(rt, score);
                self.best = score;
                new_best = true;
            }
            self.setup_target();
            self.complete_result = if self.result == Flow::Win { OWNED } else { KILLED };
            if self.simul {
                let (result, tier) = (self.complete_result, self.tier);
                self.sp.update(rt, score, result, old, tier, new_best, false);
            }
            let owned_it = old == TIER_OWNED && self.tier == TIER_KILLED;
            let killed_it = (old == TIER_KILLED && self.tier == TIER_PB) || (old == TIER_OWNED && self.tier == TIER_PB);
            if owned_it || killed_it {
                self.changed_stage = true;
            }
        } else {
            self.complete_result = ABANDONED;
        }
        if self.simul && self.changed_stage {
            self.simul_complete_result = self.complete_result;
        }
        let complete = self.changed_stage && !self.simul;
        // `vars.ChallengeScore > vars.PersonalBestScore`: a Lua error while no run has been won.
        let score = self.challenge_score.ok_or(Abort)?;
        if score > self.best {
            self.set_high(rt, score);
            self.best = score;
        }
        rt.disable_set_marker(false);
        rt.change(if complete { St::Complete } else { St::Restart });
        Ok(())
    }
}

impl Graph for Ots {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::Setup => "setup",
            St::Functions => "functions",
            St::MakeObjective => "make_objective",
            St::ViewNis => "view_nis",
            St::WaitForTrigger => "wait_for_trigger",
            St::SetupHud => "setup_hud",
            St::DisplayHud => "display_hud",
            St::ChallengeSetup => "challenge_setup",
            St::Teleport => "teleport",
            St::ChallengeRunning => "challenge_running",
            St::ChallengeCleanup => "challenge_cleanup",
            St::HighlightReel => "highlight_reel",
            St::EndOptions => "end_options",
            St::Restart => "restart",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        // [global] handlers first (as the oracle's driver runs them).
        match msg {
            // simulplaycomplete (326-331).
            Msg::SimulComplete if self.sp.on => rt.change(St::Complete),
            Msg::SimulScore { player, score, pre, post, setup } => self.simul_score(rt, *player, *score, *pre, *post, *setup),
            // timerexpired (357-364).
            Msg::Timer(id) if self.treatment_timer == Some(*id) => {
                rt.show(Component::OwnedItHUD, false);
                rt.show(Component::KilledItHUD, false);
                self.treatment_timer = None;
            }
            _ => {}
        }
        let boundary = |volume: &i64| *volume == 0;
        match (rt.state, msg) {
            (St::Initial, Msg::Enter) => rt.change(St::Setup),
            // [setup] stateenter (505-555).
            (St::Setup, Msg::Enter) => {
                self.indicators = true;
                self.simul = self.sp.on;
                self.session_timer = Some(rt.add_countup());
                let min = Row::requirement(&self.row.groups.killed)
                    .filter(|_| !self.row.groups.killed.is_empty())
                    .or_else(|| Row::requirement(&self.row.groups.owned))
                    .unwrap_or(0);
                if self.simul {
                    rt.note(format!("simulbegin {min}"));
                    self.sp.min = min;
                }
                self.starting_best = self.high;
                (self.best, self.owned, self.killed, self.leader, self.session, self.target) = (0, 0, 0, 0, 0, 0);
                self.tier = TIER_OWNED;
                self.out_of_boundary = false;
                self.result = Flow::Lose;
                (self.complete_result, self.simul_complete_result) = (ABANDONED, ABANDONED);
                (self.completed, self.changed_stage, self.group, self.locked_out, self.hud_created) = (false, false, false, false, false);
                self.treatment_timer = None;
                rt.pause(true);
                rt.change(St::Functions);
            }
            (St::Functions, Msg::Enter) => rt.change(St::MakeObjective),
            // [make_objective] stateenter (10-17).
            (St::MakeObjective, Msg::Enter) => {
                self.setup_target();
                self.cmd(rt, Cmd::Make);
                self.group = true;
                self.cmd(rt, Cmd::Evaluate(false));
                self.update_requirement(rt);
                rt.change(St::ViewNis);
            }
            // [view_nis] stateenter (483-498): in SimulPlay the intro always plays. ponytail: the career discovery
            // status (Discovery.GetDiscoveryStatus) is not kept; unseen until wait_for_trigger marks it.
            (St::ViewNis, Msg::Enter) => {
                let seen = self.seen_intro && !self.sp.on;
                if self.row.intro.is_some() && !seen {
                    self.scene = self.row.intro.clone();
                    rt.present(self.intro_len);
                } else {
                    rt.change(St::WaitForTrigger);
                }
            }
            (St::ViewNis, Msg::Presented) => {
                self.scene = None;
                rt.change(St::WaitForTrigger);
            }
            // [wait_for_trigger] stateenter (375-390).
            (St::WaitForTrigger, Msg::Enter) => {
                rt.pause(true);
                self.setup_target();
                self.update_requirement(rt);
                self.result = Flow::Lose;
                self.wipe_out = false;
                self.seen_intro = true;
                rt.change(St::SetupHud);
            }
            // [setup_hud] stateenter (402-423).
            (St::SetupHud, Msg::Enter) => {
                self.changed_stage = false;
                if !self.hud_created {
                    rt.post(Msg::HudLoaded);
                } else {
                    rt.note("objhud");
                    rt.dpo_two(true);
                    rt.show(Component::TimerHUD, false);
                    rt.change(St::DisplayHud);
                }
            }
            // [setup_hud] hudloaded (425-436).
            (St::SetupHud, Msg::HudLoaded) => {
                rt.note("objhud");
                rt.dpo_two(true);
                self.hud_created = true;
                if self.out_of_boundary {
                    self.out_of_bound(rt);
                }
                rt.change(St::DisplayHud);
            }
            // [setup_hud] triggerexitcollision / triggerentercollision (438-450).
            (St::SetupHud, Msg::Trigger { player, volume, inside }) if *player == self.me && !self.simul && boundary(volume) => {
                if !*inside {
                    self.out_of_boundary = true;
                } else if self.out_of_boundary {
                    self.out_of_boundary = false;
                }
            }
            // [display_hud] stateenter (19-33).
            (St::DisplayHud, Msg::Enter) => {
                self.update_hud(rt, 0);
                if !self.out_of_boundary {
                    rt.show(Component::ObjectiveHUD, true);
                    rt.show(Component::DPOHUD, true);
                }
                if self.simul {
                    let (best, tier) = (self.best, self.tier);
                    self.sp.update(rt, best, ABANDONED, tier, tier, false, true);
                }
                rt.change(St::ChallengeSetup);
            }
            (St::ChallengeSetup, Msg::Enter) => {
                self.result = Flow::Lose;
                rt.change(St::Teleport);
            }
            // [teleport] stateenter (675-680): RepositionAI(id, StartLocation, id, true, 0). The 4th argument (0x82d550a8
            // r23): for the local player, also post cMsgSetSessionMarker 0x9A96B5F7 {Transform, FootForward, IsOnBoard =
            // !7th}, so the session marker goes to the start.
            (St::Teleport, Msg::Enter) => {
                rt.reposition_marked(self.me, Place { field: START, index: 0, slot: self.me }, 0.);
                rt.close_loading();
                rt.change(St::ChallengeRunning);
            }
            // [challenge_running] stateenter (60-67).
            (St::ChallengeRunning, Msg::Enter) => {
                rt.pause(false);
                self.cmd(rt, Cmd::Reset);
                self.cmd(rt, Cmd::Evaluate(true));
            }
            (St::ChallengeRunning, Msg::AllObjectives { player, points }) if *player == self.me => self.all_complete(rt, *points),
            (St::ChallengeRunning, Msg::NotEnoughPoints { player, points }) if *player == self.me => self.not_enough(rt, *points),
            // [challenge_running] timerexpired (114-134).
            (St::ChallengeRunning, Msg::Timer(id)) if !self.completed => {
                if self.challenge_timer == Some(*id) {
                    rt.show(Component::ObjectiveHUD, false);
                    rt.show(Component::DPOHUD, false);
                    rt.show(Component::TimerHUD, false);
                    rt.change(St::Complete);
                } else if self.freeze_timer == Some(*id) {
                    self.update_hud(rt, 0);
                    self.cmd(rt, Cmd::Evaluate(true));
                }
            }
            // [challenge_running] wipeout / wipeoutover (136-153).
            (St::ChallengeRunning, Msg::Wipeout(p)) if *p == self.me => {
                self.wipe_out = true;
                self.cmd(rt, Cmd::Evaluate(false));
            }
            (St::ChallengeRunning, Msg::WipeoutOver(p)) if *p == self.me => {
                self.wipe_out = false;
                self.cmd(rt, Cmd::Reset);
                self.cmd(rt, Cmd::Evaluate(true));
            }
            // [challenge_running] triggerexitcollision / triggerentercollision (69-84, 155-167): outside SimulPlay.
            (St::ChallengeRunning, Msg::Trigger { player, volume, inside }) if *player == self.me && !self.simul && boundary(volume) => {
                if *inside {
                    self.result = Flow::Lose;
                    self.out_of_boundary = false;
                    if let Some(t) = self.challenge_timer {
                        rt.remove_timer(t);
                    }
                    rt.show(Component::ObjectiveHUD, true);
                    rt.show(Component::DPOHUD, true);
                    self.cmd(rt, Cmd::Evaluate(true));
                } else {
                    self.out_of_boundary = true;
                    self.out_of_bound(rt);
                }
            }
            // [challenge_cleanup] stateenter (190-212).
            (St::ChallengeCleanup, Msg::Enter) => {
                self.cmd(rt, Cmd::Evaluate(false));
                rt.disable_set_marker(true);
                rt.pause(true);
                if self.simul {
                    rt.change(St::EndOptions);
                } else {
                    match self.result {
                        Flow::Restart => {
                            self.locked_out = false;
                            rt.change(St::ChallengeRunning);
                        }
                        Flow::Kill | Flow::Win => rt.change(St::HighlightReel),
                        Flow::Exit => rt.change(St::EndOptions),
                        Flow::Lose => {}
                    }
                }
            }
            // [highlight_reel] (222-243).
            (St::HighlightReel, Msg::Enter) => {
                rt.pause(true);
                match self.result {
                    Flow::Win => self.replay_timer = Some(rt.add_timer(self.row.owned_time)),
                    Flow::Kill => self.replay_timer = Some(rt.add_timer(self.row.killed_time)),
                    _ => {}
                }
            }
            (St::HighlightReel, Msg::Reel) => rt.change(St::EndOptions),
            (St::HighlightReel, Msg::Timer(id)) if self.replay_timer == Some(*id) => rt.reel(),
            (St::EndOptions, Msg::Enter) => self.end_options(rt)?,
            // [restart] stateenter (297-307).
            (St::Restart, Msg::Enter) => {
                self.cmd(rt, Cmd::Reset);
                self.update_hud(rt, 0);
                self.locked_out = false;
                rt.pause(false);
                rt.change(St::WaitForTrigger);
            }
            // [complete] stateenter (452-481).
            (St::Complete, Msg::Enter) => {
                self.cleanup(rt);
                if self.best > self.starting_best {
                    rt.note(format!("leaderboard {}", self.best));
                }
                if self.simul {
                    rt.note(format!("resultsmenu {PLACE_NAME_STRING_SCORE}"));
                    let r = self.simul_complete_result;
                    self.complete(rt, r);
                } else if self.changed_stage {
                    let r = self.complete_result;
                    self.complete(rt, r);
                } else {
                    self.complete(rt, ABANDONED);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// What one console tells the others: its SimulPlay score (net message 49), or the host's end (48).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ots")]
pub(crate) enum Relay {
    Score { score: i64, level: i64, pre: i64, post: i64, setup: bool },
    End,
}

/// The host chose End Own the Spot Session (core_menu's pause item).
#[derive(Resource, Default)]
pub(crate) struct EndSession(pub bool);

/// What the probe saw this console's skater do.
#[derive(Clone, Debug)]
enum Seen {
    Begin,
    Enter(u64),
    Boundary(bool),
    Sequence(i64, bool),
    Wipeout,
    Over,
}

#[derive(Default)]
struct Probe {
    inside: Vec<bool>,
    in_boundary: Option<bool>,
    open: bool,
    wiping: bool,
    seen: Vec<Seen>,
}

struct Play {
    id: String,
    row: Row,
    g: Ots,
    rt: Rt<St>,
    group: Option<Group>,
    owned_def: GroupDef,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    placed: u32,
    playing: Option<String>,
    /// The host's end: ID_SIMULPLAY_OTS_ENDING_SESSION is up until then.
    ending: Option<f64>,
    /// G7: the row's VisualIndicators as skate3_indicators::show items (none without the install's looks).
    markers: Vec<(String, skate3_modes::Matrix)>,
}

/// G7: Own the Spot's looks (tools/skate3_online.py look_file).
const INDICATORS: &str = "private/skate3-online/ots/indicators.json";

#[derive(Resource, Default)]
pub(crate) struct OtsGame {
    data: Option<ModeData>,
    points: Option<ObjectiveDef>,
    play: Option<Play>,
    probe: Probe,
    /// The spot volumes (instance id, box) and the OTSTriggerBoundary while the challenge runs.
    volumes: Vec<(u64, Volume)>,
    boundary: Option<Volume>,
    live: bool,
    /// ChallengeStatus's high scores by row. ponytail: Skate 3's career save is not kept here; this session's.
    high: HashMap<String, i64>,
    warned: String,
}

/// rules.json's `points_objective` (global_points_objective, 0x82c6d4d8).
fn points_objective(data: &ModeData) -> Result<ObjectiveDef, String> {
    #[derive(Deserialize)]
    struct Rules {
        points_objective: ObjectiveDef,
    }
    let text = std::fs::read_to_string(data.root.join("rules.json")).map_err(|e| e.to_string())?;
    Ok(serde_json::from_str::<Rules>(&text).map_err(|e| e.to_string())?.points_objective)
}

pub(crate) struct Skate3OtsPlugin;
impl Plugin for Skate3OtsPlugin {
    fn build(&self, app: &mut App) {
        let data = ModeData::install(app, "ots", "Own the Spot");
        let points = data.as_ref().and_then(|d| points_objective(d).inspect_err(|e| warn!("Skate 3 Own the Spot: {e}")).ok());
        if let (Some(data), Some(points)) = (&data, &points) {
            let ids: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).is_ok_and(|r| r.playable(id, points))).cloned().collect();
            // Own the Spot is the Online Challenge Map's Career tab (SimulPlay).
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                playable.0.extend(ids.iter().map(|id| crate::match_core::career(id)));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        app.insert_resource(OtsGame { data, points, ..default() })
            .init_resource::<EndSession>()
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick while the challenge runs: this skater's sequences (sequencebegin / sequencecomplete, Wipedout
/// when it ends unlanded), the spot volumes (triggerentercollision by instance id), the OTSTriggerBoundary and the
/// wipeout (wipeout / wipeoutover).
fn probe(mut game: ResMut<OtsGame>, skater: Res<SkaterRuntime>) {
    let OtsGame { probe, volumes, boundary, live, .. } = &mut *game;
    if !*live {
        *probe = Probe::default();
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let at = Vec3::new(at[0], at[1], at[2]);
    let s = &skater.scoring;
    let open = s.sequence_open();
    if open && !probe.open {
        probe.seen.push(Seen::Begin);
    }
    probe.inside.resize(volumes.len(), false);
    for (was, (id, v)) in probe.inside.iter_mut().zip(volumes.iter()) {
        let inside = v.contains(at);
        if inside && !*was {
            probe.seen.push(Seen::Enter(*id));
        }
        *was = inside;
    }
    if let Some(b) = boundary {
        let inside = b.contains(at);
        if probe.in_boundary.is_some_and(|was| was != inside) {
            probe.seen.push(Seen::Boundary(inside));
        }
        probe.in_boundary = Some(inside);
    }
    if s.sequence_landed {
        probe.seen.push(Seen::Sequence(s.lua_sequence_score(), false));
    } else if probe.open && !open {
        probe.seen.push(Seen::Sequence(s.lua_sequence_score(), true));
    }
    probe.open = open;
    let wiping = skater.wiping_out();
    if wiping && !probe.wiping {
        probe.seen.push(Seen::Wipeout);
    }
    if !wiping && probe.wiping {
        probe.seen.push(Seen::Over);
    }
    probe.wiping = wiping;
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<OtsGame>| {
        let Some(data) = game.data.take() else { return };
        step(world, &mut game, &data);
        game.data = Some(data);
    });
}

/// The graph's Objectives.* calls on the group, then the group's messages to the graph (with the points it holds).
fn apply(play: &mut Play, points: &ObjectiveDef) {
    for c in std::mem::take(&mut play.g.cmds) {
        match c {
            Cmd::Make => match Group::new(play.me, &play.owned_def, points) {
                Ok(g) => play.group = Some(g),
                Err(e) => warn!("Skate 3 Own the Spot: {}: {e}", play.id),
            },
            Cmd::Reset => play.group.iter_mut().for_each(Group::reset),
            Cmd::Evaluate(b) => play.group.iter_mut().for_each(|g| g.enabled = b),
            Cmd::Requirement(n) => play.group.iter_mut().for_each(|g| g.requirement = n.clamp(i32::MIN.into(), i32::MAX.into()) as i32), // a peer's best: saturate, not wrap
            Cmd::Delete => play.group = None,
        }
    }
}

fn post(play: &mut Play, outs: Vec<Out>) {
    let points = play.group.as_ref().map_or(0, |g| i64::from(g.accumulated()));
    for o in outs {
        match o {
            Out::AllComplete => play.rt.post(Msg::AllObjectives { player: play.me, points }),
            Out::NotEnoughPoints => play.rt.post(Msg::NotEnoughPoints { player: play.me, points }),
            _ => {}
        }
    }
}

fn dispatch(play: &mut Play, points: &ObjectiveDef, msg: Msg, now: f64) {
    skate3_modes::dispatch(&mut play.g, &mut play.rt, msg);
    pump(play, points, now);
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

fn step(world: &mut World, game: &mut OtsGame, data: &ModeData) {
    let now = f64::from(crate::match_core::match_now(world));
    let Some(points) = game.points.clone() else { return };
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && mode == Some(Mode::OwnTheSpot) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 Own the Spot: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
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
        let high = game.high.get(&id).copied().unwrap_or(0);
        game.play = Some(begin(world, data, &id, row, high, now));
    }
    let play = game.play.as_mut().expect("begun");

    if play.online {
        let m = world.resource::<Match>();
        for p in 0..play.names.len() {
            if !play.rt.gone[p] && m.players.get(p).is_none_or(String::is_empty) {
                play.rt.gone[p] = true;
                play.g.sp.present[p] = false;
            }
        }
    }
    pump(play, &points, now);

    // This console's skater: the objective group's events.
    game.live = play.group.is_some() && !play.rt.left;
    game.volumes = play.row.trigger_volumes.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()?))).collect();
    game.boundary = play.row.boundary.clone();
    let me = play.me;
    for s in std::mem::take(&mut game.probe.seen) {
        if !game.live {
            continue;
        }
        let outs = match s {
            Seen::Begin => {
                play.group.iter_mut().for_each(Group::sequence_begin);
                dispatch(play, &points, Msg::SequenceBegin(me), now);
                Vec::new()
            }
            Seen::Enter(volume) => play.group.as_mut().map_or_else(Vec::new, |g| g.trigger_enter(volume, &mut crate::skate3_objectives::NoSkater)),
            Seen::Sequence(score, wiped) => {
                play.group.as_mut().map_or_else(Vec::new, |g| g.sequence_complete(score as i32, wiped, &mut crate::skate3_objectives::NoSkater))
            }
            Seen::Wipeout => {
                dispatch(play, &points, Msg::Wipeout(me), now);
                Vec::new()
            }
            Seen::Over => {
                play.group.iter_mut().for_each(Group::wipeout_over);
                dispatch(play, &points, Msg::WipeoutOver(me), now);
                Vec::new()
            }
            Seen::Boundary(inside) => {
                dispatch(play, &points, Msg::Trigger { player: me, volume: 0, inside }, now);
                Vec::new()
            }
        };
        post(play, outs);
        pump(play, &points, now);
    }

    // SimulPlay: this console's scores out, the others' in; the host's end (net message 48: to the others, and here).
    if play.online {
        let mut out: Vec<Relay> = std::mem::take(&mut play.g.sp.sent);
        let end = std::mem::take(&mut world.resource_mut::<EndSession>().0);
        let mut ended = false;
        // Only the session's host has the pause item (core_menu: host_can_end), also a host promoted mid-session.
        if end && play.ending.is_none() && !play.rt.left {
            out.push(Relay::End);
            ended = true;
        }
        {
            let mut m = world.resource_mut::<Match>();
            m.outbox.extend(out.iter().map(|r| serde_json::to_string(r).expect("plain data")));
        }
        let host_slot = world.resource::<Match>().host_slot;
        let received: Vec<(usize, Relay)> =
            world.resource_mut::<Match>().inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect();
        for (from, r) in received {
            match r {
                Relay::Score { score, level, pre, post, setup } if from != me => {
                    play.g.sp.receive(from, score, level, setup);
                    dispatch(play, &points, Msg::SimulScore { player: from, score, pre, post, setup }, now);
                }
                // Only the host ends the session (its pause item); a guest's End is a griefer's.
                Relay::End if Some(from) == host_slot => ended = true,
                _ => {}
            }
        }
        if ended && play.ending.is_none() {
            // 0x82565988: the 600-tick timer and ID_SIMULPLAY_OTS_ENDING_SESSION. ponytail: the message's colour and
            // widget (0x826640f0) are not traced; a white central message.
            play.ending = Some(now + ENDING_FOR);
            play.rt.central(ENDING, WHITE, &[], 0);
        }
    } else {
        play.g.sp.sent.clear();
        world.resource_mut::<EndSession>().0 = false;
    }
    if play.ending.is_some_and(|t| now >= t) {
        play.ending = Some(f64::INFINITY);
        dispatch(play, &points, Msg::SimulComplete, now);
    }

    // RepositionAI(id, StartLocation, id, true, 0): this skater to the start.
    if let Some((seq, place)) = play.rt.landed(me)
        && play.placed != seq
    {
        play.placed = seq;
        if let Some(at) = play.row.start.as_ref().map(|l| l.slot(place.slot)) {
            let mut m = world.resource_mut::<Match>();
            (m.teleport, m.teleport_marks) = (Some(at), play.rt.marked == Some(seq));
        }
    }

    // The intro NIS.
    let want = play.rt.presenting.filter(|&end| now < end).and(play.g.scene.clone()).filter(|_| play.rt.state == St::ViewNis);
    if want != play.playing {
        if want.is_some() {
            world.resource_mut::<Match>().title = data.text(&play.row.title);
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
    game.high.insert(play.id.clone(), play.g.high);
    crate::skate3_indicators::show(world, INDICATORS, if play.g.indicators { play.markers.clone() } else { Vec::new() });
    {
        let mut m = world.resource_mut::<Match>();
        // ponytail: OTS never pins (no AI.PinAI in the graph); the skater waits while the loading screen is up.
        m.hold = play.rt.covered || play.rt.presenting_now();
        m.gestures = skate3_modes::gestures(&m, play.rt.left);
        m.watch = None;
        m.score = play.g.session;
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
        (m.marker_set_disabled, m.marker_use_disabled) = (play.rt.marker_set_disabled, play.rt.marker_use_disabled);
    }
    let view = View { names: &play.names, me, online: play.online, title: data.text(&play.row.title), label: "Own the Spot" };
    // The results (complete, 462-474): Rank, Skater, Status, Score over PlaceNameStringScore. ponytail: the rows are
    // the SimulPlay manager's (CompleteChallenge's native fill is not traced): each player's best, its level as
    // ID_COMMON_OWNED / ID_CELEBRATE_KILLED, ranked by score (ties by slot).
    // Not retail (user 10-12): refreshed while shown: a score another console banked in its last moments of the
    // host's end reaches this one after it completed (each console's 600 ticks start when the end reaches it).
    if play.rt.left {
        refresh_results(&mut play.rt, &play.g.sp.entries);
    }
    let results = play.rt.left.then(|| ResultsSetup {
        title: data.text(&play.row.title),
        layout: PLACE_NAME_STRING_SCORE,
        columns: ["ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_STATUS_HEADER", "ID_COMMON_SCORE"].map(|c| data.text(c)).to_vec(),
        throwdown: 0,
        choice: None,
        times: Vec::new(),
        floats: Vec::new(),
        strings: (0..play.names.len())
            .map(|p| {
                let id = match play.g.sp.entries[p].level {
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

/// The results rows from the SimulPlay entries: (player, rank, best score), ranked by score (ties by slot).
fn refresh_results(rt: &mut Rt<St>, entries: &[Entry]) {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&p| std::cmp::Reverse(entries[p].score));
    let rows: Vec<(usize, i64, i64)> = order.into_iter().enumerate().map(|(rank, p)| (p, rank as i64 + 1, entries[p].score)).collect();
    if rt.hud.results != rows {
        rt.hud.results.clear();
        for (p, rank, score) in rows {
            rt.result(p, rank, score);
        }
    }
}

fn begin(world: &mut World, data: &ModeData, id: &str, row: Row, high: i64, now: f64) -> Play {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (vec!["Player 1".to_owned()], 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let intro = row.intro.as_ref().and_then(|s| data.intros.get(s)).map_or(0., |&l| f64::from(l));
    info!("Skate 3 Own the Spot: {id} ({}), {} skaters, this one {me}", if online { "SimulPlay" } else { "solo" }, names.len());
    let texts: HashMap<String, String> = ["ID_CHALLENGE_OTS_TARGET_SESSION_BEST", "ID_CHALLENGE_OTS_TARGET_NO_LEADER"].map(|k| (k.to_owned(), data.text(k))).into();
    let owned_def = row.groups.owned.first().cloned().flatten().unwrap_or(GroupDef { point_requirement: None, objectives: Vec::new() });
    let mut g = Ots::new(row.clone(), intro, names.clone(), me, online, high, texts);
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    g.sp.present = rt.gone.iter().map(|g| !g).collect();
    skate3_modes::start(&mut g, &mut rt);
    // G7: an export from before G7 has no looks: nothing is drawn.
    let markers =
        if data.root.join("indicators.json").is_file() { crate::skate3_indicators::looks(&row.visual_indicators) } else { Vec::new() };
    Play { id: id.to_owned(), row, g, rt, group: None, owned_def, names, me, online, hud: HudDriver::default(), placed: 0, playing: None, ending: None, markers }
}

fn end(world: &mut World, game: &mut OtsGame) {
    crate::skate3_indicators::show(world, INDICATORS, Vec::new());
    if let Some(play) = game.play.take() {
        info!("Skate 3 Own the Spot: {} ended", play.id);
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

/// The host's pause item: in an Own the Spot SimulPlay session (core_menu's crossbar).
pub(crate) fn host_can_end(m: &Match, host: bool) -> bool {
    host && m.token != 0 && m.running(Mode::OwnTheSpot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};
    use crate::skate3_objectives::NoSkater;

    fn group(req: i32, volume: u64) -> GroupDef {
        GroupDef { point_requirement: Some(req), objectives: vec![Some(crate::skate3_objectives::tests::spot(volume))] }
    }

    fn row(owned: i32, killed: Option<i32>) -> Row {
        Row {
            title: "ID_T".into(),
            online: true,
            intro: Some("ots_x".into()),
            start: Some(Locator { matrix: [[0.; 4]; 4], slots: vec![] }),
            boundary: Some(Volume { min: [0.; 3], max: [1.; 3] }),
            owned_time: 3.1,
            killed_time: 3.3,
            out_time: 5.,
            return_msg: "ID_LEFTAREA".into(),
            groups: Groups { owned: vec![Some(group(owned, 7))], killed: killed.map(|k| vec![Some(group(k, 7))]).unwrap_or_default() },
            trigger_volumes: [("7".to_owned(), Some(Volume { min: [0.; 3], max: [1.; 3] }))].into(),
            visual_indicators: Vec::new(),
        }
    }

    const SETUP: &str = r#"
local function owned_group(req) return {PointRequirement = req, Objectives = {{HighlightDefinition = "hl"}}} end
local killed = {Count = 0}
if KILLED then killed = {Count = 1, owned_group(KILLED)} end
SELF = setmetatable({Name = "ots", Title = "ID_T", StartLocation = "StartLocation0", OTSTriggerBoundary = "boundary",
  OTSScoringBoundary = "scoring", OwnedItHUDDisplayTime = 3.1, KilledItHUDDisplayTime = 3.3, OutOfBoundsTime = 5,
  CentralMessageHALIDReturnToChallengeArea = "ID_LEFTAREA", ObjectivesOwned = {Count = 1, owned_group(OWNED)},
  ObjectivesKilledIt = killed, VisualIndicators = {Count = 0}, IntroPresentationEvents = {Count = 1}},
  {__index = function() return nil end})
Challenge.EActivityCompletionType = {Abandoned = 0, Owned = 1, Killed = 2}
Challenge.eChallengeFlow = {Exit = 0, Restart = 1, Win = 2, Lose = 3, Kill = 4}
Challenge.CompleteChallenge = function(_, r) log_line("complete " .. r); log_line("leave") end
SessionMarker = setmetatable({DisableUseSessionMarker = function() log_line("use marker disabled true") end,
  EnableUseSessionMarker = function() log_line("use marker disabled false") end,
  DisableSetSessionMarker = function() log_line("set marker disabled true") end,
  EnableSetSessionMarker = function() log_line("set marker disabled false") end}, getmetatable(Challenge))
ChallengeStatus.eHighScoreTier = {Owned = 0, Killed = 1, PersonalBest = 2}
ChallengeStatus.GetHighScore = function() return HIGH end
ChallengeStatus.SetHighScore = function(_, s) HIGH = s; log_line("highscore " .. s) end
Online.SetLeaderboardScore = function(_, s) log_line("leaderboard " .. s) end
Online.GetTopFriendScore = function() return 0 end
FrontEnd.eResultsLBType.PlaceNameStringScore = 17
FrontEnd.Results_SetResultsMenuType = function(t) log_line("resultsmenu " .. t) end
Localize.ConvertText = function(id) return id end
Localize.ConvertTextNoHash = function(id) return id end
Objectives.MakeObjectivesFromGroup = function() log_line("make"); return 100 + ME end
Objectives.ResetObjectives = function() log_line("reset") end
Objectives.SetEvaluationStatus = function(_, b) log_line("eval " .. tostring(b)) end
Objectives.ChangeObjectivePointsRequirement = function(_, n) log_line(string.format("req %d", n)) end
Objectives.ChangeObjectiveText = function(_, _, t) log_line("objtext " .. t) end
Objectives.DeleteObjectiveGroup = function() log_line("delete") end
Objectives.GetAccumulatedPoints = function() return SCORE end
HUD2.SetupObjectiveHUDData = function() log_line("objhud") end
HUD2.DPOUseSecondDPO = function(_, b) log_line("dpotwo " .. tostring(b)) end
HUD2.DPOSetParams = function(_, a, b, i) log_line(string.format("dpo %d %d %d", a, b, i)) end
HUD2.DPOSetTitle = function(_, t, i) log_line(string.format("dpotitle %s %d", t, i)) end
-- The SimulPlay manager's score table (0x825655d0, 0x82838e40, 0x82838ed0, 0x82838f98, 0x82839000).
ENTRIES, MIN = {}, 0
local function best()
  local idx, b = -1, 0
  for i = 0, N - 1 do if not GONE[i] and (ENTRIES[i] or 0) > b then idx, b = i, ENTRIES[i] end end
  if b < MIN then idx = -1 end
  return idx, b
end
SimulPlay = setmetatable({eSimulPlayType = {HostTerminated = 1},
  IsInSimulPlay = function() return true end,
  BeginSimulPlay = function(_, m) MIN = m; log_line("simulbegin " .. m) end,
  UpdateSimulPlayScore = function(s, l, pre, post, pb, setup)
    log_line(string.format("simul %d %d %d %d %s %s", s, l, pre, post, tostring(pb == true), tostring(setup == true)))
    if not setup then ENTRIES[ME] = s end
  end,
  GetBestSimulPlayScore = function() local i, b = best(); if i < 0 then b = 0 end; if b < MIN then b = MIN end; return b end,
  GetBestSimulPlayScoreOwner = function() local i = best(); if i < 0 then return nil end; return "P" .. i end,
  IsBestSimulPlayScoreOwner = function(id) local i = best(); return i == id end}, getmetatable(Challenge))
function ACTOR() return nil end
function DELIVER(kind, v, w)
  if kind == "all" then SCORE = v; SEND("allobjectivecomplete", {ObjectiveGroup = 100 + ME}); return true end
  if kind == "low" then SCORE = v; SEND("didnotgetenoughpoints", {PlayerID = ME, ObjectiveGroup = 100 + ME}); return true end
  if kind == "bail" then SEND("wipeout", {PlayerID = ME, IsAPhysicsTeleport = false}); return true end
  if kind == "over" then SEND("wipeoutover", {PlayerID = ME}); return true end
  if kind == "begin" then SEND("sequencebegin", {PlayerID = ME}); return true end
  if kind == "simul" then
    local p, pre, post = math.floor(w / 16), math.floor(w / 4) % 4, math.floor(w) % 4
    if not GONE[p] then
      ENTRIES[p] = v
      SEND("updatesimulplayscore", {PlayerName = "P" .. p, Score = v, PreScoreTier = pre, PostScoreTier = post, SetupScore = false})
    end
    return true
  end
  if kind == "end" then SEND("simulplaycomplete"); return true end
  return false
end
"#;

    /// One session: per run (each challenge_running) a few sequences short of the target, wipeouts, the others'
    /// scores, then a run over the target (or, last, the host's end).
    fn plans(seed: u64, n: usize, me: usize, runs: usize) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |k: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % k
        };
        (0..runs)
            .map(|r| {
                let mut run = Vec::new();
                let mut t = 0.2;
                for _ in 0..next(6) {
                    t += 0.6 + next(30) as f64 / 10.;
                    match next(5) {
                        0 => {
                            run.push((t, Ev::Other("bail", 0, 0.)));
                            run.push((t + 1.5, Ev::Other("over", 0, 0.)));
                            t += 1.6;
                        }
                        1 if n > 1 => {
                            let mut p = next(n as u64) as usize;
                            if p == me {
                                p = (p + 1) % n;
                            }
                            let (pre, post) = (next(3), next(3));
                            run.push((t, Ev::Other("simul", 200 * next(12) as i64, (p * 16) as f64 + (pre * 4 + post) as f64)));
                        }
                        _ => {
                            run.push((t, Ev::Other("begin", 0, 0.)));
                            run.push((t + 0.4, Ev::Other("low", 100 * next(25) as i64, 0.)));
                        }
                    }
                }
                t += 0.7;
                if r + 1 == runs {
                    run.push((t, Ev::Other("end", 0, 0.)));
                } else {
                    run.push((t, Ev::Other("all", 500 + 250 * next(12) as i64, 0.)));
                }
                run
            })
            .collect()
    }

    fn oracle(src: &str, n: usize, me: usize, owned: i32, killed: Option<i32>, high: i64, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let setup = format!(
            "N, ME, FIRST = {n}, {me}, 0\nOWNED, KILLED, HIGH = {owned}, {}, {high}\n{SETUP}",
            // false, not nil: the driver's _G hands an unset global back as a table.
            killed.map_or("false".to_owned(), |k| k.to_string())
        );
        let (lua, errors) = lua_log(src, &setup, "initial", "challenge_running", plans);
        let names: Vec<String> = (0..n).map(|p| format!("P{p}")).collect();
        let mut g = Ots::new(row(owned, killed), 0., names, me, true, high, HashMap::new());
        let mut rt = Rt::new(St::Initial, n, 0.);
        let other = |g: &mut Ots, _: &mut Rt<St>, kind: &str, v: i64, w: f64| match kind {
            "all" => Some(Msg::AllObjectives { player: me, points: v }),
            "low" => Some(Msg::NotEnoughPoints { player: me, points: v }),
            "bail" => Some(Msg::Wipeout(me)),
            "over" => Some(Msg::WipeoutOver(me)),
            "begin" => Some(Msg::SequenceBegin(me)),
            "simul" => {
                let (p, pre, post) = (w as usize / 16, (w as i64 / 4) % 4, w as i64 % 4);
                g.sp.receive(p, v, 0, false);
                Some(Msg::SimulScore { player: p, score: v, pre, post, setup: false })
            }
            "end" => Some(Msg::SimulComplete),
            _ => None,
        };
        let model = model_log(&mut g, &mut rt, St::ChallengeRunning, plans, |_| None, |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// ownthespot_shared_mlui.lua itself (S3_OTS_DIR = tools/skate3_online.py's `--out`/ots, with scripts/) decides
    /// every SimulPlay session the model runs the same way: states, the target and its tier (Owned, Killed, the session
    /// best), the DPOs, the objective line, the objective group's calls, personal bests, the SimulPlay score updates
    /// and the others' messages, Owned / Killed It, the host's end and the results; 1 to 6 skaters.
    #[test]
    #[ignore = "requires the decompiled ownthespot_shared_mlui.lua (S3_OTS_DIR)"]
    fn skate3_ots_matches_ownthespot_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_OTS_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/ownthespot_shared_mlui.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for seed in 1..=40u64 {
            for n in 1..=6usize {
                let s = seed as usize;
                let me = s % n;
                let (owned, killed) = [(1000, Some(1500)), (2500, Some(4000)), (800, None)][(s + n) % 3];
                let high = [0, 900, 1800, 5000][(s * 7 + n) % 4];
                let plans = plans(seed * 131 + n as u64, n, me, 1 + s % 4);
                let (lua, model, errs) = oracle(&src, n, me, owned, killed, high, &plans);
                compare(&format!("seed {seed} n {n} me {me}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.ends_with("leave")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
                for key in ["KilledItHUD true", "OwnedItHUD true", "OTS_HIGHSCORE", "OTS_OWNED", "OTS_KILLED", "NEW_PERSONAL_BEST", "SESSION_BEST", "KILLSPOT", "leaderboard", "complete 1", "complete 2"] {
                    *seen.entry(key).or_default() += lua.iter().filter(|l| l.contains(key)).count();
                }
            }
        }
        eprintln!("covered: {seen:?}");
        assert!(seen.values().all(|&n| n > 0), "every branch exercised: {seen:?}");
        eprintln!("ownthespot_shared_mlui.lua oracle: {games} sessions, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 240);
    }

    /// The objective group from the data (S3_OTS_DIR): on every playable row, a sequence through its spot volume
    /// scoring the Owned requirement completes the group (allobjectivecomplete); one short of it does not
    /// (didnotgetenoughpoints); one that never entered the spot neither.
    #[test]
    #[ignore = "requires the Skate 3 OTS data (S3_OTS_DIR)"]
    fn skate3_ots_data() {
        let root = std::path::PathBuf::from(std::env::var("S3_OTS_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let points = points_objective(&data).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id, &points)).cloned().collect();
        // 16 disc rows; with `--dlc`, the 8 DLC rows (After Dark, Maloof, Danny Way, Party Pack's Art Gallery) too.
        let dlc = ["ots_dark_01", "ots_dark_02", "ots_mmcn_01", "ots_mmcn_02", "ots_dwmc_01", "ots_dwmc_02", "ots_artg_01", "ots_artg_02"];
        let want = if data.rows.contains_key(dlc[0]) { 24 } else { 16 };
        assert_eq!(playable.len(), want, "{playable:?}");
        assert!(want == 16 || dlc.iter().all(|id| playable.iter().any(|p| p == id)), "{playable:?}");
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            assert!(r.intro.as_ref().is_some_and(|s| data.intros.contains_key(s)), "{id}");
            let def = r.groups.owned[0].clone().unwrap();
            let req = def.point_requirement.unwrap();
            let volume: u64 = r.trigger_volumes.keys().next().unwrap().parse().unwrap();
            let mut g = Group::new(0, &def, &points).unwrap();
            g.sequence_begin();
            let out = g.sequence_complete(req, false, &mut NoSkater);
            assert!(out.is_empty(), "{id}: no spot, no completion: {out:?}");
            for (score, want) in [(req - 1, Out::NotEnoughPoints), (req, Out::AllComplete)] {
                g.reset();
                g.sequence_begin();
                g.trigger_enter(volume, &mut NoSkater);
                let out = g.sequence_complete(score, false, &mut NoSkater);
                assert_eq!(out.last(), Some(&want), "{id} {score}: {out:?}");
            }
        }
        for s in [ENDING, "ID_CROSSBAR_OTS_RETURN_TO_LOBBY", "ID_SIMULPLAY_OTS_HIGHSCORE", "ID_COMMON_OWNED", "ID_CELEBRATE_KILLED"] {
            assert!(!data.text(s).starts_with("ID_"), "{s}");
        }
    }

    /// G7: the spot's markers are up from [setup] through the runs until [complete]'s clean-up.
    #[test]
    fn the_spot_markers_show_until_the_session_completes() {
        let mut g = Ots::new(row(1000, None), 0., vec!["A".into(), "B".into()], 0, true, 0, HashMap::new());
        let mut rt = Rt::new(St::Initial, 2, 0.);
        assert!(!g.indicators, "none before the start");
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::ChallengeRunning {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert!(g.indicators, "up while the runs go");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::SimulComplete);
        assert_eq!(rt.state, St::Complete);
        assert!(!g.indicators, "the clean-up takes them down");
    }

    /// By hand: a run short of the Owned target updates the personal best and the session score; a run over it is
    /// Owned It and starts the next run at the next tier; the others' scores move the session best.
    #[test]
    fn owning_moves_the_target_up() {
        let mut g = Ots::new(row(1000, Some(1500)), 0., vec!["A".into(), "B".into()], 0, true, 0, HashMap::new());
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::ChallengeRunning {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert_eq!((g.target, g.objective_text), (1000, "ID_CHALLENGE_OBJECTIVE_BEAT_OWNSPOT"));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::NotEnoughPoints { player: 0, points: 600 });
        assert_eq!((g.high, g.session, g.sp.entries[0].score), (600, 600, 600));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::AllObjectives { player: 0, points: 1200 });
        for _ in 0..50 {
            if rt.state == St::ChallengeRunning {
                break;
            }
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert_eq!(rt.state, St::ChallengeRunning);
        assert_eq!((g.high, g.target, g.objective_text), (1200, 1500, "ID_CHALLENGE_OBJECTIVE_BEAT_KILLSPOT"));
        g.sp.receive(1, 1700, KILLED, false);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::SimulScore { player: 1, score: 1700, pre: TIER_OWNED, post: TIER_PB, setup: false });
        // Over the session best (the Killed requirement while no one has beaten it) at the personal-best tier.
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with("central ID_SIMULPLAY_OTS_HIGHSCORE 2 B 0")));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::SimulComplete);
        assert!(rt.left);
    }

    /// Online audit 10-12 (Important): the results follow a score that reaches this console after it completed (the
    /// others' end window starts when the host's end reaches them, so their last scores arrive later).
    #[test]
    fn late_simulplay_scores_reach_the_results() {
        let mut rt = Rt::new(St::Complete, 2, 0.);
        let mut entries = vec![Entry { score: 1500, level: 0 }, Entry { score: 900, level: 0 }];
        refresh_results(&mut rt, &entries);
        assert_eq!(rt.hud.results, [(0, 1, 1500), (1, 2, 900)]);
        entries[1].score = 2000;
        refresh_results(&mut rt, &entries);
        assert_eq!(rt.hud.results, [(1, 1, 2000), (0, 2, 1500)], "the late best leads");
    }

    /// Matrix Step 2 (10-12): six consoles' SimulPlay tables (UpdateSimulPlayScore / message 49) through the match relay
    /// ([`skate3_modes::tests::Wire`]), 150-300 ms, 5-10 % loss; player p banks 100 (p + 1) k at 2 k + 0.1 p s (k 1-10);
    /// the host (0) or a guest (4) banks twice and quits at 7 s. By hand (the results: best desc, ties by slot): the
    /// stayers 1000 (p + 1), the leaver's 200 (q + 1) kept: host gone 5 4 3 2 1 0; guest gone 5 3 2 1 0 4 (its 1000
    /// ties player 0's: the slot first). Every staying console shows that table.
    #[test]
    fn six_consoles_end_with_the_same_own_the_spot_table() {
        let n = 6;
        for seed in 1..=6u64 {
            let q = if seed % 2 == 0 { 0 } else { 4 };
            let mut wire = skate3_modes::tests::Wire::new(seed, (0.15, 0.3), 0.05 + 0.01 * (seed - 1) as f64);
            let mut consoles: Vec<(SimulPlay, Rt<St>)> = (0..n).map(|i| (SimulPlay::new(true, n, i), Rt::new(St::Initial, n, 0.))).collect();
            let mut banked = vec![0; n];
            for frame in 0..60 * 30 {
                let t = frame as f64 / 60.;
                for (i, (sp, rt)) in consoles.iter_mut().enumerate() {
                    if i == q && t >= 7. {
                        if (7.0..7.0 + 1. / 60.).contains(&t) {
                            (0..n).for_each(|to| wire.cut(i, to));
                        }
                        continue;
                    }
                    // The leave noticed 0.5 s + 0.4 s per slot later (the glue's present = false).
                    if t >= 7.5 + 0.4 * i as f64 && sp.present[q] {
                        sp.present[q] = false;
                    }
                    let k = banked[i] + 1;
                    if k <= if i == q { 2 } else { 10 } && t >= 2. * k as f64 + 0.1 * i as f64 {
                        banked[i] = k;
                        let score = 100 * (i as i64 + 1) * k as i64;
                        sp.update(rt, score, OWNED, 0, 0, true, false);
                    }
                    for (from, text) in wire.take(t, i) {
                        if let Relay::Score { score, level, setup, .. } = serde_json::from_str(&text).unwrap() {
                            sp.receive(from, score, level, setup);
                        }
                    }
                    let out = sp.sent.drain(..).map(|r| serde_json::to_string(&r).unwrap()).collect();
                    wire.send(t, i, n, out);
                }
            }
            let tables: Vec<Vec<(usize, i64, i64)>> = (0..n)
                .filter(|&i| i != q)
                .map(|i| {
                    let (sp, rt) = &mut consoles[i];
                    refresh_results(rt, &sp.entries);
                    rt.hud.results.clone()
                })
                .collect();
            for c in &tables[1..] {
                assert_eq!(*c, tables[0], "seed {seed}: every staying console the same");
            }
            let want: &[usize] = if q == 0 { &[5, 4, 3, 2, 1, 0] } else { &[5, 3, 2, 1, 0, 4] };
            assert_eq!(tables[0].iter().map(|r| r.0).collect::<Vec<_>>(), want, "seed {seed}: {:?}", tables[0]);
            assert_eq!(tables[0].iter().find(|r| r.0 == q).map(|r| r.2), Some(200 * (q as i64 + 1)), "seed {seed}: the leaver's best kept");
        }
    }

    #[test]
    fn relays_round_trip() {
        for r in [Relay::Score { score: 5, level: 1, pre: 0, post: 2, setup: false }, Relay::End] {
            let text = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Relay>(&text).unwrap(), r, "{text}");
        }
    }
}
