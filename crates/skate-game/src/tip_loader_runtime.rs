//! Skate 3's loading screen (main/tiploader) on the shared APT runtime. The movie
//! asks the native `TipsLoader` which tip panels are enabled and what they say,
//! and is driven through tiploader.SetState(n) as the shipped native screen does.
//! Tips cycle: Populate shows one panel, Outro plays it off, the next one plays on.
//! The corner loading wheel is the shared busy tag (controls/autosave), its own
//! movie drawn over the screen. Bevy-independent: tip_loader.rs shows it while a
//! map loads.
use crate::{
    apt_movie::Movie,
    apt_scene::{Draw, Shapes},
    apt_vm::{Host, ObjectKind, Value, Vm},
    core_menu_runtime::{array, boot_movie, boot_screen, common_call, drain, native_vm, screen_property_changed, ScreenHost},
};

/// tiploader.SetState values (kState_*).
const POPULATE: f64 = 1.;
const OUTRO: f64 = 2.;
/// Retail tip pools (language table): Coach Frank Says, and Did you know? (title, text).
const COACH_FRANK: usize = 20;
const HINTS: usize = 31;
/// Authored frames a tip stays up, and the panel's 'off' animation (frames 15-29).
const TIP_FRAMES: u32 = 8 * 60;
const OFF_FRAMES: u32 = 15;
/// _global.GetPlatform() for the busy tag: Platform_XBOX360 selects the 360 wheel, the TU3 target (boot_screens.rs
/// answers the same).
const PLATFORM_XBOX360: f64 = 1.;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tip {
    CoachFrank(String),
    /// (title, text) string IDs.
    Hint(String, String),
}

/// Every retail tip, button-glyph markup (`<B:L_Bumper_hud>`) included: apt_scene draws it inline. A tip naming a
/// glyph the screen lacks (`has_glyph`) is left out rather than shown as raw markup.
pub(crate) fn tips(language: impl Fn(&str) -> Option<String>, has_glyph: impl Fn(&str) -> bool) -> Vec<Tip> {
    let plain = |id: &str| {
        language(id).is_some_and(|t| t.split("<B:").skip(1).all(|tag| tag.split_once('>').is_some_and(|(name, _)| has_glyph(name))))
    };
    let frank = (1..=COACH_FRANK).map(|n| format!("ID_TIPS_COACHFRANK_{n}")).filter(|id| plain(id)).map(Tip::CoachFrank);
    let hints = (1..=HINTS)
        .map(|n| (format!("ID_HINTS_TITLE_{n}"), format!("ID_HINTS_TEXT_{n}")))
        .filter(|(title, text)| plain(title) && plain(text))
        .map(|(title, text)| Tip::Hint(title, text));
    frank.chain(hints).collect()
}

pub(crate) struct Bindings {
    pub movie: Movie,
    pub shapes: Shapes,
    tips: Vec<Tip>,
    /// Shuffled order of `tips`, and the position of the tip on screen.
    order: Vec<usize>,
    shown: usize,
    /// The panel is on screen (Populate) rather than playing off (Outro).
    populated: bool,
    random: u32,
    pub intro_complete: bool,
    pub outro_complete: bool,
}
impl Bindings {
    fn tip(&self) -> Option<&Tip> {
        self.tips.get(*self.order.get(self.shown)?)
    }
    /// Language text as a literal: the movie does not localize these fields, and
    /// '^' is the string table's line break.
    fn text(&self, id: &str) -> Value {
        Value::Text(format!("#{}", self.movie.text_assets.localize(id).replace('^', "\n")))
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
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(ObjectKind::Native(name)) => name.clone(),
            _ => "global".into(),
        };
        Ok(match (native.as_str(), method) {
            ("TipsLoader", "IsCoachFrankEnabled") => Value::Bool(matches!(self.tip(), Some(Tip::CoachFrank(_)))),
            ("TipsLoader", "IsHintEnabled") => Value::Bool(matches!(self.tip(), Some(Tip::Hint(..)))),
            // ponytail: no career stats, leaderboards, «Feed or «Reel in this engine.
            ("TipsLoader", "IsStatsEnabled" | "IsLeaderboardEnabled" | "IsFeedEnabled" | "IsReelEnabled") => Value::Bool(false),
            ("TipsLoader", "GetCoachFrankText") => match self.tip() {
                Some(Tip::CoachFrank(id)) => self.text(&id.clone()),
                _ => Value::Text("#".into()),
            },
            // [hintTitle, hintText]
            ("TipsLoader", "GetHint") => {
                let (title, text) = match self.tip() {
                    Some(Tip::Hint(title, text)) => (title.clone(), text.clone()),
                    _ => ("#".into(), "#".into()),
                };
                let (title, text) = (self.text(&title), self.text(&text));
                array(vm, [title, text])?
            }
            ("ScreenManager", "IntroComplete") => {
                self.intro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", "OutroComplete") => {
                self.outro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", _) | ("Game", _) => Value::Undefined,
            _ => return Err(format!("Unimplemented loading screen binding {native}.{method}")),
        })
    }
}

/// The busy tag movie (controls/autosave): `loadtag.Load_EnableTag()` spins its
/// wheel in the bottom-right corner.
pub(crate) struct Wheel {
    pub movie: Movie,
    pub shapes: Shapes,
    random: u32,
}
impl ScreenHost for Wheel {
    fn movie(&mut self) -> &mut Movie {
        &mut self.movie
    }
    fn random(&mut self) -> &mut u32 {
        &mut self.random
    }
}
impl Host for Wheel {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        screen_property_changed(&mut self.movie, vm, object, key)
    }
    fn call(&mut self, vm: &mut Vm, object: usize, method: &str, args: Vec<Value>) -> Result<Value, String> {
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        match method {
            "GetPlatform" => Ok(Value::Number(PLATFORM_XBOX360)),
            _ => Err(format!("Unimplemented busy tag binding {method}")),
        }
    }
}
impl Wheel {
    pub(crate) fn load(json: &serde_json::Value) -> Result<(Vm, Self), String> {
        let mut vm = native_vm(&[])?;
        vm.set(vm.global, "Platform_XBOX360", Value::Number(PLATFORM_XBOX360))?;
        let shapes = serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?;
        let mut wheel = Self { movie: Movie::load(json)?, shapes, random: 1 };
        boot_movie(json, &mut vm, &mut wheel)?;
        // The constructor publishes Load_EnableTag/Load_HideTag on _global.
        if !matches!(vm.get(wheel.movie.root, "loadtag"), Value::Object(_)) {
            return Err("Busy tag controller was not constructed".into());
        }
        vm.begin_update();
        vm.call_method(vm.global, "Load_EnableTag", vec![Value::Bool(false)], &mut wheel)?;
        drain(&mut vm, &mut wheel)?;
        Ok((vm, wheel))
    }
}

pub(crate) struct Runtime {
    pub vm: Vm,
    pub bindings: Bindings,
    screen: usize,
    /// Authored frames since the current panel started playing on or off.
    frames: u32,
    closing: bool,
    wheel: Option<(Vm, Wheel)>,
}
impl Runtime {
    /// `seed` picks where the shuffled tips start, so each load opens on a new one.
    /// `wheel`: the busy tag manifest; None draws no corner wheel.
    pub fn load(json: &serde_json::Value, wheel: Option<&serde_json::Value>, seed: u32) -> Result<Self, String> {
        let mut movie = Movie::load(json)?;
        // The controller glyphs the tips' <B:...> tags name (prepare_tip_loader.py, else tip_loader.rs lends them).
        let buttons: std::collections::BTreeMap<String, crate::apt_scene::Texture> =
            serde::Deserialize::deserialize(&json["buttons"]).unwrap_or_default();
        movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        let tips = tips(|id| movie.text_assets.language.get(id).cloned(), |name| crate::apt_scene::tag_glyph(&movie, name).is_some());
        if tips.is_empty() {
            return Err("Loading screen has no tips".into());
        }
        let mut random = seed | 1;
        let mut order: Vec<usize> = (0..tips.len()).collect();
        // Fisher-Yates with the screen's xorshift.
        for i in (1..order.len()).rev() {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            order.swap(i, random as usize % (i + 1));
        }
        let mut vm = native_vm(&["TipsLoader"])?;
        let mut bindings = Bindings {
            movie,
            shapes: serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?,
            tips,
            order,
            shown: 0,
            populated: false,
            random,
            intro_complete: false,
            outro_complete: false,
        };
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        // The first tip plays on as soon as the intro is in.
        let wheel = wheel.map(Wheel::load).transpose()?;
        Ok(Self { vm, bindings, screen, frames: OFF_FRAMES, closing: false, wheel })
    }
    fn state(&mut self, state: f64) -> Result<(), String> {
        self.vm.begin_update();
        self.vm.call_method(self.screen, "SetState", vec![Value::Number(state)], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// Play the screen off; OutroComplete follows.
    pub fn close(&mut self) -> Result<(), String> {
        self.closing = true;
        // The wheel stops with the load; Load_HideTag hides it at once.
        if let Some((vm, wheel)) = self.wheel.as_mut() {
            vm.begin_update();
            vm.call_method(vm.global, "Load_HideTag", Vec::new(), wheel)?;
            drain(vm, wheel)?;
        }
        self.state(OUTRO)?;
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text("outro".into())], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// One authored frame (60 fps timeline), cycling tips once the intro is in.
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)?;
        if let Some((vm, wheel)) = self.wheel.as_mut() {
            vm.begin_update();
            wheel.movie.advance(vm)?;
            drain(vm, wheel)?;
            let roots: Vec<usize> = wheel.movie.instances.keys().copied().chain(wheel.movie.classes.values().copied()).collect();
            vm.collect(roots)?;
        }
        if self.closing || !self.bindings.intro_complete {
            return Ok(());
        }
        self.frames += 1;
        if !self.bindings.populated && self.frames >= OFF_FRAMES {
            self.bindings.populated = true;
            self.frames = 0;
            self.state(POPULATE)?;
        } else if self.bindings.populated && self.frames >= TIP_FRAMES {
            self.bindings.populated = false;
            self.bindings.shown = (self.bindings.shown + 1) % self.bindings.order.len();
            self.frames = 0;
            self.state(OUTRO)?;
        }
        Ok(())
    }
    pub fn draws(&self) -> Result<Vec<Draw>, String> {
        let mut draws = crate::apt_scene::draw(&self.bindings.movie, &self.vm, &self.bindings.shapes)?;
        if let Some((vm, wheel)) = &self.wheel {
            draws.extend(crate::apt_scene::draw(&wheel.movie, vm, &wheel.shapes)?);
        }
        Ok(draws)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tips_keep_button_markup_and_skip_unpaired_hints() {
        let language = |id: &str| match id {
            "ID_TIPS_COACHFRANK_1" => Some("Call a skater.".to_string()),
            "ID_TIPS_COACHFRANK_2" => Some("Press <B:DPad_Down_hud>.".to_string()),
            "ID_TIPS_COACHFRANK_3" => Some("Press <B:Z_hud>.".to_string()),
            "ID_HINTS_TITLE_1" => Some("Stats".to_string()),
            "ID_HINTS_TEXT_1" => Some("Profile is home to your stats.".to_string()),
            "ID_HINTS_TITLE_2" => Some("No text".to_string()),
            _ => None,
        };
        let has = |name: &str| name.starts_with("DPad");
        assert_eq!(
            tips(language, has),
            [
                Tip::CoachFrank("ID_TIPS_COACHFRANK_1".into()),
                Tip::CoachFrank("ID_TIPS_COACHFRANK_2".into()),
                Tip::Hint("ID_HINTS_TITLE_1".into(), "ID_HINTS_TEXT_1".into())
            ],
            "F17: glyph tips show; one naming a missing glyph doesn't"
        );
    }

    /// Visual check against the owned, prepared screen (SKATE_TIP_LOADER_ROOT =
    /// private/tip-loader; PNGs in SKATE_CORE_MENU_OUT).
    #[test]
    #[ignore = "requires the prepared original loading screen"]
    fn loading_screen_cycles_tips() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_TIP_LOADER_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/tiploader.json")).unwrap()).unwrap();
        let wheel: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/autosave.json")).unwrap()).unwrap();
        let mut rt = Runtime::load(&json, Some(&wheel), 7).unwrap();
        let wheel_draws = |rt: &Runtime| {
            let (vm, w) = rt.wheel.as_ref().unwrap();
            crate::apt_scene::draw(&w.movie, vm, &w.shapes).unwrap().len()
        };
        let run = |rt: &mut Runtime, frames: u32| (0..frames).for_each(|_| rt.advance().unwrap());
        let snap = |rt: &Runtime, name: &str| {
            crate::core_menu_runtime::tests::raster_draws(&rt.draws().unwrap(), &root, &out.join(format!("{name}.png")));
        };
        run(&mut rt, 12);
        assert!(rt.bindings.intro_complete, "intro never reported completion");
        run(&mut rt, 60);
        snap(&rt, "tip-loader-0");
        assert!(wheel_draws(&rt) > 0, "the corner loading wheel is drawn");
        let first = rt.bindings.tip().cloned();
        run(&mut rt, TIP_FRAMES + OFF_FRAMES + 60);
        snap(&rt, "tip-loader-1");
        assert_ne!(rt.bindings.tip().cloned(), first, "the next tip is showing");
        run(&mut rt, TIP_FRAMES + OFF_FRAMES + 60);
        snap(&rt, "tip-loader-2");
        rt.close().unwrap();
        assert_eq!(wheel_draws(&rt), 0, "the wheel hides with the load");
        run(&mut rt, 20);
        assert!(rt.bindings.outro_complete, "outro never reported completion");
    }
}
