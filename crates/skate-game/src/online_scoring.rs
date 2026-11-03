//! Retail Skate 3's per-challenge trick-score multiplier gate and ManualsAreConnectors, the ways an online match
//! scores differently from freeskate (scoring-report.md in .superpowers/sdd/2026-10-04-online-modes;
//! input-2788-report.md in .superpowers/sdd/2026-10-24-spot-battle-scoring).
//!
//! TU3 8275F430 builds the ScoreModule's input every frame and puts 82761548's answer in bit 0x04000000 of
//! +2788; 82DA3C68 copies that bit to ScoreModule +130, which [`crate::scoring_runtime::Runtime`] holds as
//! `multiplier_enabled`. 82761548:
//!
//! ```text
//! r3 = [[[TheSimulation]+220]+328]->vfunc16()       ; the current challenge
//! if !r3 -> 1                                        ; none: freeskate keeps the multiplier
//! type = GlobalData.ChallengeType (824639E8)
//! if type in {2, 8, 15}                               ; demo, street, vert contests
//!     round = LocalData.ContestRounds[[[[TheSimulation]+220]+356]+1100]   ; 824826D0
//!     -> round && round.CumulativeScoring
//! else -> LocalData.ScoringMultiplier
//! ```
//!
//! Every lookup that finds nothing reads the zero bytes at 0x830D0850, i.e. false. The per-challenge values are
//! data (`tools/online_scoring.py` -> `private/skate3-online/scoring.json`); this is the decision.
//!
//! The same input's bit 0x40000000 is the player record's ManualsAreConnectors byte (`[0x83067068] + id * 0xA8 +
//! 0x74`), `Runtime::manuals_connect`: set by the record's init (824FA9A0) and by a challenge's shutdown (8256CAB0);
//! at a challenge's start (8256C918), unless its ChallengeType is 8 or 15, its GlobalData's
//! SetManualAndWalkAsConnector, when present, for every player (8256C840); inside its connector spot volumes set again
//! (8256CB60); and by the mode scripts' ChallengeManager.SetManualsAreConnectors (82844474).
use crate::{
    match_core::{Match, Mode},
    physics::SkaterRuntime,
};
use bevy::prelude::*;
use std::collections::HashMap;

const PATH: &str = "private/skate3-online/scoring.json";

/// One challenge's records, as `tools/online_scoring.py` reads them.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(crate) struct Challenge {
    /// GlobalData ChallengeType (02B828CC..).
    #[serde(rename = "type")]
    pub kind: u32,
    /// LocalData ScoringMultiplier (CB837EEF..), through its parents.
    pub scoring_multiplier: bool,
    /// LocalData ContestRounds (7AFEB11F..), each round's CumulativeScoring (DD21D0F4..).
    pub cumulative_scoring: Vec<bool>,
    /// GlobalData SetManualAndWalkAsConnector (0661A17A..), through its parents; `None` where no record sets it.
    #[serde(default)]
    pub manual_connector: Option<bool>,
    /// LocalData's connector spot volumes (0A675615..): how many.
    #[serde(default)]
    pub connector_volumes: usize,
}

#[derive(Resource)]
pub(crate) struct ScoringRules {
    challenges: HashMap<String, Challenge>,
    /// `SequenceConnector.SetContestRound(n)` stores n - 1 (82856CF8, [[TheSimulation+220]+356]+1100); a contest
    /// mode writes it as its rounds advance (contest_shared Lua line 13).
    pub contest_round: usize,
    /// A Skate 2 contest's ChallengeType while it runs (skate2_contests.rs); Skate 2 has no ScoringMultiplier data.
    pub skate2: Option<u32>,
    /// The challenge system's trick-type mask (`[[TheSimulation+0xDC]+0x1A0]+0xC`, which 8275F430 copies into the
    /// ScoreModule input): set by a script's ChallengeManager.DisableAllTrickTypes / EnableTrickType /
    /// ResetTrickTypeScoring (82844490 / 828444E8 / 82844558), all on otherwise ([`challenge_ended`], world load).
    pub trick_types: [bool; 14],
}

impl Default for ScoringRules {
    fn default() -> Self {
        Self { challenges: HashMap::new(), contest_round: 0, skate2: None, trick_types: [true; 14] }
    }
}

#[derive(serde::Deserialize)]
struct File {
    version: u32,
    challenges: HashMap<String, Challenge>,
}

impl ScoringRules {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let file: File = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if file.version != 1 {
            return Err(format!("unsupported scoring.json version {}", file.version));
        }
        Ok(Self { challenges: file.challenges, ..default() })
    }

    /// The match's row is the current challenge from its Intro to its Results; Skate 1 / 2 rows are not in the
    /// data, and no match (freeskate, offline or online) has no challenge.
    // ponytail: retail's current challenge also exists before Start Challenge in a challenge lobby (freeskate_ol);
    // the engine has no challenge there, so the multiplier stays on (base freeskate_locations: ScoringMultiplier 1).
    pub fn for_match(&self, m: &Match) -> bool {
        if let (Some(_), Some(kind)) = (m.mode, self.skate2) {
            return skate2_multiplier_enabled(kind);
        }
        multiplier_enabled(m.mode.and(self.challenges.get(&m.row)), self.contest_round)
    }

    /// ManualsAreConnectors while the match's challenge runs: set without one.
    pub fn manuals_connect(&self, m: &Match) -> bool {
        let Some(mode) = m.mode else { return true };
        // shortcut: a Skate 2 contest's GlobalData is not extracted; Spot Battle, Best Trick, Jam and 1-Up call no
        // SetManualsAreConnectors. Upgrade with skate2port's data.
        if self.skate2.is_some() {
            return true;
        }
        // shortcut: these modes' scripts clear it as they set up (contest_shared 665, skate.lua 215 and Skate 2's 207,
        // domination 861, race_shared 446, Skate 1's besttrick scripts) and set it back only inside their spot
        // volumes and trick-for-time gates, which are not ported. Upgrade when the ports relay those triggers.
        if matches!(mode, Mode::Contest | Mode::Skate | Mode::Domination | Mode::DeathRace | Mode::BestTrick) {
            return false;
        }
        match self.challenges.get(&m.row) {
            // shortcut: with connector volumes (6 Spot Battle rows: ots_dwtn_01, ots_indu_01..05) manuals connect only
            // inside them, and leaving the last one in a manual ends the sequence (8256CC60); taken as inside, where
            // the spot's lines are. Upgrade with the volumes' boxes.
            Some(c) if !matches!(c.kind, 8 | 15) => c.manual_connector.is_none_or(|on| on || c.connector_volumes > 0),
            _ => true,
        }
    }
}

/// 82761548 for a challenge (`None`: no current challenge) and its contest round.
pub(crate) fn multiplier_enabled(challenge: Option<&Challenge>, round: usize) -> bool {
    let Some(c) = challenge else { return true };
    match c.kind {
        // 0x82761594..0x827615A8: `cmpwi r11,2 / 8 / 15`. A round past the array reads the zero default.
        2 | 8 | 15 => c.cumulative_scoring.get(round).copied().unwrap_or(false),
        _ => c.scoring_multiplier,
    }
}

/// Skate 2's gate (82322628) has no data field: a jump table on ChallengeType (0x8232268C) returns 0 at
/// 0x82322708 for Photo (6), Best Trick (8), Spot Battle (9), MegaRamp (26), CreateASpot (35, 36), else 1. The
/// same offline and online, like Skate 3's.
pub(crate) fn skate2_multiplier_enabled(challenge_type: u32) -> bool {
    !matches!(challenge_type, 6 | 8 | 9 | 26 | 35 | 36)
}

/// A contest round's trick types by its eContestType, the same in both games' setup_round (Skate 3 contest_shared
/// 687-698; Skate 2's h_6FDB2BFE54BA96FD, every Best Trick and Jam template): Air_Only (15) DisableAllTrickTypes
/// then enables Air, Flip, FingerFlip, Grab; Grind_Only (16) only Grind; any other round ResetTrickTypeScoring.
fn round_trick_types(variant: u32) -> [bool; 14] {
    match variant {
        15 => std::array::from_fn(|t| matches!(t, 1..=4)),
        16 => std::array::from_fn(|t| t == 5),
        _ => [true; 14],
    }
}

fn set_trick_types(world: &mut World, types: [bool; 14]) {
    if let Some(mut rules) = world.get_resource_mut::<ScoringRules>()
        && rules.trick_types != types
    {
        rules.trick_types = types;
    }
}

/// A contest's setup_round for its current round.
pub(crate) fn set_round_trick_types(world: &mut World, variant: u32) {
    set_trick_types(world, round_trick_types(variant));
}

/// Every type on again when the challenge that set the mask ends: the challenge system tells its listeners
/// (82BD1138, slot 10), and the mask's (82D5B5C8) resets if that challenge last wrote it.
pub(crate) fn challenge_ended(world: &mut World) {
    set_trick_types(world, [true; 14]);
}

/// A loaded world builds a new challenge system, its mask all on (825506C8).
fn world_loaded(mut changed: MessageReader<crate::map_transition::WorldChanged>, mut rules: ResMut<ScoringRules>) {
    if changed.read().count() > 0 && rules.trick_types != [true; 14] {
        rules.trick_types = [true; 14];
    }
}

/// 8275F430 refreshes the bits every frame, before the scorer ticks.
fn apply(
    rules: Res<ScoringRules>,
    m: Res<Match>,
    online: Option<Res<crate::multiplayer::Online>>,
    skater: Option<ResMut<SkaterRuntime>>,
    mut connect: Local<Option<bool>>,
) {
    let Some(mut skater) = skater else { return };
    // The record byte changes only on those events, so this writes on a change (or a new skater) only and a Skate 1
    // mission's SetManualsAreConnectors (skate1_challenge) stands between them.
    let manuals = rules.manuals_connect(&m);
    if *connect != Some(manuals) || skater.is_added() {
        *connect = Some(manuals);
        skater.scoring.manuals_connect = manuals;
    }
    let enabled = rules.for_match(&m);
    if skater.scoring.multiplier_enabled != enabled {
        skater.scoring.multiplier_enabled = enabled;
    }
    if skater.scoring.trick_types != rules.trick_types {
        skater.scoring.trick_types = rules.trick_types;
    }
    // 82761460 (0x82761514): input +2788 bit 0x00800000 only while not Online.IsOnline().
    let offline = !online.is_some_and(|o| o.0);
    if skater.scoring.offline != offline {
        skater.scoring.offline = offline;
    }
}

pub(crate) struct OnlineScoringPlugin;
impl Plugin for OnlineScoringPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScoringRules>()
            .add_systems(Startup, |mut rules: ResMut<ScoringRules>, config: Res<crate::config::Config>| {
                let path = config.asset_root.join(PATH);
                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| ScoringRules::parse(&b)) {
                    Ok(loaded) => *rules = loaded,
                    // ponytail: without the extracted data every challenge keeps the multiplier; the launcher's
                    // pipeline runs tools/online_scoring.py (scoring-report.md, Requests).
                    Err(e) => warn!("{}: {e}; Skate 3 challenges keep the score multiplier", path.display()),
                }
            })
            .add_systems(Update, world_loaded)
            .add_systems(FixedUpdate, apply.before(crate::app::SimulationSet::Physics));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(kind: u32, scoring_multiplier: bool, rounds: &[bool]) -> Challenge {
        Challenge { kind, scoring_multiplier, cumulative_scoring: rounds.to_vec(), ..default() }
    }

    /// 8256C918 / 8256C840 on scoring.json's Spot Battle rows: ots_dwtn_02_ol clears ManualsAreConnectors,
    /// ots_dwtn_01_ol clears it outside its connector volume, ots_dwtn_05_ol sets it, ots_dwtn_10_ol has no field.
    #[test]
    fn a_spot_battle_row_sets_whether_manuals_connect() {
        let json = br#"{"version":1,"challenges":{
            "ots_dwtn_01_ol":{"type":53,"scoring_multiplier":false,"cumulative_scoring":[],"manual_connector":false,"connector_volumes":1},
            "ots_dwtn_02_ol":{"type":53,"scoring_multiplier":false,"cumulative_scoring":[],"manual_connector":false,"connector_volumes":0},
            "ots_dwtn_05_ol":{"type":53,"scoring_multiplier":false,"cumulative_scoring":[],"manual_connector":true,"connector_volumes":0},
            "ots_dwtn_10_ol":{"type":53,"scoring_multiplier":false,"cumulative_scoring":[]},
            "street_dwtn_01":{"type":8,"scoring_multiplier":true,"cumulative_scoring":[],"manual_connector":false}}}"#;
        let mut rules = ScoringRules::parse(json).unwrap();
        let mut m = Match::default();
        m.row = "ots_dwtn_02_ol".into();
        assert!(rules.manuals_connect(&m), "no challenge running: the record's default");
        m.mode = Some(Mode::SpotBattle);
        let rows = ["ots_dwtn_01_ol", "ots_dwtn_02_ol", "ots_dwtn_05_ol", "ots_dwtn_10_ol", "f_1a_05_ots_pj05_alphabet_ol"];
        let got = rows.map(|row| {
            m.row = row.into();
            rules.manuals_connect(&m)
        });
        assert_eq!(got, [true, false, true, true, true]);
        m.row = "street_dwtn_01".into(); // ChallengeType 8: 8256C918 leaves the players alone
        assert!(rules.manuals_connect(&m));
        m.mode = Some(Mode::Skate); // skate.lua 215
        assert!(!rules.manuals_connect(&m));
        rules.skate2 = Some(9);
        m.mode = Some(Mode::SpotBattle);
        m.row = "ots_dwtn_02_ol".into();
        assert!(rules.manuals_connect(&m), "a Skate 2 contest");
    }

    #[test]
    fn no_challenge_keeps_the_multiplier() {
        assert!(multiplier_enabled(None, 0));
    }

    #[test]
    fn non_contests_read_scoring_multiplier_and_ignore_rounds() {
        // spot battle (53), 1-Up (32), Domination (51), S.K.A.T.E. (4): framework ScoringMultiplier 0.
        for kind in [53, 32, 51, 4] {
            assert!(!multiplier_enabled(Some(&challenge(kind, false, &[true])), 0));
        }
        // Death Race (19), Hall of Meat battles (33): inherit main's 1.
        assert!(multiplier_enabled(Some(&challenge(19, true, &[])), 0));
    }

    #[test]
    fn contests_read_the_current_rounds_cumulative_scoring() {
        // street_dwtn_01: rounds [1, 0, 1].
        let street = challenge(8, true, &[true, false, true]);
        assert_eq!((0..4).map(|r| multiplier_enabled(Some(&street), r)).collect::<Vec<_>>(), [true, false, true, false]);
        // A contest ignores ScoringMultiplier entirely (demo_dwmc_01: ScoringMultiplier 0, round 1 cumulative).
        assert!(multiplier_enabled(Some(&challenge(2, false, &[true])), 0));
        assert!(!multiplier_enabled(Some(&challenge(15, true, &[])), 0));
    }

    fn rules() -> ScoringRules {
        let json = br#"{"version":1,"challenges":{
            "ots_dwtn_06_ol":{"type":53,"scoring_multiplier":false,"cumulative_scoring":[]},
            "street_dwtn_01":{"type":8,"scoring_multiplier":true,"cumulative_scoring":[true,false,true]}}}"#;
        ScoringRules::parse(json).unwrap()
    }

    #[test]
    fn only_a_match_on_a_known_row_changes_the_gate() {
        let mut rules = rules();
        let mut m = Match::default();
        assert!(rules.for_match(&m)); // freeskate
        m.row = "ots_dwtn_06_ol".into();
        assert!(rules.for_match(&m)); // a row without a match is not a current challenge
        m.mode = Some(crate::match_core::Mode::SpotBattle);
        assert!(!rules.for_match(&m));
        m.row = "f_1a_05_ots_pj05_alphabet_ol".into(); // Skate 1 / 2 rows: not Skate 3 data
        assert!(rules.for_match(&m));
        m.row = "street_dwtn_01".into();
        rules.contest_round = 1;
        assert!(!rules.for_match(&m));
    }

    /// The same two flips (50, then 10) published freeskate and in a spot battle, through the module's publish
    /// (skate_core Session = 82DA37B0) with the gate `for_match` returns. Numbers: scoring-report.md, "Worked example".
    #[test]
    fn the_same_sequence_offline_and_in_a_spot_battle() {
        use skate_core::scoring::{Scorable, session::{Rules, Session}};
        let session_rules = Rules {
            combo_capacity: 801.0,
            combo_levels: [(50.0, 1.5), (450.0, 2.0), (800.0, 3.0)],
            combo_refresh_threshold: 799.0,
            line_capacity: 400.0,
            bail_factor: 0.0,
        };
        let flip = Scorable { id: 96, class: 3, score_type: 2 };
        let run = |enabled: bool| {
            let mut s = Session::default();
            let rewards: Vec<f32> = [50.0, 10.0]
                .into_iter()
                .map(|raw| {
                    s.holder.end_trick(flip, raw);
                    s.holder.finish_collector();
                    s.publish_sequence(&session_rules, 1.0, false, enabled)
                })
                .collect();
            (rewards, s.combo.multiplier, s.holder.snapshot.line, s.line.points > 0.0)
        };
        let rules = rules();
        let mut m = Match::default();
        assert_eq!(run(rules.for_match(&m)), (vec![50.0, 15.0], 1.5, 65.0, true));
        m.mode = Some(crate::match_core::Mode::SpotBattle);
        m.row = "ots_dwtn_06_ol".into();
        assert_eq!(run(rules.for_match(&m)), (vec![50.0, 10.0], 1.0, 0.0, false));
    }

    #[test]
    fn skate2_contests_follow_skate2s_challenge_type_table() {
        // Best Trick (8), Spot Battle (9) off; Jam (15), 1-Up (32) on (scoring-report.md, "Skate 2").
        assert_eq!([8, 9, 15, 32].map(skate2_multiplier_enabled), [false, false, true, true]);
        let mut rules = rules();
        let mut m = Match::default();
        rules.skate2 = Some(9);
        assert!(rules.for_match(&m)); // no match running yet
        m.mode = Some(crate::match_core::Mode::SpotBattle);
        m.row = "street_dwtn_01".into(); // the Skate 2 contest wins over any Skate 3 data
        assert!(!rules.for_match(&m));
        rules.skate2 = Some(15);
        assert!(rules.for_match(&m));
    }

    /// setup_round's ChallengeManager calls (contest_shared 687-698) hold until the next round or the challenge's end.
    #[test]
    fn a_contest_round_sets_the_trick_types_until_the_challenge_ends() {
        let mut world = World::new();
        world.insert_resource(ScoringRules::default());
        let on = |w: &World| (0..14).filter(|&t| w.resource::<ScoringRules>().trick_types[t]).collect::<Vec<_>>();
        assert_eq!(on(&world).len(), 14);
        set_round_trick_types(&mut world, 15); // Air_Only: Air, Flip, FingerFlip, Grab
        assert_eq!(on(&world), [1, 2, 3, 4]);
        set_round_trick_types(&mut world, 16); // Grind_Only
        assert_eq!(on(&world), [5]);
        set_round_trick_types(&mut world, 0); // any other round: ResetTrickTypeScoring
        assert_eq!(on(&world).len(), 14);
        set_round_trick_types(&mut world, 15);
        challenge_ended(&mut world);
        assert_eq!(on(&world).len(), 14);
    }

    #[test]
    fn parse_rejects_other_versions() {
        let ok = br#"{"version":1,"challenges":{"ots_dwtn_06_ol":{"type":53,"scoring_multiplier":false,"cumulative_scoring":[]}}}"#;
        let rules = ScoringRules::parse(ok).unwrap();
        assert_eq!(rules.challenges["ots_dwtn_06_ol"], challenge(53, false, &[]));
        assert!(ScoringRules::parse(br#"{"version":2,"challenges":{}}"#).is_err());
    }
}
