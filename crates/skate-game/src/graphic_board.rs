//! The Graphic Creator's board card and its legend, as draws in the createskater movie's 1280x720 space:
//! a second Edit Skater card (the movie's own, captured once and moved right of the first) holding the
//! whole side being edited, and the retail Graphic Adjust legend under the skater. Pure.
use crate::{
    apt_scene::{Draw, Vertex},
    apt_text::Font,
    create_skater_runtime::{Mode as CasMode, Page, Runtime},
    custom_graphic::Layer,
    graphic_editor::{Editor, Handle, Mode, side_name},
    graphic_outline::OutlineMask,
};
use bevy::math::Vec2;
use serde_json::Value;

/// The movie's own card on its list pages.
pub(crate) const CAS_CARD: [f32; 4] = [88., 54., 452., 666.];
/// The board card, right of it.
pub(crate) const CARD: [f32; 4] = [474., 54., 838., 666.];
/// The rendered side (`Parts::draw_design` at `board_pixels`), a texture the screen fills.
pub(crate) const BOARD_TEXTURE: &str = "creator/board";
/// The garment's outline on the side (`graphic_outline`), at 25% white.
pub(crate) const OUTLINE_TEXTURE: &str = "creator/outline";
const WHITE: &str = "runtime/white.rgba";
/// The movie's thumbnail selection bracket (character 44): a top-left corner.
const BRACKET: &str = "assets/data/fe/source/screens/cas/createskater/0012_42.Texture.rgba";
/// Where the side is fitted on the card's dark panel.
const ROOM: [f32; 4] = [494., 112., 818., 616.];
/// The card footer's ink, the legend's steel blue and the unselected rows' blue.
const INK: u32 = 0x212121;
const STEEL: u32 = 0x6494AB;
const DIM: u32 = 0x3C5A78;
/// The Layers list's rows (the movie's List): the first row's centre and the pitch.
pub(crate) const ROW_Y: f32 = 126.;
pub(crate) const ROW_PITCH: f32 = 32.;
pub(crate) const LIST_ROWS: usize = 11;

pub(crate) fn colour(rgb: u32, alpha: f32) -> [f32; 4] {
    [(rgb >> 16) as f32 / 255., (rgb >> 8 & 255) as f32 / 255., (rgb & 255) as f32 / 255., alpha]
}
pub(crate) fn quad_uv(texture: &str, size: [u32; 2], [x0, y0, x1, y1]: [f32; 4], [u0, v0, u1, v1]: [f32; 4], multiply: [f32; 4]) -> Draw {
    let points = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    Draw {
        texture: texture.into(),
        size,
        vertices: [0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: uvs[i] }).to_vec(),
        multiply,
        add: [0.; 4],
    }
}
pub(crate) fn quad(texture: &str, size: [u32; 2], rect: [f32; 4], multiply: [f32; 4]) -> Draw {
    quad_uv(texture, size, rect, [0., 0., 1., 1.], multiply)
}
/// A line of text from the movie's font atlas. ponytail: boot.rs `label`, copied (boot.rs is not this work's file).
pub(crate) fn label(font: &Font, text: &str, [x, top]: [f32; 2], height: f32, colour: [f32; 4]) -> Option<Draw> {
    let (sx, sy) = (font.scale[0] * height, font.scale[1] * height);
    let mut x = x + font.offset[0] * sx;
    let y = top + font.offset[1] * sy;
    let mut vertices = Vec::new();
    let size = font.size.map(|s| s as f32);
    for c in text.chars() {
        let Some(g) = font.glyph(c) else { continue };
        let x0 = x + g.x_offset * sx;
        let y0 = y + (font.ascent - g.y_offset) * sy;
        let [u0, v0, u1, v1] = g.atlas_bounds;
        let points = [[x0, y0], [x0 + g.width * sx, y0], [x0 + g.width * sx, y0 + g.height * sy], [x0, y0 + g.height * sy]];
        let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
        vertices.extend([0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: [uvs[i][0] / size[0], uvs[i][1] / size[1]] }));
        x = g.x_advance.mul_add(sx, x);
    }
    (!vertices.is_empty()).then(|| Draw { texture: font.texture.clone(), size: font.size, vertices, multiply: colour, add: [0.; 4] })
}
/// A movie button glyph (runtime/buttons, 40x30) with its top-left at `at`, `height` tall.
pub(crate) fn button(name: &str, [x, y]: [f32; 2], height: f32) -> Draw {
    quad(&format!("runtime/buttons/{name}.rgba"), [40, 30], [x, y, x + height * 4. / 3., y + height], [1.; 4])
}
/// A language string, or the literal fallback.
fn say<'a>(language: &'a Value, id: &str, fallback: &'a str) -> &'a str {
    language[id].as_str().unwrap_or(fallback)
}

/// What the board is drawn with: the movie's empty card, moved to `CARD`, its font, and its highlighted row.
pub(crate) struct BoardArt {
    pub card: Vec<Draw>,
    pub font: Font,
    pub lit: Lit,
}
/// The movie's highlighted list row, captured on a probe word: its text layers (font, height, colour, offset
/// from the top text's origin) and the glow behind it, relative to that origin.
pub(crate) struct Lit {
    text: Vec<(Font, f32, [f32; 4], [f32; 4], [f32; 2])>,
    glow: Vec<Draw>,
    /// The probe's width and the top text's height.
    width: f32,
    pub height: f32,
}
const PROBE: &str = "Front";
impl Lit {
    /// `text` lit as the movie lights a list row, its top text's origin at `at`, `scale` times the row's size.
    pub fn draw(&self, text: &str, [x, y]: [f32; 2], scale: f32) -> Vec<Draw> {
        let stretch = self.text.last().map_or(1., |(f, h, ..)| f.width(text, *h) / self.width.max(1.));
        let mut out: Vec<Draw> = self
            .glow
            .iter()
            .cloned()
            .map(|mut d| {
                d.vertices.iter_mut().for_each(|v| v.position = [x + v.position[0] * scale * stretch, y + v.position[1] * scale]);
                d
            })
            .collect();
        for (font, height, multiply, add, [ox, oy]) in &self.text {
            out.extend(label(font, text, [x + ox * scale, y + oy * scale], height * scale, *multiply).map(|mut d| {
                d.add = *add;
                d
            }));
        }
        out
    }
}
impl BoardArt {
    /// From a runtime of its own (its page is changed): the movie's card with no rows, title or legend.
    pub fn capture(runtime: &mut Runtime) -> Result<Self, String> {
        for _ in 0..120 {
            if runtime.bindings.intro_complete {
                break;
            }
            runtime.advance()?;
        }
        runtime.show(Page { title: String::new(), mode: CasMode::List, rows: Vec::new(), index: 0, accept: "#".into(), back: "#".into() })?;
        for _ in 0..40 {
            runtime.advance()?;
        }
        let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes)?;
        let dx = CARD[0] - CAS_CARD[0];
        let card = draws
            .into_iter()
            .filter(|d| bounds(d).is_some_and(|[x0, y0, x1, y1]| x0 >= CAS_CARD[0] - 12. && x1 <= CAS_CARD[2] + 12. && y0 >= CAS_CARD[1] - 12. && y1 <= CAS_CARD[3] + 12.))
            .map(|mut d| {
                d.vertices.iter_mut().for_each(|v| v.position[0] += dx);
                d
            })
            .collect::<Vec<_>>();
        if card.is_empty() {
            return Err("the movie drew no card".into());
        }
        let lit = capture_lit(runtime, &card, dx)?;
        let text = &runtime.bindings.movie.text_assets;
        let font = text.font_named("Futura Std Medium").and_then(|id| text.fonts.get(&id)).cloned().ok_or("no Futura")?;
        Ok(Self { card, font, lit })
    }
}
/// The movie's first list row highlighted on `PROBE`: what it draws beyond the empty card (`card`, moved by `dx`).
fn capture_lit(runtime: &mut Runtime, card: &[Draw], dx: f32) -> Result<Lit, String> {
    let row = crate::create_skater_runtime::Row { text: format!("#{PROBE}"), kind: "none", ..Default::default() };
    runtime.show(Page { title: String::new(), mode: CasMode::List, rows: vec![row], index: 0, accept: "#".into(), back: "#".into() })?;
    for _ in 0..40 {
        runtime.advance()?;
    }
    let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes)?;
    let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(p, q)| (p - q).abs() < 0.5);
    let in_card = |d: &Draw| {
        let b = bounds(d);
        card.iter().any(|c| c.texture == d.texture && bounds(c).zip(b).is_some_and(|(c, b)| near(c, [b[0] + dx, b[1], b[2] + dx, b[3]])))
    };
    let row: Vec<Draw> = draws.into_iter().filter(|d| bounds(d).is_some_and(|b| b[0] < CAS_CARD[2] && b[1] > 100. && b[3] < 170.) && !in_card(d)).collect();
    let fonts = &runtime.bindings.movie.text_assets.fonts;
    let font_of = |d: &Draw| fonts.values().find(|f| f.texture == d.texture).cloned();
    // Each text layer: the label origin that reproduces its glyph box, and its height.
    let mut text = Vec::new();
    for d in &row {
        let (Some(font), Some(b)) = (font_of(d), bounds(d)) else { continue };
        let Some(unit) = label(&font, PROBE, [0., 0.], 1., [1.; 4]).as_ref().and_then(bounds) else { continue };
        let height = (b[3] - b[1]) / (unit[3] - unit[1]).max(1e-3);
        let Some(at) = label(&font, PROBE, [0., 0.], height, [1.; 4]).as_ref().and_then(bounds) else { continue };
        text.push((font, height, d.multiply, d.add, [b[0] - at[0], b[1] - at[1]]));
    }
    let Some(&(ref top, height, .., origin)) = text.last() else { return Err("the movie lit no row".into()) };
    let width = top.width(PROBE, height);
    for t in &mut text {
        t.4 = [t.4[0] - origin[0], t.4[1] - origin[1]];
    }
    let glow = row
        .into_iter()
        .filter(|d| font_of(d).is_none())
        .map(|mut d| {
            d.vertices.iter_mut().for_each(|v| v.position = [v.position[0] - origin[0], v.position[1] - origin[1]]);
            d
        })
        .collect();
    Ok(Lit { text, glow, width, height })
}
/// A draw's extent `[x0, y0, x1, y1]`.
pub(crate) fn bounds(d: &Draw) -> Option<[f32; 4]> {
    let first = d.vertices.first()?.position;
    Some(d.vertices.iter().fold([first[0], first[1], first[0], first[1]], |[a, b, c, e], v| {
        [a.min(v.position[0]), b.min(v.position[1]), c.max(v.position[0]), e.max(v.position[1])]
    }))
}

/// The pixels the side is rendered at: its long side 512.
pub(crate) fn board_pixels(extent: Vec2) -> [u32; 2] {
    let scale = 512. / extent.max_element().max(1.);
    [(extent.x * scale).round().max(1.) as u32, (extent.y * scale).round().max(1.) as u32]
}
/// Where the side sits on screen: as large as fits `ROOM` (above the keyboard while typing), centred.
pub(crate) fn frame(extent: Vec2, mode: Mode) -> [f32; 4] {
    let room = if mode == Mode::Text { [ROOM[0], ROOM[1], ROOM[2], TYPED[1] - 12.] } else { ROOM };
    let (w, h) = (room[2] - room[0], room[3] - room[1]);
    let scale = (w / extent.x.max(1.)).min(h / extent.y.max(1.));
    let (fw, fh) = (extent.x * scale, extent.y * scale);
    let (x, y) = (room[0] + (w - fw) / 2., room[1] + (h - fh) / 2.);
    [x, y, x + fw, y + fh]
}
fn layer_box(l: &Layer, extent: Vec2, f: [f32; 4]) -> [f32; 4] {
    let (s, c) = (l.angle as f32).to_radians().sin_cos();
    let (hw, hh) = (l.w as f32 / 2., l.h as f32 / 2.);
    let points = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(x, y)| {
        let p = Vec2::new(l.x as f32 + x * c - y * s, l.y as f32 + x * s + y * c) / extent + 0.5;
        Vec2::new(f[0] + p.x * (f[2] - f[0]), f[1] + p.y * (f[3] - f[1]))
    });
    let (lo, hi) = points.iter().fold((points[0], points[0]), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    [lo.x, lo.y, hi.x, hi.y]
}

/// The header's side names: their height and top, each name's left x and width, and the `>`'s x.
fn header(editor: &Editor, art: &BoardArt) -> (f32, f32, Vec<(f32, f32)>, f32) {
    let names: Vec<&str> = editor.sides.iter().map(|s| side_name(s.zone)).collect();
    let gap = 12.;
    let room = CARD[2] - 24. - (CARD[0] + 56.);
    let width = |h: f32| art.font.width("<", h) * 2. + names.iter().map(|n| art.font.width(n, h)).sum::<f32>() + gap * (names.len() + 1) as f32;
    let full = art.lit.height;
    let height = (full * room / width(full)).min(full);
    let mut x = CARD[0] + 56. + art.font.width("<", height) + gap;
    let slots = names
        .iter()
        .map(|n| {
            let w = art.font.width(n, height);
            x += w + gap;
            (x - w - gap, w)
        })
        .collect();
    (height, 82. - height / 2., slots, x)
}
/// The side whose name in the header is at `at` (a click changes to it).
pub(crate) fn side_at(editor: &Editor, art: &BoardArt, at: Vec2) -> Option<usize> {
    let (height, top, slots, _) = header(editor, art);
    (at.y >= top - 6. && at.y <= top + height + 6.).then(|| slots.iter().position(|(x, w)| at.x >= x - 6. && at.x <= x + w + 6.)).flatten()
}
/// The Layers list's row at `at` (`first`: the first row on screen), when the cursor is on the list.
pub(crate) fn list_row(at: Vec2, first: usize) -> Option<usize> {
    if at.x < CAS_CARD[0] || at.x > CAS_CARD[2] {
        return None;
    }
    let k = ((at.y - ROW_Y) / ROW_PITCH).round();
    (0. ..LIST_ROWS as f32).contains(&k).then(|| first + k as usize)
}
/// Canvas units (the side's centre the origin) to the screen, on the board's frame `f`.
pub(crate) fn to_screen(p: Vec2, extent: Vec2, f: [f32; 4]) -> Vec2 {
    let p = p / extent + 0.5;
    Vec2::new(f[0] + p.x * (f[2] - f[0]), f[1] + p.y * (f[3] - f[1]))
}
/// A bar from `a` to `b`, `width` across.
fn bar(a: Vec2, b: Vec2, width: f32, multiply: [f32; 4]) -> Draw {
    let n = (b - a).normalize_or_zero().perp() * width / 2.;
    let points = [a - n, b - n, b + n, a + n].map(|p| [p.x, p.y]);
    Draw { texture: WHITE.into(), size: [1, 1], vertices: [0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: [0.5, 0.5] }).to_vec(), multiply, add: [0.; 4] }
}

/// The board card: the side selector, the side on the garment's colour, the selected layer's brackets
/// and the footer.
pub(crate) fn board(editor: &Editor, art: &BoardArt, garment: [f32; 4], outline: Option<&OutlineMask>, grid: bool, language: &Value) -> Vec<Draw> {
    let mut out = art.card.clone();
    // Header: < Front  Back  L Sleeve  R Sleeve >, the side being edited lit as the list's highlight.
    out.push(button("DPad", [CARD[0] + 10., 67.], 30.));
    let (height, top, slots, right) = header(editor, art);
    let full = art.lit.height;
    let steel = colour(STEEL, 1.);
    out.extend(label(&art.font, "<", [CARD[0] + 56., top], height, steel));
    for (i, (side, &(x, _))) in editor.sides.iter().zip(&slots).enumerate() {
        let name = side_name(side.zone);
        if i == editor.side {
            out.extend(art.lit.draw(name, [x, top], height / full));
        } else {
            out.extend(label(&art.font, name, [x, top], height, colour(DIM, 1.)));
        }
    }
    out.extend(label(&art.font, ">", [right, top], height, steel));

    // The side: the garment's colour, the grid, then the design.
    let extent = editor.extent();
    let f = frame(extent, editor.mode);
    out.push(quad(WHITE, [1, 1], [f[0] - 3., f[1] - 3., f[2] + 3., f[3] + 3.], [0., 0., 0., 0.6]));
    out.push(quad(WHITE, [1, 1], f, garment));
    if grid {
        let step = (f[2] - f[0]).max(f[3] - f[1]) / 16.;
        let centre = [(f[0] + f[2]) / 2., (f[1] + f[3]) / 2.];
        for k in -8..=8 {
            let (x, y) = (centre[0] + k as f32 * step, centre[1] + k as f32 * step);
            let a = if k == 0 { 0.25 } else { 0.1 };
            if x > f[0] && x < f[2] {
                out.push(quad(WHITE, [1, 1], [x - 0.5, f[1], x + 0.5, f[3]], [1., 1., 1., a]));
            }
            if y > f[1] && y < f[3] {
                out.push(quad(WHITE, [1, 1], [f[0], y - 0.5, f[2], y + 0.5], [1., 1., 1., a]));
            }
        }
    }
    if let Some(mask) = outline {
        out.push(quad(OUTLINE_TEXTURE, [mask.size.0, mask.size.1], f, [1.; 4]));
    }
    out.push(quad(BOARD_TEXTURE, board_pixels(extent), f, [1.; 4]));

    // The selected layer: the movie's thumbnail brackets at its corners.
    if matches!(editor.mode, Mode::Board | Mode::Options | Mode::Text) {
        if let Some(l) = editor.current() {
            let [x0, y0, x1, y1] = layer_box(l, extent, f);
            let s = 20.;
            let (x0, y0, x1, y1) = (x0 - 6., y0 - 6., x1 + 6., y1 + 6.);
            for (rect, uv) in [
                ([x0, y0, x0 + s, y0 + s], [0., 0., 1., 1.]),
                ([x1 - s, y0, x1, y0 + s], [1., 0., 0., 1.]),
                ([x0, y1 - s, x0 + s, y1], [0., 1., 1., 0.]),
                ([x1 - s, y1 - s, x1, y1], [1., 1., 0., 0.]),
            ] {
                out.push(quad_uv(BRACKET, [32, 32], rect, uv, [1.; 4]));
            }
        }
    }
    // With the mouse: cyan marks on the handles it can drag (corners scale, edges stretch, the stem turns).
    if editor.mouse && editor.mode == Mode::Board {
        let cyan = colour(GLOW, 1.);
        let at = |p: Vec2| to_screen(p, extent, f);
        let handles = editor.handles();
        if let (Some(l), false) = (editor.current(), handles.is_empty()) {
            let centre = at(Vec2::new(l.x as f32, l.y as f32));
            let (sin, cos) = (l.angle as f32).to_radians().sin_cos();
            let (across, down) = (Vec2::new(cos, sin), Vec2::new(-sin, cos));
            for (handle, p) in &handles {
                let p = at(*p);
                match handle {
                    Handle::Corner => {
                        let out_x = across * (p - centre).dot(across).signum();
                        let out_y = down * (p - centre).dot(down).signum();
                        out.push(bar(p, p - out_x * 10., 3., cyan));
                        out.push(bar(p, p - out_y * 10., 3., cyan));
                    }
                    Handle::Width => out.push(bar(p - down * 7., p + down * 7., 3., cyan)),
                    Handle::Height => out.push(bar(p - across * 7., p + across * 7., 3., cyan)),
                    Handle::Turn => {
                        let base = at(handles[6].1);
                        out.push(bar(base, p, 2., cyan));
                        out.push(bar(p - across * 6., p + across * 6., 3., cyan));
                    }
                }
            }
        }
    }

    if editor.mode == Mode::Text {
        out.extend(keyboard(editor, art, language));
        return out;
    }
    // Footer, printed on the pulp as the movie's legend: Y Reset, Back Undo, Start Menu (the language table
    // has no plain "Menu": the Menu page's own title).
    let items = [("Y", say(language, "ID_CAC_CUSTOMGRAPHIC_RESET", "Reset")), ("Back", say(language, "ID_SKATEPARK_QUICKMENU_UNDO", "Undo")), ("Start", "Menu")];
    out.extend(footer(art, &items, CARD[2] - 12., 633.));
    out
}

/// The typed line, then the keys (10 columns, the command row last).
const TYPED: [f32; 4] = [490., 342., 822., 374.];
const KEY: [f32; 2] = [33.2, 34.];
const KEYS_Y: f32 = 382.;
/// The highlight: the movie's list glow blue (#00CBFF).
const GLOW: u32 = 0x00CBFF;

/// Text mode's on-screen keyboard (`text_entry`), in the card's style: the text typed so far, dark keys with the
/// highlighted one lit blue, and the boot keyboard's legend on the pulp.
fn keyboard(editor: &Editor, art: &BoardArt, language: &Value) -> Vec<Draw> {
    use crate::text_entry::{COMMANDS, Command2, GRID};
    let entry = &editor.keyboard;
    // Keys a shade lighter than the card, so each reads as a tile.
    let ink = [1., 1., 1., 0.07];
    let mut out = vec![quad(WHITE, [1, 1], TYPED, ink)];
    out.extend(label(&art.font, &format!("{}_", entry.text), [TYPED[0] + 8., TYPED[1] + 7.], 20., [1.; 4]));
    let count = format!("{}/{}", entry.text.chars().count(), entry.max);
    let right = TYPED[2] - 8. - art.font.width(&count, 16.);
    out.extend(label(&art.font, &count, [right, TYPED[1] + 9.], 16., colour(STEEL, 1.)));
    let mut keys: Vec<(usize, usize, usize, String)> = Vec::new();
    for (row, chars) in GRID.iter().enumerate() {
        for (column, c) in chars.chars().enumerate() {
            keys.push((row, column, 1, if entry.shift { c.to_ascii_uppercase() } else { c }.to_string()));
        }
    }
    let mut column = 0;
    for (key, span) in COMMANDS {
        let name = match key {
            Command2::Char(c) => c.to_string(),
            Command2::Shift => "Shift".into(),
            Command2::Space => "Space".into(),
            Command2::Backspace => "Back".into(),
            Command2::Done => "Done".into(),
        };
        keys.push((GRID.len(), column, span, name));
        column += span;
    }
    for (row, column, span, name) in keys {
        let (x, y) = (TYPED[0] + column as f32 * KEY[0], KEYS_Y + row as f32 * KEY[1]);
        let rect = [x + 1.5, y + 1.5, x + span as f32 * KEY[0] - 1.5, y + KEY[1] - 1.5];
        let lit = row == entry.row && (column..column + span).contains(&entry.column);
        if lit {
            out.push(quad(WHITE, [1, 1], [rect[0] - 3., rect[1] - 3., rect[2] + 3., rect[3] + 3.], colour(GLOW, 0.35)));
        }
        out.push(quad(WHITE, [1, 1], rect, if lit { colour(GLOW, 1.) } else { ink }));
        let height = if name.chars().count() > 1 { 15. } else { 19. };
        let x = (rect[0] + rect[2] - art.font.width(&name, height)) / 2.;
        out.extend(label(&art.font, &name, [x, rect[1] + (KEY[1] - 3. - height) / 2.], height, if lit { colour(INK, 1.) } else { [1.; 4] }));
    }
    // The boot keyboard's legend and the creator's half-row jump: the first line on the card (in the legend's
    // blue, as under the skater), the second on the pulp.
    let first = footer(art, &[("A", say(language, "ID_COMMON_SELECT", "Select")), ("B", say(language, "ID_COMMON_CANCEL", "Cancel")), ("R_Bumper", "Jump")], CARD[2] - 12., 594.);
    out.extend(first.into_iter().map(|mut d| {
        if d.texture == art.font.texture {
            d.multiply = colour(STEEL, 1.);
        }
        d
    }));
    out.extend(footer(art, &[("X", "Backspace"), ("Y", "Space"), ("Start", "Done")], CARD[2] - 12., 633.));
    out
}

/// A legend printed on a card's pulp, right-aligned to `right`: (button, text) pairs.
fn footer(art: &BoardArt, items: &[(&str, &str)], right: f32, y: f32) -> Vec<Draw> {
    const HEIGHT: f32 = 18.;
    const ICON: f32 = 24.;
    let mut out = Vec::new();
    let mut x = right;
    for (glyph, text) in items.iter().rev() {
        x -= art.font.width(text, HEIGHT);
        out.extend(label(&art.font, text, [x, y], HEIGHT, colour(INK, 1.)));
        x -= ICON * 4. / 3. + 4.;
        out.push(button(glyph, [x, y], ICON));
        x -= 14.;
    }
    out
}

/// X Options on the CAS card's footer, left of the movie's own A legend (the movie has slots for A and B only).
pub(crate) fn x_options(movie: &[Draw], art: &BoardArt, language: &Value) -> Vec<Draw> {
    let a = movie.iter().filter(|d| d.texture.ends_with("buttons/A.rgba")).filter_map(bounds).find(|b| b[1] > 600. && b[0] < CAS_CARD[2]);
    match a {
        Some(b) => footer(art, &[("X", say(language, "ID_COMMON_OPTIONS", "Options"))], b[0] - 10., 633.),
        None => Vec::new(),
    }
}

/// Each Layers row's picture (key, size), at the row's right end. `first`: the first row on screen.
pub(crate) fn layer_thumbs(rows: &[(String, [u32; 2])], first: usize) -> Vec<Draw> {
    rows.iter()
        .enumerate()
        .skip(first)
        .take(LIST_ROWS)
        .filter(|(_, (key, _))| !key.is_empty())
        .map(|(i, (key, size))| {
            let y = ROW_Y + (i - first) as f32 * ROW_PITCH;
            quad(key, *size, [CAS_CARD[2] - 44., y - 13., CAS_CARD[2] - 18., y + 13.], [1.; 4])
        })
        .collect()
}

/// Under the skater, in the movie's legend blue: retail Graphic Adjust's lines (verbatim), then Fine and the D-pad.
pub(crate) fn legend(editor: &Editor, art: &BoardArt, language: &Value) -> Vec<Draw> {
    // Not retail (user 10-19): the sticks work on Options too (all but the D-pad line) and place the piece on the
    // Add grid (its first two lines, clear of the movie's Rotate Skater), so the legend says so there.
    let shown = match editor.mode {
        Mode::Board => 6,
        Mode::Options => 5,
        Mode::Add => 2,
        _ => return Vec::new(),
    };
    let scale = if editor.stretch() { "Stretch Graphic" } else { say(language, "ID_CAS_SCALE", "Scale Graphic") };
    let dpad = if editor.reordering() { "Move Layer" } else { "Layer / Side" };
    let lines = [
        ("Stick_Right", say(language, "ID_CAS_MOVE", "Move Graphic")),
        ("Stick_Left", scale),
        ("L_Bumper", say(language, "ID_CAS_ROTATECC", "Rotate Counter-clockwise")),
        ("R_Bumper", say(language, "ID_CAS_ROTATE", "Rotate Clockwise")),
        ("L_Trigger", "Fine (hold)"),
        ("DPad", dpad),
    ];
    let mut out = Vec::new();
    for (i, (glyph, text)) in lines.iter().take(shown).enumerate() {
        let y = 430. + i as f32 * 38.;
        out.push(button(glyph, [930., y], 30.));
        out.extend(label(&art.font, text, [976., y + 4.], 22., colour(STEEL, 1.)));
    }
    out
}

/// The movie's own "LT RT Rotate Skater", which only the Add page keeps (the board owns the triggers).
pub(crate) fn is_rotate_skater(d: &Draw) -> bool {
    bounds(d).is_some_and(|[x0, y0, _, _]| x0 >= 900. && y0 >= 590. && y0 < 680.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_side_fits_the_room_and_renders_square_texels() {
        let tall = frame(Vec2::new(2000., 4000.), Mode::Board);
        assert!(tall[0] >= ROOM[0] && tall[2] <= ROOM[2] && (tall[3] - tall[1] - (ROOM[3] - ROOM[1])).abs() < 0.01);
        assert_eq!(board_pixels(Vec2::new(4000., 2000.)), [512, 256]);
        assert!(CARD[0] > CAS_CARD[2], "the board is right of the CAS card");
    }
}
