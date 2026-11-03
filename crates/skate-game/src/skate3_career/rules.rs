//! The exe's career rules (spec 3): the action manager, the challenge status lists, what a result pays and the career
//! totals. Pure; the progression runner and the glue act on what these return. Not tested in play.

use serde::{Deserialize, Serialize};

use super::data::{Action, CareerData, Payout};

/// A row's completion level as the status database reads it (0x82C1D988): 0 Owned, 1 Killed, 2 not done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Owned = 0,
    Killed = 1,
    NotDone = 2,
}

/// `EActivityCompletionType`, what a challenge ends with (career_flow Q2 step 2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Abandoned = 0,
    Owned = 1,
    Killed = 2,
}

/// The career totals (prog+0x48, rewards.md 4). Offline, online and profile boards stay 0.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct Stats {
    pub own_boards: u64,
    pub kill_boards: u64,
    pub online_boards: u64,
    pub profile_boards: u64,
    pub cash: u64,
    pub cash_earned: u64,
}

impl Stats {
    /// `GetTotalBoardSales` (0x825B1630): the four board counters.
    #[must_use]
    pub fn total_boards(&self) -> u64 {
        self.own_boards + self.kill_boards + self.online_boards + self.profile_boards
    }
}

/// The career state these rules keep (and the save stores).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct Rules {
    /// Armed actions waiting on their conditions, newest first (spec 12.4, stage1_traces 7).
    pub actions_active: Vec<String>,
    /// Flagged (auto-start) actions that fired (mgr+0x14), saved so they fire once per career.
    pub actions_fired: Vec<String>,
    pub active: Vec<String>,
    pub owned: Vec<String>,
    pub killed: Vec<String>,
    /// Newly activated rows, until started.
    pub new: Vec<String>,
    pub stats: Stats,
    pub achievements: Vec<String>,
}

/// An action that fired: its stategraph is the runner's to start (empty for none).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fired {
    pub action: String,
    pub stategraph: String,
}

/// What one recorded result paid.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Paid {
    pub cash: u64,
    pub boards: u64,
    /// The Kill share of `boards`.
    pub kill_boards: u64,
    pub rewards: Vec<String>,
    /// The Own The Lot parent this result finished, paid as Owned.
    pub lot: Option<String>,
    pub achievements: Vec<String>,
}

/// Which totals changed, for `reevaluate`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Changed {
    All,
    Boards,
    Cash,
}

/// The 80-completion achievement (0x827CDD60).
const EFFIN_JORDAN: &str = "effin_jordan";

impl Rules {
    /// One condition (0x82C1CA48's dispatch, gates Q1).
    fn condition(&self, d: &CareerData, action: &Action, (kind, name, value): &(u32, String, u64)) -> bool {
        match kind {
            0 => self.stats.total_boards() >= *value,
            // A missing group counts as false (0x82C1CC78).
            1 => d.groups.get(name).is_some_and(|members| {
                members.iter().filter(|row| self.level(row) != Level::NotDone).count() as u64 >= *value
            }),
            2 => self.level(name) != Level::NotDone,
            3 => false,
            4 => self.stats.cash_earned >= *value,
            // Not retail (user 10-20, spec 3 D3): branding level 10's $250,000 in hand, unreachable offline, reads as
            // the 250,000 boards its reward text announces.
            5 if action.name == "branding_level_10_complete" => self.stats.total_boards() >= *value,
            5 => self.stats.cash >= *value,
            // Kinds above 5 are skipped, counting as true (0x82C1CADC).
            _ => true,
        }
    }

    /// AND with short-circuit; an empty list is true (0x82C1CA48).
    fn holds(&self, d: &CareerData, action: &Action) -> bool {
        action.conditions.iter().all(|condition| self.condition(d, action, condition))
    }

    /// Fires `action`: a flagged one is remembered as fired (0x82C1CCEC), then its stategraph starts (0x82C1CD3C).
    fn fire(&mut self, action: &Action) -> Fired {
        if action.auto && !self.actions_fired.contains(&action.name) {
            self.actions_fired.push(action.name.clone());
        }
        Fired { action: action.name.clone(), stategraph: action.stategraph.clone() }
    }

    /// `ActivateAction` (0x82C1BF50): an armed or fired action is left alone (0x82C1BF98, crosscheck D8); otherwise it
    /// fires at once if its conditions hold, else it is armed. The fired list holds only flagged actions, so an
    /// unflagged action that fired arms and fires again (gates "milestone_5 re-arming").
    pub fn activate_action(&mut self, d: &CareerData, name: &str) -> Option<Fired> {
        if self.actions_active.iter().any(|armed| armed == name) || self.actions_fired.iter().any(|fired| fired == name) {
            return None;
        }
        let action = d.action(name)?;
        if self.holds(d, action) {
            Some(self.fire(action))
        } else {
            self.actions_active.insert(0, name.to_owned());
            None
        }
    }

    /// `ActivateChallenge`: an Owned or Killed row is left alone (0x82c1d7d0), a disabled row is never activated
    /// (FD40E4, 0x82BCF57C). Answers whether the row was newly activated.
    pub fn activate_challenge(&mut self, d: &CareerData, row: &str) -> bool {
        if self.level(row) != Level::NotDone || d.row(row).is_none_or(|r| r.disabled) || self.active.iter().any(|a| a == row) {
            return false;
        }
        self.active.push(row.to_owned());
        if !self.new.iter().any(|n| n == row) {
            self.new.push(row.to_owned());
        }
        true
    }

    /// Activates each member of `group` in the group's order; answers the rows newly activated.
    pub fn activate_group(&mut self, d: &CareerData, group: &str) -> Vec<String> {
        let members = d.groups.get(group).cloned().unwrap_or_default();
        members.into_iter().filter(|row| self.activate_challenge(d, row)).collect()
    }

    pub fn deactivate_challenge(&mut self, row: &str) {
        self.active.retain(|a| a != row);
        self.new.retain(|n| n != row);
    }

    /// 0x82C1D988.
    #[must_use]
    pub fn level(&self, row: &str) -> Level {
        if self.killed.iter().any(|k| k == row) {
            Level::Killed
        } else if self.owned.iter().any(|o| o == row) {
            Level::Owned
        } else {
            Level::NotDone
        }
    }

    /// A row was started: it is no longer new, nor is its lot parent (career_flow Q7).
    pub fn started(&mut self, d: &CareerData, row: &str) {
        let parent = d.row(row).and_then(|r| r.lot.clone());
        self.new.retain(|n| n != row && Some(n) != parent.as_ref());
    }

    /// Adds `payout` to the totals and to `paid`.
    fn pay(&mut self, payout: &Payout, kill: bool, paid: &mut Paid) {
        if kill {
            self.stats.kill_boards += payout.boards;
            paid.kill_boards += payout.boards;
        } else {
            self.stats.own_boards += payout.boards;
        }
        self.stats.cash += payout.cash;
        self.stats.cash_earned += payout.cash;
        paid.cash += payout.cash;
        paid.boards += payout.boards;
        paid.rewards.extend(payout.rewards.iter().cloned());
    }

    /// Stores `row` at `outcome`'s level; the row always leaves the activated list (0x82C1D878, career_flow Q2 step 5).
    fn store(&mut self, row: &str, outcome: Outcome) {
        if outcome != Outcome::Abandoned {
            self.active.retain(|a| a != row);
        }
        match outcome {
            Outcome::Abandoned => {}
            Outcome::Owned => {
                if self.level(row) == Level::NotDone {
                    self.owned.push(row.to_owned());
                }
            }
            Outcome::Killed => {
                self.owned.retain(|o| o != row);
                if !self.killed.iter().any(|k| k == row) {
                    self.killed.push(row.to_owned());
                }
            }
        }
    }

    /// Records a result (0x827CDB10, rewards.md 3): abandoned records nothing; the award runs before the new status
    /// is stored (not done -> Owned: Own; not done -> Killed: Own and Kill; Owned -> Killed: Kill; else nothing). Then
    /// a finished Own The Lot parent is recorded and paid (0x827cdb20-0x827cdcc4), `effin_jordan` at the 80th kill
    /// (0x827CDD60; the count is the Killed list, rewards.md 3 note 11, INFERRED), and the re-evaluation: board sales
    /// (0x82C1C2A0), cash (0x82C1C498) and, after a first completion, every armed action (0x82C1C698, career_flow Q2
    /// step 6).
    pub fn record(&mut self, d: &CareerData, row: &str, outcome: Outcome) -> (Paid, Vec<Fired>) {
        let mut paid = Paid::default();
        if outcome == Outcome::Abandoned {
            return (paid, Vec::new());
        }
        let previous = self.level(row);
        let payouts = d.row(row).map(|r| (r.own.clone(), r.kill.clone()));
        if let Some((own, kill)) = &payouts {
            if previous == Level::NotDone {
                self.pay(own, false, &mut paid);
            }
            if outcome == Outcome::Killed && previous != Level::Killed {
                self.pay(kill, true, &mut paid);
            }
        }
        self.store(row, outcome);
        let first = previous == Level::NotDone;
        if first
            && let Some(parent) = d.row(row).and_then(|r| r.lot.clone())
            && self.level(&parent) == Level::NotDone
            && let Some(lot) = d.row(&parent)
            && lot.lot_rows.iter().all(|member| self.level(member) != Level::NotDone)
        {
            let own = lot.own.clone();
            self.pay(&own, false, &mut paid);
            self.store(&parent, Outcome::Owned);
            paid.lot = Some(parent);
        }
        if self.killed.len() >= 80 && !self.achievements.iter().any(|a| a == EFFIN_JORDAN) {
            self.achievements.push(EFFIN_JORDAN.to_owned());
            paid.achievements.push(EFFIN_JORDAN.to_owned());
        }
        let mut fired = Vec::new();
        if paid.boards > 0 {
            fired.extend(self.reevaluate(d, Changed::Boards));
        }
        if paid.cash > 0 {
            fired.extend(self.reevaluate(d, Changed::Cash));
        }
        if first {
            fired.extend(self.walk(d, |_| true));
        }
        (paid, fired)
    }

    /// The career root (0x827cea60): every flagged action not yet fired is activated. In the main bank that is only
    /// `starting_action`.
    pub fn root(&mut self, d: &CareerData) -> Vec<Fired> {
        let flagged: Vec<String> = d.actions.iter().filter(|a| a.auto && !self.actions_fired.contains(&a.name)).map(|a| a.name.clone()).collect();
        flagged.iter().filter_map(|name| self.activate_action(d, name)).collect()
    }

    /// Re-checks the armed actions `what` concerns: board sales the kind-0 ones (0x82C1C2A0), cash the kind-4 and
    /// kind-5 ones (0x82C1C498), `All` every one (0x82C1C178).
    pub fn reevaluate(&mut self, d: &CareerData, what: Changed) -> Vec<Fired> {
        self.walk(d, |action| {
            action.conditions.iter().any(|(kind, ..)| match what {
                Changed::All => true,
                Changed::Boards => *kind == 0 || action.name == "branding_level_10_complete" && *kind == 5,
                Changed::Cash => matches!(kind, 4 | 5),
            }) || what == Changed::All
        })
    }

    /// Walks the armed actions front to back; the ones `concerned` picks whose conditions now hold leave the list
    /// and fire (0x82C1DCE0).
    fn walk(&mut self, d: &CareerData, concerned: impl Fn(&Action) -> bool) -> Vec<Fired> {
        let mut fired = Vec::new();
        for name in self.actions_active.clone() {
            let Some(action) = d.action(&name) else { continue };
            if concerned(action) && self.holds(d, action) {
                self.actions_active.retain(|armed| *armed != name);
                fired.push(self.fire(action));
            }
        }
        fired
    }

    /// Create-a-Skater purchases: cash down, never below 0 (0x827CEE00).
    pub fn spend(&mut self, cash: u64) {
        self.stats.cash = self.stats.cash.saturating_sub(cash);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::skate3_career::data::Row;

    fn action(name: &str, auto: bool, conditions: &[(u32, &str, u64)]) -> Action {
        Action {
            name: name.into(),
            auto,
            conditions: conditions.iter().map(|(k, n, v)| (*k, (*n).to_owned(), *v)).collect(),
            stategraph: format!("{name}_stategraph"),
            ..Action::default()
        }
    }

    fn row(own: (u64, u64), kill: (u64, u64)) -> Row {
        Row {
            own: Payout { cash: own.0, boards: own.1, rewards: vec!["own_reward".into()] },
            kill: Payout { cash: kill.0, boards: kill.1, rewards: vec![] },
            killable: true,
            ..Row::default()
        }
    }

    fn data_with(actions: Vec<Action>, groups: &[(&str, &[&str])], rows: Vec<(&str, Row)>) -> CareerData {
        CareerData {
            actions,
            groups: groups.iter().map(|(g, m)| ((*g).to_owned(), m.iter().map(|s| (*s).to_owned()).collect())).collect(),
            rows: rows.into_iter().map(|(n, r)| (n.to_owned(), r)).collect::<BTreeMap<_, _>>(),
            ..CareerData::default()
        }
    }

    fn rules_with(boards: u64, cash: u64, earned: u64) -> Rules {
        Rules { stats: Stats { own_boards: boards, cash, cash_earned: earned, ..Stats::default() }, ..Rules::default() }
    }

    #[test]
    fn activating_an_action_twice_fires_once() {
        let d = data_with(vec![action("go", true, &[])], &[], vec![]);
        let mut r = Rules::default();
        assert_eq!(r.activate_action(&d, "go"), Some(Fired { action: "go".into(), stategraph: "go_stategraph".into() }));
        assert_eq!(r.activate_action(&d, "go"), None, "a fired flagged action is left alone");
        assert_eq!(r.actions_fired, ["go"]);
        // An unflagged one that fired is in no list: it fires again (gates "milestone_5 re-arming").
        let d = data_with(vec![action("again", false, &[])], &[], vec![]);
        let mut r = Rules::default();
        assert!(r.activate_action(&d, "again").is_some());
        assert!(r.activate_action(&d, "again").is_some());
        assert!(r.actions_fired.is_empty());
    }

    #[test]
    fn an_unmet_action_waits_newest_first() {
        let d = data_with(vec![action("a", false, &[(3, "", 0)]), action("b", false, &[(3, "", 0)])], &[], vec![]);
        let mut r = Rules::default();
        assert_eq!(r.activate_action(&d, "a"), None);
        assert_eq!(r.activate_action(&d, "b"), None);
        assert_eq!(r.activate_action(&d, "a"), None, "armed again: no duplicate");
        assert_eq!(r.actions_active, ["b", "a"]);
    }

    #[test]
    fn conditions_are_and_with_short_circuit_and_empty_is_true() {
        let d = data_with(
            vec![action("empty", false, &[]), action("both", false, &[(0, "", 10), (5, "", 5)]), action("one", false, &[(0, "", 10), (3, "", 0)])],
            &[],
            vec![],
        );
        let mut r = rules_with(10, 5, 5);
        assert!(r.activate_action(&d, "empty").is_some());
        assert!(r.activate_action(&d, "both").is_some());
        assert!(r.activate_action(&d, "one").is_none(), "the never condition fails the AND");
    }

    #[test]
    fn each_condition_kind() {
        let rows = vec![("r1", row((0, 0), (0, 0))), ("r2", row((0, 0), (0, 0)))];
        let d = data_with(
            vec![
                action("boards", false, &[(0, "", 100)]),
                action("group", false, &[(1, "g", 2)]),
                action("lost_group", false, &[(1, "nope", 0)]),
                action("row", false, &[(2, "r1", 0)]),
                action("never", false, &[(3, "", 0)]),
                action("earned", false, &[(4, "", 50)]),
                action("cash", false, &[(5, "", 50)]),
                action("nine", false, &[(9, "", 1_000_000)]),
            ],
            &[("g", &["r1", "r2"])],
            rows,
        );
        let met = |r: &Rules, name: &str| r.holds(&d, d.action(name).unwrap());
        let mut r = rules_with(99, 49, 50);
        assert!(!met(&r, "boards"));
        r.stats.kill_boards = 1;
        assert!(met(&r, "boards"), "total boards counts every counter");
        assert!(!met(&r, "group"));
        r.owned.push("r1".into());
        assert!(met(&r, "row"));
        assert!(!met(&r, "group"));
        r.killed.push("r2".into());
        assert!(met(&r, "group"), "Owned or Killed members count");
        assert!(!met(&r, "lost_group"), "a missing group is false");
        assert!(!met(&r, "never"));
        assert!(met(&r, "earned"));
        assert!(!met(&r, "cash"));
        r.stats.cash = 50;
        assert!(met(&r, "cash"));
        assert!(met(&r, "nine"), "kinds above 5 count as true");
    }

    #[test]
    fn the_payment_table() {
        let d = data_with(vec![], &[], vec![("r", row((100, 1000), (50, 500)))]);
        let paid = |r: &mut Rules, outcome| r.record(&d, "r", outcome).0;
        let mut r = Rules::default();
        let p = paid(&mut r, Outcome::Owned);
        assert_eq!((p.cash, p.boards, p.rewards.clone()), (100, 1000, vec!["own_reward".to_string()]));
        let p = paid(&mut r, Outcome::Owned);
        assert_eq!(p, Paid::default(), "Owned -> Owned pays nothing");
        let p = paid(&mut r, Outcome::Killed);
        assert_eq!((p.cash, p.boards), (50, 500), "Owned -> Killed pays Kill");
        assert_eq!(r.level("r"), Level::Killed);
        assert_eq!(paid(&mut r, Outcome::Killed), Paid::default(), "Killed -> anything pays nothing");
        assert_eq!(paid(&mut r, Outcome::Owned), Paid::default());
        assert_eq!(r.level("r"), Level::Killed);
        assert_eq!(r.stats, Stats { own_boards: 1000, kill_boards: 500, cash: 150, cash_earned: 150, ..Stats::default() });

        let mut r = Rules::default();
        let p = paid(&mut r, Outcome::Killed);
        assert_eq!((p.cash, p.boards), (150, 1500), "not done -> Killed pays Own and Kill");
        let mut r = Rules::default();
        assert_eq!(paid(&mut r, Outcome::Abandoned), Paid::default());
        assert_eq!(r, Rules::default(), "an abandoned result records nothing");
    }

    #[test]
    fn a_disabled_row_is_never_activated() {
        let d = data_with(vec![], &[("g", &["off", "on"])], vec![("off", Row { disabled: true, ..row((0, 0), (0, 0)) }), ("on", row((0, 0), (0, 0)))]);
        let mut r = Rules::default();
        assert!(!r.activate_challenge(&d, "off"));
        assert_eq!(r.activate_group(&d, "g"), ["on"]);
        assert_eq!(r.active, ["on"]);
        assert_eq!(r.new, ["on"]);
    }

    #[test]
    fn an_owned_row_is_not_reactivated() {
        let d = data_with(vec![], &[], vec![("r", row((0, 0), (0, 0)))]);
        let mut r = Rules::default();
        r.record(&d, "r", Outcome::Owned);
        assert!(!r.activate_challenge(&d, "r"));
        assert!(r.active.is_empty());
    }

    #[test]
    fn a_finished_lot_is_recorded_and_paid() {
        let lot = Row { lot_rows: vec!["a".into(), "b".into()], ..row((300, 3000), (0, 0)) };
        let member = || Row { lot: Some("lot".into()), ..row((10, 0), (0, 0)) };
        let d = data_with(vec![], &[], vec![("lot", lot), ("a", member()), ("b", member())]);
        let mut r = Rules::default();
        let (p, _) = r.record(&d, "a", Outcome::Owned);
        assert_eq!(p.lot, None);
        let (p, _) = r.record(&d, "b", Outcome::Owned);
        assert_eq!(p.lot.as_deref(), Some("lot"));
        assert_eq!((p.cash, p.boards), (310, 3000), "the last activity's Own and the lot's Own");
        assert_eq!(r.level("lot"), Level::Owned);
    }

    #[test]
    fn effin_jordan_at_the_80th_kill() {
        let rows: Vec<(String, Row)> = (0..81).map(|n| (format!("r{n}"), row((0, 0), (0, 0)))).collect();
        let d = CareerData { rows: rows.into_iter().collect(), ..CareerData::default() };
        let mut r = Rules::default();
        for n in 0..79 {
            assert!(r.record(&d, &format!("r{n}"), Outcome::Killed).0.achievements.is_empty());
        }
        assert_eq!(r.record(&d, "r79", Outcome::Killed).0.achievements, [EFFIN_JORDAN]);
        assert!(r.record(&d, "r80", Outcome::Killed).0.achievements.is_empty(), "granted once");
        assert_eq!(r.achievements, [EFFIN_JORDAN]);
    }

    #[test]
    fn boards_reevaluate_kind_0_and_cash_reevaluates_kinds_4_and_5() {
        let d = data_with(
            vec![action("boards", false, &[(0, "", 1000)]), action("cash", false, &[(5, "", 100)]), action("earned", false, &[(4, "", 100)])],
            &[],
            vec![("r", row((0, 1000), (0, 0))), ("c", row((100, 0), (0, 0)))],
        );
        let mut r = Rules::default();
        for name in ["boards", "cash", "earned"] {
            assert!(r.activate_action(&d, name).is_none());
        }
        let (_, fired) = r.record(&d, "r", Outcome::Owned);
        assert_eq!(fired.iter().map(|f| f.action.as_str()).collect::<Vec<_>>(), ["boards"]);
        let (_, fired) = r.record(&d, "c", Outcome::Owned);
        assert_eq!(fired.iter().map(|f| f.action.as_str()).collect::<Vec<_>>(), ["earned", "cash"], "front to back, newest first");
        assert!(r.actions_active.is_empty());
    }

    #[test]
    fn branding_level_10_reads_board_sales() {
        let d = data_with(vec![action("branding_level_10_complete", false, &[(5, "", 250_000)])], &[], vec![]);
        let held = |r: &Rules| r.holds(&d, d.action("branding_level_10_complete").unwrap());
        assert!(!held(&rules_with(0, 250_000, 250_000)), "$250,000 cash does not reach it");
        assert!(held(&rules_with(250_000, 0, 0)), "250,000 boards do");
    }

    #[test]
    fn root_fires_only_unfired_flagged_actions() {
        let d = data_with(vec![action("starting_action", true, &[]), action("other", false, &[])], &[], vec![]);
        let mut r = Rules::default();
        assert_eq!(r.root(&d).iter().map(|f| f.action.as_str()).collect::<Vec<_>>(), ["starting_action"]);
        assert!(r.root(&d).is_empty(), "fired once per career");
    }

    #[test]
    fn spend_floors_at_zero() {
        let mut r = rules_with(0, 30, 30);
        r.spend(50);
        assert_eq!((r.stats.cash, r.stats.cash_earned), (0, 30));
    }

    #[test]
    fn a_result_takes_the_row_off_the_active_list() {
        let d = data_with(vec![], &[], vec![("r", row((0, 0), (0, 0)))]);
        let mut r = Rules::default();
        assert!(r.activate_challenge(&d, "r"));
        r.record(&d, "r", Outcome::Abandoned);
        assert_eq!(r.active, ["r"], "a lost or abandoned row stays active (career_flow Q6)");
        r.record(&d, "r", Outcome::Owned);
        assert!(r.active.is_empty());
    }

    /// Review Focus 5.
    #[test]
    fn a_replayed_owned_row_pays_and_fires_nothing() {
        let d = data_with(vec![action("after", false, &[(2, "r", 0), (3, "", 0)])], &[], vec![("r", row((100, 1000), (0, 0)))]);
        let mut r = Rules::default();
        r.activate_challenge(&d, "r");
        r.activate_action(&d, "after");
        r.record(&d, "r", Outcome::Owned);
        let (active, new, armed) = (r.active.clone(), r.new.clone(), r.actions_active.clone());
        let (paid, fired) = r.record(&d, "r", Outcome::Owned);
        assert_eq!(paid, Paid::default());
        assert!(fired.is_empty());
        assert_eq!((r.active.clone(), r.new.clone(), r.actions_active.clone()), (active, new, armed));
    }
}
