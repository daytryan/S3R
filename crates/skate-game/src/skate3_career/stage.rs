//! What a staged test build builds (spec 8, 14). Everything here is `Not retail (staged builds only)`: retail ships
//! the whole career, a stage build hides the rows and worlds it does not have yet and holds the steps that lead there.

use std::collections::BTreeSet;

use super::core::Step;
use super::data::CareerData;
use super::save::{Held, HeldStep};

pub const STOP_NOTICE: &str = "#Story mode test build: stage 1 ends here. The next part of the story arrives in stage 2.";

/// Stage 1's rows: the five lessons, the school teleports and Shingo's turn in University (Task 3's list), plus the
/// school's way back in.
pub const STAGE_1_ROWS: [&str; 15] = [
    "tut_sksc_01", "tut_sksc_02", "tut_sksc_03", "tut_sksc_04", "tut_sksc_06", "tele_sksc_01", "tele_sksc_02",
    "tele_sksc_03", "tele_sksc_04", "tele_school_to_world", "ftl_univ_02", "photo_univ_08", "film_univ_05", "ftl_univ_03",
    "tele_world_to_school",
];
pub const STAGE_1_WORLDS: [&str; 2] = ["dist_skateschool", "dist_university"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// None: every row and world (later stages and tests).
    rows: Option<BTreeSet<&'static str>>,
    worlds: Option<BTreeSet<&'static str>>,
}

impl Built {
    #[must_use]
    pub fn stage_1() -> Self {
        Self { rows: Some(STAGE_1_ROWS.into_iter().collect()), worlds: Some(STAGE_1_WORLDS.into_iter().collect()) }
    }

    #[must_use]
    pub fn everything() -> Self {
        Self { rows: None, worlds: None }
    }

    #[must_use]
    pub fn world_built(&self, world: &str) -> bool {
        self.worlds.as_ref().is_none_or(|w| w.contains(world))
    }

    /// A row whose mode or world is not built: no marker, box, trigger or Challenge Map entry.
    #[must_use]
    pub fn hides(&self, d: &CareerData, row: &str) -> bool {
        let Some(rows) = &self.rows else { return false };
        !rows.contains(row) || d.row(row).is_some_and(|r| !self.world_built(&r.world))
    }

    /// Every site of the reward is a hidden row or the stategraph of one (spec 8): the item is open now, since the
    /// build cannot earn it.
    #[must_use]
    pub fn item_open_early(&self, d: &CareerData, reward: &str) -> bool {
        let Some(sites) = d.reward_sites.get(reward).filter(|s| !s.is_empty()) else { return false };
        sites.iter().all(|site| match site.split_once(':') {
            Some(("row", row)) => self.hides(d, row),
            Some(("graph", graph)) => {
                let rows: Vec<&str> = d
                    .actions
                    .iter()
                    .filter(|a| a.stategraph == graph)
                    .flat_map(|a| a.conditions.iter().filter(|c| c.0 == 2).map(|c| c.1.as_str()))
                    .collect();
                !rows.is_empty() && rows.iter().all(|r| self.hides(d, r))
            }
            _ => false,
        })
    }

    /// The step leads into what this build lacks: a teleport into an unbuilt world, or a hidden row's poster.
    #[must_use]
    pub fn leads_out(&self, d: &CareerData, step: &Step) -> bool {
        match step {
            Step::Teleport { world, .. } => !self.world_built(world),
            Step::Poster(row) => self.hides(d, row),
            _ => false,
        }
    }
}

/// Holds a sequence from its first step that leads out (spec 14): that step and every later one of the same sequence
/// go into the save's `held`, in order.
#[derive(Default)]
pub struct Holder {
    holding: bool,
}

impl Holder {
    /// True when `step` is held (recorded in `held`) instead of performed.
    pub fn hold(&mut self, built: &Built, d: &CareerData, step: &Step, held: &mut Option<Held>) -> bool {
        self.holding |= built.leads_out(d, step);
        if !self.holding {
            return false;
        }
        let kept = match step {
            Step::Teleport { location, world } => Some(HeldStep::Teleport { location: location.clone(), world: world.clone() }),
            Step::Present { graph, state } => Some(HeldStep::Present(format!("{graph}/{state}"))),
            Step::Poster(row) => Some(HeldStep::Poster(row.clone())),
            // The autosave still runs, as retail's would (spec 14); screens of the chain are not held.
            Step::Autosave | Step::RewardSummary(_) | Step::Milestones(_) | Step::Teammates(_) | Step::OnlinePopup => None,
        };
        let Some(kept) = kept else { return false };
        held.get_or_insert_with(|| Held { steps: Vec::new(), notice_shown: false }).steps.push(kept);
        true
    }

    /// The sequence ended (the core went idle).
    pub fn end(&mut self) {
        self.holding = false;
    }
}

/// The stop notice is due: something is held and the notice has not shown yet.
#[must_use]
pub fn notice_due(held: Option<&Held>) -> bool {
    held.is_some_and(|h| !h.steps.is_empty() && !h.notice_shown)
}

#[cfg(test)]
mod tests {
    use super::super::data::{Action, Row};
    use super::*;

    fn data() -> CareerData {
        let mut d = CareerData::default();
        for (name, world) in [
            ("ftl_univ_03", "dist_university"),
            ("film_univ_09", "dist_startpark_progression"),
            ("tele_world_to_prog_park", "dist_university"),
            ("tele_prog_park_to_world", "dist_startpark_progression"),
            ("ftl_univ_01", "dist_university"),
        ] {
            d.rows.insert(name.into(), Row { world: world.into(), ..Row::default() });
        }
        d.reward_sites.insert("reda".into(), vec!["row:ftl_univ_03".into()]);
        d.reward_sites.insert("rob_dyrdek".into(), vec!["row:ftl_univ_01".into()]);
        d.reward_sites.insert("milestone_01".into(), vec!["graph:milestone_1".into()]);
        d.reward_sites.insert("late".into(), vec!["graph:ftl_univ_01".into()]);
        d.actions.push(Action {
            name: "ftl_univ_01_complete".into(),
            key: 1,
            auto: false,
            conditions: vec![(2, "ftl_univ_01".into(), 0)],
            stategraph: "ftl_univ_01".into(),
        });
        d.actions.push(Action {
            name: "milestone_01_complete".into(),
            key: 2,
            auto: false,
            conditions: vec![(5, "board_sales".into(), 15_000)],
            stategraph: "milestone_1".into(),
        });
        d
    }

    #[test]
    fn stage_one_hides_film_univ_09_and_the_prog_park_teleports() {
        let (b, d) = (Built::stage_1(), data());
        for row in ["film_univ_09", "tele_world_to_prog_park", "tele_prog_park_to_world"] {
            assert!(b.hides(&d, row), "{row}");
        }
        assert!(!b.hides(&d, "ftl_univ_03"));
        assert!(!Built::everything().hides(&d, "film_univ_09"));
    }

    #[test]
    fn an_item_earned_only_in_a_later_stage_is_open() {
        let (b, d) = (Built::stage_1(), data());
        assert!(b.item_open_early(&d, "rob_dyrdek"), "ftl_univ_01 is not built");
        assert!(b.item_open_early(&d, "late"), "the graph of a hidden row");
        assert!(!b.item_open_early(&d, "milestone_01"), "board sales are earned in any stage");
        assert!(!Built::everything().item_open_early(&d, "rob_dyrdek"));
    }

    #[test]
    fn reda_is_not_open_early() {
        assert!(!Built::stage_1().item_open_early(&data(), "reda"), "ftl_univ_03 is built");
    }

    #[test]
    fn the_first_step_out_holds_the_rest_of_its_sequence() {
        let (b, d) = (Built::stage_1(), data());
        let mut held = None;
        let mut h = Holder::default();
        let present = Step::Present { graph: "ftl_univ_03".into(), state: "reda".into() };
        assert!(!h.hold(&b, &d, &present, &mut held), "Shingo's turn plays");
        let tele = Step::Teleport { location: "tele_chan_to_start_prog_dest_locator_01".into(), world: "dist_startpark_progression".into() };
        assert!(h.hold(&b, &d, &tele, &mut held));
        assert!(h.hold(&b, &d, &Step::Present { graph: "g".into(), state: "s".into() }, &mut held));
        assert!(!h.hold(&b, &d, &Step::Autosave, &mut held), "the autosave runs");
        assert!(h.hold(&b, &d, &Step::Poster("film_univ_09".into()), &mut held));
        let steps = &held.as_ref().unwrap().steps;
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2], HeldStep::Poster("film_univ_09".into()));
        assert!(notice_due(held.as_ref()));
        held.as_mut().unwrap().notice_shown = true;
        assert!(!notice_due(held.as_ref()));
        h.end();
        assert!(!h.hold(&b, &d, &present, &mut held));
    }
}
