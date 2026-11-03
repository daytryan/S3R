//! Skate 3's confirmation popup (popup/sk8popup) on the shared APT runtime. The
//! movie asks the native `Popup` object for its text and choices and reports the
//! pick through Game.OnMenuSelect. Bevy-independent.
use crate::{
    apt_movie::Movie,
    apt_scene::Shapes,
    apt_vm::{Host, ObjectKind, Value, Vm},
    core_menu_runtime::{
        boot_screen, common_call, define_fe_sounds, drain, fe_sound_id, native_vm, played_sound, screen_property_changed, Key, ScreenHost,
    },
};

/// Choice index reported for B on a cancelable popup.
pub(crate) const CANCEL: usize = 255;

pub(crate) struct Bindings {
    pub movie: Movie,
    pub shapes: Shapes,
    description: String,
    options: Vec<String>,
    cancelable: bool,
    highlighted: usize,
    chosen: Option<usize>,
    random: u32,
    pub intro_complete: bool,
    /// Front-end sound ids (`GlobalFEPlaySound`) the movie played for the last input (nav, A, and B on a
    /// cancelable popup).
    pub sounds: Vec<u64>,
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
            Some(ObjectKind::Native(name)) => name.clone(),
            _ => "global".into(),
        };
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        Ok(match (native.as_str(), method) {
            ("Popup", "GetDescription") => Value::Text(self.description.clone()),
            ("Popup", "GetNumOptions") => Value::Number(self.options.len() as f64),
            ("Popup", "GetOption") => Value::Text(self.options.get(arg(0).number().max(0.) as usize).cloned().unwrap_or_default()),
            ("Popup", "IsPopupCancelable") => Value::Bool(self.cancelable),
            ("ScreenManager", "IntroComplete") => {
                self.intro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", _) => Value::Undefined,
            ("Game", "OnMenuHilight") => {
                self.highlighted = arg(1).number().max(0.) as usize;
                Value::Undefined
            }
            ("Game", "OnMenuSelect") => {
                // B on a cancelable popup reports (0, -1, "CANCEL!"); clamped, it read as the first option.
                let index = arg(1).number();
                self.chosen = Some(if index < 0. { CANCEL } else { index as usize });
                Value::Undefined
            }
            ("Game", "OnMenuGetItemEnabled") => Value::Bool(true),
            ("Game", _) => Value::Undefined,
            _ => return Err(format!("Unimplemented popup binding {native}.{method}")),
        })
    }
}

pub(crate) struct Runtime {
    pub vm: Vm,
    pub bindings: Bindings,
    screen: usize,
    /// C9: authored frames left during which A is ignored ([`Runtime::hold`]).
    held: u32,
}
impl Runtime {
    /// A popup showing `description` (a string ID or "#literal") with `options`.
    pub fn load(json: &serde_json::Value, description: &str, options: &[&str], cancelable: bool) -> Result<Self, String> {
        let mut vm = native_vm(&["Popup"])?;
        define_fe_sounds(&json["actions"], &mut vm)?;
        let mut bindings = Bindings {
            movie: Movie::load(json)?,
            shapes: serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?,
            description: description.into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            cancelable,
            highlighted: 0,
            chosen: None,
            random: 0x9e3779b9,
            intro_complete: false,
            sounds: Vec::new(),
        };
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        let buttons: std::collections::BTreeMap<String, crate::apt_scene::Texture> =
            serde::Deserialize::deserialize(&json["buttons"]).unwrap_or_default();
        bindings.movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        let mut runtime = Self { vm, bindings, screen, held: 0 };
        // Native PopupScreen shows the movie and asks it to lay out its content.
        let root = runtime.bindings.movie.root;
        runtime.vm.set(root, "_visible", Value::Bool(true))?;
        runtime.vm.begin_update();
        runtime.vm.call_method(screen, "SetState", vec![Value::Number(0.)], &mut runtime.bindings)?;
        drain(&mut runtime.vm, &mut runtime.bindings)?;
        Ok(runtime)
    }
    /// New text on the open popup: the movie's own `Refresh` reads Popup.GetDescription again. Refresh also rebuilds
    /// the options on the first one; the highlighted one is walked back to, silently.
    pub fn set_description(&mut self, description: &str) -> Result<(), String> {
        let keep = self.bindings.highlighted;
        self.bindings.description = description.into();
        self.vm.begin_update();
        self.vm.call_method(self.screen, "Refresh", vec![], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        for _ in 0..keep {
            self.input(Key::Down)?;
        }
        self.bindings.sounds.clear();
        Ok(())
    }
    /// C9: a popup another player can raise ignores A for its first `frames` authored frames, so a press meant for
    /// something else (the Enter that sends a chat line, A for a trick) never answers it. B and the d-pad still work.
    pub fn hold(&mut self, frames: u32) {
        self.held = frames;
    }
    pub fn advance(&mut self) -> Result<(), String> {
        self.held = self.held.saturating_sub(1);
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    /// Feed a key; returns the chosen option (or CANCEL) once the player decides.
    pub fn input(&mut self, key: Key) -> Result<Option<usize>, String> {
        if swallowed(self.held, key) {
            return Ok(None);
        }
        self.bindings.chosen = None;
        self.bindings.sounds.clear();
        self.vm.begin_update();
        let args = vec![Value::Number(key.apt().1), Value::Number(0.)];
        self.vm.call_method(self.screen, "UpdateInput", args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)?;
        // The menu picker reports A through OnMenuSelect; fall back to the
        // highlighted row if a control only reports highlights.
        Ok(self.bindings.chosen.or((key == Key::Accept).then_some(self.bindings.highlighted)))
    }
}

fn swallowed(held: u32, key: Key) -> bool {
    held > 0 && key == Key::Accept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_popup_ignores_a_but_not_b_or_the_pad() {
        assert!(swallowed(30, Key::Accept) && swallowed(1, Key::Accept));
        assert!(!swallowed(0, Key::Accept) && !swallowed(30, Key::Back) && !swallowed(30, Key::Down));
    }

    /// Needs the prepared popup (SKATE_POPUP_MANIFEST); CREATE_SKATER_OUT for a PNG.
    #[test]
    #[ignore]
    fn gender_popup_asks_and_answers() {
        let path = std::env::var("SKATE_POPUP_MANIFEST").unwrap();
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut popup = Runtime::load(&json, "ID_CAC_GENDERSWITCH_CONFIRMATION", &["ID_COMMON_YES", "ID_COMMON_NO"], false).unwrap();
        for _ in 0..40 {
            popup.advance().unwrap();
        }
        if let Ok(out) = std::env::var("CREATE_SKATER_OUT") {
            let root = std::path::Path::new(&path).parent().unwrap().parent().unwrap().to_path_buf();
            let draws = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes).unwrap();
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("popup.png"));
        }
        assert_eq!(popup.input(Key::Down).unwrap(), None);
        assert_eq!(popup.bindings.highlighted, 1, "Down highlights No");
        use crate::core_menu_runtime::{SOUND_A_BUTTON, SOUND_B_BUTTON, SOUND_NAV_DOWN};
        assert_eq!(popup.bindings.sounds, [SOUND_NAV_DOWN]);
        for _ in 0..10 {
            popup.advance().unwrap();
        }
        if let Ok(out) = std::env::var("CREATE_SKATER_OUT") {
            let root = std::path::Path::new(&path).parent().unwrap().parent().unwrap().to_path_buf();
            let draws = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes).unwrap();
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("popup-down.png"));
        }
        assert_eq!(popup.input(Key::Accept).unwrap(), Some(1), "No is the second option");
        assert_eq!(popup.bindings.sounds, [SOUND_A_BUTTON]);
        assert_eq!(popup.input(Key::Back).unwrap(), None, "the gender popup is not cancelable");
        assert!(popup.bindings.sounds.is_empty(), "an ignored B is silent");
        let mut exit = Runtime::load(&json, "ID_CAC_EXIT_CAC_CONFIRMATION_NO_SAVE", &["ID_COMMON_SAVE_CONTINUE", "ID_COMMON_CONTINUE_NO_SAVE"], true).unwrap();
        for _ in 0..40 {
            exit.advance().unwrap();
        }
        assert_eq!(exit.input(Key::Back).unwrap(), Some(CANCEL));
        assert_eq!(exit.bindings.sounds, [SOUND_B_BUTTON], "the movie plays B itself on a cancelable popup");
    }
}
