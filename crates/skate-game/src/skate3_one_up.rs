//! Skate 3's 1-Up (challenge graph `1up` < turn_based_leader < turn_based < default) on the match core, for the online
//! rows of DownTown, Industrial and University. Data: tools/skate3_online.py, installed at
//! `private/skate3-online/1up/`: rules.json (per row its CLD fields: TimeLimit, CumulativeScoring, AllowPlayerBail, the
//! Start / Wait location rotations with their slots, the message ids), strings.json, scripts/1up.lua (the decompiled
//! graph: the rules' source and the tests' oracle), the intro cameras (tutorial.json + scene/camera) and the shared HUD
//! movies (skate3_modes).
//!
//! [`OneUp`] is 1up.lua's online (non-team) path, handler by handler (line numbers below are that file's): every
//! round each player still in it takes a run from the round's start locator (StartLocationRotation[round], slot 0)
//! while the others wait at WaitLocationRotation[round], slot = player id (919-946); the first run sets the score, each
//! later one must beat it (1-Up) or its player takes a letter and sits the round out (1237-1386); the round goes on
//! until one player is left in it, who starts the next (1396-1424); three letters eliminate (1332-1347); the last
//! player standing wins (1464-1491). Scores are the sum of the run's completed sequences (CumulativeScoring,
//! objective `1up_sequence_anywhere`, 1104-1185); a bail ends the run after 3 s (1187-1235).
//!
//! The run's player's game relays its scores, bail and run end (`Match::outbox`); the others wait for that run end
//! before they move on (ponytail: retail replicates objectivecomplete per console; a relayed end keeps every console's
//! scores the same). Offline (Party Play, or `SKATE_MODE=one_up:<row>`) is the online rules on one pad with the Party
//! Play conventions (decision D6; the graph has no PartyPlay path): the Press-A card before each run, the results
//! dialog (Rematch / Exit Party Play / Change settings) at the end.
use crate::{
    challenge::cutscene::Cutscene,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate3_modes::{
        self, Abort, ChallengeFade, Component, Effect, GREEN, Graph, Handled, HudDriver, Locator, ModeData, Msg, Place,
        RED, ResultsSetup, Rt, View, WHITE, lua_sort,
    },
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// HUD2.eLeaderboardHUDMenuType.ThrowDown (0x8284df88).
const THROWDOWN: i64 = 1;
/// HUD2.eDPOFormat.Points.
const POINTS: i64 = 1;
/// FrontEnd.eResultsLBType.PlaceNameScoreTime; endchallenge_online's eThrowDownType_OneUp.
const PLACE_NAME_SCORE_TIME: i64 = 6;
const THROWDOWN_ONE_UP: i64 = 1;
/// HUD2.eTeamMessageType (0x8284fb70).
const NORMAL: u8 = 0;
const WINNING: u8 = 2;

// 1up.lua's timers (seconds).
/// check_players' wait for the presentation entities (307).
const WAIT_PRES: f64 = 0.5;
/// setup_run's fades and delays (854, 857, 913, 961, 987, 989).
const FADE: f64 = 0.5;
const FADE_DELAY: f64 = 0.1;
const LOAD_DELAY: f64 = 1.;
/// The leading message, the countdown, the bail, the letters, the run's end, the winner (971, 1027, 1222, 1302, 1308,
/// 1344, 1484).
const MESSAGE: f64 = 3.;
/// end_round's delay with no round winner to show (1399); special_disconnected's restart (1561).
const SHORT: f64 = 1.;
const RESTART: f64 = 4.;

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
    pub time_limit: f64,
    pub cumulative_scoring: bool,
    pub allow_player_bail: bool,
    /// The team turn message's time before the countdown (1023-1024).
    pub team_introduction_delay: f64,
    pub start_location_rotation: Vec<Option<Locator>>,
    pub wait_location_rotation: Vec<Option<Locator>>,
    #[serde(rename = "CentralMessageHALIDDidNotBeatHighScore")]
    pub did_not_beat: String,
    #[serde(rename = "CentralMessageHALIDNameBailed")]
    pub name_bailed: String,
    #[serde(rename = "CentralMessageHALIDRoundOver")]
    pub round_over: String,
    #[serde(rename = "CentralMessageHALIDSetScore")]
    pub set_score: String,
    #[serde(rename = "CentralMessageHALIDYourTurn")]
    pub your_turn: String,
    #[serde(rename = "ModifiedCentralMessageHALIDBailed")]
    pub bailed: String,
    #[serde(rename = "ModifiedCentralMessageHALIDOneUpped")]
    pub one_upped: String,
}
impl Row {
    /// Playable: online, every start and wait locator placed, on a known map.
    fn playable(&self, id: &str) -> bool {
        let placed = |r: &[Option<Locator>]| !r.is_empty() && r.iter().all(Option::is_some);
        self.online && placed(&self.start_location_rotation) && placed(&self.wait_location_rotation) && skate3_modes::map_of(id).is_some()
    }
    fn place(&self, place: Place) -> Option<[[f32; 4]; 4]> {
        let rotation = if place.field == START { &self.start_location_rotation } else { &self.wait_location_rotation };
        Some(rotation.get(place.index.checked_sub(1)?)?.as_ref()?.slot(place.slot))
    }
}
const START: &str = "StartLocationRotation";
const WAIT: &str = "WaitLocationRotation";

// ---------------------------------------------------------------- the graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    SetupCode,
    CheckPlayers,
    DynamicCharacterNis,
    NisOverview,
    SetupChallenge,
    SetupRound,
    SetupRun,
    Countdown,
    PerformRun,
    EndRun,
    EndRound,
    RoundHighlight,
    HighlightReel,
    ShowResults,
    CleanupChallenge,
    Complete,
    SpecialDisconnected,
}

/// vars.Competitor[i].
#[derive(Clone, Debug)]
pub(crate) struct Competitor {
    pub name: String,
    pub disconnected: bool,
    pub letters: i64,
    pub eliminated: bool,
    pub best: i64,
    pub teleported: bool,
    /// Online.GetTeam (1 or 2; 1 without teams).
    pub team: usize,
}

/// vars.TeamData[i] (setup_code 368-380).
#[derive(Clone, Debug, Default)]
pub(crate) struct TeamData {
    pub name: String,
    pub rank: i64,
    pub size: usize,
    pub disconnected: usize,
    pub letters: i64,
}

/// vars.PlayerStandings[i].
#[derive(Clone, Copy, Debug)]
struct Standing {
    player: usize,
    letters: i64,
    best: i64,
}

/// 1up.lua's vars on one console. Competitor indices are the Lua's (1-based); a competitor's ID is index - 1.
pub(crate) struct OneUp {
    row: Row,
    intro: f64,
    first: usize,
    me: usize,
    max_letters: i64,
    pub comps: Vec<Competitor>,
    // Timer ids (-1: none).
    wait_pres: i64,
    bail_timer: i64,
    challenge_timer: i64,
    countdown_timer: i64,
    end_round_timer: i64,
    end_run_timer: i64,
    fade_in_delay: i64,
    fade_out_delay: i64,
    leading_timer: i64,
    load_delay: i64,
    restart_timer: i64,
    show_letters: i64,
    show_winner: i64,
    hud_loaded: bool,
    setup_complete: bool,
    current_disconnected: bool,
    leader_disconnected: bool,
    closed_loading: bool,
    pub challenge_over: bool,
    round_over: bool,
    round_restarted: bool,
    non_elim: Vec<usize>,
    current_start: usize,
    current_wait: usize,
    /// RoundStartPlayerIndex / RoundLeaderPlayerIndex: -1 for none, None for the Lua's nil.
    round_start: Option<i64>,
    round_leader: Option<i64>,
    pub round_best: i64,
    pub round_has_leader: bool,
    players_in_round: Vec<usize>,
    round_order: Option<i64>,
    challenge_started: bool,
    pub run_best: i64,
    pub player_bailed: bool,
    /// CurrentCompetitorIndex.
    pub current: Option<usize>,
    new_leader: bool,
    standings: Vec<Standing>,
    /// Bumped at every setup_run: the relay's run number.
    pub run: u32,
    /// Online, the run is another console's: perform_run waits for its relayed end.
    pub remote: bool,
    pub awaiting_end: bool,
    /// Party Play (D6): the Press-A card opens each run's countdown, the results dialog ends the game.
    pub party: bool,
    /// Party Play's round for the card: bumped at every new round (setup_round without a restart).
    /// ponytail: 1up.lua keeps no round count; the card's round is this one.
    pub round: i64,
    /// Online.IsTeamChallenge / Online.GetTeam: each player's team (1 / 2), empty for none; Teams.GetName's names.
    pub teams_in: Vec<u8>,
    pub team_names: [String; 2],
    /// vars.TeamsEnabled, TeamData, CurrentTeam / OtherTeam (0: the Lua's nil), RoundLeadingTeam (-1: none).
    pub teams_enabled: bool,
    pub team_data: Vec<TeamData>,
    pub current_team: usize,
    other_team: usize,
    round_leading_team: i64,
    already_faded_out: bool,
    team_turn_timer: i64,
    highlight_delay: i64,
    /// CurrentlyWatchingID.
    watching: usize,
}

impl OneUp {
    /// `first`: Random.GetRandomNumber's draw (setup_challenge 734: NonEliminatedPlayers[first % count + 1]).
    pub fn new(row: Row, intro: f64, players: usize, me: usize, first: usize) -> Self {
        let comps = (0..players)
            .map(|i| Competitor { name: format!("P{i}"), disconnected: false, letters: 0, eliminated: false, best: 0, teleported: false, team: 1 })
            .collect();
        OneUp {
            row,
            intro,
            first,
            me,
            max_letters: 3,
            comps,
            wait_pres: -1,
            bail_timer: -1,
            challenge_timer: -1,
            countdown_timer: -1,
            end_round_timer: -1,
            end_run_timer: -1,
            fade_in_delay: -1,
            fade_out_delay: -1,
            leading_timer: -1,
            load_delay: -1,
            restart_timer: -1,
            show_letters: -1,
            show_winner: -1,
            hud_loaded: false,
            setup_complete: false,
            current_disconnected: false,
            leader_disconnected: false,
            closed_loading: false,
            challenge_over: false,
            round_over: false,
            round_restarted: false,
            non_elim: Vec::new(),
            current_start: 0,
            current_wait: 0,
            round_start: None,
            round_leader: Some(-1),
            round_best: 0,
            round_has_leader: false,
            players_in_round: Vec::new(),
            round_order: None,
            challenge_started: false,
            run_best: 0,
            player_bailed: false,
            current: None,
            new_leader: false,
            standings: Vec::new(),
            run: 0,
            remote: false,
            awaiting_end: false,
            party: false,
            round: 0,
            teams_in: Vec::new(),
            team_names: ["T1".into(), "T2".into()],
            teams_enabled: false,
            team_data: Vec::new(),
            current_team: 0,
            other_team: 0,
            round_leading_team: -1,
            already_faded_out: false,
            team_turn_timer: -1,
            highlight_delay: -1,
            watching: me,
        }
    }
    /// Party Play's Rematch (hudretry) and Restart Challenge (restartchallenge): setup_challenge again, as the online
    /// graph's own hudretry / restartchallenge do; the graph's timers stop first.
    fn restart(&mut self, rt: &mut Rt<St>) {
        let timers = [
            self.wait_pres,
            self.bail_timer,
            self.challenge_timer,
            self.countdown_timer,
            self.end_round_timer,
            self.end_run_timer,
            self.fade_in_delay,
            self.fade_out_delay,
            self.leading_timer,
            self.load_delay,
            self.restart_timer,
            self.show_letters,
            self.show_winner,
            self.team_turn_timer,
            self.highlight_delay,
        ];
        for t in timers {
            if let Ok(id) = u32::try_from(t) {
                rt.remove_timer(id);
            }
        }
        rt.clear_modified();
        rt.reset_effect(Effect::FadeIn);
        rt.reset_effect(Effect::FadeOut);
        rt.dialog = false;
        self.round = 0;
        rt.change(St::SetupChallenge);
    }
    fn local(&self) -> usize {
        self.me + 1
    }
    /// Competitor `i` (1-based) is this console's player. Party Play (D6) has no "you" on its one pad: every player gets
    /// the named, white message. ponytail: the others' variant for all; 1up.lua has no party path to cite.
    fn mine(&self, i: Option<usize>) -> bool {
        !self.party && i == Some(self.local())
    }
    fn comp(&self, i: Option<usize>) -> Result<&Competitor, Abort> {
        i.and_then(|i| self.comps.get(i.checked_sub(1)?)).ok_or(Abort)
    }
    /// PlayersInRound[RoundOrderIndex] (nil out of range).
    fn at_order(&self) -> Option<usize> {
        let i = self.round_order?;
        (1..=self.players_in_round.len() as i64).contains(&i).then(|| self.players_in_round[i as usize - 1])
    }
    /// The player whose run it is (Competitor[CurrentCompetitorIndex].ID).
    pub fn competitor(&self) -> Option<usize> {
        self.comp(self.current).ok().map(|_| self.current.unwrap() - 1)
    }
    /// vars:CalculateScores (479-503): with teams, the team with fewer letters ranks 1 (a tie: both), and every player
    /// takes the team's letters.
    fn calculate_scores(&mut self) {
        if self.teams_enabled {
            let (a, b) = (self.team_data[0].letters, self.team_data[1].letters);
            (self.team_data[0].rank, self.team_data[1].rank) = if a < b {
                (1, 2)
            } else if a > b {
                (2, 1)
            } else {
                (1, 1)
            };
            for c in &mut self.comps {
                c.letters = self.team_data[c.team - 1].letters;
            }
        }
        self.standings = (1..=self.comps.len()).map(|i| Standing { player: i, letters: self.comps[i - 1].letters, best: self.comps[i - 1].best }).collect();
        lua_sort(&mut self.standings, |a, b| a.letters < b.letters || (a.letters == b.letters && a.best > b.best));
    }
    /// vars:UpdateLeaderboardHUD (517-528).
    fn update_leaderboard(&self, rt: &mut Rt<St>) {
        for (i, c) in self.comps.iter().enumerate() {
            rt.board_rank(i, i as i64 + 1);
            rt.board_int(i, c.letters);
        }
        if self.teams_enabled {
            for (i, t) in self.team_data.iter().enumerate() {
                rt.board_team_rank(i + 1, t.rank);
                rt.board_team_score(i + 1, t.letters);
            }
        }
    }
    /// vars.TeamData[t] (nil out of range: the handler stops).
    fn team(&self, t: i64) -> Result<&TeamData, Abort> {
        usize::try_from(t - 1).ok().and_then(|i| self.team_data.get(i)).ok_or(Abort)
    }
    /// Competitor ids on the team whose run it is.
    fn current_members(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.comps.len()).filter(move |&i| self.comps[i].team == self.current_team)
    }
    /// vars.SwitchCameraToCurrentTeam (660-687): this player's own run, else the current team's player at this
    /// player's position in its own team.
    fn camera_to_current_team(&mut self, rt: &mut Rt<St>) {
        let me = self.me;
        let mine = self.comps[me].team;
        if mine == self.current_team {
            rt.camera(me);
            self.watching = me;
            return;
        }
        let position = 1 + (0..me).filter(|&i| self.comps[i].team == mine).count();
        let (mut seen, mut watch) = (0, me);
        for i in 0..self.comps.len() {
            if !self.comps[i].disconnected && self.comps[i].team == self.current_team {
                seen += 1;
                watch = i;
                if seen == position {
                    break;
                }
            }
        }
        rt.camera(watch);
        self.watching = watch;
    }
    /// Online, team runs: the console that ends the team's run and relays its end (the first connected player of the
    /// team). ponytail: retail has every console run the run's timers; one end keeps the consoles' scores the same.
    pub fn run_owner(&self) -> Option<usize> {
        self.current_members().find(|&i| !self.comps[i].disconnected)
    }
    /// This console's player skates in the run (its own, or its team's).
    pub fn runs(&self, player: usize) -> bool {
        if self.teams_enabled { self.comps.get(player).is_some_and(|c| c.team == self.current_team && !c.disconnected) } else { self.competitor() == Some(player) }
    }
    fn timer(id: u32, var: i64) -> bool {
        i64::from(id) == var
    }

    /// [global] playerdisconnected (201-286).
    fn disconnected(&mut self, rt: &mut Rt<St>, p: usize) -> Handled {
        if p == self.me {
            rt.leave();
            rt.change(St::CleanupChallenge);
            return Ok(());
        }
        if !self.setup_complete {
            return Ok(());
        }
        let Some(i) = (1..=self.comps.len()).find(|&i| i - 1 == p && !self.comps[i - 1].disconnected) else { return Ok(()) };
        let max = self.max_letters;
        let c = &mut self.comps[i - 1];
        (c.letters, c.eliminated, c.disconnected) = (max, true, true);
        if self.hud_loaded {
            rt.board_int(p, max);
        }
        // The team path (220-235): a team with no one left loses.
        if self.teams_enabled {
            let team = self.comps[i - 1].team;
            self.team_data[team - 1].disconnected += 1;
            if self.watching == p {
                self.camera_to_current_team(rt);
            }
            let t = &mut self.team_data[team - 1];
            if t.disconnected >= t.size {
                t.letters = max;
                self.challenge_over = true;
                if self.challenge_started {
                    rt.change(St::SpecialDisconnected);
                }
            }
            return Ok(());
        }
        if !self.challenge_started {
            return Ok(());
        }
        self.players_in_round.retain(|&x| x != i);
        if let Some(j) = self.players_in_round.iter().position(|&x| Some(x) == self.current) {
            self.round_order = Some(j as i64 + 1);
        }
        self.non_elim.retain(|&x| x != i);
        let mut special = false;
        if self.non_elim.len() < 2 {
            self.challenge_over = true;
            special = true;
        }
        if self.round_leader == Some(i as i64) {
            let name = self.comps[i - 1].name.clone();
            rt.central("ID_ONLINE_EVENT_LEAVE", WHITE, &[name], 0);
            self.round_leader = Some(-1);
            self.round_has_leader = false;
            self.leader_disconnected = true;
            special = true;
        }
        if self.comp(self.current).is_ok_and(|_| self.current == Some(i)) {
            self.current_disconnected = true;
            self.run_best = 0;
            special = true;
        }
        if special {
            rt.change(St::SpecialDisconnected);
        }
        Ok(())
    }

    /// setup_code's stateenter (318-688), the online path.
    fn setup_code(&mut self, rt: &mut Rt<St>) {
        rt.pin(self.me, true);
        self.hud_loaded = false;
        self.setup_complete = false;
        self.current_disconnected = false;
        self.leader_disconnected = false;
        self.max_letters = 3;
        self.team_turn_timer = -1;
        self.highlight_delay = -1;
        self.watching = self.me;
        self.teams_enabled = !self.party && !self.teams_in.is_empty();
        if self.teams_enabled {
            self.team_data = self.team_names.iter().map(|name| TeamData { name: name.clone(), rank: 1, ..Default::default() }).collect();
        }
        let max = self.max_letters;
        for (i, c) in self.comps.iter_mut().enumerate() {
            c.disconnected = rt.gone[i];
            c.letters = 0;
            c.eliminated = false;
            // Online.GetTeam (0x828463d8: the player's +112, plus 1).
            c.team = if self.teams_in.get(i) == Some(&2) { 2 } else { 1 };
            if c.disconnected {
                c.letters = max;
                c.eliminated = true;
            }
        }
        if self.teams_enabled {
            for c in &self.comps {
                let t = &mut self.team_data[c.team - 1];
                t.size += 1;
                t.disconnected += usize::from(c.disconnected);
            }
        }
        rt.pause(true); // 446
        self.setup_complete = true;
        // HUD2.CreateGroup: hudloaded once the group's movies are up.
        rt.post(Msg::HudLoaded);
    }

    /// setup_round's stateenter (780-849).
    fn setup_round(&mut self, rt: &mut Rt<St>) -> Handled {
        if !self.round_restarted {
            self.round += 1;
            self.current_start += 1;
            if self.current_start > self.row.start_location_rotation.len() {
                self.current_start = 1;
            }
            self.current_wait += 1;
            if self.current_wait > self.row.wait_location_rotation.len() {
                self.current_wait = 1;
            }
        }
        self.round_best = 0;
        self.round_has_leader = false;
        self.round_over = false;
        self.challenge_started = true;
        if self.teams_enabled {
            self.round_leading_team = -1;
            self.other_team = if self.current_team == 1 { 2 } else { 1 };
            // 826-834: a team with no one left ends the challenge.
            if let Some(t) = self.team_data.iter_mut().find(|t| t.disconnected >= t.size) {
                self.challenge_over = true;
                t.letters = self.max_letters;
            }
            rt.change(if self.challenge_over { St::EndRound } else { St::SetupRun });
            return Ok(());
        }
        self.round_leader = Some(-1);
        self.players_in_round = self.non_elim.clone();
        let start = self.round_start.ok_or(Abort)?;
        if 0 < start
            && let Some(j) = self.players_in_round.iter().position(|&p| p as i64 == start)
        {
            self.round_order = Some(j as i64 + 1);
        }
        self.challenge_over = self.players_in_round.len() < 2;
        rt.change(if self.challenge_over { St::EndRound } else { St::SetupRun });
        Ok(())
    }

    /// setup_run's timerexpired (919-978).
    fn setup_run_timer(&mut self, rt: &mut Rt<St>, id: u32) -> Handled {
        if Self::timer(id, self.fade_out_delay) {
            if self.teams_enabled {
                // The whole team at the start locator, slot by slot (921-930).
                let mut slot = 0;
                for i in 0..self.comps.len() {
                    let place = if self.comps[i].team == self.current_team {
                        slot += 1;
                        Place { field: START, index: self.current_start, slot: slot - 1 }
                    } else {
                        Place { field: WAIT, index: self.current_wait, slot: i }
                    };
                    rt.reposition(i, place, 0.);
                }
                self.camera_to_current_team(rt);
                return Ok(());
            }
            self.current = self.at_order();
            let current = self.competitor().ok_or(Abort)?;
            for i in 0..self.comps.len() {
                let place = if i == current {
                    Place { field: START, index: self.current_start, slot: 0 }
                } else {
                    Place { field: WAIT, index: self.current_wait, slot: i }
                };
                rt.reposition(i, place, 0.);
            }
            rt.camera(current);
            self.watching = current;
            // UseStaticCameras is false on every row (rules.json): no SwitchChallengeCamera.
        } else if Self::timer(id, self.fade_in_delay) {
            rt.start_effect(Effect::FadeIn, FADE);
        } else if Self::timer(id, self.load_delay) {
            if self.round_has_leader && self.teams_enabled {
                let param = format!("{},{}", self.team(self.round_leading_team)?.name, self.round_best);
                rt.custom_team_central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ISLEADING", self.round_leading_team - 1, WINNING, &param);
                self.leading_timer = rt.add_timer(MESSAGE).into();
            } else if self.round_has_leader {
                let leader = self.comp(self.round_leader.and_then(|l| usize::try_from(l).ok()))?;
                let param = format!("{},{}", leader.name, self.round_best);
                rt.custom_central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ISLEADING", WHITE, &param);
                self.leading_timer = rt.add_timer(MESSAGE).into();
            } else {
                rt.change(St::Countdown);
            }
        } else if Self::timer(id, self.leading_timer) {
            rt.change(St::Countdown);
        }
        Ok(())
    }

    /// end_run's stateenter (1238-1318).
    fn end_run(&mut self, rt: &mut Rt<St>) -> Handled {
        if self.current_disconnected {
            self.current_disconnected = false;
            self.show_letters = rt.add_timer(MESSAGE).into();
            return Ok(());
        }
        if self.teams_enabled {
            return self.end_team_run(rt);
        }
        let current = self.competitor().ok_or(Abort)?;
        rt.pin(current, true);
        self.show_letters = -1;
        self.new_leader = false;
        let local_run = self.mine(self.at_order());
        if self.run_best > self.round_best {
            let id = if self.round_best > 0 { "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_NEWSCORESET" } else { "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORESET" };
            rt.custom_central(id, if local_run { GREEN } else { WHITE }, &self.run_best.to_string());
            self.round_has_leader = true;
            self.new_leader = true;
            self.round_best = self.run_best;
            self.round_leader = self.at_order().map(|p| p as i64);
            // print("New leader index ... " .. RoundLeaderPlayerIndex): nil stops the handler.
            self.round_leader.ok_or(Abort)?;
        } else if self.round_has_leader {
            let max = self.max_letters;
            let c = &mut self.comps[current];
            c.letters = (c.letters + 1).min(max);
            let letters = c.letters;
            rt.letters(letters, false, self.mine(self.current));
            self.show_letters = rt.add_timer(MESSAGE).into();
        } else {
            rt.central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORENOTSET", WHITE, &[], 0);
        }
        if self.show_letters == -1 {
            self.end_run_timer = rt.add_timer(MESSAGE).into();
        }
        Ok(())
    }

    /// end_run's stateenter, the team path (1246-1309): the team sets the score, or takes a letter.
    fn end_team_run(&mut self, rt: &mut Rt<St>) -> Handled {
        for i in self.current_members().collect::<Vec<_>>() {
            rt.pin(i, true);
        }
        self.show_letters = -1;
        self.new_leader = false;
        let team = self.current_team as i64;
        if self.run_best > self.round_best {
            let id = if self.round_best > 0 { "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_NEWSCORESET" } else { "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORESET" };
            rt.custom_team_central(id, team - 1, WINNING, &self.run_best.to_string());
            self.round_has_leader = true;
            self.new_leader = true;
            self.round_best = self.run_best;
            self.round_leading_team = team;
        } else if self.round_has_leader {
            let max = self.max_letters;
            let t = self.team_data.get_mut(self.current_team.wrapping_sub(1)).ok_or(Abort)?;
            t.letters = (t.letters + 1).min(max);
            let letters = t.letters;
            for c in self.comps.iter_mut().filter(|c| c.team == self.current_team) {
                c.letters = letters;
            }
            rt.letters(letters, false, self.comps[self.me].team == self.current_team);
            self.show_letters = rt.add_timer(MESSAGE).into();
        } else {
            rt.central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORENOTSET", WHITE, &[], 0);
        }
        if self.show_letters == -1 {
            self.end_run_timer = rt.add_timer(MESSAGE).into();
        }
        Ok(())
    }

    /// end_run's timerexpired (1321-1386).
    fn end_run_timer(&mut self, rt: &mut Rt<St>, id: u32) -> Handled {
        if Self::timer(id, self.show_letters) && self.teams_enabled {
            // A letter ends the round (1324-1326).
            self.round_over = true;
            self.challenge_over = self.team(self.current_team as i64)?.letters >= self.max_letters;
            self.end_run_timer = rt.add_timer(SHORT).into();
            self.calculate_scores();
            self.update_leaderboard(rt);
        } else if Self::timer(id, self.end_run_timer) && self.teams_enabled && !self.round_over {
            std::mem::swap(&mut self.current_team, &mut self.other_team);
            rt.change(St::SetupRun);
        } else if Self::timer(id, self.show_letters) {
            let mut delay = SHORT;
            // Not retail (user 10-12): the run's player left as the round's last in order: playerdisconnected took it
            // out of PlayersInRound already and RoundOrderIndex points past the end; retail's print(... .. playerIndex)
            // stops on the nil and every console stays in end_run for good (matrix 10-12). Nothing to take out: go on.
            let player = self.at_order();
            let order = self.round_order.ok_or(Abort)? as usize;
            if player.is_some() {
                self.players_in_round.remove(order - 1);
            }
            self.round_over = self.players_in_round.len() < 2;
            let current = self.comp(self.current)?.clone();
            let c = self.current.ok_or(Abort)?;
            if current.letters >= self.max_letters {
                if let Some(k) = self.non_elim.iter().position(|&p| Some(p) == player) {
                    self.non_elim.remove(k);
                    self.comps[c - 1].eliminated = true;
                    let colour = if self.mine(Some(c)) { WHITE } else { GREEN };
                    rt.central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ELIMINATED", colour, &[current.name.clone()], 0);
                    delay = MESSAGE;
                }
            } else if !self.round_over {
                if self.mine(Some(c)) {
                    rt.central(&self.row.did_not_beat.clone(), WHITE, &[], 0);
                } else {
                    rt.central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHER_OUTFORROUND", GREEN, &[current.name.clone()], 0);
                }
                delay = MESSAGE;
            }
            self.challenge_over = self.non_elim.len() <= 1;
            self.end_run_timer = rt.add_timer(delay).into();
            self.calculate_scores();
            self.update_leaderboard(rt);
        } else if Self::timer(id, self.end_run_timer) {
            if self.round_over {
                rt.change(St::EndRound);
            } else {
                let mut order = self.round_order.ok_or(Abort)?;
                if self.new_leader || !self.round_has_leader {
                    order += 1;
                }
                if order > self.players_in_round.len() as i64 {
                    order = 1;
                }
                self.round_order = Some(order);
                rt.change(St::SetupRun);
            }
        }
        Ok(())
    }

    /// end_round's stateenter (1397-1461).
    fn end_round(&mut self, rt: &mut Rt<St>) -> Handled {
        let mut delay = SHORT;
        if self.teams_enabled {
            if !self.challenge_over && self.round_has_leader {
                delay = MESSAGE;
                let name = self.team(self.round_leading_team)?.name.clone();
                rt.team_central(&self.row.round_over.clone(), self.round_leading_team - 1, WINNING, &[name]);
            }
            rt.show(Component::DPOHUD, false);
            if !self.challenge_over {
                // The round's winner starts the next round (1414).
                self.current_team = usize::try_from(self.round_leading_team).unwrap_or(0);
                self.round_has_leader = false;
            }
            self.end_round_timer = rt.add_timer(delay).into();
            return Ok(());
        }
        if !self.challenge_over && self.round_has_leader {
            delay = MESSAGE;
            let l = self.round_leader.and_then(|l| usize::try_from(l).ok());
            let leader = self.comp(l)?.name.clone();
            let colour = if self.mine(l) { GREEN } else { WHITE };
            rt.central(&self.row.round_over.clone(), colour, &[leader], 0);
        }
        rt.show(Component::DPOHUD, false);
        if !self.challenge_over {
            self.round_start = self.round_leader;
            // print(... .. RoundLeaderPlayerIndex): nil stops the handler.
            self.round_leader.ok_or(Abort)?;
            self.round_leader = Some(-1);
            self.round_has_leader = false;
        }
        self.end_round_timer = rt.add_timer(delay).into();
        Ok(())
    }

    /// special_disconnected's stateenter (1547-1576).
    fn special(&mut self, rt: &mut Rt<St>) -> Handled {
        rt.clear_modified();
        if let Some(p) = self.competitor() {
            rt.pin(p, true);
        }
        for t in [self.countdown_timer, self.challenge_timer, self.end_run_timer] {
            if let Ok(id) = u32::try_from(t) {
                rt.remove_timer(id);
            }
        }
        rt.show(Component::CountdownHUD, false);
        if !self.challenge_over {
            if self.leader_disconnected {
                self.leader_disconnected = false;
                // The Lua's own mix-up: a PlayersInRound position used as a player index.
                self.round_start = self.round_order;
                self.round_restarted = true;
                self.restart_timer = rt.add_timer(RESTART).into();
                rt.modified("ID_CHALLENGE_CENTRAL_MESSAGE_ROUNDOVER", WHITE, &[], false);
            } else if self.current_disconnected {
                rt.change(St::EndRun);
            } else {
                rt.change(St::EndRound);
            }
        } else {
            if self.current_disconnected {
                rt.camera(self.me);
            }
            rt.reset_effect(Effect::FadeIn);
            rt.reset_effect(Effect::FadeOut);
            rt.change(St::EndRound);
        }
        Ok(())
    }

    /// The relayed end of another console's run: its score, then end_run.
    pub fn remote_end(&mut self, rt: &mut Rt<St>, best: i64) {
        if rt.state == St::PerformRun {
            self.run_best = best;
            self.awaiting_end = false;
            rt.change(St::EndRun);
        }
    }
}

impl Graph for OneUp {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::SetupCode => "setup_code",
            St::CheckPlayers => "check_players",
            St::DynamicCharacterNis => "dynamic_character_nis",
            St::NisOverview => "nis_overview",
            St::SetupChallenge => "setup_challenge",
            St::SetupRound => "setup_round",
            St::SetupRun => "setup_run",
            St::Countdown => "countdown",
            St::PerformRun => "perform_run",
            St::EndRun => "end_run",
            St::EndRound => "end_round",
            St::RoundHighlight => "round_highlight",
            St::HighlightReel => "highlight_reel",
            St::ShowResults => "show_results",
            St::CleanupChallenge => "cleanup_challenge",
            St::Complete => "complete",
            St::SpecialDisconnected => "special_disconnected",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        use St::*;
        if let Msg::Left(p) = msg {
            self.disconnected(rt, *p)?;
        }
        // Party Play: Restart Challenge, everywhere but before the game is set up.
        if *msg == Msg::Restart {
            if self.party && !matches!(rt.state, Initial | SetupCode | CheckPlayers | DynamicCharacterNis | NisOverview | Complete) {
                self.restart(rt);
            }
            return Ok(());
        }
        match (rt.state, msg) {
            (Initial, Msg::Enter) => rt.change(SetupCode),
            (SetupCode, Msg::Enter) => self.setup_code(rt),
            (SetupCode, Msg::HudLoaded) => {
                rt.dpo_format(POINTS);
                rt.change(CheckPlayers);
            }
            (CheckPlayers, Msg::Enter) => {
                let teams = if self.teams_enabled { self.comps.iter().map(|c| c.team as u8).collect() } else { Vec::new() };
                rt.board_init_teams(THROWDOWN, self.comps.len(), teams, self.team_names.clone());
                self.hud_loaded = true;
                self.wait_pres = rt.add_timer(WAIT_PRES).into();
            }
            (CheckPlayers, Msg::Timer(id)) if Self::timer(*id, self.wait_pres) => rt.change(DynamicCharacterNis),
            // IntroNIS is for team challenges (1587).
            (DynamicCharacterNis, Msg::Enter) => rt.change(NisOverview),
            (NisOverview, Msg::Enter) => rt.present(self.intro),
            (NisOverview, Msg::Presented) => rt.change(SetupChallenge),
            (SetupChallenge, Msg::Enter) => {
                self.closed_loading = false;
                self.challenge_over = false;
                self.round_over = false;
                self.already_faded_out = false;
                self.round_restarted = false;
                self.non_elim = (1..=self.comps.len()).filter(|&i| !self.comps[i - 1].disconnected).collect();
                self.current_start = 0;
                self.current_wait = 0;
                if self.teams_enabled {
                    // Random.GetRandomNumber(2) (732).
                    self.current_team = self.first % 2 + 1;
                } else {
                    if self.non_elim.is_empty() {
                        return Err(Abort);
                    }
                    self.round_start = Some(self.non_elim[self.first % self.non_elim.len()] as i64);
                }
                for i in 0..self.comps.len() {
                    rt.pin(i, true);
                }
                let max = self.max_letters;
                for c in &mut self.comps {
                    c.letters = if c.disconnected { max } else { 0 };
                    c.best = 0;
                }
                for t in &mut self.team_data {
                    t.letters = if t.disconnected >= t.size { max } else { 0 };
                }
                self.update_leaderboard(rt);
                rt.show(Component::LeaderboardHUD, true);
                rt.change(SetupRound);
            }
            (SetupRound, Msg::Enter) => self.setup_round(rt)?,
            (SetupRun, Msg::Enter) => {
                // After a team round's highlight the screen is already black (853-858).
                if !self.already_faded_out {
                    rt.start_effect(Effect::FadeOut, FADE);
                } else {
                    self.already_faded_out = false;
                    self.fade_out_delay = rt.add_timer(FADE_DELAY).into();
                }
                self.run_best = 0;
                self.player_bailed = false;
                self.run += 1;
                for c in self.comps.iter_mut().filter(|c| !c.disconnected) {
                    c.teleported = false;
                }
                if self.teams_enabled {
                    if self.round_has_leader {
                        let title = format!("#{}", self.team(self.round_leading_team)?.name);
                        rt.dpo_title(&title);
                        rt.dpo_params(0, Some(self.round_best));
                        rt.dpo_team(self.round_leading_team - 1);
                    } else {
                        rt.dpo_title(&self.row.set_score.clone());
                        rt.dpo_params(0, None);
                        rt.dpo_team(-1);
                    }
                } else if self.round_has_leader {
                    let leader = self.comp(self.round_leader.and_then(|l| usize::try_from(l).ok()))?;
                    let title = format!("#{}", leader.name);
                    rt.dpo_title(&title);
                    rt.dpo_params(0, Some(self.round_best));
                } else {
                    rt.dpo_title(&self.row.set_score.clone());
                    rt.dpo_params(0, None);
                }
                rt.show(Component::DPOHUD, true);
            }
            (SetupRun, Msg::Teleported(p)) => {
                if let Some(c) = self.comps.get_mut(*p) {
                    c.teleported = true;
                    if self.comps.iter().all(|c| c.disconnected || c.teleported) {
                        self.fade_in_delay = rt.add_timer(FADE_DELAY).into();
                    }
                }
            }
            (SetupRun, Msg::Timer(id)) => self.setup_run_timer(rt, *id)?,
            (SetupRun, Msg::Effect(Effect::FadeOut)) => {
                if !self.closed_loading {
                    rt.close_loading();
                    self.closed_loading = true;
                }
                self.fade_out_delay = rt.add_timer(FADE_DELAY).into();
            }
            (SetupRun, Msg::Effect(Effect::FadeIn)) => self.load_delay = rt.add_timer(LOAD_DELAY).into(),
            // D6: the run's player's Press-A card in place of the turn message; A starts the countdown.
            (Countdown, Msg::Enter) if self.party => {
                let current = self.competitor().ok_or(Abort)?;
                // ponytail: 1up.lua's only Pause call is setup_code's Disable (446) and it has no party path, so the
                // Party Play crossbar (Exit Party Play) never opened. turn_based_uniform's party path enables it here,
                // before its Press-A card (spot_battle.lua 1501); 1-UP does the same.
                rt.pause(false);
                rt.turn(current);
                rt.set_player(current);
                rt.press_a(current + 1, self.round);
            }
            (Countdown, Msg::HudComplete) if self.party => {
                let timer = rt.add_timer(MESSAGE);
                self.countdown_timer = timer.into();
                rt.countdown(timer);
            }
            // Teams (1001-1024): the team's turn icons and its turn message, the countdown after TeamIntroductionDelay.
            (Countdown, Msg::Enter) if self.teams_enabled => {
                let mut id = self.row.your_turn.clone();
                if !self.round_has_leader {
                    id = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SETYOURSCORE".into();
                }
                for i in 0..self.comps.len() {
                    rt.turn_set(i, self.comps[i].team == self.current_team);
                }
                let name = self.team(self.current_team as i64)?.name.clone();
                if self.current_team != self.comps[self.me].team {
                    id = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN".into();
                }
                rt.team_central(&id, self.current_team as i64 - 1, NORMAL, &[name]);
                self.team_turn_timer = rt.add_timer(self.row.team_introduction_delay).into();
            }
            (Countdown, Msg::Timer(id)) if Self::timer(*id, self.team_turn_timer) => {
                let timer = rt.add_timer(MESSAGE);
                self.countdown_timer = timer.into();
                rt.countdown(timer);
            }
            (Countdown, Msg::Enter) => {
                let mut id = self.row.your_turn.clone();
                if !self.round_has_leader {
                    id = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SETYOURSCORE".into();
                }
                let current = self.competitor().ok_or(Abort)?;
                rt.turn(current);
                let name = self.comps[current].name.clone();
                if current != self.me {
                    id = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_OTHERSTURN".into();
                }
                rt.modified(&id, WHITE, &[name], false);
                let timer = rt.add_timer(MESSAGE);
                self.countdown_timer = timer.into();
                rt.countdown(timer);
            }
            (Countdown, Msg::Timer(id)) if Self::timer(*id, self.countdown_timer) => rt.change(PerformRun),
            (PerformRun, Msg::Enter) => {
                if self.teams_enabled {
                    for i in self.current_members().collect::<Vec<_>>() {
                        rt.pin(i, false);
                    }
                } else {
                    let current = self.competitor().ok_or(Abort)?;
                    rt.pin(current, false);
                }
                let timer = rt.add_timer(self.row.time_limit);
                self.challenge_timer = timer.into();
                rt.timer(timer);
                self.awaiting_end = false;
            }
            (PerformRun, Msg::Timer(id)) if Self::timer(*id, self.challenge_timer) || Self::timer(*id, self.bail_timer) => {
                if self.remote {
                    self.awaiting_end = true;
                } else {
                    rt.change(EndRun);
                }
            }
            (PerformRun, Msg::Objective { player, score }) => {
                let i = *player;
                if i >= self.comps.len() {
                    return Ok(());
                }
                let old = self.run_best;
                let c = &mut self.comps[i];
                c.best = c.best.max(*score);
                if self.row.cumulative_scoring {
                    self.run_best = self.run_best.saturating_add(*score); // a relayed score can be i64::MAX
                } else if *score > self.run_best {
                    self.run_best = *score;
                }
                if self.round_has_leader && self.run_best > self.round_best && old <= self.round_best {
                    if self.teams_enabled {
                        rt.team_central(&self.row.one_upped.clone(), self.current_team as i64 - 1, WINNING, &[self.run_best.to_string()]);
                    } else {
                        let colour = if self.mine(self.at_order()) { GREEN } else { WHITE };
                        rt.central(&self.row.one_upped.clone(), colour, &[], 0);
                    }
                }
                rt.dpo_params(self.run_best, self.round_has_leader.then_some(self.round_best));
                if self.run_best > self.round_best {
                    if self.teams_enabled {
                        rt.dpo_team(self.current_team as i64 - 1);
                        let title = format!("#{}", self.team(self.current_team as i64)?.name);
                        rt.dpo_title(&title);
                    } else {
                        let title = format!("#{}", self.comps[i].name);
                        rt.dpo_title(&title);
                    }
                }
            }
            // Teams (1200-1212): a bail by anyone on the team ends the team's run.
            (PerformRun, Msg::Wipeout(p)) if self.teams_enabled => {
                let team = self.comps.get(*p).map(|c| c.team);
                if !self.row.allow_player_bail && !self.player_bailed && team == Some(self.current_team) {
                    self.player_bailed = true;
                    if let Ok(t) = u32::try_from(self.challenge_timer) {
                        rt.remove_timer(t);
                    }
                    if *p == self.me {
                        rt.modified(&self.row.bailed.clone(), RED, &[], false);
                    } else if self.comps[self.me].team == self.current_team {
                        rt.modified(&self.row.name_bailed.clone(), RED, &[self.comps[*p].name.clone()], false);
                    } else {
                        let name = self.team(self.current_team as i64)?.name.clone();
                        rt.modified(&self.row.name_bailed.clone(), WHITE, &[name], false);
                    }
                    self.bail_timer = rt.add_timer(MESSAGE).into();
                }
            }
            (PerformRun, Msg::Wipeout(p)) => {
                if !self.row.allow_player_bail && !self.player_bailed {
                    let current = self.competitor().ok_or(Abort)?;
                    if *p == current {
                        self.player_bailed = true;
                        if let Ok(t) = u32::try_from(self.challenge_timer) {
                            rt.remove_timer(t);
                        }
                        if self.mine(Some(current + 1)) {
                            rt.modified(&self.row.bailed.clone(), RED, &[], false);
                        } else {
                            let name = self.comps[current].name.clone();
                            rt.modified(&self.row.name_bailed.clone(), WHITE, &[name], false);
                        }
                        self.bail_timer = rt.add_timer(MESSAGE).into();
                    }
                }
            }
            (EndRun, Msg::Enter) => self.end_run(rt)?,
            (EndRun, Msg::Timer(id)) => self.end_run_timer(rt, *id)?,
            (EndRun, Msg::Exit) => rt.clear_modified(),
            (EndRound, Msg::Enter) => self.end_round(rt)?,
            (EndRound, Msg::Timer(id)) => {
                if Self::timer(*id, self.end_round_timer) {
                    if self.challenge_over {
                        self.calculate_scores();
                        if self.teams_enabled {
                            // 1470-1474: team 2 when it ranks 1 (a tie too).
                            let winner: i64 = if self.team_data[1].rank == 1 { 2 } else { 1 };
                            let name = self.team(winner)?.name.clone();
                            rt.team_central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS", winner - 1, WINNING, &[name]);
                        } else {
                            let winner = self.standings.first().ok_or(Abort)?.player;
                            let colour = if self.mine(Some(winner)) { GREEN } else { WHITE };
                            let name = self.comps[winner - 1].name.clone();
                            rt.central("ID_CHALLENGE_CENTRAL_MESSAGE_1UP_WINS", colour, &[name], 0);
                        }
                        if !self.closed_loading {
                            rt.close_loading();
                            self.closed_loading = true;
                        }
                        self.show_winner = rt.add_timer(MESSAGE).into();
                    } else {
                        rt.change(RoundHighlight);
                    }
                } else if Self::timer(*id, self.show_winner) {
                    rt.change(HighlightReel);
                }
            }
            // Team challenges fade to a round highlight; the rest go straight on (1617-1648).
            (RoundHighlight, Msg::Enter) if self.teams_enabled => rt.start_effect(Effect::FadeOut, 0.3),
            (RoundHighlight, Msg::Enter) => rt.change(SetupRound),
            (RoundHighlight, Msg::Effect(Effect::FadeOut)) => {
                self.already_faded_out = true;
                self.highlight_delay = rt.add_timer(0.2).into();
            }
            (RoundHighlight, Msg::Timer(id)) if Self::timer(*id, self.highlight_delay) => rt.reel(),
            (RoundHighlight, Msg::Reel) => rt.change(SetupRound),
            (HighlightReel, Msg::Enter) => {
                self.calculate_scores();
                rt.reel();
            }
            (HighlightReel, Msg::Reel) => rt.change(ShowResults),
            (ShowResults, Msg::Enter) => {
                for (i, s) in self.standings.clone().iter().enumerate() {
                    rt.result(s.player - 1, i as i64 + 1, s.letters);
                }
                // D6: Party Play's results dialog.
                if self.party {
                    rt.continue_retry();
                } else {
                    rt.change(CleanupChallenge);
                }
            }
            (ShowResults, Msg::HudRetry) if self.party => self.restart(rt),
            (ShowResults, Msg::HudContinue) if self.party => {
                rt.dialog = false;
                rt.change(CleanupChallenge);
            }
            (CleanupChallenge, Msg::Enter) => {
                rt.camera(self.me);
                rt.pin(self.me, false);
                rt.clear_modified();
                rt.change(Complete);
            }
            (Complete, Msg::Enter) => rt.leave(),
            (SpecialDisconnected, Msg::Enter) => self.special(rt)?,
            (SpecialDisconnected, Msg::Timer(id)) if Self::timer(*id, self.restart_timer) => rt.change(SetupRound),
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// A run event the run's console relays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Relay {
    pub run: u32,
    pub ev: RelayEv,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum RelayEv {
    Score(i64),
    Bail,
    End(i64),
}

struct Play {
    id: String,
    row: Row,
    g: OneUp,
    rt: Rt<St>,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    /// Relayed run events waiting for their run: (sender's slot, event).
    inbox: Vec<(usize, Relay)>,
    placed: Vec<u32>,
    intro: Option<f64>,
    uncovered: bool,
    sent_end: u32,
}

#[derive(Default)]
struct Probe {
    landed: Vec<i64>,
    bailed: bool,
    wiping: bool,
}

#[derive(Resource, Default)]
pub(crate) struct OneUpGame {
    data: Option<ModeData>,
    play: Option<Play>,
    probe: Probe,
    live: bool,
    warned: String,
}

pub(crate) struct Skate3OneUpPlugin;
impl Plugin for Skate3OneUpPlugin {
    fn build(&self, app: &mut App) {
        let data = ModeData::install(app, "1up", "1-Up");
        if let Some(data) = &data {
            let ids: Vec<String> =
                data.rows.keys().filter(|id| data.row::<Row>(id).is_ok_and(|r| r.playable(id))).cloned().collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                // The rows, and their Team tab rows (1up.lua's TeamsEnabled path).
                playable.0.extend(ids.iter().cloned().chain(ids.iter().map(|id| crate::match_core::team(id))));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        app.insert_resource(OneUpGame { data, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick while a run is live: completed sequences (objective `1up_sequence_anywhere`) and wipeouts.
fn probe(mut game: ResMut<OneUpGame>, skater: Res<SkaterRuntime>) {
    if !game.live {
        game.probe = Probe::default();
        return;
    }
    let s = &skater.scoring;
    if s.sequence_landed {
        game.probe.landed.push(s.lua_sequence_score());
    }
    let wiping = skater.wiping_out();
    game.probe.bailed |= wiping && !game.probe.wiping;
    game.probe.wiping = wiping;
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<OneUpGame>| {
        let Some(data) = game.data.take() else { return };
        step(world, &mut game, &data);
        game.data = Some(data);
    });
}

fn step(world: &mut World, game: &mut OneUpGame, data: &ModeData) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && mode == Some(Mode::OneUp) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 1-Up: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
            game.warned = id;
        }
        game.live = false;
        return;
    }
    let row = row.expect("on_map");
    // Another row under the same mode (a Party Play relaunch): the old game ends first.
    if game.play.as_ref().is_some_and(|p| p.id != id) {
        end(world, game);
    }
    if game.play.is_none() {
        if !matches!(phase, Phase::Running { .. }) {
            // The core's intro: everyone held (owned rows: the core leaves it to us).
            world.resource_mut::<Match>().hold = true;
            return;
        }
        game.play = Some(begin(world, data, &id, row, now));
    }
    let play = game.play.as_mut().expect("begun");
    game.live = true;

    // Who left: playerdisconnected.
    if play.online {
        let m = world.resource::<Match>();
        let left: Vec<usize> = (0..play.names.len()).filter(|&p| !play.rt.gone[p] && m.players.get(p).is_none_or(String::is_empty)).collect();
        for p in left {
            play.rt.gone[p] = true;
            skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
        }
    }
    skate3_modes::advance(&mut play.g, &mut play.rt, now);
    // Party Play: the card, the results dialog, the pad's player and the others' waiting slots out; A, Rematch and
    // Restart Challenge back.
    if play.g.party {
        let current = play.g.competitor();
        let waiting = (0..play.names.len())
            .map(|p| match play.rt.landed(p) {
                Some((_, place)) if Some(p) != current && place.field == WAIT => play.row.place(place),
                _ => None,
            })
            .collect();
        let results = play.rt.dialog.then(|| {
            let view = View { names: &play.names, me: play.me, online: false, title: data.text(&play.row.title), label: "1-Up" };
            skate3_modes::results_answers(data, &play.rt.hud, &view, &results_setup(data, &play.row), None)
        });
        let turn = crate::party_play::Turn { controlled: current.unwrap_or(play.rt.controlled), waiting, card: play.rt.card.take(), results };
        for msg in crate::party_play::exchange(world, turn).messages() {
            skate3_modes::dispatch(&mut play.g, &mut play.rt, msg);
        }
        skate3_modes::advance(&mut play.g, &mut play.rt, now);
    }

    // This console's run (its sequences and its bail) and the others' relays.
    let competitor = play.g.competitor();
    let probe = std::mem::take(&mut game.probe);
    let received: Vec<(usize, Relay)> = if play.online {
        world.resource_mut::<Match>().inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect()
    } else {
        Vec::new()
    };
    for relay in play.exchange(probe.landed, probe.bailed, received) {
        send(world, relay);
    }

    // RepositionAI: this console's skater (hot-seat: the run's player's place, on the one skater).
    let who = if play.online { play.me } else { competitor.unwrap_or(0) };
    if let Some((seq, place)) = play.rt.landed(who)
        && play.placed[who] != seq
    {
        play.placed[who] = seq;
        if (play.online || place.field == START)
            && let Some(at) = play.row.place(place)
        {
            world.resource_mut::<Match>().teleport = Some(at);
        }
    }

    // The intro presentation (nis_overview) on the row's flythrough.
    match (play.rt.presenting, play.intro) {
        (Some(end), None) if now < end => {
            if let Some(scene) = &play.row.intro {
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
    // The loading screen: up from the start until the first CloseLoadingScreen (setup_run's fade out, 983-986); an
    // intro shows through it (UNRESOLVED R1, as S.K.A.T.E.).
    play.uncovered |= play.intro.is_some() || !play.rt.covered;
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = !play.uncovered;
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        let alpha = play.rt.fade_alpha(now) as f32;
        if fade.0 != alpha {
            fade.0 = alpha;
        }
    }
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = play.rt.pinned.get(who).copied().unwrap_or(true) || play.rt.presenting_now();
        // The results (their rows set: endchallenge_online, Party Play's dialog) own the pad.
        m.gestures = skate3_modes::gestures(&m, play.rt.left || !play.rt.hud.results.is_empty());
        m.watch = (play.online && play.rt.watching != play.me).then_some(play.rt.watching);
        m.score = -play.g.comps.get(play.me).map_or(0, |c| c.letters);
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
    }
    let view = View { names: &play.names, me: play.me, online: play.online, title: data.text(&play.row.title), label: "1-Up" };
    // Party Play's results are its dialog (above).
    let results = (!play.g.party && !play.rt.hud.results.is_empty()).then(|| results_setup(data, &play.row));
    play.hud.drive(world, data, &play.rt.hud, &view, results.as_ref(), now);
}

/// show_results' columns (1039-1046): online and Party Play both read HEADER_SKATER (NAME_HEADER is SinglePlayer's).
fn results_setup(data: &ModeData, row: &Row) -> ResultsSetup {
    ResultsSetup {
        title: data.text(&row.title),
        layout: PLACE_NAME_SCORE_TIME,
        columns: vec!["ID_LEADERBOARD_HEADER_RANK".into(), "ID_LEADERBOARD_HEADER_SKATER".into(), "ID_COMMON_SCORE".into()],
        throwdown: THROWDOWN_ONE_UP,
        choice: None,
        times: Vec::new(),
        floats: Vec::new(),
        strings: Vec::new(),
    }
}

impl Play {
    /// This console's run: its sequences and its bail, relayed online (Teams: every player of the team skates the run
    /// at once; the team's first connected player's console ends it); then the others' relays (`received`). Returns
    /// what to relay.
    fn exchange(&mut self, landed: Vec<i64>, bailed: bool, received: Vec<(usize, Relay)>) -> Vec<Relay> {
        let mut out = Vec::new();
        let teams = self.g.teams_enabled;
        let competitor = self.g.competitor();
        let local_run = if teams { self.g.runs(self.me) } else { competitor.is_some_and(|c| !self.online || c == self.me) };
        let ends = if teams { self.g.run_owner() == Some(self.me) } else { local_run };
        if local_run && self.rt.state == St::PerformRun {
            let c = if teams { self.me } else { competitor.expect("local run") };
            for score in landed {
                if self.online {
                    out.push(Relay { run: self.g.run, ev: RelayEv::Score(score) });
                }
                skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Objective { player: c, score });
            }
            if bailed && !self.g.player_bailed {
                if self.online {
                    out.push(Relay { run: self.g.run, ev: RelayEv::Bail });
                }
                skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Wipeout(c));
            }
        }
        // Its end, once this console reaches end_run.
        if self.online && ends && self.rt.state == St::EndRun && self.sent_end != self.g.run {
            self.sent_end = self.g.run;
            out.push(Relay { run: self.g.run, ev: RelayEv::End(self.g.run_best) });
        }
        // Another console's run: its events as they come (an early one waits for its run).
        self.g.remote = self.online && !ends;
        // The team's ending console left mid-run and this one took over: a run already over here ends.
        if !self.g.remote && self.g.awaiting_end && self.rt.state == St::PerformRun {
            self.g.awaiting_end = false;
            self.rt.change(St::EndRun);
            skate3_modes::settle(&mut self.g, &mut self.rt);
        }
        if self.online {
            self.inbox.extend(received);
            for (from, r) in std::mem::take(&mut self.inbox) {
                // An event for a run this console hasn't started yet waits for it.
                if r.run > self.g.run || (r.run == self.g.run && matches!(self.rt.state, St::SetupRun | St::Countdown)) {
                    self.inbox.push((from, r));
                    continue;
                }
                if r.run != self.g.run || self.rt.state != St::PerformRun {
                    continue;
                }
                // Only the run's own console speaks for it: no spoofed scores or ends. ponytail: teams unchecked (a team's
                // events and End come from several consoles around a leave); add a per-team owner check if it matters.
                let sender = teams || competitor == Some(from);
                if !sender {
                    continue;
                }
                if let RelayEv::End(best) = r.ev {
                    self.g.remote_end(&mut self.rt, best);
                    skate3_modes::settle(&mut self.g, &mut self.rt);
                    continue;
                }
                // Teams: the sender's own skater; otherwise the run's player.
                let player = if teams { Some(from).filter(|&f| f != self.me && self.g.runs(f)) } else { competitor.filter(|_| !local_run) };
                let Some(c) = player else { continue };
                let msg = if let RelayEv::Score(score) = r.ev { Msg::Objective { player: c, score } } else { Msg::Wipeout(c) };
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

fn begin(world: &mut World, data: &ModeData, id: &str, row: Row, now: f64) -> Play {
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
    info!("Skate 3 1-Up: {id} ({}), {} players, this one {me}", if online { "online" } else { "Party Play" }, names.len());
    let mut g = OneUp::new(row.clone(), f64::from(intro), names.len(), me, first);
    g.party = !online;
    for (c, name) in g.comps.iter_mut().zip(&names) {
        c.name = name.clone();
    }
    // A team lobby's teams (match_core::Match::teams, the host's assignment); Teams.GetName's EA Online team names are
    // gone: retail's unranked "Red Team" / "Blue Team" (online_moves::TEAM_NAMES).
    if online && m.teams.len() == names.len() {
        g.teams_in = m.teams.clone();
    }
    g.team_names = crate::online_moves::TEAM_NAMES.map(|id| data.text(id));
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    skate3_modes::start(&mut g, &mut rt);
    let n = names.len();
    Play { id: id.to_owned(), row, g, rt, names, me, online, hud: HudDriver::default(), inbox: Vec::new(), placed: vec![0; n], intro: None, uncovered: false, sent_end: 0 }
}

fn end(world: &mut World, game: &mut OneUpGame) {
    if let Some(play) = game.play.take() {
        info!("Skate 3 1-Up: {} ended", play.id);
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

    fn row(starts: usize, waits: usize) -> Row {
        let loc = || Some(Locator { matrix: [[0.; 4]; 4], slots: vec![] });
        Row {
            title: "ID_T".into(),
            online: true,
            intro: None,
            time_limit: 20.,
            cumulative_scoring: true,
            allow_player_bail: false,
            team_introduction_delay: 3.,
            start_location_rotation: (0..starts).map(|_| loc()).collect(),
            wait_location_rotation: (0..waits).map(|_| loc()).collect(),
            did_not_beat: "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCOREDLOW".into(),
            name_bailed: "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_BAIL".into(),
            round_over: "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ROUNDOVER".into(),
            set_score: "ID_1UP_SET_A_SCORE".into(),
            your_turn: "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN".into(),
            bailed: "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_BAILED".into(),
            one_upped: "ID_CHALLENGE_CENTRAL_MESSAGE_MODIFIED_1UP_ONEUPPED".into(),
        }
    }

    /// Plans: per run a few sequences, sometimes a bail, rarely a player leaving.
    fn plans(seed: u64, n: usize, me: usize, leaves: bool) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |k: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % k
        };
        (0..400)
            .map(|_| {
                let mut run = Vec::new();
                let mut t = 0.37;
                for _ in 0..next(5) {
                    t += 0.53 + next(60) as f64 / 10.;
                    run.push((t, Ev::Score(100 * next(40) as i64 + 50)));
                }
                if next(4) == 0 {
                    run.push((0.71 + next(150) as f64 / 10., Ev::Bail));
                }
                if leaves && next(25) == 0 {
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
SELF = setmetatable({Name = "1up", Title = "ID_T", TimeLimit = 20, CumulativeScoring = true, AllowPlayerBail = false,
  UseStaticCameras = false, TeamIntroductionDelay = 3,
  StartLocationRotation = {Count = STARTS}, WaitLocationRotation = {Count = WAITS}, CameraLocations = {Count = 0},
  Objectives = {Count = 1, {VisualIndicators = {Count = 0}}}, VisualIndicators = {Count = 0},
  IntroPresentationEvents = {Count = 0},
  CentralMessageHALIDDidNotBeatHighScore = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCOREDLOW",
  CentralMessageHALIDNameBailed = "ID_CHALLENGE_CENTRALMESSAGE_FREESKATEACTIVITY_BAIL",
  CentralMessageHALIDRoundOver = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_ROUNDOVER",
  CentralMessageHALIDSetScore = "ID_1UP_SET_A_SCORE",
  CentralMessageHALIDYourTurn = "ID_CHALLENGE_CENTRAL_MESSAGE_YOURTURN",
  ModifiedCentralMessageHALIDBailed = "ID_CHALLENGE_CENTRAL_MESSAGE_1UP_BAILED",
  ModifiedCentralMessageHALIDOneUpped = "ID_CHALLENGE_CENTRAL_MESSAGE_MODIFIED_1UP_ONEUPPED"},
  -- The rows export no IntroNIS / OnlineOutroNIS (nil: the team NISs are skipped).
  {__index = function(_, k) if k == "IntroNIS" or k == "OnlineOutroNIS" then return nil end return k end})
-- Teams: plan event "member" (v, w): the current team's v-th connected player scores w, or bails (w 0).
function DELIVER(kind, v, w)
  if kind ~= "member" then return false end
  if VARS.PlayerBailed or not VARS.TeamsEnabled then return true end
  local m = {}
  for i = 1, VARS.NumCompetitors do
    local c = VARS.Competitor[i]
    if c.Team == VARS.CurrentTeam and not c.Disconnected then m[#m + 1] = c.ID end
  end
  if #m == 0 then return true end
  local id = m[v % #m + 1]
  if w > 0 then
    SCORE = math.tointeger(w)
    SEND("objectivecomplete", {ObjectiveGroup = 100 + id, ObjectiveIndex = 0})
  else
    SEND("wipeout", {PlayerID = id, IsAPhysicsTeleport = false})
  end
  return true
end
for i = 1, STARTS do SELF.StartLocationRotation[i] = "StartLocationRotation" .. i end
for i = 1, WAITS do SELF.WaitLocationRotation[i] = "WaitLocationRotation" .. i end
function ACTOR()
  if VARS.PlayerBailed or not VARS.CurrentCompetitorIndex then return nil end
  local c = VARS.Competitor[VARS.CurrentCompetitorIndex]
  return c and c.ID
end
"#;

    fn oracle(src: &str, n: usize, me: usize, first: usize, starts: usize, waits: usize, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        team_oracle(src, n, me, first, starts, waits, &[], plans)
    }

    /// The oracle with Online.IsTeamChallenge true and Online.GetTeam answering `teams` (empty: no teams).
    #[allow(clippy::too_many_arguments)]
    fn team_oracle(
        src: &str,
        n: usize,
        me: usize,
        first: usize,
        starts: usize,
        waits: usize,
        teams: &[u8],
        plans: &[Vec<(f64, Ev)>],
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let lua_teams = if teams.is_empty() {
            String::new()
        } else {
            let t: Vec<String> = teams.iter().enumerate().map(|(i, t)| format!("[{i}] = {t}")).collect();
            format!("TEAMS = {{{}}}\n", t.join(", "))
        };
        let setup = format!("N, ME, FIRST, STARTS, WAITS = {n}, {me}, {first}, {starts}, {waits}\n{lua_teams}{SETUP}");
        let (lua, errors) = lua_log(src, &setup, "initial", "perform_run", plans);
        let mut g = OneUp::new(row(starts, waits), 0., n, me, first);
        g.teams_in = teams.to_vec();
        let mut rt = Rt::new(St::Initial, n, 0.);
        let member = |g: &mut OneUp, _: &mut Rt<St>, kind: &str, v: i64, w: f64| {
            if kind != "member" || g.player_bailed || !g.teams_enabled {
                return None;
            }
            let m: Vec<usize> = g.current_members().filter(|&i| !g.comps[i].disconnected).collect();
            let id = *m.get(v as usize % m.len().max(1))?;
            Some(if w > 0. { Msg::Objective { player: id, score: w as i64 } } else { Msg::Wipeout(id) })
        };
        let model = model_log(&mut g, &mut rt, St::PerformRun, plans, |g| if g.player_bailed { None } else { g.competitor() }, |rt| rt.state == St::Complete, member);
        (lua, model, errors)
    }

    /// Team plans: per run a few of the team's players scoring, sometimes one bailing, rarely a player leaving.
    fn team_plans(seed: u64, n: usize, me: usize) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |k: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % k
        };
        (0..400)
            .map(|_| {
                let mut run = Vec::new();
                let mut t = 0.37;
                for _ in 0..next(6) {
                    t += 0.53 + next(50) as f64 / 10.;
                    run.push((t, Ev::Other("member", next(4) as i64, (100 * next(40) + 50) as f64)));
                }
                if next(4) == 0 {
                    run.push((0.71 + next(150) as f64 / 10., Ev::Other("member", next(4) as i64, 0.)));
                }
                if next(20) == 0 {
                    let p = next(n as u64) as usize;
                    if p != me {
                        run.push((0.19 + next(300) as f64 / 10., Ev::Leave(p)));
                    }
                }
                run
            })
            .collect()
    }

    /// 1up.lua's TeamsEnabled path (S3_1UP_DIR): two teams take whole-team runs, a failed 1-Up is a team letter, the
    /// round's winner starts the next, a team with every player gone loses. 2 to 6 players in varied splits (1 v 1 up to
    /// 5 v 1), varied local players, first teams and rotations, players leaving.
    #[test]
    #[ignore = "requires the decompiled 1up.lua (S3_1UP_DIR)"]
    fn skate3_one_up_teams_match_1up_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_1UP_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/1up.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let (me, first) = (seed as usize % n, (seed as usize * 7) % 11);
                let (starts, waits) = (1 + seed as usize % 3, 1 + (seed as usize / 3) % 3);
                // Team 2 gets 1 + seed % (n - 1) players, spread by the seed.
                let size2 = 1 + seed as usize % (n - 1);
                let mut teams = vec![1u8; n];
                for k in 0..size2 {
                    teams[(k * 2 + seed as usize) % n] = 2;
                }
                if !teams.contains(&1) {
                    teams[0] = 1;
                }
                let plans = team_plans(seed * 977 + n as u64, n, me);
                let (lua, model, errs) = team_oracle(&src, n, me, first, starts, waits, &teams, &plans);
                compare(&format!("teams {teams:?} seed {seed} n {n} me {me} first {first}"), &lua, &model, &errs);
                assert!(lua.iter().any(|l| l.contains("teamcentral")), "the team path ran");
                games += 1;
                lines += lua.len();
                errors += errs.len();
            }
        }
        eprintln!("1up.lua team oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// 1up.lua itself (S3_1UP_DIR = tools/skate3_online.py's `--out`/1up, with scripts/) decides every game the model
    /// plays the same way: every state, timer, fade, move, pin, camera, HUD2 call, letter and result; 2 to 6 players,
    /// varied first players, local players and location rotations, with players leaving mid-game.
    #[test]
    #[ignore = "requires the decompiled 1up.lua (S3_1UP_DIR)"]
    fn skate3_one_up_matches_1up_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_1UP_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/1up.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let (me, first) = (seed as usize % n, (seed as usize * 7) % 11);
                let (starts, waits) = (1 + seed as usize % 3, 1 + (seed as usize / 3) % 3);
                let plans = plans(seed * 131 + n as u64, n, me, seed % 3 == 0);
                let (mut lua, mut model, errs) = oracle(&src, n, me, first, starts, waits, &plans);
                // B2 (10-12, not retail): where end_run's timer stops on the nil playerIndex, the model goes on.
                if errs.first().is_some_and(|e| e.starts_with("end_run timerexpired") && e.contains("'playerIndex'"))
                    && let Some(i) = lua.iter().position(|l| l.ends_with(" error"))
                {
                    assert!(model.len() > i && !model[i].ends_with(" error"), "seed {seed} n {n}: the model goes on past B2");
                    lua.truncate(i);
                    model.truncate(i);
                }
                compare(&format!("seed {seed} n {n} me {me} first {first}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("result")) || !errs.is_empty() || lua.iter().any(|l| l.contains("left")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("1up.lua oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// The rules by hand: the first run sets the score, a lower one takes a letter and sits out, the round winner
    /// starts the next round, three letters eliminate, the winner is the one with the fewest letters.
    #[test]
    fn the_round_goes_to_the_last_one_left() {
        let mut g = OneUp::new(row(3, 1), 0., 2, 0, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        let mut t = 0.;
        let mut until = |g: &mut OneUp, rt: &mut Rt<St>, state: St| {
            for _ in 0..1000 {
                if rt.state == state {
                    return;
                }
                t = rt.due().unwrap();
                skate3_modes::advance(g, rt, t);
            }
            panic!("never reached {state:?}");
        };
        until(&mut g, &mut rt, St::PerformRun);
        assert_eq!(g.competitor(), Some(0), "the drawn first player (first 0: NonEliminatedPlayers[1])");
        assert_eq!(rt.places[0].map(|p| p.1), Some(Place { field: START, index: 1, slot: 0 }));
        assert_eq!(rt.places[1].map(|p| p.1), Some(Place { field: WAIT, index: 1, slot: 1 }));
        assert!(!rt.pinned[0] && rt.pinned[1]);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 0, score: 500 });
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 0, score: 700 });
        assert_eq!(g.run_best, 1200, "CumulativeScoring adds the sequences");
        until(&mut g, &mut rt, St::EndRun);
        assert_eq!((g.round_best, g.round_has_leader), (1200, true));
        until(&mut g, &mut rt, St::PerformRun);
        assert_eq!(g.competitor(), Some(1));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Wipeout(1));
        assert!(g.player_bailed);
        until(&mut g, &mut rt, St::EndRun);
        until(&mut g, &mut rt, St::PerformRun);
        assert_eq!((g.comps[1].letters, g.competitor()), (1, Some(0)), "a letter; the round winner starts the next round");
        assert_eq!(rt.places[0].map(|p| p.1.index), Some(2), "the next start locator");
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.contains("central ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORESET 2 1200")));
        assert!(log.iter().any(|l| l.contains("letters 1 false false")));
    }

    /// A relayed i64::MAX twice in a cumulative run saturates the run's score (no overflow panic).
    #[test]
    fn huge_relayed_scores_saturate() {
        let mut g = OneUp::new(row(3, 1), 0., 2, 0, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::PerformRun {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        for _ in 0..2 {
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 0, score: i64::MAX });
        }
        assert_eq!(g.run_best, i64::MAX);
    }

    /// Teams by hand (2 v 2, this console on team 2): the whole team skates the run, the other team's player at this
    /// player's place in its own team is watched, a failed 1-Up is a letter for the whole team.
    #[test]
    fn a_team_skates_together_and_shares_its_letters() {
        let mut g = OneUp::new(row(3, 1), 0., 4, 1, 0);
        g.teams_in = vec![1, 2, 1, 2];
        let mut rt = Rt::new(St::Initial, 4, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        let until = |g: &mut OneUp, rt: &mut Rt<St>, state: St| {
            for _ in 0..1000 {
                if rt.state == state {
                    return;
                }
                let t = rt.due().unwrap();
                skate3_modes::advance(g, rt, t);
            }
            panic!("never reached {state:?}");
        };
        until(&mut g, &mut rt, St::PerformRun);
        assert_eq!(g.current_team, 1, "Random.GetRandomNumber(2) with first 0");
        assert_eq!(rt.pinned, [false, true, false, true], "team 1 skates");
        assert_eq!(rt.places[2].map(|p| p.1), Some(Place { field: START, index: 1, slot: 1 }), "the team's second start slot");
        assert_eq!(rt.watching, 0, "team 2's first player watches team 1's first");
        assert!(g.runs(2) && !g.runs(1));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 2, score: 800 });
        until(&mut g, &mut rt, St::EndRun);
        until(&mut g, &mut rt, St::PerformRun);
        assert_eq!((g.current_team, rt.watching), (2, 1), "team 2's turn: this player skates");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Wipeout(3));
        assert!(g.player_bailed, "a teammate's bail ends the team's run");
        until(&mut g, &mut rt, St::SetupRun);
        assert_eq!((g.team_data[1].letters, g.comps[1].letters, g.comps[3].letters), (1, 1, 1));
        assert_eq!(g.current_team, 1, "the round's winner starts the next");
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("teamcentral ID_CHALLENGE_CENTRAL_MESSAGE_1UP_SCORESET 0 2 800")), "{log:#?}");
        assert!(log.iter().any(|l| l.ends_with("letters 1 false true")));
        assert!(log.iter().any(|l| l.ends_with("board 1 4 teams")));
    }

    /// Plays a Party Play game to its results dialog: each card answered (A); player 1 sets 1200 every round, player 2
    /// bails every run. Returns the cards (player 1-based, round).
    fn party_game(g: &mut OneUp, rt: &mut Rt<St>) -> Vec<(usize, i64)> {
        let (mut cards, mut ran) = (Vec::new(), None);
        for _ in 0..5000 {
            if rt.dialog {
                return cards;
            }
            if let Some(card) = rt.card.take() {
                assert_eq!(rt.controlled + 1, card.0, "the pad is the carded player's");
                assert!(!rt.pause_disabled, "Start opens the Party Play crossbar (Exit Party Play) on every run");
                cards.push(card);
                skate3_modes::dispatch(g, rt, Msg::HudComplete);
                continue;
            }
            if rt.state == St::PerformRun && let Some(c) = g.competitor().filter(|&c| ran != Some((cards.len(), c))) {
                ran = Some((cards.len(), c));
                let msg = if c == 0 { Msg::Objective { player: 0, score: 1200 } } else { Msg::Wipeout(1) };
                skate3_modes::dispatch(g, rt, msg);
                continue;
            }
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}", rt.state));
            skate3_modes::advance(g, rt, t);
        }
        panic!("no results dialog");
    }

    /// D6, Party Play by hand: 1-UP with the party conventions. Every run's player gets the pad behind its Press-A card
    /// (the round counted for it), the results dialog waits, Rematch plays again from no letters and Quit ends it.
    #[test]
    fn party_play_cards_every_run_and_the_dialog_rematches_or_quits() {
        let mut g = OneUp::new(row(3, 1), 0., 2, 0, 0);
        g.party = true;
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.online = false;
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        let cards = party_game(&mut g, &mut rt);
        assert_eq!(cards, [(1, 1), (2, 1), (1, 2), (2, 2), (1, 3), (2, 3)]);
        assert_eq!(g.comps[1].letters, 3);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::HudRetry);
        assert!(!rt.dialog);
        let cards = party_game(&mut g, &mut rt);
        assert_eq!(cards.first(), Some(&(1, 1)), "a fresh game");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::HudContinue);
        for _ in 0..100 {
            if rt.state == St::Complete {
                break;
            }
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}", rt.state));
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert_eq!(rt.state, St::Complete);
    }

    /// Matrix Step 2 (10-12): Party Play at its retail max (4, party_play page 0), from every first player. By hand
    /// (1237-1386, NumLettersBestScoreSorter): every run lands 1000 (p + 1) then bails; player 4 (4000) is the last one
    /// left of every round, the others take a letter a round and go out in round 3. Every run is carded to its player.
    #[test]
    fn party_play_holds_four_players() {
        for first in 0..4 {
            let mut g = OneUp::new(row(3, 3), 0., 4, 0, first);
            g.party = true;
            let mut rt = Rt::new(St::Initial, 4, 0.);
            rt.online = false;
            skate3_modes::start(&mut g, &mut rt);
            let (mut cards, mut ran) = (Vec::new(), None);
            for _ in 0..20000 {
                if rt.dialog {
                    break;
                }
                if let Some(card) = rt.card.take() {
                    assert_eq!(rt.controlled + 1, card.0, "first {first}: the pad is the carded player's");
                    cards.push(card);
                    skate3_modes::dispatch(&mut g, &mut rt, Msg::HudComplete);
                    continue;
                }
                if rt.state == St::PerformRun && let Some(c) = g.competitor().filter(|&c| ran != Some((cards.len(), c))) {
                    ran = Some((cards.len(), c));
                    skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: c, score: 1000 * (c as i64 + 1) });
                    skate3_modes::dispatch(&mut g, &mut rt, Msg::Wipeout(c));
                    continue;
                }
                let t = rt.due().unwrap_or_else(|| panic!("first {first}: stuck in {:?}", rt.state));
                skate3_modes::advance(&mut g, &mut rt, t);
            }
            assert!(rt.dialog, "first {first}: the results dialog");
            for round in 1..=3 {
                let mut carded: Vec<usize> = cards.iter().filter(|c| c.1 == round).map(|c| c.0).collect();
                carded.sort_unstable();
                carded.dedup();
                assert_eq!(carded, [1, 2, 3, 4], "first {first}: round {round}'s cards {cards:?}");
            }
            let standings: Vec<(usize, i64, i64)> = g.standings.iter().map(|s| (s.player, s.letters, s.best)).collect();
            assert_eq!(standings, [(4, 0, 4000), (3, 3, 3000), (2, 3, 2000), (1, 3, 1000)], "first {first}");
        }
    }

    /// I4: Party Play has no "you" on its one pad: Player 1's bail is named and white like everyone's.
    #[test]
    fn party_play_names_player_1_like_the_others() {
        let r = row(3, 1);
        let mut g = OneUp::new(r.clone(), 0., 2, 0, 0);
        g.party = true;
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.online = false;
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        for _ in 0..500 {
            if rt.card.take().is_some() {
                skate3_modes::dispatch(&mut g, &mut rt, Msg::HudComplete);
            } else if rt.state == St::PerformRun {
                break;
            } else {
                let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}", rt.state));
                skate3_modes::advance(&mut g, &mut rt, t);
            }
        }
        assert_eq!(g.competitor(), Some(0));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Wipeout(0));
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.contains(&r.name_bailed)), "{log:?}");
        assert!(!log.iter().any(|l| l.contains(&r.bailed)), "no \"you bailed\"");
    }

    /// Matrix Step 2 (10-12): `n` consoles play 1-Up through the match relay ([`skate3_modes::tests::Wire`]), started up
    /// to 0.3 s apart, each through [`Play::exchange`] as the glue runs it. Every run's player lands 1000 (p + 1) 1 s
    /// in and bails at 2 s (the run ends 3 s later, 1187-1235). `leaver` (player, own) quits once its console is in
    /// round 2 and another player's run, or (own) its own run after it landed (what it sent still on the way is lost);
    /// the others notice it 0.5 s + 0.4 s per slot later and get it once the rest have sent on its events (match_core
    /// Leaves). Every staying console's standings (player, letters, best).
    fn one_up_over_the_wire(n: usize, seed: u64, latency: (f64, f64), loss: f64, leaver: Option<(usize, bool)>) -> Vec<Vec<(usize, i64, i64)>> {
        one_up_over_the_wire_teams(n, seed, latency, loss, leaver, None, &[]).into_iter().map(|c| c.0).collect()
    }
    /// [`one_up_over_the_wire`] with `teams` (each player's team, 1 or 2): the current team's members all land at 1 s,
    /// its run owner bails at 2 s; (own) a leaver quits in its team's run. `last` (score, delay): the leaver's own
    /// round-2 run lands `score` instead and it quits `delay` s after (the landing on the way to some consoles, not
    /// others). Every staying console's team (letters, rank) too.
    #[allow(clippy::type_complexity)]
    fn one_up_over_the_wire_teams(
        n: usize,
        seed: u64,
        latency: (f64, f64),
        loss: f64,
        leaver: Option<(usize, bool)>,
        last: Option<(i64, f64)>,
        teams: &[u8],
    ) -> Vec<(Vec<(usize, i64, i64)>, Vec<(i64, i64)>)> {
        // A run's best landing (one landing per skater: the same as cumulative for one skater; teams: the best member).
        let r = Row { cumulative_scoring: false, ..row(3, 3) };
        let mut wire = skate3_modes::tests::Wire::new(seed, latency, loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        let mut runs: Vec<Option<(u32, f64, u8)>> = vec![None; n];
        let mut left_at: Option<f64> = None;
        for frame in 0..60 * 1500 {
            let t = frame as f64 / 60.;
            for i in 0..n {
                let quit = leaver.is_some_and(|l| l.0 == i);
                if t < starts[i] || (quit && left_at.is_some()) {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| {
                    let mut g = OneUp::new(r.clone(), 0., n, i, seed as usize);
                    g.teams_in = teams.to_vec();
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    skate3_modes::start(&mut g, &mut rt);
                    let names = (0..n).map(|p| format!("P{p}")).collect();
                    Play { id: "1".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), inbox: Vec::new(), placed: vec![0; n], intro: None, uncovered: false, sent_end: 0 }
                });
                let running = if play.g.teams_enabled { play.g.runs(i) } else { play.g.competitor() == Some(i) };
                let own = runs[i].is_some_and(|r| r.0 == play.g.run && r.2 == 1 && t - r.1 >= 1. + last.map_or(0., |l| l.1)) && running;
                let others = !running && (play.g.teams_enabled || play.g.competitor().is_some());
                if quit && play.g.round >= 2 && play.rt.state == St::PerformRun && (if leaver.unwrap().1 { own } else { others }) {
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
                skate3_modes::advance(&mut play.g, &mut play.rt, t);
                // This console's run: 1000 (p + 1) at 1 s, a bail at 2 s.
                let (mut landed, mut bailed) = (Vec::new(), false);
                let running = if play.g.teams_enabled { play.g.runs(i) } else { play.g.competitor() == Some(i) };
                let bails = !play.g.teams_enabled || play.g.run_owner() == Some(i);
                if play.rt.state == St::PerformRun && running {
                    let mut run = runs[i].filter(|r| r.0 == play.g.run).unwrap_or((play.g.run, t, 0));
                    if run.2 == 0 && t - run.1 >= 1. {
                        let score = last.filter(|_| quit && play.g.round >= 2).map_or(1000 * (i as i64 + 1), |l| l.0);
                        (landed, run.2) = (vec![score], 1);
                    } else if bails && run.2 == 1 && t - run.1 >= 2. {
                        (bailed, run.2) = (true, 2);
                    }
                    runs[i] = Some(run);
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                let out = play.exchange(landed, bailed, received);
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
                (p.g.standings.iter().map(|s| (s.player - 1, s.letters, s.best)).collect(), p.g.team_data.iter().map(|t| (t.letters, t.rank)).collect())
            })
            .collect()
    }

    /// Matrix G2 (10-12): six players, 150-300 ms, 5-10 % loss, from every first player the draw can give. By hand
    /// (1237-1386, 1464-1491, NumLettersBestScoreSorter 467-503): player 5 (6000) is the last one left of every
    /// round, the others take a letter a round and go out in round 3; standings by letters, then best. The host
    /// (slot 0) leaving in round 2 (in another's run, or its own: special_disconnected ends it) takes 3 letters
    /// (playerdisconnected) and keeps its 1000: still last. Player 5 leaving in round 2 (the round's leader in another's
    /// run: the round restarts, 1561; or in its own first run) leaves player 4 the last one left: it wins on 1 letter,
    /// and the leaver, 3 letters with the best 6000, ranks second (the sorter only reads letters and best: retail).
    #[test]
    fn six_players_and_a_leaver_end_one_up_with_the_retail_standings() {
        for seed in 0..6u64 {
            let all = one_up_over_the_wire(6, seed, (0.15, 0.3), 0.05 + 0.01 * seed as f64, None);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            assert_eq!(all[0].iter().map(|s| s.0).collect::<Vec<_>>(), [5, 4, 3, 2, 1, 0], "seed {seed}: {:?}", all[0]);
            assert_eq!(all[0][0].1, 0, "seed {seed}: the winner took no letter");
            assert!(all[0][1..].iter().all(|s| s.1 == 3 && s.2 == 1000 * (s.0 as i64 + 1)), "seed {seed}: {:?}", all[0]);
        }
        for seed in 0..8u64 {
            let (quit, want) = if seed % 2 == 0 { (0, [5, 4, 3, 2, 1, 0]) } else { (5, [4, 5, 3, 2, 1, 0]) };
            let all = one_up_over_the_wire(6, seed, (0.15, 0.3), 0.05 + 0.01 * (seed % 6) as f64, Some((quit, seed % 4 >= 2)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same after player {quit} left");
            }
            // ponytail: a leave in the leaver's own run (special_disconnected: the round restarts on the Lua's own
            // PlayersInRound mix-up, 1561) is checked for agreement and the leaver's 3 letters only; its order is the
            // oracle-checked port's, not derived by hand.
            if seed % 4 >= 2 {
                assert!(all[0].iter().any(|s| s.0 == quit && s.1 == 3), "seed {seed}: {:?}", all[0]);
                continue;
            }
            assert_eq!(all[0].iter().map(|s| s.0).collect::<Vec<_>>(), want, "seed {seed}: {:?}", all[0]);
            assert!(all[0][1..].iter().all(|s| s.1 == 3), "seed {seed}: {:?}", all[0]);
        }
    }

    /// Matrix Step 2 (10-12): team 1-Up at 6 (3 a side, teams 1 2 1 2 1 2), 150-300 ms, 5-10 % loss, no leaver, a guest
    /// (3) or the host (0, its team's run owner) leaving in round 2, in the other team's run or its own. By hand
    /// (1200-1212, 1321-1326, 480-490): a team's run is its best member's, team 1 5000 against team 2 6000; a letter ends
    /// the round, so team 1 takes one a round and is out after 3: team 1 (3 letters, rank 2), team 2 (0, rank 1). Neither
    /// leaver is its team's best.
    #[test]
    fn six_players_in_two_teams_end_one_up_as_retail() {
        let teams = [1u8, 2, 1, 2, 1, 2];
        for seed in 0..6u64 {
            let leaver = match seed % 3 {
                0 => None,
                1 => Some((3, seed >= 3)),
                _ => Some((0, seed >= 3)),
            };
            let all = one_up_over_the_wire_teams(6, seed, (0.15, 0.3), 0.05 + 0.01 * seed as f64, leaver, None, &teams);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} leaver {leaver:?}: every console the same");
            }
            assert_eq!(all[0].1, [(3, 2), (0, 1)], "seed {seed} leaver {leaver:?}: the teams (letters, rank); players {:?}", all[0].0);
        }
    }

    /// End review I2 (10-12): the host (0) or player 5 lands 9000 in its own round-2 run and leaves 0.2 s later, the
    /// landing on the way to some consoles only (or none). By hand (R5: a leaver takes 3 letters and keeps its best;
    /// NumLettersBestScoreSorter 467-503, letters then best): the 9000 counts on every console if any took it (the
    /// leaver then first of the 3-letter players), else its round-1 1000 / 6000 on every one. Before the fix the
    /// consoles it missed kept its 1000 / 6000 and placed it apart.
    #[test]
    fn a_landing_sent_as_its_player_leaves_counts_on_every_console() {
        let mut counted = 0;
        for seed in 0..8u64 {
            let quit = if seed % 2 == 0 { 0 } else { 5 };
            let all = one_up_over_the_wire_teams(6, seed + 30, (0.15, 0.3), 0.05 + 0.01 * (seed % 6) as f64, Some((quit, true)), Some((9000, 0.2)), &[]);
            let all: Vec<Vec<(usize, i64, i64)>> = all.into_iter().map(|c| c.0).collect();
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same after player {quit} left");
            }
            let s = &all[0];
            let before = 1000 * (quit as i64 + 1);
            assert!(s.contains(&(quit, 3, 9000)) || s.contains(&(quit, 3, before)), "seed {seed}: the leaver's 3 letters and best: {s:?}");
            assert!(s.windows(2).all(|w| (w[0].1, -w[0].2) <= (w[1].1, -w[1].2)), "seed {seed}: letters, then best: {s:?}");
            counted += usize::from(s.contains(&(quit, 3, 9000)));
        }
        assert!(counted > 0, "no console ever took the landing: nothing tested");
    }

    #[test]
    fn standings_sort_as_lua_sorts_them() {
        let mut g = OneUp::new(row(1, 1), 0., 4, 0, 0);
        for (c, (l, b)) in g.comps.iter_mut().zip([(3, 100), (1, 50), (1, 900), (3, 100)]) {
            (c.letters, c.best) = (l, b);
        }
        g.calculate_scores();
        assert_eq!(g.standings.iter().map(|s| s.player).collect::<Vec<_>>()[..2], [3, 2]);
    }

    /// The tool's output (S3_1UP_DIR; its sibling hud/) and the install's online-ui (S3_ONLINE_UI): the online rows load
    /// with their rotations, intros and titles, and the HUD movies run on the answers: the DPO, the countdown, the
    /// leaderboard, the 1-UP letters and the results.
    #[test]
    #[ignore = "requires the Skate 3 1-Up data (S3_1UP_DIR, S3_ONLINE_UI)"]
    fn skate3_one_up_data_and_movies() {
        use crate::apt_screen::Screen;
        use serde_json::json;
        let root = std::path::PathBuf::from(std::env::var("S3_1UP_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id)).cloned().collect();
        // 10-06 data: + 1up_artg_01, 1up_dark_01, 1up_dwmc_01, 1up_mmcn_01 (the DLC rows); 1up_bbpk_01 (BlackBoxPark,
        // refused before by map_of).
        assert_eq!(playable.len(), 15, "{playable:?}");
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(data.intros.contains_key(r.intro.as_deref().unwrap()), "{id}: no intro");
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            // 1up_dwmc_01's locators (DLC_DW_MegaCompund's world stream) have 3 children each in the retail data; its
            // 4th-6th players wait past them (online_map::locator_slot, 0x82910eb0's 5 m steps).
            let slots = if id == "1up_dwmc_01" { 3 } else { 6 };
            assert!(r.wait_location_rotation.iter().all(|w| w.as_ref().unwrap().slots.len() == slots), "{id}: {slots} waiting slots");
        }
        let hud = root.join("../hud/runtime");
        let texts = |s: &Screen| -> Vec<String> {
            (s.vm.objects.iter())
                .filter_map(|o| match o.fields.get("_displayText") {
                    Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                    _ => None,
                })
                .collect()
        };
        let mut dpo = Screen::load(&hud.join("dpo.json"), &[]).unwrap();
        let title = skate3_modes::Text { id: "ID_DPO_STR_POINTS".into(), params: vec!["2500".into()] };
        dpo.answer("HUDComponents.DPO_GetParams", json!(["1,200", data.fmt(&title), "#P1", "", 1, "", "", "", "", 1]));
        dpo.answer("HUDComponents.DPO_IsUsingTwoDPOs", json!(false));
        dpo.call("SetState", &[json!(1)]).unwrap();
        dpo.call("SetState", &[json!(3)]).unwrap();
        for _ in 0..30 {
            dpo.advance().unwrap();
        }
        let shown = texts(&dpo);
        assert!(shown.iter().any(|s| s == "1,200") && shown.iter().any(|s| s == "P1"), "{shown:?}");

        let mut count = Screen::load(&hud.join("hudcountdown.json"), &[]).unwrap();
        count.answer("HUDComponents.Countdown_IsInStadium", json!(false));
        count.answer("FELanguage.GetLanguage", json!("English"));
        count.answer("HUDComponents.Countdown_GetCountdownTime", json!("#3"));
        count.call("SetState", &[json!(2)]).unwrap();
        for _ in 0..10 {
            count.advance().unwrap();
        }
        assert!(count.bindings.sounds.iter().any(|s| s.contains("count") || s.contains("COUNT")), "{:?}", count.bindings.sounds);

        let mut letters = Screen::load(&hud.join("hudthrowmes.json"), &[]).unwrap();
        letters.call("SetState", &[json!(6)]).unwrap();
        for _ in 0..60 {
            letters.advance().unwrap();
        }
    }
}
