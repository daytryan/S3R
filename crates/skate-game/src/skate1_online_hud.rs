//! Skate 1's online HUD over skate1_online.rs's matches, on hud-skate1/online's movies (old_online1.py): the mode's
//! challenge overlay, the HUD components it drives (centralhud's messages and countdown, timerhud, onlinehud's
//! standings), the leaderboard and the event log, fed as default.xex fed them.
//!
//! - The script's `HUD.ChangeHUDState(state, substate)` goes through the C++ HUD handler's table ([`overlay_states`])
//!   to the overlay's `SetState`; the overlay's ActionScript runs as shipped.
//! - What the overlay asks of `HUDHooks` (hudbase's functions, which live in Skate 1's main HUD movie) is ported in
//!   [`Central`], on centralhud (the clip hudbase loads as its messenger); `HUDHooks.TimerHUD` / `OnlineRaceHUD` are the
//!   timerhud / onlinehud screens.
//! - The leaderboard (HUD state 49) comes 2.7 s after a mode's results substate, with the provider state and columns
//!   of the mode's FELeaderboard provider ([`Board`]); its countdown ("Moving to new location in" / "Returning to Lobby
//!   in") ends it, and at the end of the match that is the script's `postchallengedisplayexit`.
//! - `HUD.AddEventLog` / `AddChallengeStartMessage` / `FinishMessage` / `WinnerMessage` are event-log lines
//!   ([`OnlineRoster::lines`], which online_hud.rs shows in hud_online_eventlog).
use crate::{
    apt_screen::{SKATE1_ONLINE, Screen, Screens},
    apt_vm::{Host, ObjectKind, Value},
    match_core::Mode,
    online_roster::OnlineRoster,
    skate1_online::{Call, Data, Run, Skate1Online, Standing},
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The online HUD movies' folder under the assets.
pub(crate) const MOVIES: &str = "private/hud-skate1/online/runtime";
// Stacked by name above online_hud's "1-eventlog" / "2-roster" on the same layer.
const TIMER: &str = "5-timer";
const RACE: &str = "6-online";
const OVERLAY: &str = "7-overlay";
const CENTRAL: &str = "8-central";
const BOARD: &str = "9-leaderboard";
/// The S.K.A.T.E. overlay's components (skatehud's players and letters, skatetrick's trick list) and the spectator
/// overlay (races, after this player finished).
const PLAYERS: &str = "6p-skateplayers";
const TRICKS: &str = "6t-skatetrick";
const SPECTATOR: &str = "6s-spectator";
const SCREENS: [&str; 8] = [TIMER, RACE, PLAYERS, SPECTATOR, TRICKS, OVERLAY, CENTRAL, BOARD];
/// eSk8HUDState.FreeSkate.
const FREE_SKATE: i64 = 1;
/// The slot-4 handlers push the leaderboard this long after the results substate (default.xex: 2700 ms).
const BOARD_AFTER: f32 = 2.7;
/// The between-rounds countdown ("Moving to new location in"), default.xex: 10 s.
const MOVING_FOR: f32 = 10.;
/// "Returning to Lobby in": the deadline isn't recovered; a knob.
const LOBBY_FOR: f32 = 10.;
/// leaderboard's root outro before it closes (its 'outro' label runs about this long; a knob).
const BOARD_OUTRO: f32 = 0.5;
/// Spot Race's spot-complete substate goes back to Active this much later (default.xex: 800 ms).
const SPOT_COMPLETE_FOR: f32 = 0.8;
/// hudbase: mCentralMessageTimeLimit / kBottomMessageTimeLimit, 180 frames.
const MESSAGE_FOR: f32 = 3.;
/// FELeaderboard.GetVoipStatus: off.
const VOIP_OFF: i64 = 0;

/// A mode's overlay movie, its HUD state, the Mission object its overlay asks and the challenge type
/// (Mission.GetChallengeType, rows.json `type`).
struct Kind {
    state: i64,
    movie: &'static str,
}
fn kind(mode: Mode) -> Option<Kind> {
    let (state, movie) = match mode {
        Mode::BestTrick => (36, "hud_besttrick_challenge_overlay"),
        Mode::Jam => (38, "hud_jam_challenge_overlay"),
        Mode::SpotBattle => (16, "hud_spot_challenge_overlay"),
        Mode::Skate => (18, "hud_skate_challenge_overlay"),
        Mode::DeathRace => (34, "hud_deathrace_challenge_overlay"),
        Mode::SpotRace => (35, "hud_spotrace_challenge_overlay"),
        _ => return None,
    };
    Some(Kind { state, movie })
}

// ---------------------------------------------------------------- the lobby
/// eChallengeTypes.OnlineFreeSkate: eSk8HUDState.FreeSkate's challenge type online (0x8234f0e0 sets [0x830a3f64]),
/// the `eChallengeTypes_OnlineFreeSkate` global onlinehud compares (main.apt's RegisterChallengeEnum, 0x82465784).
const ONLINE_FREE_SKATE: i64 = 22;
/// The HUD's player table (0x830fce80 +8: 6 rows of 0x90): GetOnlineFreeskatePositions' length (0x8243b098).
pub(crate) const LOBBY_ROWS: usize = 6;
/// The free-skate mission's title in a challenge session (0x82347148: hash 0xdbb3ef87, english.big), with the
/// session's mode title (0x82371050); modes.json's `lobby_title` once re-exported.
const LOBBY_TITLE: &str = "{0:s} - Lobby";

/// The overlay eSk8HUDState.FreeSkate sets online, by the session's challenge type (enter 0x8234f550, exit
/// 0x8234f658): Online Freeskate's (None) hud_online_freeskate_overlay kStateActive / kStateHide; a challenge
/// session's its mode's overlay, kStateLobby (100) / its hide state (Best Trick and Jam 3, Spot Race 7, else 0).
/// Each shows onlinehud (OnlineRaceHUD.Show) and InkHUD's 'onlineobjective'.
pub(crate) fn lobby_overlay(mode: Option<Mode>) -> Option<(&'static str, i64, i64)> {
    let Some(mode) = mode else { return Some(("hud_online_freeskate_overlay", 1, 0)) };
    let hide = match mode {
        Mode::BestTrick | Mode::Jam => 3,
        Mode::SpotRace => 7,
        _ => 0,
    };
    Some((kind(mode)?.movie, 100, hide))
}

/// The free-skate mission's GetRaceHUDTitle (0x82347148): a session of type OnlineFreeSkate "Freeskate"
/// (ID_SIDEKICK_ONLINE_MENU_FREESKATE), a challenge session "<its mode's title> - Lobby".
pub(crate) fn lobby_title(data: &Data, mode: Option<Mode>) -> String {
    match mode {
        None => data.title("freeskate"),
        Some(mode) => fill(data.modes["strings"]["lobby_title"].as_str().unwrap_or(LOBBY_TITLE), &data.title(mode.name())),
    }
}

/// What the lobby's onlinehud asks with the challenge type OnlineFreeSkate (the free-skate mission, vtable 0x821684d8):
/// no lap field, split, turns or style (0x82328030 / 0x82328020 / 0x82328008 / 0x82328028: none, so ""
/// and 0), the players in table order (GetOnlineFreeskatePositions: slot + 1, padded with -1; GetPlayersData(slot + 1,
/// false) 0x8243ab20: [slot + 1, name, "", +0x70, slot, voip, left]).
pub(crate) fn lobby_answers(names: &[String], local: usize, title: &str) -> Vec<(String, Json)> {
    let names = &names[..names.len().min(LOBBY_ROWS)];
    let positions: Vec<i64> = (1..=LOBBY_ROWS as i64).map(|p| if p as usize <= names.len() { p } else { -1 }).collect();
    let mut out: Vec<(String, Json)> = vec![
        ("Mission.GetIsOnlineMission".into(), json!(true)),
        ("Mission.GetIsPartyPlayMission".into(), json!(false)),
        ("Mission.GetChallengeType".into(), json!(ONLINE_FREE_SKATE)),
        ("Mission.GetLocalPlayerIndex".into(), json!(local)),
        ("Mission.GetNumberOfPlayers".into(), json!(names.len())),
        ("Mission.GetRaceHUDTitle".into(), json!(title)),
        ("Mission.GetRaceHUDLapField".into(), json!("")),
        ("Mission.GetRaceHUDStyle".into(), json!(0)),
        ("Mission.GetIsTurnBased".into(), json!(false)),
        ("Mission.GetOnlineFreeskatePositions".into(), json!(positions)),
    ];
    // UNTRACED: +0x70 ([3], which onlinehud doesn't read); a member is never left (+0x71) while in the roster.
    for (i, name) in names.iter().enumerate() {
        out.push((format!("Mission.GetPlayersData({})", i + 1), json!([i + 1, name, "", false, i, crate::voice::voip_named(name), false])));
    }
    out
}

/// The natives the overlays and components address beyond apt_screen's (HUDHooks' sub-objects are its properties).
pub(crate) const MISSIONS: [&str; 6] = ["BestTrickMission", "JamMission", "OwnTheSpotMission", "DeathRaceMission", "SkateMission", "SKATEMission"];
pub(crate) const HOOKS: [&str; 5] = ["TimerHUD", "OnlineRaceHUD", "InkHUD", "ObjectiveHUD", "TrickDisplay"];

/// The leaderboard default.xex pushes after a results substate.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Push {
    /// The provider's state (its columns and title details).
    state: i64,
    /// Seconds after the substate.
    after: f32,
    /// The overlay's SetState when the board shows / when it goes.
    show: Option<i64>,
    exit: Option<i64>,
}

/// What the HUD handler does with a substate: the overlay's SetStates, a leaderboard to push, and (Spot Race's
/// spot complete) a substate to replay later.
#[derive(Debug, Default, PartialEq)]
struct Handled {
    states: Vec<i64>,
    push: Option<Push>,
    again: Option<(f32, i64)>,
    /// The races' Spectator substate: spectator_mode's SetState(kStateActive) (0x8236b7e0 / 0x8236bd0c); it goes
    /// when the leaderboard's exit callback runs (0x8236b920 / 0x8236be58).
    spectate: bool,
}

/// What the tables read: the round, the win flag (HUD.SetWinState), this player's rank and race exit.
#[derive(Clone, Copy, Default)]
struct Facts {
    round: i64,
    rounds: i64,
    win: bool,
    first: bool,
    out: bool,
}

/// default.xex's per-mode HUD handlers: substate `sub` (after `prev`) as overlay SetStates.
// ponytail: the C++ branches whose condition isn't recovered take their plain arm: S.K.A.T.E.'s 14 / 16 / 22 (31 /
// 30 when a flag is set), Death Race's 1 (an unknown call with 5); Death Race's GateComplete (15) has no arm in the
// recovered table and shows the overlay's gate-passed message (4).
fn overlay_states(mode: Mode, prev: Option<i64>, sub: i64, f: Facts) -> Handled {
    let mut h = Handled::default();
    let last = f.round >= f.rounds;
    let result = |win: bool| if win { 22 } else { 23 };
    match mode {
        Mode::BestTrick => {
            if matches!(prev, Some(10 | 13)) {
                h.states.push(21);
            }
            match sub {
                0 | 1 | 3 | 9 | 10 => h.states.push(sub),
                14 => h.states.push(21),
                2 => {
                    h.states.push(if last { result(f.win) } else { 20 });
                    let state = if f.rounds == 1 { 3 } else { 0 };
                    h.push = Some(Push { state, after: BOARD_AFTER, show: Some(3), exit: None });
                }
                _ => {}
            }
        }
        Mode::Jam => match sub {
            0 | 1 | 3..=7 | 13 => h.states.push(sub),
            8 => {
                h.states.push(if last { result(f.win) } else { 20 });
                h.push = Some(Push { state: 3, after: BOARD_AFTER, show: Some(3), exit: None });
            }
            12 => {
                h.states.push(12);
                if f.rounds > 1 {
                    h.states.push(24);
                }
            }
            19 => h.states.push(21),
            _ => {}
        },
        // The overlay hides TimerHUD for the 3-2-1 (its states 10 / 14) and its Show(true) skips it once visible, so
        // only its state 21 shows the turn's clock again (hud_spot_challenge_overlay SetState). INFERRED: the handler
        // sets 21 after a countdown, as Best Trick's recovered one does after its 10 / 13 (bug 10-31: no time left).
        Mode::SpotBattle if matches!(prev, Some(11 | 15)) && !matches!(sub, 11 | 15) => {
            h = overlay_states(mode, None, sub, f);
            h.states.insert(0, 21);
        }
        Mode::SpotBattle => match sub {
            2 | 7 => h.states.push(1),
            3 => h.states.push(2),
            4 => h.states.push(4),
            5 => h.states.push(3),
            6 | 8 | 9 => h.states.push(0),
            10 => {
                h.states.push(9);
                h.push = Some(Push { state: 2, after: 0., show: None, exit: None });
            }
            11 => h.states.push(10),
            13 => h.states.push(11),
            15 => h.states.push(14),
            _ => {}
        },
        Mode::DeathRace => match sub {
            0 => h.states.push(19),
            2 => h.states.push(12),
            3 => h.states.push(1),
            4..=6 => {
                h.states.push(if f.win { 14 } else { 15 });
                h.push = Some(Push { state: 3, after: BOARD_AFTER, show: None, exit: Some(0) });
            }
            7 | 12 => h.states.push(0),
            10 => h.states.push(13),
            11 => h.states.push(5),
            13 => h.states.extend([1, 16]),
            15 => h.states.push(4),
            16 => h.states.push(if f.out { 18 } else { 17 }),
            17 => h.spectate = true,
            _ => {}
        },
        Mode::SpotRace => match sub {
            0..=3 | 7 | 11 => h.states.push(sub),
            13 => h.states.extend([13, 16]),
            4..=6 => {
                h.states.push(if f.win { 14 } else { 15 });
                h.push = Some(Push { state: 3, after: BOARD_AFTER, show: None, exit: Some(7) });
            }
            12 => {
                h.states.push(12);
                h.again = Some((SPOT_COMPLETE_FOR, 3));
            }
            15 => h.states.push(17),
            16 => h.spectate = true,
            _ => {}
        },
        Mode::Skate => match sub {
            1 => h.states.extend([7, 32]),
            11 => {
                h.states.push(if f.first { 22 } else { 23 });
                h.push = Some(Push { state: 3, after: BOARD_AFTER, show: Some(0), exit: None });
            }
            _ => h.states.extend(
                match sub {
                    3 => Some(1),
                    4 => Some(2),
                    5 => Some(17),
                    6 => Some(3),
                    7 => Some(4),
                    8 => Some(18),
                    9 => Some(5),
                    10 => Some(0),
                    13 => Some(6),
                    14 => Some(8),
                    15 => Some(9),
                    16 => Some(10),
                    17 => Some(11),
                    18 => Some(12),
                    19 => Some(13),
                    20 => Some(14),
                    21 => Some(15),
                    22 => Some(16),
                    23 => Some(24),
                    24 => Some(25),
                    _ => None,
                },
            ),
        },
        _ => {}
    }
    h
}

// ---------------------------------------------------------------- hudbase on centralhud
pub(crate) fn clip_call(screen: &mut Screen, path: &str, method: &str, args: Vec<Value>) -> Result<(), String> {
    let clip = screen.clip(path).ok_or_else(|| format!("centralhud: no clip {path}"))?;
    screen.vm.begin_update();
    screen.vm.call_method(clip, method, args, &mut screen.bindings)?;
    crate::core_menu_runtime::drain(&mut screen.vm, &mut screen.bindings)
}
pub(crate) fn play(screen: &mut Screen, path: &str, label: &str) -> Result<(), String> {
    clip_call(screen, path, "gotoAndPlay", vec![Value::Text(label.into())])
}
pub(crate) fn set(screen: &mut Screen, path: &str, key: &str, value: Value) -> Result<(), String> {
    let clip = screen.clip(path).ok_or_else(|| format!("centralhud: no clip {path}"))?;
    screen.vm.set(clip, key, value)?;
    screen.bindings.property_changed(&mut screen.vm, clip, key)
}

const MESSAGE: &str = "mcMessageText";
const MESSAGE_TEXT: &str = "mcMessageText.mcText2.mcTextStringWhite.labelText";
const COUNTDOWN: &str = "mcCountdownText";
const COUNTDOWN_TEXT: &str = "mcCountdownText.mcNumber.labelText";
const BOTTOM: &str = "mcMessage2";
const BOTTOM_TEXT: &str = "mcMessage2.mcText2.labelText";
const CONGRATS: &str = "mcCongrats";
const CONGRATS_TEXT: &str = "mcCongrats.mcCongratstext.labelText";
/// Skate 1's ScreenManager.mColour_* (fe/source/helper/screenmanager's init block; Skate 2's red, which apt_screen
/// sets, is darker), set on every screen this HUD opens.
pub(crate) const COLOURS: [(&str, u32); 5] = [("White", 0xffffff), ("Red", 0xb1270b), ("Green", 0x82be19), ("Grey", 0x888888), ("PlayerHilight", 0xfff891)];
const WHITE: f64 = 0xffffff as f64;
const GREEN: f64 = 0x82be19 as f64;
const RED: f64 = 0xb1270b as f64;
const HILIGHT: f64 = 0xfff891 as f64;
/// hudbase.PlayCountDownSound's online sounds (FE messages 356 / 357: HUD.abk samples 35 / 36).
const COUNTDOWN_TICK: &str = "PlayCountDownGenSound";
const COUNTDOWN_GO: &str = "PlayCountDownGenGoSound";

/// hudbase's messenger state (ShowCentralMessage, ShowCentralCountdownTimer, ShowBottomMessage, ShowCongratsMessage
/// and their onEnterFrame updates), in seconds where hudbase counts frames.
#[derive(Default)]
pub(crate) struct Central {
    /// The central message: seconds shown (None: hidden).
    message: Option<f32>,
    /// The bottom message: seconds left (None: hidden; infinite: held).
    bottom: Option<f32>,
    /// mCentralCountdownMessage, mWaitingForCountdown, mCountdownOldTime; None: its onEnterFrame is off.
    countdown: Option<(String, bool, i64)>,
}

impl Central {
    /// One of the HUDHooks calls the overlay made.
    pub(crate) fn hook(&mut self, screen: &mut Screen, name: &str, args: &[Json]) -> Result<(), String> {
        let text = |i: usize| Value::Text(args.get(i).and_then(Json::as_str).unwrap_or_default().to_owned());
        let colour = |i: usize| args.get(i).and_then(Json::as_f64);
        match name {
            "ShowCentralMessage" => {
                set(screen, MESSAGE_TEXT, "text", text(0))?;
                set(screen, MESSAGE_TEXT, "textColor", Value::Number(colour(1).unwrap_or(WHITE)))?;
                play(screen, MESSAGE, "intro")?;
                self.message = Some(0.);
            }
            "ShowCentralCountdownTimer" => {
                set(screen, COUNTDOWN_TEXT, "text", Value::Text("#".into()))?;
                let end = args.first().and_then(Json::as_str).unwrap_or("ID_MISSION_COUNTDOWN_GO").to_owned();
                self.countdown = Some((end, true, 0));
            }
            "ShowBottomMessage" => {
                set(screen, BOTTOM_TEXT, "text", text(0))?;
                set(screen, BOTTOM_TEXT, "textColor", Value::Number(WHITE))?;
                let (hold, force) = (args.get(1) == Some(&Json::Bool(true)), args.get(2) == Some(&Json::Bool(true)));
                if self.bottom.is_none() || force {
                    play(screen, BOTTOM, "intro")?;
                }
                self.bottom = Some(if hold { f32::INFINITY } else { MESSAGE_FOR });
            }
            "ShowBottomSuccessMessage" => {
                set(screen, BOTTOM_TEXT, "text", text(0))?;
                play(screen, BOTTOM, "success")?;
                self.bottom = None;
            }
            "HideBottomMessage" => self.hide_bottom(screen)?,
            "ShowCongratsMessage" => {
                let colour = colour(1).unwrap_or(GREEN);
                set(screen, CONGRATS_TEXT, "textColor", Value::Number(colour))?;
                set(screen, CONGRATS_TEXT, "text", text(0))?;
                play(screen, CONGRATS, if colour == RED { "introfail" } else { "intro" })?;
            }
            "HideAllMessages" => {
                self.hide_bottom(screen)?;
                if self.message.take().is_some() {
                    play(screen, MESSAGE, "outro")?;
                }
                self.countdown = None;
                play(screen, COUNTDOWN, "invisible")?;
            }
            _ => {}
        }
        Ok(())
    }
    fn hide_bottom(&mut self, screen: &mut Screen) -> Result<(), String> {
        if self.bottom.take().is_some() {
            play(screen, BOTTOM, "outro")?;
        }
        Ok(())
    }

    /// The messenger's onEnterFrames: the central message's and the bottom message's timeouts, the countdown
    /// (Mission.GetCurrentTime: the HUD countdown timer's seconds left).
    pub(crate) fn update(&mut self, screen: &mut Screen, dt: f32, time: f32) -> Result<(), String> {
        if let Some(shown) = &mut self.message {
            *shown += dt;
            if *shown >= MESSAGE_FOR {
                self.message = None;
                play(screen, MESSAGE, "outro")?;
            }
        }
        if let Some(left) = &mut self.bottom {
            *left -= dt;
            if *left <= 0. {
                self.hide_bottom(screen)?;
            }
        }
        // UpdateCentralCountdownTimer: the end message once a number has shown and the time is out (or for a timer
        // past 3, silently), else each new number of the last three. Each sounds through PlayCountDownSound, online
        // every mode's: Audio.PlayCountDownGenSound for 3 / 2 / 1, PlayCountDownGenGoSound for the end (bug 10-31).
        let t = time.ceil() as i64;
        let (text, sound) = match &mut self.countdown {
            Some((end, waiting, _)) if (!*waiting && t <= 0) || t > 3 => {
                let end = end.clone();
                self.countdown = None;
                (end, (t <= 0).then_some(COUNTDOWN_GO))
            }
            Some((_, waiting, old)) if t > 0 && t != *old => {
                (*waiting, *old) = (false, t);
                (format!("#{t}"), Some(COUNTDOWN_TICK))
            }
            _ => return Ok(()),
        };
        // As the movies' own Audio.Play<Name>() calls: played from sfx/<Name>.wav.
        screen.bindings.sounds.extend(sound.map(str::to_owned));
        set(screen, COUNTDOWN_TEXT, "text", Value::Text(text))?;
        play(screen, COUNTDOWN, "intro")
    }
}

// ---------------------------------------------------------------- S.K.A.T.E.'s components
/// AS2 on a clip that isn't there does nothing; nor do these.
pub(crate) fn put(screen: &mut Screen, path: &str, key: &str, value: Value) -> Result<(), String> {
    if screen.clip(path).is_none() { Ok(()) } else { set(screen, path, key, value) }
}
pub(crate) fn go(screen: &mut Screen, path: &str, label: &str) -> Result<(), String> {
    if screen.clip(path).is_none() { Ok(()) } else { play(screen, path, label) }
}
pub(crate) fn text(s: &str) -> Value {
    Value::Text(s.into())
}
/// `path.name(arg)`; nothing when the clip or its method isn't there (an AS2 call on undefined).
pub(crate) fn method(screen: &mut Screen, path: &str, name: &str, arg: Value) -> Result<(), String> {
    let has = screen.clip(path).is_some_and(|c| !matches!(screen.vm.get(c, name), Value::Undefined));
    if has { clip_call(screen, path, name, vec![arg]) } else { Ok(()) }
}

/// What SkateMission / SKATEMission answer the components.
#[derive(Default)]
pub(crate) struct SkateView {
    pub(crate) names: Vec<String>,
    pub(crate) active: usize,
    /// The active player's letters (GetPlayerScore(active)).
    pub(crate) letters: i64,
    /// The "already been done" tricks, oldest first, and the one the leader repeated.
    pub(crate) abds: Vec<String>,
    pub(crate) hit: Option<usize>,
    /// SKATEMission.GetTrickLanded.
    pub(crate) landed: String,
    /// The set trick's stick: Tricks.GetGestureString(SKATEMission.GetTrickDisplayIndex()) and
    /// SKATEMission.IsTrickMirrored. None leaves mcGesture hidden.
    pub(crate) gesture: Option<(&'static str, bool)>,
}

/// hud_skate_challenge_overlay's half that works on its components: `mScreen.mSkateHUDPlayers` (skatehud) and
/// `mScreen.mSkateHUDTrickList` (skatetrick), which ScreenManager.RegisterUIComponent loads into the overlay. Here
/// they are screens of their own, so the overlay's lines on them (undefined in the overlay's VM) run here, in its
/// order, on the overlay's own SetState.
// The overlay's own UpdateLetters calls Audio.PlayGetLetter / PlayPlayerEliminated (apt_screen plays them from
// hud-skate1/online/sfx, old_online1_modes.py); PlayKnockout / PlayReposition are silent in retail (no sound record).
#[derive(Default)]
pub(crate) struct SkateParts {
    /// mVisible, mbLeaderActive, mOldNumPlayers.
    visible: bool,
    leader_active: bool,
    old_players: usize,
}

const SET: &str = "mcTrick_set";
const COPY: &str = "mcTrick_copy";
const LETTERS: [(&str, &str); 5] =
    [("mcLG_Letter_S", "letterS"), ("mcLG_Letter_K", "letterK"), ("mcLG_LetterA", "letterA"), ("mcLG_Letter_T", "letterT"), ("mcLG_Letter_E", "letterE")];

impl SkateParts {
    /// The constructor's: ResetColours, the trick list hidden.
    pub(crate) fn open(&mut self, tricks: &mut Screen) -> Result<(), String> {
        Self::tint(tricks, &[SET, COPY], WHITE, "normal")?;
        put(tricks, "", "_visible", Value::Bool(false))
    }

    pub(crate) fn state(&mut self, p: &mut Screen, t: &mut Screen, state: i64, v: &SkateView) -> Result<(), String> {
        match state {
            0 => self.show(p, t, false, v)?,
            1 => {
                self.show(p, t, true, v)?;
                for clip in [SET, COPY] {
                    put(t, &format!("{clip}.mcTrick.labelText"), "text", text("#"))?;
                }
                put(t, "mcPhase.labelText", "text", text("ID_SKATE_SET_TRICK"))?;
                for clip in [SET, COPY] {
                    put(t, &format!("{clip}.mcGesture"), "_visible", Value::Bool(false))?;
                }
                for i in 1..=4 {
                    put(p, &format!("mcPlayer{i}"), "_visible", Value::Bool(false))?;
                }
                for (i, name) in v.names.iter().enumerate() {
                    let label = format!("mcPlayer{}.mcNameStates.mcTextName.labelText", i + 1);
                    put(p, &format!("mcPlayer{}", i + 1), "_visible", Value::Bool(true))?;
                    put(p, &label, "autoSize", text("right"))?;
                    put(p, &label, "text", Value::Text(crate::apt_text::literal(name)))?;
                }
                self.icons(p, v)?;
                for i in 0..v.abds.len() {
                    put(t, &format!("mcABD.mcTrickName{i}.mcAbdtext.labelText"), "textColor", Value::Number(WHITE))?;
                    go(t, &format!("mcABD.mcTrickName{i}"), "small")?;
                }
                Self::tint(t, &[SET, COPY], WHITE, "normal")?;
            }
            2 => {
                Self::tint(t, &[SET, COPY], WHITE, "normal")?;
                // UpdateABDList.
                if v.abds.is_empty() {
                    put(t, "mcABD", "_visible", Value::Bool(false))?;
                } else {
                    put(t, "mcABD", "_visible", Value::Bool(true))?;
                    go(t, "mcABD", &format!("{}tricks", v.abds.len()))?;
                    for (i, name) in v.abds.iter().enumerate() {
                        put(t, &format!("mcABD.mcTrickName{i}.mcAbdtext.labelText"), "text", Value::Text(format!("#{name}")))?;
                    }
                }
                self.leader_active = true;
                self.icons(p, v)?;
            }
            4 => {
                Self::tint(t, &[SET, COPY], WHITE, "normal")?;
                put(t, &format!("{COPY}.mcTrick.labelText"), "text", text("#"))?;
                put(t, &format!("{COPY}.mcGesture"), "_visible", Value::Bool(false))?;
                put(t, "mcPhase.labelText", "text", text("ID_SKATE_COPY_THE_TRICK"))?;
                put(t, "mcABD", "_visible", Value::Bool(false))?;
                self.leader_active = false;
                self.icons(p, v)?;
            }
            // UpdateLetters.
            5 if (1..=5).contains(&v.letters) => {
                let n = v.letters as usize;
                for (k, (clip, _)) in LETTERS.iter().enumerate() {
                    put(p, &format!("mcLG_SkateAnimation.{clip}.mcLetter"), "_visible", Value::Bool(k < n))?;
                }
                put(p, "mcLG_SkateAnimation.mcStroke", "_visible", Value::Bool(n == 5))?;
                go(p, &format!("mcPlayer{}", v.active + 1), LETTERS[n - 1].1)?;
                if n == 5 {
                    go(p, "mcLG_SkateAnimation", "skate")?;
                }
            }
            // UpdateTrickList.
            6 => {
                let clip = if self.leader_active { SET } else { COPY };
                put(t, &format!("{clip}.mcTrick.labelText"), "text", Value::Text(format!("#{}", v.landed)))?;
                if let Some((gesture, mirrored)) = v.gesture {
                    let g = format!("{clip}.mcGesture");
                    put(t, &g, "_visible", Value::Bool(true))?;
                    method(t, &g, "SetGestureType", Value::Text(gesture.into()))?;
                    method(t, &g, "FlipGesture", Value::Bool(mirrored))?;
                }
            }
            11 => Self::tint(t, &[SET, COPY], GREEN, "green")?,
            12 => Self::tint(t, &[SET, COPY], RED, "red")?,
            // UpdateLeaderABD.
            13 => {
                Self::tint(t, &[SET], RED, "red")?;
                if let Some(i) = v.hit {
                    put(t, &format!("mcABD.mcTrickName{i}.mcAbdtext.labelText"), "textColor", Value::Number(RED))?;
                    go(t, &format!("mcABD.mcTrickName{i}"), "big")?;
                }
            }
            // UpdateTrickBailed.
            16 | 30 => Self::tint(t, if self.leader_active { &[SET][..] } else { &[SET, COPY] }, RED, "red")?,
            32 => {
                for i in 1..=4 {
                    go(p, &format!("mcPlayer{i}"), "everything")?;
                }
                self.show(p, t, false, v)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn show(&mut self, p: &mut Screen, t: &mut Screen, on: bool, v: &SkateView) -> Result<(), String> {
        if self.visible == on {
            return Ok(());
        }
        // The disc skatehud has intro2..4 / outro2..4: match_core::row_cap holds a Skate 1 S.K.A.T.E. to 4.
        if on {
            let n = v.names.len().clamp(2, 4);
            go(p, "", &format!("intro{n}"))?;
            self.old_players = n;
            put(t, "mcABD", "_visible", Value::Bool(false))?;
            put(t, "mcPhase", "_visible", Value::Bool(true))?;
            put(t, "mcABD.mcABD_title.labelText", "text", text("ID_SKATE_DONT_DO_TRICKS"))?;
            for clip in [SET, COPY] {
                put(t, &format!("{clip}.mcTrick.labelText"), "text", text("#"))?;
            }
            for i in 1..=4 {
                go(p, &format!("mcPlayer{i}.mcNameStates"), "leader")?;
            }
            // Mission.GetIsLocalPlayerObserver: online players all skate.
            put(t, "", "_visible", Value::Bool(true))?;
        } else {
            put(t, "mcABD", "_visible", Value::Bool(false))?;
            put(t, "mcPhase", "_visible", Value::Bool(false))?;
            go(t, SET, "outro")?;
            go(t, COPY, "outro")?;
            go(p, "", &format!("outro{}", self.old_players.clamp(2, 4)))?;
        }
        self.visible = on;
        Ok(())
    }

    /// UpdatePlayerIcons: the active player's arrow and highlighted name.
    fn icons(&self, p: &mut Screen, v: &SkateView) -> Result<(), String> {
        for i in 1..=v.names.len() {
            put(p, &format!("mcPlayer{i}.mcOnlineArrow"), "_visible", Value::Bool(false))?;
            put(p, &format!("mcPlayer{i}.mcNameStates.mcTextName.labelText"), "textColor", Value::Number(WHITE))?;
        }
        let i = v.active + 1;
        put(p, &format!("mcPlayer{i}.mcOnlineArrow"), "_visible", Value::Bool(true))?;
        put(p, &format!("mcPlayer{i}.mcNameStates.mcTextName.labelText"), "textColor", Value::Number(HILIGHT))
    }

    /// ResetColours / UpdateFollowerWin / UpdateTrickWrong / ...: the trick lines' colour and animation.
    fn tint(t: &mut Screen, clips: &[&str], colour: f64, label: &str) -> Result<(), String> {
        for clip in clips {
            put(t, &format!("{clip}.mcTrick.labelText"), "textColor", Value::Number(colour))?;
            go(t, clip, label)?;
        }
        Ok(())
    }
}

/// A landed trick's name as SKATEMission.GetTrickLanded writes it (0x82462470 -> 0x82343708): the trick's localized
/// name ("%s"), with its spin "%s %d" (|180s| x 180) when it has one. Skate 1's english.big names are capitals
/// (KICKFLIP, POP SHUVIT).
// ponytail: the name is this game's trick label in capitals; Skate 1's own trick table isn't ported.
fn trick_name(labels: &BTreeMap<i64, String>, (trick, spins): (i64, i64)) -> String {
    let label = labels.get(&trick).map(|l| l.to_uppercase()).unwrap_or_default();
    if spins != 0 { format!("{label} {}", spins.unsigned_abs().saturating_mul(180)) } else { label }
}

// ---------------------------------------------------------------- the leaderboard
/// The leaderboard showing: its provider state, the countdown's end and kind, the overlay state on exit.
struct Board {
    push: Push,
    /// (ends at, lobby: the match is over).
    countdown: (f32, bool),
    /// Closing since.
    closing: Option<f32>,
}

/// The FELeaderboard provider's columns of `mode` in `state` (default.xex's providers: Best Trick 8, Spot 9, Jam
/// 15, Death Race 19, Spot Race 20, S.K.A.T.E. 4): header IDs as shown.
fn columns(mode: Mode, state: i64, rounds: i64) -> Vec<&'static str> {
    let mut c = vec!["#", "Gamertag:"];
    c.extend(match (mode, state) {
        (Mode::BestTrick, 0) | (Mode::Jam, 0) => &["Score:", "Points:"][..],
        (Mode::BestTrick, _) => &["Points:"],
        (Mode::Jam, 1 | 2) if rounds > 1 => &["Points:"],
        (Mode::Jam, _) => &["Score:"],
        (Mode::SpotRace, _) => &["Time:"],
        (Mode::DeathRace, _) => &["Gates Missed:", "Time:"],
        (Mode::Skate, 4) | (Mode::SpotBattle, 0 | 1 | 3..) => &["Wins:"],
        (Mode::Skate, _) => &["Letters:"],
        (Mode::SpotBattle, _) => &["Score:"],
        _ => &[],
    });
    c
}

/// Race time as the provider writes it (Mode::show's m:ss.cc).
fn race_time(s: &Standing) -> String {
    if s.out || !s.finished { "DNF".into() } else { Mode::DeathRace.show((s.time * 1000.).round() as i64) }
}

// ---------------------------------------------------------------- the HUD
/// One match's HUD.
pub(crate) struct Hud {
    runtime: PathBuf,
    mode: Mode,
    kind: Kind,
    challenge: u32,
    title: String,
    spot: String,
    /// The row's DebugName, which the scripts' event log lines name it by.
    debug: String,
    /// Death Race's allowed missed gates; Spot Race's spots.
    allowed: i64,
    spots: i64,
    /// The substate last handled (None: the overlay isn't up).
    sub: Option<i64>,
    facts: Facts,
    /// Contest.SetCompetitorScore / SetCompetitorPoints (this round), per player.
    round_scores: BTreeMap<usize, i64>,
    earned: BTreeMap<usize, i64>,
    forfeit: i64,
    /// HUD.SetActivePlayerIndex / Skate.UpdateCurrentPlayerIndex: the player up.
    active: usize,
    /// S.K.A.T.E.: the leader (Skate.SetPlayerOrder(id, 1)), the trick last landed (Set / CompareScratchPadPatternItem:
    /// trick, 180s) and whether it took off switch, the components.
    leader: usize,
    landed: (i64, i64),
    switched: bool,
    /// The local player's stance is regular (SKATEMission.IsTrickMirrored's profile flag; the hud system sets it).
    pub regular: bool,
    skate: SkateParts,
    /// Races: each player's next way point (ChallengeManager.NotifyPlayerNextWayPoint; kept past the finish).
    next: BTreeMap<usize, i64>,
    /// Spot Race: Mission.GetSpotRaceObjective per next way point (index next - 1; past the CheckPoints the finish line).
    objectives: Vec<String>,
    /// The race HUDs' split per player (the HUD player table's +0x60) and the HUD's updates.
    splits: BTreeMap<usize, f32>,
    updates: u64,
    central: Central,
    pending: Option<(f32, Push)>,
    again: Option<(f32, i64)>,
    board: Option<Board>,
    clock: f32,
    /// modes.json's strings (the event log's, the leaderboard countdown's).
    strings: Json,
    /// Event-log texts for online_hud.
    pub log: Vec<String>,
    /// The final leaderboard is over: the script's postchallengedisplayexit.
    pub exit: bool,
}

impl Hud {
    pub fn new(data: &Data, run: &Run, assets: &Path) -> Option<Self> {
        let row = data.rows.get(&run.row)?;
        let int = |k: &str| row.fields[k].as_f64().unwrap_or(0.) as i64;
        let spots = row.fields["CheckPoints"].as_array().map_or(0, Vec::len) as i64;
        // GetSpotRaceObjective (0x82346ed0) on the HUD's (trick type, points): points first, then the type's title
        // with 1, type None "Cross the finish line!". 0x823dc7d0 sets the pair (0x82346eb0) for the next way point e
        // after each gate: SpotTrickTypes / SpotSeqRequiredPoints[e - 1], (0, 0) for the finish line.
        let objectives = (0..=spots as usize)
            .map(|g| {
                let o = &data.modes["objectives"];
                let points = row.fields["SpotSeqRequiredPoints"][g].as_i64().unwrap_or(0);
                let trick = row.fields["SpotTrickTypes"][g].as_str().and_then(|t| data.enums["Trick"]["eTrickType"][t].as_u64()).unwrap_or(0);
                match (points, o["tricks"][trick as usize].as_str()) {
                    (1.., _) => fill(o["points"].as_str().unwrap_or_default(), &points.to_string()),
                    (_, Some(title)) if trick > 0 => fill(title, "1"),
                    _ => data.modes["strings"]["finish_line"].as_str().unwrap_or_default().to_owned(),
                }
            })
            .collect();
        Some(Self {
            runtime: assets.join(MOVIES),
            mode: run.mode,
            kind: kind(run.mode)?,
            challenge: row.kind,
            title: data.title(run.mode.name()),
            spot: row.name.clone(),
            debug: row.fields["DebugName"].as_str().unwrap_or_default().to_owned(),
            allowed: int("MaxAllowedGateMissing"),
            spots,
            sub: None,
            // Spot Battle's round count is its vault NumRounds (0x823dd8d0, hash 0xb27383eb921309de); the contests set
            // theirs with Contest.SetMaxRounds.
            facts: Facts { round: 1, rounds: if run.mode == Mode::SpotBattle { int("NumRounds").max(1) } else { 1 }, ..default() },
            round_scores: BTreeMap::new(),
            earned: BTreeMap::new(),
            forfeit: 255,
            active: 0,
            leader: 0,
            landed: (0, 0),
            switched: false,
            regular: true,
            skate: SkateParts::default(),
            next: BTreeMap::new(),
            objectives,
            splits: BTreeMap::new(),
            updates: 0,
            central: Central::default(),
            pending: None,
            again: None,
            board: None,
            clock: 0.,
            strings: data.modes["strings"].clone(),
            log: Vec::new(),
            exit: false,
        })
    }
    fn string(&self, key: &str) -> &str {
        self.strings[key].as_str().unwrap_or_default()
    }

    fn open(&self, screens: &mut BTreeMap<String, Screen>, name: &str, movie: &str) -> Result<(), String> {
        let mut screen = Screen::load(&self.runtime.join(format!("{movie}.json")), &[])?;
        bind(&mut screen)?;
        screens.insert(name.to_owned(), screen);
        Ok(())
    }

    /// Closes every screen this HUD opened.
    pub fn close(screens: &mut BTreeMap<String, Screen>) {
        for name in SCREENS {
            screens.remove(name);
        }
    }

    /// One frame: the script's HUD calls since the last, the timers, the components' answers.
    /// A step that fails (a movie missing or its script erring) never skips the others: the board's countdown and the
    /// exits it owes the script (postchallengedisplayexit) keep their time. The first error is returned.
    pub fn frame(&mut self, screens: &mut BTreeMap<String, Screen>, run: &mut Run, dt: f32) -> Result<(), String> {
        let mut first = None;
        let mut keep = |r: Result<(), String>| {
            if let Err(e) = r {
                first.get_or_insert(e);
            }
        };
        self.clock += dt;
        // The race HUDs take each player's split every 4th update (0x823426e0 / 0x823467f8).
        self.updates += 1;
        if self.updates % 4 == 0 {
            self.splits = run.standings.iter().map(|s| s.split).enumerate().collect();
        }
        if !screens.contains_key(CENTRAL) {
            keep(self.open(screens, CENTRAL, "centralhud"));
        }
        let me = run.me();
        self.facts.first = rank_order(self.mode, &run.standings, &run.order).first() == Some(&me);
        self.facts.out = run.standings.get(me).is_some_and(|s| s.out);
        self.answer(screens, run);
        // What the overlay's own frames asked of HUDHooks; the components' calls are answers only.
        if let Some(overlay) = screens.get_mut(OVERLAY) {
            let calls = overlay.take_calls();
            keep(self.hooks(screens, calls));
        }
        for name in [TIMER, RACE, PLAYERS, SPECTATOR, TRICKS, CENTRAL] {
            drop(screens.get_mut(name).map(Screen::take_calls));
        }
        for call in std::mem::take(&mut run.hud) {
            keep(self.call(screens, run, &call));
        }
        if let Some((_, sub)) = self.again.filter(|(at, _)| self.clock >= *at) {
            self.again = None;
            keep(self.substate(screens, run, sub));
        }
        if let Some((_, push)) = self.pending.filter(|(at, _)| self.clock >= *at) {
            self.pending = None;
            keep(self.show_board(screens, run, push));
        }
        keep(self.update_board(screens, run));
        let time = run.session.hud_time().unwrap_or(0.);
        if let Some(central) = screens.get_mut(CENTRAL) {
            keep(self.central.update(central, dt, time));
        }
        first.map_or(Ok(()), Err)
    }

    fn call(&mut self, screens: &mut BTreeMap<String, Screen>, run: &mut Run, call: &Call) -> Result<(), String> {
        let player = |i: usize| call.int(i).and_then(|i| usize::try_from(i).ok());
        match call.name.as_str() {
            "HUD.ChangeHUDState" => {
                let (state, sub) = (call.int(0).unwrap_or(0), call.int(1).unwrap_or(0));
                if state == self.kind.state {
                    self.substate(screens, run, sub)?;
                } else if state == FREE_SKATE {
                    // The mission HUD goes: its overlay, components and any leaderboard (one HUD state at a time).
                    for name in SCREENS.into_iter().filter(|&n| n != CENTRAL) {
                        screens.remove(name);
                    }
                    (self.sub, self.pending, self.again, self.board) = (None, None, None, None);
                    self.skate = SkateParts::default();
                }
            }
            "HUD.SetWinState" => self.facts.win = call.truthy(0),
            "HUD.SetActivePlayerIndex" | "Skate.UpdateCurrentPlayerIndex" => self.active = player(0).unwrap_or(0),
            "Skate.SetPlayerOrder" if call.int(1) == Some(1) => self.leader = player(0).unwrap_or(0),
            // (pattern item, trick, 180s, switch).
            "Skate.SetScratchPadPatternItem" | "Skate.CompareScratchPadPatternItem" => {
                (self.landed, self.switched) = ((call.int(1).unwrap_or(0), call.int(2).unwrap_or(0)), call.truthy(3))
            }
            // Bug 10-31: "<DebugName> started!" named the row by its code name; the row's title stands in.
            "HUD.AddEventLog" => {
                let line = call.text(0).trim_start_matches('#');
                let title = self.spot.strip_prefix('#').unwrap_or(&self.spot);
                self.log.push(if self.debug.is_empty() { line.to_owned() } else { line.replace(&self.debug, title) });
            }
            // ponytail: the start / finish lines name the mode (the C++ argument isn't recovered).
            "HUD.AddChallengeStartMessage" => self.log.push(fill(self.string("started"), &self.title)),
            "HUD.AddChallengeFinishMessage" => self.log.push(fill(self.string("finished"), &self.title)),
            "HUD.AddChallengeWinnerMessage" => {
                let name = player(0).and_then(|i| run.names.get(i)).cloned().unwrap_or_default();
                self.log.push(fill(self.string("won"), &name));
            }
            "Contest.SetCurrentRound" => self.facts.round = call.int(1).unwrap_or(1),
            "Contest.SetMaxRounds" => self.facts.rounds = call.int(1).unwrap_or(1),
            "Contest.SetForfeitTimeLeft" => self.forfeit = call.int(1).filter(|&t| t >= 0).unwrap_or(255),
            // (self, competitor 1-based, score); (competitor 1-based, points).
            "Contest.SetCompetitorScore" => drop(player(1).map(|i| self.round_scores.insert(i.saturating_sub(1), call.int(2).unwrap_or(0)))),
            "Contest.SetCompetitorPoints" => drop(player(0).map(|i| self.earned.insert(i.saturating_sub(1), call.int(1).unwrap_or(0)))),
            "Contest.ResetRoundScores" => (self.round_scores, self.earned) = (BTreeMap::new(), BTreeMap::new()),
            // The challenge's per-player record keeps it (cm+0x1db3c, player stride 0x160): the race HUDs read it.
            "ChallengeManager.NotifyPlayerNextWayPoint" => drop(player(0).map(|i| self.next.insert(i, call.int(1).unwrap_or(0)))),
            "ChallengeManager.SetCurrentRound" => self.facts.round = call.int(0).unwrap_or(1),
            _ => {}
        }
        Ok(())
    }

    /// The HUD handler on a substate of the mode's HUD state.
    fn substate(&mut self, screens: &mut BTreeMap<String, Screen>, run: &mut Run, sub: i64) -> Result<(), String> {
        // A race HUD's state change (0x82342a68) sets each player's split to their time.
        self.splits = run.standings.iter().map(|s| s.time).enumerate().collect();
        // What the substate leads to (the board, a later substate) first: a movie failing below can't lose it.
        let handled = overlay_states(self.mode, self.sub, sub, self.facts);
        self.sub = Some(sub);
        if let Some(push) = handled.push {
            self.pending = Some((self.clock + push.after, push));
        }
        if let Some((after, sub)) = handled.again {
            self.again = Some((self.clock + after, sub));
        }
        if !screens.contains_key(OVERLAY) {
            self.open(screens, TIMER, "timerhud")?;
            self.open(screens, RACE, "onlinehud")?;
            if self.mode == Mode::Skate {
                self.open(screens, PLAYERS, "skatehud")?;
                self.open(screens, TRICKS, "skatetrick")?;
                drop(screens.get_mut(TRICKS).map(|t| self.skate.open(t)).transpose()?);
            }
            self.open(screens, OVERLAY, self.kind.movie)?;
            self.answer(screens, run);
        }
        for state in handled.states {
            self.overlay(screens, run, state)?;
        }
        if handled.spectate {
            if !screens.contains_key(SPECTATOR) {
                self.open(screens, SPECTATOR, "spectator_mode")?;
            }
            self.answer(screens, run);
            drop(screens.get_mut(SPECTATOR).map(|s| s.call("SetState", &[json!(1)])).transpose()?);
        }
        Ok(())
    }

    /// The overlay's SetState and what it asked of HUDHooks.
    fn overlay(&mut self, screens: &mut BTreeMap<String, Screen>, run: &Run, state: i64) -> Result<(), String> {
        self.answer(screens, run);
        let Some(overlay) = screens.get_mut(OVERLAY) else { return Ok(()) };
        // The Spot overlay keeps its controller (what its RegisterScreen hands ScreenManager.OnLoaded) in `spotchal`,
        // the others in `screen`.
        match overlay.has("SetState") {
            true => overlay.call("SetState", &[json!(state)])?,
            false => overlay.call_at("spotchal", "SetState", &[json!(state)])?,
        };
        let calls = overlay.take_calls();
        if let (Some(mut p), Some(mut t)) = (screens.remove(PLAYERS), screens.remove(TRICKS)) {
            let done = self.skate.state(&mut p, &mut t, state, &self.skate_view(run));
            screens.insert(PLAYERS.into(), p);
            screens.insert(TRICKS.into(), t);
            done?;
        }
        self.hooks(screens, calls)
    }

    fn skate_view(&self, run: &Run) -> SkateView {
        let (abds, hit) = run.session.abds();
        SkateView {
            names: run.names.clone(),
            active: self.active,
            letters: run.standings.get(self.active).map_or(0, |s| s.letters),
            abds: abds.into_iter().map(|t| trick_name(&run.labels, t)).collect(),
            hit,
            landed: trick_name(&run.labels, self.landed),
            gesture: run.gestures.get(&self.landed.0).map(|&g| (g, self.mirrored())),
        }
    }

    /// SKATEMission.IsTrickMirrored (0x824624a0): the trick's switch take-off XOR the local profile's regular flag
    /// (skate1_missions skate_view).
    fn mirrored(&self) -> bool {
        self.switched != self.regular
    }

    fn hooks(&mut self, screens: &mut BTreeMap<String, Screen>, calls: Vec<(String, Vec<Json>)>) -> Result<(), String> {
        for (name, args) in calls {
            let Some((object, method)) = name.split_once('.') else { continue };
            match (object, method) {
                ("HUDHooks", _) => {
                    if let Some(central) = screens.get_mut(CENTRAL) {
                        self.central.hook(central, method, &args)?;
                    }
                }
                ("TimerHUD", "Show") | ("OnlineRaceHUD", "Show") => {
                    let name = if object == "TimerHUD" { TIMER } else { RACE };
                    if let Some(screen) = screens.get_mut(name) {
                        screen.call("Show", &[args.first().cloned().unwrap_or(json!(false))])?;
                    }
                }
                // ponytail: hudink / hudobjective (the ink splats and the offline objective box) aren't converted.
                _ => {}
            }
        }
        Ok(())
    }

    fn show_board(&mut self, screens: &mut BTreeMap<String, Screen>, run: &Run, push: Push) -> Result<(), String> {
        let lobby = !(matches!(self.mode, Mode::BestTrick | Mode::Jam) && self.facts.round < self.facts.rounds);
        let ends = self.clock + if lobby { LOBBY_FOR } else { MOVING_FOR };
        self.board = Some(Board { push, countdown: (ends, lobby), closing: None });
        self.open(screens, BOARD, "leaderboard")?;
        self.answer(screens, run);
        if let Some(board) = screens.get_mut(BOARD) {
            board.call("Initialise", &[])?;
        }
        if let Some(state) = push.show {
            self.overlay(screens, run, state)?;
        }
        Ok(())
    }

    /// The board's countdown; its refresh from round to standings; its outro and end.
    /// The countdown runs even when the leaderboard movie failed to open: the script still gets its exit.
    fn update_board(&mut self, screens: &mut BTreeMap<String, Screen>, run: &mut Run) -> Result<(), String> {
        let Some(board) = &mut self.board else { return Ok(()) };
        let mut screen = screens.get_mut(BOARD);
        // FELeaderboard.CheckForRefresh (the movie asks at its 300th frame online): a round board moves on to the
        // standings.
        // ponytail: the provider's refresh rule isn't recovered; a round board (state 0) turns to the current / final
        // standings (1 / 2), as the provider's title details read.
        let refresh = screen.as_mut().is_some_and(|s| s.take_calls().iter().any(|(c, _)| c == "FELeaderboard.CheckForRefresh"))
            && board.push.state == 0;
        if refresh {
            board.push.state = if self.facts.round >= self.facts.rounds { 2 } else { 1 };
        }
        let closing = match board.closing {
            None if self.clock >= board.countdown.0 => {
                board.closing = Some(self.clock);
                drop(screen.map(|s| s.play("outro")).transpose()?);
                None
            }
            Some(since) if self.clock >= since + BOARD_OUTRO => Some((board.countdown.1, board.push.exit)),
            _ => None,
        };
        if refresh {
            self.answer(screens, run);
            drop(screens.get_mut(BOARD).map(|s| s.call("StartRefresh", &[])).transpose()?);
        }
        if let Some((lobby, exit)) = closing {
            self.board = None;
            screens.remove(BOARD);
            // The script's exit first: the overlay failing below can't hold it back.
            if lobby {
                self.exit = true;
                run.session.later(0., "postchallengedisplayexit", json!({}));
            }
            if let Some(state) = exit {
                self.overlay(screens, run, state)?;
                // The races' exit callbacks hide the spectator overlay too.
                drop(screens.get_mut(SPECTATOR).map(|s| s.call("SetState", &[json!(0)])).transpose()?);
            }
        }
        Ok(())
    }

    /// Everything the open movies ask, as of now.
    fn answer(&self, screens: &mut BTreeMap<String, Screen>, run: &Run) {
        let me = run.me();
        let n = run.names.len().max(1);
        let order = rank_order(self.mode, &run.standings, &run.order);
        let standing = |i: usize| run.standings.get(i).cloned().unwrap_or_default();
        let time = run.session.hud_time();
        let turn = matches!(self.mode, Mode::SpotBattle | Mode::Skate);
        // The race HUDs' lap fields: Death Race's (0x82342de8) missed (at most MaxAllowed + 1) / MaxAllowed + 1, Spot
        // Race's (0x82346d18) next way point / way points - 1 (start line, CheckPoints, finish line).
        let next = self.next.get(&me).copied().unwrap_or(0);
        let two = |key: &str, a: i64, b: i64| fill(self.string(key), &a.to_string()).replace("{1:d}", &b.to_string());
        let lap = match self.mode {
            Mode::DeathRace => two("gates_missed", standing(me).missed.min(self.allowed + 1), self.allowed + 1),
            Mode::SpotRace => two("spots", next, self.spots + 1),
            // Spot Battle's (0x82342308): current round / NumRounds.
            Mode::SpotBattle => two("rounds", self.facts.round, self.facts.rounds),
            _ => String::new(),
        };
        let split = |i: usize| -> String {
            let s = standing(i);
            match self.mode {
                Mode::BestTrick | Mode::Jam => self.round_scores.get(&i).copied().unwrap_or(0).to_string(),
                Mode::SpotBattle => s.score.to_string(),
                // 0x82342ca8 / Spot Race's: out "DNF", finished the time, else the split: none yet "--:--".
                Mode::DeathRace | Mode::SpotRace if s.out || s.finished => race_time(&s),
                Mode::DeathRace | Mode::SpotRace => match self.splits.get(&i).copied().unwrap_or(0.) {
                    t if t == 0. => "--:--".into(),
                    t => format!("+{}", Mode::DeathRace.show((t * 1000.).round() as i64)),
                },
                _ => String::new(),
            }
        };
        let mut common: Vec<(String, Json)> = vec![
            ("Mission.GetIsOnlineMission".into(), json!(true)),
            ("Mission.GetIsPartyPlayMission".into(), json!(false)),
            ("Mission.GetIsRankedMission".into(), json!(false)),
            ("Mission.GetChallengeType".into(), json!(self.challenge)),
            ("Mission.GetLocalPlayerIndex".into(), json!(me)),
            ("Mission.GetNumberOfPlayers".into(), json!(n)),
            ("Mission.GetCurrentPlayerPosition".into(), json!(order.iter().position(|&i| i == me).map_or(1, |p| p + 1))),
            ("Mission.GetCurrentTime".into(), json!(time.unwrap_or(0.))),
            ("Mission.GetTimerHUDText".into(), timer_answer(time)),
            ("Mission.GetRaceHUDTitle".into(), json!(self.title)),
            ("Mission.GetRaceHUDLapField".into(), json!(lap)),
            ("Mission.GetRaceHUDStyle".into(), json!(0)),
            ("Mission.GetIsTurnBased".into(), json!(turn)),
            ("Mission.GetIconString".into(), json!("")),
            ("BestTrickMission.GetCurrentRound".into(), json!(self.facts.round)),
            ("BestTrickMission.GetForfeitTimeLeft".into(), json!(self.forfeit)),
            ("JamMission.GetCurrentRound".into(), json!(self.facts.round)),
            ("JamMission.GetForfeitTimeLeft".into(), json!(self.forfeit)),
            ("OwnTheSpotMission.GetActivePlayerIndex".into(), json!(self.active)),
            ("OwnTheSpotMission.GetActivePlayerName".into(), json!(run.names.get(self.active).cloned().unwrap_or_default())),
            ("DeathRaceMission.GetGatesMissed".into(), json!(standing(me).missed)),
            ("DeathRaceMission.GetGatesMissedAllowed".into(), json!(self.allowed)),
            ("Mission.GetSpotRaceObjective".into(), json!(self.objectives.get((next.max(1) - 1) as usize).cloned().unwrap_or_default())),
            // Online, everyone skates; spectator_mode names the skater the camera follows.
            ("Mission.GetIsLocalPlayerObserver".into(), json!(false)),
            ("Mission.GetViewingPlayerName".into(), json!(run.names.get(run.camera.unwrap_or(me)).cloned().unwrap_or_default())),
            ("SkateMission.GetNumPlayers".into(), json!(run.names.len())),
            ("SkateMission.GetPlayerLeader".into(), json!(self.leader)),
            ("SkateMission.GetActivePlayerIndex".into(), json!(self.active)),
            ("SKATEMission.GetActivePlayerIndex".into(), json!(self.active)),
            ("SKATEMission.IsTrickMirrored".into(), json!(self.mirrored())),
        ];
        for (i, name) in run.names.iter().enumerate() {
            common.push((format!("SkateMission.GetPlayerName({i})"), json!(name)));
            common.push((format!("SKATEMission.GetPlayerName({i})"), json!(name)));
            common.push((format!("SkateMission.GetPlayerScore({i})"), json!(standing(i).letters)));
        }
        // onlinehud: GetPlayersData(row 1-based, by rank): [position, name, split, -, player, voip, disconnected].
        for (row, &i) in order.iter().enumerate() {
            let name = run.names.get(i).cloned().unwrap_or_default();
            let gone = name.is_empty() && i != me;
            common.push((format!("Mission.GetPlayersData({})", row + 1), json!([row + 1, name, split(i), "", i, crate::voice::voip_named(&name), gone])));
        }
        if let Some(board) = &self.board {
            common.extend(self.board_answers(&run.names, &run.standings, &order, board.push.state, board.countdown));
        }
        // The overlays' FELanguage.GetLocalizedFormatString(id, 1, value), in centralhud's strings (where they show).
        let language = screens.get(CENTRAL).map(|s| s.bindings.movie.text_assets.language.clone()).unwrap_or_default();
        let name = |i: usize| run.names.get(i).cloned().unwrap_or_default();
        for (id, value) in [
            ("ID_MISSION_ROUND_NOTIFICATION", self.facts.round.to_string()),
            ("ID_COMMON_MISSION_LEFT_AREA_WARNING", self.forfeit.to_string()),
            ("ID_RACE_GATES_MISSED", standing(me).missed.to_string()),
            ("ID_MISSION_START_TURN_NOTIFICATION", name(self.active)),
            ("ID_SKATE_IS_LEADER", name(self.leader)),
            ("ID_SKATE_COPY_TRICK", name(self.active)),
            ("ID_SKATE_PLAYER_BAILED", name(self.active)),
            ("ID_SKATE_AI_IS_UP", name(self.active)),
        ] {
            let text = language.get(id).map_or(id, String::as_str);
            common.push((format!("FELanguage.GetLocalizedFormatString({id},1)"), json!(fill(text, &value))));
        }
        for screen in screens.values_mut() {
            for (key, value) in &common {
                screen.answer(key, value.clone());
            }
        }
    }

    /// FELeaderboard's answers for provider state `state`, its rows in `order`.
    fn board_answers(&self, names: &[String], standings: &[Standing], order: &[usize], state: i64, (ends, lobby): (f32, bool)) -> Vec<(String, Json)> {
        let headers = columns(self.mode, state, self.facts.rounds);
        let details = match (self.mode, state) {
            (Mode::BestTrick, 0) => format!("ROUND {} - {}", self.facts.round, self.spot),
            (Mode::BestTrick, 1) => format!("CURRENT STANDINGS - {}", self.spot),
            (Mode::BestTrick, _) => format!("FINAL STANDINGS - {}", self.spot),
            _ => self.spot.clone(),
        };
        let left = (ends - self.clock).ceil().max(0.) as i64;
        let timer = fill(self.string(if lobby { "lobby" } else { "moving" }), &left.to_string());
        let mut out = vec![
            ("FELeaderboard.GetLeaderBoardTitle".into(), json!(self.title)),
            ("FELeaderboard.GetLeaderBoardTitleDetails".into(), json!(details)),
            ("FELeaderboard.GetAdditionalInfoType".into(), json!(1)),
            ("FELeaderboard.GetOptionsEnum".into(), json!(0)),
            ("FELeaderboard.GetNumColumns".into(), json!(headers.len())),
            ("FELeaderboard.GetRowHeader".into(), json!(headers.iter().map(|h| format!("#{h}")).collect::<Vec<_>>())),
            ("FELeaderboard.GetTimerText".into(), json!(timer)),
            ("FELeaderboard.GetMessage".into(), json!("#")),
            ("FELeaderboard.GetBottomState".into(), json!(1)),
            ("FELeaderboard.GetVoipStatus".into(), json!(VOIP_OFF)),
        ];
        let score = |v: i64| if v == 0 { "DNF".to_owned() } else { v.to_string() };
        for (row, &i) in order.iter().enumerate() {
            let s = standings.get(i).cloned().unwrap_or_default();
            let mut cells = vec![(row + 1).to_string(), names.get(i).cloned().unwrap_or_default()];
            for h in &headers[2..] {
                cells.push(match *h {
                    "Score:" if state == 0 => score(self.round_scores.get(&i).copied().unwrap_or(s.score)),
                    "Score:" => score(s.score),
                    "Points:" if state == 0 => format!("+{}", self.earned.get(&i).copied().unwrap_or(0)),
                    "Points:" => s.points.to_string(),
                    "Time:" => race_time(&s),
                    // "(+{0:d}sec) {1:d}" (0xb0efd81c) with a penalty, else the count.
                    "Gates Missed:" => match s.penalty as i64 {
                        p if p > 0 => format!("(+{p}sec) {}", s.missed),
                        _ => s.missed.to_string(),
                    },
                    "Letters:" => letters(s.letters),
                    "Wins:" => s.points.to_string(),
                    _ => String::new(),
                });
            }
            out.push((format!("FELeaderboard.GetRowData({row})"), json!(cells)));
            out.push((format!("FELeaderboard.GetPlayerIndex({row})"), json!(i)));
            // UpdateVoipStatus asks GetVoipStatus(GetPlayerIndex(row)): voice::voip_status by the name it shows.
            out.push((format!("FELeaderboard.GetVoipStatus({i})"), json!(crate::voice::voip_named(&cells[1]))));
        }
        out
    }
}

/// S.K.A.T.E. letters as the provider writes them: none FLAWLESS, else "S.", "S.K.", ...
fn letters(n: i64) -> String {
    if n <= 0 { "FLAWLESS".into() } else { "SKATE".chars().take(n.min(5) as usize).map(|c| format!("{c}.")).collect() }
}

/// Mission.GetTimerHUDText: [text, seconds] for the HUD timer's seconds left (None: no timer).
pub(crate) fn timer_answer(time: Option<f32>) -> Json {
    match time {
        None => json!(["#", -1]),
        Some(t) => {
            let s = t.ceil() as i64;
            let text = match s {
                0..=9 => format!(":0{s}"),
                10..=59 => format!(":{s}"),
                _ => format!("{}:{:02}", s / 60, s % 60),
            };
            json!([text, s])
        }
    }
}

/// Skate 1's button_item constructor attaches "PS3_" + button when PlatformCurrent == Platform_PS3, else "Xbox_" +
/// button. Unset, the two undefineds were equal and every pad got the PS3 art (playtest 10-15: Triangle on the sign-up
/// prompt). apt_screen sets these before a Skate 1 movie's first frame: the prompt's Y (actionoverlay
/// main_mc.mcDpad's button_item) is placed there and constructs during the load (run12: still Triangle while `bind`
/// set them after the load).
/// INFERRED: Skate 1's values are the boot screens' (Skate 3); the movies only compare them.
/// ponytail: read when the movie loads; a pad swapped while it is up keeps the art it opened with.
pub(crate) fn platform_globals(vm: &mut crate::apt_vm::Vm) -> Result<(), String> {
    let ps3 = crate::apt_scene::PS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
    use crate::boot_screens::{PLATFORM_PS3, PLATFORM_XBOX360};
    for (name, value) in [
        ("Platform_XBOX360", PLATFORM_XBOX360),
        ("Platform_PS3", PLATFORM_PS3),
        ("PlatformCurrent", if ps3 { PLATFORM_PS3 } else { PLATFORM_XBOX360 }),
    ] {
        vm.set(vm.global, name, Value::Number(value))?;
    }
    Ok(())
}

/// The natives the Skate 1 HUD movies address beyond apt_screen's: HUDHooks (its components as properties), the
/// mission objects, Skate 1's ScreenManager colours.
pub(crate) fn bind(screen: &mut Screen) -> Result<(), String> {
    let vm = &mut screen.vm;
    let hooks = vm.object(ObjectKind::Native("HUDHooks".into()));
    for name in HOOKS {
        let o = vm.object(ObjectKind::Native(name.into()));
        vm.set(hooks, name, Value::Object(o))?;
    }
    vm.set(vm.global, "HUDHooks", Value::Object(hooks))?;
    // default.xex hands main.apt's RegisterChallengeEnum the text "22" (0x82465784; UNTRACED whether its body keeps
    // text or a number): onlinehud only compares it with ==, the same either way.
    vm.set(vm.global, "eChallengeTypes_OnlineFreeSkate", Value::Number(ONLINE_FREE_SKATE as f64))?;
    for name in MISSIONS {
        let o = vm.object(ObjectKind::Native(name.into()));
        vm.set(vm.global, name, Value::Object(o))?;
    }
    if let Value::Object(manager) = vm.get(vm.global, "ScreenManager") {
        for (name, rgb) in COLOURS {
            vm.set(manager, &format!("mColour_{name}"), Value::Number(f64::from(rgb)))?;
        }
    }
    Ok(())
}

/// A retail string with its `{0:s}` / `{0:d}` filled; '^' is the strings' line break.
pub(crate) fn fill(text: &str, value: &str) -> String {
    // Line breaks first: a '^' in a player's name stays a '^' (unbroken: the layout breaks a bare one too).
    let value = crate::apt_text::unbroken(value);
    text.replace('^', "\n").replace("{0:s}", &value).replace("{0:d}", &value)
}

/// Players best first: the races' and Own The Spot's are the challenge manager's (`manager`, sorted every frame:
/// 0x823dbe80; Own The Spot's copied to the HUD by 0x823dd858); Best Trick / Jam / S.K.A.T.E. win by points then
/// score, the scripts' own rank (SetOverallStanding / Skate.SetPlayerRank) winning when every player has one.
fn rank_order(mode: Mode, standings: &[Standing], manager: &[usize]) -> Vec<usize> {
    if matches!(mode, Mode::DeathRace | Mode::SpotRace | Mode::SpotBattle) && manager.len() == standings.len() {
        return manager.to_vec();
    }
    let mut order: Vec<usize> = (0..standings.len()).collect();
    if standings.iter().all(|s| s.rank > 0) {
        order.sort_by_key(|&i| standings[i].rank);
        return order;
    }
    order.sort_by(|&a, &b| {
        let (a, b) = (&standings[a], &standings[b]);
        match mode {
            Mode::DeathRace | Mode::SpotRace => {
                let key = |s: &Standing| (!s.finished || s.out, (s.time * 1000.) as i64);
                key(a).cmp(&key(b))
            }
            Mode::Skate => a.letters.cmp(&b.letters),
            _ => (b.points, b.score).cmp(&(a.points, a.score)),
        }
    });
    order
}

// ---------------------------------------------------------------- Bevy side
pub(crate) struct Skate1OnlineHudPlugin;
impl Plugin for Skate1OnlineHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, hud.after(crate::skate1_online::drive).before(crate::apt_screen::render));
    }
}

/// The pad under a Skate 1 match, Skate 3's rules (user 10-28). Emotes: the mode's own waits (its `hold`: a turn, the
/// flythrough, a countdown) pass the D-pad emotes and its leaderboards do not, as skate3_modes::gestures. B on the final
/// leaderboard (`board` Some(true); Some(false) a round's) asks "Quit Session?" (online_menu's Quit, retail popup 63) as
/// skate3_modes::online_results does on Skate 3's results: online, not while the popup is up or on the B that closed it.
// ponytail: the board draws no Quit legend (Skate 1's leaderboard movie has none); B works unshown.
fn pad(m: &mut crate::match_core::Match, board: Option<bool>, pressed: u16, ui: Option<&mut crate::online_menu::OnlineUi>) {
    m.gestures = crate::skate3_modes::gestures(m, board.is_some());
    if let Some(ui) = ui
        && board == Some(true)
        && m.token != 0
        && pressed & 0x2000 != 0
        && !ui.modal
        && !ui.closed_this_frame
    {
        ui.requests.push(crate::online_menu::OnlineRequest::QuitSession);
    }
}

/// The running match's HUD; closed with the match.
#[allow(clippy::too_many_arguments)]
fn hud(
    mut state: Local<Option<(String, Hud)>>,
    mut last: Local<String>,
    mut online: ResMut<Skate1Online>,
    screens: Option<ResMut<Screens>>,
    mut roster: ResMut<OnlineRoster>,
    config: Res<crate::config::Config>,
    time: Res<Time<Real>>,
    matched: Option<ResMut<crate::match_core::Match>>,
    nav: Option<Res<crate::customiser::Navigation>>,
    mut ui: Option<ResMut<crate::online_menu::OnlineUi>>,
    skater: Option<Res<crate::physics::SkaterRuntime>>,
) {
    let Some(mut screens) = screens else { return };
    let online = &mut *online;
    let open = &mut screens.layer(SKATE1_ONLINE).open;
    let (Some(run), Some(data)) = (online.run.as_mut(), online.data.clone()) else {
        if state.take().is_some() {
            Hud::close(open);
        }
        return;
    };
    // A match's run starts after a frame without one (drive ends the last with the phase), so the row tells them apart.
    if state.as_ref().is_none_or(|(row, _)| *row != run.row) {
        Hud::close(open);
        *state = Hud::new(&data, run, &config.asset_root).map(|h| (run.row.clone(), h));
    }
    let Some((_, hud)) = state.as_mut() else { return };
    // Party Play: the turn's player rides the skater, so its stance is theirs.
    hud.regular = skater.is_none_or(|s| !s.animation.goofy());
    // The HUD failing never stops the match, nor its timing (Hud::frame): it carries on, logging each error once.
    if let Err(error) = hud.frame(open, run, time.delta_secs()) {
        if *last != error {
            warn!("Skate 1 online HUD: {error}");
            *last = error;
        }
    }
    for text in hud.log.drain(..) {
        roster.lines.push(("{0:s}", text));
    }
    if let Some(mut m) = matched {
        pad(&mut m, hud.board.as_ref().map(|b| b.countdown.1), nav.map_or(0, |n| n.pressed), ui.as_deref_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_core::{Match, Phase};
    use crate::online_menu::{OnlineRequest, OnlineUi};

    /// A Skate 1 match running with its mode's hold (a turn wait, the flythrough, a countdown).
    fn held() -> Match {
        let mut m = Match::default();
        (m.token, m.hold, m.phase) = (7, true, Phase::Running { started: 0., ends: None });
        m
    }

    /// Skate 3's rule (user 10-28, as skate3_modes::gestures): the mode's own waits pass the D-pad emotes, its
    /// leaderboards own the pad. Old code: the Skate 1 glue never wrote Match::gestures (no `pad`: does not compile).
    #[test]
    fn skate1_waits_pass_the_emotes_and_its_leaderboards_do_not() {
        let mut m = held();
        pad(&mut m, None, 0, None);
        assert!(m.gestures, "a turn wait: the emotes go through");
        pad(&mut m, Some(false), 0, None);
        assert!(!m.gestures, "a round's leaderboard");
        pad(&mut m, Some(true), 0, None);
        assert!(!m.gestures, "the final one");
        m.hold = false;
        pad(&mut m, None, 0, None);
        assert!(!m.gestures, "riding: the core's rule");
    }

    /// Skate 3's rule (user 10-28, as skate3_modes::online_results): B on the final Skate 1 leaderboard asks "Quit
    /// Session?"; not on a round's, not while the popup is up or on the B that closed it, not offline. Old code: no
    /// `pad` (does not compile); B did nothing.
    #[test]
    fn b_on_the_final_skate1_leaderboard_asks_to_quit_the_session() {
        const B: u16 = 0x2000;
        let mut m = held();
        let mut ui = OnlineUi::default();
        pad(&mut m, Some(false), B, Some(&mut ui));
        pad(&mut m, Some(true), 0, Some(&mut ui));
        assert!(ui.requests.is_empty(), "a round's board, or no B");
        pad(&mut m, Some(true), B, Some(&mut ui));
        assert_eq!(ui.requests, [OnlineRequest::QuitSession]);
        ui.requests.clear();
        ui.modal = true;
        pad(&mut m, Some(true), B, Some(&mut ui));
        (ui.modal, ui.closed_this_frame) = (false, true);
        pad(&mut m, Some(true), B, Some(&mut ui));
        ui.closed_this_frame = false;
        m.token = 0;
        pad(&mut m, Some(true), B, Some(&mut ui));
        assert!(ui.requests.is_empty(), "the popup's own B, or offline");
    }

    #[test]
    fn best_trick_rounds_end_on_round_complete_and_the_last_on_the_result() {
        let f = Facts { round: 1, rounds: 3, win: true, ..default() };
        let h = overlay_states(Mode::BestTrick, Some(1), 2, f);
        assert_eq!(h.states, [20]);
        assert_eq!(h.push, Some(Push { state: 0, after: 2.7, show: Some(3), exit: None }));
        assert_eq!(overlay_states(Mode::BestTrick, Some(1), 2, Facts { round: 3, ..f }).states, [22]);
        assert_eq!(overlay_states(Mode::BestTrick, Some(1), 2, Facts { round: 3, win: false, ..f }).states, [23]);
        // After the countdown or the load, the overlay is told 21 first (its timer shows again on Active).
        assert_eq!(overlay_states(Mode::BestTrick, Some(10), 1, f).states, [21, 1]);
        assert_eq!(overlay_states(Mode::BestTrick, Some(1), 1, f).states, [1]);
    }

    /// The lobby: Online Freeskate's overlay or the mode's in kStateLobby, and the rows in table order (at most 6,
    /// -1 past them).
    /// Voice chat: the lobby's GetPlayersData carries each player's voice::voip_status (onlinehud's UpdateVOIPIcon: 2
    /// active, 1 on, 0 off).
    #[test]
    fn a_talker_lights_skate1s_rows() {
        crate::voice::hear("Voip S1 Ann", 9101, 2);
        crate::voice::hear("Voip S1 Bo", 9102, 1);
        let names: Vec<String> = ["Voip S1 Ann", "Voip S1 Bo", "Voip S1 Cy"].map(String::from).to_vec();
        let a = lobby_answers(&names, 0, "t");
        let voip = |row: usize| a.iter().find(|(k, _)| *k == format!("Mission.GetPlayersData({row})")).unwrap().1[5].clone();
        assert_eq!([voip(1), voip(2), voip(3)], [json!(2), json!(1), json!(VOIP_OFF)]);
    }

    #[test]
    fn lobby_overlay_and_rows() {
        assert_eq!(lobby_overlay(None), Some(("hud_online_freeskate_overlay", 1, 0)));
        assert_eq!(lobby_overlay(Some(Mode::SpotBattle)), Some(("hud_spot_challenge_overlay", 100, 0)));
        assert_eq!(lobby_overlay(Some(Mode::Jam)), Some(("hud_jam_challenge_overlay", 100, 3)));
        assert_eq!(lobby_overlay(Some(Mode::SpotRace)), Some(("hud_spotrace_challenge_overlay", 100, 7)));
        let names: Vec<String> = (0..7).map(|i| format!("p{i}")).collect();
        let a = lobby_answers(&names, 2, "Freeskate");
        let get = |k: &str| a.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("Mission.GetChallengeType"), Some(json!(22)));
        assert_eq!(get("Mission.GetNumberOfPlayers"), Some(json!(6)));
        assert_eq!(get("Mission.GetOnlineFreeskatePositions"), Some(json!([1, 2, 3, 4, 5, 6])));
        assert_eq!(get("Mission.GetPlayersData(3)"), Some(json!([3, "p2", "", false, 2, VOIP_OFF, false])));
        assert_eq!(get("Mission.GetPlayersData(7)"), None);
        let a = lobby_answers(&names[..2], 0, "t");
        assert_eq!(a.iter().find(|(k, _)| k == "Mission.GetOnlineFreeskatePositions").unwrap().1, json!([1, 2, -1, -1, -1, -1]));
    }

    #[test]
    fn race_and_skate_tables() {
        let f = Facts { win: false, out: true, ..default() };
        assert_eq!(overlay_states(Mode::DeathRace, None, 13, f).states, [1, 16]);
        assert_eq!(overlay_states(Mode::DeathRace, None, 16, f).states, [18]);
        assert_eq!(overlay_states(Mode::DeathRace, None, 5, f).push.unwrap().exit, Some(0));
        let sr = overlay_states(Mode::SpotRace, None, 12, f);
        assert_eq!((sr.states, sr.again), (vec![12], Some((0.8, 3))));
        assert_eq!(overlay_states(Mode::Skate, None, 1, f).states, [7, 32]);
        assert_eq!(overlay_states(Mode::Skate, None, 11, Facts { first: true, ..f }).states, [22]);
        assert_eq!(overlay_states(Mode::Jam, None, 12, Facts { rounds: 2, ..f }).states, [12, 24]);
        // The races' Spectator substates open spectator_mode, nothing on the overlay.
        for (mode, sub) in [(Mode::DeathRace, 17), (Mode::SpotRace, 16)] {
            let h = overlay_states(mode, None, sub, f);
            assert!(h.spectate && h.states.is_empty());
        }
        assert!(!overlay_states(Mode::DeathRace, None, 16, f).spectate);
    }

    /// Bug 10-31: Spot Battle showed no time left. The countdown's states (Lua 11 / 15: movie 10 / 14) hide TimerHUD;
    /// the turn's start shows it again with state 21, whatever comes next.
    #[test]
    fn spot_battle_shows_its_clock_again_after_the_countdown() {
        let f = Facts::default();
        assert_eq!(overlay_states(Mode::SpotBattle, None, 11, f).states, [10], "the countdown hides it");
        assert_eq!(overlay_states(Mode::SpotBattle, Some(11), 2, f).states, [21, 1], "the turn shows it");
        assert_eq!(overlay_states(Mode::SpotBattle, Some(15), 2, f).states, [21, 1]);
        assert_eq!(overlay_states(Mode::SpotBattle, Some(2), 2, f).states, [1], "only after a countdown");
        assert_eq!(overlay_states(Mode::SpotBattle, Some(11), 10, f).push, overlay_states(Mode::SpotBattle, None, 10, f).push);
    }

    /// Bug 10-31: the 3-2-1 was silent. hudbase plays PlayCountDownGenSound on each of 3, 2, 1 and
    /// PlayCountDownGenGoSound with the end message (online, every mode); a timer past 3 ends silently.
    #[test]
    fn the_countdown_sounds_each_number_and_its_go() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(PathBuf::from) else { return };
        let mut screen = Screen::load(&assets.join(MOVIES).join("centralhud.json"), &[]).unwrap();
        bind(&mut screen).unwrap();
        let mut central = Central::default();
        central.hook(&mut screen, "ShowCentralCountdownTimer", &[json!("ID_MISSION_COUNTDOWN_GO")]).unwrap();
        screen.bindings.sounds.clear();
        for k in (0..=180).rev() {
            central.update(&mut screen, 1. / 60., k as f32 / 60.).unwrap();
        }
        assert_eq!(screen.bindings.sounds, [COUNTDOWN_TICK, COUNTDOWN_TICK, COUNTDOWN_TICK, COUNTDOWN_GO]);
        screen.bindings.sounds.clear();
        central.hook(&mut screen, "ShowCentralCountdownTimer", &[json!("ID_MISSION_COUNTDOWN_GO")]).unwrap();
        central.update(&mut screen, 1. / 60., 10.).unwrap();
        assert!(screen.bindings.sounds.is_empty(), "past 3: silent");
    }

    /// Bug 10-31: the 3-2-1 was far louder than the HUD's other sounds (its samples are near full scale). As played
    /// (sfx/gains.json, apt_screen::fe_gain; then the SFX setting), its loudest 400 ms is -15 dB, the level of the
    /// HUD's quieter event sounds (the leaderboard's shuffle and out).
    #[test]
    #[ignore = "requires the installed Skate 1 online HUD (SKATE3_ASSET_ROOT)"]
    fn the_countdown_plays_at_the_huds_level() {
        let sfx = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private/hud-skate1/online/sfx");
        let loudest = |key: &str| {
            let wav = std::fs::read(sfx.join(format!("{key}.wav"))).unwrap();
            let at = wav.windows(4).position(|w| w == b"data").unwrap() + 8;
            // Mono 44.1 kHz i16.
            let pcm: Vec<f32> = wav[at..].chunks_exact(2).map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.).collect();
            let window = 17_640.min(pcm.len());
            let power = pcm.windows(window).step_by(441).map(|w| w.iter().map(|x| x * x).sum::<f32>() / window as f32).fold(0., f32::max);
            10. * power.log10() + 20. * crate::apt_screen::fe_gain(&sfx, key).log10()
        };
        for key in [COUNTDOWN_TICK, COUNTDOWN_GO] {
            let db = loudest(key);
            assert!((-16.0..=-14.0).contains(&db), "{key}: {db:.1} dB");
        }
    }

    /// Bug 10-31 (Party Play): the S.K.A.T.E. circle never showed the trick's stick. Retail's UpdateTrickList shows it
    /// online too (Tricks.GetGestureString, FlipGesture(SKATEMission.IsTrickMirrored)): mirrored when the trick's
    /// switch take-off differs from the local player's regular stance, as offline (skate1_missions skate_view).
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn the_skate_circle_shows_the_tricks_stick_in_the_players_stance() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let data = crate::skate1_online::tests::data();
        let mut run = Run::new(&data, &data.rows["skate_financial_matrix"], vec!["Alice".into(), "Bob".into()], 0, 8).unwrap();
        run.start(&data).unwrap();
        let mut hud = Hud::new(&data, &run, &assets).unwrap();
        let air = json!({"PlayerID": 0, "FirstScorableId": 7, "Wipedout": 0,
            "S": {"first": 7, "first_type": 2, "label": "Kickflip", "id": "kickflip"}});
        let players = run.names.clone();
        run.frame(&data, 1. / 60., vec![("aircompleted".into(), air)], vec![], &players).unwrap();
        let mut screens = BTreeMap::new();
        for (regular, switched, mirrored) in [(true, false, true), (false, false, false), (true, true, false), (false, true, true)] {
            hud.regular = regular;
            let call = Call { name: "Skate.SetScratchPadPatternItem".into(), args: vec![json!(1), json!(7), json!(0), json!(switched)] };
            hud.call(&mut screens, &mut run, &call).unwrap();
            assert_eq!(hud.skate_view(&run).gesture, Some(("mckickflip2", mirrored)), "regular {regular}, switch {switched}");
        }
    }

    /// Bug 10-31: a Spot Battle's start read "ol_spotbattle_oldtown_warehousesteps started!" at the top left (the disc's
    /// AddEventLog names the row by its DebugName). The row's title stands in, online and in Party Play.
    #[test]
    #[ignore = "requires the installed Skate 1 online data (SKATE3_ASSET_ROOT)"]
    fn the_event_log_names_a_row_by_its_title() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let data = crate::skate1_online::tests::data();
        let mut run = Run::new(&data, &data.rows["ol_spotbattle_oldtown_warehouststeps"], vec!["Alice".into(), "Bob".into()], 0, 8).unwrap();
        let mut hud = Hud::new(&data, &run, &assets).unwrap();
        let mut screens = BTreeMap::new();
        for line in ["#ol_spotbattle_oldtown_warehousesteps started!", "#ol_spotbattle_oldtown_warehousesteps completed!", "#Bob won!"] {
            let call = Call { name: "HUD.AddEventLog".into(), args: vec![json!(line)] };
            hud.call(&mut screens, &mut run, &call).unwrap();
        }
        assert_eq!(hud.log, ["The Walrus started!", "The Walrus completed!", "Bob won!"]);
    }

    #[test]
    fn trick_names_carry_their_spin() {
        let labels = BTreeMap::from([(7, "Kickflip".to_owned())]);
        assert_eq!(trick_name(&labels, (7, 0)), "KICKFLIP");
        assert_eq!(trick_name(&labels, (7, 2)), "KICKFLIP 360");
        // A peer's 180s (its "S".spins, through SetPatternLastTrick and __abds).
        assert_eq!(trick_name(&labels, (7, i64::MIN)), format!("KICKFLIP {}", u64::MAX));
    }

    #[test]
    fn leaderboard_columns_follow_the_providers() {
        assert_eq!(columns(Mode::BestTrick, 0, 3), ["#", "Gamertag:", "Score:", "Points:"]);
        assert_eq!(columns(Mode::BestTrick, 2, 3), ["#", "Gamertag:", "Points:"]);
        assert_eq!(columns(Mode::Jam, 2, 1), ["#", "Gamertag:", "Score:"]);
        assert_eq!(columns(Mode::DeathRace, 3, 1), ["#", "Gamertag:", "Gates Missed:", "Time:"]);
        assert_eq!(columns(Mode::SpotBattle, 2, 1), ["#", "Gamertag:", "Score:"]);
        assert_eq!(columns(Mode::Skate, 3, 1), ["#", "Gamertag:", "Letters:"]);
        assert_eq!(letters(0), "FLAWLESS");
        assert_eq!(letters(2), "S.K.");
        assert_eq!(fill("{0:s}^YOUR TURN!", "Bob"), "Bob\nYOUR TURN!");
        assert_eq!(fill("{0:s} won", "Bob^Host left"), "Bob\u{2060}^Host left won", "a name breaks no line");
    }

    /// Best Trick on the Library, two players, through the HUD: the overlay, its countdown and messages on
    /// centralhud, the round leaderboards, the final one, then the script's exit. PNGs of each stage go to
    /// SKATE1_ONLINE_SHOTS when set.
    #[test]
    #[ignore = "requires the installed Skate 1 online data and HUD movies (SKATE3_ASSET_ROOT)"]
    fn best_trick_hud_runs_the_match() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let shots = std::env::var("SKATE1_ONLINE_SHOTS").ok().map(PathBuf::from);
        let data = crate::skate1_online::tests::data();
        let row = &data.rows["besttrick_financial_library"];
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 7).unwrap();
        run.start(&data).unwrap();
        let mut hud = Hud::new(&data, &run, &assets).unwrap();
        let mut screens = BTreeMap::new();
        let spots: Vec<String> = row.fields["BestTrickSpotVolumes"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let mut began: BTreeMap<i64, usize> = BTreeMap::new();
        let mut seen = Vec::new();
        let (mut boards, mut had) = (0, false);
        let mut texts = std::collections::BTreeSet::new();
        for f in 0..60 * 60 * 12 {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            if run.session.state() == "event_round" {
                let round = run.session.var("CurrentRound").as_i64().unwrap();
                let k = *began.entry(round).or_insert(f);
                let spot = &spots[round as usize - 1];
                if f == k + 60 {
                    local.push(("triggerentercollision".to_owned(), json!({"PlayerID": 0, "VolumeID": spot})));
                    inbox.push((1, crate::skate1_online::encode("triggerentercollision", &json!({"PlayerID": 1, "VolumeID": spot}), 0)));
                }
                if f == k + 120 {
                    let start = run.session.tick - 20;
                    let seq = |score| json!({"PlayerID": 0, "SequenceScore": score, "StartTick": start, "StartSimGameTick": start, "Wipedout": false});
                    local.push(("sequencecomplete".to_owned(), seq(1000 * round)));
                    inbox.push((1, crate::skate1_online::encode("sequencecomplete", &seq(1500), run.session.tick)));
                }
            }
            let players = run.names.clone();
            run.frame(&data, 1. / 60., local, inbox, &players).unwrap();
            hud.frame(&mut screens, &mut run, 1. / 60.).unwrap();
            for screen in screens.values_mut() {
                screen.advance().unwrap();
            }
            let names: Vec<String> = screens.keys().cloned().collect();
            if seen.last() != Some(&names) {
                seen.push(names.clone());
            }
            let has = screens.contains_key(BOARD);
            boards += usize::from(has && !had);
            had = has;
            if let Some(c) = screens.get(CENTRAL) {
                for path in [MESSAGE_TEXT, COUNTDOWN_TEXT, CONGRATS_TEXT, BOTTOM_TEXT] {
                    if let Some(clip) = c.clip(path) {
                        if let Value::Text(t) = c.vm.get(clip, "text") {
                            texts.insert(t);
                        }
                    }
                }
            }
            if let Some(dir) = &shots {
                let stage = match (run.session.state(), f % 600) {
                    (s, 0) => Some(format!("{f:06}-{s}")),
                    _ => None,
                };
                if let Some(stage) = stage {
                    let draws: Vec<_> = screens.values().flat_map(|s| s.draws().unwrap()).collect();
                    crate::core_menu_runtime::tests::raster_draws(&draws, &assets.join("private/hud-skate1/online"), &dir.join(format!("bt-{stage}.png")));
                }
            }
            if run.complete() {
                break;
            }
        }
        assert!(run.complete(), "stuck in {}", run.session.state());
        assert!(hud.exit, "the final leaderboard never ended");
        assert_eq!(boards, 3, "a leaderboard per round: {seen:?}");
        for text in ["ID_MISSION_ROUND_COMPLETE", "ID_MISSION_CONTEST_COMPLETE", "#3", "#1", "ID_MISSION_COUNTDOWN_GO"] {
            assert!(texts.contains(text), "{text} never shown: {texts:?}");
        }
        assert!(texts.iter().any(|t| t == "ROUND 2!"), "{texts:?}");
    }

    type Events = (Vec<(String, Json)>, Vec<(usize, String)>);

    /// Alice (this player) and Bob on `row` through the HUD at 60 Hz until the match completes (at most `seconds`):
    /// `events(run, f)` gives each frame's events, `look(f, run, hud, screens)` sees each frame once the screens
    /// advanced. PNGs go to SKATE1_ONLINE_SHOTS as `look` asks (`shot`). The match token 8 draws Alice first (8 % 2: the
    /// turn modes' first turn, Skate 3's rule 10-28).
    fn through_hud(row: &str, seconds: f32, mut events: impl FnMut(&Run, usize) -> Events, mut look: impl FnMut(usize, &Run, &Hud, &BTreeMap<String, Screen>)) -> (Run, Hud) {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let data = crate::skate1_online::tests::data();
        let mut run = Run::new(&data, &data.rows[row], vec!["Alice".into(), "Bob".into()], 0, 8).unwrap();
        run.start(&data).unwrap();
        let mut hud = Hud::new(&data, &run, &assets).unwrap();
        let mut screens = BTreeMap::new();
        let players = run.names.clone();
        for f in 0..(seconds * 60.) as usize {
            let (local, inbox) = events(&run, f);
            run.frame(&data, 1. / 60., local, inbox, &players).unwrap();
            hud.frame(&mut screens, &mut run, 1. / 60.).unwrap();
            for screen in screens.values_mut() {
                screen.advance().unwrap();
            }
            look(f, &run, &hud, &screens);
            if run.complete() {
                break;
            }
        }
        (run, hud)
    }

    fn shot(screens: &BTreeMap<String, Screen>, name: &str) {
        let (Ok(assets), Ok(dir)) = (std::env::var("SKATE3_ASSET_ROOT"), std::env::var("SKATE1_ONLINE_SHOTS")) else {
            return;
        };
        let draws: Vec<_> = screens.values().flat_map(|s| s.draws().unwrap()).collect();
        crate::core_menu_runtime::tests::raster_draws(&draws, &Path::new(&assets).join("private/hud-skate1/online"), &Path::new(&dir).join(format!("{name}.png")));
    }

    const LAP: &str = "mcOverlay.raceStatus.lapNumber.labelText";
    const VIEWING: &str = "mcTextLeftJustified.mcTextName.labelText";

    fn gate_events(f: usize, at: usize, id: usize, events: Vec<(&str, Json)>, tick: i64, local: &mut Vec<(String, Json)>, inbox: &mut Vec<(usize, String)>) {
        if f != at {
            return;
        }
        for (event, mut message) in events {
            message["PlayerID"] = json!(id);
            match id {
                0 => local.push((event.to_owned(), message)),
                _ => inbox.push((id, crate::skate1_online::encode(event, &message, tick))),
            }
        }
    }

    /// Death Race on the Old Town spillway (6 gates, 6 s a missed gate, out past 2), through the HUD: Alice skips
    /// gate 3 and crosses first at 7 s (13 s with the penalty), Bob takes every gate and crosses at 11 s. The lap
    /// field and the missed-gate message, the spectator camera and overlay on Bob 3 s after Alice's finish, Bob the
    /// winner on adjusted time, the leaderboard, complete.
    #[test]
    #[ignore = "requires the installed Skate 1 online data and HUD movies (SKATE3_ASSET_ROOT)"]
    fn death_race_hud_runs_the_match() {
        let data = crate::skate1_online::tests::data();
        let row = &data.rows["ol_deathrace_oldtown_spillway"];
        let gates: Vec<String> = row.fields["CheckPoints"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let finish = row.fields["FinishLine"].as_str().unwrap().to_owned();
        let volume = |g: usize| if g > gates.len() { finish.clone() } else { gates[g - 1].clone() };
        let mut began = None;
        let (mut texts, mut cameras, mut nexts) = (std::collections::BTreeSet::new(), Vec::new(), Vec::new());
        let mut shots = Vec::new();
        let (run, hud) = through_hud(
            "ol_deathrace_oldtown_spillway",
            150.,
            |run, f| {
                let (mut local, mut inbox) = (Vec::new(), Vec::new());
                if began.is_none() && run.session.state() == "skating" {
                    began = Some(f);
                }
                if let Some(k) = began {
                    let enter = |g: usize| vec![("triggerentercollision", json!({"VolumeID": volume(g)}))];
                    for g in [1, 2, 4, 5, 6, 7] {
                        gate_events(f, k + 60 * g, 0, enter(g), run.session.tick, &mut local, &mut inbox);
                    }
                    for g in 1..=6 {
                        gate_events(f, k + 60 * g + 30, 1, enter(g), run.session.tick, &mut local, &mut inbox);
                    }
                    gate_events(f, k + 660, 1, enter(7), run.session.tick, &mut local, &mut inbox);
                }
                (local, inbox)
            },
            |f, run, _, screens| {
                if cameras.last() != Some(&run.camera) {
                    cameras.push(run.camera);
                }
                if nexts.last() != Some(&run.next_gate) {
                    nexts.push(run.next_gate);
                }
                for (name, paths) in [(RACE, &[LAP][..]), (SPECTATOR, &[VIEWING][..]), (CENTRAL, &[MESSAGE_TEXT, CONGRATS_TEXT][..])] {
                    if let Some(s) = screens.get(name) {
                        texts_of(s, paths, &mut texts);
                    }
                }
                let state = run.session.state().to_owned();
                if shots.last().is_none_or(|(s, at): &(String, usize)| *s != state || f == at + 150) {
                    shots.push((state.clone(), f));
                    shot(screens, &format!("dr-{f:06}-{state}"));
                }
            },
        );
        assert!(run.complete(), "stuck in {}", run.session.state());
        assert!(hud.exit, "the final leaderboard never ended");
        let s = &run.standings;
        assert_eq!((s[0].missed, s[0].penalty, s[0].finished, s[1].missed, s[1].finished), (1, 6., true, 0, true), "{s:?}");
        assert!((s[0].time - 13.).abs() < 0.1 && (s[1].time - 11.).abs() < 0.1, "{s:?}");
        assert!((run.result() - 13_000).abs() < 100, "{}", run.result());
        // Gate 1 next, then 3 (Alice skips it: 5 after gate 4), the finish line, then done.
        assert_eq!(nexts, [None, Some(1), Some(2), Some(3), Some(5), Some(6), Some(7), None]);
        // Alice watches Bob from 3 s after she crossed until the match ends.
        assert_eq!(cameras, [None, Some(1)]);
        // The lap field (modes.json), the overlay's missed-gate message (centralhud's capitals), spectator_mode's name.
        for text in ["#Gates Missed: 0/3", "#Gates Missed: 1/3", "#Bob", "GATES MISSED: 1", "ID_RACE_FINISHED"] {
            assert!(texts.contains(text), "{text} never shown: {texts:?}");
        }
        assert!(hud.log.iter().any(|l| l == "Bob won."), "{:?}", hud.log);
    }

    /// Spot Race on the Old Town warehouse (11 spots, each its trick or points, none skippable), through the HUD:
    /// Alice lands every spot (short of the points twice first) and crosses; Bob tries spot 2 before spot 1, which
    /// doesn't count. The objective line per spot, the lap field, Alice watching Bob until the hurry-up runs out, Alice
    /// the winner, the leaderboard, complete.
    #[test]
    #[ignore = "requires the installed Skate 1 online data and HUD movies (SKATE3_ASSET_ROOT)"]
    fn spot_race_hud_runs_the_match() {
        let data = crate::skate1_online::tests::data();
        let row = &data.rows["spotrace_oldtown_warehouse"];
        let list = |k: &str| row.fields[k].as_array().unwrap().clone();
        let (gates, types, points) = (list("CheckPoints"), list("SpotTrickTypes"), list("SpotSeqRequiredPoints"));
        let finish = row.fields["FinishLine"].clone();
        let kind = |g: usize| data.enums["Trick"]["eTrickType"][types[g - 1].as_str().unwrap()].as_i64().unwrap();
        let (grab, manual) = (data.enums["Trick"]["eTrickType"]["Grab"].as_i64().unwrap(), data.enums["Trick"]["eTrickType"]["Manual"].as_i64().unwrap());
        let seq = |score: i64| ("sequencecomplete", json!({"SequenceScore": score, "StartTick": 0, "StartSimGameTick": 0, "Wipedout": false}));
        // A spot's trick: any trick on a points spot, else its type (a grab in the air, a manual through the volume).
        let trick = |g: usize| match (points[g - 1].as_i64().unwrap(), kind(g)) {
            (1.., _) => ("trickcomplete", json!({"TrickType": kind(g)})),
            (_, t) if t == grab => ("aircompleted", json!({"FirstScorableId": 0, "Wipedout": 0, "S": {"grab": true, "air": true}})),
            (_, t) if t == manual => ("triggerexitcollision", json!({"VolumeID": gates[g - 1], "S": {"types": {manual.to_string(): true}}})),
            (_, t) => ("trickcomplete", json!({"TrickType": t})),
        };
        let mut began = None;
        let (mut texts, mut nexts) = (std::collections::BTreeSet::new(), Vec::new());
        let mut shots = Vec::new();
        let (run, hud) = through_hud(
            "spotrace_oldtown_warehouse",
            200.,
            |run, f| {
                let (mut local, mut inbox) = (Vec::new(), Vec::new());
                if began.is_none() && run.session.state() == "skating" {
                    began = Some(f);
                }
                let Some(k) = began else {
                    return (local, inbox);
                };
                let tick = run.session.tick;
                for g in 1..=gates.len() {
                    let at = k + 120 * g;
                    let need = points[g - 1].as_i64().unwrap();
                    gate_events(f, at, 0, vec![("triggerentercollision", json!({"VolumeID": gates[g - 1]}))], tick, &mut local, &mut inbox);
                    gate_events(f, at + 10, 0, vec![trick(g)], tick, &mut local, &mut inbox);
                    if [1, 4].contains(&g) {
                        // Short of the points: the spot's trick is forgotten, so another trick before the rest.
                        gate_events(f, at + 20, 0, vec![seq(need / 2)], tick, &mut local, &mut inbox);
                        gate_events(f, at + 80, 0, vec![trick(g), seq(need - need / 2)], tick, &mut local, &mut inbox);
                    } else {
                        gate_events(f, at + 20, 0, vec![seq(need.max(100))], tick, &mut local, &mut inbox);
                    }
                }
                gate_events(f, k + 120 * (gates.len() + 1), 0, vec![("triggerentercollision", json!({"VolumeID": finish}))], tick, &mut local, &mut inbox);
                // Bob goes for spot 2 first.
                let bob = vec![("triggerentercollision", json!({"VolumeID": gates[1]})), trick(2), seq(100)];
                gate_events(f, k + 60, 1, bob, tick, &mut local, &mut inbox);
                (local, inbox)
            },
            |f, run, _, screens| {
                if nexts.last() != Some(&run.next_gate) {
                    nexts.push(run.next_gate);
                }
                for (name, paths) in [(RACE, &[LAP][..]), (SPECTATOR, &[VIEWING][..]), (CENTRAL, &[MESSAGE_TEXT, CONGRATS_TEXT, BOTTOM_TEXT][..])] {
                    if let Some(s) = screens.get(name) {
                        texts_of(s, paths, &mut texts);
                    }
                }
                let state = run.session.state().to_owned();
                if shots.last().is_none_or(|(s, at): &(String, usize)| *s != state || f == at + 150) {
                    shots.push((state.clone(), f));
                    shot(screens, &format!("sr-{f:06}-{state}"));
                }
            },
        );
        assert!(run.complete(), "stuck in {}", run.session.state());
        assert!(hud.exit, "the final leaderboard never ended");
        let s = &run.standings;
        assert!(s[0].finished && !s[0].out && s[0].missed == 0, "{s:?}");
        assert!(!s[1].finished && s[1].time == 0., "Bob's spot 2 counted: {s:?}");
        assert_eq!(hud.next.get(&1), Some(&1));
        let expected: Vec<Option<usize>> = std::iter::once(None).chain((1..=12).map(Some)).chain([None]).collect();
        assert_eq!(nexts, expected);
        let shown = |t: &str| texts.iter().any(|x| x.trim_start_matches('#') == t);
        for text in [
            "Get 25 Points",
            "Do 1 Manual",
            "Do 1 Flip Trick",
            "Get 75 Points",
            "Do 1 Grab",
            "Get 125 Points",
            "Do 1 Grind",
            "Cross the finish line!",
            "Spots 1/12",
            "Spots 12/12",
            "Bob",
        ] {
            assert!(shown(text), "{text} never shown: {texts:?}");
        }
        assert!(hud.log.iter().any(|l| l == "Alice won."), "{:?}", hud.log);
    }

    fn sequence(score: i64, tick: i64) -> (&'static str, Json) {
        ("sequencecomplete", json!({"SequenceScore": score, "StartTick": tick - 20, "StartSimGameTick": tick - 20, "Wipedout": false}))
    }

    /// Jam on the Library through the HUD: the flythrough, the countdown, the 180 s round with both in the region
    /// (Alice 1600, Bob 1000); one leaderboard, its countdown ending the match.
    #[test]
    #[ignore = "requires the installed Skate 1 online data and HUD movies (SKATE3_ASSET_ROOT)"]
    fn jam_hud_runs_the_match() {
        let data = crate::skate1_online::tests::data();
        let region = data.rows["jam_financial_library"].fields["BestTrickRegions"][0].clone();
        let mut began = None;
        let (mut texts, mut boards, mut had, mut shots) = (std::collections::BTreeSet::new(), 0, false, Vec::new());
        let (run, hud) = through_hud(
            "jam_financial_library",
            360.,
            |run, f| {
                let (mut local, mut inbox) = (Vec::new(), Vec::new());
                if run.session.state() == "event_round" {
                    let k = *began.get_or_insert(f);
                    let tick = run.session.tick;
                    for (id, score) in [(0, 800), (1, 500)] {
                        gate_events(f, k + 60, id, vec![("triggerentercollision", json!({"VolumeID": region}))], tick, &mut local, &mut inbox);
                        for at in [k + 120, k + 240] {
                            gate_events(f, at, id, vec![sequence(score, tick)], tick, &mut local, &mut inbox);
                        }
                    }
                }
                (local, inbox)
            },
            |f, run, _, screens| {
                let has = screens.contains_key(BOARD);
                boards += usize::from(has && !had);
                had = has;
                if let Some(c) = screens.get(CENTRAL) {
                    texts_of(c, &[MESSAGE_TEXT, COUNTDOWN_TEXT, CONGRATS_TEXT, BOTTOM_TEXT], &mut texts);
                }
                let state = run.session.state().to_owned();
                if shots.last().is_none_or(|(s, at): &(String, usize)| *s != state || f == at + 600) {
                    shots.push((state.clone(), f));
                    shot(screens, &format!("jam-{f:06}-{state}"));
                }
            },
        );
        assert!(run.complete(), "stuck in {}", run.session.state());
        assert!(hud.exit, "the final leaderboard never ended");
        assert_eq!(boards, 1);
        let scores: Vec<(i64, i64)> = run.standings.iter().map(|s| (s.score, s.rank)).collect();
        assert_eq!(scores, [(1600, 1), (1000, 2)], "{:?}", run.standings);
        for text in ["ID_MISSION_COUNTDOWN_GO", "#3", "#1", "ID_MISSION_CONTEST_COMPLETE"] {
            assert!(texts.contains(text), "{text} never shown: {texts:?}");
        }
        assert!(hud.log.iter().any(|l| l == "Alice won."), "{:?}", hud.log);
    }

    /// Own The Spot on fp_u1_ownspot01 through the HUD: three rounds of turns in slot order (the turn message and
    /// "Go!" countdown for each, the camera on the skater up), Alice 1000 and Bob 1500; Bob owns the spot; the
    /// leaderboard, its countdown ending the match.
    #[test]
    #[ignore = "requires the installed Skate 1 online data and HUD movies (SKATE3_ASSET_ROOT)"]
    fn spot_battle_hud_runs_the_match() {
        let data = crate::skate1_online::tests::data();
        let spots = data.rows["ol_fp_u1_ownspot01"].fields["OTSSpotVolumes"].as_array().unwrap().clone();
        let mut turn = None;
        let (mut texts, mut cameras, mut shots) = (std::collections::BTreeSet::new(), Vec::new(), Vec::new());
        let (run, hud) = through_hud(
            "ol_fp_u1_ownspot01",
            240.,
            |run, f| {
                let (mut local, mut inbox) = (Vec::new(), Vec::new());
                if run.session.state() == "entered_trick_boundary" {
                    let active = run.session.var("ActiveSkaterIdx").as_i64().unwrap() as usize - 1;
                    let round = run.session.var("CurrentRound").as_i64().unwrap();
                    let k = match turn {
                        Some((r, a, k)) if (r, a) == (round, active) => k,
                        _ => turn.insert((round, active, f)).2,
                    };
                    let tick = run.session.tick;
                    let enter = spots.iter().map(|s| ("triggerentercollision", json!({"VolumeID": s}))).collect();
                    gate_events(f, k + 30, active, enter, tick, &mut local, &mut inbox);
                    gate_events(f, k + 90, active, vec![sequence(if active == 0 { 1000 } else { 1500 }, tick)], tick, &mut local, &mut inbox);
                }
                (local, inbox)
            },
            |f, run, _, screens| {
                if cameras.last() != Some(&run.camera) {
                    cameras.push(run.camera);
                }
                if let Some(c) = screens.get(CENTRAL) {
                    texts_of(c, &[MESSAGE_TEXT, COUNTDOWN_TEXT, CONGRATS_TEXT], &mut texts);
                }
                if let Some(r) = screens.get(RACE) {
                    texts_of(r, &[LAP], &mut texts);
                }
                let state = run.session.state().to_owned();
                if shots.last().is_none_or(|(s, _): &(String, usize)| *s != state) {
                    shots.push((state.clone(), f));
                    shot(screens, &format!("ots-{f:06}-{state}"));
                }
            },
        );
        assert!(run.complete(), "stuck in {}", run.session.state());
        assert!(hud.exit, "the final leaderboard never ended");
        let scores: Vec<i64> = run.standings.iter().map(|s| s.score).collect();
        assert_eq!(scores, [1000, 1500], "{:?}", run.standings);
        // The camera goes to Bob on his turns and back to Alice on hers.
        assert!(cameras.starts_with(&[None, Some(1), None, Some(1), None, Some(1)]), "{cameras:?}");
        // onlinehud's lap field: the round over the row's NumRounds (3).
        for text in ["Alice\nYOUR TURN!", "Bob\nYOUR TURN!", "#Round 1/3", "#Round 3/3"] {
            assert!(texts.contains(text), "{text} never shown: {texts:?}");
        }
        assert!(texts.iter().any(|t| t.starts_with("ID_SPOT_MISSION_")), "no turn countdown: {texts:?}");
        assert!(hud.log.iter().any(|l| l == "Bob won."), "{:?}", hud.log);
    }

    fn texts_of(screen: &Screen, paths: &[&str], into: &mut std::collections::BTreeSet<String>) {
        for path in paths {
            if let Some(Value::Text(t)) = screen.clip(path).map(|c| screen.vm.get(c, "text")) {
                into.insert(t);
            }
        }
    }

    /// S.K.A.T.E. on the Matrix, two players, through the HUD: Alice sets a new flip each round, Bob copies with the
    /// wrong trick and spells S.K.A.T.E.: skatehud's names and letters, skatetrick's set / copy lines and its
    /// "already been done" list, centralhud's messages, the leaderboard, the script's exit. PNGs on each state to
    /// SKATE1_ONLINE_SHOTS when set.
    #[test]
    #[ignore = "requires the installed Skate 1 online data and HUD movies (SKATE3_ASSET_ROOT)"]
    fn skate_hud_runs_the_match() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let shots = std::env::var("SKATE1_ONLINE_SHOTS").ok().map(PathBuf::from);
        let data = crate::skate1_online::tests::data();
        let row = &data.rows["skate_financial_matrix"];
        // Token 8: Alice leads first (8 % 2, Skate 3's random first leader).
        let mut run = Run::new(&data, row, vec!["Alice".into(), "Bob".into()], 0, 8).unwrap();
        run.start(&data).unwrap();
        let mut hud = Hud::new(&data, &run, &assets).unwrap();
        let mut screens = BTreeMap::new();
        let spot = row.fields["SpotVolumes"][0].as_str().unwrap().to_owned();
        let set = ["Kickflip", "Heelflip", "Hardflip", "Varial Flip", "Impossible"];
        let (mut entered, mut rounds, mut shoot) = (None::<(String, usize)>, 0, None);
        let (mut texts, mut letters) = (std::collections::BTreeSet::new(), 0);
        let mut sounds: Vec<String> = Vec::new();
        for f in 0..60 * 60 * 10 {
            let (mut local, mut inbox) = (Vec::new(), Vec::new());
            let state = run.session.state().to_owned();
            if entered.as_ref().is_none_or(|(s, _)| *s != state) {
                entered = Some((state.clone(), f));
                rounds += usize::from(state == "wait_to_enter_spot_volume");
                shoot = Some((f + 20, state.clone()));
            }
            let k = entered.as_ref().unwrap().1;
            let air = |id: usize, trick: i64, label: &str| {
                json!({"PlayerID": id, "FirstScorableId": trick, "Wipedout": 0, "S": {"first": trick, "first_type": 2, "spins": 0, "label": label}})
            };
            match state.as_str() {
                "wait_to_enter_spot_volume" if f == k + 90 => local.push(("triggerentercollision".to_owned(), json!({"PlayerID": 0, "VolumeID": spot}))),
                "leader_attempt" if f == k + 30 => local.push(("aircompleted".to_owned(), air(0, 100 + rounds as i64, set[(rounds - 1) % 5]))),
                "wait_to_enter_spot2" if f == k + 30 => {
                    inbox.push((1, crate::skate1_online::encode("triggerentercollision", &json!({"PlayerID": 1, "VolumeID": spot}), 0)))
                }
                "follower_attempt" if f == k + 30 => inbox.push((1, crate::skate1_online::encode("aircompleted", &air(1, 999, "Ollie"), 0))),
                _ => {}
            }
            let players = run.names.clone();
            run.frame(&data, 1. / 60., local, inbox, &players).unwrap();
            hud.frame(&mut screens, &mut run, 1. / 60.).unwrap();
            for screen in screens.values_mut() {
                screen.advance().unwrap();
                sounds.append(&mut screen.bindings.sounds);
            }
            if let Some(t) = screens.get(TRICKS) {
                let paths = ["mcPhase.labelText", "mcTrick_set.mcTrick.labelText", "mcTrick_copy.mcTrick.labelText", "mcABD.mcTrickName0.mcAbdtext.labelText"];
                texts_of(t, &paths, &mut texts);
            }
            if let Some(p) = screens.get(PLAYERS) {
                let names = ["mcPlayer1.mcNameStates.mcTextName.labelText", "mcPlayer2.mcNameStates.mcTextName.labelText"];
                texts_of(p, &names, &mut texts);
                let shown = (LETTERS.iter())
                    .filter(|(clip, _)| p.clip(&format!("mcLG_SkateAnimation.{clip}.mcLetter")).is_some_and(|c| matches!(p.vm.get(c, "_visible"), Value::Bool(true))))
                    .count();
                letters = letters.max(shown);
            }
            if let Some(c) = screens.get(CENTRAL) {
                texts_of(c, &[MESSAGE_TEXT, CONGRATS_TEXT], &mut texts);
            }
            if let (Some(dir), Some((at, name))) = (&shots, &shoot)
                && *at == f
            {
                let draws: Vec<_> = screens.values().flat_map(|s| s.draws().unwrap()).collect();
                crate::core_menu_runtime::tests::raster_draws(&draws, &assets.join("private/hud-skate1/online"), &dir.join(format!("sk-{f:06}-{name}.png")));
            }
            if run.complete() {
                break;
            }
        }
        assert!(run.complete(), "stuck in {}", run.session.state());
        assert!(hud.exit, "the final leaderboard never ended");
        assert_eq!(rounds, 5);
        assert_eq!(letters, 5, "S.K.A.T.E. never spelled");
        for text in [
            "ID_SKATE_SET_TRICK",
            "ID_SKATE_COPY_THE_TRICK",
            "#KICKFLIP",
            "#IMPOSSIBLE",
            "#OLLIE",
            "#\u{1}Alice",
            "#\u{1}Bob",
            "Alice IS UP\n\nSET THE TRICK!",
            "Bob IS UP\n\nCOPY THE TRICK!",
            "ID_SKATE_TRICK_SET",
            "ID_SKATE_WRONG_TRICK",
            "ID_MISSION_CHALLENGE_COMPLETE",
        ] {
            assert!(texts.contains(text), "{text} never shown: {texts:?}");
        }
        // Audit 10-28 GAP-2: the overlay's UpdateLetters asks for the letter and the elimination sounds.
        for sound in ["PlayGetLetter", "PlayPlayerEliminated"] {
            assert!(sounds.iter().any(|s| s == sound), "{sound} never asked for: {sounds:?}");
        }
    }

    /// Audit 10-28 GAP-2: every Audio.Play* native the Skate 1 online movies call that has a sound in retail (a 1000 /
    /// 2000 FEMessageToSoundTrigger record) has its wav where apt_screen::render looks for it (the movie's own
    /// export's sfx); PlaySpotFound plays spot_found (core_menu_runtime::played_sound). Silent in retail, so none:
    /// PlayGenCentralMsge, PlayDeathRaceGateMissedSound, PlayKnockout, PlayReposition, PlayOptionSound,
    /// PlayHUDOwnSpotAnimSound, PlayBestTrick, PlayJam. Needs online-skate1 re-run after 10-28 (old_online1_modes.py).
    #[test]
    #[ignore = "requires the installed Skate 1 online HUD (SKATE3_ASSET_ROOT)"]
    fn every_online_movie_sound_has_its_wav() {
        const SOUNDS: [&str; 14] = [
            "PlayABDOverlayIn",
            "PlayABDOverlayOut",
            "PlayAcceptSound",
            "PlayBackSound",
            "PlayDeathRaceGateCompleteSound",
            "PlayDownSound",
            "PlayFirstPlaceIn",
            "PlayGetLetter",
            "PlayLeaderboardIn",
            "PlayLeaderboardOut",
            "PlayLeaderboardShuffle",
            "PlayPlayerEliminated",
            "PlayUpSound",
            "spot_found",
        ];
        let sfx = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join("private/hud-skate1/online/sfx");
        let missing: Vec<_> = SOUNDS.iter().filter(|s| !sfx.join(format!("{s}.wav")).is_file()).collect();
        assert!(missing.is_empty(), "no wav in {} for {missing:?}", sfx.display());
    }
}
