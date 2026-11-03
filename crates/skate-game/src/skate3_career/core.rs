//! Everything after a challenge, in retail's order (spec 5, career_flow Q2): the result, the career script's graphs,
//! the reward chain, the deferred presentation groups, the autosave, popup 306 and only then a pending auto sign-up.
//! Pure: the glue shows each [`Step`] and calls [`Core::done`] when it closes. Not tested in play.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::data::CareerData;
use super::progression::{Effect, Runner};
use super::rules::{Outcome, Rules};
use super::save::{Best, Held};

/// What else the game is doing, which holds the sequence.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ready {
    pub challenge_running: bool,
    /// A teleport or world load is pending (`[sim+0xc8]->vt[7]`, career_flow Q2 step 7a, INFERRED).
    pub transition_pending: bool,
    /// Another screen is up (a Skate 101, a popup the script raised).
    pub screen_open: bool,
}

/// The reward summary's content (career_flow Q2 step 7b).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub cash: u64,
    pub boards: u64,
    /// The Kill share of `boards` (R+0x4C; the rest is the Own share, R+0x48).
    pub kill_boards: u64,
    pub items: Vec<String>,
}

impl Summary {
    fn has_content(&self) -> bool {
        self.cash > 0 || self.boards > 0 || !self.items.is_empty()
    }
}

/// One thing for the glue to show or do; the sequence waits for [`Core::done`].
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Screen 109 unlockoverlay.
    RewardSummary(Summary),
    /// Screen 111, one page per milestone.
    Milestones(u32),
    /// Screen 112.
    Teammates(u32),
    /// A progression state's presentation events (the glue builds the group from `career.json`).
    Present { graph: String, state: String },
    Autosave,
    /// Popup 306, "Want to take your game online? ...".
    OnlinePopup,
    /// The sign-up poster of a pending auto sign-up (career_flow Q6).
    Poster(String),
    Teleport { location: String, world: String },
}

/// The reward chain's counters (RewardTracker, saved: career_flow Q4 item 7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub milestones: u32,
    pub teammates: u32,
    /// The milestone unlocks (`UnlockProgressionReward(name, true)`, results+0x8).
    pub items: Vec<String>,
}

/// Where the career last was: the last completed row's World and Location (career_flow Q5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub world: Option<String>,
    pub map: String,
    pub location: Option<String>,
}

/// The career's live state.
pub struct CareerState {
    pub rules: Rules,
    pub runner: Runner,
    pub globals: serde_json::Value,
    pub chain: Chain,
    pub place: Option<Place>,
    /// Seconds played (career_flow Q2 step 11 reads it against 7200; the unit is untraced).
    pub playtime: f64,
    /// `career+0x68`: a new career's first auto sign-up is still to come.
    pub new_career: bool,
    /// `career+0x74`: popup 306 was shown.
    pub online_popup_shown: bool,
    /// Own the Spot discovery states by row (stage 3).
    pub discovery: BTreeMap<String, String>,
    pub bests: BTreeMap<String, Best>,
    pub skatefeed: Vec<String>,
    pub brand_tiers: Vec<String>,
    pub teammates: u32,
    pub held: Option<Held>,
    pub data: Arc<CareerData>,
}

impl CareerState {
    #[must_use]
    pub fn new(data: Arc<CareerData>) -> Self {
        Self {
            rules: Rules::default(),
            runner: Runner::new(),
            globals: serde_json::json!({}),
            chain: Chain::default(),
            place: None,
            playtime: 0.0,
            new_career: false,
            online_popup_shown: false,
            discovery: BTreeMap::new(),
            bests: BTreeMap::new(),
            skatefeed: Vec::new(),
            brand_tiers: Vec::new(),
            teammates: 0,
            held: None,
            data,
        }
    }
}

/// The sequence's own flags (career_flow 0: P+0x34..0x3c). None of it is saved, as retail's (Q4).
#[derive(Default)]
pub struct Core {
    /// P+0x34: a result's rewards are pending.
    pending_result: bool,
    summary: Summary,
    /// The reward chain is up (after the summary).
    chain_active: bool,
    /// The deferred groups, in queue order.
    deferred: VecDeque<Step>,
    /// P+0x36.
    deferred_playing: bool,
    /// P+0x37: the post-challenge sequence finished.
    post_finished: bool,
    /// P+0x38: popup 306 is open.
    popup_open: bool,
    /// The running graph's own presentation groups and its teleports, which come out at once.
    immediate: VecDeque<Step>,
    /// The step the glue is showing, and whether it is a running graph's own group (`presentationeventgroupfinished`).
    open: Option<(Step, bool)>,
    pending: Option<String>,
    /// P+0x3c: an auto sign-up was just issued.
    signup_fresh: bool,
}

impl Core {
    /// A challenge ended (career_flow Q2 steps 2-6): a lost or abandoned one only finishes the sequence (step 4);
    /// Owned or Killed pays, records, keeps the row's World and Location as the career's place (step 5) and queues
    /// the stategraphs of the actions it fired.
    pub fn finished(&mut self, row: &str, outcome: Outcome, state: &mut CareerState) {
        if outcome == Outcome::Abandoned {
            self.post_finished = true;
            return;
        }
        let d = state.data.clone();
        let (paid, fired) = state.rules.record(&d, row, outcome);
        for f in fired {
            state.runner.queue(f);
        }
        self.summary.cash += paid.cash;
        self.summary.boards += paid.boards;
        self.summary.kill_boards += paid.kill_boards;
        self.summary.items.extend(paid.rewards);
        if let Some(r) = d.row(row).filter(|r| !r.location.is_empty()) {
            let map = d.worlds.get(&r.world).cloned().flatten().unwrap_or_default();
            state.place = Some(Place { world: Some(r.world.clone()), map, location: Some(r.location.clone()) });
        }
        self.pending_result = true;
    }

    /// What the career script returned: rewards to the summary or the chain, presentation groups and teleports to
    /// their steps, an auto sign-up to the queue. The rest is the glue's (screens, achievements, the feed).
    pub fn effects(&mut self, effects: Vec<Effect>, state: &mut CareerState) {
        for effect in effects {
            match effect {
                Effect::Reward { name, milestone: false } => self.summary.items.push(name),
                Effect::Reward { name, milestone: true } => state.chain.items.push(name),
                Effect::Milestone => state.chain.milestones += 1,
                Effect::Teammate => state.chain.teammates += 1,
                Effect::Events { graph, state: s, deferred: true } => self.deferred.push_back(Step::Present { graph, state: s }),
                Effect::Events { graph, state: s, deferred: false } => self.immediate.push_back(Step::Present { graph, state: s }),
                Effect::Teleport { location, world } => self.immediate.push_back(Step::Teleport { location, world }),
                Effect::AutoSignUp(row) => self.set_auto_signup(&row),
                _ => {}
            }
        }
    }

    /// Ticks the runner and takes its effects; answers them all, for the glue's screens.
    pub fn pump(&mut self, state: &mut CareerState) -> Vec<Effect> {
        let d = state.data.clone();
        let effects = state.runner.tick(&mut state.rules, &mut state.globals, &d);
        self.take(effects, state)
    }

    /// Sends `event` (`hudcomplete`, `hudyes`, `hudno`) to the running graph and takes its effects.
    pub fn reply(&mut self, event: &str, state: &mut CareerState) -> Vec<Effect> {
        let d = state.data.clone();
        let effects = state.runner.send(event, &mut state.rules, &mut state.globals, &d);
        self.take(effects, state)
    }

    /// Takes `effects`, with what the career photo's load answers. Retail posts `challengeteambrandimagecomplete`
    /// when `LoadChallengeCareerPhoto`'s image is in (7 photo graphs wait on it; h_6CCA68911E610FC8 compares its
    /// RequestID, nil on both sides here). Not retail (story mode stage 1 build): no billboard photo is drawn, so the
    /// load is done at once.
    fn take(&mut self, mut effects: Vec<Effect>, state: &mut CareerState) -> Vec<Effect> {
        let mut i = 0;
        while i < effects.len() {
            if effects[i] == Effect::CareerPhoto(true) {
                let d = state.data.clone();
                let more = state.runner.send(PHOTO_LOADED, &mut state.rules, &mut state.globals, &d);
                effects.extend(more);
            }
            i += 1;
        }
        self.effects(effects.clone(), state);
        effects
    }

    fn show(&mut self, step: Step, immediate: bool) -> Option<Step> {
        self.open = Some((step.clone(), immediate));
        Some(step)
    }

    /// The next step, or `None` while one is open or the sequence waits.
    pub fn next(&mut self, ready: &Ready, state: &mut CareerState) -> Option<Step> {
        if self.open.is_some() || ready.screen_open {
            return None;
        }
        // The running graph waits on its own group; a teleport it asked for goes at once.
        if let Some(step) = self.immediate.pop_front() {
            let immediate = matches!(step, Step::Present { .. });
            return self.show(step, immediate);
        }
        // The reward chain follows the A presses (step 8): milestones, teammates, then its end (0x827CD298, step 9).
        if self.chain_active {
            if state.chain.milestones > 0 {
                let pages = std::mem::take(&mut state.chain.milestones);
                return self.show(Step::Milestones(pages), false);
            }
            if state.chain.teammates > 0 {
                let pages = std::mem::take(&mut state.chain.teammates);
                return self.show(Step::Teammates(pages), false);
            }
            self.chain_active = false;
            self.post_finished = true;
            if self.deferred.is_empty() || self.deferred_playing {
                return self.show(Step::Autosave, false);
            }
            self.deferred_playing = true;
        }
        // Step 7a: the progression graphs must have finished and no transition be pending.
        if ready.challenge_running || ready.transition_pending || state.runner.busy() {
            return None;
        }
        if self.pending_result {
            self.pending_result = false;
            if self.summary.has_content() {
                self.chain_active = true;
                let summary = std::mem::take(&mut self.summary);
                return self.show(Step::RewardSummary(summary), false);
            }
            if self.deferred.is_empty() {
                // Step 7d.
                self.post_finished = true;
                return self.show(Step::Autosave, false);
            }
            // Step 7c.
            self.deferred_playing = true;
        }
        // Step 10: the deferred groups in queue order, then the autosave.
        if self.deferred_playing {
            if let Some(step) = self.deferred.pop_front() {
                return self.show(step, false);
            }
            self.deferred_playing = false;
            return self.show(Step::Autosave, false);
        }
        // Step 11: popup 306 once per career, past 7200 of play.
        if self.post_finished {
            self.post_finished = false;
            if !state.online_popup_shown && state.playtime > 7200.0 {
                state.online_popup_shown = true;
                self.popup_open = true;
                return self.show(Step::OnlinePopup, false);
            }
        }
        // Step 14: the new career's first auto sign-up autosaves on the next idle frame; career+0x68 clears first.
        if self.signup_fresh {
            self.signup_fresh = false;
            if state.new_career {
                state.new_career = false;
                return self.show(Step::Autosave, false);
            }
        }
        // Step 13: only then the pending auto sign-up's poster.
        if self.idle()
            && let Some(row) = self.pending.clone()
        {
            return self.show(Step::Poster(row), false);
        }
        None
    }

    /// The glue closed `step`. A running graph's own group sends `presentationeventgroupfinished` to it.
    pub fn done(&mut self, step: &Step, state: &mut CareerState) -> Vec<Effect> {
        let Some((open, immediate)) = self.open.take() else { return Vec::new() };
        debug_assert_eq!(&open, step, "the glue closed a step it was not shown");
        match step {
            Step::OnlinePopup => self.popup_open = false,
            // Accepted or declined, state 1 clears the pending auto sign-up (career_flow Q6).
            Step::Poster(_) => self.pending = None,
            // Screen 111 read them while it was up (GetMilestoneUnlockInfo 0x825B2F60).
            Step::Milestones(_) => state.chain.items.clear(),
            Step::Present { .. } if immediate => return self.reply("presentationeventgroupfinished", state),
            _ => {}
        }
        Vec::new()
    }

    /// `SignUp.SetAutoSignUp` or the new career's first sign-up (SU state 13).
    pub fn set_auto_signup(&mut self, row: &str) {
        self.pending = Some(row.to_owned());
        self.signup_fresh = true;
    }

    /// A mode change drops a pending auto sign-up (mode exit 0x82D527F0).
    pub fn drop_pending(&mut self) {
        self.pending = None;
        self.signup_fresh = false;
    }

    #[must_use]
    pub fn pending_signup(&self) -> Option<&str> {
        self.pending.as_deref()
    }

    /// 0x82D53058's progression half (0x827CFAE0): nothing deferred, no sequence, no popup 306.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.deferred.is_empty()
            && !self.deferred_playing
            && !self.post_finished
            && !self.popup_open
            && !self.pending_result
            && !self.chain_active
            && self.open.is_none()
            && self.immediate.is_empty()
    }
}

/// The career photo's load is done (TeamBrand, retail's message name).
pub const PHOTO_LOADED: &str = "challengeteambrandimagecomplete";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_career::data::{Action, Event, Payout, Row, State, StateGraph};
    use crate::skate3_career::rules::Level;

    const LUA: &str = r#"
        function after_paid(self, m, v, s, g)
            Unlock.MilestoneEarned()
            Unlock.UnlockTeammate()
            Unlock.UnlockProgressionReward("milestone_board", true)
            ChallengeStatus.ActivateChallenge("next_row", false)
            Events.DeferEventsAndChangeState("tail")
        end
        function tail(self, m, v, s, g) SignUp.SetAutoSignUp("next_row") ChangeState("complete") end
        function quiet(self, m, v, s, g) ChangeState("complete") end
    "#;

    /// `paid_row` pays and fires `paid_row_complete`, whose graph earns a milestone and a teammate, defers one group
    /// and sets an auto sign-up of `next_row`; `free_row` pays nothing and fires a graph with nothing to show.
    fn data() -> Arc<CareerData> {
        let state = |handlers: &[(&str, &str)], events: usize| State {
            handlers: handlers.iter().map(|(m, h)| ((*m).to_owned(), (*h).to_owned())).collect(),
            events: (0..events).map(|_| Event { kind: 5, args: vec!["nis".into()], flags: 0 }).collect(),
        };
        let graph = |states: Vec<(&str, State)>| StateGraph { initial: "initial".into(), states: states.into_iter().map(|(n, s)| (n.to_owned(), s)).collect() };
        let row = |cash, boards| Row {
            own: Payout { cash, boards, rewards: vec![] },
            world: "dist_university".into(),
            location: "a_locator".into(),
            ..Row::default()
        };
        Arc::new(CareerData {
            lua: LUA.into(),
            actions: vec![
                Action { name: "paid_row_complete".into(), conditions: vec![(2, "paid_row".into(), 0)], stategraph: "paid_graph".into(), ..Action::default() },
                Action { name: "free_row_complete".into(), conditions: vec![(2, "free_row".into(), 0)], stategraph: "free_graph".into(), ..Action::default() },
            ],
            stategraphs: [
                (
                    "paid_graph".to_owned(),
                    graph(vec![("initial", state(&[("stateenter", "after_paid")], 1)), ("tail", state(&[("stateenter", "tail")], 0)), ("complete", state(&[], 0))]),
                ),
                ("free_graph".to_owned(), graph(vec![("initial", state(&[("stateenter", "quiet")], 0)), ("complete", state(&[], 0))])),
            ]
            .into_iter()
            .collect(),
            rows: [("paid_row".to_owned(), row(100, 1000)), ("free_row".to_owned(), row(0, 0)), ("next_row".to_owned(), row(0, 0))].into_iter().collect(),
            worlds: [("dist_university".to_owned(), Some("maps/University.skate".to_owned()))].into_iter().collect(),
            ..CareerData::default()
        })
    }

    fn career(d: &Arc<CareerData>) -> CareerState {
        let mut state = CareerState::new(d.clone());
        for row in ["paid_row", "free_row"] {
            state.rules.activate_challenge(d, row);
        }
        for action in ["paid_row_complete", "free_row_complete"] {
            state.rules.activate_action(d, action);
        }
        state
    }

    /// Pumps the runner and shows each step, closing it at once, until nothing more comes; the steps in order.
    fn drive(core: &mut Core, state: &mut CareerState, ready: &Ready) -> Vec<Step> {
        let mut steps = Vec::new();
        for _ in 0..100 {
            core.pump(state);
            let Some(step) = core.next(ready, state) else {
                if !state.runner.busy() && core.next(ready, state).is_none() {
                    return steps;
                }
                continue;
            };
            core.done(&step, state);
            steps.push(step);
        }
        panic!("the sequence never settled: {steps:?}");
    }

    fn present(graph: &str, state: &str) -> Step {
        Step::Present { graph: graph.into(), state: state.into() }
    }

    #[test]
    fn an_owned_result_runs_the_retail_order() {
        let d = data();
        let mut state = career(&d);
        state.playtime = 7300.0;
        let mut core = Core::default();
        core.finished("paid_row", Outcome::Owned, &mut state);
        // Nothing comes while a transition is pending or the runner is busy.
        assert_eq!(core.next(&Ready { transition_pending: true, ..Ready::default() }, &mut state), None);
        core.pump(&mut state);
        let steps = drive(&mut core, &mut state, &Ready::default());
        assert_eq!(
            steps,
            [
                Step::RewardSummary(Summary { cash: 100, boards: 1000, kill_boards: 0, items: vec![] }),
                Step::Milestones(1),
                Step::Teammates(1),
                present("paid_graph", "initial"),
                Step::Autosave,
                Step::OnlinePopup,
                Step::Poster("next_row".into()),
            ]
        );
        assert!(core.idle());
        assert_eq!(state.rules.level("paid_row"), Level::Owned);
    }

    /// Screen 111 reads results+0x8 (GetMilestoneUnlockInfo 0x825B2F60) while it is up: the milestone unlocks stay
    /// until its page closes.
    #[test]
    fn the_milestone_unlocks_last_until_their_screen_closes() {
        let d = data();
        let mut state = career(&d);
        state.chain = Chain { milestones: 1, teammates: 0, items: vec!["cac_unlock_x".into()] };
        let mut core = Core { chain_active: true, ..Core::default() };
        let step = core.next(&Ready::default(), &mut state).unwrap();
        assert_eq!(step, Step::Milestones(1));
        assert_eq!(state.chain.items, ["cac_unlock_x"], "the screen reads them");
        core.done(&step, &mut state);
        assert!(state.chain.items.is_empty());
    }

    #[test]
    fn a_result_with_nothing_to_show_autosaves_at_once() {
        let d = data();
        let mut state = career(&d);
        let mut core = Core::default();
        core.finished("free_row", Outcome::Owned, &mut state);
        assert_eq!(drive(&mut core, &mut state, &Ready::default()), [Step::Autosave]);
    }

    #[test]
    fn a_lost_or_abandoned_result_runs_nothing() {
        let d = data();
        let mut state = career(&d);
        let mut core = Core::default();
        core.finished("paid_row", Outcome::Abandoned, &mut state);
        assert!(drive(&mut core, &mut state, &Ready::default()).is_empty());
        assert!(state.rules.active.contains(&"paid_row".to_owned()), "the row stays active (Q6)");
        assert_eq!(state.place, None);
        assert_eq!(state.rules.stats.cash, 0);
    }

    #[test]
    fn the_online_popup_shows_once() {
        let d = data();
        let mut state = career(&d);
        state.playtime = 9000.0;
        let mut core = Core::default();
        core.finished("free_row", Outcome::Owned, &mut state);
        assert_eq!(drive(&mut core, &mut state, &Ready::default()), [Step::Autosave, Step::OnlinePopup]);
        core.finished("paid_row", Outcome::Abandoned, &mut state);
        assert!(drive(&mut core, &mut state, &Ready::default()).is_empty(), "popup 306 is once per career");
    }

    #[test]
    fn a_world_and_location_are_kept_as_place() {
        let d = data();
        let mut state = career(&d);
        let mut core = Core::default();
        core.finished("free_row", Outcome::Owned, &mut state);
        assert_eq!(
            state.place,
            Some(Place { world: Some("dist_university".into()), map: "maps/University.skate".into(), location: Some("a_locator".into()) })
        );
    }

    #[test]
    fn an_auto_signup_waits_for_the_deferred_groups() {
        let d = data();
        let mut state = career(&d);
        let mut core = Core::default();
        core.finished("paid_row", Outcome::Owned, &mut state);
        core.pump(&mut state);
        let mut seen = Vec::new();
        while let Some(step) = core.next(&Ready::default(), &mut state) {
            if matches!(step, Step::Present { .. }) {
                assert_eq!(core.pending_signup(), Some("next_row"));
                assert!(!seen.iter().any(|s| matches!(s, Step::Poster(_))), "the poster came before the deferred group");
            }
            core.done(&step, &mut state);
            seen.push(step);
        }
        assert!(matches!(seen.last(), Some(Step::Poster(_))), "{seen:?}");
    }

    #[test]
    fn drop_pending_forgets_it() {
        let mut core = Core::default();
        core.set_auto_signup("next_row");
        core.drop_pending();
        assert_eq!(core.pending_signup(), None);
        let d = data();
        let mut state = career(&d);
        assert_eq!(core.next(&Ready::default(), &mut state), None);
    }

    #[test]
    fn a_new_career_signup_autosaves_on_the_next_idle_frame() {
        let d = data();
        let mut state = career(&d);
        state.new_career = true;
        let mut core = Core::default();
        core.set_auto_signup("paid_row");
        assert_eq!(core.next(&Ready::default(), &mut state), Some(Step::Autosave));
        assert!(!state.new_career, "career+0x68 clears before the poster");
        core.done(&Step::Autosave, &mut state);
        assert_eq!(core.next(&Ready::default(), &mut state), Some(Step::Poster("paid_row".into())));
    }
}
