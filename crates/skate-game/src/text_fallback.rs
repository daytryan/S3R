//! Glyphs the retail Futura atlases lack (Cyrillic, CJK, emoji... in player and friend names), drawn from the
//! Windows system fonts at runtime (decision Q2b): no font file ships, and a name never shows boxes or "?".
//! Each glyph is rasterised once, off the frame on one glyph thread (the text draws without it until it is ready),
//! into white coverage in memory (tinted like atlas text), which every APT canvas loads by its name through
//! apt_canvas::texture_bytes. Fonts load lazily, only those a character's script asks for. C1: chat lets strangers
//! name thousands of characters, so the queue, the characters and the bytes held are capped.
use std::collections::HashMap;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

/// A system glyph in cap-height units (multiply by the Futura font's CapsHeight for its own units); `y_offset` is
/// the top above the baseline, as the atlas glyphs have it.
pub struct SystemGlyph {
    /// Served from memory by [`pixels`] (apt_canvas::texture_bytes), never read from disk.
    pub texture: String,
    coverage: Vec<u8>,
    pub size: [u32; 2],
    pub x_offset: f32,
    pub y_offset: f32,
    pub width: f32,
    pub height: f32,
    pub x_advance: f32,
}

/// Pixels per cap height when rasterising (names draw at most ~40 px tall).
const CAP_PX: f32 = 48.;
/// Segoe UI bold for Latin/Greek/Cyrillic (Futura's weight; also the cap-height scale), then the CJK, Korean,
/// Indic, African and South-East Asian UI fonts, then symbols and emoji.
const FONTS: [&str; 12] = [
    "segoeuib.ttf", "YuGothB.ttc", "msyhbd.ttc", "malgunbd.ttf", "NirmalaB.ttf", "ebrimabd.ttf", "LeelaUIb.ttf",
    "LeelawUI.ttf", "gadugib.ttf", "seguisym.ttf", "seguiemj.ttf", "seguihis.ttf",
];

/// Font `i`, read on first use (None: not installed).
fn face(i: usize) -> Option<&'static [u8]> {
    static FACES: [OnceLock<Option<Vec<u8>>>; FONTS.len()] = [const { OnceLock::new() }; FONTS.len()];
    FACES[i]
        .get_or_init(|| {
            let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
            std::fs::read(dir.join(FONTS[i])).ok()
        })
        .as_deref()
}

/// The fonts to try for `c`, its script's first (so a kana name reads Segoe UI and Yu Gothic, not every font).
fn candidates(c: char) -> Vec<usize> {
    let first: &[usize] = match c as u32 {
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => &[3],
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF => &[1, 2],
        0x0900..=0x0DFF => &[4],
        0x1200..=0x139F | 0x2D80..=0x2DDF => &[5],
        0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF => &[6, 7],
        0x13A0..=0x13FF | 0x1400..=0x167F => &[8],
        0x2190..=0x2BFF | 0x1F000..=0x1FAFF => &[10, 9],
        _ => &[0],
    };
    let mut out = first.to_vec();
    out.extend((0..FONTS.len()).filter(|i| !first.contains(i)));
    out
}

/// Characters that only modify the one before (emoji skin tones, variation selectors, flag letters and tags), and
/// zero-width and bidi format characters (no bidi here; 3.12):
/// drawn as nothing, so "👍🏽" is a plain thumbs-up rather than a thumb and a box.
pub(crate) fn modifier(c: char) -> bool {
    matches!(c as u32, 0x1F3FB..=0x1F3FF | 0xFE00..=0xFE0F | 0x1F1E6..=0x1F1FF | 0xE0000..=0xE007F | 0x200B..=0x200F
        | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF)
}

/// `text` in NFC, so a decomposed accent ("e" + U+0308, as some clients send) draws Futura's own "ë".
pub(crate) fn nfc(text: &str) -> std::borrow::Cow<'_, str> {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(text)
}

struct Outline<'a> {
    path: &'a mut PathBuilder,
    scale: f32,
    left: f32,
    top: f32,
}
impl Outline<'_> {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.left) * self.scale + 1., (self.top - y) * self.scale + 1.)
    }
}
impl ttf_parser::OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.move_to(p.0, p.1);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.line_to(p.0, p.1);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, p) = (self.at(x1, y1), self.at(x, y));
        self.path.quad_to(a.0, a.1, p.0, p.1);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, c, p) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.path.cubic_to(a.0, a.1, c.0, c.1, p.0, p.1);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

// ponytail: monochrome outlines (tinted by the text colour like Futura); colour emoji layers (COLR) are not painted.
fn rasterise(c: char) -> Option<SystemGlyph> {
    // Cap height of the first face (Segoe UI) sets the scale for all of them, so mixed scripts share one size.
    let first = ttf_parser::Face::parse(face(0)?, 0).ok()?;
    let cap = first.capital_height().map_or(0.7 * first.units_per_em() as f32, f32::from);
    for i in candidates(c) {
        let Some(Ok(face)) = face(i).map(|d| ttf_parser::Face::parse(d, 0)) else { continue };
        let Some(id) = face.glyph_index(c) else { continue };
        // Faces differ in units per em: bring each to Segoe UI's em before the cap-height scale.
        let em = first.units_per_em() as f32 / face.units_per_em() as f32;
        let advance = face.glyph_hor_advance(id).unwrap_or(0) as f32 * em / cap;
        let Some(bbox) = face.glyph_bounding_box(id) else {
            // A blank glyph (a space of another script): advance only.
            return (advance > 0.).then(|| SystemGlyph { texture: String::new(), coverage: Vec::new(), size: [0, 0], x_offset: 0., y_offset: 0., width: 0., height: 0., x_advance: advance });
        };
        let scale = CAP_PX * em / cap;
        let size = [
            ((bbox.x_max - bbox.x_min) as f32 * scale).ceil() as u32 + 2,
            ((bbox.y_max - bbox.y_min) as f32 * scale).ceil() as u32 + 2,
        ];
        let mut path = PathBuilder::new();
        let mut outline = Outline { path: &mut path, scale, left: bbox.x_min as f32, top: bbox.y_max as f32 };
        face.outline_glyph(id, &mut outline)?;
        let Some(path) = path.finish() else { continue };
        let mut pixmap = Pixmap::new(size[0], size[1])?;
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        let px = |v: f32| v / CAP_PX;
        return Some(SystemGlyph {
            texture: key(c),
            // Premultiplied white: every channel is the coverage, kept once.
            coverage: pixmap.data().chunks_exact(4).map(|p| p[3]).collect(),
            size,
            x_offset: bbox.x_min as f32 * em / cap - px(1.),
            y_offset: bbox.y_max as f32 * em / cap + px(1.),
            width: px(size[0] as f32),
            height: px(size[1] as f32),
            x_advance: advance,
        });
    }
    None
}

/// Distinct characters a session looks up; past it a new one draws the atlas box (C1: chat can name thousands).
const MAX_GLYPHS: usize = 2048;
/// Coverage bytes the drawn glyphs hold at most (a few KB each); past it, the box.
const MAX_BYTES: usize = 8 << 20;
/// Characters waiting for the glyph thread; more are asked again on a later frame.
const QUEUE: usize = 64;

enum Slot {
    /// Queued for, or being drawn on, the glyph thread.
    Pending,
    Ready(Option<&'static SystemGlyph>),
    /// Past [`MAX_BYTES`].
    Full,
}
type Slots = HashMap<char, Slot>;
/// perf 10-18: bumped whenever a slot is added or finishes, which can change what a cached draw would show
/// (apt_scene::draw_key).
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}
fn bump() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
fn cache() -> &'static Mutex<Slots> {
    static CACHE: OnceLock<Mutex<Slots>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The one thread that draws glyphs, fed by a bounded queue (None: the OS refused the thread; nothing falls back).
fn worker() -> Option<&'static SyncSender<char>> {
    static WORKER: OnceLock<Option<SyncSender<char>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (send, receive) = std::sync::mpsc::sync_channel::<char>(QUEUE);
            let spawned = std::thread::Builder::new().name("text-fallback".into()).spawn(move || {
                // Older builds cached glyphs as files here, which other programs could plant; never read now.
                let _ = std::fs::remove_dir_all(std::env::temp_dir().join("skate3-glyphs"));
                let mut bytes = 0;
                for c in receive {
                    let slot = match rasterise(c) {
                        Some(g) if bytes + g.coverage.len() > MAX_BYTES => Slot::Full,
                        g => {
                            bytes += g.as_ref().map_or(0, |g| g.coverage.len());
                            // ponytail: leaked once each, bounded by MAX_GLYPHS / MAX_BYTES; evict if sessions outgrow it.
                            Slot::Ready(g.map(|g| &*Box::leak(Box::new(g))))
                        }
                    };
                    if let Ok(mut slots) = cache().lock() {
                        slots.insert(c, slot);
                    }
                    bump();
                }
            });
            spawned.ok().map(|_| send)
        })
        .as_ref()
}

/// Past the session's glyph budget: the caller draws a box.
#[derive(Debug)]
pub struct Full;

/// The system font glyph for `c`: Ok(None) while it is being drawn (the first time it is asked for) or when no
/// installed font has it; Err past [`MAX_GLYPHS`] / [`MAX_BYTES`].
pub fn glyph(c: char) -> Result<Option<&'static SystemGlyph>, Full> {
    let Ok(mut slots) = cache().lock() else { return Ok(None) };
    ask(&mut slots, c, MAX_GLYPHS, worker())
}
fn ask(slots: &mut Slots, c: char, max: usize, worker: Option<&SyncSender<char>>) -> Result<Option<&'static SystemGlyph>, Full> {
    match slots.get(&c) {
        Some(Slot::Ready(g)) => return Ok(*g),
        Some(Slot::Pending) => return Ok(None),
        Some(Slot::Full) => return Err(Full),
        None if slots.len() >= max => return Err(Full),
        None => {}
    }
    bump();
    match worker.map(|w| w.try_send(c)) {
        Some(Ok(())) => slots.insert(c, Slot::Pending),
        // The queue is full: asked again next frame.
        Some(Err(TrySendError::Full(_))) => None,
        _ => slots.insert(c, Slot::Ready(None)),
    };
    Ok(None)
}

/// Where glyph textures are named: an absolute path (a canvas root never re-roots it), never read from disk.
fn dir() -> &'static str {
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| std::env::temp_dir().join("skate3-glyphs").to_string_lossy().into_owned())
}
fn key(c: char) -> String {
    format!("{}{}{:x}.rgba", dir(), std::path::MAIN_SEPARATOR, c as u32)
}
/// Whether `path` names a system glyph's texture ([`pixels`]).
pub fn is_glyph(path: &str) -> bool {
    path.starts_with(dir())
}
/// A system glyph's RGBA texture by its [`SystemGlyph::texture`] name; None for any other path (an Err for a glyph
/// name this session never drew: such a path is never read from disk).
pub fn pixels(path: &str) -> Option<Result<Vec<u8>, String>> {
    let rest = path.strip_prefix(dir())?;
    let c = rest
        .strip_prefix(std::path::MAIN_SEPARATOR)
        .and_then(|r| r.strip_suffix(".rgba"))
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .and_then(char::from_u32);
    let glyph = c.and_then(|c| match cache().lock().ok()?.get(&c) {
        Some(Slot::Ready(Some(g))) => Some(*g),
        _ => None,
    });
    Some(glyph.map(|g| g.coverage.iter().flat_map(|&a| [a; 4]).collect()).ok_or_else(|| format!("{path}: no system glyph")))
}

/// [`glyph`], waiting for it to be drawn (tests).
#[cfg(test)]
pub fn glyph_now(c: char) -> Option<&'static SystemGlyph> {
    for _ in 0..2000 {
        if let Ok(Some(g)) = glyph(c) {
            return Some(g);
        }
        if matches!(cache().lock().unwrap().get(&c), Some(Slot::Ready(None))) {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("glyph {c} never finished");
}

#[cfg(test)]
mod tests {
    /// Cyrillic, kana and emoji all come from installed fonts (a Windows test machine), with coverage drawn, off
    /// the caller's thread (the first ask is None).
    #[test]
    fn names_in_other_scripts_get_system_glyphs() {
        assert!(matches!(super::glyph('Д'), Ok(None)), "the first ask starts the drawing and returns at once");
        // ('ー' is a thin bar, so the size check uses 'ケ'.)
        for c in ['В', 'л', 'ス', 'ケ', '😀', 'Д', '한'] {
            let g = super::glyph_now(c).unwrap_or_else(|| panic!("no system glyph for {c}"));
            let bytes = super::pixels(&g.texture).unwrap().unwrap();
            assert_eq!(bytes.len(), (g.size[0] * g.size[1] * 4) as usize);
            assert!(bytes.chunks(4).any(|p| p[3] > 128), "{c} is drawn");
            assert!(g.x_advance > 0.3 && g.height > 0.5, "{c} is name-sized");
        }
        // Cap-height units: Segoe UI's own 'H' is one cap tall.
        let h = super::glyph_now('H').unwrap();
        assert!((h.y_offset - 1.).abs() < 0.05, "{}", h.y_offset);
        // Only the script's fonts load: a kana name never reads the emoji or Korean fonts first.
        assert_eq!(super::candidates('ス')[..2], [1, 2]);
        assert_eq!(super::candidates('😀')[0], 10);
    }

    /// C1: a chat line of rare characters queues them for the one glyph thread (no thread each), a full queue
    /// leaves the rest for a later frame, the session cap answers Full (a box), and a refused thread only means no
    /// fallback. A glyph name is served from memory: a planted or stale file under that name is never read.
    #[test]
    fn rare_characters_are_queued_and_capped() {
        let (send, receive) = std::sync::mpsc::sync_channel(2);
        let mut slots = super::Slots::default();
        for c in ['一', '二', '三', '四'] {
            assert!(matches!(super::ask(&mut slots, c, 3, Some(&send)), Ok(None)));
        }
        assert_eq!(receive.try_iter().collect::<Vec<_>>(), ['一', '二'], "two queued, the rest wait");
        assert!(super::ask(&mut slots, '三', 3, Some(&send)).is_ok(), "asked again once there is room");
        assert!(super::ask(&mut slots, '四', 3, Some(&send)).is_err(), "past the session cap: a box");
        assert!(super::ask(&mut slots, '一', 3, Some(&send)).is_ok(), "known characters still draw");
        let mut none = super::Slots::default();
        assert!(matches!(super::ask(&mut none, '五', 3, None), Ok(None)));
        assert!(matches!(none.get(&'五'), Some(super::Slot::Ready(None))), "no thread: nothing, no retry");
        let planted = super::key('\u{10FFFC}');
        let _ = std::fs::create_dir_all(std::path::Path::new(&planted).parent().unwrap());
        let _ = std::fs::write(&planted, [1u8; 7]);
        assert!(super::pixels(&planted).unwrap().is_err(), "never drawn here: not read from disk");
        let _ = std::fs::remove_file(&planted);
        assert!(super::pixels("C:/game/futura.rgba").is_none(), "other textures are not glyphs");
    }

    /// 3.12/C5: a name can't draw controller button glyphs (apt_scene's `<B:Name>` tags and the U+E000.. they become),
    /// and zero-width / bidi format characters draw nothing (no fallback lookup); past the glyph budget, the box.
    #[test]
    fn names_cannot_draw_buttons_or_invisible_glyphs() {
        let text = crate::apt_text::TextAssets::default();
        let shown = text.localize(&crate::apt_text::literal("Press <B:A>\u{E000}\u{E001}"));
        assert!(!shown.contains("<B:") && !shown.contains('\u{E000}') && !shown.contains('\u{E001}'), "{shown:?}");
        assert_eq!(shown.replace('\u{2060}', ""), "Press <B:A>", "still reads as typed");
        assert_eq!(crate::apt_text::literal("Ann"), "#\u{1}Ann");
        let font = crate::apt_text::Font { foreground: None, texture: "futura.rgba".into(), size: [1; 2], scale: [1.; 2], offset: [0.; 2], ascent: 1., caps: 1., glyphs: Default::default() };
        for c in ['\u{2060}', '\u{200B}', '\u{202E}', '\u{2066}', '\u{FEFF}', '\u{E0041}'] {
            assert!(font.place(c).is_none(), "{:x} draws nothing", c as u32);
            assert!(super::cache().lock().unwrap().get(&c).is_none(), "{:x} never asks a system font", c as u32);
        }
    }

    /// The shared text path (apt_text::Font::place, used by apt_scene, the online name tags and the boot
    /// keyboard): Futura's own glyph first, a system glyph for what it lacks, never its 65535 box; emoji modifiers
    /// and flag letters draw nothing; NFC brings a decomposed accent back to Futura's precomposed glyph.
    #[test]
    fn futura_keeps_its_glyphs_and_falls_back_for_the_rest() {
        let atlas = crate::apt_text::Glyph { glyph_index: 1, width: 20., height: 30., x_offset: 1., y_offset: 30., x_advance: 22., atlas_bounds: [0., 0., 20., 30.] };
        let font = crate::apt_text::Font {
            foreground: None,
            texture: "futura.rgba".into(),
            size: [512, 512],
            scale: [0.028; 2],
            offset: [0.; 2],
            ascent: 37.,
            caps: 30.,
            glyphs: [('A' as u32, atlas.clone()), ('ë' as u32, atlas.clone()), (65535, atlas)].into(),
        };
        assert_eq!(font.place('A').unwrap().1, "futura.rgba");
        for c in ['Ж', '’', '\u{10FFFD}'] {
            super::glyph_now(c);
        }
        let (g, texture, _) = font.place('Ж').unwrap();
        assert_ne!(texture, "futura.rgba");
        assert!((g.y_offset - 30.).abs() < 2., "a capital sits a cap tall: {}", g.y_offset);
        assert!(font.width("Ж", 20.) > 0.);
        // Nothing anywhere draws for an unassigned code point: skipped, not the atlas box.
        assert!(font.place('\u{10FFFD}').is_none());
        // The string tables' 0x81 byte is an apostrophe.
        assert!(font.place('\u{81}').is_some());
        for c in ['\u{1F3FD}', '\u{FE0F}', '\u{1F1EF}', '\u{200D}'] {
            assert!(font.place(c).is_none(), "{:x} draws nothing", c as u32);
        }
        assert_eq!(super::nfc("Zoe\u{308}"), "Zoë");
        assert_eq!(font.place(super::nfc("e\u{308}").chars().next().unwrap()).unwrap().1, "futura.rgba");
        // A name is never a string id, and keeps its spaces; composed ids still resolve.
        let mut text = crate::apt_text::TextAssets::default();
        std::sync::Arc::make_mut(&mut text.language).insert("ID_COMMON_YES".into(), "Yes".into());
        assert_eq!(text.localize(&crate::apt_text::literal("ID_COMMON_YES")), "ID_COMMON_YES");
        assert_eq!(text.localize(&crate::apt_text::literal("a  b\u{1}")), "a  b");
        assert_eq!(text.localize("#ID_COMMON_YES 360"), "Yes 360");
    }
}
