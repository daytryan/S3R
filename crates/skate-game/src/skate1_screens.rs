//! Skate 1's tutorial-window screens (docs/superpowers/plans/2026-10-14-skate1-p5-screens.md).
//! - hud.rs: the retail HUD manager, which turns HUD.ChangeHUDState into `Act`s.
//! - This file carries the acts out on apt_screen layer 1: the overlay and its loadMovie components (each a screen of
//!   its own, as skate1_online_hud runs them), what the overlay asks of HUDHooks, the FE menus, the lead's FeScreen.
//! - parts.rs: the overlays' lines on their components.
mod hud;
mod parts;

pub(crate) use hud::{FILM, FOLLOW, FREE_SKATE, PHOTO, SKATE, TUTORIAL};
pub(crate) use parts::gesture_name;
use self::{
    hud::{Act, Ctx, Manager, Movie, minimap_shown, trick_shown},
    parts::{Action, Controller, Nis, NisAct, ObjectiveHud, controller_label},
};
use crate::{
    apt_screen::{Screen, Screens},
    apt_text::literal,
    apt_vm::{ObjectKind, Value},
    challenge::cutscene::Cutscene,
    config::Config,
    core_menu_runtime::Key,
    skate_audio::StreamPcm,
    skate1_challenge::{Fe, FeScreen},
    skate1_hud::Skate1Hud,
    skate1_online_hud as online,
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
};

/// The tutorial window's movies (old_screens1.py).
pub(crate) const TUTORIAL_MOVIES: &str = "private/hud-skate1/tutorial/runtime";
/// The HUD manager's tick (RETAIL: 16 ms per 60 Hz tick, hud.rs) on real time; a long frame catches up 0.25 s at most.
const TICK: f32 = 1. / 60.;
const CATCH_UP: f32 = 0.25;
/// Game.GetGameMode. UNTRACED: mode 6 ("tutorial mode"); 1 is the career.
const GAME_MODE: i64 = 1;
/// Natives the tutorial movies call that apt_screen doesn't define (Mission, Progression, Profile, ... it does).
const NATIVES: [&str; 4] = ["Tutorial", "FilmMission", "Load", "FEPauseMenu"];

// Layer 1 screen names, drawn in name order. They start with digits so they sort apart from the other layer-1 users.
const ACTION: &str = "1-action";
const INK: &str = "2-ink";
/// hudbase's hudink: inkbottomright, the objective HUD's splatter (hudink depth 6 under hudobjective 7).
const OBJECTIVE_INK: &str = "3-ink";
const OBJECTIVE: &str = "3-objective";
const TIMER: &str = "4-timer";
/// The best-trick overlay's onlinehud (RegisterOnlineRaceHUD): the competitors' standings.
const ONLINE: &str = "4o-online";
const OVERLAY: &str = "5-overlay";
const CONTROLLER: &str = "6-controller";
const PLAYERS: &str = "6p-skatehud";
const TRICKS: &str = "6t-skatetrick";
const CENTRAL: &str = "7-central";
const FE: &str = "8-fe";
const FE_OUT: &str = "8-fe-out";
const SKATE101: &str = "8-skate101";
const WELCOME: &str = "9-welcome";
const QUICK: &str = "9-quick";
const TIPS: &str = "9-tips";
const CAC: &str = "9-cac";
/// The HUD's screens, which `clear` closes. The FeScreen ones follow the lead's `want`.
const HUD: [&str; 12] = [ACTION, INK, OBJECTIVE_INK, OBJECTIVE, TIMER, ONLINE, OVERLAY, CONTROLLER, PLAYERS, TRICKS, CENTRAL, FE];
/// The NIS HUD state's tutorial screen (parts.rs `Nis`), under the HUD.
const NIS: &str = "0-nis";

/// The sign-up options (Mission.GetSignUpOption 0x8243a930: the type's rows at +384 into table 0x830a3f50, 0 ACCEPT_CAPS,
/// 1 DECLINE, 2 SKIP_LESSON, 3 WATCH_IT_CAPS, 4 PAUSEMENU_EXITTUTORIAL). RETAIL: every type starts [ACCEPT, DECLINE]
/// (0x8233fae0); the Tutorial type (eChallengeTypes 0) sets [ACCEPT, SKIP LESSON] until he_u1_film01's slot is set
/// (0x823faaf8), then [ACCEPT, DECLINE] (0x823462f8).
const SIGN_UP: [&str; 2] = ["ID_COMMON_ACCEPT_CAPS", "ID_COMMON_DECLINE"];
const SIGN_UP_LESSON: [&str; 2] = ["ID_COMMON_ACCEPT_CAPS", "ID_COMMON_SKIP_LESSON"];
/// The end options (Mission.GetEndMissionOptionText 0x8243a848: the type's rows at +344 into table 0x830a3f30,
/// 0 CONTINUE_CAPS, 1 RETRY_CAPS, 2 MEDIA_VIEW_PHOTO_CAPS, 3 REPLAY_EDITREPLAY, ...).
/// RETAIL: the Tutorial type's are [RETRY, CONTINUE] (0x823462f8).
/// UNTRACED: the other types' rows (film, type 7, is [CONTINUE, EDIT REPLAY, RETRY] at 0x823410a0, but nothing here
/// edits a Skate 1 replay). Fallback: Continue / Retry.
const END: [&str; 2] = ["ID_COMMON_CONTINUE_CAPS", "ID_COMMON_RETRY_CAPS"];
const END_LESSON: [&str; 2] = ["ID_COMMON_RETRY_CAPS", "ID_COMMON_CONTINUE_CAPS"];

/// The card's (sign-up, end) options; see [`SIGN_UP`] and [`END`].
fn options(facts: &Facts) -> (&'static [&'static str], &'static [&'static str]) {
    match (facts.card.challenge_type, facts.film01_done) {
        (Some(0), false) => (&SIGN_UP_LESSON, &END_LESSON),
        (Some(0), true) => (&SIGN_UP, &END_LESSON),
        _ => (&SIGN_UP, &END),
    }
}

/// A screen that never reports OutroComplete is removed after this (s).
const OUTRO_TIMEOUT: f32 = 2.;
/// The call without its audio: how long the action overlay rings (s). UNTRACED (lead-approved).
const CALL_HOLD: f32 = 3.;
/// The calls' audio, `<name>.wav`. UNTRACED: the 74-name call table 0x830a27f8; nothing exports it yet.
const CALLS: &str = "private/challenges-skate1/calls";
/// The loaders' constructor-time Load.* texts (Load is bound after load), set "" (UNTRACED tip text).
const LOADER_TEXTS: [&str; 3] = ["mcTextTitle.labelText", "mcTextTrick.labelText", "mcTextDetails.labelText"];
/// cacinfo's SetupScreen / RefreshData CaC.* texts, set "". UNTRACED: FeScreen carries no CaC values.
const CAC_TEXTS: [&str; 4] =
    ["mcName.labelText_name", "mcCACmenu.mcStanceEntry.labelText", "mcCACmenu.mcStyleEntry.labelText", "mcCACmenu.mcGestureEntry.labelText"];

/// The tutorial's visual indicator: model oi000 from visual_treatments.rx2 (research 2.0, P1 export).
const INDICATOR: &str = "private/challenges-skate1/indicator/oi000.glb";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pad {
    Up,
    Down,
    A,
    B,
    Y,
    Rb,
}

/// What the screens post (message hashes RETAIL, research.md 2.0).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// hudloaded 0x03561D20 (the overlay is up).
    Loaded,
    /// hudcomplete 0x3AFB7BB6.
    Complete,
    /// cMsgChallengeObjectiveComplete 0x72A3428B.
    ObjectiveComplete(i64),
    /// cMsgEnableDirectionIndicator 0xFE2F11CA: show / hide the oi000 indicators.
    Indicator(bool),
    /// hudyes 0xE4A8382F.
    Yes,
    /// hudno 0x4EDC56C2.
    No,
    /// hudcontinue 0x1E053B88.
    Continue,
    /// hudretry 0xB17FDD0C (end options' RETRY).
    Retry,
    /// The film menu's Y on film i (Facts::films[i]).
    StartFilm(usize),
    /// postchallengedisplayexit: the FE 49 leaderboard closed.
    DisplayExit,
}

/// The game's side of the screens, every frame. All text already localized.
#[derive(Default)]
pub(crate) struct Facts {
    /// The active / signing-up challenge (Mission.*).
    pub card: Card,
    /// The film menu's missions (FilmMission.*), in menu order.
    pub films: Vec<Card>,
    /// Mission.GetCurrentTime: the HUD timer's seconds left (the lead's).
    pub time: Option<f32>,
    /// Tutorial.ShowController's sequence enum (0 None .. 12 BothTriggers).
    pub controller: i64,
    /// Tutorial.ShowController's gesture enum (eSk8ControllerGesture, 0 None .. 55).
    pub gesture: i64,
    /// he_u1_film01's slot is set (trick display / minimap gate).
    pub film01_done: bool,
    /// The active tutorial has GlobalSpawn rows (gate, type 0).
    pub spawn_rows: bool,
    pub skate: SkateFacts,
    /// Offered challenge keys at world (x, z); None = the minimap's static set.
    pub challenges: Option<Vec<(String, [f32; 2])>>,
    /// Mission.GetDistanceHUDText's (distance, limit) in metres (s1-missions, L-P7); None = no distance HUD.
    pub distance: Option<(f32, i64)>,
    pub best: BestFacts,
}

/// Community Spirit's contest (s1-missions Contest; user 10-15).
#[derive(Default)]
pub(crate) struct BestFacts {
    pub round: i64,
    pub rounds: i64,
    pub forfeit: i64,
    /// Per competitor, the player first: name, this round's score, this round's points, total points, overall rank
    /// (0: none yet).
    pub rows: Vec<(String, i64, i64, i64, i64)>,
}

impl BestFacts {
    /// The rows by overall rank, then this round's score, then competitor. INFERRED: the providers' order
    /// (skate1_online_hud's rank_order isn't the offline contest's).
    fn order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by_key(|&i| {
            let r = &self.rows[i];
            (if r.4 > 0 { r.4 } else { i64::MAX }, std::cmp::Reverse(r.1), i)
        });
        order
    }
}

/// The active / signing-up challenge as the screens show it (Mission.*); text already localized.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Card {
    pub key: String,
    /// The record's ChallengeType (eChallengeTypes; 0 Tutorial); None: no record.
    pub challenge_type: Option<i64>,
    pub icon: String,
    pub title: String,
    pub description: String,
    pub success: String,
    pub failed: String,
    /// The first 3 show.
    pub objectives: Vec<Objective>,
    /// FilmMission.GetPointRequirement.
    pub points: i64,
    /// Mission.GetCurrentScore.
    pub score: i64,
}
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Objective {
    pub title: String,
    pub result: String,
    pub done: bool,
    pub checkbox: bool,
}

#[derive(Default)]
pub(crate) struct SkateFacts {
    pub kind3: bool,
    pub won: bool,
    /// The leader's index in view.names (the lead maps s1-missions' `leader() -> bool`).
    pub leader: usize,
    /// s1-missions' `skate_view()`.
    pub view: online::SkateView,
}

/// An overlay's movie and its loadMovie components (layer name, movie), as its constructor's RegisterUIComponent
/// calls load them (scratch/as). The action overlay is hudframe+0's, not an overlay slot.
fn overlay_movies(movie: Movie) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    const OBJ: (&str, &str) = (OBJECTIVE, "objectivehud");
    const TIME: (&str, &str) = (TIMER, "timerhud");
    Some(match movie {
        Movie::Tutorial => ("hud_tutorial_challenge_overlay", &[OBJ, TIME, (CONTROLLER, "tutorial")]),
        Movie::Film => ("hud_film_challenge_overlay", &[OBJ, TIME]),
        Movie::Photo => ("hud_photo_challenge_overlay", &[OBJ]),
        Movie::Follow => ("hud_followme_challenge_overlay", &[OBJ, TIME]),
        Movie::Skate => ("hud_skate_challenge_overlay", &[TIME, (PLAYERS, "skatehud"), (TRICKS, "skatetrick")]),
        // Its constructor's RegisterTimerHUD / RegisterOnlineRaceHUD (the online export's movie, as online).
        Movie::BestTrick => ("hud_besttrick_challenge_overlay", &[TIME, (ONLINE, "onlinehud")]),
        Movie::Action => return None,
    })
}

/// Skate 101 `name` -> Get101Header / Get101Desc's n and its ids (RETAIL switch 0x82460b10 / 0x82461468, research8 1).
/// "15": Skate.TriggerSkate101 (0x827bf2f8) opens n = 15 directly; the 0x82144ae8 name table has no name for it
/// (s1-missions, RETAIL), so the lead pushes the decimal.
fn skate101(name: &str) -> Option<(i64, &'static str)> {
    Some(match name {
        "NewChallenge" => (3, "NEW_CHALLENGE_COMPLETED"),
        "SequenceScoring" => (4, "SEQUENCE_SCORING"),
        "FilmMenu" => (5, "FILM_MENU"),
        "TrainStation" => (6, "TRAIN_STATION"),
        "15" => (15, "REPEATED_TRICK"),
        _ => return None,
    })
}

/// Mission.GetDistanceHUDText (RETAIL 0x8243b818): [text, trunc(d), d / max, 0 when max < 0]. The timer HUD shows the
/// ratio white below 0.6, red from there (the movie's own colouring). The text is RETAIL format 0x824838f0:
/// - imperial: label 0x413c56da "{0:d} ft" with trunc(d x 3);
/// - metric: label 0x92099686 "{0:d} m" with trunc(d).
/// Retail picks by console region with a profile / menu override (0x8231a078 / 0x82362704). The PC has no console
/// region, so the units come from the S3R setting (hud_runtime::imperial). `text` localizes a label.
fn distance_answer(text: &dyn Fn(&str) -> String, imperial: bool, d: f32, max: i64) -> Json {
    let (label, scale) = if imperial { ("413c56da", 3.) } else { ("92099686", 1.) };
    let shown = (d * scale).trunc() as i64;
    let ratio = if max < 0 { 0. } else { d / max as f32 };
    json!([literal(&text(label).replace("{0:d}", &shown.to_string())), d.trunc() as i64, ratio])
}

/// welcomeloader / tipsloader leave by `to_game`; the others by `outro` (their timelines, scratch/as).
fn outro_label(name: &str) -> &'static str {
    if matches!(name, WELCOME | TIPS) { "to_game" } else { "outro" }
}

/// A want whose screen can't show sets every flag it would have set (Review Focus 2).
fn missing(want: &Fe, fe: &mut FeScreen) {
    fe.intro_done = true;
    match want {
        Fe::CaCDetails => fe.confirmed = true,
        Fe::Skate101(_) => fe.closed = true,
        _ => {}
    }
}

/// An FE push's movie (None: no movie) and its rows. UNTRACED: 74 PressA has no movie; 24 MedicalBill isn't exported
/// (the script's own timer carries on).
fn fe_movie(id: u8, facts: &Facts) -> Option<(Option<&'static str>, &'static [&'static str])> {
    let (sign_up, end) = options(facts);
    match id {
        65 => Some((Some("challenge_signup"), sign_up)),
        // tutorial_end's own rows: won [CONTINUE, RETRY], lost [RETRY, QUIT] (scratch/as).
        73 => Some((Some("tutorial_end"), &["", ""])),
        23 => Some((Some("endchallengeoptions"), end)),
        74 => Some((None, &[""])),
        // Community Spirit's round leaderboard (the online export's movie).
        49 => Some((Some("leaderboard"), &[""])),
        _ => None,
    }
}

/// What an FE menu's row (None: B) of `options` posts, as the mission handlers take it:
/// - sign-up: hudyes accept, hudno decline or skip lesson (B: decline, UNTRACED);
/// - lesson end: RETRY hudyes, CONTINUE hudno (RETAIL), QUIT hudno (INFERRED);
/// - end options: CONTINUE hudcontinue, RETRY hudretry;
/// - PressA: hudcontinue (INFERRED);
/// - LeaderBoard: postchallengedisplayexit. UNTRACED: what closes it offline (online it counts down). Fallback: A.
/// B does nothing on the others.
fn fe_pick(id: u8, won: bool, row: Option<usize>, options: &[&str]) -> Option<Event> {
    let option = row.and_then(|r| options.get(r).copied());
    match (id, row) {
        (65, Some(_)) if option == Some("ID_COMMON_ACCEPT_CAPS") => Some(Event::Yes),
        (65, _) => Some(Event::No),
        (73, Some(row)) => Some(if (row == 1) == won { Event::Yes } else { Event::No }),
        (23, Some(_)) if option == Some("ID_COMMON_RETRY_CAPS") => Some(Event::Retry),
        (23, Some(_)) => Some(Event::Continue),
        (74, Some(_)) => Some(Event::Continue),
        (49, Some(_)) => Some(Event::DisplayExit),
        _ => None,
    }
}

/// The FE menu up: its push, highlighted row and its rows' option ids.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Menu {
    id: u8,
    highlight: usize,
    options: &'static [&'static str],
}

#[derive(Resource, Default)]
pub(crate) struct Skate1Screens {
    manager: Manager,
    /// Real time not yet spent on manager ticks.
    ticks: f32,
    /// Acts for the next frame: objective marks, the sign-up prompt, presses, the call's action overlay.
    pending: Vec<Act>,
    /// The overlay slot, and whether its hudloaded is due next frame.
    overlay: Option<Movie>,
    loaded: bool,
    /// The film menu's count, for presses between frames.
    films: usize,
    /// The (trick display, minimap) gate; None: free skate's ([`Self::gate`]).
    gate: Option<(bool, bool)>,
    objective: ObjectiveHud,
    controller: Controller,
    /// The running scene's NIS tutorial screen.
    nis: Nis,
    action: Action,
    central: online::Central,
    skate: online::SkateParts,
    /// FE pushes waiting to open, and the FE menu up.
    fe_queue: VecDeque<u8>,
    menu: Option<Menu>,
    /// The FeScreen want shown, and its layer name while it shows.
    opened: Option<Fe>,
    shown: Option<&'static str>,
    /// The call: when it ends (on `clock`), and its audio.
    call: Option<f32>,
    call_audio: Option<Entity>,
    /// Screens playing their outro: (name, removed at, the want whose `closed` that sets).
    outros: Vec<(String, f32, Option<Fe>)>,
    /// Real time, for the call and the outros.
    clock: f32,
    /// The sign-up prompt is up ([`Self::sign_up_prompt`]).
    prompt: bool,
}

fn ctx(facts: &Facts) -> Ctx<'_> {
    Ctx {
        key: &facts.card.key,
        films: facts.films.len(),
        film01_done: facts.film01_done,
        spawn_rows: facts.spawn_rows,
        kind3: facts.skate.kind3,
        skate_won: facts.skate.won,
        best: (facts.best.round, facts.best.rounds, facts.best.rows.first().is_some_and(|r| r.4 == 1)),
    }
}

impl Skate1Screens {
    /// s1-missions' tests: one frame with no screens open and no assets.
    #[cfg(test)]
    pub(crate) fn step_without_assets(&mut self, facts: &Facts) -> Vec<Event> {
        self.step(&mut BTreeMap::new(), &tests::no_assets(), facts, TICK)
    }

    pub(crate) fn change_hud_state(&mut self, state: u8, sub: i64) {
        self.manager.change(state, sub);
    }

    pub(crate) fn mark_objective_complete(&mut self, n: i64) {
        let acts = self.manager.mark_objective(n);
        self.pending.extend(acts);
    }

    /// The type-3 sign-up prompt (research 1.1 "Type 3": action overlay state 14, "CHALLENGE SIGNUP" + the offered
    /// challenge's icon), set each frame; only a change reaches the overlay.
    pub(crate) fn sign_up_prompt(&mut self, on: bool) {
        if std::mem::replace(&mut self.prompt, on) != on {
            self.pending.push(Act::Set(Movie::Action, if on { 14 } else { 0 }));
        }
    }

    #[cfg(test)]
    pub(crate) fn sign_up_prompt_shown(&self) -> bool {
        self.prompt
    }

    /// HUD.SetWinState (vt+116, INFERRED): what Mission.GetWinState answers the end screens.
    pub(crate) fn set_win_state(&mut self, won: bool) {
        self.manager.won = won;
    }

    /// The film replay ended (cMsgFilmReplayAutoplayFinished; hud.rs `replay_finished`).
    pub(crate) fn film_replay_finished(&mut self) {
        self.manager.replay_finished();
    }

    /// An FE menu (end options 23, ...) is up: it takes the controller's presses (bug 5, preview 3).
    pub(crate) fn holds_menu(&self) -> bool {
        self.menu.is_some()
    }

    pub(crate) fn press(&mut self, world: &mut World, pad: Pad) -> bool {
        if !world.contains_resource::<FeScreen>() {
            return false;
        }
        world.resource_scope(|world, mut fe: Mut<FeScreen>| {
            let Some(mut screens) = world.get_resource_mut::<Screens>() else { return false };
            self.press_in(&mut screens.layer(1).open, &mut fe, pad)
        })
    }

    pub(crate) fn frame(&mut self, world: &mut World, facts: &Facts) -> Vec<Event> {
        let dt = world.get_resource::<Time<Real>>().map_or(0., |t| t.delta_secs());
        let Some(root) = world.get_resource::<Config>().map(|c| c.asset_root.clone()) else { return Vec::new() };
        let scene = world.get_resource::<Cutscene>().filter(|c| c.root().is_some()).map(Cutscene::overlay);
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return Vec::new() };
        let events = self.step(&mut screens.layer(1).open, &root, facts, dt);
        if let Err(error) = self.nis_step(&mut screens.layer(1).open, &root, scene) {
            warn!("Skate 1 screens: NIS overlay: {error}");
        }
        let gate = self.gate(&ctx(facts));
        if let Some(mut hud) = world.get_resource_mut::<Skate1Hud>() {
            hud.gate = Some(gate);
            // The minimap draws icons: each offered key's (challenge_icon; film missions have none).
            hud.challenges = facts.challenges.as_ref().map(|offered| {
                offered.iter().filter_map(|(key, at)| Some((challenge_icon(key)?, *at))).collect()
            });
        }
        events
    }

    pub(crate) fn clear(&mut self, world: &mut World) {
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            close_hud(&mut screens.layer(1).open);
        }
        self.reset();
        if let Some(mut hud) = world.get_resource_mut::<Skate1Hud>() {
            (hud.gate, hud.challenges) = (None, None);
        }
    }

    /// Leaving the Skate 1 map: [`Self::clear`] plus the scene's NIS screen, which a challenge's end keeps (review
    /// 10-15 minor 8).
    pub(crate) fn unload(&mut self, world: &mut World) {
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            screens.layer(1).open.remove(NIS);
        }
        self.nis = Nis::default();
        self.clear(world);
    }

    /// A fresh HUD half; the FeScreen half and the scene's NIS screen keep following theirs.
    fn reset(&mut self) {
        let fe = (self.opened.take(), self.shown.take(), self.call.take(), self.call_audio.take(), std::mem::take(&mut self.outros), self.clock);
        let nis = std::mem::take(&mut self.nis);
        *self = Self::default();
        (self.opened, self.shown, self.call, self.call_audio, self.outros, self.clock) = fe;
        self.nis = nis;
    }

    /// The scene's overlay id (`scene`, None without one) on the NIS tutorial screen (parts.rs `Nis`). It loads as the
    /// HUD's movies do; SetState(1) is tutorialscreenimp's gotoAndPlay(1).
    fn nis_step(&mut self, open: &mut BTreeMap<String, Screen>, root: &Path, scene: Option<u8>) -> Result<(), String> {
        match self.nis.step(scene) {
            Some(NisAct::Open(movie)) => {
                load(open, root, NIS, movie, &[])?;
                match open.get_mut(NIS) {
                    Some(s) => s.call("SetState", &[json!(1)]).map(drop),
                    None => Ok(()),
                }
            }
            Some(NisAct::Close) => {
                open.remove(NIS);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// The gate as of now. UNTRACED: the gate before any HUD state; free skate's (type 32).
    fn gate(&self, c: &Ctx) -> (bool, bool) {
        self.gate.unwrap_or((trick_shown(true, 32, c), minimap_shown(true, c)))
    }

    /// One frame of the HUD:
    /// - hudloaded for last frame's overlay;
    /// - the pending acts;
    /// - the manager's 60 Hz ticks;
    /// - the answers, then the acts carried out;
    /// - a queued FE menu, once nothing holds it;
    /// - what the overlay's own frames asked of HUDHooks;
    /// - centralhud's timers.
    /// An act that fails is logged and the rest still run.
    fn step(&mut self, open: &mut BTreeMap<String, Screen>, root: &Path, facts: &Facts, dt: f32) -> Vec<Event> {
        let c = ctx(facts);
        self.films = facts.films.len();
        let mut events = Vec::new();
        let mut acts = std::mem::take(&mut self.pending);
        // UNTRACED timing: hudloaded the frame after the overlay opened, also when its movie is missing.
        if std::mem::take(&mut self.loaded)
            && let Some(movie) = self.overlay
        {
            events.push(Event::Loaded);
            acts.extend(self.manager.loaded(movie, &c));
        }
        self.ticks = (self.ticks + dt).min(CATCH_UP);
        while self.ticks >= TICK {
            self.ticks -= TICK;
            acts.extend(self.manager.tick(&c));
        }
        let answers = self.answers(open, facts);
        for name in HUD {
            if let Some(s) = open.get_mut(name) {
                for (key, value) in &answers {
                    s.answer(key, value.clone());
                }
            }
        }
        for act in acts {
            if let Err(error) = self.apply(open, root, facts, &answers, act, &mut events) {
                warn!("Skate 1 screens: {error}");
            }
        }
        if self.menu.is_none()
            && !self.holds_fe()
            && let Some(id) = self.fe_queue.pop_front()
            && let Err(error) = self.open_fe(open, root, facts, &answers, id)
        {
            warn!("Skate 1 screens: FE {id}: {error}");
        }
        if let Err(error) = self.frame_calls(open, facts, dt) {
            warn!("Skate 1 screens: {error}");
        }
        events
    }

    fn apply(
        &mut self,
        open: &mut BTreeMap<String, Screen>,
        root: &Path,
        facts: &Facts,
        answers: &[(String, Json)],
        act: Act,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let c = ctx(facts);
        match act {
            Act::Overlay(movie) => self.open_overlay(open, root, answers, movie)?,
            Act::Set(Movie::Action, state) => {
                // Whoever set it, the overlay shows the prompt only in state 14 (review 10-15 minor 6).
                self.prompt = state == 14;
                self.action_state(open, root, answers, state, &facts.card.icon)?
            }
            Act::Set(movie, state) => self.overlay_state(open, facts, movie, state)?,
            Act::Trick(on) => self.gate = Some((on, self.gate(&c).1)),
            Act::Minimap(on) => self.gate = Some((self.gate(&c).0, on)),
            Act::Fe(id) => self.fe_queue.push_back(id),
            Act::Post(event) => events.push(event),
            Act::FilmMenu(i) => self.film_menu(open, facts, i)?,
        }
        Ok(())
    }

    /// 0x82471000(idx), the overlay slot:
    /// - A new overlay closes the old one and its components.
    /// - It then opens centralhud (once), the components, then the overlay, as RegisterUIComponent loads them.
    /// - None unloads; the same overlay stays.
    fn open_overlay(
        &mut self,
        open: &mut BTreeMap<String, Screen>,
        root: &Path,
        answers: &[(String, Json)],
        movie: Option<Movie>,
    ) -> Result<(), String> {
        self.loaded = movie.is_some();
        if movie == self.overlay {
            return Ok(());
        }
        self.overlay = movie;
        for name in [OVERLAY, OBJECTIVE, TIMER, ONLINE, CONTROLLER, PLAYERS, TRICKS] {
            open.remove(name);
        }
        (self.objective, self.controller, self.skate) = Default::default();
        let Some((overlay, components)) = movie.and_then(overlay_movies) else { return Ok(()) };
        if !open.contains_key(CENTRAL) {
            load(open, root, CENTRAL, "centralhud", answers)?;
        }
        // hudbase's hudink, loaded with the HUD in retail: here once, with the first overlay.
        if !open.contains_key(OBJECTIVE_INK) {
            load(open, root, OBJECTIVE_INK, "inkbottomright", answers)?;
        }
        for (name, stem) in components {
            load(open, root, name, stem, answers)?;
        }
        load(open, root, OVERLAY, overlay, answers)?;
        if let Some(tricks) = open.get_mut(TRICKS) {
            self.skate.open(tricks)?;
        }
        Ok(())
    }

    /// hudframe+0's action overlay: actionoverlay with inkaction behind it. Both load on its first state other than
    /// Off.
    fn action_state(
        &mut self,
        open: &mut BTreeMap<String, Screen>,
        root: &Path,
        answers: &[(String, Json)],
        state: i64,
        icon: &str,
    ) -> Result<(), String> {
        if state != 0 && !open.contains_key(ACTION) {
            load(open, root, INK, "inkaction", answers)?;
            load(open, root, ACTION, "actionoverlay", answers)?;
        }
        let Some(mut a) = open.remove(ACTION) else { return Ok(()) };
        let done = self.action.set_state(&mut a, open.get_mut(INK), state, icon);
        open.insert(ACTION.into(), a);
        done
    }

    /// The loaded overlay's SetState(state), then:
    /// - its lines on its components (the tutorial's controller, S.K.A.T.E.'s players and trick list);
    /// - what it asked of HUDHooks.
    /// An overlay that isn't loaded does nothing.
    fn overlay_state(&mut self, open: &mut BTreeMap<String, Screen>, facts: &Facts, movie: Movie, state: i64) -> Result<(), String> {
        if self.overlay != Some(movie) {
            return Ok(());
        }
        let Some(o) = open.get_mut(OVERLAY) else { return Ok(()) };
        if o.has("SetState") {
            o.call("SetState", &[json!(state)])?;
        }
        let calls = o.take_calls();
        if movie == Movie::Tutorial
            && let Some(c) = open.get_mut(CONTROLLER)
        {
            self.controller.set_state(c, state, controller_label(facts.controller), gesture_name(facts.gesture))?;
        }
        if let (Some(mut p), Some(mut t)) = (open.remove(PLAYERS), open.remove(TRICKS)) {
            let done = self.skate.state(&mut p, &mut t, state, &facts.skate.view);
            open.insert(PLAYERS.into(), p);
            open.insert(TRICKS.into(), t);
            done?;
        }
        self.hooks(open, facts, calls)
    }

    /// The film menu on film i: hudobjective Setup('film') + UpdateObjectives on its card.
    /// UNTRACED: what else the C++ refresh calls. Fallback: Play('introInitiate') when objectivehud isn't showing yet.
    fn film_menu(&mut self, open: &mut BTreeMap<String, Screen>, facts: &Facts, i: usize) -> Result<(), String> {
        let Some(s) = open.get_mut(OBJECTIVE) else { return Ok(()) };
        let card = facts.films.get(i).cloned().unwrap_or_default();
        let menu = (i, facts.films.len());
        let showing = s.clip("").is_some_and(|root| matches!(s.vm.get(root, "_visible"), Value::Bool(true)));
        self.objective.call(s, "Setup", &[json!("film")], &card, menu)?;
        self.objective.call(s, "UpdateObjectives", &[], &card, menu)?;
        if !showing {
            self.objective.call(s, "Play", &[json!("introInitiate")], &card, menu)?;
        }
        Ok(())
    }

    /// What the overlay asked of HUDHooks:
    /// - its own functions go to centralhud;
    /// - ObjectiveHUD goes to objectivehud's port;
    /// - TimerHUD.Show goes to timerhud;
    /// - InkHUD.Play('objective', clip) plays inkbottomright's mInk (hudink's Play; other ink types do nothing).
    fn hooks(&mut self, open: &mut BTreeMap<String, Screen>, facts: &Facts, calls: Vec<(String, Vec<Json>)>) -> Result<(), String> {
        for (name, args) in calls {
            let Some((object, method)) = name.split_once('.') else { continue };
            match (object, method) {
                ("HUDHooks", _) => {
                    let args = localized(open, args);
                    if let Some(central) = open.get_mut(CENTRAL) {
                        self.central.hook(central, method, &args)?;
                    }
                }
                ("ObjectiveHUD", _) => {
                    if let Some(s) = open.get_mut(OBJECTIVE) {
                        let film = self.manager.film_menu().unwrap_or((0, facts.films.len()));
                        self.objective.call(s, method, &args, &facts.card, film)?;
                    }
                }
                ("TimerHUD", "Show") | ("OnlineRaceHUD", "Show") => {
                    if let Some(t) = open.get_mut(if object == "TimerHUD" { TIMER } else { ONLINE }) {
                        t.call("Show", &args)?;
                    }
                }
                ("InkHUD", "Play") => {
                    // Playtest 10-15 diagnostic (the splatter still missing). ponytail: remove once a run settles it.
                    info!("Skate 1 ink: Play {args:?} ink_loaded={}", open.contains_key(OBJECTIVE_INK));
                    if let (Some("objective"), Some(clip), Some(ink)) =
                        (args.first().and_then(Json::as_str), args.get(1).and_then(Json::as_str), open.get_mut(OBJECTIVE_INK))
                    {
                        online::go(ink, "mInk", clip)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The end of the frame:
    /// - what the overlay's frame actions asked of HUDHooks;
    /// - the other HUD screens' calls, dropped (they are answers only);
    /// - centralhud's timers.
    fn frame_calls(&mut self, open: &mut BTreeMap<String, Screen>, facts: &Facts, dt: f32) -> Result<(), String> {
        if let Some(o) = open.get_mut(OVERLAY) {
            let calls = o.take_calls();
            self.hooks(open, facts, calls)?;
        }
        for name in HUD.into_iter().filter(|&n| n != OVERLAY) {
            if let Some(s) = open.get_mut(name) {
                s.take_calls();
            }
        }
        if let Some(central) = open.get_mut(CENTRAL) {
            self.central.update(central, dt, facts.time.unwrap_or(0.))?;
        }
        Ok(())
    }

    /// What the HUD movies ask (natives listed in the plan's research), as of now. Text is literal.
    fn answers(&self, open: &BTreeMap<String, Screen>, facts: &Facts) -> Vec<(String, Json)> {
        let (card, won, view) = (&facts.card, self.manager.won, &facts.skate.view);
        let mut a: Vec<(String, Json)> = [
            ("Game.GetGameMode", json!(GAME_MODE)),
            ("Mission.GetIsOnlineMission", json!(false)),
            ("Mission.GetIsLocalPlayerObserver", json!(false)),
            ("Mission.GetTitle", json!(literal(&card.title))),
            ("Mission.GetDescription", json!(literal(&card.description))),
            ("Mission.GetChallengeTypeName", json!(type_name(card.challenge_type))),
            // INFERRED: only GetChallengeIconString (the sign-up's) is traced; the running challenge's icon is the same.
            ("Mission.GetIconString", json!(card.icon)),
            ("Mission.GetChallengeIconString", json!(card.icon)),
            ("Mission.GetWinState", json!(won)),
            ("Progression.IsActiveChallengeCompleted", json!(won)),
            ("Progression.IsActiveChallengeCoverageMoment", json!(false)),
            ("Profile.GetTotalCash", json!(0)),
            ("Mission.GetNumSignUpOptions", json!(options(facts).0.len())),
            ("Mission.GetNumEndMissionOptions", json!(options(facts).1.len())),
            ("Mission.GetCurrentTime", json!(facts.time.unwrap_or(0.))),
            ("Mission.GetTimerHUDText", online::timer_answer(facts.time)),
            ("Mission.GetCurrentScore", json!(card.score)),
            ("FilmMission.GetPointRequirement", json!(card.points)),
            ("Tutorial.GetControllerSequenceString", json!(controller_label(facts.controller))),
            ("Tutorial.GetControllerGestureString", json!(gesture_name(facts.gesture))),
            ("SkateMission.GetNumPlayers", json!(view.names.len())),
            ("SkateMission.GetPlayerLeader", json!(facts.skate.leader)),
            ("SkateMission.GetActivePlayerIndex", json!(view.active)),
            ("SKATEMission.GetActivePlayerIndex", json!(view.active)),
            ("SKATEMission.IsTrickMirrored", json!(false)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        if let (Some((d, max)), Some(o)) = (facts.distance, open.get(OVERLAY)) {
            let text = |label: &str| o.bindings.movie.text_assets.localize(label);
            a.push(("Mission.GetDistanceHUDText".into(), distance_answer(&text, crate::hud_runtime::imperial(), d, max)));
        }
        // UNTRACED: career cash and sponsorships. Fallback: 0.
        for part in ["", "Board", "Shoe", "Truck", "Wheel"] {
            a.push((format!("Progression.GetActiveMission{part}Cash"), json!(0)));
        }
        for part in ["Board", "Shoe", "Truck", "Wheel"] {
            a.push((format!("Progression.Get{part}SponsorshipStatus"), json!(0)));
        }
        a.extend(options(facts).0.iter().enumerate().map(|(i, id)| (format!("Mission.GetSignUpOption({i})"), json!(id))));
        a.extend(options(facts).1.iter().enumerate().map(|(i, id)| (format!("Mission.GetEndMissionOptionText({i})"), json!(id))));
        for (i, name) in view.names.iter().enumerate() {
            a.push((format!("SkateMission.GetPlayerName({i})"), json!(name)));
            a.push((format!("SKATEMission.GetPlayerName({i})"), json!(name)));
        }
        // Community Spirit (user 10-15): the best-trick overlay, onlinehud's rows and the FE 49 leaderboard, as
        // skate1_online_hud answers them online.
        let best = &facts.best;
        let order = best.order();
        a.extend(
            [
                ("BestTrickMission.GetCurrentRound", json!(best.round)),
                ("BestTrickMission.GetForfeitTimeLeft", json!(best.forfeit)),
                ("Mission.GetIsPartyPlayMission", json!(false)),
                ("Mission.GetIsRankedMission", json!(false)),
                ("Mission.GetLocalPlayerIndex", json!(0)),
                ("Mission.GetNumberOfPlayers", json!(best.rows.len())),
            ]
            .map(|(k, v)| (k.to_owned(), v)),
        );
        for (row, &i) in order.iter().enumerate() {
            let (name, score, ..) = &best.rows[i];
            a.push((format!("Mission.GetPlayersData({})", row + 1), json!([row + 1, name, score.to_string(), "", i, 0, false])));
        }
        a.extend(board_answers(&card.title, best, &order));
        // UNTRACED: SkateMission.GetPlayerScore for the other players (unanswered).
        a.push((format!("SkateMission.GetPlayerScore({})", view.active), json!(view.letters)));
        // The S.K.A.T.E. overlay's format strings, as skate1_online_hud answers them, in the overlay's language.
        if let Some(o) = open.get(OVERLAY) {
            let name = |i: usize| view.names.get(i).cloned().unwrap_or_default();
            for (id, value) in [
                ("ID_MISSION_START_TURN_NOTIFICATION", name(view.active)),
                ("ID_SKATE_IS_LEADER", name(facts.skate.leader)),
                ("ID_SKATE_COPY_TRICK", name(view.active)),
                ("ID_SKATE_PLAYER_BAILED", name(view.active)),
                ("ID_SKATE_AI_IS_UP", name(view.active)),
                ("ID_MISSION_ROUND_NOTIFICATION", facts.best.round.to_string()),
                ("ID_COMMON_MISSION_LEFT_AREA_WARNING", facts.best.forfeit.to_string()),
            ] {
                let text = online::fill(&o.bindings.movie.text_assets.localize(id), &value);
                a.push((format!("FELanguage.GetLocalizedFormatString({id},1)"), json!(literal(&text))));
            }
        }
        a
    }

    /// A press goes to one screen only (Review Focus 1). In order:
    /// - a shown Skate 101 takes every press, and A closes it (UNTRACED dismiss button: A);
    /// - CaCDetails takes every press, and A confirms;
    /// - an FE menu takes every press;
    /// - then the film menu and free skate's RB.
    fn press_in(&mut self, open: &mut BTreeMap<String, Screen>, fe: &mut FeScreen, pad: Pad) -> bool {
        if let (Some(Fe::Skate101(_)), Some(name)) = (self.opened.clone(), self.shown) {
            if pad == Pad::A {
                if let Some(s) = open.get_mut(name)
                    && let Err(error) = s.input(Key::Accept)
                {
                    warn!("{name}: {error}");
                }
                self.close_shown(open, fe.want.clone());
            }
            return true;
        }
        if self.opened == Some(Fe::CaCDetails) {
            fe.confirmed |= pad == Pad::A;
            return true;
        }
        if let Some(taken) = self.menu_press(open, pad) {
            return taken;
        }
        let (taken, acts) = self.manager.press(pad, &Ctx { films: self.films, ..Ctx::default() });
        self.pending.extend(acts);
        taken
    }

    /// One frame of FeScreen, after apt_screen::render advanced the screens (so intro_done lands on the IntroComplete
    /// frame). Returns the call's audio when one starts.
    fn fe_step(&mut self, open: &mut BTreeMap<String, Screen>, root: &Path, fe: &mut FeScreen, dt: f32) -> Option<StreamPcm> {
        self.clock += dt;
        let clock = self.clock;
        let mut reports = Vec::new();
        self.outros.retain(|(name, until, report)| {
            let done = open.get(name).is_none_or(|s| s.bindings.outro_complete) || clock >= *until;
            if done {
                open.remove(name);
                reports.push(report.clone());
            }
            !done
        });
        if fe.want.is_some() && reports.contains(&fe.want) {
            fe.closed = true;
        }
        let mut audio = None;
        if fe.want != self.opened {
            // RETAIL (research9 3): no IntroComplete gate. The shown screen plays its outro over what comes next.
            self.close_shown(open, None);
            if self.call.take().is_some() {
                self.pending.push(Act::Set(Movie::Action, 0));
            }
            self.opened = fe.want.clone();
            match self.open_fe_screen(open, root, fe) {
                Ok(pcm) => audio = pcm,
                Err(error) => {
                    warn!("Skate 1 screens: {error}");
                    if let Some(want) = fe.want.clone() {
                        missing(&want, fe);
                    }
                }
            }
        }
        if let Some(name) = self.shown
            && open.get(name).is_some_and(|s| s.bindings.intro_complete)
        {
            fe.intro_done = true;
        }
        // A shown screen that render stopped (removed from `open`) can never report: carry on as if its movie were
        // missing, or the frontend and its time pause hold forever (review 10-15 I2).
        if self.shown.is_some_and(|name| !open.contains_key(name))
            && let Some(want) = fe.want.clone()
        {
            missing(&want, fe);
        }
        if self.call.is_some_and(|end| clock >= end) {
            self.call = None;
            fe.closed = true;
            self.pending.push(Act::Set(Movie::Action, 0));
        }
        audio
    }

    fn open_fe_screen(&mut self, open: &mut BTreeMap<String, Screen>, root: &Path, fe: &mut FeScreen) -> Result<Option<StreamPcm>, String> {
        let Some(want) = fe.want.clone() else { return Ok(None) };
        let (name, stem, texts): (&'static str, &str, &[&str]) = match &want {
            Fe::LoadingWelcome => (WELCOME, "welcomeloader", &LOADER_TEXTS),
            Fe::LoadingQuick => (QUICK, "quickloader", &[]),
            Fe::LoadingTips => (TIPS, "tipsloader", &LOADER_TEXTS),
            Fe::CaCDetails => (CAC, "cacinfo", &CAC_TEXTS),
            Fe::Skate101(_) => (SKATE101, "skate101", &[]),
            Fe::Call(key) => {
                // The ring is action overlay state 13. UNTRACED: shown for the whole call (fallback), not the ring only.
                self.pending.push(Act::Set(Movie::Action, 13));
                fe.intro_done = true;
                return Ok(self.start_call(root, key));
            }
        };
        let mut answers: Vec<(String, Json)> =
            ["Load.GetLoadingTipsHeader", "Load.GetLoadingTipsFooter", "Load.GetLoadingTipsDescription", "Load.GetImageString", "FEPauseMenu.GetTip"]
                .map(|k| (k.to_owned(), json!("")))
                .into();
        let lesson = match &want {
            Fe::Skate101(n) => match skate101(n) {
                Some(lesson) => Some(lesson),
                // UNTRACED: a 101 the table lacks; it shows nothing and closes at once.
                None => {
                    missing(&want, fe);
                    return Ok(None);
                }
            },
            _ => None,
        };
        if let Some((n, id)) = lesson {
            answers.push((format!("Tutorial.Get101Header({n})"), json!(format!("ID_SKATE_101_{id}_TITLE"))));
            answers.push((format!("Tutorial.Get101Desc({n})"), json!(format!("ID_SKATE_101_{id}_DESC"))));
        }
        load(open, root, name, stem, &answers)?;
        let Some(s) = open.get_mut(name) else {
            missing(&want, fe);
            return Ok(None);
        };
        self.shown = Some(name);
        for path in texts {
            online::put(s, path, "text", Value::Text(String::new()))?;
        }
        match &want {
            // SetState(1) shows the tip panel as text only (`justtext`).
            Fe::LoadingTips => {
                s.call("SetState", &[json!(1)])?;
                s.play("intro")?;
            }
            Fe::Skate101(_) => {
                s.call("SetState", &[json!(lesson.map_or(0, |l| l.0))])?;
                s.play("intro")?;
            }
            // cacinfo's constructor doesn't stop its timeline: the intro plays on its own.
            Fe::CaCDetails => {}
            _ => s.play("intro")?,
        }
        Ok(None)
    }

    /// The call's audio: `calls/<key>.wav` closes the call at its end. Without a wav the overlay holds CALL_HOLD.
    fn start_call(&mut self, root: &Path, key: &str) -> Option<StreamPcm> {
        let file = root.join(CALLS).join(format!("{key}.wav"));
        let pcm = match crate::challenge::movie::read_wav(&file) {
            Ok(pcm) => Some(pcm),
            Err(_) if !file.is_file() => None,
            Err(error) => {
                warn!("{}: {error}", file.display());
                None
            }
        };
        self.call = Some(self.clock + pcm.as_ref().map_or(CALL_HOLD, |p| p.duration().as_secs_f32()));
        pcm
    }

    /// The shown FeScreen screen plays its outro as "<name>-out". It is removed at OutroComplete or after
    /// OUTRO_TIMEOUT. `report`: the want whose `closed` that sets. A screen already gone reports next frame.
    fn close_shown(&mut self, open: &mut BTreeMap<String, Screen>, report: Option<Fe>) {
        let Some(name) = self.shown.take() else { return };
        let out = format!("{name}-out");
        if let Some(mut s) = open.remove(name) {
            if let Err(error) = s.play(outro_label(name)) {
                warn!("{name}: {error}");
            }
            open.insert(out.clone(), s);
        }
        self.outros.push((out, self.clock + OUTRO_TIMEOUT, report));
    }

    /// A Skate 101 shown or playing its outro holds the FE pushes behind it (FE 71's slot, research9 2).
    fn holds_fe(&self) -> bool {
        (matches!(self.opened, Some(Fe::Skate101(_))) && self.shown.is_some())
            || self.outros.iter().any(|(name, ..)| name.starts_with(SKATE101))
    }

    /// Opens FE push `id`. A missing movie still leaves the menu answering A / B (Review Focus 2).
    fn open_fe(&mut self, open: &mut BTreeMap<String, Screen>, root: &Path, facts: &Facts, answers: &[(String, Json)], id: u8) -> Result<(), String> {
        let Some((stem, options)) = fe_movie(id, facts) else { return Ok(()) };
        self.menu = Some(Menu { id, highlight: 0, options });
        let Some(stem) = stem else { return Ok(()) };
        load(open, root, FE, stem, answers)?;
        let Some(s) = open.get_mut(FE) else { return Ok(()) };
        if id == 73 {
            // Its constructor reads Tutorial.* before Tutorial is bound: title and description go in now.
            let card = &facts.card;
            let text = if self.manager.won { &card.success } else { &card.failed };
            online::put(s, "mcTitle.mcHeader.labelText", "text", Value::Text(literal(&card.title)))?;
            online::put(s, "mcBodyText.mcDescription.labelText", "text", Value::Text(literal(text)))?;
        }
        // endchallengeoptions' constructor doesn't stop its timeline: its intro plays on its own. The leaderboard's
        // Initialise starts it (skate1_online_hud::show_board).
        if id == 49 {
            s.call("Initialise", &[])?;
        } else if id != 23 {
            s.play("intro")?;
        }
        Ok(())
    }

    /// The FE menu's press, if one is up. Every press is taken.
    /// - Up / Down move the highlight (clamped) and the movie's picker.
    /// - A picks the highlight.
    /// - B answers only where the handlers take it.
    /// UNTRACED: Skate 1 MenuPicker's select call; the highlight is tracked here.
    fn menu_press(&mut self, open: &mut BTreeMap<String, Screen>, pad: Pad) -> Option<bool> {
        let menu = self.menu.as_mut()?;
        let key = match pad {
            Pad::Up => Some(Key::Up),
            Pad::Down => Some(Key::Down),
            Pad::A => Some(Key::Accept),
            _ => None,
        };
        if let (Some(key), Some(s)) = (key, open.get_mut(FE))
            && let Err(error) = s.input(key)
        {
            warn!("FE {}: {error}", menu.id);
        }
        let row = match pad {
            Pad::Up => {
                menu.highlight = menu.highlight.saturating_sub(1);
                return Some(true);
            }
            Pad::Down => {
                menu.highlight = (menu.highlight + 1).min(menu.options.len() - 1);
                return Some(true);
            }
            Pad::A => Some(menu.highlight),
            Pad::B => None,
            _ => return Some(true),
        };
        let id = menu.id;
        if let Some(event) = fe_pick(id, self.manager.won, row, menu.options) {
            self.pending.push(Act::Post(event));
            self.close_menu(open);
        }
        Some(true)
    }

    /// The menu plays its outro as "8-fe-out" (removed at OutroComplete or after OUTRO_TIMEOUT).
    fn close_menu(&mut self, open: &mut BTreeMap<String, Screen>) {
        self.menu = None;
        if let Some(mut s) = open.remove(FE) {
            if let Err(error) = s.play("outro") {
                warn!("FE: {error}");
            }
            open.insert(FE_OUT.into(), s);
            self.outros.push((FE_OUT.into(), self.clock + OUTRO_TIMEOUT, None));
        }
    }
}

/// FE 49's FELeaderboard answers: Community Spirit's round board (RETAIL provider state 0: #, Gamertag, Score, Points;
/// skate1_online_hud::board_answers' best-trick rows). UNTRACED offline: the details' spot and the timer text.
/// Fallback: "ROUND n" and none.
/// ponytail: the round board only; online's refresh to the standings (CheckForRefresh) isn't followed.
fn board_answers(title: &str, best: &BestFacts, order: &[usize]) -> Vec<(String, Json)> {
    let headers = ["#", "Gamertag:", "Score:", "Points:"];
    let mut out: Vec<(String, Json)> = [
        ("FELeaderboard.GetLeaderBoardTitle", json!(literal(title))),
        ("FELeaderboard.GetLeaderBoardTitleDetails", json!(literal(&format!("ROUND {}", best.round)))),
        ("FELeaderboard.GetAdditionalInfoType", json!(1)),
        ("FELeaderboard.GetOptionsEnum", json!(0)),
        ("FELeaderboard.GetNumColumns", json!(headers.len())),
        ("FELeaderboard.GetRowHeader", json!(headers.map(|h| format!("#{h}")))),
        ("FELeaderboard.GetTimerText", json!("#")),
        ("FELeaderboard.GetMessage", json!("#")),
        ("FELeaderboard.GetBottomState", json!(1)),
        ("FELeaderboard.GetVoipStatus", json!(0)),
    ]
    .map(|(k, v)| (k.to_owned(), v))
    .into();
    for (row, &i) in order.iter().enumerate() {
        let (name, score, points, ..) = &best.rows[i];
        let score = if *score == 0 { "DNF".to_owned() } else { score.to_string() };
        out.push((format!("FELeaderboard.GetRowData({row})"), json!([(row + 1).to_string(), name, score, format!("+{points}")])));
        out.push((format!("FELeaderboard.GetPlayerIndex({row})"), json!(i)));
    }
    out
}

/// The overlay's HUDHooks text arguments in its own language, literal. centralhud comes from the online export, whose
/// table lacks the tutorial's ids.
fn localized(open: &BTreeMap<String, Screen>, args: Vec<Json>) -> Vec<Json> {
    let Some(o) = open.get(OVERLAY) else { return args };
    let language = &o.bindings.movie.text_assets;
    args.into_iter()
        .map(|a| match a {
            Json::String(s) => Json::String(literal(&language.localize(&s))),
            other => other,
        })
        .collect()
}

/// Opens movie `stem` as `name`. It is the tutorial export's movie, or else the Skate 1 online HUD's (centralhud,
/// timerhud, skatehud, skatetrick). It gets `answers` and is bound as skate1_online_hud binds its screens, plus the
/// tutorial's natives. A missing movie is a warning, never an error.
fn load(open: &mut BTreeMap<String, Screen>, root: &Path, name: &str, stem: &str, answers: &[(String, Json)]) -> Result<(), String> {
    let file = [TUTORIAL_MOVIES, online::MOVIES].iter().map(|dir| root.join(dir).join(format!("{stem}.json"))).find(|f| f.is_file());
    let Some(file) = file else {
        warn!("Skate 1 screens: no {stem}.json");
        return Ok(());
    };
    let pairs: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let mut screen = Screen::load(&file, &pairs)?;
    // The layer's root is challenges-skate1/screens; the movies' textures are in hud-skate1/{tutorial,online}.
    screen.own_assets(&file);
    online::bind(&mut screen)?;
    let vm = &mut screen.vm;
    for native in NATIVES {
        let o = vm.object(ObjectKind::Native(native.into()));
        vm.set(vm.global, native, Value::Object(o))?;
    }
    open.insert(name.to_owned(), screen);
    Ok(())
}

/// Closes the HUD's screens, not the FeScreen ones.
fn close_hud<T>(open: &mut BTreeMap<String, T>) {
    for name in HUD.into_iter().chain([FE_OUT]) {
        open.remove(name);
    }
}

/// FeScreen after apt_screen::render, with the call's audio (P5 owns the screen and its audio).
fn fe_system(world: &mut World) {
    if !world.contains_resource::<FeScreen>() || !world.contains_resource::<Skate1Screens>() {
        return;
    }
    let dt = world.get_resource::<Time<Real>>().map_or(0., |t| t.delta_secs());
    let Some(root) = world.get_resource::<Config>().map(|c| c.asset_root.clone()) else { return };
    world.resource_scope(|world, mut screens: Mut<Skate1Screens>| {
        world.resource_scope(|world, mut fe: Mut<FeScreen>| {
            let audio = match world.get_resource_mut::<Screens>() {
                Some(mut layers) => screens.fe_step(&mut layers.layer(1).open, &root, &mut fe, dt),
                None => None,
            };
            if let Some(pcm) = audio {
                let settings = crate::music::one_shot(world.get_resource::<crate::graphics_menu::Menu>(), crate::music::DIALOG);
                if let Some(mut assets) = world.get_resource_mut::<Assets<StreamPcm>>() {
                    let handle = assets.add(pcm);
                    screens.call_audio = Some(world.spawn((AudioPlayer(handle), settings)).id());
                }
            }
            if screens.call.is_none()
                && let Some(entity) = screens.call_audio.take()
                && let Ok(entity) = world.get_entity_mut(entity)
            {
                entity.despawn();
            }
        });
    });
}

pub(crate) struct Skate1ScreensPlugin;
impl Plugin for Skate1ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Skate1Screens>()
            .init_resource::<FeScreen>()
            .add_systems(Update, fe_system.after(crate::apt_screen::render))
            .add_systems(Update, face_arrows.after(crate::camera::present));
    }
}

/// An oi000 arrow: [`face_arrows`] turns, scales and fades it each frame.
#[derive(Component)]
pub(crate) struct Arrow;

/// One oi000 indicator at `at` (its locator's position); despawn it to remove. Drawn by oi000's own instance list
/// (0x826b7458 names it to manager+24, collection 0x2D1ADA86EF187C96), updated by 0x826b9780 (p5/arrows.md).
pub(crate) fn spawn_indicator(
    commands: &mut Commands,
    server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
    at: Transform,
) -> Entity {
    commands.spawn(indicator(server, materials, at)).id()
}

/// [`spawn_indicator`]'s entity, for a caller holding the World (skate1_online's VisualIndicators).
pub(crate) fn indicator(server: &AssetServer, materials: &mut Assets<StandardMaterial>, at: Transform) -> impl Bundle + use<> {
    let mesh = server.load(GltfAssetLabel::Primitive { mesh: 0, primitive: 0 }.from_asset(INDICATOR));
    let texture = server.load(GltfAssetLabel::Texture(0).from_asset(INDICATOR));
    (Arrow, Mesh3d(mesh), MeshMaterial3d(materials.add(indicator_material(texture))), at)
}

/// RETAIL 0x826b9780, each frame per arrow (p5/arrows.md):
/// - rotation (0x826b997c-0x826b9adc): local z = the camera's back flattened onto the ground and normalised, y up;
///   None when the camera looks straight down (retail has no guard: NaN; the caller keeps the last heading);
/// - INFERRED: camera row 2 is the view direction (mixmap/inputs.rs's convention), so -row 2 is Bevy's back();
/// - alpha x clamp((d - 6) / 14, 0, 1) (mTimer.y, 0x826b9b1c-0x826b9b88; near 6 / far 20 from collection 0x2D1ADA86);
/// - past 20 m the far variant scales by d / 20 (0x826b9900, 0x826b9c88; INFERRED: `onthetop` is ribbon_top);
/// - None past 500 m (0x826f6578). UNTRACED: what that radius is measured from; fallback: the camera.
/// ponytail: retail's cap of 16 arrows (0x826f6578) isn't kept; the most any in-window challenge places is 4.
fn arrow_pose(at: Vec3, camera: &Transform) -> Option<(Option<Quat>, f32, f32)> {
    let d = at.distance(camera.translation);
    if d > 500. {
        return None;
    }
    let back = camera.back();
    let rotation = Vec3::new(back.x, 0., back.z).try_normalize().map(|f| Quat::from_rotation_y(f.x.atan2(f.z)));
    let scale = if d > 20. { d / 20. } else { 1. };
    Some((rotation, scale, ARROW_ALPHA * ((d - 6.) / 14.).clamp(0., 1.)))
}

/// ribbon_alpha3's constant alpha (see [`indicator_material`]).
const ARROW_ALPHA: f32 = 78. / 255.;

fn face_arrows(
    cameras: Query<&Transform, (With<crate::camera::GameplayCamera>, Without<Arrow>)>,
    mut arrows: Query<(&mut Transform, &mut Visibility, &MeshMaterial3d<StandardMaterial>), With<Arrow>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(camera) = cameras.iter().next() else { return };
    for (mut transform, mut visibility, material) in &mut arrows {
        let Some((rotation, scale, alpha)) = arrow_pose(transform.translation, camera) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        visibility.set_if_neq(Visibility::Inherited);
        if let Some(rotation) = rotation {
            transform.rotation = rotation;
        }
        transform.scale = Vec3::splat(scale);
        if let Some(m) = materials.get_mut(&material.0) {
            m.base_color.set_alpha(alpha);
        }
    }
}

/// ribbon.psh: rgb = ambient_color + (color3d + lambert) x diffuse; alpha = transparent.a x mTimer.y.
/// - RETAIL: color3d comes from the obstacle's tVTShaderData, set up by 0x826bb180 via 0x826a5f50. The default
///   block Hash_51490AB3D6824B1E holds color3d = color2d = (1, 128/255, 0) orange and black lights A and B, so
///   ribbon.vsh's lambert is 0.
/// - INFERRED: the game multiplies in gamma space, so the colour goes in as sRGB.
/// - UNTRACED: ambient_color for this pass. Fallback: 0 (unlit).
/// - ribbon_alpha3 (the transparent map) decodes to a constant alpha 78 (P1 export): 78 / 255 times the distance
///   fade ([`arrow_pose`]).
/// - The alpha is baked into base_color, so the UV1 map isn't sampled.
/// - Spec C9: unlit.
fn indicator_material(texture: Handle<Image>) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgba(1., 128. / 255., 0., 78. / 255.),
        base_color_texture: Some(texture),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

/// The eChallengeTypes table at 0x830a2c28 (RETAIL, 32-byte rows): per type its game_icons icon for the paths none /
/// hero / antihero (+4 / +8 / +12; a null path entry falls back to none here) and its template title id (+16).
const TYPES: [(&str, Option<&str>, Option<&str>, &str); 31] = [
    ("nopath_tutorial", Some("regs_tutorial"), Some("slappys_tutorial"), "TUTORIAL"),
    ("unknown", None, None, "POINT_CONTEST"),
    ("chal_demo", None, None, "DEMO"),
    ("unknown", None, None, "RISK"),
    ("nopath_skate", Some("regs_skate"), Some("slappys_skate"), "SKATE"),
    ("chal_skate", Some("regs_skate"), Some("slappys_skate"), "SKATE"),
    ("nopath_photo", Some("regs_photo"), Some("slappys_photo"), "PHOTO"),
    ("regs_film", Some("regs_film"), Some("slappys_film"), "FILM"),
    ("nopath_besttrick", Some("regs_besttrick"), Some("slappys_besttrick"), "BEST_TRICK"),
    ("chal_spot", Some("chal_spot"), Some("chal_spot"), "OWN_THE_SPOT"),
    ("unknown", None, None, "SLALOM"),
    ("unknown", None, None, "SLIDE"),
    ("unknown", None, None, "LAP"),
    ("unknown", None, None, "GATE"),
    ("unknown", None, None, "FAVOUR"),
    ("nopath_jam", Some("regs_jam"), Some("slappys_jam"), "JAM"),
    ("unknown", None, None, "CRASH"),
    ("unknown", None, None, "SECURITY"),
    ("unknown", None, None, "POINT_2_POINT"),
    ("nopath_deathrace", Some("regs_deathrace"), Some("slappys_deathrace"), "DEATH_RACE"),
    ("nopath_spotrace", Some("regs_spotrace"), Some("slappys_spotrace"), "SPOT_RACE"),
    ("unknown", None, None, "LAST_MAN_STANDING"),
    ("unknown", None, None, "ONLINE_FREESKATE"),
    ("nopath_follow", Some("regs_follow"), Some("slappys_follow"), "FOLLOW"),
    ("train", None, None, "TRAIN_STATION"),
    ("shop_slappys", None, None, "SKATE_SHOP"),
    ("chal_xgames", None, None, "MEGARAMP"),
    ("chal_xgames", None, None, "STREETSKATING"),
    ("nopath_pro", Some("regs_pro"), Some("slappys_pro"), "PRO"),
    ("teleporter", None, None, "TELEPORTER"),
    ("goto", None, None, "GOTO"),
];

/// Mission.GetChallengeIconString(type, path) (RETAIL 0x8243bb68 over [`TYPES`]): path "hero" / "antihero" / else
/// none. The path is Mission.GetSignupMissionPath (0x8243aa78: 0x8233d648's path list, entry +960 0 hero, 1 antihero).
/// INFERRED: a key's path from its magazine prefix (he_ hero, ah_ antihero, else none), as [`challenge_icon`] reads it.
pub(crate) fn type_icon(challenge_type: i64, key: &str) -> String {
    let Some(&(none, hero, antihero, _)) = usize::try_from(challenge_type).ok().and_then(|t| TYPES.get(t)) else {
        return String::new();
    };
    let path = if key.starts_with("he_") { hero } else if key.starts_with("ah_") { antihero } else { None };
    path.unwrap_or(none).to_owned()
}

/// Mission.GetChallengeTypeName (RETAIL 0x82439e60): the active type's template title id (row +16); outside a
/// challenge, "#Not in a challenge!".
fn type_name(challenge_type: Option<i64>) -> String {
    match challenge_type.and_then(|t| TYPES.get(usize::try_from(t).ok()?)) {
        Some((.., name)) => format!("ID_MISSION_TEMPLATE_{name}_TITLE"),
        None => "#Not in a challenge!".into(),
    }
}

/// A career challenge key's minimap icon, named as old_hud1 names them (missions.big `<magazine>_<tier>_<kind>NN`):
/// - an own-the-spot is mini_icon_spot;
/// - anything else is mini_icon_chal_<kind>_<family>, and old_minimap falls back to the Thrasher art.
/// UNTRACED: the exe's kind -> icon switch. This is old_hud1's by-name rule; film missions have no icon.
pub(crate) fn challenge_icon(key: &str) -> Option<String> {
    const FAMILY: [(&str, &str); 3] = [("he", "smag"), ("ah", "thrash"), ("fp", "nopath")];
    const KINDS: [(&str, Option<&str>); 12] = [
        ("tutorial", Some("tutorial")), ("ownspot", None), ("spotrace", Some("spotrace")), ("deathrace", Some("deathrace")),
        ("demophoto", Some("demo")), ("photo", Some("photo")), ("prochallenge", Some("pro")), ("besttrick", Some("besttrick")),
        ("jam", Some("jam")), ("skate", Some("skate")), ("follow", Some("followme")), ("downhill", Some("race")),
    ];
    let mut parts = key.splitn(3, '_');
    let magazine = parts.next()?;
    let family = FAMILY.iter().find(|(m, _)| *m == magazine)?.1;
    let kind = parts.nth(1)?;
    let (_, icon) = KINDS.iter().find(|(k, _)| kind.starts_with(k))?;
    Some(icon.map_or_else(|| "mini_icon_spot".to_owned(), |k| format!("mini_icon_chal_{k}_{family}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The type table (0x830a2c28): icons by type and path, and the poster's header id.
    #[test]
    fn type_icons_and_names_follow_the_type_table() {
        assert_eq!(type_icon(0, "fp_u2_tutorial01"), "nopath_tutorial");
        assert_eq!(type_icon(4, "he_u1_skate01"), "regs_skate");
        assert_eq!(type_icon(23, "he_u2_follow01"), "regs_follow");
        assert_eq!(type_icon(6, "ah_x"), "slappys_photo");
        assert_eq!(type_icon(24, "he_x"), "train", "no hero entry: none's");
        assert_eq!(type_icon(32, "he_x"), "", "not a type");
        assert_eq!(type_name(Some(0)), "ID_MISSION_TEMPLATE_TUTORIAL_TITLE");
        assert_eq!(type_name(Some(19)), "ID_MISSION_TEMPLATE_DEATH_RACE_TITLE");
        assert_eq!(type_name(None), "#Not in a challenge!");
    }

    #[test]
    fn challenge_icons_follow_old_hud1() {
        assert_eq!(challenge_icon("he_u1_tutorial01").as_deref(), Some("mini_icon_chal_tutorial_smag"));
        assert_eq!(challenge_icon("fp_u3_tutorial03").as_deref(), Some("mini_icon_chal_tutorial_nopath"));
        assert_eq!(challenge_icon("he_u2_follow01").as_deref(), Some("mini_icon_chal_followme_smag"));
        assert_eq!(challenge_icon("ah_a1_ownspot02").as_deref(), Some("mini_icon_spot"));
        assert_eq!(challenge_icon("ah_a1_demophoto01").as_deref(), Some("mini_icon_chal_demo_thrash"), "demophoto before photo");
        assert_eq!(challenge_icon("he_u1_film01"), None, "film missions have no minimap icon");
        assert_eq!(challenge_icon("xx_u1_jam01"), None, "not a magazine");
    }

    #[test]
    fn the_tutorial_timer_reads_as_the_online_hud_did() {
        assert_eq!(online::timer_answer(None), json!(["#", -1]));
        assert_eq!(online::timer_answer(Some(8.2)), json!([":09", 9]));
        assert_eq!(online::timer_answer(Some(42.)), json!([":42", 42]));
        assert_eq!(online::timer_answer(Some(75.)), json!(["1:15", 75]));
    }

    /// A root with no movies: every load is a missing movie (a warning, never an error).
    pub(super) fn no_assets() -> PathBuf {
        std::env::temp_dir().join("skate1-screens-no-assets")
    }

    #[test]
    fn hudloaded_posts_the_frame_after_the_overlay_opens_even_without_its_movie() {
        let (root, facts) = (no_assets(), Facts::default());
        let (mut s, mut open) = (Skate1Screens::default(), BTreeMap::new());
        s.change_hud_state(TUTORIAL, 0);
        assert!(s.step(&mut open, &root, &facts, TICK).is_empty(), "the overlay opens this frame");
        assert_eq!(s.step(&mut open, &root, &facts, TICK), [Event::Loaded]);
        assert!(s.step(&mut open, &root, &facts, TICK).is_empty(), "once");
    }

    #[test]
    fn the_gate_starts_as_free_skates_and_follows_the_acts() {
        let root = no_assets();
        let mut facts = Facts { film01_done: true, ..Facts::default() };
        let mut s = Skate1Screens::default();
        assert_eq!(s.gate(&ctx(&facts)), (true, true));
        facts.film01_done = false;
        assert_eq!(s.gate(&ctx(&facts)), (false, false), "hidden until he_u1_film01");
        facts.film01_done = true;
        s.change_hud_state(TUTORIAL, 0); // Tutorial's Activate: Minimap off
        s.step(&mut BTreeMap::new(), &root, &facts, TICK);
        assert_eq!(s.gate(&ctx(&facts)), (true, false));
        s.change_hud_state(TUTORIAL, 16); // Enter 16: Trick off
        s.step(&mut BTreeMap::new(), &root, &facts, TICK);
        assert_eq!(s.gate(&ctx(&facts)), (false, false));
    }

    /// Review Focus 5: the HUD's screens close, the gate lifts, no stale hudloaded / objective mark fires; the FeScreen
    /// screens are left to `want`.
    #[test]
    fn clear_closes_the_hud_and_leaves_the_fe_screens_to_want() {
        let (root, facts) = (no_assets(), Facts::default());
        let mut s = Skate1Screens::default();
        s.change_hud_state(TUTORIAL, 0);
        s.step(&mut BTreeMap::new(), &root, &facts, TICK);
        s.mark_objective_complete(1);
        s.reset();
        assert!(s.step(&mut BTreeMap::new(), &root, &facts, TICK).is_empty(), "no stale hudloaded or objective mark");
        assert_eq!(s.gate, None, "the gate lifts");
        let mut names: BTreeMap<String, ()> =
            HUD.iter().chain(&[FE_OUT, SKATE101, WELCOME]).map(|n| (n.to_string(), ())).collect();
        close_hud(&mut names);
        assert_eq!(names.into_keys().collect::<Vec<_>>(), [SKATE101, WELCOME]);
    }

    #[test]
    fn win_state_and_options_feed_the_end_screens() {
        let mut s = Skate1Screens::default();
        s.set_win_state(true);
        let answers = s.answers(&BTreeMap::new(), &Facts::default());
        let get = |k: &str| answers.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("Mission.GetWinState"), Some(json!(true)));
        assert_eq!(get("Progression.IsActiveChallengeCompleted"), Some(json!(true)));
        assert_eq!(get("Mission.GetSignUpOption(1)"), Some(json!("ID_COMMON_DECLINE")));
        assert_eq!(get("Mission.GetEndMissionOptionText(1)"), Some(json!("ID_COMMON_RETRY_CAPS")));
        assert_eq!(get("Progression.GetActiveMissionCash"), Some(json!(0)));
    }

    /// Preview 4 (10-15): the sign-up prompt (action overlay state 14) is set on a change only.
    #[test]
    fn the_sign_up_prompt_sets_the_action_overlay_on_change() {
        let mut s = Skate1Screens::default();
        s.sign_up_prompt(false);
        assert!(s.pending.is_empty());
        s.sign_up_prompt(true);
        s.sign_up_prompt(true);
        assert_eq!(s.pending, [Act::Set(Movie::Action, 14)]);
        s.sign_up_prompt(false);
        assert_eq!(s.pending, [Act::Set(Movie::Action, 14), Act::Set(Movie::Action, 0)]);
    }

    /// Review 10-15 minor 8: leaving the map forgets the scene's NIS screen; the next scene opens it afresh.
    #[test]
    fn unload_forgets_the_nis_screen() {
        let mut s = Skate1Screens::default();
        assert!(matches!(s.nis.step(Some(55)), Some(NisAct::Open(_))));
        s.unload(&mut World::new());
        assert!(matches!(s.nis.step(Some(55)), Some(NisAct::Open(_))));
    }

    /// Review 10-15 minor 6: another state set on the action overlay takes the prompt down, so it is set again.
    #[test]
    fn the_sign_up_prompt_comes_back_after_another_action_state() {
        let mut s = Skate1Screens::default();
        s.sign_up_prompt(true);
        s.pending.clear();
        let _ = s.apply(&mut BTreeMap::new(), &no_assets(), &Facts::default(), &[], Act::Set(Movie::Action, 0), &mut vec![]);
        s.sign_up_prompt(true);
        assert_eq!(s.pending, [Act::Set(Movie::Action, 14)]);
    }

    /// Preview 4 (10-15): a lesson's poster offers ACCEPT / SKIP LESSON until Filming 101 is won (RETAIL 0x823462f8),
    /// and its end options are RETRY / CONTINUE.
    #[test]
    fn a_lessons_poster_offers_skip_lesson_until_filming_101() {
        let s = Skate1Screens::default();
        let mut facts = Facts { card: Card { challenge_type: Some(0), ..Card::default() }, ..Facts::default() };
        let get = |facts: &Facts, k: &str| s.answers(&BTreeMap::new(), facts).into_iter().find(|(key, _)| key == k).map(|(_, v)| v);
        assert_eq!(get(&facts, "Mission.GetNumSignUpOptions"), Some(json!(2)));
        assert_eq!(get(&facts, "Mission.GetSignUpOption(1)"), Some(json!("ID_COMMON_SKIP_LESSON")));
        assert_eq!(get(&facts, "Mission.GetEndMissionOptionText(0)"), Some(json!("ID_COMMON_RETRY_CAPS")));
        facts.film01_done = true;
        assert_eq!(get(&facts, "Mission.GetSignUpOption(1)"), Some(json!("ID_COMMON_DECLINE")));
        facts.card.challenge_type = Some(23);
        facts.film01_done = false;
        assert_eq!(get(&facts, "Mission.GetSignUpOption(1)"), Some(json!("ID_COMMON_DECLINE")), "not a lesson");
    }

    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn the_overlays_load_with_their_set_state() {
        let root = parts::tests::asset_root();
        for movie in [Movie::Tutorial, Movie::Film, Movie::Photo, Movie::Follow, Movie::Skate, Movie::BestTrick] {
            let (mut s, mut open) = (Skate1Screens::default(), BTreeMap::new());
            s.open_overlay(&mut open, &root, &[], Some(movie)).unwrap();
            assert!(open[OVERLAY].has("SetState"), "{movie:?}");
            assert!(s.loaded && s.overlay == Some(movie));
        }
    }

    /// B2 (Pushing & Timing showed no controller): lesson 1's HUD calls to its end, then lesson 2's from Load to its
    /// first ShowController (fp_u2_tutorial02 [challenge]: Active, UpdateObjectives, ShowController(LeftAndBottomButton)
    /// in one Lua pass). The controller must show its sequence.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn the_second_lesson_shows_its_controller() {
        let root = parts::tests::asset_root();
        let (mut s, mut open) = (Skate1Screens::default(), BTreeMap::new());
        let mut facts = Facts { film01_done: true, spawn_rows: true, ..Facts::default() };
        let mut run = |s: &mut Skate1Screens, open: &mut BTreeMap<String, Screen>, facts: &Facts, subs: &[i64], frames: usize| {
            for &sub in subs {
                s.change_hud_state(TUTORIAL, sub);
            }
            for _ in 0..frames {
                s.step(open, &root, facts, TICK);
                for screen in open.values_mut() {
                    screen.advance().unwrap();
                }
            }
        };
        // Lesson 1: Load, Active, its gesture, then None, then final's HideController / HideObjectives / Congratulations.
        facts.card.key = "fp_u2_tutorial01".into();
        run(&mut s, &mut open, &facts, &[0], 3);
        run(&mut s, &mut open, &facts, &[2, 10], 3);
        (facts.controller, facts.gesture) = (8, 18);
        run(&mut s, &mut open, &facts, &[5, 7], 60);
        (facts.controller, facts.gesture) = (0, 0);
        run(&mut s, &mut open, &facts, &[5, 7], 30);
        run(&mut s, &mut open, &facts, &[6, 9, 11], 200);
        // Lesson 2.
        facts.card.key = "fp_u2_tutorial02".into();
        run(&mut s, &mut open, &facts, &[0], 3);
        (facts.controller, facts.gesture) = (3, 0);
        run(&mut s, &mut open, &facts, &[2, 10, 5, 7], 40);
        let c = &open[CONTROLLER];
        assert!(matches!(parts::tests::get(c, "", "_visible"), Value::Bool(true)), "the controller shows");
        let frame = parts::tests::get(c, "mcControllers.mc360Animations", "_currentframe");
        assert!(matches!(frame, Value::Number(f) if (121. ..=151.).contains(&f)), "axintro: {frame:?}");
    }

    fn want(fe: Fe) -> FeScreen {
        FeScreen { want: Some(fe), ..FeScreen::default() }
    }

    /// Review Focus 2: a want whose movie is missing sets its flags at once. The call without a wav holds 3 s.
    #[test]
    fn a_shown_screen_that_stopped_lets_the_frontend_go_on() {
        // overnight-1015 review I2: a loader that opened and then stopped (render removed it from `open`) must not
        // hold the frontend, and its time pause, forever.
        let (root, mut open) = (no_assets(), BTreeMap::new());
        let (mut s, mut f) = (Skate1Screens::default(), want(Fe::LoadingQuick));
        s.opened = f.want.clone();
        s.shown = Some(QUICK);
        s.fe_step(&mut open, &root, &mut f, TICK);
        assert!(f.intro_done);
    }

    #[test]
    fn missing_movies_set_the_flags_at_once() {
        let (root, mut open) = (no_assets(), BTreeMap::new());
        for (fe, flags) in [
            (Fe::LoadingWelcome, (true, false, false)),
            (Fe::CaCDetails, (true, true, false)),
            (Fe::Skate101("FilmMenu".into()), (true, false, true)),
            (Fe::Skate101("15".into()), (true, false, true)),
            (Fe::Skate101("NoSuch101".into()), (true, false, true)),
        ] {
            let (mut s, mut f) = (Skate1Screens::default(), want(fe.clone()));
            assert!(s.fe_step(&mut open, &root, &mut f, TICK).is_none());
            assert_eq!((f.intro_done, f.confirmed, f.closed), flags, "{fe:?}");
        }
        let (mut s, mut f) = (Skate1Screens::default(), want(Fe::Call("Shingo01".into())));
        assert!(s.fe_step(&mut open, &root, &mut f, 0.5).is_none(), "no wav");
        assert!(f.intro_done && !f.closed);
        assert_eq!(s.pending, [Act::Set(Movie::Action, 13)]);
        for _ in 0..5 {
            s.fe_step(&mut open, &root, &mut f, 0.5);
        }
        assert!(!f.closed, "3.0 s: the hold ends at 0.5 + 3");
        s.fe_step(&mut open, &root, &mut f, 0.5);
        assert!(f.closed);
        assert_eq!(s.pending.last(), Some(&Act::Set(Movie::Action, 0)));
    }

    #[test]
    fn the_distance_hud_text_follows_the_units_setting() {
        let text = |label: &str| match label {
            "413c56da" => "{0:d} ft".to_owned(),
            "92099686" => "{0:d} m".to_owned(),
            other => other.to_owned(),
        };
        assert_eq!(distance_answer(&text, true, 12.5, 20), json!([literal("37 ft"), 12, 0.625]));
        assert_eq!(distance_answer(&text, true, 0.2, 10), json!([literal("0 ft"), 0, 0.2f32 / 10.]));
        assert_eq!(distance_answer(&text, false, 12.5, 20), json!([literal("12 m"), 12, 0.625]));
        // RETAIL 0x8243b84c: a negative max (no SetWarningInfo, the nollie lesson) shows ratio 0.
        assert_eq!(distance_answer(&text, false, 12.5, -1), json!([literal("12 m"), 12, 0.]));
    }

    #[test]
    fn skate101_names_and_the_repeated_trick_decimal_resolve() {
        assert_eq!(skate101("FilmMenu"), Some((5, "FILM_MENU")));
        assert_eq!(skate101("15"), Some((15, "REPEATED_TRICK")));
        assert_eq!(skate101("NoSuch101"), None);
    }

    /// Skate 101: intro_done on its IntroComplete frame, A plays the outro, closed when it's gone (research9 2).
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn skate101_reports_its_intro_and_closes_after_its_outro() {
        let root = parts::tests::asset_root();
        let (mut s, mut open, mut f) = (Skate1Screens::default(), BTreeMap::new(), want(Fe::Skate101("FilmMenu".into())));
        s.fe_step(&mut open, &root, &mut f, TICK);
        assert!(open.contains_key(SKATE101), "{:?}", {
            let mut again = want(Fe::Skate101("FilmMenu".into()));
            Skate1Screens::default().open_fe_screen(&mut BTreeMap::new(), &root, &mut again).err()
        });
        let header = parts::tests::text_of(&open[SKATE101], "mcTitle.mcHeader.labelText");
        assert_eq!(header, "ID_SKATE_101_FILM_MENU_TITLE");
        let frames = (1..120).find(|_| {
            open.values_mut().for_each(|s| s.advance().unwrap());
            s.fe_step(&mut open, &root, &mut f, TICK);
            f.intro_done
        });
        assert!(frames.is_some_and(|n| n >= 40), "{frames:?}");
        assert!(s.press_in(&mut open, &mut f, Pad::A));
        assert!(open.contains_key("8-skate101-out") && !f.closed);
        let frames = (1..120).find(|_| {
            open.values_mut().for_each(|s| s.advance().unwrap());
            s.fe_step(&mut open, &root, &mut f, TICK);
            f.closed
        });
        assert!(frames.is_some_and(|n| n < 100), "{frames:?}");
        assert!(open.is_empty());
    }

    /// Review Focus 3: a loader closed during its intro plays its outro, and the next want opens the same frame.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn a_close_during_the_intro_goes_to_the_outro() {
        let root = parts::tests::asset_root();
        let (mut s, mut open, mut f) = (Skate1Screens::default(), BTreeMap::new(), want(Fe::LoadingQuick));
        s.fe_step(&mut open, &root, &mut f, TICK);
        for _ in 0..10 {
            open.values_mut().for_each(|s| s.advance().unwrap());
            s.fe_step(&mut open, &root, &mut f, TICK);
        }
        assert!(!f.intro_done, "still in its intro");
        f = want(Fe::LoadingWelcome);
        s.fe_step(&mut open, &root, &mut f, TICK);
        assert_eq!(open.keys().collect::<Vec<_>>(), ["9-quick-out", WELCOME]);
        assert!(!f.intro_done);
        for _ in 0..(OUTRO_TIMEOUT / TICK) as usize + 1 {
            open.values_mut().for_each(|s| s.advance().unwrap());
            s.fe_step(&mut open, &root, &mut f, TICK);
        }
        assert!(!open.contains_key("9-quick-out"));
    }

    /// Preview 4 (10-15): every FE menu stopped at its first draw ("Nonfinite HUD transform"). Each push opens, plays
    /// its intro and draws for 2 s.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn every_fe_menu_opens_and_draws() {
        let root = parts::tests::asset_root();
        let facts = Facts { card: Card { key: "fp_u2_tutorial01".into(), title: "T".into(), ..Card::default() }, ..Facts::default() };
        for id in [65, 73, 23] {
            let (mut s, mut open) = (Skate1Screens::default(), BTreeMap::new());
            let answers = s.answers(&open, &facts);
            s.open_fe(&mut open, &root, &facts, &answers, id).unwrap();
            let fe = open.get_mut(FE).unwrap_or_else(|| panic!("FE {id} opened"));
            for frame in 0..120 {
                fe.advance().unwrap();
                fe.draws().unwrap_or_else(|e| panic!("FE {id} frame {frame}: {e}"));
            }
        }
        let (mut s, mut open, mut f) = (Skate1Screens::default(), BTreeMap::new(), want(Fe::Skate101("FilmMenu".into())));
        s.fe_step(&mut open, &root, &mut f, TICK);
        for frame in 0..120 {
            let s101 = open.get_mut(SKATE101).unwrap();
            s101.advance().unwrap();
            s101.draws().unwrap_or_else(|e| panic!("skate101 frame {frame}: {e}"));
        }
    }

    #[test]
    fn fe_picks_follow_the_mission_handlers() {
        use Event::*;
        let (s, e, two) = (&SIGN_UP[..], &END[..], &["", ""][..]);
        assert_eq!((fe_pick(65, false, Some(0), s), fe_pick(65, false, Some(1), s), fe_pick(65, false, None, s)), (Some(Yes), Some(No), Some(No)));
        assert_eq!(fe_pick(65, false, Some(1), &SIGN_UP_LESSON), Some(No), "SKIP LESSON declines");
        assert_eq!((fe_pick(73, true, Some(0), two), fe_pick(73, true, Some(1), two)), (Some(No), Some(Yes)), "won: CONTINUE, RETRY");
        assert_eq!((fe_pick(73, false, Some(0), two), fe_pick(73, false, Some(1), two)), (Some(Yes), Some(No)), "lost: RETRY, QUIT");
        assert_eq!((fe_pick(23, true, Some(0), e), fe_pick(23, true, Some(1), e)), (Some(Continue), Some(Retry)));
        assert_eq!((fe_pick(23, false, Some(0), &END_LESSON), fe_pick(23, false, Some(1), &END_LESSON)), (Some(Retry), Some(Continue)));
        assert_eq!(fe_pick(74, true, Some(0), &[""]), Some(Continue));
        assert_eq!((fe_pick(73, true, None, two), fe_pick(23, true, None, e)), (None, None), "B: nothing");
    }

    /// Review Focus 2: a sign-up poster without its movie still answers; Down stops at the last row.
    #[test]
    fn a_missing_sign_up_poster_still_answers() {
        let (root, facts) = (no_assets(), Facts::default());
        let (mut s, mut open, mut f) = (Skate1Screens::default(), BTreeMap::new(), FeScreen::default());
        s.change_hud_state(TUTORIAL, 4);
        s.step(&mut open, &root, &facts, TICK);
        s.change_hud_state(TUTORIAL, 1); // Enter 1: the poster (FE 65)
        s.step(&mut open, &root, &facts, TICK);
        assert_eq!(s.menu.map(|m| m.id), Some(65));
        assert!(s.press_in(&mut open, &mut f, Pad::Down));
        assert!(s.press_in(&mut open, &mut f, Pad::Down), "clamped at the last row");
        assert!(s.press_in(&mut open, &mut f, Pad::A));
        assert_eq!(s.step(&mut open, &root, &facts, TICK), [Event::No]);
        assert_eq!(s.menu, None);
    }

    /// Review Focus 1: the A that dismisses a Skate 101 doesn't answer the lesson end queued behind it. That menu
    /// opens after the 101's outro, and its own A answers it once.
    #[test]
    fn one_press_answers_one_screen() {
        let (root, facts) = (no_assets(), Facts::default());
        let (mut s, mut open) = (Skate1Screens::default(), BTreeMap::new());
        let mut f = want(Fe::Skate101("FilmMenu".into()));
        (s.opened, s.shown) = (f.want.clone(), Some(SKATE101));
        s.fe_queue.push_back(73);
        s.step(&mut open, &root, &facts, TICK);
        assert_eq!(s.menu, None, "FE pushes wait behind the 101");
        assert!(s.press_in(&mut open, &mut f, Pad::A), "A dismisses the 101");
        assert!(s.pending.is_empty(), "and answers nothing else");
        s.step(&mut open, &root, &facts, TICK);
        assert_eq!(s.menu, None, "the 101's outro still holds it");
        s.fe_step(&mut open, &root, &mut f, TICK);
        assert!(f.closed);
        s.step(&mut open, &root, &facts, TICK);
        assert_eq!(s.menu.map(|m| m.id), Some(73));
        s.set_win_state(true);
        assert!(s.press_in(&mut open, &mut f, Pad::A));
        assert_eq!(s.step(&mut open, &root, &facts, TICK), [Event::No], "won row 0: CONTINUE");
        assert!(!s.press_in(&mut open, &mut f, Pad::A), "nothing left to answer");
    }

    #[test]
    fn the_indicator_draws_as_authored_unlit_from_both_sides() {
        let m = indicator_material(Handle::default());
        assert_eq!(m.base_color, Color::srgba(1., 128. / 255., 0., 78. / 255.), "Hash_51490AB3D6824B1E's orange");
        assert!(matches!(m.alpha_mode, AlphaMode::Blend));
        assert!(m.unlit && m.double_sided && m.cull_mode.is_none() && m.base_color_texture.is_some());
    }

    /// 10-15 arrows (RETAIL 0x826b9780, p5/arrows.md): turned to the camera's heading about world up, faded in over
    /// 6..20 m, scaled by d / 20 past 20 m, gone past 500 m.
    #[test]
    fn an_arrow_turns_to_the_cameras_heading_and_fades_with_distance() {
        let at = Vec3::new(0., 0., 0.);
        // A camera 10 m back along +x, looking at the arrow (its back is +x).
        let camera = Transform::from_xyz(10., 3., 0.).looking_at(at, Vec3::Y);
        let (rotation, scale, alpha) = arrow_pose(at, &camera).unwrap();
        let rotation = rotation.unwrap();
        assert!((rotation * Vec3::Z).abs_diff_eq(Vec3::X, 1e-4), "local z along the camera's flat back");
        assert!((rotation * Vec3::Y).abs_diff_eq(Vec3::Y, 1e-4), "upright");
        assert_eq!(scale, 1.);
        let d = Vec3::new(10., 3., 0.).length();
        assert!((alpha - 78. / 255. * (d - 6.) / 14.).abs() < 1e-5);
        assert_eq!(arrow_pose(at, &Transform::from_xyz(0., 0., 5.).looking_at(at, Vec3::Y)).unwrap().2, 0., "within 6 m");
        let far = arrow_pose(at, &Transform::from_xyz(0., 0., 60.).looking_at(at, Vec3::Y)).unwrap();
        assert!((far.1 - 3.).abs() < 1e-5 && (far.2 - 78. / 255.).abs() < 1e-6, "d / 20 past 20 m, full alpha");
        assert!(arrow_pose(at, &Transform::from_xyz(0., 0., 501.)).is_none(), "past 500 m");
        let down = arrow_pose(at, &Transform::from_xyz(0., 30., 0.).looking_to(Vec3::NEG_Y, Vec3::Z)).unwrap();
        assert!(down.0.is_none(), "straight down has no heading: the caller keeps the last");
    }

    /// Texture 0 is the diffuse; the alpha shape sits in the material's extras (P1's oi000.glb).
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/challenges-skate1/indicator"]
    fn the_indicator_glb_is_the_ribbon_material() {
        let glb = std::fs::read(parts::tests::asset_root().join(INDICATOR)).unwrap();
        let length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json: Json = serde_json::from_slice(&glb[20..20 + length]).unwrap();
        let source = json["textures"][0]["source"].as_u64().unwrap() as usize;
        assert_eq!(json["images"][source]["name"], "ribbonbrushstoke_16x16_d");
        assert_eq!(json["materials"][0]["extras"]["transparent"]["index"], 1);
        assert_eq!(json["meshes"].as_array().map(Vec::len), Some(1));
    }

    /// One lesson end to end, without assets:
    /// - the overlay and hudloaded;
    /// - the poster and its A;
    /// - an objective mark;
    /// - the win, and 2.7 s later the lesson end;
    /// - its A.
    #[test]
    fn a_lesson_walks_through_its_screens() {
        let root = no_assets();
        let facts = Facts {
            film01_done: true,
            spawn_rows: true,
            card: Card { key: "he_u1_tutorial01".into(), ..Card::default() },
            ..Facts::default()
        };
        let step = |s: &mut Skate1Screens, open: &mut BTreeMap<String, Screen>| s.step(open, &root, &facts, TICK);
        let (mut s, mut open, mut f) = (Skate1Screens::default(), BTreeMap::new(), FeScreen::default());
        s.change_hud_state(TUTORIAL, 0);
        step(&mut s, &mut open);
        assert_eq!(step(&mut s, &mut open), [Event::Loaded]);
        s.change_hud_state(TUTORIAL, 1);
        step(&mut s, &mut open);
        assert!(s.press_in(&mut open, &mut f, Pad::A));
        assert_eq!(step(&mut s, &mut open), [Event::Yes]);
        s.change_hud_state(TUTORIAL, 2);
        step(&mut s, &mut open);
        s.mark_objective_complete(0);
        assert_eq!(step(&mut s, &mut open), [Event::ObjectiveComplete(0)]);
        s.change_hud_state(TUTORIAL, 11);
        step(&mut s, &mut open);
        let frames = (1..=200).find(|_| {
            step(&mut s, &mut open);
            s.menu.is_some()
        });
        assert_eq!(frames, Some(169), "2704 ms: the first 16 ms tick past 2.7 s");
        assert!(s.press_in(&mut open, &mut f, Pad::A));
        assert_eq!(step(&mut s, &mut open), [Event::No], "won: CONTINUE");
    }
}
