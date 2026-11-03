//! The Graphic Creator's left card: Edit Skater's own movie (create_skater_runtime) showing pages built
//! from the editor. Layers is a List, Add the retail 3x5 logo grid (ThumbsCustom), Options and the Menu
//! the Edit Skater page's rows (Morph: the movie's mode whose rows include sliders; its Style mode has no
//! slider component, `movie:60012.SetNumBars`). What the player does on a page (`Outcome`) goes back to the editor here. Pure.
use crate::{
    create_skater_runtime::{Mode as CasMode, Outcome, Page, Row},
    custom_graphic::{Art, Layer, MAX_LAYERS},
    graphic_editor::{Editor, Exit, LAYOUTS, MenuRow, Mode, Opt, PALETTE, Tab},
};
use std::collections::HashMap;

/// Thumbnail keys already in the CAS runtime's texture map (and the canvas's).
#[derive(Default)]
pub(crate) struct Thumbs {
    /// A layer's picture by its `look_key`.
    pub layers: HashMap<u64, String>,
    /// The Add grid's shapes, by shape number.
    pub shapes: Vec<String>,
    /// The Text tab's tile.
    pub text: String,
    /// Stock logos (retail logo tiles) and game art (design images), by key.
    pub logos: HashMap<u64, String>,
    pub art: HashMap<u64, String>,
    /// The item's saves' tiles, in save order (the replace grid).
    pub saves: Vec<String>,
}
impl Thumbs {
    /// A layer's picture in the Layers list: its own, but the Text tile for words (fitted to a thumbnail
    /// they shrink to a hairline).
    pub fn picture(&self, l: &Layer) -> String {
        match l.art {
            Art::Text(_) => self.text.clone(),
            _ => self.layers.get(&look_key(l)).cloned().unwrap_or_default(),
        }
    }
}

/// What a layer's thumbnail shows: its art, colour and flips (moving, sizing and turning it keep it).
pub(crate) fn look_key(l: &Layer) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match &l.art {
        Art::Shape(s) => (0u8, u64::from(*s)).hash(&mut h),
        Art::Logo(k) => (1u8, *k).hash(&mut h),
        Art::GameArt(k) => (3u8, *k).hash(&mut h),
        Art::Text(t) => (2u8, t).hash(&mut h),
    }
    (l.colour, l.flip_x, l.flip_y).hash(&mut h);
    h.finish()
}

/// Help's pages (the Menu's Help row, Left / Right): the pad's controls, the mouse's and the keyboard's, each
/// short enough for the card's info panel.
pub(crate) const HELP: [&str; 3] = [
    "#RS Move. LS Scale; L3 Stretch. LB / RB Rotate; LT held: fine. D-pad: layer and side; RT + D-pad moves the layer. Y Reset. Back Undo. A Add. X Options. B Done.",
    "#Drag a layer to move it. Corners scale, edges stretch, the top handle turns (Shift: 15 degree steps). Wheel scales, Shift turns. Right-click: Options. Alt-click: the layer below.",
    "#Ctrl+Z / Ctrl+Y undo, redo. Ctrl+D duplicate. Ctrl+C / Ctrl+V copy, paste. M mirror. Del delete; arrows nudge (Shift: further). Tab Menu. Drag a list row to reorder it.",
];

const INDENT: &str = "\u{a0}\u{a0}\u{a0}\u{a0}";
/// Not retail (user 10-19): the Layers rows' hint in the info panel.
const BOARD_HINT: &str = "Sticks move the selected one. A: add a piece. X: colour and more. B: finish and save.";

fn row(text: impl Into<String>, kind: &'static str) -> Row {
    Row { text: text.into(), kind, ..Default::default() }
}
fn rgb24([r, g, b]: [u8; 3]) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// The list row of each layer, top of the stack first.
pub(crate) fn layer_rows(editor: &Editor, thumbs: &Thumbs) -> Vec<Row> {
    let layers = &editor.doc().layers;
    layers
        .iter()
        .rev()
        .map(|l| Row {
            // Grouped layers sit indented under the layer they move with (no-break spaces: the movie drops
            // leading plain ones).
            text: format!("#{}{}", if l.grouped { INDENT } else { "" }, editor.art_name(l)),
            kind: "none",
            thumbnail: thumbs.picture(l),
            // Not retail (user 10-19): the info panel says what to do next, so nothing has to be learned first.
            description: format!("#{} / {MAX_LAYERS} layers. {BOARD_HINT}", layers.len()),
            ..Default::default()
        })
        .collect()
}

/// The page the left card shows for the editor's mode.
fn page_of(editor: &Editor, thumbs: &Thumbs) -> Page {
    match editor.mode {
        Mode::Add => {
            let tab = editor.picker.tab;
            let rows = (0..editor.tab_len(tab))
                .map(|i| {
                    let name = editor.tile_name(tab, i);
                    let thumbnail = match tab {
                        Tab::Shapes => thumbs.shapes.get(i).cloned(),
                        Tab::Logos => editor.catalog.logos.get(i).and_then(|l| thumbs.logos.get(&l.0).cloned()),
                        Tab::GameArt => editor.catalog.art.get(i).and_then(|l| thumbs.art.get(&l.0).cloned()),
                        Tab::Text => Some(thumbs.text.clone()),
                    };
                    // Not retail (user 10-19): how the highlighted piece goes on (`Editor::add_input`).
                    let description = if tab == Tab::Text { format!("#{name}") } else { format!("#{name}. A or a stick places it.") };
                    Row { description, thumbnail: thumbnail.unwrap_or_default(), ..row(format!("#{name}"), "none") }
                })
                .collect();
            Page { title: format!("#{}", tab.name()), mode: CasMode::ThumbsCustom, rows, index: editor.picker_index(), accept: "#Add".into(), back: "#Cancel".into() }
        }
        Mode::Options | Mode::Menu => {
            let (title, rows, selected) = if editor.mode == Mode::Options {
                ("#Options", editor.options().into_iter().map(|opt| option_row(editor, opt)).collect::<Vec<_>>(), editor.option)
            } else {
                ("#Menu", editor.menu_rows().into_iter().map(|r| menu_row(editor, r)).collect(), editor.menu)
            };
            let kinds: Vec<&str> = rows.iter().map(|r| r.kind).collect();
            let first = window(&kinds, selected, editor.scroll);
            let shown = fits(&kinds[first..]);
            let rows = rows.into_iter().skip(first).take(shown).collect();
            Page { title: title.into(), mode: CasMode::Morph, rows, index: selected - first, accept: "ID_COMMON_SELECT".into(), back: "ID_COMMON_BACK".into() }
        }
        Mode::StartFrom if editor.from_other => {
            let rows = editor.sources.iter().map(|s| Row { thumbnail: s.tile.clone(), description: format!("#{}", s.label), ..row(format!("#{}", s.label), "none") }).collect();
            Page { title: "#From Another Item".into(), mode: CasMode::ThumbsCustom, rows, index: editor.start, accept: "ID_COMMON_SELECT".into(), back: "ID_COMMON_BACK".into() }
        }
        Mode::StartFrom => {
            let start = |text: &str, description: &str| Row { description: description.into(), ..row(text, "none") };
            let mut rows = vec![start("#Blank", "#Start with nothing on any side.")];
            rows.extend(LAYOUTS.iter().map(|name| start(&format!("#{name}"), "#A ready-made start to change as you like.")));
            rows.push(start("#From Another Item...", "#Start from a Custom Graphic saved for another item."));
            rows.push(start("#Paste Code", "#Start from a share code copied from a Custom Graphic."));
            Page { title: "#Start From".into(), mode: CasMode::List, rows, index: editor.start, accept: "ID_COMMON_SELECT".into(), back: "ID_COMMON_BACK".into() }
        }
        Mode::Replace => {
            let name = if editor.name.is_empty() { "This item" } else { &editor.name };
            let description = format!("#{name} holds {} Custom Graphics: pick one to replace.", crate::graphic_store::MAX_SAVES);
            let rows = (0..editor.store.saves(&editor.item).len())
                .map(|i| Row { thumbnail: thumbs.saves.get(i).cloned().unwrap_or_default(), description: description.clone(), ..row(format!("#{}", i + 1), "none") })
                .collect();
            Page { title: "#Replace".into(), mode: CasMode::ThumbsCustom, rows, index: editor.replace, accept: "#Replace".into(), back: "#Cancel".into() }
        }
        Mode::Board | Mode::Text | Mode::Popup(_) => {
            let mut rows = layer_rows(editor, thumbs);
            let index = rows.len().saturating_sub(1).saturating_sub(editor.layer);
            if rows.is_empty() {
                // An empty side still has a row, so the info panel has somewhere to speak.
                rows.push(Row { description: "#Press A to add a piece.".into(), ..row("#No Layers", "none") });
            }
            // Typing: the card's legend says what the keyboard's A and B do.
            let (accept, back) = if editor.mode == Mode::Text { ("ID_COMMON_SELECT", "ID_COMMON_CANCEL") } else { ("#Add", "#Done") };
            Page { title: "#Layers".into(), mode: CasMode::List, rows, index, accept: accept.into(), back: back.into() }
        }
    }
}
/// `page`, with the editor's note (`Editor::note`) in the info panel in place of the rows' own words.
pub(crate) fn page(editor: &Editor, thumbs: &Thumbs) -> Page {
    let mut page = page_of(editor, thumbs);
    if let Some(note) = editor.note().filter(|_| editor.mode != Mode::Add) {
        for row in &mut page.rows {
            row.description = format!("#{note}");
        }
    }
    page
}
fn option_row(editor: &Editor, opt: Opt) -> Row {
    let (value, value_text) = editor.option_value(opt);
    let kind = match opt {
        Opt::Colour | Opt::Recent => "color",
        Opt::Hue | Opt::Shade | Opt::Opacity => "slider",
        Opt::Flip | Opt::Group | Opt::LayersUnder => "selector",
        _ => "option",
    };
    let colours = match opt {
        Opt::Colour => PALETTE.iter().map(|c| rgb24(*c)).collect(),
        Opt::Recent => editor.recent_colours().iter().map(|c| rgb24(*c)).collect(),
        _ => Vec::new(),
    };
    let description = match opt {
        Opt::Colour => "#Left / Right: the layer's colour.",
        Opt::Recent => "#The colours used last.",
        Opt::Hue | Opt::Shade | Opt::Opacity => "#Left / Right to change it. Hold LT for fine steps.",
        Opt::Text => "#Type the layer's words.",
        Opt::MirrorCopy => "#A mirrored copy, across the middle of the side.",
        Opt::PairCopy => "#A copy on the side across the item.",
        Opt::Flip => "#Left / Right: flip the layer.",
        Opt::Duplicate => "#A copy of the layer, on top of it.",
        Opt::MoveUp | Opt::MoveDown => "#Move the layer through the stack.",
        Opt::Group => "#On: the layer moves, scales and recolours with the layer below.",
        Opt::LayersUnder => "#The layers under this one: A selects.",
        Opt::Delete => "#Delete the layer.",
    };
    Row { value, value_text, colours, description: description.into(), ..row(editor.option_label(opt), kind) }
}
fn menu_row(editor: &Editor, r: MenuRow) -> Row {
    let (value, value_text) = editor.menu_value(r);
    let kind = match r {
        MenuRow::Background => "color",
        MenuRow::BackgroundOpacity => "slider",
        MenuRow::CopySide | MenuRow::Grid | MenuRow::Help => "selector",
        _ => "option",
    };
    let colours = if r == MenuRow::Background { PALETTE.iter().map(|c| rgb24(*c)).collect() } else { Vec::new() };
    let description = match r {
        MenuRow::Save => "#Save to this item's Custom Graphics.",
        MenuRow::SaveNew => "#Save as another Custom Graphic.",
        MenuRow::Redo => "#Put back what Back undid.",
        MenuRow::CopySide => "#Copy this side onto another side.",
        MenuRow::ClearSide => "#Take every layer off this side.",
        MenuRow::Background | MenuRow::BackgroundOpacity => "#The colour behind every layer on this side.",
        MenuRow::Grid => "#A grid on the board.",
        MenuRow::Help => HELP[editor.help % HELP.len()],
    }
    .to_owned();
    Row { value, value_text, colours, description, ..row(editor.menu_label(r), kind) }
}

/// The movie's Morph rows (Edit Skater's option pages) never scroll: the card holds this much of them
/// above the info panel, each kind as tall as the movie lays it out.
const ROOM: f32 = 350.;
pub(crate) fn row_height(kind: &str) -> f32 {
    match kind {
        "color" | "slider" => 78.,
        "selector" => 66.,
        _ => 42.,
    }
}
/// How many rows fit on the card from the first given.
fn fits(kinds: &[&str]) -> usize {
    let mut room = ROOM;
    kinds.iter().take_while(|k| {
        room -= row_height(k);
        room >= 0.
    })
    .count()
    .max(1)
}
/// The first row on the card: the one before, moved only as far as keeps `selected` on it.
pub(crate) fn window(kinds: &[&str], selected: usize, first: usize) -> usize {
    let selected = selected.min(kinds.len().saturating_sub(1));
    let mut first = first.min(selected);
    while first < selected && fits(&kinds[first..]) <= selected - first {
        first += 1;
    }
    first
}
/// Keep the editor's Options / Menu scroll where the card shows it (before the page is built).
pub(crate) fn scroll(editor: &mut Editor) {
    let kinds = |rows: Vec<Row>| rows.iter().map(|r| r.kind).collect::<Vec<_>>();
    editor.scroll = match editor.mode {
        Mode::Options => window(&kinds(editor.options().into_iter().map(|o| option_row(editor, o)).collect()), editor.option, editor.scroll),
        Mode::Menu => window(&kinds(editor.menu_rows().into_iter().map(|r| menu_row(editor, r)).collect()), editor.menu, editor.scroll),
        _ => 0,
    };
}
/// Up / Down past the card's first or last row on a long page: the editor moves on (the movie's list
/// stops at its last row). `shown`: the card's highlighted row and row count.
pub(crate) fn step_past(editor: &mut Editor, (index, rows): (usize, usize), down: bool) -> bool {
    let (total, selected) = match editor.mode {
        Mode::Options => (editor.options().len(), &mut editor.option),
        Mode::Menu => (editor.menu_rows().len(), &mut editor.menu),
        _ => return false,
    };
    if down && index + 1 == rows && *selected + 1 < total {
        *selected += 1;
    } else if !down && index == 0 && *selected > 0 {
        *selected -= 1;
    } else {
        return false;
    }
    true
}

/// What the player did on the left card, done to the editor.
pub(crate) fn apply(editor: &mut Editor, outcome: Outcome) -> Option<Exit> {
    let fine = editor.fine();
    // Option and Menu rows are numbered from the card's first.
    let outcome = match (editor.mode, outcome) {
        (Mode::Options | Mode::Menu, Outcome::Hilight(i)) => Outcome::Hilight(i + editor.scroll),
        (Mode::Options | Mode::Menu, Outcome::Adjust(i, d)) => Outcome::Adjust(i + editor.scroll, d),
        (Mode::Options | Mode::Menu, Outcome::Accept(i)) => Outcome::Accept(i + editor.scroll),
        (_, o) => o,
    };
    match (editor.mode, outcome) {
        (Mode::Board, Outcome::Hilight(row)) => {
            let count = editor.doc().layers.len();
            if row < count {
                editor.select(count - 1 - row);
            }
        }
        (Mode::Board, Outcome::Accept(_)) => editor.open_add(),
        (Mode::Board, Outcome::Back) => return editor.leave(),
        (Mode::Add, Outcome::Hilight(i)) => editor.preview(i),
        (Mode::Add, Outcome::Accept(i)) => {
            if i != editor.picker_index() {
                editor.preview(i);
            }
            editor.keep();
        }
        (Mode::Add, Outcome::Back) => editor.cancel_add(),
        (Mode::Options, Outcome::Hilight(i)) => editor.option = i,
        (Mode::Options, Outcome::Adjust(i, d)) => {
            if let Some(&opt) = editor.options().get(i) {
                editor.option = i;
                editor.adjust_option(opt, d, fine);
            }
        }
        (Mode::Options, Outcome::Accept(i)) => {
            if let Some(&opt) = editor.options().get(i) {
                editor.option = i;
                editor.activate_option(opt);
            }
        }
        (Mode::Options, Outcome::Back) => editor.close_options(),
        (Mode::Menu, Outcome::Hilight(i)) => editor.menu = i,
        (Mode::Menu, Outcome::Adjust(i, d)) => {
            if let Some(&r) = editor.menu_rows().get(i) {
                editor.menu = i;
                editor.adjust_menu(r, d, fine);
            }
        }
        (Mode::Menu, Outcome::Accept(i)) => {
            if let Some(&r) = editor.menu_rows().get(i) {
                editor.menu = i;
                return editor.activate_menu(r);
            }
        }
        (Mode::Menu, Outcome::Back) => editor.mode = Mode::Board,
        (Mode::StartFrom, Outcome::Hilight(i)) => editor.start = i,
        (Mode::StartFrom, Outcome::Accept(i)) => editor.start_from(i),
        (Mode::StartFrom, Outcome::Back) => return editor.start_back(),
        (Mode::Replace, Outcome::Hilight(i)) => editor.replace = i,
        (Mode::Replace, Outcome::Accept(i)) => return editor.replace_save(i),
        (Mode::Replace, Outcome::Back) => editor.cancel_replace(),
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphic_editor::{Ask, TABS, tests::tee};

    #[test]
    fn layers_list_top_first_with_groups_indented_and_the_count() {
        let mut e = tee();
        e.sides[0].doc.layers = vec![Layer::new(Art::Shape(0), [255; 4]), Layer { grouped: true, ..Layer::new(Art::Shape(2), [255; 4]) }, Layer::new(Art::Text("SK8".into()), [255; 4])];
        e.layer = 1;
        let p = page(&e, &Thumbs::default());
        assert_eq!((p.mode, p.title.as_str(), p.accept.as_str(), p.back.as_str()), (CasMode::List, "#Layers", "#Add", "#Done"));
        assert_eq!(p.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["#\"SK8\"".to_owned(), format!("#{INDENT}Circle"), "#Square".to_owned()]);
        assert_eq!(p.index, 1);
        assert_eq!(p.rows[0].description, format!("#3 / 64 layers. {BOARD_HINT}"));
        // Highlighting a row selects its layer; B asks Done once something is unsaved (user 10-19: else it leaves).
        apply(&mut e, Outcome::Hilight(2));
        assert_eq!(e.layer, 0);
        assert_eq!(apply(&mut e, Outcome::Back), Some(Exit::Discard), "nothing unsaved on a New: out as it was");
        e.changed = true;
        assert_eq!(apply(&mut e, Outcome::Back), None);
        assert_eq!(e.mode, Mode::Popup(Ask::Done));
    }

    #[test]
    fn add_options_and_menu_pages_follow_the_editor() {
        let mut e = tee();
        apply(&mut e, Outcome::Accept(0));
        let p = page(&e, &Thumbs::default());
        assert_eq!((p.mode, p.title.as_str(), p.rows.len()), (CasMode::ThumbsCustom, "#Shapes", crate::custom_graphic::SHAPES.len()));
        apply(&mut e, Outcome::Hilight(2));
        assert_eq!(e.doc().layers[0].art, Art::Shape(2));
        apply(&mut e, Outcome::Accept(2));
        assert_eq!((e.mode, e.doc().layers.len()), (Mode::Board, 1));
        e.mode = Mode::Options;
        let p = page(&e, &Thumbs::default());
        assert_eq!(p.mode, CasMode::Morph);
        // The card shows what fits (the movie's option rows never scroll); the rest follow in order.
        let all: Vec<Row> = e.options().into_iter().map(|o| option_row(&e, o)).collect();
        let kinds: Vec<_> = all.iter().map(|r| (r.text.as_str(), r.kind)).collect();
        assert_eq!(kinds[..6], [("#Colour", "color"), ("#Mirror Copy", "option"), ("#Copy to Back", "option"), ("#Hue", "slider"), ("#Shade", "slider"), ("#Opacity", "slider")]);
        assert!(kinds.contains(&("#Flip", "selector")) && kinds.last() == Some(&("#Delete", "option")));
        assert_eq!(p.rows[..], all[..5]);
        assert_eq!(p.rows[0].colours.len(), PALETTE.len());
        apply(&mut e, Outcome::Adjust(0, 1));
        assert_eq!(e.doc().layers[0].colour[..3], PALETTE[1]);
        let p = page(&e, &Thumbs::default());
        assert_eq!(p.rows[3].text, "#Recent", "the colour went to Recent");
        apply(&mut e, Outcome::Back);
        assert_eq!(e.mode, Mode::Board);
        e.mode = Mode::Menu;
        let p = page(&e, &Thumbs::default());
        let labels: Vec<&str> = e.menu_rows().into_iter().map(|r| e.menu_label(r)).collect();
        assert_eq!(labels, ["#Save", "#Redo", "#Copy Side to...", "#Clear Side", "#Background", "#Background Opacity", "#Grid", "#Help"]);
        assert_eq!(p.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), labels[..p.rows.len()]);
        assert!(menu_row(&e, MenuRow::Help).description.contains("Stretch"));
        apply(&mut e, Outcome::Adjust(6, 1));
        assert!(e.grid);
        assert_eq!(apply(&mut e, Outcome::Accept(0)), None);
        assert_eq!((e.mode, e.store.saves(&e.item).len()), (Mode::Board, 1));
        assert_eq!(TABS[1].name(), "Skate Art");
    }

    /// Review I2 and I4: what the editor has to say shows in the card's info panel for a moment (an empty side
    /// still has a row for it); typing, the card's legend is the keyboard's.
    #[test]
    fn notes_show_in_the_info_panel_and_typing_has_the_keyboards_legend() {
        use crate::graphic_editor::tests::press;
        let mut e = tee();
        let description = |e: &Editor| page(e, &Thumbs::default()).rows[0].description.clone();
        let rows = page(&e, &Thumbs::default()).rows;
        assert_eq!((rows.len(), rows[0].text.as_str(), rows[0].description.as_str()), (1, "#No Layers", "#Press A to add a piece."));
        press(&mut e, |p| p.x = true);
        assert_eq!(description(&e), "#Press A to add a piece first");
        for _ in 0..3 {
            e.input(&crate::graphic_editor::Pad { dt: 1., ..Default::default() }, &[]);
        }
        assert_eq!(description(&e), "#Press A to add a piece.", "gone after a moment");
        // Save: where it went.
        e.add(Layer::new(Art::Shape(0), [255; 4]));
        e.mode = Mode::Menu;
        e.activate_menu(MenuRow::Save);
        assert_eq!((e.mode, description(&e).as_str()), (Mode::Board, "#Saved to Custom Graphics (1 of 30)"));
        // Start From with nothing to start from says so on its own page.
        e.start_new(Vec::new());
        e.start_from(4);
        assert_eq!(description(&e), "#No other item has a Custom Graphic for these sides");
        // Typing: A Select, B Cancel, as the keyboard's legend.
        e.mode = Mode::Text;
        let p = page(&e, &Thumbs::default());
        assert_eq!((p.accept.as_str(), p.back.as_str()), ("ID_COMMON_SELECT", "ID_COMMON_CANCEL"));
        e.mode = Mode::Board;
        assert_eq!(page(&e, &Thumbs::default()).back, "#Done");
    }

    /// Review I6: Help is three short pages on one selector row, Left / Right between them.
    #[test]
    fn help_is_three_short_pages() {
        let mut e = tee();
        let help = e.menu_rows().iter().position(|r| *r == MenuRow::Help).unwrap();
        for (i, words) in HELP.iter().enumerate() {
            assert!(words.len() <= 200, "page {i}: {} characters", words.len());
            let row = menu_row(&e, MenuRow::Help);
            assert_eq!((row.kind, row.description.as_str()), ("selector", *words));
            e.mode = Mode::Menu;
            apply(&mut e, Outcome::Adjust(help, 1));
        }
        assert_eq!(e.help, 0, "round again");
        assert_eq!(e.menu_value(MenuRow::Help).1, "#Pad");
    }

    #[test]
    fn long_option_pages_show_a_window_that_moves_only_at_its_edges() {
        let kinds = ["color", "slider", "slider", "slider", "option", "option", "selector", "option", "option", "option", "selector", "option", "option"];
        assert_eq!(fits(&kinds), 4, "a colour and three sliders fill the card");
        assert_eq!(window(&kinds, 3, 0), 0);
        assert_eq!(window(&kinds, 4, 0), 1, "one past the bottom moves it by one");
        assert_eq!(window(&kinds, 12, 1), 6);
        assert_eq!(window(&kinds, 8, 6), 6, "moving up inside the window keeps it");
        assert_eq!(window(&kinds, 5, 6), 5);
        // On the editor: Down past the card's last row steps the option and the page follows.
        let mut e = tee();
        e.sides[0].doc.layers = vec![Layer::new(Art::Shape(0), [255; 4])];
        e.mode = Mode::Options;
        // Colour, Mirror Copy, Copy to Back, Hue, Shade fill the card; Opacity is next.
        e.option = 4;
        assert!(step_past(&mut e, (4, 5), true));
        scroll(&mut e);
        let p = page(&e, &Thumbs::default());
        assert_eq!((e.option, e.scroll, p.index), (5, 1, 4));
        assert!(!step_past(&mut e, (1, 5), true), "inside the card the movie moves");
        apply(&mut e, Outcome::Hilight(0));
        assert_eq!(e.option, 1, "card rows count from its first");
    }
}
