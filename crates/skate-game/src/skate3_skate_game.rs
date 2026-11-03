//! Skate 3's S.K.A.T.E. (challenge graph `skate` < turn_based_leader < turn_based < default) on the match core, for the
//! online rows of DownTown, Industrial and University. Data: tools/skate3_online.py, installed at
//! `private/skate3-online/skate/`: rules.json (the template's message ids and ABDListItemLimit; per row its time limit,
//! intro, attempt spawn, waiting locator with its 6 slots, start and spot volumes), strings.json, scripts/skate.lua (the
//! decompiled graph: the rules' source, and the tests' oracle), the intro cameras (tutorial.json + scene/camera, played by
//! challenge::cutscene) and the HUD movies (the modes' shared `../hud/runtime`, drawn on the online-ui layer [`ONLINE`]).
//!
//! The rules are skate.lua as [`Turn`] (line numbers below are that file's). What Skate 3 changed from Skate 2's:
//! - the first leader is drawn at random (137); a wipeout fails the attempt at once (452, 987); stepping off the board
//!   after a spot was entered fails it (leader: invalid trick 620, follower: not copied 1150);
//! - prepare_follower decides whether someone has won (755-768); process_round passes the lead on online when every
//!   follower copied (1162, leader_get_letter 1441: no letter); show_results follows the win at once (1183, 705);
//! - turn messages: "<name>, you are the leader" / "<name> is the leader" (388-391), "<name>, your turn!" /
//!   "It's <name>'s turn." (864-867), through HUD2's central and modified-central messages.
//!
//! Every game runs the same [`Turn`]: the attempter's game judges its own attempts and relays them as
//! [`AttemptResult`]s (`Match::outbox`), the others apply them as they arrive. Players go where the Lua's RepositionAI
//! puts them: the attempter onto TurnBasedAttemptSpawnPoint slot 0 (310-317, 791-797), everyone else onto
//! TurnBasedWaitingLocation slot = their player id (171-177, 207-209, 719-726, 773-780); the others are pinned (PinAI)
//! and their camera follows the attempter. The rows are [`Owned`]: no core countdown (the Lua starts none) and our own
//! results (endchallenge_online). Offline (Party Play, or `SKATE_MODE=skate:<row>`) is skate.lua's PartyPlay path on
//! one pad: each attempt's Press-A card (OpenPressADialog, the turn waits for its hudcomplete), the waiting players
//! pinned at their slots (party_play's puppets), and the results dialog (Rematch / Exit Party Play / Change settings).
use crate::{
    apt_screen::{Layer, ONLINE, Screen, Screens},
    challenge::cutscene::Cutscene,
    config::Config,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate2_arrows::Outline,
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

const DIR: &str = "private/skate3-online/skate";
// The online-ui layer's other screens are 0-homscoring, 1-eventlog, 2-roster; draw order is name order.
const ABD: &str = "3-s3skate-abd";
const BOARD: &str = "3-s3skate-board";
const TIMER: &str = "4-s3skate-timer";
const LETTERS: &str = "5-s3skate-throwmes";
const CENTRAL: &str = "6-s3skate-central";
const RESULTS: &str = "7-s3skate-results";
/// leaderboardhud's eLeaderBoardHUD_Skate (online-ui leaderboardhud: mSkate letters per row).
const LAYOUT_SKATE: i64 = 7;
/// Trick.eTrickType: aircompleted takes Flip or Air (527).
const AIR: u32 = 1;
const FLIP: u32 = 2;

// skate.lua's timers (seconds).
/// wait_for_leader / wait_for_follower: PreAttemptTimer (412, 874).
const PRE_ATTEMPT: f32 = 5.;
/// wait_for_follower's settle before the turn messages (841).
const SETTLE: f32 = 0.25;
/// leader_success / leader_failure / follower_copy (646, 709, 915).
const AFTER_RESULT: f32 = 2.;
/// follower_not_copy: to the S-K-A-T-E stinger, to the leaderboard's letter, to the next follower (956, 971, 980).
const LETTER_WAIT: f32 = 3.;
const LETTER_SHOW: f32 = 2.5;
const LETTER_AFTER: f32 = 2.;
/// show_results to the highlight reel (1236).
const RESULTS_HOLD: f32 = 3.;
/// A disconnected attempter's attempt timer (443, 615, 1069, 1145).
const GONE_LIMIT: f32 = 1.;
/// RepositionAI's fade argument: round_setup's leader (309-315), prepare_follower's follower (790-795). The native
/// (0x8284a950 -> 0x82d54c90) batches every request while a fade runs (+232) and fades out over half of it (0x82d5beb0
/// slot 2, +56 = fade * 0.5); at black it moves the batch (0x82d55fa8), waits [`FADE_HOLD`] (+60 = 30 updates,
/// 0x82d55a20), fades in over the other half (slot 1) and posts teleporttransitionfinished (0x82d55940).
pub(crate) const LEAD_FADE: f32 = 1.;
/// RepositionAI's default fade (0x82d54c90 f1 = 1.0): leader_failure sends the leader back with it (719-726).
const DEFAULT_FADE: f32 = 1.;
pub(crate) const FOLLOW_FADE: f32 = 1.5;
/// ponytail: 30 fader updates read as Skate 3's 30 Hz frame; the fader's update rate is not traced.
pub(crate) const FADE_HOLD: f32 = 1.;
/// HUD2's central message (0x82664550: 180 updates, then SetState 5) and a modified one not set to stay (SetState 11).
/// ponytail: read as 3 s; the central HUD's update rate is not traced.
const CENTRAL_FOR: f32 = 3.;
/// ponytail: aircompleted comes as the landing settles; here, still on the board this long after touchdown.
pub(crate) const LANDING_GRACE: f32 = 0.3;
/// A teleport drops the collector for a tick: no wipeout that soon after one (IsAPhysicsTeleport, 453).
const QUIET: f32 = 0.5;
/// The TimerHUD runs red and ticks under this (skate2_screens' TIMER_WARNING; S3's hudtimer is the same movie code).
const TIMER_WARNING: f64 = 10.;

// ---------------------------------------------------------------- data
type Matrix = [[f32; 4]; 4];

#[derive(Deserialize)]
struct Rules {
    /// The template's CentralMessageHALID* fields -> string ids.
    template: HashMap<String, String>,
    /// ABDListItemLimit (setup_skate 51: MaxLetters).
    max_letters: u8,
    #[serde(deserialize_with = "placed_rows")]
    rows: BTreeMap<String, Row>,
}

/// The rows that parse. skate_dwgh_01 (Danny Way's Hawaiian Dream, no ported map) comes with its locators unplaced
/// (null): it is left out, not the whole file (10-06 data: one such row cost every S.K.A.T.E. row its data). Skate 2's
/// S.K.A.T.E. reads its rules the same way (online audit M2).
pub(crate) fn placed_rows<'de, D: serde::Deserializer<'de>, R: DeserializeOwned>(d: D) -> Result<BTreeMap<String, R>, D::Error> {
    let all = BTreeMap::<String, Json>::deserialize(d)?;
    Ok(all
        .into_iter()
        .filter_map(|(id, v)| {
            // Audit 10-28 #15: an unplaced row (a map not ported) is left out on purpose; the warning read like a bug.
            let unplaced = v.get("spawn").is_some_and(Json::is_null);
            serde_json::from_value(v)
                .inspect_err(|e| match unplaced {
                    true => debug!("S.K.A.T.E.: row {id} left out: its map is not ported (its locators are unplaced)"),
                    false => warn!("S.K.A.T.E.: row {id} left out ({e})"),
                })
                .ok()
                .map(|r| (id, r))
        })
        .collect())
}

#[derive(Deserialize)]
struct Row {
    title: String,
    online: bool,
    time_limit: f32,
    intro: Option<String>,
    /// TurnBasedAttemptSpawnPoint.
    spawn: Locator,
    /// TurnBasedWaitingLocation.
    wait: Locator,
    start_volume: Option<Volume>,
    spot_volumes: Vec<Volume>,
    /// VisualIndicators with their look (tools/skate3_online.py chevrons; none in an older export): skate.lua's
    /// AddIndicator draws each, the neon arrow over the spot, from setup to `complete` (skate2_arrows, its indicators.json).
    #[serde(default)]
    visual_indicators: Vec<Indicator>,
    /// The kind-1 VisualIndicators: the trick area's outline on the ground (skate_indu_02, skate_indu_03), drawn by
    /// the same AddIndicator in the S.K.A.T.E. style.
    #[serde(default)]
    outlines: Vec<Outline>,
}

#[derive(Deserialize)]
struct Indicator {
    locator: String,
    /// None: no mission places the locator.
    matrix: Option<Matrix>,
}
/// The rows' indicator looks (shapes and styles) for skate2_arrows.
const INDICATORS: &str = "private/skate3-online/skate/indicators.json";
/// The mode's indicator textures (the outlines' blue among them).
const OUTLINE_DIR: &str = "private/skate3-online/skate/indicator";
/// The row's arrows on (`row`) or off.
fn show_indicators(world: &mut World, row: Option<&Row>) {
    let Some(mut arrows) = world.get_resource_mut::<crate::skate2_arrows::Arrows>() else { return };
    arrows.more.retain(|m| m != INDICATORS);
    arrows.wanted.clear();
    arrows.face_camera = row.is_some();
    arrows.ribbons = row.map_or_else(Vec::new, |row| row.outlines.iter().map(|o| o.ribbon(OUTLINE_DIR)).collect());
    if let Some(row) = row {
        arrows.more.push(INDICATORS.into());
        arrows.wanted = row.visual_indicators.iter().filter_map(|i| Some((i.locator.clone(), i.matrix?))).collect();
    }
}

#[derive(Deserialize)]
struct Locator {
    matrix: Matrix,
    slots: Vec<Matrix>,
}
impl Locator {
    /// RepositionAI's slot: TU3 0x82910eb0, the only code indexing a locator's children (online_map::locator_slot).
    fn slot(&self, i: usize) -> Matrix {
        crate::online_map::locator_slot(&self.matrix, &self.slots, i)
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
    strings: HashMap<String, String>,
    /// intro scene -> seconds
    intros: HashMap<String, f32>,
    /// scorable identifier -> (label id, gesture art or "none", TrickType): the engine's own tricks.
    tricks: HashMap<String, (String, String, u32)>,
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
    let scenes: Scenes = read(root, "tutorial.json")?;
    Ok(Data {
        root: root.to_owned(),
        rules: read(root, "rules.json")?,
        strings: read(root, "strings.json")?,
        intros: scenes.scenes.into_iter().map(|(k, s)| (k, s.duration)).collect(),
        tricks: HashMap::new(),
    })
}

impl Data {
    fn text(&self, id: &str) -> String {
        self.strings.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }
    /// HUD2's message formatting: the string with `{0:s}` filled.
    fn fmt(&self, id: &str, name: &str) -> String {
        self.text(id).replace("{0:s}", name)
    }
    /// A template field's string id.
    fn id(&self, field: &str) -> String {
        self.rules.template.get(field).cloned().unwrap_or_else(|| field.to_owned())
    }
    fn kind(&self, id: &str) -> Option<u32> {
        self.tricks.get(id).map(|t| t.2)
    }
    /// The engine's tricks: label and TrickType from the scoring definitions, the gesture art from the scorable vault's
    /// tTrickGuideInputSlot through the 0x830273C8 table (hud_meters::guides), which the ABD HUD's data (0x82661238)
    /// indexes too.
    fn fill(&mut self, definitions: &[skate_data::scoring::Definition], guides: &[Option<&'static str>]) {
        let ids = skate_core::scoring::catalog::IDENTIFIERS;
        let art: HashMap<&str, &str> = ids.iter().zip(guides).filter_map(|(i, g)| Some((i.0, (*g)?))).collect();
        for d in definitions {
            let gesture = art.get(d.identifier).copied().filter(|g| !g.is_empty()).unwrap_or("none");
            self.tricks.insert(d.identifier.to_owned(), (d.label.clone(), gesture.to_owned(), d.trick_type));
        }
    }
    /// hudabd's trick data: [label id, gesture ("none" hides it), spin in degrees].
    fn trick_data(&self, trick: Option<&Trick>) -> Json {
        let Some(t) = trick else { return json!(["", "none", 0]) };
        let (label, gesture) = self.tricks.get(&t.id).map_or((t.id.as_str(), "none"), |(l, g, _)| (l.as_str(), g.as_str()));
        json!([label, gesture, t.spin])
    }
}

// The map a row is on (skate_artg_01: Art_Gallery; skate_dwgh_01 has no ported map).
use crate::skate3_modes::map_of;

// ---------------------------------------------------------------- the rules
/// A landed trick as S.K.A.T.E. compares them: the air's first scorable and its spin.
/// ponytail: AddTrickToAlreadyBeenDoneList and MatchedPattern are natives (not traced); as Skate 2's port reads them
/// (the same calls, skate2_skate_game.rs), a trick is its scorable and its spin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Trick {
    pub id: String,
    /// Degrees, unsigned (Trick.GetCompletedAir180Count * 180).
    pub spin: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Fail {
    Bailed,
    TimeUp,
    LeftSpot,
    /// The leader's trick was not a Flip or Air trick, or it stepped off the board (invalidtrick).
    Invalid,
    Repeated,
    NotCopied,
}

/// What the attempter's game saw; every game judges it the same way ([`Turn::judge`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Outcome {
    /// aircompleted after a spot volume was entered: the trick, its TrickType, every spot entered.
    Landed { trick: Trick, kind: u32, spots: bool },
    Bailed,
    TimeUp,
    /// Left a spot volume not in the air with a valid launch trick.
    LeftSpot,
    /// skateroffboard after a spot volume was entered.
    OffBoard,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AttemptResult {
    pub player: usize,
    pub attempt: u32,
    pub outcome: Outcome,
}

/// SkatersList's entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Player {
    pub name: String,
    pub letters: u8,
    /// 0 until placed (setup_skate 95).
    pub rank: u8,
    /// disconnected.
    pub gone: bool,
    /// isActive: took part (FinalResults' finish reason, 1300).
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Setup {
    pub max_letters: u8,
    /// TimeLimit.
    pub limit: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Step {
    /// wait_for_leader / wait_for_follower.
    Ready,
    /// wait_for_scoring / wait_for_copy: the attempt timer runs until `Turn::ends`.
    Attempt,
    /// leader_success / leader_failure / follower_copy / follower_not_copy.
    Result(Option<Fail>),
    /// show_results.
    Over,
}

/// The whole game's state. Seconds are the caller's clock (`Time<Real>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Turn {
    /// SkatersList, by player id (the online slot).
    pub players: Vec<Player>,
    pub leader: usize,
    /// FollowerOrder.
    pub followers: Vec<usize>,
    /// FollowerIndex - 1 while a follower copies; None while the leader sets.
    pub follower: Option<usize>,
    pub set: Option<Trick>,
    /// The copy attempt's trick (Skate.UpdateAttemptItem).
    pub tried: Option<Trick>,
    /// The already-been-done list, oldest first.
    pub done: Vec<Trick>,
    pub all_copied: bool,
    pub have_winner: bool,
    pub step: Step,
    /// When the step began; a Ready behind a RepositionAI fade begins as it ends (teleporttransitionfinished).
    pub since: f32,
    /// The fade's cut to black: the attempter's teleport (ChallengeManager.SetPlayer hands the pad over then).
    pub cut: f32,
    /// The RepositionAI fade in progress ends (teleporttransitionfinished); GetTeleportFadeInProgress before it.
    pub fade_until: f32,
    /// CachedTeleportID: the last attempter, sent back to its waiting slot by the next prepare_follower / leader_failure.
    pub cached: Option<usize>,
    /// The attempt timer's end.
    pub ends: f32,
    /// CurrentRank.
    pub next_rank: u8,
    pub online: bool,
    pub setup: Setup,
    pub round: u32,
    /// Bumped at every attempt's Ready; an [`AttemptResult`] names it.
    pub attempt: u32,
    /// LeaderboardSetTurn's player.
    pub marker: Option<usize>,
    /// The attempt whose follower turn wait_for_follower announced (its 0.25 s timer ran out in Ready).
    pub announced: u32,
    /// PartyPlay: every attempt waits for its Press-A card (round_setup / prepare_follower's OpenPressADialog at
    /// teleporttransitionfinished; hudcomplete moves on to wait_for_leader / wait_for_follower).
    #[serde(default)]
    pub party: bool,
    /// The card is up (or due once the fade ends): the Lua is still in round_setup / prepare_follower.
    #[serde(default)]
    pub waiting_a: bool,
}

impl Turn {
    /// setup_skate .. round_setup. `leader`: the drawn first leader (Random.GetRandomNumber(NumSkaters), 137).
    pub fn new(names: Vec<String>, online: bool, setup: Setup, leader: usize, now: f32) -> Self {
        Self::with_party(names, online, false, setup, leader, now)
    }
    /// [`Turn::new`], PartyPlay's path when `party`.
    pub fn with_party(names: Vec<String>, online: bool, party: bool, setup: Setup, leader: usize, now: f32) -> Self {
        let n = names.len() as u8;
        let players = names.into_iter().map(|name| Player { name, letters: 0, rank: 0, gone: false, active: false }).collect();
        let mut turn = Turn {
            players,
            leader,
            followers: Vec::new(),
            follower: None,
            set: None,
            tried: None,
            done: Vec::new(),
            all_copied: false,
            have_winner: false,
            step: Step::Ready,
            since: now,
            cut: now,
            fade_until: f32::NEG_INFINITY,
            cached: None,
            ends: now,
            next_rank: n,
            online,
            setup,
            round: 0,
            attempt: 0,
            marker: None,
            announced: 0,
            party,
            waiting_a: false,
        };
        // check_players: online with one skater goes straight to show_results (244-246).
        if online && n == 1 {
            (turn.step, turn.since) = (Step::Over, now);
        } else {
            turn.round_setup(now);
        }
        turn
    }
    /// Who is on the spot: the leader, or the follower copying.
    pub fn attempter(&self) -> usize {
        self.follower.map_or(self.leader, |i| self.followers[i])
    }
    pub fn copying(&self) -> bool {
        self.follower.is_some()
    }
    /// AI.RepositionAI with its fade: a request while a fade runs joins its batch (0x82d54c90 +232); online, none for a
    /// player who left (Online.IsPlayerDisconnected guards every call).
    fn reposition(&mut self, p: usize, now: f32, fade: f32) {
        if (self.online && self.players[p].gone) || now < self.fade_until {
            return;
        }
        (self.cut, self.fade_until) = (now + fade / 2., now + fade + FADE_HOLD);
    }
    /// The next attempt, the attempter repositioned with `fade`; it begins when the fade in progress ends (round_setup /
    /// prepare_follower's teleporttransitionfinished), at once when none runs (online round_setup, 319).
    fn ready(&mut self, now: f32, fade: f32) {
        self.reposition(self.attempter(), now, fade);
        (self.step, self.since) = (Step::Ready, self.fade_until.max(now));
        self.attempt += 1;
        self.waiting_a = self.party;
    }
    /// How black the screen is: RepositionAI's fade (0x82d5beb0 out over half, black through the cut and FADE_HOLD,
    /// in over the other half), as skate3_modes::Rt::fade_alpha draws it for the other modes.
    pub fn fade_alpha(&self, now: f32) -> f32 {
        let half = self.fade_until - self.cut - FADE_HOLD;
        if half <= 0. || now >= self.fade_until {
            return 0.;
        }
        ((now - (self.cut - half)) / half).min((self.fade_until - now) / half).clamp(0., 1.)
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
    /// round_setup (268-307): FollowerOrder is everyone after the leader, wrapping, with letters to spare and still here.
    fn round_setup(&mut self, now: f32) {
        let n = self.players.len();
        let max = self.setup.max_letters;
        self.followers =
            (1..n).map(|k| (self.leader + k) % n).filter(|&p| self.players[p].letters < max && !self.players[p].gone).collect();
        (self.follower, self.set, self.tried, self.all_copied, self.have_winner) = (None, None, None, false, false);
        self.round += 1;
        self.ready(now, LEAD_FADE);
    }
    /// The attempt starts (wait_for_scoring / wait_for_copy): the pre-attempt time is up, the start volume was left or
    /// the attempter left the session; its timer is TimeLimit, 1 s for a player who left.
    pub fn go(&mut self, now: f32) {
        if self.step == Step::Ready {
            let limit = if self.players[self.attempter()].gone { GONE_LIMIT } else { self.setup.limit };
            (self.step, self.since, self.ends) = (Step::Attempt, now, now + limit);
        }
    }
    /// aircompleted's / skateroffboard's checks: None sets or copies the trick.
    pub fn judge(&self, outcome: &Outcome) -> Option<Fail> {
        let copying = self.copying();
        match outcome {
            Outcome::Bailed => Some(Fail::Bailed),
            Outcome::TimeUp => Some(Fail::TimeUp),
            Outcome::LeftSpot => Some(Fail::LeftSpot),
            Outcome::OffBoard => Some(if copying { Fail::NotCopied } else { Fail::Invalid }),
            // The leader: TrickType first (527), then every spot (536), then the done list (544).
            Outcome::Landed { kind, .. } if !copying && ![FLIP, AIR].contains(kind) => Some(Fail::Invalid),
            Outcome::Landed { spots: false, .. } => Some(Fail::LeftSpot),
            Outcome::Landed { trick, .. } if !copying && self.done.contains(trick) => Some(Fail::Repeated),
            // A follower: every spot (1085), then MatchedPattern (1095).
            Outcome::Landed { trick, .. } if copying && self.set.as_ref() != Some(trick) => Some(Fail::NotCopied),
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
        if let Outcome::Landed { trick, spots: true, .. } = &r.outcome {
            if self.copying() {
                self.tried = Some(trick.clone());
            } else if fail.is_none() {
                self.set = Some(trick.clone());
                self.done.push(trick.clone());
                // ponytail: the list holds ABDListItemLimit tricks (Skate 2's port's reading); an older one may be set again.
                let over = self.done.len().saturating_sub(usize::from(self.setup.max_letters));
                self.done.drain(..over);
                // leader_success (634).
                self.all_copied = true;
            }
        }
        let p = r.player;
        if !self.copying() {
            // leader_success / leader_failure: CachedTeleportID = LeaderID (645, 661).
            self.cached = Some(p);
            // leader_success / leader_failure: isActive (641, 681: its `timeup` field is never set).
            self.players[p].active = true;
            // leader_failure online: the lead goes to the first follower still here; none, and the leader wins (690-706).
            if fail.is_some() && self.online && self.followers.iter().all(|&f| self.players[f].gone) {
                self.win(now);
                return true;
            }
        } else if fail.is_some() {
            // follower_not_copy (929-952).
            self.all_copied = false;
            let max = self.setup.max_letters;
            let player = &mut self.players[p];
            player.letters += 1;
            if player.letters >= max && !player.gone {
                player.rank = self.next_rank;
                self.next_rank = self.next_rank.saturating_sub(1);
            }
            player.active |= fail != Some(Fail::TimeUp);
        } else {
            self.players[p].active = true;
        }
        (self.step, self.since) = (Step::Result(fail), now);
        true
    }
    /// How long a result holds: a letter runs the stinger and the leaderboard first.
    fn result_time(&self, fail: Option<Fail>) -> f32 {
        if self.copying() && fail.is_some() { LETTER_WAIT + LETTER_SHOW + LETTER_AFTER } else { AFTER_RESULT }
    }
    pub fn tick(&mut self, now: f32) {
        if self.moving(now) {
            return;
        }
        let dt = now - self.since;
        match self.step {
            // wait_for_leader's LeaderboardSetTurn (393), as the fade ends.
            Step::Ready if !self.copying() && dt >= 0. && self.marker != Some(self.leader) => self.marker = Some(self.leader),
            Step::Ready if !self.copying() && dt >= PRE_ATTEMPT => self.go(now),
            // wait_for_follower's 0.25 s timer: the turn messages and LeaderboardSetTurn (853-869); never skipped.
            Step::Ready if self.copying() && dt >= SETTLE && self.announced != self.attempt => {
                self.announced = self.attempt;
                self.marker = Some(self.attempter());
            }
            Step::Ready if self.copying() && dt >= SETTLE + PRE_ATTEMPT => self.go(now),
            // A player who left: nobody reports its attempt, so every game times it out.
            Step::Attempt if self.players[self.attempter()].gone && now >= self.ends => {
                let r = AttemptResult { player: self.attempter(), attempt: self.attempt, outcome: Outcome::TimeUp };
                self.result(&r, now);
            }
            Step::Result(fail) if dt >= self.result_time(fail) => self.after(fail, now),
            _ => {}
        }
    }
    fn after(&mut self, fail: Option<Fail>, now: f32) {
        match (self.follower, fail) {
            // leader_success -> prepare_follower (FollowerIndex 1).
            (None, None) => {
                self.follower = Some(0);
                self.prepare(now);
            }
            // leader_failure: FollowerOrder[1] leads (688); online the first one still here (691-696).
            (None, Some(_)) => {
                let next = if self.online { self.followers.iter().copied().find(|&f| !self.players[f].gone) } else { self.followers.first().copied() };
                match next {
                    Some(p) => {
                        self.leader = p;
                        if let Some(c) = self.cached.take() {
                            self.reposition(c, now, DEFAULT_FADE);
                        }
                        self.process_round(now);
                    }
                    None => self.win(now),
                }
            }
            // follower_copy / follower_not_copy: FollowerIndex + 1 (921, 979).
            (Some(i), _) => {
                self.follower = Some(i + 1);
                self.prepare(now);
            }
        }
    }
    /// prepare_follower (743-813).
    fn prepare(&mut self, now: f32) {
        self.tried = None;
        let max = self.setup.max_letters;
        let leader = self.leader;
        self.have_winner = !self.players.iter().enumerate().any(|(p, s)| p != leader && !s.gone && s.letters < max);
        // The last attempter back to its waiting slot (769-783); round_setup's leader request then joins this fade.
        if !self.have_winner && let Some(c) = self.cached.take() {
            self.reposition(c, now, FOLLOW_FADE);
        }
        let mut i = self.follower.unwrap_or(0);
        if self.online {
            while self.followers.get(i).is_some_and(|&p| self.players[p].gone) {
                i += 1;
            }
        }
        if i < self.followers.len() {
            self.follower = Some(i);
            self.ready(now, FOLLOW_FADE);
            self.cached = Some(self.followers[i]);
        } else {
            self.process_round(now);
        }
    }
    /// process_round (1161-1186), with leader_get_letter (1441-1446).
    fn process_round(&mut self, now: f32) {
        if self.online && self.all_copied && !self.have_winner {
            self.all_copied = false;
            if let Some(&p) = self.followers.first() {
                self.leader = p;
            }
        }
        self.follower = None;
        if self.have_winner { self.win(now) } else { self.round_setup(now) }
    }
    fn win(&mut self, now: f32) {
        self.players[self.leader].rank = 1;
        (self.follower, self.step, self.since) = (None, Step::Over, now);
    }
    /// playerdisconnected (global 1418-1429: last place; and the attempter's own handlers: the attempt starts, or its
    /// timer is cut to 1 s, 432-436, 612-617, 882-886, 1142-1147).
    pub fn gone(&mut self, p: usize, now: f32) {
        let n = self.players.len() as u8;
        let player = &mut self.players[p];
        if std::mem::replace(&mut player.gone, true) {
            return;
        }
        if player.rank == 0 {
            player.rank = n;
            self.next_rank = self.next_rank.saturating_sub(1);
        }
        if p == self.attempter() {
            match self.step {
                // wait_for_leader / wait_for_follower's own playerdisconnected (432, 882); behind the fade the Lua is
                // still in round_setup / prepare_follower, which have none.
                Step::Ready if !self.moving(now) => self.go(now),
                Step::Attempt => self.ends = now + GONE_LIMIT,
                _ => {}
            }
        }
    }
    /// The letters the leaderboard shows: LeaderboardHUDSetInt, at prepare_follower (757) and 5.5 s into a new letter
    /// (975).
    fn shown_letters(&self, p: usize, now: f32) -> u8 {
        let fresh = matches!(self.step, Step::Result(Some(_))) && self.copying() && p == self.attempter();
        let letters = self.players[p].letters;
        if fresh && now - self.since < LETTER_WAIT + LETTER_SHOW { letters.saturating_sub(1) } else { letters }
    }
    /// ShowSkateMessage (967) while its stinger plays: (attempt, letters).
    fn stinger(&self, now: f32) -> Option<(u32, u8)> {
        let dt = now - self.since;
        let letter = matches!(self.step, Step::Result(Some(_))) && self.copying();
        (letter && dt >= LETTER_WAIT && dt < LETTER_WAIT + LETTER_SHOW + LETTER_AFTER).then(|| (self.attempt, self.players[self.attempter()].letters))
    }
    /// show_results is over (its 3 s timer; the highlight reel is not played).
    pub fn over(&self, now: f32) -> bool {
        self.step == Step::Over && now - self.since >= RESULTS_HOLD
    }
}

/// HUD2.ShowCentralMessage: (key that changes per showing, text, audio hook).
#[derive(Clone, Debug, PartialEq)]
struct Central {
    key: String,
    text: String,
    hook: u8,
}
/// HUD2.ShowModifiedCentralMessage: (key, text, stays until cleared).
#[derive(Clone, Debug, PartialEq)]
struct Modified {
    key: String,
    text: String,
    stay: bool,
}

/// The central message the Lua last showed for the step; `me` is LocalPlayerID.
fn central(turn: &Turn, data: &Data, me: usize, now: f32) -> Option<Central> {
    if turn.moving(now) {
        return None;
    }
    let a = turn.attempt;
    let name = |p: usize| turn.players[p].name.as_str();
    let say = |field: &str| data.text(&data.id(field));
    let (key, text, hook) = match turn.step {
        // 387-389
        Step::Ready | Step::Attempt if !turn.copying() && turn.leader == me => {
            ("lead", data.fmt(&data.id("CentralMessageHALIDNewLeadYou"), name(me)), 0)
        }
        // 863-864
        Step::Ready | Step::Attempt if turn.copying() && turn.announced == a && turn.attempter() == me => {
            ("turn", data.fmt("ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN", name(me)), 0)
        }
        Step::Result(None) if !turn.copying() => ("set", say("CentralMessageHALIDNameHasSetTrickName"), 0),
        // follower_copy online (890-900): another follower to come, or everyone copied.
        Step::Result(None) => {
            let next = turn.follower.is_some_and(|i| i + 1 < turn.followers.len());
            let field = if turn.online && !next && turn.all_copied { "CentralMessageHALIDNameGetsALetter" } else { "CentralMessageHALIDNameCopiedTrick" };
            ("copied", say(field), 0)
        }
        Step::Result(Some(fail)) => {
            let text = match fail {
                Fail::Bailed => say("CentralMessageHALIDNameBailed"),
                Fail::TimeUp => say("CentralMessageHALIDOutOfTime"),
                Fail::Repeated => say("CentralMessageHALIDNameRepeatedTrick"),
                Fail::LeftSpot => say("CentralMessageHALIDNameDidNotTrickValidArea"),
                Fail::NotCopied => say("CentralMessageHALIDNameNotCopyTrick"),
                Fail::Invalid => data.text("ID_SKATE_TRICK_NOT_SET"),
            };
            ("fail", text, 0)
        }
        // 1224 (eMessageAudioHook.Success; ponytail: read as hudcentral's kAUDIOHOOK_SUCCESS 2) / 1233 (default 0).
        Step::Over => ("over", say("CentralMessageHALIDChallengeComplete"), if turn.online { 0 } else { 2 }),
        _ => return None,
    };
    Some(Central { key: format!("{key}{a}"), text, hook })
}

/// The modified central message the Lua last showed for the step.
fn modified(turn: &Turn, data: &Data, me: usize, now: f32) -> Option<Modified> {
    if turn.moving(now) {
        return None;
    }
    let a = turn.attempt;
    let name = |p: usize| turn.players[p].name.as_str();
    let (key, text, stay) = match turn.step {
        // 389 / 391
        Step::Ready | Step::Attempt if !turn.copying() => match turn.leader == me {
            true => ("set", data.text(&data.id("CentralMessageHALIDNameSetTrick")), false),
            false => ("lead", data.fmt(&data.id("CentralMessageHALIDNewLead"), name(turn.leader)), true),
        },
        // 865 / 867
        Step::Ready | Step::Attempt if turn.announced == a => match turn.attempter() == me {
            true => ("copy", data.text(&data.id("CentralMessageHALIDNameCopyTrickName")), false),
            false => ("turn", data.fmt("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN", name(turn.attempter())), true),
        },
        // 673
        Step::Result(Some(Fail::Invalid)) if !turn.copying() => ("invalid", data.text("ID_WRNGTRK_BRDGRAB"), false),
        _ => return None,
    };
    Some(Modified { key: format!("{key}{a}"), text, stay })
}

/// The air's FirstScorable from the tricks it announced, with its TrickType.
/// ponytail: retail's is the air's carrier after conversion (an ollie becomes the kickflip flicked from it); here a
/// launch (an Air trick) followed by a flip counts as the flip (as skate2_skate_game.rs).
pub(crate) fn first_scorable(tricks: &[&'static str], kind: impl Fn(&str) -> Option<u32>) -> Option<(&'static str, u32)> {
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
    /// aircompleted: the tricks announced through the air, and its signed half-turns.
    Landed { tricks: Vec<&'static str>, turns: i32 },
    Wipeout,
    OffBoard,
}

/// One physics tick of the local skater.
struct Sample<'a> {
    at: Vec3,
    air: bool,
    /// SkaterRuntime::wiping_out (a bail; not getting off the board).
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
        // A wiped-out air is no aircompleted (Wipedout == 1).
        if s.wiping && !self.wiping {
            (self.air, self.landing) = (None, None);
            out.push(Seen::Wipeout);
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

/// The local attempt: the Lua's per-attempt flags (SpotVolumeEntered, OkToScore, Lockout).
#[derive(Default)]
struct Watch {
    entered: Vec<bool>,
    quiet: f32,
}
impl Watch {
    fn outcome(&mut self, turn: &mut Turn, seen: Vec<Seen>, kind: impl Fn(&str) -> Option<u32>, now: f32) -> Option<Outcome> {
        let mut out = None;
        for s in seen {
            let ok_to_score = self.entered.contains(&true);
            match s {
                Seen::LeftStart if turn.step == Step::Ready && !turn.moving(now) => turn.go(now),
                _ if turn.step != Step::Attempt || out.is_some() => {}
                Seen::Spot { index, inside: true, .. } => self.entered[index] = true,
                // "Left spot volume and not tricking" (513, 1024).
                Seen::Spot { air: false, .. } => out = Some(Outcome::LeftSpot),
                Seen::Wipeout if now >= self.quiet => out = Some(Outcome::Bailed),
                Seen::OffBoard if ok_to_score => out = Some(Outcome::OffBoard),
                Seen::Landed { tricks, turns } if ok_to_score => {
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
    turn: Option<Turn>,
    /// Online: the others' attempts as they arrive (stale ones are dropped, early ones wait).
    inbox: Vec<AttemptResult>,
    /// LocalPlayerID: this player's index in `turn.players`.
    me: usize,
    row: Option<String>,
    in_intro: bool,
    probe: Probe,
    seen: Vec<Seen>,
    watch: Watch,
    /// The attempt the skater was placed for; whether it stands on the spawn.
    placed: u32,
    at_spawn: bool,
    hud: Hud,
    warned: String,
    /// CloseLoadingScreen has run.
    uncovered: bool,
    /// wait_for_leader has been reached: the pause menu and backpack are back (376).
    pause_lifted: bool,
    /// PartyPlay: the attempt whose card was opened.
    carded: u32,
    /// PartyPlay: play the game again (hudretry, restartchallenge).
    restart: bool,
    /// PartyPlay: the player the pad drives (SetPlayer at each attempt's cut).
    controlled: usize,
    /// The player the camera is on: the last attempter RepositionAI moved with its 6th argument true (round_setup
    /// 309-315, prepare_follower 790-795). 0x82d55fa8 switches it as it moves the batch at black, not at the call.
    camera: usize,
}

pub(crate) struct Skate3SkateGamePlugin;
impl Plugin for Skate3SkateGamePlugin {
    fn build(&self, app: &mut App) {
        let root = app.world().get_resource::<Config>().map(|c| c.asset_root.join(DIR)).unwrap_or_default();
        let data = load(&root).inspect_err(|e| warn!("Skate 3 S.K.A.T.E.: no data at {} ({e})", root.display())).ok();
        if let Some(data) = &data {
            // Offered only on a ported map, as the other modes' playable() (skate_dwgh_01 has none).
            let ids: Vec<String> = data.rules.rows.iter().filter(|(id, r)| r.online && map_of(id).is_some()).map(|(id, _)| id.clone()).collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                playable.0.extend(ids.iter().cloned());
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        app.insert_resource(SkateGame { data, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

fn probe(mut game: ResMut<SkateGame>, m: Res<Match>, skater: Res<SkaterRuntime>, time: Res<Time>) {
    let SkateGame { data, turn, probe, seen, .. } = &mut *game;
    let row = data.as_ref().and_then(|d| d.rules.rows.get(&m.row)).filter(|_| m.owned && m.running(Mode::Skate) && turn.is_some());
    let Some((row, start)) = row.and_then(|r| Some((r, r.start_volume.as_ref()?))) else {
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
    probe.step(&sample, start, &row.spot_volumes, seen);
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<SkateGame>| {
        let Some(data) = game.data.take() else { return };
        let data = game.step(world, data);
        game.data = Some(data);
    });
}

impl SkateGame {
    fn step(&mut self, world: &mut World, mut data: Data) -> Data {
        let now = crate::match_core::match_now(world);
        let (mode, phase, id, owned) = {
            let m = world.resource::<Match>();
            (m.mode, m.phase.clone(), m.row.clone(), m.owned)
        };
        let live = owned && mode == Some(Mode::Skate) && data.rules.rows.contains_key(&id) && phase != Phase::Idle;
        let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
        let on_map = live && map.is_some() && map.as_deref() == map_of(&id);
        if !on_map {
            if self.row.is_some() {
                self.end(world);
            }
            if live && self.warned != id {
                warn!("Skate 3 S.K.A.T.E.: {id} is on {:?}, not this map ({map:?})", map_of(&id));
                self.warned = id;
            }
            return data;
        }
        if data.tricks.is_empty() {
            let root = world.resource::<Config>().asset_root.clone();
            let guides = crate::hud_meters::guides(&root).inspect_err(|e| warn!("Skate 3 S.K.A.T.E.: no trick gestures ({e})")).unwrap_or_default();
            data.fill(&world.resource::<SkaterRuntime>().scoring.data.definitions, &guides);
        }
        let row = &data.rules.rows[&id];
        if self.row.is_none() {
            info!("Skate 3 S.K.A.T.E.: {id} ({})", if world.resource::<Match>().token == 0 { "hot-seat" } else { "online" });
            self.row = Some(id.clone());
            // pres_overview: the row's intro flythrough, before setup_skate (35-44).
            if let Some(scene) = row.intro.as_ref().filter(|s| data.intros.contains_key(*s)) {
                world.resource_mut::<Cutscene>().start(&data.root, scene);
                world.resource_mut::<Match>().intro = true;
                self.in_intro = true;
            }
        }
        // The intro lasts as long as its scene plays, on the scene's own clock (a load's stall holds it back, not the
        // wall clock); a scene that didn't load ends it at once.
        let scene_over = world.get_resource::<Cutscene>().is_none_or(|c| c.ended() || !c.active());
        if self.in_intro && scene_over {
            self.in_intro = false;
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
        match phase {
            Phase::Running { .. } if !self.in_intro => {
                if self.turn.is_none() {
                    self.begin(world, &data, row, now);
                }
                self.play(world, &data, row, now);
            }
            Phase::Results { .. } => {
                let mut m = world.resource_mut::<Match>();
                (m.watch, m.gestures) = (None, false);
            }
            // The intro: everyone held (owned rows: the core leaves it to us); pres_overview's own, emotes on (G9).
            _ => {
                let mut m = world.resource_mut::<Match>();
                m.hold = true;
                m.gestures = crate::skate3_modes::gestures(&m, false);
            }
        }
        {
            // make_hud (13) disables the pause menu and backpack until wait_for_leader (376), as the first round_setup's
            // fade ends; setup_skate (141) hides the trick display, complete (1401) shows it again.
            self.pause_lifted |= self.turn.as_ref().is_some_and(|t| !t.moving(now));
            let over = self.turn.as_ref().is_some_and(|t| t.over(now));
            let mut m = world.resource_mut::<Match>();
            m.pause_disabled = !self.pause_lifted;
            m.trick_display_hidden = self.turn.is_some() && !over;
        }
        // The loading screen holds until the first round_setup's teleporttransitionfinished closes it (333-337); an intro
        // shows through it (UNRESOLVED: the requester that clears the cover for a presentation, 0x825edce8 / 0x82561830).
        self.uncovered |= self.in_intro || self.turn.as_ref().is_some_and(|t| t.step != Step::Ready || now >= t.since);
        if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
            hold.0 = !self.uncovered;
        }
        // B4 (online audit 10-06): RepositionAI's fade on screen; without it every attempt's teleport (and Party
        // Play's handover to the next player's look) happened in plain view.
        let alpha = self.turn.as_ref().map_or(0., |t| t.fade_alpha(now));
        if let Some(mut fade) = world.get_resource_mut::<crate::skate3_modes::ChallengeFade>()
            && fade.0 != alpha
        {
            fade.0 = alpha;
        }
        if let Some(turn) = &self.turn {
            self.hud.drive(world, &data, row, turn, self.me, now);
        }
        // PartyPlay: the results dialog's Rematch and Restart Challenge play again (restarted -> setup_skate: a new
        // leader, no intro).
        if std::mem::take(&mut self.restart) {
            info!("Skate 3 S.K.A.T.E.: {id} again");
            close_screens(world);
            self.hud = Hud::default();
            (self.turn, self.placed, self.carded, self.pause_lifted) = (None, 0, 0, false);
            // restarted's SetPlayer(LocalPlayerID) (skate.lua ~1342): the pad back to player 1.
            self.controlled = 0;
        }
        data
    }

    fn begin(&mut self, world: &mut World, data: &Data, row: &Row, now: f32) {
        let m = world.resource::<Match>();
        let online = m.token != 0;
        let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (crate::party_play::names(world), 0) };
        if names.is_empty() {
            names.push("Player 1".into());
        }
        let m = world.resource::<Match>();
        // ponytail: Random.GetRandomNumber's generator is not recovered; every game draws the same leader from the
        // match token (PartyPlay from the clock).
        let seed = if online { m.token } else { u64::from(now.to_bits()) };
        let leader = (seed % names.len() as u64) as usize;
        info!("Skate 3 S.K.A.T.E.: {} players, this one {me}, leader {leader}", names.len());
        let setup = Setup { max_letters: data.rules.max_letters, limit: row.time_limit };
        self.turn = Some(Turn::with_party(names, online, !online, setup, leader, now));
        (self.me, self.camera) = (me, me);
        show_indicators(world, Some(row));
    }

    fn play(&mut self, world: &mut World, data: &Data, row: &Row, now: f32) {
        let Some(turn) = self.turn.as_mut() else { return };
        let me = self.me;
        if turn.online {
            // The core relays attempts (match_core `@ev` records) and blanks the name of a slot whose player left.
            let mut m = world.resource_mut::<Match>();
            // Only the attempter's own console reports its attempt: no letters given to another player.
            let received = m.inbox.drain(..).filter_map(|(from, r)| serde_json::from_str::<AttemptResult>(&r).ok().filter(|a| a.player == from));
            self.inbox.extend(received);
            let left: Vec<usize> = (0..turn.players.len()).filter(|&p| m.players.get(p).is_none_or(String::is_empty)).collect();
            for p in left {
                turn.gone(p, now);
            }
        }
        let attempter = turn.attempter();
        let local = !turn.online || attempter == me;
        let teleporting = world.resource::<Match>().teleporting();
        // A new attempt, at its fade's cut: the attempter to the spawn, the one before it (online) back to its waiting
        // slot.
        if turn.attempt != self.placed && turn.step != Step::Over && now >= turn.cut {
            let first = self.placed == 0;
            self.placed = turn.attempt;
            // ChallengeManager.SetPlayer (PartyPlay): the pad to the attempter, at the cut.
            self.controlled = attempter;
            // I1 (online audit 10-06): RepositionAI's camera switch lands with the batch, at black. Online no
            // RepositionAI is made for a player who left (Online.IsPlayerDisconnected guards it), so no switch either.
            if !(turn.online && turn.players[attempter].gone) {
                self.camera = attempter;
            }
            self.watch = Watch { entered: vec![false; row.spot_volumes.len()], quiet: now + QUIET };
            let to = if local {
                self.at_spawn = true;
                Some(row.spawn.slot(0))
            } else if std::mem::take(&mut self.at_spawn) || first {
                Some(row.wait.slot(me))
            } else {
                None
            };
            if let Some(at) = to {
                world.resource_mut::<Match>().teleport = Some(at);
            }
        }
        let seen = std::mem::take(&mut self.seen);
        if teleporting {
            self.watch.quiet = now + QUIET;
        } else if local
            && let Some(outcome) = self.watch.outcome(turn, seen, |t| data.kind(t), now)
        {
            let r = AttemptResult { player: attempter, attempt: turn.attempt, outcome };
            if turn.result(&r, now) && turn.online {
                let sent = serde_json::to_string(&r).expect("plain data");
                world.resource_mut::<Match>().outbox.push(sent);
            }
        }
        // The others' attempts; one for an attempt this game hasn't reached yet waits.
        for r in std::mem::take(&mut self.inbox) {
            if r.attempt > turn.attempt {
                self.inbox.push(r);
            } else {
                turn.result(&r, now);
            }
        }
        turn.tick(now);
        if turn.party {
            self.party(world, data, row, now);
        }
        let Some(turn) = self.turn.as_ref() else { return };
        let attempter = turn.attempter();
        let local = !turn.online || attempter == me;
        let mut m = world.resource_mut::<Match>();
        m.score = -i64::from(turn.players[me].letters);
        // PartyPlay's game ends at its dialog, not at the core's results.
        m.finished = turn.over(now) && !turn.party;
        // PinAI on everyone but the attempter (179, 211, 313/317, 797); PartyPlay's attempter too until its card's
        // hudcomplete (UnpinAI); the camera on the attempter its RepositionAI last switched to (online: Party Play has
        // one skater and its own camera).
        m.hold = !local || (turn.party && turn.moving(now));
        // G9: a turn wait (another's attempt) passes the emotes; the results screen (once over) owns the pad.
        m.gestures = crate::skate3_modes::gestures(&m, turn.over(now));
        m.watch = (turn.online && self.camera != me).then_some(self.camera);
    }

    /// PartyPlay (party_play::exchange): the card (OpenPressADialog at the fade's end), the pad's player (SetPlayer at
    /// the cut), the others' waiting slots (RepositionAI + PinAI to TurnBasedWaitingLocation, slot = player id), the
    /// results dialog once show_results is over (OpenContinueRetryDialog); back come hudcomplete, hudretry and
    /// restartchallenge.
    fn party(&mut self, world: &mut World, data: &Data, row: &Row, now: f32) {
        let Some(turn) = self.turn.as_mut() else { return };
        let card = turn.card(now).filter(|_| self.carded != turn.attempt);
        if card.is_some() {
            self.carded = turn.attempt;
        }
        let controlled = self.controlled;
        let waiting = (0..turn.players.len()).map(|p| (p != controlled).then(|| row.wait.slot(p))).collect();
        let results = turn.over(now).then(|| results(data, row, turn, self.me, None));
        let answers = crate::party_play::exchange(world, crate::party_play::Turn { controlled, waiting, card, results });
        if answers.hud_complete {
            turn.hudcomplete(now);
        }
        self.restart |= answers.retry || answers.restart;
    }

    fn end(&mut self, world: &mut World) {
        if self.in_intro {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
        if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
            hold.0 = false;
        }
        if let Some(mut fade) = world.get_resource_mut::<crate::skate3_modes::ChallengeFade>() {
            fade.0 = 0.;
        }
        close_screens(world);
        if self.turn.is_some() {
            show_indicators(world, None);
        }
        *self = SkateGame { data: self.data.take(), ..default() };
    }
}

fn close_screens(world: &mut World) {
    if let Some(mut screens) = world.get_resource_mut::<Screens>() {
        let layer = screens.layer(ONLINE);
        for name in [ABD, BOARD, TIMER, LETTERS, CENTRAL, RESULTS] {
            layer.close(name);
        }
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
            // round_setup shows the list (326); prepare_follower hides it (819).
            list_visible: !turn.copying(),
            regular: !goofy,
        }
    }
    /// The answers; trick labels are localized from the movie's own language table.
    fn answers(&self, localize: impl Fn(&str) -> String) -> Vec<(String, Json)> {
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
        for t in self.list.iter().chain([&self.set, &self.copy]) {
            if let Some(id) = t[0].as_str().filter(|id| !id.is_empty()) {
                out.push((format!("FELanguage.GetLocalizedFormatString({id},0)"), json!(localize(id))));
            }
        }
        out
    }
}

/// The online-ui leaderboardhud in its Skate layout (LeaderboardHUDInit's eLeaderboardHUDMenuType.Skate, 203): the
/// setup block, the names, and per player [rank, letters, text, highlight, turn, team, row, sort id].
fn board(data: &Data, row: &Row, turn: &Turn, me: usize, now: f32) -> Vec<(&'static str, Json)> {
    let n = turn.players.len();
    let setup = json!([n, 0, LAYOUT_SKATE, "ID_COMMON_UNKNOWN", false, turn.online, true]);
    let block: Vec<Json> = (0..n)
        .flat_map(|p| {
            // SetRank(id, i) (212); the local player highlighted online, nobody in PartyPlay (217-221).
            let highlight = turn.online && p == me;
            [json!(p + 1), json!(turn.shown_letters(p, now)), json!(""), json!(highlight), json!(turn.marker == Some(p)), json!(0), json!(p), json!(p)]
        })
        .collect();
    let names: Vec<String> = turn.players.iter().map(|p| format!("#{}", p.name)).collect();
    vec![
        ("HUDComponents.Leaderboard_GetSetupDataBlock", setup),
        ("HUDComponents.Leaderboard_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Leaderboard_GetLeaderboardDataBlock", Json::Array(block)),
        ("Mission.GetTitle", json!(data.text(&row.title))),
        // UpdateVOIPIcons' GetVoipStatus()[p] (RefreshAll calls it): voice::voip_status by the player's name, online.
        ("FELeaderboard.GetVoipStatus", json!(turn.players.iter().map(|p| if turn.online { crate::voice::voip_named(&p.name) } else { 0 }).collect::<Vec<_>>())),
    ]
}

/// endchallenge_online for show_results' highlightreelcomplete (1284-1320): Results_SetPlayerData(id, rank, letters)
/// per player, eResultsLBType.PlaceNameInt, columns Rank / Gamertag (Name offline) / Score, the Skate throwdown type
/// (its score column draws the letters).
fn results(data: &Data, row: &Row, turn: &Turn, me: usize, status: Option<i64>) -> Vec<(String, Json)> {
    let mut order: Vec<usize> = (0..turn.players.len()).collect();
    order.sort_by_key(|&p| (turn.players[p].rank == 0, turn.players[p].rank, p));
    // Per row: [rank, int, float, string, time, choice, row, name]. The int cell reaches the Skate column as a frame
    // label through `substring(1)`; ponytail: the native's "#<n>" text form is read from that call (not traced).
    let block: Vec<Json> = (order.iter().enumerate())
        .flat_map(|(i, &p)| {
            let s = &turn.players[p];
            [json!(s.rank), json!(format!("#{}", s.letters)), json!(0), json!(""), json!(""), json!(false), json!(i), json!(format!("#{}", s.name))]
        })
        .collect();
    let position = order.iter().position(|&p| p == me).map_or(0, |i| i + 1);
    let skater = if turn.online { "ID_LEADERBOARD_HEADER_SKATER" } else { "ID_LEADERBOARD_NAME_HEADER" };
    let n = turn.players.len();
    let (status_text, status_timer) = crate::skate3_modes::results_status(&data.text(RETURN_TO_LOBBY), status);
    vec![
        // ponytail: Mission.GetResultsTitle's native is not traced; the row's title.
        ("Mission.GetResultsTitle".into(), json!(data.text(&row.title))),
        ("Mission.GetResultsStatusText".into(), status_text),
        ("Mission.GetResultsStatusTimer".into(), status_timer),
        ("Mission.GetCurrentPlayerPosition".into(), json!(position)),
        ("HUDComponents.Results_GetNumPlayers".into(), json!(n)),
        ("HUDComponents.Results_GetLayoutType".into(), json!(0)),
        ("HUDComponents.Results_GetLeaderboardDataBlock".into(), Json::Array(block)),
        ("HUDComponents.Results_GetLeaderboardSubHeaderDataBlock".into(), json!([false, 0, "", ""])),
        ("HUDComponents.Results_GetLeaderboardColTitle(0)".into(), json!("ID_LEADERBOARD_HEADER_RANK")),
        ("HUDComponents.Results_GetLeaderboardColTitle(1)".into(), json!(skater)),
        ("HUDComponents.Results_GetLeaderboardColTitle(2)".into(), json!("ID_COMMON_SCORE")),
        ("HUDComponents.Results_GetThrowdownType".into(), json!(2)),
        ("FELeaderboard.EndChallengeOnline_GetLeaderboardNavData".into(), json!([n, position.saturating_sub(1), position])),
        // As skate3_modes::results_answers: Steam has no gamercard / profile to open, so no platform matches and no A
        // button (user 10-26: unanswered, both consoles' A rows drew over the status line).
        ("global.GetPlatform".into(), json!(-1)),
    ]
}

fn set_state(screen: &mut Screen, state: i64) -> Result<(), String> {
    screen.call("SetState", &[json!(state)]).map(drop)
}

/// The TimerHUD from one view to the next (skate2_screens' timer_acts: the same hudtimer code).
pub(crate) fn timer_states(screen: &mut Screen, before: Option<f64>, now: Option<f64>) -> Result<(), String> {
    let Some(left) = now else { return set_state(screen, 2) };
    if before.is_none() {
        set_state(screen, 1)?;
    }
    let prev = before.unwrap_or(0.);
    let changed = prev.ceil() != left.ceil();
    if left < TIMER_WARNING && changed {
        screen.bindings.sounds.push("timer_end".into());
        set_state(screen, 4)?;
    } else if prev < TIMER_WARNING && left > TIMER_WARNING {
        set_state(screen, 5)?;
    }
    if changed {
        let text = crate::skate2_screens::timer_text(left);
        screen.answer("HUDComponents.Timer_GetData", json!([text, u8::from(left >= TIMER_WARNING)]));
        set_state(screen, 3)?;
    }
    Ok(())
}

#[derive(Default)]
struct Hud {
    /// Screens that failed stay closed for the match.
    failed: BTreeSet<&'static str>,
    abd: Option<Abd>,
    board: Option<Vec<(&'static str, Json)>>,
    stinger: Option<(u32, u8)>,
    timer: Option<f64>,
    /// The central message shown (key) and when it hides.
    central: Option<(String, Option<f32>)>,
    /// The modified message shown (key) and when it hides (None: stays).
    modified: Option<(String, Option<f32>)>,
    /// The attempt in Ready / Attempt: leaving them clears the modified message (stateexit 607-608, 1060-1061).
    attempting: Option<u32>,
    results: bool,
    /// The results screen's status timer ([`crate::skate3_modes::online_results`]).
    status: Option<i64>,
}
/// endchallenge_online's online status line.
const RETURN_TO_LOBBY: &str = crate::skate3_modes::RETURN_TO_LOBBY;
impl Hud {
    fn drive(&mut self, world: &mut World, data: &Data, row: &Row, turn: &Turn, me: usize, now: f32) {
        // No screen layers (a headless World: the tests): no movie to drive.
        if !world.contains_resource::<Screens>() {
            return;
        }
        let goofy = world.resource::<SkaterRuntime>().animation.goofy();
        let status = crate::skate3_modes::online_results(world, self.results);
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
        let layer = screens.layer(ONLINE);
        let dir = data.root.join("../hud/runtime");
        let movie = |name: &str| dir.join(format!("{name}.json"));

        let abd = Abd::new(data, turn, goofy);
        if self.abd.as_ref() != Some(&abd) {
            let last = self.abd.replace(abd.clone());
            self.with(layer, &movie("hudabd"), ABD, |screen, fresh| {
                let localize = |id: &str| screen.bindings.movie.text_assets.localize(id);
                let answers = abd.answers(localize);
                for (k, v) in answers {
                    screen.answer(&k, v);
                }
                // SetState 1: show + UpdateAll; 4 UpdateList, 5 UpdateSetTrick, 6 UpdateCopyTrick, 7 / 8 the list shown /
                // hidden (hudabd's SetState).
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

        let answers = board(data, row, turn, me, now);
        if self.board.as_ref() != Some(&answers) {
            let fresh_setup = self.board.as_ref().is_none_or(|b| b[1] != answers[1]);
            self.board = Some(answers.clone());
            // The online-ui layer's own leaderboardhud (DIR is private/skate3-online/skate).
            let path = data.root.join("../../online-ui/runtime/leaderboardhud.json");
            self.with(layer, &path, BOARD, |screen, fresh| {
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

        let stinger = turn.stinger(now);
        if stinger != self.stinger {
            self.stinger = stinger;
            layer.close(LETTERS);
            if let Some((_, letters)) = stinger {
                // SetState(n) plays the n-th letter in; the game adds the S.K.A.T.E. letter sting (sub_826692F8), the
                // full-word one on the last letter
                let all = letters >= turn.setup.max_letters;
                self.with(layer, &movie("hudthrowmes"), LETTERS, |screen, _| {
                    screen.bindings.sounds.push(if all { "skate_all_letters" } else { "skate_letter" }.into());
                    set_state(screen, i64::from(letters))
                });
            }
        }

        // HUD2.StartTimer at wait_for_scoring / wait_for_copy (447, 1071), hidden at their exit (607, 1060).
        let timer = (turn.step == Step::Attempt).then(|| f64::from((turn.ends - now).max(0.)));
        if timer != self.timer {
            let before = std::mem::replace(&mut self.timer, timer);
            if timer.is_some() || layer.open.contains_key(TIMER) {
                self.with(layer, &movie("hudtimer"), TIMER, |screen, _| timer_states(screen, before, timer));
            }
        }

        let attempting = matches!(turn.step, Step::Ready | Step::Attempt).then_some(turn.attempt);
        let cleared = self.attempting.is_some() && self.attempting != attempting;
        self.attempting = attempting;
        let wanted = (central(turn, data, me, now), modified(turn, data, me, now));
        let (shown_central, shown_modified) = (self.central.clone(), self.modified.clone());
        let mut central_shown = shown_central.clone();
        let mut modified_shown = shown_modified.clone();
        self.with(layer, &movie("hudcentral"), CENTRAL, |screen, fresh| {
            if fresh {
                set_state(screen, 1)?;
            }
            // ClearModifiedCentralMessage: SetState 88, its outro (0x828508e0).
            if cleared {
                set_state(screen, 88)?;
                modified_shown = None;
            }
            if let Some(c) = wanted.0.filter(|c| central_shown.as_ref().is_none_or(|s| s.0 != c.key)) {
                // ShowCentralMessage (0x82663f38): [text, colour White 0], the audio hook, SetState 4.
                screen.answer("HUDComponents.Central_GetMessageData", json!([c.text, 0]));
                screen.answer("HUDComponents.Central_GetAudioHook", json!(c.hook));
                set_state(screen, 4)?;
                central_shown = Some((c.key, Some(now + CENTRAL_FOR)));
            } else if let Some((key, Some(_))) = central_shown.clone().filter(|s| s.1.is_some_and(|at| now >= at)) {
                set_state(screen, 5)?;
                central_shown = Some((key, None));
            }
            if let Some(m) = wanted.1.filter(|m| modified_shown.as_ref().is_none_or(|s| s.0 != m.key)) {
                // ShowModifiedCentralMessage (0x826640f0): SetState 10; hidden (11) after its time unless it stays.
                screen.answer("HUDComponents.Central_GetModifiedMessageData", json!([m.text, 0]));
                set_state(screen, 10)?;
                modified_shown = Some((m.key, (!m.stay).then_some(now + CENTRAL_FOR)));
            } else if let Some((key, Some(_))) = modified_shown.clone().filter(|s| s.1.is_some_and(|at| now >= at)) {
                set_state(screen, 11)?;
                modified_shown = Some((key, None));
            }
            Ok(())
        });
        (self.central, self.modified) = (central_shown, modified_shown);

        if self.results && status != self.status {
            let before = std::mem::replace(&mut self.status, status);
            if let Some(screen) = layer.open.get_mut(RESULTS)
                && let Err(e) = crate::skate3_modes::status_states(screen, &data.text(RETURN_TO_LOBBY), before, status)
            {
                warn!("Skate 3 S.K.A.T.E.: endchallenge_online: {e}");
            }
        }
        // PartyPlay's results are its dialog (SkateGame::party).
        if turn.over(now) && !self.results && !turn.party {
            self.results = true;
            self.status = status;
            let answers = results(data, row, turn, me, status);
            let refs: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            if !self.failed.contains(RESULTS) {
                match Screen::load(&movie("endchallenge_online"), &refs) {
                    Ok(screen) => drop(layer.open.insert(RESULTS.to_owned(), screen)),
                    Err(e) => {
                        warn!("Skate 3 S.K.A.T.E.: endchallenge_online: {e}");
                        self.failed.insert(RESULTS);
                    }
                }
            }
        }
    }

    /// Runs `f` on screen `name`, first loading `path` when it isn't open (`fresh`). A screen that fails is closed for
    /// the rest of the match.
    fn with(&mut self, layer: &mut Layer, path: &Path, name: &'static str, f: impl FnOnce(&mut Screen, bool) -> Result<(), String>) {
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
            warn!("Skate 3 S.K.A.T.E.: {}: {e}", path.display());
            layer.close(name);
            self.failed.insert(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: f32 = 15.;
    fn setup() -> Setup {
        Setup { max_letters: 5, limit: LIMIT }
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
    /// The attempter's outcome now, then time on to the next attempt (or the end).
    fn play(t: &mut Turn, outcome: Outcome, now: &mut f32) -> Option<Fail> {
        // Past the RepositionAI fade.
        *now = now.max(t.since);
        t.tick(*now);
        let r = AttemptResult { player: t.attempter(), attempt: t.attempt, outcome };
        assert!(t.result(&r, *now), "{r:?} at {:?}", t.step);
        let fail = match t.step {
            Step::Result(fail) => fail,
            _ => Some(Fail::TimeUp),
        };
        *now += 10.;
        t.tick(*now);
        fail
    }
    fn blank() -> Data {
        let rules = Rules { template: HashMap::new(), max_letters: 5, rows: BTreeMap::new() };
        Data { root: PathBuf::new(), rules, strings: HashMap::new(), intros: HashMap::new(), tricks: HashMap::new() }
    }

    /// round_setup (299-307): FollowerOrder wraps from the drawn leader; leader_failure passes the lead to
    /// FollowerOrder[1] (688); online, a set everyone copied passes it too, with no letter (process_round 1162,
    /// leader_get_letter 1441-1446); offline it stays (1166-1169).
    #[test]
    fn the_lead_passes_as_skate_lua_passes_it() {
        let mut now = 0.;
        let mut t = Turn::new(names(4), false, setup(), 2, now);
        assert_eq!((t.leader, t.followers.as_slice(), t.marker), (2, &[3, 0, 1][..], None));
        assert!(t.moving(1.9) && t.since == LEAD_FADE + FADE_HOLD, "wait_for_leader after the 1 s fade and its hold");
        t.tick(t.since);
        assert_eq!(t.marker, Some(2), "wait_for_leader's LeaderboardSetTurn (393)");
        assert_eq!(play(&mut t, Outcome::Bailed, &mut now), Some(Fail::Bailed), "a wipeout fails at once (452)");
        assert_eq!((t.leader, t.followers.as_slice(), t.round), (3, &[0, 1, 2][..], 2));
        // Offline, everyone copying keeps the leader.
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        for _ in 0..3 {
            assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        }
        assert_eq!((t.leader, t.round, t.players.iter().map(|p| p.letters).sum::<u8>()), (3, 3, 0));
        // Online, it passes to FollowerOrder[1] and nobody gets a letter.
        let mut now = 0.;
        let mut t = Turn::new(names(3), true, setup(), 1, now);
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        assert_eq!((t.leader, t.followers.as_slice(), t.players.iter().map(|p| p.letters).sum::<u8>()), (2, &[0, 1][..], 0));
        // The repeated set (544) and the trick type (527) still fail the new leader.
        t.done.push(trick("heelflip", 0));
        assert_eq!(t.judge(&landed("heelflip", 0)), Some(Fail::Repeated));
        assert_eq!(t.judge(&Outcome::Landed { trick: trick("50_50", 0), kind: 4, spots: true }), Some(Fail::Invalid));
        assert_eq!(t.judge(&Outcome::Landed { trick: trick("kickflip", 0), kind: FLIP, spots: false }), Some(Fail::LeftSpot));
        assert_eq!(t.judge(&Outcome::OffBoard), Some(Fail::Invalid), "skateroffboard: invalidtrick (620)");
    }

    /// PartyPlay: every attempt waits on its Press-A card once the fade is over (OpenPressADialog at
    /// teleporttransitionfinished); nothing runs until hudcomplete, then the wait begins from that moment.
    #[test]
    fn party_attempts_wait_on_their_card() {
        let mut t = Turn::with_party(names(2), false, true, setup(), 1, 0.);
        let due = t.since;
        assert_eq!(t.card(due - 0.1), None, "not before the fade ends");
        assert_eq!(t.card(due), Some((2, 1)), "the leader, round 1");
        t.tick(due + 30.);
        assert!(t.moving(due + 30.) && t.step == Step::Ready, "the attempt waits on A however long");
        t.hudcomplete(due + 30.);
        assert_eq!((t.card(due + 30.), t.since), (None, due + 30.));
        assert!(!t.moving(due + 30.));
        let mut now = due + 30.;
        play(&mut t, landed("kickflip", 0), &mut now);
        assert_eq!(t.card(now.max(t.since)), Some((1, 1)), "the follower's card, once its fade is over");
        assert_eq!(Turn::new(names(2), false, setup(), 1, 0.).card(due), None, "not outside Party Play");
    }

    /// Matrix Step 2 (10-12): Party Play at its retail max (4, party_play page 0). By hand (skate.lua; offline the lead
    /// stays, 1166-1169; a skater out takes NumSkaters then counts down, 506): player 1 sets a new trick every round;
    /// player 2 always bails, player 3 copies round 1, player 4 rounds 1-2: out in rounds 5, 6 and 7, ranked 4, 3, 2.
    /// Every attempt waits on its player's card: 4 a round for 5 rounds, then 3, then 2.
    #[test]
    fn party_play_holds_four_players() {
        let mut t = Turn::with_party(names(4), false, true, setup(), 0, 0.);
        let (mut now, mut cards) = (0f32, Vec::new());
        for _ in 0..500 {
            if t.step == Step::Over {
                break;
            }
            now = now.max(t.since);
            t.tick(now);
            if let Some(card) = t.card(now) {
                assert_eq!(card.0, t.attempter() + 1, "the card is the attempter's");
                cards.push(card);
                t.hudcomplete(now);
                continue;
            }
            let (p, round) = (t.attempter(), i64::from(t.round));
            let outcome = if !t.copying() {
                landed(&format!("set{round}"), 0)
            } else if (p == 2 && round <= 1) || (p == 3 && round <= 2) {
                Outcome::Landed { trick: t.set.clone().unwrap(), kind: FLIP, spots: true }
            } else {
                Outcome::Bailed
            };
            play(&mut t, outcome, &mut now);
        }
        assert_eq!(t.step, Step::Over);
        assert_eq!(t.players.iter().map(|p| (p.rank, p.letters)).collect::<Vec<_>>(), [(1, 0), (4, 5), (3, 5), (2, 5)]);
        let per_round: Vec<usize> = (1..=7).map(|r| cards.iter().filter(|c| c.1 == r).count()).collect();
        assert_eq!(per_round, [4, 4, 4, 4, 4, 3, 2], "{cards:?}");
    }

    /// B4 (online audit 10-06): each attempt's RepositionAI fade is on screen, black at the cut (the teleport and, in
    /// Party Play, the next player's look) and through the hold: round_setup's 1 s fade, prepare_follower's 1.5 s.
    #[test]
    fn the_screen_is_black_when_the_attempter_moves() {
        let t = Turn::new(names(2), false, setup(), 1, 0.);
        assert_eq!(t.cut, LEAD_FADE / 2.);
        for (now, alpha) in [(0., 0.), (0.25, 0.5), (t.cut, 1.), (t.cut + FADE_HOLD, 1.), (1.75, 0.5), (2., 0.), (9., 0.)] {
            assert!((t.fade_alpha(now) - alpha).abs() < 1e-5, "{now}: {} not {alpha}", t.fade_alpha(now));
        }
        let mut t = Turn::new(names(2), false, setup(), 1, 0.);
        let mut now = 0.;
        play(&mut t, landed("kickflip", 0), &mut now);
        assert!(t.copying() && t.fade_alpha(t.cut) > 0.9999, "the follower's fade covers its move");
        assert!((t.fade_alpha(t.cut - FOLLOW_FADE / 4.) - 0.5).abs() < 1e-4);
    }

    /// skate_indu_03 ("Can You Spill SKATE?"): its one VisualIndicator is kind 1, the trick area's outline on the
    /// ground. While the row is on it goes to skate2_arrows as a closed ribbon in the S.K.A.T.E. style, with no arrow;
    /// the row's end takes it away.
    #[test]
    fn a_rows_ground_outline_is_drawn_while_it_is_on() {
        let square = vec![[-34.57, 34.18, -507.32], [-24.6, 34.18, -507.32], [-24.6, 34.18, -517.78], [-34.57, 34.18, -517.78]];
        let row = Row {
            title: "T".into(),
            online: true,
            time_limit: LIMIT,
            intro: None,
            spawn: Locator { matrix: [[0.; 4]; 4], slots: vec![] },
            wait: Locator { matrix: [[0.; 4]; 4], slots: vec![] },
            start_volume: None,
            spot_volumes: vec![],
            visual_indicators: vec![],
            outlines: vec![Outline { points: square.clone(), closed: true }],
        };
        let mut world = World::new();
        world.init_resource::<crate::skate2_arrows::Arrows>();
        show_indicators(&mut world, Some(&row));
        let arrows = world.resource::<crate::skate2_arrows::Arrows>();
        assert!(arrows.wanted.is_empty(), "no arrow");
        let ribbon = crate::skate2_arrows::Ribbon {
            glow: format!("{OUTLINE_DIR}/visual_indicator_blue.png"),
            darken: format!("{OUTLINE_DIR}/visual_indicator_background.png"),
            points: square,
            closed: true,
        };
        assert_eq!(arrows.ribbons, [ribbon], "the blue look");
        assert!(arrows.more.iter().any(|m| m == INDICATORS), "its style's file is loaded");
        show_indicators(&mut world, None);
        assert!(world.resource::<crate::skate2_arrows::Arrows>().ribbons.is_empty(), "gone with the row");
    }

    // ---- The glue (`run` -> SkateGame::step) in a World with no engine and no screens: what it writes each frame.
    const ROW: &str = "skate_univ_01";
    const DT: f32 = 1. / 60.;
    /// An online match on ROW (University) of `players`, this console in slot `me`; the token draws leader 0.
    fn online_world(players: usize, me: usize) -> World {
        let place = |x: f32| -> Matrix {
            let mut m = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];
            m[3][0] = x;
            m
        };
        let row = Row {
            title: "T".into(),
            online: true,
            time_limit: LIMIT,
            intro: None,
            spawn: Locator { matrix: place(0.), slots: vec![] },
            wait: Locator { matrix: place(10.), slots: vec![] },
            start_volume: Some(Volume { min: [-1.; 3], max: [1.; 3] }),
            spot_volumes: vec![],
            visual_indicators: vec![],
            outlines: vec![],
        };
        let mut data = blank();
        data.rules.rows.insert(ROW.into(), row);
        // Filled, as from the engine's scoring definitions.
        data.tricks.insert("kickflip".into(), ("ID_TRICK_FLIP_KICKFLIP".into(), "none".into(), FLIP));
        let mut world = World::new();
        world.insert_resource(SkateGame { data: Some(data), ..default() });
        world.insert_resource(Match {
            mode: Some(Mode::Skate),
            row: ROW.into(),
            phase: Phase::Running { started: 0., ends: None },
            owned: true,
            token: players as u64,
            slot: me,
            players: names(players),
            ..default()
        });
        let path = Some(PathBuf::from("maps/University.skate"));
        world.insert_resource(CurrentMap { path, name: "University".into(), spawn: [0.; 3], heading: 0., generation: 0 });
        world.insert_resource(Time::<Real>::default());
        world.insert_resource(crate::skate3_modes::ChallengeFade::default());
        world
    }
    /// One Update frame `DT` on (the glue's `run`); the clock after it.
    fn frame(world: &mut World) -> f32 {
        world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(DT));
        run(world);
        world.resource::<Time<Real>>().elapsed_secs()
    }
    fn turn_of(world: &World) -> Turn {
        world.resource::<SkateGame>().turn.clone().expect("the game began")
    }
    fn watch(world: &World) -> Option<usize> {
        world.resource::<Match>().watch
    }
    /// The attempter's console relays its attempt (match_core's `@ev`); this one receives it.
    fn relay(world: &mut World, outcome: Outcome) {
        let t = turn_of(world);
        let r = AttemptResult { player: t.attempter(), attempt: t.attempt, outcome };
        world.resource_mut::<Match>().inbox.push((r.player, serde_json::to_string(&r).unwrap()));
    }

    /// v12 #10: Campus S.K.A.T.E.'s intro was never seen. Its 5 s ran on the wall clock from the match start, inside the
    /// load's stall, while the scene's own clock (0.1 s a frame at most) had barely left its black opening. The intro
    /// lasts until the scene has played. SKATE3_ASSET_ROOT = the install's assets (skipped without).
    #[test]
    fn the_intro_lasts_until_its_scene_has_played_not_five_wall_seconds() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(PathBuf::from) else { return };
        if !assets.join(DIR).join("tutorial.json").is_file() {
            eprintln!("skate3-online stage not run on {}", assets.display());
            return;
        }
        let mut world = online_world(2, 0);
        world.init_resource::<Cutscene>();
        {
            let mut game = world.resource_mut::<SkateGame>();
            let data = game.data.as_mut().unwrap();
            data.root = assets.join(DIR);
            data.intros.insert("skate_univ_02".into(), 5.);
            data.rules.rows.get_mut(ROW).unwrap().intro = Some("skate_univ_02".into());
        }
        frame(&mut world);
        assert!(world.resource::<Match>().intro, "the intro starts");
        world.resource_mut::<Cutscene>().run_for_test(0.5);
        // The load's stall: six wall seconds go by in one frame.
        world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs(6));
        frame(&mut world);
        assert!(world.resource::<Match>().intro && world.resource::<SkateGame>().turn.is_none(), "still the intro");
        world.resource_mut::<Cutscene>().run_for_test(5.);
        frame(&mut world);
        assert!(!world.resource::<Match>().intro, "over once the scene has played");
        assert!(world.resource::<SkateGame>().turn.is_some(), "the game begins");
    }

    /// I1 (online audit 10-06): online, every console's camera goes to the next attempter when RepositionAI moves the
    /// batch at black (round_setup / prepare_follower's 6th argument; 0x82d54c90 keeps the player, 0x82d55fa8 hands
    /// it over with the move), not at the call: the attempter before stays watched through the fade out, on the next
    /// attempter's own console too, until its own move.
    #[test]
    fn the_camera_switches_to_the_next_attempter_at_black() {
        // This console is slot 2: leader 0, then followers 1 and 2.
        let mut world = online_world(3, 2);
        let mut now = frame(&mut world);
        let t = turn_of(&world);
        assert_eq!((t.leader, t.followers.as_slice()), (0, &[1, 2][..]));
        while now < t.cut {
            assert_eq!(watch(&world), None, "{now}: round_setup's fade out: the camera stays ours");
            now = frame(&mut world);
        }
        assert_eq!(watch(&world), Some(0), "at black: the leader");
        // The leader sets; leader_success's 2 s, then prepare_follower's 1.5 s fade sends follower 1 on.
        while now < t.since {
            now = frame(&mut world);
        }
        relay(&mut world, landed("kickflip", 0));
        for (next, before) in [(1, Some(0)), (2, Some(1))] {
            while !(turn_of(&world).copying() && turn_of(&world).attempter() == next) {
                now = frame(&mut world);
            }
            let t = turn_of(&world);
            while now < t.cut {
                assert_eq!(watch(&world), before, "{now}: follower {next}'s fade out (cut {})", t.cut);
                now = frame(&mut world);
            }
            assert_eq!(watch(&world), (next != 2).then_some(next), "follower {next} at black");
            if next == 1 {
                while now < t.since {
                    now = frame(&mut world);
                }
                relay(&mut world, landed("kickflip", 0));
            }
        }
    }

    /// G9 (online audit 10-28; bug 10-17 #44 / #45): online, the console waiting for another's attempt is held (PinAI)
    /// with its emotes on; the attempter's own console plays (no hold, no emotes); the Results take them away.
    #[test]
    fn a_waiting_skater_can_emote() {
        let mut world = online_world(2, 1);
        frame(&mut world);
        let m = world.resource::<Match>();
        assert!(m.hold && m.gestures, "leader 0 attempts: this console waits, held, its emotes on");
        let mut world = online_world(2, 0);
        frame(&mut world);
        let m = world.resource::<Match>();
        assert!(!m.hold && !m.gestures, "the attempter plays");
        let mut world = online_world(2, 1);
        frame(&mut world);
        world.resource_mut::<Match>().phase = Phase::Results { until: 99. };
        frame(&mut world);
        assert!(!world.resource::<Match>().gestures, "the Results own the pad");
    }

    /// A console relaying another player's attempt (a spoofed bail for the leader) is ignored; the attempter's own counts.
    #[test]
    fn a_spoofed_attempt_is_ignored() {
        let mut world = online_world(3, 2);
        let mut now = frame(&mut world);
        let t = turn_of(&world);
        while now < t.since {
            now = frame(&mut world);
        }
        let r = AttemptResult { player: t.attempter(), attempt: t.attempt, outcome: Outcome::Bailed };
        world.resource_mut::<Match>().inbox.push((1, serde_json::to_string(&r).unwrap()));
        frame(&mut world);
        assert_eq!(turn_of(&world).cached, None, "slot 1 cannot report the leader's attempt");
        relay(&mut world, Outcome::Bailed);
        frame(&mut world);
        assert_eq!(turn_of(&world).cached, Some(0));
    }

    /// M6 (online audit 10-06): the glue writes RepositionAI's fade to ChallengeFade every frame (B4: `step` never
    /// wrote it), black on the frame it moves the skater, clear once the fade is over.
    #[test]
    fn the_glue_draws_the_fade_it_moves_behind() {
        let mut world = online_world(2, 1);
        let mut now = frame(&mut world);
        let t = turn_of(&world);
        let mut moved = None;
        while now < t.fade_until + 0.1 {
            let fade = world.resource::<crate::skate3_modes::ChallengeFade>().0;
            assert!((fade - t.fade_alpha(now)).abs() < 1e-6, "{now}: drawn {fade}, the fade is {}", t.fade_alpha(now));
            if moved.is_none() && world.resource::<Match>().teleport.is_some() {
                moved = Some(fade);
            }
            now = frame(&mut world);
        }
        assert_eq!(moved, Some(1.), "the first round_setup's move (to the waiting slot) is made in black");
        assert_eq!(world.resource::<crate::skate3_modes::ChallengeFade>().0, 0.);
    }

    /// The last letter places a follower (943); prepare_follower finds the winner (755-768) and process_round ends it at
    /// once (1176-1183); show_results' 3 s timer (1236) ends the game.
    #[test]
    fn the_last_one_standing_wins_at_once() {
        let mut now = 0.;
        let mut t = Turn::new(names(2), false, setup(), 0, now);
        for round in 0..5 {
            assert_eq!(play(&mut t, landed("kickflip", 180 * round), &mut now), None);
            assert_eq!(t.marker, Some(0));
            assert_eq!(t.since - now, FOLLOW_FADE + FADE_HOLD, "the follower's 1.5 s fade (790)");
            now = t.since + SETTLE;
            t.tick(now);
            assert_eq!(t.marker, Some(1), "wait_for_follower's LeaderboardSetTurn after 0.25 s (869)");
            let r = AttemptResult { player: 1, attempt: t.attempt, outcome: Outcome::OffBoard };
            assert!(t.result(&r, now));
            assert_eq!(t.step, Step::Result(Some(Fail::NotCopied)), "skateroffboard: notcopied (1155)");
            assert_eq!(t.stinger(now + 3.), Some((t.attempt, round as u8 + 1)), "ShowSkateMessage after 3 s (967)");
            assert_eq!(t.shown_letters(1, now + 5.4), round as u8, "the leaderboard's letter at 5.5 s (975)");
            now += 7.5;
            t.tick(now);
        }
        assert_eq!((t.step, t.players[0].rank, t.players[1].rank), (Step::Over, 1, 2));
        assert!(!t.over(now + 2.9) && t.over(now + 3.));
    }

    /// The global playerdisconnected (1418-1429) places a leaver last; its own attempt starts at once (432-436) on a
    /// 1 s timer (443, 1069); prepare_follower skips it (795); a leader who fails with nobody left wins (697-705).
    #[test]
    fn a_player_who_leaves_is_last_and_skipped() {
        let mut now = 0.;
        let mut t = Turn::new(names(3), true, setup(), 0, now);
        assert_eq!(play(&mut t, landed("kickflip", 0), &mut now), None);
        assert_eq!(t.attempter(), 1);
        t.gone(1, now);
        assert_eq!((t.players[1].rank, t.next_rank, t.step), (3, 2, Step::Ready), "prepare_follower has no playerdisconnected");
        // wait_for_follower's timers run for it as for anyone; its attempt timer is 1 s.
        now = t.since + SETTLE;
        t.tick(now);
        now += PRE_ATTEMPT;
        t.tick(now);
        assert_eq!((t.step, t.ends), (Step::Attempt, now + GONE_LIMIT));
        t.tick(now + 1.);
        assert_eq!(t.step, Step::Result(Some(Fail::TimeUp)));
        assert_eq!(t.players[1].rank, 3, "a leaver's letter does not place it again (942)");
        now += 10.;
        t.tick(now);
        assert_eq!(t.attempter(), 2);
        now = t.since + SETTLE;
        t.tick(now);
        t.gone(2, now);
        assert_eq!(t.players[2].rank, 3, "rank = NumSkaters, not CurrentRank (1423)");
        assert_eq!((t.step, t.ends), (Step::Attempt, now + GONE_LIMIT), "wait_for_follower's playerdisconnected (882)");
        t.tick(now + 1.);
        now += 10.;
        t.tick(now);
        // prepare_follower: nobody else is left, so the leader has won (755-768, 1176-1183).
        assert_eq!((t.step, t.players[0].rank), (Step::Over, 1));
        // A leader who fails online with every follower gone wins at once (690-706).
        let mut t = Turn::new(names(2), true, setup(), 0, 0.);
        t.gone(1, 0.);
        assert_eq!((t.attempter(), t.step), (0, Step::Ready));
        assert!(!t.result(&AttemptResult { player: 0, attempt: t.attempt, outcome: Outcome::Bailed }, 1.), "behind the fade");
        assert!(t.result(&AttemptResult { player: 0, attempt: t.attempt, outcome: Outcome::Bailed }, t.since));
        assert_eq!((t.step, t.players[0].rank, t.players[1].rank), (Step::Over, 1, 2));
    }

    /// Matrix G2 (10-12): `n` consoles play one game through the match relay ([`crate::skate3_modes::tests::Wire`]),
    /// each judging its own attempts and applying the others' as `play` does; `leaver` (player, time) quits there (its
    /// results still on the way are lost) and the others notice it 0.5 s + 0.4 s per slot later. Every console still
    /// here: its (rank, letters) per player.
    fn skate_over_the_wire(n: usize, seed: u64, latency: (f64, f64), loss: f64, leaver: Option<(usize, f32)>) -> Vec<Vec<(u8, u8)>> {
        let mut wire = crate::skate3_modes::tests::Wire::new(seed, latency, loss);
        let mut turns: Vec<Turn> = (0..n).map(|_| Turn::new(names(n), true, setup(), seed as usize % n, 0.)).collect();
        let mut inboxes: Vec<Vec<AttemptResult>> = vec![Vec::new(); n];
        // An attempt's outcome (0-5 lands / copies, 6-7 bails, 8 steps off, 9 runs out of time) and when it comes.
        let pick = |attempt: u32, p: usize| {
            let h = (seed ^ u64::from(attempt) * 0x9E37_79B9 ^ p as u64 * 31).wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33;
            (h % 10, 0.5 + (h / 10 % 40) as f32 / 10.)
        };
        let gone = |i: usize, t: f32| leaver.is_some_and(|(p, at)| p == i && t >= at);
        let dt = 1. / 60.;
        for frame in 0..60 * 6000 {
            let t = frame as f32 * dt;
            if let Some((p, at)) = leaver
                && (at..at + dt).contains(&t)
            {
                (0..n).for_each(|to| wire.cut(p, to));
            }
            for i in (0..n).filter(|&i| !gone(i, t)) {
                let turn = &mut turns[i];
                if let Some((p, at)) = leaver
                    && t >= at + 0.5 + 0.4 * i as f32
                {
                    turn.gone(p, t);
                }
                inboxes[i].extend(wire.take(f64::from(t), i).into_iter().map(|(_, text)| serde_json::from_str::<AttemptResult>(&text).unwrap()));
                if turn.step == Step::Attempt && turn.attempter() == i {
                    let (kind, after) = pick(turn.attempt, i);
                    let outcome = match kind {
                        _ if t >= turn.ends => Some(Outcome::TimeUp),
                        9 => None,
                        _ if t < turn.since + after => None,
                        0..=5 if turn.copying() => Some(Outcome::Landed { trick: turn.set.clone().unwrap(), kind: FLIP, spots: true }),
                        0..=5 => Some(landed(TRICKS[turn.attempt as usize % 2].0, 180 * (turn.attempt as i32 % 3))),
                        6 | 7 => Some(Outcome::Bailed),
                        _ => Some(Outcome::OffBoard),
                    };
                    if let Some(outcome) = outcome {
                        let r = AttemptResult { player: i, attempt: turn.attempt, outcome };
                        if turn.result(&r, t) {
                            wire.send(f64::from(t), i, n, vec![serde_json::to_string(&r).unwrap()]);
                        }
                    }
                }
                for r in std::mem::take(&mut inboxes[i]) {
                    if r.attempt > turn.attempt {
                        inboxes[i].push(r);
                    } else {
                        turn.result(&r, t);
                    }
                }
                turn.tick(t);
            }
            if (0..n).filter(|&i| !gone(i, t)).all(|i| turns[i].step == Step::Over) {
                break;
            }
        }
        (0..n)
            .filter(|&i| leaver.is_none_or(|(p, _)| p != i))
            .map(|i| {
                let t = &turns[i];
                assert_eq!(t.step, Step::Over, "seed {seed}: console {i} finished the game (round {}, attempt {}, attempter {}, letters {:?})", t.round, t.attempt, t.attempter(), t.players.iter().map(|p| p.letters).collect::<Vec<_>>());
                turns[i].players.iter().map(|p| (p.rank, p.letters)).collect()
            })
            .collect()
    }

    /// Matrix G2 (10-12): six players (retail's CHALLENGE_MAX) with 150-300 ms and 10 % loss: every console ends the
    /// game and ranks it the same, one winner under MaxLetters and every rank once; a guest, or the host (slot 0),
    /// leaving mid-game is ranked last everywhere and the game still ends the same on every console.
    #[test]
    fn every_console_ends_the_game_the_same() {
        for seed in 1..=12 {
            let all = skate_over_the_wire(6, seed, (0.15, 0.3), 0.1, None);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            let mut ranks: Vec<u8> = all[0].iter().map(|p| p.0).collect();
            ranks.sort_unstable();
            assert_eq!(ranks, [1, 2, 3, 4, 5, 6], "seed {seed}: {:?}", all[0]);
            assert!(all[0].iter().all(|&(rank, letters)| (rank == 1) == (letters < 5)), "seed {seed}: {:?}", all[0]);
        }
        for seed in 1..=12u64 {
            let p = if seed % 2 == 1 { 0 } else { seed as usize % 5 + 1 };
            let at = 20. + (seed * 13 % 60) as f32;
            let all = skate_over_the_wire(6, seed, (0.15, 0.3), 0.05, Some((p, at)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}, player {p} left at {at}");
            }
            assert_eq!(all[0][p].0, 6, "seed {seed}: the leaver is last");
        }
    }

    /// The HUD2 messages with their names (388-391, 864-867) and the results rows sorted by rank.
    #[test]
    fn messages_name_the_players() {
        let mut data = blank();
        data.rules.template.insert("CentralMessageHALIDNewLead".into(), "ID_SKATE_NEWLEADER".into());
        data.rules.template.insert("CentralMessageHALIDNewLeadYou".into(), "ID_SKATE_NEWLEADERYOU".into());
        data.strings.insert("ID_SKATE_NEWLEADER".into(), "{0:s} is the leader".into());
        data.strings.insert("ID_SKATE_NEWLEADERYOU".into(), "{0:s}, you are the leader".into());
        data.strings.insert("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN".into(), "It's {0:s}'s turn.".into());
        let mut now = 0.;
        let mut t = Turn::new(names(2), true, setup(), 1, now);
        assert_eq!(modified(&t, &data, 0, 0.), None, "behind the fade");
        let at = t.since;
        assert_eq!(central(&t, &data, 0, at), None);
        assert_eq!(modified(&t, &data, 0, at).map(|m| (m.text, m.stay)), Some(("P1 is the leader".into(), true)));
        assert_eq!(central(&t, &data, 1, at).unwrap().text, "P1, you are the leader");
        play(&mut t, landed("kickflip", 0), &mut now);
        let at = t.since;
        assert_eq!(modified(&t, &data, 1, at), None, "the follower's turn waits 0.25 s (841)");
        t.tick(at + SETTLE);
        assert_eq!(modified(&t, &data, 1, at + SETTLE).map(|m| m.text), Some("It's P0's turn.".into()));
        // results: rank order, the local player's position.
        t.players[0].rank = 1;
        t.players[1].rank = 2;
        let row = Row {
            title: "T".into(),
            online: true,
            time_limit: LIMIT,
            intro: None,
            spawn: Locator { matrix: [[0.; 4]; 4], slots: vec![] },
            wait: Locator { matrix: [[0.; 4]; 4], slots: vec![] },
            start_volume: None,
            spot_volumes: vec![],
            visual_indicators: vec![],
            outlines: vec![],
        };
        let r: HashMap<String, Json> = results(&data, &row, &t, 1, None).into_iter().collect();
        assert_eq!(r["HUDComponents.Results_GetLeaderboardDataBlock"][7], json!("#P0"));
        assert_eq!(r["Mission.GetCurrentPlayerPosition"], json!(2));
        assert_eq!(map_of("skate_indu_02"), Some("Industrial"));
        // Lead 10-06: the DLC row plays on Art_Gallery (it refused with "not this map"); skate_dwgh_01 has no ported map.
        assert_eq!(map_of("skate_artg_01"), Some("Art_Gallery"));
        assert_eq!(map_of("skate_dwgh_01"), None);
        // online.json's maps for the other S3 districts (their rows were refused by map_of alone).
        for (id, map) in [("street_mmcp_01", "MaloofMoneyCup"), ("1up_bbpk_01", "BlackBoxPark"), ("otl_bbx_01", "BlackBoxPark"), ("vert_skp5_01", "University")] {
            assert_eq!(map_of(id), Some(map), "{id}");
        }
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
        for _ in 0..5 {
            tick(9., false, false, false, &[], 1);
        }
        tick(9., false, true, false, &[], 0);
        tick(9., false, true, true, &[], 0);
        assert_eq!(seen, [
            Seen::LeftStart,
            Seen::Spot { index: 0, inside: true, air: true },
            Seen::Spot { index: 0, inside: false, air: true },
            Seen::Landed { tricks: vec!["ollie", "kickflip"], turns: 1 },
            Seen::Wipeout,
            Seen::OffBoard,
        ]);
        let kind = |id: &str| Some(if id == "ollie" { AIR } else { FLIP });
        let mut t = Turn::new(names(2), false, setup(), 0, 0.);
        let o = t.since;
        let mut w = Watch { entered: vec![false], quiet: 0. };
        assert_eq!(w.outcome(&mut t, seen[..1].to_vec(), kind, 1.), None);
        assert_eq!(t.step, Step::Ready, "behind the fade the start volume is not watched");
        assert_eq!(w.outcome(&mut t, seen[..1].to_vec(), kind, o), None);
        assert_eq!(t.step, Step::Attempt, "leaving the start volume starts it (424-428)");
        assert_eq!(w.outcome(&mut t, seen[1..].to_vec(), kind, o), Some(landed("kickflip", 180)));
        // A wipeout fails without a spot; stepping off needs one (621, 1151); a teleport's wipeout is ignored (453).
        let mut w = Watch { entered: vec![false], quiet: 0. };
        assert_eq!(w.outcome(&mut t, vec![Seen::OffBoard], kind, o + 1.), None);
        assert_eq!(w.outcome(&mut t, vec![Seen::Wipeout], kind, o + 1.), Some(Outcome::Bailed));
        let mut w = Watch { entered: vec![false], quiet: 5. };
        assert_eq!(w.outcome(&mut t, vec![Seen::Wipeout], kind, o + 1.), None);
        let rolled = vec![Seen::Spot { index: 0, inside: true, air: false }, Seen::Spot { index: 0, inside: false, air: false }];
        assert_eq!(w.outcome(&mut t, rolled, kind, o + 2.), Some(Outcome::LeftSpot));
        assert_eq!(w.outcome(&mut t, vec![], kind, o + LIMIT), Some(Outcome::TimeUp));
    }

    // ------------------------------------------------------------ the Lua oracle
    /// One attempt's plan: a landing (trick index, degrees, every spot entered), a copy of the set trick (the leader: a
    /// kickflip), a wipeout, the timer, rolling out of a spot, stepping off the board.
    #[derive(Clone, Debug)]
    enum Plan {
        Land(usize, i32, bool),
        Copy,
        Bail,
        Time,
        Left,
        Off,
    }
    /// The oracle's tricks and their Trick.eTrickType (Air 1, Flip 2, a grind 4).
    const TRICKS: [(&str, u32); 4] = [("kickflip", FLIP), ("heelflip", FLIP), ("ollie", AIR), ("50_50", 4)];

    fn plans(seed: u64) -> Vec<Plan> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        (0..3000)
            .map(|_| match next(20) {
                0..=6 => Plan::Copy,
                7..=11 => Plan::Land(next(4) as usize, 180 * next(3) as i32, next(10) != 0),
                12 | 13 => Plan::Bail,
                14 | 15 => Plan::Time,
                16 | 17 => Plan::Left,
                _ => Plan::Off,
            })
            .collect()
    }

    /// What the model shows, in the oracle's words.
    #[derive(Default)]
    struct Shown {
        central: Option<String>,
        modified: Option<String>,
        marker: Option<usize>,
        stinger: Option<(u32, u8)>,
        started: u32,
    }
    impl Shown {
        fn sample(&mut self, t: &Turn, data: &Data, me: usize, now: f32, log: &mut Vec<String>) {
            if t.step == Step::Attempt && self.started != t.attempt {
                self.started = t.attempt;
                log.push(format!("{now:.2} attempt {}", t.attempter()));
            }
            if let Some(c) = central(t, data, me, now).filter(|c| self.central.as_ref() != Some(&c.key)) {
                log.push(format!("{now:.2} central {} {}", c.text, c.hook));
                self.central = Some(c.key);
            }
            if let Some(m) = modified(t, data, me, now).filter(|m| self.modified.as_ref() != Some(&m.key)) {
                log.push(format!("{now:.2} modified {} {}", m.text, m.stay));
                self.modified = Some(m.key);
            }
            if let Some(p) = t.marker.filter(|_| t.marker != self.marker) {
                self.marker = t.marker;
                log.push(format!("{now:.2} turn {p}"));
            }
            let s = t.stinger(now);
            if s != self.stinger {
                self.stinger = s;
                if let Some((_, letters)) = s {
                    log.push(format!("{now:.2} letter {letters}"));
                }
            }
        }
    }

    /// The model's log, sampled every 0.25 s (every skate.lua timer is a multiple of it).
    fn model_log(online: bool, n: usize, leader: usize, me: usize, plan: &[Plan]) -> Vec<String> {
        let data = blank();
        let mut t = Turn::with_party(names(n), online, !online, setup(), leader, 0.);
        let (mut log, mut shown, mut plan) = (Vec::new(), Shown::default(), plan.iter());
        let mut timing_out = false;
        let mut now = 0f32;
        for _ in 0..1_000_000 {
            // PartyPlay: the card as the fade ends, answered at once (the oracle's hudcomplete).
            if let Some((p, r)) = t.card(now) {
                log.push(format!("{now:.2} pressa {p} {r}"));
                t.hudcomplete(now);
                t.tick(now);
            }
            shown.sample(&t, &data, me, now, &mut log);
            if t.step == Step::Attempt && !timing_out {
                let outcome = match plan.next().expect("plan ran out") {
                    Plan::Copy if !t.copying() => Some(landed("kickflip", 0)),
                    Plan::Copy => Some(Outcome::Landed { trick: t.set.clone().unwrap(), kind: FLIP, spots: true }),
                    &Plan::Land(i, spin, spots) => Some(Outcome::Landed { trick: trick(TRICKS[i].0, spin), kind: TRICKS[i].1, spots }),
                    Plan::Bail => Some(Outcome::Bailed),
                    Plan::Time => None,
                    Plan::Left => Some(Outcome::LeftSpot),
                    Plan::Off => Some(Outcome::OffBoard),
                };
                timing_out = outcome.is_none();
                if let Some(outcome) = outcome {
                    assert!(t.result(&AttemptResult { player: t.attempter(), attempt: t.attempt, outcome }, now));
                    shown.sample(&t, &data, me, now, &mut log);
                }
            }
            if timing_out && now >= t.ends {
                timing_out = false;
                assert!(t.result(&AttemptResult { player: t.attempter(), attempt: t.attempt, outcome: Outcome::TimeUp }, now));
                shown.sample(&t, &data, me, now, &mut log);
            }
            if t.over(now) {
                for (p, s) in t.players.iter().enumerate() {
                    log.push(format!("{now:.2} result {p} {} {}", s.rank, s.letters));
                }
                return log;
            }
            now += 0.25;
            t.tick(now);
        }
        panic!("the model never ended: {:?}", log.last());
    }

    /// skate.lua run as the stategraph runs it: handlers by `-- [state] message`, ChangeState queued until the handler
    /// returns (stateexit, then stateenter), countdown timers, RepositionAI's fade (batched, then
    /// teleporttransitionfinished) and PartyPlay's Press-A dialog answered at once. The natives the rules read are stubbed the way the model reads them
    /// (AddTrickToAlreadyBeenDoneList / MatchedPattern: trick and spin; nobody disconnects); every other native is a
    /// no-op. HUD2's messages, LeaderboardSetTurn, ShowSkateMessage, the attempts and Results_SetPlayerData are logged.
    const DRIVER: &str = r#"
table.getn = function(t) return #t end
print = function() end
local meta
meta = {__index = function(t, k) local v = setmetatable({}, meta); rawset(t, k, v); return v end, __call = function() return nil end}
local function ns(t) return setmetatable(t, meta) end
local LOG, NOW, timers, next_timer = {}, 0, {}, 0
local function log(s) LOG[#LOG + 1] = string.format("%.2f %s", NOW, s) end
local done, set, tried, spin, last_turn = {}, nil, nil, 0, nil
local fade_end, press_a, reel = nil, false, false
Challenge = ns{GameMode = {SinglePlayer = 0, PartyPlay = 1, OnlineChallenge = 2}, GetGameMode = function() return MODE end,
  GetTeleportFadeInProgress = function() return fade_end ~= nil end, GetPlayerName = function(id) return "P" .. id end}
Online = ns{GetLocalPlayerID = function() return ME end, GetNumOnlineSkaters = function() return N end,
  GetSkaterName = function(i) return "P" .. i end, IsPlayerDisconnected = function() return false end}
ChallengeManager = ns{GetNumHumanSkaters = function() return N end}
Random = ns{GetRandomNumber = function() return LEADER + 1 end}
Entity = ns{DoesSkaterExist = function() return true end}
GameMode = ns{SkateSpeedRounds = function() return false end}
ChallengeTimer = ns{
  AddCountdownTimer = function(s) next_timer = next_timer + 1; timers[next_timer] = NOW + s; return next_timer end,
  ResetCountdownTimer = function(id, s) timers[id] = NOW + s end,
  RemoveCountdownTimer = function(id) timers[id] = nil end}
-- 0x82d54c90: the fade argument defaults to 1; requests batch into a running fade; teleporttransitionfinished comes
-- after the fade and the hold.
AI = ns{RepositionAI = function(_, _, _, _, fade)
  fade = fade or 1
  if fade_end == nil and fade > 0.001 then fade_end = NOW + fade + HOLD end
end}
FrontEnd = ns{OpenPressADialog = function(p, r) press_a = true; log(string.format("pressa %d %d", p, r)) end,
  Results_SetPlayerData = function(id, rank, letters) log(string.format("result %d %d %d", id, rank, letters)) end}
HighlightReel = ns{PlayHighlightReelWithSpecifiedOrder = function() reel = true end}
HUD2 = ns{eMessageAudioHook = {Success = 2},
  ShowCentralMessage = function(text, _, _, _, _, _, hook) log(string.format("central %s %d", text, hook or 0)) end,
  ShowModifiedCentralMessage = function(text, _, _, _, _, _, stay) log("modified " .. text .. " " .. tostring(stay == true)) end,
  LeaderboardSetTurn = function(_, id) if id ~= last_turn then last_turn = id; log("turn " .. id) end end,
  ShowSkateMessage = function(_, letters) log("letter " .. letters) end}
Trick = ns{eTrickType = {Air = 1, Flip = 2},
  GetTrickTypeFromScorableID = function(i) return KINDS[i] end,
  GetTrickIndexFromScorableID = function(i) return i end,
  GetCompletedAir180Count = function() return spin // 180 end,
  IsInAirWithValidLaunchTrick = function() return false end}
Skate = ns{
  ResetSkateTrackers = function() done = {} end,
  AddTrickToAlreadyBeenDoneList = function(i)
    local e = TRICKS[i] .. ":" .. spin
    for _, d in ipairs(done) do if d == e then return false end end
    done[#done + 1] = e
    while #done > MAX do table.remove(done, 1) end
    set = e
    return true
  end,
  UpdateAttemptItem = function(i) tried = TRICKS[i] .. ":" .. spin end,
  MatchedPattern = function() return tried == set end,
  ResetSetItem = function() set = nil end,
  ResetAttemptItem = function() tried = nil end}
setmetatable(_G, {__index = function(t, k) local v = setmetatable({}, meta); rawset(t, k, v); return v end})

local H, queue, state, due = {}, {}, nil, nil
local VARS = {}
local SELF = setmetatable({Name = "skate", ABDListItemLimit = MAX, TimeLimit = LIMIT, TurnBasedStartVolume = 50,
  SpotVolumes = {Count = 2, 100, 101}, VisualIndicators = {Count = 0}, IntroPresentationEvents = {Count = 0}},
  {__index = function(_, k) return k end})
function ChangeState(s) queue[#queue + 1] = s end
local function send(msg, m)
  local f = H[state] and H[state][msg]
  if f then f(SELF, m or {}, VARS, {}, {}) end
end
local function settle()
  while #queue > 0 do
    local s = table.remove(queue, 1)
    if state then send("stateexit") end
    state = s
    send("stateenter")
    if s == "wait_for_scoring" or s == "wait_for_copy" then
      due = s == "wait_for_scoring" and VARS.LeaderID or VARS.FollowerOrder[VARS.FollowerIndex]
      log("attempt " .. due)
    end
  end
end
local step = 0
local function deliver(p)
  step = step + 1
  local o = assert(PLAN[step], "plan ran out")
  if o.k == "copy" then
    if state == "wait_for_scoring" then
      o = {k = "land", trick = 1, spin = 0, spots = true}
    else
      local name, deg = set:match("^(.+):(%d+)$")
      for i, t in ipairs(TRICKS) do if t == name then o = {k = "land", trick = i, spin = tonumber(deg), spots = true} end end
    end
  end
  if o.k == "time" then return end
  if o.k == "bail" then return send("wipeout", {PlayerID = p, IsAPhysicsTeleport = false}) end
  send("triggerentercollision", {PlayerID = p, VolumeID = 100})
  if o.k == "left" then
    send("triggerexitcollision", {PlayerID = p, VolumeID = 100})
  elseif o.k == "off" then
    send("skateroffboard", {PlayerID = p})
  else
    if o.spots then send("triggerentercollision", {PlayerID = p, VolumeID = 101}) end
    spin = o.spin
    send("aircompleted", {PlayerID = p, FirstScorableId = o.trick, Wipedout = 0})
  end
end
function run(src)
  for st, msg, fn in src:gmatch("%-%- %[([%w_]+)%] ([%w_]+)%s+function (h_%w+)") do
    H[st] = H[st] or {}
    H[st][msg] = rawget(_G, fn)
  end
  assert(H.wait_for_copy and H.wait_for_copy.aircompleted, "no handlers parsed")
  ChangeState("setup_skate")
  settle()
  for _ = 1, 1000000 do
    if due then
      local p = due
      due = nil
      deliver(p)
    elseif press_a then
      press_a = false
      send("hudcomplete")
    elseif reel then
      reel = false
      send("highlightreelcomplete")
    else
      local id, at = nil, math.huge
      for i, t in pairs(timers) do if t < at or (t == at and i < id) then id, at = i, t end end
      if fade_end and fade_end <= at then
        NOW, fade_end = math.max(NOW, fade_end), nil
        send("teleporttransitionfinished")
      else
        if not id then break end
        timers[id] = nil
        NOW = math.max(NOW, at)
        send("timerexpired", {TimerID = id})
      end
    end
    settle()
    if state == "complete" then break end
  end
  return LOG
end
"#;

    fn lua_log(src: &str, online: bool, n: usize, leader: usize, me: usize, plan: &[Plan]) -> Vec<String> {
        let lua = mlua::Lua::new();
        let g = lua.globals();
        g.set("MODE", if online { 2 } else { 1 }).unwrap();
        g.set("N", n).unwrap();
        g.set("LEADER", leader).unwrap();
        g.set("ME", me).unwrap();
        g.set("MAX", setup().max_letters).unwrap();
        g.set("LIMIT", LIMIT).unwrap();
        g.set("HOLD", FADE_HOLD).unwrap();
        g.set("TRICKS", lua.create_sequence_from(TRICKS.map(|t| t.0)).unwrap()).unwrap();
        g.set("KINDS", lua.create_sequence_from(TRICKS.map(|t| t.1)).unwrap()).unwrap();
        let steps = lua.create_table().unwrap();
        for (i, p) in plan.iter().enumerate() {
            let o = lua.create_table().unwrap();
            let k = match p {
                &Plan::Land(trick, spin, spots) => {
                    o.set("trick", trick + 1).unwrap();
                    o.set("spin", spin).unwrap();
                    o.set("spots", spots).unwrap();
                    "land"
                }
                Plan::Copy => "copy",
                Plan::Bail => "bail",
                Plan::Time => "time",
                Plan::Left => "left",
                Plan::Off => "off",
            };
            o.set("k", k).unwrap();
            steps.set(i + 1, o).unwrap();
        }
        g.set("PLAN", steps).unwrap();
        lua.load(DRIVER).set_name("@driver").exec().unwrap();
        lua.load(src).set_name("@skate.lua").exec().unwrap();
        let run: mlua::Function = g.get("run").unwrap();
        run.call(src).unwrap()
    }

    /// skate.lua itself (S3_SKATE_DIR = tools/skate3_online.py's `--out`/skate, with scripts/) decides every game the
    /// model plays the same way: who attempts when, every central / modified message with its stay flag and audio
    /// hook, the leaderboard's turn marker, the S-K-A-T-E stinger and the final ranks and letters;
    /// online 2 to 6 players, PartyPlay 2 to 4, varied leaders and local players.
    #[test]
    #[ignore = "requires the decompiled skate.lua (S3_SKATE_DIR)"]
    fn skate3_skate_matches_skate_lua() {
        let root = PathBuf::from(std::env::var("S3_SKATE_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/skate.lua")).unwrap();
        let (mut games, mut lines) = (0, 0);
        for seed in 1..=40u64 {
            for online in [false, true] {
                // Matrix G1 (10-12): online to retail's 6 (CHALLENGE_MAX), Party Play to its 4.
                for n in 2..=if online { 6 } else { 4 } {
                    let (leader, me) = (seed as usize % n, (seed as usize / n) % n);
                    let plan = plans(seed * 31 + n as u64);
                    let lua = lua_log(&src, online, n, leader, me, &plan);
                    let model = model_log(online, n, leader, me, &plan);
                    if let Some(i) = (0..lua.len().max(model.len())).find(|&i| lua.get(i) != model.get(i)) {
                        let from = i.saturating_sub(6);
                        panic!(
                            "seed {seed} online {online} n {n} leader {leader} me {me}: line {i}\nlua:   {:#?}\nmodel: {:#?}",
                            &lua[from..(i + 3).min(lua.len())],
                            &model[from..(i + 3).min(model.len())]
                        );
                    }
                    assert!(lua.last().is_some_and(|l| l.contains("result")), "{:?}", lua.last());
                    (games, lines) = (games + 1, lines + lua.len());
                }
            }
        }
        eprintln!("skate.lua oracle: {games} games, {lines} log lines identical");
        assert_eq!(games, 320);
    }

    fn texts(screen: &Screen) -> Vec<String> {
        (screen.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// The tool's output (S3_SKATE_DIR; its sibling hud/) and the install's online-ui (S3_ONLINE_UI = <assets>/private/
    /// online-ui): the online rows load with their spawns inside their start volumes and their intros, and every HUD
    /// movie runs on the answers: the leader message, the set trick, the leaderboard's names, the stinger, the timer,
    /// the results table.
    #[test]
    #[ignore = "requires the Skate 3 S.K.A.T.E. data (S3_SKATE_DIR, S3_ONLINE_UI)"]
    fn skate3_skate_data_and_movies() {
        let root = PathBuf::from(std::env::var("S3_SKATE_DIR").unwrap());
        let ui = PathBuf::from(std::env::var("S3_ONLINE_UI").unwrap());
        let mut data = load(&root).unwrap();
        let online: Vec<_> = data.rules.rows.iter().filter(|(_, r)| r.online).map(|(id, _)| id.clone()).collect();
        // 10-06 data: skate_artg_01 (Art_Gallery) joined the 9 disc rows; skate_dwgh_01 (no ported map, its locators
        // null) is left out at load instead of failing the whole file.
        assert_eq!(online.len(), 10, "{online:?}");
        assert!(!data.rules.rows.contains_key("skate_dwgh_01") && data.rules.rows.contains_key("skate_artg_01"));
        let mapped: Vec<_> = online.iter().filter(|id| map_of(id).is_some()).collect();
        assert_eq!(mapped.len(), 10, "{online:?}");
        assert_eq!(data.rules.max_letters, 5);
        for id in mapped {
            let row = &data.rules.rows[id];
            let at = row.spawn.slot(0)[3];
            assert!(row.start_volume.as_ref().unwrap().contains(Vec3::new(at[0], at[1], at[2])), "{id}: spawn outside its start volume");
            assert!(!row.spot_volumes.is_empty() && row.time_limit == 15. && row.wait.slots.len() == 6, "{id}");
            assert!(data.intros.contains_key(row.intro.as_deref().unwrap()), "{id}: no intro");
            assert!(map_of(id).is_some() && !data.text(&row.title).starts_with("ID_"), "{id}");
        }
        data.tricks.insert("kickflip".into(), ("ID_TRICK_FLIP_KICKFLIP".into(), "none".into(), FLIP));
        let row = &data.rules.rows["skate_dwtn_02"];
        let dir = root.join("../hud/runtime");
        let mut t = Turn::new(names(2), true, setup(), 0, 0.);
        let at = t.since;
        let mut screen = Screen::load(&dir.join("hudcentral.json"), &[]).unwrap();
        set_state(&mut screen, 1).unwrap();
        let c = central(&t, &data, 0, at).unwrap();
        assert_eq!(c.text, "P0, you are the leader");
        screen.answer("HUDComponents.Central_GetMessageData", json!([c.text, 0]));
        screen.answer("HUDComponents.Central_GetAudioHook", json!(c.hook));
        set_state(&mut screen, 4).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        assert!(texts(&screen).iter().any(|s| s == "P0, you are the leader"), "{:?}", texts(&screen));

        t.set = Some(trick("kickflip", 180));
        t.done.push(trick("kickflip", 180));
        let mut screen = Screen::load(&dir.join("hudabd.json"), &[]).unwrap();
        let answers = Abd::new(&data, &t, false).answers(|id| screen.bindings.movie.text_assets.localize(id));
        for (k, v) in answers {
            screen.answer(&k, v);
        }
        set_state(&mut screen, 1).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        assert!(texts(&screen).iter().any(|s| s.contains("Kickflip")), "{:?}", texts(&screen));

        let mut screen = Screen::load(&ui.join("runtime/leaderboardhud.json"), &[]).unwrap();
        for (k, v) in board(&data, row, &t, 0, 0.) {
            screen.answer(k, v);
        }
        set_state(&mut screen, 3).unwrap();
        set_state(&mut screen, 1).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        let shown = texts(&screen);
        assert!(shown.iter().any(|s| s == "P0") && shown.iter().any(|s| s == "P1"), "{shown:?}");

        let mut screen = Screen::load(&dir.join("hudthrowmes.json"), &[]).unwrap();
        set_state(&mut screen, 2).unwrap();
        for _ in 0..120 {
            screen.advance().unwrap();
        }
        assert!(!screen.bindings.sounds.is_empty(), "the stinger plays its sound");

        let mut screen = Screen::load(&dir.join("hudtimer.json"), &[]).unwrap();
        timer_states(&mut screen, None, Some(15.)).unwrap();
        timer_states(&mut screen, Some(15.), Some(9.5)).unwrap();
        for _ in 0..10 {
            screen.advance().unwrap();
        }
        assert!(texts(&screen).iter().any(|s| s.contains("10")), "{:?}", texts(&screen));

        t.players[1].rank = 1;
        t.players[0].rank = 2;
        let answers = results(&data, row, &t, 0, None);
        let refs: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&dir.join("endchallenge_online.json"), &refs).unwrap();
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        let shown = texts(&screen);
        assert!(shown.iter().any(|s| s == "P1") && shown.iter().any(|s| s == "Park 'N SKATE"), "{shown:?}");
        // Online Results: the status line (SetState 6), then its seconds going down (SetState 100).
        let back = data.text(RETURN_TO_LOBBY);
        assert_ne!(back, RETURN_TO_LOBBY, "the status string is installed");
        crate::skate3_modes::status_states(&mut screen, &back, None, Some(8)).unwrap();
        crate::skate3_modes::status_states(&mut screen, &back, Some(8), Some(7)).unwrap();
        for _ in 0..10 {
            screen.advance().unwrap();
        }
        let shown = texts(&screen);
        // GetResultsStatusTimer's "#:%02d" (s3re 10-28): 7 s left reads 0:07.
        assert!(shown.iter().any(|s| *s == back) && shown.iter().any(|s| s.contains("0:07")), "{shown:?}");
    }

    /// User 10-26 (v10): after an online S.K.A.T.E. game "the buttons were missing" (10-25 #4: no glyphs) and "some of the
    /// words were overlapping": S.K.A.T.E. left global.GetPlatform unanswered, so SetEndOptions added both consoles' A View
    /// gamer card / A View Profile over the status line. As every other S3 mode (skate3_modes::results_answers): B Quit
    /// only. SKATE3_ASSET_ROOT = the install's assets (skipped without).
    #[test]
    fn the_skate_results_screen_shows_only_b_quit() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(PathBuf::from) else { return };
        if !assets.join(DIR).join("rules.json").is_file() {
            eprintln!("skate3-online stage not run on {}", assets.display());
            return;
        }
        let data = load(&assets.join(DIR)).unwrap();
        let row = &data.rules.rows["skate_dwtn_02"];
        let mut t = Turn::new(names(2), true, setup(), 0, 0.);
        (t.players[0].rank, t.players[1].rank) = (1, 2);
        let answers = results(&data, row, &t, 1, Some(8));
        let refs: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&data.root.join("../hud/runtime/endchallenge_online.json"), &refs).unwrap();
        crate::skate3_modes::status_states(&mut screen, &data.text(RETURN_TO_LOBBY), None, Some(8)).unwrap();
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        let drawn: Vec<String> = screen.draws().unwrap().into_iter().map(|d| d.texture).collect();
        assert!(drawn.iter().any(|t| t.ends_with("buttons/B.rgba")), "B Quit: {drawn:?}");
        assert!(!drawn.iter().any(|t| t.ends_with("buttons/A.rgba")), "a console's A button: {drawn:?}");
    }

    /// User 10-26 (v10): after an online S.K.A.T.E. game the results screen showed no buttons and some words overlapped.
    /// Snapshots (SKATE_ONLINE_HUD_SHOTS) of endchallenge_online on S.K.A.T.E.'s own answers, 2 and 6 players with long
    /// Steam names: as it opens, then with the online Results' status line and B Quit (SetState 6) counting down.
    #[test]
    #[ignore = "snapshots: SKATE3_ASSET_ROOT, SKATE_ONLINE_HUD_SHOTS"]
    fn skate_results_screen_snapshots() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let out = PathBuf::from(std::env::var("SKATE_ONLINE_HUD_SHOTS").unwrap());
        std::fs::create_dir_all(&out).unwrap();
        let data = load(&assets.join(DIR)).unwrap();
        let row = &data.rules.rows["skate_dwtn_02"];
        let file = data.root.join("../hud/runtime/endchallenge_online.json");
        let back = data.text(RETURN_TO_LOBBY);
        let names = ["xX_ShredLord_420_Xx", "SkaterBoi99", "the_longest_steam_name_ok", "Dee", "Eve", "Fay"];
        let snap = |screen: &Screen, name: String| {
            crate::core_menu_runtime::tests::raster_draws(&screen.draws().unwrap(), &assets.join("private/online-ui"), &out.join(name));
        };
        for n in [2, 6] {
            let mut t = Turn::new(names[..n].iter().map(|s| s.to_string()).collect(), true, setup(), 0, 0.);
            for (p, s) in t.players.iter_mut().enumerate() {
                (s.rank, s.letters) = (p as u8 + 1, if p == 0 { 2 } else { 5 });
            }
            let answers = results(&data, row, &t, 1, Some(8));
            let refs: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let mut screen = Screen::load(&file, &refs).unwrap();
            for _ in 0..90 {
                screen.advance().unwrap();
            }
            snap(&screen, format!("skate-results-{n}-open.png"));
            crate::skate3_modes::status_states(&mut screen, &back, None, Some(8)).unwrap();
            for _ in 0..60 {
                screen.advance().unwrap();
            }
            crate::skate3_modes::status_states(&mut screen, &back, Some(8), Some(7)).unwrap();
            for _ in 0..30 {
                screen.advance().unwrap();
            }
            snap(&screen, format!("skate-results-{n}-online.png"));
            eprintln!("{n} players: {:?}", texts(&screen));
        }
    }
}
