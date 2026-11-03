//! Skate 3's Death Race on the match core (challenge graph `race_shared` < race_based < default), the online
//! (non-team) path, for the online rows of DownTown, Industrial and University. Data: tools/skate3_online.py, installed
//! at `private/skate3-online/race/`: rules.json (per row its CLD fields and RaceHeats: StartLocation with its 6 slots,
//! TimeLimit / TimeLimitWarning, Include321, the leg's gates and split triggers as world boxes, the online start NIS
//! per player count and the outro), strings.json, scripts/race_shared.lua (the decompiled graph: the rules' source and
//! the tests' oracle), the start / outro cameras (tutorial.json + scene/camera) and the shared HUD movies.
//!
//! [`Race`] is race_shared handler by handler (line numbers are race_shared.lua's): every racer starts at once from
//! StartLocation slot = player id (setup_heat 655-664), the heat's online NIS plays (OnlineNISs[clamp(players - 1)],
//! 699-711), the loading screen closes at its notify (623-630) and the race runs (919-972): each racer's count-up
//! timer, the heat timer (the local one's last TimeLimitWarning seconds on the HUD). The leg's last split trigger is
//! the finish (1146-1245): places score players - place, the first finisher who is this console's player wins
//! (PlaceHigherToWin, 2009-2016); anyone's heat timer running out ends the heat (975-1031). Results: Race Times,
//! DeathRace layout, a time per racer (2407-2438), then the outro NIS (2209-2217) and LeaveGame.
//!
//! Team rows (the Online Challenge Map's Team tab, `match_core::team`) take the TeamsEnabled path (Online.IsTeamChallenge,
//! Online.GetTeam = the host's split, `Match::teams`): the team message before the countdown, the team leaderboard and
//! position DPO in the team's colour, an extra point per finisher in 4 or fewer, the finishers' points summed per team
//! (a tie: the first finisher's team), the team results and report (2051-2099, 2331-2406).
//!
//! Each console races its own skater and relays its trigger entries (`Match::outbox`); the others' come from the
//! relay. ponytail: retail replicates every skater and raises the triggers on each console. Solo
//! (`SKATE_MODE=race:<row>`) is a one-racer heat on the online rules.
use crate::{
    challenge::cutscene::{Cutscene, ground},
    map_transition::CurrentMap,
    match_core::{DNF, Match, Mode, Owned, Phase, Playable},
    physics::{GamePhysics, SkaterRuntime},
    skate3_modes::{self, Abort, ChallengeFade, Component, EndSync, Graph, Handled, HudDriver, Locator, Matrix, ModeData, Msg, Place, ResultsSetup, Rt, View, WHITE},
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    f32::consts::{FRAC_PI_2, PI},
};

/// HUD2.eLeaderboardHUDMenuType.RankNameTime (leaderboardhud: eLeaderBoardHUD_RankNameTime 4).
const RANK_NAME_TIME: i64 = 4;
/// FrontEnd.eResultsLBType.DeathRace (endchallenge_online: eResultsLBType_DeathRace 3).
const DEATH_RACE: i64 = 3;
/// eResultsLBType_TeamChallengeResults 8 (endchallenge_online's one team layout: "#" + place, name, int).
const TEAM_RESULTS: i64 = 8;
/// HUD2.eTeamMessageType.Normal (0x8284fb70); HUD2.eDPOFormat.Ints (0x8284ee90).
const TEAM_NORMAL: u8 = 0;
const DPO_INTS: i64 = 0;
/// Online.eFinishReason as the oracle's DRIVER numbers them (ReportOnlineResults' 7th field; only relayed to the
/// session, which keeps its own results).
const NOT_FINISHED: i64 = 0;
const NORMAL_COMPLETION: i64 = 1;
const START: &str = "StartLocation";
/// race_shared's timers (seconds): wait_for_assetload (385), setup_heat's fade-out (487), the teleports' settle (649),
/// the countdown (160, 912), RacePassTimerUpdate (964; single player only).
const WAIT_PRES: f64 = 0.5;
const FADEOUT: f64 = 0.5;
const TELEPORT_DONE: f64 = 0.01;
const COUNTDOWN: f64 = 3.;
const RACE_PASS: f64 = 1.5;

// ---------------------------------------------------------------- data
/// A trigger volume by its name (the Lua compares VolumeIDs) and its world box.
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct RaceVolume {
    pub name: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
}
impl RaceVolume {
    pub(crate) fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|k| self.min[k] <= p[k] && p[k] <= self.max[k])
    }
}

/// A ribbon instance (tSplineBankObject): the indicator type's outline and the style, keys of indicators.json.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Look {
    pub ribbon: String,
    pub style: String,
}

/// A course chevron (the row's VisualIndicators): its locator's world matrix, None where no mission places it.
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Chevron {
    pub matrix: Option<Matrix>,
    #[serde(flatten)]
    pub look: Look,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Gate {
    #[serde(rename = "GateVolume")]
    pub volume: Option<RaceVolume>,
    #[serde(rename = "Time_Bonus", default)]
    pub bonus: i64,
    #[serde(rename = "RibbonInstance", default)]
    pub ribbon: Option<Look>,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Leg {
    #[serde(rename = "SplitTimeTriggers", default)]
    pub splits: Vec<Option<RaceVolume>>,
    #[serde(rename = "Gates", default)]
    pub gates: Vec<Gate>,
}

/// tRaceHeatDefinition (rules.json `heats`).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Heat {
    pub start_location: Option<Locator>,
    pub time_limit: i64,
    pub time_limit_warning: i64,
    pub include321: bool,
    #[serde(default)]
    pub knockout_per_leg: i64,
    #[serde(default)]
    pub wipe_out_limit: i64,
    #[serde(default)]
    pub race_terminate_volumes: Vec<Option<RaceVolume>>,
    pub legs: Vec<Leg>,
    /// OnlineNISs as `<row>|<scene>` (the tool's placement), None where the scene has no camera.
    #[serde(rename = "start_scenes", default)]
    pub start_scenes: Vec<Option<String>>,
}

/// A row's CLD fields (rules.json).
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub title: String,
    pub online: bool,
    #[serde(default)]
    pub objectives: usize,
    #[serde(rename = "RaceType")]
    pub race_type: u32,
    #[serde(rename = "RaceGateSkipable")]
    pub skipable: bool,
    #[serde(rename = "PlaceHigherToWin")]
    pub place_higher: i64,
    #[serde(rename = "CountdownDelay")]
    pub countdown_delay: f64,
    #[serde(rename = "OwnedItHUDDisplayTime")]
    pub owned_time: f64,
    #[serde(rename = "KilledItHUDDisplayTime")]
    pub killed_time: f64,
    #[serde(rename = "OnlineEndCameraLocation", default)]
    pub end_camera: Option<Locator>,
    #[serde(rename = "CentralMessageHALIDChallengeFailed")]
    pub failed: String,
    /// The team rows' countdown: the team message (ID_CHALLENGE_CENTRAL_MESSAGE_TEAMINDICATION on every online row),
    /// then TeamIntroductionDelay (3 s) before CountdownDelay (887-893, 909-910).
    #[serde(rename = "TeamIntroductionDelay", default)]
    pub team_intro_delay: f64,
    #[serde(rename = "CentralMessageHALIDTeamIndication", default)]
    pub team_indication: String,
    /// The team NISs: IntroNIS (dynamic_character_nis, both teams' skaters) and OnlineOutroNIS (the winners'). The
    /// disc's rows name them (dom_intro*, Online_Celeb_0n); not exported: the NIS player with dynamic characters is
    /// not ported (None).
    #[serde(rename = "IntroNIS", default)]
    pub intro_nis: Option<String>,
    #[serde(rename = "OnlineOutroNIS", default)]
    pub online_outro_nis: Option<String>,
    #[serde(default)]
    pub heats: Vec<Heat>,
    /// RaceGateGreyRibbon / RaceFinalGateRibbon / RaceLegFinishGateRibbon (older rules.json: none, nothing drawn).
    #[serde(rename = "RaceGateGreyRibbon", default)]
    pub grey: Option<Look>,
    #[serde(rename = "RaceFinalGateRibbon", default)]
    pub last: Option<Look>,
    #[serde(rename = "RaceLegFinishGateRibbon", default)]
    pub leg_finish: Option<Look>,
    #[serde(default)]
    pub chevrons: Vec<Chevron>,
    /// OnlineOutroSoloNIS as `<row>|outro`.
    #[serde(default)]
    pub outro: Option<String>,
}
impl Row {
    fn heat(&self) -> &Heat {
        &self.heats[0]
    }
    /// Playable: online, Downhill (RaceType 0: every online row), one leg, no knockout or wipeout limit (none on the
    /// disc; their paths are not ported), every volume placed, 6 start slots, on a known map. A row's Objectives
    /// (4 rows have one) change nothing online: race_shared only switches their evaluation (182-187, 968-970, 1190) and
    /// its online HUD group has no ObjectiveHUD (208-217).
    fn playable(&self, id: &str) -> bool {
        let Some(h) = self.heats.first() else { return false };
        let placed = |v: &[Option<RaceVolume>]| v.iter().all(Option::is_some);
        self.online
            && self.race_type == 0
            && h.legs.len() == 1
            && h.knockout_per_leg == 0
            && h.wipe_out_limit == 0
            && h.start_location.as_ref().is_some_and(|l| l.slots.len() == 6)
            && !h.legs[0].splits.is_empty()
            && placed(&h.legs[0].splits)
            && h.legs[0].gates.iter().all(|g| g.volume.is_some())
            && placed(&h.race_terminate_volumes)
            && skate3_modes::map_of(id).is_some()
    }
    /// Every volume the race tests, once each (the trigger messages' index).
    pub fn volumes(&self) -> Vec<RaceVolume> {
        let mut out: Vec<RaceVolume> = Vec::new();
        let Some(h) = self.heats.first() else { return out };
        let all = h.legs.iter().flat_map(|l| l.splits.iter().flatten().chain(l.gates.iter().filter_map(|g| g.volume.as_ref())));
        for v in all.chain(h.race_terminate_volumes.iter().flatten()) {
            if !out.iter().any(|o| o.name == v.name) {
                out.push(v.clone());
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
    DynamicCharacterNis,
    SetupChallenge,
    SetupHeat,
    Countdown,
    RunHeat,
    EndHeat,
    CleanupHeat,
    BuildResults,
    OutroNis,
    CleanupChallenge,
    Complete,
}

/// vars.Competitor[i] (player id i - 1).
#[derive(Clone, Debug, Default)]
pub(crate) struct Comp {
    pub active: bool,
    pub disconnected: bool,
    teleported: bool,
    leg: usize,
    /// CurrentRaceScore.
    pub score: i64,
    heat_timer: u32,
    warn_timer: u32,
    total_timer: u32,
    /// HeatTotalTime: the finish time, -1 unfinished.
    pub total_time: f64,
    /// Online: the race clock this racer finished at (its place comes from it).
    finished_at: Option<f64>,
    /// LastLapGateIndex.
    pub last_lap: usize,
    flags: Vec<bool>,
    pub gates: i64,
    /// Online.GetTeam (1 / 2; 1 without teams, as the stub's default).
    pub team: usize,
}

/// vars.TeamData[i] (45-55).
#[derive(Clone, Debug, Default)]
pub(crate) struct Team {
    pub size: usize,
    pub points: i64,
    pub time: f64,
    pub rank: i64,
    pub dnf: usize,
    pub name: String,
}

/// vars.RoundStandings[i].
#[derive(Clone, Debug)]
pub(crate) struct Standing {
    pub score: i64,
    pub time: f64,
    pub player: usize,
}

/// The ribbon a trigger indicator draws: the gate's own RibbonInstance, or the row's grey / final / leg-finish one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ribbon {
    Gate(usize),
    Grey,
    Final,
    LegFinish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Lose,
    Win,
}

/// race_shared's vars on one console.
#[derive(Clone)]
pub(crate) struct Race {
    row: Row,
    /// Each scene's length: the start NIS per OnlineNISs slot, the outro.
    start_lengths: Vec<f64>,
    outro_length: f64,
    me: usize,
    n: usize,
    pub comps: Vec<Comp>,
    setup_complete: bool,
    wait_pres: u32,
    fadeout: Option<u32>,
    teleport_done: Option<u32>,
    countdown_delay: Option<u32>,
    countdown_timer: Option<u32>,
    completion: Option<u32>,
    heat_timer_set: bool,
    win_pos: i64,
    pub someone_finished: bool,
    finished_count: i64,
    result: Flow,
    /// Race.IsInRace: ponytail: from StartRaceStats to StopRaceStats / ResetRaceStats (the native is not traced).
    in_race: bool,
    pub standings: Vec<Standing>,
    /// Results_SetPlayerTime: (player, time).
    pub times: Vec<(usize, f64)>,
    /// The NIS last entered (the glue plays it while Rt::presenting runs).
    pub scene: Option<String>,
    /// ChallengeManager.SwitchChallengeCamera(true, OnlineEndCameraLocation).
    pub end_camera: bool,
    /// When run_heat started the clocks (the leaderboard's running times).
    pub started: Option<f64>,
    /// Online.IsTeamChallenge / Online.GetTeam: each player's team (1 / 2), empty for none; Teams.GetName's names.
    pub teams_in: Vec<u8>,
    pub team_names: [String; 2],
    /// vars.TeamsEnabled, vars.TeamData, TeamIntroductionTimerID, the results' TeamStandings.
    pub teams_on: bool,
    pub teams: [Team; 2],
    team_intro: Option<u32>,
    pub team_standings: [Vec<Standing>; 2],
    /// race_shared's setup_heat ResetReplayBuffer (replay research Q4): the glue drops the replay recording, so a
    /// heat's replay holds that heat only.
    pub replay_reset: bool,
    /// vars.VisualIndicatorGroup's trigger indicators: (volume, ribbon), the glue draws them.
    pub indicators: Vec<(String, Ribbon)>,
    /// The row's VisualIndicators (setup_heat AddIndicator 569-574 .. cleanup_heat RemoveIndicator 733-738).
    pub chevrons: bool,
    /// Online, set by the glue around another console's relayed entry: that racer's own race clock when it entered
    /// (its finish time is that, not the moment the relay arrived here).
    pub stamp: Option<f64>,
}

fn ms(t: f64) -> i64 {
    (t * 1000. + 0.5).floor() as i64
}

impl Race {
    pub fn new(row: Row, start_lengths: Vec<f64>, outro_length: f64, players: usize, me: usize) -> Self {
        Race {
            row,
            start_lengths,
            outro_length,
            me,
            n: players,
            comps: vec![Comp::default(); players],
            setup_complete: false,
            wait_pres: 0,
            fadeout: None,
            teleport_done: None,
            countdown_delay: None,
            countdown_timer: None,
            completion: None,
            heat_timer_set: false,
            win_pos: 1,
            someone_finished: false,
            finished_count: 0,
            result: Flow::Lose,
            in_race: false,
            standings: Vec::new(),
            times: Vec::new(),
            scene: None,
            end_camera: false,
            started: None,
            teams_in: Vec::new(),
            team_names: ["T1".into(), "T2".into()],
            teams_on: false,
            teams: Default::default(),
            team_intro: None,
            team_standings: Default::default(),
            replay_reset: false,
            indicators: Vec::new(),
            chevrons: false,
            stamp: None,
        }
    }

    /// [setup_code] stateenter (12-327), the online path.
    fn setup_code(&mut self, rt: &mut Rt<St>) {
        rt.pin(self.me, true);
        let flags = self.flags();
        for (i, c) in self.comps.iter_mut().enumerate() {
            let gone = rt.gone.get(i).copied().unwrap_or(true);
            // Online.GetTeam (0x828463d8: the player's +112, plus 1).
            let team = if self.teams_in.get(i) == Some(&2) { 2 } else { 1 };
            *c = Comp { active: !gone, disconnected: gone, leg: 1, total_time: 0., flags: flags.clone(), team, ..Comp::default() };
        }
        // 24, 45-55, 95-111: online, TeamsEnabled = Online.IsTeamChallenge(); TeamData Rank i; the team sizes.
        self.teams_on = !self.teams_in.is_empty();
        self.teams = [1, 2].map(|i| Team { rank: i, name: self.team_names[i as usize - 1].clone(), ..Team::default() });
        if self.teams_on {
            for c in &self.comps {
                self.teams[c.team - 1].size += 1;
            }
        }
        self.setup_complete = true;
        self.win_pos = 1;
        self.finished_count = 0;
        // HUD2.CreateGroup: hudloaded once the group is up.
        rt.post(Msg::HudLoaded);
        rt.board_variant("ID_RACE_VARIANT_DOWNHILL");
    }

    /// VisualIndicators.AddTriggerIndicator on a volume (the oracle's "ind+").
    fn add_indicator(&mut self, rt: &mut Rt<St>, volume: &str, ribbon: Ribbon) {
        let lua = match ribbon {
            Ribbon::Gate(i) => format!("r{}", i + 1),
            Ribbon::Grey => "RaceGateGreyRibbon".into(),
            Ribbon::Final => "RaceFinalGateRibbon".into(),
            Ribbon::LegFinish => "RaceLegFinishGateRibbon".into(),
        };
        rt.note(format!("ind+ {volume} {lua}"));
        // ponytail: a second add on a drawn volume replaces it. It happens at every countdown whose finish is also the
        // last gate (12 of the 13 disc rows): the gate's ribbon, then RaceFinalGateRibbon. What the native group does
        // with the second (replace, keep, both) is not traced; replacing shows the finish ribbon.
        self.indicators.retain(|(v, _)| v != volume);
        self.indicators.push((volume.to_owned(), ribbon));
    }
    fn remove_indicator(&mut self, rt: &mut Rt<St>, volume: &str) {
        rt.note(format!("ind- {volume}"));
        self.indicators.retain(|(v, _)| v != volume);
    }
    /// RemoveByType RaceGates then RaceFinish: every trigger indicator. ponytail: the gates are added with their
    /// IndicatorType (2 on every gate) and the native's type match is not traced; both calls always come together.
    fn clear_indicators(&mut self, rt: &mut Rt<St>) {
        rt.note("indclear");
        rt.note("indclear");
        self.indicators.clear();
    }
    /// [countdown] stateenter's gates and finish (834-882), the non-FollowMe path: ordered gates show the first in its
    /// own ribbon and the second grey; skippable ones all show their own; the finish shows when the gates can be
    /// skipped or there are fewer than 2, final on a one-leg heat.
    fn countdown_indicators(&mut self, rt: &mut Rt<St>) {
        let heat = self.row.heat();
        let name = |v: &Option<RaceVolume>| v.as_ref().map_or(String::new(), |v| v.name.clone());
        let gates: Vec<String> = heat.legs[0].gates.iter().map(|g| name(&g.volume)).collect();
        let finish = heat.legs[0].splits.last().map(name).unwrap_or_default();
        let (skip, one_leg) = (self.row.skipable, heat.legs.len() == 1);
        for (i, g) in gates.iter().enumerate() {
            match (skip, i) {
                (true, _) | (false, 0) => self.add_indicator(rt, g, Ribbon::Gate(i)),
                (false, 1) => self.add_indicator(rt, g, Ribbon::Grey),
                _ => {}
            }
        }
        if skip || gates.len() < 2 {
            self.add_indicator(rt, &finish, if one_leg { Ribbon::Final } else { Ribbon::LegFinish });
        }
    }

    /// HeatGateFlags for a fresh leg: all open when the gates can be skipped, else only the first.
    fn flags(&self) -> Vec<bool> {
        let gates = self.row.heats.first().map_or(0, |h| h.legs[0].gates.len());
        (0..gates).map(|j| !self.row.skipable && j != 0).collect()
    }

    /// [setup_heat] stateenter (472-621).
    fn setup_heat(&mut self, rt: &mut Rt<St>) {
        self.replay_reset = true;
        self.heat_timer_set = false;
        self.win_pos = 1;
        self.someone_finished = false;
        self.result = Flow::Lose;
        self.fadeout = Some(rt.add_timer(FADEOUT));
        self.finished_count = 0;
        let flags = self.flags();
        for c in &mut self.comps {
            c.teleported = false;
            c.leg = 1;
            c.active = true;
            for t in [c.heat_timer, c.warn_timer] {
                if t != 0 {
                    rt.remove_timer(t);
                }
            }
            (c.heat_timer, c.warn_timer, c.total_timer, c.total_time, c.last_lap, c.gates) = (0, 0, 0, -1., 0, 0);
            c.flags = flags.clone();
        }
        // 558-563.
        for t in &mut self.teams {
            (t.points, t.time, t.rank, t.dnf) = (0, 0., 2, 0);
        }
        // 565-574: a new indicator group, the row's chevrons in it.
        self.indicators.clear();
        self.chevrons = true;
        let team = self.comps.get(self.me).map_or(1, |c| c.team);
        if self.teams_on {
            // 581-585: the team rows alone, this player's team highlighted.
            rt.board_show_players(false);
            rt.board_team_highlight(team, true);
            rt.board_init_teams(RANK_NAME_TIME, self.n, self.comps.iter().map(|c| c.team as u8).collect(), self.team_names.clone());
        } else {
            rt.board_init(RANK_NAME_TIME, self.n);
        }
        rt.show(Component::LeaderboardHUD, true);
        rt.show(Component::DPOHUD, false);
        if self.teams_on {
            // 598-610 (online: no Killed It DPO, so the first DPO): the position, 1 of NumCompetitors, in the team's
            // colour.
            rt.show(Component::DPOHUD, true);
            rt.dpo_title("ID_DPO_POSITION");
            rt.dpo_format(DPO_INTS);
            rt.dpo_params(1, Some(self.n as i64));
            rt.dpo_team(team as i64 - 1);
        }
        rt.show(Component::TimerHUD, false);
    }

    /// [setup_heat] timerexpired (655-718).
    fn setup_heat_timer(&mut self, rt: &mut Rt<St>, id: u32) {
        if self.fadeout == Some(id) {
            self.fadeout = None;
            for i in 0..self.n {
                rt.reposition(i, Place { field: START, index: 0, slot: i }, 0.);
                rt.pin(i, true);
            }
            return;
        }
        if self.teleport_done == Some(id) {
            self.teleport_done = None;
            // Race.ResetRaceStats.
            self.in_race = false;
            for c in self.comps.iter_mut().filter(|c| c.active) {
                c.score = 0;
            }
            let count = self.row.heat().start_scenes.len();
            if count > 0 {
                let i = (self.n as i64 - 1).clamp(1, count as i64) as usize - 1;
                self.scene = self.row.heat().start_scenes[i].clone();
                rt.nis(self.start_lengths.get(i).copied().unwrap_or(0.));
            } else {
                rt.close_loading();
                rt.change(St::Countdown);
            }
        }
    }

    /// [run_heat] stateenter (920-972).
    fn run_heat(&mut self, rt: &mut Rt<St>) {
        if self.heat_timer_set {
            return;
        }
        // Race.StartRaceStats.
        self.in_race = true;
        self.started = Some(rt.now);
        let (limit, warning) = (self.row.heat().time_limit, self.row.heat().time_limit_warning);
        for i in 0..self.n {
            if !self.comps[i].active {
                continue;
            }
            rt.pin(i, false);
            self.comps[i].total_timer = rt.add_countup();
            if limit > 0 {
                if i == self.me {
                    if limit > warning {
                        self.comps[i].warn_timer = rt.add_timer((limit - warning) as f64);
                    } else {
                        self.comps[i].heat_timer = rt.add_timer(limit as f64);
                        rt.timer(self.comps[i].heat_timer);
                    }
                } else {
                    self.comps[i].heat_timer = rt.add_timer(limit as f64);
                }
            }
        }
        self.heat_timer_set = true;
        // RacePassTimerUpdate: the pass speech, single player only (1054-1110).
        rt.add_timer(RACE_PASS);
    }

    /// [run_heat] timerexpired (975-1031), the online path.
    fn run_heat_timer(&mut self, rt: &mut Rt<St>, id: u32) {
        for i in 0..self.n {
            if id == self.comps[i].warn_timer && i == self.me {
                self.comps[i].heat_timer = rt.add_timer(self.row.heat().time_limit_warning as f64);
                rt.timer(self.comps[i].heat_timer);
            }
            if id == self.comps[i].heat_timer {
                self.comps[i].active = false;
                rt.pin(i, true);
                rt.pause_countup(self.comps[i].total_timer);
                rt.remove_countup(self.comps[i].total_timer);
                self.finished_count += 1;
                rt.change(St::EndHeat);
            }
        }
    }

    /// [run_heat] triggerentercollision (1114-1499) for a single-leg Downhill heat.
    fn trigger(&mut self, rt: &mut Rt<St>, p: usize, vol: &str) -> Handled {
        if p >= self.n || !self.comps[p].active {
            return Ok(());
        }
        let row = self.row.clone();
        let heat = row.heat();
        // vars.CurrentLegData: a racer past the last leg has none (a Lua error on its first use).
        let leg = heat.legs.get(self.comps[p].leg - 1).ok_or(Abort)?;
        let name = |v: &Option<RaceVolume>| v.as_ref().map_or("", |v| v.name.as_str()).to_owned();
        for (i, s) in leg.splits.iter().enumerate() {
            if name(s) == vol && i + 1 > self.comps[p].last_lap {
                self.comps[p].last_lap = i + 1;
                // Race.SetPlayerProgress.
            }
        }
        if leg.splits.last().map(name).as_deref() == Some(vol) && self.comps[p].last_lap == leg.splits.len() {
            self.comps[p].last_lap = 0;
            if self.comps[p].leg != heat.legs.len() {
                // ponytail: the next leg (1246-1394) is not ported; no playable row has a second leg.
                return Err(Abort);
            }
            self.finish(rt, p);
        }
        for (i, g) in leg.gates.iter().enumerate() {
            if name(&g.volume) != vol || self.comps[p].flags.get(i).copied() != Some(false) {
                continue;
            }
            if p == self.me && self.comps[p].active {
                rt.modified("ID_RACE_GATES_COMPLETED", WHITE, &[], false);
            }
            if heat.time_limit > 0 && g.bonus > 0 {
                let left = rt.countdown_time(self.comps[p].heat_timer) + g.bonus as f64;
                rt.remove_timer(self.comps[p].heat_timer);
                self.comps[p].heat_timer = rt.add_timer(left);
                if p == self.me {
                    // 1420-1438: this gate's indicator off; ordered gates light the next and grey the one after.
                    let count = leg.gates.len() as i64;
                    self.remove_indicator(rt, &name(&g.volume));
                    if !row.skipable && (i as i64 + 1) < count - 1 {
                        let next = name(&leg.gates[i + 1].volume);
                        self.remove_indicator(rt, &next);
                        self.add_indicator(rt, &next, Ribbon::Gate(i + 1));
                    }
                    if !row.skipable && (i as i64 + 1) < count - 2 {
                        let after = name(&leg.gates[i + 2].volume);
                        self.remove_indicator(rt, &after);
                        self.add_indicator(rt, &after, Ribbon::Grey);
                    }
                    rt.timer(self.comps[p].heat_timer);
                    rt.custom_central("ID_RACE_SECONDS_ADDED", WHITE, &g.bonus.to_string());
                }
            }
            self.comps[p].flags[i] = true;
            self.comps[p].gates += 1;
        }
        for t in heat.race_terminate_volumes.iter().map(name) {
            if t == vol {
                self.comps[p].active = false;
                rt.pin(p, true);
                if p == self.me {
                    rt.change(St::EndHeat);
                }
            }
        }
        Ok(())
    }

    /// The finish (1151-1202): the place's points, the messages, the time.
    fn finish(&mut self, rt: &mut Rt<St>, p: usize) {
        let n = self.n as i64;
        self.someone_finished = true;
        let time = self.stamp.unwrap_or_else(|| rt.countup_time(self.comps[p].total_timer));
        if rt.online {
            // Not retail (user 10-12): online the places follow the racers' own finish times, not the order their
            // finishes reached this console (a racer 0.1 s ahead was placed behind on the other consoles when its
            // relay took longer), so every console awards the same points.
            self.comps[p].finished_at = Some(time);
            let mut order: Vec<(f64, usize)> = (self.comps.iter().enumerate()).filter_map(|(i, c)| Some((c.finished_at?, i))).collect();
            order.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for (k, (_, i)) in order.into_iter().enumerate() {
                self.comps[i].score = n - (1 + k as i64);
            }
        } else {
            self.comps[p].score += n - self.win_pos;
        }
        self.win_pos += 1;
        self.comps[p].active = false;
        rt.pin(p, true);
        let racing = self.comps.iter().any(|c| c.active);
        if racing {
            if self.row.end_camera.is_some() && p == self.me {
                self.end_camera = true;
                rt.note("endcam true");
            }
            if p == self.me {
                rt.central("ID_RACE_FINISHED", WHITE, &[], 0);
            } else {
                rt.modified("ID_ONLINE_EVENT_CHALLENGE_FINISH", WHITE, &[format!("P{p}")], false);
            }
        }
        let c = &mut self.comps[p];
        c.total_time = time;
        rt.remove_countup(c.total_timer);
        self.finished_count += 1;
        if !racing {
            rt.change(St::EndHeat);
        }
    }

    /// [end_heat] stateenter (1950-2102), the online non-team path.
    fn end_heat(&mut self, rt: &mut Rt<St>) -> Handled {
        self.result = Flow::Lose;
        let mut st: Vec<Standing> = (self.comps.iter().enumerate())
            .map(|(i, c)| Standing { score: c.score, time: if c.total_time <= 0.001 { -2. } else { c.total_time }, player: i })
            .collect();
        skate3_modes::lua_sort(&mut st, |a, b| a.score > b.score);
        // 1976-1979: teams of 4 or fewer, a point more per scoring finisher.
        let extra = i64::from(self.teams_on && self.n <= 4);
        for i in 0..self.n {
            if i + 1 == self.n || st[i].time <= 0.001 {
                st[i].score = 0;
            } else {
                st[i].score += extra;
            }
            self.comps[st[i].player].score = st[i].score;
        }
        for i in 0..self.n {
            if self.comps[i].active {
                rt.pin(i, true);
            }
        }
        self.standings = st;
        for i in 0..self.row.place_higher.max(0) as usize {
            let s = self.standings.get(i).ok_or(Abort)?;
            if s.player == self.me && 0. < s.time {
                self.result = Flow::Win;
            }
        }
        // Race.StopRaceStats.
        self.in_race = false;
        if self.teams_on {
            // 2051-2082: each finisher's points and time to its team, a DNF counted; more points rank 1, a tie goes
            // to the first finisher's team (none finished: both 2).
            for s in &self.standings {
                let t = &mut self.teams[self.comps[s.player].team - 1];
                if 0. < s.time {
                    t.points += s.score;
                    t.time += s.time;
                } else {
                    t.dnf += 1;
                }
            }
            let [a, b] = [self.teams[0].points, self.teams[1].points];
            let ranks = if a < b {
                [2, 1]
            } else if a == b {
                match self.standings.first().filter(|s| 0. < s.time) {
                    Some(s) if self.comps[s.player].team == 1 => [1, 2],
                    Some(_) => [2, 1],
                    None => [2, 2],
                }
            } else {
                [1, 2]
            };
            (self.teams[0].rank, self.teams[1].rank) = (ranks[0], ranks[1]);
        }
        // 2091-2099: a win when this player's team ranks first, or this player placed high enough.
        let team_won = self.teams_on && self.teams[self.comps[self.me].team - 1].rank == 1;
        self.result = if self.someone_finished && (team_won || self.result == Flow::Win) { Flow::Win } else { Flow::Lose };
        rt.pause(true); // 2088
        rt.change(St::CleanupHeat);
        Ok(())
    }

    /// [build_results] stateenter (2331-2406), teams: the FinalResults group's two team rows (TeamChallengeResults:
    /// rank, points), then the RoundResults group's members per team by HeatTimeSorter (PlaceNameScoreTime, the second
    /// team's through Results_SetSecondLBPlayerData / Time). Only the team rows reach the screen (`rt.hud.results`):
    /// endchallenge_online's FillTable draws TeamChallengeResults (APT 5396) and one table. ponytail: which group the
    /// online screen shows is the native's (not traced); the team table.
    fn team_results(&mut self, rt: &mut Rt<St>) {
        for k in 0..2 {
            rt.result(k, self.teams[k].rank, self.teams[k].points);
        }
        // HeatTimeSorter (266-278): finishers by time, before anyone without one.
        let sorter = |a: &Standing, b: &Standing| {
            if b.time < 0. && a.time > 0. {
                return true;
            }
            if b.time > 0. && a.time < 0. {
                return false;
            }
            a.time < b.time
        };
        self.team_standings = [1, 2].map(|t| {
            let mut st: Vec<Standing> = self.standings.iter().filter(|s| self.comps[s.player].team == t).cloned().collect();
            skate3_modes::lua_sort(&mut st, sorter);
            st
        });
        for (k, list) in self.team_standings.iter().enumerate() {
            let (data, time) = if k == 0 { ("result", "time") } else { ("result2", "time2") };
            for (i, s) in list.iter().enumerate() {
                rt.note(format!("{data} {} {} {}", s.player, i + 1, s.score));
                rt.note(format!("{time} {i} {}", ms(s.time)));
            }
        }
        rt.change(St::OutroNis);
    }

    /// [build_results] stateenter (2407-2440), non-team.
    fn build_results(&mut self, rt: &mut Rt<St>) {
        if self.teams_on {
            return self.team_results(rt);
        }
        let pass = 1 + 1 < self.row.heats.len();
        for i in 0..self.standings.len() {
            if self.standings[i].time == 0. {
                self.standings[i].time = -1.;
            }
            let (p, t) = (self.standings[i].player, self.standings[i].time);
            rt.result(p, i as i64 + 1, 0);
            rt.note(format!("time {p} {}", ms(t)));
            self.times.push((p, t));
            let status = if pass {
                "ID_CHALLENG_RESULT_STATUS_PASS"
            } else if (i as i64) < self.row.place_higher {
                "ID_CHALLENG_RESULT_STATUS_WON"
            } else {
                "ID_CHALLENG_RESULT_STATUS_LOSE"
            };
            rt.note(format!("status {p} {status}"));
        }
        rt.change(St::OutroNis);
    }

    /// vars.ReportOnlineResults (293-325): Online.OnlineChallengeComplete.
    fn report(&self, rt: &mut Rt<St>) {
        for s in &self.standings {
            let reason = if self.comps[s.player].total_time == -1. { NOT_FINISHED } else { NORMAL_COMPLETION };
            rt.note(format!("report {} {} {} {reason}", s.player, s.score, ms(s.time)));
        }
        if self.teams_on {
            // TeamResults {Rank, TotalPoints} (312-320).
            for (i, t) in self.teams.iter().enumerate() {
                rt.note(format!("teamreport {} {} {}", i + 1, t.rank, t.points));
            }
        }
    }

    /// [global] playerdisconnected (1912-1947).
    fn disconnected(&mut self, rt: &mut Rt<St>, p: usize) {
        if !self.setup_complete || p >= self.n || self.comps[p].disconnected {
            return;
        }
        self.comps[p].disconnected = true;
        if p == self.me {
            rt.leave();
            return;
        }
        let c = &mut self.comps[p];
        c.total_time = -1.;
        c.active = false;
        rt.remove_countup(c.total_timer);
        self.finished_count += 1;
        if self.in_race && !self.comps.iter().any(|c| c.active) {
            rt.change(St::EndHeat);
        }
    }

    /// This console's racer's time: finished, or still racing (DNF).
    pub fn my_time(&self) -> Option<f64> {
        self.comps.get(self.me).map(|c| c.total_time).filter(|&t| t > 0.)
    }
}

impl Graph for Race {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::SetupCode => "setup_code",
            St::NisOverview => "nis_overview",
            St::WaitForAssetload => "wait_for_assetload",
            St::DynamicCharacterNis => "dynamic_character_nis",
            St::SetupChallenge => "setup_challenge",
            St::SetupHeat => "setup_heat",
            St::Countdown => "countdown",
            St::RunHeat => "run_heat",
            St::EndHeat => "end_heat",
            St::CleanupHeat => "cleanup_heat",
            St::BuildResults => "build_results",
            St::OutroNis => "outro_nis",
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
                rt.change(St::SetupCode);
            }
            (St::SetupCode, Msg::Enter) => self.setup_code(rt),
            (St::SetupCode, Msg::HudLoaded) => rt.change(St::NisOverview),
            // The intro presentation: IntroPresentationEvents is empty on every race row.
            (St::NisOverview, Msg::Enter) => rt.present(0.),
            (St::NisOverview, Msg::Presented | Msg::Nis) => rt.change(St::WaitForAssetload),
            (St::WaitForAssetload, Msg::Enter) => self.wait_pres = rt.add_timer(WAIT_PRES),
            (St::WaitForAssetload, Msg::Timer(id)) if *id == self.wait_pres => rt.change(St::DynamicCharacterNis),
            // 2295-2326: teams with an IntroNIS, the team intro with both teams' skaters, setup_challenge at its notify.
            // ponytail: the NIS player with dynamic characters is not ported (no scene plays).
            (St::DynamicCharacterNis, Msg::Enter) if self.teams_on && self.row.intro_nis.is_some() => rt.nis(0.),
            (St::DynamicCharacterNis, Msg::Enter | Msg::Nis) => rt.change(St::SetupChallenge),
            (St::SetupChallenge, Msg::Enter) => {
                self.clear_indicators(rt); // 461-462
                self.win_pos = 1;
                self.finished_count = 0;
                rt.change(St::SetupHeat);
            }
            (St::SetupHeat, Msg::Enter) => self.setup_heat(rt),
            (St::SetupHeat, Msg::Timer(id)) => self.setup_heat_timer(rt, *id),
            (St::SetupHeat, Msg::Teleported(p)) if *p < self.n => {
                self.comps[*p].teleported = true;
                rt.pin(*p, true);
                if self.comps.iter().all(|c| c.disconnected || c.teleported) {
                    self.teleport_done = Some(rt.add_timer(TELEPORT_DONE));
                }
            }
            (St::SetupHeat, Msg::Nis) if self.fadeout.is_none() => {
                rt.close_loading();
                rt.change(St::Countdown);
            }
            (St::SetupHeat, Msg::TransitionDone) => {
                rt.add_timer(0.5);
            }
            (St::Countdown, Msg::Enter) => {
                self.countdown_indicators(rt);
                if self.teams_on {
                    // 887-890: this player's team, in its colour (team - 1: 0 red, 1 blue).
                    let team = self.comps.get(self.me).map_or(1, |c| c.team);
                    let id = self.row.team_indication.clone();
                    rt.team_central(&id, team as i64 - 1, TEAM_NORMAL, &[self.teams[team - 1].name.clone()]);
                }
                if !self.row.heat().include321 {
                    rt.change(St::RunHeat);
                } else if self.teams_on {
                    self.team_intro = Some(rt.add_timer(self.row.team_intro_delay));
                } else {
                    self.countdown_delay = Some(rt.add_timer(self.row.countdown_delay));
                }
            }
            (St::Countdown, Msg::Timer(id)) => {
                if self.team_intro == Some(*id) {
                    self.countdown_delay = Some(rt.add_timer(self.row.countdown_delay));
                } else if self.countdown_delay == Some(*id) {
                    let t = rt.add_timer(COUNTDOWN);
                    self.countdown_timer = Some(t);
                    rt.countdown(t);
                } else if self.countdown_timer == Some(*id) {
                    rt.change(St::RunHeat);
                }
            }
            (St::RunHeat, Msg::Enter) => self.run_heat(rt),
            (St::RunHeat, Msg::Timer(id)) => self.run_heat_timer(rt, *id),
            (St::RunHeat, Msg::Trigger { player, volume, inside: true }) => {
                let vol = self.row.volumes().get(*volume as usize).map(|v| v.name.clone()).unwrap_or_default();
                self.trigger(rt, *player, &vol)?;
            }
            (St::RunHeat, Msg::Exit) => {
                self.end_camera = false;
                rt.note("endcam false");
            }
            (St::EndHeat, Msg::Enter) => self.end_heat(rt)?,
            (St::CleanupHeat, Msg::Enter) => {
                self.clear_indicators(rt); // 731-732
                self.chevrons = false;
                rt.show(Component::LeaderboardHUD, false);
                rt.show(Component::DPOHUD, false);
                self.completion = Some(rt.add_timer(self.row.owned_time.max(self.row.killed_time)));
                if self.result == Flow::Lose {
                    let failed = self.row.failed.clone();
                    rt.failed(&failed);
                }
            }
            (St::CleanupHeat, Msg::Timer(id)) if self.completion == Some(*id) => {
                if self.result == Flow::Win {
                    rt.show(Component::OwnedItHUD, false);
                }
                rt.change(St::BuildResults);
            }
            (St::BuildResults, Msg::Enter) => self.build_results(rt),
            // 2194-2208: teams, the winners' OnlineOutroNIS (Results.SetWinningTeam). ponytail: the NIS player with
            // dynamic characters is not ported (no scene plays).
            (St::OutroNis, Msg::Enter) if self.someone_finished && self.teams_on && self.row.online_outro_nis.is_some() => {
                rt.pause(false); // 2177
                rt.nis(0.);
            }
            // OnlineOutroSoloNIS is never nil (an empty definition stays a value).
            (St::OutroNis, Msg::Enter) if self.someone_finished && !self.teams_on => {
                rt.pause(false); // 2178
                self.scene = self.row.outro.clone();
                rt.nis(self.outro_length);
            }
            (St::OutroNis, Msg::Enter | Msg::Nis) => {
                if matches!(msg, Msg::Enter) {
                    rt.pause(false); // 2178
                }
                self.report(rt);
                rt.change(St::CleanupChallenge);
            }
            (St::CleanupChallenge, Msg::Enter) => {
                // Race.ResetRaceStats.
                self.in_race = false;
                self.clear_indicators(rt); // 1803-1804
                rt.change(St::Complete);
            }
            (St::Complete, Msg::Enter) => {
                rt.pin(self.me, false);
                rt.leave();
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// Where an indicator stands: over a trigger volume, facing the way the course comes in from, or at a locator.
#[derive(Debug)]
pub(crate) enum Spot {
    Gate(RaceVolume, Vec3),
    Locator(Matrix),
}

/// What the local racer sees of vars.VisualIndicatorGroup: the placed chevrons while the heat has them, then each
/// trigger indicator with its ribbon (an indicator whose ribbon or volume the data lacks draws nothing).
pub(crate) fn drawn<'a>(row: &'a Row, g: &Race) -> Vec<(String, &'a Look, Spot)> {
    let mut out = Vec::new();
    if g.chevrons {
        for (i, c) in row.chevrons.iter().enumerate() {
            if let Some(m) = c.matrix {
                out.push((format!("chevron {i}"), &c.look, Spot::Locator(m)));
            }
        }
    }
    let Some(heat) = row.heats.first() else { return out };
    let volumes = row.volumes();
    // The course in order: the start, each gate, the finish; a volume faces the one before it.
    let start = heat.start_location.as_ref().map_or(Vec3::ZERO, |l| Vec3::from_slice(&l.matrix[3][..3]));
    let course: Vec<&RaceVolume> =
        heat.legs.iter().flat_map(|l| l.gates.iter().filter_map(|g| g.volume.as_ref()).chain(l.splits.last().and_then(Option::as_ref))).collect();
    let approach = |name: &str| {
        let at = course.iter().position(|v| v.name == name);
        at.and_then(|i| i.checked_sub(1)).map_or(start, |i| (Vec3::from(course[i].min) + Vec3::from(course[i].max)) / 2.)
    };
    for (volume, ribbon) in &g.indicators {
        let look = match *ribbon {
            Ribbon::Gate(i) => heat.legs[0].gates.get(i).and_then(|g| g.ribbon.as_ref()),
            Ribbon::Grey => row.grey.as_ref(),
            Ribbon::Final => row.last.as_ref(),
            Ribbon::LegFinish => row.leg_finish.as_ref(),
        };
        if let (Some(look), Some(v)) = (look, volumes.iter().find(|v| &v.name == volume)) {
            out.push((volume.clone(), look, Spot::Gate(v.clone(), approach(volume))));
        }
    }
    out
}

/// indicators.json (tools/skate3_online.py): per indicator type its outline, per style its textures and tint.
#[derive(Deserialize, Default)]
struct Looks {
    shapes: HashMap<String, Vec<[f32; 2]>>,
    styles: HashMap<String, Style>,
}
#[derive(Deserialize)]
struct Style {
    texture: String,
    background: Option<String>,
    tint: [f32; 4],
}
const INDICATORS: &str = "private/skate3-online/race/indicator";

/// A gate's ribbon over its trigger volume: on the ground under the centre, across the way the course comes in, as
/// wide as the volume (the outline scaled from its own width). ponytail: as skate2_death_race::arch; the exe's fit of
/// the outline to a volume (the spline indicator manager) is not traced.
fn arch(v: &RaceVolume, from: Vec3, floor: f32, outline: &[[f32; 2]]) -> Transform {
    let (min, max) = (Vec3::from(v.min), Vec3::from(v.max));
    let (centre, size) = ((min + max) / 2., max - min);
    let xs = || outline.iter().map(|p| p[0]);
    let own = xs().fold(f32::MIN, f32::max) - xs().fold(f32::MAX, f32::min);
    let travel = centre - from;
    let (width, yaw) =
        if travel.x.abs() > travel.z.abs() { (size.z, FRAC_PI_2.copysign(travel.x)) } else { (size.x, if travel.z < 0. { PI } else { 0. }) };
    Transform::from_translation(centre.with_y(floor))
        .with_rotation(Quat::from_rotation_y(yaw))
        .with_scale(Vec3::new((width / own.max(0.01)).clamp(0.3, 4.), 1., 1.))
}

#[derive(Default)]
struct Drawn {
    key: String,
    roots: Vec<Entity>,
    meshes: HashMap<String, Handle<Mesh>>,
    materials: HashMap<String, (Handle<StandardMaterial>, Option<Handle<StandardMaterial>>)>,
}

/// The race's ribbons and chevrons ([`drawn`]), respawned when what race_shared shows changes; as
/// skate2_death_race::visuals draws Skate 2's (the darken pass, then the colour pass added).
#[allow(clippy::too_many_arguments)]
fn visuals(
    mut commands: Commands,
    game: Res<RaceGame>,
    physics: Option<Res<GamePhysics>>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut state: Local<Drawn>,
) {
    let d = &mut *state;
    let key = game.play.as_ref().map(|p| format!("{} {:?} {}", p.id, p.g.indicators, p.g.chevrons)).unwrap_or_default();
    if d.key == key {
        return;
    }
    d.key = key;
    for root in d.roots.drain(..) {
        commands.entity(root).despawn();
    }
    let (Some(p), Some(looks)) = (game.play.as_ref(), game.looks.as_ref()) else { return };
    for (name, look, spot) in drawn(&p.row, &p.g) {
        let (Some(outline), Some(style)) = (looks.shapes.get(&look.ribbon), looks.styles.get(&look.style)) else { continue };
        let transform = match spot {
            Spot::Locator(m) => Transform::from_matrix(Mat4::from_cols_array_2d(&m)),
            Spot::Gate(v, from) => {
                let top = Vec3::new((v.min[0] + v.max[0]) / 2., v.max[1], (v.min[2] + v.max[2]) / 2.);
                let floor = physics.as_deref().and_then(|ph| ground(ph, top)).map_or(v.min[1], |g| g.y);
                arch(&v, from, floor, outline)
            }
        };
        let mesh = d.meshes.entry(look.ribbon.clone()).or_insert_with(|| meshes.add(crate::skate2_arrows::ribbon(outline))).clone();
        let (glow, darken) = (d.materials.entry(look.style.clone()))
            .or_insert_with(|| {
                let [r, g, b, alpha] = style.tint;
                let pass = |texture: &str, alpha_mode, color| StandardMaterial {
                    base_color: color,
                    base_color_texture: Some(server.load(format!("{INDICATORS}/{texture}"))),
                    unlit: true,
                    alpha_mode,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                };
                let mut glow = pass(&style.texture, AlphaMode::Add, Color::linear_rgba(r, g, b, alpha));
                glow.depth_bias = 1.;
                let darken = style.background.as_deref().map(|t| materials.add(pass(t, AlphaMode::Blend, Color::WHITE)));
                (materials.add(glow), darken)
            })
            .clone();
        let root = commands
            .spawn((Name::new(format!("Death Race indicator {name}")), transform, Visibility::Inherited))
            .with_children(|root| {
                if let Some(darken) = darken {
                    root.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(darken)));
                }
                root.spawn((Mesh3d(mesh), MeshMaterial3d(glow)));
            })
            .id();
        d.roots.push(root);
    }
}

/// A racer's trigger entry, relayed by its console.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Relay {
    /// A volume this console's racer entered, at its race clock (seconds since its run_heat).
    Enter(i64, f64),
    /// [`skate3_modes::EndSync`]'s marker: this console is done with the heat.
    End(skate3_modes::EndMark),
}

struct Play {
    id: String,
    row: Row,
    g: Race,
    rt: Rt<St>,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    inbox: Vec<(usize, Relay)>,
    placed: u32,
    /// The scene playing and its end.
    playing: Option<String>,
    sync: EndSync,
}

impl Play {
    /// This console's racer is racing (its entries count).
    fn live(&self) -> bool {
        self.rt.state == St::RunHeat && !self.sync.holding() && self.g.comps.get(self.me).is_some_and(|c| c.active)
    }
    /// One frame of the heat: the clock (online, its end held for the other consoles), this racer's entries (`seen`),
    /// the others' relays. Returns what to relay.
    fn exchange(&mut self, now: f64, seen: Vec<i64>, received: Vec<(usize, Relay)>) -> Vec<String> {
        let mut out = Vec::new();
        let text = |r: &Relay| serde_json::to_string(r).expect("plain data");
        if let Some(phase) = skate3_modes::advance_synced(&mut self.g, &mut self.rt, now, St::RunHeat, self.me, &mut self.sync)
            && self.online
        {
            out.push(text(&Relay::End(phase)));
        }
        for v in seen {
            if !self.live() {
                continue;
            }
            if self.online {
                let clock = self.g.started.map_or(0., |s| self.rt.now - s);
                out.push(text(&Relay::Enter(v, clock)));
            }
            skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Trigger { player: self.me, volume: v, inside: true });
        }
        // The others' entries, from the race's start (an early one waits for run_heat; any after it are past).
        for (from, r) in received {
            match r {
                Relay::End(phase) => self.sync.ended(from, phase),
                r => self.inbox.push((from, r)),
            }
        }
        match self.rt.state {
            St::RunHeat => {
                for (from, r) in std::mem::take(&mut self.inbox) {
                    if let Relay::Enter(v, clock) = r
                        && from != self.me
                    {
                        self.g.stamp = Some(clock);
                        skate3_modes::dispatch(&mut self.g, &mut self.rt, Msg::Trigger { player: from, volume: v, inside: true });
                        self.g.stamp = None;
                    }
                }
            }
            St::EndHeat | St::CleanupHeat | St::BuildResults | St::OutroNis | St::CleanupChallenge | St::Complete => self.inbox.clear(),
            _ => {}
        }
        // An End marker for a heat that ended here between the frames (the last finish arrived).
        if let Some(phase) = skate3_modes::advance_synced(&mut self.g, &mut self.rt, now, St::RunHeat, self.me, &mut self.sync)
            && self.online
        {
            out.push(text(&Relay::End(phase)));
        }
        out
    }
}

#[derive(Resource, Default)]
pub(crate) struct RaceGame {
    data: Option<ModeData>,
    play: Option<Play>,
    /// The volumes the probe watches while this console's racer races, and what it saw.
    volumes: Vec<RaceVolume>,
    inside: Vec<bool>,
    seen: Vec<i64>,
    live: bool,
    /// OnlineEndCameraLocation while it has the view.
    camera: Option<Matrix>,
    warned: String,
    looks: Option<Looks>,
}

pub(crate) struct Skate3RacePlugin;
impl Plugin for Skate3RacePlugin {
    fn build(&self, app: &mut App) {
        let data = ModeData::install(app, "race", "Death Race");
        if let Some(data) = &data {
            let ids: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).is_ok_and(|r| r.playable(id))).cloned().collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                // The rows, and their Team tab rows (race_shared's TeamsEnabled path).
                playable.0.extend(ids.iter().cloned().chain(ids.iter().map(|id| crate::match_core::team(id))));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        let looks = data.as_ref().and_then(|d| {
            let path = d.root.join("indicators.json");
            let text = std::fs::read_to_string(&path).map_err(|e| warn!("Skate 3 Death Race: no ribbons ({}: {e})", path.display())).ok()?;
            serde_json::from_str(&text).map_err(|e| warn!("Skate 3 Death Race: {}: {e}", path.display())).ok()
        });
        app.insert_resource(RaceGame { data, looks, ..default() })
            .add_systems(Update, visuals.after(run))
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics))
            .add_systems(Update, end_camera.after(crate::camera::present).before(crate::app::FrameSet::Verification));
    }
}

/// Every physics tick while this console's racer races: the volumes it enters.
fn probe(mut game: ResMut<RaceGame>, skater: Res<SkaterRuntime>) {
    let RaceGame { volumes, inside, seen, live, .. } = &mut *game;
    if !*live {
        inside.clear();
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let at = Vec3::new(at[0], at[1], at[2]);
    inside.resize(volumes.len(), false);
    for (i, (was, v)) in inside.iter_mut().zip(volumes.iter()).enumerate() {
        let now = v.contains(at);
        if now && !*was {
            seen.push(i as i64);
        }
        *was = now;
    }
}

/// A finished racer watches from OnlineEndCameraLocation while others race (SwitchChallengeCamera): it looks along the
/// locator's Z row, as the Skate 2 port reads the same field (skate2_death_race::end_camera).
fn end_camera(game: Res<RaceGame>, mut cameras: Query<&mut Transform, With<crate::camera::GameplayCamera>>) {
    let Some(c) = game.camera else { return };
    for mut transform in &mut cameras {
        *transform = Transform::from_translation(Vec3::from_slice(&c[3][..3])).looking_to(Vec3::from_slice(&c[2][..3]), Vec3::Y);
    }
}

/// The heat's ResetReplayBuffer, once setup_heat asked for it (review I7).
fn reset_replay(world: &mut World, race: &mut Race) {
    if std::mem::take(&mut race.replay_reset)
        && let Some(mut replay) = world.get_resource_mut::<crate::replay::Replay>()
    {
        replay.reset_recording();
    }
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<RaceGame>| {
        let Some(data) = game.data.take() else { return };
        step(world, &mut game, &data);
        game.data = Some(data);
    });
}

fn step(world: &mut World, game: &mut RaceGame, data: &ModeData) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && mode == Some(Mode::DeathRace) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 Death Race: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
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
    // This console's racer's volume entries and the others' relays.
    let received: Vec<(usize, Relay)> = if play.online {
        world.resource_mut::<Match>().inbox.drain(..).filter_map(|(from, r)| Some((from, serde_json::from_str(&r).ok()?))).collect()
    } else {
        Vec::new()
    };
    let out = play.exchange(now, std::mem::take(&mut game.seen), received);
    world.resource_mut::<Match>().outbox.extend(out);
    reset_replay(world, &mut play.g);
    game.live = play.live();
    game.volumes = play.row.volumes();

    // RepositionAI: this racer to its start slot.
    if let Some((seq, place)) = play.rt.landed(play.me)
        && play.placed != seq
    {
        play.placed = seq;
        if let Some(at) = play.row.heat().start_location.as_ref().map(|l| l.slot(place.slot)) {
            world.resource_mut::<Match>().teleport = Some(at);
        }
    }

    // The start and outro NIS.
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
    // The loading screen: up until the start NIS's notify closes it (623-630), the NIS showing through it (R1).
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = play.rt.covered && play.playing.is_none();
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        let alpha = play.rt.fade_alpha(now) as f32;
        if fade.0 != alpha {
            fade.0 = alpha;
        }
    }
    game.camera = play.g.end_camera.then(|| play.row.end_camera.as_ref().map(|l| l.matrix)).flatten();

    // ponytail: the race stats natives (Race.RegisterPlayerTimer / SetPlayerProgress / FinishPlayer) rank the
    // leaderboard and write its times; not traced. Finishers by time, then the racers by split and gates passed, each
    // with its time (running while it races); a racer who left shows "--".
    if play.rt.hud.board.visible {
        let started = play.g.started;
        let comps = &play.g.comps;
        let mut order: Vec<usize> = (0..comps.len()).collect();
        let key = |i: &usize| {
            let c = &comps[*i];
            (c.total_time <= 0., if c.total_time > 0. { c.total_time } else { 0. }, std::cmp::Reverse((c.last_lap, c.gates)), *i)
        };
        order.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
        let board = &mut play.rt.hud.board;
        board.texts = vec![String::new(); comps.len()];
        for (rank, &i) in order.iter().enumerate() {
            if let Some(r) = board.ranks.get_mut(i) {
                *r = rank as i64 + 1;
            }
            let c = &comps[i];
            let t = if c.total_time > 0. { c.total_time } else { started.map_or(0., |s| now - s) };
            board.texts[i] = format!("#{}", Mode::DeathRace.show(ms(t)));
            if let Some(v) = board.ints.get_mut(i) {
                *v = if c.disconnected { -1 } else { 0 };
            }
        }
        // ponytail: with teams, Race.AddPlayerToRace(id, _, team) has the native rank the teams and move the
        // position DPO (not traced): the team of the racer ranked first leads, the DPO shows this racer's place.
        if play.g.teams_on
            && let Some(&first) = order.first()
        {
            let lead = comps[first].team;
            board.team_ranks = [1, 2].map(|t| if t == lead { 1 } else { 2 });
            if let Some(place) = order.iter().position(|&i| i == play.me) {
                play.rt.hud.dpo.a = place as i64 + 1;
            }
        }
    }
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = play.rt.pinned.get(play.me).copied().unwrap_or(true) || play.rt.presenting_now();
        m.gestures = skate3_modes::gestures(&m, play.rt.left);
        m.watch = None;
        m.score = play.g.my_time().map_or(DNF, ms);
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
    }
    // Teams: the results rows are the two teams (0 / 1), this player's team the highlighted one; the leaderboard is
    // hidden then (cleanup_heat).
    let teams = play.g.teams_on && play.rt.left;
    let team_view: Vec<String> = play.g.teams.iter().map(|t| t.name.clone()).chain(play.names.iter().skip(2).cloned()).collect();
    let my_team = play.g.comps.get(play.me).map_or(0, |c| c.team.saturating_sub(1));
    let view = if teams {
        View { names: &team_view, me: my_team, online: play.online, title: data.text(&play.row.title), label: "Death Race" }
    } else {
        View { names: &play.names, me: play.me, online: play.online, title: data.text(&play.row.title), label: "Death Race" }
    };
    // The results once the outro is over (ponytail: the front end's results screen order is not traced).
    let results = play.rt.left.then(|| ResultsSetup {
        // 2334: the FinalResults group's title over the team table.
        title: data.text(if teams { "ID_CHALLENGE_COMMON_RACE_STANDINGS" } else { "ID_CHALLENGE_COMMON_RACE_TIMES" }),
        layout: if teams { TEAM_RESULTS } else { DEATH_RACE },
        columns: [
            "ID_LEADERBOARD_HEADER_RANK",
            if play.online { "ID_LEADERBOARD_HEADER_SKATER" } else { "ID_LEADERBOARD_NAME_HEADER" },
            // ponytail: build_results sets no column titles on the team group; the round group's points title
            // (2362).
            if teams { "ID_LEADERBOARD_HEADER_RACE_POINTS" } else { "ID_LEADERBOARD_HEADER_TIME" },
        ]
        .map(|c| data.text(c))
        .to_vec(),
        throwdown: 0,
        choice: None,
        // ponytail: Results_SetPlayerTime's text is the native's (not traced): m:ss.cc, DNF under 0.
        times: (play.g.times.iter())
            .map(|&(p, t)| (p, if t < 0. { format!("#{}", data.text("ID_RACE_DNF")) } else { format!("#{}", Mode::DeathRace.show(ms(t))) }))
            .collect(),
        floats: Vec::new(),
        strings: Vec::new(),
    });
    play.hud.teams = teams.then(|| team_results(data, &play.g, &play.names, &view.title, play.online));
    play.hud.drive(world, data, &play.rt.hud, &view, results.as_ref(), now);
}

/// endchallenge_teams' data block (skate3_modes::team_block) for a team race, as build_results (2331-2406) leaves
/// it: the team rows SetPlayerData(k, Rank, TotalPoints); the RoundResults group PlaceNameScoreTime (6) with each
/// team's members by HeatTimeSorter, [rank, name, points, time] (Results_SetPlayerTime +60, 0x825BCCB8: DNF under 0).
/// build_results never calls Results_SetIsFinalResults, so the reset value (1, 0x825ECD18) stands: final results.
fn team_results(data: &ModeData, g: &Race, names: &[String], title: &str, online: bool) -> serde_json::Value {
    let time = |t: f64| if t < 0. { format!("#{}", data.text("ID_RACE_DNF")) } else { format!("#{}", Mode::DeathRace.show(ms(t))) };
    let table = |k: usize| skate3_modes::TeamTable {
        name: g.teams[k].name.clone(),
        rank: g.teams[k].rank,
        score: skate3_modes::team_int(data, g.teams[k].points),
        status: skate3_modes::team_int(data, 0),
        members: (g.team_standings[k].iter().enumerate())
            .map(|(i, s)| {
                let name = format!("#{}", names.get(s.player).map_or("", String::as_str));
                [skate3_modes::team_int(data, i as i64 + 1), name, skate3_modes::team_int(data, s.score), time(s.time)]
            })
            .collect(),
    };
    let team = g.comps.get(g.me).map_or(0, |c| c.team.saturating_sub(1)).min(1);
    let row = g.team_standings[team].iter().position(|s| s.player == g.me).map_or(-1, |i| i as i64);
    skate3_modes::team_block(online, title, true, (team, row), 6, [table(0), table(1)])
}

fn begin(world: &mut World, data: &ModeData, id: &str, row: Row, now: f64) -> Play {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (vec!["Player 1".to_owned()], 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let length = |s: &Option<String>| s.as_ref().and_then(|s| data.intros.get(s)).map_or(0., |&l| f64::from(l));
    let starts = row.heat().start_scenes.iter().map(length).collect();
    // Not retail (online audit 10-28, G4's interim): retail binds every connected racer into OnlineOutroSoloNIS in
    // finish order (2209-2217); no installed scene has a character track (tutorial.json `tracks: []`), so its shot would
    // show no one, at its origin far from the racers. The outro gets no length: no scene plays, the notify comes at once.
    let outro = 0.;
    info!("Skate 3 Death Race: {id} ({}), {} racers, this one {me}", if online { "online" } else { "solo" }, names.len());
    let mut g = Race::new(row.clone(), starts, outro, names.len(), me);
    // A team lobby's teams (match_core::Match::teams, the host's split); Teams.GetName's EA Online team names are gone:
    // retail's unranked "Red Team" / "Blue Team" (online_moves::TEAM_NAMES).
    if online && m.teams.len() == names.len() {
        g.teams_in = m.teams.clone();
    }
    g.team_names = crate::online_moves::TEAM_NAMES.map(|id| data.text(id));
    let mut rt = Rt::new(St::Initial, names.len(), now);
    rt.gone = (0..names.len()).map(|p| online && m.players.get(p).is_none_or(String::is_empty)).collect();
    rt.online = online;
    skate3_modes::start(&mut g, &mut rt);
    Play { id: id.to_owned(), row, g, rt, names, me, online, hud: HudDriver::default(), inbox: Vec::new(), placed: 0, playing: None, sync: EndSync::default() }
}

fn end(world: &mut World, game: &mut RaceGame) {
    if let Some(play) = game.play.take() {
        info!("Skate 3 Death Race: {} ended", play.id);
        if play.playing.is_some() {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
    }
    game.live = false;
    game.camera = None;
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

    fn vol(name: &str) -> Option<RaceVolume> {
        Some(RaceVolume { name: name.into(), min: [0.; 3], max: [1.; 3] })
    }

    /// A row: `gates` gates g1.. with `bonus` seconds each; the splits are the gates themselves (`splits_are_gates`, as
    /// race_dwtn_02) or the last gate alone (as race_dwtn_01_ol); a terminate volume t1 when `terminate`.
    fn row(gates: usize, splits_are_gates: bool, skipable: bool, bonus: i64, limit: i64, warning: i64, include321: bool, starts: usize, terminate: bool) -> Row {
        let names: Vec<String> = (1..=gates).map(|i| format!("g{i}")).collect();
        let splits = if splits_are_gates { names.iter().map(|n| vol(n)).collect() } else { vec![vol(&names[gates - 1])] };
        Row {
            title: "ID_T".into(),
            online: true,
            objectives: usize::from(gates % 2 == 1),
            race_type: 0,
            skipable,
            place_higher: 1,
            countdown_delay: 1.5,
            owned_time: 3.0999999046325684,
            killed_time: 3.299999952316284,
            end_camera: Some(Locator { matrix: [[0.; 4]; 4], slots: vec![] }),
            failed: "ID_FAILED".into(),
            team_intro_delay: 3.,
            team_indication: "ID_TEAM".into(),
            intro_nis: Some("IntroNIS".into()),
            online_outro_nis: Some("OnlineOutroNIS".into()),
            grey: None,
            last: None,
            leg_finish: None,
            chevrons: Vec::new(),
            heats: vec![Heat {
                start_location: Some(Locator { matrix: [[0.; 4]; 4], slots: vec![[[0.; 4]; 4]; 6] }),
                time_limit: limit,
                time_limit_warning: warning,
                include321,
                knockout_per_leg: 0,
                wipe_out_limit: 0,
                race_terminate_volumes: if terminate { vec![vol("t1")] } else { vec![] },
                legs: vec![Leg { splits, gates: names.iter().map(|n| Gate { volume: vol(n), bonus, ribbon: None }).collect() }],
                start_scenes: (0..starts).map(|i| Some(format!("nis{i}"))).collect(),
            }],
            outro: Some("outro".into()),
        }
    }

    /// Plans: every racer runs the gates in order from the start (sometimes skipping one, sometimes re-entering one),
    /// most reach the finish, some run out of time; terminate volumes now and then; rarely a racer leaving.
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
        let gates = r.heat().legs[0].gates.len();
        let limit = r.heat().time_limit as f64;
        let mut run = Vec::new();
        for p in 0..n {
            let pace = limit * (0.4 + next(80) as f64 / 100.) / gates as f64;
            let mut t = 0.13 * (p + 1) as f64;
            for g in 1..=gates {
                t += pace * (0.6 + next(80) as f64 / 100.);
                if next(9) != 0 {
                    run.push((t, Ev::Other("enter", index(&format!("g{g}")), p as f64)));
                }
                if next(15) == 0 {
                    let back = 1 + next(g as u64) as usize;
                    run.push((t + 0.21, Ev::Other("enter", index(&format!("g{back}")), p as f64)));
                }
                if r.heat().race_terminate_volumes.len() == 1 && next(60) == 0 {
                    run.push((t + 0.4, Ev::Other("enter", index("t1"), p as f64)));
                }
            }
        }
        if leaves && next(4) == 0 {
            let p = next(n as u64) as usize;
            if p != me {
                run.push((0.5 + next((limit * 10.) as u64) as f64 / 10., Ev::Leave(p)));
            }
        }
        vec![run]
    }

    const SETUP: &str = r#"
local function list(t) local o = {Count = #t} for i, v in ipairs(t) do o[i] = v end return o end
local gates = {}
for i = 1, GATES do gates[i] = {GateVolume = "g" .. i, Time_Bonus = BONUS, IndicatorType = 2, RibbonInstance = "r" .. i} end
local splits = {}
if SPLITS_ARE_GATES then for i = 1, GATES do splits[i] = "g" .. i end else splits[1] = "g" .. GATES end
local niss = {}
for i = 1, STARTS do niss[i] = "nis" .. i end
local terminate = {}
if TERMINATE then terminate[1] = "t1" end
HEAT = {StartLocation = "StartLocation0", TimeLimit = LIMIT, TimeLimitWarning = WARNING, Include321 = I321,
  KnockoutPerLeg = 0, NoPushing = false, WipeOutLimit = 0, AirUnderRequirement = 0, KilledItTime = 90,
  RaceTerminateVolumes = list(terminate), MusicPace = {Count = 0}, OnlineNISs = list(niss), AIPaths = {},
  NISFlythroughDefinition = "fly", NISFlythroughDefinitionOnline = "online", AIPathIndexOverride = -1,
  Legs = {Count = 1, {Gates = list(gates), SplitTimeTriggers = list(splits), EndTimeBonus = 0}}}
SELF = setmetatable({Name = "race", Title = "ID_T", RaceType = 0, RaceGateSkipable = SKIP, PlaceHigherToWin = 1,
  RaceTrickForTimeMultiplier = 10, MaxMiniMapIcons = 3, CountdownDelay = 1.5, TeamIntroductionDelay = 3,
  OwnedItHUDDisplayTime = 3.0999999046325684, KilledItHUDDisplayTime = 3.299999952316284,
  OnlineOutroSoloNIS = "outro", OnlineEndCameraLocation = "endcam", CentralMessageHALIDChallengeFailed = "ID_FAILED",
  CentralMessageHALIDTeamIndication = "ID_TEAM",
  IntroPresentationEvents = {Count = 0}, VisualIndicators = {Count = 0}, Objectives = {Count = OBJECTIVES},
  RaceHeats = {Count = 1, HEAT}}, {__index = function(_, k) return k end})
local function ms(t) return math.floor(t * 1000 + 0.5) end
local IN_RACE = false
Race = setmetatable({eRaceType = {Downhill = 0},
  StartRaceStats = function() IN_RACE = true end, StopRaceStats = function() IN_RACE = false end,
  ResetRaceStats = function() IN_RACE = false end, IsInRace = function() return IN_RACE end,
  GetRank = function() return 0 end}, getmetatable(Online))
ChallengeStatus.GetHighScore = function() return 0 end
AI.LoadRaceHeatPaths = function() return 0 end
AI.GetPathCount = function() return 0 end
HUD2.eLeaderboardHUDMenuType.RankNameTime = 4
HUD2.ShowChallengeFailedMessage = function(t) log_line("failed " .. t) end
FrontEnd.eResultsLBType.DeathRace = 3
FrontEnd.Results_SetPlayerTime = function(id, t) log_line(string.format("time %d %d", id, ms(t))) end
FrontEnd.Results_SetPlayerStatus = function(id, s) log_line(string.format("status %d %s", id, s)) end
FrontEnd.Results_SetSecondLBPlayerData = function(id, _, rank, v) log_line(string.format("result2 %d %d %d", id, rank, v)) end
FrontEnd.Results_SetSecondLBPlayerTime = function(i, t) log_line(string.format("time2 %d %d", i, ms(t))) end
HUD2.LeaderboardHUDSetShowPlayers = function(_, on) log_line("showplayers " .. tostring(on)) end
ChallengeManager.SwitchChallengeCamera = function(on) log_line("endcam " .. tostring(on)) end
VisualIndicators.AddTriggerIndicator = function(_, v, _, r) log_line(string.format("ind+ %s %s", v, r)) end
VisualIndicators.RemoveTriggerIndicator = function(_, v) log_line("ind- " .. v) end
VisualIndicators.RemoveByType = function() log_line("indclear") end
NIS.EnterOnlineNISUsingDefinition = NIS.EnterNISUsingDefinition
Online.OnlineChallengeComplete = function(r, t)
  for _, x in ipairs(r) do log_line(string.format("report %d %d %d %d", x[1], x[3], ms(x[4]), x[7])) end
  for i, x in ipairs(t or {}) do log_line(string.format("teamreport %d %d %d", i, x[1], x[2])) end
end
function ACTOR() return nil end
function DELIVER(kind, v, w)
  if kind == "enter" then SEND("triggerentercollision", {PlayerID = math.tointeger(w), VolumeID = VOLS[v + 1]}); return true end
  return false
end
"#;

    fn oracle(src: &str, n: usize, me: usize, r: &Row, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        team_oracle(src, n, me, r, plans, &[])
    }

    /// The oracle with Online.IsTeamChallenge true and Online.GetTeam answering `teams` (empty: no teams).
    fn team_oracle(src: &str, n: usize, me: usize, r: &Row, plans: &[Vec<(f64, Ev)>], teams: &[u8]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let h = r.heat();
        let vols: Vec<String> = r.volumes().iter().map(|v| format!("{:?}", v.name)).collect();
        let mut setup = format!(
            "N, ME, FIRST, OBJECTIVES = {n}, {me}, 0, {}\nGATES, SPLITS_ARE_GATES, SKIP, BONUS, LIMIT, WARNING, I321, STARTS, TERMINATE = {}, {}, {}, {}, {}, {}, {}, {}, {}\nVOLS = {{{}}}\n{SETUP}",
            r.objectives,
            h.legs[0].gates.len(),
            h.legs[0].splits.len() > 1,
            r.skipable,
            h.legs[0].gates[0].bonus,
            h.time_limit,
            h.time_limit_warning,
            h.include321,
            h.start_scenes.len(),
            !h.race_terminate_volumes.is_empty(),
            vols.join(", "),
        );
        if !teams.is_empty() {
            let t: Vec<String> = teams.iter().enumerate().map(|(i, t)| format!("[{i}] = {t}")).collect();
            setup.push_str(&format!("TEAMS = {{{}}}\n", t.join(", ")));
        }
        if r.online_outro_nis.is_none() {
            setup.push_str("getmetatable(SELF).__index = function(_, k) if k ~= \"OnlineOutroNIS\" then return k end end\n");
        }
        let (lua, errors) = lua_log(src, &setup, "initial", "run_heat", plans);
        let mut g = Race::new(r.clone(), vec![0.; h.start_scenes.len()], 0., n, me);
        g.teams_in = teams.to_vec();
        let mut rt = Rt::new(St::Initial, n, 0.);
        let other = |_: &mut Race, _: &mut Rt<St>, kind: &str, v: i64, w: f64| {
            (kind == "enter").then_some(Msg::Trigger { player: w as usize, volume: v, inside: true })
        };
        let model = model_log(&mut g, &mut rt, St::RunHeat, plans, |_| None, |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// race_shared.lua itself (S3_RACE_DIR = tools/skate3_online.py's `--out`/race, with scripts/) decides every race
    /// the model runs the same way: every state, timer, teleport, pin, HUD2 call, finish, result, time and report;
    /// 1 to 6 racers, the splits on every gate or the finish alone, skippable or ordered gates, time bonuses, the
    /// countdown on or off, 0 to 5 start scenes, the HUD timer from the warning or from the start, terminate volumes,
    /// racers leaving mid-race.
    #[test]
    #[ignore = "requires the decompiled race_shared.lua (S3_RACE_DIR)"]
    fn skate3_race_matches_race_shared_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_RACE_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/race_shared.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=50u64 {
            for n in 1..=6usize {
                let s = seed as usize;
                let me = s % n;
                let limit = [30, 45, 20][s % 3];
                let warning = [10, 30, 60][s / 3 % 3];
                let r = row(3 + s % 5, s % 2 == 0, s % 7 != 3, if s % 5 == 2 { 5 } else { 0 }, limit, warning, s % 4 == 1, s % 6, s % 9 == 4);
                let plans = plans(seed * 97 + n as u64, &r, n, me, seed % 3 == 0);
                let (lua, model, errs) = oracle(&src, n, me, &r, &plans);
                compare(&format!("seed {seed} n {n} me {me}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("report")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("race_shared.lua oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// race_shared.lua's TeamsEnabled path (Online.IsTeamChallenge, Online.GetTeam) decides every team race the model
    /// runs the same way: the team leaderboard and position DPO, the team message and its delay, the extra point, the
    /// team points and ranks (ties to the first finisher's team), the win, the team NISs, the team results and report;
    /// 2 to 6 racers in even, uneven and one-sided splits, racers leaving.
    #[test]
    #[ignore = "requires the decompiled race_shared.lua (S3_RACE_DIR)"]
    fn skate3_race_teams_match_race_shared_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_RACE_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/race_shared.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let s = seed as usize;
                let me = (s + 1) % n;
                let limit = [30, 45, 20][s % 3];
                let warning = [10, 30, 60][s / 3 % 3];
                let mut r = row(3 + s % 5, s % 2 == 0, s % 7 != 3, if s % 5 == 2 { 5 } else { 0 }, limit, warning, s % 4 != 1, s % 6, s % 9 == 4);
                if seed % 5 == 0 {
                    r.online_outro_nis = None;
                }
                let teams: Vec<u8> = (0..n)
                    .map(|i| match seed % 3 {
                        0 => 1 + (i % 2) as u8,
                        1 => if i < (n + 1) / 2 { 1 } else { 2 },
                        _ => if seed % 9 == 2 { 2 } else { 1 + ((i + s) % 2) as u8 },
                    })
                    .collect();
                let plans = plans(seed * 89 + n as u64, &r, n, me, seed % 3 == 1);
                let (lua, model, errs) = team_oracle(&src, n, me, &r, &plans, &teams);
                compare(&format!("seed {seed} n {n} me {me} teams {teams:?}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("teamreport")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("race_shared.lua team oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// A team race by hand: the team message before the countdown, the extra point in 4 racers, the team with more
    /// points ranks first and this player's team wins; the results show the two teams.
    #[test]
    fn team_points_decide_the_race() {
        let r = row(3, false, true, 0, 60, 30, true, 0, false);
        let finish = r.volumes().iter().position(|v| v.name == "g3").unwrap() as i64;
        let mut g = Race::new(r, vec![], 0., 4, 2);
        g.teams_in = vec![1, 2, 2, 1];
        let mut rt = Rt::new(St::Initial, 4, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunHeat);
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("teamcentral ID_TEAM 1 0 T2")), "{log:?}");
        assert!(log.iter().any(|l| l.ends_with("board 4 4 teams")) && rt.hud.board.hide_players && rt.hud.board.team_highlight == [false, true]);
        assert_eq!(rt.hud.dpo.team, Some(1), "the position DPO in blue");
        let t = rt.now + 10.;
        skate3_modes::advance(&mut g, &mut rt, t);
        for p in [0, 2, 1] {
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: p, volume: finish, inside: true });
        }
        let t = rt.now + 70.;
        skate3_modes::advance(&mut g, &mut rt, t);
        until(&mut g, &mut rt, St::Complete);
        // Places 1-3 score 3, 2, 1 (+1 each, the last none); player 3 never finished.
        assert_eq!(g.teams.each_ref().map(|t| t.points), [4, 5]);
        assert_eq!(g.teams.each_ref().map(|t| t.rank), [2, 1]);
        assert_eq!(g.result, Flow::Win, "this player's team ranks first");
        assert_eq!(rt.hud.results, vec![(0, 2, 4), (1, 1, 5)]);
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with("teamreport 2 1 5")));
    }

    /// Matrix Step 2 (10-12): a team race at 6 (3 a side, teams 1 2 1 2 1 2), 150-300 ms, 5-10 % loss, no leaver, a
    /// guest (3) or the host (0) leaving 10 s in. By hand (race_shared 1950-1985: places 1-5 score 5..1, the last
    /// place and a DNF 0, no +1 above 4 racers; TeamResults by TotalPoints): racer p in at 25 + 0.05 p. No leaver:
    /// team 1 (0 2 4) 5 + 3 + 1 = 9, team 2 6. Player 3 gone: 5 4 3 _ 2 1: team 1 10, team 2 5. Player 0 gone: team 1
    /// 0 + 4 + 2 = 6, team 2 5 + 3 + 1 = 9: team 2 first.
    #[test]
    fn six_racers_in_two_teams_rank_the_race_as_retail() {
        let gates = |finish: f64| vec![(finish * 0.3, 1), (finish * 0.6, 2), (finish, 3)];
        let plan = |p: usize| gates(25. + 0.05 * p as f64);
        let teams = [1u8, 2, 1, 2, 1, 2];
        for seed in 1..=6u64 {
            let leaver = match seed % 3 {
                0 => None,
                1 => Some((3, 10.)),
                _ => Some((0, 10.)),
            };
            let all = heat_over_the_wire_teams(6, seed, (0.15, 0.3), 0.05 + 0.01 * (seed - 1) as f64, &plan, leaver, &teams);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            let want = match leaver {
                None => [(9, 1), (6, 2)],
                Some((3, _)) => [(10, 1), (5, 2)],
                _ => [(6, 2), (9, 1)],
            };
            assert_eq!(all[0].2, want, "seed {seed} leaver {leaver:?}: the teams (points, rank)");
        }
    }

    fn until(g: &mut Race, rt: &mut Rt<St>, state: St) {
        for _ in 0..1000 {
            if rt.state == state {
                return;
            }
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}", rt.state));
            skate3_modes::advance(g, rt, t);
        }
        panic!("never reached {state:?}");
    }

    /// The race by hand: everyone starts on their slot, the finish scores the place, the first finisher who is this
    /// console's player wins, the last one in ends the heat, the times go to the results.
    #[test]
    fn the_finish_places_and_the_first_wins() {
        let r = row(3, false, true, 0, 60, 30, false, 5, false);
        let finish = r.volumes().iter().position(|v| v.name == "g3").unwrap() as i64;
        let mut g = Race::new(r, vec![0.; 5], 0., 3, 1);
        let mut rt = Rt::new(St::Initial, 3, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunHeat);
        assert_eq!(rt.places[2].map(|p| p.1), Some(Place { field: START, index: 0, slot: 2 }), "slot = player id");
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with(" nis")), "3 racers: OnlineNISs[2]");
        assert_eq!(rt.pinned, [false; 3], "all racing");
        let t = rt.now + 10.;
        skate3_modes::advance(&mut g, &mut rt, t);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: finish, inside: true });
        assert_eq!(g.comps[1].score, 2, "first of 3: 2 points");
        assert!((g.comps[1].total_time - 10.).abs() < 1e-9, "10 s");
        assert!(g.end_camera, "a finisher watches from the end camera");
        let t = rt.now + 5.;
        skate3_modes::advance(&mut g, &mut rt, t);
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: finish, inside: true });
        assert_eq!(rt.state, St::RunHeat, "one racer left");
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 2, volume: finish, inside: true });
        assert_ne!(rt.state, St::RunHeat, "all in: the heat ends");
        until(&mut g, &mut rt, St::Complete);
        assert_eq!(g.standings.iter().map(|s| (s.player, s.score)).collect::<Vec<_>>(), [(1, 2), (0, 1), (2, 0)]);
        let log = rt.log.as_ref().unwrap();
        assert!(!log.iter().any(|l| l.contains("failed")), "this console's racer won");
        assert!(log.iter().any(|l| l.ends_with("time 1 10000")));
        assert!(log.iter().any(|l| l.ends_with("status 1 ID_CHALLENG_RESULT_STATUS_WON")));
        assert!(log.iter().any(|l| l.ends_with("modified ID_ONLINE_EVENT_CHALLENGE_FINISH 0 P0 false")));
    }

    /// G4's interim (online audit 10-28): no installed outro poses a racer, so the glue gives a solo race's outro no
    /// length: no camera shot at its far origin, the report and the results at once. Before, OnlineOutroSoloNIS's
    /// camera played its full length (5 s here) with no one in it.
    #[test]
    fn the_glue_skips_an_outro_with_no_one_in_it() {
        let r = row(1, false, true, 0, 60, 30, false, 1, false);
        let finish = r.volumes().iter().position(|v| v.name == "g1").unwrap() as i64;
        let mut data = ModeData { root: Default::default(), rows: Default::default(), strings: Default::default(), intros: Default::default() };
        data.intros.insert("outro".into(), 5.);
        let mut world = World::new();
        world.insert_resource(Match::default());
        let mut play = begin(&mut world, &data, "race_x", r, 0.);
        until(&mut play.g, &mut play.rt, St::RunHeat);
        play.rt.log = Some(Vec::new());
        skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Trigger { player: 0, volume: finish, inside: true });
        until(&mut play.g, &mut play.rt, St::Complete);
        let log = play.rt.log.as_ref().unwrap();
        let at = |state: &str| log.iter().find(|l| l.ends_with(&format!(" enter {state}"))).map(|l| l.split(' ').next().unwrap_or("").to_owned());
        assert!(at("outro_nis").is_some(), "{log:?}");
        assert_eq!(at("outro_nis"), at("cleanup_challenge"), "no time in the outro: {log:?}");
    }

    /// Bug 10-09 #6: the indicators race_shared adds are drawn with the row's ribbons: a gate's own RibbonInstance, the
    /// finish's RaceFinalGateRibbon, each placed chevron at its locator; a gate faces the way from the start / the
    /// gate before it.
    #[test]
    fn the_shown_indicators_draw_the_rows_ribbons() {
        let look = |r: &str, s: &str| Look { ribbon: r.into(), style: s.into() };
        let mut r = row(3, false, true, 0, 30, 10, true, 1, false);
        for (i, g) in r.heats[0].legs[0].gates.iter_mut().enumerate() {
            g.ribbon = Some(look("arch", &format!("s{i}")));
        }
        r.last = Some(look("arch", "red"));
        let at = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        r.chevrons = vec![Chevron { matrix: Some(at(7.)), look: look("chev", "s0") }, Chevron { matrix: None, look: look("chev", "s0") }];
        let mut g = Race::new(r.clone(), vec![0.], 0., 1, 0);
        let mut rt = Rt::new(St::Countdown, 1, 0.);
        g.countdown_indicators(&mut rt);
        // The finish g3 is also the last gate: its final ribbon replaces the gate's.
        let names = |v: &[(&str, Ribbon)]| v.iter().map(|(n, r)| (n.to_string(), *r)).collect::<Vec<_>>();
        assert_eq!(g.indicators, names(&[("g1", Ribbon::Gate(0)), ("g2", Ribbon::Gate(1)), ("g3", Ribbon::Final)]));
        assert_eq!(drawn(&r, &g).len(), 3, "the chevrons wait for setup_heat");
        g.chevrons = true;
        let shown = drawn(&r, &g);
        let looks: Vec<(&str, &str)> = shown.iter().map(|(_, l, _)| (l.ribbon.as_str(), l.style.as_str())).collect();
        assert_eq!(looks, [("chev", "s0"), ("arch", "s0"), ("arch", "s1"), ("arch", "red")], "the unplaced chevron is left out");
        assert!(drawn(&r, &Race::new(r.clone(), vec![0.], 0., 1, 0)).is_empty(), "nothing before the countdown");
        assert!(matches!(shown[0].2, Spot::Locator(m) if m[3][0] == 7.));
        assert!(matches!(&shown[1].2, Spot::Gate(v, from) if v.name == "g1" && *from == Vec3::ZERO), "g1 faces the start");
    }

    /// Review I7: driving the race into setup_heat resets the replay recording (race_shared's ResetReplayBuffer),
    /// through the same glue step() runs.
    #[test]
    fn setup_heat_resets_the_replay() {
        let mut world = World::new();
        let mut replay = crate::replay::Replay::default();
        for i in 0..200 {
            let pose = crate::presentation::Snapshot {
                root: Transform::from_xyz(i as f32, 0., 0.),
                bones: vec![],
                camera: Transform::IDENTITY,
                fov: 1.,
            };
            replay.record(pose, 1. / 60., false);
        }
        assert_eq!(replay.frames().len(), 200);
        world.insert_resource(replay);
        let mut g = Race::new(row(2, false, true, 0, 40, 10, true, 0, false), vec![], 0., 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::SetupHeat);
        assert!(g.replay_reset, "setup_heat asks for the reset");
        reset_replay(&mut world, &mut g);
        assert!(world.resource::<crate::replay::Replay>().frames().is_empty(), "the heat starts a fresh recording");
        assert!(!g.replay_reset, "once");
    }

    /// Time up: the heat timer ends the heat for everyone; nobody who didn't finish has a time; a loss shows failed.
    #[test]
    fn time_up_ends_the_heat() {
        let r = row(2, false, true, 0, 40, 10, true, 0, false);
        let mut g = Race::new(r, vec![], 0., 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunHeat);
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.contains("countdown")), "Include321: the 3-2-1");
        let started = rt.now;
        until(&mut g, &mut rt, St::CleanupHeat);
        assert!((rt.now - started - 40.).abs() < 1e-9, "the heat timer");
        until(&mut g, &mut rt, St::Complete);
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("failed ID_FAILED")));
        assert!(log.iter().any(|l| l.ends_with("time 0 -2000")), "{log:?}");
        assert!(!g.someone_finished && log.iter().any(|l| l.ends_with("report 0 0 -2000 0")), "no finisher: no outro, DNFs reported");
    }

    /// Online audit 10-12: `n` consoles race one heat through the match relay ([`skate3_modes::tests::Wire`]), each
    /// started up to 0.3 s apart, each racer entering the gates on its own console at its `plan`'s race clock; `leaver`
    /// (player, global time) drops out and the others notice it at different moments. Every console's standings
    /// (player, points, ms) and results times.
    #[allow(clippy::type_complexity)]
    fn heat_over_the_wire(n: usize, seed: u64, latency: (f64, f64), loss: f64, plan: &dyn Fn(usize) -> Vec<(f64, usize)>, leaver: Option<(usize, f64)>) -> Vec<(Vec<(usize, i64, i64)>, Vec<(usize, i64)>)> {
        heat_over_the_wire_teams(n, seed, latency, loss, plan, leaver, &[]).into_iter().map(|(s, t, _)| (s, t)).collect()
    }
    /// [`heat_over_the_wire`] with `teams` (each player's team, 1 or 2): every console's team (points, rank) too.
    #[allow(clippy::type_complexity)]
    fn heat_over_the_wire_teams(
        n: usize,
        seed: u64,
        latency: (f64, f64),
        loss: f64,
        plan: &dyn Fn(usize) -> Vec<(f64, usize)>,
        leaver: Option<(usize, f64)>,
        teams: &[u8],
    ) -> Vec<(Vec<(usize, i64, i64)>, Vec<(usize, i64)>, [(i64, i64); 2])> {
        let r = row(3, true, true, 0, 60, 10, false, 0, false);
        let index = |g: usize| r.volumes().iter().position(|v| v.name == format!("g{g}")).unwrap() as i64;
        let mut wire = skate3_modes::tests::Wire::new(seed, latency, loss);
        let starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        let mut fed = vec![0; n];
        let plans: Vec<Vec<(f64, usize)>> = (0..n).map(plan).collect();
        let dt = 1. / 60.;
        for frame in 0..60 * 200 {
            let t = frame as f64 * dt;
            for i in 0..n {
                if t < starts[i] || leaver.is_some_and(|(p, at)| p == i && t >= at) {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| {
                    let mut g = Race::new(r.clone(), vec![], 0., n, i);
                    g.teams_in = teams.to_vec();
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    skate3_modes::start(&mut g, &mut rt);
                    let names = (0..n).map(|p| format!("P{p}")).collect();
                    Play { id: "r".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), inbox: Vec::new(), placed: 0, playing: None, sync: EndSync::default() }
                });
                if let Some((p, at)) = leaver
                    && t >= at + 0.5 + 0.4 * i as f64
                    && !play.rt.gone[p]
                {
                    play.rt.gone[p] = true;
                    skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Left(p));
                }
                let mut seen = Vec::new();
                if let Some(started) = play.g.started {
                    while fed[i] < plans[i].len() && t - started >= plans[i][fed[i]].0 {
                        seen.push(index(plans[i][fed[i]].1));
                        fed[i] += 1;
                    }
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                let out = play.exchange(t, seen, received);
                wire.send(t, i, n, out);
            }
            let staying = |i: &usize| leaver.is_none_or(|(p, _)| p != *i);
            if (0..n).filter(staying).all(|i| plays[i].as_ref().is_some_and(|p| p.rt.state == St::Complete)) {
                break;
            }
        }
        (0..n)
            .filter(|i| leaver.is_none_or(|(p, _)| p != *i))
            .map(|i| {
                let p = plays[i].as_ref().unwrap();
                assert_eq!(p.rt.state, St::Complete, "console {i} finished the heat");
                (
                    p.g.standings.iter().map(|s| (s.player, s.score, ms(s.time))).collect(),
                    p.g.times.iter().map(|&(p, t)| (p, ms(t))).collect(),
                    p.g.teams.each_ref().map(|t| (t.points, t.rank)),
                )
            })
            .collect()
    }

    /// Online audit 10-12 (Critical): every console ranks the heat the same, whatever the relay's latency, loss and
    /// order. Before the fix each console placed the finishes in the order they reached it (two racers 0.1 s apart
    /// with 150-300 ms of latency each won on their own console), timed the others' finishes on its own clock, and a
    /// finish in the heat's last moments was dropped on consoles whose timer had already ended the heat.
    #[test]
    fn every_console_ranks_the_heat_the_same() {
        let gates = |finish: f64| vec![(finish * 0.3, 1), (finish * 0.6, 2), (finish, 3)];
        // Two racers 0.1 s apart.
        let close = |p: usize| gates(20. + 0.1 * p as f64);
        let two = heat_over_the_wire(2, 1, (0.15, 0.3), 0., &close, None);
        assert_eq!(two[0], two[1], "both consoles agree");
        assert_eq!(two[0].0.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), [(0, 1), (1, 0)], "the faster racer wins");
        // (Gates are fed on 60 Hz frames: within one frame of 20 s.)
        assert!((20_000..20_017).contains(&two[0].0[0].2), "its own time, not the relay's arrival: {}", two[0].0[0].2);
        // Six racers within 50 ms, 10% loss; one finishes 50 ms before the heat's end, one never finishes.
        let six = |p: usize| match p {
            4 => gates(59.95),
            5 => vec![(5., 1)],
            p => gates(30. + 0.05 * p as f64),
        };
        for seed in 1..=8 {
            let all = heat_over_the_wire(6, seed, (0.15, 0.3), 0.1, &six, None);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            let order: Vec<usize> = all[0].0.iter().map(|s| s.0).collect();
            assert_eq!(order[..5], [0, 1, 2, 3, 4], "seed {seed}: by finish time, the last-moment finisher counted");
            assert_eq!(all[0].1.iter().find(|t| t.0 == 5).map(|t| t.1), Some(-2000), "seed {seed}: the DNF");
        }
        // A racer leaving mid-heat (each console notices it at another moment).
        for seed in 1..=4 {
            let all = heat_over_the_wire(4, seed, (0.15, 0.3), 0.05, &|p| gates(25. + 0.05 * p as f64), Some((2, 10.)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same after the leave");
            }
        }
        // Matrix Step 2 (10-12): six racers, a guest (odd seeds) or the host (player 0, even seeds) leaving at 10 s,
        // 5-10 % loss. By hand (finish 1151-1202, end_heat 1950-1979): the five stayers finish in slot order 50 ms
        // apart and score 5..1 by place; the leaver (time -1, -2 once sorted) is last on 0.
        for seed in 1..=6 {
            let quit = if seed % 2 == 0 { 0 } else { 3 };
            let all = heat_over_the_wire(6, seed, (0.15, 0.3), 0.05 + 0.01 * seed as f64, &|p| gates(25. + 0.05 * p as f64), Some((quit, 10.)));
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same after the leave");
            }
            let stayers: Vec<usize> = (0..6).filter(|&p| p != quit).collect();
            let want: Vec<(usize, i64)> = stayers.iter().zip((1..=5).rev()).map(|(&p, s)| (p, s)).chain([(quit, 0)]).collect();
            assert_eq!(all[0].0.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), want, "seed {seed}: placed by finish time");
            for s in &all[0].0[..5] {
                let at = 25_000 + 50 * s.0 as i64;
                assert!((at..at + 17).contains(&s.2), "seed {seed}: racer {}'s own time: {}", s.0, s.2);
            }
            assert_eq!(all[0].0[5].2, -2000, "seed {seed}: the leaver has no time");
        }
    }

    /// Online audit 10-12 (end review I3): a heat's end that moves later while this console holds it (another
    /// racer's gate bonus reached it) lets this racer race on. Before, the hold stayed for the rest of the heat: the
    /// leader could not steer (none of its entries counted) and no fresh End marker went out.
    #[test]
    fn a_gate_bonus_that_moves_the_end_releases_the_hold() {
        let r = row(3, true, true, 5, 20, 20, false, 0, false);
        let g1 = r.volumes().iter().position(|v| v.name == "g1").unwrap() as i64;
        let mut plays: Vec<Play> = (0..2)
            .map(|i| {
                let mut g = Race::new(r.clone(), vec![], 0., 2, i);
                let mut rt = Rt::new(St::Initial, 2, 0.);
                rt.online = true;
                skate3_modes::start(&mut g, &mut rt);
                let names = (0..2).map(|p| format!("P{p}")).collect();
                Play { id: "r".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), inbox: Vec::new(), placed: 0, playing: None, sync: EndSync::default() }
            })
            .collect();
        let mut wire = skate3_modes::tests::Wire::new(1, (0.2, 0.2), 0.);
        let (mut fed, mut held, mut live) = ([false; 2], false, None);
        for frame in 0..60 * 40 {
            let t = frame as f64 / 60.;
            for (i, play) in plays.iter_mut().enumerate() {
                // Racer 0 takes gate 1 (+5 s) early; racer 1 takes it 0.1 s before its time runs out.
                let mut seen = Vec::new();
                if let Some(s) = play.g.started
                    && !fed[i]
                    && t - s >= if i == 0 { 5. } else { 19.9 }
                {
                    (seen, fed[i]) = (vec![g1], true);
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                let out = play.exchange(t, seen, received);
                wire.send(t, i, 2, out);
            }
            held |= plays[0].sync.holding();
            if let Some(s) = plays[0].g.started
                && t - s >= 21.
                && live.is_none()
            {
                live = Some(plays[0].live());
            }
        }
        assert!(held, "console 0 held at racer 1's time");
        assert_eq!(live, Some(true), "racer 0 (5 s left) races on once racer 1's bonus arrived");
        for p in &plays {
            assert_eq!(p.rt.state, St::Complete, "the heat ended");
        }
    }

    /// Matrix G8 (10-12, the EndSync leftover): an End marker relayed before its console's end moved later is stale.
    /// Console 0 holds at racer 1's time (20 s) and relays its marker; racer 1's gate bonus (+5 s) then reaches it and
    /// racer 0 finishes at 24.9 s. Before, console 1 took that first marker at its own end (25 s) and ended the heat
    /// before racer 0's finish arrived: racer 0 finished on console 0 only.
    #[test]
    fn a_stale_end_marker_does_not_end_the_heat() {
        let r = row(3, true, true, 5, 20, 20, false, 0, false);
        let index = |g: usize| r.volumes().iter().position(|v| v.name == format!("g{g}")).unwrap() as i64;
        let plans = [vec![(5., 1), (10., 2), (24.9, 3)], vec![(19.9, 1)]];
        let mut plays: Vec<Play> = (0..2)
            .map(|i| {
                let mut g = Race::new(r.clone(), vec![], 0., 2, i);
                let mut rt = Rt::new(St::Initial, 2, 0.);
                rt.online = true;
                skate3_modes::start(&mut g, &mut rt);
                let names = (0..2).map(|p| format!("P{p}")).collect();
                Play { id: "r".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), inbox: Vec::new(), placed: 0, playing: None, sync: EndSync::default() }
            })
            .collect();
        let mut wire = skate3_modes::tests::Wire::new(1, (0.3, 0.3), 0.);
        let mut fed = [0; 2];
        for frame in 0..60 * 40 {
            let t = frame as f64 / 60.;
            for (i, play) in plays.iter_mut().enumerate() {
                let mut seen = Vec::new();
                if let Some(s) = play.g.started {
                    while fed[i] < plans[i].len() && t - s >= plans[i][fed[i]].0 {
                        seen.push(index(plans[i][fed[i]].1));
                        fed[i] += 1;
                    }
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                let out = play.exchange(t, seen, received);
                wire.send(t, i, 2, out);
            }
        }
        let results: Vec<_> = (plays.iter())
            .map(|p| {
                assert_eq!(p.rt.state, St::Complete, "the heat ended");
                (p.g.standings.iter().map(|s| (s.player, s.score, ms(s.time))).collect::<Vec<_>>(), p.g.times.iter().map(|&(p, t)| (p, ms(t))).collect::<Vec<_>>())
            })
            .collect();
        assert_eq!(results[0], results[1], "both consoles agree");
        let finish = results[1].1.iter().find(|t| t.0 == 0).map(|t| t.1);
        assert!(finish.is_some_and(|t| (24_900..24_917).contains(&t)), "racer 0's last-moment finish counts on console 1: {finish:?}");
    }

    /// Matrix Step 3 (10-12, S3_RACE_DIR): every playable online row shows its gates at the countdown in course order,
    /// each in its own RibbonInstance (every online row is RaceGateSkipable: all lit, none grey), the finish in
    /// RaceFinalGateRibbon; each draws over its own trigger volume, facing the volume before it (the first faces the
    /// start). A racer passing them leaves them up (Time_Bonus is 0 on every gate; race_shared 1416-1438 removes a
    /// gate's indicator only with a bonus), and cleanup_heat clears them all.
    #[test]
    #[ignore = "requires the Skate 3 Death Race data (S3_RACE_DIR)"]
    fn every_online_rows_gates_show_in_order_over_their_volumes() {
        let root = std::path::PathBuf::from(std::env::var("S3_RACE_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let mut rows = 0;
        for id in data.rows.keys() {
            let r: Row = data.row(id).unwrap();
            if !r.playable(id) {
                continue;
            }
            rows += 1;
            let h = r.heat();
            let leg = &h.legs[0];
            let name = |v: &Option<RaceVolume>| v.as_ref().unwrap().name.clone();
            let finish = name(leg.splits.last().unwrap());
            assert!(r.skipable && leg.gates.iter().all(|g| g.bonus == 0), "{id}: online rows skip freely, no bonus");
            let mut want: Vec<(String, Ribbon)> = leg.gates.iter().enumerate().map(|(i, g)| (name(&g.volume), Ribbon::Gate(i))).collect();
            want.retain(|(v, _)| *v != finish);
            want.push((finish.clone(), Ribbon::Final));
            let mut g = Race::new(r.clone(), vec![0.; h.start_scenes.len()], 0., 1, 0);
            let mut rt = Rt::new(St::Initial, 1, 0.);
            skate3_modes::start(&mut g, &mut rt);
            let mut t = 0.;
            while rt.state != St::RunHeat && t < 300. {
                t += 0.05;
                skate3_modes::advance(&mut g, &mut rt, t);
            }
            assert_eq!((rt.state, &g.indicators), (St::RunHeat, &want), "{id}: the countdown's gates");
            let vols = r.volumes();
            let center = |n: &str| vols.iter().find(|v| v.name == n).map(|v| (Vec3::from(v.min) + Vec3::from(v.max)) / 2.).unwrap();
            let start = Vec3::from_slice(&h.start_location.as_ref().unwrap().matrix[3][..3]);
            // The course's chevrons (row.chevrons, on locators) draw too; the gates are the volume spots.
            let shown: Vec<_> = drawn(&r, &g).into_iter().filter(|s| matches!(s.2, Spot::Gate(..))).collect();
            assert_eq!(shown.len(), want.len(), "{id}: every gate has a look");
            for (k, (v, look, spot)) in shown.iter().enumerate() {
                let Spot::Gate(vol, from) = spot else { unreachable!() };
                assert_eq!((v, &vol.name), (&want[k].0, &want[k].0), "{id}");
                let ribbon = match want[k].1 {
                    Ribbon::Gate(i) => leg.gates[i].ribbon.as_ref(),
                    _ => r.last.as_ref(),
                };
                assert_eq!(Some(*look), ribbon, "{id}: {v}");
                assert_eq!(*from, if k == 0 { start } else { center(&want[k - 1].0) }, "{id}: {v} faces the one before it");
            }
            let index = |n: &str| vols.iter().position(|v| v.name == n).unwrap() as i64;
            for (v, _) in &want[..want.len() - 1] {
                skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: index(v), inside: true });
                assert_eq!((rt.state, &g.indicators), (St::RunHeat, &want), "{id}: passed {v}");
            }
            assert!(g.comps[0].flags.iter().filter(|f| **f).count() >= want.len() - 1, "{id}: every gate counted");
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: index(&finish), inside: true });
            while rt.state != St::Complete && t < 600. {
                t += 0.05;
                skate3_modes::advance(&mut g, &mut rt, t);
            }
            assert!(g.times.iter().any(|&(p, at)| p == 0 && at > 0.), "{id}: the racer finished");
            assert!(g.indicators.is_empty(), "{id}: cleanup_heat cleared the gates");
        }
        assert_eq!(rows, 10);
    }

    /// The tool's output (S3_RACE_DIR): the online rows load with their start slots, gates, finish and scenes.
    #[test]
    #[ignore = "requires the Skate 3 Death Race data (S3_RACE_DIR)"]
    fn skate3_race_data() {
        let root = std::path::PathBuf::from(std::env::var("S3_RACE_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id)).cloned().collect();
        assert_eq!(playable.len(), 10, "{playable:?}");
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            let h = r.heat();
            assert_eq!(h.start_scenes.len(), 5, "{id}");
            assert!(h.start_scenes.iter().flatten().all(|s| data.intros.contains_key(s)), "{id}");
            assert!(r.outro.as_ref().is_some_and(|s| data.intros.contains_key(s)), "{id}");
            assert!(h.time_limit > h.time_limit_warning, "{id}");
            assert!(r.end_camera.is_some(), "{id}");
            // Bug 10-09 #6: every gate, the grey / final ribbons and the chevrons have a look indicators.json draws.
            let looks: Looks = serde_json::from_str(&std::fs::read_to_string(root.join("indicators.json")).unwrap()).unwrap();
            let gates = h.legs[0].gates.iter().map(|g| g.ribbon.as_ref());
            for l in gates.chain([r.grey.as_ref(), r.last.as_ref()]).chain(r.chevrons.iter().map(|c| Some(&c.look))) {
                let l = l.unwrap_or_else(|| panic!("{id}: a ribbon is missing"));
                assert!(looks.shapes.get(&l.ribbon).is_some_and(|s| s.len() > 1) && looks.styles.contains_key(&l.style), "{id}: {l:?}");
            }
            assert!(!r.chevrons.is_empty(), "{id}");
        }
    }
}
