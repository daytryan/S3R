//! Retail Skate 3's Skate School lessons (tut_sksc_01, 02, 06, 03, 04): challenge graph `tutorials` < default, ported
//! handler by handler from the career export's `scripts/graphs/tutorials.lua` (line numbers are its; stage1_school 1,
//! spec 12.1, 12.6). Single player, GameMode 0: one path (HUD group, Intro presentation, the reposition to the
//! StartLocation, the objectives, OwnedIt 3.5 s, dialog 33 Continue / Retry) and no fail path.
//!
//! [`Lesson`] runs on [`Rt`]. What it asks of the natives the engine does not keep in `Rt` (the objective group, the
//! HUD group, the indicators, HOM, the load screen, the coach's speech, Progression's lock) are [`Cmd`]s the career
//! glue applies in order; how it ends is [`Lesson::done`]. Two messages have no [`Msg`] variant (skate3_modes.rs is
//! shared): abandonchallenge is [`Lesson::abandon`], challengeassetloadcomplete [`Lesson::asset_loaded`]. Not tested
//! in play.
use crate::{
    skate3_career::data::Row,
    skate3_modes::{self, Component, Graph, Handled, Msg, Place, Rt},
};

/// Challenge.EActivityCompletionType (0x8283e078), as CompleteChallenge gets it. Killed (2) is never set here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Abandoned = 0,
    Owned = 1,
}

/// show_results' countdown (L171); the template's OwnedItHUDDisplayTime 3.1 is not read by this graph.
pub const OWNED_IT_FOR: f64 = 3.5;
/// The end dialog: OpenContinueRetryDialog 0x82854360 -> selector 0x825F0CE8 -> screen 33
/// hudComponents/endchallengeoptions.swf (presentation.md 1.5), titled by Results_SetMainTitle (L263).
pub const END_DIALOG: u32 = 33;
pub const END_TITLE: &str = "ID_MISSION_CHALLENGE_COMPLETE";
/// FrontEnd.SetEndChallengeOption(i, FrontEnd.eSk8BEEndMissionOptions.<name>) for i = 0, 1, 2 (L264-266).
pub const END_OPTIONS: [&str; 3] = ["Continue_Retry", "Retry_Quit", "Unknown"];
/// AudioPlayerQuitChallenge for the skateschool rows: ChallengeSpeech.PlayNPCSpeechUsingAttribute, SkateCoach
/// (character 0x1F), line 0x164A (stage1_school 1 G1).
pub const COACH: u32 = 0x1F;
pub const QUIT_LINE: u32 = 5706;
const START: &str = "StartLocation";
/// Online.GetLocalPlayerID: single player.
const ME: usize = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Initial,
    MakeHud,
    MakeObjectives,
    IntroPres,
    Setup,
    AssetLoad,
    ChallengeRunning,
    ShowResults,
    PlayPresEvent,
    EndOptions,
    Restarted,
    ChallengeCleanup,
    Complete,
}

/// The row's three presentation groups (intro_pres L231-243: Outro, Intro, Restart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pres {
    Intro,
    Restart,
    Outro,
}

/// What the graph asks of the natives `Rt` does not keep, applied by the glue in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Objectives.MakeObjectives(LocalPlayerID) (0x82c609b8).
    Make,
    /// Objectives.SetEvaluationStatus (0x82c61500).
    Evaluate(bool),
    /// Objectives.ResetObjectives: every objective disabled, its `vars` dropped (stage1_traces 6.4).
    ResetAll,
    /// Objectives.ResetObjective(group, index, true, false) (0x82c613a8).
    Reset(usize),
    /// Objectives.ActivateChallengeStartObjectives (0x82c61158).
    Start,
    /// Objectives.DeleteObjectiveGroup.
    Delete,
    /// HUD2.CreateGroup (ObjectiveHUD, TutorialInstructionHUD, OwnedItHUD; the glue answers hudloaded) / DeleteGroup.
    Hud(bool),
    /// VisualIndicators.AddIndicator / RemoveIndicator of the row's VisualIndicators[index] (1-based, as the Lua).
    Indicator { index: usize, add: bool },
    /// VisualIndicators.DeleteGroup.
    IndicatorsDelete,
    /// HOM.EnableHOMVisuals.
    Hom(bool),
    /// ChallengeAsset.ActivateChallengeAssetLoadScreen(self, ToGameplayLoadScreen) / DeactivateChallengeAssetLoadScreen.
    LoadScreen(bool),
    Speech { character: u32, id: u32 },
    /// Progression.LockForPresentation / UnlockFromPresentation.
    Lock(bool),
    /// HUD2.SetTrickAnalyzerEnabled / SetManualMeterEnabled.
    TrickAnalyzer(bool),
    ManualMeter(bool),
    /// ChallengePresentation.RemoveAllPresentationGroups.
    Unpresent,
}

impl Cmd {
    /// The oracle's line for the call.
    fn line(&self) -> String {
        match self {
            Cmd::Make => "make".into(),
            Cmd::Evaluate(b) => format!("eval {b}"),
            Cmd::ResetAll => "reset".into(),
            Cmd::Reset(i) => format!("resetone {i}"),
            Cmd::Start => "start".into(),
            Cmd::Delete => "delete".into(),
            Cmd::Hud(b) => format!("hud {b}"),
            Cmd::Indicator { index, add } => format!("indicator {} {index}", if *add { "add" } else { "remove" }),
            Cmd::IndicatorsDelete => "indicators delete".into(),
            Cmd::Hom(b) => format!("hom {b}"),
            Cmd::LoadScreen(b) => format!("loadscreen {b}"),
            Cmd::Speech { character, id } => format!("speech {character} {id}"),
            Cmd::Lock(b) => format!("lock {b}"),
            Cmd::TrickAnalyzer(b) => format!("trickanalyzer {b}"),
            Cmd::ManualMeter(b) => format!("manualmeter {b}"),
            Cmd::Unpresent => "presentations off".into(),
        }
    }
}

/// One lesson's graph.
pub struct Lesson {
    key: u64,
    /// IntroPresentationEvents / RestartPresentationEvents / OutroPresentationEvents have events (Count > 0). Outro is
    /// count 0 @tutorial and no row overrides it (stage1_school 1).
    pub intro: bool,
    pub restart: bool,
    pub outro: bool,
    /// self.VisualIndicators.Count.
    pub indicators: usize,
    /// ChallengeAsset.IsChallengeAssetLoadComplete(self). The engine has the lesson assets resident, so true unless the
    /// lesson starts through a world load (spec 12.6, 18.25); then the glue sets it false and calls
    /// [`Lesson::asset_loaded`] with the row's key once the world is in.
    pub loaded: bool,
    /// The glue plays [`Lesson::playing`] and dispatches [`Msg::Presented`] when it ends. False: each group ends at once,
    /// as the oracle's StartPresentation.
    pub driven: bool,
    pub cmds: Vec<Cmd>,
    /// The presentation group started (StartPresentation) and not finished yet.
    pub playing: Option<Pres>,
    /// Challenge.CompleteChallenge(self, result, Persistent = false): the career glue's ChallengeResult.
    pub done: Option<Outcome>,
    not_yet: bool,
    teleported: bool,
    restarted: bool,
    result: Outcome,
    wait_timer: Option<u32>,
}

impl Lesson {
    pub fn new(key: u64, intro: bool, restart: bool, indicators: usize) -> Self {
        Lesson {
            key,
            intro,
            restart,
            outro: false,
            indicators,
            loaded: true,
            driven: false,
            cmds: Vec::new(),
            playing: None,
            done: None,
            not_yet: true,
            teleported: false,
            restarted: false,
            // shortcut: vars.ChallengeCompleteResult is nil until setup; the pause menu (abandon) is off until
            // challenge_running, so nothing reads it earlier.
            result: Outcome::Abandoned,
            wait_timer: None,
        }
    }

    /// vars.kActivityID: the row's u64 key as a 32-bit float, set once when the vars table is made (0x82D057C0,
    /// stage1_teleport 1.1).
    pub fn activity_id(&self) -> f32 {
        self.key as f32
    }

    fn cmd(&mut self, rt: &mut Rt<St>, c: Cmd) {
        rt.note(c.line());
        self.cmds.push(c);
    }

    /// ChallengePresentation.StartPresentation(group).
    fn present(&mut self, rt: &mut Rt<St>, p: Pres) {
        self.playing = Some(p);
        if self.driven {
            rt.note("present");
        } else {
            rt.present(0.);
        }
    }

    /// The group that just finished was `p`'s (presentationeventgroupfinished's ChallengePresentationGroup check).
    fn finished(&mut self, p: Pres) -> bool {
        let is = self.playing == Some(p);
        if is {
            self.playing = None;
        }
        is
    }

    /// [restarted] L197-200 / L207-210.
    fn again(&mut self, rt: &mut Rt<St>) {
        (self.not_yet, self.restarted) = (true, true);
        self.cmd(rt, Cmd::Evaluate(false));
        rt.change(St::Setup);
    }

    /// [global] abandonchallenge (L138-143): the pause menu's Quit. The result stays what setup left (Abandoned).
    pub fn abandon(&mut self, rt: &mut Rt<St>) {
        self.cmd(rt, Cmd::Speech { character: COACH, id: QUIT_LINE });
        rt.change(St::ChallengeCleanup);
        skate3_modes::settle(self, rt);
    }

    /// challengeassetloadcomplete { ActivityKey } (the challenge system's preload manager, 0x82DD517C): the row's
    /// load finished. [asset_load] (L160-165) goes on when `message.ActivityKey == vars.kActivityID`. INFERRED
    /// (stage1_teleport 1.1): the u64 ActivityKey reaches Lua as a float too, so two keys can compare equal.
    pub fn asset_loaded(&mut self, rt: &mut Rt<St>, key: u64) {
        if key as f32 != self.activity_id() {
            return;
        }
        self.loaded = true;
        if rt.state == St::AssetLoad {
            self.cmd(rt, Cmd::LoadScreen(false));
            rt.change(St::ChallengeRunning);
            skate3_modes::settle(self, rt);
        }
    }
}

impl Graph for Lesson {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::MakeHud => "make_hud",
            St::MakeObjectives => "make_objectives",
            St::IntroPres => "intro_pres",
            St::Setup => "setup",
            St::AssetLoad => "asset_load",
            St::ChallengeRunning => "challenge_running",
            St::ShowResults => "show_results",
            St::PlayPresEvent => "play_pres_event",
            St::EndOptions => "end_options",
            St::Restarted => "restarted",
            St::ChallengeCleanup => "challenge_cleanup",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        match (rt.state, msg) {
            // [global] restartchallenge (L146-148): the pause menu's Restart.
            (_, Msg::Restart) => rt.change(St::Restarted),
            (St::Initial, Msg::Enter) => rt.change(St::MakeHud),
            // [make_hud] stateenter (L21-30). VisualIndicators.CreateGroup: the glue's group is made on first use.
            (St::MakeHud, Msg::Enter) => {
                rt.pause(true);
                self.cmd(rt, Cmd::Hud(true));
                rt.post(Msg::HudLoaded);
            }
            // [make_hud] hudloaded (L33-37): the group is this graph's own.
            (St::MakeHud, Msg::HudLoaded) => rt.change(St::MakeObjectives),
            // [make_objectives] stateenter (L10-18). SetupObjectiveHUDData / SetActiveGroup: the objective HUD shows
            // this group (the glue's).
            (St::MakeObjectives, Msg::Enter) => {
                self.cmd(rt, Cmd::Make);
                self.cmd(rt, Cmd::Evaluate(false));
                self.restarted = false;
                rt.change(St::IntroPres);
            }
            // [intro_pres] (L231-256): the three groups are the row's; the glue builds them.
            (St::IntroPres, Msg::Enter) => {
                if self.intro {
                    self.present(rt, Pres::Intro);
                } else {
                    rt.change(St::Setup);
                }
            }
            (St::IntroPres, Msg::Presented) if self.finished(Pres::Intro) => rt.change(St::Setup),
            // [setup] stateenter (L40-47). vars.ShowObjectiveHUD is always true.
            (St::Setup, Msg::Enter) => {
                self.cmd(rt, Cmd::Hom(false));
                (self.not_yet, self.teleported, self.result) = (true, false, Outcome::Abandoned);
                rt.change(St::AssetLoad);
            }
            // [asset_load] stateenter (L151-157).
            (St::AssetLoad, Msg::Enter) => {
                if self.loaded {
                    rt.change(St::ChallengeRunning);
                } else {
                    self.cmd(rt, Cmd::LoadScreen(true));
                }
            }
            // [challenge_running] stateenter (L50-74). IgnoreInitialTutorialReposition is false @tutorial and no row
            // overrides it, so L56 always holds: every start and restart goes through AI.RepositionAI(id,
            // StartLocation, 0, false, 1, false, SpawnOffboard) (0x8284a950) and waits for its transition; the
            // L60-72 branch (no teleport, FadeIn 0.25) is not ported. SpawnOffboard is the row's `offboard` (glue).
            // Challenge.NotifyChallengeRunning: the glue sees the state.
            (St::ChallengeRunning, Msg::Enter) => {
                rt.pause(false);
                for index in 1..=self.indicators {
                    self.cmd(rt, Cmd::Indicator { index, add: false });
                    self.cmd(rt, Cmd::Indicator { index, add: true });
                }
                rt.reposition(ME, Place { field: START, index: 0, slot: 0 }, 1.);
                self.teleported = true;
            }
            // [challenge_running] teleporttransitionfinished (L95-110).
            (St::ChallengeRunning, Msg::TransitionDone) if self.teleported => {
                rt.close_loading();
                self.teleported = false;
                rt.show(Component::ObjectiveHUD, true);
                if self.restarted {
                    self.cmd(rt, Cmd::ResetAll);
                }
                self.cmd(rt, Cmd::Start);
                self.cmd(rt, Cmd::Evaluate(true));
                self.not_yet = false;
            }
            // [challenge_running] objectivefailed (L83-92). No lesson posts it (every failure record is a hint, spec
            // 12.1); ported as written.
            (St::ChallengeRunning, Msg::GroupFailed { index, .. }) if !self.not_yet => self.cmd(rt, Cmd::Reset(*index)),
            // [challenge_running] allobjectivecomplete (L77-80), posted by 0x82c6f7d8 when every objective is Complete.
            (St::ChallengeRunning, Msg::GroupAll { .. }) => {
                self.result = Outcome::Owned;
                rt.change(St::ShowResults);
            }
            // [show_results] (L168-189); its hudcontinue / hudretry are empty.
            (St::ShowResults, Msg::Enter) => {
                rt.pause(true);
                rt.show(Component::OwnedItHUD, true);
                self.wait_timer = Some(rt.add_timer(OWNED_IT_FOR));
            }
            (St::ShowResults, Msg::Timer(id)) if self.wait_timer == Some(*id) => {
                rt.show(Component::OwnedItHUD, false);
                rt.pause(false);
                rt.change(St::PlayPresEvent);
            }
            // [play_pres_event] (L215-228).
            (St::PlayPresEvent, Msg::Enter) => {
                if self.outro {
                    self.present(rt, Pres::Outro);
                } else {
                    rt.change(St::EndOptions);
                }
            }
            (St::PlayPresEvent, Msg::Presented) if self.finished(Pres::Outro) => rt.change(St::EndOptions),
            // [end_options] stateenter (L259-269): dialog 33 (the glue draws it from rt.dialog and the consts).
            (St::EndOptions, Msg::Enter) => {
                self.cmd(rt, Cmd::Lock(true));
                rt.note(format!("title {END_TITLE}"));
                for (i, option) in END_OPTIONS.iter().enumerate() {
                    rt.note(format!("option {i} {option}"));
                }
                rt.note("players 0");
                rt.continue_retry();
            }
            // [end_options] hudretry (L272-275).
            (St::EndOptions, Msg::HudRetry) => {
                rt.dialog = false;
                self.cmd(rt, Cmd::Lock(false));
                rt.change(St::Restarted);
            }
            // [end_options] hudcontinue (L278-280): no UnlockFromPresentation here (stage1_school OQ5; see complete).
            (St::EndOptions, Msg::HudContinue) => {
                rt.dialog = false;
                rt.change(St::ChallengeCleanup);
            }
            // [restarted] (L192-212).
            (St::Restarted, Msg::Enter) => {
                rt.open_loading();
                if self.restart {
                    self.present(rt, Pres::Restart);
                } else {
                    self.again(rt);
                }
            }
            (St::Restarted, Msg::Presented) if self.finished(Pres::Restart) => self.again(rt),
            // [challenge_cleanup] stateenter (L113-115).
            (St::ChallengeCleanup, Msg::Enter) => rt.change(St::Complete),
            // [complete] stateenter (L118-135).
            (St::Complete, Msg::Enter) => {
                rt.pause(false);
                self.cmd(rt, Cmd::Hud(false));
                (rt.hud.objective, rt.hud.owned_it) = (false, None);
                self.cmd(rt, Cmd::TrickAnalyzer(false));
                self.cmd(rt, Cmd::ManualMeter(false));
                for index in 1..=self.indicators {
                    self.cmd(rt, Cmd::Indicator { index, add: false });
                }
                self.cmd(rt, Cmd::IndicatorsDelete);
                self.cmd(rt, Cmd::Hom(true));
                self.cmd(rt, Cmd::Unpresent);
                self.playing = None;
                self.cmd(rt, Cmd::Delete);
                // Persistent is false @tutorial.
                rt.note(format!("complete {} false", self.result as i64));
                self.done = Some(self.result);
                // INFERRED (spec 18.21): CompleteChallenge releases the presentation lock, as the challenge result
                // does (career_flow Q2 step 4). Not a Lua call, so not logged.
                self.cmds.push(Cmd::Lock(false));
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- which port runs a row
/// The stage 1 ports a career row starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    Lesson,
    Teleport,
    Follow,
    Film,
    Photo,
}

/// The port for a row, by its challenge graph: `objectives_shared` is Film for Challenge.eChallengeTypes.Film (7) and
/// Follow for the rest (ftl_univ_02's kind is 23). None: no port yet.
pub fn port_of(row: &Row) -> Option<Port> {
    match row.graph.as_str() {
        "tutorials" => Some(Port::Lesson),
        "teleport" => Some(Port::Teleport),
        "photo_skate2" => Some(Port::Photo),
        "objectives_shared" if i64::from(row.kind) == crate::skate3_film::FILM => Some(Port::Film),
        "objectives_shared" => Some(Port::Follow),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::{
        advance, dispatch, start,
        tests::{Ev, compare, lua_log, model_log},
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// tut_sksc_01's key (career.json).
    const KEY: u64 = 6_639_973_523_196_558_502;
    /// The objective group pointer the messages carry (the graph does not read it).
    const GROUP: u32 = 100;
    const LESSONS: [&str; 5] = ["tut_sksc_01", "tut_sksc_02", "tut_sksc_06", "tut_sksc_03", "tut_sksc_04"];

    /// A lesson started with nothing to wait for: in challenge_running, its teleport fading (it lands at 2 s).
    fn begin() -> (Lesson, Rt<St>) {
        let mut g = Lesson::new(KEY, false, false, 0);
        let mut rt = Rt::new(St::Initial, 1, 0.);
        rt.log = Some(Vec::new());
        start(&mut g, &mut rt);
        advance(&mut g, &mut rt, 0.);
        (g, rt)
    }

    /// A lesson won at 3 s and through OwnedIt: at the end dialog.
    fn won() -> (Lesson, Rt<St>) {
        let (mut g, mut rt) = begin();
        advance(&mut g, &mut rt, 3.);
        dispatch(&mut g, &mut rt, Msg::GroupAll { group: GROUP });
        advance(&mut g, &mut rt, 7.);
        (g, rt)
    }

    /// The log without its times.
    fn lines(rt: &Rt<St>) -> Vec<&str> {
        rt.log.as_ref().unwrap().iter().map(|l| l.split_once(' ').unwrap().1).collect()
    }

    #[test]
    fn the_lesson_always_repositions_and_waits_for_the_teleport() {
        let (mut g, mut rt) = begin();
        assert_eq!(rt.state, St::ChallengeRunning);
        assert_eq!(rt.places[0].map(|p| p.1), Some(Place { field: START, index: 0, slot: 0 }));
        assert!(!g.cmds.contains(&Cmd::Start), "the objectives wait for the teleport");
        dispatch(&mut g, &mut rt, Msg::GroupFailed { group: GROUP, index: 1, failed: 0 });
        assert!(!g.cmds.contains(&Cmd::Reset(1)), "NotYet: a failure before the teleport lands is ignored");
        advance(&mut g, &mut rt, 2.);
        assert!(!rt.covered);
        assert!(rt.hud.objective);
        assert_eq!(g.cmds[g.cmds.len() - 2..], [Cmd::Start, Cmd::Evaluate(true)]);
        // A restart teleports again; ResetObjectives comes before the start objectives.
        g.cmds.clear();
        dispatch(&mut g, &mut rt, Msg::Restart);
        assert_eq!(rt.state, St::ChallengeRunning);
        assert!(rt.covered);
        assert!(!g.cmds.contains(&Cmd::Start));
        advance(&mut g, &mut rt, 10.);
        let order: Vec<Cmd> = g.cmds.iter().filter(|c| matches!(c, Cmd::ResetAll | Cmd::Start)).cloned().collect();
        assert_eq!(order, [Cmd::ResetAll, Cmd::Start]);
        assert_eq!(lines(&rt).iter().filter(|l| l.starts_with("move 0 StartLocation0 0 1")).count(), 2);
    }

    #[test]
    fn owned_it_shows_for_3_5_seconds() {
        let (mut g, mut rt) = begin();
        advance(&mut g, &mut rt, 3.);
        dispatch(&mut g, &mut rt, Msg::GroupAll { group: GROUP });
        assert_eq!((rt.state, rt.hud.owned_it), (St::ShowResults, Some(3.)));
        assert!(rt.pause_disabled);
        advance(&mut g, &mut rt, 6.49);
        assert_eq!(rt.state, St::ShowResults);
        advance(&mut g, &mut rt, 6.5);
        assert_eq!((rt.state, rt.hud.owned_it, rt.pause_disabled), (St::EndOptions, None, false));
        assert!(rt.dialog);
    }

    #[test]
    fn the_end_dialog_has_retails_options() {
        assert_eq!((END_DIALOG, END_TITLE), (33, "ID_MISSION_CHALLENGE_COMPLETE"));
        assert_eq!(END_OPTIONS, ["Continue_Retry", "Retry_Quit", "Unknown"]);
        let (mut g, mut rt) = won();
        let log = lines(&rt);
        let at = log.iter().position(|l| *l == "lock true").expect("LockForPresentation");
        assert_eq!(
            log[at + 1..],
            ["title ID_MISSION_CHALLENGE_COMPLETE", "option 0 Continue_Retry", "option 1 Retry_Quit", "option 2 Unknown", "players 0", "dialog"]
        );
        dispatch(&mut g, &mut rt, Msg::HudContinue);
        assert_eq!((rt.state, g.done), (St::Complete, Some(Outcome::Owned)));
        assert_eq!(g.cmds.last(), Some(&Cmd::Lock(false)), "CompleteChallenge releases the lock");
        // Retry unlocks and plays again.
        let (mut g, mut rt) = won();
        dispatch(&mut g, &mut rt, Msg::HudRetry);
        assert_eq!((rt.state, g.done), (St::ChallengeRunning, None));
        assert!(g.cmds.contains(&Cmd::Lock(false)));
    }

    #[test]
    fn a_lesson_has_no_fail_path() {
        let (mut g, mut rt) = begin();
        advance(&mut g, &mut rt, 3.);
        for index in 0..5 {
            dispatch(&mut g, &mut rt, Msg::GroupFailed { group: GROUP, index, failed: 0 });
        }
        dispatch(&mut g, &mut rt, Msg::Wipeout(ME));
        advance(&mut g, &mut rt, 100.);
        assert_eq!((rt.state, g.done), (St::ChallengeRunning, None));
        assert_eq!(g.cmds.iter().filter(|c| matches!(c, Cmd::Reset(_))).count(), 5, "a failure only resets its objective");
        // The only other end is the pause menu's Quit: the coach's line, then Abandoned.
        g.abandon(&mut rt);
        assert_eq!((rt.state, g.done), (St::Complete, Some(Outcome::Abandoned)));
        assert!(g.cmds.contains(&Cmd::Speech { character: 0x1F, id: 5706 }));
    }

    #[test]
    fn kactivityid_is_the_key_as_f32() {
        let g = Lesson::new(KEY, false, false, 0);
        assert_eq!(g.activity_id().to_bits(), (KEY as f32).to_bits());
        // Started through a world load: the load screen until the row's own key arrives.
        let mut g = Lesson::new(KEY, false, false, 0);
        g.loaded = false;
        let mut rt = Rt::new(St::Initial, 1, 0.);
        start(&mut g, &mut rt);
        advance(&mut g, &mut rt, 0.);
        assert_eq!(rt.state, St::AssetLoad);
        assert!(g.cmds.contains(&Cmd::LoadScreen(true)));
        g.asset_loaded(&mut rt, 672_209_558_898_382_773);
        assert_eq!(rt.state, St::AssetLoad, "another row's load");
        g.asset_loaded(&mut rt, KEY);
        assert_eq!(rt.state, St::ChallengeRunning);
        assert!(g.cmds.contains(&Cmd::LoadScreen(false)));
    }

    fn career_dir() -> PathBuf {
        PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"))
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn port_of_maps_the_stage_one_rows() {
        let data = crate::skate3_career::data::load(&career_dir()).expect("the export");
        let row = |id: &str| data.row(id).unwrap_or_else(|| panic!("no row {id}"));
        for id in LESSONS {
            assert_eq!(port_of(row(id)), Some(Port::Lesson), "{id}");
        }
        for (id, r) in data.rows.iter().filter(|(_, r)| r.graph == "teleport") {
            assert_eq!(port_of(r), Some(Port::Teleport), "{id}");
        }
        assert_eq!(port_of(row("tele_sksc_01")), Some(Port::Teleport));
        assert_eq!(row("ftl_univ_02").kind, 23);
        assert_eq!(port_of(row("ftl_univ_02")), Some(Port::Follow));
        assert_eq!(port_of(row("film_univ_05")), Some(Port::Film));
        assert_eq!(port_of(row("photo_univ_08")), Some(Port::Photo));
    }

    /// The oracle's natives tutorials.lua calls beyond [`crate::skate3_modes::tests::DRIVER`]'s, logging the
    /// model's lines. A test sets KEYF (vars.kActivityID), LOADED, INTRO / RESTART (0 or 1), NIND, DIALOG_ANSWERS.
    const SETUP: &str = r#"
local ind = {Count = NIND}
for i = 1, NIND do ind[i] = i end
SELF = setmetatable({Name = "tut", StartLocation = "StartLocation0", SpawnOffboard = false,
  IgnoreInitialTutorialReposition = false, Persistent = false, AudioPlayerQuitChallenge = {Character = 31, Id = 5706},
  VisualIndicators = ind, IntroPresentationEvents = {Count = INTRO}, RestartPresentationEvents = {Count = RESTART},
  OutroPresentationEvents = {Count = 0}}, {__index = function() return nil end})
VARS.kActivityID = KEYF
Challenge.GetGameMode = function() return 0 end
Challenge.CompleteChallenge = function(_, r, p) log_line("complete " .. r .. " " .. tostring(p)) end
Objectives.MakeObjectives = function(id) log_line("make"); return 100 + id end
Objectives.SetEvaluationStatus = function(_, b) log_line("eval " .. tostring(b)) end
Objectives.ResetObjectives = function() log_line("reset") end
Objectives.ResetObjective = function(_, i) log_line("resetone " .. i) end
Objectives.ActivateChallengeStartObjectives = function() log_line("start") end
Objectives.DeleteObjectiveGroup = function() log_line("delete") end
HUD2.CreateGroup = function() log_line("hud true"); POST("hudloaded") end
HUD2.DeleteGroup = function() log_line("hud false") end
HUD2.SetTrickAnalyzerEnabled = function(b) log_line("trickanalyzer " .. tostring(b)) end
HUD2.SetManualMeterEnabled = function(b) log_line("manualmeter " .. tostring(b)) end
VisualIndicators = setmetatable({CreateGroup = function() return 9 end,
  AddIndicator = function(_, i) log_line("indicator add " .. i) end,
  RemoveIndicator = function(_, i) log_line("indicator remove " .. i) end,
  DeleteGroup = function() log_line("indicators delete") end}, getmetatable(Challenge))
HOM.EnableHOMVisuals = function(b) log_line("hom " .. tostring(b)) end
ChallengeAsset.IsChallengeAssetLoadComplete = function() return LOADED end
ChallengeAsset.ActivateChallengeAssetLoadScreen = function() log_line("loadscreen true") end
ChallengeAsset.DeactivateChallengeAssetLoadScreen = function() log_line("loadscreen false") end
ChallengeSpeech.PlayNPCSpeechUsingAttribute = function(a) log_line(string.format("speech %d %d", a.Character, a.Id)) end
Progression.LockForPresentation = function() log_line("lock true") end
Progression.UnlockFromPresentation = function() log_line("lock false") end
FrontEnd.eSk8BEEndMissionOptions = {Continue_Retry = "Continue_Retry", Retry_Quit = "Retry_Quit", Unknown = "Unknown"}
FrontEnd.Results_SetMainTitle = function(t) log_line("title " .. t) end
FrontEnd.SetEndChallengeOption = function(i, o) log_line("option " .. i .. " " .. o) end
FrontEnd.Results_SetNumPlayers = function(n) log_line("players " .. n) end
ChallengePresentation.RemoveAllPresentationGroups = function() log_line("presentations off") end
function ACTOR() return ME end
function DELIVER(kind, v, w)
  if kind == "all" then SEND("allobjectivecomplete", {ObjectiveGroup = 100 + ME}); return true end
  if kind == "fail" then SEND("objectivefailed", {ObjectiveGroup = 100 + ME, ObjectiveIndex = v, FailedIndex = math.floor(w)}); return true end
  if kind == "quit" then SEND("abandonchallenge"); return true end
  if kind == "restart" then SEND("restartchallenge"); return true end
  if kind == "load" then
    local key = 0
    if v == 1 then key = VARS.kActivityID; LOADED = true end
    SEND("challengeassetloadcomplete", {ActivityKey = key})
    return true
  end
  return false
end
"#;

    #[derive(Clone, Copy, Debug)]
    struct Case {
        key: u64,
        intro: bool,
        restart: bool,
        indicators: usize,
        loaded: bool,
    }

    type Plans = Vec<Vec<(f64, Ev)>>;

    /// (a plan per asset_load entry, the end dialog's answers, assets resident). The teleport lands 2 s in.
    fn scenarios() -> Vec<(Plans, Vec<&'static str>, bool)> {
        let all = |t: f64| (t, Ev::Other("all", 0, 0.));
        let fail = |t: f64, i: i64| (t, Ev::Other("fail", i, 0.));
        let ev = |t: f64, k: &'static str, v: i64| (t, Ev::Other(k, v, 0.));
        vec![
            // Win, Continue (a failure before and after the teleport lands; a load message outside asset_load).
            (vec![vec![fail(0.5, 2), fail(3., 2), ev(3.5, "load", 1), all(4.)]], vec![], true),
            // Win, Retry, win, Continue.
            (vec![vec![all(4.)], vec![fail(3., 1), all(4.)]], vec!["hudretry"], true),
            // The pause menu's Restart, then Quit while the teleport fades.
            (vec![vec![ev(3., "restart", 0)], vec![ev(0.5, "quit", 0)]], vec![], true),
            // A world load: another row's load, the lesson's, then win, Retry (loaded now), win.
            (vec![vec![ev(0.5, "load", 0), ev(1., "load", 1), all(5.)], vec![all(4.)]], vec!["hudretry"], false),
            // Win, Retry, then Quit: Abandoned again.
            (vec![vec![all(4.)], vec![ev(3., "quit", 0)]], vec!["hudretry"], true),
        ]
    }

    fn oracle(src: &str, c: &Case, answers: &[&str], plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let quoted: String = answers.iter().map(|a| format!("{a:?}, ")).collect();
        let setup = format!(
            "N, ME, FIRST = 1, 0, 0\nKEYF = {:?}\nLOADED = {}\nINTRO, RESTART, NIND = {}, {}, {}\nDIALOG_ANSWERS = {{{quoted}}}\n{SETUP}",
            f64::from(c.key as f32),
            c.loaded,
            u8::from(c.intro),
            u8::from(c.restart),
            c.indicators
        );
        let (lua, errors) = lua_log(src, &setup, "initial", "asset_load", plans);
        let mut g = Lesson::new(c.key, c.intro, c.restart, c.indicators);
        g.loaded = c.loaded;
        let mut rt = Rt::new(St::Initial, 1, 0.);
        rt.auto = Some(answers.iter().map(|a| if *a == "hudretry" { Msg::HudRetry } else { Msg::HudContinue }).collect());
        let other = |g: &mut Lesson, rt: &mut Rt<St>, kind: &str, v: i64, w: f64| -> Option<Msg> {
            match kind {
                "all" => Some(Msg::GroupAll { group: GROUP }),
                "fail" => Some(Msg::GroupFailed { group: GROUP, index: v as usize, failed: w as usize }),
                "restart" => Some(Msg::Restart),
                "quit" => {
                    g.abandon(rt);
                    None
                }
                "load" => {
                    let key = if v == 1 { g.key } else { 0 };
                    g.asset_loaded(rt, key);
                    None
                }
                _ => None,
            }
        };
        let model = model_log(&mut g, &mut rt, St::AssetLoad, plans, |_| Some(ME), |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// tutorials.lua itself (SKATE3_CAREER_DIR, scripts/graphs/tutorials.lua) decides every lesson the model runs the
    /// same way: the win path, Retry (the Restart presentation, ResetObjectives, setup again), the pause menu's Restart
    /// and Quit, a world load's wait for the row's key; with and without Intro / Restart events and indicators.
    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn skate3_tutorial_matches_tutorials_lua() {
        let dir = career_dir();
        let src = std::fs::read_to_string(dir.join("scripts/graphs/tutorials.lua")).expect("tutorials.lua");
        let data = crate::skate3_career::data::load(&dir).expect("the export");
        let keys = [
            " complete 1 false", " complete 0 false", " speech 31 5706", " resetone 1", " resetone 2", " loadscreen true",
            " loadscreen false", " lock false", " present", " reset", " indicator add 2", " enter restarted", " open",
        ];
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let mut runs = 0;
        for id in LESSONS {
            let key = data.row(id).unwrap_or_else(|| panic!("no row {id}")).key;
            for (s, (plans, answers, loaded)) in scenarios().into_iter().enumerate() {
                for pres in 0..4 {
                    for indicators in [0, 2] {
                        let c = Case { key, intro: pres & 1 == 1, restart: pres & 2 == 2, indicators, loaded };
                        let what = format!("{id} scenario {s} {c:?}");
                        let (lua, model, errors) = oracle(&src, &c, &answers, &plans);
                        compare(&what, &lua, &model, &errors);
                        assert!(errors.is_empty(), "{what}: {errors:?}");
                        for k in keys {
                            *seen.entry(k).or_default() += lua.iter().filter(|l| l.ends_with(k)).count();
                        }
                        runs += 1;
                    }
                }
            }
        }
        assert!(seen.values().all(|&n| n > 0), "every branch exercised: {seen:?}");
        eprintln!("tutorials oracle: {runs} runs identical; covered {seen:?}");
    }
}
