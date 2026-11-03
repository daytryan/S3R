//! Skate 1's online modes (Best Trick, Jam, Spot Battle, S.K.A.T.E., Death Race, Spot Race) on the match core, run
//! by the disc's own challenge Lua. Data: skate2port/old_online1.py + old_online1_modes.py, installed at
//! `private/skate1-online/` (rows.json: the vault rows with their places resolved and each template's steps;
//! scripts/<template>.lua: the decompiled handlers; modes.json: the HUD enums, titles and strings; intros/: the
//! flythrough cameras).
//!
//! Every player's game runs the row's script itself ([`Session`], on the shared challenge core), as retail's
//! consoles each ran theirs: the local player's collisions, sequences and wipeouts ([`Probe`]) go to the script and,
//! through `Match::outbox`, to every other player's script with that player's id. The natives the scripts call are
//! skate1_online.lua: queries answer from the binding's state, every other call is recorded and applied here
//! ([`Run::apply`]): repositions, the stop / pin hold, flythroughs, visual indicators, the standings; the HUD calls
//! go to skate1_online_hud.rs.
use crate::{
    challenge::{Condition, Machine, Script, cutscene::Cutscene, to_lua},
    config::Config,
    match_core::{Match, Mode, Owned, Phase, Playable},
    physics::SkaterRuntime,
};
use skate_core::physics::filtered_state::FilteredCategory;
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const ROOT: &str = "private/skate1-online";
const NATIVES: &str = include_str!("skate1_online.lua");
/// The match core's cap on an owned match (the scripts end themselves; this only stops a stuck one). A knob.
const CAP: f32 = 30. * 60.;
/// The C++ fires these a frame after the call that asks for them.
const NEXT_FRAME: f32 = 1. / 60.;
/// The most one frame catches up (Session::step), seconds.
const CATCH_UP: f32 = 30.;

// ---------------------------------------------------------------- data
#[derive(Deserialize)]
struct RowsFile {
    trick_types: Vec<String>,
    modes: BTreeMap<String, Vec<Row>>,
    scripts: BTreeMap<String, Steps>,
}
#[derive(Deserialize)]
struct Steps {
    steps: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}
impl Aabb {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Locator {
    pub matrix: [[f32; 4]; 4],
    #[serde(default)]
    pub slots: Vec<[[f32; 4]; 4]>,
}
impl Locator {
    /// AI.RepositionAI's spot for `slot` (S1 0x823d9088, online_map::locator_slot).
    fn slot(&self, slot: usize) -> [[f32; 4]; 4] {
        crate::online_map::locator_slot(&self.matrix, &self.slots, slot)
    }
    /// [`Self::slot`] as the modes place a player. Bug 10-31 (The Walrus "in a wall"), not retail: a locator with no
    /// spawns (13 Spot Battles' BAM.loc ones, all left at one default -X heading) faces its row's spot, its later slots
    /// 2 m abreast (+2, -2, +4 m) instead of 5 m on along -Z, which went off the landing on four rows; lifted
    /// [`BAM_LIFT`].
    pub(crate) fn spawn(&self, slot: usize, row: &Row) -> [[f32; 4]; 4] {
        let (true, Some(to)) = (self.slots.is_empty(), row.spot()) else { return self.slot(slot) };
        let mut m = self.matrix;
        let (x, z) = (to[0] - m[3][0], to[2] - m[3][2]);
        let len = x.hypot(z);
        if len > 0.01 {
            let f = [x / len, 0., z / len, 0.];
            (m[0], m[1], m[2]) = ([f[2], 0., -f[0], 0.], [0., 1., 0., 0.], f);
        }
        let side = (slot as f32 / 2.).ceil() * 2. * if slot % 2 == 1 { 1. } else { -1. };
        m[3][0] += side * m[0][0];
        m[3][1] += BAM_LIFT;
        m[3][2] += side * m[0][2];
        m
    }
}

/// Bug 11-01 (The Walrus again: "in a wall", then under the map), not retail: a BAM.loc locator's height isn't the
/// city map's either (observers float 3-18 m over their ground, and The Walrus' attempt locator lies 4.6 m under its
/// top landing, on a floor among walls with no collision, whose collision ends 1 m ahead), so [`Locator::spawn`] lifts
/// it this far: the teleport's ground probe (travel_ground, from 0.75 m over the target) lands on the landing.
pub(crate) const BAM_LIFT: f32 = 5.;

/// A row's place: a locator (spawn, indicator) or a trigger volume (its world box: the exporter's matrices are all
/// identity).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Place {
    Locator(Locator),
    Volume(Aabb),
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Row {
    pub id: String,
    /// The challenge type (8 Best Trick, 15 Jam, 9 Own The Spot, 4 S.K.A.T.E., 19 Death Race, 20 Spot Race).
    #[serde(rename = "type")]
    pub kind: u32,
    pub mode: String,
    pub template: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub places: BTreeMap<String, Place>,
    /// Places the row names that the exporter found in no level file.
    #[serde(default)]
    pub missing_places: Vec<String>,
    /// Each VisualIndicators record's type (old_online1.py; none in an install from before it).
    #[serde(default)]
    pub indicator_types: Vec<u32>,
    #[serde(rename = "self")]
    pub fields: Json,
}
impl Row {
    /// A row whose spot volume the exporter didn't find can't be scored; it isn't offered (none since it reads BAM.xsf's
    /// type-4 assets too).
    pub fn playable(&self) -> bool {
        !self.missing_places.iter().any(|p| p.to_ascii_lowercase().contains("spotvolume"))
    }
    /// The centre of the row's spot volumes (Own The Spot's OTSSpotVolumes, S.K.A.T.E.'s SpotVolumes).
    fn spot(&self) -> Option<[f32; 3]> {
        let names = self.fields["OTSSpotVolumes"].as_array().or_else(|| self.fields["SpotVolumes"].as_array())?;
        let boxes: Vec<&Aabb> = (names.iter())
            .filter_map(|n| match self.places.get(n.as_str()?) {
                Some(Place::Volume(v)) => Some(v),
                _ => None,
            })
            .collect();
        (!boxes.is_empty()).then(|| std::array::from_fn(|i| boxes.iter().map(|b| (b.min[i] + b.max[i]) / 2.).sum::<f32>() / boxes.len() as f32))
    }
}

pub(crate) struct Data {
    pub rows: BTreeMap<String, Row>,
    steps: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// `__enums`' tables: HUD.eSk8HUD*, Trick.eTrickType.
    pub enums: Json,
    /// modes.json: mode -> title, the event log strings, the HUD labels, Spot Race's objectives, the HUD enums.
    pub modes: Json,
    /// Flythrough scene -> seconds (intros/tutorial.json).
    pub scenes: HashMap<String, f32>,
    pub root: PathBuf,
    /// The race gate models (gates/gates.json: rg000 RaceGate, sg000 SpotGate); none in an install from before them.
    gates: BTreeMap<String, GateModel>,
    /// The engine's trick names (the Skate 3 HUD's ID_TRICK_* strings): the S.K.A.T.E. HUD names the tricks this
    /// game's scoring identifies, not Skate 1's.
    names: HashMap<String, String>,
}

/// A visual treatment model (old_online1_gates.py): a triangle list in model space and its RGBA page.
#[derive(Deserialize)]
pub(crate) struct GateModel {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    min: [f32; 3],
    max: [f32; 3],
    texture: String,
}
#[derive(Deserialize)]
struct GatesFile {
    models: BTreeMap<String, GateModel>,
}

impl Data {
    pub fn load(assets: &Path) -> Result<Self, String> {
        let root = assets.join(ROOT);
        let read = |name: &str| -> Result<Json, String> {
            let path = root.join(name);
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
                .map_err(|e| format!("{}: {e}", path.display()))
        };
        let file: RowsFile = serde_json::from_value(read("rows.json")?).map_err(|e| format!("rows.json: {e}"))?;
        let modes = read("modes.json")?;
        let scenes = read("intros/tutorial.json")?["scenes"]
            .as_object()
            .map(|s| s.iter().map(|(k, v)| (k.clone(), v["duration"].as_f64().unwrap_or(0.) as f32)).collect())
            .unwrap_or_default();
        let int = |v: &Json| json!(v.as_f64().unwrap_or(0.) as i64);
        let hud: serde_json::Map<String, Json> = (modes["enums"].as_object().into_iter().flatten())
            .filter(|(k, _)| k.starts_with("eSk8HUD"))
            .map(|(k, v)| (k.clone(), v.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), int(v))).collect()).unwrap_or_default()))
            .collect();
        let tricks: serde_json::Map<String, Json> = file.trick_types.iter().enumerate().map(|(i, t)| (t.clone(), json!(i))).collect();
        let enums = json!({"HUD": hud, "Trick": {"eTrickType": tricks}});
        // A suffixed event (timerexpired1) is its base event in that step.
        let steps = (file.scripts.into_iter())
            .map(|(t, s)| {
                let steps = (s.steps.into_iter())
                    .map(|(step, events)| (step, events.into_iter().map(|(e, f)| (e.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned(), f)).collect()))
                    .collect();
                (t, steps)
            })
            .collect();
        let rows = (file.modes.into_iter())
            .filter(|(mode, _)| Mode::parse(mode).is_some())
            .flat_map(|(_, rows)| rows)
            .map(|r| (r.id.clone(), r))
            .collect();
        let gates = read("gates/gates.json")
            .and_then(|g| serde_json::from_value::<GatesFile>(g).map_err(|e| format!("gates.json: {e}")))
            .inspect_err(|e| warn!("Skate 1 online: no race gate models, rings stand in ({e})"))
            .map_or_else(|_| BTreeMap::new(), |g| g.models);
        // As skate2_skate_game's with_skate3_tricks: the Skate 3 HUD's string table.
        let hud = assets.join("private/hud/runtime/trickdisplay.json");
        let names = (std::fs::read(&hud).ok().and_then(|b| serde_json::from_slice::<Json>(&b).ok()))
            .and_then(|j| j["language"].as_object().cloned())
            .map(|o| o.into_iter().filter(|(k, _)| k.starts_with("ID_TRICK_")).filter_map(|(k, v)| Some((k, v.as_str()?.to_owned()))).collect())
            .unwrap_or_default();
        Ok(Self { rows, steps, enums, modes, scenes, root, gates, names })
    }

    /// A scoring definition's label as text ([`trick_text`]).
    pub fn trick_name(&self, label: &str) -> String {
        trick_text(&self.names, label)
    }

    /// The localized title of a mode (modes.json titles: "Best Trick", "Deathrace", ...).
    pub fn title(&self, mode: &str) -> String {
        self.modes["titles"][mode].as_str().unwrap_or(mode).to_owned()
    }
}

// ---------------------------------------------------------------- the script
/// A trick label's name: the Skate 3 HUD's string, else the id itself read as words (ID_TRICK_DARKCATCH: DARKCATCH),
/// never a raw id. Not retail: Skate 1 named its own tricks (english.big, capitals); these are this game's scoring's
/// tricks, which Skate 1 never had (a darkcatch), named as old_hud.py names them for Skate 2.
pub(crate) fn trick_text(names: &HashMap<String, String>, label: &str) -> String {
    names.get(label).cloned().unwrap_or_else(|| label.trim_start_matches("ID_TRICK_").replace('_', " "))
}

/// A native call the script made (skate1_online.lua's record): `Table.Method` and its arguments (a table argument
/// is "<table>").
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Call {
    pub name: String,
    pub args: Vec<Json>,
}
impl Call {
    pub fn arg(&self, i: usize) -> &Json {
        self.args.get(i).unwrap_or(&Json::Null)
    }
    pub fn int(&self, i: usize) -> Option<i64> {
        self.arg(i).as_f64().map(|f| f as i64)
    }
    pub fn text(&self, i: usize) -> &str {
        self.arg(i).as_str().unwrap_or_default()
    }
    pub fn truthy(&self, i: usize) -> bool {
        !matches!(self.arg(i), Json::Null | Json::Bool(false))
    }
}

fn from_lua(v: &mlua::Value) -> Json {
    match v {
        mlua::Value::Boolean(b) => json!(b),
        mlua::Value::Integer(i) => json!(i),
        mlua::Value::Number(n) if n.fract() == 0. && n.abs() < 9e15 => json!(*n as i64),
        mlua::Value::Number(n) => json!(n),
        mlua::Value::String(s) => json!(s.to_string_lossy()),
        _ => Json::Null,
    }
}

/// One row's script on its own Lua: the clock, the timers and the queued events the C++ would send.
pub(crate) struct Session {
    machine: Machine,
    /// Refills the instruction budget before the binding's own calls (the machine refills it per event only).
    refill: Condition,
    steps: BTreeMap<String, BTreeMap<String, String>>,
    pub me: usize,
    pub clock: f32,
    pub tick: i64,
    /// (session seconds, event, message) the binding queued.
    later: Vec<(f32, String, Json)>,
}

/// The NIS-done notice: niscomplete, or nischallengenotify for the steps that wait on that (S.K.A.T.E.'s show_nis).
pub(crate) const NIS_DONE: &str = "@nis";

/// S.K.A.T.E.'s challenge type (rows.json `type`).
const SKATE: u32 = 4;
/// Skate 3's rule (user 10-28): stepping off the board in the spot fails a S.K.A.T.E. attempt (S3 skate.lua 619-627,
/// 1149-1156). The disc's script has no skateroffboard handler; skate1_online.lua's answer it in these steps.
const OFF_BOARD: [(&str, &str); 2] = [("leader_attempt", "__offboard_leader"), ("follower_attempt", "__offboard_follower")];

impl Session {
    pub fn new(data: &Data, row: &Row, names: &[String], me: usize, seed: u64) -> Result<Self, String> {
        let mut steps = data.steps.get(&row.template).ok_or_else(|| format!("{}: no steps for {}", row.id, row.template))?.clone();
        if row.kind == SKATE {
            for (state, function) in OFF_BOARD {
                if let Some(events) = steps.get_mut(state) {
                    events.insert("skateroffboard".into(), function.into());
                }
            }
        }
        let path = data.root.join(format!("scripts/{}.lua", row.template));
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        // `self.X.Count` / `self.X[...]` on a field the row leaves out reads an empty list (the vault's default).
        let mut fields = row.fields.clone();
        if let Some(map) = fields.as_object_mut() {
            for name in list_fields(&source) {
                map.entry(name).or_insert_with(|| json!([]));
            }
            // Spot Race's SpotTrickTypes is a vault eTrickType field: the scripts compare it with Trick.eTrickType's
            // numbers and the trickcomplete TrickType.
            if let Some(types) = map.get_mut("SpotTrickTypes").and_then(Json::as_array_mut) {
                for t in types.iter_mut().filter(|t| t.is_string()) {
                    *t = data.enums["Trick"]["eTrickType"][t.as_str().unwrap_or_default()].clone();
                }
            }
        }
        let n = names.len().max(1);
        let script = Script { name: &row.id, source: &source, steps: steps.clone(), initial: "initial", enter: "stateenter", exit: "stateexit" };
        let machine = Machine::new(script, &fields, |lua| {
            lua.load(NATIVES).set_name("@skate1_online").exec()?;
            let g = lua.globals();
            g.get::<mlua::Function>("__enums")?.call::<()>(to_lua(lua, &data.enums)?)?;
            let s: mlua::Table = g.get("S")?;
            s.set("n", n)?;
            s.set("me", me)?;
            // xorshift needs a non-zero seed; every player's is the match token's, so their draws agree.
            s.set("seed", (seed | 1) as i64 & 0x7fff_ffff_ffff)?;
            // Skate 3's rule (user 10-28): the random first turn (S.K.A.T.E.'s leader, Spot Battle's first player) is the
            // match token's draw, as skate3_skate_game::begin's (`seed % names.len()`), so every console's is the same.
            s.set("first", (seed % n as u64) as i64)?;
            // The challenge manager's records: the penalty per missed gate, the type (Own The Spot's finalize), which
            // comparator ranks the players (the +0xdad race flag: types 18-20) and a race's route.
            s.set("per_gate", row.fields["PenaltyPerMissedGate"].as_f64().unwrap_or(0.))?;
            s.set("kind", row.kind)?;
            s.set("race", matches!(row.kind, 18..=20))?;
            if let Some((points, lap, total)) = route(row) {
                let t = lua.create_table()?;
                for (k, (cum, mid)) in points.into_iter().enumerate() {
                    let p = lua.create_table()?;
                    p.set("cum", cum)?;
                    p.set("mid", mid.to_vec())?;
                    t.set(k, p)?;
                }
                s.set("route", t)?;
                s.set("lap", lap)?;
                s.set("total", total)?;
            }
            let t: mlua::Table = s.get("names")?;
            for (i, name) in names.iter().enumerate() {
                t.set(i, name.as_str())?;
            }
            Ok(())
        })?;
        // Skate 3's rules over the disc's handlers (skate1_online.lua RULES), inside the empty-message wrap below.
        let rule: mlua::Function = machine.lua.globals().get("__rule").map_err(|e| format!("{}: {e}", row.id))?;
        for (state, events) in &steps {
            for (event, function) in events {
                rule.call::<()>((state.as_str(), event.as_str(), function.as_str())).map_err(|e| format!("{}: {e}", row.id))?;
            }
        }
        // stateenter gets no message (nil); the handlers index theirs, so hand them an empty one.
        let functions: BTreeSet<&String> = steps.values().flat_map(|e| e.values()).collect();
        for f in functions {
            machine
                .lua
                .load(format!("_G[{f:?}] = (function(o) return function(s, m, v) return o(s, m or {{}}, v) end end)(_G[{f:?}])"))
                .exec()
                .map_err(|e| format!("{}: {e}", row.id))?;
        }
        let refill = Condition::compile(&machine.lua, "refill", "")?;
        Ok(Self { machine, refill, steps, me, clock: 0., tick: 0, later: Vec::new() })
    }

    pub fn state(&self) -> &str {
        self.machine.state()
    }
    pub fn handles(&self, event: &str) -> bool {
        self.steps.get(self.state()).is_some_and(|e| e.contains_key(event))
    }
    pub fn start(&mut self) -> Result<(), String> {
        self.machine.start()
    }
    pub fn send(&mut self, event: &str, message: &Json) -> Result<(), String> {
        let event = if event == NIS_DONE {
            if self.handles("niscomplete") || !self.handles("nischallengenotify") { "niscomplete" } else { "nischallengenotify" }
        } else {
            event
        };
        self.machine.send(event, message)
    }
    /// `event` after `seconds` of session time.
    pub fn later(&mut self, seconds: f32, event: &str, message: Json) {
        self.later.push((self.clock + seconds, event.to_owned(), message));
    }
    fn lua<T: mlua::FromLuaMulti>(&self, code: &str) -> Result<T, String> {
        self.refill.evaluate(&self.machine)?;
        self.machine.lua.load(code).eval::<T>().map_err(|e| e.to_string())
    }
    /// The race clock this player's relayed events carry (none while no race runs).
    fn race_clock(&self) -> Option<f64> {
        self.lua::<Option<f64>>("return __race_clock()").ok().flatten()
    }
    /// While a relayed event is delivered: its sender's race clock, which the challenge manager times it by.
    fn heard(&self, clock: Option<f64>) -> Result<(), String> {
        self.s()?.set("heard", clock).map_err(|e| e.to_string())
    }
    fn s(&self) -> Result<mlua::Table, String> {
        self.refill.evaluate(&self.machine)?;
        self.machine.lua.globals().get::<mlua::Table>("S").map_err(|e| e.to_string())
    }

    /// `dt` seconds on: the clock, every expired countdown's timerexpired, the queued events now due. Retail's
    /// ChallengeTimer counts 1/60 s ticks (0x823da1b0, 0x823d9fd8), so a long frame is caught up a tick at a time: a
    /// timer a handler re-arms keeps its spacing. Not retail (audit 10-16): no time is ever dropped, so a console that
    /// hitched ends its rounds when the others do (retail's fixed-rate console had no hitches to drop).
    pub fn step(&mut self, dt: f32) -> Result<(), String> {
        // Even steps of at most a tick (a frame's own dt is one step).
        let steps = (dt / NEXT_FRAME - 1e-3).ceil().max(1.);
        for _ in 0..steps as u32 {
            self.clock += dt / steps;
            self.tick = (f64::from(self.clock) * 60.).floor() as i64;
            let s = self.s()?;
            s.set("time", self.clock).map_err(|e| e.to_string())?;
            s.set("tick", self.tick).map_err(|e| e.to_string())?;
            while let Some(id) = self.lua::<Option<i64>>("return __next_expired()")? {
                self.machine.send("timerexpired", &json!({"PlayerID": self.me, "TimerID": id}))?;
            }
            while let Some(i) = self.later.iter().position(|(at, ..)| *at <= self.clock) {
                let (_, event, message) = self.later.remove(i);
                self.send(&event, &message)?;
            }
        }
        Ok(())
    }

    /// The calls recorded since the last drain.
    pub fn drain(&self) -> Result<Vec<Call>, String> {
        let calls: mlua::Table = self.lua("return __drain()")?;
        let mut out = Vec::new();
        for c in calls.sequence_values::<mlua::Table>() {
            let c = c.map_err(|e| e.to_string())?;
            let n: usize = c.get("n").unwrap_or(0);
            let name: String = c.get(1).map_err(|e| e.to_string())?;
            let args = (0..n).map(|i| c.get::<mlua::Value>(i + 2).map(|v| from_lua(&v)).unwrap_or(Json::Null)).collect();
            out.push(Call { name, args });
        }
        Ok(out)
    }
    /// The HUD countdown's seconds left (AddHUDCountdownTimer), if one runs.
    pub fn hud_time(&self) -> Option<f32> {
        self.lua::<Option<f64>>("return __hud_time()").ok().flatten().map(|t| t as f32)
    }
    /// A player left: Online.IsPlayerDisconnected answers true from now on, and a running challenge has them out.
    pub fn gone(&mut self, slot: usize) -> Result<(), String> {
        let gone: mlua::Table = self.s()?.get("gone").map_err(|e| e.to_string())?;
        gone.set(slot, true).map_err(|e| e.to_string())?;
        self.lua::<()>(&format!("__left({slot})"))?;
        self.send("playerdisconnected", &json!({"PlayerIndex": slot}))
    }
    /// The challenge manager's frame on the players' positions (by slot), then its order (slots, first place first)
    /// and each player's (finished, out, time with penalties, split).
    pub fn records(&self, positions: &[Option<[f32; 3]>]) -> Result<(Vec<usize>, Vec<(bool, bool, f32, f32)>), String> {
        self.refill.evaluate(&self.machine)?;
        let lua = &self.machine.lua;
        let at = lua.create_table().map_err(|e| e.to_string())?;
        for (i, p) in positions.iter().enumerate() {
            if let Some(p) = p {
                at.set(i, p.to_vec()).map_err(|e| e.to_string())?;
            }
        }
        lua.globals().get::<mlua::Function>("__frame").and_then(|f| f.call::<()>(at)).map_err(|e| e.to_string())?;
        let (order, finished, out, time, split): (Vec<usize>, Vec<bool>, Vec<bool>, Vec<f32>, Vec<f32>) = self.lua("return __standings()")?;
        Ok((order, (0..finished.len()).map(|i| (finished[i], out[i], time[i], split[i])).collect()))
    }
    /// A player's trick state as the Trick queries read it (`S.skaters[id]`).
    pub fn skater(&self, id: usize, state: &Json) -> Result<(), String> {
        let skaters: mlua::Table = self.s()?.get("skaters").map_err(|e| e.to_string())?;
        skaters.set(id, to_lua(&self.machine.lua, state).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
    /// Party Play's card answered (not retail: the disc has no Party Play): S.K.A.T.E.'s settle before the turn's unpin
    /// (the leader's 1 s NewWaitTimer, the follower's 2.5 s WaitTime) runs out now.
    pub fn settled(&mut self) -> Result<(), String> {
        let timer = match self.state() {
            "wait_to_enter_spot_volume" => "NewWaitTimer",
            "follower_wait" => "TimerID",
            _ => return Ok(()),
        };
        match self.var(timer).as_i64() {
            Some(id) => self.lua::<()>(&format!("__run_out({id})")),
            None => Ok(()),
        }
    }
    /// A script variable (tests, the HUD's round count).
    pub fn var(&self, name: &str) -> Json {
        self.machine.vars().get::<mlua::Value>(name).map(|v| from_lua(&v)).unwrap_or(Json::Null)
    }
    /// S.K.A.T.E.'s follower up (FollowerOrder[FollowerIndex]).
    fn follower(&self) -> Option<i64> {
        let vars = self.machine.vars();
        let order: mlua::Table = vars.get("FollowerOrder").ok()?;
        order.get::<Option<i64>>(vars.get::<i64>("FollowerIndex").ok()?).ok()?
    }
    /// S.K.A.T.E.'s "already been done" tricks ([trick, 180s], oldest first) and the place of the one last repeated.
    pub fn abds(&self) -> (Vec<(i64, i64)>, Option<usize>) {
        let Ok((list, hit)) = self.lua::<(Vec<Vec<i64>>, i64)>("return __abds()") else { return (Vec::new(), None) };
        (list.into_iter().map(|p| (p[0], p.get(1).copied().unwrap_or(0))).collect(), usize::try_from(hit).ok())
    }
}

/// A race row's route as SetupChallengeStatus (0x823daba8) builds it: per way point (0 the start line, the CheckPoints,
/// then the finish line unless the route loops) its distance along the route and its gate's middle; the lap's and the
/// race's (NumLaps of them when looping) length. The distance from gate to gate is between the volumes' min corners
/// (0x823dd470 reads the trigger's +0x40, the volume record's min); the race frame aims at the middle.
pub(crate) fn route(row: &Row) -> Option<(Vec<(f32, [f32; 3])>, f32, f32)> {
    let volume = |name: &Json| match row.places.get(name.as_str()?) {
        Some(Place::Volume(v)) => Some(*v),
        _ => None,
    };
    let looping = row.fields["IsLooping"].as_bool().unwrap_or(false);
    let mut gates = vec![volume(&row.fields["StartLine"])?];
    for c in row.fields["CheckPoints"].as_array()? {
        gates.push(volume(c)?);
    }
    let mid = |v: &Aabb| -> [f32; 3] { std::array::from_fn(|i| (v.min[i] + v.max[i]) / 2.) };
    let far = |a: &Aabb, b: &Aabb| Vec3::from(a.min).distance(Vec3::from(b.min));
    let mut points = vec![(0., mid(&gates[0]))];
    for w in gates.windows(2) {
        points.push((points[points.len() - 1].0 + far(&w[0], &w[1]), mid(&w[1])));
    }
    let finish = volume(&row.fields["FinishLine"]);
    let mut lap = points[points.len() - 1].0;
    if let (Some(f), true) = (&finish, gates.len() > 1) {
        lap += far(&gates[gates.len() - 1], f);
    }
    if looping {
        points[0].0 = lap;
        return Some((points, lap, row.fields["NumLaps"].as_f64().unwrap_or(1.) as f32 * lap));
    }
    points.push((lap, mid(&finish?)));
    Some((points, lap, lap))
}

/// The `self` fields a script reads as lists (`self.X.Count`, `self.X[`).
fn list_fields(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, _) in source.match_indices("self.") {
        let rest = &source[i + 5..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let after = &rest[end..];
        if end > 0 && (after.starts_with(".Count") || after.starts_with('[')) {
            out.insert(rest[..end].to_owned());
        }
    }
    out
}

// ---------------------------------------------------------------- relay
/// An event for every other player's script: `{"e": event, "m": message, "k": the sender's tick}`. Ticks in the
/// message (a sequence's StartTick) are the sender's clock; [`decode`] moves them onto the receiver's.
pub(crate) fn encode(event: &str, message: &Json, tick: i64) -> String {
    json!({"e": event, "m": message, "k": tick}).to_string()
}
/// A peer's event is its word: only a [`RELAYED`] one whose fields are all ones the probe writes, of their type (a
/// script or native indexing or adding a wrong one raises a Lua error, which ends this player's match), from its slot.
pub(crate) fn decode(text: &str, slot: usize, tick: i64) -> Option<(String, Json)> {
    let v: Json = serde_json::from_str(text).ok()?;
    let event = v["e"].as_str().filter(|e| RELAYED.contains(e))?.to_owned();
    let sent = v["k"].as_i64().unwrap_or(tick);
    let mut m = v["m"].as_object().filter(|m| m.iter().all(|(k, v)| fits(k, v)))?.clone();
    m.insert("PlayerID".into(), json!(slot));
    for key in ["StartTick", "StartSimGameTick"] {
        if let Some(t) = m.get(key).and_then(Json::as_i64) {
            m.insert(key.into(), json!(t.saturating_sub(sent).saturating_add(tick)));
        }
    }
    Some((event, Json::Object(m)))
}

/// A relayed message's field (and its `"S"` trick state's) as the probe writes it.
fn fits(key: &str, v: &Json) -> bool {
    match key {
        "PlayerID" => true,
        "VolumeID" => v.is_string(),
        "TrickType" | "FirstScorableId" | "SequenceScore" | "StartTick" | "StartSimGameTick" | "BoundaryID" | "spins" | "first_type" => v.is_i64(),
        "Wipedout" => v.is_boolean() || v.is_i64(),
        RACE_CLOCK => v.is_number(),
        "InBoundary" | "air" | "trick" | "grab" | "launch" | "switch" | MARKER_USED => v.is_boolean(),
        "first" => v.is_null() || v.is_i64(),
        "label" | "id" => v.is_null() || v.is_string(),
        "types" => v.as_object().is_some_and(|t| t.values().all(Json::is_boolean)),
        "S" => v.as_object().is_some_and(|s| s.iter().all(|(k, v)| k != "S" && fits(k, v))),
        _ => false,
    }
}

/// A relayed event's field with its sender's race clock (seconds since ChallengeStart).
const RACE_CLOCK: &str = "RaceClock";
/// Skate 3's rule (user 10-28): a sequence after a session-marker return carries this, true (S3 contest_shared 1091).
const MARKER_USED: &str = "UsedSessionMarker";

/// Events the local player's probe makes that every script hears. Each carries the player's trick state at the
/// time (`"S"`, which the Trick queries read for that player: [`Run::deliver`]). skateroffboard is Skate 3's (user
/// 10-28: S.K.A.T.E.'s step off the board); an older build drops it.
pub(crate) const RELAYED: [&str; 8] = [
    "triggerentercollision",
    "triggerexitcollision",
    "sequencecomplete",
    "wipeout",
    "aircompleted",
    "trickcomplete",
    "boundarytriggered",
    "skateroffboard",
];

// ---------------------------------------------------------------- standings
/// What the scripts report of each player (Contest / ChallengeManager / Skate calls), for the HUD and the result.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Standing {
    /// Best Trick / Jam: the round score; Spot Battle / Spot Race: SetPlayerScore.
    pub score: i64,
    /// Best Trick / Jam: the contest points; S.K.A.T.E.: wins.
    pub points: i64,
    pub rank: i64,
    /// Races (the challenge manager's records): finished, out (DNF / DQ / left), the race clock at the end plus the
    /// penalty, the gap to the leader (0x823dbe80's split); gates missed, penalty seconds.
    pub finished: bool,
    pub out: bool,
    pub time: f32,
    pub split: f32,
    pub missed: i64,
    pub penalty: f32,
    /// S.K.A.T.E.: letters (Skate.SetPlayerStatus).
    pub letters: i64,
}

// ---------------------------------------------------------------- the run (no Bevy)
/// A running match: the script and what its calls asked of the world, which the plugin applies.
pub(crate) struct Run {
    pub session: Session,
    pub row: String,
    pub mode: Mode,
    pub names: Vec<String>,
    /// AI.ForceLocalPlayerStop / ForceLocalPlayerAndAIStop, AI.PinAI on this player.
    pub stop: bool,
    pub pin: bool,
    pub teleport: Option<[[f32; 4]; 4]>,
    /// The flythrough playing: (scene, ends at session seconds).
    pub scene: Option<(String, f32)>,
    /// VisualIndicators shown (1-based).
    pub indicators: BTreeSet<usize>,
    /// The player the camera follows (Online.SwitchCamera; None: this one).
    pub camera: Option<usize>,
    /// Races: this player's next way point (ChallengeManager.NotifyPlayerNextWayPoint: 0 the start line, 1.. the
    /// CheckPoints, then the finish line); None once finished or out.
    pub next_gate: Option<usize>,
    /// Races: the way points shown ahead of this player ([`upcoming`]), and its laps done as they read them.
    pub gates: Vec<usize>,
    laps: i64,
    /// The challenge manager's order (slots, first place first: GetPlayerWon's first).
    pub order: Vec<usize>,
    /// Where each player is (by slot) for the races' estimate; the binding fills it each frame.
    pub positions: Vec<Option<[f32; 3]>>,
    /// HUD calls for skate1_online_hud, in order.
    pub hud: Vec<Call>,
    pub standings: Vec<Standing>,
    /// Races: the row's PenaltyPerMissedGate (seconds).
    per_gate: f32,
    pub left: bool,
    /// Pause.DisablePauseMenuAndBackpack is in force (EnablePauseMenuAndBackpack lifts it).
    pub pause_disabled: bool,
    /// Events for the other players.
    pub outbox: Vec<String>,
    /// Scorable id -> trick name, as the players' aircompleted reported them (the S.K.A.T.E. HUD's trick names).
    pub labels: BTreeMap<i64, String>,
    /// Scorable id -> its stick gesture (the S.K.A.T.E. circle's mcGesture), from the same reports.
    pub gestures: BTreeMap<i64, &'static str>,
    gone: BTreeSet<usize>,
    /// ChallengeBoundary.AddVTBoundaryArea: (boundary, trigger volume). Spot Battle's OTSTriggerBoundary.
    boundaries: Vec<(i64, String)>,
    /// Challenge.SetNewSessionMarker on this player not yet applied (S1 0x827AF2B8: cMsgSetSessionMarker at its
    /// skater; Best Trick's event_round, each round).
    pub markers: u32,
}

impl Run {
    pub fn new(data: &Data, row: &Row, names: Vec<String>, me: usize, seed: u64) -> Result<Self, String> {
        let mode = Mode::parse(&row.mode).ok_or_else(|| format!("{}: mode {}", row.id, row.mode))?;
        let session = Session::new(data, row, &names, me, seed)?;
        Ok(Self {
            session,
            row: row.id.clone(),
            mode,
            standings: vec![Standing::default(); names.len().max(1)],
            names,
            stop: false,
            pin: false,
            teleport: None,
            scene: None,
            indicators: BTreeSet::new(),
            camera: None,
            next_gate: None,
            gates: Vec::new(),
            laps: 0,
            order: Vec::new(),
            positions: Vec::new(),
            hud: Vec::new(),
            per_gate: row.fields["PenaltyPerMissedGate"].as_f64().unwrap_or(0.) as f32,
            left: false,
            pause_disabled: false,
            outbox: Vec::new(),
            labels: BTreeMap::new(),
            gestures: BTreeMap::new(),
            gone: BTreeSet::new(),
            boundaries: Vec::new(),
            markers: 0,
        })
    }
    pub fn me(&self) -> usize {
        self.session.me
    }
    /// Audit 10-28 GAP-9: a race's spectator (camera on another player, Online.SwitchCamera) moves to the next racer
    /// (RT, input.cfg SpectatorNext = RTrigger.pressed) or the one before (LT, SpectatorPrev). INFERRED: the C++
    /// consumer is UNRECOVERED; the cycle keeps the scripts' own pick (FindAnActiveSkater: unfinished, connected
    /// skaters in slot order), wrapping. SpectatorFree (X) isn't done.
    pub fn spectate(&mut self, forward: bool) {
        let Some(at) = self.camera.filter(|_| matches!(self.mode, Mode::DeathRace | Mode::SpotRace)) else { return };
        let me = self.me();
        let racing: Vec<bool> = (0..self.names.len())
            .map(|i| i != me && !self.gone.contains(&i) && self.standings.get(i).is_some_and(|s| !s.finished && !s.out))
            .collect();
        if let Some(next) = next_watched(at, &racing, forward) {
            self.camera = Some(next);
        }
    }
    pub fn start(&mut self, data: &Data) -> Result<(), String> {
        self.session.start()?;
        self.apply(data)
    }
    /// One frame: `dt` seconds on, the local events (relayed too), the other players' (`inbox`) and leavers.
    pub fn frame(&mut self, data: &Data, dt: f32, local: Vec<(String, Json)>, inbox: Vec<(usize, String)>, players: &[String]) -> Result<(), String> {
        self.session.step(dt)?;
        self.apply(data)?;
        for (event, mut message) in local {
            if RELAYED.contains(&event.as_str()) {
                // Not retail (user 10-12): the race clock rides along (skate1_online.lua's clock()).
                if let (Some(clock), Some(m)) = (self.session.race_clock(), message.as_object_mut()) {
                    m.insert(RACE_CLOCK.into(), json!(clock));
                }
                self.outbox.push(encode(&event, &message, self.session.tick));
            }
            self.deliver(data, &event, &message)?;
        }
        for (slot, text) in inbox {
            if let Some((event, message)) = decode(&text, slot, self.session.tick) {
                self.session.heard(message.get(RACE_CLOCK).and_then(Json::as_f64))?;
                let delivered = self.deliver(data, &event, &message);
                self.session.heard(None)?;
                delivered?;
            }
        }
        for (slot, name) in players.iter().enumerate() {
            if name.is_empty() && slot != self.me() && self.gone.insert(slot) {
                self.session.gone(slot)?;
            }
        }
        self.apply(data)?;
        let (order, records) = self.session.records(&self.positions)?;
        self.order = order;
        if matches!(self.mode, Mode::DeathRace | Mode::SpotRace) {
            for (s, (finished, out, time, split)) in self.standings.iter_mut().zip(records) {
                (s.finished, s.out, s.time, s.split) = (finished, out, time, split);
            }
        }
        Ok(())
    }
    fn deliver(&mut self, data: &Data, event: &str, message: &Json) -> Result<(), String> {
        // ponytail: another player's trick state is as of their last event (the scripts query it on those events).
        if let (Some(state), Some(id)) = (message.get("S"), message["PlayerID"].as_u64()) {
            self.session.skater(id as usize, state)?;
            if let (Some(first), Some(label)) = (state["first"].as_i64(), state["label"].as_str()) {
                self.labels.insert(first, data.trick_name(label));
            }
            if let (Some(first), Some(id)) = (state["first"].as_i64(), state["id"].as_str()) {
                self.gestures.insert(first, crate::skate1_missions::gesture_of(id));
            }
        }
        self.session.send(event, message)?;
        // A trigger-volume boundary's player crossing its volume: cMsgBoundaryTriggered after the collision.
        let inside = match event {
            "triggerentercollision" => true,
            "triggerexitcollision" => false,
            _ => return Ok(()),
        };
        let volume = message["VolumeID"].as_str().unwrap_or_default();
        let ids: Vec<i64> = self.boundaries.iter().filter(|(_, v)| v == volume).map(|(id, _)| *id).collect();
        for id in ids {
            self.session.send("boundarytriggered", &json!({"BoundaryID": id, "PlayerID": message["PlayerID"], "InBoundary": inside}))?;
        }
        Ok(())
    }

    /// Applies the calls the script made: the world's part here, the HUD's queued in `hud`.
    pub fn apply(&mut self, data: &Data) -> Result<(), String> {
        let row = data.rows.get(&self.row).ok_or("row gone")?;
        let me = self.me() as i64;
        for call in self.session.drain()? {
            let ours = |i: usize| call.int(i) == Some(me);
            match call.name.as_str() {
                // (player, place[, slot]): the slot is the player's own unless given (Spot Battle's attempt spawn 0,
                // S.K.A.T.E.'s waiting id): child[slot] (S1 0x823d9088), never the root.
                "AI.RepositionAI" if ours(0) => {
                    match row.places.get(call.text(1)) {
                        Some(Place::Locator(l)) => self.teleport = Some(l.spawn(call.int(2).map_or(self.me(), |s| s.max(0) as usize), row)),
                        _ => warn!("Skate 1 online: {}: no locator {}", row.id, call.text(1)),
                    }
                }
                // AI.ResetPlayersPosition(start, location): everyone back on the start line, each on their slot.
                "AI.ResetPlayersPosition" => match row.places.get(call.text(0)) {
                    Some(Place::Locator(l)) => self.teleport = Some(l.spawn(self.me(), row)),
                    _ => warn!("Skate 1 online: {}: no locator {}", row.id, call.text(0)),
                },
                // AI.SpawnSkaterRegular (S.K.A.T.E.'s follower spawned in the leader's stance, SKATE 451-458, 1116) is not
                // applied. Skate 3's rule (user 10-28): the follower copies in its own stance, as Skate 3's skate.lua.
                "AI.ForceLocalPlayerStop" | "AI.ForceLocalPlayerAndAIStop" => self.stop = call.truthy(0),
                "AI.PinAI" if ours(0) => self.pin = true,
                "AI.UnpinAI" if ours(0) => self.pin = false,
                "NIS.EnterFlythroughNIS" => {
                    let scene = call.text(0).to_owned();
                    match data.scenes.get(&scene) {
                        Some(&seconds) => {
                            // It runs to its camera's end on its clock (nis-clock.md 10), not its record's length.
                            let intros = data.root.join("intros");
                            let seconds = crate::challenge::cutscene::real_length(&intros, &scene).unwrap_or(seconds);
                            self.scene = Some((scene, self.session.clock + seconds));
                            self.session.later(seconds, NIS_DONE, json!({}));
                        }
                        None => {
                            warn!("Skate 1 online: no flythrough {scene}");
                            self.session.later(NEXT_FRAME, NIS_DONE, json!({}));
                        }
                    }
                }
                // ponytail: no replay recorder / basic NIS player for these; the script goes on as if it had played.
                "NIS.EnterReplayNIS" | "NIS.EnterBasicNIS" => self.session.later(NEXT_FRAME, NIS_DONE, json!({})),
                "Challenge.SetChallengeVisualIndicatorByIndex" => drop(call.int(1).map(|i| self.indicators.insert(i as usize))),
                "Challenge.RemoveChallengeVisualIndicatorByIndex" => drop(call.int(1).map(|i| self.indicators.remove(&(i as usize)))),
                "Challenge.SetAllChallengeVisualIndicators" => {
                    let n = row.fields["VisualIndicators"].as_array().map_or(0, Vec::len);
                    self.indicators = (1..=n).collect();
                }
                "Challenge.RemoveAllChallengeVisualIndicators" => self.indicators.clear(),
                "ChallengeBoundary.AddVTBoundaryArea" => drop(call.int(0).map(|id| self.boundaries.push((id, call.text(1).to_owned())))),
                "ChallengeBoundary.DestoryBoundary" => self.boundaries.retain(|(id, _)| Some(*id) != call.int(0)),
                // S.K.A.T.E.'s winner state ends with Online.OnlineChallengeComplete and waits on the same notice.
                "ChallengeManager.FinalizeChallengeResult" | "Online.OnlineChallengeComplete" => {
                    self.session.later(NEXT_FRAME, "finalresultsready", json!({}))
                }
                "Online.SwitchCamera" | "ChallengeManager.SwitchCamera" => {
                    self.camera = call.int(0).map(|i| i as usize).filter(|&i| i != self.me());
                }
                "Online.LeaveGame" => self.left = true,
                "Challenge.SetNewSessionMarker" if ours(0) => self.markers += 1,
                "Pause.DisablePauseMenuAndBackpack" => self.pause_disabled = true,
                "Pause.EnablePauseMenuAndBackpack" => self.pause_disabled = false,
                "ChallengeManager.NotifyPlayerNextWayPoint" if ours(0) => self.next_gate = call.int(1).and_then(|g| usize::try_from(g).ok()),
                "ChallengeManager.NotifyLapFinished" if ours(0) => self.laps = call.int(1).unwrap_or(0),
                "ChallengeManager.NotifyPlayerHitWayPoint" if ours(0) => {
                    self.gates = call.int(1).and_then(|g| usize::try_from(g).ok()).map(|hit| upcoming(row, self.mode, hit, self.laps)).unwrap_or_default()
                }
                // ponytail: a DNF / DQF clearing the gates is assumed (a finish hides them by upcoming's own rule).
                "ChallengeManager.NotifyPlayerFinished" | "ChallengeManager.NotifyPlayerDNF" | "ChallengeManager.NotifyPlayerDQF" if ours(0) => {
                    self.next_gate = None;
                    self.gates.clear();
                }
                _ => {}
            }
            self.standing(&call);
            if ["HUD.", "Contest.", "Skate.", "ChallengeManager."].iter().any(|p| call.name.starts_with(p)) {
                self.hud.push(call);
            }
        }
        if let Some((_, ends)) = &self.scene {
            if self.session.clock >= *ends {
                self.scene = None;
            }
        }
        Ok(())
    }

    fn standing(&mut self, call: &Call) {
        let at = |i: Option<i64>, standings: &mut Vec<Standing>| -> Option<usize> {
            let i = usize::try_from(i?).ok()?;
            if i >= standings.len() {
                standings.resize(i + 1, Standing::default());
            }
            Some(i)
        };
        let per_gate = self.per_gate;
        let s = &mut self.standings;
        match call.name.as_str() {
            // (self, player 1-based, score, total points, rank)
            "Contest.SetOverallStanding" => {
                if let Some(i) = at(call.int(1).map(|i| i - 1), s) {
                    (s[i].score, s[i].points, s[i].rank) = (call.int(2).unwrap_or(0), call.int(3).unwrap_or(0), call.int(4).unwrap_or(0));
                }
            }
            "ChallengeManager.SetPlayerScore" => drop(at(call.int(0), s).map(|i| s[i].score = call.int(1).unwrap_or(0))),
            // (player, missed): the penalty is the row's PenaltyPerMissedGate each.
            "ChallengeManager.NotifyNumMissedWayPoint" => {
                if let Some(i) = at(call.int(0), s) {
                    s[i].missed = call.int(1).unwrap_or(0);
                    s[i].penalty = per_gate * s[i].missed as f32;
                }
            }
            "Skate.SetPlayerRank" => drop(at(call.int(0), s).map(|i| s[i].rank = call.int(1).unwrap_or(0))),
            "Skate.SetPlayerStatus" => drop(at(call.int(0), s).map(|i| s[i].letters = call.int(1).unwrap_or(0))),
            "Skate.SetRoundTime" => drop(at(call.int(0), s).map(|i| s[i].time = call.arg(1).as_f64().unwrap_or(0.) as f32)),
            _ => {}
        }
    }

    /// This player's result for the match core: race time in ms (DNF when out), S.K.A.T.E. minus its letters,
    /// else the contest points (Best Trick / Jam) or the score.
    pub fn result(&self) -> i64 {
        let s = self.standings.get(self.me()).cloned().unwrap_or_default();
        match self.mode {
            Mode::DeathRace | Mode::SpotRace if s.out || !s.finished => crate::match_core::DNF,
            Mode::DeathRace | Mode::SpotRace => (s.time * 1000.).round() as i64,
            Mode::Skate => -s.letters,
            Mode::BestTrick | Mode::Jam => s.points,
            _ => s.score,
        }
    }
    pub fn complete(&self) -> bool {
        self.session.state() == "complete" || self.left
    }
}

// ---------------------------------------------------------------- Party Play
/// Party Play on one pad, Skate 3's rules (user 10-28): every player's console runs the row's script as online, in
/// lockstep, each one's events relayed to the others at once. The pad drives the player whose turn it is ([`turn`]):
/// that console sits in [`Skate1Online::run`], so the HUD, the camera and the skater follow it; the others' moves
/// only place their waiting puppets. A new turn opens the Press-A card, and no console's clock runs until its A.
pub(crate) struct Party {
    /// Each player's console, but the pad's (None), by player.
    pub consoles: Vec<Option<Run>>,
    /// Each console's events from the others, for its next frame.
    mail: Vec<Vec<(usize, String)>>,
    /// Where each player was last put (its script's RepositionAI; the pad's last player: where the pad left it).
    pub at: Vec<Option<[[f32; 4]; 4]>>,
    /// The Press-A card to open (player 1-based, round), taken once.
    pub card: Option<(usize, i64)>,
    /// The card is up: no time passes on any console until its A.
    pub waiting: bool,
    /// The pad's console's state last frame; S.K.A.T.E.'s rounds begun (its script counts none).
    state: String,
    rounds: i64,
}

impl Party {
    /// Every player's console on `row`, started; the first is the pad's.
    pub fn new(data: &Data, row: &Row, names: Vec<String>, seed: u64) -> Result<(Run, Self), String> {
        let n = names.len().max(1);
        let mut consoles = Vec::with_capacity(n);
        for me in 0..n {
            let mut run = Run::new(data, row, names.clone(), me, seed)?;
            run.start(data)?;
            consoles.push(Some(run));
        }
        let run = consoles[0].take().ok_or("no players")?;
        let party = Self { consoles, mail: vec![Vec::new(); n], at: vec![None; n], card: None, waiting: false, state: String::new(), rounds: 0 };
        Ok((run, party))
    }

    /// One frame of every console: the pad's `run` with the pad's `local` events, then the others'. A turn's player
    /// takes the pad (and `run`; true then), the one it leaves standing at `here` unless its script just moved it.
    pub fn frame(&mut self, run: &mut Run, data: &Data, dt: f32, local: Vec<(String, Json)>, here: [[f32; 4]; 4]) -> Result<bool, String> {
        let dt = if self.waiting { 0. } else { dt };
        let (names, me) = (run.names.clone(), run.me());
        run.frame(data, dt, local, std::mem::take(&mut self.mail[me]), &names)?;
        let out = std::mem::take(&mut run.outbox);
        self.post(me, &out);
        for i in 0..self.consoles.len() {
            let mail = std::mem::take(&mut self.mail[i]);
            let Some(other) = self.consoles[i].as_mut() else { continue };
            // Only this frame's HUD calls are kept: the HUD takes them if this player's turn begins now.
            (other.hud, other.markers) = (Vec::new(), 0);
            other.frame(data, dt, Vec::new(), mail, &names)?;
            let out = std::mem::take(&mut other.outbox);
            if let Some(at) = other.teleport.take() {
                self.at[i] = Some(at);
            }
            self.post(i, &out);
        }
        let next = turn(run).filter(|&p| p != me && self.consoles.get(p).is_some_and(Option::is_some));
        if let Some(p) = next {
            let new = self.consoles[p].take().expect("checked");
            let mut old = std::mem::replace(run, new);
            self.at[me] = Some(old.teleport.take().unwrap_or(here));
            self.consoles[me] = Some(old);
            run.teleport = self.at[p];
        }
        let state = run.session.state().to_owned();
        let entered = state != self.state;
        self.state = state;
        if entered && turn_starts(run.mode, &self.state) {
            self.rounds += i64::from(self.state == "wait_to_enter_spot_volume");
            let round = if run.mode == Mode::SpotBattle { run.session.var("CurrentRound").as_i64().unwrap_or(1) } else { self.rounds };
            if let Some(p) = turn(run) {
                (self.card, self.waiting) = (Some((p + 1, round)), true);
            }
        }
        Ok(next.is_some())
    }

    /// The card's A: time runs again, and a S.K.A.T.E. turn starts unpinned at once, as Skate 3's (skate.lua 359-371,
    /// 828-834 UnpinAI on the card's hudcomplete): the disc's settle before its unpin runs out now on every console.
    pub fn answer(&mut self, run: &mut Run) -> Result<(), String> {
        self.waiting = false;
        for r in std::iter::once(run).chain(self.consoles.iter_mut().flatten()) {
            r.session.settled()?;
        }
        Ok(())
    }

    fn post(&mut self, from: usize, out: &[String]) {
        for (_, mail) in self.mail.iter_mut().enumerate().filter(|(to, _)| *to != from) {
            mail.extend(out.iter().map(|text| (from, text.clone())));
        }
    }
}

/// Whose turn the script is on (None between turns): Spot Battle's ActiveSkaterIdx (1-based) through challenge_setup's
/// countdown and entered_trick_boundary; S.K.A.T.E.'s leader (LeaderID) through challenge_setup, wait_to_enter_spot_volume
/// and leader_attempt, each follower (FollowerOrder[FollowerIndex]) through follower_wait, wait_to_enter_spot2 and
/// follower_attempt.
fn turn(run: &Run) -> Option<usize> {
    let s = &run.session;
    let int = |name: &str| s.var(name).as_i64();
    let p = match (run.mode, s.state()) {
        (Mode::SpotBattle, "challenge_setup" | "entered_trick_boundary") => int("ActiveSkaterIdx")? - 1,
        (Mode::Skate, "challenge_setup" | "wait_to_enter_spot_volume" | "leader_attempt") => int("LeaderID")?,
        (Mode::Skate, "follower_wait" | "wait_to_enter_spot2" | "follower_attempt") => s.follower()?,
        _ => return None,
    };
    usize::try_from(p).ok()
}

/// A turn's first state: its card then (Spot Battle's countdown, the S.K.A.T.E. leader's and each follower's wait).
fn turn_starts(mode: Mode, state: &str) -> bool {
    matches!((mode, state), (Mode::SpotBattle, "challenge_setup") | (Mode::Skate, "wait_to_enter_spot_volume" | "follower_wait"))
}

/// The results dialog's rows (party_play::standings), best first: Spot Battle by score (ties share a rank), S.K.A.T.E.
/// by its script's ranks with each player's letters.
pub(crate) fn party_results(run: &Run, data: &Data) -> Vec<(String, Json)> {
    let skate = run.mode == Mode::Skate;
    let s = |p: usize| run.standings.get(p).cloned().unwrap_or_default();
    let value = |p: usize| if skate { s(p).letters } else { s(p).score };
    let n = run.names.len();
    let rank = |p: usize| if skate { s(p).rank.max(1) } else { 1 + (0..n).filter(|&q| value(q) > value(p)).count() as i64 };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&p| rank(p));
    let rows: Vec<_> = order.into_iter().map(|p| (p, run.names[p].clone(), rank(p), value(p))).collect();
    let title = data.rows.get(&run.row).map_or("", |r| r.name.as_str());
    crate::party_play::standings(title, &rows, "ID_COMMON_SCORE", if skate { 2 } else { 0 })
}

// ---------------------------------------------------------------- the local player
/// The physics ticks' view of the local player ([`probe`] fills it; [`drive`] takes the events).
#[derive(Default)]
struct Probe {
    live: bool,
    volumes: Vec<(String, Aabb)>,
    inside: BTreeSet<usize>,
    /// Physics seconds since live.
    clock: f32,
    open: Option<f32>,
    air: bool,
    /// The last landed air's first scorable (definition index), whether it had a grab, its half turns, whether it
    /// took off switch.
    first: Option<usize>,
    grab: bool,
    spins: i64,
    switch: bool,
    /// Riding switch as the air under way took off.
    took_off_switch: bool,
    /// The air under way: the tricks announced through it ([`Probe::air_step`]).
    tricks: Option<Vec<&'static str>>,
    /// A touchdown settling: (seconds left, the air's tricks, its half-turns).
    landing: Option<(f32, Vec<&'static str>, i32)>,
    wiping: bool,
    /// Off the board (the biped's physical category, 500) last tick.
    off: bool,
    /// SessionMarker::uses as last seen, and a return since the last sequence ended ([`Probe::note_marker`]).
    uses: Option<u64>,
    marked: bool,
    /// (event, message, seconds ago its StartTick was): taken each frame.
    events: Vec<(String, Json, f32)>,
    kinds: Vec<u32>,
    /// The definitions' labels (the trick names the S.K.A.T.E. HUD shows).
    labels: Vec<String>,
    /// The definitions' identifiers (the S.K.A.T.E. circle's gesture), and back.
    idents: Vec<&'static str>,
    ids: HashMap<&'static str, usize>,
    pos: [f32; 3],
    /// The trick state the Trick queries read (`S.skaters[me]`).
    state: Json,
}
impl Probe {
    /// Party Play's handover: the next player rides the same skater, so its physical state stays (air, bail, off the
    /// board); what it meant for the last player (the volumes it was in, the open sequence, its air's tricks, a marker
    /// return) goes.
    // shortcut: the last player gets no triggerexitcollision for the volumes it was in; its script moves it to its
    // waiting spot and no Spot Battle / S.K.A.T.E. handler counts entries. Send them if a mode ever does.
    fn handed_over(&mut self) {
        let old = std::mem::take(self);
        *self = Self {
            live: old.live,
            volumes: old.volumes,
            clock: old.clock,
            air: old.air,
            wiping: old.wiping,
            off: old.off,
            uses: old.uses,
            kinds: old.kinds,
            labels: old.labels,
            idents: old.idents,
            ids: old.ids,
            pos: old.pos,
            state: old.state,
            ..default()
        };
    }
    /// One tick of an air as Skate 3's S.K.A.T.E. reads it (skate3_skate_game's Probe): every trick announced through
    /// it and while it settles (the scoring announces a trick on its frame only, a flip often as the board lands),
    /// handed back with the air's half-turns LANDING_GRACE after touchdown; a bail or stepping off (`dropped`) loses it.
    fn air_step(&mut self, announced: &[&'static str], air: bool, turns: i32, dt: f32, dropped: bool) -> Option<(Vec<&'static str>, i32)> {
        if air {
            self.tricks.get_or_insert_with(Vec::new).extend(announced);
        } else if let Some(mut tricks) = self.tricks.take() {
            tricks.extend(announced);
            self.landing = Some((crate::skate3_skate_game::LANDING_GRACE, tricks, turns));
        } else if let Some((left, tricks, _)) = &mut self.landing {
            tricks.extend(announced);
            *left -= dt;
        }
        if dropped {
            (self.tricks, self.landing) = (None, None);
        }
        if self.landing.as_ref().is_some_and(|l| l.0 <= 0.) {
            let (_, tricks, turns) = self.landing.take()?;
            return Some((tricks, turns));
        }
        None
    }
    /// Skate 3's rule (user 10-28): a session-marker return (SessionMarker::uses moving on) flags the next sequence
    /// UsedSessionMarker. A return is a teleport, which wipes the open sequence (scoring_runtime reset_module), so the
    /// flag can only be on the one after.
    fn note_marker(&mut self, uses: Option<u64>) {
        let Some(uses) = uses else { return };
        self.marked |= self.uses.is_some_and(|was| was != uses);
        self.uses = Some(uses);
    }
    /// The trick state every event carries (`"S"`); a peer drops an event with a field [`fits`] doesn't know.
    fn trick_state(&self, air: bool, open: bool, types: serde_json::Map<String, Json>) -> Json {
        let first_type = self.first.map_or(0, |i| skate1_trick_type(self.kinds[i]));
        json!({
            // IsInAirWithValidLaunchTrick: Skate 3's "left the spot and not tricking" (skate3_skate_game Watch): in the
            // air or settling, whatever it has announced yet.
            "air": air, "trick": open, "types": types, "grab": self.grab, "spins": self.spins, "launch": air || self.landing.is_some(),
            "first": self.first, "first_type": first_type, "label": self.first.map(|i| self.labels[i].clone()),
            "id": self.first.map(|i| self.idents[i]), "switch": self.switch,
        })
    }
}

/// Skate 3's TrickType (scoring definitions) as Skate 1's Trick.eTrickType index (rows.json trick_types: None Air
/// Flip Gap Grab Grind Invert Lip Manual Slide Spin Revert).
// ponytail: Skate 3's extra kinds folded onto the nearest Skate 1 one (finger flip a flip, no comply / hippy jump /
// boneless an air, handplant an invert, footplant a lip).
pub(crate) fn skate1_trick_type(skate3: u32) -> i64 {
    match skate3 {
        1 | 7 | 10 | 12 => 1,
        2 | 3 => 2,
        4 => 4,
        5 => 5,
        6 => 6,
        13 => 7,
        8 => 8,
        9 => 9,
        11 => 11,
        _ => 0,
    }
}

/// Skate 1's wipeout state (0x827a3c18: 4 while the physics-teleport byte is set or the category is Wipeout). The
/// byte is the engine's bail reset (mode teleports never set it, R1), which follows the Wipeout category straight into
/// the 702 teleport (category Teleport; 701 and unknown states keep the prior category): so a Teleport stretch counts
/// only when it continues a wipeout.
// ponytail: the byte's other writers (S1 [[+0]+28]+34) aren't traced; an engine teleport without a bail never reads
// as a wipeout here.
fn s1_wiping(category: u32, was: bool) -> bool {
    category == FilteredCategory::Wipeout as u32 || (was && category == FilteredCategory::Teleport as u32)
}

fn probe(
    mut online: ResMut<Skate1Online>,
    skater: Res<SkaterRuntime>,
    time: Res<Time>,
    marker: Option<Res<crate::session_marker::SessionMarker>>,
) {
    let p = &mut online.probe;
    if !p.live {
        return;
    }
    p.clock += time.delta_secs();
    p.note_marker(marker.map(|m| m.uses));
    let from = p.events.len();
    let s = &skater.scoring;
    if p.kinds.is_empty() {
        p.kinds = s.data.definitions.iter().map(|d| d.trick_type).collect();
        p.labels = s.data.definitions.iter().map(|d| d.label.clone()).collect();
        p.idents = s.data.definitions.iter().map(|d| d.identifier).collect();
        p.ids = p.idents.iter().enumerate().map(|(i, &d)| (d, i)).collect();
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let pos = [at[0], at[1], at[2]];
    p.pos = pos;
    let me = json!(0); // PlayerID is set by drive
    for (i, (name, v)) in p.volumes.iter().enumerate() {
        let inside = v.contains(pos);
        if inside != p.inside.contains(&i) {
            let event = if inside { "triggerentercollision" } else { "triggerexitcollision" };
            p.events.push((event.into(), json!({"PlayerID": me, "VolumeID": name}), 0.));
        }
    }
    p.inside = (0..p.volumes.len()).filter(|&i| p.volumes[i].1.contains(pos)).collect();
    // Tricks as the engine announces them (this tick's): trickcomplete with the Skate 1 type.
    for &id in &s.announced {
        if let Some(&i) = p.ids.get(id) {
            p.events.push(("trickcomplete".into(), json!({"PlayerID": me, "TrickType": skate1_trick_type(p.kinds[i])}), 0.));
        }
    }
    let wiping = s1_wiping(skater.player_input.physical.filtered_state_0, p.wiping);
    let off = skater.player_input.physical.state.category_12 == 500;
    let air = s.in_air();
    // The air as Skate 3's S.K.A.T.E. reads it: aircompleted once it has settled, its FirstScorable the engine's
    // (skate3_skate_game::first_scorable: a launch then a flip is the flip). A bailed air sends none.
    let dropped = (wiping && !p.wiping) || (off && !p.off);
    if air && !p.air {
        p.took_off_switch = skater.animation.packet.riding_switch;
    }
    if let Some((tricks, turns)) = p.air_step(&s.announced, air, s.spin_turns, time.delta_secs(), dropped) {
        let kind = |id: &str| p.ids.get(id).map(|&i| p.kinds[i]);
        let first = crate::skate3_skate_game::first_scorable(&tricks, kind).and_then(|(id, _)| p.ids.get(id).copied());
        let grab = tricks.iter().any(|t| kind(t) == Some(4));
        (p.first, p.grab, p.spins, p.switch) = (first, grab, i64::from(turns.unsigned_abs()), p.took_off_switch);
        let first = first.map_or(-1, |i| i as i64);
        p.events.push(("aircompleted".into(), json!({"PlayerID": me, "FirstScorableId": first, "Wipedout": 0}), 0.));
    }
    p.air = air;
    let open = s.sequence_open();
    if open && p.open.is_none() {
        p.open = Some(p.clock);
    }
    if s.sequence_landed || (wiping && !p.wiping && p.open.is_some()) {
        let ago = p.clock - p.open.unwrap_or(p.clock);
        let score = if s.sequence_landed { s.sequence_score.round() as i64 } else { 0 };
        let mut message = json!({"PlayerID": me, "SequenceScore": score, "Wipedout": !s.sequence_landed, "StartTick": 0, "StartSimGameTick": 0});
        // Only when true: an older build (whose `fits` drops an event with a key it lacks) loses just these, which
        // score nothing anyway.
        if std::mem::take(&mut p.marked) {
            message[MARKER_USED] = json!(true);
        }
        p.events.push(("sequencecomplete".into(), message, ago));
        p.open = None;
    } else if !open {
        p.open = None;
    }
    if wiping && !p.wiping {
        p.events.push(("wipeout".into(), json!({"PlayerID": me}), 0.));
    }
    p.wiping = wiping;
    // Skate 3's rule (user 10-28): stepping off the board is skateroffboard (S3 skate.lua 619, 1149); a bail's is its
    // wipeout.
    if off && !p.off && !wiping {
        p.events.push(("skateroffboard".into(), json!({"PlayerID": me}), 0.));
    }
    p.off = off;
    let types: serde_json::Map<String, Json> =
        s.holding().filter_map(|id| p.ids.get(id)).map(|&i| (skate1_trick_type(p.kinds[i]).to_string(), json!(true))).collect();
    p.state = p.trick_state(air, open, types);
    for (_, message, _) in &mut p.events[from..] {
        message["S"] = p.state.clone();
    }
}

// ---------------------------------------------------------------- Bevy side
#[derive(Resource, Default)]
pub(crate) struct Skate1Online {
    pub data: Option<Arc<Data>>,
    pub run: Option<Run>,
    /// Party Play: the other players' consoles ([`Party`]); `run` is the pad's.
    pub party: Option<Party>,
    probe: Probe,
    rings: Vec<(usize, Entity)>,
    /// The race gates (or, without their models, rings) on the race's way points ahead.
    gates: Vec<(usize, Entity)>,
    /// Each gate model's mesh and material, made on first show.
    looks: HashMap<&'static str, Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
    /// The flythrough this started.
    playing: Option<String>,
    /// The script stopped on an error: the match is over for this player.
    failed: bool,
    /// LT / RT held last frame (the spectator's press edges, [`Run::spectate`]).
    triggers: [bool; 2],
}

pub(crate) struct Skate1OnlinePlugin;
impl Plugin for Skate1OnlinePlugin {
    fn build(&self, app: &mut App) {
        let assets = app.world().resource::<Config>().asset_root.clone();
        let data = Data::load(&assets).inspect_err(|e| warn!("Skate 1 online: {e}")).ok().map(Arc::new);
        if let Some(data) = &data {
            let (ids, lost): (Vec<&Row>, Vec<&Row>) = data.rows.values().partition(|r| r.playable());
            if !lost.is_empty() {
                info!("Skate 1 online: {} rows without their spot volume left out: {:?}", lost.len(), lost.iter().map(|r| &r.id).collect::<Vec<_>>());
            }
            let ids: Vec<String> = ids.into_iter().map(|r| r.id.clone()).collect();
            if let Some(mut playable) = app.world_mut().get_resource_mut::<Playable>() {
                playable.0.extend(ids.iter().cloned());
            }
            if let Some(mut owned) = app.world_mut().get_resource_mut::<Owned>() {
                owned.0.extend(ids);
            }
        }
        app.insert_resource(Skate1Online { data, ..default() })
            .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics))
            .add_systems(
                FixedUpdate,
                crate::online_boundary::add_skate1_walls
                    .after(crate::multiplayer::prepare)
                    .before(crate::app::SimulationSet::Physics),
            )
            .add_systems(Update, drive.after(crate::app::FrameSet::Physics));
    }
}

/// Starts, runs and ends the row's script with the match.
pub(crate) fn drive(world: &mut World) {
    world.resource_scope(|world, mut online: Mut<Skate1Online>| {
        let Some(data) = online.data.clone() else { return };
        let (mode, phase, row, owned, slot, token) = {
            let m = world.resource::<Match>();
            (m.mode, m.phase.clone(), m.row.clone(), m.owned, m.slot, m.token)
        };
        let ours = owned && mode.is_some() && data.rows.contains_key(&row) && matches!(phase, Phase::Intro { .. } | Phase::Running { .. });
        if !ours {
            if online.run.is_some() || online.probe.live || online.failed || !online.rings.is_empty() || !online.gates.is_empty() {
                end(world, &mut online);
                online.failed = false;
            }
            return;
        }
        if online.failed {
            return;
        }
        if online.run.is_none() && !trip_over(world, "Skate_1_San_Vanelona") {
            world.resource_mut::<Match>().hold = true;
            return;
        }
        if online.run.is_none() {
            // Party Play: every player's console here, one pad (Skate 3's rules: the first turn drawn from the clock, as
            // skate2_skate_game's).
            let party = token == 0 && crate::party_play::in_session(world);
            let names = {
                let m = world.resource::<Match>();
                if party {
                    crate::party_play::names(world)
                } else if m.players.is_empty() {
                    vec![crate::skate2_contests::player_name(world)]
                } else {
                    m.players.clone()
                }
            };
            let me = if token == 0 { 0 } else { slot };
            let result = if party {
                let seed = u64::from(crate::match_core::match_now(world).to_bits());
                Party::new(&data, &data.rows[&row], names, seed).map(|(run, p)| {
                    online.party = Some(p);
                    run
                })
            } else {
                Run::new(&data, &data.rows[&row], names, me, token).and_then(|mut run| run.start(&data).map(|()| run))
            };
            match result {
                Ok(run) => {
                    info!("Skate 1 online: {} on {row} as player {me}", run.mode.name());
                    let volumes = (data.rows[&row].places.iter())
                        .filter_map(|(name, p)| match p {
                            Place::Volume(v) => Some((name.clone(), *v)),
                            _ => None,
                        })
                        .collect();
                    online.probe = Probe { live: true, volumes, ..default() };
                    online.run = Some(run);
                    let mut m = world.resource_mut::<Match>();
                    // A party session has no cap (its dialog ends it), as the Skate 2 ports'.
                    m.time_limit = online.party.is_none().then_some(CAP);
                    // The intro's spotfound card (intro_overlay.rs) names the spot by Mission.GetLocation online: the
                    // row's name is its LocationID's text (old_online1.py; "#Library", '#' = literal text).
                    let name = &data.rows[&row].name;
                    m.title = name.strip_prefix('#').unwrap_or(name).to_owned();
                }
                Err(error) => return fail(world, &mut online, &error),
            }
        }
        // Whole: a hitch's time is caught up (Session::step), not dropped, so this console's rounds end with the others'.
        // ponytail: CATCH_UP caps one frame's catch-up (a debugger pause); raise it if a real stall runs longer.
        // The match clock's step: 0 while a Party Play pause holds it.
        let dt = crate::match_core::match_dt(world).min(CATCH_UP);
        // The card up: the pad's events wait (a teleport's volume entry among them).
        let waiting = online.party.as_ref().is_some_and(|p| p.waiting);
        let probe = if waiting { Vec::new() } else { std::mem::take(&mut online.probe.events) };
        let state = online.probe.state.clone();
        let (inbox, players) = {
            let mut m = world.resource_mut::<Match>();
            (std::mem::take(&mut m.inbox), m.players.clone())
        };
        // Where everyone is (the races' estimate): this player's root, the others' skaters by their lobby slot.
        let roster = world.get_resource::<crate::online_roster::OnlineRoster>();
        let slots = roster.and_then(|r| r.state.game.as_ref()).map(|g| g.slots.clone()).unwrap_or_default();
        let mut remotes = world.query::<(&crate::multiplayer::appearance::RemoteCharacter, &GlobalTransform)>();
        let remotes: HashMap<u64, [f32; 3]> = remotes.iter(world).map(|(r, t)| (r.0, t.translation().to_array())).collect();
        let here = online.probe.pos;
        // The spectator's LT / RT: pressed past half, not while the skater's input is gated or the settings are open.
        let menu = world.get_resource::<crate::graphics_menu::Menu>().is_some_and(|m| m.open);
        let held = (world.get_resource::<crate::input::ControllerInput>())
            .filter(|input| !input.gated() && !menu)
            .map_or([false; 2], |input| input.raw_input().triggers.map(|t| t > 0.5));
        let pressed = [0, 1].map(|k| held[k] && !online.triggers[k]);
        online.triggers = held;
        // Where the pad's player stands (its waiting place if a turn passes now).
        let pad = (world.get_resource::<SkaterRuntime>())
            .map_or(skate_core::physics::skeleton_animation_record::IDENTITY, |s| s.animated_skeleton.roots.animation_to_world);
        let online = &mut *online;
        let Some(run) = online.run.as_mut() else { return };
        let (me, tick) = (run.me(), run.session.tick);
        run.positions = (0..run.names.len().max(1))
            .map(|i| if i == me { Some(here) } else { slots.get(i).and_then(|a| remotes.get(a)).copied() })
            .collect();
        let local: Vec<(String, Json)> = probe
            .into_iter()
            .map(|(event, mut message, ago)| {
                message["PlayerID"] = json!(me);
                if message.get("StartTick").is_some() {
                    let start = tick - (ago * 60.).round() as i64;
                    (message["StartTick"], message["StartSimGameTick"]) = (json!(start), json!(start));
                }
                (event, message)
            })
            .collect();
        // The local trick state as the Trick queries read it between events.
        let result = run.session.skater(me, &state).and_then(|()| match online.party.as_mut() {
            Some(party) => party.frame(run, &data, dt, local, pad),
            None => run.frame(&data, dt, local, inbox, &players).map(|()| false),
        });
        match result {
            Err(error) => return fail(world, online, &error),
            // The skater is the next player's now: what the probe held of the last one's goes.
            Ok(true) => online.probe.handed_over(),
            Ok(false) => {}
        }
        if let Some(run) = online.run.as_mut() {
            if pressed[0] {
                run.spectate(false);
            }
            if pressed[1] {
                run.spectate(true);
            }
        }
        apply(world, online, &data);
        if online.party.is_some() {
            party_exchange(world, online, &data);
        }
    });
}

/// Party Play's frame with party_play (its `exchange`): the card, the pad's player, the others' waiting places, the
/// results dialog once the pad's script is complete (after Skate 1's own leaderboard); back come the card's A, Rematch
/// and Restart Challenge (the game again from the start: the run goes for a frame, so the HUD starts afresh).
fn party_exchange(world: &mut World, online: &mut Skate1Online, data: &Data) {
    let (Some(run), Some(party)) = (online.run.as_ref(), online.party.as_mut()) else { return };
    let me = run.me();
    let waiting = (0..party.at.len()).map(|p| party.at[p].filter(|_| p != me)).collect();
    let results = run.complete().then(|| party_results(run, data));
    let row = run.row.clone();
    let turn = crate::party_play::Turn { controlled: me, waiting, card: party.card.take(), results };
    let answers = crate::party_play::exchange(world, turn);
    if answers.hud_complete
        && let (Some(run), Some(party)) = (online.run.as_mut(), online.party.as_mut())
        && let Err(error) = party.answer(run)
    {
        warn!("Skate 1 Party Play: {error}");
    }
    if answers.retry || answers.restart {
        info!("Skate 1 Party Play: {row} again");
        end(world, online);
    }
}

/// Bug 10-31: a Party Play start travels to the Skate 1 map behind the loading screen. The script, started with the
/// match, played its flythrough behind that screen, and its RepositionAI landed before the trip's own spot, which then
/// won. The run starts once the trip is over ([`trip_ready`]). Online (the session already on the map) that is the
/// start slot's cover lifting. `map` is the row's map (its file stem); Skate 2's modes wait for theirs too (bug 11-01).
pub(crate) fn trip_over(world: &World, map: &str) -> bool {
    let current = world.get_resource::<crate::map_transition::CurrentMap>();
    trip_ready(
        current.is_none_or(|m| m.path.as_deref().and_then(std::path::Path::file_stem).is_some_and(|s| s == map)),
        world.get_resource::<crate::map_transition::MapTransition>().is_some_and(|t| t.busy()),
        world.get_resource::<crate::teleport_menu::Travel>().is_some_and(|t| t.pending.is_some() || t.pending_matrix.is_some()),
        world.resource::<Match>().teleporting(),
        world.get_resource::<crate::tip_loader::Covered>().is_some_and(|c| c.0),
    )
}
/// On the row's map, nothing loading, no travel or match teleport still to come, the loading screen down.
fn trip_ready(on_map: bool, loading: bool, travelling: bool, teleporting: bool, covered: bool) -> bool {
    on_map && !loading && !travelling && !teleporting && !covered
}

/// The run's asks of the world: teleport, hold, flythrough, indicators, the outbox and the result.
fn apply(world: &mut World, online: &mut Skate1Online, data: &Data) {
    let Some(run) = online.run.as_mut() else { return };
    // Party Play: the pad's player is the skater itself (no camera on another), and its dialog ends the game.
    let party = online.party.as_ref().map(|p| p.waiting);
    let scene = run.scene.as_ref().map(|(s, _)| s.clone());
    let hold = run.stop || run.pin || scene.is_some() || party == Some(true);
    let pad_held = world.get_resource::<crate::input::ControllerInput>().is_none_or(|i| i.match_hold);
    if let Some(at) = run.teleport.filter(|_| teleport_now(hold, pad_held)) {
        if crate::skate2_contests::teleport(world, at) {
            run.teleport = None;
        }
    }
    let wanted = run.indicators.clone();
    let (complete, result) = (run.complete(), run.result());
    let outbox = std::mem::take(&mut run.outbox);
    let (camera, pause_disabled) = (run.camera, run.pause_disabled);
    if std::mem::take(&mut run.markers) > 0
        && let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>()
    {
        marker.request_set = true;
    }
    {
        let mut m = world.resource_mut::<Match>();
        m.hold = hold;
        m.pause_disabled = pause_disabled;
        m.watch = camera.filter(|_| party.is_none());
        m.score = result;
        m.finished |= complete && party.is_none();
        if m.token != 0 {
            m.outbox.extend(outbox);
        }
    }
    // The flythrough: started once, stopped when the script's NIS is over.
    if scene != online.playing {
        let mut cutscene = world.resource_mut::<Cutscene>();
        match &scene {
            Some(scene) => cutscene.start(&data.root.join("intros"), scene),
            None => cutscene.stop(),
        }
        online.playing = scene;
    }
    if let Some(row) = online.run.as_ref().and_then(|r| data.rows.get(&r.row)) {
        rings(world, &mut online.rings, row, &wanted);
    }
    gate(world, online, data);
}

/// Bug 10-31 (Party Play): sitting at the Press-A card, the skater turned and rolled. physics::pin holds a board only
/// when it lands from a teleport under the pad's hold, and the pad's hold comes from match_core::step, which may run
/// before this system: a teleport sent with the hold landed a tick ahead of it, unheld. A held teleport now waits for
/// the pad's hold (a frame at most).
fn teleport_now(hold: bool, pad_held: bool) -> bool {
    !hold || pad_held
}

/// The racer after (or before) `at` among `racing` (by slot), wrapping; None when no other is racing.
fn next_watched(at: usize, racing: &[bool], forward: bool) -> Option<usize> {
    let n = racing.len();
    (1..n).map(|k| if forward { (at + k) % n } else { (at + n - k) % n }).find(|&i| racing[i])
}

/// The way points a race shows after this player hit `hit` (0x823dc7d0, on the local player's NotifyPlayerHitWayPoint):
/// the five after it round the route, by place shown (Death Race: the next, then three more; Spot Race: the next and one
/// more) unless it is the gate hit, no lap is left, or on the last lap it is behind (a looping route's start line still
/// shows, as the finish).
fn upcoming(row: &Row, mode: Mode, hit: usize, laps: i64) -> Vec<usize> {
    let looping = row.fields["IsLooping"].as_bool().unwrap_or(false);
    let len = way_points(row);
    let left = if looping { row.fields["NumLaps"].as_f64().unwrap_or(1.) as i64 } else { 1 } - laps;
    let shown = if mode == Mode::SpotRace { 2 } else { 4 };
    let behind = |e: usize| left == 1 && if looping { (e < hit || hit == 0) && e > 0 } else { e < hit };
    (1..=shown).map(|k| (hit + k) % len).filter(|&e| e != hit && left >= 1 && !behind(e)).collect()
}

/// A race's way point `index` (0 the start line, 1.. the CheckPoints, then the finish line) by its volume name.
fn way_point(row: &Row, index: usize) -> Option<&str> {
    let points = row.fields["CheckPoints"].as_array()?;
    match index {
        0 => row.fields["StartLine"].as_str(),
        i if i <= points.len() => points[i - 1].as_str(),
        i if i == points.len() + 1 => row.fields["FinishLine"].as_str(),
        _ => None,
    }
}

/// Retail's race gate visual treatments on the way points shown ahead ([`Run::gates`]): the visual treatment manager
/// (0x826b5c08) pools four RaceGates (rg000, Death Race) and its SpotGates (sg000, Spot Race), each here filling its
/// trigger volume ([`gate_transform`]). No finish model: rgf00 is in the file but no code names it. Without the models
/// (an install from before them) the indicator ring stands in.
// ponytail: the treatments' tint (color3d), scroll (mTimer) and second texcoord set aren't traced: every gate shown
// draws alike, its page on texcoord 0, unlit and blended.
fn gate(world: &mut World, online: &mut Skate1Online, data: &Data) {
    let wanted = online.run.as_ref().map(|r| r.gates.clone()).unwrap_or_default();
    if online.gates.iter().map(|(i, _)| *i).eq(wanted.iter().copied()) {
        return;
    }
    for (_, e) in online.gates.drain(..) {
        world.despawn(e);
    }
    let Some((row, mode)) = online.run.as_ref().and_then(|r| Some((data.rows.get(&r.row)?, r.mode))) else { return };
    let name = if mode == Mode::SpotRace { "sg000" } else { "rg000" };
    let model = data.gates.get(name);
    let look = model.and_then(|m| online.looks.entry(name).or_insert_with(|| gate_look(world, &data.root, m)).clone());
    for index in wanted {
        let Some(Place::Volume(v)) = way_point(row, index).and_then(|n| row.places.get(n)) else { continue };
        let entity = match (model, &look) {
            (Some(model), Some((mesh, material))) => {
                let from = way_point_middle(row, index.checked_sub(1).unwrap_or_else(|| way_points(row) - 1));
                let at = gate_transform(v, from, model, mode == Mode::SpotRace);
                world.spawn((Name::new("Skate 1 race gate"), Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), at)).id()
            }
            _ => {
                let (min, max) = (Vec3::from(v.min), Vec3::from(v.max));
                let ring = spawn_ring(world, Vec3::new((min.x + max.x) / 2., min.y, (min.z + max.z) / 2.));
                let span = ((max.x - min.x).max(max.z - min.z) / 2.).max(1.);
                if let Some(mut t) = world.get_mut::<Transform>(ring) {
                    t.scale = Vec3::new(span, 1., span);
                }
                ring
            }
        };
        online.gates.push((index, entity));
    }
}

/// A race's way points: the start line, the CheckPoints, and the finish line unless the route loops.
fn way_points(row: &Row) -> usize {
    let looping = row.fields["IsLooping"].as_bool().unwrap_or(false);
    row.fields["CheckPoints"].as_array().map_or(0, Vec::len) + if looping { 1 } else { 2 }
}

/// A way point's volume middle (None: no volume).
fn way_point_middle(row: &Row, index: usize) -> Option<Vec3> {
    match way_point(row, index).and_then(|n| row.places.get(n)) {
        Some(Place::Volume(v)) => Some((Vec3::from(v.min) + Vec3::from(v.max)) / 2.),
        _ => None,
    }
}

/// A gate model over its trigger volume, on the volume's floor: a RaceGate across the way the course comes in from
/// `from` (the way point before; Skate 2's arch rule), as wide and tall as the volume's side that faces it; a SpotGate
/// round the volume, as big as it.
// ponytail: retail's volumes are axis-aligned boxes and the exe's fit of a treatment to one isn't traced; this fills it.
pub(crate) fn gate_transform(v: &Aabb, from: Option<Vec3>, model: &GateModel, spot: bool) -> Transform {
    let (min, max) = (Vec3::from(v.min), Vec3::from(v.max));
    let (size, own) = (max - min, (Vec3::from(model.max) - Vec3::from(model.min)).max(Vec3::splat(1e-3)));
    let at = ((min + max) / 2.).with_y(min.y);
    if spot {
        return Transform::from_translation(at).with_scale(size / own);
    }
    let (width, yaw) = crate::skate2_death_race::across(min, max, from.unwrap_or(at - Vec3::Z));
    Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)).with_scale(Vec3::new(width / own.x, size.y / own.y, 1.))
}

/// A gate model's mesh and material: its page (gates/), unlit and blended, both sides. None without the page.
pub(crate) fn gate_look(world: &mut World, root: &Path, model: &GateModel) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
    let path = root.join("gates").join(&model.texture);
    let page = image::open(&path).inspect_err(|e| warn!("Skate 1 online: {}: {e}", path.display())).ok()?.into_rgba8();
    let size = page.dimensions();
    let page = world.resource_mut::<Assets<Image>>().add(crate::custom_graphic::texture(page.into_raw(), size));
    let mesh = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, model.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 0., 1.]; model.positions.len()])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, model.uvs.clone())
        .with_inserted_indices(bevy::mesh::Indices::U32(model.indices.clone()));
    let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let material = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color_texture: Some(page),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    Some((mesh, material))
}

/// The race gates shown ahead, for the minimap (world x, z of each way point's volume middle).
pub(crate) fn gate_marks(online: &Skate1Online) -> Vec<[f32; 2]> {
    let Some((run, row)) = online.run.as_ref().and_then(|r| Some((r, online.data.as_ref()?.rows.get(&r.row)?))) else { return Vec::new() };
    run.gates.iter().filter_map(|&i| way_point_middle(row, i)).map(|m| [m.x, m.z]).collect()
}

/// The VisualIndicators shown (1-based) that are placed, at their locators' translations (0x823df6f0 -> 0x826b7f10):
/// type 4 is the obstacle indicator; a type-0 record is never placed (0x823df8b8). P7 (audit 10-28): every online
/// record is type 4 but ol_fp_u1_ownspot01's second, type 0. An install from before `indicator_types` places them all.
fn indicator_places(row: &Row, wanted: &BTreeSet<usize>) -> Vec<(usize, Vec3)> {
    let names = row.fields["VisualIndicators"].as_array();
    (wanted.iter().copied())
        .filter(|&i| row.indicator_types.get(i.wrapping_sub(1)).is_none_or(|&t| t == 4))
        .filter_map(|i| match row.places.get(names?.get(i.wrapping_sub(1))?.as_str()?)? {
            Place::Locator(l) => Some((i, Vec3::new(l.matrix[3][0], l.matrix[3][1], l.matrix[3][2]))),
            Place::Volume(_) => None,
        })
        .collect()
}

/// The VisualIndicators: Skate 1's oi000 arrows, as the tutorial draws them (skate1_screens::face_arrows turns, scales
/// and fades each). Audit 10-28 GAP-4: this drew a gold torus.
fn rings(world: &mut World, shown: &mut Vec<(usize, Entity)>, row: &Row, wanted: &BTreeSet<usize>) {
    let places = indicator_places(row, wanted);
    if shown.iter().map(|(i, _)| *i).eq(places.iter().map(|(i, _)| *i)) {
        return;
    }
    for (_, e) in shown.drain(..) {
        world.despawn(e);
    }
    let server = world.resource::<AssetServer>().clone();
    for (i, at) in places {
        let arrow = crate::skate1_screens::indicator(&server, &mut world.resource_mut::<Assets<StandardMaterial>>(), Transform::from_translation(at));
        shown.push((i, world.spawn(arrow).id()));
    }
}

/// A ring on a race gate's volume, standing in for the gate models in an install from before them.
fn spawn_ring(world: &mut World, at: Vec3) -> Entity {
    let mesh = world.resource_mut::<Assets<Mesh>>().add(Torus::new(0.85, 1.0));
    let material = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.82, 0.2, 0.75),
        emissive: LinearRgba::rgb(2.0, 1.4, 0.3),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    world.spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::from_translation(at + Vec3::Y * 0.05))).id()
}

/// The script stopped on an error: this player is done and quits the match (Match::quit), so the others' scripts
/// get its playerdisconnected and play on without it.
fn fail(world: &mut World, online: &mut Skate1Online, error: &str) {
    error!("Skate 1 online: {error}");
    end(world, online);
    online.failed = true;
    let mut m = world.resource_mut::<Match>();
    (m.finished, m.quit) = (true, true);
}

/// The match is over (or not ours): everything the run showed or held goes.
fn end(world: &mut World, online: &mut Skate1Online) {
    online.party = None;
    if online.run.take().is_some() || online.playing.is_some() {
        world.resource_mut::<Cutscene>().stop();
        let mut m = world.resource_mut::<Match>();
        (m.hold, m.watch, m.gestures) = (false, None, false);
    }
    online.playing = None;
    online.probe = Probe::default();
    for (_, e) in online.rings.drain(..).chain(online.gates.drain(..)) {
        world.despawn(e);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_peer_s_extreme_ticks_cannot_overflow() {
        let text = json!({"e": "sequencecomplete", "m": {"StartTick": i64::MAX}, "k": i64::MIN}).to_string();
        let (_, m) = decode(&text, 0, 5).unwrap();
        assert_eq!(m["StartTick"], json!(i64::MAX));
    }

    /// What the natives would choke on (`S.skaters[1] = 5`: skater(1).air indexes a number; rec(nil) with no
    /// PlayerID; a script's arithmetic on a text score) is dropped; the sender is always its slot.
    #[test]
    fn a_peer_s_malformed_event_is_dropped() {
        let bad = [
            json!({"e": "aircompleted", "m": {"PlayerID": 1, "S": 5}}),
            json!({"e": "aircompleted", "m": {"S": {"types": 5}}}),
            json!({"e": "sequencecomplete", "m": {"SequenceScore": "lots"}}),
            json!({"e": "timerexpired", "m": {"TimerID": 1}}),
            json!({"e": "wipeout", "m": 5}),
        ];
        for v in bad {
            assert_eq!(decode(&v.to_string(), 1, 0), None, "{v}");
        }
        let (_, m) = decode(&encode("wipeout", &json!({}), 0), 3, 0).unwrap();
        assert_eq!(m, json!({"PlayerID": 3}));
    }

    pub(crate) fn data() -> Data {
        let assets = std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT");
        Data::load(Path::new(&assets)).unwrap()
    }

    /// Runs `run` at 60 Hz until its script completes or `seconds` pass; `play(run, frame)` gives each frame's
    /// events. Returns the frame it completed on.
    pub(crate) fn play(
        data: &Data,
        run: &mut Run,
        seconds: f32,
        mut events: impl FnMut(&Run, usize) -> (Vec<(String, Json)>, Vec<(usize, String)>),
    ) -> Option<usize> {
        let players = run.names.clone();
        for f in 0..(seconds * 60.) as usize {
            let (local, inbox) = events(run, f);
            run.frame(data, 1. / 60., local, inbox, &players).unwrap();
            if run.complete() {
                return Some(f);
            }
        }
        None
    }

    #[test]
    fn relayed_events_carry_the_senders_id_and_move_its_ticks_onto_ours() {
        let text = encode("sequencecomplete", &json!({"PlayerID": 0, "SequenceScore": 900, "StartTick": 500, "StartSimGameTick": 500}), 520);
        let (event, message) = decode(&text, 2, 1020).unwrap();
        assert_eq!(event, "sequencecomplete");
        assert_eq!(message, json!({"PlayerID": 2, "SequenceScore": 900, "StartTick": 1000, "StartSimGameTick": 1000}));
        assert_eq!(decode("{}", 0, 0), None);
    }

    /// The wipeout is Skate 1's state 4: a bail (category 4, not a free board) through its reset teleport (5), one
    /// rising edge; a mode's teleport (5 from riding) is no wipeout.
    #[test]
    fn a_bail_through_its_reset_is_one_wipeout_and_a_mode_teleport_none() {
        let mut was = false;
        let level: Vec<bool> = [1, 6, 4, 4, 5, 5, 1, 5, 5, 1]
            .into_iter()
            .map(|c| {
                was = s1_wiping(c, was);
                was
            })
            .collect();
        assert_eq!(level, [false, false, true, true, true, true, false, false, false, false]);
    }

    #[test]
    fn list_fields_are_the_ones_read_as_lists() {
        let found = list_fields("x = self.Spots.Count y = self.Times[2] z = self.Name w = self.Other.Count");
        assert_eq!(found.into_iter().collect::<Vec<_>>(), ["Other", "Spots", "Times"]);
    }

    /// Best Trick on the Library, two players: three rounds of flythrough, countdown and 120 s, both landing a
    /// sequence in the round's spot volume each round; the final results, then complete (as the harness prototype
    /// run_bt.py did on lupa).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn best_trick_runs_three_rounds_to_complete() {
        let data = data();
        let row = &data.rows["besttrick_financial_library"];
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 7).unwrap();
        run.start(&data).unwrap();
        let spots: Vec<String> = row.fields["BestTrickSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut round_began: BTreeMap<i64, usize> = BTreeMap::new();
        let mut results_from = None;
        let mut hud = Vec::new();
        let done = play(&data, &mut run, 60. * 12., |run, f| {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            if run.session.state() == "event_round" {
                let round = run.session.var("CurrentRound").as_i64().unwrap();
                let k = *round_began.entry(round).or_insert(f);
                let spot = &spots[round as usize - 1];
                if f == k + 60 {
                    local.push(("triggerentercollision".into(), json!({"PlayerID": 0, "VolumeID": spot})));
                    inbox.push((1, encode("triggerentercollision", &json!({"PlayerID": 1, "VolumeID": spot}), 0)));
                }
                if f == k + 120 {
                    let start = run.session.tick - 20;
                    let seq = |score| json!({"PlayerID": 0, "SequenceScore": score, "StartTick": start, "StartSimGameTick": start, "Wipedout": false});
                    local.push(("sequencecomplete".into(), seq(1000 * round)));
                    inbox.push((1, encode("sequencecomplete", &seq(1500), run.session.tick)));
                }
            }
            if run.session.state() == "results" && run.hud.iter().any(|c| c.name == "HUD.SetWinState") && results_from.is_none() {
                results_from = Some(f);
            }
            if results_from.is_some_and(|r| f == r + 600) {
                local.push(("postchallengedisplayexit".into(), json!({})));
            }
            (local, inbox)
        });
        for call in &run.hud {
            hud.push(call.name.clone());
        }
        assert!(done.is_some(), "stuck in {}", run.session.state());
        assert_eq!(round_began.len(), 3);
        // Rounds: Alice 1000 / 2000 / 3000 against Bob's 1500: Bob, Alice, Alice. 1 point a round won.
        let points: Vec<i64> = run.standings.iter().map(|s| s.points).collect();
        assert_eq!(points, [2, 1], "{:?}", run.standings);
        assert_eq!(run.result(), 2);
        assert!(run.left);
        assert!(hud.iter().any(|h| h == "HUD.AddChallengeWinnerMessage"));
        // Audit 10-28 GAP-8: event_round's Challenge.SetNewSessionMarker(own id), once a round (the Bevy apply hands
        // each to SessionMarker.request_set).
        assert_eq!(run.markers, 3, "a session marker each round");
    }

    /// S.K.A.T.E.'s pattern natives: a set trick is copied by the same trick with the same 180s; the same trick and
    /// 180s again while among the last three set is "already been done".
    #[test]
    fn skate_patterns_follow_the_last_three_set() {
        let lua = mlua::Lua::new();
        lua.load(NATIVES).exec().unwrap();
        let got: Vec<bool> = lua
            .load(
                r#"
                local r = {}
                local function leader(trick, spins) S.skaters[0] = {first = trick, first_type = 2, spins = spins} end
                local function follower(trick, spins) S.skaters[1] = {first = trick, first_type = 2, spins = spins} end
                leader(7, 0)
                r[#r + 1] = Trick.SetPatternLastTrick(7, 0)        -- set
                follower(7, 2) r[#r + 1] = Skate.MatchedPattern(1, 0) -- other 180s: wrong
                follower(7, 0) r[#r + 1] = Skate.MatchedPattern(1, 0) -- copied
                r[#r + 1] = Trick.SetPatternLastTrick(7, 0)        -- already been done
                leader(7, 2) r[#r + 1] = Trick.SetPatternLastTrick(7, 0) -- a 360 of it is new
                leader(8, 0) r[#r + 1] = Trick.SetPatternLastTrick(8, 0)
                leader(9, 0) r[#r + 1] = Trick.SetPatternLastTrick(9, 0)
                leader(7, 0) r[#r + 1] = Trick.SetPatternLastTrick(7, 0) -- out of the last three: new again
                r[#r + 1] = Trick.GetTrickTypeFromScorableID(7) == Trick.eTrickType.Flip
                r[#r + 1] = Skate.eSkateChallengeType.Trick == 0
                return r
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(got, [true, false, true, false, true, true, true, true, false, true]);
    }

    /// `code` on a fresh copy of the natives: its list.
    fn natives(code: &str) -> Vec<i64> {
        let lua = mlua::Lua::new();
        lua.load(NATIVES).exec().unwrap();
        lua.load(code).eval().unwrap()
    }

    /// The challenge manager's order on retail's comparators (0x823da688 races, 0x823da600 the rest): finishers on
    /// the clock at the finish plus the penalty (whatever time the script passes with its way point), racers live on
    /// their estimate, then the out (DNF / DQF / left) who lasted longest; ChallengeStop puts whoever still races out
    /// with no clock, and a leaver is out only while the challenge runs. Score rows: the higher score, leavers last.
    #[test]
    fn manager_ranks_on_the_retail_comparators() {
        let finished = natives(
            r#"
            local CM = ChallengeManager
            S.n, S.race, S.per_gate = 3, true, 6
            S.time = 2 CM.ChallengeStart()
            S.time = 7 CM.NotifyPlayerDQF(2)
            S.time = 12 CM.NotifyPlayerHitWayPoint(0, 7, 999) CM.NotifyNumMissedWayPoint(0, 1) CM.NotifyPlayerFinished(0)
            S.time = 16 CM.NotifyPlayerHitWayPoint(1, 7, 1) CM.NotifyPlayerFinished(1)
            CM.ChallengeStop() CM.FinalizeChallengeResult()
            local o, _, _, time = __standings()
            return {CM.GetPlayerWon(), o[1], o[2], o[3], time[1], time[2]}"#,
        );
        assert_eq!(finished, [1, 1, 0, 2, 16, 14], "14 s beats 10 s + 6 s; the DQF last");
        let out = natives(
            r#"
            local CM = ChallengeManager
            S.n, S.race = 3, true
            S.time = 0 CM.ChallengeStart()
            S.time = 5 CM.NotifyPlayerDQF(0)
            S.time = 8 CM.NotifyPlayerDNF(1)
            S.time = 9 CM.ChallengeStop() CM.FinalizeChallengeResult()
            return __standings()"#,
        );
        assert_eq!(out, [1, 0, 2], "all out: the DNF at 8 s, the DQF at 5 s, then the one stopped");
        let left = natives(
            r#"
            local CM = ChallengeManager
            S.n, S.race = 3, true
            S.time = 0 CM.ChallengeStart()
            S.time = 3 __left(1)
            S.time = 10 CM.NotifyPlayerFinished(0)
            S.time = 12 CM.NotifyPlayerFinished(2)
            local all = CM.GetAllHumanFinished()
            CM.ChallengeStop() __left(0) CM.FinalizeChallengeResult()
            local o = __standings()
            return {all and 1 or 0, o[1], o[2], o[3]}"#,
        );
        assert_eq!(left, [1, 0, 2, 1], "a leaver mid-race is out; leaving after the stop keeps the win");
        let live = natives(
            r#"
            local CM = ChallengeManager
            S.n, S.race, S.total, S.lap = 2, true, 20, 20
            S.route = {[0] = {cum = 0, mid = {0, 0, 0}}, [1] = {cum = 10, mid = {10, 0, 0}}, [2] = {cum = 20, mid = {20, 0, 0}}}
            S.time = 0 CM.ChallengeStart()
            S.time = 1 CM.NotifyPlayerNextWayPoint(0, 1) CM.NotifyPlayerNextWayPoint(1, 2)
            __frame({[0] = {5, 0, 0}, [1] = {15, 0, 0}})
            local o, _, _, _, split = __standings()
            return {CM.GetPlayerWon(), o[2], math.floor(split[1] * 900 + 0.5), math.floor(split[2] * 900 + 0.5)}"#,
        );
        assert_eq!(live, [1, 0, 1000, 0], "15 m to go at 9 m/s trails 5 m to go by 10/9 s");
        let scores = natives(
            r#"
            local CM = ChallengeManager
            S.n = 3
            CM.ChallengeStart()
            CM.SetPlayerScore(0, 900) CM.SetPlayerScore(1, 400) CM.SetPlayerScore(2, 400)
            __left(0)
            __frame({})
            return __standings()"#,
        );
        assert_eq!(scores, [1, 2, 0], "the leaver last; equal scores keep their order");
    }

    /// A Death Race row: start line, three checkpoints, a finish line (a looping one's is its start line, 3 laps).
    fn race_row(looping: bool) -> Row {
        let v = |x: f32| json!({"volume": {"min": [x, 0., 0.], "max": [x + 2., 2., 2.]}});
        let (finish, laps) = if looping { ("s", 3) } else { ("f", 1) };
        serde_json::from_value(json!({
            "id": "r", "type": 19, "mode": "deathrace", "template": "t",
            "places": {"s": v(0.), "a": v(10.), "b": v(30.), "c": v(60.), "f": v(70.)},
            "self": {"StartLine": "s", "CheckPoints": ["a", "b", "c"], "FinishLine": finish, "IsLooping": looping, "NumLaps": laps},
        }))
        .unwrap()
    }

    /// SetupChallengeStatus's route: the distance from gate to gate (min corners), the finish line closing the lap; a
    /// looping route's start line is a lap on, the race NumLaps laps.
    #[test]
    fn race_route_measures_gate_to_gate() {
        let (points, lap, total) = route(&race_row(false)).unwrap();
        assert_eq!(points.iter().map(|p| p.0).collect::<Vec<_>>(), [0., 10., 30., 60., 70.]);
        assert_eq!((points[1].1, lap, total), ([11., 1., 1.], 70., 70.));
        let (points, lap, total) = route(&race_row(true)).unwrap();
        assert_eq!(points.iter().map(|p| p.0).collect::<Vec<_>>(), [120., 10., 30., 60.]);
        assert_eq!((lap, total), (120., 360.));
    }

    /// The gates shown after a hit: Death Race the next four, Spot Race two, none behind on the last lap; a looping
    /// route's last lap shows its start line as the finish.
    #[test]
    fn upcoming_gates_follow_the_hit_one() {
        let (line, ring) = (race_row(false), race_row(true));
        assert_eq!(upcoming(&line, Mode::DeathRace, 0, 0), [1, 2, 3, 4]);
        assert_eq!(upcoming(&line, Mode::SpotRace, 0, 0), [1, 2]);
        assert_eq!(upcoming(&line, Mode::DeathRace, 2, 0), [3, 4]);
        assert_eq!(upcoming(&line, Mode::DeathRace, 4, 0), [0; 0], "the finish line hit");
        assert_eq!(upcoming(&ring, Mode::DeathRace, 0, 0), [1, 2, 3]);
        assert_eq!(upcoming(&ring, Mode::DeathRace, 2, 2), [3, 0], "last lap: ahead, then the finish");
        assert_eq!(upcoming(&ring, Mode::DeathRace, 0, 2), [0; 0], "the last lap's finish");
        assert_eq!(upcoming(&ring, Mode::DeathRace, 1, 3), [0; 0], "no lap left");
    }

    /// A gate model with only its bounds (the tests place it, they don't draw it).
    fn model(min: [f32; 3], max: [f32; 3]) -> GateModel {
        GateModel { positions: vec![min, max], uvs: vec![[0., 0.]; 2], indices: Vec::new(), min, max, texture: String::new() }
    }
    /// rg000 (an upright quad, 14 x 14.9 m) and sg000 (four walls round a 14.7 m square) as visual_treatments.rx2 has
    /// them (old_online1_gates.py), for an install from before the gate models.
    fn stand_in(name: &str) -> GateModel {
        match name {
            "sg000" => model([-7.336, 0.03, -7.336], [7.336, 5.31, 7.336]),
            _ => model([-6.999, 0., 0.], [7., 14.943, 0.]),
        }
    }

    /// A RaceGate stands on its volume's floor across the way the course comes in, as wide and tall as the side that
    /// faces it; a SpotGate fills its volume.
    #[test]
    fn race_gates_fill_their_volumes() {
        let (gate, spot) = (stand_in("rg000"), stand_in("sg000"));
        let slab = Aabb { min: [10., 1., -5.], max: [12., 5., 5.] };
        let corners = |t: Transform, m: &GateModel| (t.transform_point(Vec3::from(m.min)), t.transform_point(Vec3::from(m.max)));
        // Coming along +x: turned across x, spanning the slab's 10 m side, 4 m tall, on its floor.
        let (a, b) = corners(gate_transform(&slab, Some(Vec3::ZERO), &gate, false), &gate);
        assert!((a.x - 11.).abs() < 1e-3 && (b.x - 11.).abs() < 1e-3, "{a} {b}");
        assert!(((a.z - b.z).abs() - 10.).abs() < 1e-3 && (a.z + b.z).abs() < 1e-2, "{a} {b}");
        assert!((a.y - 1.).abs() < 1e-4 && (b.y - 5.).abs() < 1e-3, "{a} {b}");
        // Coming along +z (and with no way point before it): across z, as wide as the slab's 2 m.
        for from in [Some(Vec3::new(11., 0., -50.)), None] {
            let (a, b) = corners(gate_transform(&slab, from, &gate, false), &gate);
            assert!(((a.x - b.x).abs() - 2.).abs() < 1e-3 && a.z.abs() < 1e-3 && b.z.abs() < 1e-3, "{from:?}: {a} {b}");
        }
        // A SpotGate: its walls on the volume's sides.
        let (a, b) = corners(gate_transform(&slab, None, &spot, true), &spot);
        assert!(a.abs_diff_eq(Vec3::new(10., 1., -5.), 0.05) && b.abs_diff_eq(Vec3::new(12., 5., 5.), 0.05), "{a} {b}");
    }

    /// Audit 10-28 GAP-4: SetAll / ByIndex show Skate 1's oi000 obstacle arrow (skate1_screens::Arrow, which face_arrows
    /// turns and fades) on each type-4 record's locator, translation only, and nothing for a type-0 record (P7:
    /// ol_fp_u1_ownspot01's second); RemoveAll takes them away. It drew a gold torus.
    #[test]
    fn visual_indicators_are_oi000_arrows_on_their_locators() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Mesh>().init_asset::<Image>().init_asset::<StandardMaterial>();
        let at = |x: f32| json!({"locator": {"matrix": [[0., 0., 1., 0.], [0., 1., 0., 0.], [-1., 0., 0., 0.], [x, 2., 3., 1.]]}});
        let row: Row = serde_json::from_value(json!({
            "id": "r", "type": 9, "mode": "spot_battle", "template": "t",
            "places": {"a": at(10.), "b": at(20.), "c": at(30.)},
            "indicator_types": [4, 0, 4],
            "self": {"VisualIndicators": ["a", "b", "c"]},
        }))
        .unwrap();
        let world = app.world_mut();
        let mut shown = Vec::new();
        rings(world, &mut shown, &row, &BTreeSet::from([1, 2, 3]));
        let mut arrows = world.query_filtered::<&Transform, With<crate::skate1_screens::Arrow>>();
        let mut placed: Vec<[f32; 3]> = arrows.iter(world).map(|t| t.translation.to_array()).collect();
        placed.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert_eq!(placed, [[10., 2., 3.], [30., 2., 3.]]);
        assert_eq!(world.query::<&Mesh3d>().iter(world).count(), 2, "no other model");
        rings(world, &mut shown, &row, &BTreeSet::new());
        assert_eq!(arrows.iter(world).count(), 0);
    }

    /// Audit 10-28 GAP-9: RT / LT step the spectator to the next / previous racer still racing, by slot, wrapping;
    /// alone on the course nobody else is picked.
    #[test]
    fn spectator_steps_through_the_racers_still_racing() {
        // Slot 0 is this player (finished), 2 finished too.
        let racing = [false, true, false, true];
        assert_eq!(next_watched(1, &racing, true), Some(3));
        assert_eq!(next_watched(3, &racing, true), Some(1), "wraps");
        assert_eq!(next_watched(1, &racing, false), Some(3), "wraps back");
        assert_eq!(next_watched(3, &racing, false), Some(1));
        assert_eq!(next_watched(1, &[false, true, false], true), None, "the one watched is the only racer");
        assert_eq!(next_watched(0, &[false], true), None);
    }

    /// GAP-9 on a real Death Race run: only once the script's camera is on another player, only racers (not this
    /// player, not a finished one), and never outside a race.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn spectate_cycles_only_a_race_spectators_camera() {
        let data = data();
        let names: Vec<String> = ["Ann", "Bo", "Cy", "Di"].map(String::from).into();
        let mut run = Run::new(&data, &data.rows["ol_deathrace_oldtown_spillway"], names.clone(), 0, 7).unwrap();
        run.spectate(true);
        assert_eq!(run.camera, None, "still racing: no spectator");
        run.standings = vec![Standing::default(); 4];
        run.standings[0].finished = true;
        run.standings[2].finished = true;
        run.camera = Some(1);
        run.spectate(true);
        assert_eq!(run.camera, Some(3));
        run.spectate(true);
        assert_eq!(run.camera, Some(1));
        run.spectate(false);
        assert_eq!(run.camera, Some(3));
        let mut skate = Run::new(&data, &data.rows["skate_financial_matrix"], names[..2].to_vec(), 0, 7).unwrap();
        skate.camera = Some(1);
        skate.spectate(true);
        assert_eq!(skate.camera, Some(1), "S.K.A.T.E.'s camera is the script's");
    }

    /// Every Skate 1 online race row (Death Race, Spot Race), ridden through its way points in order: the next one
    /// shows first, every gate shown is on a placed volume (standing on its floor, inside it; the minimap's mark at its
    /// middle), and the race's end (the finish line, or a looping route's start line on its last lap) shows none after.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn every_race_rows_gates_show_in_order_on_their_volumes() {
        let data = data();
        let mut counted = BTreeMap::<String, usize>::new();
        for row in data.rows.values().filter(|r| r.playable()) {
            let Some(mode @ (Mode::DeathRace | Mode::SpotRace)) = Mode::parse(&row.mode) else { continue };
            let spot = mode == Mode::SpotRace;
            let fallback = stand_in(if spot { "sg000" } else { "rg000" });
            let model = data.gates.get(if spot { "sg000" } else { "rg000" }).unwrap_or(&fallback);
            let (n, looping) = (way_points(row), row.fields["IsLooping"].as_bool().unwrap_or(false));
            let laps_to_go = if looping { row.fields["NumLaps"].as_f64().unwrap_or(1.) as usize } else { 1 };
            let (mut hit, mut laps, mut steps) = (0, 0, 0);
            let mut shown = upcoming(row, mode, 0, 0);
            while let Some(&next) = shown.first() {
                assert_eq!(next, (hit + 1) % n, "{}: after way point {hit}", row.id);
                for &i in &shown {
                    let Some(Place::Volume(v)) = way_point(row, i).and_then(|p| row.places.get(p)) else {
                        panic!("{}: way point {i} has no volume", row.id)
                    };
                    let t = gate_transform(v, way_point_middle(row, i.checked_sub(1).unwrap_or(n - 1)), model, spot);
                    let inside = [0, 2].iter().all(|&k| (v.min[k] - 1e-3..=v.max[k] + 1e-3).contains(&t.translation[k]));
                    assert!(inside && t.translation.y == v.min[1] && t.scale.is_finite() && t.scale.min_element() > 0., "{}: way point {i}: {t:?}", row.id);
                    let mark = way_point_middle(row, i).unwrap();
                    assert!(v.contains(mark.to_array()), "{}: way point {i}'s minimap mark", row.id);
                }
                hit = next;
                shown = upcoming(row, mode, hit, laps as i64);
                if looping && hit == 0 {
                    laps += 1;
                }
                steps += 1;
                assert!(steps <= n * laps_to_go, "{}: no end", row.id);
            }
            assert_eq!((hit, laps), if looping { (0, laps_to_go) } else { (n - 1, 0) }, "{}: the gates ran out early", row.id);
            *counted.entry(row.mode.clone()).or_default() += 1;
        }
        println!("race rows: {counted:?}");
        assert_eq!(counted.len(), 2, "{counted:?}");
    }

    /// Jam on the Library, two players: flythrough, countdown, the 180 s round with both in the region; the final
    /// standings, the leaderboard's exit, complete.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn jam_runs_its_round_to_complete() {
        let data = data();
        let row = &data.rows["jam_financial_library"];
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 7).unwrap();
        run.start(&data).unwrap();
        let region = row.fields["BestTrickRegions"][0].as_str().unwrap().to_owned();
        let (mut began, mut results_from) = (None, None);
        let done = play(&data, &mut run, 60. * 6., |run, f| {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            if run.session.state() == "event_round" {
                let k = *began.get_or_insert(f);
                if f == k + 60 {
                    local.push(("triggerentercollision".into(), json!({"PlayerID": 0, "VolumeID": region})));
                    inbox.push((1, encode("triggerentercollision", &json!({"PlayerID": 1, "VolumeID": region}), 0)));
                }
                if f == k + 120 || f == k + 240 {
                    let start = run.session.tick - 20;
                    let seq = |score| json!({"PlayerID": 0, "SequenceScore": score, "StartTick": start, "StartSimGameTick": start, "Wipedout": false});
                    local.push(("sequencecomplete".into(), seq(800)));
                    inbox.push((1, encode("sequencecomplete", &seq(500), run.session.tick)));
                }
                // Skate 3's rule (user 10-28): a line after a marker return scores nothing (S3 contest_shared 1091).
                if f == k + 300 {
                    let start = run.session.tick - 20;
                    local.push((
                        "sequencecomplete".into(),
                        json!({"PlayerID": 0, "SequenceScore": 5000, "StartTick": start, "StartSimGameTick": start, "Wipedout": false, MARKER_USED: true}),
                    ));
                }
            }
            if run.session.state() == "results" && run.hud.iter().any(|c| c.name == "HUD.SetWinState") && results_from.is_none() {
                results_from = Some(f);
            }
            if results_from.is_some_and(|r| f == r + 600) {
                local.push(("postchallengedisplayexit".into(), json!({})));
            }
            (local, inbox)
        });
        assert!(done.is_some(), "stuck in {}", run.session.state());
        assert!(began.is_some());
        let scores: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.score, s.rank)).collect();
        assert_eq!(scores, [(1600, 1), (1000, 2)], "{:?}", run.standings);
        assert!(run.left);
        assert!(run.hud.iter().any(|c| c.name == "HUD.AddChallengeWinnerMessage"));
        // Skate 3's rule (user 10-28): the round's start sets the session marker (S3 contest_shared 880), as Best Trick's.
        assert_eq!(run.markers, 1, "a session marker at the round's start");
    }

    /// S.K.A.T.E. on the Matrix, two players: Alice leads (the token's draw: 8 % 2) and sets a new flip each round, Bob
    /// copies with the wrong trick and spells S.K.A.T.E.; the winner, the results, complete.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn skate_runs_to_a_winner() {
        let data = data();
        let row = &data.rows["skate_financial_matrix"];
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 8).unwrap();
        run.start(&data).unwrap();
        let spot = row.fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let (mut entered, mut results_from, mut rounds) = (None::<(String, usize)>, None, 0);
        let done = play(&data, &mut run, 60. * 10., |run, f| {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            let state = run.session.state().to_owned();
            if entered.as_ref().is_none_or(|(s, _)| *s != state) {
                entered = Some((state.clone(), f));
                rounds += usize::from(state == "wait_to_enter_spot_volume");
            }
            let k = entered.as_ref().unwrap().1;
            let air = |id: usize, trick: i64| json!({"PlayerID": id, "FirstScorableId": trick, "Wipedout": 0, "S": {"first": trick, "first_type": 2, "spins": 0}});
            match state.as_str() {
                "wait_to_enter_spot_volume" if f == k + 90 => local.push(("triggerentercollision".into(), json!({"PlayerID": 0, "VolumeID": spot}))),
                "leader_attempt" if f == k + 30 => local.push(("aircompleted".into(), air(0, 100 + rounds as i64))),
                "wait_to_enter_spot2" if f == k + 30 => inbox.push((1, encode("triggerentercollision", &json!({"PlayerID": 1, "VolumeID": spot}), 0))),
                "follower_attempt" if f == k + 30 => inbox.push((1, encode("aircompleted", &air(1, 999), 0))),
                "show_results" if results_from.is_none() => results_from = Some(f),
                _ => {}
            }
            if results_from.is_some_and(|r| f == r + 600) {
                local.push(("postchallengedisplayexit".into(), json!({})));
            }
            (local, inbox)
        });
        assert!(done.is_some(), "stuck in {}", run.session.state());
        assert_eq!(rounds, 5);
        let letters: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.letters, s.rank)).collect();
        assert_eq!(letters, [(0, 1), (5, 2)], "{:?}", run.standings);
        assert_eq!(run.result(), 0);
        assert!(run.left);
        let states: Vec<i64> = (run.hud.iter())
            .filter(|c| c.name == "HUD.ChangeHUDState" && c.int(0) == Some(18))
            .filter_map(|c| c.int(1))
            .collect();
        // MessageTrickSet (15) each round, MessageTrickWrong (18) each copy, the leaderboard (11) last.
        assert_eq!(states.iter().filter(|&&s| s == 15).count(), 5);
        assert_eq!(states.iter().filter(|&&s| s == 18).count(), 5);
        assert_eq!(states.last(), Some(&11));
    }

    /// Review 10-28 C1: a follower who leaves in follower_wait (its 2.5 s before the copy) no longer stops the game there
    /// until the 30-minute cap (the disc set TimerID = -1, SKATE 1151-1155): the flow moves on.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn skate_a_follower_leaving_before_its_copy_does_not_stop_the_game() {
        let data = data();
        let row = &data.rows["skate_financial_matrix"];
        // Token 9 of 3: Alice (local, slot 0) leads; Bob then Carl copy.
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into(), "Carl".into()], 0, 9).unwrap();
        run.start(&data).unwrap();
        let spot = row.fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let mut players = run.names.clone();
        let (mut entered, mut left_at, mut moved_on) = (None::<(String, usize)>, None, false);
        for f in 0..60 * 120 {
            let state = run.session.state().to_owned();
            if entered.as_ref().is_none_or(|(s, _)| *s != state) {
                entered = Some((state.clone(), f));
                moved_on |= left_at.is_some() && state != "follower_wait";
            }
            let k = entered.as_ref().unwrap().1;
            let air = json!({"PlayerID": 0, "FirstScorableId": 100, "Wipedout": 0, "S": {"first": 100, "first_type": 2, "spins": 0}});
            let mut local = Vec::new();
            match state.as_str() {
                "wait_to_enter_spot_volume" if f == k + 90 => local.push(("triggerentercollision".into(), json!({"PlayerID": 0, "VolumeID": spot}))),
                "leader_attempt" if f == k + 30 => local.push(("aircompleted".into(), air)),
                // Bob leaves (an empty name: Run::frame's leave).
                "follower_wait" if left_at.is_none() && f == k + 30 => {
                    players[1].clear();
                    left_at = Some(f);
                }
                _ => {}
            }
            run.frame(&data, 1. / 60., local, Vec::new(), &players).unwrap();
            if moved_on {
                break;
            }
        }
        assert!(left_at.is_some(), "Bob never came to copy; now {}", run.session.state());
        assert!(moved_on, "stuck in {} after Bob left", run.session.state());
    }

    /// Own The Spot on fp_u1_ownspot01, two players: the flythrough, then three rounds of turns, each a 3 s countdown
    /// and the row's 10 s with the camera on the skater up; Alice lands 1000 and Bob 1500 through both spot volumes;
    /// Bob wins; the leaderboard's exit, complete. Skate 3's rule (user 10-28, S3 spot_battle.lua 143-144, 263): the
    /// first player is the match token's draw (7 % 2: Bob) and every round starts again from him, slot order after; the
    /// disc went from slot 0 (Alice) every round.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn spot_battle_takes_turns_to_complete() {
        let data = data();
        let row = &data.rows["ol_fp_u1_ownspot01"];
        assert!(row.playable());
        assert!(data.rows["ol_fp_u1_ownspot_04"].playable(), "its volumes are in BAM.xsf's type-4 assets");
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 7).unwrap();
        run.start(&data).unwrap();
        let spots: Vec<String> = row.fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let boundary = row.fields["OTSTriggerBoundary"].as_str().unwrap().to_owned();
        let place = |k: &str| match &row.places[row.fields[k].as_str().unwrap()] {
            Place::Locator(l) => l.clone(),
            _ => panic!("{k} is not a locator"),
        };
        let (attempt, watch) = (place("SkateAttemptSpawnPoint"), place("Location"));
        let (mut turn, mut turns, mut spawns, mut results_from) = (None, Vec::new(), Vec::new(), None);
        let done = play(&data, &mut run, 60. * 4., |run, f| {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            if run.session.state() == "entered_trick_boundary" {
                let active = run.session.var("ActiveSkaterIdx").as_i64().unwrap() as usize - 1;
                let round = run.session.var("CurrentRound").as_i64().unwrap();
                let k = match turn {
                    Some((r, a, k)) if (r, a) == (round, active) => k,
                    _ => {
                        turn = Some((round, active, f));
                        turns.push((round, active, run.camera));
                        spawns.push(run.teleport);
                        f
                    }
                };
                let tick = run.session.tick;
                let mut send = |event: &str, message: Json| match active {
                    0 => local.push((event.to_owned(), message)),
                    _ => inbox.push((active, encode(event, &message, tick))),
                };
                if f == k + 30 {
                    for spot in &spots {
                        send("triggerentercollision", json!({"PlayerID": active, "VolumeID": spot}));
                    }
                }
                // Alice leaves the trick boundary on her last turn (boundarytriggered: the LeftArea substate, and her
                // 1200 there doesn't count).
                if f == k + 60 && (round, active) == (3, 0) {
                    send("triggerexitcollision", json!({"PlayerID": 0, "VolumeID": boundary}));
                }
                if f == k + 90 {
                    let score = match (round, active) {
                        (3, 0) => 1200,
                        (_, 0) => 1000,
                        _ => 1500,
                    };
                    let start = tick - 20;
                    send("sequencecomplete", json!({"PlayerID": active, "SequenceScore": score, "StartTick": start, "StartSimGameTick": start, "Wipedout": false}));
                }
            }
            if run.session.state() == "challenge_teardown" && results_from.is_none() {
                results_from = Some(f);
            }
            if results_from.is_some_and(|r| f == r + 600) {
                local.push(("postchallengedisplayexit".into(), json!({})));
            }
            (local, inbox)
        });
        assert!(done.is_some(), "stuck in {}", run.session.state());
        // Bob first each round, then Alice; the camera follows the other skater on their turn (SwitchCamera to oneself:
        // None).
        assert_eq!(turns, [(1, 1, Some(1)), (1, 0, None), (2, 1, Some(1)), (2, 0, None), (3, 1, Some(1)), (3, 0, None)]);
        // Alice's own spot each turn: RepositionAI(0, SkateAttemptSpawnPoint, 0) is the attempt locator's child 0
        // (7.5 m off its root here), else her slot (0) at the observer locator.
        assert_ne!(attempt.slot(0), attempt.matrix);
        let want: Vec<_> = turns.iter().map(|&(_, a, _)| Some(if a == 0 { attempt.slot(0) } else { watch.slot(0) })).collect();
        assert_eq!(spawns, want);
        let scores: Vec<i64> = run.standings.iter().map(|s| s.score).collect();
        assert_eq!(scores, [1000, 1500], "{:?}", run.standings);
        assert_eq!(run.result(), 1000);
        assert!(run.left);
        let winner = run.hud.iter().find(|c| c.name == "HUD.AddChallengeWinnerMessage").map(|c| c.int(0));
        assert_eq!(winner, Some(Some(1)));
        let left = data.enums["HUD"]["eSk8HUDSpotState"]["LeftArea"].as_i64();
        assert!(run.hud.iter().any(|c| c.name == "HUD.ChangeHUDState" && c.int(1) == left), "no LeftArea substate");
    }

    const SIX: [&str; 6] = ["Ann", "Bo", "Cy", "Dee", "Eve", "Fay"];
    /// A console's own events this frame; ("@leave", _) takes it out of the match.
    type Act<'a> = &'a mut dyn FnMut(usize, &Run, usize) -> Vec<(String, Json)>;

    /// Six consoles each running `row`'s script (online audit 10-12, step 2), their relayed events over the Wire
    /// (150-300 ms, 8 % loss). A console whose `act` says "@leave" goes: what it still had on the way is lost, and the
    /// others notice it apart, 0.2 s + 0.1 s per slot later, and blank its name once the rest have sent on its events
    /// (match_core Leaves). Each console leaves each results screen 10 s
    /// after its script reaches `results`. Every console still there must complete within `cap` s; returns the runs (a
    /// leaver's None). The match token is `seed` down to a multiple of six, so its draw (the first S.K.A.T.E. leader,
    /// Spot Battle's first player: token % 6) is Ann, as the hand-computed orders have it; the Wire draws from `seed`.
    fn six_consoles(data: &Data, row: &str, seed: u64, cap: f32, results: &str, act: Act) -> Vec<Option<Run>> {
        let names: Vec<String> = SIX.map(String::from).to_vec();
        let row = &data.rows[row];
        let mut runs: Vec<Option<Run>> = (0..6)
            .map(|me| {
                let mut run = Run::new(data, row, names.clone(), me, seed - seed % 6).unwrap();
                run.start(data).unwrap();
                Some(run)
            })
            .collect();
        let mut wire = crate::skate3_modes::tests::Wire::new(seed, (0.15, 0.3), 0.08);
        let (mut shown, mut left) = ([None::<usize>; 6], [None::<usize>; 6]);
        for f in 0..(cap * 60.) as usize {
            if runs.iter().flatten().all(Run::complete) {
                return runs;
            }
            let now = f as f64 / 60.;
            for me in 0..6 {
                let Some(run) = runs[me].as_mut().filter(|r| !r.complete()) else { continue };
                let mut local = act(me, &*run, f);
                if local.iter().any(|(e, _)| e == "@leave") {
                    (runs[me], left[me]) = (None, Some(f));
                    (0..6).for_each(|to| wire.cut(me, to));
                    continue;
                }
                if run.session.state() != results {
                    shown[me] = None;
                } else if f == *shown[me].get_or_insert(f) + 600 {
                    local.push(("postchallengedisplayexit".into(), json!({})));
                }
                // Heard 12 + 6 * me frames after; given once every other console has sent its mark (match_core Leaves).
                let mut heard = |i: usize| left[i].is_some_and(|at| f >= at + 12 + 6 * me) && wire.left(now, me, i, 6);
                let players: Vec<String> = (0..6).map(|i| if heard(i) { String::new() } else { names[i].clone() }).collect();
                let inbox = wire.take(now, me);
                run.frame(data, 1. / 60., local, inbox, &players).unwrap_or_else(|e| panic!("console {me}: {e}"));
                let out = std::mem::take(&mut run.outbox);
                if !out.is_empty() {
                    wire.send(now, me, 6, out);
                }
            }
        }
        let stuck: Vec<_> = runs.iter().flatten().filter(|r| !r.complete()).map(|r| (r.me(), r.session.state().to_owned())).collect();
        panic!("{}: stuck after {cap} s: {stuck:?}", row.id);
    }

    /// The frame each console entered its script's current state (per console).
    #[derive(Default)]
    struct Entered(Vec<(String, usize)>);
    impl Entered {
        /// (frames in the state, whether it is newly entered).
        fn note(&mut self, me: usize, run: &Run, f: usize) -> (usize, bool) {
            self.0.resize(6, (String::new(), 0));
            // Keyed with the leader: leader_failure -> round_over -> the same state can run within one frame.
            let state = format!("{}/{}", run.session.state(), run.session.var("LeaderID"));
            let new = self.0[me].0 != state;
            if new {
                self.0[me] = (state, f);
            }
            (f - self.0[me].1, new)
        }
    }

    /// S.K.A.T.E.'s players, each on its own console: the leader rides into the spot and sets a new flip each round
    /// (100 + the round), a follower rides in and lands the same flip, or (`fails(slot, round)`) another one and takes a
    /// letter. Each rides in 1.5 s into its state and lands 0.5 s after; `leaves` = (slot, round, into, after) leaves
    /// `into` frames into that round on its console, or with `after` that many frames after it set that round's trick
    /// as the leader.
    fn skate_players(spot: String, fails: fn(usize, usize) -> bool, leaves: (usize, usize, usize, Option<usize>)) -> impl FnMut(usize, &Run, usize) -> Vec<(String, Json)> {
        let (mut entered, mut rounds, mut set) = (Entered::default(), [0; 6], [None::<(usize, usize)>; 6]);
        move |me, run, f| {
            let (k, new) = entered.note(me, run, f);
            let state = run.session.state();
            rounds[me] += usize::from(new && state == "wait_to_enter_spot_volume");
            let round = rounds[me];
            let after = leaves.3.map(|d| set[me].is_some_and(|(r, at)| r == round && f == at + d));
            if (me, round) == (leaves.0, leaves.1) && after.unwrap_or(k >= leaves.2) {
                return vec![("@leave".into(), json!({}))];
            }
            let vars = run.session.machine.vars();
            let leader = vars.get::<i64>("LeaderID").ok() == Some(me as i64);
            let follower = (|| vars.get::<mlua::Table>("FollowerOrder").ok()?.get::<Option<i64>>(vars.get::<i64>("FollowerIndex").ok()?).ok()?)();
            let follower = follower == Some(me as i64);
            let air = |trick: i64| ("aircompleted".to_owned(), json!({"PlayerID": me, "FirstScorableId": trick, "Wipedout": 0, "S": {"first": trick, "first_type": 2, "spins": 0}}));
            let enter = || ("triggerentercollision".to_owned(), json!({"PlayerID": me, "VolumeID": spot}));
            let set_trick = 100 + round as i64;
            match state {
                "wait_to_enter_spot_volume" if leader && k == 90 => vec![enter()],
                "leader_attempt" if leader && k == 30 => {
                    set[me] = Some((round, f));
                    vec![air(set_trick)]
                }
                "wait_to_enter_spot2" if follower && k == 90 => vec![enter()],
                "follower_attempt" if follower && k == 30 => vec![air(if fails(me, round) { 999 } else { set_trick })],
                _ => vec![],
            }
        }
    }

    /// Every console still there ended with `ranks` (by slot) and the same letters, and left.
    fn skate_ranks_agree(runs: &[Option<Run>], ranks: [i64; 6]) {
        let letters: Vec<Vec<i64>> = runs.iter().flatten().map(|r| r.standings.iter().map(|s| s.letters).collect()).collect();
        for run in runs.iter().flatten() {
            let got: Vec<i64> = run.standings.iter().map(|s| s.rank).collect();
            assert_eq!(got, ranks, "console {}: {:?}", run.me(), run.standings);
            assert!(run.left, "console {} never left", run.me());
        }
        assert!(letters.windows(2).all(|w| w[0] == w[1]), "the consoles' letters differ: {letters:?}");
        assert_eq!(runs.iter().flatten().count(), 5);
    }

    /// Six on the Matrix (online audit 10-12, step 2), each console its own script over the Wire: Ann leads every
    /// round; follower p lands the wrong flip from round p on, so Bo spells S.K.A.T.E. in round 5, Cy in 6, Eve in 8
    /// and Fay in 9; Dee leaves as round 3 starts. Hand-computed from the script: a player out takes rank 2
    /// (CurrentRank) and those out before move down one (follower_failure); the winner is 1, and one who left
    /// unranked takes the last (winner: NumSkaters - unranked + 1).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_skate_a_guest_leaves_and_every_console_ranks_alike() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let mut players = skate_players(spot, |p, r| r >= p, (3, 3, 0, None));
        let runs = six_consoles(&data, "skate_financial_matrix", 30, 900., "show_results", &mut players);
        skate_ranks_agree(&runs, [1, 5, 4, 6, 3, 2]);
    }

    /// The host (Ann, leading) leaves 0.5 s into round 2: leader_failure hands the lead to the first follower still
    /// there, Bo, who sets every round after; Cy is out in round 6, Dee 7, Eve 8, Fay 9. Ann, unranked, is last. Not
    /// at the round's start: a leader's leave heard before NewWaitTimer (1 s, lua 174) loses its ResetCountdownTimer
    /// (243) to the fresh 12 s AttemptTimerID (217), after it costs 1 s, so consoles hearing it either side of that
    /// second part by 11 s: retail's own race, and the notices of a leave at the start fall astride it once the relay
    /// waits for every console's mark (end review I2).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_skate_the_host_leaves_and_every_console_ranks_alike() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let mut players = skate_players(spot, |p, r| r >= p, (0, 2, 30, None));
        let runs = six_consoles(&data, "skate_financial_matrix", 31, 900., "show_results", &mut players);
        skate_ranks_agree(&runs, [6, 1, 5, 4, 3, 2]);
    }

    /// End review I2 (10-12): Ann (leading) sets round 2's flip and leaves 0.2 s later, the set on the way to some
    /// consoles only. By hand, every console voids it, set or not: leader_attempt's playerdisconnected (406-415) cuts
    /// AttemptTimerID to 1 s, whose expiry clears AttemptSuccessful (283-287) before the set's WaitTime (2.5 s) ends;
    /// leader_failure makes Bo the leader, as in the host-leaves test: the same ranks, Bo on his round-1 letter only
    /// (the set would have given him a second). It passed before the relay fix too (the Lua converges while the leave
    /// is heard within 1.5 s of the set); it guards that the leave, now given once every console has the set, still is.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_skate_a_leader_leaving_after_its_set_ranks_alike() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        for seed in 32..40 {
            let mut players = skate_players(spot.clone(), |p, r| r >= p, (0, 2, 0, Some(12)));
            let runs = six_consoles(&data, "skate_financial_matrix", seed, 900., "show_results", &mut players);
            skate_ranks_agree(&runs, [6, 1, 5, 4, 3, 2]);
            assert_eq!(runs[1].as_ref().unwrap().standings[1].letters, 1, "seed {seed}: Bo's letters (the set voided)");
        }
    }

    /// Death Race racers, each on its own console: racer p takes way point g (1.. the CheckPoints, then the finish
    /// line) `pace[p] * g` frames into the race, but for `skips` (racer, way point); `leaves` = (racer, frame) leaves.
    fn racers(row: &Row, pace: [usize; 6], skips: (usize, usize), leaves: (usize, usize)) -> impl FnMut(usize, &Run, usize) -> Vec<(String, Json)> + use<> {
        let mut volumes: Vec<String> = row.fields["CheckPoints"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        volumes.push(row.fields["FinishLine"].as_str().unwrap().to_owned());
        let mut began = [None::<usize>; 6];
        move |me, run, f| {
            if run.session.state() == "skating" {
                began[me].get_or_insert(f);
            }
            let Some(k) = began[me] else { return vec![] };
            if (me, f - k) == leaves {
                return vec![("@leave".into(), json!({}))];
            }
            let g = (f - k) / pace[me];
            if (f - k) % pace[me] != 0 || g == 0 || g > volumes.len() || (me, g) == skips {
                return vec![];
            }
            vec![("triggerentercollision".into(), json!({"PlayerID": me, "VolumeID": volumes[g - 1]}))]
        }
    }

    /// Every console still there placed the racers in `order` (slots), with each finisher's time (penalty in) as its
    /// own console clocked it, and left.
    fn race_agrees(runs: &[Option<Run>], order: [usize; 6], times: [Option<f32>; 6]) {
        for run in runs.iter().flatten() {
            assert_eq!(run.order, order, "console {}: {:?}", run.me(), run.standings);
            for (p, want) in times.iter().enumerate() {
                let s = &run.standings[p];
                match want {
                    Some(t) => assert!(s.finished && (s.time - t).abs() < 0.02, "console {}: racer {p}: {s:?}", run.me()),
                    None => assert!(s.out, "console {}: racer {p} isn't out: {s:?}", run.me()),
                }
            }
            assert!(run.left, "console {} never left", run.me());
        }
        assert_eq!(runs.iter().flatten().count(), 5);
    }

    /// Six on the Old Town spillway (6 CheckPoints, 6 s a missed gate), each console its own script over the Wire:
    /// Ann takes a way point every 1 s, Bo 1.1 s, Cy 1.2 s but skips gate 3, Dee 1.3 s, Eve leaves 4 s in, Fay 1.317 s
    /// (0.12 s behind Dee at the line: closer than the wire). Hand-computed (0x823da688): finishers by their clock plus
    /// penalty (Ann 7, Bo 7.7, Dee 9.1, Fay 9.217, Cy 8.4 + 6), then the one who left.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_death_race_a_guest_leaves_and_every_console_places_alike() {
        let data = data();
        let mut racers = racers(&data.rows["ol_deathrace_oldtown_spillway"], [60, 66, 72, 78, 80, 79], (2, 3), (4, 240));
        let runs = six_consoles(&data, "ol_deathrace_oldtown_spillway", 32, 300., "show_result", &mut racers);
        race_agrees(&runs, [0, 1, 3, 5, 2, 4], [Some(7.), Some(7.7), Some(14.4), Some(9.1), None, Some(9.217)]);
    }

    /// The host (Ann) leaves 3 s into the race: the rest finish (Eve 1.4 s a way point), Ann last.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_death_race_the_host_leaves_and_every_console_places_alike() {
        let data = data();
        let mut racers = racers(&data.rows["ol_deathrace_oldtown_spillway"], [60, 66, 72, 78, 84, 79], (2, 3), (0, 180));
        let runs = six_consoles(&data, "ol_deathrace_oldtown_spillway", 33, 300., "show_result", &mut racers);
        race_agrees(&runs, [1, 3, 5, 4, 2, 0], [None, Some(7.7), Some(14.4), Some(9.1), Some(9.8), Some(9.217)]);
    }

    /// Contest players (Jam, Best Trick), each on its own console: each round, 1 s in, ride into the round's spot (if
    /// any) and 1 s later land a sequence worth `score(slot)`; `leaves` = (slot, round) leaves 0.5 s into that round.
    fn contest_players(spots: Vec<String>, score: fn(usize) -> i64, leaves: (usize, i64)) -> impl FnMut(usize, &Run, usize) -> Vec<(String, Json)> + use<> {
        let mut began: BTreeMap<(usize, i64), usize> = BTreeMap::new();
        move |me, run, f| {
            if run.session.state() != "event_round" {
                return vec![];
            }
            let round = run.session.var("CurrentRound").as_i64().unwrap();
            let k = *began.entry((me, round)).or_insert(f);
            if (me, round) == leaves && f == k + 30 {
                return vec![("@leave".into(), json!({}))];
            }
            match f - k {
                60 if !spots.is_empty() => vec![("triggerentercollision".into(), json!({"PlayerID": me, "VolumeID": spots[round as usize - 1]}))],
                120 => {
                    let start = run.session.tick - 20;
                    vec![("sequencecomplete".into(), json!({"PlayerID": me, "SequenceScore": score(me), "StartTick": start, "StartSimGameTick": start, "Wipedout": false}))]
                }
                _ => vec![],
            }
        }
    }

    /// Every console still there ended with `ranks` and `points` (by slot) and left.
    fn contest_agrees(runs: &[Option<Run>], ranks: [i64; 6], points: [i64; 6]) {
        skate_ranks_agree(runs, ranks);
        for run in runs.iter().flatten() {
            let got: Vec<i64> = run.standings.iter().map(|s| s.points).collect();
            assert_eq!(got, points, "console {}: {:?}", run.me(), run.standings);
        }
    }

    /// Six in the Library Jam (one 180 s round; online audit 10-12, step 2), each console its own script over the Wire:
    /// slot p scores 200 (p + 1); Cy leaves before scoring. Hand-computed from the script's results: round points 5 down
    /// to 0 by score (Cy's 0 takes 0), the rank each player's place in the exchange sort of totals (SetOverallStanding's
    /// last argument; the leavers-last pass only reorders the calls).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_jam_a_guest_leaves_and_every_console_ranks_alike() {
        let data = data();
        let mut players = contest_players(vec![], |p| 200 * (p as i64 + 1), (2, 1));
        let runs = six_consoles(&data, "jam_financial_library", 34, 300., "results", &mut players);
        contest_agrees(&runs, [5, 4, 6, 3, 2, 1], [1, 2, 0, 3, 4, 5]);
    }

    /// The host (Ann) leaves the Jam before scoring: last on 0.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_jam_the_host_leaves_and_every_console_ranks_alike() {
        let data = data();
        let mut players = contest_players(vec![], |p| 200 * (p as i64 + 1), (0, 1));
        let runs = six_consoles(&data, "jam_financial_library", 35, 300., "results", &mut players);
        contest_agrees(&runs, [6, 5, 4, 3, 2, 1], [0, 1, 2, 3, 4, 5]);
    }

    /// Six in the Library Best Trick (3 rounds of 120 s, a spot each): slot p lands 1000 (p + 1) a round; Cy leaves
    /// as round 2 starts. Round 1 points 0..5 by slot; rounds 2 and 3 Cy 0, Ann 1, Bo 2, Dee 3, Eve 4, Fay 5. Totals
    /// 2, 5, 2, 9, 12, 15: Ann and Cy tie, and the exchange sort puts Cy (who left) 5th and Ann 6th, as retail's
    /// rank argument has it.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_best_trick_a_guest_leaves_and_every_console_ranks_alike() {
        let data = data();
        let spots = data.rows["besttrick_financial_library"].fields["BestTrickSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut players = contest_players(spots, |p| 1000 * (p as i64 + 1), (2, 2));
        let runs = six_consoles(&data, "besttrick_financial_library", 36, 600., "results", &mut players);
        contest_agrees(&runs, [6, 4, 5, 3, 2, 1], [2, 5, 2, 9, 12, 15]);
    }

    /// The host (Ann) leaves Best Trick as round 2 starts: Ann's round-1 score was the lowest (0 points), so last.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_best_trick_the_host_leaves_and_every_console_ranks_alike() {
        let data = data();
        let spots = data.rows["besttrick_financial_library"].fields["BestTrickSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut players = contest_players(spots, |p| 1000 * (p as i64 + 1), (0, 2));
        let runs = six_consoles(&data, "besttrick_financial_library", 37, 600., "results", &mut players);
        contest_agrees(&runs, [6, 5, 4, 3, 2, 1], [0, 3, 6, 9, 12, 15]);
    }

    /// Own The Spot's skaters, each on its own console: on its turn a skater rides into every spot 0.5 s in and lands
    /// 100 (slot + 1) x the round 1 s later; `leaves` = (slot, round, the active skater's slot, frames) leaves that many
    /// frames into that turn.
    fn spot_players(spots: Vec<String>, leaves: (usize, i64, usize, usize)) -> impl FnMut(usize, &Run, usize) -> Vec<(String, Json)> + use<> {
        let mut began: BTreeMap<(usize, i64, usize), usize> = BTreeMap::new();
        move |me, run, f| {
            if run.session.state() != "entered_trick_boundary" {
                return vec![];
            }
            let active = run.session.var("ActiveSkaterIdx").as_i64().unwrap() as usize - 1;
            let round = run.session.var("CurrentRound").as_i64().unwrap();
            let k = *began.entry((me, round, active)).or_insert(f);
            if (me, round, active, f - k) == leaves {
                return vec![("@leave".into(), json!({}))];
            }
            if active != me {
                return vec![];
            }
            match f - k {
                30 => spots.iter().map(|v| ("triggerentercollision".to_owned(), json!({"PlayerID": me, "VolumeID": v}))).collect(),
                90 => {
                    let start = run.session.tick - 20;
                    let score = 100 * (me as i64 + 1) * round;
                    vec![("sequencecomplete".into(), json!({"PlayerID": me, "SequenceScore": score, "StartTick": start, "StartSimGameTick": start, "Wipedout": false}))]
                }
                _ => vec![],
            }
        }
    }

    /// Every console still there placed the skaters in `order` with these best scores, and left.
    fn spot_agrees(runs: &[Option<Run>], order: [usize; 6], best: [i64; 6]) {
        for run in runs.iter().flatten() {
            assert_eq!(run.order, order, "console {}: {:?}", run.me(), run.standings);
            assert_eq!(run.standings.iter().map(|s| s.score).collect::<Vec<_>>(), best, "console {}", run.me());
            assert!(run.left, "console {} never left", run.me());
        }
        assert_eq!(runs.iter().flatten().count(), 5);
    }

    /// Six on Own The Spot fp_u1_ownspot01 (3 rounds of turns in slot order; online audit 10-12, step 2), each console
    /// its own script over the Wire: Dee leaves in Bo's round-2 turn, so her turns are skipped (getNextSkaterIdx).
    /// Hand-computed (each skater's best, the manager comparator 0x823da600: one who left last, then the higher score):
    /// Fay 1800, Eve 1500, Cy 900, Bo 600, Ann 300, Dee (400, left).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_spot_battle_a_guest_leaves_and_every_console_places_alike() {
        let data = data();
        let spots = data.rows["ol_fp_u1_ownspot01"].fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut players = spot_players(spots, (3, 2, 1, 45));
        let runs = six_consoles(&data, "ol_fp_u1_ownspot01", 38, 600., "challenge_teardown", &mut players);
        spot_agrees(&runs, [5, 4, 2, 1, 0, 3], [300, 600, 900, 400, 1500, 1800]);
    }

    /// The host (Ann) leaves 0.75 s into her own round-2 turn, before landing: every console cuts the turn
    /// (DCToolTimer) and plays on; Ann (100 from round 1) is last.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_spot_battle_the_host_leaves_and_every_console_places_alike() {
        let data = data();
        let spots = data.rows["ol_fp_u1_ownspot01"].fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut players = spot_players(spots, (0, 2, 0, 45));
        let runs = six_consoles(&data, "ol_fp_u1_ownspot01", 39, 600., "challenge_teardown", &mut players);
        spot_agrees(&runs, [5, 4, 3, 2, 1, 0], [100, 600, 900, 1200, 1500, 1800]);
    }

    /// End review I2 (10-12): Bo lands 400 in his round-2 turn and leaves 0.2 s later, the landing on the way to some
    /// consoles only (or none). Hand-computed (0x823da600: one who left last, then the higher score): Bo's 400 counts
    /// on every console if any took it, else his 200 on every one (before the fix the consoles it missed kept his 200):
    /// Fay 1800, Eve 1500, Dee 1200, Cy 900, Ann 300, Bo (400 / 200, left).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_spot_battle_a_landing_sent_as_its_skater_leaves_counts_everywhere() {
        let data = data();
        let spots: Vec<String> = data.rows["ol_fp_u1_ownspot01"].fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut counted = 0;
        for seed in 42..48 {
            let mut players = spot_players(spots.clone(), (1, 2, 1, 102));
            let runs = six_consoles(&data, "ol_fp_u1_ownspot01", seed, 600., "challenge_teardown", &mut players);
            let bo = runs[0].as_ref().unwrap().standings[1].score;
            assert!(bo == 400 || bo == 200, "seed {seed}: Bo's best {bo}");
            spot_agrees(&runs, [5, 4, 3, 2, 0, 1], [300, bo, 900, 1200, 1500, 1800]);
            counted += usize::from(bo == 400);
        }
        assert!(counted > 0, "no console ever took the landing: nothing tested");
    }

    /// Spot Race racers, each on its own console: racer p reaches way point g (1.. the CheckPoints, then the finish
    /// line) `pace[p] * g` frames into the race and earns each gate as its SpotTrickTypes / SpotSeqRequiredPoints ask (a
    /// trick of the type, or any trick and a sequence of the points; a Grab gate by being in a grab as it leaves the
    /// volume), but for `skips` (racer, gate: it rides through without the trick); `leaves` = (racer, frame) leaves.
    fn spot_racers(data: &Data, row: &Row, pace: [usize; 6], skips: (usize, usize), leaves: (usize, usize)) -> impl FnMut(usize, &Run, usize) -> Vec<(String, Json)> + use<> {
        let names = |k: &str| -> Vec<String> { row.fields[k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect() };
        let gates = names("CheckPoints");
        let finish = row.fields["FinishLine"].as_str().unwrap().to_owned();
        let kinds = &data.enums["Trick"]["eTrickType"];
        let types: Vec<i64> = names("SpotTrickTypes").iter().map(|t| kinds[t.as_str()].as_i64().unwrap()).collect();
        let points: Vec<i64> = row.fields["SpotSeqRequiredPoints"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        let grab = kinds["Grab"].as_i64().unwrap();
        let mut began = [None::<usize>; 6];
        move |me, run, f| {
            if run.session.state() == "skating" {
                began[me].get_or_insert(f);
            }
            let Some(k) = began[me] else { return vec![] };
            if (me, f - k) == leaves {
                return vec![("@leave".into(), json!({}))];
            }
            let (g, at) = ((f - k) / pace[me], (f - k) % pace[me]);
            if g == 0 || g > gates.len() + 1 {
                return vec![];
            }
            let volume = if g > gates.len() { &finish } else { &gates[g - 1] };
            let enter = ("triggerentercollision".to_owned(), json!({"PlayerID": me, "VolumeID": volume}));
            if g > gates.len() || (me, g) == skips {
                return if at == 0 { vec![enter] } else { vec![] };
            }
            let (kind, need) = (types[g - 1], points[g - 1]);
            let by_exit = need == 0 && kind == grab;
            let start = run.session.tick - 5;
            match at {
                0 => vec![enter],
                3 if !by_exit => vec![("trickcomplete".into(), json!({"PlayerID": me, "TrickType": if need > 0 { 1 } else { kind }}))],
                6 if by_exit => {
                    let state = json!({"air": true, "trick": true, "types": {kind.to_string(): true}});
                    vec![("triggerexitcollision".into(), json!({"PlayerID": me, "VolumeID": volume, "S": state}))]
                }
                9 => vec![("sequencecomplete".into(), json!({"PlayerID": me, "SequenceScore": need, "Wipedout": false, "StartTick": start, "StartSimGameTick": start}))],
                _ => vec![],
            }
        }
    }

    /// Six on the Old Town warehouse Spot Race (11 trick gates, none may be missed; online audit 10-12, step 2), each
    /// console its own script over the Wire: racer p takes a way point every 50 + 5p frames (Fay 66: 0.2 s behind Dee at
    /// the line, closer than the wire); Cy rides through gate 5 without its trick, so the gates after it don't count and
    /// the 45 s hurry-up ends her race (DNF at ChallengeStop); Eve leaves 4 s in. Hand-computed (0x823da688):
    /// finishers by their clock (Ann 10, Bo 11, Dee 13, Fay 13.2), then the out ones as they stood (Cy, still racing
    /// until the stop, ahead of Eve, out since she left; both clockless, so the insertion sort keeps them).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_spot_race_a_guest_leaves_and_every_console_places_alike() {
        let data = data();
        let mut racers = spot_racers(&data, &data.rows["spotrace_oldtown_warehouse"], [50, 55, 60, 65, 70, 66], (2, 5), (4, 240));
        let runs = six_consoles(&data, "spotrace_oldtown_warehouse", 40, 300., "show_result", &mut racers);
        race_agrees(&runs, [0, 1, 3, 5, 2, 4], [Some(10.), Some(11.), None, Some(13.), None, Some(13.2)]);
    }

    /// The host (Ann) leaves 3 s into the Spot Race; the rest finish, Ann last.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_spot_race_the_host_leaves_and_every_console_places_alike() {
        let data = data();
        let mut racers = spot_racers(&data, &data.rows["spotrace_oldtown_warehouse"], [50, 55, 60, 65, 70, 66], (9, 9), (0, 180));
        let runs = six_consoles(&data, "spotrace_oldtown_warehouse", 41, 300., "show_result", &mut racers);
        race_agrees(&runs, [1, 2, 3, 5, 4, 0], [None, Some(11.), Some(12.), Some(13.), Some(14.), Some(13.2)]);
    }

    // ---------------------------------------------------------------- Skate 3's rules (user 10-28, fix-brief-3.md)
    /// Alice (this console) and Bob on the Matrix S.K.A.T.E. under match token `token`, started.
    fn skate_run(data: &Data, token: u64) -> Run {
        let mut run = Run::new(data, &data.rows["skate_financial_matrix"], vec!["Alice".into(), "Bob".into()], 0, token).unwrap();
        run.start(data).unwrap();
        run
    }
    /// `run` on at 60 Hz with no events until its script is in `state` (at most `seconds`).
    fn until(data: &Data, run: &mut Run, state: &str, seconds: f32) {
        let players = run.names.clone();
        for _ in 0..(seconds * 60.) as usize {
            if run.session.state() == state {
                return;
            }
            run.frame(data, 1. / 60., Vec::new(), Vec::new(), &players).unwrap();
        }
        panic!("never in {state}: in {}", run.session.state());
    }
    /// `frames` frames at 60 Hz, the first with this console's `local` events and the others' `inbox`.
    fn step(data: &Data, run: &mut Run, frames: usize, local: Vec<(String, Json)>, inbox: Vec<(usize, String)>) {
        let players = run.names.clone();
        let (mut local, mut inbox) = (Some(local), Some(inbox));
        for _ in 0..frames {
            run.frame(data, 1. / 60., local.take().unwrap_or_default(), inbox.take().unwrap_or_default(), &players).unwrap();
        }
    }
    fn enter(id: usize, volume: &str) -> (String, Json) {
        ("triggerentercollision".into(), json!({"PlayerID": id, "VolumeID": volume}))
    }
    /// A landed air whose first trick is the flip `trick`, no 180s.
    fn air(id: usize, trick: i64) -> (String, Json) {
        ("aircompleted".into(), json!({"PlayerID": id, "FirstScorableId": trick, "Wipedout": 0, "S": {"first": trick, "first_type": 2, "spins": 0}}))
    }
    fn sequence(id: usize, score: i64, tick: i64) -> (String, Json) {
        ("sequencecomplete".into(), json!({"PlayerID": id, "SequenceScore": score, "StartTick": tick - 20, "StartSimGameTick": tick - 20, "Wipedout": false}))
    }
    /// Another console's event, as its relay brings it.
    fn from(id: usize, (event, message): (String, Json)) -> (usize, String) {
        (id, encode(&event, &message, 0))
    }
    fn var(run: &Run, name: &str) -> i64 {
        run.session.var(name).as_i64().unwrap_or_else(|| panic!("{name}"))
    }

    /// A peer's step off the board (skateroffboard) and its sequence's marker return (UsedSessionMarker) reach this
    /// console's script. Old code: decode dropped both (skateroffboard not RELAYED; `fits` had no UsedSessionMarker).
    #[test]
    fn a_peer_s_off_board_and_marker_return_ride_the_relay() {
        let (event, m) = decode(&encode("skateroffboard", &json!({"PlayerID": 0, "S": {"air": false}}), 0), 1, 0).unwrap();
        assert_eq!((event.as_str(), &m["PlayerID"]), ("skateroffboard", &json!(1)));
        let (_, m) = decode(&encode("sequencecomplete", &json!({"SequenceScore": 9000, MARKER_USED: true}), 0), 1, 0).unwrap();
        assert_eq!(m[MARKER_USED], json!(true));
        assert_eq!(decode(&encode("sequencecomplete", &json!({MARKER_USED: 1}), 0), 1, 0), None, "of its type only");
    }

    /// Bug 11-01: the trick state the probe puts on every event gained the gesture id and switch, which `fits` didn't
    /// know, so every peer event was dropped: Party Play's consoles split (two turns at once, the pad passed back and
    /// forth every frame) and online players went unheard.
    #[test]
    fn a_peer_hears_the_probes_trick_state_whole() {
        let landed = Probe { first: Some(0), kinds: vec![2], labels: vec!["Kickflip".into()], idents: vec!["kickflip"], switch: true, ..Default::default() };
        for p in [Probe::default(), landed] {
            let state = p.trick_state(true, true, [("2".to_owned(), json!(true))].into_iter().collect());
            for event in RELAYED {
                let got = decode(&encode(event, &json!({"PlayerID": 0, "S": state}), 0), 1, 0);
                assert_eq!(got.map(|(_, m)| m["S"].clone()), Some(state.clone()), "{event}");
            }
        }
    }

    /// Skate 3's UsedSessionMarker: a marker return (SessionMarker::uses moving on) flags the next sequence, once; the
    /// count the match starts on is no return. Old code: no `note_marker` (does not compile).
    #[test]
    fn a_marker_return_flags_the_next_sequence_only() {
        let mut p = Probe::default();
        p.note_marker(Some(4));
        assert!(!p.marked, "the count as the match found it");
        p.note_marker(Some(4));
        assert!(!p.marked);
        p.note_marker(Some(5));
        assert!(std::mem::take(&mut p.marked), "a return");
        p.note_marker(None);
        p.note_marker(Some(5));
        assert!(!p.marked, "taken by its sequence; no new return");
    }

    /// Skate 3's rule (user 10-28): S.K.A.T.E.'s first leader is drawn at random (S3 skate.lua 137), the same on every
    /// console: the match token's draw (token % players, as skate3_skate_game's), the followers after it in slot
    /// order, wrapping; only the leader's console goes to the attempt spawn. Old code: slot 0 led for every token.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn skate_first_leader_is_drawn_from_the_match_token_on_every_console() {
        let data = data();
        let row = &data.rows["skate_financial_matrix"];
        let locator = |field: &str| match &row.places[row.fields[field].as_str().unwrap()] {
            Place::Locator(l) => l.clone(),
            _ => panic!("{field} is not a locator"),
        };
        let (spawn, wait) = (locator("SkateAttemptSpawnPoint").slot(0), locator("SkateWaitingLocation"));
        let names: Vec<String> = SIX[..3].iter().map(|s| s.to_string()).collect();
        for token in 0..6u64 {
            let lead = (token % 3) as i64;
            for me in 0..3 {
                let mut run = Run::new(&data, row, names.clone(), me, token).unwrap();
                run.start(&data).unwrap();
                until(&data, &mut run, "challenge_setup", 60.);
                let at = format!("token {token}, console {me}");
                assert_eq!(var(&run, "LeaderID"), lead, "{at}");
                let order: Vec<i64> = run.session.machine.vars().get::<mlua::Table>("FollowerOrder").unwrap().sequence_values::<i64>().map(Result::unwrap).collect();
                assert_eq!(order, [(lead + 1) % 3, (lead + 2) % 3], "{at}");
                let first = run.hud.iter().find(|c| c.name == "Skate.SetPlayerOrder" && c.int(1) == Some(1)).and_then(|c| c.int(0));
                assert_eq!(first, Some(lead), "{at}: the board's leader");
                assert_eq!(run.teleport, Some(if me as i64 == lead { spawn } else { wait.slot(me) }), "{at}");
            }
        }
    }

    /// Skate 3's rule (user 10-28): once every follower has copied, the lead passes to the next player, no letter (S3
    /// skate.lua 1162-1164, 1440-1445). Six consoles over the Wire, Ann leading: everyone copies in round 1, so Bo leads
    /// from round 2, where every follower misses each round until round 6 puts them out in order: Cy, Dee, Eve, Fay, then
    /// Ann (the old leader, last in the order). Hand-computed as the other S.K.A.T.E. tests (one out takes rank 2, those
    /// out before move down one; the winner is 1). Old code: Ann kept the lead and won.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn six_over_the_wire_skate_the_lead_passes_when_every_follower_copies() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let mut players = skate_players(spot, |_, r| r >= 2, (6, 0, 0, None));
        let runs = six_consoles(&data, "skate_financial_matrix", 48, 900., "show_results", &mut players);
        for run in runs.iter().flatten() {
            let got: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.rank, s.letters)).collect();
            assert_eq!(got, [(2, 5), (1, 0), (6, 5), (5, 5), (4, 5), (3, 5)], "console {}", run.me());
            assert!(run.left, "console {} never left", run.me());
        }
        assert_eq!(runs.iter().flatten().count(), 6);
    }

    /// Skate 3's rule (user 10-28): stepping off the board once in the spot fails the attempt (S3 skate.lua 619-627: the
    /// leader's trick is invalid; 1149-1156: the follower's not copied), shown as the disc's own (Trick not set / not
    /// copied) with its WaitTime; before the spot it does nothing; it is relayed. Old code: nothing handled it, the
    /// attempt ran on to its clock.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn skate_stepping_off_the_board_in_the_spot_fails_the_attempt() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let states = &data.enums["HUD"]["eSk8HUDSkateState"];
        let shown = |run: &Run, k: &str| run.hud.iter().any(|c| c.name == "HUD.ChangeHUDState" && c.int(1) == states[k].as_i64());
        let off = |id: usize| ("skateroffboard".to_owned(), json!({"PlayerID": id}));
        let mut run = skate_run(&data, 8);
        until(&data, &mut run, "wait_to_enter_spot_volume", 60.);
        step(&data, &mut run, 90, vec![], vec![]);
        step(&data, &mut run, 1, vec![off(0)], vec![]);
        assert!(!shown(&run, "MessageTrickNotSet"), "before the spot: nothing");
        step(&data, &mut run, 1, vec![enter(0, &spot)], vec![]);
        step(&data, &mut run, 30, vec![off(0)], vec![]);
        assert!(shown(&run, "MessageTrickNotSet"), "in the spot: the leader's trick is invalid");
        assert!(run.outbox.iter().any(|t| decode(t, 0, 0).is_some_and(|(e, _)| e == "skateroffboard")), "relayed");
        step(&data, &mut run, 130, vec![], vec![]);
        assert_eq!(var(&run, "LeaderID"), 1, "WaitTime on: leader_failure, Bob leads");
        // Bob sets a trick; Alice copies from the spot and steps off: not copied, her letter WaitTime on.
        step(&data, &mut run, 90, vec![], vec![]);
        step(&data, &mut run, 1, vec![], vec![from(1, enter(1, &spot))]);
        step(&data, &mut run, 1, vec![], vec![from(1, air(1, 201))]);
        until(&data, &mut run, "wait_to_enter_spot2", 30.);
        step(&data, &mut run, 1, vec![enter(0, &spot)], vec![]);
        assert_eq!(run.session.state(), "follower_attempt");
        step(&data, &mut run, 30, vec![off(0)], vec![]);
        assert!(shown(&run, "MessageTrickNotCopied"), "the follower's: not copied");
        step(&data, &mut run, 130, vec![], vec![]);
        assert_eq!(run.standings[0].letters, 1, "Alice's letter");
    }

    /// Skate 3's rule (user 10-28): an attempt's clock (the HUD countdown, the row's UserAttemptTimeOut) waits out S3's
    /// 5 s pre-attempt (skate.lua 412-447) after the disc's own 1 s settle, or starts as the attempter reaches the spot
    /// (Skate 1's rows have no start volume), whose entry then counts; the follower's the same from its turn's start.
    /// Old code: the clock ran from the settle's end (the follower's from its turn's start).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn skate_the_attempt_clock_starts_after_the_pre_attempt_or_at_the_spot() {
        let data = data();
        let row = &data.rows["skate_financial_matrix"];
        let spot = row.fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let limit = row.fields["UserAttemptTimeOut"].as_f64().unwrap() as f32;
        let full = |run: &Run, less: f32| run.session.hud_time().is_some_and(|t| (limit - less - 0.1..=limit).contains(&t));
        let mut run = skate_run(&data, 8);
        until(&data, &mut run, "wait_to_enter_spot_volume", 60.);
        step(&data, &mut run, 90, vec![], vec![]);
        assert_eq!(run.session.hud_time(), None, "1.5 s in: the pre-attempt, no clock");
        step(&data, &mut run, 276, vec![], vec![]);
        assert!(full(&run, 0.1), "6.1 s in (1 s settle, 5 s): {:?}", run.session.hud_time());
        // Reaching the spot 2 s in starts it then.
        let mut run = skate_run(&data, 8);
        until(&data, &mut run, "wait_to_enter_spot_volume", 60.);
        step(&data, &mut run, 120, vec![], vec![]);
        step(&data, &mut run, 1, vec![enter(0, &spot)], vec![]);
        assert_eq!(run.session.state(), "leader_attempt", "the entry counts");
        assert!(full(&run, 0.), "{:?}", run.session.hud_time());
        // Bob copies: no clock 1 s into his turn, the full one as he reaches the spot.
        step(&data, &mut run, 30, vec![air(0, 101)], vec![]);
        until(&data, &mut run, "wait_to_enter_spot2", 30.);
        step(&data, &mut run, 60, vec![], vec![]);
        assert_eq!(run.session.hud_time(), None, "the follower's pre-attempt");
        step(&data, &mut run, 1, vec![], vec![from(1, enter(1, &spot))]);
        assert_eq!(run.session.state(), "follower_attempt");
        assert!(full(&run, 0.), "{:?}", run.session.hud_time());
    }

    /// Skate 3's rule (user 10-28): a bail fails the attempt and the turn moves on 2 s later (S3 skate.lua 452, 709;
    /// 987), the leader's and the follower's. Old code: 3.5 s.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn skate_a_bail_moves_the_turn_on_two_seconds_later() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let wipeout = |id: usize| ("wipeout".to_owned(), json!({"PlayerID": id}));
        let mut run = skate_run(&data, 8);
        until(&data, &mut run, "wait_to_enter_spot_volume", 60.);
        step(&data, &mut run, 90, vec![], vec![]);
        step(&data, &mut run, 1, vec![enter(0, &spot)], vec![]);
        step(&data, &mut run, 114, vec![wipeout(0)], vec![]);
        assert_eq!((run.session.state(), var(&run, "LeaderID")), ("leader_attempt", 0), "1.9 s after the bail");
        step(&data, &mut run, 12, vec![], vec![]);
        assert_eq!(var(&run, "LeaderID"), 1, "2.1 s after: leader_failure, Bob leads");
        // Bob sets a trick; Alice copies from the spot and bails.
        step(&data, &mut run, 90, vec![], vec![]);
        step(&data, &mut run, 1, vec![], vec![from(1, enter(1, &spot))]);
        step(&data, &mut run, 1, vec![], vec![from(1, air(1, 201))]);
        until(&data, &mut run, "wait_to_enter_spot2", 30.);
        step(&data, &mut run, 1, vec![enter(0, &spot)], vec![]);
        step(&data, &mut run, 114, vec![wipeout(0)], vec![]);
        assert_eq!((run.session.state(), run.standings[0].letters), ("follower_attempt", 0), "1.9 s after the bail");
        step(&data, &mut run, 12, vec![], vec![]);
        assert_eq!(run.standings[0].letters, 1, "2.1 s after: follower_failure, her letter");
    }

    /// Skate 3's rule (user 10-28): a line under way at the buzzer still counts if it lands in the 2 s after it (S3
    /// spot_battle.lua 902-911), but not on the match's last run, which ends at its buzzer; the user kept the 10 s turn,
    /// so a line started after the buzzer doesn't (review 10-28 M2). Alice (first: token 8 % 2) lands 900 1 s after her
    /// round-1 buzzer (started 0.5 s before it), then 1500 started after it, and 100 in time later; Bob lands 500 in
    /// time, and 2000 1 s after the last run's buzzer (his round-3 turn). Old code: every turn ended at its buzzer
    /// (Alice's best 100); the plain 2 s window: Alice's best 1500.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn spot_battle_a_line_just_after_the_buzzer_counts_but_not_on_the_last_run() {
        let data = data();
        let row = &data.rows["ol_fp_u1_ownspot01"];
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 8).unwrap();
        run.start(&data).unwrap();
        let spots: Vec<String> = row.fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let limit = (row.fields["TimeLimit"].as_f64().unwrap() * 60.) as usize;
        let rounds = row.fields["NumRounds"].as_i64().unwrap();
        // (frame, player, event, ticks the line ran before it landed)
        let (mut turn, mut plan, mut results_from) = (None, Vec::<(usize, usize, (String, Json), i64)>::new(), None);
        let done = play(&data, &mut run, 60. * 4., |run, f| {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            if run.session.state() == "entered_trick_boundary" {
                let active = run.session.var("ActiveSkaterIdx").as_i64().unwrap() as usize - 1;
                let round = run.session.var("CurrentRound").as_i64().unwrap();
                if turn != Some((round, active)) {
                    turn = Some((round, active));
                    plan.extend(spots.iter().map(|s| (f + 30, active, enter(active, s), 0)));
                    match (round, active) {
                        (1, 0) => {
                            plan.push((f + limit + 60, 0, sequence(0, 900, 0), 90));
                            plan.push((f + limit + 90, 0, sequence(0, 1500, 0), 20));
                        }
                        (r, 1) if r == rounds => plan.push((f + limit + 60, 1, sequence(1, 2000, 0), 90)),
                        (_, 0) => plan.push((f + 90, 0, sequence(0, 100, 0), 20)),
                        _ => plan.push((f + 90, 1, sequence(1, 500, 0), 20)),
                    }
                }
            }
            let tick = run.session.tick;
            for (_, id, (event, mut message), ran) in plan.iter().filter(|(at, ..)| *at == f).cloned().collect::<Vec<_>>() {
                if event == "sequencecomplete" {
                    (message["StartTick"], message["StartSimGameTick"]) = (json!(tick - ran), json!(tick - ran));
                }
                match id {
                    0 => local.push((event, message)),
                    _ => inbox.push((id, encode(&event, &message, tick))),
                }
            }
            if run.session.state() == "challenge_teardown" && results_from.is_none() {
                results_from = Some(f);
            }
            if results_from.is_some_and(|r| f == r + 600) {
                local.push(("postchallengedisplayexit".into(), json!({})));
            }
            (local, inbox)
        });
        assert!(done.is_some(), "stuck in {}", run.session.state());
        let scores: Vec<i64> = run.standings.iter().map(|s| s.score).collect();
        assert_eq!(scores, [900, 500], "{:?}", run.standings);
    }

    /// Best Trick on the Library, `names` with Alice this console: each round 1 s in everyone in `scores` rides into the
    /// round's spot, and 1 s later lands each of their (score, after a marker return) lines a quarter second apart; the
    /// final results' exit. The completed run.
    fn best_trick_with(data: &Data, names: Vec<String>, scores: &[(usize, Vec<(i64, bool)>)]) -> Run {
        let row = &data.rows["besttrick_financial_library"];
        let mut run = Run::new(data, row, names, 0, 7).unwrap();
        run.start(data).unwrap();
        let spots: Vec<String> = row.fields["BestTrickSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let (mut began, mut results_from) = (BTreeMap::<i64, usize>::new(), None);
        let done = play(data, &mut run, 60. * 12., |run, f| {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            let tick = run.session.tick;
            let mut send = |id: usize, event: (String, Json)| match id {
                0 => local.push(event),
                _ => inbox.push((id, encode(&event.0, &event.1, tick))),
            };
            if run.session.state() == "event_round" {
                let round = run.session.var("CurrentRound").as_i64().unwrap();
                let k = *began.entry(round).or_insert(f);
                for (id, lines) in scores {
                    let id = *id;
                    if f == k + 60 {
                        send(id, enter(id, &spots[round as usize - 1]));
                    }
                    for (n, &(score, marked)) in lines.iter().enumerate() {
                        if f == k + 120 + 15 * n {
                            let mut line = sequence(id, score, tick);
                            if marked {
                                line.1[MARKER_USED] = json!(true);
                            }
                            send(id, line);
                        }
                    }
                }
            }
            if run.session.state() == "results" && run.hud.iter().any(|c| c.name == "HUD.SetWinState") && results_from.is_none() {
                results_from = Some(f);
            }
            if results_from.is_some_and(|r| f == r + 600) {
                send(0, ("postchallengedisplayexit".into(), json!({})));
            }
            (local, inbox)
        });
        assert!(done.is_some(), "stuck in {}", run.session.state());
        run
    }

    /// Skate 3's rule (user 10-28): every competitor still here earns its place's points, a 0 round score too (S3
    /// contest_shared 1561-1573). Three in the Library Best Trick, only Ann scoring (1000 a round): Bo and Cy tie second
    /// on 0, 1 point each a round, and their standings' scores stay 0. Old code: a 0 earned none ([6, 0, 0]).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn best_trick_a_zero_round_still_earns_its_places_points() {
        let data = data();
        let names: Vec<String> = SIX[..3].iter().map(|s| s.to_string()).collect();
        let run = best_trick_with(&data, names, &[(0, vec![(1000, false)])]);
        let got: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.score, s.points)).collect();
        assert_eq!(got, [(3000, 6), (0, 3), (0, 3)], "{:?}", run.standings);
    }

    /// Skate 3's rule (user 10-28): a sequence after a marker return scores nothing (S3 contest_shared 1091). Library
    /// Best Trick, three: Ann's (this console's) 3000 after a return doesn't count, her 1000 does; Bo's 1200 does, his
    /// relayed 9000 after a return doesn't; Cy lands 1500. Each round Cy 2 points, Bo 1, Ann 0. Either flag ignored
    /// changes the order (Ann first on 3000, or Bo on 9000). Old code: Ann's 3000 counted ([6, 0, 3]; `fits` also
    /// dropped Bo's flagged event whole).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn contest_sequences_after_a_marker_return_score_nothing() {
        let data = data();
        let names: Vec<String> = SIX[..3].iter().map(|s| s.to_string()).collect();
        let lines = [(0, vec![(3000, true), (1000, false)]), (1, vec![(1200, false), (9000, true)]), (2, vec![(1500, false)])];
        let run = best_trick_with(&data, names, &lines);
        let points: Vec<i64> = run.standings.iter().map(|s| s.points).collect();
        assert_eq!(points, [0, 3, 6], "{:?}", run.standings);
    }

    #[test]
    fn the_watched_skater_gets_this_players_framing() {
        let from = Transform::from_xyz(5., 0., 5.);
        let camera = Transform::from_xyz(5., 1.5, 2.).looking_at(Vec3::new(5., 1., 5.), Vec3::Y);
        let to = Transform::from_xyz(-10., 2., 0.).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let got = crate::match_core::reframe(from, camera, to);
        // 3 m behind along -z turns to 3 m along -x for a skater turned 90 degrees about y.
        assert!(got.translation.distance(Vec3::new(-13., 3.5, 0.)) < 1e-4, "{got:?}");
        assert!((got.forward().as_vec3() - to.rotation * camera.forward().as_vec3()).length() < 1e-4);
    }

    // ------------------------------------------------------------------------------------------- Party Play
    /// One pad through a Party Play game to `results` (each frame `pad(run, frames in its state)` gives the pad's
    /// events, attributed to the pad's player): every card is answered after `hold` frames, and the results screen's
    /// exit comes 10 s in. Returns the pad's run, the party, the cards and the pad's player per card.
    fn party_game(
        row: &str,
        names: usize,
        seed: u64,
        results: &str,
        hold: usize,
        mut pad: impl FnMut(&Run, &Party, usize) -> Vec<(&'static str, Json)>,
    ) -> (Run, Party, Vec<(usize, i64)>, Vec<usize>) {
        let data = data();
        let names = (1..=names).map(|n| format!("Player {n}")).collect();
        let (mut run, mut party) = Party::new(&data, &data.rows[row], names, seed).unwrap();
        // The pad's state (with its player) and the frames it has run since, the card's frames not counted.
        let (mut cards, mut pads, mut entered, mut card_at) = (Vec::new(), Vec::new(), (String::new(), 0), None);
        for f in 0..60 * 60 * 6 {
            if run.complete() {
                return (run, party, cards, pads);
            }
            let key = format!("{}/{}", run.session.state(), run.me());
            if entered.0 != key {
                entered = (key, 0);
            }
            let mut local: Vec<(String, Json)> = Vec::new();
            if !party.waiting {
                let k = entered.1;
                entered.1 += 1;
                local = pad(&run, &party, k).into_iter().map(|(e, m)| (e.to_owned(), m)).collect();
                if run.session.state() == results && k == 600 {
                    local.push(("postchallengedisplayexit".into(), json!({})));
                }
            }
            let clock = run.session.clock;
            party.frame(&mut run, &data, 1. / 60., local, skate_core::physics::skeleton_animation_record::IDENTITY).unwrap();
            if let Some(card) = party.card.take() {
                cards.push(card);
                pads.push(run.me());
                card_at = Some(f);
            }
            if party.waiting && card_at.is_some_and(|at| at < f) {
                assert_eq!(run.session.clock, clock, "no time passes while the card is up");
            }
            if card_at.is_some_and(|at| f >= at + hold) {
                party.answer(&mut run).unwrap();
                card_at = None;
            }
        }
        panic!("{row}: stuck in {} with the pad on player {}", run.session.state(), run.me());
    }
    fn me_json(run: &Run) -> Json {
        json!(run.me())
    }

    /// Party Play's Own The Spot on fp_u1_ownspot01, two players on one pad (Skate 3's rules): the pad goes to each
    /// turn's player behind its card, three rounds of Player 2 (7 % 2) then Player 1; the waiting player stands on its
    /// own console's spot (the observer locator's slot); every console scores alike; the results rank Player 2 first.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn party_spot_battle_passes_the_pad_to_each_turns_player() {
        let data = data();
        let row = &data.rows["ol_fp_u1_ownspot01"];
        let spots: Vec<String> = row.fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let locator = |k: &str| match &row.places[row.fields[k].as_str().unwrap()] {
            Place::Locator(l) => l.clone(),
            _ => panic!("{k} is not a locator"),
        };
        let (attempt, watch) = (locator("SkateAttemptSpawnPoint"), locator("Location"));
        let mut waited = Vec::new();
        let (run, party, cards, pads) = party_game("ol_fp_u1_ownspot01", 2, 7, "challenge_teardown", 120, |run, party, k| {
            if run.session.state() != "entered_trick_boundary" {
                return Vec::new();
            }
            let active = run.session.var("ActiveSkaterIdx").as_i64().unwrap() as usize - 1;
            assert_eq!(active, run.me(), "the pad is the turn's player");
            if k == 1 {
                waited.push((run.me(), run.teleport, party.at[1 - run.me()]));
            }
            let tick = run.session.tick;
            match k {
                30 => spots.iter().map(|s| ("triggerentercollision", json!({"PlayerID": me_json(run), "VolumeID": s}))).collect(),
                90 => {
                    let score = if run.me() == 0 { 1000 } else { 1500 };
                    vec![("sequencecomplete", json!({"PlayerID": me_json(run), "SequenceScore": score, "StartTick": tick - 20, "StartSimGameTick": tick - 20, "Wipedout": false}))]
                }
                _ => Vec::new(),
            }
        });
        assert_eq!(cards, [(2, 1), (1, 1), (2, 2), (1, 2), (2, 3), (1, 3)], "a card per turn: (player, round)");
        assert_eq!(pads, [1, 0, 1, 0, 1, 0], "the pad on the card's player");
        for (me, spawn, other) in waited {
            assert_eq!(spawn, Some(attempt.slot(0)), "player {me} on the attempt spot");
            assert_eq!(other, Some(watch.slot(1 - me)), "the other waits on its own slot at the observer locator");
        }
        let scores = |r: &Run| r.standings.iter().map(|s| s.score).collect::<Vec<_>>();
        assert_eq!(scores(&run), [1000, 1500], "{:?}", run.standings);
        assert!(party.consoles.iter().flatten().all(|r| scores(r) == scores(&run)), "every console scores alike");
        let results = party_results(&run, &data);
        let block = results.iter().find(|(k, _)| k == "HUDComponents.Results_GetLeaderboardDataBlock").unwrap().1.clone();
        assert_eq!((block[0].clone(), block[1].clone(), block[7].clone()), (json!(1), json!("#1500"), json!("#Player 2")));
        assert_eq!((block[8].clone(), block[9].clone(), block[15].clone()), (json!(2), json!("#1000"), json!("#Player 1")));
        assert!(results.contains(&("Mission.GetResultsTitle".to_owned(), json!(row.name))));
    }

    /// Party Play's S.K.A.T.E. on the Matrix, two players on one pad: Player 1 (8 % 2) leads every round and sets a
    /// flip, Player 2 copies a wrong one; the pad goes leader, follower each round behind their cards; Player 2 spells
    /// S.K.A.T.E. in five rounds; the results rank Player 1 first with no letters.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn party_skate_passes_the_pad_between_leader_and_follower() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let mut round = 0;
        let (run, party, cards, pads) = party_game("skate_financial_matrix", 2, 8, "show_results", 60, |run, _, k| {
            let state = run.session.state();
            round += usize::from(state == "wait_to_enter_spot_volume" && k == 0);
            let air = |trick: i64| ("aircompleted", json!({"PlayerID": me_json(run), "FirstScorableId": trick, "Wipedout": 0, "S": {"first": trick, "first_type": 2, "spins": 0}}));
            let enter = || ("triggerentercollision", json!({"PlayerID": me_json(run), "VolumeID": spot}));
            match state {
                "wait_to_enter_spot_volume" | "wait_to_enter_spot2" if k == 90 => vec![enter()],
                "leader_attempt" if k == 30 => vec![air(100 + round as i64)],
                "follower_attempt" if k == 30 => vec![air(999)],
                _ => Vec::new(),
            }
        });
        let want: Vec<(usize, i64)> = (1..=5).flat_map(|r| [(1, r), (2, r)]).collect();
        assert_eq!(cards, want, "the leader's card, then the follower's, each round");
        assert_eq!(pads, [0usize, 1].repeat(5));
        let letters: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.letters, s.rank)).collect();
        assert_eq!(letters, [(0, 1), (5, 2)], "{:?}", run.standings);
        assert!(party.consoles.iter().flatten().all(|r| r.standings == run.standings), "every console agrees");
        let block = party_results(&run, &data).into_iter().find(|(k, _)| k == "HUDComponents.Results_GetLeaderboardDataBlock").unwrap().1;
        assert_eq!((block[7].clone(), block[9].clone(), block[15].clone()), (json!("#Player 1"), json!("#5"), json!("#Player 2")));
    }

    /// The probe's own trick state for a landed air whose first scorable is `first` (a flip), as every event carries it.
    fn probe_state(first: usize) -> Json {
        let n = first + 1;
        let p = Probe { first: Some(first), kinds: vec![2; n], labels: vec!["ID_TRICK_KICKFLIP".into(); n], idents: vec!["kickflip"; n], ..Default::default() };
        p.trick_state(false, false, serde_json::Map::new())
    }
    fn in_step(run: &Run, party: &Party) {
        for other in party.consoles.iter().flatten() {
            let at = |r: &Run| (r.session.state().to_owned(), r.session.var("ActiveSkaterIdx"), r.session.var("LeaderID"));
            assert_eq!(at(other), at(run), "player {}'s console and the pad's", other.me() + 1);
        }
    }

    /// Bug 11-01: Player 1 bailed in a Party Play Spot Battle and the pad was stuck between both players' turns (the
    /// card back every frame, the clock flipping between the turn's 6 s and the 3-2-1): the pad's events, with the
    /// probe's trick state on them, never reached Player 2's console. With that state on every event, the bail ends the
    /// turn on every console: the consoles in step throughout, one card per turn.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn a_party_bail_ends_the_turn_on_every_console() {
        let data = data();
        let row = &data.rows["ol_fp_u1_ownspot01"];
        let spots: Vec<String> = row.fields["OTSSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let s = |mut m: Json| {
            m["S"] = probe_state(0);
            m
        };
        let (run, _, cards, pads) = party_game("ol_fp_u1_ownspot01", 2, 7, "challenge_teardown", 120, |run, party, k| {
            in_step(run, party);
            if run.session.state() != "entered_trick_boundary" {
                return Vec::new();
            }
            let tick = run.session.tick;
            match (k, run.me()) {
                (30, _) => spots.iter().map(|v| ("triggerentercollision", s(json!({"PlayerID": me_json(run), "VolumeID": v})))).collect(),
                (60, 0) => vec![("wipeout", s(json!({"PlayerID": me_json(run)})))],
                (90, 1) => vec![(
                    "sequencecomplete",
                    s(json!({"PlayerID": me_json(run), "SequenceScore": 1500, "StartTick": tick - 20, "StartSimGameTick": tick - 20, "Wipedout": false})),
                )],
                _ => Vec::new(),
            }
        });
        assert_eq!(cards, [(2, 1), (1, 1), (2, 2), (1, 2), (2, 3), (1, 3)], "a card per turn: (player, round)");
        assert_eq!(pads, [1, 0, 1, 0, 1, 0]);
        let scores: Vec<i64> = run.standings.iter().map(|s| s.score).collect();
        assert_eq!(scores, [0, 1500], "Player 1 bailed every turn; Player 2's lines counted");
    }

    /// Bug 11-01: the same split in a Party Play S.K.A.T.E. (the round counter climbing as the pad passed back and
    /// forth): with the probe's trick state on every event, every console stays in step through five rounds.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn a_party_skate_game_keeps_every_console_in_step() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let mut round = 0;
        let (run, _, cards, _) = party_game("skate_financial_matrix", 2, 8, "show_results", 60, |run, party, k| {
            in_step(run, party);
            let state = run.session.state();
            round += usize::from(state == "wait_to_enter_spot_volume" && k == 0);
            let air = |trick: usize| {
                ("aircompleted", json!({"PlayerID": me_json(run), "FirstScorableId": trick, "Wipedout": 0, "S": probe_state(trick)}))
            };
            match state {
                "wait_to_enter_spot_volume" | "wait_to_enter_spot2" if k == 90 => {
                    vec![("triggerentercollision", json!({"PlayerID": me_json(run), "VolumeID": spot, "S": probe_state(0)}))]
                }
                "leader_attempt" if k == 30 => vec![air(100 + round)],
                "follower_attempt" if k == 30 => vec![air(999)],
                _ => Vec::new(),
            }
        });
        let want: Vec<(usize, i64)> = (1..=5).flat_map(|r| [(1, r), (2, r)]).collect();
        assert_eq!(cards, want, "the rounds count once each");
        let letters: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.letters, s.rank)).collect();
        assert_eq!(letters, [(0, 1), (5, 2)], "{:?}", run.standings);
    }

    /// Bug 10-31 (Party Play): after a S.K.A.T.E. card's A the player could not move for a second (the leader: the
    /// disc's 1 s settle; the follower: its 2.5 s wait). Skate 3 unpins the turn's player on the card's A: free within
    /// two frames of it, every turn.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn a_skate_turn_is_free_on_its_cards_a() {
        let data = data();
        let spot = data.rows["skate_financial_matrix"].fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let (mut pinned, mut round): (BTreeMap<String, usize>, i64) = (BTreeMap::new(), 0);
        party_game("skate_financial_matrix", 2, 8, "show_results", 60, |run, _, k| {
            let state = run.session.state();
            if matches!(state, "wait_to_enter_spot_volume" | "follower_wait" | "wait_to_enter_spot2") && run.pin {
                *pinned.entry(format!("{state}/{}", run.me())).or_default() = k + 1;
            }
            round += i64::from(state == "wait_to_enter_spot_volume" && k == 0);
            let air = |trick: i64| ("aircompleted", json!({"PlayerID": me_json(run), "FirstScorableId": trick, "Wipedout": 0, "S": {"first": trick, "first_type": 2, "spins": 0}}));
            match state {
                "wait_to_enter_spot_volume" | "wait_to_enter_spot2" if k == 90 => {
                    vec![("triggerentercollision", json!({"PlayerID": me_json(run), "VolumeID": spot}))]
                }
                // A new trick each round (a repeat is "already been done").
                "leader_attempt" if k == 30 => vec![air(100 + round)],
                "follower_attempt" if k == 30 => vec![air(999)],
                _ => Vec::new(),
            }
        });
        assert!(!pinned.is_empty(), "never pinned at a card");
        assert!(pinned.values().all(|&frames| frames <= 2), "pinned frames after the card's A: {pinned:?}");
    }

    // ------------------------------------------------------------------------------------------- tricks
    /// Bug 10-31: every Skate 1 S.K.A.T.E. trick read "trick not set". The probe read the scoring's announced tricks as
    /// a running list, but the engine announces each on its own frame only: a flip announced the tick after the ollie,
    /// or as the board lands, was lost, and aircompleted went at touchdown without its trick. An air now reads as Skate
    /// 3's S.K.A.T.E. reads it: every trick through it and as it settles, its FirstScorable the flip a launch carries.
    #[test]
    fn an_air_reads_as_skate3s_skate_reads_it() {
        let dt = 1. / 60.;
        let mut p = Probe::default();
        assert_eq!(p.air_step(&["ollie"], true, 0, dt, false), None, "takeoff");
        assert_eq!(p.air_step(&["kickflip"], true, 1, dt, false), None, "the very next tick");
        assert_eq!(p.air_step(&[], true, 1, dt, false), None);
        assert_eq!(p.air_step(&[], false, 1, dt, false), None, "touchdown: it settles first");
        assert!(p.landing.is_some(), "settling still counts as in the air (IsInAirWithValidLaunchTrick)");
        assert_eq!(p.air_step(&["indy"], false, 1, dt, false), None, "announced as it lands");
        let landed = (0..60).find_map(|_| p.air_step(&[], false, 1, dt, false));
        assert_eq!(landed, Some((vec!["ollie", "kickflip", "indy"], 1)), "nothing lost");
        let kind = |id: &str| match id {
            "ollie" => Some(1),
            "kickflip" => Some(2),
            "indy" => Some(4),
            _ => None,
        };
        assert_eq!(crate::skate3_skate_game::first_scorable(&landed.unwrap().0, kind), Some(("kickflip", 2)), "the flip, not the ollie");
        // A bail (or stepping off) loses the air: no aircompleted.
        p.air_step(&["ollie"], true, 0, dt, false);
        p.air_step(&[], false, 0, dt, false);
        assert_eq!(p.air_step(&[], false, 0, dt, true), None);
        assert!((0..60).all(|_| p.air_step(&[], false, 0, dt, false).is_none()));
    }

    /// Bug 10-31 (Party Play): the skater turned at the Press-A card. A held teleport (RepositionAI with PinAI, or under
    /// the card) waits until the pad is held, so the board lands held and physics::pin keeps it still.
    #[test]
    fn a_held_teleport_waits_for_the_pads_hold() {
        assert!(teleport_now(false, false), "an unheld teleport goes at once");
        assert!(!teleport_now(true, false), "held, but not at the pad yet");
        assert!(teleport_now(true, true));
    }

    /// Bug 10-31 (Party Play): The Walrus put the skater side-on under its landing, "in a wall". Its locators (BAM.loc,
    /// no spawns) all carry one default -X heading: a turn's player faces the row's spot instead, the waiting ones 2 m
    /// abreast. A locator with spawns is placed as before.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn a_locator_without_spawns_faces_its_spot() {
        let data = data();
        let row = &data.rows["ol_spotbattle_oldtown_warehouststeps"];
        let locator = |k: &str| match &row.places[row.fields[k].as_str().unwrap()] {
            Place::Locator(l) => l.clone(),
            _ => panic!("{k} is not a locator"),
        };
        let (attempt, watch) = (locator("SkateAttemptSpawnPoint"), locator("Location"));
        assert!(attempt.slots.is_empty() && watch.slots.is_empty());
        let spot = row.spot().unwrap();
        for (l, slot) in [(&attempt, 0), (&watch, 0), (&watch, 1), (&watch, 2)] {
            let m = l.spawn(slot, row);
            let to = [spot[0] - m[3][0], spot[2] - m[3][2]];
            let cos = (m[2][0] * to[0] + m[2][2] * to[1]) / to[0].hypot(to[1]);
            assert!(cos > 0.99, "slot {slot} faces the spot: {:?} toward {spot:?}", m[2]);
            assert_eq!(m[3][1], l.matrix[3][1] + BAM_LIFT, "lifted over the city map's landing");
        }
        let side = |slot| {
            let (a, b) = (watch.spawn(0, row)[3], watch.spawn(slot, row)[3]);
            (b[0] - a[0]).hypot(b[2] - a[2])
        };
        assert_eq!([side(1), side(2), side(3)].map(|d| (d * 10.).round() / 10.), [2., 2., 4.], "abreast");
        let spawns = &data.rows["ol_fp_u1_ownspot01"];
        let Place::Locator(l) = &spawns.places[spawns.fields["SkateAttemptSpawnPoint"].as_str().unwrap()] else { panic!() };
        assert_eq!(l.spawn(0, spawns), l.slot(0), "a locator with spawns: its own");
    }

    /// Bug 10-31: the run waits for the start's trip (Party Play's map load) to be over.
    #[test]
    fn the_run_starts_once_the_trip_is_over() {
        assert!(trip_ready(true, false, false, false, false));
        assert!(!trip_ready(false, false, false, false, false), "still on the last map");
        assert!(!trip_ready(true, true, false, false, false), "loading");
        assert!(!trip_ready(true, false, true, false, false), "the trip's spot still to come");
        assert!(!trip_ready(true, false, false, true, false), "the match's start teleport still to come");
        assert!(!trip_ready(true, false, false, false, true), "the loading screen still up");
    }

    /// Bug 10-31: a set trick read "ID_TRICK_DARKCATCH". The names are the engine's (the Skate 3 HUD's strings), and an
    /// id without one reads as words.
    #[test]
    fn trick_names_are_the_engines_and_never_a_raw_id() {
        let names = HashMap::from([("ID_TRICK_FLIP_KICKFLIP".to_owned(), "Kickflip".to_owned())]);
        assert_eq!(trick_text(&names, "ID_TRICK_FLIP_KICKFLIP"), "Kickflip");
        assert_eq!(trick_text(&names, "ID_TRICK_DARKCATCH"), "DARKCATCH");
    }

    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn the_installed_trick_names_come_from_skate3s_hud() {
        let data = data();
        assert!(data.names.len() > 100, "{} names", data.names.len());
        assert_eq!(data.trick_name("ID_TRICK_FLIP_KICKFLIP"), "Kickflip");
    }
}
