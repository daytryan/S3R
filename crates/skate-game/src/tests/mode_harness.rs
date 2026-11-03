//! Headless online / Party Play mode harness (online audit 2026-10-06). A child of physics.rs, like map_startup.rs.
//!
//! It loads a real map from the install (read-only: `SKATE3_ASSET_ROOT`, maps from `<assets>/../maps`, or
//! `SKATE_MAP_TEST_PATH`), spawns the stock skater the way the game does (the cross-map travel's map load, or the
//! same-map teleport), steps physics with scripted pads and records what the modes read: spawn transforms against the
//! ground under them, bail vs dismount signals, and where an idle (waiting) skater ends up. Mode state machines are
//! stepped against these samples by the tests below (and by the Skate 2 / Skate 1 audit's own tests).
//!
//! Every test here is `#[ignore]`: run with
//! `SKATE3_ASSET_ROOT=<install>/data/installations/<id>/assets cargo test -p skate-game -j 1 --no-default-features
//! --bin skate3rust mode_harness -- --ignored --nocapture`.
use super::*;
use crate::{camera::CameraRuntime, graph_runtime::StockGraphs, input::ControllerInput};
use skate_core::{input::xbox::XboxState, physics::filtered_state::FilteredCategory, player::state::PhysicalStateId};
use skate_data::skate_map::SkateMap;
use std::path::PathBuf;

pub(crate) type Matrix = [[f32; 4]; 4];

pub(crate) fn asset_root() -> PathBuf {
    std::env::var_os("SKATE3_ASSET_ROOT").map(PathBuf::from).expect("set SKATE3_ASSET_ROOT to the install's assets")
}

/// `<assets>/../maps/<name>.skate` (the install layout), or SKATE_MAP_TEST_PATH.
pub(crate) fn map_path(name: &str) -> PathBuf {
    std::env::var_os("SKATE_MAP_TEST_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root().parent().expect("assets has a parent").join("maps").join(format!("{name}.skate")))
}

/// A private JSON file of the install (`<assets>/private/<path>`).
pub(crate) fn private_json(path: &str) -> serde_json::Value {
    let file = asset_root().join("private").join(path);
    let bytes = std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    serde_json::from_slice(&bytes).unwrap()
}

/// A JSON matrix ([[f32; 4]; 4], translation in row 3).
pub(crate) fn matrix(v: &serde_json::Value) -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| v[r][c].as_f64().unwrap() as f32))
}

/// How the skater gets onto the map.
#[derive(Clone, Copy)]
pub(crate) enum Spawn {
    /// The package spawn (`supported_spawn`): a fresh launch.
    Package,
    /// A Challenge Map / Party Play trip to another map: `map_transition` builds the new world at the destination
    /// (`GamePhysics::load_at`).
    MapLoad(Matrix),
}

/// One physics tick of the skater, as the modes read it.
#[derive(Clone, Debug)]
pub(crate) struct Sample {
    pub tick: u64,
    /// `animated_skeleton.roots.animation_to_world` (the presented player root; what online sends as its root).
    pub root: [f32; 3],
    pub deck: [f32; 3],
    /// The lowest wheel's bottom (its centre's height minus the wheel radius).
    pub wheel_bottom: f32,
    /// The lowest skeleton body's centre (a skater fallen through the map has every body under the face).
    pub lowest_body: f32,
    /// The face under the deck (None: nothing under it within 10 m).
    pub ground: Option<f32>,
    /// `GamePhysics::board_wiping_out`.
    pub board_wiping_out: bool,
    /// `filtered_state_0`: the physics category the game publishes (4 = Wipeout).
    pub category: u32,
    /// `state.category_12` (500 = off the board).
    pub state_category: u32,
    pub state: PhysicalStateId,
    pub in_air: bool,
    pub speed: f32,
    /// The wipeout's reset countdown is running (wipeoutalmostover).
    pub teleport_pending: bool,
    /// The deck's forward (x, z): the heading a spawn gave.
    pub forward: [f32; 2],
}
impl Sample {
    pub fn bailing(&self) -> bool {
        self.category == FilteredCategory::Wipeout as u32
    }
    /// How far the wheels sit above the face under the deck (negative: in the ground).
    pub fn clearance(&self) -> Option<f32> {
        self.ground.map(|g| self.wheel_bottom - g)
    }
}

/// Scripted pads.
pub(crate) mod pads {
    use super::XboxState;
    pub const A: u16 = 0x1000;
    pub const Y: u16 = 0x8000;
    pub fn idle() -> XboxState {
        XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] }
    }
    pub fn buttons(buttons: u16) -> XboxState {
        XboxState { buttons, ..idle() }
    }
    /// A held for 30 of every 40 ticks (a push cadence), counted from `t`.
    pub fn push(t: u64) -> XboxState {
        buttons(if t % 40 < 30 { A } else { 0 })
    }
    /// The manual bail: both sticks clicked and both triggers (board_bail_assembly's request).
    pub fn bail() -> XboxState {
        XboxState { buttons: 0x00C0, triggers: [255; 2], ..idle() }
    }
}

pub(crate) struct Harness {
    pub physics: GamePhysics,
    pub skater: SkaterRuntime,
    pub controls: PlayerControls,
    pub input: ControllerInput,
    pub camera: CameraRuntime,
    pub graphs: StockGraphs,
    pub map: SkateMap,
    /// Neutral input while set (ControllerInput.match_hold / pinned).
    pub hold: bool,
    /// physics::advance's pin (user 10-27), run after each tick on `hold` when set.
    pub pin: Option<super::pin::Pin>,
}

impl Harness {
    pub fn load(map: &str, spawn: Spawn) -> Self {
        let root = asset_root();
        let path = map_path(map);
        let map = SkateMap::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        crate::skate_world::validate_runtime(&map).unwrap();
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = StockGraphs::load(&root, &assets).unwrap();
        let at = match spawn {
            Spawn::Package => None,
            Spawn::MapLoad(m) => Some(m),
        };
        // map_transition::start: physics at the destination, then the skater from its deck.
        let physics = GamePhysics::load_at(&root, Some(&map), crate::difficulty::Difficulty::Easy, at).unwrap();
        let skater = SkaterRuntime::load_for_world(&root, &graphs, &physics, crate::difficulty::Difficulty::Easy.key(), None)
            .unwrap();
        Self {
            physics,
            skater,
            controls: PlayerControls::load(&root).unwrap(),
            input: ControllerInput::default(),
            camera: CameraRuntime::load(&root).unwrap(),
            graphs,
            map,
            hold: false,
            pin: None,
        }
    }

    /// The same-map teleport every mode and the Challenge Map use (`SkaterRuntime::travel_to`).
    pub fn teleport(&mut self, at: Matrix) {
        self.skater.travel_to(at).unwrap();
    }

    pub fn tick(&mut self, pad: XboxState) -> Sample {
        self.input.sample_raw_for_test(if self.hold { pads::idle() } else { pad });
        let mut actions = self.input.player_actions();
        self.controls.update_for_physics(&mut actions, &self.physics, &self.skater, &self.camera).unwrap();
        // physics::advance's force brake while held (retail's pin).
        self.physics.animation_profile.force_braking = self.hold;
        frame::advance(&mut self.physics, &mut self.skater, &mut self.controls, &self.graphs, &mut actions, true, &mut self.camera)
            .unwrap_or_else(|e| panic!("harness tick {}: {e}", self.physics.ticks));
        self.physics.animation_profile.force_braking = false;
        if let Some(pin) = self.pin.as_mut() {
            pin.step(self.hold, self.skater.player_input.physical.filtered_state_0, &mut self.physics.board);
        }
        self.sample()
    }

    pub fn run(&mut self, ticks: u64, mut pad: impl FnMut(u64) -> XboxState) -> Vec<Sample> {
        (0..ticks).map(|t| self.tick(pad(t))).collect()
    }

    /// The face under a point: the first hit of a line from 1 m above to 10 m below it (as `supported_spawn`).
    pub fn ground(&self, p: [f32; 3]) -> Option<f32> {
        crate::skate_world::face_under(self.physics.world(), p)
    }

    pub fn sample(&self) -> Sample {
        let parts = self.physics.board.part_transforms();
        let r = self.physics.settings.wheel_radius;
        let wheel_bottom = parts[..4].iter().map(|p| p.translation.y - r).fold(f32::INFINITY, f32::min);
        let deck = parts[BodyId::Deck.index()].translation;
        let lowest_body = self.skater.skeleton.bodies().iter().map(|b| b.rates.position.y).fold(f32::INFINITY, f32::min);
        let root = self.skater.animated_skeleton.roots.animation_to_world[3];
        let v = self.physics.board.bodies()[BodyId::Deck.index()].rates.linear_velocity;
        let p = &self.skater.player_input.physical;
        Sample {
            tick: self.physics.ticks,
            root: [root[0], root[1], root[2]],
            deck: [deck.x, deck.y, deck.z],
            wheel_bottom,
            lowest_body,
            ground: self.ground([deck.x, deck.y, deck.z]),
            board_wiping_out: self.physics.board_wiping_out,
            category: p.filtered_state_0,
            state_category: p.state.category_12,
            state: self.skater.player_state.current(),
            in_air: self.skater.scoring.in_air(),
            speed: v.x.hypot(v.z),
            teleport_pending: self.skater.wipeout_state.state.teleport_pending,
            forward: {
                let f = parts[BodyId::Deck.index()].basis.columns[2];
                [f[0], f[2]]
            },
        }
    }
}

/// skate3-online/<mode>/rules.json.
pub(crate) fn s3_rules(mode: &str) -> serde_json::Value {
    private_json(&format!("skate3-online/{mode}/rules.json"))
}

/// The Online Challenge Map row (online.json) with this id: its `start` {matrix, slots} and map.
pub(crate) fn online_row(id: &str) -> serde_json::Value {
    fn find(v: &serde_json::Value, id: &str) -> Option<serde_json::Value> {
        match v {
            serde_json::Value::Object(o) if o.get("id").and_then(|i| i.as_str()) == Some(id) && o.contains_key("start") => {
                Some(v.clone())
            }
            serde_json::Value::Object(o) => o.values().find_map(|v| find(v, id)),
            serde_json::Value::Array(a) => a.iter().find_map(|v| find(v, id)),
            _ => None,
        }
    }
    find(&private_json("challenge-map/online/online.json"), id).unwrap_or_else(|| panic!("online.json has no {id}"))
}

/// Survey (prints, asserts nothing): every University S.K.A.T.E. row's locator slots against the face under them, the
/// online start (online.json, the Challenge Map's and match_core's spawn), the attempt spawn and the waiting slots
/// (rules.json, the mode's RepositionAI places).
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn survey_university_skate_slots() {
    let h = Harness::load("University", Spawn::Package);
    let rules = s3_rules("skate");
    let height = |m: &Matrix| h.ground([m[3][0], m[3][1], m[3][2]]).map(|g| m[3][1] - g);
    for (id, row) in rules["rows"].as_object().unwrap().iter().filter(|(id, _)| id.contains("_univ_")) {
        let online = online_row(id);
        let start = online["start"]["slots"].as_array().unwrap().iter().map(|s| height(&matrix(s))).collect::<Vec<_>>();
        let spawn = row["spawn"]["slots"].as_array().unwrap().iter().map(|s| height(&matrix(s))).collect::<Vec<_>>();
        let wait = row["wait"]["slots"].as_array().unwrap().iter().map(|s| height(&matrix(s))).collect::<Vec<_>>();
        eprintln!("{id}: online start slots above face {start:?}\n  attempt spawn {spawn:?}\n  waiting {wait:?}");
    }
}

/// Inventory (prints, asserts nothing): every Skate 3 Online Challenge Map row (Team, Career, Versus tabs) with
/// whether a mode plugin offers it (match_core::Playable, as the installed data makes it), by tab and folder.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn inventory_s3_rows() {
    use crate::match_core::{Owned, Playable};
    let mut app = App::new();
    app.insert_resource(crate::config::Config {
        asset_root: asset_root(),
        verification_capture: None,
        hero_clip: None,
        skate2_physics_dir: None,
        physics_override: None,
        map: None,
        map_path: None,
        difficulty: Default::default(),
        check_assets: false,
        start_paused: false,
        multiplayer: Default::default(),
        map_fingerprint: 0,
        teleport: None,
    });
    app.init_resource::<Playable>().init_resource::<Owned>();
    app.add_plugins(crate::skate3_skate_game::Skate3SkateGamePlugin);
    app.add_plugins((
        crate::skate3_one_up::Skate3OneUpPlugin,
        crate::skate3_uniform::Skate3UniformPlugin,
        crate::skate3_race::Skate3RacePlugin,
        crate::skate3_contest::Skate3ContestPlugin,
        crate::skate3_domination::Skate3DominationPlugin,
        crate::skate3_ots::Skate3OtsPlugin,
        crate::skate3_film::Skate3FilmPlugin,
        crate::skate3_otl::Skate3OtlPlugin,
    ));
    let playable = app.world().resource::<Playable>();
    let online = private_json("challenge-map/online/online.json");
    fn rows<'a>(v: &'a serde_json::Value, folder: &str, out: &mut Vec<(String, &'a serde_json::Value)>) {
        for r in v.as_array().into_iter().flatten() {
            if let Some(kids) = r.get("rows").or(r.get("children")).filter(|k| k.is_array()) {
                rows(kids, r["name"].as_str().unwrap_or(""), out);
            } else if r.get("challenge").is_some() {
                out.push((folder.to_owned(), r));
            }
        }
    }
    for (tab, t) in online["tabs"].as_array().unwrap().iter().enumerate() {
        let name = t["name"].as_str().unwrap_or("");
        if name.starts_with('#') {
            continue; // the Skate 2 / Skate 1 tabs (their own audit)
        }
        let mut list = Vec::new();
        rows(&t["rows"], "", &mut list);
        let mut folders: std::collections::BTreeMap<&str, (usize, Vec<String>, Vec<String>)> = Default::default();
        for (folder, r) in &list {
            let id = r["challenge"]["id"].as_str().unwrap_or("?");
            let map = r["challenge"]["map"].as_str().unwrap_or("?");
            let f = folders.entry(folder.as_str()).or_default();
            f.0 += 1;
            let list = if playable.plays(tab, id) { &mut f.1 } else { &mut f.2 };
            list.push(format!("{id}@{map}"));
        }
        for (folder, (n, yes, no)) in folders {
            eprintln!("INVENTORY tab{tab} {name} | {folder} | {n} rows | offered {} | refused {}: {}", yes.len(), no.len(), no.join(" "));
            eprintln!("INVENTORY_OFFERED tab{tab} {folder}: {}", yes.join(" "));
        }
    }
}

/// How long a placement is watched: the review's fallers were in Wipeout by t37-t60 and tens of metres down by t180;
/// the slow one (hom_indu_03_ol slot 1) sank 1 m first and was in Wipeout by t180.
const STAND_TICKS: u64 = 240;
/// How far the wheels may go under the face below the deck (`Sample::clearance`) before the skater counts as falling
/// through it. Idle skaters roll: a slot on a slope or a hilltop start rolls down it in retail too (PinAI holds only
/// the input), so the fall is measured against the surface under the skater each tick, not the slot's face. On a slope
/// the downhill wheel sits under the face below the deck's centre (about 0.1 m on the steep starts).
const MAX_PENETRATION: f32 = 0.3;

/// Whether a placement onto `face` (the probe's) stood: the wheels out of the ground on the first tick; never more
/// than MAX_PENETRATION into the face under the deck and under `face` at once; at the end a face within 10 m under it (a
/// faller drops past every face; a skater rolling off a big drop lands), on the board and not bailing.
fn stood(face: f32, s: &[Sample]) -> Result<String, String> {
    let (first, last) = (&s[0], s.last().unwrap());
    let low = s.iter().map(|x| x.wheel_bottom).fold(f32::INFINITY, f32::min);
    let deepest = s.iter().filter_map(Sample::clearance).fold(f32::INFINITY, f32::min);
    let summary = format!(
        "first tick wheels {:+.3} above the probed face, lowest {:+.3}, deepest {:+.3} under the face below, end {:?} cat {}, {:.2} m away",
        first.wheel_bottom - face,
        low - face,
        deepest,
        last.state,
        last.category,
        ((last.deck[0] - first.deck[0]).powi(2) + (last.deck[2] - first.deck[2]).powi(2)).sqrt()
    );
    // Through a face only if the wheels also dropped under the probed one: a face found over the deck (an overhang
    // within face_under's 1 m) reads as one the board is under while it stands (Skate 1's he_p1_besttrick03 slot 4).
    let fault = if first.wheel_bottom < face - 0.01 {
        Some("spawned with the wheels in the ground")
    } else if deepest < -MAX_PENETRATION && low < face - MAX_PENETRATION {
        Some("fell through the face under it")
    } else if last.ground.is_none() {
        Some("nothing under it at the end")
    } else if last.bailing() {
        Some("wiping out")
    } else if last.state_category == 500 {
        Some("off the board")
    } else {
        None
    };
    match fault {
        Some(f) => Err(format!("{f}: {summary}")),
        None => Ok(summary),
    }
}

/// A game teleport to `at` (travel_to: skate_world::travel_ground, whose landing is 0.15 m over its face), watched for
/// STAND_TICKS.
fn stands_after_teleport(h: &mut Harness, at: Matrix) -> Result<String, String> {
    let face = crate::skate_world::travel_ground(h.physics.world(), at, true)[3][1] - 0.15;
    h.skater.travel_to(at)?;
    let s = h.run(STAND_TICKS, |_| pads::idle());
    stood(face, &s)
}

/// [`stands_after_teleport`] held, as a mode holds its teleported players through the card and the countdown
/// (match_core's hold, physics::pin).
fn stands_held(h: &mut Harness, at: Matrix) -> Result<String, String> {
    (h.hold, h.pin) = (true, Some(Default::default()));
    let result = stands_after_teleport(h, at);
    (h.hold, h.pin) = (false, None);
    result
}

/// B1 / C1 (map load): a Challenge Map / Party Play trip from another map builds the world at the row's start
/// (`load_at` -> `spawn_at`), which grounds it as the teleport does. Every University S.K.A.T.E. row's start slot 0,
/// and ots_univ_04_ol's: skate_univ_02's sits 0.31 m and ots_univ_04_ol's 0.235 m under the face in the retail data,
/// and fell through the world before the probe.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn b1_skate_spawns_stand_on_the_ground() {
    let rules = s3_rules("skate");
    let ids = rules["rows"].as_object().unwrap().keys().filter(|id| id.contains("_univ_")).cloned();
    for id in ids.chain(["ots_univ_04_ol".to_owned()]) {
        let online = online_row(&id);
        let slots: Vec<Matrix> = online["start"]["slots"].as_array().unwrap().iter().map(matrix).collect();
        let at = crate::online_map::locator_slot(&matrix(&online["start"]["matrix"]), &slots, 0);
        let mut h = Harness::load("University", Spawn::MapLoad(at));
        let face = crate::skate_world::probe_ground(h.physics.world(), at).1.expect("a face under the start");
        let s: Vec<Sample> = std::iter::once(h.sample()).chain(h.run(STAND_TICKS, |_| pads::idle())).collect();
        match stood(face, &s) {
            Ok(summary) => eprintln!("B1 {id} map load: {summary}"),
            Err(e) => panic!("B1 {id} map load: {e}"),
        }
    }
}

/// Every online placement slot, by map: where a mode's RepositionAI or the Challenge Map puts a player (the start,
/// attempt and waiting locators; not cameras, volumes or the spots a mode scores). S3: online.json's starts and the
/// modes' rules.json. S2: rows.json's modes, contests.json, hom.json and the online death races. S1: rows.json's starts
/// and the places its scripts reposition onto (`self.Location`, `SkateAttemptSpawnPoint`, `SkateWaitingLocation`,
/// `BestTrickStartLocations`). Each locator gives its first CHALLENGE_MAX slots (`locator_slot`, past the children
/// too); a position repeated across rows or files is listed once.
pub(crate) fn online_placements() -> std::collections::BTreeMap<String, Vec<(String, Matrix)>> {
    use serde_json::Value;
    fn placements<'a>(v: &'a Value, path: String, out: &mut Vec<(String, &'a Value)>) {
        match v {
            Value::Object(o) if o.get("matrix").is_some_and(Value::is_array) => {
                let key = path.rsplit('.').next().unwrap_or("").to_lowercase();
                let lower = path.to_lowercase();
                if ["start", "wait", "spawn", "attempt"].iter().any(|w| key.contains(w))
                    && !["camera", "volume", "objects"].iter().any(|w| lower.contains(w))
                {
                    out.push((path, v));
                }
            }
            Value::Object(o) => o.iter().for_each(|(k, x)| placements(x, format!("{path}.{k}"), out)),
            Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| placements(x, format!("{path}[{i}]"), out)),
            _ => {}
        }
    }
    fn challenges<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(o) if o.contains_key("challenge") => out.push(&o["challenge"]),
            Value::Object(o) => o.values().for_each(|x| challenges(x, out)),
            Value::Array(a) => a.iter().for_each(|x| challenges(x, out)),
            _ => {}
        }
    }
    let mut by_map: std::collections::BTreeMap<String, Vec<(String, Matrix)>> = Default::default();
    let max = crate::match_core::CHALLENGE_MAX;
    // `n`: the slots its modes use (the 1-Up and Skate 1 attempt spawns take slot 0; a Skate 2 S.K.A.T.E. lobby 4).
    let mut add = |map: &str, label: String, l: &Value, n: usize| {
        let slots: Vec<Matrix> = l["slots"].as_array().map(|s| s.iter().map(matrix).collect()).unwrap_or_default();
        let root = matrix(&l["matrix"]);
        let list = by_map.entry(map.to_owned()).or_default();
        for k in 0..n {
            let at = crate::online_map::locator_slot(&root, &slots, k);
            let key = |m: &Matrix| m[3][..3].iter().map(|v| (v * 100.).round() as i64).collect::<Vec<_>>();
            if !list.iter().any(|(_, m)| key(m) == key(&at)) {
                list.push((format!("{label} slot {k}"), at));
            }
        }
    };
    let online = private_json("challenge-map/online/online.json");
    let mut rows = Vec::new();
    challenges(&online["tabs"], &mut rows);
    for c in rows.into_iter().filter(|c| c["start"].is_object()) {
        let (map, id) = (c["map"].as_str().unwrap(), c["id"].as_str().unwrap_or("?"));
        // ponytail: Skate 2's S.K.A.T.E. rows by id (online.json's rows carry no mode); user 10-12: they hold 4.
        let n = if map == "New_San_Vanelona" && id.contains("skate_") { 4 } else { max };
        add(map, format!("online {id}.start"), &c["start"], n);
    }
    for mode in ["skate", "1up", "spot_battle", "hall_of_meat", "race", "contest", "domination", "ots", "film", "photo", "otl"] {
        for (id, row) in s3_rules(mode)["rows"].as_object().into_iter().flatten() {
            let Some(map) = crate::skate3_modes::map_of(id) else { continue };
            let mut found = Vec::new();
            placements(row, format!("{mode}/{id}"), &mut found);
            // A row not played online (rules `online`: a career challenge, as most photo / film / contest rows are)
            // only ever has its one player, in slot 0.
            let solo = row["online"] == false;
            for (path, l) in found {
                // 1-Up runs every attempt from its round's start locator's slot 0 (skate3_one_up).
                let n = if solo || mode == "1up" && path.contains(".StartLocationRotation[") { 1 } else { max };
                add(map, path, l, n);
            }
        }
    }
    let s2 = private_json("skate2-online/rows.json");
    let s2_map = s2["map"].as_str().unwrap().to_owned();
    for (file, v) in [
        ("rows.json", s2["modes"].clone()),
        ("contests.json", private_json("skate2-online/contests/contests.json")["rows"].clone()),
        ("hom.json", private_json("skate2-online/hall-of-meat/hom.json")["rows"].clone()),
    ] {
        let mut found = Vec::new();
        placements(&v, format!("s2 {file}"), &mut found);
        for (path, l) in found {
            // 1-Up's attempter goes to its attempt spawn's slot 0 (skate2_contests one_up_spawn), in rows.json and in
            // contests.json (its ol_1up_* rows).
            let one_up = path.contains(".one_up[") || path.contains("_1up_");
            let n = if one_up && (path.ends_with("TurnBasedAttemptSpawnPoint") || path.ends_with(".attempt")) { 1 } else { max };
            add(&s2_map, path, l, n);
        }
    }
    let races = private_json("skate2-online/death-race/death_race.json");
    for (id, race) in races["races"].as_object().unwrap().iter().filter(|(_, r)| r["online"] == true) {
        let start = &race["locators"][race["heat"]["start"].as_str().unwrap_or("")];
        if start.is_array() {
            add(races["map"].as_str().unwrap(), format!("s2 death race {id}"), &serde_json::json!({"matrix": start, "slots": race["heat"]["slots"]}), max);
        }
    }
    let s1 = private_json("skate1-online/rows.json");
    let s1_map = s1["map"].as_str().unwrap();
    for row in s1["modes"].as_object().unwrap().values().flat_map(|r| r.as_array().unwrap()) {
        let id = row["id"].as_str().unwrap_or("?");
        if row["start"].is_object() {
            add(s1_map, format!("s1 {id}.start"), &row["start"], max);
        }
        let me = &row["self"];
        let names = ["Location", "SkateAttemptSpawnPoint", "SkateWaitingLocation", "StartLocation"]
            .iter()
            .map(|k| &me[*k])
            .chain(me["BestTrickStartLocations"].as_array().into_iter().flatten());
        for name in names.filter_map(Value::as_str) {
            let place = &row["places"][name]["locator"];
            if place.is_object() {
                // The scripts reposition onto SkateAttemptSpawnPoint's slot 0 (RepositionAI(..., 0)).
                let n = if name == me["SkateAttemptSpawnPoint"] { 1 } else { max };
                add(s1_map, format!("s1 {id}.{name}"), place, n);
            }
        }
    }
    by_map
}

/// C1, the acceptance bar: every online placement slot (S3, S2 and S1 rows: start, attempt and waiting), teleported
/// onto as the modes do (`travel_to`: the ground probe, then horizontal_spawn) and watched for STAND_TICKS: it stands,
/// never falling more than MAX_FALL under the face. Before the probe, the six C1 locators fell through the world.
/// A slot with no face under it in our map lands on the nearest walkable face (skate_world::travel_ground) and must
/// stand there too; such slots are counted apart, as a map-coverage finding. One that falls unheld is tried again
/// held, as the modes hold every teleported player through its card and countdown: standing held, it is a skater
/// left to roll (a slope, a post at its nose), listed but not failed. `SKATE_C1_MAPS=University,Industrial` limits it
/// to those maps.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn c1_every_online_placement_stands() {
    let only = std::env::var("SKATE_C1_MAPS").ok();
    let (mut checked, mut failed, mut misses, mut held, mut moved) = (0, Vec::new(), Vec::new(), 0, 0);
    for (map, list) in online_placements() {
        if only.as_deref().is_some_and(|o| !o.split(',').any(|m| m == map)) {
            continue;
        }
        if !map_path(&map).exists() {
            eprintln!("C1 {map}: not installed, {} slots skipped", list.len());
            continue;
        }
        let started = std::time::Instant::now();
        let mut h = Harness::load(&map, Spawn::Package);
        h.run(60, |_| pads::idle());
        for (label, at) in &list {
            let (probed, face) = crate::skate_world::probe_ground(h.physics.world(), *at);
            if face.is_none() {
                eprintln!("C1 NO GROUND {map} {label} at {:?}: onto the nearest walkable face", at[3]);
                misses.push(format!("{map} {label} at {:?}", at[3]));
            }
            let landed = crate::skate_world::travel_ground(h.physics.world(), *at, true);
            if face.is_some() && landed != probed {
                moved += 1;
                let turn = (landed[2][0].atan2(landed[2][2]) - probed[2][0].atan2(probed[2][2]) + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                eprintln!(
                    "C1 MOVED {map} {label}: the board didn't fit, {:.2} m on, turned {:.0}°",
                    (landed[3][0] - probed[3][0]).hypot(landed[3][2] - probed[3][2]),
                    turn.to_degrees()
                );
            }
            checked += 1;
            match stands_after_teleport(&mut h, *at) {
                Ok(summary) => eprintln!("C1 {map} {label}: {summary}"),
                Err(e) => match stands_held(&mut h, *at) {
                    Ok(summary) => {
                        held += 1;
                        eprintln!("C1 HELD {map} {label}: unheld {e}; held {summary}");
                    }
                    Err(still) => {
                        eprintln!("C1 FAIL {map} {label} at {:?}: {e}; held {still}", at[3]);
                        failed.push(format!("{map} {label}: {e}; held {still}"));
                    }
                },
            }
        }
        eprintln!("C1 {map}: {} slots in {:.0?}", list.len(), started.elapsed());
    }
    eprintln!(
        "C1 checked {checked}, failed {}, stand only held {held}, no ground under {}, moved to fit the board {moved}",
        failed.len(),
        misses.len()
    );
    assert!(checked > 0 && failed.is_empty(), "{failed:#?}");
}

/// C2: a mode's teleport during a bail is released at once (match_core's gate) and the skater stays on the target:
/// retail's RepositionAI path has no wipeout gate, and the physics teleport ends the bail, so the bail's own reset never
/// follows. Requested 30 ticks into the bail, and inside its reset countdown (wipeoutalmostover). Before the fix the
/// gate held it for the whole bail (5.4 s) and the engine's reset popped the skater to its checkpoint first.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn c2_a_teleport_mid_bail_lands_and_stays() {
    let rules = s3_rules("skate");
    let spawn = |id: &str| matrix(&rules["rows"][id]["spawn"]["slots"][0]);
    let target = spawn("skate_univ_03");
    let face = |h: &Harness| crate::skate_world::probe_ground(h.physics.world(), target).1.unwrap();
    for countdown in [false, true] {
        let label = if countdown { "C2 in the reset countdown" } else { "C2 at bail t30" };
        let mut h = Harness::load("University", Spawn::Package);
        h.run(60, |_| pads::idle());
        h.teleport(spawn("skate_univ_01"));
        h.run(120, |_| pads::idle());
        h.run(90, pads::push);
        let mut t = 0;
        loop {
            let s = h.tick(if t < 30 { pads::bail() } else { pads::idle() });
            t += 1;
            let due = if countdown { s.teleport_pending } else { t >= 30 };
            if s.bailing() && due {
                break;
            }
            assert!(t < 600, "{label}: the bail never got there");
        }
        eprintln!("{label}: requested at bail tick {t}");
        assert!(!crate::match_core::teleport_blocked(&h.skater), "{label}: the gate holds the mode's teleport");
        h.teleport(target);
        let s = h.run(600, |_| pads::idle());
        let reset = s[5..].iter().position(|x| x.bailing() || x.category == FilteredCategory::Teleport as u32);
        assert_eq!(reset.map(|i| (i + 5, s[i + 5].category)), None, "{label}: the bail went on after the teleport");
        let last = s.last().unwrap();
        let flat = ((last.deck[0] - target[3][0]).powi(2) + (last.deck[2] - target[3][2]).powi(2)).sqrt();
        let result = stood(face(&h), &s);
        eprintln!("{label}: {flat:.2} m from the target 600 ticks later; {result:?}");
        assert!(flat < 1., "{label}: {flat:.2} m from the target");
        result.unwrap();
    }
}

/// B3: a waiting Party Play player's puppet stands where retail's physical waiting skater ends up: teleported to its
/// slot (RepositionAI) and settled on its board. Every University S.K.A.T.E. waiting slot, against a skater the
/// harness teleports there and lets settle; before the fix the puppet stood at the slot matrix itself.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn b3_waiting_puppets_stand_where_a_settled_skater_would() {
    let mut h = Harness::load("University", Spawn::Package);
    // M3: the puppet's height over the face is the board's rest height from retail tuning (WheelRadius and the
    // wheel's authored height under the deck), not a number read off this test.
    let rest = h.physics.settings.wheel_radius - h.physics.settings.authored[0].translation.y;
    assert!((crate::party_play::SETTLED_ROOT - rest).abs() < 1e-4, "SETTLED_ROOT vs the board's rest height {rest}");
    h.run(60, |_| pads::idle());
    let rules = s3_rules("skate");
    let mut worst: f32 = 0.;
    for (id, row) in rules["rows"].as_object().unwrap().iter().filter(|(id, _)| id.contains("_univ_")) {
        for (i, slot) in row["wait"]["slots"].as_array().unwrap().iter().enumerate() {
            let slot = matrix(slot);
            h.teleport(slot);
            let s = h.run(240, |_| pads::idle());
            let settled = s.last().unwrap();
            let face = h.ground(settled.root).unwrap_or(f32::NAN);
            let place = crate::party_play::waiting_place(h.physics.world(), slot);
            let (before, after) = (slot[3][1] - settled.root[1], place[3][1] - settled.root[1]);
            let flat = ((place[3][0] - settled.root[0]).powi(2) + (place[3][2] - settled.root[2]).powi(2)).sqrt();
            eprintln!(
                "B3 {id} slot {i}: settled root {:+.3} above face ({:?}); slot matrix {before:+.3} off, puppet {after:+.3} off, {flat:.3} m aside",
                settled.root[1] - face,
                settled.state
            );
            worst = worst.max(after.abs());
        }
    }
    assert!(worst < 0.01, "a waiting puppet stands {worst:.3} m off a settled skater's root");
}

/// User 10-27: Party Play's Spot Battle on the Super Ultra Mega Park: sitting on "press A to start", the skater rolled
/// away. Retail's AI.PinAI holds the player from the mode's teleport through the card and the countdown (cMsgPinPlayer,
/// all AI.ForceLocalPlayerStop sends); physics::pin holds a board a held match teleported where it settled. Every
/// University Spot Battle start, teleported to while held, stays where it settled for a minute, on the ground.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn a_held_skater_stays_on_its_start_for_a_minute() {
    let mut h = Harness::load("University", Spawn::Package);
    h.run(60, |_| pads::idle());
    (h.hold, h.pin) = (true, Some(Default::default()));
    let moved = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let rules = s3_rules("spot_battle");
    let (mut worst, mut starts) = ((0f32, String::new()), 0);
    for (id, row) in rules["rows"].as_object().unwrap() {
        let Some(spawn) = row["TurnBasedAttemptSpawnPoint"].get("matrix").filter(|_| crate::skate3_modes::map_of(id) == Some("University")) else {
            continue;
        };
        starts += 1;
        h.teleport(matrix(spawn));
        // The teleport lands and settles (the pin holds it from SETTLE ground ticks on); the minute starts a second in.
        let start = h.run(60, |_| pads::idle()).pop().unwrap();
        let s = h.run(3600, |_| pads::idle());
        let end = s.last().unwrap();
        let (deck, root) = (moved(start.deck, end.deck), moved(start.root, end.root));
        let off = s.iter().filter(|x| x.category != FilteredCategory::Ground as u32).count();
        eprintln!("{id}: deck {deck:.4} m, root {root:.4} m in a minute; {off} ticks off the ground");
        let worse = if off > 0 { f32::INFINITY } else { deck.max(root) };
        if worse > worst.0 {
            worst = (worse, id.clone());
        }
    }
    assert!(starts > 0, "no University Spot Battle start was found");
    assert!(worst.0 < 0.02, "{} moved {:.3} m in a minute on its start, held (inf: left the ground)", worst.1, worst.0);
}

/// Survey (item 1, every S3 mode): every locator a mode's rules.json places players on (each object with a `matrix`,
/// its slots, 8 players deep through `locator_slot`) and every online.json start, against the face under it on its
/// map. Prints each slot more than 2 cm under the face, with no face within 10 m, or more than 0.5 m above it.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn survey_every_s3_locator() {
    fn locators<'a>(v: &'a serde_json::Value, path: String, out: &mut Vec<(String, &'a serde_json::Value)>) {
        match v {
            serde_json::Value::Object(_) if path.contains("Camera") => {}
            serde_json::Value::Object(o) if o.contains_key("matrix") && o["matrix"].is_array() => out.push((path, v)),
            serde_json::Value::Object(o) => o.iter().for_each(|(k, x)| locators(x, format!("{path}.{k}"), out)),
            serde_json::Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| locators(x, format!("{path}[{i}]"), out)),
            _ => {}
        }
    }
    let modes = ["skate", "1up", "spot_battle", "hall_of_meat", "race", "contest", "domination", "ots", "film", "photo", "otl"];
    let mut by_map: std::collections::BTreeMap<&str, Vec<(String, Matrix, Vec<Matrix>)>> = Default::default();
    for mode in modes {
        let rules = s3_rules(mode);
        for (id, row) in rules["rows"].as_object().into_iter().flatten() {
            let Some(map) = crate::skate3_modes::map_of(id) else { continue };
            let mut found = Vec::new();
            locators(row, format!("{mode}/{id}"), &mut found);
            if let Some(start) = online_row_maybe(id) {
                found.push((format!("{mode}/{id}.online_start"), start));
            }
            for (path, l) in found {
                let slots: Vec<Matrix> = l["slots"].as_array().map(|s| s.iter().map(matrix).collect()).unwrap_or_default();
                by_map.entry(map).or_default().push((path, matrix(&l["matrix"]), slots));
            }
        }
    }
    let (mut checked, mut flagged) = (0, 0);
    for (map, list) in by_map {
        let path = map_path(map);
        if !path.exists() {
            eprintln!("LOCATORS {map}: not installed ({} locators skipped)", list.len());
            continue;
        }
        let skate_map = SkateMap::load(&path).unwrap();
        let physics = GamePhysics::load_at(&asset_root(), Some(&skate_map), crate::difficulty::Difficulty::Easy, None).unwrap();
        for (what, m, slots) in list {
            for k in 0..8 {
                let at = crate::online_map::locator_slot(&m, &slots, k)[3];
                checked += 1;
                let face = crate::skate_world::face_under(physics.world(), [at[0], at[1], at[2]]);
                let height = face.map(|f| at[1] - f);
                if height.is_none_or(|h| !(-0.02..=0.5).contains(&h)) {
                    flagged += 1;
                    eprintln!("LOCATOR {map} {what} slot {k}{}: {height:?} above the face", if k >= slots.len() { " (past the children)" } else { "" });
                }
            }
        }
    }
    eprintln!("LOCATORS checked {checked}, flagged {flagged}");
}

/// Survey: every face in the column under skate_univ_02's online start slot 0 (0.88 m below its siblings), and where
/// a skater spawned there (map load, as a cross-map pick) ends up.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn survey_skate_univ_02_start_slot_0() {
    let at = matrix(&online_row("skate_univ_02")["start"]["slots"][0]);
    let [x, y, z] = [at[3][0], at[3][1], at[3][2]];
    let mut h = Harness::load("University", Spawn::MapLoad(at));
    let mut top = y + 5.;
    let mut faces = Vec::new();
    while let Some(face) = crate::skate_world::face_under(h.physics.world(), [x, top - 1., z]).filter(|f| *f > y - 12.) {
        faces.push(face);
        top = face - 0.01 + 1.;
        if faces.len() > 10 {
            break;
        }
    }
    eprintln!("univ_02 start slot 0 at y {y:.3}: faces below y+4 {faces:?}");
    for (i, s) in h.run(240, |_| pads::idle()).iter().enumerate() {
        if i % 20 == 0 || i < 5 {
            eprintln!("univ_02 slot 0 map load t{i}: deck {:?} wheels {:.3} {:?} cat {}", s.deck, s.wheel_bottom, s.state, s.category);
        }
    }
}

/// online.json's row with this id, if it has one.
fn online_row_maybe(id: &str) -> Option<&'static serde_json::Value> {
    static ONLINE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let online = ONLINE.get_or_init(|| private_json("challenge-map/online/online.json"));
    fn find<'a>(v: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
        match v {
            serde_json::Value::Object(o) if o.get("id").and_then(|i| i.as_str()) == Some(id) && o.contains_key("start") => o.get("start"),
            serde_json::Value::Object(o) => o.values().find_map(|v| find(v, id)),
            serde_json::Value::Array(a) => a.iter().find_map(|v| find(v, id)),
            _ => None,
        }
    }
    find(online, id).filter(|s| s.is_object())
}

fn print(label: &str, samples: &[Sample], h: &Harness) {
    for (i, s) in samples.iter().enumerate() {
        if i < 4 || i % 30 == 0 || i + 1 == samples.len() {
            let g = h.ground(s.root).or(s.ground).unwrap_or(f32::NAN);
            eprintln!(
                "{label} t{i}: wheels {:+.3} lowest body {:+.3} root {:+.3} above face; deck {:?} {:?} cat {} wiping {}",
                s.clearance().unwrap_or(f32::NAN),
                s.lowest_body - g,
                s.root[1] - g,
                s.deck,
                s.state,
                s.category,
                s.board_wiping_out
            );
        }
    }
}

/// D5 (online audit 10-06, shared by every mode's teleport): a skater who got off its board (pinned on foot, so never
/// remounting) is still moved by a mode's RepositionAI, and lands on its board. Before the fix match_core waited for
/// `board_wiping_out`, which stays up while the board is free.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn d5_a_skater_on_foot_is_teleported_onto_its_board() {
    let at = matrix(&online_row("skate_univ_01")["start"]["slots"][0]);
    let mut h = Harness::load("University", Spawn::Package);
    h.run(60, |_| pads::idle());
    let s = h.run(240, |t| if t == 20 { pads::buttons(pads::Y) } else { pads::idle() });
    let last = s.last().unwrap();
    eprintln!("D5 on foot: board_wiping_out {}, category {}, state {:?}", last.board_wiping_out, last.category, last.state);
    assert_eq!(last.state_category, 500, "got off the board");
    assert!(!crate::match_core::teleport_blocked(&h.skater), "a skater on foot is not mid-bail: its teleport goes");
    stands_after_teleport(&mut h, at).map(|s| eprintln!("D5 teleported walker: {s}")).unwrap();
}

/// Film / Photo SpawnOffboard: RepositionAI's on-foot spawn stands the skater at the start, off its board, not
/// bailing. Before the fix the row started on the board. photo_indu_01 is the online row that sets it (film_univ_09
/// is the University tutorial, offline, its start in the tutorial's own space at y 3: no University face under it).
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn spawn_offboard_rows_start_on_foot() {
    let row = &s3_rules("photo")["rows"]["photo_indu_01"];
    assert_eq!((&row["SpawnOffboard"], &row["online"]), (&serde_json::json!(true), &serde_json::json!(true)));
    let start: Vec<Matrix> = row["StartLocation"]["slots"].as_array().unwrap().iter().map(matrix).collect();
    let at = crate::online_map::locator_slot(&matrix(&row["StartLocation"]["matrix"]), &start, 0);
    let mut h = Harness::load("Industrial", Spawn::Package);
    h.run(60, |_| pads::idle());
    h.skater.travel_to_as(at, false).unwrap();
    let s = h.run(120, |_| pads::idle());
    eprintln!("photo_indu_01 start {:?}, face under it {:?}", at[3], h.ground([at[3][0], at[3][1], at[3][2]]));
    for x in s.iter().step_by(20) {
        eprintln!("  t{} root {:?} deck {:?} lowest {:.3} {:?} cat {} sc {}", x.tick, x.root, x.deck, x.lowest_body, x.state, x.category, x.state_category);
    }
    let last = s.last().unwrap();
    let face = h.ground(last.root).expect("a face under the skater");
    let flat = ((last.root[0] - at[3][0]).powi(2) + (last.root[2] - at[3][2]).powi(2)).sqrt();
    eprintln!("on foot: {:?}, cat {}, lowest body {:+.3} above face, {flat:.2} m from the start", last.state, last.category, last.lowest_body - face);
    assert_eq!(last.state_category, 500, "on foot");
    assert!(!last.bailing() && last.lowest_body > face - 0.05 && flat < 1.);
}

/// Survey: the spawn paths onto S.K.A.T.E. University 01's start (online.json slot 0): the map load's own spawn, the
/// package spawn then a teleport before the first tick (Party Play's skate rows, which have no teleport id), and a
/// settled teleport (the reference).
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn survey_spawn_paths() {
    let at = matrix(&online_row("skate_univ_01")["start"]["slots"][0]);
    let mut h = Harness::load("University", Spawn::MapLoad(at));
    print("map load", &[h.sample()], &h);
    let s = h.run(240, |_| pads::idle());
    print("map load", &s, &h);
    let mut h = Harness::load("University", Spawn::Package);
    h.teleport(at);
    let s = h.run(240, |_| pads::idle());
    print("package+early teleport", &s, &h);
    let mut h = Harness::load("University", Spawn::Package);
    h.run(60, |_| pads::idle());
    h.teleport(at);
    let s = h.run(240, |_| pads::idle());
    print("settled teleport", &s, &h);
    // Getting off (Y) and back on, then a manual bail: the signals the modes read.
    let s = h.run(500, |t| match t {
        20 | 300 => pads::buttons(pads::Y),
        _ => pads::idle(),
    });
    let up = |f: &dyn Fn(&Sample) -> bool| s.iter().filter(|s| f(s)).count();
    eprintln!(
        "dismount+remount: board_wiping_out {} ticks, Wipeout category {} ticks, off board {} ticks",
        up(&|s| s.board_wiping_out),
        up(&|s| s.bailing()),
        up(&|s| s.state_category == 500)
    );
    print("dismount", &s, &h);
    let s = h.run(600, |t| if t < 30 { pads::bail() } else { pads::idle() });
    let edges: Vec<_> = s
        .windows(2)
        .filter(|w| {
            (w[0].board_wiping_out, w[0].category, w[0].teleport_pending) != (w[1].board_wiping_out, w[1].category, w[1].teleport_pending)
        })
        .map(|w| (w[1].tick, w[1].board_wiping_out, w[1].category, w[1].teleport_pending, w[1].state))
        .collect();
    eprintln!("bail edges (tick, board_wiping_out, category, teleport_pending, state): {edges:?}");
}

/// Bug 10-31 (Party Play): "you spawn in a wall" on The Walrus, and other Skate 1 spawns look wrong. Every place the
/// Skate 1 Spot Battle and S.K.A.T.E. scripts put a player (the attempter's SkateAttemptSpawnPoint slot 0, the waiting
/// Location / SkateWaitingLocation slots of four players, as RepositionAI resolves them: skate1_online's
/// Locator::spawn), teleported onto as the modes do. Prints what is over it, around its board, how far the nearest
/// wall is ahead and behind (the camera's side) at 1 m, and whether it stands; fails on a place with no ground under it,
/// an attempt spawn with a hole in the map's collision 1-3 m ahead (bug 11-01: The Walrus' attempt locator under its
/// landing), a place that falls held too (the scripts stop every player they place: ForceLocalPlayerStop(true),
/// PinAI; c1's rule), and a place other than The Walrus' attempt that BAM_LIFT lands on another face. NO_VENUE: rows
/// whose venue the converted city map lacks (the X Games; Mini Mega's ramp),
/// printed, not failed.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn s1_turn_spawns_are_clear_and_stand() {
    use crate::skate1_online::{Place, Row};
    use skate_core::math::Vector3;
    const NO_VENUE: [&str; 3] = ["spotbattle_financial_xgames", "skate_financial_xgames", "ol_spotbattle_urbanres_minimega"];
    let rows = private_json("skate1-online/rows.json");
    let mut h = Harness::load(rows["map"].as_str().unwrap(), Spawn::Package);
    h.run(60, |_| pads::idle());
    let hit = |h: &Harness, a: [f32; 3], b: [f32; 3]| {
        let v = |p: [f32; 3]| Vector3::new(p[0], p[1], p[2]);
        let x = h.physics.world().query_thin_line(v(a), v(b)).ok().flatten()?;
        Some([x.geometry.position.x, x.geometry.position.y, x.geometry.position.z])
    };
    let mut failed = Vec::new();
    let mut lifted_elsewhere = Vec::new();
    // The waiting place: Spot Battle's Location (the observer), S.K.A.T.E.'s SkateWaitingLocation (its Location is the
    // lobby start, which its script never repositions onto).
    for (mode, waiting) in [("spot_battle", "Location"), ("skate", "SkateWaitingLocation")] {
        for json in rows["modes"][mode].as_array().unwrap() {
            let row: Row = serde_json::from_value(json.clone()).unwrap();
            let id = row.id.as_str();
            let mut places = Vec::new();
            for (key, n) in [("SkateAttemptSpawnPoint", 1), (waiting, 4)] {
                let Some(Place::Locator(l)) = row.fields[key].as_str().and_then(|name| row.places.get(name)) else { continue };
                for slot in 0..n {
                    let at = l.spawn(slot, &row);
                    let lifted = l.slots.is_empty() && (at[3][1] - l.matrix[3][1] - crate::skate1_online::BAM_LIFT).abs() < 1e-3;
                    places.push((format!("{key}[{slot}]"), at, lifted));
                }
            }
            for (label, at, lifted) in places {
                let (probed, face) = crate::skate_world::probe_ground(h.physics.world(), at);
                let Some(face) = face else {
                    eprintln!("S1 {id} {label}: NO GROUND at {:?}", at[3]);
                    if !NO_VENUE.contains(&id) {
                        failed.push(format!("{id} {label}: no ground"));
                    }
                    continue;
                };
                let p = probed[3];
                // Over the skater: anything from 0.3 m to 2.2 m above the face it lands on.
                let over = hit(&h, [p[0], face + 2.2, p[2]], [p[0], face + 0.3, p[2]]);
                // Around the board: its nose and tail (0.5 m along its heading) at wheel and knee height.
                let f = [probed[2][0], probed[2][2]];
                let len = f[0].hypot(f[1]).max(1e-6);
                let f = [f[0] / len, f[1] / len];
                let ray = |h: &Harness, up: f32, s: f32, reach: f32| {
                    hit(h, [p[0], face + up, p[2]], [p[0] + s * reach * f[0], face + up, p[2] + s * reach * f[1]])
                        .map(|q| (q[0] - p[0]).hypot(q[2] - p[2]))
                };
                let around: Vec<f32> = [0.1f32, 0.5].iter().flat_map(|&up| [1f32, -1.].map(|s| ray(&h, up, s, 0.5))).flatten().collect();
                let (ahead, behind) = (ray(&h, 1., 1., 10.), ray(&h, 1., -1., 10.));
                // Only the attempter rides off its spawn; a waiting place may face a wall with a building behind it.
                let hole = [1f32, 1.5, 2., 2.5, 3.].into_iter().filter(|_| label.starts_with("SkateAttemptSpawnPoint")).find(|d| {
                    let q = [p[0] + d * f[0], p[2] + d * f[1]];
                    hit(&h, [q[0], face + 4., q[1]], [q[0], face - 30., q[1]]).is_none()
                });
                let mut unlifted = at;
                unlifted[3][1] -= crate::skate1_online::BAM_LIFT;
                let before = lifted.then(|| crate::skate_world::probe_ground(h.physics.world(), unlifted).1).flatten();
                if lifted && before.is_none_or(|b| (b - face).abs() > 0.3) {
                    eprintln!("S1 {id} {label}: BAM_LIFT lands on {face:.2}, not {before:?}");
                    lifted_elsewhere.push(format!("{id} {label}"));
                }
                let stands = stands_after_teleport(&mut h, at)
                    .or_else(|unheld| stands_held(&mut h, at).map(|held| format!("stands only held ({unheld}): {held}")));
                let report = format!(
                    "{:?} heading [{:.2}, {:.2}] -> face {face:.2} ({:+.2} under the locator); over {over:?}; around {around:?}; \
                     wall ahead {ahead:?} behind {behind:?}; hole ahead {hole:?}; {}",
                    at[3],
                    f[0],
                    f[1],
                    at[3][1] - face,
                    stands.as_ref().map_or_else(|e| format!("FAILS: {e}"), String::clone)
                );
                eprintln!("S1 {id} {label}: {report}");
                if (stands.is_err() || hole.is_some()) && !NO_VENUE.contains(&id) {
                    failed.push(format!("{id} {label}: {report}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{} Skate 1 places have no ground, a hole ahead or do not stand:\n{}", failed.len(), failed.join("\n"));
    assert_eq!(lifted_elsewhere, ["ol_spotbattle_oldtown_warehouststeps SkateAttemptSpawnPoint[0]"], "BAM_LIFT moves only The Walrus' attempt");
}

/// Party Play 11-01, The Walrus again ("you spawn inside a wall, and when you ride forward you fall under the map").
/// Survey (prints, asserts nothing) of the row's places as Party Play reaches them: the online start (the trip's spot,
/// with the locator's default -X heading), the attempt spawn and the watcher's slot 0 (Locator::spawn). For each: the
/// faces under the path 25 m ahead, the drawn and the collision triangles round the skater's body, and a 5 s push.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn survey_the_walrus_spawns() {
    use crate::skate1_online::{Place, Row};
    use bevy::math::Vec3;
    use skate_core::math::Vector3;
    const ID: &str = "ol_spotbattle_oldtown_warehouststeps";
    let rows = private_json("skate1-online/rows.json");
    let json = rows["modes"]["spot_battle"].as_array().unwrap().iter().find(|r| r["id"] == ID).unwrap().clone();
    let row: Row = serde_json::from_value(json).unwrap();
    let locator = |k: &str| match &row.places[row.fields[k].as_str().unwrap()] {
        Place::Locator(l) => l.clone(),
        _ => panic!("{k} is not a locator"),
    };
    let places = [
        ("online start", matrix(&online_row(ID)["start"]["matrix"])),
        ("attempt", locator("SkateAttemptSpawnPoint").spawn(0, &row)),
        ("watcher 0", locator("Location").spawn(0, &row)),
    ];
    let mut h = Harness::load(rows["map"].as_str().unwrap(), Spawn::Package);
    h.run(60, |_| pads::idle());
    for (label, at) in places {
        let p = Vec3::new(at[3][0], at[3][1], at[3][2]);
        let f = Vec3::new(at[2][0], 0., at[2][2]).normalize();
        eprintln!("WALRUS {label}: at {p:?} heading {f:?}");
        // The first face under the path, from 4 m over the spawn's height to 30 m under it ("-": none).
        let ahead: Vec<String> = (0..=50)
            .map(|i| {
                let q = p + f * (i as f32 * 0.5);
                let line = h.physics.world().query_thin_line(Vector3::new(q.x, p.y + 4., q.z), Vector3::new(q.x, p.y - 30., q.z));
                line.ok().flatten().map_or("-".into(), |x| format!("{:.2}", x.geometry.position.y - p.y))
            })
            .collect();
        eprintln!("WALRUS {label}: faces under the path every 0.5 m, height vs the spawn: {}", ahead.join(" "));
        // Triangles whose bounds reach into the body's box: 1 m either side, 0.1-2 m over the spawn.
        let crosses = |q: [[f32; 3]; 3]| {
            let lo = |k: usize| q.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min);
            let hi = |k: usize| q.iter().map(|v| v[k]).fold(f32::NEG_INFINITY, f32::max);
            lo(0) < p.x + 1. && hi(0) > p.x - 1. && lo(2) < p.z + 1. && hi(2) > p.z - 1. && lo(1) < p.y + 2. && hi(1) > p.y + 0.1
        };
        let mut drawn = std::collections::BTreeMap::<String, usize>::new();
        for t in h.map.geometry.indices.chunks_exact(3) {
            let v = [t[0], t[1], t[2]].map(|i| &h.map.geometry.vertices[i as usize]);
            if crosses(v.map(|v| v.position)) {
                let name = h.map.materials.get(v[0].material as usize).map_or("?", |m| m.name.as_str());
                *drawn.entry(name.to_owned()).or_default() += 1;
            }
        }
        let solid = h.map.geometry.collision.iter().filter(|c| crosses(c.points)).count();
        eprintln!("WALRUS {label}: drawn triangles round the body by material {drawn:?}; collision triangles {solid}");
        h.teleport(at);
        let settle = h.run(90, |_| pads::idle());
        let ride = h.run(300, pads::push);
        for s in settle.iter().chain(&ride).step_by(30) {
            eprintln!(
                "WALRUS {label} t{}: deck {:.2?} ground {:?} lowest body {:.2} {:?} cat {} speed {:.2}",
                s.tick, s.deck, s.ground, s.lowest_body, s.state, s.category, s.speed
            );
        }
    }
}
