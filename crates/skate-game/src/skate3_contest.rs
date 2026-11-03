//! Skate 3's Street and Vert contests on the match core (challenge graph `contest_shared` < contest < default), the
//! online (non-team, no online AI) path, for the online rows of DownTown, Industrial and University. Data:
//! tools/skate3_online.py, installed at `private/skate3-online/contest/`: rules.json (per row its CLD fields and
//! ContestRounds: variant, run type, scoring, times, StartLocation with its 6 slots, spot volumes, round boundary, AI
//! out-of-bounds volumes, results columns, the round's intro NIS), strings.json, scripts/contest_shared.lua (the
//! decompiled graph: the rules' source and the tests' oracle), the round cameras (tutorial.json + scene/camera).
//!
//! [`Contest`] is contest_shared handler by handler (line numbers are contest_shared.lua's): every skater skates the
//! round at once. setup_round (520-778) sends this console's skater to StartLocation slot = its id and syncs; the
//! round's intro presentation plays (802-805), the loading screen closes and the screen fades in (781-787); the round
//! message and the 3-2-1 (831-880); the round runs for RoundTime (RoundTimeOnline when set, 911-921). Sequences score
//! under the round's variant (1087-1179; Grab / Grind need that trick in the sequence; out of the boundary or away
//! from the spot volumes scores 0), airs (1399-1429) and grinds (1321-1334) under theirs; UpdateScore (200-388) keeps
//! the best or the sum (CumulativeScoring) and reorders the leaderboard. Leaving the boundary shows Return To
//! Challenge Area and sends the skater back to the start after AudioOutOfBoundsWarningTime + 3 s (1289-1301, 1077-1083).
//! end_round (1460-1634) ranks the round and awards NumCompetitors - place points (ties share), the round results show
//! for 6 + 1 s (2230-2436); after the last round the standings decide the win and the results are reported (2157-2114).
//!
//! Team rows (the Online Challenge Map's Team tab, `match_core::team`) take the TeamsEnabled path (Online.IsTeamChallenge,
//! Online.GetTeam = the host's split, `Match::teams`): every member's score adds to the team's round score, the team
//! ahead takes the round point (a tie: none), the team with more round points wins (1504-1555, 2047-2087).
//!
//! Each console skates its own skater and relays what it did (`Match::outbox`); the others' come from the relay.
//! ponytail: retail replicates every skater and raises their messages on each console. Solo (`SKATE_MODE=contest:<row>`)
//! is a one-skater contest on the online rules.
use crate::{
    challenge::cutscene::Cutscene,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate3_modes::{
        self, Abort, ChallengeFade, Component, Effect, EndSync, Graph, Handled, HudDriver, Locator, ModeData, Msg, Place, RED,
        ResultsSetup, Rt, Text, View, WHITE,
    },
    skate3_race::RaceVolume,
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// HUD2.eLeaderboardHUDMenuType (leaderboardhud: eLeaderBoardHUD_RankNameScore 3, RankNameFloatscore 6).
const RANK_NAME_SCORE: i64 = 3;
const RANK_NAME_FLOATSCORE: i64 = 6;
/// FrontEnd.eResultsLBType (endchallenge_online: BestTrickNonElimFloat 4, BestTrickNonElimInt 5).
const NON_ELIM_FLOAT: i64 = 4;
const NON_ELIM_INT: i64 = 5;
/// eResultsLBType_TeamChallengeResults 8: the one team layout endchallenge_online's FillTable draws ("#" + place,
/// name, int).
const TEAM_RESULTS: i64 = 8;
/// HUD2.eTeamMessageType.Normal (0x8284fb70).
const TEAM_NORMAL: u8 = 0;
/// HUD2.eDPOFormat.Points (0x8284ee90).
const DPO_POINTS: i64 = 1;
/// Challenge.eChallengeTypes (challenge_global_data ChallengeType: street 8, vert 15).
const STREET: u32 = 8;
const VERT: u32 = 15;
const START: &str = "StartLocation";
/// Contest.eContestType (0x8284c8a8 registers Sequence 0 .. Grind_Only 16, Unknown 17).
mod variant {
    pub const SEQUENCE: u32 = 0;
    pub const GRAB: u32 = 1;
    pub const GRIND: u32 = 2;
    pub const FLIP: u32 = 3;
    pub const FOOTPLANT: u32 = 4;
    pub const HANDPLANT: u32 = 5;
    pub const HIPPYJUMP: u32 = 6;
    pub const GRIND_LENGTH: u32 = 7;
    pub const AIR_DISTANCE: u32 = 9;
    pub const AIR_GRAB_TIME: u32 = 10;
    pub const AIR_HEIGHT: u32 = 11;
    pub const AIR_SPIN: u32 = 12;
    pub const AIR_TIME: u32 = 13;
    pub const BEST_LINE: u32 = 14;
    pub const AIR_ONLY: u32 = 15;
    pub const GRIND_ONLY: u32 = 16;
}
use variant::*;
/// Contest.eRunType (0x8284cfa0: None 0, SoloRun 1, SoloRunCycle 2, TimedRunSingle 3, TimedRunMulti 4).
const SOLO_RUN: u32 = 1;
const SOLO_RUN_CYCLE: u32 = 2;
const TIMED_RUN_SINGLE: u32 = 3;
const TIMED_RUN_MULTI: u32 = 4;
/// HUD2.LeaderboardHUDSetVariantType's strings by eContestType (0x828519e8: 0x83025d30, or 0x83025d78 for a round
/// that is not "best": its _JAM strings).
const VARIANT_STRINGS: [&str; 17] = [
    "SEQUENCE", "SEQUENCE_GRAB", "SEQUENCE_GRIND", "SEQUENCE_FLIP", "SEQUENCE_FOOTPLANT", "SEQUENCE_HANDPLANT",
    "SEQUENCE_HIPPYJUMP", "GRIND_LENGTH", "SPEED", "AIR_DISTANCE", "AIR_GRAB_TIME", "AIR_HEIGHT", "AIR_SPIN", "AIR_TIME",
    "AIR_LINE", "AIR_ONLY", "GRIND_ONLY",
];
/// The trick types a sequence held (Trick.GetNumTrickType > 0), as relayed.
pub(crate) const T_GRAB: u32 = 1;
pub(crate) const T_GRIND: u32 = 2;
pub(crate) const T_FOOTPLANT: u32 = 4;
pub(crate) const T_HANDPLANT: u32 = 8;
pub(crate) const T_HIPPYJUMP: u32 = 16;
/// contest_shared's timers (seconds): wait_for_assetload (1866), the load delay (826), the intro's post-NIS (783), the
/// fade in (784), the countdown (23), the round's fade out (1671) and its settle (1685), the results (2412, 2432).
const WAIT_PRES: f64 = 0.5;
const LOAD_DELAY: f64 = 0.5;
const POST_NIS: f64 = 1.5;
const FADE_IN: f64 = 1.;
const COUNTDOWN: f64 = 3.;
const FADE_OUT: f64 = 0.3;
const FADE_SETTLE: f64 = 0.2;
const RESULTS: f64 = 6.;
const RESULTS_POPPED: f64 = 1.;
/// HighlightReel.AddHighlightUsingDefinition's handle (any id but -1: the graph only tests for -1).
const HIGHLIGHT: i64 = 5;

// ---------------------------------------------------------------- data
/// tContestRound (rules.json `rounds`).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Round {
    pub contest_variant: u32,
    #[serde(default)]
    pub run_type: u32,
    pub cumulative_scoring: bool,
    #[serde(default)]
    pub cumulative_min_score: i64,
    pub round_time: i64,
    #[serde(default)]
    pub round_time_online: Option<i64>,
    pub start_location: Option<Locator>,
    #[serde(default)]
    pub spot_volumes: Vec<Option<RaceVolume>>,
    #[serde(default)]
    pub round_boundary: Option<RaceVolume>,
    #[serde(default)]
    pub path_end_volume: Option<RaceVolume>,
    #[serde(default)]
    pub restart_last_chance_volume: Vec<Option<RaceVolume>>,
    #[serde(rename = "AIOutOfBoundsVolumes", default)]
    pub ai_out_of_bounds: Option<Vec<Option<RaceVolume>>>,
    pub audio_out_of_bounds_warning_time: i64,
    pub audio_round_end_warning_time: i64,
    #[serde(rename = "ResultsScreenHALIDCol1")]
    pub col1: String,
    #[serde(rename = "ResultsScreenHALIDCol2")]
    pub col2: String,
    #[serde(rename = "ResultsScreenHALIDCol3")]
    pub col3: String,
    /// The round's intro NIS (its IntroPresentationEvents' kind 5), None where the scene has no camera.
    #[serde(rename = "intro", default)]
    pub intro: Option<String>,
    /// G7: the round's VisualIndicators with their look (tools/skate3_online.py contest_rounds; none in an older export).
    #[serde(rename = "visual_indicators", default)]
    pub visual_indicators: Vec<crate::skate3_race::Chevron>,
}
impl Round {
    fn use_int(&self) -> bool {
        // setup_round 698-702.
        matches!(self.contest_variant, SEQUENCE..=HIPPYJUMP | BEST_LINE | AIR_SPIN | AIR_ONLY | GRIND_ONLY)
    }
    fn air(&self) -> bool {
        matches!(self.contest_variant, AIR_DISTANCE..=AIR_TIME)
    }
    fn sequence(&self) -> bool {
        // 682-685: Sequence, Best_Line, the trick-type and flip contests, Air_Only, Grind_Only.
        matches!(self.contest_variant, SEQUENCE..=HIPPYJUMP | BEST_LINE | AIR_ONLY | GRIND_ONLY)
    }
    fn name(v: &Option<RaceVolume>) -> Option<&str> {
        v.as_ref().map(|v| v.name.as_str())
    }
}

/// A row's CLD fields (rules.json).
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub title: String,
    pub online: bool,
    #[serde(default)]
    pub challenge_type: Option<u32>,
    #[serde(rename = "ContestElimination", default)]
    pub elimination: Option<bool>,
    #[serde(rename = "AIOnline", default)]
    pub ai_online: Option<bool>,
    #[serde(rename = "CentralMessageHALIDFinalRound")]
    pub final_round: String,
    #[serde(rename = "CentralMessageHALIDRoundNum")]
    pub round_num: String,
    #[serde(rename = "CentralMessageHALIDReturnToChallengeArea")]
    pub return_to_area: String,
    /// Not in the street / vert templates' chains: nil in retail.
    #[serde(rename = "CentralMessageHALIDNewLead", default)]
    pub new_lead: Option<String>,
    #[serde(rename = "CentralMessageHALIDNewLeadYou", default)]
    pub new_lead_you: Option<String>,
    /// The team rows' first countdown: the team message (ID_CHALLENGE_CENTRAL_MESSAGE_TEAMINDICATION on every online
    /// row) for TeamIntroductionDelay (3 s), then the 3-2-1 (832-834, 884-886).
    #[serde(rename = "TeamIntroductionDelay", default)]
    pub team_intro_delay: f64,
    #[serde(rename = "CentralMessageHALIDTeamIndication", default)]
    pub team_indication: String,
    /// The team NISs: IntroNIS / OnlineIntroNIS (nis_with_characters, both teams' skaters) and OnlineOutroNIS (the
    /// winners' celebration). The disc's rows name them (dom_intro, Online_Celeb_0n); not exported: the NIS player
    /// with dynamic characters is not ported (None).
    #[serde(rename = "IntroNIS", default)]
    pub intro_nis: Option<String>,
    #[serde(rename = "OnlineIntroNIS", default)]
    pub online_intro_nis: Option<String>,
    #[serde(rename = "OnlineOutroNIS", default)]
    pub online_outro_nis: Option<String>,
    #[serde(default)]
    pub rounds: Vec<Round>,
}
impl Row {
    /// Playable: online, a street or vert contest without elimination or online AI (none on the disc), every round
    /// with its 6 start slots and its volumes placed, of a variant the online rows use (Sequence, Grab, Grind,
    /// Grind_Length, Air_Distance, Air_Height, Air_Only) and without RestartLastChanceVolumes (none online; a
    /// PathEndVolume plays, [`Contest::path_end`]: street_indu_02, vert_dwmc_01), on a known map. ponytail: the other
    /// variants' graph code is ported but their measures (Speed's entry speed, the air grab time, the footplant /
    /// handplant / hippy jump trick types) are not wired.
    pub fn playable(&self, id: &str) -> bool {
        let placed = |v: &[Option<RaceVolume>]| v.iter().all(Option::is_some);
        self.online
            && matches!(self.challenge_type, Some(STREET | VERT))
            && self.elimination != Some(true)
            && self.ai_online != Some(true)
            && !self.rounds.is_empty()
            && self.rounds.iter().all(|r| {
                matches!(r.contest_variant, SEQUENCE | GRAB | GRIND | GRIND_LENGTH | AIR_DISTANCE | AIR_HEIGHT | AIR_ONLY)
                    && r.start_location.as_ref().is_some_and(|l| l.slots.len() == 6)
                    && placed(&r.spot_volumes)
                    && r.round_boundary.is_some()
                    && r.restart_last_chance_volume.is_empty()
                    && r.ai_out_of_bounds.as_deref().is_none_or(placed)
            })
            && skate3_modes::map_of(id).is_some()
    }
    /// Every volume a round tests, once each (the trigger messages' index).
    pub fn volumes(&self) -> Vec<RaceVolume> {
        let mut out: Vec<RaceVolume> = Vec::new();
        for r in &self.rounds {
            let all = (r.round_boundary.iter().chain(&r.path_end_volume))
                .chain(r.spot_volumes.iter().flatten())
                .chain(r.ai_out_of_bounds.iter().flatten().flatten());
            for v in all {
                if !out.iter().any(|o| o.name == v.name) {
                    out.push(v.clone());
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------- the graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    SetupCode,
    NisOverview,
    WaitForAssetload,
    NisWithCharacters,
    SetupChallenge,
    SetupRound,
    Countdown,
    RunRound,
    EndRound,
    Highlight,
    ResultsScreen,
    CleanupRound,
    EndChallenge,
    EndOptions,
    CleanupChallenge,
    Complete,
}

/// vars.Competitor[i] (id i - 1).
#[derive(Clone, Debug, Default)]
pub(crate) struct Comp {
    pub active: bool,
    pub disconnected: bool,
    pub total_score: i64,
    pub round_score: f64,
    round_highlight: i64,
    round_highlight_score: f64,
    pub round_rank: i64,
    in_boundary: bool,
    oob_timer: Option<u32>,
    in_spots: i64,
    hit_spot: bool,
    did_flip: bool,
    scored_points: bool,
    /// startLocator: the StartLocation slot the path end sends this competitor back to (its id at setup_round, 571).
    start_locator: usize,
    /// The team (1 / 2; 0: the Lua's -1, no teams).
    pub team: usize,
}

/// vars.Teams[i] (123-133).
#[derive(Clone, Debug, Default)]
pub(crate) struct Team {
    pub round_score: f64,
    /// Rounds won.
    pub total_score: i64,
    pub rank: i64,
    pub members: usize,
    pub name: String,
}

/// vars.RoundStandings[i] / vars.TotalStandings[i].
#[derive(Clone, Debug)]
pub(crate) struct Standing {
    pub score: f64,
    pub c: usize,
    pub rank: i64,
}

/// contest_shared's vars on one console.
#[derive(Clone)]
pub(crate) struct Contest {
    row: Row,
    /// Each round's intro presentation length.
    intros: Vec<f64>,
    me: usize,
    n: usize,
    pub comps: Vec<Comp>,
    setup_complete: bool,
    /// CurRoundNum (SequenceConnector.SetContestRound's).
    pub round: usize,
    wait_pres: Option<u32>,
    load_delay: Option<u32>,
    post_nis: Option<u32>,
    countdown_timer: Option<u32>,
    round_timer: Option<u32>,
    audio_end: Option<u32>,
    audio_warn: Option<u32>,
    oob_timer: Option<u32>,
    fade_out_timer: Option<u32>,
    fe_timer: Option<u32>,
    fe_popped: Option<u32>,
    teleports: usize,
    scoring: bool,
    points_scored: bool,
    best_highlight: i64,
    best_highlight_score: f64,
    wrong_tricks: i64,
    round_winner: Vec<Option<usize>>,
    pub round_standings: Vec<Standing>,
    pub total_standings: Vec<Standing>,
    highlights: usize,
    pub won: bool,
    /// The sequence being delivered: its trick types (T_*).
    pub types: u32,
    /// The air being delivered: Trick.GetCompletedAirHorizontalDistance, AirGrabTime, AirHeight, AirDegreesOfSpin,
    /// AirTime, AirNumFlips.
    pub air: [f64; 6],
    /// The round intro NIS last entered (the glue plays it while Rt::presenting runs).
    pub scene: Option<String>,
    /// The results showing: the round's (true at results_screen) and, after end_options, the standings.
    pub results_round: bool,
    pub floats: Vec<(usize, f64)>,
    /// Online.IsTeamChallenge / Online.GetTeam: each player's team (1 / 2), empty for none; Teams.GetName's names.
    pub teams_in: Vec<u8>,
    pub team_names: [String; 2],
    /// vars.TeamsEnabled, vars.Teams, vars.TeamStandings, TeamIntroductionTimerID.
    pub teams_on: bool,
    pub teams: [Team; 2],
    team_standings: [Vec<Standing>; 2],
    team_intro: Option<u32>,
    /// G7: the round's VisualIndicators are added (setup_round 552-555) and not yet removed (cleanup_round 1650-1653;
    /// cleanup_challenge's DeleteGroup 1711).
    indicators: bool,
    /// SessionMarker.SetSessionMarker (countdown 880) called: the glue sets this console's marker where it stands.
    pub set_marker: bool,
}

impl Contest {
    pub fn new(row: Row, intros: Vec<f64>, players: usize, me: usize) -> Self {
        Contest {
            row,
            intros,
            me,
            n: players,
            comps: vec![Comp::default(); players],
            setup_complete: false,
            round: 0,
            wait_pres: None,
            load_delay: None,
            post_nis: None,
            countdown_timer: None,
            round_timer: None,
            audio_end: None,
            audio_warn: None,
            oob_timer: None,
            fade_out_timer: None,
            fe_timer: None,
            fe_popped: None,
            teleports: 0,
            scoring: false,
            points_scored: false,
            best_highlight: -1,
            best_highlight_score: 0.,
            wrong_tricks: 0,
            round_winner: Vec::new(),
            round_standings: Vec::new(),
            total_standings: Vec::new(),
            highlights: 0,
            won: false,
            types: 0,
            air: [0.; 6],
            scene: None,
            results_round: false,
            floats: Vec::new(),
            teams_in: Vec::new(),
            team_names: ["T1".into(), "T2".into()],
            teams_on: false,
            teams: Default::default(),
            team_standings: Default::default(),
            team_intro: None,
            indicators: false,
            set_marker: false,
        }
    }

    pub fn cur(&self) -> Option<&Round> {
        self.round.checked_sub(1).and_then(|r| self.row.rounds.get(r))
    }
    /// G7: the round's VisualIndicators while they are up, as skate3_indicators::show items, each in its own look:
    /// SetVolumeHighlight (403-407) recolours only Best_Line rounds, which no row has (playable() refuses one).
    pub fn markers(&self) -> Vec<(String, skate3_modes::Matrix)> {
        self.cur().filter(|_| self.indicators).map_or_else(Vec::new, |r| crate::skate3_indicators::looks(&r.visual_indicators))
    }
    fn r(&self) -> Result<Round, Abort> {
        self.cur().cloned().ok_or(Abort)
    }
    /// vars.GetCompetitorIndex.
    fn index(&self, p: usize) -> Option<usize> {
        (p < self.n).then_some(p)
    }
    fn start(&self, slot: usize) -> Place {
        Place { field: START, index: self.round.saturating_sub(1), slot }
    }

    /// [setup_code] stateenter (13-455), the online path.
    fn setup_code(&mut self, rt: &mut Rt<St>) {
        rt.pin(self.me, true);
        for (i, c) in self.comps.iter_mut().enumerate() {
            *c = Comp { active: true, disconnected: rt.gone.get(i).copied().unwrap_or(true), round_highlight: -1, round_rank: i as i64 + 1, in_boundary: true, ..Comp::default() };
        }
        // 19, 123-190: online, TeamsEnabled = Online.IsTeamChallenge(); each player's Online.GetTeam (0x828463d8: the
        // player's +112, plus 1).
        self.teams_on = !self.teams_in.is_empty();
        if self.teams_on {
            self.teams = self.team_names.clone().map(|name| Team { rank: 2, name, ..Team::default() });
            for (i, c) in self.comps.iter_mut().enumerate() {
                c.team = if self.teams_in.get(i) == Some(&2) { 2 } else { 1 };
                self.teams[c.team - 1].members += 1;
            }
        }
        self.round = 0;
        self.scoring = false;
        self.setup_complete = true;
        self.round_winner = vec![None; self.row.rounds.len()];
        // HUD2.CreateGroup: hudloaded once the group is up.
        rt.post(Msg::HudLoaded);
    }

    /// [setup_code] hudloaded (458-479).
    fn hud_loaded(&mut self, rt: &mut Rt<St>) {
        rt.show(Component::ObjectiveHUD, false);
        rt.show(Component::MiniMap, true);
        rt.dpo_format(DPO_POINTS);
        rt.dpo_title("ID_CHALLENGE_DEMO_OBJECTIVE");
        rt.change(St::NisOverview);
    }

    /// [setup_round] stateenter (520-778).
    fn setup_round(&mut self, rt: &mut Rt<St>) -> Handled {
        self.round += 1;
        let r = self.r()?;
        rt.note(format!("contestround {}", self.round));
        self.indicators = true; // 552-555
        (self.load_delay, self.post_nis, self.audio_end, self.audio_warn, self.oob_timer) = (None, None, None, None, None);
        self.wrong_tricks = 0;
        let mut pos = 1;
        for (i, c) in self.comps.iter_mut().enumerate() {
            if c.active {
                c.round_score = 0.;
                c.round_rank = pos;
                pos += 1;
            } else {
                c.round_score = -1.;
                c.round_rank = c.total_score;
            }
            (c.round_highlight, c.round_highlight_score, c.in_boundary, c.oob_timer, c.start_locator) = (-1, 0., true, None, i);
            (c.in_spots, c.hit_spot, c.did_flip) = (0, false, false);
        }
        self.points_scored = false;
        if self.teams_on {
            // 593-598.
            for t in &mut self.teams {
                (t.round_score, t.rank) = (0., 2);
            }
        }
        if r.contest_variant == BEST_LINE {
            // ponytail: Best_Line (its line volumes, DPO and termination objectives) is not ported; no online row.
            return Err(Abort);
        }
        rt.show(Component::DPOHUD, false);
        (self.best_highlight, self.best_highlight_score) = (-1, 0.);
        self.teleports = 0;
        rt.reposition(self.me, self.start(self.me), 0.);
        rt.pin(self.me, true);
        let use_int = r.use_int();
        let layout = if use_int { RANK_NAME_SCORE } else { RANK_NAME_FLOATSCORE };
        if self.teams_on {
            rt.board_init_teams(layout, self.n, self.comps.iter().map(|c| c.team as u8).collect(), self.team_names.clone());
        } else {
            rt.board_init(layout, self.n);
        }
        for i in 0..self.n {
            let c = &self.comps[i];
            let (rank, score) = (c.round_rank, c.round_score);
            rt.board_rank(i, rank);
            rt.board_int(i, score as i64);
            rt.note(format!("float {i} {score:.3}"));
        }
        if self.teams_on {
            // 742-753 (the ranks change no HUD; SetTeamScore both).
            let [a, b] = [self.teams[0].round_score, self.teams[1].round_score];
            if 0. < a || 0. < b {
                if a > b {
                    self.teams[0].rank = 1;
                } else if b > a {
                    self.teams[1].rank = 1;
                }
            }
            for t in 1..=2 {
                rt.board_team_score(t, self.teams[t - 1].round_score as i64);
            }
        }
        // LeaderboardHUDSetVariantType(variant, IsContest and not CumulativeScoring).
        let best = !r.cumulative_scoring;
        rt.note(format!("varianttype {} {best}", r.contest_variant));
        let text = VARIANT_STRINGS.get(r.contest_variant as usize).filter(|_| best || r.contest_variant != BEST_LINE);
        rt.hud.board.variant = Some(text.map_or("ID_COMMON_UNKNOWN".into(), |t| format!("ID_CONTEST_VARIANT_{t}{}", if best { "" } else { "_JAM" })));
        rt.show(Component::TrickDisplayHUD, true);
        rt.show(Component::LeaderboardHUD, true);
        // Online.OnlineChallengeRoundSyncPoint.
        rt.sync();
        Ok(())
    }

    /// [countdown] stateenter (831-880).
    fn countdown(&mut self, rt: &mut Rt<St>) {
        let team = self.comps.get(self.me).map_or(1, |c| c.team.max(1));
        if self.teams_on && self.round == 1 {
            // 832-834: this player's team, in its colour (team - 1: 0 red, 1 blue); the 3-2-1 after TeamIntroductionDelay.
            let id = self.row.team_indication.clone();
            rt.team_central(&id, team as i64 - 1, TEAM_NORMAL, &[self.teams[team - 1].name.clone()]);
            self.team_intro = Some(rt.add_timer(self.row.team_intro_delay));
        } else {
            if self.row.rounds.len() != 1 {
                if self.round == self.row.rounds.len() {
                    let id = self.row.final_round.clone();
                    rt.central(&id, RED, &[], 0);
                } else {
                    let id = self.row.round_num.clone();
                    rt.central(&id, WHITE, &[self.round.to_string()], 0);
                }
            }
            self.start_countdown(rt);
        }
        if self.round > 1 {
            // 855-866 (RoundWinnerIndex: the audio's round history).
            let winner = if self.teams_on {
                (self.teams[team - 1].total_score == self.round as i64 - 1).then_some(self.me)
            } else {
                self.round_standings.first().map(|s| s.c)
            };
            if let Some(w) = self.round_winner.get_mut(self.round - 2) {
                *w = winner;
            }
        }
        // s2rules 6 (online audit 10-28): SessionMarker.SetSessionMarker, every round (880).
        self.set_marker = true;
    }

    /// ChallengeTimer.AddHUDCountdownTimer(CountdownTime) + HUD2.StartCountdown (849-850, 885-886).
    fn start_countdown(&mut self, rt: &mut Rt<St>) {
        let t = rt.add_timer(COUNTDOWN);
        self.countdown_timer = Some(t);
        rt.countdown(t);
    }

    /// [run_round] stateenter (894-958), the online path.
    fn run_round(&mut self, rt: &mut Rt<St>) -> Handled {
        let r = self.r()?;
        self.scoring = true;
        rt.pin(self.me, false);
        let time = r.round_time_online.filter(|&t| t > 0).unwrap_or(r.round_time);
        let t = rt.add_timer(time as f64);
        self.round_timer = Some(t);
        rt.show(Component::TimerHUD, true);
        rt.timer(t);
        // The audio timers run on RoundTime even when RoundTimeOnline is set (924-927).
        self.audio_end = Some(rt.add_timer((r.round_time - 3) as f64));
        self.audio_warn = Some(rt.add_timer((r.round_time - r.audio_round_end_warning_time) as f64));
        // The round presentation: RoundPresentationEvents is empty on every row.
        rt.present(0.);
        Ok(())
    }

    /// [run_round] timerexpired (961-1084), the online path.
    fn run_round_timer(&mut self, rt: &mut Rt<St>, id: u32) -> Handled {
        let r = self.r()?;
        if self.round_timer == Some(id) {
            if r.run_type == TIMED_RUN_SINGLE {
                self.comps[self.me].round_score = 0.;
            }
            if !r.cumulative_scoring && self.best_highlight == -1 {
                self.best_highlight = HIGHLIGHT;
            } else if r.cumulative_scoring && self.comps[self.me].round_highlight == -1 {
                self.comps[self.me].round_highlight = HIGHLIGHT;
            }
            rt.change(St::EndRound);
        } else if self.audio_end == Some(id) || self.audio_warn == Some(id) {
            // The music / speech cues.
        } else if self.oob_timer == Some(id) {
            rt.start_effect(Effect::OOB, 5.);
        } else if let Some(i) = (0..self.n).find(|&i| self.comps[i].oob_timer == Some(id)) {
            rt.reposition(i, self.start(i), 0.);
        }
        Ok(())
    }

    /// vars:UpdateScore (200-388), the online non-team path.
    fn update_score(&mut self, rt: &mut Rt<St>, c: usize, points: f64) -> Handled {
        if !self.scoring {
            return Ok(());
        }
        let r = self.r()?;
        self.comps[c].scored_points = true;
        let mut needs = false;
        if !r.cumulative_scoring && self.comps[c].round_score < points {
            needs = true;
            let increase = points - self.comps[c].round_score;
            self.comps[c].round_score = points;
            self.team_add(rt, c, increase);
            self.board_value(rt, &r, c);
            if c == self.me && self.best_highlight_score < points {
                self.best_highlight_score = points;
                self.best_highlight = HIGHLIGHT;
            }
        } else if r.cumulative_scoring && r.cumulative_min_score as f64 <= points {
            needs = true;
            self.comps[c].round_score += points;
            self.team_add(rt, c, points);
            self.board_value(rt, &r, c);
            if self.comps[c].round_highlight_score < points {
                self.comps[c].round_highlight_score = points;
                if c == self.me {
                    self.comps[c].round_highlight = HIGHLIGHT;
                }
            }
        }
        let mut took = false;
        while needs {
            needs = false;
            if self.teams_on {
                // 279-296: the scorer's team takes the lead from 2nd, or both are 2nd on a tie.
                let ours = self.comps[c].team;
                let other = 3 - ours;
                let (a, b) = (self.teams[ours - 1].round_score, self.teams[other - 1].round_score);
                if self.teams[ours - 1].rank == 2 && a > b {
                    took = true;
                    (self.teams[ours - 1].rank, self.teams[other - 1].rank) = (1, 2);
                    rt.board_team_rank(ours, 1);
                    rt.board_team_rank(other, 2);
                } else if a == b {
                    (self.teams[ours - 1].rank, self.teams[other - 1].rank) = (2, 2);
                    rt.board_team_rank(ours, 2);
                    rt.board_team_rank(other, 2);
                }
            }
            let cur = self.comps[c].round_rank;
            if let Some(i) = (0..self.n).find(|&i| self.comps[i].round_rank == cur - 1) {
                if self.comps[i].round_score < self.comps[c].round_score {
                    self.comps[i].round_rank = cur;
                    self.comps[c].round_rank = cur - 1;
                    rt.board_rank(i, cur);
                    rt.board_rank(c, cur - 1);
                    if 2 < cur {
                        needs = true;
                        continue;
                    }
                    took = true;
                }
            }
        }
        if !self.points_scored && points > 0. {
            took = true;
            self.points_scored = true;
        }
        // 328-354: a team's lead is the announcer's alone (AudioNewLeader), no message.
        if took && !self.teams_on && r.run_type != SOLO_RUN && r.run_type != SOLO_RUN_CYCLE {
            let (id, params) = if c == self.me { (self.row.new_lead_you.clone(), vec![]) } else { (self.row.new_lead.clone(), vec![format!("P{c}")]) };
            match id {
                Some(id) => rt.modified(&id, WHITE, &params, false),
                // ponytail: ShowModifiedCentralMessage with a nil text (no NewLead fields in the contest templates):
                // the native's handling is not traced; nothing is shown.
                None => rt.note(format!("modified nil {WHITE} {} false", params.join(","))),
            }
        }
        Ok(())
    }

    /// 218-222 / 241-245: the scorer's team's round score, LeaderboardHUDSetTeamScore.
    fn team_add(&mut self, rt: &mut Rt<St>, c: usize, v: f64) {
        if !self.teams_on {
            return;
        }
        let t = self.comps[c].team;
        self.teams[t - 1].round_score += v;
        rt.board_team_score(t, self.teams[t - 1].round_score as i64);
    }

    /// vars.TeamStandings (1523-1537, 2328-2342): each team's members by round score.
    fn team_standings(&self) -> [Vec<Standing>; 2] {
        [1, 2].map(|t| {
            let mut st: Vec<Standing> =
                (0..self.n).filter(|&j| self.comps[j].team == t).map(|j| Standing { score: self.comps[j].round_score, c: j, rank: 0 }).collect();
            skate3_modes::lua_sort(&mut st, |a, b| a.score > b.score);
            st
        })
    }

    /// LeaderboardHUDSetInt / SetFloat (the round's score kind).
    fn board_value(&self, rt: &mut Rt<St>, r: &Round, c: usize) {
        let v = self.comps[c].round_score;
        if r.use_int() {
            rt.board_int(c, v as i64);
        } else {
            rt.note(format!("float {c} {v:.3}"));
        }
    }

    /// [run_round] sequencecomplete (1087-1179).
    fn sequence(&mut self, rt: &mut Rt<St>, p: usize, score: i64, wiped: bool) -> Handled {
        let Some(c) = self.index(p) else { return Ok(()) };
        if wiped {
            return Ok(());
        }
        let r = self.r()?;
        if !r.sequence() {
            return Ok(());
        }
        let mut points = score as f64;
        let mut wrong = false;
        let v = r.contest_variant;
        let lacks = |t: u32| self.types & t == 0;
        if !self.comps[c].in_boundary || (!r.spot_volumes.is_empty() && !self.comps[c].hit_spot) {
            points = 0.;
        } else if (v == GRAB && lacks(T_GRAB))
            || (v == GRIND && lacks(T_GRIND))
            || (v == FOOTPLANT && lacks(T_FOOTPLANT))
            || (v == HANDPLANT && lacks(T_HANDPLANT))
            || (v == HIPPYJUMP && lacks(T_HIPPYJUMP))
            || (v == FLIP && !self.comps[c].did_flip)
        {
            wrong = true;
            points = 0.;
        }
        if c == self.me {
            if wrong {
                self.wrong_tricks += 1;
                if self.wrong_tricks >= 3 {
                    self.wrong_tricks = 0;
                    rt.modified("ID_CONTEST_WRONG_TYPE", WHITE, &[], false);
                }
            } else if points > 0. {
                self.wrong_tricks = 0;
            }
        }
        self.update_score(rt, c, points)
    }

    /// [run_round] aircompleted (1399-1429).
    fn air_done(&mut self, rt: &mut Rt<St>, p: usize) -> Handled {
        let r = self.r()?;
        if !(r.air() || r.contest_variant == FLIP) {
            return Ok(());
        }
        let Some(c) = self.index(p) else { return Ok(()) };
        if !r.air() {
            self.comps[c].did_flip = self.air[5] != 0.;
            return Ok(());
        }
        let mut points = match r.contest_variant {
            AIR_DISTANCE => self.air[0],
            AIR_GRAB_TIME => self.air[1],
            AIR_HEIGHT => self.air[2],
            AIR_SPIN => self.air[3].abs(),
            _ => self.air[4],
        };
        if !self.comps[c].in_boundary || (!r.spot_volumes.is_empty() && !self.comps[c].hit_spot) {
            points = 0.;
        }
        self.update_score(rt, c, points)
    }

    /// [run_round] triggerentercollision (1182-1286) / triggerexitcollision (1289-1318).
    fn trigger(&mut self, rt: &mut Rt<St>, p: usize, vol: &str, inside: bool) -> Handled {
        let Some(c) = self.index(p) else { return Ok(()) };
        let r = self.r()?;
        let boundary = Round::name(&r.round_boundary) == Some(vol);
        if inside {
            if boundary {
                if let Some(t) = self.comps[c].oob_timer.take() {
                    rt.remove_timer(t);
                }
                self.comps[c].in_boundary = true;
                if c == self.me {
                    rt.reset_effect(Effect::OOB);
                    if let Some(t) = self.oob_timer {
                        rt.remove_timer(t);
                    }
                }
            }
            for s in &r.spot_volumes {
                if Round::name(s) == Some(vol) {
                    self.comps[c].in_spots += 1;
                    self.comps[c].hit_spot = true;
                }
            }
            if self.scoring && Round::name(&r.path_end_volume) == Some(vol) {
                self.path_end(rt, c, &r);
            }
            if c != self.me
                && let Some(ai) = &r.ai_out_of_bounds
            {
                for v in ai {
                    if Round::name(v) == Some(vol) {
                        rt.reposition(p, self.start(p), 0.);
                    }
                }
            }
        } else {
            if boundary {
                self.comps[c].in_boundary = false;
                self.comps[c].oob_timer = Some(rt.add_timer((r.audio_out_of_bounds_warning_time + 3) as f64));
                if c == self.me {
                    let id = self.row.return_to_area.clone();
                    rt.central(&id, WHITE, &[], 0);
                    self.oob_timer = Some(rt.add_timer(r.audio_out_of_bounds_warning_time as f64));
                }
            }
            for s in &r.spot_volumes {
                if Round::name(s) == Some(vol) {
                    self.comps[c].in_spots -= 1;
                }
            }
        }
        Ok(())
    }

    /// The end of the run's path (1234-1272): SoloRunCycle / TimedRunMulti take the next start slot (round robin over
    /// the competitors); TimedRunMulti goes back to it (this console's skater behind RepositionAI's default fade, the
    /// others at once); TimedRunSingle ends this console's round, or pins the other who got there. Online, SoloRun is
    /// off (SinglePlayer only, 617) and playable() keeps the RestartLastChanceVolume rows out, so restartAtStart stays
    /// false. ponytail: AudioRunComplete (the NPC speech) is not ported, as the other Audio* cues.
    fn path_end(&mut self, rt: &mut Rt<St>, c: usize, r: &Round) {
        if matches!(r.run_type, SOLO_RUN_CYCLE | TIMED_RUN_MULTI) {
            let next = self.comps[c].start_locator + 1;
            self.comps[c].start_locator = if next >= self.n { 0 } else { next };
        }
        let slot = self.comps[c].start_locator;
        match r.run_type {
            TIMED_RUN_MULTI => rt.reposition(c, self.start(slot), if c == self.me { 1. } else { 0. }),
            TIMED_RUN_SINGLE if c == self.me => rt.change(St::EndRound),
            TIMED_RUN_SINGLE => rt.pin(c, true),
            _ => {}
        }
    }

    /// [end_round] stateenter (1460-1634), the online non-team, non-elimination path.
    fn end_round(&mut self, rt: &mut Rt<St>) -> Handled {
        rt.pause(true); // 1462
        let r = self.r()?;
        self.scoring = false;
        rt.pin(self.me, true);
        // HUD2.ShowGroup(false).
        (rt.hud.board.visible, rt.hud.dpo.visible, rt.hud.countdown, rt.hud.timer) = (false, false, None, None);
        let mut st: Vec<Standing> = (0..self.n).map(|i| Standing { score: self.comps[i].round_score, c: i, rank: 0 }).collect();
        skate3_modes::lua_sort(&mut st, |a, b| a.score > b.score);
        for (i, s) in st.iter().enumerate() {
            self.comps[s.c].round_rank = i as i64 + 1;
        }
        let mut total: Vec<Standing> = if self.teams_on {
            // 1504-1555: the round's point to the team ahead (rank 1; a tie: none); TotalStandings by round score.
            if self.teams[0].rank == 1 {
                self.teams[0].total_score += 1;
            } else if self.teams[1].rank == 1 {
                self.teams[1].total_score += 1;
            }
            self.round_standings = st;
            self.team_standings = self.team_standings();
            (0..self.n).map(|i| Standing { score: self.comps[i].round_score, c: i, rank: -1 }).collect()
        } else {
            let n = self.n as i64;
            let mut points = n - 1;
            if self.comps[st[0].c].active {
                self.comps[st[0].c].total_score += points;
            }
            for i in 1..self.n {
                if st[i].score != st[i - 1].score {
                    points = n - 1 - i as i64;
                }
                if self.comps[st[i].c].active {
                    self.comps[st[i].c].total_score += points;
                }
            }
            self.round_standings = st;
            (0..self.n).map(|i| Standing { score: self.comps[i].total_score as f64, c: i, rank: -1 }).collect()
        };
        skate3_modes::lua_sort(&mut total, |a, b| a.score > b.score);
        total[0].rank = 1;
        for i in 1..self.n {
            total[i].rank = if total[i].score == total[i - 1].score { total[i - 1].rank } else { i as i64 + 1 };
        }
        self.total_standings = total;
        self.highlights = usize::from(if r.cumulative_scoring { self.comps[self.me].round_highlight != -1 } else { self.best_highlight != -1 });
        rt.change(if self.highlights > 0 { St::Highlight } else { St::ResultsScreen });
        Ok(())
    }

    /// [results_screen] stateenter (2230-2416), the online non-team, non-elimination path: the standings
    /// (FinalResults), then the round (RoundResults) the screen shows.
    fn results_screen(&mut self, rt: &mut Rt<St>) -> Handled {
        let r = self.r()?;
        rt.hud.results.clear();
        if self.teams_on {
            self.team_results_screen(rt, &r);
            self.results_round = true;
            self.fe_timer = Some(rt.add_timer(RESULTS));
            return Ok(());
        }
        for i in 0..self.n {
            let s = &self.total_standings[i];
            rt.result(s.c, i as i64 + 1, s.score as i64);
        }
        rt.hud.results.clear();
        self.floats.clear();
        for i in 0..self.n {
            let s = self.round_standings[i].clone();
            if r.use_int() {
                rt.result(s.c, i as i64 + 1, s.score as i64);
            } else {
                rt.result(s.c, i as i64 + 1, -1);
                rt.note(format!("resultf {} {:.3}", s.c, s.score));
                self.floats.push((s.c, s.score));
            }
        }
        self.results_round = true;
        self.fe_timer = Some(rt.add_timer(RESULTS));
        Ok(())
    }

    /// [results_screen] stateenter's team path (2284-2295, 2319-2384): the FinalResults group's two team rows
    /// (TeamChallengeResults: rank by rounds won, the round score), then the RoundResults group's two member lists
    /// (TeamChallengeInt / Float, the second team's through Results_SetSecondLBPlayerData). Only the team rows reach
    /// the screen (`rt.hud.results`): endchallenge_online's FillTable draws TeamChallengeResults and no
    /// TeamChallengeInt / Float layout (its eResultsLBType tests, APT 5396). ponytail: which group the online screen
    /// shows is the native's (not traced); the team table.
    fn team_results_screen(&mut self, rt: &mut Rt<St>, r: &Round) {
        let [a, b] = [self.teams[0].total_score, self.teams[1].total_score];
        let ranks = [if a > b { 1 } else { 2 }, if a < b { 1 } else { 2 }];
        for k in 0..2 {
            let s = self.teams[k].round_score;
            rt.result(k, ranks[k], s as i64);
            rt.note(format!("resultf {k} {s:.3}"));
        }
        self.team_standings = self.team_standings();
        for (k, list) in self.team_standings.iter().enumerate() {
            let tag = if k == 0 { "result" } else { "result2" };
            for (i, s) in list.iter().enumerate() {
                if r.use_int() {
                    rt.note(format!("{tag} {} {} {}", s.c, i + 1, s.score as i64));
                    if k == 0 {
                        rt.note(format!("resultf {} 0.000", s.c));
                    }
                } else {
                    rt.note(format!("{tag} {} {} 0", s.c, i + 1));
                    rt.note(format!("resultf {} {:.3}", s.c, s.score));
                }
            }
        }
    }

    /// [end_options] stateenter (1922-2114), the online path: the standings, then the report (teams: the winners'
    /// OnlineOutroNIS first, when the row names one and a team won).
    fn end_options(&mut self, rt: &mut Rt<St>) {
        rt.hud.results.clear();
        self.floats.clear();
        self.results_round = false;
        if !self.teams_on {
            for i in 0..self.n {
                let s = &self.total_standings[i];
                rt.result(s.c, i as i64 + 1, s.score as i64);
            }
            self.report(rt);
            rt.change(St::CleanupChallenge);
            return;
        }
        // 1968-1997: the teams' rounds won (TeamChallengeResults), then each team's members (Name).
        let [a, b] = [self.teams[0].total_score, self.teams[1].total_score];
        let ranks = if a > b { [1, 2] } else if a < b { [2, 1] } else { [2, 2] };
        rt.result(0, ranks[0], a);
        rt.result(1, ranks[1], b);
        for (k, list) in self.team_standings.iter().enumerate() {
            for (i, s) in list.iter().enumerate() {
                rt.note(format!("{} {} {} 0", if k == 0 { "result" } else { "result2" }, s.c, i + 1));
                rt.note(format!("resultf {} 0.000", s.c));
            }
        }
        // 2047-2060 (a tie: no winning team, both rank 2).
        let winner = if a > b { Some(1) } else if b > a { Some(2) } else { None };
        (self.teams[0].rank, self.teams[1].rank) = (ranks[0], ranks[1]);
        if winner.is_some() && self.row.online_outro_nis.is_some() {
            // 2069-2078: NIS.SubmitNISPlayerIndex per winner, Results.SetWinningTeam, the NIS; nischallengenotify
            // reports (2133-2141). ponytail: the NIS player with dynamic characters is not ported: no scene plays.
            rt.nis(0.);
        } else {
            self.report(rt);
            rt.change(St::CleanupChallenge);
        }
    }

    /// Online.OnlineChallengeComplete(FinalResults[, TeamResults]): per TotalStandings {id, Rank, Score, -1, false,
    /// npc, eFinishReason, ChallengeCompleteResult}; per team {rank, totalScore}.
    fn report(&self, rt: &mut Rt<St>) {
        let result = i64::from(self.won);
        for s in &self.total_standings {
            let reason = i64::from(self.comps[s.c].scored_points);
            rt.note(format!("report {} {} {} {reason} {result}", s.c, s.rank, s.score as i64));
        }
        if self.teams_on {
            for (i, t) in self.teams.iter().enumerate() {
                rt.note(format!("teamreport {} {} {}", i + 1, t.rank, t.total_score));
            }
        }
    }

    /// [global] playerdisconnected (1801-1831).
    fn disconnected(&mut self, rt: &mut Rt<St>, p: usize) {
        if !self.setup_complete {
            return;
        }
        let Some(c) = self.index(p) else { return };
        self.comps[c].disconnected = true;
        if c == self.me {
            rt.leave();
            return;
        }
        self.comps[c].active = false;
        let mut gone = 0;
        for i in 0..self.n {
            if self.comps[i].active {
                if self.comps[i].round_rank > self.comps[c].round_rank {
                    self.comps[i].round_rank -= 1;
                    rt.board_rank(i, self.comps[i].round_rank);
                }
            } else {
                gone += 1;
            }
        }
        let rank = self.n as i64 - gone + 1;
        self.comps[c].round_rank = rank;
        self.comps[c].total_score = rank;
        rt.board_int(c, -1);
        rt.board_rank(c, rank);
    }

    /// The round's score kind is a float (a distance or a length).
    pub fn floats_round(&self) -> bool {
        self.cur().is_some_and(|r| !r.use_int())
    }
}

impl Graph for Contest {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::SetupCode => "setup_code",
            St::NisOverview => "nis_overview",
            St::WaitForAssetload => "wait_for_assetload",
            St::NisWithCharacters => "nis_with_characters",
            St::SetupChallenge => "setup_challenge",
            St::SetupRound => "setup_round",
            St::Countdown => "countdown",
            St::RunRound => "run_round",
            St::EndRound => "end_round",
            St::Highlight => "highlight",
            St::ResultsScreen => "results_screen",
            St::CleanupRound => "cleanup_round",
            St::EndChallenge => "end_challenge",
            St::EndOptions => "end_options",
            St::CleanupChallenge => "cleanup_challenge",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        // The global handler first.
        if let Msg::Left(p) = msg {
            self.disconnected(rt, *p);
        }
        match (rt.state, msg) {
            (St::Initial, Msg::Enter) => {
                self.setup_complete = false;
                rt.pause(true); // 9
                rt.change(St::SetupCode);
            }
            (St::SetupCode, Msg::Enter) => self.setup_code(rt),
            (St::SetupCode, Msg::HudLoaded) => self.hud_loaded(rt),
            (St::NisOverview, Msg::Enter) => rt.change(St::WaitForAssetload),
            (St::WaitForAssetload, Msg::Enter) => self.wait_pres = Some(rt.add_timer(WAIT_PRES)),
            (St::WaitForAssetload, Msg::Timer(id)) if self.wait_pres == Some(*id) => rt.change(St::NisWithCharacters),
            // 2491-2524: teams, the team intro NIS (OnlineIntroNIS, else IntroNIS) with both teams' skaters, setup_challenge
            // at its nischallengenotify; no teams, setup_challenge. ponytail: the NIS player with dynamic characters is
            // not ported (no scene plays); a team row without either NIS goes on (retail would wait in this state).
            (St::NisWithCharacters, Msg::Enter) => {
                if self.teams_on && (self.row.online_intro_nis.is_some() || self.row.intro_nis.is_some()) {
                    rt.nis(0.);
                } else {
                    rt.change(St::SetupChallenge);
                }
            }
            (St::NisWithCharacters, Msg::Nis) => rt.change(St::SetupChallenge),
            (St::SetupChallenge, Msg::Enter) => {
                self.round = 0;
                for c in &mut self.comps {
                    c.active = true;
                    c.total_score = 0;
                }
                if self.teams_on {
                    // 504-510.
                    for t in &mut self.teams {
                        (t.round_score, t.total_score, t.rank) = (0., 0, 2);
                    }
                }
                rt.change(St::SetupRound);
            }
            (St::SetupRound, Msg::Enter) => self.setup_round(rt)?,
            (St::SetupRound, Msg::Teleported(p)) if self.index(*p).is_some() => self.teleports += 1,
            (St::SetupRound, Msg::Synced) => {
                rt.pin(self.me, true);
                self.load_delay = Some(rt.add_timer(LOAD_DELAY));
            }
            (St::SetupRound, Msg::Timer(id)) if self.load_delay == Some(*id) => {
                let r = self.round - 1;
                self.scene = self.cur().and_then(|r| r.intro.clone());
                rt.present(self.intros.get(r).copied().unwrap_or(0.));
            }
            (St::SetupRound, Msg::Timer(id)) if self.post_nis == Some(*id) => rt.change(St::Countdown),
            (St::SetupRound, Msg::Presented) => {
                self.post_nis = Some(rt.add_timer(POST_NIS));
                rt.start_effect(Effect::FadeIn, FADE_IN);
                rt.close_loading();
            }
            (St::Countdown, Msg::Enter) => self.countdown(rt),
            (St::Countdown, Msg::Timer(id)) if self.team_intro == Some(*id) => self.start_countdown(rt),
            (St::Countdown, Msg::Timer(id)) if self.countdown_timer == Some(*id) => {
                rt.pause(false); // 889
                rt.change(St::RunRound);
            }
            (St::RunRound, Msg::Enter) => self.run_round(rt)?,
            (St::RunRound, Msg::Timer(id)) => self.run_round_timer(rt, *id)?,
            (St::RunRound, Msg::Sequence { player, score, wiped }) => self.sequence(rt, *player, *score, *wiped)?,
            (St::RunRound, Msg::SequenceBegin(p)) => {
                if let Some(c) = self.index(*p) {
                    let c = &mut self.comps[c];
                    c.hit_spot = c.in_spots > 0;
                    c.did_flip = false;
                }
            }
            (St::RunRound, Msg::AirDone(p)) => self.air_done(rt, *p)?,
            (St::RunRound, Msg::GrindDone { player, length }) => {
                let r = self.r()?;
                if r.contest_variant == GRIND_LENGTH
                    && let Some(c) = self.index(*player)
                {
                    let points = if self.comps[c].in_boundary { *length } else { 0. };
                    self.update_score(rt, c, points)?;
                }
            }
            (St::RunRound, Msg::Trigger { player, volume, inside }) => {
                let vol = self.row.volumes().get(*volume as usize).map(|v| v.name.clone()).unwrap_or_default();
                self.trigger(rt, *player, &vol, *inside)?;
            }
            (St::EndRound, Msg::Enter) => self.end_round(rt)?,
            // HighlightReel.PlayHighlightReelWithSpecifiedOrder.
            (St::Highlight, Msg::Enter) => rt.reel(),
            (St::Highlight, Msg::Reel) => rt.change(St::ResultsScreen),
            (St::ResultsScreen, Msg::Enter) => self.results_screen(rt)?,
            (St::ResultsScreen, Msg::Timer(id)) if self.fe_timer == Some(*id) => self.fe_popped = Some(rt.add_timer(RESULTS_POPPED)),
            (St::ResultsScreen, Msg::Timer(id)) if self.fe_popped == Some(*id) => rt.change(St::CleanupRound),
            (St::CleanupRound, Msg::Enter) => {
                self.results_round = false;
                self.indicators = false; // 1650-1653
                if self.round == self.row.rounds.len() {
                    rt.change(St::EndChallenge);
                } else {
                    rt.start_effect(Effect::FadeOut, FADE_OUT);
                }
            }
            (St::CleanupRound, Msg::Effect(Effect::FadeOut)) => self.fade_out_timer = Some(rt.add_timer(FADE_SETTLE)),
            (St::CleanupRound, Msg::Timer(id)) if self.fade_out_timer == Some(*id) => rt.change(St::SetupRound),
            (St::EndChallenge, Msg::Enter) => {
                // 2168-2174: teams, a win when team 1 has more rounds than team 2 (the Lua's test, whichever team this
                // player is on).
                self.won = if self.teams_on {
                    self.teams[0].total_score > self.teams[1].total_score
                } else {
                    self.total_standings.first().is_some_and(|s| self.comps[self.me].total_score as f64 == s.score)
                };
                rt.change(St::EndOptions);
            }
            (St::EndOptions, Msg::Enter) => self.end_options(rt),
            (St::EndOptions, Msg::Nis) => {
                self.report(rt);
                rt.change(St::CleanupChallenge);
            }
            (St::CleanupChallenge, Msg::Enter) => {
                rt.pin(self.me, false);
                self.indicators = false; // 1711 DeleteGroup
                rt.change(St::Complete);
            }
            (St::Complete, Msg::Enter) => {
                rt.leave();
                rt.pause(false); // 1725
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// What a skater did, relayed by its console with the round it happened in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Relay {
    round: usize,
    ev: RelayEv,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum RelayEv {
    Trigger(i64, bool),
    Begin,
    Sequence(i64, u32),
    Air([f64; 6]),
    Grind(f64),
    /// [`skate3_modes::EndSync`]'s marker: this console is done with the round.
    End(skate3_modes::EndMark),
}

/// What the probe saw this console's skater do.
#[derive(Clone, Debug)]
enum Seen {
    Trigger(i64, bool),
    Begin,
    Sequence(i64, u32),
    Air([f64; 6]),
    Grind(f64),
}

/// The probe's view of the skater between physics ticks.
#[derive(Default)]
struct Probe {
    inside: Vec<bool>,
    open: bool,
    types: u32,
    /// The session marker's returns (SessionMarker::uses) last seen, and a return since the last sequence ended
    /// (UsedSessionMarker).
    marker_uses: Option<u64>,
    marked: bool,
    /// Scorable identifier -> TrickType.
    kinds: std::collections::HashMap<&'static str, u32>,
    airborne: Option<Air>,
    grind: Option<([f32; 3], f32, u64)>,
    wiping: bool,
    seen: Vec<Seen>,
}

/// An air under way: take-off point and time, the highest point, spin and flips so far.
#[derive(Clone, Copy)]
struct Air {
    from: [f32; 3],
    at: f32,
    peak: f32,
    spin: f32,
    flips: bool,
    /// The session marker's returns at take-off.
    uses: u64,
}

struct Play {
    id: String,
    row: Row,
    g: Contest,
    rt: Rt<St>,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    inbox: Vec<(usize, Relay)>,
    placed: u32,
    playing: Option<String>,
    /// The results screen showing: (the round's, round) or the standings.
    showing: Option<(bool, usize)>,
    sync: EndSync,
    /// G7: the install has the contest's looks (an export from before G7 has none: nothing is drawn).
    looks: bool,
}

/// G7: the contests' looks (tools/skate3_online.py look_file).
const INDICATORS: &str = "private/skate3-online/contest/indicators.json";

impl Play {
    /// This console's skater plays the round (its events count).
    fn live(&self) -> bool {
        self.rt.state == St::RunRound && !self.sync.holding() && self.g.comps.get(self.me).is_some_and(|c| c.active)
    }
    /// The clock (online, a round's end held for the other consoles): the End marker to relay.
    fn tick(&mut self, now: f64) -> Option<String> {
        let phase = skate3_modes::advance_synced(&mut self.g, &mut self.rt, now, St::RunRound, self.me, &mut self.sync)?;
        let relay = Relay { round: self.g.round, ev: RelayEv::End(phase) };
        self.online.then(|| serde_json::to_string(&relay).expect("plain data"))
    }
    /// This console's skater did `ev`: delivered here, and what to relay.
    fn local(&mut self, ev: RelayEv) -> Option<String> {
        let text = self.online.then(|| serde_json::to_string(&Relay { round: self.g.round, ev: ev.clone() }).expect("plain data"));
        let me = self.me;
        deliver(self, me, ev);
        text
    }
    /// The others' events: their round's, while it runs (an early one waits; a past round's are dropped).
    fn receive(&mut self, received: Vec<(usize, Relay)>) {
        for (from, r) in received {
            match r.ev {
                RelayEv::End(phase) => self.sync.ended(from, phase),
                _ => self.inbox.push((from, r)),
            }
        }
        for (from, r) in std::mem::take(&mut self.inbox) {
            let here = self.g.round;
            if r.round > here || (r.round == here && matches!(self.rt.state, St::SetupRound | St::Countdown)) {
                self.inbox.push((from, r));
            } else if r.round == here && self.rt.state == St::RunRound && from != self.me {
                deliver(self, from, r.ev);
            }
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct ContestGame {
    data: Option<ModeData>,
    play: Option<Play>,
    probe: Probe,
    volumes: Vec<RaceVolume>,
    live: bool,
    clock: f32,
    warned: String,
}

pub(crate) struct Skate3ContestPlugin;
impl Plugin for Skate3ContestPlugin {
    fn build(&self, app: &mut App) {
        let data = ModeData::install(app, "contest", "Contest");
        if let Some(data) = &data {
            let ids: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).is_ok_and(|r| r.playable(id))).cloned().collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                // The rows, and their Team tab rows (contest_shared's TeamsEnabled path).
                playable.0.extend(ids.iter().cloned().chain(ids.iter().map(|id| crate::match_core::team(id))));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        app.insert_resource(ContestGame { data, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            // After online_boundary's: it clears the session marker's OOB static each frame; ours is the contest's.
            .add_systems(Update, run.after(crate::app::FrameSet::Physics).after(crate::online_boundary::update));
    }
}

/// The engine's TrickType numbering as skate2_contests::variant_points reads it: None 0, Air 1, Flip 2, FingerFlip 3,
/// Grab 4, Grind 5.
fn trick_bits(trick_type: u32) -> u32 {
    match trick_type {
        4 => T_GRAB,
        5 => T_GRIND,
        _ => 0,
    }
}

/// Every physics tick while this console's skater skates a round: the volumes (trigger enter / exit), the sequence
/// (sequencebegin / sequencecomplete with its trick types), each air (aircompleted) and grind (grindcompleted) it
/// lands. ponytail: the natives' measures (Trick.GetCompletedAir*, grindcompleted's GrindLength) are not traced; read
/// from the skater: the air's horizontal take-off-to-landing distance, its height over the take-off, its spin
/// (half-turns x 180) and time; the grind's path length. A wipeout drops the air or grind (Wipedout). One skated with a
/// return to the session marker (UsedSessionMarker) is dropped here: its handlers skip it whole (1091, 1323, 1401;
/// s2rules 6, online audit 10-28). A return is a teleport, which wipes the sequence under way (scoring_runtime reset),
/// so a sequence's flag is a return since the last one ended, as skate1_online's probe; an air or grind is flagged by a
/// return while it was under way (its measure would run to the marker).
fn probe(
    mut game: ResMut<ContestGame>,
    skater: Res<SkaterRuntime>,
    time: Res<Time>,
    marker: Option<Res<crate::session_marker::SessionMarker>>,
) {
    let ContestGame { probe, volumes, live, clock, .. } = &mut *game;
    *clock += time.delta_secs();
    if !*live {
        *probe = Probe { kinds: std::mem::take(&mut probe.kinds), ..Probe::default() };
        return;
    }
    let uses = marker.map_or(0, |m| m.uses);
    probe.marked |= probe.marker_uses.is_some_and(|was| was != uses);
    probe.marker_uses = Some(uses);
    let s = &skater.scoring;
    if probe.kinds.is_empty() {
        probe.kinds = s.data.definitions.iter().map(|d| (d.identifier, d.trick_type)).collect();
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let pos = [at[0], at[1], at[2]];
    probe.inside.resize(volumes.len(), false);
    for (i, (was, v)) in probe.inside.iter_mut().zip(volumes.iter()).enumerate() {
        let inside = v.contains(Vec3::from(pos));
        if inside != *was {
            *was = inside;
            probe.seen.push(Seen::Trigger(i as i64, inside));
        }
    }
    let open = s.sequence_open();
    if open && !probe.open {
        probe.types = 0;
        probe.seen.push(Seen::Begin);
    }
    if open || s.sequence_landed {
        for id in &s.announced {
            if let Some(&t) = probe.kinds.get(id) {
                probe.types |= trick_bits(t);
            }
        }
    }
    if s.sequence_landed && !std::mem::take(&mut probe.marked) {
        probe.seen.push(Seen::Sequence(s.lua_sequence_score(), probe.types));
    }
    let was_open = probe.open;
    probe.open = open;
    let wiping = skater.wiping_out();
    if wiping && !probe.wiping {
        (probe.airborne, probe.grind) = (None, None);
        // A sequence a bail ended was the one after the return.
        probe.marked &= !(open || was_open);
    }
    probe.wiping = wiping;
    if s.in_air() && !wiping {
        let a = probe.airborne.get_or_insert(Air { from: pos, at: *clock, peak: pos[1], spin: 0., flips: false, uses });
        a.peak = a.peak.max(pos[1]);
        a.spin = a.spin.max(s.spin_turns.unsigned_abs() as f32 * 180.);
        a.flips |= s.flip_seen;
    } else if let Some(a) = probe.airborne.take().filter(|a| !wiping && a.uses == uses) {
        let distance = Vec2::new(pos[0] - a.from[0], pos[2] - a.from[2]).length();
        let measures = [distance, 0., a.peak - a.from[1], a.spin, *clock - a.at, if a.flips { 1. } else { 0. }];
        probe.seen.push(Seen::Air(measures.map(f64::from)));
    }
    if s.grinding() && !wiping {
        let g = probe.grind.get_or_insert((pos, 0., uses));
        g.1 += Vec3::from(g.0).distance(Vec3::from(pos));
        g.0 = pos;
    } else if let Some((_, length, _)) = probe.grind.take().filter(|g| !wiping && g.2 == uses) {
        probe.seen.push(Seen::Grind(f64::from(length)));
    }
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<ContestGame>| {
        let Some(data) = game.data.take() else { return };
        step(world, &mut game, &data);
        game.data = Some(data);
    });
}

/// SequenceConnector.SetContestRound(n) stores n - 1 for the score multiplier gate (82856CF8; online_scoring);
/// 0 again when the contest ends.
fn set_contest_round(world: &mut World, round: usize) {
    if let Some(mut rules) = world.get_resource_mut::<crate::online_scoring::ScoringRules>()
        && rules.contest_round != round.saturating_sub(1)
    {
        rules.contest_round = round.saturating_sub(1);
    }
}

/// endchallenge_teams' data block (skate3_modes::team_block) for a team contest. The round's results
/// (results_screen 2284-2295, 2319-2384): the team rows SetPlayerData(k, rank, roundScore, roundScore, 0, k,
/// totalScore) under TeamChallengeInt / Float, the members by round score. The standings (end_options 1968-1997):
/// IsFinalResults, the rows SetPlayerData(k, rank, totalScore), the members under Name. More rounds won ranks 1, a tie
/// ranks both 2.
fn team_results(data: &ModeData, g: &Contest, names: &[String], title: &str, online: bool, round: bool) -> serde_json::Value {
    let [a, b] = [g.teams[0].total_score, g.teams[1].total_score];
    let ranks = [if a > b { 1 } else { 2 }, if a < b { 1 } else { 2 }];
    let floats = round && g.cur().is_some_and(|r| !r.use_int());
    // TeamChallengeInt 10, TeamChallengeFloat 11, Name 13 (endchallenge_teams' eResultsLBType).
    let layout = if !round { 13 } else if floats { 11 } else { 10 };
    // 0x825BD398's score cell: the float +12 under layout 11 (0x825BCC10: "%.1f" into ID_COMMON_DISTANCE), else the
    // int +4 (the standings pass 0). Column 3 stays empty: the table draws three columns under these layouts.
    let cell = |v: f64| if floats { format!("#{}", distance(data, v)) } else { skate3_modes::team_int(data, if round { v as i64 } else { 0 }) };
    let table = |k: usize| skate3_modes::TeamTable {
        name: g.teams[k].name.clone(),
        rank: ranks[k],
        score: if !round {
            skate3_modes::team_int(data, g.teams[k].total_score)
        } else if floats {
            format!("#{}", distance(data, g.teams[k].round_score))
        } else {
            skate3_modes::team_int(data, g.teams[k].round_score as i64)
        },
        status: skate3_modes::team_int(data, if round { g.teams[k].total_score } else { 0 }),
        members: (g.team_standings[k].iter().enumerate())
            .map(|(i, s)| [skate3_modes::team_int(data, i as i64 + 1), format!("#{}", names.get(s.c).map_or("", String::as_str)), cell(s.score), String::new()])
            .collect(),
    };
    let team = g.comps.get(g.me).map_or(0, |c| c.team.saturating_sub(1)).min(1);
    let row = g.team_standings[team].iter().position(|s| s.c == g.me).map_or(-1, |i| i as i64);
    skate3_modes::team_block(online, title, !round, (team, row), layout, [table(0), table(1)])
}

/// A distance as HUDComponents.NumberToDistance writes it (0x826635f8: the value, at least 0, "%.1f" into
/// ID_COMMON_DISTANCE through the string formatter 0x8290c9a0).
fn distance(data: &ModeData, v: f64) -> String {
    data.fmt(&Text { id: "ID_COMMON_DISTANCE".into(), params: vec![format!("{:.1}", v.max(0.))] })
}

/// One relayed or probed event as the graph's message, its trick types and air measures set first.
fn deliver(play: &mut Play, from: usize, ev: RelayEv) {
    let msg = match ev {
        RelayEv::Trigger(v, inside) => Msg::Trigger { player: from, volume: v, inside },
        RelayEv::Begin => Msg::SequenceBegin(from),
        RelayEv::Sequence(score, types) => {
            play.g.types = types;
            Msg::Sequence { player: from, score, wiped: false }
        }
        RelayEv::Air(measures) => {
            play.g.air = measures;
            Msg::AirDone(from)
        }
        RelayEv::Grind(length) => Msg::GrindDone { player: from, length },
        RelayEv::End(_) => return,
    };
    skate3_modes::dispatch(&mut play.g, &mut play.rt, msg);
}

fn step(world: &mut World, game: &mut ContestGame, data: &ModeData) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && mode == Some(Mode::Contest) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 Contest: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
            game.warned = id;
        }
        return;
    }
    let row = row.expect("on_map");
    if game.play.is_none() {
        if !matches!(phase, Phase::Running { .. }) {
            world.resource_mut::<Match>().hold = true;
            return;
        }
        game.play = Some(begin(world, data, &id, row, now));
    }
    let play = game.play.as_mut().expect("begun");

    if play.online {
        let m = world.resource::<Match>();
        let left: Vec<usize> = (0..play.names.len()).filter(|&p| !play.rt.gone[p] && m.players.get(p).is_none_or(String::is_empty)).collect();
        for p in left {
            play.rt.gone[p] = true;
            skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
        }
    }
    if let Some(text) = play.tick(now) {
        world.resource_mut::<Match>().outbox.push(text);
    }
    set_contest_round(world, play.g.round);
    if let Some(r) = play.g.cur() {
        crate::online_scoring::set_round_trick_types(world, r.contest_variant);
    }

    // This console's skater: what it did, relayed.
    game.live = play.live();
    game.volumes = play.row.volumes();
    for s in std::mem::take(&mut game.probe.seen) {
        if !game.live {
            continue;
        }
        let ev = match s {
            Seen::Trigger(v, inside) => RelayEv::Trigger(v, inside),
            Seen::Begin => RelayEv::Begin,
            // Air_Only / Grind_Only: the scorer itself skips the disabled trick types (online_scoring::trick_types).
            Seen::Sequence(score, types) => RelayEv::Sequence(score, types),
            Seen::Air(m) => RelayEv::Air(m),
            Seen::Grind(l) => RelayEv::Grind(l),
        };
        if let Some(text) = play.local(ev) {
            world.resource_mut::<Match>().outbox.push(text);
        }
    }
    if play.online {
        let received: Vec<(usize, Relay)> =
            world.resource_mut::<Match>().inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect();
        play.receive(received);
    }
    if let Some(text) = play.tick(now) {
        world.resource_mut::<Match>().outbox.push(text);
    }
    // SessionMarker.SetSessionMarker (countdown 880): session_marker places it where the skater stands once the pause
    // gate lifts (the round's start).
    if std::mem::take(&mut play.g.set_marker)
        && let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>()
    {
        marker.request_set = true;
    }

    // RepositionAI: this skater to its start slot (the round's, or back from out of bounds).
    if let Some((seq, place)) = play.rt.landed(play.me)
        && play.placed != seq
    {
        play.placed = seq;
        if let Some(at) = play.row.rounds.get(place.index).and_then(|r| r.start_location.as_ref()).map(|l| l.slot(place.slot)) {
            world.resource_mut::<Match>().teleport = Some(at);
        }
    }

    // The round's intro NIS.
    let want = play.rt.presenting.filter(|&end| now < end).and(play.g.scene.clone());
    if want != play.playing {
        if want.is_some() {
            world.resource_mut::<Cutscene>().start(&data.root, want.as_deref().expect("some"));
        } else {
            world.resource_mut::<Cutscene>().stop();
        }
        world.resource_mut::<Match>().intro = want.is_some();
        play.playing = want;
    }
    // The loading screen: up until the first round's presentation ends (785), its NIS showing through it (R1). The
    // round fades (1671, 784) lift while a round's NIS plays (ponytail: the presentation's own transition is native).
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = play.rt.covered && play.playing.is_none();
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        let alpha = if play.playing.is_some() { 0. } else { play.rt.fade_alpha(now) as f32 };
        if fade.0 != alpha {
            fade.0 = alpha;
        }
    }

    // The leaderboard's score cells: ints grouped (the movie's default), a float round's as distances.
    // ponytail: LeaderboardHUDSetFloat's text is the native's (not traced); written as NumberToDistance writes.
    if play.rt.hud.board.visible {
        let floats = play.g.floats_round();
        play.rt.hud.board.texts = (play.g.comps.iter())
            .map(|c| if floats && c.round_score >= 0. { format!("#{}", distance(data, c.round_score)) } else { String::new() })
            .collect();
        play.rt.hud.board.team_texts = play.g.teams.each_ref().map(|t| if floats { format!("#{}", distance(data, t.round_score)) } else { String::new() });
    }
    // G8: the OOB static (1013 .. 1193) on the session marker's noise, as free skate's; none once the contest is over.
    let oob = if play.rt.left { 0. } else { play.rt.oob_static(now) };
    if let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>()
        && marker.oob != oob
    {
        marker.oob = oob;
    }
    crate::skate3_indicators::show(world, INDICATORS, if play.looks { play.g.markers() } else { Vec::new() });
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = play.rt.pinned.get(play.me).copied().unwrap_or(true) || play.rt.presenting_now();
        // The round results (results_screen) and the standings own the pad.
        m.gestures = skate3_modes::gestures(&m, play.g.results_round || play.rt.left);
        m.watch = None;
        m.score = play.g.comps.get(play.me).map_or(0, |c| c.total_score);
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
        // The round sync points: ours out, passed once every console has reached them.
        m.round = play.rt.syncs;
        play.rt.release(&m.rounds);
    }
    // The round's results during results_screen (6 + 1 s), the standings once the contest is over.
    let want = if play.g.results_round { Some((true, play.g.round)) } else if play.rt.left { Some((false, 0)) } else { None };
    if want != play.showing {
        play.hud.reset_results(world);
        play.showing = want;
    }
    // Teams: the results rows are the two teams (0 / 1), this player's team the highlighted one; the leaderboard is
    // hidden then (HUD2.ShowGroup(false) at end_round).
    let teams = play.g.teams_on && want.is_some();
    let team_view: Vec<String> = play.g.teams.iter().map(|t| t.name.clone()).chain(play.names.iter().skip(2).cloned()).collect();
    let my_team = play.g.comps.get(play.me).map_or(0, |c| c.team.saturating_sub(1));
    let view = if teams {
        View { names: &team_view, me: my_team, online: play.online, title: data.text(&play.row.title), label: "Contest" }
    } else {
        View { names: &play.names, me: play.me, online: play.online, title: data.text(&play.row.title), label: "Contest" }
    };
    let results = want.map(|(round, _)| {
        let r = play.g.cur().cloned();
        let (title, layout) = match (round, &r) {
            // 2275 / 1938: the FinalResults group's title over the team table.
            _ if teams => ("ID_CHALLENGE_COMMON_CONTEST_STANDINGS", TEAM_RESULTS),
            (true, Some(r)) if !r.use_int() => ("ID_CHALLENGE_COMMON_ROUND_RESULTS", NON_ELIM_FLOAT),
            (true, _) => ("ID_CHALLENGE_COMMON_ROUND_RESULTS", NON_ELIM_INT),
            (false, _) => ("ID_CHALLENGE_COMMON_CONTEST_STANDINGS", NON_ELIM_INT),
        };
        ResultsSetup {
            title: data.text(title),
            layout,
            columns: r.map_or_else(Vec::new, |r| [r.col1, r.col2, r.col3].iter().map(|c| data.text(c)).collect()),
            throwdown: 0,
            choice: None,
            times: Vec::new(),
            floats: if round { play.g.floats.iter().map(|&(p, v)| (p, v, format!("#{}", distance(data, v)))).collect() } else { Vec::new() },
            strings: Vec::new(),
        }
    });
    play.hud.teams = want.filter(|_| teams).map(|(round, _)| team_results(data, &play.g, &play.names, &view.title, play.online, round));
    play.hud.drive(world, data, &play.rt.hud, &view, results.as_ref(), now);
}

fn begin(world: &mut World, data: &ModeData, id: &str, row: Row, now: f64) -> Play {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (vec!["Player 1".to_owned()], 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let intros = row.rounds.iter().map(|r| r.intro.as_ref().and_then(|s| data.intros.get(s)).map_or(0., |&l| f64::from(l))).collect();
    info!("Skate 3 Contest: {id} ({}), {} skaters, this one {me}", if online { "online" } else { "solo" }, names.len());
    let mut g = Contest::new(row.clone(), intros, names.len(), me);
    // A team lobby's teams (match_core::Match::teams, the host's split); Teams.GetName's EA Online team names are gone:
    // retail's unranked "Red Team" / "Blue Team" (online_moves::TEAM_NAMES).
    if online && m.teams.len() == names.len() {
        g.teams_in = m.teams.clone();
    }
    g.team_names = crate::online_moves::TEAM_NAMES.map(|id| data.text(id));
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.online = online;
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    skate3_modes::start(&mut g, &mut rt);
    let looks = data.root.join("indicators.json").is_file();
    Play { id: id.to_owned(), row, g, rt, names, me, online, hud: HudDriver::default(), inbox: Vec::new(), placed: 0, playing: None, showing: None, sync: EndSync::default(), looks }
}

fn end(world: &mut World, game: &mut ContestGame) {
    crate::skate3_indicators::show(world, INDICATORS, Vec::new());
    if let Some(play) = game.play.take() {
        info!("Skate 3 Contest: {} ended", play.id);
        if play.playing.is_some() {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
    }
    game.live = false;
    set_contest_round(world, 0);
    crate::online_scoring::challenge_ended(world);
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = false;
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        fade.0 = 0.;
    }
    if let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>() {
        marker.oob = 0.;
    }
    HudDriver::close(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};

    fn vol(name: &str) -> Option<RaceVolume> {
        Some(RaceVolume { name: name.into(), min: [0.; 3], max: [1.; 3] })
    }

    /// A round: `spots` spot volumes s<r>_k, the boundary b<r>, an AI out-of-bounds volume a<r>_1 when `ai`.
    fn round(r: usize, v: u32, run_type: u32, cumulative: bool, time: i64, online: Option<i64>, spots: usize, ai: bool, min: i64) -> Round {
        Round {
            contest_variant: v,
            run_type,
            cumulative_scoring: cumulative,
            cumulative_min_score: min,
            round_time: time,
            round_time_online: online,
            start_location: Some(Locator { matrix: [[0.; 4]; 4], slots: vec![[[0.; 4]; 4]; 6] }),
            spot_volumes: (1..=spots).map(|k| vol(&format!("s{r}_{k}"))).collect(),
            round_boundary: vol(&format!("b{r}")),
            path_end_volume: None,
            restart_last_chance_volume: vec![],
            ai_out_of_bounds: ai.then(|| vec![vol(&format!("a{r}_1"))]),
            audio_out_of_bounds_warning_time: 1,
            audio_round_end_warning_time: 11,
            col1: "ID_COMMON_RANK".into(),
            col2: "ID_COMMON_NAME".into(),
            col3: "ID_COMMON_SCORE".into(),
            intro: Some(format!("intro{r}")),
            visual_indicators: Vec::new(),
        }
    }

    fn row(rounds: Vec<Round>) -> Row {
        Row {
            title: "ID_T".into(),
            online: true,
            challenge_type: Some(STREET),
            elimination: None,
            ai_online: None,
            final_round: "ID_FINAL".into(),
            round_num: "ID_ROUND".into(),
            return_to_area: "ID_RETURN".into(),
            new_lead: None,
            new_lead_you: None,
            team_intro_delay: 3.,
            team_indication: "ID_TEAM".into(),
            intro_nis: Some("intro".into()),
            online_intro_nis: None,
            online_outro_nis: Some("outro".into()),
            rounds,
        }
    }

    /// Online audit 10-12: `n` consoles play a two-round Street contest through the match relay
    /// ([`skate3_modes::tests::Wire`]), started up to 0.3 s apart; each skater lands its `plan`'s (round, round clock,
    /// score) sequences on its own console; `leaver` (player, second-round clock) quits then (what it sent still on the
    /// way is lost; the others notice it 0.5 s + 0.4 s per slot later, its sync count then u32::MAX as match_core
    /// passes a gone slot's). Every staying console's round standings (skater, score) as each results screen opens,
    /// and its final standings (skater, points, rank).
    #[allow(clippy::type_complexity)]
    fn contest_over_the_wire(n: usize, seed: u64, latency: (f64, f64), loss: f64, plan: &dyn Fn(usize) -> Vec<(usize, f64, i64)>, leaver: Option<(usize, f64)>) -> Vec<(Vec<Vec<(usize, i64)>>, Vec<(usize, i64, i64)>)> {
        contest_over_the_wire_teams(n, seed, latency, loss, plan, leaver, &[]).into_iter().map(|(r, s, _)| (r, s)).collect()
    }
    /// [`contest_over_the_wire`] with `teams` (each player's team, 1 or 2): every console's teams (rounds won, rank) too.
    #[allow(clippy::type_complexity)]
    fn contest_over_the_wire_teams(
        n: usize,
        seed: u64,
        latency: (f64, f64),
        loss: f64,
        plan: &dyn Fn(usize) -> Vec<(usize, f64, i64)>,
        leaver: Option<(usize, f64)>,
        teams: &[u8],
    ) -> Vec<(Vec<Vec<(usize, i64)>>, Vec<(usize, i64, i64)>, [(i64, i64); 2])> {
        let r = row(vec![round(1, SEQUENCE, 0, false, 30, None, 0, false, 0), round(2, SEQUENCE, 0, true, 30, None, 0, false, 0)]);
        let mut wire = skate3_modes::tests::Wire::new(seed, latency, loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        let mut fed = vec![0; n];
        let mut round_start: Vec<Option<(usize, f64)>> = vec![None; n];
        let mut rounds: Vec<Vec<Vec<(usize, i64)>>> = vec![Vec::new(); n];
        let mut reached: Vec<Vec<f64>> = vec![Vec::new(); n];
        let plans: Vec<Vec<(usize, f64, i64)>> = (0..n).map(plan).collect();
        let mut left_at: Option<f64> = None;
        for frame in 0..60 * 300 {
            let t = frame as f64 / 60.;
            for i in 0..n {
                let quit = leaver.is_some_and(|(p, _)| p == i);
                if t < starts[i] || (quit && left_at.is_some()) {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| {
                    let mut g = Contest::new(r.clone(), vec![0.; 2], n, i);
                    g.teams_in = teams.to_vec();
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    skate3_modes::start(&mut g, &mut rt);
                    let names = (0..n).map(|p| format!("P{p}")).collect();
                    Play { id: "c".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), inbox: Vec::new(), placed: 0, playing: None, showing: None, sync: EndSync::default(), looks: false }
                });
                if let Some((p, clock)) = leaver
                    && quit
                    && round_start[i].is_some_and(|(rd, at)| rd == 1 && t - at >= clock)
                {
                    left_at = Some(t);
                    (0..n).for_each(|to| wire.cut(p, to));
                    continue;
                }
                if let (Some((p, _)), Some(left)) = (leaver, left_at)
                    && t >= left + 0.5 + 0.4 * i as f64
                    && !play.rt.gone[p]
                {
                    play.rt.gone[p] = true;
                    skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
                }
                // The round sync points: the others' counts reach this console 0.25 s after they get there
                // (match_core's @score), as match_core passes them.
                let syncs: Vec<u32> = (0..n)
                    .map(|j| match j {
                        _ if j == i => play.rt.syncs,
                        _ if play.rt.gone[j] => u32::MAX,
                        _ => reached[j].iter().filter(|&&at| at + 0.25 <= t).count() as u32,
                    })
                    .collect();
                play.rt.release(&syncs);
                let mut out: Vec<String> = play.tick(t).into_iter().collect();
                // Rounds count from 1 (setup_round); the plans' from 0.
                if play.rt.state == St::RunRound && round_start[i].is_none_or(|(r, _)| r + 1 != play.g.round) {
                    round_start[i] = Some((play.g.round - 1, t));
                }
                if play.rt.state == St::ResultsScreen && rounds[i].len() < play.g.round {
                    rounds[i].push(play.g.round_standings.iter().map(|s| (s.c, s.score as i64)).collect());
                }
                if play.live()
                    && let Some((rd, at)) = round_start[i]
                {
                    while fed[i] < plans[i].len() && plans[i][fed[i]].0 <= rd && (plans[i][fed[i]].0 < rd || t - at >= plans[i][fed[i]].1) {
                        let (pr, _, score) = plans[i][fed[i]];
                        if pr == rd {
                            out.extend(play.local(RelayEv::Sequence(score, 0)));
                        }
                        fed[i] += 1;
                    }
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                play.receive(received);
                out.extend(play.tick(t));
                while reached[i].len() < play.rt.syncs as usize {
                    reached[i].push(t);
                }
                wire.send(t, i, n, out);
            }
            let here = |i: &usize| left_at.is_none() || leaver.is_none_or(|(p, _)| p != *i);
            if (0..n).filter(here).all(|i| plays[i].as_ref().is_some_and(|p| p.rt.state == St::Complete)) {
                break;
            }
        }
        plays
            .iter()
            .enumerate()
            .filter(|(i, _)| leaver.is_none_or(|(p, _)| p != *i))
            .map(|(i, p)| {
                let p = p.as_ref().unwrap();
                assert_eq!(p.rt.state, St::Complete, "console {i} finished");
                (
                    rounds[i].clone(),
                    p.g.total_standings.iter().map(|s| (s.c, s.score as i64, s.rank)).collect(),
                    p.g.teams.each_ref().map(|t| (t.total_score, t.rank)),
                )
            })
            .collect()
    }

    /// Online audit 10-12 (Critical): every console shows the same round results and standings. Before the fix a
    /// sequence banked in a round's last moments reached the other consoles after their own round timer had ended the
    /// round, and was dropped there: the round went to someone else on their screens.
    #[test]
    fn every_console_scores_the_contest_the_same() {
        // Two skaters: player 1 banks the round's best 0.1 s before the buzzer.
        let late = |p: usize| if p == 1 { vec![(0, 29.9, 5000), (1, 29.9, 5000)] } else { vec![(0, 10., 3000), (1, 10., 3000)] };
        let two = contest_over_the_wire(2, 1, (0.15, 0.3), 0., &late, None);
        assert_eq!(two[0], two[1], "both consoles agree");
        assert_eq!(two[0].0[0][0], (1, 5000), "the buzzer-beater won round 1");
        // Six skaters, 10% loss, everyone banking through each round up to its last 50 ms.
        for seed in 1..=6u64 {
            let plan = move |p: usize| {
                let mut v = Vec::new();
                for rd in 0..2 {
                    for k in 0..6 {
                        v.push((rd, 3. + 5. * k as f64 + 0.01 * p as f64, 100 * (1 + ((seed as usize * 5 + p * 7 + k * 3 + rd) % 9) as i64)));
                    }
                    v.push((rd, 29.95, 1000 + 10 * p as i64));
                }
                v
            };
            let all = contest_over_the_wire(6, seed, (0.15, 0.3), 0.1, &plan, None);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
        }
    }

    /// Matrix Step 2 (10-12): six skaters, a guest (odd seeds) or the host (player 0, even seeds) leaving 10 s into
    /// round 2 after banking its best, 5-10 % loss, 150-300 ms. By hand: round 1 ranks by score (5 points down to 0,
    /// end_round 1563-1574); the leaver's totalScore becomes its rank at the leave, 6 - 1 + 1 = 6 (playerdisconnected
    /// 1823-1824); round 2's top score is the leaver's, so it takes the 5 nobody gets (inactive) and the stayers earn
    /// 4..0 by score.
    #[test]
    fn six_skaters_and_a_leaver_end_with_the_retail_standings() {
        for seed in 1..=6u64 {
            let quit = if seed % 2 == 0 { 0 } else { 3 };
            // Round 1: skater p banks 1000 (p + 1). Round 2: the stayers again; the leaver 9000 before it goes.
            let plan = move |p: usize| vec![(0, 5. + p as f64, 1000 * (p as i64 + 1)), if p == quit { (1, 3., 9000) } else { (1, 5. + p as f64, 1000 * (p as i64 + 1)) }];
            let all = contest_over_the_wire(6, seed, (0.15, 0.3), 0.05 + 0.01 * seed as f64, &plan, Some((quit, 10.)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            let first: Vec<(usize, i64)> = (0..6).rev().map(|p| (p, 1000 * (p as i64 + 1))).collect();
            let second: Vec<(usize, i64)> = [(quit, 9000)].into_iter().chain((0..6).rev().filter(|&p| p != quit).map(|p| (p, 1000 * (p as i64 + 1)))).collect();
            assert_eq!(all[0].0, [first, second], "seed {seed}: the round results");
            let want: Vec<(usize, i64, i64)> = if quit == 3 {
                // Round 1 points p; round 2: 5 4, 4 3, 2 2, 1 1, 0 0; the leaver 6.
                vec![(5, 9, 1), (4, 7, 2), (3, 6, 3), (2, 4, 4), (1, 2, 5), (0, 0, 6)]
            } else {
                vec![(5, 9, 1), (4, 7, 2), (0, 6, 3), (3, 5, 4), (2, 3, 5), (1, 1, 6)]
            };
            assert_eq!(all[0].1, want, "seed {seed}: the standings");
        }
    }

    /// Plans per round: every skater rolls in and out of the spots with sequences (some missing the round's trick
    /// type), airs and grinds; sometimes out of the boundary (back in time, or long enough to be sent back), into the
    /// AI out-of-bounds volume; rarely a skater leaving.
    fn plans(seed: u64, r: &Row, n: usize, me: usize, leaves: bool) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |k: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % k
        };
        let vols = r.volumes();
        let index = |name: &str| vols.iter().position(|v| v.name == name).unwrap() as i64;
        let mut out = Vec::new();
        for (ri, rd) in r.rounds.iter().enumerate() {
            let rn = ri + 1;
            let time = rd.round_time_online.filter(|&t| t > 0).unwrap_or(rd.round_time) as f64;
            let mut run = Vec::new();
            for p in 0..n {
                let w = p as f64;
                let mut t = 0.07 * (p + 1) as f64;
                while t < time - 2. {
                    t += 1. + next(40) as f64 / 10.;
                    let spot = (!rd.spot_volumes.is_empty()).then(|| format!("s{rn}_{}", 1 + next(rd.spot_volumes.len() as u64)));
                    match next(7) {
                        0..=3 => {
                            if let Some(s) = &spot
                                && next(5) != 0
                            {
                                run.push((t, Ev::Other("enter", index(s), w)));
                            }
                            run.push((t + 0.1, Ev::Other("begin", 0, w)));
                            run.push((t + 1.0, Ev::Other("seqt", [0, 1, 2, 3, 32, 33][next(6) as usize], w)));
                            run.push((t + 1.0, Ev::Other("seq", 100 + 50 * next(80) as i64, w)));
                            if let Some(s) = &spot {
                                run.push((t + 1.5, Ev::Other("exit", index(s), w)));
                            }
                        }
                        4 => run.push((t, Ev::Other("air", p as i64, 0.37 + next(90) as f64 / 7.))),
                        5 => run.push((t, Ev::Other("grind", p as i64, 1.13 + next(60) as f64 / 3.))),
                        _ => {
                            let b = format!("b{rn}");
                            run.push((t, Ev::Other("exit", index(&b), w)));
                            let back = if next(2) == 0 { 2. } else { 6. };
                            run.push((t + back, Ev::Other("enter", index(&b), w)));
                            if rd.ai_out_of_bounds.is_some() && next(3) == 0 {
                                run.push((t + 0.5, Ev::Other("enter", index(&format!("a{rn}_1")), w)));
                            }
                        }
                    }
                    // The run's path end (street_indu_02, vert_dwmc_01).
                    if let Some(e) = Round::name(&rd.path_end_volume)
                        && next(4) == 0
                    {
                        run.push((t + 0.6, Ev::Other("enter", index(e), w)));
                        run.push((t + 0.7, Ev::Other("exit", index(e), w)));
                    }
                }
            }
            if leaves && next(5) == 0 {
                let p = next(n as u64) as usize;
                if p != me {
                    run.push((1. + next((time * 10.) as u64) as f64 / 10., Ev::Leave(p)));
                }
            }
            out.push(run);
        }
        out
    }

    const SETUP: &str = r#"
local function list(t) local o = {Count = #t} for i, v in ipairs(t) do o[i] = v end return o end
local rounds = {Count = #ROUNDS}
for i, r in ipairs(ROUNDS) do
  local spots = {}
  for k = 1, r.spots do spots[k] = "s" .. i .. "_" .. k end
  local ai = nil
  if r.ai then ai = list({"a" .. i .. "_1"}) end
  rounds[i] = {ContestVariant = r.v, RunType = r.run, CumulativeScoring = r.cum, CumulativeMinScore = r.min,
    RoundTime = r.time, RoundTimeOnline = r.online, StartLocation = "StartLocation" .. (i - 1), SpotVolumes = list(spots),
    RoundBoundary = "b" .. i, PathEndVolume = r.pathend, RestartLastChanceVolume = {Count = 0}, RestartOnWipeout = false,
    AIOutOfBoundsVolumes = ai, VisualIndicators = {Count = 0}, RequiredChallengeHulls = {Count = 1},
    SpectatorWaypoints = {Count = 0}, LargeCrowdData = {Count = 0}, IntroPresentationEvents = {Count = 1},
    RoundPresentationEvents = {Count = 0}, AudioOutOfBoundsWarningTime = 1, AudioOutOfBoundsWarning = "w",
    AudioRoundEndWarningTime = 11, AudioLeadEarlyTime = 1, AudioLeadLateTime = 20, AudioNewLeaderTime = 4,
    ResultsScreenHALIDCol1 = "ID_COMMON_RANK", ResultsScreenHALIDCol2 = "ID_COMMON_NAME",
    ResultsScreenHALIDCol3 = "ID_COMMON_SCORE", ResultsScreenHALIDCol4 = "ID_COMMON_STATUS", HighlightBest = "h",
    AIScoreAdjustment = 1}
end
SELF = setmetatable({Name = "contest", Title = "ID_T", ChallengeType = 8, ContestRounds = rounds,
  CentralMessageHALIDFinalRound = "ID_FINAL", CentralMessageHALIDRoundNum = "ID_ROUND",
  CentralMessageHALIDReturnToChallengeArea = "ID_RETURN", TeamIntroductionDelay = 3,
  CentralMessageHALIDTeamIndication = "ID_TEAM",
  IntroPresentationEvents = {Count = 0}, OutroPresentationEvents = {Count = 0}, TeamNames = {Count = 0},
  IntroNIS = "intro", OutroNIS = "outro", OnlineOutroNIS = "outro"}, {__index = function() return nil end})
Challenge.eChallengeTypes = {Demo = 2, Street = 8, Vert = 15}
Contest = setmetatable({eContestType = {Sequence = 0, Sequence_Grab = 1, Sequence_Grind = 2, Sequence_Flip = 3,
  Sequence_Footplant = 4, Sequence_Handplant = 5, Sequence_Hippyjump = 6, Grind_Length = 7, Speed = 8,
  Air_Distance = 9, Air_Grab_Time = 10, Air_Height = 11, Air_Spin = 12, Air_Time = 13, Best_Line = 14, Air_Only = 15,
  Grind_Only = 16, Unknown = 17},
  eRunType = {None = 0, SoloRun = 1, SoloRunCycle = 2, TimedRunSingle = 3, TimedRunMulti = 4}}, getmetatable(Online))
SequenceConnector.SetContestRound = function(n) log_line("contestround " .. n) end
HUD2.eLeaderboardHUDMenuType.RankNameScore = 3
HUD2.eLeaderboardHUDMenuType.RankNameFloatscore = 6
HUD2.LeaderboardHUDSetFloat = function(_, id, v) log_line(string.format("float %d %.3f", id, v)) end
HUD2.LeaderboardHUDSetVariantType = function(_, v, best) log_line(string.format("varianttype %d %s", v, tostring(best))) end
local modified = HUD2.ShowModifiedCentralMessage
HUD2.ShowModifiedCentralMessage = function(t, ...) modified(tostring(t), ...) end
Effects.eChallengeEffect.OOB = "OOB"
FrontEnd.eResultsLBType.BestTrickNonElimFloat = 4
FrontEnd.eResultsLBType.BestTrickNonElimInt = 5
FrontEnd.Results_SetPlayerData = function(id, rank, v, f)
  log_line(string.format("result %d %d %d", id, rank, v))
  if f then log_line(string.format("resultf %d %.3f", id, f)) end
end
FrontEnd.Results_SetSecondLBPlayerData = function(id, _, rank, v, f)
  log_line(string.format("result2 %d %d %d", id, rank, v))
  if f then log_line(string.format("resultf %d %.3f", id, f)) end
end
Online.OnlineChallengeComplete = function(r, t)
  for _, x in ipairs(r) do log_line(string.format("report %d %d %d %d %d", x[1], x[2], x[3], x[7], x[8])) end
  for i, x in ipairs(t or {}) do log_line(string.format("teamreport %d %d %d", i, x[1], x[2])) end
end
local bits = {Grab = 1, Grind = 2, Footplant = 4, Handplant = 8, HippyJump = 16}
Trick.eTrickType = {Air = "Air", FingerFlip = "FingerFlip", Flip = "Flip", Grab = "Grab", Grind = "Grind",
  Footplant = "Footplant", Handplant = "Handplant", HippyJump = "HippyJump"}
TYPES, AIRV = 0, 0
Trick.GetNumTrickType = function(_, t) if (TYPES // (bits[t] or 64)) % 2 == 1 then return 1 end return 0 end
Trick.GetCompletedAirHorizontalDistance = function() return AIRV end
Trick.GetCompletedAirGrabTime = function() return AIRV end
Trick.GetCompletedAirHeight = function() return AIRV end
Trick.GetCompletedAirDegreesOfSpin = function() return AIRV end
Trick.GetCompletedAirTime = function() return AIRV end
Trick.GetCompletedAirNumFlips = function() return 0 end
function ACTOR() return nil end
function DELIVER(kind, v, w)
  local p = math.tointeger(w)
  if kind == "enter" then SEND("triggerentercollision", {PlayerID = p, VolumeID = VOLS[v + 1]}); return true end
  if kind == "exit" then SEND("triggerexitcollision", {PlayerID = p, VolumeID = VOLS[v + 1]}); return true end
  if kind == "begin" then SEND("sequencebegin", {PlayerID = p}); return true end
  if kind == "seqt" then TYPES = v; return true end
  if kind == "seq" then SEND("sequencecomplete", {PlayerID = p, SequenceScore = v, Wipedout = false}); return true end
  if kind == "air" then AIRV = w; SEND("aircompleted", {PlayerID = v, Wipedout = 0}); return true end
  if kind == "grind" then SEND("grindcompleted", {PlayerID = v, GrindLength = w, Wipedout = 0}); return true end
  return false
end
"#;

    fn oracle(src: &str, n: usize, me: usize, r: &Row, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        team_oracle(src, n, me, r, plans, &[])
    }

    /// The oracle with Online.IsTeamChallenge true and Online.GetTeam answering `teams` (empty: no teams).
    fn team_oracle(src: &str, n: usize, me: usize, r: &Row, plans: &[Vec<(f64, Ev)>], teams: &[u8]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let vols: Vec<String> = r.volumes().iter().map(|v| format!("{:?}", v.name)).collect();
        let rounds: Vec<String> = (r.rounds.iter())
            .map(|d| {
                format!(
                    "{{v = {}, run = {}, cum = {}, min = {}, time = {}, online = {}, spots = {}, ai = {}, pathend = {}}}",
                    d.contest_variant,
                    d.run_type,
                    d.cumulative_scoring,
                    d.cumulative_min_score,
                    d.round_time,
                    d.round_time_online.map_or("nil".into(), |t| t.to_string()),
                    d.spot_volumes.len(),
                    d.ai_out_of_bounds.is_some(),
                    Round::name(&d.path_end_volume).map_or("nil".into(), |n| format!("{n:?}"))
                )
            })
            .collect();
        let mut setup = format!("N, ME, FIRST = {n}, {me}, 0\nROUNDS = {{{}}}\nVOLS = {{{}}}\n{SETUP}", rounds.join(", "), vols.join(", "));
        if !teams.is_empty() {
            let t: Vec<String> = teams.iter().enumerate().map(|(i, t)| format!("[{i}] = {t}")).collect();
            setup.push_str(&format!("TEAMS = {{{}}}\n", t.join(", ")));
        }
        if r.online_outro_nis.is_none() {
            setup.push_str("rawset(SELF, \"OnlineOutroNIS\", nil)\n");
        }
        let (lua, errors) = lua_log(src, &setup, "initial", "run_round", plans);
        let mut g = Contest::new(r.clone(), vec![0.; r.rounds.len()], n, me);
        g.teams_in = teams.to_vec();
        let mut rt = Rt::new(St::Initial, n, 0.);
        let other = |g: &mut Contest, _: &mut Rt<St>, kind: &str, v: i64, w: f64| match kind {
            "enter" | "exit" => Some(Msg::Trigger { player: w as usize, volume: v, inside: kind == "enter" }),
            "begin" => Some(Msg::SequenceBegin(w as usize)),
            "seqt" => {
                g.types = v as u32;
                None
            }
            "seq" => Some(Msg::Sequence { player: w as usize, score: v, wiped: false }),
            "air" => {
                g.air = [w, w, w, w, w, 0.];
                Some(Msg::AirDone(v as usize))
            }
            "grind" => Some(Msg::GrindDone { player: v as usize, length: w }),
            _ => None,
        };
        let model = model_log(&mut g, &mut rt, St::RunRound, plans, |_| None, |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// contest_shared.lua itself (S3_CONTEST_DIR = tools/skate3_online.py's `--out`/contest, with scripts/) decides
    /// every contest the model runs the same way: every state, timer, teleport, pin, HUD2 call, score, rank, message,
    /// result and report; 1 to 6 skaters, 1 to 3 rounds of the online variants (best or cumulative, int or float),
    /// spot volumes or none, the boundary's warning and send-back, the AI out-of-bounds volumes, RoundTimeOnline,
    /// skaters leaving mid-round.
    #[test]
    #[ignore = "requires the decompiled contest_shared.lua (S3_CONTEST_DIR)"]
    fn skate3_contest_matches_contest_shared_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_CONTEST_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/contest_shared.lua")).unwrap();
        let variants = [SEQUENCE, GRAB, GRIND, GRIND_LENGTH, AIR_DISTANCE, AIR_HEIGHT, AIR_ONLY, AIR_SPIN, FLIP];
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=50u64 {
            for n in 1..=6usize {
                let s = seed as usize;
                let me = s % n;
                let count = 1 + (s + n) % 3;
                let rounds = (0..count)
                    .map(|i| {
                        let k = s * 7 + i * 3 + n;
                        let online = (k % 5 == 1).then_some(40);
                        let run = [0, 0, 1, 4, 5, 3, 2][k % 7];
                        let mut r = round(i + 1, variants[k % variants.len()], run, k % 3 != 0, [30, 20, 25][k % 3], online, k % 4, k % 6 == 2, if k % 7 == 3 { 400 } else { 0 });
                        // A PathEndVolume on about half the rounds, every run type (Lead 10-06: street_indu_02, vert_dwmc_01).
                        if (k / 7) % 2 == 0 {
                            r.path_end_volume = vol(&format!("e{}", i + 1));
                        }
                        r
                    })
                    .collect();
                let r = row(rounds);
                let plans = plans(seed * 131 + n as u64, &r, n, me, seed % 3 == 0);
                let (lua, model, errs) = oracle(&src, n, me, &r, &plans);
                compare(&format!("seed {seed} n {n} me {me}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("report")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("contest_shared.lua oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// contest_shared.lua's TeamsEnabled path (Online.IsTeamChallenge, Online.GetTeam) decides every team contest the
    /// model runs the same way: the team intro message and its delay, team round scores and ranks, the round point,
    /// the team results and report, the team NIS; 2 to 6 skaters in even, uneven and one-sided splits, skaters leaving.
    #[test]
    #[ignore = "requires the decompiled contest_shared.lua (S3_CONTEST_DIR)"]
    fn skate3_contest_teams_match_contest_shared_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_CONTEST_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/contest_shared.lua")).unwrap();
        let variants = [SEQUENCE, GRAB, GRIND, GRIND_LENGTH, AIR_DISTANCE, AIR_HEIGHT, AIR_ONLY, AIR_SPIN, FLIP];
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let s = seed as usize;
                let me = s % n;
                let count = 1 + (s + n) % 3;
                let rounds = (0..count)
                    .map(|i| {
                        let k = s * 5 + i * 7 + n;
                        let online = (k % 5 == 1).then_some(40);
                        round(i + 1, variants[k % variants.len()], [0, 0, 1, 4, 5][k % 5], k % 3 != 0, [30, 20, 25][k % 3], online, k % 4, k % 6 == 2, if k % 7 == 3 { 400 } else { 0 })
                    })
                    .collect();
                let mut r = row(rounds);
                if seed % 4 == 0 {
                    r.online_outro_nis = None;
                }
                // Alternating, first half / second half, or everyone on one team.
                let teams: Vec<u8> = (0..n)
                    .map(|i| match seed % 3 {
                        0 => 1 + (i % 2) as u8,
                        1 => if i < (n + 1) / 2 { 1 } else { 2 },
                        _ => if seed % 9 == 2 { 2 } else { 1 + ((i + s) % 2) as u8 },
                    })
                    .collect();
                let plans = plans(seed * 173 + n as u64, &r, n, me, seed % 3 == 1);
                let (lua, model, errs) = team_oracle(&src, n, me, &r, &plans, &teams);
                compare(&format!("seed {seed} n {n} me {me} teams {teams:?}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("teamreport")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("contest_shared.lua team oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// Two teams by hand: a member's best adds to the team's round score, the team ahead takes the round's point, the
    /// team results show the two teams; the first countdown waits on the team message.
    #[test]
    fn teams_score_together_and_take_the_round() {
        let r = row(vec![round(1, SEQUENCE, 0, false, 30, None, 0, false, 0), round(2, SEQUENCE, 0, true, 30, None, 0, false, 0)]);
        let mut g = Contest::new(r, vec![0., 0.], 4, 1);
        g.teams_in = vec![1, 2, 1, 2];
        let mut rt = Rt::new(St::Initial, 4, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunRound);
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("teamcentral ID_TEAM 1 0 T2")), "this player's team, blue: {log:?}");
        assert!(log.iter().any(|l| l.ends_with("board 3 4 teams")));
        for (p, s) in [(0, 500), (2, 300), (1, 700), (3, 50)] {
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: p, score: s, wiped: false });
        }
        assert_eq!(g.teams.each_ref().map(|t| t.round_score), [800., 750.]);
        assert_eq!(g.teams.each_ref().map(|t| t.rank), [1, 2]);
        until(&mut g, &mut rt, St::ResultsScreen);
        assert_eq!(g.teams.each_ref().map(|t| t.total_score), [1, 0], "the team ahead takes the round");
        assert_eq!(rt.hud.results, vec![(0, 1, 800), (1, 2, 750)], "the team table");
        until(&mut g, &mut rt, St::RunRound);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 3, score: 900, wiped: false });
        until(&mut g, &mut rt, St::Complete);
        assert_eq!(g.teams.each_ref().map(|t| t.total_score), [1, 1]);
        assert!(!g.won, "a 1-1 tie: no win");
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with("teamreport 2 2 1")));
    }

    /// Matrix Step 2 (10-12): a team contest at 6 (3 a side, teams 1 2 1 2 1 2), 150-300 ms, 5-10 % loss, no leaver, a
    /// guest (3) or the host (0) leaving 10 s into round 2 after banking 12000. By hand (team_add: a member's best /
    /// banked points add to its team's round score, playerdisconnected leaves TeamData alone; the team ahead takes the
    /// round; end_options: more rounds won ranks 1, a tie both 2). Round 1, best 1000 (p + 1): 9000 to 12000, team 2.
    /// Round 2, banked: even 4000, odd 1000; team 1 12000 to 3000 (no leaver: 1-1, both 2); player 3's 12000 makes team
    /// 2 14000: 0-2; player 0's makes team 1 20000: 1-1.
    #[test]
    fn six_skaters_in_two_teams_take_the_contest_as_retail() {
        let teams = [1u8, 2, 1, 2, 1, 2];
        for seed in 1..=6u64 {
            let leaver = match seed % 3 {
                0 => None,
                1 => Some((3, 10.)),
                _ => Some((0, 10.)),
            };
            let quit = leaver.map(|l| l.0);
            let plan = move |p: usize| {
                let second = if Some(p) == quit { (1, 3., 12000) } else { (1, 5. + p as f64, if p % 2 == 0 { 4000 } else { 1000 }) };
                vec![(0, 5. + p as f64, 1000 * (p as i64 + 1)), second]
            };
            let all = contest_over_the_wire_teams(6, seed, (0.15, 0.3), 0.05 + 0.01 * (seed - 1) as f64, &plan, leaver, &teams);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            let want = if quit == Some(3) { [(0, 2), (2, 1)] } else { [(1, 2), (1, 2)] };
            assert_eq!(all[0].2, want, "seed {seed} leaver {leaver:?}: the teams (rounds won, rank)");
        }
    }

    fn until(g: &mut Contest, rt: &mut Rt<St>, state: St) {
        for _ in 0..1000 {
            if rt.state == state {
                return;
            }
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}", rt.state));
            skate3_modes::advance(g, rt, t);
        }
        panic!("never reached {state:?}");
    }

    /// s2rules 6 (online audit 10-28): countdown's stateenter calls SessionMarker.SetSessionMarker every round (880),
    /// so each round's marker is its start; the glue hands it to session_marker.
    #[test]
    fn each_rounds_countdown_sets_the_session_marker() {
        let r = row(vec![round(1, SEQUENCE, 0, false, 30, None, 0, false, 0), round(2, SEQUENCE, 0, false, 30, None, 0, false, 0)]);
        let mut g = Contest::new(r, vec![0., 0.], 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::Countdown);
        assert!(std::mem::take(&mut g.set_marker), "round 1's countdown");
        until(&mut g, &mut rt, St::RunRound);
        assert!(!g.set_marker, "once a round");
        until(&mut g, &mut rt, St::Countdown);
        assert_eq!((g.round, g.set_marker), (2, true), "round 2's countdown");
    }

    /// Two rounds by hand: best-sequence then cumulative; the best scores keep the max, the cumulative sums; places
    /// give n - place points, the standings decide the win.
    #[test]
    fn rounds_score_places_and_the_standings_win() {
        let r = row(vec![round(1, SEQUENCE, 0, false, 30, None, 0, false, 0), round(2, GRAB, 0, true, 30, Some(45), 0, false, 0)]);
        let mut g = Contest::new(r, vec![2., 2.], 3, 1);
        let mut rt = Rt::new(St::Initial, 3, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunRound);
        assert_eq!(g.round, 1);
        assert_eq!(rt.places[1].map(|p| p.1), Some(Place { field: START, index: 0, slot: 1 }), "slot = player id");
        for (p, s) in [(0, 500), (1, 900), (1, 300), (2, 700)] {
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: p, score: s, wiped: false });
        }
        assert_eq!(g.comps.iter().map(|c| c.round_score).collect::<Vec<_>>(), [500., 900., 700.], "best sequence");
        assert_eq!(g.comps.iter().map(|c| c.round_rank).collect::<Vec<_>>(), [3, 1, 2]);
        until(&mut g, &mut rt, St::SetupRound);
        assert_eq!(g.comps.iter().map(|c| c.total_score).collect::<Vec<_>>(), [0, 2, 1], "n - place points");
        until(&mut g, &mut rt, St::RunRound);
        let started = rt.now;
        g.types = T_GRAB;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: 400, wiped: false });
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: 400, wiped: false });
        g.types = 0;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 1, score: 2000, wiped: false });
        assert_eq!(g.comps[0].round_score, 800., "a grab round sums sequences with a grab");
        assert_eq!(g.comps[1].round_score, 0., "a sequence without a grab scores nothing");
        until(&mut g, &mut rt, St::ResultsScreen);
        let ended = rt.log.as_ref().unwrap().iter().rev().find(|l| l.ends_with("enter end_round")).unwrap();
        assert_eq!(ended, &format!("{:.2} enter end_round", started + 45.), "RoundTimeOnline");
        until(&mut g, &mut rt, St::Complete);
        assert_eq!(g.comps.iter().map(|c| c.total_score).collect::<Vec<_>>(), [2, 3, 2], "the 0-0 tie shares second's point");
        assert!(g.won, "player 1 tops the standings");
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("central ID_ROUND 0 1 0")), "round 1 of 2");
        assert!(log.iter().any(|l| l.ends_with("central ID_FINAL 1  0")), "the final round in red");
        assert!(log.iter().any(|l| l.ends_with("report 1 1 3 1 1")), "{log:?}");
        assert!(log.iter().any(|l| l.ends_with(" leave")));
    }

    /// Lead 10-06 (street_indu_02 round 2, vert_dwmc_01 round 3 set a PathEndVolume; playable() refused them): the path
    /// end (1234-1272). TimedRunMulti sends the skater to the next start slot, round robin; this console's behind
    /// RepositionAI's default fade (it lands at black), another's at once. TimedRunSingle ends this console's round.
    #[test]
    fn the_path_end_sends_the_skater_round_the_start_slots() {
        let mut multi = round(1, SEQUENCE, TIMED_RUN_MULTI, false, 60, None, 0, false, 0);
        multi.path_end_volume = vol("e1");
        let r = row(vec![multi]);
        assert!(r.playable("street_indu_02"));
        let e = r.volumes().iter().position(|v| v.name == "e1").unwrap() as i64;
        let mut g = Contest::new(r, vec![0.], 3, 1);
        let mut rt = Rt::new(St::Initial, 3, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunRound);
        let slot = |rt: &Rt<St>, p: usize| rt.places[p].map(|p| p.1.slot);
        assert_eq!(slot(&rt, 1), Some(1), "slot = player id");
        let at = rt.now;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: e, inside: true });
        assert_eq!(slot(&rt, 1), Some(2), "the next slot");
        assert_eq!(rt.landed(1).map(|p| p.1.slot), Some(1), "not moved in view as the fade starts");
        skate3_modes::advance(&mut g, &mut rt, at + 0.5);
        assert_eq!(rt.landed(1).map(|p| p.1.slot), Some(2), "at black");
        skate3_modes::advance(&mut g, &mut rt, at + 3.);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: e, inside: true });
        assert_eq!(slot(&rt, 1), Some(0), "round robin over the 3 competitors");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: e, inside: true });
        assert_eq!(rt.landed(0).map(|p| p.1.slot), Some(1), "another console's skater: at once");
        assert_eq!(rt.state, St::RunRound);

        let mut single = round(1, SEQUENCE, TIMED_RUN_SINGLE, false, 60, None, 0, false, 0);
        single.path_end_volume = vol("e1");
        let mut g = Contest::new(row(vec![single]), vec![0.], 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunRound);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: e, inside: true });
        assert!(rt.pinned[1] && rt.state == St::RunRound, "the other who got there is pinned");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: e, inside: true });
        assert_ne!(rt.state, St::RunRound, "this console's round ends at its path end");
    }

    /// Out of the boundary: Return To Challenge Area, then back to the start after the warning time + 3 s; a
    /// sequence out of bounds scores nothing.
    #[test]
    fn out_of_bounds_sends_the_skater_back() {
        let r = row(vec![round(1, SEQUENCE, 0, false, 60, None, 0, false, 0)]);
        let b = r.volumes().iter().position(|v| v.name == "b1").unwrap() as i64;
        let mut g = Contest::new(r, vec![0.], 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunRound);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: b, inside: false });
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Sequence { player: 0, score: 999, wiped: false });
        assert_eq!(g.comps[0].round_score, 0.);
        let moves = rt.places[0].map(|p| p.0);
        let t = rt.now + 4.;
        skate3_modes::advance(&mut g, &mut rt, t);
        assert_ne!(rt.places[0].map(|p| p.0), moves, "sent back to the start");
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with("central ID_RETURN 0  0")));
    }

    /// G8 (online audit 10-28): out of the boundary, the OOB static starts after AudioOutOfBoundsWarningTime (1012-1013),
    /// full over 5 s, and goes once the skater is back inside (1193). Before, StartEffect(OOB) was only logged.
    #[test]
    fn out_of_bounds_raises_the_static_until_back_inside() {
        let r = row(vec![round(1, SEQUENCE, 0, false, 60, None, 0, false, 0)]);
        let b = r.volumes().iter().position(|v| v.name == "b1").unwrap() as i64;
        let mut g = Contest::new(r, vec![0.], 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunRound);
        let out = rt.now;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: b, inside: false });
        skate3_modes::advance(&mut g, &mut rt, out + 0.5);
        assert_eq!(rt.oob_static(rt.now), 0., "the warning time first");
        skate3_modes::advance(&mut g, &mut rt, out + 3.5);
        assert!((rt.oob_static(rt.now) - 0.5).abs() < 1e-6, "2.5 s of 5: {}", rt.oob_static(rt.now));
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: b, inside: true });
        assert_eq!(rt.oob_static(rt.now), 0., "back inside");
    }

    /// G7 (online audit 10-28, round 2): each round's VisualIndicators are up from its setup_round (552-555) until its
    /// cleanup_round (1650-1653), in their own looks (SetVolumeHighlight's recolouring is Best_Line's only); none once
    /// the contest is over. An unplaced one draws nothing.
    #[test]
    fn each_rounds_markers_show_from_its_setup_until_its_cleanup() {
        use crate::skate3_race::{Chevron, Look};
        let at = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let chevron = |matrix, ribbon: &str| Chevron { matrix, look: Look { ribbon: ribbon.into(), style: "D7ED".into() } };
        let mut one = round(1, SEQUENCE, 0, false, 30, None, 0, false, 0);
        one.visual_indicators = vec![chevron(Some(at(1.)), "B094"), chevron(None, "B094")];
        let mut two = round(2, SEQUENCE, 0, false, 30, None, 0, false, 0);
        two.visual_indicators = vec![chevron(Some(at(2.)), "02CE")];
        let mut g = Contest::new(row(vec![one, two]), vec![2., 2.], 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        assert!(g.markers().is_empty(), "none before the first round");
        until(&mut g, &mut rt, St::SetupRound);
        assert_eq!(g.markers(), vec![("skate3:B094:D7ED".to_owned(), at(1.))], "round 1's, from its setup");
        until(&mut g, &mut rt, St::RunRound);
        assert_eq!(g.markers().len(), 1, "up while the round runs");
        until(&mut g, &mut rt, St::CleanupRound);
        assert!(g.markers().is_empty(), "the round's clean-up takes them down");
        until(&mut g, &mut rt, St::SetupRound);
        assert_eq!(g.markers(), vec![("skate3:02CE:D7ED".to_owned(), at(2.))], "round 2's own");
        until(&mut g, &mut rt, St::Complete);
        assert!(g.markers().is_empty(), "none after the contest");
    }

    /// SetContestRound's n - 1 reaches the score multiplier gate; 0 once the contest ends.
    #[test]
    fn the_contest_round_reaches_the_scoring_gate() {
        let mut world = World::new();
        world.insert_resource(crate::online_scoring::ScoringRules::default());
        set_contest_round(&mut world, 3);
        assert_eq!(world.resource::<crate::online_scoring::ScoringRules>().contest_round, 2);
        set_contest_round(&mut world, 0);
        assert_eq!(world.resource::<crate::online_scoring::ScoringRules>().contest_round, 0);
    }

    /// The tool's output (S3_CONTEST_DIR): the online rows load with their rounds, start slots, volumes and cameras.
    #[test]
    #[ignore = "requires the Skate 3 contest data (S3_CONTEST_DIR)"]
    fn skate3_contest_data() {
        let root = std::path::PathBuf::from(std::env::var("S3_CONTEST_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id)).cloned().collect();
        // 10-06 data: + street_indu_02 and vert_dwmc_01 (a PathEndVolume each), vert_indu_02/03, street_mmcn_01; and
        // street_mmcp_01 / vert_mmcp_01 (MaloofMoneyCup), vert_skp5_01 (University), refused before by map_of.
        assert_eq!(playable.len(), 23, "{playable:?}");
        for id in ["street_indu_02", "vert_dwmc_01"] {
            assert!(playable.iter().any(|p| p == id), "{id}");
        }
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            for d in &r.rounds {
                assert!(d.intro.as_ref().is_some_and(|s| data.intros.contains_key(s)), "{id}");
                assert!(!data.text(&d.col3).starts_with("ID_"), "{id}");
                let best = !d.cumulative_scoring;
                let v = format!("ID_CONTEST_VARIANT_{}{}", VARIANT_STRINGS[d.contest_variant as usize], if best { "" } else { "_JAM" });
                assert!(!data.text(&v).starts_with("ID_"), "{id}: {v}");
            }
        }
        assert!(!data.text("ID_COMMON_DISTANCE").starts_with("ID_"));
    }
}
