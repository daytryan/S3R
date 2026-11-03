//! Skate 3's Domination on the match core (challenge graph `domination` < objectives_multiplayer < objectives <
//! default), online (also in teams: the Team tab, Online.IsTeamChallenge) and solo, for the 13 online rows of
//! DownTown, Industrial and University. Data:
//! tools/skate3_online.py, installed at `private/skate3-online/domination/`: rules.json (per row TimeLimit,
//! CountdownDelay, the central messages, Location with its 6 slots, the Objectives' ScoringBoundary volumes, the intro
//! NIS), strings.json, scripts/domination.lua (the decompiled graph: the rules' source and the tests' oracle), the
//! intro camera (tutorial.json + scene/camera).
//!
//! [`Domination`] is the graph handler by handler (line numbers are domination.lua's as the tool writes it): every
//! skater skates at once for TimeLimit; each objective (a spot) is owned by whoever landed the best sequence on it
//! (objectivecomplete, 754-786), a skater's Points are the spots it owns and its Score their sequences (CalculateScores
//! 607-654); owning, losing and out-scoring a spot, dominating (every spot) and a new leader show central messages
//! (OwnObject 496-606, HandleLeadChangeMessaging 655-669); standing in a spot shows its owner and score on the DPO
//! (789-820). The standings rank by Points then Score (PointsScoreSorter); the results show Points and Score
//! (PlaceNameIntInt) and are reported. Teams: a team's Points are the spots its members own (an exact tie ranks both
//! 2), the board shows the team rows, a spot is lost to the other team only, the team message precedes the
//! countdown, the results are the TeamChallengeResults rows and each team's members, reported with TeamResults.
//!
//! Each console evaluates its own skater's objectives (Objectives.MakeObjectives' group from the row's Objectives as
//! data, on [`skate3_objectives::Group`] with skate3_film's skater natives; ResetObjective after each completion) and
//! relays the completions (`Match::outbox`); the others' come from the relay. ponytail: retail evaluates every
//! skater's objective group on each console. Solo
//! (`SKATE_MODE=domination:<row>`) is a one-skater Domination on the online rules.
use crate::{
    challenge::cutscene::Cutscene,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
    skate3_modes::{
        self, Abort, ChallengeFade, Component, EndSync, Graph, Handled, HudDriver, Locator, ModeData, Msg, Place, RED,
        ResultsSetup, Rt, View, GREEN, WHITE,
    },
    skate3_film::{self, Frame, SENSE_NATIVES, Sense},
    skate3_modes::Volume,
    skate3_objectives::{Group as Objectives, GroupDef, ObjectiveDef, Out},
    skate3_race::RaceVolume,
    tip_loader::LoadingHold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// HUD2.eLeaderboardHUDMenuType.RankNameScore (leaderboardhud).
const RANK_NAME_SCORE: i64 = 3;
/// HUD2.eDPOFormat (0x8284ee90): Ints 0, Points 1.
const DPO_INTS: i64 = 0;
const DPO_POINTS: i64 = 1;
/// HUD2.eMessageAudioHook.Owned (registration 0x8284ddd0: Default 0, Fail 1, Success 2, Owned 3).
const HOOK_OWNED: u8 = 3;
/// FrontEnd.eResultsLBType.PlaceNameIntInt (endchallenge_online: 14; rank, name, item 1, item 2).
const PLACE_NAME_INT_INT: i64 = 14;
/// Online.eFinishReason (the oracle's stubs; UNRESOLVED as in race_shared): NOT_FINISHED 0, NORMAL_COMPLETION 1,
/// PLAYER_DISCONNECT 2.
const NOT_FINISHED: i64 = 0;
const NORMAL: i64 = 1;
const DISCONNECT: i64 = 2;
/// Challenge.EActivityCompletionType.Abandoned: the results report it for every skater (943).
const ABANDONED: i64 = 0;
const LOCATION: &str = "Location";
/// The graph's timers (seconds): wait_for_assetload (23), the load delay (341), the countdown (42).
const WAIT_PRES: f64 = 0.5;
const LOAD_DELAY: f64 = 1.;
const COUNTDOWN: f64 = 3.;
const DOMINATING: &str = "ID_DOMINATION_CENTRAL_MESSAGE_DOMINATING";
/// HUD2.eTeamMessageType (0x8284fb70): Normal 0, Winning 2.
const TEAM_NORMAL: u8 = 0;
const TEAM_WINNING: u8 = 2;
/// eResultsLBType_TeamChallengeResults 8 (889: the FinalResults group's team table).
const TEAM_RESULTS: i64 = 8;

// ---------------------------------------------------------------- data
/// An objective (challengebanks dom_<row>_<k>): its ScoringBoundary and its VisualIndicators' locators (None where no
/// mission places one).
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Objective {
    #[serde(rename = "ScoringBoundary")]
    pub boundary: Option<RaceVolume>,
    #[serde(rename = "VisualIndicators", default)]
    pub indicators: Vec<Option<skate3_modes::Matrix>>,
}

/// A row (rules.json).
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub title: String,
    pub online: bool,
    pub intro: Option<String>,
    #[serde(rename = "TimeLimit")]
    pub time_limit: f64,
    #[serde(rename = "CountdownDelay")]
    pub countdown_delay: f64,
    #[serde(rename = "CentralMessageHALIDDominationOwned")]
    pub owned: String,
    #[serde(rename = "CentralMessageHALIDDominationLostSpot")]
    pub lost: String,
    #[serde(rename = "CentralMessageHALIDDominationScoreIncreased")]
    pub increased: String,
    #[serde(rename = "CentralMessageHALIDNewLead")]
    pub new_lead: String,
    #[serde(rename = "CentralMessageHALIDNewLeadYou")]
    pub new_lead_you: String,
    /// Teams: the team message (871) and TeamIntroductionDelay (3 s on every row) less CountdownDelay before the
    /// countdown (869).
    #[serde(rename = "CentralMessageHALIDTeamIndication", default)]
    pub team_indication: String,
    #[serde(rename = "TeamIntroductionDelay", default)]
    pub team_intro_delay: f64,
    /// The team intro NIS (dynamic_character_nis 416-446; {scene, at, dir} on every row). ponytail: the NIS player
    /// with dynamic characters is not ported: only its presence is read.
    #[serde(rename = "IntroNIS", default)]
    pub intro_nis: Option<serde_json::Value>,
    /// The spots' looks (461-464, 531-548): RibbonIndicatorNeutral in RibbonColourNeutral until a spot is owned, then
    /// RibbonIndicatorOwned in RibbonColoursPlayers[owner] (teams: RibbonColoursTeams[team]). Refspecs, the key their
    /// second 8 bytes.
    #[serde(rename = "RibbonIndicatorNeutral", default)]
    pub ribbon_neutral: String,
    #[serde(rename = "RibbonIndicatorOwned", default)]
    pub ribbon_owned: String,
    #[serde(rename = "RibbonColourNeutral", default)]
    pub colour_neutral: String,
    #[serde(rename = "RibbonColoursPlayers", default)]
    pub colours_players: Vec<String>,
    #[serde(rename = "RibbonColoursTeams", default)]
    pub colours_teams: Vec<String>,
    #[serde(rename = "Location")]
    pub location: Option<Locator>,
    #[serde(rename = "objective_list", default)]
    pub objectives: Vec<Objective>,
    /// The Objectives as data (tools/skate3_online.py: each a one-objective group) and the volumes their snippets name.
    #[serde(default)]
    pub groups: Groups,
    #[serde(default)]
    pub trigger_volumes: BTreeMap<String, Option<Volume>>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Groups {
    #[serde(rename = "Objectives", default)]
    pub objectives: Vec<Option<GroupDef>>,
}

impl Row {
    /// Objectives.MakeObjectives(id)'s group: the spots' objectives in order, no PointRequirement.
    pub fn group(&self) -> Option<GroupDef> {
        let objectives: Option<Vec<_>> =
            self.groups.objectives.iter().map(|g| g.as_ref().filter(|g| g.objectives.len() == 1).map(|g| g.objectives[0].clone())).collect();
        objectives.filter(|o| o.len() == self.objectives.len()).map(|objectives| GroupDef { point_requirement: None, objectives })
    }
    /// Playable: online, its 6 start slots, every objective's volume placed, its objectives whole and ported, an
    /// intro, on a known map.
    pub fn playable(&self, id: &str, points: &ObjectiveDef) -> bool {
        self.online
            && self.location.as_ref().is_some_and(|l| l.slots.len() == 6)
            && !self.objectives.is_empty()
            && self.objectives.iter().all(|o| o.boundary.is_some())
            && self.group().is_some_and(|g| Objectives::supported(&g, points, SENSE_NATIVES).is_ok())
            && self.trigger_volumes.values().all(Option::is_some)
            && self.intro.is_some()
            && skate3_modes::map_of(id).is_some()
    }
}

// ---------------------------------------------------------------- the graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Initial,
    SetupCodeCommon,
    SetupCodeSpecific,
    WaitForAssetload,
    DynamicCharacterNis,
    IntroPresentation,
    SetupChallengeCommon,
    SetupChallengeSpecific,
    Countdown,
    RunChallenge,
    EndChallenge,
    CalculateScores,
    Highlight,
    OutroPresentation,
    Results,
    CleanupChallengeCommon,
    CleanupChallengeSpecific,
    Complete,
}

/// vars.Competitor[i] (id i - 1).
#[derive(Clone, Debug, Default)]
pub(crate) struct Comp {
    pub disconnected: bool,
    pub points: i64,
    pub score: i64,
    finish: i64,
    attempted: bool,
    in_spot: Option<i64>,
    highest: i64,
    /// Online.GetTeam (1 / 2; 1 without teams) and StartLocator (the slot: the id, teams each from its team's first
    /// slot, 220-233).
    pub team: usize,
    start_locator: usize,
}

/// vars.TeamStandings[i] (Rank 2 until ranked, 240-245).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TeamStanding {
    pub team: usize,
    pub points: i64,
    pub score: i64,
    pub rank: i64,
}

/// vars.DominationObjects[k]: the owner (1-based competitor, -1 none) and its score.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spot {
    pub owner: i64,
    pub score: i64,
    /// Online: when the owner's sequence landed on its console's challenge clock, and its id (ties go to the earlier).
    at: (f64, usize),
}

/// vars.PlayerStandings[i].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Standing {
    pub player: usize,
    pub points: i64,
    pub score: i64,
}

/// The presentation groups (ChallengePresentation.AddPresentationGroup): intro, outro, restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Intro,
    Outro,
}

#[derive(Clone)]
pub(crate) struct Domination {
    row: Row,
    intro_len: f64,
    me: usize,
    n: usize,
    pub comps: Vec<Comp>,
    setup_complete: bool,
    pub spots: Vec<Spot>,
    pub standings: Vec<Standing>,
    somebody_scored: bool,
    last_leader: i64,
    wait_pres: Option<u32>,
    load_delay: Option<u32>,
    countdown_delay: Option<u32>,
    countdown_timer: Option<u32>,
    challenge_timer: Option<u32>,
    presenting: Option<Group>,
    /// The objective being delivered (ObjectiveIndex).
    pub index: usize,
    /// The intro NIS while its presentation runs (the glue plays it).
    pub scene: Option<String>,
    /// Online, set by the glue with each objective: when it landed (its skater's challenge clock) and that skater.
    pub stamp: (f64, usize),
    /// When run_challenge began here (the challenge clock's zero).
    pub started: Option<f64>,
    /// Online.IsTeamChallenge / Online.GetTeam: each player's team (1 / 2), empty for none; Teams.GetName's names.
    pub teams_in: Vec<u8>,
    pub team_names: [String; 2],
    /// vars.TeamsEnabled, TeamStandings, TeamPlayerStandings (calculate_scores 384-400), LastNotifiedTeamLeader,
    /// TeamIntroductionTimerID.
    pub teams_on: bool,
    pub team_standings: [TeamStanding; 2],
    pub team_players: [Vec<Standing>; 2],
    last_team_leader: i64,
    team_intro: Option<u32>,
    /// The spots' VisualIndicators are in vars.VisualIndicatorGroup (setup_code_specific's AddIndicator 461-464 until
    /// cleanup_challenge_specific's RemoveIndicator 695-697).
    indicators_on: bool,
}

impl Domination {
    pub fn new(row: Row, intro_len: f64, players: usize, me: usize) -> Self {
        let spots = vec![Spot { owner: -1, score: 0, at: (0., 0) }; row.objectives.len()];
        Domination {
            row,
            intro_len,
            me,
            n: players,
            comps: vec![Comp::default(); players],
            setup_complete: false,
            spots,
            standings: Vec::new(),
            somebody_scored: false,
            last_leader: -1,
            wait_pres: None,
            load_delay: None,
            countdown_delay: None,
            countdown_timer: None,
            challenge_timer: None,
            presenting: None,
            index: 0,
            scene: None,
            stamp: (0., 0),
            started: None,
            teams_in: Vec::new(),
            team_names: ["T1".into(), "T2".into()],
            teams_on: false,
            team_standings: [1, 2].map(|team| TeamStanding { team, points: 0, score: 0, rank: 2 }),
            team_players: Default::default(),
            last_team_leader: -1,
            team_intro: None,
            indicators_on: false,
        }
    }

    /// G3 (online audit 10-28): what vars.VisualIndicatorGroup shows of the spots, each indicator at its locator in its
    /// spot's look ("skate3:<type>:<style>", domination/indicators.json): neutral until owned, then RibbonIndicatorOwned
    /// in the owner's colour, its team's in a team match (OwnObject 531-548; the colour is the owner's own, it does not
    /// go back). A look the row lacks, or an unplaced locator, draws nothing. ponytail: the map icons (474, 533) wait for
    /// a Skate 3 minimap (G5); the row's own VisualIndicators (253-255) are not exported.
    pub fn markers(&self) -> Vec<(String, skate3_modes::Matrix)> {
        if !self.indicators_on {
            return Vec::new();
        }
        let r = &self.row;
        let key = |refspec: &str| refspec.get(16..32).map(str::to_owned);
        let mut out = Vec::new();
        for (spot, objective) in self.spots.iter().zip(&r.objectives) {
            let (ribbon, colour) = match spot.owner {
                owner if owner <= 0 => (&r.ribbon_neutral, Some(&r.colour_neutral)),
                owner if self.teams_on => (&r.ribbon_owned, r.colours_teams.get(self.team_of(owner) - 1)),
                owner => (&r.ribbon_owned, r.colours_players.get(owner as usize - 1)),
            };
            let (Some(ribbon), Some(colour)) = (key(ribbon.as_str()), colour.and_then(|c| key(c.as_str()))) else { continue };
            let look = format!("skate3:{ribbon}:{colour}");
            out.extend(objective.indicators.iter().flatten().map(|m| (look.clone(), *m)));
        }
        out
    }

    fn local(&self) -> i64 {
        self.me as i64 + 1
    }
    fn name(c: i64) -> String {
        format!("P{}", c - 1)
    }
    /// vars.Competitor[c].Team (c 1-based).
    fn team_of(&self, c: i64) -> usize {
        self.comps[c as usize - 1].team
    }
    /// vars.TeamData[team].Points (CalculateScores 640).
    fn team_points(&self, team: usize) -> i64 {
        self.team_standings.iter().find(|t| t.team == team).map_or(0, |t| t.points)
    }

    /// vars:CalculateScores (607-654).
    fn calculate(&mut self) {
        for c in &mut self.comps {
            (c.points, c.score) = (0, 0);
        }
        for s in &self.spots {
            if 0 < s.owner {
                let c = &mut self.comps[s.owner as usize - 1];
                c.points += 1;
                c.score = c.score.saturating_add(s.score); // a peer's relayed score can be i64::MAX
            }
        }
        let mut st: Vec<Standing> = (0..self.n).map(|i| Standing { player: i + 1, points: self.comps[i].points, score: self.comps[i].score }).collect();
        skate3_modes::lua_sort(&mut st, |a, b| a.points > b.points || (a.points == b.points && a.score > b.score));
        self.standings = st;
        if !self.teams_on {
            return;
        }
        // 627-653: each slot back to team i, the spots' points and scores by their owner's team; an exact tie (both)
        // ranks both 2 unsorted, else PointsScoreSorter ranks 1, 2.
        for (i, t) in self.team_standings.iter_mut().enumerate() {
            (t.points, t.score, t.team) = (0, 0, i + 1);
        }
        for k in 0..self.spots.len() {
            let s = self.spots[k];
            if 0 < s.owner {
                let t = &mut self.team_standings[self.team_of(s.owner) - 1];
                t.points += 1;
                t.score = t.score.saturating_add(s.score);
            }
        }
        let [a, b] = self.team_standings;
        if a.points == b.points && a.score == b.score {
            self.team_standings.iter_mut().for_each(|t| t.rank = 2);
        } else {
            skate3_modes::lua_sort(&mut self.team_standings, |a, b| a.points > b.points || (a.points == b.points && a.score > b.score));
            for (i, t) in self.team_standings.iter_mut().enumerate() {
                t.rank = i as i64 + 1;
            }
        }
    }

    /// vars:UpdateLeaderboardHUD (277-288).
    fn update_board(&self, rt: &mut Rt<St>) {
        for (i, s) in self.standings.iter().enumerate() {
            rt.board_rank(s.player - 1, i as i64 + 1);
            rt.board_int(s.player - 1, s.points);
        }
        if self.teams_on {
            for t in &self.team_standings {
                rt.board_team_score(t.team, t.points);
                rt.board_team_rank(t.team, t.rank);
            }
        }
    }

    /// vars:OwnObject (496-606). The ribbons' new look (536-548) is the new owner's ([`Domination::markers`]).
    fn own(&mut self, rt: &mut Rt<St>, new: i64, k: usize, score: i64) {
        let local = self.local();
        let owner = self.spots[k].owner;
        let mut increased = false;
        if self.teams_on {
            // 500-517: a spot taken from the other team is lost to this player's team; from a teammate (or itself)
            // the score increased.
            let team = self.team_of(new);
            if owner <= 0 {
                if local == new {
                    let id = self.row.owned.clone();
                    rt.central(&id, GREEN, &[], HOOK_OWNED);
                }
            } else if self.team_of(owner) != team {
                if self.team_of(local) != team {
                    let id = self.row.lost.clone();
                    rt.central(&id, RED, &[self.team_names[team - 1].clone()], 0);
                }
                if local == new {
                    let id = self.row.owned.clone();
                    rt.central(&id, GREEN, &[], HOOK_OWNED);
                }
            } else {
                increased = true;
                if local == new {
                    let id = self.row.increased.clone();
                    rt.central(&id, GREEN, &[], 0);
                }
            }
        } else if owner != new {
            if local == owner {
                let id = self.row.lost.clone();
                rt.central(&id, RED, &[Self::name(new)], 0);
            }
            if local == new {
                let id = self.row.owned.clone();
                rt.central(&id, GREEN, &[], HOOK_OWNED);
            }
        } else {
            increased = true;
            if local == new {
                let id = self.row.increased.clone();
                rt.central(&id, GREEN, &[], 0);
            }
        }
        self.spots[k] = Spot { owner: new, score, at: self.stamp };
        // 551-575: every spot this owner's (teams: its team's).
        if !increased && self.teams_on {
            let team = self.team_of(new);
            if self.spots.iter().all(|s| s.owner > 0 && self.team_of(s.owner) == team) {
                rt.team_central(DOMINATING, team as i64 - 1, TEAM_WINNING, &[self.team_names[team - 1].clone()]);
            }
        } else if !increased && self.spots.iter().all(|s| s.owner == new) {
            rt.central(DOMINATING, GREEN, &[Self::name(new)], 0);
        }
        let mut old = if self.last_leader == -1 { self.standings.first().map_or(-1, |s| s.player as i64) } else { self.last_leader };
        // 583-590.
        let mut old_team = match (self.teams_on, self.last_team_leader) {
            (false, _) => -1,
            (true, -1) => self.team_standings[0].team as i64,
            (true, t) => t,
        };
        if !self.somebody_scored {
            (old, old_team) = (-1, -1);
            self.somebody_scored = true;
        }
        self.calculate();
        self.update_board(rt);
        let first = self.standings[0];
        let lead = self.team_standings[0];
        if self.teams_on && old_team > 0 && lead.points == self.team_points(old_team as usize) {
            self.last_team_leader = old_team;
        } else if !self.teams_on && old > 0 && first.points == self.comps[old as usize - 1].points {
            self.last_leader = old;
        } else if self.teams_on {
            // HandleLeadChangeMessaging (656-660), teams.
            if lead.team as i64 != old_team {
                let id = self.row.new_lead.clone();
                rt.team_central(&id, lead.team as i64 - 1, TEAM_WINNING, &[self.team_names[lead.team - 1].clone()]);
            }
            (self.last_team_leader, self.last_leader) = (-1, -1);
        } else {
            // HandleLeadChangeMessaging (661-669).
            let leader = first.player as i64;
            if leader != old {
                if local == leader {
                    let id = self.row.new_lead_you.clone();
                    rt.central(&id, GREEN, &[], 0);
                } else {
                    let id = self.row.new_lead.clone();
                    rt.central(&id, if local == old { RED } else { WHITE }, &[Self::name(leader)], 0);
                }
            }
            self.last_leader = -1;
        }
    }

    /// [run_challenge] objectivecomplete (754-786).
    fn objective(&mut self, rt: &mut Rt<St>, p: usize, score: i64) -> Handled {
        if p >= self.n {
            return Ok(());
        }
        let i = p as i64 + 1;
        self.comps[p].attempted = true;
        let k = self.index;
        let spot = *self.spots.get(k).ok_or(Abort)?;
        // Not retail (user 10-12): online an equal score takes the spot when it landed earlier (each skater's own
        // challenge clock, then its id), so every console names the same owner whichever relay reached it first.
        let earlier = rt.online && spot.owner > 0 && score == spot.score && self.stamp < spot.at;
        if score > spot.score || earlier {
            self.own(rt, i, k, score);
            if score > self.comps[p].highest {
                self.comps[p].highest = score;
            }
        }
        let spot = self.spots[k];
        if spot.owner > 0 && i == self.local() {
            rt.dpo_params(score, Some(spot.score));
            rt.dpo_title(&format!("#{}", Self::name(spot.owner)));
            if self.teams_on {
                rt.dpo_team(self.team_of(spot.owner) as i64 - 1); // 779-781
            }
        }
        Ok(())
    }

    /// [run_challenge] triggerentercollision / triggerexitcollision (789-820): this skater in a spot.
    fn trigger(&mut self, rt: &mut Rt<St>, p: usize, k: i64, inside: bool) {
        if p != self.me || !(0..self.spots.len() as i64).contains(&k) {
            return;
        }
        if inside {
            if self.comps[p].in_spot != Some(k) {
                self.comps[p].in_spot = Some(k);
                let spot = self.spots[k as usize];
                if spot.owner > 0 {
                    rt.dpo_title(&format!("#{}", Self::name(spot.owner)));
                    if self.teams_on {
                        rt.dpo_team(self.team_of(spot.owner) as i64 - 1); // 797-799
                    }
                } else {
                    // 802: an unowned spot clears the DPO's team colour, teams or not (a colour left by the last spot).
                    rt.dpo_title("#");
                    rt.dpo_team(-1);
                }
                rt.dpo_params(0, Some(spot.score));
                rt.show(Component::DPOHUD, true);
            }
        } else {
            rt.show(Component::DPOHUD, false);
            self.comps[p].in_spot = None;
        }
    }

    /// [calculate_scores] stateenter (381-414): teams, each team's members (competitor order) by PointsScoreSorter.
    fn team_player_standings(&mut self) {
        self.team_players = [1, 2].map(|team| {
            let mut st: Vec<Standing> = (0..self.n)
                .filter(|&i| self.comps[i].team == team)
                .map(|i| Standing { player: i + 1, points: self.comps[i].points, score: self.comps[i].score })
                .collect();
            skate3_modes::lua_sort(&mut st, |a, b| a.points > b.points || (a.points == b.points && a.score > b.score));
            st
        });
    }

    /// [results] stateenter (884-957).
    fn results(&mut self, rt: &mut Rt<St>) {
        rt.hud.results.clear();
        if self.teams_on {
            // 893-894 as written: row 0 rank 1 with the first standing's Points, row 1 rank 2 the second's (the
            // leader's points on the red row whichever team leads; a tie's unsorted order: team 1's). ponytail: copied
            // as retail has it.
            for k in 0..2 {
                rt.result(k, k as i64 + 1, self.team_standings[k].points);
            }
            // 908-915: RoundResults' two lists (PlaceNameIntInt: Points, Score).
            for (i, s) in self.team_players[0].iter().enumerate() {
                rt.note(format!("result {} {} {}", s.player - 1, i + 1, s.points));
                rt.note(format!("resultf {} {:.3}", s.player - 1, s.score as f64));
            }
            for (i, s) in self.team_players[1].iter().enumerate() {
                rt.note(format!("result2 {} {} {} {}", s.player - 1, i + 1, s.points, s.score));
            }
        } else {
            for (i, s) in self.standings.iter().enumerate() {
                rt.result(s.player - 1, i as i64 + 1, s.points);
                rt.note(format!("resultf {} {:.3}", s.player - 1, s.score as f64));
            }
        }
        for (i, s) in self.standings.iter().enumerate() {
            let c = &self.comps[s.player - 1];
            rt.note(format!("report {} {} {} {} {} {ABANDONED}", s.player - 1, i + 1, s.points, s.score, c.finish));
        }
        if self.teams_on {
            // TeamResults[Team] = {Rank, Points} (945-952).
            let mut by_team = self.team_standings;
            by_team.sort_by_key(|t| t.team);
            for t in by_team {
                rt.note(format!("teamreport {} {} {}", t.team, t.rank, t.points));
            }
        }
        rt.change(St::CleanupChallengeCommon);
    }
}

impl Graph for Domination {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::SetupCodeCommon => "setup_code_common",
            St::SetupCodeSpecific => "setup_code_specific",
            St::WaitForAssetload => "wait_for_assetload",
            St::DynamicCharacterNis => "dynamic_character_nis",
            St::IntroPresentation => "intro_presentation",
            St::SetupChallengeCommon => "setup_challenge_common",
            St::SetupChallengeSpecific => "setup_challenge_specific",
            St::Countdown => "countdown",
            St::RunChallenge => "run_challenge",
            St::EndChallenge => "end_challenge",
            St::CalculateScores => "calculate_scores",
            St::Highlight => "highlight",
            St::OutroPresentation => "outro_presentation",
            St::Results => "results",
            St::CleanupChallengeCommon => "cleanup_challenge_common",
            St::CleanupChallengeSpecific => "cleanup_challenge_specific",
            St::Complete => "complete",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        // [global] playerdisconnected (104-123).
        if let Msg::Left(p) = msg
            && self.setup_complete
            && let Some(c) = self.comps.get_mut(*p).filter(|c| !c.disconnected)
        {
            c.disconnected = true;
            c.finish = DISCONNECT;
            if *p == self.me {
                rt.leave();
            }
        }
        match (rt.state, msg) {
            (St::Initial, Msg::Enter) => {
                self.setup_complete = false;
                rt.change(St::SetupCodeCommon);
            }
            // [setup_code_common] stateenter (154-290), the online path. ponytail: AIAutoBalanceTeams (180-219) is false
            // on every row: no AI skaters.
            (St::SetupCodeCommon, Msg::Enter) => {
                rt.pin(self.me, true);
                self.teams_on = !self.teams_in.is_empty();
                for (i, c) in self.comps.iter_mut().enumerate() {
                    // Online.GetTeam (0x828463d8: the player's +112, plus 1).
                    let team = if self.teams_in.get(i) == Some(&2) { 2 } else { 1 };
                    *c = Comp { disconnected: rt.gone.get(i).copied().unwrap_or(true), finish: NOT_FINISHED, team, start_locator: i, ..Comp::default() };
                }
                if self.teams_on {
                    // 222-233: team 1 from slot 0, team 2 after team 1's size.
                    let mut next = [0, self.comps.iter().filter(|c| c.team == 1).count()];
                    for c in &mut self.comps {
                        c.start_locator = next[c.team - 1];
                        next[c.team - 1] += 1;
                    }
                }
                self.setup_complete = true;
                self.standings.clear();
                // 237-250.
                self.team_standings = [1, 2].map(|team| TeamStanding { team, points: 0, score: 0, rank: 2 });
                self.team_players = Default::default();
                rt.change(St::SetupCodeSpecific);
            }
            // [setup_code_specific] stateenter (449-670).
            (St::SetupCodeSpecific, Msg::Enter) => {
                self.indicators_on = true; // 461-464
                for s in &mut self.spots {
                    *s = Spot { owner: -1, score: 0, at: (0., 0) };
                }
                for c in &mut self.comps {
                    (c.in_spot, c.highest) = (None, 0);
                }
                rt.post(Msg::HudLoaded);
                rt.dpo_format(DPO_POINTS);
            }
            // [setup_code_specific] hudloaded (673-688).
            (St::SetupCodeSpecific, Msg::HudLoaded) => {
                rt.dpo_format(DPO_INTS);
                rt.show(Component::DPOHUD, false);
                rt.show(Component::MiniMap, true);
                rt.show(Component::TrickDisplayHUD, true);
                if self.teams_on {
                    // 681-685: the team rows alone, this player's team highlighted.
                    rt.board_show_players(false);
                    rt.board_team_highlight(self.comps[self.me].team, true);
                    rt.board_init_teams(RANK_NAME_SCORE, self.n, self.comps.iter().map(|c| c.team as u8).collect(), self.team_names.clone());
                } else {
                    rt.board_init(RANK_NAME_SCORE, self.n);
                }
                self.update_board(rt);
                rt.show(Component::LeaderboardHUD, true);
                rt.change(St::WaitForAssetload);
            }
            (St::WaitForAssetload, Msg::Enter) => self.wait_pres = Some(rt.add_timer(WAIT_PRES)),
            (St::WaitForAssetload, Msg::Timer(id)) if self.wait_pres == Some(*id) => rt.change(St::DynamicCharacterNis),
            // 416-446: teams with an IntroNIS, the team intro with both teams' skaters, intro_presentation at its notify.
            // ponytail: the NIS player with dynamic characters is not ported (no scene plays).
            (St::DynamicCharacterNis, Msg::Enter) if self.teams_on && self.row.intro_nis.is_some() => rt.nis(0.),
            (St::DynamicCharacterNis, Msg::Enter | Msg::Nis) => rt.change(St::IntroPresentation),
            (St::IntroPresentation, Msg::Enter) => {
                self.presenting = Some(Group::Intro);
                self.scene = self.row.intro.clone();
                rt.present(self.intro_len);
            }
            (St::IntroPresentation, Msg::Presented) if self.presenting == Some(Group::Intro) => rt.change(St::SetupChallengeCommon),
            // [setup_challenge_common] stateenter (311-343).
            (St::SetupChallengeCommon, Msg::Enter) => {
                for c in &mut self.comps {
                    c.attempted = false;
                }
                self.calculate();
                for p in 0..self.n {
                    rt.reposition(p, Place { field: LOCATION, index: 0, slot: self.comps[p].start_locator }, 1.);
                    rt.pin(p, true);
                }
                self.load_delay = Some(rt.add_timer(LOAD_DELAY));
            }
            (St::SetupChallengeCommon, Msg::Timer(id)) if self.load_delay == Some(*id) => rt.change(St::SetupChallengeSpecific),
            // [setup_challenge_specific] stateenter (823-882).
            (St::SetupChallengeSpecific, Msg::Enter) => {
                self.somebody_scored = false;
                (self.last_leader, self.last_team_leader) = (-1, -1);
                rt.close_loading();
                if self.teams_on {
                    // 867-871: this player's team, in its colour (team - 1: 0 red, 1 blue), then the countdown.
                    let team = self.comps[self.me].team;
                    self.team_intro = Some(rt.add_timer((self.row.team_intro_delay - self.row.countdown_delay).max(0.)));
                    let id = self.row.team_indication.clone();
                    rt.team_central(&id, team as i64 - 1, TEAM_NORMAL, &[self.team_names[team - 1].clone()]);
                } else {
                    rt.change(St::Countdown);
                }
            }
            (St::SetupChallengeSpecific, Msg::Timer(id)) if self.team_intro == Some(*id) => rt.change(St::Countdown),
            (St::Countdown, Msg::Enter) => self.countdown_delay = Some(rt.add_timer(self.row.countdown_delay)),
            (St::Countdown, Msg::Timer(id)) if self.countdown_delay == Some(*id) => {
                let t = rt.add_timer(COUNTDOWN);
                self.countdown_timer = Some(t);
                rt.countdown(t);
            }
            (St::Countdown, Msg::Timer(id)) if self.countdown_timer == Some(*id) => rt.change(St::RunChallenge),
            // [run_challenge] stateenter (717-736). ponytail: GameMode.DominationSpeedRounds (10 s) is a debug switch.
            (St::RunChallenge, Msg::Enter) => {
                self.started = Some(rt.now);
                for p in 0..self.n {
                    rt.pin(p, false);
                }
                let t = rt.add_timer(self.row.time_limit);
                self.challenge_timer = Some(t);
                rt.timer(t);
            }
            (St::RunChallenge, Msg::Timer(id)) if self.challenge_timer == Some(*id) => {
                for c in &mut self.comps {
                    if c.attempted && !c.disconnected {
                        c.finish = NORMAL;
                    }
                }
                rt.change(St::EndChallenge);
            }
            (St::RunChallenge, Msg::Objective { player, score }) => self.objective(rt, *player, *score)?,
            (St::RunChallenge, Msg::Trigger { player, volume, inside }) => self.trigger(rt, *player, *volume, *inside),
            // [end_challenge] stateenter (75-96).
            (St::EndChallenge, Msg::Enter) => {
                rt.pause(true);
                for p in 0..self.n {
                    rt.pin(p, true);
                }
                rt.show(Component::TimerHUD, false);
                rt.change(St::CalculateScores);
            }
            (St::CalculateScores, Msg::Enter) => {
                self.calculate();
                if self.teams_on {
                    self.team_player_standings();
                }
                rt.change(St::Highlight);
            }
            (St::Highlight, Msg::Enter) => rt.reel(),
            (St::Highlight, Msg::Reel) => rt.change(St::OutroPresentation),
            // OutroPresentationEvents is empty on every row.
            (St::OutroPresentation, Msg::Enter) => {
                self.presenting = Some(Group::Outro);
                rt.present(0.);
            }
            (St::OutroPresentation, Msg::Presented) if self.presenting == Some(Group::Outro) => rt.change(St::Results),
            (St::Results, Msg::Enter) => self.results(rt),
            (St::CleanupChallengeCommon, Msg::Enter) => {
                rt.pin(self.me, false);
                rt.change(St::CleanupChallengeSpecific);
            }
            (St::CleanupChallengeSpecific, Msg::Enter) => {
                self.indicators_on = false; // 693-697
                rt.change(St::Complete);
            }
            (St::Complete, Msg::Enter) => rt.leave(),
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Bevy side
/// A spot this console's skater completed (objectivecomplete), relayed with its sequence score and when it landed (the
/// challenge clock); [`skate3_modes::EndSync`]'s marker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Relay {
    Obj { index: usize, score: i64, at: f64 },
    End(skate3_modes::EndMark),
}

/// What the probe saw: a spot volume edge (index, inside).
#[derive(Clone, Debug)]
enum Seen {
    Trigger(i64, bool),
}

#[derive(Default)]
struct Probe {
    inside: Vec<bool>,
    seen: Vec<Seen>,
    /// The skater per physics tick, for its objective group.
    frames: Vec<Frame>,
}

struct Play {
    id: String,
    row: Row,
    g: Domination,
    rt: Rt<St>,
    names: Vec<String>,
    me: usize,
    online: bool,
    hud: HudDriver,
    placed: u32,
    playing: Option<String>,
    /// This skater's objective group (pointer 100 + id) and its natives.
    group: Vec<(u32, Objectives)>,
    sense: Sense,
    sync: EndSync,
    /// Other consoles' spots waiting for this one's run_challenge (it loaded later).
    inbox: Vec<(usize, Relay)>,
}

impl Play {
    /// This console's skater plays (its spots count).
    fn live(&self) -> bool {
        self.rt.state == St::RunChallenge && !self.sync.holding() && self.g.comps.get(self.me).is_some_and(|c| !c.disconnected)
    }
    /// The clock (online, its end held for the other consoles): the End marker to relay.
    fn tick(&mut self, now: f64) -> Option<String> {
        let phase = skate3_modes::advance_synced(&mut self.g, &mut self.rt, now, St::RunChallenge, self.me, &mut self.sync)?;
        self.online.then(|| serde_json::to_string(&Relay::End(phase)).expect("plain data"))
    }
    /// This skater completed spot `index`: delivered here, and what to relay.
    fn complete(&mut self, index: usize, score: i64) -> Option<String> {
        let at = self.g.started.map_or(0., |s| self.rt.now - s);
        let me = self.me;
        deliver(self, me, index, score, at);
        self.online.then(|| serde_json::to_string(&Relay::Obj { index, score, at }).expect("plain data"))
    }
    /// The other consoles' relays: their spots in run_challenge (end review M6: one from before it waits for it, as
    /// the contest's; after it, dropped).
    fn receive(&mut self, received: Vec<(usize, Relay)>) {
        for (from, r) in received {
            match r {
                Relay::End(phase) => self.sync.ended(from, phase),
                Relay::Obj { .. } if from != self.me => self.inbox.push((from, r)),
                Relay::Obj { .. } => {}
            }
        }
        match self.rt.state {
            St::RunChallenge => {
                for (from, r) in std::mem::take(&mut self.inbox) {
                    if let Relay::Obj { index, score, at } = r {
                        deliver(self, from, index, score, at);
                    }
                }
            }
            St::EndChallenge
            | St::CalculateScores
            | St::Highlight
            | St::OutroPresentation
            | St::Results
            | St::CleanupChallengeCommon
            | St::CleanupChallengeSpecific
            | St::Complete => self.inbox.clear(),
            _ => {}
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct DominationGame {
    data: Option<ModeData>,
    /// rules.json's points objective and trick groups.
    points: Option<ObjectiveDef>,
    tricks: HashMap<String, Vec<String>>,
    play: Option<Play>,
    probe: Probe,
    volumes: Vec<RaceVolume>,
    /// The volumes the objectives' snippets name (instance id, box).
    triggers: Vec<(u64, Volume)>,
    live: bool,
    warned: String,
    /// The install has the spots' looks ([`INDICATORS`]; an export from before G3 has none: nothing is drawn).
    indicators: bool,
}

/// The spots' looks (tools/skate3_online.py look_file), drawn by skate2_arrows (skate3_indicators::show).
const INDICATORS: &str = "private/skate3-online/domination/indicators.json";

/// The rows' [`Playable`] entries: each row, and its Team tab row (the TeamsEnabled path).
fn entries(ids: &[String]) -> Vec<String> {
    ids.iter().cloned().chain(ids.iter().map(|id| crate::match_core::team(id))).collect()
}

pub(crate) struct Skate3DominationPlugin;
impl Plugin for Skate3DominationPlugin {
    fn build(&self, app: &mut App) {
        let data = ModeData::install(app, "domination", "Domination");
        let rules = data.as_ref().and_then(|d| skate3_film::rules(d).inspect_err(|e| warn!("Skate 3 Domination: {e}")).ok());
        let (points, tricks) = rules.map_or((None, HashMap::new()), |r| (Some(r.points_objective), r.trick_groups));
        if let (Some(data), Some(points)) = (&data, &points) {
            let ids: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).is_ok_and(|r| r.playable(id, points))).cloned().collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                playable.0.extend(entries(&ids));
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        let indicators = data.as_ref().is_some_and(|d| d.root.join("indicators.json").is_file());
        app.insert_resource(DominationGame { data, points, tricks, indicators, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

/// Every physics tick while the challenge runs: this skater's spot volumes (the DPO's trigger enter / exit) and its
/// [`Frame`] for the objective group.
fn probe(
    mut game: ResMut<DominationGame>,
    skater: Res<SkaterRuntime>,
    marker: Option<Res<crate::session_marker::SessionMarker>>,
) {
    let DominationGame { probe, volumes, triggers, live, .. } = &mut *game;
    if !*live {
        *probe = Probe::default();
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let pos = Vec3::new(at[0], at[1], at[2]);
    probe.inside.resize(volumes.len(), false);
    for (i, (was, v)) in probe.inside.iter_mut().zip(volumes.iter()).enumerate() {
        let inside = v.contains(pos);
        if inside != *was {
            *was = inside;
            probe.seen.push(Seen::Trigger(i as i64, inside));
        }
    }
    probe.frames.push(skate3_film::frame_of(&skater, marker.as_deref(), triggers, None));
}

fn run(world: &mut World) {
    world.resource_scope(|world, mut game: Mut<DominationGame>| {
        let Some(data) = game.data.take() else { return };
        step(world, &mut game, &data);
        game.data = Some(data);
    });
}

fn deliver(play: &mut Play, from: usize, index: usize, score: i64, at: f64) {
    play.g.index = index;
    play.g.stamp = (at, from);
    skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Objective { player: from, score });
}

fn step(world: &mut World, game: &mut DominationGame, data: &ModeData) {
    let now = f64::from(crate::match_core::match_now(world));
    let (mode, phase, id, owned) = {
        let m = world.resource::<Match>();
        (m.mode, m.phase.clone(), m.row.clone(), m.owned)
    };
    let row = (owned && mode == Some(Mode::Domination) && phase != Phase::Idle).then(|| data.row::<Row>(&id).ok()).flatten();
    let map = world.get_resource::<CurrentMap>().and_then(|m| Some(m.path.as_deref()?.file_stem()?.to_string_lossy().into_owned()));
    let on_map = row.is_some() && map.is_some() && map.as_deref() == skate3_modes::map_of(&id);
    if !on_map {
        if game.play.is_some() {
            end(world, game);
        }
        if row.is_some() && game.warned != id {
            warn!("Skate 3 Domination: {id} is on {:?}, not this map ({map:?})", skate3_modes::map_of(&id));
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
        game.play = Some(begin(world, data, &id, row, now, game.tricks.clone()));
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

    // This console's skater: its spots and objectives; the objectives relayed.
    game.live = play.live();
    game.volumes = play.row.objectives.iter().filter_map(|o| o.boundary.clone()).collect();
    game.triggers = play.row.trigger_volumes.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()?))).collect();
    let me = play.me;
    for Seen::Trigger(k, inside) in std::mem::take(&mut game.probe.seen) {
        if game.live {
            skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Trigger { player: me, volume: k, inside });
        }
    }
    // MakeObjectives(id) (453-457), evaluated while run_challenge runs (733); each completion is objectivecomplete
    // with Trick.GetCompletedSequenceScore (the banked sequence's), then ResetObjective (784).
    if game.live && play.group.is_empty() {
        match (play.row.group(), &game.points) {
            (Some(def), Some(points)) => match Objectives::new(me, &def, points) {
                Ok(g) => play.group.push((100 + me as u32, g)),
                Err(e) => warn!("Skate 3 Domination: {}: {e}", play.id),
            },
            _ => warn!("Skate 3 Domination: {}: no objective data", play.id),
        }
    }
    for f in std::mem::take(&mut game.probe.frames) {
        if !game.live {
            continue;
        }
        for (_, _, o) in skate3_film::feed(&mut play.group, &mut play.sense, f) {
            let Out::Complete { index, points: false, .. } = o else { continue };
            let score = play.sense.finished_score();
            if let Some(text) = play.complete(index, score) {
                world.resource_mut::<Match>().outbox.push(text);
            }
            play.group.iter_mut().for_each(|g| g.1.reset_one(index));
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

    // RepositionAI(id, Location, id): this skater to its start slot.
    if let Some((seq, place)) = play.rt.landed(me)
        && play.placed != seq
    {
        play.placed = seq;
        if let Some(at) = play.row.location.as_ref().map(|l| l.slot(place.slot)) {
            world.resource_mut::<Match>().teleport = Some(at);
        }
    }

    // The intro NIS.
    let want = play.rt.presenting.filter(|&end| now < end).and(play.g.scene.clone()).filter(|_| play.rt.state == St::IntroPresentation);
    if want != play.playing {
        if want.is_some() {
            world.resource_mut::<Cutscene>().start(&data.root, want.as_deref().expect("some"));
        } else {
            world.resource_mut::<Cutscene>().stop();
        }
        world.resource_mut::<Match>().intro = want.is_some();
        play.playing = want;
    }
    // The loading screen: up until setup_challenge_specific closes it (865), the intro NIS showing through it (R1).
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = play.rt.covered && play.playing.is_none();
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        let alpha = if play.playing.is_some() { 0. } else { play.rt.fade_alpha(now) as f32 };
        if fade.0 != alpha {
            fade.0 = alpha;
        }
    }
    // The spots' indicators (461-464 .. 693-697), recoloured as they change hands.
    let markers = if game.indicators { play.g.markers() } else { Vec::new() };
    crate::skate3_indicators::show(world, INDICATORS, markers);
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = play.rt.pinned.get(me).copied().unwrap_or(true) || play.rt.presenting_now();
        m.gestures = skate3_modes::gestures(&m, play.rt.left);
        m.watch = None;
        m.score = play.g.comps.get(me).map_or(0, |c| c.points);
        m.finished = play.rt.left;
        (m.pause_disabled, m.trick_display_hidden) = (play.rt.pause_disabled, play.rt.hud.trick_hidden);
    }
    // Teams: the results rows are the two teams (0 / 1), this player's team the highlighted one.
    let teams = play.g.teams_on && play.rt.left;
    let team_view: Vec<String> = play.g.team_names.iter().cloned().chain(play.names.iter().skip(2).cloned()).collect();
    let my_team = play.g.comps.get(me).map_or(0, |c| c.team.saturating_sub(1));
    let view = if teams {
        View { names: &team_view, me: my_team, online: play.online, title: data.text(&play.row.title), label: "Domination" }
    } else {
        View { names: &play.names, me, online: play.online, title: data.text(&play.row.title), label: "Domination" }
    };
    // The DPO's title is "#" .. the owner's name (777, 795).
    let mut hud = play.rt.hud.clone();
    if let Some(p) = hud.dpo.title.strip_prefix("#P").and_then(|n| n.parse::<usize>().ok()) {
        hud.dpo.title = format!("#{}", play.names.get(p).cloned().unwrap_or_default());
    }
    // The results once the challenge is over: Rank, Skater, Score, Points (922-926) over item 1 = Points and item 2 =
    // Score (928); teams, the TeamChallengeResults table (889). ponytail: the front end's results order is not traced.
    let results = play.rt.left.then(|| ResultsSetup {
        title: data.text(&play.row.title),
        layout: if teams { TEAM_RESULTS } else { PLACE_NAME_INT_INT },
        columns: ["ID_LEADERBOARD_HEADER_RANK", "ID_LEADERBOARD_HEADER_SKATER", "ID_COMMON_SCORE", "ID_LEADERBOARD_POINTS_HEADER"]
            .map(|c| data.text(c))
            .to_vec(),
        throwdown: 0,
        choice: None,
        times: Vec::new(),
        floats: if teams { Vec::new() } else { play.g.standings.iter().map(|s| (s.player - 1, s.score as f64, s.score.to_string())).collect() },
        strings: Vec::new(),
    });
    play.hud.teams = teams.then(|| team_results(data, &play.g, &play.names, &view.title, play.online));
    play.hud.drive(world, data, &hud, &view, results.as_ref(), now);
}

/// endchallenge_teams' data block (skate3_modes::team_block) for team Domination, as results (886-915) leaves it:
/// the team rows SetPlayerData(k, k + 1, TeamStandings[k + 1].Points) as written (ponytail: row 0 is the red team's
/// with the leader's rank and points, copied from retail); the RoundResults group PlaceNameIntInt with each team's
/// members by PointsScoreSorter, [rank, name, points, score]. Results_SetIsFinalResults is never called: final.
fn team_results(data: &ModeData, g: &Domination, names: &[String], title: &str, online: bool) -> serde_json::Value {
    let table = |k: usize| skate3_modes::TeamTable {
        name: g.team_names[k].clone(),
        rank: k as i64 + 1,
        score: skate3_modes::team_int(data, g.team_standings[k].points),
        status: skate3_modes::team_int(data, 0),
        members: (g.team_players[k].iter().enumerate())
            .map(|(i, s)| {
                let name = format!("#{}", names.get(s.player - 1).map_or("", String::as_str));
                [skate3_modes::team_int(data, i as i64 + 1), name, skate3_modes::team_int(data, s.points), skate3_modes::team_int(data, s.score)]
            })
            .collect(),
    };
    let team = g.comps.get(g.me).map_or(0, |c| c.team.saturating_sub(1)).min(1);
    let row = g.team_players[team].iter().position(|s| s.player == g.me + 1).map_or(-1, |i| i as i64);
    skate3_modes::team_block(online, title, true, (team, row), PLACE_NAME_INT_INT, [table(0), table(1)])
}

fn begin(world: &mut World, data: &ModeData, id: &str, row: Row, now: f64, tricks: HashMap<String, Vec<String>>) -> Play {
    let m = world.resource::<Match>();
    let online = m.token != 0;
    let (mut names, me) = if online { (m.players.clone(), m.slot) } else { (vec!["Player 1".to_owned()], 0) };
    if names.is_empty() {
        names.push("Player 1".into());
    }
    let intro = row.intro.as_ref().and_then(|s| data.intros.get(s)).map_or(0., |&l| f64::from(l));
    info!("Skate 3 Domination: {id} ({}), {} skaters, this one {me}", if online { "online" } else { "solo" }, names.len());
    let mut g = Domination::new(row.clone(), intro, names.len(), me);
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
    Play { id: id.to_owned(), row, g, rt, names, me, online, hud: HudDriver::default(), placed: 0, playing: None, group: Vec::new(), sense: Sense::new(tricks), sync: EndSync::default(), inbox: Vec::new() }
}

fn end(world: &mut World, game: &mut DominationGame) {
    if let Some(play) = game.play.take() {
        info!("Skate 3 Domination: {} ended", play.id);
        if play.playing.is_some() {
            world.resource_mut::<Cutscene>().stop();
            world.resource_mut::<Match>().intro = false;
        }
    }
    game.live = false;
    if let Some(mut hold) = world.get_resource_mut::<LoadingHold>() {
        hold.0 = false;
    }
    if let Some(mut fade) = world.get_resource_mut::<ChallengeFade>() {
        fade.0 = 0.;
    }
    crate::skate3_indicators::show(world, INDICATORS, Vec::new());
    HudDriver::close(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};

    fn row(spots: usize, time: f64) -> Row {
        Row {
            title: "ID_T".into(),
            online: true,
            intro: Some("dom_x".into()),
            time_limit: time,
            countdown_delay: 1.5,
            owned: "ID_OWNED".into(),
            lost: "ID_LOST".into(),
            increased: "ID_INCREASED".into(),
            new_lead: "ID_NEWLEAD".into(),
            new_lead_you: "ID_NEWLEADYOU".into(),
            team_indication: "ID_TEAM".into(),
            team_intro_delay: 3.,
            intro_nis: Some(serde_json::json!("dom_intro")),
            ribbon_neutral: String::new(),
            ribbon_owned: String::new(),
            colour_neutral: String::new(),
            colours_players: Vec::new(),
            colours_teams: Vec::new(),
            location: Some(Locator { matrix: [[0.; 4]; 4], slots: vec![[[0.; 4]; 4]; 6] }),
            objectives: (0..spots)
                .map(|k| Objective { boundary: Some(RaceVolume { name: format!("o{}", k + 1), min: [0.; 3], max: [1.; 3] }), indicators: Vec::new() })
                .collect(),
            groups: Groups::default(),
            trigger_volumes: BTreeMap::new(),
        }
    }

    /// One run: every skater lands sequences on random spots (some lower than the spot's score, some equal), the
    /// local skater rolls in and out of spots; rarely a skater leaves (its events stop there).
    fn plans(seed: u64, spots: usize, n: usize, me: usize, time: f64, leaves: bool) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |k: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % k
        };
        let mut run = Vec::new();
        let leave = (leaves && n > 1).then(|| {
            let mut p = next(n as u64) as usize;
            if p == me {
                p = (p + 1) % n;
            }
            (p, 1. + next((time * 8.) as u64) as f64 / 10.)
        });
        for p in 0..n {
            let mut t = 0.03 * (p + 1) as f64;
            while t < time - 1. {
                t += 0.5 + next(60) as f64 / 10.;
                if leave.is_some_and(|(q, at)| q == p && t >= at) {
                    break;
                }
                let k = next(spots as u64) as i64;
                if p == me && next(3) == 0 {
                    run.push((t, Ev::Other("enter", k, p as f64)));
                    run.push((t + 0.4, Ev::Other("exit", k, p as f64)));
                }
                let score = [0, 100, 250, 250, 500, 1000, 1500][next(7) as usize] + 10 * next(30) as i64;
                run.push((t + 0.2, Ev::Other("obj", score, (p * 16) as f64 + k as f64)));
            }
        }
        if let Some((p, at)) = leave {
            run.push((at, Ev::Leave(p)));
        }
        vec![run]
    }

    const SETUP: &str = r#"
local objs = {Count = K}
for k = 1, K do objs[k] = {ScoringBoundary = "o" .. k, VisualIndicators = {Count = 0}} end
SELF = setmetatable({Name = "domination", Title = "ID_T", Location = "Location0", TimeLimit = TIME, CountdownDelay = 1.5,
  Objectives = objs, VisualIndicators = {Count = 0}, IntroPresentationEvents = {Count = 1},
  OutroPresentationEvents = {Count = 0}, RestartPresentationEvents = {Count = 0}, RibbonColoursPlayers = {},
  RibbonColoursTeams = {}, CentralMessageHALIDDominationOwned = "ID_OWNED", CentralMessageHALIDDominationLostSpot = "ID_LOST",
  CentralMessageHALIDDominationScoreIncreased = "ID_INCREASED", CentralMessageHALIDNewLead = "ID_NEWLEAD",
  CentralMessageHALIDNewLeadYou = "ID_NEWLEADYOU", CentralMessageHALIDTeamIndication = "ID_TEAM",
  TeamIntroductionDelay = 3, IntroNIS = "dom_intro", DisableHOM = true}, {__index = function() return nil end})
HUD2.eMessageAudioHook = {Default = 0, Fail = 1, Success = 2, Owned = 3}
HUD2.eLeaderboardHUDMenuType.RankNameScore = 3
FrontEnd.eResultsLBType.PlaceNameIntInt = 14
FrontEnd.Results_SetPlayerData = function(id, rank, v, f)
  log_line(string.format("result %d %d %d", id, rank, v))
  if f then log_line(string.format("resultf %d %.3f", id, f)) end
end
FrontEnd.Results_SetSecondLBPlayerData = function(id, _, rank, v, f) log_line(string.format("result2 %d %d %d %d", id, rank, v, f)) end
HUD2.LeaderboardHUDSetShowPlayers = function(_, on) log_line("showplayers " .. tostring(on)) end
Online.OnlineChallengeComplete = function(r, t)
  for _, x in ipairs(r) do log_line(string.format("report %d %d %d %d %d %d", x[1], x[2], x[3], x[4], x[7], x[8])) end
  for i, x in ipairs(t or {}) do log_line(string.format("teamreport %d %d %d", i, x[1], x[2])) end
end
AI.LoadPaths = function() return 0 end
local groups = 0
ChallengePresentation.AddPresentationGroup = function() groups = groups + 1; return groups end
function ACTOR() return nil end
function DELIVER(kind, v, w)
  if kind == "obj" then
    local p, k = math.floor(w / 16), math.floor(w % 16)
    SCORE = v
    SEND("objectivecomplete", {ObjectiveGroup = 100 + p, ObjectiveIndex = k})
    return true
  end
  local p = math.tointeger(w)
  if kind == "enter" then SEND("triggerentercollision", {PlayerID = p, VolumeID = "o" .. (v + 1)}); return true end
  if kind == "exit" then SEND("triggerexitcollision", {PlayerID = p, VolumeID = "o" .. (v + 1)}); return true end
  return false
end
"#;

    fn oracle(src: &str, n: usize, me: usize, r: &Row, plans: &[Vec<(f64, Ev)>]) -> (Vec<String>, Vec<String>, Vec<String>) {
        team_oracle(src, n, me, r, plans, &[])
    }

    /// The oracle with Online.IsTeamChallenge true and Online.GetTeam answering `teams` (empty: no teams).
    fn team_oracle(src: &str, n: usize, me: usize, r: &Row, plans: &[Vec<(f64, Ev)>], teams: &[u8]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut setup = format!("N, ME, FIRST = {n}, {me}, 0\nK, TIME = {}, {}\n{SETUP}", r.objectives.len(), r.time_limit);
        if !teams.is_empty() {
            let t: Vec<String> = teams.iter().enumerate().map(|(i, t)| format!("[{i}] = {t}")).collect();
            setup.push_str(&format!("TEAMS = {{{}}}\n", t.join(", ")));
        }
        if r.intro_nis.is_none() {
            setup.push_str("SELF.IntroNIS = nil\n");
        }
        let (lua, errors) = lua_log(src, &setup, "initial", "run_challenge", plans);
        let mut g = Domination::new(r.clone(), 0., n, me);
        g.teams_in = teams.to_vec();
        let mut rt = Rt::new(St::Initial, n, 0.);
        let other = |g: &mut Domination, _: &mut Rt<St>, kind: &str, v: i64, w: f64| match kind {
            "obj" => {
                g.index = (w as usize) % 16;
                Some(Msg::Objective { player: w as usize / 16, score: v })
            }
            "enter" | "exit" => Some(Msg::Trigger { player: w as usize, volume: v, inside: kind == "enter" }),
            _ => None,
        };
        let model = model_log(&mut g, &mut rt, St::RunChallenge, plans, |_| None, |rt| rt.state == St::Complete, other);
        (lua, model, errors)
    }

    /// domination.lua itself (S3_DOMINATION_DIR = tools/skate3_online.py's `--out`/domination, with scripts/) decides
    /// every game the model runs the same way: states, timers, teleports, pins, HUD2 calls, owners, points, standings,
    /// central messages (owned, lost, out-scored, dominating, new leader), the DPO, results and report; 1 to 6 skaters,
    /// 1 to 9 spots, skaters leaving.
    #[test]
    #[ignore = "requires the decompiled domination.lua (S3_DOMINATION_DIR)"]
    fn skate3_domination_matches_domination_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_DOMINATION_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/domination.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=50u64 {
            for n in 1..=6usize {
                let s = seed as usize;
                let me = s % n;
                let spots = 1 + (s * 5 + n) % 9;
                let time = [60., 45., 90.][(s + n) % 3];
                let r = row(spots, time);
                let plans = plans(seed * 977 + n as u64, spots, n, me, time, seed % 3 == 0);
                let (lua, model, errs) = oracle(&src, n, me, &r, &plans);
                compare(&format!("seed {seed} n {n} me {me} spots {spots}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("report")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("domination.lua oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    /// domination.lua's TeamsEnabled path (Online.IsTeamChallenge, Online.GetTeam) decides every team game the model
    /// runs the same way: the team start slots, the team board, the team intro NIS, the team message and its delay,
    /// spots lost to the other team only, team dominating and lead messages, the DPO's team colour, team points and
    /// ranks (an exact tie both 2), the team results and report; 2 to 6 skaters in even, uneven and one-sided splits,
    /// skaters leaving.
    #[test]
    #[ignore = "requires the decompiled domination.lua (S3_DOMINATION_DIR)"]
    fn skate3_domination_teams_match_domination_lua() {
        let root = std::path::PathBuf::from(std::env::var("S3_DOMINATION_DIR").unwrap());
        let src = std::fs::read_to_string(root.join("scripts/domination.lua")).unwrap();
        let (mut games, mut lines, mut errors) = (0, 0, 0);
        for seed in 1..=60u64 {
            for n in 2..=6usize {
                let s = seed as usize;
                let me = (s + 1) % n;
                let spots = 1 + (s * 5 + n) % 9;
                let time = [60., 45., 90.][(s + n) % 3];
                let mut r = row(spots, time);
                if seed % 7 == 0 {
                    r.intro_nis = None;
                }
                let teams: Vec<u8> = (0..n)
                    .map(|i| match seed % 3 {
                        0 => 1 + (i % 2) as u8,
                        1 => if i < (n + 1) / 2 { 1 } else { 2 },
                        _ => if seed % 9 == 2 { 2 } else { 1 + ((i + s) % 2) as u8 },
                    })
                    .collect();
                let plans = plans(seed * 787 + n as u64, spots, n, me, time, seed % 3 == 1);
                let (lua, model, errs) = team_oracle(&src, n, me, &r, &plans, &teams);
                compare(&format!("seed {seed} n {n} me {me} spots {spots} teams {teams:?}"), &lua, &model, &errs);
                games += 1;
                lines += lua.len();
                errors += errs.len();
                assert!(lua.iter().any(|l| l.contains("teamreport")), "{:?}", &lua[lua.len().saturating_sub(5)..]);
            }
        }
        eprintln!("domination.lua team oracle: {games} games, {lines} log lines identical ({errors} Lua errors replayed)");
        assert_eq!(games, 300);
    }

    fn until(g: &mut Domination, rt: &mut Rt<St>, state: St) {
        for _ in 0..1000 {
            if rt.state == state {
                return;
            }
            let t = rt.due().unwrap_or_else(|| panic!("stuck in {:?}", rt.state));
            skate3_modes::advance(g, rt, t);
        }
        panic!("never reached {state:?}");
    }

    /// A team game by hand (teams 1 2 2 1, this player 2 on team 2): the team slots, board and message, team points
    /// and scores, an exact tie ranks both 2 unsorted, a teammate's better score is no lost spot, every spot the
    /// team's is dominating; the results rows as retail writes them and the team report.
    #[test]
    fn team_spots_decide_domination() {
        let mut g = Domination::new(row(3, 60.), 0., 4, 2);
        g.teams_in = vec![1, 2, 2, 1];
        let mut rt = Rt::new(St::Initial, 4, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunChallenge);
        let slots: Vec<usize> = (0..4).map(|p| rt.places[p].unwrap().1.slot).collect();
        assert_eq!(slots, [0, 2, 3, 1], "team 1 from slot 0, team 2 after it");
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("teamcentral ID_TEAM 1 0 T2")), "{log:?}");
        assert!(log.iter().any(|l| l.ends_with("board 3 4 teams")) && rt.hud.board.hide_players && rt.hud.board.team_highlight == [false, true]);
        let obj = |g: &mut Domination, rt: &mut Rt<St>, p: usize, k: usize, score: i64| {
            g.index = k;
            skate3_modes::dispatch(g, rt, Msg::Objective { player: p, score });
        };
        let ranks = |g: &Domination| g.team_standings.map(|t| (t.team, t.points, t.score, t.rank));
        obj(&mut g, &mut rt, 0, 0, 500);
        assert_eq!(ranks(&g), [(1, 1, 500, 1), (2, 0, 0, 2)]);
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with("teamcentral ID_NEWLEAD 0 2 T1")));
        obj(&mut g, &mut rt, 1, 1, 500);
        assert_eq!(ranks(&g), [(1, 1, 500, 2), (2, 1, 500, 2)], "an exact tie: both 2");
        let lost = |rt: &Rt<St>| rt.log.as_ref().unwrap().iter().filter(|l| l.contains("central ID_LOST")).count();
        obj(&mut g, &mut rt, 3, 0, 600);
        assert_eq!(lost(&rt), 0, "a teammate's spot is not lost");
        assert_eq!(ranks(&g), [(1, 1, 600, 1), (2, 1, 500, 2)]);
        obj(&mut g, &mut rt, 2, 2, 100);
        assert_eq!(ranks(&g), [(2, 2, 600, 1), (1, 1, 600, 2)]);
        assert_eq!(rt.hud.board.team_scores, [1, 2]);
        assert_eq!(rt.hud.board.team_ranks, [2, 1]);
        assert_eq!(rt.hud.dpo.team, Some(1), "this player's spot: the DPO in blue");
        obj(&mut g, &mut rt, 1, 0, 700);
        assert!(rt.log.as_ref().unwrap().iter().any(|l| l.ends_with(&format!("teamcentral {DOMINATING} 1 2 T2"))));
        let t = rt.now + 70.;
        skate3_modes::advance(&mut g, &mut rt, t);
        until(&mut g, &mut rt, St::Complete);
        assert_eq!(rt.hud.results, vec![(0, 1, 3), (1, 2, 0)], "row 0 rank 1 with the leader's points, as written");
        assert_eq!(g.team_players.each_ref().map(|t| t.iter().map(|s| s.player).collect::<Vec<_>>()), [vec![1, 4], vec![2, 3]]);
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with("teamreport 1 2 0")) && log.iter().any(|l| l.ends_with("teamreport 2 1 3")), "{log:?}");
    }

    /// By hand: a spot is taken by a higher score only; Points = spots owned, Score = their scores; the standings
    /// rank by Points then Score; owning every spot is dominating.
    #[test]
    fn spots_change_hands_on_higher_scores() {
        let r = row(2, 60.);
        let mut g = Domination::new(r, 0., 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        rt.log = Some(Vec::new());
        skate3_modes::start(&mut g, &mut rt);
        for _ in 0..100 {
            if rt.state == St::RunChallenge {
                break;
            }
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        assert_eq!(rt.state, St::RunChallenge);
        assert_eq!(rt.places[1].map(|p| p.1), Some(Place { field: LOCATION, index: 0, slot: 1 }), "slot = player id");
        let obj = |g: &mut Domination, rt: &mut Rt<St>, p: usize, k: usize, score: i64| {
            g.index = k;
            skate3_modes::dispatch(g, rt, Msg::Objective { player: p, score });
        };
        obj(&mut g, &mut rt, 1, 0, 500);
        obj(&mut g, &mut rt, 0, 0, 500);
        assert_eq!(g.spots[0].owner, 2, "an equal score does not take the spot");
        obj(&mut g, &mut rt, 0, 0, 600);
        obj(&mut g, &mut rt, 0, 1, 100);
        assert_eq!((g.comps[0].points, g.comps[0].score, g.comps[1].points), (2, 700, 0));
        let log = rt.log.as_ref().unwrap();
        assert!(log.iter().any(|l| l.ends_with(&format!("central {DOMINATING} 2 P0 0"))), "{log:?}");
        assert!(log.iter().any(|l| l.ends_with("central ID_OWNED 2  3")), "owned with its audio hook");
        assert!(log.iter().any(|l| l.ends_with("dpo 600 600")));
        assert_eq!(g.standings[0].player, 1);
    }

    /// G3 (online audit 10-28): the spots' indicators show from setup in the neutral look (461-464), a taken spot's in
    /// RibbonIndicatorOwned and its owner's colour, in a team match its team's (536-541), until cleanup removes them
    /// (693-697); an unplaced locator draws nothing. Before, no spot had a marker.
    #[test]
    fn the_spots_show_their_owners_colour() {
        // dom_dwtn_01's refspecs (class, key, 0): the neutral and owned types; grey, then red and blue (players 1 / 2,
        // teams 1 / 2).
        const NEUTRAL: &str = "3297556BECB605E2B094183C726DEB4E0000000000000000";
        const OWNED: &str = "3297556BECB605E2769C2AEE9C86C11A0000000000000000";
        const GREY: &str = "F0DB8767D611AF941DA37485495114CE0000000000000000";
        const P1: &str = "F0DB8767D611AF945D7D090ED988F3D30000000000000000";
        const P2: &str = "F0DB8767D611AF9454603FFC31D8DBF30000000000000000";
        let at = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let mut r = row(2, 60.);
        (r.ribbon_neutral, r.ribbon_owned, r.colour_neutral) = (NEUTRAL.into(), OWNED.into(), GREY.into());
        (r.colours_players, r.colours_teams) = (vec![P1.into(), P2.into()], vec![P1.into(), P2.into()]);
        r.objectives[0].indicators = vec![Some(at(1.)), None];
        r.objectives[1].indicators = vec![Some(at(2.))];
        let neutral = || "skate3:B094183C726DEB4E:1DA37485495114CE".to_owned();
        let blue = || "skate3:769C2AEE9C86C11A:54603FFC31D8DBF3".to_owned();
        let looks = |g: &Domination| g.markers().into_iter().map(|(look, m)| (look, m[3][0])).collect::<Vec<_>>();
        let mut g = Domination::new(r.clone(), 0., 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        assert!(g.markers().is_empty(), "nothing before setup");
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunChallenge);
        assert_eq!(looks(&g), [(neutral(), 1.), (neutral(), 2.)], "both spots neutral; the unplaced locator draws nothing");
        g.index = 1;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 1, score: 500 });
        assert_eq!(looks(&g), [(neutral(), 1.), (blue(), 2.)], "spot 2 is player 2's, in its colour");
        let t = rt.now + 70.;
        skate3_modes::advance(&mut g, &mut rt, t);
        until(&mut g, &mut rt, St::Complete);
        assert!(g.markers().is_empty(), "removed at cleanup");
        // A team match: player 1 (on team 2) takes spot 1, in team 2's colour.
        let mut g = Domination::new(r, 0., 2, 0);
        g.teams_in = vec![2, 1];
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        until(&mut g, &mut rt, St::RunChallenge);
        g.index = 0;
        skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 0, score: 500 });
        assert_eq!(looks(&g)[0], (blue(), 1.), "team 2's colour");
    }

    /// A peer's relayed i64::MAX on two spots: the owner's Score saturates, no overflow panic.
    #[test]
    fn huge_relayed_scores_saturate() {
        let mut g = Domination::new(row(2, 60.), 0., 2, 0);
        let mut rt = Rt::new(St::Initial, 2, 0.);
        skate3_modes::start(&mut g, &mut rt);
        while rt.state != St::RunChallenge {
            let t = rt.due().unwrap();
            skate3_modes::advance(&mut g, &mut rt, t);
        }
        for k in 0..2 {
            g.index = k;
            skate3_modes::dispatch(&mut g, &mut rt, Msg::Objective { player: 1, score: i64::MAX });
        }
        assert_eq!(g.comps[1].score, i64::MAX);
    }

    type Snapshot = (Vec<(i64, i64)>, Vec<(usize, i64, i64)>, [TeamStanding; 2]);

    /// Online audit 10-12: `n` consoles play one Domination through the match relay
    /// ([`skate3_modes::tests::Wire`]), started up to 0.3 s apart; each skater lands its `plan`'s (challenge clock, spot,
    /// score) on its own console; `leaver` (player, its challenge clock) quits then (what it sent still on the way is
    /// lost) and the others notice it 0.5 s + 0.4 s per slot later; `teams` the lobby's split (empty: none). Every console still
    /// here: its spots (owner, score), standings (player, points, score) and team standings.
    fn domination_over_the_wire(n: usize, seed: u64, latency: (f64, f64), loss: f64, plan: &dyn Fn(usize) -> Vec<(f64, usize, i64)>, late: f64, leaver: Option<(usize, f64)>, teams: &[u8]) -> Vec<Snapshot> {
        let r = row(3, 60.);
        let mut wire = skate3_modes::tests::Wire::new(seed, latency, loss);
        // The last console loaded `late` seconds after the others (mixed loading).
        let mut starts: Vec<f64> = (0..n).map(|_| 0.3 * wire.draw()).collect();
        starts[n - 1] += late;
        let mut plays: Vec<Option<Play>> = (0..n).map(|_| None).collect();
        let mut fed = vec![0; n];
        let plans: Vec<Vec<(f64, usize, i64)>> = (0..n).map(plan).collect();
        let mut left_at: Option<f64> = None;
        for frame in 0..60 * 200 {
            let t = frame as f64 / 60.;
            for i in 0..n {
                let quit = leaver.is_some_and(|(p, _)| p == i);
                if t < starts[i] || (quit && left_at.is_some()) {
                    continue;
                }
                let play = plays[i].get_or_insert_with(|| {
                    let mut g = Domination::new(r.clone(), 0., n, i);
                    g.teams_in = teams.to_vec();
                    let mut rt = Rt::new(St::Initial, n, t);
                    rt.online = true;
                    skate3_modes::start(&mut g, &mut rt);
                    let names = (0..n).map(|p| format!("P{p}")).collect();
                    Play { id: "d".into(), row: r.clone(), g, rt, names, me: i, online: true, hud: HudDriver::default(), placed: 0, playing: None, group: Vec::new(), sense: Sense::new(HashMap::new()), sync: EndSync::default(), inbox: Vec::new() }
                });
                if let Some((p, at)) = leaver
                    && quit
                    && play.g.started.is_some_and(|s| t - s >= at)
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
                let mut out: Vec<String> = play.tick(t).into_iter().collect();
                if play.live()
                    && let Some(started) = play.g.started
                {
                    while fed[i] < plans[i].len() && t - started >= plans[i][fed[i]].0 {
                        let (_, k, score) = plans[i][fed[i]];
                        out.extend(play.complete(k, score));
                        fed[i] += 1;
                    }
                }
                let received = wire.take(t, i).into_iter().map(|(from, text)| (from, serde_json::from_str(&text).unwrap())).collect();
                play.receive(received);
                out.extend(play.tick(t));
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
                (p.g.spots.iter().map(|s| (s.owner, s.score)).collect(), p.g.standings.iter().map(|s| (s.player, s.points, s.score)).collect(), p.g.team_standings)
            })
            .collect()
    }

    /// Online audit 10-12 (Critical): every console names the same owners and standings. Before the fix two skaters
    /// landing the same score on a spot within the relay's latency each owned it on their own console (the first to
    /// arrive kept it), and a spot taken in the challenge's last moments was dropped on consoles whose timer had
    /// already ended it.
    #[test]
    fn every_console_agrees_on_the_spots() {
        // Two skaters, the same score on spot 0, 50 ms apart: the earlier owns it everywhere.
        let tie = |p: usize| vec![(10. + 0.05 * p as f64, 0, 3000)];
        let two = domination_over_the_wire(2, 1, (0.15, 0.3), 0., &tie, 0., None, &[]);
        assert_eq!(two[0], two[1], "both consoles agree");
        assert_eq!(two[0].0[0], (1, 3000), "player 0 landed it first");
        // Six skaters, 10% loss: random spots and scores (ties included), one steal 50 ms before the end.
        for seed in 1..=8u64 {
            let plan = move |p: usize| {
                let mut v: Vec<(f64, usize, i64)> =
                    (0..8).map(|k| (5. + 6. * k as f64 + 0.01 * p as f64, ((seed as usize + p + k) % 3), 1000 * (1 + ((seed as usize * 7 + p * 3 + k) % 4) as i64))).collect();
                if p == 5 {
                    v.push((59.95, 2, 9000));
                }
                v
            };
            let all = domination_over_the_wire(6, seed, (0.15, 0.3), 0.1, &plan, 0., None, &[]);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed}: every console the same");
            }
            assert_eq!(all[0].0[2], (6, 9000), "seed {seed}: the last-moment steal counted");
        }
    }

    /// Matrix G2/G3 (10-12): six skaters (retail's max) with 150-300 ms and 10 % loss, a guest or the host (slot 0)
    /// leaving mid-challenge: every console still here ends it with the hand-computed owners and standings. Spot 0:
    /// P0 1000, P1 2000 (beats it), P3 2500 at 40 s; spot 1: P2 3000, P0 4000 at 30 s; spot 2: P3 1500, P4 1500 (a tie
    /// keeps the earlier owner), P5 1600 at 50 s. A leaver keeps what it owns (playerdisconnected 104-123 only marks it).
    /// PointsScoreSorter: P0 (1 spot, 4000), P3 (1, 2500), P5 (1, 1600), then the three with none.
    #[test]
    fn six_skaters_and_a_leaver_end_with_the_retail_standings() {
        let plan = |p: usize| match p {
            0 => vec![(5., 0, 1000), (30., 1, 4000)],
            1 => vec![(8., 0, 2000)],
            2 => vec![(10., 1, 3000)],
            3 => vec![(12., 2, 1500), (40., 0, 2500)],
            4 => vec![(15., 2, 1500)],
            _ => vec![(50., 2, 1600)],
        };
        for (seed, leaver) in [(1, None), (2, Some((4, 20.))), (3, Some((0, 35.))), (4, Some((4, 20.))), (5, Some((0, 35.)))] {
            let all = domination_over_the_wire(6, seed, (0.15, 0.3), 0.1, &plan, 0., leaver, &[]);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} {leaver:?}: every console the same");
            }
            assert_eq!(all[0].0, [(4, 2500), (1, 4000), (6, 1600)], "seed {seed} {leaver:?}: owners");
            assert_eq!(all[0].1[..3], [(1, 1, 4000), (4, 1, 2500), (6, 1, 1600)], "seed {seed} {leaver:?}: standings");
            let mut rest: Vec<usize> = all[0].1[3..].iter().map(|s| s.0).collect();
            rest.sort_unstable();
            assert_eq!(rest, [2, 3, 5]);
        }
    }

    /// Online audit 10-12 (end review M6): a spot taken just after the challenge started reaches a console still
    /// counting down (it loaded a second later) and counts there once its challenge runs; before, it was dropped there.
    #[test]
    fn an_early_spot_waits_for_the_challenge() {
        let plan = |p: usize| if p == 0 { vec![(0.05, 0, 3000)] } else { vec![(5., 1, 2000)] };
        for seed in 1..=4u64 {
            let two = domination_over_the_wire(2, seed, (0.15, 0.3), 0.05, &plan, 1., None, &[]);
            assert_eq!(two[0], two[1], "seed {seed}: both consoles agree");
            assert_eq!(two[0].0[0], (1, 3000), "seed {seed}: player 0 took spot 0 everywhere");
        }
    }

    /// Matrix (10-12): team Domination at 6 (3 a side, teams 1 2 1 2 1 2), 150-300 ms, 10 % loss, no leaver, a guest
    /// (1) or the host (0) leaving. By hand: spot 0 P0 1000, P1 2000, P2 2500 at 40 s; spot 1 P3 3000, P4 4000 at
    /// 30 s; spot 2 P5 1500. Team 1 (P2, P4) 2 spots 6500 ranks 1, team 2 (P5) 1 spot 1500 ranks 2 (CalculateScores
    /// 627-653); a leaver keeps what it owns (neither leaver owns a spot then).
    #[test]
    fn six_skaters_in_two_teams_end_with_the_retail_team_standings() {
        let plan = |p: usize| match p {
            0 => vec![(5., 0, 1000)],
            1 => vec![(8., 0, 2000)],
            2 => vec![(40., 0, 2500)],
            3 => vec![(10., 1, 3000)],
            4 => vec![(30., 1, 4000)],
            _ => vec![(12., 2, 1500)],
        };
        let teams = [1u8, 2, 1, 2, 1, 2];
        for (seed, leaver) in [(1, None), (2, Some((1, 20.))), (3, Some((0, 35.))), (4, None), (5, Some((1, 45.)))] {
            let all = domination_over_the_wire(6, seed, (0.15, 0.3), 0.1, &plan, 0., leaver, &teams);
            for c in &all[1..] {
                assert_eq!(*c, all[0], "seed {seed} {leaver:?}: every console the same");
            }
            assert_eq!(all[0].0, [(3, 2500), (5, 4000), (6, 1500)], "seed {seed} {leaver:?}: owners");
            let want = [TeamStanding { team: 1, points: 2, score: 6500, rank: 1 }, TeamStanding { team: 2, points: 1, score: 1500, rank: 2 }];
            assert_eq!(all[0].2, want, "seed {seed} {leaver:?}: the teams");
        }
    }

    /// The Team tab's rows play (match_core::team entries), as the Versus tab's do.
    #[test]
    fn team_rows_are_playable() {
        let ids: Vec<String> = ["dom_dwtn_01"].map(String::from).to_vec();
        let playable = Playable(entries(&ids).into_iter().collect());
        assert_eq!(crate::match_core::unavailable(crate::match_core::TEAM_TAB, &ids[0], &playable), None, "Team tab");
        assert_eq!(crate::match_core::unavailable(3, &ids[0], &playable), None, "Versus tab");
    }

    /// The tool's output (S3_DOMINATION_DIR): the 13 online rows load with their start slots, spots, intro camera and
    /// objectives; on each, a sequence through a spot's first volume (grinding there, so the grind snippets hold too)
    /// completes that spot's objective, once per sequence, and ResetObjective re-arms it.
    #[test]
    #[ignore = "requires the Skate 3 domination data (S3_DOMINATION_DIR)"]
    fn skate3_domination_data() {
        let root = std::path::PathBuf::from(std::env::var("S3_DOMINATION_DIR").unwrap());
        let data = ModeData::load(&root).unwrap();
        let rules = skate3_film::rules(&data).unwrap();
        let playable: Vec<String> = data.rows.keys().filter(|id| data.row::<Row>(id).unwrap().playable(id, &rules.points_objective)).cloned().collect();
        // 10-06 data: + dom_artg_01, dom_dark_01, dom_dwmc_01, dom_mmcn_01 (the DLC rows).
        assert_eq!(playable.len(), 17, "{playable:?}");
        let (mut spots, mut completed) = (0, 0);
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            let def = r.group().unwrap();
            for k in 0..def.objectives.len() {
                let text = data.rows[id.as_str()]["groups"]["Objectives"][k].to_string();
                let Some(volume) = r.trigger_volumes.keys().find(|v| text.contains(v.as_str())) else { continue };
                let mut groups = vec![(100, Objectives::new(0, &def, &rules.points_objective).unwrap())];
                let mut sense = Sense::new(rules.trick_groups.clone());
                let mut done = Vec::new();
                for f in skate3_film::sequence_through(volume.parse().unwrap(), 400.) {
                    for (_, _, o) in skate3_film::feed(&mut groups, &mut sense, f) {
                        if let Out::Complete { index, .. } = o {
                            done.push(index);
                            groups[0].1.reset_one(index);
                        }
                    }
                }
                spots += 1;
                if done.contains(&k) {
                    completed += 1;
                }
                assert!(done.iter().filter(|&&i| i == k).count() <= 1, "{id} {k}: {done:?}");
            }
        }
        eprintln!("domination objectives: {completed} of {spots} spots complete on a grinding sequence through their volume");
        assert!(completed * 10 >= spots * 9, "{completed} of {spots}");
        for id in &playable {
            let r: Row = data.row(id).unwrap();
            assert!(!data.text(&r.title).starts_with("ID_"), "{id}");
            assert!(r.intro.as_ref().is_some_and(|s| data.intros.contains_key(s)), "{id}");
            for s in [&r.owned, &r.lost, &r.increased, &r.new_lead, &r.new_lead_you] {
                assert!(!data.text(s).starts_with("ID_"), "{id}: {s}");
            }
        }
        for s in [DOMINATING, "ID_LEADERBOARD_POINTS_HEADER", "ID_COMMON_SCORE"] {
            assert!(!data.text(s).starts_with("ID_"), "{s}");
        }
    }
}
