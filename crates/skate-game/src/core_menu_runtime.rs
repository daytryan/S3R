//! Original Start-button crossbar (main/core_menu) with native bindings that
//! expose this engine's features instead of Skate 3's career/online screens.
//! Bevy-independent: the plugin in core_menu.rs owns rendering and actions.
use crate::{
    apt_movie::Movie,
    apt_scene::Shapes,
    apt_vm::{Host, ObjectKind, Value, Vm},
};

/// Host constants the shipped scripts compare against. Only distinctness matters.
const MODES: [(&str, f64); 6] = [
    ("FECrossBarScreenMode_RefreshMenu", 1.),
    ("FECrossBarScreenMode_RefreshOption", 2.),
    ("FECrossBarScreenMode_InitialiseMenu", 3.),
    ("FECrossBarScreenMode_InitialiseSubOption", 4.),
    ("FECrossBarScreenMode_InitSubOptionOnlineTeam", 5.),
    ("FECrossBarScreenMode_RefreshDLCBanner", 6.),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Back,
}
impl Key {
    pub(crate) fn apt(self) -> (&'static str, f64) {
        match self {
            Key::Up => ("AptUp", 1.),
            Key::Down => ("AptDown", 2.),
            Key::Left => ("AptLeft", 3.),
            Key::Right => ("AptRight", 4.),
            Key::Accept => ("AptNext", 5.),
            Key::Back => ("AptPrevious", 6.),
        }
    }
}
const OPTION_LEVEL: u8 = 1;
const SUBOPTION_LEVEL: u8 = 2;
/// The most choices the original picker lists at once (coremenu.GetMaxSubOptions clamps to it).
pub(crate) const MAX_CHOICES: usize = 8;
/// The rows the movie draws at once (its MAX_SUBCATS, mSubcat0..7). Not retail (user 10-26): a longer category
/// scrolls through them ([`scrolled`]).
pub(crate) const MAX_ROWS: usize = 8;
/// The first drawn row once row `option` is highlighted, from `top`: the window moves only as far as it must.
fn scrolled(top: usize, option: usize) -> usize {
    if option < top {
        option
    } else if option >= top + MAX_ROWS {
        option + 1 - MAX_ROWS
    } else {
        top
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Item<A> {
    pub name: String,
    /// Frame label of the authored option icon (e.g. "map", "settings", "quit").
    pub icon: &'static str,
    pub help: String,
    pub enabled: bool,
    pub action: A,
    /// Non-empty: Accept opens the original picker with these choices.
    pub choices: Vec<String>,
    pub chosen: usize,
    /// Choices shown greyed; the movie's menu_picker skips them (Game.OnMenuGetItemEnabled).
    pub disabled_choices: Vec<usize>,
    /// A slider: its picker shows one line (the value), Left/Right are `Outcome::Slide`, and Accept or Back closes it.
    pub slider: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Category<A> {
    pub name: String,
    /// Frame label of the authored category icon (singleplayer, multiplayer, create, learn, options).
    pub icon: &'static str,
    pub items: Vec<Item<A>>,
}
/// What the player confirmed; the owner decides what it means.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome<A> {
    Activate(A),
    Choose(A, usize),
    /// A slider moved one step (-1 or 1); the owner updates its value line with `show_slider`.
    Slide(A, i32),
    Close,
}

#[derive(Clone)]
pub(crate) struct Bindings<A> {
    pub movie: Movie,
    pub shapes: Shapes,
    pub categories: Vec<Category<A>>,
    pub title: String,
    menu: usize,
    option: usize,
    /// The current category's first drawn row: the movie's row bindings below count from it ([`MAX_ROWS`]).
    top: usize,
    level: u8,
    sub: usize,
    picked: Option<usize>,
    /// The last Up/Down found an enabled row (its sound plays).
    moved: bool,
    random: u32,
    pub intro_complete: bool,
    pub outro_complete: bool,
    /// `coremenu.GetProgressionData`: [milestone state, count, previous, next, total board sales, cash].
    pub progression: [f64; 6],
}
impl<A> Bindings<A> {
    /// The `option`th drawn row of category `menu`.
    fn item(&self, menu: &Value, option: &Value) -> Option<&Item<A>> {
        let menu = menu.number() as usize;
        self.categories.get(menu)?.items.get(self.top_of(menu) + option.number() as usize)
    }
    /// Category `menu`'s first drawn row: only the current one scrolls.
    fn top_of(&self, menu: usize) -> usize {
        if menu == self.menu { self.top } else { 0 }
    }
    fn current(&self) -> Option<&Item<A>> {
        self.categories.get(self.menu)?.items.get(self.option)
    }
    /// 0x8261f0e8: from the current row, the first enabled row going down (`forward`) or up, wrapping, at most two
    /// passes (IsItemEnabled, vtable +24). None enabled: row 0 and false (no sound, no refresh).
    fn seek(&mut self, forward: bool) -> bool {
        let enabled: Vec<bool> = self.categories[self.menu].items.iter().map(|i| i.enabled).collect();
        let (option, found) = seek(&enabled, self.option, forward);
        self.option = option;
        found
    }
}
fn seek(enabled: &[bool], mut option: usize, forward: bool) -> (usize, bool) {
    let count = enabled.len();
    if option >= count {
        option = 0;
    }
    for _ in 0..2 {
        loop {
            if enabled[option] {
                return (option, true);
            }
            if forward && option + 1 < count {
                option += 1;
            } else if !forward && option > 0 {
                option -= 1;
            } else {
                break;
            }
        }
        option = if forward { 0 } else { count - 1 };
    }
    (0, false)
}
pub(crate) fn array(vm: &mut Vm, values: impl IntoIterator<Item = Value>) -> Result<Value, String> {
    let object = vm.object(ObjectKind::Plain);
    let mut n = 0;
    for value in values {
        vm.set(object, n.to_string(), value)?;
        n += 1;
    }
    vm.set(object, "length", Value::Number(n as f64))?;
    Ok(Value::Object(object))
}
pub(crate) fn thousands(value: f64) -> String {
    let digits = (value.abs().round() as u64).to_string();
    let mut out = String::from(if value < 0. { "-" } else { "" });
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
/// A host running one of the original screens on the shared APT runtime.
pub(crate) trait ScreenHost: Host + Sized {
    fn movie(&mut self) -> &mut Movie;
    fn random(&mut self) -> &mut u32;
}
fn native_of(vm: &Vm, object: usize, method: &str) -> String {
    match vm.objects.get(object).map(|o| &o.kind) {
        Some(ObjectKind::Native(name)) => name.clone(),
        Some(ObjectKind::Plain) if matches!(method, "push" | "pop") => "Array".into(),
        _ => "global".into(),
    }
}
pub(crate) fn screen_property_changed(movie: &mut Movie, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
    if key == "text" || key == "autoSize" {
        movie.text_changed(vm, object)?;
    }
    if key == "_x" || key == "_y" {
        movie.scripted.insert(object);
    }
    // And _width/_height follow an assigned _xscale/_yscale: three-slice layouts (hudobjective2's ThreeSliceScale)
    // scale the middle piece, then place it by its new width.
    if (key == "_xscale" || key == "_yscale") && movie.instances.contains_key(&object) {
        let is_text = movie.characters.get(&movie.instances[&object].character).is_some_and(|c| c.text.is_some());
        if let (false, Some(b)) = (is_text, movie.extent(vm, object, 0)) {
            let (size, measure) = if key == "_xscale" { (b[2] - b[0], "_width") } else { (b[3] - b[1], "_height") };
            let percent = vm.get(object, key).number();
            vm.set(object, measure, Value::Number(f64::from(size) * percent / 100.))?;
        }
    }
    // Assigning _width/_height rescales a clip (Flash), keeping the assigned value.
    if (key == "_width" || key == "_height") && movie.instances.contains_key(&object) {
        let is_text = movie.characters.get(&movie.instances[&object].character).is_some_and(|c| c.text.is_some());
        if let (false, Some(b)) = (is_text, movie.extent(vm, object, 0)) {
            let (size, scale) = if key == "_width" { (b[2] - b[0], "_xscale") } else { (b[3] - b[1], "_yscale") };
            if size > 1e-3 {
                let wanted = vm.get(object, key).number();
                vm.set(object, scale, Value::Number(wanted / size as f64 * 100.))?;
            }
        }
    }
    Ok(())
}
/// Flash behaviour every original screen needs: clip methods, attachMovie, Array,
/// registerClass, Math and localisation. `None`: a screen-specific native.
pub(crate) fn common_call<H: ScreenHost>(
    h: &mut H,
    vm: &mut Vm,
    object: usize,
    method: &str,
    args: &[Value],
) -> Result<Option<Value>, String> {
    if h.movie().method(vm, object, method, args)? {
        return Ok(Some(Value::Undefined));
    }
    if method == "attachMovie" && h.movie().instances.contains_key(&object) {
        let [linkage, name, depth] = [0, 1, 2].map(|i| args.get(i).cloned().unwrap_or_default());
        let Some(child) = h.movie().attach(vm, object, &linkage.text(), &name.text(), depth.number() as i32)? else {
            return Ok(Some(Value::Undefined));
        };
        // attachMovie's initObject: properties land on the clip before its constructor.
        if let Some(Value::Object(init)) = args.get(3) {
            let fields: Vec<(String, Value)> = vm.objects[*init].fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (key, value) in fields {
                vm.set(child, key.as_str(), value)?;
                screen_property_changed(h.movie(), vm, child, &key)?;
            }
        }
        // The caller configures the returned clip immediately; construct it first.
        while let Some((clip, class)) = h.movie().constructing.pop_front() {
            if h.movie().instances.contains_key(&clip) {
                vm.construct_on(class, clip, h)?;
            }
        }
        return Ok(Some(Value::Object(child)));
    }
    let native = native_of(vm, object, method);
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let value = match (native.as_str(), method) {
            ("Array", "push") => {
                let length = vm.get(object, "length").number().max(0.) as usize;
                for (i, value) in args.iter().enumerate() {
                    vm.set(object, (length + i).to_string(), value.clone())?;
                }
                let length = (length + args.len()) as f64;
                vm.set(object, "length", Value::Number(length))?;
                Value::Number(length)
            }
            ("Array", "pop") => {
                let length = vm.get(object, "length").number().max(0.) as usize;
                if length == 0 {
                    return Ok(Some(Value::Undefined));
                }
                let value = vm.get(object, &(length - 1).to_string());
                vm.remove(object, &(length - 1).to_string());
                vm.set(object, "length", Value::Number((length - 1) as f64))?;
                value
            }
            ("Object", "registerClass") => {
                if let (Some(&character), Value::Object(class)) = (h.movie().exports.get(&arg(0).text()), arg(1)) {
                    h.movie().classes.insert(character, class);
                }
                Value::Bool(true)
            }
            ("Math", "floor") => Value::Number(arg(0).number().floor()),
            ("Math", "ceil") => Value::Number(arg(0).number().ceil()),
            ("Math", "round") => Value::Number((arg(0).number() + 0.5).floor()),
            ("Math", "abs") => Value::Number(arg(0).number().abs()),
            ("Math", "min") => Value::Number(arg(0).number().min(arg(1).number())),
            ("Math", "max") => Value::Number(arg(0).number().max(arg(1).number())),
            ("Math", "random") => {
                // Presentation-only glow flicker; never affects menu state.
                let r = h.random();
                *r ^= *r << 13;
                *r ^= *r >> 17;
                *r ^= *r << 5;
                Value::Number(*r as f64 / 4294967296.0)
            }
            ("global", "ASSetPropFlags") | ("LetterBox", _) => Value::Undefined,
            (_, "setTextFormat") => {
                if let Value::Object(format) = arg(0) {
                    h.movie().set_text_format(vm, object, format)?;
                }
                Value::Undefined
            }
            (_, "createTextField") if h.movie().instances.contains_key(&object) => {
                let n = |i: usize| arg(i).number() as f32;
                let field = h.movie().create_text_field(vm, object, &arg(0).text(), arg(1).number() as i32, [n(2), n(3), n(4), n(5)])?;
                Value::Object(field)
            }
            (_, "createEmptyMovieClip") if h.movie().instances.contains_key(&object) => {
                Value::Object(h.movie().create_empty_clip(vm, object, &arg(0).text(), arg(1).number() as i32)?)
            }
            (_, "removeTextField") => Value::Undefined,
            ("Audio", _) => Value::Undefined,
            ("FELanguage", "GetLanguage") => Value::Text("english".into()),
            ("FELanguage", "GetLocalizedFormatNumberString") => Value::Text(thousands(arg(0).number())),
            ("FELanguage", "GetLocalizedFormatStringCustom") => {
                let mut text = h.movie().text_assets.localize(&arg(0).text());
                for value in arg(1).text().split(',') {
                    if let Some(at) = text.find('%') {
                        let end = text[at + 1..].find(|c: char| c.is_ascii_alphabetic()).map_or(text.len(), |e| at + 2 + e);
                        text.replace_range(at..end, value);
                    }
                }
                Value::Text(text)
            }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
impl<A> ScreenHost for Bindings<A> {
    fn movie(&mut self) -> &mut Movie {
        &mut self.movie
    }
    fn random(&mut self) -> &mut u32 {
        &mut self.random
    }
}
impl<A> Host for Bindings<A> {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        screen_property_changed(&mut self.movie, vm, object, key)
    }
    fn call(&mut self, vm: &mut Vm, object: usize, method: &str, args: Vec<Value>) -> Result<Value, String> {
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        let native = native_of(vm, object, method);
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
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
            ("Game", "OnMenuHilight") => {
                self.sub = arg(1).number().max(0.) as usize;
                Value::Undefined
            }
            ("Game", "OnMenuSelect") => {
                self.picked = Some(arg(1).number().max(0.) as usize);
                Value::Undefined
            }
            // (menu, choice index), asked by the picker for each entry of the highlighted item.
            ("Game", "OnMenuGetItemEnabled") => {
                Value::Bool(self.current().is_none_or(|i| !i.disabled_choices.contains(&(arg(1).number().max(0.) as usize))))
            }
            ("Game", "GetDLCPackBannerURI") => Value::Text(String::new()),
            ("Game", _) if method.starts_with("ImageRenderer_") => Value::Number(-1.),
            ("coremenu", "GetIsDemo") | ("coremenu", "IsDLCEnabled") => Value::Bool(false),
            ("coremenu", "GetCurrentDLCBannerIndex") => Value::Number(-1.),
            ("coremenu", "GetMaxSubOptions") => Value::Number(
                // SetNumOptions(max - NULL_ITEM): one spare row, as the shipped screen expects.
                self.categories.iter().flat_map(|c| &c.items).map(|i| i.choices.len()).max().unwrap_or(0).clamp(1, MAX_CHOICES) as f64 + 1.,
            ),
            // A negative milestone selects the authored "welcome" panel instead of career board-sales progress.
            ("coremenu", "GetProgressionData") => array(vm, self.progression.map(Value::Number))?,
            ("coremenu", "GetCurrentModeTitle") => Value::Text(self.title.clone()),
            ("coremenu", "GetNumMenus") => Value::Number(self.categories.len() as f64),
            ("coremenu", "GetMenuIconName") => Value::Text(self.categories.get(arg(0).number() as usize).map_or("", |c| c.icon).into()),
            ("coremenu", "GetMenuName") => Value::Text(self.categories.get(arg(0).number() as usize).map_or(String::new(), |c| c.name.clone())),
            ("coremenu", "GetNumOptions") => {
                let menu = arg(0).number() as usize;
                let rows = self.categories.get(menu).map_or(0, |c| c.items.len().saturating_sub(self.top_of(menu)).min(MAX_ROWS));
                Value::Number(rows as f64)
            }
            ("coremenu", "GetOptionName") => Value::Text(self.item(&arg(0), &arg(1)).map_or(String::new(), |i| i.name.clone())),
            ("coremenu", "GetOptionIconName") => Value::Text(self.item(&arg(0), &arg(1)).map_or("", |i| i.icon).into()),
            ("coremenu", "IsItemEnabled") => Value::Bool(self.item(&arg(0), &arg(1)).is_some_and(|i| i.enabled)),
            ("coremenu", "GetNumSubOptions") => Value::Number(self.item(&arg(0), &arg(1)).map_or(0, |i| i.choices.len()) as f64),
            ("coremenu", "GetSubOptionName") => Value::Text(
                self.item(&arg(0), &arg(1)).and_then(|i| i.choices.get(arg(2).number() as usize)).cloned().unwrap_or_default(),
            ),
            ("coremenu", "GetOptionHelperDescription") => Value::Text(self.current().map_or(String::new(), |i| i.help.clone())),
            ("coremenu", "GetCurrentMenuIndex") => Value::Number(self.menu as f64),
            ("coremenu", "GetCurrentOptionIndex") => Value::Number(self.option.saturating_sub(self.top) as f64),
            ("coremenu", "GetCurrentSubOptionIndex") => Value::Number(self.sub as f64),
            ("coremenu", "GetCurrentMenuLevel") => Value::Number(self.level as f64),
            _ => return Err(format!("Unimplemented core menu binding {native}.{method}")),
        })
    }
}

/// VM with the natives every original screen addresses, plus `extra` host objects.
pub(crate) fn native_vm(extra: &[&str]) -> Result<Vm, String> {
    let mut vm = Vm::new();
    for name in ["MovieClip", "Object", "Math", "FELanguage", "ScreenManager", "Game", "Audio", "LetterBox"].iter().chain(extra) {
        let object = vm.object(ObjectKind::Native((*name).into()));
        let prototype = vm.object(ObjectKind::Plain);
        vm.set(object, "prototype", Value::Object(prototype))?;
        vm.set(vm.global, *name, Value::Object(object))?;
    }
    for (name, value) in [Key::Up, Key::Down, Key::Left, Key::Right, Key::Accept, Key::Back].map(Key::apt) {
        vm.set(vm.global, name, Value::Number(value))?;
    }
    Ok(vm)
}
/// The game defines `_global.FE_SOUND_<KEY>` for the movies: each holds its `fe` record name, `<key>`, which the
/// movie hands to `Audio.PlaySound`. Defines every such name found in `source` (a manifest, or its actions).
pub(crate) fn define_fe_sounds(source: &serde_json::Value, vm: &mut Vm) -> Result<(), String> {
    crate::apt_movie::fe_sound_names(source, &mut |name| {
        vm.set(vm.global, name, Value::Text(name["FE_SOUND_".len()..].to_ascii_lowercase()))
    })
}
/// The `fe` key an `Audio.PlaySound(key)` call plays, if `object.method` is one. Hosts check this ahead of
/// [`common_call`], whose `("Audio", _)` arm silences the screens that play their sounds natively.
pub(crate) fn played_sound(vm: &Vm, object: usize, method: &str, args: &[Value]) -> Option<String> {
    match (vm.objects.get(object).map(|o| &o.kind), method, args.first()) {
        (Some(ObjectKind::Native(n)), "PlaySound", Some(Value::Text(key))) if n == "Audio" => Some(key.clone()),
        // Skate 1's spotfound: message 236 -> menu sound 2038, HUD.abk sample 38 (item10-flashes.md D2), exported as
        // spot_found.wav (old_online1.py).
        (Some(ObjectKind::Native(n)), "PlaySpotFound", _) if n == "Audio" => Some("spot_found".into()),
        _ => None,
    }
}
/// The id `GlobalFEPlaySound` takes for an `fe` key: its AttribSys hash (`core_a_button` -> [`SOUND_A_BUTTON`]).
pub(crate) fn fe_sound_id(key: &str) -> u64 {
    skate_data::attrib_hash::hash(key)
}
/// Runs class libraries, builds the root, and starts the intro; returns the
/// screen controller the root's constructor stored in `screen`.
pub(crate) fn boot_screen<H: ScreenHost>(json: &serde_json::Value, vm: &mut Vm, host: &mut H) -> Result<usize, String> {
    boot_movie(json, vm, host)?;
    let root = host.movie().root;
    let Value::Object(screen) = vm.get(root, "screen") else {
        return Err("Original screen controller was not constructed".into());
    };
    // The constructor stops the root; the native screen manager then plays the
    // intro, which reports IntroComplete.
    vm.begin_update();
    vm.call_method(root, "gotoAndPlay", vec![Value::Text("intro".into())], host)?;
    drain(vm, host)?;
    Ok(screen)
}
/// Runs class libraries and builds the root with the native layout globals.
pub(crate) fn boot_movie<H: ScreenHost>(json: &serde_json::Value, vm: &mut Vm, host: &mut H) -> Result<(), String> {
    // Class libraries first: dependency order was fixed when the bundles were merged.
    let order: Vec<u32> = serde::Deserialize::deserialize(&json["init_order"]).map_err(|e| e.to_string())?;
    for offset in order {
        let code = host.movie().actions.get(&offset.to_string()).ok_or("Missing screen init block")?.clone();
        vm.run(&code, host)?;
    }
    vm.begin_update();
    host.movie().initialize(vm)?;
    // Native layout globals, read through _global or _root. The authored layout
    // is already title-safe at 720p, so the overscan offset is zero.
    let root = host.movie().root;
    for object in [vm.global, root] {
        vm.set(object, "Screen_EdgeOffset", Value::Number(0.))?;
        let letterbox = vm.get(vm.global, "LetterBox");
        vm.set(object, "LetterBox", letterbox)?;
    }
    drain(vm, host)
}
/// Registered constructors precede frame actions, as clips exist before scripts address them.
pub(crate) fn drain<H: ScreenHost>(vm: &mut Vm, host: &mut H) -> Result<(), String> {
    for _ in 0..8192 {
        if let Some((clip, class)) = host.movie().constructing.pop_front() {
            if host.movie().instances.contains_key(&clip) {
                vm.construct_on(class, clip, host)?;
            }
        } else if let Some((object, offset)) = host.movie().pending.pop_front() {
            if !host.movie().instances.contains_key(&object) {
                continue;
            }
            let code = host.movie().actions.get(&offset.to_string()).ok_or("Missing screen action block")?.clone();
            vm.run_on(object, &code, host)?;
            // A script that sends its own clip to the frame it runs on doesn't run again: Flash doesn't re-enter the
            // current frame (throwdownresults' constructor, on 'intro', plays 'intro').
            host.movie().pending.retain(|p| *p != (object, offset));
        } else {
            return Ok(());
        }
    }
    Err("Screen frame action limit".into())
}
/// The placeholder page a [`Booted`] Start menu loads with. Booting sizes the picker's list once
/// (`coremenu.GetMaxSubOptions`), so the placeholder carries as many choices as any page may: a copy then lists
/// every choice, like a fresh load of a real page.
pub(crate) fn boot_probe<A>(action: A) -> Vec<Category<A>> {
    let item = Item { name: String::new(), icon: "settings", help: String::new(), enabled: true, action, choices: Vec::new(), chosen: 0, disabled_choices: Vec::new(), slider: false };
    let item = Item { choices: vec![String::new(); MAX_CHOICES], ..item };
    vec![Category { name: String::new(), icon: "options", items: vec![item] }]
}

/// A screen's runtime as a fresh load leaves it (no frame run), so opening the screen takes a copy instead of
/// parsing and booting the movie again (measured: Start menu 35 ms, Challenge Map 77 ms, online map 83 ms a load,
/// 7-18 ms a copy). The next copy is made on a worker thread as one is taken, so an open only moves it in.
pub(crate) struct Booted<T> {
    booted: std::sync::Arc<T>,
    spare: Option<std::thread::JoinHandle<T>>,
}
impl<T: Clone + Send + Sync + 'static> Booted<T> {
    pub fn new(booted: T) -> Self {
        let mut ready = Self { booted: std::sync::Arc::new(booted), spare: None };
        ready.prepare();
        ready
    }
    fn prepare(&mut self) {
        let booted = self.booted.clone();
        self.spare = std::thread::Builder::new().name("screen-copy".into()).spawn(move || (*booted).clone()).ok();
    }
    /// A copy of the fresh load; waits only while the worker is still making it.
    pub fn take(&mut self) -> T {
        let copy = self.spare.take().and_then(|spare| spare.join().ok()).unwrap_or_else(|| (*self.booted).clone());
        self.prepare();
        copy
    }
}

/// GetProgressionData with no career: the welcome panel.
pub(crate) const NO_PROGRESSION: [f64; 6] = [-1., 0., 0., 0., 0., 0.];

#[derive(Clone)]
pub(crate) struct Runtime<A> {
    pub vm: Vm,
    pub bindings: Bindings<A>,
    screen: usize,
    /// Front-end sound ids (`GlobalFEPlaySound`) the inputs since the last drain played.
    pub sounds: Vec<u64>,
}

/// `crossbar_in` / `crossbar_out`: the crossbar screen's intro and outro (`sub_8261B1F0`,
/// `sub_8261B000`).
pub(crate) const SOUND_CROSSBAR_IN: u64 = 0xE3C3_762D_6D1D_0153;
/// `pause_menu` (sk8_menu 0xDE, Volume 1.05): gameplay's Start handler (825EB998) plays it as the press opens the
/// crossbar (before it pushes FE state 56, whose screen intro plays `crossbar_in`).
pub(crate) const SOUND_PAUSE_MENU: u64 = 0x2DFE_3758_6937_4FA0;
pub(crate) const SOUND_CROSSBAR_OUT: u64 = 0xEFF5_C21D_83AD_974B;
const SOUND_CROSSBAR_UP: u64 = 0x0209_9DD4_C4CC_9E59;
const SOUND_CROSSBAR_DOWN: u64 = 0x7BB8_068A_E28B_26FC;
const SOUND_CROSSBAR_LEFT: u64 = 0x1BB5_C6BF_5DF2_4641;
const SOUND_CROSSBAR_RIGHT: u64 = 0x5E03_48C8_4D82_83E0;
pub(crate) const SOUND_A_BUTTON: u64 = 0x373D_38D7_EA16_7A71;
pub(crate) const SOUND_B_BUTTON: u64 = 0x4D92_7CD6_4CC8_B818;
pub(crate) const SOUND_NAV_UP: u64 = 0x73D7_CC18_FE21_DBE9;
pub(crate) const SOUND_NAV_DOWN: u64 = 0x7F97_5D2B_6425_9AC2;

/// The crossbar's input sounds, as its native handler `sub_8261B560` plays them (the names are
/// the `fe` collections the ids hash): Left/Right always `crossbar_left`/`_right`; Up/Down
/// `crossbar_up`/`_down` when the highlight moves; A on an enabled item `core_a_button`; B
/// `core_b_button`. In a sub-option picker the movie's menu_picker plays
/// `FE_SOUND_CORE_NAV_UP`/`_DOWN` when its highlight moves and `FE_SOUND_CORE_A_BUTTON` on
/// select; B is still the native `core_b_button`. `before` is (level, menu, option, sub,
/// enabled), `after` (whether Up/Down found an enabled row, sub). 0x8261b868/b90c play Up/Down whenever
/// 0x8261f0e8 finds an enabled row, even the same one in a one-row category.
fn sound_for(key: Key, before: (u8, usize, usize, usize, bool), after: (bool, usize)) -> Option<u64> {
    let (level, _, _, sub, enabled) = before;
    if level == SUBOPTION_LEVEL {
        return match key {
            Key::Up if after.1 != sub => Some(SOUND_NAV_UP),
            Key::Down if after.1 != sub => Some(SOUND_NAV_DOWN),
            Key::Accept => Some(SOUND_A_BUTTON),
            Key::Back => Some(SOUND_B_BUTTON),
            _ => None,
        };
    }
    match key {
        Key::Left => Some(SOUND_CROSSBAR_LEFT),
        Key::Right => Some(SOUND_CROSSBAR_RIGHT),
        Key::Up if after.0 => Some(SOUND_CROSSBAR_UP),
        Key::Down if after.0 => Some(SOUND_CROSSBAR_DOWN),
        Key::Accept if enabled => Some(SOUND_A_BUTTON),
        Key::Back => Some(SOUND_B_BUTTON),
        _ => None,
    }
}
fn check<A>(categories: &[Category<A>]) -> Result<(), String> {
    if categories.is_empty() || categories.iter().any(|c| c.items.is_empty()) {
        return Err("Core menu needs non-empty categories".into());
    }
    Ok(())
}
impl<A: Clone> Runtime<A> {
    pub fn load(json: &serde_json::Value, categories: Vec<Category<A>>, title: String) -> Result<Self, String> {
        check(&categories)?;
        let mut vm = native_vm(&["coremenu"])?;
        for (name, value) in MODES {
            vm.set(vm.global, name, Value::Number(value))?;
        }
        let shapes: Shapes = serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?;
        let mut bindings = Bindings {
            movie: Movie::load(json)?,
            shapes,
            categories,
            title,
            menu: 0,
            option: 0,
            top: 0,
            level: OPTION_LEVEL,
            sub: 0,
            picked: None,
            moved: false,
            random: 0x9e3779b9,
            intro_complete: false,
            outro_complete: false,
            progression: NO_PROGRESSION,
        };
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        Ok(Self { vm, bindings, screen, sounds: Vec::new() })
    }
    /// This copy of a fresh load (see [`Booted`]) as `load` would give it for `categories` and `title`: booting reads
    /// neither (test `a_booted_copy_draws_as_a_fresh_load`), so only they change.
    pub fn with(mut self, categories: Vec<Category<A>>, title: String) -> Result<Self, String> {
        check(&categories)?;
        self.bindings.categories = categories;
        self.bindings.title = title;
        Ok(self)
    }
    fn drain(&mut self) -> Result<(), String> {
        drain(&mut self.vm, &mut self.bindings)
    }
    fn set_state(&mut self, mode: &str) -> Result<(), String> {
        self.vm.begin_update();
        let value = self.vm.get(self.vm.global, mode);
        self.vm.call_method(self.screen, "SetState", vec![value], &mut self.bindings)?;
        self.drain()
    }
    fn forward(&mut self, key: Key) -> Result<(), String> {
        self.vm.begin_update();
        let args = vec![Value::Number(key.apt().1), Value::Number(0.)];
        self.vm.call_method(self.screen, "UpdateInput", args, &mut self.bindings)?;
        self.drain()
    }
    /// Populate the crossbar once the intro has run (native InitialiseMenu).
    pub fn show(&mut self) -> Result<(), String> {
        self.set_state("FECrossBarScreenMode_InitialiseMenu")
    }
    /// Highlight `menu`/`option` instead of the first item; call before `show`.
    /// SetMode (0x8261bb10) then moves down to the first enabled row.
    pub fn focus(&mut self, menu: usize, option: usize) {
        let b = &mut self.bindings;
        b.menu = menu.min(b.categories.len() - 1);
        b.option = option.min(b.categories[b.menu].items.len() - 1);
        b.seek(true);
        b.top = scrolled(0, b.option);
    }
    /// The highlighted (menu, row).
    pub fn position(&self) -> (usize, usize) {
        (self.bindings.menu, self.bindings.option)
    }
    pub fn current_mut(&mut self) -> Option<&mut Item<A>> {
        let b = &mut self.bindings;
        b.categories.get_mut(b.menu)?.items.get_mut(b.option)
    }
    /// Redraw the category's rows after an option's name or help changed
    /// (RefreshOption redraws only the helper line).
    pub fn refresh(&mut self) -> Result<(), String> {
        self.set_state("FECrossBarScreenMode_RefreshMenu")
    }
    /// After a scroll, each drawn row shows the item now under it: UpdateColumn's row loop (c7c0) without its title
    /// bounce. The highlighted row is left to SetIndex (RefreshOption), which lights it.
    fn redraw_rows(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let column = ["mScreen", "mColumnAnim", "mColumn"].into_iter().fold(self.vm.get(self.vm.global, "core_menu"), |at, key| match at {
            Value::Object(at) => self.vm.get(at, key),
            _ => Value::Undefined,
        });
        let Value::Object(column) = column else { return Err("Start menu: no row column to scroll".into()) };
        let b = &self.bindings;
        let lit = b.option.saturating_sub(b.top);
        let rows: Vec<_> = (b.top..b.top + MAX_ROWS)
            .map(|i| b.categories[b.menu].items.get(i).map(|item| (item.name.clone(), item.icon, item.enabled)))
            .collect();
        for (slot, row) in rows.into_iter().enumerate() {
            let Value::Object(clip) = self.vm.get(column, &format!("mSubcat{slot}")) else { continue };
            let Some((name, icon, enabled)) = row else {
                self.vm.script_set(&mut self.bindings, clip, "_visible", Value::Bool(false))?;
                continue;
            };
            self.vm.script_set(&mut self.bindings, clip, "_visible", Value::Bool(true))?;
            let text = self.vm.get(clip, "mText");
            if slot != lit {
                let (goto, frame, state) =
                    if enabled { ("gotoAndStop", "stopunselected", "unselected") } else { ("gotoAndPlay", "disabled", "disabled") };
                self.vm.call_method(clip, goto, vec![Value::Text(frame.into())], &mut self.bindings)?;
                if let Value::Object(text) = text {
                    self.vm.call_method(text, "SetState", vec![Value::Text(state.into())], &mut self.bindings)?;
                }
            }
            if let Value::Object(text) = text {
                self.vm.call_method(text, "SetTitle", vec![Value::Text(name)], &mut self.bindings)?;
            }
            if let Value::Object(icons) = self.vm.get(clip, "mIcon") {
                self.vm.call_method(icons, "gotoAndPlay", vec![Value::Text(icon.into())], &mut self.bindings)?;
            }
        }
        self.drain()
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text("outro".into())], &mut self.bindings)?;
        self.drain()
    }
    #[cfg(test)]
    fn level(&self) -> u8 {
        self.bindings.level
    }
    /// One authored frame (60 fps timeline).
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        self.drain()?;
        let roots: Vec<usize> = self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    /// Native navigation state changes first; then, as in the shipped game, every
    /// press is also forwarded to the screen's UpdateInput (its SKIP_BUTTONPRESS
    /// logic expects to see the Accept that opened a picker).
    /// One key, and the front-end sound retail plays for it ([`sound_for`]).
    pub fn input(&mut self, key: Key) -> Result<Option<Outcome<A>>, String> {
        let b = &self.bindings;
        let before = (b.level, b.menu, b.option, b.sub, b.current().is_some_and(|i| i.enabled));
        self.bindings.moved = false;
        let outcome = self.navigate(key)?;
        if !matches!(outcome, Some(Outcome::Choose(..))) {
            self.forward(key)?;
        }
        let b = &self.bindings;
        self.sounds.extend(sound_for(key, before, (b.moved, b.sub)));
        // A slider step ticks like the crossbar's own Left/Right.
        if let Some(Outcome::Slide(_, step)) = &outcome {
            self.sounds.push(if *step < 0 { SOUND_CROSSBAR_LEFT } else { SOUND_CROSSBAR_RIGHT });
        }
        Ok(outcome)
    }
    /// A slider's picker shows `line` (its one choice) in place.
    pub fn show_slider(&mut self, line: String) -> Result<(), String> {
        if let Some(item) = self.current_mut() {
            item.choices = vec![line];
        }
        self.set_state("FECrossBarScreenMode_InitialiseSubOption")
    }
    fn navigate(&mut self, key: Key) -> Result<Option<Outcome<A>>, String> {
        let b = &mut self.bindings;
        if b.level == SUBOPTION_LEVEL {
            let slider = b.current().filter(|i| i.slider).map(|i| i.action.clone());
            return match (key, slider) {
                (Key::Left | Key::Right, Some(action)) => Ok(Some(Outcome::Slide(action, if key == Key::Left { -1 } else { 1 }))),
                // The value is live, so Back keeps it as Accept does.
                (Key::Back, Some(action)) => {
                    self.bindings.level = OPTION_LEVEL;
                    self.forward(Key::Back)?;
                    Ok(Some(Outcome::Choose(action, 0)))
                }
                (Key::Up | Key::Down | Key::Left | Key::Right, _) => Ok(None),
                (Key::Accept, _) => {
                    self.bindings.picked = None;
                    self.forward(Key::Accept)?;
                    let choice = self.bindings.picked.take().unwrap_or(self.bindings.sub);
                    let Some(item) = self.bindings.categories.get_mut(self.bindings.menu).and_then(|c| c.items.get_mut(self.bindings.option)) else {
                        return Ok(None);
                    };
                    item.chosen = choice;
                    let action = item.action.clone();
                    self.bindings.level = OPTION_LEVEL;
                    self.forward(Key::Back)?;
                    Ok(Some(Outcome::Choose(action, choice)))
                }
                (Key::Back, None) => {
                    // The movie hides the picker when it sees AptPrevious at option level.
                    self.bindings.level = OPTION_LEVEL;
                    Ok(None)
                }
            };
        }
        match key {
            Key::Left | Key::Right => {
                let count = b.categories.len();
                b.menu = if key == Key::Left { (b.menu + count - 1) % count } else { (b.menu + 1) % count };
                // 0x8261d9f8: row 0, then the first enabled row.
                b.option = 0;
                b.seek(true);
                b.top = scrolled(0, b.option);
                self.set_state("FECrossBarScreenMode_RefreshMenu")?;
                Ok(None)
            }
            Key::Up | Key::Down => {
                let count = b.categories[b.menu].items.len();
                b.option = if key == Key::Up { (b.option + count - 1) % count } else { (b.option + 1) % count };
                b.moved = b.seek(key == Key::Down);
                if b.moved {
                    let top = scrolled(b.top, b.option);
                    if top != b.top {
                        b.top = top;
                        self.redraw_rows()?;
                    }
                    self.set_state("FECrossBarScreenMode_RefreshOption")?;
                }
                Ok(None)
            }
            Key::Accept => {
                let Some(item) = b.current() else { return Ok(None) };
                if !item.enabled {
                    return Ok(None);
                }
                if item.choices.is_empty() {
                    return Ok(Some(Outcome::Activate(item.action.clone())));
                }
                b.sub = item.chosen.min(item.choices.len() - 1);
                b.level = SUBOPTION_LEVEL;
                self.set_state("FECrossBarScreenMode_InitialiseSubOption")?;
                Ok(None)
            }
            Key::Back => Ok(Some(Outcome::Close)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn movie_sound_keys_hash_to_the_retail_ids() {
        // _global.FE_SOUND_CORE_A_BUTTON = "core_a_button" -> the id the crossbar plays natively.
        for (key, id) in
            [("core_a_button", SOUND_A_BUTTON), ("core_b_button", SOUND_B_BUTTON), ("core_nav_up", SOUND_NAV_UP), ("core_nav_down", SOUND_NAV_DOWN)]
        {
            assert_eq!(fe_sound_id(key), id, "{key}");
        }
        let mut vm = native_vm(&[]).unwrap();
        define_fe_sounds(&serde_json::json!([{ "value": "FE_SOUND_BLUE_BOX_NAV" }]), &mut vm).unwrap();
        assert_eq!(vm.get(vm.global, "FE_SOUND_BLUE_BOX_NAV"), Value::Text("blue_box_nav".into()));
    }

    /// Pre-10-12 define_fe_sounds: scanned `source.to_string()`. Kept as the oracle and timing reference for the
    /// tree walk in apt_movie::fe_sound_names.
    pub(crate) fn fe_sound_names_by_text(source: &serde_json::Value) -> Vec<String> {
        let text = source.to_string();
        text.match_indices("FE_SOUND_")
            .map(|(at, _)| text[at..].chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect())
            .collect()
    }
    pub(crate) fn fe_sound_names_by_walk(source: &serde_json::Value) -> Vec<String> {
        let mut names = Vec::new();
        crate::apt_movie::fe_sound_names(source, &mut |name| Ok(names.push(name.to_owned()))).unwrap();
        names
    }

    /// The tree walk finds the same names, in the same order, as the text scan: synthetic edge cases and every
    /// installed screen that names a sound.
    #[test]
    fn fe_sound_walk_matches_the_text_scan() {
        let tricky = serde_json::json!({
            "FE_SOUND_KEY_A": ["FE_SOUND_X1", "pre FE_SOUND_Y_2z", { "b": "FE_SOUND_FE_SOUND_Q", "a": "\"FE_SOUND_Q\"\n\tFE_SOUND_Rx" }],
            "z": "FE_SOUND_", "m": [1, true, null, 2.5, "FE_SOUND_\u{e9}", "FE_SOUND_\u{1}A", "FE_SOUND_AB\\FE_SOUND_C"],
            "esc\\FE_SOUND_K\"": "FE_SOUND_LAST", "": { "FE_SOUND_": "" }
        });
        let names = fe_sound_names_by_walk(&tricky);
        assert_eq!(names, fe_sound_names_by_text(&tricky));
        assert_eq!(names.len(), 15);
        let Some(root) = std::env::var_os("SKATE3_ASSET_ROOT") else { return };
        let mut stack = vec![std::path::PathBuf::from(root).join("private")];
        let (mut files, mut found) = (0, 0);
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "json") && path.parent().is_some_and(|p| p.ends_with("runtime")) {
                    let bytes = std::fs::read(&path).unwrap();
                    if !bytes.windows(9).any(|w| w == b"FE_SOUND_") {
                        continue;
                    }
                    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    let names = fe_sound_names_by_walk(&json);
                    assert_eq!(names, fe_sound_names_by_text(&json), "{}", path.display());
                    files += 1;
                    found += names.len();
                }
            }
        }
        assert!(files >= 30 && found > files, "{files} files, {found} names");
    }

    #[test]
    fn crossbar_keys_play_the_retail_front_end_sounds() {
        let at = |level| (level, 0usize, 1usize, 2usize, true);
        // Option level: left/right always, up/down when 0x8261f0e8 finds an enabled row.
        assert_eq!(sound_for(Key::Left, at(OPTION_LEVEL), (false, 2)), Some(SOUND_CROSSBAR_LEFT));
        assert_eq!(sound_for(Key::Right, at(OPTION_LEVEL), (false, 2)), Some(SOUND_CROSSBAR_RIGHT));
        assert_eq!(sound_for(Key::Down, at(OPTION_LEVEL), (true, 2)), Some(SOUND_CROSSBAR_DOWN));
        assert_eq!(sound_for(Key::Up, at(OPTION_LEVEL), (false, 2)), None, "no enabled row");
        assert_eq!(sound_for(Key::Accept, at(OPTION_LEVEL), (false, 2)), Some(SOUND_A_BUTTON));
        assert_eq!(sound_for(Key::Accept, (OPTION_LEVEL, 0, 1, 2, false), (false, 2)), None, "disabled item");
        assert_eq!(sound_for(Key::Back, at(OPTION_LEVEL), (false, 2)), Some(SOUND_B_BUTTON));
        // Picker: the movie's nav sounds.
        assert_eq!(sound_for(Key::Up, at(SUBOPTION_LEVEL), (false, 1)), Some(SOUND_NAV_UP));
        assert_eq!(sound_for(Key::Down, at(SUBOPTION_LEVEL), (false, 2)), None);
        assert_eq!(sound_for(Key::Left, at(SUBOPTION_LEVEL), (false, 2)), None);
        assert_eq!(sound_for(Key::Back, at(SUBOPTION_LEVEL), (false, 2)), Some(SOUND_B_BUTTON));
    }

    /// The ids gameplay's Start / Back handler (825EB998) builds are the vault's `pause_menu` / `editor_select`.
    #[test]
    fn start_and_back_sounds_are_the_vaults_records() {
        assert_eq!(SOUND_PAUSE_MENU, fe_sound_id("pause_menu"));
        assert_eq!(0x3940_15B6_8783_BE9E, fe_sound_id("editor_select"));
    }

    /// Not retail (user 10-26): a category past the movie's 8 rows scrolls only as far as the highlight needs, a row at
    /// a time; a wrap takes the window with it.
    #[test]
    fn a_long_category_scrolls_a_row_at_a_time() {
        assert_eq!(scrolled(0, 7), 0, "the eighth row is drawn");
        assert_eq!(scrolled(0, 8), 1, "Down past the last drawn row");
        assert_eq!(scrolled(1, 9), 2);
        assert_eq!(scrolled(2, 4), 2, "inside the window: no scroll");
        assert_eq!(scrolled(2, 1), 1, "Up past the first drawn row");
        assert_eq!(scrolled(2, 0), 0, "a wrap back to the first row");
        assert_eq!(scrolled(0, 9), 2, "a wrap to the last of ten rows");
    }

    /// 0x8261f0e8: skip greyed rows, wrap, two passes; none enabled -> row 0, false.
    #[test]
    fn cursor_skips_greyed_rows_like_retail() {
        let rows = [true, false, false, true, false];
        // Down from row 0 lands on 1 (greyed), moves on to 3; Down from 3 lands on 4, wraps to 0.
        assert_eq!(seek(&rows, 1, true), (3, true));
        assert_eq!(seek(&rows, 4, true), (0, true));
        // Up from 3 lands on 2, moves up to 0; Up from 0 wraps to 4, moves up to 3.
        assert_eq!(seek(&rows, 2, false), (0, true));
        assert_eq!(seek(&rows, 4, false), (3, true));
        // Left/Right: row 0 greyed -> the first enabled row.
        assert_eq!(seek(&[false, false, true], 0, true), (2, true));
        assert_eq!(seek(&[false, false], 1, false), (0, false));
        assert_eq!(seek(&[false, false], 0, true), (0, false));
        assert_eq!(seek(&[true], 0, true), (0, true), "a one-row category still 'moves' (sound plays)");
    }

    fn categories() -> Vec<Category<u32>> {
        let item = |name: &str, icon: &'static str, action: u32, choices: &[&str]| Item {
            name: name.into(),
            icon,
            help: format!("{name} help text"),
            enabled: true,
            action,
            choices: choices.iter().map(|s| s.to_string()).collect(),
            chosen: 0,
            disabled_choices: Vec::new(),
            slider: false,
        };
        vec![
            Category { name: "Skate".into(), icon: "singleplayer", items: vec![
                item("Resume", "freeskate", 0, &[]),
                item("Teleport", "map", 1, &["Plaza", "Stadium", "Gap"]),
                item("Quit game", "quit", 2, &[]),
            ] },
            Category { name: "Options".into(), icon: "options", items: vec![item("Difficulty", "settings", 3, &["Easy", "Normal", "Hard"])] },
        ]
    }
    #[test]
    fn thousands_grouping() {
        assert_eq!(thousands(1234567.), "1,234,567");
        assert_eq!(thousands(-12.), "-12");
        assert_eq!(thousands(0.), "0");
    }

    /// Software raster matching hud_render.wgsl (clamp(tex*mul+add)) with straight-alpha
    /// blending over a mid-grey backdrop. Nearest sampling is enough for inspection.
    fn raster(runtime: &Runtime<u32>, root: &std::path::Path, out: &std::path::Path) {
        let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes).unwrap();
        raster_draws(&draws, root, out)
    }
    /// Software rasterizer for offline screen checks (no GPU/window).
    pub(crate) fn raster_draws(draws: &[crate::apt_scene::Draw], root: &std::path::Path, out: &std::path::Path) {
        let mut canvas = image::RgbaImage::from_pixel(1280, 720, image::Rgba([70, 80, 90, 255]));
        let mut cache = std::collections::HashMap::<String, Vec<u8>>::new();
        for draw in draws {
            let mut size = draw.size;
            if draw.texture.ends_with(".png") {
                let decoded = image::open(root.join(&draw.texture)).unwrap().into_rgba8();
                size = [decoded.width(), decoded.height()];
                cache.entry(draw.texture.clone()).or_insert_with(|| decoded.into_raw());
            }
            // Our built-in art (buttons, the solid fill) first, as apt_canvas::texture_bytes serves it.
            let tex = cache.entry(draw.texture.clone()).or_insert_with(|| crate::apt_canvas::texture_bytes(root, &draw.texture, size).unwrap().0);
            let [w, h] = size;
            for tri in draw.vertices.chunks_exact(3) {
                let p: Vec<[f32; 2]> = tri.iter().map(|v| v.position).collect();
                let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
                if area.abs() < 1e-6 {
                    continue;
                }
                let (x0, x1) = (p.iter().map(|q| q[0]).fold(f32::MAX, f32::min).max(0.) as u32, p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).min(1279.) as u32);
                let (y0, y1) = (p.iter().map(|q| q[1]).fold(f32::MAX, f32::min).max(0.) as u32, p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).min(719.) as u32);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((p[1][0] - px) * (p[2][1] - py) - (p[2][0] - px) * (p[1][1] - py)) / area;
                        let w1 = ((p[2][0] - px) * (p[0][1] - py) - (p[0][0] - px) * (p[2][1] - py)) / area;
                        let w2 = 1. - w0 - w1;
                        if w0 < 0. || w1 < 0. || w2 < 0. {
                            continue;
                        }
                        let u = w0 * tri[0].uv[0] + w1 * tri[1].uv[0] + w2 * tri[2].uv[0];
                        let v = w0 * tri[0].uv[1] + w1 * tri[1].uv[1] + w2 * tri[2].uv[1];
                        let (tx, ty) = (((u * w as f32) as i64).clamp(0, w as i64 - 1) as usize, ((v * h as f32) as i64).clamp(0, h as i64 - 1) as usize);
                        let t = &tex[(ty * w as usize + tx) * 4..][..4];
                        let c: [f32; 4] = std::array::from_fn(|i| ((t[i] as f32 / 255.) * draw.multiply[i] + draw.add[i]).clamp(0., 1.));
                        let dst = canvas.get_pixel_mut(x, y);
                        for i in 0..3 {
                            dst[i] = (c[i] * 255. * c[3] + dst[i] as f32 * (1. - c[3])) as u8;
                        }
                    }
                }
            }
        }
        canvas.save(out).unwrap();
    }

    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn icon_contact_sheet() {
        const ICONS: [&str; 40] = ["replay", "map", "quitsession", "extras", "soloranked", "teamranked", "contacts", "skatewith", "partyinvite", "parkeditor", "challengeeditor", "onscreenhelp", "trickguide", "settings", "skatepark", "quit", "restart", "freeskate", "myonlineteam", "skatereel", "profile", "eanation", "view", "start", "players", "hostoptions", "skatechallenge", "savepark", "uploadpark", "parktutorial", "mycareerteam", "onlineteam", "solofreesakte", "skateschool", "skatefeed", "leaderboards", "musicplayer", "resumecareer", "teamfreeskate", "switchteam"];
        let root = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let categories: Vec<Category<u32>> = ICONS.chunks(8).map(|chunk| Category {
            name: "Icons".into(),
            icon: "options",
            items: chunk.iter().map(|icon| Item { name: icon.to_string(), icon, help: String::new(), enabled: true, action: 0, choices: vec![], chosen: 0, disabled_choices: vec![], slider: false }).collect(),
        }).collect();
        let mut rt = Runtime::load(&json, categories, "Icons".into()).unwrap();
        for _ in 0..10 { rt.advance().unwrap(); }
        rt.show().unwrap();
        for sheet in 0..5 {
            for _ in 0..40 { rt.advance().unwrap(); }
            raster(&rt, &root, &out.join(format!("icons-{sheet}.png")));
            rt.input(Key::Right).unwrap();
        }
    }

    #[test]
    fn booted_copies_are_always_the_fresh_load() {
        let mut booted = Booted::new(vec![1, 2, 3]);
        for _ in 0..3 {
            let mut copy = booted.take();
            assert_eq!(copy, [1, 2, 3]);
            copy.push(4);
        }
    }
    /// What the screen draws, frame after frame, through the intro, the crossbar and a few presses.
    fn drawn<A: Clone>(mut rt: Runtime<A>) -> Vec<Vec<(String, [u32; 2], Vec<crate::apt_scene::Vertex>, [f32; 4], [f32; 4])>> {
        let mut frames = Vec::new();
        let mut frame = |rt: &mut Runtime<A>| {
            rt.advance().unwrap();
            let draws = crate::apt_scene::draw(&rt.bindings.movie, &rt.vm, &rt.bindings.shapes).unwrap();
            frames.push(draws.into_iter().map(|d| (d.texture, d.size, d.vertices, d.multiply, d.add)).collect());
        };
        for _ in 0..30 {
            frame(&mut rt);
        }
        rt.show().unwrap();
        for key in [None, Some(Key::Down), Some(Key::Right), Some(Key::Accept), Some(Key::Back), Some(Key::Left)] {
            if let Some(key) = key {
                rt.input(key).unwrap();
            }
            for _ in 0..12 {
                frame(&mut rt);
            }
        }
        frames
    }
    /// Opening the Start menu copies a load made at setup for a placeholder page, then names its own pages and title.
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn a_booted_copy_draws_as_a_fresh_load() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        // The game's own placeholder (core_menu.rs): a copy of it once listed only one picker choice (10-11).
        let mut booted = Booted::new(Runtime::load(&json, boot_probe(0), String::new()).unwrap());
        let mut six = categories();
        six.extend((0..4).map(|i| Category { name: format!("Page {i}"), icon: "multiplayer", items: categories()[0].items.clone() }));
        for (pages, title) in [(categories(), "University"), (six, "Game Settings")] {
            let fresh = drawn(Runtime::load(&json, pages.clone(), title.into()).unwrap());
            let copied = drawn(booted.take().with(pages, title.into()).unwrap());
            assert_eq!(fresh.len(), copied.len());
            for (i, (a, b)) in fresh.iter().zip(&copied).enumerate() {
                assert!(a == b, "{title}: frame {i} differs ({} vs {} draws)", a.len(), b.len());
            }
        }
    }
    /// Visual check against the owned, prepared menu (SKATE_CORE_MENU_ROOT = private/core-menu).
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn original_core_menu_navigation_renders() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let started = std::time::Instant::now();
        let mut rt = Runtime::load(&json, categories(), "University".into()).unwrap();
        eprintln!("load {:?}", started.elapsed());
        for _ in 0..30 {
            rt.advance().unwrap();
        }
        assert!(rt.bindings.intro_complete, "intro never reported completion");
        rt.show().unwrap();
        for _ in 0..40 {
            rt.advance().unwrap();
        }
        raster(&rt, &root, &out.join("core-menu-0-open.png"));
        let get = |rt: &Runtime<u32>, o: usize, k: &str| match rt.vm.get(o, k) {
            Value::Object(o) => o,
            v => panic!("{k}: {v:?}"),
        };
        let screen = get(&rt, get(&rt, rt.vm.global, "core_menu"), "mScreen");
        // Scripted positions survive the title's authored bounce (Flash semantics).
        let title = get(&rt, screen, "mTitle");
        let label = get(&rt, get(&rt, title, "mMode"), "textLabel");
        assert_eq!(
            rt.vm.get(get(&rt, title, "mColumn"), "_x").number(),
            rt.vm.get(label, "_width").number() + 23.
        );
        assert_eq!(rt.input(Key::Down).unwrap(), None);
        for _ in 0..20 {
            rt.advance().unwrap();
        }
        raster(&rt, &root, &out.join("core-menu-1-down.png"));
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "Teleport opens its picker");
        assert_eq!(rt.level(), SUBOPTION_LEVEL);
        for _ in 0..20 {
            rt.advance().unwrap();
        }
        raster(&rt, &root, &out.join("core-menu-2-picker.png"));
        let picker = get(&rt, get(&rt, screen, "mColumnAnim"), "mSubColumn");
        assert_eq!(rt.vm.get(picker, "mCurrentValue"), Value::Number(0.));
        rt.input(Key::Down).unwrap();
        // The press that opened the picker was consumed by SKIP_BUTTONPRESS, this one moves it.
        assert_eq!(rt.vm.get(picker, "mCurrentValue"), Value::Number(1.));
        assert_eq!(rt.bindings.sub, 1, "OnMenuHilight keeps native state in sync");
        for _ in 0..10 {
            rt.advance().unwrap();
        }
        assert_eq!(rt.input(Key::Accept).unwrap(), Some(Outcome::Choose(1, 1)));
        assert_eq!(rt.level(), OPTION_LEVEL);
        rt.input(Key::Right).unwrap();
        for _ in 0..30 {
            rt.advance().unwrap();
        }
        raster(&rt, &root, &out.join("core-menu-3-options.png"));
        assert_eq!(rt.input(Key::Back).unwrap(), Some(Outcome::Close));
        rt.close().unwrap();
        for _ in 0..30 {
            rt.advance().unwrap();
        }
        assert!(rt.bindings.outro_complete, "outro never reported completion");
    }

    /// Not retail (user 10-26): ten rows in one category, in the movie. Down past its eighth row scrolls the rows a row
    /// at a time with the highlight on the last one, a wrap takes them back, and a row scrolled into view opens its
    /// own picker.
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn a_category_past_eight_rows_scrolls_in_the_movie() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let mut pages = categories();
        let row = pages[0].items[0].clone();
        pages[0].items = (0..10).map(|i| Item { name: format!("Row {i}"), action: i, ..row.clone() }).collect();
        pages[0].items[9].choices = ["A", "B", "C"].map(String::from).to_vec();
        let mut rt = Runtime::load(&json, pages, "University".into()).unwrap();
        let run = |rt: &mut Runtime<u32>, frames: usize| (0..frames).for_each(|_| rt.advance().unwrap());
        let press = |rt: &mut Runtime<u32>, key: Key, times: usize| {
            for _ in 0..times {
                rt.input(key).unwrap();
                run(rt, 8);
            }
        };
        // The names the movie's eight rows show, and the one it highlights (core_menu.mOptionIndex).
        let drawn = |rt: &Runtime<u32>| {
            let get = |at: Value, key: &str| match at {
                Value::Object(o) => rt.vm.get(o, key),
                _ => Value::Undefined,
            };
            let column = ["mScreen", "mColumnAnim", "mColumn"].into_iter().fold(rt.vm.get(rt.vm.global, "core_menu"), get);
            let names: Vec<String> = (0..MAX_ROWS)
                .map(|i| ["mText", "mTitle", "TEXTLABEL", "text"].into_iter().fold(get(column.clone(), &format!("mSubcat{i}")), get).text())
                .collect();
            (names, rt.vm.get(rt.screen, "mOptionIndex").number() as usize)
        };
        let rows = |first: usize| (first..first + MAX_ROWS).map(|i| format!("Row {i}")).collect::<Vec<_>>();
        run(&mut rt, 30);
        rt.show().unwrap();
        run(&mut rt, 40);
        assert_eq!(drawn(&rt), (rows(0), 0));
        press(&mut rt, Key::Down, 7);
        assert_eq!(drawn(&rt), (rows(0), 7), "the eighth row: no scroll yet");
        press(&mut rt, Key::Down, 1);
        assert_eq!(drawn(&rt), (rows(1), 7), "the ninth row scrolls into the last place");
        raster(&rt, &root, &out.join("scroll-1-row-8.png"));
        press(&mut rt, Key::Down, 1);
        assert_eq!(drawn(&rt), (rows(2), 7));
        press(&mut rt, Key::Down, 1);
        assert_eq!(drawn(&rt), (rows(0), 0), "Down from the last row wraps to the first");
        press(&mut rt, Key::Up, 1);
        assert_eq!(drawn(&rt), (rows(2), 7), "Up from the first row wraps to the last");
        press(&mut rt, Key::Up, 2);
        assert_eq!(drawn(&rt), (rows(2), 5), "inside the window the rows stay");
        press(&mut rt, Key::Down, 2);
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "the tenth row opens its picker");
        run(&mut rt, 20);
        raster(&rt, &root, &out.join("scroll-2-picker.png"));
        press(&mut rt, Key::Down, 1);
        assert_eq!(rt.input(Key::Accept).unwrap(), Some(Outcome::Choose(9, 1)), "its own choices, not the eighth row's");
    }

    /// The movie's menu_picker asks Game.OnMenuGetItemEnabled(menu, option index) per entry and skips greyed
    /// ones; the answer comes from the highlighted item's `disabled_choices`.
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn picker_skips_disabled_choices() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let mut cats = categories();
        cats[0].items[1].disabled_choices = vec![1];
        let mut rt = Runtime::load(&json, cats, "University".into()).unwrap();
        rt.focus(0, 1);
        for _ in 0..30 {
            rt.advance().unwrap();
        }
        rt.show().unwrap();
        for _ in 0..40 {
            rt.advance().unwrap();
        }
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "opens the picker");
        for _ in 0..20 {
            rt.advance().unwrap();
        }
        rt.input(Key::Down).unwrap();
        for _ in 0..10 {
            rt.advance().unwrap();
        }
        assert_eq!(rt.input(Key::Accept).unwrap(), Some(Outcome::Choose(1, 2)), "Down skips the greyed Stadium");
    }
}
