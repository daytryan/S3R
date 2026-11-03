//! The Graphic Creator's editing rules, with no screen in them: one item's sides (each a whole-side
//! design), the selected layer, the Add picker, layer Options and the Menu, undo and redo, saving into
//! the item's Custom Graphics list, and the controls. The pad is retail Skate 3's Graphic Adjust
//! (RS moves, LS scales, LB / RB rotate, Y resets) grown up: L3 stretch, LT fine, RT + D-pad reorder.
//! `graphic_pages` shows it on the Edit Skater card, `graphic_board` draws the side, and
//! `graphic_creator` feeds it the frame's input.
use crate::{
    boot_flow::Key,
    core_menu_runtime::{SOUND_A_BUTTON, SOUND_B_BUTTON, SOUND_NAV_DOWN, SOUND_NAV_UP, fe_sound_id},
    text_entry::{Command2, TextEntry},
    custom_graphic::{Art, Doc, Layer, MAX_LAYERS, MAX_TEXT, REACH, SHAPES, SIZES},
    graphic_store::{Full, Saved, Store},
};
use bevy::math::Vec2;

/// XInput button bits, as `customiser::Navigation` reports them.
pub(crate) mod button {
    pub const UP: u16 = 0x1;
    pub const DOWN: u16 = 0x2;
    pub const LEFT: u16 = 0x4;
    pub const RIGHT: u16 = 0x8;
    pub const START: u16 = 0x10;
    pub const BACK: u16 = 0x20;
    pub const L3: u16 = 0x40;
    pub const LB: u16 = 0x100;
    pub const RB: u16 = 0x200;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// The swatch row (the website's picker was a palette too).
pub(crate) const PALETTE: [[u8; 3]; 24] = [
    // Red next to white: one Right from a new piece (research: a red circle in 7 presses).
    [255, 255, 255], [220, 30, 40], [0, 0, 0], [192, 192, 192], [128, 128, 128], [64, 64, 64],
    [255, 110, 20], [255, 200, 0], [255, 240, 120], [150, 220, 40], [30, 160, 60], [0, 110, 70],
    [0, 180, 170], [60, 200, 255], [30, 100, 220], [20, 40, 120], [120, 60, 200], [200, 60, 200],
    [255, 105, 180], [120, 20, 30], [140, 80, 40], [210, 170, 120], [200, 160, 40], [250, 230, 200],
];
/// Undo steps kept.
const HISTORY: usize = 50;
/// Recent colours kept.
const RECENT: usize = 8;
/// The Add grid (retail ThumbsCustom, mThumbs2): columns, and the rows on screen at once.
pub(crate) const PICKER_COLS: usize = 3;
/// Saves shown per row on the replace screen (the same 3x5 grid).
pub(crate) const REPLACE_COLS: usize = 3;
/// Pad speeds at full deflection (LT held is `FINE` of that): the layer moves this many thousandths a
/// second, sizes by this factor's power a second, and turns this many degrees a second.
pub(crate) const MOVE: f32 = 1400.;
pub(crate) const GROW: f32 = 0.9;
pub(crate) const TURN: f32 = 100.;
pub(crate) const FINE: f32 = 0.15;
const DEADZONE: f32 = 0.2;
/// Not retail (user 10-19): a stick pushed this far on the Add grid places the highlighted piece (past drift).
const PLACE: f32 = 0.5;
/// A held turn stops this long on every multiple of 45 degrees.
pub(crate) const DETENT: f32 = 0.15;
/// D-pad repeat: first after this long held, then this often.
const REPEAT_DELAY: f32 = 0.4;
const REPEAT_EVERY: f32 = 0.1;
/// A handle's reach, and the turn handle's distance above the layer's box, in screen pixels.
pub(crate) const HANDLE: f32 = 13.;
pub(crate) const TURN_GAP: f32 = 30.;
/// Shift snaps a mouse turn to this many degrees; the arrow keys nudge this far (Shift ten times), canvas units.
const SNAP: f32 = 15.;
const NUDGE: i16 = 10;
/// A note (`Editor::status`) shows in the card's info panel this long, seconds.
const NOTE: f32 = 2.;
/// Left / Right sounds of the movie's own selectors.
fn nav_side(right: bool) -> u64 {
    fe_sound_id(if right { "core_nav_right" } else { "core_nav_left" })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tab {
    Shapes,
    Logos,
    GameArt,
    Text,
}
/// The Add grid's tabs, LB / RB.
pub(crate) const TABS: [Tab; 4] = [Tab::Shapes, Tab::GameArt, Tab::Logos, Tab::Text];
impl Tab {
    pub fn name(self) -> &'static str {
        match self {
            Tab::Shapes => "Shapes",
            Tab::Logos => "Logos",
            Tab::GameArt => "Skate Art",
            Tab::Text => "Text",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// The Layers list and the board: the pad edits the selected layer.
    Board,
    /// The Add grid: the highlighted piece is previewed on the board.
    Add,
    /// The selected layer's Options.
    Options,
    /// Start's Menu.
    Menu,
    /// Typing into the selected text layer.
    Text,
    /// A retail popup asks.
    Popup(Ask),
    /// New: what to start from (Blank, a layout, another item's save).
    StartFrom,
    /// The item's list is full: which save to write over.
    Replace,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ask {
    /// B with unsaved changes: Save and Wear / Wear Without Saving / Discard Changes / Keep Editing.
    Done,
    /// Delete the selected layer: Yes / No.
    Delete,
    /// Clear the side: Yes / No.
    ClearSide,
    /// A pasted share code failed (true: it is for another item): OK, back to Start From.
    Code(bool),
    /// The Custom Graphics file could not be written: OK.
    SaveFailed,
    /// The side already holds `MAX_LAYERS`: OK.
    Full,
}
/// The Done popup's rows, in this order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DoneChoice {
    Save,
    KeepWithoutSaving,
    Discard,
    Cancel,
}
pub(crate) const DONE_CHOICES: [DoneChoice; 4] = [DoneChoice::Save, DoneChoice::KeepWithoutSaving, DoneChoice::Discard, DoneChoice::Cancel];
impl Ask {
    /// The popup's question and rows (string ids, or `#` literals).
    pub fn question(self) -> (&'static str, &'static [&'static str]) {
        match self {
            // Not retail (user 10-19): asked only with unsaved changes (`Editor::leave`), each row saying what it
            // does to the skater; Cancel reads as staying.
            Ask::Done => ("#Save your changes?", &["#Save and Wear", "#Wear Without Saving", "#Discard Changes", "#Keep Editing"]),
            Ask::Delete => ("#Delete this layer?", &["ID_COMMON_YES", "ID_COMMON_NO"]),
            Ask::ClearSide => ("#Clear everything on this side?", &["ID_COMMON_YES", "ID_COMMON_NO"]),
            Ask::Code(other) => (
                crate::share_code::failure(&if other { crate::share_code::CodeError::OtherItem(String::new()) } else { crate::share_code::CodeError::Format }),
                &["ID_COMMON_OK"],
            ),
            Ask::SaveFailed => ("#Could not save to Custom Graphics.", &["ID_COMMON_OK"]),
            Ask::Full => ("#A side holds at most 64 layers.", &["ID_COMMON_OK"]),
        }
    }
}

/// New's ready-made starts, after Blank (`layout`).
pub(crate) const LAYOUTS: [&str; 3] = ["Chest Logo", "Big Back Print", "Sleeve Stripes"];
/// Another item's save New can start from.
#[derive(Clone, Debug)]
pub(crate) struct Source {
    pub label: String,
    /// Its Custom Graphics tile, when drawn.
    pub tile: String,
    pub saved: Saved,
}
/// Layout `i`'s layers for side `zone` (of `size`, canvas units); empty where it puts nothing.
fn layout(i: usize, zone: &str, size: Vec2) -> Vec<Layer> {
    let shape = |n: u8, colour: [u8; 4], x: i16, y: i16, w: u16, h: u16| Layer { x, y, w, h, ..Layer::new(Art::Shape(n), colour) };
    let (red, white) = ([200, 30, 40, 255], [255; 4]);
    match (i, zone) {
        // Wearer's left chest: a ring with a star in it, moving together.
        (0, "front" | "hat" | "deck") => vec![shape(3, white, 600, -900, 700, 700), Layer { grouped: true, ..shape(7, red, 600, -900, 420, 420) }],
        // A burst across the back with a word over it.
        (1, "back" | "deck") => {
            let mut word = Layer { y: -300, w: 2400, h: 520, ..Layer::new(Art::Text("SKATE".into()), white) };
            word.fit_text();
            vec![shape(8, red, 0, -300, 2600, 2600), word]
        }
        // Two bands round each sleeve.
        (2, "left" | "right") => {
            let (w, gap) = ((size.x as u16).min(SIZES.1), (size.y * 0.12) as i16);
            vec![shape(14, red, 0, -gap, w, 260), shape(14, white, 0, gap, w, 260)]
        }
        _ => Vec::new(),
    }
}

/// A Layer Options row, in the page's order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Opt {
    Colour,
    Recent,
    Hue,
    Shade,
    Opacity,
    Text,
    MirrorCopy,
    PairCopy,
    Flip,
    Duplicate,
    MoveUp,
    MoveDown,
    Group,
    LayersUnder,
    Delete,
}
/// A Menu row, in the page's order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuRow {
    Save,
    SaveNew,
    Redo,
    CopySide,
    ClearSide,
    Background,
    BackgroundOpacity,
    Grid,
    Help,
}

/// How the editor ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Exit {
    /// Keep what the skater wears.
    Done,
    /// Put the skater back as it was.
    Discard,
}

/// A key typed on the PC keyboard into a text layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Typed {
    Char(char),
    Backspace,
    Delete,
    Enter,
    Escape,
}

/// One frame of pad state: what is held now. The editor finds the presses (and the D-pad's repeats).
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Pad {
    /// Sticks, -1 to 1, y up.
    pub rs: Vec2,
    pub ls: Vec2,
    pub l3: bool,
    pub lb: bool,
    pub rb: bool,
    pub lt: bool,
    pub rt: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub back: bool,
    pub start: bool,
    pub dt: f32,
}
/// The buttons that went down this frame (D-pad repeats included).
#[derive(Clone, Copy, Default, Debug)]
struct Presses {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    a: bool,
    b: bool,
    x: bool,
    y: bool,
    back: bool,
    start: bool,
    l3: bool,
    lb: bool,
    rb: bool,
}

/// The D-pad's presses: the edge, then a repeat while held. The creator keeps one for the movie's pages.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Repeat {
    held: [f32; 4],
    prev: [bool; 4],
}
impl Repeat {
    /// `now`: up, down, left, right held this frame.
    pub fn step(&mut self, now: [bool; 4], dt: f32) -> [bool; 4] {
        let mut out = [false; 4];
        for i in 0..4 {
            if !now[i] {
                self.held[i] = 0.;
            } else if !self.prev[i] {
                self.held[i] = 0.;
                out[i] = true;
            } else {
                self.held[i] += dt;
                if self.held[i] >= REPEAT_DELAY {
                    self.held[i] -= REPEAT_EVERY;
                    out[i] = true;
                }
            }
        }
        self.prev = now;
        out
    }
}

/// The mouse, in canvas units (thousandths, y down, the side's centre the origin).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Pointer {
    /// Alt cycles through the layers under the cursor.
    Press { at: Vec2, right: bool, alt: bool },
    /// Shift snaps a turn to 15 degrees.
    Drag { at: Vec2, shift: bool },
    Release,
    /// Shift turns instead of scaling.
    Wheel { at: Vec2, lines: f32, shift: bool },
}
/// A handle on the selected layer's box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Handle {
    /// Scales evenly.
    Corner,
    /// The left and right edges stretch the width, the top and bottom the height.
    Width,
    Height,
    Turn,
}
/// The keyboard's shortcuts on the board.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shortcut {
    Undo,
    Redo,
    Duplicate,
    Copy,
    Paste,
    Mirror,
    Delete,
    /// Arrow keys: canvas steps, Shift ten times as far.
    Nudge(i16, i16, bool),
}

/// The stock logos and game art the picker offers: (key, name).
#[derive(Clone, Default, Debug)]
pub(crate) struct Catalog {
    pub logos: Vec<(u64, String)>,
    pub art: Vec<(u64, String)>,
}

/// One side of the item: a whole-side design on the side's rectangle.
#[derive(Clone, Debug)]
pub(crate) struct SideDoc {
    pub zone: &'static str,
    /// `[top, bottom, left, right]`, V up.
    pub rect: [f32; 4],
    pub doc: Doc,
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    docs: Vec<Doc>,
    side: usize,
    layer: usize,
}
/// The selected layer's exact values while a stick (or the mouse) moves it, and its group as it was
/// when the gesture began: the layers hold whole thousandths, and the group follows from those.
#[derive(Clone, Debug)]
struct Live {
    side: usize,
    layer: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    angle: f32,
    /// The group's layers (the selected one included) at the start.
    start: Vec<(usize, Layer)>,
    /// Seconds left at a 45 degree detent.
    hold: f32,
}
#[derive(Clone, Copy, Debug)]
enum Drag {
    Move { grab: Vec2 },
    Size { start: f32, w: f32, h: f32 },
    /// Along the layer's own x (0) or y (1) axis, from `start` canvas units off its centre.
    Stretch { axis: usize, start: f32, size: f32 },
    Turn,
}
#[derive(Clone, Copy, Debug)]
struct Dragging {
    kind: Drag,
    /// The undo step is taken at the first movement.
    started: bool,
}

pub(crate) struct Picker {
    pub tab: Tab,
    pub index: [usize; 4],
}

pub(crate) struct Editor {
    /// The worn item's model id: the key of its Custom Graphics list.
    pub item: String,
    pub name: String,
    pub sides: Vec<SideDoc>,
    pub side: usize,
    pub layer: usize,
    pub mode: Mode,
    pub picker: Picker,
    pub option: usize,
    pub menu: usize,
    /// The popup's highlighted row.
    pub popup: usize,
    /// Replace: the save picked.
    pub replace: usize,
    /// A short note for the card's info panel (`say`), and the seconds it still shows.
    pub status: String,
    status_left: f32,
    /// Sounds of the presses only the editor handles, for the screen to play (the movie plays its own).
    pub sounds: Vec<u64>,
    pub catalog: Catalog,
    pub store: Store,
    /// The save being edited (number among the item's valid saves).
    pub editing: Option<usize>,
    /// Anything changed since the editor opened or last saved.
    pub changed: bool,
    /// Counts every change to what is drawn, and the designs alone.
    pub rev: u64,
    pub doc_rev: u64,
    /// Thousandths of canvas per screen pixel, for the handles (set by the screen).
    pub view_scale: f32,
    /// The mouse has been used: the board draws the edge and turn handles too.
    pub mouse: bool,
    /// Ctrl+C's layer, for Ctrl+V on any side.
    copied: Option<Layer>,
    /// The mode a popup was asked from (No goes back there).
    asked_from: Mode,
    /// A save just written, for the screen to put on disk and draw a tile for.
    pub saved: Option<Saved>,
    /// The board's grid (Menu).
    pub grid: bool,
    /// The pad keyboard of Text mode.
    pub keyboard: TextEntry,
    /// Options: the highlighted recent colour, layer under here and Copy Side target.
    pub recent_at: usize,
    pub under: usize,
    pub copy_to: usize,
    /// The Menu's Help page showing (`graphic_pages::HELP`).
    pub help: usize,
    /// The first Options / Menu row on the card (`graphic_pages::scroll`).
    pub scroll: usize,
    /// Start From: the highlighted row, the From Another Item list showing, and that list.
    pub start: usize,
    pub from_other: bool,
    pub sources: Vec<Source>,
    /// Paste Code was chosen: the screen reads the clipboard into `paste`.
    pub pasting: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    live: Option<Live>,
    gesture: bool,
    /// A mouse burst (a list-row drag, wheel notches) is one undo step: `Some(true)` once it is taken.
    burst: Option<bool>,
    drag: Option<Dragging>,
    stretch: bool,
    prev: Pad,
    dpad: Repeat,
    recent: Vec<[u8; 3]>,
    /// The newest recent colour is the one a slider is moving (replaced, not added, while it moves).
    recent_live: bool,
    /// Add: the layer the highlighted piece is previewed as, and the side before the preview.
    preview: Option<usize>,
    add_before: Option<Snapshot>,
    /// Text: the text before typing began (Cancel puts it back; `None` for a new text layer).
    text_before: Option<String>,
    /// The Done popup's Save filled the list: leave once a save is replaced.
    exit_after_save: bool,
}

pub(crate) fn palette_index(colour: [u8; 4]) -> usize {
    let distance = |p: &[u8; 3]| (0..3).map(|i| (p[i] as i32 - colour[i] as i32).pow(2)).sum::<i32>();
    PALETTE.iter().enumerate().min_by_key(|(_, p)| distance(p)).map_or(0, |(i, _)| i)
}
pub(crate) fn opacity_percent(alpha: u8) -> u32 {
    (alpha as u32 * 100 + 127) / 255
}
fn alpha_of(percent: i32) -> u8 {
    ((percent.clamp(0, 100) * 255 + 50) / 100) as u8
}
fn wrap_angle(degrees: f32) -> f32 {
    (degrees + 180.).rem_euclid(360.) - 180.
}
/// RGB to hue (degrees), saturation and lightness (0-1).
pub(crate) fn hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.;
    let d = max - min;
    if d < 1e-6 {
        return (0., 0., l);
    }
    let s = d / (1. - (2. * l - 1.).abs()).max(1e-6);
    let h = if max == r {
        60. * ((g - b) / d).rem_euclid(6.)
    } else if max == g {
        60. * ((b - r) / d + 2.)
    } else {
        60. * ((r - g) / d + 4.)
    };
    (h, s.min(1.), l)
}
pub(crate) fn rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h / 60.).rem_euclid(2.) - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match (h.rem_euclid(360.) / 60.) as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    [r, g, b].map(|v| ((v + m) * 255.).round().clamp(0., 255.) as u8)
}
/// The side a canvas shows, in thousandths: its long side 4000 (`custom_graphic::canvas`).
pub(crate) fn extent(rect: [f32; 4]) -> Vec2 {
    let [top, bottom, left, right] = rect;
    let (w, h) = (right - left, top - bottom);
    if !(w > 0. && h > 0.) {
        return Vec2::new(4000., 2000.);
    }
    Vec2::new(w, h) / w.max(h) * 4000.
}
/// The side across the item: what Copy to Back sends a layer to.
fn pair_zone(zone: &str) -> Option<&'static str> {
    match zone {
        "front" => Some("back"),
        "back" => Some("front"),
        "left" => Some("right"),
        "right" => Some("left"),
        _ => None,
    }
}
/// A side's name on the board's selector.
pub(crate) fn side_name(zone: &str) -> &'static str {
    match zone {
        "front" => "Front",
        "back" => "Back",
        "left" => "L Sleeve",
        "right" => "R Sleeve",
        "hat" => "Hat Front",
        _ => "Deck Bottom",
    }
}
/// A layer mirrored across its side's centre line (text keeps reading the right way).
fn mirrored(l: &Layer) -> Layer {
    let text = matches!(l.art, Art::Text(_));
    Layer { x: -l.x, angle: if l.angle == -180 { 180 } else { -l.angle }, flip_x: l.flip_x != !text, grouped: false, ..l.clone() }
}
/// The four corners of a layer's turned box, canvas units.
fn corners(l: &Layer) -> [Vec2; 4] {
    let (s, c) = (l.angle as f32).to_radians().sin_cos();
    let (hw, hh) = (l.w as f32 / 2., l.h as f32 / 2.);
    [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| Vec2::new(l.x as f32 + x * c - y * s, l.y as f32 + x * s + y * c))
}

impl Editor {
    pub fn new(item: &str, name: &str, sides: Vec<SideDoc>, side: usize, catalog: Catalog, store: Store) -> Self {
        let layer = sides.get(side).map_or(0, |s| s.doc.layers.len().saturating_sub(1));
        Self {
            item: item.to_owned(),
            name: name.to_owned(),
            side: side.min(sides.len().saturating_sub(1)),
            layer,
            sides,
            mode: Mode::Board,
            picker: Picker { tab: Tab::Shapes, index: [0; 4] },
            option: 0,
            menu: 0,
            popup: 0,
            replace: 0,
            status: String::new(),
            status_left: 0.,
            sounds: Vec::new(),
            catalog,
            store,
            editing: None,
            changed: false,
            rev: 1,
            doc_rev: 1,
            view_scale: 1.,
            mouse: false,
            copied: None,
            asked_from: Mode::Board,
            saved: None,
            grid: false,
            keyboard: TextEntry { shift: true, ..TextEntry::new("", MAX_TEXT) },
            recent_at: 0,
            under: 0,
            copy_to: 0,
            help: 0,
            scroll: 0,
            start: 0,
            from_other: false,
            sources: Vec::new(),
            pasting: false,
            undo: Vec::new(),
            redo: Vec::new(),
            live: None,
            gesture: false,
            burst: None,
            drag: None,
            stretch: false,
            prev: Pad::default(),
            dpad: Repeat::default(),
            recent: Vec::new(),
            recent_live: false,
            preview: None,
            add_before: None,
            text_before: None,
            exit_after_save: false,
        }
    }
    /// An editor on a saved Custom Graphic: its sides' designs (sides it holds none for are blank).
    pub fn edit(mut self, number: usize, saved: &Saved) -> Self {
        for side in &mut self.sides {
            side.doc = saved.sides.get(side.zone).and_then(|t| Doc::decode(t).ok()).unwrap_or_default();
        }
        self.editing = Some(number);
        self.layer = self.doc().layers.len().saturating_sub(1);
        self
    }

    pub fn zone(&self) -> &'static str {
        self.sides.get(self.side).map_or("front", |s| s.zone)
    }
    pub fn doc(&self) -> &Doc {
        static BLANK: std::sync::OnceLock<Doc> = std::sync::OnceLock::new();
        self.sides.get(self.side).map_or_else(|| BLANK.get_or_init(Doc::default), |s| &s.doc)
    }
    fn doc_mut(&mut self) -> &mut Doc {
        let side = self.side;
        &mut self.sides[side].doc
    }
    pub fn rect(&self) -> [f32; 4] {
        self.sides.get(self.side).map_or([1., 0., 0., 1.], |s| s.rect)
    }
    pub fn extent(&self) -> Vec2 {
        extent(self.rect())
    }
    pub fn current(&self) -> Option<&Layer> {
        self.doc().layers.get(self.layer)
    }
    fn current_mut(&mut self) -> Option<&mut Layer> {
        let layer = self.layer;
        self.sides.get_mut(self.side)?.doc.layers.get_mut(layer)
    }
    /// L3's Stretch: LS X sets the width, LS Y the height.
    pub fn stretch(&self) -> bool {
        self.stretch
    }
    /// RT is held: the D-pad reorders (the legend says so).
    pub fn reordering(&self) -> bool {
        self.prev.rt
    }
    fn touch(&mut self) {
        self.rev += 1;
    }
    fn edited(&mut self) {
        self.changed = true;
        self.doc_rev += 1;
        self.touch();
    }
    /// A note in the card's info panel for `NOTE` seconds.
    fn say(&mut self, text: &str) {
        self.status = text.into();
        self.status_left = NOTE;
        self.touch();
    }
    /// The note showing, if any.
    pub fn note(&self) -> Option<&str> {
        (!self.status.is_empty()).then_some(self.status.as_str())
    }
    /// The side is full: the retail popup says so (OK goes back to the board).
    fn full(&mut self) {
        self.say(&format!("A side holds at most {MAX_LAYERS} layers"));
        self.ask(Ask::Full);
        self.asked_from = Mode::Board;
    }
    /// Back to the board, unless a popup is asking now.
    fn to_board(&mut self) {
        if !matches!(self.mode, Mode::Popup(_)) {
            self.mode = Mode::Board;
        }
    }
    fn play(&mut self, sound: u64) {
        self.sounds.push(sound);
    }
    /// A mouse burst starts (`true`) or ends: its changes are one undo step.
    pub fn burst(&mut self, on: bool) {
        self.burst = on.then_some(false);
    }
    /// The designs the skater wears: each side's, or none for a blank side. Empty text is left out.
    pub fn designs(&self) -> Vec<(&'static str, Option<String>)> {
        self.sides.iter().map(|s| (s.zone, design(&s.doc))).collect()
    }
    /// What Save keeps: every side that holds something.
    pub fn to_save(&self) -> Saved {
        Saved { sides: self.designs().into_iter().filter_map(|(zone, text)| Some((zone.to_owned(), text?))).collect() }
    }
    /// The layer's name: its shape, logo or game art, or its text.
    pub fn art_name(&self, layer: &Layer) -> String {
        match &layer.art {
            Art::Shape(s) => SHAPES.get(*s as usize).copied().unwrap_or("Shape").to_owned(),
            Art::Logo(key) => self.catalog.logos.iter().find(|l| l.0 == *key).map_or("Logo".into(), |l| l.1.clone()),
            Art::GameArt(key) => self.catalog.art.iter().find(|l| l.0 == *key).map_or("Skate Art".into(), |l| l.1.clone()),
            Art::Text(t) if t.is_empty() => "Text".into(),
            Art::Text(t) => format!("\"{t}\""),
        }
    }

    // ---- history ----

    fn snapshot(&self) -> Snapshot {
        Snapshot { docs: self.sides.iter().map(|s| s.doc.clone()).collect(), side: self.side, layer: self.layer }
    }
    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.last() != Some(&snapshot) {
            self.undo.push(snapshot);
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }
    /// Take an undo step before a change.
    pub fn checkpoint(&mut self) {
        self.live = None;
        match self.burst {
            Some(true) => return,
            Some(false) => self.burst = Some(true),
            None => {}
        }
        let snapshot = self.snapshot();
        self.push_undo(snapshot);
    }
    fn restore(&mut self, snapshot: Snapshot) {
        for (side, doc) in self.sides.iter_mut().zip(snapshot.docs) {
            side.doc = doc;
        }
        self.side = snapshot.side.min(self.sides.len().saturating_sub(1));
        self.layer = snapshot.layer.min(self.doc().layers.len().saturating_sub(1));
        self.live = None;
        self.edited();
    }
    pub fn undo(&mut self) {
        // A burst going on takes a fresh step for what follows the undo.
        self.burst = self.burst.map(|_| false);
        let now = self.snapshot();
        while let Some(before) = self.undo.pop() {
            if before.docs != now.docs {
                self.redo.push(now);
                return self.restore(before);
            }
        }
        self.say("Nothing to undo");
    }
    pub fn redo(&mut self) {
        self.burst = self.burst.map(|_| false);
        let now = self.snapshot();
        while let Some(after) = self.redo.pop() {
            if after.docs != now.docs {
                self.undo.push(now);
                return self.restore(after);
            }
        }
        self.say("Nothing to redo");
    }
    #[cfg(test)]
    pub fn undo_steps(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    // ---- layers ----

    pub fn select(&mut self, layer: usize) {
        if layer < self.doc().layers.len() && layer != self.layer {
            self.layer = layer;
            self.live = None;
            self.touch();
        }
    }
    /// The next side, wrapping within the item's sides.
    pub fn change_side(&mut self, step: i32) {
        let count = self.sides.len() as i32;
        if count > 1 {
            self.side = (self.side as i32 + step).rem_euclid(count) as usize;
            self.layer = self.doc().layers.len().saturating_sub(1);
            self.live = None;
            self.drag = None;
            self.touch();
        }
    }
    pub fn set_side(&mut self, side: usize) {
        if side < self.sides.len() && side != self.side {
            self.change_side(side as i32 - self.side as i32);
        }
    }
    /// Add a layer on top and select it.
    pub fn add(&mut self, layer: Layer) -> bool {
        if self.doc().layers.len() >= MAX_LAYERS {
            self.full();
            return false;
        }
        self.checkpoint();
        self.doc_mut().layers.push(Layer { grouped: false, ..layer });
        self.layer = self.doc().layers.len() - 1;
        self.edited();
        true
    }
    pub fn duplicate(&mut self) {
        let Some(mut copy) = self.current().cloned() else { return };
        copy.x = (copy.x + 60).min(REACH);
        copy.y = (copy.y + 60).min(REACH);
        self.add(copy);
    }
    /// The layers that move with `layer`: it, the ones grouped onto it above, and the ones it is grouped onto below.
    pub fn group(&self, layer: usize) -> std::ops::RangeInclusive<usize> {
        let layers = &self.doc().layers;
        if layer >= layers.len() {
            return layer..=layer;
        }
        let mut lo = layer;
        while lo > 0 && layers[lo].grouped {
            lo -= 1;
        }
        let mut hi = layer;
        while hi + 1 < layers.len() && layers[hi + 1].grouped {
            hi += 1;
        }
        lo..=hi
    }
    /// The bottom layer is never grouped (nothing is below it).
    fn tidy_groups(&mut self) {
        if let Some(bottom) = self.doc_mut().layers.first_mut() {
            bottom.grouped = false;
        }
    }
    pub fn delete(&mut self) {
        let at = self.layer;
        if at >= self.doc().layers.len() {
            return;
        }
        self.checkpoint();
        let layers = &mut self.doc_mut().layers;
        layers.remove(at);
        // The layer that was grouped onto the deleted one is not grouped onto the one below it now.
        if let Some(above) = layers.get_mut(at) {
            above.grouped = false;
        }
        self.tidy_groups();
        self.layer = self.layer.min(self.doc().layers.len().saturating_sub(1));
        self.edited();
    }
    /// Move a layer in the stack: `to` is its new number from the bottom. It leaves its group.
    pub fn reorder(&mut self, from: usize, to: usize) {
        let count = self.doc().layers.len();
        if from >= count || to >= count || from == to {
            return;
        }
        self.checkpoint();
        let layers = &mut self.doc_mut().layers;
        let mut layer = layers.remove(from);
        if let Some(above) = layers.get_mut(from) {
            above.grouped = false;
        }
        layer.grouped = false;
        layers.insert(to, layer);
        if let Some(above) = layers.get_mut(to + 1) {
            above.grouped = false;
        }
        self.tidy_groups();
        self.layer = to;
        self.edited();
    }
    /// Y: the transform a new layer of this art has (colour, art and group stay).
    pub fn reset_transform(&mut self) {
        let Some(l) = self.current().cloned() else { return };
        let fresh = Layer { colour: l.colour, grouped: l.grouped, ..Layer::new(l.art.clone(), l.colour) };
        if fresh == l {
            return;
        }
        self.checkpoint();
        if let Some(slot) = self.current_mut() {
            *slot = fresh;
        }
        self.edited();
    }
    /// Mirror Copy: a copy flipped across the side's centre line, just above the source's group.
    pub fn mirror_copy(&mut self) {
        let Some(source) = self.current().cloned() else { return };
        if self.doc().layers.len() >= MAX_LAYERS {
            return self.full();
        }
        let at = *self.group(self.layer).end() + 1;
        self.checkpoint();
        self.doc_mut().layers.insert(at, mirrored(&source));
        self.layer = at;
        self.edited();
    }
    /// The side across the item from this one, when the item has it.
    fn pair(&self) -> Option<usize> {
        let zone = pair_zone(self.zone())?;
        self.sides.iter().position(|s| s.zone == zone)
    }
    /// The pair copy's row: "#Copy to Back" and so on; none when the item has no side across.
    pub fn pair_name(&self) -> Option<&'static str> {
        Some(match self.sides[self.pair()?].zone {
            "back" => "#Copy to Back",
            "front" => "#Copy to Front",
            "right" => "#Copy to R Sleeve",
            _ => "#Copy to L Sleeve",
        })
    }
    /// Copy the selected layer to the side across (front and back, the sleeves), mirrored, on top.
    pub fn copy_to_pair(&mut self) -> bool {
        let (Some(pair), Some(source)) = (self.pair(), self.current().cloned()) else { return false };
        if self.sides[pair].doc.layers.len() >= MAX_LAYERS {
            self.full();
            return false;
        }
        self.checkpoint();
        self.sides[pair].doc.layers.push(mirrored(&source));
        self.edited();
        self.say(&format!("Copied to {}", side_name(self.sides[pair].zone)));
        true
    }
    /// Menu Copy Side to...: the target side becomes a copy of this one (one undo step).
    pub fn copy_side_to(&mut self, side: usize) {
        if side >= self.sides.len() || side == self.side {
            return;
        }
        self.checkpoint();
        let doc = self.doc().clone();
        self.sides[side].doc = doc;
        self.edited();
        self.say(&format!("Copied to {}", side_name(self.sides[side].zone)));
    }
    /// The sides Copy Side to... offers: every other one.
    pub fn copy_targets(&self) -> Vec<usize> {
        (0..self.sides.len()).filter(|s| *s != self.side).collect()
    }
    /// Group with below: the selected layer moves, scales and recolours with the one under it.
    pub fn toggle_group(&mut self) {
        if self.layer == 0 || self.current().is_none() {
            return self.say("Nothing below to group with");
        }
        self.checkpoint();
        if let Some(l) = self.current_mut() {
            l.grouped = !l.grouped;
        }
        self.edited();
    }
    /// The layers below `layer` its box overlaps, nearest first (Layers under here).
    pub fn layers_under(&self, layer: usize) -> Vec<usize> {
        let layers = &self.doc().layers;
        let Some(top) = layers.get(layer) else { return Vec::new() };
        let bounds = |l: &Layer| corners(l).iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), c| (lo.min(*c), hi.max(*c)));
        let (lo, hi) = bounds(top);
        (0..layer)
            .rev()
            .filter(|i| {
                let (a, b) = bounds(&layers[*i]);
                a.x < hi.x && b.x > lo.x && a.y < hi.y && b.y > lo.y
            })
            .collect()
    }

    // ---- colour ----

    /// Colour every colourable layer of the selected one's group (stock art keeps its own colours).
    fn recolour(&mut self, colour: [u8; 3]) {
        let group = self.group(self.layer);
        let layers = &mut self.doc_mut().layers;
        for l in &mut layers[group] {
            if !matches!(l.art, Art::Logo(_) | Art::GameArt(_)) {
                l.colour = [colour[0], colour[1], colour[2], l.colour[3]];
            }
        }
    }
    fn colourable(&self) -> bool {
        self.current().is_some_and(|l| !matches!(l.art, Art::Logo(_) | Art::GameArt(_)))
    }
    fn remember(&mut self, colour: [u8; 3], live: bool) {
        if live && self.recent_live && !self.recent.is_empty() {
            self.recent[0] = colour;
        } else {
            self.recent.retain(|c| *c != colour);
            self.recent.insert(0, colour);
            self.recent.truncate(RECENT);
        }
        self.recent_live = live;
    }
    /// A palette swatch on the layer (and its group).
    pub fn set_swatch(&mut self, index: usize) {
        let Some(colour) = PALETTE.get(index).copied() else { return };
        if !self.colourable() {
            return;
        }
        self.checkpoint();
        self.recolour(colour);
        self.remember(colour, false);
        self.edited();
    }
    fn set_hsl(&mut self, f: impl Fn((f32, f32, f32)) -> (f32, f32, f32)) {
        let Some(l) = self.current().filter(|_| self.colourable()) else { return };
        let (h, s, light) = f(hsl([l.colour[0], l.colour[1], l.colour[2]]));
        let colour = rgb(h, s, light);
        self.checkpoint();
        self.recolour(colour);
        self.remember(colour, true);
        self.edited();
    }
    /// Hue, 0-359 degrees (a grey takes full saturation so the hue shows once it has a shade).
    pub fn set_hue(&mut self, degrees: u16) {
        let h = f32::from(degrees % 360);
        self.set_hsl(|(_, s, l)| (h, if s < 0.01 { 1. } else { s }, l));
    }
    /// Shade: HSL lightness, 0-100.
    pub fn set_shade(&mut self, percent: u8) {
        let l = f32::from(percent.min(100)) / 100.;
        self.set_hsl(|(h, s, _)| (h, s, l));
    }
    /// Opacity, 0-100%, on the layer and its group.
    pub fn set_opacity(&mut self, percent: u8) {
        if self.current().is_none() {
            return;
        }
        self.checkpoint();
        let alpha = alpha_of(i32::from(percent));
        let group = self.group(self.layer);
        for l in &mut self.doc_mut().layers[group] {
            l.colour[3] = alpha;
        }
        self.edited();
    }
    pub fn hue(&self) -> u16 {
        self.current().map_or(0, |l| hsl([l.colour[0], l.colour[1], l.colour[2]]).0.round() as u16 % 360)
    }
    pub fn shade(&self) -> u8 {
        self.current().map_or(0, |l| (hsl([l.colour[0], l.colour[1], l.colour[2]]).2 * 100.).round() as u8)
    }
    pub fn opacity(&self) -> u8 {
        self.current().map_or(100, |l| opacity_percent(l.colour[3]) as u8)
    }
    /// Recent colours, newest first (up to 8, this session).
    pub fn recent_colours(&self) -> &[[u8; 3]] {
        &self.recent
    }

    // ---- the Add picker ----

    pub fn picker_len(&self) -> usize {
        self.tab_len(self.picker.tab)
    }
    pub fn tab_len(&self, tab: Tab) -> usize {
        match tab {
            Tab::Shapes => SHAPES.len(),
            Tab::Logos => self.catalog.logos.len(),
            Tab::GameArt => self.catalog.art.len(),
            Tab::Text => 1,
        }
    }
    fn tab_slot(&self) -> usize {
        TABS.iter().position(|t| *t == self.picker.tab).unwrap_or(0)
    }
    pub fn picker_index(&self) -> usize {
        self.picker.index[self.tab_slot()]
    }
    /// A: the Add grid, with its highlighted piece previewed on the board.
    pub fn open_add(&mut self) {
        self.mode = Mode::Add;
        self.add_before = Some(self.snapshot());
        self.preview(self.picker_index());
        self.touch();
    }
    pub fn set_tab(&mut self, step: i32) {
        let at = self.tab_slot() as i32;
        self.picker.tab = TABS[(at + step).rem_euclid(TABS.len() as i32) as usize];
        self.preview(self.picker_index());
        self.touch();
    }
    /// The piece a picker tile adds, as a new layer (white, at the side's middle).
    fn piece(&self, index: usize) -> Option<Layer> {
        Some(match self.picker.tab {
            Tab::Shapes if index < SHAPES.len() => Layer::new(Art::Shape(index as u8), [255; 4]),
            Tab::Logos => Layer::new(Art::Logo(self.catalog.logos.get(index)?.0), [255; 4]),
            Tab::GameArt => Layer::new(Art::GameArt(self.catalog.art.get(index)?.0), [255; 4]),
            _ => return None,
        })
    }
    /// Highlight a tile: its piece shows on the board in place of the last one previewed.
    pub fn preview(&mut self, index: usize) {
        let slot = self.tab_slot();
        self.picker.index[slot] = index.min(self.picker_len().saturating_sub(1));
        self.drop_preview();
        if let Some(layer) = self.piece(self.picker_index()) {
            if self.doc().layers.len() < MAX_LAYERS {
                self.doc_mut().layers.push(layer);
                self.layer = self.doc().layers.len() - 1;
                self.preview = Some(self.layer);
            }
        }
        self.doc_rev += 1;
        self.touch();
    }
    fn drop_preview(&mut self) {
        if let Some(at) = self.preview.take() {
            if at < self.doc().layers.len() {
                self.doc_mut().layers.remove(at);
            }
            if let Some(before) = &self.add_before {
                self.layer = before.layer.min(self.doc().layers.len().saturating_sub(1));
            }
        }
    }
    /// A: keep the previewed piece (one undo step), or start a text layer.
    pub fn keep(&mut self) {
        if self.picker.tab == Tab::Text {
            self.drop_preview();
            self.add_before = None;
            if self.add(Layer { w: 400, ..Layer::new(Art::Text(String::new()), [255; 4]) }) {
                self.text_before = None;
                self.start_keyboard(String::new());
            } else {
                self.to_board();
            }
            return;
        }
        if self.preview.take().is_some() {
            if let Some(before) = self.add_before.take() {
                self.push_undo(before);
            }
            self.edited();
        } else if self.doc().layers.len() >= MAX_LAYERS {
            self.full();
        }
        self.add_before = None;
        self.to_board();
        self.touch();
    }
    /// B: the preview comes off.
    pub fn cancel_add(&mut self) {
        self.drop_preview();
        self.add_before = None;
        self.mode = Mode::Board;
        self.doc_rev += 1;
        self.touch();
    }
    /// The name of a picker tile.
    pub fn tile_name(&self, tab: Tab, index: usize) -> String {
        match tab {
            Tab::Shapes => SHAPES.get(index).map_or(String::new(), |n| (*n).to_owned()),
            Tab::Logos => self.catalog.logos.get(index).map_or(String::new(), |l| l.1.clone()),
            Tab::GameArt => self.catalog.art.get(index).map_or(String::new(), |l| l.1.clone()),
            Tab::Text => "Add Text".into(),
        }
    }

    // ---- Options ----

    /// The selected layer's Options rows.
    pub fn options(&self) -> Vec<Opt> {
        let Some(l) = self.current() else { return Vec::new() };
        // The common ones first, on the same rows for every kind of layer: its colour (stock art keeps
        // its own: its opacity), then Mirror Copy (one Down) and Copy to Back (two).
        let colourable = self.colourable();
        let mut rows = vec![if colourable { Opt::Colour } else { Opt::Opacity }, Opt::MirrorCopy];
        if self.pair_name().is_some() {
            rows.push(Opt::PairCopy);
        }
        if matches!(l.art, Art::Text(_)) {
            rows.push(Opt::Text);
        }
        if colourable {
            if !self.recent.is_empty() {
                rows.push(Opt::Recent);
            }
            rows.extend([Opt::Hue, Opt::Shade, Opt::Opacity]);
        }
        rows.extend([Opt::Flip, Opt::Duplicate, Opt::MoveUp, Opt::MoveDown]);
        if self.layer > 0 {
            rows.push(Opt::Group);
        }
        if !self.layers_under(self.layer).is_empty() {
            rows.push(Opt::LayersUnder);
        }
        rows.push(Opt::Delete);
        rows
    }
    pub fn option_label(&self, opt: Opt) -> String {
        match opt {
            Opt::Colour => "#Colour",
            Opt::Recent => "#Recent",
            Opt::Hue => "#Hue",
            Opt::Shade => "#Shade",
            Opt::Opacity => "#Opacity",
            Opt::Text => "#Edit Text",
            Opt::MirrorCopy => "#Mirror Copy",
            Opt::PairCopy => self.pair_name().unwrap_or("#Copy"),
            Opt::Flip => "#Flip",
            Opt::Duplicate => "#Duplicate",
            Opt::MoveUp => "#Move Up",
            Opt::MoveDown => "#Move Down",
            Opt::Group => "#Group with below",
            Opt::LayersUnder => "#Layers under here",
            Opt::Delete => "#Delete",
        }
        .to_owned()
    }
    /// Left or right on an Options row (`direction` -1 or 1; `fine` LT held).
    pub fn adjust_option(&mut self, opt: Opt, direction: i32, fine: bool) {
        let step = |coarse: i32| direction * if fine { 1 } else { coarse };
        match opt {
            Opt::Colour => {
                let at = self.current().map_or(0, |l| palette_index(l.colour)) as i32;
                self.set_swatch((at + direction).rem_euclid(PALETTE.len() as i32) as usize);
            }
            Opt::Recent if !self.recent.is_empty() => {
                self.recent_at = (self.recent_at as i32 + direction).rem_euclid(self.recent.len() as i32) as usize;
                let colour = self.recent[self.recent_at];
                if self.colourable() {
                    self.checkpoint();
                    self.recolour(colour);
                    self.edited();
                }
            }
            Opt::Hue => self.set_hue((i32::from(self.hue()) + step(10)).rem_euclid(360) as u16),
            Opt::Shade => self.set_shade((i32::from(self.shade()) + step(5)).clamp(0, 100) as u8),
            Opt::Opacity => self.set_opacity((i32::from(self.opacity()) + step(10)).clamp(0, 100) as u8),
            Opt::Flip => {
                let Some(l) = self.current() else { return };
                // None, Horizontal, Vertical, Both.
                let at = i32::from(l.flip_x) | i32::from(l.flip_y) << 1;
                let next = (at + direction).rem_euclid(4);
                self.checkpoint();
                if let Some(l) = self.current_mut() {
                    (l.flip_x, l.flip_y) = (next & 1 != 0, next & 2 != 0);
                }
                self.edited();
            }
            Opt::Group => self.toggle_group(),
            Opt::LayersUnder => {
                let count = self.layers_under(self.layer).len().max(1) as i32;
                self.under = (self.under as i32 + direction).rem_euclid(count) as usize;
                self.touch();
            }
            _ => {}
        }
    }
    /// A on an Options row: actions run; value rows step forward.
    pub fn activate_option(&mut self, opt: Opt) {
        self.recent_live = false;
        match opt {
            Opt::Text => {
                if let Some(Layer { art: Art::Text(t), .. }) = self.current() {
                    let before = t.clone();
                    self.checkpoint();
                    self.text_before = Some(before.clone());
                    self.start_keyboard(before);
                }
            }
            Opt::MirrorCopy => {
                self.mirror_copy();
                self.to_board();
            }
            Opt::PairCopy => {
                self.copy_to_pair();
                self.to_board();
            }
            Opt::Duplicate => {
                self.duplicate();
                self.to_board();
            }
            Opt::MoveUp => self.reorder(self.layer, self.layer + 1),
            Opt::MoveDown if self.layer > 0 => self.reorder(self.layer, self.layer - 1),
            Opt::MoveDown => {}
            Opt::LayersUnder => {
                if let Some(&layer) = self.layers_under(self.layer).get(self.under) {
                    self.select(layer);
                    self.mode = Mode::Board;
                }
                self.under = 0;
            }
            Opt::Delete => self.ask(Ask::Delete),
            _ => self.adjust_option(opt, 1, false),
        }
        self.touch();
    }
    /// An Options row's value: (0-10 bars or a swatch index, the text shown).
    pub fn option_value(&self, opt: Opt) -> (f64, String) {
        let l = self.current();
        let bars = |v: f64, max: f64| (v / max * 10.).round();
        match opt {
            Opt::Colour => (l.map_or(0, |l| palette_index(l.colour)) as f64, String::new()),
            Opt::Recent => (self.recent_at.min(self.recent.len().saturating_sub(1)) as f64, String::new()),
            Opt::Hue => (bars(f64::from(self.hue()), 359.), format!("#{}", self.hue())),
            Opt::Shade => (bars(f64::from(self.shade()), 100.), format!("#{}", self.shade())),
            Opt::Opacity => (bars(f64::from(self.opacity()), 100.), format!("#{}%", self.opacity())),
            Opt::Flip => {
                let names = ["#Off", "#Horizontal", "#Vertical", "#Both"];
                (0., names[l.map_or(0, |l| usize::from(l.flip_x) | usize::from(l.flip_y) << 1)].into())
            }
            Opt::Group => (0., if l.is_some_and(|l| l.grouped) { "#On" } else { "#Off" }.into()),
            Opt::LayersUnder => {
                let under = self.layers_under(self.layer);
                let name = under.get(self.under.min(under.len().saturating_sub(1))).map_or(String::new(), |i| self.art_name(&self.doc().layers[*i]));
                (0., format!("#{name}"))
            }
            _ => (0., String::new()),
        }
    }

    // ---- the Menu ----

    pub fn menu_rows(&self) -> Vec<MenuRow> {
        // Save as New comes after Copy Side to..., so the rows above it stay put once a save exists.
        let mut rows = vec![MenuRow::Save, MenuRow::Redo];
        if self.sides.len() > 1 {
            rows.push(MenuRow::CopySide);
        }
        if self.editing.is_some() {
            rows.push(MenuRow::SaveNew);
        }
        rows.extend([MenuRow::ClearSide, MenuRow::Background, MenuRow::BackgroundOpacity, MenuRow::Grid, MenuRow::Help]);
        rows
    }
    pub fn menu_label(&self, row: MenuRow) -> &'static str {
        match row {
            MenuRow::Save => "#Save",
            MenuRow::SaveNew => "#Save as New",
            MenuRow::Redo => "#Redo",
            MenuRow::CopySide => "#Copy Side to...",
            MenuRow::ClearSide => "#Clear Side",
            MenuRow::Background => "#Background",
            MenuRow::BackgroundOpacity => "#Background Opacity",
            MenuRow::Grid => "#Grid",
            MenuRow::Help => "#Help",
        }
    }
    /// A Menu row's value: (0-10 bars or a swatch index, the text shown).
    pub fn menu_value(&self, row: MenuRow) -> (f64, String) {
        let background = self.doc().background;
        match row {
            MenuRow::CopySide => {
                let targets = self.copy_targets();
                let side = targets.get(self.copy_to.min(targets.len().saturating_sub(1))).copied().unwrap_or(0);
                (0., format!("#{}", side_name(self.sides.get(side).map_or("", |s| s.zone))))
            }
            MenuRow::Background => (palette_index(background) as f64, String::new()),
            MenuRow::BackgroundOpacity => {
                let percent = opacity_percent(background[3]);
                ((f64::from(percent) / 10.).round(), format!("#{percent}%"))
            }
            MenuRow::Grid => (0., if self.grid { "#On" } else { "#Off" }.into()),
            MenuRow::Help => (0., ["#Pad", "#Mouse", "#Keys"][self.help % 3].into()),
            _ => (0., String::new()),
        }
    }
    pub fn adjust_menu(&mut self, row: MenuRow, direction: i32, fine: bool) {
        match row {
            MenuRow::CopySide => {
                let count = self.copy_targets().len().max(1) as i32;
                self.copy_to = (self.copy_to as i32 + direction).rem_euclid(count) as usize;
                self.touch();
            }
            MenuRow::Background => {
                self.checkpoint();
                let background = &mut self.doc_mut().background;
                let [r, g, b] = PALETTE[(palette_index(*background) as i32 + direction).rem_euclid(PALETTE.len() as i32) as usize];
                *background = [r, g, b, if background[3] == 0 { 255 } else { background[3] }];
                self.edited();
            }
            MenuRow::BackgroundOpacity => {
                self.checkpoint();
                let background = &mut self.doc_mut().background;
                background[3] = alpha_of(opacity_percent(background[3]) as i32 + direction * if fine { 1 } else { 10 });
                self.edited();
            }
            MenuRow::Grid => {
                self.grid = !self.grid;
                self.touch();
            }
            MenuRow::Help => {
                self.help = (self.help as i32 + direction).rem_euclid(3) as usize;
                self.touch();
            }
            _ => {}
        }
    }
    /// A on a Menu row.
    pub fn activate_menu(&mut self, row: MenuRow) -> Option<Exit> {
        match row {
            MenuRow::Save | MenuRow::SaveNew => {
                self.mode = Mode::Board;
                self.save(row == MenuRow::SaveNew);
            }
            MenuRow::Redo => {
                self.mode = Mode::Board;
                self.redo();
            }
            MenuRow::CopySide => {
                if let Some(&side) = self.copy_targets().get(self.copy_to) {
                    self.copy_side_to(side);
                }
                self.mode = Mode::Board;
            }
            MenuRow::ClearSide => {
                if !self.doc().layers.is_empty() || self.doc().background[3] > 0 {
                    self.ask(Ask::ClearSide);
                } else {
                    self.mode = Mode::Board;
                }
            }
            _ => self.adjust_menu(row, 1, false),
        }
        self.touch();
        None
    }

    // ---- popups and leaving ----

    fn ask(&mut self, ask: Ask) {
        self.end_gesture();
        self.asked_from = self.mode;
        self.mode = Mode::Popup(ask);
        self.popup = 0;
        self.touch();
    }
    /// The Done popup.
    pub fn ask_done(&mut self) {
        self.ask(Ask::Done);
    }
    /// B on the board. Not retail (user 10-19): with nothing unsaved it just leaves, keeping what is worn (a
    /// save, or the one being edited) or, on a New left untouched, putting the skater back; else the popup asks.
    pub fn leave(&mut self) -> Option<Exit> {
        if self.changed {
            self.ask_done();
            return None;
        }
        Some(if self.editing.is_some() { Exit::Done } else { Exit::Discard })
    }
    /// The screen could not write the save to disk: the popup says so and the editor stays open.
    pub fn save_failed(&mut self, error: &str) {
        self.say(&format!("Could not save: {error}"));
        self.ask(Ask::SaveFailed);
    }
    /// LT is held (the last pad state): Left / Right on a row step finely.
    pub fn fine(&self) -> bool {
        self.prev.lt
    }
    /// B on Options: back to the board.
    pub fn close_options(&mut self) {
        self.recent_live = false;
        self.mode = Mode::Board;
        self.touch();
    }
    /// B on the replace grid: nothing is written.
    pub fn cancel_replace(&mut self) {
        self.exit_after_save = false;
        self.mode = Mode::Board;
        self.touch();
    }
    /// The Done popup's choice.
    pub fn done(&mut self, choice: DoneChoice) -> Option<Exit> {
        self.mode = Mode::Board;
        self.touch();
        match choice {
            DoneChoice::Save => {
                self.save(false);
                if self.mode == Mode::Replace {
                    self.exit_after_save = true;
                    None
                } else {
                    self.saved.is_some().then_some(Exit::Done)
                }
            }
            DoneChoice::KeepWithoutSaving => Some(Exit::Done),
            DoneChoice::Discard => Some(Exit::Discard),
            DoneChoice::Cancel => None,
        }
    }
    /// A popup's row chosen (`None`: B, which cancels).
    pub fn answer(&mut self, row: Option<usize>) -> Option<Exit> {
        let Mode::Popup(asked) = self.mode else { return None };
        self.mode = Mode::Board;
        self.touch();
        match (asked, row) {
            (Ask::Done, Some(row)) => self.done(DONE_CHOICES.get(row).copied().unwrap_or(DoneChoice::Cancel)),
            (Ask::Delete, Some(0)) => {
                self.delete();
                None
            }
            (Ask::ClearSide, Some(0)) => {
                self.checkpoint();
                *self.doc_mut() = Doc::default();
                self.layer = 0;
                self.edited();
                None
            }
            // No, B, or OK on a notice: back where it was asked from.
            (Ask::Delete | Ask::ClearSide | Ask::SaveFailed | Ask::Full, _) => {
                self.mode = self.asked_from;
                None
            }
            (Ask::Code(_), _) => {
                self.mode = Mode::StartFrom;
                None
            }
            _ => None,
        }
    }

    // ---- New: Start From ----

    /// New opens on Start From. `sources`: other items' saves; those with a side this item has are offered.
    pub fn start_new(&mut self, sources: Vec<Source>) {
        self.sources = sources.into_iter().filter(|s| self.sides.iter().any(|d| s.saved.sides.contains_key(d.zone))).collect();
        (self.mode, self.start, self.from_other) = (Mode::StartFrom, 0, false);
        self.touch();
    }
    /// A on Start From row `i`: Blank, a layout, From Another Item (its list), Paste Code, or one of
    /// the other items' saves.
    pub fn start_from(&mut self, i: usize) {
        self.start = i;
        if !self.from_other && i == LAYOUTS.len() + 2 {
            self.pasting = true;
            return;
        }
        if self.from_other {
            let Some(saved) = self.sources.get(i).map(|s| s.saved.clone()) else { return };
            for side in &mut self.sides {
                if let Some(doc) = saved.sides.get(side.zone).and_then(|t| Doc::decode(t).ok()) {
                    side.doc = doc;
                }
            }
        } else if i == LAYOUTS.len() + 1 {
            if self.sources.is_empty() {
                return self.say("No other item has a Custom Graphic for these sides");
            }
            (self.from_other, self.start) = (true, 0);
            return self.touch();
        } else if i > 0 {
            for side in &mut self.sides {
                side.doc.layers = layout(i - 1, side.zone, extent(side.rect));
            }
            // An item without the layout's sides gets it on the side being edited.
            if self.sides.iter().all(|s| s.doc.layers.is_empty()) {
                let primary = ["front", "back", "left"][i - 1];
                let size = self.extent();
                self.doc_mut().layers = layout(i - 1, primary, size);
            }
        }
        self.changed = self.sides.iter().any(|s| !s.doc.layers.is_empty());
        self.mode = Mode::Board;
        self.layer = self.doc().layers.len().saturating_sub(1);
        self.doc_rev += 1;
        self.touch();
    }
    /// A share code (`share_code`) pasted on Start From: its sides go on the item and into a new save
    /// (the replace list when the item is full); a code that fails asks the retail popup.
    pub fn paste(&mut self, code: &str) {
        let catalog = &self.catalog;
        let known = |id: u64| catalog.logos.iter().any(|l| l.0 == id) || catalog.art.iter().any(|a| a.0 == id);
        match crate::share_code::decode(code, &self.item, known) {
            Ok(saved) => {
                for side in &mut self.sides {
                    side.doc = saved.sides.get(side.zone).and_then(|t| Doc::decode(t).ok()).unwrap_or_default();
                }
                self.mode = Mode::Board;
                self.layer = self.doc().layers.len().saturating_sub(1);
                (self.changed, self.doc_rev) = (true, self.doc_rev + 1);
                self.save(true);
            }
            Err(error) => {
                (self.mode, self.popup) = (Mode::Popup(Ask::Code(matches!(error, crate::share_code::CodeError::OtherItem(_)))), 0);
                self.touch();
            }
        }
    }
    /// B on Start From: out of the other items' list, or out of New altogether.
    pub fn start_back(&mut self) -> Option<Exit> {
        if !self.from_other {
            return Some(Exit::Discard);
        }
        (self.from_other, self.start) = (false, LAYOUTS.len() + 1);
        self.touch();
        None
    }

    // ---- saving ----

    /// Save the whole item into its list: over the save being edited, or as a new one.
    pub fn save(&mut self, as_new: bool) {
        let saved = self.to_save();
        if saved.sides.is_empty() {
            return self.say("Add a layer before saving");
        }
        let overwrite = self.editing.filter(|_| !as_new);
        let result = match overwrite {
            Some(n) if self.store.replace(&self.item, n, saved.clone()) => Ok(n),
            _ => self.store.add(&self.item, saved.clone()),
        };
        match result {
            Ok(n) => self.wrote(n, saved),
            Err(Full) => {
                self.say(&format!("{} holds {} Custom Graphics: pick one to replace.", if self.name.is_empty() { "This item" } else { &self.name }, crate::graphic_store::MAX_SAVES));
                self.mode = Mode::Replace;
                self.replace = self.editing.unwrap_or(0);
            }
        }
        self.touch();
    }
    /// Write over save number `n` of the item's list.
    pub fn replace_save(&mut self, n: usize) -> Option<Exit> {
        let saved = self.to_save();
        self.mode = Mode::Board;
        self.touch();
        if saved.sides.is_empty() || !self.store.replace(&self.item, n, saved.clone()) {
            self.exit_after_save = false;
            return None;
        }
        self.wrote(n, saved);
        std::mem::take(&mut self.exit_after_save).then_some(Exit::Done)
    }
    fn wrote(&mut self, n: usize, saved: Saved) {
        self.editing = Some(n);
        self.changed = false;
        self.say(&format!("Saved to Custom Graphics ({} of {})", n + 1, crate::graphic_store::MAX_SAVES));
        self.saved = Some(saved);
    }
    pub fn take_saved(&mut self) -> Option<Saved> {
        self.saved.take()
    }

    // ---- the pad ----

    /// This frame's presses from the held state: edges, and the D-pad's repeat.
    fn presses(&mut self, pad: &Pad) -> Presses {
        let prev = self.prev;
        let edge = |now: bool, before: bool| now && !before;
        let dpad = self.dpad.step([pad.up, pad.down, pad.left, pad.right], pad.dt);
        self.prev = *pad;
        Presses {
            up: dpad[0],
            down: dpad[1],
            left: dpad[2],
            right: dpad[3],
            a: edge(pad.a, prev.a),
            b: edge(pad.b, prev.b),
            x: edge(pad.x, prev.x),
            y: edge(pad.y, prev.y),
            back: edge(pad.back, prev.back),
            start: edge(pad.start, prev.start),
            l3: edge(pad.l3, prev.l3),
            lb: edge(pad.lb, prev.lb),
            rb: edge(pad.rb, prev.rb),
        }
    }
    pub fn input(&mut self, pad: &Pad, typed: &[Typed]) -> Option<Exit> {
        let p = self.presses(pad);
        let before = (self.mode, self.layer, self.side);
        if self.status_left > 0. {
            self.status_left -= pad.dt;
            if self.status_left <= 0. {
                self.status.clear();
                self.touch();
            }
        }
        // The sticks edit the layer on the board and on its Options (a gesture runs on there).
        if !matches!(self.mode, Mode::Board | Mode::Options) {
            self.end_gesture();
        }
        let exit = match self.mode {
            Mode::Board => self.board_input(pad, &p),
            Mode::Add => {
                self.add_input(pad, &p);
                None
            }
            Mode::Options => {
                self.options_input(pad, &p);
                None
            }
            Mode::Menu => self.menu_input(pad, &p),
            Mode::Text => {
                self.text_input(&p, typed);
                None
            }
            Mode::Popup(_) => self.popup_input(&p),
            // The page's D-pad, A and B (`graphic_pages::apply`).
            Mode::StartFrom => None,
            Mode::Replace => self.replace_input(&p),
        };
        if before != (self.mode, self.layer, self.side) {
            self.touch();
        }
        exit
    }
    fn board_input(&mut self, pad: &Pad, p: &Presses) -> Option<Exit> {
        if p.l3 {
            self.stretch = !self.stretch;
            self.play(SOUND_A_BUTTON);
            self.touch();
        }
        if p.back {
            self.play(SOUND_B_BUTTON);
            self.undo();
        }
        if p.a {
            self.end_gesture();
            self.open_add();
            return None;
        }
        if p.x {
            self.end_gesture();
            if self.current().is_some() {
                self.mode = Mode::Options;
                self.option = 0;
                self.status.clear();
                self.play(SOUND_A_BUTTON);
            } else {
                self.say("Press A to add a piece first");
            }
            return None;
        }
        if p.start {
            self.end_gesture();
            self.mode = Mode::Menu;
            self.menu = 0;
            self.play(SOUND_A_BUTTON);
            return None;
        }
        if p.b {
            self.end_gesture();
            return self.leave();
        }
        if p.y {
            self.end_gesture();
            self.play(fe_sound_id("core_y_button"));
            self.reset_transform();
        }
        // Up is toward the top of the stack, as the list shows it; RT held moves the layer instead.
        let count = self.doc().layers.len();
        for (pressed, step) in [(p.up, 1i32), (p.down, -1)] {
            let to = self.layer as i32 + step;
            if pressed && (0..count as i32).contains(&to) {
                self.end_gesture();
                if pad.rt {
                    self.play(if step > 0 { SOUND_NAV_UP } else { SOUND_NAV_DOWN });
                    self.reorder(self.layer, to as usize);
                } else {
                    self.select(to as usize);
                }
            }
        }
        if (p.left || p.right) && self.sides.len() > 1 {
            self.end_gesture();
            self.play(nav_side(p.right));
            self.change_side(if p.right { 1 } else { -1 });
        }
        // LT held: the bumpers turn one degree a press.
        if pad.lt && (p.lb || p.rb) {
            self.end_gesture();
            self.turn_by(if p.rb { 1. } else { -1. });
        }
        self.sticks(pad);
        None
    }
    fn end_gesture(&mut self) {
        self.gesture = false;
        self.live = None;
    }
    /// The group as it is now, for a gesture to start from.
    fn begin_live(&self) -> Option<Live> {
        let l = self.current()?;
        let group = self.group(self.layer);
        Some(Live {
            side: self.side,
            layer: self.layer,
            x: l.x as f32,
            y: l.y as f32,
            w: l.w as f32,
            h: l.h as f32,
            angle: l.angle as f32,
            start: group.map(|i| (i, self.doc().layers[i].clone())).collect(),
            hold: 0.,
        })
    }
    fn turn_by(&mut self, degrees: f32) {
        let Some(mut live) = self.begin_live() else { return };
        self.checkpoint();
        live.angle = wrap_angle(live.angle + degrees);
        self.apply_live(&live);
    }
    /// RS moves, LS scales (each axis apart with Stretch), the held bumpers turn with a stop at every
    /// 45 degrees; LT makes it all fine. One gesture is one undo step.
    fn sticks(&mut self, pad: &Pad) {
        let dead = |v: f32| {
            let m = v.abs();
            if m < DEADZONE { 0. } else { v.signum() * (m - DEADZONE) / (1. - DEADZONE) }
        };
        let curve = |v: f32| v * v.abs();
        let (ls, rs) = (Vec2::new(curve(dead(pad.ls.x)), curve(dead(pad.ls.y))), Vec2::new(curve(dead(pad.rs.x)), curve(dead(pad.rs.y))));
        let turn = if pad.lt { 0. } else { f32::from(u8::from(pad.rb)) - f32::from(u8::from(pad.lb)) };
        if ls == Vec2::ZERO && rs == Vec2::ZERO && turn == 0. || self.current().is_none() {
            return self.end_gesture();
        }
        let (side, layer) = (self.side, self.layer);
        let mut live = match self.live.take().filter(|l| l.side == side && l.layer == layer) {
            Some(live) => live,
            None => {
                let live = self.begin_live().expect("a layer");
                if !self.gesture {
                    self.checkpoint();
                    self.gesture = true;
                }
                live
            }
        };
        let rate = if pad.lt { FINE } else { 1. };
        let dt = pad.dt;
        let reach = f32::from(REACH);
        live.x = (live.x + rs.x * MOVE * rate * dt).clamp(-reach, reach);
        live.y = (live.y - rs.y * MOVE * rate * dt).clamp(-reach, reach);
        let (lo, hi) = (SIZES.0 as f32, SIZES.1 as f32);
        if self.stretch {
            live.w = (live.w * (ls.x * GROW * rate * dt).exp()).clamp(lo, hi);
            live.h = (live.h * (ls.y * GROW * rate * dt).exp()).clamp(lo, hi);
        } else {
            let factor = (ls.y * GROW * rate * dt).exp().clamp(lo / live.w.min(live.h), hi / live.w.max(live.h));
            live.w *= factor;
            live.h *= factor;
        }
        if live.hold > 0. {
            live.hold -= dt;
        } else if turn != 0. {
            let next = live.angle + turn * TURN * rate * dt;
            // The first multiple of 45 degrees past the angle, in the turn's direction.
            let stop = if turn > 0. { ((live.angle / 45.).floor() + 1.) * 45. } else { ((live.angle / 45.).ceil() - 1.) * 45. };
            if (turn > 0. && next >= stop) || (turn < 0. && next <= stop) {
                live.angle = stop;
                live.hold = DETENT;
            } else {
                live.angle = next;
            }
        }
        self.apply_live(&live);
        self.live = Some(live);
    }
    /// The selected layer takes the live values; the rest of its group moves, scales and turns with it.
    fn apply_live(&mut self, live: &Live) {
        let reach = f32::from(REACH);
        let (lo, hi) = (SIZES.0 as f32, SIZES.1 as f32);
        let Some((_, base)) = live.start.iter().find(|(i, _)| *i == live.layer).cloned() else { return };
        let (fw, fh) = (live.w / f32::from(base.w), live.h / f32::from(base.h));
        let turn = live.angle - f32::from(base.angle);
        let (s, c) = turn.to_radians().sin_cos();
        let layers = &mut self.sides[live.side].doc.layers;
        for (i, start) in &live.start {
            let Some(l) = layers.get_mut(*i) else { continue };
            let (x, y, w, h, angle) = if *i == live.layer {
                (live.x, live.y, live.w, live.h, live.angle)
            } else {
                let o = Vec2::new(f32::from(start.x) - f32::from(base.x), f32::from(start.y) - f32::from(base.y)) * Vec2::new(fw, fh);
                (live.x + o.x * c - o.y * s, live.y + o.x * s + o.y * c, f32::from(start.w) * fw, f32::from(start.h) * fh, f32::from(start.angle) + turn)
            };
            l.x = x.clamp(-reach, reach).round() as i16;
            l.y = y.clamp(-reach, reach).round() as i16;
            l.w = w.round().clamp(lo, hi) as u16;
            l.h = h.round().clamp(lo, hi) as u16;
            l.angle = wrap_angle(angle).round().clamp(-180., 180.) as i16;
        }
        self.edited();
    }
    fn add_input(&mut self, pad: &Pad, p: &Presses) {
        // Not retail (user 10-19): a stick pushed on the grid places the highlighted piece and moves or sizes it
        // at once, on the board (Forza's vinyl editor: a picked shape is live under the sticks), no A first.
        if self.preview.is_some() && pad.ls.length().max(pad.rs.length()) > PLACE {
            self.play(SOUND_A_BUTTON);
            self.keep();
            self.sticks(pad);
            return;
        }
        if p.lb || p.rb {
            self.play(nav_side(p.rb));
            self.set_tab(if p.rb { 1 } else { -1 });
        }
        for (pressed, step) in [(p.left, -1i32), (p.right, 1), (p.up, -(PICKER_COLS as i32)), (p.down, PICKER_COLS as i32)] {
            let to = self.picker_index() as i32 + step;
            if pressed && (0..self.picker_len() as i32).contains(&to) {
                self.preview(to as usize);
            }
        }
        if p.a {
            self.keep();
        } else if p.b {
            self.cancel_add();
        }
    }
    fn options_input(&mut self, pad: &Pad, p: &Presses) {
        let rows = self.options();
        if rows.is_empty() {
            self.mode = Mode::Board;
            return;
        }
        self.option = self.option.min(rows.len() - 1);
        if p.up && self.option > 0 {
            self.option -= 1;
        }
        if p.down && self.option + 1 < rows.len() {
            self.option += 1;
        }
        let opt = rows[self.option];
        if p.left || p.right {
            self.adjust_option(opt, if p.right { 1 } else { -1 }, pad.lt);
        }
        if p.a {
            self.activate_option(opt);
        } else if p.b {
            self.close_options();
        }
        // Not retail (user 10-19): Back undoes and the sticks move, scale and turn the layer here too, so a colour
        // and a place are set without going back to the board.
        if p.back {
            self.play(SOUND_B_BUTTON);
            self.undo();
        }
        if self.mode == Mode::Options {
            self.sticks(pad);
        }
    }
    fn menu_input(&mut self, pad: &Pad, p: &Presses) -> Option<Exit> {
        let rows = self.menu_rows();
        self.menu = self.menu.min(rows.len() - 1);
        if p.up && self.menu > 0 {
            self.menu -= 1;
        }
        if p.down && self.menu + 1 < rows.len() {
            self.menu += 1;
        }
        let row = rows[self.menu];
        if p.left || p.right {
            self.adjust_menu(row, if p.right { 1 } else { -1 }, pad.lt);
        }
        if p.a {
            return self.activate_menu(row);
        }
        if p.b || p.start {
            self.play(SOUND_B_BUTTON);
            self.mode = Mode::Board;
        }
        // Not retail (user 10-19): Back undoes on the Menu too (Redo is its row).
        if p.back {
            self.play(SOUND_B_BUTTON);
            self.undo();
        }
        None
    }
    fn popup_input(&mut self, p: &Presses) -> Option<Exit> {
        let Mode::Popup(ask) = self.mode else { return None };
        let count = ask.question().1.len();
        if p.up && self.popup > 0 {
            self.popup -= 1;
        }
        if p.down && self.popup + 1 < count {
            self.popup += 1;
        }
        if p.a {
            return self.answer(Some(self.popup));
        }
        if p.b {
            return self.answer(None);
        }
        None
    }
    fn replace_input(&mut self, p: &Presses) -> Option<Exit> {
        let count = self.store.saves(&self.item).len();
        if count == 0 {
            self.mode = Mode::Board;
            return None;
        }
        for (pressed, step) in [(p.left, -1i32), (p.right, 1), (p.up, -(REPLACE_COLS as i32)), (p.down, REPLACE_COLS as i32)] {
            let to = self.replace as i32 + step;
            if pressed && (0..count as i32).contains(&to) {
                self.replace = to as usize;
            }
        }
        if p.a {
            return self.replace_save(self.replace);
        }
        if p.b {
            self.cancel_replace();
        }
        None
    }

    // ---- text ----

    fn start_keyboard(&mut self, text: String) {
        self.keyboard = TextEntry { shift: true, ..TextEntry::new(&text, MAX_TEXT) };
        self.mode = Mode::Text;
        self.touch();
    }
    /// The selected text layer's letters, as typed: printable ASCII, at most `MAX_TEXT`.
    fn set_text(&mut self, text: String) {
        let text: String = text.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).take(MAX_TEXT).collect();
        if let Some(l) = self.current_mut() {
            if l.art != Art::Text(text.clone()) {
                l.art = Art::Text(text.clone());
                l.fit_text();
                self.keyboard.text = text;
                self.doc_rev += 1;
                self.changed = true;
            }
        }
        self.touch();
    }
    /// Typing: the PC keyboard types directly; the pad uses the on-screen keyboard (`TextEntry`'s grid:
    /// A types the key, X erases, Y is a space, Start is Done, B cancels, LB / RB jump half a row).
    fn text_input(&mut self, p: &Presses, typed: &[Typed]) {
        let Some(Layer { art: Art::Text(start), .. }) = self.current() else {
            self.mode = Mode::Board;
            return;
        };
        let mut text = start.clone();
        let mut finish = None;
        for key in typed {
            match key {
                Typed::Char(c) => text.push(*c),
                Typed::Backspace | Typed::Delete => {
                    text.pop();
                }
                Typed::Enter => finish = Some(true),
                Typed::Escape => finish = Some(false),
            }
        }
        for (pressed, key, sound) in [(p.up, Key::Up, SOUND_NAV_UP), (p.down, Key::Down, SOUND_NAV_DOWN), (p.left, Key::Left, nav_side(false)), (p.right, Key::Right, nav_side(true))] {
            if pressed {
                self.play(sound);
                self.keyboard.input(key);
            }
        }
        if p.lb || p.rb {
            self.play(nav_side(p.rb));
            self.keyboard.column = (self.keyboard.column + 5) % 10;
        }
        if p.a || p.x || p.y || p.start {
            self.play(SOUND_A_BUTTON);
        }
        if p.b {
            self.play(SOUND_B_BUTTON);
        }
        if p.a {
            match self.keyboard.key() {
                Command2::Char(c) => text.push(c),
                Command2::Space => text.push(' '),
                Command2::Backspace => {
                    text.pop();
                }
                Command2::Shift => self.keyboard.shift = !self.keyboard.shift,
                Command2::Done => finish = Some(true),
            }
        }
        if p.x {
            text.pop();
        }
        if p.y {
            text.push(' ');
        }
        if p.start {
            finish = Some(true);
        }
        if p.b {
            finish = Some(false);
        }
        self.set_text(text);
        match finish {
            Some(true) => self.finish_text(),
            Some(false) => self.cancel_text(),
            None => {}
        }
    }
    fn finish_text(&mut self) {
        self.mode = Mode::Board;
        if matches!(self.current(), Some(Layer { art: Art::Text(t), .. }) if t.trim().is_empty()) {
            let layer = self.layer;
            self.doc_mut().layers.remove(layer);
            self.tidy_groups();
            self.layer = self.layer.min(self.doc().layers.len().saturating_sub(1));
            self.say("Empty text removed");
        }
        self.text_before = None;
        self.edited();
    }
    fn cancel_text(&mut self) {
        match self.text_before.take() {
            Some(before) => {
                self.set_text(before);
                self.mode = Mode::Board;
            }
            // A new text layer: it goes.
            None => {
                self.set_text(String::new());
                self.finish_text();
            }
        }
    }

    // ---- the mouse ----

    /// The selected layer's handles, in canvas units: its corners, the middles of its edges, and the
    /// turn handle above its top.
    pub fn handles(&self) -> Vec<(Handle, Vec2)> {
        let Some(l) = self.current() else { return Vec::new() };
        let (c, (sin, cos)) = (Vec2::new(l.x as f32, l.y as f32), (l.angle as f32).to_radians().sin_cos());
        let at = |x: f32, y: f32| c + Vec2::new(x * cos - y * sin, x * sin + y * cos);
        let (w, h) = (l.w as f32 / 2., l.h as f32 / 2.);
        vec![
            (Handle::Corner, at(-w, -h)),
            (Handle::Corner, at(w, -h)),
            (Handle::Corner, at(w, h)),
            (Handle::Corner, at(-w, h)),
            (Handle::Width, at(-w, 0.)),
            (Handle::Width, at(w, 0.)),
            (Handle::Height, at(0., -h)),
            (Handle::Height, at(0., h)),
            (Handle::Turn, at(0., -h - TURN_GAP * self.view_scale)),
        ]
    }
    /// Every layer under a point, the top one first.
    pub fn hits(&self, at: Vec2) -> Vec<usize> {
        let layers = &self.doc().layers;
        (0..layers.len())
            .rev()
            .filter(|&i| {
                let l = &layers[i];
                let (sin, cos) = (l.angle as f32).to_radians().sin_cos();
                let d = at - Vec2::new(l.x as f32, l.y as f32);
                let local = Vec2::new(d.x * cos + d.y * sin, -d.x * sin + d.y * cos);
                local.x.abs() <= l.w as f32 / 2. && local.y.abs() <= l.h as f32 / 2.
            })
            .collect()
    }
    pub fn pointer(&mut self, event: Pointer) {
        self.mouse = true;
        if self.mode != Mode::Board && !(self.mode == Mode::Options && matches!(event, Pointer::Release | Pointer::Drag { .. })) {
            return;
        }
        match event {
            Pointer::Press { at, right, alt } => {
                self.drag = None;
                let under = self.hits(at);
                if right {
                    if let Some(&hit) = under.first() {
                        self.select(hit);
                        self.mode = Mode::Options;
                        self.option = 0;
                        self.touch();
                    }
                    return;
                }
                if !alt {
                    let radius = HANDLE * 1.4 * self.view_scale;
                    let near = self.handles().into_iter().filter(|(_, p)| at.distance(*p) <= radius).min_by(|a, b| at.distance(a.1).total_cmp(&at.distance(b.1)));
                    if let (Some((handle, _)), Some(l)) = (near, self.current()) {
                        let centre = Vec2::new(l.x as f32, l.y as f32);
                        let (sin, cos) = (l.angle as f32).to_radians().sin_cos();
                        let d = at - centre;
                        let kind = match handle {
                            Handle::Corner => Drag::Size { start: centre.distance(at).max(1.), w: l.w as f32, h: l.h as f32 },
                            Handle::Width => Drag::Stretch { axis: 0, start: (d.x * cos + d.y * sin).abs().max(1.), size: l.w as f32 },
                            Handle::Height => Drag::Stretch { axis: 1, start: (-d.x * sin + d.y * cos).abs().max(1.), size: l.h as f32 },
                            Handle::Turn => Drag::Turn,
                        };
                        self.drag = Some(Dragging { kind, started: false });
                        return;
                    }
                }
                // Alt: the next layer down under the cursor, round to the top again.
                let pick = match under.iter().position(|&i| i == self.layer) {
                    Some(at) if alt => under.get(at + 1).or(under.first()).copied(),
                    _ => under.first().copied(),
                };
                if let Some(hit) = pick {
                    self.select(hit);
                    let l = &self.doc().layers[hit];
                    let grab = Vec2::new(l.x as f32, l.y as f32) - at;
                    self.drag = Some(Dragging { kind: Drag::Move { grab }, started: false });
                }
            }
            Pointer::Drag { at, shift } => {
                let Some(mut drag) = self.drag else { return };
                if !drag.started {
                    self.checkpoint();
                    drag.started = true;
                    self.drag = Some(drag);
                    self.live = self.begin_live();
                }
                let Some(mut live) = self.live.clone() else { return };
                let centre = Vec2::new(live.x, live.y);
                let (lo, hi) = (SIZES.0 as f32, SIZES.1 as f32);
                match drag.kind {
                    Drag::Move { grab } => {
                        live.x = at.x + grab.x;
                        live.y = at.y + grab.y;
                    }
                    Drag::Size { start, w, h } => {
                        let factor = (centre.distance(at) / start).clamp(lo / w.min(h), hi / w.max(h));
                        live.w = w * factor;
                        live.h = h * factor;
                    }
                    Drag::Stretch { axis, start, size } => {
                        let (sin, cos) = live.angle.to_radians().sin_cos();
                        let d = at - centre;
                        let along = if axis == 0 { d.x * cos + d.y * sin } else { -d.x * sin + d.y * cos };
                        let size = (size * along.abs() / start).clamp(lo, hi);
                        if axis == 0 {
                            live.w = size;
                        } else {
                            live.h = size;
                        }
                    }
                    Drag::Turn => {
                        let v = at - centre;
                        if v.length() > 1. {
                            let angle = v.x.atan2(-v.y).to_degrees();
                            // Shift: 15 degree steps. Else close to a multiple of 45 degrees it settles there.
                            let snapped = (angle / 45.).round() * 45.;
                            live.angle = wrap_angle(if shift {
                                (angle / SNAP).round() * SNAP
                            } else if (angle - snapped).abs() < 3. {
                                snapped
                            } else {
                                angle
                            });
                        }
                    }
                }
                self.apply_live(&live);
                self.live = Some(live);
            }
            Pointer::Release => {
                self.drag = None;
                self.live = None;
            }
            Pointer::Wheel { at, lines, shift } => {
                let over = self.hits(at).first().copied().or_else(|| self.current().map(|_| self.layer));
                let Some(over) = over else { return };
                self.select(over);
                let Some(mut live) = self.begin_live() else { return };
                self.checkpoint();
                if shift {
                    live.angle = wrap_angle(live.angle + lines * 5.);
                } else {
                    let (lo, hi) = (SIZES.0 as f32, SIZES.1 as f32);
                    let factor = 1.08f32.powf(lines).clamp(lo / live.w.min(live.h), hi / live.w.max(live.h));
                    live.w *= factor;
                    live.h *= factor;
                }
                self.apply_live(&live);
            }
        }
    }
    /// The keyboard's shortcuts, on the board.
    pub fn shortcut(&mut self, key: Shortcut) {
        if self.mode != Mode::Board {
            return;
        }
        match key {
            Shortcut::Undo => self.undo(),
            Shortcut::Redo => self.redo(),
            Shortcut::Duplicate => self.duplicate(),
            Shortcut::Mirror => self.mirror_copy(),
            Shortcut::Copy => {
                if let Some(l) = self.current().cloned() {
                    self.copied = Some(l);
                    self.say("Layer copied");
                }
            }
            Shortcut::Paste => {
                if let Some(l) = self.copied.clone() {
                    self.add(l);
                }
            }
            Shortcut::Delete => {
                if self.current().is_some() {
                    self.ask(Ask::Delete);
                }
            }
            Shortcut::Nudge(x, y, far) => {
                let Some(mut live) = self.begin_live() else { return };
                let step = f32::from(if far { NUDGE * 10 } else { NUDGE });
                self.checkpoint();
                let reach = f32::from(REACH);
                live.x = (live.x + f32::from(x) * step).clamp(-reach, reach);
                live.y = (live.y + f32::from(y) * step).clamp(-reach, reach);
                self.apply_live(&live);
            }
        }
    }
}

/// A side's design as worn and saved: empty text left out, none when it holds no layer and no
/// background colour (a side of one colour is a design).
pub(crate) fn design(doc: &Doc) -> Option<String> {
    let mut doc = doc.clone();
    doc.layers.retain(|l| !matches!(&l.art, Art::Text(t) if t.is_empty()));
    if let Some(bottom) = doc.layers.first_mut() {
        bottom.grouped = false;
    }
    (!doc.layers.is_empty() || doc.background[3] > 0).then(|| doc.encode())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const TEE_FRONT: [f32; 4] = [0.98, 0.48, 0.13, 0.37];
    const TEE_BACK: [f32; 4] = [0.98, 0.48, 0.64, 0.86];
    const SLEEVE_L: [f32; 4] = [0.48, 0.25, 0.05, 0.45];
    const SLEEVE_R: [f32; 4] = [0.48, 0.25, 0.55, 0.95];
    const TEE: &str = "801095192c838106";
    const DT: f32 = 1. / 60.;

    pub(crate) fn tee() -> Editor {
        let sides = vec![
            SideDoc { zone: "front", rect: TEE_FRONT, doc: Doc::default() },
            SideDoc { zone: "back", rect: TEE_BACK, doc: Doc::default() },
            SideDoc { zone: "left", rect: SLEEVE_L, doc: Doc::default() },
            SideDoc { zone: "right", rect: SLEEVE_R, doc: Doc::default() },
        ];
        let catalog = Catalog { logos: vec![(0x11, "Brand".into())], art: vec![(0x22, "Tattoo".into())] };
        Editor::new(TEE, "Custom Tee", sides, 0, catalog, Store::default())
    }
    fn hat() -> Editor {
        let sides = vec![SideDoc { zone: "hat", rect: [0.98, 0.5, 0.2, 0.8], doc: Doc::default() }];
        Editor::new("hat", "Cap", sides, 0, Catalog::default(), Store::default())
    }
    /// One frame with `set` held, then one with nothing: a press.
    pub(crate) fn press(e: &mut Editor, set: impl Fn(&mut Pad)) -> Option<Exit> {
        let mut pad = Pad { dt: DT, ..Pad::default() };
        set(&mut pad);
        let exit = e.input(&pad, &[]);
        exit.or(e.input(&Pad { dt: DT, lt: pad.lt, rt: pad.rt, ..Pad::default() }, &[]))
    }
    /// Hold a pad state for `seconds` at 60 Hz, then let go.
    fn hold(e: &mut Editor, pad: Pad, seconds: f32) {
        for _ in 0..(seconds * 60.).round() as usize {
            e.input(&Pad { dt: DT, ..pad }, &[]);
        }
        e.input(&Pad { dt: DT, ..Pad::default() }, &[]);
    }
    fn with(e: &mut Editor, layers: Vec<Layer>) {
        e.sides[e.side].doc.layers = layers;
        e.layer = e.doc().layers.len().saturating_sub(1);
    }
    fn shape(n: u8) -> Layer {
        Layer::new(Art::Shape(n), [255; 4])
    }

    #[test]
    fn rs_moves_ls_scales_and_bumpers_rotate_with_45_degree_detents() {
        let mut e = tee();
        with(&mut e, vec![shape(2)]);
        // RS right a second at full deflection: MOVE thousandths (the curve keeps full as full).
        hold(&mut e, Pad { rs: Vec2::X, ..Pad::default() }, 1.);
        assert!((e.doc().layers[0].x as i32 - MOVE as i32).abs() <= 25, "{}", e.doc().layers[0].x);
        assert_eq!(e.doc().layers[0].y, 0);
        // RS up moves up (y is down on the canvas).
        hold(&mut e, Pad { rs: Vec2::Y, ..Pad::default() }, 0.5);
        assert!(e.doc().layers[0].y < -600);
        // LS up grows width and height alike; LS down shrinks.
        let w = e.doc().layers[0].w;
        hold(&mut e, Pad { ls: Vec2::Y, ..Pad::default() }, 0.5);
        let l = &e.doc().layers[0];
        assert!(l.w > w && l.w == l.h, "{} {}", l.w, l.h);
        // LS sideways alone does nothing without Stretch.
        let before = e.doc().clone();
        hold(&mut e, Pad { ls: Vec2::X, ..Pad::default() }, 0.5);
        assert_eq!(e.doc(), &before);
        // RB held turns clockwise at TURN and stops at 45 degrees for DETENT.
        let mut pad = Pad { rb: true, dt: DT, ..Pad::default() };
        let mut t = 0.;
        let mut at_45 = 0.;
        while t < 0.75 {
            e.input(&pad, &[]);
            t += DT;
            if e.doc().layers[0].angle == 45 {
                at_45 += DT;
            }
        }
        assert!((at_45 - DETENT).abs() < 2.5 * DT, "held at 45 for {at_45}");
        let angle = e.doc().layers[0].angle as f32;
        assert!((angle - (0.75 - DETENT) * TURN).abs() < 3., "{angle}");
        pad.rb = false;
        e.input(&pad, &[]);
        // LB turns back, counter-clockwise, through the same stop.
        hold(&mut e, Pad { lb: true, ..Pad::default() }, 0.3);
        assert!(e.doc().layers[0].angle < angle as i16);
        // Each stick gesture is one undo step.
        let (steps, _) = e.undo_steps();
        assert!(steps >= 5, "{steps}");
    }

    #[test]
    fn l3_stretch_scales_width_and_height_apart() {
        let mut e = tee();
        with(&mut e, vec![shape(0)]);
        assert!(!e.stretch());
        press(&mut e, |p| p.l3 = true);
        assert!(e.stretch());
        hold(&mut e, Pad { ls: Vec2::X, ..Pad::default() }, 0.5);
        let l = e.doc().layers[0].clone();
        assert!(l.w > 1000 && l.h == 1000, "{} {}", l.w, l.h);
        hold(&mut e, Pad { ls: Vec2::NEG_Y, ..Pad::default() }, 0.5);
        let l = e.doc().layers[0].clone();
        assert!(l.h < 1000 && l.w > 1000, "{} {}", l.w, l.h);
        press(&mut e, |p| p.l3 = true);
        assert!(!e.stretch());
    }

    #[test]
    fn lt_is_fine_and_rt_dpad_reorders_with_one_undo_each() {
        let mut e = tee();
        with(&mut e, vec![shape(0)]);
        // LT held: 15% speed.
        hold(&mut e, Pad { rs: Vec2::X, lt: true, ..Pad::default() }, 1.);
        assert!((e.doc().layers[0].x as f32 - MOVE * FINE).abs() <= 6., "{}", e.doc().layers[0].x);
        // LT held: a bumper press turns one degree.
        press(&mut e, |p| {
            p.lt = true;
            p.rb = true;
        });
        assert_eq!(e.doc().layers[0].angle, 1);
        press(&mut e, |p| {
            p.lt = true;
            p.lb = true;
        });
        press(&mut e, |p| {
            p.lt = true;
            p.lb = true;
        });
        assert_eq!(e.doc().layers[0].angle, -1);
        // RT + D-pad moves the layer in the stack, one undo step a press; the D-pad alone selects.
        with(&mut e, vec![shape(0), shape(1), shape(2)]);
        e.undo.clear();
        assert_eq!(e.layer, 2);
        press(&mut e, |p| p.down = true);
        assert_eq!(e.layer, 1, "down selects the layer below");
        press(&mut e, |p| {
            p.rt = true;
            p.down = true;
        });
        assert_eq!((e.layer, e.doc().layers[0].art.clone()), (0, Art::Shape(1)));
        press(&mut e, |p| {
            p.rt = true;
            p.up = true;
        });
        press(&mut e, |p| {
            p.rt = true;
            p.up = true;
        });
        assert_eq!((e.layer, e.doc().layers[2].art.clone()), (2, Art::Shape(1)));
        assert_eq!(e.undo_steps().0, 3);
        press(&mut e, |p| p.back = true);
        assert_eq!(e.doc().layers[1].art, Art::Shape(1), "Back undoes one move");
        // A held D-pad repeats after a pause.
        with(&mut e, (0..10).map(|n| shape(n)).collect());
        e.layer = 9;
        hold(&mut e, Pad { down: true, ..Pad::default() }, 0.75);
        assert!((4..=6).contains(&e.layer), "{}", e.layer);
    }

    #[test]
    fn y_resets_the_transform_only() {
        let mut e = tee();
        let moved = Layer { x: 500, y: -300, w: 600, h: 1800, angle: 30, flip_x: true, ..Layer::new(Art::Shape(4), [10, 20, 30, 128]) };
        with(&mut e, vec![shape(0), Layer { grouped: true, ..moved }]);
        press(&mut e, |p| p.y = true);
        let l = &e.doc().layers[1];
        assert_eq!((l.x, l.y, l.w, l.h, l.angle, l.flip_x, l.flip_y), (0, 0, 1000, 1000, 0, false, false));
        assert_eq!((l.art.clone(), l.colour, l.grouped), (Art::Shape(4), [10, 20, 30, 128], true));
        press(&mut e, |p| p.back = true);
        assert_eq!(e.doc().layers[1].x, 500);
    }

    #[test]
    fn sides_wrap_within_the_item() {
        // A hat has its front alone: Left and Right never leave it.
        let mut e = hat();
        for _ in 0..3 {
            press(&mut e, |p| p.right = true);
            press(&mut e, |p| p.left = true);
        }
        assert_eq!((e.side, e.zone()), (0, "hat"));
        assert!(e.doc().layers.is_empty() && e.current().is_none());
        // A tee: front, back, left, right, and round again; Left goes the other way.
        let mut e = tee();
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(e.zone());
            press(&mut e, |p| p.right = true);
        }
        assert_eq!(seen, ["front", "back", "left", "right", "front"]);
        // From the back, Left twice wraps past the front.
        press(&mut e, |p| p.left = true);
        press(&mut e, |p| p.left = true);
        assert_eq!(e.zone(), "right");
        assert!(e.side < e.sides.len());
    }

    #[test]
    fn mirror_copy_flips_across_the_centre_line() {
        let mut e = tee();
        with(&mut e, vec![Layer { x: 300, angle: 20, ..shape(9) }, shape(1)]);
        e.select(0);
        e.mirror_copy();
        let layers = &e.doc().layers;
        assert_eq!(layers.len(), 3);
        assert_eq!((e.layer, layers[1].x, layers[1].flip_x, layers[1].angle), (1, -300, true, -20), "just above the source");
        assert_eq!(layers[2].art, Art::Shape(1));
        // Text keeps reading the right way.
        with(&mut e, vec![Layer { x: 300, ..Layer::new(Art::Text("SK8".into()), [255; 4]) }]);
        e.mirror_copy();
        assert_eq!((e.doc().layers[1].x, e.doc().layers[1].flip_x), (-300, false));
        press(&mut e, |p| p.back = true);
        assert_eq!(e.doc().layers.len(), 1);
    }

    #[test]
    fn copy_to_pair_mirrors_onto_the_paired_side() {
        let mut e = tee();
        with(&mut e, vec![Layer { x: 400, ..shape(3) }]);
        assert_eq!(e.pair_name(), Some("#Copy to Back"));
        assert!(e.copy_to_pair());
        assert_eq!(e.zone(), "front", "it stays on the side being edited");
        let back = &e.sides[1].doc.layers;
        assert_eq!((back.len(), back[0].x, back[0].flip_x), (1, -400, true));
        e.set_side(2);
        assert_eq!(e.pair_name(), Some("#Copy to R Sleeve"));
        with(&mut e, vec![shape(5)]);
        assert!(e.copy_to_pair());
        assert_eq!(e.sides[3].doc.layers.len(), 1);
        e.set_side(3);
        assert_eq!(e.pair_name(), Some("#Copy to L Sleeve"));
        // One undo step brings the back's copy off again.
        e.undo();
        assert!(e.sides[3].doc.layers.is_empty());
        // A hat has no side across.
        let mut h = hat();
        with(&mut h, vec![shape(0)]);
        assert_eq!(h.pair_name(), None);
        assert!(!h.copy_to_pair());
        assert!(!h.options().contains(&Opt::PairCopy));
        // Copy Side to... replaces the target's layers, one undo step.
        let mut e = tee();
        with(&mut e, vec![shape(1), shape(2)]);
        e.sides[1].doc.layers = vec![shape(7)];
        e.copy_side_to(1);
        assert_eq!(e.sides[1].doc, e.sides[0].doc);
        e.undo();
        assert_eq!(e.sides[1].doc.layers, vec![shape(7)]);
    }

    #[test]
    fn groups_move_scale_and_recolour_together() {
        let mut e = tee();
        with(&mut e, vec![shape(0), Layer { x: 1000, ..shape(1) }, Layer { x: -500, ..shape(2) }]);
        e.select(1);
        e.toggle_group();
        assert!(e.doc().layers[1].grouped);
        assert_eq!(e.group(0), 0..=1);
        assert_eq!(e.group(1), 0..=1);
        assert_eq!(e.group(2), 2..=2);
        // Moving the upper member moves the lower too, by the same amount.
        hold(&mut e, Pad { rs: Vec2::X, ..Pad::default() }, 0.5);
        let (a, b) = (e.doc().layers[0].x, e.doc().layers[1].x);
        assert!(a > 500 && b - a == 1000, "{a} {b}");
        assert_eq!(e.doc().layers[2].x, -500, "not grouped: stays");
        // Scaling scales both, and the gap between them.
        hold(&mut e, Pad { ls: Vec2::Y, ..Pad::default() }, 0.5);
        let (l0, l1) = (e.doc().layers[0].clone(), e.doc().layers[1].clone());
        assert!(l0.w > 1000 && (i32::from(l0.w) - i32::from(l1.w)).abs() <= 1);
        assert!(l1.x - l0.x > 1000);
        // Recolouring colours both; the ungrouped layer keeps its white.
        e.set_swatch(5);
        assert_eq!(e.doc().layers[0].colour[..3], PALETTE[5]);
        assert_eq!(e.doc().layers[1].colour[..3], PALETTE[5]);
        assert_eq!(e.doc().layers[2].colour, [255; 4]);
        // The group is saved: flags bit 2.
        let doc = Doc::decode(&design(e.doc()).unwrap()).unwrap();
        assert!(doc.layers[1].grouped && !doc.layers[0].grouped);
    }

    #[test]
    fn groups_survive_delete_and_undo() {
        let mut e = tee();
        with(&mut e, vec![shape(0), shape(1), shape(2)]);
        // Group on the bottom layer: nothing below, nothing happens.
        e.select(0);
        let before = e.doc().clone();
        e.toggle_group();
        assert_eq!(e.doc(), &before);
        assert!(!e.options().contains(&Opt::Group));
        // 2 onto 1 onto 0; deleting the lower member (1) clears the upper's flag.
        e.select(1);
        e.toggle_group();
        e.select(2);
        e.toggle_group();
        let grouped = e.doc().clone();
        e.select(1);
        e.delete();
        assert_eq!(e.doc().layers.iter().map(|l| l.grouped).collect::<Vec<_>>(), [false, false]);
        assert_eq!(e.group(1), 1..=1);
        e.undo();
        assert_eq!(e.doc(), &grouped, "undo restores the group exactly");
        // Deleting the bottom layer leaves no grouped bottom.
        e.select(0);
        e.delete();
        assert!(!e.doc().layers[0].grouped);
        e.undo();
        // Reordering a member out of its group unlinks it and its old neighbour.
        e.select(1);
        e.reorder(1, 2);
        assert!(e.doc().layers.iter().all(|l| !l.grouped));
        e.undo();
        assert_eq!(e.doc(), &grouped);
    }

    #[test]
    fn hue_shade_opacity_round_trip_and_feed_recent_colours() {
        let mut e = tee();
        with(&mut e, vec![shape(2)]);
        assert_eq!(e.doc().layers[0].colour, [255; 4], "new pieces are white");
        assert!(e.recent_colours().is_empty());
        e.set_shade(40);
        e.set_hue(200);
        assert!((i32::from(e.hue()) - 200).abs() <= 2, "{}", e.hue());
        assert!((i32::from(e.shade()) - 40).abs() <= 1, "{}", e.shade());
        e.set_opacity(55);
        assert_eq!(e.opacity(), 55);
        // A slider's moves are one recent colour, replaced as it moves.
        assert_eq!(e.recent_colours().len(), 1);
        let hued = e.recent_colours()[0];
        assert_eq!(hued, [e.doc().layers[0].colour[0], e.doc().layers[0].colour[1], e.doc().layers[0].colour[2]]);
        // A swatch adds one, newest first; at most eight.
        e.set_swatch(5);
        assert_eq!(e.recent_colours(), [PALETTE[5], hued]);
        for i in 6..20 {
            e.set_swatch(i);
        }
        assert_eq!(e.recent_colours().len(), 8);
        assert_eq!(e.recent_colours()[0], PALETTE[19]);
        // The Options rows show the values; the sliders step and show their number.
        e.mode = Mode::Options;
        let at = e.options().iter().position(|o| *o == Opt::Opacity).unwrap();
        e.option = at;
        press(&mut e, |p| p.left = true);
        assert_eq!(e.opacity(), 45);
        assert_eq!(e.option_value(Opt::Opacity), (5., "#45%".into()));
        // Stock art keeps its own colours: only opacity.
        with(&mut e, vec![Layer::new(Art::Logo(0x11), [255; 4])]);
        assert!(!e.options().iter().any(|o| matches!(o, Opt::Colour | Opt::Hue | Opt::Shade)) && e.options().contains(&Opt::Opacity));
    }

    #[test]
    fn layers_under_lists_overlapping_layers_below() {
        let mut e = tee();
        with(&mut e, vec![shape(0), Layer { x: 2000, ..shape(1) }, Layer { x: 300, ..shape(2) }, shape(3)]);
        assert_eq!(e.layers_under(3), [2, 0]);
        assert!(e.layers_under(0).is_empty());
        // The Options row cycles them and A selects one.
        e.mode = Mode::Options;
        e.option = e.options().iter().position(|o| *o == Opt::LayersUnder).unwrap();
        press(&mut e, |p| p.right = true);
        press(&mut e, |p| p.a = true);
        assert_eq!((e.mode, e.layer), (Mode::Board, 0));
    }

    #[test]
    fn b_popup_choices_leave_the_right_look() {
        // B, Cancel, B, Discard: out, putting the look back; nothing saved.
        let mut e = tee();
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.a = true);
        assert_eq!(e.doc().layers.len(), 1);
        assert_eq!(press(&mut e, |p| p.b = true), None);
        assert_eq!(e.mode, Mode::Popup(Ask::Done));
        for _ in 0..3 {
            press(&mut e, |p| p.down = true);
        }
        assert_eq!(press(&mut e, |p| p.a = true), None, "Cancel");
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Board, 1));
        press(&mut e, |p| p.b = true);
        press(&mut e, |p| p.down = true);
        press(&mut e, |p| p.down = true);
        assert_eq!(press(&mut e, |p| p.a = true), Some(Exit::Discard));
        assert!(e.store.saves(TEE).is_empty() && e.take_saved().is_none());
        // User 10-19: nothing unsaved, B just leaves: a New left untouched as it was, a save kept on.
        let mut e = tee();
        assert_eq!(press(&mut e, |p| p.b = true), Some(Exit::Discard));
        e.editing = Some(0);
        assert_eq!(press(&mut e, |p| p.b = true), Some(Exit::Done));
        e.editing = None;
        // B on the popup is Cancel too.
        e.changed = true;
        press(&mut e, |p| p.b = true);
        assert_eq!((press(&mut e, |p| p.b = true), e.mode), (None, Mode::Board));
        // Keep Without Saving keeps the look, saves nothing.
        assert_eq!(e.done(DoneChoice::KeepWithoutSaving), Some(Exit::Done));
        assert!(e.store.saves(TEE).is_empty());
        // Save writes the save and leaves; with nothing to save it stays.
        assert_eq!(e.done(DoneChoice::Save), None);
        assert_eq!(e.status, "Add a layer before saving");
        with(&mut e, vec![shape(1)]);
        assert_eq!(e.done(DoneChoice::Save), Some(Exit::Done));
        assert_eq!(e.store.saves(TEE).len(), 1);
        // A full list: Save asks which to replace, and leaves once one is replaced.
        let mut e = tee();
        with(&mut e, vec![shape(1)]);
        for n in 0..crate::graphic_store::MAX_SAVES {
            e.doc_mut().layers[0].angle = n as i16;
            e.save(true);
        }
        e.editing = None;
        e.doc_mut().layers[0].angle = 99;
        assert_eq!(e.done(DoneChoice::Save), None);
        assert_eq!(e.mode, Mode::Replace);
        press(&mut e, |p| p.right = true);
        assert_eq!(press(&mut e, |p| p.a = true), Some(Exit::Done));
        assert_eq!(e.store.saves(TEE)[1].sides.len(), 1);
        assert_eq!(e.store.saves(TEE).len(), crate::graphic_store::MAX_SAVES);
    }

    #[test]
    fn the_add_grid_previews_keeps_and_cancels() {
        let mut e = tee();
        press(&mut e, |p| p.a = true);
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Add, 1), "the highlighted piece shows at once");
        press(&mut e, |p| p.right = true);
        press(&mut e, |p| p.right = true);
        assert_eq!((e.doc().layers.len(), e.doc().layers[0].art.clone()), (1, Art::Shape(2)), "one preview, the Circle");
        press(&mut e, |p| p.b = true);
        assert!(e.doc().layers.is_empty() && e.mode == Mode::Board && e.undo_steps().0 == 0, "B puts it back");
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.rb = true);
        assert_eq!(e.picker.tab, Tab::GameArt);
        press(&mut e, |p| p.rb = true);
        assert_eq!(e.doc().layers[0].art, Art::Logo(0x11));
        press(&mut e, |p| p.a = true);
        assert_eq!((e.mode, e.doc().layers.len(), e.undo_steps().0), (Mode::Board, 1, 1));
        assert_eq!((e.doc().layers[0].w, e.doc().layers[0].h), (2000, 1000));
        press(&mut e, |p| p.back = true);
        assert!(e.doc().layers.is_empty(), "one undo step takes the kept piece off");
    }

    /// User 10-19: a stick pushed on the Add grid places the highlighted piece and moves it with the same push; on
    /// Options the sticks still move it and Back undoes, as on the Menu.
    #[test]
    fn a_stick_places_the_piece_and_options_and_the_menu_keep_the_sticks_and_undo() {
        let mut e = tee();
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.right = true);
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Add, 1), "previewed");
        hold(&mut e, Pad { rs: Vec2::X * 0.3, ..Pad::default() }, 0.2);
        assert_eq!(e.mode, Mode::Add, "a stick resting off centre places nothing");
        e.sounds.clear();
        hold(&mut e, Pad { rs: Vec2::X, ..Pad::default() }, 0.5);
        assert_eq!((e.mode, e.doc().layers.len(), e.current().unwrap().art.clone()), (Mode::Board, 1, Art::Shape(1)), "placed");
        assert!(e.current().unwrap().x > 500 && e.changed, "and moved: {}", e.current().unwrap().x);
        assert!(e.sounds.contains(&SOUND_A_BUTTON));
        // The move, then the piece: an undo step each.
        press(&mut e, |p| p.back = true);
        assert_eq!((e.doc().layers.len(), e.doc().layers[0].x), (1, 0));
        press(&mut e, |p| p.back = true);
        assert!(e.doc().layers.is_empty());
        // The Text tab has no piece to push: the sticks do nothing there.
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.lb = true);
        hold(&mut e, Pad { rs: Vec2::X, ..Pad::default() }, 0.2);
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Add, 0));
        press(&mut e, |p| p.b = true);

        // Options: RS moves the layer (one undo step), Back takes it back, the page stays.
        with(&mut e, vec![shape(2)]);
        press(&mut e, |p| p.x = true);
        hold(&mut e, Pad { rs: Vec2::Y, ..Pad::default() }, 0.5);
        assert!(e.doc().layers[0].y < -300 && e.mode == Mode::Options, "{}", e.doc().layers[0].y);
        press(&mut e, |p| p.back = true);
        assert_eq!((e.doc().layers[0].y, e.mode), (0, Mode::Options));
        press(&mut e, |p| p.b = true);
        // The Menu: Back undoes too.
        hold(&mut e, Pad { rs: Vec2::X, ..Pad::default() }, 0.3);
        assert!(e.doc().layers[0].x > 0);
        press(&mut e, |p| p.start = true);
        press(&mut e, |p| p.back = true);
        assert_eq!((e.doc().layers[0].x, e.mode), (0, Mode::Menu));
    }

    #[test]
    fn text_is_ascii_and_capped_at_24() {
        let mut e = tee();
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.lb = true);
        assert_eq!(e.picker.tab, Tab::Text);
        press(&mut e, |p| p.a = true);
        assert_eq!(e.mode, Mode::Text);
        // The PC keyboard types printable ASCII.
        e.input(&Pad::default(), &[Typed::Char('H'), Typed::Char('i'), Typed::Char('\u{7}'), Typed::Char('é')]);
        assert_eq!(e.doc().layers[0].art, Art::Text("Hi".into()));
        // The pad: A types the highlighted key (QWERTY's first: "1"), X erases, Y is a space.
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.y = true);
        press(&mut e, |p| p.x = true);
        assert_eq!(e.doc().layers[0].art, Art::Text("Hi1".into()));
        // At most 24 characters.
        e.input(&Pad::default(), &[Typed::Char('W'); 40]);
        assert!(matches!(&e.doc().layers[0].art, Art::Text(t) if t.len() == MAX_TEXT));
        press(&mut e, |p| p.start = true);
        assert_eq!(e.mode, Mode::Board);
        // Edit Text, erase it all, Done: the empty layer goes.
        e.mode = Mode::Options;
        e.activate_option(Opt::Text);
        e.input(&Pad::default(), &[Typed::Backspace; 30]);
        e.input(&Pad::default(), &[Typed::Enter]);
        assert!(e.doc().layers.is_empty());
        assert_eq!(e.status, "Empty text removed");
    }

    /// Research table: "SK8" took about 40 presses; the grid keyboard with its half-row jump takes 12, Done included.
    #[test]
    fn pad_types_sk8_in_a_few_presses() {
        let mut e = tee();
        press(&mut e, |p| p.a = true);
        press(&mut e, |p| p.lb = true);
        press(&mut e, |p| p.a = true);
        assert_eq!(e.mode, Mode::Text);
        // QWERTY (text_entry::GRID): S row 2 column 1, K column 7 (RB jumps 5), 8 two rows up.
        let steps: [fn(&mut Pad); 11] = [
            |p| p.down = true,
            |p| p.down = true,
            |p| p.right = true,
            |p| p.a = true, // S
            |p| p.rb = true,
            |p| p.right = true,
            |p| p.a = true, // K
            |p| p.up = true,
            |p| p.up = true,
            |p| p.a = true, // 8
            |p| p.start = true,
        ];
        for step in steps {
            press(&mut e, step);
        }
        assert_eq!((e.mode, e.doc().layers[0].art.clone()), (Mode::Board, Art::Text("SK8".into())));
    }

    #[test]
    fn saving_overwrites_saves_as_new_and_asks_which_to_replace_when_full() {
        let mut e = tee();
        with(&mut e, vec![shape(0)]);
        assert!(!e.menu_rows().contains(&MenuRow::SaveNew), "no Save as New before a save is opened");
        press(&mut e, |p| p.start = true);
        assert_eq!(e.mode, Mode::Menu);
        press(&mut e, |p| p.a = true);
        let first = e.take_saved().expect("saved");
        assert_eq!(e.editing, Some(0));
        assert_eq!(e.store.saves(TEE), [&first]);
        hold(&mut e, Pad { rs: Vec2::X, ..Pad::default() }, 0.3);
        e.save(false);
        assert_eq!(e.store.saves(TEE).len(), 1);
        assert_ne!(e.store.saves(TEE)[0], &first);
        assert_eq!(e.menu_rows()[3], MenuRow::SaveNew, "after Copy Side to...: the rows above keep their places");
        // Save as New of a changed design (the store keeps one copy of an identical save).
        hold(&mut e, Pad { rs: Vec2::Y, ..Pad::default() }, 0.3);
        e.activate_menu(MenuRow::SaveNew);
        assert_eq!((e.store.saves(TEE).len(), e.editing), (2, Some(1)));
        // Clear Side asks first; undo brings it back.
        e.activate_menu(MenuRow::ClearSide);
        assert_eq!(e.mode, Mode::Popup(Ask::ClearSide));
        press(&mut e, |p| p.a = true);
        assert!(e.doc().layers.is_empty());
        e.undo();
        assert_eq!(e.doc().layers.len(), 1);
        // Delete asks first too.
        e.mode = Mode::Options;
        e.activate_option(Opt::Delete);
        assert_eq!(e.mode, Mode::Popup(Ask::Delete));
        press(&mut e, |p| p.down = true);
        press(&mut e, |p| p.a = true);
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Options, 1), "No");
        e.activate_option(Opt::Delete);
        press(&mut e, |p| p.a = true);
        assert!(e.doc().layers.is_empty());
        // An empty item cannot be saved.
        let mut blank = tee();
        blank.save(false);
        assert_eq!(blank.status, "Add a layer before saving");
    }

    #[test]
    fn a_side_holds_64_layers_and_edits_stay_inside_what_a_design_can_hold() {
        let mut e = tee();
        let mut told = false;
        for _ in 0..MAX_LAYERS + 8 {
            press(&mut e, |p| p.a = true);
            press(&mut e, |p| p.a = true);
            told |= e.mode == Mode::Popup(Ask::Full);
        }
        assert_eq!(e.doc().layers.len(), MAX_LAYERS);
        assert!(told, "the retail popup says the side is full");
        assert!(e.status.contains("at most 64"), "{}", e.status);
        assert_eq!(Ask::Full.question(), ("#A side holds at most 64 layers.", &["ID_COMMON_OK"][..]));
        while e.mode != Mode::Board {
            press(&mut e, |p| p.b = true);
        }
        // Mirror Copy on a full side says so too, and OK is the board again.
        e.mirror_copy();
        assert_eq!(e.mode, Mode::Popup(Ask::Full));
        press(&mut e, |p| p.a = true);
        assert_eq!(e.mode, Mode::Board);
        hold(&mut e, Pad { rs: Vec2::X, ls: Vec2::Y, ..Pad::default() }, 20.);
        let l = e.current().unwrap();
        assert_eq!(l.x, REACH);
        assert!(l.w <= SIZES.1 && l.h <= SIZES.1);
        hold(&mut e, Pad { rs: Vec2::NEG_X, ls: Vec2::NEG_Y, ..Pad::default() }, 30.);
        let l = e.current().unwrap();
        assert_eq!(l.x, -REACH);
        assert!(l.w >= SIZES.0 && l.h >= SIZES.0);
        let text = design(e.doc()).unwrap();
        assert_eq!(Doc::decode(&text).unwrap().layers.len(), MAX_LAYERS, "every edit decodes");
    }

    #[test]
    fn a_scripted_mouse_drag_and_handle_sequence_gives_the_expected_layers() {
        let mut e = tee();
        e.view_scale = 4.;
        e.add(Layer::new(Art::Shape(0), [255; 4]));
        e.add(Layer { x: 1200, y: 800, ..Layer::new(Art::Shape(2), [255, 0, 0, 255]) });
        let press = |e: &mut Editor, at: Vec2| e.pointer(Pointer::Press { at, right: false, alt: false });
        let drag = |e: &mut Editor, at: Vec2| e.pointer(Pointer::Drag { at, shift: false });
        press(&mut e, Vec2::new(100., 100.));
        assert_eq!(e.layer, 0);
        drag(&mut e, Vec2::new(300., 0.));
        drag(&mut e, Vec2::new(600., -300.));
        e.pointer(Pointer::Release);
        assert_eq!((e.doc().layers[0].x, e.doc().layers[0].y), (500, -400));
        let handle = |e: &Editor, kind: Handle, n: usize| e.handles().into_iter().filter(|h| h.0 == kind).nth(n).unwrap().1;
        let corner = handle(&e, Handle::Corner, 2);
        assert_eq!(corner, Vec2::new(1000., 100.));
        press(&mut e, corner);
        let centre = Vec2::new(500., -400.);
        drag(&mut e, centre + (corner - centre) * 2.);
        e.pointer(Pointer::Release);
        assert_eq!((e.doc().layers[0].w, e.doc().layers[0].h), (2000, 2000));
        // The right edge stretches the width alone.
        let edge = handle(&e, Handle::Width, 1);
        press(&mut e, edge);
        drag(&mut e, centre + (edge - centre) * 1.5);
        e.pointer(Pointer::Release);
        assert_eq!((e.doc().layers[0].w, e.doc().layers[0].h), (3000, 2000));
        let top = handle(&e, Handle::Turn, 0);
        assert!(top.y < centre.y);
        press(&mut e, top);
        drag(&mut e, centre + Vec2::new(900., 0.));
        e.pointer(Pointer::Release);
        assert_eq!(e.doc().layers[0].angle, 90);
        // Shift snaps a turn to 15 degrees.
        let turn = handle(&e, Handle::Turn, 0);
        press(&mut e, turn);
        let at = centre + Vec2::new(900. * 20f32.to_radians().cos(), 900. * 20f32.to_radians().sin());
        e.pointer(Pointer::Drag { at, shift: true });
        e.pointer(Pointer::Release);
        assert_eq!(e.doc().layers[0].angle, 105);
        e.add(Layer { w: 2000, h: 200, angle: 90, ..Layer::new(Art::Shape(0), [255; 4]) });
        assert_eq!(e.hits(Vec2::new(0., 900.)).first(), Some(&2));
        assert_ne!(e.hits(Vec2::new(900., 0.)).first(), Some(&2));
        let w = e.doc().layers[2].w;
        e.pointer(Pointer::Wheel { at: Vec2::new(0., 0.), lines: 3., shift: false });
        assert!(e.doc().layers[2].w > w);
        e.pointer(Pointer::Wheel { at: Vec2::new(0., 0.), lines: 3., shift: true });
        assert_eq!(e.doc().layers[2].angle, 105, "Shift+wheel turns");
        e.pointer(Pointer::Press { at: Vec2::new(1200., 800.), right: true, alt: false });
        assert_eq!((e.mode, e.layer), (Mode::Options, 1));
        e.mode = Mode::Board;
        let steps = e.undo_steps().0;
        press(&mut e, Vec2::new(1200., 800.));
        e.pointer(Pointer::Release);
        assert_eq!(e.undo_steps().0, steps, "a drag that never moves takes no undo step");
    }

    /// Research §D.2's common flows, on the pad, each at most its press count.
    #[test]
    fn common_tasks_take_few_presses() {
        fn run(e: &mut Editor, steps: &[fn(&mut Pad)]) -> Option<Exit> {
            steps.iter().map(|step| press(e, *step)).last().flatten()
        }
        let mut e = tee();
        // A red circle, 7: Add, Right x2 to the Circle, keep, Options, Right to red, back.
        run(&mut e, &[|p| p.a = true, |p| p.right = true, |p| p.right = true, |p| p.a = true, |p| p.x = true, |p| p.right = true, |p| p.b = true]);
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Board, 1));
        assert_eq!((e.doc().layers[0].art.clone(), e.doc().layers[0].colour), (Art::Shape(2), [220, 30, 40, 255]));
        // Mirror, 3: Options, Down to Mirror Copy, A.
        e.sides[0].doc.layers[0].x = 600;
        run(&mut e, &[|p| p.x = true, |p| p.down = true, |p| p.a = true]);
        assert_eq!(e.doc().layers.iter().map(|l| l.x).collect::<Vec<_>>(), [600, -600]);
        // Copy the front to the back, 4: Menu, Down x2 to Copy Side to (the Back first), A.
        run(&mut e, &[|p| p.start = true, |p| p.down = true, |p| p.down = true, |p| p.a = true]);
        assert_eq!(e.sides[1].doc.layers.len(), 2, "the back has the front's layers");
        // Undo, 1.
        run(&mut e, &[|p| p.back = true]);
        assert!(e.sides[1].doc.layers.is_empty());
        // Save, 2: Menu, A (Save is the first row).
        run(&mut e, &[|p| p.start = true, |p| p.a = true]);
        assert_eq!(e.store.saves(TEE).len(), 1);
        // Leave keeping changes, 2: B, then Save and Wear.
        e.add(shape(4));
        assert_eq!(run(&mut e, &[|p| p.b = true, |p| p.a = true]), Some(Exit::Done));
        // A stripe via L3, 4: Add (the Square shows), keep, L3, push LS sideways.
        let mut e = tee();
        run(&mut e, &[|p| p.a = true, |p| p.a = true, |p| p.l3 = true]);
        hold(&mut e, Pad { ls: Vec2::X, ..Pad::default() }, 1.5);
        let l = e.current().unwrap();
        assert!(l.art == Art::Shape(0) && l.w > 2 * l.h, "a stripe: {} x {}", l.w, l.h);
        // User 10-19: a piece on and in place, 1 press and a push: Add, push RS (no A).
        let mut e = tee();
        run(&mut e, &[|p| p.a = true]);
        hold(&mut e, Pad { rs: Vec2::NEG_X, ..Pad::default() }, 0.5);
        assert!(e.mode == Mode::Board && e.current().is_some_and(|l| l.x < -500));
    }

    /// Review I5: presses only the editor takes play the movie's own sounds (its page plays the rest).
    #[test]
    fn presses_only_the_editor_takes_play_the_movies_sounds() {
        let mut e = tee();
        with(&mut e, vec![shape(0), shape(2)]);
        let sound = |e: &mut Editor, set: fn(&mut Pad)| {
            e.sounds.clear();
            press(e, set);
            std::mem::take(&mut e.sounds)
        };
        assert_eq!(sound(&mut e, |p| p.right = true), [fe_sound_id("core_nav_right")], "the side selector");
        assert_eq!(sound(&mut e, |p| p.left = true), [fe_sound_id("core_nav_left")]);
        assert_eq!(sound(&mut e, |p| {
            p.rt = true;
            p.down = true;
        }), [SOUND_NAV_DOWN], "RT + D-pad moves the layer");
        assert_eq!(sound(&mut e, |p| p.back = true), [SOUND_B_BUTTON], "undo");
        assert_eq!(sound(&mut e, |p| p.y = true), [fe_sound_id("core_y_button")], "reset");
        assert_eq!(sound(&mut e, |p| p.x = true), [SOUND_A_BUTTON], "Options open");
        e.mode = Mode::Board;
        assert_eq!(sound(&mut e, |p| p.start = true), [SOUND_A_BUTTON], "the Menu opens");
        assert_eq!(sound(&mut e, |p| p.start = true), [SOUND_B_BUTTON], "and Start closes it");
        press(&mut e, |p| p.a = true);
        assert_eq!(sound(&mut e, |p| p.rb = true), [fe_sound_id("core_nav_right")], "Add's tabs");
        // The keyboard: moves and keys.
        e.mode = Mode::Board;
        e.add(Layer::new(Art::Text("A".into()), [255; 4]));
        e.mode = Mode::Text;
        assert_eq!(sound(&mut e, |p| p.down = true), [SOUND_NAV_DOWN]);
        assert_eq!(sound(&mut e, |p| p.rb = true), [fe_sound_id("core_nav_right")]);
        assert_eq!(sound(&mut e, |p| p.a = true), [SOUND_A_BUTTON]);
        assert_eq!(sound(&mut e, |p| p.b = true), [SOUND_B_BUTTON]);
    }

    /// Review M7 and M8: Clear Side's No goes back to the Menu; Mirror Copy and Copy to Back keep their rows
    /// for every kind of layer, and Copy Side to... keeps its row once a save is open.
    #[test]
    fn rows_stay_put_and_no_goes_back_where_it_was_asked() {
        let mut e = tee();
        for art in [Art::Shape(2), Art::Logo(0x11), Art::Text("SK8".into())] {
            with(&mut e, vec![Layer::new(art.clone(), [255; 4])]);
            let rows = e.options();
            assert_eq!(rows[1..3], [Opt::MirrorCopy, Opt::PairCopy], "{art:?}");
        }
        let copy_side = |e: &Editor| e.menu_rows().iter().position(|r| *r == MenuRow::CopySide);
        let before = copy_side(&e);
        e.save(false);
        assert_eq!((e.editing, copy_side(&e)), (Some(0), before));
        e.mode = Mode::Menu;
        e.activate_menu(MenuRow::ClearSide);
        assert_eq!(e.mode, Mode::Popup(Ask::ClearSide));
        e.answer(Some(1));
        assert_eq!(e.mode, Mode::Menu, "No: the Menu again");
    }

    #[test]
    fn alt_click_cycles_stacked_layers() {
        let mut e = tee();
        for n in 0..3 {
            e.add(Layer { w: 1000 + n * 200, h: 1000 + n * 200, ..Layer::new(Art::Shape(n as u8), [255; 4]) });
        }
        e.add(Layer { x: 2000, ..Layer::new(Art::Shape(5), [255; 4]) });
        let click = |e: &mut Editor, alt: bool| {
            e.pointer(Pointer::Press { at: Vec2::new(100., 100.), right: false, alt });
            e.pointer(Pointer::Release);
            e.layer
        };
        assert_eq!(click(&mut e, false), 2, "a click takes the top layer under the cursor");
        assert_eq!([click(&mut e, true), click(&mut e, true), click(&mut e, true)], [1, 0, 2], "Alt goes down the stack and round");
        assert_eq!(e.hits(Vec2::new(100., 100.)), [2, 1, 0]);
        assert!(e.hits(Vec2::new(3000., 3000.)).is_empty());
    }

    #[test]
    fn editing_a_save_opens_its_sides_and_overwrites_it() {
        let mut first = tee();
        with(&mut first, vec![shape(0)]);
        first.change_side(1);
        with(&mut first, vec![shape(1)]);
        first.save(false);
        let saved = first.take_saved().unwrap();
        assert_eq!(saved.sides.len(), 2);
        let mut again = tee().edit(0, &saved);
        again.store = first.store.clone();
        assert_eq!((again.sides[0].doc.layers.len(), again.sides[1].doc.layers.len()), (1, 1));
        again.change_side(1);
        again.delete();
        again.save(false);
        assert_eq!(again.store.saves(TEE).len(), 1);
        assert_eq!(again.store.saves(TEE)[0].sides.len(), 1, "the blank side is left out");
    }

    #[test]
    fn a_side_of_one_colour_is_a_design_that_is_worn_and_saved() {
        let mut e = tee();
        e.sides[0].doc.background = [20, 90, 200, 255];
        let designs = e.designs();
        assert!(designs[0].1.is_some() && designs[1].1.is_none());
        let saved = e.to_save();
        assert_eq!(saved.sides.keys().collect::<Vec<_>>(), ["front"]);
        e.sides[0].doc.background = [0; 4];
        e.sides[0].doc.layers.push(Layer::new(Art::Text(String::new()), [255; 4]));
        assert!(design(&e.sides[0].doc).is_none());
    }
}
