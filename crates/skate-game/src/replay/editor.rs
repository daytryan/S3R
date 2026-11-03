//! Retail Skate 3's Replay Editor (the Skate.Create `replay_editor3` version) as a pure model: its states, the
//! normalised timeline with in/out marks and clip markers, the seven channels and their keys, the X menu and popups.
//! hud.rs drives the retail movie from it; nothing here touches Bevy's world.
//!
//! Evidence: `.superpowers/sdd/2026-10-04-replay-editor/research.md` (Q1-Q3, Q8) and `build-progress.md` (Stage 0).
use bevy::math::Vec3;

/// Not retail, by request (user 10-27: "there should be no limit for keyframes"): retail's 0x82613500 allowed 100 clip
/// markers and 100 keys per channel; the editor has no limit. A save is untrusted, so a load still caps each list at
/// one per recorded frame (record::RING_FRAMES), past what any timeline holds.
pub(crate) const MAX_LOADED: usize = super::record::RING_FRAMES;
/// 0x82610734 seeks the opening Instant Replay to 0.98 ([0x82099800]), inside the edit window (1f): at 1.0 the sticks'
/// orbit and the timeline's D-pad did nothing until a scrub back.
pub(crate) const OPEN_PLAYHEAD: f32 = 0.98;
/// research Q2 [disc, R2]: 0x825084c8 / [0x822428ec], the hit and proximity radius on the normalised timeline.
pub(crate) const MARKER_RADIUS: f32 = 0.0089;
/// Audit 3b (user 10-27: retail's): the in and out marks stay a tenth of the recording apart (0x82509DA0 in <= out -
/// 0.1, 0x82509EB0 out >= in + 0.1; [0x820641A8] = 0.1): 9 s of a full 90 s recording.
pub(crate) const MIN_CLIP: f32 = 0.1;
/// research Q8 [disc, R2]: IsEncodedLengthValid 0x825adf88 is `<= 60` for everyone; U2.
pub(crate) const MAX_ENCODED_SECONDS: u32 = 60;
/// research Q3 [disc]: 0x82610f18 at 0x8261136c, per 60 Hz update.
/// One retail editor update (60 Hz): the HUD runs [`Editor::scrub`] / [`Editor::play`] once per update.
pub(crate) const UPDATE: f32 = 1. / 60.;
const SCRUB_GAIN: f32 = 0.005;
const SCRUB_EASE: f32 = 0.25;
const SCRUB_TARGET_SETTLE: f32 = 1e-5;
const SCRUB_VELOCITY_SETTLE: f32 = 1e-4;
/// research Q2 [disc]: lens 24..75 mm in 0.5 mm steps (0x82c80b48), reset 24 (0x825087b0).
pub(crate) const LENS_MIN: f32 = 24.;
pub(crate) const LENS_MAX: f32 = 75.;
pub(crate) const LENS_STEP: f32 = 0.5;
/// research Q2/Q3 [disc]: height ±0.01 per update (editor+112, 0x82610c08).
pub(crate) const HEIGHT_STEP: f32 = 0.01;
/// research Q2 [disc, R2]: a cut key adds a second ICE key at t + 0.02 ([0x821e9580]).
pub(crate) const CUT_HOLD: f32 = 0.02;
/// Item 16 extension (not retail): roll per 60 Hz update while LB / RB is held in a camera edit, degrees.
pub(crate) const ROLL_STEP: f32 = 1.;
/// Item 16 extension: Replay Settings' letterbox values (Off, 2.39:1, 1.85:1, 4:3) as the picture's aspect.
pub(crate) const LETTERBOX: [f32; 4] = [0., 2.39, 1.85, 4. / 3.];
const LETTERBOX_TEXT: [&str; 4] = ["Off", "2.39:1", "1.85:1", "4:3"];
/// Item 16 extension: midpoint steps per eased speed segment (encoded length and the export's frame walk).
const RAMP_STEPS: usize = 256;

/// The FE sounds the exe plays on the editor's keys (the movie plays none); each is cited at its key in
/// `.superpowers/sdd/2026-10-11-bug-list/replay-progress.md` (TU3 RE pass 2).
pub(crate) mod sound {
    pub const SELECT: u64 = 0x373D_38D7_EA16_7A71;
    pub const BACK: u64 = 0x4D92_7CD6_4CC8_B818;
    pub const QUIT: u64 = 0x8EEC_5B3D_7392_0FA7;
    pub const MENU: u64 = 0xE28D_B5D6_5B9C_9313;
    pub const HELP: u64 = 0x648A_6811_D713_694E;
    pub const CHANNEL: u64 = 0x695F_84D6_C23E_714F;
    pub const PREVIOUS: u64 = 0x234F_CD1A_D7C5_D294;
    pub const NEXT: u64 = 0x9396_2981_6845_CADE;
    pub const REMOVE: u64 = 0x9DEF_3BCC_EF92_2B96;
    pub const ADD: u64 = 0xEE45_AA44_0CEB_D1E8;
    pub const ERROR: u64 = 0x47F7_2022_2F2C_AED4;
    pub const EDIT: u64 = 0x29C0_CC3C_EA31_4C92;
    pub const KEY: u64 = 0xA1E6_D757_DFFA_A0FE;
    pub const HUD: u64 = 0xF2B1_4282_F615_71A2;
    pub const CREATE: u64 = 0x1747_2B6D_55E3_6C95;
    pub const UP: u64 = 0x73D7_CC18_FE21_DBE9;
    pub const DOWN: u64 = 0x7F97_5D2B_6425_9AC2;
    pub const DEC: u64 = 0xAD8B_F9A0_B499_7B49;
    pub const INC: u64 = 0xD8EE_38A6_1C45_EA28;
}

/// Item 16 extension: how a key's value moves to the next key in its clip. Retail keys hold (non-camera channels,
/// build-progress Task 4) or run Catmull-Rom (Adv* camera keys, Stage 0 §8): [`Ease::retail`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Ease {
    #[default]
    Hold,
    Linear,
    Smooth,
}
impl Ease {
    pub const ALL: [Ease; 3] = [Self::Hold, Self::Linear, Self::Smooth];
    pub fn retail(channel: Channel) -> Self {
        if channel == Channel::Camera { Self::Smooth } else { Self::Hold }
    }
    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % 3]
    }
    pub fn name(self) -> &'static str {
        ["Hold", "Linear", "Smooth"][self as usize]
    }
}

/// Catmull-Rom (Hermite, tangent 1) from p1 to p2.
pub(crate) fn catmull(p0: f32, p1: f32, p2: f32, p3: f32, u: f32) -> f32 {
    let (u2, u3) = (u * u, u * u * u);
    0.5 * ((2. * p1) + (-p0 + p2) * u + (2. * p0 - 5. * p1 + 4. * p2 - p3) * u2 + (-p0 + 3. * p1 - 3. * p2 + p3) * u3)
}
/// `degrees` in -180..180.
pub(crate) fn wrap_degrees(degrees: f32) -> f32 {
    (degrees + 180.).rem_euclid(360.) - 180.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CameraKind {
    Follow,
    Tripod,
    Game,
    Free,
    AdvFollow,
    AdvTripod,
    AdvFree,
}
impl CameraKind {
    /// RE3's `mCameraList` order (kEditFollowCamera 0 .. kEditAdvancedFreeCamera 6).
    pub const LIST: [CameraKind; 7] = [
        Self::Follow,
        Self::Tripod,
        Self::Game,
        Self::Free,
        Self::AdvFollow,
        Self::AdvTripod,
        Self::AdvFree,
    ];
    pub fn index(self) -> usize {
        Self::LIST.iter().position(|&k| k == self).unwrap_or(2)
    }
    /// The movie's edit state for this camera (kFollowEditMode 1, kTripodEditMode 2, kDefaultCameraMode 10,
    /// kFreeCameraMode 7, kAdvancedFollowEditMode 12, kAdvancedTripodEditMode 13, kAdvancedFreeCameraEditMode 14).
    pub fn edit_state(self) -> u32 {
        match self {
            Self::Follow => 1,
            Self::Tripod => 2,
            Self::Game => 10,
            Self::Free => 7,
            Self::AdvFollow => 12,
            Self::AdvTripod => 13,
            Self::AdvFree => 14,
        }
    }
    /// Adv* cameras key several shots inside a clip (research Q6).
    pub fn advanced(self) -> bool {
        matches!(self, Self::AdvFollow | Self::AdvTripod | Self::AdvFree)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    Camera,
    Speed,
    Shake,
    Brightness,
    Contrast,
    Saturation,
    Tint,
}
impl Channel {
    /// GetMode / ed+620 order (research Q1, EXE 0x825ae3a0).
    pub const ALL: [Channel; 7] =
        [Self::Camera, Self::Speed, Self::Shake, Self::Brightness, Self::Contrast, Self::Saturation, Self::Tint];
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn text(self) -> &'static str {
        [
            "ID_REPLAYEDITOR_CHANNEL_CAMERA",
            "ID_REPLAYEDITOR_CHANNEL_SPEED",
            "ID_REPLAYEDITOR_CHANNEL_SHAKE",
            "ID_REPLAYEDITOR_CHANNEL_BRIGHTNESS",
            "ID_REPLAYEDITOR_CHANNEL_CONTRAST",
            "ID_REPLAYEDITOR_CHANNEL_SATURATE",
            "ID_REPLAYEDITOR_CHANNEL_TINT",
        ][self.index()]
    }
    /// (default, step, min, max); research Q2 (DLC inc/dec 0x8250ab28 / 0x8250adf8) and Stage 0 §6 (B 0, C 10, S 0).
    fn range(self) -> (i32, i32, i32, i32) {
        match self {
            Self::Camera => (0, 1, 0, 6),
            Self::Speed => (100, 10, 10, 100),
            Self::Shake => (0, 1, 0, 3),
            Self::Brightness => (0, 20, -100, 100),
            Self::Contrast => (10, 2, 10, 30),
            Self::Saturation => (0, 10, 0, 100),
            Self::Tint => (0, 25, 0, 250),
        }
    }
    pub fn default_value(self) -> i32 {
        self.range().0
    }
}

/// One camera shot: what an editor camera key holds (research Q6: eye, look, type, lens) plus the orbit the
/// Follow/Tripod/Free edit drives (editor+80/+84/+88/+112).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shot {
    pub kind: CameraKind,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub height: f32,
    pub lens: f32,
    pub eye: Vec3,
    pub look: Vec3,
    /// Item 16 extension: the Dutch angle, degrees -180..180 (0 = retail).
    pub roll: f32,
}
impl Shot {
    pub fn new(kind: CameraKind) -> Self {
        Self { kind, yaw: 0., pitch: 0., distance: 0., height: 0., lens: LENS_MIN, eye: Vec3::ZERO, look: Vec3::ZERO, roll: 0. }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Int(i32),
    Tint([i32; 4]),
    Camera(Shot),
}

/// A channel key. Hard keys sit on clip starts (0 or a clip marker) and set the clip's value ("Apply" with no key
/// markers in the clip); soft keys are the key markers inside a clip ("Create Key").
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Key {
    pub t: f32,
    pub soft: bool,
    /// Adv* camera keys: a cut holds the previous shot (research Q2, +0.02).
    pub cut: bool,
    pub value: Value,
    /// Item 16 extension: the move to the next key in the clip ([`Ease::retail`] by default).
    pub ease: Ease,
}

/// Retail's editor popups. ID_REPLAY_EDITOR_POPUP_CANCEL / _PROXIMITY are not in tu3.pe: retail has no Cancel or
/// Proximity popup (a camera edit has no B; a marker too close is the error sound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PopupKind {
    /// D-pad Down in a clip with key markers (0x826132a8): ID_REPLAY_EDITOR_POPUP_CLIPMARKER, Yes inserts.
    ClipMarker,
    DeleteClip,
    DeleteKey,
    ReplaceCamera,
    Quit,
    ResetTimeline,
    ResetChannel,
    SaveReplay,
    SavePhoto,
    Upload,
}
impl PopupKind {
    /// (body, choices); Yes/No popups answer index 1 = Yes.
    pub fn text(self) -> (&'static str, &'static [&'static str]) {
        const NO_YES: &[&str] = &["ID_COMMON_NO", "ID_COMMON_YES"];
        match self {
            Self::ClipMarker => ("ID_REPLAY_EDITOR_POPUP_CLIPMARKER", NO_YES),
            Self::DeleteClip => ("ID_REPLAY_EDITOR_DELETE_CLIP_DESC", NO_YES),
            Self::DeleteKey => ("ID_REPLAY_EDITOR_DELETE_KEY_DESC", NO_YES),
            Self::ReplaceCamera => ("ID_REPLAY_EDITOR_REPLACE_CAMERA_DESC", NO_YES),
            Self::Quit => ("ID_REPLAY_EDITOR_QUIT", NO_YES),
            Self::ResetTimeline => ("ID_REPLAY_EDITOR_RESET_TIMELINE_CONFRIM", NO_YES),
            Self::ResetChannel => ("ID_REPLAY_EDITOR_RESET_TIMELINE_CURRENT_CHANNEL_CONFIRM", NO_YES),
            Self::SaveReplay => ("ID_REPLAY_EDITOR_POPUP_REPLAYSAVE", NO_YES),
            Self::SavePhoto => ("ID_REPLAY_EDITOR_POPUP_PHOTOSAVE", NO_YES),
            Self::Upload => ("ID_REPLAY_EDITOR_POPUP_UPLOAD", NO_YES),
        }
    }
}

/// Not retail, by request (user 10-27: "upload video should say render video"): the X menu's Upload row (retail
/// ID_REPLAYEDITOR_UPLOAD, "Upload Video") and the export screen's start row.
pub(crate) const RENDER_VIDEO: &str = "#Render Video";

/// The X menu (review I8, traced in TU3; build-progress §Fix round): button handler 0x82612c28 / 0x82613988 opens
/// builder 0x82614f38 (title ID_REPLAYEDITOR_TITLE), whose rows and ids are, in order: Upload 1, Save Replay 2,
/// Take Photo 3, Replay Settings 4, Reset Current Channel 5, Reset Timeline 6; handler 0x82616270 acts on the id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuItem {
    Upload,
    SaveReplay,
    TakePhoto,
    ReplaySettings,
    ResetChannel,
    ResetTimeline,
}
impl MenuItem {
    pub fn text(self) -> &'static str {
        match self {
            Self::Upload => RENDER_VIDEO,
            Self::SaveReplay => "ID_REPLAYEDITOR_BASIC_SAVE_REPLAY",
            Self::TakePhoto => "ID_REPLAYEDITOR_TAKE_PHOTO",
            Self::ReplaySettings => "ID_REPLAY_EDITOR_REPLAY_MODE",
            Self::ResetChannel => "ID_REPLAY_EDITOR_RESET_TIMELINE_CURRENT_CHANNEL",
            Self::ResetTimeline => "ID_REPLAY_EDITOR_RESET_TIMELINE",
        }
    }
}

/// Replay Settings (0x82614f38 id 4 -> popup ID_REPLAY_EDITOR_REPLAY_MODE, handler 0x82615728): three toggles, each
/// flipping a replay-block bool (+0x347AC / +0x347B4 / +0x347B0) and posting it (research Q2:
/// cMsgReplayEnableRenderPlayer / OtherPlayers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub green_screen: bool,
    pub hide_player: bool,
    pub hide_skaters: bool,
    /// Item 16 extension rows, after retail's three: the letterbox (index into [`LETTERBOX`]) and the camera path.
    pub letterbox: u8,
    pub camera_path: bool,
}
pub(crate) const SETTINGS: [&str; 3] = [
    "ID_REPLAY_EDITOR_REPLAY_MODE_GREENSCREEN",
    "ID_REPLAY_EDITOR_REPLAY_MODE_PLAYER_VISIBLE",
    "ID_REPLAY_EDITOR_REPLAY_MODE_SKATERS_VISIBLE",
];
impl Look {
    pub fn toggle(&mut self, row: usize) {
        match row {
            0 => self.green_screen ^= true,
            1 => self.hide_player ^= true,
            2 => self.hide_skaters ^= true,
            3 => self.letterbox = (self.letterbox + 1) % LETTERBOX.len() as u8,
            4 => self.camera_path ^= true,
            _ => {}
        }
    }
    /// Replay Settings' rows: retail's three ids, then the extension rows as literal text with their value.
    pub fn rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = SETTINGS.iter().map(|s| s.to_string()).collect();
        rows.push(crate::apt_text::literal(&format!("Letterbox: {}", LETTERBOX_TEXT[usize::from(self.letterbox) % 4])));
        rows.push(crate::apt_text::literal(&format!("Camera Path: {}", if self.camera_path { "On" } else { "Off" })));
        rows
    }
    /// The letterbox's picture aspect, 0 = off.
    pub fn aspect(&self) -> f32 {
        LETTERBOX[usize::from(self.letterbox) % 4]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    /// kScrubMode 0: instant replay.
    Scrub,
    /// kMainEditMode 9: edit timeline.
    MainEdit,
    /// kEffectsMode 4: the channel's panel.
    Effects,
    Camera(CameraKind),
    Help,
    Menu,
    Settings,
    Popup(PopupKind),
    /// kOnlineInstantReplayMode 20: view and photo only.
    OnlineInstant,
}

/// The pad as the editor reads it (FE key codes = Apt input index - 124, research Q3 / replay-progress.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pad {
    /// 0
    Start,
    /// 1 (AptSelect)
    Back,
    /// 2
    A,
    /// 3
    B,
    /// 5
    X,
    /// 4
    Y,
    /// 6
    Rb,
    /// 7 / 10: a trigger's press edge (mode 1 stops playback on it).
    Trigger,
    /// 8
    R3,
    /// 9
    Lb,
    /// 11
    L3,
    /// The D-pad: 16..19 on press, 32..35 on release; hud.rs sends the edge the editor's state reads.
    Up,
    Down,
    Left,
    Right,
    /// 46
    ARelease,
    /// 53
    LbRelease,
    /// 20..23: right stick flicks (unhandled by every retail editor handler).
    RsLeft,
    RsRight,
    RsUp,
    RsDown,
}

/// What a key asks of the HUD.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Out {
    /// An FE sound (sound::*).
    Sound(u64),
    /// Item 16 extension: re-set the Effects panel's header (GetModeText, with the key ease).
    Header,
    /// The movie's `SetState(n)`.
    State(u32),
    /// Open (true) / close the help movie.
    Help(bool),
    Popup(PopupKind),
    Menu,
    Settings,
    ClosePopup,
    Exit,
    Photo,
    SaveReplay,
    Upload,
}

#[derive(Clone, Debug)]
pub(crate) struct Editor {
    pub state: State,
    /// 0..1 over the recording.
    pub playhead: f32,
    pub playing: bool,
    pub in_mark: f32,
    pub out_mark: f32,
    pub clip_markers: Vec<f32>,
    pub keys: [Vec<Key>; 7],
    pub hidden: bool,
    pub channel: Channel,
    /// GetCurrentTint (ed+640): the tint sub-channel R, G, B, A.
    pub tint_channel: usize,
    /// The panel's value (GetSimpleSpeedSetting, GetModeSliderValue, the tint getters) until Apply / Create Key.
    pub pending: [i32; 4],
    /// The camera list's selection in the Effects panel (GetCurrentCameraEditIndex).
    pub camera_index: usize,
    /// The shot being edited in a camera edit state.
    pub shot: Shot,
    /// 1a: `shot` is new (Shot::new: no eye) and waits for hud.rs to start it at the camera on screen
    /// (cameras::start_edit) before anything draws it.
    pub unplaced: bool,
    pub look: Look,
    pub online: bool,
    pub(crate) scrub_velocity: f32,
    /// The in / out marks' eased velocities (review M2: the scrub block, 0x826114d8 / 0x8261158c).
    mark_velocity: [f32; 2],
    /// Where Help / a popup / the menu returns to.
    resume: Option<State>,
    /// Mode 2 (LB held on the timeline, 0x82612f00): only LB's release (53) passes.
    pub(crate) moving_marks: bool,
    /// ed+157: A reached a camera key; only A's release (46) passes, which opens that key's camera edit.
    pub(crate) a_lock: bool,
    /// ed+176: Instant Replay's stick-orbit follow camera (mode-5 update 0x8261135c); None = the clip's camera.
    pub orbit: Option<Shot>,
    /// ed+124: the Free camera's height velocity (0x8261173c, x0.6 per update).
    pub rise: f32,
    /// cam+0x38: the D-pad's aim (Follow / Tripod, +-1) or rise (Free, +-10) total in this camera edit, zeroed each
    /// time a camera is entered (0x82c80044 / 0x82c80324 / 0x82c805dc / 0x82c80870) and as Instant Replay's orbit
    /// starts (0x82611494).
    pub aim_total: f32,
    /// A save from before 10-27 (file.rs): its Follow shots are still in the skater's frame until hud.rs opens the
    /// editor over its recording (cameras::migrate_follow).
    pub legacy_follow: bool,
    /// Bug 10-17 #28: the hand-steered shot ([`Editor::hand`]) before the last 60 Hz update; the display carries the
    /// shot on by that step between updates (cameras::evaluate). None = nothing to carry on from.
    pub before: Option<Shot>,
    /// Item 16 extension: the ease the panel / camera edit writes with its key.
    pub pending_ease: Ease,
    /// Item 16 extension: a copied key (channel, value, ease).
    pub clipboard: Option<(Channel, Value, Ease)>,
}

impl Editor {
    pub fn new(online: bool) -> Self {
        Self {
            state: if online { State::OnlineInstant } else { State::Scrub },
            playhead: OPEN_PLAYHEAD,
            playing: false,
            in_mark: 0.,
            out_mark: 1.,
            clip_markers: Vec::new(),
            keys: Default::default(),
            hidden: false,
            channel: Channel::Camera,
            tint_channel: 0,
            pending: [0; 4],
            camera_index: CameraKind::Game.index(),
            shot: Shot::new(CameraKind::Game),
            unplaced: false,
            look: Look::default(),
            online,
            scrub_velocity: 0.,
            mark_velocity: [0.; 2],
            resume: None,
            moving_marks: false,
            a_lock: false,
            orbit: None,
            rise: 0.,
            aim_total: 0.,
            legacy_follow: false,
            before: None,
            pending_ease: Ease::Hold,
            clipboard: None,
        }
    }

    /// The movie state for the editor's current state (what `SetState` shows it).
    pub fn movie_state(&self) -> u32 {
        let number = |state: &State| match state {
            State::Scrub => Some(0),
            State::MainEdit => Some(9),
            State::Effects => Some(4),
            State::Camera(kind) => Some(kind.edit_state()),
            State::OnlineInstant => Some(20),
            State::Help | State::Menu | State::Settings | State::Popup(_) => None,
        };
        number(&self.state).or_else(|| self.resume.as_ref().and_then(number)).unwrap_or(0)
    }

    /// One retail update (60 Hz) of the trigger scrub; research Q3 [disc].
    pub fn scrub(&mut self, rt: f32, lt: f32) {
        ease(&mut self.scrub_velocity, rt - lt);
        if self.scrub_velocity != 0. {
            self.playing = false;
        }
        self.playhead = (self.playhead + self.scrub_velocity).clamp(0.0, 1.0);
    }

    /// Playback for `seconds` of real time over a recording `length` seconds long, at the Speed channel's
    /// slow motion; stops at the out mark. Returns whether it is still playing.
    pub fn play(&mut self, seconds: f32, length: f32) -> bool {
        if !self.playing || length <= 0. {
            self.playing = false;
            return false;
        }
        // Retail: the clip's held Speed (int_at); an eased Speed key (extension) ramps it.
        let speed = self.value_at(Channel::Speed, self.playhead) * 0.01;
        self.playhead = (self.playhead + seconds * speed / length).min(self.out_mark.max(self.in_mark));
        if self.playhead >= self.out_mark {
            self.playing = false;
        }
        self.playing
    }

    /// The hand-steered shot: a camera edit's, or Instant Replay's stick orbit (None = the timeline's camera).
    pub fn hand(&self) -> Option<Shot> {
        match self.state {
            State::Camera(_) => Some(self.shot),
            State::Scrub | State::OnlineInstant => self.orbit,
            _ => None,
        }
    }

    /// Bug 10-17 #28: where the replay is shown, `ahead` seconds (under one update) past the last 60 Hz update. The
    /// editor steps at 60 Hz as retail's does, but retail also draws at exactly 60 Hz; drawn faster, or with frame-time
    /// wobble, the picture held between steps and then jumped (the judder). Display only: the playhead stays on the
    /// updates. `scrubbing`: this state runs the triggers' scrub.
    pub fn shown(&self, ahead: f32, length: f32, scrubbing: bool) -> f32 {
        if self.playing && length > 0. {
            let speed = self.value_at(Channel::Speed, self.playhead) * 0.01;
            (self.playhead + ahead * speed / length).min(self.out_mark.max(self.in_mark))
        } else if scrubbing {
            (self.playhead + self.scrub_velocity * (ahead / UPDATE)).clamp(0., 1.)
        } else {
            self.playhead
        }
    }

    // ------------------------------------------------------------ the timeline
    /// The clip containing `t`: its start and end (0 / clip markers / 1).
    pub fn clip_at(&self, t: f32) -> (f32, f32) {
        let start = self.clip_markers.iter().copied().filter(|&m| m <= t).fold(0., f32::max);
        let end = self.clip_markers.iter().copied().filter(|&m| m > t).fold(1., f32::min);
        (start, end)
    }

    /// Every segment of the encoded range (in..out) with constant speed: (from, to, speed %).
    pub fn clips(&self) -> impl Iterator<Item = (f32, f32, i32)> + '_ {
        let (a, b) = (self.in_mark.min(self.out_mark), self.out_mark.max(self.in_mark));
        let mut cuts: Vec<f32> = self.keys[Channel::Speed.index()].iter().map(|k| k.t).chain(self.clip_markers.iter().copied()).filter(|&t| t > a && t < b).collect();
        cuts.push(a);
        cuts.push(b);
        cuts.sort_by(f32::total_cmp);
        cuts.dedup();
        let pairs: Vec<(f32, f32)> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
        pairs.into_iter().map(move |(x, y)| (x, y, self.int_at(Channel::Speed, x)))
    }

    /// GetEncodedLength 0x8250b590 [disc]: sum of clip_seconds / (speed * 0.01), + 0.5, truncated.
    pub fn encoded_seconds(&self, timeline_seconds: f32) -> u32 {
        (self.encoded_length(timeline_seconds) + 0.5) as u32
    }
    /// The encoded length in seconds, unrounded (the sum GetEncodedLength rounds).
    pub fn encoded_length(&self, timeline_seconds: f32) -> f32 {
        self.clips().map(|(a, b, speed)| self.span_seconds(a, b, speed, timeline_seconds)).sum()
    }
    /// One segment of [`Editor::clips`] in encoded seconds: retail's `(b - a) * T / (speed * 0.01)`, or, under an
    /// eased Speed key (extension), the midpoint sum of the ramp.
    pub fn span_seconds(&self, a: f32, b: f32, speed: i32, timeline_seconds: f32) -> f32 {
        if !self.ramped(a) {
            return (b - a) * timeline_seconds / (speed.max(1) as f32 * 0.01);
        }
        let dt = (b - a) / RAMP_STEPS as f32;
        (0..RAMP_STEPS).map(|i| dt * timeline_seconds / (self.time_scale(a + (i as f32 + 0.5) * dt))).sum()
    }
    /// The playhead `seconds` (encoded) into the ramped segment a..b: the inverse of [`Editor::span_seconds`].
    pub fn ramp_playhead(&self, a: f32, b: f32, mut seconds: f32, timeline_seconds: f32) -> f32 {
        let dt = (b - a) / RAMP_STEPS as f32;
        for i in 0..RAMP_STEPS {
            let step = dt * timeline_seconds / self.time_scale(a + (i as f32 + 0.5) * dt);
            if seconds < step {
                return a + i as f32 * dt + dt * seconds / step.max(1e-9);
            }
            seconds -= step;
        }
        b
    }
    /// The Speed key segment holding `t` eases to a next key (an extension speed ramp).
    pub fn ramped(&self, t: f32) -> bool {
        let (keys, i) = self.segment(Channel::Speed, t);
        i.is_some_and(|i| keys[i].2 != Ease::Hold && i + 1 < keys.len())
    }
    /// Playback's and the audio records' time scale at `t`: speed * 0.01 (retail `speed_at.max(1) * 0.01`).
    pub fn time_scale(&self, t: f32) -> f32 {
        self.value_at(Channel::Speed, t).max(1.) * 0.01
    }

    /// The channel's keys of the clip at `t` as (time, value, ease), and the one at or before `t` (key_at's).
    fn segment(&self, channel: Channel, t: f32) -> (Vec<(f32, [f32; 4], Ease)>, Option<usize>) {
        let (start, end) = self.clip_at(t);
        let keys: Vec<_> = self.keys[channel.index()]
            .iter()
            .filter(|k| k.t >= start - 1e-6 && (k.t < end || k.t <= t + 1e-6))
            .filter_map(|k| match &k.value {
                Value::Int(v) => Some((k.t, [*v as f32, 0., 0., 0.], k.ease)),
                Value::Tint(v) => Some((k.t, v.map(|c| c as f32), k.ease)),
                Value::Camera(_) => None,
            })
            .collect();
        let i = keys.iter().rposition(|k| k.0 <= t + 1e-6);
        (keys, i)
    }
    /// The channel's value at `t` with the key's ease toward the next key in the clip (Hold = retail's step).
    fn blend(&self, channel: Channel, t: f32) -> Option<[f32; 4]> {
        let (keys, i) = self.segment(channel, t);
        let i = i?;
        let (ta, a, ease) = keys[i];
        let Some(&(tb, b, _)) = keys.get(i + 1) else { return Some(a) };
        let u = ((t - ta) / (tb - ta).max(1e-6)).clamp(0., 1.);
        let p0 = if i > 0 { keys[i - 1].1 } else { a };
        let p3 = keys.get(i + 2).map_or(b, |k| k.1);
        Some(std::array::from_fn(|c| match ease {
            Ease::Hold => a[c],
            Ease::Linear => a[c] + (b[c] - a[c]) * u,
            Ease::Smooth => catmull(p0[c], a[c], b[c], p3[c], u),
        }))
    }
    /// [`Editor::int_at`] with the extension eases: what the speed, grade and export read.
    pub fn value_at(&self, channel: Channel, t: f32) -> f32 {
        let (_, _, min, max) = channel.range();
        self.blend(channel, t).map_or(channel.default_value() as f32, |v| v[0].clamp(min as f32, max as f32))
    }
    /// [`Editor::tint_at`] with the extension eases.
    pub fn tint_value_at(&self, t: f32) -> [f32; 4] {
        self.blend(Channel::Tint, t).map_or([0.; 4], |v| v.map(|c| c.clamp(0., 250.)))
    }

    fn key_at(&self, channel: Channel, t: f32) -> Option<&Key> {
        let (start, _) = self.clip_at(t);
        self.keys[channel.index()].iter().filter(|k| k.t <= t + 1e-6 && k.t >= start - 1e-6).last()
    }

    /// The channel's value at `t`: the last key at or before `t` in the same clip, else the default
    /// (RULING, build-progress Task 4: keys step-hold until the next key).
    pub fn int_at(&self, channel: Channel, t: f32) -> i32 {
        match self.key_at(channel, t).map(|k| &k.value) {
            Some(Value::Int(v)) => *v,
            _ => channel.default_value(),
        }
    }
    pub fn tint_at(&self, t: f32) -> [i32; 4] {
        match self.key_at(Channel::Tint, t).map(|k| &k.value) {
            Some(Value::Tint(v)) => *v,
            _ => [0; 4],
        }
    }
    /// The clip's camera at `t` (its hard key, else the Game camera).
    pub fn camera_at(&self, t: f32) -> Shot {
        let (start, _) = self.clip_at(t);
        self.keys[0]
            .iter()
            .filter(|k| !k.soft && (k.t - start).abs() < 1e-6)
            .find_map(|k| match k.value {
                Value::Camera(shot) => Some(shot),
                _ => None,
            })
            .unwrap_or(Shot::new(CameraKind::Game))
    }
    /// The Adv* camera keys of the clip at `t`, in time order: (t, shot, cut, ease).
    pub fn camera_keys(&self, t: f32) -> Vec<(f32, Shot, bool, Ease)> {
        let (start, end) = self.clip_at(t);
        let mut keys: Vec<_> = self.keys[0]
            .iter()
            .filter(|k| k.soft && k.t >= start && k.t < end)
            .filter_map(|k| match k.value {
                Value::Camera(shot) => Some((k.t, shot, k.cut, k.ease)),
                _ => None,
            })
            .collect();
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        keys
    }

    /// The clip marker or key marker (of the current channel) under the playhead: 0x825084c8 / 0x82508d88.
    pub fn marker_under(&self) -> Option<f32> {
        self.clip_markers.iter().copied().find(|m| (m - self.playhead).abs() <= MARKER_RADIUS)
    }
    pub fn key_under(&self) -> Option<f32> {
        self.keys[self.channel.index()]
            .iter()
            .filter(|k| k.soft)
            .map(|k| k.t)
            .find(|t| (t - self.playhead).abs() <= MARKER_RADIUS)
    }
    /// AnySoftMarkersInClip 0x82508828: key markers of the current channel in the playhead's clip.
    pub fn any_soft_markers_in_clip(&self) -> bool {
        let (start, end) = self.clip_at(self.playhead);
        self.keys[self.channel.index()].iter().any(|k| k.soft && k.t >= start && k.t < end)
    }

    /// Reset Timeline (all) or Reset Channel.
    pub fn reset_timeline(&mut self, channel: Option<Channel>) {
        match channel {
            Some(channel) => self.keys[channel.index()].clear(),
            None => {
                self.keys = Default::default();
                self.clip_markers.clear();
                self.in_mark = 0.;
                self.out_mark = 1.;
            }
        }
    }

    /// The playhead is inside the timeline's margins (0x82613050 / 0x826132a8: (0.0089, 0.9911)).
    pub(crate) fn inside(&self) -> bool {
        self.playhead > MARKER_RADIUS && self.playhead < 1. - MARKER_RADIUS
    }
    /// D-pad Down: drop a clip marker at the playhead (0x826132a8). Err(None) = the error sound (on a clip marker or
    /// outside the margins); Err(Some) = the popup it raises (key markers in the clip ask first). No count limit
    /// ([`MAX_LOADED`]).
    pub fn drop_marker(&mut self) -> Result<(), Option<PopupKind>> {
        if !self.inside() || self.marker_under().is_some() {
            return Err(None);
        }
        if self.any_soft_markers_in_clip() {
            return Err(Some(PopupKind::ClipMarker));
        }
        self.insert_marker(self.playhead);
        Ok(())
    }
    fn insert_marker(&mut self, t: f32) {
        // The new clip keeps the value it showed: pin every channel at the marker.
        for channel in Channel::ALL {
            let value = match channel {
                Channel::Camera => Value::Camera(self.camera_at(t)),
                Channel::Tint => Value::Tint(self.tint_at(t)),
                _ => Value::Int(self.int_at(channel, t)),
            };
            if self.key_at(channel, t).is_some() || channel == Channel::Camera {
                let ease = self.key_at(channel, t).map_or(Ease::retail(channel), |k| k.ease);
                self.keys[channel.index()].push(Key { t, soft: false, cut: false, value, ease });
            }
        }
        self.clip_markers.push(t);
        self.clip_markers.sort_by(f32::total_cmp);
        for keys in &mut self.keys {
            keys.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.soft.cmp(&b.soft)));
        }
    }

    /// D-pad Up over a clip marker: removes it, its hard keys and the key markers of both clips it bounds
    /// (ID_REPLAY_EDITOR_POPUP_CLIPMARKER).
    pub fn remove_marker(&mut self) -> Result<(), PopupKind> {
        let Some(m) = self.marker_under() else {
            return Err(PopupKind::DeleteClip);
        };
        let before = self.clip_markers.iter().copied().filter(|&x| x < m).fold(0., f32::max);
        let after = self.clip_markers.iter().copied().filter(|&x| x > m).fold(1., f32::min);
        for keys in &mut self.keys {
            keys.retain(|k| !((k.t - m).abs() < 1e-6 || (k.soft && k.t >= before && k.t < after)));
        }
        self.clip_markers.retain(|&x| x != m);
        Ok(())
    }
    /// D-pad Up over a key marker of the current channel.
    pub fn remove_key(&mut self) -> bool {
        let Some(t) = self.key_under() else { return false };
        self.keys[self.channel.index()].retain(|k| !(k.soft && k.t == t));
        true
    }

    /// The panel's value for the channel at the playhead.
    fn load_pending(&mut self) {
        self.pending = match self.channel {
            Channel::Tint => self.tint_at(self.playhead),
            channel => [self.int_at(channel, self.playhead), 0, 0, 0],
        };
        self.pending_ease = self.key_at(self.channel, self.playhead).map_or(Ease::retail(self.channel), |k| k.ease);
    }

    /// Left / right in the panel: retail's step, range and wrap.
    pub fn adjust(&mut self, channel: Channel, up: bool) {
        let (_, step, min, max) = channel.range();
        if self.state != State::Effects {
            self.channel = channel;
            self.load_pending();
        }
        let slot = if channel == Channel::Tint { self.tint_channel } else { 0 };
        let v = &mut self.pending[slot];
        *v = match channel {
            // 0x8250adc0: +1 and wrap at 4.
            Channel::Shake if up => (*v + 1) % (max + 1),
            Channel::Shake => (*v + max) % (max + 1),
            // ×0.04 + 0.5 rounding onto the 25 grid (0x825ae1c0).
            Channel::Tint => ((((*v + if up { step } else { -step }) as f32) * 0.04 + 0.5).floor() as i32 * 25).clamp(min, max),
            _ => (*v + if up { step } else { -step }).clamp(min, max),
        };
        // The panel edits the speed channel's live value (the speed tests read it at the playhead).
        if self.state != State::Effects {
            let pending = self.pending;
            self.write_value(channel, pending, false, Ease::retail(channel));
        }
    }

    /// Apply: the pending value to the key under the playhead, else to the playhead's clip (its hard key).
    fn apply(&mut self) {
        let (pending, ease, channel) = (self.pending, self.pending_ease, self.channel);
        if let Some(t) = self.key_under() {
            if let Some(k) = self.keys[channel.index()].iter_mut().find(|k| k.soft && k.t == t) {
                k.value = value_of(channel, pending, &k.value);
                k.ease = ease;
            }
            return;
        }
        self.write_value(channel, pending, false, ease);
    }
    /// Create Key: a key marker at the playhead with the pending value (no count limit, [`MAX_LOADED`]).
    fn create_key(&mut self) {
        let (channel, pending) = (self.channel, self.pending);
        self.write_value(channel, pending, true, self.pending_ease);
    }
    fn write_value(&mut self, channel: Channel, pending: [i32; 4], soft: bool, ease: Ease) {
        let (start, end) = self.clip_at(self.playhead);
        let t = if soft { self.playhead } else { start };
        // The next clip keeps what it showed.
        if end < 1. && !self.keys[channel.index()].iter().any(|k| !k.soft && (k.t - end).abs() < 1e-6) {
            let value = match channel {
                Channel::Tint => Value::Tint(self.tint_at(end)),
                _ => Value::Int(self.int_at(channel, end)),
            };
            self.keys[channel.index()].push(Key { t: end, soft: false, cut: false, value, ease: Ease::retail(channel) });
        }
        let keys = &mut self.keys[channel.index()];
        let value = value_of(channel, pending, &Value::Int(0));
        match keys.iter_mut().find(|k| k.soft == soft && (k.t - t).abs() < 1e-6) {
            Some(k) => {
                k.value = value;
                k.ease = ease;
            }
            None => keys.push(Key { t, soft, cut: false, value, ease }),
        }
        keys.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.soft.cmp(&b.soft)));
    }
    /// Whole-timeline speed (tests and the Speed channel with no markers).
    pub fn set_whole_clip_speed(&mut self, speed: i32) {
        let keys = &mut self.keys[Channel::Speed.index()];
        keys.clear();
        let value = Value::Int(speed.clamp(10, 100));
        keys.push(Key { t: 0., soft: false, cut: false, value: value.clone(), ease: Ease::Hold });
        for &m in &self.clip_markers {
            keys.push(Key { t: m, soft: false, cut: false, value: value.clone(), ease: Ease::Hold });
        }
    }
    pub fn speed_at(&self, t: f32) -> i32 {
        self.int_at(Channel::Speed, t)
    }

    /// Commit the edited shot: the clip's camera, or an Adv* key at the playhead.
    fn commit_camera(&mut self) {
        let shot = self.shot;
        let (start, _) = self.clip_at(self.playhead);
        let keys = &mut self.keys[0];
        let clip_kind = keys.iter().find(|k| !k.soft && (k.t - start).abs() < 1e-6).and_then(|k| match k.value {
            Value::Camera(s) => Some(s.kind),
            _ => None,
        });
        if clip_kind != Some(shot.kind) {
            // A different camera drops the clip's Adv keys (ID_REPLAY_EDITOR_REPLACE_CAMERA_DESC).
            let (s, e) = self.clip_at(self.playhead);
            self.keys[0].retain(|k| !(k.soft && k.t >= s && k.t < e));
        }
        let ease = self.pending_ease;
        let keys = &mut self.keys[0];
        match keys.iter_mut().find(|k| !k.soft && (k.t - start).abs() < 1e-6) {
            Some(k) => k.value = Value::Camera(shot),
            None => keys.push(Key { t: start, soft: false, cut: false, value: Value::Camera(shot), ease: Ease::Smooth }),
        }
        if shot.kind.advanced() {
            let t = self.playhead;
            match keys.iter_mut().find(|k| k.soft && (k.t - t).abs() <= MARKER_RADIUS) {
                Some(k) => {
                    k.value = Value::Camera(shot);
                    k.ease = ease;
                }
                None => keys.push(Key { t, soft: true, cut: false, value: Value::Camera(shot), ease }),
            }
        }
        keys.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.soft.cmp(&b.soft)));
    }

    /// 1b (TU3 0x8261141c): scrubbing in Instant Replay commits its stick orbit; the clip under the playhead becomes
    /// that Follow camera, as a camera edit's A commits. Review I1: never over a clip with Adv keys (what retail does
    /// to the keys between is unverified, and the commit would drop them all): there the orbit just goes.
    pub fn commit_orbit(&mut self) {
        let Some(orbit) = self.orbit.take() else { return };
        if self.camera_at(self.playhead).kind.advanced() {
            return;
        }
        let shot = std::mem::replace(&mut self.shot, orbit);
        self.commit_camera();
        self.shot = shot;
    }

    // ------------------------------------------------------------ input
    fn open(&mut self, state: State) {
        if !matches!(self.state, State::Help | State::Menu | State::Settings | State::Popup(_)) {
            self.resume = Some(self.state.clone());
        }
        self.state = state;
    }
    fn back(&mut self) -> State {
        let resume = self.resume.take().unwrap_or(if self.online { State::OnlineInstant } else { State::Scrub });
        self.state = resume.clone();
        resume
    }
    /// Opens `kind` over the current state.
    pub(crate) fn popup(&mut self, kind: PopupKind) -> Vec<Out> {
        self.playing = false;
        self.open(State::Popup(kind));
        vec![Out::Popup(kind)]
    }
    /// The stop helper 0x82615648: from Instant Replay back to mode 5 (state 19); from playback started in Edit
    /// Timeline back to mode 6 (states 21 then 19).
    fn stop_outs(&mut self) -> Vec<Out> {
        self.playing = false;
        if self.state == State::MainEdit { vec![Out::State(21), Out::State(19)] } else { vec![Out::State(19)] }
    }
    /// What the HUD sends when playback runs out (0x82611318 calls the stop helper at the end head).
    pub fn stop_states(&self) -> Vec<u32> {
        if self.state == State::MainEdit { vec![21, 19] } else { vec![19] }
    }
    /// 0x82615560(1): show the HUD (state 1002).
    fn show(&mut self) -> Out {
        self.hidden = false;
        Out::State(1002)
    }
    /// L3: CanDisplayShowHide's modes {1, 5, 6} (0x825adf00) toggle the HUD (kHideEditor 1003 / kShowEditor 1002).
    fn toggle_hud(&mut self) -> Out {
        self.hidden ^= true;
        Out::State(if self.hidden { 1003 } else { 1002 })
    }
    fn to_main(&mut self, sound: u64) -> Vec<Out> {
        self.state = State::MainEdit;
        vec![Out::Sound(sound), Out::State(21), Out::State(9)]
    }
    fn help(&mut self, sounds: &[u64]) -> Vec<Out> {
        self.open(State::Help);
        let mut out: Vec<Out> = sounds.iter().map(|&s| Out::Sound(s)).collect();
        out.push(Out::Help(true));
        out
    }

    /// A press (or the releases and flicks hud.rs reports). Returns what the HUD shows and plays.
    pub fn key(&mut self, pad: Pad) -> Vec<Out> {
        match self.state.clone() {
            State::Scrub | State::MainEdit | State::OnlineInstant => self.timeline_key(pad),
            State::Effects => self.effects_key(pad),
            State::Camera(kind) => self.camera_key(kind, pad),
            State::Help => match pad {
                Pad::B | Pad::Start => {
                    self.back();
                    vec![Out::Help(false)]
                }
                _ => Vec::new(),
            },
            // The popups, menu and settings take their input through the popup movie (hud.rs -> answer / pick).
            State::Menu | State::Settings | State::Popup(_) => Vec::new(),
        }
    }

    /// RB from a stop: play from the in mark when at or past the out mark (mode 1, state 18). Instant Replay's RB
    /// drops the stick orbit first (0x82613988 clears +176).
    fn toggle_play(&mut self) -> Vec<Out> {
        if self.playing {
            return self.stop_outs();
        }
        if self.playhead >= self.out_mark - 1e-4 || self.playhead < self.in_mark {
            self.playhead = self.in_mark;
        }
        self.orbit = None;
        self.playing = true;
        self.scrub_velocity = 0.;
        vec![Out::State(18)]
    }

    /// Instant Replay (mode 5, handler 0x82613988), Edit Timeline (mode 6, 0x82612c28), playback (mode 1) and the
    /// LB mark move (mode 2), per the dispatcher 0x826127b8. Online (state 20, [0x830b7ae8] in {4, 5}) gates Start,
    /// A, RB and the D-pad.
    fn timeline_key(&mut self, pad: Pad) -> Vec<Out> {
        use sound::*;
        let main = self.state == State::MainEdit;
        let gated = self.state == State::OnlineInstant;
        if self.moving_marks {
            if pad != Pad::LbRelease {
                return Vec::new();
            }
            self.moving_marks = false;
            return vec![Out::State(21), Out::State(1004)];
        }
        if self.a_lock {
            if pad != Pad::ARelease {
                return Vec::new();
            }
            self.a_lock = false;
            return self.open_key_camera();
        }
        if self.playing {
            return match pad {
                Pad::B => {
                    let mut out = self.stop_outs();
                    out.push(self.show());
                    out.push(Out::Sound(QUIT));
                    out.extend(self.popup(PopupKind::Quit));
                    out
                }
                Pad::Rb | Pad::Trigger => self.stop_outs(),
                Pad::L3 => vec![Out::Sound(HUD), self.toggle_hud()],
                _ => Vec::new(),
            };
        }
        if pad == Pad::L3 {
            return vec![self.toggle_hud()];
        }
        if self.hidden {
            return match pad {
                Pad::Rb if !gated => self.toggle_play(),
                Pad::X if main => self.menu_from_hidden(),
                Pad::Back if !main => self.menu_from_hidden(),
                Pad::B if main => {
                    let mut out = vec![Out::State(19), self.show(), Out::Sound(QUIT)];
                    out.extend(self.popup(PopupKind::Quit));
                    out
                }
                Pad::B => {
                    let mut out = vec![Out::Sound(QUIT)];
                    out.extend(self.popup(PopupKind::Quit));
                    out
                }
                _ => Vec::new(),
            };
        }
        match pad {
            Pad::B => {
                let mut out = vec![Out::Sound(QUIT)];
                out.extend(self.popup(PopupKind::Quit));
                out
            }
            Pad::X => {
                self.open(State::Menu);
                vec![Out::Sound(MENU), Out::Menu]
            }
            Pad::Start if !gated => self.help(&[HELP]),
            Pad::Rb if !gated => self.toggle_play(),
            // Instant A: Edit Timeline (state 21, mode 6, state 9; the instant flag +130 cleared).
            Pad::A if !main && !gated => {
                self.orbit = None;
                self.state = State::MainEdit;
                vec![Out::Sound(KEY), Out::State(21), Out::State(9)]
            }
            // Instant D-pad Left / Right act on press: the start (0x825088b8) / end (0x82c7f378).
            Pad::Left | Pad::Right if !main && !gated => {
                self.orbit = None;
                let right = pad == Pad::Right;
                self.playhead = if right { self.out_mark } else { self.in_mark };
                vec![Out::Sound(if right { NEXT } else { PREVIOUS }), Out::State(999)]
            }
            Pad::A if main => self.edit_clip(),
            Pad::Y if main => {
                self.channel = Channel::ALL[(self.channel.index() + 1) % 7];
                vec![Out::Sound(CHANNEL), Out::State(23)]
            }
            Pad::Lb if main => {
                self.moving_marks = true;
                vec![Out::Sound(PREVIOUS), Out::State(1005)]
            }
            Pad::Up if main => self.remove_under(),
            Pad::Down if main => self.add_marker(),
            // Previous / next marker (0x82613450 / 0x82613408): clip markers and the in / out marks.
            Pad::Left | Pad::Right if main => {
                let right = pad == Pad::Right;
                let mut stops: Vec<f32> = self.clip_markers.clone();
                stops.extend([self.in_mark, self.out_mark]);
                self.step_to(&stops, right);
                vec![Out::Sound(if right { NEXT } else { PREVIOUS }), Out::State(999)]
            }
            // Item 16 extensions on the right stick (codes 20..23, unhandled by 0x82612c28): previous / next key of
            // the channel, copy / paste a key.
            Pad::RsLeft | Pad::RsRight if main => {
                let right = pad == Pad::RsRight;
                let stops: Vec<f32> = self.keys[self.channel.index()].iter().map(|k| k.t).collect();
                let moved = self.step_to(&stops, right);
                vec![Out::Sound(match (moved, right) {
                    (false, _) => ERROR,
                    (true, true) => NEXT,
                    (true, false) => PREVIOUS,
                })]
            }
            Pad::RsUp if main => {
                self.copy_key();
                vec![Out::Sound(SELECT)]
            }
            Pad::RsDown if main => self.paste_key(),
            _ => Vec::new(),
        }
    }
    fn menu_from_hidden(&mut self) -> Vec<Out> {
        let mut out = vec![Out::State(19), self.show(), Out::Sound(sound::MENU)];
        self.open(State::Menu);
        out.push(Out::Menu);
        out
    }
    /// The nearest stop before / after the playhead; false when there is none.
    fn step_to(&mut self, stops: &[f32], right: bool) -> bool {
        let p = self.playhead;
        let next = if right {
            stops.iter().copied().filter(|&m| m > p + 1e-6).fold(None, |a: Option<f32>, m| Some(a.map_or(m, |a| a.min(m))))
        } else {
            stops.iter().copied().filter(|&m| m < p - 1e-6).fold(None, |a: Option<f32>, m| Some(a.map_or(m, |a| a.max(m))))
        };
        if let Some(t) = next {
            self.playing = false;
            self.playhead = t;
        }
        next.is_some()
    }

    /// Main A (0x82613050). 0x82508d88 reads a key marker of the channel before a clip marker (RULING: the order of
    /// its two tests is taken as written; both together need a key within 0.0089 of a clip marker).
    fn edit_clip(&mut self) -> Vec<Out> {
        use sound::*;
        if let Some(t) = self.key_under() {
            self.playhead = t;
            if self.channel == Channel::Camera {
                self.a_lock = true;
                return vec![Out::Sound(KEY)];
            }
            return self.open_effects(KEY);
        }
        if self.marker_under().is_some() || !self.inside() {
            return vec![Out::Sound(ERROR)];
        }
        self.open_effects(EDIT)
    }
    fn open_effects(&mut self, sound: u64) -> Vec<Out> {
        self.state = State::Effects;
        self.load_pending();
        self.camera_index = self.camera_at(self.playhead).kind.index();
        vec![Out::Sound(sound), Out::State(22), Out::State(4)]
    }
    /// A's release over a camera key (0x82508ce8): that key's camera edit, on that key's shot.
    fn open_key_camera(&mut self) -> Vec<Out> {
        let t = self.playhead;
        let Some(key) = self.keys[0].iter().find(|k| k.soft && (k.t - t).abs() <= MARKER_RADIUS).cloned() else {
            return Vec::new();
        };
        let Value::Camera(shot) = key.value else { return Vec::new() };
        self.shot = shot;
        self.pending_ease = key.ease;
        self.camera_index = shot.kind.index();
        self.state = State::Camera(shot.kind);
        vec![Out::State(22), Out::State(shot.kind.edit_state())]
    }
    /// D-pad Up (0x82613650): the key marker or clip marker under the playhead asks to be deleted.
    fn remove_under(&mut self) -> Vec<Out> {
        let kind = if self.key_under().is_some() {
            PopupKind::DeleteKey
        } else if self.marker_under().is_some() {
            PopupKind::DeleteClip
        } else {
            return vec![Out::Sound(sound::ERROR)];
        };
        let mut out = vec![Out::Sound(sound::REMOVE)];
        out.extend(self.popup(kind));
        out
    }
    /// D-pad Down (0x826132a8).
    fn add_marker(&mut self) -> Vec<Out> {
        match self.drop_marker() {
            Ok(()) => vec![Out::Sound(sound::ADD), Out::State(1006)],
            Err(None) => vec![Out::Sound(sound::ERROR)],
            Err(Some(kind)) => {
                let mut out = vec![Out::Sound(sound::ADD)];
                out.extend(self.popup(kind));
                out
            }
        }
    }

    /// Item 16 extension: the channel's key under the playhead (else its value there) to the clipboard.
    pub fn copy_key(&mut self) {
        let (channel, t) = (self.channel, self.playhead);
        let soft = self.keys[channel.index()].iter().find(|k| k.soft && (k.t - t).abs() <= MARKER_RADIUS);
        let ease = soft.or(self.key_at(channel, t)).map_or(Ease::retail(channel), |k| k.ease);
        let value = match (soft, channel) {
            (Some(k), _) => k.value.clone(),
            (None, Channel::Camera) => Value::Camera(self.camera_at(t)),
            (None, Channel::Tint) => Value::Tint(self.tint_at(t)),
            (None, _) => Value::Int(self.int_at(channel, t)),
        };
        self.clipboard = Some((channel, value, ease));
    }
    /// Item 16 extension: the clipboard onto the channel at the playhead: the key under it, else a new key marker
    /// (Shake: the clip's value). A camera shot pastes only onto a clip of its camera; anything else is the error.
    pub fn paste_key(&mut self) -> Vec<Out> {
        let error = vec![Out::Sound(sound::ERROR)];
        let Some((channel, value, ease)) = self.clipboard.clone().filter(|c| c.0 == self.channel) else { return error };
        if !self.inside() || self.marker_under().is_some() {
            return error;
        }
        match value {
            Value::Camera(shot) => {
                if self.camera_at(self.playhead).kind != shot.kind {
                    return error;
                }
                (self.shot, self.pending_ease) = (shot, ease);
                self.commit_camera();
            }
            Value::Int(_) | Value::Tint(_) => {
                self.pending = match value {
                    Value::Tint(v) => v,
                    Value::Int(v) => [v, 0, 0, 0],
                    Value::Camera(_) => unreachable!(),
                };
                self.pending_ease = ease;
                if self.key_under().is_some() || channel == Channel::Shake {
                    self.apply();
                } else {
                    self.create_key();
                }
            }
        }
        vec![Out::Sound(sound::CREATE)]
    }

    /// The Effects panel (mode 11, 0x82612328).
    fn effects_key(&mut self, pad: Pad) -> Vec<Out> {
        use sound::*;
        let channel = self.channel;
        match (channel, pad) {
            (_, Pad::Start) => self.help(&[HELP]),
            (_, Pad::B) => self.to_main(BACK),
            (_, Pad::A) if self.marker_under().is_some() => Vec::new(),
            (Channel::Camera, Pad::A) => {
                let kind = CameraKind::LIST[self.camera_index];
                let mut out = vec![Out::Sound(SELECT)];
                let current = self.camera_at(self.playhead);
                if current.kind != kind && !self.camera_keys(self.playhead).is_empty() {
                    out.extend(self.popup(PopupKind::ReplaceCamera));
                    return out;
                }
                out.extend(self.enter_camera(kind));
                out
            }
            (Channel::Camera, Pad::Up | Pad::Down) => {
                let up = pad == Pad::Up;
                self.camera_index = (self.camera_index + if up { 6 } else { 1 }) % 7;
                vec![Out::Sound(if up { UP } else { DOWN }), Out::State(5)]
            }
            (Channel::Tint, Pad::Up | Pad::Down) => {
                let up = pad == Pad::Up;
                self.tint_channel = (self.tint_channel + if up { 3 } else { 1 }) % 4;
                vec![Out::Sound(if up { UP } else { DOWN }), Out::State(24)]
            }
            (_, Pad::Up | Pad::Down) => vec![Out::Sound(if pad == Pad::Up { UP } else { DOWN }), Out::State(5)],
            (_, Pad::Left | Pad::Right) if channel != Channel::Camera => {
                let before = self.pending;
                let up = pad == Pad::Right;
                self.adjust(channel, up);
                let mut out = Vec::new();
                if self.pending != before {
                    out.push(Out::Sound(if up { INC } else { DEC }));
                }
                out.push(Out::State(6));
                out
            }
            // A: Apply over a key marker or in a clip without them, else Create Key (UpdateEffectsSelection's
            // prompts); then back to Edit Timeline.
            (_, Pad::A) => {
                if !self.any_soft_markers_in_clip() || self.key_under().is_some() {
                    self.apply();
                } else if channel != Channel::Shake {
                    self.create_key();
                }
                self.to_main(SELECT)
            }
            (_, Pad::X) if channel != Channel::Shake => {
                self.create_key();
                self.to_main(CREATE)
            }
            // Item 16 extension: Y (ignored by 0x82612328) cycles the ease the key is written with.
            (_, Pad::Y) if !matches!(channel, Channel::Camera | Channel::Shake) => {
                self.pending_ease = self.pending_ease.next();
                vec![Out::Sound(INC), Out::Header]
            }
            _ => Vec::new(),
        }
    }
    /// The panel header's text: the channel, and the ease when it is not the retail one (extension).
    pub fn mode_text(&self) -> String {
        // The camera list's ease lives in the camera edit (Y there); Shake has none.
        if self.state == State::Effects && !matches!(self.channel, Channel::Camera | Channel::Shake) && self.pending_ease != Ease::retail(self.channel) {
            format!("#{} ({})", self.channel.text(), self.pending_ease.name())
        } else {
            self.channel.text().to_owned()
        }
    }

    fn enter_camera(&mut self, kind: CameraKind) -> Vec<Out> {
        let current = self.camera_at(self.playhead);
        self.shot = if current.kind == kind { current } else { Shot::new(kind) };
        // Retail starts a new camera from the one on screen (Free 0x82c852f0, Tripod 0x82c80298, Follow 0x82c7ffb8).
        self.unplaced = current.kind != kind && kind != CameraKind::Game;
        self.pending_ease = self.keys[0]
            .iter()
            .find(|k| k.soft && (k.t - self.playhead).abs() <= MARKER_RADIUS)
            .map_or(Ease::retail(Channel::Camera), |k| k.ease);
        (self.rise, self.aim_total) = (0., 0.);
        self.before = None;
        self.state = State::Camera(kind);
        vec![Out::State(kind.edit_state())]
    }

    /// A camera edit (modes 7-10): the dispatcher handles only Start and A; the sticks and D-pad are the per-mode
    /// update's (hud.rs -> cameras::steer). The other buttons are the item 16 extensions: LB / RB held roll (hud.rs),
    /// R3 resets the roll, Y cycles an Adv key's ease, X copies the shot, Back pastes one of the same camera.
    fn camera_key(&mut self, kind: CameraKind, pad: Pad) -> Vec<Out> {
        use sound::*;
        match pad {
            Pad::Start => self.help(&[SELECT, HELP]),
            Pad::A => {
                self.commit_camera();
                self.to_main(SELECT)
            }
            Pad::R3 => {
                self.shot.roll = 0.;
                // #28: a jump, not a step to carry on.
                self.before = None;
                vec![Out::Sound(DEC)]
            }
            Pad::Y if kind.advanced() => {
                self.pending_ease = self.pending_ease.next();
                vec![Out::Sound(INC)]
            }
            Pad::X => {
                self.clipboard = Some((Channel::Camera, Value::Camera(self.shot), self.pending_ease));
                vec![Out::Sound(SELECT)]
            }
            Pad::Back => match self.clipboard.clone() {
                Some((Channel::Camera, Value::Camera(shot), ease)) if shot.kind == kind => {
                    self.shot = shot;
                    self.before = None;
                    self.pending_ease = ease;
                    vec![Out::Sound(CREATE)]
                }
                _ => vec![Out::Sound(ERROR)],
            },
            _ => Vec::new(),
        }
    }

    /// The popup's choice (Yes/No popups: 1 = Yes).
    pub fn answer(&mut self, choice: usize) -> Vec<Out> {
        let State::Popup(kind) = self.state.clone() else { return Vec::new() };
        let yes = choice == 1;
        self.back();
        let mut out = vec![Out::ClosePopup];
        match kind {
            PopupKind::Quit if yes => out.push(Out::Exit),
            PopupKind::ClipMarker if yes => {
                self.insert_marker(self.playhead);
            }
            PopupKind::DeleteClip if yes => {
                let _ = self.remove_marker();
            }
            PopupKind::DeleteKey if yes => {
                self.remove_key();
            }
            PopupKind::ReplaceCamera if yes => {
                let kind = CameraKind::LIST[self.camera_index];
                out.extend(self.enter_camera(kind));
            }
            PopupKind::ResetTimeline if yes => self.reset_timeline(None),
            PopupKind::ResetChannel if yes => self.reset_timeline(Some(self.channel)),
            PopupKind::SaveReplay if yes => out.push(Out::SaveReplay),
            PopupKind::SavePhoto if yes => out.push(Out::Photo),
            PopupKind::Upload if yes => out.push(Out::Upload),
            _ => {}
        }
        out
    }

    /// 0x82614f38's rows for this editor: Upload only offline with an encoded length of at most 60 s (retail hides it
    /// rather than refusing), Save Replay and Take Photo always (signed in = always on PC, CanSaveMedia ruling;
    /// Take Photo needs the DLC, which RE3 is), the rest offline only (mode [0x830b7ae8] not 4 / 5).
    pub fn menu_rows(&self, timeline_seconds: f32) -> Vec<MenuItem> {
        let offline = !self.online;
        let mut rows = Vec::new();
        if offline && self.encoded_seconds(timeline_seconds) <= MAX_ENCODED_SECONDS {
            rows.push(MenuItem::Upload);
        }
        rows.extend([MenuItem::SaveReplay, MenuItem::TakePhoto]);
        if offline {
            rows.extend([MenuItem::ReplaySettings, MenuItem::ResetChannel, MenuItem::ResetTimeline]);
        }
        rows
    }

    /// The menu's pick (None = B).
    pub fn pick(&mut self, item: Option<MenuItem>) -> Vec<Out> {
        if self.state != State::Menu {
            return Vec::new();
        }
        self.back();
        let mut out = vec![Out::ClosePopup];
        match item {
            None => {}
            Some(MenuItem::SaveReplay) => out.extend(self.popup(PopupKind::SaveReplay)),
            Some(MenuItem::Upload) => out.extend(self.popup(PopupKind::Upload)),
            Some(MenuItem::TakePhoto) => out.extend(self.popup(PopupKind::SavePhoto)),
            Some(MenuItem::ReplaySettings) => {
                self.open(State::Settings);
                out.push(Out::Settings);
            }
            Some(MenuItem::ResetTimeline) => out.extend(self.popup(PopupKind::ResetTimeline)),
            Some(MenuItem::ResetChannel) => out.extend(self.popup(PopupKind::ResetChannel)),
        }
        out
    }

    /// Replay Settings: a row toggles (and the list stays); None = B.
    pub fn setting(&mut self, row: Option<usize>) -> Vec<Out> {
        if self.state != State::Settings {
            return Vec::new();
        }
        match row {
            Some(row) => {
                self.look.toggle(row);
                Vec::new()
            }
            None => {
                self.back();
                vec![Out::ClosePopup]
            }
        }
    }

    /// LB + left / right stick: move the in / out mark, one 60 Hz update with retail's scrub ease and settle per
    /// mark (0x826114d8 / 0x8261158c repeat the 0x82610f18 block; review M2). Call every update, the stick at 0
    /// included, so a released stick settles. The bounds stay MIN_CLIP apart (review I4, audit 3b). Returns whether
    /// the mark moved.
    pub fn move_mark(&mut self, out_mark: bool, stick: f32) -> bool {
        if self.state != State::MainEdit {
            self.mark_velocity = [0.; 2];
            return false;
        }
        let step = ease(&mut self.mark_velocity[usize::from(out_mark)], stick.clamp(-1., 1.));
        if step == 0. {
            return false;
        }
        if out_mark {
            self.out_mark = (self.out_mark + step).clamp((self.in_mark + MIN_CLIP).min(1.), 1.);
        } else {
            self.in_mark = (self.in_mark + step).clamp(0., (self.out_mark - MIN_CLIP).max(0.));
        }
        true
    }

    /// GetModeSliderValue 0x825ae0b8: B/20+5, (C-10)/2, S/10.
    pub fn slider_value(&self) -> i32 {
        let v = self.pending[0];
        match self.channel {
            Channel::Brightness => v / 20 + 5,
            Channel::Contrast => (v - 10) / 2,
            Channel::Saturation => v / 10,
            _ => v,
        }
    }
    /// GetEditIndex: the panel's selection (the speed / shake list has one row).
    pub fn edit_index(&self) -> usize {
        if self.channel == Channel::Camera { self.camera_index } else { 0 }
    }
}

/// research Q3 [disc]: 0x82610f18's per-update scrub block (gain, ease, clamp, settle) on one velocity; returns it.
fn ease(velocity: &mut f32, input: f32) -> f32 {
    let target = input * SCRUB_GAIN;
    *velocity += (target - *velocity) * SCRUB_EASE;
    *velocity = velocity.clamp(-SCRUB_GAIN, SCRUB_GAIN);
    if target.abs() < SCRUB_TARGET_SETTLE && velocity.abs() <= SCRUB_VELOCITY_SETTLE {
        *velocity = 0.0;
    }
    *velocity
}

/// Review I4: a loaded timeline inside the editor's own bounds (Channel::range, the 25 tint grid, lens 24..75,
/// finite shots, in <= out, MIN_CLIP apart); a timeline that cannot be made so is refused.
pub(crate) fn sanitise(editor: &mut Editor) -> Result<(), String> {
    let unit = |v: f32| v.is_finite() && (0. ..=1.).contains(&v);
    if !unit(editor.in_mark) || !unit(editor.out_mark) || editor.in_mark > editor.out_mark {
        return Err("in / out marks out of order".into());
    }
    // A save from before the MIN_CLIP gap (10-27) widens to it, as moving its marks would.
    if editor.out_mark - editor.in_mark < MIN_CLIP {
        editor.out_mark = (editor.in_mark + MIN_CLIP).min(1.);
        editor.in_mark = (editor.out_mark - MIN_CLIP).max(0.);
    }
    if editor.clip_markers.len() > MAX_LOADED || !editor.clip_markers.iter().all(|&m| unit(m)) {
        return Err("clip markers".into());
    }
    editor.clip_markers.sort_by(f32::total_cmp);
    editor.clip_markers.dedup();
    for channel in Channel::ALL {
        let (_, _, min, max) = channel.range();
        let keys = &mut editor.keys[channel.index()];
        if keys.len() > MAX_LOADED || !keys.iter().all(|k| unit(k.t)) {
            return Err(format!("{channel:?} keys"));
        }
        for key in keys.iter_mut() {
            match (&mut key.value, channel) {
                (Value::Camera(shot), Channel::Camera) => {
                    let finite = [shot.yaw, shot.pitch, shot.distance, shot.height, shot.lens, shot.roll].iter().all(|v| v.is_finite())
                        && shot.eye.is_finite()
                        && shot.look.is_finite();
                    if !finite {
                        return Err("camera key".into());
                    }
                    shot.lens = shot.lens.clamp(LENS_MIN, LENS_MAX);
                    shot.roll = wrap_degrees(shot.roll);
                }
                (Value::Tint(rgba), Channel::Tint) => {
                    *rgba = rgba.map(|v| ((v as f32 * 0.04 + 0.5).floor() as i32 * 25).clamp(min, max));
                }
                (Value::Int(v), c) if c != Channel::Camera && c != Channel::Tint => *v = (*v).clamp(min, max),
                _ => return Err(format!("{channel:?} key type")),
            }
        }
        keys.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.soft.cmp(&b.soft)));
    }
    editor.playhead = editor.in_mark;
    Ok(())
}

fn value_of(channel: Channel, pending: [i32; 4], _old: &Value) -> Value {
    match channel {
        Channel::Tint => Value::Tint(pending),
        _ => Value::Int(pending[0]),
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
