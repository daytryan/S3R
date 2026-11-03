//! Skate 3's Edit Skater page layout, recorded from the retail game (PS3 build,
//! captured page by page) and expressed over this engine's customisation data.
//! Titles, labels and help texts are the owned string table's IDs.
use crate::{
    create_skater::Art,
    create_skater_runtime::{Mode, Page, Row},
    customiser::{Customiser, patch_selected, scalar_of},
    customiser_parts::Parts,
    graphic_store::{self, Saved, Store},
    graphic_tile::Job,
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    Root,
    Body,
    Skin,
    Hair,
    Shape,
    Presets,
    Face,
    Eyes,
    Brow,
    Nose,
    Jaw,
    Mouth,
    Chin,
    FacialHair,
    /// Upper (true) or lower body placement list.
    Tattoo(bool),
    /// Category list for a placement (side index as the customiser stores it).
    TattooKind(bool, u8),
    TattooGrid(bool, u8, u8),
    Merch,
    /// A clothing grid by its customiser label ("Hats", "T-shirts", ...).
    Clothes(&'static str),
    Accessories,
    Boards,
    Board(&'static str),
    /// Grip Color (ours, untraced: retail has no grip row): the deck grip's colour and style.
    Grip,
    /// Edit Clothing / Edit Deck for one grid item: (grid label, is board, entry, design).
    Garment(&'static str, bool, usize, usize),
    Style,
    /// Graphic Adjust (retail Shirt/Hat/BoardGraphicAdjust) for a Graphic Location's
    /// (custom_graphic::ZONES key) logo or design: the movie's stick legend, no rows.
    Graphic(&'static str),
    /// Graphics (retail GraphicCategories) for a location: Brand Logos, Team Logos, ours
    /// (Custom Graphics, the one way into the Graphic Creator), Remove Logo.
    GraphicKind(&'static str),
    /// A location's graphics of one kind (GRAPHIC_KINDS index).
    GraphicGrid(&'static str, u8),
    /// One saved Custom Graphic of the item at a location (tile `n` of its Custom Graphics grid):
    /// Select, Edit, Delete.
    SavedGraphic(&'static str, u8),
    /// Graphic Location (retail ShirtGraphicLocation): where the worn top's one graphic goes
    /// (front, back, sleeves).
    GraphicLocation,
    /// Graphic Adjust (retail TattooUpper/LowerAdjust) for the upper (true) or lower body tattoo.
    TattooAdjust(bool),
}
/// GraphicGrid kinds: list label and the library kind ("" = Custom Graphics, the designs saved from the Graphic Creator).
const GRAPHIC_KINDS: [(&str, &str); 3] =
    [("ID_CAC_BRANDGRAPHICS_TITLE", "brand"), ("ID_CAC_TEAMGRAPHICS_TITLE", "team"), ("#Custom Graphics", "")];
/// The GraphicGrid kind that is the item's Custom Graphics.
pub(crate) const CUSTOM: u8 = 2;
/// What a row does.
#[derive(Clone, Debug)]
pub(crate) enum Act {
    Open(Node),
    Gender,
    Undo,
    /// Apply this authored choice (grid tile, list choice).
    Choose(Option<Value>),
    /// A applies the patch and opens the page: a grid item (previewed on highlight) and its
    /// Edit Clothing page, or a Graphic Location (the top's graphic moved there) and its Graphics.
    Item(Value, Node),
    /// Swatch or selector choices: Left/Right steps through them.
    Cycle(Vec<Value>),
    /// Not retail (user 10-11, item 31): a colour row that is the colour wheel's, one swatch in the colour now:
    /// Left / Right on it, or X on its page, opens the wheel (create_skater, colour_wheel).
    Wheel(Custom),
    Slider { key: String, lo: f64, hi: f64, step: f64, initial: f64 },
    /// Open the Graphic Creator on a blank canvas for the item at a Graphic Location (New).
    CreatorNew(&'static str),
    /// Open save `n` of the item at a Graphic Location in the Graphic Creator (Edit).
    CreatorSaved(&'static str, usize),
    /// Ask, then delete save `n` of the item at a Graphic Location.
    DeleteSaved(&'static str, usize),
    /// Put save `n`'s share code (`share_code`) on the clipboard.
    CopyCode(&'static str, usize),
}
/// Not retail (user 10-11, item 31): a colour row's colour wheel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Custom {
    /// The row's patch; the wheel's colour goes at `at` (a JSON pointer) in it.
    pub template: Value,
    pub at: String,
    /// Where the wheel starts (the colour worn) and where Y (Default) takes it (the item's own), as shown.
    pub start: [f32; 3],
    pub authored: [f32; 3],
    /// Y: the item's own colour back (a null tint: the material's own; no grip colour).
    pub default: Value,
    /// Hair: its tint is far darker than the colour it shows (hair_tint, hair_shown).
    pub hair: bool,
}
impl Custom {
    /// The row in shown colour `rgb` (kept to 0.001, so a saved look reads back equal).
    pub(crate) fn patch(&self, rgb: [f32; 3]) -> Value {
        let rgb = if self.hair { hair_tint(rgb) } else { rgb };
        let mut patch = self.template.clone();
        if let Some(tint) = patch.pointer_mut(&self.at) {
            *tint = json!(rgb.map(|v| (f64::from(v.clamp(0., 1.)) * 1000.).round() / 1000.));
        }
        patch
    }
}
/// Hair's tint for a shown colour, tint = HAIR_GAIN x shown^HAIR_GAMMA: a fit to the retail Hair colour swatches
/// (Blond shows [1, 0.9, 0.65] for tint [0.3, 0.2, 0.1], Grey 0.62 for 0.1, Black 0 for 0.017).
/// ponytail: one curve for all ten (Red and Pink sit off it); tune the two knobs by eye.
const HAIR_GAIN: f32 = 0.3;
const HAIR_GAMMA: f32 = 2.;
pub(crate) fn hair_tint(shown: [f32; 3]) -> [f32; 3] {
    shown.map(|v| HAIR_GAIN * v.clamp(0., 1.).powf(HAIR_GAMMA))
}
pub(crate) fn hair_shown(tint: [f32; 3]) -> [f32; 3] {
    tint.map(|v| (v.max(0.) / HAIR_GAIN).powf(1. / HAIR_GAMMA).min(1.))
}
fn rgb_of(v: &Value) -> Option<[f32; 3]> {
    serde_json::from_value(v.clone()).ok()
}
/// Camera: full body, face close-up, board close-up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Frame {
    Body,
    Face,
    Head,
    Torso,
    Legs,
    Feet,
    Hands,
    Board,
    /// The carried board seen edge-on (wheels).
    BoardSide,
    /// The torso from behind, and from either side (graphics on the back and sleeves).
    Back,
    LeftSide,
    RightSide,
}
impl Frame {
    /// Preview (subject height, camera distance, skater turn). Body, Face, Torso,
    /// Feet and Board are measured from retail footage (level camera, 50 degree
    /// field of view, skater framed against the room's ledge); the rest are
    /// scaled from those. ponytail: estimates off video frames; tune here.
    pub fn view(self) -> (f32, f32, f32) {
        match self {
            Frame::Body => (0.85, 2.25, 0.),
            Frame::Face => (1.54, 0.65, 0.),
            Frame::Head => (1.62, 0.9, 0.),
            Frame::Torso => (1.28, 1.25, 0.),
            Frame::Legs => (0.6, 1.5, 0.),
            Frame::Feet => (0.15, 0.85, 0.),
            Frame::Hands => (0.95, 1.2, 0.3),
            Frame::Board => (0.47, 1.3, 0.),
            Frame::BoardSide => (0.5, 1.1, -1.2),
            Frame::Back => (1.28, 1.25, std::f32::consts::PI),
            Frame::LeftSide => (1.28, 1.25, -1.3),
            Frame::RightSide => (1.28, 1.25, 1.3),
        }
    }
    /// The view of a Graphic Location.
    pub(crate) fn of_zone(zone: &str) -> Self {
        match zone {
            "back" => Frame::Back,
            "left" => Frame::LeftSide,
            "right" => Frame::RightSide,
            "deck" => Frame::Board,
            "hat" => Frame::Head,
            _ => Frame::Torso,
        }
    }
}
pub(crate) struct Built {
    pub page: Page,
    pub acts: Vec<Act>,
    pub frame: Frame,
    /// Clothing removed while choosing a tattoo.
    pub strip: Option<&'static str>,
}
impl Node {
    /// Edit pages: A accepts and returns, B is not offered.
    pub fn is_editor(self) -> bool {
        matches!(
            self,
            Node::Skin | Node::Hair | Node::Shape | Node::Presets | Node::Eyes | Node::Brow | Node::Nose | Node::Jaw
                | Node::Mouth | Node::Chin | Node::FacialHair | Node::Style | Node::Garment(..) | Node::Grip
        )
    }
    pub fn is_grid(self) -> bool {
        matches!(self, Node::TattooGrid(..) | Node::Clothes(_) | Node::Board(_) | Node::GraphicGrid(..))
    }
    /// A grid of items from the three games, which RB / LB narrow to one (GAMES).
    pub fn by_game(self) -> bool {
        matches!(self, Node::Clothes(_) | Node::Board(_))
    }
    /// Graphic Adjust: A accepts and leaves the graphic flow, B is Change Graphic.
    pub fn is_adjust(self) -> bool {
        matches!(self, Node::Graphic(_) | Node::TattooAdjust(_))
    }
    /// A graphic or tattoo flow's pages, left together by Graphic Adjust's A and Remove Logo.
    pub fn in_graphic_flow(self) -> bool {
        matches!(
            self,
            Node::Graphic(_) | Node::GraphicKind(_) | Node::GraphicGrid(..) | Node::SavedGraphic(..) | Node::GraphicLocation
                | Node::TattooKind(..) | Node::TattooGrid(..) | Node::TattooAdjust(_)
        )
    }
}

const BODY_BLURB: &str = "ID_CAC_BACKALLEY_PETES_DESC";
/// Gesture rows by the customiser's direction index (Up, Down, Left, Right),
/// in the retail order Up, Right, Left, Down.
pub(crate) const GESTURE_ROWS: [usize; 4] = [0, 3, 2, 1];
fn gesture_name(label: &str) -> String {
    crate::cas_pose::GESTURES.iter().find(|g| g.0 == label).map_or(format!("#{label}"), |g| g.1.to_owned())
}
/// The engine label of the gesture on a direction (for the preview clip).
pub(crate) fn gesture_on(c: &Customiser, direction: usize) -> Option<&'static str> {
    let index = c.draft["gestures"][direction.to_string()].as_u64().map_or(direction, |i| i as usize);
    crate::cas_pose::GESTURES.get(index).map(|g| g.0)
}
/// cac_colour swatch colours are display values: 0-1 straight to 0-255.
pub(crate) const SWATCH: fn([f32; 3]) -> u32 = |c| {
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    b(c[0]) << 16 | b(c[1]) << 8 | b(c[2])
};

fn row(text: &str, kind: &'static str, description: &str) -> Row {
    Row { text: text.into(), kind, description: description.into(), ..Default::default() }
}
/// A plain entry on an options page (Edit Skater's Body Mods, Undo Changes).
fn open(text: &str, node: Node, description: &str) -> (Row, Act) {
    (row(text, "option", description), Act::Open(node))
}
fn undo() -> (Row, Act) {
    (row("ID_CAC_MORPHING_UNDO", "option", BODY_BLURB), Act::Undo)
}
/// A morph slider over the customiser's authored range.
fn morph(c: &Customiser, text: &str, key: &'static str) -> (Row, Act) {
    let (lo, hi, initial) = (0., 0.5, if key == "morphs.fat" || key == "morphs.thin" { 0. } else { 0.25 });
    let v = scalar_of(&c.draft, key, initial);
    let mut r = row(text, "morph", BODY_BLURB);
    // highlight_morph places its knob at value / mMaxValue (100).
    r.value = ((v - lo) / (hi - lo) * 100.).round().clamp(0., 100.);
    (r, Act::Slider { key: key.into(), lo, hi, step: 0.05, initial })
}
/// A patch for one side that takes its stock logo and the logo's adjust off; the side's design stays.
fn no_logo() -> Value {
    let mut side = serde_json::Map::from_iter([("logo".to_string(), Value::Null)]);
    side.extend(crate::custom_graphic::ADJUST.iter().map(|a| (a.0.to_string(), Value::Null)));
    Value::Object(side)
}
/// The item that takes the Graphic Location `zone`: its model id and all its locations.
pub(crate) fn graphic_item(c: &Customiser, parts: &Parts, zone: &str) -> Option<(String, Vec<(&'static str, [f32; 4])>)> {
    let worn = c.draft["selections"].as_object()?.values().find(|w| graphic_locations(parts, w).iter().any(|l| l.0 == zone))?;
    Some((worn["asset_id"].as_str()?.to_owned(), graphic_locations(parts, worn)))
}
/// Save `n`'s share code, of the item at `zone`.
pub(crate) fn saved_code(c: &Customiser, parts: &Parts, zone: &str, n: usize) -> Option<String> {
    let (item, _) = graphic_item(c, parts, zone)?;
    let store = Store::load(c.profile_path());
    store.saves(&item).get(n).map(|saved| crate::share_code::encode(&item, saved))
}
/// What wearing a save patches in: every side of the item gets its design (none where the save has none),
/// and no stock logo, so the item looks exactly as it was saved.
pub(crate) fn wear_patch(sides: &[(&'static str, [f32; 4])], saved: &Saved) -> Value {
    let patch: serde_json::Map<String, Value> = sides
        .iter()
        .map(|(zone, _)| {
            let mut side = no_logo();
            side["design"] = saved.sides.get(*zone).map_or(Value::Null, |text| json!(text));
            ((*zone).to_owned(), side)
        })
        .collect();
    json!({ "graphics": patch })
}
/// The draft wears exactly this save.
fn wears(c: &Customiser, sides: &[(&'static str, [f32; 4])], saved: &Saved) -> bool {
    wears_in(&c.draft, sides, saved)
}
fn wears_in(draft: &Value, sides: &[(&'static str, [f32; 4])], saved: &Saved) -> bool {
    sides.iter().all(|(zone, _)| draft["graphics"][zone]["design"].as_str() == saved.sides.get(*zone).map(String::as_str))
}
/// The preview tiles the item's saves need drawn (each skipped once its PNG exists).
pub(crate) fn custom_jobs(c: &Customiser, parts: &Parts, zone: &str) -> Vec<Job> {
    let Some((item, _)) = graphic_item(c, parts, zone) else { return Vec::new() };
    let profile = c.profile_path();
    Store::load(profile)
        .saves(&item)
        .into_iter()
        .map(|saved| Job { path: graphic_store::tile_file(profile, &item, saved), item: item.clone(), saved: saved.clone() })
        .collect()
}
/// Delete save `n` of the item at `zone`, with its preview tile. `entered` is the look the Custom
/// Graphics grid was entered with (what B on it goes back to): the skater returns to it, and when it
/// wore the deleted save the item goes blank (in the draft and in `entered`) as it has nothing to wear.
pub(crate) fn delete_saved(c: &mut Customiser, parts: &Parts, zone: &str, n: usize, entered: &mut Value) -> bool {
    let Some((item, sides)) = graphic_item(c, parts, zone) else { return false };
    let profile = c.profile_path().to_path_buf();
    let mut store = Store::load(&profile);
    let Some(saved) = store.remove(&item, n) else { return false };
    if store.save(&profile).is_err() {
        return false;
    }
    let _ = std::fs::remove_file(graphic_store::tile_file(&profile, &item, &saved));
    c.draft = entered.clone();
    c.redraw = true;
    if wears_in(entered, &sides, &saved) {
        c.apply(parts, Some(&wear_patch(&sides, &Saved::default())));
        *entered = c.draft.clone();
    }
    true
}
/// Tiles drawn for designs that were since changed or deleted (or by an older tile version) are removed.
fn forget_old_tiles(profile: &std::path::Path, item: &str, store: &Store) {
    let Some(dir) = graphic_store::tile_file(profile, item, &Saved::default()).parent().map(std::path::Path::to_path_buf) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let keep: Vec<String> = store.saves(item).iter().map(|s| format!("{}.png", s.hash())).collect();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !keep.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
/// The customizable tees My Graphics were made for, as the library names them (both are called "Tee";
/// the other tees of a gender are not logoable): the male one and the female one.
pub(crate) const OLD_TEES: [(&str, bool); 2] = [("801095192c838106", false), ("862d97f973760883", true)];
/// My Graphics (the old eight single designs) moves into the lists of both customizable tees the first
/// time the Custom Graphics tab is built, as front designs drawn for each body: the old file never said
/// which one made them, and each gender then finds them on its own tee.
fn migrate_old_graphics(c: &Customiser, parts: &Parts) {
    if !crate::custom_graphic::slots_file(c.profile_path()).exists() {
        return;
    }
    let targets: Vec<(&str, [f32; 4], bool)> = OLD_TEES
        .iter()
        .filter_map(|&(id, female)| {
            let model = parts.library.models.get(id)?;
            let front = crate::custom_graphic::locations(id, model, true).into_iter().find(|l| l.0 == "front")?;
            Some((id, front.1, female))
        })
        .collect();
    if !targets.is_empty() {
        graphic_store::migrate_my_graphics(c.profile_path(), &targets);
    }
}
fn graphic_zones(parts: &Parts, worn: &Value) -> Vec<&'static str> {
    graphic_locations(parts, worn).into_iter().map(|(zone, _)| zone).collect()
}
/// The worn item's Graphic Locations and rectangles (custom_graphic::locations; none for other items).
pub(crate) fn graphic_locations(parts: &Parts, worn: &Value) -> Vec<(&'static str, [f32; 4])> {
    let lib = &parts.library;
    let Some((id, model)) = worn["asset_id"].as_str().and_then(|m| lib.models.get_key_value(m)) else { return Vec::new() };
    let logoable = worn["material_id"]
        .as_str()
        .and_then(|m| lib.materials.get(m))
        .is_some_and(|m| m.flag("IsLogoable").eq_ignore_ascii_case("true"));
    crate::custom_graphic::locations(id, model, logoable)
}
fn current(c: &Customiser, choices: &[Value]) -> Option<usize> {
    choices.iter().position(|p| patch_selected(&c.draft, p))
}
/// A "< value >" selector over labelled choices.
fn selector(c: &Customiser, text: &str, choices: Vec<(String, Value)>, description: &str) -> (Row, Act) {
    let patches: Vec<Value> = choices.iter().map(|(_, p)| p.clone()).collect();
    let mut r = row(text, "selector", description);
    r.value_text = current(c, &patches).map_or(String::new(), |i| choices[i].0.clone());
    (r, Act::Cycle(patches))
}
/// A colour swatch row.
fn swatches(c: &Customiser, text: &str, choices: Vec<([f32; 3], Value)>) -> (Row, Act) {
    let patches: Vec<Value> = choices.iter().map(|(_, p)| p.clone()).collect();
    let mut r = row(text, "color", BODY_BLURB);
    r.colours = choices.iter().map(|(rgb, _)| SWATCH(*rgb)).collect();
    r.value = current(c, &patches).unwrap_or(0) as f64;
    (r, Act::Cycle(patches))
}
/// Not retail (user 10-11, item 31): the colour row's blurb, the info panel's while it is highlighted.
pub(crate) const WHEEL_BLURB: &str = "#Press A to change the color on the color wheel.";
/// Not retail (user 10-11, item 31): a colour row that is the colour wheel's: one swatch, the colour worn.
fn wheel_row(text: &str, custom: Custom) -> (Row, Act) {
    let mut r = row(text, "color", WHEEL_BLURB);
    r.colours = vec![SWATCH(custom.start)];
    (r, Act::Wheel(custom))
}
fn patches(c: &Customiser, labels: &[&str]) -> Vec<(String, Value)> {
    c.entry(labels)
        .map(|e| c.for_gender(e).into_iter().filter_map(|e| Some((e.label.clone(), e.patch.clone()?))).collect())
        .unwrap_or_default()
}
fn tint(patch: &Value, key: &str) -> [f32; 3] {
    let v = &patch[key];
    [0, 1, 2].map(|i| v[i].as_f64().unwrap_or(0.) as f32)
}
/// Head/eye material variants selecting on one flag (EyeColour, EyebrowStyle).
fn variants(c: &Customiser, parts: &Parts, slot: &str, flag: &str) -> Vec<(String, String)> {
    let lib = &parts.library;
    let Some(asset) = c.draft["selections"][slot]["asset_id"].as_str() else { return Vec::new() };
    let Some(model) = lib.models.get(asset) else { return Vec::new() };
    let mut out: Vec<(String, String)> = Vec::new();
    for id in &model.materials {
        let Some(m) = lib.materials.get(id) else { continue };
        let value = m.flag(flag);
        if !value.is_empty() && !out.iter().any(|(v, _)| v == value) {
            out.push((value.to_owned(), id.clone()));
        }
    }
    out.sort();
    out
}
/// A flag of the worn head material (its brow and facial hair are variants).
fn head_flag(c: &Customiser, parts: &Parts, flag: &str) -> String {
    c.draft["selections"]["Rostral"]["material_id"]
        .as_str()
        .and_then(|id| parts.library.materials.get(id))
        .map_or(String::new(), |m| m.flag(flag).to_owned())
}
/// Customiser entries that select a part, as grid tiles.
/// Accessories > the Accessory slot's tabs (row text, customiser label). Retail lists Rings only
/// (row 44, ctor 0x825f4b98); Belts uses retail's unused ID_CAC_BELTS_TITLE; Chains, Backpacks, Other and Gloves
/// have no retail string. The ported tabs show only when the skater's gender has items in them.
const ACCESSORY_TABS: [(&str, &str); 6] = [
    ("ID_CAC_RINGS_TITLE", "Rings"), ("ID_CAC_BELTS_TITLE", "Belts"), ("#Chains", "Chains"), ("#Backpacks", "Backpacks"),
    ("#Other", "Other"), ("#Gloves", "Gloves"),
];
/// Customiser path of a grid's entries.
fn grid_path(label: &'static str, board: bool) -> Vec<&'static str> {
    if board {
        vec!["Clothes", "Board", label]
    } else if ["Glasses", "Necklaces", "Wristwear"].contains(&label) || ACCESSORY_TABS.iter().any(|t| t.1 == label) {
        vec!["Clothes", "Accessories", label]
    } else {
        vec!["Clothes", label]
    }
}
/// The slot a part patch selects.
fn slot_of(patch: &Value) -> Option<&str> {
    patch["selections"].as_object()?.keys().next().map(String::as_str)
}
fn recoloured(patch: &Value) -> bool {
    slot_of(patch).is_some_and(|slot| !patch["colours"][slot].is_null())
}
/// The model a part patch selects.
fn model_of(patch: &Value) -> Option<&str> {
    let slot = slot_of(patch)?;
    patch["selections"][slot]["asset_id"].as_str()
}
/// The same design on another wear-style model. Alternates list their designs
/// in their own order and sometimes shorten the name ("Blackhero Hood" /
/// "Blackhero"): exact name, else most shared words, else the same index.
fn same_design(parts: &Parts, from: &str, design: usize, to: &str) -> Option<usize> {
    let lib = &parts.library;
    let name = |model: &str, i: usize| -> Option<String> {
        Some(lib.materials.get(lib.models.get(model)?.materials.get(i)?)?.name.to_lowercase())
    };
    let wanted = name(from, design)?;
    let count = lib.models.get(to)?.materials.len();
    if let Some(i) = (0..count).find(|&i| name(to, i).as_deref() == Some(wanted.as_str())) {
        return Some(i);
    }
    let words = |s: &str| s.split_whitespace().map(str::to_owned).collect::<std::collections::HashSet<_>>();
    let w = words(&wanted);
    let best = (0..count)
        .filter_map(|i| {
            let other = words(&name(to, i)?);
            let shared = w.intersection(&other).count();
            (shared > 0).then(|| (shared as f32 / w.union(&other).count() as f32, i))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0));
    best.map(|(_, i)| i).or((design < count).then_some(design))
}
/// Which design (of the default model) and wear style the skater wears in a family.
fn worn_in(c: &Customiser, parts: &Parts, slot: &str, styles: &[String]) -> Option<(usize, usize)> {
    let asset = c.draft["selections"][slot]["asset_id"].as_str()?;
    let material = c.draft["selections"][slot]["material_id"].as_str()?;
    let style = styles.iter().position(|s| s == asset)?;
    let design = parts.library.models.get(asset)?.materials.iter().position(|m| m == material)?;
    let design = if style == 0 { design } else { same_design(parts, asset, design, &styles[0])? };
    Some((design, style))
}
/// The patch wearing the default model's `design` in wear style `style`.
fn wear(parts: &Parts, slot: &str, styles: &[String], design: usize, style: usize) -> Option<Value> {
    let model = styles.get(style)?;
    let index = if style == 0 { design } else { same_design(parts, &styles[0], design, model)? };
    let material = parts.library.models.get(model)?.materials.get(index)?;
    Some(json!({"selections":{slot:{"asset_id":model,"material_id":material}},"colours":{slot:Value::Null}}))
}
/// A model whose designs are the retail New / Worn finish of one item (shoes).
fn new_and_worn(parts: &Parts, model: &str) -> bool {
    parts.library.models.get(model).is_some_and(|m| {
        m.materials.len() == 2
            && m.materials.iter().filter_map(|id| parts.library.materials.get(id)).map(|x| x.name.as_str()).eq(["New", "Worn"])
    })
}
fn colourisable(parts: &Parts, patch: &Value) -> bool {
    let Some(slot) = slot_of(patch) else { return false };
    patch["selections"][slot]["material_id"]
        .as_str()
        .and_then(|m| parts.library.materials.get(m))
        .is_some_and(|m| m.flag("IsColourizable").eq_ignore_ascii_case("true"))
}
/// One tile per design, as the retail grids list them. Alternate wear styles
/// (hood up, open, cap backwards...) are not items: they are the design's
/// Edit Clothing options. Items with neither options nor colours are simply
/// chosen.
fn grid(c: &Customiser, parts: &Parts, art: &Art, label: &'static str, board: bool, rows: &mut Vec<Row>, acts: &mut Vec<Act>) -> Option<usize> {
    let mut worn = None;
    let gender = c.gender().to_owned();
    let entry = c.entry(&grid_path(label, board));
    for (i, e) in entry.map(|e| c.for_gender(e)).unwrap_or_default().into_iter().enumerate() {
        if e.options.is_empty() {
            let Some(patch) = &e.patch else { continue };
            if patch_selected(&c.draft, patch) {
                worn = Some(rows.len());
            }
            rows.push(Row { text: "ID_CAC_REMOVE_PART".into(), kind: "none", description: "ID_CAC_REMOVE_PART".into(), ..Default::default() });
            acts.push(Act::Choose(Some(patch.clone())));
            continue;
        }
        if board {
            // Skateboard parts keep the engine's New/Worn grouping per graphic.
            let materials: Vec<&Value> = e.options.iter().filter_map(|o| o.patch.as_ref()).filter(|p| !recoloured(p)).collect();
            let current = materials.iter().copied().find(|p| patch_selected(&c.draft, p));
            let Some(shown) = current.or_else(|| materials.iter().copied().find(|p| art.part(p, &gender).is_some())) else { continue };
            let Some(part) = art.part(shown, &gender) else { continue };
            // The stampable deck's plain colours (White 1, Blue Dirty 1 ...) are left out: the Customizable
            // Board is that deck, in any colour and with graphics (10-11, by request: retail lists them).
            if customizable(parts, shown) && !part.name.starts_with("#Customizable") {
                continue;
            }
            if current.is_some() {
                worn = Some(rows.len());
            }
            rows.push(tile(part));
            acts.push(Act::Item(shown.clone(), Node::Garment(label, board, i, 0)));
            continue;
        }
        let Some(model) = e.options.iter().filter_map(|o| o.patch.as_ref()).find_map(model_of) else { continue };
        if art.is_alternate(model) {
            continue;
        }
        let styles = art.wear_styles(model);
        let Some(slot) = e.options.iter().filter_map(|o| o.patch.as_ref()).find_map(slot_of).map(str::to_owned) else { continue };
        if new_and_worn(parts, model) {
            // Shoes: one item; its Edit Clothing option is New / Worn.
            let materials: Vec<Value> = parts.library.models[model]
                .materials
                .iter()
                .map(|m| json!({"selections":{&slot:{"asset_id":model,"material_id":m}},"colours":{&slot:Value::Null}}))
                .collect();
            let current = materials.iter().find(|p| patch_selected(&c.draft, p));
            let Some(shown) = current.or_else(|| materials.iter().find(|p| art.part(p, &gender).is_some())) else { continue };
            let Some(part) = art.part(shown, &gender) else { continue };
            if current.is_some() {
                worn = Some(rows.len());
            }
            rows.push(tile(part));
            acts.push(Act::Item(shown.clone(), Node::Garment(label, board, i, 0)));
            continue;
        }
        let designs = parts.library.models.get(model).map_or(0, |m| m.materials.len());
        let worn_now = worn_in(c, parts, &slot, &styles);
        for design in 0..designs {
            let Some(patch) = wear(parts, &slot, &styles, design, 0) else { continue };
            let Some(part) = art.part(&patch, &gender) else { continue };
            if worn_now.is_some_and(|(d, _)| d == design) {
                worn = Some(rows.len());
            }
            rows.push(tile(part));
            acts.push(if styles.len() > 1 || colourisable(parts, &patch) {
                Act::Item(patch, Node::Garment(label, board, i, design))
            } else {
                Act::Choose(Some(patch))
            });
        }
    }
    // As in retail, the customizable items lead their grid (after the remove tile): those the
    // retail string table names "Customizable ...", then anything else that takes a graphic
    // (the DLC beanies, the helmet).
    let rank = |r: &Row, a: &Act| match a {
        _ if r.text == "ID_CAC_REMOVE_PART" => 0,
        _ if r.text.starts_with("#Customizable") => 1,
        Act::Item(p, _) | Act::Choose(Some(p)) if customizable(parts, p) => 2,
        _ => 3,
    };
    let mut tiles: Vec<_> = rows.drain(..).zip(acts.drain(..)).enumerate().map(|(i, (r, a))| (r, a, worn == Some(i))).collect();
    tiles.sort_by_key(|(r, a, _)| rank(r, a));
    let worn = tiles.iter().position(|t| t.2);
    for (r, a, _) in tiles {
        rows.push(r);
        acts.push(a);
    }
    worn
}
/// Retail's customizable items: anything that takes a graphic (an IsLogoable tee, hoodie,
/// windbreaker, hat or deck).
fn customizable(parts: &Parts, patch: &Value) -> bool {
    slot_of(patch).is_some_and(|s| !graphic_zones(parts, &patch["selections"][s]).is_empty())
}
fn tile(part: crate::create_skater::PartArt) -> Row {
    // The thumbnail icon and info panel clips both have a "dlc" frame (the plug badge).
    let frame = if part.dlc { "dlc" } else { "owned" };
    Row {
        text: part.name.clone(),
        kind: frame,
        thumbnail: part.thumbnail,
        description: part.name,
        panel: frame,
        logo: part.logo,
        game: part.game,
        ..Default::default()
    }
}
/// The longest grid search (as the classic panel's).
pub(crate) const SEARCH_MAX: usize = 32;
/// Does a grid row answer a search (item 11, ours)? Its name holds the words (letters and digits only, any
/// case), or the search names its game ("skate 1", "Skate2", ...) or "dlc" (the tile's DLC badge). Rows named by
/// a string ID (Remove tiles) answer only an empty search.
pub(crate) fn found(row: &Row, search: &str) -> bool {
    let plain = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_lowercase();
    let wanted = plain(search);
    wanted.is_empty()
        || row.text.strip_prefix('#').is_some_and(|name| plain(name).contains(&wanted))
        || !row.game.is_empty() && plain(row.game) == wanted
        || wanted == "dlc" && row.kind == "dlc"
}
/// Keep the grid rows `search` finds (with their acts). The highlight stays on the worn tile if it is kept,
/// else the first.
pub(crate) fn search(built: &mut Built, search: &str) {
    keep(built, |row| found(row, search));
}
/// Not retail: the games a clothing or board grid shows, stepped through with RB (and back with LB); all first.
pub(crate) const GAMES: [&str; 4] = ["", "Skate 3", "Skate 2", "Skate 1"];
/// The bar's hint on those grids, after the search's: "LB RB Game".
pub(crate) const GAME_LEGEND: [(&str, &str); 2] = [("L_Bumper", "#"), ("R_Bumper", "#Game")];
/// The GAMES index after RB / LB on `game`.
pub(crate) fn next_game(game: usize, rb: bool, lb: bool) -> usize {
    (game + usize::from(rb) + usize::from(lb) * (GAMES.len() - 1)) % GAMES.len()
}
/// Keep the grid rows from `game` (every row for ""); rows from no game (the Remove tile) stay.
pub(crate) fn only_game(built: &mut Built, game: &str) {
    keep(built, |row| game.is_empty() || row.game.is_empty() || row.game == game);
}
/// Keep the grid rows `wanted` takes (with their acts). The highlight stays on the worn tile if it is kept,
/// else the first.
fn keep(built: &mut Built, wanted: impl Fn(&Row) -> bool) {
    let worn = built.page.index;
    let mut index = 0;
    let mut kept = 0;
    let rows = std::mem::take(&mut built.page.rows);
    let acts = std::mem::take(&mut built.acts);
    for (i, (row, act)) in rows.into_iter().zip(acts).enumerate() {
        if wanted(&row) {
            if i == worn {
                index = kept;
            }
            kept += 1;
            built.page.rows.push(row);
            built.acts.push(act);
        }
    }
    built.page.index = index;
}

pub(crate) fn build(node: Node, c: &Customiser, parts: &Parts, art: &Art) -> Built {
    let mut list: Vec<(Row, Act)> = Vec::new();
    let mut frame = Frame::Body;
    let mut strip = None;
    let mut index = None;
    let (title, mode, accept, back): (&str, Mode, &str, &str) = match node {
        Node::Root => {
            list.push(open("ID_CAC_BACKALLEY_PETES_TITLE", Node::Body, BODY_BLURB));
            list.push(open("ID_CAC_SLAPPYS_MERCH_TITLE", Node::Merch, "ID_CAC_SLAPPYS_MERCH_DESC"));
            list.push(open("ID_CAC_SKATE_STYLE_TITLE", Node::Style, "ID_CAC_SKATE_STYLE_DESC"));
            for (text, key, names, desc) in [
                ("ID_CAC_LABEL_TRUCK_TIGHTNESS", "truck", ["ID_CAC_EDIT_TRUCKS_LOOSE", "ID_CAC_EDIT_TRUCKS_MEDIUM", "ID_CAC_EDIT_TRUCKS_TIGHT"], "ID_CAC_LABEL_TRUCK_DESC"),
                ("ID_CAC_LABEL_WHEEL_STIFFNESS", "wheel", ["ID_CAC_EDIT_WHEELS_SOFT", "ID_CAC_EDIT_WHEELS_MEDIUM", "ID_CAC_EDIT_WHEELS_HARD"], "ID_CAC_LABEL_WHEEL_DESC"),
            ] {
                let v = scalar_of(&c.draft, key, 0.7);
                let mut r = row(text, "slider", desc);
                r.value = (v * 10.).round().clamp(0., 10.);
                r.value_text = names[((v * 3.).floor() as usize).min(2)].into();
                list.push((r, Act::Slider { key: key.into(), lo: 0., hi: 1., step: 0.1, initial: 0.7 }));
            }
            // Not retail (user 10-13, feature #1): the board's size (animation::BOARD_SIZES).
            let sizes = crate::animation::BOARD_SIZES;
            let v = scalar_of(&c.draft, "board_size", 1.).round().clamp(0., 3.);
            let mut r = row("Board size", "slider", "Penny, standard, wide or longboard. Changes the look only: the board rides as a standard deck.");
            r.value = (v * 10. / 3.).round();
            r.value_text = sizes[v as usize].0.into();
            list.push((r, Act::Slider { key: "board_size".into(), lo: 0., hi: 3., step: 1., initial: 1. }));
            ("ID_CAC_EDITSKATER_TITLE", Mode::Morph, "ID_COMMON_SELECT", "ID_CAC_EXIT")
        }
        Node::Body => {
            let gender = if c.gender() == "female" { "ID_CAC_CHOOSE_GENDER_FEMALE" } else { "ID_CAC_CHOOSE_GENDER_MALE" };
            list.push((row(gender, "", ""), Act::Gender));
            for (text, n) in [
                ("ID_CAC_SKIN_TITLE", Node::Skin),
                ("ID_CAC_HAIR_TITLE", Node::Hair),
                ("ID_CAC_BODY_SHAPE_TITLE", Node::Shape),
                ("ID_CAC_PRESETS_TITLE", Node::Presets),
                ("ID_CAC_FACE_MODIFICATION_TITLE", Node::Face),
                ("ID_CAC_FACIAL_HAIR_TITLE", Node::FacialHair),
                ("ID_CAC_UPPER_BODY_TATTOO", Node::Tattoo(true)),
                ("ID_CAC_LOWER_BODY_TATTOO", Node::Tattoo(false)),
            ] {
                // ponytail: facial hair is male-only, as the head materials are.
                if n == Node::FacialHair && c.gender() == "female" {
                    continue;
                }
                list.push((row(text, "", ""), Act::Open(n)));
            }
            ("ID_CAC_BODY_MODIFICATION_TITLE", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::Skin => {
            let tones = patches(c, &["Body", "Skin tone"]);
            list.push(swatches(c, "ID_CAC_SKIN_COLOUR_TITLE", tones.into_iter().map(|(_, p)| (tint(&p, "skin_tint"), p)).collect()));
            list.push(undo());
            ("ID_CAC_SKIN_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Hair => {
            frame = Frame::Face;
            let styles: Vec<(String, Value)> = c
                .entry(&["Body", "Hair"])
                .map(|e| c.for_gender(e))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| {
                    let id = format!("ID_CAC_HAIRSTYLE_{}", e.label.to_uppercase().replace("MOUSSED", "MOOSED").replace(' ', "_"));
                    Some((art.string(&id, &e.label), e.options.first()?.patch.clone()?))
                })
                .collect();
            list.push(selector(c, "ID_CAC_HAIR_STYLE", styles, BODY_BLURB));
            // Not retail (user 10-11, item 31): the colour wheel in place of the retail swatches. It starts on the
            // retail colour worn (its swatch), else the tint's colour, else the worn hair's own.
            let own = c.draft["selections"]["Hair"]["material_id"].as_str().and_then(|m| parts.library.materials.get(m));
            let authored = own.map_or([0.; 3], |m| hair_shown(m.tint));
            let retail = c
                .entry(&["Body", "Hair colour"])
                .map(|e| c.for_gender(e))
                .unwrap_or_default()
                .into_iter()
                .find(|e| e.patch.as_ref().is_some_and(|p| patch_selected(&c.draft, p)))
                .and_then(|e| e.swatch);
            let start = retail.or_else(|| rgb_of(&c.draft["hair_tint"]).map(hair_shown)).unwrap_or(authored);
            let default = json!({"hair_tint": null});
            let custom = Custom { template: json!({"hair_tint": [0, 0, 0]}), at: "/hair_tint".into(), start, authored, default, hair: true };
            list.push(wheel_row("ID_CAC_HAIR_COLOUR", custom));
            list.push(undo());
            ("ID_CAC_HAIR_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Shape => {
            list.push(morph(c, "ID_CAC_BODY_SHAPE_SIZE", "morphs.fat"));
            list.push(undo());
            ("ID_CAC_BODY_SHAPE_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Presets => {
            frame = Frame::Face;
            let mut choices: Vec<(String, Value)> =
                patches(c, &["Body", "Face presets"]).into_iter().enumerate().map(|(i, (_, p))| (format!("ID_CAC_PRESET_{:02}", i % 10 + 1), p)).collect();
            let mut r = selector(c, "", choices.clone(), BODY_BLURB);
            if r.0.value_text.is_empty() {
                r.0.value_text = "ID_CAC_PRESET_CUSTOM".into();
            }
            // Retail row 95 (ctor 0x825f4b98): a "category" row, as the gestures are.
            r.0.kind = "category";
            choices.clear();
            list.push(r);
            ("ID_CAC_PRESETS_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Face => {
            frame = Frame::Face;
            for (text, n) in [
                ("ID_CAC_EYES_TITLE", Node::Eyes),
                ("ID_CAC_BROW_TITLE", Node::Brow),
                ("ID_CAC_NOSE_TITLE", Node::Nose),
                ("ID_CAC_JAW_TITLE", Node::Jaw),
                ("ID_CAC_MOUTH_TITLE", Node::Mouth),
                ("ID_CAC_CHIN_TITLE", Node::Chin),
            ] {
                list.push((row(text, "", ""), Act::Open(n)));
            }
            ("ID_CAC_FACE_MODIFICATION_TITLE", Mode::List, "ID_COMMON_ACCEPT", "ID_COMMON_BACK")
        }
        Node::Eyes => {
            frame = Frame::Face;
            // Retail eye swatches (cac_colour *_eye display colours), in swatch order.
            const EYES: [(&str, [f32; 3]); 5] = [
                ("blue", [0.29, 0.431, 0.592]),
                ("dark blue", [0.2, 0.267, 0.337]),
                ("green", [0.369, 0.388, 0.196]),
                ("light brown", [0.392, 0.259, 0.129]),
                ("dark brown", [0.2, 0.129, 0.11]),
            ];
            let found = variants(c, parts, "Organ", "EyeColour");
            let asset = c.draft["selections"]["Organ"]["asset_id"].clone();
            let choices = EYES
                .iter()
                .filter_map(|(name, rgb)| {
                    let (_, id) = found.iter().find(|(v, _)| v == name)?;
                    Some((*rgb, json!({"selections":{"Organ":{"asset_id":asset,"material_id":id}}})))
                })
                .collect();
            list.push(swatches(c, "ID_CAC_EYES_COLOUR", choices));
            list.push(morph(c, "ID_CAC_EYES_ROTATION", "morphs.local_eye_rotation"));
            list.push(morph(c, "ID_CAC_EYES_WIDTH", "morphs.local_eye_width"));
            list.push(morph(c, "ID_CAC_EYES_HEIGHT", "morphs.local_eye_height"));
            list.push(undo());
            ("ID_CAC_EYES_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Brow => {
            frame = Frame::Face;
            let styles = variants(c, parts, "Rostral", "EyebrowStyle")
                .into_iter()
                .map(|(style, _)| (format!("ID_CAC_EYEBROW_{}", style.to_uppercase()), json!({ "brow": style })))
                .collect();
            let mut brow = selector(c, "ID_CAC_BROW_STYLE", styles, BODY_BLURB);
            if brow.0.value_text.is_empty() {
                brow.0.value_text = format!("ID_CAC_EYEBROW_{}", head_flag(c, parts, "EyebrowStyle").to_uppercase());
            }
            list.push(brow);
            list.push(morph(c, "ID_CAC_BROW_PROFILE", "morphs.local_brows_depth"));
            list.push(morph(c, "ID_CAC_BROW_ROTATION", "morphs.local_brows_rotation"));
            list.push(morph(c, "ID_CAC_BROW_HEIGHT", "morphs.local_brows_height"));
            list.push(undo());
            ("ID_CAC_BROW_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Nose => {
            frame = Frame::Face;
            list.push(morph(c, "ID_CAC_NOSE_LENGTH", "morphs.local_nose_length"));
            list.push(morph(c, "ID_CAC_NOSE_WIDTH", "morphs.local_nose_width"));
            list.push(morph(c, "ID_CAC_NOSE_HEIGHT", "morphs.local_nose_height"));
            list.push(morph(c, "ID_CAC_NOSE_CURVE", "morphs.local_nose_curve"));
            list.push(undo());
            ("ID_CAC_NOSE_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Jaw => {
            frame = Frame::Face;
            list.push(morph(c, "ID_CAC_JAW_DEFINITION", "morphs.local_jaw_chiseled"));
            list.push(morph(c, "ID_CAC_JAW_ROUNDNESS", "morphs.local_jaw_depth"));
            list.push(undo());
            ("ID_CAC_JAW_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Mouth => {
            frame = Frame::Face;
            list.push(morph(c, "ID_CAC_MOUTH_WIDTH", "morphs.local_mouth_width"));
            list.push(morph(c, "ID_CAC_MOUTH_FULLNESS", "morphs.local_mouth_lipsize"));
            list.push(morph(c, "ID_CAC_MOUTH_SMILE", "morphs.local_mouth_corner"));
            list.push(undo());
            ("ID_CAC_MOUTH_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Chin => {
            frame = Frame::Face;
            list.push(morph(c, "ID_CAC_CHIN_LENGTH", "morphs.local_chin_length"));
            list.push(undo());
            ("ID_CAC_CHIN_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::FacialHair => {
            frame = Frame::Face;
            let styles = patches(c, &["Body", "Facial hair"])
                .into_iter()
                .map(|(_, p)| {
                    let id = p["beard"].as_str().unwrap_or("").to_uppercase().replace(' ', "_");
                    let id = match id.as_str() {
                        "BILLY_GOAT" | "CLEAN_SHAVEN" | "FULL_BEARD" | "SOUL_PATCH" => id,
                        "HANDLE_BAR" => "HANDLE_BAR".into(),
                        "STACHE" => "STACHE".into(),
                        other => other.into(),
                    };
                    (format!("ID_CAC_FACIAL_HAIR_{id}"), p)
                })
                .collect();
            let mut beard = selector(c, "ID_CAC_FACIAL_HAIR_STYLE", styles, BODY_BLURB);
            if beard.0.value_text.is_empty() {
                beard.0.value_text = format!("ID_CAC_FACIAL_HAIR_{}", head_flag(c, parts, "FacialHairStyle").to_uppercase().replace(' ', "_"));
            }
            list.push(beard);
            list.push(undo());
            ("ID_CAC_FACIAL_HAIR_TITLE", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Tattoo(upper) => {
            let sides: &[(&str, u8)] = if upper {
                &[
                    ("ID_CAC_CUSTOMGRAPHIC_UPPERTATTOO_FRONT", 2),
                    ("ID_CAC_CUSTOMGRAPHIC_UPPERTATTOO_BACK", 3),
                    ("ID_CAC_CUSTOMGRAPHIC_UPPERTATTOO_LEFTARM", 0),
                    ("ID_CAC_CUSTOMGRAPHIC_UPPERTATTOO_RIGHTARM", 1),
                ]
            } else {
                &[("ID_CAC_CUSTOMGRAPHIC_LOWERATTOO_LEFTLEG", 0), ("ID_CAC_CUSTOMGRAPHIC_LOWERATTOO_RIGHTLEG", 1)]
            };
            for (text, side) in sides {
                list.push((row(text, "", ""), Act::Open(Node::TattooKind(upper, *side))));
            }
            (if upper { "ID_CAC_UPPER_BODY_TATTOO" } else { "ID_CAC_LOWER_BODY_TATTOO" }, Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::TattooKind(upper, side) => {
            for category in 1..=3u8 {
                list.push((row(&format!("ID_CAC_TATTOO_CATEGORY_0{category}"), "", ""), Act::Open(Node::TattooGrid(upper, side, category))));
            }
            // Category rows only: retail's row table (ctor 0x825f4b98) has no Graphic Adjust row; a worn
            // tattoo is re-adjusted by choosing it again in its grid, which keeps its adjust. Not retail (user 10-30):
            // B goes back to the spots, so opening one doesn't force a tattoo.
            ("ID_CAC_TATTOOS_TITLE", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        // Graphic Adjust: the movie's stick legend (create_skater::adjust_graphic drives it).
        Node::TattooAdjust(upper) => {
            strip = Some(if upper { "OuterTorso" } else { "Pants" });
            frame = if upper { Frame::Torso } else { Frame::Legs };
            ("ID_CAC_GRAPHIC_ADJUST", Mode::Tattoo, "ID_COMMON_ACCEPT", "ID_CAC_CHANGE_GRAPHIC")
        }
        Node::Graphic(zone) => {
            frame = Frame::of_zone(zone);
            ("ID_CAC_GRAPHIC_ADJUST", Mode::Scale, "ID_COMMON_ACCEPT", "ID_CAC_CHANGE_GRAPHIC")
        }
        Node::GraphicLocation => {
            use crate::custom_graphic::{SHIRT, ZONES};
            frame = Frame::Torso;
            let worn: Vec<&str> = c.draft["selections"].as_object().into_iter().flatten().flat_map(|(_, w)| graphic_zones(parts, w)).collect();
            // A top wears one stock logo: choosing a location moves it there, its adjust re-seeded from
            // that location's retail start (0x825fe3c0 sets the location, then 0x82536fa0), then Graphics.
            // Creator designs stay on their own sides, and a logo does not go onto one, so a side with a
            // design moves nothing.
            let graphics = &c.draft["graphics"];
            let worn_logo = SHIRT.iter().find_map(|z| crate::custom_graphic::logo(&graphics[z]));
            for &(zone, _, text) in ZONES.iter().filter(|z| SHIRT.contains(&z.0) && worn.contains(&z.0)) {
                let mut patch: serde_json::Map<String, Value> = SHIRT.iter().map(|z| (z.to_string(), no_logo())).collect();
                if let Some(logo) = worn_logo {
                    let mut moved = no_logo();
                    moved["logo"] = json!(logo);
                    patch.insert(zone.into(), moved);
                }
                let patch = if crate::custom_graphic::design(&graphics[zone]).is_some() { json!({}) } else { json!({ "graphics": patch }) };
                list.push((row(text, "", ""), Act::Item(patch, Node::GraphicKind(zone))));
            }
            // Not retail (it ignores B here): B backs out with nothing chosen, as the tattoo spots do.
            ("ID_CAC_SHIRT_GRAPHIC_LOCATION", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::GraphicKind(zone) => {
            frame = Frame::of_zone(zone);
            // Brand and Team Logos, and the item's Custom Graphics (which always has its New tile). A side
            // wearing a Custom Graphic lists no logos (10-11: a team logo could be put over one); Remove
            // Graphic takes off whatever it wears.
            let designed = crate::custom_graphic::design(&c.draft["graphics"][zone]).is_some();
            for (kind, (text, _)) in GRAPHIC_KINDS.iter().enumerate() {
                if !designed || kind as u8 == CUSTOM {
                    list.push((row(text, "", ""), Act::Open(Node::GraphicGrid(zone, kind as u8))));
                }
            }
            let mut remove = no_logo();
            remove["design"] = Value::Null;
            list.push((row("ID_CAC_REMOVE_GRAPHIC", "", ""), Act::Choose(Some(json!({"graphics":{zone:remove}})))));
            ("ID_CAC_GRAPHICS_TITLE", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::GraphicGrid(zone, kind) => {
            frame = Frame::of_zone(zone);
            let entry = &c.draft["graphics"][zone];
            let mut tile = |name: String, thumbnail: String, patch: Value, worn: bool, game: &'static str| {
                if worn {
                    index = Some(list.len());
                }
                list.push((Row { text: name.clone(), kind: "none", thumbnail, description: name, game, ..Default::default() }, Act::Choose(Some(patch))));
            };
            let (_, library_kind) = GRAPHIC_KINDS[kind as usize % GRAPHIC_KINDS.len()];
            if library_kind.is_empty() {
                // Custom Graphics: New, then this item's saves, each a preview of the item wearing it.
                list.push((
                    Row { text: "#New Graphic".into(), kind: "none", description: "#Make a new graphic from shapes, logos and text".into(), ..Default::default() },
                    Act::CreatorNew(zone),
                ));
                if let Some((item, sides)) = graphic_item(c, parts, zone) {
                    migrate_old_graphics(c, parts);
                    let profile = c.profile_path();
                    let store = Store::load(profile);
                    for (n, saved) in store.saves(&item).into_iter().enumerate().take(graphic_store::MAX_SAVES) {
                        let png = graphic_store::tile_file(profile, &item, saved);
                        let thumbnail = if png.exists() { png.to_string_lossy().into_owned() } else { String::new() };
                        if wears(c, &sides, saved) {
                            index = Some(list.len());
                        }
                        let name = format!("#Custom Graphic {}", n + 1);
                        let row = Row { text: name.clone(), kind: "none", thumbnail, description: name, ..Default::default() };
                        list.push((row, Act::Item(wear_patch(&sides, saved), Node::SavedGraphic(zone, n as u8))));
                    }
                    forget_old_tiles(profile, &item, &store);
                }
            } else {
                let mut graphics: Vec<_> = parts.library.graphics.iter().filter(|(_, g)| g.kind == library_kind).collect();
                graphics.sort_by(|a, b| a.1.name.cmp(&b.1.name));
                for (id, g) in graphics {
                    // Retail's own 128x128 tile; the 2:1 decal (drawn at its shape) where there is none.
                    let decal = || parts.graphic_file(id).map_or(String::new(), |p| p.to_string_lossy().into_owned());
                    let thumbnail = art.graphic_tile(id).map_or_else(decal, str::to_owned);
                    let worn = entry["logo"].as_str() == Some(id.as_str());
                    tile(format!("#{}", g.name), thumbnail, json!({"graphics":{zone:{"logo":id}}}), worn, art.game(id));
                }
            }
            ("ID_CAC_THUMBNAILS_TITLE", Mode::ThumbsCustom, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::SavedGraphic(zone, n) => {
            frame = Frame::of_zone(zone);
            let n = n as usize;
            list.push((row("#Select", "", "#Wear it"), Act::Choose(None)));
            list.push((row("#Edit", "", "#Change it in the Graphic Creator"), Act::CreatorSaved(zone, n)));
            list.push((row("#Copy Code", "", "#Copy a code for this graphic to share. New's Paste Code puts it on the same item."), Act::CopyCode(zone, n)));
            list.push((row("#Delete", "", "#Throw it away"), Act::DeleteSaved(zone, n)));
            ("#Custom Graphic", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::TattooGrid(upper, side, category) => {
            strip = Some(if upper { "OuterTorso" } else { "Pants" });
            frame = if upper { Frame::Torso } else { Frame::Legs };
            let slot = if upper { "Arm" } else { "Leg" };
            let mut rows = Vec::new();
            let mut acts = Vec::new();
            // One tattoo per slot (retail's upper and lower tattoo categories): a different tattoo or
            // spot starts from the spot's retail start (0x825fdf58 -> 0x825fad28 re-seeds via
            // 0x82536fa0). The worn one on its spot keeps its adjust (INFERRED: retail's path for
            // it is not decoded), carried in its patch so a highlight preview can't lose it.
            let worn = &c.draft["tattoos"][slot];
            let spot = |id: Value| {
                let keep = worn["id"] == id && worn["side"].as_u64().unwrap_or(0) == u64::from(side);
                let mut e = json!({"id": id, "side": side});
                for (key, ..) in crate::custom_graphic::ADJUST {
                    e[key] = if keep { worn[key].clone() } else { Value::Null };
                }
                json!({"tattoos": {slot: e}})
            };
            let none = spot(Value::Null);
            if patch_selected(&c.draft, &json!({"tattoos":{slot:{"id":Value::Null}}})) {
                index = Some(0);
            }
            rows.push(Row { text: "ID_CAC_REMOVE_TATTOO".into(), kind: "none", description: "ID_CAC_REMOVE_TATTOO".into(), ..Default::default() });
            acts.push(Act::Choose(Some(none)));
            for (id, name, texture) in art.tattoos(category) {
                let patch = spot(json!(id));
                if patch_selected(&c.draft, &json!({"tattoos":{slot:{"id":id}}})) {
                    index = Some(rows.len());
                }
                rows.push(Row { text: name.clone(), kind: "none", thumbnail: texture, description: name, game: art.game(&id), ..Default::default() });
                acts.push(Act::Choose(Some(patch)));
            }
            list = rows.into_iter().zip(acts).collect();
            // Retail page 11 TattooThumbnails: mode 6, the 3x5 grid (page table 0x8302fa10).
            ("ID_CAC_THUMBNAILS_TITLE", Mode::ThumbsCustom, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::Merch => {
            for (text, n) in [
                ("ID_CAC_HATS_TITLE", Node::Clothes("Hats")),
                ("ID_CAC_TSHIRTS_TITLE", Node::Clothes("T-shirts")),
                ("ID_CAC_BUTTONSHIRTS_TITLE", Node::Clothes("Shirts")),
                ("ID_CAC_HOODIES_TITLE", Node::Clothes("Hoodies")),
                ("ID_CAC_JACKETS_TITLE", Node::Clothes("Jackets")),
                ("ID_CAC_SWEATERS_TITLE", Node::Clothes("Sweaters")),
                ("ID_CAC_LEGS_TITLE", Node::Clothes("Pants & shorts")),
                ("ID_CAC_SHOES_TITLE", Node::Clothes("Shoes")),
                ("ID_CAC_SOCKS_TITLE", Node::Clothes("Socks")),
                ("ID_CAC_ACCESSORIES_TITLE", Node::Accessories),
                ("ID_CAC_SKATEBOARDS_TITLE", Node::Boards),
            ] {
                list.push((row(text, "", ""), Act::Open(n)));
            }
            ("ID_CAC_SLAPPYS_MERCH_TITLE", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::Accessories => {
            for (text, n) in [
                ("ID_CAC_GLASSES_TITLE", "Glasses"),
                ("ID_CAC_WRIST_TITLE", "Wristwear"),
                ("ID_CAC_NECKLACES_TITLE", "Necklaces"),
            ] {
                list.push((row(text, "", ""), Act::Open(Node::Clothes(n))));
            }
            for (i, (text, n)) in ACCESSORY_TABS.into_iter().enumerate() {
                let items = c.entry(&grid_path(n, false)).map_or(0, |e| c.for_gender(e).iter().filter(|e| !e.options.is_empty()).count());
                if i == 0 || items > 0 {
                    list.push((row(text, "", ""), Act::Open(Node::Clothes(n))));
                }
            }
            ("ID_CAC_ACCESSORIES_TITLE", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::Clothes(label) | Node::Board(label) => {
            frame = match label {
                "Hats" | "Glasses" => Frame::Head,
                "Necklaces" => Frame::Face,
                "T-shirts" | "Shirts" | "Hoodies" | "Jackets" | "Sweaters" => Frame::Torso,
                "Pants & shorts" => Frame::Legs,
                "Shoes" | "Socks" => Frame::Feet,
                "Wristwear" | "Rings" | "Gloves" => Frame::Hands,
                // Ported tabs (no retail page): INFERRED views of where the item sits.
                "Belts" | "Chains" => Frame::Legs,
                "Backpacks" => Frame::Back,
                "Other" => Frame::Face,
                "Wheels" => Frame::BoardSide,
                _ if matches!(node, Node::Board(_)) => Frame::Board,
                _ => Frame::Body,
            };
            let mut rows = Vec::new();
            let mut acts = Vec::new();
            index = grid(c, parts, art, label, matches!(node, Node::Board(_)), &mut rows, &mut acts);
            list = rows.into_iter().zip(acts).collect();
            let title = match label {
                "Hats" => "ID_CAC_HATS_TITLE",
                "T-shirts" => "ID_CAC_TSHIRTS_TITLE",
                "Shirts" => "ID_CAC_BUTTONSHIRTS_TITLE",
                "Hoodies" => "ID_CAC_HOODIES_TITLE",
                "Jackets" => "ID_CAC_JACKETS_TITLE",
                "Sweaters" => "ID_CAC_SWEATERS_TITLE",
                "Pants & shorts" => "ID_CAC_LEGS_TITLE",
                "Shoes" => "ID_CAC_SHOES_TITLE",
                "Socks" => "ID_CAC_SOCKS_TITLE",
                "Glasses" => "ID_CAC_GLASSES_TITLE",
                "Necklaces" => "ID_CAC_NECKLACES_TITLE",
                "Wristwear" => "ID_CAC_WRIST_TITLE",
                "Deck" => "ID_CAC_BOARDS_TITLE",
                "Trucks" => "ID_CAC_TRUCKS_TITLE",
                "Wheels" => "ID_CAC_WHEELS_TITLE",
                _ => ACCESSORY_TABS.iter().find(|t| t.1 == label).map_or("ID_CAC_ACCESSORIES_TITLE", |t| t.0),
            };
            (title, Mode::Thumbs, "ID_COMMON_ACCEPT", "ID_COMMON_BACK")
        }
        Node::Garment(label, board, i, design) => {
            frame = match label {
                "Hats" | "Glasses" => Frame::Head,
                "Necklaces" => Frame::Face,
                "Pants & shorts" => Frame::Legs,
                "Shoes" | "Socks" => Frame::Feet,
                "Wristwear" | "Rings" | "Gloves" => Frame::Hands,
                "Belts" | "Chains" => Frame::Legs,
                "Backpacks" => Frame::Back,
                "Other" => Frame::Face,
                "Wheels" => Frame::BoardSide,
                _ if board => Frame::Board,
                _ => Frame::Torso,
            };
            let entry = c.entry(&grid_path(label, board)).and_then(|e| c.for_gender(e).into_iter().nth(i));
            let options: Vec<Value> = entry.map(|e| e.options.iter().filter_map(|o| o.patch.clone()).collect()).unwrap_or_default();
            let slot = options.iter().find_map(slot_of).unwrap_or("").to_owned();
            let worn = &c.draft["selections"][&slot];
            // Color: the worn model/design's. Not retail (user 10-11, item 31): the colour wheel in place of the
            // library's clothing colour swatches; it starts on the colour worn, else the material's own.
            let colourizable = |m: &&crate::customiser_parts::Material| m.flag("IsColourizable").eq_ignore_ascii_case("true");
            if let Some(material) = worn["material_id"].as_str().and_then(|m| parts.library.materials.get(m)).filter(colourizable) {
                let custom = Custom {
                    template: json!({"selections": {&slot: worn.clone()}, "colours": {&slot: [0, 0, 0]}}),
                    at: format!("/colours/{slot}"),
                    start: rgb_of(&c.draft["colours"][&slot]).unwrap_or(material.tint),
                    authored: material.tint,
                    default: json!({"selections": {&slot: worn.clone()}, "colours": {&slot: null}}),
                    hair: false,
                };
                list.push(wheel_row("ID_CAC_WEARSTYLES_COLOUR", custom));
            }
            let model = options.iter().find_map(model_of).unwrap_or("").to_owned();
            let choices: Vec<(String, Value)> = if board || new_and_worn(parts, &model) {
                options.iter().filter(|p| !recoloured(p)).enumerate().map(|(n, p)| (format!("ID_CAC_WEAR_STYLE_OPTION_{}", n % 16 + 1), p.clone())).collect()
            } else {
                let styles = art.wear_styles(&model);
                (0..styles.len())
                    .filter_map(|n| Some((format!("ID_CAC_WEAR_STYLE_OPTION_{}", n % 16 + 1), wear(parts, &slot, &styles, design, n)?)))
                    .collect()
            };
            if choices.len() > 1 {
                let mut option = selector(c, "", choices, "ID_CAC_SLAPPYS_MERCH_DESC");
                if option.0.value_text.is_empty() {
                    option.0.value_text = "ID_CAC_WEAR_STYLE_OPTION_1".into();
                }
                list.push(option);
            }
            // Add Team Logo (retail WearStyles row): a top's Graphic Location list, or
            // straight to Graphics on the customizable hat and deck.
            let zones = graphic_zones(parts, worn);
            if let Some(&zone) = zones.first() {
                let next = if zones.len() > 1 { Node::GraphicLocation } else { Node::GraphicKind(zone) };
                list.push(open("ID_CAC_ADD_GRAPHIC", next, "ID_CAC_SLAPPYS_MERCH_DESC"));
            }
            (if board && label == "Deck" { "ID_CUSTOM_EDITDECK" } else { "ID_CAC_WEAR_STYLES_TITLE" }, Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Boards => {
            for (text, n) in [("ID_CAC_BOARDS_TITLE", "Deck"), ("ID_CAC_TRUCKS_TITLE", "Trucks"), ("ID_CAC_WHEELS_TITLE", "Wheels")] {
                list.push((row(text, "", ""), Act::Open(Node::Board(n))));
            }
            list.push((row("#Grip Color", "", ""), Act::Open(Node::Grip)));
            ("ID_CAC_SKATEBOARDS_TITLE", Mode::List, "ID_COMMON_SELECT", "ID_COMMON_BACK")
        }
        Node::Grip => {
            // Its own edit page: the Skateboards list is a retail List page, whose movie draws no swatches.
            frame = Frame::Board;
            let grip = &c.draft["grip"];
            let solid = grip["solid"].as_bool() == Some(true);
            // Not retail (user 10-11, item 31): the colour wheel in place of the swatches. Its Default (Y) is no Grip
            // Colour, the authored grip, shown as its mean (the Skateboard deck's grip half).
            const AUTHORED: [f32; 3] = [0.256, 0.272, 0.296];
            let custom = Custom {
                template: json!({"grip": {"rgb": [0, 0, 0], "solid": solid}}),
                at: "/grip/rgb".into(),
                start: rgb_of(&grip["rgb"]).unwrap_or(AUTHORED),
                authored: AUTHORED,
                default: json!({"grip": null}),
                hair: false,
            };
            list.push(wheel_row("ID_CAC_WEARSTYLES_COLOUR", custom));
            if !grip.is_null() {
                let style = |solid: bool| json!({"grip": {"rgb": grip["rgb"], "solid": solid}});
                let styles = vec![("#Realistic".into(), style(false)), ("#Solid".into(), style(true))];
                list.push(selector(c, "#Style", styles, "ID_CAC_SLAPPYS_MERCH_DESC"));
            }
            ("#Grip Color", Mode::Morph, "ID_COMMON_ACCEPT", "#")
        }
        Node::Style => {
            let gestures = c.entry(&["Style", "Gestures"]).map(|e| e.children.clone()).unwrap_or_default();
            // Retail row order follows the movie's button icons: Up, Right, Left, Down.
            for direction in GESTURE_ROWS {
                let Some(entry) = gestures.get(direction) else { continue };
                let choices: Vec<(String, Value)> = entry
                    .children
                    .iter()
                    .filter_map(|g| Some((gesture_name(&g.label), g.patch.clone()?)))
                    .collect();
                let (mut r, act) = selector(c, "", choices.clone(), "ID_CAC_SKATE_GESTURE_DESC");
                // Unassigned directions use the first four gestures (ResetGestureSet).
                if r.value_text.is_empty() {
                    r.value_text = choices.get(direction).map_or(String::new(), |(l, _)| l.clone());
                }
                r.kind = "category";
                r.text = r.value_text.clone();
                list.push((r, act));
            }
            let named = |labels: &[&str], ids: &[&str], desc: &str, text: &str| {
                let choices: Vec<(String, Value)> =
                    patches(c, labels).into_iter().enumerate().map(|(i, (l, p))| (ids.get(i).map_or(format!("#{l}"), |s| s.to_string()), p)).collect();
                selector(c, text, choices, desc)
            };
            list.push(named(&["Style", "Stance"], &["ID_CAC_STANCE_REGULAR", "ID_CAC_STANCE_GOOFY"], "ID_CAC_EDIT_STANCE_DESC", "ID_CAC_EDIT_STANCE"));
            list.push(named(
                &["Style", "Skating style"],
                &["ID_CAC_LABEL_STYLE_NORMAL", "ID_CAC_LABEL_STYLE_LOOSE", "ID_CAC_LABEL_STYLE_GONZO", "ID_CAC_LABEL_STYLE_AGGRESSIVE"],
                "ID_CAC_EDIT_STYLE_DESC",
                "ID_CAC_EDIT_STYLE",
            ));
            list.push(named(
                &["Style", "Posture"],
                &["ID_CAC_LABEL_POSTURE_1", "ID_CAC_LABEL_POSTURE_2", "ID_CAC_LABEL_POSTURE_3", "ID_CAC_LABEL_POSTURE_4"],
                "ID_CAC_EDIT_POSTURE_DESC",
                "ID_CAC_EDIT_POSTURE",
            ));
            ("ID_CAC_SKATE_STYLE_TITLE", Mode::Style, "ID_COMMON_ACCEPT", "#")
        }
    };
    let (rows, acts): (Vec<Row>, Vec<Act>) = list.into_iter().unzip();
    let index = index.unwrap_or(0).min(rows.len().saturating_sub(1));
    // Item 31 (not retail, user 10-11): A on the colour row opens the colour wheel, so B leaves the page as A
    // does on its other rows (a page may have no other row).
    let back = if acts.iter().any(|a| matches!(a, Act::Wheel(_))) { "ID_COMMON_BACK" } else { back };
    Built {
        page: Page { title: title.into(), mode, rows, index, accept: accept.into(), back: back.into() },
        acts,
        frame,
        strip,
    }
}

/// The choice a Left/Right goes to: the next one round, or the first when none is current.
fn next(c: &Customiser, choices: &[Value], direction: i32) -> usize {
    match current(c, choices) {
        Some(i) => (i as i32 + direction).rem_euclid(choices.len() as i32) as usize,
        None => 0,
    }
}
/// Step a row's value (Left/Right); true when something changed.
pub(crate) fn step(c: &mut Customiser, parts: &Parts, act: &Act, direction: i32) -> bool {
    match act {
        Act::Cycle(choices) if !choices.is_empty() => {
            c.apply(parts, Some(&choices[next(c, choices, direction)]));
            true
        }
        Act::Slider { key, lo, hi, step, initial } => {
            let v = (scalar_of(&c.draft, key, *initial) + *step * direction as f64).clamp(*lo, *hi);
            // Whole steps: a long walk of float additions would not land back on 0. Clamped again: the snap lands the
            // truck/wheel floor on -1e-16, which Parts::resolve refuses, so they stopped one bar short (bug 10-25 #20).
            let v = (((v - initial) / step).round() * step + initial).clamp(*lo, *hi);
            c.set_scalar(parts, key, v);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core_menu_runtime::Key,
        create_skater_runtime::{Outcome, Runtime},
        customiser_parts::Library,
    };

    /// The prepared set: SKATE_CAC_TEST_LIBRARY, CREATE_SKATER_MANIFEST.
    fn prepared() -> (String, Value, Parts, Customiser, Art) {
        let library = std::env::var("SKATE_CAC_TEST_LIBRARY").unwrap();
        let manifest = std::env::var("CREATE_SKATER_MANIFEST").unwrap();
        let source: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        let lib: Library = serde_json::from_slice(&std::fs::read(&library).unwrap()).unwrap();
        let extras: Vec<crate::customiser::Entry> =
            serde_json::from_slice(&std::fs::read(std::path::Path::new(&library).with_file_name("extra-menu.json")).unwrap()).unwrap();
        let assets = std::path::Path::new(&manifest).ancestors().nth(4).unwrap().to_path_buf();
        let art = Art::load(&std::path::Path::new(&library).with_file_name("catalog.json"), &lib, &source, &assets).unwrap();
        let parts = Parts::for_test(lib);
        let c = Customiser::for_test(&parts, extras, "male");
        (manifest, source, parts, c, art)
    }

    /// Every page reachable from the root builds and can be left. Retail ignores B where its
    /// legend is hidden (edit pages: Runtime::input), so on an edit page some row's A must
    /// return; Graphic Adjust has no rows, so A always leaves it.
    #[test]
    #[ignore]
    fn every_reachable_page_builds_and_can_be_left() {
        let (_, _, parts, mut c, art) = prepared();
        let mut seen = Vec::new();
        let mut todo = vec![(Node::Root, None)];
        let mut only_b_leaves = Vec::new();
        while let Some((node, wear)) = todo.pop() {
            if seen.contains(&node) {
                continue;
            }
            seen.push(node);
            // As in game, choosing an item wears it before its Edit Clothing page opens.
            if let Some(patch) = &wear {
                c.apply(&parts, Some(patch));
            }
            let built = build(node, &c, &parts, &art);
            assert_eq!(built.page.rows.len(), built.acts.len(), "{node:?}");
            // create_skater::interact: A on an edit page returns unless the row opens,
            // chooses or undoes something.
            let a_returns = |a: &Act| !matches!(a, Act::Open(_) | Act::Item(..) | Act::Gender | Act::Undo | Act::Choose(_));
            if node.is_editor() && !built.acts.is_empty() && !built.acts.iter().any(a_returns) {
                only_b_leaves.push(node);
            }
            if node.is_adjust() {
                assert!(built.page.rows.is_empty() && built.page.back == "ID_CAC_CHANGE_GRAPHIC", "{node:?}: the retail legend");
            }
            for a in built.acts {
                match a {
                    Act::Open(next) => todo.push((next, None)),
                    Act::Item(patch, next) => todo.push((next, Some(patch))),
                    // A logo choice lands on Graphic Adjust.
                    Act::Choose(Some(patch)) => {
                        if let Node::GraphicGrid(zone, _) = node {
                            todo.push((Node::Graphic(zone), Some(patch)));
                        }
                    }
                    _ => {}
                }
            }
        }
        eprintln!("{} pages; edit pages only B leaves: {only_b_leaves:?}", seen.len());
        assert!(only_b_leaves.is_empty(), "B is ignored on edit pages, so these strand the player: {only_b_leaves:?}");
        assert!(seen.len() > 100, "the whole tree was walked");
        for page in [Node::GraphicLocation, Node::GraphicKind("front"), Node::GraphicKind("hat"), Node::GraphicKind("deck"), Node::Graphic("front")] {
            assert!(seen.contains(&page), "{page:?} reached");
        }
    }

    /// create_skater::interact's page stack over the real movie and built pages (its
    /// Screen holds a Bevy canvas, so the system itself cannot run headless).
    /// ponytail: mirrors interact's outcome match; keep the two in step.
    struct Sim<'a> {
        runtime: Runtime,
        /// (page, draft when entered)
        stack: Vec<(Node, Value)>,
        acts: Vec<Act>,
        c: Customiser,
        parts: &'a Parts,
        art: &'a Art,
    }
    impl Sim<'_> {
        fn top(&self) -> Node {
            self.stack.last().unwrap().0
        }
        fn show(&mut self) {
            let built = build(self.top(), &self.c, self.parts, self.art);
            self.acts = built.acts;
            self.runtime.show(built.page).unwrap();
            for _ in 0..20 {
                self.runtime.advance().unwrap();
            }
        }
        fn push(&mut self, node: Node) {
            self.stack.push((node, self.c.draft.clone()));
            self.show();
        }
        fn pop(&mut self) {
            self.stack.pop();
            self.show();
        }
        fn press(&mut self, key: Key) {
            let node = self.top();
            let outcome = self.runtime.input(key).unwrap();
            for _ in 0..4 {
                self.runtime.advance().unwrap();
            }
            let acts = self.acts.clone();
            let act = |row: usize| acts.get(row).cloned();
            match outcome {
                None => {}
                Some(Outcome::Hilight(row)) => match act(row) {
                    Some(Act::Choose(p)) if node.is_grid() => self.c.apply(self.parts, p.as_ref()),
                    Some(Act::Item(p, _)) if node.is_grid() => self.c.apply(self.parts, Some(&p)),
                    _ => {}
                },
                Some(Outcome::Adjust(row, direction)) => {
                    if let Some(a) = act(row) {
                        step(&mut self.c, self.parts, &a, direction);
                    }
                }
                Some(Outcome::Accept(row)) => match act(row) {
                    Some(Act::Open(next)) => self.push(next),
                    Some(Act::Item(p, next)) => {
                        self.c.apply(self.parts, Some(&p));
                        self.push(next);
                    }
                    Some(Act::Choose(p)) => {
                        let tattoo = p.as_ref().is_some_and(|p| p["tattoos"].as_object().is_some_and(|t| t.values().any(|e| e["id"].is_string())));
                        self.c.apply(self.parts, p.as_ref());
                        match node {
                            Node::GraphicGrid(zone, _) => self.push(Node::Graphic(zone)),
                            Node::TattooGrid(upper, ..) if tattoo => self.push(Node::TattooAdjust(upper)),
                            Node::GraphicKind(_) | Node::SavedGraphic(..) => self.leave_flow(),
                            _ if node.is_grid() => self.back_to_shop(),
                            _ => self.show(),
                        }
                    }
                    Some(Act::Undo) => {
                        self.c.draft = self.stack.last().unwrap().1.clone();
                        self.show();
                    }
                    // The Graphic Creator opened and was left unchanged.
                    Some(Act::CreatorNew(_) | Act::CreatorSaved(..) | Act::CopyCode(..)) => self.show(),
                    // Delete asked and answered Yes.
                    Some(Act::DeleteSaved(zone, n)) => {
                        let grid = self.stack.len().checked_sub(2);
                        let mut entered = grid.map_or_else(|| self.c.draft.clone(), |g| self.stack[g].1.clone());
                        delete_saved(&mut self.c, self.parts, zone, n, &mut entered);
                        if let Some(g) = grid {
                            self.stack[g].1 = entered;
                        }
                        self.pop();
                    }
                    _ if node.is_adjust() => self.leave_flow(),
                    _ if node.is_editor() => self.leave(node),
                    Some(a) => {
                        step(&mut self.c, self.parts, &a, 1);
                    }
                    None => {}
                },
                // create_skater::interact: item 31's B on Edit Clothing leaves as its A does.
                Some(Outcome::Back) if matches!(node, Node::Garment(..)) => self.back_to_shop(),
                Some(Outcome::Back) => {
                    if node.is_grid() {
                        self.c.draft = self.stack.last().unwrap().1.clone();
                    }
                    self.pop();
                }
            }
        }
        /// An edit page's A: Edit Clothing goes back to the shop, others pop.
        fn leave(&mut self, node: Node) {
            if let Node::Garment(..) = node {
                self.back_to_shop();
            } else {
                self.pop();
            }
        }
        fn back_to_shop(&mut self) {
            if !self.stack.iter().any(|l| l.0 == Node::Merch) {
                return self.pop();
            }
            while self.top() != Node::Merch {
                self.stack.pop();
            }
            self.show();
        }
        fn leave_flow(&mut self) {
            while self.stack.len() > 1 && self.top().in_graphic_flow() {
                self.stack.pop();
            }
            self.show();
        }
        /// Highlight `row` with the movie's own navigation (Right/Left in grids), then A.
        fn choose(&mut self, row: usize) {
            for _ in 0..400 {
                let at = self.runtime.bindings.page.index;
                if at == row {
                    break;
                }
                let grid = self.top().is_grid();
                self.press(match (at < row, grid) {
                    (true, true) => Key::Right,
                    (false, true) => Key::Left,
                    (true, false) => Key::Down,
                    (false, false) => Key::Up,
                });
            }
            assert_eq!(self.runtime.bindings.page.index, row, "{:?}: highlight row {row}", self.top());
            self.press(Key::Accept);
        }
        fn row(&self, what: &str, f: impl Fn(&Act) -> bool) -> usize {
            self.acts.iter().position(f).unwrap_or_else(|| panic!("{:?}: no {what} row", self.top()))
        }
    }

    /// User 10-30: from a tattoo spot's categories, and from a category's grid with a tattoo highlighted, B goes back out
    /// past the spots to Appearance, with nothing chosen.
    #[test]
    #[ignore]
    fn b_backs_out_of_a_tattoo_spot_without_choosing_one() {
        let (_, source, parts, c, art) = prepared();
        let runtime = Runtime::load(&source, build(Node::Root, &c, &parts, &art).page).unwrap();
        let mut sim = Sim { runtime, stack: Vec::new(), acts: Vec::new(), c, parts: &parts, art: &art };
        for _ in 0..30 {
            sim.runtime.advance().unwrap();
        }
        sim.stack = vec![(Node::Root, Value::Null)];
        sim.push(Node::Body);
        let before = sim.c.draft.clone();
        sim.push(Node::Tattoo(true));
        let front = sim.row("Front", |a| matches!(a, Act::Open(Node::TattooKind(true, 2))));
        sim.choose(front);
        assert_eq!(sim.top(), Node::TattooKind(true, 2));
        sim.choose(0);
        assert_eq!(sim.top(), Node::TattooGrid(true, 2, 1));
        sim.press(Key::Right);
        assert_ne!(sim.c.draft, before, "highlighting a tattoo previews it");
        for back_on in [Node::TattooKind(true, 2), Node::Tattoo(true), Node::Body] {
            sim.press(Key::Back);
            assert_eq!(sim.top(), back_on, "B goes back");
        }
        assert_eq!(sim.c.draft, before, "no tattoo chosen");
    }

    /// The reported soft lock: into the logos, choose one, and A or B must get back to
    /// Edit Clothing. Drives the real movie (Runtime) with key presses over the built pages.
    #[test]
    #[ignore]
    fn logo_flow_never_strands_the_player() {
        let (_, source, parts, c, art) = prepared();
        let tee = build(Node::Clothes("T-shirts"), &c, &parts, &art)
            .acts
            .into_iter()
            .find_map(|a| match a {
                Act::Item(p, g) if customizable(&parts, &p) => Some((p, g)),
                _ => None,
            })
            .unwrap();
        let runtime = Runtime::load(&source, build(Node::Root, &c, &parts, &art).page).unwrap();
        let mut sim = Sim { runtime, stack: Vec::new(), acts: Vec::new(), c, parts: &parts, art: &art };
        for _ in 0..30 {
            sim.runtime.advance().unwrap();
        }
        sim.stack = vec![(Node::Merch, Value::Null), (Node::Clothes("T-shirts"), Value::Null)];
        sim.c.apply(&parts, Some(&tee.0));
        let garment = tee.1;
        sim.push(garment);
        // Edit Clothing -> Add Team Logo -> Front -> Graphics -> Brand Logos.
        let toward_logos = |a: &Act| {
            matches!(a, Act::Open(Node::GraphicLocation | Node::GraphicGrid(_, 0)) | Act::Item(_, Node::GraphicKind("front")))
        };
        let into = |sim: &mut Sim, stop: fn(Node) -> bool| {
            for _ in 0..4 {
                if stop(sim.top()) {
                    return;
                }
                let row = sim.row("toward Brand Logos", toward_logos);
                sim.choose(row);
            }
            panic!("never got there: {:?}", sim.top());
        };
        into(&mut sim, |n| matches!(n, Node::GraphicGrid(..)));
        let logo_tiles = |sim: &Sim| -> Vec<usize> {
            sim.acts.iter().enumerate().filter(|(_, a)| matches!(a, Act::Choose(Some(p)) if p["graphics"]["front"]["logo"].is_string())).map(|(i, _)| i).collect()
        };
        assert_eq!(logo_tiles(&sim).len(), sim.acts.len(), "logos only: Remove Logo is a Graphics row");
        // Choose the second logo tile.
        sim.choose(1);
        let logo = sim.c.draft["graphics"]["front"]["logo"].clone();
        assert!(logo.is_string(), "the logo is worn");
        assert_eq!(sim.top(), Node::Graphic("front"), "a logo choice lands on Graphic Adjust");
        for key in [Key::Up, Key::Down, Key::Left, Key::Right] {
            assert_eq!(sim.runtime.input(key).unwrap(), None, "Graphic Adjust's {key:?} is the owner's (sticks)");
        }
        // B there is Change Graphic: back to the grid, on the chosen logo.
        sim.press(Key::Back);
        assert_eq!(sim.top(), Node::GraphicGrid("front", 0));
        assert_eq!(sim.runtime.bindings.page.index, 1, "the grid opens on the worn logo");
        sim.press(Key::Accept);
        assert_eq!(sim.top(), Node::Graphic("front"));
        // A on the page a choice lands on accepts it and returns to Edit Clothing. It used
        // to land on the location page with Change Graphic highlighted, so A went straight
        // back into the logo menus and B did nothing there: the reported loop.
        let landed = sim.top();
        sim.press(Key::Accept);
        assert_eq!(sim.top(), garment, "A after choosing a logo (on {landed:?}) returns to Edit Clothing");
        assert_eq!(sim.c.draft["graphics"]["front"]["logo"], logo, "and keeps it");

        // A top wears one graphic (retail's shirt category): Graphic Location moves it there,
        // starting from retail's start for that location.
        sim.c.apply(&parts, Some(&json!({"graphics": {"front": {"x": 0.05}}})));
        let add = sim.row("Add Team Logo", |a| matches!(a, Act::Open(Node::GraphicLocation)));
        sim.choose(add);
        let back = sim.row("Shirt Back", |a| matches!(a, Act::Item(_, Node::GraphicKind("back"))));
        sim.choose(back);
        assert_eq!(sim.top(), Node::GraphicKind("back"));
        assert_eq!(sim.c.draft["graphics"], json!({"back": {"logo": logo}}), "moved, re-seeded, and the only one");
        // Graphics hides B and retail's B handler (0x825fc768) ignores it there: nothing moves.
        let moved = sim.c.draft.clone();
        sim.press(Key::Back);
        assert_eq!((sim.top(), &sim.c.draft), (Node::GraphicKind("back"), &moved), "B is ignored on Graphics");
        // Its way out: a logo, then Graphic Adjust's A.
        into(&mut sim, |n| matches!(n, Node::GraphicGrid(..)));
        sim.choose(1);
        assert_eq!(sim.top(), Node::Graphic("back"));
        sim.press(Key::Accept);
        assert_eq!(sim.top(), garment);
        // Retail ignores B on Edit Clothing; with item 31's colour row it shows B, which leaves as A does.
        let b_shown = sim.runtime.bindings.page.back != "#";
        sim.press(Key::Back);
        if b_shown {
            assert_eq!(sim.top(), Node::Merch, "B on Edit Clothing with a colour row goes back to the shop");
            sim.stack.insert(1, (Node::Clothes("T-shirts"), Value::Null));
            sim.push(garment);
        } else {
            assert_eq!(sim.top(), garment, "B is ignored on Edit Clothing");
        }

        // B on the grid (B shown) drops its preview and goes back; on Graphics it is ignored.
        into(&mut sim, |n| matches!(n, Node::GraphicGrid(..)));
        let entered = sim.c.draft.clone();
        sim.press(Key::Right);
        assert_ne!(sim.c.draft, entered, "highlighting a logo previews it");
        for stays_on in [Node::GraphicKind("front"), Node::GraphicKind("front")] {
            sim.press(Key::Back);
            assert_eq!(sim.top(), stays_on);
        }
        assert_eq!(sim.c.draft, entered, "B on the grid put the worn logo back");

        // Graphics: retail rows, the item's Custom Graphics, then Remove Logo.
        into(&mut sim, |n| matches!(n, Node::GraphicKind(_)));
        let built = build(sim.top(), &sim.c, &parts, &art);
        let texts: Vec<&str> = built.page.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["ID_CAC_BRANDGRAPHICS_TITLE", "ID_CAC_TEAMGRAPHICS_TITLE", "#Custom Graphics", "ID_CAC_REMOVE_GRAPHIC"]);
        assert_eq!((built.page.title.as_str(), built.page.accept.as_str(), built.page.back.as_str()), ("ID_CAC_GRAPHICS_TITLE", "ID_COMMON_SELECT", "ID_COMMON_BACK"));
        sim.choose(3);
        assert_eq!(sim.top(), garment, "Remove Logo returns to Edit Clothing");
        assert!(sim.c.draft["graphics"]["front"].is_null(), "and the logo is off");
    }

    /// Graphic Adjust at retail's per-frame steps, storing what retail's fit leaves: held past
    /// the zone edge it stops there, and the reverse answers at once.
    #[test]
    #[ignore]
    fn graphic_adjust_steps_like_retail_and_stores_what_is_shown() {
        use crate::custom_graphic::{self as graphic, Adjust};
        let (_, _, parts, mut c, art) = prepared();
        let tee = build(Node::Clothes("T-shirts"), &c, &parts, &art)
            .acts
            .into_iter()
            .find_map(|a| match a {
                Act::Item(p, _) if customizable(&parts, &p) => Some(p),
                _ => None,
            })
            .unwrap();
        c.apply(&parts, Some(&tee));
        let Some(Act::Choose(Some(logo))) = build(Node::GraphicGrid("front", 0), &c, &parts, &art).acts.first().cloned() else { panic!("a logo tile") };
        c.apply(&parts, Some(&logo));
        let rect = c.draft["selections"].as_object().unwrap().values().flat_map(|w| graphic_locations(&parts, w)).find(|l| l.0 == "front").unwrap().1;
        let bounds = parts.library.graphics[logo["graphics"]["front"]["logo"].as_str().unwrap()].bounds;
        let shown = |a: Adjust| graphic::fit(graphic::entry("front"), false, rect, bounds, a);
        let keys = bevy::prelude::ButtonInput::<bevy::prelude::KeyCode>::default();
        // Held for some retail frames (30 a second).
        let mut hold = |pad: (u16, i16, [i16; 2]), frames: usize| {
            for _ in 0..frames {
                crate::create_skater::adjust_graphic(Node::Graphic("front"), Some(pad), &keys, 0, 1., &mut c, &parts);
            }
            Adjust::of(&c.draft["graphics"]["front"])
        };
        let near = |a: f32, b: f32| (a - b).abs() < 0.002;
        // Right stick: 0.005 UV a frame (Adjust x is the UV offset from the start).
        let moved = hold((0, 0, [32767, 0]), 6);
        assert!(near(moved.x, 0.03), "{moved:?}");
        // LB: 0.01 rad a frame.
        let turned = hold((0x100, 0, [0, 0]), 30);
        assert!(near(turned.angle, 0.3f32.to_degrees()), "{turned:?}");
        // Held past the edge (and grown past the fit): what is stored is what shows.
        let edge = hold((0, 0, [32767, 0]), 300);
        assert!(near(shown(edge).x, edge.x) && edge.x < 0.5, "{edge:?}");
        let back = hold((0, 0, [-32767, 0]), 1);
        assert!(near(back.x, edge.x - 0.005), "the reverse answers at once: {edge:?} -> {back:?}");
        let big = hold((0, 32767, [0, 0]), 300);
        assert!(near(shown(big).size, big.size) && big.size < 8., "{big:?}");
        // Shrinking stops at retail's s = 15 (the front starts at 5).
        let small = hold((0, -32767, [0, 0]), 600);
        assert!(near(small.size, 5. / 15.), "{small:?}");
        // One upper tattoo: the worn one keeps its adjust on its spot, even after the grid previewed
        // another; a different tattoo or spot starts from retail's start there.
        let tattoo = |c: &Customiser, side: u8, tile: usize| match build(Node::TattooGrid(true, side, 1), c, &parts, &art).acts.get(tile) {
            Some(Act::Choose(Some(p))) => p.clone(),
            other => panic!("a tattoo tile: {other:?}"),
        };
        c.apply(&parts, Some(&tattoo(&c, 2, 1)));
        c.apply(&parts, Some(&json!({"tattoos": {"Arm": {"x": 0.1}}})));
        let (worn, other) = (tattoo(&c, 2, 1), tattoo(&c, 2, 2));
        c.apply(&parts, Some(&other));
        assert!(c.draft["tattoos"]["Arm"]["x"].is_null(), "a different tattoo re-seeds");
        c.apply(&parts, Some(&worn));
        assert_eq!(c.draft["tattoos"]["Arm"]["x"], json!(0.1), "the worn one, on its spot, keeps its adjust");
        c.apply(&parts, Some(&tattoo(&c, 3, 1)));
        assert!(c.draft["tattoos"]["Arm"]["x"].is_null(), "a new spot re-seeds");
    }

    /// Every recorded page through the original screen (needs the prepared set;
    /// CREATE_SKATER_OUT for PNGs).
    #[test]
    #[ignore]
    fn every_recorded_page_builds_and_presents() {
        let (manifest, source, parts, mut c, art) = prepared();
        let root = std::path::Path::new(&manifest).parent().unwrap().parent().unwrap().to_path_buf();
        let out = std::env::var("CREATE_SKATER_OUT").ok();
        let mut runtime = Runtime::load(&source, build(Node::Root, &c, &parts, &art).page).unwrap();
        for _ in 0..30 {
            runtime.advance().unwrap();
        }
        let first_item = |node: Node| build(node, &c, &parts, &art).acts.iter().find_map(|a| match a { Act::Item(p, g) => Some((p.clone(), *g)), _ => None }).unwrap();
        let items = [first_item(Node::Clothes("Hats")), first_item(Node::Board("Deck"))];
        let pages = [items[0].1, items[1].1,
            Node::Root, Node::Body, Node::Skin, Node::Hair, Node::Shape, Node::Presets, Node::Face, Node::Eyes, Node::Brow,
            Node::Nose, Node::Jaw, Node::Mouth, Node::Chin, Node::FacialHair, Node::Tattoo(true), Node::TattooKind(true, 2),
            Node::TattooGrid(true, 2, 1), Node::Tattoo(false), Node::Merch, Node::Clothes("Hats"), Node::Boards,
            Node::Board("Deck"), Node::Board("Wheels"), Node::Grip,  Node::Style, Node::TattooAdjust(true),
            Node::Graphic("front"), Node::GraphicKind("front"), Node::GraphicGrid("front", 0), Node::GraphicGrid("deck", 1),
        ];
        for node in pages {
            // As in game, choosing an item wears it before its Edit Clothing page opens.
            if let Some((patch, _)) = items.iter().find(|(_, g)| *g == node) {
                c.apply(&parts, Some(patch));
            }
            let built = build(node, &c, &parts, &art);
            assert!(!built.page.rows.is_empty() || node.is_adjust(), "{node:?} has rows");
            assert_eq!(built.page.rows.len(), built.acts.len());
            runtime.show(built.page).unwrap();
            for _ in 0..40 {
                runtime.advance().unwrap();
            }
            if let Some(out) = &out {
                let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(out).join(format!("cas-{}.png", format!("{node:?}").replace('"', ""))));
            }
            runtime.input(Key::Down).unwrap();
        }
        // Items per design; wear styles are options, not items (fact-checked
        // against the retail captures: hoodies hood up/open, caps 5 options,
        // accessories without an Edit Clothing page).
        assert!(art.family_count() > 20);
        for label in ["Hats", "T-shirts", "Shirts", "Hoodies", "Jackets", "Sweaters", "Pants & shorts", "Shoes", "Socks", "Glasses", "Necklaces", "Wristwear", "Rings", "Belts", "Chains", "Backpacks", "Other", "Gloves"] {
            let built = build(Node::Clothes(label), &c, &parts, &art);
            let items = built.acts.iter().filter(|a| matches!(a, Act::Item(..))).count();
            let plain = built.acts.iter().filter(|a| matches!(a, Act::Choose(_))).count();
            let options: Vec<usize> = built.acts.iter().filter_map(|a| match a {
                Act::Item(_, g) => Some(build(*g, &c, &parts, &art).page.rows.iter().filter(|r| r.kind == "selector").count()),
                _ => None,
            }).collect();
            eprintln!("{label}: {} tiles, {items} with Edit Clothing, {plain} direct, option rows {:?}", built.page.rows.len(), &options[..options.len().min(12)]);
            if ["Glasses", "Necklaces", "Wristwear", "Rings", "Belts", "Chains", "Backpacks", "Other", "Gloves"].contains(&label) {
                assert_eq!(items, 0, "{label} have no options");
            }
        }
        // Accessories: retail's four rows first (page 2: rows 13, 14, 43, 44), then the ported tabs.
        let accessories: Vec<String> = build(Node::Accessories, &c, &parts, &art).page.rows.into_iter().map(|r| r.text).collect();
        assert_eq!(accessories[..4], ["ID_CAC_GLASSES_TITLE", "ID_CAC_WRIST_TITLE", "ID_CAC_NECKLACES_TITLE", "ID_CAC_RINGS_TITLE"]);
        eprintln!("accessories: {accessories:?}");
        // Each accessory tab lists only its own slot's kind: no belt or backpack under Rings, the wallet chain under
        // Chains (user 10-30: worn together).
        for (tab, slot) in [("Rings", "Accessory"), ("Belts", "Belt"), ("Chains", "Chain"), ("Backpacks", "Backpack"), ("Other", "AccessoryOther"), ("Gloves", "Gloves")] {
            let entry = c.entry(&grid_path(tab, false)).unwrap();
            for e in c.for_gender(entry) {
                let Some(model) = e.options.iter().filter_map(|o| o.patch.as_ref()).find_map(model_of) else { continue };
                assert_eq!(parts.library.models[model].slot, slot, "{tab}: {}", e.label);
                assert_eq!(tab == "Chains", e.label.contains("Wallet Chain"), "{tab}: {}", e.label);
                if tab == "Rings" {
                    assert!(!e.label.contains("Belt") && !e.label.contains("Backpack") && !e.label.contains("Back Plates"), "Rings: {}", e.label);
                }
            }
        }
        // One No Top tile: the bare torso (old_skate_clothing.py leaves out Skate 2's, whose skin
        // drew invisible and was a second Remove tile, 10-11).
        let tees = build(Node::Clothes("T-shirts"), &c, &parts, &art);
        assert_eq!(tees.page.rows.iter().filter(|r| r.text == "ID_CAC_REMOVE_PART").count(), 1, "one No Top tile");
        // Customizable items lead their grids, after the remove tile.
        for node in [Node::Clothes("Hats"), Node::Clothes("T-shirts"), Node::Clothes("Hoodies"), Node::Board("Deck")] {
            let built = build(node, &c, &parts, &art);
            let first = built.page.rows.iter().zip(&built.acts).find_map(|(r, a)| match a {
                Act::Item(p, _) | Act::Choose(Some(p)) if r.text != "ID_CAC_REMOVE_PART" => Some(p),
                _ => None,
            });
            assert!(first.is_some_and(|p| customizable(&parts, p)), "{node:?} opens on a customizable item");
        }
        // Installed DLC parts (cas.DlcID) are tiles with the DLC badge frame.
        if parts.library.materials.values().any(|m| !m.flag("DlcID").is_empty()) {
            let dlc: usize = ["Hats", "T-shirts", "Glasses", "Wristwear"].iter()
                .map(|l| build(Node::Clothes(l), &c, &parts, &art).page.rows.iter().filter(|r| r.kind == "dlc" && r.panel == "dlc").count())
                .sum();
            eprintln!("DLC tiles: {dlc}");
            assert!(dlc > 0, "DLC parts are listed with the DLC badge");
        }
        // Graphic Locations: stock logo grids (retail's 3x5 mThumbs2), and Graphic Adjust is
        // the retail stick legend.
        let brands = build(Node::GraphicGrid("front", 0), &c, &parts, &art);
        let teams = build(Node::GraphicGrid("front", 1), &c, &parts, &art);
        eprintln!("graphics: {} brand tiles, {} team tiles", brands.page.rows.len(), teams.page.rows.len());
        assert!(brands.page.rows.len() > 100 && teams.page.rows.len() > 40, "stock logos are listed");
        assert_eq!(brands.page.mode, Mode::ThumbsCustom);
        let Some(Act::Choose(Some(logo))) = brands.acts.first().cloned() else { panic!("a logo tile") };
        c.apply(&parts, Some(&logo));
        let adjust = build(Node::Graphic("front"), &c, &parts, &art).page;
        assert!(adjust.rows.is_empty() && adjust.mode == Mode::Scale, "Graphic Adjust: no rows, mScalemenu");
        assert_eq!((adjust.title.as_str(), adjust.accept.as_str(), adjust.back.as_str()), ("ID_CAC_GRAPHIC_ADJUST", "ID_COMMON_ACCEPT", "ID_CAC_CHANGE_GRAPHIC"));
        let hoodies = build(Node::Clothes("Hoodies"), &c, &parts, &art);
        let with_styles = hoodies.acts.iter().find_map(|a| match a { Act::Item(_, g) => Some(*g), _ => None }).unwrap();
        let page = build(with_styles, &c, &parts, &art);
        let option = page.page.rows.iter().zip(&page.acts).find_map(|(r, a)| match a { Act::Cycle(v) if r.kind == "selector" => Some(v.clone()), _ => None }).unwrap();
        let designs: std::collections::HashSet<_> = option.iter().map(|p| {
            let slot = slot_of(p).unwrap();
            let asset = p["selections"][slot]["asset_id"].as_str().unwrap();
            let material = &parts.library.materials[p["selections"][slot]["material_id"].as_str().unwrap()];
            // The design is its retail texture (StyleSubcategory); names differ between a
            // model's styles (the Customizable Hoodie's blank is "Genericl" and "Original").
            (asset.to_owned(), material.flag("StyleSubcategory").to_owned(), material.name.clone())
        }).collect();
        eprintln!("hoodie options {designs:?}");
        let textures: std::collections::HashSet<_> = designs.iter().map(|d| &d.1).collect();
        assert_eq!(textures.len(), 1, "switching options keeps the design");
        assert!(option.len() >= 2 && designs.iter().map(|d| &d.0).collect::<std::collections::HashSet<_>>().len() == option.len(), "options switch models");
        // Completeness: every design of every listed model is a tile (or lacks retail art).
        for label in ["T-shirts", "Shirts", "Hoodies", "Jackets", "Sweaters", "Pants & shorts"] {
            let entry = c.entry(&grid_path(label, false)).unwrap();
            let mut total = 0;
            let mut no_art = Vec::new();
            for e in c.for_gender(entry) {
                let Some(model) = e.options.iter().filter_map(|o| o.patch.as_ref()).find_map(model_of) else { continue };
                if art.is_alternate(model) { continue; }
                let slot = e.options.iter().filter_map(|o| o.patch.as_ref()).find_map(slot_of).unwrap().to_owned();
                for (d, m) in parts.library.models[model].materials.iter().enumerate() {
                    total += 1;
                    let patch = wear(&parts, &slot, &[model.to_owned()], d, 0).unwrap();
                    if art.part(&patch, "male").is_none() { no_art.push(parts.library.materials[m].name.clone()); }
                }
            }
            eprintln!("{label}: {total} designs, {} without thumbnail {:?}", no_art.len(), &no_art[..no_art.len().min(15)]);
        }
        let mut mismatched = Vec::new();
        for label in ["Hats", "T-shirts", "Shirts", "Hoodies", "Jackets", "Sweaters", "Pants & shorts", "Socks"] {
            for a in build(Node::Clothes(label), &c, &parts, &art).acts {
                let Act::Item(_, g) = a else { continue };
                let page = build(g, &c, &parts, &art);
                for (row, act) in page.page.rows.iter().zip(&page.acts) {
                    let Act::Cycle(options) = act else { continue };
                    if row.kind != "selector" {
                        continue;
                    }
                    let names: Vec<String> = options.iter().map(|p| {
                        let slot = slot_of(p).unwrap();
                        parts.library.materials[p["selections"][slot]["material_id"].as_str().unwrap()].name.clone()
                    }).collect();
                    let first = |n: &String| n.split_whitespace().next().unwrap_or("").to_lowercase();
                    if names.iter().any(|n| first(n) != first(&names[0])) {
                        mismatched.push(format!("{label}: {names:?}"));
                    }
                }
            }
        }
        eprintln!("mismatched designs across wear styles: {}
{}", mismatched.len(), mismatched.join("
"));
        // Stepping a slider and a swatch changes the profile.
        let before = c.draft.clone();
        let eyes = build(Node::Eyes, &c, &parts, &art);
        assert!(step(&mut c, &parts, &eyes.acts[1], 1));
        assert!(step(&mut c, &parts, &eyes.acts[0], 1));
        assert_ne!(c.draft, before);
    }

    /// The retail Customizable Tee, Hoodie, Windbreaker, Hat and Board (cas_db models named
    /// "Customizable ..." in the retail string table), each gender: first in its grid, its
    /// Graphic Locations behind Add Team Logo, Graphics (logos, Graphic Creator) on each, a stock
    /// logo and a creator design worn there, kept through a save and reload. Prints each
    /// location's stamp (STAMP model material zone rows rectangle) for the offline renders.
    #[test]
    #[ignore]
    fn customizable_items_lead_and_take_graphics() {
        use crate::custom_graphic::{self as graphic, Doc, Layer};
        let library = std::env::var("SKATE_CAC_TEST_LIBRARY").unwrap();
        let manifest = std::env::var("CREATE_SKATER_MANIFEST").unwrap();
        let source: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        let lib: Library = serde_json::from_slice(&std::fs::read(&library).unwrap()).unwrap();
        let extras: Vec<crate::customiser::Entry> =
            serde_json::from_slice(&std::fs::read(std::path::Path::new(&library).with_file_name("extra-menu.json")).unwrap()).unwrap();
        let assets = std::path::Path::new(&manifest).ancestors().nth(4).unwrap().to_path_buf();
        let art = Art::load(&std::path::Path::new(&library).with_file_name("catalog.json"), &lib, &source, &assets).unwrap();
        // Bug 10-25 #22: the Customizable Longsleeve Tee's second style is its rolled-up sleeves (male, female).
        assert_eq!(art.wear_styles("a68f7c7e30bfd4ed"), ["a68f7c7e30bfd4ed", "a68f457e30bfe89c"]);
        assert_eq!(art.wear_styles("a37b8e2e92feebb4"), ["a37b8e2e92feebb4", "a37b892e92fee262"]);
        let parts = Parts::for_test(lib);
        let design = Doc { layers: vec![Layer::new(graphic::Art::Shape(2), [255, 0, 0, 255])], ..Default::default() }.encode();
        const SHIRT: &[&str] = &["front", "back", "left", "right"];
        // Grid, board, the Customizable models (male, female; none = not sold), its locations.
        // T-shirts: the Customizable Tee or Longsleeve Tee (both lead, in the grid's order).
        let items: [(&str, bool, [&[&str]; 2], &[&str]); 5] = [
            ("Hats", false, [&["dcbfd26f04c7744e"], &["c4995bcd26de49b9"]], &["hat"]),
            ("T-shirts", false, [&["801095192c838106", "a68f7c7e30bfd4ed"], &["862d97f973760883", "a37b8e2e92feebb4"]], SHIRT),
            ("Hoodies", false, [&["c620cb8d10a788b4"], &["c53f0b3dc2ebef07"]], SHIRT),
            ("Jackets", false, [&["c620cb8d10a788ec"], &[]], SHIRT),
            ("Deck", true, [&["a841a65d5c4dd593"]; 2], &["deck"]),
        ];
        for (g, gender) in ["male", "female"].into_iter().enumerate() {
            let mut c = Customiser::for_test(&parts, extras.clone(), gender);
            for (label, board, models, zones) in items {
                if models[g].is_empty() {
                    continue;
                }
                // Each top starts with clean sides: a side keeping the last top's design takes no logo (GraphicLocation).
                if zones == SHIRT && let Some(sides) = c.draft["graphics"].as_object_mut() {
                    sides.retain(|zone, _| !SHIRT.contains(&zone.as_str()));
                }
                let grid = if board { Node::Board(label) } else { Node::Clothes(label) };
                let built = build(grid, &c, &parts, &art);
                let (name, patch, act) = built.page.rows.iter().zip(&built.acts).find_map(|(r, a)| match a {
                    Act::Item(p, _) | Act::Choose(Some(p)) if r.text != "ID_CAC_REMOVE_PART" => Some((r.text.clone(), p.clone(), a.clone())),
                    _ => None,
                }).unwrap();
                let slot = slot_of(&patch).unwrap().to_owned();
                eprintln!("{gender} {label}: first tile {name}");
                let model = patch["selections"][&slot]["asset_id"].as_str().unwrap().to_owned();
                assert!(models[g].contains(&model.as_str()), "{gender} {label}: the Customizable item is the first tile, not {model}");
                assert!(name.starts_with("#Customizable"), "{gender} {label}: {name}");
                let Act::Item(_, garment) = act else { panic!("{gender} {label}: no Edit Clothing page") };
                c.apply(&parts, Some(&patch));
                let page = build(garment, &c, &parts, &art);
                // Add Team Logo: a top's Graphic Location list, the hat and deck's Graphics.
                let add = page.page.rows.iter().zip(&page.acts).find_map(|(r, a)| match a {
                    Act::Open(n) if r.text == "ID_CAC_ADD_GRAPHIC" => Some(*n),
                    _ => None,
                });
                // A top's location: its patch, as the Graphic Location page builds it now.
                let location = |c: &Customiser, zone: &str| {
                    build(Node::GraphicLocation, c, &parts, &art).acts.into_iter().find_map(|a| match a {
                        Act::Item(p, Node::GraphicKind(z)) if z == zone => Some(p),
                        _ => None,
                    })
                };
                let locations: Vec<&str> = match add {
                    Some(Node::GraphicKind(z)) => vec![z],
                    Some(Node::GraphicLocation) => build(Node::GraphicLocation, &c, &parts, &art)
                        .acts
                        .iter()
                        .filter_map(|a| match a { Act::Item(_, Node::GraphicKind(z)) => Some(*z), _ => None })
                        .collect(),
                    other => panic!("{gender} {label}: Add Team Logo opens {other:?}"),
                };
                assert_eq!(locations, zones, "{gender} {label}: Graphic Locations");
                for (done, &zone) in locations.iter().enumerate() {
                    // The top's one stock logo (the last location's) moves here, re-seeded; the designs stay on their sides.
                    if let Some(moved) = location(&c, zone) {
                        c.apply(&parts, Some(&moved));
                        let logos: Vec<&str> = SHIRT.iter().copied().filter(|z| c.draft["graphics"][*z]["logo"].is_string()).collect();
                        assert!(logos.is_empty() || logos == [zone], "{zone}: {logos:?}");
                        for &earlier in &locations[..done] {
                            assert_eq!(c.draft["graphics"][earlier]["design"], json!(design), "{zone}: the design on {earlier} stays");
                        }
                    }
                    let at = build(Node::GraphicKind(zone), &c, &parts, &art);
                    assert!(matches!(at.acts[0], Act::Open(Node::GraphicGrid(z, 0)) if z == zone));
                    assert!(at.acts.iter().any(|a| matches!(a, Act::Open(Node::GraphicGrid(z, CUSTOM)) if *z == zone)), "{zone}: Custom Graphics");
                    assert_eq!(at.page.rows.last().unwrap().text, "ID_CAC_REMOVE_GRAPHIC");
                    let brands = build(Node::GraphicGrid(zone, 0), &c, &parts, &art);
                    let Some(Act::Choose(Some(logo))) = brands.acts.get(1).cloned() else { panic!("{zone}: a logo tile") };
                    c.apply(&parts, Some(&logo));
                    assert!(c.draft["graphics"][zone]["logo"].is_string(), "{zone}: logo worn");
                    // A design merged into a side by Customiser::apply keeps the side's logo (the Graphic Creator's
                    // Done takes it off itself: graphic_creator's a_design_takes_the_stock_logo_off_its_side_live_and_on_done).
                    c.apply(&parts, Some(&json!({"graphics":{zone:{"design":design}}})));
                    assert!(c.draft["graphics"][zone]["logo"].is_string(), "{zone}: the logo stays over the design");
                    assert_eq!(c.draft["graphics"][zone]["design"], json!(design), "{zone}: and the design is under it");
                }
                // Every wear style (hood up, open, cap turned, worn deck) keeps the graphics and the locations.
                let worn = c.draft["graphics"].clone();
                let styles = page.page.rows.iter().zip(&page.acts).find_map(|(r, a)| match a { Act::Cycle(v) if r.kind == "selector" => Some(v.clone()), _ => None });
                for style in styles.unwrap_or_default() {
                    c.apply(&parts, Some(&style));
                    assert_eq!(c.draft["graphics"], worn, "{gender} {label}: {style}");
                    assert_eq!(graphic_zones(&parts, &c.draft["selections"][&slot]), zones, "{gender} {label}: {style}");
                }
                c.apply(&parts, Some(&patch));
                for &id in models[g] {
                    let part = &parts.library.models[id];
                    for (zone, rect) in graphic::locations(id, part, true) {
                        let [a, b] = graphic::stamp(graphic::entry(zone), gender == "female", graphic::Adjust::default());
                        eprintln!("STAMP {id} {} {zone} {} {} {} {} {} {} {rect:?}", part.materials[0], a.x, a.y, a.z, b.x, b.y, b.z);
                    }
                }
            }
            // Retail's colour-only Customizable Pants (female) lead their grid too.
            if gender == "female" {
                let pants = build(Node::Clothes("Pants & shorts"), &c, &parts, &art);
                assert_eq!(pants.page.rows.iter().find(|r| r.text != "ID_CAC_REMOVE_PART").unwrap().text, "#Customizable Pants");
            }
            // Saved to disk and loaded back (both through resolve), every graphic stays.
            let saved = serde_json::to_vec_pretty(&c.draft).unwrap();
            let loaded = parts.resolve(&serde_json::from_slice(&saved).unwrap()).unwrap();
            assert_eq!(loaded["graphics"], c.draft["graphics"]);
            let kept: Vec<&String> = loaded["graphics"].as_object().unwrap().keys().collect();
            assert_eq!(kept.len(), 6, "{gender}: the top's four sides, the hat's, the deck's: {kept:?}");
            // A profile from an older build, a logo on each of the top's locations (saved, or a
            // peer's), loads with one: the first in retail's location order.
            let mut old = c.draft.clone();
            let logo = parts.library.graphics.keys().next().unwrap();
            for zone in SHIRT {
                old["graphics"][zone] = json!({"logo": logo, "x": 0.01});
            }
            let loaded = parts.resolve(&old).unwrap();
            assert_eq!(SHIRT.iter().filter(|z| loaded["graphics"].get(**z).is_some()).count(), 1, "{gender}");
            assert_eq!(loaded["graphics"]["front"], json!({"logo": logo, "x": 0.01}), "{gender}");
        }
    }

    /// The tees the old My Graphics move onto are the installed library's real ones: present, named
    /// "Tee", with a front location, and of the gender OLD_TEES says.
    #[test]
    #[ignore]
    fn the_old_my_graphics_tees_are_the_installed_librarys_real_ones() {
        let (_, _, parts, _, _) = prepared();
        for (id, female) in OLD_TEES {
            let model = parts.library.models.get(id).unwrap_or_else(|| panic!("{id} is in the library"));
            assert_eq!(model.name, "Tee", "{id}");
            assert_eq!(model.flag("Gender"), if female { "female" } else { "male" }, "{id}");
            assert_eq!(model.slot, "OuterTorso", "{id}");
            let front = crate::custom_graphic::locations(id, model, true).into_iter().find(|l| l.0 == "front");
            assert!(front.is_some(), "{id} has a front to draw on");
        }
        assert!(parts.library.models.values().all(|m| m.name != "Custom Tee"), "no model is named Custom Tee");
    }

    /// tools/pipeline/dlc/rainbow_deck.py's deck: in the Deck grid as a free, unlocked "owned" tile
    /// with its own thumbnail and texture. Decks are listed by name, so it follows "Europe".
    #[test]
    #[ignore]
    fn the_rainbow_deck_is_listed_free_with_its_tile() {
        const MID: &str = "ffff000000000001";
        let (manifest, source, parts, c, art) = prepared();
        let built = build(Node::Board("Deck"), &c, &parts, &art);
        let names: Vec<&str> = built.page.rows.iter().map(|r| r.text.trim_start_matches('#')).collect();
        let at = names.iter().position(|n| *n == "Everyone Is Welcome Here").expect("the rainbow deck is in the Deck grid");
        assert_eq!(names[at - 1], "Europe", "listed by name: {:?}", &names[at.saturating_sub(3)..(at + 3).min(names.len())]);
        let row = &built.page.rows[at];
        assert_eq!(row.kind, "owned", "no DLC frame");
        let (Act::Choose(Some(patch)) | Act::Item(patch, _)) = &built.acts[at] else { panic!("the tile selects nothing") };
        assert_eq!(patch["selections"]["SkateBoard"]["material_id"], MID);
        let library: Value = serde_json::from_slice(&std::fs::read(std::env::var("SKATE_CAC_TEST_LIBRARY").unwrap()).unwrap()).unwrap();
        let material = &library["materials"][MID];
        assert_eq!(material["flags"]["Cost"], "0");
        assert_eq!(material["flags"]["ManufacturerID"], "skate");
        for locked in ["Unlock", "SponsorUnlock", "DlcID", "AlternateMaterialName"] {
            assert!(material["flags"].get(locked).is_none(), "{locked}");
        }
        let assets = std::path::Path::new(&manifest).ancestors().nth(4).unwrap();
        assert!(assets.join(material["diffuse"].as_str().unwrap()).is_file(), "texture");
        let tile = source["thumbnails"][&row.thumbnail]["rgba"].as_str().expect("a thumbnail entry");
        let tile = std::path::Path::new(&manifest).ancestors().nth(2).unwrap().join(tile);
        assert_eq!(std::fs::metadata(&tile).unwrap().len(), 128 * 128 * 4, "{}", tile.display());
    }
}

/// The Custom Graphics tab on a small made-up library (no install needed).
#[cfg(test)]
mod custom_graphics_tests {
    use super::*;
    use crate::custom_graphic::{Art as Shapes, Doc, Layer};
    use std::path::PathBuf;

    /// The installed library's two customizable tees, by their real ids and name (no "Custom Tee" exists).
    const TEE: &str = "801095192c838106";
    const WOMANS_TEE: &str = "862d97f973760883";
    fn parts() -> Parts {
        let mut parts = Parts::default();
        let lib = &mut parts.library;
        let flags = |gender: &str, q: [&str; 4]| -> std::collections::HashMap<String, String> {
            [("TopType", "tshirt"), ("Gender", gender), ("StampUVConstraintQ1", q[0]), ("StampUVConstraintQ2", q[1]), ("StampUVConstraintQ3", q[2]), ("StampUVConstraintQ4", q[3])]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect()
        };
        let tee = |gender, q| crate::customiser_parts::Part { slot: "OuterTorso".into(), name: "Tee".into(), flags: flags(gender, q), materials: vec!["white".into()], scene: String::new() };
        lib.models.insert(TEE.into(), tee("male", ["0.98,0.48,0.13,0.37", "0.98,0.48,0.64,0.86", "0.48,0.25,0.05,0.45", "0.48,0.25,0.55,0.95"]));
        lib.models.insert(WOMANS_TEE.into(), tee("female", ["0.94,0.48,0.08,0.42", "0.97,0.48,0.58,0.92", "0.47,0,0.01,0.47", "0.47,0,0.52,0.98"]));
        lib.models.insert("head".into(), crate::customiser_parts::Part { slot: "Rostral".into(), name: "Head".into(), flags: Default::default(), materials: vec!["white".into()], scene: String::new() });
        lib.materials.insert(
            "white".into(),
            serde_json::from_value(json!({"name": "White", "flags": {"IsLogoable": "true"}, "diffuse": "x.png", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1})).unwrap(),
        );
        lib.defaults.insert(
            "male".into(),
            json!({"gender": "male", "selections": {"OuterTorso": {"asset_id": TEE, "material_id": "white"}, "Rostral": {"asset_id": "head", "material_id": "white"}}}),
        );
        lib.defaults.insert(
            "female".into(),
            json!({"gender": "female", "selections": {"OuterTorso": {"asset_id": WOMANS_TEE, "material_id": "white"}, "Rostral": {"asset_id": "head", "material_id": "white"}}}),
        );
        parts
    }
    fn profile(name: &str) -> PathBuf {
        // ponytail: one folder per test, wiped and reused (a pid suffix left one per run). Two test
        // runs at once would share it.
        let dir = std::env::temp_dir().join(format!("skate-cas-custom-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("profile.json")
    }
    fn design(n: u8) -> String {
        Doc { layers: vec![Layer { x: i16::from(n) * 10, ..Layer::new(Shapes::Shape(n), [200, 10, 10, 255]) }], ..Default::default() }.encode()
    }
    fn saved(sides: &[(&str, String)]) -> Saved {
        Saved { sides: sides.iter().map(|(z, d)| ((*z).to_owned(), d.clone())).collect() }
    }

    /// Bug 10-25 #20: the truck and wheel sliders run down to no bars, as retail's ten-bar slider does (and back up).
    #[test]
    fn truck_and_wheel_sliders_reach_zero() {
        let parts = parts();
        let mut c = Customiser::for_test(&parts, Vec::new(), "male");
        for (key, row) in [("truck", 3), ("wheel", 4)] {
            let act = build(Node::Root, &c, &parts, &Art::default()).acts[row].clone();
            assert!(matches!(&act, Act::Slider { key: k, .. } if k == key));
            for _ in 0..12 {
                assert!(step(&mut c, &parts, &act, -1));
            }
            assert_eq!((scalar_of(&c.draft, key, 0.7), c.status.as_str()), (0., ""), "{key}");
            assert_eq!(build(Node::Root, &c, &parts, &Art::default()).page.rows[row].value, 0., "{key}: no bars");
            step(&mut c, &parts, &act, 1);
            assert_eq!(build(Node::Root, &c, &parts, &Art::default()).page.rows[row].value, 1., "{key}: one bar");
        }
    }

    /// The Graphic Creator has one door: each location's Custom Graphics (New, or a save's Edit).
    #[test]
    fn one_way_in_through_custom_graphics() {
        let (parts, path) = (parts(), profile("oneway"));
        let mut c = Customiser::for_test(&parts, Vec::new(), "male");
        c.set_profile_path(path);
        let art = Art::default();
        for zone in crate::custom_graphic::SHIRT {
            let kinds = build(Node::GraphicKind(zone), &c, &parts, &art);
            assert!(kinds.page.rows.iter().all(|r| !r.text.contains("Graphic Creator")), "{zone}");
            let custom = kinds.acts.iter().position(|a| matches!(a, Act::Open(Node::GraphicGrid(z, CUSTOM)) if *z == zone)).expect("Custom Graphics");
            assert_eq!(kinds.page.rows[custom].text, "#Custom Graphics");
            let grid = build(Node::GraphicGrid(zone, CUSTOM), &c, &parts, &art);
            assert!(matches!(grid.acts[0], Act::CreatorNew(z) if z == zone), "{zone}: New");
        }
    }

    #[test]
    fn the_custom_graphics_tab_lists_new_then_the_saves_wears_them_and_deletes_with_their_tiles() {
        let (parts, path) = (parts(), profile("tab"));
        let mut c = Customiser::for_test(&parts, Vec::new(), "male");
        c.set_profile_path(path.clone());
        let original = c.draft.clone();
        let art = Art::default();

        // The location's page offers Custom Graphics beside the retail logo tabs.
        let kinds = build(Node::GraphicKind("front"), &c, &parts, &art);
        let texts: Vec<&str> = kinds.page.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["ID_CAC_BRANDGRAPHICS_TITLE", "ID_CAC_TEAMGRAPHICS_TITLE", "#Custom Graphics", "ID_CAC_REMOVE_GRAPHIC"]);
        assert!(matches!(kinds.acts[2], Act::Open(Node::GraphicGrid("front", CUSTOM))));

        // Nothing saved yet: just New.
        let grid = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art);
        assert_eq!(grid.page.rows.len(), 1);
        assert!(matches!(grid.acts[0], Act::CreatorNew("front")) && grid.page.rows[0].text == "#New Graphic");

        // Two saves of the tee: the first on the front and back, the second on the front and a sleeve.
        let (one, two) = (saved(&[("front", design(1)), ("back", design(2))]), saved(&[("front", design(3)), ("left", design(4))]));
        let mut store = Store::default();
        store.add(TEE, one.clone()).unwrap();
        store.add(TEE, two.clone()).unwrap();
        store.save(&path).unwrap();
        let grid = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art);
        let names: Vec<&str> = grid.page.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(names, ["#New Graphic", "#Custom Graphic 1", "#Custom Graphic 2"]);
        assert!(grid.page.rows[1].thumbnail.is_empty(), "no tile drawn yet");
        let Act::Item(patch, Node::SavedGraphic("front", 0)) = grid.acts[1].clone() else { panic!("{:?}", grid.acts[1]) };

        // Highlighting wears the whole item as saved; the next one clears what it does not name.
        c.apply(&parts, Some(&patch));
        assert_eq!(c.draft["graphics"]["front"]["design"], json!(design(1)));
        assert_eq!(c.draft["graphics"]["back"]["design"], json!(design(2)));
        assert_eq!(build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art).page.index, 1, "the worn save is highlighted");
        let Act::Item(patch, _) = grid.acts[2].clone() else { panic!() };
        c.apply(&parts, Some(&patch));
        assert_eq!(c.draft["graphics"]["front"]["design"], json!(design(3)));
        assert!(c.draft["graphics"]["back"]["design"].is_null(), "the second save has no back design");
        assert_eq!(c.draft["graphics"]["left"]["design"], json!(design(4)));
        assert_eq!(build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art).page.index, 2);

        // The tiles the page wants, and the ones it shows once drawn; stray tiles are cleared away.
        let jobs = custom_jobs(&c, &parts, "front");
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.item == TEE && !j.path.exists()));
        std::fs::create_dir_all(jobs[0].path.parent().unwrap()).unwrap();
        std::fs::write(&jobs[0].path, b"png").unwrap();
        let stray = jobs[0].path.with_file_name("0000000000000000.png");
        std::fs::write(&stray, b"old").unwrap();
        let grid = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art);
        assert_eq!(grid.page.rows[1].thumbnail, jobs[0].path.to_string_lossy());
        assert!(grid.page.rows[2].thumbnail.is_empty());
        assert!(jobs[0].path.exists() && !stray.exists(), "a stray tile goes, a wanted one stays");

        // A save's page: Select, Edit, Copy Code, Delete.
        let page = build(Node::SavedGraphic("front", 1), &c, &parts, &art);
        assert_eq!(page.page.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["#Select", "#Edit", "#Copy Code", "#Delete"]);
        assert!(matches!(page.acts[0], Act::Choose(None)));
        assert!(matches!(page.acts[1], Act::CreatorSaved("front", 1)));
        assert!(matches!(page.acts[2], Act::CopyCode("front", 1)));
        assert!(matches!(page.acts[3], Act::DeleteSaved("front", 1)));
        // Its code is the save's, for this item.
        let code = saved_code(&c, &parts, "front", 1).unwrap();
        assert_eq!(crate::share_code::decode(&code, TEE, |_| true).as_ref(), Ok(Store::load(&path).saves(TEE)[1]));
        assert!(saved_code(&c, &parts, "front", 9).is_none());
        assert!(Node::SavedGraphic("front", 1).in_graphic_flow(), "Select leaves the flow with it");

        // Delete: the save and its tile go; the grid was entered without wearing it, so the skater is
        // back as the grid found it (not blanked); the other save stays.
        std::fs::write(&jobs[1].path, b"png").unwrap();
        let mut entered = original.clone();
        assert!(delete_saved(&mut c, &parts, "front", 1, &mut entered));
        let store = Store::load(&path);
        assert_eq!(store.saves(TEE), [&one]);
        assert!(jobs[0].path.exists() && !jobs[1].path.exists(), "its tile went with it");
        assert_eq!(c.draft, original, "the look the grid was entered with is back");
        assert_eq!(entered, original);
        assert!(!delete_saved(&mut c, &parts, "front", 5, &mut entered), "there is no sixth save");
        assert_eq!(Store::load(&path).saves(TEE).len(), 1);

        // Entered wearing the save that is deleted: the item goes blank, and B on the grid keeps it blank.
        let Act::Item(patch, _) = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art).acts[1].clone() else { panic!() };
        c.apply(&parts, Some(&patch));
        let mut entered = c.draft.clone();
        assert_eq!(entered["graphics"]["front"]["design"], json!(design(1)));
        assert!(delete_saved(&mut c, &parts, "front", 0, &mut entered));
        assert!(c.draft["graphics"]["front"]["design"].is_null() && c.draft["graphics"]["back"]["design"].is_null());
        assert_eq!(entered, c.draft, "the grid is entered blank now");
        assert!(Store::load(&path).saves(TEE).is_empty());
    }

    #[test]
    fn a_corrupt_save_is_not_listed_and_the_old_my_graphics_move_in_on_first_use() {
        let (parts, path) = (parts(), profile("migrate"));
        let mut c = Customiser::for_test(&parts, Vec::new(), "male");
        c.set_profile_path(path.clone());
        let art = Art::default();
        // One good save beside one that no longer decodes: the page lists the good one, the file keeps both.
        std::fs::write(
            crate::graphic_store::file(&path),
            serde_json::to_vec(&json!({ TEE: [{"sides": {"front": "not a design"}}, {"sides": {"front": design(5)}}]})).unwrap(),
        )
        .unwrap();
        let grid = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art);
        assert_eq!(grid.page.rows.len(), 2, "New and the one good save");
        // My Graphics (eight single designs) land as front designs in the customizable tee's list, once.
        crate::custom_graphic::save_slots(&path, &[Some(design(7)), None, Some(design(8)), None, None, None, None, None]).unwrap();
        let grid = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art);
        assert_eq!(grid.page.rows.len(), 4, "New, the saved one, and the two moved in");
        assert!(!crate::custom_graphic::slots_file(&path).exists());
        let text = std::fs::read_to_string(crate::graphic_store::file(&path)).unwrap();
        assert!(text.contains("not a design"), "the entry that did not decode is kept in the file");
        assert_eq!(build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &art).page.rows.len(), 4, "and nothing moves twice");
    }

    /// Both genders find the old My Graphics: on the tee of their own body, whichever they play, and
    /// no model has to be called "Custom Tee" (none is).
    #[test]
    fn the_old_my_graphics_reach_the_tee_of_each_gender_by_its_real_id() {
        for gender in ["male", "female"] {
            let (parts, path) = (parts(), profile(&format!("both-{gender}")));
            assert!(parts.library.models.values().all(|m| m.name != "Custom Tee"));
            let mut c = Customiser::for_test(&parts, Vec::new(), gender);
            c.set_profile_path(path.clone());
            crate::custom_graphic::save_slots(&path, &[Some(design(7)), None, Some(design(8)), None, None, None, None, None]).unwrap();
            let grid = build(Node::GraphicGrid("front", CUSTOM), &c, &parts, &Art::default());
            assert_eq!(grid.page.rows.len(), 3, "{gender}: New and the two moved in");
            let store = Store::load(&path);
            let (own, other) = if gender == "male" { (TEE, WOMANS_TEE) } else { (WOMANS_TEE, TEE) };
            assert_eq!((store.saves(own).len(), store.saves(other).len()), (2, 2), "{gender}: each tee has them");
            assert!(!crate::custom_graphic::slots_file(&path).exists());
        }
    }

    /// A side wearing a Custom Graphic offers no Brand or Team Logos and Graphic Location moves no logo onto
    /// it (10-11: a team logo could be put over a Custom Graphic); Remove Graphic takes the design off, and the
    /// logos are offered again. A side with only a logo keeps them.
    #[test]
    fn a_custom_graphic_keeps_stock_logos_off_its_side() {
        let mut parts = parts();
        parts.library.graphics.insert("team".into(), serde_json::from_value(json!({"name": "Team", "kind": "team", "texture": "t.png"})).unwrap());
        let mut c = Customiser::for_test(&parts, Vec::new(), "male");
        let art = Art::default();
        c.apply(&parts, Some(&json!({"graphics": {"back": {"logo": "team"}, "front": {"design": design(1)}}})));
        assert_eq!(c.draft["graphics"]["back"]["logo"], "team");
        let kinds = build(Node::GraphicKind("front"), &c, &parts, &art);
        let texts: Vec<&str> = kinds.page.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["#Custom Graphics", "ID_CAC_REMOVE_GRAPHIC"]);
        assert!(matches!(kinds.acts[0], Act::Open(Node::GraphicGrid("front", CUSTOM))));
        let front = build(Node::GraphicLocation, &c, &parts, &art)
            .acts
            .into_iter()
            .find_map(|a| match a { Act::Item(p, Node::GraphicKind("front")) => Some(p), _ => None })
            .unwrap();
        c.apply(&parts, Some(&front));
        assert_eq!(c.draft["graphics"], json!({"back": {"logo": "team"}, "front": {"design": design(1)}}), "Shirt Front moves nothing");
        assert_eq!(build(Node::GraphicKind("back"), &c, &parts, &art).page.rows.len(), 4, "a side with only a logo lists them");
        let Some(Act::Choose(Some(remove))) = kinds.acts.last().cloned() else { panic!("Remove Graphic") };
        c.apply(&parts, Some(&remove));
        assert!(c.draft["graphics"].get("front").is_none(), "Remove Graphic takes the design off");
        assert_eq!(build(Node::GraphicKind("front"), &c, &parts, &art).page.rows.len(), 4, "the logos are back");
    }
}

#[cfg(test)]
mod deck_tests {
    use super::*;

    /// The Deck grid leaves out the stampable deck's plain colours (10-11): its Customizable Board is that deck in
    /// any colour. Other decks stay.
    #[test]
    fn the_deck_grid_lists_the_customizable_board_not_its_plain_colours() {
        let mut parts = Parts::default();
        let lib = &mut parts.library;
        let part = |slot: &str, name: &str, materials: &[&str]| crate::customiser_parts::Part {
            slot: slot.into(),
            name: name.into(),
            flags: Default::default(),
            materials: materials.iter().map(|m| (*m).to_owned()).collect(),
            scene: String::new(),
        };
        lib.models.insert("deck".into(), part("SkateBoard", "Skateboard", &["b", "bw", "y", "yw"]));
        lib.models.insert("pro".into(), part("SkateBoard", "Pro", &["p"]));
        lib.models.insert("head".into(), part("Rostral", "Head", &["p"]));
        for (id, name, logoable) in [("b", "Blue 1", true), ("bw", "Blue Dirty 1 (Worn)", true), ("y", "Yellow 1", true), ("yw", "Yellow Dirty 1 (Worn)", true), ("p", "Board Zero Bl 01", false)] {
            let flags = if logoable { json!({"IsLogoable": "true"}) } else { json!({}) };
            let material = json!({"name": name, "flags": flags, "diffuse": "x.png", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1});
            lib.materials.insert(id.into(), serde_json::from_value(material).unwrap());
        }
        lib.defaults.insert("male".into(), json!({"gender": "male", "selections": {
            "Rostral": {"asset_id": "head", "material_id": "p"}, "SkateBoard": {"asset_id": "deck", "material_id": "bw"}}}));
        let instances = |names: &[(&str, &str)]| names.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>();
        let catalog = json!({"materials": {}, "components": [{"models": [
            {"id": "deck", "name": "skateboard", "lods": [{"material_instances": [instances(&[("b", "blue_1"), ("bw", "blue_dirty_1"), ("y", "yellow_1"), ("yw", "yellow_dirty_1")])]}]},
            {"id": "pro", "name": "board_zero", "lods": [{"material_instances": [instances(&[("p", "bl_01")])]}]},
        ]}]});
        let thumbnails: serde_json::Map<String, Value> = ["skateboard_blue_1", "skateboard_blue_dirty_1", "skateboard_yellow_1", "skateboard_yellow_dirty_1", "board_zero_bl_01"]
            .iter()
            .map(|stem| (format!("skateboard/{stem}"), json!({})))
            .collect();
        let source = json!({"thumbnails": thumbnails, "language": {"ID_CAC_PART_SKATEBOARD_YELLOW_1": "Customizable Board", "ID_CAC_PART_SKATEBOARD_BLUE_1": "Blue"}});
        let path = std::env::temp_dir().join(format!("skate-cas-deck-{}.json", std::process::id()));
        std::fs::write(&path, catalog.to_string()).unwrap();
        let art = Art::load(&path, &parts.library, &source, std::path::Path::new("")).unwrap();
        let _ = std::fs::remove_file(&path);
        let c = Customiser::for_test(&parts, Vec::new(), "male");
        let built = build(Node::Board("Deck"), &c, &parts, &art);
        let texts: Vec<&str> = built.page.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["#Customizable Board", "#Pro"]);
    }

    /// Grip Color: its own page off Skateboards. Item 31 (not retail, user 10-11): its colour is the colour wheel's
    /// (one swatch, none: the authored grip); a colour offers Realistic or Solid, and a new colour keeps the style.
    #[test]
    fn the_grip_colour_page_sets_none_a_colour_and_its_style() {
        let mut parts = Parts::default();
        parts.library.colours = serde_json::from_value(json!([{"name": "Black", "rgb": [0.1, 0.1, 0.1]}, {"name": "Red", "rgb": [0.8, 0.1, 0.1]}])).unwrap();
        let head = crate::customiser_parts::Part { slot: "Rostral".into(), name: "Head".into(), flags: Default::default(), materials: vec!["p".into()], scene: String::new() };
        parts.library.models.insert("head".into(), head);
        let material = json!({"name": "Head", "flags": {}, "diffuse": "x.png", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1});
        parts.library.materials.insert("p".into(), serde_json::from_value(material).unwrap());
        parts.library.defaults.insert("male".into(), json!({"gender": "male", "selections": {"Rostral": {"asset_id": "head", "material_id": "p"}}}));
        let mut c = Customiser::for_test(&parts, Vec::new(), "male");
        let art = Art::default();
        let boards = build(Node::Boards, &c, &parts, &art);
        assert!(boards.acts.iter().any(|a| matches!(a, Act::Open(Node::Grip))));
        let page = |c: &Customiser| build(Node::Grip, c, &parts, &art);
        let first = page(&c);
        let Act::Wheel(wheel) = &first.acts[0] else { panic!("{:?}", first.acts[0]) };
        assert_eq!((first.page.rows.len(), first.page.rows[0].colours.len()), (1, 1), "none: no Style row");
        c.apply(&parts, Some(&wheel.patch([0.1, 0.1, 0.1])));
        assert_eq!(c.draft["grip"], json!({"rgb": [0.1, 0.1, 0.1], "solid": false}), "{}", c.status);
        let coloured = page(&c);
        assert_eq!((coloured.page.rows[0].colours[0], coloured.page.rows[1].value_text.as_str()), (SWATCH([0.1, 0.1, 0.1]), "#Realistic"));
        assert!(step(&mut c, &parts, &coloured.acts[1], 1));
        assert_eq!(c.draft["grip"]["solid"], true);
        let Act::Wheel(wheel) = page(&c).acts.swap_remove(0) else { unreachable!() };
        c.apply(&parts, Some(&wheel.patch([0.8, 0.1, 0.1])));
        assert_eq!(c.draft["grip"], json!({"rgb": [0.8, 0.1, 0.1], "solid": true}), "the style stays");
        assert_eq!(page(&c).page.rows[1].value_text, "#Solid");
        c.apply(&parts, Some(&wheel.default));
        assert!(c.draft["grip"].is_null() && page(&c).page.rows.len() == 1, "Default: none again");
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    /// Item 11: a grid search keeps tiles by name or by game ("skate 2", "dlc"), the acts in step, and the
    /// highlight on the worn tile when it is kept.
    #[test]
    fn a_grid_search_finds_names_games_and_dlc() {
        let tile = |text: &str, game: &'static str, kind: &'static str| Row { text: text.into(), game, kind, ..Default::default() };
        let rows = vec![
            tile("ID_CAC_REMOVE_PART", "", "none"),
            tile("#Black Zip Hoodie", "Skate 3", "owned"),
            tile("#Throwback Tee", "Skate 2", "dlc"),
            tile("#Plain Tee", "Skate 1", "owned"),
        ];
        let built = |worn: usize| Built {
            page: Page { title: String::new(), mode: crate::create_skater_runtime::Mode::Thumbs, rows: rows.clone(), index: worn, accept: String::new(), back: String::new() },
            acts: (0..rows.len()).map(|i| Act::Choose(Some(json!(i)))).collect(),
            frame: Frame::Body,
            strip: None,
        };
        let names = |search: &str, worn: usize| {
            let mut b = built(worn);
            super::search(&mut b, search);
            assert_eq!(b.page.rows.len(), b.acts.len());
            for (row, act) in b.page.rows.iter().zip(&b.acts) {
                let Act::Choose(Some(i)) = act else { panic!() };
                assert_eq!(rows[i.as_u64().unwrap() as usize], *row, "acts follow their rows");
            }
            (b.page.rows.iter().map(|r| r.text.trim_start_matches('#').to_owned()).collect::<Vec<_>>(), b.page.index)
        };
        assert_eq!(names("", 2).0.len(), 4, "no search: every tile");
        assert_eq!(names("", 2).1, 2);
        assert_eq!(names("TEE", 3), (vec!["Throwback Tee".into(), "Plain Tee".into()], 1), "any case; the worn tile keeps the highlight");
        assert_eq!(names("zip-hoodie", 3), (vec!["Black Zip Hoodie".into()], 0), "letters and digits only; worn tile gone: the first");
        assert_eq!(names("skate 2", 0).0, ["Throwback Tee"]);
        assert_eq!(names("Skate1", 0).0, ["Plain Tee"]);
        assert_eq!(names("DLC", 0).0, ["Throwback Tee"]);
        assert!(names("skate", 0).0.is_empty(), "a game is named whole");
        assert_eq!(names("remove", 0).0.len(), 0, "string-ID rows answer only no search");
        // RB / LB: all, then one game at a time, the Remove tile always kept.
        let only = |game: usize, worn: usize| {
            let mut b = built(worn);
            super::only_game(&mut b, GAMES[game]);
            assert_eq!(b.page.rows.len(), b.acts.len());
            (b.page.rows.iter().map(|r| r.text.trim_start_matches('#').to_owned()).collect::<Vec<_>>(), b.page.index)
        };
        assert_eq!(only(0, 2).0.len(), 4, "all");
        assert_eq!(only(1, 2), (vec!["ID_CAC_REMOVE_PART".into(), "Black Zip Hoodie".into()], 0));
        assert_eq!(only(2, 2), (vec!["ID_CAC_REMOVE_PART".into(), "Throwback Tee".into()], 1), "the worn tile keeps the highlight");
        assert_eq!(only(3, 2).0, ["ID_CAC_REMOVE_PART", "Plain Tee"]);
        let steps: Vec<usize> = std::iter::successors(Some(0), |&g| Some(next_game(g, true, false))).take(5).collect();
        assert_eq!(steps, [0, 1, 2, 3, 0], "RB: Skate 3, Skate 2, Skate 1, then all again");
        assert_eq!(next_game(0, false, true), 3, "LB the other way");
        assert_eq!(next_game(1, false, true), 0);
        assert_eq!(next_game(2, true, true), 2, "both at once: no step");
    }
}

/// Item 31 (not retail, user 10-11): the clothing, grip and hair colour rows are the colour wheel's.
#[cfg(test)]
mod colour_wheel_tests {
    use super::*;
    use crate::colour_wheel::{Open, Wheel};

    const SHOE: [f32; 3] = [0.8, 0.7, 0.6];
    const HAIR: [f32; 3] = [0.02, 0.01, 0.01];
    /// A head, worn hair and a colourizable shoe, two skin tones and two retail hair colours.
    fn fixture() -> (Parts, Customiser) {
        let mut parts = Parts::default();
        let lib = &mut parts.library;
        lib.colours = serde_json::from_value(json!([{"name": "Black", "rgb": [0.1, 0.1, 0.1]}, {"name": "Red", "rgb": [0.8, 0.1, 0.1]}])).unwrap();
        for (id, slot, material) in [("head", "Rostral", "m"), ("shoe", "Feet", "m"), ("hair", "Hair", "h")] {
            let part = crate::customiser_parts::Part { slot: slot.into(), name: id.into(), flags: Default::default(), materials: vec![material.into()], scene: String::new() };
            lib.models.insert(id.into(), part);
        }
        for (id, flags, tint) in [("m", json!({"IsColourizable": "True"}), SHOE), ("h", json!({}), HAIR)] {
            let material = json!({"name": id, "flags": flags, "diffuse": "x.png", "alpha": false, "tint": tint, "metallic": 0, "roughness": 1});
            lib.materials.insert(id.into(), serde_json::from_value(material).unwrap());
        }
        let worn = |id: &str, m: &str| json!({"asset_id": id, "material_id": m});
        lib.defaults.insert(
            "male".into(),
            json!({"gender": "male", "selections": {"Rostral": worn("head", "m"), "Feet": worn("shoe", "m"), "Hair": worn("hair", "h")}}),
        );
        let extras: Vec<crate::customiser::Entry> = serde_json::from_value(json!([
            {"label": "Skin tone", "children": [{"label": "Pale", "patch": {"skin_tint": [0.9, 0.8, 0.7]}}, {"label": "Dark", "patch": {"skin_tint": [0.3, 0.2, 0.1]}}]},
            {"label": "Hair colour", "children": [
                {"label": "Blond", "swatch": [1.0, 0.9, 0.65], "patch": {"hair_tint": [0.3, 0.2, 0.1]}},
                {"label": "Grey", "swatch": [0.62, 0.62, 0.62], "patch": {"hair_tint": [0.1, 0.1, 0.1]}},
            ]},
        ]))
        .unwrap();
        let c = Customiser::for_test(&parts, extras, "male");
        (parts, c)
    }
    const SHOES: Node = Node::Garment("Shoes", false, 0, 0);
    /// The page's colour wheel row: its index and wheel.
    fn wheel_of(built: &Built) -> Option<(usize, Custom)> {
        built.acts.iter().enumerate().find_map(|(i, a)| match a {
            Act::Wheel(custom) => Some((i, custom.clone())),
            _ => None,
        })
    }
    fn near(a: [f32; 3], b: [f32; 3], by: f32) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= by)
    }

    /// The user (10-11): the Custom swatch sat past the end of the swatch row, off screen; the rows are the wheel now.
    #[test]
    fn the_clothing_grip_and_hair_colour_rows_are_one_swatch_for_the_wheel_and_skin_and_eyes_stay_retail() {
        let (parts, mut c) = fixture();
        let art = Art::default();
        let page = |n: Node, c: &Customiser| build(n, c, &parts, &art);
        for (node, own) in [(SHOES, SHOE), (Node::Grip, [0.256, 0.272, 0.296]), (Node::Hair, hair_shown(HAIR))] {
            let built = page(node, &c);
            let (row, custom) = wheel_of(&built).unwrap_or_else(|| panic!("{node:?} has the colour wheel"));
            assert_eq!(built.page.rows[row].colours, [SWATCH(own)], "{node:?}: one swatch, the item's own colour");
            assert_eq!((custom.start, custom.authored), (own, own), "{node:?}");
            assert!(!step(&mut c, &parts, &built.acts[row], 1), "{node:?}: Left / Right step no swatches (they open the wheel)");
        }
        let skin = page(Node::Skin, &c);
        assert!(matches!(&skin.acts[0], Act::Cycle(tones) if tones.len() == 2) && skin.page.rows[0].colours.len() == 2, "Skin keeps its retail swatches");
        for node in [Node::Skin, Node::Eyes] {
            assert!(wheel_of(&page(node, &c)).is_none(), "{node:?} has no colour wheel");
        }
        // Worn colours: the swatch and the wheel's start.
        let (_, shoe) = wheel_of(&page(SHOES, &c)).unwrap();
        c.apply(&parts, Some(&shoe.patch([0.25, 0.5, 0.1234])));
        assert_eq!(c.draft["colours"]["Feet"], json!([0.25, 0.5, 0.123]), "{}", c.status);
        let built = page(SHOES, &c);
        let (row, custom) = wheel_of(&built).unwrap();
        assert_eq!((built.page.rows[row].colours[0], custom.start, custom.authored), (SWATCH([0.25, 0.5, 0.123]), [0.25, 0.5, 0.123], SHOE));
        // Saved and loaded: the profile file, a teammate's look and a peer's outfit are this JSON, resolved.
        let saved: Value = serde_json::from_slice(&serde_json::to_vec_pretty(&c.draft).unwrap()).unwrap();
        assert_eq!(parts.resolve(&saved).unwrap(), parts.resolve(&c.draft).unwrap());
        let mut loaded = fixture().1;
        loaded.draft = parts.resolve(&saved).unwrap();
        assert_eq!(wheel_of(&page(SHOES, &loaded)).unwrap().1.start, [0.25, 0.5, 0.123], "still worn after loading");
    }

    #[test]
    fn the_wheel_previews_live_a_keeps_it_b_puts_it_back_and_y_is_the_items_own() {
        let (parts, mut c) = fixture();
        let art = Art::default();
        let (_, custom) = wheel_of(&build(Node::Grip, &c, &parts, &art)).unwrap();
        let before = c.draft.clone();
        let mut open = Open::new(&c, custom.clone());
        open.preview(&mut c, &parts);
        assert_eq!(c.draft, before, "opening changes nothing: A at once keeps the grip as it was");
        open.wheel = Wheel { hue: 0.5, saturation: 1., value: 1. };
        open.preview(&mut c, &parts);
        assert_eq!(c.draft["grip"]["rgb"], json!([0.0, 1.0, 1.0]), "live: {}", c.status);
        open.cancel(&mut c);
        assert_eq!(c.draft, before, "B: the grip as it was");
        let mut open = Open::new(&c, custom);
        open.wheel = Wheel { hue: 0., saturation: 1., value: 0.5 };
        open.preview(&mut c, &parts);
        drop(open);
        assert_eq!(c.draft["grip"], json!({"rgb": [0.5, 0.0, 0.0], "solid": false}), "A: the colour stays");
        // Y: the item's own colour, and the wheel goes to it.
        for node in [Node::Grip, SHOES, Node::Hair] {
            let (_, custom) = wheel_of(&build(node, &c, &parts, &art)).unwrap();
            let mut open = Open::new(&c, custom.clone());
            open.wheel = Wheel { hue: 0.3, saturation: 1., value: 1. };
            open.preview(&mut c, &parts);
            assert_ne!(wheel_of(&build(node, &c, &parts, &art)).unwrap().1.start, custom.authored, "{node:?} coloured");
            open.default(&mut c, &parts);
            assert!(near(open.wheel.rgb(), custom.authored, 1e-4), "{node:?}: the wheel on the item's own");
            open.preview(&mut c, &parts);
            drop(open);
            assert_eq!(wheel_of(&build(node, &c, &parts, &art)).unwrap().1.start, custom.authored, "{node:?}: {}", c.status);
            assert!(parts.resolve(&c.draft).is_ok(), "{node:?}: the item's own colour is a valid look: {:?}", parts.resolve(&c.draft).err());
        }
        assert!(c.draft["grip"].is_null() && c.draft["colours"]["Feet"].is_null() && c.draft["hair_tint"].is_null());
    }

    /// Round 3 (user 10-11): A on the colour row opens the wheel; B leaves the page (it may have no other row).
    #[test]
    fn a_page_with_the_colour_wheel_shows_b_back_and_says_a_opens_it() {
        let (parts, c) = fixture();
        let art = Art::default();
        for node in [SHOES, Node::Grip, Node::Hair] {
            let built = build(node, &c, &parts, &art);
            let (row, _) = wheel_of(&built).unwrap();
            assert_eq!(built.page.back, "ID_COMMON_BACK", "{node:?}");
            assert_eq!(built.page.rows[row].description, WHEEL_BLURB, "{node:?}");
        }
        for node in [Node::Skin, Node::Eyes] {
            assert_eq!(build(node, &c, &parts, &art).page.back, "#", "{node:?} stays retail");
        }
    }

    /// User 10-30, not retail (it ignores B there): a tattoo spot's categories show B Back, so opening a spot doesn't
    /// force a tattoo.
    #[test]
    fn the_tattoo_categories_show_b_back() {
        let (parts, c) = fixture();
        let art = Art::default();
        for node in [Node::TattooKind(true, 2), Node::TattooKind(false, 1)] {
            assert_eq!(build(node, &c, &parts, &art).page.back, "ID_COMMON_BACK", "{node:?}");
        }
    }

    #[test]
    fn hair_shows_its_colour_not_its_tint() {
        let (parts, mut c) = fixture();
        let art = Art::default();
        let page = |c: &Customiser| wheel_of(&build(Node::Hair, c, &parts, &art)).unwrap().1;
        for tint in [HAIR, [0.3, 0.2, 0.1], [0.1, 0.1, 0.1], [0., 0., 0.]] {
            assert!(near(hair_tint(hair_shown(tint)), tint, 1e-5), "{tint:?}");
        }
        // A retail hair colour worn: the wheel starts on its swatch.
        let blond = c.entry(&["Body", "Hair colour"]).unwrap().children[0].patch.clone();
        c.apply(&parts, blond.as_ref());
        assert_eq!(page(&c).start, [1.0, 0.9, 0.65]);
        // The wheel on the swatch's colour is about the retail tint, and never brighter than retail's brightest.
        let custom = page(&c);
        let tint: [f32; 3] = serde_json::from_value(custom.patch(custom.start)["hair_tint"].clone()).unwrap();
        assert!(near(tint, [0.3, 0.2, 0.1], 0.05), "{tint:?}");
        assert_eq!(custom.patch([1.; 3])["hair_tint"], json!([0.3, 0.3, 0.3]));
        c.apply(&parts, Some(&custom.patch([0.62; 3])));
        assert!(near(page(&c).start, [0.62; 3], 0.002), "a custom tint shows its colour: {:?}", page(&c).start);
    }

    #[test]
    fn peers_out_of_range_tints_are_rejected_and_the_old_memory_is_dropped() {
        use crate::multiplayer::appearance::{Look, read_look};
        let (parts, c) = fixture();
        // What a peer receives: the outfit's bytes, then Parts::resolve (multiplayer::render).
        let received = |p: &Value| match read_look(&serde_json::to_vec(&Look::Outfit(p.clone())).unwrap()) {
            Ok(Look::Outfit(p)) => parts.resolve(&p),
            other => panic!("{other:?}"),
        };
        for (key, bad, error) in [
            ("colours", json!({"Feet": [1.5, 0, 0]}), "Invalid clothing colour"),
            ("colours", json!({"Feet": [0, 1e39, 0]}), "Invalid clothing colour"),
            ("colours", json!({"Feet": [0, 0]}), "Invalid clothing colour"),
            ("hair_tint", json!([-0.1, 0, 0]), "Invalid colour"),
            ("hair_tint", json!("brown"), "Invalid colour"),
            ("grip", json!({"rgb": [0, 0, 2], "solid": false}), "Invalid grip colour"),
            ("grip", json!({"rgb": [0, 0, "x"]}), "Invalid grip colour"),
        ] {
            let mut p = c.draft.clone();
            p[key] = bad.clone();
            assert_eq!(received(&p).unwrap_err(), error, "{key} {bad}");
        }
        let mut p = c.draft.clone();
        p["hair_tint"] = Value::Null;
        assert!(received(&p).is_ok(), "no hair tint: the hair's own (Default)");
        // The first preview (10-11) kept each row's Custom colour in the profile: no longer anything's.
        p["custom_colours"] = json!({"Feet": [0.2, 0.3, 0.4]});
        assert!(received(&p).unwrap().get("custom_colours").is_none());
    }

    #[test]
    fn a_custom_tint_changes_the_look_hash_and_reaches_a_friends_copy() {
        let (parts, mut c) = fixture();
        let art = Art::default();
        let setup: crate::team_select::PhotoSetup =
            serde_json::from_value(json!({"camera": [0, 0, 3], "target": [0, 0, 0], "colour": [0, 0, 0, 1], "background": null, "width": 8, "height": 8})).unwrap();
        let photo = |look: &Value| crate::team_photo::cache_file(std::path::Path::new("photos"), look, &setup, None, "inputs");
        let (_, grip) = wheel_of(&build(Node::Grip, &c, &parts, &art)).unwrap();
        let before = photo(&c.draft);
        c.apply(&parts, Some(&grip.patch([0.2, 0.4, 0.6])));
        let custom = photo(&c.draft);
        assert_ne!(custom, before, "a team photo of the look is drawn again");
        c.apply(&parts, Some(&grip.patch([0.2, 0.4, 0.61])));
        assert_ne!(photo(&c.draft), custom, "and again for another custom colour");
        let friend = crate::team_select::sanitize_friend(&serde_json::to_vec(&c.draft).unwrap()).unwrap();
        assert_eq!(friend["grip"], c.draft["grip"], "a friend's copy keeps the grip");
        assert_eq!(parts.resolve(&friend).unwrap()["grip"], c.draft["grip"]);
    }
}
