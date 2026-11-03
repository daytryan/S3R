use crate::{Manifest, read_bounded};
use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, VmState, chunk::ChunkMode};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    NetworkState {
        key: String,
        #[serde(default)]
        value: Value,
    },
    VehicleTune {
        key: String,
        tuning: skate_vehicles::VehicleTuning,
    },
    VehicleSpawn {
        key: String,
        definition: String,
        position: [f32; 3],
        heading: f32,
    },
    VehicleRemove {
        key: String,
    },
    VehicleEnter {
        key: String,
    },
    VehicleExit {
        key: String,
    },
    VehicleReset {
        key: String,
        position: [f32; 3],
        heading: f32,
    },
    VehicleControl {
        key: String,
        controls: skate_vehicles::Controls,
    },
    Trainer {
        tuning: crate::TrainerTuning,
    },
    Log {
        text: String,
    },
    Animation {
        path: String,
    },
    Overlay {
        key: String,
        text: String,
    },
    Cube {
        key: String,
        position: [f32; 3],
        size: [f32; 3],
        color: [f32; 3],
    },
    Remove {
        key: String,
    },
    Teleport {
        position: [f32; 3],
        heading: f32,
        on_board: bool,
    },
}
impl Command {
    pub fn validate(&self) -> bool {
        let point = |p: &[f32; 3]| p.iter().all(|v| v.is_finite() && v.abs() <= 100_000.);
        match self {
            Self::NetworkState { key, value } => {
                crate::schema::valid_id(key)
                    && serde_json::to_vec(value).is_ok_and(|v| v.len() <= 512)
            }
            Self::VehicleTune { key, tuning } => crate::schema::valid_id(key) && tuning.valid(),
            Self::VehicleSpawn {
                key,
                definition,
                position,
                heading,
            } => {
                crate::schema::valid_id(key)
                    && skate_vehicles::package_path(definition)
                    && point(position)
                    && heading.is_finite()
            }
            Self::VehicleReset {
                key,
                position,
                heading,
            } => crate::schema::valid_id(key) && point(position) && heading.is_finite(),
            Self::VehicleControl { key, controls } => {
                crate::schema::valid_id(key) && controls.valid()
            }
            Self::VehicleRemove { key }
            | Self::VehicleEnter { key }
            | Self::VehicleExit { key } => crate::schema::valid_id(key),
            Self::Trainer { tuning } => tuning.valid(),
            Self::Animation { path } => !path.is_empty() && path.len() <= 256,
            Self::Log { text } => text.len() <= 2048,
            Self::Overlay { key, text } => crate::schema::valid_id(key) && text.len() <= 1024,
            Self::Cube {
                key,
                position,
                size,
                color,
            } => {
                crate::schema::valid_id(key)
                    && point(position)
                    && size.iter().all(|v| v.is_finite() && *v > 0. && *v <= 100.)
                    && color
                        .iter()
                        .all(|v| v.is_finite() && (0. ..=1.).contains(v))
            }
            Self::Remove { key } => crate::schema::valid_id(key),
            Self::Teleport {
                position, heading, ..
            } => point(position) && heading.is_finite(),
        }
    }
}
pub(crate) struct Vm {
    lua: Lua,
    callbacks: Table,
    advance: mlua::Function,
    budget: Arc<AtomicUsize>,
    queue: Arc<Mutex<Vec<Command>>>,
    id: String,
}
/// serde walks Lua values with no depth or sharing limit: a 10k-deep nest overflows the stack (abort), a
/// 40-level chain of tables listed twice takes 2^40 steps outside the instruction hook. No command needs more.
fn bounded(v: &mlua::Value, depth: usize, budget: &mut usize) -> bool {
    if *budget == 0 || depth > 32 {
        return false;
    }
    *budget -= 1;
    let mlua::Value::Table(t) = v else {
        return true;
    };
    t.pairs::<mlua::Value, mlua::Value>()
        .all(|kv| kv.is_ok_and(|(k, v)| bounded(&k, depth + 1, budget) && bounded(&v, depth + 1, budget)))
}
/// The snapshot as mod `id` sees it: peers' shared state for its own mod only (sdk.net.read's view). Every peer
/// can fill 256 records, and this conversion runs for each mod on every callback.
fn snapshot_for(lua: &Lua, snapshot: &Value, id: &str) -> mlua::Result<mlua::Value> {
    let (Value::Object(all), Some(Value::Object(net))) = (snapshot, snapshot.get("network")) else {
        return lua.to_value(snapshot);
    };
    let copy = |m: &serde_json::Map<String, Value>, skip: &str| -> mlua::Result<Table> {
        let t = lua.create_table()?;
        for (k, v) in m.iter().filter(|(k, _)| *k != skip) {
            t.set(k.as_str(), lua.to_value(v)?)?;
        }
        Ok(t)
    };
    let network = copy(net, "states")?;
    if let Some(states) = net.get("states") {
        let own = lua.create_table()?;
        if let Some(mine) = states.get(id) {
            own.set(id, lua.to_value(mine)?)?;
        }
        network.set("states", own)?;
    }
    let t = copy(all, "network")?;
    t.set("network", network)?;
    Ok(mlua::Value::Table(t))
}
impl Vm {
    pub fn new(
        root: &Path,
        manifest: &Manifest,
        settings: &BTreeMap<String, Value>,
        snapshot: &Value,
    ) -> Result<Self, String> {
        let build = || -> mlua::Result<Self> {
            let lua = Lua::new_with(
                StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
                LuaOptions::default(),
            )?;
            lua.set_memory_limit(8 * 1024 * 1024)?;
            // No catch/retry loops around quota errors, coroutine bypass, bytecode or filesystem loaders.
            for key in [
                "pcall",
                "xpcall",
                "load",
                "loadfile",
                "dofile",
                "collectgarbage",
                "print",
            ] {
                lua.globals().set(key, mlua::Value::Nil)?;
            }
            // No loader takes its output, but it is the first half of every bytecode escape.
            lua.globals().get::<Table>("string")?.set("dump", mlua::Value::Nil)?;
            let budget = Arc::new(AtomicUsize::new(100));
            let counter = budget.clone();
            lua.set_hook(
                HookTriggers::new().every_nth_instruction(1000),
                move |_, _| {
                    if counter
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                        .is_err()
                    {
                        return Err(mlua::Error::RuntimeError(
                            "Lua instruction budget exhausted".into(),
                        ));
                    }
                    Ok(VmState::Continue)
                },
            )?;
            let queue = Arc::new(Mutex::new(Vec::<Command>::new()));
            let out = queue.clone();
            let sdk = lua.create_table()?;
            sdk.set("api_version", 1)?;
            sdk.set("mod_id", manifest.id.clone())?;
            sdk.set(
                "_submit",
                lua.create_function(move |lua, value: mlua::Value| {
                    if !bounded(&value, 0, &mut 1024) {
                        return Err(mlua::Error::RuntimeError(
                            "Command arguments too large or nested".into(),
                        ));
                    }
                    let c: Command = lua.from_value(value)?;
                    if !c.validate() {
                        return Err(mlua::Error::RuntimeError(
                            "Invalid command arguments".into(),
                        ));
                    }
                    let mut q = out.lock().unwrap();
                    if q.len() >= 128 {
                        return Err(mlua::Error::RuntimeError(
                            "128 commands per callback maximum".into(),
                        ));
                    }
                    q.push(c);
                    Ok(())
                })?,
            )?;
            let asset_root = root.to_path_buf();
            sdk.set(
                "read_text",
                lua.create_function(move |_, path: String| {
                    let b = read_bounded(&asset_root, &path, 256 * 1024)
                        .map_err(mlua::Error::RuntimeError)?;
                    String::from_utf8(b).map_err(mlua::Error::external)
                })?,
            )?;
            sdk.set("settings", lua.to_value(settings)?)?;
            sdk.set("snapshot", snapshot_for(&lua, snapshot, &manifest.id)?)?;
            lua.globals().set("sdk", sdk)?;
            lua.load(include_str!("api.lua"))
                .set_name("@skate-sdk-1")
                .exec()?;
            let sdk: Table = lua.globals().get("sdk")?;
            let advance = sdk.get("_advance")?;
            sdk.set("_advance", mlua::Value::Nil)?;
            let code = crate::read_bounded(root, &manifest.entry, 256 * 1024)
                .map_err(mlua::Error::RuntimeError)?;
            let source = std::str::from_utf8(&code).map_err(mlua::Error::external)?;
            // Text only: a precompiled chunk ("\x1bLua") skips the parser, and Lua has no bytecode verifier.
            let callbacks: Table = lua
                .load(source)
                .set_mode(ChunkMode::Text)
                .set_name(format!("@{}/{}", manifest.id, manifest.entry))
                .eval()?;
            for pair in callbacks.clone().pairs::<String, mlua::Value>() {
                let (key, value) = pair?;
                if ![
                    "on_load",
                    "on_unload",
                    "on_update",
                    "on_fixed_update",
                    "on_event",
                    "on_settings",
                ]
                .contains(&key.as_str())
                    || !matches!(value, mlua::Value::Function(_))
                {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Unknown callback or non-function: {key}"
                    )));
                }
            }
            // Top-level commands are staged together with on_load, not committed yet.
            Ok(Self {
                lua,
                callbacks,
                advance,
                budget,
                queue,
                id: manifest.id.clone(),
            })
        };
        build().map_err(|e| e.to_string())
    }
    pub fn settings(&mut self, settings: &BTreeMap<String, Value>) -> mlua::Result<()> {
        self.lua
            .globals()
            .get::<Table>("sdk")?
            .set("settings", self.lua.to_value(settings)?)
    }
    pub fn call(
        &mut self,
        name: &str,
        payload: Value,
        snapshot: &Value,
    ) -> Result<Vec<Command>, String> {
        self.budget.store(100, Ordering::Relaxed);
        let invoke = || -> mlua::Result<()> {
            self.lua
                .globals()
                .get::<Table>("sdk")?
                .set("snapshot", snapshot_for(&self.lua, snapshot, &self.id)?)?;
            if let Some(f) = self.callbacks.get::<Option<mlua::Function>>(name)? {
                f.call::<()>(self.lua.to_value(&payload)?)?;
            }
            if name == "on_update" {
                self.advance
                    .call::<()>(payload["dt"].as_f64().unwrap_or(0.))?;
            }
            Ok(())
        };
        let result = invoke();
        let commands = std::mem::take(&mut *self.queue.lock().unwrap());
        result.map(|_| commands).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Item 29: a mod reloaded all session long must free each Lua state. The state's own closures hold the only
    /// other clone of `queue`, so a dead weak pointer after the drop proves the whole VM went.
    #[test]
    fn reloading_a_mod_frees_every_lua_state() {
        let root = std::env::temp_dir().join(format!("skate-sdk-vm-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        // A Lua-side cycle (a table holding a closure over itself and sdk) and staged commands.
        std::fs::write(
            root.join("main.lua"),
            "local t = {}; t.f = function() return t, sdk end; sdk.log('hi'); return {on_update=function() t.f(); sdk.log('u') end}",
        )
        .unwrap();
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id":"leak","api":1,"name":"Leak","version":"1.0.0","author":"Test","description":"Test","entry":"main.lua"
        }))
        .unwrap();
        for _ in 0..50 {
            let mut vm = Vm::new(&root, &manifest, &BTreeMap::new(), &serde_json::json!({"map":"x"})).unwrap();
            vm.call("on_update", Value::Null, &serde_json::json!({"map":"y"})).unwrap();
            let (queue, budget) = (Arc::downgrade(&vm.queue), Arc::downgrade(&vm.budget));
            drop(vm);
            assert!(queue.upgrade().is_none() && budget.upgrade().is_none(), "a dropped mod VM stayed alive");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
