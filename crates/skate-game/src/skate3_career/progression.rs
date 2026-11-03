//! The career script (spec 4): each fired action's stategraph runs retail's own progression Lua on a
//! `challenge::Machine`, one at a time (runner 0x82C1AF88, career_flow Q1). The natives act on the rules at once or
//! return [`Effect`]s, which the core and the glue perform; nothing here draws or teleports. Not tested in play.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use mlua::{Lua, MultiValue, Table};
use serde_json::{Value, json};

use super::data::CareerData;
use super::rules::{Fired, Level, Rules};
use crate::challenge::{Machine, STATE_KEY, Script};

/// What the career script asked of the game.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `Notifications101.Show101`: Skate 101 screen 37 (0x827d0040); the state waits for `hudcomplete`.
    Show101 { title: String, description: String },
    /// `Popup.ShowYesNoPopup`: Yes sends `hudyes`, No `hudno` (progression.lua 1265-1295).
    YesNo { title: String, description: String },
    /// `SignUp.SetAutoSignUp` (0x827c4c80).
    AutoSignUp(String),
    /// `Teleport.TeleportLocalToWorld` (order 4.B).
    Teleport { location: String, world: String },
    /// A state's presentation events (spec 5): `deferred` plays them later in the post-challenge sequence; else the
    /// state waits for `presentationeventgroupfinished`.
    Events { graph: String, state: String, deferred: bool },
    /// `Unlock.UnlockProgressionReward(name, milestone)` (0x82839958).
    Reward { name: String, milestone: bool },
    /// `Unlock.MilestoneEarned` (0x82839930).
    Milestone,
    /// `Unlock.UnlockTeammate` (0x82839A80).
    Teammate,
    /// `TeamBrand.UnlockTier(tier)`: the script passes the tier's name (`"0213_logo_level_10"`).
    BrandTier(String),
    /// `Achievement.GrantAchievement` (0x82839f00), the first time only.
    Achievement(String),
    /// `SkateFeed.SendMessage` (0x827c4b30).
    SkateFeed(String),
    /// `Progression.HideScreen` (true) / `UnhideScreen` (false).
    Screen(bool),
    /// `Discovery.AllowDiscovery` (0x82838a38).
    AllowDiscovery,
    /// `TeamBrand.LoadChallengeCareerPhoto` (true) / `UnloadChallengeCareerPhoto` (false).
    CareerPhoto(bool),
}

/// A native's request, applied in call order after the handler returns (none of them is read back inside a handler
/// except a row's completion level, which no handler changes).
enum Op {
    Effect(Effect),
    ActivateAction(String),
    ActivateChallenge(String),
    ActivateGroup(String),
    Deactivate(String),
    /// The state to change to, and the state whose events play (the handler's own).
    Execute { next: String, state: String },
    Defer { next: String, state: String },
    Achievement(String),
    Unsupported { native: String, state: String },
}

/// Between the natives and the runner.
#[derive(Default)]
struct Shared {
    ops: Vec<Op>,
    owned: Vec<String>,
    killed: Vec<String>,
}

type Cell = Arc<Mutex<Shared>>;

/// The step whose handler is running (`Machine::call` keeps it in the registry).
fn step(lua: &Lua) -> String {
    lua.named_registry_value::<String>(STATE_KEY).unwrap_or_default()
}

/// A native that only records its request.
fn native<A: mlua::FromLuaMulti>(lua: &Lua, cell: &Cell, op: impl Fn(A) -> Op + Send + 'static) -> mlua::Result<mlua::Function> {
    let cell = cell.clone();
    lua.create_function(move |_, args: A| {
        cell.lock().unwrap().ops.push(op(args));
        Ok(())
    })
}

/// A native table; a member it does not have is recorded as unsupported and does nothing (spec 4: the retail default
/// is nil).
fn table(lua: &Lua, cell: &Cell, name: &'static str, members: Vec<(&str, mlua::Function)>) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (key, f) in members {
        t.set(key, f)?;
    }
    let meta = lua.create_table()?;
    let cell = cell.clone();
    meta.set(
        "__index",
        lua.create_function(move |lua, (_, key): (Table, String)| {
            let cell = cell.clone();
            let native = format!("{name}.{key}");
            lua.create_function(move |lua, _: MultiValue| {
                cell.lock().unwrap().ops.push(Op::Unsupported { native: native.clone(), state: step(lua) });
                Ok(())
            })
        })?,
    )?;
    t.set_metatable(Some(meta))?;
    Ok(t)
}

/// The 23 natives of spec 4.
fn natives(lua: &Lua, cell: &Cell, d: &CareerData) -> mlua::Result<()> {
    let g = lua.globals();
    let effect = |e: fn(MultiValue) -> Effect| native(lua, cell, move |args: MultiValue| Op::Effect(e(args)));
    g.set("Actions", table(lua, cell, "Actions", vec![("ActivateAction", native(lua, cell, Op::ActivateAction)?)])?)?;
    let levels = cell.clone();
    let level = lua.create_function(move |_, name: String| {
        let shared = levels.lock().unwrap();
        // 0x82C1D988: 0 Owned, 1 Killed, 2 not done.
        let level = if shared.killed.contains(&name) {
            Level::Killed
        } else if shared.owned.contains(&name) {
            Level::Owned
        } else {
            Level::NotDone
        };
        Ok(level as i64)
    })?;
    let status = table(
        lua,
        cell,
        "ChallengeStatus",
        vec![
            // The boolean argument is ignored (order 2.2).
            ("ActivateChallenge", native(lua, cell, |(row, _): (String, MultiValue)| Op::ActivateChallenge(row))?),
            ("ActivateChallengeGroup", native(lua, cell, |(group, _): (String, MultiValue)| Op::ActivateGroup(group))?),
            ("DeactivateChallenge", native(lua, cell, |(row, _): (String, MultiValue)| Op::Deactivate(row))?),
            ("GetChallengeCompletionLevel", level),
        ],
    )?;
    // 0x827D0710.
    let completion = lua.create_table()?;
    completion.set("Owned", 0)?;
    completion.set("Killed", 1)?;
    status.raw_set("EChallengeCompletionLevel", completion)?;
    g.set("ChallengeStatus", status)?;
    g.set(
        "Unlock",
        table(
            lua,
            cell,
            "Unlock",
            vec![
                (
                    "UnlockProgressionReward",
                    native(lua, cell, |(name, milestone): (String, Option<bool>)| {
                        Op::Effect(Effect::Reward { name, milestone: milestone.unwrap_or(false) })
                    })?,
                ),
                ("MilestoneEarned", effect(|_| Effect::Milestone)?),
                ("UnlockTeammate", effect(|_| Effect::Teammate)?),
            ],
        )?,
    )?;
    g.set(
        "TeamBrand",
        table(
            lua,
            cell,
            "TeamBrand",
            vec![
                ("UnlockTier", native(lua, cell, |tier: String| Op::Effect(Effect::BrandTier(tier)))?),
                ("LoadChallengeCareerPhoto", effect(|_| Effect::CareerPhoto(true))?),
                ("UnloadChallengeCareerPhoto", effect(|_| Effect::CareerPhoto(false))?),
            ],
        )?,
    )?;
    let events = |defer: bool| {
        let cell = cell.clone();
        lua.create_function(move |lua, (next, _): (String, MultiValue)| {
            let state = step(lua);
            cell.lock().unwrap().ops.push(if defer { Op::Defer { next, state } } else { Op::Execute { next, state } });
            Ok(())
        })
    };
    g.set(
        "Events",
        table(lua, cell, "Events", vec![("ExecuteEventsAndChangeState", events(false)?), ("DeferEventsAndChangeState", events(true)?)])?,
    )?;
    g.set("SignUp", table(lua, cell, "SignUp", vec![("SetAutoSignUp", native(lua, cell, |row: String| Op::Effect(Effect::AutoSignUp(row)))?)])?)?;
    g.set(
        "Notifications101",
        table(
            lua,
            cell,
            "Notifications101",
            vec![("Show101", native(lua, cell, |(title, description): (String, String)| Op::Effect(Effect::Show101 { title, description }))?)],
        )?,
    )?;
    g.set(
        "Popup",
        table(
            lua,
            cell,
            "Popup",
            vec![("ShowYesNoPopup", native(lua, cell, |(title, description): (String, String)| Op::Effect(Effect::YesNo { title, description }))?)],
        )?,
    )?;
    g.set(
        "Teleport",
        table(
            lua,
            cell,
            "Teleport",
            vec![(
                "TeleportLocalToWorld",
                native(lua, cell, |(location, world, _): (String, String, MultiValue)| Op::Effect(Effect::Teleport { location, world }))?,
            )],
        )?,
    )?;
    g.set("Achievement", table(lua, cell, "Achievement", vec![("GrantAchievement", native(lua, cell, Op::Achievement)?)])?)?;
    g.set("SkateFeed", table(lua, cell, "SkateFeed", vec![("SendMessage", native(lua, cell, |id: String| Op::Effect(Effect::SkateFeed(id)))?)])?)?;
    g.set(
        "Progression",
        table(lua, cell, "Progression", vec![("HideScreen", effect(|_| Effect::Screen(true))?), ("UnhideScreen", effect(|_| Effect::Screen(false))?)])?,
    )?;
    g.set("Discovery", table(lua, cell, "Discovery", vec![("AllowDiscovery", effect(|_| Effect::AllowDiscovery)?)])?)?;
    // Attrib.RefSpec(Attrib.ClassName.challenges, row): the row's Name, GlobalData.Location and GlobalData.World.Name
    // (progression.lua 1287-1289).
    let rows: BTreeMap<String, (String, String)> = d.rows.iter().map(|(name, row)| (name.clone(), (row.location.clone(), row.world.clone()))).collect();
    let refspec = lua.create_function(move |lua, (_, name): (mlua::Value, String)| {
        let Some((location, world)) = rows.get(&name) else { return Ok(mlua::Value::Nil) };
        let record = lua.create_table()?;
        record.set("Name", name.as_str())?;
        let global = lua.create_table()?;
        global.set("Location", location.as_str())?;
        let w = lua.create_table()?;
        w.set("Name", world.as_str())?;
        global.set("World", w)?;
        record.set("GlobalData", global)?;
        Ok(mlua::Value::Table(record))
    })?;
    let attrib = table(lua, cell, "Attrib", vec![("RefSpec", refspec)])?;
    let classes = lua.create_table()?;
    classes.set("challenges", "challenges")?;
    attrib.raw_set("ClassName", classes)?;
    g.set("Attrib", attrib)?;
    Ok(())
}

/// The stategraph that is running.
struct Running {
    graph: String,
    machine: Machine,
    cell: Cell,
    /// The state `ExecuteEventsAndChangeState` changes to once its group has finished.
    waiting: Option<String>,
}

impl Running {
    fn done(&self) -> bool {
        self.waiting.is_none() && (self.machine.state() == "complete" || self.machine.terminated())
    }
}

/// The one-at-a-time stategraph runner.
#[derive(Default)]
pub struct Runner {
    queue: VecDeque<Fired>,
    running: Option<Running>,
    unsupported: Vec<String>,
    /// Every row the script activated, with the stategraph that did (the order oracle reads it).
    activated: Vec<(String, String)>,
}

impl Runner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a fired action's stategraph behind the ones queued before it.
    pub fn queue(&mut self, fired: Fired) {
        self.queue.push_back(fired);
    }

    /// A stategraph is running (it has not reached `complete`) or one is queued.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.done()) || !self.queue.is_empty()
    }

    /// Natives the script called that this stage does not support, `"<native> in <graph>/<state>"`, once each, and
    /// the script errors that stopped a stategraph.
    #[must_use]
    pub fn unsupported(&self) -> &[String] {
        &self.unsupported
    }

    #[must_use]
    pub fn activated(&self) -> &[(String, String)] {
        &self.activated
    }

    fn report(&mut self, line: String) {
        if !self.unsupported.contains(&line) {
            bevy::log::warn!("Skate 3 career: {line}");
            self.unsupported.push(line);
        }
    }

    /// When idle, starts the queued stategraphs in order; each one's initial `stateenter` runs inside its start (update
    /// 0x82C1B1A8). Stops at the first that does not reach `complete` at once.
    pub fn tick(&mut self, rules: &mut Rules, globals: &mut Value, d: &CareerData) -> Vec<Effect> {
        let mut effects = Vec::new();
        loop {
            if self.running.as_ref().is_some_and(|r| !r.done()) {
                break;
            }
            self.running = None;
            let Some(fired) = self.queue.pop_front() else { break };
            if fired.stategraph.is_empty() {
                continue;
            }
            match self.start(&fired.stategraph, rules, globals, d, &mut effects) {
                Ok(running) => self.running = Some(running),
                Err(why) => self.report(why),
            }
        }
        effects
    }

    /// Delivers `event` (`hudcomplete`, `hudyes`, `hudno`, `presentationeventgroupfinished`) to the running stategraph.
    pub fn send(&mut self, event: &str, rules: &mut Rules, globals: &mut Value, d: &CareerData) -> Vec<Effect> {
        let mut effects = Vec::new();
        let Some(mut running) = self.running.take() else { return effects };
        let result = match running.waiting.take() {
            Some(next) if event == "presentationeventgroupfinished" => {
                self.drive(&mut running, rules, globals, d, &mut effects, |m| m.change_state(&next))
            }
            waiting => {
                running.waiting = waiting;
                // A message is an object in retail (the photo graphs read message.RequestID); nil would fault them.
                self.drive(&mut running, rules, globals, d, &mut effects, |m| m.send(event, &json!({})))
            }
        };
        match result {
            Ok(()) => self.running = Some(running),
            // The graph stops; the career goes on without it.
            Err(why) => self.report(why),
        }
        effects
    }

    fn start(&mut self, name: &str, rules: &mut Rules, globals: &mut Value, d: &CareerData, effects: &mut Vec<Effect>) -> Result<Running, String> {
        let graph = d.stategraphs.get(name).ok_or_else(|| format!("stategraph {name} is not in the export"))?;
        let steps = graph.states.iter().map(|(state, s)| (state.clone(), s.handlers.clone())).collect();
        let cell = Cell::default();
        let script = Script { name, source: &d.lua, steps, initial: &graph.initial, enter: "stateenter", exit: "stateexit" };
        let machine = Machine::new(script, &json!({ "Name": name }), |lua| natives(lua, &cell, d))?;
        let mut running = Running { graph: name.to_owned(), machine, cell, waiting: None };
        self.drive(&mut running, rules, globals, d, effects, Machine::start)?;
        Ok(running)
    }

    /// Runs `call` on the machine with the career's globals, then applies the natives' requests in order (a state
    /// change they make runs more handlers, whose requests follow).
    fn drive(
        &mut self,
        running: &mut Running,
        rules: &mut Rules,
        globals: &mut Value,
        d: &CareerData,
        effects: &mut Vec<Effect>,
        call: impl FnOnce(&mut Machine) -> Result<(), String>,
    ) -> Result<(), String> {
        running.machine.set_globalvars(globals)?;
        {
            let mut shared = running.cell.lock().unwrap();
            shared.owned.clone_from(&rules.owned);
            shared.killed.clone_from(&rules.killed);
        }
        let result = call(&mut running.machine).and_then(|()| self.apply(running, rules, d, effects));
        *globals = running.machine.globalvars();
        result
    }

    fn apply(&mut self, running: &mut Running, rules: &mut Rules, d: &CareerData, effects: &mut Vec<Effect>) -> Result<(), String> {
        loop {
            let ops = std::mem::take(&mut running.cell.lock().unwrap().ops);
            if ops.is_empty() {
                return Ok(());
            }
            for op in ops {
                let graph = &running.graph;
                let has_events =
                    |state: &str| d.stategraphs.get(graph).and_then(|g| g.states.get(state)).is_some_and(|s| !s.events.is_empty());
                match op {
                    Op::Effect(effect) => effects.push(effect),
                    // An action that fires inside a handler queues its graph behind the running one (0x82C1AF88).
                    Op::ActivateAction(name) => {
                        if let Some(fired) = rules.activate_action(d, &name) {
                            self.queue.push_back(fired);
                        }
                    }
                    Op::ActivateChallenge(row) => {
                        if rules.activate_challenge(d, &row) {
                            self.activated.push((row, running.graph.clone()));
                        }
                    }
                    Op::ActivateGroup(group) => {
                        for row in rules.activate_group(d, &group) {
                            self.activated.push((row, running.graph.clone()));
                        }
                    }
                    Op::Deactivate(row) => rules.deactivate_challenge(&row),
                    Op::Achievement(name) => {
                        if !rules.achievements.contains(&name) {
                            rules.achievements.push(name.clone());
                            effects.push(Effect::Achievement(name));
                        }
                    }
                    // 0x827d16f0: the current state's events play now and the state changes when they have finished;
                    // with none it changes at once (spec 5).
                    Op::Execute { next, state } => {
                        if has_events(&state) {
                            effects.push(Effect::Events { graph: graph.clone(), state, deferred: false });
                            running.waiting = Some(next);
                        } else {
                            running.machine.change_state(&next)?;
                        }
                    }
                    // 0x827d1970: the group goes on the deferred list and the state changes at once.
                    Op::Defer { next, state } => {
                        if has_events(&state) {
                            effects.push(Effect::Events { graph: graph.clone(), state, deferred: true });
                        }
                        running.machine.change_state(&next)?;
                    }
                    Op::Unsupported { native, state } => self.report(format!("{native} in {graph}/{state}")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_career::data::{Action, Event, State, StateGraph};

    /// A graph from `(state, [(message, handler)], events)` rows; its initial state is `initial`.
    fn graph(states: &[(&str, &[(&str, &str)], usize)]) -> StateGraph {
        StateGraph {
            initial: "initial".into(),
            states: states
                .iter()
                .map(|(name, handlers, events)| {
                    let handlers = handlers.iter().map(|(m, h)| ((*m).to_owned(), (*h).to_owned())).collect();
                    let events = (0..*events).map(|_| Event { kind: 5, args: vec!["nis".into()], flags: 0 }).collect();
                    ((*name).to_owned(), State { handlers, events })
                })
                .collect(),
        }
    }

    fn data(lua: &str, graphs: Vec<(&str, StateGraph)>, actions: &[(&str, &str)]) -> CareerData {
        CareerData {
            lua: lua.into(),
            stategraphs: graphs.into_iter().map(|(n, g)| (n.to_owned(), g)).collect(),
            actions: actions.iter().map(|(n, g)| Action { name: (*n).into(), stategraph: (*g).into(), ..Action::default() }).collect(),
            ..CareerData::default()
        }
    }

    fn fired(graph: &str) -> Fired {
        Fired { action: format!("{graph}_action"), stategraph: graph.into() }
    }

    #[test]
    fn one_stategraph_runs_at_a_time() {
        let lua = r#"
            function g1_enter(self, m, v, s, g) g.log = (g.log or "") .. "g1;" Actions.ActivateAction("third") Notifications101.Show101("T", "D") end
            function g1_done(self, m, v, s, g) ChangeState("complete") end
            function g2_enter(self, m, v, s, g) g.log = g.log .. "g2;" ChangeState("complete") end
            function g3_enter(self, m, v, s, g) g.log = g.log .. "g3;" ChangeState("complete") end
        "#;
        let d = data(
            lua,
            vec![
                ("g1", graph(&[("initial", &[("stateenter", "g1_enter"), ("hudcomplete", "g1_done")], 0), ("complete", &[], 0)])),
                ("g2", graph(&[("initial", &[("stateenter", "g2_enter")], 0), ("complete", &[], 0)])),
                ("g3", graph(&[("initial", &[("stateenter", "g3_enter")], 0), ("complete", &[], 0)])),
            ],
            &[("third", "g3")],
        );
        let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
        runner.queue(fired("g1"));
        runner.queue(fired("g2"));
        let effects = runner.tick(&mut rules, &mut globals, &d);
        assert_eq!(effects, [Effect::Show101 { title: "T".into(), description: "D".into() }]);
        assert!(runner.tick(&mut rules, &mut globals, &d).is_empty());
        assert_eq!(globals["log"], "g1;", "the second graph waits for the first");
        assert!(runner.busy());
        assert!(runner.send("hudcomplete", &mut rules, &mut globals, &d).is_empty());
        runner.tick(&mut rules, &mut globals, &d);
        assert_eq!(globals["log"], "g1;g2;g3;", "then the queue in order, the action fired inside a handler last");
        assert!(!runner.busy());
        assert!(runner.unsupported().is_empty(), "{:?}", runner.unsupported());
    }

    /// The 7 photo graphs (h_6CCA68911E610FC8): the load's answer compares message.RequestID with the native's.
    #[test]
    fn the_career_photo_answer_moves_the_photo_graph_on() {
        let lua = r#"
            function load(self, m, v, s, g) v.RequestID = TeamBrand.LoadChallengeCareerPhoto("photo_anyw_11") end
            function loaded(self, m, v, s, g) if m.RequestID == v.RequestID then g.done = true ChangeState("complete") end end
        "#;
        let d = data(
            lua,
            vec![("photo", graph(&[("initial", &[("stateenter", "load"), (super::super::core::PHOTO_LOADED, "loaded")], 0), ("complete", &[], 0)]))],
            &[],
        );
        let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
        runner.queue(fired("photo"));
        assert_eq!(runner.tick(&mut rules, &mut globals, &d), [Effect::CareerPhoto(true)]);
        runner.send(super::super::core::PHOTO_LOADED, &mut rules, &mut globals, &d);
        assert_eq!(globals["done"], true, "{:?}", runner.unsupported());
        assert!(!runner.busy());
    }

    #[test]
    fn show101_waits_for_hudcomplete_and_yesno_branches() {
        let lua = r#"
            function ask(self, m, v, s, g) Popup.ShowYesNoPopup("Q", "D") end
            function yes(self, m, v, s, g) ChangeState("yes") end
            function no(self, m, v, s, g) ChangeState("no") end
            function said_yes(self, m, v, s, g) g.branch = "yes" ChangeState("complete") end
            function said_no(self, m, v, s, g) g.branch = "no" ChangeState("complete") end
        "#;
        let d = data(
            lua,
            vec![(
                "ask",
                graph(&[
                    ("initial", &[("stateenter", "ask"), ("hudyes", "yes"), ("hudno", "no")], 0),
                    ("yes", &[("stateenter", "said_yes")], 0),
                    ("no", &[("stateenter", "said_no")], 0),
                    ("complete", &[], 0),
                ]),
            )],
            &[],
        );
        for answer in ["hudyes", "hudno"] {
            let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
            runner.queue(fired("ask"));
            assert_eq!(runner.tick(&mut rules, &mut globals, &d), [Effect::YesNo { title: "Q".into(), description: "D".into() }]);
            runner.send("hudcomplete", &mut rules, &mut globals, &d);
            assert!(runner.busy(), "the popup state only takes yes or no");
            runner.send(answer, &mut rules, &mut globals, &d);
            assert_eq!(globals["branch"], answer.trim_start_matches("hud"));
            assert!(!runner.busy());
        }
    }

    #[test]
    fn execute_waits_and_defer_changes_at_once() {
        let lua = r#"
            function exec(self, m, v, s, g) Events.ExecuteEventsAndChangeState("b") end
            function defer(self, m, v, s, g) Events.DeferEventsAndChangeState("b") end
            function b(self, m, v, s, g) g.b = true ChangeState("complete") end
        "#;
        let run = |enter: &str, events: usize| {
            let d = data(
                lua,
                vec![("p", graph(&[("initial", &[("stateenter", enter)], events), ("b", &[("stateenter", "b")], 0), ("complete", &[], 0)]))],
                &[],
            );
            let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
            runner.queue(fired("p"));
            let effects = runner.tick(&mut rules, &mut globals, &d);
            (effects, globals, runner, rules, d)
        };
        let (effects, mut globals, mut runner, mut rules, d) = run("exec", 1);
        assert_eq!(effects, [Effect::Events { graph: "p".into(), state: "initial".into(), deferred: false }]);
        assert_eq!(globals.get("b"), None, "the state waits for the group");
        assert!(runner.busy());
        runner.send("presentationeventgroupfinished", &mut rules, &mut globals, &d);
        assert_eq!(globals["b"], true);
        assert!(!runner.busy());

        let (effects, globals, runner, ..) = run("defer", 1);
        assert_eq!(effects, [Effect::Events { graph: "p".into(), state: "initial".into(), deferred: true }]);
        assert_eq!(globals["b"], true, "deferred: the state changed at once");
        assert!(!runner.busy());

        for enter in ["exec", "defer"] {
            let (effects, globals, runner, ..) = run(enter, 0);
            assert!(effects.is_empty(), "{enter}: no events, no group");
            assert_eq!(globals["b"], true, "{enter}: a state with no events changes at once");
            assert!(!runner.busy());
        }
    }

    #[test]
    fn an_unsupported_native_is_reported_once() {
        let lua = r#"function a(self, m, v, s, g) Unlock.Nope() Unlock.Nope(1) ChangeState("complete") end"#;
        let d = data(lua, vec![("g", graph(&[("initial", &[("stateenter", "a")], 0), ("complete", &[], 0)]))], &[]);
        let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
        runner.queue(fired("g"));
        runner.tick(&mut rules, &mut globals, &d);
        assert_eq!(runner.unsupported(), ["Unlock.Nope in g/initial"]);
        assert!(!runner.busy());
    }

    /// Answers every prompt the script raises (Yes to a popup) and plays every group, until the runner is idle.
    fn run_to_idle(runner: &mut Runner, rules: &mut Rules, globals: &mut Value, d: &CareerData) -> Vec<Effect> {
        let mut all = Vec::new();
        for _ in 0..10_000 {
            let mut effects = runner.tick(rules, globals, d);
            while let Some(effect) = effects.pop() {
                let reply = match &effect {
                    Effect::Show101 { .. } => Some("hudcomplete"),
                    Effect::YesNo { .. } => Some("hudyes"),
                    Effect::Events { deferred: false, .. } => Some("presentationeventgroupfinished"),
                    Effect::CareerPhoto(true) => Some(super::super::core::PHOTO_LOADED),
                    _ => None,
                };
                all.push(effect);
                if let Some(reply) = reply {
                    effects.extend(runner.send(reply, rules, globals, d));
                }
            }
            if !runner.busy() {
                return all;
            }
        }
        let tail: Vec<String> = all.iter().rev().take(12).map(|e| format!("{e:?}")).collect();
        panic!("the career script never went idle; last effects: {tail:#?}");
    }

    fn export() -> CareerData {
        let dir = std::path::PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        crate::skate3_career::load(&dir).unwrap_or_else(|why| panic!("{why}"))
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn starting_stategraph_activates_what_boot_md_lists() {
        let d = export();
        let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
        for fired in rules.root(&d) {
            runner.queue(fired);
        }
        run_to_idle(&mut runner, &mut rules, &mut globals, &d);
        let first = ["tut_sksc_01", "tele_sksc_01", "tele_sksc_02", "tele_sksc_03", "tele_sksc_04", "tele_school_to_world", "ftl_univ_02"];
        assert_eq!(&rules.active[..first.len()], first, "{:?}", rules.active);
        for armed in ["skateschool_complete_01", "skateschool_complete_all", "skateschool_complete_basic", "skateschool_unlock_intermediate", "ftl_univ_02_complete"] {
            assert!(rules.actions_active.iter().any(|a| a == armed), "{armed} not armed: {:?}", rules.actions_active);
        }
        // State `achievements` arms 20 (boot.md 7): 18 named so and the first OTL and 1-UP rows' completions.
        let achievements = rules.actions_active.iter().filter(|a| a.contains("achievement")).count();
        assert_eq!(achievements, 18, "{:?}", rules.actions_active);
        for armed in ["otl_bbx_01_complete", "1up_bbpk_01_complete"] {
            assert!(rules.actions_active.iter().any(|a| a == armed), "{armed} not armed");
        }
        for global in [
            "CompletedFirstFTL",
            "CompletedFirstPhoto",
            "CompletedFirstFilm",
            "CompletedSecondFTL",
            "CompletedMovingTutorial",
            "CompletedDIYTutorial",
            "CompletedBasicSchool",
            "CompletedIntSchool",
            "CompletedAdvSchool",
        ] {
            assert_eq!(globals[global], false, "{global} in {globals}");
        }
        assert!(runner.unsupported().is_empty(), "{:?}", runner.unsupported());
    }

    /// Owns `rows` in order through the rules and the script, answering every prompt; answers what they raised.
    fn own_in_order(d: &CareerData, rows: &[&str]) -> (Rules, Runner, Vec<Effect>) {
        let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
        for fired in rules.root(d) {
            runner.queue(fired);
        }
        let mut effects = run_to_idle(&mut runner, &mut rules, &mut globals, d);
        for row in rows {
            assert!(rules.active.iter().any(|a| a == row), "{row} is not active: {:?}", rules.active);
            for fired in rules.record(d, row, super::super::rules::Outcome::Owned).1 {
                runner.queue(fired);
            }
            effects.extend(run_to_idle(&mut runner, &mut rules, &mut globals, d));
        }
        (rules, runner, effects)
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn every_stage_one_handler_runs_without_an_unsupported_native() {
        let d = export();
        let stage1 = [
            "tut_sksc_01",
            "tut_sksc_02",
            "tut_sksc_06",
            "tut_sksc_03",
            "tut_sksc_04",
            "tele_sksc_01",
            "tele_sksc_02",
            "tele_sksc_03",
            "tele_sksc_04",
            "tele_school_to_world",
            "ftl_univ_02",
            "photo_univ_08",
            "film_univ_05",
            "ftl_univ_03",
        ];
        let (_, runner, _) = own_in_order(&d, &stage1);
        assert!(runner.unsupported().is_empty(), "{:?}", runner.unsupported());
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn the_whole_career_activates_rows_as_order_md_says() {
        let d = export();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/superpowers/specs/skate3-story-mode-reference/work/order/closure.json");
        let closure: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let (mut rules, mut globals, mut runner) = (Rules::default(), json!({}), Runner::new());
        for fired in rules.root(&d) {
            runner.queue(fired);
        }
        let mut effects = run_to_idle(&mut runner, &mut rules, &mut globals, &d);
        // Own the oldest active row not yet done, until none is left.
        while let Some(row) = rules.active.iter().find(|row| rules.level(row) == Level::NotDone).cloned() {
            for fired in rules.record(&d, &row, super::super::rules::Outcome::Owned).1 {
                runner.queue(fired);
            }
            effects.extend(run_to_idle(&mut runner, &mut rules, &mut globals, &d));
        }
        let by: BTreeMap<&str, &str> = runner.activated().iter().map(|(row, graph)| (row.as_str(), graph.as_str())).collect();
        let mut wrong = Vec::new();
        for (row, from) in closure["challenges"].as_object().unwrap() {
            // "<graph>", "<group> group via <graph>" or "ots discovery via <graph>", then "@<line>".
            let how = from.as_str().unwrap().split('@').next().unwrap();
            let graph = how.rsplit(" via ").next().unwrap();
            // Own the Spot rows are found in the world once the graph allows it (Discovery.AllowDiscovery; stage 3).
            if how.contains(" discovery via ") {
                if !effects.contains(&Effect::AllowDiscovery) {
                    wrong.push(format!("{row}: {graph} never allowed discovery"));
                }
            } else if by.get(row.as_str()) != Some(&graph) {
                wrong.push(format!("{row}: {how} expected, got {:?}", by.get(row.as_str())));
            }
        }
        assert!(wrong.is_empty(), "{} of {} rows: {wrong:#?}", wrong.len(), closure["challenges"].as_object().unwrap().len());
    }
}
