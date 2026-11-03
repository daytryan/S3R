//! The online match core's contract with game modes (docs/superpowers/specs/2026-10-02-skate2-online-modes-plan.md).
//! A mode plugin registers the rows it plays in [`Playable`], reads [`Match`] (its mode, the row, the phase) and
//! writes this player's `score` and `finished` (and `intro`, `time_limit`); the core runs the phases.
//!
//! Online, the host's Start Challenge puts a [`Game`] in its `@session`: the mode and the players, in spawn-slot
//! order, on the lobby's row. Each of them spawns on its slot and runs the phases on its own clock, and its `@score`
//! record carries its result. The host ends the run once everyone in it has finished or its time is up, and clears
//! the game after the results.
//!
//! `SKATE_MODE=<mode>:<row id>` (e.g. `death_race:dr_nsv_01`) starts a solo match at launch, to test a mode.
use crate::{multiplayer::Multiplayer, online_map::Pick, online_roster::OnlineRoster};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD as B64};
use bevy::prelude::*;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use skate_net::roster::{EVENTS_KEY, Events, Game, READY_KEY, Ready, SCORE_KEY, Score, SessionState, Target};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use skate_net::lobby::MAX_PLAYERS;

/// A mode claims the intro by setting `Match::intro` in time; it lasts at most INTRO_MAX.
const INTRO_MIN: f32 = 0.25;
const INTRO_MAX: f32 = 20.;
const COUNTDOWN: f32 = 3.;
/// The Results' countdown: Skate 3's default resultstime, 10 s (0x8273E1A0, 0x82DCFDC8; s3re 10-28). A server option
/// could change it in retail; none is installed. Skate 2's is not traced and shares it.
pub(crate) const RESULTS_FOR: f32 = 10.;
/// Online: finishes this late after the host's time is up still count (guests started a little later). A knob.
const LATE: f32 = 2.;
/// A Death Race racer who is out (finished, with this as the time: it ranks after every real time).
pub(crate) const DNF: i64 = 5_999_990;
/// Events a player keeps for players that have not taken them yet; past this the oldest go (a player that stopped
/// acknowledging, never a live one: a record carries several events and every console acknowledges each frame).
pub(crate) const EVENTS_BACKLOG: usize = 512;

/// S3 (release review 10-18): a mode's queue of peers' items held for later (a later attempt, an event no snapshot has
/// taken yet) never grows past EVENTS_BACKLOG: the oldest go, logged. A peer could otherwise number its items far
/// ahead and fill it for ever.
pub(crate) fn bound<T>(queue: &mut Vec<T>, what: &str) {
    let over = queue.len().saturating_sub(EVENTS_BACKLOG);
    if over > 0 {
        warn!("{what}: {over} held items over the limit, the oldest dropped");
        queue.drain(..over);
    }
}
/// The longest a player's leave waits for the other consoles' marks ([`Leaves`]). A knob: past the 5 s link timeout
/// (skate-net lobby) plus a roster and a record's trip, so only a console that never sends its mark meets it.
const LEAVE_HOLD: f64 = 8.;
/// Host: once everyone else still in the match has finished, a player that has not finished this long after is
/// counted finished, so one stuck console never holds the session.
// ponytail: retail's mode scripts all end by themselves (timers, rounds, playerdisconnected); this knob only frees a
// console our port left stuck. It is longer than any Lua wait after the others finish (race_shared's completion timer).
const STRAGGLER: f32 = 60.;
/// The Online Challenge Map's Career tab (online_session::TABS).
pub(crate) const CAREER_TAB: usize = 1;
/// The Online Challenge Map's Skate 2 tab (online_session::TABS).
pub(crate) const SKATE2_TAB: usize = 4;
/// The Online Challenge Map's Skate 1 tab (online_session::TABS).
pub(crate) const SKATE1_TAB: usize = 5;
/// The retail notices Start Challenge and the Online Challenge Map give.
pub(crate) const NEED_MORE_PLAYERS: &str = "ID_ONLINE_NEED_MORE_PLAYERS";
pub(crate) const TOO_MANY_PLAYERS: &str = "ID_ONLNE_TOO_MANY_PLAYERS";
pub(crate) const WAITING_FOR_SKATERS: &str = "ID_ONLINE_WAITING_FOR_SKATERS";
/// Retail's unavailable-mode title (ID_ONLINE_HUB_UNAVAILABLE_TITLE, "Unavailable").
// ponytail: retail lists every row as playable; a row with no ported mode here (Career's SimulPlay rows, Own the
// Spot / Own the Lot / Film / Photo, rows whose data is not installed) gets this retail string instead of a lobby that
// could never start. Its retail popup (title + ID_ONLINE_HUB_EXIT_UNAVAILABLE_MODE) is not traced; an OK notice here.
pub(crate) const UNAVAILABLE: &str = "ID_ONLINE_HUB_UNAVAILABLE_TITLE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    DeathRace,
    HallOfMeat,
    SpotBattle,
    BestTrick,
    Jam,
    Skate,
    OneUp,
    /// Skate 1 only: Death Race's course with a required trick at every gate (skate1_online.rs).
    SpotRace,
    /// Skate 3 only (skate3_*.rs): Domination, the Street / Vert contests (contest_shared), Own the Spot, Own the
    /// Lot, Film and Photo.
    Domination,
    Contest,
    OwnTheSpot,
    OwnTheLot,
    Film,
    Photo,
}
impl Mode {
    const ALL: [Mode; 14] = [
        Self::DeathRace,
        Self::HallOfMeat,
        Self::SpotBattle,
        Self::BestTrick,
        Self::Jam,
        Self::Skate,
        Self::OneUp,
        Self::SpotRace,
        Self::Domination,
        Self::Contest,
        Self::OwnTheSpot,
        Self::OwnTheLot,
        Self::Film,
        Self::Photo,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::DeathRace => "death_race",
            Self::HallOfMeat => "hall_of_meat",
            Self::SpotBattle => "spot_battle",
            Self::BestTrick => "best_trick",
            Self::Jam => "jam",
            Self::Skate => "skate",
            Self::OneUp => "one_up",
            Self::SpotRace => "spot_race",
            Self::Domination => "domination",
            Self::Contest => "contest",
            Self::OwnTheSpot => "own_the_spot",
            Self::OwnTheLot => "own_the_lot",
            Self::Film => "film",
            Self::Photo => "photo",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
    /// The mode of an Online Challenge Map folder (its challenge type filter).
    pub fn from_filter(filter: &str) -> Option<Self> {
        Some(match filter.strip_prefix("ID_MAP_FILTER_CHALLENGETYPES_")? {
            "DEATHRACE" => Self::DeathRace,
            "HOMBATTLE" | "HALLOFMEAT" => Self::HallOfMeat,
            "SPOTBATTLE" => Self::SpotBattle,
            "BESTTRICK" => Self::BestTrick,
            "JAM" => Self::Jam,
            "SKATE" => Self::Skate,
            "1UP" => Self::OneUp,
            "SPOTRACE" => Self::SpotRace,
            "DOMINATION" => Self::Domination,
            "STREET" | "VERT" => Self::Contest,
            "OTS" => Self::OwnTheSpot,
            "OTL" => Self::OwnTheLot,
            "FILM" => Self::Film,
            "PHOTO" => Self::Photo,
            _ => return None,
        })
    }
    /// Race times rank lowest first; points highest first.
    pub fn lower_wins(self) -> bool {
        matches!(self, Self::DeathRace | Self::SpotRace)
    }
    /// A result as the results list shows it: race time, letters (S.K.A.T.E. scores minus its letters) or points.
    pub fn show(self, score: i64) -> String {
        match self {
            Self::DeathRace | Self::SpotRace if score >= DNF => "DNF".into(),
            Self::DeathRace | Self::SpotRace => {
                let ms = score.max(0);
                format!("{}:{:02}.{:02}", ms / 60_000, ms / 1000 % 60, ms / 10 % 100)
            }
            // A peer's @score can be i64::MIN (P7): no overflow.
            Self::Skate => "SKATE".chars().take(score.saturating_neg().clamp(0, 5) as usize).map(|c| format!("{c}.")).collect(),
            _ => score.to_string(),
        }
    }
}

/// Seconds are `Time<Real>` elapsed seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Phase {
    #[default]
    Idle,
    /// Players are on their spawn slots; the mode plays its intro (camera, animation) while `Match::intro` is set.
    Intro { since: f32 },
    /// The mode shows its countdown.
    Countdown { ends: f32 },
    /// `ends`: the time limit, if the mode has one (set by the mode before Running, or None).
    Running { started: f32, ends: Option<f32> },
    Results { until: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Standing {
    pub name: String,
    pub score: i64,
    pub finished: bool,
    pub local: bool,
}

#[derive(Resource, Default)]
pub(crate) struct Match {
    pub mode: Option<Mode>,
    /// The Online Challenge Map row (challenge id) the match is on.
    pub row: String,
    pub phase: Phase,
    /// This player's slot: its spawn is the row's `start.slot(slot)`.
    pub slot: usize,
    /// Written by the mode: this player's result (meaning per mode: points, or race time in ms).
    pub score: i64,
    /// Written by the mode: this player is done (crossed the finish, out of attempts...).
    pub finished: bool,
    /// Written by the mode before Running: the time limit in seconds, if any.
    pub time_limit: Option<f32>,
    /// Written by the mode during Intro: its intro is still playing, so the countdown waits.
    pub intro: bool,
    /// Written by the mode as its intro starts: the challenge's title (Mission.GetTitle), which the intro's Spot Found
    /// card shows (intro_overlay.rs).
    pub title: String,
    /// Online: everyone in the match, best first (written by the core).
    pub standings: Vec<Standing>,
    /// The session's match (`Game::token`); 0 solo.
    pub token: u64,
    /// Online: the session host's slot in the match, if it plays in it (written by the core).
    pub host_slot: Option<usize>,
    /// The row is in [`Owned`]: its mode runs its own intro, countdown and results (Skate 1's Lua does), so the core
    /// goes straight from Intro to Running and shows no results; `hold` is then the mode's to set.
    pub owned: bool,
    /// Written by the mode every frame it wants it: hold the skater still (AI.PinAI / ForceLocalPlayerStop) — its own
    /// countdowns, intros and waits during Running. The core also holds through its Intro / Countdown (unless
    /// `owned`) and Results.
    pub hold: bool,
    /// Written by the mode every frame it wants it: its `hold` is its own intro, countdown or turn wait (a Skate 3 row
    /// runs them inside Running), so the emotes go through as in the core's Intro / Countdown (bug 10-17 #44 / #45).
    pub gestures: bool,
    /// Written by the mode: the slot whose skater the camera follows (Online.SwitchCamera); None is this player's.
    pub watch: Option<usize>,
    /// This player's start slot on the row (written by the core): the match starts there and Results return here.
    pub spawn: Option<[[f32; 4]; 4]>,
    /// A core teleport waiting for the engine's own (a wipeout's) to finish.
    pub teleport: Option<[[f32; 4]; 4]>,
    /// Written by the mode with `teleport`: it lands on foot (RepositionAI's 7th, SpawnOffboard). Taken with the teleport.
    pub teleport_on_foot: bool,
    /// Written by the mode with `teleport`: RepositionAI's 4th was true, so the session marker goes where it lands
    /// (cMsgSetSessionMarker 0x9A96B5F7, `Rt::reposition_marked`). Taken with the teleport.
    pub teleport_marks: bool,
    /// Online: everyone in the match by slot (written by the core); a player who left keeps a blank name, from when
    /// every console still here has its last events ([`Leaves`]).
    pub players: Vec<String>,
    /// Online: the blank names' slots, in the order the first console still here saw them go ([`Leaves::order`]), the
    /// same on every console. Not retail (user 10-12), end review M2: retail's playerdisconnected order is each
    /// console's own, UNTRACED native sync.
    pub left: Vec<usize>,
    /// Written by the mode online: events (its own JSON) every other player's mode gets, in order, in `inbox`.
    pub outbox: Vec<String>,
    /// Written by the core: other players' events, with their slots; the mode drains it.
    pub inbox: Vec<(usize, String)>,
    /// Written by the core: this player joined the session while the match was under way, so it only watches
    /// (retail ID_ONLINE_EVENT_JOIN_OBSERVER): held, the camera on `watch`.
    pub observer: bool,
    /// Written by the mode: the round sync points (Online.OnlineChallengeRoundSyncPoint) it has reached.
    pub round: u32,
    /// Written by the mode: it stopped on an error (Skate 1's script), so every other console's mode gets this player's
    /// leave (playerdisconnected, through [`Leaves`]) rather than waiting on its turns. Not retail: the error is ours.
    pub quit: bool,
    /// Written by the core: every slot's `round` (u32::MAX for a player who left: it holds no one up).
    pub rounds: Vec<u32>,
    /// Written by the mode: Pause.DisablePauseMenuAndBackpack is in force (EnablePauseMenuAndBackpack lifts it).
    pub pause_disabled: bool,
    /// Written by the mode: SessionMarker.DisableSetSessionMarker / DisableUseSessionMarker are in force.
    pub marker_set_disabled: bool,
    pub marker_use_disabled: bool,
    /// Written by the mode: HUD2.Show(TrickDisplayHUD, false) is in force (scoring_hud draws no trick text meanwhile).
    pub trick_display_hidden: bool,
    /// Written by the mode: the countdown is the mode's own (or it has none), so the core goes from its Intro straight to
    /// Running, as for `owned` rows (Skate 2 S.K.A.T.E.: skate.lua has no countdown at its start).
    pub no_countdown: bool,
    /// Written by the mode: seconds of its own ending between the run's end and the Results (Skate 2 Death Race's
    /// end_heat 3 s + cleanup_heat 2 s), the skater held meanwhile.
    pub outro: f32,
    /// Written by the core: the run is over and the Results begin at this time (`outro` after its end).
    pub ending: Option<f32>,
    /// Written by the core: a team match's team per slot (1 or 2, Online.GetTeam); empty otherwise
    /// (Online.IsTeamChallenge is false).
    pub teams: Vec<u8>,
    /// Written by the mode online: its own results order, (slot, score shown) best first, as its Lua ranks them (a tie
    /// keeps the earlier scorer, a player who left listed last). Empty: the core's [`standings`].
    pub order: Vec<(usize, i64)>,
}
impl Match {
    /// The core's Results: the seconds left before everyone returns to freeskate.
    pub fn results_left(&self, now: f32) -> Option<f32> {
        match self.phase {
            Phase::Results { until } => Some((until - now).max(0.)),
            _ => None,
        }
    }
    /// The mode's Lua has Pause.DisablePauseMenuAndBackpack in force: no pause menu (crossbar), backpack or session
    /// marker.
    /// A pin (`hold`) doesn't: retail's AI.PinAI (TU3 0x8284adc8, S2 0x826dc5f8) only sends the local skater
    /// cMsgPinPlayer, and S2's Meet Slappy keeps the player pinned by that message through its set-marker objective.
    pub fn pause_blocked(&self) -> bool {
        self.pause_disabled && self.phase != Phase::Idle
    }
    /// The session markers (place and return) are off: pause_blocked, or a Spot Battle under way. UNTRACED: the game
    /// mode test in the marker panel's gate 0x82668BD8 (session_marker::update). Fallback (user 10-14: Return to
    /// Marker worked in a Spot Battle; retail's doesn't): a Spot Battle blocks them from its intro to its end. On Skate
    /// 3 maps retail's reason is the challenge's own flags (own_the_spots_battles allows neither; session_marker's
    /// "challenge" rule); this stays for Skate 1 / 2 maps and an online.json from before a Rebuild.
    pub fn markers_blocked(&self) -> bool {
        // A game of S.K.A.T.E. too, online and in Party Play: its tricks are set at its spot (UNTRACED in retail's Lua).
        self.pause_blocked() || matches!(self.mode, Some(Mode::SpotBattle | Mode::Skate)) && self.phase != Phase::Idle
    }
    /// The mode's entries in the marker's set- and use-disabled lists (session_marker), while a match is on.
    pub fn marker_lists(&self) -> (bool, bool) {
        let on = self.phase != Phase::Idle;
        (on && self.marker_set_disabled, on && self.marker_use_disabled)
    }
    pub fn running(&self, mode: Mode) -> bool {
        self.mode == Some(mode) && matches!(self.phase, Phase::Running { .. })
    }
    /// The core's own teleport (onto the start slot) hasn't been made yet: a mode's teleport waits for it.
    pub fn teleporting(&self) -> bool {
        self.teleport.is_some()
    }
    pub(crate) fn start(&mut self, mode: Mode, row: &str, now: f32) {
        *self = Match { mode: Some(mode), row: row.into(), phase: Phase::Intro { since: now }, ..default() };
    }
}

/// Rows (challenge ids) a mode plugin plays; Start Challenge is offered on these only. A Career-tab row (SimulPlay)
/// is listed as [`career`]`(id)`: Career rows share ids with other tabs' rows a mode may play differently.
#[derive(Resource, Default)]
pub(crate) struct Playable(pub HashSet<String>);
impl Playable {
    pub fn plays(&self, tab: usize, row: &str) -> bool {
        match tab {
            CAREER_TAB => self.0.contains(&career(row)),
            TEAM_TAB => self.0.contains(&team(row)),
            _ => self.0.contains(row),
        }
    }
}
/// A Career-tab row's [`Playable`] entry.
pub(crate) fn career(row: &str) -> String {
    format!("career:{row}")
}
/// A Team-tab row's [`Playable`] entry: a mode that ports the row's TeamsEnabled path (skate3_contest, skate3_race,
/// skate3_one_up, skate3_uniform) registers it.
pub(crate) fn team(row: &str) -> String {
    format!("team:{row}")
}
/// Playable rows whose mode owns its sequencing ([`Match::owned`]); set before any mode sees the match.
#[derive(Resource, Default)]
pub(crate) struct Owned(pub HashSet<String>);

/// One step of the phase machine: intro → countdown → running → (time up or finished) → results → idle. Online the
/// host ends the run (`Game::results`, see `sync`): time up only finishes this player.
pub(crate) fn advance(m: &mut Match, now: f32) {
    m.phase = match m.phase.clone() {
        Phase::Intro { since } if (m.owned || m.no_countdown) && now - since >= INTRO_MIN && (m.owned || !m.intro || now - since >= INTRO_MAX) => {
            Phase::Running { started: now, ends: m.time_limit.map(|t| now + t) }
        }
        Phase::Intro { since } if now - since >= INTRO_MIN && (!m.intro || now - since >= INTRO_MAX) => {
            // Online the countdown starts once every player's intro is over (`round` 1), so the consoles start
            // together (at most INTRO_MAX in).
            // ponytail: Skate 2's start sync is native (its mode Lua has no sync point), not traced.
            m.round = m.round.max(1);
            if m.token != 0 && now - since < INTRO_MAX && m.rounds.iter().any(|&r| r < 1) {
                return;
            }
            Phase::Countdown { ends: now + COUNTDOWN }
        }
        Phase::Countdown { ends } if now >= ends => Phase::Running { started: now, ends: m.time_limit.map(|t| now + t) },
        Phase::Running { ends, .. } if m.token != 0 => {
            m.finished |= ends.is_some_and(|e| now >= e);
            return;
        }
        Phase::Running { ends, .. } if m.finished || ends.is_some_and(|e| now >= e) => {
            if now < *m.ending.get_or_insert(now + m.outro) {
                return;
            }
            Phase::Results { until: now + RESULTS_FOR }
        }
        Phase::Results { until } if now >= until => {
            m.mode = None;
            Phase::Idle
        }
        other => other,
    };
}

/// Why an Online Challenge Map challenge row on `tab` can't be picked here (its retail notice), or None: a row no mode
/// plays here on that tab (a Career row: a SimulPlay mode that registered it, skate3_ots; a Team row: a mode with its
/// team path ported). Such a row never makes or moves to a lobby that could not start. Decision T1: teams are the
/// host's, so retail's Online Team refusal (ID_ONLINE_TEAM_REQUIRED) never applies.
pub(crate) fn unavailable(tab: usize, row: &str, playable: &Playable) -> Option<&'static str> {
    (!playable.plays(tab, row)).then_some(UNAVAILABLE)
}

/// The Online Challenge Map's Team tab (online_session::TABS).
pub(crate) const TEAM_TAB: usize = 0;

/// An Online Challenge Map pick that must not make, join or move to a lobby: a challenge row [`unavailable`] here.
pub(crate) fn refused_pick(pick: &Pick, playable: &Playable) -> Option<&'static str> {
    match pick {
        Pick::Challenge { challenge, tab, .. } => unavailable(*tab, &challenge.id, playable),
        _ => None,
    }
}

/// What Start Challenge offers to start: the mode of the lobby's row, when a mode plays that row on its tab (not a
/// Team or Career lobby: [`unavailable`]) and no match or move is under way.
pub(crate) fn startable(state: &SessionState, playable: &Playable) -> Option<Mode> {
    let lobby = state.challenge.as_ref()?;
    let free = state.game.is_none() && state.moving.is_none();
    let row = (lobby.row.as_ref()).filter(|r| free && unavailable(r.tab as usize, &r.id, playable).is_none())?;
    Mode::from_filter(&lobby.title).filter(|_| !row.id.is_empty())
}

/// How many players a row's lobby holds: CHALLENGE_MAX (Skate 1 / 2 S.K.A.T.E.: 4), whatever its start locator's slot
/// count. Retail (audit 10-16, disc 0x82da3bf0): a challenge session's Blaze `max_players` is that same limit, so a
/// lobby never holds more than can start; a start with fewer slots spaces its later players (`locator_slot`), as
/// retail's RepositionAI does. skate_net::directory::Tags::capacity sizes the Steam lobby the same way.
pub(crate) fn row_cap(tab: usize, filter: &str) -> usize {
    // user 10-12 (cap at 4): inferred from retail's 4-row throwdownresults (its "skate" frame places playerName0-3);
    // retail S2 S.K.A.T.E.'s lobby max itself is UNRECOVERED. Skate 1's (audit 10-16): its disc skatehud has only
    // intro2..4 / outro2..4 and mcPlayer1..4, and hud_skate_challenge_overlay plays "intro" + GetNumPlayers() unclamped,
    // so retail never held a 5th (INFERRED from the HUD: no exe check was found). Skate 1 doesn't settle Skate 2's.
    if (tab == SKATE2_TAB || tab == SKATE1_TAB) && Mode::from_filter(filter) == Some(Mode::Skate) {
        return 4;
    }
    CHALLENGE_MAX
}

/// Retail's Start Challenge limit on the session's players (TU3 0x82643570 / 0x82643a48 against 0x82743528,
/// ID_ONLNE_TOO_MANY_PLAYERS at 0x82643fe0): 6. Skate 1 / 2 take it too (ONLINE_REPORT infers S2's 6). ponytail: 3 for
/// Street (8), Vert (15) and Death Race (19) rows while the Career menu state is current (hash 0x83BB0BE416CC9029,
/// `lis -31813` at 0x8264368c) is not ported: no Career row of those types is playable here.
pub(crate) const CHALLENGE_MAX: usize = 6;

/// A refused Start Challenge: the retail notice and its `{0:d}`.
pub(crate) type Refused = (&'static str, usize);

/// Host only: Start Challenge. The players that can play the row (`@ready`) play, in roster order (the host first:
/// slot 0); one that cannot never joins it (it watches). Refused while a player has not answered for this row yet,
/// with fewer than 2 players, a team lobby's side with none of them, or more in the session than the row holds
/// (`cap`, [`row_cap`]: retail counts the whole session, 0x82721368, watchers too). `Err(None)`: nothing to start. An Own the Lot lobby
/// starts one of its activities (`activity`, the crossbar's Select Activity): the game's mode is
/// `own_the_lot:<activity>`.
pub(crate) fn start_game(
    state: &mut SessionState,
    members: &[u64],
    ready: &BTreeMap<u64, Ready>,
    cap: usize,
    playable: &Playable,
    activity: Option<&str>,
) -> Result<(), Option<Refused>> {
    let mode = startable(state, playable).ok_or(None)?;
    if (mode == Mode::OwnTheLot) != activity.is_some() {
        return Err(None);
    }
    let row = state.challenge.as_ref().and_then(|c| c.row.as_ref()).map(|r| r.id.clone()).unwrap_or_default();
    let answer = |a: &u64| ready.get(a).filter(|r| r.row == row).map(|r| r.ok);
    if members.iter().any(|a| answer(a).is_none()) {
        return Err(Some((WAITING_FOR_SKATERS, 0)));
    }
    let slots: Vec<u64> = members.iter().copied().filter(|a| answer(a) == Some(true)).collect();
    if slots.len() < 2 {
        return Err(Some((NEED_MORE_PLAYERS, 0)));
    }
    // Not retail (audit 10-16): retail's Start Challenge has no team check (its sides came from EA Online Teams, never
    // an empty one), and its scripts end a team match with an empty side at once (1up.lua 827-833). Each side needs a
    // skater; uneven sides play (the scripts count TeamSize per side). The closest retail notice.
    let team_lobby = state.challenge.as_ref().is_some_and(|c| c.team);
    if team_lobby && ![1, 2].iter().all(|t| slots.iter().any(|&a| state.team(a) == *t)) {
        return Err(Some((NEED_MORE_PLAYERS, 0)));
    }
    if members.len() > cap {
        return Err(Some((TOO_MANY_PLAYERS, cap)));
    }
    state.proposal = None;
    let mode = activity.map_or_else(|| mode.name().into(), |a| format!("{}:{a}", mode.name()));
    // A team lobby's match carries each slot's team (Online.GetTeam).
    let teams = if team_lobby { slots.iter().map(|a| state.team(*a)).collect() } else { Vec::new() };
    state.game = Some(Game { token: crate::multiplayer::unique(), mode, slots, results: false, teams });
    Ok(())
}

/// Everyone still in the match, best first. Race times: finishers by time, then the rest; points: highest first.
pub(crate) fn standings(
    game: &Game,
    scores: &BTreeMap<u64, Score>,
    names: &BTreeMap<u64, String>,
    local: u64,
    lower_wins: bool,
) -> Vec<Standing> {
    let mut out: Vec<Standing> = (game.slots.iter())
        .filter_map(|a| {
            let s = scores.get(a).filter(|s| s.token == game.token);
            Some(Standing {
                name: names.get(a)?.clone(),
                score: s.map_or(0, |s| s.value),
                finished: s.is_some_and(|s| s.finished),
                local: *a == local,
            })
        })
        .collect();
    if lower_wins {
        out.sort_by_key(|s| (!s.finished, s.score));
    } else {
        out.sort_by_key(|s| std::cmp::Reverse(s.score));
    }
    out
}

/// A mode's own results order ([`Match::order`]): its slots best first with the scores it shows, a player who left
/// listed under the name it had (`known`, per slot); a slot never named is skipped.
pub(crate) fn ordered(order: &[(usize, i64)], game: &Game, scores: &BTreeMap<u64, Score>, known: &[String], local: u64) -> Vec<Standing> {
    (order.iter())
        .filter_map(|&(k, score)| {
            let a = *game.slots.get(k)?;
            let name = known.get(k).filter(|n| !n.is_empty())?.clone();
            let finished = scores.get(&a).is_some_and(|s| s.token == game.token && s.finished);
            Some(Standing { name, score, finished, local: a == local })
        })
        .collect()
}

fn records<T>(net: &Multiplayer, key: &str, decode: fn(&[u8]) -> Option<T>) -> BTreeMap<u64, T> {
    net.session().map_or_else(BTreeMap::new, |s| {
        (s.actors.iter()).filter_map(|(&a, actor)| Some((a, decode(&actor.application.get(key)?.value)?))).collect()
    })
}
fn scores(net: &Multiplayer) -> BTreeMap<u64, Score> {
    records(net, SCORE_KEY, Score::decode)
}

/// The reliable event channel (retail session messages are reliable). Our outbox joins our backlog (`sent`, our
/// `@ev` record); an event leaves it once every other player still here (`present`) has taken it (its `@score`'s
/// `taken`). The other players' new events go into the inbox, each once and in order (`leaves`: per player, the first
/// event not taken yet, which our own `@score` carries back to them). A player who left gets its blank name in
/// `m.players` once every console still here has its last events ([`Leaves`]).
#[allow(clippy::too_many_arguments)]
fn relay(
    m: &mut Match,
    game: &Game,
    local: u64,
    present: &dyn Fn(u64) -> bool,
    records: &BTreeMap<u64, Events>,
    scores: &BTreeMap<u64, Score>,
    sent: &mut Events,
    leaves: &mut Leaves,
    now: f64,
) {
    if sent.token != game.token {
        *sent = Events { token: game.token, ..default() };
        *leaves = Leaves::new(game.token);
    }
    let me = game.slots.iter().position(|&a| a == local).unwrap_or(usize::MAX);
    leaves.publish(me, sent, std::mem::take(&mut m.outbox));
    // Before this frame's events: a leave given now comes after every event of the leaver's the mode already has.
    let here = |b: u64| b == local || present(b);
    let mut released = Vec::new();
    for (k, _) in game.slots.iter().enumerate().filter(|(_, a)| !here(**a)) {
        let sent_on = leaves.left(me, k, now);
        leaves.publish(me, sent, sent_on);
        let others = (game.slots.iter().enumerate()).filter(|&(j, &b)| j != k && b != local && present(b)).map(|(j, _)| j);
        if leaves.released(k, others, now)
            && let Some(name) = m.players.get_mut(k)
        {
            name.clear();
            released.push(k);
        }
    }
    m.left = leaves.order(game.slots.iter().position(|&a| here(a)).unwrap_or(me), released);
    let end = sent.first + sent.items.len() as u32;
    let others = || game.slots.iter().copied().filter(|&a| a != local && present(a));
    let taken_by = |a: u64| {
        let s = scores.get(&a).filter(|s| s.token == game.token);
        s.and_then(|s| s.taken.iter().find(|t| t.0 == local)).map_or(0, |t| t.1)
    };
    let acked = others().map(taken_by).min().unwrap_or(end).clamp(sent.first, end);
    sent.items.drain(..(acked - sent.first) as usize);
    sent.first = acked;
    let over = sent.items.len().saturating_sub(EVENTS_BACKLOG);
    if over > 0 {
        warn!("Match events: {over} not taken by every player in time, dropped");
        sent.items.drain(..over);
        sent.first += over as u32;
    }
    // An event bigger than a record could never be sent; it would hold every later one back.
    while !sent.items.is_empty() && sent.fitted().0 == 0 {
        error!("Match event of {} bytes does not fit a record; dropped", sent.items[0].len());
        sent.items.remove(0);
        sent.first += 1;
    }
    for (slot, actor) in game.slots.iter().enumerate().filter(|(_, a)| **a != local) {
        let Some(e) = records.get(actor).filter(|e| e.token == game.token) else { continue };
        leaves.forget(slot, e.first);
        leaves.take(slot, e.first, &e.items, &mut m.inbox);
    }
}

/// Before an event a player sent on: "@on <leaver's slot> <its number> <the event>". No mode's JSON starts with '@'.
const SENT_ON: &str = "@on ";
/// A console's mark that it saw a player go, after what it sent on of that player's: "@gone <slot>".
const SAW_GO: &str = "@gone ";
/// A player's first event in a match: "@key <its public key>". Its every event after it goes out as "<signature>
/// <event>" ([`Leaves::publish`]), so one sent on as a leaver's is that player's own, never a peer's make-up.
const KEY: &str = "@key ";
/// An Ed25519 signature's length in [`B64`].
const SIGNATURE_CHARS: usize = 86;

/// An event at its biggest on the wire: signed, sent on as a leaver's at the largest numbers (the modes' size tests).
#[cfg(test)]
pub(crate) fn at_its_biggest(event: &str) -> String {
    let signature = "A".repeat(SIGNATURE_CHARS);
    format!("{signature} {SENT_ON}{} {} {signature} {event}", MAX_PLAYERS - 1, u32::MAX)
}

/// An event as its sender signed it: (the signature, the event); None for one without.
fn signed(item: &str) -> Option<(&str, &str)> {
    let (signature, body) = item.split_once(' ')?;
    let base64 = |b: u8| b.is_ascii_alphanumeric() || b == b'+' || b == b'/';
    (signature.len() == SIGNATURE_CHARS && signature.bytes().all(base64)).then_some((signature, body))
}

/// Each console's taking of the other players' events (`next`), and a leaver's last ones: they reach every console
/// still in the match or none, and before its leave.
/// Not retail (user 10-12), end review I2: a player's events travel only in its own record, which goes with it, so its
/// last ones reached only the consoles that read the record in time, and the consoles decided apart (a 1-Up best, a
/// Players' Choice, a Skate 1 leader's trick). Retail's session sync is native, UNTRACED. Here a console that sees a
/// player go sends on, in its own events, the leaver's it holds that another may lack, then its mark; it gives its
/// mode the leave only once every other console still here has sent its mark, each behind its copies. So every console
/// has the leaver's events up to the furthest any console took, then its leave, with no race against a clock.
/// Online safety 10-26: each player signs its own events with a key of this match (its first event), so an event sent
/// on as a player's counts only with that player's signature, and only straight after what this console already has of
/// a player still here: a peer can't make one up for anyone (a S.K.A.T.E. letter, a frozen host snapshot) nor skip
/// someone's real ones. Sent on events still count the moment they come (delaying them split the consoles' modes).
/// ponytail: a second console leaving with the only copy before it went on can still split the rest; LEAVE_HOLD caps
/// the wait for a mark that never comes.
#[derive(Default)]
pub(crate) struct Leaves {
    /// Per slot: the first of its events not taken yet.
    next: BTreeMap<usize, u32>,
    /// Per slot: its events taken that another console may still lack, numbered (`forget` drops the rest).
    held: BTreeMap<usize, Vec<(u32, String)>>,
    /// Per slot that left as this console sees it: since when.
    gone: BTreeMap<usize, f64>,
    /// The marks heard: (leaver, console).
    marks: BTreeSet<(usize, usize)>,
    /// Every console's marks (this one's too) as (console, leaver), in the order it made them: its own stream carries
    /// them in order, so every console holds the same order for a given console ([`Self::order`]).
    marked: Vec<(usize, usize)>,
    /// The match: what every signature covers, with the signer's slot and the event's number.
    token: u64,
    /// This console's key for its own events this match, made with its first.
    signer: Option<Ed25519KeyPair>,
    /// Per slot: its public key, from its own first event.
    keys: BTreeMap<usize, Vec<u8>>,
}
impl Leaves {
    /// A match's taking (`token`), with a key of its own once it has an event to send.
    pub fn new(token: u64) -> Self {
        Leaves { token, ..default() }
    }
    fn message(&self, slot: usize, seq: u32, body: &str) -> Vec<u8> {
        [&self.token.to_le_bytes()[..], &(slot as u64).to_le_bytes(), &seq.to_le_bytes(), body.as_bytes()].concat()
    }
    /// This console's key as its first event says it, made now (None without the system's random source).
    pub fn key_item(&mut self) -> Option<String> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).ok()?;
        let key = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).ok()?;
        let item = format!("{KEY}{}", B64.encode(key.public_key()));
        self.signer = Some(key);
        Some(item)
    }
    /// This console's (`me`) event number `seq` as it goes out: "<signature> <event>" (as is without a key).
    pub fn sign(&self, me: usize, seq: u32, body: &str) -> String {
        match &self.signer {
            Some(key) => format!("{} {body}", B64.encode(key.sign(&self.message(me, seq, body)))),
            None => body.to_owned(),
        }
    }
    /// This console's (`me`) events onto its stream, each signed, behind its key (sent once, first).
    pub fn publish(&mut self, me: usize, sent: &mut Events, items: Vec<String>) {
        for body in items {
            if self.signer.is_none()
                && let Some(key) = self.key_item()
            {
                sent.items.push(key);
            }
            let seq = sent.first + sent.items.len() as u32;
            sent.items.push(self.sign(me, seq, &body));
        }
    }
    /// The event `slot` signed as number `seq`, if its signature is that player's own.
    fn verified<'a>(&self, slot: usize, seq: u32, item: &'a str) -> Option<&'a str> {
        let (signature, body) = signed(item)?;
        let signature = B64.decode(signature).ok()?;
        let key = self.keys.get(&slot)?;
        UnparsedPublicKey::new(&ED25519, key).verify(&self.message(slot, seq, body), &signature).ok()?;
        Some(body)
    }
    /// `slot`'s own events, `first` numbering `items[0]`: the new ones go to `out` in order (an event sent on as its
    /// leaver's), none once `slot` left here (what this console took of it is what its mark stands for).
    pub fn take(&mut self, slot: usize, first: u32, items: &[String], out: &mut Vec<(usize, String)>) {
        if !self.gone.contains_key(&slot) {
            // A peer's `first` near u32::MAX numbers only what fits (no overflow panic).
            for (seq, item) in (first..=u32::MAX).zip(items) {
                self.one(slot, seq, item, 0, out);
            }
        }
    }
    fn one(&mut self, slot: usize, seq: u32, item: &str, depth: usize, out: &mut Vec<(usize, String)>) {
        let next = self.next.get(&slot).copied().unwrap_or(0);
        if seq < next {
            return;
        }
        // From its own stream (the session vouches for its sender) an event is its own; sent on, it needs its
        // signature, and a player still here only its next one.
        let body = if depth == 0 { Some(signed(item).map_or(item, |(_, body)| body)) } else { self.verified(slot, seq, item) };
        let Some(body) = body.filter(|_| depth == 0 || seq == next || self.gone.contains_key(&slot)) else {
            warn!("Match events: slot {slot}'s {seq} sent on without its signature or ahead of its own; dropped");
            return;
        };
        if seq > next {
            warn!("Match events: slot {slot}'s {next}..{seq} were dropped before they reached us");
        }
        self.next.insert(slot, seq.saturating_add(1));
        // As it came (signed): what this console sends on of it carries its sender's signature.
        self.held.entry(slot).or_default().push((seq, item.to_owned()));
        if let Some(key) = body.strip_prefix(KEY) {
            // A player's key, from its own stream only.
            if depth == 0
                && let Ok(key) = B64.decode(key)
            {
                self.keys.insert(slot, key);
            }
        } else if let Some(on) = body.strip_prefix(SENT_ON) {
            let mut part = on.splitn(3, ' ');
            if let (Some(Ok(leaver)), Some(Ok(seq)), Some(item)) = (part.next().map(str::parse), part.next().map(str::parse), part.next()) {
                // Online safety P7: a peer's "@on" names another slot a session can have, nested no deeper than a
                // chain of leavers can (a deep one blew the stack).
                if leaver != slot && leaver < MAX_PLAYERS && depth < MAX_PLAYERS {
                    self.one(leaver, seq, item, depth + 1, out);
                }
            }
        } else if let Some(Ok(leaver)) = body.strip_prefix(SAW_GO).map(str::parse) {
            // A mark counts once, and a flood of made-up ones stays small.
            if self.marks.len() < EVENTS_BACKLOG && self.marks.insert((leaver, slot)) {
                self.marked.push((slot, leaver));
            }
        } else {
            out.push((slot, body.to_owned()));
        }
    }
    /// `slot`'s sender let go of its events before `first`: every console still here took them.
    pub fn forget(&mut self, slot: usize, first: u32) {
        if let Some(held) = self.held.get_mut(&slot) {
            held.retain(|e| e.0 >= first);
        }
    }
    /// `slot` left, as this console (`me`) sees it at `now`: the first time, what to send on (its events held here,
    /// then the mark); after, nothing.
    pub fn left(&mut self, me: usize, slot: usize, now: f64) -> Vec<String> {
        if self.gone.contains_key(&slot) {
            return Vec::new();
        }
        self.gone.insert(slot, now);
        self.marked.push((me, slot));
        // Its key is never taken sent on.
        let held = self.held.get(&slot).into_iter().flatten().filter(|(_, item)| !item.starts_with(KEY));
        held.map(|(seq, item)| format!("{SENT_ON}{slot} {seq} {item}")).chain([format!("{SAW_GO}{slot}")]).collect()
    }
    /// Whether this console's mode gets `slot`'s leave: every other console still here (`others`) has sent its mark,
    /// or LEAVE_HOLD has passed since it left here.
    pub fn released(&self, slot: usize, others: impl IntoIterator<Item = usize>, now: f64) -> bool {
        self.gone.get(&slot).is_some_and(|&at| now - at >= LEAVE_HOLD || others.into_iter().all(|o| self.marks.contains(&(slot, o))))
    }
    /// The leaves given (`released`), in the order console `first` (the first slot still here) marked them, then any
    /// it has not marked (given at LEAVE_HOLD) in slot order: every console ends with the same order whatever order it
    /// saw them go in.
    pub fn order(&self, first: usize, mut released: Vec<usize>) -> Vec<usize> {
        let at = |k: &usize| self.marked.iter().position(|&m| m == (first, *k)).unwrap_or(usize::MAX);
        released.sort_by_key(|k| (at(k), *k));
        released
    }
    /// What our `@score` says we took: per other player (`slots`: the actors by slot), its first event not taken yet.
    pub fn taken(&self, slots: &[u64]) -> Vec<(u64, u32)> {
        self.next.iter().filter_map(|(&k, &n)| Some((*slots.get(k)?, n))).collect()
    }
}

/// Whether this player can play the lobby's row: a mode plays it and the row is on a map installed here.
fn ready_for(state: &SessionState, playable: &Playable, on_map: impl FnOnce(&Target) -> bool) -> Option<Ready> {
    let row = state.challenge.as_ref()?.row.as_ref()?;
    Some(Ready { row: row.id.clone(), ok: playable.plays(row.tab as usize, &row.id) && on_map(row) })
}

/// Every player's `@ready` (the host's Start Challenge reads them).
pub(crate) fn readies(net: &Multiplayer) -> BTreeMap<u64, Ready> {
    records(net, READY_KEY, Ready::decode)
}

/// How many players the lobby's row holds ([`row_cap`]).
pub(crate) fn lobby_cap(state: &SessionState) -> usize {
    let lobby = state.challenge.as_ref();
    row_cap(lobby.and_then(|c| c.row.as_ref()).map_or(0, |r| r.tab as usize), lobby.map_or("", |c| c.title.as_str()))
}

/// The row's spawn for this slot, found on this player's own map.
fn spawn(challenge: Option<&crate::challenge_map::ChallengeMap>, labels: &[String], row: &Target, slot: usize) -> Option<[[f32; 4]; 4]> {
    match challenge?.find_online(labels, row.tab as usize, &row.id)? {
        Pick::Challenge { challenge, .. } => challenge.spawn.start.map(|s| s.slot(slot)),
        _ => None,
    }
}

/// Online freeskate spectating: the watched player's actor. Retail picks it from the LB marker menu's B option and
/// cycles through the players; here the Start menu lists them by name (user 10-10) and B leaves ([`spectate_back`]).
#[derive(Resource, Default)]
pub(crate) struct Spectate(pub Option<u64>);

/// Spectate's list: everyone in the session in the roster's order, us included. Retail's Swap Camera cycle includes
/// the local player, and landing on it is the only way back (0x826682b0); it shows with 2+ players only.
pub(crate) fn spectatable(roster: &OnlineRoster) -> Vec<(u64, String)> {
    if roster.members.len() < 2 {
        return Vec::new();
    }
    roster.members.iter().map(|p| (p.actor, p.name.clone())).collect()
}

/// The Spectate pick: the player listed at `index` when the menu opened (`listed`, [`spectatable`]) while still in
/// the session; our own row (or a player gone since) is our own camera (None).
pub(crate) fn spectate_pick(listed: &[u64], index: usize, roster: &OnlineRoster) -> Option<u64> {
    listed.get(index).copied().filter(|&a| a != roster.local_actor && roster.members.iter().any(|p| p.actor == a))
}

/// Spectating ends when we leave the session, a match of ours starts, or the watched player leaves.
fn end_spectate(mut spectate: ResMut<Spectate>, net: Res<Multiplayer>, m: Res<Match>, roster: Res<OnlineRoster>) {
    let gone = |actor: u64| !net.active() || m.token != 0 || !roster.members.iter().any(|p| p.actor == actor);
    if spectate.0.is_some_and(gone) {
        spectate.0 = None;
    }
}

/// B leaves spectating (user 10-24; not retail, whose Swap Camera only cycles on, round to us), read as the pad
/// publishes it (a menu's pad is discarded, so its B never counts). Held B never reaches the skater (menu_release).
/// LB+B is the marker panel's Player List instead (user 10-25, session_marker::player_list).
fn spectate_back(mut spectate: ResMut<Spectate>, input: Option<ResMut<crate::input::ControllerInput>>) {
    let back = input.filter(|i| spectate.0.is_some() && !i.gated() && i.b_pressed() && !i.session_marker_actions().0);
    let Some(mut input) = back else { return };
    spectate.0 = None;
    input.menu_release = true;
}

/// Spectating's handover (user 10-25: the player you watch is always smooth, cutting to a far one too): who the camera
/// is asked to show ([`ask_watch`]) and since when (the session clock, s), and who it shows. `shown` takes `asked` once
/// that player's samples come at the watched rate (multiplayer::render::present); until then the camera stays where it
/// is, never on a far player's sparse, floaty past nor the snap forward that follows it.
#[derive(Resource, Default)]
pub(crate) struct Watching {
    pub asked: Option<u64>,
    pub since: f64,
    pub shown: Option<u64>,
}

/// The skater the camera is asked to show (a match's watched player, else the Spectate pick), published as our
/// `@watch` so that player sends us everything at the nearest rate before the camera cuts to it (skate_net::lobby).
pub(crate) fn ask_watch(m: Res<Match>, roster: Res<OnlineRoster>, spectate: Res<Spectate>, mut watching: ResMut<Watching>, mut net: ResMut<Multiplayer>) {
    let slot = m.watch.filter(|&s| (m.observer || s != m.slot) && m.token != 0);
    let asked = slot.and_then(|slot| roster.state.game.as_ref()?.slots.get(slot).copied()).or(spectate.0);
    // Only a match's observer follows ([`followed`]). The Spectate pick and a waiting player's turn show the skater asked
    // for, a spectator's parked one too: every camera row is its sender's own skater's (multiplayer::sent_camera).
    let asked = match net.session() {
        Some(s) if m.observer && slot.is_some() => {
            let watch = |id: u64| {
                let value = &s.actors.get(&id)?.application.get(skate_net::roster::WATCH_KEY)?.value;
                Some(u64::from_le_bytes(value.get(..8)?.try_into().ok()?)).filter(|a| s.actors.contains_key(a))
            };
            followed(s.local, asked, watch)
        }
        _ => asked,
    };
    if watching.asked != asked {
        (watching.asked, watching.since) = (asked, net.now_ms() as f64 / 1000.);
    }
    // The one still shown too, until the cut: its view stays smooth while the camera waits.
    net.publish_watch(asked, watching.shown);
}

/// Who a match observer (joined under way) is shown when asked for `asked`, the first player still in it
/// (`Match::watch`): one waiting for its turn watches the player up, so follow its `@watch` (`watch`: the actor it
/// names, if in the session) to the one the mode's camera is on (audit 10-30: an observer on Spot Battle's waiting
/// slot 0 sat on that player's parked skater the whole turn). Never round to us, nor round a loop. INFERRED: retail
/// picks its observer camera's player natively (not traced).
fn followed(local: u64, asked: Option<u64>, watch: impl Fn(u64) -> Option<u64>) -> Option<u64> {
    let mut at = asked?;
    let mut seen = vec![at];
    while let Some(next) = watch(at).filter(|n| *n != local && !seen.contains(n)) {
        seen.push(next);
        at = next;
    }
    Some(at)
}

/// The camera [`observer_camera`] put on the screen this frame (a match's watched player or the Spectate pick); None on
/// our own. The audio listens from it (user 10-24: the watched board sounded from our parked skater). INFERRED:
/// retail's listener is the game camera block (`+0x2FD20`), which Swap Camera's view switch would move with it.
#[derive(Resource, Default)]
pub(crate) struct ObserverView(pub Option<Transform>);

impl ObserverView {
    /// The view as the audio observation's native (at, position): Bevy cameras look along local -Z.
    pub(crate) fn listener(&self) -> Option<([f32; 3], [f32; 3])> {
        self.0.map(|view| ((view.rotation * Vec3::NEG_Z).to_array(), view.translation.to_array()))
    }
}

pub(crate) struct MatchCorePlugin;
impl Plugin for MatchCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Match>()
            .init_resource::<Playable>()
            .init_resource::<Owned>()
            .init_resource::<Spectate>()
            .init_resource::<ObserverView>()
            .init_resource::<Watching>()
            .init_resource::<MatchClock>()
            .add_systems(First, tick_clock)
            .add_systems(FixedUpdate, spectate_back.after(crate::app::SimulationSet::Input))
            .add_systems(Update, (start_from_env, sync, referee, end_spectate, step, ask_watch).chain().after(crate::online_roster::track))
            .add_systems(Update, observer_camera.after(crate::camera::present).before(crate::app::FrameSet::Verification));
    }
}

/// The offline pause time inside the running match (user 10-14: Party Play's timer ran on under the pause menu).
/// The modes' seconds are `Time<Real>`'s less this. Offline, a pause stops `Time<Virtual>` (core_menu); online
/// nothing pauses, so there it is 0 and the clock is `Time<Real>`'s. Not retail-traced: retail's challenge timers
/// stop with the game while paused. 0 again with no match, so a stamp made outside a match is the real clock's.
#[derive(Resource, Default)]
pub(crate) struct MatchClock {
    paused: f32,
    /// This frame went into `paused`: the clock stood still for it.
    stopped: bool,
}

fn tick_clock(mut clock: ResMut<MatchClock>, m: Res<Match>, real: Res<Time<Real>>, virt: Res<Time<Virtual>>) {
    clock.stopped = m.mode.is_some() && m.token == 0 && virt.is_paused();
    if m.mode.is_none() {
        clock.paused = 0.;
    } else if clock.stopped {
        clock.paused += real.delta_secs();
    }
}

/// The match clock's seconds (MatchClock): what every mode stamps and compares its times with.
pub(crate) fn match_now(world: &World) -> f32 {
    let real = world.get_resource::<Time<Real>>().map_or(0., Time::elapsed_secs);
    real - world.get_resource::<MatchClock>().map_or(0., |c| c.paused)
}

/// The match clock's step this frame: `Time<Real>`'s, 0 while it stands still. What a mode counts its timers down by.
pub(crate) fn match_dt(world: &World) -> f32 {
    if world.get_resource::<MatchClock>().is_some_and(|c| c.stopped) {
        return 0.;
    }
    world.get_resource::<Time<Real>>().map_or(0., Time::delta_secs)
}

fn start_from_env(mut m: ResMut<Match>, owned: Res<Owned>, time: Res<Time<Real>>, mut done: Local<bool>) {
    if std::mem::replace(&mut *done, true) {
        return;
    }
    let Ok(spec) = std::env::var("SKATE_MODE") else { return };
    let (mode, row) = spec.split_once(':').unwrap_or((&spec, ""));
    match Mode::parse(mode) {
        Some(mode) => {
            m.start(mode, row, time.elapsed_secs());
            m.owned = owned.0.contains(row);
        }
        None => warn!("SKATE_MODE: unknown mode {mode:?}"),
    }
}

/// Everyone: answer whether we can play the lobby's row, start the host's new match (when we are in it; else watch it),
/// end ours when the host does, relay the events, publish our score and keep the standings.
#[allow(clippy::too_many_arguments)]
fn sync(
    mut net: ResMut<Multiplayer>,
    roster: Res<OnlineRoster>,
    mut m: ResMut<Match>,
    time: Res<Time<Real>>,
    challenge: Option<Res<crate::challenge_map::ChallengeMap>>,
    menu: Res<crate::graphics_menu::Menu>,
    (owned, playable): (Res<Owned>, Res<Playable>),
    mut seen: Local<u64>,
    mut relayed: Local<(Events, Leaves)>,
    mut answered: Local<Option<(Target, Ready)>>,
    mut known: Local<(u64, Vec<String>)>,
) {
    let now = time.elapsed_secs();
    let (active, local, _) = net.mod_identity();
    // `@ready` for the lobby's row (the map lookup reads online.json: once per row).
    if active && let Some(row) = roster.state.challenge.as_ref().and_then(|c| c.row.clone()) {
        if answered.as_ref().is_none_or(|(r, _)| *r != row) {
            let labels = menu.map_labels();
            let on_map = |r: &Target| {
                let found = challenge.as_deref().and_then(|c| c.find_online(&labels, r.tab as usize, &r.id));
                matches!(found, Some(Pick::Challenge { .. }))
            };
            *answered = ready_for(&roster.state, &playable, on_map).map(|ready| (row, ready));
        }
        if let Some((_, ready)) = answered.as_ref() {
            net.publish_ready(ready);
        }
    }
    let game = roster.state.game.clone().filter(|_| active);
    // We left the session: single player again at once, where we stand (no results, no trip to the start slot).
    if m.token != 0 && !active {
        *m = Match::default();
    }
    // The host dropped our match (a move, a new host without it); results already up run out.
    if m.token != 0 && game.as_ref().is_none_or(|g| g.token != m.token) && !matches!(m.phase, Phase::Results { .. }) {
        drop_match(&mut m);
    }
    let Some(game) = game else { return };
    let names: BTreeMap<u64, String> = roster.members.iter().map(|p| (p.actor, p.name.clone())).collect();
    let connected = |a: u64| net.session().is_some_and(|s| s.actors.contains_key(&a));
    // A player still connected whose `@player` has not reached us yet is pending, not gone: the match waits for its
    // name, so no mode reads it as having left (its blank name).
    let pending = game.slots.iter().any(|&a| !names.contains_key(&a) && connected(a));
    if *seen != game.token && !pending {
        *seen = game.token;
        // `mode:row`: the row played is not the lobby's (an Own the Lot activity of the lobby's lot).
        let (name, played) = game.mode.split_once(':').map_or((game.mode.as_str(), None), |(n, r)| (n, Some(r)));
        match (Mode::parse(name), game.slots.iter().position(|&a| a == local)) {
            (Some(mode), Some(slot)) => {
                let row = roster.state.challenge.as_ref().and_then(|c| c.row.clone()).unwrap_or_default();
                info!("Match {:x}: {} on {} in slot {slot}", game.token, game.mode, row.id);
                m.start(mode, played.unwrap_or(&row.id), now);
                (m.token, m.slot, m.owned, m.teams) = (game.token, slot, owned.0.contains(&row.id), game.teams.clone());
                // Onto the slot (retried past a wipeout in `step`), held there through Intro and Countdown.
                m.spawn = spawn(challenge.as_deref(), &menu.map_labels(), &row, slot);
                m.teleport = m.spawn;
                if m.spawn.is_none() {
                    warn!("Match spawn: row {} has no start on this map", row.id);
                }
            }
            // Joined while it was under way, or cannot play its row: we watch it (retail's observer).
            (_, None) if !game.results => *m = Match { token: game.token, observer: true, ..default() },
            _ => {}
        }
    }
    let host = net.session().and_then(|s| s.host_actor());
    m.host_slot = host.and_then(|h| game.slots.iter().position(|&a| a == h));
    let players: Vec<String> = game.slots.iter().map(|a| names.get(a).cloned().unwrap_or_default()).collect();
    // Each slot's last name seen: a mode's own order (`Match::order`) lists a player who left under it.
    if known.0 != game.token {
        *known = (game.token, players.clone());
    }
    for (k, name) in players.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
        if let Some(slot) = known.1.get_mut(k) {
            slot.clone_from(name);
        }
    }
    if m.observer && m.token == game.token {
        // ponytail: the first player still in it; retail's observer camera target is chosen natively (not traced).
        m.watch = players.iter().position(|n| !n.is_empty());
        m.players = players;
        return;
    }
    let scores = scores(&net);
    let Some(mode) = m.mode.filter(|_| m.token == game.token) else {
        // Audit 10-28 #2: a Skate 1 row's script ends its mode in `step` the frame it finishes; that player's finished
        // result still goes (the host never saw it, and a 2-player lobby stayed in the match for good). Review 10-29:
        // and it keeps taking the others' events, or theirs to each other stall behind its frozen `taken`.
        if m.token == game.token {
            let here = |a: u64| names.contains_key(&a) && !scores.get(&a).is_some_and(|s| s.token == game.token && s.quit);
            let (sent, leaves) = &mut *relayed;
            relay(&mut m, &game, local, &here, &records(&net, EVENTS_KEY, Events::decode), &scores, sent, leaves, f64::from(now));
            m.inbox.clear(); // no mode reads it any more
            if !sent.items.is_empty() {
                net.publish_events(sent);
            }
            let taken = leaves.taken(&game.slots);
            net.publish_score(&Score { token: m.token, value: m.score, finished: m.finished, round: m.round, taken, quit: m.quit });
        }
        return;
    };
    let shown = if m.order.is_empty() {
        standings(&game, &scores, &names, local, mode.lower_wins())
    } else {
        ordered(&m.order, &game, &scores, &known.1, local)
    };
    m.standings = shown;
    // A player who left keeps its name until `relay` gives its mode the leave (Leaves).
    m.players.clone_from(&known.1);
    let round = m.round;
    // Gone: out of the session, or its mode quit the match (Match::quit).
    let here = |a: u64| names.contains_key(&a) && !scores.get(&a).is_some_and(|s| s.token == game.token && s.quit);
    m.rounds = (game.slots.iter())
        .map(|a| match scores.get(a).filter(|s| s.token == game.token) {
            _ if *a == local => round,
            _ if !here(*a) => u32::MAX,
            Some(s) => s.round,
            None => 0,
        })
        .collect();
    let (sent, leaves) = &mut *relayed;
    relay(&mut m, &game, local, &here, &records(&net, EVENTS_KEY, Events::decode), &scores, sent, leaves, f64::from(now));
    if !sent.items.is_empty() {
        net.publish_events(sent);
    }
    // online_hud shows the results, after the mode's own ending (`outro`) when the run was under way.
    if game.results && matches!(m.phase, Phase::Intro { .. } | Phase::Countdown { .. } | Phase::Running { .. }) {
        let outro = if matches!(m.phase, Phase::Running { .. }) { m.outro } else { 0. };
        if now >= *m.ending.get_or_insert(now + outro) {
            m.phase = Phase::Results { until: now + RESULTS_FOR };
        }
    }
    let taken = leaves.taken(&game.slots);
    net.publish_score(&Score { token: m.token, value: m.score, finished: m.finished, round: m.round, taken, quit: m.quit });
}

/// Host: whether the run is over. Everyone in it still here has finished, or all but one have and the last of the
/// others finished STRAGGLER ago (`since`: when the finished count last changed). Audit 10-28 #1: only a last holdout
/// gets the clock; Skate 2 S.K.A.T.E. counts a knocked-out player finished, so with 3 or 4 it ended the game a minute
/// after the first knockout.
fn run_over(finished: &[bool], since: &mut Option<f32>, now: f32) -> bool {
    let left = finished.iter().filter(|f| !**f).count();
    if left == 0 {
        return true;
    }
    if left > 1 || left == finished.len() {
        *since = None;
        return false;
    }
    now - *since.get_or_insert(now) >= STRAGGLER
}

/// Host only: end the run once everyone in it (still here) has finished (a straggler at most STRAGGLER after the
/// others) or the host's time is up, and clear the game once its results have been shown.
fn referee(
    net: Res<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    m: Res<Match>,
    time: Res<Time<Real>>,
    mut clear_at: Local<Option<(u64, f32)>>,
    mut others_done: Local<(u64, usize, Option<f32>)>,
) {
    let now = time.elapsed_secs();
    let (active, local, host) = net.mod_identity();
    let Some(game) = roster.state.game.as_ref().filter(|_| active && host) else { return };
    if game.results {
        match *clear_at {
            Some((token, at)) if token == game.token => {
                if now >= at {
                    roster.state.game = None;
                }
            }
            _ => *clear_at = Some((game.token, now + m.outro + RESULTS_FOR + 1.)),
        }
        return;
    }
    let scores = scores(&net);
    // Review 10-29: a Skate 1 row's script ends the host's mode as it finishes (step, GAP-1): still in it, finished.
    let playing = m.token == game.token && (m.mode.is_some() || m.finished);
    // A host that is not playing (its spawn failed) does not hold the others up.
    let finished: Vec<bool> = (game.slots.iter())
        .filter(|&&a| roster.members.iter().any(|p| p.actor == a) && (a != local || playing))
        .map(|a| scores.get(a).is_some_and(|s| s.token == game.token && s.finished))
        .collect();
    // The straggler clock restarts whenever another player finishes.
    let count = finished.iter().filter(|f| **f).count();
    if (others_done.0, others_done.1) != (game.token, count) {
        *others_done = (game.token, count, None);
    }
    let done = run_over(&finished, &mut others_done.2, now);
    if done && finished.iter().any(|f| !*f) {
        warn!("Match {:x}: a player never finished; counted finished {STRAGGLER} s after the others", game.token);
    }
    let time_up = playing && matches!(m.phase, Phase::Running { ends: Some(e), .. } if now >= e + LATE);
    if done || time_up {
        roster.state.game.as_mut().expect("checked above").results = true;
    }
}

/// Whether the skater is held still: the core's Intro and Countdown (an owning mode holds its own countdown), the
/// Results, and whatever the mode asks for while Running (its turn countdowns, its waiting players).
fn held(m: &Match) -> bool {
    if m.observer {
        return true;
    }
    match m.phase {
        Phase::Idle => false,
        // GAP-3 (online audit 10-28): an owning mode's own hold comes a frame or two into its run, after the core's
        // start teleport (a Skate 1 race's grid: its Lua never repositions online), so that teleport lands held and
        // physics::pin keeps the racer on a sloped grid; the Intro (INTRO_MIN) bridges to the mode's hold.
        Phase::Intro { .. } => true,
        Phase::Countdown { .. } => m.hold || !m.owned,
        Phase::Running { .. } => m.hold || m.ending.is_some(),
        Phase::Results { .. } => true,
    }
}

/// [`held`], or a Party Play screen (results dialog, its popups, the Change-settings setup, the card) owns the pad:
/// retail's FE states take the pad from the skater (skate.lua only UnpinAIs after the results, 1342-1345).
fn input_held(m: &Match, party_modal: bool) -> bool {
    held(m) || party_modal
}

/// The Replay Editor's ID_REPLAY_QUIT_CONF Yes (replay/hud.rs): an offline match ends as a dropped one does.
pub(crate) fn abandon(m: &mut Match) {
    drop_match(m);
}

/// A match ended before its Results (the host moved the session or dropped it): the modes' abandonchallenge, then
/// Online.LeaveGame, which puts the player back on the slot it began on (a session move then takes it on after its
/// delay). A trip back the Results' end already queued ([`leave_game`]) still happens. ponytail: a teleport still
/// waiting on a wipeout past the move's delay would land after the move's own.
pub(crate) fn drop_match(m: &mut Match) {
    let back = m.spawn.filter(|_| m.mode.is_some()).or(m.teleport);
    *m = Match::default();
    m.teleport = back;
}

/// The Results begin: the camera is this player's again. The skater stays where the match left it until they end
/// (G1, online audit 10-28: no Skate 3 graph repositions anyone in its results, and Skate 2's Lua leaves its waiting
/// players and its last attempter where they are).
fn results_begin(m: &mut Match, was_results: bool) {
    if matches!(m.phase, Phase::Results { .. }) && !was_results {
        m.watch = None;
    }
}

/// Online.LeaveGame as the Results end (G1, online audit 10-28): retail's load state (0x826E2A10) puts its black cover
/// and the tip loader up and takes the player back to free skate behind them. Bug 10-25 #5: back to the start slot it
/// began on (where retail lands it is UNRECOVERED). True: that trip is queued, so the loading screen goes up for it.
/// Skate 1 never gets here (its LeaveGame is free skate at once, [`step`]). Not traced for Skate 2 (its LeaveGame's
/// receiver is UNRECOVERED): it gets Skate 3's trip, as its join gets Skate 3's loading screen.
fn leave_game(m: &mut Match, was_results: bool) -> bool {
    was_results && m.mode.is_none() && !matches!(m.phase, Phase::Results { .. }) && trip_back(m)
}

/// The trip back to the start slot the match began on, queued; false without one.
fn trip_back(m: &mut Match) -> bool {
    m.teleport = m.spawn.or(m.teleport);
    m.spawn.is_some()
}

/// Skate 3's rule (user 10-28): a Skate 1 / Skate 2 row's trip to its start slot is behind the loading screen, as a
/// Skate 3 row's is ([`starts_covered`]); how the old games hid it is UNRECOVERED. A Skate 2 Best Trick / Jam start is
/// hidden by its own fade instead ([`contest_fading`]).
fn start_trip(m: &Match, game: Option<u8>) -> bool {
    matches!(game, Some(1 | 2))
        && m.teleport.is_some()
        && matches!(m.phase, Phase::Intro { .. })
        && !(game == Some(2) && matches!(m.mode, Some(Mode::BestTrick | Mode::Jam)))
}

/// Whether a mode's teleport must wait: a teleport is in flight (the engine's reset teleport, category Teleport /
/// state 702, or one already queued), which would land after it. A bail does not hold it: retail's RepositionAI path
/// (Teleporter 0x82864330 -> TeleportPlayer 0x82864038 -> actor Teleport 0x825926f8) has no wipeout gate, and the
/// physics teleport ends the bail (reset_player: the wipeout systems reset), so the bail's own reset never follows
/// (`mode_harness::c2_a_teleport_mid_bail_lands_and_stays`). RepositionAI moves the skater whatever its board state
/// (online audit 10-06, S2 R1 / D5); Skate 2 / Skate 1 glue gates on this too.
pub(crate) fn teleport_blocked(skater: &crate::physics::SkaterRuntime) -> bool {
    let teleporting = skater.player_input.physical.filtered_state_0 == skate_core::physics::filtered_state::FilteredCategory::Teleport as u32;
    teleporting || skater.player_input.pending_teleport().is_some()
}

/// How long into the run a Skate 3 start's loading screen waits for its mode's own to take over ([`starts_covered`]).
const TAKEOVER: f32 = 0.5;

/// User 10-26: whether this match's start is behind the loading screen. Retail opens it natively as a challenge starts
/// and the Lua repositions under it (CloseLoadingScreen after); a Skate 3 row's mode (`owned`, on a Skate 3 map) puts
/// up its own only as its run begins, so the cover is up from the start until that mode's takes over (its LoadingHold
/// up, or its intro claimed), at most TAKEOVER into the run. The start slot waits for it.
fn starts_covered(m: &Match, skate3: bool, hold: bool, now: f32) -> bool {
    skate3
        && m.owned
        && m.mode.is_some()
        && match m.phase {
            Phase::Intro { .. } => true,
            Phase::Running { started, .. } => !hold && !m.intro && now - started < TAKEOVER,
            _ => false,
        }
}

/// s2fix R1 (10-28): a Skate 2 Best Trick / Jam start slot waits for its mode's FadeOut to be black (contest_jam.lua
/// 7: initial FadeOut 0.5, setup_round's reposition under it).
fn contest_fading(m: &Match, game: Option<u8>, now: f32) -> bool {
    game == Some(2)
        && matches!(m.mode, Some(Mode::BestTrick | Mode::Jam))
        && matches!(m.phase, Phase::Intro { since } if now - since < crate::skate2_contests::CONTEST_FADE)
}

#[allow(clippy::too_many_arguments)]
fn step(
    mut m: ResMut<Match>,
    (time, clock): (Res<Time<Real>>, Res<MatchClock>),
    input: Option<ResMut<crate::input::ControllerInput>>,
    skater: Option<ResMut<crate::physics::SkaterRuntime>>,
    party: Option<Res<crate::party_play::PartyPlay>>,
    spectate: Option<Res<Spectate>>,
    (map, hold, covered, cover): (
        Option<Res<crate::map_transition::CurrentMap>>,
        Option<Res<crate::tip_loader::LoadingHold>>,
        Option<Res<crate::tip_loader::Covered>>,
        Option<ResMut<crate::tip_loader::MatchCover>>,
    ),
    (mut was_results, mut leaving, mut arriving, mut landed_at): (Local<bool>, Local<bool>, Local<bool>, Local<Option<f32>>),
) {
    let now = time.elapsed_secs() - clock.paused;
    let game = map.as_ref().map(|map| crate::skate2_hud::map_game(map));
    // GAP-1 (online audit 10-28): a Skate 1 row's board ("Returning to Lobby in N") is its exit screen, so no core
    // Results after it. Skate 3's rule (user 10-28): then the trip back to the start slot behind the loading screen, as
    // after a Skate 3 row's results (Skate 1's own LeaveGame left the player where it stood: U-2).
    if game == Some(1) && m.owned && m.finished && m.mode.is_some() {
        (m.mode, m.phase) = (None, Phase::Idle);
        *leaving |= trip_back(&mut m);
    }
    if m.mode.is_some() {
        advance(&mut m, now);
    }
    // Bug 10-25 #5: last frame's phase, not this one's on entry: online the Results begin in `sync`, before this.
    results_begin(&mut m, *was_results);
    *leaving |= leave_game(&mut m, *was_results);
    *was_results = matches!(m.phase, Phase::Results { .. });
    if m.token == 0 && !m.outbox.is_empty() {
        m.outbox.clear(); // solo: no one to send to
    }
    let start_cover = starts_covered(&m, game == Some(3), hold.is_some_and(|h| h.0), now);
    *arriving |= start_trip(&m, game);
    // The trip back, or a Skate 1 / 2 start's trip, stays behind the loading screen until it has landed (no teleport
    // left in flight) and settled (tip_loader::SETTLE); a new match's start ends the trip back (it has its own).
    let landed = m.teleport.is_none() && !skater.as_deref().is_some_and(teleport_blocked);
    let tripping = *leaving || *arriving;
    if !tripping || !landed {
        *landed_at = None;
    }
    let settled = tripping && landed && now - *landed_at.get_or_insert(now) >= crate::tip_loader::SETTLE;
    if *leaving && (m.mode.is_some() || settled) {
        *leaving = false;
    }
    if *arriving && (m.mode.is_none() || settled) {
        *arriving = false;
    }
    let cover_up = start_cover || *leaving || *arriving;
    if let Some(mut cover) = cover {
        cover.set_if_neq(crate::tip_loader::MatchCover(cover_up));
    }
    // Without the loading screen's resource (a test app) nothing waits for it.
    let unseen = (!cover_up || covered.is_none_or(|c| c.0)) && !contest_fading(&m, game, now);
    if let Some(mut skater) = skater
        && let Some(at) = m.teleport.filter(|_| unseen && !teleport_blocked(&skater))
    {
        m.teleport = None;
        let on_board = !std::mem::take(&mut m.teleport_on_foot);
        let marks = std::mem::take(&mut m.teleport_marks);
        match skater.travel_to_as(at, on_board) {
            Ok(()) => skater.mark_after_travel |= marks,
            Err(e) => warn!("Match teleport: {e}"),
        }
    }
    // Neutral input, as the tutorial's PinPlayer; physics::pin also holds a skater the held match teleported still.
    let modal = party.is_some_and(|p| p.modal);
    let spectating = spectate.is_some_and(|s| s.0.is_some());
    // Review I4 (online fixes 11-01): the trip back after the Results lands held too, behind its loading screen, as the
    // match's start did (a start slot whose board only settles held).
    let hold = input_held(&m, modal) || spectating || *leaving;
    // G9 (online audit 10-28): a Skate 3 row runs its intro, countdown and turn waits inside Running (`Match::gestures`).
    let own_wait = m.gestures && matches!(m.phase, Phase::Running { .. });
    let gestures = !modal && !m.observer && (spectating || own_wait || matches!(m.phase, Phase::Intro { .. } | Phase::Countdown { .. }));
    if let Some(mut input) = input.filter(|i| (i.match_hold, i.match_gestures) != (hold, gestures)) {
        (input.match_hold, input.match_gestures) = (hold, gestures);
    }
}

/// Online / ChallengeManager.SwitchCamera(player): the gameplay camera shows `Match::watch`'s skater (Spot Battle's
/// and 1-Up's turns, S.K.A.T.E.'s and Hall of Meat's waiting players, Skate 1's scripts) through that player's own
/// camera, as it streams in their pose packets (user 10-09: framing their body swung with every turn and flip). The
/// skater is [`Watching::shown`], the asked one ([`ask_watch`]) once its samples come at the watched rate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn observer_camera(
    watching: Res<Watching>,
    views: Res<crate::multiplayer::RemoteCameras>,
    history: Res<crate::presentation::Presentation>,
    replay: Res<crate::replay::Replay>,
    time: Res<Time<Fixed>>,
    real: Res<Time<Real>>,
    mut held: Local<(f32, Option<(u64, Transform)>)>,
    remotes: Query<(&crate::multiplayer::appearance::RemoteCharacter, &GlobalTransform)>,
    mut cameras: Query<&mut Transform, With<crate::camera::GameplayCamera>>,
    mut shown: ResMut<ObserverView>,
    customiser: Option<Res<crate::customiser::Customiser>>,
) {
    *shown = ObserverView::default();
    // Edit Skater and the Replay Editor frame their own camera (camera::present); watching resumes when they close.
    let Some(actor) = watching.shown.filter(|_| !replay.active && customiser.is_none_or(|c| !c.open)) else { return };
    // A teleport clears the watched player's camera rows until its next ones come (multiplayer::push_root): its last
    // view holds that long, up to HOLD_VIEW_S, rather than framing the root for a few frames.
    let view = match views.0.get(&actor) {
        Some(&view) => {
            *held = (0., Some((actor, view)));
            Some(view)
        }
        None => {
            held.0 += real.delta_secs();
            held.1.filter(|&(a, _)| a == actor && held.0 <= HOLD_VIEW_S).map(|(_, view)| view)
        }
    };
    if let Some(view) = view {
        for mut transform in &mut cameras {
            (transform.translation, transform.rotation) = (view.translation, view.rotation);
            *shown = ObserverView(Some(*transform));
        }
        return;
    }
    // ponytail: a sender with no camera row yet (its first packets): this player's framing on the watched root.
    let Some((_, watched)) = remotes.iter().find(|(r, _)| r.0 == actor) else { return };
    let Some((previous, current, alpha)) = history.view(&replay, time.overstep_fraction()) else { return };
    let root = crate::presentation::blend(previous.root, current.root, alpha);
    let watched = watched.compute_transform();
    for mut transform in &mut cameras {
        *transform = reframe(root, *transform, watched);
        *shown = ObserverView(Some(*transform));
    }
}

/// How long the camera holds the watched player's last streamed view while its camera rows are missing.
const HOLD_VIEW_S: f32 = 0.3;

/// `camera` as framed on `from`, moved onto `to` (translation and rotation; scale kept).
pub(crate) fn reframe(from: Transform, camera: Transform, to: Transform) -> Transform {
    let back = from.rotation.inverse();
    Transform {
        translation: to.translation + to.rotation * (back * (camera.translation - from.translation)),
        rotation: (to.rotation * back * camera.rotation).normalize(),
        scale: camera.scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_numbering_its_events_at_the_top_cannot_overflow() {
        let (mut leaves, mut out) = (Leaves::default(), vec![]);
        let items = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        leaves.take(1, u32::MAX - 1, &items, &mut out);
        leaves.take(1, u32::MAX, &items, &mut out);
        assert_eq!(Mode::Skate.show(i64::MIN), "S.K.A.T.E.", "a peer's @score at i64::MIN");
    }

    /// Online safety P7: a peer's "@on" can't name a slot no session has nor its own, nor nest deep enough to blow the
    /// stack; made-up marks stay few.
    #[test]
    fn a_peer_s_made_up_sent_on_events_are_dropped() {
        let (mut leaves, mut out) = (Leaves::default(), vec![]);
        let deep = (0..2_000).map(|i| format!("@on {} {i} ", 2 + i % 2)).collect::<String>() + "x";
        leaves.take(1, 0, &["@on 999999999 0 x".into(), "@on 1 9 x".into(), deep], &mut out);
        assert!(out.is_empty(), "{out:?}");
        let marks: Vec<String> = (0..10_000).map(|i| format!("@gone {i}")).collect();
        leaves.take(1, 3, &marks, &mut out);
        assert!(leaves.marked.len() <= EVENTS_BACKLOG);
    }

    /// Online safety 10-26: an event sent on as a player's counts only with that player's own signature (its key its
    /// first event), and only straight after what this console has of a player still here. A made-up "Bailed" (a
    /// S.K.A.T.E. letter), a made-up host snapshot (every guest's contest frozen), another event's signature, a number
    /// far past its own (silencing it) or one ahead of its own all go; its real one counts the moment it comes.
    #[test]
    fn only_a_players_own_signed_events_are_sent_on() {
        let mut player = Leaves::default();
        let key = player.key_item().expect("random source");
        let real = player.sign(0, 1, "real");
        let (mut leaves, mut out) = (Leaves::default(), vec![]);
        leaves.take(0, 0, &[key], &mut out);
        assert!(out.is_empty(), "its key");
        let mut peer = Leaves::default();
        let peer_key = peer.key_item().unwrap();
        let made_up = [
            "@on 0 1 forged".to_string(),
            format!("@on 0 1 {} forged", real.split_once(' ').unwrap().0),
            format!("@on 0 1 {}", peer.sign(0, 1, "forged")),
            format!("@on 0 4000000000 {real}"),
            format!("@on 0 2 {}", player.sign(0, 2, "ahead")),
        ];
        leaves.take(1, 0, &made_up, &mut out);
        assert!(out.is_empty(), "{out:?}");
        leaves.take(1, 5, &[format!("@on 0 1 {real}")], &mut out);
        assert_eq!(out, [(0, "real".to_string())], "its own, sent on while it is still here: at once");
        leaves.take(0, 1, &[real.clone(), player.sign(0, 2, "next")], &mut out);
        assert_eq!(out, [(0, "real".to_string()), (0, "next".into())], "its own copy is not taken twice");
        // Nor can a peer swap in a key of its own for another player's.
        leaves.take(1, 6, &[format!("@on 0 3 {peer_key}"), format!("@on 0 3 {}", peer.sign(0, 3, "x"))], &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
    }

    /// What a console's stream says with every signature off (its own and those of what it sends on), its key left out.
    fn bodies(s: &Events) -> Vec<String> {
        fn unsigned(item: &str) -> String {
            let body = signed(item).map_or(item, |(_, b)| b);
            match body.strip_prefix(SENT_ON).map(|on| on.splitn(3, ' ').collect::<Vec<_>>()) {
                Some(p) if p.len() == 3 => format!("{SENT_ON}{} {} {}", p[0], p[1], unsigned(p[2])),
                _ => body.to_owned(),
            }
        }
        s.items.iter().filter(|e| !e.starts_with(KEY)).map(|e| unsigned(e)).collect()
    }
    use serde_json::json;

    /// Online audit 10-12: Spectate follows the player whose name was picked, 2-10 players, through churn. Before, the
    /// pick indexed the session as of the pick, not the list on screen: a join or leave while the menu was up followed
    /// the next player down (a leaver's row: the player after it).
    #[test]
    fn spectate_follows_the_picked_name() {
        let member = |actor: u64| skate_net::roster::Member { actor, name: format!("P{actor}"), steam: 0, host: actor == 1 };
        for n in 2..=10u64 {
            let mut roster = OnlineRoster::default();
            (roster.local_actor, roster.members) = (2, (1..=n).map(member).collect());
            let listed: Vec<u64> = spectatable(&roster).into_iter().map(|(a, _)| a).collect();
            assert_eq!(listed.len() as u64, n);
            assert_eq!(spectate_pick(&listed, 0, &roster), Some(1), "{n}: the host");
            assert_eq!(spectate_pick(&listed, 1, &roster), None, "{n}: our own name, our own camera");
            // While the menu is up: the host leaves (migrates), the last player leaves, a player joins.
            roster.members.retain(|m| m.actor != 1 && m.actor != n);
            roster.members.push(member(11));
            for (i, &a) in listed.iter().enumerate() {
                let want = Some(a).filter(|&a| a != 1 && a != 2 && a != n);
                assert_eq!(spectate_pick(&listed, i, &roster), want, "{n} players, row {i} (P{a})");
            }
        }
    }
    /// User 10-24 (build 10): spectating, the audio listens from the camera on the screen, as the native (at, position)
    /// the audio observation carries; on our own camera nothing is overridden.
    #[test]
    fn the_spectate_camera_is_the_listener() {
        let view = Transform::from_xyz(1., 2., 3.).looking_at(Vec3::new(1., 2., 13.), Vec3::Y);
        let (at, position) = ObserverView(Some(view)).listener().unwrap();
        assert!(Vec3::from_array(at).distance(Vec3::Z) < 1e-5, "{at:?}");
        assert_eq!(position, [1., 2., 3.]);
        assert_eq!(ObserverView(None).listener(), None);
    }

    /// Reviewer 10-25: the watched player teleports, which clears its camera rows until the next ones come; the camera
    /// holds its last streamed view (HOLD_VIEW_S) instead of framing the root, and only for that player.
    #[test]
    fn the_watched_view_holds_through_a_gap_in_its_camera_rows() {
        let mut world = World::new();
        let view = Transform::from_xyz(5., 2., 0.);
        world.insert_resource(Watching { shown: Some(7), ..default() });
        world.insert_resource(crate::multiplayer::RemoteCameras([(7, view)].into()));
        world.init_resource::<crate::presentation::Presentation>();
        world.init_resource::<crate::replay::Replay>();
        world.init_resource::<Time<Fixed>>();
        world.init_resource::<ObserverView>();
        let mut real = Time::<Real>::default();
        real.update_with_duration(std::time::Duration::ZERO);
        world.insert_resource(real);
        let camera = world.spawn((Transform::default(), crate::camera::GameplayCamera)).id();
        // One system across frames: its Local (the held view) lives as long as it does, as in the app.
        let system = world.register_system(observer_camera);
        let mut frame = |world: &mut World, ms: u64| {
            world.resource_mut::<Time<Real>>().update_with_duration(std::time::Duration::from_millis(ms));
            world.run_system(system).unwrap();
            world.resource::<ObserverView>().0.map(|t| t.translation)
        };
        assert_eq!(frame(&mut world, 16), Some(view.translation), "the streamed view");
        world.resource_mut::<crate::multiplayer::RemoteCameras>().0.clear();
        assert_eq!(frame(&mut world, 100), Some(view.translation), "0.1 s without rows: held");
        assert_eq!(frame(&mut world, 150), Some(view.translation), "0.25 s: held");
        assert_eq!(frame(&mut world, 100), None, "past HOLD_VIEW_S: no view (no root to frame here)");
        assert_eq!(world.get::<Transform>(camera).unwrap().translation, view.translation);
        world.resource_mut::<crate::multiplayer::RemoteCameras>().0.insert(7, view);
        frame(&mut world, 16);
        world.resource_mut::<crate::multiplayer::RemoteCameras>().0.clear();
        world.resource_mut::<Watching>().shown = Some(8);
        assert_eq!(frame(&mut world, 16), None, "another player's last view is not held");
    }

    /// Review 10-29 (I-2): Edit Skater and the Replay Editor stay open online; spectating someone overwrote their camera
    /// with that player's live view every frame. While either is open the watched view waits.
    #[test]
    fn edit_skater_and_the_replay_editor_keep_their_own_camera() {
        let mut world = World::new();
        let view = Transform::from_xyz(5., 2., 0.);
        world.insert_resource(Watching { shown: Some(7), ..default() });
        world.insert_resource(crate::multiplayer::RemoteCameras([(7, view)].into()));
        world.init_resource::<crate::presentation::Presentation>();
        world.init_resource::<crate::replay::Replay>();
        world.init_resource::<Time<Fixed>>();
        world.init_resource::<ObserverView>();
        world.init_resource::<Time<Real>>();
        let camera = world.spawn((Transform::default(), crate::camera::GameplayCamera)).id();
        let system = world.register_system(observer_camera);
        world.resource_mut::<crate::replay::Replay>().active = true;
        world.run_system(system).unwrap();
        assert_eq!(world.resource::<ObserverView>().0, None, "the Replay Editor's camera");
        assert_eq!(world.get::<Transform>(camera).unwrap().translation, Vec3::ZERO);
        world.resource_mut::<crate::replay::Replay>().active = false;
        let mut parts = crate::customiser_parts::Parts::default();
        parts.library.defaults.insert("male".into(), serde_json::json!({}));
        world.insert_resource(crate::customiser::Customiser::for_test(&parts, Vec::new(), "male"));
        world.run_system(system).unwrap();
        assert_eq!(world.resource::<ObserverView>().0, None, "Edit Skater's camera");
        assert_eq!(world.get::<Transform>(camera).unwrap().translation, Vec3::ZERO);
        world.resource_mut::<crate::customiser::Customiser>().open = false;
        world.run_system(system).unwrap();
        assert_eq!(world.resource::<ObserverView>().0.map(|t| t.translation), Some(view.translation), "closed: watching again");
    }

    /// Audit 10-30: a match observer asked for a player who waits for its turn is shown the player that one watches.
    #[test]
    fn an_observer_follows_a_waiting_player_to_the_player_up() {
        let watches = |pairs: &'static [(u64, u64)]| move |id: u64| pairs.iter().find(|p| p.0 == id).map(|p| p.1);
        assert_eq!(followed(1, Some(2), watches(&[])), Some(2), "2 is up");
        assert_eq!(followed(1, Some(2), watches(&[(2, 3)])), Some(3), "Spot Battle's waiting slot: the player up");
        assert_eq!(followed(1, Some(2), watches(&[(2, 3), (3, 4)])), Some(4), "a chain");
        assert_eq!(followed(1, Some(2), watches(&[(2, 3), (3, 2)])), Some(3), "a loop stops");
        assert_eq!(followed(1, Some(2), watches(&[(2, 1)])), Some(2), "never round to us");
        assert_eq!(followed(1, None, watches(&[(2, 3)])), None);
    }
    #[test]
    fn players_are_held_through_countdowns_and_sent_back_to_their_spawn_at_the_end() {
        let mut m = Match::default();
        m.start(Mode::Jam, "r", 0.);
        assert!(held(&m), "the intro");
        m.phase = Phase::Countdown { ends: 3. };
        assert!(held(&m), "the countdown");
        m.phase = Phase::Running { started: 3., ends: None };
        assert!(!held(&m), "the run");
        m.hold = true;
        assert!(held(&m), "the mode's own countdowns and waits");
        (m.spawn, m.watch) = (Some([[1.; 4]; 4]), Some(2));
        m.phase = Phase::Results { until: 9. };
        results_begin(&mut m, false);
        assert!(!leave_game(&mut m, false));
        assert_eq!((m.teleport, m.watch), (None, None), "G1: the camera ours, the skater where the match left it");
        m.hold = false;
        assert!(held(&m), "the results");
        (m.mode, m.phase) = (None, Phase::Idle);
        assert!(leave_game(&mut m, true), "the Results' end");
        assert_eq!(m.teleport, Some([[1.; 4]; 4]), "back to the start slot");
        m.teleport = None;
        assert!(!leave_game(&mut m, false), "once");
        assert!(!held(&m));
        m.start(Mode::Skate, "s1", 0.);
        m.owned = true;
        assert!(held(&m), "GAP-3: an owning mode's start teleport lands held, so physics::pin keeps it there");
        m.phase = Phase::Running { started: INTRO_MIN, ends: None };
        assert!(!held(&m), "then the mode holds its own");
    }
    /// Bug 10-25 #5 (build 10): after an online Spot Battle the player stayed at its waiting spot. Online the Results
    /// begin in `sync` (the host's `Game::results`), which runs before `step`; step read the phase on entry as already
    /// Results, so Online.LeaveGame's trip back to the start slot never happened. G1 (online audit 10-28): the trip is
    /// as the Results end, not as they begin (the world jumped behind the results screen).
    #[test]
    fn online_results_send_the_player_back_to_its_start_slot() {
        fn host_ends_the_run(mut m: ResMut<Match>) {
            if m.finished && matches!(m.phase, Phase::Running { .. }) {
                m.phase = Phase::Results { until: 100. };
            }
        }
        let mut app = App::new();
        app.init_resource::<Time<Real>>().init_resource::<MatchClock>().add_systems(Update, (host_ends_the_run, step).chain());
        let mut m = Match::default();
        m.start(Mode::SpotBattle, "ots_dwtn_01_ol", 0.);
        let start = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [-113.98, 31.51, 160.54, 1.]];
        (m.token, m.owned, m.spawn, m.watch) = (7, true, Some(start), Some(1));
        m.phase = Phase::Running { started: 0., ends: None };
        app.insert_resource(m);
        app.update();
        assert_eq!(app.world().resource::<Match>().teleport, None, "the run");
        app.world_mut().resource_mut::<Match>().finished = true;
        app.update();
        let m = app.world().resource::<Match>();
        assert!(matches!(m.phase, Phase::Results { .. }));
        assert_eq!((m.teleport, m.watch), (None, None), "the Results: the camera ours, the skater where it is");
        app.world_mut().resource_mut::<Match>().phase = Phase::Results { until: 0. };
        app.update();
        let m = app.world().resource::<Match>();
        assert_eq!((m.mode, m.phase.clone(), m.teleport), (None, Phase::Idle, Some(start)), "their end: back to the start slot");
    }

    /// G1 (online audit 10-28): the trip back as the Results end goes behind the loading screen (retail's load state:
    /// black cover, tip loader) and waits for it; the screen stays until the trip has landed, and the host clearing the
    /// game meanwhile (drop_match) doesn't cancel it.
    #[test]
    fn the_trip_back_after_the_results_is_behind_the_loading_screen() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>().init_resource::<MatchClock>().add_systems(Update, step);
        app.insert_resource(crate::tip_loader::MatchCover(false)).insert_resource(crate::tip_loader::Covered(false));
        app.insert_resource(crate::input::ControllerInput::default());
        let pad_held = |app: &App| app.world().resource::<crate::input::ControllerInput>().match_hold;
        let mut m = Match::default();
        m.start(Mode::SpotBattle, "ots_dwtn_01_ol", 0.);
        let start = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [4., 5., 6., 1.]];
        (m.token, m.spawn, m.phase) = (7, Some(start), Phase::Results { until: 1000. });
        app.insert_resource(m);
        app.update();
        assert!(!app.world().resource::<crate::tip_loader::MatchCover>().0, "the results screen, no loading screen");
        app.world_mut().resource_mut::<Match>().phase = Phase::Results { until: 0. };
        app.update();
        assert_eq!(app.world().resource::<Match>().teleport, Some(start));
        assert!(app.world().resource::<crate::tip_loader::MatchCover>().0, "the loading screen for the trip");
        assert!(pad_held(&app), "the trip back lands held");
        drop_match(&mut app.world_mut().resource_mut::<Match>());
        app.update();
        assert_eq!(app.world().resource::<Match>().teleport, Some(start), "the host cleared the game: still queued");
        assert!(app.world().resource::<crate::tip_loader::MatchCover>().0);
        app.world_mut().resource_mut::<Match>().teleport = None; // the physics took it (no skater in this app)
        app.update();
        assert!(app.world().resource::<crate::tip_loader::MatchCover>().0, "landed: retail's black holds 15 frames more");
        app.world_mut().resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(crate::tip_loader::SETTLE - 0.01));
        app.update();
        assert!(app.world().resource::<crate::tip_loader::MatchCover>().0, "still settling");
        assert!(pad_held(&app), "held while it settles");
        app.world_mut().resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(0.02));
        app.update();
        assert!(!app.world().resource::<crate::tip_loader::MatchCover>().0, "landed and settled: the screen goes");
        assert!(!pad_held(&app), "and the skater is the player's again");
    }

    /// GAP-1 (online audit 10-28): on the Skate 1 map the row's board is its exit screen: once its script has left the
    /// game (finished) there is no core Results after it (no 8 s blank hold). Skate 3's rule (user 10-28): the trip back
    /// to the start slot behind the loading screen follows, as after a Skate 3 row's results.
    #[test]
    fn a_skate1_row_goes_back_to_its_start_behind_the_loading_screen_after_its_board() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>().init_resource::<MatchClock>().add_systems(Update, step);
        app.insert_resource(crate::tip_loader::MatchCover(false)).insert_resource(crate::tip_loader::Covered(false));
        let path = std::path::PathBuf::from("maps/Skate_1_San_Vanelona.skate");
        app.insert_resource(crate::map_transition::CurrentMap { path: Some(path), name: String::new(), spawn: [0.; 3], heading: 0., generation: 0 });
        let mut m = Match::default();
        m.start(Mode::DeathRace, "ol_deathrace_oldtown_spillway", 0.);
        let start = Some([[2.; 4]; 4]);
        (m.token, m.owned, m.spawn, m.phase) = (7, true, start, Phase::Running { started: 0., ends: None });
        app.insert_resource(m);
        app.update();
        assert!(matches!(app.world().resource::<Match>().phase, Phase::Running { .. }), "the run and its board");
        assert!(!app.world().resource::<crate::tip_loader::MatchCover>().0);
        app.world_mut().resource_mut::<Match>().finished = true;
        app.update();
        let m = app.world().resource::<Match>();
        assert_eq!((m.mode, m.phase.clone(), m.teleport), (None, Phase::Idle, start), "no core Results; the trip back");
        assert!(!held(m));
        assert!(app.world().resource::<crate::tip_loader::MatchCover>().0, "behind the loading screen");
    }

    /// Skate 3's rule (user 10-28): a Skate 1 / Skate 2 row's start slot is taken behind the loading screen, as a Skate 3
    /// row's is; the screen stays until the trip has landed and settled. A Skate 2 Best Trick / Jam start is hidden by
    /// its own fade instead.
    #[test]
    fn a_skate1_or_skate2_start_is_behind_the_loading_screen() {
        for (map, mode, covered) in [
            ("Skate_1_San_Vanelona", Mode::Skate, true),
            ("New_San_Vanelona", Mode::SpotBattle, true),
            ("New_San_Vanelona", Mode::Jam, false),
        ] {
            let mut app = App::new();
            app.init_resource::<Time<Real>>().init_resource::<MatchClock>().add_systems(Update, step);
            app.insert_resource(crate::tip_loader::MatchCover(false)).insert_resource(crate::tip_loader::Covered(false));
            let path = std::path::PathBuf::from(format!("maps/{map}.skate"));
            app.insert_resource(crate::map_transition::CurrentMap { path: Some(path), name: String::new(), spawn: [0.; 3], heading: 0., generation: 0 });
            let mut m = Match::default();
            m.start(mode, "row", 0.);
            (m.token, m.spawn) = (7, Some([[3.; 4]; 4]));
            m.teleport = m.spawn;
            app.insert_resource(m);
            app.update();
            let cover = |app: &App| app.world().resource::<crate::tip_loader::MatchCover>().0;
            assert_eq!(cover(&app), covered, "{map} {mode:?}: the start");
            if !covered {
                continue;
            }
            app.world_mut().resource_mut::<Match>().teleport = None; // the physics took it (no skater in this app)
            app.update();
            assert!(cover(&app), "{map}: landed, settling");
            app.world_mut().resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(crate::tip_loader::SETTLE + 0.01));
            app.update();
            assert!(!cover(&app), "{map}: landed and settled: the intro shows");
        }
    }

    /// s2fix R1 (10-28): a Skate 2 Best Trick / Jam start teleport waits for the mode's 0.5 s FadeOut to be black.
    #[test]
    fn a_skate2_contest_start_waits_for_its_fade() {
        let mut m = Match::default();
        m.start(Mode::Jam, "jam", 0.);
        let fade = crate::skate2_contests::CONTEST_FADE;
        assert!(contest_fading(&m, Some(2), 0.1), "black not yet");
        assert!(!contest_fading(&m, Some(2), fade), "black");
        assert!(!contest_fading(&m, Some(1), 0.1), "Skate 1's Jam has its own flythrough");
        m.start(Mode::DeathRace, "dr", 0.);
        assert!(!contest_fading(&m, Some(2), 0.1));
    }

    /// G9 (online audit 10-28): a Skate 3 row holds its own intro, countdown and turn waits inside Running; the emotes
    /// go through there as in the core's (bug 10-17 #44 / #45), and not once the mode stops asking.
    #[test]
    fn a_modes_own_wait_lets_the_emotes_through() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>().init_resource::<MatchClock>().add_systems(Update, step);
        app.insert_resource(crate::input::ControllerInput::default());
        let mut m = Match::default();
        m.start(Mode::SpotBattle, "ots_dwtn_01_ol", 0.);
        (m.owned, m.hold, m.gestures, m.phase) = (true, true, true, Phase::Running { started: 0., ends: None });
        app.insert_resource(m);
        app.update();
        let input = app.world().resource::<crate::input::ControllerInput>();
        assert_eq!((input.match_hold, input.match_gestures), (true, true), "the mode's countdown");
        app.world_mut().resource_mut::<Match>().gestures = false;
        app.update();
        let input = app.world().resource::<crate::input::ControllerInput>();
        assert_eq!((input.match_hold, input.match_gestures), (true, false), "its results");
    }
    /// I1: a Party Play S.K.A.T.E. game over (owned, Running, the mode holding nothing, never `finished`) is still held
    /// while the results dialog or its setup is up.
    #[test]
    fn a_party_screen_holds_the_skater() {
        let mut m = Match::default();
        m.start(Mode::Skate, "skate_univ_02", 0.);
        (m.owned, m.phase) = (true, Phase::Running { started: 1., ends: None });
        assert!(!input_held(&m, false), "the game over leaves the skater free");
        assert!(input_held(&m, true), "the dialog owns the pad");
        assert!(input_held(&Match::default(), true), "the setup over free skate too");
    }
    #[test]
    fn phases_run_intro_to_results_to_idle() {
        let mut m = Match::default();
        m.start(Mode::DeathRace, "dr", 0.);
        m.time_limit = Some(60.);
        advance(&mut m, 0.1);
        assert!(matches!(m.phase, Phase::Intro { .. }), "a mode has a moment to claim the intro");
        m.intro = true;
        advance(&mut m, 5.);
        assert!(matches!(m.phase, Phase::Intro { .. }), "the intro is playing");
        m.intro = false;
        advance(&mut m, 6.);
        assert_eq!(m.phase, Phase::Countdown { ends: 6. + COUNTDOWN });
        advance(&mut m, 7.);
        assert!(matches!(m.phase, Phase::Countdown { .. }));
        let go = 6. + COUNTDOWN;
        advance(&mut m, go);
        assert_eq!(m.phase, Phase::Running { started: go, ends: Some(go + 60.) });
        assert!(m.running(Mode::DeathRace) && !m.running(Mode::HallOfMeat));
        m.finished = true;
        advance(&mut m, 20.);
        assert!(matches!(m.phase, Phase::Results { .. }), "finishing ends a solo run early");
        advance(&mut m, 20. + RESULTS_FOR);
        assert_eq!((m.mode, m.phase.clone()), (None, Phase::Idle));
        // A stuck intro gives way; online, time up only finishes this player (the host ends the run).
        m.start(Mode::Skate, "s", 0.);
        m.intro = true;
        advance(&mut m, INTRO_MAX);
        assert!(matches!(m.phase, Phase::Countdown { .. }));
        m.token = 7;
        m.phase = Phase::Running { started: 0., ends: Some(10.) };
        advance(&mut m, 11.);
        assert!(m.finished && matches!(m.phase, Phase::Running { .. }));
    }

    /// A mode's own ending (`outro`: S2 Death Race's end_heat + cleanup_heat) runs between the run's end and the
    /// Results, everyone held; a match the host drops (a session move) sends the player back to its slot.
    #[test]
    fn a_modes_outro_comes_before_the_results_and_a_dropped_match_leaves_the_game() {
        let mut m = Match::default();
        m.start(Mode::DeathRace, "dr", 0.);
        (m.phase, m.outro, m.finished) = (Phase::Running { started: 0., ends: None }, 5., true);
        advance(&mut m, 10.);
        assert_eq!((m.ending, held(&m)), (Some(15.), true), "the outro, held");
        advance(&mut m, 14.9);
        assert!(matches!(m.phase, Phase::Running { .. }));
        advance(&mut m, 15.);
        assert_eq!(m.phase, Phase::Results { until: 15. + RESULTS_FOR });
        m.start(Mode::DeathRace, "dr", 0.);
        (m.token, m.spawn, m.phase) = (7, Some([[2.; 4]; 4]), Phase::Running { started: 0., ends: None });
        drop_match(&mut m);
        assert_eq!((m.mode, m.token, m.teleport), (None, 0, Some([[2.; 4]; 4])), "Online.LeaveGame: back to the start slot");
        let mut watcher = Match { token: 7, observer: true, ..default() };
        drop_match(&mut watcher);
        assert_eq!(watcher.teleport, None, "an observer never left its spot");
    }

    /// A mode that owns its sequencing (Skate 1's Lua) skips the core's countdown; finishing ends a solo run.
    #[test]
    fn an_owned_match_runs_its_own_intro_and_countdown() {
        let mut m = Match::default();
        m.start(Mode::BestTrick, "besttrick_financial_library", 0.);
        m.owned = true;
        m.time_limit = Some(900.);
        advance(&mut m, 0.1);
        assert!(matches!(m.phase, Phase::Intro { .. }));
        advance(&mut m, INTRO_MIN);
        assert_eq!(m.phase, Phase::Running { started: INTRO_MIN, ends: Some(INTRO_MIN + 900.) }, "no core countdown");
        m.finished = true;
        advance(&mut m, 400.);
        assert!(matches!(m.phase, Phase::Results { .. }));
    }

    /// User 10-26: a Skate 3 row's players reached their start slots before its loading screen came up; retail opens it
    /// as the challenge starts. Up from the start until the mode's own takes over; the start slot waits for it (step).
    #[test]
    fn a_skate3_start_is_behind_the_loading_screen() {
        let mut m = Match::default();
        m.start(Mode::OneUp, "1up_dwtn_01", 0.);
        m.owned = true;
        assert!(starts_covered(&m, true, false, 0.1), "its start");
        assert!(!starts_covered(&m, false, false, 0.1), "a Skate 1 / 2 map's mode has its own start");
        m.phase = Phase::Running { started: 1., ends: None };
        assert!(starts_covered(&m, true, false, 1.05), "until the mode's own is up");
        assert!(!starts_covered(&m, true, true, 1.05), "its LoadingHold took over");
        assert!(!starts_covered(&m, true, false, 1. + TAKEOVER), "a mode that puts none up");
        m.intro = true;
        assert!(!starts_covered(&m, true, false, 1.05), "an intro shows through it");
        (m.owned, m.intro, m.phase) = (false, false, Phase::Intro { since: 0. });
        assert!(!starts_covered(&m, true, false, 0.1), "the core's own intro and countdown");
    }

    #[test]
    fn modes_names_filters_and_results() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
        }
        assert_eq!(Mode::from_filter("ID_MAP_FILTER_CHALLENGETYPES_HOMBATTLE"), Some(Mode::HallOfMeat));
        assert_eq!(Mode::from_filter("ID_MAP_FILTER_CHALLENGETYPES_1UP"), Some(Mode::OneUp));
        assert_eq!(Mode::from_filter("ID_MAP_FILTER_CHALLENGETYPES_SPOTRACE"), Some(Mode::SpotRace));
        assert!(Mode::SpotRace.lower_wins());
        assert_eq!(Mode::SpotRace.show(61_000), "1:01.00");
        assert_eq!(Mode::from_filter("ID_MAP_FILTER_CHALLENGETYPES_STREET"), Some(Mode::Contest), "Skate 3 street contests");
        assert_eq!(Mode::from_filter("ID_MAP_FILTER_CHALLENGETYPES_NOSUCH"), None);
        assert_eq!(Mode::from_filter("ID_CROSSBAR_ONLINE_CHALLENGE_MODE_TITLE"), None);
        assert_eq!(Mode::DeathRace.show(83_456), "1:23.45");
        assert_eq!(Mode::Skate.show(-2), "S.K.");
        assert_eq!(Mode::SpotBattle.show(12_000), "12000");
        assert!(Mode::DeathRace.lower_wins() && !Mode::HallOfMeat.lower_wins());
    }

    #[test]
    fn the_host_starts_a_match_on_a_playable_row_only() {
        use skate_net::roster::ChallengeLobby;
        let race = "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE";
        let lobby = |tab: usize, row: &str| ChallengeLobby { title: race.into(), team: tab == TEAM_TAB, row: Some(Target::new(tab, row, "", "")) };
        let playable = Playable(["race_nsv_01".to_string()].into());
        let mut state = SessionState { challenge: Some(lobby(3, "race_nsv_01")), ..default() };
        assert_eq!(startable(&state, &playable), Some(Mode::DeathRace));
        assert_eq!(startable(&SessionState { challenge: Some(lobby(3, "race_dwtn_01_ol")), ..default() }, &playable), None,
                   "no mode plays that row");
        assert_eq!(startable(&SessionState::default(), &playable), None, "freeskate");
        // C4 / T1: a Career or Team lobby is offered only on a row a mode registered for that tab (SimulPlay; a ported
        // team path).
        assert_eq!(startable(&SessionState { challenge: Some(lobby(TEAM_TAB, "race_nsv_01")), ..default() }, &playable), None);
        assert_eq!(startable(&SessionState { challenge: Some(lobby(CAREER_TAB, "race_nsv_01")), ..default() }, &playable), None);
        let teams = Playable(["race_nsv_01".to_string(), team("race_nsv_01")].into());
        let mut team_state = SessionState { challenge: Some(lobby(TEAM_TAB, "race_nsv_01")), team_a: vec![10], team_b: vec![20], ..default() };
        assert_eq!(startable(&team_state, &teams), Some(Mode::DeathRace));
        let both = BTreeMap::from([(10, Ready { row: "race_nsv_01".into(), ok: true }), (20, Ready { row: "race_nsv_01".into(), ok: true })]);
        assert_eq!(start_game(&mut team_state, &[20, 10], &both, 6, &teams, None), Ok(()));
        assert_eq!(team_state.game.as_ref().map(|g| g.teams.as_slice()), Some(&[2, 1][..]), "each slot's team");
        assert!(state.game.is_none());
        let ready = |pairs: &[(u64, &str, bool)]| -> BTreeMap<u64, Ready> {
            pairs.iter().map(|&(a, row, ok)| (a, Ready { row: row.into(), ok })).collect()
        };
        // C6: the host waits for an answer for this row (an older one is not one).
        let stale = ready(&[(10, "race_nsv_01", true), (20, "other_row", true)]);
        assert_eq!(start_game(&mut state, &[10, 20], &stale, 6, &playable, None), Err(Some((WAITING_FOR_SKATERS, 0))));
        // I3: two skaters at least; one that cannot play the row does not count (and never joins: it watches).
        let one_can = ready(&[(10, "race_nsv_01", true), (20, "race_nsv_01", false)]);
        assert_eq!(start_game(&mut state, &[10, 20], &one_can, 6, &playable, None), Err(Some((NEED_MORE_PLAYERS, 0))));
        // I4: no more than the row holds.
        let three = ready(&[(10, "race_nsv_01", true), (20, "race_nsv_01", true), (30, "race_nsv_01", true)]);
        assert_eq!(start_game(&mut state, &[10, 20, 30], &three, 2, &playable, None), Err(Some((TOO_MANY_PLAYERS, 2))));
        // U5: retail's 6 is on the session's players, watchers too (0x82721368).
        let seven: Vec<u64> = (1..=7).collect();
        let mut seven_ready = ready(&seven.iter().map(|&a| (a, "race_nsv_01", true)).collect::<Vec<_>>());
        seven_ready.get_mut(&7).unwrap().ok = false;
        assert_eq!(start_game(&mut state, &seven, &seven_ready, CHALLENGE_MAX, &playable, None), Err(Some((TOO_MANY_PLAYERS, CHALLENGE_MAX))));
        assert_eq!(state.game, None);
        let mut all = three.clone();
        all.insert(40, Ready { row: "race_nsv_01".into(), ok: false });
        assert_eq!(start_game(&mut state, &[10, 20, 30, 40], &all, 6, &playable, None), Ok(()));
        let game = state.game.clone().unwrap();
        assert_eq!((game.mode.as_str(), game.slots.as_slice(), game.results), ("death_race", &[10, 20, 30][..], false), "40 cannot play it");
        assert_eq!(start_game(&mut state, &[10, 20], &three, 6, &playable, None), Err(None), "one match at a time");
        // The Online Challenge Map refuses those rows before any lobby exists.
        assert_eq!(unavailable(3, "race_nsv_01", &playable), None);
        assert_eq!(unavailable(TEAM_TAB, "race_nsv_01", &playable), Some(UNAVAILABLE), "no team path registered");
        assert_eq!(unavailable(TEAM_TAB, "race_nsv_01", &teams), None, "T1: no Online Team needed");
        assert_eq!(unavailable(CAREER_TAB, "race_nsv_01", &playable), Some(UNAVAILABLE));
        assert_eq!(unavailable(3, "ots_film_01", &playable), Some(UNAVAILABLE), "an unported mode's row");
        // A Career row is playable only as a SimulPlay mode registered it (skate3_ots), not by its id on another tab.
        let career_ots = Playable(["race_nsv_01".to_string(), career("ots_dwtn_01")].into());
        assert_eq!(unavailable(CAREER_TAB, "ots_dwtn_01", &career_ots), None);
        assert_eq!(unavailable(3, "ots_dwtn_01", &career_ots), Some(UNAVAILABLE));
        assert_eq!(unavailable(CAREER_TAB, "race_nsv_01", &career_ots), Some(UNAVAILABLE));
        assert_eq!(row_cap(3, "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE"), CHALLENGE_MAX);
    }

    /// Audit 10-16 (not retail: retail's sides came from EA Online Teams): a team match needs a skater on each side;
    /// uneven sides start.
    #[test]
    fn a_team_match_needs_both_sides() {
        let lobby = |tab: usize, row: &str| skate_net::roster::ChallengeLobby {
            title: "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE".into(),
            team: tab == TEAM_TAB,
            row: Some(Target::new(tab, row, "", "")),
        };
        let teams = Playable(["race_nsv_01".to_string(), team("race_nsv_01")].into());
        let all = |a: &[u64]| -> BTreeMap<u64, Ready> { a.iter().map(|&a| (a, Ready { row: "race_nsv_01".into(), ok: true })).collect() };
        let mut one_side = SessionState { challenge: Some(lobby(TEAM_TAB, "race_nsv_01")), team_a: vec![10, 20], ..default() };
        assert_eq!(start_game(&mut one_side, &[10, 20], &all(&[10, 20]), 6, &teams, None), Err(Some((NEED_MORE_PLAYERS, 0))));
        // Side B's only skater can't play the row: it watches, so side B is empty.
        let mut watcher = SessionState { challenge: Some(lobby(TEAM_TAB, "race_nsv_01")), team_a: vec![10, 20], team_b: vec![30], ..default() };
        let mut ready = all(&[10, 20, 30]);
        ready.get_mut(&30).unwrap().ok = false;
        assert_eq!(start_game(&mut watcher, &[10, 20, 30], &ready, 6, &teams, None), Err(Some((NEED_MORE_PLAYERS, 0))));
        let mut uneven = SessionState { challenge: Some(lobby(TEAM_TAB, "race_nsv_01")), team_a: vec![10, 20], team_b: vec![30], ..default() };
        assert_eq!(start_game(&mut uneven, &[10, 20, 30], &all(&[10, 20, 30]), 6, &teams, None), Ok(()));
    }

    /// Retail has no per-row lobby size: every challenge session holds CHALLENGE_MAX (Blaze max_players, audit 10-16),
    /// whatever its start's slot count. user 10-12: Skate 2 S.K.A.T.E. holds 4 (retail throwdownresults draws 4 rows),
    /// as does Skate 1's (skatehud's 4 players); Skate 3's doesn't.
    #[test]
    fn rows_hold_challenge_max_and_skate2_skate_four() {
        let (spot, skate) = ("ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE", "ID_MAP_FILTER_CHALLENGETYPES_SKATE");
        assert_eq!([3, SKATE2_TAB, SKATE1_TAB].map(|t| row_cap(t, spot)), [CHALLENGE_MAX; 3]);
        assert_eq!(row_cap(SKATE1_TAB, "ID_MAP_FILTER_CHALLENGETYPES_BESTTRICK"), CHALLENGE_MAX);
        assert_eq!([3, SKATE2_TAB, SKATE1_TAB].map(|t| row_cap(t, skate)), [CHALLENGE_MAX, 4, 4]);
        // The Steam lobby is the same size, so strangers can't fill it past what can start.
        let tags = |tab: &str, filter: &str| skate_net::directory::Tags { tab: tab.into(), filter: filter.into(), ..default() };
        assert_eq!([tags("versus", spot), tags("skate1", skate), tags("skate2", skate), tags("versus", skate)].map(|t| t.capacity()), [6, 4, 4, 6]);
    }

    /// A mode's own order (Skate 2 S.K.A.T.E., contests, Hall of Meat): its order and scores as given, so a tie stays
    /// as the mode broke it, and a player who left stays listed under its last name (retail Skate 2: last, -1); a slot
    /// never named is skipped. The core's own `standings` would put the tie in slot order and drop the leaver.
    #[test]
    fn a_modes_own_order_keeps_its_ties_and_leavers() {
        let game = Game { token: 5, mode: String::new(), slots: vec![10, 11, 12, 13], results: false, teams: vec![] };
        let scores = BTreeMap::from([(10, Score { token: 5, value: 500, finished: true, ..default() })]);
        let known = ["Ann", "Bo", "Cy", ""].map(String::from);
        let got = ordered(&[(1, 500), (0, 500), (2, -1), (3, -1)], &game, &scores, &known, 10);
        let rows: Vec<_> = got.iter().map(|s| (s.name.as_str(), s.score, s.finished, s.local)).collect();
        assert_eq!(rows, [("Bo", 500, false, false), ("Ann", 500, true, true), ("Cy", -1, false, false)]);
    }

    #[test]
    fn standings_rank_race_times_and_points() {
        let game = Game { token: 5, mode: "death_race".into(), slots: vec![1, 2, 3, 4], results: false, teams: vec![] };
        let names = BTreeMap::from([(1, "Host".to_string()), (2, "Ann".into()), (3, "Bo".into())]);
        let score = |value, finished| Score { token: 5, value, finished, ..default() };
        let scores = BTreeMap::from([(1, score(90_000, true)), (2, score(80_000, true)), (3, score(10_000, false))]);
        let order = |lower| standings(&game, &scores, &names, 2, lower).into_iter().map(|s| (s.name, s.local)).collect::<Vec<_>>();
        assert_eq!(order(true), [("Ann".into(), true), ("Host".into(), false), ("Bo".into(), false)], "unfinished last; 4 left");
        assert_eq!(order(false)[0].0, "Host", "points: highest first");
        let stale = BTreeMap::from([(1, Score { token: 4, value: 1, finished: true, ..default() })]);
        assert!(!standings(&game, &stale, &names, 1, true)[0].finished, "an older match's score does not count");
        assert_eq!(Mode::DeathRace.show(DNF), "DNF");
    }

    #[test]
    fn events_reach_every_other_player_once_in_order() {
        let game = Game { token: 5, mode: "skate".into(), slots: vec![1, 2, 3], results: false, teams: vec![] };
        let (mut m, mut sent, mut leaves) = (Match::default(), Events::default(), Leaves::default());
        let all = |_: u64| true;
        let none = BTreeMap::new();
        m.outbox = vec!["a".into(), "b".into()];
        relay(&mut m, &game, 2, &all, &BTreeMap::new(), &none, &mut sent, &mut leaves, 0.);
        assert_eq!((sent.token, sent.first, bodies(&sent), m.outbox.len()), (5, 0, vec!["a".to_string(), "b".into()], 0));
        assert!(sent.items[0].starts_with(KEY) && sent.items.len() == 3, "behind our key, each signed");
        let ev = |token, first, items: &[&str]| Events { token, first, items: items.iter().map(|s| s.to_string()).collect() };
        let records = BTreeMap::from([(1, ev(5, 0, &["x", "y"])), (2, sent.clone()), (3, ev(4, 0, &["old match"]))]);
        relay(&mut m, &game, 2, &all, &records, &none, &mut sent, &mut leaves, 0.);
        assert_eq!(m.inbox, [(0, "x".to_string()), (0, "y".into())], "not our own, not another match's");
        m.inbox.clear();
        // Slot 0 has dropped "x" (we took it) and sends one more.
        let records = BTreeMap::from([(1, ev(5, 1, &["y", "z"]))]);
        relay(&mut m, &game, 2, &all, &records, &none, &mut sent, &mut leaves, 0.);
        assert_eq!(m.inbox, [(0, "z".to_string())]);
        assert_eq!(leaves.taken(&game.slots), [(1, 3)], "what our @score tells slot 0 we have taken");
        // C5: ours (our key, "a", "b") stay until both others have taken them.
        let taken = |a, n| Score { token: 5, taken: vec![(a, n)], ..default() };
        let scores = BTreeMap::from([(1, taken(2, 3)), (3, taken(2, 2))]);
        relay(&mut m, &game, 2, &all, &BTreeMap::new(), &scores, &mut sent, &mut leaves, 0.);
        assert_eq!((sent.first, bodies(&sent)), (2, vec!["b".to_string()]), "slot 2 has not taken \"b\"");
        let gone = |a: u64| a != 3;
        relay(&mut m, &game, 2, &gone, &BTreeMap::new(), &scores, &mut sent, &mut leaves, 0.);
        assert_eq!((sent.first, bodies(&sent)), (3, vec!["@gone 2".to_string()]), "a player who left holds nothing back; our mark that it went (Leaves)");
        // An older match's acknowledgement is not one.
        m.outbox = vec!["c".into()];
        let old = BTreeMap::from([(1, Score { token: 4, taken: vec![(2, 9)], ..default() })]);
        relay(&mut m, &game, 2, &all, &BTreeMap::new(), &old, &mut sent, &mut leaves, 0.);
        assert_eq!((sent.first, sent.items.len()), (3, 2));
    }

    /// End review I2 (10-12): slot 0's last event ("voted") reached console 2 but not console 1, then slot 0 left.
    /// Console 2 sends it on; console 1's mode gets it, then the leave, once console 2's mark is in; a console whose
    /// other never marks gives the leave after LEAVE_HOLD.
    #[test]
    fn a_leavers_last_event_reaches_every_console_before_its_leave() {
        let game = Game { token: 5, mode: "spot_battle".into(), slots: vec![10, 20, 30], results: false, teams: vec![] };
        // Slot 0's stream: its key, then its events signed.
        let mut zero = Leaves::new(5);
        let (key, run, voted) = (zero.key_item().unwrap(), zero.sign(0, 1, "run"), zero.sign(0, 2, "voted"));
        let ev = |items: &[&String]| Events { token: 5, first: 0, items: items.iter().map(|s| s.to_string()).collect() };
        #[derive(Default)]
        struct Console {
            m: Match,
            sent: Events,
            leaves: Leaves,
        }
        let mut c: [Console; 3] = Default::default();
        let step = |c: &mut Console, local: u64, now: f64, here: &dyn Fn(u64) -> bool, records: BTreeMap<u64, Events>| {
            c.m.players = ["A", "B", "C"].map(String::from).to_vec();
            relay(&mut c.m, &game, local, here, &records, &BTreeMap::new(), &mut c.sent, &mut c.leaves, now);
            (c.m.players[0].clone(), std::mem::take(&mut c.m.inbox))
        };
        let all = |_: u64| true;
        let gone = |a: u64| a != 10;
        assert_eq!(step(&mut c[2], 30, 0., &all, BTreeMap::from([(10, ev(&[&key, &run, &voted]))])).1, [(0, "run".to_string()), (0, "voted".into())]);
        assert_eq!(step(&mut c[1], 20, 0., &all, BTreeMap::from([(10, ev(&[&key, &run]))])).1, [(0, "run".to_string())]);
        // Console 1 sees slot 0 go first: it sends on "run" and its mark, and holds the leave.
        assert_eq!(step(&mut c[1], 20, 0.1, &gone, BTreeMap::new()), ("A".into(), vec![]));
        let marked = |s: &Events| bodies(s).into_iter().filter(|e| e.starts_with('@')).collect::<Vec<_>>();
        assert_eq!(marked(&c[1].sent), ["@on 0 1 run", "@gone 0"]);
        let from1 = c[1].sent.clone();
        // Console 2 hears console 1's mark, sees slot 0 go itself: nothing new to take, and the leave at once.
        assert_eq!(step(&mut c[2], 30, 0.1, &all, BTreeMap::from([(20, from1.clone())])), ("A".into(), vec![]));
        assert_eq!(step(&mut c[2], 30, 0.2, &gone, BTreeMap::from([(20, from1)])), (String::new(), vec![]));
        assert_eq!(marked(&c[2].sent), ["@on 0 1 run", "@on 0 2 voted", "@gone 0"]);
        // Console 1 takes "voted" as slot 0's, before the leave, which comes the frame after.
        let from2 = BTreeMap::from([(30, c[2].sent.clone())]);
        assert_eq!(step(&mut c[1], 20, 0.2, &gone, from2.clone()), ("A".into(), vec![(0, "voted".into())]));
        assert_eq!(step(&mut c[1], 20, 0.3, &gone, from2), (String::new(), vec![]), "once, then the leave");
        // A third console that never hears console 2's mark gives the leave after LEAVE_HOLD.
        let mut late = Leaves::default();
        let _ = late.left(2, 0, 1.);
        assert!(!late.released(0, [1], 1. + LEAVE_HOLD - 0.1) && late.released(0, [1], 1. + LEAVE_HOLD));
    }

    /// End review M2 (10-12): slots 2 and 3 go close together, and the two consoles still here see them go in opposite
    /// orders (slot 0: 2 then 3; slot 1: 3 then 2). Both give their modes one order, slot 0's (`Match::left`).
    #[test]
    fn two_leaves_seen_in_opposite_orders_reach_every_mode_in_one_order() {
        let game = Game { token: 5, mode: "hall_of_meat".into(), slots: vec![10, 20, 30, 40], results: false, teams: vec![] };
        type Console = (Match, Events, Leaves);
        let step = |c: &mut Console, local: u64, now: f64, here: &[u64], records: BTreeMap<u64, Events>| {
            c.0.players = ["A", "B", "X", "Y"].map(String::from).to_vec();
            relay(&mut c.0, &game, local, &|a: u64| here.contains(&a), &records, &BTreeMap::new(), &mut c.1, &mut c.2, now);
            c.0.left.clone()
        };
        let (mut a, mut b) = <(Console, Console)>::default();
        let none = Vec::<usize>::new();
        assert_eq!(step(&mut a, 10, 0.1, &[10, 20, 40], BTreeMap::new()), none);
        assert_eq!(step(&mut b, 20, 0.1, &[10, 20, 30], BTreeMap::new()), none);
        assert_eq!(step(&mut a, 10, 0.2, &[10, 20], BTreeMap::new()), none);
        assert_eq!(step(&mut b, 20, 0.2, &[10, 20], BTreeMap::new()), none);
        assert_eq!((bodies(&a.1), bodies(&b.1)), (vec!["@gone 2".to_string(), "@gone 3".into()], vec!["@gone 3".to_string(), "@gone 2".into()]));
        let (from_a, from_b) = (BTreeMap::from([(10, a.1.clone())]), BTreeMap::from([(20, b.1.clone())]));
        // Each takes the other's marks this frame and gives both leaves the next.
        assert_eq!(step(&mut a, 10, 0.3, &[10, 20], from_b.clone()), none);
        assert_eq!(step(&mut b, 20, 0.3, &[10, 20], from_a.clone()), none);
        assert_eq!(step(&mut a, 10, 0.4, &[10, 20], from_b), [2, 3]);
        assert_eq!(step(&mut b, 20, 0.4, &[10, 20], from_a), [2, 3], "slot 0's order, not its own");
    }

    /// C5 / I13, two instances over the real lobby protocol (skate-net Sessions, host and guest): every Skate 1 event of
    /// a burst too big for one record (each carries the player's "S" trick state) reaches the other console, once and in
    /// order, both ways.
    #[test]
    fn a_skate1_burst_reaches_the_other_console_whole() {
        use skate_net::lobby::{Info, Session};
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut sessions = [Session::new(7, info(10), None), Session::new(7, info(20), Some(1))];
        let endpoints = [1u64, 2];
        let game = Game { token: 9, mode: "spot_battle".into(), slots: vec![10, 20], results: false, teams: vec![] };
        let state = json!({"air": false, "trick": true, "types": {"1": true, "4": true}, "grab": true, "spins": 2,
                           "launch": false, "first": 113, "first_type": 1, "label": "ID_TRICK_FLIP_KICKFLIP_TO_INDY"});
        let burst = |who: usize, frame: usize| -> Vec<String> {
            crate::skate1_online::RELAYED
                .iter()
                .map(|e| {
                    let message = json!({"PlayerID": who, "VolumeID": format!("OTS_Spot_{frame}"), "SequenceScore": 12_345 + frame,
                                         "Wipedout": false, "StartTick": 400 + frame, "StartSimGameTick": 400 + frame, "S": state});
                    crate::skate1_online::encode(e, &message, 1_000 + frame as i64)
                })
                .collect()
        };
        assert!(burst(0, 0).iter().map(String::len).sum::<usize>() > skate_net::lobby::MAX_APP_VALUE, "a burst needs several records");
        let mut m: [Match; 2] = Default::default();
        let mut sent: [Events; 2] = Default::default();
        let mut leaves: [Leaves; 2] = Default::default();
        let mut expected: [Vec<String>; 2] = Default::default();
        let mut got: [Vec<String>; 2] = Default::default();
        let mut now = 0;
        for frame in 0..400 {
            for i in 0..2 {
                // Three bursts each, a frame apart, from both consoles at once.
                if frame < 3 {
                    m[i].outbox = burst(i, frame);
                    expected[1 - i].extend(m[i].outbox.clone());
                }
                let session = &mut sessions[i];
                let read = |key: &str| -> Vec<(u64, Vec<u8>)> {
                    session.actors.iter().filter_map(|(&a, actor)| Some((a, actor.application.get(key)?.value.clone()))).collect()
                };
                let records: BTreeMap<u64, Events> = read(EVENTS_KEY).into_iter().filter_map(|(a, v)| Some((a, Events::decode(&v)?))).collect();
                let scores: BTreeMap<u64, Score> = read(SCORE_KEY).into_iter().filter_map(|(a, v)| Some((a, Score::decode(&v)?))).collect();
                let local = [10, 20][i];
                relay(&mut m[i], &game, local, &|_| true, &records, &scores, &mut sent[i], &mut leaves[i], 0.);
                got[i].extend(m[i].inbox.drain(..).map(|(_, text)| text));
                let taken = leaves[i].taken(&game.slots);
                session.publish_application(EVENTS_KEY, sent[i].encode(), now);
                session.publish_application(SCORE_KEY, Score { token: 9, taken, ..default() }.encode(), now);
            }
            now += 33;
            for p in sessions[0].service(now) {
                if p.peer == endpoints[1] {
                    sessions[1].receive(endpoints[0], &p.data, now);
                }
            }
            for p in sessions[1].service(now) {
                if p.peer == endpoints[0] {
                    sessions[0].receive(endpoints[1], &p.data, now);
                }
            }
            if frame > 3 && got[0].len() == expected[0].len() && got[1].len() == expected[1].len() && sent.iter().all(|s| s.items.is_empty()) {
                break;
            }
        }
        assert_eq!(got[0], expected[0], "the host took every one of the guest's events, in order");
        assert_eq!(got[1], expected[1], "the guest took every one of the host's");
        assert!(sent.iter().all(|s| s.items.is_empty() && s.first as usize == 1 + 3 * crate::skate1_online::RELAYED.len()),
                "each sender let them (and its key) go once taken");
    }

    /// User 10-14 (Party Play's timer ran on while paused): an offline pause stops the match clock; online, and
    /// after the match, it is the real clock.
    #[test]
    fn an_offline_pause_stops_the_match_clock() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<MatchClock>();
        world.insert_resource(Time::<Real>::default());
        world.insert_resource(Time::<Virtual>::default());
        let mut m = Match::default();
        m.start(Mode::SpotBattle, "spot", 0.);
        world.insert_resource(m);
        let mut tick = |world: &mut World, secs: f32| {
            world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(secs));
            world.run_system_once(tick_clock).unwrap();
            match_now(world)
        };
        assert_eq!(tick(&mut world, 1.), 1.);
        world.resource_mut::<Time<Virtual>>().pause();
        assert_eq!(tick(&mut world, 2.), 1., "paused: the clock holds");
        world.resource_mut::<Time<Virtual>>().unpause();
        assert_eq!(tick(&mut world, 1.), 2.);
        world.resource_mut::<Match>().token = 5;
        world.resource_mut::<Time<Virtual>>().pause();
        assert_eq!(tick(&mut world, 1.), 3., "online nothing pauses");
        world.resource_mut::<Match>().mode = None;
        assert_eq!(tick(&mut world, 1.), 6., "no match: the real clock");
    }

    /// Online audit 11-01 (Party Play's Skate 1 and 2 turns ran on under the pause menu): the step their timers count
    /// down by is 0 while an offline match is paused, the real one online.
    #[test]
    fn a_paused_offline_match_steps_its_timers_by_0() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<MatchClock>();
        world.insert_resource(Time::<Real>::default());
        world.insert_resource(Time::<Virtual>::default());
        let mut m = Match::default();
        m.start(Mode::Skate, "skate", 0.);
        world.insert_resource(m);
        let tick = |world: &mut World, secs: f32| {
            world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(secs));
            world.run_system_once(tick_clock).unwrap();
            match_dt(world)
        };
        assert_eq!(tick(&mut world, 0.5), 0.5);
        world.resource_mut::<Time<Virtual>>().pause();
        assert_eq!(tick(&mut world, 30.), 0., "paused: the turn's time stays");
        world.resource_mut::<Time<Virtual>>().unpause();
        assert_eq!(tick(&mut world, 0.5), 0.5);
        world.resource_mut::<Match>().token = 5;
        world.resource_mut::<Time<Virtual>>().pause();
        assert_eq!(tick(&mut world, 0.5), 0.5, "online nothing pauses");
    }

    /// The Skate 1 and 2 modes Party Play plays take their seconds from the match clock (match_now, match_dt), never
    /// from `Time<Real>`, which a pause doesn't stop.
    #[test]
    fn party_plays_skate_1_and_2_modes_read_only_the_match_clock() {
        for (file, source) in [
            ("skate2_contests.rs", include_str!("skate2_contests.rs")),
            ("skate2_skate_game.rs", include_str!("skate2_skate_game.rs")),
            ("skate2_hall_of_meat.rs", include_str!("skate2_hall_of_meat.rs")),
            ("skate1_online.rs", include_str!("skate1_online.rs")),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            assert!(!code.contains("Time<Real>"), "{file} reads the real clock");
        }
    }

    #[test]
    fn a_disabled_pause_menu_holds_only_during_a_match() {
        let mut m = Match { pause_disabled: true, ..default() };
        assert!(!m.pause_blocked(), "a stale flag after the match");
        m.start(Mode::Contest, "street_dwtn_01", 0.);
        m.pause_disabled = true;
        assert!(m.pause_blocked());
        m.pause_disabled = false;
        assert!(!m.pause_blocked(), "the Lua enabled it again (a run)");
        assert!(!m.markers_blocked());
        m.start(Mode::SpotBattle, "spot", 0.);
        assert!(m.markers_blocked() && !m.pause_blocked(), "user 10-14: no marker in a Spot Battle, pause still works");
        m.phase = Phase::Idle;
        assert!(!m.markers_blocked());
        m.start(Mode::Skate, "skate", 0.);
        assert!(m.markers_blocked(), "nor in a game of S.K.A.T.E.");
    }

    #[test]
    fn a_straggler_is_counted_finished_after_the_others() {
        let mut since = None;
        assert!(!run_over(&[false, false], &mut since, 0.), "nobody done");
        assert!(run_over(&[true, true], &mut since, 0.));
        assert!(!run_over(&[true, false], &mut since, 10.), "the others are done: the clock starts");
        assert!(!run_over(&[true, false], &mut since, 10. + STRAGGLER - 0.1));
        assert!(run_over(&[true, false], &mut since, 10. + STRAGGLER), "a stuck console never holds the session");
    }

    /// Audit 10-28 #1: Skate 2 S.K.A.T.E. counts a knocked-out player finished, so with 3 or 4 the clock ran from the
    /// first knockout and the game ended a minute later with two still playing. Only a last holdout gets it.
    #[test]
    fn the_straggler_clock_waits_for_one_left() {
        let mut since = None;
        assert!(!run_over(&[true, false, false], &mut since, 0.));
        assert!(!run_over(&[true, false, false], &mut since, 10. * STRAGGLER), "two still playing: no clock");
        assert!(!run_over(&[true, true, false], &mut since, 20. * STRAGGLER), "one left: the clock starts");
        assert!(run_over(&[true, true, false], &mut since, 21. * STRAGGLER), "a stuck last one still never holds the session");
    }

    /// Audit 10-28 #2: a Skate 1 row ends in `step` the frame its script finishes, so `sync` found no mode and never
    /// sent that player's finished result: the host waited for good (a 2-player lobby stayed in the match).
    #[test]
    fn a_mode_ended_by_its_script_still_sends_its_finished_result() {
        let info = skate_net::lobby::Info { id: 10, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<Owned>()
            .init_resource::<Playable>()
            .insert_resource(crate::graphics_menu::test_menu())
            .insert_resource(Multiplayer::for_test(skate_net::lobby::Session::new(7, info, None)))
            .add_systems(Update, sync);
        let mut roster = OnlineRoster::default();
        // A mode name `sync` cannot start, so the Match below stays as `step` left it.
        roster.state.game = Some(Game { token: 5, mode: "-".into(), slots: vec![10, 11], results: false, teams: vec![] });
        roster.members = vec![skate_net::roster::Member { actor: 10, name: "a".into(), steam: 0, host: true }];
        app.insert_resource(roster);
        let mut m = Match::default();
        (m.token, m.owned, m.finished, m.score) = (5, true, true, 3);
        app.insert_resource(m);
        app.update();
        let score = scores(app.world().resource::<Multiplayer>()).remove(&10);
        assert!(score.is_some_and(|s| s.token == 5 && s.finished && s.value == 3), "its finish reaches the host");
    }

    /// Review 10-29 (I-A): a player whose Skate 1 script ended its mode (finished) keeps taking the others' events;
    /// frozen, its `taken` held every sender's record at the oldest untaken event (~1 kB), and with 3+ players the
    /// others' events to each other stopped.
    #[test]
    fn a_finished_player_keeps_taking_the_others_events() {
        use skate_net::lobby::{Info, Session};
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut sessions = [Session::new(7, info(10), None), Session::new(7, info(20), Some(1))];
        let events = Events { token: 5, first: 0, items: vec!["x".into(), "y".into()] };
        let mut now = 0;
        for _ in 0..60 {
            sessions[1].publish_application(EVENTS_KEY, events.encode(), now);
            now += 33;
            for p in sessions[0].service(now) {
                if p.peer == 2 {
                    sessions[1].receive(1, &p.data, now);
                }
            }
            for p in sessions[1].service(now) {
                if p.peer == 1 {
                    sessions[0].receive(2, &p.data, now);
                }
            }
        }
        let [host, _] = sessions;
        assert!(host.actors.get(&20).is_some_and(|a| a.application.contains_key(EVENTS_KEY)), "the guest's events reached the host");
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<Owned>()
            .init_resource::<Playable>()
            .insert_resource(crate::graphics_menu::test_menu())
            .insert_resource(Multiplayer::for_test(host))
            .add_systems(Update, sync);
        let mut roster = OnlineRoster::default();
        // A mode name `sync` cannot start, so the Match below stays as `step` left it.
        roster.state.game = Some(Game { token: 5, mode: "-".into(), slots: vec![10, 20], results: false, teams: vec![] });
        roster.members = vec![
            skate_net::roster::Member { actor: 10, name: "a".into(), steam: 0, host: true },
            skate_net::roster::Member { actor: 20, name: "b".into(), steam: 0, host: false },
        ];
        app.insert_resource(roster);
        let mut m = Match::default();
        (m.token, m.owned, m.finished) = (5, true, true);
        app.insert_resource(m);
        app.update();
        let score = scores(app.world().resource::<Multiplayer>()).remove(&10).expect("our @score");
        assert_eq!(score.taken, [(20, 2)], "its @score acknowledges the guest's events");
    }

    /// Review 10-29 (I-B): a Skate 1 row's script ends the host's mode as it finishes (GAP-1); the host still counts as
    /// a finished player, so a guest who never finishes gets the straggler clock, not the 30 min cap.
    #[test]
    fn a_host_whose_script_ended_still_counts_as_finished() {
        let info = skate_net::lobby::Info { id: 10, map: 1, rig: 2, physics: 3, appearance: 4 };
        // Time<Real>'s first update only starts it: later durations then count.
        let mut real = Time::<Real>::default();
        real.update_with_duration(std::time::Duration::ZERO);
        let mut app = App::new();
        app.insert_resource(real)
            .insert_resource(Multiplayer::for_test(skate_net::lobby::Session::new(7, info, None)))
            .add_systems(Update, referee);
        let mut roster = OnlineRoster::default();
        roster.state.game = Some(Game { token: 5, mode: "-".into(), slots: vec![10, 11], results: false, teams: vec![] });
        roster.members = vec![
            skate_net::roster::Member { actor: 10, name: "h".into(), steam: 0, host: true },
            skate_net::roster::Member { actor: 11, name: "g".into(), steam: 0, host: false },
        ];
        app.insert_resource(roster);
        app.world_mut().resource_mut::<Multiplayer>().publish_score(&Score { token: 5, finished: true, ..default() });
        let mut m = Match::default();
        (m.token, m.finished) = (5, true);
        app.insert_resource(m);
        app.update();
        app.world_mut().resource_mut::<Time<Real>>().update_with_duration(std::time::Duration::from_secs_f32(STRAGGLER + 1.));
        app.update();
        let game = app.world().resource::<OnlineRoster>().state.game.clone().expect("the game");
        assert!(game.results, "the guest who never finishes gets the straggler clock");
    }

    #[test]
    fn observers_are_held_and_online_countdowns_start_together() {
        let m = Match { token: 3, observer: true, watch: Some(0), ..default() };
        assert!(held(&m), "I1: a player who joined mid-match only watches");
        let mut m = Match::default();
        m.start(Mode::DeathRace, "dr", 0.);
        (m.token, m.rounds) = (5, vec![0, 0]);
        advance(&mut m, 1.);
        assert!(matches!(m.phase, Phase::Intro { .. }) && m.round == 1, "our intro is over: we wait for the other console");
        m.rounds = vec![1, 1];
        advance(&mut m, 1.5);
        assert_eq!(m.phase, Phase::Countdown { ends: 1.5 + COUNTDOWN });
        let mut late = Match::default();
        late.start(Mode::DeathRace, "dr", 0.);
        (late.token, late.rounds) = (5, vec![1, 0]);
        advance(&mut late, INTRO_MAX);
        assert!(matches!(late.phase, Phase::Countdown { .. }), "a console that never gets there holds no one past INTRO_MAX");
        // I10: a mode with no core countdown (Skate 2 S.K.A.T.E.) goes from the intro straight to its run.
        let mut skate = Match::default();
        skate.start(Mode::Skate, "s", 0.);
        skate.no_countdown = true;
        advance(&mut skate, INTRO_MIN);
        assert!(matches!(skate.phase, Phase::Running { .. }));
        let r = Match { phase: Phase::Results { until: 10. }, ..default() };
        assert_eq!((r.results_left(4.), Match::default().results_left(4.)), (Some(6.), None));
    }
}
