//! Skate 3's turn-based battles on the match core: Spot Battle (challenge graph `spot_battle` < turn_based_uniform <
//! turn_based < default) for the online rows of DownTown, Industrial and University. Data: tools/skate3_online.py,
//! installed at `private/skate3-online/spot_battle/`: rules.json (per row its CLD fields: TimeLimit, NumRounds,
//! NumSequencesToEndTurn, CumulativeScoring, CountdownDelay, the attempt spawn and waiting locators with their slots,
//! SpotVolumes, the message ids, the NIS flythrough), strings.json, scripts/spot_battle.lua (the decompiled graph: the
//! rules' source and the tests' oracle), the intro cameras (tutorial.json + scene/camera) and the shared HUD movies
//! (skate3_modes).
//!
//! [`Uniform`] is turn_based_uniform's online (non-team) path with spot_battle's challenge_running, handler by handler
//! (line numbers are spot_battle.lua's): turns go round from a random player (reset 142) for NumRounds rounds (countdown
//! 244-280); each turn the others wait at TurnBasedWaitingLocation (slot = player id) and the turn's player starts at
//! TurnBasedAttemptSpawnPoint with the camera on them (334-357); a sequence that touched a spot volume scores (949-1073:
//! the best one, or the turn's sum on CumulativeScoring rows), NumSequencesToEndTurn sequences, the TimeLimit or a bail
//! end the turn; the best score ranks; with 3+ players online the others vote on every run (VotingManager) and the best
//! mean over 1 is the Player's Choice (778-813).
//!
//! The turn's player's game relays its spot volumes, sequences, bail and turn end (`Match::outbox`); the others take the
//! turn's end from that relay rather than their own clock (ponytail: retail replicates these messages per console).
//! Votes are relayed by whoever casts them. Solo (`SKATE_MODE=spot_battle:<row>`) is hot-seat on the online rules with
//! two players on this one skater.
use crate::{
    challenge::cutscene::Cutscene,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate3_modes::{
        self, Abort, ChallengeFade, Component, Effect, Graph, Handled, HudDriver, Locator, Matrix, ModeData, Msg, Place,
        ResultsSetup, Rt, View, Volume, WHITE,
    },
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// HUD2.eLeaderboardHUDMenuType.IntStringInt (leaderboardhud: eLeaderBoardHUD_IntStringInt 0).
const INT_STRING_INT: i64 = 0;
/// FrontEnd.eResultsLBType (endchallenge_online): PlaceNameInt 0, PlaceNameIntChoice 12.
const PLACE_NAME_INT: i64 = 0;
const PLACE_NAME_INT_CHOICE: i64 = 12;
/// Online.GetGhostID (0x82847998 returns 6): the id wipeoutalmostover carries for the local player's own run.
const GHOST: usize = 6;
/// GameMode.GetMinPlayersForVoting (0x828393e8 returns 3).
const MIN_VOTERS: usize = 3;
/// GameMode.SpotBattleHOMRoundOverride (0x82839358 returns false): NumRounds is the row's.
/// VotingManager's votes (0x82563bd8 / 0x82563cc8): 7, reset to 1 (hudonlinevoting's SCORE_MED).
const VOTE_SLOTS: usize = 7;
const VOTE_DEFAULT: u8 = 1;
/// eChallengeTypes (challenge_global_data ChallengeType): own_the_spots_battles 0x35, hall_of_meats_battles 0x21 (every
/// row of each; the Lua's SpotBattle and HallOfMeatBattle).
pub(crate) const SPOT_BATTLE: u32 = 0x35;
pub(crate) const HOM_BATTLE: u32 = 0x21;
/// HUD2.eTeamMessageType (0x8284fb70).
const NORMAL: u8 = 0;
const WINNING: u8 = 2;

// turn_based_uniform's timers (seconds; spot_battle.lua lines).
/// wait_for_players (817), onlinesynccomplete's teleport delay (415), the teleport's settle (357), the fade in (360).
const WAIT_PRES: f64 = 0.5;
const TELEPORT_DELAY: f64 = 0.5;
const TELEPORT_SETTLE: f64 = 1.;
const FADE_IN_DELAY: f64 = 0.5;
/// The countdown (324), the run's end after time up (908) and its fade (911).
const COUNTDOWN: f64 = 3.;
const END_RUN: f64 = 2.;
const END_RUN_FADE: f64 = 0.5;
/// end_turn: 2 s, 1.5 s after a wipeout (732-734), then 0.5 s (763); challenge_finished (597).
const END_TURN: f64 = 2.;
const END_TURN_WIPED: f64 = 1.5;
const END_TURN_DELAY: f64 = 0.5;
const COMPLETED: f64 = 3.;

const WAITING: &str = "TurnBasedWaitingLocation";
const ATTEMPT: &str = "TurnBasedAttemptSpawnPoint";

// ---------------------------------------------------------------- data
/// A row's CLD fields (rules.json, by their Lua names).
#[derive(Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Row {
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "online")]
    pub online: bool,
    #[serde(rename = "intro")]
    pub intro: Option<String>,
    /// challenge_global_data Objectives' count (self.Objectives.Count).
    #[serde(rename = "objectives", default)]
    pub objectives: usize,
    #[serde(rename = "challenge_type")]
    pub challenge_type: Option<u32>,
    pub time_limit: f64,
    pub num_rounds: i64,
    #[serde(default)]
    pub num_sequences_to_end_turn: i64,
    /// nil (absent) on most rows: Lua false.
    #[serde(default)]
    pub cumulative_scoring: Option<bool>,
    pub countdown_delay: f64,
    pub turn_based_attempt_spawn_point: Option<Locator>,
    pub turn_based_waiting_location: Option<Locator>,
    #[serde(default)]
    pub spot_volumes: Vec<Option<Volume>>,
    #[serde(rename = "CentralMessageHALIDYourTurn")]
    pub your_turn: String,
    #[serde(rename = "CentralMessageHALIDChallengeComplete")]
    pub complete: String,
    #[serde(rename = "CentralMessageHALIDSpotOwned", default)]
    pub spot_owned: String,
    /// HOM Battle's time-up message.
    #[serde(rename = "CentralMessageHALIDOutOfTime", default)]
    pub out_of_time: String,
    /// Teams: the local turn's team message, its time before the countdown (370-377), the team intro NIS (830).
    #[serde(rename = "CentralMessageHALIDYourTurnTeam", default)]
    pub your_turn_team: String,
    #[serde(rename = "TeamIntroductionDelay", default)]
    pub team_intro_delay: f64,
    #[serde(rename = "IntroNIS", default)]
    pub intro_nis: Option<serde_json::Value>,
    /// G7: the row's VisualIndicators with their look (tools/skate3_online.py chevrons; none in an older export).
    #[serde(rename = "visual_indicators", default)]
    pub visual_indicators: Vec<crate::skate3_race::Chevron>,
    /// The kind-1 VisualIndicators: the spot's area traced on the ground (ots_indu_01_ol, ots_univ_05_ol), drawn
    /// with the markers by the same AddIndicator.
    #[serde(default)]
    pub outlines: Vec<crate::skate2_arrows::Outline>,
}
impl Row {
    /// Playable: online, its spawn and waiting slots placed (a Spot Battle its spot volumes too), no bonus objectives
    /// (not ported), on a known map.
    fn playable(&self, id: &str) -> bool {
        let spots = !self.spot_volumes.is_empty() && self.spot_volumes.iter().all(Option::is_some);
        self.online
            && self.objectives == 0
            && self.turn_based_attempt_spawn_point.is_some()
            && self.turn_based_waiting_location.is_some()
            && (spots || self.challenge_type == Some(HOM_BATTLE))
            && skate3_modes::map_of(id).is_some()
    }
    fn place(&self, place: Place) -> Option<Matrix> {
        let locator = if place.field == ATTEMPT { &self.turn_based_attempt_spawn_point } else { &self.turn_based_waiting_location };
        Some(locator.as_ref()?.slot(place.slot))
    }
}

// ---------------------------------------------------------------- the graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    Setup,
    Reset,
    WaitForPlayers,
    DynamicCharacterNis,
    NisFlythrough,
    Countdown,
    ChallengeRunning,
    EndTurn,
    ChallengeFinished,
    HighlightReel,
    Leaderboard,
    ChallengeCleanup,
    Complete,
}

/// vars.PlayerData[i].
#[derive(Clone, Debug, Default)]
pub(crate) struct Player {
    pub score: i64,
    pub turn_score: i64,
    pub rank: i64,
    pub disconnected: bool,
    pub run_complete: bool,
    /// Online.GetTeam (1 or 2; 1 without teams), and UpdateTeamScores' ScoreIgnored.
    pub team: usize,
    pub score_ignored: bool,
}

/// vars.TeamData[i] (24-48).
#[derive(Clone, Debug, Default)]
pub(crate) struct TeamData {
    pub score: i64,
    pub rank: i64,
    pub size: usize,
    pub name: String,
}

/// turn_based_uniform's vars on one console (player ids are the Lua's, 1-based; a player's PlayerID is id - 1).
pub(crate) struct Uniform {
    row: Row,
    intro: f64,
    first: usize,
    me: usize,
    n: usize,
    lock_out: bool,
    hud_loaded: bool,
    pub players: Vec<Player>,
    /// PlayerRankMap[i] = vec[i - 1]. PlayerOrderMap is the identity online.
    rank_map: Vec<usize>,
    pub enable_voting: bool,
    use_dpo: bool,
    cumulative: bool,
    num_rounds: i64,
    pub current_round: i64,
    round_changed: bool,
    current_leader: i64,
    random_start: i64,
    /// CurrentPlayerIndex.
    index: i64,
    /// ActivePlayerID.
    pub active: Option<usize>,
    in_boundary: bool,
    in_spot: Vec<bool>,
    force_scoring: bool,
    wiped: bool,
    sequences: i64,
    /// G7: the row's VisualIndicators are up (challenge_running AddIndicator: spot_battle 887, hall_of_meat 882, until
    /// the turn's last sequence, spot_battle's active wipeout or end_turn's RemoveIndicator).
    pub indicators: bool,
    sequence_begun: bool,
    showed_owned: bool,
    top_sequence: i64,
    last_run: bool,
    // Timer ids (0: none).
    main_timer: u32,
    countdown_timer: u32,
    countdown_delay: u32,
    teleport_delay: u32,
    teleport_settle: u32,
    fade_in: u32,
    end_run_timer: u32,
    end_turn_delay: u32,
    completed_timer: u32,
    wait_pres: u32,
    /// HighestVote: (Score, PlayerID 1-based).
    highest_vote: (f64, i64),
    winning_highlight: bool,
    voted_highlight: bool,
    // VotingManager.
    votes: [u8; VOTE_SLOTS],
    vote_active: i64,
    pub voting_on: bool,
    /// Bumped at every challenge_running: the relay's run number.
    pub run: u32,
    /// Online, the turn is another console's: challenge_running's own timers wait for its relayed end.
    pub remote: bool,
    pub awaiting_end: bool,
    /// Online: the turn's mean vote, this console's own (to relay) or the turn's console's (relayed); and whether
    /// end_turn waits for it.
    pub tally: Option<f64>,
    pub awaiting_vote: bool,
    /// Trick.GetHOMScore for the turn's player, set before its wipeoutover.
    pub hom_score: i64,
    /// Online.IsOnline is false: Party Play (one pad; SetPlayer, the Press-A card, the results dialog).
    pub offline: bool,
    /// DoRestart: challenge_cleanup goes back to reset (Rematch, Restart Challenge).
    do_restart: bool,
    /// Online.IsTeamChallenge / Online.GetTeam: each player's team (1 / 2), empty for none; Teams.GetName's names.
    pub teams_in: Vec<u8>,
    pub team_names: [String; 2],
    /// vars.TeamsEnabled, TeamData.
    pub teams_enabled: bool,
    pub team_data: Vec<TeamData>,
}

impl Uniform {
    /// `first`: Random.GetRandomNumber's draw (reset 142: first % NumPlayers + 1).
    pub fn new(row: Row, intro: f64, players: usize, me: usize, first: usize) -> Self {
        Uniform {
            num_rounds: row.num_rounds,
            row,
            intro,
            first,
            me,
            n: players,
            lock_out: true,
            hud_loaded: false,
            players: vec![Player::default(); players],
            rank_map: (1..=players).collect(),
            enable_voting: false,
            use_dpo: false,
            cumulative: false,
            current_round: 0,
            round_changed: false,
            current_leader: -1,
            random_start: 0,
            index: 0,
            active: None,
            in_boundary: false,
            in_spot: Vec::new(),
            force_scoring: false,
            wiped: false,
            sequences: 0,
            indicators: false,
            sequence_begun: false,
            showed_owned: false,
            top_sequence: 0,
            last_run: false,
            main_timer: 0,
            countdown_timer: 0,
            countdown_delay: 0,
            teleport_delay: 0,
            teleport_settle: 0,
            fade_in: 0,
            end_run_timer: 0,
            end_turn_delay: 0,
            completed_timer: 0,
            wait_pres: 0,
            highest_vote: (-1., -1),
            winning_highlight: false,
            voted_highlight: false,
            votes: [VOTE_DEFAULT; VOTE_SLOTS],
            vote_active: 0,
            voting_on: false,
            run: 0,
            remote: false,
            awaiting_end: false,
            tally: None,
            awaiting_vote: false,
            hom_score: 0,
            offline: false,
            do_restart: false,
            teams_in: Vec::new(),
            team_names: ["T1".into(), "T2".into()],
            teams_enabled: false,
            team_data: Vec::new(),
        }
    }
    fn local(&self) -> usize {
        self.me + 1
    }
    /// hall_of_meat's challenge_running in place of spot_battle's (Challenge.GetChallengeType == HallOfMeatBattle).
    pub fn hom(&self) -> bool {
        self.row.challenge_type == Some(HOM_BATTLE)
    }
    fn p(&mut self, id: usize) -> Result<&mut Player, Abort> {
        id.checked_sub(1).and_then(|i| self.players.get_mut(i)).ok_or(Abort)
    }
    fn name(id: usize) -> String {
        // Challenge.GetPlayerName(id - 1): the glue's names stand behind the same ids.
        format!("P{}", id - 1)
    }
    /// The turn's player (PlayerID).
    pub fn actor(&self) -> Option<usize> {
        self.active.map(|a| a - 1)
    }
    /// The Players' Choice (Online.SetPlayersChoice), once highlight_reel has named it.
    pub fn choice(&self) -> Option<usize> {
        (self.voted_highlight && self.highest_vote.1 > 0).then(|| self.highest_vote.1 as usize - 1)
    }

    // ------------------------------------------------------------ VotingManager
    /// Online.StartVoting (0x82563bd8): a vote already open is reset first.
    fn start_voting(&mut self, rt: &mut Rt<St>, player: i64) {
        if self.voting_on {
            self.votes = [VOTE_DEFAULT; VOTE_SLOTS];
        }
        self.vote_active = player;
        self.voting_on = true;
        (self.tally, self.awaiting_vote) = (None, false);
        rt.note(format!("vote start {player}"));
    }
    /// Online.StopVoting (0x82563cc8): votingcomplete with the mean of the connected others' votes (0 with none), then
    /// every vote back to 1 and the active player to 0.
    fn stop_voting(&mut self, rt: &mut Rt<St>) {
        // Not retail (user 10-12): online, another console's turn takes that console's mean (relayed, `remote_voted`),
        // so a last-moment vote counts on every console or on none (matrix 10-12: a vote reaching some consoles after
        // their own stop named different Players' Choices). Retail's VotingManager sync is native, UNTRACED.
        let turns = usize::try_from(self.vote_active).ok().filter(|&a| !rt.gone.get(a).copied().unwrap_or(true));
        if self.remote && turns.is_some() {
            self.awaiting_vote = true;
            if let Some(score) = self.tally.take() {
                self.voted(rt, score);
            }
            return;
        }
        let score = self.mean(rt);
        self.tally = Some(score);
        self.voted(rt, score);
    }
    /// The turn's console's relayed mean (`None`: that console left, this one's own); end_turn goes on if it waited.
    pub fn remote_voted(&mut self, rt: &mut Rt<St>, score: Option<f64>) {
        let score = score.unwrap_or_else(|| self.mean(rt));
        if self.awaiting_vote {
            self.voted(rt, score);
        } else {
            self.tally = Some(score);
        }
    }
    fn mean(&self, rt: &Rt<St>) -> f64 {
        let (mut sum, mut count) = (0u32, 0u32);
        for i in 0..self.n {
            if i as i64 != self.vote_active && !rt.gone.get(i).copied().unwrap_or(true) {
                sum += u32::from(self.votes.get(i).copied().unwrap_or(VOTE_DEFAULT));
                count += 1;
            }
        }
        if count > 0 { f64::from(sum as f32 / count as f32) } else { 0. }
    }
    fn voted(&mut self, rt: &mut Rt<St>, score: f64) {
        self.awaiting_vote = false;
        rt.note(format!("vote stop {} {score:.3}", self.vote_active));
        rt.post(Msg::Voted { player: self.vote_active, score });
        self.voting_on = false;
        self.vote_active = 0;
        self.votes = [VOTE_DEFAULT; VOTE_SLOTS];
    }
    /// A player's vote (hudonlinevoting: 0 weak, 1 ok, 2 sweet, 3 gnarly).
    pub fn vote(&mut self, player: usize, level: u8) {
        if let Some(v) = self.votes.get_mut(player) {
            *v = level.min(3);
        }
    }

    // ------------------------------------------------------------ handlers
    /// [global] playerdisconnected (561-587).
    fn disconnected(&mut self, rt: &mut Rt<St>, p: usize) -> Handled {
        // Not retail (user 10-12): the turn's console left while end_turn waited for its mean: this console's own.
        if self.awaiting_vote && self.vote_active == p as i64 {
            self.remote_voted(rt, None);
        }
        let id = p + 1;
        if self.p(id)?.disconnected {
            return Ok(());
        }
        let pl = self.p(id)?;
        pl.disconnected = true;
        pl.score = -1;
        if self.hud_loaded {
            rt.board_int(p, -1);
        }
        let from = self.p(id)?.rank + 1;
        for i in from..=self.n as i64 {
            let i = i as usize;
            let at = self.rank_map[i - 1];
            if -1 < self.p(at)?.score {
                self.rank_map[i - 2] = at;
                self.rank_map[i - 1] = id;
                let before = self.rank_map[i - 2];
                self.p(before)?.rank = i as i64 - 1;
                self.p(id)?.rank = i as i64;
            }
        }
        if self.hud_loaded && !self.teams_enabled {
            for i in 0..self.n {
                rt.board_rank(i, self.players[i].rank);
            }
        }
        if self.teams_enabled {
            self.update_team_scores(rt);
        }
        Ok(())
    }

    /// vars:UpdateTeamScores (96-126): each team counts its best players in rank order, as many as the smaller team
    /// has (the rest ScoreIgnored; a gone player counts nothing but takes a place); the higher total ranks 1, a tie
    /// ranks both 2.
    fn update_team_scores(&mut self, rt: &mut Rt<St>) {
        let count = self.team_data[0].size.min(self.team_data[1].size);
        let (mut counted, mut scores) = ([0usize; 2], [0i64; 2]);
        for i in 0..self.n {
            let id = self.rank_map[i];
            let p = &mut self.players[id - 1];
            let t = p.team - 1;
            p.score_ignored = count <= counted[t];
            rt.note(format!("ignore {} {}", id - 1, p.score_ignored));
            if !p.score_ignored && !p.disconnected {
                scores[t] = scores[t].saturating_add(p.score); // relayed scores can be i64::MAX
                counted[t] += 1;
            }
        }
        for k in 0..2 {
            self.team_data[k].score = scores[k];
            rt.board_team_score(k + 1, scores[k]);
        }
        let ranks = match scores[0].cmp(&scores[1]) {
            std::cmp::Ordering::Greater => [1, 2],
            std::cmp::Ordering::Less => [2, 1],
            std::cmp::Ordering::Equal => [2, 2],
        };
        for k in 0..2 {
            self.team_data[k].rank = ranks[k];
            rt.board_team_rank(k + 1, ranks[k]);
        }
    }

    /// reset's stateenter (136-217), the online path.
    fn reset(&mut self, rt: &mut Rt<St>) {
        self.countdown_timer = 0;
        self.main_timer = 0;
        self.current_round = 1;
        self.round_changed = true;
        self.current_leader = -1;
        self.random_start = (self.first % self.n.max(1)) as i64 + 1;
        self.index = self.random_start - 1;
        self.active = None;
        self.in_spot.clear();
        self.force_scoring = false;
        self.cumulative = self.row.cumulative_scoring == Some(true);
        self.wiped = false;
        self.sequences = 0;
        // 153-164: offline (a Rematch) every player starts again.
        if self.offline {
            self.rank_map = (1..=self.n).collect();
            for (i, p) in self.players.iter_mut().enumerate() {
                (p.score, p.turn_score, p.rank, p.run_complete) = (0, 0, i as i64 + 1, false);
            }
        }
        if self.teams_enabled {
            for t in &mut self.team_data {
                (t.score, t.rank) = (0, 2);
            }
        }
        self.winning_highlight = false;
        self.voted_highlight = false;
        self.highest_vote = (-1., -1);
        if self.hom() {
            rt.show(Component::TrickDisplayHUD, false);
        } else {
            rt.show(Component::TrickDisplayHUD, true);
            rt.show(Component::HOMScoringHUD, false);
            // HOM.EnableHOMVisuals(false): the engine's Hall of Meat scorer is on only in a HOM Battle turn.
        }
        rt.show(Component::MiniMap, false);
        self.lock_out = false;
        self.do_restart = false;
        let teams = if self.teams_enabled { self.players.iter().map(|p| p.team as u8).collect() } else { Vec::new() };
        rt.board_init_teams(INT_STRING_INT, self.n, teams, self.team_names.clone());
        if self.cumulative {
            rt.board_variant("ID_VARIANT_SESSIONSPOT");
            rt.dpo_title("ID_COMMON_SCORE");
        }
        // EnableObjectives: no playable row has bonus objectives (Row::playable).
        rt.change(St::WaitForPlayers);
    }

    /// countdown's stateenter (220-318).
    fn countdown(&mut self, rt: &mut Rt<St>) -> Handled {
        if self.lock_out {
            return Ok(());
        }
        if let Some(old) = self.active {
            self.p(old)?.turn_score = 0;
        }
        if self.use_dpo {
            rt.show(Component::DPOHUD, false);
        }
        let n = self.n as i64;
        // ponytail: with every player gone the Lua loops for ever; capped here (the local player never leaves).
        for guard in 0.. {
            if guard > 10_000 {
                return Err(Abort);
            }
            self.index += 1;
            if self.index > n {
                self.index = 1;
            }
            let round_over = self.players.iter().all(|p| p.disconnected || p.run_complete);
            if round_over {
                for p in &mut self.players {
                    p.run_complete = false;
                }
                self.current_round += 1;
                self.index = self.random_start;
                self.round_changed = true;
            }
            if !self.p(self.index as usize)?.disconnected {
                break;
            }
        }
        let last_round = self.current_round == self.num_rounds;
        self.last_run = false;
        if last_round {
            let idx = self.index as usize;
            self.last_run = !self.players.iter().enumerate().any(|(i, p)| !p.disconnected && !p.run_complete && idx != i + 1);
        }
        if self.current_round > self.num_rounds {
            let active = self.active.ok_or(Abort)?;
            if !self.p(active)?.disconnected {
                if self.offline {
                    rt.set_player(active - 1);
                }
                rt.pin(active - 1, true);
            } else {
                rt.camera(self.index as usize - 1);
            }
            rt.change(St::ChallengeFinished);
        } else {
            let active = self.index as usize;
            self.active = Some(active);
            self.in_boundary = true;
            if self.teams_enabled {
                let score = self.p(active)?.score;
                rt.dpo_params(0, Some(score));
            }
            rt.turn(active - 1);
            if self.offline {
                self.teleport_delay = rt.add_timer(TELEPORT_DELAY);
            } else {
                rt.sync();
            }
        }
        Ok(())
    }

    /// countdown's timerexpired (321-390).
    fn countdown_timer(&mut self, rt: &mut Rt<St>, id: u32) -> Handled {
        if id == self.countdown_delay {
            rt.show(Component::LeaderboardHUD, true);
            self.countdown_timer = rt.add_timer(COUNTDOWN);
            rt.countdown(self.countdown_timer);
        } else if id == self.countdown_timer {
            self.countdown_timer = 0;
            rt.change(St::ChallengeRunning);
        } else if id == self.teleport_delay {
            let active = self.active.ok_or(Abort)?;
            for i in 1..=self.n {
                if i != active && !self.players[i - 1].disconnected {
                    rt.reposition(i - 1, Place { field: WAITING, index: 0, slot: i - 1 }, 0.);
                    if self.offline {
                        rt.set_player(i - 1);
                    }
                }
            }
            rt.reposition_watched(active - 1, Place { field: ATTEMPT, index: 0, slot: 0 }, 0.);
            rt.pin(active - 1, true);
            self.teleport_settle = rt.add_timer(TELEPORT_SETTLE);
        } else if id == self.teleport_settle {
            rt.close_loading();
            self.fade_in = rt.add_timer(FADE_IN_DELAY);
        } else if id == self.fade_in {
            rt.pause(false); // 1501
            let active = self.active.ok_or(Abort)?;
            if self.offline {
                // 363-367: the pad to the turn's player, and its Press-A card (hudcomplete starts the countdown).
                rt.set_player(active - 1);
                rt.press_a(active, self.current_round);
                return Ok(());
            }
            if self.teams_enabled {
                // 370-377: the team's colour and name, TeamIntroductionDelay before the countdown.
                let team = self.p(active)?.team;
                let name = self.team_data[team - 1].name.clone();
                if active == self.local() {
                    rt.team_central(&self.row.your_turn_team.clone(), team as i64 - 1, NORMAL, &[name, Self::name(active)]);
                } else {
                    rt.team_central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN", team as i64 - 1, NORMAL, &[name]);
                }
                self.countdown_delay = rt.add_timer(self.row.team_intro_delay);
            } else {
                let id = if active == self.local() { self.row.your_turn.clone() } else { "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN".into() };
                rt.central(&id, WHITE, &[Self::name(active)], 0);
                self.countdown_delay = rt.add_timer(self.row.countdown_delay);
            }
            if self.round_changed {
                rt.modified("ID_CONTEST_CENTRALMESSAGE_ROUNDNUMBER", WHITE, &[self.current_round.to_string()], false);
            }
        }
        Ok(())
    }

    /// The turn ends on this console's clock, or (another console's turn) when its relayed end comes.
    fn end_turn_now(&mut self, rt: &mut Rt<St>) {
        if self.remote {
            self.awaiting_end = true;
        } else {
            rt.change(St::EndTurn);
        }
    }
    /// The relayed end of another console's turn.
    pub fn remote_end(&mut self, rt: &mut Rt<St>) {
        if rt.state == St::ChallengeRunning {
            self.awaiting_end = false;
            rt.change(St::EndTurn);
        }
    }

    /// spot_battle's challenge_running sequencecomplete (950-1073).
    fn sequence(&mut self, rt: &mut Rt<St>, player: usize, score: i64, wiped: bool) -> Handled {
        let id = player + 1;
        if Some(id) != self.active || wiped || !(self.in_boundary || self.force_scoring) {
            return Ok(());
        }
        self.sequence_begun = false;
        self.sequences += 1;
        let mut seq = score;
        if self.cumulative {
            let p = self.p(id)?;
            p.turn_score = p.turn_score.saturating_add(score);
            seq = p.turn_score;
        }
        let new_high = self.rank_up(rt, player, seq)?;
        if self.teams_enabled {
            let best = self.p(id)?.score;
            rt.dpo_params(seq, Some(best));
        } else if self.cumulative {
            let first = self.rank_map[0];
            let (turn, best) = (self.p(id)?.turn_score, self.p(first)?.score);
            rt.dpo_params(turn, Some(best));
        }
        if new_high {
            // 1004-1016: with teams the lead is the team's.
            let team = self.p(id)?.team;
            let (in_first, leader) =
                if self.teams_enabled { (self.team_data[team - 1].rank == 1, team as i64) } else { (self.p(id)?.rank == 1, id as i64) };
            let mut new_leader = false;
            if in_first && self.current_leader != leader {
                self.current_leader = leader;
                new_leader = true;
            }
            if in_first {
                if !self.showed_owned {
                    let msg = if new_leader { self.row.spot_owned.clone() } else { "ID_DOMINATION_CENTRAL_MESSAGE_SCOREINCREASED".into() };
                    if self.teams_enabled {
                        rt.team_central(&msg, team as i64 - 1, WINNING, &[]);
                    } else {
                        rt.central(&msg, WHITE, &[], 0);
                    }
                    self.showed_owned = true;
                }
                if score > self.top_sequence {
                    self.top_sequence = score;
                    self.winning_highlight = true;
                }
            }
        }
        if self.force_scoring {
            self.force_scoring = false;
        }
        if !self.cumulative && self.sequences >= self.row.num_sequences_to_end_turn {
            self.indicators = false;
            rt.remove_timer(self.main_timer);
            self.main_timer = 0;
            rt.show(Component::TimerHUD, false);
            self.main_timer = 0;
            rt.show(Component::TimerHUD, false);
            if self.last_run {
                rt.change(St::EndTurn);
            } else {
                self.end_run_timer = rt.add_timer(END_RUN);
            }
        }
        Ok(())
    }

    /// The turn's score against the board (spot_battle 960-990, hall_of_meat 947-976): a new best is set on the
    /// leaderboard and climbs the ranks past every lower score. Whether it was a new best.
    fn rank_up(&mut self, rt: &mut Rt<St>, player: usize, seq: i64) -> Result<bool, Abort> {
        let id = player + 1;
        let mut new_high = false;
        if seq > self.p(id)?.score {
            self.p(id)?.score = seq;
            rt.board_int(player, seq);
            new_high = true;
        }
        let mut require_update = false;
        for i in 1..=self.n {
            let at = self.rank_map[i - 1];
            if at == id {
                break;
            }
            if seq > self.p(at)?.score {
                let from = self.p(id)?.rank;
                let mut j = from;
                while j >= i as i64 + 1 {
                    let j_ = j as usize;
                    self.rank_map[j_ - 1] = self.rank_map[j_ - 2];
                    let moved = self.rank_map[j_ - 1];
                    self.p(moved)?.rank = j;
                    j -= 1;
                }
                self.rank_map[i - 1] = id;
                self.p(id)?.rank = i as i64;
                require_update = true;
                break;
            }
        }
        if self.teams_enabled {
            self.update_team_scores(rt);
        }
        if require_update {
            for i in 0..self.n {
                rt.board_rank(i, self.players[i].rank);
            }
        }
        Ok(new_high)
    }

    /// hall_of_meat's challenge_running wipeoutover (943-1000): the bail's Hall of Meat score is the turn's, and the turn
    /// ends.
    fn bail_over(&mut self, rt: &mut Rt<St>, player: usize) -> Handled {
        let id = player + 1;
        if Some(id) != self.active {
            return Ok(());
        }
        let new_high = self.rank_up(rt, player, self.hom_score)?;
        if self.teams_enabled {
            let best = self.p(id)?.score;
            rt.dpo_params(self.hom_score, Some(best));
        }
        if new_high && self.p(id)?.rank == 1 {
            self.winning_highlight = true;
        }
        rt.change(St::EndTurn);
        Ok(())
    }

    /// end_turn's way on after the vote (or with none): the run's flags, the loading screen between turns (749-763,
    /// 800-812).
    fn after_turn(&mut self, rt: &mut Rt<St>) {
        self.wiped = false;
        self.force_scoring = false;
        self.round_changed = false;
        if !self.last_run {
            rt.reset_effect(Effect::FadeOut);
            rt.open_loading();
        }
        self.end_turn_delay = rt.add_timer(END_TURN_DELAY);
    }
}

impl Graph for Uniform {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::Setup => "setup",
            St::Reset => "reset",
            St::WaitForPlayers => "wait_for_players",
            St::DynamicCharacterNis => "dynamic_character_nis",
            St::NisFlythrough => "nis_flythrough",
            St::Countdown => "countdown",
            St::ChallengeRunning => "challenge_running",
            St::EndTurn => "end_turn",
            St::ChallengeFinished => "challenge_finished",
            St::HighlightReel => "highlight_reel",
            St::Leaderboard => "leaderboard",
            St::ChallengeCleanup => "challenge_cleanup",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        use St::*;
        if let Msg::Left(p) = msg {
            self.disconnected(rt, *p)?;
        }
        // [global] restartchallenge (544-552): offline only.
        if *msg == Msg::Restart {
            if self.offline && !self.lock_out {
                self.do_restart = true;
                self.lock_out = true;
                rt.pause(true);
                rt.change(ChallengeCleanup);
            }
            return Ok(());
        }
        match (rt.state, msg) {
            (Initial, Msg::Enter) => {
                self.lock_out = true;
                self.hud_loaded = false;
                for (i, p) in self.players.iter_mut().enumerate() {
                    // Online.GetTeam (0x828463d8: the player's +112, plus 1).
                    let team = if self.teams_in.get(i) == Some(&2) { 2 } else { 1 };
                    *p = Player { rank: i as i64 + 1, disconnected: rt.gone[i], team, ..default() };
                }
                // 39-54: TeamsEnabled (Online.IsTeamChallenge: never offline), each team's size.
                self.teams_enabled = !self.offline && !self.teams_in.is_empty();
                self.team_data = self.team_names.iter().map(|name| TeamData { rank: 2, name: name.clone(), ..default() }).collect();
                if self.teams_enabled {
                    for p in &self.players {
                        self.team_data[p.team - 1].size += 1;
                    }
                }
                self.rank_map = (1..=self.n).collect();
                self.num_rounds = self.row.num_rounds;
                rt.pause(true); // 1193
                rt.change(Setup);
            }
            (Setup, Msg::Enter) => {
                self.enable_voting = !self.offline && !self.teams_enabled && MIN_VOTERS <= self.n;
                // UseDPO: teams, or ChallengeType SpotBattle.
                self.use_dpo = self.teams_enabled || self.row.challenge_type == Some(SPOT_BATTLE);
                // HUD2.CreateGroup: hudloaded once the group's movies are up.
                rt.post(Msg::HudLoaded);
                self.lock_out = false;
                // Offline, hudloaded moves on (HUDLoaded stays false).
                if !self.offline {
                    self.hud_loaded = true;
                    rt.change(Reset);
                }
            }
            (Setup, Msg::HudLoaded) => rt.change(Reset),
            (Reset, Msg::Enter) => self.reset(rt),
            (WaitForPlayers, Msg::Enter) => self.wait_pres = rt.add_timer(WAIT_PRES),
            (WaitForPlayers, Msg::Timer(id)) if *id == self.wait_pres => rt.change(DynamicCharacterNis),
            // 829-858: teams with an IntroNIS, the team intro with both teams' skaters (team 1 padded to 3), nis_flythrough
            // at its notify. ponytail: the NIS player with dynamic characters is not ported (no scene plays), as race.
            (DynamicCharacterNis, Msg::Enter) if self.teams_enabled && self.row.intro_nis.is_some() => rt.nis(0.),
            (DynamicCharacterNis, Msg::Enter | Msg::Nis) => rt.change(NisFlythrough),
            (NisFlythrough, Msg::Enter) => rt.nis(self.intro),
            (NisFlythrough, Msg::Nis) => {
                for i in 0..self.n {
                    if !self.players[i].disconnected {
                        rt.reposition(i, Place { field: WAITING, index: 0, slot: i }, 1.);
                        if self.offline {
                            rt.set_player(i);
                        }
                    }
                }
                rt.pin(self.me, true);
                rt.change(Countdown);
            }
            (Countdown, Msg::Enter) => self.countdown(rt)?,
            (Countdown, Msg::Timer(id)) => self.countdown_timer(rt, *id)?,
            // 392-396: the card answered.
            (Countdown, Msg::HudComplete) => {
                rt.show(Component::LeaderboardHUD, true);
                self.countdown_delay = rt.add_timer(self.row.countdown_delay);
            }
            // ponytail: [countdown] skaterteleportcomplete (418-423) re-arms TeleportDelayTimer offline; taken as written
            // every turn's own teleports re-arm it for ever (each repositions everyone at once), so it is not taken.
            (Countdown, Msg::Synced) => {
                rt.pin(self.me, true);
                self.teleport_delay = rt.add_timer(TELEPORT_DELAY);
            }
            (Countdown, Msg::Left(p)) => {
                if Some(p + 1) == self.active {
                    if self.countdown_timer != 0 {
                        rt.remove_timer(self.countdown_timer);
                        self.countdown_timer = 0;
                        rt.show(Component::CountdownHUD, false);
                    }
                    rt.change(EndTurn);
                }
            }
            (ChallengeRunning, Msg::Enter) => {
                if self.lock_out {
                    return Ok(());
                }
                let active = self.active.ok_or(Abort)?;
                self.run += 1;
                self.awaiting_end = false;
                rt.pin(active - 1, false);
                self.main_timer = rt.add_timer(self.row.time_limit);
                rt.timer(self.main_timer);
                if self.enable_voting {
                    self.start_voting(rt, active as i64 - 1);
                }
                // hall_of_meat's stateenter (862-883) has no cumulative block and no spot state.
                if self.teams_enabled {
                    // 875-879 (HOM 875-880): the turn's best, the Challenge icon, the team's colour.
                    let (score, team) = (self.p(active)?.score, self.p(active)?.team);
                    rt.dpo_params(0, Some(score));
                    rt.dpo_team(team as i64 - 1);
                    rt.show(Component::DPOHUD, true);
                } else if self.cumulative && !self.hom() {
                    let first = self.rank_map[0];
                    let best = self.p(first)?.score;
                    rt.dpo_params(0, Some(best));
                    rt.show(Component::DPOHUD, true);
                }
                self.in_spot = vec![false; self.row.spot_volumes.len()];
                self.in_boundary = false;
                self.sequence_begun = false;
                self.showed_owned = false;
                self.sequences = 0;
                self.top_sequence = 0;
                self.indicators = true;
            }
            (ChallengeRunning, Msg::Timer(id)) => {
                if *id == self.main_timer {
                    self.main_timer = 0;
                    rt.show(Component::TimerHUD, false);
                    if self.hom() {
                        rt.central(&self.row.out_of_time.clone(), WHITE, &[], 0);
                    }
                    if self.last_run {
                        self.end_turn_now(rt);
                    } else {
                        self.end_run_timer = rt.add_timer(END_RUN);
                    }
                } else if *id == self.end_run_timer {
                    rt.start_effect(Effect::FadeOut, END_RUN_FADE);
                    self.end_turn_now(rt);
                }
            }
            // hall_of_meat's trigger handlers set only InScoringBoundary for its ScoringBoundary, which nothing reads.
            (ChallengeRunning, Msg::Trigger { player, volume, inside }) if !self.hom() => {
                if Some(player + 1) == self.active {
                    let hit = usize::try_from(*volume).ok().filter(|&v| v < self.in_spot.len());
                    if *inside {
                        if let Some(v) = hit {
                            self.in_spot[v] = true;
                            self.in_boundary = true;
                        }
                    } else {
                        self.in_boundary = false;
                        let mut was = false;
                        for (i, s) in self.in_spot.iter_mut().enumerate() {
                            if hit == Some(i) {
                                was = true;
                                *s = false;
                            }
                            self.in_boundary = self.in_boundary || *s;
                        }
                        if was && !self.in_boundary && self.sequence_begun {
                            self.force_scoring = true;
                        }
                    }
                }
            }
            // hall_of_meat has no sequence handlers.
            (ChallengeRunning, Msg::SequenceBegin(p)) if !self.hom() => {
                if Some(p + 1) == self.active {
                    self.sequence_begun = true;
                }
            }
            (ChallengeRunning, Msg::Sequence { player, score, wiped }) if !self.hom() => self.sequence(rt, *player, *score, *wiped)?,
            (ChallengeRunning, Msg::WipeoutOver(p)) if self.hom() => self.bail_over(rt, *p)?,
            (ChallengeRunning, Msg::WipeoutOver(p)) => {
                if Some(p + 1) == self.active {
                    rt.change(EndTurn);
                }
            }
            // hall_of_meat's wipeout (920-931): once per turn, no pin.
            (ChallengeRunning, Msg::Wipeout(p)) if self.hom() => {
                if Some(p + 1) == self.active && !self.wiped {
                    rt.show(Component::TimerHUD, false);
                    rt.remove_timer(self.main_timer);
                    self.wiped = true;
                }
            }
            (ChallengeRunning, Msg::Wipeout(p)) => {
                if Some(p + 1) == self.active {
                    rt.show(Component::TimerHUD, false);
                    rt.remove_timer(self.main_timer);
                    rt.pin(*p, true);
                    self.sequence_begun = false;
                    self.force_scoring = false;
                    self.wiped = true;
                    self.indicators = false;
                    // IsAPhysicsTeleport: the engine's wipeouts are never one.
                }
            }
            (ChallengeRunning, Msg::AlmostOver { player, time }) => {
                let active = self.active.ok_or(Abort)?;
                let base = if self.local() == active { GHOST } else { active - 1 };
                if !self.last_run && *player == base {
                    rt.start_effect(Effect::FadeOut, *time);
                }
            }
            (ChallengeRunning, Msg::Left(p)) => {
                if Some(p + 1) == self.active {
                    if self.main_timer != 0 {
                        rt.remove_timer(self.main_timer);
                        self.main_timer = 0;
                        rt.show(Component::TimerHUD, false);
                    }
                    rt.change(EndTurn);
                }
            }
            (EndTurn, Msg::Enter) => {
                if self.lock_out {
                    return Ok(());
                }
                self.indicators = false;
                let index = self.index as usize;
                self.p(index)?.run_complete = true;
                self.main_timer = 0;
                rt.show(Component::TimerHUD, false);
                if self.cumulative {
                    rt.show(Component::DPOHUD, false);
                }
                self.main_timer = rt.add_timer(if self.wiped { END_TURN_WIPED } else { END_TURN });
            }
            (EndTurn, Msg::Timer(id)) => {
                if *id == self.main_timer {
                    if self.enable_voting {
                        self.stop_voting(rt);
                    } else {
                        self.after_turn(rt);
                    }
                } else if *id == self.end_turn_delay {
                    rt.change(Countdown);
                }
            }
            (EndTurn, Msg::Voted { player, score }) => {
                let id = player + 1;
                let disconnected = id > 0 && id <= self.n as i64 && self.players[id as usize - 1].disconnected;
                if 1. < *score && *score > self.highest_vote.0 && !disconnected {
                    self.highest_vote = (*score, id);
                    self.voted_highlight = true;
                }
                self.after_turn(rt);
            }
            (ChallengeFinished, Msg::Enter) => {
                if self.lock_out {
                    return Ok(());
                }
                rt.reset_effect(Effect::FadeOut);
                rt.close_loading();
                rt.pause(true); // 1734
                self.completed_timer = rt.add_timer(COMPLETED);
                rt.central(&self.row.complete.clone(), WHITE, &[], 0);
            }
            // 612-625: with teams the winning team (team 1 unless team 2 ranks 1: a tie names team 1) and 3 s more.
            (ChallengeFinished, Msg::Timer(id)) if *id == self.completed_timer => {
                if self.teams_enabled {
                    let w: i64 = if self.team_data[1].rank == 1 { 2 } else { 1 };
                    rt.note(format!("winningteam {w}"));
                    rt.team_central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS", w - 1, WINNING, &[self.team_data[w as usize - 1].name.clone()]);
                    self.main_timer = rt.add_timer(COMPLETED);
                } else {
                    rt.change(HighlightReel);
                }
            }
            (ChallengeFinished, Msg::Timer(id)) if *id == self.main_timer => rt.change(HighlightReel),
            (HighlightReel, Msg::Enter) => {
                if self.lock_out {
                    return Ok(());
                }
                if self.winning_highlight {
                    let winner = self.players.iter().position(|p| p.rank == 1).map_or(-1, |i| i as i64);
                    rt.note(format!("winner {winner}"));
                }
                if self.voted_highlight {
                    rt.note(format!("choice {}", self.highest_vote.1 - 1));
                }
                rt.reel();
            }
            (HighlightReel, Msg::Reel) => rt.change(Leaderboard),
            (Leaderboard, Msg::Enter) => {
                rt.pause(false); // 1638
                for i in 0..self.n {
                    let (rank, score) = (self.players[i].rank, self.players[i].score);
                    rt.result(i, rank, score);
                    // 520-524: Results_SetPlayerScoreIgnored, team 2's on the second list.
                    let p = &self.players[i];
                    let tag = if !self.teams_enabled || p.team == 1 { "resultignored" } else { "resultignored2" };
                    rt.note(format!("{tag} {i} {}", p.score_ignored));
                }
                // 525-529: offline the results dialog (Rematch / Exit Party Play / Change settings).
                if self.offline {
                    rt.continue_retry();
                } else {
                    rt.change(ChallengeCleanup);
                }
            }
            (Leaderboard, Msg::HudContinue) => rt.change(ChallengeCleanup),
            (Leaderboard, Msg::HudRetry) => {
                rt.pause(true);
                self.do_restart = true;
                rt.change(ChallengeCleanup);
            }
            (ChallengeCleanup, Msg::Enter) => {
                // The Lua's own: SwitchCamera (online) / UnpinAI with LocalPlayerID, the 1-based id.
                if !self.offline {
                    rt.camera(self.local());
                }
                rt.pin(self.local(), false);
                rt.reset_effect(Effect::FadeOut);
                rt.close_loading();
                rt.remove_timer(self.main_timer);
                rt.remove_timer(self.countdown_timer);
                rt.dialog = false;
                if self.do_restart {
                    rt.change(Reset);
                } else {
                    self.voting_on = false;
                    rt.change(Complete);
                }
            }
            (Complete, Msg::Enter) => {
                if self.offline {
                    // Challenge.CompleteChallenge: the party session ends the match.
                    rt.left = true;
                } else {
                    rt.leave();
                }
                rt.pause(false); // 1631
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// A turn event the turn's console relays, or a vote from any console.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Relay {
    pub run: u32,
    pub ev: RelayEv,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum RelayEv {
    Trigger(i64, bool),
    Begin,
    Sequence(i64),
    Wipeout,
    Almost(f64),
    /// With the bail's Hall of Meat score (HOM Battle).
    Over(i64),
    End,
    Vote(u8),
    /// The turn's mean vote (Online.StopVoting on the turn's console).
    Voted(f64),
}

/// What the local skater did this tick, in the graph's words.
#[derive(Clone, Debug)]
enum Seen {
    Trigger(i64, bool),
    Begin,
    Sequence(i64),
    Wipeout,
    Almost(f64),
    Over(i64),
}

#[derive(Default)]
struct Probe {
    inside: Vec<bool>,
    open: bool,
    wiping: bool,
    pending: bool,
    /// The bail as wipeoutover sees it: the Hall of Meat scorer's own while it is on, else the category's.
    over: bool,
    seen: Vec<Seen>,
}

struct Play {
    id: String,
    row: Row,
    g: Uniform,
    rt: Rt<St>,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    inbox: Vec<(usize, Relay)>,
    placed: Vec<u32>,
    intro: Option<f64>,
    closed: bool,
    sent_end: u32,
    vote: u8,
    label: &'static str,
    /// G7: the row's VisualIndicators as skate3_indicators::show items (none without the install's looks).
    markers: Vec<(String, Matrix)>,
    /// Its outlines as ribbons in the mode's style, shown with the markers.
    ribbons: Vec<crate::skate2_arrows::Ribbon>,
}

/// The mode's indicator textures (the outlines' blue among them) by its label.
fn outline_dir(label: &str) -> &'static str {
    if label == "HOM Battle" { "private/skate3-online/hall_of_meat/indicator" } else { "private/skate3-online/spot_battle/indicator" }
}

/// G7: the mode's looks (tools/skate3_online.py look_file) by its label.
fn indicators(label: &str) -> &'static str {
    if label == "HOM Battle" { "private/skate3-online/hall_of_meat/indicators.json" } else { "private/skate3-online/spot_battle/indicators.json" }
}

#[derive(Resource, Default)]
pub(crate) struct UniformGame {
    spot: Option<ModeData>,
    hom: Option<ModeData>,
    play: Option<Play>,
    probe: Probe,
    /// The spot volumes the probe watches while the local skater's turn is live.
    volumes: Vec<Volume>,
    live: bool,
    /// The local skater's HOM Battle turn: the engine's Hall of Meat scorer runs (hall_of_meat::gate).
    pub hom_live: bool,
    warned: String,
}

/// The [`Playable`] entries for a battle's playable rows: each row, and its Team tab entry (TeamsEnabled, 3 a side).
fn entries(ids: &[String]) -> Vec<String> {
    ids.iter().cloned().chain(ids.iter().map(|id| crate::match_core::team(id))).collect()
}

pub(crate) struct Skate3UniformPlugin;
impl Plugin for Skate3UniformPlugin {
    fn build(&self, app: &mut App) {
        let [spot, hom] = [("spot_battle", "Spot Battle"), ("hall_of_meat", "HOM Battle")].map(|(mode, label)| {
            let data = ModeData::install(app, mode, label)?;
            let ids: Vec<String> =
                data.rows.keys().filter(|id| data.row::<Row>(id).is_ok_and(|r| r.playable(id))).cloned().collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                playable.0.extend(entries(&ids));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
            Some(data)
        });
        app.insert_resource(UniformGame { spot, hom, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick while the local skater's turn is live: the spot volumes (trigger enter / exit), sequences
/// (sequencebegin / sequencecomplete) and the wipeout (wipeout, wipeoutalmostover at the engine's reset countdown,
/// wipeoutover).
fn probe(mut game: ResMut<UniformGame>, skater: Res<SkaterRuntime>) {
    let UniformGame { probe, volumes, live, .. } = &mut *game;
    if !*live {
        *probe = Probe::default();
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let at = Vec3::new(at[0], at[1], at[2]);
    probe.inside.resize(volumes.len(), false);
    for (i, (was, v)) in probe.inside.iter_mut().zip(volumes.iter()).enumerate() {
        let inside = v.contains(at);
        if inside != *was {
            *was = inside;
            probe.seen.push(Seen::Trigger(i as i64, inside));
        }
    }
    let s = &skater.scoring;
    let open = s.sequence_open();
    if open && !probe.open {
        probe.seen.push(Seen::Begin);
    }
    probe.open = open;
    if s.sequence_landed {
        probe.seen.push(Seen::Sequence(s.lua_sequence_score()));
    }
    let wiping = skater.wiping_out();
    let w = &skater.wipeout_state.state;
    let pending = wiping && w.teleport_pending;
    if wiping && !probe.wiping {
        probe.seen.push(Seen::Wipeout);
    }
    if pending && !probe.pending {
        probe.seen.push(Seen::Almost(f64::from(w.time_until_teleport)));
    }
    // A HOM Battle turn ends at the reset countdown, while the screen is black, not after the skater is back up.
    let over = skater.hall_of_meat.wiping().unwrap_or(wiping);
    if !over && probe.over {
        // Trick.GetHOMScore: ponytail: read as the engine scorer's last published bail score (the panel's
        // HOM_GetScore); the native's own source is not traced. 0 outside a HOM Battle (scorer off).
        probe.seen.push(Seen::Over(skater.hall_of_meat.score()));
    }
    (probe.wiping, probe.pending, probe.over) = (wiping, pending, over);
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<UniformGame>| {
        let hom = world.resource::<Match>().mode == Some(Mode::HallOfMeat);
        let slot = if hom { &mut game.hom } else { &mut game.spot };
        let Some(data) = slot.take() else {
            if game.play.is_some() {
                end(world, &mut game);
            }
            game.live = false;
            game.hom_live = false;
            return;
        };
        step(world, &mut game, &data, if hom { "HOM Battle" } else { "Spot Battle" });
        *(if hom { &mut game.hom } else { &mut game.spot }) = Some(data);
    });
}

/// customiser::Navigation's XInput bits: hudonlinevoting puts A by Weak, B by OK, X by Sweet and Y by Gnarly.
/// ponytail: the C++ that reads the vote buttons is not traced; the movie's layout pairs them.
const PAD_VOTES: [(u16, u8); 4] = [(0x1000, 0), (0x2000, 1), (0x4000, 2), (0x8000, 3)];

fn step(world: &mut World, game: &mut UniformGame, data: &ModeData, label: &'static str) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && matches!(mode, Some(Mode::SpotBattle | Mode::HallOfMeat)) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 {label}: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
            game.warned = id;
        }
        game.live = false;
        game.hom_live = false;
        return;
    }
    let row = row.expect("on_map");
    // Another row under the same mode (a Party Play relaunch): the old game ends first.
    if game.play.as_ref().is_some_and(|p| p.id != id) {
        end(world, game);
    }
    if game.play.is_none() {
        if !matches!(phase, Phase::Running { .. }) {
            world.resource_mut::<Match>().hold = true;
            return;
        }
        game.play = Some(begin(world, data, &id, row, now, label));
    }
    let play = game.play.as_mut().expect("begun");

    // Who left: playerdisconnected.
    if play.online {
        let m = world.resource::<Match>();
        let left: Vec<usize> = (0..play.names.len()).filter(|&p| !play.rt.gone[p] && m.players.get(p).is_none_or(String::is_empty)).collect();
        for p in left {
            play.rt.gone[p] = true;
            skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
        }
    }
    let (online, me) = (play.online, play.me);
    let local_turn = |g: &Uniform| g.actor().is_some_and(|a| !online || a == me);
    play.g.remote = play.online && !local_turn(&play.g);
    skate3_modes::advance(&mut play.g, &mut play.rt, now);
    // Party Play: the card, the results dialog, the pad's player and the others' waiting slots out; A, Rematch and
    // Restart Challenge back.
    if !play.online {
        let actor = play.g.actor();
        let waiting = (0..play.names.len())
            .map(|p| match play.rt.landed(p) {
                Some((_, place)) if Some(p) != actor && place.field == WAITING => play.row.place(place),
                _ => None,
            })
            .collect();
        let results = play.rt.dialog.then(|| {
            let view = View { names: &play.names, me: play.me, online: false, title: data.text(&play.row.title), label };
            skate3_modes::results_answers(data, &play.rt.hud, &view, &results_setup(data, &play.row, &play.g), None)
        });
        let turn = crate::party_play::Turn { controlled: actor.unwrap_or(0), waiting, card: play.rt.card.take(), results };
        let answers = crate::party_play::exchange(world, turn);
        for msg in answers.messages() {
            skate3_modes::dispatch(&mut play.g, &mut play.rt, msg);
        }
        skate3_modes::advance(&mut play.g, &mut play.rt, now);
    }
    let actor = play.g.actor();
    let local_turn = local_turn(&play.g);
    play.g.remote = play.online && !local_turn;

    // This console's turn: what the skater did, relayed online.
    let running = play.rt.state == St::ChallengeRunning;
    game.live = local_turn && running;
    game.hom_live = game.live && play.g.hom();
    game.volumes = play.row.spot_volumes.iter().flatten().cloned().collect();
    let seen = std::mem::take(&mut game.probe.seen);
    let pressed = world.get_resource::<crate::customiser::Navigation>().map_or(0, |n| n.pressed);
    let received: Vec<(usize, Relay)> = if play.online {
        world.resource_mut::<Match>().inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect()
    } else {
        Vec::new()
    };
    for relay in play.exchange(seen, pressed, received) {
        send(world, relay);
    }
    // RepositionAI: this console's skater (hot-seat: the turn's start, on the one skater).
    let who = if play.online { play.me } else { actor.unwrap_or(0) };
    if let Some((seq, place)) = play.rt.landed(who)
        && play.placed[who] != seq
    {
        play.placed[who] = seq;
        if (play.online || place.field == ATTEMPT)
            && let Some(at) = play.row.place(place)
        {
            world.resource_mut::<Match>().teleport = Some(at);
        }
    }

    // The intro (nis_flythrough) on the row's NIS flythrough.
    match (play.rt.presenting, play.intro) {
        (Some(end), None) if now < end => {
            if let Some(scene) = &play.row.intro {
                world.resource_mut::<Match>().title = data.text(&play.row.title);
                world.resource_mut::<Cutscene>().start(&data.root, scene);
                world.resource_mut::<Match>().intro = true;
            }
            play.intro = Some(end);
        }
        (_, Some(end)) if now >= end => {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
            play.intro = Some(f64::INFINITY);
            play.rt.presenting = None;
        }
        _ => {}
    }
    // The loading screen: up from the start, the intro showing through it (UNRESOLVED R1, as S.K.A.T.E.), then as the
    // Lua opens and closes it (CloseLoadingScreen at each turn's teleport 359, OpenLoadingScreen between turns 761).
    play.closed |= !play.rt.covered;
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = play.rt.covered && (play.closed || play.intro.is_none());
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        let alpha = play.rt.fade_alpha(now) as f32;
        if fade.0 != alpha {
            fade.0 = alpha;
        }
    }
    // G7: the row's markers while the turn runs.
    let shown = play.rt.state == St::ChallengeRunning && play.g.indicators;
    let (markers, ribbons) = if shown { (play.markers.clone(), play.ribbons.clone()) } else { Default::default() };
    crate::skate3_indicators::show_all(world, indicators(label), markers, ribbons);
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = play.rt.pinned.get(who).copied().unwrap_or(true) || play.rt.presenting_now();
        // The results (their rows set: endchallenge_online, Party Play's dialog) own the pad.
        m.gestures = skate3_modes::gestures(&m, play.rt.left || !play.rt.hud.results.is_empty());
        m.watch = (play.online && !play.rt.left && play.rt.watching != play.me).then_some(play.rt.watching);
        m.score = play.g.players.get(play.me).map_or(0, |p| p.score);
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
        // The round sync points: ours out, passed once every console has reached them.
        m.round = play.rt.syncs;
        play.rt.release(&m.rounds);
    }
    let view = View { names: &play.names, me: play.me, online: play.online, title: data.text(&play.row.title), label };
    // Offline the results are Party Play's dialog (above).
    let results = (play.online && !play.rt.hud.results.is_empty()).then(|| results_setup(data, &play.row, &play.g));
    play.hud.teams = play.g.teams_enabled.then(|| team_results(data, &play.g, &play.names, &view.title, play.online));
    play.hud.drive(world, data, &play.rt.hud, &view, results.as_ref(), now);
}

/// endchallenge_teams' data block (skate3_modes::team_block) for a team battle. leaderboard (509-524) makes
/// RoundResults the active group and sets every player's Rank and Score (team 2's ScoreIgnored on the second list);
/// the team rows are TeamData's rank and score (OnlineChallengeComplete's TeamResults, 699-707). Results_SetIsFinalResults
/// is never called: its reset value (final) stands. ponytail: which rows the native puts in each team's list from the
/// player-indexed SetPlayerData is not traced: each team's players by rank; the ignored scores are not drawn apart
/// (UNRECOVERED).
fn team_results(data: &ModeData, g: &Uniform, names: &[String], title: &str, online: bool) -> serde_json::Value {
    let members = |team: usize| -> Vec<usize> { g.rank_map.iter().map(|&id| id - 1).filter(|&p| g.players[p].team == team).collect() };
    let table = |k: usize| skate3_modes::TeamTable {
        name: g.team_data[k].name.clone(),
        rank: g.team_data[k].rank,
        score: skate3_modes::team_int(data, g.team_data[k].score),
        status: skate3_modes::team_int(data, 0),
        members: (members(k + 1).into_iter())
            .map(|p| {
                let name = format!("#{}", names.get(p).map_or("", String::as_str));
                let pl = &g.players[p];
                [skate3_modes::team_int(data, pl.rank), name, skate3_modes::team_int(data, pl.score), String::new()]
            })
            .collect(),
    };
    let team = g.players.get(g.me).map_or(0, |p| p.team.saturating_sub(1)).min(1);
    let row = members(team + 1).iter().position(|&p| p == g.me).map_or(-1, |i| i as i64);
    skate3_modes::team_block(online, title, true, (team, row), PLACE_NAME_INT, [table(0), table(1)])
}

/// leaderboard's Results_SetColTitle / Results_SetResultsMenuType (501-516).
fn results_setup(data: &ModeData, row: &Row, g: &Uniform) -> ResultsSetup {
    ResultsSetup {
        title: data.text(&row.title),
        layout: if g.enable_voting { PLACE_NAME_INT_CHOICE } else { PLACE_NAME_INT },
        columns: ["ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_NAME_HEADER", "ID_LEADERBOARD_POINTS_HEADER", "ID_PLAYERS_CHOICE"]
            .map(|c| data.text(c))
            .to_vec(),
        throwdown: 0,
        choice: g.choice(),
        times: Vec::new(),
        floats: Vec::new(),
        strings: Vec::new(),
    }
}

impl Play {
    /// This console's turn: what the skater did (`seen`), relayed online; its end; this player's vote on another's
    /// run (`pressed`: the pad's buttons); then the relays (`received`): another console's turn as it comes, and the
    /// votes (an early one waits for its run). Returns what to relay.
    fn exchange(&mut self, seen: Vec<Seen>, pressed: u16, received: Vec<(usize, Relay)>) -> Vec<Relay> {
        let mut out = Vec::new();
        let actor = self.g.actor();
        let local_turn = actor.is_some_and(|a| !self.online || a == self.me);
        let running = self.rt.state == St::ChallengeRunning;
        if let (true, Some(a)) = (local_turn && running, actor) {
            for s in seen {
                let (msg, ev) = match s {
                    Seen::Trigger(v, inside) => (Msg::Trigger { player: a, volume: v, inside }, RelayEv::Trigger(v, inside)),
                    Seen::Begin => (Msg::SequenceBegin(a), RelayEv::Begin),
                    Seen::Sequence(score) => (Msg::Sequence { player: a, score, wiped: false }, RelayEv::Sequence(score)),
                    Seen::Wipeout => (Msg::Wipeout(a), RelayEv::Wipeout),
                    // The local player's own run comes as the ghost's id (wipeoutalmostover 1112-1115).
                    Seen::Almost(t) => (Msg::AlmostOver { player: if a == self.me { GHOST } else { a }, time: t }, RelayEv::Almost(t)),
                    Seen::Over(score) => {
                        self.g.hom_score = score;
                        (Msg::WipeoutOver(a), RelayEv::Over(score))
                    }
                };
                if self.online {
                    out.push(Relay { run: self.g.run, ev });
                }
                skate3_modes::dispatch(&mut self.g, &mut self.rt, msg);
            }
        }
        // Its end, once this console reaches end_turn.
        if self.online && local_turn && self.rt.state == St::EndTurn && self.sent_end != self.g.run {
            self.sent_end = self.g.run;
            out.push(Relay { run: self.g.run, ev: RelayEv::End });
        }
        // The turn's mean vote, once this console stopped it.
        if self.online && local_turn && let Some(score) = self.g.tally.take() {
            out.push(Relay { run: self.g.run, ev: RelayEv::Voted(score) });
        }
        // This player's vote on another's run.
        let voting = self.online && self.g.voting_on && !local_turn;
        if voting {
            if let Some(&(_, level)) = PAD_VOTES.iter().find(|(bit, _)| pressed & bit != 0)
                && level != self.vote
            {
                self.vote = level;
                self.g.vote(self.me, level);
                out.push(Relay { run: self.g.run, ev: RelayEv::Vote(level) });
            }
        } else {
            self.vote = 1;
        }
        self.rt.hud.vote = voting.then_some(self.vote);
        // Relays: another console's turn as it comes, and the votes (an early one waits for its run).
        if self.online {
            self.inbox.extend(received);
            for (from, r) in std::mem::take(&mut self.inbox) {
                if r.run > self.g.run || (r.run == self.g.run && self.rt.state == St::Countdown) {
                    self.inbox.push((from, r));
                    continue;
                }
                if r.run != self.g.run {
                    continue;
                }
                if let RelayEv::Vote(level) = r.ev {
                    // Not retail (user 10-12): a vote reaching this console after it closed the turn's vote is
                    // dropped; it stayed in `votes` (StartVoting resets only an open vote, as the oracle-checked
                    // VotingManager) and counted for the next player's run (matrix 10-12).
                    if self.g.voting_on {
                        self.g.vote(from, level);
                    }
                    continue;
                }
                if let RelayEv::Voted(score) = r.ev {
                    // Only the turn's console relays its mean: a peer can't pick the Players' Choice.
                    if !local_turn && actor == Some(from) {
                        self.g.remote_voted(&mut self.rt, Some(score));
                        // 10-26: its Voted now, not at the next frame's advance: a leave given next frame (the mean's
                        // copy and the last mark come in one relay) came first there and dropped the leaver's mean on
                        // those consoles only.
                        skate3_modes::deliver(&mut self.g, &mut self.rt);
                    }
                    continue;
                }
                // Only the turn's own console speaks for its run: no spoofed scores or ends.
                let Some(a) = actor.filter(|&a| !local_turn && a == from && self.rt.state == St::ChallengeRunning) else { continue };
                let msg = match r.ev {
                    RelayEv::Trigger(v, inside) => Msg::Trigger { player: a, volume: v, inside },
                    RelayEv::Begin => Msg::SequenceBegin(a),
                    RelayEv::Sequence(score) => Msg::Sequence { player: a, score, wiped: false },
                    RelayEv::Wipeout => Msg::Wipeout(a),
                    RelayEv::Almost(t) => Msg::AlmostOver { player: a, time: t },
                    RelayEv::Over(score) => {
                        self.g.hom_score = score;
                        Msg::WipeoutOver(a)
                    }
                    RelayEv::End => {
                        self.g.remote_end(&mut self.rt);
                        skate3_modes::settle(&mut self.g, &mut self.rt);
                        continue;
                    }
                    RelayEv::Vote(_) | RelayEv::Voted(_) => continue,
                };
                skate3_modes::dispatch(&mut self.g, &mut self.rt, msg);
            }
        }
        out
    }
}

fn send(world: &mut World, relay: Relay) {
    let text = serde_json::to_string(&relay).expect("plain data");
    world.resource_mut::<Match>().outbox.push(text);
}

fn begin(world: &mut World, data: &ModeData, id: &str, row: Row, now: f64, label: &'static str) -> Play {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (crate::party_play::names(world), 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let m = world.resource::<Match>();
    // ponytail: Random.GetRandomNumber's generator is not recovered; every console draws from the match token
    // (Party Play from the clock), as skate3_skate_game's first leader.
    let first = if online { m.token } else { u64::from((now as f32).to_bits()) } as usize;
    let intro = row.intro.as_ref().and_then(|s| data.intros.get(s)).copied().unwrap_or(0.);
    info!("Skate 3 {label}: {id} ({}), {} players, this one {me}", if online { "online" } else { "Party Play" }, names.len());
    let mut g = Uniform::new(row.clone(), f64::from(intro), names.len(), me, first);
    g.offline = !online;
    // A team lobby's teams (match_core::Match::teams, the host's assignment); Teams.GetName as 1-Up's
    // (online_moves::TEAM_NAMES).
    if online && m.teams.len() == names.len() {
        g.teams_in = m.teams.clone();
    }
    g.team_names = crate::online_moves::TEAM_NAMES.map(|id| data.text(id));
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.online = online;
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    skate3_modes::start(&mut g, &mut rt);
    let n = names.len();
    // G7: an export from before G7 has no looks: nothing is drawn.
    let markers =
        if data.root.join("indicators.json").is_file() { crate::skate3_indicators::looks(&row.visual_indicators) } else { Vec::new() };
    let ribbons = row.outlines.iter().map(|o| o.ribbon(outline_dir(label))).collect();
    Play {
        id: id.to_owned(),
        row,
        g,
        rt,
        names,
        me,
        online,
        hud: HudDriver::default(),
        inbox: Vec::new(),
        placed: vec![0; n],
        intro: None,
        closed: false,
        sent_end: 0,
        vote: 1,
        label,
        markers,
        ribbons,
    }
}

fn end(world: &mut World, game: &mut UniformGame) {
    if let Some(play) = game.play.take() {
        info!("Skate 3 {}: {} ended", play.label, play.id);
        crate::skate3_indicators::show(world, indicators(play.label), Vec::new());
        if play.intro.is_some_and(f64::is_finite) {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
    }
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

    fn row(spots: usize, rounds: i64, seqs: i64, cumulative: bool, time: f64) -> Row {
        let loc = || Some(Locator { matrix: [[0.; 4]; 4], slots: vec![] });
        Row {
            title: "ID_T".into(),
            online: true,
            intro: None,
            objectives: 0,
            challenge_type: Some(SPOT_BATTLE),
            time_limit: time,
            num_rounds: rounds,
            num_sequences_to_end_turn: seqs,
            cumulative_scoring: cumulative.then_some(true),
            countdown_delay: 1.5,
            turn_based_attempt_spawn_point: loc(),
            turn_based_waiting_location: loc(),
            spot_volumes: (0..spots).map(|_| Some(Volume { min: [0.; 3], max: [1.; 3] })).collect(),
            your_turn: "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN".into(),
            complete: "ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE".into(),
            spot_owned: "ID_CHALLENGE_CENTRAL_MESSAGE_SPOTOWNED".into(),
            out_of_time: "ID_CHALLENGE_CENTRALMESSAGE_RANOUTOFTIME".into(),
            your_turn_team: "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN_TEAM".into(),
            team_intro_delay: 3.,
            intro_nis: Some(serde_json::json!({"scene": "dom_intro"})),
            visual_indicators: Vec::new(),
            outlines: Vec::new(),
        }
    }
    /// A HOM Battle row: no spots, scored by the bail.
    fn hom_row(rounds: i64, time: f64) -> Row {
        Row { challenge_type: Some(HOM_BATTLE), ..row(0, rounds, 0, false, time) }
    }

    /// Plans: per run spot entries and exits, sequences in and out of the spots, sometimes a bail (its reset countdown,
    /// its end with the bail's Hall of Meat score; with no spots (HOM Battle) mostly a bail), the others' votes, rarely a
    /// player leaving.
    fn plans(seed: u64, n: usize, me: usize, spots: usize, leaves: bool) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |k: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % k
        };
        (0..200)
            .map(|_| {
                let mut run = Vec::new();
                let mut t = 0.21;
                for _ in 0..next(5) {
                    t += 0.33 + next(40) as f64 / 10.;
                    let v = next(spots.max(1) as u64) as i64;
                    let inside = spots > 0 && next(4) != 0;
                    if inside {
                        run.push((t, Ev::Other("enter", v, 0.)));
                    }
                    run.push((t + 0.13, Ev::Other("begin", 0, 0.)));
                    if next(3) == 0 {
                        run.push((t + 0.4, Ev::Other("exit", v, 0.)));
                    }
                    run.push((t + 0.9 + next(20) as f64 / 10., Ev::Other("seq", 100 * next(40) as i64 + 50, 0.)));
                    if inside {
                        run.push((t + 3.1, Ev::Other("exit", v, 0.)));
                    }
                    t += 3.5;
                }
                if (spots == 0 && next(5) != 0) || next(4) == 0 {
                    let b = 0.57 + next(200) as f64 / 10.;
                    run.push((b, Ev::Other("bail", 0, 0.)));
                    run.push((b + 1.3, Ev::Other("almost", 0, 0.5 + next(3) as f64 * 0.25)));
                    run.push((b + 2.1, Ev::Other("over", 100 * next(80) as i64, 0.)));
                }
                for _ in 0..next(4) {
                    run.push((0.3 + next(250) as f64 / 10., Ev::Other("vote", next(n as u64) as i64, next(4) as f64)));
                }
                if leaves && next(20) == 0 {
                    let p = next(n as u64) as usize;
                    if p != me {
                        run.push((0.19 + next(300) as f64 / 10., Ev::Leave(p)));
                    }
                }
                run
            })
            .collect()
    }

    const SETUP: &str = r#"
SELF = setmetatable({Name = "battle", Title = "ID_T", ChallengeType = CTYPE, TimeLimit = TIME, NumRounds = ROUNDS,
  NumSequencesToEndTurn = SEQS, CumulativeScoring = CUMUL, CountdownDelay = 1.5, TeamIntroductionDelay = 3,
  TurnBasedAttemptSpawnPoint = "TurnBasedAttemptSpawnPoint0", TurnBasedWaitingLocation = "TurnBasedWaitingLocation0",
  SpotVolumes = {Count = SPOTS}, VisualIndicators = {Count = 0}, Objectives = {Count = 0},
  CentralMessageHALIDYourTurn = "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN",
  CentralMessageHALIDChallengeComplete = "ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE",
  CentralMessageHALIDSpotOwned = "ID_CHALLENGE_CENTRAL_MESSAGE_SPOTOWNED",
  CentralMessageHALIDOutOfTime = "ID_CHALLENGE_CENTRALMESSAGE_RANOUTOFTIME",
  CentralMessageHALIDYourTurnTeam = "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN_TEAM", IntroNIS = "dom_intro"},
  {__index = function(_, k) return k end})
HUD2.LeaderboardHUDSetIgnoreStatus = function(_, p, b) log_line(string.format("ignore %d %s", p, tostring(b))) end
Results.SetWinningTeam = function(t) log_line("winningteam " .. t) end
FrontEnd.Results_SetPlayerScoreIgnored = function(p, b) log_line(string.format("resultignored %d %s", p, tostring(b))) end
FrontEnd.Results_SetSecondLBPlayerScoreIgnored = function(p, b) log_line(string.format("resultignored2 %d %s", p, tostring(b))) end
for i = 1, SPOTS do SELF.SpotVolumes[i] = "SpotVolumes" .. i end
Challenge.eChallengeTypes = {SpotBattle = 53, HallOfMeatBattle = 33}
Trick.GetHOMScore = function() return SCORE end
Challenge.GetChallengeType = function(s) return s.ChallengeType end
Challenge.GetPlayerName = function(i) return "P" .. i end
ChallengeManager.GetSkaterCount = function() return N end
Online.IsOnline = function() return true end
Online.GetGhostID = function() return 6 end
GameMode.GetMinPlayersForVoting = function() return 3 end
GameMode.SpotBattleHOMRoundOverride = function() return false end
function ACTOR() if VARS.ActivePlayerID then return VARS.ActivePlayerID - 1 end return nil end
function DELIVER(kind, v, w)
  if kind == "vote" then VOTES[v] = w; return true end
  local a = ACTOR()
  if kind == "seq" then if a then SEND("sequencecomplete", {PlayerID = a, SequenceScore = v, Wipedout = false}) end
  elseif kind == "begin" then if a then SEND("sequencebegin", {PlayerID = a}) end
  elseif kind == "enter" then if a then SEND("triggerentercollision", {PlayerID = a, VolumeID = SELF.SpotVolumes[v + 1]}) end
  elseif kind == "exit" then if a then SEND("triggerexitcollision", {PlayerID = a, VolumeID = SELF.SpotVolumes[v + 1]}) end
  elseif kind == "bail" then if a then SEND("wipeout", {PlayerID = a, IsAPhysicsTeleport = false}) end
  elseif kind == "almost" then
    if a then local p = a; if a == ME then p = 6 end; SEND("wipeoutalmostover", {PlayerID = p, TimeToTeleport = w}) end
  elseif kind == "over" then if a then SCORE = v; SEND("wipeoutover", {PlayerID = a}) end
  else return false end
  return true
end
"#;

    fn oracle(src: &str, n: usize, me: usize, first: usize, r: &Row, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        team_oracle(src, n, me, first, r, &[], plans)
    }
    /// `teams`: each player's team (TEAMS: Online.IsTeamChallenge, GetTeam), empty for none.
    fn team_oracle(
        src: &str,
        n: usize,
        me: usize,
        first: usize,
        r: &Row,
        teams: &[u8],
        plans: &[Vec<(f64, Ev)>],
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let lua_teams = if teams.is_empty() {
            String::new()
        } else {
            let t: Vec<String> = teams.iter().enumerate().map(|(i, t)| format!("[{i}] = {t}")).collect();
            format!("TEAMS = {{{}}}\n", t.join(", "))
        };
        let setup = format!(
            "N, ME, FIRST, SPOTS, TIME, ROUNDS, SEQS, CUMUL, CTYPE = {n}, {me}, {first}, {}, {}, {}, {}, {}, {}\n{lua_teams}{SETUP}",
            r.spot_volumes.len(),
            r.time_limit,
            r.num_rounds,
            r.num_sequences_to_end_turn,
            r.cumulative_scoring == Some(true),
            r.challenge_type.unwrap_or(0)
        );
        let (lua, errors) = lua_log(src, &setup, "initial", "challenge_running", plans);
        let mut g = Uniform::new(r.clone(), 0., n, me, first);
        g.teams_in = teams.to_vec();
        let mut rt = Rt::new(St::Initial, n, 0.);
        let other = |g: &mut Uniform, _: &mut Rt<St>, kind: &str, v: i64, w: f64| {
            if kind == "vote" {
                g.vote(v as usize, w as u8);
                return None;
            }
            let a = g.actor()?;
            Some(match kind {
                "seq" => Msg::Sequence { player: a, score: v, wiped: false },
                "begin" => Msg::SequenceBegin(a),
                "enter" => Msg::Trigger { player: a, volume: v, inside: true },
                "exit" => Msg::Trigger { player: a, volume: v, inside: false },
                "bail" => Msg::Wipeout(a),
                "almost" => Msg::AlmostOver { player: if a == me { GHOST } else { a }, time: w },
                "over" => {
                    g.hom_score = v;
                    Msg::WipeoutOver(a)
                }
                _ => return None,
            })
        };
        let model = model_log(&mut g, &mut rt, St::ChallengeRunning, plans, Uniform::actor, |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// spot_battle.lua itself (S3_SPOT_DIR = tools/skate3_online.py's `--out`/spot_battle, with scripts/) decides
    /// every game the model plays the same way: every state, timer, fade, move, pin, camera, vote, HUD2 call and
    /// result; 2 to 6 players (voting from 3), 1 to 3 spots and rounds, best-sequence and cumulative rows, varied
    /// first and local players, bails, players leaving mid-game.
    #[test]
    #[ignore = "requires the decompiled spot_battle.lua (S3_SPOT_DIR)"]
    fn skate3_spot_battle_matches_spot_battle_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_SPOT_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/spot_battle.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let s = seed as usize;
                let (me, first) = (s % n, (s * 7) % 11);
                let r = row(1 + s % 3, 1 + (s / 3 % 3) as i64, 1 + (s / 2 % 3) as i64, s % 4 == 1, 15. + (s % 3) as f64 * 5.);
                let plans = plans(seed * 131 + n as u64, n, me, r.spot_volumes.len(), seed % 3 == 0);
                let (lua, model, errs) = oracle(&src, n, me, first, &r, &plans);
                compare(&format!("seed {seed} n {n} me {me} first {first}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("result")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("spot_battle.lua oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// hall_of_meat.lua (S3_HOM_DIR = the tool's `--out`/hall_of_meat, with scripts/) against the same model as a HOM
    /// Battle: bails scored at wipeoutover, the time-up message, the votes; 2 to 6 players, 1 to 3 rounds.
    #[test]
    #[ignore = "requires the decompiled hall_of_meat.lua (S3_HOM_DIR)"]
    fn skate3_hom_battle_matches_hall_of_meat_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_HOM_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/hall_of_meat.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let s = seed as usize;
                let (me, first) = (s % n, (s * 7) % 11);
                let r = hom_row(1 + (s / 3 % 3) as i64, 15. + (s % 2) as f64 * 5.);
                let plans = plans(seed * 137 + n as u64, n, me, 0, seed % 3 == 0);
                let (lua, model, errs) = oracle(&src, n, me, first, &r, &plans);
                compare(&format!("seed {seed} n {n} me {me} first {first}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("result")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("hall_of_meat.lua oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// A team split for the team oracles: alternating, or (every third seed) uneven, the second team one player.
    fn split(s: usize, n: usize) -> Vec<u8> {
        (0..n).map(|i| if s % 3 == 0 { if i == s % n { 2 } else { 1 } } else if (i + s) % 2 == 0 { 1 } else { 2 }).collect()
    }

    /// The TeamsEnabled path of both battles against their Lua: no voting, the team DPO and colour, the team turn
    /// messages and TeamIntroductionDelay, the team intro NIS, UpdateTeamScores (the smaller team's count, ignored
    /// scores, ties), the winning team and its 3 s, the ignored flags on the results; 2 to 6 players, even and uneven
    /// teams, players leaving.
    #[test]
    #[ignore = "requires the decompiled spot_battle.lua and hall_of_meat.lua (S3_SPOT_DIR, S3_HOM_DIR)"]
    fn skate3_team_battles_match_the_lua() {
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for (var, hom) in [("S3_SPOT_DIR", false), ("S3_HOM_DIR", true)] {
            let root = std::path::PathBuf::from(std::env::var(var).unwrap());
            let file = if hom { "scripts/hall_of_meat.lua" } else { "scripts/spot_battle.lua" };
            let src = std::fs::read_to_string(root.join(file)).unwrap();
            for seed in 1..=40u64 {
                for n in 2..=6usize {
                    let s = seed as usize;
                    let (me, first) = (s % n, (s * 7) % 11);
                    let r = if hom {
                        hom_row(1 + (s / 3 % 3) as i64, 15. + (s % 2) as f64 * 5.)
                    } else {
                        row(1 + s % 3, 1 + (s / 3 % 3) as i64, 1 + (s / 2 % 3) as i64, s % 4 == 1, 15. + (s % 3) as f64 * 5.)
                    };
                    let teams = split(s, n);
                    let plans = plans(seed * 139 + n as u64, n, me, r.spot_volumes.len(), seed % 3 != 1);
                    let (lua, model, errs) = team_oracle(&src, n, me, first, &r, &teams, &plans);
                    compare(&format!("{var} seed {seed} n {n} me {me} first {first} teams {teams:?}"), &lua, &model, &errs);
                    games += 1;
                    lines += lua.len();
                    errors += errs.len();
                    assert!(lua.iter().any(|l| l.contains("winningteam")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
                }
            }
        }
        eprintln!("team battles oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 400);
    }

    /// HOM Battle by hand: a bail's score counts once it is over, a second bail in the turn is not a new wipeout, time
    /// up says so.
    #[test]
    fn hom_battle_scores_the_bail() {
        let mut g = Uniform::new(hom_row(1, 15.), 0., 2, 0, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::ChallengeRunning);
        assert!(!g.enable_voting, "2 players: no vote");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: 900, wiped: false });
        assert_eq!(g.players[0].score, 0, "no sequence scoring");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Wipeout(0));
        assert!(g.wiped && !rt.pinned[0], "HOM's wipeout pins nobody");
        g.hom_score = 4200;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::WipeoutOver(0));
        assert_eq!((g.players[0].score, rt.state), (4200, St::EndTurn));
        until(&mut g, &mut rt, St::ChallengeRunning);
        until(&mut g, &mut rt, St::Complete);
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("show TrickDisplayHUD false")), "{log:?}");
        assert!(log.iter().any(|l| l.contains("central ID_CHALLENGE_CENTRALMESSAGE_RANOUTOFTIME")), "player 2's time ran out");
        assert!(log.iter().any(|l| l.ends_with("result 0 1 4200")));
    }

    /// Plays a Party Play game to its results dialog: each turn's card answered (A) once it is up, each run one sequence
    /// in the spot. Returns the cards (player 1-based, round) in order.
    /// G7: the spot markers are up from the turn's start until the active skater's bail or the turn's last sequence,
    /// and back on the next turn.
    #[test]
    fn the_spot_markers_show_while_a_turn_runs() {
        let mut g = Uniform::new(row(1, 1, 1, false, 20.), 0., 2, 0, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        assert!(!g.indicators, "none before the first turn");
        until(&mut g, &mut rt, St::ChallengeRunning);
        assert_eq!(g.actor(), Some(0));
        assert!(g.indicators, "the turn puts them up");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Wipeout(0));
        assert!(!g.indicators, "the active skater's bail takes them down");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::WipeoutOver(0));
        until(&mut g, &mut rt, St::ChallengeRunning);
        assert_eq!(g.actor(), Some(1));
        assert!(g.indicators, "the next turn puts them up again");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: 0, inside: true });
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 1, score: 700, wiped: false });
        assert!(!g.indicators, "the turn's last sequence takes them down");
    }

    fn party_game(g: &mut Uniform, rt: &mut Rt<St>) -> Vec<(usize, i64)> {
        let mut cards = Vec::new();
        let mut scored = None;
        for _ in 0..2000 {
            if rt.dialog {
                return cards;
            }
            if let Some(card) = rt.card.take() {
                assert_eq!(rt.controlled + 1, card.0, "the pad is the carded player's");
                assert_ne!(rt.state, St::ChallengeRunning, "the turn waits on the card");
                assert!(!rt.pause_disabled, "Start opens the Party Play crossbar on every turn (1501)");
                cards.push(card);
                skate3_modes::dispatch(g, rt, Msg::HudComplete);
                continue;
            }
            if rt.state == St::ChallengeRunning && let Some(a) = g.actor().filter(|&a| scored != Some((cards.len(), a))) {
                scored = Some((cards.len(), a));
                skate3_modes::dispatch(g, rt, Msg::Trigger { player: a, volume: 0, inside: true });
                skate3_modes::dispatch(g, rt, Msg::Sequence { player: a, score: 500 * (a as i64 + 1), wiped: false });
                continue;
            }
            let log = rt.log.as_deref().unwrap_or_default();
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}: {:?}", rt.state, &log[log.len().saturating_sub(6)..]));
            skate3_modes::advance(g, rt, t);
        }
        panic!("no results dialog");
    }

    /// Matrix Step 2 (10-12): Party Play at its retail max (4, party_play page 0), two rounds, from every first player.
    /// By hand (spot_battle.lua 960-980): each run's one sequence 500 (p + 1), the best kept: ranks 4, 3, 2, 1 by
    /// score; every turn carded to its player, eight in all.
    #[test]
    fn party_play_holds_four_players() {
        for first in 0..4 {
            let mut g = Uniform::new(row(1, 2, 1, false, 20.), 0., 4, 0, first);
            g.offline = true;
            let mut rt = Rt::new(St::Initial, 4, 0.);
            rt.online = false;
            rt.log = Some(Vec::new());
            skate3_modes::start(&mut g, &mut rt);
            let cards = party_game(&mut g, &mut rt);
            assert_eq!(cards.len(), 8, "first {first}: one card per turn: {cards:?}");
            for round in 1..=2 {
                let mut carded: Vec<usize> = cards.iter().filter(|c| c.1 == round).map(|c| c.0).collect();
                carded.sort_unstable();
                assert_eq!(carded, [1, 2, 3, 4], "first {first}: round {round}'s cards {cards:?}");
            }
            let results: Vec<(i64, i64)> = g.players.iter().map(|p| (p.rank, p.score)).collect();
            assert_eq!(results, [(4, 500), (3, 1000), (2, 1500), (1, 2000)], "first {first}");
        }
    }

    /// Party Play by hand (IsOnline false): no vote; every turn hands the pad to its player behind the Press-A card;
    /// the results dialog's Rematch (hudretry) plays it again from no score, Restart Challenge too, and Quit
    /// (hudcontinue) ends it.
    #[test]
    fn party_play_cards_every_turn_and_the_dialog_rematches_or_quits() {
        let mut g = Uniform::new(row(1, 1, 1, false, 20.), 0., 3, 0, 0);
        g.offline = true;
        let mut rt = Rt::new(St::Initial, 3, 0.);
        rt.online = false;
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        let cards = party_game(&mut g, &mut rt);
        assert!(!g.enable_voting, "no vote offline");
        assert_eq!(cards.len(), 3, "one card per turn: {cards:?}");
        assert!(cards.iter().all(|c| c.1 == 1), "round 1: {cards:?}");
        assert_eq!(g.players.iter().map(|p| p.score).sum::<i64>(), 500 + 1000 + 1500);
        assert_eq!(rt.state, St::Leaderboard, "the dialog holds the leaderboard");

        // Rematch: the same players again, from nothing.
        skate3_modes::dispatch(&mut g, &mut rt, Msg::HudRetry);
        assert!(!rt.dialog);
        let mut seen_reset = false;
        for _ in 0..50 {
            if rt.card.is_some() {
                break;
            }
            seen_reset |= g.players.iter().all(|p| p.score == 0);
            let t = rt.due().expect("rematch stuck");
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert!(seen_reset && rt.card.is_some(), "a fresh game, its first card up");
        // Restart Challenge mid-game does the same.
        rt.card = None;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Restart);
        assert_eq!(party_game(&mut g, &mut rt).len(), 3);

        // Quit.
        skate3_modes::dispatch(&mut g, &mut rt, Msg::HudContinue);
        until(&mut g, &mut rt, St::Complete);
        assert!(rt.left);
    }

    fn until(g: &mut Uniform, rt: &mut Rt<St>, state: St) {
        for _ in 0..1000 {
            if rt.state == state {
                return;
            }
            let log = rt.log.as_deref().unwrap_or_default();
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}: {:?}", rt.state, &log[log.len().saturating_sub(6)..]));
            skate3_modes::advance(g, rt, t);
        }
        panic!("never reached {state:?}");
    }

    /// The rules by hand: a sequence scores only after a spot was touched, the best one ranks, NumSequencesToEndTurn
    /// ends the turn, the turns go round, the others vote and the best mean over 1 is the Players' Choice.
    #[test]
    fn turns_score_in_the_spots_and_the_votes_pick_the_choice() {
        let mut g = Uniform::new(row(1, 1, 1, false, 20.), 0., 3, 0, 0);
        let mut rt = Rt::new(St::Initial, 3, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::ChallengeRunning);
        assert_eq!(g.actor(), Some(0), "the drawn first player (first 0: RandomStartIndex 1)");
        assert_eq!(rt.places[0].map(|p| p.1), Some(Place { field: ATTEMPT, index: 0, slot: 0 }));
        assert_eq!(rt.places[2].map(|p| p.1), Some(Place { field: WAITING, index: 0, slot: 2 }));
        assert_eq!((rt.watching, rt.pinned[0]), (0, false), "the local player is up and free");
        assert!(g.enable_voting, "3 players vote");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: 900, wiped: false });
        assert_eq!(g.players[0].score, 0, "no spot touched: no score");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: 0, inside: true });
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: 700, wiped: false });
        assert_eq!(g.players[0].score, 700);
        g.vote(1, 3);
        g.vote(2, 2);
        until(&mut g, &mut rt, St::EndTurn);
        until(&mut g, &mut rt, St::ChallengeRunning);
        assert_eq!(g.actor(), Some(1));
        assert_eq!(g.highest_vote, (2.5, 1), "the others' mean: (3 + 2) / 2");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: 0, inside: true });
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 1, score: 1500, wiped: false });
        assert_eq!((g.players[1].rank, g.players[0].rank), (1, 2), "the better score takes first");
        until(&mut g, &mut rt, St::Complete);
        assert_eq!(g.choice(), Some(0));
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.contains("central ID_CHALLENGE_CENTRAL_MESSAGE_SPOTOWNED")), "{log:?}");
        assert!(log.iter().any(|l| l.ends_with("result 1 1 1500")));
    }

    /// A relayed i64::MAX twice in a cumulative turn saturates the turn's score (no overflow panic).
    #[test]
    fn huge_relayed_scores_saturate() {
        let mut g = Uniform::new(row(1, 1, 3, true, 20.), 0., 2, 0, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::ChallengeRunning);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: 0, inside: true });
        for _ in 0..2 {
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: i64::MAX, wiped: false });
        }
        assert_eq!(g.players[0].turn_score, i64::MAX);
    }

    /// Matrix Step 2 (10-12): `n` consoles play a two-round Spot Battle (one sequence ends a turn) or, with `hom`, HOM
    /// Battle (the bail ends it) through the match relay ([`skate3_modes::tests::Wire`]), started up to 0.3 s apart,
    /// each through [`Play::exchange`] as the glue runs it. The turn's player enters spot 0 (bails) 1 s into its run
    /// and lands 1000 (p + 1) (its bail is over, scored so) at 1.5 s in round 1, 100 (6 - p) in round 2. Votes: console 1 votes 2 (sweet) on player 3's runs 0.5 s in; with `late`, console 0 votes 3 on
    /// player 4's runs 0.05 s before its own vote closes (END_TURN after its end_turn). `leaver` (player, own) quits
    /// once its console is in round 2 and another player's run, or (own) its own run once in the spot (each other
    /// console notices 0.5 s + 0.4 s per slot later, and gets it once the rest have sent on its events: match_core
    /// Leaves); with `voted`, 0.2 s after its console relayed its round-2 turn's mean (`RelayEv::Voted`: on the way to
    /// some consoles, not others), every other console voting 3 (gnarly) on that run 0.5 s in. Every staying console's
    /// (rank, score) per player and Players' Choice.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn spot_battle_over_the_wire(
        n: usize,
        seed: u64,
        latency: (f64, f64),
        loss: f64,
        hom: bool,
        late: bool,
        leaver: Option<(usize, bool)>,
        voted: bool,
        teams: &[u8],
    ) -> Vec<(Vec<(i64, i64)>, Option<usize>, Vec<(i64, i64)>, Vec<bool>)> {
        let r = if hom { hom_row(2, 20.) } else { row(1, 2, 1, false, 20.) };
        let mut wire = skate3_modes::tests::Wire::new(seed, latency, loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        // Per console: (run, when it began here, what was done), (run, when end_turn began here).
        let mut runs: Vec<(u32, f64, u8)> = vec![(0, 0., 0); n];
        let mut ends: Vec<(u32, f64)> = vec![(0, 0.); n];
        let mut left_at: Option<f64> = None;
        // When the leaver's console relayed its round-2 mean.
        let mut voted_at: Option<f64> = None;
        // When each console reached each sync point (match_core's @score carries it 0.25 s later).
        let mut reached: Vec<Vec<f64>> = vec![Vec::new(); n];
        let bit = |level: u8| PAD_VOTES.iter().find(|v| v.1 == level).unwrap().0;
        for frame in 0..60 * 600 {
            let t = frame as f64 / 60.;
            for i in 0..n {
                let quit = leaver.is_some_and(|l| l.0 == i);
                if t < starts[i] || (quit && left_at.is_some()) {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| {
                    let mut g = Uniform::new(r.clone(), 0., n, i, seed as usize);
                    g.teams_in = teams.to_vec();
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    skate3_modes::start(&mut g, &mut rt);
                    let names = (0..n).map(|p| format!("P{p}")).collect();
                    Play { id: "s".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), inbox: Vec::new(), placed: vec![0; n], intro: None, closed: false, sent_end: 0, vote: 1, label: if hom { "HOM Battle" } else { "Spot Battle" }, markers: Vec::new(), ribbons: Vec::new() }
                });
                let own = play.g.actor() == Some(i) && runs[i].0 == play.g.run && runs[i].2 == 1;
                let others = play.g.actor().is_some_and(|a| a != i);
                let quits = quit
                    && if voted {
                        voted_at.is_some_and(|at| t >= at + 0.2)
                    } else {
                        play.g.current_round >= 2 && play.rt.state == St::ChallengeRunning && (if leaver.unwrap().1 { own } else { others })
                    };
                if quits {
                    left_at = Some(t);
                    (0..n).for_each(|to| wire.cut(i, to));
                    continue;
                }
                if let (Some((p, _)), Some(left)) = (leaver, left_at)
                    && t >= left + 0.5 + 0.4 * i as f64
                    && !play.rt.gone[p]
                    && wire.left(t, i, p, n)
                {
                    play.rt.gone[p] = true;
                    skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
                }
                // The turn sync points (OnlineChallengeRoundSyncPoint), as match_core passes them.
                let syncs: Vec<u32> = (0..n)
                    .map(|j| match j {
                        _ if j == i => play.rt.syncs,
                        _ if play.rt.gone[j] => u32::MAX,
                        _ => reached[j].iter().filter(|&&at| at + 0.25 <= t).count() as u32,
                    })
                    .collect();
                play.rt.release(&syncs);
                play.g.remote = play.online && play.g.actor().is_some_and(|a| a != i);
                skate3_modes::advance(&mut play.g, &mut play.rt, t);
                play.g.remote = play.online && play.g.actor().is_some_and(|a| a != i);
                let (mut seen, mut pressed) = (Vec::new(), 0);
                let actor = play.g.actor();
                if play.rt.state == St::ChallengeRunning && runs[i].0 != play.g.run {
                    runs[i] = (play.g.run, t, 0);
                }
                if play.rt.state == St::EndTurn && ends[i].0 != play.g.run {
                    ends[i] = (play.g.run, t);
                }
                let since = t - runs[i].1;
                if play.rt.state == St::ChallengeRunning && actor == Some(i) {
                    let score = if play.g.current_round == 1 { 1000 * (i as i64 + 1) } else { 100 * (6 - i as i64) };
                    if runs[i].2 == 0 && since >= 1. {
                        (seen, runs[i].2) = (vec![if hom { Seen::Wipeout } else { Seen::Trigger(0, true) }], 1);
                    } else if runs[i].2 == 1 && since >= 1.5 {
                        (seen, runs[i].2) = (vec![if hom { Seen::Over(score) } else { Seen::Sequence(score) }], 2);
                    }
                } else if play.rt.state == St::ChallengeRunning && (i, actor) == (1, Some(3)) && runs[i].2 == 0 && since >= 0.5 {
                    (pressed, runs[i].2) = (bit(2), 1);
                } else if late && play.rt.state == St::EndTurn && (i, actor) == (0, Some(4)) && runs[i].2 == 0 && t - ends[i].1 >= END_TURN - 0.05 {
                    (pressed, runs[i].2) = (bit(3), 1);
                } else if voted && play.rt.state == St::ChallengeRunning && play.g.current_round >= 2 && actor == leaver.map(|l| l.0) && runs[i].2 == 0 && since >= 0.5 {
                    (pressed, runs[i].2) = (bit(3), 1);
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                let out = play.exchange(seen, pressed, received);
                if quit && play.g.current_round >= 2 && out.iter().any(|r| matches!(r.ev, RelayEv::Voted(_))) {
                    voted_at.get_or_insert(t);
                }
                while reached[i].len() < play.rt.syncs as usize {
                    reached[i].push(t);
                }
                wire.send(t, i, n, out.iter().map(|r| serde_json::to_string(r).unwrap()).collect());
            }
            let here = |i: &usize| left_at.is_none() || leaver.is_none_or(|l| l.0 != *i);
            if (0..n).filter(here).all(|i| plays[i].as_ref().is_some_and(|p| p.rt.state == St::Complete)) {
                break;
            }
        }
        (0..n)
            .filter(|&i| leaver.is_none_or(|l| l.0 != i))
            .map(|i| {
                let p = plays[i].as_ref().unwrap();
                assert_eq!(p.rt.state, St::Complete, "seed {seed}: console {i} finished ({:?}, run {})", p.rt.state, p.g.run);
                let team_data = p.g.team_data.iter().map(|t| (t.rank, t.score)).collect();
                let ignored = p.g.players.iter().map(|p| p.score_ignored).collect();
                (p.g.players.iter().map(|p| (p.rank, p.score)).collect(), p.g.choice(), team_data, ignored)
            })
            .collect()
    }

    /// Lead 10-12 (Team Spot Battle / Team HOM Battle): the Team tab's rows play (match_core::team entries), as the
    /// Versus tab's do.
    #[test]
    fn team_rows_are_playable() {
        let ids: Vec<String> = ["ots_dwtn_01_ol", "hom_dwtn_01"].map(String::from).to_vec();
        let playable = Playable(entries(&ids).into_iter().collect());
        for id in &ids {
            assert_eq!(crate::match_core::unavailable(crate::match_core::TEAM_TAB, id, &playable), None, "{id}: Team tab");
            assert_eq!(crate::match_core::unavailable(3, id, &playable), None, "{id}: Versus tab");
        }
    }

    /// Matrix G2 (10-12): six players, 150-300 ms, 5-10 % loss. By hand (spot_battle.lua 960-980, 566-577, the
    /// VotingManager mean of the connected others' votes, the best over 1 is the Players' Choice, the earlier keeps a
    /// tie): ranks by each player's best (round 1's 1000 (p + 1)), a leaver's score -1 below everyone; player 3's runs
    /// average (2 + 4 x 1) / 5 = 1.2: the Players' Choice. Console 0's late vote reaches player 4's console after that
    /// console (in end_turn a wire delay earlier) stopped the vote: it counts nowhere, every console names player 3
    /// (before the 10-12 fix some consoles counted it: (3 + 4) / 5 = 1.4, player 4).
    /// HOM Battle (turn_based_uniform's same ranking and vote, the bail's score at wipeoutover) the same.
    #[test]
    fn six_players_and_a_leaver_end_spot_battle_with_the_retail_results() {
        for (seed, hom) in (0..12u64).map(|s| (s, false)).chain((0..8u64).map(|s| (s, true))) {
            let late = seed % 2 == 1;
            let own = seed >= 8 || hom && seed >= 4;
            let leaver = match seed % 4 {
                2 => Some((0, own)),
                3 => Some((5, own)),
                _ => None,
            };
            let all = spot_battle_over_the_wire(6, seed, (0.15, 0.3), 0.05 + 0.01 * (seed % 6) as f64, hom, late, leaver, false, &[]);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} hom {hom}: every console the same (late {late}, leaver {leaver:?})");
            }
            let quit = leaver.map(|l| l.0);
            let mut order: Vec<usize> = (0..6).rev().filter(|&p| Some(p) != quit).collect();
            order.extend(quit);
            let want: Vec<(i64, i64)> = (0..6)
                .map(|p| {
                    let rank = order.iter().position(|&o| o == p).unwrap() as i64 + 1;
                    (rank, if Some(p) == quit { -1 } else { 1000 * (p as i64 + 1) })
                })
                .collect();
            assert_eq!(all[0].0, want, "seed {seed} hom {hom}: the ranks");
            assert_eq!(all[0].1, Some(3), "seed {seed} hom {hom}: the Players' Choice (late {late})");
        }
    }

    /// End review I2 (10-12): the turn's console relays its round-2 mean (Voted) and its player leaves 0.2 s later, the
    /// mean on the way to some consoles only (or none). By hand: everyone else votes 3 on that run, so its mean 3 tops
    /// player 3's 1.2 and the leaver is the Players' Choice where its Voted comes before its leave (not disconnected
    /// yet, the EndTurn Voted handler); where no console took it, the leave's own mean does not count: player 3. Ranks
    /// as G2 (the leaver's -1 last). Before the fix the consoles the mean missed named player 3, the rest the leaver.
    #[test]
    fn a_turns_mean_relayed_as_its_player_leaves_names_one_players_choice() {
        let mut counted = 0;
        for (seed, hom) in (0..6u64).map(|s| (s, false)).chain((0..4u64).map(|s| (s, true))) {
            let quit = if seed % 2 == 0 { 5 } else { 0 };
            let loss = 0.05 + 0.01 * (seed % 6) as f64;
            let all = spot_battle_over_the_wire(6, seed + 60, (0.15, 0.3), loss, hom, false, Some((quit, true)), true, &[]);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} hom {hom}: every console the same (player {quit} left)");
            }
            let mut order: Vec<usize> = (0..6).rev().filter(|&p| p != quit).collect();
            order.push(quit);
            let want: Vec<(i64, i64)> =
                (0..6).map(|p| (order.iter().position(|&o| o == p).unwrap() as i64 + 1, if p == quit { -1 } else { 1000 * (p as i64 + 1) })).collect();
            assert_eq!(all[0].0, want, "seed {seed} hom {hom}: the ranks");
            assert!([Some(quit), Some(3)].contains(&all[0].1), "seed {seed} hom {hom}: the Players' Choice {:?}", all[0].1);
            counted += usize::from(all[0].1 == Some(quit));
        }
        assert!(counted > 0, "no console ever took the mean: nothing tested");
    }

    /// 10-26: the case above at seed 3 once each console's match key draws its latency (the online safety signing). The
    /// mean's copy and the last console's mark came in one relay, so most consoles were given the leave the frame after
    /// they took the mean. Its Voted waited for that frame's advance and the leave, dispatched first, made the leaver
    /// disconnected for it: three consoles named player 3, two the leaver. By hand: every console took the mean 3 before
    /// the leave (the relay's order), so the leaver (3 over player 3's 1.2) is the Players' Choice everywhere.
    #[test]
    fn a_leave_the_frame_after_its_relayed_mean_keeps_one_players_choice() {
        skate3_modes::tests::KEY_DRAWS.with(|k| k.set(true));
        let all = spot_battle_over_the_wire(6, 3 + 60, (0.15, 0.3), 0.08, false, false, Some((0, true)), true, &[]);
        let choices: Vec<_> = all.iter().map(|c| c.1).collect();
        assert_eq!(choices, [Some(0); 5], "consoles 1-5");
    }

    /// Lead 10-12: Team Spot Battle / Team HOM Battle at the full lobby (3 a side) over the wire, 150-300 ms, 5-10 %
    /// loss, start to results to freeskate on every console. By hand (UpdateTeamScores 96-126, challenge_finished
    /// 614-622): every player's round-1 best 1000 (p + 1) counts (3 a side: none ignored), a leaver's nothing; the higher
    /// team total ranks 1, a tie ranks both 2 (and challenge_finished names team 1). No votes with teams.
    /// - [1,2,1,2,1,2], nobody leaves: team 1 1000+3000+5000 = 9000, team 2 2000+4000+6000 = 12000: team 2 wins.
    /// - the same, guest 5 (team 2) leaves in round 2: team 2 2000+4000 = 6000 < 9000: team 1 wins.
    /// - [1,2,2,1,2,1], the host (0, team 1) leaves in round 2: team 1 4000+6000 = 10000, team 2 2000+3000+5000 =
    ///   10000: both rank 2.
    #[test]
    fn three_a_side_team_battles_end_with_the_retail_team_order() {
        for (seed, hom) in (0..6u64).map(|s| (s, false)).chain((0..6u64).map(|s| (s, true))) {
            let (teams, leaver, want): (&[u8], _, [(i64, i64); 2]) = match seed % 3 {
                0 => (&[1, 2, 1, 2, 1, 2], None, [(2, 9000), (1, 12000)]),
                1 => (&[1, 2, 1, 2, 1, 2], Some((5, seed >= 3)), [(1, 9000), (2, 6000)]),
                _ => (&[1, 2, 2, 1, 2, 1], Some((0, seed >= 3)), [(2, 10000), (2, 10000)]),
            };
            let loss = 0.05 + 0.01 * (seed % 6) as f64;
            let all = spot_battle_over_the_wire(6, seed + 40, (0.15, 0.3), loss, hom, false, leaver, false, teams);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} hom {hom}: every console the same (leaver {leaver:?})");
            }
            let quit = leaver.map(|l| l.0);
            let scores: Vec<(i64, i64)> = all[0].0.clone();
            for p in 0..6 {
                assert_eq!(scores[p].1, if Some(p) == quit { -1 } else { 1000 * (p as i64 + 1) }, "seed {seed} hom {hom}: player {p}");
            }
            assert_eq!(all[0].1, None, "seed {seed} hom {hom}: no Players' Choice with teams");
            assert_eq!(all[0].2, want.to_vec(), "seed {seed} hom {hom}: the team (rank, score)");
            assert_eq!(all[0].3, vec![false; 6], "seed {seed} hom {hom}: 3 a side, no score ignored");
        }
    }

    /// The tool's output (S3_SPOT_DIR; its sibling hud/): the online rows load with their spawns, waiting slots, spot
    /// volumes and intros, and the voting HUD runs.
    #[test]
    #[ignore = "requires the Skate 3 Spot Battle data (S3_SPOT_DIR)"]
    fn skate3_spot_battle_data_and_movies() {
        use crate::apt_screen::Screen;
        use serde_json::json;
        let root = std::path::PathBuf::from(std::env::var("S3_SPOT_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id)).cloned().collect();
        // 10-06 data: + the 10 DLC ots_{artg,dark,dwmc,mmcn}_*_ol rows.
        assert_eq!(playable.len(), 42, "{playable:?}");
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(data.intros.contains_key(r.intro.as_deref().unwrap()), "{id}: no intro");
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            assert_eq!(r.turn_based_waiting_location.as_ref().unwrap().slots.len(), 6, "{id}: 6 waiting slots");
            assert_eq!(r.challenge_type, Some(SPOT_BATTLE));
            // The Team tab's (TeamsEnabled) fields: the team turn message, its delay, the team intro NIS.
            assert!(!data.text(&r.your_turn_team).starts_with("ID_"), "{id}: {}", r.your_turn_team);
            assert_eq!(r.team_intro_delay, 3., "{id}");
            assert!(r.intro_nis.is_some(), "{id}: IntroNIS");
            for t in ["ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN"] {
                assert!(!data.text(t).starts_with("ID_"), "{t}");
            }
            assert!(r.num_rounds >= 1 && r.time_limit > 0., "{id}");
        }
        let mut vote = Screen::load(&root.join("../hud/runtime/hudonlinevoting.json"), &[]).unwrap();
        vote.call("SetState", &[json!(1)]).unwrap();
        vote.call("SetState", &[json!(6)]).unwrap();
        for _ in 0..30 {
            vote.advance().unwrap();
        }
    }

    /// The HOM Battle rows (S3_HOM_DIR): all 16 load with their spawns, waiting slots and intros.
    #[test]
    #[ignore = "requires the Skate 3 HOM Battle data (S3_HOM_DIR)"]
    fn skate3_hom_battle_data() {
        let root = std::path::PathBuf::from(std::env::var("S3_HOM_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id)).cloned().collect();
        assert_eq!(playable.len(), 16, "{playable:?}");
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(data.intros.contains_key(r.intro.as_deref().unwrap()), "{id}: no intro");
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            assert!(!data.text(&r.out_of_time).starts_with("ID_"), "{id}");
            assert_eq!(r.turn_based_waiting_location.as_ref().unwrap().slots.len(), 6, "{id}: 6 waiting slots");
            assert_eq!(r.challenge_type, Some(HOM_BATTLE));
            // The Team tab's (TeamsEnabled) fields: the team turn message, its delay, the team intro NIS.
            assert!(!data.text(&r.your_turn_team).starts_with("ID_"), "{id}: {}", r.your_turn_team);
            assert_eq!(r.team_intro_delay, 3., "{id}");
            assert!(r.intro_nis.is_some(), "{id}: IntroNIS");
            for t in ["ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS", "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN"] {
                assert!(!data.text(t).starts_with("ID_"), "{t}");
            }
        }
    }
}
