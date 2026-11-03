//! Original Create-a-Skater screen (cas/createskater) on the shared APT runtime.
//! The movie asks the native CAC system for everything it shows through
//! `Game.CAC_*`; this host answers from one page the owner supplies and reports
//! what the player did. Bevy-independent: create_skater.rs owns the skater.
use crate::{
    apt_movie::Movie,
    apt_scene::{Shapes, Texture},
    apt_vm::{Host, Value, Vm},
    core_menu_runtime::{
        array, boot_screen, common_call, define_fe_sounds, drain, fe_sound_id, native_vm, played_sound, screen_property_changed, Key, ScreenHost,
        SOUND_A_BUTTON,
    },
};
use std::collections::BTreeMap;

/// createskater.SetState values: its authored sub-screens, plus refresh requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Text list (menu picker): categories and simple choices.
    List = 0,
    /// 3x3 thumbnail browser of parts.
    Thumbs = 1,
    /// Option rows: selectors, sliders, colour swatches.
    Style = 2,
    /// Face/body morph sliders.
    Morph = 3,
    /// Graphic Adjust (mScalemenu): no rows, the stick/bumper legend.
    Scale = 4,
    /// Tattoo Graphic Adjust (mTattoomenu): Scale's legend plus clothing on/off.
    Tattoo = 5,
    /// 3x5 thumbnail browser (mThumbs2): the logo grids.
    ThumbsCustom = 6,
}
impl Mode {
    /// Thumbnail rows on screen.
    fn window_rows(self) -> usize {
        if self == Mode::ThumbsCustom { 5 } else { 3 }
    }
}
const REFRESH_ITEM: f64 = 50.;
/// A searchable grid's extra legend (ours, item 11: retail has no search), after the movie's B and A in its
/// 4-item button_menu. Retail's OnButton (0x825fc7e8) gives X and Y nothing on the part grids (page 10/11 modes 1
/// and 6); X is "Search" (ID_COMMON_SEARCH), Y "Top" (no retail string).
const SEARCH_LEGEND: [(&str, &str); 2] = [("Y", "#Top"), ("X", "ID_COMMON_SEARCH")];
/// Beside a game-filtered grid's name (cas_menu::GAMES).
const GAME_PROMPT: &str = "LB / RB";

/// String-table markup as plain text: '^' breaks the line and `<B:DPad_Down_hud>`
/// names its button. ponytail: words, not glyphs, until APT text draws inline icons.
fn plain_markup(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("<B:") {
        out.push_str(before);
        let (button, tail) = after.split_once('>').unwrap_or((after, ""));
        out.push_str(&button.trim_start_matches('~').trim_end_matches("_hud").replace("DPad", "D-Pad").replace('_', " "));
        rest = tail;
    }
    out.push_str(rest);
    out.replace('^', "\n").trim_end().to_owned()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Row {
    pub text: String,
    /// CAC_GetOptionType. Thumbs: icon frame ("none", "owned", "new", "locked", "dlc").
    /// Style/Morph: row control ("option", "slider", "selector", "morph", "color", "category").
    pub kind: &'static str,
    /// Key into the prepared part thumbnails.
    pub thumbnail: String,
    /// Info panel text; empty hides the panel.
    pub description: String,
    /// Info panel frame for parts ("owned", "locked", ...); empty: blurb/noblurb.
    pub panel: &'static str,
    /// Brand logo (thumbnail key) shown beside a part's name.
    pub logo: String,
    /// CAC_GetIntegerValue: slider bars (0-10), morph position, swatch index.
    pub value: f64,
    /// CAC_GetStringValue: a selector's current choice.
    pub value_text: String,
    /// Swatch colours for "color" rows (0xRRGGBB).
    pub colours: Vec<u32>,
    pub new: bool,
    /// The game an item comes from ("Skate 1", "Skate 2", "Skate 3"; empty: not an item), for the grid search.
    pub game: &'static str,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Page {
    pub title: String,
    pub mode: Mode,
    pub rows: Vec<Row>,
    pub index: usize,
    /// Button legends ("#" hides one).
    pub accept: String,
    pub back: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The highlighted row changed (the original previews parts on highlight).
    Hilight(usize),
    Accept(usize),
    Adjust(usize, i32),
    Back,
}

pub(crate) struct Bindings {
    pub movie: Movie,
    pub shapes: Shapes,
    pub page: Page,
    /// The first row (tile) on screen.
    pub first: usize,
    /// Prepared part thumbnails by lower-case key; the Graphic Creator adds its own.
    pub thumbnails: BTreeMap<String, Texture>,
    slots: u32,
    random: u32,
    pub intro_complete: bool,
    pub outro_complete: bool,
    /// Front-end sound ids (`GlobalFEPlaySound`) the movie played for the last input.
    pub sounds: Vec<u64>,
    /// A searchable grid's search (Runtime::show_searchable): the title shows it and the legend gains
    /// SEARCH_LEGEND. None on every other page.
    pub search: Option<String>,
    /// The movie just ran UpdateButtonHelp (it asked CAC_GetButtonHelpTextB): SEARCH_LEGEND goes on after it.
    legend_due: bool,
    /// More of ours after it (Runtime::show_with; item 31: X Colour Wheel). Empty on most pages.
    pub legend: Vec<(&'static str, &'static str)>,
}
impl Bindings {
    /// The page title; a grid search takes its place, after retail's "Filter:" (ID_CAC_FILTER_PREFIX). A search
    /// wider than the header shows its end.
    fn title(&self) -> String {
        let text = &self.movie.text_assets;
        let Some(search) = self.search.as_ref().filter(|s| !s.is_empty()) else {
            // A grid that RB / LB narrow by game (its bar has the bumpers) says so beside its name.
            if !self.legend.iter().any(|(button, _)| *button == "R_Bumper") {
                return self.page.title.clone();
            }
            return format!("#{}   {GAME_PROMPT}", text.localize(&self.page.title));
        };
        let prefix = text.localize("ID_CAC_FILTER_PREFIX");
        let font = text.font_named("Futura Std Medium").and_then(|id| text.fonts.get(&id));
        let width = |s: &str| font.map_or(s.chars().count() as f32 * 10., |f| f.width(s, 20.));
        // ponytail: the header's room, as a measured render fitted it ("Hats - Filter: skate 2"); measure the
        // header clip instead if it ever matters.
        let room = width("Hats - Filter: skate 2");
        let mut tail: Vec<char> = search.chars().collect();
        let mut shown = search.clone();
        while width(&format!("{prefix} {shown}")) > room && !tail.is_empty() {
            tail.remove(0);
            shown = format!("...{}", tail.iter().collect::<String>());
        }
        format!("#{prefix} {shown}")
    }
    fn row(&self, index: &Value) -> Option<&Row> {
        self.page.rows.get(index.number().max(0.) as usize)
    }
}
impl ScreenHost for Bindings {
    fn movie(&mut self) -> &mut Movie {
        &mut self.movie
    }
    fn random(&mut self) -> &mut u32 {
        &mut self.random
    }
}
impl Host for Bindings {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        screen_property_changed(&mut self.movie, vm, object, key)
    }
    fn call(&mut self, vm: &mut Vm, object: usize, method: &str, args: Vec<Value>) -> Result<Value, String> {
        if let Some(key) = played_sound(vm, object, method, &args) {
            self.sounds.push(fe_sound_id(&key));
            return Ok(Value::Undefined);
        }
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(crate::apt_vm::ObjectKind::Native(name)) => name.clone(),
            _ => "global".into(),
        };
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let text = |row: Option<&Row>, f: fn(&Row) -> &str| Value::Text(row.map_or("", f).into());
        Ok(match (native.as_str(), method) {
            ("ScreenManager", "IntroComplete") => {
                self.intro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", "OutroComplete") => {
                self.outro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", _) => Value::Undefined,
            // Selection reports. The owner reads `page.index` after each input.
            ("Game", "CAC_UpdateMenuIndices") => {
                self.page.index = arg(0).number().max(0.) as usize;
                self.first = arg(1).number().max(0.) as usize;
                Value::Undefined
            }
            ("Game", "OnMenuHilight") | ("Game", "CAC_OnThumbnailSelect") | ("Game", "OnMenuSelect") => {
                let at = if method == "CAC_OnThumbnailSelect" { 0 } else { 1 };
                self.page.index = arg(at).number().max(0.) as usize;
                // The movie plays no select sound for tiles or option rows; retail's natives do:
                // CAC_OnThumbnailSelect 0x825fd9f0 (every thumbnail page) and OnMenuSelect 0x825fe3c0
                // (page modes 2/3 except the truck/wheel sliders, rows 47/48 = kind "slider").
                let option_row = matches!(self.page.mode, Mode::Style | Mode::Morph)
                    && self.page.rows.get(self.page.index).is_some_and(|r| r.kind != "slider");
                if method == "CAC_OnThumbnailSelect" || method == "OnMenuSelect" && option_row {
                    self.sounds.push(SOUND_A_BUTTON);
                }
                Value::Undefined
            }
            ("Game", "OnMenuGetItemEnabled") => Value::Bool(true),
            ("Game", "CAC_GetTitleText") => Value::Text(self.title()),
            // "#" hides the brand-filter tab: every owned part is shown. (This movie places no mPanel.mFilterTab,
            // so UpdateFilterText draws nothing either way; a grid search shows in the title.)
            ("Game", "CAC_GetFilterText") => Value::Text("#".into()),
            ("Game", "CAC_GetNumItems") => Value::Number(self.page.rows.len() as f64),
            ("Game", "CAC_GetCurrentMenuIndex") => Value::Number(self.page.index as f64),
            ("Game", "CAC_GetCurrentMenuMinWindowIndex") => {
                // First thumbnail row shown, keeping the selection inside the 3x3 (3x5) window.
                let row = self.page.index / 3;
                self.first = self.first.min(row * 3).max((row + 1).saturating_sub(self.page.mode.window_rows()) * 3);
                Value::Number(self.first as f64)
            }
            ("Game", "CAC_GetItemText") => text(self.row(&arg(0)), |r| &r.text),
            ("Game", "CAC_GetOptionType") => text(self.row(&arg(0)), |r| r.kind),
            ("Game", "CAC_IsPartNew") => Value::Bool(self.row(&arg(0)).is_some_and(|r| r.new)),
            ("Game", "CAC_GetThumbnailFileName") => text(self.row(&arg(0)), |r| &r.thumbnail),
            // "blurb" with empty text also clears the previous page's description,
            // which the authored "noblurb" path leaves standing.
            ("Game", "CAC_GetInfoPanelType") => Value::Text(
                match self.row(&arg(0)) {
                    Some(r) if !r.panel.is_empty() => r.panel,
                    Some(r) if !r.description.is_empty() => "blurb",
                    _ => "noblurb",
                }
                .into(),
            ),
            ("Game", "CAC_GetDescriptionText") => {
                let id = self.row(&arg(0)).map_or("", |r| &r.description);
                let full = self.movie.text_assets.localize(id);
                if full.contains('^') || full.contains("<B:") { Value::Text(format!("#{}", plain_markup(&full))) } else { Value::Text(id.into()) }
            }
            ("Game", "CAC_GetItemUnlockHALID") => Value::Text(String::new()),
            ("Game", "CAC_GetIntegerValue") => Value::Number(self.row(&arg(0)).map_or(0., |r| r.value)),
            ("Game", "CAC_GetStringValue") => text(self.row(&arg(0)), |r| &r.value_text),
            ("Game", "CAC_GetColours") => {
                let colours = self.row(&arg(0)).map(|r| r.colours.clone()).unwrap_or_default();
                // Flat [r, g, b, ...] components, 0-255 (the screen reads length / 3 swatches).
                array(vm, colours.into_iter().flat_map(|c| [c >> 16, c >> 8 & 255, c & 255]).map(|v| Value::Number(v as f64)))?
            }
            // The engine has no store: the wallet stays hidden (its update frame only).
            ("Game", "CAC_GetReceiptBalance") => Value::Text(String::new()),
            ("Game", "CAC_GetButtonHelpTextA") => {
                Value::Text(if self.page.accept.is_empty() { "#".into() } else { self.page.accept.clone() })
            }
            ("Game", "CAC_GetButtonHelpTextB") => Value::Text({
                // Only UpdateButtonHelp asks for it, just before it adds B and A.
                self.legend_due = true;
                if self.page.back.is_empty() { "#".into() } else { self.page.back.clone() }
            }),
            ("Game", "CAC_GetSponsorLogoFileName") => {
                let index = Value::Number(self.page.index as f64);
                text(self.row(&index), |r| &r.logo)
            }
            ("Game", "ImageRenderer_ReserveNextFreeIndex") => {
                self.slots += 1;
                Value::Number(self.slots as f64)
            }
            ("Game", "ImageRenderer_Load") | ("Game", "ImageRenderer_LoadAssetByName") => {
                // (asset name, slot index): arguments arrive in push order.
                let slot = arg(1).number().to_string();
                let name = arg(0).text();
                match self.thumbnails.get(&name.to_lowercase()) {
                    Some(texture) => self.movie.images.insert(slot, texture.clone()),
                    // Engine artwork (tattoo designs, decals) by file path; decoded by the presenter. A
                    // non-square image is framed in a square, so the tile shows it centred at its shape.
                    None if name.ends_with(".png") => {
                        let (width, height) = image::image_dimensions(&name).unwrap_or((0, 0));
                        let side = width.max(height) as f32;
                        let (w, h) = (width as f32, height as f32);
                        let frame = (width != height).then(|| [(w - side) / 2., (h - side) / 2., (w + side) / 2., (h + side) / 2.]);
                        self.movie.images.insert(slot, Texture { width, height, rgba: name, frame })
                    }
                    None => self.movie.images.remove(&slot),
                };
                Value::Undefined
            }
            ("Game", "ImageRenderer_Unload") | ("Game", "ImageRenderer_FreeImageIndex") => {
                self.movie.images.remove(&arg(0).number().to_string());
                Value::Undefined
            }
            ("Game", _) if method.starts_with("ImageRenderer_") => Value::Undefined,
            ("Game", "SwatchRenderer_ReserveIndex") => {
                self.slots += 1;
                Value::Number(self.slots as f64)
            }
            // (index, red, green, blue), 0-255: swatch_renderer.SetColour(red, green, blue).
            ("Game", "SwatchRenderer_SetColour") => {
                let c = |i: usize| (arg(i).number() / 255.).clamp(0., 1.) as f32;
                self.movie.colours.insert(arg(0).number().to_string(), [c(1), c(2), c(3), 1.]);
                Value::Undefined
            }
            ("Game", "SwatchRenderer_FreeIndex") => {
                self.movie.colours.remove(&arg(0).number().to_string());
                Value::Undefined
            }
            _ => return Err(format!("Unimplemented Create-a-Skater binding {native}.{method}")),
        })
    }
}

pub(crate) struct Runtime {
    pub vm: Vm,
    pub bindings: Bindings,
    screen: usize,
}
impl Runtime {
    pub fn load(json: &serde_json::Value, page: Page) -> Result<Self, String> {
        let mut vm = native_vm(&[])?;
        define_fe_sounds(&json["actions"], &mut vm)?;
        let mut bindings = Bindings {
            movie: Movie::load(json)?,
            shapes: serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?,
            page,
            first: 0,
            thumbnails: serde::Deserialize::deserialize(&json["thumbnails"]).map_err(|e| e.to_string())?,
            slots: 0,
            random: 0x9e3779b9,
            intro_complete: false,
            outro_complete: false,
            sounds: Vec::new(),
            search: None,
            legend_due: false,
            legend: Vec::new(),
        };
        let buttons: BTreeMap<String, Texture> = serde::Deserialize::deserialize(&json["buttons"]).map_err(|e| e.to_string())?;
        bindings.movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        Ok(Self { vm, bindings, screen })
    }
    fn set_state(&mut self, state: f64) -> Result<(), String> {
        self.vm.begin_update();
        self.vm.call_method(self.screen, "SetState", vec![Value::Number(state)], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        self.extend_legend()
    }
    /// SEARCH_LEGEND after the movie's own legend (createskater.UpdateButtonHelp: ClearButtonItems, B, A,
    /// RefreshButtonHelp), through the panel's own AddButtonItem / RefreshButtonHelp.
    fn extend_legend(&mut self) -> Result<(), String> {
        let search: &[(&'static str, &'static str)] = if self.bindings.search.is_some() { &SEARCH_LEGEND } else { &[] };
        let items: Vec<(&'static str, &'static str)> = search.iter().chain(&self.bindings.legend).copied().collect();
        if !std::mem::take(&mut self.bindings.legend_due) || items.is_empty() {
            return Ok(());
        }
        let mut panel = self.bindings.movie.root;
        for name in ["mCreateSkater", "panel"] {
            let Value::Object(next) = self.vm.get(panel, name) else { return Ok(()) };
            panel = next;
        }
        self.vm.begin_update();
        for (button, text) in items {
            self.vm.call_method(panel, "AddButtonItem", vec![Value::Text(button.into()), Value::Text(text.into())], &mut self.bindings)?;
        }
        self.vm.call_method(panel, "RefreshButtonHelp", Vec::new(), &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// Show `page` in its authored sub-screen.
    pub fn show(&mut self, page: Page) -> Result<(), String> {
        self.show_searchable(page, None)
    }
    /// show, for a grid with a search (item 11): `search` is its text, empty for none yet.
    pub fn show_searchable(&mut self, page: Page, search: Option<String>) -> Result<(), String> {
        self.show_with(page, search, Vec::new())
    }
    /// show_searchable, with `legend` after the movie's own (and the search's).
    pub fn show_with(&mut self, page: Page, search: Option<String>, legend: Vec<(&'static str, &'static str)>) -> Result<(), String> {
        self.bindings.search = search;
        self.bindings.legend = legend;
        let mode = page.mode as u8 as f64;
        // Re-entering the same mode must still rebuild it: pass through another,
        // empty, sub-screen first. Button glyphs stay loaded.
        self.bindings.page = Page { rows: Vec::new(), index: 0, ..page.clone() };
        self.bindings.first = 0;
        self.bindings.movie.images.retain(|key, _| key.starts_with("button:"));
        self.bindings.movie.colours.clear();
        self.set_state(if mode == 0. { 2. } else { 0. })?;
        self.bindings.page = page;
        self.set_state(mode)
    }
    /// Redraw one row's value after the owner changed it.
    pub fn refresh_row(&mut self, row: Row) -> Result<(), String> {
        let index = self.bindings.page.index;
        if let Some(slot) = self.bindings.page.rows.get_mut(index) {
            *slot = row;
        }
        self.set_state(REFRESH_ITEM)
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text("outro".into())], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// One authored frame (60 fps timeline).
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        self.extend_legend()?;
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    /// The movie navigates its own lists and grids; values and choices are native.
    /// B and the Style/Morph Left/Right are native: the movie plays no sound for them.
    pub fn input(&mut self, key: Key) -> Result<Option<Outcome>, String> {
        // Only this input's sounds: show()'s sub-screen rebuilds are not the player's doing.
        self.bindings.sounds.clear();
        let before = self.bindings.page.index;
        let mode = self.bindings.page.mode;
        match key {
            // B handler 0x825fc768 runs only for a shown B (legend B1-B3): on a page whose B
            // legend is hidden (edit pages, tattoo categories, graphic location/categories)
            // retail plays nothing and stays.
            Key::Back if matches!(self.bindings.page.back.as_str(), "" | "#") => return Ok(None),
            Key::Back => return Ok(Some(Outcome::Back)),
            Key::Left | Key::Right if matches!(mode, Mode::Style | Mode::Morph) => {
                // OnButton 0x825fc7e8 (buttons 14/15): value rows click core_nav_left/right on every
                // press, clamped or not; morph rows run the cas_slider loop instead (authored silent).
                let kind = self.bindings.page.rows.get(before).map_or("", |r| r.kind);
                if matches!(kind, "slider" | "category" | "selector" | "color") {
                    self.bindings.sounds.push(fe_sound_id(if key == Key::Left { "core_nav_left" } else { "core_nav_right" }));
                }
                return Ok(Some(Outcome::Adjust(before, if key == Key::Left { -1 } else { 1 })));
            }
            // Graphic Adjust's A: 0x825fc7e8 plays core_a_button for pages 7/8/34-36.
            Key::Accept if matches!(mode, Mode::Scale | Mode::Tattoo) => self.bindings.sounds.push(SOUND_A_BUTTON),
            // Graphic Adjust has no rows: its sticks are the owner's.
            Key::Up | Key::Down | Key::Left | Key::Right if matches!(mode, Mode::Scale | Mode::Tattoo) => return Ok(None),
            _ => {}
        }
        self.vm.begin_update();
        let args = vec![Value::Number(key.apt().1), Value::Number(0.)];
        self.vm.call_method(self.screen, "UpdateInput", args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        self.extend_legend()?;
        let index = self.bindings.page.index;
        Ok(if key == Key::Accept {
            // An empty page still answers A (the owner decides what it means).
            (index < self.bindings.page.rows.len() || self.bindings.page.rows.is_empty()).then_some(Outcome::Accept(index))
        } else {
            (index != before).then_some(Outcome::Hilight(index))
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Perf 10-12: what opening Edit Skater costs on the main thread (create_skater::interact loads the movie on the
    /// press), and what the parsed manifest it keeps all session weighs. SKATE3_ASSET_ROOT = the install's assets.
    #[test]
    #[ignore = "requires the installed Create-a-Skater screen"]
    fn open_cost() {
        use std::time::Instant;
        let file = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private/create-skater/runtime/createskater.json");
        let empty = || Page { title: String::new(), mode: Mode::List, rows: Vec::new(), index: 0, accept: String::new(), back: String::new() };
        #[cfg(windows)]
        let heap = || {
            use windows_sys::Win32::System::Memory::{GetProcessHeap, HEAP_SUMMARY, HeapSummary};
            let mut s = HEAP_SUMMARY { cb: size_of::<HEAP_SUMMARY>() as u32, ..Default::default() };
            // SAFETY: a plain query of this process's default heap into a sized struct.
            unsafe { HeapSummary(GetProcessHeap(), 0, &mut s) };
            s.cbAllocated as f64 / 1e6
        };
        #[cfg(not(windows))]
        let heap = || 0.;
        let bytes = std::fs::read(&file).unwrap();
        let base = heap();
        let t = Instant::now();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        eprintln!("createskater.json {:.1} MB: parse {:.1} ms, parsed Value {:.1} MB of heap", bytes.len() as f64 / 1e6, t.elapsed().as_secs_f64() * 1e3, heap() - base);
        drop(Runtime::load(&json, empty()).unwrap());
        let mut total = 0.;
        for _ in 0..3 {
            let t = Instant::now();
            let runtime = Runtime::load(&json, empty()).unwrap();
            total += t.elapsed().as_secs_f64() * 1e3;
            drop(runtime);
        }
        let base = heap();
        let runtime = Runtime::load(&json, empty()).unwrap();
        eprintln!("Runtime::load {:.1} ms; one runtime {:.1} MB of heap", total / 3., heap() - base);
        drop(runtime);
        // Pre-10-12 costs Runtime::load no longer pays: the to_string FE sound scan and from_value's subtree clones.
        let t = Instant::now();
        let mut vm = native_vm(&[]).unwrap();
        define_fe_sounds(&json["actions"], &mut vm).unwrap();
        let walk = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        drop(crate::core_menu_runtime::tests::fe_sound_names_by_text(&json["actions"]));
        let scan = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        for key in ["characters", "actions", "exports", "scoped_exports", "language", "shapes", "thumbnails", "buttons"] {
            drop(json[key].clone());
        }
        for font in json["fonts"].as_object().into_iter().flat_map(|f| f.values()) {
            drop(font["definition"]["glyphs"].clone());
        }
        let clones = t.elapsed().as_secs_f64() * 1e3;
        eprintln!("fe sounds {walk:.1} ms (was {scan:.1}); subtree clones no longer taken {clones:.1} ms; Runtime::load was ~{:.1} ms", total / 3. + scan - walk + clones);
    }

    #[test]
    fn description_markup_reads_as_text() {
        assert_eq!(
            plain_markup("Select a gesture.^^Use <B:DPad_Down_hud> to scare! "),
            "Select a gesture.\n\nUse D-Pad Down to scare!"
        );
    }

    fn page(mode: Mode, n: usize) -> Page {
        Page {
            title: "Hats".into(),
            mode,
            rows: (0..n)
                .map(|i| Row {
                    text: format!("Item {i}"),
                    kind: if mode == Mode::Style { "selector" } else { "none" },
                    value_text: "Value".into(),
                    ..Default::default()
                })
                .collect(),
            index: 0,
            accept: "ID_COMMON_SELECT".into(),
            back: "ID_COMMON_BACK".into(),
        }
    }

    /// Needs the private prepared manifest (CREATE_SKATER_MANIFEST); not in CI.
    #[test]
    #[ignore]
    fn original_create_skater_navigates() {
        let path = std::env::var("CREATE_SKATER_MANIFEST").expect("CREATE_SKATER_MANIFEST");
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut runtime = Runtime::load(&json, page(Mode::List, 4)).unwrap();
        for _ in 0..30 {
            runtime.advance().unwrap();
        }
        assert!(runtime.bindings.intro_complete, "intro reports IntroComplete");
        let root = std::path::Path::new(&path).parent().unwrap().parent().unwrap().to_path_buf();
        let out = std::env::var("CREATE_SKATER_OUT").ok();
        for mode in [Mode::List, Mode::Thumbs, Mode::Style, Mode::Morph] {
            let mut p = page(mode, 12);
            if mode == Mode::Thumbs {
                let keys: Vec<String> = runtime.bindings.thumbnails.keys().filter(|k| k.contains("/hat/")).take(12).cloned().collect();
                for (row, key) in p.rows.iter_mut().zip(keys) {
                    row.thumbnail = key;
                }
            }
            runtime.show(p).unwrap();
            for _ in 0..40 {
                runtime.advance().unwrap();
            }
            if let Some(out) = &out {
                let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(out).join(format!("cas-{mode:?}.png")));
            }
            let down = runtime.input(Key::Down).unwrap();
            assert!(matches!(down, Some(Outcome::Hilight(i)) if i > 0), "{mode:?} moves: {down:?}");
            // The movie's own sounds: the thumbnail grid's moves are blue_box_nav, lists core_nav_*.
            let nav = if mode == Mode::Thumbs { fe_sound_id("blue_box_nav") } else { crate::core_menu_runtime::SOUND_NAV_DOWN };
            assert_eq!(runtime.bindings.sounds, [nav], "{mode:?} Down sound");
            let accepted = runtime.input(Key::Accept).unwrap();
            assert!(matches!(accepted, Some(Outcome::Accept(_))), "{mode:?} accepts");
            // A on a tile or an option row plays the select sound once (movie for lists, native else).
            assert_eq!(runtime.bindings.sounds, [SOUND_A_BUTTON], "{mode:?} A sound");
            if matches!(mode, Mode::Style | Mode::Morph) {
                runtime.bindings.page.rows.iter_mut().for_each(|r| r.kind = "selector");
                runtime.input(Key::Left).unwrap();
                assert_eq!(runtime.bindings.sounds, [fe_sound_id("core_nav_left")], "{mode:?} selector Left");
                runtime.bindings.page.rows.iter_mut().for_each(|r| r.kind = "morph");
                runtime.input(Key::Right).unwrap();
                assert!(runtime.bindings.sounds.is_empty(), "{mode:?} morph Right is the silent slider loop");
                runtime.bindings.page.rows.iter_mut().for_each(|r| r.kind = "slider");
                runtime.input(Key::Accept).unwrap();
                assert!(runtime.bindings.sounds.is_empty(), "{mode:?} truck/wheel slider A is silent");
            }
        }
        let mut adjust = page(Mode::Scale, 0);
        adjust.rows.clear();
        runtime.show(adjust).unwrap();
        runtime.input(Key::Accept).unwrap();
        assert_eq!(runtime.bindings.sounds, [SOUND_A_BUTTON], "Graphic Adjust A sound");
        // B (0x825fc768) answers only a shown B legend; a hidden one ("#") is ignored, silently.
        for (mode, back, answered) in
            [(Mode::Morph, "#", false), (Mode::List, "#", false), (Mode::List, "ID_COMMON_BACK", true), (Mode::Scale, "ID_CAC_CHANGE_GRAPHIC", true)]
        {
            let mut p = page(mode, if mode == Mode::Scale { 0 } else { 3 });
            p.back = back.into();
            runtime.show(p).unwrap();
            assert_eq!(runtime.input(Key::Back).unwrap(), answered.then_some(Outcome::Back), "{mode:?} B legend {back}");
            assert!(runtime.bindings.sounds.is_empty(), "the B sound is the owner's, on Outcome::Back only");
        }
    }

    /// The installed movie (CREATE_SKATER_MANIFEST), its asset root, and a runtime past its intro.
    pub(crate) fn installed() -> Option<(serde_json::Value, std::path::PathBuf, Runtime)> {
        let path = std::env::var("CREATE_SKATER_MANIFEST").ok()?;
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        let root = std::path::Path::new(&path).parent()?.parent()?.to_path_buf();
        let mut runtime = Runtime::load(&json, page(Mode::List, 0)).ok()?;
        for _ in 0..30 {
            runtime.advance().ok()?;
        }
        Some((json, root, runtime))
    }
    /// Render harness: the movie's draws for `page` after `frames` frames, plus `extra` on top, written to `out`.
    pub(crate) fn render_page_to_png(runtime: &mut Runtime, root: &std::path::Path, page: Page, frames: usize, extra: &[crate::apt_scene::Draw], out: &std::path::Path) -> Vec<crate::apt_scene::Draw> {
        runtime.show(page).unwrap();
        for _ in 0..frames {
            runtime.advance().unwrap();
        }
        let mut draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
        draws.extend_from_slice(extra);
        crate::core_menu_runtime::tests::raster_draws(&draws, root, out);
        draws
    }

    /// Stage 0: an option page with a colour, a slider, a selector and an option row renders (CREATE_SKATER_OUT).
    #[test]
    #[ignore]
    fn style_rows_render() {
        let Some((_, root, mut runtime)) = installed() else { panic!("CREATE_SKATER_MANIFEST") };
        let out = std::path::PathBuf::from(std::env::var("CREATE_SKATER_OUT").unwrap_or_else(|_| ".".into()));
        let row = |text: &str, kind: &'static str| Row { text: text.into(), kind, ..Default::default() };
        // Morph: Style mode has no slider component (movie:60012.SetNumBars); Morph's rows take every kind.
        let mode = Mode::Morph;
        let p = Page {
            title: "#Options".into(),
            mode,
            rows: vec![
                Row { colours: vec![0xffffff, 0x000000, 0xdc1e28, 0xff6e14, 0xffc800, 0x1e64dc], value: 2., ..row("#Colour", "color") },
                Row { value: 6., value_text: "#200".into(), ..row("#Hue", "slider") },
                Row { value_text: "#On".into(), ..row("#Group with below", "selector") },
                row("#Mirror Copy", "option"),
            ],
            index: 0,
            accept: "ID_COMMON_SELECT".into(),
            back: "ID_COMMON_BACK".into(),
        };
        let draws = render_page_to_png(&mut runtime, &root, p, 40, &[], &out.join(format!("cas-{mode:?}-rows.png")));
        assert!(!draws.is_empty());
    }

    /// Item 11: a searchable grid's legend is the movie's B and A plus Y Top and X Search, kept across highlight
    /// moves, and its title shows the search; other pages keep the movie's own. CREATE_SKATER_OUT renders it.
    #[test]
    #[ignore]
    fn a_searchable_grid_shows_its_legend_and_filter() {
        let Some((json, root, mut runtime)) = installed() else { panic!("CREATE_SKATER_MANIFEST") };
        let clip = |runtime: &Runtime, path: &[&str]| {
            path.iter().fold(runtime.bindings.movie.root, |o, n| match runtime.vm.get(o, n) {
                Value::Object(next) => next,
                other => panic!("{n}: {other:?}"),
            })
        };
        let items = |runtime: &Runtime| runtime.vm.get(clip(runtime, &["mCreateSkater", "panel", "buttonHelp"]), "mItemCount").number();
        let mut grid = page(Mode::Thumbs, 12);
        let keys: Vec<String> = runtime.bindings.thumbnails.keys().filter(|k| k.contains("/hat/")).take(12).cloned().collect();
        for (row, key) in grid.rows.iter_mut().zip(keys) {
            row.thumbnail = key;
        }
        runtime.show_searchable(grid.clone(), Some("skate 2".into())).unwrap();
        for _ in 0..40 {
            runtime.advance().unwrap();
        }
        assert_eq!(items(&runtime), 4., "B, A, Y, X");
        assert_eq!(runtime.bindings.title(), "#Filter: skate 2");
        runtime.input(Key::Down).unwrap();
        for _ in 0..20 {
            runtime.advance().unwrap();
        }
        assert_eq!(items(&runtime), 4., "a highlight move keeps it");
        if let Ok(out) = std::env::var("CREATE_SKATER_OUT") {
            let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("cas-search.png"));
            // create_skater::animate's on-screen keyboard over it.
            let text = &runtime.bindings.movie.text_assets;
            let font = text.font_named("Futura Std Medium").and_then(|id| text.fonts.get(&id)).expect("the movie's font");
            let entry = crate::text_entry::TextEntry::new("skate 2", 32);
            let mut typing = draws.clone();
            typing.extend(crate::boot::keyboard_panel(font, &entry, &text.localize("ID_COMMON_SEARCH"), &json["buttons"]));
            crate::core_menu_runtime::tests::raster_draws(&typing, &root, &std::path::Path::new(&out).join("cas-search-keyboard.png"));
            runtime.show_searchable(grid.clone(), Some("WWWWWWWWWWWWWWWWWWWW".into())).unwrap();
            assert!(runtime.bindings.title().starts_with("#Filter: ...WWW"), "the end of a long search");
            let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("cas-search-long.png"));
        }
        runtime.show(grid).unwrap();
        assert_eq!(items(&runtime), 2., "a page without a search: the movie's B and A");
        assert_eq!(runtime.bindings.title(), "Hats");
    }

    /// Item 31 (not retail, user 10-11): an edit page with a colour row (B Back shown: A on the row opens the wheel),
    /// then the wheel's own page (the row alone, Y Reset after the movie's legend). CREATE_SKATER_OUT renders both,
    /// the wheel at 16:9 and 21:9, and lists the wheel page's draws.
    #[test]
    #[ignore]
    fn a_colour_wheel_page_shows_its_swatch_its_legend_and_the_wheel() {
        let Some((_, root, mut runtime)) = installed() else { panic!("CREATE_SKATER_MANIFEST") };
        let clip = |runtime: &Runtime, path: &[&str]| {
            path.iter().fold(runtime.bindings.movie.root, |o, n| match runtime.vm.get(o, n) {
                Value::Object(next) => next,
                other => panic!("{n}: {other:?}"),
            })
        };
        let items = |runtime: &Runtime| runtime.vm.get(clip(runtime, &["mCreateSkater", "panel", "buttonHelp"]), "mItemCount").number();
        let row = |text: &str, kind: &'static str| Row { text: text.into(), kind, ..Default::default() };
        let colour = Row { colours: vec![0x3366cc], description: crate::cas_menu::WHEEL_BLURB.into(), ..row("ID_CAC_WEARSTYLES_COLOUR", "color") };
        let p = Page {
            title: "ID_CAC_WEAR_STYLES_TITLE".into(),
            mode: Mode::Morph,
            rows: vec![colour, Row { value_text: "#Option 1".into(), ..row("", "selector") }],
            index: 0,
            accept: "ID_COMMON_ACCEPT".into(),
            back: "ID_COMMON_BACK".into(),
        };
        let settle = |runtime: &mut Runtime| (0..40).for_each(|_| runtime.advance().unwrap());
        runtime.show(p.clone()).unwrap();
        settle(&mut runtime);
        let plain = items(&runtime);
        let out = std::env::var("CREATE_SKATER_OUT").ok().map(std::path::PathBuf::from);
        let draw = |runtime: &Runtime| crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
        if let Some(out) = &out {
            crate::core_menu_runtime::tests::raster_draws(&draw(&runtime), &root, &out.join("cas-wheel-page.png"));
        }
        let page = crate::colour_wheel::page_for(&p, 0, [0.2, 0.4, 0.8]);
        runtime.show_with(page, None, vec![("Y", "ID_CAC_CUSTOMGRAPHIC_RESET")]).unwrap();
        settle(&mut runtime);
        assert_eq!(items(&runtime), plain + 1., "the movie's B and A, then Y Reset");
        runtime.show(p).unwrap();
        settle(&mut runtime);
        assert_eq!(items(&runtime), plain, "the page again without it");
        let Some(out) = out else { return };
        use crate::colour_wheel as wheel;
        runtime.show_with(crate::colour_wheel::page_for(&runtime.bindings.page.clone(), 0, [0.2, 0.4, 0.8]), None, vec![("Y", "ID_CAC_CUSTOMGRAPHIC_RESET")]).unwrap();
        settle(&mut runtime);
        let mut draws = draw(&runtime);
        let listing: Vec<String> = draws.iter().map(|d| format!("{} {:?} {:?}", d.texture, crate::graphic_board::bounds(d), d.multiply)).collect();
        std::fs::write(out.join("cas-wheel-draws.txt"), listing.join("\n")).unwrap();
        let mut parts = crate::customiser_parts::Parts::default();
        parts.library.defaults.insert("male".into(), serde_json::json!({}));
        let c = crate::customiser::Customiser::for_test(&parts, Vec::new(), "male");
        let custom = crate::cas_menu::Custom {
            template: serde_json::json!({"grip": {"rgb": [0, 0, 0], "solid": false}}),
            at: "/grip/rgb".into(),
            start: [0.2, 0.4, 0.8],
            authored: [0.2, 0.4, 0.8],
            default: serde_json::json!({"grip": null}),
            hair: false,
        };
        let mut open = wheel::Open::new(&c, custom);
        open.steer(bevy::math::Vec2::ZERO, 0., 1.);
        draws.retain(|d| !crate::graphic_board::is_rotate_skater(d));
        let mut over = open.draws(&draws);
        for (name, rgba) in [(wheel::DISC_TEXTURE, wheel::disc_rgba()), (wheel::BAR_TEXTURE, wheel::bar_rgba())] {
            let file = out.join(name.replace('/', "-"));
            std::fs::write(&file, rgba).unwrap();
            over.iter_mut().filter(|d| d.texture == name).for_each(|d| d.texture = file.to_string_lossy().into_owned());
        }
        draws.extend(over);
        let wide = out.join("cas-wheel-open-16x9.png");
        crate::core_menu_runtime::tests::raster_draws(&draws, &root, &wide);
        // 21:9 (2560x1080): the canvas fits the stage whole (AutoMin), 200 stage units of the room either side.
        let stage = image::open(&wide).unwrap().into_rgba8();
        let mut ultra = image::RgbaImage::from_pixel(1680, 720, image::Rgba([40, 44, 48, 255]));
        image::imageops::overlay(&mut ultra, &stage, 200, 0);
        ultra.save(out.join("cas-wheel-open-21x9.png")).unwrap();
    }

    /// The nav sound names hash to the ids retail's OnButton (0x825fc7e8) loads for buttons 14/15.
    #[test]
    fn nav_sound_names_match_retail_ids() {
        assert_eq!(fe_sound_id("core_nav_left"), 0xAD8B_F9A0_B499_7B49);
        assert_eq!(fe_sound_id("core_nav_right"), 0xD8EE_38A6_1C45_EA28);
        assert_eq!(fe_sound_id("core_a_button"), SOUND_A_BUTTON);
    }
}
