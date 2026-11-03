//! The game-neutral challenge core the Skate 1 and Skate 2 tutorial ports share
//! (docs/superpowers/specs/2026-09-30-skate{1,2}-tutorial-design.md).
//!
//! A [`Machine`] is one of the disc's step machines: a lesson's steps (Skate 1 and 2 ChallengeCore) or a
//! progression trigger's states (Skate 2 progressioncore). Each step maps event names to handler functions in
//! the decompiled Lua the data scripts wrote. `ChangeState(name)` runs the current step's exit handler,
//! switches, then runs the new step's enter handler; any other event goes to the current step's handler of that
//! name and is dropped when there is none. The core knows no game's natives: a binding registers them.
pub(crate) mod cutscene;
pub(crate) mod follow;
pub(crate) mod movie;

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// What the decompiled Lua 5.1 uses that Lua 5.4 dropped, plus the helpers both games' objective scripts call.
const PRELUDE: &str = r#"
table.getn = function(t) return #t end
unpack = table.unpack
function InitVars(vars, key, value) if vars[key] == nil then vars[key] = value end end
-- 5.1's integer conversions truncate a float ("%d", 6.25 -> "6"); 5.4's reject one with a fraction.
do
  local format = string.format
  string.format = function(fmt, ...)
    local args, at, n = table.pack(...), 1, 0
    while true do
      local _, last, conv = fmt:find("%%[-+ #0]*%d*%.?%d*(.)", at)
      if not last then break end
      if conv ~= "%" then
        n = n + 1
        local v = args[n]
        if conv:find("[dicoxX]") and math.type(v) == "float" and v == v and math.abs(v) < 2^63 then
          args[n] = math.tointeger(v < 0 and math.ceil(v) or math.floor(v))
        end
      end
      at = last + 1
    end
    return format(fmt, table.unpack(args, 1, args.n))
  end
end
"#;
/// Instruction budget per call, in thousands; the disc's handlers run a few hundred instructions.
const BUDGET: usize = 200;
/// ChangeState chains longer than this are a loop in the data, not a sequence.
const MAX_TRANSITIONS: usize = 64;
/// The step whose handlers apply in every step.
const GLOBAL_STEP: &str = "global";
/// The Lua registry key that holds the step whose handler is running, for natives that act on their own step.
pub(crate) const STATE_KEY: &str = "challenge.state";

/// The Start menu's lesson entries (Restart / Abandon). A binding sets `active` while one of its lessons runs; the
/// Start menu then lists the entries and leaves the chosen one in `request` for the binding to send.
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct LessonMenu {
    pub active: bool,
    pub request: Option<LessonRequest>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LessonRequest {
    Restart,
    Abandon,
}

/// One sandboxed Lua per running machine: table/string/math only, a memory cap and an instruction budget, as
/// skate-mods' `Vm`.
pub(crate) fn sandbox(label: &str) -> Result<(Lua, Arc<AtomicUsize>), String> {
    let build = || -> mlua::Result<(Lua, Arc<AtomicUsize>)> {
        let lua = Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::MATH, LuaOptions::default())?;
        lua.set_memory_limit(16 * 1024 * 1024)?;
        for key in ["load", "loadfile", "dofile", "collectgarbage", "require"] {
            lua.globals().set(key, mlua::Value::Nil)?;
        }
        let budget = Arc::new(AtomicUsize::new(BUDGET));
        let counter = budget.clone();
        lua.set_hook(HookTriggers::new().every_nth_instruction(1000), move |_, _| {
            match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)) {
                Ok(_) => Ok(VmState::Continue),
                Err(_) => Err(mlua::Error::RuntimeError("Lua instruction budget exhausted".into())),
            }
        })?;
        let label = label.to_owned();
        lua.globals().set(
            "print",
            lua.create_function(move |_, text: mlua::Value| {
                bevy::log::debug!("{label}: {}", text.to_string().unwrap_or_default());
                Ok(())
            })?,
        )?;
        lua.load(PRELUDE).set_name("@prelude").exec()?;
        Ok((lua, budget))
    };
    build().map_err(|e| format!("{label}: {e}"))
}

/// JSON as the disc's scripts see a vault row: arrays are 1-based tables carrying `Count`, null is nil.
pub(crate) fn to_lua(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        Value::Null => mlua::Value::Nil,
        Value::Bool(b) => mlua::Value::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => mlua::Value::Integer(i),
            None => mlua::Value::Number(n.as_f64().unwrap_or(0.)),
        },
        Value::String(s) => mlua::Value::String(lua.create_string(s)?),
        Value::Array(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, to_lua(lua, item)?)?;
            }
            t.set("Count", items.len())?;
            mlua::Value::Table(t)
        }
        Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                t.set(k.as_str(), to_lua(lua, v)?)?;
            }
            mlua::Value::Table(t)
        }
    })
}

/// A step machine's script: step -> event -> handler function name, the Lua defining those functions, and the
/// names of its enter/exit events (`stateenter`/`stateexit` for lessons, `cMsgStateEnter`/`cMsgStateExit` for
/// Skate 2 progression triggers).
pub(crate) struct Script<'a> {
    pub name: &'a str,
    pub source: &'a str,
    pub steps: BTreeMap<String, BTreeMap<String, String>>,
    pub initial: &'a str,
    pub enter: &'static str,
    pub exit: &'static str,
}

/// Requests the handlers make of the machine itself.
#[derive(Default)]
struct Control {
    next: Option<String>,
    terminated: bool,
}

pub(crate) struct Machine {
    pub lua: Lua,
    budget: Arc<AtomicUsize>,
    name: String,
    steps: BTreeMap<String, BTreeMap<String, String>>,
    state: String,
    enter: &'static str,
    exit: &'static str,
    this: Table,
    vars: Table,
    /// Handler argument 4: one table per machine (Skate 3 progression stategraphs).
    sharedvars: Table,
    /// Handler argument 5: the career-wide table, which the owner sets before and reads after each call.
    globalvars: Table,
    control: Arc<Mutex<Control>>,
}

impl Machine {
    /// Loads the script with `self` = `fields`. `natives` registers the binding's tables before the script runs.
    pub(crate) fn new(script: Script, fields: &Value, natives: impl FnOnce(&Lua) -> mlua::Result<()>) -> Result<Self, String> {
        let (lua, budget) = sandbox(script.name)?;
        let control = Arc::new(Mutex::new(Control::default()));
        let build = || -> mlua::Result<(Table, Table, Table, Table)> {
            let c = control.clone();
            lua.globals().set(
                "ChangeState",
                lua.create_function(move |_, next: String| {
                    c.lock().unwrap().next = Some(next);
                    Ok(())
                })?,
            )?;
            for name in ["Terminate", "TerminateWithoutSave"] {
                let c = control.clone();
                lua.globals().set(
                    name,
                    lua.create_function(move |_, ()| {
                        c.lock().unwrap().terminated = true;
                        Ok(())
                    })?,
                )?;
            }
            natives(&lua)?;
            lua.load(script.source).set_name(format!("@{}", script.name)).exec()?;
            let this = match to_lua(&lua, fields)? {
                mlua::Value::Table(t) => t,
                _ => lua.create_table()?,
            };
            Ok((this, lua.create_table()?, lua.create_table()?, lua.create_table()?))
        };
        let (this, vars, sharedvars, globalvars) = build().map_err(|e| format!("{}: {e}", script.name))?;
        for (step, events) in &script.steps {
            for function in events.values() {
                if !matches!(lua.globals().get::<mlua::Value>(function.as_str()), Ok(mlua::Value::Function(_))) {
                    return Err(format!("{}: step {step} names {function}, which the script does not define", script.name));
                }
            }
        }
        Ok(Self {
            lua,
            budget,
            name: script.name.to_owned(),
            steps: script.steps,
            state: script.initial.to_owned(),
            enter: script.enter,
            exit: script.exit,
            this,
            vars,
            sharedvars,
            globalvars,
            control,
        })
    }

    /// Replaces the career-wide `globalvars` table (handler argument 5).
    pub(crate) fn set_globalvars(&mut self, value: &Value) -> Result<(), String> {
        self.globalvars = match to_lua(&self.lua, value).map_err(|e| e.to_string())? {
            mlua::Value::Table(t) => t,
            _ => self.lua.create_table().map_err(|e| e.to_string())?,
        };
        Ok(())
    }

    /// The `globalvars` table as JSON.
    pub(crate) fn globalvars(&self) -> Value {
        from_lua(&mlua::Value::Table(self.globalvars.clone()))
    }

    /// `ChangeState(next)` from the host: the current step's exit, then `next`'s enter (a presentation group that
    /// finished, for a state that waited on it).
    pub(crate) fn change_state(&mut self, next: &str) -> Result<(), String> {
        if self.terminated() {
            return Ok(());
        }
        self.control.lock().unwrap().next = Some(next.to_owned());
        self.settle()
    }

    pub(crate) fn state(&self) -> &str {
        &self.state
    }
    /// The script called Terminate (progression triggers) — the machine is done.
    pub(crate) fn terminated(&self) -> bool {
        self.control.lock().unwrap().terminated
    }
    pub(crate) fn vars(&self) -> &Table {
        &self.vars
    }

    /// Enters the initial step.
    pub(crate) fn start(&mut self) -> Result<(), String> {
        let initial = self.state.clone();
        self.call(&initial, self.enter, &Value::Null)?;
        self.settle()
    }

    /// Delivers one event to the current step, or to the `global` step when the current one has no handler for it
    /// (Skate 2 lessons keep abandon/restart there).
    pub(crate) fn send(&mut self, event: &str, message: &Value) -> Result<(), String> {
        if self.terminated() {
            return Ok(());
        }
        let handles = |step: &str| self.steps.get(step).is_some_and(|events| events.contains_key(event));
        let state = if handles(&self.state) { self.state.clone() } else { GLOBAL_STEP.to_owned() };
        self.call(&state, event, message)?;
        self.settle()
    }

    fn settle(&mut self) -> Result<(), String> {
        for _ in 0..MAX_TRANSITIONS {
            let Some(next) = self.control.lock().unwrap().next.take() else { return Ok(()) };
            if self.terminated() {
                return Ok(());
            }
            if !self.steps.contains_key(&next) {
                return Err(format!("{}: ChangeState to unknown step {next}", self.name));
            }
            let current = std::mem::replace(&mut self.state, next.clone());
            self.call(&current, self.exit, &Value::Null)?;
            self.call(&next, self.enter, &Value::Null)?;
        }
        Err(format!("{}: more than {MAX_TRANSITIONS} state changes in one event", self.name))
    }

    fn call(&mut self, step: &str, event: &str, message: &Value) -> Result<(), String> {
        let Some(function) = self.steps.get(step).and_then(|events| events.get(event)) else { return Ok(()) };
        self.budget.store(BUDGET, Ordering::Relaxed);
        let run = || -> mlua::Result<()> {
            self.lua.set_named_registry_value(STATE_KEY, step)?;
            let f: mlua::Function = self.lua.globals().get(function.as_str())?;
            f.call::<()>((
                self.this.clone(),
                to_lua(&self.lua, message)?,
                self.vars.clone(),
                self.sharedvars.clone(),
                self.globalvars.clone(),
            ))
        };
        run().map_err(|e| format!("{} [{step}] {event}: {e}", self.name))
    }
}

/// A Lua value as JSON: tables become objects keyed by their keys' text (the scripts' variable tables are flat).
pub(crate) fn from_lua(value: &mlua::Value) -> Value {
    match value {
        mlua::Value::Boolean(b) => Value::Bool(*b),
        mlua::Value::Integer(i) => Value::from(*i),
        mlua::Value::Number(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
        mlua::Value::String(s) => Value::String(s.to_string_lossy()),
        mlua::Value::Table(t) => Value::Object(
            t.pairs::<mlua::Value, mlua::Value>()
                .filter_map(Result::ok)
                .filter_map(|(k, v)| {
                    let key = match &k {
                        mlua::Value::String(s) => s.to_string_lossy(),
                        mlua::Value::Integer(i) => i.to_string(),
                        mlua::Value::Number(n) => n.to_string(),
                        _ => return None,
                    };
                    Some((key, from_lua(&v)))
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// A condition the disc keeps as Lua source (Skate 2 objective definitions: a `do … return … end` block over
/// `vars`), compiled once into a function of a `vars` table. Skate 2 hints share their objective's table.
pub(crate) struct Condition {
    function: mlua::Function,
    vars: Table,
}
impl Condition {
    pub(crate) fn compile(lua: &Lua, name: &str, source: &str) -> Result<Self, String> {
        let vars = lua.create_table().map_err(|e| e.to_string())?;
        Self::compile_with(lua, name, source, vars)
    }
    pub(crate) fn compile_with(lua: &Lua, name: &str, source: &str, vars: Table) -> Result<Self, String> {
        let function = lua
            .load(format!("return function(vars)
{source}
end"))
            .set_name(format!("@{name}"))
            .eval()
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(Self { function, vars })
    }
    /// Forgets the condition's variables (an objective reset), in place so sharers see it too.
    pub(crate) fn reset(&mut self) -> Result<(), String> {
        self.vars.clear().map_err(|e| e.to_string())
    }
    /// Lua truthiness of the block's result; nil (no return) is false.
    pub(crate) fn evaluate(&self, budget: &Machine) -> Result<bool, String> {
        budget.budget.store(BUDGET, Ordering::Relaxed);
        self.function
            .call::<mlua::Value>(self.vars.clone())
            .map(|v| !matches!(v, mlua::Value::Nil | mlua::Value::Boolean(false)))
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn script(source: &str, steps: Value) -> Machine {
        let steps = serde_json::from_value(steps).unwrap();
        Machine::new(
            Script { name: "test", source, steps, initial: "initial", enter: "stateenter", exit: "stateexit" },
            &json!({"Name": "lesson", "Events": [{"a": 1}, {"a": 2}]}),
            |_| Ok(()),
        )
        .unwrap()
    }

    #[test]
    fn change_state_runs_exit_then_enter_and_events_reach_the_current_step() {
        let mut m = script(
            r#"
            function a(self, message, vars) vars.log = (vars.log or "") .. "enter-initial;" ChangeState("run") end
            function b(self, message, vars) vars.log = vars.log .. "exit-initial;" end
            function c(self, message, vars) vars.log = vars.log .. "enter-run " .. self.Events.Count .. ";" end
            function d(self, message, vars) vars.log = vars.log .. "hit " .. message.Index .. ";" end
            "#,
            json!({"initial": {"stateenter": "a", "stateexit": "b"}, "run": {"stateenter": "c", "hit": "d"}}),
        );
        m.start().unwrap();
        m.send("hit", &json!({"Index": 3})).unwrap();
        m.send("unhandled", &Value::Null).unwrap();
        assert_eq!(m.state(), "run");
        assert_eq!(m.vars().get::<String>("log").unwrap(), "enter-initial;exit-initial;enter-run 2;hit 3;");
    }

    /// Skate 3's progression handlers take `(self, message, vars, sharedvars, globalvars)`.
    #[test]
    fn handlers_receive_sharedvars_and_globalvars() {
        let mut m = script(
            r#"
            function a(self, message, vars, sharedvars, globalvars)
                sharedvars.n = (sharedvars.n or 0) + 1
                globalvars.X = true
                globalvars.Seen = globalvars.Before
            end
            function b(self, message, vars) vars.three = true end
            "#,
            json!({"initial": {"stateenter": "a", "old": "b"}}),
        );
        m.set_globalvars(&json!({"Before": 7})).unwrap();
        m.start().unwrap();
        assert_eq!(m.globalvars(), json!({"Before": 7, "X": true, "Seen": 7}));
        m.send("old", &Value::Null).unwrap();
        assert!(m.vars().get::<bool>("three").unwrap(), "a three-argument handler still runs");
        assert_eq!(m.sharedvars.get::<i64>("n").unwrap(), 1);
    }

    #[test]
    fn integer_formats_truncate_floats_as_lua_5_1_did() {
        let m = script("", json!({"initial": {}}));
        let text: String = m.lua.load(r#"return string.format("%d%% %s %d %.1f", 6.75, "x", -2.5, 1.25)"#).eval().unwrap();
        assert_eq!(text, "6% x -2 1.2");
    }

    #[test]
    fn a_state_loop_and_a_runaway_script_are_errors_not_hangs() {
        let mut m = script(
            "function a(self, message, vars) ChangeState('initial') end function spin() while true do end end",
            json!({"initial": {"stateenter": "a", "spin": "spin"}}),
        );
        assert!(m.start().is_err());
        assert!(m.send("spin", &Value::Null).is_err());
    }

    #[test]
    fn a_condition_keeps_its_own_vars_until_reset() {
        let m = script("", json!({"initial": {}}));
        let mut c = Condition::compile(&m.lua, "obj", "do InitVars(vars,'n',0) vars.n = vars.n + 1 return vars.n >= 2 end").unwrap();
        assert!(!c.evaluate(&m).unwrap());
        assert!(c.evaluate(&m).unwrap());
        c.reset().unwrap();
        assert!(!c.evaluate(&m).unwrap());
    }

    #[test]
    fn ending_a_challenge_frees_its_lua_state() {
        // 50 challenges back to back, each with a Lua-side cycle holding the natives: a leaked state keeps `control` alive.
        let held: Vec<_> = (0..50)
            .map(|_| {
                let mut m = script(
                    "function a(self, message, vars) local t = {} t.t = t t.f = ChangeState vars.keep = t self.keep = t end",
                    json!({"initial": {"stateenter": "a"}}),
                );
                m.start().unwrap();
                (Arc::downgrade(&m.control), Arc::downgrade(&m.budget))
            })
            .collect();
        assert!(held.iter().all(|(c, b)| c.upgrade().is_none() && b.upgrade().is_none()));
    }
}
