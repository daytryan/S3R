//! Skate 3's Follow the Leader in the career (stage 1 Task 32, spec 13.2): ftl_univ_02 "PCU Tour" and ftl_univ_03
//! "Shingo's Turn", following Giovanni Reda (`reda_ftl` -> `giovanni_reda`). The challenge graph is
//! `objectives_shared` (< objectives < default) in single player with ChallengeType Follow (0x17); [`FollowLeader`] ports
//! it handler by handler on [`Rt`] (line numbers are objectives_shared.lua's; stage1_univ.md 2-6, ai.md 5,
//! ai_runtime.md).
//!
//! The leader is FilmAIID = LocalPlayerID + 1 (setup 1351), spawned at the StartLocation (1415) and sent along its
//! paths (skate3_paths) by AI.StartPath when the countdown ends (325). The row's one Owned objective
//! (`<row>_0`) and its hint records are hand ported in [`FollowLeader::tick`] (the exported snippets, rows/<row>.json):
//! too far (`Leader.GetDistanceToNearestAISkater() >= FTLMaxFollowDistance`, fail first) fails the challenge,
//! entering FTLEndVolume wins it, and Reda's 309 / 308 / 310 play by distance every 60 evaluations. The leader reaching
//! the end volume first pins him and starts a 10 s timer that loses the challenge (552-575, 479-482).
//!
//! What the glue performs is left in pub fields: the leader's spawns and path starts ([`FollowLeader::spawns`],
//! [`FollowLeader::starts`]), his pin (`rt.pinned[leader]`), the follow indicator, the DistanceMeter
//! ([`FollowLeader::follow`], challenge::follow's FollowTheLeader state), the tutorial callout, speech as (speaker,
//! line id) pairs, the end dialog (won: 33 endchallengeoptions, lost: 36 challengefailed_small) and the
//! ChallengeCompleteResult ([`FollowLeader::result`]: Owned 1, else Abandoned 0).
use crate::{
    challenge::follow::Follow,
    skate3_modes::{Component, Graph, Handled, Msg, Place, Rt, Text, WHITE, settle},
    skate3_objectives::{ACTIVE, COMPLETE, DISABLED, FAILED},
};

/// Challenge.EActivityCompletionType (0x8283e078): Abandoned 0, Owned 1, Killed 2.
pub(crate) const ABANDONED: i64 = 0;
pub(crate) const OWNED: i64 = 1;
/// The pointer Objectives.MakeObjectivesFromGroup hands the Lua for the one Owned group (skate3_film's numbering).
const GROUP: u32 = 101;
/// The SpotVolumes index the glue sends FTLEndVolume's entries as (volume 0 is the ChallengeBoundary, none here).
pub(crate) const END_VOLUME: i64 = 1;
const START: &str = "StartLocation";
/// The hint records' speaker and InitVars (rows/<row>.json `hints`): FallBehindDistance 15, PassingDistance 3,
/// Delay 60 evaluations.
const REDA: &str = "Reda";
const FALL_BEHIND: f32 = 15.;
const PASSING: f32 = 3.;
const DELAY: u32 = 60;
/// ftl_univ_02_0_3: ShowTutorialCalloutHUD(HINT_01 "Push to get speed"), Wait(10), Hide (stage1_univ 2.3, 18.14).
const HINT: &str = "ID_CHALLENGE_TUT_SCHOOL_HINT_01";
const HINT_TIME: f64 = 10.;

const SPEECH_02: &[(&str, i64)] = &[
    ("AudioPhotographerSetupPlayerSpeech", 8572),
    ("AudioPlayerRestartChallenge", 8569),
    ("AudioPlayerFailedChallenge", 1010),
    ("AudioPlayerQuitChallenge", 1010),
    ("AudioPlayerWonChallenge", 8566),
];
const SPEECH_03: &[(&str, i64)] = &[
    ("AudioPhotographerSetupPlayerSpeech", 8575),
    ("AudioPlayerRestartChallenge", 8569),
    ("AudioPlayerFailedChallenge", 1010),
    ("AudioPlayerQuitChallenge", 1010),
    ("AudioPlayerWonChallenge", 8564),
];

/// The row fields the graph reads (stage1_univ.md 2.1-2.2, 6; the career export does not carry the L: data).
#[derive(Clone, Debug)]
pub(crate) struct Row {
    /// StartLocation, the player's and the leader's.
    pub start: &'static str,
    /// FTLMaxFollowDistance (m).
    pub max: f64,
    /// FTLEndVolume.
    pub end_volume: &'static str,
    pub countdown_delay: f64,
    pub prefilm: f64,
    /// OwnedItHUDDisplayTime.
    pub owned_time: f64,
    pub allow_bail: Option<bool>,
    pub allow_failure: Option<bool>,
    /// ResetAILinesOnPlayerUseSessionMarker / ResetObjectivesOnPlayerUseSessionMarker.
    pub reset_ai_on_marker: bool,
    pub marker_resets: Option<bool>,
    /// CentralMessageHALIDChallengeFailed ('Challenge Failed!').
    pub failed_msg: &'static str,
    /// The objective's ftl_univ_02_0_3 hint.
    pub push_hint: bool,
    /// The ChallengeSpeech calls with a line (rows/<row>.json speech.calls, all character 0x1E Reda).
    pub speech: &'static [(&'static str, i64)],
}

impl Row {
    /// ftl_univ_02 / ftl_univ_03 as retail has them. INFERRED: ResetObjectivesOnPlayerUseSessionMarker unset (not in
    /// the row's data), the failed text's id `ID_CHALLENGE_CHALLENGEFAILED` (stage1_univ 2.4 names it CHALLENGEFAILED).
    pub fn stage1(id: &str) -> Option<Row> {
        let (start, max, end_volume, push_hint, speech) = match id {
            "ftl_univ_02" => ("ftl_univ_02_startlocator_01", 100., "DIST_University|ftl_univ_02_spotvolume_01", true, SPEECH_02),
            "ftl_univ_03" => ("ftl_univ_03_startlocator_01", 50., "DIST_University|ftl_univ_03_spotvolume_01", false, SPEECH_03),
            _ => return None,
        };
        Some(Row {
            start,
            max,
            end_volume,
            countdown_delay: 1.5,
            prefilm: 0.,
            owned_time: 3.1,
            allow_bail: Some(true),
            allow_failure: Some(false),
            reset_ai_on_marker: true,
            marker_resets: None,
            failed_msg: "ID_CHALLENGE_CHALLENGEFAILED",
            push_hint,
            speech,
        })
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
    CheckAi,
    Wait,
    Countdown,
    ChallengeRunning,
    ChallengeCleanup,
    KilledIt,
    HighlightReel,
    EndOptions,
    Restarted,
    Complete,
}

/// Challenge.eChallengeFlow: Win 2, Lose 3 (a Follow row is never killed, stage1_univ 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Win,
    Lose,
}

/// The `<row>_0` objective as skate3_objectives runs one (status, SetEvaluationStatus, its `vars`): activation 0, so
/// Active from MakeObjectivesFromGroup and again at each ResetObjectives (the vars cleared).
#[derive(Clone, Copy, Debug, Default)]
struct Objective {
    status: u8,
    evaluating: bool,
    /// vars.StartFollowTrail.
    trail: bool,
    /// ObjectiveTrigger.EnteredVolume: FTLEndVolume entered while Active.
    entered: bool,
    /// The hints' vars.Timer1..3 and ftl_univ_02_0_3's AlreadyFired. INFERRED: one vars table for the objective and
    /// its hint records (_0_1 reads the Timer2 only _0_0 initialises).
    timers: [u32; 3],
    fired: bool,
}

impl Objective {
    fn made(evaluating: bool) -> Self {
        Objective { status: ACTIVE, evaluating, ..Default::default() }
    }
}

pub(crate) struct FollowLeader {
    row: Row,
    me: usize,
    /// Progression.GetChallengeCompletionType (make_objectives 38).
    stored: i64,
    objective: Option<Objective>,
    /// FollowTheLeader (SetActorIDs, Start / StopDistanceUpdates, the distance [`FollowLeader::tick`] feeds).
    pub follow: Follow,
    /// HUD2's DistanceMeter shown ("Stay within {0}", [`FollowLeader::objective_text`]).
    pub meter: bool,
    /// Leader.AddFollowIndicator(1) / RemoveFollowIndicator(1): drawn with the existing follow indicator (18.6).
    pub indicator: bool,
    /// The objective's race_final icon at `<row>_vi_01` (presentation events 66 / 67). INFERRED: shown while Active.
    pub icon: bool,
    /// ShowTutorialCalloutHUD's text and when its Wait hides it.
    pub callout: Option<(&'static str, f64)>,
    /// Speech for the glue (Task 26): (speaker, line id), oldest first.
    pub speech: Vec<(&'static str, i64)>,
    /// AI.SpawnAIAtLocation(leader, StartLocation) calls (the glue respawns him there on each), AI.StartPath calls,
    /// and whether he is spawned (AI.UnspawnAI at complete).
    pub spawns: u32,
    pub starts: u32,
    pub spawned: bool,
    /// Entity.DoesSkaterExist(leader) (check_ai), set by the glue.
    pub leader_ready: bool,
    /// end_options' OpenContinueRetryDialog(won): 33 when won, 36 when lost.
    pub won: bool,
    /// Challenge.CompleteChallenge's ChallengeCompleteResult.
    pub result: Option<i64>,
    restarted: bool,
    locked_out: bool,
    flow: Flow,
    complete_result: i64,
    completed_one: bool,
    objective_fail: bool,
    wipe_out: bool,
    countdown_delay: Option<u32>,
    countdown_timer: Option<u32>,
    owned_pending: Option<u32>,
    leader_timer: Option<u32>,
    restart_path: Option<u32>,
    session_reset: Option<u32>,
    central_delay: Option<u32>,
    replay_wait: Option<u32>,
}

impl FollowLeader {
    pub fn new(row: Row, me: usize, stored: i64) -> Self {
        FollowLeader {
            row,
            me,
            stored,
            objective: None,
            follow: Follow::default(),
            meter: false,
            indicator: false,
            icon: false,
            callout: None,
            speech: Vec::new(),
            spawns: 0,
            starts: 0,
            spawned: false,
            leader_ready: true,
            won: false,
            result: None,
            restarted: false,
            locked_out: false,
            flow: Flow::Lose,
            complete_result: ABANDONED,
            completed_one: false,
            objective_fail: false,
            wipe_out: false,
            countdown_delay: None,
            countdown_timer: None,
            owned_pending: None,
            leader_timer: None,
            restart_path: None,
            session_reset: None,
            central_delay: None,
            replay_wait: None,
        }
    }

    /// FilmAIID[1] = LocalPlayerID + 1.
    pub fn leader(&self) -> usize {
        self.me + 1
    }

    /// The graph's runtime: the leader is a player slot (pin, RepositionAI).
    pub fn runtime(&self, now: f64) -> Rt<St> {
        Rt::new(St::Initial, self.leader() + 1, now)
    }

    /// The objective HUD's line: ID_CHALLENGE_FTL_FAILURE "Stay within {0:distance.1}" with FTLMaxFollowDistance.
    pub fn objective_text(&self) -> Text {
        Text { id: "ID_CHALLENGE_FTL_FAILURE".into(), params: vec![self.row.max.to_string()] }
    }

    /// One frame of the objective group (the per-frame update 0x82c6f998, trigger byte 12, the objective's only one):
    /// `distance` is Leader.GetDistanceToNearestAISkater (3D; -1 with no leader), `inside` the player inside
    /// FTLEndVolume. Its objectivefailed / objectivecomplete + allobjectivecomplete are posted for the next advance.
    /// INFERRED: the hint records run after the objective, while it is still Active.
    pub fn tick(&mut self, rt: &mut Rt<St>, distance: f32, inside: bool) {
        if self.callout.is_some_and(|c| rt.now >= c.1) {
            self.callout = None;
        }
        if self.follow.updating {
            self.follow.distance = f64::from(distance);
        }
        let (max, push) = (self.row.max as f32, self.row.push_hint);
        let Some(o) = self.objective.as_mut().filter(|o| o.evaluating && o.status == ACTIVE) else { return };
        o.entered |= inside;
        // fail_first: the inline failure, then the objective snippet.
        if distance >= max {
            o.status = FAILED;
            rt.post(Msg::GroupFailed { group: GROUP, index: 0, failed: 0 });
            return;
        }
        if !o.trail {
            o.trail = true;
            self.indicator = true;
        }
        if o.entered {
            o.status = COMPLETE;
            self.indicator = false;
            // Its complete events: HideTutorialCalloutHUD (ftl_univ_02 only, the only one with a callout) and RemoveIcon.
            (self.callout, self.icon) = (None, false);
            rt.post(Msg::GroupObjective { group: GROUP, index: 0, points: false, accumulated: 0 });
            rt.post(Msg::GroupAll { group: GROUP });
            return;
        }
        // _0_0 / _0_1 / _0_2 (at 15 m both _0_0 and _0_1 fire, as the snippets read), then ftl_univ_02_0_3.
        for t in &mut o.timers {
            *t += 1;
        }
        let lines = [(distance >= FALL_BEHIND, 309), (distance <= FALL_BEHIND && distance > PASSING, 308), (distance <= PASSING, 310)];
        for (k, (near, line)) in lines.into_iter().enumerate() {
            if o.timers[k] >= DELAY {
                o.timers[k] = 0;
                if near {
                    self.speech.push((REDA, line));
                }
            }
        }
        if push && !o.fired {
            o.fired = true;
            self.callout = Some((HINT, rt.now + HINT_TIME));
        }
    }

    /// [global] abandonchallenge (1280-1284), the crossbar's Abandon (skate3_modes has no Msg for it).
    pub fn abandon(&mut self, rt: &mut Rt<St>) {
        self.complete_result = ABANDONED;
        self.say("AudioPlayerQuitChallenge");
        rt.change(St::Complete);
        settle(self, rt);
    }

    /// ChallengeSpeech.PlayNPCSpeechUsing(Override)Attribute(self.<field>): a row field without a line says nothing.
    fn say(&mut self, field: &str) {
        if let Some(&(_, id)) = self.row.speech.iter().find(|s| s.0 == field) {
            self.speech.push((REDA, id));
        }
    }

    fn eval(&mut self, rt: &mut Rt<St>, on: bool) {
        rt.note(format!("eval {GROUP} {on}"));
        if let Some(o) = &mut self.objective {
            o.evaluating = on;
        }
    }
    fn reset(&mut self, rt: &mut Rt<St>) {
        rt.note(format!("reset {GROUP}"));
        if let Some(o) = &mut self.objective {
            *o = Objective::made(o.evaluating);
            self.icon = true;
        }
    }
    fn fail_all(&mut self, rt: &mut Rt<St>) {
        rt.note(format!("failall {GROUP}"));
        if let Some(o) = self.objective.as_mut().filter(|o| o.status != DISABLED) {
            o.status = FAILED;
        }
    }
    fn spawn(&mut self, rt: &mut Rt<St>) {
        rt.note(format!("spawn {} {START}0", self.leader()));
        (self.spawns, self.spawned) = (self.spawns + 1, true);
    }
    fn start_path(&mut self, rt: &mut Rt<St>) {
        rt.note(format!("startpath {}", self.leader()));
        self.starts += 1;
    }
    /// The gate objectivecomplete / allobjectivecomplete share (589, 661).
    fn open(&self) -> bool {
        !self.locked_out || self.owned_pending.is_some()
    }

    /// [challenge_running] timerexpired (383-501): the FTL rows' timers. ChallengeResultSelector (82-116) is true for
    /// Follow with no side effect.
    fn running_timer(&mut self, rt: &mut Rt<St>, id: u32) {
        let id = Some(id);
        if id == self.owned_pending {
            self.owned_pending = None;
            self.say("AudioPlayerSucceedChallenge");
            rt.show(Component::OwnedItHUD, true);
            rt.change(St::ChallengeCleanup);
        } else if id == self.leader_timer {
            // "Player LOSES!": the result stays Lose unless the player had already won (OwnedPending).
            self.complete_result = ABANDONED;
            rt.change(St::ChallengeCleanup);
        } else if id == self.restart_path {
            self.start_path(rt);
        } else if id == self.session_reset {
            self.reset(rt);
            if self.row.reset_ai_on_marker {
                self.restart_path = Some(rt.add_timer(0.2));
            }
        }
    }
}

impl Graph for FollowLeader {
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
            St::CheckAi => "check_ai",
            St::Wait => "wait",
            St::Countdown => "countdown",
            St::ChallengeRunning => "challenge_running",
            St::ChallengeCleanup => "challenge_cleanup",
            St::KilledIt => "killed_it",
            St::HighlightReel => "highlight_reel",
            St::EndOptions => "end_options",
            St::Restarted => "restarted",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        let (me, leader) = (self.me, self.leader());
        // [global] restartchallenge (1270-1277).
        if matches!(msg, Msg::Restart) {
            self.locked_out = true;
            self.follow.updating = false;
            rt.note("distance off");
            self.meter = false;
            rt.note("show DistanceMeter false");
            rt.change(St::Restarted);
            return Ok(());
        }
        match (rt.state, msg) {
            (St::Initial, Msg::Enter) => rt.change(St::PinPlayer),
            // [pin_player] (10-19).
            (St::PinPlayer, Msg::Enter) => {
                rt.pin(me, true);
                rt.pause(true);
                rt.disable_use_marker(true);
                rt.change(St::MakeObjectives);
            }
            // [make_objectives] (21-134): one Owned group, no Killed It group, NumCompetitors 1 (Reda).
            (St::MakeObjectives, Msg::Enter) => {
                rt.pin(me, true);
                self.locked_out = false;
                rt.note(format!("make {GROUP}"));
                self.objective = Some(Objective::made(true));
                self.icon = true;
                self.eval(rt, false);
                rt.change(St::PrefilmDisplay);
            }
            // [prefilm_display] (136-190): HUD2.CreateGroup (with the DistanceMeter), then hudloaded.
            (St::PrefilmDisplay, Msg::Enter) => {
                self.restarted = false;
                rt.post(Msg::HudLoaded);
            }
            (St::PrefilmDisplay, Msg::HudLoaded) => {
                if (0..=2).contains(&self.stored) {
                    rt.note(format!("objhud {GROUP}"));
                }
                rt.change(St::NisOverview);
            }
            // [nis_overview] (192-203): no IntroPresentationEvents on an FTL row.
            (St::NisOverview, Msg::Enter) => rt.change(St::Setup),
            // [setup] (1330-1407).
            (St::Setup, Msg::Enter) => {
                (self.countdown_timer, self.central_delay, self.replay_wait, self.owned_pending) = (None, None, None, None);
                (self.restart_path, self.leader_timer) = (None, None);
                self.objective_fail = false;
                rt.note("loadpaths");
                self.flow = Flow::Lose;
                (self.wipe_out, self.completed_one) = (false, false);
                self.complete_result = ABANDONED;
                rt.change(St::WaitForAssetload);
            }
            // [wait_for_assetload] (212-219). ponytail: the challenge's assets are always loaded here.
            (St::WaitForAssetload, Msg::Enter) => rt.change(St::SpawnPlayer),
            // [spawn_player] (1409-1424).
            (St::SpawnPlayer, Msg::Enter) => {
                if self.restarted {
                    rt.change(St::Countdown);
                } else {
                    rt.note("enterai");
                    self.spawn(rt);
                    rt.change(St::CheckAi);
                }
            }
            // [check_ai] / [wait] (229-256).
            (St::CheckAi, Msg::Enter) => {
                if self.leader_ready {
                    rt.pin(leader, true);
                    rt.change(St::Countdown);
                } else {
                    rt.change(St::Wait);
                }
            }
            (St::Wait, Msg::Enter) => self.countdown_timer = Some(rt.add_timer(0.25)),
            (St::Wait, Msg::Timer(id)) if self.countdown_timer == Some(*id) => rt.change(St::CheckAi),
            // [countdown] stateenter (258-305). ponytail: VisualIndicators' ribbon / arrow are the follow indicator
            // (18.6).
            (St::Countdown, Msg::Enter) => {
                rt.pause(false);
                self.locked_out = false;
                rt.show(Component::ObjectiveHUD, true);
                rt.reposition_marked(me, Place { field: START, index: 0, slot: me }, 1.);
                if self.restarted {
                    rt.reposition(leader, Place { field: START, index: 0, slot: leader }, 0.);
                }
                self.follow.leader = leader as i64;
                rt.note(format!("actor {leader}"));
                self.meter = true;
                rt.note("show DistanceMeter true");
                rt.note(format!("distancehud {}", self.row.max));
                self.follow.updating = true;
                rt.note("distance on");
            }
            // [countdown] teleporttransitionfinished (340-372).
            (St::Countdown, Msg::TransitionDone) => {
                rt.show(Component::ObjectiveHUD, true);
                rt.close_loading();
                self.countdown_delay = Some(rt.add_timer(self.row.countdown_delay));
                self.say(if self.restarted { "AudioPlayerRestartChallenge" } else { "AudioPhotographerSetupPlayerSpeech" });
                self.eval(rt, true);
            }
            // [countdown] timerexpired (307-338): no film timer in single player (313).
            (St::Countdown, Msg::Timer(id)) => {
                if self.countdown_delay == Some(*id) {
                    let t = rt.add_timer(self.row.prefilm);
                    self.countdown_timer = Some(t);
                    rt.countdown(t);
                } else if self.countdown_timer == Some(*id) {
                    rt.pin(me, false);
                    rt.pin(leader, false);
                    self.start_path(rt);
                    // ActivateChallengeStartObjectives: activation 1 only; `<row>_0` is 0.
                    rt.note(format!("start {GROUP}"));
                    self.eval(rt, true);
                    rt.change(St::ChallengeRunning);
                }
            }
            // [challenge_running] stateenter (374-380).
            (St::ChallengeRunning, Msg::Enter) => rt.disable_use_marker(false),
            (St::ChallengeRunning, Msg::Timer(id)) => self.running_timer(rt, *id),
            // [challenge_running] wipeout (503-527): AllowPlayerBail true on both rows; not a film type, so at once.
            (St::ChallengeRunning, Msg::Wipeout(p)) if *p == me && !self.locked_out && self.row.allow_bail != Some(true) => {
                self.fail_all(rt);
                self.wipe_out = true;
                self.flow = Flow::Lose;
                rt.change(St::ChallengeCleanup);
            }
            // [challenge_running] triggerentercollision (552-575): the leader (competitor 1) in FTLEndVolume.
            (St::ChallengeRunning, Msg::Trigger { player, volume, inside: true }) if *player == leader && *volume == END_VOLUME => {
                rt.pin(leader, true);
                self.meter = false;
                rt.note("show DistanceMeter false");
                let t = rt.add_timer(10.);
                self.leader_timer = Some(t);
                rt.show(Component::TimerHUD, true);
                rt.timer(t);
            }
            // [challenge_running] allobjectivecomplete (586-656): the Owned group, no Killed It group to wait for.
            (St::ChallengeRunning, Msg::GroupAll { group }) if self.open() && *group == GROUP => {
                self.locked_out = true;
                rt.pause(true);
                rt.disable_use_marker(true);
                self.flow = Flow::Win;
                self.owned_pending = Some(rt.add_timer(0.5));
            }
            // [challenge_running] objectivecomplete (658-814): no film timer, no highlight (RequiresHighlight false).
            (St::ChallengeRunning, Msg::GroupObjective { group, points: false, .. }) if self.open() && *group == GROUP => {
                if self.objective.is_some_and(|o| o.status != COMPLETE) {
                    self.say("AudioPlayerCompleteAnObjective");
                }
                self.completed_one = true;
            }
            // [challenge_running] objectivefailed (886-934): the failure's HAL text is '' (no message).
            (St::ChallengeRunning, Msg::GroupFailed { group, .. }) if !self.locked_out && *group == GROUP => {
                if self.row.allow_failure == Some(false) {
                    self.fail_all(rt);
                }
                self.flow = Flow::Lose;
                self.say("AudioReinforcePlayerOnObjectiveOnFail");
                self.objective_fail = true;
                if self.row.allow_failure != Some(true) {
                    rt.change(St::ChallengeCleanup);
                }
            }
            // [challenge_running] usemarker (944-968).
            (St::ChallengeRunning, Msg::UseMarker(p)) if *p == me && !self.locked_out => {
                if self.row.reset_ai_on_marker {
                    self.spawn(rt);
                    self.restart_path = Some(rt.add_timer(1.));
                }
                if self.row.marker_resets == Some(true) {
                    self.fail_all(rt);
                    self.session_reset = Some(rt.add_timer(1.));
                }
            }
            // [challenge_cleanup] stateenter (970-1013), single player.
            (St::ChallengeCleanup, Msg::Enter) => {
                self.eval(rt, false);
                self.follow.updating = false;
                rt.note("distance off");
                self.meter = false;
                rt.note("show DistanceMeter false");
                rt.show(Component::TimerHUD, false);
                if self.flow == Flow::Lose {
                    rt.central(self.row.failed_msg, WHITE, &[], 0);
                }
                rt.disable_set_marker(true);
                rt.change(St::KilledIt);
            }
            // [killed_it] (1042-1083).
            (St::KilledIt, Msg::Enter) => {
                rt.pause(true);
                rt.pin(me, true);
                if self.flow == Flow::Lose {
                    rt.pause(true);
                    self.central_delay = Some(rt.add_timer(1.5));
                } else {
                    self.replay_wait = Some(rt.add_timer(self.row.owned_time));
                }
            }
            (St::KilledIt, Msg::Timer(id)) if self.central_delay == Some(*id) => {
                if self.flow == Flow::Win {
                    rt.show(Component::OwnedItHUD, false);
                }
                rt.change(St::EndOptions);
            }
            (St::KilledIt, Msg::Timer(id)) if self.replay_wait == Some(*id) => {
                if self.flow == Flow::Win {
                    rt.show(Component::OwnedItHUD, false);
                }
                rt.change(St::HighlightReel);
            }
            // [highlight_reel] (1097-1142): the group is empty (RequiresHighlight false), so nothing plays (18.9):
            // Rt::reel completes at once.
            (St::HighlightReel, Msg::Enter) => {
                self.say("AudioBeginReplay");
                rt.reel();
            }
            (St::HighlightReel, Msg::Reel) => rt.change(St::EndOptions),
            // [end_options] (1144-1201): Progression.LockForPresentation(true), the result's speech, the dialog.
            (St::EndOptions, Msg::Enter) => {
                rt.note("lock true");
                rt.pause(false);
                let won = self.flow == Flow::Win;
                let line = if won {
                    "AudioPlayerWonChallenge"
                } else if self.objective_fail || !self.wipe_out {
                    if self.completed_one { "AudioPlayerFailedAttemptOneObj" } else { "AudioPlayerFailedChallenge" }
                } else {
                    "AudioPlayerBailed"
                };
                self.say(line);
                self.won = won;
                rt.continue_retry();
            }
            // [end_options] hudretry (1204-1212) / hudcontinue (1215-1230).
            (St::EndOptions, Msg::HudRetry) => {
                rt.dialog = false;
                rt.pin(me, true);
                rt.change(St::Restarted);
            }
            (St::EndOptions, Msg::HudContinue) => {
                rt.dialog = false;
                self.complete_result = if self.flow == Flow::Win { OWNED } else { ABANDONED };
                if self.flow != Flow::Win {
                    self.say("AudioPlayerQuitChallenge");
                }
                rt.change(St::Complete);
            }
            // [restarted] (1232-1267).
            (St::Restarted, Msg::Enter) => {
                self.reset(rt);
                self.eval(rt, false);
                self.restarted = true;
                rt.pin(me, true);
                rt.pin(leader, true);
                rt.show(Component::ObjectiveHUD, false);
                rt.pause(true);
                rt.disable_use_marker(true);
                rt.note("unlock");
                rt.change(St::Setup);
            }
            // [complete] (1286-1328).
            (St::Complete, Msg::Enter) => {
                rt.note(format!("unspawn {leader}"));
                self.spawned = false;
                rt.pin(me, false);
                rt.note("unloadpaths");
                rt.note(format!("delete {GROUP}"));
                self.objective = None;
                (self.indicator, self.icon, self.callout, self.meter, self.follow.updating) = (false, false, None, false, false);
                // HUD2.DeleteGroup: every component down.
                (rt.hud.objective, rt.hud.timer, rt.hud.countdown, rt.hud.owned_it) = (false, None, None, None);
                rt.disable_set_marker(false);
                rt.pause(false);
                rt.note(format!("complete {}", self.complete_result));
                self.result = Some(self.complete_result);
                rt.leave();
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};
    use crate::skate3_modes::{advance, dispatch, start};
    use std::collections::BTreeMap;

    fn running(id: &str) -> (FollowLeader, Rt<St>) {
        let mut g = FollowLeader::new(Row::stage1(id).unwrap(), 0, ABANDONED);
        let mut rt = g.runtime(0.);
        start(&mut g, &mut rt);
        advance(&mut g, &mut rt, 30.);
        assert_eq!(rt.state, St::ChallengeRunning, "{id}");
        assert!(g.meter && g.follow.updating && g.starts == 1 && g.spawned && !rt.pinned[1]);
        (g, rt)
    }

    /// `Leader.GetDistanceToNearestAISkater() >= 100` (ftl_univ_02) / `>= 50` (ftl_univ_03): 'Challenge Failed!', then
    /// the lost dialog (36) and Abandoned.
    #[test]
    fn too_far_fails_at_100_and_50_m() {
        for (id, max) in [("ftl_univ_02", 100.), ("ftl_univ_03", 50.)] {
            let (mut g, mut rt) = running(id);
            let now = rt.now;
            g.tick(&mut rt, max - 0.1, false);
            advance(&mut g, &mut rt, now);
            assert_eq!(rt.state, St::ChallengeRunning, "{id}");
            g.tick(&mut rt, max, false);
            advance(&mut g, &mut rt, now);
            assert_eq!(rt.state, St::KilledIt, "{id}");
            assert!(rt.hud.central.as_ref().is_some_and(|c| c.0.text.id == g.row.failed_msg), "{id}");
            assert!(!g.meter && !g.follow.updating);
            advance(&mut g, &mut rt, now + 2.);
            assert_eq!(rt.state, St::EndOptions);
            assert!(rt.dialog && !g.won, "{id}: dialog 36");
            dispatch(&mut g, &mut rt, Msg::HudContinue);
            assert_eq!((rt.state, g.result), (St::Complete, Some(ABANDONED)));
            assert!(!g.spawned && rt.left);
        }
    }

    /// The leader in FTLEndVolume first: he is pinned, the meter goes, a 10 s TimerHUD runs, then the player loses.
    /// The player's own entry is the objective's, not this handler's.
    #[test]
    fn the_leader_first_starts_a_10_s_timer() {
        let (mut g, mut rt) = running("ftl_univ_02");
        let now = rt.now;
        dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: END_VOLUME, inside: true });
        assert!(rt.hud.timer.is_none() && g.meter);
        dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: END_VOLUME, inside: true });
        assert!(rt.pinned[1] && !g.meter);
        assert!(rt.hud.timer.is_some_and(|t| (t.1 - now - 10.).abs() < 1e-9));
        advance(&mut g, &mut rt, now + 9.9);
        assert_eq!(rt.state, St::ChallengeRunning);
        advance(&mut g, &mut rt, now + 10.);
        assert_eq!(rt.state, St::KilledIt);
        advance(&mut g, &mut rt, now + 20.);
        assert!(rt.dialog && !g.won);
        dispatch(&mut g, &mut rt, Msg::HudContinue);
        assert_eq!(g.result, Some(ABANDONED));
    }

    /// The player entering FTLEndVolume completes the objective: Owned pending 0.5 s, Owned It, no reel, the won
    /// dialog (33), Owned.
    #[test]
    fn the_end_volume_wins() {
        let (mut g, mut rt) = running("ftl_univ_03");
        let now = rt.now;
        g.tick(&mut rt, 10., false);
        assert!(g.indicator && g.icon);
        g.tick(&mut rt, 10., true);
        assert!(!g.indicator && !g.icon);
        advance(&mut g, &mut rt, now);
        assert_eq!(rt.state, St::ChallengeRunning);
        g.tick(&mut rt, 120., true);
        advance(&mut g, &mut rt, now + 0.5);
        assert_eq!(rt.state, St::KilledIt);
        assert!(rt.hud.owned_it.is_some() && rt.hud.central.is_none());
        advance(&mut g, &mut rt, now + 10.);
        assert_eq!(rt.state, St::EndOptions);
        assert!(rt.dialog && g.won, "dialog 33");
        assert!(g.speech.contains(&(REDA, 8564)));
        dispatch(&mut g, &mut rt, Msg::HudContinue);
        assert_eq!(g.result, Some(OWNED));
    }

    fn sixty(g: &mut FollowLeader, rt: &mut Rt<St>, d: f32) -> Vec<(&'static str, i64)> {
        g.speech.clear();
        for _ in 0..60 {
            g.tick(rt, d, false);
        }
        std::mem::take(&mut g.speech)
    }

    /// The hint records every 60 evaluations: 309 at 15 m or more, 308 over 3 m to 15 m, 310 at 3 m or less (at 15 m
    /// both of the first two); ftl_univ_02's "Push to get speed" once, for 10 s.
    #[test]
    fn reda_speaks_by_distance() {
        let (mut g, mut rt) = running("ftl_univ_02");
        assert_eq!(sixty(&mut g, &mut rt, 20.), [(REDA, 309)]);
        assert_eq!(g.callout.map(|c| c.0), Some(HINT));
        assert_eq!(sixty(&mut g, &mut rt, 10.), [(REDA, 308)]);
        assert_eq!(sixty(&mut g, &mut rt, 2.), [(REDA, 310)]);
        assert_eq!(sixty(&mut g, &mut rt, 15.), [(REDA, 309), (REDA, 308)]);
        rt.now += HINT_TIME;
        g.tick(&mut rt, 10., false);
        assert!(g.callout.is_none());
        let (mut g, mut rt) = running("ftl_univ_03");
        assert_eq!(sixty(&mut g, &mut rt, 40.), [(REDA, 309)]);
        assert!(g.callout.is_none());
    }

    // ------------------------------------------------------------ the oracle
    #[derive(Clone, Copy, Debug)]
    struct Case {
        row: &'static str,
        stored: i64,
        bail: Option<bool>,
        fail_ok: Option<bool>,
        ai_marker: bool,
        marker: Option<bool>,
    }

    const SETUP: &str = r#"
local meta = getmetatable(Challenge)
SELF = setmetatable({Name = "ftl", Title = "ID_T", StartLocation = "StartLocation0", ChallengeBoundary = "",
  FTLMaxFollowDistance = MAX, FTLEndVolume = "endvol", CountdownDelay = 1.5, PrefilmTime = 0, TotalFilmTime = 0,
  OwnedItHUDDisplayTime = 3.1, KilledItHUDDisplayTime = 3.3, TimeToWaitBeforeReplay = 2, OutOfBoundsTime = 5,
  AllowPlayerBail = ROW.bail, AllowObjectiveFailure = ROW.failok, AllowSessionMarkersToBeSet = false,
  ResetAILinesOnBail = true, ResetAILinesOnPlayerUseSessionMarker = ROW.aimarker,
  ResetObjectivesOnPlayerUseSessionMarker = ROW.marker, CentralMessageHALIDChallengeFailed = FAILED,
  AudioDefineChallengeWinLowTime = 10, ObjectivesOwned = {Count = 1, {PointRequirement = 0, Objectives = {{}}}},
  ObjectivesKilledIt = {Count = 0}, Competitors = {Count = 1}, VisualIndicators = {Count = 1},
  IntroPresentationEvents = {Count = 0}}, {__index = function() return nil end})
Challenge.eChallengeTypes = {Film = 7, Follow = 23, SpotPhoto = 38, AnywherePhoto = 39, SpotFilm = 40, AnywhereFilm = 41}
Challenge.GetChallengeType = function() return 23 end
Challenge.eChallengeFlow = {Exit = 0, Restart = 1, Win = 2, Lose = 3, Kill = 4}
Challenge.IsReplayingChallenge = function() return false end
Challenge.CompleteChallenge = function(_, r) log_line("complete " .. r); log_line("leave") end
SimulPlay = setmetatable({IsInSimulPlay = function() return false end}, meta)
Progression = setmetatable({GetChallengeCompletionType = function() return STORED end,
  LockForPresentation = function(b) log_line("lock " .. tostring(b)) end,
  UnlockFromPresentation = function() log_line("unlock") end}, meta)
SessionMarker = setmetatable({DisableUseSessionMarker = function() log_line("use marker disabled true") end,
  EnableUseSessionMarker = function() log_line("use marker disabled false") end,
  DisableSetSessionMarker = function() log_line("set marker disabled true") end,
  EnableSetSessionMarker = function() log_line("set marker disabled false") end}, meta)
Objectives.EObjectiveGroupType = {Owned = 0, Killed = 1}
Objectives.MakeObjectivesFromGroup = function(_, i) log_line("make " .. (100 + i)); return 100 + i end
Objectives.SetEvaluationStatus = function(g, b) log_line("eval " .. g .. " " .. tostring(b)) end
Objectives.ResetObjectives = function(g) log_line("reset " .. g) end
Objectives.FailAllObjectiveStatus = function(g) log_line("failall " .. g) end
Objectives.ActivateChallengeStartObjectives = function(g) log_line("start " .. g) end
Objectives.DeleteObjectiveGroup = function(g) log_line("delete " .. g) end
Objectives.GetAccumulatedPoints = function() return SCORE or 0 end
Objectives.GetTotalNumObjectives = function() return 1 end
Objectives.GetCurrentNumCompletedObjectives = function() return 1 end
Objectives.GetObjectiveFailureHALText = function() return "" end
Objectives.IsTrackedUniqueTrickGroup = function() return false end
HUD2.SetupObjectiveHUDData = function(_, g) log_line("objhud " .. g) end
HUD2.SetupDistanceHUDData = function(_, m) log_line(string.format("distancehud %.14g", m)) end
FollowTheLeader = setmetatable({SetActorIDs = function(id) log_line("actor " .. id) end,
  GetDistance = function() return 0 end,
  StartDistanceUpdates = function() log_line("distance on") end,
  StopDistanceUpdates = function() log_line("distance off") end}, meta)
ChallengeAsset.IsChallengeAssetLoadComplete = function() return true end
Entity.DoesSkaterExist = function() return true end
AI.LoadPaths = function() log_line("loadpaths"); return 7 end
AI.EnterAiBasedChallenge = function() log_line("enterai") end
AI.SpawnAIAtLocation = function(id, loc) log_line("spawn " .. id .. " " .. tostring(loc)) end
AI.StartPath = function(id) log_line("startpath " .. id) end
AI.UnspawnAI = function(id) log_line("unspawn " .. id) end
AI.UnloadPaths = function() log_line("unloadpaths") end
function ACTOR() return ME end
function DELIVER(kind, v, w)
  local g = math.floor(w)
  if kind == "obj" then
    SCORE = v
    SEND("objectivecomplete", {ObjectiveGroup = g, ObjectiveIndex = 0, IsPointObjective = false, RequiresHighlight = false})
    return true
  end
  if kind == "all" then SEND("allobjectivecomplete", {ObjectiveGroup = g}); return true end
  if kind == "fail" then SEND("objectivefailed", {ObjectiveGroup = g, ObjectiveIndex = 0, FailedIndex = 0}); return true end
  if kind == "leader" then
    local vol = "other"
    if g == 1 then vol = "endvol" end
    SEND("triggerentercollision", {PlayerID = v, VolumeID = vol})
    return true
  end
  if kind == "marker" then SEND("usemarker", {PlayerID = ME}); return true end
  if kind == "restart" then SEND("restartchallenge"); return true end
  if kind == "abandon" then SEND("abandonchallenge"); return true end
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

    /// Three runs (a restart starts the next): objective messages for the group and a stray one, the leader's
    /// volume entries, markers and bails, then an ending (too far, the volume, the leader first, a restart, an abandon).
    fn plan(seed: u64) -> Vec<Vec<(f64, Ev)>> {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        (0..3)
            .map(|_| {
                let mut run = Vec::new();
                let mut t = 0.2;
                for _ in 0..r.next(6) {
                    t += 0.3 + r.next(30) as f64 / 10.;
                    let g = [101., 101., 999.][r.next(3) as usize];
                    match r.next(7) {
                        0 => run.push((t, Ev::Other("obj", 100 * r.next(5) as i64, g))),
                        1 => run.push((t, Ev::Other("leader", r.next(2) as i64, (r.next(3) != 0) as i64 as f64))),
                        2 => run.push((t, Ev::Other("marker", 0, 0.))),
                        3 => run.push((t, Ev::Bail)),
                        4 => run.push((t, Ev::Other("fail", 0, g))),
                        5 => run.push((t, Ev::Other("all", 0, g))),
                        _ => {}
                    }
                }
                t += 0.5 + r.next(20) as f64 / 10.;
                match r.next(6) {
                    0 => run.push((t, Ev::Other("fail", 0, 101.))),
                    1 => {
                        run.push((t, Ev::Other("obj", 0, 101.)));
                        run.push((t, Ev::Other("all", 0, 101.)));
                    }
                    3 => run.push((t, Ev::Other("restart", 0, 0.))),
                    4 => run.push((t, Ev::Other("abandon", 0, 0.))),
                    _ => run.push((t, Ev::Other("leader", 1, 1.))),
                }
                run
            })
            .collect()
    }

    fn lua_opt(v: Option<bool>) -> String {
        v.map_or("nil".to_owned(), |b| b.to_string())
    }

    fn oracle(src: &str, c: &Case, plans: &[Vec<(f64, Ev)>], retries: &[bool]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let r = Row { allow_bail: c.bail, allow_failure: c.fail_ok, reset_ai_on_marker: c.ai_marker, marker_resets: c.marker, ..Row::stage1(c.row).unwrap() };
        let answers: Vec<&str> = retries.iter().map(|&a| if a { "\"hudretry\"" } else { "\"hudcontinue\"" }).collect();
        let setup = format!(
            "N, ME, FIRST, STORED, MAX, FAILED = 2, 0, 0, {}, {}, {:?}\nROW = {{bail = {}, failok = {}, aimarker = {}, marker = {}}}\n\
             DIALOG_ANSWERS = {{{}}}\n{SETUP}",
            c.stored,
            r.max,
            r.failed_msg,
            lua_opt(r.allow_bail),
            lua_opt(r.allow_failure),
            r.reset_ai_on_marker,
            lua_opt(r.marker_resets),
            answers.join(", ")
        );
        let (lua, errors) = lua_log(src, &setup, "initial", "challenge_running", plans);
        let mut g = FollowLeader::new(r, 0, c.stored);
        let mut rt = g.runtime(0.);
        rt.auto = Some(retries.iter().map(|&a| if a { Msg::HudRetry } else { Msg::HudContinue }).collect());
        let other = |g: &mut FollowLeader, rt: &mut Rt<St>, kind: &str, v: i64, w: f64| {
            let group = w as u32;
            Some(match kind {
                "obj" => Msg::GroupObjective { group, index: 0, points: false, accumulated: v },
                "all" => Msg::GroupAll { group },
                "fail" => Msg::GroupFailed { group, index: 0, failed: 0 },
                "leader" => Msg::Trigger { player: v as usize, volume: if w == 1. { END_VOLUME } else { 7 }, inside: true },
                "marker" => Msg::UseMarker(0),
                "restart" => Msg::Restart,
                "abandon" => {
                    g.abandon(rt);
                    return None;
                }
                _ => return None,
            })
        };
        let model = model_log(&mut g, &mut rt, St::ChallengeRunning, plans, |_| Some(0), |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// objectives_shared.lua itself (SKATE3_CAREER_DIR: scripts/graphs/) runs both FTL rows' sessions the way the
    /// model does: too far, the end volume, the leader first and its timer, markers, bails, restarts (the dialog's and
    /// the crossbar's), abandons, Owned / lost / Abandoned.
    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn skate3_follow_matches_objectives_shared_lua() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        let src = std::fs::read_to_string(root.join("scripts/graphs/objectives_shared.lua")).unwrap();
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=120u64 {
            let s = seed as usize;
            let c = Case {
                row: ["ftl_univ_02", "ftl_univ_03"][s % 2],
                stored: [ABANDONED, OWNED][(s / 2) % 2],
                bail: [Some(true), None, Some(false)][(s / 4) % 3],
                fail_ok: [Some(false), None, Some(true)][(s / 3) % 3],
                ai_marker: s % 5 != 0,
                marker: [None, Some(true)][(s / 7) % 2],
            };
            let retries: Vec<bool> = (0..3).map(|k| (s >> k) & 1 == 1).collect();
            let plans = plan(seed);
            let (lua, model, errs) = oracle(&src, &c, &plans, &retries);
            compare(&format!("seed {seed} {c:?}"), &lua, &model, &errs);
            (games, lines, errors) = (games + 1, lines + lua.len(), errors + errs.len());
            let keys = [
                "enter killed_it", "enter highlight_reel", "show OwnedItHUD true", "show TimerHUD true", "central ", "failall",
                "reset 101", "startpath 1", "move 1 ", "complete 0", "complete 1", "unlock", "distance off",
            ];
            for key in keys {
                *seen.entry(key).or_default() += lua.iter().filter(|l| l.contains(key)).count();
            }
        }
        eprintln!("covered: {seen:?}");
        assert!(seen.values().all(|&n| n > 0), "every branch exercised: {seen:?}");
        eprintln!("objectives_shared follow oracle: {games} sessions, {lines} log lines identical ({errors} Lua errors replayed)");
    }
}
