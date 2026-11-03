//! Skate 2's S.K.A.T.E. (the `skate` challenge template) on New_San_Vanelona, on the match core (match_core.rs).
//! Data: skate2port/old_skate_game.py, installed at `private/skate2-online/skate/`: rules.json (per row: spawn, wait
//! locator, start and spot volumes, time limit, failure line, intro; the template's messages and timings),
//! gestures.json (each trick's label, stick gesture and TrickType), strings.json, the intro cameras (tutorial.json +
//! scene/camera, played by challenge::cutscene) and the HUD movies (screens/runtime, opened on the Skate 2 tutorial
//! layer; their sounds play from screens/sfx, apt_screen::sfx_dir).
//!
//! The rules are the disc's Lua (scripts/skate.lua), as [`Turn`]: one plain serializable struct and pure functions.
//! - round_setup: the leader (the first drawn at random, Skate 3's rule) goes to the spawn; the followers are the
//!   players after it in slot order with letters to spare. wait_for_leader: "Set the trick"; the attempt starts after
//!   5 s or on leaving the start volume, and has the row's TimeLimit (TimerHUD).
//! - An air counts once a spot volume was entered. The leader's sets the trick when it is a Flip or Air trick, every
//!   spot was entered and it (trick and spin) is not on the already-been-done list; else Invalid / LeftSpot /
//!   Repeated. Leaving a spot volume on the ground, a bail (at its start) or the timer fail the attempt, and so does
//!   stepping off the board once a spot was entered (Skate 3's rules). A failed set passes the lead to the first
//!   follower.
//! - Each follower copies (same trick, same spin) or takes a letter (hudthrowmes' S-K-A-T-E stinger, then the
//!   leaderboard). Five letters is out; the leader wins when no one else is left. Online, a set everyone copied
//!   passes the lead on ("Everyone copied! New Leader!!").
//!
//! The intro is the row's NIS flythrough (IntroPresentationEvents type 5: a 5 s camera, no actors, no audio), held
//! with `Match::intro`. Retail S.K.A.T.E. has no countdown; the core's 3 s one shows hudcountdown.
//!
//! HUD: hudabd (the set trick and its stick gesture, the copy attempt, the done list), leaderboardhud's throwdown
//! layout (names, S-K-A-T-E letters, the turn arrow), hudthrowmes, and hudcentral / hudtimer / hudcountdown through
//! [`TutorialScreens`]. The stick gesture follows the local player's natural stance: gesture_item mirrors its art
//! (drawn for goofy) when HUDComponents.PlayerStance_IsRegular, which Skate 2 answers from the profile's stance.
//!
//! Offline (Party Play, or `SKATE_MODE=skate:<row>` with two players) is Skate 3's PartyPlay path on this one skater,
//! as skate3_skate_game's: the pad goes to each attempter at its fade's cut, every attempt waits on its Press-A card,
//! the waiting players stand as party_play's puppets, and the results dialog (Rematch / Exit Party Play / Change
//! settings) follows "Challenge Complete!". Online every game runs the same [`Turn`]: the attempter's game judges its
//! own attempts and queues them as [`AttemptResult`]s in `SkateGame::outbox`; the others' arrive in `SkateGame::inbox`.
use crate::{
    apt_screen::{Layer, Screen, Screens},
    challenge::cutscene::Cutscene,
    config::Config,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Phase, Playable},
    physics::SkaterRuntime,
    skate2_arrows::Arrows,
    skate2_screens::{Central, TutorialScreens, View},
    skate3_modes::ChallengeFade,
    skate3_skate_game::FADE_HOLD,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

const DIR: &str = "private/skate2-online/skate";
/// The rows' indicator looks (each VisualIndicators locator's type outline and style) for skate2_arrows.
const INDICATORS: &str = "private/skate2-online/skate/indicator/indicators.json";
const MAP: &str = "New_San_Vanelona";
/// The Skate 2 tutorial's screen layer: the movies' payload paths are relative to its root, and it plays their sfx.
const GAME: u8 = 2;
// Draw order is name order, among skate2_screens' names.
const ABD: &str = "3-abd";
const BOARD: &str = "3-leaderboard";
const LETTERS: &str = "6-throwmes";
/// leaderboardhud's eLeaderBoardHUD_Skate: the throwdown layout.
const LAYOUT_SKATE: i64 = 7;
/// endchallenge_online's eThrowDownType Skate: Party Play's results dialog.
const THROWDOWN_SKATE: i64 = 2;
/// Trick.eTrickType.
const AIR: u32 = 1;
const FLIP: u32 = 2;
/// ponytail: aircompleted comes as the landing settles; here, still riding this long after touchdown.
const LANDING_GRACE: f32 = 0.3;
/// show_results holds "Challenge Complete!" this long.
const RESULTS_HOLD: f32 = 3.;
/// AI.RepositionAI's fade argument (S2 0x826dc118 -> 0x82b79d18, the native S3 ports as skate3_skate_game's): a request
/// while a fade runs joins its batch; it fades out over half, moves the batch at black (switching the camera to the
/// player when the 6th argument is true), holds [`FADE_HOLD`], fades in over the other half and posts
/// teleporttransitionfinished. round_setup's leader (skate.lua 301-307).
const LEAD_FADE: f32 = 1.;
/// prepare_follower's last attempter and follower (749-772).
const FOLLOW_FADE: f32 = 1.5;
/// The default fade (1.0): check_players' waiting spots (199-201), leader_failure's send-back (709-716).
const DEFAULT_FADE: f32 = 1.;
/// A disconnected attempter's attempt timer (wait_for_scoring 426, wait_for_copy 1041; reset to it, 616, 1135).
const GONE_LIMIT: f32 = 1.;

// ---------------------------------------------------------------- data
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Timings {
    /// wait_for_leader / wait_for_follower's PreAttemptTimer.
    pub pre_attempt: f32,
    /// wait_for_follower's settle before "Copy the trick".
    pub follower_settle: f32,
    /// leader_success / leader_failure / follower_copy.
    pub after_result: f32,
    /// follower_not_copy: to the letter stinger, to the leaderboard's letter, to the next follower.
    pub letter_wait: f32,
    pub letter_show: f32,
    pub letter_after: f32,
    /// process_round's winner to show_results.
    pub winner: f32,
}

#[derive(Deserialize)]
struct Rules {
    /// The template's message fields -> string ids.
    template: HashMap<String, String>,
    max_letters: u8,
    timings: Timings,
    valid_trick_types: Vec<u32>,
    /// The rows that parse (M2: one malformed row is left out, not every row).
    #[serde(deserialize_with = "crate::skate3_skate_game::placed_rows")]
    rows: BTreeMap<String, Row>,
}

type Matrix = [[f32; 4]; 4];

#[derive(Deserialize)]
struct Row {
    title: String,
    online: bool,
    time_limit: f32,
    /// ModifiedCentralMessageHALIDSkateFailureExplanation ("Did not Trick over Stairs").
    failure: Option<String>,
    intro: Option<String>,
    /// TurnBasedAttemptSpawnPoint.
    spawn: Matrix,
    /// TurnBasedWaitingLocation.
    wait: Matrix,
    start_volume: Volume,
    spot_volumes: Vec<Volume>,
    indicators: Vec<Matrix>,
    /// Each VisualIndicators entry's locator, which names its look in [`INDICATORS`]; none in an older export.
    #[serde(default)]
    indicator_locators: Vec<String>,
    /// The two locators' children (the online rows' `skate2-online/rows.json`).
    #[serde(skip)]
    spawn_slots: Vec<Matrix>,
    #[serde(skip)]
    wait_slots: Vec<Matrix>,
}
impl Row {
    /// check_players' AddIndicator for each VisualIndicators entry (skate.lua 231-234): its locator, drawn in its own
    /// look (user 11-01: the orange arrow, not the guide chevron). An older export's draw skate2_arrows' fallback.
    fn arrows(&self) -> Vec<(String, Matrix)> {
        let name = |i: usize| self.indicator_locators.get(i).cloned().unwrap_or_else(|| format!("skate_spot_{i}"));
        self.indicators.iter().enumerate().map(|(i, m)| (name(i), *m)).collect()
    }
    /// RepositionAI(attempter, TurnBasedAttemptSpawnPoint, 0, ...) (skate.lua 307, 772).
    fn spawn_at(&self) -> Matrix {
        crate::online_map::locator_slot(&self.spawn, &self.spawn_slots, 0)
    }
    /// RepositionAI(id, TurnBasedWaitingLocation, id) (199-201, 709-716, 749-755).
    fn wait_at(&self, slot: usize) -> Matrix {
        crate::online_map::locator_slot(&self.wait, &self.wait_slots, slot)
    }
}

#[derive(Deserialize, Clone)]
struct Volume {
    min: [f32; 3],
    max: [f32; 3],
}
impl Volume {
    fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|k| self.min[k] <= p[k] && p[k] <= self.max[k])
    }
}

struct Data {
    root: PathBuf,
    rules: Rules,
    /// identifier -> (label id, stick gesture or "none", TrickType)
    tricks: HashMap<String, (String, String, Option<u32>)>,
    strings: HashMap<String, String>,
    /// intro scene -> seconds
    intros: HashMap<String, f32>,
    /// The engine's own tricks have joined `tricks`.
    filled: bool,
}

fn read<T: DeserializeOwned>(root: &Path, name: &str) -> Result<T, String> {
    let text = std::fs::read_to_string(root.join(name)).map_err(|e| format!("{name}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
}

fn load(root: &Path) -> Result<Data, String> {
    #[derive(Deserialize)]
    struct Scenes {
        scenes: HashMap<String, Scene>,
    }
    #[derive(Deserialize)]
    struct Scene {
        duration: f32,
    }
    let raw: HashMap<String, Option<(String, String, Json)>> = read(root, "gestures.json")?;
    let tricks = (raw.into_iter())
        .filter_map(|(id, t)| t.map(|(label, gesture, kind)| (id, (label, gesture, kind.as_u64().map(|k| k as u32)))))
        .collect();
    let scenes: Scenes = read(root, "tutorial.json")?;
    // The online rows' locator children (skate2port/old_online.py).
    #[derive(Deserialize)]
    struct Online {
        modes: Modes,
    }
    #[derive(Deserialize)]
    struct Modes {
        skate: Vec<Json>,
    }
    #[derive(Deserialize)]
    struct OnlineRow {
        id: String,
        #[serde(rename = "TurnBasedAttemptSpawnPoint")]
        spawn: Slots,
        #[serde(rename = "TurnBasedWaitingLocation")]
        wait: Slots,
    }
    #[derive(Deserialize)]
    struct Slots {
        slots: Vec<Matrix>,
    }
    let mut rules: Rules = read(root, "rules.json")?;
    let online: Online = read(root.parent().unwrap_or(root), "rows.json")?;
    for v in online.modes.skate {
        let id = v.get("id").and_then(Json::as_str).unwrap_or_default().to_owned();
        match serde_json::from_value::<OnlineRow>(v) {
            Ok(r) => {
                if let Some(row) = rules.rows.get_mut(&r.id) {
                    (row.spawn_slots, row.wait_slots) = (r.spawn.slots, r.wait.slots);
                }
            }
            // M2 (online audit 10-06): its slots unknown, the row is left out, not the whole mode.
            Err(e) => {
                warn!("Skate 2 S.K.A.T.E.: rows.json row {id:?} left out ({e})");
                rules.rows.remove(&id);
            }
        }
    }
    Ok(Data {
        root: root.to_owned(),
        rules,
        tricks,
        strings: with_skate3_tricks(read(root, "strings.json")?, root),
        intros: scenes.scenes.into_iter().map(|(k, s)| (k, s.duration)).collect(),
        filled: false,
    })
}

/// Tricks only Skate 3 has (Lateflip Darkcatch) aren't in Skate 2's table: their names come from the Skate 3 HUD's
/// strings (private/hud/runtime/trickdisplay.json), as old_hud.py fills the Skate 2 trick display's. Skate 2's own
/// names win.
fn with_skate3_tricks(mut strings: HashMap<String, String>, root: &Path) -> HashMap<String, String> {
    let Some(private) = root.ancestors().find(|p| p.file_name().is_some_and(|n| n == "private")) else { return strings };
    let Ok(hud) = read::<Json>(&private.join("hud/runtime"), "trickdisplay.json") else { return strings };
    for (id, name) in hud["language"].as_object().into_iter().flatten() {
        if let (true, Some(name)) = (id.starts_with("ID_TRICK_"), name.as_str()) {
            strings.entry(id.clone()).or_insert_with(|| name.to_owned());
        }
    }
    strings
}

impl Data {
    fn text(&self, id: &str) -> String {
        self.strings.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }
    /// A template message (`CentralMessageHALID...`), localized.
    fn say(&self, field: &str) -> String {
        self.text(self.rules.template.get(field).map_or(field, String::as_str))
    }
    fn kind(&self, id: &str) -> Option<u32> {
        self.tricks.get(id).and_then(|t| t.2)
    }
    /// Tricks Skate 2 lacks keep the engine's label and TrickType, and no gesture.
    fn fill(&mut self, definitions: &[skate_data::scoring::Definition]) {
        for d in definitions {
            self.tricks.entry(d.identifier.to_owned()).or_insert_with(|| (d.label.clone(), "none".into(), Some(d.trick_type)));
        }
        self.filled = true;
    }
    /// hudabd's trick data: [label id, gesture ("none" hides it), spin in degrees].
    fn trick_data(&self, trick: Option<&Trick>) -> Json {
        let Some(t) = trick else { return json!(["", "none", 0]) };
        let (label, gesture) = self.tricks.get(&t.id).map_or((t.id.as_str(), "none"), |(l, g, _)| (l.as_str(), g.as_str()));
        json!([label, gesture, t.spin])
    }
    fn setup(&self, row: &Row) -> Setup {
        let rules = &self.rules;
        Setup { max_letters: rules.max_letters, limit: row.time_limit, valid: rules.valid_trick_types.clone(), timings: rules.timings }
    }
}

// ---------------------------------------------------------------- the rules
/// A landed trick as S.K.A.T.E. compares them: the air's first scorable and its spin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Trick {
    /// Scorable identifier ("kickflip").
    pub id: String,
    /// Degrees, unsigned (Trick.GetCompletedAir180Count * 180).
    pub spin: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Fail {
    Bailed,
    TimeUp,
    LeftSpot,
    /// The leader's trick was not a Flip or Air trick.
    Invalid,
    /// The leader's trick is on the already-been-done list.
    Repeated,
    NotCopied,
}

/// What the attempter's game saw; every game judges it the same way ([`Turn::result`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Outcome {
    /// An air landed after a spot volume was entered: its trick, the trick's TrickType, every spot entered.
    Landed { trick: Trick, kind: u32, spots: bool },
    Bailed,
    TimeUp,
    /// Left a spot volume on the ground.
    LeftSpot,
    /// Skate 3's rule (user 10-28): skateroffboard after a spot volume was entered (S3 skate.lua 619-627, 1149-1156).
    OffBoard,
}

/// One attempt's outcome, for the attempt (`Turn::attempt`) by the player (index) it names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AttemptResult {
    pub player: usize,
    pub attempt: u32,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Player {
    pub name: String,
    pub letters: u8,
    /// Final place: 1 the winner; set when out.
    pub rank: Option<u8>,
    /// Left the session.
    pub gone: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Setup {
    pub max_letters: u8,
    /// TimeLimit: seconds per attempt.
    pub limit: f32,
    /// TrickTypes a set may be (Flip, Air).
    pub valid: Vec<u32>,
    pub timings: Timings,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Step {
    /// wait_for_leader / wait_for_follower: on the spawn until the pre-attempt time is up or the start volume is left.
    Ready,
    /// wait_for_scoring / wait_for_copy: the attempt timer runs.
    Attempt,
    /// leader_success / leader_failure / follower_copy / follower_not_copy.
    Result(Option<Fail>),
    /// process_round found the winner (the leader).
    Won,
    /// show_results.
    Over,
}

/// One AI.RepositionAI: at its batch's cut to black `player` goes to the attempt spawn (`spawn`, whose 6th argument
/// also switches every game's camera to it) or to its waiting slot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Move {
    pub cut: f32,
    pub player: usize,
    pub spawn: bool,
}

/// The whole game's state. Seconds are the caller's clock (`match_core::match_now`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Turn {
    /// Slot order.
    pub players: Vec<Player>,
    pub leader: usize,
    /// FollowerOrder: this round's followers.
    pub followers: Vec<usize>,
    /// FollowerIndex into `followers` while they copy; None while the leader sets.
    pub follower: Option<usize>,
    pub set: Option<Trick>,
    /// The copy attempt's trick (Skate.UpdateAttemptItem).
    pub tried: Option<Trick>,
    /// The already-been-done list, oldest first.
    pub done: Vec<Trick>,
    pub all_copied: bool,
    pub step: Step,
    /// When the step began; a Ready behind a RepositionAI fade begins as it ends (teleporttransitionfinished).
    pub since: f32,
    /// The fade in progress: its cut to black, where its batch of moves is made, and its end.
    pub cut: f32,
    pub fade_until: f32,
    /// RepositionAI moves not yet taken by the game, oldest first.
    pub moves: Vec<Move>,
    /// CachedTeleportID: the last attempter, sent back to its waiting slot by the next prepare_follower / leader_failure.
    pub cached: Option<usize>,
    /// The attempt timer's end.
    pub ends: f32,
    /// LeaderboardSetTurn's player (wait_for_leader 370, wait_for_follower's 0.25 s timer 838).
    pub marker: Option<usize>,
    /// CurrentRank: the place the next player out takes.
    pub next_rank: u8,
    pub online: bool,
    pub setup: Setup,
    pub round: u32,
    /// Bumped at every attempt's Ready; an [`AttemptResult`] names it.
    pub attempt: u32,
    /// PartyPlay: every attempt waits for its Press-A card once its fade is over (S3 round_setup / prepare_follower's
    /// OpenPressADialog at teleporttransitionfinished; hudcomplete moves on).
    #[serde(default)]
    pub party: bool,
    /// The card is up (or due once the fade ends): still round_setup / prepare_follower.
    #[serde(default)]
    pub waiting_a: bool,
}

impl Turn {
    /// `leader`: the first leader (Skate 3's rule, user 10-28: drawn at random, S3 skate.lua 137; [`SkateGame::begin`]).
    pub fn new(names: Vec<String>, online: bool, setup: Setup, leader: usize, now: f32) -> Self {
        Self::with_party(names, online, false, setup, leader, now)
    }
    /// [`Turn::new`], Skate 3's PartyPlay path when `party`.
    pub fn with_party(names: Vec<String>, online: bool, party: bool, setup: Setup, leader: usize, now: f32) -> Self {
        let n = names.len() as u8;
        let players = names.into_iter().map(|name| Player { name, letters: 0, rank: None, gone: false }).collect();
        let mut turn = Turn {
            players,
            leader,
            followers: Vec::new(),
            follower: None,
            set: None,
            tried: None,
            done: Vec::new(),
            all_copied: false,
            step: Step::Ready,
            since: now,
            cut: now,
            fade_until: f32::NEG_INFINITY,
            moves: Vec::new(),
            cached: None,
            ends: now,
            marker: None,
            next_rank: n,
            online,
            setup,
            round: 0,
            attempt: 0,
            party,
            waiting_a: false,
        };
        // check_players (193-203): everyone to its waiting slot; round_setup's leader request joins this fade.
        for p in 0..turn.players.len() {
            turn.reposition(p, now, DEFAULT_FADE, false);
        }
        // Online with one skater: show_results at once (236-238).
        if online && n == 1 {
            (turn.step, turn.since) = (Step::Over, now);
        } else {
            turn.round_setup(now);
        }
        turn
    }
    /// AI.RepositionAI with a fade: a request while a fade runs joins its batch; online none for a player who left
    /// (Online.IsPlayerDisconnected guards every call).
    fn reposition(&mut self, p: usize, now: f32, fade: f32, spawn: bool) {
        if self.online && self.players[p].gone {
            return;
        }
        if now >= self.fade_until {
            (self.cut, self.fade_until) = (now + fade / 2., now + fade + FADE_HOLD);
        }
        self.moves.push(Move { cut: self.cut, player: p, spawn });
    }
    /// Ready behind its fade (or, PartyPlay, its Press-A card): the Lua is still in round_setup / prepare_follower.
    pub fn moving(&self, now: f32) -> bool {
        self.step == Step::Ready && (now < self.since || self.waiting_a)
    }
    /// PartyPlay: the card's Press-A is due (the fade has ended): (player 1-based, CurrentRound).
    pub fn card(&self, now: f32) -> Option<(usize, i64)> {
        (self.step == Step::Ready && self.waiting_a && now >= self.since).then(|| (self.attempter() + 1, i64::from(self.round)))
    }
    /// PartyPlay's hudcomplete: round_setup / prepare_follower move on (UnpinAI), the attempt's wait begins now.
    pub fn hudcomplete(&mut self, now: f32) {
        if self.card(now).is_some() {
            (self.waiting_a, self.since) = (false, now);
        }
    }
    /// How black the screen is: out over half the fade, black through the hold, in over the other half.
    /// ponytail: linear; the fader's curve is not traced.
    pub fn black(&self, now: f32) -> f32 {
        if now >= self.fade_until {
            return 0.;
        }
        let half = (self.fade_until - self.cut - FADE_HOLD).max(1e-3);
        ((now - (self.cut - half)) / half).min((self.fade_until - now) / half).clamp(0., 1.)
    }
    /// AI.PinAI on the attempter: hot-seat (PartyPlay) behind its fades (round_setup / prepare_follower pin it until the
    /// card's hudcomplete), and once process_round has its winner (pinned as the last CachedTeleportID). The result
    /// states pin NPCs only (624-632, 668-676, 877-885, 916-927), and leader_failure's lone leader goes to show_results
    /// unpinned (its CachedTeleportID is never sent back).
    pub fn pinned(&self, now: f32) -> bool {
        (!self.online && self.moving(now)) || self.step == Step::Won || (self.step == Step::Over && self.cached.is_none())
    }
    /// Who is on the spot: the leader, or the follower copying.
    pub fn attempter(&self) -> usize {
        self.follower.map_or(self.leader, |i| self.followers[i])
    }
    pub fn copying(&self) -> bool {
        self.follower.is_some()
    }
    fn out(&self, p: usize) -> bool {
        self.players[p].gone || self.players[p].letters >= self.setup.max_letters
    }
    /// The next attempt, its attempter repositioned with `fade`: it begins as the fade ends (round_setup /
    /// prepare_follower's teleporttransitionfinished), at once when none runs (online round_setup, 310-320).
    fn ready(&mut self, now: f32, fade: f32) {
        self.reposition(self.attempter(), now, fade, true);
        (self.step, self.since) = (Step::Ready, self.fade_until.max(now));
        self.attempt += 1;
        self.waiting_a = self.party;
    }
    fn round_setup(&mut self, now: f32) {
        let n = self.players.len();
        self.followers = (1..n).map(|k| (self.leader + k) % n).filter(|&p| !self.out(p)).collect();
        (self.follower, self.set, self.tried, self.all_copied) = (None, None, None, false);
        self.round += 1;
        self.ready(now, LEAD_FADE);
    }
    /// The attempt starts (wait_for_scoring / wait_for_copy): the pre-attempt time is up, the start volume was left or
    /// the attempter left the session; its timer is TimeLimit, 1 s for a player who left.
    pub fn go(&mut self, now: f32) {
        if self.step == Step::Ready && !self.moving(now) {
            let limit = if self.players[self.attempter()].gone { GONE_LIMIT } else { self.setup.limit };
            (self.step, self.since, self.ends) = (Step::Attempt, now, now + limit);
        }
    }
    /// The attempt's verdict (aircompleted's checks): None sets or copies the trick.
    pub fn judge(&self, outcome: &Outcome) -> Option<Fail> {
        match outcome {
            Outcome::Bailed => Some(Fail::Bailed),
            Outcome::TimeUp => Some(Fail::TimeUp),
            Outcome::LeftSpot => Some(Fail::LeftSpot),
            // Skate 3's rule (user 10-28): the leader's "invalid trick", a follower's "not copied" (S3 skate.lua 620, 1150).
            Outcome::OffBoard => Some(if self.copying() { Fail::NotCopied } else { Fail::Invalid }),
            Outcome::Landed { kind, .. } if !self.copying() && !self.setup.valid.contains(kind) => Some(Fail::Invalid),
            Outcome::Landed { spots: false, .. } => Some(Fail::LeftSpot),
            Outcome::Landed { trick, .. } if !self.copying() && self.done.contains(trick) => Some(Fail::Repeated),
            Outcome::Landed { trick, .. } if self.copying() && self.set.as_ref() != Some(trick) => Some(Fail::NotCopied),
            Outcome::Landed { .. } => None,
        }
    }
    /// Applies an attempt's outcome; false (ignored) when it isn't the current attempt's.
    pub fn result(&mut self, r: &AttemptResult, now: f32) -> bool {
        let live = matches!(self.step, Step::Ready | Step::Attempt) && !self.moving(now);
        if r.attempt != self.attempt || r.player != self.attempter() || !live {
            return false;
        }
        let fail = self.judge(&r.outcome);
        if !self.copying() {
            // leader_success / leader_failure: CachedTeleportID = LeaderID (633, 653).
            self.cached = Some(r.player);
            // leader_failure online: no follower still here, and the leader wins: show_results at once (680-697).
            if fail.is_some() && self.online && self.followers.iter().all(|&p| self.players[p].gone) {
                self.players[r.player].rank = Some(1);
                (self.step, self.since) = (Step::Over, now);
                return true;
            }
        }
        if let Outcome::Landed { trick, spots: true, .. } = &r.outcome {
            if self.copying() {
                self.tried = Some(trick.clone());
            } else if fail.is_none() {
                self.set = Some(trick.clone());
                self.done.push(trick.clone());
                // ponytail: the list holds ABDListItemLimit (= MaxLetters) tricks; an older one may be set again.
                let over = self.done.len().saturating_sub(usize::from(self.setup.max_letters));
                self.done.drain(..over);
                self.all_copied = true;
            }
        }
        if self.copying() && fail.is_some() {
            self.all_copied = false;
            let player = &mut self.players[r.player];
            player.letters += 1;
            if player.letters >= self.setup.max_letters && !player.gone && player.rank.is_none() {
                player.rank = Some(self.next_rank);
                self.next_rank = self.next_rank.saturating_sub(1);
            }
        }
        (self.step, self.since) = (Step::Result(fail), now);
        true
    }
    /// How long a result holds: a letter runs the stinger and the leaderboard first.
    fn result_time(&self, fail: Option<Fail>) -> f32 {
        let t = self.setup.timings;
        if self.copying() && fail.is_some() { t.letter_wait + t.letter_show + t.letter_after } else { t.after_result }
    }
    pub fn tick(&mut self, now: f32) {
        if self.moving(now) {
            return;
        }
        let t = self.setup.timings;
        let dt = now - self.since;
        match self.step {
            Step::Ready if !self.copying() && self.marker != Some(self.leader) => self.marker = Some(self.leader),
            Step::Ready if self.copying() && dt >= t.follower_settle && self.marker != Some(self.attempter()) => {
                self.marker = Some(self.attempter())
            }
            Step::Ready if dt >= t.pre_attempt + if self.copying() { t.follower_settle } else { 0. } => self.go(now),
            // A player who left: nobody reports its attempt, so every game times it out.
            Step::Attempt if self.players[self.attempter()].gone && now >= self.ends => {
                let r = AttemptResult { player: self.attempter(), attempt: self.attempt, outcome: Outcome::TimeUp };
                self.result(&r, now);
            }
            Step::Result(fail) if dt >= self.result_time(fail) => self.after(fail, now),
            Step::Won if dt >= t.winner => (self.step, self.since) = (Step::Over, now),
            _ => {}
        }
    }
    fn after(&mut self, fail: Option<Fail>, now: f32) {
        match (self.follower, fail) {
            (None, None) => {
                self.follower = Some(0);
                self.prepare(now);
            }
            // leader_failure: the lead passes to the first follower still here (none online went to show_results in
            // `result`); its timer sends the leader back to its waiting slot (702-726).
            (None, Some(_)) => match self.followers.iter().copied().find(|&p| !self.players[p].gone) {
                Some(p) => {
                    self.leader = p;
                    if let Some(c) = self.cached.take() {
                        self.reposition(c, now, DEFAULT_FADE, false);
                    }
                    self.process_round(now);
                }
                None => self.win(now),
            },
            (Some(i), _) => {
                self.follower = Some(i + 1);
                self.prepare(now);
            }
        }
    }
    /// prepare_follower (732-792): the last attempter back to its waiting slot, then the next follower still here
    /// copies, else the round is over.
    fn prepare(&mut self, now: f32) {
        self.tried = None;
        if let Some(c) = self.cached.take() {
            self.reposition(c, now, FOLLOW_FADE, false);
        }
        let mut i = self.follower.unwrap_or(0);
        // ponytail: retail's online search (768-783) wraps around FollowerOrder, skipping only the follower just played:
        // when the followers after a leaver have all copied it hands the turn back to the earlier ones, over and over,
        // and the round never ends (a retail loop, not ported). Here the round ends.
        while self.followers.get(i).is_some_and(|&p| self.players[p].gone) {
            i += 1;
        }
        if i < self.followers.len() {
            self.follower = Some(i);
            self.ready(now, FOLLOW_FADE);
            self.cached = Some(self.followers[i]);
        } else {
            self.process_round(now);
        }
    }
    fn process_round(&mut self, now: f32) {
        // leader_get_letter: online, a set everyone copied passes the lead on.
        if self.online && self.all_copied {
            self.all_copied = false;
            if let Some(&p) = self.followers.first() {
                self.leader = p;
            }
        }
        self.follower = None;
        if (0..self.players.len()).any(|p| p != self.leader && !self.out(p)) {
            self.round_setup(now);
        } else {
            self.win(now);
        }
    }
    fn win(&mut self, now: f32) {
        self.players[self.leader].rank = Some(1);
        (self.follower, self.step, self.since) = (None, Step::Won, now);
    }
    /// playerdisconnected: last place (global, 1385-1396); and the attempter's own handlers: its attempt starts
    /// (wait_for_leader 414-419, wait_for_follower 853-858) or its timer is cut to 1 s (611-617, 1131-1137). Behind the
    /// fade the Lua is still in round_setup / prepare_follower, which have none.
    pub fn gone(&mut self, p: usize, now: f32) {
        let n = self.players.len() as u8;
        let player = &mut self.players[p];
        if std::mem::replace(&mut player.gone, true) {
            return;
        }
        if player.rank.is_none() {
            player.rank = Some(n);
            self.next_rank = self.next_rank.saturating_sub(1);
        }
        if p == self.attempter() {
            match self.step {
                Step::Ready => self.go(now),
                Step::Attempt => self.ends = now + GONE_LIMIT,
                _ => {}
            }
        }
    }
    /// The letters the leaderboard shows: a new letter shows once its stinger has played (LeaderboardHUDSetInt).
    fn shown_letters(&self, p: usize, now: f32) -> u8 {
        let t = self.setup.timings;
        let fresh = matches!(self.step, Step::Result(Some(_))) && self.copying() && p == self.attempter();
        let letters = self.players[p].letters;
        if fresh && now - self.since < t.letter_wait + t.letter_show { letters.saturating_sub(1) } else { letters }
    }
    /// The S-K-A-T-E stinger (ShowSkateMessage) while it plays: (attempt, letters).
    fn stinger(&self, now: f32) -> Option<(u32, u8)> {
        let t = self.setup.timings;
        let dt = now - self.since;
        let letter = matches!(self.step, Step::Result(Some(_))) && self.copying();
        (letter && dt >= t.letter_wait && dt < t.letter_wait + t.letter_show + t.letter_after)
            .then(|| (self.attempt, self.players[self.attempter()].letters))
    }
}

/// The other games' attempts, in order: one for an attempt this game hasn't reached (or whose fade it is still in)
/// waits; a stale one is dropped. Returns the ones it took, for the game to send on.
///
/// Not retail (user 10-12): every game sends on each attempt it takes from another. A player who leaves takes its
/// record with it, so its last attempt reached only the games that read the record in time; the rest timed it out, and
/// the games then played different rounds and never ended (online audit 10-12). Sent on, one copy that got through
/// reaches every game still there. ponytail: it must beat the leave notice plus GONE_LIMIT (~1 s; one hop is ~0.3 s).
/// Online the core now closes that race under it (end review I2, match_core `Leaves`: the mode gets the leave only
/// once every game still here has the leaver's last events); the wire tests here model the old notice, so this stays.
fn take_results(turn: &mut Turn, inbox: &mut Vec<AttemptResult>, now: f32) -> Vec<AttemptResult> {
    let mut taken = Vec::new();
    for r in std::mem::take(inbox) {
        if r.attempt > turn.attempt || (r.attempt == turn.attempt && turn.moving(now)) {
            inbox.push(r);
        } else if turn.result(&r, now) {
            taken.push(r);
        }
    }
    crate::match_core::bound(inbox, "Skate 2 S.K.A.T.E. results");
    taken
}

/// This player's result for the match core (`Match::score`, best first): minus its letters, and for a player out,
/// minus the rank it went out at too (skate.lua 920-921: CurrentRank counts down, so the first out is last), so the
/// core puts the players out in the script's order, not slot order (online audit 10-12). The results screen clamps the
/// count it shows at 5.
/// Online, the results order ([`Match::order`]): by the Lua's rank (the winner 1, then elimination order: CurrentRank
/// counts down, 920-921; a player who left takes the last free rank, 1385-1396, so it goes last), those not ranked yet
/// by their letters; each shown with its letters (FrontEnd.Results_SetPlayerData(id, rank, letters), 1269).
fn placing(turn: &Turn) -> Vec<(usize, i64)> {
    let mut order: Vec<usize> = (0..turn.players.len()).collect();
    order.sort_by_key(|&p| (turn.players[p].rank.unwrap_or(1), turn.players[p].letters));
    order.into_iter().map(|p| (p, -i64::from(turn.players[p].letters))).collect()
}

fn score(turn: &Turn, p: usize) -> i64 {
    let player = &turn.players[p];
    let out = if player.letters >= turn.setup.max_letters { player.rank.map_or(0, i64::from) } else { 0 };
    -i64::from(player.letters) - out
}

/// The central message the Lua shows for the step (text, a key that changes per showing, kind).
fn message(turn: &Turn, row: &Row, data: &Data, now: f32) -> Option<(String, String, Central)> {
    let say = |field: &str| data.say(field);
    let text = match turn.step {
        // round_setup / prepare_follower: nothing new until the fade ends.
        Step::Ready if turn.moving(now) => return None,
        Step::Ready if !turn.copying() => say("CentralMessageHALIDNameSetTrick"),
        Step::Ready if now - turn.since >= turn.setup.timings.follower_settle => say("CentralMessageHALIDNameCopyTrickName"),
        Step::Result(None) if !turn.copying() => say("CentralMessageHALIDNameHasSetTrickName"),
        Step::Result(None) => {
            let last = turn.follower.is_some_and(|i| i + 1 >= turn.followers.len());
            say(if turn.online && last && turn.all_copied { "CentralMessageHALIDNameGetsALetter" } else { "CentralMessageHALIDNameCopiedTrick" })
        }
        Step::Result(Some(fail)) => match fail {
            Fail::Bailed => say("CentralMessageHALIDNameBailed"),
            Fail::TimeUp => say("CentralMessageHALIDOutOfTime"),
            Fail::Repeated => say("CentralMessageHALIDNameRepeatedTrick"),
            Fail::NotCopied => say("CentralMessageHALIDNameNotCopyTrick"),
            // ponytail: ShowModifiedCentralMessage's second line goes under the first in one message.
            Fail::LeftSpot => {
                let why = row.failure.as_deref().map_or_else(|| say("ModifiedCentralMessageHALIDSkateFailureExplanation"), |f| data.text(f));
                format!("{}\n{why}", say("CentralMessageHALIDNameDidNotTrickValidArea"))
            }
            Fail::Invalid => format!("{}\n{}", data.text("ID_SKATE_TRICK_NOT_SET"), data.text("ID_WRNGTRK_BRDGRAB")),
        },
        Step::Over => return Some((say("CentralMessageHALIDChallengeComplete"), String::new(), Central::Complete)),
        _ => return None,
    };
    Some((text, turn.attempt.to_string(), Central::Plain))
}

/// The air's FirstScorable from the tricks it announced, with its TrickType.
/// ponytail: retail's is the air's carrier after conversion (an ollie becomes the kickflip flicked from it); here a
/// launch (an Air trick) followed by a flip counts as the flip.
fn first_scorable(tricks: &[&'static str], kind: impl Fn(&str) -> Option<u32>) -> Option<(&'static str, u32)> {
    let first = *tricks.first()?;
    let k = kind(first).unwrap_or(0);
    if k == AIR
        && let Some(&flip) = tricks.iter().find(|t| kind(t) == Some(FLIP))
    {
        return Some((flip, FLIP));
    }
    Some((first, k))
}

// ---------------------------------------------------------------- engine -> attempts
#[derive(Clone, Debug, PartialEq)]
enum Seen {
    LeftStart,
    /// A spot volume entered or left; `air`: airborne (or touching down) as it happened.
    Spot { index: usize, inside: bool, air: bool },
    /// An air ended on the board: the tricks announced through it, and its signed half-turns.
    Landed { tricks: Vec<&'static str>, turns: i32 },
    /// wipeout: the Wipeout category's rising edge (S2 0x8234d2e0, posted as 0x823348e8 enters state 3); a dismount or
    /// remount is never one.
    Bailed,
    /// skateroffboard: the skater's category became the biped's (500), as skate3_skate_game's.
    OffBoard,
}

/// One physics tick of the local skater.
struct Sample<'a> {
    at: Vec3,
    air: bool,
    /// SkaterRuntime::wiping_out: the published category is Wipeout.
    wiping: bool,
    /// The skater's physical category is the biped's (500): off the board.
    off: bool,
    announced: &'a [&'static str],
    turns: i32,
    dt: f32,
}

/// The engine's signals turned into the Lua's messages (trigger enter/exit, aircompleted, wipeout, skateroffboard).
#[derive(Default)]
struct Probe {
    in_start: bool,
    in_spots: Vec<bool>,
    wiping: bool,
    off: bool,
    air: Option<Vec<&'static str>>,
    /// A touchdown settling: (seconds left, tricks, turns).
    landing: Option<(f32, Vec<&'static str>, i32)>,
}
impl Probe {
    fn step(&mut self, s: &Sample, start: &Volume, spots: &[Volume], out: &mut Vec<Seen>) {
        if s.air {
            self.air.get_or_insert_with(Vec::new).extend(s.announced);
        } else if let Some(mut tricks) = self.air.take() {
            tricks.extend(s.announced);
            self.landing = Some((LANDING_GRACE, tricks, s.turns));
        } else if let Some((left, tricks, _)) = &mut self.landing {
            tricks.extend(s.announced);
            *left -= s.dt;
        }
        if s.wiping && !self.wiping {
            // aircompleted's Wipedout: the air in progress doesn't count.
            (self.air, self.landing) = (None, None);
            out.push(Seen::Bailed);
        }
        if s.off && !self.off {
            (self.air, self.landing) = (None, None);
            out.push(Seen::OffBoard);
        }
        (self.wiping, self.off) = (s.wiping, s.off);
        if self.landing.as_ref().is_some_and(|l| l.0 <= 0.) {
            let (_, tricks, turns) = self.landing.take().expect("checked");
            out.push(Seen::Landed { tricks, turns });
        }
        let air = s.air || self.landing.is_some();
        let inside = start.contains(s.at);
        if self.in_start && !inside {
            out.push(Seen::LeftStart);
        }
        self.in_start = inside;
        self.in_spots.resize(spots.len(), false);
        for (index, (was, volume)) in self.in_spots.iter_mut().zip(spots).enumerate() {
            let inside = volume.contains(s.at);
            if inside != *was {
                *was = inside;
                out.push(Seen::Spot { index, inside, air });
            }
        }
    }
}

/// The local attempt: the Lua's per-attempt flags.
#[derive(Default)]
struct Watch {
    /// SpotVolumeEntered.
    entered: Vec<bool>,
}
impl Watch {
    fn outcome(&mut self, turn: &mut Turn, seen: Vec<Seen>, kind: impl Fn(&str) -> Option<u32>, now: f32) -> Option<Outcome> {
        let mut out = None;
        for s in seen {
            match s {
                // round_setup / prepare_follower handle none of these.
                _ if turn.moving(now) => {}
                Seen::LeftStart if turn.step == Step::Ready => turn.go(now),
                _ if turn.step != Step::Attempt || out.is_some() => {}
                Seen::Spot { index, inside: true, .. } => self.entered[index] = true,
                // "Left spot volume and not tricking".
                Seen::Spot { air: false, .. } => out = Some(Outcome::LeftSpot),
                // Skate 3's rule (user 10-28): a wipeout fails the attempt at its start, not at its wipeoutover (S3
                // skate.lua 451-475, 986-1010: leader_failure / follower_not_copy at once).
                Seen::Bailed => out = Some(Outcome::Bailed),
                // Skate 3's rule (user 10-28): stepping off the board once a spot was entered (OkToScore) fails it (S3
                // skate.lua 619-627, 1149-1156).
                Seen::OffBoard if self.entered.contains(&true) => out = Some(Outcome::OffBoard),
                // aircompleted counts once a spot was entered (OkToScore).
                Seen::Landed { tricks, turns } if self.entered.contains(&true) => {
                    if let Some((id, kind)) = first_scorable(&tricks, &kind) {
                        let trick = Trick { id: id.into(), spin: turns.abs() * 180 };
                        out = Some(Outcome::Landed { trick, kind, spots: !self.entered.contains(&false) });
                    }
                }
                _ => {}
            }
        }
        if out.is_none() && turn.step == Step::Attempt && now >= turn.ends {
            out = Some(Outcome::TimeUp);
        }
        out
    }
}

// ---------------------------------------------------------------- Bevy side
#[derive(Resource, Default)]
pub(crate) struct SkateGame {
    data: Option<Data>,
    /// The match's turn state; online every game holds the same one.
    pub turn: Option<Turn>,
    /// Online: this game's own attempts, for the sync to send to the others in order.
    pub outbox: Vec<AttemptResult>,
    /// Online: the others' attempts as the sync receives them (stale ones are dropped, early ones wait).
    pub inbox: Vec<AttemptResult>,
    /// This player's index in `turn.players`.
    me: usize,
    /// The row set up for this match (its intro started).
    row: Option<String>,
    intro_ends: Option<f32>,
    probe: Probe,
    seen: Vec<Seen>,
    watch: Watch,
    /// The attempt `watch` is for.
    placed: u32,
    /// The player the camera is on (the last attempter repositioned with RepositionAI's 6th argument); PartyPlay, the
    /// one the pad drives too (SetPlayer at the cut).
    watching: usize,
    /// wait_for_leader's Pause.EnablePauseMenuAndBackpack has run (make_hud disabled it).
    pause_lifted: bool,
    message: Option<(String, String, Central)>,
    /// When `message` first showed: it plays out after [`crate::skate3_modes::CENTRAL_FOR`].
    message_at: f32,
    screens: TutorialScreens,
    hud: Hud,
    warned: String,
    /// PartyPlay: the attempt whose card was opened.
    carded: u32,
    /// PartyPlay: play the game again (hudretry, restartchallenge).
    restart: bool,
}

pub(crate) struct Skate2SkateGamePlugin;
impl Plugin for Skate2SkateGamePlugin {
    fn build(&self, app: &mut App) {
        let root = app.world().get_resource::<Config>().map(|c| c.asset_root.join(DIR)).unwrap_or_default();
        let data = load(&root).inspect_err(|e| warn!("Skate 2 S.K.A.T.E.: no data at {} ({e})", root.display())).ok();
        if let (Some(data), Some(mut playable)) = (&data, app.world_mut().get_resource_mut::<Playable>()) {
            playable.0.extend(data.rules.rows.iter().filter(|(_, r)| r.online).map(|(id, _)| id.clone()));
        }
        app.insert_resource(SkateGame { data, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

fn probe(mut game: ResMut<SkateGame>, m: Res<Match>, skater: Res<SkaterRuntime>, time: Res<Time>) {
    let SkateGame { data, turn, probe, seen, .. } = &mut *game;
    let row = data.as_ref().and_then(|d| d.rules.rows.get(&m.row)).filter(|_| m.running(Mode::Skate) && turn.is_some());
    let Some(row) = row else {
        *probe = Probe::default();
        return;
    };
    let s = &skater.scoring;
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let sample = Sample {
        at: Vec3::new(at[0], at[1], at[2]),
        air: s.in_air(),
        wiping: skater.wiping_out(),
        off: skater.player_input.physical.state.category_12 == 500,
        announced: &s.announced,
        turns: s.spin_turns,
        dt: time.delta_secs(),
    };
    probe.step(&sample, &row.start_volume, &row.spot_volumes, seen);
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<SkateGame>| {
        let Some(mut data) = game.data.take() else { return };
        if !data.filled && world.resource::<Match>().mode == Some(Mode::Skate) {
            data.fill(&world.resource::<SkaterRuntime>().scoring.data.definitions);
        }
        game.step(world, &data);
        game.data = Some(data);
    });
}

/// RepositionAI's move, through the match core's teleport (gated by match_core::teleport_blocked, retried each tick),
/// as every other mode moves its skater. M1 (online audit 10-06): a direct travel_to failed while another teleport was
/// queued (PlayerInputState::request_teleport), and the move was dropped with a warning.
fn travel(world: &mut World, at: &Matrix) {
    world.resource_mut::<Match>().teleport = Some(*at);
}

impl SkateGame {
    fn step(&mut self, world: &mut World, data: &Data) {
        let now = crate::match_core::match_now(world);
        let (mode, phase, id, token, owned) = {
            let m = world.resource::<Match>();
            (m.mode, m.phase.clone(), m.row.clone(), m.token, m.owned)
        };
        // A Skate 1 row (skate1_online owns its match) is not ours.
        let live = !owned && mode == Some(Mode::Skate) && matches!(phase, Phase::Intro { .. } | Phase::Countdown { .. } | Phase::Running { .. });
        let on_map = world.get_resource::<CurrentMap>().and_then(|m| m.path.as_deref()?.file_stem().map(|s| s == MAP)).unwrap_or(false);
        let Some(row) = data.rules.rows.get(&id).filter(|_| live && on_map) else {
            if self.row.is_some() {
                self.end(world);
            }
            if live && self.warned != id {
                warn!("Skate 2 S.K.A.T.E.: no row {id:?} to play here (data {}, on {MAP}: {on_map})", data.root.display());
                self.warned = id;
            }
            return;
        };
        if self.row.is_none() {
            info!("Skate 2 S.K.A.T.E.: {id} ({})", if token == 0 { "hot-seat" } else { "online" });
            self.row = Some(id);
            // Solo: on the spot for the intro (online the core puts each player on its start slot).
            if token == 0 {
                travel(world, &row.spawn_at());
            }
            let intro = row.intro.as_ref().and_then(|s| Some((s, *data.intros.get(s)?)));
            if let Some((scene, seconds)) = intro.filter(|_| matches!(phase, Phase::Intro { .. })) {
                world.resource_mut::<Cutscene>().start(&data.root, scene);
                world.resource_mut::<Match>().intro = true;
                self.intro_ends = Some(now + seconds);
            }
        }
        if self.intro_ends.is_some_and(|end| now >= end || !matches!(phase, Phase::Intro { .. })) {
            self.intro_ends = None;
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
        if matches!(phase, Phase::Running { .. }) {
            if self.turn.is_none() {
                self.begin(world, data, row, now);
            }
            self.play(world, data, row, now);
        }
        {
            // skate.lua has no countdown at its start (check_players' fade, then wait_for_leader's 5 s pre-attempt), so
            // the core's is skipped. make_hud (9) disables the pause menu and backpack until wait_for_leader (363), after
            // the first fade, or check_players alone online (237); setup_skate (133) hides the trick display, complete
            // (1369) shows it again.
            self.pause_lifted |= self.turn.as_ref().is_some_and(|t| !t.moving(now));
            let over = self.turn.as_ref().is_some_and(|t| t.step == Step::Over && now - t.since >= RESULTS_HOLD);
            let mut m = world.resource_mut::<Match>();
            m.no_countdown = true;
            m.pause_disabled = !self.pause_lifted;
            m.trick_display_hidden = self.turn.is_some() && !over;
        }
        let black = self.turn.as_ref().map_or(0., |t| t.black(now));
        if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>()
            && fade.0 != black
        {
            fade.0 = black;
        }
        self.show(world, data, row, &phase, now);
        if std::mem::take(&mut self.restart) {
            info!("Skate 2 S.K.A.T.E.: {} again", row.title);
            self.restarted(world);
        }
    }

    fn begin(&mut self, world: &mut World, data: &Data, row: &Row, now: f32) {
        let m = world.resource::<Match>();
        let online = m.token != 0;
        // Online: the match's players by slot (the turn's player index is the slot).
        // Offline: Party Play's players (two without a session).
        let (mut names, me): (Vec<String>, usize) =
            if online { (m.players.clone(), m.slot) } else { (crate::party_play::names(world), 0) };
        if names.is_empty() {
            names.push("Player 1".into());
        }
        // Skate 3's rule (user 10-28): the first leader is drawn at random (S3 skate.lua 137), the same on every console:
        // the match token, as skate3_skate_game's draw (PartyPlay from the clock).
        let seed = if online { m.token } else { u64::from(now.to_bits()) };
        let leader = (seed % names.len() as u64) as usize;
        info!("Skate 2 S.K.A.T.E.: {} players, this one {me}, leader {leader}", names.len());
        self.turn = Some(Turn::with_party(names, online, !online, data.setup(row), leader, now));
        (self.me, self.watching) = (me, me);
        if let Some(mut arrows) = world.get_resource_mut::<Arrows>() {
            if !arrows.more.iter().any(|m| m == INDICATORS) {
                arrows.more.push(INDICATORS.into());
            }
            arrows.wanted = row.arrows();
        }
    }

    fn play(&mut self, world: &mut World, data: &Data, row: &Row, now: f32) {
        let Some(turn) = self.turn.as_mut() else { return };
        if turn.online {
            // The core relays attempts (match_core `@ev` records) and blanks the name of a slot whose player left.
            let mut m = world.resource_mut::<Match>();
            let sent = self.outbox.drain(..).filter_map(|r| serde_json::to_string(&r).ok());
            m.outbox.extend(sent);
            // S2 (release review 10-18): a player's result only from that player's own events (a leaver's reach us under
            // its slot through match_core `Leaves`), never one a peer made up for another player.
            let received = m.inbox.drain(..).filter_map(|(slot, r)| {
                let r: AttemptResult = serde_json::from_str(&r).ok()?;
                if r.player != slot {
                    warn!("Skate 2 S.K.A.T.E.: slot {slot} sent a result for player {}; dropped", r.player);
                    return None;
                }
                Some(r)
            });
            self.inbox.extend(received);
            let left: Vec<usize> = (0..turn.players.len()).filter(|&p| m.players.get(p).is_none_or(String::is_empty)).collect();
            for p in left {
                turn.gone(p, now);
            }
        }
        let attempter = turn.attempter();
        let local = !turn.online || attempter == self.me;
        if turn.attempt != self.placed {
            self.placed = turn.attempt;
            self.watch = Watch { entered: vec![false; row.spot_volumes.len()] };
            self.seen.clear();
        }
        // RepositionAI's batches at their cut to black: this skater's own moves, and the camera to a repositioned
        // attempter. Hot-seat has one skater: the attempters' moves only.
        let due = turn.moves.iter().take_while(|m| m.cut <= now).count();
        let mut to = None;
        for m in turn.moves.drain(..due) {
            if m.spawn {
                self.watching = m.player;
            }
            if if turn.online { m.player == self.me } else { m.spawn } {
                to = Some(if m.spawn { row.spawn_at() } else { row.wait_at(self.me) });
            }
        }
        if let Some(at) = to {
            travel(world, &at);
        }
        let seen = std::mem::take(&mut self.seen);
        if local
            && let Some(outcome) = self.watch.outcome(turn, seen, |t| data.kind(t), now)
        {
            let r = AttemptResult { player: attempter, attempt: turn.attempt, outcome };
            if turn.result(&r, now) && turn.online {
                self.outbox.push(r);
            }
        }
        // Online the results are no longer sent on: each comes from its own player (S2), a leaver's last ones through
        // match_core `Leaves`, so a copy would only be dropped at every console.
        take_results(turn, &mut self.inbox, now);
        turn.tick(now);
        if turn.party {
            self.party(world, data, row, now);
        }
        let Some(turn) = self.turn.as_ref() else { return };
        let me = &turn.players[self.me];
        let out = me.gone || me.letters >= turn.setup.max_letters;
        let over = turn.step == Step::Over && now - turn.since >= RESULTS_HOLD;
        // PinAI on everyone but the attempter (203, 710-717, 750-756), unpinned from its own reposition on (308, 773) and
        // through its result; the camera is on the last attempter repositioned.
        let attempter = turn.attempter();
        let local = !turn.online || attempter == self.me;
        let score = score(turn, self.me);
        let mut m = world.resource_mut::<Match>();
        m.score = score;
        m.order = if turn.online { placing(turn) } else { Vec::new() };
        // PartyPlay's game ends at its dialog, not at the core's results.
        m.finished = (over && !turn.party) || turn.online && out;
        m.hold = !local || turn.pinned(now);
        // Skate 3's rule (user 10-28, s3rules S2 item 5): the D-pad emotes go through this mode's own waits, as
        // skate3_skate_game's (skate3_modes::gestures); not once show_results' hold is over.
        m.gestures = crate::skate3_modes::gestures(&m, over);
        m.watch = (turn.online && self.watching != self.me).then_some(self.watching);
    }

    /// PartyPlay (party_play::exchange), as skate3_skate_game's: the card (OpenPressADialog at the fade's end), the
    /// pad's player (the attempter its RepositionAI last moved, at the cut), the others' waiting slots
    /// (TurnBasedWaitingLocation, slot = player id), the results dialog once "Challenge Complete!" has held; back come
    /// hudcomplete, hudretry and restartchallenge.
    fn party(&mut self, world: &mut World, data: &Data, row: &Row, now: f32) {
        let Some(turn) = self.turn.as_mut() else { return };
        let card = turn.card(now).filter(|_| self.carded != turn.attempt);
        if card.is_some() {
            self.carded = turn.attempt;
        }
        let controlled = self.watching;
        let waiting = (0..turn.players.len()).map(|p| (p != controlled).then(|| row.wait_at(p))).collect();
        let over = turn.step == Step::Over && now - turn.since >= RESULTS_HOLD;
        let results = over.then(|| {
            let rows: Vec<_> = (placing(turn).into_iter())
                .map(|(p, _)| (p, turn.players[p].name.clone(), i64::from(turn.players[p].rank.unwrap_or(1)), i64::from(turn.players[p].letters)))
                .collect();
            crate::party_play::standings(&data.text(&row.title), &rows, "ID_COMMON_SCORE", THROWDOWN_SKATE)
        });
        let answers = crate::party_play::exchange(world, crate::party_play::Turn { controlled, waiting, card, results });
        if answers.hud_complete {
            turn.hudcomplete(now);
        }
        self.restart |= answers.retry || answers.restart;
    }

    /// PartyPlay's Rematch and Restart Challenge: the game again on this row (restarted -> setup_skate: a new leader,
    /// no intro), the pad back to player 1.
    fn restarted(&mut self, world: &mut World) {
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            let layer = screens.layer(GAME);
            for name in [ABD, BOARD, LETTERS] {
                layer.close(name);
            }
        }
        self.hud = Hud::default();
        (self.turn, self.placed, self.carded, self.pause_lifted, self.message) = (None, 0, 0, false, None);
        self.seen.clear();
        self.watching = self.me;
    }

    fn show(&mut self, world: &mut World, data: &Data, row: &Row, phase: &Phase, now: f32) {
        let goofy = world.resource::<SkaterRuntime>().animation.goofy();
        // No start countdown (skate.lua's start has none); the core's, were it ever shown.
        let countdown = match *phase {
            Phase::Countdown { ends } => Some((1, (ends - now).ceil().max(1.) as i64)),
            _ => None,
        };
        // A message shows until the next one, for HUD2's central time at most (0x82664550's 180 updates, as Skate 3's
        // modes): "Trick not copied" stayed up through the whole hand-over to the next player.
        if let Some(message) = self.turn.as_ref().and_then(|t| message(t, row, data, now)) {
            if self.message.as_ref() != Some(&message) {
                self.message_at = now;
            }
            self.message = Some(message);
        }
        let timer = (self.turn.as_ref())
            .filter(|t| t.step == Step::Attempt)
            .map(|t| f64::from((t.ends - now).max(0.)));
        let result = self.message.clone().filter(|_| f64::from(now - self.message_at) < crate::skate3_modes::CENTRAL_FOR);
        let view = View { goofy, countdown, timer, result, ..default() };
        self.screens.sync(world, &view);
        if let Some(turn) = &self.turn {
            self.hud.drive(world, data, row, turn, self.me, goofy, now);
        }
    }

    fn end(&mut self, world: &mut World) {
        if self.intro_ends.is_some() {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
        self.screens.clear(world);
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            let layer = screens.layer(GAME);
            for name in [ABD, BOARD, LETTERS] {
                layer.close(name);
            }
        }
        if self.turn.is_some()
            && let Some(mut arrows) = world.get_resource_mut::<Arrows>()
        {
            arrows.wanted.clear();
            arrows.more.retain(|m| m != INDICATORS);
        }
        if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
            fade.0 = 0.;
        }
        let (data, screens) = (self.data.take(), std::mem::take(&mut self.screens));
        *self = SkateGame { data, screens, ..default() };
    }
}

// ---------------------------------------------------------------- HUD movies
/// What hudabd shows.
#[derive(Clone, PartialEq)]
struct Abd {
    set: Json,
    copy: Json,
    list: Vec<Json>,
    list_visible: bool,
    regular: bool,
}
impl Abd {
    fn new(data: &Data, turn: &Turn, goofy: bool) -> Self {
        Abd {
            set: data.trick_data(turn.set.as_ref()),
            copy: data.trick_data(turn.tried.as_ref()),
            list: turn.done.iter().map(|t| data.trick_data(Some(t))).collect(),
            // round_setup shows the list; prepare_follower hides it.
            list_visible: !turn.copying(),
            regular: !goofy,
        }
    }
    fn answers(&self, data: &Data) -> Vec<(String, Json)> {
        let names = self.list.iter().map(|t| t[0].clone());
        let spins = self.list.iter().map(|t| t[2].clone());
        let mut out = vec![
            ("HUDComponents.PlayerStance_IsRegular".to_owned(), json!(self.regular)),
            ("HUDComponents.ABD_GetTrickSetData".into(), self.set.clone()),
            ("HUDComponents.ABD_GetTricCopyData".into(), self.copy.clone()),
            ("HUDComponents.ABD_GetNumTricksInList".into(), json!(self.list.len())),
            ("HUDComponents.ABD_GetTrickHalNames".into(), Json::Array(names.chain(spins).collect())),
            ("HUDComponents.ABD_GetMoveToUpperPosition".into(), json!(false)),
        ];
        // A label with a spin is localized here before the spin goes on.
        for t in self.list.iter().chain([&self.set, &self.copy]) {
            if let Some(id) = t[0].as_str().filter(|id| !id.is_empty()) {
                out.push((format!("FELanguage.GetLocalizedFormatString({id},0)"), json!(data.text(id))));
            }
        }
        out
    }
}

/// leaderboardhud's throwdown answers: names, and per player [position, letters, text, highlight, turn].
fn board(data: &Data, row: &Row, turn: &Turn, me: usize, goofy: bool, now: f32) -> Vec<(String, Json)> {
    let n = turn.players.len();
    let attempter = turn.attempter();
    let block: Vec<Json> = (0..n)
        .flat_map(|p| {
            // Online the local player's name is highlighted; hot-seat, the one on the spot.
            let highlight = if turn.online { p == me } else { p == attempter };
            // The turn arrow: LeaderboardSetTurn (370, 838).
            [json!(p + 1), json!(turn.shown_letters(p, now)), json!(""), json!(highlight), json!(turn.marker == Some(p))]
        })
        .collect();
    vec![
        ("Mission.GetIsOnlineMission".into(), json!(turn.online)),
        ("Mission.GetTitle".into(), json!(data.text(&row.title))),
        ("HUDComponents.Leaderboard_GetLeaderboardNameBlock".into(), json!(turn.players.iter().map(|p| &p.name).collect::<Vec<_>>())),
        ("HUDComponents.Leaderboard_GetNumPlayers".into(), json!(n)),
        ("HUDComponents.Leaderboard_GetLayoutType".into(), json!(LAYOUT_SKATE)),
        // ID_COMMON_UNKNOWN: no contest variant.
        ("HUDComponents.Leaderboard_GetLeaderboardVariantName".into(), json!("ID_COMMON_UNKNOWN")),
        ("FELanguage.GetLocalizedFormatString(ID_COMMON_UNKNOWN,0)".into(), json!(data.text("ID_COMMON_UNKNOWN"))),
        ("HUDComponents.Leaderboard_GetLeaderboardDataBlock".into(), Json::Array(block)),
        // UpdateVOIPIcons' GetVoipStatus()[p]: voice::voip_status by the player's name, online.
        ("FELeaderboard.GetVoipStatus".into(), json!(turn.players.iter().map(|p| if turn.online { crate::voice::voip_named(&p.name) } else { 0 }).collect::<Vec<_>>())),
        ("HUDComponents.PlayerStance_IsRegular".into(), json!(!goofy)),
    ]
}

fn set_state(screen: &mut Screen, state: i64) -> Result<(), String> {
    screen.call("SetState", &[json!(state)]).map(drop)
}

#[derive(Default)]
struct Hud {
    /// Screens that failed stay closed for the match.
    failed: BTreeSet<&'static str>,
    abd: Option<Abd>,
    board: Option<Vec<(String, Json)>>,
    stinger: Option<(u32, u8)>,
}
impl Hud {
    #[allow(clippy::too_many_arguments)]
    fn drive(&mut self, world: &mut World, data: &Data, row: &Row, turn: &Turn, me: usize, goofy: bool, now: f32) {
        // round_setup shows the leaderboard and the ABD as its fade ends (316-318, 332-334).
        if self.abd.is_none() && turn.moving(now) {
            return;
        }
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
        let layer = screens.layer(GAME);
        let dir = data.root.join("screens/runtime");

        let mut abd = Abd::new(data, turn, goofy);
        // Behind a fade: round_setup refreshes the set and copy tricks and shows the list as it ends (312-317);
        // prepare_follower clears the copy trick at once (740) and hides the list as it ends (798).
        if let Some(last) = self.abd.as_ref().filter(|_| turn.moving(now)) {
            if turn.copying() {
                abd.list_visible = last.list_visible;
            } else {
                abd = last.clone();
            }
        }
        if self.abd.as_ref() != Some(&abd) {
            let last = self.abd.replace(abd.clone());
            self.with(layer, &dir, ABD, "hudabd", |screen, fresh| {
                for (k, v) in abd.answers(data) {
                    screen.answer(&k, v);
                }
                // SetState 1: show + UpdateAll; 4 UpdateList, 5 UpdateSetTrick, 6 UpdateCopyTrick (its success / fail
                // sound and animation), 7 / 8 the list shown / hidden.
                let last = last.filter(|_| !fresh);
                let set = last.as_ref().is_none_or(|l| l.regular != abd.regular || l.set != abd.set);
                let copy = set || last.as_ref().is_none_or(|l| l.copy != abd.copy);
                if fresh {
                    set_state(screen, 1)?;
                } else {
                    if last.as_ref().is_none_or(|l| l.list != abd.list) {
                        set_state(screen, 4)?;
                    }
                    if set {
                        set_state(screen, 5)?;
                    }
                    if copy {
                        set_state(screen, 6)?;
                    }
                }
                if last.as_ref().is_none_or(|l| l.list_visible != abd.list_visible) {
                    set_state(screen, if abd.list_visible { 7 } else { 8 })?;
                }
                Ok(())
            });
        }

        let answers = board(data, row, turn, me, goofy, now);
        if self.board.as_ref() != Some(&answers) {
            let renamed = self.board.as_ref().is_none_or(|b| b[2] != answers[2]);
            self.board = Some(answers.clone());
            self.with(layer, &dir, BOARD, "leaderboardhud", |screen, fresh| {
                for (k, v) in answers {
                    screen.answer(&k, v);
                }
                // 3 SetupScreen (layout, title, RefreshAll), 1 show, 5 RefreshAll; 6 UpdateVOIPIcons (RefreshAll leaves
                // the mics).
                if fresh || renamed {
                    set_state(screen, 3)?;
                }
                set_state(screen, if fresh { 1 } else { 5 })?;
                set_state(screen, 6)
            });
        }

        let stinger = turn.stinger(now);
        if stinger != self.stinger {
            self.stinger = stinger;
            layer.close(LETTERS);
            if let Some((_, letters)) = stinger {
                // SetState(n) plays the n-th letter in, then its outro.
                self.with(layer, &dir, LETTERS, "hudthrowmes", |screen, _| set_state(screen, i64::from(letters)));
            }
        }
    }

    /// Runs `f` on screen `name`, first opening `movie` from `dir` when it isn't open (`fresh`). A screen that fails is
    /// closed for the rest of the match.
    fn with(
        &mut self,
        layer: &mut Layer,
        dir: &Path,
        name: &'static str,
        movie: &str,
        f: impl FnOnce(&mut Screen, bool) -> Result<(), String>,
    ) {
        if self.failed.contains(name) {
            return;
        }
        let fresh = !layer.open.contains_key(name);
        let result = (|| {
            if fresh {
                layer.open.insert(name.to_owned(), Screen::load(&dir.join(format!("{movie}.json")), &[])?);
            }
            f(layer.open.get_mut(name).expect("opened above"), fresh)
        })();
        if let Err(e) = result {
            warn!("Skate 2 S.K.A.T.E.: {movie}: {e}");
            layer.close(name);
            self.failed.insert(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v12 list 2: a Skate 3-only trick read "ID_TRICK_FLIP_LATEFLIP_DARKCATCH"; Skate 3's HUD strings name it, and
    /// Skate 2's own name for a trick stays.
    #[test]
    fn skate3_only_tricks_take_skate3s_names() {
        let dir = std::env::temp_dir().join(format!("s3r-skate2-names-{}", std::process::id()));
        let root = dir.join("private/skate2-online/skate");
        std::fs::create_dir_all(dir.join("private/hud/runtime")).unwrap();
        std::fs::write(
            dir.join("private/hud/runtime/trickdisplay.json"),
            r#"{"language": {"ID_TRICK_FLIP_LATEFLIP_DARKCATCH": "Lateflip Darkcatch", "ID_TRICK_FLIP_KICKFLIP": "Skate 3 Kickflip", "ID_MENU_X": "Menu"}}"#,
        )
        .unwrap();
        let own = HashMap::from([("ID_TRICK_FLIP_KICKFLIP".to_owned(), "Kickflip".to_owned())]);
        let strings = with_skate3_tricks(own, &root);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(strings["ID_TRICK_FLIP_LATEFLIP_DARKCATCH"], "Lateflip Darkcatch");
        assert_eq!(strings["ID_TRICK_FLIP_KICKFLIP"], "Kickflip");
        assert!(!strings.contains_key("ID_MENU_X"), "only trick names");
    }

    const TIMINGS: Timings = Timings {
        pre_attempt: 5.,
        follower_settle: 0.25,
        after_result: 2.,
        letter_wait: 3.,
        letter_show: 2.5,
        letter_after: 2.,
        winner: 2.,
    };
    fn setup() -> Setup {
        Setup { max_letters: 5, limit: 15., valid: vec![AIR, FLIP], timings: TIMINGS }
    }
    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("P{i}")).collect()
    }
    fn trick(id: &str, spin: i32) -> Trick {
        Trick { id: id.into(), spin }
    }
    fn landed(id: &str, spin: i32) -> Outcome {
        Outcome::Landed { trick: trick(id, spin), kind: FLIP, spots: true }
    }
    /// The attempter's outcome once its fade is over, then time on until the next attempt (or the end).
    fn play(t: &mut Turn, outcome: Outcome, now: &mut f32) -> Option<Fail> {
        *now = now.max(t.since);
        let r = AttemptResult { player: t.attempter(), attempt: t.attempt, outcome };
        assert!(t.result(&r, *now), "{r:?} at {:?}", t.step);
        let Step::Result(fail) = t.step else { panic!() };
        *now += 10.;
        t.tick(*now);
        fail
    }

    #[test]
    fn a_copy_needs_the_same_trick_and_spin() {
        let mut now = 0.;
        let mut t = Turn::new(names(2), false, setup(), 0, now);
        assert_eq!((t.leader, t.followers.as_slice(), t.step), (0, &[1][..], Step::Ready));
        assert_eq!(play(&mut t, landed("kickflip", 180), &mut now), None, "set");
        assert_eq!((t.attempter(), t.set.clone()), (1, Some(trick("kickflip", 180))));
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), Some(Fail::NotCopied), "no spin");
        assert_eq!(t.tried, None, "the next attempt starts clean");
        // The leader keeps leading offline; the same set again is a repeat.
        assert_eq!(play(&mut t, landed("kickflip", 180), &mut now), Some(Fail::Repeated));
        assert_eq!((t.leader, t.players[1].letters), (1, 1), "a failed set passes the lead");
        assert_eq!(play(&mut t, landed("heelflip", 0), &mut now), None);
        assert_eq!(play(&mut t, landed("heelflip", 0), &mut now), None, "copied");
        assert_eq!(t.players[0].letters, 0);
    }

    #[test]
    fn the_leader_must_flip_or_air_inside_the_spots() {
        let mut t = Turn::new(names(2), false, setup(), 0, 0.);
        let grind = Outcome::Landed { trick: trick("50_50", 0), kind: 4, spots: true };
        assert_eq!(t.judge(&grind), Some(Fail::Invalid));
        assert_eq!(t.judge(&Outcome::Landed { trick: trick("kickflip", 0), kind: FLIP, spots: false }), Some(Fail::LeftSpot));
        assert_eq!(t.judge(&Outcome::Landed { trick: trick("ollie", 0), kind: AIR, spots: true }), None);
        assert_eq!(t.judge(&Outcome::Bailed), Some(Fail::Bailed));
        t.done.push(trick("ollie", 0));
        assert_eq!(t.judge(&landed("ollie", 0)), Some(Fail::Repeated));
        assert_eq!(t.judge(&landed("ollie", 180)), None, "another spin is another trick");
        // Results for another attempt or player are ignored, and the attempt's own until its fade is over.
        let since = t.since;
        let stale = AttemptResult { player: 0, attempt: t.attempt - 1, outcome: Outcome::Bailed };
        let other = AttemptResult { player: 1, attempt: t.attempt, outcome: Outcome::Bailed };
        let early = AttemptResult { player: 0, attempt: t.attempt, outcome: Outcome::Bailed };
        assert!(!t.result(&stale, since) && !t.result(&other, since) && !t.result(&early, since - 0.1));
        // The attempt starts after the pre-attempt time, counted from the fade's end.
        t.tick(since);
        t.tick(since + 4.9);
        assert_eq!(t.step, Step::Ready);
        t.tick(since + 5.);
        assert_eq!((t.step, t.ends), (Step::Attempt, since + 5. + 15.));
    }

    /// check_players' default fade sends everyone to its waiting slot and round_setup's leader request joins it: one
    /// cut to black at half the fade, Ready once it and its hold are over. prepare_follower's 1.5 s fade sends the
    /// leader back and the follower to the spawn; leader_failure's 1 s fade sends the leader back and round_setup's
    /// request joins.
    #[test]
    fn repositions_fade_and_batch_as_retail() {
        let mut t = Turn::new(names(3), true, setup(), 0, 10.);
        let at = |cut: f32, moves: &[(usize, bool)]| moves.iter().map(|&(player, spawn)| Move { cut, player, spawn }).collect::<Vec<_>>();
        assert_eq!(t.moves, at(10.5, &[(0, false), (1, false), (2, false), (0, true)]));
        assert_eq!((t.since, t.cut), (10. + DEFAULT_FADE + FADE_HOLD, 10.5));
        assert!(t.moving(11.9) && !t.moving(12.));
        assert_eq!((t.black(10.), t.black(10.25), t.black(10.5), t.black(11.5), t.black(11.75), t.black(12.)), (0., 0.5, 1., 1., 0.5, 0.));
        assert_eq!(message(&t, &row(), &data(), 11.), None, "\"Set the trick\" waits for the fade");
        t.tick(11.);
        assert_eq!(t.marker, None, "LeaderboardSetTurn too");
        t.tick(12.);
        assert_eq!(t.marker, Some(0));
        assert!(!t.pinned(11.) && Turn::new(names(2), false, setup(), 0, 0.).pinned(1.), "hot-seat pins behind its fade");
        t.moves.clear();
        let mut now = 12.;
        play(&mut t, landed("kickflip", 0), &mut now);
        assert_eq!(t.moves, at(now + FOLLOW_FADE / 2., &[(0, false), (1, true)]), "prepare_follower");
        assert_eq!(t.since, now + FOLLOW_FADE + FADE_HOLD);
        t.moves.clear();
        // The follower misses, the next follower misses, and a new round: the follower is sent back, the leader leads.
        play(&mut t, Outcome::Bailed, &mut now);
        play(&mut t, Outcome::Bailed, &mut now);
        assert_eq!((t.round, t.attempter()), (2, 0));
        t.moves.clear();
        assert_eq!(play(&mut t, Outcome::TimeUp, &mut now), Some(Fail::TimeUp));
        assert_eq!(t.leader, 1, "leader_failure: the first follower leads");
        assert_eq!(t.moves, at(now + DEFAULT_FADE / 2., &[(0, false), (1, true)]), "leader_failure's 1 s fade, round_setup joins");
    }

    /// M1 (online audit 10-06): RepositionAI's move is queued on the match core's gated teleport (Match::teleport;
    /// match_core::teleport_blocked retries it each tick), as every other glue's, never a direct travel_to (dropped while
    /// a teleport was queued). At check_players' black this console's skater goes to its waiting slot, over the core's
    /// own queued spawn.
    #[test]
    fn repositions_go_through_the_cores_gated_teleport() {
        let queued = [[9.; 4]; 4];
        let mut world = World::new();
        world.insert_resource(Match { mode: Some(Mode::Skate), token: 7, slot: 1, players: names(2), teleport: Some(queued), ..default() });
        let (data, row) = (data(), row());
        let turn = Turn::new(names(2), true, setup(), 0, 0.);
        let cut = turn.cut;
        let mut game = SkateGame { turn: Some(turn), me: 1, watching: 1, ..default() };
        game.play(&mut world, &data, &row, cut - 0.01);
        assert_eq!(world.resource::<Match>().teleport, Some(queued), "nothing before the cut");
        game.play(&mut world, &data, &row, cut);
        assert_eq!(world.resource::<Match>().teleport, Some(row.wait_at(1)), "the waiting slot, at black, on the core");
    }

    /// M2 (online audit 10-06): a malformed row (rules.json, or its slots in rows.json) is left out; the others load.
    #[test]
    fn a_malformed_row_is_left_out_not_the_mode() {
        let dir = std::env::temp_dir().join(format!("s3r-skate2-rows-{}", std::process::id()));
        let root = dir.join("skate");
        std::fs::create_dir_all(&root).unwrap();
        let id = json!([[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]);
        let good = json!({"title": "T", "online": true, "time_limit": 15, "failure": null, "intro": null, "spawn": id,
            "wait": id, "start_volume": {"min": [0, 0, 0], "max": [1, 1, 1]}, "spot_volumes": [], "indicators": []});
        let timings = serde_json::to_value(TIMINGS).unwrap();
        let rules = json!({"template": {}, "max_letters": 5, "timings": timings, "valid_trick_types": [1, 2],
            "rows": {"good": good, "bad_slots": good, "bad": {"title": "T", "spawn": null}}});
        let slots = json!({"slots": [id, id]});
        let rows = json!({"modes": {"skate": [
            {"id": "good", "TurnBasedAttemptSpawnPoint": slots, "TurnBasedWaitingLocation": slots},
            {"id": "bad_slots", "TurnBasedAttemptSpawnPoint": null, "TurnBasedWaitingLocation": slots}]}});
        for (path, v) in [(root.join("rules.json"), rules), (dir.join("rows.json"), rows), (root.join("gestures.json"), json!({})),
            (root.join("tutorial.json"), json!({"scenes": {}})), (root.join("strings.json"), json!({}))]
        {
            std::fs::write(path, v.to_string()).unwrap();
        }
        let loaded = load(&root);
        std::fs::remove_dir_all(&dir).ok();
        let data = loaded.unwrap();
        assert_eq!(data.rules.rows.keys().collect::<Vec<_>>(), ["good"]);
        assert_eq!(data.rules.rows["good"].wait_slots.len(), 2);
    }

    /// Skate 2's online S.K.A.T.E. edge cases: a leader who fails with no follower still here wins at once
    /// (leader_failure -> show_results), unpinned; a disconnected attempter's attempt runs 1 s (after its 5 s wait when
    /// it left before, at once when it leaves during wait_for_leader, 1 s from leaving during the attempt).
    #[test]
    fn online_leavers_follow_skate_lua() {
        let mut t = Turn::new(names(2), true, setup(), 0, 0.);
        t.gone(1, 0.);
        let now = t.since;
        assert!(t.result(&AttemptResult { player: 0, attempt: t.attempt, outcome: Outcome::Bailed }, now));
        assert_eq!((t.step, t.players[0].rank), (Step::Over, Some(1)));
        assert!(!t.pinned(now), "leader_failure pins NPCs only");

        let mut t = Turn::new(names(3), true, setup(), 0, 0.);
        let since = t.since;
        t.tick(since);
        t.gone(0, since + 1.);
        assert_eq!((t.step, t.ends), (Step::Attempt, since + 1. + GONE_LIMIT), "wait_for_leader's playerdisconnected");
        t.tick(since + 1.9);
        assert_eq!(t.step, Step::Attempt);
        t.tick(since + 2.);
        assert_eq!(t.step, Step::Result(Some(Fail::TimeUp)));

        let mut t = Turn::new(names(3), true, setup(), 0, 0.);
        let since = t.since;
        t.tick(since);
        t.tick(since + 5.);
        t.gone(0, since + 7.);
        assert_eq!(t.ends, since + 7. + GONE_LIMIT, "wait_for_scoring's: the timer cut to 1 s");

        // A follower who leaves behind prepare_follower's fade: wait_for_follower runs its 0.25 + 5 s, then 1 s.
        let mut now = 0.;
        let mut t = Turn::new(names(3), true, setup(), 0, now);
        play(&mut t, landed("kickflip", 0), &mut now);
        let since = t.since;
        t.gone(1, since - 0.1);
        assert_eq!(t.step, Step::Ready);
        for dt in [0., 0.25, 5.25] {
            t.tick(since + dt);
        }
        assert_eq!((t.step, t.ends), (Step::Attempt, since + 5.25 + GONE_LIMIT));
        t.tick(since + 6.25);
        assert_eq!(t.step, Step::Result(Some(Fail::TimeUp)));
    }

    /// M15: the result states pin NPCs only: a human attempter rides free through its result until the next
    /// reposition pins it; process_round's winner is pinned.
    #[test]
    fn the_attempter_stays_unpinned_through_its_result() {
        let mut t = Turn::new(names(2), false, setup(), 0, 0.);
        let mut now = t.since;
        let r = AttemptResult { player: 0, attempt: t.attempt, outcome: Outcome::Bailed };
        assert!(t.result(&r, now) && !t.pinned(now));
        now += 10.;
        t.tick(now);
        for spin in 0..5 {
            play(&mut t, landed("kickflip", 180 * spin), &mut now);
            play(&mut t, Outcome::TimeUp, &mut now);
        }
        assert_eq!(t.step, Step::Won);
        assert!(t.pinned(now));
    }

    #[test]
    fn five_letters_is_out_and_the_last_one_standing_wins() {
        let mut now = 0.;
        let mut t = Turn::new(names(3), false, setup(), 0, now);
        for round in 0..5 {
            assert_eq!(play(&mut t, landed("kickflip", 180 * round), &mut now), None);
            assert_eq!(play(&mut t, Outcome::Bailed, &mut now), Some(Fail::Bailed), "P1 misses");
            if round < 4 {
                assert_eq!(play(&mut t, Outcome::TimeUp, &mut now), Some(Fail::TimeUp), "P2 misses");
            } else {
                assert_eq!(play(&mut t, landed("kickflip", 720), &mut now), None, "P2 copies the last");
            }
        }
        assert_eq!((t.players[1].letters, t.players[1].rank), (5, Some(3)), "out first: last place");
        assert_eq!(t.players[2].letters, 4);
        // P1 is out: only P2 follows now.
        assert_eq!(t.followers, [2]);
        assert_eq!(play(&mut t, landed("heelflip", 0), &mut now), None);
        assert_eq!(play(&mut t, landed("nollie", 0), &mut now), Some(Fail::NotCopied));
        assert_eq!((t.step, t.players[2].rank, t.players[0].rank), (Step::Won, Some(2), Some(1)));
        t.tick(now + 2.);
        assert_eq!(t.step, Step::Over);
    }

    #[test]
    fn online_a_set_everyone_copied_passes_the_lead() {
        let mut now = 0.;
        let mut t = Turn::new(names(3), true, setup(), 0, now);
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        let copy = |t: &mut Turn, player| AttemptResult { player, attempt: t.attempt, outcome: landed("kickflip", 0) };
        let first = copy(&mut t, 1);
        now = t.since;
        assert!(t.result(&first, now));
        assert_eq!(message(&t, &row(), &data(), now), Some(("Trick copied!".into(), t.attempt.to_string(), Central::Plain)));
        now += 2.;
        t.tick(now);
        let last = copy(&mut t, 2);
        now = t.since;
        assert!(t.result(&last, now));
        assert_eq!(message(&t, &row(), &data(), now).unwrap().0, "Everyone copied!  New Leader!!");
        now += 2.;
        t.tick(now);
        assert_eq!((t.leader, t.followers.as_slice(), t.round), (1, &[2, 0][..], 2));
        // A player who leaves is last; its copy times out (1 s, after wait_for_follower) and it is skipped from then on.
        assert_eq!(play(&mut t, landed("heelflip", 0), &mut now), None);
        t.gone(2, now);
        assert_eq!(t.players[2].rank, Some(3));
        let since = t.since;
        for at in [since, since + 0.25, since + 5.25, since + 6.25] {
            t.tick(at);
        }
        assert_eq!(t.step, Step::Result(Some(Fail::TimeUp)));
        t.tick(since + 20.);
        assert_eq!(t.attempter(), 0);
    }

    #[test]
    fn the_probe_turns_engine_signals_into_attempts() {
        let start = Volume { min: [0., -1., 0.], max: [2., 1., 2.] };
        let spot = Volume { min: [0., -1., 5.], max: [2., 5., 7.] };
        let (mut probe, mut seen) = (Probe::default(), Vec::new());
        let mut tick = |z: f32, air: bool, wiping: bool, off: bool, announced: &[&'static str], turns: i32| {
            let s = Sample { at: Vec3::new(1., 0., z), air, wiping, off, announced, turns, dt: 0.1 };
            probe.step(&s, &start, std::slice::from_ref(&spot), &mut seen);
        };
        tick(1., false, false, false, &[], 0);
        tick(3., false, false, false, &[], 0);
        tick(5.5, true, false, false, &["ollie"], 0);
        tick(7.5, true, false, false, &["kickflip"], 1);
        tick(9., false, false, false, &[], 1);
        for _ in 0..4 {
            tick(10., false, false, false, &[], 0);
        }
        tick(11., false, true, false, &[], 0);
        tick(11., false, true, false, &[], 0);
        tick(11., false, false, false, &[], 0);
        tick(11., false, false, true, &[], 0);
        tick(11., false, false, true, &[], 0);
        let tricks = vec!["ollie", "kickflip"];
        assert_eq!(seen, [
            Seen::LeftStart,
            Seen::Spot { index: 0, inside: true, air: true },
            Seen::Spot { index: 0, inside: false, air: true },
            Seen::Landed { tricks, turns: 1 },
            Seen::Bailed,
            Seen::OffBoard,
        ]);
        // The attempt (past check_players' fade): the ollie launched the kickflip, half a turn is 180.
        let mut t = Turn::new(names(2), false, setup(), 0, 0.);
        let s = t.since;
        let mut w = Watch { entered: vec![false] };
        let kind = |id: &str| Some(if id == "ollie" { AIR } else { FLIP });
        assert_eq!(w.outcome(&mut t, seen[..1].to_vec(), kind, s - 0.1), None);
        assert_eq!(t.step, Step::Ready, "behind the fade the start volume is not watched");
        assert_eq!(w.outcome(&mut t, seen[..1].to_vec(), kind, s + 1.), None);
        assert_eq!(t.step, Step::Attempt, "leaving the start volume starts it");
        let got = w.outcome(&mut t, seen[1..4].to_vec(), kind, s + 2.);
        assert_eq!(got, Some(Outcome::Landed { trick: trick("kickflip", 180), kind: FLIP, spots: true }));
        // Rolling out of the spot on the ground fails; a landing without a spot entered doesn't count.
        let mut w = Watch { entered: vec![false] };
        let rolled = [Seen::Spot { index: 0, inside: true, air: false }, Seen::Spot { index: 0, inside: false, air: false }];
        assert_eq!(w.outcome(&mut t, rolled.to_vec(), kind, s + 3.), Some(Outcome::LeftSpot));
        let mut w = Watch { entered: vec![false] };
        assert_eq!(w.outcome(&mut t, vec![Seen::Landed { tricks: vec!["kickflip"], turns: 0 }], kind, s + 3.), None);
        assert_eq!(w.outcome(&mut t, vec![], kind, s + 1. + 15.), Some(Outcome::TimeUp));
    }

    /// Skate 3's rules (user 10-28, s3rules S2 items 7 and 9): a wipeout fails the attempt at its start (S3 skate.lua
    /// 451-475, 986-1010), with or without a spot; stepping off the board fails it once a spot volume was entered (619-627,
    /// 1149-1156): the leader's "invalid trick", a follower's "not copied" (a letter).
    #[test]
    fn a_bail_fails_at_once_and_stepping_off_in_the_spot_fails() {
        let kind = |_: &str| Some(FLIP);
        let mut t = Turn::new(names(2), false, setup(), 0, 0.);
        let s = t.since;
        t.tick(s);
        t.tick(s + 5.);
        assert_eq!(t.step, Step::Attempt);
        let mut w = Watch { entered: vec![false] };
        assert_eq!(w.outcome(&mut t, vec![Seen::Bailed], kind, s + 6.), Some(Outcome::Bailed), "at the wipeout's start");
        assert_eq!(w.outcome(&mut t, vec![Seen::OffBoard], kind, s + 6.), None, "off the board before any spot: fine");
        let entered = vec![Seen::Spot { index: 0, inside: true, air: true }, Seen::OffBoard];
        assert_eq!(w.outcome(&mut t, entered, kind, s + 7.), Some(Outcome::OffBoard));
        assert_eq!(t.judge(&Outcome::OffBoard), Some(Fail::Invalid), "the leader: invalidtrick (620)");
        let mut now = s + 7.;
        assert_eq!(play(&mut t, Outcome::OffBoard, &mut now), Some(Fail::Invalid));
        assert_eq!(t.leader, 1, "a failed set passes the lead");
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None, "set");
        assert!(t.copying());
        assert_eq!(t.judge(&Outcome::OffBoard), Some(Fail::NotCopied), "a follower: copyfailed (1150)");
        assert_eq!(play(&mut t, Outcome::OffBoard, &mut now), Some(Fail::NotCopied));
        assert_eq!(t.players[0].letters, 1);
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 4): the first leader is drawn at random, the same on every console: the
    /// match token (as skate3_skate_game's draw). Token 7 over three players: slot 1 leads first, then 2 and 0 follow.
    #[test]
    fn the_first_leader_is_drawn_from_the_match_token() {
        let (data, row) = (data(), row());
        for slot in [0, 2] {
            let mut world = World::new();
            world.insert_resource(Match { mode: Some(Mode::Skate), token: 7, slot, players: names(3), ..default() });
            let mut game = SkateGame::default();
            game.begin(&mut world, &data, &row, 0.);
            let t = game.turn.as_ref().unwrap();
            assert_eq!((t.leader, t.followers.as_slice(), t.attempter()), (1, &[2, 0][..], 1), "console {slot}");
            assert!(t.moves.contains(&Move { cut: t.cut, player: 1, spawn: true }), "the drawn leader to the spawn");
        }
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 5): the D-pad emotes go through while this console waits held
    /// (skate3_modes::gestures), not for the attempter riding, nor once the results take over.
    #[test]
    fn a_waiting_player_can_emote() {
        let (data, row) = (data(), row());
        let mut world = World::new();
        let phase = Phase::Running { started: 0., ends: None };
        world.insert_resource(Match { mode: Some(Mode::Skate), token: 4, slot: 1, players: names(2), phase, ..default() });
        let turn = Turn::new(names(2), true, setup(), 0, 0.);
        let since = turn.since;
        let mut game = SkateGame { turn: Some(turn), me: 1, watching: 1, ..default() };
        game.play(&mut world, &data, &row, since);
        assert!(world.resource::<Match>().hold && world.resource::<Match>().gestures, "slot 0 sets the trick: this one waits");
        game.me = 0;
        game.play(&mut world, &data, &row, since + 0.1);
        assert!(!world.resource::<Match>().hold && !world.resource::<Match>().gestures, "the attempter rides");
        game.me = 1;
        let t = game.turn.as_mut().unwrap();
        (t.step, t.since) = (Step::Over, since);
        game.play(&mut world, &data, &row, since + RESULTS_HOLD);
        assert!(!world.resource::<Match>().gestures, "show_results is over: the results own the pad");
    }

    /// B2 against the engine (the mode harness): getting off the board, walking and getting back on is never a bail (it is
    /// one skateroffboard); a manual bail is one wipeout.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
    fn a_dismount_is_no_bail_a_wipeout_is() {
        use crate::physics::mode_harness::{Harness, Spawn, pads};
        use skate_core::input::xbox::XboxState;
        /// Runs the pad through the probe; whether the skater was off the board.
        fn run(h: &mut Harness, probe: &mut Probe, seen: &mut Vec<Seen>, ticks: u64, pad: impl Fn(u64) -> XboxState) -> bool {
            let nowhere = Volume { min: [1e6; 3], max: [1e6; 3] };
            let mut off = false;
            for t in 0..ticks {
                off |= h.tick(pad(t)).state_category == 500;
                let air = h.skater.scoring.in_air();
                let off = h.skater.player_input.physical.state.category_12 == 500;
                let sample = Sample { at: Vec3::ZERO, air, wiping: h.skater.wiping_out(), off, announced: &[], turns: 0, dt: 1. / 60. };
                probe.step(&sample, &nowhere, &[], seen);
            }
            off
        }
        let mut h = Harness::load("University", Spawn::Package);
        h.run(60, |_| pads::idle());
        let (mut probe, mut seen) = (Probe::default(), Vec::new());
        // Y off at 20, walk, Y back on at 300 (the harness survey's dismount + remount).
        let off = run(&mut h, &mut probe, &mut seen, 500, |t| match t {
            20 | 300 => pads::buttons(pads::Y),
            40..200 => XboxState { left: [0, 24000], ..pads::idle() },
            _ => pads::idle(),
        });
        // The remount's hop may end an air with nothing announced: aircompleted with FirstScorableId -1, which skate.lua
        // ignores (508, 1048) and `first_scorable` drops.
        let trickless = |s: &Seen| matches!(s, Seen::Landed { tricks, .. } if tricks.is_empty());
        seen.retain(|s| !trickless(s));
        assert!(off && seen.contains(&Seen::OffBoard) && !seen.contains(&Seen::Bailed), "off the board {off}; a dismount read as a bail: {seen:?}");
        seen.clear();
        run(&mut h, &mut probe, &mut seen, 600, |t| if t < 30 { pads::bail() } else { pads::idle() });
        assert_eq!(seen.iter().filter(|s| **s == Seen::Bailed).count(), 1, "{seen:?}");
    }

    #[test]
    fn the_first_scorable_is_the_flip_an_ollie_launched() {
        let kind = |id: &str| match id {
            "ollie" | "nollie" => Some(AIR),
            "kickflip" | "heelflip" => Some(FLIP),
            _ => Some(4),
        };
        assert_eq!(first_scorable(&["ollie", "kickflip"], kind), Some(("kickflip", FLIP)));
        assert_eq!(first_scorable(&["ollie"], kind), Some(("ollie", AIR)));
        assert_eq!(first_scorable(&["heelflip", "kickflip"], kind), Some(("heelflip", FLIP)));
        assert_eq!(first_scorable(&["indy", "kickflip"], kind), Some(("indy", 4)), "a grab first: invalid");
        assert_eq!(first_scorable(&[], kind), None);
    }

    fn data() -> Data {
        let template = [
            ("CentralMessageHALIDNameCopiedTrick", "ID_SKATE_TRICK_COPIED"),
            ("CentralMessageHALIDNameGetsALetter", "ID_CHALLENGE_CENTRAL_MESSAGE_NAMEGETSALETTER"),
        ];
        let strings = [
            ("ID_SKATE_TRICK_COPIED", "Trick copied!"),
            ("ID_CHALLENGE_CENTRAL_MESSAGE_NAMEGETSALETTER", "Everyone copied!  New Leader!!"),
            ("ID_TRICK_FLIP_KICKFLIP", "Kickflip"),
        ];
        let kickflip = ("ID_TRICK_FLIP_KICKFLIP".to_owned(), "ollie_kickflip".to_owned(), Some(FLIP));
        Data {
            root: PathBuf::new(),
            rules: Rules {
                template: template.map(|(k, v)| (k.to_owned(), v.to_owned())).into(),
                max_letters: 5,
                timings: TIMINGS,
                valid_trick_types: vec![AIR, FLIP],
                rows: BTreeMap::new(),
            },
            tricks: [("kickflip".to_owned(), kickflip)].into(),
            strings: strings.map(|(k, v)| (k.to_owned(), v.to_owned())).into(),
            intros: HashMap::new(),
            filled: true,
        }
    }
    fn row() -> Row {
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];
        let volume = Volume { min: [0.; 3], max: [1.; 3] };
        Row {
            title: "T".into(),
            online: true,
            time_limit: 15.,
            failure: None,
            intro: None,
            spawn: at,
            wait: at,
            start_volume: volume.clone(),
            spot_volumes: vec![volume],
            indicators: vec![],
            indicator_locators: vec![],
            spawn_slots: vec![],
            wait_slots: vec![],
        }
    }

    /// User 11-01 (bug 5): each VisualIndicators entry is drawn as its own look, which its locator names in the mode's
    /// indicators.json (the orange arrow on "Can You Spell Girl?"); an older export's keep skate2_arrows' fallback.
    #[test]
    fn a_rows_arrows_are_named_for_their_looks() {
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [1033., -52., 718., 1.]];
        let named = Row { indicators: vec![at], indicator_locators: vec!["m_4a_01_skate_carroll_koston_vi01".into()], ..row() };
        assert_eq!(named.arrows(), [("m_4a_01_skate_carroll_koston_vi01".to_owned(), at)]);
        assert_eq!(Row { indicators: vec![at], ..row() }.arrows(), [("skate_spot_0".to_owned(), at)]);
    }

    /// The installed rows' VisualIndicators in their retail looks (indicator/indicators.json, old_skate_game.py): "Can
    /// You Spell Girl?" the orange arrow (type arrow, outline 0001AE2203E38707), Library S.K.A.T.E. two "<" chevrons,
    /// and every online row's entries an outline of the file.
    #[test]
    #[ignore = "requires the installed Skate 2 S.K.A.T.E. data"]
    fn installed_rows_draw_their_retail_looks() {
        let root = PathBuf::from(std::env::var("SKATE2_SKATE_DIR").unwrap());
        let data = load(&root).unwrap();
        let looks: Json = serde_json::from_str(&std::fs::read_to_string(root.join("indicator/indicators.json")).unwrap()).unwrap();
        let shape = |locator: &str| looks["locators"][locator]["shape"].as_str().unwrap_or("none").to_owned();
        let shapes = |id: &str| data.rules.rows[id].arrows().iter().map(|(l, _)| shape(l.as_str())).collect::<Vec<_>>();
        assert_eq!(shapes("m_4a_01_skate_carroll_koston_ol"), ["0001AE2203E38707"]);
        assert_eq!(shapes("t_3b_01_skate_library"), ["0001AE2403E38707", "0001AE2403E38707"]);
        for (id, row) in data.rules.rows.iter().filter(|(_, r)| r.online) {
            for (l, _) in row.arrows() {
                assert!(looks["shapes"][shape(l.as_str()).as_str()].is_array(), "{id}: {l} has no outline");
            }
        }
    }

    /// The set trick's stick gesture follows the local player's stance: hudabd hands gesture_item
    /// PlayerStance_IsRegular, and it mirrors its art (drawn for goofy) for a regular skater, because the same flick
    /// is the other stance's mirror trick (the engine maps a mirrored Kickflip flick to Heelflip).
    #[test]
    fn the_set_gesture_follows_the_local_stance() {
        use crate::input::{gesture_catalog::Group, gesture_mapping::select};
        let data = data();
        let mut t = Turn::new(names(2), false, setup(), 0, 0.);
        t.set = Some(trick("kickflip", 180));
        for goofy in [true, false] {
            let answers: HashMap<_, _> = Abd::new(&data, &t, goofy).answers(&data).into_iter().collect();
            assert_eq!(answers["HUDComponents.PlayerStance_IsRegular"], json!(!goofy));
            assert_eq!(answers["HUDComponents.ABD_GetTrickSetData"], json!(["ID_TRICK_FLIP_KICKFLIP", "ollie_kickflip", 180]));
            assert_eq!(answers["FELanguage.GetLocalizedFormatString(ID_TRICK_FLIP_KICKFLIP,0)"], json!("Kickflip"));
            assert_eq!(answers["HUDComponents.ABD_GetTricCopyData"], json!(["", "none", 0]), "no attempt yet: hidden");
        }
        let mut flick = skate_core::graph::intents::IntentMap::new();
        flick.insert("Kickflip", 1.);
        assert_eq!(select(Group::Square, &flick, false), Some("Kickflip"));
        assert_eq!(select(Group::Square, &flick, true), Some("Heelflip"));
    }

    fn number(screen: &Screen, path: &str, key: &str) -> f64 {
        let clip = screen.clip(path).unwrap_or_else(|| panic!("no clip {path}"));
        match screen.vm.get(clip, key) {
            crate::apt_vm::Value::Number(n) => n,
            other => panic!("{path}.{key} is {other:?}"),
        }
    }
    fn texts(screen: &Screen) -> Vec<String> {
        (screen.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// The installed data (SKATE2_SKATE_DIR = <install>/assets/private/skate2-online/skate): the online rows load
    /// with their volumes and intros, and the HUD movies run: the set kickflip's stick art is as drawn for a goofy
    /// skater and mirrored for a regular one, the leaderboard lists the players, the stinger plays its letter.
    #[test]
    #[ignore = "requires the installed Skate 2 S.K.A.T.E. data"]
    fn skate2_skate_data_and_movies() {
        let root = PathBuf::from(std::env::var("SKATE2_SKATE_DIR").unwrap());
        let data = load(&root).unwrap();
        let online: Vec<_> = data.rules.rows.iter().filter(|(_, r)| r.online).map(|(id, _)| id.as_str()).collect();
        // S2's online S.K.A.T.E. list, as online.json has it (ol_, t_ and the career row's _ol copy).
        assert_eq!(online.len(), 10, "{online:?}");
        for id in online {
            let row = &data.rules.rows[id];
            assert!(row.start_volume.contains(Vec3::from_slice(&row.spawn[3][..3])), "{id}: spawn outside its start volume");
            assert!(!row.spot_volumes.is_empty() && row.time_limit > 0., "{id}");
            // B1: each player waits on its own child of TurnBasedWaitingLocation, not stacked on the locator.
            assert!(row.wait_slots.len() >= 4 && row.wait_at(1) == row.wait_slots[1], "{id}: waiting slots");
            assert!(data.intros.contains_key(row.intro.as_deref().unwrap()), "{id}: no intro");
            assert!(data.say("CentralMessageHALIDNameSetTrick") == "Set the trick" && !data.text(&row.title).starts_with("ID_"));
        }
        assert_eq!(data.kind("kickflip"), Some(FLIP));
        let row = &data.rules.rows["ol_skate_01_ot_plazastairs"];
        let dir = root.join("screens/runtime");
        let mut t = Turn::new(names(2), false, data.setup(row), 0, 0.);
        t.set = Some(trick("kickflip", 0));
        for (goofy, xscale) in [(true, 100.), (false, -100.)] {
            let mut screen = Screen::load(&dir.join("hudabd.json"), &[]).unwrap();
            for (k, v) in Abd::new(&data, &t, goofy).answers(&data) {
                screen.answer(&k, v);
            }
            screen.call("SetState", &[json!(1)]).unwrap();
            let art = ["mcTrick_set.mGestureComp.gestureitem", "mScreen.mcTrick_set.mGestureComp.gestureitem"]
                .into_iter()
                .find(|p| screen.clip(p).is_some())
                .expect("no set gesture");
            assert_eq!(number(&screen, art, "_xscale"), xscale, "goofy {goofy}");
            assert!(texts(&screen).iter().any(|s| s.contains("Kickflip")), "{:?}", texts(&screen));
        }
        let mut screen = Screen::load(&dir.join("leaderboardhud.json"), &[]).unwrap();
        for (k, v) in board(&data, row, &t, 0, false, 0.) {
            screen.answer(&k, v);
        }
        screen.call("SetState", &[json!(3)]).unwrap();
        screen.call("SetState", &[json!(1)]).unwrap();
        let shown = texts(&screen);
        assert!(shown.iter().any(|s| s == "P0") && shown.iter().any(|s| s == "P1"), "{shown:?}");
        let mut screen = Screen::load(&dir.join("hudthrowmes.json"), &[]).unwrap();
        screen.call("SetState", &[json!(2)]).unwrap();
        for _ in 0..120 {
            screen.advance().unwrap();
        }
        assert!(screen.bindings.sounds.iter().any(|s| s == "skate_letter"), "{:?}", screen.bindings.sounds);
    }

    // ------------------------------------------------ six games over the wire (online audit 10-12)
    const SIX: usize = 6;
    /// The leader sets a new flip; a follower copies it, bails, or lets the timer run out (None).
    fn lead(t: &Turn) -> Option<Outcome> {
        Some(landed(&format!("t{}", t.attempt), 0))
    }
    fn copy(t: &Turn) -> Option<Outcome> {
        Some(Outcome::Landed { trick: t.set.clone().unwrap(), kind: FLIP, spots: true })
    }
    fn bail(_: &Turn) -> Option<Outcome> {
        Some(Outcome::Bailed)
    }

    /// Six games over a laggy, lossy wire (150-300 ms, 8 % loss), each on its own clock, each judging its own attempts
    /// (`act(turn, me)`, one second into the attempt; None lets the timer run out) and taking the others' as `play` does.
    /// `leaver` leaves when `leave` holds on its own game: what it still had on the way is lost, and each other game
    /// hears of it after its own delay. Returns each game still there (None: the leaver), once all show the results.
    fn over_the_wire(
        seed: u64,
        leaver: usize,
        leave: impl Fn(&Turn, f32) -> bool,
        act: impl Fn(&Turn, usize) -> Option<Outcome>,
    ) -> Vec<Option<Turn>> {
        let mut wire = crate::skate3_modes::tests::Wire::new(seed, (0.15, 0.3), 0.08);
        let offsets: Vec<f64> = (0..SIX).map(|_| 0.3 * wire.draw()).collect();
        let mut games: Vec<(Turn, Vec<AttemptResult>, u32)> =
            offsets.iter().map(|&o| (Turn::new(names(SIX), true, setup(), 0, o as f32), Vec::new(), 0)).collect();
        let (mut left, mut heard) = (false, [f64::INFINITY; SIX]);
        for frame in 0..60 * 3600 {
            let t = f64::from(frame) / 60.;
            for (i, (turn, inbox, reported)) in games.iter_mut().enumerate() {
                if i == leaver && left {
                    continue;
                }
                let now = (t + offsets[i]) as f32;
                inbox.extend(wire.take(t, i).into_iter().map(|(_, text)| serde_json::from_str::<AttemptResult>(&text).unwrap()));
                if heard[i] <= t {
                    turn.gone(leaver, now);
                }
                if turn.attempter() == i && turn.step == Step::Attempt && *reported != turn.attempt {
                    let outcome = match act(turn, i) {
                        Some(o) if now >= turn.since + 1. => Some(o),
                        None if now >= turn.ends => Some(Outcome::TimeUp),
                        _ => None,
                    };
                    if let Some(outcome) = outcome {
                        let r = AttemptResult { player: i, attempt: turn.attempt, outcome };
                        if turn.result(&r, now) {
                            wire.send(t, i, SIX, vec![serde_json::to_string(&r).unwrap()]);
                        }
                        *reported = turn.attempt;
                    }
                }
                let taken: Vec<String> = take_results(turn, inbox, now).iter().map(|r| serde_json::to_string(r).unwrap()).collect();
                if !taken.is_empty() {
                    wire.send(t, i, SIX, taken);
                }
                turn.tick(now);
                if i == leaver && leave(turn, now) {
                    left = true;
                    for to in 0..SIX {
                        wire.cut(leaver, to);
                        heard[to] = t + 0.15 + 0.15 * wire.draw();
                    }
                }
            }
            if games.iter().enumerate().all(|(i, (turn, ..))| (i == leaver && left) || turn.step == Step::Over) {
                break;
            }
        }
        assert!(left, "the leaver never left");
        games.into_iter().enumerate().map(|(i, (turn, ..))| (i != leaver).then_some(turn)).collect()
    }

    /// Every game still there shows the results, with the same ranks and the same results order (`placing`, the
    /// leaver last), by slot.
    fn agreed(games: &[Option<Turn>]) -> Vec<usize> {
        let first = games.iter().flatten().next().unwrap();
        let ranks: Vec<_> = first.players.iter().map(|p| p.rank).collect();
        for turn in games.iter().flatten() {
            assert_eq!(turn.step, Step::Over);
            assert_eq!(turn.players.iter().map(|p| p.rank).collect::<Vec<_>>(), ranks, "the games disagree");
        }
        for turn in games.iter().flatten() {
            assert_eq!(placing(turn), placing(first), "the games' results disagree");
        }
        placing(first).into_iter().map(|(p, _)| p).collect()
    }

    /// Six players, the last one (P5) leaving as its round 2 copy starts. P0 leads throughout; P1 bails every copy, P2
    /// the odd rounds', P3 the even rounds', P4 never tries. Hand-computed (skate.lua): P5 takes rank 6 when it leaves
    /// (playerdisconnected, 1385-1396); P1 and P4 spell S.K.A.T.E. in round 5 (ranks 5, 4: CurrentRank counts down in
    /// follower order, 920-921), P2 in round 9 (3), P3 in round 10 (2); P0 wins. The results go in rank order.
    #[test]
    fn six_over_the_wire_a_guest_leaves_and_the_results_go_by_rank() {
        let games = over_the_wire(
            11,
            5,
            |t, _| t.round == 2 && t.attempter() == 5 && t.step == Step::Attempt,
            |t, me| match (t.copying(), me) {
                (false, _) => lead(t),
                (true, 1) => bail(t),
                (true, 2) if t.round % 2 == 1 => bail(t),
                (true, 3) if t.round % 2 == 0 => bail(t),
                (true, 4) => None,
                _ => copy(t),
            },
        );
        let ranks: Vec<_> = games[0].as_ref().unwrap().players.iter().map(|p| p.rank).collect();
        assert_eq!(ranks, [1, 5, 3, 2, 4, 6].map(Some));
        assert_eq!(agreed(&games), [0, 3, 2, 4, 1, 5]);
    }

    /// Six players, the host (P0, slot 0) leaving as its round 3 set begins: every game times its attempt out and the
    /// lead goes to P1 (leader_failure). P1 copies everything, P2 bails every copy, P3 and P4 from round 4 on, P5 never
    /// tries. Hand-computed: P0 rank 6; P2 and P5 out in round 6 (5, 4); P3 and P4 in round 8 (3, 2); P1 wins.
    #[test]
    fn six_over_the_wire_the_host_leaves_and_the_results_go_by_rank() {
        let games = over_the_wire(
            12,
            0,
            |t, _| t.round == 3 && t.attempter() == 0 && t.step == Step::Ready,
            |t, me| match (t.copying(), me) {
                (false, _) => lead(t),
                (true, 2) => bail(t),
                (true, 3 | 4) if t.round >= 4 => bail(t),
                (true, 5) => None,
                _ => copy(t),
            },
        );
        let ranks: Vec<_> = games[1].as_ref().unwrap().players.iter().map(|p| p.rank).collect();
        assert_eq!(ranks, [6, 1, 5, 3, 2, 4].map(Some));
        assert_eq!(agreed(&games), [1, 4, 3, 5, 2, 0]);
    }

    /// The host leaves 0.2 s after sending its set: the set reached some games and not others (the wire loses what
    /// a leaver still had on the way). Every game must still play on the same way and agree on the results.
    #[test]
    fn six_over_the_wire_a_set_the_leaver_sent_reaches_every_game_or_none() {
        for seed in 20..26 {
            let games = over_the_wire(
                seed,
                0,
                |t, now| t.round == 2 && !t.copying() && t.step == Step::Result(None) && now >= t.since + 0.2,
                |t, me| match (t.copying(), me) {
                    (false, _) => lead(t),
                    (true, 2) => bail(t),
                    (true, 3 | 4) if t.round >= 4 => bail(t),
                    (true, 5) => None,
                    _ => copy(t),
                },
            );
            agreed(&games);
        }
    }

    /// Party Play (Skate 3's path): every attempt waits on its Press-A card once its fade is over, pinned; A starts the
    /// attempt's pre-attempt wait.
    #[test]
    fn party_attempts_wait_on_their_card() {
        let mut t = Turn::with_party(names(2), false, true, setup(), 1, 0.);
        let since = t.since;
        assert!(since > 0. && t.card(since - 0.1).is_none(), "not before the fade is over");
        assert_eq!(t.card(since), Some((2, 1)));
        t.tick(since + 60.);
        assert!(t.moving(since + 60.) && t.pinned(since + 60.) && t.step == Step::Ready, "waits on the card");
        let early = AttemptResult { player: 1, attempt: t.attempt, outcome: Outcome::Bailed };
        assert!(!t.result(&early, since + 60.), "nothing counts behind the card");
        t.hudcomplete(since + 60.);
        assert!(!t.moving(since + 60.) && t.card(since + 60.).is_none());
        t.tick(since + 64.9);
        assert_eq!(t.step, Step::Ready);
        t.tick(since + 65.);
        assert_eq!(t.step, Step::Attempt, "the pre-attempt time counts from the card's A");
        assert!(Turn::new(names(2), false, setup(), 0, 0.).card(10.).is_none(), "no card off the party path");
    }

    /// Party Play at its max (4): the leader's card, then each follower's in slot order after it.
    #[test]
    fn party_play_hands_each_of_four_players_its_card() {
        let mut now = 0.;
        let mut t = Turn::with_party(names(4), false, true, setup(), 2, now);
        let mut cards = Vec::new();
        for _ in 0..4 {
            now = now.max(t.since);
            cards.push(t.card(now).expect("the card is due").0);
            t.hudcomplete(now);
            assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        }
        now = now.max(t.since);
        cards.push(t.card(now).expect("round 2's card").0);
        assert_eq!(cards, [3, 4, 1, 2, 3], "offline the leader keeps the lead: round 2 starts with it again");
        assert_eq!(t.round, 2);
    }
}
