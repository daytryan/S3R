//! Any original Skate 1/2 Flash screen on the shared APT runtime, driven by its caller: the tutorial and challenge
//! screens (hudtutorial, hudobjective2, challenge_signup, subtitle, letterbox, ...) the Skate 1/2 tutorials show.
//! Converted by skate2port/old_screens.py (the old_hud.py manifest format).
//!
//! The movie's own ActionScript runs as shipped. What it asks of the game (`HUDComponents.Objective_GetObjectiveTitle`,
//! `Mission.GetTitle`, ...) is answered from [`Screen::answer`]; every native call it makes is recorded for
//! [`Screen::take_calls`] (`ScreenManager.IntroComplete`, `Game.OnMenuSelect`, ...). An unanswered native returns
//! undefined and is logged once, so a screen never stops on a missing binding.
//!
//! [`Screens`] draws the open screens of each game on its own canvas above that game's HUD, and plays what they ask of
//! `Audio.PlaySound` from `<key>.wav` in the movie's own export's `sfx`, else its layer's ([`sfx_dir`]; skate-data's
//! s2_fe_sounds exports Skate 2's).
use crate::{
    apt_canvas::Canvas,
    apt_movie::Movie,
    apt_scene::{self, Draw, Shapes},
    apt_vm::{Host, ObjectKind, Value, Vm},
    config::Config,
    core_menu_runtime::{Key, ScreenHost, array, common_call, drain, native_vm, screen_property_changed},
    scoring_hud::{HudComposite, HudMaterial},
};
use bevy::{prelude::*, window::PrimaryWindow};
use serde_json::Value as Json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

/// Host objects the Skate 1/2 screens address beyond native_vm's.
const NATIVES: [&str; 23] = [
    "HUDComponents",
    "Mission",
    // Skate 1's pro intro cards (intro_overlay.rs): pro_bioinfo_overlay's NIS.GetProInfo. Missing, the call went to an
    // undefined variable and the card read undefined (playtest 10-15).
    "NIS",
    "Profile",
    "Tricks",
    "SignUpPoster",
    "Popup",
    "FEUnlock",
    "Progression",
    "EventLog",
    "PlayerList",
    "FELeaderboard",
    // Skate 1's hud_online_players.
    "Player",
    // Skate 3's Music Player (music_ui.rs).
    "MusicPlayer",
    // Skate 3's Party Play setup and Press-A card (party_play.rs).
    "FEPartyPlay",
    // Skate 3's Replay Editor (replay/hud.rs) and its export settings (skatereel_tags).
    "FEReplayMenu",
    "Tags",
    // Skate 3's skate.Reel My Footage list (browser; replay/hud.rs).
    "CommonBrowser",
    // Skate 3's Trick Guide, Extras > Credits and Extras > Licenses (retail_ui.rs).
    "FETrickTutorial",
    "FECredits",
    "tos",
    // Skate 3's career reward screens (skate3_career::screens): unlock_milestone, unlockoverlay.
    "FERewardSummary",
    "coremenu",
];
/// A native the game answers as it is called (nucleus_tos's `tos.GetWordRange` clumps): `Object.Method` and the
/// arguments; None falls back to the static answers.
pub(crate) type Native = Box<dyn FnMut(&str, &[Json]) -> Option<Json> + Send + Sync>;
/// Answers every screen starts with: what the constructors read before the caller's own answers matter. The
/// caller's [`Screen::answer`]s (or `load`'s) replace them.
fn defaults() -> HashMap<String, Json> {
    [
        ("HUDComponents.Objective_GetMaxNumObjectives", Json::from(4)),
        ("HUDComponents.Objective_GetNumObjectives", Json::from(0)),
        ("HUDComponents.Objective_IsModifiedPosition", Json::from(false)),
        ("HUDComponents.Objective_GetActivityTypeVisibility", Json::from(false)),
        ("HUDComponents.Objective_GetActivityNameVisibility", Json::from(true)),
        ("HUDComponents.Objective_GetCurrAndReqOPOVisibility", Json::from(false)),
        ("HUDComponents.Objective_GetVariantName", Json::from("#")),
        ("HUDComponents.PlayerStance_IsRegular", Json::from(true)),
        ("HUDComponents.Results_GetNumPlayers", Json::from(0)),
        ("HUDComponents.Results_GetLayoutType", Json::from(0)),
        ("HUDComponents.GetTutorialHalText", Json::from("")),
        ("HUDComponents.GetTutorialGesturesButtons", Json::from(vec![Json::from(0)])),
        ("Tricks.IsStanceTrickGesturesFlipped", Json::from(false)),
        ("Mission.GetNumEndMissionOptions", Json::from(0)),
        ("Mission.GetWinState", Json::from(true)),
        ("SignUpPoster.HasTutorialNIS", Json::from(false)),
        ("Profile.GetTotalCash", Json::from(0)),
        ("Game.GetMovieSubtitleString", Json::from("#")),
        ("Popup.GetNumOptions", Json::from(0)),
        ("Popup.IsPopupCancelable", Json::from(false)),
        // menu_picker skips options the game reports disabled (popup_runtime answers the same).
        ("Game.OnMenuGetItemEnabled", Json::from(true)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}
/// One authored frame, as the front end advances its screens.
const FRAME: f32 = 1. / 60.;

pub(crate) struct Bindings {
    pub movie: Movie,
    random: u32,
    answers: HashMap<String, Json>,
    calls: Vec<(String, Vec<Json>)>,
    pub(crate) unanswered: BTreeSet<String>,
    label: String,
    pub intro_complete: bool,
    pub outro_complete: bool,
    /// `Audio.PlaySound` keys since the last take (the `fe` record names; see [`Screen::from_json`]).
    pub sounds: Vec<String>,
    /// Image slots `Game.ImageRenderer_ReserveNextFreeIndex` handed out.
    slots: u32,
    /// Asked before the static answers.
    pub native: Option<Native>,
    /// A Skate 1 export (`source.game`): its `Audio.Play<Name>()` natives play `sfx/<Name>.wav`.
    skate1: bool,
}
impl ScreenHost for Bindings {
    fn movie(&mut self) -> &mut Movie {
        &mut self.movie
    }
    fn random(&mut self) -> &mut u32 {
        &mut self.random
    }
}

fn to_json(vm: &Vm, value: &Value) -> Json {
    match value {
        Value::Undefined => Json::Null,
        // Whole numbers as integers: answer keys are `Method(0)`, not `Method(0.0)`, and callers read as_u64.
        Value::Number(n) if n.fract() == 0. && n.abs() < 9e15 => Json::from(*n as i64),
        Value::Number(n) => serde_json::Number::from_f64(*n).map_or(Json::Null, Json::Number),
        Value::Bool(b) => Json::Bool(*b),
        Value::Text(t) => Json::String(t.clone()),
        Value::Object(o) => match vm.get(*o, "length") {
            Value::Number(n) => Json::Array((0..n as usize).map(|i| to_json(vm, &vm.get(*o, &i.to_string()))).collect()),
            _ => Json::Null,
        },
    }
}
fn to_value(vm: &mut Vm, value: &Json) -> Result<Value, String> {
    Ok(match value {
        Json::Null => Value::Undefined,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.)),
        Json::String(s) => Value::Text(s.clone()),
        Json::Array(a) => {
            let items = a.iter().map(|v| to_value(vm, v)).collect::<Result<Vec<_>, _>>()?;
            array(vm, items)?
        }
        Json::Object(fields) => {
            let object = vm.object(ObjectKind::Plain);
            for (key, value) in fields {
                let value = to_value(vm, value)?;
                vm.set(object, key, value)?;
            }
            Value::Object(object)
        }
    })
}

/// The answer for `key` called with `args`: `key(a0,a1)`, then `key(a0)`, then `key`.
fn lookup(answers: &HashMap<String, Json>, key: &str, args: &[Json]) -> Option<Json> {
    let arg = |a: &Json| a.as_str().map_or_else(|| a.to_string(), str::to_owned);
    let two = (args.len() >= 2).then(|| format!("{key}({},{})", arg(&args[0]), arg(&args[1])));
    let one = args.first().map(|a| format!("{key}({})", arg(a)));
    [two, one, Some(key.to_owned())].into_iter().flatten().find_map(|k| answers.get(&k)).cloned()
}

/// `{i}` / `{i:fmt}` in a FELanguage format string replaced by `values[i]` (a text value's `^` breaks no line: only the
/// format's own do).
fn fill(mut text: String, values: &[Json]) -> String {
    for (i, value) in values.iter().enumerate() {
        let value = value.as_str().map_or_else(|| value.to_string(), crate::apt_text::unbroken);
        let (open, mut from) = (format!("{{{i}"), 0);
        while let Some(at) = text[from..].find(&open).map(|at| at + from) {
            let Some(end) = text[at..].find('}') else { break };
            text.replace_range(at..at + end + 1, &value);
            from = at + value.len();
        }
    }
    text
}

impl Host for Bindings {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        screen_property_changed(&mut self.movie, vm, object, key)
    }
    fn call(&mut self, vm: &mut Vm, object: usize, method: &str, args: Vec<Value>) -> Result<Value, String> {
        // Ahead of common_call, whose ("Audio", _) arm answers every Audio call for Skate 3's menus.
        if let Some(key) = crate::core_menu_runtime::played_sound(vm, object, method, &args) {
            self.sounds.push(key);
            return Ok(Value::Undefined);
        }
        // Skate 1's Audio.Play<Name>(): each plays one FE message's menu sound (RETAIL: registered in 0x82441860, closure
        // `li r3,<message>; bl 0x82467740`, FEMessageToSoundTrigger -> HUD.abk / PAUSE_MENU.abk), which old_screens1
        // exports as sfx/<Name>.wav. A native without a sound has no wav and stays silent.
        let audio = matches!(vm.objects.get(object).map(|o| &o.kind), Some(ObjectKind::Native(n)) if n == "Audio");
        if self.skate1 && audio && method.starts_with("Play") {
            self.sounds.push(method.to_owned());
            return Ok(Value::Undefined);
        }
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(ObjectKind::Native(name)) => name.clone(),
            _ => "global".into(),
        };
        let key = format!("{native}.{method}");
        let json_args: Vec<Json> = args.iter().map(|a| to_json(vm, a)).collect();
        match key.as_str() {
            "ScreenManager.IntroComplete" => self.intro_complete = true,
            "ScreenManager.OutroComplete" => self.outro_complete = true,
            // ponytail: the line height of a font size, taken as the size itself (challenge_objectives' layout).
            "ScreenManager.CalculateFontHeight" => return Ok(args.first().cloned().unwrap_or_default()),
            // image_renderer on the textures the manifest carries (`images`, by file name; dpo's icons): a loaded
            // slot draws in place of the clip renamed to it.
            "Game.ImageRenderer_ReserveNextFreeIndex" => {
                self.slots += 1;
                return Ok(Value::Number(f64::from(self.slots)));
            }
            // (path, slot) / (slot); the path's file name without extension names the image.
            "Game.ImageRenderer_Load" | "Game.ImageRenderer_Unload" | "Game.ImageRenderer_FreeImageIndex" => {
                let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
                let load = method == "ImageRenderer_Load";
                let slot = arg(usize::from(load)).number().to_string();
                let path = arg(0).text().to_lowercase();
                let file = path.rsplit(['\\', '/']).next().unwrap_or_default().split('.').next().unwrap_or_default();
                match self.movie.images.get(&format!("image:{file}")).filter(|_| load).cloned() {
                    Some(texture) => self.movie.images.insert(slot, texture),
                    None => self.movie.images.remove(&slot),
                };
                return Ok(Value::Undefined);
            }
            _ => {}
        }
        let native = self.native.as_mut().and_then(|native| native(&key, &json_args));
        let answer = native.or_else(|| lookup(&self.answers, &key, &json_args)).or_else(|| {
            // Unanswered (id, count, values...): the id's text with {0}, {1}, ... filled (ID_COMMON_PERCENT '{0:d}%').
            if key != "FELanguage.GetLocalizedFormatString" {
                return None;
            }
            let text = self.movie.text_assets.localize(&args.first()?.text());
            Some(Json::from(fill(text, json_args.get(2..)?)))
        });
        self.calls.push((key.clone(), json_args));
        match answer {
            Some(answer) => to_value(vm, &answer),
            None => {
                if !key.starts_with("ScreenManager.") && self.unanswered.insert(key.clone()) {
                    debug!("{} screen: {key} has no answer; undefined", self.label);
                }
                Ok(Value::Undefined)
            }
        }
    }
}

pub(crate) struct Screen {
    pub vm: Vm,
    pub bindings: Bindings,
    pub(crate) shapes: Shapes,
    /// The `screen` controller the movie's constructor registers, else the root clip.
    controller: usize,
    /// Where the stage sits on the canvas ([`Screen::tilt`]); identity unless the caller turns it.
    placement: [f32; 6],
    /// The sfx folder of the movie's own export ([`movie_sfx`]): its sounds play from there first ([`sfx_file`]).
    sfx: Option<PathBuf>,
    /// The movie's own export root when its textures don't live under its layer's root ([`Screen::own_assets`]).
    assets: Option<PathBuf>,
}

/// Points every texture of `draws` at `root` (an absolute path wins over the canvas's layer root in `Path::join`).
fn rooted(draws: &mut [Draw], root: &Path) {
    for draw in draws.iter_mut().filter(|d| !d.texture.is_empty()) {
        draw.texture = root.join(&draw.texture).to_string_lossy().into_owned();
    }
}

/// `<export>/sfx` for a movie at `<export>/runtime/<movie>.json`: where the pipeline's fe_sounds writes the sounds of the
/// movies it exported (`skate2-online/skate/screens/sfx` for S.K.A.T.E.'s hudabd and hudthrowmes).
fn movie_sfx(file: &Path) -> Option<PathBuf> {
    Some(file.parent()?.parent()?.join("sfx"))
}

/// The sfx folder an `Audio.PlaySound(FE_SOUND_<KEY>)` plays from: the movie's own export's when it has the key, else
/// its layer's. I4 (online audit 10-06): S.K.A.T.E.'s movies draw on the Skate 2 tutorial layer, whose sfx the tutorial
/// stage rewrites without their keys (skate_letter, gesture_copy_*), so they played nothing. Retail has one FE sound
/// bank; these folders are per-stage exports of the same records.
pub(crate) fn sfx_dir(own: Option<&Path>, layer_root: &Path, key: &str) -> PathBuf {
    own.filter(|dir| dir.join(format!("{key}.wav")).is_file()).map_or_else(|| layer_root.join("sfx"), Path::to_path_buf)
}

fn read_json(file: &Path) -> Result<Json, String> {
    serde_json::from_slice(&std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?)
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// perf 10-18: parsed manifests an open takes instead of reading and parsing its 1.6-2.3 MB file on the press frame
/// (30-75 ms): ones [`warm`]ed while the crossbar highlights the row that opens them (the last [`WARM_KEPT`]), and ones
/// [`kept_json`] keeps for good (sk8popup). None while a worker parses it.
#[derive(Default)]
struct Parsed {
    json: HashMap<PathBuf, Option<std::sync::Arc<Json>>>,
    warmed: Vec<PathBuf>,
}
const WARM_KEPT: usize = 3;
fn parsed_lock() -> std::sync::MutexGuard<'static, Parsed> {
    static PARSED: std::sync::LazyLock<std::sync::Mutex<Parsed>> = std::sync::LazyLock::new(Default::default);
    PARSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn parsed(file: &Path) -> Option<std::sync::Arc<Json>> {
    parsed_lock().json.get(file).cloned().flatten()
}
/// Parses `files` on workers, for a [`Screen::load`] soon. ponytail: an open while its worker still parses parses
/// again rather than waiting. SKATE_SCREEN_WARM=0: nothing.
pub(crate) fn warm(files: impl IntoIterator<Item = PathBuf>) {
    if !crate::config::perf_on("SKATE_SCREEN_WARM") {
        return;
    }
    let mut cache = parsed_lock();
    for file in files {
        if cache.json.contains_key(&file) {
            continue;
        }
        cache.json.insert(file.clone(), None);
        let key = file.clone();
        let worker = std::thread::Builder::new().name("screen-warm".into()).spawn(move || {
            let json = read_json(&file).ok().map(std::sync::Arc::new);
            let mut cache = parsed_lock();
            let pending = matches!(cache.json.get(&file), Some(None));
            let evicted: Vec<_> = match json {
                Some(json) if pending => {
                    cache.json.insert(file.clone(), Some(json));
                    cache.warmed.push(file);
                    let over = cache.warmed.len().saturating_sub(WARM_KEPT);
                    let old: Vec<PathBuf> = cache.warmed.drain(..over).collect();
                    old.iter().filter_map(|f| cache.json.remove(f)).collect()
                }
                _ if pending => cache.json.remove(&file).into_iter().collect(),
                _ => Vec::new(),
            };
            // The dropped manifests free here, off the main thread.
            drop(cache);
            drop(evicted);
        });
        if worker.is_err() {
            cache.json.remove(&key);
        }
    }
}
/// `file` parsed once and kept (sk8popup: the replay and Party Play popups boot from it on each ask).
/// SKATE_SCREEN_WARM=0: parsed on each call.
pub(crate) fn kept_json(file: &Path) -> Result<std::sync::Arc<Json>, String> {
    if let Some(json) = parsed(file) {
        return Ok(json);
    }
    let json = std::sync::Arc::new(read_json(file)?);
    if crate::config::perf_on("SKATE_SCREEN_WARM") {
        parsed_lock().json.insert(file.to_path_buf(), Some(json.clone()));
    }
    Ok(json)
}
/// `runtime/<movie>.json` of `game`'s screens: the file [`Layer::open`] loads (to [`warm`] it).
pub(crate) fn movie_file(asset_root: &Path, game: u8, movie: &str) -> PathBuf {
    let i = usize::from(game).clamp(1, GAMES.len()) - 1;
    asset_root.join(GAMES[i].0).join("runtime").join(format!("{movie}.json"))
}

/// Bug 10-25 #4 / #15: the skate3-online HUD and online-movies stages (tools/skate3_online.py screens,
/// prepare_online_movies) never gave their manifests the glyph set prepare_online_ui does (`_glyphs`), so their
/// button_item2s (the results' B Quit, the vote's A / B / X / Y) drew nothing. Their layer's root is private/online-ui,
/// whose runtime/buttons holds the disc set (button_icons serves it by path either way).
fn online_glyphs(format: &Json, buttons: BTreeMap<String, crate::apt_scene::Texture>) -> BTreeMap<String, crate::apt_scene::Texture> {
    if !buttons.is_empty() || !matches!(format.as_str(), Some("skate3-online-hud" | "skate3-online-movie")) {
        return buttons;
    }
    let glyph = |n: &str| crate::apt_scene::Texture { width: 40, height: 30, rgba: format!("runtime/buttons/{n}.rgba"), frame: None };
    crate::button_icons::PS_NAMES.iter().map(|&n| (n.to_owned(), glyph(n))).collect()
}

impl Screen {
    /// Boots `file` (a runtime manifest) with `answers` in place first: constructors read them at once
    /// (hudobjective2's objective count, challenge_objectives' title and description).
    pub fn load(file: &Path, answers: &[(&str, Json)]) -> Result<Self, String> {
        let label = file.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        // perf 10-18: a manifest warmed while the crossbar highlighted its row is already parsed.
        if let Some(json) = parsed(file) {
            let screen = Self::from_json(&json, label, answers).map_err(|e| format!("{}: {e}", file.display()))?;
            return Ok(Self { sfx: movie_sfx(file), ..screen });
        }
        let json = read_json(file)?;
        let screen = Self::from_json(&json, label, answers).map_err(|e| format!("{}: {e}", file.display()));
        // Freeing the parsed manifest is 5-23 ms of small frees (perf 10-12): a worker does it, off the open's frame.
        // ponytail: a thread per open; if one can't start, the closure (and the manifest) drops here as before.
        let _ = std::thread::Builder::new().name("manifest-free".into()).spawn(move || drop(json));
        Ok(Self { sfx: movie_sfx(file), ..screen? })
    }

    fn from_json(json: &Json, label: String, answers: &[(&str, Json)]) -> Result<Self, String> {
        let mut vm = native_vm(&NATIVES)?;
        // Skate 2's helper/screenmanager sets these; hudobjective2 colours an objective red / green / off-white by them.
        if let Value::Object(manager) = vm.get(vm.global, "ScreenManager") {
            for (name, rgb) in [("Red", 0x97220b), ("Green", 0x82be19), ("OffWhite", 0xfffbec), ("White", 0xffffff)] {
                vm.set(manager, &format!("mColour_{name}"), Value::Number(f64::from(rgb)))?;
            }
        }
        // Skate 1's button_item picks its pad art in its constructor, which runs for first-frame clips during this
        // load (skate1_online_hud::platform_globals).
        if json["source"]["game"] == "skate1" {
            crate::skate1_online_hud::platform_globals(&mut vm)?;
        }
        // The letterbox movie defines LetterBox itself (its constructor sets _global.LetterBox = this). The stub
        // would satisfy its `if (_global.letterbox)` guard, as names resolve case-insensitively, and skip the class.
        if label == "letterbox" {
            vm.set(vm.global, "LetterBox", Value::Undefined)?;
        }
        let mut bindings = Bindings {
            movie: Movie::load(json)?,
            random: 0x9e37_79b9,
            answers: defaults().into_iter().chain(answers.iter().map(|(k, v)| ((*k).to_owned(), v.clone()))).collect(),
            calls: Vec::new(),
            unanswered: BTreeSet::new(),
            label,
            intro_complete: false,
            outro_complete: false,
            sounds: Vec::new(),
            slots: 0,
            native: None,
            skate1: json["source"]["game"] == "skate1",
        };
        // Audio.PlaySound hands back each `fe` key, which `render` plays as <screens>/sfx/<key>.wav.
        crate::core_menu_runtime::define_fe_sounds(json, &mut vm)?;
        let shapes = serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?;
        // Controller glyphs button_item2 clips name in `_test` (apt_scene draws them).
        let buttons: BTreeMap<String, crate::apt_scene::Texture> =
            serde::Deserialize::deserialize(&json["buttons"]).unwrap_or_default();
        let buttons = online_glyphs(&json["format"], buttons);
        bindings.movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        let images: BTreeMap<String, crate::apt_scene::Texture> =
            serde::Deserialize::deserialize(&json["images"]).unwrap_or_default();
        bindings.movie.images.extend(images.into_iter().map(|(name, texture)| (format!("image:{name}"), texture)));
        // Class libraries (do_init_action) before the root, as hud_runtime boots the old HUD movies. A manifest
        // merged with its imported controls lists them in dependency order (superclasses first).
        let initial: Vec<u32> = match json.get("init_order") {
            Some(order) => serde::Deserialize::deserialize(order).map_err(|e| e.to_string())?,
            None => (bindings.movie.characters.values())
                .flat_map(|c| &c.frames)
                .flat_map(|f| &f.controls)
                .filter(|c| c.type_name == "do_init_action")
                .map(|c| c.actions_offset)
                .collect(),
        };
        for offset in initial {
            let code = bindings.movie.actions.get(&offset.to_string()).ok_or("missing init block")?.clone();
            vm.run(&code, &mut bindings)?;
        }
        vm.begin_update();
        bindings.movie.initialize(&mut vm)?;
        let root = bindings.movie.root;
        // The stage size the game sets as _global.Screen_Width/Height (sk8popup centres its panel on it).
        let stage = &json["characters"][0]["movie"];
        for object in [vm.global, root] {
            // 0: retail's widescreen value. TU3 0x825D95A8 calls SetScreenEdgeOffset(136) only when the widescreen
            // byte (+0x12040, read by 0x825D3DD8) is clear (4:3); widescreen keeps 0.
            vm.set(object, "Screen_EdgeOffset", Value::Number(0.))?;
            vm.set(object, "Screen_Width", Value::Number(stage["width"].as_f64().unwrap_or(1280.)))?;
            vm.set(object, "Screen_Height", Value::Number(stage["height"].as_f64().unwrap_or(720.)))?;
        }
        drain(&mut vm, &mut bindings)?;
        // ponytail: the root var the constructor keeps (`var screen = new X(this)`; actionoverlay2's is `overlay`), not
        // the object it hands ScreenManager.OnLoaded; capture that call if a movie ever keeps neither.
        let controller = ["screen", "overlay"]
            .iter()
            .find_map(|name| match vm.get(root, name) {
                Value::Object(o) => Some(o),
                _ => None,
            })
            .unwrap_or(root);
        Ok(Self { vm, bindings, shapes, controller, placement: [1., 0., 0., 1., 0., 0.], sfx: None, assets: None })
    }

    /// What the movie gets back from `Object.Method`, or from `Object.Method(first argument)` for one argument
    /// (`HUDComponents.Objective_GetObjectiveTitle(0)`), or `Object.Method(a0,a1)` for two; the most specific key
    /// wins. Values or arrays.
    pub fn answer(&mut self, key: &str, value: Json) {
        self.bindings.answers.insert(key.to_owned(), value);
    }
    /// Sets `_global.<name>` (screens that read globals the game set, like `districtzone`).
    pub fn set_global(&mut self, name: &str, value: Json) -> Result<(), String> {
        let value = to_value(&mut self.vm, &value)?;
        let global = self.vm.global;
        self.vm.set(global, name, value)
    }
    /// The native calls the movie made since the last take, in order: (`Object.Method`, arguments).
    pub fn take_calls(&mut self) -> Vec<(String, Vec<Json>)> {
        std::mem::take(&mut self.bindings.calls)
    }
    /// A field of the controller (or root): leaderboardhud's `mNameArray`.
    pub fn field(&self, key: &str) -> Json {
        to_json(&self.vm, &self.vm.get(self.controller, key))
    }
    /// The controller (or root) defines `method`.
    pub fn has(&self, method: &str) -> bool {
        !matches!(self.vm.get(self.controller, method), Value::Undefined)
    }
    /// Calls the controller's `method` (`Show`, `SetState`, `UpdateObjectivesX`, ...); a method the movie lacks is
    /// an error.
    pub fn call(&mut self, method: &str, args: &[Json]) -> Result<Json, String> {
        self.call_on(self.controller, method, args)
    }
    /// The clip at `path` from the root ("mcLetterbox", "mScreen.mcTextSubtitle"), or from `_global` for a path
    /// starting "_global." (manualmeter's `_global.MeterForManualing`).
    pub fn clip(&self, path: &str) -> Option<usize> {
        let (start, path) = path.strip_prefix("_global.").map_or((self.bindings.movie.root, path), |p| (self.vm.global, p));
        path.split('.').filter(|p| !p.is_empty()).try_fold(start, |object, part| {
            match self.vm.get(object, part) {
                Value::Object(o) => Some(o),
                _ => None,
            }
        })
    }
    /// Calls `method` on the clip at `path` (see [`Screen::clip`]).
    pub fn call_at(&mut self, path: &str, method: &str, args: &[Json]) -> Result<Json, String> {
        let object = self.clip(path).ok_or_else(|| format!("{}: no clip {path}", self.bindings.label))?;
        self.call_on(object, method, args)
    }
    fn call_on(&mut self, object: usize, method: &str, args: &[Json]) -> Result<Json, String> {
        if matches!(self.vm.get(object, method), Value::Undefined) {
            return Err(format!("{}: no method {method}", self.bindings.label));
        }
        self.vm.begin_update();
        let args = args.iter().map(|a| to_value(&mut self.vm, a)).collect::<Result<Vec<_>, _>>()?;
        let result = self.vm.call_method(object, method, args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        Ok(to_json(&self.vm, &result))
    }
    /// Shows the root, as a native screen owner does for movies that start hidden (sk8popup).
    pub fn show(&mut self) -> Result<(), String> {
        let root = self.bindings.movie.root;
        self.vm.set(root, "_visible", Value::Bool(true))
    }
    /// Plays a root timeline label ("intro", "outro", "idle", ...).
    pub fn play(&mut self, label: &str) -> Result<(), String> {
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text(label.into())], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// A pad press, as the front end forwards it to the controller's UpdateInput.
    pub fn input(&mut self, key: Key) -> Result<(), String> {
        if !self.has("UpdateInput") {
            return Ok(());
        }
        self.call("UpdateInput", &[Json::from(key.apt().1), Json::from(0)]).map(drop)
    }
    /// One authored frame.
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        // Screens that animate from script (subtitle fades, timers) run onEnterFrame.
        let entering: Vec<usize> = (self.bindings.movie.instances.keys().copied())
            .filter(|&id| matches!(self.vm.get(id, "onEnterFrame"), Value::Object(_)))
            .collect();
        for id in entering {
            self.vm.call_method(id, "onEnterFrame", vec![], &mut self.bindings)?;
            drain(&mut self.vm, &mut self.bindings)?;
        }
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    /// Draws this movie's textures from its own export (`<export>` for `<export>/runtime/<movie>.json`), not its
    /// layer's root: Skate 1's tutorial and online movies share one layer but are exported to two folders.
    pub(crate) fn own_assets(&mut self, file: &Path) {
        self.assets = file.parent().and_then(Path::parent).map(Path::to_path_buf);
    }
    pub fn draws(&self) -> Result<Vec<Draw>, String> {
        apt_scene::draw_at(&self.bindings.movie, &self.vm, &self.shapes, self.placement)
    }
    /// Where clip `id` draws on the canvas (stage pixels): for a native drawing in a clip's space (trickanalyser's trace).
    pub fn stage_matrix(&self, id: usize) -> [f32; 6] {
        apt_scene::stage_matrix(&self.bindings.movie, &self.vm, id, self.placement)
    }
    /// Draws the whole screen under `m` (stage pixels): a loadMovie child in its parent clip (intro_overlay's logos).
    pub(crate) fn place(&mut self, m: [f32; 6]) {
        self.placement = m;
    }
    /// Turns the whole screen `degrees` clockwise about `pivot` (stage pixels), as the game places a screen.
    pub fn tilt(&mut self, degrees: f32, [x, y]: [f32; 2]) {
        let (s, c) = degrees.to_radians().sin_cos();
        self.placement = [c, s, -s, c, x - c * x + s * y, y - s * x - c * y];
    }
}

// ---------------------------------------------------------------- Bevy side
/// The games whose screens [`Screens`] shows: the asset folder of their converted screens and their canvas layer
/// (Skate 1 tutorial 37, Skate 2 tutorial 38: above each game's HUD, 36 / 35; online UI 39; Skate 2 Hall of Meat 40;
/// Skate 2 online HUD 41; Skate 1 online HUD 42; Skate 3's music screens 43; Skate 3's Trick Analyzer / Manual Meter 44;
/// Skate 3's Party Play 45; Skate 3's Replay Editor 46; Skate 3's other front-end screens 47).
const GAMES: [(&str, usize, isize); 12] = [
    ("private/challenges-skate1/screens", 37, -9),
    ("private/skate2-tutorial/screens", 38, -10),
    ("private/online-ui", 39, -11),
    ("private/skate2-online/hall-of-meat", 40, -12),
    ("private/hud-skate2/online", 41, -13),
    ("private/hud-skate1/online", 42, -14),
    ("private/music-ui", 43, -15),
    ("private/hud-meters", 44, -16),
    ("private/party-play", 45, -17),
    ("private/replay-ui", 46, -18),
    ("private/retail-screens", 47, -19),
    (CAREER_FOLDER, 48, -20),
];
/// The Skate 3 career export's screens; rooted at the career's own root ([`crate::skate3_career::root`]).
const CAREER_FOLDER: &str = "private/skate3-career/screens";
/// The online session UI's layer ([`Screens::layer`]).
pub(crate) const ONLINE: u8 = 3;
/// Skate 2's Hall of Meat screens (skate2_hall_of_meat.rs).
pub(crate) const HALL_OF_MEAT: u8 = 4;
/// Skate 2's online player HUD movies (hud-skate2/online: leaderboardhud, hudonlinevoting, ...; skate2_contests.rs).
pub(crate) const SKATE2_ONLINE: u8 = 5;
/// Skate 1's online HUD movies (hud-skate1/online: hud_online_eventlog, hud_online_players; online_hud.rs).
pub(crate) const SKATE1_ONLINE: u8 = 6;
/// Skate 3's now-playing chyron, Audio Settings and Music Player (music-ui; music_ui.rs).
pub(crate) const MUSIC: u8 = 7;
/// Skate 3's On Screen Help HUDs: the Trick Analyzer and Manual Meter (hud-meters; hud_meters.rs).
pub(crate) const HUD_METERS: u8 = 8;
/// Skate 3's Party Play setup, Press-A card and confirm popups (party-play; party_play.rs).
pub(crate) const PARTY_PLAY: u8 = 9;
/// Skate 3's Replay Editor: replay_editor3, replay_help2 and its popups (replay-ui; replay/hud.rs).
pub(crate) const REPLAY: u8 = 10;
/// Skate 3's Trick Guide, Extras, Video Calibration and subtitles (retail-screens; retail_ui.rs).
pub(crate) const RETAIL: u8 = 11;
/// The Skate 3 career's screens: Skate 101, the reward chain (skate3_career/popups.rs).
pub(crate) const CAREER: u8 = 12;

pub(crate) struct Layer {
    canvas: Canvas,
    root: PathBuf,
    /// Open screens by name, drawn in name order: name them "1-objective", "2-poster", ... to stack.
    pub open: BTreeMap<String, Screen>,
    /// Draws the game adds (stage pixels), drawn after the screens in key order (name tags).
    pub extra: BTreeMap<&'static str, Vec<Draw>>,
    /// Drawn even while the graphics menu is open.
    pub modal: bool,
    /// The canvas has draws (perf 10-18: kept while they stand).
    shows: bool,
}

/// The open Skate 1 / Skate 2 / online / Hall of Meat screens. `layer(1)` / `layer(2)` / `layer(ONLINE)` /
/// `layer(HALL_OF_MEAT)`.
#[derive(Resource)]
pub(crate) struct Screens {
    layers: Vec<Layer>,
    accumulated: f32,
    /// Replay Take Photo (replay/hud.rs): no layer draws, so the picture has no names, chat or tags (online safety Q11).
    pub photo: bool,
}
impl Screens {
    /// "layer/name" of every open screen (SKATE_PERF_MODES census).
    pub fn census(&self) -> Vec<String> {
        self.layers.iter().enumerate().flat_map(|(i, l)| l.open.keys().map(move |n| format!("{}/{n}", i + 1))).collect()
    }
    pub fn layer(&mut self, game: u8) -> &mut Layer {
        let i = usize::from(game).clamp(1, self.layers.len()) - 1;
        &mut self.layers[i]
    }
}
impl Layer {
    /// Loads `runtime/<movie>.json` of this game's screens as `name` with `answers` (see [`Screen::load`]),
    /// replacing a screen open under that name.
    pub fn open(&mut self, name: &str, movie: &str, answers: &[(&str, Json)]) -> Result<&mut Screen, String> {
        let screen = Screen::load(&self.root.join("runtime").join(format!("{movie}.json")), answers)?;
        self.open.insert(name.to_owned(), screen);
        Ok(self.open.get_mut(name).unwrap())
    }
    pub fn close(&mut self, name: &str) {
        self.open.remove(name);
    }
    pub fn get(&mut self, name: &str) -> Option<&mut Screen> {
        self.open.get_mut(name)
    }
}

pub(crate) struct ScreensPlugin;
impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(Update, (resize, render).chain().after(crate::app::FrameSet::Physics))
            .add_systems(PostUpdate, crate::apt_canvas::hide_idle_composites.before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate));
    }
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
    let layers = GAMES
        .iter()
        .map(|&(folder, layer, order)| {
            let root = if folder == CAREER_FOLDER {
                crate::skate3_career::root(&config.asset_root).join("screens")
            } else {
                config.asset_root.join(folder)
            };
            let canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), layer, order);
            commands.spawn((
                MaterialNode(canvas.composite.clone()),
                crate::apt_canvas::CompositeOf(canvas.camera()),
                UiTargetCamera(output),
                GlobalZIndex(stack(folder)),
                Pickable::IGNORE,
                Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
            ));
            Layer { canvas, root, open: BTreeMap::new(), extra: BTreeMap::new(), modal: false, shows: false }
        })
        .collect();
    commands.insert_resource(Screens { layers, accumulated: 0., photo: false });
}

fn resize(
    screens: Option<ResMut<Screens>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    let Some(mut screens) = screens else { return };
    for layer in &mut screens.layers {
        layer.canvas.resize(&window, &mut images, &mut composites);
    }
}

/// Plays `<sfx>/<key>.wav` once; a key without a file (a record Skate 2 leaves silent) plays nothing. Decoded here:
/// Bevy's own audio is built without WAV support (an AudioSource .wav panics its player). True when it played.
pub(crate) fn play_sfx(
    commands: &mut Commands,
    pcm: &mut Assets<crate::skate_audio::StreamPcm>,
    sfx: &Path,
    key: &str,
    settings: PlaybackSettings,
) -> bool {
    let path = sfx.join(format!("{key}.wav"));
    // perf 10-18: each wav is decoded once (the copies share its samples); SKATE_SFX_CACHE=0 decodes per play.
    static DECODED: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, crate::skate_audio::StreamPcm>>> =
        std::sync::LazyLock::new(Default::default);
    let cache = crate::config::perf_on("SKATE_SFX_CACHE");
    let cached = cache.then(|| DECODED.lock().ok()?.get(&path).cloned()).flatten();
    if cached.is_none() && !path.is_file() {
        return false;
    }
    let decoded = cached.map_or_else(
        || -> Result<_, String> {
            let sound = crate::challenge::movie::read_wav(&path)?;
            if let Some(mut decoded) = DECODED.lock().ok().filter(|_| cache) {
                decoded.insert(path.clone(), sound.clone());
            }
            Ok(sound)
        },
        Ok,
    );
    match decoded {
        Ok(sound) => {
            let volume = bevy::audio::Volume::Linear(settings.volume.to_linear() * fe_gain(sfx, key));
            commands.spawn((AudioPlayer(pcm.add(sound)), settings.with_volume(volume)));
            true
        }
        Err(error) => {
            warn!("{}: {error}", path.display());
            false
        }
    }
}

/// Skate 2's front-end gain for `<sfx>/<key>.wav` (8241b2a0): the fe record's Volume with the export's headroom scale
/// (sfx/gains.json, s2_fe_sounds) x SFXObj_Master's MixMap output 1 / 32767 x the voice group (dumpster-sound.md F5,
/// F9). A folder or key without a gain (an export from before gains.json, Skate 3's folders) plays the file as it is.
pub(crate) fn fe_gain(sfx: &Path, key: &str) -> f32 {
    // Not retail (user 10-13): Skate 2's output 1 and voice group at default settings are UNTRACED (F9); this is Skate
    // 3's measured master gain for the same chain (Gai0 = 1, skate-data audio/game_tests.rs).
    const MASTER: f32 = 1.;
    // perf 10-18: each folder's gains.json is parsed once (it was per play); SKATE_SFX_CACHE=0 reads it per play.
    type Gains = std::sync::Arc<HashMap<String, f32>>;
    static PARSED: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Gains>>> = std::sync::LazyLock::new(Default::default);
    let read = || -> Gains {
        std::sync::Arc::new(std::fs::read(sfx.join("gains.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default())
    };
    let gains = match PARSED.lock().ok().filter(|_| crate::config::perf_on("SKATE_SFX_CACHE")) {
        Some(mut parsed) => parsed.entry(sfx.to_path_buf()).or_insert_with(read).clone(),
        None => read(),
    };
    gains.get(key).copied().unwrap_or(1.) * MASTER
}

/// The Skate 3 online HUD movies (hudcountdown, hudcentral, ownthespot, killedit...) have no sfx folder: their
/// Audio.PlaySound keys are front-end vault sounds, played by id through GlobalFEPlaySound as retail does.
/// `slot`: the 0-based index into `Screens::layers` (game ONLINE lives at ONLINE - 1).
fn fe_fallback(slot: usize, played: bool, key: &str) -> Option<u64> {
    (!played && slot + 1 == ONLINE as usize).then(|| crate::core_menu_runtime::fe_sound_id(key))
}

/// Advances every open screen on real time (they animate while the game is paused) and draws them. A screen that
/// fails is closed with an error, never stopping the game.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    mut commands: Commands,
    screens: Option<ResMut<Screens>>,
    real: Res<Time<Real>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut cameras: Query<&mut Camera>,
    mut pcm: ResMut<Assets<crate::skate_audio::StreamPcm>>,
    mut fe: MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
) {
    let Some(mut screens) = screens else { return };
    let screens = &mut *screens;
    screens.accumulated = (screens.accumulated + real.delta_secs()).min(4. * FRAME);
    let mut frames = 0;
    while screens.accumulated >= FRAME {
        screens.accumulated -= FRAME;
        frames += 1;
    }
    let hidden = menu.as_ref().is_some_and(|m| m.open);
    let photo = screens.photo;
    for (index, layer) in screens.layers.iter_mut().enumerate() {
        let mut failed = Vec::new();
        for (name, screen) in &mut layer.open {
            let result = (0..frames).try_for_each(|_| screen.advance());
            // ponytail: one-shots only; StopSound (the reward screen's looping number scroll) isn't honoured.
            for key in screen.bindings.sounds.drain(..) {
                let settings = crate::music::one_shot(menu.as_deref(), crate::music::SFX);
                let played = play_sfx(&mut commands, &mut pcm, &sfx_dir(screen.sfx.as_deref(), &layer.root, &key), &key, settings);
                if let Some(id) = fe_fallback(index, played, &key) {
                    fe.write(crate::skate_audio::FrontEndSoundRequest(id));
                }
            }
            if let Err(error) = result {
                error!("Screen {name} stopped: {error}");
                failed.push(name.clone());
            }
        }
        for name in failed.drain(..) {
            layer.open.remove(&name);
        }
        // perf 10-18: the canvas keeps last frame's draws while nothing in them changed.
        let shown = drawn(hidden, layer.modal, photo);
        let key = if shown {
            let open: Vec<_> = layer
                .open
                .iter()
                .map(|(name, s)| (name, apt_scene::draw_key(&s.bindings.movie, &s.vm), s.placement.map(f32::to_bits), &s.assets))
                .collect();
            apt_scene::hash_of((open, &layer.extra))
        } else {
            0 // nothing drawn
        };
        if layer.canvas.up_to_date(key) {
            layer.canvas.set_active(layer.shows, &mut cameras);
            continue;
        }
        let mut draws = Vec::new();
        for (name, screen) in layer.open.iter().filter(|_| shown) {
            match screen.draws() {
                Ok(mut d) => {
                    // ponytail: a path string per draw per frame; cache per screen if it ever shows in a profile.
                    if let Some(root) = &screen.assets {
                        rooted(&mut d, root);
                    }
                    draws.extend(d)
                }
                Err(error) => {
                    error!("Screen {name} stopped: {error}");
                    failed.push(name.clone());
                }
            }
        }
        for name in failed {
            layer.open.remove(&name);
        }
        draws.extend(layer.extra.values().flatten().cloned());
        if !shown {
            draws.clear();
        }
        layer.shows = !draws.is_empty();
        layer.canvas.set_active(layer.shows, &mut cameras);
        layer.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
        // A screen whose draws failed is gone: the next frame's key differs.
        layer.canvas.drawn_as(key);
    }
}
/// Above the HUDs (1), below the movie player (60) and menus; Skate 3's own front-end screens also carry the movie
/// subtitles, so they sit over the player and the boot's skip prompt (61). The now-playing chyron shows over CAS (flag 1,
/// music_ui::flagged), so music-ui sits over Edit Skater (create_skater.rs 21 / 22, customiser.rs 20) and the Challenge
/// Map (12), below the loading tip (25). Bug 10-17 #38: at 2 it was drawn under Edit Skater.
fn stack(folder: &str) -> i32 {
    match folder {
        "private/retail-screens" => 62,
        "private/music-ui" => 23,
        _ => 2,
    }
}

/// A layer draws unless the graphics menu covers it (a modal one shows over it) or a replay photo is being taken.
fn drawn(menu_open: bool, modal: bool, photo: bool) -> bool {
    !photo && (modal || !menu_open)
}

#[cfg(test)]
mod tests {
    /// Bug 10-17 #38: the now-playing chyron draws over Edit Skater's screens (create_skater.rs GlobalZIndex 21 / 22).
    #[test]
    fn the_chyron_draws_over_edit_skater() {
        assert!((23..25).contains(&super::stack("private/music-ui")));
        assert_eq!(super::stack("private/hud-meters"), 2);
    }
    /// Online safety Q11: a replay photo shows no layer, modal ones (popups) included, so no other player's name.
    #[test]
    fn a_replay_photo_draws_no_layer() {
        assert!(super::drawn(false, false, false) && super::drawn(true, true, false) && !super::drawn(true, false, false));
        assert!(!super::drawn(false, false, true) && !super::drawn(false, true, true));
    }
    /// Skate 1's medical form drew nothing (preview 10-14): its textures are under hud-skate1/tutorial, not the
    /// layer's challenges-skate1/screens, so it waited unseen for A.
    #[test]
    fn a_screen_with_its_own_assets_draws_textures_from_its_export() {
        use std::path::Path;
        let draw = |t: &str| super::Draw { texture: t.into(), size: [1, 1], vertices: Vec::new(), multiply: [1.; 4], add: [0.; 4] };
        let mut draws = [draw("assets/fonts/tradebld.rgba"), draw("")];
        let root = Path::new("C:/inst/private/hud-skate1/tutorial");
        super::rooted(&mut draws, root);
        assert_eq!(Path::new(&draws[0].texture), root.join("assets/fonts/tradebld.rgba"));
        assert_eq!(draws[1].texture, "", "untextured fills stay untextured");
    }

    #[test]
    fn online_movie_sounds_without_a_wav_go_to_the_front_end_player() {
        // hudcountdown's 3-2-1 on the Skate 3 online layer has no sfx folder: the fe vault plays it
        // render enumerates `layers`, which Screens::layer(game) indexes as layers[game - 1].
        let slot = |game: u8| game as usize - 1;
        assert_eq!(fe_fallback(slot(ONLINE), false, "challenge_count_3"), Some(crate::core_menu_runtime::fe_sound_id("challenge_count_3")));
        assert_eq!(fe_fallback(slot(ONLINE), true, "challenge_count_3"), None, "a wav that played is not doubled");
        assert_eq!(fe_fallback(slot(SKATE2_ONLINE), false, "x"), None, "Skate 2 leaves its missing records silent");
        assert_eq!(fe_fallback(ONLINE as usize, false, "x"), None, "the slot after online-ui is Skate 2's Hall of Meat");
    }

    use super::*;

    /// Item 10: the online intros' overlay movies (intro_overlay.rs), as installed. A SpotFound corner's intro plays
    /// spot_found and names the spot, its outro hides the card; Skate 2's photo snap plays photo_flythrough; the San
    /// Van TV logo shows and hides. SKATE3_ASSET_ROOT = the install's assets.
    #[test]
    #[ignore = "requires the installed screens"]
    fn the_intro_overlay_movies_play_their_states() {
        fn play(file: &Path, states: &[i64]) -> (Vec<String>, bool, Json) {
            let answers = [
                ("Mission.GetIsOnlineMission", Json::from(true)),
                ("Mission.GetTitle", Json::from("Hubba Hideout")),
                ("Mission.GetLocation", Json::from("Hubba Hideout")),
            ];
            let mut s = Screen::load(file, &answers).unwrap();
            for &state in states {
                s.call("SetState", &[Json::from(state)]).unwrap();
                for _ in 0..60 {
                    s.advance().unwrap();
                }
            }
            let screen = match s.vm.get(s.controller, "mScreen") {
                Value::Object(o) => o,
                _ => s.bindings.movie.root,
            };
            // S3: mScreen.mcSpotName.labelText; S1/S2: mScreen.mcOwned.mcSpotName.labelText.text.
            let mut name = Json::Null;
            let mut at = screen;
            for key in ["mcOwned", "mcSpotName", "labelText", "text"] {
                match s.vm.get(at, key) {
                    Value::Object(o) => at = o,
                    Value::Undefined if key == "mcOwned" => {}
                    v => {
                        name = to_json(&s.vm, &v);
                        break;
                    }
                }
            }
            let hidden = [screen, s.bindings.movie.root].iter().any(|&o| matches!(s.vm.get(o, "_visible"), Value::Bool(false)));
            (std::mem::take(&mut s.bindings.sounds), !hidden, name)
        }
        let private = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private");
        for movie in ["hud-skate1/online/runtime/spotfound.json", "hud-skate2/online/runtime/spotfound.json", "skate3-online/hud/runtime/spotfound.json"] {
            let file = private.join(movie);
            for (intro, outro) in [(0, 1), (2, 3), (4, 5), (6, 7)] {
                assert_eq!(play(&file, &[intro]), (vec!["spot_found".into()], true, Json::from("Hubba Hideout")), "{movie} {intro}");
                assert!(!play(&file, &[intro, outro]).1, "{movie}: outro {outro} hides the card");
            }
        }
        assert_eq!(play(&private.join("hud-skate2/online/runtime/phototreatment.json"), &[1]).0, ["photo_flythrough"]);
        let (sounds, ..) = play(&private.join("hud-skate2/online/runtime/sanvantv.json"), &[0, -1]);
        assert!(sounds.is_empty(), "the logo is silent");
    }

    /// Every converted Skate 2 tutorial screen boots and runs a second of frames with no answers at all
    /// (SKATE2_SCREENS = private/skate2-tutorial/screens).
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn every_converted_screen_boots_and_runs() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap()).join("runtime");
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let mut screen = Screen::load(&path, &[]).unwrap();
            for _ in 0..60 {
                screen.advance().unwrap();
            }
            screen.draws().unwrap();
        }
    }

    #[test]
    fn two_argument_answers_win() {
        let answers = HashMap::from([
            ("Game.OnMenuGetItemEnabled".to_string(), Json::from(true)),
            ("Game.OnMenuGetItemEnabled(0)".to_string(), Json::from(true)),
            ("Game.OnMenuGetItemEnabled(0,3)".to_string(), Json::from(false)),
        ]);
        let get = |args: &[Json]| lookup(&answers, "Game.OnMenuGetItemEnabled", args);
        assert_eq!(get(&[Json::from(0), Json::from(3)]), Some(Json::from(false)));
        assert_eq!(get(&[Json::from(0), Json::from(2)]), Some(Json::from(true)));
        assert_eq!(get(&[]), Some(Json::from(true)));
    }

    #[test]
    fn format_strings_fill_their_values() {
        assert_eq!(fill("{0:d}%".into(), &[Json::from(70)]), "70%");
        assert_eq!(fill("{1} of {0}".into(), &[Json::from(3), Json::from("a")]), "a of 3");
    }

    /// `Objective_GetObjectiveTitle(0)` answers key on "0", not "0.0".
    #[test]
    fn whole_numbers_go_out_as_integers() {
        let vm = native_vm(&[]).unwrap();
        assert_eq!(to_json(&vm, &Value::Number(0.)).to_string(), "0");
        assert_eq!(to_json(&vm, &Value::Number(3.)).as_u64(), Some(3));
        assert_eq!(to_json(&vm, &Value::Number(0.5)).to_string(), "0.5");
    }

    /// sk8popup centres its panel on _global.Screen_Height; unset, the panel's _y went NaN and the popup stopped.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_popup_lays_out() {
        let file = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap()).join("runtime/sk8popup.json");
        let answers = [
            ("Popup.GetTitle", Json::from("Title")),
            ("Popup.GetDescription", Json::from("Body")),
            ("Popup.GetNumOptions", Json::from(2)),
            ("Popup.GetOption(0)", Json::from("Continue")),
            ("Popup.GetOption(1)", Json::from("Retry")),
        ];
        let mut screen = Screen::load(&file, &answers).unwrap();
        screen.show().unwrap();
        screen.call("OnLoaded", &[]).unwrap();
        screen.call("SetState", &[Json::from(0)]).unwrap();
        for _ in 0..90 {
            screen.advance().unwrap();
        }
        screen.draws().unwrap();
        screen.take_calls();
        // Down then A picks the second option (every option enabled).
        screen.input(Key::Down).unwrap();
        screen.input(Key::Accept).unwrap();
        let picked = screen.take_calls().into_iter().find(|(call, _)| call == "Game.OnMenuSelect").map(|(_, args)| args[1].clone());
        assert_eq!(picked, Some(Json::from(1)));
    }

    /// The tutorial prompt draws the push guide's A / X glyphs from FillTrickData's [count, (gesture, button,
    /// connector)...] (an extra slot shifted every field: gesture "0" asked for the unknown linkage g_0).
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_prompt_draws_its_buttons() {
        let file = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap()).join("runtime/hudtutorial.json");
        let answers = [
            ("HUDComponents.GetTutorialHalText", Json::from("Push")),
            ("HUDComponents.GetTutorialGesturesButtons", serde_json::json!([2, "", "A", "Or", "", "X", ""])),
        ];
        let mut screen = Screen::load(&file, &answers).unwrap();
        screen.play("hudintro").unwrap();
        screen.call("SetState", &[Json::from(246)]).unwrap();
        screen.call("SetState", &[Json::from(10)]).unwrap();
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        let drawn: BTreeSet<String> = screen.draws().unwrap().into_iter().map(|d| d.texture).collect();
        for button in ["A", "X"] {
            let glyph = &screen.bindings.movie.images[&format!("button:{button}")].rgba;
            assert!(drawn.contains(glyph), "{button} glyph not drawn");
        }
    }

    /// Offline snapshots of the tutorial screens (SKATE2_SHOTS = output folder), to compare with retail footage
    /// without a game window.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_screens_snapshot() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let out = PathBuf::from(std::env::var("SKATE2_SHOTS").unwrap());
        let snap = |screen: &mut Screen, name: &str| {
            for _ in 0..90 {
                screen.advance().unwrap();
            }
            crate::core_menu_runtime::tests::raster_draws(&screen.draws().unwrap(), &root, &out.join(format!("{name}.png")));
        };
        let objective = [
            ("HUDComponents.Objective_GetNumObjectives", Json::from(1)),
            ("HUDComponents.Objective_GetObjectiveTitle(0)", Json::from("Find the back entrance to Slappy's")),
            ("HUDComponents.Objective_GetObjectiveState(0)", Json::from(2)),
            ("HUDComponents.Objective_GetObjectiveType(0)", Json::from(1)),
            ("HUDComponents.Objective_GetObjectiveValue(0)", Json::from("")),
            ("HUDComponents.Objective_GetActivityTitle", Json::from("Meet Slappy")),
            ("HUDComponents.Objective_GetActivityType", Json::from("")),
        ];
        let mut screen = Screen::load(&root.join("runtime/hudobjective2.json"), &objective).unwrap();
        for state in [1, 3, 3] {
            screen.call("SetState", &[Json::from(state)]).unwrap();
        }
        snap(&mut screen, "objective");
        // The bar ends under the tab: ThreeSliceScale places the middle piece by the _width its _xscale gave it.
        let right = screen.draws().unwrap().iter().flat_map(|d| d.vertices.iter().map(|v| v.position[0])).fold(f32::MIN, f32::max);
        assert!(right <= 1184.5, "the objective box reaches x {right}");
        let (degrees, pivot) = crate::skate2_screens::OBJECTIVE_TILT;
        screen.tilt(degrees, pivot);
        snap(&mut screen, "objective-tilt");
        screen.answer("HUDComponents.Objective_GetObjectiveState(0)", Json::from(1));
        screen.call("SetState", &[Json::from(4)]).unwrap();
        snap(&mut screen, "objective-done");
        for (name, gesture, state) in [
            ("prompt-push", serde_json::json!([2, "", "A", "Or", "", "X", ""]), 10),
            ("prompt-turn", serde_json::json!([1, "", "Stick_Left", ""]), 10),
            ("prompt-onboard", serde_json::json!([1, "", "Y", ""]), 10),
            ("prompt-ollie", Json::Null, 12),
        ] {
            let answers = [("HUDComponents.GetTutorialHalText", Json::from(name)), ("HUDComponents.GetTutorialGesturesButtons", gesture)];
            let mut screen = Screen::load(&root.join("runtime/hudtutorial.json"), &answers).unwrap();
            screen.play("hudintro").unwrap();
            screen.call("SetState", &[Json::from(246)]).unwrap();
            screen.call("SetState", &[Json::from(state)]).unwrap();
            snap(&mut screen, name);
        }
        // The sign-up poster as skate2_screens opens it, with everything it asks answered.
        let description = "Prison might have left you a bit rusty, so let's warm up with a few tricks.\n\nWork 'em a bit and you'll be killing it in no time!";
        let answers = crate::skate2_screens::signup_answers("Flipping Wood 101", description, Some("View flip tricks"));
        let mut screen = Screen::load(&root.join("runtime/challenge_signup.json"), &answers).unwrap();
        screen.play("intro").unwrap();
        snap(&mut screen, "signup");
        assert!(screen.bindings.unanswered.is_empty(), "the poster asks {:?}", screen.bindings.unanswered);
    }

    /// I4: a movie plays a sound from its own export's sfx when that has it, else from its layer's.
    #[test]
    fn a_movie_plays_sounds_from_its_own_export_first() {
        let dir = std::env::temp_dir().join(format!("s3r-movie-sfx-{}", std::process::id()));
        let (own, layer) = (dir.join("skate/screens"), dir.join("tutorial/screens"));
        for folder in [own.join("sfx"), layer.join("sfx")] {
            std::fs::create_dir_all(folder).unwrap();
        }
        for file in [own.join("sfx/skate_letter.wav"), layer.join("sfx/skate_letter.wav"), layer.join("sfx/core_popup.wav")] {
            std::fs::write(file, b"").unwrap();
        }
        let mine = movie_sfx(&own.join("runtime/hudthrowmes.json"));
        assert_eq!(mine, Some(own.join("sfx")));
        assert_eq!(sfx_dir(mine.as_deref(), &layer, "skate_letter"), own.join("sfx"));
        assert_eq!(sfx_dir(mine.as_deref(), &layer, "core_popup"), layer.join("sfx"), "a key it lacks: the layer's");
        assert_eq!(sfx_dir(None, &layer, "skate_letter"), layer.join("sfx"));
        std::fs::remove_dir_all(dir).ok();
    }

    /// I4: every `FE_SOUND_` key of a screen a Skate 2 mode opens has a wav where [`render`] looks, and S.K.A.T.E.'s
    /// (opened on the tutorial layer) come from their own export, which online-skate2 writes, not from the tutorial's,
    /// which skate2-tutorial rewrites without them. Silent in retail, so no wav: central_message_default and
    /// challenge_failed (their fe records' sample is 0 in both banks, sk8_menu and Sk82_FE; Volume 0). hud-skate2/online
    /// needs online-skate1's fe_sounds (stages.py). SKATE3_ASSET_ROOT = the install's assets.
    #[test]
    #[ignore = "requires the installed screens"]
    fn every_mode_screen_sound_has_a_wav() {
        const SILENT: [&str; 2] = ["central_message_default", "challenge_failed"];
        let private = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private");
        let (tutorial, skate, hom, online) = ("skate2-tutorial/screens", "skate2-online/skate/screens", "skate2-online/hall-of-meat", "hud-skate2/online");
        // (layer root, the movie's folder, movies): skate2_skate_game on layer GAME; contests and Death Race's
        // countdown, timer and messages; Hall of Meat; the online HUD (online_hud.rs, skate2_contests.rs).
        let cases: [(&str, &str, &[&str]); 4] = [
            (tutorial, skate, &["hudabd", "hudthrowmes", "leaderboardhud"]),
            (tutorial, tutorial, &["hudcentral", "hudtimer", "hudcountdown"]),
            (hom, hom, &["homscoring", "leaderboardhud"]),
            (online, online, &["roundresults_lb", "throwdownresults", "leaderboardhud", "contests_voting", "dpo", "online_chyron"]),
        ];
        let (mut missing, mut own_folder) = (Vec::new(), 0);
        for (layer, folder, movies) in cases {
            for movie in movies {
                let file = private.join(folder).join(format!("runtime/{movie}.json"));
                let text = std::fs::read_to_string(&file).unwrap();
                let screen = Screen::load(&file, &[]).unwrap();
                for (at, _) in text.match_indices("FE_SOUND_") {
                    let key = text[at + 9..].chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect::<String>();
                    let key = key.to_ascii_lowercase();
                    let dir = sfx_dir(screen.sfx.as_deref(), &private.join(layer), &key);
                    if !dir.join(format!("{key}.wav")).is_file() {
                        if !SILENT.contains(&key.as_str()) {
                            missing.push(format!("{folder}/{movie}: {key}"));
                        }
                    } else if folder != layer {
                        assert_eq!(dir, private.join(folder).join("sfx"), "{movie}: {key} plays from another stage's export");
                        own_folder += 1;
                    }
                }
            }
        }
        assert!(own_folder >= 4, "S.K.A.T.E.'s letter and copy sounds: {own_folder}");
        assert!(missing.is_empty(), "no wav for {missing:?}");
    }

    /// I4: Skate 2's results screen, as online_hud opens it for contests, Death Race and Hall of Meat, reaches its
    /// `Audio.PlaySound(FE_SOUND_CHALLENGE_END_SUCCESS)`. SKATE3_ASSET_ROOT = the install's assets.
    #[test]
    #[ignore = "requires the installed screens"]
    fn skate2_results_play_their_sound() {
        use crate::match_core::{Mode, Standing};
        let root = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private/hud-skate2/online");
        let standing = |name: &str, score, local| Standing { name: name.into(), score, finished: true, local };
        let standings = [standing("Ann", 1200, false), standing("Danny", 900, true), standing("Bo", 300, false)];
        for (mode, kind) in [
            (Mode::DeathRace, "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE"),
            (Mode::SpotBattle, "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE"),
            (Mode::HallOfMeat, "ID_MAP_FILTER_CHALLENGETYPES_HALLOFMEAT"),
        ] {
            let answers = crate::online_hud::results_answers(mode, &standings, "#Projects Picnic", kind);
            let mut screen = Screen::load(&root.join("runtime/roundresults_lb.json"), &answers).unwrap();
            let mut sounds = Vec::new();
            for _ in 0..600 {
                screen.advance().unwrap();
                sounds.append(&mut screen.bindings.sounds);
            }
            println!("{mode:?}: {sounds:?}");
            assert!(sounds.iter().any(|s| s == "challenge_end_success"), "{mode:?}: {sounds:?}");
        }
    }

    /// Screens whose scripts lay out by TextField.maxscroll (retail: the wrapped line count, 1 without word wrap),
    /// each with text long enough to wrap (SKATE3_ASSET_ROOT = the install's assets; PNGs in SKATE_MAXSCROLL_SHOTS).
    #[test]
    #[ignore = "requires the installed screens"]
    fn maxscroll_screens_render() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let out = std::env::var("SKATE_MAXSCROLL_SHOTS").ok().map(PathBuf::from);
        let long = "Prison might have left you a bit rusty, so let's warm up with a few tricks. Work 'em a bit and you'll be killing it in no time!";
        let s2 = "private/skate2-tutorial/screens";
        let popup = [
            ("Popup.GetTitle", Json::from("Title")),
            ("Popup.GetDescription", Json::from(long)),
            ("Popup.GetNumOptions", Json::from(1)),
            ("Popup.GetOption(0)", Json::from("Continue")),
        ];
        let tutorial = [("HUDComponents.GetTutorialHalText", Json::from(long)), ("HUDComponents.GetTutorialGesturesButtons", Json::Null)];
        let signup = crate::skate2_screens::signup_answers("Flipping Wood 101", long, None);
        let card = [
            ("Mission.GetTitle", Json::from("Flipping Wood 101")),
            ("Mission.GetDescription", Json::from(long)),
            ("Mission.GetChallengeTypeName", Json::from("")),
            ("Mission.GetIconString", Json::from("")),
        ];
        let chyron = [("HUDComponents.OnlineChyron_GetData", serde_json::json!(["#Steam", format!("#{long}")]))];
        let subtitle = [("Game.GetMovieSubtitleString", Json::from(long))];
        type Drive = fn(&mut Screen);
        let cases: [(&str, &str, &[(&str, Json)], Drive); 7] = [
            ("private/retail-screens", "subtitle", &subtitle, |s| drop(s.call("RefreshSubtitle", &[]).unwrap())),
            (s2, "subtitle", &subtitle, |s| drop(s.call("RefreshSubtitle", &[]).unwrap())),
            (s2, "sk8popup", &popup, |s| {
                s.show().unwrap();
                s.call("OnLoaded", &[]).unwrap();
                s.call("SetState", &[Json::from(0)]).unwrap();
            }),
            (s2, "hudtutorial", &tutorial, |s| {
                s.play("hudintro").unwrap();
                s.call("SetState", &[Json::from(246)]).unwrap();
                s.call("SetState", &[Json::from(12)]).unwrap();
            }),
            (s2, "challenge_signup", &signup, |s| s.play("intro").unwrap()),
            (s2, "challenge_objectives", &card, |s| drop(s.play("intro"))),
            ("private/hud-skate2/online", "online_chyron", &chyron, |s| drop(s.call("SetState", &[Json::from(6)]).unwrap())),
        ];
        for (folder, movie, answers, drive) in cases {
            let root = assets.join(folder);
            let mut screen = Screen::load(&root.join(format!("runtime/{movie}.json")), answers).unwrap();
            drive(&mut screen);
            for _ in 0..90 {
                screen.advance().unwrap();
            }
            let fields: Vec<(String, f64)> = (screen.bindings.movie.instances.keys())
                .filter_map(|&id| match (screen.vm.get(id, "_displayText"), screen.vm.get(id, "maxscroll")) {
                    (Value::Text(t), Value::Number(n)) if !t.is_empty() => Some((t, n)),
                    _ => None,
                })
                .collect();
            println!("{folder}/{movie}: {fields:?}; asks {:?}", screen.bindings.unanswered);
            let draws = screen.draws().unwrap();
            assert!(!draws.is_empty(), "{movie} draws nothing");
            assert!(fields.iter().any(|(t, n)| t.len() > 60 && *n >= 2.), "{movie}: no wrapped long field in {fields:?}");
            if let Some(out) = &out {
                let name = format!("maxscroll-{}-{movie}.png", if folder.contains("skate2") { "s2" } else { "s3" });
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join(name));
            }
        }
    }

    /// Perf 10-12: what `Layer::open` costs on the main thread per installed screen, by phase. SKATE3_ASSET_ROOT =
    /// the install's assets. `cargo test ... screen_open_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires the installed screens"]
    fn screen_open_cost() {
        use std::time::Instant;
        let assets = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.;
        let (mut total, mut saved) = (0., 0.);
        for file in [
            "private/replay-ui/runtime/replay_editor3.json",
            "private/replay-ui/runtime/browser.json",
            "private/music-ui/runtime/musicplayertest.json",
            "private/music-ui/runtime/options.json",
            "private/party-play/runtime/partyplay.json",
            "private/online-ui/runtime/friends.json",
            "private/online-ui/runtime/sk8popup.json",
            "private/hud-skate2/online/runtime/leaderboardhud.json",
            "private/hud-skate1/online/runtime/skatetrick.json",
            "private/retail-screens/runtime/trickguide.json",
            "private/skate2-tutorial/screens/runtime/hudtutorial.json",
        ] {
            let path = assets.join(file);
            let _ = Screen::load(&path, &[]).unwrap(); // warm the file cache
            let t = Instant::now();
            let bytes = std::fs::read(&path).unwrap();
            let read = ms(t);
            let t = Instant::now();
            let json: Json = serde_json::from_slice(&bytes).unwrap();
            let parse = ms(t);
            let t = Instant::now();
            crate::core_menu_runtime::define_fe_sounds(&json, &mut native_vm(&NATIVES).unwrap()).unwrap();
            let sounds = ms(t);
            // Pre-10-12 costs this load no longer pays: the to_string scan, and the subtree clones from_value took.
            let t = Instant::now();
            let names = crate::core_menu_runtime::tests::fe_sound_names_by_text(&json);
            let old_sounds = ms(t);
            let t = Instant::now();
            assert_eq!(names, crate::core_menu_runtime::tests::fe_sound_names_by_walk(&json));
            eprint!("[walk alone {:.1} ms, {} names] ", ms(t), names.len());
            let t = Instant::now();
            for key in ["characters", "actions", "exports", "scoped_exports", "language", "shapes", "buttons", "images", "init_order"] {
                drop(json[key].clone());
            }
            for font in json["fonts"].as_object().into_iter().flat_map(|f| f.values()) {
                drop(font["definition"]["glyphs"].clone());
            }
            let clones = ms(t);
            saved += old_sounds - sounds + clones;
            let t = Instant::now();
            drop(crate::apt_movie::Movie::load(&json).unwrap());
            let movie = ms(t);
            let t = Instant::now();
            let screen = Screen::load(&path, &[]).unwrap();
            let load = ms(t);
            total += load;
            eprintln!(
                "{file}: {:.1} MB, load {load:.1} ms (read {read:.1}, parse {parse:.1}, fe sounds {sounds:.1} [was {old_sounds:.1}], Movie::load {movie:.1}); was ~{:.1} ms (+ {clones:.1} ms subtree clones)",
                bytes.len() as f64 / 1e6,
                load + old_sounds - sounds + clones
            );
            drop(screen);
            let t = Instant::now();
            drop(json);
            eprintln!("  (dropping the parsed manifest: {:.1} ms of that load)", ms(t));
        }
        eprintln!("total {total:.1} ms, was ~{:.1} ms", total + saved);
    }

    /// Task 9: a Skate 2 fe sound plays at its record's Volume over the export's headroom scale (sfx/gains.json), each
    /// folder its own; a folder or key without one plays the file as it is.
    #[test]
    fn fe_gain_reads_each_folders_gains() {
        let dir = std::env::temp_dir().join(format!("s3r-fe-gain-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        for (folder, gain) in [(&a, 1.7), (&b, 0.85)] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("gains.json"), format!(r#"{{"multiplyer_3": {gain}}}"#)).unwrap();
        }
        assert_eq!(fe_gain(&a, "multiplyer_3"), 1.7);
        assert_eq!(fe_gain(&b, "multiplyer_3"), 0.85, "each folder reads its own gains");
        assert_eq!(fe_gain(&a, "core_popup"), 1.0);
        assert_eq!(fe_gain(&dir.join("none"), "multiplyer_3"), 1.0);
        // perf 10-18: parsed once per folder.
        std::fs::write(a.join("gains.json"), r#"{"multiplyer_3": 0.1}"#).unwrap();
        assert_eq!(fe_gain(&a, "multiplyer_3"), 1.7, "the folder's gains were cached");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// perf 10-18: a warmed manifest is parsed on a worker and kept for the last WARM_KEPT warms; a kept one stays.
    #[test]
    fn warmed_manifests_are_parsed_ahead_and_the_oldest_dropped() {
        let dir = std::env::temp_dir().join(format!("s3r-screen-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<_> = (0..=WARM_KEPT).map(|i| dir.join(format!("m{i}.json"))).collect();
        for (i, file) in files.iter().enumerate() {
            std::fs::write(file, format!(r#"{{"n": {i}}}"#)).unwrap();
        }
        let wait = |file: &Path| {
            let start = std::time::Instant::now();
            while parsed_lock().json.get(file).is_some_and(Option::is_none) && start.elapsed().as_secs() < 10 {
                std::thread::yield_now();
            }
        };
        for file in &files {
            warm([file.clone()]);
            wait(file);
        }
        assert!(parsed(&files[0]).is_none(), "the oldest warm is dropped");
        assert_eq!(parsed(&files[WARM_KEPT]).unwrap()["n"], WARM_KEPT);
        let popup = dir.join("popup.json");
        std::fs::write(&popup, r#"{"n": 9}"#).unwrap();
        assert_eq!(kept_json(&popup).unwrap()["n"], 9);
        std::fs::write(&popup, r#"{"n": 1}"#).unwrap();
        assert_eq!(kept_json(&popup).unwrap()["n"], 9, "parsed once");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
