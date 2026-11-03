//! Retail Online Challenge Map: data model and navigation (no rendering).
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::apt_scene::Texture;

/// Retail spawn: the row's MapStartLocation/Location global locator and its `*_spawn_<n>` children.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Start {
    pub matrix: [[f32; 4]; 4],
    /// One per lobby slot (retail `AI.RepositionAI(player, Location, playerSlot)`).
    #[serde(default)]
    pub slots: Vec<[[f32; 4]; 4]>,
}

/// A locator's spawn for `slot`, as the engine resolves one (S2 0x82879950, S1 0x823d9088, TU3 0x82910eb0, the same
/// code): child `slot` in table order; past the children, child 0 (with none, the locator) moved 5 m along world -Z
/// per slot.
pub(crate) fn locator_slot(matrix: &[[f32; 4]; 4], slots: &[[[f32; 4]; 4]], slot: usize) -> [[f32; 4]; 4] {
    if let Some(m) = slots.get(slot) {
        return *m;
    }
    let mut m = slots.first().unwrap_or(matrix).to_owned();
    m[3][2] -= 5. * slot as f32;
    m
}

impl Start {
    /// The lobby slot's spawn ([`locator_slot`]).
    pub fn slot(&self, slot: usize) -> [[f32; 4]; 4] {
        locator_slot(&self.matrix, &self.slots, slot)
    }

    fn valid(&self) -> bool {
        std::iter::once(&self.matrix).chain(&self.slots).all(|m| crate::teleport_menu::valid_matrix(*m))
    }
}

/// Spawn and online-area data shared by challenge and location rows (absent in older online.json).
#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Spawn {
    #[serde(default)]
    pub start: Option<Start>,
    /// Online-area polygon, world [x, z].
    #[serde(default)]
    pub boundary: Option<Vec<[f32; 2]>>,
    #[serde(default)]
    pub oob_time: Option<f32>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Challenge {
    pub id: String,
    pub map: String,
    #[serde(default)]
    pub teleport: Option<String>,
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    #[serde(flatten)]
    pub spawn: Spawn,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub objectives: Vec<String>,
    #[serde(default)]
    pub bonus: u32,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub map_image: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Location {
    #[serde(default)]
    pub id: String,
    pub map: String,
    #[serde(default)]
    pub teleport: Option<String>,
    #[serde(flatten)]
    pub spawn: Spawn,
}
impl Location {
    /// User 10-29: an added Teleport Spot's row has no start of its own: its teleport's spot is its locator, so each
    /// player gets a slot (locator_slot: 5 m apart), not one spot for everyone.
    pub fn spawn_at(&self, destinations: &[crate::teleport_menu::Destination]) -> Spawn {
        let mut spawn = self.spawn.clone();
        if spawn.start.is_none() {
            let matrix = self.teleport.as_ref().and_then(|id| destinations.iter().find(|d| &d.id == id)?.matrix);
            spawn.start = matrix.map(|matrix| Start { matrix, slots: Vec::new() });
        }
        spawn
    }
}

/// User 10-29: 12 added Teleport Spots (rows with a teleport and no start) shared their name with a retail online
/// location on the same tab, and a row is found by its name (a lobby's tag, a move's target), so picking the Teleport
/// Spot sent everyone to the retail one. Those get " (Teleport Spot)" on their name.
fn teleport_spot_names(mut tab: Tab) -> Tab {
    fn names(rows: &[Entry], out: &mut BTreeSet<String>) {
        for e in rows {
            if e.location.as_ref().is_some_and(|l| l.spawn.start.is_some()) {
                out.insert(e.name.clone());
            }
            names(&e.rows, out);
        }
    }
    fn rename(rows: &mut [Entry], retail: &BTreeSet<String>) {
        for e in rows {
            let spot = e.location.as_ref().is_some_and(|l| l.spawn.start.is_none() && l.teleport.is_some());
            if spot && e.name.starts_with('#') && retail.contains(&e.name) {
                e.name.push_str(" (Teleport Spot)");
            }
            rename(&mut e.rows, retail);
        }
    }
    let mut retail = BTreeSet::new();
    names(&tab.rows, &mut retail);
    rename(&mut tab.rows, &retail);
    tab
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Entry {
    pub name: String,
    #[serde(default)]
    pub header: bool,
    #[serde(default)]
    pub folder: bool,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub challenge: Option<Challenge>,
    #[serde(default)]
    pub location: Option<Location>,
    #[serde(default)]
    pub rows: Vec<Entry>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct Tab {
    pub name: String,
    #[serde(default)]
    pub rows: Vec<Entry>,
}

#[derive(Deserialize)]
struct File {
    format: String,
    version: u32,
    #[serde(default)]
    icons: BTreeMap<String, serde_json::Value>,
    tabs: Vec<Tab>,
}

#[derive(Clone)]
pub(crate) struct OnlineMap {
    pub tabs: Vec<Tab>,
    pub icons: BTreeMap<String, Texture>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    JoinPublic,
    CreatePrivate,
}

#[derive(Clone, Debug)]
pub(crate) enum Pick {
    /// `folder`: the open folder's name (a challenge type filter), empty at the tab root.
    Command { command: String, tab: usize, folder: String },
    /// `name`: the challenge row's title (a string id).
    Challenge { challenge: Challenge, name: String, choice: Choice, tab: usize, folder: String },
    /// `name`: the location row's string id.
    Location { location: Location, tab: usize, name: String, choice: Choice },
}
impl Pick {
    /// The row a session move or a proposal names (challenges and locations only).
    pub fn target(&self) -> Option<skate_net::roster::Target> {
        use skate_net::roster::Target;
        match self {
            Pick::Command { .. } => None,
            Pick::Challenge { challenge, name, tab, .. } => Some(Target::new(*tab, &challenge.id, &challenge.map, name)),
            Pick::Location { location, tab, name, .. } => Some(Target::new(*tab, name, &location.map, name)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Nav {
    pub tab: usize,
    pub open: Option<usize>,
    pub highlighted: usize,
}

/// Challenge/location rows a folder still holds.
fn count(rows: &[Entry]) -> usize {
    rows.iter().filter(|e| e.challenge.is_some() || e.location.is_some()).count()
}

/// Skate 1 rows in places the Skate 1 map (the PS3 map mod: the city alone) doesn't have, the X Games venue and Plan B:
/// their starts are over nothing (user 10-30: spawned in the middle of nowhere and fell). Mini Mega's ramp isn't in the
/// map either: no ground under its spawns (user 10-31: hidden until it is).
const MISSING_AREAS: [&str; 10] = [
    "freeskate_financial_xgames", "spotbattle_financial_xgames", "ol_besttrick_financial_xgames", "jam_financial_xgames",
    "jam_finanical_xgames_test", "skate_financial_xgames", "freeskate_planb", "jam_oldtown_planb", "ol_he_a4_besttrick03",
    "ol_spotbattle_urbanres_minimega",
];

/// Skate 2 had no 1-Up (user 11-01): the Skate 2 tab's 1-Up folder (these ten rows) is left out, so neither the map nor
/// Party Play offers one. skate2_contests keeps the mode's rules.
const SKATE_2_ONE_UPS: [&str; 10] = [
    "ol_1up_01_svm_mountainup", "ol_1up_02_dt_matrix", "ol_1up_03_ot_cathedral", "ol_1up_04_wf_gvr",
    "ol_1up_05_ur_lombardier", "t_1a_01_1up_location", "t_2a_01_1up_location", "t_3a_01_1up_location",
    "t_4a_01_1up_classic", "t_5a_01_1up_snakes",
];

fn prune(rows: Vec<Entry>, installed: &[String]) -> Vec<Entry> {
    let has = |m: &str| installed.iter().any(|i| crate::map_library::same_name(i, m));
    let missing = |id: &str| MISSING_AREAS.contains(&id);
    let kept: Vec<Entry> = rows
        .into_iter()
        .filter_map(|mut e| {
            if e.challenge.as_ref().is_some_and(|c| !has(&c.map) || missing(&c.id) || SKATE_2_ONE_UPS.contains(&c.id.as_str()))
                || e.location.as_ref().is_some_and(|l| !has(&l.map) || missing(&l.id))
            {
                return None;
            }
            // A start that would put NaN or a non-affine transform into travel_to falls back to the row's position.
            let spawns = e.challenge.as_mut().map(|c| &mut c.spawn).into_iter();
            for spawn in spawns.chain(e.location.as_mut().map(|l| &mut l.spawn)) {
                if spawn.start.as_ref().is_some_and(|s| !s.valid()) {
                    spawn.start = None;
                }
                // A polygon holding none of the row's spawns (Skate 2's freeskate_training sits 2.5 km from the
                // Training Facility) would warn and send the skater back from the moment it lands: no boundary then.
                if let (Some(start), Some(polygon)) = (&spawn.start, &spawn.boundary)
                    && polygon.len() >= 3
                {
                    let spawns = if start.slots.is_empty() { std::slice::from_ref(&start.matrix) } else { &start.slots[..] };
                    if !spawns.iter().any(|m| crate::online_boundary::inside(polygon, [m[3][0], m[3][2]])) {
                        spawn.boundary = None;
                    }
                }
            }
            if e.folder {
                e.rows = prune(e.rows, installed);
                if count(&e.rows) == 0 {
                    return None;
                }
            }
            Some(e)
        })
        .collect();
    bare_headers_dropped(kept)
}

/// A header stays only while a non-header row follows it before the next header.
fn bare_headers_dropped(kept: Vec<Entry>) -> Vec<Entry> {
    let mut out = Vec::with_capacity(kept.len());
    for (i, e) in kept.iter().enumerate() {
        if e.header && !kept.get(i + 1).is_some_and(|n| !n.header) {
            continue;
        }
        out.push(e.clone());
    }
    out
}

/// The headers retail shows only outside a session (their Lua: `GetOnlineMode()==NotInSession`).
const NOT_IN_SESSION_HEADERS: [&str; 5] = [
    "ID_MAP_HEADER_TEAM_OPTIONS",
    "ID_MAP_HEADER_VERSUS_OPTIONS",
    "ID_MAP_COMMAND_FREESKATE_OPTIONS",
    "ID_MAP_HEADER_FIND_A_MATCH",
    "ID_MAP_COMMAND_RANKED_VERSUS",
];

/// Retail's map in a session (user 10-27: Quick Match in a session put them in a 1-Up on the map they stood on): every
/// command row (each Quick Match, a type folder's too, Custom Match, Join Team Lobby, Join Team Lobby Here, Solo
/// Freeskate Here) and the Options / Find a Match / Versus: Ranked headers are `GetOnlineMode()==NotInSession` in
/// skatercollections' map Lua, so they are gone, not greyed (.superpowers/sdd/2026-10-27-preview-list/retail-report.md
/// Q3); challenge and location rows stay (their info panel moves or proposes: online_map_runtime).
fn in_session(rows: Vec<Entry>) -> Vec<Entry> {
    let kept = (rows.into_iter())
        .filter(|e| e.command.is_none() && !(e.header && NOT_IN_SESSION_HEADERS.contains(&e.name.as_str())))
        .map(|mut e| {
            if e.folder {
                e.rows = in_session(e.rows);
            }
            e
        })
        .collect();
    bare_headers_dropped(kept)
}

/// Skate 3's session-marker rules by challenge key: online.json's `markers` (tools/online_challenge_map.py
/// marker_flags), each challenge_local_data record's [AllowSessionMarkersToBeSet, AllowSessionMarkersToBeUsed,
/// AllowSessionMarkersToBeUsedInWipeout]. Empty when the file predates them (a launcher Rebuild writes them).
pub(crate) fn marker_rules(root: &Path) -> BTreeMap<String, [bool; 3]> {
    #[derive(Deserialize)]
    struct Rules {
        #[serde(default)]
        markers: BTreeMap<String, [bool; 3]>,
    }
    let text = std::fs::read_to_string(root.join("online/online.json")).unwrap_or_default();
    serde_json::from_str::<Rules>(&text).map_or_else(|_| BTreeMap::new(), |r| r.markers)
}

impl OnlineMap {
    pub fn from_json(json: &str, installed: &[String]) -> Result<Self, String> {
        Self::from_file(serde_json::from_str(json).map_err(|e| e.to_string())?, installed)
    }

    fn from_file(file: File, installed: &[String]) -> Result<Self, String> {
        if file.format != "skate3-online-map" || file.version != 1 {
            return Err(format!("unsupported online map {} v{}", file.format, file.version));
        }
        let mut icons = BTreeMap::new();
        for (name, v) in file.icons {
            if let Ok(t) = serde_json::from_value::<Texture>(v) {
                // The movie's ImageRenderer_Load lowercases paths (DLC_Maloof_Map_512.rx2).
                icons.insert(name.to_ascii_lowercase(), t);
            }
        }
        let tabs = file.tabs.into_iter().map(|t| Tab { name: t.name, rows: prune(t.rows, installed) }).map(teleport_spot_names).collect();
        Ok(Self { tabs, icons })
    }

    pub fn load(root: &Path, installed: &[String]) -> Result<Self, String> {
        let path = root.join("online/online.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut file: File = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        // Icons may be paths (relative to `root`) to texture JSON; inline them.
        for v in file.icons.values_mut() {
            if let Some(rel) = v.as_str() {
                let json = std::fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
                *v = serde_json::from_str(&json).map_err(|e| format!("{rel}: {e}"))?;
            }
        }
        Self::from_file(file, installed)
    }

    /// The map as it shows in a session ([`in_session`]).
    pub fn in_session(&self) -> Self {
        let tabs = self.tabs.iter().map(|t| Tab { name: t.name.clone(), rows: in_session(t.rows.clone()) }).collect();
        Self { tabs, icons: self.icons.clone() }
    }

    pub fn list<'a>(&'a self, nav: &Nav) -> &'a [Entry] {
        let Some(tab) = self.tabs.get(nav.tab) else { return &[] };
        match nav.open.and_then(|i| tab.rows.get(i)) {
            Some(folder) => &folder.rows,
            None => &tab.rows,
        }
    }

    /// The challenge (by id) or location (by string id) row on `tab`, as a Create Private pick; None when this
    /// player's map does not have it (an uninstalled map pruned it).
    pub fn find(&self, tab: usize, id: &str) -> Option<Pick> {
        let pick = |e: &Entry, folder: &str| -> Option<Pick> {
            let choice = Choice::CreatePrivate;
            match (&e.challenge, &e.location) {
                (Some(c), _) if c.id == id => {
                    Some(Pick::Challenge { challenge: c.clone(), name: e.name.clone(), choice, tab, folder: folder.into() })
                }
                (None, Some(l)) if e.name == id => Some(Pick::Location { location: l.clone(), tab, name: e.name.clone(), choice }),
                _ => None,
            }
        };
        let rows = &self.tabs.get(tab)?.rows;
        rows.iter().find_map(|e| pick(e, "").or_else(|| e.rows.iter().find_map(|r| pick(r, &e.name))))
    }

    /// The spawn of the row a session names (user 10-29): its Online Freeskate `location` (a location row's string
    /// id, on any tab), else its challenge `row`; None when this player's map does not have it.
    pub fn session_spawn(
        &self,
        location: Option<&str>,
        row: Option<(usize, &str)>,
        destinations: &[crate::teleport_menu::Destination],
    ) -> Option<Spawn> {
        fn walk<'a>(rows: &'a [Entry], name: &str) -> Option<&'a Location> {
            rows.iter().find_map(|e| e.location.as_ref().filter(|_| e.name == name).or_else(|| walk(&e.rows, name)))
        }
        if let Some(name) = location {
            return self.tabs.iter().find_map(|t| walk(&t.rows, name)).map(|l| l.spawn_at(destinations));
        }
        match self.find(row?.0, row?.1)? {
            Pick::Challenge { challenge, .. } => Some(challenge.spawn),
            Pick::Location { location, .. } => Some(location.spawn_at(destinations)),
            Pick::Command { .. } => None,
        }
    }
    pub fn label(&self, entry: &Entry, localize: impl Fn(&str) -> String) -> String {
        let name = localize(&entry.name);
        if entry.folder { format!("{name} ({})", count(&entry.rows)) } else { name }
    }
}

impl Nav {
    pub fn first_selectable(list: &[Entry]) -> usize {
        list.iter().position(|e| !e.header).unwrap_or(0)
    }

    pub fn step(&mut self, map: &OnlineMap, down: bool) -> bool {
        let list = map.list(self);
        let next = if down {
            (self.highlighted + 1..list.len()).find(|&i| !list[i].header)
        } else {
            (0..self.highlighted.min(list.len())).rev().find(|&i| !list[i].header)
        };
        match next {
            Some(i) => {
                self.highlighted = i;
                true
            }
            None => false,
        }
    }

    pub fn tab(&mut self, map: &OnlineMap, right: bool) -> bool {
        let next = if right { self.tab + 1 } else { self.tab.wrapping_sub(1) };
        if next >= map.tabs.len() {
            return false;
        }
        *self = Nav { tab: next, open: None, highlighted: Nav::first_selectable(&map.tabs[next].rows) };
        true
    }

    pub fn back(&mut self) -> bool {
        match self.open.take() {
            Some(i) => {
                self.highlighted = i;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #3: online.json's marker rules load by key; a file without them (before a Rebuild), or none, loads empty.
    #[test]
    fn marker_rules_load_by_key_and_default_to_none() {
        let dir = std::env::temp_dir().join(format!("s3r-marker-rules-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("online")).unwrap();
        assert!(marker_rules(&dir).is_empty());
        std::fs::write(dir.join("online/online.json"), r#"{"format":"skate3-online-map","version":1,"tabs":[]}"#).unwrap();
        assert!(marker_rules(&dir).is_empty());
        std::fs::write(dir.join("online/online.json"), r#"{"tabs":[],"markers":{"film_dwtn_01":[true,true,false]}}"#)
            .unwrap();
        assert_eq!(marker_rules(&dir).get("film_dwtn_01"), Some(&[true, true, false]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const SAMPLE: &str = r#"{"format":"skate3-online-map","version":1,"icons":{},"tabs":[
 {"name":"T","rows":[{"name":"H","header":true},{"name":"Join","command":"online_freeskate_team_join"},
  {"name":"1-Up","folder":true,"rows":[{"name":"H","header":true},{"name":"Quick","command":"q"},{"name":"1-Up","header":true},
   {"name":"A","challenge":{"id":"a","map":"DownTown","description":"d","objectives":[],"bonus":0,"icon":"mini_1up"}},
   {"name":"B","challenge":{"id":"b","map":"Sanatorium","description":"d","objectives":[],"bonus":0,"icon":"mini_1up"}}]},
  {"name":"Dom","folder":true,"rows":[{"name":"Dom","header":true},
   {"name":"C","challenge":{"id":"c","map":"Sanatorium","description":"d","objectives":[],"bonus":0,"icon":"mini_domination"}}]}]},
 {"name":"U","rows":[{"name":"Q","command":"q2"}]}]}"#;

    /// User 10-29: a guest that joined by invite spawned on the map's single-player spawn: the session's location row
    /// (any tab, in a folder too) or challenge row gives the spawn instead.
    #[test]
    fn a_sessions_row_gives_its_spawn() {
        let start = |x: f32| format!(r#"{{"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[{x},0,0,1]]}}"#);
        let json = format!(
            r##"{{"format":"skate3-online-map","version":1,"tabs":[
            {{"name":"T","rows":[{{"name":"A","challenge":{{"id":"a","map":"DownTown","description":"d","objectives":[],"bonus":0,"icon":"mini_1up","start":{a}}}}}]}},
            {{"name":"S2","rows":[{{"name":"Spots","folder":true,"rows":[{{"name":"#Mega Compound","location":{{"map":"DownTown","start":{m}}}}}]}}]}}]}}"##,
            a = start(1.),
            m = start(7.)
        );
        let map = OnlineMap::from_json(&json, &["DownTown".into()]).unwrap();
        let x = |s: Option<Spawn>| s.and_then(|s| s.start).map(|s| s.matrix[3][0]);
        assert_eq!(x(map.session_spawn(Some("#Mega Compound"), None, &[])), Some(7.));
        assert_eq!(x(map.session_spawn(None, Some((0, "a")), &[])), Some(1.));
        assert_eq!(x(map.session_spawn(Some("#Elsewhere"), Some((0, "a")), &[])), None, "a location this map lacks");
        assert_eq!(x(map.session_spawn(None, None, &[])), None);
    }

    /// User 10-30: online at Skate 1's X Games you spawned over nothing and fell: the Skate 1 map has no X Games venue
    /// or Plan B, so their rows are left out (and an emptied folder with them); the city's stay. User 10-31: Mini Mega
    /// too (its ramp isn't in the map: no ground under its spawns), in Party Play as online.
    #[test]
    fn rows_in_areas_the_skate1_map_lacks_are_left_out() {
        let row = |id: &str, location: bool| {
            if location {
                format!(r#"{{"name":"{id}","location":{{"id":"{id}","map":"Skate_1_San_Vanelona"}}}}"#)
            } else {
                format!(r#"{{"name":"{id}","challenge":{{"id":"{id}","map":"Skate_1_San_Vanelona","description":"d","objectives":[],"bonus":0,"icon":"i"}}}}"#)
            }
        };
        let json = format!(
            r##"{{"format":"skate3-online-map","version":1,"tabs":[{{"name":"#Skate 1","rows":[{},{},{},{{"name":"Jam","folder":true,"rows":[{},{}]}},{{"name":"ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE","folder":true,"rows":[{},{}]}}]}}]}}"##,
            row("freeskate_financial_xgames", true),
            row("freeskate_planb", true),
            row("freeskate_sanvanelona_zen_plaza", true),
            row("jam_financial_xgames", false),
            row("jam_oldtown_planb", false),
            row("ol_spotbattle_urbanres_minimega", false),
            row("ol_spotbattle_oldtown_warehouststeps", false),
        );
        let map = OnlineMap::from_json(&json, &["Skate_1_San_Vanelona".into()]).unwrap();
        let names: Vec<&str> = map.tabs[0].rows.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["freeskate_sanvanelona_zen_plaza", "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE"]);
        let spots: Vec<&str> = map.tabs[0].rows[1].rows.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(spots, ["ol_spotbattle_oldtown_warehouststeps"], "Mini Mega left out");
        // Party Play's places come from these tabs: none of the left-out rows reach it.
        let listed: Vec<String> = crate::party_play::places(&map).into_iter().flatten().map(|p| p.challenge.id).collect();
        assert_eq!(listed, ["ol_spotbattle_oldtown_warehouststeps"], "Party Play lists none of them");
    }

    /// User 10-29: an added Teleport Spot named like a retail location sent everyone to the retail one, and stacked
    /// everyone on one spot (no start). It gets its own name, and its teleport's spot as its start.
    #[test]
    fn a_teleport_spot_is_its_own_row_with_its_own_slots() {
        let json = r##"{"format":"skate3-online-map","version":1,"tabs":[{"name":"S2","rows":[{"name":"F","folder":true,"rows":[
            {"name":"#Mega Compound","location":{"map":"DownTown","start":{"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[7,0,0,1]]}}},
            {"name":"#Mega Compound","location":{"map":"DownTown","teleport":"spot_mega"}},
            {"name":"#Only A Spot","location":{"map":"DownTown","teleport":"spot_only"}}]}]}]}"##;
        let map = OnlineMap::from_json(json, &["DownTown".into()]).unwrap();
        let names: Vec<&str> = map.tabs[0].rows[0].rows.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["#Mega Compound", "#Mega Compound (Teleport Spot)", "#Only A Spot"]);
        let Some(Pick::Location { location, .. }) = map.find(0, "#Mega Compound (Teleport Spot)") else { panic!("its own row") };
        assert_eq!(location.teleport.as_deref(), Some("spot_mega"));
        let at = |x: f32| Some([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]]);
        let spot = crate::teleport_menu::Destination { id: "spot_mega".into(), name: String::new(), map: "DownTown".into(), matrix: at(3.), unavailable_reason: None };
        let start = location.spawn_at(std::slice::from_ref(&spot)).start.unwrap();
        assert_eq!((start.slot(0)[3], start.slot(1)[3]), ([3., 0., 0., 1.], [3., 0., -5., 1.]), "a slot each, 5 m apart");
        assert!(location.spawn_at(&[]).start.is_none(), "an unknown teleport: as before");
    }
    #[test]
    fn uninstalled_maps_drop_rows_and_empty_folders() {
        let map = OnlineMap::from_json(SAMPLE, &["DownTown".into()]).unwrap();
        let root = &map.tabs[0].rows;
        assert_eq!(root.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["H", "Join", "1-Up"], "Dom had only Sanatorium");
        assert_eq!(map.label(&root[2], |s| s.to_string()), "1-Up (1)");
    }
    /// User 10-27 (retail's map Lua): in a session the commands and the Options / Ranked headers are gone, a type
    /// folder opens on its challenges, and the rest stays.
    #[test]
    fn a_session_hides_every_command_and_its_headers() {
        let challenge = |id: &str| format!(r#"{{"name":"{id}","challenge":{{"id":"{id}","map":"DownTown","description":"d","objectives":[],"bonus":0,"icon":"mini_1up"}}}}"#);
        let json = format!(
            r#"{{"format":"skate3-online-map","version":1,"tabs":[
            {{"name":"ID_MAP_FILTER_VERSUS","rows":[{{"name":"ID_MAP_COMMAND_RANKED_VERSUS","header":true}},
              {{"name":"ID_MAP_COMMAND_QUICKMATCH","command":"online_versus_ranked_quickmatch"}},
              {{"name":"ID_MAP_COMMAND_CUSTOMMATCH","command":"online_versus_ranked_custommatch"}},
              {{"name":"ID_MAP_COMMAND_UNRANKED_VERSUS","header":true}},
              {{"name":"ID_MAP_COMMAND_QUICKMATCH","command":"online_versus_unranked_allchallengequickmatch"}},
              {{"name":"ID_MAP_FILTER_CHALLENGETYPES_1UP","folder":true,"rows":[{{"name":"ID_MAP_HEADER_VERSUS_OPTIONS","header":true}},
                {{"name":"ID_MAP_COMMAND_QUICKMATCH","command":"online_versus_unranked_challengetypequickmatch"}},
                {{"name":"ID_MAP_FILTER_CHALLENGETYPES_1UP","header":true}},{a}]}}]}},
            {{"name":"ID_FREESKATEOPTION_START","rows":[{{"name":"ID_MAP_COMMAND_FREESKATE_OPTIONS","header":true}},
              {{"name":"ID_MAP_COMMAND_JOIN_TEAM_FREESKATE","command":"online_freeskate_team_join"}},
              {{"name":"ID_MAP_COMMAND_SOLO_FREESKATE_HERE","command":"online_freeskate_here_solo"}},
              {{"name":"ID_MAP_COMMAND_FREESKATE_LOCATIONS","header":true}},
              {{"name":"ID_DISTRICT_DOWNTOWN","folder":true,"rows":[{b}]}}]}}]}}"#,
            a = challenge("a"),
            b = challenge("b"),
        );
        let out = OnlineMap::from_json(&json, &["DownTown".into()]).unwrap();
        let session = out.in_session();
        let names = |rows: &[Entry]| rows.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&session.tabs[0].rows), ["ID_MAP_COMMAND_UNRANKED_VERSUS", "ID_MAP_FILTER_CHALLENGETYPES_1UP"], "Versus: Unranked, then the folders");
        assert_eq!(names(&session.tabs[0].rows[1].rows), ["ID_MAP_FILTER_CHALLENGETYPES_1UP", "a"], "1-Up opens on its challenges: no Quick Match");
        assert_eq!(names(&session.tabs[1].rows), ["ID_MAP_COMMAND_FREESKATE_LOCATIONS", "ID_DISTRICT_DOWNTOWN"]);
        assert_eq!(names(&out.tabs[0].rows).len(), 6, "outside a session, all of it");
        assert_eq!(Nav::first_selectable(&session.tabs[0].rows), 1);
    }

    #[test]
    fn moves_find_their_row_on_each_players_map() {
        let map = OnlineMap::from_json(SAMPLE, &["DownTown".into(), "Sanatorium".into()]).unwrap();
        let Some(Pick::Challenge { challenge, name, choice, folder, .. }) = map.find(0, "c") else { panic!("c is in Dom") };
        assert_eq!((challenge.id.as_str(), name.as_str(), choice, folder.as_str()), ("c", "C", Choice::CreatePrivate, "Dom"));
        let target = map.find(0, "c").unwrap().target().unwrap();
        assert_eq!((target.tab, target.id.as_str(), target.map.as_str(), target.name.as_str()), (0, "c", "Sanatorium", "C"));
        assert!(map.find(1, "c").is_none(), "another tab");
        let downtown = OnlineMap::from_json(SAMPLE, &["DownTown".into()]).unwrap();
        assert!(downtown.find(0, "c").is_none(), "not installed here");
    }

    #[test]
    fn spawn_fields_load_and_pick_the_lobby_slot() {
        let old = &map_of(SAMPLE).tabs[0].rows[2].rows[3];
        assert!(old.challenge.as_ref().unwrap().spawn.start.is_none(), "online.json without spawns still loads");
        let json = r#"{"format":"skate3-online-map","version":1,"tabs":[{"name":"T","rows":[
          {"name":"A","challenge":{"id":"a","map":"DownTown","boundary":[[0,0],[10,0],[0,10]],"oob_time":3,
            "start":{"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[5,6,7,1]],"slots":[[[0,0,-1,0],[0,1,0,0],[1,0,0,0],[1,2,3,1]],
                                                                          [[1,0,0,0],[0,1,0,0],[0,0,1,0],[4,5,6,1]]]}}},
          {"name":"L","location":{"map":"DownTown","teleport":null,"start":{"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[8,9,10,1]],"slots":[]},
            "boundary":null,"oob_time":null}}]}]}"#;
        let map = map_of(json);
        let spawn = &map.tabs[0].rows[0].challenge.as_ref().unwrap().spawn;
        let start = spawn.start.as_ref().unwrap();
        assert_eq!(start.slot(0)[3], [1., 2., 3., 1.]);
        assert_eq!(start.slot(0)[0], [0., 0., -1., 0.], "heading kept");
        assert_eq!(start.slot(1)[3], [4., 5., 6., 1.]);
        // Past the children, the resolver (S2 0x82879950 / TU3 0x82910eb0) moves child 0 by -5 m in z per slot.
        assert_eq!(start.slot(3)[3], [1., 2., -12., 1.], "slot 3 of 2");
        assert_eq!(start.slot(3)[0], [0., 0., -1., 0.], "child 0's heading");
        assert_eq!((spawn.boundary.as_ref().unwrap().len(), spawn.oob_time), (3, Some(3.)));
        let location = &map.tabs[0].rows[1].location.as_ref().unwrap().spawn;
        let start = location.start.as_ref().unwrap();
        assert_eq!((start.slot(0)[3], start.slot(2)[3]), ([8., 9., 10., 1.], [8., 9., 0., 1.]), "no slots: the locator, 5 m apart");
        assert!(location.boundary.is_none());
        let bad = json.replace("[[0,0,-1,0],[0,1,0,0],[1,0,0,0],[1,2,3,1]]", "[[0,0,-1,0],[0,1,0,0],[1,0,0,0],[1,2,3,0]]");
        let map = map_of(&bad);
        let spawn = &map.tabs[0].rows[0].challenge.as_ref().unwrap().spawn;
        assert!(spawn.start.is_none(), "an invalid slot drops the whole start");
        assert!(spawn.boundary.is_some());
    }

    #[test]
    fn a_boundary_around_none_of_the_spawns_is_dropped() {
        let row = |boundary: &str| {
            format!(
                r#"{{"format":"skate3-online-map","version":1,"tabs":[{{"name":"T","rows":[
                  {{"name":"L","location":{{"map":"DownTown","boundary":{boundary},
                    "start":{{"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[50,0,50,1]],"slots":[[[1,0,0,0],[0,1,0,0],[0,0,1,0],[1,0,3,1]]]}}}}}}]}}]}}"#
            )
        };
        let boundary = |json: &str| map_of(json).tabs[0].rows[0].location.as_ref().unwrap().spawn.boundary.clone();
        assert!(boundary(&row("[[0,0],[10,0],[0,10]]")).is_some(), "the slot is inside; the locator outside does not count");
        assert!(boundary(&row("[[100,100],[110,100],[100,110]]")).is_none(), "nowhere near the spawns");
    }

    fn map_of(json: &str) -> OnlineMap {
        OnlineMap::from_json(json, &["DownTown".into(), "Sanatorium".into()]).unwrap()
    }

    #[test]
    fn navigation_skips_headers_and_stops_at_ends() {
        let map = OnlineMap::from_json(SAMPLE, &["DownTown".into(), "Sanatorium".into()]).unwrap();
        let mut nav = Nav { tab: 0, open: None, highlighted: Nav::first_selectable(&map.tabs[0].rows) };
        assert_eq!(nav.highlighted, 1, "header skipped");
        assert!(nav.step(&map, true));
        assert!(nav.step(&map, true));
        assert!(!nav.step(&map, true), "stops at last");
        assert!(!nav.tab(&map, false), "LT on the first tab stays");
        assert!(nav.tab(&map, true));
        assert!(!nav.tab(&map, true), "RT on the last tab stays");
        assert_eq!(nav.highlighted, 0);
        assert_eq!(nav.open, None);
        nav.tab(&map, false);
        nav.open = Some(2);
        nav.highlighted = Nav::first_selectable(map.list(&nav));
        assert_eq!(nav.highlighted, 1, "Quick Match under the Options header");
        assert!(nav.back());
        assert_eq!(nav.open, None);
        assert_eq!(nav.highlighted, 2, "back on the folder row");
        assert!(!nav.back(), "tab root: close");
    }
}
