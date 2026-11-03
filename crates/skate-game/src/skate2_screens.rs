//! The Skate 2 tutorial's own Flash screens (skate2port/old_screens.py, run by apt_screen) for what
//! skate2_challenge.rs shows: the objective box (hudobjective2), the instruction box (hudtutorial with its
//! animated stick or button guide; the disc's `tutorial` movie is a dev placeholder, "#TODO: I WILL BE SOME KINDA
//! INSTRUCTION", that retail never shows), subtitles (no letterbox: retail's tutorial scenes are full frame), the lesson card (challenge_objectives), the sign-up
//! poster (challenge_signup), the central message (hudcentral) and the yes/no popup (sk8popup). A prompt without an animated stick shows its disc trick guide (screens/trick_guides.json:
//! the vault's gesture/button slots) the way FillTrickData lays them out. Their sounds are Skate 2's own (apt_screen plays
//! screens/sfx), plus the two the game adds: the popup opening and the won lesson's end sting. An element whose screen fails (missing data, a script error) is reported not
//! shown, and the binding draws it as plain text instead.
use crate::{apt_screen::Screens, challenge::cutscene::Cutscene, config::Config, core_menu_runtime::Key};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use std::collections::{BTreeSet, HashMap};

const GAME: u8 = 2;
// Draw order is name order.
const PHOTOS: &str = "1-photos";
const SUBTITLE: &str = "2-subtitle";
const COUNTDOWN: &str = "4-countdown";
const DISTANCE: &str = "4-distance";
const TIMER: &str = "4-timer";
const OBJECTIVE: &str = "3-objective";
const INSTRUCTION: &str = "5-hudtutorial";
const CARD: &str = "6-card";
const SIGNUP: &str = "6-signup";
const RESULT: &str = "7-result";
const SKATE101: &str = "7-skate101";
const SIGNUPHUD: &str = "5-signuphud";
/// SignUpHUD's kActionOverlayState_Off: retail's SignUpHUD component hides it with SetState(0) (s2.pe 0x82589abc).
/// Not 255: the movie's other hide value is PushByte 0xFF, -1 in EA's APT (signed), so 255 reran button_intro.
const SIGN_UP_OFF: i64 = 0;
const UNLOCK: &str = "7-unlock";
/// How long a closing screen's outro may run before it closes anyway (seconds); it normally ends on
/// ScreenManager.OutroComplete.
const OUTRO_TIMEOUT: f32 = 2.;
const POPUP: &str = "8-popup";
const OVERLAY: &str = "9-overlay";
const GUIDES: &str = "private/skate2-tutorial/screens/trick_guides.json";
/// Retail shows the objective box turned ~4° clockwise (measured off footage; nothing in hudobjective2 turns it, so
/// the game places it so). (degrees, pivot on the 1280x720 stage)
pub(crate) const OBJECTIVE_TILT: (f32, [f32; 2]) = (4.2, [1080., 520.]);

/// What the binding wants on screen; all text already localized.
#[derive(Default, Clone, PartialEq)]
pub(crate) struct View {
    /// (lesson title, objective line)
    pub objective: Option<(String, String)>,
    /// (gesture animation from presentation event 0x14, text)
    pub instruction: Option<(String, String)>,
    pub subtitle: Option<String>,
    /// (title, description) as a lesson starts.
    pub card: Option<(String, String)>,
    /// The sign-up poster (challenge_signup): title, description, and its X option's label when the lesson has a
    /// tutorial movie.
    pub signup: Option<(String, String, Option<String>)>,
    /// A central message (hudcentral): text, lesson title, and which of the game's calls showed it.
    pub result: Option<(String, String, Central)>,
    pub popup: Option<Popup>,
    /// The skater's natural stance is goofy: hudtutorial mirrors its trick gestures for a regular skater.
    pub goofy: bool,
    /// HUD2.StartCountdown's CountdownHUD (hudcountdown): (its timer, whole seconds left; 0 is "Go!").
    pub countdown: Option<(i64, i64)>,
    /// The TimerHUD (hudtimer) while shown: seconds left on its timer.
    pub timer: Option<f64>,
    /// The DistanceMeter (huddistance) while shown: HUD2.SetupDistanceHUDData's (limit, metres).
    pub distance: Option<(f64, f64)>,
    /// FrontEnd.OpenPhotoSelection's pick (photoselect): (highlighted photo, photos, the last step was "next").
    pub photos: Option<(usize, usize, bool)>,
    /// SignUpHUD (actionoverlay2) in an open lesson's sign-up box: the lesson's title.
    pub sign_up: Option<String>,
    /// FE state 10's UnlockOverlay: the unlocked characters' text labels.
    pub unlocks: Option<Vec<String>>,
    /// FE state 79's Skate101: (title, text).
    pub skate101: Option<(String, String)>,
}
/// Which HUD2 call put up a central message: hudcentral colours it and plays its audio hook by this.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub(crate) enum Central {
    /// ShowCentralMessage / ShowModifiedCentralMessage.
    #[default]
    Plain,
    /// ShowChallengeCompleteMessage.
    Complete,
    /// ShowChallengeFailedMessage.
    Failed,
}
impl Central {
    /// hudcentral's ShowMessage inputs: the message colour (0 off-white, 1 red, else green), the audio hook
    /// (kAUDIOHOOK_DEFAULT 0, _FAIL 1, _SUCCESS 2), and the end sting the game plays itself (0x8252E3A8).
    fn presentation(self) -> (u8, u8, Option<&'static str>) {
        match self {
            Self::Plain => (0, 0, None),
            Self::Complete => (0, 2, Some("challenge_end_success")),
            Self::Failed => (1, 1, Some("challenge_end_fail")),
        }
    }
}
/// hudtutorial's two stance natives, which its gesture_item reads interchangeably (IsRegular == !Flipped):
/// Skate 2's native (0x82508F00) answers from the profile's natural stance.
fn stance_answers(goofy: bool) -> [(&'static str, Json); 2] {
    [("Tricks.IsStanceTrickGesturesFlipped", json!(goofy)), ("HUDComponents.PlayerStance_IsRegular", json!(!goofy))]
}
#[derive(Clone, PartialEq)]
pub(crate) struct Popup {
    pub title: String,
    pub body: String,
    pub options: Vec<String>,
}

/// Which View elements a screen is showing; the binding draws the rest as text.
#[derive(Default, Clone, Copy)]
pub(crate) struct Shown {
    pub objective: bool,
    pub instruction: bool,
    pub subtitle: bool,
    pub card: bool,
    pub signup: bool,
    pub result: bool,
    pub popup: bool,
    pub countdown: bool,
    pub timer: bool,
    pub distance: bool,
    pub photos: bool,
    pub unlocks: bool,
    pub skate101: bool,
    pub sign_up: bool,
}

/// A popup answer: the option index, or `None` for cancel (B).
pub(crate) type Choice = Option<usize>;

#[derive(Default)]
pub(crate) struct TutorialScreens {
    last: View,
    shown: Shown,
    /// Screens that failed once stay text for the session.
    failed: BTreeSet<&'static str>,
    highlight: usize,
    /// Prompt gesture -> flat [gesture, button, connector, ...] slots; loaded once.
    guides: Option<HashMap<String, Vec<String>>>,
    /// The scene's HUD overlay id, and when (real seconds) a cleared overlay's outro is over.
    overlay: (u8, Option<f32>),
    /// When (real seconds) a picked photo's fakeoutro is over and photoselect closes.
    photos_outro: Option<f32>,
    /// Screens playing their root "outro" before they close: (layer, give-up time in real seconds).
    outros: Vec<(&'static str, f32)>,
    /// Countdown_ChallengeHasAudio answers false (a Death Race: challenge type 0x13 counts silently).
    pub(crate) silent_countdown: bool,
}

impl TutorialScreens {
    /// Brings the screens to `view`; returns what they show.
    pub(crate) fn sync(&mut self, world: &mut World, view: &View) -> Shown {
        let guides = self.guides.get_or_insert_with(|| {
            let path = world.resource::<Config>().asset_root.join(GUIDES);
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_else(|| {
                    warn!("Skate 2 tutorial screens: no trick guides at {}", path.display());
                    HashMap::new()
                })
        });
        let guide = view.instruction.as_ref().and_then(|(gesture, _)| guides.get(gesture)).cloned();
        let overlay = world.get_resource::<Cutscene>().map_or(0, Cutscene::overlay);
        let now = world.resource::<Time<Real>>().elapsed_secs();
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return Shown::default() };
        let last = std::mem::replace(&mut self.last, view.clone());
        let mut shown = self.shown;
        if view.subtitle != last.subtitle {
            let line = view.subtitle.clone();
            shown.subtitle = self.run(&mut screens, SUBTITLE, |layer| {
                if layer.get(SUBTITLE).is_none() {
                    layer.open(SUBTITLE, "subtitle", &[])?;
                }
                let screen = layer.get(SUBTITLE).unwrap();
                match &line {
                    Some(text) => {
                        screen.answer("Game.GetMovieSubtitleString", json!(text));
                        screen.call("RefreshSubtitle", &[]).map(drop)
                    }
                    None => screen.call("Clear", &[]).map(drop),
                }
            }) && view.subtitle.is_some();
        }
        if view.objective != last.objective {
            let (now, before) = (view.objective.clone(), last.objective.clone());
            // A lesson's objective only moves on (next line, or n/N up) or clears with the result up when it was met.
            let met = match (&now, &before) {
                (Some((title, _)), Some((was, _))) => title == was,
                (None, Some(_)) => view.result.as_ref().is_some_and(|r| r.2 == Central::Complete),
                _ => false,
            };
            shown.objective = self.run(&mut screens, OBJECTIVE, |layer| {
                if let Some(screen) = layer.get(OBJECTIVE).filter(|_| met) {
                    // As the game marks objective 0 done: state 1, then SetState(4) runs UpdateObjectiveResultX(0, true)
                    // (green, success_check, FE_SOUND_TRICK_SUCCESSFUL).
                    screen.answer("HUDComponents.Objective_GetObjectiveState(0)", json!(1));
                    screen.call("SetState", &[json!(4)])?;
                }
                match (&now, &before) {
                    (Some((title, line)), _) => {
                        let answers = objective_answers(title, line);
                        if before.is_some() && layer.get(OBJECTIVE).is_some() {
                            // The same lesson's next objective: refresh in place.
                            let screen = layer.get(OBJECTIVE).unwrap();
                            for (k, v) in answers {
                                screen.answer(k, v);
                            }
                            screen.call("SetState", &[json!(3)]).map(drop)
                        } else {
                            // SetState(1) only plays the intro; the text is filled by UpdateObjectivesX, which state 3 runs.
                            let screen = layer.open(OBJECTIVE, "hudobjective2", &answers)?;
                            screen.tilt(OBJECTIVE_TILT.0, OBJECTIVE_TILT.1);
                            screen.call("SetState", &[json!(1)])?;
                            // Twice: a row's first jump to "fraction" swaps in a new label (the text just set is lost);
                            // from there on the row keeps it.
                            screen.call("SetState", &[json!(3)])?;
                            screen.call("SetState", &[json!(3)]).map(drop)
                        }
                    }
                    (None, Some(_)) => layer.get(OBJECTIVE).map_or(Ok(()), |s| s.call("SetState", &[json!(2)]).map(drop)),
                    (None, None) => Ok(()),
                }
            }) && view.objective.is_some();
        }
        if view.goofy != last.goofy {
            info!("Skate 2 tutorial screens: natural stance {}, trick gestures flipped {}", if view.goofy { "goofy" } else { "regular" }, view.goofy);
        }
        if view.instruction != last.instruction {
            let (now, appearing) = (view.instruction.clone(), last.instruction.is_none());
            shown.instruction = self.run(&mut screens, INSTRUCTION, |layer| {
                if layer.get(INSTRUCTION).is_none() {
                    layer.open(INSTRUCTION, "hudtutorial", &[])?;
                }
                let screen = layer.get(INSTRUCTION).unwrap();
                for (k, v) in stance_answers(view.goofy) {
                    screen.answer(k, v);
                }
                match &now {
                    Some((gesture, text)) => {
                        screen.answer("HUDComponents.GetTutorialHalText", json!(text));
                        // The root timeline's hudintro/hudoutro slide the box in and out (outro ends hidden).
                        if appearing {
                            screen.play("hudintro")?;
                        }
                        screen.call("SetState", &[json!(246)])?;
                        let state = match (gesture_state(gesture), &guide) {
                            // FillTrickData reads [slot count, (gesture, button, connector) per slot]: its index
                            // starts at 1 and post-increments.
                            (11, Some(slots)) if !slots.is_empty() => {
                                let mut list = vec![json!(slots.len() / 3)];
                                list.extend(slots.iter().map(|s| json!(s)));
                                screen.answer("HUDComponents.GetTutorialGesturesButtons", Json::Array(list));
                                10
                            }
                            (state, _) => state,
                        };
                        screen.call("SetState", &[json!(state)]).map(drop)
                    }
                    None => screen.play("hudoutro"),
                }
            }) && view.instruction.is_some();
        } else if view.goofy != last.goofy && shown.instruction {
            // A stance change under a shown prompt: state 77 re-reads the stance for every gesture.
            self.run(&mut screens, INSTRUCTION, |layer| {
                let Some(screen) = layer.get(INSTRUCTION) else { return Ok(()) };
                for (k, v) in stance_answers(view.goofy) {
                    screen.answer(k, v);
                }
                screen.call("SetState", &[json!(77)]).map(drop)
            });
        }
        if view.card != last.card {
            let now = view.card.clone();
            shown.card = self.run(&mut screens, CARD, |layer| {
                layer.close(CARD);
                if let Some((title, description)) = &now {
                    layer.open(CARD, "challenge_objectives", &mission_answers(title, description))?;
                }
                Ok(())
            }) && view.card.is_some();
        }
        if view.signup != last.signup {
            let now = view.signup.clone();
            shown.signup = self.run(&mut screens, SIGNUP, |layer| {
                // ponytail: it closes outright (under the accept's flash); its 12-frame "outro" isn't played.
                layer.close(SIGNUP);
                if let Some((title, description, movie)) = &now {
                    layer.open(SIGNUP, "challenge_signup", &signup_answers(title, description, movie.as_deref()))?.play("intro")?;
                }
                Ok(())
            }) && view.signup.is_some();
        }
        if view.result != last.result {
            let now = view.result.clone();
            shown.result = self.run(&mut screens, RESULT, |layer| {
                layer.close(RESULT);
                // HUD2.ShowCentralMessage is CentralHUD (hud2/hudcentral): SetState(4) shows Central_GetMessageData's
                // [text, colour] centred and plays its audio hook (Central::presentation). Not endchallenge_small:
                // its state 5 is a challenge grade ("Good Enough"), which retail doesn't show here.
                if let Some((banner, _title, kind)) = &now {
                    let (colour, hook, sting) = kind.presentation();
                    let answers = [
                        ("HUDComponents.Central_GetMessageData", json!([banner, colour])),
                        ("HUDComponents.Central_GetAudioHook", json!(hook)),
                    ];
                    let screen = layer.open(RESULT, "hudcentral", &answers)?;
                    screen.call("SetState", &[json!(1)])?;
                    screen.call("SetState", &[json!(4)])?;
                    // The game's own end sting (a single-round challenge won or lost, 0x8252E3A8); the movie plays none.
                    screen.bindings.sounds.extend(sting.map(String::from));
                }
                Ok(())
            }) && view.result.is_some();
        }
        if view.popup != last.popup {
            let now = view.popup.clone();
            self.highlight = 0;
            shown.popup = self.run(&mut screens, POPUP, |layer| {
                layer.close(POPUP);
                if let Some(p) = &now {
                    let mut answers = vec![
                        ("Popup.GetTitle", json!(p.title)),
                        ("Popup.GetDescription", json!(p.body)),
                        ("Popup.GetNumOptions", json!(p.options.len())),
                        ("Popup.IsPopupCancelable", json!(p.options.len() > 1)),
                    ];
                    let keys: Vec<String> = (0..p.options.len()).map(|i| format!("Popup.GetOption({i})")).collect();
                    answers.extend(keys.iter().map(String::as_str).zip(p.options.iter().map(|o| json!(o))));
                    // The movie starts hidden and empty: the native popup shows it, plays its intro and has
                    // SetState(0) run Refresh (title, body, options, button help), as popup_runtime does for Skate 3.
                    let screen = layer.open(POPUP, "sk8popup", &answers)?;
                    screen.show()?;
                    screen.call("OnLoaded", &[])?;
                    screen.call("SetState", &[json!(0)])?;
                    // The native popup's open sound (0x8252B618); the movie only plays its nav and A sounds.
                    screen.bindings.sounds.push("core_popup".into());
                }
                Ok(())
            }) && view.popup.is_some();
        }
        // A frontend state's Exit closes its screen through vtable[13] (0x82b46940), which completes only on the
        // screen's OnScreenOutroComplete (Step 1b): the root's "outro" plays first.
        self.outros.retain(|&(name, until)| {
            let layer = screens.layer(GAME);
            let done = layer.get(name).is_none_or(|screen| screen.bindings.outro_complete) || now >= until;
            if done {
                layer.close(name);
            }
            !done
        });
        // SignUpHUD stays loaded: SetState(1) (kActionOverlayState_Challenge) shows the prompt with button_intro,
        // SIGN_UP_OFF hides it (Show(false), button_outro).
        if view.sign_up != last.sign_up {
            let title = view.sign_up.clone();
            shown.sign_up = self.run(&mut screens, SIGNUPHUD, |layer| match &title {
                Some(title) => {
                    if layer.get(SIGNUPHUD).is_none() {
                        // No OnLoaded: the constructor registers itself (RegisterScreen -> ScreenManager.OnLoaded).
                        layer.open(SIGNUPHUD, "actionoverlay2", &[])?;
                    }
                    let screen = layer.get(SIGNUPHUD).unwrap();
                    for (native, value) in sign_up_answers(title) {
                        screen.answer(native, value);
                    }
                    screen.call("SetState", &[json!(1)]).map(drop)
                }
                None => layer.get(SIGNUPHUD).map_or(Ok(()), |screen| screen.call("SetState", &[json!(SIGN_UP_OFF)]).map(drop)),
            }) && view.sign_up.is_some();
        }
        let mut closing = std::mem::take(&mut self.outros);
        if view.unlocks != last.unlocks {
            let want = view.unlocks.clone();
            shown.unlocks = self.run(&mut screens, UNLOCK, |layer| {
                match &want {
                    Some(characters) => {
                        closing.retain(|(name, _)| *name != UNLOCK);
                        layer.close(UNLOCK);
                        // Its constructor registers the screen and runs UpdateAnimation every frame; there is no OnLoaded.
                        layer.open(UNLOCK, "unlockoverlay", &unlock_answers(characters))?;
                    }
                    // State 10's A (0x82528280): core_a_button (Step 1e), then the close.
                    None => {
                        if let Some(screen) = layer.get(UNLOCK) {
                            screen.bindings.sounds.push("core_a_button".into());
                            screen.play("outro")?;
                            closing.push((UNLOCK, now + OUTRO_TIMEOUT));
                        }
                    }
                }
                Ok(())
            }) && view.unlocks.is_some();
        }
        // FE state 10's Update (0x82528220): SetState(50) every frame, the AS's UpdateAnimation.
        if shown.unlocks {
            self.run(&mut screens, UNLOCK, |layer| layer.get(UNLOCK).map_or(Ok(()), |s| s.call("SetState", &[json!(50)]).map(drop)));
        }
        if view.skate101 != last.skate101 {
            let want = view.skate101.clone();
            shown.skate101 = self.run(&mut screens, SKATE101, |layer| {
                match &want {
                    Some((title, text)) => {
                        closing.retain(|(name, _)| *name != SKATE101);
                        layer.close(SKATE101);
                        let screen = layer.open(SKATE101, "skate101", &skate101_answers(title, text))?;
                        screen.call("OnLoaded", &[])?;
                        // The constructor stops the root on frame 1; the intro runs when the owner plays it.
                        screen.play("intro")?;
                    }
                    None => {
                        if let Some(screen) = layer.get(SKATE101) {
                            screen.play("outro")?;
                            closing.push((SKATE101, now + OUTRO_TIMEOUT));
                        }
                    }
                }
                Ok(())
            }) && view.skate101.is_some();
        }
        self.outros = closing;
        if view.countdown != last.countdown {
            let (now, acts) = (view.countdown, countdown_acts(last.countdown, view.countdown));
            let audio = !self.silent_countdown;
            shown.countdown = self.run(&mut screens, COUNTDOWN, |layer| {
                if now.is_some() && !acts.is_empty() && layer.get(COUNTDOWN).is_none() {
                    layer.open(COUNTDOWN, "hudcountdown", &countdown_answers(audio))?;
                }
                layer.get(COUNTDOWN).map_or(Ok(()), |screen| apply(screen, acts))
            }) && now.is_some();
        }
        if view.timer != last.timer {
            let (now, acts) = (view.timer, timer_acts(last.timer, view.timer));
            shown.timer = self.run(&mut screens, TIMER, |layer| {
                if now.is_some() && layer.get(TIMER).is_none() {
                    layer.open(TIMER, "hudtimer", &[])?;
                }
                layer.get(TIMER).map_or(Ok(()), |screen| apply(screen, acts))
            }) && now.is_some();
        }
        if view.distance != last.distance {
            let (now, before) = (view.distance, last.distance);
            shown.distance = self.run(&mut screens, DISTANCE, |layer| match now {
                Some((limit, metres)) => {
                    if layer.get(DISTANCE).is_none() {
                        layer.open(DISTANCE, "huddistance", &[])?;
                    }
                    let screen = layer.get(DISTANCE).unwrap();
                    if before.is_none() {
                        screen.call("SetState", &[json!(1)])?;
                    }
                    let text = |id: &str| screen.bindings.movie.text_assets.localize(id);
                    let data = distance_data(limit, metres, crate::hud_runtime::imperial(), text);
                    screen.answer("HUDComponents.DistanceMeter_GetData", data);
                    screen.call("SetState", &[json!(3)]).map(drop)
                }
                None => layer.get(DISTANCE).map_or(Ok(()), |s| s.call("SetState", &[json!(2)]).map(drop)),
            }) && now.is_some();
        }
        if view.photos != last.photos {
            let (now, before, picked) = (view.photos, last.photos, self.photos_outro.is_some());
            shown.photos = self.run(&mut screens, PHOTOS, |layer| match (now, before) {
                // OpenPhotoSelection (0x826E4FD0): a choice only with more than one photo (else "Continue", no
                // arrows); the open selects photo 0 (SetState(index) lights its slot in the strip of 8).
                (Some((index, count, _)), None) => {
                    let answers = [
                        ("HUDComponents.PhotoSelect_IsSelectionAvail", json!(count > 1)),
                        ("HUDComponents.PhotoSelect_GetNumPhotos", json!(count)),
                    ];
                    let screen = layer.open(PHOTOS, "photoselect", &answers)?;
                    screen.call("OnLoaded", &[])?;
                    screen.call("SetState", &[json!(index)]).map(drop)
                }
                (Some(_), Some(_)) => layer.get(PHOTOS).map_or(Ok(()), |screen| apply(screen, photo_acts(before, now))),
                // Closed without a pick (the port's retry): at once. A pick runs `pick_photo` first.
                (None, _) => {
                    if !picked {
                        layer.close(PHOTOS);
                    }
                    Ok(())
                }
            }) && now.is_some();
        }
        if self.photos_outro.is_some_and(|end| now >= end) {
            screens.layer(GAME).close(PHOTOS);
            self.photos_outro = None;
        }
        if overlay != self.overlay.0 {
            let closing = self.overlay.1.is_some();
            self.overlay = (overlay, None);
            let open = self.run(&mut screens, OVERLAY, |layer| match overlay {
                // TutorialGesture_OlliePt1/Pt2: gestollie's kScreenState_OlliePt1/2 (the stick down, then up).
                80 | 81 => {
                    if closing || layer.get(OVERLAY).is_none() {
                        layer.close(OVERLAY);
                        layer.open(OVERLAY, "gestollie", &[])?;
                    }
                    layer.get(OVERLAY).unwrap().call("SetState", &[json!(overlay - 79)]).map(drop)
                }
                // ponytail: Clear, and the overlays the tutorial never keys (sponsor logos, photos, ...), fade out.
                _ => layer.get(OVERLAY).map_or(Ok(()), |screen| screen.play("outro")),
            });
            // The outro is 30 frames at 60 fps, and its last frame leaves the backer up: the owner closes it.
            if open && !matches!(overlay, 80 | 81) {
                self.overlay.1 = Some(now + 0.5);
            }
        }
        if self.overlay.1.is_some_and(|end| now >= end) {
            screens.layer(GAME).close(OVERLAY);
            self.overlay.1 = None;
        }
        self.shown = shown;
        shown
    }

    pub(crate) fn popup_shown(&self) -> bool {
        self.shown.popup
    }

    /// UnlockOverlay's A is live once its cash count is done (FEUnlock.Cash_SetCashAnimDone, Step 1a), or at once
    /// when the screen isn't showing (the text stand-in).
    pub(crate) fn unlocks_ready(&mut self, world: &mut World) -> bool {
        if !self.shown.unlocks {
            return true;
        }
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return true };
        let Some(screen) = screens.layer(GAME).get(UNLOCK) else { return true };
        screen.take_calls().iter().any(|(call, _)| call == "FEUnlock.Cash_SetCashAnimDone")
    }

    /// Forwards a press to the open popup; Some(choice) once it picks (A on an option, or B to cancel).
    pub(crate) fn popup_input(&mut self, world: &mut World, key: Key) -> Option<Choice> {
        if !self.shown.popup {
            return None;
        }
        let mut screens = world.get_resource_mut::<Screens>()?;
        let screen = screens.layer(GAME).get(POPUP)?;
        if let Err(error) = screen.input(key) {
            warn!("Skate 2 tutorial screens: popup input: {error}");
        }
        let mut picked = None;
        for (call, args) in screen.take_calls() {
            let index = args.get(1).and_then(Json::as_u64).map(|i| i as usize);
            match (call.as_str(), index) {
                ("Game.OnMenuHilight", Some(i)) => self.highlight = i,
                ("Game.OnMenuSelect", Some(255)) => picked = Some(None),
                ("Game.OnMenuSelect", Some(i)) => picked = Some(Some(i)),
                _ => {}
            }
        }
        // The popup never reported a pick: A takes the highlighted option, B cancels.
        picked.or(match key {
            Key::Accept => Some(Some(self.highlight)),
            Key::Back => Some(None),
            _ => None,
        })
    }

    /// The photo pick's A (0x8254B4A8): the take-photo button glows, the selection runs its fakeoutro and photo_pick
    /// plays; photoselect closes once the outro is over, whatever the view says meanwhile.
    pub(crate) fn pick_photo(&mut self, world: &mut World) {
        if !self.shown.photos {
            return;
        }
        let now = world.resource::<Time<Real>>().elapsed_secs();
        let Some(mut screens) = world.get_resource_mut::<Screens>() else { return };
        let picked = self.run(&mut screens, PHOTOS, |layer| {
            let Some(screen) = layer.get(PHOTOS) else { return Ok(()) };
            screen.call("SetState", &[json!(253)])?;
            screen.call("SetState", &[json!(246)])?;
            screen.bindings.sounds.push("photo_pick".into());
            Ok(())
        });
        if picked {
            self.photos_outro = Some(now + PHOTOS_OUTRO);
        }
    }

    /// A picked photo's fakeoutro is playing.
    pub(crate) fn picking(&self) -> bool {
        self.photos_outro.is_some()
    }

    /// Closes everything (tutorial stopped or the map changed).
    pub(crate) fn clear(&mut self, world: &mut World) {
        if let Some(mut screens) = world.get_resource_mut::<Screens>() {
            let layer = screens.layer(GAME);
            for name in [PHOTOS, SUBTITLE, COUNTDOWN, DISTANCE, TIMER, OBJECTIVE, INSTRUCTION, CARD, SIGNUP, RESULT, POPUP, OVERLAY, UNLOCK, SKATE101, SIGNUPHUD] {
                layer.close(name);
            }
        }
        (self.last, self.shown, self.highlight, self.overlay, self.photos_outro) = (View::default(), Shown::default(), 0, (0, None), None);
        self.outros.clear();
    }

    /// Runs one screen's update; a failure closes it and leaves the element to the binding's text from now on.
    fn run(
        &mut self,
        screens: &mut Screens,
        name: &'static str,
        update: impl FnOnce(&mut crate::apt_screen::Layer) -> Result<(), String>,
    ) -> bool {
        if self.failed.contains(name) {
            return false;
        }
        let layer = screens.layer(GAME);
        match update(layer) {
            Ok(()) => true,
            Err(error) => {
                warn!("Skate 2 tutorial screens: {name}: {error}; plain text stands in");
                layer.close(name);
                self.failed.insert(name);
                false
            }
        }
    }
}

/// One thing sync does to a screen.
#[derive(Debug, PartialEq)]
enum Act {
    Answer(&'static str, Json),
    State(u32),
    Sound(&'static str),
}
fn apply(screen: &mut crate::apt_screen::Screen, acts: Vec<Act>) -> Result<(), String> {
    for act in acts {
        match act {
            Act::Answer(native, value) => screen.answer(native, value),
            Act::State(state) => drop(screen.call("SetState", &[json!(state)])?),
            Act::Sound(key) => screen.bindings.sounds.push(key.into()),
        }
    }
    Ok(())
}

/// The countdown component (0x825892C8 / 0x825891B8) from one view to the next: a start shows its first second if it
/// has one (the photo challenges' 0 s countdown shows nothing), then SetState(2) again each time the whole seconds
/// change, down to Go!; there it stops and the movie's intro plays out. Show(false) is SetState(1).
fn countdown_acts(before: Option<(i64, i64)>, now: Option<(i64, i64)>) -> Vec<Act> {
    let Some((timer, left)) = now else { return vec![Act::State(1)] };
    let show = match before {
        Some((was, shown)) if was == timer => left != shown,
        _ => left > 0,
    };
    if !show {
        return Vec::new();
    }
    vec![Act::Answer("HUDComponents.Countdown_GetCountdownTime", json!(countdown_text(left))), Act::State(2)]
}

/// The TimerHUD (0x82588710) from one view to the next: Show(true) is hudintro (SetState(1)); under 10 s each new
/// second plays timer_end and flashes (4); climbing back over 10 s (time added) returns to normal (5); a new second
/// refreshes the text (3), red under 10 s. Show(false) is hudoutro (2), which ends hidden.
/// ponytail: run on every change, not every 5th frame (kUpdateCycles); a second is never missed.
fn timer_acts(before: Option<f64>, now: Option<f64>) -> Vec<Act> {
    let Some(left) = now else { return vec![Act::State(2)] };
    let mut acts = Vec::new();
    if before.is_none() {
        acts.push(Act::State(1));
    }
    // The component's stored time starts at 0.0 (its constructor, 0x8257EB20), so a first show over 10 s sends 5 too.
    // ponytail: retail keeps the last time across a hide; a reshow here starts from 0.0 again.
    let prev = before.unwrap_or(0.);
    let changed = prev.ceil() != left.ceil();
    if left < TIMER_WARNING && changed {
        acts.extend([Act::Sound("timer_end"), Act::State(4)]);
    } else if prev < TIMER_WARNING && left > TIMER_WARNING {
        acts.push(Act::State(5));
    }
    if changed {
        acts.push(Act::Answer("HUDComponents.Timer_GetData", json!([timer_text(left), u8::from(left >= TIMER_WARNING)])));
        acts.push(Act::State(3));
    }
    acts
}

/// photoselect's input (0x8254B4A8) after a step: photo_choose, the arrow lit (255 next, 254 previous), then the new
/// slot. Nothing when the highlighted photo didn't move.
fn photo_acts(before: Option<(usize, usize, bool)>, now: Option<(usize, usize, bool)>) -> Vec<Act> {
    match (before, now) {
        (Some((was, ..)), Some((index, _, forward))) if index != was => {
            vec![Act::Sound("photo_choose"), Act::State(if forward { 255 } else { 254 }), Act::State(index as u32)]
        }
        _ => Vec::new(),
    }
}

/// Under this many seconds the TimerHUD runs red, flashing and ticking (0x8204969C).
const TIMER_WARNING: f64 = 10.;
/// photoselect's fakeoutro, after which the native closes it (seconds).
const PHOTOS_OUTRO: f32 = 0.5;

/// hudcountdown's natives besides the time: Countdown_ChallengeHasAudio (0x825828C0) is false only for challenge types
/// 0, 4, 0x13, 0x1E and 0x20, so the race (23) and spot photo (38) count aloud (challenge_count); no tutorial
/// challenge is in a stadium; the Go! graphic is the frame named for the language. `audio`: false for a Death Race (0x13).
fn countdown_answers(audio: bool) -> [(&'static str, Json); 3] {
    [
        ("HUDComponents.Countdown_ChallengeHasAudio", json!(audio)),
        ("HUDComponents.Countdown_IsInStadium", json!(false)),
        ("FELanguage.GetLanguage", json!("English")),
    ]
}

/// Countdown_GetCountdownTime (0x82582808): "#%d" while seconds are left, then ID_MISSION_COUNTDOWN_GO.
pub(crate) fn countdown_text(left: i64) -> String {
    if left > 0 { format!("#{left}") } else { "Go!".into() }
}

/// The TimerHUD text (0x82588710 through the time formatter 0x827AD440, fields {2,2,2,2} with ':' separators, flag
/// 0x10): whole seconds rounded up, "#MM:SS" from a minute, else "#SS", and an empty "#" at zero.
pub(crate) fn timer_text(left: f64) -> String {
    let t = left.max(0.).ceil() as i64;
    match (t / 60, t % 60) {
        (0, 0) => "#".into(),
        (0, s) => format!("#{s:02}"),
        (m, s) => format!("#{m:02}:{s:02}"),
    }
}

/// DistanceMeter_GetData (0x825818B0) as the meter's update (0x82589018) fills it: "%.1f" formatted through
/// ID_COMMON_DISTANCE's {0:distance} (0x8246F4F0 with Settings > Units: "%s %s", metres or x3.0 feet, and the unit's
/// text), and red (0) once the skater is past 0.6 of the limit (0x8209F554), else white (1).
pub(crate) fn distance_data(limit: f64, metres: f64, imperial: bool, text: impl Fn(&str) -> String) -> Json {
    let red = limit >= 0. && metres / limit > 0.6;
    let shown = crate::hud_runtime::distance(crate::hud_runtime::skate2_number(&format!("{metres:.1}")), 1, imperial, text);
    json!([shown, u8::from(!red)])
}

/// hudtutorial's SetState for an instruction: the animated stick for the tricks it has one for, else 11 (text only,
/// or 10 with the trick guide when the gesture has one).
fn gesture_state(gesture: &str) -> u32 {
    match gesture {
        "ollie" => 12,
        "nollie" => 13,
        "kickflip" => 14,
        "popshuvit" | "fspopshuvit" => 15,
        "360popshuvit" => 16,
        _ => 11,
    }
}

/// SetTypeState reads state 0 failed (red), 1 done (green), 2 in progress (off-white), and a nonzero type as the
/// checkbox circle (success_check / fail_check once decided) instead of a fraction.
fn objective_answers(title: &str, line: &str) -> Vec<(&'static str, Json)> {
    vec![
        ("HUDComponents.Objective_GetNumObjectives", json!(1)),
        ("HUDComponents.Objective_GetObjectiveTitle(0)", json!(line)),
        ("HUDComponents.Objective_GetObjectiveState(0)", json!(2)),
        ("HUDComponents.Objective_GetObjectiveType(0)", json!(1)),
        ("HUDComponents.Objective_GetObjectiveValue(0)", json!("")),
        ("HUDComponents.Objective_GetActivityTitle", json!(title)),
        ("HUDComponents.Objective_GetActivityType", json!("")),
    ]
}

/// challenge_signup in its sign-up mode (SetMode(2)): free, no betting, only the description; A Accept / B Decline
/// and, with a tutorial movie, X and the movie's label.
pub(crate) fn signup_answers(title: &str, description: &str, movie: Option<&str>) -> Vec<(&'static str, Json)> {
    vec![
        ("SignUpPoster.GetSignUpTitle", json!(title)),
        ("SignUpPoster.GetSignUpDescription", json!(description)),
        ("SignUpPoster.HasTutorialNIS", json!(movie.is_some())),
        ("SignUpPoster.GetTutorialNISHalName", json!(movie.unwrap_or_default())),
        ("SignUpPoster.EntryFee_GetFee", json!(0)),
        ("SignUpPoster.IsBettingChallenge", json!(false)),
        ("Mission.IsDescriptionEnabled", json!(true)),
        ("Mission.IsObjectivesEnabled", json!(false)),
        ("Mission.IsCompetitorsEnabled", json!(false)),
    ]
}

/// SignUpHUD's Challenge state reads the lesson's title (its label and glow); the rest is the movie's own.
pub(crate) fn sign_up_answers(title: &str) -> Vec<(&'static str, Json)> {
    vec![("HUDComponents.SignUp_GetChallengeTitle", json!(title))]
}

/// FEUnlock as FE state 10 answers it for a tutorial win: the characters it unlocked (their text labels, Step 1c), no
/// sponsors, no cash. Everything else stays undefined, which the AS reads as false; it forces Trophy and PhotoPath false
/// itself.
pub(crate) fn unlock_answers(characters: &[String]) -> Vec<(&'static str, Json)> {
    vec![
        ("FEUnlock.Character_Unlocked", json!(!characters.is_empty())),
        ("FEUnlock.Character_GetUnlockedList", json!(characters)),
        ("FEUnlock.Sponsors_GetNumSponsorsToCycle", json!(0)),
        ("FEUnlock.Sponsors_GetCurrentSponsorsArray", json!([])),
        // UNTRACED (Step 1d): the cash rows' vault fields weren't found; a tutorial lesson shows none here.
        ("FEUnlock.Cash_GetChallengeCashPrize", json!(0)),
        ("FEUnlock.Cash_GetChallengeCashPurse", json!(0)),
        ("FEUnlock.Cash_GetChallengeObjBonusCash", json!(0)),
        ("FEUnlock.Cash_GetWagerPot", json!(0)),
        ("FEUnlock.Cash_GetSponsorBonus", json!(0)),
        ("FEUnlock.Cash_GetTrickCalloutBonus", json!(0)),
    ]
}

/// Skate101 as FE state 79 fills it (0x8250fc18 copies TitleHALID / TextHALID): the title goes through
/// FELanguage.GetLocalizedFormatStringCustom(title, 'x'), which apt_screen doesn't fill by itself.
pub(crate) fn skate101_answers(title: &str, text: &str) -> Vec<(&'static str, Json)> {
    vec![
        ("Mission.GetSkate101TitleString", json!(title)),
        ("FELanguage.GetLocalizedFormatStringCustom", json!(title)),
        ("Mission.GetSkate101DescriptionString", json!(text)),
    ]
}

fn mission_answers(title: &str, description: &str) -> Vec<(&'static str, Json)> {
    vec![
        ("Mission.GetTitle", json!(title)),
        ("Mission.GetDescription", json!(description)),
        ("Mission.GetChallengeTypeName", json!("")),
        ("Mission.GetIconString", json!("")),
    ]
}

#[cfg(test)]
mod tests {
    use crate::apt_screen::Screen;
    use serde_json::json;
    use std::path::PathBuf;

    /// What the S2 natives hand the challenge HUD movies.
    #[test]
    fn challenge_hud_texts() {
        assert_eq!([super::countdown_text(3), super::countdown_text(0)], ["#3", "Go!"]);
        assert_eq!([super::timer_text(125.2), super::timer_text(9.4), super::timer_text(0.)], ["#02:06", "#10", "#"]);
        let unit = |id: &str| if id.ends_with("FEET") { "ft" } else { "m" }.to_owned();
        assert_eq!(super::distance_data(30., 18., false, unit), json!(["18.0 m", 1]));
        assert_eq!(super::distance_data(30., 18.5, false, unit), json!(["18.5 m", 0]));
        assert_eq!(super::distance_data(-1., 99., false, unit), json!(["99.0 m", 1]));
        assert_eq!(super::distance_data(30., 18.5, true, unit), json!(["55.5 ft", 0]));
    }

    /// sync's decisions view to view: the countdown's gating down to Go!, the timer's tick / back-to-normal / refresh,
    /// and the photo arrows (a step left with two photos is still "previous"; a count change alone does nothing).
    #[test]
    fn challenge_hud_steps() {
        use super::{Act::*, countdown_acts, photo_acts, timer_acts};
        let time = |text: &str, white: u8| Answer("HUDComponents.Timer_GetData", json!([text, white]));
        let count = |text: &str| vec![Answer("HUDComponents.Countdown_GetCountdownTime", json!(text)), State(2)];
        // A 0 s countdown (or a timer not yet running) shows nothing; then 3, 2, 1, Go!; hiding is SetState(1).
        assert_eq!(countdown_acts(None, Some((7, 0))), []);
        assert_eq!(countdown_acts(Some((7, 0)), Some((7, 3))), count("#3"));
        assert_eq!(countdown_acts(Some((7, 1)), Some((7, 0))), count("Go!"));
        assert_eq!(countdown_acts(Some((7, 3)), Some((8, 3))), count("#3"), "a retry's new timer starts over");
        assert_eq!(countdown_acts(Some((7, 0)), None), [State(1)]);
        // The timer: intro + text; the same second again does nothing; under 10 s a tick and red; back over 10 s, 5.
        // Retail's stored time starts at 0.0, so a first show over 10 s also sends 5.
        assert_eq!(timer_acts(None, Some(45.)), [State(1), State(5), time("#45", 1), State(3)]);
        assert_eq!(timer_acts(Some(45.), Some(44.2)), []);
        assert_eq!(timer_acts(Some(10.), Some(9.)), [Sound("timer_end"), State(4), time("#09", 0), State(3)]);
        assert_eq!(timer_acts(Some(9.), Some(12.)), [State(5), time("#12", 1), State(3)]);
        assert_eq!(timer_acts(Some(3.), None), [State(2)]);
        // Photos.
        assert_eq!(photo_acts(Some((0, 2, true)), Some((1, 2, false))), [Sound("photo_choose"), State(254), State(1)]);
        assert_eq!(photo_acts(Some((2, 3, false)), Some((0, 3, true))), [Sound("photo_choose"), State(255), State(0)]);
        assert_eq!(photo_acts(Some((1, 2, true)), Some((1, 3, true))), []);
    }

    /// The won lesson's "Challenge Complete!" through hudcentral, as the RESULT layer opens it (SKATE2_SCREENS,
    /// SKATE2_SHOTS as apt_screen's snapshot test): centred text, and the success sound.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_central_message_snapshot() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let out = PathBuf::from(std::env::var("SKATE2_SHOTS").unwrap());
        let answers = [
            ("HUDComponents.Central_GetMessageData", json!(["Challenge Complete!", 0])),
            ("HUDComponents.Central_GetAudioHook", json!(2)),
        ];
        let mut screen = Screen::load(&root.join("runtime/hudcentral.json"), &answers).unwrap();
        screen.call("SetState", &[json!(1)]).unwrap();
        screen.call("SetState", &[json!(4)]).unwrap();
        assert!(screen.bindings.sounds.iter().any(|s| s == "challenge_success"), "{:?}", screen.bindings.sounds);
        for _ in 0..45 {
            screen.advance().unwrap();
        }
        let draws = screen.draws().unwrap();
        assert!(!draws.is_empty(), "nothing drawn");
        crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join("central.png"));
    }

    /// The clip at `path` (dot-separated) from the screen's root.
    fn clip(screen: &Screen, path: &str) -> usize {
        path.split('.').fold(screen.bindings.movie.root, |at, part| match screen.vm.get(at, part) {
            crate::apt_vm::Value::Object(o) => o,
            _ => panic!("no clip {part} in {path}"),
        })
    }
    fn number(screen: &Screen, object: usize, key: &str) -> f64 {
        match screen.vm.get(object, key) {
            crate::apt_vm::Value::Number(n) => n,
            other => panic!("{key} is {other:?}"),
        }
    }

    /// hudtutorial mirrors its trick gestures (_xscale -100) unless Tricks.IsStanceTrickGesturesFlipped: a goofy skater
    /// (the flipped answer) sees the kickflip stick as authored, a regular one mirrored; state 77 re-reads it.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_gestures_follow_the_stance() {
        let file = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap()).join("runtime/hudtutorial.json");
        let kickflip = "container_mc.mcGestureButtonGroup.mAnimKickflip";
        for (goofy, xscale) in [(true, 100.), (false, -100.)] {
            let mut screen = Screen::load(&file, &super::stance_answers(goofy)).unwrap();
            screen.play("hudintro").unwrap();
            screen.call("SetState", &[json!(246)]).unwrap();
            screen.call("SetState", &[json!(14)]).unwrap();
            let anim = clip(&screen, kickflip);
            assert_eq!(number(&screen, anim, "_xscale"), xscale, "goofy {goofy}");
            // The stance flips under the shown prompt.
            for (k, v) in super::stance_answers(!goofy) {
                screen.answer(k, v);
            }
            screen.call("SetState", &[json!(77)]).unwrap();
            assert_eq!(number(&screen, anim, "_xscale"), -xscale, "goofy {goofy} flipped by state 77");
        }
    }

    /// Every text field's shown text.
    fn texts(screen: &Screen) -> Vec<String> {
        (screen.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(crate::apt_vm::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// The challenge HUD movies as sync drives them (SKATE2_SCREENS, SKATE2_SHOTS), view by view through sync's own
    /// steps: the countdown counts aloud only on the seconds it shows (none for the 0 s start, one for Go!), the timer
    /// shows its seconds and ticks under 10 s, the distance meter goes red past 0.6 of the limit, and the photo pick
    /// lights its arrows the way the player stepped.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_challenge_hud_snapshot() {
        use super::{apply, countdown_acts, photo_acts, timer_acts};
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let out = PathBuf::from(std::env::var("SKATE2_SHOTS").unwrap());
        let load = |movie: &str, answers: &[(&str, serde_json::Value)]| Screen::load(&root.join(format!("runtime/{movie}.json")), answers).unwrap();
        let shoot = |screen: &mut Screen, frames: usize, name: &str| {
            for _ in 0..frames {
                screen.advance().unwrap();
            }
            let draws = screen.draws().unwrap();
            assert!(!draws.is_empty(), "{name}: nothing drawn");
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join(format!("{name}.png")));
        };
        let asked = |screen: &mut Screen, native: &str| assert!(screen.take_calls().iter().any(|(c, _)| c == native), "{native}");

        let mut countdown = load("hudcountdown", &super::countdown_answers(true));
        let mut last = None;
        for (left, name, counted) in [(0, "", 0), (3, "countdown_3", 1), (2, "countdown_2", 1), (1, "", 1), (0, "countdown_go", 1)] {
            apply(&mut countdown, countdown_acts(last, Some((7, left)))).unwrap();
            last = Some((7, left));
            let sounds: Vec<String> = countdown.bindings.sounds.drain(..).collect();
            assert_eq!(sounds, vec!["challenge_count"; counted], "{left}");
            if !name.is_empty() {
                shoot(&mut countdown, 10, name);
            }
        }

        let mut timer = load("hudtimer", &[]);
        let mut last = None;
        for (left, name, shows, ticks) in [(45., "timer_45", "45", false), (44.5, "", "45", false), (9., "timer_9", "09", true)] {
            apply(&mut timer, timer_acts(last, Some(left))).unwrap();
            last = Some(left);
            let sounds: Vec<String> = timer.bindings.sounds.drain(..).collect();
            assert_eq!(sounds.iter().any(|s| s == "timer_end"), ticks, "{left}: {sounds:?}");
            if !name.is_empty() {
                asked(&mut timer, "HUDComponents.Timer_GetData");
                shoot(&mut timer, 20, name);
            }
            assert!(texts(&timer).iter().any(|t| t.contains(shows)), "{left}: {:?}", texts(&timer));
        }

        let mut distance = load("huddistance", &[]);
        distance.call("SetState", &[json!(1)]).unwrap();
        let data = super::distance_data(30., 21.4, false, |id| distance.bindings.movie.text_assets.localize(id));
        distance.answer("HUDComponents.DistanceMeter_GetData", data);
        distance.call("SetState", &[json!(3)]).unwrap();
        asked(&mut distance, "HUDComponents.DistanceMeter_GetData");
        shoot(&mut distance, 20, "distance");

        let answers = [("HUDComponents.PhotoSelect_IsSelectionAvail", json!(true)), ("HUDComponents.PhotoSelect_GetNumPhotos", json!(3))];
        let mut photos = load("photoselect", &answers);
        photos.call("OnLoaded", &[]).unwrap();
        asked(&mut photos, "HUDComponents.PhotoSelect_GetNumPhotos");
        photos.call("SetState", &[json!(0)]).unwrap();
        let mut last = Some((0, 3, true));
        // Left (wraps to the third), then right twice: back to the first, on to the second.
        for step in [(2, 3, false), (0, 3, true), (1, 3, true)] {
            let acts = photo_acts(last, Some(step));
            assert_eq!(acts[1], super::Act::State(if step.2 { 255 } else { 254 }), "{step:?}");
            apply(&mut photos, acts).unwrap();
            last = Some(step);
        }
        assert_eq!(photos.bindings.sounds.drain(..).filter(|s| s == "photo_choose").count(), 3);
        shoot(&mut photos, 30, "photoselect");
    }

    /// Complete, Failed and plain central messages: colour (off-white, red) and the movie's own audio hook sound.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_central_message_kinds() {
        use super::Central;
        let file = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap()).join("runtime/hudcentral.json");
        for (kind, colour, sound) in [
            (Central::Complete, 0xfffbec, "challenge_success"),
            (Central::Failed, 0x97220b, "challenge_failed"),
            (Central::Plain, 0xfffbec, "central_message_default"),
        ] {
            let (c, hook, _) = kind.presentation();
            let answers = [
                ("HUDComponents.Central_GetMessageData", json!(["Message", c])),
                ("HUDComponents.Central_GetAudioHook", json!(hook)),
            ];
            let mut screen = Screen::load(&file, &answers).unwrap();
            screen.call("SetState", &[json!(1)]).unwrap();
            screen.call("SetState", &[json!(4)]).unwrap();
            assert_eq!(screen.bindings.sounds, [sound], "{kind:?}");
            let label = clip(&screen, "mcMessageText.mcText2.mcTextStringWhite.labelText");
            assert_eq!(number(&screen, label, "textColor"), f64::from(colour), "{kind:?}");
        }
    }

    /// UnlockOverlay as FE state 10 runs it after Flipping Wood 101 (Reda): SetState(50) every frame (0x82528220)
    /// animates the character tile and the cash count, which ends in FEUnlock.Cash_SetCashAnimDone. Its close plays
    /// the root's "outro" to ScreenManager.OutroComplete (Step 1b).
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_unlock_overlay_snapshot() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let out = PathBuf::from(std::env::var("SKATE2_SHOTS").unwrap());
        let mut screen = Screen::load(&root.join("runtime/unlockoverlay.json"), &super::unlock_answers(&["ID_COMMON_REDA".into()])).unwrap();
        let mut done = false;
        for _ in 0..600 {
            screen.call("SetState", &[json!(50)]).unwrap();
            screen.advance().unwrap();
            done |= screen.take_calls().iter().any(|(call, _)| call == "FEUnlock.Cash_SetCashAnimDone");
        }
        assert!(done, "the cash count never finished");
        assert!(!screen.bindings.sounds.is_empty(), "no reward sounds");
        let draws = screen.draws().unwrap();
        assert!(!draws.is_empty(), "nothing drawn");
        crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join("unlock.png"));
        screen.play("outro").unwrap();
        for _ in 0..120 {
            screen.advance().unwrap();
        }
        assert!(screen.bindings.outro_complete, "the outro never completed");
    }

    /// Skate101 as FE state 79 fills it for 201_Follow_Slappy: title, body, the A glyph. Its intro calls
    /// Audio.Play101OverlayIn, which no native binds in retail (Step 1e): no sound. Its close plays the root's "outro"
    /// to ScreenManager.OutroComplete (Step 1b).
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_skate101_snapshot() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let out = PathBuf::from(std::env::var("SKATE2_SHOTS").unwrap());
        let answers = super::skate101_answers("Footage Time!", "Time to get started on your sponsor me tape. Skate with Slappy to the school.");
        let mut screen = Screen::load(&root.join("runtime/skate101.json"), &answers).unwrap();
        screen.call("OnLoaded", &[]).unwrap();
        screen.play("intro").unwrap();
        for _ in 0..45 {
            screen.advance().unwrap();
        }
        assert!(screen.bindings.intro_complete, "the intro never completed");
        assert!(screen.bindings.sounds.is_empty(), "{:?}", screen.bindings.sounds);
        let draws = screen.draws().unwrap();
        assert!(!draws.is_empty(), "nothing drawn");
        crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join("skate101.png"));
        screen.play("outro").unwrap();
        for _ in 0..120 {
            screen.advance().unwrap();
        }
        assert!(screen.bindings.outro_complete, "the outro never completed");
    }

    /// SignUpHUD (hud2/actionoverlay2, screen 91) in a lesson's sign-up box: SetState(1) (kActionOverlayState_Challenge)
    /// sets the D-pad Up button, ID_COMMON_SIGNUP and the lesson's title (HUDComponents.SignUp_GetChallengeTitle) and
    /// plays button_intro; SIGN_UP_OFF (Off) hides it (Show(false), button_outro).
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_sign_up_hud_snapshot() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let out = PathBuf::from(std::env::var("SKATE2_SHOTS").unwrap());
        let mut screen = Screen::load(&root.join("runtime/actionoverlay2.json"), &super::sign_up_answers("Flipping Wood 101")).unwrap();
        screen.call("SetState", &[json!(1)]).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        assert!(screen.take_calls().iter().any(|(call, _)| call == "HUDComponents.SignUp_GetChallengeTitle"));
        let draws = screen.draws().unwrap();
        assert!(!draws.is_empty(), "nothing drawn");
        crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join("signup_hud.png"));
    }

    /// Signed up (or out of the box), sync's SetState(SIGN_UP_OFF) plays button_outro and hides the prompt: nothing stays drawn.
    #[test]
    #[ignore = "requires the converted Skate 2 screens"]
    fn skate2_sign_up_hud_hides() {
        let root = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap());
        let mut screen = Screen::load(&root.join("runtime/actionoverlay2.json"), &super::sign_up_answers("Flipping Wood 101")).unwrap();
        screen.call("SetState", &[json!(1)]).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        screen.call("SetState", &[json!(super::SIGN_UP_OFF)]).unwrap();
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        let draws = screen.draws().unwrap();
        assert!(draws.is_empty(), "still drawn after SetState({}): {} draws", super::SIGN_UP_OFF, draws.len());
    }
}
