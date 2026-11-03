//! The career export (`private/skate3-career`, `tools/skate3_career.py`): `career.json`, `strings.json` and
//! `scripts/progression.lua`, read once at startup. The types mirror the exporter's JSON (Tasks 2-6); unknown fields
//! are ignored, so a later stage's export still loads here. Not tested in play.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The `career.json` version this build reads (the exporter's `EXPORT_VERSION`).
pub const EXPORT_VERSION: u32 = 1;

/// One career action (`careeractions`, career_flow Q1): its conditions are `(kind, name, value)`, ANDed.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Action {
    pub name: String,
    pub key: u64,
    pub auto: bool,
    pub conditions: Vec<(u32, String, u64)>,
    pub stategraph: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StateGraph {
    pub initial: String,
    pub states: BTreeMap<String, State>,
}

/// A stategraph state: its Lua handlers by message (`stateenter`, `hudcomplete`, ...) and its presentation events.
#[derive(Clone, Debug, Deserialize)]
pub struct State {
    pub handlers: BTreeMap<String, String>,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub kind: u32,
    pub args: Vec<String>,
    pub flags: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Reward {
    pub category: u32,
    pub item: String,
    pub title: String,
    pub description: String,
    pub icon: String,
    pub raw: Vec<i64>,
}

/// What owning or killing a row pays.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Payout {
    pub cash: u64,
    pub boards: u64,
    pub rewards: Vec<String>,
}

/// A world and a locator in it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Place {
    pub world: String,
    pub location: String,
}

/// One challenge row (`challenges` bank).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Row {
    pub key: u64,
    pub title: String,
    pub description: String,
    pub kind: u32,
    pub graph: String,
    pub world: String,
    pub location: String,
    pub map_start: Option<String>,
    pub start: Option<String>,
    pub offboard: bool,
    pub indicator: String,
    pub own: Payout,
    pub kill: Payout,
    pub online_bonus: u64,
    pub killable: bool,
    pub lot: Option<String>,
    pub lot_rows: Vec<String>,
    pub disabled: bool,
    pub global: u32,
    pub signup_half: f32,
    pub end_teleport: Option<String>,
    pub end_world: Option<String>,
    pub pack: Option<String>,
    pub exits: Vec<Place>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FeLocations {
    pub schoolstart: Place,
    pub gamestart: Place,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Locator {
    pub map: String,
    pub matrix: [[f32; 4]; 4],
    pub slots: Vec<Vec<Vec<f32>>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Volume {
    pub map: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub id: u64,
}

/// A VisualIndicator of an objective: a locator's chevron (`kind` 0) or a spline ribbon (`kind` 1).
#[derive(Clone, Debug, Deserialize)]
pub struct Indicator {
    pub objective: String,
    pub kind: u32,
    pub locator: Option<String>,
    #[serde(rename = "type")]
    pub shape: String,
    pub style: String,
    pub spline: Vec<[f32; 3]>,
    pub flags: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Character {
    pub key: String,
    pub recipe: String,
    pub name: String,
    pub category: String,
    pub unlock: String,
}

/// `career.json` as the exporter writes it.
#[derive(Deserialize)]
struct CareerJson {
    actions: Vec<Action>,
    groups: BTreeMap<String, Vec<String>>,
    stategraphs: BTreeMap<String, StateGraph>,
    rewards: BTreeMap<String, Reward>,
    rows: BTreeMap<String, Row>,
    fe_locations: FeLocations,
    worlds: BTreeMap<String, Option<String>>,
    locators: BTreeMap<String, Locator>,
    volumes: BTreeMap<String, Volume>,
    indicators: BTreeMap<String, Vec<Indicator>>,
    characters: Vec<Character>,
    all_characters_key: String,
    reward_sites: BTreeMap<String, Vec<String>>,
}

/// The whole export, loaded.
#[derive(Debug, Default)]
pub struct CareerData {
    pub actions: Vec<Action>,
    pub groups: BTreeMap<String, Vec<String>>,
    pub stategraphs: BTreeMap<String, StateGraph>,
    pub rewards: BTreeMap<String, Reward>,
    pub rows: BTreeMap<String, Row>,
    pub fe_locations: FeLocations,
    pub worlds: BTreeMap<String, Option<String>>,
    pub locators: BTreeMap<String, Locator>,
    pub volumes: BTreeMap<String, Volume>,
    pub indicators: BTreeMap<String, Vec<Indicator>>,
    pub characters: Vec<Character>,
    pub all_characters_key: String,
    pub reward_sites: BTreeMap<String, Vec<String>>,
    pub strings: BTreeMap<String, String>,
    /// `scripts/progression.lua`, retail's progression script.
    pub lua: String,
    pub root: PathBuf,
}

impl CareerData {
    #[must_use]
    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.name == name)
    }

    #[must_use]
    pub fn row(&self, name: &str) -> Option<&Row> {
        self.rows.get(name)
    }

    /// The string `id`, or `id` itself when the export has no such string.
    #[must_use]
    pub fn text<'a>(&'a self, id: &'a str) -> &'a str {
        self.strings.get(id).map_or(id, String::as_str)
    }
}

/// Why the export did not load. Its `Display` is the log line that turns the career off.
#[derive(Debug)]
pub enum LoadError {
    Missing(PathBuf),
    Version { found: u32 },
    Damaged(PathBuf, String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "Skate 3 career off: {} is missing", path.display()),
            Self::Version { found } => {
                write!(f, "Skate 3 career off: the export is version {found}, this build reads version {EXPORT_VERSION}")
            }
            Self::Damaged(path, why) => write!(f, "Skate 3 career off: {} is damaged: {why}", path.display()),
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, LoadError> {
    std::fs::read(path).map_err(|why| match why.kind() {
        std::io::ErrorKind::NotFound => LoadError::Missing(path.to_owned()),
        _ => LoadError::Damaged(path.to_owned(), why.to_string()),
    })
}

/// Loads the export at `root`: the version first, so an old export says so rather than failing to parse.
///
/// # Errors
///
/// `Missing` names the folder or file that is not there, `Version` an export of another version, `Damaged` a file
/// that does not parse.
pub fn load(root: &Path) -> Result<CareerData, LoadError> {
    if !root.is_dir() {
        return Err(LoadError::Missing(root.to_owned()));
    }
    let path = root.join("career.json");
    let bytes = read(&path)?;
    let damaged = |why: serde_json::Error| LoadError::Damaged(path.clone(), why.to_string());
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(damaged)?;
    let found = value.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if found != u64::from(EXPORT_VERSION) {
        return Err(LoadError::Version { found: u32::try_from(found).unwrap_or(u32::MAX) });
    }
    let career: CareerJson = serde_json::from_value(value).map_err(damaged)?;
    let lua_path = root.join("scripts").join("progression.lua");
    let lua = String::from_utf8(read(&lua_path)?).map_err(|why| LoadError::Damaged(lua_path.clone(), why.to_string()))?;
    let strings_path = root.join("strings.json");
    let strings = serde_json::from_slice(&read(&strings_path)?)
        .map_err(|why| LoadError::Damaged(strings_path.clone(), why.to_string()))?;
    Ok(CareerData {
        actions: career.actions,
        groups: career.groups,
        stategraphs: career.stategraphs,
        rewards: career.rewards,
        rows: career.rows,
        fe_locations: career.fe_locations,
        worlds: career.worlds,
        locators: career.locators,
        volumes: career.volumes,
        indicators: career.indicators,
        characters: career.characters,
        all_characters_key: career.all_characters_key,
        reward_sites: career.reward_sites,
        strings,
        lua,
        root: root.to_owned(),
    })
}
