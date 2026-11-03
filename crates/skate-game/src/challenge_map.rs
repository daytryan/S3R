//! Original Challenge Map screen (Locations tab), opened from the Start menu over
//! the paused game. Rows are the prepared teleport catalog grouped as the retail
//! Locations list; the Start menu performs the travel the player picks.
use crate::{
    apt_canvas::{Canvas, movie_textures},
    challenge_map_runtime::{Outcome, Place, Row, Runtime},
    config::Config,
    core_menu_runtime::{Booted, Key},
    scoring_hud::{HudComposite, HudMaterial},
    teleport_menu::Destination,
};
use bevy::{prelude::*, window::PrimaryWindow};
use crate::online_map::{OnlineMap, Pick};
use crate::online_map_runtime::{OnlineOutcome, OnlineRuntime};
use crate::apt_scene::Draw;
use std::path::PathBuf;

/// The open screen: the Locations map or the online challenge map.
enum Screen {
    Offline(Runtime),
    Online(OnlineRuntime),
}
impl Screen {
    fn advance(&mut self) -> Result<(), String> {
        match self { Self::Offline(r) => r.advance(), Self::Online(r) => r.advance() }
    }
    fn show(&mut self) -> Result<(), String> {
        match self { Self::Offline(r) => r.show(), Self::Online(r) => r.show() }
    }
    fn close(&mut self) -> Result<(), String> {
        match self { Self::Offline(r) => r.close(), Self::Online(r) => r.close() }
    }
    fn draws(&self) -> Result<Vec<Draw>, String> {
        match self { Self::Offline(r) => r.draws(), Self::Online(r) => r.draws() }
    }
    fn intro_complete(&self) -> bool {
        match self { Self::Offline(r) => r.bindings.intro_complete, Self::Online(r) => r.intro_complete() }
    }
    fn outro_complete(&self) -> bool {
        match self { Self::Offline(r) => r.bindings.outro_complete, Self::Online(r) => r.outro_complete() }
    }
    fn sounds_mut(&mut self) -> &mut Vec<u64> {
        match self { Self::Offline(r) => &mut r.sounds, Self::Online(r) => &mut r.sounds }
    }
}

const LAYER: usize = 33;
/// The movie is authored at 16 ms per frame.
const FRAME: f32 = 1. / 60.;
/// Outro fallback if the movie never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;
/// Retail Locations rows in order (PS3, BLUS30464): name, helper text, own map
/// image (None: the Port Carverton overview), maps. skate.Park lists PARKS.
/// Then locally converted content, shown only when its maps are installed: the
/// Skate 2 city and its online DLC parks (RPCS3 New San Van mod), and the Xbox DLC.
/// DLC strings live in the DLC's own language tables, so they are literals here.
const AREAS: [(&str, &str, Option<&str>, &[&str]); 13] = [
    ("ID_DISTRICT_DOWNTOWN", "ID_MAP_HELPER_LOCATION_DOWNTOWN", None, &["DownTown"]),
    ("ID_DISTRICT_INDUSTRY", "ID_MAP_HELPER_LOCATION_INDUSTRIAL", None, &["Industrial"]),
    ("ID_DISTRICT_UNIVERSITY", "ID_MAP_HELPER_LOCATION_UNIVERSITY", None, &["University"]),
    ("ID_LOCATION_SKATESCHOOL", "ID_MAP_HELPER_LOCATION_SKATE_SCHOOL", Some("map_skateschool.rx2"), &["SkateSchool"]),
    ("ID_MAP_FILTER_SKATE_PARK", "ID_MAP_HELPER_LOCATION_SKATE_PARKS", None, &[]),
    ("ID_CHALLENGE_FREESKATE_DLC_MALOOF_TITLE", "ID_MAP_HELPER_LOCATION_MALOOF", None, &["MaloofMoneyCup"]),
    ("ID_LOCATION_BLACKBOXPARK", "ID_MAP_HELPER_LOCATION_BLACKBOXPARK", None, &["BlackBoxPark"]),
    ("ID_COMMON_NEW_SAN_VANELONA", "ID_STUFFMENU_CATEGORY_OTHERLOCATIONS_RETURNTOSANVAN", None,
        &["New_San_Vanelona", "Skate_2_Center", "Skate_2_Factory", "Skate_2_Maloof", "Skate_2_Parkade", "Skate_2_School"]),
    // The Xbox DLC places show their own map pictures (data/fe/source/images/map in each DLC
    // package, installed into the prepared screen by dlc_location_image.py).
    ("#Maloof Money Cup 2010 NYC", "#Shred the Maloof Money Cup NYC course and surrounding spots.", Some("dlc_maloof_map_512.rx2"),
        &["Maloof_Money_Cup_NYC"]),
    ("#Danny Way's Hawaiian Dream", "#Killer street, monstrous tranny. This is Danny Way's Hawaiian Dream Compound.",
        Some("map_danny.rx2"), &["Danny_Way_Hawaiian_Dream"]),
    ("#Art Gallery", "#Take a trip back to the San Van and skate the Art Gallery, Regs and the new Marraffa Skatepark",
        Some("map_partypack.rx2"), &["Art_Gallery"]),
    // After Dark: ID_LOCATION_SANITARIUMDLC / ID_LOCATION_HELPER_SANITARIUMDLC.
    ("#The Sanatorium", "#The chilling Sanatorium and the notorious DIY skatepark, but you can only visit after dark.",
        Some("map_afterdark.rx2"), &["Sanatorium"]),
    // PS3 custom map (CUSTOMM000000000): the original skate. city.
    ("#Skate 1 San Vanelona", "#The original San Vanelona: Old Town, the Res, Financial, the Suburbs and the Outskirts.", None,
        &["Skate_1_San_Vanelona"]),
];
const SKATE_PARK: &str = "ID_MAP_FILTER_SKATE_PARK";
/// skate.Park rows (header, park, map, layout picture) for the parks this engine
/// loads, in retail order, then the converted DLC parks (Zen Plaza from the San Van Party
/// Pack, Back Lot and Downtown Night from Skate.Create) with their packages' own
/// screen_*_empty pictures, installed by dlc_location_image.py.
/// ponytail: PCU Training - Full needs a map package.
const PARKS: [(&str, &str, &str, Option<&str>); 7] = [
    ("ID_WORLD_SKATEPARK_DOWNTOWN", "ID_WORLD_SKATEPARK_DOWNTOWN_FULL", "DownTownSkatePark", Some("map_skatepark_downtown_full.rx2")),
    ("ID_WORLD_SKATEPARK_STARTPARK", "ID_WORLD_SKATEPARK_STARTPARK_PROG", "StartPark", Some("map_skatepark_basic.rx2")),
    ("ID_WORLD_SKATEPARK_INDUSTRIAL", "ID_WORLD_SKATEPARK_INDUSTRIAL_FULL", "IndustrialSkatePark", Some("map_skatepark_industrial_full.rx2")),
    ("ID_WORLD_SKATEPARK_MEGAPARK", "ID_WORLD_SKATEPARK_MEGAPARK_FULL", "MegaPark", Some("map_skatepark_stadium_full.rx2")),
    ("#Zen Plaza", "#Zen Plaza - Empty", "Zen_Plaza", Some("screen_zen_park_empty.rx2")),
    ("#Back Lot Park", "#Back Lot Park - Empty", "Back_Lot_Park", Some("screen_backlot_park_empty.rx2")),
    ("#Downtown Night Park", "#Downtown Night Park - Empty", "Downtown_Night_Park", Some("screen_dtnight_park_empty.rx2")),
];
/// The place name the Locations list shows for `map` (a string id, or `#` literal text); None for an unlisted map.
pub(crate) fn place_name(map: &str) -> Option<&'static str> {
    let same = |m: &str| crate::map_library::same_name(m, map);
    AREAS.iter().find(|a| a.3.iter().any(|m| same(m))).map(|a| a.0).or_else(|| PARKS.iter().find(|p| same(p.2)).map(|p| p.0))
}
/// Where the native map draws each location's marker, in texels of the place's
/// map (map1024, or map_skateschool), measured from the PS3 retail screen in RPCS3.
pub(crate) const MARKERS: [(&str, [f32; 2]); 44] = [
    ("dist_downtown_aletown", [403.1, 536.7]),
    ("dist_downtown_mall", [387.4, 526.7]),
    ("dist_downtown_memorial", [390.3, 584.6]),
    ("dist_downtown_coreplaza", [444.6, 451.7]),
    ("dist_downtown_hotelplaza", [486.7, 526.7]),
    ("dist_downtown_cubetower", [486.7, 425.3]),
    ("dist_downtown_midtownpromenade", [415.3, 455.3]),
    ("dist_downtown_parkade", [501.7, 436.7]),
    ("dist_downtown_canalplaza", [411.7, 403.1]),
    ("dist_downtown_resplaza", [461.0, 507.4]),
    ("dist_downtown_skatepark", [390.3, 451.7]),
    ("dist_downtown_carlot", [464.6, 528.9]),
    ("dist_downtown_tresoutta", [418.1, 502.4]),
    ("dist_downtown_spillway", [456.0, 404.6]),
    ("dist_industrial_2ndnnavy", [561.0, 717.4]),
    ("dist_industrial_quarry", [666.0, 682.4]),
    ("dist_industrial_drydocks", [466.0, 658.9]),
    ("dist_industrial_newfactoryroof", [562.4, 623.1]),
    ("dist_industrial_ghettospot", [496.7, 651.7]),
    ("dist_industrial_reclaimed", [580.3, 679.6]),
    ("dist_industrial_shiploadingdocks", [508.1, 686.0]),
    ("dist_industrial_spillwaybottom", [531.7, 624.6]),
    ("dist_industrial_newfactory", [590.3, 633.1]),
    ("dist_industrial_spillway", [750.3, 628.9]),
    ("dist_industrial_oldfactory", [616.7, 703.9]),
    ("dist_industrial_tankership", [511.7, 710.3]),
    ("dist_university_campusentrance", [614.6, 343.9]),
    ("dist_university_campusstart", [715.3, 311.7]),
    ("dist_university_clocktower", [668.9, 330.3]),
    ("dist_university_lowroad", [605.3, 403.9]),
    ("dist_university_stadium", [693.1, 363.9]),
    ("dist_university_library", [696.0, 290.3]),
    ("dist_university_pavilion", [670.3, 362.4]),
    ("dist_university_practice", [704.6, 232.4]),
    ("dist_university_communityskatepark", [701.7, 454.6]),
    ("dist_university_observatory", [524.6, 211.0]),
    ("dist_maloof_street", [390.3, 473.1]),
    ("dist_maloof_vert", [390.3, 472.4]),
    ("dist_blackboxpark", [393.9, 461.7]),
    ("dist_skateschool", [260.1, 198.8]),
    // map_afterdark, measured from an Xbox 360 retail capture of the Locations tab.
    ("sanatorium_z_sanitarium_start", [217.3, 326.8]),
    // The other DLC pictures: the teleport locator through a world -> picture fit, found by
    // correlating each picture's outlines with a top-down render of its level's geometry
    // (checked on map_afterdark: outlines coincide; 12 texels from its retail marker, the
    // districts' usual locator-to-marker offset).
    ("maloof_money_cup_nyc_z_maloofmcnyc_start", [279.2, 292.6]),
    ("danny_way_hawaiian_dream_freeskate_dannyway_megacompound_locator", [164.9, 266.7]),
    ("art_gallery_freeskate_artgallery_locator", [160.4, 214.4]),
];
/// Retail location details (photo, description) opening the info panel before
/// Teleport: data/fe/source/images/locations/<photo>.rx2 and ID_MAP_LOCATION_DESC_*.
/// After Dark: DLC_location_after_dark.rx2, ID_LOCATION_DESC_SANITARIUMDLC.
const DETAILS: [(&str, &str, &str); 41] = [
    ("dist_downtown_aletown", "aletown.rx2", "ID_MAP_LOCATION_DESC_DT_ALETOWN"),
    ("dist_downtown_mall", "allmart.rx2", "ID_MAP_LOCATION_DESC_DT_ALLMART"),
    ("dist_downtown_memorial", "memorial.rx2", "ID_MAP_LOCATION_DESC_DT_CARVERTONMEMORIALPARK"),
    ("dist_downtown_coreplaza", "crystalplaza.rx2", "ID_MAP_LOCATION_DESC_DT_CRYSTALTOWERS"),
    ("dist_downtown_hotelplaza", "hotel.rx2", "ID_MAP_LOCATION_DESC_DT_HOTELDISTRICT"),
    ("dist_downtown_cubetower", "kube.rx2", "ID_MAP_LOCATION_DESC_DT_KUBETOWER"),
    ("dist_downtown_midtownpromenade", "midtown_promenade.rx2", "ID_MAP_LOCATION_DESC_DT_MIDTOWNPROMENADE"),
    ("dist_downtown_parkade", "parkade.rx2", "ID_MAP_LOCATION_DESC_DT_PARKNPLAY"),
    ("dist_downtown_canalplaza", "plaza_of_solitude.rx2", "ID_MAP_LOCATION_DESC_DT_PLAZAOFSOLITUDE"),
    ("dist_downtown_resplaza", "rippon.rx2", "ID_MAP_LOCATION_DESC_DT_RIPPONTOWERS"),
    ("dist_downtown_skatepark", "rosalita.rx2", "ID_MAP_LOCATION_DESC_DT_ROSALITASKATEPARK"),
    ("dist_downtown_carlot", "slappys.rx2", "ID_MAP_LOCATION_DESC_DT_SLAPPYSCARLOT"),
    ("dist_downtown_tresoutta", "tresoutta.rx2", "ID_MAP_LOCATION_DESC_DT_TRESOUTTAPLAZA"),
    ("dist_downtown_spillway", "uptown_promenade.rx2", "ID_MAP_LOCATION_DESC_DT_UPTOWNPROMENADE"),
    ("dist_industrial_2ndnnavy", "2nd_and_navy.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_2NDANDNAVY"),
    ("dist_industrial_quarry", "quarry.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_CARVERTONQUARRY"),
    ("dist_industrial_drydocks", "drydocks.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_DRYDOCKS"),
    ("dist_industrial_newfactoryroof", "factory_roofs.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_FACTORYROOFS"),
    ("dist_industrial_ghettospot", "ghetto.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_GHETTOSPOT"),
    ("dist_industrial_reclaimed", "hastings.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_HAYSTINGSPARK"),
    ("dist_industrial_shiploadingdocks", "loading_docks.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_LOADINGDOCKS"),
    ("dist_industrial_spillwaybottom", "miracle_bowl.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_MIRACLEBOWL"),
    ("dist_industrial_newfactory", "pipeworks.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_PIPEWORKS"),
    ("dist_industrial_spillway", "spillway_entrance.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_SPILLWAYENTRANCE"),
    ("dist_industrial_oldfactory", "old_factory.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_OLDFACTORY"),
    ("dist_industrial_tankership", "tanker.rx2", "ID_MAP_LOCATION_DESC_INDUSTRIAL_TANKER"),
    ("dist_university_campusentrance", "campus_entrance.rx2", "ID_MAP_LOCATION_DESC_UNIV_CAMPUSENTRANCE"),
    ("dist_university_campusstart", "chancenter.rx2", "ID_MAP_LOCATION_DESC_UNIV_CHANCENTER"),
    ("dist_university_clocktower", "clocktower.rx2", "ID_MAP_LOCATION_DESC_UNIV_CLOCKTOWER"),
    ("dist_university_lowroad", "daly_estates.rx2", "ID_MAP_LOCATION_DESC_UNIV_DALYESTATES"),
    ("dist_university_stadium", "stadium.rx2", "ID_MAP_LOCATION_DESC_UNIV_HARTLEYSTADIUM"),
    ("dist_university_library", "library.rx2", "ID_MAP_LOCATION_DESC_UNIV_PCULIBRARY"),
    ("dist_university_pavilion", "peterson_pavilion.rx2", "ID_MAP_LOCATION_DESC_UNIV_PETERSONPAVILION"),
    ("dist_university_practice", "super_ultramega_park.rx2", "ID_MAP_LOCATION_DESC_UNIV_SUPERULTRAMEGAPARK"),
    ("dist_university_communityskatepark", "carvatron.rx2", "ID_MAP_LOCATION_DESC_UNIV_CARVATRON"),
    ("dist_university_observatory", "observatory.rx2", "ID_MAP_LOCATION_DESC_UNIV_OBSERVATORY"),
    ("dist_maloof_street", "maloof.rx2", "ID_MAP_LOCATION_DESC_MALOOF_STREET"),
    ("dist_maloof_vert", "maloof.rx2", "ID_MAP_LOCATION_DESC_MALOOF_VERT"),
    ("dist_blackboxpark", "blackboxpark.rx2", "ID_MAP_LOCATION_DESC_DLC_BLACKBOXPARK"),
    ("dist_skateschool", "skate_school.rx2", "ID_MAP_LOCATION_DESC_SKATESCHOOL"),
    (
        "sanatorium_z_sanitarium_start",
        "dlc_location_after_dark.rx2",
        "#This place is one creepy spot at night, ghostly and abandoned...and yo, guess what else? The DIY park's back! Definitely worth the visit.",
    ),
];

/// Locally converted locations described by data (runtime/locations-extra.json, written by the
/// Skate 1/2 location port): a place's own map picture and pin art, and per-destination map
/// markers and details, on top of the retail tables above.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub(crate) struct Extra {
    /// Place name (AREAS string) -> its map picture and pin.
    #[serde(default)]
    places: std::collections::HashMap<String, ExtraPlace>,
    /// Destination id -> marker centre in texels of its place's map picture.
    #[serde(default)]
    markers: std::collections::HashMap<String, [f32; 2]>,
    /// Destination id -> (photo file, "#description").
    #[serde(default)]
    details: std::collections::HashMap<String, (String, String)>,
}
#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct ExtraPlace {
    pub map: String,
    /// Pin texture (a challengemap thumbnail) and the point of it that marks the spot, as
    /// fractions of its size ([0.5, 1]: a teardrop's tip).
    pub pin: String,
    pub anchor: [f32; 2],
}
impl Extra {
    /// Every level a place's own map picture covers: (level, place).
    pub(crate) fn own_maps(&self) -> Vec<(&'static str, &ExtraPlace)> {
        AREAS.iter().filter_map(|a| Some((a.3, self.places.get(a.0)?))).flat_map(|(maps, p)| maps.iter().map(move |&m| (m, p))).collect()
    }
    pub(crate) fn marker(&self, id: &str) -> Option<[f32; 2]> {
        self.markers.get(id).copied()
    }
    pub(crate) fn load(root: &std::path::Path) -> Result<Self, String> {
        match std::fs::read(root.join("runtime/locations-extra.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("locations-extra.json: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("locations-extra.json: {e}")),
        }
    }
}

/// Where a Locations row goes: a locator on `map`, or the map's spawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spot {
    pub map: String,
    /// Teleport catalog id, for travelling once another map has loaded.
    pub id: Option<String>,
    pub matrix: Option<[[f32; 4]; 4]>,
}

#[derive(Component)]
struct Overlay;

#[derive(Resource)]
pub(crate) struct ChallengeMap {
    source: serde_json::Value,
    /// Loads of the map and the online map for the installed maps they list, copied on open (see [`Booted`]).
    booted: Option<(Vec<String>, Booted<Runtime>)>,
    booted_online: Option<(Vec<String>, Booted<OnlineRuntime>)>,
    canvas: Canvas,
    destinations: Vec<Destination>,
    extra: Extra,
    runtime: Option<Screen>,
    /// online/online.json loaded fine at setup.
    online: bool,
    /// perf 10-18: online/online.json for the installed maps it was pruned to (see [`with_online`]).
    online_cache: std::sync::Mutex<Option<(Vec<String>, OnlineMap)>>,
    root: PathBuf,
    /// Chosen online pick, for the Start menu to act on.
    pub picked_online: Option<Pick>,
    /// Per place, per row (None: a header).
    spots: Vec<Vec<Option<Spot>>>,
    shown: bool,
    closing: Option<f32>,
    accumulated: f32,
    /// Chosen spot, for the Start menu to travel to.
    pub picked: Option<Spot>,
    /// The screen consumed this frame's press; the Start menu must not act on it.
    pub closed_this_frame: bool,
}
impl ChallengeMap {
    pub(crate) fn active(&self) -> bool {
        self.runtime.is_some()
    }
    /// Open over the Start menu; `installed` are the loadable map names.
    pub(crate) fn open(&mut self, installed: &[String]) -> Result<(), String> {
        let (places, spots) = places(&self.destinations, installed, &self.extra);
        self.runtime = Some(Screen::Offline(copy(&mut self.booted, installed, || Runtime::load(&self.source, places))?));
        self.spots = spots;
        self.shown = false;
        self.closing = None;
        self.accumulated = crate::core_menu::opened_accumulator(); // perf 10-18
        Ok(())
    }
    pub(crate) fn online_available(&self) -> bool {
        self.online
    }
    /// Open the retail online challenge map over the Start menu; `session`: Some(hosting) in a session.
    pub(crate) fn open_online(&mut self, installed: &[String], session: Option<bool>) -> Result<(), String> {
        let mut runtime = copy(&mut self.booted_online, installed, || {
            OnlineRuntime::load(&self.source, with_online(&self.online_cache, &self.root, installed, OnlineMap::clone)?, &self.destinations, &self.extra)
        })?;
        runtime.bindings.session = session;
        // User 10-27: in a session, retail's map has no commands (Quick Match and the rest) nor their headers.
        if session.is_some() {
            let map = runtime.bindings.map.in_session();
            let highlighted = map.tabs.first().map_or(0, |t| crate::online_map::Nav::first_selectable(&t.rows));
            runtime.bindings.nav = crate::online_map::Nav { tab: 0, open: None, highlighted };
            runtime.bindings.map = map;
        }
        self.runtime = Some(Screen::Online(runtime));
        self.shown = false;
        self.closing = None;
        self.accumulated = crate::core_menu::opened_accumulator(); // perf 10-18
        Ok(())
    }
    /// online/online.json for `installed` maps (Party Play's location lists).
    pub(crate) fn online_map(&self, installed: &[String]) -> Result<OnlineMap, String> {
        with_online(&self.online_cache, &self.root, installed, OnlineMap::clone)
    }
    /// Skate 3's session-marker rules by challenge key ([`crate::online_map::marker_rules`]).
    pub(crate) fn marker_rules(&self) -> std::collections::BTreeMap<String, [bool; 3]> {
        crate::online_map::marker_rules(&self.root)
    }
    /// A session move's or a proposal's row on this player's own map (None: not installed here).
    pub(crate) fn find_online(&self, installed: &[String], tab: usize, id: &str) -> Option<Pick> {
        with_online(&self.online_cache, &self.root, installed, |map| map.find(tab, id)).ok()?
    }
    /// The proposal tab's card for an online row (None: not installed here). Not retail (user 10-13).
    pub(crate) fn proposal_card(&self, installed: &[String], tab: usize, id: &str) -> Option<Card> {
        let (pick, tab_name) = with_online(&self.online_cache, &self.root, installed, |map| {
            Some((map.find(tab, id)?, map.tabs.get(tab).map_or(String::new(), |t| t.name.clone())))
        })
        .ok()??;
        Some(card(&pick, &tab_name, &self.source, &self.destinations, &self.extra, &self.root))
    }
    /// Close at once without the outro, dropping any pick (a Steam join is taking over).
    pub(crate) fn dismiss(&mut self) {
        self.runtime = None;
        self.closing = None;
        self.picked = None;
        self.picked_online = None;
    }
    fn begin_close(&mut self) {
        if let Some(Err(error)) = self.runtime.as_mut().map(Screen::close) {
            warn!("Challenge Map outro: {error}");
        }
        self.closing = Some(0.);
        self.closed_this_frame = true;
    }
}

/// What the proposal tab shows for a proposed row. Not retail (user 10-13): retail's online poster has no picture
/// and no description (EnableImage(false)).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Card {
    pub title: String,
    /// The row's folder (Spot Battle, ...), else its tab (Free Skate, ...).
    pub mode: String,
    pub place: String,
    pub description: String,
    /// A location photo (absolute .rgba path) and its size.
    pub photo: Option<(String, [u32; 2])>,
}

/// A row's card: the photo of the teleport spot nearest its start on the same map, else the row's own map image,
/// else its place's map picture. ponytail: nearest spot can be ~100 m off; a per-row photo table if that reads wrong.
fn card(pick: &Pick, tab_name: &str, source: &serde_json::Value, destinations: &[Destination], extra: &Extra, root: &std::path::Path) -> Card {
    let language = &source["language"];
    let text = |id: &str| match id.strip_prefix('#') {
        Some(literal) => literal.to_owned(),
        None => language[id].as_str().unwrap_or(id).replace('^', " ").trim().to_owned(),
    };
    let (title, folder, map, teleport, at, own_image, description) = match pick {
        Pick::Challenge { challenge: c, name, folder, .. } => {
            let at = c.spawn.start.as_ref().map(|s| [s.matrix[3][0], s.matrix[3][2]]).or(c.position.map(|p| [p[0], p[2]]));
            let description = Some(text(&c.description)).filter(|d| !d.is_empty());
            (name, folder.as_str(), &c.map, &c.teleport, at, c.map_image.clone(), description)
        }
        Pick::Location { location: l, name, .. } => {
            (name, "", &l.map, &l.teleport, l.spawn.start.as_ref().map(|s| [s.matrix[3][0], s.matrix[3][2]]), None, None)
        }
        Pick::Command { .. } => return Card::default(),
    };
    let detail = |id: &str| DETAILS.iter().find(|d| d.0 == id).map(|d| (d.1.to_owned(), d.2.to_owned())).or_else(|| extra.details.get(id).cloned());
    let nearest = teleport.as_deref().and_then(detail).or_else(|| at.and_then(|[x, z]| {
        destinations
            .iter()
            .filter(|d| d.map.eq_ignore_ascii_case(map))
            .filter_map(|d| Some((d.matrix?, detail(&d.id)?)))
            .min_by(|a, b| {
                let far = |m: &[[f32; 4]; 4]| (m[3][0] - x).powi(2) + (m[3][2] - z).powi(2);
                far(&a.0).total_cmp(&far(&b.0))
            })
            .map(|(_, d)| d)
    }));
    let same = |m: &str| crate::map_library::same_name(m, map);
    let area = AREAS.iter().find(|a| a.3.iter().any(|m| same(m)));
    let picture = area
        .and_then(|a| a.2.map(String::from).or_else(|| extra.places.get(a.0).map(|p| p.map.clone())))
        .or_else(|| PARKS.iter().find(|p| same(p.2)).and_then(|p| p.3.map(String::from)));
    let thumbnails = source["thumbnails"].as_object();
    let photo = [own_image, nearest.as_ref().map(|d| d.0.clone()), picture].into_iter().flatten().find_map(|file| {
        let (_, t) = thumbnails?.iter().find(|(k, _)| k.eq_ignore_ascii_case(&file))?;
        Some((root.join(t["rgba"].as_str()?).to_string_lossy().into_owned(), [t["width"].as_u64()? as u32, t["height"].as_u64()? as u32]))
    });
    Card {
        title: text(title),
        mode: text(if folder.is_empty() { tab_name } else { folder }),
        place: place_name(map).map_or_else(|| map.replace('_', " "), text),
        description: description.or_else(|| nearest.map(|d| text(&d.1))).unwrap_or_default(),
        photo,
    }
}

/// A copy of the load for `installed`, made once (setup makes it for the Start menu's maps).
fn copy<T: Clone + Send + Sync + 'static>(
    booted: &mut Option<(Vec<String>, Booted<T>)>,
    installed: &[String],
    load: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match booted {
        Some((maps, booted)) if maps.as_slice() == installed => Ok(booted.take()),
        _ => {
            let runtime = load()?;
            *booted = Some((installed.to_vec(), Booted::new(runtime.clone())));
            Ok(runtime)
        }
    }
}

/// Every map the Locations list can show.
fn known_maps() -> Vec<String> {
    AREAS.iter().flat_map(|a| a.3.iter().copied()).chain(PARKS.iter().map(|p| p.2)).map(String::from).collect()
}

/// Group the catalog into the retail Locations rows, keeping installed maps only.
pub(crate) fn places(destinations: &[Destination], installed: &[String], extra: &Extra) -> (Vec<Place>, Vec<Vec<Option<Spot>>>) {
    let has = |map: &str| installed.iter().any(|i| crate::map_library::same_name(i, map));
    let marker = |id: &str| MARKERS.iter().find(|(m, _)| *m == id).map(|(_, at)| *at).or_else(|| extra.markers.get(id).copied());
    // A located destination on `map`, else the map's own spawn.
    let spot = |d: Option<&Destination>, map: &str| Spot {
        map: map.into(),
        id: d.and_then(|d| d.matrix.map(|_| d.id.clone())),
        matrix: d.and_then(|d| d.matrix),
    };
    let mut places = Vec::new();
    let mut spots = Vec::new();
    for (name, help, map, maps) in AREAS {
        let mut rows = Vec::new();
        let mut targets = Vec::new();
        if name == SKATE_PARK {
            for (header, park, map, picture) in PARKS.into_iter().filter(|p| has(p.2)) {
                rows.push(Row { name: header.into(), header: true, ..Default::default() });
                targets.push(None);
                rows.push(Row { name: park.into(), preview: picture.map(String::from), ..Default::default() });
                targets.push(Some(spot(destinations.iter().find(|d| d.map.eq_ignore_ascii_case(map)), map)));
            }
        }
        for &map in maps.iter().filter(|m| has(m)) {
            let mut here: Vec<&Destination> = destinations.iter().filter(|d| d.map.eq_ignore_ascii_case(map)).collect();
            // Retail lists letters before digits (2nd & Navy last).
            here.sort_by_key(|d| (d.name.starts_with(|c: char| c.is_ascii_digit()), d.name.to_lowercase()));
            for d in here {
                let info = DETAILS.iter().find(|(id, ..)| *id == d.id).map(|(_, image, text)| (image.to_string(), text.to_string()))
                    .or_else(|| extra.details.get(&d.id).cloned());
                rows.push(Row { name: d.name.clone(), marker: marker(&d.id), info, ..Default::default() });
                targets.push(Some(spot(Some(d), map)));
            }
        }
        if !rows.is_empty() {
            let own = extra.places.get(name);
            places.push(Place {
                name: name.into(),
                help: help.into(),
                map: map.map(String::from).or_else(|| own.map(|p| p.map.clone())),
                pin: own.map(|p| (p.pin.clone(), p.anchor)),
                rows,
            });
            spots.push(targets);
        }
    }
    (places, spots)
}

pub(crate) struct ChallengeMapPlugin;
impl Plugin for ChallengeMapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(
                PreUpdate,
                // The Start menu runs after this, so it never sees the press that closed it.
                interact.after(crate::customiser::navigation),
            )
            .add_systems(Update, (resize_target, animate).chain());
    }
}

const MANIFEST: &str = "runtime/challengemap.json";
/// `f` of online/online.json for `installed`. perf 10-18: parsed (with its icons) once per installed-maps list, not on
/// every lookup; a failed load is not kept. SKATE_ONLINE_MAP_CACHE=0: read every time, as before.
fn with_online<T>(
    cache: &std::sync::Mutex<Option<(Vec<String>, OnlineMap)>>,
    root: &std::path::Path,
    installed: &[String],
    f: impl FnOnce(&OnlineMap) -> T,
) -> Result<T, String> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("SKATE_ONLINE_MAP_CACHE").is_none_or(|v| v != "0")) {
        return OnlineMap::load(root, installed).map(|map| f(&map));
    }
    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.as_ref().is_none_or(|(maps, _)| maps != installed) {
        *cache = Some((installed.to_vec(), OnlineMap::load(root, installed)?));
    }
    Ok(f(&cache.as_ref().expect("just loaded").1))
}

fn root(asset_root: &std::path::Path) -> PathBuf {
    std::env::var_os("SKATE_CHALLENGE_MAP_ROOT").map(PathBuf::from).unwrap_or_else(|| asset_root.join("private/challenge-map"))
}
/// What setup reads, for the startup prefetch (apt_canvas::prefetch).
pub(crate) fn manifest(asset_root: &std::path::Path) -> PathBuf {
    root(asset_root).join(MANIFEST)
}

fn setup(
    mut commands: Commands,
    config: Res<Config>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    let Ok(output) = cameras.single() else { return };
    // The maps the Start menu opens the screens for.
    let installed = menu.map_or_else(known_maps, |menu| menu.map_labels());
    let root = root(&config.asset_root);
    let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -5);
    let result = (|| -> Result<(serde_json::Value, Vec<Destination>, Extra, Option<Runtime>), String> {
        let source: serde_json::Value = match crate::apt_canvas::prefetched_json(&root.join(MANIFEST)) {
            Some(json) => json,
            None => serde_json::from_slice(
                &std::fs::read(root.join(MANIFEST)).map_err(|e| format!("{}: {e}", root.display()))?,
            )
            .map_err(|e| e.to_string())?,
        };
        if source["format"] != "skate3-challenge-map" || source["version"] != 1 {
            return Err("Unsupported Challenge Map manifest".into());
        }
        let destinations = crate::teleport_menu::load(&config.asset_root)?;
        let extra = Extra::load(&root)?;
        // Run the movie once now, so a defective install falls back before the menu opens. The load for the
        // installed maps is the one opening copies; with none of the retail places installed, check the full list.
        let booted = Runtime::load(&source, places(&destinations, &installed, &extra).0).ok();
        if booted.is_none() {
            Runtime::load(&source, places(&destinations, &known_maps(), &extra).0)?;
        }
        for (path, size) in movie_textures(&source)? {
            canvas.preload(&mut images, &path, size)?;
        }
        Ok((source, destinations, extra, booted))
    })();
    let (source, destinations, extra, booted) = match result {
        Ok(value) => value,
        Err(error) => {
            warn!("Original Challenge Map unavailable ({error}); Challenge Map stays a Start menu picker");
            return;
        }
    };
    let full = || Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() };
    // A full-screen page: black letterbox bars, as the console shows around 16:9.
    commands
        .spawn((
            Overlay,
            UiTargetCamera(output),
            GlobalZIndex(12),
            Pickable::IGNORE,
            BackgroundColor(Color::BLACK),
            Node { display: Display::None, ..full() },
        ))
        .with_child((MaterialNode(canvas.composite.clone()), Pickable::IGNORE, full()));
    let online = OnlineMap::load(&root, &known_maps())
        .map_err(|e| warn!("Online challenge map unavailable ({e}); Online stays the classic panel"))
        .is_ok();
    let booted_online = online
        .then(|| OnlineRuntime::load(&source, OnlineMap::load(&root, &installed).ok()?, &destinations, &extra).ok())
        .flatten()
        .map(|runtime| (installed.clone(), Booted::new(runtime)));
    commands.insert_resource(ChallengeMap {
        booted: booted.map(|runtime| (installed, Booted::new(runtime))),
        booted_online,
        source,
        canvas,
        destinations,
        extra,
        runtime: None,
        online,
        online_cache: Default::default(),
        root: root.clone(),
        picked_online: None,
        spots: Vec::new(),
        shown: false,
        closing: None,
        accumulated: 0.,
        picked: None,
        closed_this_frame: false,
    });
    info!("Original Challenge Map loaded from {}", root.display());
}

pub(crate) fn interact(
    map: Option<ResMut<ChallengeMap>>,
    keys: Res<ButtonInput<KeyCode>>,
    nav: Res<crate::customiser::Navigation>,
    mut sounds: MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
    mut intro_nav: Local<Option<Key>>,
) {
    let Some(mut map) = map else { return };
    if let Some(runtime) = map.runtime.as_mut() {
        sounds.write_batch(runtime.sounds_mut().drain(..).map(crate::skate_audio::FrontEndSoundRequest));
    }
    map.closed_this_frame = false;
    if map.runtime.is_none() || map.closing.is_some() || !map.shown {
        // perf 10-18: a direction pressed during the intro plays once it is in (core_menu::buffer_intro_nav).
        if map.runtime.is_some() && map.closing.is_none() {
            crate::core_menu::buffer_intro_nav(&mut intro_nav, nav.pressed);
        } else {
            *intro_nav = None;
        }
        return;
    }
    // The online map's top tabs (InitTabs, not second level) show L_Trigger / R_Trigger beside them: LT / RT step
    // them as Left / Right do.
    let online = matches!(map.runtime, Some(Screen::Online(_)));
    let pressed = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0 || online && nav.triggers & 1 != 0, Key::Left),
        (nav.pressed & 8 != 0 || online && nav.triggers & 2 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        // Start leaves the map as B does, back to the Start menu.
        (keys.just_pressed(KeyCode::Backspace) || keys.just_pressed(KeyCode::Escape) || nav.pressed & (0x2000 | 0x10) != 0, Key::Back),
    ];
    let buffered = intro_nav.take().map(|key| (true, key));
    for (_, key) in buffered.into_iter().chain(pressed).filter(|(down, _)| *down) {
        let Some(runtime) = map.runtime.as_mut() else { return };
        let outcome = match runtime {
            Screen::Offline(r) => r.input(key).map(|o| o.map(Ok)),
            Screen::Online(r) => r.input(key).map(|o| o.map(Err)),
        };
        match outcome {
            Ok(None) => {}
            Ok(Some(Err(OnlineOutcome::Pick(pick)))) => {
                map.picked_online = Some(pick);
                map.begin_close();
                return;
            }
            Ok(Some(Err(OnlineOutcome::Close))) | Ok(Some(Ok(Outcome::Close))) => {
                map.begin_close();
                return;
            }
            Ok(Some(Ok(Outcome::Travel(place, spot)))) => {
                map.picked = map.spots.get(place).and_then(|s| s.get(spot)).cloned().flatten();
                map.begin_close();
                return;
            }
            Err(error) => {
                error!("Original Challenge Map stopped: {error}");
                map.runtime = None;
                map.closed_this_frame = true;
                return;
            }
        }
    }
}

fn resize_target(
    map: Option<ResMut<ChallengeMap>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut map) = map {
        map.canvas.resize(&window, &mut images, &mut composites);
    }
}

#[allow(clippy::too_many_arguments)]
fn animate(
    mut commands: Commands,
    map: Option<ResMut<ChallengeMap>>,
    real: Res<Time<Real>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<&mut Camera>,
    mut overlays: Query<&mut Node, With<Overlay>>,
    presence: Option<Res<crate::presence::Presence>>,
    roster: Option<Res<crate::online_roster::OnlineRoster>>,
) {
    let Some(mut map) = map else { return };
    let map = &mut *map;
    if let Some(runtime) = map.runtime.as_mut() {
        map.accumulated = (map.accumulated + real.delta_secs()).min(4. * FRAME);
        let mut result = Ok(());
        while map.accumulated >= FRAME && result.is_ok() {
            map.accumulated -= FRAME;
            result = runtime.advance();
            if result.is_ok() && runtime.intro_complete() && !map.shown {
                map.shown = true;
                result = runtime.show();
            }
        }
        if let Some(closing) = map.closing.as_mut() {
            *closing += real.delta_secs();
            if runtime.outro_complete() || *closing >= CLOSE_TIMEOUT {
                map.runtime = None;
                map.closing = None;
            }
        } else if let Err(error) = result {
            error!("Original Challenge Map stopped: {error}");
            map.runtime = None;
        }
    }
    let active = map.runtime.is_some();
    map.canvas.set_active(active, &mut cameras);
    for mut node in &mut overlays {
        let display = if active { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    let mut draws = match map.runtime.as_ref().map(Screen::draws) {
        Some(Ok(draws)) => draws,
        Some(Err(error)) => {
            error!("Challenge Map geometry: {error}");
            Vec::new()
        }
        None => Vec::new(),
    };
    if let (Some(Screen::Online(online)), Some(players)) = (map.runtime.as_ref(), presence.and_then(|p| p.players)) {
        // User 10-29: never fewer than our own session holds (a player just signed in isn't counted yet).
        let players = players.max(roster.map_or(0, |r| r.members.len() as u32));
        draws.extend(players_online(&online.bindings.movie.text_assets, players));
    }
    map.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

/// Not retail (user 10-25): "Players Online: N" bottom right of the online map, in the spectate legend's look
/// (online_menu::legend): the players signed in to online on this version, as the presence helper last counted
/// them (every 30 seconds), never fewer than our session's players.
fn players_online(text: &crate::apt_text::TextAssets, players: u32) -> Vec<Draw> {
    use crate::graphic_board::{colour, label, quad};
    const RIGHT: f32 = 1280. - 116.;
    const TOP: f32 = 720. - 96.;
    const SIZE: f32 = 18.;
    let Some(font) = text.font_named("Futura Std Medium").and_then(|id| text.fonts.get(&id)) else { return Vec::new() };
    let words = format!("Players Online: {players}");
    let x = RIGHT - font.width(&words, SIZE);
    let mut out = vec![quad("runtime/white.rgba", [1, 1], [x - 10., TOP - 7., RIGHT + 10., TOP + SIZE + 7.], [0.024, 0.04, 0.055, 0.85])];
    out.extend(label(font, &words, [x, TOP], SIZE, colour(0xEBF2F8, 1.)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_map_is_read_once_per_installed_list() {
        let root = std::env::temp_dir().join(format!("skate-online-cache-{}", std::process::id()));
        std::fs::create_dir_all(root.join("online")).unwrap();
        let file = root.join("online/online.json");
        std::fs::write(&file, r#"{"format":"skate3-online-map","version":1,"tabs":[]}"#).unwrap();
        let cache = Default::default();
        let installed = vec!["University".to_string()];
        assert_eq!(with_online(&cache, &root, &installed, |m| m.tabs.len()), Ok(0));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(with_online(&cache, &root, &installed, |m| m.tabs.len()), Ok(0), "kept, not read again");
        assert!(with_online(&cache, &root, &[], |m| m.tabs.len()).is_err(), "another list reads the file again");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn destination(id: &str, name: &str, map: &str, placed: bool) -> Destination {
        let matrix = placed.then_some([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]]);
        Destination { id: id.into(), name: name.into(), map: map.into(), matrix, unavailable_reason: None }
    }

    #[test]
    fn locations_follow_retail_rows_and_installed_maps() {
        let destinations = [
            destination("dist_university_campusstart", "Chan Center", "University", true),
            destination("dist_downtown_aletown", "Aletown", "DownTown", true),
            destination("dist_industrial_2ndnnavy", "2nd & Navy", "Industrial", true),
            destination("dist_industrial_quarry", "Carverton Quarry", "Industrial", true),
            destination("m1", "Stadium - Monster Park", "MegaPark", false),
            destination("dist_blackboxpark", "Black Box Park", "BlackBoxPark", true),
        ];
        let installed = ["DownTown", "Industrial", "University", "DownTownSkatePark", "MegaPark"].map(String::from);
        let (places, spots) = places(&destinations, &installed, &Extra::default());
        let names: Vec<&str> = places.iter().map(|p| p.name.as_str()).collect();
        // Retail order; Black Box Park's map is not installed.
        assert_eq!(names, ["ID_DISTRICT_DOWNTOWN", "ID_DISTRICT_INDUSTRY", "ID_DISTRICT_UNIVERSITY", "ID_MAP_FILTER_SKATE_PARK"]);
        assert_eq!(places[0].rows[0].marker, Some([403.1, 536.7]), "Aletown's measured marker");
        assert_eq!(places[0].rows[0].info, Some(("aletown.rx2".into(), "ID_MAP_LOCATION_DESC_DT_ALETOWN".into())), "retail photo + description");
        let industrial: Vec<&str> = places[1].rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(industrial, ["Carverton Quarry", "2nd & Navy"], "digits after letters, as retail");
        // skate.Park: a header row before each park, and the park's picture.
        let parks: Vec<(&str, bool)> = places[3].rows.iter().map(|r| (r.name.as_str(), r.header)).collect();
        assert_eq!(parks, [("ID_WORLD_SKATEPARK_DOWNTOWN", true), ("ID_WORLD_SKATEPARK_DOWNTOWN_FULL", false),
            ("ID_WORLD_SKATEPARK_MEGAPARK", true), ("ID_WORLD_SKATEPARK_MEGAPARK_FULL", false)]);
        assert_eq!(places[3].rows[1].preview.as_deref(), Some("map_skatepark_downtown_full.rx2"));
        assert_eq!(spots[3][0], None, "headers go nowhere");
        // Unplaced parks travel to the map's spawn.
        assert_eq!(spots[3][3], Some(Spot { map: "MegaPark".into(), id: None, matrix: None }));
    }

    #[test]
    fn converted_content_lists_when_installed() {
        let destinations = [
            destination("p", "Parkade", "Skate_2_Parkade", true),
            destination("s", "Waterfront Sea Wall", "New_San_Vanelona", true),
            destination("o", "Old Pool", "New_San_Vanelona", true),
        ];
        // Installed names are the Start menu's labels: file stems with `_` shown as spaces.
        let installed = ["New San Vanelona", "Skate 2 Parkade", "Zen Plaza"].map(String::from);
        let (places, spots) = places(&destinations, &installed, &Extra::default());
        let names: Vec<&str> = places.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["ID_MAP_FILTER_SKATE_PARK", "ID_COMMON_NEW_SAN_VANELONA"]);
        let san_van: Vec<&str> = places[1].rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(san_van, ["Old Pool", "Waterfront Sea Wall", "Parkade"], "city spots, then its parks");
        assert_eq!(places[0].rows[1].preview.as_deref(), Some("screen_zen_park_empty.rx2"), "the DLC park's own picture");
        assert_eq!(spots[0][1], Some(Spot { map: "Zen_Plaza".into(), id: None, matrix: None }), "unplaced: map spawn");
    }

    #[test]
    fn every_area_is_listed_when_all_maps_are_installed() {
        let destinations = [
            destination("d", "Aletown", "DownTown", true),
            destination("i", "The Tanker", "Industrial", true),
            destination("u", "Chan Center", "University", true),
            destination("s", "skate.School", "SkateSchool", true),
            destination("m", "Maloof Vert", "MaloofMoneyCup", true),
            destination("b", "Black Box Park", "BlackBoxPark", true),
            destination("n", "Waterfront Sea Wall", "New_San_Vanelona", true),
            destination("maloof_money_cup_nyc_z_maloofmcnyc_start", "Maloof Money Cup 2010 NYC", "Maloof_Money_Cup_NYC", true),
            destination("danny_way_hawaiian_dream_freeskate_dannyway_megacompound_locator", "Danny Way's Hawaiian Dream",
                "Danny_Way_Hawaiian_Dream", true),
            destination("art_gallery_freeskate_artgallery_locator", "Art Gallery", "Art_Gallery", true),
            destination("sanatorium_z_sanitarium_start", "The Sanatorium", "Sanatorium", true),
            destination("s1", "Slappys", "Skate_1_San_Vanelona", true),
        ];
        // The startup probe loads the screen with every known map.
        let (places, _) = places(&destinations, &known_maps(), &Extra::default());
        assert_eq!(places.len(), AREAS.len());
        // Every Xbox DLC place shows its own map picture, with its location marked on it.
        for (name, map) in [
            ("#Maloof Money Cup 2010 NYC", "dlc_maloof_map_512.rx2"),
            ("#Danny Way's Hawaiian Dream", "map_danny.rx2"),
            ("#Art Gallery", "map_partypack.rx2"),
            ("#The Sanatorium", "map_afterdark.rx2"),
        ] {
            let place = places.iter().find(|p| p.name == name).unwrap();
            assert_eq!(place.map.as_deref(), Some(map), "{name}");
            assert!(place.rows.iter().all(|r| r.marker.is_some_and(|[u, v]| (0. ..512.).contains(&u) && (0. ..512.).contains(&v))), "{name}");
        }
        // The DLC parks show their own pictures.
        let parks = places.iter().find(|p| p.name == SKATE_PARK).unwrap();
        for picture in ["screen_zen_park_empty.rx2", "screen_backlot_park_empty.rx2", "screen_dtnight_park_empty.rx2"] {
            assert!(parks.rows.iter().any(|r| r.preview.as_deref() == Some(picture)), "{picture}");
        }
    }

    /// Opening copies the load kept from setup: each copy draws as a fresh load would, intro, list and presses.
    #[test]
    #[ignore = "requires the installed screens"]
    fn copied_maps_draw_as_fresh_loads() {
        let assets = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let root = assets.join("private/challenge-map");
        let source: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/challengemap.json")).unwrap()).unwrap();
        let (destinations, extra, installed) = (crate::teleport_menu::load(&assets).unwrap(), Extra::load(&root).unwrap(), known_maps());
        let offline = || Runtime::load(&source, places(&destinations, &installed, &extra).0);
        let online = || OnlineRuntime::load(&source, OnlineMap::load(&root, &installed)?, &destinations, &extra);
        let drawn = |mut screen: Screen| {
            let mut frames = Vec::new();
            let mut frame = |screen: &mut Screen| {
                screen.advance().unwrap();
                let draws = screen.draws().unwrap().into_iter().map(|d| (d.texture, d.size, d.vertices, d.multiply, d.add));
                frames.push(draws.collect::<Vec<_>>());
            };
            while !screen.intro_complete() {
                frame(&mut screen);
            }
            screen.show().unwrap();
            for key in [None, Some(Key::Down), Some(Key::Accept), Some(Key::Down), Some(Key::Back), Some(Key::Right)] {
                match (key, &mut screen) {
                    (Some(key), Screen::Offline(r)) => drop(r.input(key).unwrap()),
                    (Some(key), Screen::Online(r)) => drop(r.input(key).unwrap()),
                    (None, _) => {}
                }
                (0..12).for_each(|_| frame(&mut screen));
            }
            frames
        };
        let (mut booted, mut booted_online) = (None, None);
        for open in 0..3 {
            let fresh = drawn(Screen::Offline(offline().unwrap()));
            assert!(fresh == drawn(Screen::Offline(copy(&mut booted, &installed, offline).unwrap())), "map, open {open}");
            let fresh = drawn(Screen::Online(online().unwrap()));
            assert!(fresh == drawn(Screen::Online(copy(&mut booted_online, &installed, online).unwrap())), "online, open {open}");
        }
    }
    /// What opening the Start menu, the Challenge Map and the Online Challenge Map costs on the main thread, against
    /// the installed screens (SKATE3_ASSET_ROOT). `cargo test --release ... open_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires the installed screens"]
    fn open_cost() {
        use std::time::Instant;
        let assets = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let time = |what: &str, f: &mut dyn FnMut()| {
            f();
            let t = Instant::now();
            (0..5).for_each(|_| f());
            eprintln!("{what}: {:.1} ms", t.elapsed().as_secs_f64() * 1000. / 5.);
        };
        let read = |p: PathBuf| serde_json::from_slice::<serde_json::Value>(&std::fs::read(p).unwrap()).unwrap();
        // The Start menu.
        let core = read(assets.join("private/core-menu/runtime/core_menu.json"));
        let item = |name: &str| crate::core_menu_runtime::Item {
            name: name.into(), icon: "settings", help: String::new(), enabled: true, action: 0u32,
            choices: vec![], chosen: 0, disabled_choices: vec![], slider: false,
        };
        let cats = || vec![crate::core_menu_runtime::Category { name: "Skate".into(), icon: "singleplayer", items: vec![item("A"), item("B")] }];
        time("core menu Movie::load", &mut || drop(crate::apt_movie::Movie::load(&core).unwrap()));
        time("core menu Runtime::load", &mut || drop(crate::core_menu_runtime::Runtime::load(&core, cats(), "T".into()).unwrap()));
        // The Challenge Map and the Online Challenge Map.
        let root = assets.join("private/challenge-map");
        let source = read(root.join("runtime/challengemap.json"));
        let destinations = crate::teleport_menu::load(&assets).unwrap();
        let extra = Extra::load(&root).unwrap();
        time("challenge map Movie::load", &mut || drop(crate::apt_movie::Movie::load(&source).unwrap()));
        time("challenge map Runtime::load", &mut || {
            drop(Runtime::load(&source, places(&destinations, &known_maps(), &extra).0).unwrap())
        });
        time("OnlineMap::load", &mut || drop(OnlineMap::load(&root, &known_maps()).unwrap()));
        time("online OnlineRuntime::load", &mut || {
            let map = OnlineMap::load(&root, &known_maps()).unwrap();
            drop(OnlineRuntime::load(&source, map, &destinations, &extra).unwrap())
        });
        // What a ready-made copy costs instead.
        let core_rt = crate::core_menu_runtime::Runtime::load(&core, cats(), "T".into()).unwrap();
        time("core menu Runtime clone", &mut || drop(core_rt.clone()));
        let movie = crate::apt_movie::Movie::load(&source).unwrap();
        time("challenge map Movie clone", &mut || drop(movie.clone()));
        let map_rt = Runtime::load(&source, places(&destinations, &known_maps(), &extra).0).unwrap();
        time("challenge map Runtime clone", &mut || drop(map_rt.clone()));
        let online_rt = OnlineRuntime::load(&source, OnlineMap::load(&root, &known_maps()).unwrap(), &destinations, &extra).unwrap();
        time("online OnlineRuntime clone", &mut || drop(online_rt.clone()));
        time("  of which vm", &mut || drop(online_rt.vm.clone()));
        time("  of which movie", &mut || drop(online_rt.bindings.movie.clone()));
        time("  of which shapes", &mut || drop(online_rt.bindings.shapes.clone()));
        time("  of which map", &mut || drop(online_rt.bindings.map.clone()));
        time("  of which language", &mut || drop(online_rt.bindings.movie.text_assets.clone()));
        eprintln!("  vm objects {}", online_rt.vm.objects.len());
        #[cfg(windows)]
        {
            let heap = || {
                use windows_sys::Win32::System::Memory::{GetProcessHeap, HEAP_SUMMARY, HeapSummary};
                let mut s = HEAP_SUMMARY { cb: size_of::<HEAP_SUMMARY>() as u32, ..Default::default() };
                // SAFETY: a plain query of this process's default heap into a sized struct.
                unsafe { HeapSummary(GetProcessHeap(), 0, &mut s) };
                s.cbAllocated as f64 / 1e6
            };
            let base = heap();
            let a = core_rt.clone();
            let core_mb = heap() - base;
            let b = map_rt.clone();
            let map_mb = heap() - base - core_mb;
            let c = online_rt.clone();
            eprintln!("heap per copy: core menu {core_mb:.1} MB, challenge map {map_mb:.1} MB, online {:.1} MB", heap() - base - core_mb - map_mb);
            drop((a, b, c));
        }
        // What an open costs now: taking the copy the worker made since the last open (kept: the old screen is
        // dropped when it closes, as before).
        let take = |what: &str, mut take: Box<dyn FnMut()>| {
            let mut total = 0.;
            for _ in 0..5 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                let t = Instant::now();
                take();
                total += t.elapsed().as_secs_f64() * 1000.;
            }
            eprintln!("{what}: {:.2} ms", total / 5.);
        };
        let mut core_booted = Booted::new(core_rt.clone());
        let mut kept = Vec::new();
        take("core menu open (Booted::take + with)", Box::new(move || kept.push(core_booted.take().with(cats(), "T".into()).unwrap())));
        let mut map_booted = Booted::new(map_rt.clone());
        let mut kept = Vec::new();
        take("challenge map open (Booted::take)", Box::new(move || kept.push(map_booted.take())));
        let mut online_booted = Booted::new(online_rt.clone());
        let mut kept = Vec::new();
        take("online open (Booted::take)", Box::new(move || kept.push(online_booted.take())));
        // Then the frames until the list shows (intro), as `animate` runs them.
        let mut rt = Runtime::load(&source, places(&destinations, &known_maps(), &extra).0).unwrap();
        let (t, mut frames) = (Instant::now(), 0);
        while !rt.bindings.intro_complete {
            rt.advance().unwrap();
            rt.draws().unwrap();
            frames += 1;
        }
        rt.show().unwrap();
        eprintln!("challenge map intro: {frames} frames, {:.1} ms of work", t.elapsed().as_secs_f64() * 1000.);
        // Textures the first second of the list draws that setup did not preload (Canvas reads them on first draw).
        let preloaded = crate::apt_canvas::movie_textures(&source).unwrap();
        let mut lazy = std::collections::BTreeSet::new();
        let mut online = OnlineRuntime::load(&source, OnlineMap::load(&root, &known_maps()).unwrap(), &destinations, &extra).unwrap();
        while !online.bindings.intro_complete {
            online.advance().unwrap();
            lazy.extend(online.draws().unwrap().into_iter().map(|d| d.texture));
        }
        online.show().unwrap();
        for _ in 0..60 {
            rt.advance().unwrap();
            online.advance().unwrap();
            lazy.extend(rt.draws().unwrap().into_iter().chain(online.draws().unwrap()).map(|d| d.texture));
        }
        // A press in the shown list: Accept opens a place / a folder, Down moves, Back returns.
        use crate::core_menu_runtime::Key;
        for (what, key) in [("Accept", Key::Accept), ("Down", Key::Down), ("Back", Key::Back), ("Down", Key::Down)] {
            let t = Instant::now();
            rt.input(key).unwrap();
            let offline = t.elapsed().as_secs_f64() * 1000.;
            let t = Instant::now();
            online.input(key).unwrap();
            eprintln!("press {what}: challenge map {offline:.1} ms, online {:.1} ms", t.elapsed().as_secs_f64() * 1000.);
            for _ in 0..30 {
                rt.advance().unwrap();
                online.advance().unwrap();
                lazy.extend(rt.draws().unwrap().into_iter().chain(online.draws().unwrap()).map(|d| d.texture));
            }
        }
        lazy.retain(|t| !preloaded.contains_key(t) && root.join(t).is_file());
        let t = Instant::now();
        let bytes: usize = lazy.iter().map(|p| std::fs::read(root.join(p)).unwrap().len()).sum();
        eprintln!("lazy textures: {} files, {:.1} MB, {:.1} ms to read (warm)", lazy.len(), bytes as f64 / 1e6, t.elapsed().as_secs_f64() * 1000.);
    }

    /// Not retail (user 10-13): the proposal tab's card for every installed online row. SKATE3_ASSET_ROOT = the
    /// install's assets.
    #[test]
    #[ignore = "needs INST\\assets"]
    fn proposal_cards_name_the_row_and_find_a_photo() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let root = assets.join("private/challenge-map");
        let source: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(MANIFEST)).unwrap()).unwrap();
        let destinations = crate::teleport_menu::load(&assets).unwrap();
        let extra = Extra::load(&root).unwrap();
        let installed = known_maps();
        let map = OnlineMap::load(&root, &installed).unwrap();
        let pick = map.find(3, "ots_dwtn_06_ol").unwrap();
        let arbor = card(&pick, &map.tabs[3].name, &source, &destinations, &extra, &root);
        assert_eq!((arbor.title.as_str(), arbor.mode.as_str(), arbor.place.as_str()), ("Arbor Ledge", "Spot Battle", "Downtown District"), "{arbor:?}");
        assert!(arbor.description.contains("ledges and gaps"), "{arbor:?}");
        let (path, size) = arbor.photo.clone().expect("a photo");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size[0] as u64 * size[1] as u64 * 4, "{path}");
        let (mut rows, mut photos, mut missing) = (0, 0, Vec::new());
        fn walk(rows: &[crate::online_map::Entry], out: &mut Vec<String>) {
            for e in rows {
                out.extend(e.challenge.as_ref().map(|c| c.id.clone()).or_else(|| e.location.as_ref().map(|_| e.name.clone())));
                walk(&e.rows, out);
            }
        }
        for (tab, t) in map.tabs.iter().enumerate() {
            let mut ids = Vec::new();
            walk(&t.rows, &mut ids);
            for id in ids {
                let Some(pick) = map.find(tab, &id) else { continue };
                let c = card(&pick, &t.name, &source, &destinations, &extra, &root);
                rows += 1;
                assert!(!c.title.is_empty() && !c.title.starts_with("ID_") && !c.mode.starts_with("ID_"), "{c:?}");
                if c.photo.as_ref().is_some_and(|(p, _)| std::path::Path::new(p).is_file()) { photos += 1 } else { missing.push(c.title) }
            }
        }
        eprintln!("{photos}/{rows} rows with a photo; without: {missing:?}");
        assert!(photos * 10 >= rows * 9, "{photos}/{rows}");
    }

    /// Not retail: Party Play's location page shows each row's photo (its proposal card's), for every type's Skate 3,
    /// Skate 2 and Skate 1 rows. SKATE3_ASSET_ROOT = the install's assets.
    #[test]
    #[ignore = "needs INST\\assets"]
    fn every_party_play_row_finds_a_photo() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let root = assets.join("private/challenge-map");
        let source: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(MANIFEST)).unwrap()).unwrap();
        let destinations = crate::teleport_menu::load(&assets).unwrap();
        let extra = Extra::load(&root).unwrap();
        let map = OnlineMap::load(&root, &known_maps()).unwrap();
        let mut games = std::collections::BTreeMap::<(usize, &str), usize>::new();
        for (kind, list) in crate::party_play::places(&map).iter().enumerate() {
            for place in list {
                let pick = map.find(place.tab, &place.challenge.id).unwrap_or_else(|| panic!("{} not on tab {}", place.title, place.tab));
                let c = card(&pick, &map.tabs[place.tab].name, &source, &destinations, &extra, &root);
                let (path, size) = c.photo.unwrap_or_else(|| panic!("{}: no photo", place.title));
                assert_eq!(std::fs::metadata(&path).unwrap().len(), size[0] as u64 * size[1] as u64 * 4, "{path}");
                let game = if place.title.ends_with("(Skate 1)") { "Skate 1" } else if place.title.ends_with("(Skate 2)") { "Skate 2" } else { "Skate 3" };
                *games.entry((kind, game)).or_default() += 1;
            }
        }
        eprintln!("rows per (type, game): {games:?}");
        for game in ["Skate 1", "Skate 2", "Skate 3"] {
            assert!(games.contains_key(&(0, game)) && games.contains_key(&(1, game)), "{game}: Spot Battle and S.K.A.T.E. rows");
        }
    }
}
