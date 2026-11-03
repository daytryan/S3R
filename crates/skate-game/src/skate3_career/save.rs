//! The career's save, `settings/skate3-career.json` (spec 9): what retail's SKATER.P holds for the career (career_flow
//! Q4) plus the last place, the play time and the Challenge Map's bests; and how a launch starts from it, including
//! the D4 upgrade of an install that finished the opening before the career existed. Not tested in play.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::core::{CareerState, Chain, Place};
use super::data::CareerData;
use super::rules::{Rules, Stats};
use crate::saves::{self, Loaded};

pub const FILE: &str = "skate3-career.json";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actions {
    pub active: Vec<String>,
    pub fired: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenges {
    pub active: Vec<String>,
    pub owned: Vec<String>,
    pub killed: Vec<String>,
    pub new: Vec<String>,
}

/// A row's best on the Challenge Map.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Best {
    pub score: f64,
    pub level: u32,
}

/// What stage 1 held back at its stop (spec 14; Task 20 fills it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub steps: Vec<HeldStep>,
    pub notice_shown: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeldStep {
    Teleport { location: String, world: String },
    Present(String),
    Poster(String),
}

/// `skate3-career.json`, field for field as spec 9 shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CareerSave {
    pub version: u32,
    pub actions: Actions,
    pub challenges: Challenges,
    pub stats: Stats,
    pub globals: serde_json::Value,
    pub discovery: BTreeMap<String, String>,
    pub place: Option<Place>,
    pub playtime: f64,
    pub new_career: bool,
    pub online_popup_shown: bool,
    pub reward_chain: Chain,
    pub bests: BTreeMap<String, Best>,
    pub achievements: Vec<String>,
    pub skatefeed: Vec<String>,
    pub brand_tiers: Vec<String>,
    pub teammates: u32,
    pub held: Option<Held>,
}

/// The save of `s`. A running stategraph is not saved, only its globals (career_flow Q4).
#[must_use]
pub fn from_state(s: &CareerState) -> CareerSave {
    let r = &s.rules;
    CareerSave {
        version: VERSION,
        actions: Actions { active: r.actions_active.clone(), fired: r.actions_fired.clone() },
        challenges: Challenges { active: r.active.clone(), owned: r.owned.clone(), killed: r.killed.clone(), new: r.new.clone() },
        stats: r.stats.clone(),
        globals: s.globals.clone(),
        discovery: s.discovery.clone(),
        place: s.place.clone(),
        playtime: s.playtime,
        new_career: s.new_career,
        online_popup_shown: s.online_popup_shown,
        reward_chain: s.chain.clone(),
        bests: s.bests.clone(),
        achievements: r.achievements.clone(),
        skatefeed: s.skatefeed.clone(),
        brand_tiers: s.brand_tiers.clone(),
        teammates: s.teammates,
        held: s.held.clone(),
    }
}

/// The boards-sold achievements retail's load grants from the totals (career_flow Q4 load, 0x8274A590).
const BOARD_ACHIEVEMENTS: [(&str, u64); 6] = [
    ("employee_2", 15_000),
    ("head_quartered", 50_000),
    ("employee_3", 100_000),
    ("employee_4", 300_000),
    ("full_team", 600_000),
    ("sold_a_lot_of_boards", 1_000_000),
];

/// The career state a save holds; the load grants the boards-sold achievements its totals reach.
#[must_use]
pub fn into_state(save: CareerSave, d: Arc<CareerData>) -> CareerState {
    let mut state = CareerState::new(d);
    state.rules = Rules {
        actions_active: save.actions.active,
        actions_fired: save.actions.fired,
        active: save.challenges.active,
        owned: save.challenges.owned,
        killed: save.challenges.killed,
        new: save.challenges.new,
        stats: save.stats,
        achievements: save.achievements,
    };
    let total = state.rules.stats.total_boards();
    for (name, boards) in BOARD_ACHIEVEMENTS {
        if total >= boards && !state.rules.achievements.iter().any(|a| a == name) {
            state.rules.achievements.push(name.to_owned());
        }
    }
    state.globals = save.globals;
    state.discovery = save.discovery;
    state.place = save.place;
    state.playtime = save.playtime;
    state.new_career = save.new_career;
    state.online_popup_shown = save.online_popup_shown;
    state.chain = save.reward_chain;
    state.bests = save.bests;
    state.skatefeed = save.skatefeed;
    state.brand_tiers = save.brand_tiers;
    state.teammates = save.teammates;
    state.held = save.held;
    state
}

pub fn load(settings: &Path) -> Loaded<CareerSave> {
    saves::load_json(&settings.join(FILE))
}

/// Writes whole, keeping the last good copy (`saves::write_atomic`).
///
/// # Errors
///
/// When the file cannot be written.
pub fn write(settings: &Path, s: &CareerSave) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
    saves::write_atomic(&settings.join(FILE), &bytes)
}

/// How this launch's career begins.
#[derive(Debug, PartialEq)]
pub enum Start {
    /// The opening has not run: a new career.
    New,
    /// D4 (`Not retail (user 10-20)`): the opening ran before the career existed; its Coach Frank answer picks the
    /// first world, and `None` asks it once.
    Upgrade { tutorial: Option<bool> },
    Resume(CareerSave),
    /// The save is damaged with no good backup (kept aside by `saves`), or could not be read: the career is off this
    /// session and nothing saves over it.
    Off(String),
}

#[must_use]
pub fn start(boot: &crate::boot::Career, save: Loaded<CareerSave>) -> Start {
    if !boot.intro_done {
        return Start::New;
    }
    let off = save.problem();
    match save {
        Loaded::Loaded(save) | Loaded::Restored(save, _) => Start::Resume(save),
        Loaded::Missing => Start::Upgrade { tutorial: boot.tutorial },
        Loaded::Damaged(_) | Loaded::Unreadable => Start::Off(off.unwrap_or_else(|| "the career save could not be loaded".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_career::core::{Core, Ready, Step};
    use crate::skate3_career::rules::Outcome;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("s3career-save-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn full() -> CareerSave {
        CareerSave {
            version: VERSION,
            actions: Actions { active: vec!["ftl_univ_02_complete".into()], fired: vec!["starting_action".into()] },
            challenges: Challenges { active: vec!["ftl_univ_02".into()], owned: vec!["tut_sksc_01".into()], killed: vec!["tut_sksc_02".into()], new: vec!["ftl_univ_02".into()] },
            stats: Stats { own_boards: 1, kill_boards: 2, online_boards: 3, profile_boards: 4, cash: 5, cash_earned: 6 },
            globals: serde_json::json!({ "CompletedFirstFTL": false }),
            discovery: [("ots_univ_01".to_owned(), "SeenIntro".to_owned())].into_iter().collect(),
            place: Some(Place { world: Some("dist_university".into()), map: "maps/University.skate".into(), location: Some("ftl_univ_02_challengelocator_01".into()) }),
            playtime: 12.5,
            new_career: true,
            online_popup_shown: true,
            reward_chain: Chain { milestones: 1, teammates: 2, items: vec!["x".into()] },
            bests: [("ftl_univ_02".to_owned(), Best { score: 10.0, level: 1 })].into_iter().collect(),
            achievements: vec!["a".into()],
            skatefeed: vec!["ID_SKATEFEED_X".into()],
            brand_tiers: vec!["0213_logo_level_10".into()],
            teammates: 3,
            held: Some(Held { steps: vec![HeldStep::Teleport { location: "l".into(), world: "w".into() }, HeldStep::Present("p".into()), HeldStep::Poster("r".into())], notice_shown: true }),
        }
    }

    #[test]
    fn a_career_save_round_trips() {
        let dir = scratch("round");
        let save = full();
        write(&dir, &save).unwrap();
        assert_eq!(load(&dir).ok(), Some(save.clone()));
        let state = into_state(save.clone(), Arc::new(CareerData::default()));
        assert_eq!(from_state(&state), save, "the state keeps every field");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Review Focus 3.
    #[test]
    fn a_damaged_save_restores_the_last_good_one() {
        let dir = scratch("damaged");
        let first = full();
        write(&dir, &first).unwrap();
        assert!(matches!(load(&dir), Loaded::Loaded(_)), "the load keeps a last-good copy");
        write(&dir, &CareerSave { playtime: 99.0, ..first.clone() }).unwrap();
        std::fs::write(dir.join(FILE), b"{ damaged").unwrap();
        let Loaded::Restored(restored, bad) = load(&dir) else { panic!("restored from .last-good") };
        assert_eq!(restored, first);
        assert!(bad.file_name().unwrap().to_string_lossy().starts_with("skate3-career.json.bad-"), "{}", bad.display());
        assert_eq!(std::fs::read(&bad).unwrap(), b"{ damaged", "the damaged copy is kept aside");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loading_grants_the_board_sales_achievements() {
        let with = |boards: u64| {
            let save = CareerSave { stats: Stats { own_boards: boards, ..Stats::default() }, achievements: vec![], ..full() };
            into_state(save, Arc::new(CareerData::default())).rules.achievements
        };
        assert!(with(14_999).is_empty());
        assert_eq!(with(15_000), ["employee_2"]);
        assert_eq!(with(1_000_000), ["employee_2", "head_quartered", "employee_3", "employee_4", "full_team", "sold_a_lot_of_boards"]);
    }

    fn boot(intro_done: bool, tutorial: Option<bool>) -> crate::boot::Career {
        crate::boot::Career { intro_done, team_name: None, tutorial }
    }

    #[test]
    fn an_old_install_upgrades_by_its_coach_frank_answer() {
        for tutorial in [Some(true), Some(false), None] {
            assert_eq!(start(&boot(true, tutorial), Loaded::Missing), Start::Upgrade { tutorial });
        }
        assert_eq!(start(&boot(true, None), Loaded::Loaded(full())), Start::Resume(full()));
        assert!(matches!(start(&boot(true, None), Loaded::Damaged("x.bad-1".into())), Start::Off(_)));
    }

    #[test]
    fn a_new_career_starts_new() {
        assert_eq!(start(&boot(false, None), Loaded::Missing), Start::New);
        assert_eq!(start(&boot(false, Some(true)), Loaded::Loaded(full())), Start::New);
    }

    /// Review Focus 1: quitting at any point of the post-challenge sequence and relaunching strands nothing. Each cut
    /// saves the state, rebuilds it (a fresh `Core`: the running graph and the sequence are not saved, as retail's)
    /// and drives to idle.
    #[test]
    fn quitting_anywhere_in_the_sequence_strands_nothing() {
        use crate::skate3_career::data::{Action, Event, Payout, Row, State, StateGraph};
        let lua = r#"
            function enter(self, m, v, s, g)
                ChallengeStatus.ActivateChallenge("next_row", false)
                Unlock.MilestoneEarned()
                Unlock.MilestoneEarned()
                Notifications101.Show101("T", "D")
            end
            function after(self, m, v, s, g) Events.DeferEventsAndChangeState("tail") end
            function tail(self, m, v, s, g) SignUp.SetAutoSignUp("next_row") ChangeState("complete") end
        "#;
        let state_of = |handlers: &[(&str, &str)], events: usize| State {
            handlers: handlers.iter().map(|(m, h)| ((*m).to_owned(), (*h).to_owned())).collect(),
            events: (0..events).map(|_| Event { kind: 5, args: vec![], flags: 0 }).collect(),
        };
        let graph = StateGraph {
            initial: "initial".into(),
            states: [
                ("initial".to_owned(), state_of(&[("stateenter", "enter"), ("hudcomplete", "after")], 1)),
                ("tail".to_owned(), state_of(&[("stateenter", "tail")], 0)),
                ("complete".to_owned(), state_of(&[], 0)),
            ]
            .into_iter()
            .collect(),
        };
        let row = |cash| Row { own: Payout { cash, boards: 500, rewards: vec![] }, world: "w".into(), location: "l".into(), ..Row::default() };
        let d = Arc::new(CareerData {
            lua: lua.into(),
            actions: vec![Action { name: "done".into(), conditions: vec![(2, "row".into(), 0)], stategraph: "g".into(), ..Action::default() }],
            stategraphs: [("g".to_owned(), graph)].into_iter().collect(),
            rows: [("row".to_owned(), row(100)), ("next_row".to_owned(), row(0))].into_iter().collect(),
            ..CareerData::default()
        });
        let fresh = || {
            let mut s = CareerState::new(d.clone());
            s.rules.activate_challenge(&d, "row");
            s.rules.activate_action(&d, "done");
            s
        };
        // Drives to idle, answering every 101; the steps, and how many were closed before `cut` stopped it.
        let drive = |core: &mut Core, s: &mut CareerState, cut: Option<usize>| -> (Vec<Step>, bool) {
            let mut steps = Vec::new();
            let mut events = 0;
            for _ in 0..200 {
                for effect in core.pump(s) {
                    if matches!(effect, crate::skate3_career::progression::Effect::Show101 { .. }) {
                        events += 1;
                        if cut == Some(events) {
                            return (steps, true);
                        }
                        core.reply("hudcomplete", s);
                    }
                }
                if let Some(step) = core.next(&Ready::default(), s) {
                    if cut.is_some_and(|c| c == 100 + steps.len()) {
                        return (steps, true);
                    }
                    core.done(&step, s);
                    steps.push(step);
                } else if !s.runner.busy() && core.idle() && core.pending_signup().is_none() {
                    return (steps, false);
                }
            }
            panic!("never idle: {steps:?}");
        };
        // The full run, uncut.
        let mut s = fresh();
        let mut core = Core::default();
        core.finished("row", Outcome::Owned, &mut s);
        let (full_steps, _) = drive(&mut core, &mut s, None);
        assert!(full_steps.len() >= 4, "{full_steps:?}");
        let paid = s.rules.stats.clone();
        // Cut inside the 101 wait (1), and before each step of the sequence (100 + n).
        for cut in std::iter::once(1).chain((0..full_steps.len()).map(|n| 100 + n)) {
            let mut s = fresh();
            let mut core = Core::default();
            core.finished("row", Outcome::Owned, &mut s);
            let (before, stopped) = drive(&mut core, &mut s, Some(cut));
            assert!(stopped, "cut {cut} never came");
            let save = serde_json::to_vec(&from_state(&s)).unwrap();
            let mut s = into_state(serde_json::from_slice(&save).unwrap(), d.clone());
            let mut core = Core::default();
            let (after, _) = drive(&mut core, &mut s, None);
            assert!(s.rules.active.contains(&"next_row".to_owned()), "cut {cut}: next_row lost its marker");
            assert_eq!(s.rules.stats, paid, "cut {cut}: paid once");
            assert_eq!(s.rules.actions_active, Vec::<String>::new(), "cut {cut}: the action fired once, not re-armed");
            for step in &after {
                assert!(!before.contains(step) || matches!(step, Step::Autosave), "cut {cut}: {step:?} came twice");
            }
        }
    }
}
