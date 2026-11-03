//! Native HUD bindings and the original ActionScript-driven movie lifecycle.
use crate::{
    apt_movie::Movie,
    apt_vm::{Host, ObjectKind, Value, Vm},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub sequence_score: i32,
    pub line_score: i32,
    pub sequence_timer: i32,
    pub line_time: f32,
    pub line_capacity: f32,
    /// Points on the line timer, and the points at which the multiplier steps to x1.5, x2 and x3.
    pub line_points: f32,
    pub line_thresholds: [f32; 3],
    pub multiplier: f32,
    pub clean: bool,
    pub sketchy: bool,
    pub stance: [bool; 4],
    pub trick_name: String,
    pub trick_metrics: [Value; 5],
    pub context_tricks: Vec<Value>,
}
pub struct Bindings {
    pub movie: Movie,
    pub input: Input,
    random: u32,
    /// `Audio.PlaySound` keys (the `fe` record names, from `_global.FE_SOUND_<KEY>`) since the host last drained them.
    /// Only Skate 2's movies play any; its HUD host drains them.
    pub sounds: Vec<String>,
    /// Skate 1's new personal bests (title label, value text) for `Stats.GetNewPersonalBests`, which empties it.
    pub personal_bests: Vec<(String, String)>,
}
pub struct Runtime {
    pub vm: Vm,
    pub bindings: Bindings,
    pub controller: usize,
    /// Skate 2's trick-display component flag +66 (0x823bd240): the line timer was full last frame, so state 11 is
    /// sent again this frame.
    line_full: bool,
}

fn array(vm: &mut Vm, values: impl IntoIterator<Item = Value>) -> Result<Value, String> {
    let object = vm.object(ObjectKind::Plain);
    let mut n = 0;
    for value in values {
        vm.set(object, n.to_string(), value)?;
        n += 1;
    }
    vm.set(object, "length", Value::Number(n as f64))?;
    Ok(Value::Object(object))
}
pub(crate) fn score(value: i32) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    if value < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
/// Settings > Units (Skate 3's settings+168, Skate 2's +144, Skate 1's +18904: 0 metric, 1 imperial), published by
/// graphics_menu for the HUD's distance and speed text.
pub static IMPERIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn imperial() -> bool {
    IMPERIAL.load(std::sync::atomic::Ordering::Relaxed)
}
/// The exe's number formatter (Skate 3 0x8290E370; Skate 2's 0x827ACB90 is the same code): digits by powers of ten
/// with ',' every three, `places` decimals, then half up on what is left, carried back through the digits.
pub fn decimal(value: f32, places: u32) -> String {
    let digit = |t: f64| (t as i32) as i8; // fctiwz, low byte
    let mut v = value.abs() as f64;
    let mut buf = vec![b'0']; // the carry's spare digit
    let (mut unit, mut n) = (1f64, 0);
    if v > 1. {
        (unit, n) = (10., 1);
        while unit < v {
            unit *= 10.;
            n += 1;
        }
    }
    if unit > v {
        unit *= 0.1;
        n -= 1;
    }
    let (ints, mut left) = (n + 1, n + 1);
    while unit >= 1. {
        if left % 3 == 0 && left != ints {
            buf.push(b',');
        }
        left -= 1;
        let d = digit(v / unit);
        buf.push(b'0'.wrapping_add_signed(d));
        v = (-(d as f64)).mul_add(unit, v); // fnmsub
        unit *= 0.1;
    }
    if buf.len() == 1 {
        buf.push(b'0');
    }
    if places > 0 {
        buf.push(b'.');
        for _ in 0..places {
            let t = v * 10.;
            let d = digit(t);
            buf.push(b'0'.wrapping_add_signed(d));
            v = t - d as f64;
        }
    }
    let mut start = 1;
    if v * 10. >= 5. {
        let mut i = buf.len();
        loop {
            i -= 1;
            match buf[i] {
                b'9' => buf[i] = b'0',
                b'.' | b',' => {}
                c => {
                    buf[i] = c.wrapping_add(1);
                    break;
                }
            }
        }
        start = usize::from(i > 0);
    }
    let sign = if value < 0. { "-" } else { "" };
    sign.chars().chain(buf[start..].iter().map(|&b| b as char)).collect()
}
/// {0:distance} (Skate 3 0x824FE848, Skate 2 0x8246F4F0): metres, or x3.0 feet, "%s %s" with the unit's text.
pub fn distance(metres: f32, places: u32, imperial: bool, text: impl Fn(&str) -> String) -> String {
    let (value, unit) = if imperial { (metres * 3., "ID_COMMON_METRIC_NEW_FEET") } else { (metres, "ID_COMMON_METRIC_DISTANCE") };
    format!("{} {}", decimal(value, places), text(unit))
}
/// {0:speed} (Skate 3 0x824FE968, Skate 2 0x8246F620): m/s as x3.6 Km/h or x2.25 MPH.
pub fn speed(mps: f32, places: u32, imperial: bool, text: impl Fn(&str) -> String) -> String {
    let (value, unit) = if imperial { (mps * 2.25, "ID_COMMON_MPH") } else { (mps * 3.6, "ID_COMMON_KMPH") };
    format!("{} {}", decimal(value, places), text(unit))
}
/// Skate 2's formatters read their argument back from text (0x829F63E0): the number up to the first ','.
pub fn skate2_number(text: &str) -> f32 {
    text.split(',').next().and_then(|t| t.parse().ok()).unwrap_or(0.)
}
/// Skate 1's multiplier label: 1 -> "x1", 1.5 -> "x1.5".
fn multiplier_label(multiplier: f32) -> String {
    format!("x{multiplier}")
}
/// Skate 2's localized format strings: `{N}` or `{N:kind}` takes the Nth argument.
fn format_text(format: &str, args: &[Value]) -> String {
    let mut out = String::new();
    let mut rest = format;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else { break };
        out.push_str(&rest[..start]);
        let field = &rest[start + 1..start + end];
        match field.split(':').next().and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => out.push_str(&args.get(n).map(Value::text).unwrap_or_default()),
            None => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out + rest
}
impl Host for Bindings {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        if key == "text" || key == "autoSize" {
            self.movie.text_changed(vm, object)?;
        }
        Ok(())
    }
    fn call(
        &mut self,
        vm: &mut Vm,
        object: usize,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        if self.movie.method(vm, object, method, &args)? {
            return Ok(Value::Undefined);
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(ObjectKind::Native(name)) => name.as_str(),
            _ => "global",
        };
        match (native, method) {
            ("Math", "floor") => Ok(Value::Number(
                args.first().ok_or("Math.floor argument")?.number().floor(),
            )),
            ("Math", "random") => {
                // Presentation-only random source; never influences scoring.
                self.random ^= self.random << 13;
                self.random ^= self.random >> 17;
                self.random ^= self.random << 5;
                Ok(Value::Number(self.random as f64 / 4294967296.0))
            }
            // The HUD never enumerates prototype properties. Marking their
            // enumeration flags therefore leaves its observable fields intact.
            ("global", "ASSetPropFlags") => Ok(Value::Undefined),
            ("FELanguage", "GetLanguage") => Ok(Value::Text("english".into())),
            // Only Skate 1 calls this; its UpdateMultiplier matches the labels 'x1', 'x1.5', 'x2', 'x3'.
            ("Tricks", "GetSequenceMultiplier") => Ok(Value::Text(multiplier_label(self.input.multiplier))),
            // Skate 1's trick display asks for the timer's points and the multiplier step points itself.
            ("Tricks", "GetLineTimerPoints") => Ok(Value::Number(self.input.line_points as f64)),
            ("Tricks", "GetLineTimerFirstThreshold") => Ok(Value::Number(self.input.line_thresholds[0] as f64)),
            ("Tricks", "GetLineTimerSecondThreshold") => Ok(Value::Number(self.input.line_thresholds[1] as f64)),
            ("Tricks", "GetLineTimerThirdThreshold") => Ok(Value::Number(self.input.line_thresholds[2] as f64)),
            // ponytail: Skate 1's momentum meter has no Skate 3 counterpart; it reads as an idle meter (max 1 so it never divides by zero).
            ("Tricks", "GetMomentumScore") => Ok(Value::Number(0.)),
            ("Tricks", "GetMomentumMaxPoints") => Ok(Value::Number(1.)),
            ("Tricks", "GetLineTimerMaxPoints") => {
                Ok(Value::Number(self.input.line_capacity as f64))
            }
            //825C2910 truncates manager+32 to integer;825C28F8 returns+192.
            ("Tricks", "GetLineTimeRemaining") => {
                Ok(Value::Number(self.input.line_time.trunc() as f64))
            }
            ("Tricks", "GetLineScore") => Ok(Value::Number(self.input.line_score as f64)),
            ("HUDComponents", "TrickDisplay_GetSequenceMultiplier") => {
                Ok(Value::Number(self.input.multiplier as f64))
            }
            ("Tricks", "GetGeneralInfo") => array(
                vm,
                [
                    Value::Number(self.input.sequence_score as f64),
                    Value::Text(score(self.input.sequence_score)),
                    // 825C2D90 exposes backend152/153; the shipped
                    // UpdateTrickScoring reads [2] as sketchy, [3] as clean.
                    Value::Bool(self.input.sketchy),
                    Value::Bool(self.input.clean),
                    Value::Number(self.input.sequence_timer as f64),
                    Value::Number(self.input.line_score as f64),
                    Value::Text(score(self.input.line_score)),
                ],
            ),
            ("Tricks", "GetCurrentTrickStance") => array(vm, self.input.stance.map(Value::Bool)),
            ("Tricks", "GetCurrentTrickName") => Ok(Value::Text(self.input.trick_name.clone())),
            ("Tricks", "GetCurrentTrickMetrics") => array(vm, self.input.trick_metrics.clone()),
            ("Tricks", "TrickDisplay_GetAllTrickData") => {
                array(vm, self.input.context_tricks.clone())
            }
            // Skate 2's trickdisplay2 (the Skate 2 maps' HUD) asks for scores and text preformatted.
            ("Tricks", "GetCurrentSequenceScore") => Ok(Value::Number(self.input.sequence_score as f64)),
            ("Tricks", "GetFormattedCurrentSequenceScore") => Ok(Value::Text(score(self.input.sequence_score))),
            ("Tricks", "GetFormatLineScore") => Ok(Value::Text(score(self.input.line_score))),
            ("Tricks", "IsSketchyLanding") => Ok(Value::Bool(self.input.sketchy)),
            // Skate 2's (0x82582A78): the metres to one place as text, through ID_BIGTRICK_DISTANCE_FORMAT's {0:distance}
            // (which reads it back, then Settings > Units), behind the localizer's '#' (0x82d99d10) that
            // UpdateBigTrickDisplay strips.
            ("HUDComponents", "TrickDisplay_GetFormatDistance") => {
                let text = &self.movie.text_assets;
                let metres = skate2_number(&decimal(args.first().map_or(0., Value::number) as f32, 1));
                let shown = distance(metres, 1, imperial(), |id| text.localize(id));
                Ok(Value::Text(format!("#{}", format_text(&text.localize("ID_BIGTRICK_DISTANCE_FORMAT"), &[Value::Text(shown)]))))
            }
            // Skate 1's UpdatePersonalBests: [count, title, value, ...] of the bests set since it last asked.
            ("Stats", "GetNewPersonalBests") => {
                let bests = std::mem::take(&mut self.personal_bests);
                let count = Value::Number(bests.len() as f64);
                array(vm, std::iter::once(count).chain(bests.into_iter().flat_map(|(t, v)| [Value::Text(t), Value::Text(v)])))
            }
            // ponytail: Skate 1's personal-best stingers (audio events 360/361, 0x824491e0/0x82449210); its sound bank isn't ported.
            ("Audio", "PlayPersBestFinalIntroSound" | "PlayPersBestFinalOutroSound") => Ok(Value::Undefined),
            ("FELanguage", "GetLocaleText") => {
                Ok(Value::Text(self.movie.text_assets.localize(&args.first().ok_or("GetLocaleText label")?.text())))
            }
            ("FELanguage", "GetLocalizedFormatString" | "GetLocalizedFormatStringCustom") => {
                let label = args.first().ok_or("GetLocalizedFormatString label")?.text();
                Ok(Value::Text(format_text(&self.movie.text_assets.localize(&label), &args[1..])))
            }
            // trickdisplay2's multiplier steps, hudcentral's message hooks.
            ("Audio", "PlaySound") => {
                if let Some(Value::Text(key)) = args.first() {
                    self.sounds.push(key.clone());
                }
                Ok(Value::Undefined)
            }
            _ => Err(format!(
                "Unimplemented original HUD binding {native}.{method}"
            )),
        }
    }
}
impl Runtime {
    pub fn load(json: &serde_json::Value, input: Input) -> Result<Self, String> {
        let mut vm = Vm::new();
        for name in ["MovieClip", "Object", "Tricks", "Stats", "FELanguage", "HUDComponents", "Math", "Audio"] {
            let object = vm.object(ObjectKind::Native(name.into()));
            let prototype = vm.object(ObjectKind::Plain);
            vm.set(object, "prototype", Value::Object(prototype))?;
            vm.set(vm.global, name, Value::Object(object))?;
        }
        vm.set(vm.global, "Screen_EdgeOffset", Value::Number(0.0))?;
        // The game defines `_global.FE_SOUND_<KEY>` as the `fe` key; PlaySound hands it back. core_menu_runtime's
        // define_fe_sounds, repeated: this file also builds alone (skate-data's scoring_flow_data example).
        crate::apt_movie::fe_sound_names(json, &mut |name| {
            vm.set(vm.global, name, Value::Text(name["FE_SOUND_".len()..].to_ascii_lowercase()))
        })?;
        // Globals the game sets before the movie runs (Skate 2's score threshold enums, "%d" text: 0x8250dd78).
        for (name, value) in json["globals"].as_object().into_iter().flatten() {
            vm.set(vm.global, name, Value::Text(value.as_str().unwrap_or_default().into()))?;
        }
        let mut bindings = Bindings {
            movie: Movie::load(json)?,
            input,
            random: 0x9e3779b9,
            sounds: Vec::new(),
            personal_bests: Vec::new(),
        };
        let initial: Vec<_> = bindings
            .movie
            .characters
            .values()
            .flat_map(|c| &c.frames)
            .flat_map(|f| &f.controls)
            .filter(|c| c.type_name == "do_init_action")
            .map(|c| c.actions_offset)
            .collect();
        for offset in initial {
            let code = bindings.movie.actions[&offset.to_string()].clone();
            vm.run(&code, &mut bindings)?;
        }
        vm.begin_update();
        bindings.movie.initialize(&mut vm)?;
        let mut runtime = Self {
            vm,
            bindings,
            controller: usize::MAX,
            line_full: false,
        };
        runtime.drain()?;
        let Value::Object(controller) = runtime.vm.get(runtime.bindings.movie.root, "screen")
        else {
            return Err("Original HUD controller was not constructed".into());
        };
        runtime.controller = controller;
        runtime.show()?;
        Ok(runtime)
    }
    /// Open the screen: trickdisplay2 with ScreenShow, Skate 2's hudcentral and Skate 1's trickdisplay with Show.
    /// Skate 1's hides itself on a bail (its "bail" label) until it is shown again.
    pub fn show(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let controller = self.controller;
        let show = ["ScreenShow", "Show"].into_iter().find(|m| !matches!(self.vm.get(controller, m), Value::Undefined));
        if let Some(show) = show {
            self.vm.call_method(controller, show, vec![Value::Bool(true)], &mut self.bindings)?;
        }
        self.drain()
    }
    fn drain(&mut self) -> Result<(), String> {
        let mut calls = 0;
        while let Some((object, offset)) = self.bindings.movie.pending.pop_front() {
            calls += 1;
            if calls > 4096 {
                return Err("HUD frame action limit".into());
            }
            if !self.bindings.movie.instances.contains_key(&object) {
                continue;
            }
            let code = self
                .bindings
                .movie
                .actions
                .get(&offset.to_string())
                .ok_or("Missing HUD action block")?
                .clone();
            self.vm.run_on(object, &code, &mut self.bindings)?;
        }
        Ok(())
    }
    pub fn update(
        &mut self,
        input: Input,
        new_trick: bool,
        modified_trick: bool,
        close_tricks: bool,
    ) -> Result<(), String> {
        self.vm.begin_update();
        let previous = self.bindings.input.clone();
        self.bindings.input = input;
        let mut methods: Vec<(&str, Vec<Value>)> = Vec::new();
        if previous.multiplier != self.bindings.input.multiplier {
            // Skate 1's UpdateMultiplier(mult) takes the label; the Skate 2/3 movies ignore the argument.
            methods.push(("UpdateMultiplier", vec![Value::Text(multiplier_label(self.bindings.input.multiplier))]));
        }
        if previous.sequence_timer != self.bindings.input.sequence_timer
            || previous.sequence_score != self.bindings.input.sequence_score
            || previous.clean != self.bindings.input.clean
            || previous.sketchy != self.bindings.input.sketchy
            || previous.line_score != self.bindings.input.line_score
        {
            methods.push(("UpdateTrickScoring", vec![]));
        }
        if previous.stance != self.bindings.input.stance {
            methods.push(("UpdateStance", vec![]));
        }
        if new_trick {
            methods.push(("RefreshTrickName", vec![]));
        } else if modified_trick {
            methods.push(("CurrentTrickModified", vec![]));
        }
        // Skate 1's controller is handed each trick (queued when new, renamed in place when modified); its
        // onEnterFrame shows the queue every tenth frame.
        if new_trick || modified_trick {
            let name = Value::Text(self.bindings.input.trick_name.clone());
            methods.push(("UpdateTricks", vec![name, Value::Bool(new_trick)]));
        }
        if close_tricks {
            methods.push(("CloseTrickDisplay", vec![]));
            // Skate 1's landing: bail / sketchy from GetCurrentTrickMetrics, then its trick queue resets.
            methods.push(("UpdateDisplay", vec![]));
        }
        for (method, arguments) in methods {
            // Skate 1's script polls the natives every frame and defines only some of these entry points.
            if matches!(self.vm.get(self.controller, method), Value::Undefined) {
                continue;
            }
            self.vm
                .call_method(self.controller, method, arguments, &mut self.bindings)?;
            self.drain()?;
        }
        // The movie handles this as event 11 (0196CC), not onEnterFrame.
        // Re-seeking the 501-frame multiTimer_mc on every tick pins it to
        // the same whole-second sample instead of letting gotoAndPlay run.
        if previous.line_time.trunc() != self.bindings.input.line_time.trunc()
            || previous.line_score != self.bindings.input.line_score
        {
            if !matches!(self.vm.get(self.controller, "UpdateLineDisplay"), Value::Undefined) {
                self.vm.call_method(
                    self.controller,
                    "UpdateLineDisplay",
                    vec![],
                    &mut self.bindings,
                )?;
                self.drain()?;
            }
        }
        self.step()
    }
    /// Skate 2's trick display, driven the way its exe drives trickdisplay2. The component update 0x823bd240 sends
    /// SetState 10 (UpdateMultiplier) when the multiplier changes, then 11 (UpdateLineDisplay) when the line score
    /// changes or the line timer was full last frame, then 7 (UpdateTrickScoring + UpdateStance) when the sequence
    /// score or the stance changes. The display controller 0x823d42c8 then sends RefreshTrickName (hook 1) or the
    /// rename (hook 2), and CloseTrickDisplay (hook 3) on a bail. The line draining doesn't re-send 7. The generic
    /// [`Runtime::update`] did that once a second, which replayed the score's dim-to-bright 'set' pulse.
    pub fn update_skate2(
        &mut self,
        input: Input,
        new_trick: bool,
        modified_trick: bool,
        close_tricks: bool,
    ) -> Result<(), String> {
        self.vm.begin_update();
        let previous = std::mem::replace(&mut self.bindings.input, input);
        let now = &self.bindings.input;
        let line_changed = previous.line_score != now.line_score;
        let mut methods: Vec<(&str, Vec<Value>)> = Vec::new();
        if previous.multiplier != now.multiplier {
            methods.push(("SetState", vec![Value::Number(10.)]));
        }
        if line_changed || self.line_full {
            methods.push(("SetState", vec![Value::Number(11.)]));
        }
        // ponytail: the exe also sends 7 when score-module+64 changes (a float of unknown meaning).
        // The line score stands in for it here, so the line total text can't go stale when a banked
        // sequence happens to equal the live total.
        if previous.sequence_score != now.sequence_score || line_changed || previous.stance != now.stance {
            methods.push(("SetState", vec![Value::Number(7.)]));
        }
        self.line_full = now.line_points >= now.line_capacity;
        if new_trick {
            methods.push(("RefreshTrickName", vec![]));
        } else if modified_trick {
            methods.push(("CurrentTrickModified", vec![]));
        }
        if close_tricks {
            methods.push(("CloseTrickDisplay", vec![]));
        }
        for (method, arguments) in methods {
            self.vm.call_method(self.controller, method, arguments, &mut self.bindings)?;
            self.drain()?;
        }
        self.step()
    }
    /// A sequence was banked without a bail. Skate 1's ActivateTrickScoring shows the score and line bar and
    /// starts its end-of-sequence count: 90 frames later its Update clears the trick names, unless another
    /// trick (UpdateTricks) comes first. The other movies do not define it.
    pub fn landed(&mut self) -> Result<(), String> {
        self.call("ActivateTrickScoring")
    }
    /// One of the game's own calls into the screen (Skate 2's table at 0x82d6dc88, Skate 1's at 0x8309e598), e.g.
    /// ContextTrickDone or UpdatePersonalBests; nothing where the movie doesn't define it.
    pub fn call(&mut self, method: &str) -> Result<(), String> {
        if matches!(self.vm.get(self.controller, method), Value::Undefined) {
            return Ok(());
        }
        self.vm.begin_update();
        self.vm.call_method(self.controller, method, vec![], &mut self.bindings)?;
        self.drain()
    }
    /// One frame of a screen driven by calls rather than trick input (Skate 2's district card).
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.step()
    }
    fn step(&mut self) -> Result<(), String> {
        self.bindings.movie.advance(&mut self.vm)?;
        self.drain()?;
        // Skate 1's trickdisplay runs its trick queue from mScreen.onEnterFrame (set by Show).
        let entering: Vec<usize> = (self.bindings.movie.instances.keys().copied())
            .filter(|&id| matches!(self.vm.get(id, "onEnterFrame"), Value::Object(_)))
            .collect();
        for id in entering {
            self.vm.call_method(id, "onEnterFrame", vec![], &mut self.bindings)?;
            self.drain()?;
        }
        self.vm
            .collect(self.bindings.movie.instances.keys().copied())?;
        Ok(())
    }
    /// Skate 2's hudcentral title card: ShowDistrictMessage reads `_global.districtzone`.
    pub fn show_district(&mut self, title: &str) -> Result<(), String> {
        self.vm.begin_update();
        let global = self.vm.global;
        self.vm.set(global, "districtzone", Value::Text(title.into()))?;
        self.vm
            .call_method(self.controller, "ShowDistrictMessage", vec![], &mut self.bindings)?;
        self.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(trick: &str, sequence: i32, sketchy: bool) -> Input {
        Input {
            sequence_score: sequence,
            line_score: 12_345,
            sequence_timer: 3,
            line_time: 3.5,
            line_capacity: 10_000.,
            line_points: 1_250.,
            line_thresholds: [50., 450., 800.],
            multiplier: 1.5,
            clean: !sketchy,
            sketchy,
            stance: [false, true, false, false],
            trick_name: trick.into(),
            trick_metrics: [Value::Text(trick.into()), Value::Bool(false), Value::Bool(false), Value::Bool(false), Value::Bool(false)],
            context_tricks: Vec::new(),
        }
    }

    #[test]
    fn skate2_format_strings_take_numbered_arguments() {
        let args = [Value::Text("1,250".into()), Value::Number(3.)];
        assert_eq!(format_text("{0:points} pts", &args), "1,250 pts");
        assert_eq!(format_text("{1} of {0}", &args), "3 of 1,250");
        assert_eq!(format_text("{5} left, {x}, {open", &args), " left, {x}, {open");
    }

    /// Settings > Units through the exe's formatters, metric and imperial for one value.
    #[test]
    fn units_format_like_the_exe() {
        let text = |id: &str| {
            match id {
                "ID_COMMON_METRIC_DISTANCE" => "m",
                "ID_COMMON_METRIC_NEW_FEET" => "ft",
                "ID_COMMON_KMPH" => "Km/h",
                "ID_COMMON_MPH" => "MPH",
                _ => id,
            }
            .to_owned()
        };
        assert_eq!(speed(11.2, 1, false, text), "40.3 Km/h");
        assert_eq!(speed(11.2, 1, true, text), "25.2 MPH");
        assert_eq!(distance(9.4, 1, false, text), "9.4 m");
        assert_eq!(distance(9.4, 1, true, text), "28.2 ft");
        assert_eq!(speed(11.2, 3, false, text), "40.320 Km/h");
        // Half up (not to even), carried back through the point; the ',' places are fixed before the carry.
        assert_eq!(decimal(-2.25, 1), "-2.3");
        assert_eq!(decimal(9.96, 1), "10.0");
        assert_eq!(decimal(999.95, 1), "1000.0");
        assert_eq!(decimal(1234.5, 1), "1,234.5");
        assert_eq!((decimal(0.04, 1), decimal(0., 0)), ("0.0".into(), "0".into()));
        // Skate 2's big-trick text: metres to one place first, read back, then the feet.
        assert_eq!(distance(skate2_number(&decimal(61.27, 1)), 1, true, text), "183.9 ft");
    }

    /// A PNG of the screen in SKATE_CORE_MENU_OUT, when set.
    fn snapshot(draws: &[crate::apt_scene::Draw], root: &std::path::Path, name: &str) {
        if let Some(out) = std::env::var_os("SKATE_CORE_MENU_OUT") {
            crate::core_menu_runtime::tests::raster_draws(draws, root, &std::path::Path::new(&out).join(format!("{name}.png")));
        }
    }

    /// Skate 2's own trickdisplay2 (skate2port/old_hud.py; SKATE2_HUD_ROOT = private/hud-skate2) runs a whole
    /// trick through every native it calls.
    #[test]
    #[ignore = "requires the prepared Skate 2 HUD"]
    fn skate2_trick_display_runs_a_trick() {
        let root = std::path::PathBuf::from(std::env::var("SKATE2_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        // A Skate 3-only trick: its label resolves through the Skate 3 strings old_hud.py falls back to.
        let trick = "ID_TRICK_DARKCATCH_KICKFLIP";
        runtime.update_skate2(input(trick, 500, false), true, false, false).unwrap();
        for _ in 0..30 {
            runtime.update_skate2(input(trick, 500, false), false, false, false).unwrap();
        }
        assert_eq!(runtime.bindings.movie.text_assets.localize(trick), "Kickflip Dark Catch");
        let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap();
        assert!(!draws.is_empty(), "the trick name never drew");
        snapshot(&draws, &root, "skate2-trickdisplay");
        // Its UpdateMultiplier plays a sound for each step (Skate 3's is silent at x1.5; Skate 2's isn't).
        runtime.update_skate2(Input { multiplier: 1., ..input(trick, 500, false) }, false, false, false).unwrap();
        runtime.bindings.sounds.clear();
        for multiplier in [1.5, 2., 3.] {
            runtime.update_skate2(Input { multiplier, ..input(trick, 500, false) }, false, false, false).unwrap();
        }
        assert_eq!(runtime.bindings.sounds, ["multiplyer_15", "multiplyer_2", "multiplyer_3"]);
        runtime.update_skate2(input("Kickflip to Crooked Grind", 2_750, true), false, true, false).unwrap();
        runtime.update_skate2(input("Kickflip to Crooked Grind", 2_750, true), false, false, true).unwrap();
        for _ in 0..120 {
            runtime.update_skate2(input("", 0, false), false, false, false).unwrap();
        }
    }

    /// The clip at `path` from `_global`: its 0-based timeline frame, the alpha multiplier its parent's timeline
    /// last placed it with (colour transform a_mul, 0-255), and `_visible`.
    fn clip(r: &Runtime, path: &[&str]) -> (usize, u8, bool) {
        let Value::Object(id) = member(r, path) else { panic!("{path:?} is not a clip") };
        let i = &r.bindings.movie.instances[&id];
        (i.frame, i.placement.as_ref().map_or(255, |p| p.color[0]), r.vm.get(id, "_visible") == Value::Bool(true))
    }

    /// trickdisplay2's animation at retail times, one update per 60 Hz tick, as skate2_hud's FixedUpdate drives it.
    /// Frames (0-based) and alphas are the disc's own timelines (trickdisplay2.apt): mcScoreModes char 35, mcTrickAnimN
    /// char 7, root char 0.
    #[test]
    #[ignore = "requires the prepared Skate 2 HUD"]
    fn skate2_trick_display_animates_at_retail_times() {
        let root = std::path::PathBuf::from(std::env::var("SKATE2_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let mut r = Runtime::load(&json, input("", 0, false)).unwrap();
        let screen = |r: &Runtime, rest: &[&str]| clip(r, &[&["trickdisplay2", "mScreen"][..], rest].concat());
        let trick = "ID_TRICK_KICKFLIP";
        let tick = |r: &mut Runtime, i: Input| r.update_skate2(i, false, false, false).unwrap();

        // The score arriving sends SetState(7). UpdateTrickScoring's gotoAndPlay("set") puts mcScoreModes at frame 9,
        // and the tick steps it to 10. The score (depth 4) goes 101 -> 255 and the line total (depth 7)
        // 178 -> 255 over frames 10-19. RefreshTrickName opens the root at 'intro' (3).
        r.update_skate2(input(trick, 500, false), true, false, false).unwrap();
        assert_eq!(screen(&r, &["mcScoreModes"]).0, 10);
        assert_eq!(screen(&r, &["mcScoreModes", "mcScore"]).1, 116);
        assert_eq!(screen(&r, &["mcScoreModes", "mcLinescore"]).1, 193);
        assert_eq!(screen(&r, &[]).0, 4);
        assert_eq!(screen(&r, &["mcTrickAnim0", "mcTrickName"]).1, 255);
        for _ in 0..9 {
            tick(&mut r, input(trick, 500, false));
        }
        assert_eq!(screen(&r, &["mcScoreModes"]).0, 19);
        assert_eq!(screen(&r, &["mcScoreModes", "mcScore"]).1, 255);
        assert_eq!(screen(&r, &["mcScoreModes", "mcLinescore"]).1, 255);
        // The line drains with the score unchanged. Its whole seconds tick over, and nothing replays 'set'.
        // mcScoreModes stops at 22, fully lit.
        for n in 0..120 {
            let t = 3. - n as f32 / 60.;
            tick(&mut r, Input { sequence_timer: t as i32, line_time: t, ..input(trick, 500, false) });
            assert_eq!(screen(&r, &["mcScoreModes", "mcScore"]).1, 255, "tick {n}: the score pulsed");
        }
        assert_eq!(screen(&r, &["mcScoreModes"]).0, 22);

        // The sequence ends (score 0): 7 -> Clear() + gotoAndPlay("hide") (23). The score holds to 110 and fades
        // out over 111-125, then hides. The trick name's '0Outro' (8) holds to 92, flickers 93-101 and hides.
        let mut at = vec![(0, 0, 0, 0)];
        for _ in 0..110 {
            tick(&mut r, input(trick, 0, false));
            let (m, s) = (screen(&r, &["mcScoreModes"]), screen(&r, &["mcScoreModes", "mcScore"]));
            let (t, n) = (screen(&r, &["mcTrickAnim0"]), screen(&r, &["mcTrickAnim0", "mcTrickName"]));
            at.push((m.0, s.1, t.0, n.1));
        }
        assert_eq!(at[1], (24, 255, 9, 255));
        assert_eq!(at[84], (107, 255, 92, 255));
        assert_eq!(at[85], (108, 255, 93, 207));
        assert_eq!(at[87], (110, 255, 95, 76));
        assert_eq!(at[88], (111, 254, 96, 102));
        assert_eq!((at[93].2, at[93].3), (101, 0));
        assert_eq!((at[95].0, at[95].1), (118, 182));
        assert_eq!((at[102].0, at[102].1), (125, 0));
        assert_eq!(at[110].0, 125, "'hide' stops at 125");
        assert!(!screen(&r, &["mcScoreModes"]).2 && !screen(&r, &["mcTrickAnim0"]).2, "the outros left a clip up");

        // A bail: CloseTrickDisplay plays the root's 'bail' (52). It holds 10 frames, everything fades to 0 at
        // 62, and the screen hides at 63.
        r.update_skate2(input(trick, 300, false), true, false, false).unwrap();
        r.update_skate2(input(trick, 300, false), false, false, true).unwrap();
        assert_eq!(screen(&r, &[]).0, 53);
        for _ in 0..9 {
            tick(&mut r, input(trick, 300, false));
        }
        assert_eq!(screen(&r, &[]).0, 62);
        assert!(screen(&r, &[]).2);
        tick(&mut r, input(trick, 300, false));
        assert_eq!(screen(&r, &[]).0, 63);
        assert!(!screen(&r, &[]).2, "the bail left the screen up");

        // A full line timer re-sends 11 every tick (0x823bd240's force flag), so UpdateLineDisplay pins multiTimer_mc.
        // Off full, it plays one frame a tick.
        let line = |points| Input { line_points: points, line_capacity: 1_000., ..input(trick, 300, false) };
        tick(&mut r, line(1_000.));
        tick(&mut r, line(1_000.));
        let pinned = screen(&r, &["multiplier_mc", "multiTimer_mc"]).0;
        for _ in 0..10 {
            tick(&mut r, line(1_000.));
        }
        assert_eq!(screen(&r, &["multiplier_mc", "multiTimer_mc"]).0, pinned);
        for _ in 0..10 {
            tick(&mut r, line(900.));
        }
        assert_eq!(screen(&r, &["multiplier_mc", "multiTimer_mc"]).0, pinned + 9);
    }

    /// hudcentral's district card (mcDistrict, char 6) at retail times: 'intro' (1) fades the title 0 -> 255 by 19.
    /// It holds through 'idle' to 139, fades over 'outro' 140-199, then hides.
    #[test]
    #[ignore = "requires the prepared Skate 2 HUD"]
    fn skate2_district_card_animates_at_retail_times() {
        let root = std::path::PathBuf::from(std::env::var("SKATE2_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/central.json")).unwrap()).unwrap();
        let mut r = Runtime::load(&json, input("", 0, false)).unwrap();
        let card = |r: &Runtime| {
            let (frame, _, visible) = clip(r, &["hudcentral", "mScreen", "mcDistrict"]);
            (frame, clip(r, &["hudcentral", "mScreen", "mcDistrict", "mcDistrictTitle"]).1, visible)
        };
        r.show_district("Old Town").unwrap();
        assert_eq!(card(&r), (1, 0, true));
        let mut at = vec![card(&r)];
        for _ in 0..210 {
            r.advance().unwrap();
            at.push(card(&r));
        }
        assert_eq!(at[1], (2, 13, true));
        assert_eq!(at[9], (10, 127, true));
        assert_eq!(at[18], (19, 255, true));
        assert_eq!(at[138], (139, 255, true));
        assert_eq!(at[169], (170, 125, true));
        assert_eq!((at[198].0, at[198].1, at[198].2), (199, 0, false));
        assert_eq!(at[210].0, 199, "the outro stops at 199");
    }

    /// Skate 1's own trickdisplay (skate2port/old_hud1.py; SKATE1_HUD_ROOT = private/hud-skate1) runs a whole
    /// trick through every native it calls.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_trick_display_runs_a_trick() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        runtime.update(input("OLLIE", 10, false), true, false, false).unwrap();
        for _ in 0..30 {
            runtime.update(input("OLLIE", 10, false), false, false, false).unwrap();
        }
        // Its onEnterFrame takes the queued trick onto the display.
        assert_eq!(runtime.vm.get(runtime.controller, "mPendingTricks"), Value::Number(0.), "the trick stayed queued");
        let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap();
        assert!(!draws.is_empty(), "the trick name never drew");
        snapshot(&draws, &root, "skate1-trickdisplay");
        // Its film strip follows the multiplier label it is handed.
        let x2 = || Input { multiplier: 2., line_points: 600., line_capacity: 1000., ..input("OLLIE", 10, false) };
        runtime.update(x2(), false, false, false).unwrap();
        assert_eq!(runtime.vm.get(runtime.controller, "mMultiplier"), Value::Text("x2".into()));
        for _ in 0..30 {
            runtime.update(x2(), false, false, false).unwrap();
        }
        snapshot(&crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap(), &root, "skate1-x2");
        runtime.update(input("Kickflip to Crooked Grind", 2_750, true), false, true, false).unwrap();
        for _ in 0..120 {
            runtime.update(input("", 0, false), false, false, false).unwrap();
        }
    }

    /// A long free-skate session through Skate 1's trick display: lines that build x1 -> x3 and drain, stance
    /// flips, modified tricks, clean, sketchy and bailed landings. The in-game HUD drops out on the first error.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_trick_display_survives_a_session() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let tricks = ["KICKFLIP", "ID_TRICK_HEELFLIP", "NOSE MANUAL", "50-50 GRIND", "360 HARDFLIP", "FAKIE", ""];
        let mut seed = 12345u32;
        let mut rand = move |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % n
        };
        let (mut sequence, mut line, mut points, mut stance) = (0, 0, 0f32, [false, false, true, false]);
        for frame in 0..6000 {
            let event = rand(40);
            let (mut new_trick, mut modified, mut close) = (false, false, false);
            let trick = tricks[rand(tricks.len() as u32) as usize];
            match event {
                0..=2 => {
                    new_trick = true;
                    sequence += 10 + rand(300) as i32;
                    points = (points + 120.).min(1000.);
                }
                3 => modified = true,
                4 => {
                    close = true;
                    line += sequence;
                    sequence = 0;
                }
                5 => stance[rand(2) as usize] ^= true,
                _ => points = (points - 2.).max(0.),
            }
            let multiplier = match points {
                p if p >= 800. => 3.,
                p if p >= 450. => 2.,
                p if p >= 50. => 1.5,
                _ => 1.,
            };
            let sketchy = rand(5) == 0;
            let mut i = input(trick, sequence, sketchy);
            (i.line_score, i.line_points, i.line_capacity, i.multiplier, i.stance) = (line, points, 1000., multiplier, stance);
            i.sequence_timer = (points / 100.) as i32;
            if let Err(error) = runtime.update(i, new_trick, modified, close) {
                panic!("frame {frame} (event {event}, trick {trick:?}, x{multiplier}): {error}");
            }
            let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes)
                .unwrap_or_else(|error| panic!("frame {frame}: geometry {error}"));
            if frame % 250 == 0 {
                println!("frame {frame}: {} draws, x{multiplier}, points {points}, sequence {sequence}", draws.len());
            }
        }
    }

    /// A long manual/grind line: the trick is modified every frame for a minute with no landing in between,
    /// then lands. Skate 1 queues each name it is handed; the queue must not grow without bound.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_trick_display_survives_a_long_combo() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let names = ["MANUAL", "NOSE MANUAL", "50-50 GRIND", "KICKFLIP TO 5-0 GRIND", "BOARDSLIDE"];
        for frame in 0..3600 {
            let name = names[(frame / 45) % names.len()];
            let mut i = input(name, 100 + frame as i32, false);
            (i.line_points, i.line_capacity, i.multiplier) = (900., 1000., 3.);
            let (new_trick, modified) = (frame % 45 == 0, frame % 45 != 0);
            runtime
                .update(i, new_trick, modified, false)
                .unwrap_or_else(|error| panic!("frame {frame} ({name}): {error}"));
            let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap();
            if frame % 600 == 0 {
                println!("frame {frame}: {} clips, {} draws, pending {:?}", runtime.bindings.movie.instances.len(), draws.len(),
                    runtime.vm.get(runtime.controller, "mPendingTricks"));
            }
        }
        runtime.update(input("BOARDSLIDE", 5000, false), false, false, true).unwrap();
    }

    /// Lines separated by idle stretches where the combo timer drains to zero and the line ends: every line's
    /// tricks must draw as the first line's did.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_trick_display_comes_back_for_every_line() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let mut frame = |r: &mut Runtime, trick: &str, sequence: i32, line: i32, points: f32, events: (bool, bool, bool)| {
            let mut i = input(trick, sequence, false);
            let multiplier = [(800., 3.), (450., 2.), (50., 1.5)].iter().find(|(t, _)| points >= *t).map_or(1., |(_, m)| *m);
            (i.line_score, i.line_points, i.line_capacity, i.multiplier, i.stance) = (line, points, 1000., multiplier, [false, false, true, false]);
            i.sequence_timer = (points / 100.) as i32;
            r.update(i, events.0, events.1, events.2).unwrap();
            crate::apt_scene::draw(&r.bindings.movie, &r.vm, &shapes).unwrap().len()
        };
        let mut shown = Vec::new();
        for line_no in 0..4 {
            let (mut points, mut sequence, mut line) = (0f32, 0, 0);
            let mut most = 0;
            for t in 0..6 {
                sequence += 100;
                points = (points + 200.).min(1000.);
                most = most.max(frame(&mut runtime, "KICKFLIP", sequence, line, points, (true, false, false)));
                for _ in 0..40 {
                    points -= 1.;
                    most = most.max(frame(&mut runtime, "KICKFLIP", sequence, line, points, (false, false, false)));
                }
                if t % 2 == 1 {
                    line += sequence;
                    sequence = 0;
                    most = most.max(frame(&mut runtime, "KICKFLIP", sequence, line, points, (false, false, true)));
                }
            }
            // Idle: the timer drains, the line ends and its score clears.
            let mut idle = 0;
            while points > 0. {
                points = (points - 4.).max(0.);
                idle = frame(&mut runtime, "", 0, if points > 0. { line } else { 0 }, points, (false, false, false));
            }
            for _ in 0..300 {
                idle = frame(&mut runtime, "", 0, 0, 0., (false, false, false));
            }
            println!("line {line_no}: most draws while skating {most}, idle {idle}");
            shown.push(most);
        }
        assert!(shown.iter().all(|&n| n >= shown[0]), "a later line drew less than the first: {shown:?}");
    }

    /// Falling off the board mid-line: the line and multiplier reset at once and GetCurrentTrickMetrics()[3] says
    /// so, which sends the screen to its "bail" label and hides it. Once the skater is back up skate1_hud shows it
    /// again; every later trick must draw as the first did, whatever was on screen at the bail.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_trick_display_comes_back_after_a_bail() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let frame = |r: &mut Runtime, trick: &str, sequence: i32, line: i32, points: f32, events: (bool, bool, bool)| {
            let mut i = input(trick, sequence, false);
            let multiplier = [(800., 3.), (450., 2.), (50., 1.5)].iter().find(|(t, _)| points >= *t).map_or(1., |(_, m)| *m);
            (i.line_score, i.line_points, i.line_capacity, i.multiplier, i.stance) = (line, points, 1000., multiplier, [false, false, true, false]);
            i.sequence_timer = (points / 100.) as i32;
            i.trick_metrics[3] = Value::Bool(events.2); // the game's close_tricks: set on a bail only
            r.update(i, events.0, events.1, events.2).unwrap();
            crate::apt_scene::draw(&r.bindings.movie, &r.vm, &shapes).unwrap().len()
        };
        let mut shown = Vec::new();
        for bail_no in 0..6 {
            let (mut sequence, mut points) = (0, 0f32);
            let mut most = 0;
            for _ in 0..3 {
                sequence += 150;
                points = (points + 300.).min(1000.);
                most = most.max(frame(&mut runtime, "KICKFLIP", sequence, 200, points, (true, false, false)));
                for _ in 0..20 {
                    most = most.max(frame(&mut runtime, "KICKFLIP", sequence, 200, points, (false, false, false)));
                }
            }
            // The bail frame: everything resets, the display is told to close.
            frame(&mut runtime, "KICKFLIP", 0, 0, 0., (false, false, true));
            // Ragdoll: no scoring for a while (the name is cleared); the bail animation hides the display.
            let mut down = 0;
            for _ in 0..120 {
                down = frame(&mut runtime, "", 0, 0, 0., (false, false, false));
            }
            assert_eq!(down, 0, "the bail label should hide the display");
            // Back up (skate1_hud's show), then the first trick of the next line.
            runtime.show().unwrap();
            let mut back = 0;
            for _ in 0..2 {
                back = back.max(frame(&mut runtime, "OLLIE", 100, 0, 200., (true, false, false)));
                for _ in 0..20 {
                    back = back.max(frame(&mut runtime, "OLLIE", 100, 0, 200., (false, false, false)));
                }
            }
            println!("bail {bail_no}: drew {most} before, {back} after");
            shown.push(back);
        }
        assert!(shown.iter().all(|&n| n > 0), "the display did not come back after a bail: {shown:?}");
    }

    /// Skate 1 clears its trick names 90 frames after a sequence lands (ActivateTrickScoring starts the count, then
    /// Clear plays each name's outro, which hides it); without the landing call they stay up for good.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_trick_names_clear_after_a_landing() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        // How many of the three trick-name clips (trickdisplay.mScreen.mcTrickAnim0..2) are showing.
        let names_shown = |r: &Runtime| {
            let Value::Object(td) = r.vm.get(r.vm.global, "trickdisplay") else { panic!("no trickdisplay") };
            let Value::Object(screen) = r.vm.get(td, "mScreen") else { panic!("no mScreen") };
            (0..3).filter(|n| match r.vm.get(screen, &format!("mcTrickAnim{n}")) {
                Value::Object(clip) => r.vm.get(clip, "_visible") == Value::Bool(true),
                _ => false,
            }).count()
        };
        let run = |land: bool| {
            let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
            let frame = |r: &mut Runtime, new_trick: bool| {
                let mut i = input("KICKFLIP", 100, false);
                i.stance = [false, false, true, false];
                r.update(i, new_trick, false, false).unwrap();
            };
            frame(&mut runtime, true);
            for _ in 0..40 {
                frame(&mut runtime, false);
            }
            let shown = names_shown(&runtime);
            if land {
                runtime.landed().unwrap();
            }
            let mut counts = Vec::new();
            for _ in 0..150 {
                frame(&mut runtime, false);
                counts.push(names_shown(&runtime));
            }
            (shown, counts)
        };
        let (shown, landed) = run(true);
        let (_, never) = run(false);
        assert_eq!(shown, 1, "the trick name never showed");
        // Still up through Skate 1's 90-frame count, gone once Clear's outro has played.
        assert_eq!(landed[80], 1, "the name went early");
        assert_eq!(landed[149], 0, "the trick name stayed up after landing");
        assert_eq!(never[149], 1, "without a landing the name should stay");
    }

    /// `_global.<path...>`, a member at a time.
    fn member(r: &Runtime, path: &[&str]) -> Value {
        path.iter().fold(Value::Object(r.vm.global), |at, key| match at {
            Value::Object(object) => r.vm.get(object, key),
            _ => Value::Undefined,
        })
    }

    /// Skate 2's big-trick callouts: trick_stats' slots, through the game's ContextTrickDone (HUD call 4), put the
    /// measurement up in its title with the Big or Huge backer, and play out once the slot times out (2 s).
    #[test]
    #[ignore = "requires the prepared Skate 2 HUD"]
    fn skate2_big_tricks_show_their_measurement() {
        use crate::trick_stats::{big_trick, BigTricks, Stat};
        let root = std::path::PathBuf::from(std::env::var("SKATE2_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let mut callouts = BigTricks::default();
        // The disc's thresholds: air time Big 7 s / Huge 10 s, manual Big 10 m / Huge 60 m, sequence size 5 / 10.
        assert_eq!(big_trick(&json["trick_stats"], Stat::AirTime, 6.9), None);
        let add = |c: &mut BigTricks, stat, value| {
            let (kind, achievement) = big_trick(&json["trick_stats"], stat, value).unwrap();
            c.add(kind, achievement, value);
        };
        // The callouts live in the trick display, which hides itself between sequences: they come at a landing,
        // with the sequence still up.
        let trick = "ID_TRICK_DARKCATCH_KICKFLIP";
        runtime.update_skate2(input(trick, 500, false), true, false, false).unwrap();
        for _ in 0..30 {
            runtime.update_skate2(input(trick, 500, false), false, false, false).unwrap();
        }
        runtime.landed().unwrap();
        let bare = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap().len();
        add(&mut callouts, Stat::AirTime, 8.4);
        add(&mut callouts, Stat::Manual, 61.27);
        let frame = |r: &mut Runtime, c: &mut BigTricks| {
            let redraw = c.tick(17);
            let mut i = input(trick, 500, false);
            i.context_tricks = c.data();
            r.update_skate2(i, false, false, false).unwrap();
            if redraw {
                r.call("ContextTrickDone").unwrap();
            }
        };
        let slot = |r: &Runtime, n: usize, key: &str| member(r, &["trickdisplay2", "mScreen", &format!("bigTrick_mc{n}"), "dmo1_mc", key]);
        let title = |r: &Runtime, n: usize| match slot(r, n, "title") {
            Value::Object(field) => r.vm.get(field, "text").text(),
            other => panic!("bigTrick_mc{n} title: {other:?}"),
        };
        // ContextTrickDone's gotoAndPlay("intro") puts bigTrick_mc (char 108) at frame 14, with dmo1_mc at alpha 101. The
        // slot dips to 0 at 21 and rises to 255 at 'idle' (29), where it stops at 42: 15 ticks in.
        let dmo = |r: &Runtime| (clip(r, &["trickdisplay2", "mScreen", "bigTrick_mc0"]).0, clip(r, &["trickdisplay2", "mScreen", "bigTrick_mc0", "dmo1_mc"]).1);
        frame(&mut runtime, &mut callouts);
        assert_eq!(dmo(&runtime), (14, 101));
        for _ in 0..7 {
            frame(&mut runtime, &mut callouts);
        }
        assert_eq!(dmo(&runtime), (21, 0));
        for _ in 0..8 {
            frame(&mut runtime, &mut callouts);
        }
        assert_eq!(dmo(&runtime), (29, 255));
        for _ in 0..4 {
            frame(&mut runtime, &mut callouts);
        }
        assert_eq!(title(&runtime, 0), "Air Time 8s");
        assert_eq!(title(&runtime, 1), "Manual Distance 61.3 m");
        // A repeat that beats it redraws in place (the slot stays up, no second intro), now with the Huge backer.
        add(&mut callouts, Stat::AirTime, 10.6);
        add(&mut callouts, Stat::SequenceTricks, 6.);
        for _ in 0..20 {
            frame(&mut runtime, &mut callouts);
        }
        assert_eq!(title(&runtime, 0), "Air Time 10s");
        assert_eq!(title(&runtime, 2), "Tricks in Sequence 6");
        // Three up: AddBigTrick drops anything more, even a repeat.
        add(&mut callouts, Stat::Grind, 12.);
        add(&mut callouts, Stat::Manual, 70.);
        frame(&mut runtime, &mut callouts);
        assert_eq!(title(&runtime, 1), "Manual Distance 61.3 m");
        assert_eq!(callouts.data().iter().filter(|v| **v == Value::Bool(true)).count(), 3);
        let draws = crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap();
        snapshot(&draws, &root, "skate2-bigtricks");
        assert!(draws.len() > bare, "the callouts never drew ({} draws, {bare} without them)", draws.len());
        // Two seconds after their last report the slots free and the screen plays each one's outro.
        for _ in 0..150 {
            frame(&mut runtime, &mut callouts);
        }
        assert!(callouts.data().iter().all(|v| *v != Value::Bool(true)), "a callout outlived its 2 s");
        for n in 0..3 {
            assert_eq!(member(&runtime, &["trickdisplay2", "mContextStatusArray", &n.to_string()]), Value::Bool(false), "slot {n}");
        }
        snapshot(&crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap(), &root, "skate2-bigtricks-out");
    }

    /// Skate 1's personal-best callout: trick_stats' table commits a new best, and the game's UpdatePersonalBests
    /// (HUD call 16) puts its title and value text up in mcPersonalbest.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_personal_best_shows_its_title_and_value() {
        let root = std::path::PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let file = std::env::temp_dir().join(format!("skate1-bests-{}.json", std::process::id()));
        let mut bests = crate::trick_stats::Bests::load(&json["trick_stats"], &file);
        let air = crate::trick_stats::best_index(crate::trick_stats::Stat::AirTime).unwrap();
        bests.submit(air, 1.5); // under its 2 s minimum
        assert!(bests.commit(false).is_empty());
        bests.submit(air, 2.534);
        let new = bests.commit(false);
        assert_eq!(new, [("ID_RECORD_BESTS_10".to_string(), "2.53 s".to_string())]);
        // Kept for the profile: a fresh load knows 2.534 s, so 2.5 s is no best.
        let mut again = crate::trick_stats::Bests::load(&json["trick_stats"], &file);
        again.submit(air, 2.5);
        assert!(again.commit(false).is_empty());
        let _ = std::fs::remove_file(&file);
        let draws = |r: &Runtime| crate::apt_scene::draw(&r.bindings.movie, &r.vm, &shapes).unwrap();
        for _ in 0..10 {
            runtime.update(input("", 0, false), false, false, false).unwrap();
        }
        let before = draws(&runtime).len();
        runtime.bindings.personal_bests.extend(new);
        runtime.call("UpdatePersonalBests").unwrap();
        let mut most = 0;
        for n in 0..60 {
            runtime.update(input("", 0, false), false, false, false).unwrap();
            let d = draws(&runtime);
            most = most.max(d.len());
            if n == 30 {
                snapshot(&d, &root, "skate1-personal-best");
            }
        }
        assert!(runtime.bindings.personal_bests.is_empty(), "the screen never asked for its bests");
        assert!(most > before, "the personal best never drew ({most} draws, {before} before)");
        let text = |field: &str| match member(&runtime, &["trickdisplay", "mScreen", "mcPersonalbest", "mcAchievement", field, "labeltext"]) {
            Value::Object(label) => runtime.vm.get(label, "text").text(),
            other => panic!("{field}: {other:?}"),
        };
        assert_eq!(text("mcTitle"), "ID_RECORD_BESTS_10");
        assert_eq!(runtime.bindings.movie.text_assets.localize("ID_RECORD_BESTS_10"), "LONGEST AIR TIME");
        assert_eq!(text("mcInfo"), "2.53 s");
        // Each best stays up 120 frames (RefreshPersonalBests), then the callout plays out.
        for _ in 0..200 {
            runtime.update(input("", 0, false), false, false, false).unwrap();
        }
        assert!(draws(&runtime).len() <= before, "the personal best stayed up");
    }

    /// Skate 2's hudcentral district title card shows the district it's handed and plays out on its own.
    #[test]
    #[ignore = "requires the prepared Skate 2 HUD"]
    fn skate2_district_card_shows_the_title() {
        let root = std::path::PathBuf::from(std::env::var("SKATE2_HUD_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/central.json")).unwrap()).unwrap();
        let shapes: crate::apt_scene::Shapes = serde_json::from_value(json["shapes"].clone()).unwrap();
        let mut runtime = Runtime::load(&json, input("", 0, false)).unwrap();
        let drawn = |r: &Runtime| crate::apt_scene::draw(&r.bindings.movie, &r.vm, &shapes).unwrap().len();
        runtime.show_district("Old Town").unwrap();
        let mut most = 0;
        for frame in 0..30 {
            runtime.advance().unwrap();
            most = most.max(drawn(&runtime));
            if frame == 29 {
                snapshot(&crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &shapes).unwrap(), &root, "skate2-district");
            }
        }
        assert!(most > 0, "the title card never drew");
        for _ in 0..600 {
            runtime.advance().unwrap();
        }
        assert_eq!(drawn(&runtime), 0, "the title card stayed up");
    }
}
