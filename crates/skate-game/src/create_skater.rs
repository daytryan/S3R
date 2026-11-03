//! Edit Skater through Skate 3's original Create-a-Skater screen (cas/createskater).
//! The customiser keeps the menu tree, profile, live preview and saving; this
//! plugin presents its pages in the shipped movie and feeds it controller input.
//! Without the prepared screen (tools/prepare_create_skater.py) the classic
//! customiser panel stays in charge.
use crate::{
    apt_canvas::{Canvas, movie_textures},
    config::Config,
    core_menu_runtime::{Key, SOUND_A_BUTTON, SOUND_B_BUTTON},
    cas_menu::{self, Act, Node as Page_},
    create_skater_runtime::{Mode, Outcome, Page, Runtime},
    customiser::Customiser,
    customiser_parts::{Library, Parts},
    graphics_menu::Menu,
    scoring_hud::{HudComposite, HudMaterial},
    skate_audio::FrontEndSoundRequest,
};
use bevy::{prelude::*, window::PrimaryWindow};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

const LAYER: usize = 31;
const FRAME: f32 = 1. / 60.;
/// Outro fallback if the movie never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;

#[derive(Deserialize)]
struct Catalog {
    components: Vec<CatalogComponent>,
    materials: HashMap<String, CatalogMaterial>,
}
#[derive(Deserialize)]
struct CatalogComponent {
    models: Vec<CatalogModel>,
}
#[derive(Deserialize)]
struct CatalogModel {
    id: String,
    name: String,
    #[serde(default)]
    flags: HashMap<String, String>,
    lods: Vec<CatalogLod>,
}
#[derive(Deserialize)]
struct CatalogLod {
    material_instances: Vec<Vec<CatalogInstance>>,
}
#[derive(Deserialize)]
struct CatalogInstance {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct CatalogMaterial {
    #[serde(default)]
    flags: HashMap<String, String>,
}
pub(crate) struct PartArt {
    pub thumbnail: String,
    pub name: String,
    pub logo: String,
    /// Downloadable content (cas.DlcID): the retail tile and panel show the DLC badge.
    pub dlc: bool,
    /// The game it comes from (Art::game).
    pub game: &'static str,
}
/// Authored presentation of each part: the retail thumbnail
/// (data/fe/cas/<gender>/<slot>/<model>_<material>), string-table name and brand logo.
#[derive(Default)]
pub(crate) struct Art {
    /// (asset id, material id) -> (thumbnail keys, display name, logo key, DLC)
    parts: HashMap<(String, String), (Vec<String>, String, String, bool)>,
    /// Stock graphic id -> its retail logo tile (data/fe/cas/logos/logo_object_<material>).
    graphics: HashMap<String, String>,
    /// (retail TattooCategory 1-3, id, name, retail tile key, else the design PNG path)
    tattoos: Vec<(u8, String, String, String)>,
    /// String-table IDs present in the owned language file.
    strings: std::collections::HashSet<String>,
    /// Wear-style families: default model -> [default, alternates...]. The
    /// retail Edit Clothing "Option N" switches between these models (hood up,
    /// open, cap backwards...), keeping the design.
    families: HashMap<String, Vec<String>>,
    /// Alternate wear-style models: not listed as items of their own.
    alternates: std::collections::HashSet<String>,
    /// Material id -> the game its model comes from (no material is shared between games).
    games: HashMap<String, &'static str>,
}
fn alnum(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_lowercase()
}
/// The game a catalog model comes from: old_skate_clothing.py prefixes every token of a ported Skate 1 / 2 model
/// name with "s1" / "s2" (tools/pipeline/skate2port/families.py `origin`); the rest are Skate 3's. pro_clothing.py's
/// items carry "s2mq" on every token only to keep their tile names apart: its Skate 2 ones are the Skate 2 pros' (keyed
/// "s2_<pro>") and Mike's backpack, and the rest are the Skate 3 roster's (Mike Carroll's among them).
fn origin(model: &str) -> &'static str {
    let tokens: Vec<&str> = model.split('_').collect();
    if tokens.first() == Some(&"s2mq") {
        let skate2 = tokens.get(1) == Some(&"s2mqs2") || tokens.get(1..3) == Some(&["s2mqmike", "s2mqaccessory"][..]);
        return if skate2 { "Skate 2" } else { "Skate 3" };
    }
    let ported = |game: &str| tokens.iter().all(|token| token.starts_with(game));
    if ported("s1") {
        "Skate 1"
    } else if ported("s2") {
        "Skate 2"
    } else {
        "Skate 3"
    }
}
impl Art {
    fn build(catalog: Catalog, library: &Library, source: &Value, assets: &std::path::Path) -> Self {
        let mut stems: HashMap<String, Vec<String>> = HashMap::new();
        let mut logos = HashMap::new();
        for key in source["thumbnails"].as_object().into_iter().flat_map(|t| t.keys()) {
            let stem = key.rsplit('/').next().unwrap_or(key);
            if let Some(brand) = stem.strip_prefix("logo_object_logo_").and_then(|b| b.strip_suffix("_colour_mat")) {
                logos.insert(alnum(brand), key.clone());
            }
            stems.entry(stem.to_owned()).or_default().push(key.clone());
        }
        let language = &source["language"];
        let mut parts = HashMap::new();
        let mut graphics = HashMap::new();
        let mut tattoo_tiles = HashMap::new();
        let mut games = HashMap::new();
        for model in catalog.components.iter().flat_map(|c| &c.models) {
            let Some(lod) = model.lods.first() else { continue };
            for instance in lod.material_instances.iter().flatten() {
                games.insert(instance.id.clone(), origin(&model.name));
                let stem = format!("{}_{}", model.name, instance.name).to_lowercase();
                let Some(thumbnails) = stems.get(&stem) else { continue };
                // Stock graphics are materials of the "logo_object" model.
                if library.graphics.contains_key(&instance.id) {
                    graphics.insert(instance.id.clone(), thumbnails[0].clone());
                }
                // Tattoos too: their retail tile is data/fe/cas/logos/logo_object_<material> (a few also have one
                // under tattoo/), not the decal stretched into a tile.
                if library.tattoos.contains_key(&instance.id) {
                    let tile = thumbnails.iter().find(|t| t.starts_with("logos/")).unwrap_or(&thumbnails[0]);
                    tattoo_tiles.insert(instance.id.clone(), tile.clone());
                }
                let name = language[format!("ID_CAC_PART_{}", stem.to_uppercase())]
                    .as_str()
                    .filter(|n| !n.trim().is_empty())
                    .map(str::to_owned)
                    .or_else(|| library.models.get(&model.id).map(|m| m.name.clone()))
                    .unwrap_or_default();
                let flag = |k: &str| catalog.materials.get(&instance.id).and_then(|m| m.flags.get(k));
                let logo = flag("cas.ManufacturerID").and_then(|b| logos.get(&alnum(b))).cloned().unwrap_or_default();
                let dlc = flag("cas.DlcID").is_some_and(|d| !d.is_empty());
                parts.insert((model.id.clone(), instance.id.clone()), (thumbnails.clone(), format!("#{name}"), logo, dlc));
            }
        }
        let mut tattoos: Vec<(u8, String, String, String)> = library
            .tattoos
            .iter()
            .filter_map(|(id, t)| {
                let category = catalog.materials.get(id)?.flags.get("cas.TattooCategory")?.parse().ok()?;
                let tile = tattoo_tiles.get(id).cloned().unwrap_or_else(|| assets.join(&t.texture).to_string_lossy().into_owned());
                Some((category, id.clone(), format!("#{}", t.name), tile))
            })
            .collect();
        tattoos.sort();
        let strings = source["language"].as_object().map(|l| l.keys().cloned().collect()).unwrap_or_default();
        let (families, alternates) = wear_families(&catalog);
        Self { parts, graphics, tattoos, strings, families, alternates, games }
    }
    /// The game a material (part design, tattoo, stock graphic) comes from; empty if the catalog lacks it.
    pub(crate) fn game(&self, material: &str) -> &'static str {
        self.games.get(material).copied().unwrap_or_default()
    }
    /// A stock graphic's retail tile (a thumbnail key), when it has one.
    pub(crate) fn graphic_tile(&self, id: &str) -> Option<&str> {
        self.graphics.get(id).map(String::as_str)
    }
    /// The wear styles of an item's model (default first); one entry if none.
    pub(crate) fn wear_styles(&self, model: &str) -> Vec<String> {
        self.families.get(model).cloned().unwrap_or_else(|| vec![model.to_owned()])
    }
    pub(crate) fn is_alternate(&self, model: &str) -> bool {
        self.alternates.contains(model)
    }
    /// Test hook: the families found in a catalog.
    #[cfg(test)]
    pub(crate) fn family_count(&self) -> usize {
        self.families.len()
    }
    /// The game's string ID when it exists, else the literal text.
    pub(crate) fn string(&self, id: &str, fallback: &str) -> String {
        if self.strings.contains(id) { id.to_owned() } else { format!("#{fallback}") }
    }
    /// Designs of one retail tattoo category: (id, name, tile: a thumbnail key, else the design PNG).
    pub(crate) fn tattoos(&self, category: u8) -> Vec<(String, String, String)> {
        self.tattoos.iter().filter(|t| t.0 == category).map(|t| (t.1.clone(), t.2.clone(), t.3.clone())).collect()
    }
    /// The tile for a customiser patch selecting one part and its authored material.
    pub(crate) fn part(&self, patch: &Value, gender: &str) -> Option<PartArt> {
        let (slot, selection) = patch["selections"].as_object()?.iter().next()?;
        if !patch["colours"][slot].is_null() {
            return None;
        }
        let key = (selection["asset_id"].as_str()?.to_owned(), selection["material_id"].as_str()?.to_owned());
        let (thumbnails, name, logo, dlc) = self.parts.get(&key)?;
        let thumbnail = thumbnails.iter().find(|t| t.starts_with(&format!("{gender}/"))).or(thumbnails.first())?;
        Some(PartArt { thumbnail: thumbnail.clone(), name: name.clone(), logo: logo.clone(), dlc: *dlc, game: self.game(&key.1) })
    }
}

/// Group each default wear-style model (IsDefaultWearStyle) with its alternates:
/// same slot and gender, a WearStyle/HatOrientation, the same number of designs,
/// and either shared design materials, the default's name contained in the
/// alternate's (hoodie_zipup_male -> hoodie_zipup_male_hood_up), or the
/// closed/open naming pair (vato_jacket_closed_male -> vato_jacket_open_male).
/// Designs match by index across a family.
fn wear_families(catalog: &Catalog) -> (HashMap<String, Vec<String>>, std::collections::HashSet<String>) {
    let mut families = HashMap::new();
    let mut alternates = std::collections::HashSet::new();
    for component in &catalog.components {
        let ids = |m: &CatalogModel| -> Vec<String> {
            m.lods.first().map(|l| l.material_instances.iter().flatten().map(|i| i.id.clone()).collect()).unwrap_or_default()
        };
        let tokens = |m: &CatalogModel| -> std::collections::HashSet<String> { m.name.to_lowercase().split('_').map(str::to_owned).collect() };
        let flag = |m: &CatalogModel, k: &str| m.flags.get(k).cloned().unwrap_or_default();
        // Bug 10-25 #22: the customizable (stamp UV) longsleeves carry no IsDefaultWearStyle nor WearStyle, yet retail
        // offers their sleeves-up model as the second style; only those two families (male, female) come of this.
        let head = |m: &CatalogModel| {
            flag(m, "cas.IsDefaultWearStyle") == "yes" || !flag(m, "cas.StampUVConstraintQ1").is_empty() && flag(m, "cas.WearStyle").is_empty()
        };
        for default in component.models.iter().filter(|m| head(m)) {
            let designs = ids(default);
            let mut members: Vec<&CatalogModel> = component
                .models
                .iter()
                .filter(|s| {
                    s.id != default.id
                        && flag(s, "cas.IsDefaultWearStyle") != "yes"
                        && (!flag(s, "cas.WearStyle").is_empty() || !flag(s, "cas.HatOrientation").is_empty())
                        && flag(s, "cas.Gender") == flag(default, "cas.Gender")
                        && ids(s).len() == designs.len()
                        && (ids(s).iter().any(|i| designs.contains(i))
                            || tokens(default).is_subset(&tokens(s))
                            || default.name.replace("closed", "open") == s.name)
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            // Authored numbering (_1_flippedup, _2_45left, ...) gives the option order.
            members.sort_by(|a, b| a.name.cmp(&b.name));
            alternates.extend(members.iter().map(|m| m.id.clone()));
            families.insert(default.id.clone(), std::iter::once(default.id.clone()).chain(members.iter().map(|m| m.id.clone())).collect());
        }
    }
    (families, alternates)
}

#[derive(Component)]
struct Overlay;

/// One open page: where it is, what its rows do, and the profile when it opened
/// (Undo Changes, and Back from a grid, restore it).
struct Level {
    node: Page_,
    acts: Vec<Act>,
    entered: Value,
    row: usize,
    /// A grid's search (item 11, ours): kept while its Edit Clothing is open, gone with the grid.
    search: String,
    /// A clothing or board grid's game (cas_menu::GAMES, RB / LB; 0 = all), kept as its search is.
    game: usize,
}

/// The original confirmation popup (popup/sk8popup), drawn over the screen.
struct Popup {
    source: Value,
    canvas: Canvas,
    runtime: Option<crate::popup_runtime::Runtime>,
    accumulated: f32,
    asking: Ask,
}
/// What the open popup asks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Gender,
    /// Leaving Edit Skater: Save and Exit, Continue without saving; Back stays.
    Exit,
    /// Delete this Custom Graphic (location, save number): Yes deletes it.
    Delete(&'static str, usize),
    /// The Graphic Creator's question (its editor reads the answer).
    Creator,
    /// A Custom Graphic's share code went to the clipboard (or did not): OK.
    Copied,
}
impl Popup {
    fn ask(&mut self, asking: Ask, description: &str, options: &[&str], cancelable: bool) -> Result<(), String> {
        self.runtime = Some(crate::popup_runtime::Runtime::load(&self.source, description, options, cancelable)?);
        self.accumulated = 0.;
        self.asking = asking;
        Ok(())
    }
}
#[derive(Component)]
struct PopupOverlay;

/// The next open's movie, made ahead: a load is ~50 ms of the main thread (perf 10-12: CAS open and the Graphic
/// Creator's board art each paid one). Taking one starts the next on a worker. One spare, so ~19 MB resident.
struct Spare {
    source: Arc<Value>,
    ready: Option<Runtime>,
    loading: Option<std::thread::JoinHandle<Result<Runtime, String>>>,
}
impl Spare {
    fn new(source: Arc<Value>, first: Runtime) -> Self {
        Self { source, ready: Some(first), loading: None }
    }
    /// A fresh `Runtime::load(source, empty_page())`; waits only while the worker is still loading it.
    fn take(&mut self) -> Result<Runtime, String> {
        let runtime = match (self.ready.take(), self.loading.take().and_then(|loading| loading.join().ok())) {
            (Some(ready), _) => Ok(ready),
            (None, Some(loaded)) => loaded,
            (None, None) => Runtime::load(&self.source, empty_page()),
        };
        let source = self.source.clone();
        self.loading = std::thread::Builder::new().name("cas-load".into()).spawn(move || Runtime::load(&source, empty_page())).ok();
        runtime
    }
}

#[derive(Resource)]
struct Screen {
    source: Arc<Value>,
    spare: Spare,
    canvas: Canvas,
    popup: Option<Popup>,
    art: Option<Art>,
    /// The thumbnail catalog, read and parsed on a worker from startup, so the first open doesn't (user 10-16: online
    /// the world runs on under Edit Skater, and that read stalled the frame).
    catalog: Option<std::thread::JoinHandle<Result<Catalog, String>>>,
    runtime: Option<Runtime>,
    stack: Vec<Level>,
    shown: bool,
    closing: Option<f32>,
    accumulated: f32,
    /// The on-screen keyboard (team_select's): a controller types the top grid's search with it.
    keyboard: Option<crate::text_entry::TextEntry>,
    /// Item 31 (not retail, user 10-11): whether the colour wheel's pictures are made (the wheel: ColourWheel).
    wheel_art: bool,
    /// The screen failed to load or stopped: the classic customiser panel stands in for the rest of the session.
    broken: bool,
}

pub(crate) struct CreateSkaterPlugin;
impl Plugin for CreateSkaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .init_resource::<crate::graphic_creator::Creator>()
            .init_resource::<crate::colour_wheel::ColourWheel>()
            .add_systems(
                Update,
                (creator_frame, interact, animate)
                    .chain()
                    .after(crate::graphics_menu::interact)
                    .before(crate::app::FrameSet::Animation),
            )
            .add_systems(Update, resize_target);
    }
}

const MANIFEST: &str = "runtime/createskater.json";
const POPUP: &str = "runtime/sk8popup.json";
fn root(asset_root: &std::path::Path) -> PathBuf {
    std::env::var_os("SKATE_CREATE_SKATER_ROOT").map(PathBuf::from).unwrap_or_else(|| asset_root.join("private/create-skater"))
}
/// What setup reads, for the startup prefetch (apt_canvas::prefetch).
pub(crate) fn manifests(asset_root: &std::path::Path) -> [PathBuf; 2] {
    let root = root(asset_root);
    [root.join(MANIFEST), root.join("popup").join(POPUP)]
}

fn setup(
    mut commands: Commands,
    config: Res<Config>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    let Ok(output) = cameras.single() else { return };
    let root = root(&config.asset_root);
    let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -3);
    let result = (|| -> Result<(Value, Runtime), String> {
        let source: Value = match crate::apt_canvas::prefetched_json(&root.join(MANIFEST)) {
            Some(json) => json,
            None => serde_json::from_slice(
                &std::fs::read(root.join(MANIFEST)).map_err(|e| format!("{}: {e}", root.display()))?,
            )
            .map_err(|e| e.to_string())?,
        };
        if source["format"] != "skate3-create-skater" || source["version"] != 1 {
            return Err("Unsupported Create-a-Skater manifest".into());
        }
        // Run the movie once now, so a defective install falls back before the menu opens (and the first open
        // takes this load).
        let runtime = Runtime::load(&source, empty_page())?;
        for (path, size) in movie_textures(&source)? {
            canvas.preload(&mut images, &path, size)?;
        }
        Ok((source, runtime))
    })();
    let (source, runtime) = match result {
        Ok(loaded) => loaded,
        Err(error) => {
            warn!("Original Create-a-Skater screen unavailable ({error}); using the classic customiser panel");
            return;
        }
    };
    commands.spawn((
        Overlay,
        UiTargetCamera(output),
        GlobalZIndex(21),
        Pickable::IGNORE,
        MaterialNode(canvas.composite.clone()),
        Node { display: Display::None, position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
    ));
    // The popup is optional: without it gender switches without asking.
    let popup_root = root.join("popup");
    let popup = (|| -> Result<Popup, String> {
        let source: Value = match crate::apt_canvas::prefetched_json(&popup_root.join(POPUP)) {
            Some(json) => json,
            None => serde_json::from_slice(&std::fs::read(popup_root.join(POPUP)).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
        };
        crate::popup_runtime::Runtime::load(&source, "", &[], false)?;
        let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, popup_root.clone(), LAYER + 1, -4);
        for (path, size) in movie_textures(&source)? {
            canvas.preload(&mut images, &path, size)?;
        }
        Ok(Popup { source, canvas, runtime: None, accumulated: 0., asking: Ask::Gender })
    })()
    .map_err(|e| warn!("Original confirmation popup unavailable: {e}"))
    .ok();
    if let Some(popup) = &popup {
        commands.spawn((
            PopupOverlay,
            UiTargetCamera(output),
            GlobalZIndex(22),
            Pickable::IGNORE,
            MaterialNode(popup.canvas.composite.clone()),
            Node { display: Display::None, position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
        ));
    }
    let source = Arc::new(source);
    commands.insert_resource(Screen {
        spare: Spare::new(source.clone(), runtime),
        source,
        canvas,
        popup,
        art: None,
        catalog: Some({
            let path = crate::customiser_parts::asset_directory(&config.asset_root).join("catalog.json");
            std::thread::spawn(move || Art::read_catalog(&path))
        }),
        runtime: None,
        stack: Vec::new(),
        shown: false,
        closing: None,
        accumulated: 0.,
        keyboard: None,
        wheel_art: false,
        broken: false,
    });
    info!("Original Create-a-Skater screen loaded from {}", root.display());
}
fn empty_page() -> Page {
    Page { title: String::new(), mode: Mode::List, rows: Vec::new(), index: 0, accept: String::new(), back: String::new() }
}

/// A level's page, a grid's narrowed to its search (and the search, for the screen: grids only).
fn build(level: &Level, customiser: &Customiser, parts: &Parts, art: &Art) -> (cas_menu::Built, Option<String>) {
    let mut built = cas_menu::build(level.node, customiser, parts, art);
    if !level.node.is_grid() {
        return (built, None);
    }
    cas_menu::search(&mut built, &level.search);
    cas_menu::only_game(&mut built, cas_menu::GAMES[level.game]);
    // The title's filter names the game too ("Filter: Skate 2 tee").
    let filter = [cas_menu::GAMES[level.game], level.search.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    (built, Some(filter))
}
/// Present the top page (after opening, backing out or a choice that changes it).
fn show(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts) -> Result<(), String> {
    let art = screen.art.as_ref().ok_or("Create-a-Skater art missing")?;
    let level = screen.stack.last_mut().ok_or("Create-a-Skater page missing")?;
    let (mut built, search) = build(level, customiser, parts, art);
    if !level.node.is_grid() {
        built.page.index = level.row.min(built.page.rows.len().saturating_sub(1));
    }
    level.row = built.page.index;
    level.acts = built.acts;
    customiser.view = Some(built.frame.view());
    customiser.skate2 = Some(crate::cas_room::skate2_shot(level.node, built.frame));
    customiser.grip = matches!(level.node, cas_menu::Node::Grip);
    customiser.strip = built.strip;
    let legend = if level.node.by_game() { cas_menu::GAME_LEGEND.to_vec() } else { Vec::new() };
    screen.runtime.as_mut().ok_or("Create-a-Skater closed")?.show_with(built.page, search, legend)
}
/// Done in the Graphic Creator keeps what it left on the skater: the grids the creator was opened from
/// (and the page on top) are entered with this look from now on, so B on them does not take it off.
/// The root keeps its own, so Continue without saving still goes back to the skater as it was.
fn commit_look(stack: &mut [Level], draft: &Value) {
    let top = stack.len().saturating_sub(1);
    for (i, level) in stack.iter_mut().enumerate().skip(1) {
        if i == top || level.node.is_grid() {
            level.entered = draft.clone();
        }
    }
}
/// Show the open grid again with the highlight where it is (a Custom Graphics tile just finished).
fn reshow_grid(screen: &mut Screen, customiser: &Customiser, parts: &Parts) -> Result<(), String> {
    let art = screen.art.as_ref().ok_or("Create-a-Skater art missing")?;
    let level = screen.stack.last_mut().ok_or("Create-a-Skater page missing")?;
    let runtime = screen.runtime.as_mut().ok_or("Create-a-Skater closed")?;
    let (mut built, search) = build(level, customiser, parts, art);
    built.page.index = runtime.bindings.page.index.min(built.page.rows.len().saturating_sub(1));
    level.row = built.page.index;
    level.acts = built.acts;
    let legend = if level.node.by_game() { cas_menu::GAME_LEGEND.to_vec() } else { Vec::new() };
    runtime.show_with(built.page, search, legend)
}
/// Delete save `n` of the item at `zone` (its page is on top), and go back to the grid. The skater
/// goes back to the look the grid was entered with (blank where that wore the deleted save).
fn delete_saved(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts, pause: &mut Menu, zone: &'static str, n: usize) -> Result<(), String> {
    let grid = screen.stack.len().checked_sub(2);
    let mut entered = grid.map_or_else(|| customiser.draft.clone(), |g| screen.stack[g].entered.clone());
    cas_menu::delete_saved(customiser, parts, zone, n, &mut entered);
    if let Some(grid) = grid {
        screen.stack[grid].entered = entered;
    }
    pop(screen, customiser, parts, pause)
}
/// Refresh the highlighted row's value in place (sliders, swatches, selectors).
fn refresh(screen: &mut Screen, customiser: &Customiser, parts: &Parts) -> Result<(), String> {
    let art = screen.art.as_ref().ok_or("Create-a-Skater art missing")?;
    let Some(level) = screen.stack.last_mut() else { return Ok(()) };
    let (built, _) = build(level, customiser, parts, art);
    let Some(runtime) = screen.runtime.as_mut() else { return Ok(()) };
    let index = runtime.bindings.page.index;
    level.acts = built.acts;
    match built.page.rows.get(index) {
        Some(row) => runtime.refresh_row(row.clone()),
        None => Ok(()),
    }
}
/// Item 31 (not retail, user 10-11): the colour wheel a press on row `row` opens (A, Enter, a click), if it is
/// the colour row.
fn wheel_for(acts: &[Act], row: usize) -> Option<cas_menu::Custom> {
    match acts.get(row) {
        Some(Act::Wheel(custom)) => Some(custom.clone()),
        _ => None,
    }
}
/// Where an edit (Morph) page's first row starts on the stage: its rows never scroll, each as tall as the movie lays
/// its kind out (graphic_pages::row_height).
const MORPH_TOP: f32 = 112.;
/// Item 31: the edit page row under `at` (stage units), for a click.
fn row_under(rows: &[crate::create_skater_runtime::Row], at: Vec2) -> Option<usize> {
    let card = crate::graphic_board::CAS_CARD;
    if !(card[0]..=card[2]).contains(&at.x) {
        return None;
    }
    let mut top = MORPH_TOP;
    rows.iter().position(|r| {
        let bottom = top + crate::graphic_pages::row_height(r.kind);
        let hit = (top..bottom).contains(&at.y);
        top = bottom;
        hit
    })
}
/// Item 31: open the colour wheel on the page's colour row `row`. The movie shows that row alone on the page's card,
/// the wheel under it; closing shows the page again on the row.
fn open_wheel(screen: &mut Screen, colour: &mut crate::colour_wheel::ColourWheel, customiser: &Customiser, row: usize, custom: cas_menu::Custom) -> Result<(), String> {
    let runtime = screen.runtime.as_mut().ok_or("Create-a-Skater closed")?;
    let page = crate::colour_wheel::page_for(&runtime.bindings.page, row, custom.start);
    if let Some(level) = screen.stack.last_mut() {
        level.row = row;
    }
    colour.0 = Some(crate::colour_wheel::Open::new(customiser, custom));
    runtime.show_with(page, None, vec![("Y", "ID_CAC_CUSTOMGRAPHIC_RESET")])
}
fn push(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts, node: Page_) -> Result<(), String> {
    if let Some(level) = screen.stack.last_mut() {
        level.row = screen.runtime.as_ref().map_or(0, |r| r.bindings.page.index);
    }
    screen.stack.push(Level { node, acts: Vec::new(), entered: customiser.draft.clone(), row: 0, search: String::new(), game: 0 });
    show(screen, customiser, parts)
}
/// Leave the top page. The root saves the skater and ends editing.
fn pop(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts, pause: &mut Menu) -> Result<(), String> {
    screen.stack.pop();
    if screen.stack.is_empty() {
        customiser.save_and_close();
        if !customiser.open {
            pause.cas_closed();
        }
        return Ok(());
    }
    show(screen, customiser, parts)
}
impl Art {
    pub(crate) fn load(path: &std::path::Path, library: &Library, source: &Value, assets: &std::path::Path) -> Result<Self, String> {
        Ok(Self::build(Self::read_catalog(path)?, library, source, assets))
    }
    fn read_catalog(path: &std::path::Path) -> Result<Catalog, String> {
        serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())
    }
}
/// After choosing merchandise the original returns to the Merchandise list
/// (past Accessories/Skateboards and any Edit Clothing page). A grid outside the shop (tattoos) returns
/// to the page it opened from; popping down to the root would have saved and left Edit Skater.
fn back_to_shop(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts, pause: &mut Menu) -> Result<(), String> {
    if !screen.stack.iter().any(|l| l.node == Page_::Merch) {
        return pop(screen, customiser, parts, pause);
    }
    while screen.stack.last().is_some_and(|l| l.node != Page_::Merch) {
        screen.stack.pop();
    }
    show(screen, customiser, parts)
}
/// Leave a graphic or tattoo flow (Graphic Adjust's A, Remove Logo) for the page that
/// started it: Edit Clothing, or the tattoo placements.
fn leave_flow(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts) -> Result<(), String> {
    while screen.stack.len() > 1 && screen.stack.last().is_some_and(|l| l.node.in_graphic_flow()) {
        screen.stack.pop();
    }
    show(screen, customiser, parts)
}
/// Graphic Adjust, as the movie's legend (mScalemenu / mTattoomenu) shows it: right stick
/// moves, left stick scales, LB / RB turn, Y resets, and on tattoos X takes the clothing
/// off and on. The D-pad moves too. Keyboard: arrows, W / S, Q / E, Y, X.
/// Retail's steps (0x825fbae8 into 0x82c2e698), per frame while an input is past 0.01:
/// turn 0.01 rad, move 0.005 UV, scale s -= 0.1 x stick (up grows; size = start scale / s),
/// then retail's fit (`custom_graphic::fit`), so holding past a zone edge stores nothing the
/// garment doesn't show and the reverse answers at once. `frames`: whole retail frames since
/// the last call (stepping whole frames keeps each step on the profile's 0.001 grid).
pub(crate) fn adjust_graphic(node: Page_, pad: Option<(u16, i16, [i16; 2])>, keys: &ButtonInput<KeyCode>, pressed: u16, frames: f32, customiser: &mut Customiser, parts: &Parts) {
    use crate::custom_graphic::{self as graphic, ADJUST, Adjust};
    use serde_json::json;
    let (p, z) = match node {
        Page_::Graphic(zone) => ("graphics", zone),
        Page_::TattooAdjust(upper) => ("tattoos", if upper { "Arm" } else { "Leg" }),
        _ => return,
    };
    if pressed & 0x8000 != 0 || keys.just_pressed(KeyCode::KeyY) {
        let reset: serde_json::Map<String, Value> = ADJUST.iter().map(|a| (a.0.into(), Value::Null)).collect();
        customiser.apply(parts, Some(&json!({ p: { z: reset } })));
        return;
    }
    if let Page_::TattooAdjust(upper) = node {
        if pressed & 0x4000 != 0 || keys.just_pressed(KeyCode::KeyX) {
            customiser.strip = if customiser.strip.is_some() { None } else { Some(if upper { "OuterTorso" } else { "Pants" }) };
        }
    }
    let (buttons, left_y, right) = pad.unwrap_or_default();
    // XInput's recommended dead zones (left 7849, right 8689) stand in for retail's pad layer.
    let stick = |v: i16, dead: f32| {
        let v = (v as f32 / 32767.).clamp(-1., 1.);
        if v.abs() < dead { 0. } else { v }
    };
    let held = |bit: u16| f32::from(u8::from(buttons & bit != 0));
    let key = |plus: KeyCode, minus: KeyCode| f32::from(u8::from(keys.pressed(plus))) - f32::from(u8::from(keys.pressed(minus)));
    // Retail's threshold, then a full deflection at most. LB turns counter-clockwise (+angle).
    let input = |v: f32| if v.abs() > 0.01 { v.clamp(-1., 1.) } else { 0. };
    let [right_x, up, grow, turn] = [
        stick(right[0], 0.265) + held(8) - held(4) + key(KeyCode::ArrowRight, KeyCode::ArrowLeft),
        stick(right[1], 0.265) + held(1) - held(2) + key(KeyCode::ArrowUp, KeyCode::ArrowDown),
        stick(left_y, 0.24) + key(KeyCode::KeyW, KeyCode::KeyS),
        held(0x100) - held(0x200) + key(KeyCode::KeyQ, KeyCode::KeyE),
    ]
    .map(input);
    if [right_x, up, grow, turn] == [0.; 4] || frames < 1. {
        return;
    }
    let (draft, female) = (&customiser.draft, customiser.gender() == "female");
    let entry = &draft[p][z];
    let worn = || draft["selections"].as_object().into_iter().flatten().map(|(_, w)| w);
    // Retail's start entry, the zone rectangle and the art box, as customiser_parts draws it.
    let placed = match node {
        Page_::Graphic(_) => {
            let rect = worn().flat_map(|w| cas_menu::graphic_locations(parts, w)).find(|l| l.0 == z).map(|l| l.1);
            // Only a stock logo is adjusted; a design fills its whole side.
            let art = graphic::logo(entry).and_then(|id| parts.library.graphics.get(id)).map(|g| g.bounds);
            rect.zip(art).map(|(rect, art)| (graphic::entry(z), rect, art))
        }
        // The skin model's rectangle for the spot (customiser_parts' tattoo slots), else its quadrant.
        // ponytail: retail reads CAS slot 0 or 23 (upper) by a byte not decoded.
        _ => graphic::tattoo(z, entry["side"].as_u64().unwrap_or(0)).zip(entry["id"].as_str().and_then(|id| parts.library.tattoos.get(id))).map(|((e, key), t)| {
            let skin_slot = |m: &crate::customiser_parts::Part| (if m.slot == "OuterTorso" && m.flag("TopType").is_empty() { "Arm" } else { m.slot.as_str() }) == z;
            let skin = worn().filter_map(|w| parts.library.models.get(w["asset_id"].as_str()?)).find(|m| skin_slot(m));
            (e, skin.and_then(|m| graphic::rectangle(m.flag(key))).unwrap_or(graphic::quarter(e)), t.bounds)
        }),
    };
    let Some((start_entry, rect, art)) = placed else { return };
    let mut a = Adjust::of(entry);
    let [row, _] = graphic::stamp(start_entry, female, a);
    let s = Vec2::new(row.x, row.y).length();
    let start = s * a.size;
    let s = (s - 0.1 * grow * frames).max(start / ADJUST[2].2 as f32);
    let angle = a.angle + (0.01 * turn * frames).to_degrees();
    // Adjust::of clamps to the profile's ranges; the turn wraps first.
    let v = json!({ "x": a.x + 0.005 * right_x * frames, "y": a.y + 0.005 * up * frames, "size": start / s, "angle": (angle + 180.).rem_euclid(360.) - 180. });
    a = graphic::fit(start_entry, female, rect, art, Adjust::of(&v));
    customiser.apply(parts, Some(&json!({ p: { z: { "x": a.x, "y": a.y, "size": a.size, "angle": a.angle } } })));
}
fn switch_gender(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts) -> Result<(), String> {
    let gender = if customiser.gender() == "female" { "male" } else { "female" };
    customiser.apply(parts, Some(&serde_json::json!({ "gender": gender })));
    show(screen, customiser, parts)
}
/// The startup worker's catalog (waiting for it if it's still reading), else read now.
fn load_art(config: &Config, parts: &Parts, source: &Value, catalog: Option<std::thread::JoinHandle<Result<Catalog, String>>>) -> Result<Art, String> {
    if let Some(Ok(catalog)) = catalog.map(|w| w.join().unwrap_or_else(|_| Err("the catalog loader panicked".into()))) {
        return Ok(Art::build(catalog, &parts.library, source, &config.asset_root));
    }
    Art::load(
        &crate::customiser_parts::asset_directory(&config.asset_root).join("catalog.json"),
        &parts.library,
        source,
        &config.asset_root,
    )
}

#[allow(clippy::too_many_arguments)]
fn interact(
    screen: Option<ResMut<Screen>>,
    mut customiser: ResMut<Customiser>,
    parts: Res<Parts>,
    config: Res<Config>,
    mut pause: ResMut<Menu>,
    (keys, mut typing, mouse, windows, mut colour): (
        Res<ButtonInput<KeyCode>>,
        MessageReader<bevy::input::keyboard::KeyboardInput>,
        Option<Res<ButtonInput<MouseButton>>>,
        Query<&Window, With<PrimaryWindow>>,
        ResMut<crate::colour_wheel::ColourWheel>,
    ),
    nav: Res<crate::customiser::Navigation>,
    mut pose: ResMut<crate::cas_pose::CasPose>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    mut creator: ResMut<crate::graphic_creator::Creator>,
    time: Res<Time<Real>>,
    mut pads: Local<[crate::input::platform::CapabilityCache; 4]>,
    mut retail_frames: Local<f32>,
    mut tiles: ResMut<crate::graphic_tile::Tiles>,
    mut tiles_asked: Local<Option<&'static str>>,
    active: Option<Res<crate::input::ActivePad>>,
) {
    // What the PC keyboard typed this frame (read every frame, so nothing typed under a popup arrives late).
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let typed: String = typing
        .read()
        .filter(|e| e.state == bevy::input::ButtonState::Pressed && !control)
        .filter_map(|e| e.text.as_deref())
        .flat_map(str::chars)
        .filter(|c| !c.is_control())
        .collect();
    let Some(mut screen) = screen else { return };
    let screen = &mut *screen;
    if customiser.fallback != screen.broken {
        customiser.fallback = screen.broken;
        customiser.redraw = true;
    }
    if screen.runtime.is_none() {
        if !customiser.open || customiser.original || screen.closing.is_some() || screen.broken {
            return;
        }
        if screen.art.is_none() {
            match load_art(&config, &parts, &screen.source, screen.catalog.take()) {
                Ok(art) => screen.art = Some(art),
                Err(error) => {
                    // ponytail: without the catalog every part grid is empty.
                    warn!("Create-a-Skater thumbnails unavailable: {error}");
                    screen.art = Some(Art::default());
                }
            }
        }
        match screen.spare.take() {
            Ok(runtime) => {
                screen.runtime = Some(runtime);
                screen.shown = false;
                screen.accumulated = 0.;
                // A restart under the Graphic Creator keeps the look Edit Skater opened with (the draft is the
                // creator's now): Continue without saving still goes back to it.
                let entered = match screen.stack.first() {
                    Some(root) if creator.is_open() => root.entered.clone(),
                    _ => customiser.draft.clone(),
                };
                screen.stack = vec![Level { node: Page_::Root, acts: Vec::new(), entered, row: 0, search: String::new(), game: 0 }];
                colour.0 = None;
                customiser.original = true;
            }
            Err(error) => {
                error!("Original Create-a-Skater screen failed to open: {error}");
                screen.broken = true;
                customiser.fallback = true;
                customiser.redraw = true;
            }
        }
        return;
    }
    if !customiser.open {
        // Closed (saved at the root, or elsewhere): play the outro.
        if screen.closing.is_none() {
            screen.closing = Some(0.);
            customiser.view = None;
            customiser.skate2 = None;
            customiser.grip = false;
            customiser.strip = None;
            if let Some(Err(error)) = screen.runtime.as_mut().map(Runtime::close) {
                warn!("Create-a-Skater outro: {error}");
            }
        }
        return;
    }
    if !screen.shown || screen.closing.is_some() {
        return;
    }
    // The Graphic Creator takes every press; the press that closes it is not ours.
    if creator.is_open() {
        return;
    }
    // Custom Graphics: draw the tiles the page is missing once, and show them as they finish.
    let drawn = std::mem::take(&mut tiles.finished);
    match screen.stack.last().map(|l| l.node) {
        Some(Page_::GraphicGrid(zone, cas_menu::CUSTOM)) => {
            if *tiles_asked != Some(zone) {
                *tiles_asked = Some(zone);
                for job in cas_menu::custom_jobs(&customiser, &parts, zone) {
                    tiles.request(job);
                }
            }
            if !drawn.is_empty() {
                if let Err(error) = reshow_grid(screen, &customiser, &parts) {
                    error!("Custom Graphics tiles: {error}");
                }
            }
        }
        _ => *tiles_asked = None,
    }
    if std::mem::take(&mut creator.closed) {
        *tiles_asked = None;
        // Done keeps what is worn: the grids the creator was opened from (and its page) go back to this
        // look on B, not to the one before it. The root keeps its own, so Continue without saving still works.
        if std::mem::take(&mut creator.committed) {
            commit_look(&mut screen.stack, &customiser.draft);
        }
        if let Err(error) = show(screen, &mut customiser, &parts) {
            error!("Edit Skater after the Graphic Creator: {error}");
        }
        return;
    }
    // An open popup takes every press until the player answers.
    if let Some(popup) = screen.popup.as_mut().filter(|p| p.runtime.is_some()) {
        let keys_down = [
            (nav.pressed & 1 != 0, Key::Up),
            (nav.pressed & 2 != 0, Key::Down),
            (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
            (keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
        ];
        for (_, key) in keys_down.into_iter().filter(|(down, _)| *down) {
            let Some(runtime) = popup.runtime.as_mut() else { break };
            let input = runtime.input(key);
            sounds.write_batch(runtime.bindings.sounds.drain(..).map(FrontEndSoundRequest));
            match input {
                Ok(None) => {}
                Ok(Some(choice)) => {
                    popup.runtime = None;
                    let asking = popup.asking;
                    let result = match (asking, choice) {
                        (Ask::Gender, 0) => switch_gender(screen, &mut customiser, &parts),
                        (Ask::Delete(zone, n), 0) => delete_saved(screen, &mut customiser, &parts, &mut pause, zone, n),
                        (Ask::Exit, 0) => pop(screen, &mut customiser, &parts, &mut pause),
                        (Ask::Exit, 1) => {
                            // Back to the skater as Edit Skater found it; nothing is written.
                            if let Some(level) = screen.stack.first() {
                                customiser.draft = level.entered.clone();
                            }
                            screen.stack.clear();
                            customiser.close_without_saving();
                            pause.cas_closed();
                            Ok(())
                        }
                        _ => Ok(()),
                    };
                    if let Err(error) = result {
                        error!("Edit Skater popup: {error}");
                    }
                    break;
                }
                Err(error) => {
                    error!("Confirmation popup stopped: {error}");
                    popup.runtime = None;
                    break;
                }
            }
        }
        return;
    }
    // Item 31 (not retail, user 10-11): the colour wheel takes every press (LT / RT still turn the skater,
    // user 10-13) until A keeps the colour or B puts back what the row had; Y is the item's own colour. Pad: left stick,
    // right stick up / down or D-pad Left / Right; keyboard: arrows / WASD, Q / E, Y, Enter / Escape; mouse: click and drag on the
    // disc and the bar.
    if let Some(open) = colour.0.as_mut() {
        let pad = crate::input::platform::active_packet(active.as_deref().and_then(|a| a.0), &mut pads).map(|p| p.state);
        let (stick, brightness) = crate::colour_wheel::controls(pad.as_ref(), &keys);
        open.steer(stick, brightness, time.delta_secs());
        let cursor = windows.single().ok().and_then(|w| Some(crate::graphic_creator::to_movie(w.cursor_position()?, w.size())));
        if let (Some(mouse), Some(at)) = (mouse.as_deref(), cursor) {
            open.mouse(at, mouse.just_pressed(MouseButton::Left), mouse.pressed(MouseButton::Left));
        }
        let reset = nav.pressed & 0x8000 != 0 || keys.just_pressed(KeyCode::KeyY);
        if reset {
            open.default(&mut customiser, &parts);
            sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
        }
        // The row's swatch shows the colour as it changes.
        if open.preview(&mut customiser, &parts) || reset {
            if let Some(runtime) = screen.runtime.as_mut() {
                if let Some(mut row) = runtime.bindings.page.rows.first().cloned() {
                    row.colours = vec![cas_menu::SWATCH(open.wheel.rgb())];
                    if let Err(error) = runtime.refresh_row(row) {
                        error!("Edit Skater colour wheel: {error}");
                    }
                }
            }
        }
        let accept = keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0;
        let cancel = keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0;
        if accept || cancel {
            if let Some(open) = colour.0.take().filter(|_| cancel) {
                open.cancel(&mut customiser);
            }
            sounds.write(FrontEndSoundRequest(if cancel { SOUND_B_BUTTON } else { SOUND_A_BUTTON }));
            // The page again, on the colour row (Grip's Style row comes and goes with its colour).
            if let Err(error) = show(screen, &mut customiser, &parts) {
                error!("Edit Skater colour wheel: {error}");
            }
        }
        return;
    }
    // Item 11 (ours): a grid's search and jump to top. The PC keyboard types straight into the search
    // (Backspace erases, Escape clears); a controller opens the on-screen keyboard with X; Y (Home) goes to the
    // first tile. B clears a search before it leaves, as the classic panel's does.
    let grid = screen.stack.last().is_some_and(|l| l.node.is_grid());
    if !grid {
        screen.keyboard = None;
    }
    if let Some(entry) = screen.keyboard.as_mut() {
        use crate::{boot_flow::Key as Pad, text_entry::Typed};
        entry.type_text(&typed);
        if keys.just_pressed(KeyCode::Backspace) {
            entry.backspace();
        }
        let mut answer = keys.just_pressed(KeyCode::Enter).then_some(Typed::Done).or(keys.just_pressed(KeyCode::Escape).then_some(Typed::Cancel));
        for (bit, key) in [(1, Pad::Up), (2, Pad::Down), (4, Pad::Left), (8, Pad::Right), (0x10, Pad::Start), (0x1000, Pad::A), (0x2000, Pad::B), (0x4000, Pad::X), (0x8000, Pad::Y)] {
            if nav.pressed & bit != 0 {
                answer = answer.or(entry.input(key));
            }
        }
        let text = entry.text.clone();
        match answer {
            Some(Typed::Done) => {
                screen.keyboard = None;
                if let Some(level) = screen.stack.last_mut() {
                    level.search = text;
                }
                if let Err(error) = show(screen, &mut customiser, &parts) {
                    error!("Edit Skater search: {error}");
                }
            }
            Some(Typed::Cancel) => screen.keyboard = None,
            None => {}
        }
        return;
    }
    let mut back = keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0;
    if let Some(level) = screen.stack.last_mut().filter(|_| grid) {
        let before = (level.search.clone(), level.game);
        if level.node.by_game() {
            level.game = cas_menu::next_game(level.game, nav.pressed & 0x200 != 0, nav.pressed & 0x100 != 0);
        }
        if !level.search.is_empty() && (keys.just_pressed(KeyCode::Escape) || nav.pressed & 0x2000 != 0) {
            level.search.clear();
            back = false;
        } else if !level.search.is_empty() && keys.just_pressed(KeyCode::Backspace) {
            level.search.pop();
            back = false;
        }
        for c in typed.chars() {
            if level.search.chars().count() < cas_menu::SEARCH_MAX {
                level.search.push(c);
            }
        }
        let open_keyboard = nav.pressed & 0x4000 != 0;
        if open_keyboard {
            screen.keyboard = Some(crate::text_entry::TextEntry::new(&level.search, cas_menu::SEARCH_MAX));
        }
        let result = if (level.search.clone(), level.game) != before {
            show(screen, &mut customiser, &parts)
        } else if nav.pressed & 0x8000 != 0 || keys.just_pressed(KeyCode::Home) {
            top(screen, &mut customiser, &parts)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            error!("Edit Skater search: {error}");
        }
        if open_keyboard {
            return;
        }
    }
    if let Some(node) = screen.stack.last().map(|l| l.node).filter(|n| n.is_adjust()) {
        let pad = crate::input::platform::active_packet(active.and_then(|a| a.0), &mut pads).map(|p| (p.state.buttons, p.state.left[1], p.state.right));
        // Retail steps Graphic Adjust once a frame at Skate 3's 30 fps (INFERRED: not read from the exe).
        *retail_frames = (*retail_frames + time.delta_secs() * 30.).min(2.);
        let frames = retail_frames.floor();
        *retail_frames -= frames;
        adjust_graphic(node, pad, &keys, nav.pressed, frames, &mut customiser, &parts);
    }
    // Item 31 (not retail, user 10-11): a click on the colour row opens its colour wheel; so does X anywhere on the page.
    if !grid {
        let x = nav.pressed & 0x4000 != 0 || keys.just_pressed(KeyCode::KeyX);
        let click = mouse.as_deref().filter(|m| m.just_pressed(MouseButton::Left)).and_then(|_| {
            let w = windows.single().ok()?;
            Some(crate::graphic_creator::to_movie(w.cursor_position()?, w.size()))
        });
        let row = match (screen.stack.last(), screen.runtime.as_ref()) {
            (Some(level), Some(runtime)) => {
                let colour_row = level.acts.iter().position(|a| matches!(a, Act::Wheel(_)));
                click.and_then(|at| row_under(&runtime.bindings.page.rows, at)).filter(|r| Some(*r) == colour_row).or(colour_row.filter(|_| x))
            }
            _ => None,
        };
        if let Some((row, custom)) = row.and_then(|r| Some((r, screen.stack.last().and_then(|l| wheel_for(&l.acts, r))?))) {
            sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
            if let Err(error) = open_wheel(screen, &mut colour, &customiser, row, custom) {
                error!("Edit Skater colour wheel: {error}");
            }
            return;
        }
    }
    // Arrow keys arrive through the navigation bits (keyboard included, with repeat).
    let pressed = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0, Key::Left),
        (nav.pressed & 8 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (back, Key::Back),
    ];
    for (_, key) in pressed.into_iter().filter(|(down, _)| *down) {
        let Some(runtime) = screen.runtime.as_mut() else { return };
        let Some(node) = screen.stack.last().map(|l| l.node) else { return };
        let outcome = runtime.input(key);
        sounds.write_batch(runtime.bindings.sounds.drain(..).map(FrontEndSoundRequest));
        // B is native (the movie never sees it): core_b_button.
        if matches!(outcome, Ok(Some(Outcome::Back))) {
            sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON));
        }
        let act =|screen: &Screen, row: usize| screen.stack.last().and_then(|l| l.acts.get(row).cloned());
        let result = match outcome {
            Err(error) => Err(error),
            Ok(None) => Ok(()),
            Ok(Some(Outcome::Hilight(row))) if node == Page_::Style && row < 4 => {
                if let Some(label) = cas_menu::gesture_on(&customiser, cas_menu::GESTURE_ROWS[row]) {
                    pose.preview(label);
                }
                Ok(())
            }
            Ok(Some(Outcome::Hilight(row))) => {
                if node.is_grid() {
                    preview(screen, &mut customiser, &parts, row);
                }
                Ok(())
            }
            Ok(Some(Outcome::Adjust(row, direction))) => match act(screen, row) {
                // Item 31 (not retail, user 10-11): Left / Right on a colour row opens its colour wheel.
                Some(Act::Wheel(custom)) => open_wheel(screen, &mut colour, &customiser, row, custom),
                Some(a) if cas_menu::step(&mut customiser, &parts, &a, direction) => {
                    // Choosing a gesture plays it, as the retail Edit Style page does.
                    if node == Page_::Style && row < 4 {
                        if let Some(label) = cas_menu::gesture_on(&customiser, cas_menu::GESTURE_ROWS[row]) {
                            pose.preview(label);
                        }
                    }
                    refresh(screen, &customiser, &parts)
                }
                _ => Ok(()),
            },
            Ok(Some(Outcome::Accept(row))) => match act(screen, row) {
                Some(Act::Open(next)) => push(screen, &mut customiser, &parts, next),
                // Item 31 (not retail, user 10-11): A on a colour row opens its colour wheel.
                Some(Act::Wheel(custom)) => open_wheel(screen, &mut colour, &customiser, row, custom),
                Some(Act::Item(patch, next)) => {
                    customiser.apply(&parts, Some(&patch));
                    // An item with no colours or styles to edit (some ported decks) is simply worn.
                    push(screen, &mut customiser, &parts, next).and_then(|()| match screen.stack.last() {
                        Some(level) if level.acts.is_empty() && matches!(level.node, Page_::Garment(..)) => {
                            back_to_shop(screen, &mut customiser, &parts, &mut pause)
                        }
                        _ => Ok(()),
                    })
                }
                Some(Act::Gender) => match screen.popup.as_mut() {
                    // As the original: confirm first; clothing is kept for switching back.
                    Some(popup) => popup.ask(Ask::Gender, "ID_CAC_GENDERSWITCH_CONFIRMATION", &["ID_COMMON_YES", "ID_COMMON_NO"], false),
                    None => switch_gender(screen, &mut customiser, &parts),
                },
                Some(Act::Undo) => {
                    if let Some(level) = screen.stack.last() {
                        customiser.draft = level.entered.clone();
                    }
                    show(screen, &mut customiser, &parts)
                }
                Some(Act::Choose(patch)) => {
                    let tattoo = patch.as_ref().is_some_and(|p| p["tattoos"].as_object().is_some_and(|t| t.values().any(|e| e["id"].is_string())));
                    customiser.apply(&parts, patch.as_ref());
                    match node {
                        // A logo or tattoo goes on: Graphic Adjust, whose B (Change Graphic)
                        // comes back to this grid.
                        Page_::GraphicGrid(zone, _) => push(screen, &mut customiser, &parts, Page_::Graphic(zone)),
                        Page_::TattooGrid(upper, ..) if tattoo => push(screen, &mut customiser, &parts, Page_::TattooAdjust(upper)),
                        // Remove Logo.
                        Page_::GraphicKind(_) => leave_flow(screen, &mut customiser, &parts),
                        // Select on a saved Custom Graphic: it is worn (the highlight put it on); leave the flow.
                        Page_::SavedGraphic(..) => leave_flow(screen, &mut customiser, &parts),
                        _ if node.is_grid() => back_to_shop(screen, &mut customiser, &parts, &mut pause),
                        _ => show(screen, &mut customiser, &parts),
                    }
                }
                Some(Act::CreatorNew(zone)) => {
                    creator.open_new(zone, &mut customiser, &parts);
                    Ok(())
                }
                Some(Act::CreatorSaved(zone, n)) => {
                    creator.open_saved(zone, &mut customiser, &parts, n);
                    Ok(())
                }
                Some(Act::CopyCode(zone, n)) => {
                    let copied = cas_menu::saved_code(&customiser, &parts, zone, n).is_some_and(|code| crate::share_code::copy_to_clipboard(&code).is_ok());
                    let said = if copied { "#Code copied. Paste it on New's Start From for the same item." } else { "#That code could not be copied." };
                    screen.popup.as_mut().map_or(Ok(()), |popup| popup.ask(Ask::Copied, said, &["ID_COMMON_OK"], true))
                }
                Some(Act::DeleteSaved(zone, n)) => match screen.popup.as_mut() {
                    Some(popup) => popup.ask(Ask::Delete(zone, n), "#Delete this Custom Graphic?", &["ID_COMMON_YES", "ID_COMMON_NO"], true),
                    // Without the popup art nothing is thrown away unasked.
                    None => Ok(()),
                },
                // Graphic Adjust's A keeps the graphic and leaves the flow.
                _ if node.is_adjust() => leave_flow(screen, &mut customiser, &parts),
                // A on an edit page accepts its changes and returns; Edit Clothing
                // returns past its grid to Merchandise, as the original does.
                // Even an edit page with no rows: A is its only way out.
                _ if node.is_editor() => {
                    if matches!(node, Page_::Garment(..)) {
                        back_to_shop(screen, &mut customiser, &parts, &mut pause)
                    } else {
                        pop(screen, &mut customiser, &parts, &mut pause)
                    }
                }
                Some(a) if cas_menu::step(&mut customiser, &parts, &a, 1) => refresh(screen, &customiser, &parts),
                _ => Ok(()),
            },
            // Only pages with a shown B get here (Runtime::input): edit pages and graphic
            // location/categories hide it and retail ignores B there; the tattoo categories show it (user
            // 10-30). (Graphic Adjust's B is Change Graphic: a plain pop, below.)
            Ok(Some(Outcome::Back)) => {
                if let (1, Some(popup)) = (screen.stack.len(), screen.popup.as_mut()) {
                    // Leaving Edit Skater asks first, as the original does.
                    popup.ask(Ask::Exit, "ID_CAC_EXIT_CAC_CONFIRMATION_NO_SAVE", &["ID_COMMON_SAVE_CONTINUE", "ID_COMMON_CONTINUE_NO_SAVE"], true)
                } else if matches!(node, Page_::Garment(..)) {
                    // Item 31: Edit Clothing shows B when its colour row's A opens the colour wheel; B leaves as A
                    // does on its other rows, past the grid to Merchandise.
                    back_to_shop(screen, &mut customiser, &parts, &mut pause)
                } else {
                    // A grid, or a graphic's location / kind list: B leaves with nothing chosen.
                    if node.is_grid() || matches!(node, Page_::GraphicKind(_) | Page_::GraphicLocation) {
                        if let Some(level) = screen.stack.last() {
                            customiser.draft = level.entered.clone();
                        }
                    }
                    pop(screen, &mut customiser, &parts, &mut pause)
                }
            }
        };
        if let Err(error) = result {
            // Never strand the player: continue in the classic customiser panel.
            error!("Original Create-a-Skater screen stopped: {error}");
            screen.runtime = None;
            screen.broken = true;
            customiser.fallback = true;
            customiser.original = false;
            customiser.view = None;
            customiser.skate2 = None;
            customiser.grip = false;
            customiser.strip = None;
            customiser.redraw = true;
            return;
        }
    }
}

/// Highlighting a tile previews it on the skater, as in the original.
fn preview(screen: &Screen, customiser: &mut Customiser, parts: &Parts, row: usize) {
    match screen.stack.last().and_then(|l| l.acts.get(row)) {
        Some(Act::Choose(patch)) => customiser.apply(parts, patch.as_ref()),
        Some(Act::Item(patch, _)) => customiser.apply(parts, Some(patch)),
        // New: the skater is back as the page found it.
        Some(Act::CreatorNew(_)) => {
            if let Some(level) = screen.stack.last() {
                customiser.draft = level.entered.clone();
                customiser.redraw = true;
            }
        }
        _ => {}
    }
}
/// Item 11: the highlight to the grid's first tile, previewed as a move there is.
fn top(screen: &mut Screen, customiser: &mut Customiser, parts: &Parts) -> Result<(), String> {
    let Some(runtime) = screen.runtime.as_mut().filter(|r| r.bindings.page.index != 0) else { return Ok(()) };
    runtime.bindings.page.index = 0;
    reshow_grid(screen, customiser, parts)?;
    preview(screen, customiser, parts, 0);
    Ok(())
}

fn resize_target(
    screen: Option<ResMut<Screen>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut screen) = screen {
        screen.canvas.resize(&window, &mut images, &mut composites);
        if let Some(popup) = screen.popup.as_mut() {
            popup.canvas.resize(&window, &mut images, &mut composites);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn animate(
    mut commands: Commands,
    screen: Option<ResMut<Screen>>,
    mut customiser: ResMut<Customiser>,
    parts: Res<Parts>,
    real: Res<Time<Real>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<&mut Camera>,
    mut overlays: Query<&mut Node, (With<Overlay>, Without<PopupOverlay>)>,
    mut popup_overlays: Query<&mut Node, (With<PopupOverlay>, Without<Overlay>)>,
    creator: Res<crate::graphic_creator::Creator>,
    colour: Res<crate::colour_wheel::ColourWheel>,
) {
    let Some(mut screen) = screen else { return };
    let screen = &mut *screen;
    if let Some(popup) = screen.popup.as_mut() {
        if !customiser.open {
            popup.runtime = None;
        }
        if let Some(runtime) = popup.runtime.as_mut() {
            popup.accumulated = (popup.accumulated + real.delta_secs()).min(4. * FRAME);
            while popup.accumulated >= FRAME {
                popup.accumulated -= FRAME;
                if let Err(error) = runtime.advance() {
                    error!("Confirmation popup stopped: {error}");
                    popup.runtime = None;
                    break;
                }
            }
        }
        let active = popup.runtime.is_some();
        popup.canvas.set_active(active, &mut cameras);
        for mut node in &mut popup_overlays {
            let display = if active { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
        let draws = match popup.runtime.as_ref().map(|r| crate::apt_scene::draw(&r.bindings.movie, &r.vm, &r.bindings.shapes)) {
            Some(Ok(draws)) => draws,
            Some(Err(error)) => {
                error!("Confirmation popup geometry: {error}");
                Vec::new()
            }
            None => Vec::new(),
        };
        popup.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
    }
    if screen.runtime.is_some() {
        screen.accumulated = (screen.accumulated + real.delta_secs()).min(4. * FRAME);
        let mut result = Ok(());
        while screen.accumulated >= FRAME && result.is_ok() {
            screen.accumulated -= FRAME;
            result = screen.runtime.as_mut().map_or(Ok(()), Runtime::advance);
            if result.is_ok() && !screen.shown && screen.runtime.as_ref().is_some_and(|r| r.bindings.intro_complete) {
                screen.shown = true;
                result = show(screen, &mut customiser, &parts);
            }
        }
        if let Some(closing) = screen.closing.as_mut() {
            *closing += real.delta_secs();
            if screen.runtime.as_ref().is_some_and(|r| r.bindings.outro_complete) || *closing >= CLOSE_TIMEOUT {
                screen.runtime = None;
                screen.closing = None;
                customiser.original = false;
            }
        } else if let Err(error) = result {
            error!("Original Create-a-Skater screen stopped: {error}");
            screen.runtime = None;
            screen.broken = true;
            customiser.fallback = true;
            customiser.original = false;
            customiser.redraw = true;
        }
    }
    let active = screen.runtime.is_some();
    screen.canvas.set_active(active, &mut cameras);
    for mut node in &mut overlays {
        let display = if active { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    let mut draws = match screen.runtime.as_ref().map(|r| crate::apt_scene::draw(&r.bindings.movie, &r.vm, &r.bindings.shapes)) {
        Some(Ok(draws)) => draws,
        Some(Err(error)) => {
            error!("Create-a-Skater geometry: {error}");
            Vec::new()
        }
        None => Vec::new(),
    };
    // The Graphic Creator: its board card and legend over the movie's page.
    if let (Some(s), Some(runtime)) = (creator.session.as_ref(), screen.runtime.as_ref()) {
        let garment = crate::graphic_creator::garment(&customiser, &s.slot);
        draws = creator.draws(draws, runtime.bindings.first, &screen.source["language"], garment);
    }
    // Item 31 (not retail, user 10-11): the colour wheel over the page (its pictures made once).
    if colour.0.is_some() && !screen.wheel_art {
        use crate::colour_wheel as wheel;
        screen.canvas.insert_texture(wheel::DISC_TEXTURE, images.add(screen_image(wheel::disc_rgba(), [wheel::DISC_SIZE; 2])));
        screen.canvas.insert_texture(wheel::BAR_TEXTURE, images.add(screen_image(wheel::bar_rgba(), wheel::BAR_SIZE)));
        screen.wheel_art = true;
    }
    if let Some(open) = colour.0.as_ref() {
        // LT / RT still turn the skater (user 10-13), so the movie's Rotate Skater stays.
        let wheel = open.draws(&draws);
        draws.extend(wheel);
    }
    // Item 11: the grid search's on-screen keyboard (team_select's panel) in the movie's font and button glyphs.
    if let (Some(entry), Some(runtime)) = (screen.keyboard.as_ref(), screen.runtime.as_ref()) {
        let text = &runtime.bindings.movie.text_assets;
        if let Some(font) = text.font_named("Futura Std Medium").and_then(|id| text.fonts.get(&id)) {
            draws.extend(crate::boot::keyboard_panel(font, entry, &text.localize("ID_COMMON_SEARCH"), &screen.source["buttons"]));
        }
    }
    screen.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

/// A picture for the screen: no mips, on the GPU alone.
fn screen_image(rgba: Vec<u8>, [w, h]: [u32; 2]) -> Image {
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    };
    Image::new(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, rgba, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}
/// A picture the screen made, under `key`, for the movie's pages (thumbnails) and the creator's draws.
fn add_picture(screen: &mut Screen, images: &mut Assets<Image>, key: &str, rgba: Vec<u8>, size: [u32; 2]) {
    let handle = images.add(screen_image(rgba, size));
    screen.canvas.insert_texture(key, handle);
    if let Some(runtime) = screen.runtime.as_mut() {
        runtime.bindings.thumbnails.insert(key.to_lowercase(), crate::apt_scene::Texture { width: size[0], height: size[1], rgba: key.into(), frame: None });
    }
}

/// The Graphic Creator's frame: the pad shared between the movie's page (or popup) and the editor, the
/// page kept in step, the pictures it shows drawn, and the skater following the design.
#[allow(clippy::too_many_arguments)]
fn creator_frame(
    screen: Option<ResMut<Screen>>,
    mut creator: ResMut<crate::graphic_creator::Creator>,
    mut customiser: ResMut<Customiser>,
    mut parts: ResMut<Parts>,
    mut images: ResMut<Assets<Image>>,
    mut tiles: ResMut<crate::graphic_tile::Tiles>,
    config: Res<Config>,
    keys: Res<ButtonInput<KeyCode>>,
    mut keyboard: MessageReader<bevy::input::keyboard::KeyboardInput>,
    time: Res<Time<Real>>,
    mut pads: Local<[crate::input::platform::CapabilityCache; 4]>,
    active: Option<Res<crate::input::ActivePad>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    (buttons, mut wheel, window): (Res<ButtonInput<MouseButton>>, MessageReader<bevy::input::mouse::MouseWheel>, Single<&Window, With<PrimaryWindow>>),
) {
    use crate::{
        custom_graphic::{self as graphic, Doc, Layer},
        graphic_board as board,
        graphic_creator::{self as gc, THUMB},
        graphic_editor::{Mode as EdMode, Pad, Typed, button as bit},
    };
    use bevy::input::keyboard::Key as Typing;
    // A held Enter or Esc repeats: only its first press ends typing (one held from the Add grid would end a
    // fresh text layer at once).
    let typed_keys: Vec<Typing> = keyboard
        .read()
        .filter(|k| k.state.is_pressed() && !(k.repeat && matches!(k.logical_key, Typing::Enter | Typing::Escape)))
        .map(|k| k.logical_key.clone())
        .collect();
    let Some(mut screen) = screen else { return };
    let screen = &mut *screen;
    if !creator.is_open() {
        return;
    }
    if !customiser.open {
        creator.drop_session(&mut tiles);
        return;
    }
    if screen.runtime.is_none() || !screen.shown || screen.closing.is_some() {
        return;
    }
    let dt = time.delta_secs().min(0.1);

    // First frame: the board's art from a movie of its own, and the Add grid's tiles.
    if creator.session.as_ref().is_some_and(|s| s.art.is_none()) {
        match screen.spare.take().and_then(|mut r| board::BoardArt::capture(&mut r)) {
            Ok(art) => {
                if let Some(s) = creator.session.as_mut() {
                    s.art = Some(art);
                }
            }
            Err(error) => {
                // Leave as Discard does: the look the creator found goes back on, held from peers until worn.
                error!("Graphic Creator board: {error}");
                creator.finish(crate::graphic_editor::Exit::Discard, &mut customiser, &parts, &mut tiles);
                return;
            }
        }
        let mut shapes = Vec::new();
        for i in 0..graphic::SHAPES.len() {
            let key = format!("creator/shape/{i}");
            add_picture(screen, &mut images, &key, parts.draw_design(&gc::shape_doc(i), (THUMB, THUMB)), [THUMB, THUMB]);
            shapes.push(key);
        }
        let text = Layer { w: 3400, h: 1700, ..Layer::new(graphic::Art::Text("Aa".into()), [255; 4]) };
        add_picture(screen, &mut images, "creator/text", parts.draw_design(&Doc { layers: vec![text], background: [0; 4] }, (THUMB, THUMB)), [THUMB, THUMB]);
        if let Some(s) = creator.session.as_mut() {
            s.thumbs.shapes = shapes;
            s.thumbs.text = "creator/text".into();
            // Logos show their retail tiles, game art its design image.
            for (key, _) in &s.editor.catalog.logos {
                if let Some(tile) = screen.art.as_ref().and_then(|a| a.graphic_tile(&graphic::logo_id(*key))) {
                    s.thumbs.logos.insert(*key, tile.to_owned());
                }
            }
            for (key, _) in &s.editor.catalog.art {
                if let Some(t) = parts.library.tattoos.get(&graphic::logo_id(*key)) {
                    s.thumbs.art.insert(*key, config.asset_root.join(&t.texture).to_string_lossy().into_owned());
                }
            }
        }
    }

    // The movie shows the creator's page before any press reaches it (the first frame: Edit Skater's).
    if let (Some(s), Some(runtime)) = (creator.session.as_mut(), screen.runtime.as_mut()) {
        if let Err(error) = gc::sync(&mut s.editor, runtime, &s.thumbs) {
            error!("Graphic Creator page: {error}");
        }
    }

    // The pad, and the keyboard standing in for it.
    let packet = crate::input::platform::active_packet(active.and_then(|a| a.0), &mut pads);
    let stick = |v: [i16; 2]| Vec2::new(f32::from(v[0]) / 32767., f32::from(v[1]) / 32767.).clamp_length_max(1.);
    let (held, ls, rs, lt, rt) =
        packet.map_or((0, Vec2::ZERO, Vec2::ZERO, false, false), |p| (p.state.buttons, stick(p.state.left), stick(p.state.right), p.state.triggers[0] > 90, p.state.triggers[1] > 90));
    let typing = creator.session.as_ref().is_some_and(|s| s.editor.mode == EdMode::Text);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let on_board = creator.session.as_ref().is_some_and(|s| s.editor.mode == EdMode::Board);
    let k = gc::keyboard_pad(&keys, typing, on_board);
    let on = |b: u16| held & b != 0;
    let pad = Pad {
        rs,
        ls,
        l3: on(bit::L3),
        lb: on(bit::LB) || k.lb,
        rb: on(bit::RB) || k.rb,
        lt: lt || k.lt,
        rt,
        up: on(bit::UP) || k.up,
        down: on(bit::DOWN) || k.down,
        left: on(bit::LEFT) || k.left,
        right: on(bit::RIGHT) || k.right,
        a: on(bit::A) || k.a,
        b: on(bit::B) || k.b,
        x: on(bit::X) || k.x,
        y: on(bit::Y) || k.y,
        back: on(bit::BACK) || k.back,
        start: on(bit::START) || k.start,
        dt,
    };
    let typed: Vec<Typed> = typed_keys
        .iter()
        .filter(|_| typing)
        .flat_map(|k| match k {
            Typing::Character(c) => c.chars().map(Typed::Char).collect(),
            Typing::Space => vec![Typed::Char(' ')],
            Typing::Backspace => vec![Typed::Backspace],
            Typing::Delete => vec![Typed::Delete],
            Typing::Enter => vec![Typed::Enter],
            Typing::Escape => vec![Typed::Escape],
            _ => Vec::new(),
        })
        .collect();

    let popup_art = screen.popup.is_some();
    let (pad, page_keys) = creator.route(pad, popup_art);
    let mut played = Vec::new();
    let mut exit = None;
    let asking = matches!(creator.session.as_ref().map(|s| s.editor.mode), Some(EdMode::Popup(_)));
    if asking && popup_art {
        // The popup's presses; the editor still gets the rest.
        if let Some(s) = creator.session.as_mut() {
            exit = s.editor.input(&pad, &typed);
            played.append(&mut s.editor.sounds);
        }
        for key in page_keys {
            let Some(runtime) = screen.popup.as_mut().and_then(|p| p.runtime.as_mut()) else { break };
            let answer = runtime.input(key);
            played.append(&mut runtime.bindings.sounds);
            let chosen = match answer {
                Ok(None) => continue,
                Ok(Some(choice)) => (choice != crate::popup_runtime::CANCEL).then_some(choice),
                Err(error) => {
                    error!("Graphic Creator popup: {error}");
                    None
                }
            };
            if let Some(popup) = screen.popup.as_mut() {
                popup.runtime = None;
            }
            exit = creator.answer(chosen);
            break;
        }
    } else if let (Some(s), Some(runtime)) = (creator.session.as_mut(), screen.runtime.as_mut()) {
        match gc::feed(&mut s.editor, runtime, &pad, &page_keys, &typed, &mut played) {
            Ok(e) => exit = e,
            Err(error) => error!("Graphic Creator page: {error}"),
        }
    }
    // The keyboard's shortcuts and the mouse (not while a popup asks).
    let lines: f32 = wheel.read().map(|w| if w.unit == bevy::input::mouse::MouseScrollUnit::Line { w.y } else { w.y / 100. }).sum();
    if let (false, Some(s)) = (asking, creator.session.as_mut()) {
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        for shortcut in gc::shortcuts(ctrl, shift, |k| !typing && keys.just_pressed(k)) {
            s.editor.shortcut(shortcut);
        }
        if let (Some(cursor), Some(runtime)) = (window.cursor_position(), screen.runtime.as_ref()) {
            let mouse = gc::Mouse {
                at: gc::to_movie(cursor, window.size()),
                press: buttons.any_just_pressed([MouseButton::Left, MouseButton::Right]),
                right: buttons.just_pressed(MouseButton::Right),
                held: buttons.pressed(MouseButton::Left),
                release: buttons.just_released(MouseButton::Left),
                wheel: lines,
                shift,
                alt: keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]),
                dt,
            };
            s.mouse(mouse, runtime.bindings.first);
        }
    }
    sounds.write_batch(played.into_iter().map(FrontEndSoundRequest));
    if let Some(exit) = exit {
        creator.finish(exit, &mut customiser, &parts, &mut tiles);
        return;
    }
    // The editor's question goes to the retail popup (without its art the editor takes B).
    if let (Some(ask), Some(popup)) = (creator.question(popup_art), screen.popup.as_mut()) {
        let (description, rows) = ask.question();
        if let Err(error) = popup.ask(Ask::Creator, description, rows, true) {
            error!("Graphic Creator popup: {error}");
            creator.answer(None);
        }
    }
    let Some(s) = creator.session.as_mut() else { return };
    s.follow(&mut customiser, &parts, &mut tiles, dt);
    // The replace grid's tiles: once when it opens (the list does not change while it shows).
    if s.editor.mode != EdMode::Replace {
        s.thumbs.saves.clear();
    } else if s.thumbs.saves.is_empty() {
        s.thumbs.saves =
            s.editor.store.saves(&s.editor.item).iter().map(|saved| crate::graphic_store::tile_file(&s.profile, &s.editor.item, saved).to_string_lossy().into_owned()).collect();
    }

    // The pictures: the side on the board, then any layer's not drawn yet.
    // While a stick or the mouse changes it every frame, at most every `BOARD_EVERY`; a new side at once.
    let side = (s.editor.doc_rev, s.editor.side);
    s.board_age += dt;
    if s.drawn != Some(side) && (s.drawn.map(|d| d.1) != Some(side.1) || s.board_age >= gc::BOARD_EVERY) {
        s.drawn = Some(side);
        s.board_age = 0.;
        let size = board::board_pixels(s.editor.extent());
        let rgba = parts.draw_design(s.editor.doc(), (size[0], size[1]));
        add_picture(screen, &mut images, board::BOARD_TEXTURE, rgba, size);
    }
    // The garment's outline of the side, from the item's model (its secondary UVs), once per side.
    let zone = s.editor.zone();
    if !s.outlines.contains_key(zone) {
        let size = board::board_pixels(s.editor.extent());
        // ponytail: the model is read once per item and rasterised once per side, on this frame; a few ms.
        let item = &s.editor.item;
        let triangles = s.garment_uv.get_or_insert_with(|| {
            parts.library.models.get(item).and_then(|m| std::fs::read(config.asset_root.join(&m.scene)).ok()).and_then(|bytes| crate::graphic_outline::glb_triangles(&bytes))
        });
        let mask = triangles
            .as_ref()
            .map(|t| crate::graphic_outline::outline(t, s.editor.rect(), (size[0], size[1])))
            .unwrap_or(crate::graphic_outline::OutlineMask { size: (1, 1), edges: vec![0] });
        s.outlines.insert(zone, mask);
    }
    if s.outline_shown != Some(zone) {
        s.outline_shown = Some(zone);
        let mask = &s.outlines[zone];
        add_picture(screen, &mut images, board::OUTLINE_TEXTURE, mask.rgba(64), [mask.size.0, mask.size.1]);
    }
    for (look, layer) in s.missing_thumbs() {
        let key = s.thumb_key(look);
        add_picture(screen, &mut images, &key, parts.draw_design(&gc::thumb_doc(&layer), (THUMB, THUMB)), [THUMB, THUMB]);
    }
    if let Some(runtime) = screen.runtime.as_mut() {
        if let Err(error) = gc::sync(&mut s.editor, runtime, &s.thumbs) {
            error!("Graphic Creator page: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each item's game: Skate 1 / 2 ports by their prefix, pro_clothing.py's by who wore it, never by a name alone.
    #[test]
    fn items_are_tagged_with_the_game_they_come_from() {
        assert_eq!(origin("s2es_s2breeze_s2male"), "Skate 2");
        assert_eq!(origin("s1cap_s1new_s1era"), "Skate 1");
        assert_eq!(origin("cargo_shorts1_male"), "Skate 3");
        assert_eq!(origin("s2mq_s2mqandrew_s2mqreynolds_s2mqfeet_s2mqfeet"), "Skate 3", "a Skate 3 pro's");
        assert_eq!(origin("s2mq_s2mqmike_s2mqcarroll_s2mqfeet_s2mqfeet"), "Skate 3", "Mike Carroll is Skate 3's");
        assert_eq!(origin("s2mq_s2mqs2_s2mqalex_s2mqchalmers_s2mqfeet_s2mqfeet"), "Skate 2", "a Skate 2 pro's");
        assert_eq!(origin("s2mq_s2mqmike_s2mqaccessory_s2mqaccessory"), "Skate 2", "Skate 2's Mike's backpack");
    }
    use serde_json::json;

    /// Perf 10-12: each runtime the spare hands out (the setup's, then worker loads) plays exactly like a fresh
    /// load on the main thread; and how long an open now waits for it.
    #[test]
    fn spare_runtimes_play_like_a_fresh_load() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT") else { return };
        let file = PathBuf::from(assets).join("private/create-skater/runtime/createskater.json");
        let Ok(bytes) = std::fs::read(file) else { return };
        let source = Arc::new(serde_json::from_slice::<Value>(&bytes).unwrap());
        let play = |mut runtime: Runtime| {
            let mut frames = Vec::new();
            for _ in 0..40 {
                runtime.advance().unwrap();
                let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
                frames.push(
                    draws
                        .iter()
                        .map(|d| {
                            let bits = d.vertices.iter().flat_map(|v| v.position.into_iter().chain(v.uv)).chain(d.multiply).chain(d.add);
                            (d.texture.clone(), d.size, bits.map(f32::to_bits).collect::<Vec<_>>())
                        })
                        .collect::<Vec<_>>(),
                );
            }
            frames
        };
        let fresh = play(Runtime::load(&source, empty_page()).unwrap());
        assert!(fresh.iter().any(|f| !f.is_empty()));
        let mut spare = Spare::new(source.clone(), Runtime::load(&source, empty_page()).unwrap());
        for i in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(300)); // a player's open, not back to back
            let t = std::time::Instant::now();
            let runtime = spare.take().unwrap();
            eprintln!("open {i}: waited {:.2} ms for the spare", t.elapsed().as_secs_f64() * 1e3);
            assert!(play(runtime) == fresh, "take {i}");
        }
    }

    /// Item 31 round 3 (user 10-11): A, Enter or a click on the colour row (its swatch) opens the colour wheel.
    #[test]
    fn a_press_or_a_click_on_the_colour_row_opens_the_wheel() {
        let custom = cas_menu::Custom {
            template: json!({"grip": {"rgb": [0, 0, 0], "solid": false}}),
            at: "/grip/rgb".into(),
            start: [0.5; 3],
            authored: [0.5; 3],
            default: json!({"grip": null}),
            hair: false,
        };
        let acts = vec![Act::Wheel(custom.clone()), Act::Cycle(Vec::new())];
        assert_eq!(wheel_for(&acts, 0), Some(custom));
        assert_eq!(wheel_for(&acts, 1), None, "A on another row leaves the page");
        let row = |kind: &'static str| crate::create_skater_runtime::Row { kind, ..Default::default() };
        let rows = vec![row("color"), row("selector")];
        assert_eq!(row_under(&rows, Vec2::new(122., 165.)), Some(0), "the swatch");
        assert_eq!(row_under(&rows, Vec2::new(300., 130.)), Some(0), "the row's name line");
        assert_eq!(row_under(&rows, Vec2::new(150., 233.)), Some(1), "the selector under it");
        assert_eq!(row_under(&rows, Vec2::new(600., 165.)), None, "off the card");
        assert_eq!(row_under(&rows, Vec2::new(150., 400.)), None, "below the rows");
    }

    #[test]
    fn done_in_the_graphic_creator_survives_b_on_the_grid_it_was_opened_from_but_not_the_roots_discard() {
        let level = |node, look: &str| Level { node, acts: Vec::new(), entered: json!(look), row: 0, search: String::new(), game: 0 };
        let mut stack = vec![
            level(Page_::Root, "root"),
            level(Page_::GraphicKind("front"), "kind"),
            level(Page_::GraphicGrid("front", cas_menu::CUSTOM), "grid"),
            level(Page_::SavedGraphic("front", 0), "saved"),
        ];
        commit_look(&mut stack, &json!("new design"));
        let entered: Vec<_> = stack.iter().map(|l| l.entered.as_str().unwrap().to_owned()).collect();
        // B on the grid puts back its entered look: now the new design. The page on top too. The
        // root's stays: Continue without saving still discards everything. Pages in between are
        // not grids and restore nothing on B.
        assert_eq!(entered, ["root", "kind", "new design", "new design"]);
        // Opened straight from the kind page (the Graphic Creator row): that page is the top.
        stack.truncate(2);
        commit_look(&mut stack, &json!("again"));
        assert_eq!(stack[1].entered, json!("again"));
        assert_eq!(stack[0].entered, json!("root"));
        // A lone root changes nothing.
        let mut root = vec![level(Page_::Root, "root")];
        commit_look(&mut root, &json!("x"));
        assert_eq!(root[0].entered, json!("root"));
    }

    /// The tattoo grid's tiles are the retail logo_object tiles (10-11: they were the decals, squashed).
    #[test]
    fn tattoo_tiles_are_the_retail_logo_tiles() {
        let catalog: Catalog = serde_json::from_value(json!({
            "components": [{"models": [{"id": "logo", "name": "logo_object", "lods": [{"material_instances": [[
                {"id": "t1", "name": "tattoo_viper_mat"}, {"id": "t2", "name": "tattoo_plain_mat"}]]}]}]}],
            "materials": {"t1": {"flags": {"cas.TattooCategory": "2"}}, "t2": {"flags": {"cas.TattooCategory": "2"}}}
        }))
        .unwrap();
        let library: Library = serde_json::from_value(json!({
            "models": {}, "materials": {}, "defaults": {}, "morphs": [],
            "tattoos": {"t1": {"name": "Viper", "texture": "ded/viper.png", "bounds": [0, 0, 1, 1]},
                        "t2": {"name": "Plain", "texture": "ded/plain.png", "bounds": [0, 0, 1, 1]}}
        }))
        .unwrap();
        let source = json!({"thumbnails": {"tattoo/logo_object_tattoo_viper_mat": {}, "logos/logo_object_tattoo_viper_mat": {}}});
        let assets = std::path::Path::new("assets");
        let art = Art::build(catalog, &library, &source, assets);
        assert_eq!(
            art.tattoos(2),
            [
                ("t1".to_owned(), "#Viper".to_owned(), "logos/logo_object_tattoo_viper_mat".to_owned()),
                // No retail tile: the design itself.
                ("t2".to_owned(), "#Plain".to_owned(), assets.join("ded/plain.png").to_string_lossy().into_owned()),
            ]
        );
    }
}
