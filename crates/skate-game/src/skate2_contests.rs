//! Skate 2's Spot Battle, Best Trick, Jam and 1-Up on New_San_Vanelona, on the match core (`match_core.rs`). Data:
//! skate2port/old_contests.py, installed at `private/skate2-online/contests/` (contests.json: the vault rows with
//! their locators and volumes resolved; indicator/indicators.json); the intro cameras are
//! `private/skate2-online/intros/` (tutorial.json + scene/camera).
//!
//! The rules are the disc's ChallengeCore Lua (f_1a_*_ots_* spot battles, c_*_besttrick / bowl_series / jam
//! contests, ol_1up_*), kept in one serializable [`Contest`] with pure rule functions ([`Contest::apply`],
//! [`Contest::tick`]):
//! - Spot Battle: turns in roster order for NumRounds rounds. A sequence that touched a spot volume scores
//!   (cumulative rows add up a turn's sequences); NumSequencesToEndTurn sequences, the TimeLimit or a bail end the
//!   turn. The best score ranks; "Spot Owned!" when the attempter takes rank 1. Online with 3+ players the others
//!   vote (hudonlinevoting; VotingManager: the mean of the voters' 0-3); the best mean over 1 is the Player's Choice.
//! - Best Trick / Jam: everyone skates each round for its RoundTime. The round's best sequence (cumulative: the sum
//!   of those over CumulativeMinScore) ranks, and the round awards N-1, N-2, ... contest points (ties share). The
//!   round's variant gates the tricks (grab, grind, air only) or scores spin / grind length instead.
//! - 1-Up: the leader sets a score on an unused object; each follower tries to beat it there (and takes the lead if
//!   so); the round's leader wins it; NumToWin rounds win.
//!
//! Solo runs the contest here for one player. Online the lead's sync runs it on the host (`net = Host`: the guests'
//! events arrive in `inbox`) and copies it to the guests (`net = Guest`: their own events leave by `outbox`). Its
//! timers count seconds left, so a copy needs no clock offset.
//!
//! HUD: hudcountdown / hudtimer / hudcentral through [`TutorialScreens`], and Skate 2's online leaderboardhud and
//! hudonlinevoting (layer [`SKATE2_ONLINE`]).
use crate::{
    apt_screen::{SKATE2_ONLINE, Screens},
    challenge::cutscene::Cutscene,
    config::Config,
    match_core::{Match, Phase, Playable},
    physics::SkaterRuntime,
    skate2_arrows::Arrows,
    skate2_screens::{Central, TutorialScreens, View},
    skate3_modes::ChallengeFade,
    skate3_skate_game::FADE_HOLD,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::HashMap,
    mem::{Discriminant, discriminant, take},
    path::{Path, PathBuf},
    sync::Arc,
};

const ROOT: &str = "private/skate2-online/contests";
const INTROS: &str = "private/skate2-online/intros";
/// Every row's map.
const MAP: &str = "New_San_Vanelona";
// The templates' ChallengeTimer lengths (seconds).
const COUNTDOWN: f32 = 3.;
/// Spot Battle end_turn: 2 s, 0.01 s after a wipeout.
const END_TURN: f32 = 2.;
const END_TURN_WIPED: f32 = 0.01;
/// Skate 3's rule (user 10-28): Spot Battle's EndRunTimerID, the turn running on after its time is up (S3
/// spot_battle.lua 901-912).
const END_RUN: f32 = 2.;
/// AI.RepositionAI's fade (S2 0x826dc118 -> 0x82b79d18; every Spot Battle / 1-Up / contest send-back passes 1 or the
/// default 1): out over half, the batch moved at black (the camera switched to the player on a 6th argument of true),
/// held FADE_HOLD, in over the other half, then teleporttransitionfinished.
pub(crate) const REPOSITION_FADE: f32 = 1.;
pub(crate) const MOVE: f32 = REPOSITION_FADE + FADE_HOLD;
/// Contest round results (results_screen's 6 s, + 1 s online).
const ROUND_RESULTS: f32 = 7.;
/// results_screen online: the round's results screen is up for its 6 s timer (OnlineFrontEndTimerID), then closed
/// (CloseOnlineContinueRetryDialog) for FEPoppedStateTimerID's 1 s.
const ROUND_SCREEN: f32 = 6.;
/// Contests between rounds: cleanup_round's 0.1 s fade, setup_round's 0.5 s load delay before the intro.
// ponytail: setup_round also waits for the round sync and the hull loads; those are not modelled.
const CLEANUP_FADE: f32 = 0.1;
const LOAD_DELAY: f32 = 0.5;
const ROUND_SETUP: f32 = CLEANUP_FADE + LOAD_DELAY;
/// Contests: initial's FadeOut and the FadeIn as each round's intro presentation ends (Effects.StartEffect 0.5).
pub(crate) const CONTEST_FADE: f32 = 0.5;
/// The end before OnlineChallengeComplete: Spot Battle's challenge_finished (2 s) and 1-Up's show_results (3 s), each
/// with "Challenge Complete".
const SPOT_BATTLE_ENDING: f32 = 2.;
const ONE_UP_ENDING: f32 = 3.;
/// 1-Up: an attempter who left gets a 1 s AttemptTimer (wait_for_scoring / wait_for_1up stateenter, and
/// ResetCountdownTimer(AttemptTimerID, 1) on playerdisconnected).
const GONE_ATTEMPT: f32 = 1.;
/// 1-Up: wait_for_leader / wait_for_follower's PreAttemptTimer, leader_success / follower_1up's wait, process_round's.
const PICK: f32 = 5.;
const AFTER_ATTEMPT: f32 = 2.;
const ROUND_OVER: f32 = 3.;
/// 1-Up: an attempt still in the air or grinding at time up runs on until its sequence ends. ponytail: retail waits
/// however long; capped so a stuck sequence can't hang the match.
const OVERTIME: f32 = 10.;
/// Contests: outside the round's boundary for its AudioOutOfBoundsWarningTime + this puts the skater back
/// (run_round triggerexitcollision).
const OUT_OF_AREA_AFTER_WARNING: f32 = 3.;
/// The contest Lua's NumWrongTricksToWarn: "Wrong Trick Type!" on every third wrong-type sequence.
const WRONG_TRICKS_TO_WARN: u32 = 3;
/// eRunType.SoloRunCycle / TimedRunMulti (S3 TU3's Contest.eRunType registration; Skate 2's data uses 0 and 4).
// ponytail: Skate 2's own eRunType table is not read; S3's (None 0, SoloRun 1, SoloRunCycle 2, TimedRunSingle 3,
// TimedRunMulti 4) stands in. No online row uses 1-3.
const SOLO_RUN_CYCLE: u32 = 2;
const TIMED_RUN_MULTI: u32 = 4;
/// How long a central message stays (hudcentral animates itself out).
const MESSAGE: f32 = 2.;
/// The match core's time cap: per turn / round beyond its own timers. A knob.
const SLACK: f32 = 15.;
// Draw order is name order.
const LEADERBOARD: &str = "1-leaderboard";
/// hudonlinevoting on Skate 2's online HUD layer (Spot Battle's and Hall of Meat's vote).
pub(crate) const VOTING: &str = "2-voting";
const DPO: &str = "3-dpo";
/// A contest round's results screen, over the HUD.
const ROUND_RESULTS_SCREEN: &str = "3-roundresults";
/// customiser::Navigation bits. ponytail: the vote buttons are taken as A up / B down (hudonlinevoting shows both
/// buttons; the C++ that reads them isn't traced).
const PAD_A: u16 = 0x1000;
const PAD_B: u16 = 0x2000;

// ---------------------------------------------------------------- data (contests.json)
#[derive(Deserialize)]
struct Data {
    rows: HashMap<String, Row>,
    strings: HashMap<String, String>,
    /// Scene -> seconds, from the intros' tutorial.json.
    #[serde(skip)]
    intros: HashMap<String, f32>,
    /// The intros' folder.
    #[serde(skip)]
    intro_root: PathBuf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Row {
    mode: String,
    title: String,
    time_limit: Option<f32>,
    /// The template's `CentralMessageHALID...` fields.
    messages: HashMap<String, String>,
    /// TurnBasedAttemptSpawnPoint / TurnBasedWaitingLocation.
    attempt: Option<Place>,
    waiting: Option<Place>,
    num_rounds: u32,
    sequences_to_end_turn: u32,
    cumulative: bool,
    spot_volumes: Vec<Aabb>,
    indicators: Vec<Place>,
    intro: Option<String>,
    rounds: Vec<Round>,
    num_to_win: u32,
    start_volume: Option<Aabb>,
    objects: Vec<Object>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Round {
    /// eContestType.
    variant: u32,
    time: f32,
    cumulative_min: f32,
    restart_on_wipeout: bool,
    /// eRunType.
    run_type: u32,
    /// AudioOutOfBoundsWarningTime. ponytail: an older contests.json lacks it; 1 s is every online row's but two.
    oob_warning: Option<f32>,
    /// PathEndVolume and RestartLastChanceVolume.
    path_end: Option<Aabb>,
    restart_volumes: Vec<Aabb>,
    boundary: Option<Aabb>,
    spot_volumes: Vec<Aabb>,
    start: Option<Place>,
    indicators: Vec<Place>,
    intro: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Object {
    volumes: Vec<Aabb>,
    start: Option<Place>,
    indicators: Vec<Place>,
}
#[derive(Deserialize, Clone)]
struct Place {
    locator: String,
    matrix: [[f32; 4]; 4],
    #[serde(default)]
    slots: Vec<[[f32; 4]; 4]>,
}
/// A trigger volume: every one on the disc has an identity rotation, so its box is world-aligned.
#[derive(Deserialize, Clone, Copy, Debug)]
struct Aabb {
    min: [f32; 3],
    max: [f32; 3],
}

impl Aabb {
    fn contains(&self, p: [f32; 3]) -> bool {
        (0..3).all(|k| self.min[k] <= p[k] && p[k] <= self.max[k])
    }
}
impl Place {
    /// AI.RepositionAI's spot for slot `i` (online_map::locator_slot).
    fn slot(&self, i: usize) -> [[f32; 4]; 4] {
        crate::online_map::locator_slot(&self.matrix, &self.slots, i)
    }
}
impl Data {
    fn load(assets: &Path) -> Result<Self, String> {
        let path = assets.join(ROOT).join("contests.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut data: Self = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let scenes = assets.join(INTROS).join("tutorial.json");
        let json = std::fs::read(&scenes)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<Json>(&b).map_err(|e| e.to_string()));
        match json {
            Ok(json) => {
                data.intros = (json["scenes"].as_object().into_iter().flatten())
                    .filter_map(|(k, v)| Some((k.clone(), v["duration"].as_f64()? as f32)))
                    .collect();
            }
            Err(e) => warn!("Skate 2 contests: {}: {e} (no intros)", scenes.display()),
        }
        data.intro_root = assets.join(INTROS);
        Ok(data)
    }
    fn text(&self, id: &str) -> String {
        self.strings.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }
    /// A row's template message, else `fallback`, localized.
    fn say(&self, row: &Row, field: &str, fallback: &str) -> String {
        self.text(row.messages.get(field).map_or(fallback, String::as_str))
    }
    fn intro(&self, scene: Option<&str>) -> Option<(String, f32)> {
        let scene = scene?;
        // It runs to its camera's end on its clock (nis-clock.md 10), not its record's length.
        let real = |s: f32| crate::challenge::cutscene::real_length(&self.intro_root, scene).unwrap_or(s);
        self.intros.get(scene).map(|&s| (scene.to_owned(), real(s)))
    }
}
impl Row {
    fn kind(&self) -> Option<Kind> {
        Kind::parse(&self.mode)
    }
    /// The flythrough before the match: the row's (Spot Battle, 1-Up) or round 1's (contests).
    fn intro(&self) -> Option<&str> {
        self.intro.as_deref().or_else(|| self.rounds.first()?.intro.as_deref())
    }
    fn rules(&self, kind: Kind, data: &Data) -> Rules {
        Rules {
            kind,
            rounds: match kind {
                Kind::SpotBattle => self.num_rounds.max(1),
                Kind::OneUp => 0,
                _ => self.rounds.len() as u32,
            },
            time_limit: self.time_limit.unwrap_or(30.),
            sequences_to_end_turn: self.sequences_to_end_turn.max(1),
            cumulative: self.cumulative,
            round_times: self.rounds.iter().map(|r| r.time).collect(),
            cumulative_min: self.rounds.iter().map(|r| r.cumulative_min).collect(),
            intros: self.rounds.iter().map(|r| data.intro(r.intro.as_deref()).map_or(0., |(_, s)| s)).collect(),
            num_to_win: self.num_to_win.max(1),
            objects: self.objects.len(),
        }
    }
}

// ---------------------------------------------------------------- the networkable contest
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Kind {
    #[default]
    SpotBattle,
    BestTrick,
    Jam,
    OneUp,
}
impl Kind {
    fn parse(mode: &str) -> Option<Self> {
        Some(match mode {
            "spot_battle" => Self::SpotBattle,
            "best_trick" => Self::BestTrick,
            "jam" => Self::Jam,
            "one_up" => Self::OneUp,
            _ => return None,
        })
    }
    fn contest(self) -> bool {
        matches!(self, Self::BestTrick | Self::Jam)
    }
}

/// The row's numbers the rules need.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Rules {
    pub kind: Kind,
    /// Spot Battle: NumRounds; contests: their rounds; 1-Up: open-ended (0).
    pub rounds: u32,
    /// Spot Battle / 1-Up: an attempt's TimeLimit.
    pub time_limit: f32,
    pub sequences_to_end_turn: u32,
    pub cumulative: bool,
    /// Contests, per round: RoundTime, CumulativeMinScore, the round's flythrough seconds (round 1's is the match
    /// core's intro: 0 here).
    pub round_times: Vec<f32>,
    pub cumulative_min: Vec<f32>,
    pub intros: Vec<f32>,
    pub num_to_win: u32,
    pub objects: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Player {
    pub name: String,
    /// Spot Battle: the best score (Score); contests: this round's score (roundScore); 1-Up: the last counted attempt.
    /// -1 once gone (it ranks last).
    pub best: f32,
    /// Spot Battle: this turn's sum (TurnScore, cumulative rows).
    pub turn: f32,
    /// Contests: the round scores summed (the solo result).
    pub points: f32,
    /// Contests: contest points; 1-Up: rounds won.
    pub total: i64,
    /// Disconnected.
    pub gone: bool,
}

/// Every timer counts seconds left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) enum Stage {
    /// Before the match core's countdown.
    #[default]
    Ready,
    /// Contests' later rounds: the round's flythrough.
    Intro { left: f32 },
    /// Spot Battle's countdown / 1-Up's round_setup and prepare_follower until teleporttransitionfinished: AI.RepositionAI's
    /// fade, its batch moved at black. `turn`: the attempter's own (an attempt follows); else the last one going back
    /// before the end (Spot Battle) or the round's results (1-Up).
    Move { left: f32, turn: bool },
    Countdown { left: f32 },
    /// 1-Up: the attempt starts on leaving the start volume, or when this runs out.
    Pick { left: f32 },
    Play { left: f32 },
    /// Spot Battle: the attempter is wiping out (the timer stops).
    Bailing,
    /// Between turns / attempts.
    Pause { left: f32 },
    /// Between rounds.
    Results { left: f32 },
    /// Spot Battle / 1-Up: "Challenge Complete" before the results (challenge_finished, show_results).
    Ending { left: f32 },
    Done,
}

/// What players do, as the rules take it (player = index in `Contest::players`, the match slot).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Event {
    /// A sequence landed that counts where it was skated (spot, boundary, variant); 1-Up: on `object`, if any.
    Scored { player: usize, points: f32, object: Option<usize> },
    Bailed { player: usize },
    /// The wipeout is over.
    Recovered { player: usize },
    /// 1-Up: left the start volume.
    Started { player: usize },
    /// In the air / grinding / in a sequence (1-Up's overtime waits for it).
    Busy { player: usize, on: bool },
    /// Spot Battle voting: 0 weak, 1 ok, 2 sweet, 3 gnarly.
    Vote { player: usize, level: u8 },
    Left { player: usize },
}
impl Event {
    /// The player the event is about (online: the only one allowed to send it).
    fn player(self) -> usize {
        match self {
            Event::Scored { player, .. }
            | Event::Bailed { player }
            | Event::Recovered { player }
            | Event::Started { player }
            | Event::Busy { player, .. }
            | Event::Vote { player, .. }
            | Event::Left { player } => player,
        }
    }
}

/// The host's contest as guests mirror it: everything but the rules and names (each game has those), in short keys to
/// fit an `@ev` record (1024 bytes; 10 players come to about 500).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Snap {
    /// Per player: best, turn, points, total, gone.
    p: Vec<(f32, f32, f32, i64, bool)>,
    o: Vec<usize>,
    r: u32,
    a: usize,
    st: Stage,
    sq: u32,
    ow: bool,
    vt: bool,
    v: Vec<u8>,
    bv: Option<(usize, f32)>,
    sc: bool,
    l: usize,
    fo: Vec<usize>,
    f: Option<usize>,
    ob: Option<usize>,
    u: Vec<usize>,
    tb: f32,
    aw: bool,
    ou: bool,
    b: bool,
    ot: bool,
    nr: bool,
    po: bool,
    mk: Option<usize>,
    /// The last few messages.
    n: Vec<(u32, Say)>,
    sd: u32,
    /// Per player: its events the host has taken.
    #[serde(default)]
    tk: Vec<u32>,
}

/// What a contest's players send each other through the match core's `@ev` records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Wire {
    /// A guest's own event, for the host, numbered by its sender.
    E(u32, Event),
    /// The host's contest, numbered.
    S(u32, Snap),
}

impl Snap {
    /// Every player index in range: a host's snapshot is a peer's word, and `placing` and a guest taking over index
    /// `players` with them.
    fn sane(&self) -> bool {
        let n = self.p.len();
        (self.o.iter().chain(&self.fo).chain([&self.a, &self.l])).all(|&i| i < n) && self.f.is_none_or(|f| f < self.fo.len())
    }
}

/// News a snapshot carries: a message shows on every game that sees it within this many.
const SNAP_NEWS: usize = 4;
/// The host re-sends an unchanged snapshot this often (timers drift; a dropped record is made good).
const HEARTBEAT: f32 = 1.;

/// Central messages the rules raise (the plugin words them from the row).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Say {
    YourTurn(usize),
    Round(u32),
    FinalRound,
    SpotOwned,
    Complete,
    NewLead(usize),
    LeaderPick,
    BeatHighScore,
    FollowerPick,
    NewScoreSet,
    FailedPick,
    DidNotBeat,
    OneUpped,
    ScoredLow,
    Bailed,
    TimeUp,
    RoundOver,
}

/// One match of any of the four modes: everything the online sync has to carry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Contest {
    pub rules: Rules,
    pub players: Vec<Player>,
    /// Standings, best first (player indices).
    pub order: Vec<usize>,
    /// 1-based.
    pub round: u32,
    /// The attempter (Spot Battle, 1-Up).
    pub active: usize,
    pub stage: Stage,
    /// Spot Battle: sequences this turn; the spot was owned this turn.
    pub sequences: u32,
    pub owned: bool,
    pub voting: bool,
    pub votes: Vec<u8>,
    /// The Player's Choice: (player, mean vote).
    pub best_voted: Option<(usize, f32)>,
    /// Contests: someone scored this round.
    pub scored: bool,
    /// 1-Up.
    pub leader: usize,
    pub followers: Vec<usize>,
    /// Index in `followers` of the attempter; None while the leader attempts.
    pub follower: Option<usize>,
    pub object: Option<usize>,
    pub used: Vec<usize>,
    pub to_beat: f32,
    pub award: bool,
    pub oneupped: bool,
    pub busy: bool,
    pub overtime: bool,
    /// Spot Battle: RoundNumberChanged (the turn's Move began a round); Pause.EnablePauseMenuAndBackpack has run (the
    /// first countdown's teleporttransitionfinished).
    pub new_round: bool,
    pub pause_ok: bool,
    /// 1-Up: the leaderboard's turn marker (LeaderboardSetTurn at wait_for_leader / wait_for_follower).
    pub marker: Option<usize>,
    /// The last messages, numbered by `said`.
    pub news: Vec<(u32, Say)>,
    pub said: u32,
    /// Online, per player: how many of its numbered events the host has taken.
    pub taken: Vec<u32>,
    /// Spot Battle: the first player, each round starting again from it (Skate 3's rule, user 10-28: drawn at random,
    /// S3 spot_battle.lua 143-144, 263). Every console draws it from the match token ([`setup`]), so snapshots don't
    /// carry it.
    #[serde(default)]
    pub first: usize,
    /// Party Play (Skate 3's party path, offline only): each turn's teleporttransitionfinished waits on the attempter's
    /// Press-A card; `cards` numbers them, `answered` is the card's hudcomplete.
    #[serde(default)]
    pub party: bool,
    #[serde(default)]
    pub waiting_a: bool,
    #[serde(default)]
    pub cards: u32,
    #[serde(default)]
    answered: bool,
}

/// VotingManager's score for the attempter: the mean of the other connected players' votes (0 with no voter).
pub(crate) fn tally(votes: &[u8], active: usize, gone: impl Fn(usize) -> bool) -> f32 {
    let (sum, n) = (votes.iter().enumerate())
        .filter(|&(i, _)| i != active && !gone(i))
        .fold((0., 0.), |(s, n), (_, &v)| (s + f32::from(v), n + 1.));
    if n > 0. { sum / n } else { 0. }
}

/// Moves `p` up `order` past everyone scoring strictly less; true when that took first place from someone.
pub(crate) fn promote(order: &mut [usize], score: impl Fn(usize) -> f32, p: usize) -> bool {
    let Some(from) = order.iter().position(|&i| i == p) else { return false };
    let mut at = from;
    while at > 0 && score(order[at - 1]) < score(p) {
        order.swap(at - 1, at);
        at -= 1;
    }
    at == 0 && from > 0
}

impl Contest {
    pub(crate) fn new(rules: Rules, names: Vec<String>, online: bool) -> Self {
        let n = names.len();
        Self {
            voting: online && n >= 3 && rules.kind == Kind::SpotBattle,
            votes: vec![1; n],
            players: names.into_iter().map(|name| Player { name, ..default() }).collect(),
            order: (0..n).collect(),
            rules,
            ..default()
        }
    }

    pub(crate) fn snap(&self) -> Snap {
        Snap {
            p: self.players.iter().map(|p| (p.best, p.turn, p.points, p.total, p.gone)).collect(),
            o: self.order.clone(),
            r: self.round,
            a: self.active,
            st: self.stage,
            sq: self.sequences,
            ow: self.owned,
            vt: self.voting,
            v: self.votes.clone(),
            bv: self.best_voted,
            sc: self.scored,
            l: self.leader,
            fo: self.followers.clone(),
            f: self.follower,
            ob: self.object,
            u: self.used.clone(),
            tb: self.to_beat,
            aw: self.award,
            ou: self.oneupped,
            b: self.busy,
            ot: self.overtime,
            nr: self.new_round,
            po: self.pause_ok,
            mk: self.marker,
            n: self.news[self.news.len().saturating_sub(SNAP_NEWS)..].to_vec(),
            sd: self.said,
            tk: self.taken.clone(),
        }
    }

    /// Takes the host's contest, keeping this game's rules and names.
    pub(crate) fn restore(&mut self, s: Snap) {
        self.players.resize_with(s.p.len(), Player::default);
        for (p, (best, turn, points, total, gone)) in self.players.iter_mut().zip(s.p) {
            (p.best, p.turn, p.points, p.total, p.gone) = (best, turn, points, total, gone);
        }
        (self.order, self.round, self.active, self.stage, self.sequences, self.owned) = (s.o, s.r, s.a, s.st, s.sq, s.ow);
        (self.voting, self.votes, self.best_voted, self.scored) = (s.vt, s.v, s.bv, s.sc);
        (self.leader, self.followers, self.follower, self.object, self.used) = (s.l, s.fo, s.f, s.ob, s.u);
        (self.to_beat, self.award, self.oneupped, self.busy, self.overtime) = (s.tb, s.aw, s.ou, s.b, s.ot);
        (self.new_round, self.pause_ok, self.marker) = (s.nr, s.po, s.mk);
        (self.news, self.said, self.taken) = (s.n, s.sd, s.tk);
    }
    /// Event `n` from `p` is new to the contest (not taken yet): takes it.
    fn take(&mut self, p: usize, n: u32) -> bool {
        if self.taken.len() <= p {
            self.taken.resize(p + 1, 0);
        }
        let new = n > self.taken[p];
        self.taken[p] = self.taken[p].max(n);
        new
    }

    /// A guest between snapshots: the timers run down, the host makes every change.
    pub(crate) fn coast(&mut self, dt: f32) {
        if let Stage::Intro { left }
        | Stage::Countdown { left }
        | Stage::Pick { left }
        | Stage::Play { left }
        | Stage::Pause { left }
        | Stage::Results { left }
        | Stage::Move { left, .. }
        | Stage::Ending { left } = &mut self.stage
        {
            *left = (*left - dt).max(0.);
        }
    }

    fn say(&mut self, say: Say) {
        self.said = self.said.saturating_add(1);
        self.news.push((self.said, say));
        if self.news.len() > 16 {
            self.news.remove(0);
        }
    }

    /// The match core's countdown began (`left` seconds of it): a contest's first round counts down with it. Spot Battle
    /// and 1-Up have no core countdown (the run began): wait_for_players / check_players send everyone to their waiting
    /// slot and the first attempter to its spawn in one fade (countdown stateenter / round_setup).
    pub(crate) fn start(&mut self, left: f32) {
        self.round = 1;
        match self.rules.kind {
            Kind::SpotBattle => {
                // Skate 3's rule (user 10-28): the drawn first player starts (S3 spot_battle.lua 143-144, 263).
                let n = self.players.len();
                self.active = (0..n).map(|k| (self.first + k) % n).find(|&p| !self.players[p].gone).unwrap_or(0);
                self.new_round = true;
                self.stage = Stage::Move { left: MOVE, turn: true };
            }
            Kind::OneUp => self.round_setup(true),
            _ => {
                self.stage = Stage::Countdown { left };
                // countdown stateenter: the round message only when ContestRounds.Count ~= 1.
                if self.rules.rounds != 1 {
                    self.say(self.round_news());
                }
            }
        }
    }

    /// FinalRound (red) on the last round, else RoundNum.
    fn round_news(&self) -> Say {
        if self.round == self.rules.rounds { Say::FinalRound } else { Say::Round(self.round) }
    }

    /// Party Play: the Press-A card that is up: (its number, player 1-based, round).
    pub(crate) fn card(&self) -> Option<(u32, usize, i64)> {
        self.waiting_a.then(|| (self.cards, self.active + 1, i64::from(self.round)))
    }
    /// Party Play's hudcomplete: the turn goes on at the next tick.
    pub(crate) fn hudcomplete(&mut self) {
        if std::mem::take(&mut self.waiting_a) {
            self.answered = true;
        }
    }

    pub(crate) fn tick(&mut self, dt: f32) {
        if !self.players.is_empty() && self.players.iter().all(|p| p.gone) && !matches!(self.stage, Stage::Ending { .. } | Stage::Done) {
            return self.finish();
        }
        let left = match &mut self.stage {
            Stage::Intro { left }
            | Stage::Countdown { left }
            | Stage::Pick { left }
            | Stage::Play { left }
            | Stage::Pause { left }
            | Stage::Results { left }
            | Stage::Move { left, .. }
            | Stage::Ending { left } => {
                *left -= dt;
                *left
            }
            Stage::Ready | Stage::Bailing | Stage::Done => return,
        };
        if left > 0. {
            return;
        }
        // Party Play: the turn's fade is over; its card's A (`hudcomplete`) moves on.
        if self.party && matches!(self.stage, Stage::Move { turn: true, .. }) && !take(&mut self.answered) {
            if !self.waiting_a {
                (self.waiting_a, self.cards) = (true, self.cards + 1);
            }
            return;
        }
        let kind = self.rules.kind;
        match self.stage {
            Stage::Intro { .. } => self.stage = Stage::Countdown { left: COUNTDOWN },
            Stage::Countdown { .. } => match kind {
                Kind::SpotBattle => {
                    self.sequences = 0;
                    self.owned = false;
                    self.overtime = false;
                    self.stage = Stage::Play { left: self.rules.time_limit };
                }
                // 1-Up has no countdown.
                _ => self.start_round(),
            },
            // wait_for_scoring / wait_for_1up stateenter: TimeLimit, or 1 s for an attempter who left.
            Stage::Pick { .. } => {
                let gone = self.players.get(self.active).is_none_or(|p| p.gone);
                self.stage = Stage::Play { left: if gone { GONE_ATTEMPT } else { self.rules.time_limit } };
            }
            Stage::Play { .. } => match kind {
                // Skate 3's rule (user 10-28): time up, the turn runs on END_RUN more seconds (the timer hidden) and a
                // sequence or a bail in them still counts, except on the match's last run (LastRun); then end_turn (S3
                // spot_battle.lua 901-912).
                Kind::SpotBattle if !self.overtime && !self.last_run() => {
                    self.overtime = true;
                    self.stage = Stage::Play { left: END_RUN };
                }
                Kind::SpotBattle => self.stage = Stage::Pause { left: END_TURN },
                Kind::OneUp if self.busy && !self.overtime => {
                    self.overtime = true;
                    self.stage = Stage::Play { left: OVERTIME };
                }
                Kind::OneUp => {
                    if !self.overtime {
                        self.say(Say::TimeUp);
                    }
                    self.attempt_done();
                }
                _ => self.end_round(),
            },
            Stage::Pause { .. } if kind == Kind::OneUp => self.next_attempt(),
            Stage::Pause { .. } => self.next_turn(),
            Stage::Results { .. } if kind == Kind::OneUp => {
                self.round = self.round.saturating_add(1);
                self.round_setup(false);
            }
            Stage::Results { .. } => self.next_round(),
            // teleporttransitionfinished.
            Stage::Move { turn, .. } => match (kind, turn) {
                // countdown: the pause menu back, "<name>, your turn" (+ the round when it changed), the 3 s countdown.
                (Kind::SpotBattle, true) => {
                    self.pause_ok = true;
                    self.stage = Stage::Countdown { left: COUNTDOWN };
                    self.say(Say::YourTurn(self.active));
                    if self.new_round {
                        self.say(Say::Round(self.round));
                    }
                }
                (Kind::SpotBattle, false) => self.finish(),
                // round_setup -> wait_for_leader, prepare_follower -> wait_for_follower.
                (_, true) => self.wait_for_attempt(),
                (_, false) => self.process_round(),
            },
            Stage::Ending { .. } => self.stage = Stage::Done,
            _ => {}
        }
    }

    pub(crate) fn apply(&mut self, event: Event) {
        let kind = self.rules.kind;
        let playing = matches!(self.stage, Stage::Play { .. });
        match event {
            Event::Left { player } => self.leave(player),
            Event::Vote { player, level } => {
                if self.voting && player != self.active && player < self.votes.len() {
                    self.votes[player] = level.min(3);
                }
            }
            Event::Busy { player, on } if player == self.active => {
                self.busy = on;
                // 1-Up's overtime (Spot Battle's after-time-up run ends on its own timer).
                if !on && self.overtime && playing && kind == Kind::OneUp {
                    self.attempt_done();
                }
            }
            Event::Scored { player, points, object } if player < self.players.len() => match kind {
                Kind::SpotBattle if playing && player == self.active => self.spot_scored(points),
                Kind::OneUp if playing && player == self.active => self.one_up_scored(points, object),
                Kind::BestTrick | Kind::Jam if playing && !self.players[player].gone => self.contest_scored(player, points),
                _ => {}
            },
            Event::Bailed { player } if playing && player == self.active => match kind {
                Kind::SpotBattle => self.stage = Stage::Bailing,
                Kind::OneUp => {
                    self.say(Say::Bailed);
                    self.attempt_done();
                }
                _ => {}
            },
            Event::Recovered { player } if player == self.active && self.stage == Stage::Bailing => {
                self.stage = Stage::Pause { left: END_TURN_WIPED };
            }
            Event::Started { player } if player == self.active && matches!(self.stage, Stage::Pick { .. }) => {
                self.stage = Stage::Play { left: self.rules.time_limit };
            }
            _ => {}
        }
    }

    /// This player's result for the match core.
    pub(crate) fn score(&self, player: usize) -> i64 {
        let Some(p) = self.players.get(player) else { return 0 };
        match self.rules.kind {
            Kind::SpotBattle => p.best.max(0.) as i64,
            Kind::OneUp => p.total,
            _ if self.players.len() > 1 => p.total,
            _ => p.points as i64,
        }
    }

    /// Online, the results order ([`Match::order`]): the Lua's leaderboard `order` (a tie keeps the earlier scorer:
    /// spot_battle.lua 98-107's strict `>` in `promote`, the contests' stable `rank_by_total`), a player who left last at
    /// -1 (spot_battle.lua 658-663, `leave`). 1-Up gives none: its tie order is UNRECOVERED, so the core's.
    pub(crate) fn placing(&self) -> Vec<(usize, i64)> {
        if self.rules.kind == Kind::OneUp {
            return Vec::new();
        }
        self.order.iter().map(|&i| (i, if self.players[i].gone { -1 } else { self.score(i) })).collect()
    }

    /// What the leaderboard shows (the Lua's LeaderboardHUDSetInt / SetFloat): the round's score while a contest round
    /// runs, 1-Up the attempt's (0 between rounds), else [`Self::score`]; -1 ('--') once gone (1-Up never sets it).
    fn shown(&self, player: usize) -> f64 {
        let p = &self.players[player];
        match (self.rules.kind, self.stage) {
            (Kind::OneUp, _) => f64::from(p.best),
            _ if p.gone => -1.,
            (Kind::BestTrick | Kind::Jam, Stage::Play { .. }) => f64::from(p.best.max(0.)),
            _ => self.score(player) as f64,
        }
    }

    /// The end: Spot Battle's challenge_finished and 1-Up's show_results show "Challenge Complete" for 2 / 3 s; a
    /// contest's last round goes on to its results (end_challenge has no message).
    fn finish(&mut self) {
        let ending = match self.rules.kind {
            Kind::SpotBattle => SPOT_BATTLE_ENDING,
            Kind::OneUp => ONE_UP_ENDING,
            _ => {
                self.stage = Stage::Done;
                return;
            }
        };
        self.stage = Stage::Ending { left: ending };
        self.say(Say::Complete);
    }

    fn leave(&mut self, player: usize) {
        let Some(p) = self.players.get_mut(player).filter(|p| !p.gone) else { return };
        p.gone = true;
        if self.rules.kind != Kind::OneUp {
            p.best = -1.;
            self.order.retain(|&i| i != player);
            self.order.push(player);
        }
        if player != self.active {
            return;
        }
        match (self.rules.kind, self.stage) {
            // countdown (its fade too) / challenge_running playerdisconnected: end_turn, its 2 s (0.01 s if wiped out).
            (Kind::SpotBattle, Stage::Move { .. } | Stage::Countdown { .. } | Stage::Play { .. }) => {
                self.stage = Stage::Pause { left: END_TURN }
            }
            (Kind::SpotBattle, Stage::Bailing) => self.stage = Stage::Pause { left: END_TURN_WIPED },
            // wait_for_leader / wait_for_follower: on to the attempt (1 s); wait_for_scoring / wait_for_1up: the
            // attempt timer reset to 1 s. ponytail: in overtime retail resets the expired timer too; same here.
            (Kind::OneUp, Stage::Pick { .. } | Stage::Play { .. }) => {
                (self.busy, self.overtime) = (false, false);
                self.stage = Stage::Play { left: GONE_ATTEMPT };
            }
            _ => {}
        }
    }

    // -------- Spot Battle
    fn spot_scored(&mut self, points: f32) {
        let a = self.active;
        self.sequences += 1;
        let score = if self.rules.cumulative {
            self.players[a].turn += points;
            self.players[a].turn
        } else {
            points
        };
        if score > self.players[a].best {
            self.players[a].best = score;
            let players = &self.players;
            promote(&mut self.order, |i| players[i].best, a);
            if self.order.first() == Some(&a) && !self.owned {
                self.owned = true;
                self.say(Say::SpotOwned);
            }
        }
        if !self.rules.cumulative && self.sequences >= self.rules.sequences_to_end_turn {
            self.stage = Stage::Pause { left: END_TURN };
        }
    }

    /// end_turn's timer: the votes, then countdown stateenter: the next connected player (a new round after the last) to
    /// the attempt spawn, the last one back to its waiting slot, in one fade. After the last round the last attempter
    /// goes back to the attempt spawn (its own slot, no camera switch) and challenge_finished follows the fade; gone, the
    /// camera goes to the next player (Online.SwitchCamera) and it finishes at once.
    fn next_turn(&mut self) {
        if self.voting {
            let players = &self.players;
            let score = tally(&self.votes, self.active, |i| players.get(i).is_some_and(|p| p.gone));
            if score > 1. && self.best_voted.is_none_or(|(_, best)| score > best) {
                self.best_voted = Some((self.active, score));
            }
            self.votes.fill(1);
        }
        if let Some(p) = self.players.get_mut(self.active) {
            p.turn = 0.;
        }
        let n = self.players.len();
        let mut next = self.active;
        self.new_round = false;
        for _ in 0..n {
            next = (next + 1) % n;
            // Skate 3's rule (user 10-28): each round starts again from the drawn first player (S3 spot_battle.lua 263).
            if next == self.first % n {
                self.round = self.round.saturating_add(1);
                self.new_round = true;
            }
            if !self.players[next].gone {
                break;
            }
        }
        if self.round <= self.rules.rounds && n > 0 {
            self.active = next;
            self.stage = Stage::Move { left: MOVE, turn: true };
        } else if self.players.get(self.active).is_some_and(|p| !p.gone) {
            self.stage = Stage::Move { left: MOVE, turn: false };
        } else {
            self.active = next;
            self.finish();
        }
    }

    /// LastRun (S3 spot_battle.lua countdown): this turn is the match's last: the last round, and no one still here
    /// after the attempter in its order.
    fn last_run(&self) -> bool {
        let n = self.players.len().max(1);
        let place = |p: usize| (p + n - self.first % n) % n;
        let later = (0..self.players.len()).any(|p| !self.players[p].gone && place(p) > place(self.active));
        self.round >= self.rules.rounds && !later
    }

    // -------- Best Trick / Jam
    fn start_round(&mut self) {
        for p in &mut self.players {
            p.best = if p.gone { -1. } else { 0. };
        }
        self.scored = false;
        let r = self.round.max(1) as usize - 1;
        self.stage = Stage::Play { left: self.rules.round_times.get(r).copied().unwrap_or(60.) };
    }

    fn contest_scored(&mut self, player: usize, points: f32) {
        let r = self.round.max(1) as usize - 1;
        let min = self.rules.cumulative_min.get(r).copied().unwrap_or(0.);
        let p = &mut self.players[player];
        if self.rules.cumulative && points >= min {
            p.best += points;
        } else if !self.rules.cumulative && points > p.best {
            p.best = points;
        } else {
            return;
        }
        let players = &self.players;
        let took = promote(&mut self.order, |i| players[i].best, player);
        let first = !self.scored && points > 0.;
        self.scored |= points > 0.;
        if took || first {
            self.say(Say::NewLead(player));
        }
    }

    /// The round's contest points: N-1 for the best, N-1-k for the k-th unless tied with the one above (gone players
    /// keep their rank but score nothing).
    fn end_round(&mut self) {
        let n = self.players.len() as i64;
        let mut ranked: Vec<usize> = (0..self.players.len()).collect();
        ranked.sort_by(|&a, &b| self.players[b].best.total_cmp(&self.players[a].best));
        let mut points = n - 1;
        for (k, &i) in ranked.iter().enumerate() {
            if k > 0 && self.players[i].best != self.players[ranked[k - 1]].best {
                points = n - 1 - k as i64;
            }
            let p = &mut self.players[i];
            if !p.gone {
                p.total = p.total.saturating_add(points);
                p.points += p.best;
            }
        }
        self.rank_by_total();
        if self.round >= self.rules.rounds {
            self.finish();
        } else {
            self.stage = Stage::Results { left: ROUND_RESULTS + ROUND_SETUP };
        }
    }

    fn next_round(&mut self) {
        self.round = self.round.saturating_add(1);
        let intro = self.rules.intros.get(self.round as usize - 1).copied().unwrap_or(0.);
        self.stage = if intro > 0. { Stage::Intro { left: intro } } else { Stage::Countdown { left: COUNTDOWN } };
        self.say(self.round_news());
    }

    fn rank_by_total(&mut self) {
        let players = &self.players;
        self.order.sort_by_key(|&i| (players[i].gone, std::cmp::Reverse(players[i].total)));
    }

    // -------- 1-Up
    /// round_setup: the leader picks first, then everyone after them in roster order. The leader goes to its spawn
    /// (UnpinAI at once) and wait_for_leader follows the fade; a leader who left isn't moved, so with no fade under way
    /// (`fading`: check_players' own) it follows at once.
    fn round_setup(&mut self, fading: bool) {
        let n = self.players.len();
        self.object = None;
        self.to_beat = 0.;
        self.award = false;
        self.follower = None;
        self.followers = (1..n).map(|k| (self.leader + k) % n).collect();
        self.active = self.leader;
        if fading || self.players.get(self.leader).is_some_and(|p| !p.gone) {
            self.stage = Stage::Move { left: MOVE, turn: true };
        } else {
            self.wait_for_attempt();
        }
    }

    /// wait_for_leader / wait_for_follower: LeaderboardSetTurn, the message, the 5 s pick. A leader who already left
    /// still gets it, then its 1 s attempt.
    fn wait_for_attempt(&mut self) {
        self.say(match (self.follower, self.object) {
            (None, _) => Say::LeaderPick,
            (Some(_), Some(_)) => Say::BeatHighScore,
            (Some(_), None) => Say::FollowerPick,
        });
        self.marker = Some(self.active);
        self.busy = false;
        self.overtime = false;
        self.oneupped = false;
        self.stage = Stage::Pick { left: PICK };
    }

    /// wait_for_scoring / wait_for_1up's sequencecomplete: a sequence on an unused object picks it (no object yet),
    /// one on the picked object is compared (followers).
    fn one_up_scored(&mut self, points: f32, object: Option<usize>) {
        let counted = match (self.object, object) {
            (None, Some(o)) if !self.used.contains(&o) => {
                self.object = Some(o);
                self.used.push(o);
                true
            }
            (Some(picked), Some(o)) => picked == o && self.follower.is_some(),
            _ => false,
        };
        if !counted {
            if self.overtime {
                self.attempt_done();
            }
            return;
        }
        self.players[self.active].best = points;
        if self.follower.is_none() {
            self.to_beat = points;
        } else if points > self.to_beat {
            self.leader = self.active;
            self.to_beat = points;
            self.oneupped = true;
            self.say(Say::OneUpped);
        } else {
            self.say(Say::ScoredLow);
        }
        self.attempt_done();
    }

    /// leader_success / follower_1up.
    fn attempt_done(&mut self) {
        self.award = true;
        let say = match (self.follower, self.object.is_some(), self.oneupped) {
            (None, true, _) | (Some(_), _, true) => Say::NewScoreSet,
            (None, false, _) => Say::FailedPick,
            (Some(_), _, false) => Say::DidNotBeat,
        };
        self.say(say);
        self.busy = false;
        self.stage = Stage::Pause { left: AFTER_ATTEMPT };
    }

    /// prepare_follower: the last attempter (CachedTeleportID) back to its waiting slot (if still here) and the next
    /// connected follower to the object's start in one fade, wait_for_follower after it. With the followers all done the
    /// fade's end is the round's (process_round); with the rest gone the online search finds none and process_round
    /// follows at once (the fade runs on). ponytail: retail's search wraps round FollowerOrder skipping only the last
    /// follower, so a follower who already went can go again, and with the last attempter gone too at the round's end no
    /// fade ever finishes (a hang); neither is ported.
    fn next_attempt(&mut self) {
        let from = self.follower.map_or(0, |i| i + 1);
        let cached = self.players.get(self.active).is_some_and(|p| !p.gone);
        match (from..self.followers.len()).find(|&i| !self.players[self.followers[i]].gone) {
            Some(i) => {
                self.follower = Some(i);
                self.active = self.followers[i];
                self.stage = Stage::Move { left: MOVE, turn: true };
            }
            None if from >= self.followers.len() && cached => self.stage = Stage::Move { left: MOVE, turn: false },
            None => self.process_round(),
        }
    }

    /// process_round: the leader (if still here) wins the round; NumToWin rounds win the match.
    fn process_round(&mut self) {
        self.say(Say::RoundOver);
        if self.used.len() >= self.rules.objects {
            self.used.clear();
        }
        let leader = self.leader;
        let mut won = false;
        if self.award && !self.players[leader].gone {
            self.players[leader].total = self.players[leader].total.saturating_add(1);
            won = self.players[leader].total >= i64::from(self.rules.num_to_win);
        }
        // LeaderboardHUDSetInt(id, 0) for everyone.
        for p in &mut self.players {
            p.best = 0.;
        }
        self.rank_by_total();
        if won {
            self.finish();
        } else {
            self.stage = Stage::Results { left: ROUND_OVER };
        }
    }
}

/// A landed sequence as a contest round scores it.
#[derive(Clone, Copy, Debug, Default)]
struct Landed {
    score: f32,
    /// Bit per Trick.eTrickType announced in the sequence.
    types: u32,
    /// It touched a spot volume (or ended in one).
    hit: bool,
    /// It ended inside the round's boundary (or there is none).
    inside: bool,
    /// 1-Up: the object it was skated on.
    object: Option<usize>,
    /// UsedSessionMarker: a return to the session marker came while it was under way.
    used: bool,
}

/// The contest Lua's run_round sequencecomplete for a UseSequence variant (eContestType Sequence 0, the trick-type ones
/// 1-6, Best_Line 14, Air_Only 15, Grind_Only 16): (points, a wrong trick type). Out of the boundary, or a Best Trick
/// sequence that touched no spot, scores 0; then the trick-type check. None: the variant scores airs or grinds.
/// Air_Only / Grind_Only score the sequence as it is: the scorer already skipped the types setup_round disabled
/// (online_scoring::set_round_trick_types).
/// ponytail: Footplant / Handplant / HippyJump (4-6) and Best_Line's volume chain are taken as Sequence (no online row
/// uses them).
fn sequence_points(best_trick: bool, variant: u32, l: &Landed) -> Option<(f32, bool)> {
    // Trick.eTrickType bits: None, Air, Flip, FingerFlip, Grab, Grind.
    const FLIP: u32 = 1 << 2;
    const GRAB: u32 = 1 << 4;
    const GRIND: u32 = 1 << 5;
    if !matches!(variant, 0..=6 | 14..=16) {
        return None;
    }
    if !l.inside || (best_trick && !l.hit) {
        return Some((0., false));
    }
    let wrong = match variant {
        1 => l.types & GRAB == 0,
        2 => l.types & GRIND == 0,
        3 => l.types & FLIP == 0,
        _ => false,
    };
    Some(if wrong { (0., true) } else { (l.score, false) })
}

/// CurNumWrongTricks after a sequence: a wrong type counts, and the NumWrongTricksToWarn-th warns (true) and starts
/// over; points clear it.
fn count_wrong(count: &mut u32, wrong: bool, points: f32) -> bool {
    if wrong {
        *count += 1;
        if *count >= WRONG_TRICKS_TO_WARN {
            *count = 0;
            return true;
        }
    } else if points > 0. {
        *count = 0;
    }
    false
}

/// aircompleted (Air_Distance 9, Air_Grab_Time 10, Air_Height 11, Air_Spin 12, Air_Time 13) and grindcompleted
/// (Grind_Length 7): the measure, 0 out of the boundary or (Best Trick airs) without a spot. None: the variant doesn't
/// score this.
fn measured_points(best_trick: bool, variant: u32, m: &Measured, grind: bool) -> Option<f32> {
    let measure = match (variant, grind) {
        (7, true) => m.measures[0],
        (9..=13, false) => m.measures[variant as usize - 9],
        _ => return None,
    };
    let refused = !m.inside || (!grind && best_trick && !m.hit);
    Some(if refused { 0. } else { measure })
}

/// The 1-Up attempter's spawn (RepositionAI(id, place, 0, false, 1, true): slot 0): a follower on the picked object's
/// start, the leader on the attempt spawn (or the last unused object's start).
fn one_up_spawn(row: &Row, s: &Contest) -> Option<[[f32; 4]; 4]> {
    let object = |i: usize| row.objects.get(i)?.start.as_ref().map(|p| p.slot(0));
    let attempt = row.attempt.as_ref().map(|p| p.slot(0));
    match (s.follower, s.object) {
        (Some(_), Some(o)) => object(o).or(attempt),
        (Some(_), None) => attempt,
        (None, _) => {
            let unused: Vec<usize> = (0..row.objects.len()).filter(|i| !s.used.contains(i)).collect();
            if unused.len() == 1 { object(unused[0]).or(attempt) } else { attempt }
        }
    }
}

/// The match core's time cap: the whole contest at its longest. ponytail: 1-Up is open-ended; capped at every player
/// winning NumToWin - 1 rounds, plus one.
fn time_cap(r: &Rules, players: usize) -> f32 {
    let n = players.max(1) as f32;
    match r.kind {
        Kind::SpotBattle => r.rounds as f32 * n * (MOVE + COUNTDOWN + r.time_limit + END_RUN + END_TURN + SLACK) + MOVE,
        Kind::OneUp => {
            let rounds = n * (r.num_to_win.max(1) - 1) as f32 + 1.;
            rounds * (n * (MOVE + PICK + r.time_limit + OVERTIME + AFTER_ATTEMPT) + MOVE + ROUND_OVER + SLACK)
        }
        _ => (r.round_times.iter().zip(&r.intros)).map(|(t, i)| t + i + COUNTDOWN + ROUND_RESULTS + SLACK).sum(),
    }
}

// ---------------------------------------------------------------- Bevy side
/// Online, the lowest slot still in the match runs the contest for everyone (it takes over when the host leaves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Net {
    /// This game runs the contest alone (solo).
    #[default]
    Solo,
    /// Runs the shared contest: applies `inbox` and its own events, ticks it, sends snapshots.
    Host,
    /// Mirrors the host's snapshots and sends its own events in `outbox`.
    Guest,
}

#[derive(Resource, Default)]
pub(crate) struct Contests {
    pub state: Option<Contest>,
    pub net: Net,
    /// The match's names in slot order (empty: just this player).
    pub names: Vec<String>,
    pub outbox: Vec<Event>,
    pub inbox: Vec<Event>,
    /// Host: the snapshot numbering, the last sent (timers zeroed) and when.
    seq: u32,
    sent: Option<(Snap, f32)>,
    /// Guest: the last snapshot taken (from slot, number).
    heard: Option<(usize, u32)>,
    /// Guest: this game's events numbered so far, and every numbered event (sender, number) sent or heard that the
    /// host's snapshots don't have taken yet: should the host leave, the next host takes them.
    numbered: u32,
    pending: Vec<(usize, u32, Event)>,
    data: Option<Arc<Data>>,
    assets: PathBuf,
    /// The row last warned about (a missing row is logged once, not every frame).
    warned: String,
    probe: Probe,
    run: Option<Run>,
    screens: TutorialScreens,
}

/// The physics ticks' view ([`tick`] fills it; the frame side sets the zones and takes the facts).
#[derive(Default)]
struct Probe {
    live: bool,
    spots: Vec<Aabb>,
    boundary: Option<Aabb>,
    /// 1-Up: the objects a sequence may count on.
    objects: Vec<(usize, Vec<Aabb>)>,
    start: Option<Aabb>,
    /// Scorable identifier -> TrickType.
    kinds: HashMap<&'static str, u32>,
    open: bool,
    seq: Landed,
    /// The session marker's returns (SessionMarker::uses) last seen, and a return since the last sequence ended.
    marker_uses: Option<u64>,
    marked: bool,
    last: Option<[f32; 3]>,
    wiping: bool,
    in_start: bool,
    current: Option<usize>,
    pos: [f32; 3],
    busy: bool,
    landed: Vec<Landed>,
    bailed: bool,
    recovered: bool,
    left_start: bool,
    /// The physics clock (seconds), for air times.
    clock: f32,
    /// The air and the grind under way, and those landed since the frame side last looked (aircompleted /
    /// grindcompleted).
    air: Option<Air>,
    grind: Option<([f32; 3], f32, u64)>,
    airs: Vec<Measured>,
    grinds: Vec<Measured>,
    /// The round's PathEndVolume and RestartLastChanceVolumes: inside them now, and entered since the frame side looked.
    path_end: Option<Aabb>,
    restarts: Vec<Aabb>,
    in_path_end: bool,
    in_restart: bool,
    reached_end: bool,
    hit_restart: bool,
    /// wipeoutalmostover: the wipeout's reset countdown (the engine's TeleportAutoResetFadeOutTime), (left, length).
    fade_out: Option<(f32, f32)>,
}

/// An air under way: take-off point and time, its highest point, its biggest rotation (degrees).
#[derive(Clone, Copy, Debug, Default)]
struct Air {
    from: [f32; 3],
    at: f32,
    peak: f32,
    spin: f32,
    /// The session marker's returns (SessionMarker::uses) at take-off.
    uses: u64,
}

/// A landed air or grind: its measure under each air variant (eContestType 9 Air_Distance .. 13 Air_Time; a grind's
/// length in [0]), whether it ended inside the round's boundary and in a sequence that touched a spot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Measured {
    measures: [f32; 5],
    inside: bool,
    hit: bool,
    /// UsedSessionMarker, as [`Landed::used`].
    used: bool,
}

#[derive(Default)]
struct Run {
    row: String,
    title: String,
    /// The match core's intro flythrough ends.
    intro_ends: Option<f32>,
    /// A contest round's flythrough is playing.
    cutscene: bool,
    /// (stage, round, attempter, follower) last handled.
    seen: Option<(Discriminant<Stage>, u32, usize, Option<usize>)>,
    was_active: bool,
    said: u32,
    /// (text, presentation, shown from).
    messages: Vec<(String, Central, f32)>,
    /// This console's own skater's next RepositionAI move (made at the fade's black).
    teleport: Option<[[f32; 4]; 4]>,
    /// A RepositionAI fade under way here (its start), and the player its black switches the camera to (bool6).
    fade_from: Option<f32>,
    switch_to: Option<usize>,
    /// The player the camera follows (Online.SwitchCamera); None (this player) until a turn's fade switches it.
    watching: Option<usize>,
    /// The first RepositionAI fade (everyone to their waiting slot) has started.
    moved: bool,
    /// Spot Battle's wipeoutalmostover FadeOut has gone black: it stays black until the next RepositionAI's black.
    dark: bool,
    out_since: Option<f32>,
    /// CurNumWrongTricks.
    wrong: u32,
    /// The contest round's startLocator, restartAtStart and restartVolumeHit (setup_round: this player's slot,
    /// RestartOnWipeout or any RestartLastChanceVolume, false), and the start slot a rule sends it back to.
    start_locator: usize,
    restart_at_start: bool,
    restart_hit: bool,
    restart_to: Option<usize>,
    go_until: f32,
    busy: bool,
    vote: u8,
    vote_shown: Option<u8>,
    /// leaderboardhud's answers last given.
    board: Option<[(&'static str, Json); 9]>,
    /// dpo's answers while shown.
    dpo: Option<[(&'static str, Json); 3]>,
    /// Contests: round 1's intro scene and when it starts (after initial's FadeOut and setup_round's load delay).
    intro_at: Option<(String, f32)>,
    /// Contests: the Effects fade in force ([`effects_black`]).
    effect: Option<Fade>,
    /// Contests: the round's results screen is up.
    round_shown: bool,
    /// Party Play: the last card opened ([`Contest::cards`]).
    carded: u32,
}

/// An Effects.StartEffect fade: out to black or in from it, over `length` seconds from `from`. ponytail: linear, as
/// ChallengeFade.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    out: bool,
    from: f32,
    length: f32,
}
impl Fade {
    fn black(self, now: f32) -> f32 {
        let t = ((now - self.from) / self.length).clamp(0., 1.);
        if self.out { t } else { 1. - t }
    }
}
impl Run {
    /// RepositionAI at black: true once this console's own move is due (no fade, or the fade's black), when the
    /// camera switch and the wipeout's black are handed over. A move asked for while a fade runs joins it. ponytail:
    /// one asked for after that fade's black is made at once (whether retail's batch still takes it then is not traced).
    fn at_black(&mut self, now: f32) -> bool {
        let due = self.fade_from.is_none_or(|from| now - from >= REPOSITION_FADE / 2.);
        if due && self.fade_from.is_some() {
            self.dark = false;
            if let Some(p) = self.switch_to.take() {
                self.watching = Some(p);
            }
        }
        due
    }
    /// The screen: the attempter's wipeout FadeOut (`wipe`: time left, length), black held until the next
    /// RepositionAI's black, else that RepositionAI's fade (dropped once over).
    fn black(&mut self, wipe: Option<(f32, f32)>, now: f32) -> f32 {
        self.dark |= wipe.is_some();
        let black = match (wipe, self.dark) {
            (Some((left, length)), _) => 1. - (left / length).clamp(0., 1.),
            (None, true) => 1.,
            (None, false) => self.fade_from.map_or(0., |from| reposition_black(now - from)),
        };
        if self.fade_from.is_some_and(|from| now - from >= MOVE) {
            self.fade_from = None;
        }
        black
    }
}

pub(crate) struct Skate2ContestsPlugin;
impl Plugin for Skate2ContestsPlugin {
    fn build(&self, app: &mut App) {
        let assets = app.world().resource::<Config>().asset_root.clone();
        let data = Data::load(&assets).inspect_err(|e| warn!("Skate 2 contests: {e}")).ok().map(Arc::new);
        if let (Some(data), Some(mut playable)) = (&data, app.world_mut().get_resource_mut::<Playable>()) {
            playable.0.extend(data.rows.iter().filter(|(_, r)| r.kind().is_some()).map(|(id, _)| id.clone()));
        }
        app.insert_resource(Contests { data, assets, ..default() })
            .add_systems(FixedUpdate, tick.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, update.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick: the sequence under way (spot touched, trick types, spin, grind), its landing, each air and
/// grind landed, the wipeout edges, the start / path-end / restart volumes and the 1-Up object under the skater.
/// ponytail: the natives' air and grind measures (Trick.GetCompletedAir*, grindcompleted's GrindLength) are read from
/// the skater as skate3_contest's probe does (horizontal take-off-to-landing distance, height over the take-off, spin
/// half-turns x 180, time, path length; grab time 0); a wipeout drops the air or grind.
fn tick(
    mut contests: ResMut<Contests>,
    skater: Res<SkaterRuntime>,
    time: Res<Time>,
    marker: Option<Res<crate::session_marker::SessionMarker>>,
) {
    let p = &mut contests.probe;
    if !p.live {
        // A return outside a run (between rounds) flags nothing.
        (p.marker_uses, p.marked) = (None, false);
        return;
    }
    // UsedSessionMarker (Skate 3's rule, user 10-28): a return to the session marker is a teleport, which wipes the
    // sequence under way (scoring_runtime reset), so the flag is on the next sequence, as skate1_online's probe. An air or
    // grind cut short by the return is still dropped (its measure would run to the marker).
    let uses = marker.map_or(0, |m| m.uses);
    p.marked |= p.marker_uses.is_some_and(|was| was != uses);
    p.marker_uses = Some(uses);
    p.clock += time.delta_secs();
    let s = &skater.scoring;
    if p.kinds.is_empty() {
        p.kinds = s.data.definitions.iter().map(|d| (d.identifier, d.trick_type)).collect();
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let pos = [at[0], at[1], at[2]];
    let in_spot = p.spots.iter().any(|v| v.contains(pos));
    let open = s.sequence_open();
    if open && !p.open {
        p.seq = Landed::default();
    }
    if open || s.sequence_landed {
        p.seq.hit |= in_spot;
        for id in &s.announced {
            if let Some(&t) = p.kinds.get(id) {
                p.seq.types |= 1 << t.min(31);
            }
        }
    }
    if s.sequence_landed {
        let inside = p.boundary.is_none_or(|b| b.contains(pos));
        let used = std::mem::take(&mut p.marked);
        p.landed.push(Landed { score: s.lua_sequence_score() as f32, inside, object: p.current, used, ..p.seq });
    }
    let was_open = p.open;
    p.open = open;
    p.last = Some(pos);
    p.busy = open || s.in_air() || s.grinding();
    // wipeout / wipeoutover: the Wipeout category's edges (S2 0x8234d2e0 / 0x8234dd30); a dismount is neither.
    let wiping = skater.wiping_out();
    let inside = p.boundary.is_none_or(|b| b.contains(pos));
    let hit = p.seq.hit || in_spot;
    if wiping && !p.wiping {
        (p.air, p.grind) = (None, None);
        // A sequence a bail ended was the one after the return, as skate1_online's wipeout sequencecomplete takes it.
        p.marked &= !(open || was_open);
    }
    if s.in_air() && !wiping {
        let a = p.air.get_or_insert(Air { from: pos, at: p.clock, peak: pos[1], spin: 0., uses });
        a.peak = a.peak.max(pos[1]);
        a.spin = a.spin.max(s.spin_turns.unsigned_abs() as f32 * 180.);
    } else if let Some(a) = p.air.take().filter(|_| !wiping) {
        let distance = Vec2::new(pos[0] - a.from[0], pos[2] - a.from[2]).length();
        let measures = [distance, 0., pos[1].max(a.peak) - a.from[1], a.spin, p.clock - a.at];
        p.airs.push(Measured { measures, inside, hit, used: uses != a.uses });
    }
    if s.grinding() && !wiping {
        let g = p.grind.get_or_insert((pos, 0., uses));
        g.1 += Vec3::from(g.0).distance(Vec3::from(pos));
        g.0 = pos;
    } else if let Some((_, length, since)) = p.grind.take().filter(|_| !wiping) {
        p.grinds.push(Measured { measures: [length, 0., 0., 0., 0.], inside, hit, used: uses != since });
    }
    let in_end = p.path_end.is_some_and(|v| v.contains(pos));
    p.reached_end |= in_end && !p.in_path_end;
    p.in_path_end = in_end;
    let in_restart = p.restarts.iter().any(|v| v.contains(pos));
    p.hit_restart |= in_restart && !p.in_restart;
    p.in_restart = in_restart;
    // The Lua's CurrentObjectIndex: the object entered; leaving it mid-air keeps it until the sequence ends.
    let on = p.objects.iter().find(|(_, v)| v.iter().any(|v| v.contains(pos))).map(|(i, _)| *i);
    if on.is_some() || !p.busy {
        p.current = on;
    }
    let in_start = p.start.is_some_and(|v| v.contains(pos));
    p.left_start |= p.in_start && !in_start;
    p.in_start = in_start;
    p.bailed |= wiping && !p.wiping;
    p.recovered |= !wiping && p.wiping;
    p.wiping = wiping;
    p.pos = pos;
    let w = &skater.wipeout_state.state;
    p.fade_out = match p.fade_out {
        _ if !(wiping && w.teleport_pending) => None,
        Some((_, length)) => Some((w.time_until_teleport, length)),
        None => Some((w.time_until_teleport, w.time_until_teleport.max(1. / 60.))),
    };
}

fn update(world: &mut World) {
    world.resource_scope(|world, mut contests: Mut<Contests>| {
        let (now, dt) = (crate::match_core::match_now(world), crate::match_core::match_dt(world));
        let (mode, phase, owned) = {
            let m = world.resource::<Match>();
            (m.mode, m.phase.clone(), m.owned)
        };
        // A Skate 1 row (skate1_online owns its match) is not ours.
        let kind = mode.and_then(|m| Kind::parse(m.name())).filter(|_| phase != Phase::Idle && !owned);
        match (kind, contests.data.clone()) {
            (Some(kind), Some(data)) => step(world, &mut contests, &data, kind, &phase, now, dt),
            _ if contests.run.is_some() => finish(world, &mut contests),
            _ => {}
        }
    });
}

/// Ends the match: screens, scene and indicators off, the contest dropped.
fn finish(world: &mut World, c: &mut Contests) {
    c.run = None;
    c.state = None;
    c.outbox.clear();
    c.inbox.clear();
    (c.net, c.seq, c.sent, c.heard, c.numbered) = (Net::Solo, 0, None, None, 0);
    c.pending.clear();
    c.names.clear();
    c.probe = Probe::default();
    if let Some(mut rules) = world.get_resource_mut::<crate::online_scoring::ScoringRules>() {
        rules.skate2 = None;
    }
    crate::online_scoring::challenge_ended(world);
    c.screens.clear(world);
    if let Some(mut screens) = world.get_resource_mut::<Screens>() {
        let layer = screens.layer(SKATE2_ONLINE);
        layer.close(LEADERBOARD);
        layer.close(VOTING);
        layer.close(DPO);
        layer.close(ROUND_RESULTS_SCREEN);
    }
    world.resource_mut::<Cutscene>().stop();
    let mut arrows = world.resource_mut::<Arrows>();
    arrows.wanted.clear();
    arrows.more.clear();
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        fade.0 = 0.;
    }
}

pub(crate) fn player_name(world: &World) -> String {
    world.get_resource::<crate::presence::Presence>().map_or_else(|| "The Legend".into(), |p| p.display_name())
}

/// The match starts once its trip is over: the contest (unless a guest's arrives from the host), the intro flythrough,
/// the time cap.
fn setup(world: &mut World, c: &mut Contests, data: &Data, kind: Kind, now: f32) -> bool {
    // Bug 11-01 (as skate1_online's 10-31): Party Play's start travels here behind the loading screen (online: the start
    // slot's cover), and the flythrough played out behind it; the core's intro waits meanwhile. A contest's start is
    // under its own FadeOut (match_core::contest_fading).
    if !kind.contest() && !crate::skate1_online::trip_over(world, MAP) {
        let mut m = world.resource_mut::<Match>();
        (m.intro, m.no_countdown) = (true, true);
        return false;
    }
    let row_id = world.resource::<Match>().row.clone();
    let Some(row) = data.rows.get(&row_id).filter(|r| r.kind() == Some(kind)) else {
        if c.warned != row_id {
            warn!("Skate 2 contests: no {kind:?} row {row_id:?} in contests.json");
            c.warned = row_id;
        }
        world.resource_mut::<Match>().finished = true;
        return false;
    };
    let online = world.resource::<Match>().token != 0;
    // Party Play's turn kinds: its players take turns on this one skater (Skate 3's party path).
    let party = !online && matches!(kind, Kind::SpotBattle | Kind::OneUp) && crate::party_play::in_session(world);
    // Every game starts the same contest from the same names; a guest's then follows the host's snapshots.
    if c.state.is_none() {
        let names = if party {
            crate::party_play::names(world)
        } else if c.names.is_empty() {
            vec![player_name(world)]
        } else {
            c.names.clone()
        };
        let mut s = Contest::new(row.rules(kind, data), names, online);
        s.party = party;
        if kind == Kind::SpotBattle {
            // Skate 3's rule (user 10-28): a random first player, the same on every console: the match token, as
            // skate3_uniform's draw (S3 spot_battle.lua 143-144); Party Play from the clock. Solo has one player.
            let seed = if party { u64::from(now.to_bits()) } else { world.resource::<Match>().token };
            s.first = (seed % s.players.len().max(1) as u64) as usize;
        }
        c.state = Some(s);
    }
    let intro = data.intro(row.intro());
    // Contests: initial's FadeOut 0.5 (contest_jam.lua 7) and setup_round's 0.5 s load delay (685-690) come before
    // round 1's intro presentation; the core's intro waits through them, with or without one.
    let contest = kind.contest();
    let wait = if contest { CONTEST_FADE + LOAD_DELAY } else { 0. };
    world.resource_mut::<Match>().intro = intro.is_some() || contest;
    world.resource_mut::<Match>().title = data.text(&row.title);
    if let Some((scene, _)) = intro.as_ref().filter(|_| !contest) {
        world.resource_mut::<Cutscene>().start(&c.assets.join(INTROS), scene);
    }
    world.resource_mut::<Arrows>().more = vec![format!("{ROOT}/indicator/indicators.json")];
    // Skate 2's ChallengeType for the score multiplier gate (online_scoring.rs).
    let challenge_type = match kind { Kind::SpotBattle => 9, Kind::BestTrick => 8, Kind::Jam => 15, Kind::OneUp => 32 };
    if let Some(mut rules) = world.get_resource_mut::<crate::online_scoring::ScoringRules>() {
        rules.skate2 = Some(challenge_type);
    }
    c.probe = Probe { live: true, ..default() };
    c.run = Some(Run {
        row: row_id,
        title: data.text(&row.title),
        intro_ends: (intro.is_some() || contest).then(|| now + wait + intro.as_ref().map_or(0., |(_, s)| *s)),
        intro_at: intro.filter(|_| contest).map(|(scene, _)| (scene, now + wait)),
        effect: contest.then_some(Fade { out: true, from: now, length: CONTEST_FADE }),
        vote: 1,
        ..default()
    });
    true
}

fn end_intro(world: &mut World, run: &mut Run) {
    run.intro_ends = None;
    run.intro_at = None;
    world.resource_mut::<Cutscene>().stop();
    world.resource_mut::<Match>().intro = false;
}

#[allow(clippy::too_many_arguments)]
fn step(world: &mut World, c: &mut Contests, data: &Data, kind: Kind, phase: &Phase, now: f32, dt: f32) {
    let (token, slot, players) = {
        let m = world.resource::<Match>();
        (m.token, m.slot, m.players.clone())
    };
    if token != 0 && !players.is_empty() {
        let host = players.iter().position(|n| !n.is_empty());
        c.net = if host == Some(slot) { Net::Host } else { Net::Guest };
        if c.state.is_none() {
            c.names = players.clone();
        }
    }
    if c.run.is_none() && !setup(world, c, data, kind, now) {
        return;
    }
    let Some(mut run) = c.run.take() else { return };
    let row = &data.rows[&run.row];
    // Party Play: the one skater is whoever's turn it is.
    let party = c.state.as_ref().is_some_and(|s| s.party);
    let me = match &c.state {
        Some(s) if s.party => s.active,
        _ if c.net == Net::Solo => 0,
        _ => slot,
    };
    if c.net != Net::Solo {
        let items = take(&mut world.resource_mut::<Match>().inbox);
        receive(c, &players, items);
    }

    // The match core's intro, then a contest's first countdown with the core's (Spot Battle and 1-Up have none: their
    // Lua goes from the flythrough to its first reposition); the cap before Running.
    match phase {
        Phase::Intro { .. } if run.intro_ends.is_some_and(|e| now >= e) => end_intro(world, &mut run),
        Phase::Countdown { .. } | Phase::Running { .. } if run.intro_ends.is_some() => end_intro(world, &mut run),
        _ => {}
    }
    // A contest's round 1 intro presentation, once the start is black.
    if matches!(phase, Phase::Intro { .. })
        && let Some((scene, _)) = run.intro_at.take_if(|(_, at)| now >= *at)
    {
        world.resource_mut::<Cutscene>().start(&c.assets.join(INTROS), &scene);
    }
    let begin = match phase {
        Phase::Countdown { ends } if kind.contest() => Some((ends - now).max(0.)),
        Phase::Running { .. } if !kind.contest() => Some(0.),
        _ => None,
    };
    if let Some(left) = begin
        && let Some(s) = c.state.as_mut().filter(|s| s.stage == Stage::Ready)
    {
        s.start(left);
    }
    if matches!(phase, Phase::Intro { .. } | Phase::Countdown { .. }) {
        let m = world.resource::<Match>();
        let players = (c.state.as_ref().map_or(0, |s| s.players.len())).max(c.names.len()).max(m.standings.len());
        let cap = time_cap(&row.rules(kind, data), players);
        let mut m = world.resource_mut::<Match>();
        // Party Play has no cap: its cards and its results dialog wait on the players.
        m.time_limit = (!party).then_some(cap);
        m.no_countdown = !kind.contest();
    }

    // This player's events, from what the physics ticks saw.
    let p = &mut c.probe;
    let (mut landed, bailed, recovered, left_start) =
        (take(&mut p.landed), take(&mut p.bailed), take(&mut p.recovered), take(&mut p.left_start));
    let (mut airs, mut grinds, reached_end, hit_restart) = (take(&mut p.airs), take(&mut p.grinds), take(&mut p.reached_end), take(&mut p.hit_restart));
    let (busy, pos) = (p.busy, p.pos);
    if kind.contest() {
        // Skate 3's rule (user 10-28): a contest's sequence, air or grind skated with a return to the session marker
        // counts for nothing, its whole handler skipped (S3 contest_shared.lua 1091, 1323, 1401: UsedSessionMarker).
        landed.retain(|l| !l.used);
        airs.retain(|m| !m.used);
        grinds.retain(|m| !m.used);
    }
    let mut events = Vec::new();
    let mut local = Vec::new();
    if let Some(s) = &c.state {
        let mine = kind.contest() || s.active == me;
        let playing = matches!(s.stage, Stage::Play { .. });
        let round = row.rounds.get(s.round.max(1) as usize - 1);
        if kind.contest() && s.round > 0 && let Some(r) = round {
            crate::online_scoring::set_round_trick_types(world, r.variant);
        }
        let best_trick = kind == Kind::BestTrick;
        for l in landed.iter().filter(|_| mine) {
            let points = match (kind, round) {
                (Kind::SpotBattle, _) => l.hit.then_some(l.score),
                (Kind::OneUp, _) => Some(l.score),
                (_, Some(r)) => sequence_points(best_trick, r.variant, l).and_then(|(points, wrong)| {
                    if playing && count_wrong(&mut run.wrong, wrong, points) {
                        local.push((data.text("ID_CONTEST_WRONG_TYPE"), Central::Plain));
                    }
                    (points > 0.).then_some(points)
                }),
                _ => None,
            };
            if let Some(points) = points {
                events.push(Event::Scored { player: me, points, object: l.object });
            }
        }
        if let Some(r) = round.filter(|_| kind.contest()) {
            let measured = airs.iter().map(|m| (m, false)).chain(grinds.iter().map(|m| (m, true)));
            for points in measured.filter_map(|(m, grind)| measured_points(best_trick, r.variant, m, grind)).filter(|&p| p > 0.) {
                events.push(Event::Scored { player: me, points, object: None });
            }
            if playing {
                restart_rules(&mut run, r, me, c.names.len().max(1), [reached_end, hit_restart, !landed.is_empty(), recovered]);
            }
        }
        if mine {
            events.extend(bailed.then_some(Event::Bailed { player: me }));
            events.extend(recovered.then_some(Event::Recovered { player: me }));
            events.extend(left_start.then_some(Event::Started { player: me }));
            if busy != run.busy {
                run.busy = busy;
                events.push(Event::Busy { player: me, on: busy });
            }
        }
        if s.voting && s.active != me && matches!(s.stage, Stage::Play { .. } | Stage::Bailing) {
            let pressed = world.get_resource::<crate::customiser::Navigation>().map_or(0, |n| n.pressed);
            let vote = next_vote(pressed, run.vote);
            if vote != run.vote {
                run.vote = vote;
                events.push(Event::Vote { player: me, level: vote });
            }
        }
    }
    let wire = exchange(c, &players, events, now, dt);
    world.resource_mut::<Match>().outbox.extend(wire);
    let Some(state) = c.state.clone() else {
        c.run = Some(run);
        return;
    };
    // Party Play: the turn that just began is the skater's (its events above were the last turn's player's).
    let me = if state.party { state.active } else { me };

    // Stage changes: teleports, indicators, flythroughs, what the probe watches.
    let key = (discriminant(&state.stage), state.round, state.active, state.follower);
    if run.seen != Some(key) {
        transition(world, c, row, &mut run, &state, me, now);
        run.seen = Some(key);
    }
    let round = row.rounds.get(state.round.max(1) as usize - 1);
    if let (true, Stage::Play { .. }, Some(round)) = (kind.contest(), state.stage, round) {
        // run_round triggerexitcollision on the boundary: ReturnToChallengeArea, and its timer
        // (AudioOutOfBoundsWarningTime + 3 s) puts the skater back on the round's start (RepositionAI(id, StartLocation):
        // its slot = the player id, the default 1 s fade). Coming back in stops it.
        match round.boundary {
            Some(b) if !b.contains(pos) => match run.out_since {
                Some(since) if now - since >= round.oob_warning.unwrap_or(1.) + OUT_OF_AREA_AFTER_WARNING => {
                    run.teleport = round.start.as_ref().map(|p| p.slot(me));
                    run.fade_from.get_or_insert(now);
                    // Spent: retail's timer fires once per exit.
                    run.out_since = Some(f32::INFINITY);
                }
                Some(_) => {}
                None => {
                    run.out_since = Some(now);
                    let text = data.say(row, "CentralMessageHALIDReturnToChallengeArea", "ID_CONTEST_CENTRALMESSAGE_RETURNTOCHALLENGEAREA");
                    local.push((text, Central::Plain));
                }
            },
            _ => run.out_since = None,
        }
        // The wipeoutover restart and TimedRunMulti's path end: RepositionAI(local, StartLocation[, slot]), default fade.
        if let Some(slot) = run.restart_to.take() {
            run.teleport = round.start.as_ref().map(|p| p.slot(slot));
            run.fade_from.get_or_insert(now);
        }
    }
    if run.at_black(now)
        && let Some(at) = run.teleport
        && teleport(world, at)
    {
        run.teleport = None;
    }
    // wipeoutalmostover (challenge_running): Effects FadeOut over TimeToTeleport for the attempter, black until the next
    // RepositionAI's. ponytail: this console's own skater's only (whether a remote attempter's message reaches the
    // others' Lua isn't traced).
    let wipe = c.probe.fade_out.filter(|_| {
        kind == Kind::SpotBattle && state.active == me && matches!(state.stage, Stage::Play { .. } | Stage::Bailing)
    });
    let mut black = run.black(wipe, now);
    if kind.contest() {
        let nis = world.resource::<Cutscene>().active();
        black = black.max(effects_black(&mut run, state.stage, nis, now));
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>()
        && fade.0 != black
    {
        fade.0 = black;
    }

    {
        let mut m = world.resource_mut::<Match>();
        m.score = state.score(me);
        // Party Play's match ends at its results dialog, not at the core's results.
        m.finished |= state.stage == Stage::Done && !state.party;
        m.pause_disabled = pause_disabled(&state);
        m.order = if c.net == Net::Solo { Vec::new() } else { state.placing() };
        if !matches!(phase, Phase::Results { .. }) {
            (m.hold, m.watch) = hold_and_watch(&state, me, c.net != Net::Solo, run.watching);
            // PinAI until the card's hudcomplete (UnpinAI), as Skate 3's party path.
            m.hold |= state.waiting_a;
            m.gestures = gestures(&m, &state);
        }
    }

    // HUD.
    for (id, say) in &state.news {
        if *id > run.said {
            local.push(say_text(data, row, *say, me, &state));
        }
    }
    run.said = state.said;
    for (text, central) in local {
        let from = run.messages.last().map_or(now, |(.., from)| (from + MESSAGE / 2.).max(now));
        run.messages.push((text, central, from));
    }
    run.messages.retain(|(.., from)| now < from + MESSAGE);
    let message = run.messages.iter().filter(|(.., from)| *from <= now).next_back();
    let round_no = i64::from(state.round.max(1));
    let countdown = match state.stage {
        Stage::Countdown { left } => Some((round_no, left.ceil().max(1.) as i64)),
        _ if now < run.go_until => Some((round_no, 0)),
        _ => None,
    };
    let timer = match state.stage {
        Stage::Play { left } if !state.overtime => Some(f64::from(left.max(0.))),
        _ => None,
    };
    let view = View {
        countdown,
        timer,
        result: message.map(|(m, central, _)| (m.clone(), run.title.clone(), *central)),
        ..default()
    };
    c.screens.sync(world, &view);
    let show = !matches!(phase, Phase::Intro { .. }) && !matches!(state.stage, Stage::Intro { .. });
    leaderboard(world, &mut run, row, &state, me, show);
    let voting = state.voting && state.active != me && matches!(state.stage, Stage::Play { .. } | Stage::Bailing);
    vote_screen(world, &mut run.vote_shown, voting.then_some(run.vote));
    dpo(world, &mut run, &state);
    let up = kind.contest() && round_screen_up(state.stage);
    round_screen(world, &mut run, &state, round.map_or(0, |r| r.variant), up);
    c.run = Some(run);
    if state.party && party_exchange(world, c, row, &state) {
        info!("Skate 2 contests: {:?} again", state.rules.kind);
        finish(world, c);
    }
}

/// Party Play (party_play::exchange), as Skate 3's party modes: the card at each turn's teleporttransitionfinished, the
/// pad on the player the last turn's fade switched to (at its black), the others on their TurnBasedWaitingLocation
/// slots (slot = player id), the results dialog once the match is done. Back come hudcomplete and, for Rematch /
/// Restart Challenge, true: the match again on this row (setup runs afresh next frame).
fn party_exchange(world: &mut World, c: &mut Contests, row: &Row, s: &Contest) -> bool {
    let Some(run) = c.run.as_mut() else { return false };
    let card = s.card().filter(|&(n, ..)| run.carded != n).map(|(n, player, round)| {
        run.carded = n;
        (player, round)
    });
    let controlled = run.watching.unwrap_or(s.active);
    let waiting = (0..s.players.len()).map(|p| row.waiting.as_ref().filter(|_| p != controlled).map(|w| w.slot(p))).collect();
    let results = (s.stage == Stage::Done).then(|| {
        // 1-Up ranks by rounds won (process_round's `order`, show_results' Results_SetPlayerData): `placing` has none.
        let placed: Vec<(usize, i64)> = if s.rules.kind == Kind::OneUp { s.order.iter().map(|&p| (p, s.score(p))).collect() } else { s.placing() };
        let rows: Vec<_> = (placed.into_iter().enumerate())
            .map(|(i, (p, score))| (p, s.players[p].name.clone(), i as i64 + 1, score))
            .collect();
        let (score, throwdown) = if s.rules.kind == Kind::OneUp { ("ID_COMMON_SCORE", 1) } else { ("ID_LEADERBOARD_POINTS_HEADER", 0) };
        crate::party_play::standings(&run.title, &rows, score, throwdown)
    });
    let answers = crate::party_play::exchange(world, crate::party_play::Turn { controlled, waiting, card, results });
    if answers.hud_complete
        && let Some(state) = c.state.as_mut()
    {
        state.hudcomplete();
    }
    answers.retry || answers.restart
}

/// The contest Lua's run_round restart rules for this player, in handler order, for what the physics ticks saw this
/// frame (`seen`: entered PathEndVolume while scoring, entered a RestartLastChanceVolume, landed a sequence, wipeout
/// over): TimedRunMulti / SoloRunCycle move startLocator on at the path's end (TimedRunMulti puts the skater there at
/// once); restart volumes re-arm restartAtStart there; a sequence landed after one clears it; a wipeout while it's armed
/// goes back to the start (startLocator's slot for those run types, else StartLocation itself: ponytail, this player's
/// slot). `n` is NumCompetitors.
fn restart_rules(run: &mut Run, round: &Round, me: usize, n: usize, seen: [bool; 4]) {
    let [reached_end, hit_restart, landed, recovered] = seen;
    let cycles = matches!(round.run_type, SOLO_RUN_CYCLE | TIMED_RUN_MULTI);
    if reached_end {
        if cycles {
            run.start_locator = (run.start_locator + 1) % n;
        }
        if !round.restart_volumes.is_empty() {
            (run.restart_at_start, run.restart_hit) = (true, false);
        }
        if round.run_type == TIMED_RUN_MULTI {
            run.restart_to = Some(run.start_locator);
        }
    }
    run.restart_hit |= hit_restart;
    if landed && run.restart_hit {
        (run.restart_at_start, run.restart_hit) = (false, false);
    }
    if recovered && run.restart_at_start {
        run.restart_hit = false;
        run.restart_to = Some(if cycles { run.start_locator } else { me });
    }
}

/// Pause.DisablePauseMenuAndBackpack as each mode's Lua has it: contests from initial and every end_round until the
/// next round's countdown ends (complete enables it); Spot Battle from initial to the first countdown's
/// teleporttransitionfinished, and from challenge_finished; 1-Up from make_hud to check_players.
fn pause_disabled(s: &Contest) -> bool {
    match s.rules.kind {
        Kind::BestTrick | Kind::Jam => !matches!(s.stage, Stage::Play { .. } | Stage::Done),
        Kind::SpotBattle => matches!(s.stage, Stage::Ending { .. }) || (!s.pause_ok && s.stage != Stage::Done),
        Kind::OneUp => s.stage == Stage::Ready,
    }
}

/// `Match::gestures` (Skate 3's rule, user 10-28, s3rules S2 item 5): the D-pad emotes go through this mode's own
/// waits (skate3_modes::gestures), not under a round's results screen. 1-Up too: the user kept its game, and emotes
/// while waiting are a detail of every mode.
fn gestures(m: &Match, s: &Contest) -> bool {
    crate::skate3_modes::gestures(m, round_screen_up(s.stage))
}

/// Whether this player is held still (AI.PinAI), and whose skater it watches (`watching`, switched at a turn's fade):
/// everyone rides a contest round's Play; in a turn-based kind only the attempter, from its UnpinAI to the next
/// reposition's PinAI: Spot Battle's challenge_running on through end_turn (it pins no one), 1-Up's from round_setup /
/// prepare_follower (UnpinAI at once, before the fade) on through leader_success / follower_1up (they pin NPCs only).
fn hold_and_watch(s: &Contest, me: usize, online: bool, watching: Option<usize>) -> (bool, Option<usize>) {
    let mine = s.active == me;
    let free = match s.rules.kind {
        Kind::BestTrick | Kind::Jam => matches!(s.stage, Stage::Play { .. }),
        Kind::SpotBattle => mine && matches!(s.stage, Stage::Play { .. } | Stage::Bailing | Stage::Pause { .. }),
        Kind::OneUp => {
            mine && matches!(s.stage, Stage::Move { turn: true, .. } | Stage::Pick { .. } | Stage::Play { .. } | Stage::Pause { .. })
        }
    };
    (!free, watching.filter(|&p| online && p != me))
}

/// The other players' wire items: a host takes each guest's own new events (an event about anyone else is dropped), a
/// guest takes the host's newer snapshots and keeps the other guests' events until a snapshot has them taken.
///
/// Not retail (user 10-12): retail's contest ran on every console; here one host runs it, and an event that reached
/// only a host who then left was lost: the contest waited on it for ever (a bail's Recovered; online audit 10-12).
fn receive(c: &mut Contests, players: &[String], items: Vec<(usize, String)>) {
    let host = players.iter().position(|n| !n.is_empty());
    for (from, text) in items {
        match serde_json::from_str::<Wire>(&text) {
            Ok(Wire::E(n, e)) if e.player() == from => match (c.net, c.state.as_mut()) {
                (Net::Host, Some(s)) => {
                    if s.take(from, n) {
                        c.inbox.push(e);
                    }
                }
                (Net::Guest, _) => c.pending.push((from, n, e)),
                _ => {}
            },
            Ok(Wire::S(seq, snap)) if c.net == Net::Guest && Some(from) == host && snap.sane() => {
                if c.heard.is_none_or(|(f, q)| f != from || seq > q) {
                    c.heard = Some((from, seq));
                    if let Some(s) = c.state.as_mut() {
                        s.restore(snap);
                        c.pending.retain(|(p, n, _)| s.taken.get(*p).is_none_or(|t| n > t));
                    }
                }
            }
            _ => {}
        }
    }
    crate::match_core::bound(&mut c.pending, "Skate 2 contest events");
}

/// A stage with its timer zeroed: what a snapshot changing means.
fn still(mut stage: Stage) -> Stage {
    if let Stage::Intro { left }
    | Stage::Countdown { left }
    | Stage::Pick { left }
    | Stage::Play { left }
    | Stage::Pause { left }
    | Stage::Results { left }
    | Stage::Move { left, .. }
    | Stage::Ending { left } = &mut stage
    {
        *left = 0.;
    }
    stage
}

/// This frame's contest online (`players`: the match's names, blank for a player who left): a guest sends its own
/// `events` and coasts; the host (or solo) applies the leavers, its own events and the guests', and ticks it. Returns
/// the wire items to send.
fn exchange(c: &mut Contests, players: &[String], events: Vec<Event>, now: f32, dt: f32) -> Vec<String> {
    match c.net {
        Net::Guest => {
            c.outbox.extend(events);
            if let Some(s) = c.state.as_mut() {
                s.coast(dt);
            }
        }
        _ => {
            let mut inbox = Vec::new();
            if let Some(s) = c.state.as_mut() {
                // Taking over from a host who left: the events it hadn't taken first.
                for (p, n, e) in take(&mut c.pending) {
                    if s.take(p, n) {
                        inbox.push(e);
                    }
                }
            }
            inbox.append(&mut c.inbox);
            if let Some(s) = c.state.as_mut() {
                // playerdisconnected: a slot whose player left (its name blanked by the core).
                let left = (0..s.players.len()).filter(|&p| players.get(p).is_some_and(String::is_empty) && !s.players[p].gone);
                let left: Vec<Event> = left.map(|player| Event::Left { player }).collect();
                for e in left.into_iter().chain(events).chain(inbox) {
                    s.apply(e);
                }
                if s.stage != Stage::Ready {
                    s.tick(dt);
                }
            }
        }
    }
    if c.net == Net::Solo { Vec::new() } else { send(c, now) }
}

/// A guest's events out; the host's snapshot out when it changed, or every HEARTBEAT.
fn send(c: &mut Contests, now: f32) -> Vec<String> {
    let mut wire = Vec::new();
    match (c.net, &c.state) {
        (Net::Guest, _) => {
            for e in std::mem::take(&mut c.outbox) {
                c.numbered += 1;
                c.pending.push((e.player(), c.numbered, e));
                wire.extend(serde_json::to_string(&Wire::E(c.numbered, e)).ok());
            }
        }
        (Net::Host, Some(s)) => {
            let snap = s.snap();
            let key = Snap { st: still(snap.st), ..snap.clone() };
            if c.sent.as_ref().is_none_or(|(last, at)| *last != key || now - at >= HEARTBEAT) {
                c.seq += 1;
                c.sent = Some((key, now));
                wire.extend(serde_json::to_string(&Wire::S(c.seq, snap)).ok());
            }
        }
        _ => {}
    }
    wire
}

/// What follows a stage change for this player.
#[allow(clippy::too_many_arguments)]
fn transition(world: &mut World, c: &mut Contests, row: &Row, run: &mut Run, s: &Contest, me: usize, now: f32) {
    let kind = s.rules.kind;
    let round = row.rounds.get(s.round.max(1) as usize - 1);
    let mine = s.active == me;
    let from_countdown = run.seen.is_some_and(|(d, ..)| d == discriminant(&Stage::Countdown { left: 0. }));
    if matches!(s.stage, Stage::Play { .. }) && from_countdown {
        run.go_until = now + 1.;
    }
    // A contest round's flythrough (round 1's is the match core's intro).
    if let Stage::Intro { .. } = s.stage {
        if let Some(scene) = round.and_then(|r| r.intro.as_deref()) {
            world.resource_mut::<Cutscene>().start(&c.assets.join(INTROS), scene);
            run.cutscene = true;
        }
    } else if take(&mut run.cutscene) {
        world.resource_mut::<Cutscene>().stop();
    }
    // RepositionAI: every console fades (the Lua runs on each), each moves only its own skater, at black; a turn's
    // attempter (bool6) takes the camera there.
    let (fade, to) = destination(row, run, s, me);
    if fade {
        let elapsed = if let Stage::Move { left, .. } = s.stage { MOVE - left } else { 0. };
        run.fade_from = Some(now - elapsed);
        run.switch_to = matches!(s.stage, Stage::Move { turn: true, .. }).then_some(s.active);
        run.moved = true;
    }
    if to.is_some() {
        run.teleport = to;
    }
    // Spot Battle after the last round: Online.SwitchCamera to the next player when the last attempter left (else
    // the camera is on that attempter already).
    if kind == Kind::SpotBattle && matches!(s.stage, Stage::Ending { .. }) {
        run.watching = Some(s.active);
    }
    if matches!(s.stage, Stage::Countdown { .. } | Stage::Pick { .. }) {
        run.vote = 1;
    }
    run.was_active = mine;
    // Skate 3's rule (user 10-28): each round's countdown sets the session marker where the skater stands at its round
    // start (S3 contest_shared.lua 880, SessionMarker.SetSessionMarker); session_marker places it once the pause gate
    // lifts with the round's start.
    if kind.contest()
        && matches!(s.stage, Stage::Countdown { .. })
        && let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>()
    {
        marker.request_set = true;
    }
    // setup_round: each round starts this player's run state afresh.
    if let (true, Stage::Intro { .. } | Stage::Countdown { .. }, Some(r)) = (kind.contest(), s.stage, round) {
        run.wrong = 0;
        (run.start_locator, run.restart_hit, run.restart_to) = (me, false, None);
        run.restart_at_start = r.restart_on_wipeout || !r.restart_volumes.is_empty();
    }

    let unused = || row.objects.iter().enumerate().filter(|(i, _)| !s.used.contains(i));
    let indicators: Vec<&Place> = match (kind, s.stage) {
        (_, Stage::Ready | Stage::Done) => vec![],
        (Kind::SpotBattle, Stage::Play { .. }) => row.indicators.iter().collect(),
        (Kind::BestTrick | Kind::Jam, Stage::Countdown { .. } | Stage::Play { .. }) => {
            round.map_or(vec![], |r| r.indicators.iter().collect())
        }
        (Kind::OneUp, _) => match s.object.and_then(|o| row.objects.get(o)) {
            Some(o) => o.indicators.iter().collect(),
            None => unused().flat_map(|(_, o)| &o.indicators).collect(),
        },
        _ => vec![],
    };
    world.resource_mut::<Arrows>().wanted = indicators.iter().map(|p| (p.locator.clone(), p.matrix)).collect();

    let p = &mut c.probe;
    p.spots = match kind {
        Kind::SpotBattle if mine => row.spot_volumes.clone(),
        Kind::BestTrick | Kind::Jam => round.map_or(vec![], |r| r.spot_volumes.clone()),
        _ => vec![],
    };
    p.boundary = round.and_then(|r| r.boundary).filter(|_| kind.contest());
    p.path_end = round.and_then(|r| r.path_end).filter(|_| kind.contest());
    p.restarts = round.filter(|_| kind.contest()).map_or(vec![], |r| r.restart_volumes.clone());
    p.start = row.start_volume.filter(|_| kind == Kind::OneUp);
    p.objects = match (kind, s.object) {
        (Kind::OneUp, Some(o)) => row.objects.get(o).map(|x| (o, x.volumes.clone())).into_iter().collect(),
        (Kind::OneUp, None) => unused().map(|(i, o)| (i, o.volumes.clone())).collect(),
        _ => vec![],
    };
}

/// What a stage change means for this console: whether a RepositionAI fade starts here, and where its own skater goes
/// (RepositionAI moves only the local skater).
fn destination(row: &Row, run: &Run, s: &Contest, me: usize) -> (bool, Option<[[f32; 4]; 4]>) {
    let kind = s.rules.kind;
    let mine = s.active == me;
    let waiting = || row.waiting.as_ref().map(|p| p.slot(me));
    // 1-Up's prepare_follower finding no connected follower: process_round at once, the last attempter's fade running on.
    let from_pause = run.seen.is_some_and(|(d, ..)| d == discriminant(&Stage::Pause { left: 0. }));
    let cached = kind == Kind::OneUp
        && from_pause
        && matches!(s.stage, Stage::Results { .. } | Stage::Ending { .. })
        && s.players.get(s.active).is_some_and(|p| !p.gone);
    match (kind, s.stage) {
        // countdown / round_setup / prepare_follower: the attempter to AttemptSpawn's slot 0 (1-Up: or an object's start).
        (Kind::SpotBattle, Stage::Move { turn: true, .. }) if mine => (true, row.attempt.as_ref().map(|p| p.slot(0))),
        (Kind::OneUp, Stage::Move { turn: true, .. }) if mine => (true, one_up_spawn(row, s)),
        // Spot Battle after the last round: the last attempter to the attempt spawn on its own slot.
        (Kind::SpotBattle, Stage::Move { turn: false, .. }) if mine => (true, row.attempt.as_ref().map(|p| p.slot(me))),
        // The last attempter back to its waiting slot; wait_for_players / check_players send everyone there first.
        (_, Stage::Move { .. }) if run.was_active || !run.moved => (true, waiting()),
        (_, Stage::Move { .. }) => (true, None),
        _ if cached => (true, waiting().filter(|_| mine)),
        // setup_round: the round's start on this player's slot, at once (fade 0).
        (Kind::BestTrick | Kind::Jam, Stage::Intro { .. } | Stage::Countdown { .. }) => {
            let round = row.rounds.get(s.round.max(1) as usize - 1);
            (false, round.and_then(|r| r.start.as_ref()).map(|p| p.slot(me)))
        }
        _ => (false, None),
    }
}

/// A rule's message, worded by the row's template (`{0:s}`: a name or the round).
fn say_text(data: &Data, row: &Row, say: Say, me: usize, s: &Contest) -> (String, Central) {
    let (field, fallback) = match say {
        Say::YourTurn(_) => ("CentralMessageHALIDYourTurn", "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN"),
        // Spot Battle's countdown shows the fixed id; contests their row's.
        Say::Round(_) if s.rules.kind == Kind::SpotBattle => ("", "ID_CONTEST_CENTRALMESSAGE_ROUNDNUMBER"),
        Say::Round(_) => ("CentralMessageHALIDRoundNum", "ID_CONTEST_CENTRALMESSAGE_ROUNDNUMBER"),
        Say::FinalRound => ("CentralMessageHALIDFinalRound", "ID_CONTEST_CENTRALMESSAGE_FINALROUND"),
        Say::SpotOwned => ("CentralMessageHALIDSpotOwned", "ID_CHALLENGE_CENTRAL_MESSAGE_SPOTOWNED"),
        Say::Complete => ("CentralMessageHALIDChallengeComplete", "ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE"),
        Say::NewLead(p) if p == me => ("CentralMessageHALIDNewLeadYou", "ID_CONTEST_CENTRALMESSAGE_NEWLEADYOU"),
        Say::NewLead(_) => ("CentralMessageHALIDNewLead", "ID_CONTEST_MODCENTRALMESSAGE_NEWLEAD"),
        Say::LeaderPick => ("CentralMessageHALIDLeaderPickObject", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_LEADERTURN"),
        Say::BeatHighScore => ("CentralMessageHALIDBeatHighScore", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_BEATHIGHSCORE"),
        Say::FollowerPick => ("CentralMessageHALIDFollowerPickObject", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_LEADERFAILED"),
        Say::NewScoreSet => ("CentralMessageHALIDNewScoreSet", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCOREDHIGH"),
        Say::FailedPick => ("CentralMessageHALIDFailedPickObject", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_FAILEDTOPICK"),
        Say::DidNotBeat => ("CentralMessageHALIDDidNotBeatHighScore", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCOREDLOW"),
        Say::OneUpped => ("ModifiedCentralMessageHALIDOneUpped", "ID_CHALLENGE_CENTRAL_MESSAGE_MODIFIED_1UP_ONEUPPED"),
        Say::ScoredLow => ("ModifiedCentralMessageHALIDScoredLow", "ID_CHALLENGE_CENTRAL_MESSAGE_MODIFIED_1UP_SCOREDLOW"),
        Say::Bailed => ("ModifiedCentralMessageHALIDBailed", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_BAILED"),
        Say::TimeUp => ("ModifiedCentralMessageHALIDTimeUp", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_TIMEUP"),
        Say::RoundOver => ("CentralMessageHALIDRoundOver", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ROUNDOVER"),
    };
    let text = data.say(row, field, fallback);
    let name = |p: usize| s.players.get(p).map_or("", |p| p.name.as_str()).to_owned();
    let text = match say {
        Say::YourTurn(p) | Say::NewLead(p) => text.replace("{0:s}", &name(p)),
        Say::Round(n) => text.replace("{0:s}", &n.to_string()),
        _ => text,
    };
    (text, if say == Say::Complete { Central::Complete } else { Central::Plain })
}

/// A Skate 2 / Skate 1 mode's RepositionAI move: travel to `at`; false while the engine's own (a wipeout's reset) or
/// the match core's is still to come. A skater on foot is moved like any other (match_core::teleport_blocked).
pub(crate) fn teleport(world: &mut World, at: [[f32; 4]; 4]) -> bool {
    if crate::match_core::teleport_blocked(world.resource::<SkaterRuntime>()) || world.resource::<Match>().teleporting() {
        return false;
    }
    if let Err(error) = world.resource_mut::<SkaterRuntime>().travel_to(at) {
        warn!("mode teleport: {error}");
    }
    true
}

/// RepositionAI's screen `t` seconds into its fade: out over half of it, black through FADE_HOLD, in over the other
/// half. ponytail: linear; the fader's curve is not traced.
pub(crate) fn reposition_black(t: f32) -> f32 {
    (t.min(MOVE - t) / (REPOSITION_FADE / 2.)).clamp(0., 1.)
}

/// LeaderboardHUDSetVariantType's names by eContestType: the image's Best Trick table; Jam's adds `_JAM` up to
/// Air_Time and has no Best Line (14) or Trick List (17).
const VARIANTS: [&str; 18] = [
    "SEQUENCE",
    "SEQUENCE_GRAB",
    "SEQUENCE_GRIND",
    "SEQUENCE_FLIP",
    "SEQUENCE_FOOTPLANT",
    "SEQUENCE_HANDPLANT",
    "SEQUENCE_HIPPYJUMP",
    "GRIND_LENGTH",
    "SPEED",
    "AIR_DISTANCE",
    "AIR_GRAB_TIME",
    "AIR_HEIGHT",
    "AIR_SPIN",
    "AIR_TIME",
    "AIR_LINE",
    "AIR_ONLY",
    "GRIND_ONLY",
    "TRICK_LIST",
];

/// The contest round's variant name (SetVariantType's bool: Best Trick's table, else Jam's).
fn variant_name(best_trick: bool, variant: u32) -> String {
    match (VARIANTS.get(variant as usize), best_trick, variant) {
        (Some(v), true, _) | (Some(v), false, 15 | 16) => format!("ID_CONTEST_VARIANT_{v}"),
        (Some(v), false, 0..=13) => format!("ID_CONTEST_VARIANT_{v}_JAM"),
        _ => "ID_COMMON_UNKNOWN".into(),
    }
}

/// A leaderboard score's text as the image formats it: RankNameFloatscore / Distance / FloatDistance one decimal in
/// metres (ID_COMMON_DISTANCE), the rest an int with ',' grouping.
pub(crate) fn score_text(layout: u8, score: f64) -> String {
    match layout {
        6 | 9 | 10 => format!("#{score:.1} m"),
        _ => format!("#{}", crate::core_menu_runtime::thousands(score)),
    }
}

/// LeaderboardHUD as each mode's Lua sets it up: LeaderboardHUDInit's layout, SetVariantType / SetVariantString,
/// SetPlayerHighlight and LeaderboardSetTurn (online without the sync: the core's standings).
fn leaderboard(world: &mut World, run: &mut Run, row: &Row, s: &Contest, me: usize, show: bool) {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let kind = s.rules.kind;
    let variant = row.rounds.get(s.round.max(1) as usize - 1).map_or(0, |r| r.variant);
    let (layout, name) = match kind {
        Kind::SpotBattle if s.rules.cumulative => (0, "ID_VARIANT_SESSIONSPOT".into()),
        Kind::SpotBattle => (0, "ID_COMMON_UNKNOWN".into()),
        Kind::OneUp => (1, "ID_COMMON_UNKNOWN".into()),
        // UseIntScore (Sequence, the trick-type ones, Air_Spin, Best Line, Air_Only, Grind_Only): RankNameScore, else
        // RankNameFloatscore. ponytail: Best_Line taken as 14 (the table's "Best Line"); no row uses it.
        _ => (if matches!(variant, 0..=6 | 12 | 14..=16) { 3 } else { 6 }, variant_name(kind == Kind::BestTrick, variant)),
    };
    let playing = !matches!(s.stage, Stage::Ready | Stage::Done);
    let (rows, turn): (Vec<_>, _) = if online && s.players.len() == 1 && !m.standings.is_empty() {
        let rows = m.standings.iter().map(|x| (x.name.clone(), x.score, score_text(layout, x.score as f64), x.local));
        (rows.collect(), None)
    } else {
        // 1-Up's throwdown strip is per player: roster order, the attempter's turn marked.
        let order: Vec<usize> = if kind == Kind::OneUp { (0..s.players.len()).collect() } else { s.order.clone() };
        // Spot Battle: Init highlights the local player; each countdown moves it from the last attempter to the next, and
        // the one after the last round only clears it.
        let ended = matches!(s.stage, Stage::Move { turn: false, .. } | Stage::Ending { .. });
        let rows = order.iter().map(|&i| {
            let lit = match kind {
                Kind::SpotBattle => (playing && !ended && i == s.active) || (i == me && s.round <= 1 && s.active <= me),
                _ => i == me,
            };
            let v = s.shown(i);
            (s.players[i].name.clone(), v.floor() as i64, score_text(layout, v), lit)
        });
        (rows.collect(), s.marker.filter(|_| kind == Kind::OneUp && playing))
    };
    let answers = leaderboard_answers(&format!("#{}", run.title), online, layout, &name, &rows, turn);
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    let layer = screens.layer(SKATE2_ONLINE);
    if !show {
        return;
    }
    // A new layout, row count or variant needs SetupScreen.
    let setup = |b: &[(&str, Json); 9]| [2, 3, 6].map(|k| b[k].1.clone());
    let reopen = layer.get(LEADERBOARD).is_none() || run.board.as_ref().is_none_or(|b| setup(b) != setup(&answers));
    if !reopen && run.board.as_ref() == Some(&answers) {
        return;
    }
    let result = (|| -> Result<(), String> {
        if reopen {
            let screen = layer.open(LEADERBOARD, "leaderboardhud", &answers)?;
            screen.call("SetState", &[json!(3)])?;
            screen.call("SetState", &[json!(1)]).map(drop)
        } else {
            refresh_leaderboard(layer.get(LEADERBOARD).ok_or("leaderboardhud closed")?, answers.clone())
        }
    })();
    if let Err(error) = result {
        warn!("Skate 2 contests: leaderboardhud: {error}");
        layer.close(LEADERBOARD);
    }
    run.board = Some(answers);
}

/// What leaderboardhud's SetupScreen / RefreshAll ask, for (name, score, score text, highlighted) rows in rank order:
/// `layout` an eLeaderBoardHUD_* (0 IntStringInt, 1 1up, 3 RankNameScore, 4 RankNameTime, 6 RankNameFloatscore,
/// 7 Skate, 11 RankName), `title` and `variant` as the movie localizes them (ID_COMMON_UNKNOWN hides the variant), a
/// score < 0 greys its row out, `turn` the row whose turn it is (LeaderboardSetTurn).
pub(crate) fn leaderboard_answers(
    title: &str,
    online: bool,
    layout: u8,
    variant: &str,
    rows: &[(String, i64, String, bool)],
    turn: Option<usize>,
) -> [(&'static str, Json); 9] {
    let n = rows.len();
    let block: Vec<Json> = (rows.iter().enumerate())
        .flat_map(|(i, (_, score, text, lit))| [json!(i + 1), json!(score), json!(text), json!(lit), json!(turn == Some(i))])
        .collect();
    // A player can name themselves a string id; names are shown as written.
    let names: Vec<Json> = rows.iter().map(|(n, ..)| json!(crate::apt_text::literal(n))).collect();
    [
        ("Mission.GetIsOnlineMission", json!(online)),
        ("Mission.GetTitle", json!(title)),
        ("HUDComponents.Leaderboard_GetLayoutType", json!(layout)),
        ("HUDComponents.Leaderboard_GetNumPlayers", json!(n)),
        ("HUDComponents.Leaderboard_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Leaderboard_GetLeaderboardDataBlock", json!(block)),
        ("HUDComponents.Leaderboard_GetLeaderboardVariantName", json!(variant)),
        // UpdateVOIPIcons' GetVoipStatus()[row] (by data row): voice::voip_status by the name it shows, online.
        ("FELeaderboard.GetVoipStatus", json!(rows.iter().map(|(name, ..)| if online { crate::voice::voip_named(name) } else { 0 }).collect::<Vec<_>>())),
        ("FELanguage.GetLocalizedFormatString(ID_COMMON_UNKNOWN,0)", json!("ID_COMMON_UNKNOWN")),
    ]
}

/// Refreshes an open leaderboardhud (SetState 5) with `answers` (leaderboard_answers'). RefreshAll names data row i
/// from the name block SetupScreen kept and boxes it by its rank, so the rows go in that kept order: a refresh after the
/// ranks change keeps each name on its own score. Setup itself goes in rank order, as the 'lobby' frame swap wants.
// ponytail: a player replaced at the same count keeps the kept name until SetupScreen; callers reopen on a count change.
pub(crate) fn refresh_leaderboard(screen: &mut crate::apt_screen::Screen, mut answers: [(&'static str, Json); 9]) -> Result<(), String> {
    let kept = screen.field("mNameArray");
    let at = |key: &str| answers.iter().position(|(k, _)| *k == key).unwrap();
    let (names_at, block_at) = (at("HUDComponents.Leaderboard_GetLeaderboardNameBlock"), at("HUDComponents.Leaderboard_GetLeaderboardDataBlock"));
    let voip_at = at("FELeaderboard.GetVoipStatus");
    let names = answers[names_at].1.as_array().cloned().unwrap_or_default();
    let block = answers[block_at].1.as_array().cloned().unwrap_or_default();
    let voip = answers[voip_at].1.as_array().cloned().unwrap_or_default();
    let mut order: Vec<usize> = Vec::new();
    for name in kept.as_array().into_iter().flatten() {
        if let Some(i) = (0..names.len()).find(|i| &names[*i] == name && !order.contains(i)) {
            order.push(i);
        }
    }
    let rest: Vec<usize> = (0..names.len()).filter(|i| !order.contains(i)).collect();
    order.extend(rest);
    answers[names_at].1 = order.iter().map(|&i| names[i].clone()).collect();
    answers[block_at].1 = order.iter().flat_map(|&i| block[i * 5..i * 5 + 5].to_vec()).collect();
    answers[voip_at].1 = order.iter().map(|&i| voip.get(i).cloned().unwrap_or(json!(0))).collect();
    for (key, value) in answers {
        screen.answer(key, value);
    }
    screen.call("SetState", &[json!(5)])?;
    // RefreshAll leaves the mics: UpdateVOIPIcons (6) redraws them (voice chat).
    screen.call("SetState", &[json!(6)]).map(drop)
}

/// The contests' Effects fades this frame (contest_jam.lua): initial's FadeOut 0.5 (7, started by `setup`) and
/// cleanup_round's FadeOut 0.1 (1494, as a round's results end) hold black through setup_round, whose teleport goes at
/// once (fade 0); a round's intro presentation shows through it (`nis`; as skate2_death_race's heat_black: whether the
/// FadeOut draws over the NIS isn't traced); the countdown starts with FadeIn 0.5 (presentationeventgroupfinished,
/// 666-671).
fn effects_black(run: &mut Run, stage: Stage, nis: bool, now: f32) -> f32 {
    let out = run.effect.is_some_and(|f| f.out);
    match stage {
        Stage::Results { left } if left <= ROUND_SETUP && !out => {
            run.effect = Some(Fade { out: true, from: now - (ROUND_SETUP - left), length: CLEANUP_FADE });
        }
        Stage::Countdown { .. } if out => run.effect = Some(Fade { out: false, from: now, length: CONTEST_FADE }),
        _ => {}
    }
    if nis { 0. } else { run.effect.map_or(0., |f| f.black(now)) }
}

/// results_screen online (contest_jam.lua 2133-2156): a round's results screen is up for its 6 s timer as the round's
/// Results begin, then closed for 1 s before cleanup_round.
fn round_screen_up(stage: Stage) -> bool {
    matches!(stage, Stage::Results { left } if left > ROUND_RESULTS - ROUND_SCREEN + ROUND_SETUP)
}

/// What roundresults asks between a contest's rounds: results_screen (contest_jam.lua 1962-2135) leaves its RoundResults
/// group active: ID_CHALLENGE_COMMON_ROUND_RESULTS over the round's scores best first (PointSorter; a tie keeps slot
/// order), placed 1..n, as BestTrickNonElimInt (5) or, for a round that isn't UseIntScore, BestTrickNonElimFloat (4)
/// with each cell's NumberToDistance (results_lb reads the int at [1], the float at [2]). Every online row's
/// ResultsScreenHALIDCol1-3 are ID_COMMON_RANK / NAME / SCORE (the vault, 10-28); `kind` is GetChallengeTypeName's.
fn round_results_answers(s: &Contest, variant: u32, title: &str, kind: &str) -> Vec<(String, Json)> {
    let float = !matches!(variant, 0..=6 | 12 | 14..=16);
    let mut order: Vec<usize> = (0..s.players.len()).collect();
    order.sort_by(|&a, &b| s.players[b].best.total_cmp(&s.players[a].best));
    let mut data = Vec::new();
    let mut distances = Vec::new();
    for (i, &p) in order.iter().enumerate() {
        let score = f64::from(s.players[p].best);
        let int = if float { -1 } else { score as i64 };
        data.extend([json!(i + 1), json!(int), json!(if float { score } else { 0. }), json!(""), json!("")]);
        if float {
            // Keyed as apt_screen keys a call's argument (a whole number as an integer).
            let key = if score.fract() == 0. && score.abs() < 9e15 { (score as i64).to_string() } else { json!(score).to_string() };
            distances.push((format!("HUDComponents.NumberToDistance({key})"), json!(score_text(6, score.max(0.)))));
        }
    }
    let names: Vec<String> = order.iter().map(|&p| crate::apt_text::literal(&s.players[p].name)).collect();
    let answers = [
        ("HUDComponents.Results_GetNumPlayers", json!(order.len())),
        ("HUDComponents.Results_GetLayoutType", json!(if float { 4 } else { 5 })),
        ("HUDComponents.Results_GetLeaderboardDataBlock", Json::Array(data)),
        ("HUDComponents.Results_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Results_GetLeaderboardMainTitle", json!("ID_CHALLENGE_COMMON_ROUND_RESULTS")),
        ("HUDComponents.Results_GetLeaderboardColTitle(0)", json!("ID_COMMON_RANK")),
        ("HUDComponents.Results_GetLeaderboardColTitle(1)", json!("ID_COMMON_NAME")),
        ("HUDComponents.Results_GetLeaderboardColTitle(2)", json!("ID_COMMON_SCORE")),
        ("HUDComponents.Results_GetIsNavigatable", json!(false)),
        ("Mission.GetTitle", json!(title)),
        ("Mission.GetChallengeTypeName", json!(kind)),
        ("Mission.GetDescription", json!("")),
    ];
    answers.into_iter().map(|(k, v)| (k.to_owned(), v)).chain(distances).collect()
}

/// The round's results screen on Skate 2's online HUD layer while `up` (results_screen's
/// OpenOnlineContinueRetryDialog(true) to its CloseOnlineContinueRetryDialog). Not retail: retail's is FE slot 116,
/// ranked/ranked_results_overall, not exported (online audit 10-28 GAP-1); this is the installed roundresults_lb
/// (EndChallengeOptions), the screen the match's end shows today.
fn round_screen(world: &mut World, run: &mut Run, s: &Contest, variant: u32, up: bool) {
    if up == run.round_shown {
        return;
    }
    run.round_shown = up;
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    let layer = screens.layer(SKATE2_ONLINE);
    if !up {
        layer.close(ROUND_RESULTS_SCREEN);
        return;
    }
    let kind = if s.rules.kind == Kind::Jam { "ID_MISSION_TEMPLATE_ONLINE_JAM" } else { "ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK" };
    let answers = round_results_answers(s, variant, &format!("#{}", run.title), kind);
    let answers: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    if let Err(error) = layer.open(ROUND_RESULTS_SCREEN, "roundresults_lb", &answers) {
        warn!("Skate 2 contests: round results: {error}");
    }
}

/// This player's vote (hudonlinevoting: 0 weak, 1 ok, 2 sweet, 3 gnarly) after this frame's press: A up, B down.
pub(crate) fn next_vote(pressed: u16, vote: u8) -> u8 {
    match pressed {
        b if b & PAD_A != 0 => (vote + 1).min(3),
        b if b & PAD_B != 0 => vote.saturating_sub(1),
        _ => vote,
    }
}

/// hudonlinevoting, installed as contests_voting (its construct clip events as placement values; old_contests.py
/// --voting): SetState 1 shows it, 2 hides it, 3-6 show the vote (weak / ok / sweet / gnarly). `want`: the vote to show,
/// None to hide it; `shown`: what it last showed.
pub(crate) fn vote_screen(world: &mut World, shown: &mut Option<u8>, want: Option<u8>) {
    if want == *shown {
        return;
    }
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    let layer = screens.layer(SKATE2_ONLINE);
    let result = (|| -> Result<(), String> {
        match want {
            Some(vote) => {
                if layer.get(VOTING).is_none() {
                    layer.open(VOTING, "contests_voting", &[])?;
                }
                let screen = layer.get(VOTING).ok_or("hudonlinevoting closed")?;
                if shown.is_none() {
                    screen.call("SetState", &[json!(1)])?;
                }
                screen.call("SetState", &[json!(3 + vote)]).map(drop)
            }
            None => layer.get(VOTING).map_or(Ok(()), |s| s.call("SetState", &[json!(2)]).map(drop)),
        }
    })();
    if let Err(error) = result {
        warn!("Skate 2 contests: hudonlinevoting: {error}");
        layer.close(VOTING);
    }
    *shown = want;
}

/// hud2/dpo for cumulative Spot Battle, as the mode's Lua drives it: challenge_running shows it with
/// DPOSetParams(0, the leader's Score) and DPOSetIcon(Challenge), each counted sequence sets (TurnScore, the leader's
/// Score), end_turn hides it; LeaderboardHUDInit's reset titles it ID_COMMON_SCORE.
fn dpo(world: &mut World, run: &mut Run, s: &Contest) {
    let on = s.rules.kind == Kind::SpotBattle && s.rules.cumulative && matches!(s.stage, Stage::Play { .. } | Stage::Bailing);
    let want = on.then(|| {
        let leader = s.order.first().map_or(0., |&i| s.players[i].best.max(0.));
        dpo_answers(s.players.len(), s.players.get(s.active).map_or(0., |p| p.turn), leader)
    });
    if want == run.dpo {
        return;
    }
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
    let layer = screens.layer(SKATE2_ONLINE);
    let result = (|| -> Result<(), String> {
        let Some(answers) = &want else {
            return layer.get(DPO).map_or(Ok(()), |s| s.call("SetState", &[json!(2)]).map(drop));
        };
        if layer.get(DPO).is_none() {
            layer.open(DPO, "dpo", answers)?;
        }
        let screen = layer.get(DPO).ok_or("dpo closed")?;
        for (key, value) in answers {
            screen.answer(key, value.clone());
        }
        screen.call("SetState", &[json!(3)])?;
        if run.dpo.is_none() {
            screen.call("SetState", &[json!(1)])?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        warn!("Skate 2 contests: dpo: {error}");
        layer.close(DPO);
    }
    run.dpo = want;
}

/// What dpo's Update asks: DPO_GetParams [numerator (the movie adds '#'), denominator, title, icon], icon 0 (Challenge)
/// taking GetActiveChallengeIconName's (Spot Battle's mini_spotfound -> dpoicons\challenge_spotfound).
// ponytail: Spot Battle sets no DPOSetFormatType; both numbers take the leaderboard's ',' grouping.
fn dpo_answers(players: usize, turn: f32, leader: f32) -> [(&'static str, Json); 3] {
    let int = |v: f32| crate::core_menu_runtime::thousands(f64::from(v).floor());
    [
        ("HUDComponents.Leaderboard_GetNumPlayers", json!(players)),
        ("HUDComponents.DPO_GetParams", json!([int(turn), format!("#{}", int(leader)), "ID_COMMON_SCORE", 0])),
        ("Game.GetActiveChallengeIconName", json!("mini_spotfound")),
    ]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Voice chat: leaderboardhud's GetVoipStatus is each row's voice::voip_status online (UpdateVOIPIcons lights a
    /// talker's row), none offline.
    #[test]
    fn a_talker_lights_skate2s_boards() {
        crate::voice::hear("Voip S2 Talker", 9301, 2);
        crate::voice::hear("Voip S2 Quiet", 9302, 1);
        let rows: Vec<_> = ["Voip S2 Talker", "Voip S2 Quiet", "Voip S2 Nobody"].iter().map(|n| (n.to_string(), 0, String::new(), false)).collect();
        let voip = |online| {
            let answers = leaderboard_answers("#t", online, 11, "ID_COMMON_UNKNOWN", &rows, None);
            answers.iter().find(|(k, _)| *k == "FELeaderboard.GetVoipStatus").unwrap().1.clone()
        };
        assert_eq!(voip(true), json!([2, 1, 0]));
        assert_eq!(voip(false), json!([0, 0, 0]));
    }

    #[test]
    fn leaderboard_names_are_never_looked_up_as_string_ids() {
        let rows = [("ID_ONLINE_KICKED_BY_GAME_HOST".to_owned(), 0, String::new(), false)];
        let answers = leaderboard_answers("#t", true, 11, "ID_COMMON_UNKNOWN", &rows, None);
        let names = &answers.iter().find(|(k, _)| *k == "HUDComponents.Leaderboard_GetLeaderboardNameBlock").unwrap().1;
        assert_eq!(names[0], json!(crate::apt_text::literal("ID_ONLINE_KICKED_BY_GAME_HOST")));
    }

    /// The match core's results order (`match_core::standings`) for per-slot results, as the Skate 2 modes' wire tests
    /// (online audit 10-12) read it: `None` for a player who left (the core drops a blank name). Slots, best first.
    pub(crate) fn placed(results: &[Option<(i64, bool)>], lower_wins: bool) -> Vec<usize> {
        use skate_net::roster::{Game, Score};
        let game = Game { token: 1, mode: String::new(), slots: (0..results.len() as u64).collect(), results: false, teams: vec![] };
        let names = (results.iter().enumerate()).filter(|(_, r)| r.is_some()).map(|(a, _)| (a as u64, format!("{a}"))).collect();
        let scores = (results.iter().enumerate())
            .filter_map(|(a, r)| r.map(|(value, finished)| (a as u64, Score { token: 1, value, finished, ..default() })))
            .collect();
        let standings = crate::match_core::standings(&game, &scores, &names, 0, lower_wins);
        standings.iter().map(|s| s.name.parse().unwrap()).collect()
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("P{i}")).collect()
    }
    fn scored(player: usize, points: f32) -> Event {
        Event::Scored { player, points, object: None }
    }
    fn on(player: usize, points: f32, object: Option<usize>) -> Event {
        Event::Scored { player, points, object }
    }
    fn said(c: &Contest) -> Vec<Say> {
        c.news.iter().map(|(_, s)| *s).collect()
    }

    /// Online: the host runs the contest; a guest's event goes over the wire to it, the host's snapshot comes back,
    /// and the guest then shows what the host has (its own names and rules kept); 10 players fit one record.
    #[test]
    fn a_guest_mirrors_the_hosts_contest_over_the_wire() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 2, time_limit: 20., sequences_to_end_turn: 2, ..default() };
        let mut host = Contest::new(rules.clone(), names(10), true);
        let mut guest = Contest::new(rules, names(10), true);
        host.start(0.);
        host.tick(MOVE);
        host.tick(COUNTDOWN);
        let wire = serde_json::to_string(&Wire::E(1, scored(0, 512.5))).unwrap();
        let Wire::E(1, e) = serde_json::from_str(&wire).unwrap() else { panic!() };
        assert_eq!(e.player(), 0, "the host checks it against the sending slot");
        host.apply(e);
        host.apply(Event::Vote { player: 3, level: 3 });
        let wire = serde_json::to_string(&Wire::S(7, host.snap())).unwrap();
        assert!(wire.len() < 700, "a snapshot of 10 players is {} bytes; an @ev record holds 1024", wire.len());
        let Wire::S(7, snap) = serde_json::from_str(&wire).unwrap() else { panic!() };
        guest.restore(snap);
        assert_eq!(guest, host);
        guest.coast(5.);
        assert_eq!(guest.stage, Stage::Play { left: 15. }, "a guest's timers run down between snapshots");
        assert_eq!((guest.round, guest.active), (1, 0), "and it changes nothing else");
    }

    /// A host's snapshot naming a player that isn't there is dropped (the leaderboard indexed `players` with it).
    #[test]
    fn a_snapshot_with_a_bad_player_index_is_dropped() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 2, ..default() };
        let host = Contest::new(rules.clone(), names(2), true);
        let mut c = Contests { net: Net::Guest, state: Some(Contest::new(rules, names(2), true)), ..default() };
        let snap = Snap { o: vec![0, 1, 99], ..host.snap() };
        receive(&mut c, &names(2), vec![(0, serde_json::to_string(&Wire::S(1, snap)).unwrap())]);
        assert_eq!(c.state.unwrap().placing().len(), 2);
    }

    /// PinAI / UnpinAI as the Lua has them (f_1a_01_ots_pj01_books.lua 7, 443-471, 748-766; ol_1up_01_svm_mountainup.lua
    /// 196-216, 471-490, 549-590, 619-633): the Spot Battle attempter rides from challenge_running through end_turn,
    /// the 1-Up attempter from its UnpinAI (before the fade) through leader_success / follower_1up.
    #[test]
    fn only_the_attempter_rides_and_the_others_watch_it() {
        let rules = |kind| Rules { kind, rounds: 1, time_limit: 20., num_to_win: 2, objects: 2, ..default() };
        let mut c = Contest::new(rules(Kind::SpotBattle), names(3), true);
        c.start(0.);
        assert_eq!(hold_and_watch(&c, 0, true, None), (true, None), "the attempter is held through its fade");
        assert_eq!(hold_and_watch(&c, 1, true, None), (true, None), "the camera switches at the fade's black");
        assert_eq!(hold_and_watch(&c, 1, true, Some(0)), (true, Some(0)), "waiting, watching the attempter");
        c.tick(MOVE);
        assert!(hold_and_watch(&c, 0, true, Some(0)).0, "and its countdown");
        c.tick(COUNTDOWN);
        assert_eq!(hold_and_watch(&c, 0, true, Some(0)), (false, None), "the attempter rides");
        assert_eq!(hold_and_watch(&c, 2, true, Some(0)), (true, Some(0)));
        assert_eq!(hold_and_watch(&c, 2, false, Some(0)).1, None, "solo has no one to watch");
        c.tick(20.);
        assert_eq!(hold_and_watch(&c, 0, true, Some(0)), (false, None), "time up: the turn runs on (Skate 3's rule)");
        c.tick(END_RUN);
        assert_eq!(hold_and_watch(&c, 0, true, Some(0)), (false, None), "end_turn pins no one");
        c.tick(END_TURN);
        assert_eq!((c.active, hold_and_watch(&c, 0, true, Some(0)).0), (1, true), "the next countdown pins the last one");

        let mut one_up = Contest::new(rules(Kind::OneUp), names(2), true);
        one_up.start(0.);
        assert!(!hold_and_watch(&one_up, 0, true, None).0, "round_setup's UnpinAI comes before the fade");
        one_up.tick(MOVE);
        one_up.apply(Event::Started { player: 0 });
        one_up.apply(on(0, 900., Some(1)));
        assert_eq!((one_up.stage, hold_and_watch(&one_up, 0, true, None).0), (Stage::Pause { left: AFTER_ATTEMPT }, false));
        one_up.tick(AFTER_ATTEMPT);
        assert_eq!((one_up.active, hold_and_watch(&one_up, 0, true, None).0), (1, true), "prepare_follower pins the last");
        assert!(!hold_and_watch(&one_up, 1, true, None).0);

        let mut jam = Contest::new(rules(Kind::Jam), names(3), true);
        jam.start(3.);
        assert_eq!(hold_and_watch(&jam, 2, true, None), (true, None), "a round's countdown holds everyone");
        jam.stage = Stage::Play { left: 10. };
        assert_eq!(hold_and_watch(&jam, 2, true, None), (false, None), "everyone rides a contest round");
        jam.stage = Stage::Results { left: 7. };
        assert!(hold_and_watch(&jam, 2, true, None).0, "and waits through its results");
    }

    /// RepositionAI's fade (R4) in Spot Battle's countdown (f_1a_01_ots_pj01_books.lua 395-518, 825-848): the turn's
    /// messages, pause menu and countdown wait for teleporttransitionfinished; after the last round the last attempter is
    /// moved once more and challenge_finished follows that fade; one who left ends it at once, the camera on the next.
    #[test]
    fn spot_battle_turns_wait_for_their_reposition_fade() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let mut c = Contest::new(rules.clone(), names(2), true);
        c.start(0.);
        assert_eq!((c.stage, c.active, said(&c), pause_disabled(&c)), (Stage::Move { left: MOVE, turn: true }, 0, vec![], true));
        c.tick(MOVE - 0.5);
        assert!(said(&c).is_empty() && pause_disabled(&c));
        c.tick(0.5);
        assert_eq!((c.stage, said(&c), pause_disabled(&c)), (Stage::Countdown { left: COUNTDOWN }, vec![Say::YourTurn(0), Say::Round(1)], false));
        c.tick(COUNTDOWN);
        c.apply(scored(0, 500.));
        c.tick(END_TURN);
        assert_eq!((c.stage, c.active), (Stage::Move { left: MOVE, turn: true }, 1));
        c.tick(MOVE);
        assert_eq!(said(&c).last(), Some(&Say::YourTurn(1)), "no round message mid-round");
        c.tick(COUNTDOWN);
        c.tick(20.);
        c.tick(END_TURN);
        assert_eq!((c.stage, c.active), (Stage::Move { left: MOVE, turn: false }, 1), "the last attempter, once more");
        assert!(!pause_disabled(&c));
        c.tick(MOVE);
        assert_eq!((c.stage, said(&c).last()), (Stage::Ending { left: SPOT_BATTLE_ENDING }, Some(&Say::Complete)));
        assert!(pause_disabled(&c));

        let mut c = Contest::new(rules, names(3), true);
        c.start(0.);
        for t in [MOVE, COUNTDOWN, 20., END_RUN, END_TURN, MOVE, COUNTDOWN, 20., END_RUN, END_TURN, MOVE, COUNTDOWN] {
            c.tick(t);
        }
        c.tick(20.);
        assert_eq!((c.active, c.stage), (2, Stage::Pause { left: END_TURN }));
        c.apply(Event::Left { player: 2 });
        c.tick(END_TURN);
        assert_eq!((c.active, c.stage), (0, Stage::Ending { left: SPOT_BATTLE_ENDING }), "SwitchCamera(next), finished");
    }

    #[test]
    fn spot_battle_turns_rounds_ownership_and_bails() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 2, time_limit: 20., sequences_to_end_turn: 2, ..default() };
        let mut c = Contest::new(rules, names(2), false);
        c.start(0.);
        c.tick(MOVE);
        assert_eq!(said(&c), [Say::YourTurn(0), Say::Round(1)]);
        c.tick(COUNTDOWN);
        assert_eq!(c.stage, Stage::Play { left: 20. });
        c.apply(scored(1, 900.));
        assert_eq!(c.players[1].best, 0., "not the attempter's");
        c.apply(scored(0, 500.));
        assert_eq!(c.stage, Stage::Play { left: 20. }, "one sequence of two");
        c.apply(scored(0, 300.));
        assert_eq!(c.players[0].best, 500.);
        assert_eq!(c.stage, Stage::Pause { left: END_TURN }, "the second sequence ends the turn");
        assert_eq!(said(&c).iter().filter(|s| **s == Say::SpotOwned).count(), 1);
        c.tick(END_TURN);
        c.tick(MOVE);
        assert_eq!((c.active, c.round, c.stage), (1, 1, Stage::Countdown { left: COUNTDOWN }));
        c.tick(COUNTDOWN);
        c.apply(scored(1, 800.));
        assert_eq!(c.order, [1, 0]);
        c.apply(Event::Bailed { player: 1 });
        c.tick(100.);
        assert_eq!(c.stage, Stage::Bailing, "the timer stops while bailing");
        c.apply(Event::Recovered { player: 1 });
        c.tick(END_TURN_WIPED);
        assert_eq!((c.active, c.round), (0, 2));
        c.tick(MOVE);
        assert!(said(&c).ends_with(&[Say::YourTurn(0), Say::Round(2)]));
        // P0's time-up runs on END_RUN (Skate 3's rule); P1's is the match's last run (LastRun): end_turn at once.
        for t in [COUNTDOWN, 20., END_RUN, END_TURN, MOVE, COUNTDOWN, 20., END_TURN, MOVE] {
            c.tick(t);
        }
        // challenge_finished: "Challenge Complete" for 2 s, the pause menu off, then the results.
        assert_eq!(c.stage, Stage::Ending { left: SPOT_BATTLE_ENDING });
        assert_eq!(said(&c).last(), Some(&Say::Complete));
        assert!(pause_disabled(&c) && hold_and_watch(&c, 0, true, Some(1)) == (true, Some(1)));
        c.tick(SPOT_BATTLE_ENDING);
        assert_eq!(c.stage, Stage::Done);
        assert_eq!((c.score(0), c.score(1)), (500, 800));

        // Cumulative: the turn's sequences add up and don't end it.
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 1, cumulative: true, ..default() };
        let mut c = Contest::new(rules, names(1), false);
        c.start(0.);
        c.tick(MOVE);
        c.tick(COUNTDOWN);
        c.apply(scored(0, 300.));
        c.apply(scored(0, 400.));
        assert_eq!((c.players[0].best, c.stage), (700., Stage::Play { left: 20. }));
    }

    /// Online results (Match::order), Best Trick: Ann ties Bo's 900 after him, so Bo keeps the lead; Cy leaves and is
    /// listed last at -1. 1-Up gives no order (the core's).
    #[test]
    fn results_keep_the_earlier_scorer_and_list_a_leaver_last() {
        let rules = Rules { kind: Kind::BestTrick, rounds: 1, round_times: vec![60.], cumulative_min: vec![0.], intros: vec![0.], ..default() };
        let mut c = Contest::new(rules, names(3), false);
        c.start(3.);
        c.tick(3.);
        c.apply(scored(1, 900.));
        c.apply(scored(0, 900.));
        c.apply(scored(2, 100.));
        c.apply(Event::Left { player: 2 });
        c.tick(60.);
        assert_eq!(c.placing(), [(1, 2), (0, 2), (2, -1)]);
        let one_up = Contest::new(Rules { kind: Kind::OneUp, ..default() }, names(2), false);
        assert_eq!(one_up.placing(), Vec::<(usize, i64)>::new());
    }

    #[test]
    fn contest_rounds_award_points_with_shared_ties() {
        let rules = Rules {
            kind: Kind::BestTrick,
            rounds: 2,
            round_times: vec![60., 60.],
            cumulative_min: vec![0., 0.],
            intros: vec![0., 5.],
            ..default()
        };
        let mut c = Contest::new(rules, names(3), false);
        c.start(3.);
        c.tick(3.);
        c.apply(scored(2, 100.));
        assert_eq!((c.order[0], said(&c).last()), (2, Some(&Say::NewLead(2))), "first points take the lead");
        c.apply(scored(0, 900.));
        c.apply(scored(1, 900.));
        c.apply(scored(0, 400.));
        assert_eq!(c.players[0].best, 900., "a lower sequence doesn't replace the best");
        assert_eq!(c.order, [0, 1, 2], "a tie doesn't pass");
        c.tick(60.);
        assert_eq!(c.players.iter().map(|p| p.total).collect::<Vec<_>>(), [2, 2, 0]);
        assert_eq!(c.stage, Stage::Results { left: ROUND_RESULTS + ROUND_SETUP });
        assert!(pause_disabled(&c), "end_round disables the pause menu");
        c.tick(ROUND_RESULTS + ROUND_SETUP);
        assert_eq!((c.stage, said(&c).last()), (Stage::Intro { left: 5. }, Some(&Say::FinalRound)));
        c.tick(5.);
        c.tick(COUNTDOWN);
        assert_eq!(c.stage, Stage::Play { left: 60. });
        c.apply(Event::Left { player: 1 });
        c.apply(scored(2, 50.));
        c.tick(60.);
        assert_eq!(c.players.iter().map(|p| p.total).collect::<Vec<_>>(), [3, 2, 2], "the gone player scores nothing");
        assert_eq!((c.stage, c.order[0], c.order[2]), (Stage::Done, 0, 1));

        // Jam: the round sums the sequences over the minimum; solo, the result is the round scores.
        let rules = Rules { kind: Kind::Jam, rounds: 1, cumulative: true, round_times: vec![120.], cumulative_min: vec![100.], intros: vec![0.], ..default() };
        let mut c = Contest::new(rules, names(1), false);
        c.start(0.);
        c.tick(0.);
        for points in [50., 200., 300.] {
            c.apply(scored(0, points));
        }
        c.tick(120.);
        assert_eq!((c.stage, c.score(0)), (Stage::Done, 500), "a contest ends on its last round's results, no message");
        assert!(!said(&c).contains(&Say::Complete) && !said(&c).iter().any(|s| matches!(s, Say::Round(_) | Say::FinalRound)),
            "one round: no round message (ContestRounds.Count ~= 1)");
        assert!(!pause_disabled(&c), "complete enables the pause menu");
    }

    #[test]
    fn one_up_leader_sets_followers_beat_and_rounds_win() {
        let rules = Rules { kind: Kind::OneUp, time_limit: 20., num_to_win: 2, objects: 2, ..default() };
        let mut c = Contest::new(rules, names(2), false);
        c.start(0.);
        assert_eq!((c.stage, c.active, said(&c), c.marker), (Stage::Move { left: MOVE, turn: true }, 0, vec![], None));
        c.tick(MOVE);
        assert_eq!((c.stage, c.active, said(&c), c.marker), (Stage::Pick { left: PICK }, 0, vec![Say::LeaderPick], Some(0)));
        c.apply(Event::Started { player: 0 });
        assert_eq!(c.stage, Stage::Play { left: 20. });
        c.apply(on(0, 800., None));
        assert_eq!(c.stage, Stage::Play { left: 20. }, "not on an object");
        c.apply(on(0, 1000., Some(0)));
        assert_eq!((c.object, c.to_beat, c.used.clone()), (Some(0), 1000., vec![0]));
        assert_eq!(said(&c).last(), Some(&Say::NewScoreSet));
        c.tick(AFTER_ATTEMPT);
        assert_eq!((c.stage, c.active, c.follower, c.marker), (Stage::Move { left: MOVE, turn: true }, 1, Some(0), Some(0)));
        c.tick(MOVE);
        assert_eq!((said(&c).last(), c.marker), (Some(&Say::BeatHighScore), Some(1)), "wait_for_follower, after the fade");
        c.tick(PICK);
        assert_eq!(c.stage, Stage::Play { left: 20. }, "the pick time ran out");
        c.apply(on(1, 5000., Some(1)));
        assert_eq!(c.stage, Stage::Play { left: 20. }, "the other object doesn't count");
        c.apply(on(1, 1500., Some(0)));
        assert_eq!((c.leader, c.oneupped), (1, true));
        c.tick(AFTER_ATTEMPT);
        assert_eq!(c.stage, Stage::Move { left: MOVE, turn: false }, "the last follower back first");
        c.tick(MOVE);
        assert_eq!((c.players[1].total, c.order.clone(), c.stage), (1, vec![1, 0], Stage::Results { left: ROUND_OVER }));
        c.tick(ROUND_OVER);
        assert_eq!((c.round, c.active, c.follower, c.followers.clone()), (2, 1, None, vec![0]));
        c.tick(MOVE);
        // Time up mid-air: overtime until the sequence ends; on a used object it doesn't count.
        c.apply(Event::Started { player: 1 });
        c.apply(Event::Busy { player: 1, on: true });
        c.tick(20.);
        assert!(c.overtime && matches!(c.stage, Stage::Play { .. }));
        c.apply(on(1, 700., Some(0)));
        assert_eq!((c.stage, said(&c).last()), (Stage::Pause { left: AFTER_ATTEMPT }, Some(&Say::FailedPick)));
        c.tick(AFTER_ATTEMPT);
        c.tick(MOVE);
        assert_eq!((c.active, said(&c).last()), (0, Some(&Say::FollowerPick)));
        c.tick(PICK);
        c.apply(Event::Bailed { player: 0 });
        assert!(said(&c).ends_with(&[Say::Bailed, Say::DidNotBeat]));
        c.tick(AFTER_ATTEMPT);
        c.tick(MOVE);
        assert_eq!((c.stage, c.score(1), c.score(0)), (Stage::Ending { left: ONE_UP_ENDING }, 2, 0), "the leader kept the round");
        assert_eq!(said(&c).last(), Some(&Say::Complete), "show_results: Challenge Complete for 3 s");
        c.tick(ONE_UP_ENDING);
        assert_eq!(c.stage, Stage::Done);
    }

    /// c_*_besttrick / jam run_round (s2lua c_1a_01_besttrick_street.lua 931-1002, 1157-1171, 1268-1299): out of
    /// bounds scores 0; Best Trick (only) needs a spot; the trick-type check; airs and grinds score per air / grind.
    #[test]
    fn contest_variants_score_as_the_lua_gates_them() {
        let l = Landed { score: 1000., types: 1 << 1 | 1 << 4, hit: true, inside: true, ..default() };
        assert_eq!(sequence_points(true, 0, &l), Some((1000., false)));
        assert_eq!(sequence_points(true, 0, &Landed { hit: false, ..l }), Some((0., false)), "Best Trick: no spot, no points");
        assert_eq!(sequence_points(false, 0, &Landed { hit: false, ..l }), Some((1000., false)), "a Jam needs no spot");
        assert_eq!(sequence_points(false, 0, &Landed { inside: false, ..l }), Some((0., false)), "out of bounds");
        assert_eq!(sequence_points(true, 1, &l), Some((1000., false)), "Sequence_Grab: a grab in it");
        assert_eq!(sequence_points(true, 2, &l), Some((0., true)), "Sequence_Grind without one: wrong trick");
        assert_eq!(sequence_points(true, 2, &Landed { hit: false, ..l }), Some((0., false)), "the spot check comes first");
        assert_eq!(sequence_points(false, 15, &l), Some((1000., false)), "Air_Only: the scorer left the other types out");
        assert_eq!(sequence_points(false, 16, &Landed { types: 1 << 5, ..l }), Some((1000., false)), "Grind_Only");
        assert_eq!(sequence_points(false, 12, &l), None, "Air_Spin scores airs");
        let air = Measured { measures: [3.5, 0., 1.2, 540., 0.9], inside: true, hit: false, used: false };
        assert_eq!(measured_points(false, 12, &air, false), Some(540.), "Jam Air_Spin: the air's degrees");
        assert_eq!(measured_points(true, 12, &air, false), Some(0.), "Best Trick airs need the spot");
        assert_eq!(measured_points(false, 9, &air, false), Some(3.5));
        assert_eq!(measured_points(false, 12, &Measured { inside: false, ..air }, false), Some(0.));
        let grind = Measured { measures: [7.6, 0., 0., 0., 0.], inside: true, hit: false, used: false };
        assert_eq!(measured_points(true, 7, &grind, true), Some(7.6), "Grind_Length: metres, no spot needed");
        assert_eq!((measured_points(true, 7, &air, false), measured_points(true, 12, &grind, true)), (None, None));
        // NumWrongTricksToWarn = 3; points clear the count.
        let mut n = 0;
        assert_eq!([count_wrong(&mut n, true, 0.), count_wrong(&mut n, true, 0.), count_wrong(&mut n, true, 0.)], [false, false, true]);
        count_wrong(&mut n, true, 0.);
        count_wrong(&mut n, false, 10.);
        assert_eq!((n, count_wrong(&mut n, true, 0.)), (0, false));
    }

    /// run_round's restart rules (c_1a_01_besttrick_street.lua 489-500, 994-1000, 1072-1116, 1238-1251): TimedRunMulti
    /// (c_3a_01_jam_mega_ol) cycles the start slot at the path's end, restart volumes arm and a landed sequence after
    /// one disarms the wipeout restart.
    #[test]
    fn timed_run_multi_cycles_start_slots_and_restart_volumes_arm() {
        let b = Aabb { min: [0.; 3], max: [1.; 3] };
        let round = Round { run_type: TIMED_RUN_MULTI, path_end: Some(b), restart_volumes: vec![b], ..default() };
        let mut run = Run { start_locator: 2, restart_at_start: true, ..default() };
        restart_rules(&mut run, &round, 2, 3, [false, false, false, true]);
        assert_eq!(run.restart_to.take(), Some(2), "a wipeout before any restart volume: back to this slot");
        restart_rules(&mut run, &round, 2, 3, [false, true, false, false]);
        restart_rules(&mut run, &round, 2, 3, [false, false, true, false]);
        assert!(!run.restart_at_start, "a sequence landed after the restart volume");
        restart_rules(&mut run, &round, 2, 3, [false, false, false, true]);
        assert_eq!(run.restart_to, None, "so a wipeout stays put");
        restart_rules(&mut run, &round, 2, 3, [true, false, false, false]);
        assert_eq!((run.start_locator, run.restart_to.take(), run.restart_at_start), (0, Some(0), true), "path end: next slot, wrapping");
        // RestartOnWipeout without restart volumes (ol_jam_03_svm_newmega round 3); an ordinary round goes back to
        // StartLocation (this player's slot).
        let plain = Round { restart_on_wipeout: true, ..default() };
        let mut run = Run { start_locator: 1, restart_at_start: true, ..default() };
        restart_rules(&mut run, &plain, 1, 3, [true, false, false, true]);
        assert_eq!((run.start_locator, run.restart_to), (1, Some(1)), "no cycling outside TimedRunMulti / SoloRunCycle");
    }

    /// Spot Battle playerdisconnected (f_1a_01_ots_pj01_books.lua 203-214, 520-531, 748-766): the attempter leaving
    /// ends the turn with end_turn's 2 s (0.01 s when wiped out). 1-Up (ol_1up_01_svm_mountainup.lua 281-286, 435-457,
    /// 710-723, 912-917): an attempter who left gets a 1 s attempt, then time up.
    #[test]
    fn leavers_end_turns_and_attempts_as_the_lua_does() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 2, ..default() };
        let mut c = Contest::new(rules.clone(), names(3), true);
        c.start(0.);
        c.apply(Event::Left { player: 0 });
        assert_eq!(c.stage, Stage::Pause { left: END_TURN }, "in its countdown's fade");
        let mut c = Contest::new(rules.clone(), names(3), true);
        c.start(0.);
        c.tick(MOVE);
        c.tick(COUNTDOWN);
        c.apply(Event::Left { player: 0 });
        assert_eq!(c.stage, Stage::Pause { left: END_TURN }, "mid-turn");
        let mut c = Contest::new(rules, names(3), true);
        c.start(0.);
        c.tick(MOVE);
        c.tick(COUNTDOWN);
        c.apply(Event::Bailed { player: 0 });
        c.apply(Event::Left { player: 0 });
        assert_eq!(c.stage, Stage::Pause { left: END_TURN_WIPED }, "wiped out");

        let rules = Rules { kind: Kind::OneUp, time_limit: 20., num_to_win: 2, objects: 3, ..default() };
        let mut c = Contest::new(rules.clone(), names(2), true);
        c.start(0.);
        c.tick(MOVE);
        c.apply(Event::Started { player: 0 });
        c.apply(Event::Left { player: 0 });
        assert_eq!(c.stage, Stage::Play { left: GONE_ATTEMPT }, "ResetCountdownTimer(AttemptTimerID, 1)");
        c.tick(GONE_ATTEMPT);
        assert!(said(&c).ends_with(&[Say::TimeUp, Say::FailedPick]));
        // A leader gone before its turn: wait_for_leader's 5 s (after check_players' fade), then a 1 s attempt; a later
        // round_setup moves no one for it, so no fade to wait for.
        let mut c = Contest::new(rules, names(3), true);
        c.apply(Event::Left { player: 0 });
        c.start(0.);
        c.tick(MOVE);
        assert_eq!((c.active, c.stage), (0, Stage::Pick { left: PICK }));
        c.tick(PICK);
        assert_eq!(c.stage, Stage::Play { left: GONE_ATTEMPT });
        c.round_setup(false);
        assert_eq!(c.stage, Stage::Pick { left: PICK });
        // prepare_follower with the rest of the followers gone: process_round at once.
        c.leader = 1;
        c.round_setup(false);
        c.tick(MOVE);
        c.apply(Event::Left { player: 2 });
        c.tick(PICK);
        c.tick(20.);
        c.tick(AFTER_ATTEMPT);
        assert_eq!(c.stage, Stage::Results { left: ROUND_OVER });
    }

    /// D2 / D4: every console fades and moves only its own skater (RepositionAI): the attempter to AttemptSpawn's
    /// child 0 (slot 0), the others to their own waiting slot, the last attempter after the last round to its own
    /// AttemptSpawn slot; the fade's screen.
    #[test]
    fn each_console_moves_its_own_skater_in_the_fade() {
        let m = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let place = |x: f32| Place { locator: String::new(), matrix: m(x), slots: (1..=4).map(|k| m(x + k as f32)).collect() };
        let row = Row { attempt: Some(place(100.)), waiting: Some(place(200.)), ..default() };
        let at = |run: &Run, c: &Contest, me| destination(&row, run, c, me);
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let mut c = Contest::new(rules, names(3), true);
        c.start(0.);
        assert_eq!(at(&Run::default(), &c, 0), (true, Some(m(101.))), "the attempter: AttemptSpawn's child 0");
        assert_eq!(at(&Run::default(), &c, 2), (true, Some(m(203.))), "the others: their own waiting slot");
        for t in [MOVE, COUNTDOWN, 20., END_RUN, END_TURN] {
            c.tick(t);
        }
        assert_eq!(at(&Run { was_active: true, moved: true, ..default() }, &c, 0), (true, Some(m(201.))), "back to wait");
        assert_eq!(at(&Run { moved: true, ..default() }, &c, 2), (true, None), "a waiting player only fades");
        for t in [MOVE, COUNTDOWN, 20., END_RUN, END_TURN, MOVE, COUNTDOWN, 20., END_TURN] {
            c.tick(t);
        }
        assert_eq!(c.stage, Stage::Move { left: MOVE, turn: false });
        assert_eq!(at(&Run { was_active: true, moved: true, ..default() }, &c, 2), (true, Some(m(103.))));

        let rules = Rules { kind: Kind::OneUp, time_limit: 20., num_to_win: 3, objects: 2, ..default() };
        let mut u = Contest::new(rules, names(3), true);
        u.start(0.);
        assert_eq!(at(&Run::default(), &u, 0), (true, Some(m(101.))), "1-Up's leader: AttemptSpawn's child 0");
        for t in [MOVE, PICK, 20.] {
            u.tick(t);
        }
        u.apply(Event::Left { player: 1 });
        u.apply(Event::Left { player: 2 });
        u.tick(AFTER_ATTEMPT);
        assert_eq!(u.stage, Stage::Results { left: ROUND_OVER });
        let seen = Some((discriminant(&Stage::Pause { left: 0. }), 1, 0, None));
        let run = Run { seen, was_active: true, moved: true, ..default() };
        assert_eq!(at(&run, &u, 0), (true, Some(m(201.))), "process_round at once, the leader's fade running on");

        let black = [0., 0.25, 0.5, 1.5, 1.75, MOVE].map(reposition_black);
        assert_eq!(black, [0., 0.5, 1., 1., 0.5, 0.], "out over half, black through the hold, in over half");
    }

    /// M6: an out-of-bounds / restart send-back (RepositionAI(local, StartLocation): the default 1 s fade) moves at its
    /// black, not at once (contest_jam.lua 919, 1240-1249); Spot Battle's wipeout FadeOut holds black until the next
    /// RepositionAI's black.
    #[test]
    fn send_backs_move_at_the_black_of_their_fade() {
        let mut run = Run { teleport: Some(Default::default()), fade_from: Some(10.), ..default() };
        assert!(!run.at_black(10.4), "still fading out");
        assert_eq!(run.black(None, 10.25), 0.5);
        assert!(run.at_black(10. + REPOSITION_FADE / 2.), "the move at black");
        assert_eq!(run.black(None, 10. + MOVE), 0.);
        assert_eq!(run.fade_from, None, "the fade is over");
        let mut run = Run::default();
        assert_eq!(run.black(Some((0.5, 1.)), 20.), 0.5);
        assert_eq!(run.black(None, 21.), 1., "held");
        run.fade_from = Some(22.);
        assert!(!run.at_black(22.25));
        assert_eq!(run.black(None, 22.25), 1.);
        assert!(run.at_black(22.5) && !run.dark);
        assert_eq!(run.black(None, 22.5), 1., "the RepositionAI's own black");
        assert_eq!(run.black(None, 22. + MOVE), 0.);
    }

    /// SK8NET09 (the contest snapshot's Move stage and fields): an SK8NET08 peer's datagram is refused at the
    /// envelope, never decoded as a 09 one.
    #[test]
    fn an_sk8net08_datagram_is_refused_not_misread() {
        use skate_net::packed::{BODY, HEADER, envelope, header};
        let mut frame = header(1, 2, BODY, 3);
        frame.resize(HEADER, 0);
        assert_eq!(envelope(&frame), Some((1, 2, BODY, 3)));
        frame[..8].copy_from_slice(b"SK8NET08");
        assert_eq!(envelope(&frame), None);
    }

    /// Pause.DisablePauseMenuAndBackpack per mode's Lua (see pause_disabled): Spot Battle's first EnablePauseMenu is
    /// its first countdown's teleporttransitionfinished; 1-Up's check_players enables it before round_setup's fade.
    #[test]
    fn pause_menu_follows_each_modes_lua() {
        let rules = |kind| Rules { kind, rounds: 2, time_limit: 20., round_times: vec![60.; 2], ..default() };
        for (kind, at_start, after) in [(Kind::BestTrick, true, 3.), (Kind::SpotBattle, true, MOVE), (Kind::OneUp, false, MOVE)] {
            let mut c = Contest::new(rules(kind), names(3), true);
            assert!(pause_disabled(&c), "{kind:?} before the start");
            c.start(3.);
            assert_eq!(pause_disabled(&c), at_start, "{kind:?} at the start");
            c.tick(after);
            assert!(!pause_disabled(&c), "{kind:?} after it");
        }
    }

    #[test]
    fn voting_takes_the_voters_mean_and_keeps_the_best_over_one() {
        assert_eq!(tally(&[1, 3, 2, 0], 0, |i| i == 3), 2.5);
        assert_eq!(tally(&[3], 0, |_| false), 0., "no voters");
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        assert!(!Contest::new(rules.clone(), names(2), true).voting, "3+ players only");
        assert!(!Contest::new(rules.clone(), names(3), false).voting, "online only");
        let mut c = Contest::new(rules, names(3), true);
        c.start(0.);
        c.tick(MOVE);
        c.tick(COUNTDOWN);
        c.apply(Event::Vote { player: 1, level: 3 });
        c.apply(Event::Vote { player: 2, level: 2 });
        c.apply(Event::Vote { player: 0, level: 0 });
        assert_eq!(c.votes, [1, 3, 2], "the attempter doesn't vote");
        c.tick(20.);
        c.tick(END_RUN);
        c.tick(END_TURN);
        assert_eq!((c.best_voted, c.votes.clone()), (Some((0, 2.5)), vec![1, 1, 1]));
        for t in [MOVE, COUNTDOWN, 20., END_RUN, END_TURN] {
            c.tick(t);
        }
        assert_eq!(c.best_voted, Some((0, 2.5)), "a mean of 1 is no Player's Choice");
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 3): a Spot Battle's first player is drawn from the match token, the
    /// same on every console (as skate3_uniform's draw); 1-Up keeps its slot 0 (user 10-28: kept as it is).
    #[test]
    fn every_console_draws_the_same_first_spot_battle_player() {
        let rows = [("sb", "spot_battle"), ("up", "one_up")].map(|(id, mode)| (id.to_owned(), Row { mode: mode.into(), ..default() }));
        let data = Data { rows: rows.into(), strings: HashMap::new(), intros: HashMap::new(), intro_root: PathBuf::new() };
        for (row, kind, first) in [("sb", Kind::SpotBattle, 1), ("up", Kind::OneUp, 0)] {
            for slot in [0, 2] {
                let mut world = World::new();
                world.insert_resource(Match { row: row.into(), token: 7, slot, players: names(3), ..default() });
                world.init_resource::<Arrows>();
                let mut c = Contests { names: names(3), ..default() };
                assert!(setup(&mut world, &mut c, &data, kind, 0.));
                assert_eq!(c.state.as_ref().unwrap().first, first, "{row} on console {slot}");
            }
        }
    }

    /// Skate 3's rule (user 10-28): turns start from the drawn first player and every round starts again from it (S3
    /// spot_battle.lua 143-144, 263).
    #[test]
    fn spot_battle_rounds_start_from_the_drawn_first_player() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 2, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let mut c = Contest::new(rules, names(3), true);
        c.first = 2;
        c.start(0.);
        let mut turns = vec![(c.round, c.active)];
        for _ in 0..5 {
            c.tick(MOVE);
            c.tick(COUNTDOWN);
            c.apply(scored(c.active, 100.));
            c.tick(END_TURN);
            turns.push((c.round, c.active));
        }
        assert_eq!(turns, [(1, 2), (1, 0), (1, 1), (2, 2), (2, 0), (2, 1)]);
        // The drawn player gone: the next one still here starts, and the round still turns over at the drawn slot.
        let mut c = Contest::new(Rules { kind: Kind::SpotBattle, rounds: 2, time_limit: 20., sequences_to_end_turn: 1, ..default() }, names(3), true);
        c.first = 2;
        c.apply(Event::Left { player: 2 });
        c.start(0.);
        assert_eq!((c.round, c.active), (1, 0));
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 6): time up, a Spot Battle turn runs on 2 s more (EndRunTimerID; the
    /// timer hidden) and a sequence or a bail in them still counts; the match's last run (LastRun) ends at once (S3
    /// spot_battle.lua 901-912).
    #[test]
    fn a_trick_just_after_the_buzzer_still_counts() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let mut c = Contest::new(rules.clone(), names(2), true);
        c.start(0.);
        c.tick(MOVE);
        c.tick(COUNTDOWN);
        c.tick(20.);
        assert_eq!((c.stage, c.overtime), (Stage::Play { left: END_RUN }, true), "time up: 2 s more");
        c.apply(Event::Busy { player: 0, on: false });
        assert_eq!(c.stage, Stage::Play { left: END_RUN }, "not 1-Up's overtime");
        c.apply(scored(0, 700.));
        assert_eq!((c.players[0].best, c.stage), (700., Stage::Pause { left: END_TURN }), "a sequence landed in them scores");
        c.tick(END_TURN);
        c.tick(MOVE);
        c.tick(COUNTDOWN);
        assert_eq!((c.active, c.overtime), (1, false), "each turn on its own clock");
        c.tick(20.);
        assert_eq!(c.stage, Stage::Pause { left: END_TURN }, "the match's last run: end_turn at once");
        // A bail in the 2 s ends the turn as a bail; with nothing done, they run out into end_turn.
        let mut c = Contest::new(rules.clone(), names(2), true);
        c.start(0.);
        for t in [MOVE, COUNTDOWN, 20.] {
            c.tick(t);
        }
        c.apply(Event::Bailed { player: 0 });
        assert_eq!(c.stage, Stage::Bailing);
        let mut c = Contest::new(rules, names(2), true);
        c.start(0.);
        for t in [MOVE, COUNTDOWN, 20., END_RUN] {
            c.tick(t);
        }
        assert_eq!(c.stage, Stage::Pause { left: END_TURN });
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 5): the D-pad emotes go through a held player's waits, not under a
    /// round's results screen; 1-Up's waiting players too.
    #[test]
    fn waiting_players_can_emote_in_every_contest() {
        let running = || Match { hold: true, phase: Phase::Running { started: 0., ends: None }, ..default() };
        let rules = |kind| Rules { kind, rounds: 2, time_limit: 20., round_times: vec![60.; 2], ..default() };
        let mut sb = Contest::new(rules(Kind::SpotBattle), names(3), true);
        sb.start(0.);
        assert!(gestures(&running(), &sb), "held through another's turn");
        assert!(!gestures(&Match { hold: false, ..running() }, &sb), "riding");
        let mut bt = Contest::new(rules(Kind::BestTrick), names(3), true);
        bt.stage = Stage::Countdown { left: 3. };
        assert!(gestures(&running(), &bt), "a round's countdown");
        bt.stage = Stage::Results { left: ROUND_RESULTS + ROUND_SETUP };
        assert!(!gestures(&running(), &bt), "the round's results screen owns the pad");
        let mut one_up = Contest::new(rules(Kind::OneUp), names(3), true);
        one_up.start(0.);
        assert!(gestures(&running(), &one_up), "1-Up: a held waiting player");
    }

    /// Skate 3's rule (user 10-28, s3rules S2 item 8): each Best Trick / Jam round's countdown sets the session marker
    /// (S3 contest_shared.lua 880); a Spot Battle's turns don't.
    #[test]
    fn each_contest_rounds_countdown_sets_the_session_marker() {
        use crate::session_marker::SessionMarker;
        let mut world = World::new();
        world.init_resource::<Arrows>();
        world.init_resource::<SessionMarker>();
        let (row, mut c) = (Row::default(), Contests::default());
        let rules = Rules { kind: Kind::Jam, rounds: 2, round_times: vec![60.; 2], cumulative_min: vec![0.; 2], intros: vec![0.; 2], ..default() };
        let mut s = Contest::new(rules, names(2), true);
        let mut set = |world: &mut World, s: &Contest| {
            transition(world, &mut c, &row, &mut Run::default(), s, 0, 0.);
            std::mem::take(&mut world.resource_mut::<SessionMarker>().request_set)
        };
        s.start(3.);
        assert!(set(&mut world, &s), "round 1's countdown");
        s.tick(3.);
        assert!(!set(&mut world, &s), "not while it runs");
        s.tick(60.);
        s.tick(ROUND_RESULTS + ROUND_SETUP);
        assert!(matches!(s.stage, Stage::Countdown { .. }));
        assert!(set(&mut world, &s), "round 2's countdown");
        let mut sb = Contest::new(Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., ..default() }, names(2), true);
        sb.start(0.);
        sb.tick(MOVE);
        assert!(matches!(sb.stage, Stage::Countdown { .. }) && !set(&mut world, &sb), "Spot Battle blocks the markers");
    }

    /// GAP-2 (online audit 10-28): between a contest's rounds results_screen shows the round's results for its 6 s,
    /// closes for 1 s, then cleanup_round / setup_round (contest_jam.lua 1962-2156): ID_CHALLENGE_COMMON_ROUND_RESULTS
    /// over the round's scores best first, placed 1..n; a float round (Grind_Length) as distances.
    #[test]
    fn a_contest_round_ends_on_its_results_screen() {
        let rules = Rules { kind: Kind::BestTrick, rounds: 2, round_times: vec![60.; 2], cumulative_min: vec![0.; 2], intros: vec![0.; 2], ..default() };
        let mut c = Contest::new(rules, names(3), true);
        c.start(3.);
        c.tick(3.);
        c.apply(scored(0, 300.));
        c.apply(scored(1, 1200.));
        assert!(!round_screen_up(c.stage), "while the round runs");
        c.tick(60.);
        assert!(round_screen_up(c.stage), "end_round goes straight to results_screen (no highlight reel here)");
        let get = |answers: &[(String, Json)], k: &str| answers.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        let answers = round_results_answers(&c, 0, "#Books", "ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK");
        assert_eq!(get(&answers, "HUDComponents.Results_GetLeaderboardMainTitle"), Some(json!("ID_CHALLENGE_COMMON_ROUND_RESULTS")));
        assert_eq!(get(&answers, "HUDComponents.Results_GetLayoutType"), Some(json!(5)));
        assert_eq!(get(&answers, "HUDComponents.Results_GetLeaderboardNameBlock"), Some(json!(["P1", "P0", "P2"].map(crate::apt_text::literal))));
        let data = get(&answers, "HUDComponents.Results_GetLeaderboardDataBlock").unwrap();
        assert_eq!([&data[0], &data[1], &data[5], &data[6], &data[10], &data[11]], [&json!(1), &json!(1200), &json!(2), &json!(300), &json!(3), &json!(0)]);
        c.tick(ROUND_SCREEN - 0.01);
        assert!(round_screen_up(c.stage), "its 6 s");
        c.tick(0.02);
        assert!(!round_screen_up(c.stage) && matches!(c.stage, Stage::Results { .. }), "closed 1 s before cleanup_round");
        // A float round: BestTrickNonElimFloat, the float cell and its NumberToDistance.
        c.players[1].best = 12.5;
        let answers = round_results_answers(&c, 7, "#Books", "ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK");
        assert_eq!(get(&answers, "HUDComponents.Results_GetLayoutType"), Some(json!(4)));
        let data = get(&answers, "HUDComponents.Results_GetLeaderboardDataBlock").unwrap();
        assert_eq!([&data[1], &data[2], &data[7]], [&json!(-1), &json!(300.), &json!(12.5)]);
        assert_eq!(get(&answers, "HUDComponents.NumberToDistance(12.5)"), Some(json!("#12.5 m")));
        assert_eq!(get(&answers, "HUDComponents.NumberToDistance(300)"), Some(json!("#300.0 m")), "a whole number keys as an integer");
    }

    /// GAP-10 (online audit 10-28): a contest's Effects fades (contest_jam.lua 7, 666-671, 1494): initial's FadeOut 0.5
    /// and cleanup_round's FadeOut 0.1 hold black through setup_round, a round's intro presentation shows through it,
    /// and each countdown starts with FadeIn 0.5.
    #[test]
    fn contests_fade_out_to_each_round_and_in_with_its_countdown() {
        // Round 1: `setup` starts initial's FadeOut with the match core's intro.
        let mut run = Run { effect: Some(Fade { out: true, from: 0., length: CONTEST_FADE }), ..default() };
        assert_eq!(effects_black(&mut run, Stage::Ready, false, 0.25), 0.5, "initial's FadeOut");
        assert_eq!(effects_black(&mut run, Stage::Ready, false, 0.75), 1., "black through setup_round's load delay");
        assert_eq!(effects_black(&mut run, Stage::Ready, true, 1.), 0., "round 1's intro shows through it");
        assert_eq!(effects_black(&mut run, Stage::Ready, false, 9.), 1., "black again at its end");
        assert_eq!(effects_black(&mut run, Stage::Countdown { left: 3. }, false, 9.), 1., "the countdown starts black...");
        assert_eq!(effects_black(&mut run, Stage::Countdown { left: 2.75 }, false, 9.25), 0.5, "...and fades in over 0.5 s");
        assert_eq!(effects_black(&mut run, Stage::Play { left: 60. }, false, 10.), 0.);
        // Between rounds: the results screen over the round, then cleanup_round's FadeOut 0.1 with 0.6 s left.
        assert_eq!(effects_black(&mut run, Stage::Results { left: ROUND_RESULTS + ROUND_SETUP }, false, 70.), 0.);
        assert_eq!(effects_black(&mut run, Stage::Results { left: 1. }, false, 76.6), 0.);
        let half = effects_black(&mut run, Stage::Results { left: ROUND_SETUP - CLEANUP_FADE / 2. }, false, 77.25);
        assert!((half - 0.5).abs() < 1e-3, "{half}");
        assert_eq!(effects_black(&mut run, Stage::Results { left: 0.2 }, false, 77.6), 1., "black through setup_round");
        assert_eq!(effects_black(&mut run, Stage::Intro { left: 5. }, true, 77.8), 0., "the round's intro");
        assert_eq!(effects_black(&mut run, Stage::Countdown { left: 3. }, false, 82.8), 1., "its countdown starts black");
    }

    /// The installed export (SKATE3_ASSET_ROOT): every row loads with what its mode needs, and an intro camera.
    #[test]
    fn installed_contests_json_loads_every_mode() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let data = Data::load(Path::new(&assets)).unwrap();
        let mut modes: HashMap<&str, usize> = HashMap::new();
        let mut no_intro = Vec::new();
        for (id, row) in &data.rows {
            let kind = row.kind().unwrap_or_else(|| panic!("{id}: {}", row.mode));
            *modes.entry(row.mode.as_str()).or_default() += 1;
            let rules = row.rules(kind, &data);
            match kind {
                Kind::SpotBattle => assert!(
                    row.attempt.is_some() && row.waiting.is_some() && !row.spot_volumes.is_empty() && rules.rounds == 3,
                    "{id}"
                ),
                Kind::OneUp => {
                    assert!(row.objects.len() >= 3 && row.start_volume.is_some() && rules.num_to_win == 3, "{id}");
                    assert!(row.objects.iter().all(|o| !o.volumes.is_empty() && o.start.is_some()), "{id}");
                }
                _ => assert!(row.rounds.iter().all(|r| r.start.is_some() && r.time > 0.) && rules.rounds > 0, "{id}"),
            }
            assert!(time_cap(&rules, 8) > 60., "{id}");
            if data.intro(row.intro()).is_none() {
                no_intro.push(id.clone());
            }
        }
        assert_eq!(modes, HashMap::from([("spot_battle", 33), ("best_trick", 10), ("jam", 11), ("one_up", 10)]));
        assert!(no_intro.len() <= 1, "no intro camera: {no_intro:?}");
    }

    /// The installed leaderboardhud and hudonlinevoting run with every native they ask answered (SKATE3_ASSET_ROOT).
    #[test]
    fn installed_online_screens_have_every_native_answered() {
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let root = PathBuf::from(assets).join("private/hud-skate2/online");
        let runtime = root.join("runtime");
        let shots = std::env::var("SKATE_ONLINE_HUD_SHOTS").ok().map(PathBuf::from);
        // Each mode's layout: Spot Battle (Session Spot), 1-Up's throwdown (Danny's turn), Best Trick, a Jam on grind
        // length.
        let modes = [
            ("spot_battle", 0, "ID_VARIANT_SESSIONSPOT".to_owned(), [1200., 900.], None),
            ("one_up", 1, "ID_COMMON_UNKNOWN".to_owned(), [1500., 0.], Some(1)),
            ("best_trick", 3, variant_name(true, 1), [12345., 900.], None),
            ("jam", 6, variant_name(false, 7), [8.04, 3.5], None),
        ];
        for (mode, layout, variant, scores, turn) in modes {
            let rows: Vec<_> = (["Ryan", "Danny"].iter().zip(scores))
                .enumerate()
                .map(|(i, (name, v))| (name.to_string(), v as i64, score_text(layout, v), i == 0))
                .collect();
            let answers = leaderboard_answers("#Books", true, layout, &variant, &rows, turn);
            let mut screen = crate::apt_screen::Screen::load(&runtime.join("leaderboardhud.json"), &answers).unwrap();
            for state in [3, 1, 5] {
                screen.call("SetState", &[json!(state)]).unwrap();
            }
            for _ in 0..60 {
                screen.advance().unwrap();
            }
            let draws = screen.draws().unwrap();
            assert!(!draws.is_empty(), "{mode}");
            assert!(screen.bindings.unanswered.is_empty(), "leaderboardhud asks {:?}", screen.bindings.unanswered);
            if let Some(out) = &shots {
                std::fs::create_dir_all(out).unwrap();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join(format!("leaderboard_{mode}.png")));
            }
        }
        assert_eq!((score_text(3, 12345.), score_text(6, 8.04)), ("#12,345".to_owned(), "#8.0 m".to_owned()));
        assert_eq!(variant_name(false, 14), "ID_COMMON_UNKNOWN", "Jam has no Best Line");
        assert_eq!(variant_name(false, 15), "ID_CONTEST_VARIANT_AIR_ONLY");
        let mut screen = crate::apt_screen::Screen::load(&runtime.join("contests_voting.json"), &[]).unwrap();
        for state in [1, 6] {
            screen.call("SetState", &[json!(state)]).unwrap();
        }
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        assert!(!screen.draws().unwrap().is_empty());
        assert!(screen.bindings.unanswered.is_empty(), "hudonlinevoting asks {:?}", screen.bindings.unanswered);
        // Spot Battle's DPO: this turn's 2,500 against the leader's 12,345, the spot icon in its image slot.
        let mut screen = crate::apt_screen::Screen::load(&runtime.join("dpo.json"), &dpo_answers(2, 2500., 12345.)).unwrap();
        for state in [3, 1] {
            screen.call("SetState", &[json!(state)]).unwrap();
        }
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        let draws = screen.draws().unwrap();
        assert!(!draws.is_empty());
        assert!(screen.bindings.unanswered.is_empty(), "dpo asks {:?}", screen.bindings.unanswered);
        let images: Vec<_> = screen.bindings.movie.images.keys().filter(|k| !k.contains(':')).cloned().collect();
        assert_eq!(images, ["1"], "the challenge_spotfound icon loads; asked {:?}", screen.take_calls());
        if let Some(out) = &shots {
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join("dpo.png"));
        }
    }

    // ------------------------------------------------ six games over the wire (online audit 10-12)
    const SIX: usize = 6;
    type Key = (Discriminant<Stage>, u32, usize, Option<usize>, usize);

    /// Six games running one contest over a laggy, lossy wire (150-300 ms, 8 % loss), each on its own clock, as `step`
    /// runs it: the first player still here runs the contest (`exchange`), the others mirror its snapshots (`receive`).
    /// `act(mirror, me)` is what a game's own skater does in the stage its mirror shows: events, each at seconds into
    /// the stage. `leaver` leaves when `leave(mirror, seconds into its stage)` holds on its own game: what it still had
    /// on the way is lost, and each other game hears of it after its own delay. Returns each game's contest (None: the
    /// leaver's) once every game still there is done.
    fn contest_over_the_wire(
        rules: Rules,
        seed: u64,
        leaver: usize,
        leave: impl Fn(&Contest, f64) -> bool,
        act: impl Fn(&Contest, usize) -> Vec<(f64, Event)>,
    ) -> Vec<Option<Contest>> {
        let mut wire = crate::skate3_modes::tests::Wire::new(seed, (0.15, 0.3), 0.08);
        let offsets: Vec<f64> = (0..SIX).map(|_| 0.3 * wire.draw()).collect();
        let mut games: Vec<(Contests, Option<(Key, f64)>, usize)> = (0..SIX)
            .map(|_| (Contests { state: Some(Contest::new(rules.clone(), names(SIX), true)), names: names(SIX), ..default() }, None, 0))
            .collect();
        let (mut left, mut heard) = (false, [f64::INFINITY; SIX]);
        for frame in 0..60 * 3600 {
            let t = f64::from(frame) / 60.;
            for (i, (c, seen, fired)) in games.iter_mut().enumerate() {
                if (i == leaver && left) || t < offsets[i] {
                    continue;
                }
                let now = (t - offsets[i]) as f32;
                let players: Vec<String> = (0..SIX).map(|p| if p == leaver && heard[i] <= t { String::new() } else { format!("P{p}") }).collect();
                c.net = if players.iter().position(|n| !n.is_empty()) == Some(i) { Net::Host } else { Net::Guest };
                receive(c, &players, wire.take(t, i));
                let s = c.state.as_mut().unwrap();
                if s.stage == Stage::Ready {
                    s.start(if s.rules.kind.contest() { COUNTDOWN } else { 0. });
                }
                let key = (discriminant(&s.stage), s.round, s.active, s.follower, s.leader);
                if seen.is_none_or(|(k, _)| k != key) {
                    (*seen, *fired) = (Some((key, t)), 0);
                }
                let into = t - seen.unwrap().1;
                let mine = act(s, i);
                let events: Vec<Event> = mine.iter().skip(*fired).take_while(|(at, _)| into >= *at).map(|(_, e)| *e).collect();
                *fired += events.len();
                let out = exchange(c, &players, events, now, 1. / 60.);
                wire.send(t, i, SIX, out);
                if i == leaver && leave(c.state.as_ref().unwrap(), into) {
                    left = true;
                    for to in 0..SIX {
                        wire.cut(leaver, to);
                        heard[to] = t + 0.15 + 0.15 * wire.draw();
                    }
                }
            }
            if games.iter().enumerate().all(|(i, (c, ..))| (i == leaver && left) || c.state.as_ref().unwrap().stage == Stage::Done) {
                break;
            }
        }
        assert!(left, "the leaver never left");
        games.into_iter().enumerate().map(|(i, (c, ..))| (i != leaver).then(|| c.state.unwrap())).collect()
    }

    /// Every game still there is done and shows the same standings: what its players see, (slot, score) best first.
    /// That is the contest's own order online (`placing` -> `Match::order`, a player who left last at -1); for 1-Up,
    /// which gives none, the core's (`placed`, each game's own `score`; the leaver's blank name drops it).
    fn contest_agreed(games: &[Option<Contest>]) -> Vec<(usize, i64)> {
        let view = |c: &Contest| (c.stage, c.placing(), c.order.clone(), c.players.iter().map(|p| (p.best, p.total, p.gone)).collect::<Vec<_>>());
        let first = view(games.iter().flatten().next().unwrap());
        for c in games.iter().flatten() {
            assert_eq!(view(c), first, "the games disagree");
        }
        assert_eq!(first.0, Stage::Done, "not done");
        if !first.1.is_empty() {
            return first.1;
        }
        let results: Vec<_> = games.iter().enumerate().map(|(p, c)| c.as_ref().map(|c| (c.score(p), true))).collect();
        placed(&results, false).into_iter().map(|p| (p, results[p].unwrap().0)).collect()
    }

    /// Spot Battle's attempter: one sequence a second into its turn (`points(me, round)`; None: it bails, and its
    /// wipeout is over two seconds after the host has it bailing).
    fn spot_battle_act(points: impl Fn(usize, u32) -> Option<f32>) -> impl Fn(&Contest, usize) -> Vec<(f64, Event)> {
        move |s, me| match s.stage {
            _ if s.active != me => vec![],
            Stage::Play { .. } => match points(me, s.round) {
                Some(points) => vec![(1., Event::Scored { player: me, points, object: None })],
                None => vec![(1., Event::Bailed { player: me })],
            },
            Stage::Bailing => vec![(2., Event::Recovered { player: me })],
            _ => vec![],
        }
    }

    /// Spot Battle at six, two rounds: the last player (P5) leaves in its round 2 turn. Best score ranks
    /// (spot_battle.lua 98-107); P2 bails in round 1. Bests: P1 900, P3 800, P4 700, P0 500, P2 400.
    #[test]
    fn spot_battle_six_over_the_wire_a_guest_leaves() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 2, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let points = |p: usize, round: u32| [[Some(500.), Some(300.), None, Some(800.), Some(100.), Some(600.)], [Some(200.), Some(900.), Some(400.), Some(50.), Some(700.), Some(650.)]][round as usize - 1][p];
        let leave = |s: &Contest, into: f64| s.round == 2 && s.active == 5 && matches!(s.stage, Stage::Play { .. }) && into >= 0.5;
        let games = contest_over_the_wire(rules, 1, 5, leave, spot_battle_act(points));
        assert_eq!(contest_agreed(&games), [(1, 900), (3, 800), (4, 700), (0, 500), (2, 400), (5, -1)]);
    }

    /// The host (P0) leaves while P2 is bailing in round 1, just before P2's wipeout is over: P1 takes the contest
    /// over (the first player still here) and must still hear P2's Recovered. Bests: P1 900, P3 800, P4 700, P5 650,
    /// P2 400.
    #[test]
    fn spot_battle_six_over_the_wire_the_host_leaves_mid_bail() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 2, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let points = |p: usize, round: u32| [[Some(500.), Some(300.), None, Some(800.), Some(100.), Some(600.)], [Some(200.), Some(900.), Some(400.), Some(50.), Some(700.), Some(650.)]][round as usize - 1][p];
        for seed in 30..40 {
            let leave = |s: &Contest, into: f64| s.round == 1 && s.active == 2 && s.stage == Stage::Bailing && into >= 2.2;
            let games = contest_over_the_wire(rules.clone(), seed, 0, leave, spot_battle_act(points));
            assert_eq!(contest_agreed(&games), [(1, 900), (3, 800), (4, 700), (5, 650), (2, 400), (0, -1)], "seed {seed}");
        }
    }

    /// Best Trick / Jam: everyone's round score (`points(me, round)`), Jam's as two sequences (and one under the
    /// cumulative minimum, which counts nothing), a little later for each slot.
    fn contest_act(jam: bool, points: impl Fn(usize, u32) -> f32) -> impl Fn(&Contest, usize) -> Vec<(f64, Event)> {
        move |s, me| {
            let (at, points) = (1. + 0.3 * me as f64, points(me, s.round));
            let scored = |points| Event::Scored { player: me, points, object: None };
            match s.stage {
                Stage::Play { .. } if jam => vec![(at, scored(points / 2.)), (at + 0.5, scored(50.)), (at + 1., scored(points / 2.))],
                Stage::Play { .. } => vec![(at, scored(points))],
                _ => vec![],
            }
        }
    }

    /// Best Trick and Jam at six, two rounds; a guest (P5) leaves early in round 2, then (another run) the host (P0).
    /// Round points N-1-k (6 players: 5..0; a player who left scores none). Round 1: P1 600, P2 500, P3 400, P4 300,
    /// P5 200, P0 100 -> 5 4 3 2 1 0. Round 2: P1 900, P2 800, P3 700, P4 600, P5 500, P0 100.
    #[test]
    fn best_trick_and_jam_six_over_the_wire_a_guest_or_the_host_leaves() {
        let points = |p: usize, round: u32| [[100., 600., 500., 400., 300., 200.], [100., 900., 800., 700., 600., 500.]][round as usize - 1][p];
        for kind in [Kind::BestTrick, Kind::Jam] {
            let rules = Rules {
                kind,
                rounds: 2,
                cumulative: kind == Kind::Jam,
                round_times: vec![30., 30.],
                cumulative_min: vec![100., 100.],
                intros: vec![0., 0.],
                ..default()
            };
            let jam = kind == Kind::Jam;
            let early = |s: &Contest, into: f64| s.round == 2 && matches!(s.stage, Stage::Play { .. }) && into >= 0.2;
            // P5 gone in round 2: P0 1 point (5th), P1 10, P2 8, P3 6, P4 4.
            let games = contest_over_the_wire(rules.clone(), 2, 5, early, contest_act(jam, points));
            assert_eq!(contest_agreed(&games), [(1, 10), (2, 8), (3, 6), (4, 4), (0, 1), (5, -1)], "{kind:?}, a guest leaves");
            // P0 gone in round 2: P1 10, P2 8, P3 6, P4 4, P5 2.
            let games = contest_over_the_wire(rules, 3, 0, early, contest_act(jam, points));
            assert_eq!(contest_agreed(&games), [(1, 10), (2, 8), (3, 6), (4, 4), (5, 2), (0, -1)], "{kind:?}, the host leaves");
        }
    }

    /// 1-Up's attempter: off the start half a second into its pick, then its sequence a second into the attempt, on
    /// the picked object (a leader: the first unused one). `what(me, round)`: Some(points), Some(-1) bails, None lets
    /// the attempt time out.
    fn one_up_act(what: impl Fn(usize, u32) -> Option<f32>) -> impl Fn(&Contest, usize) -> Vec<(f64, Event)> {
        move |s, me| match s.stage {
            _ if s.active != me => vec![],
            Stage::Pick { .. } => vec![(0.5, Event::Started { player: me })],
            Stage::Play { .. } => match what(me, s.round) {
                Some(p) if p < 0. => vec![(1., Event::Bailed { player: me })],
                Some(points) => {
                    let object = s.object.unwrap_or_else(|| (0..).find(|o| !s.used.contains(o)).unwrap());
                    vec![(1., Event::Scored { player: me, points, object: Some(object) })]
                }
                None => vec![],
            },
            _ => vec![],
        }
    }

    /// 1-Up at six, NumToWin 2. Round 1: P0 sets 1000; P1 1500 takes the lead, P2 1200 doesn't, P3 2000 does, P4
    /// bails, P5 times out: P3 wins it. Round 2: P3 sets 5000 and nobody beats it: P3 wins. Rounds won rank; the rest
    /// (0) in slot order. A guest (P5) leaves at its round 2 pick; then (another run) the host (P0) during P1's round 1
    /// attempt.
    #[test]
    fn one_up_six_over_the_wire_a_guest_or_the_host_leaves() {
        let rules = Rules { kind: Kind::OneUp, time_limit: 20., num_to_win: 2, objects: 6, ..default() };
        let what = |p: usize, round: u32| match (round, p) {
            (1, 0) => Some(1000.),
            (1, 1) => Some(1500.),
            (1, 2) => Some(1200.),
            (1, 3) => Some(2000.),
            (1, 4) | (2, 2) => Some(-1.),
            (1, 5) => None,
            (2, 3) => Some(5000.),
            (2, p) => Some(100. * (p + 1) as f32),
            _ => None,
        };
        let leave = |s: &Contest, into: f64| s.round == 2 && s.active == 5 && matches!(s.stage, Stage::Pick { .. }) && into >= 0.2;
        let games = contest_over_the_wire(rules.clone(), 4, 5, leave, one_up_act(what));
        assert_eq!(contest_agreed(&games), [(3, 2), (0, 0), (1, 0), (2, 0), (4, 0)]);
        let leave = |s: &Contest, into: f64| s.round == 1 && s.active == 1 && matches!(s.stage, Stage::Play { .. }) && into >= 0.3;
        let games = contest_over_the_wire(rules, 5, 0, leave, one_up_act(what));
        assert_eq!(contest_agreed(&games), [(3, 2), (1, 0), (2, 0), (4, 0), (5, 0)]);
    }

    /// Six players on each Skate 2 results screen (roundresults_lb, throwdownresults) and the in-match leaderboardhud:
    /// every name drawn, nothing unanswered, no row dropped (online audit 10-12: results at max players). But for
    /// S.K.A.T.E.: throwdownresults' "skate" frame places playerName0-3 only (its "1up" frame 0-7), so retail's S.K.A.T.E.
    /// results hold four; a fifth and sixth go undrawn (handoff to the lead: the lobby cap is a user call).
    #[test]
    fn six_players_fit_every_skate2_results_screen() {
        use crate::{
            apt_vm::Value,
            match_core::{Mode, Standing},
            online_hud::{results_answers, tests::results_noop, throwdown_answers},
        };
        let Ok(assets) = std::env::var("SKATE3_ASSET_ROOT") else { return };
        let runtime = PathBuf::from(assets).join("private/hud-skate2/online/runtime");
        let names = ["Ann", "Bo", "Cy", "Dee", "Eve", "Fay"];
        let standings = |scores: [i64; 6]| -> Vec<Standing> {
            (names.iter().zip(scores).enumerate()).map(|(i, (n, score))| Standing { name: (*n).into(), score, finished: true, local: i == 4 }).collect()
        };
        let shown = |s: &crate::apt_screen::Screen| -> Vec<String> {
            (s.vm.objects.iter()).filter_map(|o| match o.fields.get("_displayText") { Some(Value::Text(t)) => Some(t.clone()), _ => None }).collect()
        };
        let cases = [
            (Mode::DeathRace, standings([79_900, 83_450, 91_020, 95_000, 120_500, 5_999_990])),
            (Mode::SpotBattle, standings([2500, 2000, 1500, 1200, 900, 700])),
            (Mode::BestTrick, standings([14, 12, 9, 7, 4, 2])),
            (Mode::Jam, standings([14, 12, 9, 7, 4, 2])),
            (Mode::HallOfMeat, standings([2500, 2000, 1500, 1200, 900, 700])),
            (Mode::Skate, standings([-2, -6, -7, -8, -9, -10])),
            (Mode::OneUp, standings([3, 2, 1, 1, 0, 0])),
        ];
        for (mode, standings) in cases {
            let (movie, answers) = match mode {
                Mode::Skate | Mode::OneUp => ("throwdownresults", throwdown_answers(mode, &standings, "#t")),
                _ => ("roundresults_lb", results_answers(mode, &standings, "#t", "k")),
            };
            let mut s = crate::apt_screen::Screen::load(&runtime.join(format!("{movie}.json")), &answers).unwrap();
            for _ in 0..400 {
                s.advance().unwrap();
            }
            assert!(!s.draws().unwrap().is_empty(), "{mode:?}");
            assert!(s.bindings.unanswered.iter().all(|k| results_noop(k)), "{mode:?}: {movie} asks {:?}", s.bindings.unanswered);
            let texts = shown(&s);
            let rows = if mode == Mode::Skate { 4 } else { 6 };
            for n in &names[..rows] {
                assert!(texts.iter().any(|t| t == n), "{mode:?}: {movie} has no row for {n}: {texts:?}");
            }
            for n in &names[rows..] {
                assert!(!texts.iter().any(|t| t == n), "{mode:?}: {movie} now has a row for {n}: update the S.K.A.T.E. cap note");
            }
        }
        let rows: Vec<_> = names.iter().enumerate().map(|(i, n)| (n.to_string(), 0, format!("#{i}"), i == 4)).collect();
        let mut s = crate::apt_screen::Screen::load(&runtime.join("leaderboardhud.json"), &leaderboard_answers("#t", true, 4, "#v", &rows, None)).unwrap();
        for state in [3, 1] {
            s.call("SetState", &[json!(state)]).unwrap();
        }
        for _ in 0..60 {
            s.advance().unwrap();
        }
        assert!(s.bindings.unanswered.is_empty(), "leaderboardhud asks {:?}", s.bindings.unanswered);
        let named = |row: usize| s.vm.get(s.clip(&format!("mcLeaderboardHUD.playerName{row}.mLeaderboardData.name_txt")).unwrap(), "_displayText").text();
        assert_eq!((0..6).map(named).collect::<Vec<_>>(), names, "leaderboardhud rows");
    }

    /// Party Play: each Spot Battle turn's teleporttransitionfinished waits on its attempter's Press-A card (one
    /// numbered card a turn, from the drawn first player round the roster); A goes on to the countdown. The last
    /// attempter's move back has none.
    #[test]
    fn party_spot_battle_turns_wait_on_their_card() {
        let rules = Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., sequences_to_end_turn: 1, ..default() };
        let mut c = Contest::new(rules, names(4), false);
        (c.party, c.first) = (true, 2);
        c.start(0.);
        let mut cards = Vec::new();
        for _ in 0..4 {
            c.tick(MOVE);
            c.tick(30.);
            assert!(matches!(c.stage, Stage::Move { turn: true, .. }) && c.waiting_a, "waits on the card");
            let card = c.card().expect("the card is up");
            assert_eq!(c.card(), Some(card), "the same card until A");
            cards.push((card.0, card.1));
            c.hudcomplete();
            assert_eq!(c.card(), None);
            c.tick(0.);
            assert_eq!(c.stage, Stage::Countdown { left: COUNTDOWN });
            c.tick(COUNTDOWN);
            c.apply(scored(c.active, 100.));
            c.tick(END_TURN);
        }
        assert_eq!(cards, [(1, 3), (2, 4), (3, 1), (4, 2)]);
        assert_eq!(c.stage, Stage::Move { left: MOVE, turn: false }, "the last attempter, once more");
        c.tick(MOVE);
        assert!(!c.waiting_a && matches!(c.stage, Stage::Ending { .. }), "no card for it");
        // Off the party path nothing waits.
        let mut solo = Contest::new(Rules { kind: Kind::SpotBattle, rounds: 1, time_limit: 20., ..default() }, names(1), false);
        solo.start(0.);
        solo.tick(MOVE);
        assert_eq!((solo.stage, solo.card()), (Stage::Countdown { left: COUNTDOWN }, None));
    }

    /// Party Play's 1-Up: the leader's and each follower's attempt wait on their card; the glue holds the skater
    /// meanwhile (1-Up's own UnpinAI comes at once).
    #[test]
    fn party_one_up_cards_every_attempt() {
        let rules = Rules { kind: Kind::OneUp, time_limit: 20., num_to_win: 2, objects: 2, ..default() };
        let mut c = Contest::new(rules, names(2), false);
        c.party = true;
        c.start(0.);
        c.tick(MOVE);
        assert_eq!(c.card(), Some((1, 1, 1)), "the leader's");
        assert!(!hold_and_watch(&c, 0, false, None).0 && c.waiting_a, "held by `waiting_a`, not 1-Up's pin");
        c.hudcomplete();
        c.tick(0.);
        assert_eq!((c.stage, said(&c).last()), (Stage::Pick { left: PICK }, Some(&Say::LeaderPick)));
        c.apply(Event::Started { player: 0 });
        c.apply(on(0, 1000., Some(0)));
        c.tick(AFTER_ATTEMPT);
        assert_eq!((c.stage, c.active), (Stage::Move { left: MOVE, turn: true }, 1));
        c.tick(MOVE);
        assert_eq!(c.card(), Some((2, 2, 1)), "the follower's");
        c.hudcomplete();
        c.tick(0.);
        assert_eq!(said(&c).last(), Some(&Say::BeatHighScore));
    }

    /// Bug 11-01: Party Play's 1-Up results listed no one (`placing` has none for 1-Up). show_results lists every skater
    /// with its rank and rounds won (ol_1up_01_svm_mountainup.lua 1053-1067).
    #[test]
    fn party_one_up_results_rank_the_rounds_won() {
        let rules = Rules { kind: Kind::OneUp, time_limit: 20., num_to_win: 1, objects: 2, ..default() };
        let mut s = Contest::new(rules, names(2), false);
        s.party = true;
        s.start(0.);
        // P0 sets 500 on object 0; P1 beats it there, takes the round and with it the match.
        for (player, points) in [(0, 500.), (1, 900.)] {
            s.tick(MOVE);
            s.hudcomplete();
            s.tick(0.);
            s.apply(Event::Started { player });
            s.apply(on(player, points, Some(0)));
            s.tick(AFTER_ATTEMPT);
        }
        s.tick(MOVE);
        s.tick(ONE_UP_ENDING);
        assert_eq!(s.stage, Stage::Done);
        let mut world = World::new();
        world.init_resource::<crate::party_play::PartyPlay>();
        let mut c = Contests { run: Some(Run::default()), ..default() };
        party_exchange(&mut world, &mut c, &Row::default(), &s);
        let results = world.resource::<crate::party_play::PartyPlay>().results.clone().expect("the results dialog");
        let get = |k: &str| results.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("HUDComponents.Results_GetNumPlayers"), Some(json!(2)));
        let block = get("HUDComponents.Results_GetLeaderboardDataBlock").unwrap();
        assert_eq!([&block[0], &block[1], &block[7]], [&json!(1), &json!("#1"), &json!("#P1")], "the winner first, a round won");
        assert_eq!([&block[8], &block[9], &block[15]], [&json!(2), &json!("#0"), &json!("#P0")]);
    }

    /// Bug 11-01: Party Play's start travels to New_San_Vanelona behind the loading screen, and the 1-Up's flythrough,
    /// started with the match, played out behind it. The match starts once the trip is over (as skate1_online's run, bug
    /// 10-31), the core's intro waiting for it.
    #[test]
    fn the_flythrough_waits_for_the_trip_to_the_map() {
        let row = Row { mode: "one_up".into(), intro: Some("up".into()), ..default() };
        let intros = [("up".to_owned(), 7.)].into();
        let data = Data { rows: [("up".to_owned(), row)].into(), strings: HashMap::new(), intros, intro_root: PathBuf::new() };
        let mut world = World::new();
        world.insert_resource(Match { row: "up".into(), phase: Phase::Intro { since: 0. }, ..default() });
        world.init_resource::<Arrows>();
        world.init_resource::<Cutscene>();
        let current = |map: &str| crate::map_transition::CurrentMap {
            path: Some(format!("maps/{map}.skate").into()),
            name: String::new(),
            spawn: [0.; 3],
            heading: 0.,
            generation: 0,
        };
        world.insert_resource(current("Skate_1_San_Vanelona"));
        let mut c = Contests::default();
        assert!(!setup(&mut world, &mut c, &data, Kind::OneUp, 0.), "still on the last map");
        assert!(c.run.is_none() && !world.resource::<Cutscene>().active(), "no flythrough behind the loading screen");
        assert!(world.resource::<Match>().intro, "the core's intro waits for it");
        world.insert_resource(current("New_San_Vanelona"));
        assert!(setup(&mut world, &mut c, &data, Kind::OneUp, 0.), "the trip is over");
        assert!(c.run.is_some() && world.resource::<Cutscene>().active(), "the flythrough starts on the map");
    }
}
