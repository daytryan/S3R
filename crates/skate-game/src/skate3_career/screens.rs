//! The career's retail screens (plan Task 19; stage1_screens.md): which movie of `<export>/screens/runtime` shows a
//! screen the career waits on, what the movie asks the game, and what the game does with a key on it. popups.rs opens
//! them on the career layer ([`crate::apt_screen::CAREER`]). The yes/no popup and popup 306 stay on sk8popup, which is
//! retail's popup movie too (it has a body and no title, so the title is folded into the body); the teammate page
//! (112) stays on it until its name entry is traced. Not tested in play.

use serde_json::{Value as Json, json};

use super::core::{CareerState, Step, Summary};
use super::data::{CareerData, Reward};
use super::glue::Asking;
use super::rules::{Level, Rules};
use crate::core_menu_runtime::Key;

/// A retail movie for a career screen and what it is asked.
pub struct Retail {
    pub movie: &'static str,
    pub answers: Vec<(String, Json)>,
    /// A presses the screen takes before it closes (the milestone pages).
    pub pages: usize,
    /// The `SetState` the exe sends once the movie is up (the poster's 20, mState_Normal_Sign_Up: 0x825ED160).
    pub state: Option<u32>,
    /// Keys do nothing until the movie's intro is done (the poster: state+0x48 until vt[26], 0x825ED430; INFERRED that
    /// "shown" is the intro's end).
    pub wait_intro: bool,
}

/// What the game does with a key on a retail screen: the movie shows its next page (with the exe's sound), or the
/// screen closes with an option index (as the stand-in popup's: 0 Continue / Accept, 1 Decline) and its sound.
#[derive(Debug, PartialEq, Eq)]
pub enum Press {
    Page(&'static str),
    Close(usize, Option<&'static str>),
}

/// The retail movie for `asking`; None for a screen that stays on the stand-in popup.
#[must_use]
pub fn retail(state: &CareerState, asking: &Asking) -> Option<Retail> {
    let d = &*state.data;
    let screen = |movie, answers: Vec<(&str, Json)>, pages| Retail {
        movie,
        answers: answers.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        pages,
        state: None,
        wait_intro: false,
    };
    match asking {
        Asking::Skate101 { description } => {
            Some(screen("skate101", vec![("Mission.GetChallenge101DescriptionString", text(d, description))], 1))
        }
        // FE 65 (0x826504E0): one A per entry of R+8, counted against R+0x10; the movie pages itself on the same A.
        Asking::Step(Step::Milestones(_)) => Some(screen(
            "unlock_milestone",
            vec![("FERewardSummary.GetMilestoneUnlockInfo", milestone_info(d, &state.chain.items))],
            state.chain.items.len().max(1),
        )),
        // Screen 23 (0x825ED020): mState_Normal_Sign_Up for a solo row.
        Asking::Step(Step::Poster(row)) => Some(Retail {
            movie: "challenge_signup",
            answers: poster_answers(d, &state.rules, row),
            pages: 1,
            state: Some(20),
            wait_intro: true,
        }),
        // Screen 109 (FE 64).
        Asking::Step(Step::RewardSummary(summary)) => Some(Retail {
            movie: "unlockoverlay",
            answers: summary_answers(state, summary),
            pages: 1,
            state: None,
            wait_intro: false,
        }),
        _ => None,
    }
}

/// The career's string `id` as a literal (the movies' own language tables lack the career's strings).
fn text(d: &CareerData, id: &str) -> Json {
    json!(crate::apt_text::literal(d.text(id)))
}

/// What `key` does on `asking`'s retail screen with `pages_left` A presses to go (None: nothing, the key is ignored).
#[must_use]
pub fn press(asking: &Asking, key: Key, pages_left: usize) -> Option<Press> {
    match (asking, key) {
        (Asking::Skate101 { .. }, Key::Accept) => Some(Press::Close(0, Some("core_a_button"))),
        (Asking::Step(Step::Milestones(_)), Key::Accept) if pages_left > 1 => Some(Press::Page("core_a_button")),
        (Asking::Step(Step::Milestones(_)), Key::Accept) => Some(Press::Close(0, Some("core_a_button"))),
        // 0x825ED410: A accepts, B declines (the exe also posts hudyes / hudno to the sim).
        (Asking::Step(Step::Poster(_)), Key::Accept) => Some(Press::Close(0, Some("challenge_accept"))),
        (Asking::Step(Step::Poster(_)), Key::Back) => Some(Press::Close(1, Some("challenge_decline"))),
        (Asking::Step(Step::RewardSummary(_)), Key::Accept) => Some(Press::Close(0, Some("core_a_button"))),
        _ => None,
    }
}

/// `FERewardSummary.GetMilestoneUnlockInfo` (0x825B2F60, results+0x8): [names, icon paths, descriptions], one entry per
/// milestone unlock (unlock_milestone.swf reads index 0 name, 1 icon, 2 description).
#[must_use]
pub fn milestone_info(d: &CareerData, items: &[String]) -> Json {
    let rewards: Vec<Option<&Reward>> = items.iter().map(|item| d.rewards.get(item)).collect();
    let column = |f: &dyn Fn(&Reward) -> Json| Json::Array(rewards.iter().map(|r| r.map_or_else(|| json!(""), f)).collect());
    json!([column(&|r| text(d, &r.title)), column(&|r| json!(r.icon)), column(&|r| text(d, &r.description))])
}

/// The reward summary's tabs (`progression_reward_tabs`, stage1_screens 1.2): id, HALID, the eUNLOCK_CATEGORY values it
/// gathers, and whether it shows as a list (the characters tab shows portraits).
pub const REWARD_TABS: [(u32, &str, &[u32], bool); 5] = [
    (0, "ID_REWARD_TAB_CHARACTERS", &[1], false),
    (1, "ID_REWARD_TAB_CHALLENGES", &[12], true),
    (2, "ID_REWARD_TAB_GEAR", &[6, 5], true),
    (3, "ID_REWARD_TAB_LOCATIONS", &[0], true),
    (4, "ID_REWARD_TAB_SKATE_PARK", &[7, 9], true),
];

/// `milestone_actions`' children (stage1_screens 1.4, 0x828DA2C8): their thresholds are their board-sales triggers.
const MILESTONE_ACTIONS: [&str; 6] = [
    "milestone_01_complete",
    "milestone_02_complete",
    "milestone_03_complete",
    "milestone_04_complete",
    "milestone_05_complete",
    "milestone_final_complete",
];

/// The six milestone thresholds, ascending (comparator 0x827CC288).
fn thresholds(d: &CareerData) -> Vec<u64> {
    let mut thresholds: Vec<u64> = MILESTONE_ACTIONS
        .iter()
        .filter_map(|name| d.actions.iter().find(|a| a.name == *name))
        .filter_map(|a| a.conditions.iter().find(|c| c.0 == 0).map(|c| c.2))
        .collect();
    thresholds.sort_unstable();
    thresholds
}

/// `row` is finished (owned or killed).
fn done(rules: &Rules, row: &str) -> bool {
    rules.owned.iter().chain(&rules.killed).any(|r| r == row)
}

/// The milestone index m (0x827CD728): 0 until photo_univ_05 is done, else 6 less the milestone actions still armed,
/// less this batch's milestones (R+4).
fn milestone(state: &CareerState) -> usize {
    if !done(&state.rules, "photo_univ_05") {
        return 0;
    }
    let armed = MILESTONE_ACTIONS.iter().filter(|a| state.rules.actions_active.iter().any(|x| x == *a)).count();
    (MILESTONE_ACTIONS.len() - armed).saturating_sub(state.chain.milestones as usize)
}

/// The thresholds around milestone `m`: [m-1] and [m], 0 out of range.
fn bounds(d: &CareerData, m: usize) -> (u64, u64) {
    let t = thresholds(d);
    (m.checked_sub(1).and_then(|i| t.get(i)).copied().unwrap_or(0), t.get(m).copied().unwrap_or(0))
}

/// `coremenu.GetProgressionData` (0x825B73D8): [milestone state, milestone count, previous threshold, next threshold,
/// total board sales, cash]. The state is -2 while film_skpk_univ_01 is not done, -1 while photo_univ_05 is not, else
/// the milestone index m (0x827CD728): 6 less the milestone actions still armed, less this batch's milestones (R+4).
#[must_use]
pub fn progression_data(state: &CareerState) -> [f64; 6] {
    let r = &state.rules;
    let m = milestone(state);
    let phase = if !done(r, "film_skpk_univ_01") {
        -2.
    } else if !done(r, "photo_univ_05") {
        -1.
    } else {
        m as f64
    };
    let (prev, next) = bounds(&state.data, m);
    [phase, MILESTONE_ACTIONS.len() as f64, prev as f64, next as f64, r.stats.total_boards() as f64, r.stats.cash as f64]
}

/// The reward summary (screen 109, FE 64): its tabs, unlocks and board-sales bar from `summary` and the career.
#[must_use]
pub fn summary_answers(state: &CareerState, summary: &Summary) -> Vec<(String, Json)> {
    let d = &*state.data;
    let unlocks = |categories: &[u32]| -> Vec<&Reward> {
        summary.items.iter().filter_map(|i| d.rewards.get(i)).filter(|r| categories.contains(&r.category)).collect()
    };
    let tabs: Vec<_> = REWARD_TABS.iter().filter(|(_, _, categories, _)| !unlocks(categories).is_empty()).collect();
    let (prev, next) = bounds(d, milestone(state));
    let mut answers: Vec<(String, Json)> = [
        ("FERewardSummary.GetUnlockCategoryNames", json!(tabs.iter().map(|t| t.1).collect::<Vec<_>>())),
        ("FERewardSummary.GetUnlockCategoryIDs", json!(tabs.iter().map(|t| t.0).collect::<Vec<_>>())),
        ("FERewardSummary.GetOwnedBoardSalesReward", json!(summary.boards.saturating_sub(summary.kill_boards))),
        ("FERewardSummary.GetKilledBoardSalesReward", json!(summary.kill_boards)),
        ("FERewardSummary.GetOnlineBoardSalesReward", json!(0)),
        ("FERewardSummary.GetTotalBoardSales", json!(state.rules.stats.total_boards())),
        ("FERewardSummary.GetPrevBoardSaleMilestone", json!(prev)),
        ("FERewardSummary.GetNextBoardSaleMilestone", json!(next)),
        ("Mission.GetIsOnlineMission", json!(false)),
        ("coremenu.GetProgressionData", json!(progression_data(state))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (id, _, categories, list) in tabs {
        let shown = unlocks(categories);
        let titles: Vec<Json> = shown.iter().map(|r| text(d, &r.title)).collect();
        let icons: Vec<&str> = shown.iter().map(|r| r.icon.as_str()).collect();
        answers.push((format!("FERewardSummary.GetUnlocksInCategory({id})"), json!([titles, icons, vec![false; shown.len()]])));
        answers.push((format!("FERewardSummary.IsCategoryShownAsList({id})"), json!(list)));
    }
    answers
}

/// The sign-up poster (screen 23) for a solo career row (stage1_screens 5.2).
#[must_use]
pub fn poster_answers(d: &CareerData, rules: &Rules, row: &str) -> Vec<(String, Json)> {
    let Some(r) = d.row(row) else { return Vec::new() };
    let has = |list: &[String]| list.iter().any(|x| x == row);
    [
        ("SignUpPoster.GetSignUpTitle", text(d, &r.title)),
        ("SignUpPoster.GetSignUpDescription", text(d, &r.description)),
        ("SignUpPoster.CanSignUpToChallenge", json!(true)),
        // One option: the movie shows no menu.
        ("SignUpPoster.GetCICMenuOptions", json!(["ID_COMMON_ACCEPT"])),
        ("Mission.IsDescriptionEnabled", json!(true)),
        // Not retail (story mode stage 1 build): the export has no objective titles yet (Task 19 ruling).
        ("Mission.IsObjectivesEnabled", json!(false)),
        ("Mission.AreRewardsShownForChallenge", json!(true)),
        ("Mission.IsChallengeKillable", json!(r.killable)),
        ("Mission.IsChallengeOwned", json!(has(&rules.owned))),
        ("Mission.IsChallengeKilled", json!(has(&rules.killed))),
        ("Mission.GetBoardSalesReward(false,false)", json!(r.own.boards)),
        ("Mission.GetBoardSalesReward(false,true)", json!(r.kill.boards)),
        ("Mission.GetCoopBoardSaleBonusReward", json!(0)),
        ("Mission.ShowCareerTeammates", json!(false)),
        ("Mission.IsCompetitorsEnabled", json!(false)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// The career challenge list of the Challenge Map's Active and Completed tabs: (rows, state) with state 'new' / 'none'
/// for active rows and 'owned' / 'killed' for completed ones (0x826481B8), 'new' rows first, then by title (comparator
/// 0x82647B80; career_flow Q7).
#[must_use]
pub fn challenge_list(d: &CareerData, rules: &Rules) -> (Vec<(String, &'static str)>, Vec<(String, &'static str)>) {
    // INFERRED: the name compared (0x82644430) is the row's title as shown.
    fn shown<'a>(d: &'a CareerData, row: &'a str) -> &'a str {
        d.row(row).map_or(row, |r| d.text(&r.title))
    }
    let title = |row: &String| shown(d, row).to_owned();
    let order = |a: &(String, &str), b: &(String, &str)| (a.1 != "new").cmp(&(b.1 != "new")).then_with(|| title(&a.0).cmp(&title(&b.0)));
    let mut active: Vec<(String, &'static str)> =
        rules.active.iter().map(|row| (row.clone(), if rules.new.contains(row) { "new" } else { "none" })).collect();
    active.sort_by(order);
    let mut completed: Vec<(String, &'static str)> = rules.killed.iter().map(|row| (row.clone(), "killed")).collect();
    completed.extend(rules.owned.iter().filter(|row| !rules.killed.contains(row)).map(|row| (row.clone(), "owned")));
    completed.sort_by(order);
    (active, completed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::skate3_career::data::{Action, Payout, Row};
    use crate::skate3_career::glue::{self, NO_YES};

    fn skate101() -> Asking {
        Asking::Skate101 { description: "ID_TUT_101_DESC".into() }
    }

    fn data() -> CareerData {
        let reward = |title: &str, category: u32| Reward {
            category,
            item: String::new(),
            title: title.into(),
            description: format!("{title}_DESC"),
            icon: format!("icons/{title}.rx2"),
            raw: Vec::new(),
        };
        let row = |title: &str| Row { title: title.into(), ..Row::default() };
        let milestone = |name: &str, boards| Action { name: name.into(), conditions: vec![(0, String::new(), boards)], ..Action::default() };
        CareerData {
            strings: [("ID_TUT_101_DESC", "Push to go."), ("ID_A", "Alpha"), ("ID_A_DESC", "First"), ("ID_B", "Beta")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            rewards: [("a", reward("ID_A", 1)), ("b", reward("ID_B", 5)), ("egg", reward("ID_EGG", 3)), ("c", reward("ID_C", 6))]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
            rows: [
                ("r_zed", row("ID_Z")),
                ("r_alpha", row("ID_ALPHA")),
                ("r_mid", row("ID_MID")),
                ("r_done", row("ID_D")),
                ("r_poster", Row {
                    title: "ID_A".into(),
                    description: "ID_A_DESC".into(),
                    own: Payout { cash: 500, boards: 1200, rewards: vec![] },
                    kill: Payout { cash: 0, boards: 3000, rewards: vec![] },
                    killable: true,
                    ..Row::default()
                }),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
            actions: [
                ("milestone_03_complete", 100_000),
                ("milestone_01_complete", 15_000),
                ("milestone_02_complete", 50_000),
                ("milestone_04_complete", 300_000),
                ("milestone_05_complete", 600_000),
                ("milestone_final_complete", 1_000_000),
                ("public_milestone_01_complete", 24_750),
            ]
            .into_iter()
            .map(|(n, b)| milestone(n, b))
            .collect(),
            ..CareerData::default()
        }
    }

    fn state(items: &[&str]) -> CareerState {
        let mut state = CareerState::new(Arc::new(data()));
        state.chain.items = items.iter().map(|i| (*i).to_owned()).collect();
        state
    }

    fn answer<'a>(retail: &'a [(String, Json)], key: &str) -> &'a Json {
        &retail.iter().find(|(k, _)| k == key).unwrap_or_else(|| panic!("no answer for {key}")).1
    }

    /// career_flow Q3 (0x825EC6E8): Skate 101 closes only on A (key 2), with core_a_button; every other key is ignored.
    #[test]
    fn skate101_closes_only_on_a() {
        assert_eq!(press(&skate101(), Key::Accept, 1), Some(Press::Close(0, Some("core_a_button"))));
        for key in [Key::Back, Key::Up, Key::Down, Key::Left, Key::Right] {
            assert_eq!(press(&skate101(), key, 1), None, "{key:?}");
        }
    }

    /// skate101.swf's OnLoaded sets its body from Mission.GetChallenge101DescriptionString; it hides its header.
    #[test]
    fn skate101_asks_for_its_description() {
        let screen = retail(&state(&[]), &skate101()).expect("skate101 is a retail movie");
        assert_eq!(screen.movie, "skate101");
        assert_eq!(answer(&screen.answers, "Mission.GetChallenge101DescriptionString"), &json!(crate::apt_text::literal("Push to go.")));
    }

    /// Spec 18.13 / career_flow Q3: the yes/no popup is sk8popup with No (hudno) first, not cancelable, so B does
    /// nothing and the focus starts on No.
    #[test]
    fn the_yes_no_popup_starts_on_no_and_ignores_b() {
        assert!(retail(&state(&[]), &Asking::YesNo).is_none(), "sk8popup, the stand-in, is retail's popup");
        assert_eq!(NO_YES, ["ID_COMMON_NO", "ID_COMMON_YES"]);
        let shown = glue::yes_no(&data(), "ID_A", "ID_B");
        assert!(!shown.cancelable);
        assert_eq!(shown.options, NO_YES);
    }

    /// Popup 306 (career_flow Q2 step 11): "Want to take your game online?" under the warning title, one OK.
    #[test]
    fn popup_306_text() {
        let d = CareerData {
            strings: [("ID_COMMON_WARNING_TITLE", "Warning"), ("ID_MAP_VISIT_THE_ONLINEMAP", "Go online")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            ..CareerData::default()
        };
        let shown = glue::shown(&d, &Step::OnlinePopup).unwrap();
        assert_eq!(shown.text, "#Warning  Go online");
        assert_eq!(shown.options, glue::OK);
    }

    /// stage1_screens 2.2 (FE 65, 0x826504E0): one A per milestone unlock, each with core_a_button; the movie pages
    /// itself on the same A and the last one closes the screen.
    #[test]
    fn the_reward_chain_pages_milestones_one_a_at_a_time() {
        let asking = Asking::Step(Step::Milestones(1));
        let screen = retail(&state(&["a", "b"]), &asking).expect("unlock_milestone is a retail movie");
        assert_eq!((screen.movie, screen.pages), ("unlock_milestone", 2));
        assert_eq!(press(&asking, Key::Accept, 2), Some(Press::Page("core_a_button")));
        assert_eq!(press(&asking, Key::Back, 1), None);
        assert_eq!(press(&asking, Key::Accept, 1), Some(Press::Close(0, Some("core_a_button"))));
    }

    #[test]
    fn milestone_info_is_names_icons_and_descriptions() {
        let info = milestone_info(&data(), &["a".to_owned(), "b".to_owned()]);
        let lit = crate::apt_text::literal;
        assert_eq!(info[0], json!([lit("Alpha"), lit("Beta")]), "names, as the career's strings give them");
        assert_eq!(info[1], json!(["icons/ID_A.rx2", "icons/ID_B.rx2"]));
        assert_eq!(info[2], json!([lit("First"), lit("ID_B_DESC")]));
    }

    /// career_flow Q7 (0x826481B8, 0x82647B80): 'new' rows first, then by title; completed rows say owned / killed.
    #[test]
    fn the_challenge_map_sorts_new_rows_first() {
        let rules = Rules {
            active: vec!["r_zed".into(), "r_alpha".into(), "r_mid".into()],
            new: vec!["r_zed".into()],
            owned: vec!["r_done".into()],
            ..Rules::default()
        };
        let (active, completed) = challenge_list(&data(), &rules);
        let names: Vec<(&str, &str)> = active.iter().map(|(r, s)| (r.as_str(), *s)).collect();
        assert_eq!(names, [("r_zed", "new"), ("r_alpha", "none"), ("r_mid", "none")]);
        assert_eq!(completed, [("r_done".to_owned(), "owned")]);
    }

    /// stage1_screens 5.3 (0x825ED410): A accepts (hudyes, challenge_accept), B declines (hudno, challenge_decline).
    #[test]
    fn the_poster_maps_a_and_b() {
        let poster = Asking::Step(Step::Poster("r_poster".into()));
        assert_eq!(press(&poster, Key::Accept, 1), Some(Press::Close(0, Some("challenge_accept"))));
        assert_eq!(press(&poster, Key::Back, 1), Some(Press::Close(1, Some("challenge_decline"))));
        assert_eq!(press(&poster, Key::Up, 1), None);
    }

    /// stage1_screens 5.1-5.2: challenge_signup in mState_Normal_Sign_Up (20) for a solo row: its title, description,
    /// Own / Kill board sales, sign-up allowed, keys held until it is shown.
    #[test]
    fn the_poster_asks_for_a_solo_rows_sign_up() {
        let screen = retail(&state(&[]), &Asking::Step(Step::Poster("r_poster".into()))).expect("challenge_signup");
        assert_eq!((screen.movie, screen.state, screen.wait_intro), ("challenge_signup", Some(20), true));
        let lit = crate::apt_text::literal;
        assert_eq!(answer(&screen.answers, "SignUpPoster.GetSignUpTitle"), &json!(lit("Alpha")));
        assert_eq!(answer(&screen.answers, "SignUpPoster.GetSignUpDescription"), &json!(lit("First")));
        assert_eq!(answer(&screen.answers, "SignUpPoster.CanSignUpToChallenge"), &json!(true));
        assert_eq!(answer(&screen.answers, "Mission.GetBoardSalesReward(false,false)"), &json!(1200));
        assert_eq!(answer(&screen.answers, "Mission.GetBoardSalesReward(false,true)"), &json!(3000));
        assert_eq!(answer(&screen.answers, "Mission.IsChallengeKillable"), &json!(true));
        assert_eq!(answer(&screen.answers, "Mission.IsChallengeOwned"), &json!(false));
        assert_eq!(answer(&screen.answers, "Mission.ShowCareerTeammates"), &json!(false));
        assert_eq!(answer(&screen.answers, "SignUpPoster.GetCICMenuOptions"), &json!(["ID_COMMON_ACCEPT"]), "one option: no menu");
    }

    /// stage1_screens 1.2-1.3: a tab per reward-tab record with a matching unlock, in id order; Eggs (3) get no tab.
    #[test]
    fn the_summary_tabs_follow_the_reward_tab_records() {
        let summary = Summary { cash: 0, boards: 0, kill_boards: 0, items: vec!["c".into(), "egg".into(), "a".into(), "b".into()] };
        let answers = summary_answers(&state(&[]), &summary);
        assert_eq!(answer(&answers, "FERewardSummary.GetUnlockCategoryNames"), &json!(["ID_REWARD_TAB_CHARACTERS", "ID_REWARD_TAB_GEAR"]));
        assert_eq!(answer(&answers, "FERewardSummary.GetUnlockCategoryIDs"), &json!([0, 2]));
        let lit = crate::apt_text::literal;
        assert_eq!(
            answer(&answers, "FERewardSummary.GetUnlocksInCategory(2)"),
            &json!([[lit("ID_C"), lit("Beta")], ["icons/ID_C.rx2", "icons/ID_B.rx2"], [false, false]]),
            "award order"
        );
        assert_eq!(answer(&answers, "FERewardSummary.IsCategoryShownAsList(0)"), &json!(false));
        assert_eq!(answer(&answers, "FERewardSummary.IsCategoryShownAsList(2)"), &json!(true));
        assert_eq!(answer(&answers, "Mission.GetIsOnlineMission"), &json!(false));
    }

    /// stage1_screens 1.1 / 1.4: the board-sales bar: this result's Own and Kill sales, the career total, and the
    /// milestone bounds around it.
    #[test]
    fn the_summary_board_bar() {
        let mut s = state(&[]);
        s.rules.stats.own_boards = 20_000;
        s.rules.stats.kill_boards = 3_000;
        for row in ["film_skpk_univ_01", "photo_univ_05"] {
            s.rules.owned.push(row.into());
        }
        s.rules.actions_active = MILESTONE_ACTIONS[1..].iter().map(|a| (*a).to_owned()).collect();
        let summary = Summary { cash: 0, boards: 4_000, kill_boards: 3_000, items: vec![] };
        let answers = summary_answers(&s, &summary);
        assert_eq!(answer(&answers, "FERewardSummary.GetOwnedBoardSalesReward"), &json!(1_000));
        assert_eq!(answer(&answers, "FERewardSummary.GetKilledBoardSalesReward"), &json!(3_000));
        assert_eq!(answer(&answers, "FERewardSummary.GetOnlineBoardSalesReward"), &json!(0));
        assert_eq!(answer(&answers, "FERewardSummary.GetTotalBoardSales"), &json!(23_000));
        assert_eq!(answer(&answers, "FERewardSummary.GetPrevBoardSaleMilestone"), &json!(15_000));
        assert_eq!(answer(&answers, "FERewardSummary.GetNextBoardSaleMilestone"), &json!(50_000));
    }

    /// stage1_screens 3 (0x825B73D8): six values; -2 before film_skpk_univ_01, -1 before photo_univ_05, then m.
    #[test]
    fn progression_data_reads_the_career() {
        let mut s = state(&[]);
        s.rules.stats.own_boards = 20_000;
        s.rules.stats.cash = 750;
        s.rules.actions_active = MILESTONE_ACTIONS[1..].iter().map(|a| (*a).to_owned()).collect();
        assert_eq!(progression_data(&s)[0], -2.);
        s.rules.owned.push("film_skpk_univ_01".into());
        assert_eq!(progression_data(&s)[0], -1.);
        s.rules.owned.push("photo_univ_05".into());
        assert_eq!(progression_data(&s), [1., 6., 15_000., 50_000., 20_000., 750.], "one milestone past");
        s.chain.milestones = 1;
        assert_eq!(progression_data(&s)[..4], [0., 6., 0., 15_000.], "while milestone 1's batch shows, m = 0");
        assert_eq!(thresholds(&data()), [15_000, 50_000, 100_000, 300_000, 600_000, 1_000_000]);
    }

    /// The real movies (SKATE3_CAREER_DIR): each asks for what the port answers, and asks nothing else.
    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn the_retail_movies_ask_what_the_port_answers() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_CAREER_DIR").unwrap()).join("screens/runtime");
        let s = state(&["a", "b"]);
        let summary = Summary { cash: 100, boards: 1_200, kill_boards: 0, items: vec!["a".into(), "c".into()] };
        let mut problems = Vec::new();
        for asking in [
            skate101(),
            Asking::Step(Step::Milestones(1)),
            Asking::Step(Step::Poster("r_poster".into())),
            Asking::Step(Step::RewardSummary(summary)),
        ] {
            let retail = retail(&s, &asking).unwrap();
            let answers: Vec<(&str, Json)> = retail.answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let mut screen = crate::apt_screen::Screen::load(&root.join(format!("{}.json", retail.movie)), &answers).unwrap();
            if screen.has("OnLoaded") {
                screen.call("OnLoaded", &[]).unwrap();
            }
            screen.play("intro").unwrap();
            if let Some(n) = retail.state {
                screen.call("SetState", &[json!(n)]).unwrap();
            }
            for _ in 0..90 {
                screen.advance().unwrap();
            }
            for _ in 1..retail.pages {
                screen.input(Key::Accept).unwrap();
                screen.advance().unwrap();
            }
            let calls: Vec<String> = screen.take_calls().into_iter().map(|(c, _)| c).collect();
            for (key, _) in &retail.answers {
                let native = key.split('(').next().unwrap();
                if !calls.iter().any(|c| c == native) {
                    problems.push(format!("{}: {native} was never asked", retail.movie));
                }
            }
            // Getters only: the menu's OnMenuHilight / OnMenuSelect reports need no answer.
            let getter = |k: &&String| k.contains(".Get") || k.contains(".Is") || k.contains(".Can") || k.contains(".Are");
            for key in screen.bindings.unanswered.iter().filter(getter).filter(|k| !k.starts_with("Game.ImageRenderer")) {
                problems.push(format!("{}: {key} unanswered", retail.movie));
            }
        }
        // Every movie's problems at once, so one run shows the whole list.
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
