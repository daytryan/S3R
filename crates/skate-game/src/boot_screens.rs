//! The boot flow's original APT movies (bootflow/esrb, pressstart, welcomescreen, coachfrank,
//! TeamSkater_Selection/team_management_boot, screens/main/fmv_popup, controls/autosave) on the shared APT
//! runtime, with the natives they call. Bevy-independent: boot.rs shows them as boot_flow.rs orders. Edit Skaters
//! and Import Skater (TeamSkater_Selection/team_management, import_skater) run here too: their TeamManagement /
//! ImportSkaters natives and team images are team_select::Natives.
use crate::{
    apt_movie::Movie,
    apt_scene::{Draw, Shapes},
    apt_vm::{Host, ObjectKind, Value, Vm},
    core_menu_runtime::{
        boot_movie, boot_screen, common_call, define_fe_sounds, drain, fe_sound_id, native_vm, played_sound,
        screen_property_changed, Key, ScreenHost,
    },
};

/// AptDUp, AptDDown, AptDLeft, AptDRight: what [`Screen::input`] sends after Up, Down, Left, Right.
const SPECIFIC_KEYS: [(&str, f64); 4] = [("AptDUp", 16.), ("AptDDown", 17.), ("AptDLeft", 18.), ("AptDRight", 19.)];
const STICK_KEYS: [(&str, f64); 4] = [("AptLStickUp", 24.), ("AptLStickDown", 25.), ("AptLStickLeft", 26.), ("AptLStickRight", 27.)];

/// `_global.GetPlatform()` / `PlatformCurrent`: the Xbox 360 build (its button glyphs, its title text).
pub(crate) const PLATFORM_XBOX360: f64 = 1.;
pub(crate) const PLATFORM_PS3: f64 = 2.;

pub(crate) struct Bindings {
    pub movie: Movie,
    pub shapes: Shapes,
    random: u32,
    pub intro_complete: bool,
    pub outro_complete: bool,
    /// `Game.OnMenuSelect(menu, index)` since the last input.
    pub selected: Option<usize>,
    /// Front-end sound ids the movie played (`Audio.PlaySound`), drained by the host.
    pub sounds: Vec<u64>,
    /// `TeamManagement.GetNewTeamState()`: 0 the welcome screen, 10 the team-name prompt.
    new_team_state: f64,
    /// Picker rows `Game.OnMenuGetItemEnabled` reports greyed.
    pub disabled: Vec<usize>,
    profile: String,
    /// The team the TeamManagement / ImportSkaters natives answer from, and the images they load.
    pub team: crate::team_select::Natives,
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
        // Ahead of common_call's ("Audio", _): fmv_popup's ShowMovieSkip sets _y = -168 over a subtitle line.
        if method == "HasSubtitles" && matches!(vm.objects.get(object).map(|o| &o.kind), Some(ObjectKind::Native(n)) if n == "Audio") {
            return Ok(Value::Bool(crate::retail_ui::SUBTITLE_SHOWING.load(std::sync::atomic::Ordering::Relaxed)));
        }
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(ObjectKind::Native(name)) => name.clone(),
            _ => "global".into(),
        };
        // Before the team natives: their TeamManagement catch-all answers undefined, and welcomescreen SetStates by this.
        if native == "TeamManagement" && method == "GetNewTeamState" {
            return Ok(Value::Number(self.new_team_state));
        }
        if let Some(value) = self.team.call(&mut self.movie, vm, &native, method, &args)? {
            return Ok(value);
        }
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
            ("Game", "OnMenuSelect") => {
                self.selected = Some(arg(1).number().max(0.) as usize);
                Value::Undefined
            }
            ("Game", "OnMenuGetItemEnabled") => Value::Bool(!self.disabled.contains(&(arg(1).number().max(0.) as usize))),
            ("Game", "GetProfileName") => Value::Text(crate::apt_text::literal(&self.profile)),
            ("Game", _) => Value::Undefined,
            ("TeamManagement", _) => Value::Undefined,
            ("global", "GetPlatform") => Value::Number(PLATFORM_XBOX360),
            // Flash: calling a method a clip's class lacks does nothing (welcomescreen calls mMenu.SetSkin, which
            // menu_picker never defines).
            (clip, _) if clip.starts_with("movie:") => Value::Undefined,
            _ => return Err(format!("Unimplemented boot screen binding {native}.{method}")),
        })
    }
}

pub(crate) struct Screen {
    pub vm: Vm,
    pub bindings: Bindings,
    /// The root's `screen` controller (none for the level movies fmv_popup and autosave).
    screen: Option<usize>,
}
impl Screen {
    fn vm() -> Result<Vm, String> {
        let mut vm = native_vm(&["TeamManagement", "ImportSkaters"])?;
        for (name, value) in
            [("Platform_XBOX360", PLATFORM_XBOX360), ("Platform_PS3", PLATFORM_PS3), ("PlatformCurrent", PLATFORM_XBOX360)]
        {
            vm.set(vm.global, name, Value::Number(value))?;
        }
        // NTSC: the legal screen shows the ESRB notice (esrb.apt's PALMode test).
        vm.set(vm.global, "PALMode", Value::Bool(false))?;
        // The source-specific direction keys (FE key table 0x83027BF0 entries 16-19, 24-27) team_management tests
        // beside the generic ones (native_vm's): a D-pad press is the generic key, then its D-pad key.
        for (name, value) in SPECIFIC_KEYS.iter().copied().chain(STICK_KEYS) {
            vm.set(vm.global, name, Value::Number(value))?;
        }
        Ok(vm)
    }
    fn bindings(json: &serde_json::Value, new_team_state: f64, profile: &str) -> Result<Bindings, String> {
        let mut movie = Movie::load(json)?;
        let buttons: std::collections::BTreeMap<String, crate::apt_scene::Texture> =
            serde::Deserialize::deserialize(&json["buttons"]).unwrap_or_default();
        movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        Ok(Bindings {
            movie,
            shapes: serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?,
            random: 0x9e37_79b9,
            intro_complete: false,
            outro_complete: false,
            selected: None,
            sounds: Vec::new(),
            new_team_state,
            disabled: Vec::new(),
            profile: profile.to_owned(),
            team: Default::default(),
        })
    }
    /// A screen: class libraries, the root, then the intro (the native screen manager's load).
    pub fn load(json: &serde_json::Value, new_team_state: f64, profile: &str, disabled: Vec<usize>) -> Result<Self, String> {
        let mut vm = Self::vm()?;
        define_fe_sounds(&json["actions"], &mut vm)?;
        let mut bindings = Self::bindings(json, new_team_state, profile)?;
        bindings.disabled = disabled;
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        Ok(Self { vm, bindings, screen: Some(screen) })
    }
    /// A level movie main.apt loads with getURL (fmv_popup, autosave): built and left on its first frame, where its
    /// constructor publishes its `_global` functions.
    pub fn level(json: &serde_json::Value) -> Result<Self, String> {
        let mut vm = Self::vm()?;
        define_fe_sounds(&json["actions"], &mut vm)?;
        let mut bindings = Self::bindings(json, 0., "")?;
        boot_movie(json, &mut vm, &mut bindings)?;
        Ok(Self { vm, bindings, screen: None })
    }
    pub fn set_state(&mut self, state: f64) -> Result<(), String> {
        let Some(screen) = self.screen else { return Ok(()) };
        self.vm.begin_update();
        self.vm.call_method(screen, "SetState", vec![Value::Number(state)], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// The key to the screen's UpdateInput; returns `Game.OnMenuSelect`'s index if the key picked a row.
    pub fn input(&mut self, key: Key) -> Result<Option<usize>, String> {
        self.input_code(key.apt().1)
    }
    /// A direction as the front end sends it: the generic key, then the D-pad's own (AptRight, then AptDRight).
    pub fn input_pad(&mut self, key: Key) -> Result<Option<usize>, String> {
        let specific = match key {
            Key::Up => SPECIFIC_KEYS[0].1,
            Key::Down => SPECIFIC_KEYS[1].1,
            Key::Left => SPECIFIC_KEYS[2].1,
            Key::Right => SPECIFIC_KEYS[3].1,
            _ => return self.input(key),
        };
        let first = self.input(key)?;
        Ok(first.or(self.input_code(specific)?))
    }
    fn input_code(&mut self, code: f64) -> Result<Option<usize>, String> {
        let Some(screen) = self.screen else { return Ok(None) };
        self.bindings.selected = None;
        self.vm.begin_update();
        let args = vec![Value::Number(code), Value::Number(0.)];
        self.vm.call_method(screen, "UpdateInput", args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        Ok(self.bindings.selected.take())
    }
    /// A `_global` function a level movie published (ShowMovieSkip, Load_EnableTag, ...).
    pub fn call_global(&mut self, name: &str) -> Result<(), String> {
        self.vm.begin_update();
        let global = self.vm.global;
        self.vm.call_method(global, name, Vec::new(), &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// A method of the clip at `path` (dotted, from the root), as the movie's own code calls it.
    pub fn call_clip(&mut self, path: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let mut object = self.bindings.movie.root;
        for name in path.split('.') {
            let Value::Object(next) = self.vm.get(object, name) else { return Err(format!("{path}: no {name}")) };
            object = next;
        }
        self.vm.begin_update();
        let value = self.vm.call_method(object, method, args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        Ok(value)
    }
    /// Hides a level movie's root, as fmv_popup's f20 does after HideMovieSkip's `gotoAndPlay("end")`. That goto
    /// re-runs f19's stop() in this runtime, and from the prompt's intro (f2-f18) it stops on f19 still visible.
    pub fn hide(&mut self) -> Result<(), String> {
        let root = self.bindings.movie.root;
        self.vm.set(root, "_visible", Value::Bool(false))
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text("outro".into())], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// One authored frame (16 ms).
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    pub fn draws(&self) -> Result<Vec<Draw>, String> {
        crate::apt_scene::draw(&self.bindings.movie, &self.vm, &self.bindings.shapes)
    }
    pub fn take_sounds(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.bindings.sounds)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn root() -> PathBuf {
        PathBuf::from(std::env::var("SKATE_BOOT_ROOT").unwrap())
    }

    #[test]
    #[ignore = "requires the prepared boot screens (SKATE_BOOT_ROOT)"]
    fn the_skip_prompt_rises_over_a_subtitle_line() {
        let top = |subtitled: bool| {
            crate::retail_ui::SUBTITLE_SHOWING.store(subtitled, std::sync::atomic::Ordering::Relaxed);
            let mut skip = Screen::level(&manifest("fmv_popup")).unwrap();
            skip.call_global("ShowMovieSkip").unwrap();
            for _ in 0..20 {
                skip.advance().unwrap();
            }
            skip.draws().unwrap().iter().flat_map(|d| d.vertices.iter().map(|v| v.position[1])).fold(f32::MAX, f32::min)
        };
        let (plain, subtitled) = (top(false), top(true));
        crate::retail_ui::SUBTITLE_SHOWING.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!((plain - subtitled - 168.).abs() < 0.5, "fmv_popup's _y = -168 (plain {plain}, subtitled {subtitled})");
    }
    pub(crate) fn manifest(name: &str) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(root().join(format!("runtime/{name}.json"))).unwrap()).unwrap()
    }
    fn snap(draws: &[Draw], name: &str) {
        if let Ok(out) = std::env::var("SKATE_BOOT_OUT") {
            crate::core_menu_runtime::tests::raster_draws(draws, &root(), &Path::new(&out).join(format!("{name}.png")));
        }
    }
    fn run(screen: &mut Screen, frames: u32) {
        for _ in 0..frames {
            screen.advance().unwrap();
        }
    }
    fn text(screen: &Screen, path: &[&str]) -> String {
        let mut object = screen.bindings.movie.root;
        for name in path {
            let Value::Object(next) = screen.vm.get(object, name) else { panic!("{name} missing") };
            object = next;
        }
        screen.vm.get(object, "_displayText").text()
    }

    /// Visual and behavioural check against the prepared boot movies (SKATE_BOOT_ROOT = private/boot; PNGs in
    /// SKATE_BOOT_OUT).
    #[test]
    #[ignore = "requires the prepared boot screens"]
    fn boot_screens_run_and_render() {
        let mut esrb = Screen::load(&manifest("esrb"), 0., "", Vec::new()).unwrap();
        run(&mut esrb, 30);
        assert!(esrb.bindings.intro_complete, "ESRB intro (IntroComplete f29)");
        snap(&esrb.draws().unwrap(), "boot-0-esrb");
        assert!(!esrb.draws().unwrap().is_empty());

        let mut title = Screen::load(&manifest("pressstart"), 0., "", Vec::new()).unwrap();
        title.set_state(1.).unwrap();
        run(&mut title, 120);
        assert_eq!(title.take_sounds(), [fe_sound_id("skate3_title")], "pressstart f10 plays skate3_title");
        let shown = text(&title, &["mStartText", "labelText"]);
        assert!(shown.contains('\u{E000}') || shown.contains("<B:Start>"), "the 360 text: {shown}");
        snap(&title.draws().unwrap(), "boot-1-pressstart");
        title.set_state(2.).unwrap();
        run(&mut title, 30);
        title.close().unwrap();
        run(&mut title, 20);
        assert!(title.bindings.outro_complete, "pressstart OutroComplete f150");

        let mut welcome = Screen::load(&manifest("welcomescreen"), 0., "", Vec::new()).unwrap();
        run(&mut welcome, 30);
        assert!(welcome.bindings.intro_complete);
        let played = welcome.take_sounds();
        assert!(played.contains(&fe_sound_id("skate3_welcome")), "skate3_welcome: {played:x?}");
        snap(&welcome.draws().unwrap(), "boot-2-welcome-difficulty");
        assert_eq!(welcome.input(Key::Down).unwrap(), None);
        assert_eq!(welcome.take_sounds(), [fe_sound_id("core_nav_down")]);
        run(&mut welcome, 10);
        assert_eq!(welcome.input(Key::Accept).unwrap(), Some(2), "Challenge Me");
        welcome.set_state(1.).unwrap();
        run(&mut welcome, 40);
        snap(&welcome.draws().unwrap(), "boot-3-welcome-camera-low");
        welcome.set_state(2.).unwrap();
        run(&mut welcome, 20);
        snap(&welcome.draws().unwrap(), "boot-4-welcome-camera-high");

        let mut prompt = Screen::load(&manifest("welcomescreen"), 10., "", Vec::new()).unwrap();
        run(&mut prompt, 40);
        snap(&prompt.draws().unwrap(), "boot-5-team-prompt");

        // Slot 67 as the boot loads it: the team's natives (boss photo: its background art here), Preset Skaters live.
        let mut team = Screen::load(&manifest("team_management_boot"), 0., "Player", Vec::new()).unwrap();
        let root = std::path::PathBuf::from(std::env::var("SKATE_BOOT_ROOT").unwrap());
        if let Ok(data) = crate::team_select::Data::load(&root) {
            let photo = data.team.thumbnails.get("team_profile_background_0").cloned();
            let settings = std::env::temp_dir().join(format!("skate3-boot-render-{}", std::process::id()));
            let t = crate::team_select::load_team(&data, &settings);
            let friends: crate::team_select::Friends = std::sync::Arc::new(std::sync::Mutex::new(crate::team_select::NoFriends));
            team.bindings.team = crate::team_select::Manager::new(data, t, settings.clone(), friends).natives();
            if let Some(photo) = photo {
                let movie = &mut team.bindings.movie;
                team.bindings.team.set_image(movie, &crate::team_select::photo_name(0), photo);
            }
            let _ = std::fs::remove_dir_all(settings);
        }
        team.set_state(0.).unwrap();
        run(&mut team, 30);
        snap(&team.draws().unwrap(), "boot-6-team-management");
        assert_eq!(team.input(Key::Down).unwrap(), None);
        run(&mut team, 5);
        assert_eq!(team.input(Key::Accept).unwrap(), Some(1), "Preset Skaters is live");

        let mut frank = Screen::load(&manifest("coachfrank"), 0., "", Vec::new()).unwrap();
        run(&mut frank, 50);
        let played = frank.take_sounds();
        assert!(played.contains(&fe_sound_id("skate3_tutorial")), "skate3_tutorial: {played:x?}");
        snap(&frank.draws().unwrap(), "boot-7-coach-frank");
        assert_eq!(frank.input(Key::Accept).unwrap(), Some(0), "Yes");

        let mut skip = Screen::level(&manifest("fmv_popup")).unwrap();
        skip.call_global("ShowMovieSkip").unwrap();
        run(&mut skip, 20);
        snap(&skip.draws().unwrap(), "boot-8-movie-skip");
        assert!(!skip.draws().unwrap().is_empty());
        // boot.rs's MovieSkip(false): HideMovieSkip, then the root hidden as f20 would.
        skip.call_global("HideMovieSkip").unwrap();
        skip.hide().unwrap();
        run(&mut skip, 3);
        assert!(skip.draws().unwrap().is_empty(), "hidden");
        // Hidden during its intro (f2-f18) too: it must not leak into the next movie.
        skip.call_global("ShowMovieSkip").unwrap();
        run(&mut skip, 5);
        skip.call_global("HideMovieSkip").unwrap();
        skip.hide().unwrap();
        run(&mut skip, 20);
        assert!(skip.draws().unwrap().is_empty(), "still hidden once its intro has run out");
        skip.call_global("ShowMovieSkip").unwrap();
        run(&mut skip, 20);
        assert!(!skip.draws().unwrap().is_empty(), "the next movie's prompt shows again");

        let mut tag = Screen::level(&manifest("autosave")).unwrap();
        tag.call_global("Load_EnableTag").unwrap();
        run(&mut tag, 20);
        snap(&tag.draws().unwrap(), "boot-9-loading-tag");
        assert!(!tag.draws().unwrap().is_empty());
        tag.call_global("Load_HideTag").unwrap();
        assert!(tag.draws().unwrap().is_empty());

        let mut popup =
            crate::popup_runtime::Runtime::load(&manifest("save_warning"), "MC_AUTOSAVE_WARNING2_PS3", &["ID_COMMON_OK"], false).unwrap();
        for _ in 0..30 {
            popup.advance().unwrap();
        }
        snap(&crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes).unwrap(), "boot-10-autosave-popup");
        // Refresh (mScreen = the movie root) lays the panel out by body.maxscroll * 24, which retail's TextField +16 makes the wrapped line
        // count: the body centred on y 360 and the button frame 16 + 10 below its last line, clear of the text.
        let at = |path: &[&str]| {
            let mut object = popup.bindings.movie.root;
            for name in path {
                let Value::Object(next) = popup.vm.get(object, name) else { panic!("{name} missing") };
                object = next;
            }
            object
        };
        let (body, bottom) = (at(&["mcPanel", "body"]), at(&["mcPanel", "mFrameBottom"]));
        let lines = popup.vm.get(body, "maxscroll").number();
        let body_y = popup.vm.get(body, "_y").number();
        assert!(lines >= 4., "MC_AUTOSAVE_WARNING2_PS3 wraps to several lines: maxscroll {lines}");
        assert_eq!(body_y, 360. - lines * 24. / 2.);
        assert_eq!(popup.vm.get(bottom, "_y").number(), body_y + lines * 24. + 16. + 10.);
        assert_eq!(popup.input(Key::Accept).unwrap(), Some(0));
        let mut invalid =
            crate::popup_runtime::Runtime::load(&manifest("sk8popup"), "ID_VIRTUALKEYBOARD_ALPHA_NUMERIC_STRING_ERROR", &["ID_COMMON_OK"], false)
                .unwrap();
        for _ in 0..30 {
            invalid.advance().unwrap();
        }
        snap(&crate::apt_scene::draw(&invalid.bindings.movie, &invalid.vm, &invalid.bindings.shapes).unwrap(), "boot-11-popup-263");
    }

    /// Bug 10-29 #3: welcomescreen's constructor runs SetState(TeamManagement.GetNewTeamState()), which retail
    /// (0x825C3D08) answers 10 once the welcome has popped, else 0. Undefined matched no state: no picker, no prompt.
    #[test]
    fn get_new_team_state_answers_the_screens_state() {
        let json = serde_json::json!({"characters": [], "actions": {}, "shapes": {}, "language": {}});
        for state in [0., 10.] {
            let mut vm = Screen::vm().unwrap();
            let mut bindings = Screen::bindings(&json, state, "").unwrap();
            let Value::Object(team) = vm.get(vm.global, "TeamManagement") else { panic!("no TeamManagement native") };
            assert_eq!(bindings.call(&mut vm, team, "GetNewTeamState", Vec::new()).unwrap(), Value::Number(state));
        }
    }
}
