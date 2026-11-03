//! The Graphic Creator, inside Edit Skater: the movie's own card shows the editor's pages
//! (`graphic_pages`: Layers, Add, Options, Menu), a second card beside it the whole side being edited
//! (`graphic_board`), the retail Graphic Adjust legend sits under the skater, and the skater wears the
//! design as it changes. This file keeps the session: opening on a Graphic Location, sharing a frame's
//! input between the movie's page and the editor, keeping the page in step, and leaving.
use crate::{
    apt_scene::Draw,
    cas_menu,
    core_menu_runtime::{Key, SOUND_B_BUTTON, SOUND_NAV_DOWN, SOUND_NAV_UP},
    create_skater_runtime::{Outcome, Runtime},
    custom_graphic::{self as graphic, Art, Doc, Layer},
    customiser::Customiser,
    customiser_parts::Parts,
    graphic_board::{self as board, BoardArt},
    graphic_outline::OutlineMask,
    graphic_editor::{Ask, Catalog, Editor, Exit, Mode, Pad, Pointer, Repeat, Shortcut, SideDoc, Source, Typed},
    graphic_pages::{self as pages, Thumbs},
    graphic_store::{self, Store},
    graphic_tile::{Job, Tiles},
};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::path::PathBuf;

/// The skater follows edits at most this often (each follow draws new side textures).
const SETTLE: f32 = 0.25;
/// Layer and shape pictures, pixels.
pub(crate) const THUMB: u32 = 56;
/// Layer pictures kept at once: past this, one no layer on the side shows gives up its texture.
const THUMBS_KEPT: usize = 128;
/// While a gesture changes the side, the board is drawn again at most this often, seconds.
pub(crate) const BOARD_EVERY: f32 = 1. / 30.;
/// Wheel notches this close together are one undo step, seconds.
const WHEEL_BURST: f32 = 0.5;

#[derive(Resource, Default)]
pub(crate) struct Creator {
    pub session: Option<Session>,
    /// The creator just closed: Edit Skater redraws its page and ignores that press.
    pub closed: bool,
    /// ...and it closed with Done (what it left on the skater stays), not Discard.
    pub committed: bool,
    /// After it closes: the draft's graphics the skater has to wear before the outfit is sent to other
    /// players again, and since when. Nothing between the creator's edit steps (or a step the skater
    /// has not caught up with) is ever sent.
    awaiting: Option<(Value, std::time::Instant)>,
}
pub(crate) struct Session {
    pub editor: Editor,
    /// The skater as the creator found it (Discard).
    before: Value,
    pub profile: PathBuf,
    /// The draft's slot the item is worn in: its colour is the board's.
    pub slot: String,
    /// Seconds since the skater was last shown the design, while it is behind.
    pending: Option<f32>,
    worn_rev: u64,
    /// The side the skater is turned to.
    shown_side: usize,
    /// The pictures the pages name, and the board's art (captured from the movie on the first frame).
    pub thumbs: Thumbs,
    pub art: Option<BoardArt>,
    /// The board texture was drawn for this (design, side).
    pub drawn: Option<(u64, usize)>,
    /// The garment's outline of each side (Stage 3), and the side whose outline texture is up.
    pub outlines: std::collections::HashMap<&'static str, OutlineMask>,
    pub outline_shown: Option<&'static str>,
    /// The movie's page's D-pad, and the last pad (A / B edges).
    dpad: Repeat,
    prev: Pad,
    /// The popup was asked for the editor's current question.
    asked: bool,
    /// The mouse: the list row being dragged (its layer), and a press that began on the board.
    row_drag: Option<usize>,
    board_drag: bool,
    /// Seconds since the last wheel notch of a burst (`None`: no burst).
    wheel_idle: Option<f32>,
    /// Seconds since the board texture was last drawn.
    pub board_age: f32,
    /// The garment model's secondary-UV triangles, read once per item (`None` inside: unreadable).
    pub garment_uv: Option<Option<Vec<[[f32; 2]; 3]>>>,
}

impl Creator {
    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }
    /// LT / RT are the board's (fine, reorder), not Rotate Skater: everywhere but the Add page.
    pub fn owns_triggers(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.editor.mode != Mode::Add)
    }
    /// Whether the outfit must not be sent to other players now: while the creator is open, and after
    /// it closes until the skater wears what the draft says (or a few seconds pass).
    pub fn holds_publish(&mut self, applied: &Value) -> bool {
        if self.session.is_some() {
            return true;
        }
        let Some((wanted, since)) = &self.awaiting else { return false };
        let worn = graphic::ZONES.iter().all(|z| graphic::design(&applied["graphics"][z.0]) == graphic::design(&wanted[z.0]));
        if worn || since.elapsed() > std::time::Duration::from_secs(3) {
            self.awaiting = None;
            return false;
        }
        true
    }
    /// The item that takes the Graphic Location `zone`, as the editor: its sides with the designs
    /// the draft wears; and the slot it is worn in.
    fn editor(zone: &str, customiser: &Customiser, parts: &Parts) -> Option<(Editor, String)> {
        let (slot, worn) = customiser.draft["selections"].as_object()?.iter().find(|(_, w)| cas_menu::graphic_locations(parts, w).iter().any(|l| l.0 == zone))?;
        let item = worn["asset_id"].as_str()?;
        let sides: Vec<SideDoc> = cas_menu::graphic_locations(parts, worn)
            .into_iter()
            .map(|(zone, rect)| SideDoc {
                zone,
                rect,
                doc: customiser.draft["graphics"][zone]["design"].as_str().and_then(|t| Doc::decode_any(t).ok()).unwrap_or_default(),
            })
            .collect();
        let start = sides.iter().position(|s| s.zone == zone)?;
        let name = parts.library.models.get(item).map(|m| m.name.clone()).unwrap_or_default();
        Some((Editor::new(item, &name, sides, start, catalog(parts), Store::load(customiser.profile_path())), slot.clone()))
    }
    fn begin(&mut self, (editor, slot): (Editor, String), customiser: &mut Customiser, before: Value) {
        customiser.view = Some(view_of(editor.zone()));
        self.awaiting = None;
        self.session = Some(Session {
            before,
            profile: customiser.profile_path().to_path_buf(),
            slot,
            pending: None,
            worn_rev: editor.doc_rev,
            shown_side: editor.side,
            thumbs: Thumbs::default(),
            art: None,
            drawn: None,
            outlines: Default::default(),
            outline_shown: None,
            dpad: Repeat::default(),
            // The press that opened the creator is not the board's.
            prev: Pad { a: true, b: true, ..Pad::default() },
            asked: false,
            row_drag: None,
            board_drag: false,
            wheel_idle: None,
            board_age: 0.,
            garment_uv: None,
            editor,
        });
    }
    /// Open on a Graphic Location, editing the designs the item wears (a blank canvas where none).
    /// ponytail: tests only since Custom Graphics became the one way in (New / Edit).
    #[cfg(test)]
    pub fn open(&mut self, zone: &'static str, customiser: &mut Customiser, parts: &Parts) -> bool {
        let Some(found) = Self::editor(zone, customiser, parts) else { return false };
        let before = customiser.draft.clone();
        self.begin(found, customiser, before);
        true
    }
    /// Open a saved Custom Graphic of the worn item, wearing it (saving again overwrites it).
    pub fn open_saved(&mut self, zone: &'static str, customiser: &mut Customiser, parts: &Parts, number: usize) -> bool {
        let Some((editor, slot)) = Self::editor(zone, customiser, parts) else { return false };
        let Some(saved) = editor.store.saves(&editor.item).get(number).map(|s| (*s).clone()) else { return false };
        let mut editor = editor.edit(number, &saved);
        // Not retail (user 10-19): opened on a side the save leaves blank, it starts on the side its design is on
        // (the side its tile shows).
        if crate::graphic_editor::design(editor.doc()).is_none() {
            let shown = crate::graphic_tile::shown_zone(&saved);
            if let Some(side) = editor.sides.iter().position(|s| s.zone == shown) {
                editor.set_side(side);
            }
        }
        // Discard puts the skater back as it was before this, not as the save.
        let before = customiser.draft.clone();
        wear(&editor, customiser, parts, &before);
        self.begin((editor, slot), customiser, before);
        true
    }
    /// New: blank sides on the worn item, and Start From (Blank, a layout, another item's save).
    pub fn open_new(&mut self, zone: &'static str, customiser: &mut Customiser, parts: &Parts) -> bool {
        let Some((mut editor, slot)) = Self::editor(zone, customiser, parts) else { return false };
        for side in &mut editor.sides {
            side.doc = Doc::default();
        }
        editor.layer = 0;
        let profile = customiser.profile_path();
        let sources: Vec<Source> = editor
            .store
            .items()
            .filter(|(item, _)| *item != editor.item)
            .flat_map(|(item, saves)| {
                let name = parts.library.models.get(item).map_or_else(|| item.to_owned(), |m| m.name.clone());
                saves.into_iter().enumerate().map(move |(n, saved)| {
                    let png = graphic_store::tile_file(profile, item, saved);
                    let tile = if png.exists() { png.to_string_lossy().into_owned() } else { String::new() };
                    Source { label: format!("{name} {}", n + 1), tile, saved: saved.clone() }
                })
            })
            .collect();
        editor.start_new(sources);
        let before = customiser.draft.clone();
        wear(&editor, customiser, parts, &before);
        self.begin((editor, slot), customiser, before);
        true
    }
    /// Leave: Done keeps what the skater wears, Discard puts it back as the creator found it. A save made
    /// on the way out is written first; if that fails the creator stays open and says so (false).
    pub fn finish(&mut self, exit: Exit, customiser: &mut Customiser, parts: &Parts, tiles: &mut Tiles) -> bool {
        if !self.session.as_mut().is_some_and(|s| s.flush(tiles)) {
            return false;
        }
        let Some(s) = self.session.take() else { return false };
        match exit {
            Exit::Done => wear(&s.editor, customiser, parts, &s.before),
            Exit::Discard => {
                customiser.draft = s.before;
                customiser.redraw = true;
            }
        }
        self.committed = exit == Exit::Done;
        self.awaiting = Some((customiser.draft["graphics"].clone(), std::time::Instant::now()));
        tiles.hold = false;
        self.closed = true;
        true
    }
    /// Edit Skater closed under the creator: nothing more is done.
    pub fn drop_session(&mut self, tiles: &mut Tiles) {
        self.session = None;
        tiles.hold = false;
    }
    /// The editor's question for the popup, once each time it asks. Without the popup's art (`art`) nothing
    /// is asked: the editor takes B (nothing chosen unseen; a notice stays in the info panel).
    pub fn question(&mut self, art: bool) -> Option<Ask> {
        let s = self.session.as_mut()?;
        match s.editor.mode {
            Mode::Popup(_) if !art => {
                s.editor.answer(None);
                None
            }
            Mode::Popup(ask) if !s.asked => {
                s.asked = true;
                Some(ask)
            }
            Mode::Popup(_) => None,
            _ => {
                s.asked = false;
                None
            }
        }
    }
    /// The popup's answer (`None`: B).
    pub fn answer(&mut self, row: Option<usize>) -> Option<Exit> {
        let s = self.session.as_mut()?;
        s.asked = false;
        s.editor.answer(row)
    }
    /// The frame's presses for the movie's page, and the pad the editor is given (without them).
    pub fn route(&mut self, pad: Pad, popup: bool) -> (Pad, Vec<Key>) {
        let Some(s) = self.session.as_mut() else { return (pad, Vec::new()) };
        let dpad = s.dpad.step([pad.up, pad.down, pad.left, pad.right], pad.dt);
        let (a, b) = (pad.a && !s.prev.a, pad.b && !s.prev.b);
        s.prev = pad;
        route(s.editor.mode, pad, dpad, a, b, popup)
    }
    /// Everything drawn over the movie's page while the creator is open: the board card, the legend,
    /// X Options and the layers' pictures. `first`: the Layers list's first row on screen.
    pub fn draws(&self, mut movie: Vec<Draw>, first: usize, language: &Value, garment: [f32; 4]) -> Vec<Draw> {
        let Some(s) = self.session.as_ref() else { return movie };
        let Some(art) = s.art.as_ref() else { return movie };
        let e = &s.editor;
        if e.mode != Mode::Add {
            movie.retain(|d| !board::is_rotate_skater(d));
        }
        // X Options only where X is Options (typing, X is Backspace).
        if e.mode == Mode::Board {
            movie.extend(board::x_options(&movie, art, language));
        }
        if matches!(e.mode, Mode::Board | Mode::Text | Mode::Popup(_)) {
            let rows: Vec<(String, [u32; 2])> = e.doc().layers.iter().rev().map(|l| (s.thumbs.picture(l), [THUMB, THUMB])).collect();
            movie.extend(board::layer_thumbs(&rows, first));
        }
        let outline = s.outlines.get(e.zone()).filter(|m| m.edges.contains(&1) && s.outline_shown == Some(e.zone()));
        movie.extend(board::board(e, art, garment, outline, e.grid, language));
        movie.extend(board::legend(e, art, language));
        movie
    }
}

/// Who takes which press. The movie's page navigates (its D-pad, A and B, with its own sounds); the
/// editor gets the rest. On the board the D-pad's Left / Right change side, and with RT held Up / Down
/// move the layer; a popup's presses go to the popup (`popup`: its art is there).
pub(crate) fn route(mode: Mode, mut pad: Pad, [up, down, left, right]: [bool; 4], a: bool, b: bool, popup: bool) -> (Pad, Vec<Key>) {
    let mut keys = Vec::new();
    let mut take = |on: bool, key: Key| {
        if on {
            keys.push(key);
        }
    };
    match mode {
        Mode::Text => return (pad, keys),
        Mode::Popup(_) if !popup => return (pad, keys),
        Mode::Board => {
            if !pad.rt {
                take(up, Key::Up);
                take(down, Key::Down);
                (pad.up, pad.down) = (false, false);
            }
        }
        _ => {
            take(up, Key::Up);
            take(down, Key::Down);
            take(left, Key::Left);
            take(right, Key::Right);
            (pad.up, pad.down, pad.left, pad.right) = (false, false, false, false);
        }
    }
    take(a, Key::Accept);
    take(b, Key::Back);
    (pad.a, pad.b) = (false, false);
    (pad, keys)
}

/// One frame of input: the pad to the editor, then the page's presses to the movie, each outcome back to
/// the editor. The editor's sounds, the movie's (and the native B) are added to `sounds`.
pub(crate) fn feed(editor: &mut Editor, runtime: &mut Runtime, pad: &Pad, keys: &[Key], typed: &[Typed], sounds: &mut Vec<u64>) -> Result<Option<Exit>, String> {
    let mut exit = editor.input(pad, typed);
    sounds.append(&mut editor.sounds);
    for key in keys {
        if exit.is_some() {
            break;
        }
        let shown = (runtime.bindings.page.index, runtime.bindings.page.rows.len());
        if matches!(key, Key::Up | Key::Down) && pages::step_past(editor, shown, *key == Key::Down) {
            sounds.push(if *key == Key::Down { SOUND_NAV_DOWN } else { SOUND_NAV_UP });
            pages::scroll(editor);
            runtime.show(pages::page(editor, &Thumbs::default()))?;
            continue;
        }
        let outcome = runtime.input(*key)?;
        sounds.append(&mut runtime.bindings.sounds);
        if outcome == Some(Outcome::Back) {
            sounds.push(SOUND_B_BUTTON);
        }
        if let Some(outcome) = outcome {
            exit = pages::apply(editor, outcome);
        }
    }
    if std::mem::take(&mut editor.pasting) {
        editor.paste(&crate::share_code::paste_from_clipboard().unwrap_or_default());
    }
    Ok(exit)
}

/// Keep the movie's page in step with the editor: nothing when it already shows it, the highlighted row
/// alone when only its value changed, else the whole page.
pub(crate) fn sync(editor: &mut Editor, runtime: &mut Runtime, thumbs: &Thumbs) -> Result<(), String> {
    pages::scroll(editor);
    let next = pages::page(editor, thumbs);
    let shown = &runtime.bindings.page;
    let same_page = shown.mode == next.mode && shown.title == next.title && shown.accept == next.accept && shown.back == next.back && shown.rows.len() == next.rows.len();
    if same_page && shown.index == next.index {
        let changed: Vec<usize> = (0..next.rows.len()).filter(|i| shown.rows[*i] != next.rows[*i]).collect();
        match changed[..] {
            [] => return Ok(()),
            [i] if i == next.index => return runtime.refresh_row(next.rows[i].clone()),
            _ => {}
        }
    }
    runtime.show(next)
}

pub(crate) fn view_of(zone: &str) -> (f32, f32, f32) {
    cas_menu::Frame::of_zone(zone).view()
}
/// The stock logos and the game's tattoos and decals the picker offers.
fn catalog(parts: &Parts) -> Catalog {
    let mut logos: Vec<(&str, u64, String)> = parts
        .library
        .graphics
        .iter()
        .filter_map(|(id, g)| Some((g.kind.as_str(), graphic::logo_key(id)?, g.name.clone())))
        .collect();
    logos.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
    let mut art: Vec<(u64, String)> = parts.library.tattoos.iter().filter_map(|(id, t)| Some((graphic::logo_key(id)?, t.name.clone()))).collect();
    art.sort_by(|a, b| a.1.cmp(&b.1));
    Catalog { logos: logos.into_iter().map(|(_, key, name)| (key, name)).collect(), art }
}
/// Show every side's design on the skater (Edit Skater's draft) without leaving the creator. On Done
/// (`done`) a side that holds a design also takes its stock logo off: one graphic per location, as
/// Edit Skater's Graphic Location rows keep it (the logo shows under the design only while editing).
/// What the skater wears, live and on Done: a side holding a design takes its stock logo off; a side left
/// blank wears the logo it had when the creator opened (`before`).
pub(crate) fn wear(editor: &Editor, customiser: &mut Customiser, parts: &Parts, before: &Value) {
    let sides: serde_json::Map<String, Value> = editor
        .designs()
        .into_iter()
        .map(|(zone, text)| {
            let logo = if text.is_some() { Value::Null } else { before["graphics"][zone]["logo"].clone() };
            let mut side = json!({ "design": text });
            if text.is_some() || logo.is_string() {
                side["logo"] = logo;
            }
            (zone.to_owned(), side)
        })
        .collect();
    customiser.apply(parts, Some(&json!({ "graphics": sides })));
}
/// One layer alone, centred and fitted to a thumbnail's canvas.
pub(crate) fn thumb_doc(layer: &Layer) -> Doc {
    let reach = f32::from(layer.w).hypot(f32::from(layer.h)).max(1.);
    let s = 3400. / reach;
    let size = |v: u16| (f32::from(v) * s).round().max(f32::from(graphic::SIZES.0)) as u16;
    Doc { layers: vec![Layer { x: 0, y: 0, w: size(layer.w), h: size(layer.h), ..layer.clone() }], background: [0; 4] }
}
/// The board's canvas colour: the garment's colour where the draft sets one, else mid-grey.
pub(crate) fn garment(customiser: &Customiser, slot: &str) -> [f32; 4] {
    let rgb = serde_json::from_value::<[f32; 3]>(customiser.draft["colours"][slot].clone()).unwrap_or([0.5; 3]);
    [rgb[0], rgb[1], rgb[2], 1.]
}

/// The cursor from window to movie coordinates: the 1280x720 stage fits inside the window, centred
/// (`apt_canvas`'s AutoMin projection).
pub(crate) fn to_movie(cursor: Vec2, window: Vec2) -> Vec2 {
    let scale = (window.x / 1280.).min(window.y / 720.).max(1e-6);
    (cursor - (window - Vec2::new(1280., 720.) * scale) / 2.) / scale
}
/// This frame's mouse, in movie coordinates.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mouse {
    pub at: Vec2,
    /// A button went down this frame (`right`: the right one), is held, or came up.
    pub press: bool,
    pub right: bool,
    pub held: bool,
    pub release: bool,
    /// Wheel lines, up positive.
    pub wheel: f32,
    pub shift: bool,
    pub alt: bool,
    /// The frame's seconds (a wheel burst ends `WHEEL_BURST` after its last notch).
    pub dt: f32,
}
/// The PC keyboard standing in for the pad (nothing while typing: the keys type). The buttons are this
/// frame's presses, not held keys: Enter or Esc that ends typing, still held, is no A or B on the page
/// after. Ctrl+ keys are shortcuts, and on the board the arrows nudge: neither stands in for the pad.
pub(crate) fn keyboard_pad(keys: &ButtonInput<KeyCode>, typing: bool, on_board: bool) -> Pad {
    let free = !typing && !keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let held = |code: KeyCode| free && keys.pressed(code);
    let tap = |code: KeyCode| free && keys.just_pressed(code);
    let arrow = |code: KeyCode| !on_board && held(code);
    Pad {
        lb: held(KeyCode::PageUp),
        rb: held(KeyCode::PageDown),
        lt: held(KeyCode::ShiftLeft),
        up: arrow(KeyCode::ArrowUp),
        down: arrow(KeyCode::ArrowDown),
        left: arrow(KeyCode::ArrowLeft),
        right: arrow(KeyCode::ArrowRight),
        a: tap(KeyCode::Enter),
        b: tap(KeyCode::Escape) || tap(KeyCode::Backspace),
        x: tap(KeyCode::KeyX),
        y: tap(KeyCode::KeyY),
        back: tap(KeyCode::KeyZ),
        // M is Mirror (a shortcut); Tab opens the Menu.
        start: tap(KeyCode::Tab),
        ..Pad::default()
    }
}
/// The keyboard's shortcuts this frame (the Menu's Help lists them). `down`: pressed this frame.
pub(crate) fn shortcuts(ctrl: bool, shift: bool, down: impl Fn(KeyCode) -> bool) -> Vec<Shortcut> {
    let mut out = Vec::new();
    let keys: &[(KeyCode, Shortcut)] = if ctrl {
        &[(KeyCode::KeyZ, Shortcut::Undo), (KeyCode::KeyY, Shortcut::Redo), (KeyCode::KeyD, Shortcut::Duplicate), (KeyCode::KeyC, Shortcut::Copy), (KeyCode::KeyV, Shortcut::Paste)]
    } else {
        &[
            (KeyCode::KeyM, Shortcut::Mirror),
            (KeyCode::Delete, Shortcut::Delete),
            (KeyCode::ArrowLeft, Shortcut::Nudge(-1, 0, false)),
            (KeyCode::ArrowRight, Shortcut::Nudge(1, 0, false)),
            (KeyCode::ArrowUp, Shortcut::Nudge(0, -1, false)),
            (KeyCode::ArrowDown, Shortcut::Nudge(0, 1, false)),
        ]
    };
    for (key, shortcut) in keys {
        if down(*key) {
            out.push(match *shortcut {
                Shortcut::Nudge(x, y, _) => Shortcut::Nudge(x, y, shift),
                s => s,
            });
        }
    }
    out
}

impl Session {
    /// The mouse: on the board `Editor::pointer` (select, drag, handles, wheel, right-click Options);
    /// on the Layers list a click selects and a drag reorders; on the header a click changes side.
    /// `first`: the list's first row on screen.
    pub fn mouse(&mut self, m: Mouse, first: usize) {
        let Session { art, editor: e, row_drag, board_drag, wheel_idle, .. } = self;
        // Wheel notches close together are one undo step, as one drag is.
        if let Some(idle) = wheel_idle.as_mut().filter(|_| m.wheel == 0.) {
            *idle += m.dt;
            if *idle >= WHEEL_BURST {
                *wheel_idle = None;
                e.burst(false);
            }
        }
        let extent = e.extent();
        let f = board::frame(extent, e.mode);
        e.view_scale = extent.x / (f[2] - f[0]).max(1.);
        let canvas = (m.at - Vec2::new((f[0] + f[2]) / 2., (f[1] + f[3]) / 2.)) * e.view_scale;
        // The board takes the side's room and a little round it (handles stick out); not the header or legend.
        let on_board = m.at.x >= board::CARD[0] && m.at.x <= board::CARD[2] && m.at.y >= (f[1] - 40.).max(104.) && m.at.y <= 622.;
        let count = e.doc().layers.len();
        let layer_at = |at: Vec2| board::list_row(at, first).filter(|r| *r < count).map(|r| count - 1 - r);
        if m.press && e.mode == Mode::Board {
            if wheel_idle.take().is_some() {
                e.burst(false);
            }
            if on_board {
                e.pointer(Pointer::Press { at: canvas, right: m.right, alt: m.alt });
                *board_drag = !m.right;
            } else if let Some(layer) = layer_at(m.at).filter(|_| !m.right) {
                e.select(layer);
                e.mouse = true;
                *row_drag = Some(layer);
                // However many rows the drag crosses, it is one undo step.
                e.burst(true);
            } else if let Some(side) = art.as_ref().and_then(|art| board::side_at(e, art, m.at)).filter(|_| !m.right) {
                e.set_side(side);
            }
        } else if m.held {
            if let Some(from) = *row_drag {
                if let Some(to) = layer_at(m.at).filter(|to| *to != from) {
                    e.reorder(from, to);
                    *row_drag = Some(to);
                }
            } else if *board_drag {
                e.pointer(Pointer::Drag { at: canvas, shift: m.shift });
            }
        }
        if m.release {
            if *board_drag {
                e.pointer(Pointer::Release);
            }
            if row_drag.is_some() {
                e.burst(false);
            }
            (*row_drag, *board_drag) = (None, false);
        }
        if m.wheel != 0. && on_board && e.mode == Mode::Board {
            if wheel_idle.is_none() {
                e.burst(true);
            }
            *wheel_idle = Some(0.);
            e.pointer(Pointer::Wheel { at: canvas, lines: m.wheel, shift: m.shift });
        }
    }
    /// After the frame's input: saves to disk, the skater following the design and turning to the side.
    pub fn follow(&mut self, customiser: &mut Customiser, parts: &Parts, tiles: &mut Tiles, dt: f32) {
        tiles.hold = true;
        self.flush(tiles);
        if self.editor.doc_rev != self.worn_rev {
            let waited = self.pending.get_or_insert(0.);
            *waited += dt;
            if *waited >= SETTLE {
                wear(&self.editor, customiser, parts, &self.before);
                self.worn_rev = self.editor.doc_rev;
                self.pending = None;
            }
        }
        if self.editor.side != self.shown_side {
            self.shown_side = self.editor.side;
            customiser.view = Some(view_of(self.editor.zone()));
        }
    }
    /// A save the editor just made: to disk, and its Custom Graphics tile asked for. A write that fails
    /// asks the retail popup (the save stays in the list in memory: the next save writes it) and is false.
    fn flush(&mut self, tiles: &mut Tiles) -> bool {
        let Some(saved) = self.editor.take_saved() else { return true };
        match self.editor.store.save(&self.profile) {
            Ok(()) => {
                let path = graphic_store::tile_file(&self.profile, &self.editor.item, &saved);
                tiles.request(Job { item: self.editor.item.clone(), saved, path });
                true
            }
            Err(error) => {
                error!("Graphic Creator save: {error}");
                self.editor.save_failed(&error.to_string());
                false
            }
        }
    }
    /// The texture key for a layer picture `look`: a new one, or past `THUMBS_KEPT` one that no layer on
    /// the side shows any more (its texture is replaced, so the pictures never pile up).
    pub fn thumb_key(&mut self, look: u64) -> String {
        if self.thumbs.layers.len() >= THUMBS_KEPT {
            let used: std::collections::HashSet<u64> = self.editor.doc().layers.iter().map(pages::look_key).collect();
            if let Some(old) = self.thumbs.layers.keys().copied().find(|k| !used.contains(k)) {
                let key = self.thumbs.layers.remove(&old).unwrap_or_default();
                self.thumbs.layers.insert(look, key.clone());
                return key;
            }
        }
        let key = format!("creator/look/{}", self.thumbs.layers.len());
        self.thumbs.layers.insert(look, key.clone());
        key
    }
    /// Layer pictures not drawn yet: (key, layer) for the screen to render at `THUMB`.
    pub fn missing_thumbs(&self) -> Vec<(u64, Layer)> {
        let mut out: Vec<(u64, Layer)> = Vec::new();
        for l in self.editor.doc().layers.iter().filter(|l| !matches!(l.art, Art::Text(_))) {
            let key = pages::look_key(l);
            if !self.thumbs.layers.contains_key(&key) && !out.iter().any(|(k, _)| *k == key) {
                out.push((key, l.clone()));
            }
        }
        out
    }
}

/// The shape a picker tile shows, in white.
pub(crate) fn shape_doc(index: usize) -> Doc {
    Doc { layers: vec![Layer { w: 3400, h: 3400, ..Layer::new(Art::Shape(index as u8), [255; 4]) }], background: [0; 4] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The installed library's customizable tee (its real id and name).
    const TEE: &str = "801095192c838106";
    /// A tiny library: one customizable tee with its four sides.
    fn parts() -> Parts {
        let mut parts = Parts::default();
        let lib = &mut parts.library;
        let flags: std::collections::HashMap<String, String> = [
            ("TopType", "tshirt"),
            ("StampUVConstraintQ1", "0.98,0.48,0.13,0.37"),
            ("StampUVConstraintQ2", "0.98,0.48,0.64,0.86"),
            ("StampUVConstraintQ3", "0.48,0.25,0.05,0.45"),
            ("StampUVConstraintQ4", "0.48,0.25,0.55,0.95"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        lib.models.insert(TEE.into(), crate::customiser_parts::Part { slot: "OuterTorso".into(), name: "Tee".into(), flags, materials: vec!["white".into()], scene: String::new() });
        lib.models.insert("head".into(), crate::customiser_parts::Part { slot: "Rostral".into(), name: "Head".into(), flags: Default::default(), materials: vec!["white".into()], scene: String::new() });
        lib.materials.insert(
            "white".into(),
            serde_json::from_value(json!({"name": "White", "flags": {"IsLogoable": "true"}, "diffuse": "x.png", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1})).unwrap(),
        );
        lib.defaults.insert("male".into(), json!({"gender": "male", "selections": {"OuterTorso": {"asset_id": TEE, "material_id": "white"}, "Rostral": {"asset_id": "head", "material_id": "white"}}}));
        lib.graphics.insert(
            "00000000000000aa".into(),
            serde_json::from_value(json!({"name": "Brand", "kind": "brand", "texture": "x/brand.png"})).unwrap(),
        );
        parts
    }
    fn customiser(parts: &Parts, name: &str) -> Customiser {
        // ponytail: one folder per test, wiped and reused (a pid suffix left one per run). Two test
        // runs at once would share it.
        let dir = std::env::temp_dir().join(format!("skate-creator-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut customiser = Customiser::for_test(parts, Vec::new(), "male");
        customiser.set_profile_path(dir.join("profile.json"));
        customiser
    }

    #[test]
    fn it_opens_on_the_worn_item_with_its_four_sides_and_turns_the_skater_to_the_side() {
        let parts = parts();
        let mut customiser = customiser(&parts, "open");
        let mut creator = Creator::default();
        assert!(!creator.open("deck", &mut customiser, &parts), "no deck is worn");
        assert!(creator.open("back", &mut customiser, &parts));
        let s = creator.session.as_ref().unwrap();
        let editor = &s.editor;
        assert_eq!((editor.item.as_str(), editor.name.as_str(), editor.zone(), s.slot.as_str()), (TEE, "Tee", "back", "OuterTorso"));
        assert_eq!(editor.sides.iter().map(|s| s.zone).collect::<Vec<_>>(), ["front", "back", "left", "right"]);
        assert_eq!(editor.catalog.logos, [(0xaa, "Brand".to_owned())]);
        assert_eq!(customiser.view, Some(cas_menu::Frame::Back.view()));
        assert!(creator.is_open() && creator.owns_triggers());
        assert_eq!(garment(&customiser, "OuterTorso"), [0.5, 0.5, 0.5, 1.], "no colour set: mid-grey");
    }

    #[test]
    fn discard_after_new_or_edit_puts_the_skater_back_as_it_was_before_and_done_is_held_back_from_other_players() {
        let parts = parts();
        let mut customiser = customiser(&parts, "before");
        // The skater wears a design on the front before anything is opened.
        let worn = Doc { layers: vec![Layer::new(Art::Shape(2), [9, 9, 9, 255])], ..default() }.encode();
        customiser.draft["graphics"] = json!({"front": {"design": worn}});
        let original = customiser.draft.clone();
        let mut creator = Creator::default();
        // New wears a blank canvas, but Discard goes back to the look before New.
        assert!(creator.open_new("front", &mut customiser, &parts));
        assert!(customiser.draft["graphics"]["front"]["design"].is_null(), "New starts blank");
        assert_eq!(creator.session.as_ref().unwrap().before, original);
        let mut tiles = Tiles::default();
        creator.finish(Exit::Discard, &mut customiser, &parts, &mut tiles);
        assert_eq!(customiser.draft, original);
        assert!(creator.closed && !creator.committed && !creator.is_open());
        // Edit of a save: the same, even though the save is worn while it is edited.
        let mut store = Store::default();
        store.add(TEE, graphic_store::Saved { sides: [("back".to_owned(), worn.clone())].into() }).unwrap();
        store.save(customiser.profile_path()).unwrap();
        assert!(creator.open_saved("front", &mut customiser, &parts, 0));
        assert_eq!(creator.session.as_ref().unwrap().before, original);
        // User 10-19: opened from the front, a save with only a back design starts on the back.
        assert_eq!(creator.session.as_ref().unwrap().editor.zone(), "back");
        assert_eq!(customiser.view, Some(cas_menu::Frame::Back.view()), "the skater turned to it");
        assert_eq!(customiser.draft["graphics"]["back"]["design"], json!(worn), "the save is worn while edited");

        // Nothing is sent while the creator is open; after it closes, only once the skater wears the draft.
        let applied = |front: Option<&str>| json!({"graphics": {"front": {"design": front}}});
        assert!(creator.holds_publish(&applied(None)), "open");
        creator.session = None;
        creator.awaiting = Some((json!({"front": {"design": "FINAL"}}), std::time::Instant::now()));
        assert!(creator.holds_publish(&applied(Some("an edit step"))), "the skater still wears an earlier step");
        assert!(creator.holds_publish(&applied(None)));
        assert!(!creator.holds_publish(&applied(Some("FINAL"))), "now it wears the committed look");
        assert!(!creator.holds_publish(&applied(Some("an edit step"))), "and the hold is gone");
        // A skater that never catches up does not stay silent for good.
        creator.awaiting = Some((json!({"front": {"design": "FINAL"}}), std::time::Instant::now().checked_sub(std::time::Duration::from_secs(4)).unwrap()));
        assert!(!creator.holds_publish(&applied(None)));
    }

    #[test]
    fn a_design_takes_the_stock_logo_off_its_side_live_and_on_done() {
        let parts = parts();
        let mut tiles = Tiles::default();
        for (exit, side, logo) in [(Exit::Done, 1, None), (Exit::Discard, 1, None), (Exit::Done, 0, Some("00000000000000aa"))] {
            let mut customiser = customiser(&parts, "logo-off");
            customiser.apply(&parts, Some(&json!({"graphics": {"back": {"logo": "00000000000000aa"}}})));
            assert_eq!(customiser.draft["graphics"]["back"]["logo"], "00000000000000aa");
            let mut creator = Creator::default();
            assert!(creator.open_new("back", &mut customiser, &parts));
            let s = creator.session.as_mut().unwrap();
            s.editor.sides[side].doc = Doc { layers: vec![Layer::new(Art::Shape(2), [200, 10, 10, 255])], ..default() };
            s.editor.doc_rev += 1;
            s.follow(&mut customiser, &parts, &mut tiles, SETTLE);
            assert_eq!(customiser.draft["graphics"]["back"]["logo"].as_str(), logo, "live, with a design on side {side}");
            if side == 1 {
                // Cleared again: the side wears its logo back.
                let s = creator.session.as_mut().unwrap();
                s.editor.sides[1].doc = Doc::default();
                s.editor.doc_rev += 1;
                s.follow(&mut customiser, &parts, &mut tiles, SETTLE);
                assert_eq!(customiser.draft["graphics"]["back"]["logo"], "00000000000000aa");
                let s = creator.session.as_mut().unwrap();
                s.editor.sides[1].doc = Doc { layers: vec![Layer::new(Art::Shape(2), [200, 10, 10, 255])], ..default() };
                s.editor.doc_rev += 1;
            }
            creator.finish(exit, &mut customiser, &parts, &mut tiles);
            let kept = if exit == Exit::Discard { Some("00000000000000aa") } else { logo };
            assert_eq!(customiser.draft["graphics"]["back"]["logo"].as_str(), kept, "{exit:?} with a design on side {side}");
        }
    }

    fn design(x: i16) -> String {
        Doc { layers: vec![Layer { x, ..Layer::new(Art::Shape(2), [200, 10, 10, 255]) }], ..default() }.encode()
    }

    #[test]
    fn start_from_another_items_save_copies_matching_sides() {
        let parts = parts();
        let mut customiser = customiser(&parts, "start");
        let mut store = Store::default();
        let saved = |sides: &[(&str, String)]| graphic_store::Saved { sides: sides.iter().map(|(z, d)| ((*z).to_owned(), d.clone())).collect() };
        store.add("other", saved(&[("front", design(10)), ("hat", design(20))])).unwrap();
        store.add("cap", saved(&[("hat", design(30))])).unwrap();
        store.add(TEE, saved(&[("front", design(40))])).unwrap();
        store.save(customiser.profile_path()).unwrap();
        let mut creator = Creator::default();
        assert!(creator.open_new("front", &mut customiser, &parts));
        let e = &mut creator.session.as_mut().unwrap().editor;
        assert_eq!(e.mode, Mode::StartFrom, "New starts on Start From");
        let p = pages::page(e, &Thumbs::default());
        let rows: Vec<&str> = p.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(rows, ["#Blank", "#Chest Logo", "#Big Back Print", "#Sleeve Stripes", "#From Another Item...", "#Paste Code"]);
        // Only another item's save with a side the tee has; the tee's own and the cap's are not offered.
        assert_eq!(e.sources.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["other 1"]);
        e.start_from(4);
        assert!(e.from_other && e.mode == Mode::StartFrom);
        assert_eq!(pages::page(e, &Thumbs::default()).rows[0].text, "#other 1");
        assert_eq!(pages::apply(e, Outcome::Back), None, "B: back to Start From");
        assert!(!e.from_other && e.start == 4);
        e.start_from(4);
        pages::apply(e, Outcome::Accept(0));
        assert_eq!(e.mode, Mode::Board);
        assert_eq!(e.sides[0].doc.encode(), design(10), "the matching side is copied");
        assert!(e.sides[1..].iter().all(|s| s.doc.layers.is_empty()), "the rest stay blank");
        assert!(e.changed);
        // The layouts: Chest Logo on the front, Sleeve Stripes on both sleeves.
        e.start_new(Vec::new());
        for side in &mut e.sides {
            side.doc = Doc::default();
        }
        e.start_from(1);
        assert_eq!(e.sides.iter().map(|s| s.doc.layers.len()).collect::<Vec<_>>(), [2, 0, 0, 0]);
        e.start_new(Vec::new());
        e.start_from(3);
        assert_eq!(e.sides.iter().map(|s| s.doc.layers.len()).collect::<Vec<_>>(), [0, 0, 2, 2]);
        // No other items: From Another Item says so and stays; B on Start From leaves New.
        e.start_new(Vec::new());
        e.start_from(4);
        assert!(!e.from_other && e.mode == Mode::StartFrom && !e.status.is_empty());
        assert_eq!(pages::apply(e, Outcome::Back), Some(Exit::Discard));
    }

    #[test]
    fn full_item_asks_which_save_to_replace() {
        let parts = parts();
        let mut customiser = customiser(&parts, "full");
        let mut store = Store::default();
        for n in 0..graphic_store::MAX_SAVES {
            store.add(TEE, graphic_store::Saved { sides: [("front".to_owned(), design(n as i16))].into() }).unwrap();
        }
        store.save(customiser.profile_path()).unwrap();
        let mut creator = Creator::default();
        assert!(creator.open_new("front", &mut customiser, &parts));
        let e = &mut creator.session.as_mut().unwrap().editor;
        e.start_from(1);
        e.save(false);
        assert_eq!(e.mode, Mode::Replace, "thirty saves: pick one to write over");
        assert_eq!(pages::page(e, &Thumbs::default()).rows.len(), graphic_store::MAX_SAVES);
        assert_eq!(e.replace_save(5), None);
        let saves = e.store.saves(TEE);
        assert_eq!((saves.len(), saves[5].sides["front"].clone()), (graphic_store::MAX_SAVES, e.sides[0].doc.encode()));
        assert_eq!(e.editing, Some(5));
    }

    #[test]
    fn a_pasted_code_goes_into_a_new_save_and_a_bad_one_asks() {
        let parts = parts();
        let mut customiser = customiser(&parts, "paste");
        let mut creator = Creator::default();
        assert!(creator.open_new("front", &mut customiser, &parts));
        let e = &mut creator.session.as_mut().unwrap().editor;
        e.start_from(5);
        assert!(e.pasting && e.mode == Mode::StartFrom, "Paste Code: the screen reads the clipboard");
        let code = crate::share_code::encode(TEE, &graphic_store::Saved { sides: [("back".to_owned(), design(7))].into() });
        e.paste(&code);
        assert_eq!((e.mode, e.editing, e.side), (Mode::Board, Some(0), 0));
        assert_eq!(e.sides[1].doc.encode(), design(7));
        assert_eq!(e.store.saves(TEE).len(), 1, "into a new save");
        assert!(e.take_saved().is_some(), "for the screen to write and draw");
        // Another item's code, and junk: the popup, then Start From again.
        e.start_new(Vec::new());
        e.paste(&crate::share_code::encode("other", &graphic_store::Saved { sides: [("front".to_owned(), design(1))].into() }));
        assert_eq!(e.mode, Mode::Popup(Ask::Code(true)));
        assert_eq!(Ask::Code(true).question().0, "#That code is for a different item.");
        assert_eq!(e.answer(Some(0)), None);
        assert_eq!(e.mode, Mode::StartFrom);
        e.paste("hello");
        assert_eq!(e.mode, Mode::Popup(Ask::Code(false)));
        assert_eq!(Ask::Code(false).question(), ("#That code could not be loaded.", &["ID_COMMON_OK"][..]));
    }

    /// Review C1: B then Save (and Save on a full item, then Replace) leaves through `finish`, which writes
    /// the save to disk and asks for its tile before the session goes. A write that fails keeps the creator
    /// open, and the popup says so.
    #[test]
    fn done_save_and_done_replace_reach_the_disk() {
        let parts = parts();
        let mut customiser = customiser(&parts, "c1");
        let mut creator = Creator::default();
        let mut tiles = Tiles::default();
        assert!(creator.open_new("front", &mut customiser, &parts));
        let e = &mut creator.session.as_mut().unwrap().editor;
        e.start_from(1);
        e.ask_done();
        assert_eq!(creator.answer(Some(0)), Some(Exit::Done), "Save to Custom Graphics");
        assert!(creator.finish(Exit::Done, &mut customiser, &parts, &mut tiles));
        let store = Store::load(customiser.profile_path());
        assert_eq!(store.saves(TEE).len(), 1, "on disk");
        assert!(tiles.busy(), "its tile is asked for");

        // A full item: Save opens the replace grid, and A there leaves with the save written over.
        let mut store = store;
        for n in 1..graphic_store::MAX_SAVES {
            store.add(TEE, graphic_store::Saved { sides: [("front".to_owned(), design(n as i16))].into() }).unwrap();
        }
        store.save(customiser.profile_path()).unwrap();
        assert!(creator.open_new("front", &mut customiser, &parts));
        let e = &mut creator.session.as_mut().unwrap().editor;
        e.start_from(3);
        e.set_side(2);
        e.ask_done();
        assert_eq!(creator.answer(Some(0)), None);
        let e = &mut creator.session.as_mut().unwrap().editor;
        assert_eq!(e.mode, Mode::Replace);
        assert_eq!(pages::page(e, &Thumbs::default()).rows[0].description, "#Tee holds 30 Custom Graphics: pick one to replace.");
        let sleeve = e.sides[2].doc.encode();
        assert_eq!(e.replace_save(5), Some(Exit::Done));
        assert!(creator.finish(Exit::Done, &mut customiser, &parts, &mut tiles));
        let saves = Store::load(customiser.profile_path());
        let saves = saves.saves(TEE);
        assert_eq!((saves.len(), saves[5].sides.get("left")), (graphic_store::MAX_SAVES, Some(&sleeve)), "written over on disk");

        // The file cannot be written (its folder is a file): the creator stays open and the popup says so.
        assert!(creator.open_new("front", &mut customiser, &parts));
        let s = creator.session.as_mut().unwrap();
        s.profile = graphic_store::file(customiser.profile_path()).join("profile.json");
        // Big Back Print: not among the saves yet (an identical save is not added twice).
        s.editor.start_from(2);
        s.editor.save(true);
        assert_eq!(s.editor.mode, Mode::Replace);
        s.editor.replace_save(0);
        assert!(!creator.finish(Exit::Done, &mut customiser, &parts, &mut tiles), "still open");
        let e = &creator.session.as_ref().unwrap().editor;
        assert_eq!(e.mode, Mode::Popup(Ask::SaveFailed));
        assert_eq!(Ask::SaveFailed.question(), ("#Could not save to Custom Graphics.", &["ID_COMMON_OK"][..]));
        assert_eq!(creator.question(true), Some(Ask::SaveFailed));
        assert_eq!(creator.answer(Some(0)), None);
        assert_eq!(creator.session.as_ref().unwrap().editor.mode, Mode::Board, "OK: back on the board");
    }

    /// Review I1: Enter (or Esc) that ends typing is still held on the next frame: the Layers page must not
    /// take it as A (Add) or B (Leave?). Review M12: with no popup art nothing is asked unseen.
    #[test]
    fn a_key_held_after_typing_is_no_press_and_no_popup_is_asked_unseen() {
        let parts = parts();
        let mut customiser = customiser(&parts, "held");
        let mut creator = Creator::default();
        assert!(creator.open("front", &mut customiser, &parts));
        let s = creator.session.as_mut().unwrap();
        s.editor.add(Layer::new(Art::Text("SK8".into()), [255; 4]));
        s.editor.mode = Mode::Text;
        for key in [KeyCode::Enter, KeyCode::Escape] {
            let mut keys = ButtonInput::<KeyCode>::default();
            let s = creator.session.as_mut().unwrap();
            s.editor.mode = Mode::Text;
            // The frame the key goes down: it types (Done or Cancel), and is no pad press.
            keys.press(key);
            let typed = if key == KeyCode::Enter { Typed::Enter } else { Typed::Escape };
            let pad = keyboard_pad(&keys, true, false);
            assert!(!pad.a && !pad.b);
            let (pad, page) = creator.route(Pad { dt: 0.016, ..pad }, true);
            assert!(page.is_empty());
            creator.session.as_mut().unwrap().editor.input(&pad, &[typed]);
            assert_eq!(creator.session.as_ref().unwrap().editor.mode, Mode::Board, "{key:?} ends typing");
            // The next frames, still held: nothing for the page or the editor.
            for _ in 0..3 {
                keys.clear();
                let pad = keyboard_pad(&keys, false, true);
                let (pad, page) = creator.route(Pad { dt: 0.016, ..pad }, true);
                assert!(page.is_empty(), "{key:?} held: {page:?}");
                creator.session.as_mut().unwrap().editor.input(&pad, &[]);
                assert_eq!(creator.session.as_ref().unwrap().editor.mode, Mode::Board, "{key:?} held");
            }
        }
        // A fresh press is A again.
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::Enter);
        assert!(keyboard_pad(&keys, false, true).a);

        // No popup art: B on the board asks nothing; the editor takes it as Cancel.
        let s = creator.session.as_mut().unwrap();
        s.editor.ask_done();
        assert_eq!(creator.question(false), None);
        assert_eq!(creator.session.as_ref().unwrap().editor.mode, Mode::Board);
    }

    /// Review M3 and M6: layer pictures reuse their textures past `THUMBS_KEPT`; a list-row drag and a burst
    /// of wheel notches are one undo step each.
    #[test]
    fn pictures_are_reused_and_mouse_bursts_are_one_undo_step() {
        let parts = parts();
        let mut customiser = customiser(&parts, "bursts");
        let mut creator = Creator::default();
        assert!(creator.open("front", &mut customiser, &parts));
        let s = creator.session.as_mut().unwrap();
        let shape = |n: u8, y: i16| Layer { y, w: 900, h: 600, ..Layer::new(Art::Shape(n), [200, 10, 10, 255]) };
        s.editor.sides[0].doc.layers = vec![shape(0, -900), shape(2, 0), shape(4, 900)];
        s.editor.layer = 2;
        let keys: Vec<String> = (0..THUMBS_KEPT as u64 + 40).map(|look| s.thumb_key(look)).collect();
        let distinct: std::collections::HashSet<&String> = keys.iter().collect();
        assert_eq!((s.thumbs.layers.len(), distinct.len()), (THUMBS_KEPT, THUMBS_KEPT), "no more textures than kept");

        // The top row dragged to the bottom: three layers crossed, one undo.
        let before = s.editor.doc().clone();
        let row = |r: usize| Vec2::new(200., board::ROW_Y + r as f32 * board::ROW_PITCH);
        s.mouse(Mouse { at: row(0), press: true, held: true, ..default() }, 0);
        s.mouse(Mouse { at: row(1), held: true, ..default() }, 0);
        s.mouse(Mouse { at: row(2), held: true, ..default() }, 0);
        s.mouse(Mouse { at: row(2), release: true, ..default() }, 0);
        assert_ne!(s.editor.doc(), &before);
        s.editor.undo();
        assert_eq!(s.editor.doc(), &before, "one step");

        // Three wheel notches close together, then a pause: one undo.
        let extent = s.editor.extent();
        let f = board::frame(extent, Mode::Board);
        let middle = Vec2::new((f[0] + f[2]) / 2., (f[1] + f[3]) / 2.);
        for _ in 0..3 {
            s.mouse(Mouse { at: middle, wheel: 1., dt: 0.1, ..default() }, 0);
            s.mouse(Mouse { at: middle, dt: 0.1, ..default() }, 0);
        }
        s.mouse(Mouse { at: middle, dt: 1., ..default() }, 0);
        assert_ne!(s.editor.doc(), &before);
        s.editor.undo();
        assert_eq!(s.editor.doc(), &before, "one step");
        // After the pause a notch is a step of its own.
        s.mouse(Mouse { at: middle, wheel: 1., dt: 0.1, ..default() }, 0);
        let one = s.editor.doc().clone();
        s.mouse(Mouse { at: middle, dt: 1., ..default() }, 0);
        s.mouse(Mouse { at: middle, wheel: 1., dt: 0.1, ..default() }, 0);
        s.editor.undo();
        assert_eq!(s.editor.doc(), &one);
    }

    /// The mouse in window pixels at 16:9 and 21:9 reaches each handle of the selected layer (a drag there
    /// changes it) and each Layers row (a click selects, a drag reorders).
    #[test]
    fn mouse_reaches_every_handle_and_row() {
        let parts = parts();
        let mut customiser = customiser(&parts, "mouse");
        let mut creator = Creator::default();
        assert!(creator.open("front", &mut customiser, &parts));
        let s = creator.session.as_mut().unwrap();
        let shape = |n: u8, y: i16| Layer { y, w: 900, h: 600, ..Layer::new(Art::Shape(n), [200, 10, 10, 255]) };
        s.editor.sides[0].doc.layers = vec![shape(0, -900), shape(2, 0), shape(4, 900)];
        s.editor.layer = 1;
        for window in [Vec2::new(1920., 1080.), Vec2::new(2560., 1080.)] {
            let scale = (window.x / 1280.).min(window.y / 720.);
            let to_window = |p: Vec2| p * scale + (window - Vec2::new(1280., 720.) * scale) / 2.;
            let movie = |p: Vec2| to_movie(to_window(p), window);
            assert!((movie(Vec2::new(17., 700.)) - Vec2::new(17., 700.)).length() < 1e-3);
            let extent = s.editor.extent();
            let f = board::frame(extent, Mode::Board);
            s.editor.view_scale = extent.x / (f[2] - f[0]);
            let l = s.editor.current().unwrap().clone();
            let centre = board::to_screen(Vec2::new(f32::from(l.x), f32::from(l.y)), extent, f);
            for (handle, p) in s.editor.handles() {
                let at = board::to_screen(p, extent, f);
                let to = if handle == crate::graphic_editor::Handle::Turn { at + Vec2::new(25., 0.) } else { at + (at - centre).normalize() * 25. };
                s.mouse(Mouse { at: movie(at), press: true, held: true, ..default() }, 0);
                s.mouse(Mouse { at: movie(to), held: true, ..default() }, 0);
                s.mouse(Mouse { at: movie(to), release: true, ..default() }, 0);
                assert_ne!(s.editor.current(), Some(&l), "{window} {handle:?}");
                s.editor.undo();
                assert_eq!(s.editor.current(), Some(&l));
            }
            // Rows: top of the stack first.
            let row = |r: usize| movie(Vec2::new(200., board::ROW_Y + r as f32 * board::ROW_PITCH));
            for r in 0..3 {
                s.mouse(Mouse { at: row(r), press: true, held: true, ..default() }, 0);
                s.mouse(Mouse { at: row(r), release: true, ..default() }, 0);
                assert_eq!(s.editor.layer, 2 - r);
            }
            s.mouse(Mouse { at: row(0), press: true, held: true, ..default() }, 0);
            s.mouse(Mouse { at: row(2), held: true, ..default() }, 0);
            s.mouse(Mouse { at: row(2), release: true, ..default() }, 0);
            assert_eq!((s.editor.layer, s.editor.doc().layers[0].art.clone()), (0, Art::Shape(4)), "the top layer dragged to the bottom");
            s.editor.undo();
            s.editor.layer = 1;
        }
    }

    #[test]
    fn shortcuts_do_what_the_legend_says() {
        for words in ["Ctrl+Z / Ctrl+Y undo, redo", "Ctrl+D duplicate", "Ctrl+C / Ctrl+V copy, paste", "M mirror", "Del delete", "arrows nudge", "Tab Menu"] {
            assert!(pages::HELP.iter().any(|page| page.contains(words)), "{words}");
        }
        let one = |ctrl: bool, shift: bool, key: KeyCode| shortcuts(ctrl, shift, |k| k == key);
        assert_eq!(one(true, false, KeyCode::KeyZ), [Shortcut::Undo]);
        assert_eq!(one(true, false, KeyCode::KeyY), [Shortcut::Redo]);
        assert_eq!(one(true, false, KeyCode::KeyD), [Shortcut::Duplicate]);
        assert_eq!(one(true, false, KeyCode::KeyC), [Shortcut::Copy]);
        assert_eq!(one(true, false, KeyCode::KeyV), [Shortcut::Paste]);
        assert_eq!(one(false, false, KeyCode::KeyM), [Shortcut::Mirror]);
        assert_eq!(one(false, false, KeyCode::Delete), [Shortcut::Delete]);
        assert_eq!(one(false, true, KeyCode::ArrowLeft), [Shortcut::Nudge(-1, 0, true)]);
        assert!(one(false, false, KeyCode::KeyZ).is_empty() && one(true, false, KeyCode::KeyM).is_empty(), "plain Z is the pad's Back");

        let mut e = crate::graphic_editor::tests::tee();
        e.add(Layer { x: 300, ..Layer::new(Art::Shape(0), [255; 4]) });
        e.shortcut(Shortcut::Duplicate);
        assert_eq!(e.doc().layers.len(), 2);
        e.shortcut(Shortcut::Undo);
        assert_eq!(e.doc().layers.len(), 1);
        e.shortcut(Shortcut::Redo);
        assert_eq!(e.doc().layers.len(), 2);
        e.shortcut(Shortcut::Mirror);
        assert_eq!((e.doc().layers.len(), e.current().unwrap().x), (3, -360));
        e.shortcut(Shortcut::Copy);
        e.change_side(1);
        e.shortcut(Shortcut::Paste);
        assert_eq!((e.zone(), e.doc().layers.len(), e.current().unwrap().x), ("back", 1, -360), "pasted on another side");
        e.shortcut(Shortcut::Nudge(1, 0, false));
        e.shortcut(Shortcut::Nudge(0, 1, true));
        assert_eq!((e.current().unwrap().x, e.current().unwrap().y), (-350, 160));
        e.shortcut(Shortcut::Delete);
        assert_eq!(e.mode, Mode::Popup(Ask::Delete));
        e.answer(Some(1));
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Board, 1), "No: back to the board it was asked from");
        e.shortcut(Shortcut::Delete);
        e.answer(Some(0));
        assert!(e.doc().layers.is_empty());
    }

    #[test]
    fn the_page_takes_its_d_pad_a_and_b_and_the_editor_the_rest() {
        let held = Pad { up: true, a: true, b: true, x: true, lb: true, ..default() };
        // The board: Up to the list, Left / Right stay the editor's (side); RT held, Up moves the layer.
        let (pad, keys) = route(Mode::Board, held, [true, false, true, false], true, false, true);
        assert_eq!(keys, [Key::Up, Key::Accept]);
        assert!(!pad.up && !pad.a && !pad.b && pad.x && pad.lb);
        let (pad, keys) = route(Mode::Board, Pad { rt: true, ..held }, [true, false, false, false], false, false, true);
        assert!(keys.is_empty() && pad.up);
        // Options: the whole D-pad.
        let (pad, keys) = route(Mode::Options, held, [false, false, false, true], false, true, true);
        assert_eq!(keys, [Key::Right, Key::Back]);
        assert!(!pad.up);
        // Text: all the editor's; a popup without its art too.
        assert!(route(Mode::Text, held, [true; 4], true, true, true).1.is_empty());
        assert!(route(Mode::Popup(Ask::Done), held, [true; 4], true, true, false).1.is_empty());
        assert_eq!(route(Mode::Popup(Ask::Done), held, [false; 4], true, false, true).1, [Key::Accept]);
    }

    /// The headless walk through the pages on the installed movie (CREATE_SKATER_MANIFEST).
    #[test]
    #[ignore = "needs the prepared Create-a-Skater movie"]
    fn the_creator_pages_follow_the_pad() {
        use crate::core_menu_runtime::{SOUND_A_BUTTON, SOUND_NAV_DOWN, fe_sound_id};
        use crate::create_skater_runtime::Mode as CasMode;
        let (_, _, mut runtime) = crate::create_skater_runtime::tests::installed().expect("CREATE_SKATER_MANIFEST");
        let parts = parts();
        let mut customiser = customiser(&parts, "pages");
        let original = customiser.draft.clone();
        let mut creator = Creator::default();
        creator.open("front", &mut customiser, &parts);
        let thumbs = Thumbs::default();
        let mut log = Vec::new();
        let mut sounds = Vec::new();
        // One frame with `pad` held (after a frame with nothing held, so presses are edges).
        let frame = |creator: &mut Creator, runtime: &mut Runtime, pad: Pad, sounds: &mut Vec<u64>| -> Option<Exit> {
            sounds.clear();
            for pad in [Pad { dt: 0.016, ..default() }, Pad { dt: 0.016, ..pad }] {
                let (pad, keys) = creator.route(pad, true);
                let s = creator.session.as_mut().unwrap();
                if let Some(exit) = feed(&mut s.editor, runtime, &pad, &keys, &[], sounds).unwrap() {
                    return Some(exit);
                }
                sync(&mut s.editor, runtime, &thumbs).unwrap();
            }
            None
        };
        sync(&mut creator.session.as_mut().unwrap().editor, &mut runtime, &thumbs).unwrap();
        let mode = |c: &Creator| c.session.as_ref().unwrap().editor.mode;

        frame(&mut creator, &mut runtime, Pad { a: true, ..default() }, &mut sounds);
        assert_eq!((mode(&creator), runtime.bindings.page.mode), (Mode::Add, CasMode::ThumbsCustom));
        log.push("Board: A -> Add page");
        frame(&mut creator, &mut runtime, Pad { right: true, ..default() }, &mut sounds);
        assert_eq!(sounds, [fe_sound_id("blue_box_nav")], "the grid's own move sound");
        frame(&mut creator, &mut runtime, Pad { right: true, ..default() }, &mut sounds);
        assert_eq!(creator.session.as_ref().unwrap().editor.doc().layers.len(), 1, "the highlighted piece is previewed");
        assert_eq!(creator.session.as_ref().unwrap().editor.doc().layers[0].art, Art::Shape(2));
        log.push("Add: Right twice -> preview");
        frame(&mut creator, &mut runtime, Pad { a: true, ..default() }, &mut sounds);
        assert_eq!((mode(&creator), runtime.bindings.page.mode, runtime.bindings.page.rows.len()), (Mode::Board, CasMode::List, 1));
        assert!(sounds.contains(&SOUND_A_BUTTON), "{sounds:?}");
        log.push("Add: A -> layer added");
        // The board's side selector plays the movie's selector sounds (review I5).
        frame(&mut creator, &mut runtime, Pad { right: true, ..default() }, &mut sounds);
        assert_eq!((creator.session.as_ref().unwrap().editor.side, &sounds[..]), (1, &[fe_sound_id("core_nav_right")][..]));
        frame(&mut creator, &mut runtime, Pad { left: true, ..default() }, &mut sounds);
        assert_eq!((creator.session.as_ref().unwrap().editor.side, &sounds[..]), (0, &[fe_sound_id("core_nav_left")][..]));
        frame(&mut creator, &mut runtime, Pad { x: true, ..default() }, &mut sounds);
        assert_eq!((mode(&creator), runtime.bindings.page.mode), (Mode::Options, CasMode::Morph));
        assert_eq!(sounds, [SOUND_A_BUTTON], "X opens Options as A opens a page");
        log.push("Board: X -> Options");
        frame(&mut creator, &mut runtime, Pad { right: true, ..default() }, &mut sounds);
        assert_eq!(creator.session.as_ref().unwrap().editor.doc().layers[0].colour[..3], crate::graphic_editor::PALETTE[1]);
        assert_eq!(sounds, [fe_sound_id("core_nav_right")]);
        log.push("Options: Right on Colour -> colour changed");
        frame(&mut creator, &mut runtime, Pad { down: true, ..default() }, &mut sounds);
        assert_eq!(sounds, [SOUND_NAV_DOWN]);
        frame(&mut creator, &mut runtime, Pad { b: true, ..default() }, &mut sounds);
        assert_eq!((mode(&creator), runtime.bindings.page.mode), (Mode::Board, CasMode::List));
        assert_eq!(sounds, [SOUND_B_BUTTON]);
        log.push("Options: B -> back on Board");
        frame(&mut creator, &mut runtime, Pad { b: true, ..default() }, &mut sounds);
        assert_eq!(mode(&creator), Mode::Popup(Ask::Done));
        assert_eq!(creator.question(true), Some(Ask::Done));
        assert_eq!(creator.question(true), None, "asked once");
        log.push("Board: B -> Done popup");
        let exit = creator.answer(Some(2));
        assert_eq!(exit, Some(Exit::Discard));
        let mut tiles = Tiles::default();
        creator.finish(Exit::Discard, &mut customiser, &parts, &mut tiles);
        assert_eq!(customiser.draft, original);
        log.push("Done: Discard -> exit with the original look");
        assert_eq!(
            log,
            [
                "Board: A -> Add page",
                "Add: Right twice -> preview",
                "Add: A -> layer added",
                "Board: X -> Options",
                "Options: Right on Colour -> colour changed",
                "Options: B -> back on Board",
                "Board: B -> Done popup",
                "Done: Discard -> exit with the original look",
            ]
        );
    }

    // ---- offline renders (CREATE_SKATER_MANIFEST; written to CREATE_SKATER_OUT) ----

    /// The creator's whole screen for `editor` on the installed movie: the page synced and settled, the
    /// creator's draws over it. Pictures are written under the output folder as raw RGBA (the test
    /// rasteriser reads textures from disk). Returns the movie's draws, the creator's, and the board's.
    fn render(name: &str, mut editor: Editor, grey: bool) -> (Vec<Draw>, Vec<Draw>, Vec<Draw>) {
        use crate::create_skater_runtime::tests::installed;
        let (json, root, mut runtime) = installed().expect("CREATE_SKATER_MANIFEST");
        let out = std::path::PathBuf::from(std::env::var("CREATE_SKATER_OUT").unwrap_or_else(|_| ".".into()));
        // Each render its own pictures (the tests run at once).
        let pictures = out.join("pictures").join(name);
        std::fs::create_dir_all(&pictures).unwrap();
        let mut parts = Parts::default();
        let picture = |key: &str, rgba: Vec<u8>, [w, h]: [u32; 2], runtime: &mut Runtime| -> String {
            let path = pictures.join(format!("{}.rgba", key.replace('/', "_"))).to_string_lossy().into_owned();
            std::fs::write(&path, rgba).unwrap();
            runtime.bindings.thumbnails.insert(path.to_lowercase(), crate::apt_scene::Texture { width: w, height: h, rgba: path.clone(), frame: None });
            path
        };
        let mut thumbs = Thumbs::default();
        for i in 0..graphic::SHAPES.len() {
            let rgba = parts.draw_design(&shape_doc(i), (THUMB, THUMB));
            thumbs.shapes.push(picture(&format!("shape{i}"), rgba, [THUMB, THUMB], &mut runtime));
        }
        for side in &editor.sides {
            for l in &side.doc.layers {
                let rgba = parts.draw_design(&thumb_doc(l), (THUMB, THUMB));
                let key = picture(&format!("look{:016x}", pages::look_key(l)), rgba, [THUMB, THUMB], &mut runtime);
                thumbs.layers.insert(pages::look_key(l), key);
            }
        }
        let size = board::board_pixels(editor.extent());
        let board_file = picture("board", parts.draw_design(editor.doc(), (size[0], size[1])), size, &mut runtime);
        // The installed tee's outline (the sample is the tee).
        let sets = root.parent().unwrap().join("customisation/sets");
        let glb = std::fs::read_dir(&sets).unwrap().flatten().map(|e| e.path().join(format!("library/{}_v4.glb", editor.item))).find(|p| p.exists()).unwrap();
        let triangles = crate::graphic_outline::glb_triangles(&std::fs::read(glb).unwrap()).unwrap();
        let mask = crate::graphic_outline::outline(&triangles, editor.rect(), (size[0], size[1]));
        let outline_file = picture("outline", mask.rgba(64), size, &mut runtime);
        let zone = editor.zone();
        let art = BoardArt::capture(&mut Runtime::load(&json, crate::create_skater_runtime::Page { title: String::new(), mode: crate::create_skater_runtime::Mode::List, rows: Vec::new(), index: 0, accept: String::new(), back: String::new() }).unwrap()).unwrap();

        sync(&mut editor, &mut runtime, &thumbs).unwrap();
        for _ in 0..40 {
            runtime.advance().unwrap();
        }
        let movie = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
        let garment = [0.45, 0.1, 0.1, 1.];
        let board_draws = board::board(&editor, &art, garment, None, editor.grid, &json["language"]);
        let creator = Creator {
            session: Some(Session {
                editor,
                before: Value::Null,
                profile: PathBuf::new(),
                slot: String::new(),
                pending: None,
                worn_rev: 0,
                shown_side: 0,
                thumbs,
                art: Some(art),
                drawn: None,
                outlines: [(zone, mask)].into(),
                outline_shown: Some(zone),
                dpad: Repeat::default(),
                prev: Pad::default(),
                asked: false,
                row_drag: None,
                board_drag: false,
                wheel_idle: None,
                board_age: 0.,
                garment_uv: None,
            }),
            ..default()
        };
        let mut all = creator.draws(movie.clone(), runtime.bindings.first, &json["language"], garment);
        for d in &mut all {
            if d.texture == board::BOARD_TEXTURE {
                d.texture = board_file.clone();
            }
            if d.texture == board::OUTLINE_TEXTURE {
                d.texture = outline_file.clone();
            }
        }
        // The movie's draws come first (Rotate Skater dropped off the Add page), then the creator's.
        let adding = creator.session.as_ref().unwrap().editor.mode == Mode::Add;
        let kept = movie.iter().filter(|d| adding || !board::is_rotate_skater(d)).count();
        let added = all[kept..].to_vec();
        if grey {
            let backdrop = crate::apt_scene::Draw {
                texture: "runtime/white.rgba".into(),
                size: [1, 1],
                vertices: [[0., 0.], [1280., 0.], [1280., 720.], [0., 0.], [1280., 720.], [0., 720.]].map(|p| crate::apt_scene::Vertex { position: p, uv: [0.5, 0.5] }).to_vec(),
                multiply: [0.5, 0.5, 0.5, 1.],
                add: [0.; 4],
            };
            all.insert(0, backdrop);
        }
        crate::core_menu_runtime::tests::raster_draws(&all, &root, &out.join(format!("{name}.png")));
        (movie, added, board_draws)
    }
    /// The checks every render keeps: no yellow, no chequerboard, all on screen, the board right of the CAS card.
    fn check(added: &[Draw], board_draws: &[Draw]) {
        for d in added {
            let [r, g, b, a] = d.multiply;
            assert!(!(r > 0.8 && g > 0.6 && b < 0.4 && a > 0.), "a yellow draw: {} {:?}", d.texture, d.multiply);
            assert!(!d.texture.contains("checker"), "a chequerboard");
            let [x0, y0, x1, y1] = board::bounds(d).unwrap();
            assert!(x0 >= -1. && y0 >= -1. && x1 <= 1281. && y1 <= 721., "off screen: {} {:?}", d.texture, [x0, y0, x1, y1]);
        }
        for d in board_draws {
            let [x0, ..] = board::bounds(d).unwrap();
            assert!(x0 >= board::CAS_CARD[2], "the board overlaps the CAS card: {} at {x0}", d.texture);
        }
    }
    /// A hoodie front: stripes, a circle grouped under text, a triangle.
    fn sample() -> Editor {
        let mut e = crate::graphic_editor::tests::tee();
        let layer = |art: Art, colour: [u8; 4], x: i16, y: i16, w: u16, h: u16| Layer { x, y, w, h, ..Layer::new(art, colour) };
        e.sides[0].doc.layers = vec![
            layer(Art::Shape(14), [200, 30, 40, 255], 0, -1300, 1900, 300),
            layer(Art::Shape(14), [255, 200, 0, 255], 0, -1050, 1900, 110),
            layer(Art::Shape(2), [20, 40, 120, 255], 0, -200, 1000, 1000),
            Layer { grouped: true, ..layer(Art::Text("PORT CARVERTON".into()), [255; 4], 0, 600, 1600, 260) },
            layer(Art::Shape(4), [255; 4], 0, 1300, 400, 500),
        ];
        e.layer = 2;
        e
    }

    #[test]
    #[ignore = "needs the prepared Create-a-Skater movie"]
    fn creator_layers_page_renders() {
        // With the mouse used: the handles' cyan marks too.
        let mut e = sample();
        e.mouse = true;
        let f = board::frame(e.extent(), Mode::Board);
        e.view_scale = e.extent().x / (f[2] - f[0]);
        let (movie, added, board_draws) = render("creator-layers", e, false);
        check(&added, &board_draws);
        assert!(movie.iter().any(|d| d.texture.ends_with("buttons/A.rgba")), "the movie's own A legend");
        assert!(movie.iter().any(board::is_rotate_skater), "the movie draws Rotate Skater");
    }

    #[test]
    #[ignore = "needs the prepared Create-a-Skater movie"]
    fn creator_add_page_renders() {
        let mut e = sample();
        e.open_add();
        e.preview(2);
        let (_, added, board_draws) = render("creator-add", e, false);
        check(&added, &board_draws);
    }

    #[test]
    #[ignore = "needs the prepared Create-a-Skater movie"]
    fn creator_options_page_renders() {
        let mut e = sample();
        e.mode = Mode::Options;
        e.option = 2;
        let (movie, added, board_draws) = render("creator-options", e, false);
        check(&added, &board_draws);
        // The movie's option rows never scroll: the page holds what fits on the card, low rows too.
        let mut e = sample();
        e.mode = Mode::Options;
        e.option = 11;
        let (low, ..) = render("creator-options-low", e, false);
        // Every row's text (left-aligned at x ~110; the info panel's is right-aligned) sits above the panel (y 475).
        for d in movie.iter().chain(&low).filter(|d| d.texture.contains("/fonts/")) {
            let [x0, y0, _, y1] = board::bounds(d).unwrap();
            assert!(x0 >= 125. || y0 < 110. || y1 <= 475., "a row under the info panel: {y1}");
        }
        // Review I6: each Help page fits the info panel (under the rows, above the card's legend).
        for page in 0..pages::HELP.len() {
            let mut e = sample();
            e.mode = Mode::Menu;
            e.menu = e.menu_rows().iter().position(|r| *r == crate::graphic_editor::MenuRow::Help).unwrap();
            e.help = page;
            let (movie, ..) = render(&format!("creator-help-{page}"), e, false);
            let words: Vec<[f32; 4]> = movie.iter().filter(|d| d.texture.contains("/fonts/")).filter_map(board::bounds).filter(|b| b[1] >= 475. && b[3] < 600. && b[2] <= board::CAS_CARD[2]).collect();
            assert!(!words.is_empty(), "page {page}: the panel shows it");
            // The panel's words start under the rows (y 475) and above the card's legend (y ~635).
            for b in movie.iter().filter(|d| d.texture.contains("/fonts/")).filter_map(board::bounds).filter(|b| b[1] >= 475. && b[1] < 620. && b[2] <= board::CAS_CARD[2]) {
                assert!(b[3] <= HELP_BOTTOM && b[0] >= board::CAS_CARD[0], "page {page}: past the panel: {b:?}");
            }
        }
    }

    /// The bottom of the info panel's dark box (movie units, measured on the render).
    const HELP_BOTTOM: f32 = 610.;

    #[test]
    #[ignore = "needs the prepared Create-a-Skater movie"]
    fn creator_board_and_legend_render() {
        let mut e = sample();
        e.grid = true;
        let (_, added, board_draws) = render("creator-composite", e, true);
        check(&added, &board_draws);
        // The legend: six lines, all right of the board card.
        let legend: Vec<_> = added.iter().filter_map(board::bounds).filter(|b| b[0] >= 900. && b[1] >= 420.).collect();
        assert!(legend.len() >= 12, "six glyphs and six lines: {}", legend.len());
    }

    #[test]
    #[ignore = "needs the prepared Create-a-Skater movie"]
    fn creator_keyboard_renders_in_the_card() {
        let mut e = sample();
        e.layer = 3;
        e.mode = Mode::Text;
        e.keyboard = crate::text_entry::TextEntry { shift: true, row: 2, column: 4, ..crate::text_entry::TextEntry::new("PORT CARVERTON", graphic::MAX_TEXT) };
        let (_, added, board_draws) = render("creator-text", e, false);
        check(&added, &board_draws);
        // Review I4: typing, X is Backspace: no X Options on the CAS card.
        assert!(!added.iter().any(|d| d.texture.ends_with("buttons/X.rgba") && board::bounds(d).is_some_and(|b| b[2] <= board::CAS_CARD[2])), "no X Options while typing");
        for b in board_draws.iter().filter_map(board::bounds) {
            // The card art's own shadow reaches 12 px past it.
            assert!(b[0] >= board::CARD[0] - 13. && b[2] <= board::CARD[2] + 13. && b[3] <= board::CARD[3] + 13., "outside the card: {b:?}");
        }
    }
}
