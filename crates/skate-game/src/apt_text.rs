//! SkateAptString82CA14D0 and cFont828076D0/82808708 layout.
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
pub struct Glyph {
    pub glyph_index: usize,
    pub width: f32,
    pub height: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub x_advance: f32,
    pub atlas_bounds: [f32; 4],
}
#[derive(Clone)]
pub struct Font {
    /// 825D6B68 attaches futuraheavy to Futura Shadow; 82CA1FD8 draws both.
    pub foreground: Option<i32>,
    pub texture: String,
    pub size: [u32; 2],
    pub scale: [f32; 2],
    pub offset: [f32; 2],
    pub ascent: f32,
    /// Metrics CapsHeight: the size of a system-font fallback glyph ([`crate::text_fallback`]).
    pub caps: f32,
    pub glyphs: BTreeMap<u32, Glyph>,
}
#[derive(Clone, Default)]
pub struct TextAssets {
    pub fonts: BTreeMap<i32, Font>,
    /// Shared by every movie with the same table ([`interned_language`]).
    pub language: std::sync::Arc<BTreeMap<String, String>>,
    /// Authored font family -> font character id (TextFormat.font lookups).
    pub names: BTreeMap<String, i32>,
    /// Fonts mapped to the "debug" bitmap (Arial, "NOT FOR USE IN APT"): never a fallback.
    pub debug: Vec<i32>,
}
impl TextAssets {
    /// The font a TextFormat names: exact family, else the same family with a
    /// different weight ("Futura Std Heavy" -> "Futura Std Medium"), else any.
    pub fn font_named(&self, name: &str) -> Option<i32> {
        let family = |n: &str| n.rsplit_once(' ').map_or(n.to_owned(), |(f, _)| f.to_owned());
        self.names
            .get(name)
            .or_else(|| self.names.iter().find(|(n, _)| family(n) == family(name)).map(|(_, id)| id))
            // INFERRED: an unknown family (credits' "Helvetica LT Std") gets a real font, not the debug bitmap
            // (user 10-14, credits giant and blocky). UNTRACED: retail's own fallback.
            .or_else(|| self.fonts.keys().find(|id| !self.debug.contains(id)))
            .or_else(|| self.fonts.keys().next())
            .copied()
    }
}
/// perf 10-18: one copy of each language table. Every movie export embeds the same ~1 MB one, and each open built it
/// into a new map (and freed it on close); a table equal to one kept is shared instead. SKATE_INTERN_TEXT=0 builds
/// per load. ponytail: a scan of the few kept tables (same length, then every entry), capped at [`KEPT`].
fn interned_language(table: &serde_json::Value) -> Result<std::sync::Arc<BTreeMap<String, String>>, String> {
    const KEPT: usize = 8;
    type Table = std::sync::Arc<BTreeMap<String, String>>;
    static TABLES: std::sync::Mutex<Vec<Table>> = std::sync::Mutex::new(Vec::new());
    let build = || -> Result<Table, String> { serde::Deserialize::deserialize(table).map(Table::new).map_err(|e| e.to_string()) };
    let Some(entries) = table.as_object().filter(|_| crate::config::perf_on("SKATE_INTERN_TEXT")) else { return build() };
    let same = |t: &Table| {
        t.len() == entries.len() && entries.iter().all(|(k, v)| v.as_str().is_some_and(|v| t.get(k).is_some_and(|s| s == v)))
    };
    let mut tables = TABLES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(kept) = tables.iter().find(|t| same(t)) {
        return Ok(kept.clone());
    }
    let built = build()?;
    if tables.len() >= KEPT {
        tables.remove(0);
    }
    tables.push(built.clone());
    Ok(built)
}
impl TextAssets {
    pub fn load(json: &serde_json::Value) -> Result<Self, String> {
        let mut out = Self::default();
        out.language = interned_language(&json["language"])?;
        for c in json["characters"]
            .as_array()
            .ok_or("HUD characters missing")?
        {
            if c["type_name"] != "font" {
                continue;
            }
            let name = c["font"]["name"].as_str().ok_or("HUD font name missing")?;
            if let Some(id) = c["id"].as_i64() {
                out.names.entry(name.to_owned()).or_insert(id as i32);
            }
            let asset = &json["fonts"][name];
            if json["font_mappings"][name]["file_name"] == "debug" {
                out.debug.extend(c["id"].as_i64().map(|id| id as i32));
            }
            let d = &asset["definition"];
            let glyphs: Vec<Glyph> =
                serde::Deserialize::deserialize(&d["glyphs"]).map_err(|e| e.to_string())?;
            let layout = &json["font_mappings"][name]["native_layout"];
            let f = |key: &str| {
                layout[key]
                    .as_f64()
                    .map(|v| v as f32)
                    .ok_or_else(|| format!("Missing native font {name}.{key}"))
            };
            let mut font = Font {
                foreground: None,
                texture: asset["texture"]
                    .as_str()
                    .ok_or("Font texture missing")?
                    .into(),
                size: [
                    d["textures"][0]["width"]
                        .as_u64()
                        .ok_or("Font width missing")? as u32,
                    d["textures"][0]["height"]
                        .as_u64()
                        .ok_or("Font height missing")? as u32,
                ],
                scale: [f("ScaleX")?, f("ScaleY")?],
                offset: [f("OffsetX")?, f("OffsetY")?],
                ascent: d["metrics"]["Ascent"]
                    .as_f64()
                    .ok_or("Font ascent missing")? as f32,
                caps: 0.,
                glyphs: BTreeMap::new(),
            };
            font.caps = d["metrics"]["CapsHeight"].as_f64().map_or(font.ascent * 0.8, |v| v as f32);
            for mapping in d["characters"]
                .as_array()
                .ok_or("Font character map missing")?
            {
                let index = mapping["glyph_index"]
                    .as_u64()
                    .ok_or("Invalid font glyph index")? as usize;
                let glyph = glyphs
                    .iter()
                    .find(|g| g.glyph_index == index)
                    .ok_or("Absent mapped glyph")?;
                font.glyphs.insert(
                    mapping["codepoint"]
                        .as_u64()
                        .ok_or("Invalid font codepoint")? as u32,
                    glyph.clone(),
                );
            }
            out.fonts.insert(
                c["id"].as_i64().ok_or("Invalid font character id")? as i32,
                font,
            );
        }
        for c in json["characters"].as_array().unwrap() {
            if c["type_name"] == "font" && c["font"]["name"] == "Futura Shadow" {
                let foreground = json["characters"].as_array().unwrap().iter().find(|f|
                    f["type_name"] == "font" && json["font_mappings"][f["font"]["name"].as_str().unwrap_or("")]["file_name"] == "futuraheavy")
                    .and_then(|f| f["id"].as_i64()).ok_or("Missing native Futura Shadow foreground font")? as i32;
                out.fonts
                    .get_mut(&(c["id"].as_i64().unwrap() as i32))
                    .unwrap()
                    .foreground = Some(foreground);
            }
        }
        Ok(out)
    }
    /// A leading `#` means *resolve every token*, not "leave this alone".
    ///
    /// The trick display's name is composed by `sub_825E51A0`, which formats with the
    /// literals `"#%s %d "` (8220E43C) and `"#%s "` (8220E444) and then `strcat`s further
    /// localisation ids onto the result. So a displayed name is one `#` followed by
    /// space-separated ids and bare numbers -- `#ID_TRICK_FLIP_KICKFLIP 360
    /// ID_TRICK_AIR_METRICS_FRONTFLIP` -- and each token is resolved separately. Treating
    /// `#` as "literal" is the inverse of that, and left every composed name unreadable.
    ///
    /// A token with no entry, such as a spin's `360`, is kept exactly as written.
    ///
    /// A [`literal`] (a player's, friend's or team's name) is shown exactly as written: never looked up.
    pub fn localize(&self, text: &str) -> String {
        if let Some(name) = text.strip_prefix(LITERAL) {
            return name.into();
        }
        let Some(composed) = text.strip_prefix('#') else {
            return self
                .language
                .get(text)
                .cloned()
                .unwrap_or_else(|| text.into());
        };
        composed
            .split_ascii_whitespace()
            .map(|token| {
                self.language
                    .get(token)
                    .map_or(token, String::as_str)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}
impl Font {
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs
            .get(&(c as u32))
            .or_else(|| self.glyphs.get(&65535))
    }
    /// What text draws for `c`: this font's atlas glyph, else a Windows system font's (its own texture and
    /// size; decision Q2b), else nothing -- never the atlas's 65535 box. (glyph, texture, texture size).
    pub fn place(&self, c: char) -> Option<(Glyph, &str, [u32; 2])> {
        // The string tables' bytes 0x81 / 0x82 read as an apostrophe and an ellipsis (as prepare_retail_screens.py
        // maps them); data that kept the raw bytes draws them here.
        let c = match c {
            '\u{81}' => '’',
            '\u{82}' => '…',
            c if crate::text_fallback::modifier(c) => return None,
            c => c,
        };
        if let Some(g) = self.glyphs.get(&(c as u32)) {
            return Some((g.clone(), &self.texture, self.size));
        }
        // Past the session's fallback budget (C1), the atlas box: no new texture.
        let Ok(s) = crate::text_fallback::glyph(c) else {
            return self.glyphs.get(&65535).map(|g| (g.clone(), self.texture.as_str(), self.size));
        };
        let s = s?;
        let k = self.caps;
        let glyph = Glyph {
            glyph_index: usize::MAX,
            width: s.width * k,
            height: s.height * k,
            x_offset: s.x_offset * k,
            y_offset: s.y_offset * k,
            x_advance: s.x_advance * k,
            atlas_bounds: [0., 0., s.size[0] as f32, s.size[1] as f32],
        };
        Some((glyph, &s.texture, s.size))
    }
    pub fn width(&self, text: &str, height: f32) -> f32 {
        text.chars()
            .filter_map(|c| self.place(c))
            .fold(0.0, |x, (g, _, _)| g.x_advance.mul_add(self.scale[0] * height, x))
    }
    /// Lines of a TextField: explicit breaks always (see [`paragraphs`]); with word wrap, greedy
    /// breaks at spaces within `width` (a word longer than a line stays whole).
    pub fn lines(&self, text: &str, height: f32, width: Option<f32>) -> Vec<String> {
        let mut out = Vec::new();
        for paragraph in paragraphs(text) {
            let Some(width) = width else {
                out.push(paragraph.to_owned());
                continue;
            };
            let mut line = String::new();
            for word in paragraph.split(' ') {
                let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
                if !line.is_empty() && self.width(&candidate, height) > width {
                    out.push(std::mem::replace(&mut line, word.to_owned()));
                } else {
                    line = candidate;
                }
            }
            out.push(line);
        }
        out
    }
}
/// `text` cut at its explicit breaks: a newline, and the string tables' `^` (retail's SkateAptString layout breaks on
/// it, 0x82CA1BB8; the replay's popups and help showed it raw, user 10-27). A `^` right after a word joiner is a
/// player's own ([`unbroken`]) and stays a character.
fn paragraphs(text: &str) -> Vec<&str> {
    let (mut out, mut start, mut joined) = (Vec::new(), 0, false);
    for (i, c) in text.char_indices() {
        if matches!(c, '\n' | '\r') || (c == '^' && !joined) {
            out.push(&text[start..i]);
            start = i + c.len_utf8();
        }
        joined = c == '\u{2060}';
    }
    out.push(&text[start..]);
    out
}
/// A player's text (a name, a search) whose `^` breaks no line: each gets a word joiner before it, which draws as
/// nothing (text_fallback::modifier).
pub fn unbroken(text: &str) -> String {
    text.replace('^', "\u{2060}^")
}
/// Line pitch of authored text fields: font height plus Flash's leading
/// (a 20 px field is 27.35 px tall for one line, including the 2 px gutters).
pub const LINE_PITCH: f32 = 1.1675;

/// Marks text that is shown as written and never looked up as a string id (see [`TextAssets::localize`]).
const LITERAL: &str = "#\u{1}";
/// A name (player, friend, team, skater) for a movie's text: shown exactly, whatever it spells ("ID_COMMON_YES",
/// double spaces). Control characters in the name itself are dropped, so nothing can fake the marker; so is private
/// use, and a `<B:` keeps a word joiner after its `<`: apt_scene draws both as controller button glyphs. Its `^`
/// break no line ([`unbroken`]).
pub fn literal(name: &str) -> String {
    let name: String = name.chars().filter(|&c| !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(&c)).collect();
    format!("{LITERAL}{}", unbroken(&name.replace("<B:", "<\u{2060}B:")))
}
/// Not retail (user 10-25): leads a [`gold`] literal. A control character, which [`literal`] drops from any name, so
/// only the game writes it.
const GOLD: char = '\u{2}';
/// A [`literal`] drawn in the creator tag's colour (creator::colour, a rainbow sweep; gold until 10-26) whatever colour
/// its field has (apt_movie::text_changed takes the marker off and flags the field; apt_scene paints it).
pub fn gold(name: &str) -> String {
    let plain = literal(name);
    format!("{LITERAL}{GOLD}{}", &plain[LITERAL.len()..])
}
/// `shown` without the leading [`GOLD`] marker, and whether it had one.
pub fn split_gold(shown: &str) -> (&str, bool) {
    shown.strip_prefix(GOLD).map_or((shown, false), |rest| (rest, true))
}
/// Not retail (user 10-27): follows [`LITERAL`] (and [`GOLD`]) in a [`fit`] literal. A control character, which
/// [`literal`] drops from any name, so only the game writes it.
const FIT: char = '\u{3}';
/// A [`literal`] (or [`gold`]) that apt_scene shrinks into its row, then cuts with "…" (the server browser's and the
/// Player List's rows ran off the menu: user 10-27); anything else as it is.
pub fn fit(shown: &str) -> String {
    let Some(rest) = shown.strip_prefix(LITERAL) else { return shown.to_owned() };
    let (rest, gold) = split_gold(rest);
    let gold = if gold { GOLD.to_string() } else { String::new() };
    format!("{LITERAL}{gold}{FIT}{rest}")
}
/// `shown` (its [`GOLD`] marker taken off) without the leading [`FIT`] marker, and whether it had one.
pub fn split_fit(shown: &str) -> (&str, bool) {
    shown.strip_prefix(FIT).map_or((shown, false), |rest| (rest, true))
}
/// A [`literal`] (or [`gold`]) with a controller glyph before it (`<B:name>`, drawn inline by apt_scene); anything else
/// as it is.
pub fn glyph_first(name: &str, shown: &str) -> String {
    let Some(rest) = shown.strip_prefix(LITERAL) else { return shown.to_owned() };
    let (rest, gold) = split_gold(rest);
    let gold = if gold { GOLD.to_string() } else { String::new() };
    format!("{LITERAL}{gold}<B:{name}> {rest}")
}
/// Wrapping width, or None for single-line fields.
/// A script's TextField.wordWrap wins over the authored flag: nucleus_tos turns it off on its line fields, which it
/// fills to mMaxTextLineWidth itself (user 10-14, Licenses lines overlapping).
pub fn wrap_width(text: &serde_json::Value, bounds: [f32; 4], scripted: &crate::apt_vm::Value) -> Option<f32> {
    let wrap = match scripted {
        crate::apt_vm::Value::Bool(b) => *b,
        _ => text["word_wrap"].as_bool() == Some(true),
    };
    wrap.then_some(bounds[2] - bounds[0] - 4.)
}

#[cfg(test)]
mod tests {
    #[test]
    fn equal_language_tables_are_shared() {
        let table = serde_json::json!({"ID_INTERN_A": "a", "ID_INTERN_B": "b"});
        let (a, b) = (super::interned_language(&table).unwrap(), super::interned_language(&table.clone()).unwrap());
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        let other = super::interned_language(&serde_json::json!({"ID_INTERN_A": "a", "ID_INTERN_B": "c"})).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a, &other) && other["ID_INTERN_B"] == "c");
        assert!(super::interned_language(&serde_json::json!({"ID_INTERN_A": 1})).is_err(), "a table of non-strings still fails");
    }

    /// A fit literal keeps its gold and loses both markers on the way to the field; a name can't fake one; a string
    /// id stays a string id.
    #[test]
    fn a_fit_literal_carries_its_marker_after_gold() {
        let gold_name = super::fit(&super::gold("Dee - Creator"));
        let (rest, gold) = super::split_gold(&gold_name[super::LITERAL.len()..]);
        assert_eq!((super::split_fit(rest), gold), (("Dee - Creator", true), true));
        let name = super::fit(&super::literal("Bo\u{3}x"));
        assert_eq!(super::split_fit(&name[super::LITERAL.len()..]), ("Box", true), "the name's own \\u{{3}} is dropped");
        assert_eq!(super::fit("ID_COMMON_BACK"), "ID_COMMON_BACK");
    }

    /// User 10-27 ("This will take a few moments to process.^^Continue?"): the string tables' `^` breaks the line, as
    /// retail's layout does (0x82CA1BB8), alongside newlines; a player's own `^` (a literal, or [`super::unbroken`]
    /// text spliced into a string) stays on its line.
    #[test]
    fn a_table_caret_breaks_the_line_and_a_name_caret_does_not() {
        assert_eq!(super::paragraphs("This will take a few moments to process.^^Continue?"), ["This will take a few moments to process.", "", "Continue?"]);
        assert_eq!(super::paragraphs("a\nb^c"), ["a", "b", "c"]);
        let name = super::literal("Bo^Host left");
        assert_eq!(super::paragraphs(&name[super::LITERAL.len()..]), ["Bo\u{2060}^Host left"]);
        assert_eq!(super::paragraphs(&format!("Kick {}?^No rejoining.", super::unbroken("x^y"))), ["Kick x\u{2060}^y?", "No rejoining."]);
    }
}
