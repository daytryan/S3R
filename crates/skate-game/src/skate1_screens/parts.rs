//! The tutorial window's lines on its overlays' loadMovie components, run on the components' own screens: the
//! overlays' VMs see those components as undefined. RETAIL AS (scratch/as):
//! - hudobjective on `objectivehud`;
//! - the tutorial overlay's controller lines on `tutorial`;
//! - hud_action_overlay on `actionoverlay` + `inkaction`.
use super::Card;
use crate::{
    apt_screen::Screen,
    apt_text::literal,
    apt_vm::Value,
    skate1_online_hud::{clip_call, go, method, put},
};
use serde_json::Value as Json;

/// objectivehud's three result rows.
const RESULTS: usize = 3;
const LEFT: &str = "mcTextLeftJustified";

/// hudobjective (HUDHooks.ObjectiveHUD) on the objectivehud component: mObjectiveState, mObjectiveType (true: checkbox).
#[derive(Default)]
pub(crate) struct ObjectiveHud {
    state: [bool; RESULTS],
    kind: [bool; RESULTS],
}

impl ObjectiveHud {
    /// One `ObjectiveHUD.<method>` call the overlay made. `film`: the film menu's (selected, count).
    pub(crate) fn call(&mut self, s: &mut Screen, method: &str, args: &[Json], card: &Card, film: (usize, usize)) -> Result<(), String> {
        let arg = args.first();
        match method {
            "Setup" => self.setup(s, arg.and_then(Json::as_str).unwrap_or_default(), card, film),
            "UpdateObjectives" => self.update_objectives(s, card),
            "UpdateObjectiveResults" => self.update_results(s, card, arg == Some(&Json::Bool(true))),
            "Play" => play(s, arg.and_then(Json::as_str).unwrap_or_default()),
            "Refresh" => {
                let score = formatted(s, "ID_COMMON_NUMBER", &[card.score]);
                put(s, "mcTextScore.labelText", "text", Value::Text(score))
            }
            _ => Ok(()),
        }
    }

    /// Setup(type). Its LT / B / A / RT button labels name clips objectivehud doesn't have (no-ops in retail too).
    fn setup(&mut self, s: &mut Screen, kind: &str, card: &Card, (selected, films): (usize, usize)) -> Result<(), String> {
        put(s, "mcTextScore", "_visible", Value::Bool(false))?;
        if kind == "film" {
            put(s, "mcHelpButtonY", "_visible", Value::Bool(films > 1))?;
            if films > 1 {
                let cycle = formatted(s, "ID_COMMON_CYCLE_NUMBER_OVER_NUMBER", &[selected as i64 + 1, films as i64]);
                put(s, "mcHelpButtonY.mcButtonText.labelText", "text", Value::Text(cycle))?;
            }
            game_icon(s, "mcChallengeIcon.mObjectiveIcon", &card.icon)?;
            put(s, "mcTextScore", "_visible", Value::Bool(card.points > 0))?;
        }
        put(s, "mcTextTitle.title100.labelText", "text", Value::Text(literal(&card.title)))?;
        self.state = [false; RESULTS];
        for i in 0..RESULTS {
            self.clear_state(s, card, i)?;
        }
        put(s, "mcHelpButtonY.mcButtonText.labelText", "autoSize", Value::Text("right".into()))
    }

    /// UpdateObjectiveClearStates(i): an objective not yet done goes back to its empty box.
    /// UNTRACED: GetObjectiveStatus / GetObjectiveType past the objective count (taken as false).
    fn clear_state(&mut self, s: &mut Screen, card: &Card, i: usize) -> Result<(), String> {
        let o = card.objectives.get(i);
        if o.is_some_and(|o| o.done) {
            return Ok(());
        }
        self.kind[i] = o.is_some_and(|o| o.checkbox);
        stop(s, &format!("mcResult{i}"), if self.kind[i] { "checkbox" } else { "fraction" })
    }

    /// UpdateObjectives: a row per objective (the first three), then UpdateObjectiveResults().
    fn update_objectives(&mut self, s: &mut Screen, card: &Card) -> Result<(), String> {
        for i in 0..RESULTS {
            let clip = format!("mcResult{i}");
            put(s, &clip, "_visible", Value::Bool(i < card.objectives.len()))?;
            if let Some(o) = card.objectives.get(i) {
                let label = format!("{clip}.mcObjective.labelText");
                put(s, &label, "text", Value::Text(literal(&o.title)))?;
                put(s, &label, "autoSize", Value::Text("right".into()))?;
                self.kind[i] = o.checkbox;
            }
        }
        self.update_results(s, card, false)
    }

    /// UpdateObjectiveResults(lost):
    /// - each row shows its result text;
    /// - a newly done objective plays its success once;
    /// - with `lost`, an open objective plays its fail;
    /// - every other row stops on its empty box.
    /// With `lost` it first plays Audio.PlayObjectiveFailed (hudobjective line 135; RETAIL message 223 -> sound 2018,
    /// HUD.abk sample 18), as apt_screen plays a Skate 1 movie's own Audio calls.
    fn update_results(&mut self, s: &mut Screen, card: &Card, lost: bool) -> Result<(), String> {
        if lost {
            s.bindings.sounds.push("PlayObjectiveFailed".into());
        }
        for (i, o) in card.objectives.iter().enumerate().take(RESULTS) {
            let clip = format!("mcResult{i}");
            put(s, &format!("{clip}.mcFraction.labelText"), "text", Value::Text(literal(&o.result)))?;
            let check = self.kind[i];
            if o.done {
                if !self.state[i] {
                    go(s, &clip, if check { "success_check" } else { "success_fraction" })?;
                    self.state[i] = true;
                }
            } else {
                self.state[i] = false;
                if lost {
                    go(s, &clip, if check { "fail_check" } else { "fail_fraction" })?;
                } else {
                    stop(s, &clip, if check { "checkbox" } else { "fraction" })?;
                }
            }
        }
        Ok(())
    }
}

/// Play(clip):
/// - 'replay' and 'photoIntro' set the left-justified controls first. Screen_EdgeOffset is 0, so `_x` is left as it is.
/// - Then the component plays `clip`. Its labels' frame actions show and hide it (RETAIL timeline).
/// DEMOMode is undefined, which gives retail's non-demo branch.
fn play(s: &mut Screen, clip: &str) -> Result<(), String> {
    let control = |i: usize| format!("{LEFT}.mcTextControls{i}.labelText");
    match clip {
        "replay" => {
            put(s, &format!("{LEFT}.mcTextTitle"), "_visible", Value::Bool(false))?;
            go(s, LEFT, "film")?;
            for (i, id) in ["ID_COMMON_PLAY_PAUSE", "ID_COMMON_REWIND", "ID_COMMON_FORWARD", "ID_REPLAY_EDITREPLAY", "ID_COMMON_SKIP"].into_iter().enumerate() {
                put(s, &control(i), "text", Value::Text(id.into()))?;
            }
        }
        "photoIntro" => {
            put(s, &format!("{LEFT}.mcTextTitle"), "_visible", Value::Bool(true))?;
            go(s, LEFT, "photo")?;
            put(s, &format!("{LEFT}.mcTextTitle.labelText"), "text", Value::Text("ID_REPLAY_BUTTONS_PHOTO".into()))?;
            put(s, &control(1), "text", Value::Text("ID_COMMON_PREVIOUS".into()))?;
            put(s, &control(2), "text", Value::Text("ID_COMMON_NEXT".into()))?;
            put(s, &format!("{LEFT}.mcTextControls3"), "_visible", Value::Bool(true))?;
            put(s, &format!("{LEFT}.mcYbutton"), "_visible", Value::Bool(true))?;
            put(s, &control(3), "text", Value::Text("ID_REPLAY_TAKEPHOTO".into()))?;
            put(s, &control(4), "text", Value::Text("ID_COMMON_SKIP".into()))?;
        }
        _ => {}
    }
    s.play(clip)
}

/// `gotoAndStop(label)` on a clip; nothing when the clip isn't there (as AS2).
pub(crate) fn stop(s: &mut Screen, path: &str, label: &str) -> Result<(), String> {
    if s.clip(path).is_none() {
        return Ok(());
    }
    clip_call(s, path, "gotoAndStop", vec![Value::Text(label.into())])
}

/// `SetGameIcon(icon)` on the game_icons clip at `path`. UNTRACED: a clip without the method draws no icon.
pub(crate) fn game_icon(s: &mut Screen, path: &str, icon: &str) -> Result<(), String> {
    let has = s.clip(path).is_some_and(|c| !matches!(s.vm.get(c, "SetGameIcon"), Value::Undefined));
    if has { s.call_at(path, "SetGameIcon", &[Json::from(icon)]).map(drop) } else { Ok(()) }
}

/// FELanguage.GetLocalizedFormatString(id, n, values..) as the game answers it: the id's text in the movie's language
/// with {0} / {0:d} / {0:s}, {1} ..., filled; literal, so nothing looks it up again.
pub(crate) fn formatted(s: &Screen, id: &str, values: &[i64]) -> String {
    let mut text = s.bindings.movie.text_assets.localize(id);
    for (i, value) in values.iter().enumerate() {
        for spec in [format!("{{{i}}}"), format!("{{{i}:d}}"), format!("{{{i}:s}}")] {
            text = text.replace(&spec, &value.to_string());
        }
    }
    literal(&text)
}

/// Tutorial.GetControllerSequenceString for Tutorial.ShowController's sequence enum (0 None .. 12 BothTriggers).
/// RETAIL: 0x824609b0 answers the (Xbox, PS3) string pair table 0x830a3ff8's Xbox name at the current sequence
/// ([0x830fcb18]+584); the strings are the controller movie's `<name>intro` / `<name>outro` labels on
/// mcControllers.mc360Animations.
pub(crate) fn controller_label(seq: i64) -> &'static str {
    match seq {
        1 => "x",
        2 => "a",
        3 => "ax",
        4 => "leftanalog",
        5 => "leftanalogquick",
        6 => "rightanalog",
        7 => "triggerrightanalog",
        8 => "rightanalogGesture",
        9 => "christAir",
        10 => "triggerleft",
        11 => "triggerright",
        12 => "triggersBoth",
        _ => "none",
    }
}

/// Tutorial.GetControllerGestureString for ShowController's gesture enum (eSk8ControllerGesture 0x827b7558, 0 None ..
/// 55 TriggerBothLarge). RETAIL: 0x824609e0 answers the (Xbox, PS3) pair table 0x830a4060's Xbox name at the current
/// gesture ([0x830fcb18]+588): trick_gestures' export names, which SetGestureType attaches.
pub(crate) fn gesture_name(gesture: i64) -> &'static str {
    const NAMES: [&str; 56] = [
        "mcnone", "mc360flip", "mc360popshuv", "mcdirections", "mcdpadup", "mcfsnoseslide", "mcheelflip", "mckickflip2",
        "mclbdpadleft", "mclbdpadright", "mclbhold", "mcmanual", "mcmanualtoflip", "mcnollie", "mcnollie360hardflip",
        "mcnollie360shuv", "mcnolliekickflip", "mcnosemanual2", "mcollie", "mcolliehold", "mcfspopshuvit", "mcpopshuvit",
        "mc360pumpinganimated", "mcpushahold", "mcpushlight", "mcpushxhold", "mctriggerdirections", "mctriggerleft",
        "mctriggerright", "mctriggersb", "mc360christair", "mc360spin", "mc360tweak", "mc360hardflip", "mc360heelflip",
        "mc360inwardheel", "mcfs360popshuv", "mchardflip", "mcinwardheel", "mcvarialheelflip", "mcvarialkickflip",
        "mcnolliefspopshuvit", "mcnollie360flip", "mcnollie360heelflip", "mcnollie360popshuv", "mcnolliefs360popshuv",
        "mcnolliehardflip", "mcnollieheel", "mcnollieinwardheel", "mcnolliepopshuvit", "mcnollievarialheelflip",
        "mcnollievarialkickflip", "mcnollie360inwardheel", "xbox_l_triggerlarge", "xbox_r_triggerlarge",
        "xbox_both_triggerslarge",
    ];
    usize::try_from(gesture).ok().and_then(|g| NAMES.get(g)).copied().unwrap_or("mcnone")
}

const ANIMATIONS: &str = "mcControllers.mc360Animations";

/// mc360Animations' mcGesture: trick_gestures' 'Trick Gesture' (imported into tutorial.swf).
const GESTURE: &str = "mcControllers.mc360Animations.mcGesture.mcGestureComponent";

/// The tutorial overlay's lines on its `controller` component (tutorial.swf, the 360 branch): mControllerVisible,
/// mControllerString, and the gesture component's SetGestureType / SetGestureLoop.
pub(crate) struct Controller {
    visible: bool,
    string: &'static str,
}
impl Default for Controller {
    fn default() -> Self {
        Self { visible: false, string: "none" }
    }
}

impl Controller {
    /// The overlay's SetState(state) half on the controller. `sequence` / `gesture` are Tutorial's
    /// GetControllerSequenceString / GetControllerGestureString.
    /// - 0: the gesture's SetGestureLoop(255), then Show(false); 3: the outro if shown.
    /// - 1: Show(true) plays the 360 pad.
    /// - 2: the intro if hidden.
    /// - 4: the sequence's intro, then the gesture's SetGestureType(gesture) + SetGestureLoop(180). On "none" it plays
    ///   the last sequence's outro and keeps that sequence.
    pub(crate) fn set_state(&mut self, s: &mut Screen, state: i64, sequence: &'static str, gesture: &str) -> Result<(), String> {
        if state == 0 {
            gesture_call(s, "SetGestureLoop", Value::Number(255.))?;
        }
        match state {
            0 | 3 if self.visible => {
                self.visible = false;
                s.play("outro")
            }
            1 => go(s, "mcControllers", "360"),
            2 if !self.visible => {
                self.visible = true;
                s.play("intro")
            }
            4 if sequence == "none" => go(s, ANIMATIONS, &format!("{}outro", self.string)),
            4 => {
                self.string = sequence;
                go(s, ANIMATIONS, &format!("{sequence}intro"))?;
                gesture_call(s, "SetGestureType", Value::Text(gesture.into()))?;
                gesture_call(s, "SetGestureLoop", Value::Number(180.))
            }
            _ => Ok(()),
        }
    }
}

/// `method(arg)` on the gesture component; nothing when it isn't there or lacks the method (an AS2 call on undefined).
fn gesture_call(s: &mut Screen, name: &str, arg: Value) -> Result<(), String> {
    method(s, GESTURE, name, arg)
}

/// The NIS HUD state's tutorial screen for overlay id `id` (ICE channel 101, enum names at 0x8309dee0; screens table
/// near 0x8309ef14: Tutorial_<Name> -> nis/tutorial_nis/<file>.swf, exported as nis_<file>).
pub(crate) fn nis_movie(id: u8) -> Option<&'static str> {
    Some(match id {
        55 => "nis_grind_tutorial",
        56 => "nis_advancedflips_tutorial",
        57 => "nis_airs_tutorial",
        58 => "nis_basics",
        59 => "nis_prod_tutorial",
        60 => "nis_timing_tutorial",
        61 => "nis_jdill_tutorial",
        62 => "nis_dway_tutorial",
        69 => "nis_mcarroll_tutorial",
        70 => "nis_chaslam_tutorial_new",
        _ => return None,
    })
}

/// What the scene's overlay track asks of the NIS tutorial screen this frame.
#[derive(Debug, PartialEq)]
pub(crate) enum NisAct {
    /// Open the movie and SetState(1): tutorialscreenimp's gotoAndPlay(1).
    Open(&'static str),
    Close,
}

/// The NIS state's tutorial screen (state 2, SetSubState 0x82351eb0):
/// - RETAIL: entering a tutorial id plays its screen from frame 1 (0x82352018 case 0x8235266c, SetState(1));
///   leaving one (0x82352688) and entering 0 Clear do nothing to it, so it plays on.
/// - UNTRACED: whether a shot from a take without OVERLAY keys posts 0 then the id again (each restarting it).
///   Ruling: no restart. The movies are authored as one timeline over the whole scene (Timing 2303 frames = 38.4 s
///   vs the 38.23 s scene; Grinds 27.4 vs 27.19 s; Timing `ollie` at 19.94 s vs its shot at 19.66 s), and only take 5
///   of Timing keys OVERLAY at all (rulings.md B1).
/// - UNTRACED: the close. Fallback: when the scene ends.
#[derive(Default)]
pub(crate) struct Nis {
    shown: u8,
}
impl Nis {
    /// `scene`: the running scene's overlay id, None when no scene runs.
    pub(crate) fn step(&mut self, scene: Option<u8>) -> Option<NisAct> {
        match scene {
            None if self.shown != 0 => {
                self.shown = 0;
                Some(NisAct::Close)
            }
            Some(id) if id != self.shown && nis_movie(id).is_some() => {
                self.shown = id;
                nis_movie(id).map(NisAct::Open)
            }
            _ => None,
        }
    }
}

const MAIN: &str = "action_overlay.main_mc";
const LABEL: &str = "action_overlay.main_mc.mcPaperpanel.labelmc.textLabel";
const ICON: &str = "action_overlay.main_mc.iconType.mChallengeIcon";

/// hud_action_overlay (hudframe+0) on actionoverlay, with inkaction behind it: mVisible.
#[derive(Default)]
pub(crate) struct Action {
    visible: bool,
}

impl Action {
    /// SetState(state) for the states the window uses:
    /// - 0 Off, 1 None;
    /// - 3 RecordFilm;
    /// - 13 Call;
    /// - 14 Challenge, the sign-up prompt. `icon` is Mission.GetChallengeIconString.
    /// Audio.PlayTmobIncomingCall stays silent (UNTRACED Skate 1 FE sounds).
    pub(crate) fn set_state(&mut self, a: &mut Screen, ink: Option<&mut Screen>, state: i64, icon: &str) -> Result<(), String> {
        let (label, icon) = match state {
            0 => return self.show(a, ink, false),
            1 => return self.show(a, ink, true),
            3 => ("ID_MISSION_FILM_CHALLENGE_BEGIN_RECORD_CAPS", "misc_record"),
            13 => ("ID_COMMON_INCOMING_CALL_CAPS", "misc_call"),
            14 => ("ID_CHALLENGE_COMMON_SIGNUP", icon),
            _ => return Ok(()),
        };
        put(a, LABEL, "text", Value::Text(label.into()))?;
        self.show(a, ink, true)?;
        game_icon(a, ICON, icon)
    }

    /// Show(on): on a change, the ink's mcInkAction and the panel play intro / outro. Showing hides the D-pad overlay.
    fn show(&mut self, a: &mut Screen, ink: Option<&mut Screen>, on: bool) -> Result<(), String> {
        if self.visible == on {
            return Ok(());
        }
        let label = if on { "intro" } else { "outro" };
        if let Some(ink) = ink {
            go(ink, "mcInkAction", label)?;
        }
        go(a, MAIN, label)?;
        if on {
            put(a, "action_overlay.mcDpadoverlay", "_visible", Value::Bool(false))?;
        }
        self.visible = on;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::skate1_screens::Objective;
    use serde_json::json;
    use std::path::PathBuf;

    pub(crate) fn asset_root() -> PathBuf {
        PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"))
    }
    /// A tutorial movie as skate1_screens loads it.
    pub(crate) fn movie(stem: &str) -> Screen {
        let file = asset_root().join(super::super::TUTORIAL_MOVIES).join(format!("{stem}.json"));
        let mut screen = Screen::load(&file, &[]).unwrap();
        crate::skate1_online_hud::bind(&mut screen).unwrap();
        screen
    }
    pub(crate) fn get(s: &Screen, path: &str, key: &str) -> Value {
        s.clip(path).map_or(Value::Undefined, |c| s.vm.get(c, key))
    }
    pub(crate) fn text_of(s: &Screen, path: &str) -> String {
        match get(s, path, "text") {
            Value::Text(t) => t,
            other => format!("{other:?}"),
        }
    }

    /// hudobjective's Setup / UpdateObjectives / UpdateObjectiveResults / Play on the converted objectivehud.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn objective_hud_follows_hudobjective() {
        let mut s = movie("objectivehud");
        let mut card = Card { title: "Ollie".into(), ..Card::default() };
        card.objectives = vec![
            Objective { title: "Ollie 3 times".into(), result: "0/3".into(), done: false, checkbox: false },
            Objective { title: "Land it".into(), result: String::new(), done: false, checkbox: true },
        ];
        let mut o = ObjectiveHud::default();
        o.call(&mut s, "Setup", &[json!("tutorial")], &card, (0, 0)).unwrap();
        o.call(&mut s, "UpdateObjectives", &[], &card, (0, 0)).unwrap();
        assert_eq!(text_of(&s, "mcTextTitle.title100.labelText"), crate::apt_text::literal("Ollie"));
        assert_eq!(text_of(&s, "mcResult0.mcObjective.labelText"), crate::apt_text::literal("Ollie 3 times"));
        assert_eq!(text_of(&s, "mcResult0.mcFraction.labelText"), crate::apt_text::literal("0/3"));
        assert!(matches!(get(&s, "mcResult1", "_visible"), Value::Bool(true)));
        assert!(matches!(get(&s, "mcResult2", "_visible"), Value::Bool(false)));
        assert_eq!((o.kind, o.state), ([false, true, false], [false; 3]));
        // Done: success once (the state latches), not replayed.
        card.objectives[0].done = true;
        o.call(&mut s, "UpdateObjectiveResults", &[], &card, (0, 0)).unwrap();
        assert_eq!(o.state, [true, false, false]);
        // B4: success_fraction's frame 176 (label + 15) calls Audio.PlayObjectiveComplete, played as its export's
        // sfx/PlayObjectiveComplete.wav (apt_screen's Skate 1 Audio arm).
        for _ in 0..16 {
            s.advance().unwrap();
        }
        assert_eq!(std::mem::take(&mut s.bindings.sounds), ["PlayObjectiveComplete"]);
        // Lost: the open one fails, the done one stays; hudobjective plays PlayObjectiveFailed first.
        o.call(&mut s, "UpdateObjectiveResults", &[json!(true)], &card, (0, 0)).unwrap();
        assert_eq!(o.state, [true, false, false]);
        assert_eq!(std::mem::take(&mut s.bindings.sounds), ["PlayObjectiveFailed"]);
        // Its labels show it: hidden at load, shown by 'introInitiate' (frame 38's _visible = true).
        assert!(matches!(get(&s, "", "_visible"), Value::Bool(false)));
        o.call(&mut s, "Play", &[json!("introInitiate")], &card, (0, 0)).unwrap();
        s.advance().unwrap();
        assert!(matches!(get(&s, "", "_visible"), Value::Bool(true)));
    }

    /// RETAIL 0x830a4060: the gesture names by eSk8ControllerGesture (0x827b7558).
    #[test]
    fn gesture_names_follow_the_retail_table() {
        assert_eq!(gesture_name(0), "mcnone");
        assert_eq!(gesture_name(7), "mckickflip2", "KickFlip");
        assert_eq!(gesture_name(18), "mcollie", "Ollie");
        assert_eq!(gesture_name(31), "mc360spin", "Spin");
        assert_eq!(gesture_name(32), "mc360tweak", "Tweak");
        assert_eq!(gesture_name(55), "xbox_both_triggerslarge", "TriggerBothLarge");
        assert_eq!(gesture_name(56), "mcnone");
        assert_eq!(gesture_name(-1), "mcnone");
    }

    /// A tutorial scene's overlay track: Timing keys 60, Clear, 60 ... The screen opens once (gotoAndPlay(1)) and
    /// plays through the Clears; the scene's end closes it. Non-tutorial ids (a spot card) aren't this screen's.
    #[test]
    fn a_tutorial_scene_opens_its_nis_screen_once() {
        let mut n = Nis::default();
        assert_eq!(n.step(None), None);
        assert_eq!(n.step(Some(60)), Some(NisAct::Open("nis_timing_tutorial")));
        assert_eq!(n.step(Some(60)), None);
        assert_eq!(n.step(Some(0)), None, "Clear leaves it playing");
        assert_eq!(n.step(Some(60)), None, "no restart on the re-key");
        assert_eq!(n.step(Some(4)), None, "SpotFound isn't a tutorial screen");
        assert_eq!(n.step(None), Some(NisAct::Close));
        assert_eq!(n.step(None), None);
        assert_eq!(n.step(Some(58)), Some(NisAct::Open("nis_basics")));
        assert_eq!([55, 57, 70].map(nis_movie), [Some("nis_grind_tutorial"), Some("nis_airs_tutorial"), Some("nis_chaslam_tutorial_new")]);
        assert_eq!(nis_movie(10), None, "ProBioInfoOverlay_Left");
    }

    #[test]
    fn controller_sequences_name_the_movies_labels() {
        assert_eq!(controller_label(0), "none");
        assert_eq!(controller_label(2), "a");
        assert_eq!(controller_label(12), "triggersBoth");
        assert_eq!(controller_label(13), "none");
    }

    /// Every sequence's `<label>intro` / `<label>outro` exists in the converted controller movie (tutorial.swf).
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn controller_labels_are_the_tutorial_movies() {
        let file = asset_root().join(super::super::TUTORIAL_MOVIES).join("tutorial.json");
        let manifest = std::fs::read_to_string(file).unwrap();
        for seq in 1..=12 {
            let label = controller_label(seq);
            for end in ["intro", "outro"] {
                assert!(manifest.contains(&format!("\"{label}{end}\"")), "{label}{end}");
            }
        }
        // SetState 2 / 4 / 3 on the real movie, and 4 with "none" before any sequence (plays "noneoutro": no label).
        let mut s = movie("tutorial");
        let mut c = Controller::default();
        c.set_state(&mut s, 4, "none", "mcnone").unwrap();
        c.set_state(&mut s, 2, "none", "mcnone").unwrap();
        c.set_state(&mut s, 4, controller_label(2), "mcnone").unwrap();
        assert_eq!((c.visible, c.string), (true, "a"));
        c.set_state(&mut s, 3, "none", "mcnone").unwrap();
        assert!(!c.visible);
    }

    /// The lessons' controller pictures, rastered to $SKATE1_SHOTS/controller-<label>.png (offline, no game): Active
    /// (SetState 1), then ShowController's 2 + 4, then a second of frames.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial and $SKATE1_SHOTS"]
    fn controller_snapshots() {
        let out = PathBuf::from(std::env::var("SKATE1_SHOTS").unwrap());
        let root = asset_root().join("private/hud-skate1/tutorial");
        // The lessons' ShowController pairs (scripts): tutorial01 (RightAnalogGesture, KickFlip), tutorial02
        // (LeftAndBottomButton, None), tutorial04's CtrlButton / CtrlGesture (BothTriggers, TriggerBothLarge; LeftAnalog,
        // Spin; RightAnalogTrigger, Tweak), Danny Way's tutorial03 (LeftAnalogQuick, Pumping; ChristAir, ChristAir).
        for (seq, gesture) in [(8, 7), (3, 0), (12, 55), (4, 31), (7, 32), (5, 22), (9, 30)] {
            let mut s = movie("tutorial");
            let mut c = Controller::default();
            for state in [1, 2, 4] {
                c.set_state(&mut s, state, controller_label(seq), gesture_name(gesture)).unwrap();
                s.advance().unwrap();
            }
            // B2: SetGestureType attached the gesture's clip (the circle was empty: no gesture name reached it).
            let item = get(&s, &format!("{GESTURE}.gestureitem"), "_name");
            assert!(!matches!(item, Value::Undefined), "{} {}", controller_label(seq), gesture_name(gesture));
            for _ in 0..60 {
                s.advance().unwrap();
            }
            let draws = s.draws().unwrap();
            let name = format!("controller-{}-{}.png", controller_label(seq), gesture_name(gesture));
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join(name));
        }
    }

    /// B3: hudink's Play('objective', 'intro') on inkbottomright shows its mInk (hidden at load; 'intro' frame 9 sets
    /// _visible), and 'outro' hides it again at its end (frame 76).
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn objective_ink_plays_intro_and_outro() {
        let mut s = movie("inkbottomright");
        s.advance().unwrap();
        assert!(matches!(get(&s, "mInk", "_visible"), Value::Bool(false)));
        go(&mut s, "mInk", "intro").unwrap();
        s.advance().unwrap();
        assert!(matches!(get(&s, "mInk", "_visible"), Value::Bool(true)));
        assert!(!s.draws().unwrap().is_empty(), "the splatter draws");
        go(&mut s, "mInk", "outro").unwrap();
        for _ in 0..60 {
            s.advance().unwrap();
        }
        assert!(matches!(get(&s, "mInk", "_visible"), Value::Bool(false)));
    }

    /// B1: a NIS tutorial screen holds frame 1 until SetState(1) (tutorialscreenimp: gotoAndStop(1) / gotoAndPlay(1)),
    /// then plays its timeline; snapshots at nis_basics' labels go to $SKATE1_SHOTS when set.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn nis_tutorial_screens_play_from_set_state_1() {
        let shots = std::env::var("SKATE1_SHOTS").ok().map(PathBuf::from);
        let root = asset_root().join("private/hud-skate1/tutorial");
        for id in [55, 57, 58, 60] {
            let stem = nis_movie(id).unwrap();
            let mut s = movie(stem);
            s.advance().unwrap();
            let frame = |s: &Screen| get(s, "mcObject", "_currentframe").number();
            let held = frame(&s);
            s.advance().unwrap();
            assert_eq!(frame(&s), held, "{stem}: held before SetState");
            s.call("SetState", &[json!(1)]).unwrap();
            for n in 1..=1240 {
                s.advance().unwrap();
                if let (Some(dir), true) = (&shots, matches!(n, 400 | 500 | 1140 | 1240)) {
                    crate::core_menu_runtime::tests::raster_draws(&s.draws().unwrap(), &root, &dir.join(format!("{stem}-{n}.png")));
                }
            }
            assert!(frame(&s) > held + 1000., "{stem}: plays ({} -> {})", held, frame(&s));
        }
    }

    /// hud_action_overlay's states on actionoverlay + inkaction: the sign-up prompt shows its label, Off hides it.
    #[test]
    #[ignore = "requires $SKATE3_ASSET_ROOT/private/hud-skate1/tutorial"]
    fn action_overlay_states() {
        let (mut a, mut ink) = (movie("actionoverlay"), movie("inkaction"));
        // button_item's "Xbox_" art: PlatformCurrent isn't Platform_PS3 (playtest 10-15: Triangle on an Xbox pad).
        let global = |k| a.vm.get(a.vm.global, k);
        assert_eq!(global("PlatformCurrent"), global("Platform_XBOX360"));
        assert_ne!(global("PlatformCurrent"), global("Platform_PS3"));
        // Run12: the prompt's Y (main_mc.mcDpad's button_item, on the first frame) constructs during the load, so the
        // globals must be set before it: Xbox_Y, not PS3_Y's Triangle.
        let m = &a.bindings.movie;
        let placed = |export: &str| m.instances.values().any(|i| i.character == m.exports[export]);
        assert!(placed("Xbox_Y") && !placed("PS3_Y"), "the sign-up prompt's Y is the Xbox art");
        let mut action = Action::default();
        action.set_state(&mut a, Some(&mut ink), 14, "chal_tutorial").unwrap();
        assert!(action.visible);
        assert_eq!(text_of(&a, LABEL), "ID_CHALLENGE_COMMON_SIGNUP");
        action.set_state(&mut a, Some(&mut ink), 13, "").unwrap();
        assert_eq!(text_of(&a, LABEL), "ID_COMMON_INCOMING_CALL_CAPS");
        action.set_state(&mut a, None, 0, "").unwrap();
        assert!(!action.visible);
    }
}
