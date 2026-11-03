//! What the Skate 3 online modes (skate3_*.rs, after S.K.A.T.E.) share. Each mode's challenge graph is a hand port of
//! its decompiled Lua (tools/skate3_online.py `scripts/<graph>.lua`) as a [`Graph`] on [`Rt`], the stategraph runtime
//! the Lua runs on: ChangeState queued until the handler returns (stateexit, then stateenter), ChallengeTimer's
//! countdown timers, Effects' FadeOut / FadeIn, AI.RepositionAI and the presentations. What the graph asks of HUD2 is
//! kept in [`Hud2`] and, in tests, logged in the Lua oracle's words ([`tests::DRIVER`] logs the same lines from the
//! disc Lua); [`HudDriver`] draws it with the modes' shared HUD movies (`private/skate3-online/hud/runtime`) and the
//! online-ui leaderboard, on the online-ui layer [`ONLINE`].
use crate::{
    apt_screen::{Layer, ONLINE, Screen, Screens},
    config::Config,
    core_menu_runtime::thousands,
};
use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
};

/// The modes' shared HUD movies, installed beside each mode's folder.
pub(crate) const HUD_DIR: &str = "private/skate3-online/hud/runtime";
pub(crate) type Matrix = [[f32; 4]; 4];

// ---------------------------------------------------------------- data
/// A mode's data (tools/skate3_online.py): rules.json rows (the CLD fields the graph reads, by their Lua names),
/// strings.json, the intro scenes' lengths (tutorial.json).
pub(crate) struct ModeData {
    pub root: PathBuf,
    pub rows: BTreeMap<String, Json>,
    pub strings: HashMap<String, String>,
    pub intros: HashMap<String, f32>,
}

fn read<T: DeserializeOwned>(root: &Path, name: &str) -> Result<T, String> {
    let text = std::fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
}

impl ModeData {
    pub fn load(root: &Path) -> Result<Self, String> {
        #[derive(serde::Deserialize)]
        struct Rules {
            rows: BTreeMap<String, Json>,
        }
        #[derive(serde::Deserialize)]
        struct Scenes {
            scenes: HashMap<String, Scene>,
        }
        #[derive(serde::Deserialize)]
        struct Scene {
            duration: f32,
        }
        let rules: Rules = read(root, "rules.json")?;
        let scenes: Scenes = read(root, "tutorial.json")?;
        Ok(ModeData {
            root: root.to_owned(),
            rows: rules.rows,
            strings: read(root, "strings.json")?,
            intros: scenes.scenes.into_iter().map(|(k, s)| (k, s.duration)).collect(),
        })
    }
    /// The plugin's data from the install (`<asset root>/private/skate3-online/<mode>`), with a warning when absent.
    pub fn install(app: &App, mode: &str, label: &str) -> Option<Self> {
        let root = app.world().get_resource::<Config>().map(|c| c.asset_root.join("private/skate3-online").join(mode)).unwrap_or_default();
        Self::load(&root).inspect_err(|e| warn!("Skate 3 {label}: no data at {} ({e})", root.display())).ok()
    }
    pub fn row<T: DeserializeOwned>(&self, id: &str) -> Result<T, String> {
        let row = self.rows.get(id).ok_or_else(|| format!("no row {id}"))?;
        serde_json::from_value(row.clone()).map_err(|e| format!("{id}: {e}"))
    }
    pub fn text(&self, id: &str) -> String {
        self.strings.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }
    /// A message's text: a '#' literal, or the string with its parameters filled ([`fill`]).
    pub fn fmt(&self, text: &Text) -> String {
        match text.id.strip_prefix('#') {
            Some(literal) => literal.to_owned(),
            None => fill(&self.text(&text.id), &text.params),
        }
    }
}

/// The localized-string formatter (0x8290c9a0): `{n:kind}` takes parameter n; ints (`d`) and `points` are read as
/// integers (atoi, 0x82f4dec0) and points are grouped by the locale's separator (0x8290e178: ',' every 3 digits, as
/// FELanguage.GetLocalizedFormatNumberString's `thousands`). Other kinds take the parameter as it is.
pub(crate) fn fill(format: &str, params: &[String]) -> String {
    let mut out = String::new();
    let mut rest = format;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else { break };
        out.push_str(&rest[..start]);
        let field = &rest[start + 1..start + end];
        let mut parts = field.splitn(2, ':');
        match parts.next().and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => {
                let p = params.get(n).map(String::as_str).unwrap_or_default();
                let int = || p.trim().parse::<i64>().unwrap_or(0);
                match parts.next() {
                    Some("points") => out.push_str(&thousands(int() as f64)),
                    Some("d") => out.push_str(&int().to_string()),
                    // A player's name: its `^` breaks no line (only the format's own do).
                    _ => out.push_str(&crate::apt_text::unbroken(p)),
                }
            }
            None => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out + rest
}

/// The map a row is on, from its id's district (`<mode>_<dwtn|indu|univ>_NN`, as challenge_map's districts name them;
/// the DLC packages' districts as online.json places their rows: ots_mmcn_01 on Maloof_Money_Cup_NYC). Only rows a
/// mode's data has reach this: the DLC ones come from tools/skate3_online.py `--dlc` (Own the Spot).
pub(crate) fn map_of(row: &str) -> Option<&'static str> {
    [
        ("_dwtn_", "DownTown"),
        ("_indu_", "Industrial"),
        ("_univ_", "University"),
        ("_mmcn_", "Maloof_Money_Cup_NYC"),
        ("_dwmc_", "Danny_Way_Hawaiian_Dream"),
        ("_artg_", "Art_Gallery"),
        ("_dark_", "Sanatorium"),
        // online.json's own map for these ids (10-06: their rows had data and a ported map but were refused).
        ("_mmcp_", "MaloofMoneyCup"),
        ("_bbpk_", "BlackBoxPark"),
        ("_bbx_", "BlackBoxPark"),
        ("_skp5_", "University"),
    ]
    .into_iter()
    .find(|(d, _)| row.contains(d))
    .map(|(_, m)| m)
}

/// A locator from rules.json: its matrix and its slot matrices (RepositionAI's slot argument).
#[derive(serde::Deserialize, Clone, Debug)]
pub(crate) struct Locator {
    pub matrix: Matrix,
    #[serde(default)]
    pub slots: Vec<Matrix>,
}
impl Locator {
    /// RepositionAI's slot: TU3 0x82910eb0, the only code indexing a locator's children (online_map::locator_slot).
    pub fn slot(&self, i: usize) -> Matrix {
        crate::online_map::locator_slot(&self.matrix, &self.slots, i)
    }
}

/// A trigger volume from rules.json: its world box (cSim_Global.xsf's min / max).
#[derive(serde::Deserialize, Clone, Debug)]
pub(crate) struct Volume {
    pub min: [f32; 3],
    pub max: [f32; 3],
}
impl Volume {
    pub fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|k| self.min[k] <= p[k] && p[k] <= self.max[k])
    }
}

// ---------------------------------------------------------------- the stategraph runtime
/// Effects.eChallengeEffect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum Effect {
    FadeOut,
    FadeIn,
    /// The out-of-bounds static (contest_shared 1013, freeskate_ol 212): [`Rt::oob_static`], drawn through the session
    /// marker's noise as online_boundary draws free skate's. No fade and no effectfinished (no graph waits on it).
    OOB,
}

/// The stategraph messages the ported graphs handle (their Lua names in the comments).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Msg {
    /// stateenter / stateexit
    Enter,
    Exit,
    /// timerexpired { TimerID }
    Timer(u32),
    /// effectfinished { Effect }
    Effect(Effect),
    /// skaterteleportcomplete { PlayerID }
    Teleported(usize),
    /// teleporttransitionfinished
    TransitionDone,
    /// objectivecomplete for a player's objective group; the sequence's score (Trick.GetCompletedSequenceScore).
    Objective { player: usize, score: i64 },
    /// wipeout { PlayerID }
    Wipeout(usize),
    /// playerdisconnected { PlayerIndex }
    Left(usize),
    /// presentationeventgroupfinished
    Presented,
    /// highlightreelcomplete
    Reel,
    /// hudloaded
    HudLoaded,
    /// nischallengenotify
    Nis,
    /// onlinesynccomplete
    Synced,
    /// votingcomplete { PlayerID, Score }
    Voted { player: i64, score: f64 },
    /// triggerentercollision / triggerexitcollision { PlayerID, VolumeID }: a SpotVolumes index.
    Trigger { player: usize, volume: i64, inside: bool },
    /// sequencebegin { PlayerID }
    SequenceBegin(usize),
    /// sequencecomplete { PlayerID, SequenceScore, Wipedout }
    Sequence { player: usize, score: i64, wiped: bool },
    /// wipeoutalmostover { PlayerID, TimeToTeleport }
    AlmostOver { player: usize, time: f64 },
    /// wipeoutover { PlayerID }
    WipeoutOver(usize),
    /// aircompleted { PlayerID } (the measures are the graph's: contest_shared reads Trick.GetCompletedAir*).
    AirDone(usize),
    /// grindcompleted { PlayerID, GrindLength }
    GrindDone { player: usize, length: f64 },
    /// allobjectivecomplete / didnotgetenoughpoints for a player's objective group (skate3_objectives), with
    /// Objectives.GetAccumulatedPoints as the handler reads it.
    AllObjectives { player: usize, points: i64 },
    NotEnoughPoints { player: usize, points: i64 },
    /// updatesimulplayscore { PlayerName, Score, PreScoreTier, PostScoreTier, SetupScore } (SimulPlay manager
    /// 0x82565788, net message 49 from another console).
    SimulScore { player: usize, score: i64, pre: i64, post: i64, setup: bool },
    /// simulplaycomplete (the SimulPlay manager's forced completion, 0x82564a78).
    SimulComplete,
    /// simulplaycomplete { Success } from the QuantifiedCompletion rule (0x82564800).
    SimulDone(bool),
    /// objectivecomplete / allobjectivecomplete / didnotgetenoughpoints / objectivefailed by objective group (the
    /// Lua's group pointer), with what the handlers read of the group (Objectives.GetAccumulatedPoints).
    GroupObjective { group: u32, index: usize, points: bool, accumulated: i64 },
    GroupAll { group: u32 },
    GroupLow { player: usize, group: u32 },
    GroupFailed { group: u32, index: usize, failed: usize },
    /// usemarker { PlayerID }
    UseMarker(usize),
    /// skatermoving { PlayerID, IsMoving }
    Moving { player: usize, moving: bool },
    /// skateronboard / skateroffboard { PlayerID }
    Board { player: usize, on: bool },
    /// niscomplete
    NisDone,
    /// Party Play: hudcomplete (A on FrontEnd.OpenPressADialog's card), hudretry / hudcontinue (the results dialog's
    /// Rematch / the others, FrontEnd.OpenContinueRetryDialog), restartchallenge (the crossbar's Restart Challenge).
    HudComplete,
    HudRetry,
    HudContinue,
    Restart,
}

/// A handler stopped by a Lua runtime error (indexing nil, concatenating nil): the rest of it does not run.
#[derive(Debug)]
pub(crate) struct Abort;
pub(crate) type Handled = Result<(), Abort>;

/// RepositionAI's target: a locator field of the row (by its Lua name), its index in a rotation, the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub field: &'static str,
    pub index: usize,
    pub slot: usize,
}

/// A mode's challenge graph.
pub(crate) trait Graph {
    type State: Copy + PartialEq + std::fmt::Debug;
    /// The state's Lua name.
    fn name(state: Self::State) -> &'static str;
    fn on(&mut self, rt: &mut Rt<Self::State>, msg: &Msg) -> Handled;
}

/// The stategraph and the natives under a ported graph.
#[derive(Clone)]
pub(crate) struct Rt<S> {
    pub now: f64,
    pub state: S,
    queue: VecDeque<S>,
    timers: Vec<(u32, f64)>,
    next_timer: u32,
    /// ChallengeTimer's count-up timers: (id, start, paused at). They share the countdown timers' ids (ponytail: the
    /// native's id allocation is not traced; ids are opaque to the graphs).
    countups: Vec<(u32, f64, Option<f64>)>,
    effects: Vec<(Effect, f64)>,
    /// Effects.StartEffect(OOB, seconds) in force: (its start, seconds).
    pub oob: Option<(f64, f64)>,
    /// Messages posted for now (FIFO), and messages due later (presentations, the highlight reel).
    posted: VecDeque<Msg>,
    later: Vec<(f64, Msg)>,
    /// The screen fade: (alpha from, alpha to, start, end).
    fade: (f64, f64, f64, f64),
    /// RepositionAI's fade in progress: (start, fade, end = teleporttransitionfinished).
    transition: Option<(f64, f64, f64)>,
    pub hud: Hud2,
    /// Players: who left (Online.IsPlayerDisconnected), who is pinned (AI.PinAI), where each was last sent.
    pub gone: Vec<bool>,
    pub pinned: Vec<bool>,
    pub places: Vec<Option<(u32, Place)>>,
    /// When each player's last move lands ([`Rt::landed`]: at once, or at its fade's black) and the place shown until then.
    lands: Vec<(f64, Option<(u32, Place)>)>,
    moves: u32,
    /// The last move made by [`Rt::reposition_marked`] (its seq in `places`): the glue sets the session marker where it lands.
    pub marked: Option<u32>,
    /// Online.SwitchCamera's player.
    pub watching: usize,
    /// A faded RepositionAI's camera switch, due at its black: (when, player).
    camera_at: Option<(f64, usize)>,
    /// Challenge.CloseLoadingScreen / OpenLoadingScreen.
    pub covered: bool,
    /// The intro presentation's end.
    pub presenting: Option<f64>,
    /// Online.LeaveGame (the challenge is complete).
    pub left: bool,
    /// Pause.DisablePauseMenuAndBackpack is in force (EnablePauseMenuAndBackpack lifts it).
    pub pause_disabled: bool,
    /// SessionMarker.DisableSetSessionMarker / DisableUseSessionMarker are in force (their Enable lifts them).
    pub marker_set_disabled: bool,
    pub marker_use_disabled: bool,
    /// Online (set by the glue): the round sync points wait for the other consoles ([`Rt::release`]).
    pub online: bool,
    /// The round sync points reached (Online.OnlineChallengeRoundSyncPoint calls), and whether the last still waits.
    pub syncs: u32,
    pub sync_waiting: bool,
    /// Party Play: FrontEnd.OpenPressADialog's (player 1-based, round) waiting for the party session to show it, and
    /// FrontEnd.OpenContinueRetryDialog called (the results dialog is up).
    pub card: Option<(usize, i64)>,
    pub dialog: bool,
    /// ChallengeManager.SetPlayer: the player the one pad drives (Party Play).
    pub controlled: usize,
    /// Tests: the oracle answers the card at once (hudcomplete) and each dialog with the next of these (then
    /// hudcontinue).
    pub auto: Option<VecDeque<Msg>>,
    /// The oracle log (tests).
    pub log: Option<Vec<String>>,
}

impl<S: Copy + PartialEq + std::fmt::Debug> Rt<S> {
    pub fn new(initial: S, players: usize, now: f64) -> Self {
        Rt {
            now,
            state: initial,
            queue: VecDeque::new(),
            timers: Vec::new(),
            next_timer: 0,
            countups: Vec::new(),
            effects: Vec::new(),
            oob: None,
            posted: VecDeque::new(),
            later: Vec::new(),
            fade: (0., 0., now, now),
            transition: None,
            hud: Hud2::default(),
            gone: vec![false; players],
            pinned: vec![false; players],
            places: vec![None; players],
            lands: vec![(now, None); players],
            moves: 0,
            marked: None,
            watching: 0,
            camera_at: None,
            covered: true,
            presenting: None,
            left: false,
            pause_disabled: false,
            marker_set_disabled: false,
            marker_use_disabled: false,
            online: false,
            syncs: 0,
            sync_waiting: false,
            card: None,
            dialog: false,
            controlled: 0,
            auto: None,
            log: None,
        }
    }
    /// FrontEnd.OpenPressADialog(player, round): "Player N ready!"; A answers hudcomplete.
    pub fn press_a(&mut self, player: usize, round: i64) {
        self.note(format!("pressa {player} {round}"));
        self.card = Some((player, round));
        if self.auto.is_some() {
            self.post(Msg::HudComplete);
        }
    }
    /// FrontEnd.OpenContinueRetryDialog: the results dialog (Party Play's Rematch / Exit / Change settings).
    pub fn continue_retry(&mut self) {
        self.note("dialog");
        self.dialog = true;
        if let Some(answers) = self.auto.as_mut() {
            let answer = answers.pop_front().unwrap_or(Msg::HudContinue);
            self.post(answer);
        }
    }
    /// ChallengeManager.SetPlayer: the pad drives `player` (not logged: the oracle's ChallengeManager is a stub).
    pub fn set_player(&mut self, player: usize) {
        self.controlled = player;
    }
    pub fn note(&mut self, line: impl AsRef<str>) {
        let now = self.now;
        if let Some(log) = self.log.as_mut() {
            log.push(format!("{now:.2} {}", line.as_ref()));
        }
    }
    pub fn change(&mut self, state: S) {
        self.queue.push_back(state);
    }
    pub fn post(&mut self, msg: Msg) {
        self.posted.push_back(msg);
    }
    /// ChallengeTimer.AddCountdownTimer / AddHUDCountdownTimer: ids count up from 1.
    pub fn add_timer(&mut self, seconds: f64) -> u32 {
        self.next_timer += 1;
        self.timers.push((self.next_timer, self.now + seconds));
        self.next_timer
    }
    pub fn remove_timer(&mut self, id: u32) {
        self.timers.retain(|t| t.0 != id);
        if self.hud.timer.is_some_and(|t| t.0 == id) {
            self.hud.timer = None;
        }
    }
    pub fn timer_end(&self, id: u32) -> Option<f64> {
        self.timers.iter().find(|t| t.0 == id).map(|t| t.1)
    }
    /// ChallengeTimer.GetCountdownTime: the time left (ponytail: 0 for a timer that is not running; not traced).
    pub fn countdown_time(&self, id: u32) -> f64 {
        self.timer_end(id).map_or(0., |end| end - self.now)
    }
    /// ChallengeTimer.AddCountupTimer.
    pub fn add_countup(&mut self) -> u32 {
        self.next_timer += 1;
        self.countups.push((self.next_timer, self.now, None));
        self.next_timer
    }
    /// ChallengeTimer.GetCountupTime (ponytail: 0 for a removed timer; not traced).
    pub fn countup_time(&self, id: u32) -> f64 {
        self.countups.iter().find(|c| c.0 == id).map_or(0., |c| c.2.unwrap_or(self.now) - c.1)
    }
    /// ChallengeTimer.PauseTimer on a count-up timer.
    pub fn pause_countup(&mut self, id: u32) {
        let now = self.now;
        if let Some(c) = self.countups.iter_mut().find(|c| c.0 == id) {
            c.2.get_or_insert(now);
        }
    }
    pub fn remove_countup(&mut self, id: u32) {
        self.countups.retain(|c| c.0 != id);
    }
    /// Effects.StartEffect: the screen fades over `seconds`, then effectfinished; OOB starts the static.
    pub fn start_effect(&mut self, effect: Effect, seconds: f64) {
        self.note(format!("effect {effect:?} {seconds}"));
        if effect == Effect::OOB {
            self.oob = Some((self.now, seconds));
            return;
        }
        let alpha = self.fade_alpha(self.now);
        let to = if effect == Effect::FadeOut { 1. } else { 0. };
        self.fade = (alpha, to, self.now, self.now + seconds);
        self.effects.retain(|e| e.0 != effect);
        self.effects.push((effect, self.now + seconds));
    }
    /// Effects.ResetEffect: the effect stops and the screen clears (OOB: the static only, a fade runs on).
    pub fn reset_effect(&mut self, effect: Effect) {
        self.note(format!("reseteffect {effect:?}"));
        if effect == Effect::OOB {
            self.oob = None;
            return;
        }
        self.effects.retain(|e| e.0 != effect);
        self.fade = (0., 0., self.now, self.now);
    }
    /// The OOB static's amount (session_marker's `oob`): 0 as StartEffect(OOB, seconds) starts, 1 at its end, 0 after
    /// ResetEffect(OOB). ponytail: the effect's curve is not traced; online_boundary's linear ramp over its countdown.
    pub fn oob_static(&self, now: f64) -> f32 {
        self.oob.map_or(0., |(start, seconds)| ((now - start) / seconds.max(1e-3)).clamp(0., 1.) as f32)
    }
    /// How black the screen is: the Effects fade, or RepositionAI's (half out, the hold, half in).
    pub fn fade_alpha(&self, now: f64) -> f64 {
        let (a, b, t0, t1) = self.fade;
        let effect = if now >= t1 { b } else { a + (b - a) * ((now - t0) / (t1 - t0)).clamp(0., 1.) };
        let transition = self.transition.map_or(0., |(start, fade, end)| {
            let half = (fade / 2.).max(1e-3);
            ((now - start) / half).min((end - now) / half).clamp(0., 1.)
        });
        effect.max(transition)
    }
    /// AI.RepositionAI: a fade at or under 0.001 moves the player at once (skaterteleportcomplete); a longer one joins
    /// the fade in progress or starts one (0x82d54c90): out over half, the move, the hold, in over half, then
    /// teleporttransitionfinished.
    pub fn reposition(&mut self, player: usize, place: Place, fade: f64) {
        self.note(format!("move {player} {}{} {} {fade}", place.field, place.index, place.slot));
        if self.gone.get(player).copied().unwrap_or(true) {
            return;
        }
        let shown = self.landed(player);
        self.moves += 1;
        self.places[player] = Some((self.moves, place));
        if fade <= 0.001 {
            self.post(Msg::Teleported(player));
            self.lands[player] = (self.now, shown);
        } else {
            let hold = f64::from(crate::skate3_skate_game::FADE_HOLD);
            let (start, fade, _) = *self.transition.get_or_insert((self.now, fade, self.now + fade + hold));
            self.lands[player] = (start + fade / 2., shown);
        }
    }
    /// AI.RepositionAI with its 4th argument true: for the local player 0x82d550a8 also posts cMsgSetSessionMarker
    /// 0x9A96B5F7 {Transform, FootForward, IsOnBoard = !7th} (forced), so the session marker lands on the place.
    /// ponytail: one seq, as only the local player's own moves pass it.
    pub fn reposition_marked(&mut self, player: usize, place: Place, fade: f64) {
        let before = self.moves;
        self.reposition(player, place, fade);
        if self.moves != before {
            self.marked = Some(self.moves);
        }
    }
    /// Where the player is as the screen shows it (the glue teleports the skater, Party Play stands the puppets): its
    /// last move once landed, a faded one at black when 0x82d55fa8 moves the batch, the place before until then (online
    /// audit 10-06: every S3 mode moved its skater in view as the fade started).
    pub fn landed(&self, player: usize) -> Option<(u32, Place)> {
        let (at, before) = *self.lands.get(player)?;
        if self.now >= at { self.places[player] } else { before }
    }
    /// AI.RepositionAI with its 6th argument true (the Lua: "Repositioning ... and switching camera"): the move, then
    /// the camera on that player: 0x82d54c90 keeps the player at +64 and, once the batch moves (0x82d55fa8), hands it
    /// to the camera owner (+24 vtable 40), as SwitchCamera. M4 (online audit 10-06): so a faded move switches the
    /// camera with it, at black (as [`Rt::landed`] moves the player); an immediate one (fade 0) at once.
    pub fn reposition_watched(&mut self, player: usize, place: Place, fade: f64) {
        self.reposition(player, place, fade);
        if !self.gone.get(player).copied().unwrap_or(true) {
            let before = self.watching;
            self.camera(player);
            let at = self.lands[player].0;
            if self.now < at {
                (self.watching, self.camera_at) = (before, Some((at, player)));
            }
        }
    }
    /// The camera switch of a faded [`Rt::reposition_watched`], once the clock has reached its black.
    fn switch_camera(&mut self) {
        if let Some((_, player)) = self.camera_at.filter(|c| self.now >= c.0) {
            (self.watching, self.camera_at) = (player, None);
        }
    }
    /// NIS.EnterNISUsingDefinition: the scene plays, then nischallengenotify. ponytail: the notify is taken at the
    /// scene's end, as Skate 2's progression NIS sends cMsgNISChallengeNotify after its cutscene
    /// (skate2port/tutorial_recovery.md); the S3 NIS player is not traced.
    pub fn nis(&mut self, seconds: f64) {
        self.note("nis");
        self.presenting = Some(self.now + seconds);
        self.later.push((self.now + seconds, Msg::Nis));
    }
    /// Online.OnlineChallengeRoundSyncPoint: onlinesynccomplete once every console is there ([`Rt::release`]; the
    /// glue publishes `syncs` as match_core's `Match::round`). Offline (hot-seat, the oracles) at once.
    pub fn sync(&mut self) {
        self.note("sync");
        self.syncs += 1;
        if self.online {
            self.sync_waiting = true;
            self.hud.waiting = true;
        } else {
            self.post(Msg::Synced);
        }
    }
    /// The sync point waiting is passed once every player still in the match (`rounds`: match_core's `Match::rounds`,
    /// u32::MAX for one who left) has reached it.
    pub fn release(&mut self, rounds: &[u32]) {
        if self.sync_waiting && rounds.iter().all(|&r| r >= self.syncs) {
            self.sync_waiting = false;
            self.hud.waiting = false;
            self.post(Msg::Synced);
        }
    }
    pub fn pin(&mut self, player: usize, pinned: bool) {
        self.note(format!("{} {player}", if pinned { "pin" } else { "unpin" }));
        if let Some(p) = self.pinned.get_mut(player) {
            *p = pinned;
        }
    }
    pub fn camera(&mut self, player: usize) {
        self.note(format!("camera {player}"));
        self.watching = player;
    }
    pub fn close_loading(&mut self) {
        self.note("close");
        self.covered = false;
    }
    pub fn open_loading(&mut self) {
        self.note("open");
        self.covered = true;
    }
    /// The intro's NIS or presentation is playing ([`Rt::nis`] / [`Rt::present`]): the skater is held (user 10-14:
    /// tricks went off during a Spot Battle's intro scene). INFERRED: retail's NIS player takes the pad while a scene
    /// plays; UNTRACED (the S3 NIS player). Each glue's Match::hold adds it.
    pub fn presenting_now(&self) -> bool {
        self.presenting.is_some_and(|end| self.now < end)
    }
    /// ChallengePresentation.StartPresentation: presentationeventgroupfinished after the intro's `seconds`.
    pub fn present(&mut self, seconds: f64) {
        self.note("present");
        self.presenting = Some(self.now + seconds);
        self.later.push((self.now + seconds, Msg::Presented));
    }
    /// HighlightReel.PlayHighlightReel: ponytail: no replay is kept, so highlightreelcomplete comes at once.
    pub fn reel(&mut self) {
        self.note("reel");
        self.post(Msg::Reel);
    }
    pub fn leave(&mut self) {
        self.note("leave");
        self.left = true;
    }
    /// Pause.DisablePauseMenuAndBackpack (true) / EnablePauseMenuAndBackpack (false): the pause menu, the backpack and
    /// the session markers (match_core::Match::pause_disabled).
    pub fn pause(&mut self, disabled: bool) {
        self.note(format!("pause {disabled}"));
        self.pause_disabled = disabled;
    }
    /// SessionMarker.DisableSetSessionMarker (true) / EnableSetSessionMarker (false): placing a marker
    /// (session_marker's set-disabled list, through match_core::Match).
    pub fn disable_set_marker(&mut self, disabled: bool) {
        self.note(format!("set marker disabled {disabled}"));
        self.marker_set_disabled = disabled;
    }
    /// SessionMarker.DisableUseSessionMarker (true) / EnableUseSessionMarker (false): returning to it.
    pub fn disable_use_marker(&mut self, disabled: bool) {
        self.note(format!("use marker disabled {disabled}"));
        self.marker_use_disabled = disabled;
    }

    /// The next message due: posted ones first, then (by time) later ones, the effects, RepositionAI's transition and
    /// the timers (by id); ties in that order.
    fn next(&self) -> Option<(f64, u8, usize)> {
        let mut best: Option<(f64, u8, usize)> = None;
        let mut offer = |c: (f64, u8, usize)| {
            if best.is_none_or(|b| (c.0, c.1, c.2) < (b.0, b.1, b.2)) {
                best = Some(c);
            }
        };
        for (i, l) in self.later.iter().enumerate() {
            offer((l.0, 0, i));
        }
        for (i, e) in self.effects.iter().enumerate() {
            offer((e.1, 1, i));
        }
        if let Some((_, _, end)) = self.transition {
            offer((end, 2, 0));
        }
        for t in &self.timers {
            offer((t.1, 3, t.0 as usize));
        }
        best
    }
    /// When the next message is due (posted ones: now).
    pub fn due(&self) -> Option<f64> {
        if !self.posted.is_empty() { Some(self.now) } else { self.next().map(|n| n.0) }
    }
    fn pop(&mut self, now: f64) -> Option<Msg> {
        if let Some(m) = self.posted.pop_front() {
            return Some(m);
        }
        let (at, kind, i) = self.next().filter(|n| n.0 <= now)?;
        self.now = self.now.max(at);
        self.switch_camera();
        Some(match kind {
            0 => self.later.remove(i).1,
            1 => Msg::Effect(self.effects.remove(i).0),
            2 => {
                self.transition = None;
                Msg::TransitionDone
            }
            _ => {
                let id = i as u32;
                self.timers.retain(|t| t.0 != id);
                if self.hud.timer.is_some_and(|t| t.0 == id) {
                    self.hud.timer = None;
                }
                Msg::Timer(id)
            }
        })
    }

    // ------------------------------------------------------------ HUD2
    fn shown(&mut self, kind: &str, id: &str, colour: u8, params: &[String], tail: impl std::fmt::Display) -> Message {
        self.note(format!("{kind} {id} {colour} {} {tail}", params.join(",")));
        self.hud.seq += 1;
        Message { seq: self.hud.seq, text: Text { id: id.to_owned(), params: params.to_vec() }, colour, at: self.now }
    }
    /// HUD2.ShowCentralMessage(text, colour = White, a, b, c, d, hook = 0) (0x82850618 -> 0x82663f38: SetState 4).
    pub fn central(&mut self, id: &str, colour: u8, params: &[String], hook: u8) {
        let m = self.shown("central", id, colour, params, hook);
        self.hud.central = Some((m, hook));
    }
    /// HUD2.ShowCustomCentralMessage(text, colour, params): the comma-separated params fill the string (0x82850968 ->
    /// 0x82664038 -> 0x8290c9a0, split at ','), shown as a central message (SetState 4).
    pub fn custom_central(&mut self, id: &str, colour: u8, params: &str) {
        let params: Vec<String> = params.split(',').map(str::to_owned).collect();
        self.central(id, colour, &params, 0);
    }
    /// HUD2.ShowModifiedCentralMessage(text, colour, a, b, c, d, stay) (0x82850770: SetState 10; 11 after its time
    /// unless it stays).
    pub fn modified(&mut self, id: &str, colour: u8, params: &[String], stay: bool) {
        let m = self.shown("modified", id, colour, params, stay);
        self.hud.modified = Some((m, stay));
    }
    /// HUD2.ClearModifiedCentralMessage (0x828508e0: SetState 88).
    /// HUD2.ShowChallengeFailedMessage(text). ponytail: drawn as a red central message; the native is not traced.
    pub fn failed(&mut self, id: &str) {
        self.note(format!("failed {id}"));
        self.hud.seq += 1;
        let m = Message { seq: self.hud.seq, text: Text { id: id.to_owned(), params: vec![] }, colour: RED, at: self.now };
        self.hud.central = Some((m, 0));
    }
    pub fn clear_modified(&mut self) {
        self.note("clear");
        self.hud.seq += 1;
        self.hud.cleared = self.hud.seq;
        self.hud.modified = None;
    }
    /// HUD2.ShowSkateMessage(name, letters, skate = true, local = false) (0x82850f18: the throwdown message component's
    /// +12 / +16 / +13): hudthrowmes plays S-K-A-T-E letter n (SetState n) or 1-UP letter n (SetState 5 + n).
    pub fn letters(&mut self, letters: i64, skate: bool, local: bool) {
        self.note(format!("letters {letters} {skate} {local}"));
        self.hud.seq += 1;
        // 1-Up's states are silent: sub_826692F8 plays 1up_{1,u,p}_user for this console's side, else _challenger.
        let sound = (!skate).then(|| match (letters, local) {
            (1, true) => "1up_1_user",
            (2, true) => "1up_u_user",
            (_, true) => "1up_p_user",
            (1, false) => "1up_1_challenger",
            (2, false) => "1up_u_challenger",
            (_, false) => "1up_p_challenger",
        });
        self.hud.letters = Some((self.hud.seq, if skate { letters } else { 5 + letters }, sound));
    }
    /// HUD2.LeaderboardSetTurn (0x828513c8): without its 4th argument the turn icon is this player's alone.
    pub fn turn(&mut self, player: usize) {
        self.note(format!("turn {player}"));
        self.hud.turn = Some(player);
        self.hud.turns.clear();
    }
    /// HUD2.LeaderboardSetTurn(name, id, enable, false) (0x828513c8: the 4th argument false sets this player's icon
    /// (+70 + 60 id) and leaves the others').
    pub fn turn_set(&mut self, player: usize, on: bool) {
        self.note(format!("turn {player} {on}"));
        if self.hud.turns.len() <= player {
            self.hud.turns.resize(player + 1, false);
        }
        self.hud.turns[player] = on;
    }
    /// HUD2.LeaderboardHUDInit(name, menu type, players, teams).
    pub fn board_init(&mut self, layout: i64, players: usize) {
        self.board_init_teams(layout, players, Vec::new(), Default::default());
    }
    /// HUD2.LeaderboardHUDInit with teams: each player's team (Online.GetTeam: 1 top / red, 2 bottom / blue; empty for
    /// no teams) and the two team names (Teams.GetName).
    /// The native (0x82851018) writes the menu type (+556), the player count (+412), the teams flag (+562) and the
    /// player rows; the show-players flag (+563) and the team records (+352) keep what was set before it.
    pub fn board_init_teams(&mut self, layout: i64, players: usize, teams: Vec<u8>, team_names: [String; 2]) {
        self.note(if teams.is_empty() { format!("board {layout} {players}") } else { format!("board {layout} {players} teams") });
        let variant = self.hud.board.variant.take();
        let (hide_players, team_highlight) = (self.hud.board.hide_players, self.hud.board.team_highlight);
        self.hud.board = Board {
            layout,
            ranks: (1..=players as i64).collect(),
            ints: vec![0; players],
            variant,
            teams,
            team_names,
            hide_players,
            team_highlight,
            ..Default::default()
        };
    }
    /// HUD2.LeaderboardHUDSetShowPlayers(name, show = true) (0x828511b0: +563): false shows the team rows alone.
    pub fn board_show_players(&mut self, show: bool) {
        self.note(format!("showplayers {show}"));
        self.hud.board.hide_players = !show;
    }
    /// HUD2.LeaderboardHUDSetTeamRank(name, team, rank) (0x82851330: team record +352, 20 bytes each).
    pub fn board_team_rank(&mut self, team: usize, rank: i64) {
        self.note(format!("teamrank {team} {rank}"));
        if let Some(r) = self.hud.board.team_ranks.get_mut(team.wrapping_sub(1)) {
            *r = rank;
        }
    }
    /// HUD2.LeaderboardHUDSetTeamScore(name, team, score) (0x82851230).
    pub fn board_team_score(&mut self, team: usize, score: i64) {
        self.note(format!("teamscore {team} {score}"));
        if let Some(s) = self.hud.board.team_scores.get_mut(team.wrapping_sub(1)) {
            *s = score;
        }
    }
    /// HUD2.LeaderboardHUDSetTeamHighlight(name, team, on) (0x82851bb0).
    pub fn board_team_highlight(&mut self, team: usize, on: bool) {
        self.note(format!("teamhighlight {team} {on}"));
        if let Some(h) = self.hud.board.team_highlight.get_mut(team.wrapping_sub(1)) {
            *h = on;
        }
    }
    /// HUD2.ShowTeamCentralMessage(text, team, type, a, b, c, d) (0x82850af0 -> 0x826642c8: the team (0 red, 1 blue)
    /// and the text for Central_GetTeamMessageData; HUD2.eTeamMessageType (0x8284fb70) Normal 0 -> SetState 13,
    /// Assignment 1 -> 15, Winning 2 -> 16; SetState 14 after 120 updates).
    pub fn team_central(&mut self, id: &str, team: i64, kind: u8, params: &[String]) {
        self.note(format!("teamcentral {id} {team} {kind} {}", params.join(",")));
        self.hud.seq += 1;
        let m = Message { seq: self.hud.seq, text: Text { id: id.to_owned(), params: params.to_vec() }, colour: WHITE, at: self.now };
        self.hud.team_central = Some((m, team, kind));
    }
    /// HUD2.ShowCustomTeamCentralMessage(text, team, type, params) (0x82850c58 -> 0x826643c0): the comma-separated
    /// params fill the string, as ShowCustomCentralMessage.
    pub fn custom_team_central(&mut self, id: &str, team: i64, kind: u8, params: &str) {
        let params: Vec<String> = params.split(',').map(str::to_owned).collect();
        self.team_central(id, team, kind, &params);
    }
    pub fn board_rank(&mut self, player: usize, rank: i64) {
        self.note(format!("rank {player} {rank}"));
        if let Some(r) = self.hud.board.ranks.get_mut(player) {
            *r = rank;
        }
    }
    pub fn board_int(&mut self, player: usize, value: i64) {
        self.note(format!("int {player} {value}"));
        if let Some(v) = self.hud.board.ints.get_mut(player) {
            *v = value;
        }
    }
    /// HUD2.Show(name, component, visible).
    pub fn show(&mut self, component: Component, visible: bool) {
        self.note(format!("show {component:?} {visible}"));
        match component {
            Component::LeaderboardHUD => self.hud.board.visible = visible,
            Component::DPOHUD => self.hud.dpo.visible = visible,
            Component::CountdownHUD if !visible => self.hud.countdown = None,
            Component::TimerHUD if !visible => self.hud.timer = None,
            Component::TrickDisplayHUD => self.hud.trick_hidden = !visible,
            Component::ObjectiveHUD => self.hud.objective = visible,
            Component::OwnedItHUD => self.hud.owned_it = visible.then_some(self.now),
            Component::KilledItHUD => self.hud.killed_it = visible.then_some(self.now),
            // ponytail: no Skate 3 minimap is drawn on the Skate 3 maps here, so hiding it has nothing to hide.
            _ => {}
        }
    }
    /// HUD2.LeaderboardHUDSetVariantString(name, text, _): the leaderboard's variant line (leaderboardhud's setup
    /// block item 3).
    pub fn board_variant(&mut self, id: &str) {
        self.note(format!("variant {id}"));
        self.hud.board.variant = Some(id.to_owned());
    }
    /// HUD2.DPOSetTeamColour(name, team = -1) (0x828531f0 -> 0x8266aed0, both DPOs): team 0 red, 1 blue, else none.
    pub fn dpo_team(&mut self, team: i64) {
        self.note(format!("dpoteam {team}"));
        self.hud.dpo.team = (0..=1).contains(&team).then_some(team);
    }
    pub fn dpo_format(&mut self, format: i64) {
        self.note(format!("dpoformat {format}"));
        self.hud.dpo.format = format;
    }
    pub fn dpo_title(&mut self, title: &str) {
        self.note(format!("dpotitle {title}"));
        self.hud.dpo.title = title.to_owned();
    }
    /// HUD2.DPOSetParams(name, a, b, index) / DPOSetTitle(name, title, index) for one of the two DPOs.
    pub fn dpo_at(&mut self, index: usize, a: i64, b: i64) {
        self.note(format!("dpo {a} {b} {index}"));
        if index == 0 {
            (self.hud.dpo.a, self.hud.dpo.b) = (a, b);
        } else {
            (self.hud.dpo.a2, self.hud.dpo.b2) = (a, b);
        }
    }
    pub fn dpo_title_at(&mut self, index: usize, title: &str) {
        self.note(format!("dpotitle {title} {index}"));
        if index == 0 {
            self.hud.dpo.title = title.to_owned();
        } else {
            self.hud.dpo.title2 = title.to_owned();
        }
    }
    /// HUD2.DPOUseSecondDPO(name, use).
    pub fn dpo_two(&mut self, two: bool) {
        self.note(format!("dpotwo {two}"));
        self.hud.dpo.two = two;
    }
    /// Objectives.ChangeObjectiveText(group, 0, text): the objective HUD's line.
    pub fn objective_text(&mut self, id: &str) {
        self.note(format!("objtext {id}"));
        self.hud.objective_text = id.to_owned();
    }
    /// HUD2.DPOSetParams(name, a = 0, b = 0) (0x82852d20).
    pub fn dpo_params(&mut self, a: i64, b: Option<i64>) {
        self.note(format!("dpo {a} {}", b.map_or("-".to_owned(), |b| b.to_string())));
        self.hud.dpo.a = a;
        self.hud.dpo.b = b.unwrap_or(0);
    }
    /// HUD2.StartCountdown(name, timer): the countdown HUD counts the timer down.
    pub fn countdown(&mut self, timer: u32) {
        self.note(format!("countdown {timer}"));
        self.hud.countdown = self.timer_end(timer);
    }
    /// HUD2.StartTimer(name, timer): the TimerHUD shows the timer until it ends or is removed.
    pub fn timer(&mut self, timer: u32) {
        self.note(format!("timer {timer}"));
        self.hud.timer = self.timer_end(timer).map(|end| (timer, end));
    }
    /// FrontEnd.Results_SetPlayerData(player, rank, value): that player's row, set (user 10-14: a Party Play Rematch's
    /// results listed the first game's rows too). INFERRED from the name: one row per player. UNTRACED: the native.
    pub fn result(&mut self, player: usize, rank: i64, value: i64) {
        self.note(format!("result {player} {rank} {value}"));
        match self.hud.results.iter_mut().find(|r| r.0 == player) {
            Some(row) => *row = (player, rank, value),
            None => self.hud.results.push((player, rank, value)),
        }
    }
}

/// Runs one message through the graph, then its state changes (stateexit, stateenter) until none are queued.
pub(crate) fn dispatch<G: Graph>(g: &mut G, rt: &mut Rt<G::State>, msg: Msg) {
    if g.on(rt, &msg).is_err() {
        rt.note("error");
    }
    settle(g, rt);
}

/// Not retail (user 10-12, online audit): the end of a timed phase (a heat, a round, the challenge) waits for every
/// other console still here to reach that end too, so every console ends it on the same events. Each console relays an
/// End marker ([`EndMark`]) once it is done with the phase (its own timer ran out, or the phase ended); the
/// marker travels behind that console's last event of the phase (the match relay keeps each sender's order). A
/// console whose timer ends the phase first holds there (the graph's clock stops just short of it, the local
/// skater scores nothing more) until every other console still here has sent its marker, at most END_HOLD_MAX.
/// Without this an event sent in the phase's last moments (a sequence banked on the buzzer, a finish) reached the
/// other consoles after their own timers had ended the phase and was dropped there: the results differed per console.
#[derive(Clone, Debug, Default)]
pub(crate) struct EndSync {
    /// The current phase's number (every console numbers the phases the same way: one per end of the running state).
    pub phase: u32,
    /// The other consoles' End markers: (player, phase) -> the end it carried (a later marker replaces it).
    ends: BTreeMap<(usize, u32), u32>,
    /// When this console entered the running state (seen at a frame).
    entered: f64,
    /// When this console began to hold the current phase's end.
    holding: Option<f64>,
    sent: bool,
    running: bool,
    /// Relay this console's marker only when it leaves the phase, not when it starts to hold (a judge's verdicts on
    /// what arrives while it holds go out first: skate3_otl).
    pub defer: bool,
    /// How long to hold at most (0: [`END_HOLD_MAX`]).
    pub max: f64,
    /// The judge (skate3_otl): its marker is flagged [`JUDGED`] and relayed after its last verdict; a plain one it
    /// relayed before it judged (as a follower) does not count for it.
    pub judge: Option<usize>,
}
/// The longest a console holds a phase's end for another (a stuck or vanished console); a knob.
pub(crate) const END_HOLD_MAX: f64 = 3.;
/// The judge's End marker: the phase number with this bit.
pub(crate) const JUDGED: u32 = 1 << 31;
/// An End marker: the phase number (the judge's with [`JUDGED`]), and the sender's end of it in ms since it entered the
/// running state (u32::MAX: it left the phase, final). Matrix G8 (10-12): a console that relayed its marker while
/// holding, then saw the end move later (a race's gate bonus, a judge's Sync), relays a fresh one; the first is
/// stale and no longer lets the others end the phase before that console's last events reach them.
pub(crate) type EndMark = (u32, u32);
/// A marker whose end is this much (s) before this console's own is stale; a knob. Ends on every console are the same
/// timeline to a frame (each sees its entry at a frame); a move smaller than this is not caught (the phase then ends as
/// before G8), a skew bigger costs at most a hold to [`END_HOLD_MAX`] (the sender's events are ahead of its marker).
pub(crate) const END_STALE: f64 = 0.5;
impl EndSync {
    /// The marker `player` relays for `phase`.
    fn marker(&self, player: usize, phase: u32) -> u32 {
        if self.judge == Some(player) { phase | JUDGED } else { phase }
    }
    /// The phase's end moved later, or this console now judges it: no hold (the next one starts in full) and the next
    /// end relays a fresh marker.
    pub fn rearm(&mut self) {
        (self.holding, self.sent) = (None, false);
    }
    /// Another console's End marker.
    pub fn ended(&mut self, player: usize, (phase, end): EndMark) {
        self.ends.insert((player, phase), end);
    }
    /// This console is holding the phase's end: the local skater's play no longer counts.
    pub fn holding(&self) -> bool {
        self.holding.is_some()
    }
    /// The running state was left (since the last look): that phase's marker (this console's, `me`), if not sent yet.
    fn observe(&mut self, running: bool, me: usize, now: f64) -> Option<EndMark> {
        let mut out = None;
        if !self.running && running {
            self.entered = now;
        }
        if self.running && !running {
            out = (!self.sent).then(|| (self.marker(me, self.phase), u32::MAX));
            (self.phase, self.sent, self.holding) = (self.phase + 1, false, None);
        }
        self.running = running;
        out
    }
}

/// When the next message due by `now` takes the graph out of `running` (a copy is run ahead): its time.
fn ends_at<G: Graph + Clone>(g: &G, rt: &Rt<G::State>, now: f64, running: G::State) -> Option<f64> {
    let (mut g, mut rt) = (g.clone(), rt.clone());
    rt.log = None;
    for _ in 0..10_000 {
        let at = rt.due().filter(|&d| d <= now)?;
        let msg = rt.pop(now)?;
        dispatch(&mut g, &mut rt, msg);
        if rt.state != running {
            return Some(at);
        }
    }
    None
}

/// [`advance`] online, with [`EndSync`]: returns the End marker this console relays now, if any.
pub(crate) fn advance_synced<G: Graph + Clone>(
    g: &mut G,
    rt: &mut Rt<G::State>,
    now: f64,
    running: G::State,
    me: usize,
    sync: &mut EndSync,
) -> Option<EndMark> {
    let mut out = sync.observe(rt.state == running, me, rt.now);
    let mut until = now;
    if rt.online
        && rt.state == running
        && rt.due().is_some_and(|d| d <= now)
        && let Some(at) = ends_at(g, rt, now, running)
    {
        let since = *sync.holding.get_or_insert(now);
        let own = ((at - sync.entered).max(0.) * 1000.).round() as u32;
        if !sync.defer && !std::mem::replace(&mut sync.sent, true) {
            out = Some((sync.marker(me, sync.phase), own));
        }
        let phase = sync.phase;
        let fresh = |end: u32| f64::from(end) + END_STALE * 1000. >= f64::from(own);
        let waiting = (0..rt.gone.len()).any(|p| p != me && !rt.gone[p] && !sync.ends.get(&(p, sync.marker(p, phase))).is_some_and(|&e| fresh(e)));
        let max = if sync.max > 0. { sync.max } else { END_HOLD_MAX };
        if waiting && now - since < max {
            until = at - 1e-6;
        }
    } else if rt.state == running && sync.holding.is_some() {
        // End review I3: the end moved later while held (a race's gate bonus, a judge's Sync): play on; the next end
        // relays a fresh marker.
        sync.rearm();
    }
    advance(g, rt, until);
    out.or(sync.observe(rt.state == running, me, rt.now))
}

/// Delivers everything due by `now`, in order, then sets the clock to `now`.
pub(crate) fn advance<G: Graph>(g: &mut G, rt: &mut Rt<G::State>, now: f64) {
    for _ in 0..10_000 {
        let Some(msg) = rt.pop(now) else { break };
        dispatch(g, rt, msg);
    }
    rt.now = rt.now.max(now);
    rt.switch_camera();
}

/// Delivers the messages posted for now ([`advance`] delivers them first) and nothing due later: what a frame's relays
/// posted stays ahead of what the next frame dispatches at once (a leave).
pub(crate) fn deliver<G: Graph>(g: &mut G, rt: &mut Rt<G::State>) {
    for _ in 0..10_000 {
        let Some(msg) = rt.posted.pop_front() else { break };
        dispatch(g, rt, msg);
    }
}

/// Starts a graph in `initial` (its stateenter runs).
pub(crate) fn start<G: Graph>(g: &mut G, rt: &mut Rt<G::State>) {
    rt.note(format!("enter {}", G::name(rt.state)));
    if g.on(rt, &Msg::Enter).is_err() {
        rt.note("error");
    }
    settle(g, rt);
}
/// Runs queued state changes (stateexit, stateenter) until none are left.
pub(crate) fn settle<G: Graph>(g: &mut G, rt: &mut Rt<G::State>) {
    while let Some(next) = rt.queue.pop_front() {
        if g.on(rt, &Msg::Exit).is_err() {
            rt.note("error");
        }
        rt.state = next;
        rt.note(format!("enter {}", G::name(next)));
        if g.on(rt, &Msg::Enter).is_err() {
            rt.note("error");
        }
    }
}

/// Lua 5.1's table.sort (ltablib.c auxsort, the quicksort the disc's Lua runs): `less` is the comparator. The order it
/// leaves ties in is part of the result (PlayerStandings), so it is ported as is.
pub(crate) fn lua_sort<T: Clone>(a: &mut [T], less: impl Fn(&T, &T) -> bool) {
    fn aux<T: Clone>(a: &mut [T], mut l: usize, mut u: usize, less: &impl Fn(&T, &T) -> bool) {
        // 1-based l..u as in the C.
        let g = |a: &[T], i: usize| a[i - 1].clone();
        while l < u {
            if less(&g(a, u), &g(a, l)) {
                a.swap(l - 1, u - 1);
            }
            if u - l == 1 {
                break;
            }
            let mut i = (l + u) / 2;
            if less(&g(a, i), &g(a, l)) {
                a.swap(i - 1, l - 1);
            } else if less(&g(a, u), &g(a, i)) {
                a.swap(i - 1, u - 1);
            }
            if u - l == 2 {
                break;
            }
            let p = g(a, i);
            a.swap(i - 1, u - 2);
            i = l;
            let mut j = u - 1;
            loop {
                i += 1;
                while less(&g(a, i), &p) {
                    i += 1;
                }
                j -= 1;
                while less(&p, &g(a, j)) {
                    j -= 1;
                }
                if j < i {
                    break;
                }
                a.swap(i - 1, j - 1);
            }
            a.swap(u - 2, i - 1);
            let (lo, hi);
            if i - l < u - i {
                (lo, hi) = (l, i - 1);
                l = i + 1;
            } else {
                (lo, hi) = (i + 1, u);
                u = i - 1;
            }
            aux(a, lo, hi, less);
        }
    }
    let n = a.len();
    if n > 1 {
        aux(a, 1, n, &less);
    }
}

// ---------------------------------------------------------------- HUD2's state
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Text {
    pub id: String,
    pub params: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Message {
    pub seq: u32,
    pub text: Text,
    pub colour: u8,
    pub at: f64,
}
/// HUD2.eHUDComponent, as far as the ported graphs show and hide them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum Component {
    LeaderboardHUD,
    DPOHUD,
    CountdownHUD,
    TimerHUD,
    TrickDisplayHUD,
    MiniMap,
    HOMScoringHUD,
    OwnedItHUD,
    KilledItHUD,
    ObjectiveHUD,
    ABDHUD,
}
/// HUD2.eMessageColour (0x8284dc78).
pub(crate) const WHITE: u8 = 0;
pub(crate) const RED: u8 = 1;
pub(crate) const GREEN: u8 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Board {
    /// HUD2.eLeaderboardHUDMenuType (0x8284df88): ThrowDown 1, Skate 7, ...
    pub layout: i64,
    pub ranks: Vec<i64>,
    pub ints: Vec<i64>,
    pub visible: bool,
    pub variant: Option<String>,
    /// Each player's text cell, as the native race stats fill it (RankNameTime's time); empty: the int, grouped.
    pub texts: Vec<String>,
    /// LeaderboardHUDInit's teams: each player's team (1 top, 2 bottom; empty: no teams), the teams' names, and
    /// LeaderboardHUDSetTeamRank / SetTeamScore / SetTeamHighlight per team.
    pub teams: Vec<u8>,
    pub team_names: [String; 2],
    pub team_ranks: [i64; 2],
    pub team_scores: [i64; 2],
    pub team_highlight: [bool; 2],
    /// Each team's score cell as a mode sets it (a contest's float round: distances); empty: the score, grouped.
    pub team_texts: [String; 2],
    /// LeaderboardHUDSetShowPlayers(false): the setup block's last item (show players) false.
    pub hide_players: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dpo {
    pub visible: bool,
    /// HUD2.eDPOFormat (0x8284ee90): Ints 0, Points 1, Time 2, Distance 3, Players 4.
    pub format: i64,
    pub title: String,
    pub a: i64,
    pub b: i64,
    /// HUD2.DPOUseSecondDPO: the second DPO (DPO_IsUsingTwoDPOs) and its numbers and title.
    pub two: bool,
    pub title2: String,
    pub a2: i64,
    pub b2: i64,
    /// HUD2.DPOSetTeamColour: 0 red, 1 blue.
    pub team: Option<i64>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Hud2 {
    seq: u32,
    pub central: Option<(Message, u8)>,
    pub modified: Option<(Message, bool)>,
    pub cleared: u32,
    /// (seq, hudthrowmes state, the native letter sound)
    pub letters: Option<(u32, i64, Option<&'static str>)>,
    pub turn: Option<usize>,
    /// LeaderboardSetTurn(.., enable, false)'s icons, per player.
    pub turns: Vec<bool>,
    /// ShowTeamCentralMessage: the message, the team (0 red, 1 blue) and eTeamMessageType.
    pub team_central: Option<(Message, i64, u8)>,
    pub board: Board,
    pub dpo: Dpo,
    /// The countdown timer's end.
    pub countdown: Option<f64>,
    /// The TimerHUD's timer and its end.
    pub timer: Option<(u32, f64)>,
    /// Results_SetPlayerData calls: (player, rank, value).
    pub results: Vec<(usize, i64, i64)>,
    /// The voting HUD (VotingManager: up for the others while a run is voted on) and this player's vote.
    pub vote: Option<u8>,
    /// HUD2.Show(TrickDisplayHUD, false) is in force (match_core::Match::trick_display_hidden).
    pub trick_hidden: bool,
    /// Online: a round sync point waits for the other consoles ("Waiting for Skaters").
    pub waiting: bool,
    /// HUD2.Show(ObjectiveHUD) and its line (Objectives.ChangeObjectiveText).
    pub objective: bool,
    pub objective_text: String,
    /// HUD2.Show(OwnedItHUD / KilledItHUD, true): since when.
    pub owned_it: Option<f64>,
    pub killed_it: Option<f64>,
}

// ---------------------------------------------------------------- the HUD movies
/// The results screen's setup (Results_SetResultsMenuType, the column titles, the throwdown column).
pub(crate) struct ResultsSetup {
    pub title: String,
    /// FrontEnd.eResultsLBType (endchallenge_online: PlaceNameInt 0, PlaceNameScoreTime 6, ...).
    pub layout: i64,
    pub columns: Vec<String>,
    /// endchallenge_online's eThrowDownType: None 0, OneUp 1, Skate 2.
    pub throwdown: i64,
    /// Online.SetPlayersChoice's player: its row's choice flag (PlaceNameIntChoice draws the icon in column 3).
    pub choice: Option<usize>,
    /// Per player the time cell (DeathRace, PlaceNameScoreTime), resolved text.
    pub times: Vec<(usize, String)>,
    /// Per player the float cell (BestTrick*Float) and its HUDComponents.NumberToDistance text.
    pub floats: Vec<(usize, f64, String)>,
    /// Per player the string cell (PlaceNameString*: Own the Spot's status), resolved text.
    pub strings: Vec<(usize, String)>,
}

/// The results sting retail's sub_825F1F88 plays as the standings come up: a win for first place, a loss otherwise,
/// the plain leaderboard sting for a player with no row (inferred from its fe records; the trace stops at the call).
fn results_sound(results: &[(usize, i64, i64)], me: usize) -> &'static str {
    match results.iter().find(|r| r.0 == me).map(|r| r.1) {
        Some(1) => "contest_win",
        Some(_) => "contest_lose",
        None => "contest_leaderboard",
    }
}

/// Who is looking: the players' names (by player id), this player, online or hot-seat.
pub(crate) struct View<'a> {
    pub names: &'a [String],
    pub me: usize,
    pub online: bool,
    pub title: String,
    pub label: &'static str,
}

const BOARD: &str = "3-s3m-board";
const DPO: &str = "3-s3m-dpo";
const TIMER: &str = "4-s3m-timer";
const COUNTDOWN: &str = "4-s3m-countdown";
const LETTERS: &str = "5-s3m-throwmes";
const CENTRAL: &str = "6-s3m-central";
const RESULTS: &str = "7-s3m-results";
const VOTING: &str = "2-s3m-voting";
const WAITING: &str = "6-s3m-waiting";
const OBJECTIVE: &str = "3-s3m-objective";
const OWNED_IT: &str = "5-s3m-ownedit";
const KILLED_IT: &str = "5-s3m-killedit";
/// HUD2's central message (0x82664550: 180 updates, then SetState 5) and a modified one not set to stay (SetState 11).
/// ponytail: read as 3 s at 60 updates a second, as skate3_skate_game's CENTRAL_FOR.
pub(crate) const CENTRAL_FOR: f64 = 3.;
/// The team message (0x82664550: +496 counts 120 updates, then SetState 14). ponytail: 2 s at 60 updates a second.
const TEAM_FOR: f64 = 2.;

fn set_state(screen: &mut Screen, state: i64) -> Result<(), String> {
    screen.call("SetState", &[json!(state)]).map(drop)
}

/// Draws [`Hud2`] with the shared movies.
#[derive(Default)]
pub(crate) struct HudDriver {
    failed: BTreeSet<&'static str>,
    central: Option<(u32, bool)>,
    modified: Option<(u32, bool)>,
    team_central: Option<(u32, bool)>,
    cleared: u32,
    letters: Option<u32>,
    board: Option<Vec<(&'static str, Json)>>,
    dpo: Option<(bool, Vec<Json>)>,
    countdown: Option<(f64, i64)>,
    timer: Option<f64>,
    results: bool,
    /// The results screen's status timer (whole seconds of the core's Results left), once online Results begin.
    status: Option<i64>,
    /// "Waiting for Skaters" is up.
    waiting: bool,
    vote: Option<u8>,
    /// The objective HUD's line while shown; the Owned It / Killed It treatments' starts.
    objective: Option<String>,
    owned_it: Option<f64>,
    killed_it: Option<f64>,
    /// Team challenges: endchallenge_teams' Mission.GetTeamResultsDataBlock ([`team_block`]), set by the mode before
    /// each drive. Item 6: with the movie installed ([`TEAMS_MOVIE`], screen 35 EndChallengeOptions_TeamChallenge)
    /// it replaces endchallenge_online's TeamChallengeResults table.
    pub teams: Option<Json>,
    /// The open results screen is endchallenge_teams.
    team_screen: bool,
}

/// The online-movies stage's endchallenge_teams (disc data/fe/source/screens/hudComponents/endchallenge_teams).
pub(crate) const TEAMS_MOVIE: &str = "private/online-movies/runtime/endchallenge_teams.json";

/// One team of Mission.GetTeamResultsDataBlock (0x825BD5A8, 5 items per team from the FinalResults group's row k):
/// its name, rank (row +0), score text (row +4, or the float +12 under TeamChallengeFloat), status text (row +8, the
/// 7th Results_SetPlayerData argument) and members (0x825BD398: [rank, name, score, column 3] per row).
pub(crate) struct TeamTable {
    pub name: String,
    pub rank: i64,
    pub score: String,
    pub status: String,
    pub members: Vec<[String; 4]>,
}

/// Mission.GetTeamResultsDataBlock (0x825BD5A8, 17 items: li r4,17 at 0x825BD7F0): the header [online (0x830B7C2B),
/// throwdown (group +2456), title (0x825BC1E8), IsFinalResults (results +7548), this player's team 0 / 1 (slot +112),
/// its row in that team (0x825BD0C8, -1 none), layout (group +2452)], then per team [`TeamTable`]'s five. Team
/// challenges have no throwdown column (contest and race set none).
pub(crate) fn team_block(online: bool, title: &str, final_results: bool, (team, row): (usize, i64), layout: i64, teams: [TeamTable; 2]) -> Json {
    let mut out = vec![json!(online), json!(0), json!(title), json!(final_results), json!(team), json!(row), json!(layout)];
    for t in teams {
        out.extend([json!(format!("#{}", t.name)), json!(t.rank), json!(t.score), json!(t.status), json!(t.members)]);
    }
    Json::Array(out)
}

/// A results cell's whole number (0x825BCB88): grouped through 0x8290E178, ID_RACE_DNF under 0.
pub(crate) fn team_int(data: &ModeData, n: i64) -> String {
    if n < 0 { format!("#{}", data.text("ID_RACE_DNF")) } else { format!("#{}", thousands(n as f64)) }
}

/// endchallenge_teams' answers: the data block, the status line and FELeaderboard.EndChallengeTeams_GetLeaderboardNavData
/// (0x825AF2E8: [rows left, rows right, selected column, selected row, this player's team, its rank]). ponytail: the
/// selection's start (FE nav +136 / +132) is not traced; it sits on this player's own row. No team logos:
/// Game.ImageRenderer_LoadTeamLogo's EA team art is gone (the frame stays empty).
pub(crate) fn team_answers(data: &ModeData, block: &Json, status: Option<i64>) -> Vec<(String, Json)> {
    let rows = |k: usize| block[7 + 5 * k + 4].as_array().map_or(0, Vec::len);
    let (team, row) = (block[4].as_i64().unwrap_or(0), block[5].as_i64().unwrap_or(-1));
    let (text, timer) = results_status(&data.text(RETURN_TO_LOBBY), status);
    vec![
        ("Mission.GetTeamResultsDataBlock".to_owned(), block.clone()),
        ("Mission.GetResultsStatusText".into(), text),
        ("Mission.GetResultsStatusTimer".into(), timer),
        ("FELeaderboard.EndChallengeTeams_GetLeaderboardNavData".into(), json!([rows(0), rows(1), team, row.max(0), team, row + 1])),
        ("Game.ImageRenderer_LoadTeamLogo".into(), Json::Null),
        // Steam has no gamercard / profile to open: no platform matches (-1; null would equal the undefined
        // Platform_PS3 / Platform_XBOX360), so no A button (B Quit stays).
        ("global.GetPlatform".into(), json!(-1)),
        // ponytail: the Killed It camera shake is not ported; its on / off calls do nothing.
        ("Mission.SetKilledItShakeActivation".into(), Json::Null),
    ]
}

/// endchallenge_teams' status line: SetState 100 (UpdateStatus) as the seconds go down. Its Initialize sets the line
/// up (SetupStatus) only when the screen opens; Results that begin after it opened run Initialize again.
fn team_status(screen: &mut Screen, data: &ModeData, before: Option<i64>, left: Option<i64>) -> Result<(), String> {
    let Some(left) = left.filter(|l| before != Some(*l)) else { return Ok(()) };
    let (text, timer) = results_status(&data.text(RETURN_TO_LOBBY), Some(left));
    screen.answer("Mission.GetResultsStatusText", text);
    screen.answer("Mission.GetResultsStatusTimer", timer);
    if before.is_none() { screen.call("Initialize", &[]).map(drop) } else { set_state(screen, 100) }
}

impl HudDriver {
    pub fn drive(&mut self, world: &mut World, data: &ModeData, hud: &Hud2, view: &View, results: Option<&ResultsSetup>, now: f64) {
        let status = online_results(world, self.results);
        let ui = world.resource::<Config>().asset_root.join("private/online-ui/runtime");
        let dir = world.resource::<Config>().asset_root.join(HUD_DIR);
        let teams_file = world.resource::<Config>().asset_root.join(TEAMS_MOVIE);
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
        let layer = screens.layer(ONLINE);
        let movie = |name: &str| dir.join(format!("{name}.json"));

        // The online-ui leaderboardhud (LeaderboardHUDInit's menu type; SetRank / SetInt / SetTurn per player).
        if hud.board.visible {
            let answers = board(data, hud, view);
            if self.board.as_ref() != Some(&answers) {
                let fresh_setup = self.board.as_ref().is_none_or(|b| b[0] != answers[0] || b[1] != answers[1]);
                self.board = Some(answers.clone());
                self.with(layer, &ui.join("leaderboardhud.json"), BOARD, view.label, |screen, fresh| {
                    for (k, v) in answers {
                        screen.answer(k, v);
                    }
                    // 3 SetupScreen (layout, title, RefreshAll), 1 show, 5 RefreshAll.
                    if fresh || fresh_setup {
                        set_state(screen, 3)?;
                    }
                    set_state(screen, if fresh { 1 } else { 5 })
                });
            }
        } else if self.board.take().is_some() {
            layer.close(BOARD);
        }

        // The DPO: SetState 1 SetupDPO + show, 3 Update, 2 hide (dpo's SetState).
        let params = dpo_params(data, &hud.dpo);
        let want = (hud.dpo.visible, params.clone());
        if self.dpo.as_ref() != Some(&want) {
            let before = self.dpo.replace(want);
            if hud.dpo.visible || layer.open.contains_key(DPO) {
                let shown = before.as_ref().is_some_and(|b| b.0);
                self.with(layer, &movie("dpo"), DPO, view.label, |screen, _| {
                    screen.answer("HUDComponents.DPO_GetParams", Json::Array(params));
                    screen.answer("HUDComponents.DPO_IsUsingTwoDPOs", json!(hud.dpo.two));
                    match (shown, hud.dpo.visible) {
                        (false, true) => set_state(screen, 1).and_then(|_| set_state(screen, 3)),
                        (true, true) => set_state(screen, 3),
                        (_, false) => set_state(screen, 2),
                    }
                });
            }
        }

        // The countdown: "#3".."#1", then "#0" (the movie's Go! frame), SetState 2 at each; Show(false) is SetState 1.
        let count = hud.countdown.map(|end| (end, (end - now).max(0.).ceil() as i64));
        if count != self.countdown {
            let before = std::mem::replace(&mut self.countdown, count);
            let show = match (before, count) {
                (Some((was, shown)), Some((end, left))) if was == end => left != shown,
                (_, Some(_)) => true,
                _ => false,
            };
            if show || (count.is_none() && layer.open.contains_key(COUNTDOWN)) {
                self.with(layer, &movie("hudcountdown"), COUNTDOWN, view.label, |screen, _| {
                    screen.answer("HUDComponents.Countdown_IsInStadium", json!(false));
                    screen.answer("FELanguage.GetLanguage", json!("English"));
                    match count {
                        Some((_, left)) => {
                            screen.answer("HUDComponents.Countdown_GetCountdownTime", json!(format!("#{left}")));
                            set_state(screen, 2)
                        }
                        None => set_state(screen, 1),
                    }
                });
            }
        }

        // The TimerHUD.
        let timer = hud.timer.map(|(_, end)| (end - now).max(0.));
        if timer != self.timer {
            let before = std::mem::replace(&mut self.timer, timer);
            if timer.is_some() || layer.open.contains_key(TIMER) {
                self.with(layer, &movie("hudtimer"), TIMER, view.label, |screen, _| timer_states(screen, before, timer));
            }
        }

        // hudthrowmes: a fresh movie per letter, SetState(n).
        let letters = hud.letters.map(|l| l.0);
        if letters != self.letters {
            self.letters = letters;
            layer.close(LETTERS);
            if let Some((_, state, sound)) = hud.letters {
                self.with(layer, &movie("hudthrowmes"), LETTERS, view.label, |screen, _| {
                    screen.bindings.sounds.extend(sound.map(String::from));
                    set_state(screen, state)
                });
            }
        }

        // hudcentral: 4 central (5 after 3 s), 10 modified (11 after 3 s unless it stays), 88 clear; the team message
        // 13 / 15 / 16 by type (14 after 2 s).
        let mut state = (self.central, self.modified, self.cleared, self.team_central);
        self.with(layer, &movie("hudcentral"), CENTRAL, view.label, |screen, fresh| {
            if fresh {
                set_state(screen, 1)?;
            }
            if hud.cleared != state.2 {
                state.2 = hud.cleared;
                set_state(screen, 88)?;
                state.1 = None;
            }
            if let Some((m, hook)) = &hud.central {
                if state.0.is_none_or(|s| s.0 != m.seq) {
                    screen.answer("HUDComponents.Central_GetMessageData", json!([data.fmt(&named(&m.text, view)), m.colour]));
                    screen.answer("HUDComponents.Central_GetAudioHook", json!(hook));
                    set_state(screen, 4)?;
                    state.0 = Some((m.seq, false));
                } else if state.0.is_some_and(|s| !s.1) && now >= m.at + CENTRAL_FOR {
                    set_state(screen, 5)?;
                    state.0 = Some((m.seq, true));
                }
            }
            if let Some((m, stay)) = &hud.modified {
                if state.1.is_none_or(|s| s.0 != m.seq) {
                    screen.answer("HUDComponents.Central_GetModifiedMessageData", json!([data.fmt(&named(&m.text, view)), m.colour]));
                    set_state(screen, 10)?;
                    state.1 = Some((m.seq, false));
                } else if !stay && state.1.is_some_and(|s| !s.1) && now >= m.at + CENTRAL_FOR {
                    set_state(screen, 11)?;
                    state.1 = Some((m.seq, true));
                }
            }
            // Central_GetTeamMessageData [text, team]; the movie's ShowTeamMessage / ShowTeamCelebratory colour it by
            // team (kTEAM_RED 0, kTEAM_BLUE 1). ponytail: no team logo (LoadTeamLogoTexture's EA Online team art is gone).
            if let Some((m, team, kind)) = &hud.team_central {
                if state.3.is_none_or(|s| s.0 != m.seq) {
                    screen.answer("HUDComponents.Central_GetTeamMessageData", json!([data.fmt(&named(&m.text, view)), team]));
                    set_state(screen, match kind {
                        0 => 13,
                        1 => 15,
                        _ => 16,
                    })?;
                    state.3 = Some((m.seq, false));
                } else if state.3.is_some_and(|s| !s.1) && now >= m.at + TEAM_FOR {
                    set_state(screen, 14)?;
                    state.3 = Some((m.seq, true));
                }
            }
            Ok(())
        });
        (self.central, self.modified, self.cleared, self.team_central) = state;

        // "Waiting for Skaters" (ID_ONLINE_CHALLENGE_WAITING_FOR_OTHER_SKATERS) while a round sync point waits: a
        // hudcentral of its own, its modified message (SetState 10) until the others are there (11).
        // ponytail: the retail widget that shows this string is not traced; hudcentral's staying message carries it.
        if hud.waiting != self.waiting {
            self.waiting = hud.waiting;
            let text = data.text("ID_ONLINE_CHALLENGE_WAITING_FOR_OTHER_SKATERS");
            if hud.waiting || layer.open.contains_key(WAITING) {
                self.with(layer, &movie("hudcentral"), WAITING, view.label, |screen, fresh| {
                    if fresh {
                        set_state(screen, 1)?;
                    }
                    if hud.waiting {
                        screen.answer("HUDComponents.Central_GetModifiedMessageData", json!([text, WHITE]));
                    }
                    set_state(screen, if hud.waiting { 10 } else { 11 })
                });
            }
        }

        // hudonlinevoting: SetState 1 shows it (Init: the OK vote), 3-6 the vote (weak / ok / sweet / gnarly), 2 hides.
        if hud.vote != self.vote {
            let before = std::mem::replace(&mut self.vote, hud.vote);
            if hud.vote.is_some() || layer.open.contains_key(VOTING) {
                self.with(layer, &movie("hudonlinevoting"), VOTING, view.label, |screen, _| match (before, hud.vote) {
                    (None, Some(v)) => set_state(screen, 1).and_then(|_| set_state(screen, 3 + i64::from(v))),
                    (Some(_), Some(v)) => set_state(screen, 3 + i64::from(v)),
                    (_, None) => set_state(screen, 2),
                });
            }
        }

        // hudobjective2 (HUD2's ObjectiveHUD): SetState 1 the intro, 3 the line (twice, as skate2_screens: a row's first
        // jump to "fraction" swaps in a new label), 2 hides. The one objective is Objectives.ChangeObjectiveText's line.
        // ponytail: HUD2.SetupObjectiveHUDData's natives are not traced; Skate 2's answers for one live objective.
        let line = hud.objective.then(|| data.text(&hud.objective_text));
        if line != self.objective {
            let before = std::mem::replace(&mut self.objective, line.clone());
            if line.is_some() || layer.open.contains_key(OBJECTIVE) {
                self.with(layer, &movie("hudobjective2"), OBJECTIVE, view.label, |screen, fresh| {
                    let Some(line) = line else { return set_state(screen, 2) };
                    for (k, v) in [
                        ("HUDComponents.Objective_GetNumObjectives", json!(1)),
                        ("HUDComponents.Objective_GetObjectiveTitle(0)", json!(line)),
                        ("HUDComponents.Objective_GetObjectiveState(0)", json!(2)),
                        ("HUDComponents.Objective_GetObjectiveType(0)", json!(1)),
                        ("HUDComponents.Objective_GetObjectiveValue(0)", json!("")),
                        ("HUDComponents.Objective_GetActivityTitle", json!(view.title)),
                        ("HUDComponents.Objective_GetActivityType", json!("")),
                    ] {
                        screen.answer(k, v);
                    }
                    if fresh || before.is_none() {
                        set_state(screen, 1)?;
                        set_state(screen, 3)?;
                    }
                    set_state(screen, 3)
                });
            }
        }
        // The Owned It / Killed It treatments (ownthespot / killedit: SetState 1 plays it); hidden, the movie closes.
        // ponytail: what HUD2.Show(.., false) asks of these movies is not traced; closing ends the treatment.
        for (want, shown, name, file) in [(hud.owned_it, &mut self.owned_it, OWNED_IT, "ownthespot"), (hud.killed_it, &mut self.killed_it, KILLED_IT, "killedit")] {
            if want != *shown {
                *shown = want;
                layer.close(name);
                if want.is_some() && !self.failed.contains(name) {
                    match Screen::load(&movie(file), &[]).and_then(|mut screen| set_state(&mut screen, 1).map(|_| screen)) {
                        Ok(screen) => drop(layer.open.insert(name.to_owned(), screen)),
                        Err(e) => {
                            warn!("Skate 3 {}: {file}: {e}", view.label);
                            self.failed.insert(name);
                        }
                    }
                }
            }
        }

        if let Some(setup) = results.filter(|_| !self.results) {
            self.results = true;
            self.status = status;
            // Team challenges: endchallenge_teams when the stage installed it, else the TeamChallengeResults table.
            let block = self.teams.as_ref().filter(|_| teams_file.is_file());
            self.team_screen = block.is_some();
            let (file, answers) = match block {
                Some(block) => (teams_file, team_answers(data, block, status)),
                None => (movie("endchallenge_online"), results_answers(data, hud, view, setup, status)),
            };
            let refs: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            if !self.failed.contains(RESULTS) {
                match Screen::load(&file, &refs) {
                    Ok(mut screen) => {
                        // the movie itself plays no sound: the sting goes out through its layer's front-end sounds
                        screen.bindings.sounds.push(results_sound(&hud.results, view.me).into());
                        layer.open.insert(RESULTS.to_owned(), screen);
                    }
                    Err(e) => {
                        warn!("Skate 3 {}: endchallenge_online: {e}", view.label);
                        self.failed.insert(RESULTS);
                    }
                }
            }
        } else if self.results && status != self.status {
            // SetState 6 (SetEndOptions: the status line set up) as the Results begin, then 100 (UpdateStatus).
            let before = std::mem::replace(&mut self.status, status);
            if let Some(screen) = layer.open.get_mut(RESULTS)
                && let Err(e) = if self.team_screen {
                    team_status(screen, data, before, status)
                } else {
                    status_states(screen, &data.text(RETURN_TO_LOBBY), before, status)
                }
            {
                warn!("Skate 3 {}: endchallenge_online: {e}", view.label);
            }
        }
    }

    /// The results screen down, to show the next one (a contest's rounds, then its standings).
    pub fn reset_results(&mut self, world: &mut World) {
        self.results = false;
        self.status = None;
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            screens.layer(ONLINE).close(RESULTS);
        }
    }

    pub fn close(world: &mut World) {
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            let layer = screens.layer(ONLINE);
            for name in [BOARD, DPO, TIMER, COUNTDOWN, LETTERS, CENTRAL, RESULTS, VOTING, WAITING, OBJECTIVE, OWNED_IT, KILLED_IT] {
                layer.close(name);
            }
        }
    }

    fn with(&mut self, layer: &mut Layer, path: &Path, name: &'static str, label: &str, f: impl FnOnce(&mut Screen, bool) -> Result<(), String>) {
        if self.failed.contains(name) {
            return;
        }
        let fresh = !layer.open.contains_key(name);
        let result = (|| {
            if fresh {
                layer.open.insert(name.to_owned(), Screen::load(path, &[])?);
            }
            f(layer.open.get_mut(name).expect("opened above"), fresh)
        })();
        if let Err(e) = result {
            warn!("Skate 3 {label}: {}: {e}", path.display());
            layer.close(name);
            self.failed.insert(name);
        }
    }
}

/// A message's parameters with the graphs' player names (`P<id>`, Online.GetSkaterName as the ports and their oracle
/// name players) replaced by the players' names.
fn named(text: &Text, view: &View) -> Text {
    let name = |p: &String| p.strip_prefix('P').and_then(|n| n.parse::<usize>().ok()).and_then(|n| view.names.get(n)).cloned();
    Text { id: text.id.clone(), params: text.params.iter().map(|p| name(p).unwrap_or_else(|| p.clone())).collect() }
}

/// The TimerHUD from one view to the next (skate2_screens' timer_acts: the same hudtimer code).
pub(crate) fn timer_states(screen: &mut Screen, before: Option<f64>, now: Option<f64>) -> Result<(), String> {
    crate::skate3_skate_game::timer_states(screen, before, now)
}

/// DPO_GetParams: [numerator, denominator, title, "", highlight] for the primary DPO (kStruct_Items 5) and an empty
/// secondary. The numerator is the int grouped (0x8290e178; the movie adds its '#'); the Points denominator is
/// ID_DPO_STR_POINTS with b (0x8266af68), Ints' is ID_COMMON_NUMBER.
/// ponytail: the highlight is idle (any value but 0 and 2..7); the natives that set it are not traced.
fn dpo_params(data: &ModeData, dpo: &Dpo) -> Vec<Json> {
    let denominator = match dpo.format {
        1 => data.fmt(&Text { id: "ID_DPO_STR_POINTS".into(), params: vec![dpo.b.to_string()] }),
        _ => thousands(dpo.b as f64),
    };
    let text = |t: &String| if t.starts_with('#') { t.clone() } else { data.text(t) };
    let (title, title2) = (text(&dpo.title), text(&dpo.title2));
    // The second DPO (HUD2.DPOUseSecondDPO): its own numbers, as Ints.
    let (a2, b2) = if dpo.two { (json!(thousands(dpo.a2 as f64)), json!(thousands(dpo.b2 as f64))) } else { (json!(""), json!("")) };
    // DPOSetTeamColour's highlight (0x8266aed0): 4 red, 5 blue (the movie's SetHighlight "red" / "blue").
    let highlight = dpo.team.map_or(1, |t| 4 + t);
    vec![json!(thousands(dpo.a as f64)), json!(denominator), json!(title), json!(""), json!(highlight), a2, b2, json!(title2), json!(""), json!(highlight)]
}

/// The online-ui leaderboardhud: the setup block [players, top team's players, layout, variant, teams, online, show
/// players], the names, and per player [rank, int, text, highlight, turn, team, row, sort id]. With teams the rows
/// are the top team's then the bottom's (the movie's mNumPlayersTop rows first), the names end with the two team
/// names, and the block with [score text, rank, highlight] per team (lower rank wins: redwinning / bluewinning).
fn board(data: &ModeData, hud: &Hud2, view: &View) -> Vec<(&'static str, Json)> {
    let n = view.names.len();
    let b = &hud.board;
    let teamed = b.teams.len() == n && n > 0;
    let team = |p: usize| if teamed { b.teams[p] } else { 0 };
    // Rows: by team (stable), else by player.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&p| team(p));
    let top = if teamed { b.teams.iter().filter(|&&t| t == 1).count() } else { 0 };
    // Item 3 is the variant line (lb3 SetupScreen: shown unless "ID_COMMON_UNKNOWN" / "#Unknown" or a throwdown).
    let variant = b.variant.as_ref().map_or_else(|| "ID_COMMON_UNKNOWN".to_owned(), |v| data.text(v));
    let setup = json!([n, top, b.layout, variant, teamed, view.online, !b.hide_players]);
    let mut block: Vec<Json> = (order.iter().enumerate())
        .flat_map(|(row, &p)| {
            let highlight = view.online && p == view.me;
            let rank = b.ranks.get(p).copied().unwrap_or(0);
            let int = b.ints.get(p).copied().unwrap_or(0);
            // ponytail: Leaderboard_GetLeaderboardDataBlock's text cell (non-throwdown layouts draw it as the score) is
            // the native's; not traced. The int, grouped, unless the mode set a text (the race's times).
            let text = b.texts.get(p).filter(|t| !t.is_empty()).cloned().unwrap_or_else(|| format!("#{}", thousands(int as f64)));
            let turn = hud.turn == Some(p) || hud.turns.get(p).copied().unwrap_or(false);
            [json!(rank), json!(int), json!(text), json!(highlight), json!(turn), json!(team(p)), json!(row), json!(p)]
        })
        .collect();
    let mut names: Vec<String> = order.iter().map(|&p| format!("#{}", view.names[p])).collect();
    if teamed {
        for t in 0..2 {
            let text = Some(&b.team_texts[t]).filter(|t| !t.is_empty()).cloned().unwrap_or_else(|| format!("#{}", thousands(b.team_scores[t] as f64)));
            block.extend([json!(text), json!(b.team_ranks[t]), json!(b.team_highlight[t])]);
            names.push(format!("#{}", b.team_names[t]));
        }
    }
    vec![
        ("HUDComponents.Leaderboard_GetSetupDataBlock", setup),
        ("HUDComponents.Leaderboard_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Leaderboard_GetLeaderboardDataBlock", Json::Array(block)),
        ("Mission.GetTitle", json!(view.title)),
        // UpdateVOIPIcons' GetVoipStatus()[p] (RefreshAll calls it): voice::voip_status by the player's name, online.
        ("FELeaderboard.GetVoipStatus", json!((0..n).map(|p| if view.online { crate::voice::voip_named(&view.names[p]) } else { 0 }).collect::<Vec<_>>())),
    ]
}

/// `Match::gestures` for a Skate 3 mode (G9, online audit 10-28; user 10-17 #44 / #45: emotes go through under a match
/// hold): while it runs (the core's Running), its `hold` is its own intro, countdown or turn wait, so the D-pad emotes
/// pass; not once its results screen (`results`) owns the pad. The core's own phases keep the core's rule. UNRECOVERED:
/// whether retail's PinAI lets emotes through.
pub(crate) fn gestures(m: &crate::match_core::Match, results: bool) -> bool {
    m.hold && !results && matches!(m.phase, crate::match_core::Phase::Running { .. })
}

/// endchallenge_online's answers: per Results_SetPlayerData row [rank, int, float, string, time, choice, row, name]
/// in rank order; the int reaches a throwdown column as a frame label through `substring(1)`.
/// Online, during the core's Results: the whole seconds left (endchallenge_online's status timer). B (the screen's
/// ID_COMMON_QUIT) on an open results screen is retail's Quit (s3re 10-28: input 0x825F1D20 code 3 -> popup 63, whose
/// Yes posts event 18, leave the session): online_menu's Quit Session question. When retail enables the button
/// (R+0x80) is UNRECOVERED; it is always on here.
// The whole seconds, rounded up ([`results_status`] formats them as the native does). The screen's A (View
// Gamercard / Profile) has no profile to open here and does nothing.
pub(crate) fn online_results(world: &mut World, results_open: bool) -> Option<i64> {
    let real = world.resource::<Time<Real>>().elapsed_secs();
    let m = world.resource::<crate::match_core::Match>();
    let left = m.results_left(real).filter(|_| m.token != 0)?;
    let quit = world.get_resource::<crate::customiser::Navigation>().is_some_and(|n| n.pressed & 0x2000 != 0);
    // An open popup, or the B that just closed it, is the popup's press.
    if results_open
        && quit
        && let Some(mut ui) = world.get_resource_mut::<crate::online_menu::OnlineUi>()
        && !ui.modal
        && !ui.closed_this_frame
    {
        ui.requests.push(crate::online_menu::OnlineRequest::QuitSession);
    }
    Some(left.ceil() as i64)
}

/// GetResultsStatusText's online default (s3re 10-28: final results +0x1D7C, "Returning to Lobby"; the only installed
/// string with that text). Not "Returning to Freeskate".
pub(crate) const RETURN_TO_LOBBY: &str = "ID_ONLINE_RESULTS_SCREEN_MESSAGE_RETURN_TO_LOBBY";

/// Mission.GetResultsStatusText / GetResultsStatusTimer: online, during the core's Results, "Returning to Lobby"
/// ([`RETURN_TO_LOBBY`], `back`) and the time left as GetResultsStatusTimer (0x825BEA80) formats it, "#:%02d" minutes
/// and seconds ("#0:10"); otherwise no status.
pub(crate) fn results_status(back: &str, left: Option<i64>) -> (Json, Json) {
    match left {
        Some(left) => (json!(back), json!(format!("#{}:{:02}", left / 60, left % 60))),
        None => (json!(""), json!(0)),
    }
}

/// endchallenge_online's status line from one frame to the next: SetState 6 (SetEndOptions sets it up) as the
/// Results begin, then 100 (UpdateStatus) as the seconds go down.
pub(crate) fn status_states(screen: &mut Screen, back: &str, before: Option<i64>, left: Option<i64>) -> Result<(), String> {
    let Some(left) = left.filter(|l| before != Some(*l)) else { return Ok(()) };
    let (text, timer) = results_status(back, Some(left));
    screen.answer("Mission.GetResultsStatusText", text);
    screen.answer("Mission.GetResultsStatusTimer", timer);
    set_state(screen, if before.is_none() { 6 } else { 100 })
}

pub(crate) fn results_answers(data: &ModeData, hud: &Hud2, view: &View, setup: &ResultsSetup, status: Option<i64>) -> Vec<(String, Json)> {
    let mut rows = hud.results.clone();
    rows.sort_by_key(|r| r.1);
    let block: Vec<Json> = (rows.iter().enumerate())
        .flat_map(|(i, &(p, rank, value))| {
            let name = view.names.get(p).cloned().unwrap_or_default();
            let choice = setup.choice == Some(p);
            let time = setup.times.iter().find(|t| t.0 == p).map_or("", |t| t.1.as_str());
            let float = setup.floats.iter().find(|f| f.0 == p).map_or(0., |f| f.1);
            let string = setup.strings.iter().find(|s| s.0 == p).map_or(String::new(), |s| format!("#{}", s.1));
            [json!(rank), json!(format!("#{value}")), json!(float), json!(string), json!(time), json!(choice), json!(i), json!(format!("#{name}"))]
        })
        .collect();
    let position = rows.iter().position(|r| r.0 == view.me).map_or(0, |i| i + 1);
    let n = rows.len();
    let (status_text, status_timer) = results_status(&data.text(RETURN_TO_LOBBY), status);
    let mut out = vec![
        // ponytail: Mission.GetResultsTitle's native is not traced; the row's title (Results_SetMainTitle(self.Title)).
        ("Mission.GetResultsTitle".to_owned(), json!(setup.title)),
        ("Mission.GetResultsStatusText".into(), status_text),
        ("Mission.GetResultsStatusTimer".into(), status_timer),
        ("Mission.GetCurrentPlayerPosition".into(), json!(position)),
        ("HUDComponents.Results_GetNumPlayers".into(), json!(n)),
        ("HUDComponents.Results_GetLayoutType".into(), json!(setup.layout)),
        ("HUDComponents.Results_GetLeaderboardDataBlock".into(), Json::Array(block)),
        ("HUDComponents.Results_GetLeaderboardSubHeaderDataBlock".into(), json!([false, 0, "", ""])),
        ("HUDComponents.Results_GetThrowdownType".into(), json!(setup.throwdown)),
        ("FELeaderboard.EndChallengeOnline_GetLeaderboardNavData".into(), json!([n, position.saturating_sub(1), position])),
        // As endchallenge_teams (team_answers): no gamercard on Steam, so no platform matches and no A button (matrix
        // 10-12: endchallenge_online asked it unanswered).
        ("global.GetPlatform".into(), json!(-1)),
    ];
    for i in 0..4 {
        let title = setup.columns.get(i).cloned().unwrap_or_default();
        out.push((format!("HUDComponents.Results_GetLeaderboardColTitle({i})"), json!(title)));
    }
    // NumberToDistance(x) per float cell, keyed as apt_screen keys a call's argument (a whole number as an integer).
    for (_, v, text) in &setup.floats {
        let key = if v.fract() == 0. && v.abs() < 9e15 { (*v as i64).to_string() } else { json!(v).to_string() };
        out.push((format!("HUDComponents.NumberToDistance({key})"), json!(text)));
    }
    out
}

// ---------------------------------------------------------------- the screen fade
/// How black the screen is (Effects.FadeOut / FadeIn, RepositionAI's fade), written by the S3 modes each frame.
/// ponytail: black; the fade's colour and curve are not traced (linear here).
#[derive(Resource, Default)]
pub(crate) struct ChallengeFade(pub f32);

#[derive(Component)]
struct FadeOverlay;

pub(crate) struct Skate3ModesPlugin;
impl Plugin for Skate3ModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChallengeFade>()
            .add_systems(PostStartup, spawn_fade.after(crate::graphics_menu::PresentationSetup))
            .add_systems(Update, draw_fade);
    }
}

fn spawn_fade(mut commands: Commands, cameras: Query<Entity, With<IsDefaultUiCamera>>) {
    let Ok(camera) = cameras.single() else { return };
    commands.spawn((
        FadeOverlay,
        UiTargetCamera(camera),
        GlobalZIndex(20),
        Pickable::IGNORE,
        BackgroundColor(Color::NONE),
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), display: Display::None, ..default() },
    ));
}

/// The modes' fade, or a NIS's black transition ([`crate::challenge::cutscene::Cutscene::transition`]) or brightness
/// dip ([`crate::challenge::cutscene::Cutscene::brightness`]) when darker.
fn draw_fade(
    fade: Res<ChallengeFade>,
    cutscene: Option<Res<crate::challenge::cutscene::Cutscene>>,
    mut overlays: Query<(&mut Node, &mut BackgroundColor), With<FadeOverlay>>,
) {
    if !fade.is_changed() && !cutscene.as_ref().is_some_and(|c| c.is_changed()) {
        return;
    }
    for (mut node, mut colour) in &mut overlays {
        // A career scene's BRIGHTNESS dip darkens through the same black (Task 27, INFERRED).
        let scene = cutscene.as_ref().map_or(0., |c| c.transition().0.max(1. - c.brightness()));
        let alpha = fade.0.max(scene).clamp(0., 1.);
        node.display = if alpha > 0. { Display::Flex } else { Display::None };
        colour.0 = Color::BLACK.with_alpha(alpha);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    fn team_data() -> super::ModeData {
        let strings = [("ID_RACE_DNF", "DNF"), ("ID_ONLINE_RESULTS_SCREEN_MESSAGE_RETURN_TO_LOBBY", "Returning to Lobby")];
        super::ModeData {
            root: Default::default(),
            rows: Default::default(),
            strings: strings.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            intros: Default::default(),
        }
    }
    /// Voice chat: the online board's GetVoipStatus is each player's voice::voip_status by name (RefreshAll's
    /// UpdateVOIPIcons lights a talker's row); none offline.
    #[test]
    fn a_talker_lights_skate3s_board() {
        crate::voice::hear("Voip S3 Talker", 9401, 2);
        let names: Vec<String> = ["Voip S3 Talker", "Voip S3 Other"].map(String::from).to_vec();
        let data = team_data();
        let voip = |online| {
            let view = super::View { names: &names, me: 1, online, title: "#t".into(), label: "test" };
            let answers = super::board(&data, &super::Hud2::default(), &view);
            answers.into_iter().find(|(k, _)| *k == "FELeaderboard.GetVoipStatus").unwrap().1
        };
        assert_eq!(voip(true), serde_json::json!([2, 0]));
        assert_eq!(voip(false), serde_json::json!([0, 0]));
    }
    /// G8 (online audit 10-28): StartEffect(OOB, 5) starts the out-of-bounds static, full at its end as free skate's;
    /// ResetEffect(OOB) clears it and leaves a fade in progress alone. Before, OOB was only logged.
    #[test]
    fn the_oob_effect_raises_the_static_and_leaves_the_fade() {
        use super::{Effect, Rt};
        let mut rt: Rt<u8> = Rt::new(0, 1, 10.);
        assert_eq!(rt.oob_static(10.), 0.);
        rt.start_effect(Effect::FadeOut, 2.);
        rt.start_effect(Effect::OOB, 5.);
        assert!((rt.oob_static(12.5) - 0.5).abs() < 1e-6, "half way: {}", rt.oob_static(12.5));
        assert_eq!(rt.oob_static(20.), 1.);
        rt.now = 11.;
        rt.reset_effect(Effect::OOB);
        assert_eq!(rt.oob_static(12.), 0., "reset");
        assert!((rt.fade_alpha(11.) - 0.5).abs() < 1e-6, "the FadeOut runs on: {}", rt.fade_alpha(11.));
    }
    /// G9 (online audit 10-28): a running mode's hold (its intro, countdown, turn wait) passes the emotes; its results
    /// screen, no hold, or the core's Results don't.
    #[test]
    fn a_held_running_mode_passes_the_emotes_until_its_results() {
        use crate::match_core::{Match, Phase};
        let mut m = Match { hold: true, phase: Phase::Running { started: 0., ends: None }, ..Default::default() };
        assert!(super::gestures(&m, false), "held while it runs: the emotes go through");
        assert!(!super::gestures(&m, true), "its results screen owns the pad");
        m.hold = false;
        assert!(!super::gestures(&m, false), "not held");
        m.hold = true;
        m.phase = Phase::Results { until: 9. };
        assert!(!super::gestures(&m, false), "the core's Results");
    }
    /// A team race's final results: Red 2 members, Blue 1; this player is Blue's first row.
    fn team_example(data: &super::ModeData) -> serde_json::Value {
        use super::{TeamTable, team_int};
        let member = |rank: i64, name: &str, points: i64, time: &str| [team_int(data, rank), format!("#{name}"), team_int(data, points), time.to_owned()];
        let red = TeamTable {
            name: "Red Team".into(),
            rank: 1,
            score: team_int(data, 1234),
            status: team_int(data, 0),
            members: vec![member(1, "Ann", 3, "#0:41.20"), member(2, "Bob", 1, "#DNF")],
        };
        let blue = TeamTable { name: "Blue Team".into(), rank: 2, score: team_int(data, 2), status: team_int(data, 0), members: vec![member(1, "Cy", 2, "#0:45.00")] };
        super::team_block(true, "Death Race", true, (1, 0), 6, [red, blue])
    }

    /// Mission.GetTeamResultsDataBlock's layout (0x825BD5A8: 17 items) as endchallenge_teams reads it
    /// (dataStructItems 7, teamStructItems 5): header 0 online, 1 throwdown, 3 final, 4 team, 5 row, 6 layout; per
    /// team +0 name, +1 rank, +2 score, +3 status, +4 members. Numbers group as 0x8290E178 does, DNF under 0.
    #[test]
    fn team_results_block_follows_the_retail_layout() {
        use serde_json::json;
        let data = team_data();
        assert_eq!(super::team_int(&data, 1234567), "#1,234,567");
        assert_eq!(super::team_int(&data, -1), "#DNF");
        let block = team_example(&data);
        let items = block.as_array().unwrap();
        assert_eq!(items.len(), 17);
        assert_eq!(items[..7], [json!(true), json!(0), json!("Death Race"), json!(true), json!(1), json!(0), json!(6)]);
        assert_eq!(items[7..11], [json!("#Red Team"), json!(1), json!("#1,234"), json!("#0")]);
        assert_eq!(items[11], json!([["#1", "#Ann", "#3", "#0:41.20"], ["#2", "#Bob", "#1", "#DNF"]]));
        assert_eq!(items[12..14], [json!("#Blue Team"), json!(2)]);
        assert_eq!(items[16].as_array().unwrap().len(), 1);
        // NavData (0x825AF2E8): rows left / right, the selection on this player's row, its team and rank.
        let answers = super::team_answers(&data, &block, Some(7));
        let get = |k: &str| answers.iter().find(|(key, _)| key == k).unwrap().1.clone();
        assert_eq!(get("FELeaderboard.EndChallengeTeams_GetLeaderboardNavData"), json!([2, 1, 1, 0, 1, 1]));
        assert_eq!(get("Mission.GetResultsStatusTimer"), json!("#0:07"));
        assert_eq!(get("Mission.GetResultsStatusText"), json!("Returning to Lobby"));
    }

    /// Item 6: the installed endchallenge_teams (screen 35) with a team race's block: both teams' names and members,
    /// the winner's treatment, the status line (Initialize again when the Results begin, SetState 100 after), no
    /// native left unanswered. SKATE3_ASSET_ROOT = the install's assets; SKATE_ONLINE_HUD_SHOTS saves a picture.
    #[test]
    fn team_results_movie_shows_both_teams() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT").map(std::path::PathBuf::from) else { return };
        let file = assets.join(super::TEAMS_MOVIE);
        if !file.is_file() {
            eprintln!("online-movies stage not run on {}", assets.display());
            return;
        }
        let data = team_data();
        let block = team_example(&data);
        let answers = super::team_answers(&data, &block, None);
        let refs: Vec<(&str, serde_json::Value)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut s = super::Screen::load(&file, &refs).unwrap();
        for _ in 0..60 {
            s.advance().unwrap();
        }
        super::team_status(&mut s, &data, None, Some(6)).unwrap();
        super::team_status(&mut s, &data, Some(6), Some(5)).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        let shown: Vec<String> = (s.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect();
        for want in ["Red Team", "Blue Team", "Ann", "Bob", "Cy", "1,234", "0:41.20", "DNF"] {
            assert!(shown.iter().any(|t| t.contains(want)), "{want} not in {shown:?}");
        }
        let draws = s.draws().unwrap();
        assert!(!draws.is_empty());
        assert!(s.bindings.unanswered.is_empty(), "endchallenge_teams asks {:?}", s.bindings.unanswered);
        if let Ok(out) = std::env::var("SKATE_ONLINE_HUD_SHOTS") {
            std::fs::create_dir_all(&out).unwrap();
            let root = assets.join("private/online-ui");
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("endchallenge_teams.png"));
        }
    }

    /// Matrix Step 2 (10-12, results at max players): six rows (retail's CHALLENGE_MAX) on endchallenge_online in every
    /// layout an S3 mode fills, and on the in-match leaderboardhud in every layout an S3 mode inits (teams 3 a side
    /// too): every name drawn, nothing unanswered. SKATE3_ASSET_ROOT = the install's assets (skipped without it).
    #[test]
    fn six_players_fit_every_s3_results_screen_and_leaderboard() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT").map(std::path::PathBuf::from) else { return };
        let results = assets.join(super::HUD_DIR).join("endchallenge_online.json");
        let board = assets.join("private/online-ui/runtime/leaderboardhud.json");
        if !results.is_file() || !board.is_file() {
            eprintln!("skate3-online / online-ui stages not run on {}", assets.display());
            return;
        }
        let data = team_data();
        let names: Vec<String> = ["Ann", "Bo", "Cy", "Dee", "Eve", "Fay"].map(String::from).to_vec();
        let shown = |s: &super::Screen| -> Vec<String> {
            (s.vm.objects.iter())
                .filter_map(|o| match o.fields.get("_displayText") {
                    Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                    _ => None,
                })
                .collect()
        };
        let view = super::View { names: &names, me: 4, online: true, title: "#Six".into(), label: "test" };
        let mut hud = super::Hud2::default();
        hud.results = (0..6).map(|p| (p, p as i64 + 1, 9000 - 1000 * p as i64)).collect();
        // Skate 3's eResultsLBType per mode: PlaceNameInt 0 (Spot / HOM Battle, OTL), DeathRace 3, BestTrickNonElim
        // Float 4 / Int 5 (Street, Vert), PlaceNameScoreTime 6 (1-Up, OTL Gap/Tag), Skate 7, PlaceNameIntChoice 12,
        // PlaceNameIntInt 14 (Domination), PlaceNameStringTime 15 (Film / Photo), PlaceNameStringScore 17 (OTS).
        for (layout, throwdown) in [(0, 0), (3, 0), (4, 0), (5, 0), (6, 1), (7, 2), (12, 0), (14, 0), (15, 0), (17, 0)] {
            let setup = super::ResultsSetup {
                title: "#Six".into(),
                layout,
                // Spot Battle's Results_SetColTitle (spot_battle.lua 502-505), English.
                columns: ["#Rank", "#Name", "#Points", "#Players' Choice"].map(String::from).to_vec(),
                throwdown,
                choice: Some(2),
                times: (0..6).map(|p| (p, format!("#0:4{p}.00"))).collect(),
                floats: (0..6).map(|p| (p, 10. + p as f64, format!("#{}m", 10 + p))).collect(),
                strings: (0..6).map(|p| (p, "#Owned".to_owned())).collect(),
            };
            let answers = super::results_answers(&data, &hud, &view, &setup, None);
            let refs: Vec<(&str, serde_json::Value)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let mut s = super::Screen::load(&results, &refs).unwrap();
            for _ in 0..90 {
                s.advance().unwrap();
            }
            let text = shown(&s);
            for name in &names {
                assert!(text.iter().any(|t| t.contains(name.as_str())), "layout {layout}: {name} not in {text:?}");
            }
            assert!(s.bindings.unanswered.is_empty(), "layout {layout}: endchallenge_online asks {:?}", s.bindings.unanswered);
            // User 10-14: "Rank" ran off the panel's left edge, centred by its clip's authored 104 px width.
            for (i, o) in s.vm.objects.iter().enumerate() {
                let Some(crate::apt_vm::Value::Text(t)) = o.fields.get("_displayText") else { continue };
                let Some(crate::apt_vm::Value::Object(p)) = o.fields.get("_parent") else { continue };
                if o.fields.get("autoSize") == Some(&crate::apt_vm::Value::Text("left".into())) && t == "Rank" {
                    let x = s.vm.get(*p, "_x").number();
                    assert!(x >= -2., "layout {layout}: header {t:?} ({i}) at {x}");
                }
            }
            if let Ok(out) = std::env::var("SKATE_ONLINE_HUD_SHOTS") {
                std::fs::create_dir_all(&out).unwrap();
                let root = assets.join("private/online-ui");
                let png = std::path::Path::new(&out).join(format!("endchallenge_layout{layout}.png"));
                crate::core_menu_runtime::tests::raster_draws(&s.draws().unwrap(), &root, &png);
            }
        }
        // leaderboardhud: ThrowDown 1 (1-Up), IntStringInt 0 (Spot / HOM Battle), RankNameScore 3 (Street, Vert,
        // Domination), RankNameTime 4 (Death Race), RankNameFloatscore 6; teams 3 a side on 1, 3, 4.
        for (layout, teams) in [(0, false), (1, false), (3, false), (4, false), (6, false), (1, true), (3, true), (4, true)] {
            let mut hud = super::Hud2::default();
            hud.board = super::Board {
                layout,
                ranks: (1..=6).collect(),
                ints: (0..6).map(|p| 900 - 100 * p).collect(),
                visible: true,
                teams: if teams { vec![1, 2, 1, 2, 1, 2] } else { Vec::new() },
                team_names: ["Red Team".into(), "Blue Team".into()],
                team_ranks: [1, 2],
                team_scores: [3, 2],
                ..Default::default()
            };
            let answers = super::board(&data, &hud, &view);
            let mut s = super::Screen::load(&board, &[]).unwrap();
            for (k, v) in answers {
                s.answer(k, v);
            }
            super::set_state(&mut s, 3).unwrap();
            super::set_state(&mut s, 1).unwrap();
            for _ in 0..90 {
                s.advance().unwrap();
            }
            let text = shown(&s);
            for name in &names {
                assert!(text.iter().any(|t| t.contains(name.as_str())), "board {layout} teams {teams}: {name} not in {text:?}");
            }
            assert!(s.bindings.unanswered.is_empty(), "board {layout} teams {teams}: leaderboardhud asks {:?}", s.bindings.unanswered);
        }
    }

    /// The textures an installed skate3-online HUD movie draws once `states` are set and it has run a second, and
    /// the text it shows (None: SKATE3_ASSET_ROOT unset or the stage not run).
    fn drawn_hud(movie: &str, answers: &[(String, serde_json::Value)], states: &[i64]) -> Option<(Vec<String>, Vec<String>)> {
        let assets = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").ok()?);
        let file = assets.join(super::HUD_DIR).join(format!("{movie}.json"));
        if !file.is_file() {
            eprintln!("skate3-online stage not run on {}", assets.display());
            return None;
        }
        let refs: Vec<(&str, serde_json::Value)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut s = super::Screen::load(&file, &refs).unwrap();
        for &state in states {
            super::set_state(&mut s, state).unwrap();
        }
        for _ in 0..60 {
            s.advance().unwrap();
        }
        let text = (s.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect();
        Some((s.draws().unwrap().into_iter().map(|d| d.texture).collect(), text))
    }

    /// Bug 10-25 #4 (build 10): the online results screen after a Spot Battle showed no button icons. SetEndOptions
    /// (SetState 6 as the Results begin) adds B Quit through button_item2, whose glyph the skate3-online manifest lacked.
    #[test]
    fn the_online_results_screen_draws_its_quit_button() {
        let data = team_data();
        let names: Vec<String> = ["Ann", "Bo", "Cy"].map(String::from).to_vec();
        let view = super::View { names: &names, me: 1, online: true, title: "#Spot".into(), label: "test" };
        let mut hud = super::Hud2::default();
        hud.results = (0..3).map(|p| (p, p as i64 + 1, 900 - 100 * p as i64)).collect();
        let columns = ["#Rank", "#Name", "#Points", "#Players' Choice"].map(String::from).to_vec();
        let setup = super::ResultsSetup { title: "#Spot".into(), layout: 12, columns, throwdown: 0, choice: Some(0), times: vec![], floats: vec![], strings: vec![] };
        let answers = super::results_answers(&data, &hud, &view, &setup, Some(7));
        let Some((drawn, _)) = drawn_hud("endchallenge_online", &answers, &[6]) else { return };
        assert!(drawn.iter().any(|t| t.ends_with("buttons/B.rgba")), "no B glyph in {drawn:?}");
    }

    /// Bug 10-25 #15 (build 10): an online versus vote showed no Sick / Tight / Whatevs / Fail buttons. hudonlinevoting's
    /// Init sets its four button_item2s to Y_hud / X_hud / B_hud / A_hud; the manifest had no glyph for any of them.
    #[test]
    fn the_vote_hud_draws_its_four_buttons() {
        let Some((drawn, text)) = drawn_hud("hudonlinevoting", &[], &[1, 4]) else { return };
        for button in ["A", "B", "X", "Y"] {
            assert!(drawn.iter().any(|t| t.ends_with(&format!("buttons/{button}.rgba"))), "no {button} glyph in {drawn:?}");
        }
        for label in ["Sick", "Tight", "Whatevs", "Fail"] {
            assert!(text.iter().any(|t| t.contains(label)), "{label} not in {text:?}");
        }
    }

    /// User 10-14: a Rematch's results show only its own game's players (the second game sets each player again).
    #[test]
    fn a_rematch_sets_each_players_result_again() {
        let mut rt: Rt<u8> = Rt::new(0, 2, 0.);
        rt.result(0, 1, 500);
        rt.result(1, 2, 300);
        rt.result(0, 2, 100);
        rt.result(1, 1, 900);
        assert_eq!(rt.hud.results, [(0, 2, 100), (1, 1, 900)]);
    }

    #[test]
    fn one_up_letters_play_the_users_or_the_challengers_sting() {
        let mut rt: Rt<u8> = Rt::new(0, 2, 0.);
        rt.letters(2, false, true);
        assert_eq!(rt.hud.letters, Some((1, 7, Some("1up_u_user"))));
        rt.letters(3, false, false);
        assert_eq!(rt.hud.letters, Some((2, 8, Some("1up_p_challenger"))));
        rt.letters(1, true, true);
        assert_eq!(rt.hud.letters, Some((3, 1, None)), "S.K.A.T.E.'s movie states play their own");
    }
    #[test]
    fn the_results_play_a_win_or_a_loss_for_this_player() {
        let results = [(0, 2, 10), (1, 1, 20)];
        assert_eq!(results_sound(&results, 1), "contest_win");
        assert_eq!(results_sound(&results, 0), "contest_lose");
        assert_eq!(results_sound(&results, 5), "contest_leaderboard", "a spectator sees the standings");
    }

    use super::*;

    /// I5: online, Online.OnlineChallengeRoundSyncPoint holds the graph until every console still in the match is there.
    #[test]
    fn an_online_sync_point_waits_for_every_console() {
        let mut rt: Rt<u8> = Rt::new(0, 3, 0.);
        rt.online = true;
        rt.sync();
        assert!(rt.hud.waiting && rt.due().is_none(), "waiting for skaters");
        rt.release(&[1, 0, 1]);
        assert!(rt.due().is_none(), "one console is not there yet");
        rt.release(&[1, 1, u32::MAX]);
        assert!(!rt.hud.waiting && rt.due() == Some(0.), "a player who left holds no one up");
        let mut hot_seat: Rt<u8> = Rt::new(0, 2, 0.);
        hot_seat.sync();
        assert_eq!((hot_seat.due(), hot_seat.syncs), (Some(0.), 1), "offline at once");
    }

    /// I7 / I8: HUD2.Show(TrickDisplayHUD) and Pause.Disable/EnablePauseMenuAndBackpack reach the flags the glue
    /// hands match_core (scoring_hud / skate2_hud hide the trick display; the crossbar and session markers stay shut).
    #[test]
    fn trick_display_and_pause_follow_the_lua() {
        let mut rt: Rt<u8> = Rt::new(0, 2, 0.);
        rt.log = Some(Vec::new());
        rt.show(Component::TrickDisplayHUD, false);
        rt.pause(true);
        assert!(rt.hud.trick_hidden && rt.pause_disabled);
        rt.show(Component::TrickDisplayHUD, true);
        rt.pause(false);
        assert!(!rt.hud.trick_hidden && !rt.pause_disabled);
        assert_eq!(rt.log.unwrap()[1], "0.00 pause true", "the oracle's words");
    }

    /// M4 (online audit 10-06): RepositionAI's 6th argument hands the camera over as the batch moves (0x82d55fa8): a
    /// faded move at its black, the player before watched until then, the call still logged where the Lua made it;
    /// fade 0 at once.
    #[test]
    fn a_watched_reposition_switches_the_camera_at_black() {
        struct Quiet;
        impl Graph for Quiet {
            type State = u8;
            fn name(_: u8) -> &'static str {
                "quiet"
            }
            fn on(&mut self, _: &mut Rt<u8>, _: &Msg) -> Handled {
                Ok(())
            }
        }
        let place = Place { field: "TurnBasedAttemptSpawnPoint", index: 0, slot: 0 };
        let mut rt: Rt<u8> = Rt::new(0, 3, 10.);
        rt.log = Some(Vec::new());
        rt.camera(2);
        rt.reposition_watched(1, place, 1.);
        assert_eq!(rt.watching, 2, "the call: the fade out still shows the player before");
        assert_eq!(rt.log.as_ref().unwrap().last().map(String::as_str), Some("10.00 camera 1"), "the oracle's line, at the call");
        advance(&mut Quiet, &mut rt, 10.49);
        assert_eq!(rt.watching, 2);
        advance(&mut Quiet, &mut rt, 10.5);
        assert_eq!((rt.watching, rt.landed(1).map(|l| l.1.slot)), (1, Some(0)), "at black, with the move");
        rt.reposition_watched(0, place, 0.);
        assert_eq!(rt.watching, 0, "fade 0: at once");
    }

    /// I6: endchallenge_online's status line online. s3re 10-28: retail's text is "Returning to Lobby" and
    /// GetResultsStatusTimer (0x825BEA80) a "#:%02d" string of minutes and seconds, not the bare seconds.
    #[test]
    fn online_results_say_when_freeskate_returns() {
        assert_eq!(results_status("Returning to Lobby", Some(7)), (json!("Returning to Lobby"), json!("#0:07")));
        assert_eq!(results_status("Returning to Lobby", Some(70)).1, json!("#1:10"));
        assert_eq!(team_data().text(RETURN_TO_LOBBY), "Returning to Lobby");
        assert_eq!(results_status("x", None), (json!(""), json!(0)), "offline: no status");
    }

    /// s3re (10-28): the results screen's B is retail's Quit: popup 63 ("Quit Session?") whose Yes leaves the session
    /// (event 18). It no longer ends the results on this console. When retail enables it is UNRECOVERED: always here.
    #[test]
    fn online_results_b_asks_to_quit_the_session() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        let mut m = crate::match_core::Match::default();
        (m.token, m.phase) = (7, crate::match_core::Phase::Results { until: 9. });
        world.insert_resource(m);
        let mut nav = crate::customiser::Navigation::default();
        nav.pressed = 0x2000;
        world.insert_resource(nav);
        world.init_resource::<crate::online_menu::OnlineUi>();
        assert_eq!(online_results(&mut world, true), Some(9));
        assert_eq!(world.resource::<crate::online_menu::OnlineUi>().requests, [crate::online_menu::OnlineRequest::QuitSession]);
        assert_eq!(world.resource::<crate::match_core::Match>().phase, crate::match_core::Phase::Results { until: 9. }, "the results go on");
        // The popup up, or B closing it this frame: that press is the popup's.
        world.resource_mut::<crate::online_menu::OnlineUi>().requests.clear();
        world.resource_mut::<crate::online_menu::OnlineUi>().modal = true;
        online_results(&mut world, true);
        world.resource_mut::<crate::online_menu::OnlineUi>().modal = false;
        world.resource_mut::<crate::online_menu::OnlineUi>().closed_this_frame = true;
        online_results(&mut world, true);
        assert!(world.resource::<crate::online_menu::OnlineUi>().requests.is_empty());
        // No results screen up: B is play's.
        world.resource_mut::<crate::online_menu::OnlineUi>().closed_this_frame = false;
        online_results(&mut world, false);
        assert!(world.resource::<crate::online_menu::OnlineUi>().requests.is_empty());
    }

    #[test]
    fn lua_sort_orders_as_lua_does() {
        // Every permutation of a list with ties sorts the way Lua's own table.sort leaves it.
        let lua = mlua::Lua::new();
        let sort: mlua::Function = lua
            .load("return function(t) table.sort(t, function(a, b) return a[1] < b[1] or (a[1] == b[1] and a[2] > b[2]) end) local o = {} for i, v in ipairs(t) do o[i] = v[3] end return o end")
            .eval()
            .unwrap();
        let items = [(1, 5, 0), (0, 5, 1), (1, 5, 2), (0, 9, 3), (2, 0, 4), (1, 5, 5), (0, 5, 6)];
        let mut x = 7u64;
        for _ in 0..300 {
            let mut v = items.to_vec();
            for i in (1..v.len()).rev() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                v.swap(i, (x % (i as u64 + 1)) as usize);
            }
            let n = 1 + (x % v.len() as u64) as usize;
            v.truncate(n);
            let t = lua.create_sequence_from(v.iter().map(|&(a, b, c)| vec![a, b, c])).unwrap();
            let want: Vec<i32> = sort.call(t).unwrap();
            lua_sort(&mut v, |a, b| a.0 < b.0 || (a.0 == b.0 && a.1 > b.1));
            assert_eq!(v.iter().map(|e| e.2).collect::<Vec<_>>(), want);
        }
    }

    #[test]
    fn strings_fill_as_the_formatter_does() {
        let p = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(fill("{0:s} is Leading the Round: {1:points} pts", &p(&["P1", "12345"])), "P1 is Leading the Round: 12,345 pts");
        assert_eq!(fill("Score Set: {0:points}", &p(&["999"])), "Score Set: 999");
        assert_eq!(fill("{0:s}, your turn!", &p(&[])), ", your turn!");
    }

    /// The oracle's natives for every S3 mode graph: the stategraph (handlers by `-- [state] message`, ChangeState
    /// queued), ChallengeTimer, Effects, RepositionAI, the presentations and HUD2, logging what [`Rt`] logs. A mode's
    /// test sets SELF (the row's fields), N / ME / FIRST and PLANS (per run: {dt, kind, value}) and ACTOR() (the player
    /// whose run it is), and calls run(src, initial, run_state).
    pub(crate) const DRIVER: &str = r##"
table.getn = function(t) return #t end
print = function() end
local meta
meta = {__index = function(t, k) local v = setmetatable({}, meta); rawset(t, k, v); return v end, __call = function() return nil end}
local function ns(t) return setmetatable(t, meta) end
LOG, ERRORS = {}, {}
NOW = 0
local timers, next_timer, effects, later, posted, transition = {}, 0, {}, {}, {}, nil
GONE = {}
local function log(s) LOG[#LOG + 1] = string.format("%.2f %s", NOW, s) end
log_line = log
local function join(...)
  local out = {}
  for i = 1, select("#", ...) do local v = select(i, ...); if v ~= nil and v ~= "" then out[#out + 1] = tostring(v) end end
  return table.concat(out, ",")
end
local function post(msg, m) posted[#posted + 1] = {msg, m or {}} end
POST = post
Challenge = ns{GameMode = {SinglePlayer = 0, PartyPlay = 1, OnlineChallenge = 2}, GetGameMode = function() return 2 end,
  EActivityCompletionType = {Abandoned = 0, Owned = 1, Killed = 2},
  GetTeleportFadeInProgress = function() return transition ~= nil end,
  CloseLoadingScreen = function() log("close") end, OpenLoadingScreen = function() log("open") end}
Online = ns{GetLocalPlayerID = function() return ME end, GetNumOnlineSkaters = function() return N end,
  GetSkaterName = function(i) return "P" .. i end, IsPlayerDisconnected = function(i) return GONE[i] == true end,
  -- A team test sets TEAMS (by player id, 1 / 2); Teams.GetName(i) answers "T<i>".
  IsTeamChallenge = function() return rawget(_G, "TEAMS") ~= nil end,
  GetTeam = function(i) local t = rawget(_G, "TEAMS"); return t and t[i] or 1 end,
  eFinishReason = {NOT_FINISHED = 0, NORMAL_COMPLETION = 1, PLAYER_DISCONNECT = 2},
  SwitchCamera = function(id) log("camera " .. id) end, LeaveGame = function() log("leave") end,
  OnlineChallengeRoundSyncPoint = function() log("sync"); post("onlinesynccomplete") end,
  SetPlayersChoice = function(id) log("choice " .. id) end}
-- VotingManager (0x82563bd8 start, 0x82563cc8 stop): 7 votes, 1 each; the stop posts the others' mean.
VOTES, VOTE_ACTIVE, VOTING = {}, 0, false
local function reset_votes() for i = 0, 6 do VOTES[i] = 1 end end
reset_votes()
Online.StartVoting = function(p) if VOTING then reset_votes() end VOTE_ACTIVE = p; VOTING = true; log("vote start " .. p) end
Online.StopVoting = function()
  local sum, n = 0, 0
  for i = 0, N - 1 do if i ~= VOTE_ACTIVE and not GONE[i] then sum = sum + VOTES[i]; n = n + 1 end end
  local score = 0
  if n > 0 then score = sum / n end
  log(string.format("vote stop %d %.3f", VOTE_ACTIVE, score))
  post("votingcomplete", {PlayerID = VOTE_ACTIVE, Score = score})
  VOTING, VOTE_ACTIVE = false, 0
  reset_votes()
end
Results = ns{SetWinner = function(id) log("winner " .. id) end}
Teams = ns{GetName = function(i) return "T" .. i end}
Pause = ns{DisablePauseMenuAndBackpack = function() log("pause true") end, EnablePauseMenuAndBackpack = function() log("pause false") end}
NIS = ns{EnterNISUsingDefinition = function() log("nis"); later[#later + 1] = {NOW, "nischallengenotify", {}} end}
Random = ns{GetRandomNumber = function(n) return FIRST % n + 1 end}
Debug = ns{IsDebugEnabled = function() return false end}
GameMode = ns{OneUpSpeedRounds = function() return false end, SkateSpeedRounds = function() return false end}
ChallengeReplay = ns{GetLastFrameNumberWritten = function() return 1000 end}
Objectives = ns{MakeObjectives = function(id) return 100 + id end}
Trick = ns{GetCompletedSequenceScore = function() return SCORE end}
ChallengeTimer = ns{}
function ChallengeTimer.AddCountdownTimer(s) next_timer = next_timer + 1; timers[next_timer] = NOW + s; return next_timer end
ChallengeTimer.AddHUDCountdownTimer = ChallengeTimer.AddCountdownTimer
function ChallengeTimer.RemoveCountdownTimer(id) if id then timers[id] = nil end end
ChallengeTimer.RemoveHUDCountdownTimer = ChallengeTimer.RemoveCountdownTimer
function ChallengeTimer.GetCountdownTime(id) if id and timers[id] then return timers[id] - NOW end return 0 end
-- Count-up timers share the ids; a removed one reads 0.
local countups = {}
function ChallengeTimer.AddCountupTimer() next_timer = next_timer + 1; countups[next_timer] = {NOW, nil}; return next_timer end
function ChallengeTimer.GetCountupTime(id) local c = id and countups[id]; if not c then return 0 end return (c[2] or NOW) - c[1] end
function ChallengeTimer.PauseTimer(id) local c = id and countups[id]; if c and not c[2] then c[2] = NOW end end
function ChallengeTimer.RemoveCountupTimer(id) if id then countups[id] = nil end end
-- Retail's Lua 5.1 formats a fractional number for %d / %i by truncating it (the oracle's 5.4 raises an error).
local format = string.format
string.format = function(f, ...)
  local args, n, i = {...}, select("#", ...), 0
  for _, spec in f:gmatch("%%([-+ #0]*%d*%.?%d*)([%a%%])") do
    if spec ~= "%" then
      i = i + 1
      local v = args[i]
      if (spec == "d" or spec == "i") and math.type(v) == "float" and v == v and math.abs(v) < 2^53 then
        if v >= 0 then v = math.floor(v) else v = math.ceil(v) end
        args[i] = math.tointeger(v)
      end
    end
  end
  return format(f, table.unpack(args, 1, n))
end
Effects = ns{eChallengeEffect = {FadeOut = "FadeOut", FadeIn = "FadeIn"},
  -- %.14g: retail's Lua 5.1 prints 1.0 as "1" (the oracle's Lua keeps "1.0").
  StartEffect = function(e, s) log("effect " .. e .. " " .. string.format("%.14g", s)); effects[e] = NOW + s end,
  ResetEffect = function(e) log("reseteffect " .. e); effects[e] = nil end}
AI = ns{
  RepositionAI = function(id, loc, slot, _, fade, cam)
    -- 0x8284a950: slot defaults to the player id, fade to 1.
    slot = slot or id
    fade = fade or 1
    log(string.format("move %d %s %d %.14g", id, tostring(loc), slot, fade))
    if GONE[id] then return end
    if cam then log("camera " .. id) end
    if fade <= 0.001 then post("skaterteleportcomplete", {PlayerID = id})
    elseif transition == nil then transition = NOW + fade + HOLD end
  end,
  PinAI = function(id) log("pin " .. id) end, UnpinAI = function(id) log("unpin " .. id) end}
ChallengePresentation = ns{StartPresentation = function(g) log("present"); later[#later + 1] = {NOW, "presentationeventgroupfinished", {ChallengePresentationGroup = g}} end,
  AddPresentationGroup = function() return 7 end}
HighlightReel = ns{PlayHighlightReel = function() log("reel"); post("highlightreelcomplete") end,
  PlayHighlightReelWithSpecifiedOrder = function() log("reel"); post("highlightreelcomplete") end,
  AddHighlightUsingDefinition = function() return 5 end, GetNumberOfHighlights = function() return 1 end}
FrontEnd = ns{Results_SetPlayerData = function(id, rank, v) log(string.format("result %d %d %d", id, rank, v)) end,
  eResultsLBType = {PlaceNameInt = 0, PlaceNameScoreTime = 6},
  -- Party Play: the card is answered at once, the results dialog with DIALOG_ANSWER (default hudcontinue).
  OpenPressADialog = function(p, r) log(string.format("pressa %d %d", p, r)); post("hudcomplete") end,
  OpenContinueRetryDialog = function() log("dialog"); post(table.remove(DIALOG_ANSWERS or {}, 1) or "hudcontinue") end}
HUD2 = ns{eMessageColour = {White = 0, Red = 1, Green = 2}, eMessageAudioHook = {Success = 2},
  eHUDComponent = setmetatable({}, {__index = function(_, k) return k end}),
  eLeaderboardHUDMenuType = {IntStringInt = 0, ThrowDown = 1, IntString = 2, Skate = 7},
  eDPOFormat = {Ints = 0, Points = 1, Time = 2, Distance = 3, Players = 4},
  CreateGroup = function() post("hudloaded", {}) end,
  ShowCentralMessage = function(t, c, a, b, cc, d, hook) log(string.format("central %s %d %s %d", t, c or 0, join(a, b, cc, d), hook or 0)) end,
  ShowCustomCentralMessage = function(t, c, p) log(string.format("central %s %d %s 0", t, c or 0, p or "")) end,
  ShowModifiedCentralMessage = function(t, c, a, b, cc, d, stay) log(string.format("modified %s %d %s %s", t, c or 0, join(a, b, cc, d), tostring(stay == true))) end,
  ClearModifiedCentralMessage = function() log("clear") end,
  ShowSkateMessage = function(_, n, skate, loc) if skate == nil then skate = true end log(string.format("letters %d %s %s", n, tostring(skate), tostring(loc == true))) end,
  LeaderboardSetTurn = function(_, id, on) if on == nil then log("turn " .. id) else log("turn " .. id .. " " .. tostring(on)) end end,
  LeaderboardHUDInit = function(_, t, n, teams) log(string.format("board %d %d", t, n) .. (teams and " teams" or "")) end,
  LeaderboardHUDSetTeamRank = function(_, i, r) log(string.format("teamrank %d %d", i, r)) end,
  LeaderboardHUDSetTeamScore = function(_, i, s) log(string.format("teamscore %d %d", i, s)) end,
  LeaderboardHUDSetTeamHighlight = function(_, i, on) log(string.format("teamhighlight %d %s", i, tostring(on))) end,
  eTeamMessageType = {Normal = 0, Assignment = 1, Winning = 2},
  ShowTeamCentralMessage = function(t, team, kind, a, b, cc, d) log(string.format("teamcentral %s %d %d %s", t, team, kind, join(a, b, cc, d))) end,
  ShowCustomTeamCentralMessage = function(t, team, kind, p) log(string.format("teamcentral %s %d %d %s", t, team, kind, p or "")) end,
  DPOSetTeamColour = function(_, t) log("dpoteam " .. t) end,
  LeaderboardHUDSetRank = function(_, id, r) log(string.format("rank %d %d", id, r)) end,
  LeaderboardHUDSetInt = function(_, id, v) log(string.format("int %d %d", id, v)) end,
  LeaderboardHUDSetVariantString = function(_, t) log("variant " .. t) end,
  Show = function(_, c, v) log("show " .. c .. " " .. tostring(v)) end,
  DPOSetFormatType = function(_, f) log("dpoformat " .. f) end,
  DPOSetTitle = function(_, t) log("dpotitle " .. t) end,
  DPOSetParams = function(_, a, b) log(string.format("dpo %d %s", a or 0, b and string.format("%d", b) or "-")) end,
  StartCountdown = function(_, id) log("countdown " .. id) end,
  StartTimer = function(_, id) log("timer " .. id) end}
setmetatable(_G, {__index = function(t, k) local v = setmetatable({}, meta); rawset(t, k, v); return v end})

local H, queue, state = {}, {}, nil
VARS = {}
function ChangeState(s) queue[#queue + 1] = s end
local function send(msg, m)
  local f = H[state] and H[state][msg]
  local g = H.global and H.global[msg]
  for k = 1, 2 do
    local h = f
    if k == 1 then h = g end
    if h then
      local ok, e = pcall(h, SELF, m or {}, VARS, {}, {})
      if not ok then log("error"); ERRORS[#ERRORS + 1] = state .. " " .. msg .. ": " .. tostring(e) end
    end
  end
end
SEND = send
local plan, runs = {}, 0
local function settle()
  while #queue > 0 do
    local s = table.remove(queue, 1)
    if state then send("stateexit") end
    state = s
    log("enter " .. s)
    send("stateenter")
    if s == RUN_STATE then
      runs = runs + 1
      for _, e in ipairs(PLANS[runs] or {}) do plan[#plan + 1] = {NOW + e[1], e[2], e[3], e[4]} end
    end
  end
end
local function deliver(e)
  local kind, v = e[2], e[3]
  -- A mode's own events (its setup's DELIVER), before the shared ones.
  if DELIVER and DELIVER(kind, v, e[4]) then return end
  if kind == "score" then
    SCORE = v
    local a = ACTOR()
    if a then send("objectivecomplete", {ObjectiveGroup = 100 + a, ObjectiveIndex = 0}) end
  elseif kind == "bail" then
    local a = ACTOR()
    if a then send("wipeout", {PlayerID = a, IsAPhysicsTeleport = false}) end
  elseif kind == "leave" then
    if not GONE[v] then GONE[v] = true; log("left " .. v); send("playerdisconnected", {PlayerIndex = v}) end
  end
end
function run(src, initial)
  for st, msg, fn in src:gmatch("%-%- %[([%w_]+)%] ([%w_]+)%s+function (h_%w+)") do
    H[st] = H[st] or {}
    H[st][msg] = rawget(_G, fn)
  end
  ChangeState(initial)
  settle()
  for _ = 1, 200000 do
    if #posted > 0 then
      local p = table.remove(posted, 1)
      send(p[1], p[2])
    else
      -- later (FIFO), effects, the transition, then timers by id; plan events after all of those.
      local at, pick = math.huge, nil
      for i, l in ipairs(later) do if l[1] < at then at, pick = l[1], {"later", i} end end
      for _, e in ipairs({"FadeOut", "FadeIn"}) do
        local d = effects[e]
        if d and d < at then at, pick = d, {"effect", e} end
      end
      if transition and transition < at then at, pick = transition, {"transition"} end
      local tid = nil
      for id, t in pairs(timers) do if t < at or (t == at and pick and pick[1] == "timer" and id < tid) then at, pick, tid = t, {"timer", id}, id end end
      local pi = nil
      for i, e in ipairs(plan) do if e[1] < at and (pi == nil or e[1] < plan[pi][1]) then pi = i end end
      if pi then
        local e = table.remove(plan, pi)
        NOW = math.max(NOW, e[1])
        deliver(e)
      elseif not pick then
        break
      else
        NOW = math.max(NOW, at)
        if pick[1] == "later" then
          local l = table.remove(later, pick[2])
          send(l[2], l[3])
        elseif pick[1] == "effect" then
          effects[pick[2]] = nil
          send("effectfinished", {Effect = pick[2]})
        elseif pick[1] == "transition" then
          transition = nil
          send("teleporttransitionfinished")
        else
          timers[pick[2]] = nil
          send("timerexpired", {TimerID = pick[2]})
        end
      end
    end
    settle()
    if state == "complete" then break end
  end
  return LOG
end
"##;

    thread_local! {
        /// A console's key draws its latency as any message does (skate3_uniform's 10-26 leave-after-the-mean case);
        /// otherwise it goes out at once, drawing nothing, so every test's latencies stay the ones they were written
        /// against.
        pub(crate) static KEY_DRAWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// The match relay between several consoles in one test (online audit 10-12): every message one console sends
    /// reaches each other console after a latency drawn in `latency` (seconds), a lost packet costing a resend (the
    /// record's ~110-240 ms), each sender's messages in its order (match_core's relay keeps it), different senders'
    /// interleaved as their latencies fall (reordered). Each console takes them through its own
    /// [`crate::match_core::Leaves`], as match_core's relay does: numbered per sender, so duplicates never reach a mode,
    /// and a leaver's events sent on by the others ([`Wire::left`], end review I2).
    pub(crate) struct Wire {
        x: u64,
        pub latency: (f64, f64),
        pub loss: f64,
        queue: Vec<(f64, u64, usize, usize, u32, String)>,
        last: std::collections::BTreeMap<(usize, usize), f64>,
        seq: u64,
        /// Per sender: the number its next message gets.
        numbered: std::collections::BTreeMap<usize, u32>,
        /// Per console: its relay's taking, and its key for its own events.
        relays: std::collections::BTreeMap<usize, crate::match_core::Leaves>,
        /// The consoles cut off.
        gone: std::collections::BTreeSet<usize>,
    }
    impl Wire {
        pub fn new(seed: u64, latency: (f64, f64), loss: f64) -> Self {
            Wire {
                x: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
                latency,
                loss,
                queue: Vec::new(),
                last: Default::default(),
                seq: 0,
                numbered: Default::default(),
                relays: Default::default(),
                gone: Default::default(),
            }
        }
        /// A uniform draw in [0, 1).
        pub fn draw(&mut self) -> f64 {
            self.x ^= self.x << 13;
            self.x ^= self.x >> 7;
            self.x ^= self.x << 17;
            (self.x % 1_000_000) as f64 / 1_000_000.
        }
        /// `from` sends `texts` (in order) to every other of `n` consoles at `now`, each signed, behind its key in its
        /// first message (match_core::Leaves::publish). One bigger than a match event record (skate_net
        /// Events::fitted, 1024 bytes) never leaves: match_core drops it (end review I2).
        pub fn send(&mut self, now: f64, from: usize, n: usize, texts: Vec<String>) {
            let fits = |t: &String| skate_net::roster::Events { token: u64::MAX, first: u32::MAX, items: vec![t.clone()] }.fitted().0 == 1;
            let key = if self.numbered.contains_key(&from) { None } else { self.relays.entry(from).or_default().key_item() };
            for (text, is_key) in key.into_iter().map(|k| (k, true)).chain(texts.into_iter().map(|t| (t, false))) {
                let k = *self.numbered.entry(from).or_default();
                let text = if is_key { text } else { self.relays.entry(from).or_default().sign(from, k, &text) };
                if !fits(&text) {
                    eprintln!("wire: {} bytes do not fit a record; dropped", text.len());
                    continue;
                }
                *self.numbered.entry(from).or_default() += 1;
                let draws = !is_key || KEY_DRAWS.with(std::cell::Cell::get);
                for to in (0..n).filter(|&t| t != from) {
                    let mut at = now;
                    if draws {
                        at += self.latency.0 + (self.latency.1 - self.latency.0) * self.draw();
                        while self.draw() < self.loss {
                            at += 0.11 + 0.13 * self.draw();
                        }
                    }
                    let last = self.last.entry((from, to)).or_insert(0.);
                    at = at.max(*last);
                    *last = at;
                    self.seq += 1;
                    self.queue.push((at, self.seq, from, to, k, text.clone()));
                }
            }
        }
        /// `from` vanished: what it sent `to` that is still on the way is lost.
        pub fn cut(&mut self, from: usize, to: usize) {
            self.gone.insert(from);
            self.queue.retain(|q| !(q.2 == from && q.3 == to));
        }
        /// Console `to` (of `n`) sees `who` gone at `now`: the first time, it sends on what it took of `who`'s, then
        /// its mark. Whether its mode gets the leave now: every other console still here has sent its mark
        /// (match_core::Leaves; a call before this frame's [`Wire::take`], as the relay does).
        pub fn left(&mut self, now: f64, to: usize, who: usize, n: usize) -> bool {
            let texts = self.relays.entry(to).or_default().left(to, who, now);
            self.send(now, to, n, texts);
            let others: Vec<usize> = (0..n).filter(|&o| o != to && o != who && !self.gone.contains(&o)).collect();
            self.relays[&to].released(who, others, now)
        }
        /// What reached `to` by `now`, in arrival order: each new event once, with its sender's slot (a leaver's sent
        /// on by another: the leaver's).
        pub fn take(&mut self, now: f64, to: usize) -> Vec<(usize, String)> {
            let mut due: Vec<(f64, u64, usize, u32, String)> = Vec::new();
            self.queue.retain(|(at, seq, from, t, k, text)| {
                let take = *t == to && *at <= now;
                if take {
                    due.push((*at, *seq, *from, *k, text.clone()));
                }
                !take
            });
            due.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let relay = self.relays.entry(to).or_default();
            let mut out = Vec::new();
            for (_, _, from, k, text) in due {
                relay.take(from, k, std::slice::from_ref(&text), &mut out);
            }
            out
        }
    }

    /// One plan event: seconds into a run, then a score, a bail or a player leaving.
    #[derive(Clone, Debug)]
    pub(crate) enum Ev {
        Score(i64),
        Bail,
        Leave(usize),
        /// A mode's own event: its kind (DELIVER's in the oracle, `other`'s in the model) and two values.
        Other(&'static str, i64, f64),
    }

    /// Runs the oracle (the disc Lua under [`DRIVER`]) and returns its log, or the Lua errors.
    pub(crate) fn lua_log(src: &str, setup: &str, initial: &str, run_state: &str, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>) {
        let lua = mlua::Lua::new();
        let g = lua.globals();
        g.set("HOLD", f64::from(crate::skate3_skate_game::FADE_HOLD)).unwrap();
        g.set("RUN_STATE", run_state).unwrap();
        let t = lua.create_table().unwrap();
        for (i, run) in plans.iter().enumerate() {
            let r = lua.create_table().unwrap();
            for (j, (dt, ev)) in run.iter().enumerate() {
                let (kind, v, w) = match ev {
                    Ev::Score(n) => ("score", *n, 0.),
                    Ev::Bail => ("bail", 0, 0.),
                    Ev::Leave(p) => ("leave", *p as i64, 0.),
                    Ev::Other(k, v, w) => (*k, *v, *w),
                };
                let s = lua.create_string(kind).unwrap();
                let e = [mlua::Value::Number(*dt), mlua::Value::String(s), mlua::Value::Integer(v), mlua::Value::Number(w)];
                r.set(j + 1, lua.create_sequence_from(e).unwrap()).unwrap();
            }
            t.set(i + 1, r).unwrap();
        }
        g.set("PLANS", t).unwrap();
        lua.load(DRIVER).set_name("@driver").exec().unwrap();
        lua.load(setup).set_name("@setup").exec().unwrap();
        lua.load(src).set_name("@graph").exec().unwrap();
        let run: mlua::Function = g.get("run").unwrap();
        let log: Vec<String> = run.call((src, initial)).unwrap();
        let errors: Vec<String> = g.get("ERRORS").unwrap();
        (log, errors)
    }

    /// Runs a ported graph the way [`DRIVER`] runs the Lua: `actor` names whose run it is, `deliver` turns a plan
    /// event into the graph's message.
    pub(crate) fn model_log<G: Graph>(
        g: &mut G,
        rt: &mut Rt<G::State>,
        run_state: G::State,
        plans: &[Vec<(f64, Ev)>],
        actor: impl Fn(&G) -> Option<usize>,
        done: impl Fn(&Rt<G::State>) -> bool,
        mut other: impl FnMut(&mut G, &mut Rt<G::State>, &str, i64, f64) -> Option<Msg>,
    ) -> Vec<String> {
        rt.log = Some(Vec::new());
        start(g, rt);
        let entered = format!("enter {}", G::name(run_state));
        let (mut plan, mut runs, mut seen) = (Vec::<(f64, Ev)>::new(), 0, 0);
        for _ in 0..200_000 {
            // Runs entered since the last step: their plans start now (DRIVER's settle).
            let log = rt.log.as_ref().unwrap();
            let fresh = log[seen..].iter().filter(|l| l.ends_with(&entered)).count();
            seen = log.len();
            for _ in 0..fresh {
                let now = rt.now;
                plan.extend(plans.get(runs).into_iter().flatten().map(|(dt, e)| (now + dt, e.clone())));
                runs += 1;
            }
            if done(rt) {
                break;
            }
            let due = rt.due();
            let pi = (0..plan.len()).filter(|&i| due.is_none_or(|d| plan[i].0 < d)).min_by(|&a, &b| plan[a].0.total_cmp(&plan[b].0));
            if let Some(i) = pi {
                let (at, ev) = plan.remove(i);
                rt.now = rt.now.max(at);
                let msg = match ev {
                    Ev::Score(n) => actor(g).map(|p| Msg::Objective { player: p, score: n }),
                    Ev::Bail => actor(g).map(Msg::Wipeout),
                    Ev::Leave(p) if !rt.gone[p] => {
                        rt.gone[p] = true;
                        rt.note(format!("left {p}"));
                        Some(Msg::Left(p))
                    }
                    Ev::Leave(_) => None,
                    Ev::Other(kind, v, w) => other(g, rt, kind, v, w),
                };
                if let Some(m) = msg {
                    dispatch(g, rt, m);
                }
            } else if let Some(at) = due {
                if let Some(m) = rt.pop(at) {
                    dispatch(g, rt, m);
                }
            } else {
                break;
            }
        }
        rt.log.take().unwrap()
    }

    /// The first difference between two logs, with context.
    pub(crate) fn compare(what: &str, lua: &[String], model: &[String], errors: &[String]) {
        if let Some(i) = (0..lua.len().max(model.len())).find(|&i| lua.get(i) != model.get(i)) {
            let from = i.saturating_sub(8);
            panic!(
                "{what}: line {i}\nlua:   {:#?}\nmodel: {:#?}\nlua errors: {errors:?}",
                &lua[from.min(lua.len())..(i + 4).min(lua.len())],
                &model[from.min(model.len())..(i + 4).min(model.len())]
            );
        }
    }
}
