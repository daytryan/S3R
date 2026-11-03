//! The Replay Editor's front end: retail's `replay_editor3` (and `replay_help2`, `sk8popup`, `skatereel_tags`) on the
//! shared APT runtime, layer [`REPLAY`]. The movies run as shipped; this module answers their `FEReplayMenu.*`
//! natives from [`Editor`], sends them `SetState`, turns the pad into editor keys and the editor into the replay's
//! playhead, camera and grade. Entry: the pause menu's Replay row, or Back offline with >= 150 frames (Stage 0 §5).
//! Exit: B → "Quit?" → yes.
use super::{
    Replay, cameras,
    editor::{CameraKind, Channel, Ease, Editor, MAX_ENCODED_SECONDS, MenuItem, Out, Pad, State},
    export, mf,
    file::{self, Folder},
    grade::{self, Grade},
    reel::{Pick, Reel},
    timeline,
};
use crate::{
    apt_screen::{Layer, REPLAY, Screen, Screens},
    config::Config,
    core_menu_runtime::Key,
    popup_runtime,
    presentation::blend,
    skate_audio::FrontEndSoundRequest,
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use serde_json::{Value as Json, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) const FOLDER: &str = "private/replay-ui";
/// skate.Reel > My Replays (reel.rs), under the editor's screens.
const BROWSER: &str = "0-browser";
const EDITOR: &str = "1-editor";
const HELP: &str = "2-help";
const TAGS: &str = "3-tags";
const FRAME: f32 = super::editor::UPDATE;
/// An export capture that never lands (a minimised window that stopped rendering) fails the export (review I1).
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
/// A save's map that never comes up (review M7).
const LOAD_TIMEOUT: Duration = Duration::from_secs(90);
/// Engine strings with no retail id (prepare_replay_ui.py adds them to the movies' tables; the English is the
/// fallback for a table built before they existed).
const ENGINE_TEXT: [(&str, &str); 5] = [
    (
        "ID_S3R_REPLAY_DELETE_FAILED",
        "#This replay could not be deleted. Close any program that is using it, then try again.",
    ),
    (
        "ID_S3R_MEDIA_FEATURE_PACK",
        "#Exporting footage needs Windows Media Foundation. Install the Media Feature Pack (Settings > Apps > Optional features), then try again.",
    ),
    ("ID_S3R_REPLAY_MAP_UNAVAILABLE", "#The map this replay was recorded on could not be loaded."),
    ("ID_S3R_REPLAY_OFFLINE_ONLY", "#Saved replays open in single player only."),
    (
        "ID_S3R_EXPORT_AUDIO_MISSING",
        "#Exporting needs the game's sound files, and they could not be loaded. Run the launcher's setup again, then try again.",
    ),
];
/// The progress popup before the export's first frame (not retail, by request (owner 10-18)): the previous export's
/// sound still ending, then this one's loading, instead of a 0 % bar.
const EXPORT_STOPPING: &str = "#Stopping the previous render...";
const EXPORT_LOADING_SOUND: &str = "#Loading sound...";
/// Not retail, by request (user 10-27: "upload video should say render video"): the export's screens and notices
/// read as rendering, where retail's (an upload to skate.Reel) read as uploading. The start row is RENDER_VIDEO.
const RENDER_HELP: &str = "#Pick the video's resolution, frame rate and quality, then Render Video.";
const RENDER_CANCELLED: &str = "#The render was canceled.";
const RENDER_FAILED: &str = "#The video could not be rendered. Please try again.";
const RENDER_COMPLETE: &str = "#Render complete! Saved to";

// XInput button bits (ControllerInput::raw_input).
const UP: u16 = 0x0001;
const DOWN: u16 = 0x0002;
const LEFT: u16 = 0x0004;
const RIGHT: u16 = 0x0008;
const START: u16 = 0x0010;
const BACK: u16 = 0x0020;
const L3: u16 = 0x0040;
const R3: u16 = 0x0080;
const LB: u16 = 0x0100;
const RB: u16 = 0x0200;
const A: u16 = 0x1000;
const B: u16 = 0x2000;
const X: u16 = 0x4000;
const Y: u16 = 0x8000;
const DPAD: u16 = UP | DOWN | LEFT | RIGHT;

/// What the open sk8popup is for.
enum Ask {
    Editor,
    /// The X menu (0x82614f38) with the rows it was built with.
    Menu(Vec<MenuItem>),
    Settings,
    /// A one-button message (save / photo / export results).
    Notice,
    /// ID_MEMCARD_REPLAY_LOAD_FAILED: the damaged save; Yes deletes it.
    DeleteDamaged(PathBuf),
    /// skate.Reel's Y: ID_MEMCARD_PROMPT_DELETE_REPLAY; Yes deletes it.
    DeleteSave(PathBuf),
    /// ID_REPLAY_QUIT_CONF over a running mode (review I6); Yes ends it and opens the editor (online: state 20).
    QuitMode(bool),
    /// The export's progress; B cancels.
    Progress,
}

/// A replay loaded from disk, waiting for its map.
pub(crate) struct Loaded {
    pub header: file::Header,
    pub ring: super::record::Ring,
    pub editor: Editor,
    since: Instant,
    /// The map transition was asked for; idle again on another map is a failure (review M7).
    requested: bool,
}

/// `editor_select`: the fe sound gameplay's Back plays as it opens the replay editor (825EB998).
const EDITOR_SELECT: u64 = 0x3940_15B6_8783_BE9E;

/// An export in flight: the file, its sound, the camera that renders it (review I1, I2).
struct Exporting {
    job: export::Job,
    sound: export::Sound,
    /// Recorded audio frame the sound last took (Replay::audio_at).
    sent: Option<usize>,
    /// The export's thunder: strikes the playhead crosses (weather_audio::replay).
    thunder: crate::weather_audio::replay::ReplayThunder,
    frames: Frames<Image>,
    /// This export's captures as they land, by frame: its own, so one landing after an earlier export ended can't
    /// pass for one of this one's frames (bug 10-16 #32).
    captured: Arc<Mutex<Vec<(u32, Image)>>>,
    /// The boot text the progress popup shows, until the first frame.
    shown: Option<&'static str>,
    /// The percentage the progress popup shows.
    percent: Option<u32>,
    /// Where the playhead was before the export (review M10).
    playhead: f32,
    /// Review M4: no frame was asked this update (the encoder is behind, every frame is out, or the export is
    /// booting): the frame pacer caps the loop, as it does a menu, so the encoder thread gets the CPU.
    waiting: bool,
}

/// Frames asked of the export camera and not yet in the file. Render speed (10-27): one at a time waited out the
/// capture's few frames of readback for every frame; a few more cover the encoder's queue. At 4K each is 33 MB.
const AHEAD: u32 = 6;

/// The export's frames between its camera and the encoder: up to [`AHEAD`] asked for, one per update, each going
/// on in frame order once captured.
struct Frames<T> {
    /// The next frame to ask for.
    next: u32,
    /// Asked for and not captured yet, with when.
    asked: std::collections::BTreeMap<u32, Instant>,
    /// Captured, waiting their turn (and their sound; owner 10-18: never a timeout).
    landed: std::collections::BTreeMap<u32, T>,
}
impl<T> Default for Frames<T> {
    fn default() -> Self {
        Self { next: 0, asked: Default::default(), landed: Default::default() }
    }
}
impl<T> Frames<T> {
    /// The frame to ask for now: while frames remain and fewer than [`AHEAD`] are past the `written` ones.
    fn ask(&mut self, total: u32, written: u32, now: Instant) -> Option<u32> {
        let frame = self.next;
        (frame < total && frame - written < AHEAD).then(|| {
            self.asked.insert(frame, now);
            self.next += 1;
            frame
        })
    }
    /// A capture, kept only if it was asked for.
    fn land(&mut self, frame: u32, image: T) {
        if self.asked.remove(&frame).is_some() {
            self.landed.insert(frame, image);
        }
    }
    fn has(&self, frame: u32) -> bool {
        self.landed.contains_key(&frame)
    }
    fn take(&mut self, frame: u32) -> Option<T> {
        self.landed.remove(&frame)
    }
    /// The longest-asked frame still out has been out over `timeout`.
    fn overdue(&self, now: Instant, timeout: Duration) -> bool {
        self.asked.values().next().is_some_and(|asked| now.saturating_duration_since(*asked) > timeout)
    }
}

/// The export's own camera: a copy of the gameplay camera rendering into an image at the export size (review I1).
#[derive(Component)]
pub(crate) struct ExportCamera(Handle<Image>);

/// Why a save can't be loaded: retail's load-failure popup (Review Focus 2). Every [`file::LoadError`] is one.
pub(crate) fn load_failure_text(_: &file::LoadError) -> &'static str {
    "ID_MEMCARD_REPLAY_LOAD_FAILED"
}

#[derive(Resource, Default)]
pub(crate) struct ReplayHud {
    pub editor: Option<Editor>,
    /// The pause menu's Replay row: open the editor (true when the session is online: state 20).
    pub request: Option<bool>,
    /// Create > skate.Reel: open the load list.
    pub open_list: bool,
    popup: Option<(Ask, popup_runtime::Runtime)>,
    accumulated: f32,
    popup_accumulated: f32,
    previous: u16,
    /// The triggers (L, R) and right-stick flicks (left, right, up, down) past their press thresholds last update.
    edges: [bool; 6],
    /// Item 16: the help movie's page (0-based; the extra Video Tools page after retail's), None until it opens.
    help_page: Option<u32>,
    /// The camera edit's zoom readout (lens, roll, ease) last sent with state 11 (UpdateZoom).
    sent_zoom: Option<(f32, f32, Ease)>,
    /// Take Photo: frames until the capture, the screens hidden meanwhile.
    photo: Option<u8>,
    stashed: Option<std::collections::BTreeMap<String, Screen>>,
    sizes: Option<[[u32; 2]; 4]>,
    /// The state-dependent movie states last sent (over a marker, the marks), to send only changes.
    sent: (Option<u32>, f32, (f32, f32)),
    pending_load: Option<Loaded>,
    export: Option<Exporting>,
    /// skate.Reel > My Replays, while it is up (review I3).
    reel: Option<Reel>,
    /// Take Photo's write: Ok once the PNG is on disk (review M5: the notice follows the write).
    photo_saved: Arc<Mutex<Option<Result<(), String>>>>,
    /// An online match's quick save ([`quick_save`]): true while it is written, then its result for the event log.
    quick_saving: bool,
    quick_saved: Arc<Mutex<Option<Result<PathBuf, file::SaveError>>>>,
}

impl ReplayHud {
    pub fn open(&self) -> bool {
        self.editor.is_some()
    }
    /// The editor, its popup or the reel owns the keyboard (online_chat).
    pub(crate) fn busy(&self) -> bool {
        self.editor.is_some() || self.popup.is_some() || self.reel.is_some()
    }
    /// An export is rendering: the export camera draws the world (camera.rs) and the frame pacer stands aside.
    pub(crate) fn exporting(&self) -> bool {
        self.export.is_some()
    }
    /// While an export runs: whether this update asked it a frame (false: waiting on its encoder; review M4).
    pub(crate) fn export_rendering(&self) -> Option<bool> {
        self.export.as_ref().map(|e| !e.waiting)
    }
}

/// The help movie's mode for the editor state it was opened over (replay_help2 kInstantReplayMode 0 ..
/// kAdvancedFreeCameraEditMode 9; GetHelpMode 0x82613fe8).
fn help_mode(movie_state: u32) -> u32 {
    match movie_state {
        9 => 1,
        4 => 2,
        1 => 3,
        10 => 4,
        2 => 5,
        7 => 6,
        12 => 7,
        13 => 8,
        14 => 9,
        _ => 0,
    }
}

/// replay_help2's mNumPages: 2 for help modes 0, 1, 2, 7, 8, 9; 1 for 3..6.
fn retail_help_pages(mode: u32) -> u32 {
    if (3..=6).contains(&mode) { 1 } else { 2 }
}

/// Item 16: the extra help page's text, the extensions the mode offers, in the active pad's button names (`ps`).
/// Bug 10-17 #31: the right stick's key tools are Main Edit's alone (Editor::key `main`); in Instant Replay the right
/// stick orbits the camera.
fn extra_help(mode: u32, ps: bool) -> String {
    let text = match mode {
        0 => "Replay Settings: Letterbox, Camera Path",
        1 => "Right Stick Left / Right: Previous / Next Key^Right Stick Up: Copy Key^Right Stick Down: Paste Key\
              ^Replay Settings: Letterbox, Camera Path",
        2 => "Y: Key Ease (Hold / Linear / Smooth)^Speed keys with an ease ramp the speed",
        7..=9 => "LB / RB (hold): Roll^R3: Reset Roll^Y: Key Ease (Hold / Linear / Smooth)^X: Copy Camera^Back: Paste Camera",
        _ => "LB / RB (hold): Roll^R3: Reset Roll^X: Copy Camera^Back: Paste Camera",
    };
    // The PlayStation buttons in the same places (button_icons' ps set draws Back as Create).
    // Plain text, not a literal: its `^` break lines as retail's help pages' do (a literal's stay characters).
    if ps {
        [("LB / RB", "L1 / R1"), ("Y:", "Triangle:"), ("X:", "Square:"), ("Back:", "Create:")].iter().fold(text.to_owned(), |t, (a, b)| t.replace(a, b))
    } else {
        text.to_owned()
    }
}

/// A single-page help gets retail's "A Next" item (UpdateHelpTitle adds it only for two pages) to reach the extra page.
fn add_next(help: &mut Screen) -> Result<(), String> {
    help.call_at("mHelp.panelHelp", "AddButtonItem", &[json!("A"), json!("ID_COMMON_NEXT")])?;
    help.call_at("mHelp.panelHelp", "RefreshButtonHelp", &[]).map(drop)
}

/// A in the help: retail's pages (SetState(0) = NextHelpPage), then the extra Video Tools page, then back to the first.
/// Returns the page now shown.
fn next_help_page(help: &mut Screen, mode: u32, page: u32) -> Result<u32, String> {
    let pages = retail_help_pages(mode);
    let next = (page + 1) % (pages + 1);
    if next == pages {
        help.call_at("mHelp.panelHelp", "SetHeader", &[json!(crate::apt_text::literal("VIDEO TOOLS")), json!("Small")])?;
        let field = help.clip("mHelp.mContent.mTextHelp.labelText").ok_or("no mTextHelp")?;
        help.vm.set(field, "text", crate::apt_vm::Value::Text(extra_help(mode, crate::apt_scene::PS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed))))?;
    } else if pages > 1 {
        help.call("SetState", &[json!(0)])?;
    } else {
        help.call("UpdateHelpTitle", &[])?;
        add_next(help)?;
    }
    Ok(next)
}

fn mm_ss(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Every `FEReplayMenu` native RE3's ActionScript calls (build-progress §9), answered from the editor.
/// `seconds` is the recording's length; `localize` reads the movie's string table.
pub(crate) fn answers(editor: &Editor, seconds: f32, localize: &dyn Fn(&str) -> String) -> Vec<(&'static str, Json)> {
    let t = editor.playhead;
    let channel_value = |c: Channel| if editor.channel == c { editor.pending[0] } else { editor.int_at(c, t) };
    let tint = if editor.channel == Channel::Tint { editor.pending } else { editor.tint_at(t) };
    let encoded = editor.encoded_seconds(seconds);
    let shot = match editor.state {
        State::Camera(_) => editor.shot,
        _ => editor.orbit.unwrap_or_else(|| editor.camera_at(t)),
    };
    let mut zoom = localize("ID_REPLAYEDITOR_ZOOM").replace("{0:d}", &(shot.lens as i32).to_string());
    // Item 16 extensions on the same readout: the roll, and an Adv key's ease when it is not retail's Smooth.
    if shot.roll != 0. {
        zoom += &format!("  Roll {}\u{b0}", shot.roll.round() as i32);
    }
    if matches!(editor.state, State::Camera(kind) if kind.advanced()) && editor.pending_ease != Ease::retail(Channel::Camera) {
        zoom += &format!("  {}", editor.pending_ease.name());
    }
    let mode_value = match editor.channel {
        Channel::Brightness | Channel::Contrast | Channel::Saturation => editor.pending[0],
        _ => -1,
    };
    vec![
        ("FEReplayMenu.GetCursorPosition", json!(t)),
        ("FEReplayMenu.GetPlayheadPosition", json!(t)),
        ("FEReplayMenu.GetStartHeadPosition", json!(editor.in_mark)),
        ("FEReplayMenu.GetEndHeadPosition", json!(editor.out_mark)),
        // 0x825add90: the 30 fps encode timeline.
        ("FEReplayMenu.GetFrameCount", json!((seconds * 30. + 0.0005) as i64)),
        ("FEReplayMenu.GetEditIndex", json!(editor.edit_index())),
        ("FEReplayMenu.GetSimpleSpeedSetting", json!(channel_value(Channel::Speed))),
        ("FEReplayMenu.IsFilmDLCInstalled", json!(true)),
        ("FEReplayMenu.GetWaterMarkStatus", json!(0)),
        ("FEReplayMenu.GetShakeCamType", json!(channel_value(Channel::Shake))),
        ("FEReplayMenu.GetCameraZoomString", json!(zoom)),
        // 0x825adf00: the modes that offer L3.
        ("FEReplayMenu.CanDisplayShowHide", json!(matches!(editor.state, State::Scrub | State::MainEdit | State::OnlineInstant))),
        ("FEReplayMenu.GetCurrentCameraEditIndex", json!(editor.camera_index)),
        ("FEReplayMenu.GetEncodedLength", json!(localize("ID_REPLAY_EDITOR_LENGTH").replace("{0:time}", &mm_ss(encoded)))),
        ("FEReplayMenu.IsEncodedLengthValid", json!(encoded <= MAX_ENCODED_SECONDS)),
        // st+130, the instant-replay flag: Instant Replay and online state 20 (review I5: false sends the movie's
        // InstantDone to kMainEditMode).
        ("FEReplayMenu.IsInstantReplayMode", json!(matches!(editor.movie_state(), 0 | 20))),
        ("FEReplayMenu.UpdateReplayEditorReady", Json::Null),
        ("FEReplayMenu.RefreshCursor", Json::Null),
        ("FEReplayMenu.Ready", Json::Null),
        ("FEReplayMenu.GetModeValue", json!(mode_value)),
        ("FEReplayMenu.GetModeSliderValue", json!(editor.slider_value())),
        ("FEReplayMenu.GetCurrentTint", json!(editor.tint_channel)),
        ("FEReplayMenu.GetCurrentTintValue", json!(tint[editor.tint_channel])),
        ("FEReplayMenu.GetCurrentSliderTintValue", json!(tint[editor.tint_channel] / 25)),
        ("FEReplayMenu.GetRedTintValue", json!(tint[0] / 25)),
        ("FEReplayMenu.GetGreenTintValue", json!(tint[1] / 25)),
        ("FEReplayMenu.GetBlueTintValue", json!(tint[2] / 25)),
        ("FEReplayMenu.GetAlphaTintValue", json!(tint[3] / 25)),
        ("FEReplayMenu.GetModeText", json!(editor.mode_text())),
        ("FEReplayMenu.GetMode", json!(editor.channel.index())),
        // RULING (build-progress, Task 9): no sign-in on PC, so Take Photo is always offered.
        ("FEReplayMenu.CanSaveMedia", json!(true)),
        ("FEReplayMenu.GetHelpMode", json!(help_mode(editor.movie_state()))),
        ("FEReplayMenu.IsOverMarker", json!(editor.marker_under().is_some() || editor.key_under().is_some())),
        ("FEReplayMenu.AnySoftMarkersInClip", json!(editor.any_soft_markers_in_clip())),
        ("Game.IsDemo", json!(false)),
    ]
}

fn answer_all(screen: &mut Screen, editor: &Editor, seconds: f32) {
    let text = &screen.bindings.movie.text_assets;
    let strings = ["ID_REPLAYEDITOR_ZOOM", "ID_REPLAY_EDITOR_LENGTH"].map(|id| (id, text.localize(id)));
    let localize = |id: &str| strings.iter().find(|(k, _)| *k == id).map_or_else(|| id.to_owned(), |(_, v)| v.clone());
    for (key, value) in answers(editor, seconds, &localize) {
        screen.answer(key, value);
    }
}

/// `SetState` on the editor controller (`ScreenManager.OnLoaded` registers it; `_global.ReplayEditor` otherwise).
fn set_state(screen: &mut Screen, state: u32) -> Result<(), String> {
    let args = [json!(state)];
    if screen.has("SetState") { screen.call("SetState", &args) } else { screen.call_at("_global.ReplayEditor", "SetState", &args) }
        .map(drop)
}

/// The tags screen as the export settings (plan Task 10): Style → Resolution, Type → Frame Rate, HD.
fn tags_answers() -> Vec<(String, Json)> {
    let mut out = vec![
        ("Tags.GetMode".into(), json!(0)),
        ("Tags.IsFilmerPackInstalled".into(), json!(true)),
        ("Tags.GetIsDemoPublic".into(), json!(false)),
    ];
    let rows: [(&str, Vec<String>); 3] = [
        ("Resolution", export::RESOLUTIONS.iter().map(|(_, h)| format!("{h}p")).collect()),
        ("Frame Rate", export::RATES.iter().map(|r| format!("{r} fps")).collect()),
        ("HD", vec!["Off".into(), "On".into()]),
    ];
    for (category, (name, tags)) in rows.iter().enumerate() {
        out.push((format!("Tags.GetCategoryHALName({category})"), json!(name)));
        out.push((format!("Tags.GetNumTagsInCategory({category})"), json!(tags.len())));
        for (i, tag) in tags.iter().enumerate() {
            out.push((format!("Tags.GetTagHALName({category},{i})"), json!(tag)));
        }
    }
    out
}

/// The map a saved replay needs loaded first; None when it is the current one.
pub(crate) fn needs_map(header: &file::Header, current: Option<&Path>) -> Option<crate::map_library::Entry> {
    let wanted = header.map_path.as_ref().map(PathBuf::from);
    (wanted.as_deref() != current).then(|| crate::map_library::Entry { label: header.map.clone(), path: wanted })
}

pub(crate) struct ReplayHudPlugin;
impl Plugin for ReplayHudPlugin {
    fn build(&self, app: &mut App) {
        grade::install(app);
        app.init_resource::<ReplayHud>()
            .add_systems(
                PreUpdate,
                (
                    // Before the crossbar (it must not see the editor's presses), so it reads the pad one poll
                    // late: core_menu -> MenuInput -> map_transition -> poll_controllers forbids running after it.
                    interact
                        .after(bevy::input::InputSystems)
                        .after(crate::customiser::navigation)
                        .before(crate::core_menu::interact),
                    hold_pad.after(crate::input::poll_controllers),
                ),
            )
            // Bug 10-16 #32: before physics / animation / camera `present` sample the replay, so the frame drawn
            // (and an export's capture) shows the playhead set this frame, not the last one.
            .add_systems(Update, (load_pending, present.before(crate::apt_screen::render).before(crate::app::FrameSet::Physics)).chain())
            .add_systems(
                PostUpdate,
                (
                    look.after(super::play_back).before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
                    export_camera.before(bevy::transform::TransformSystems::Propagate),
                    camera_path.run_if(resource_exists::<GizmoConfigStore>),
                ),
            );
    }
}

/// The poll refills the pads after `interact`; while the replay owns the pad gameplay never sees them.
fn hold_pad(hud: Res<ReplayHud>, replay: Res<Replay>, mut input: ResMut<crate::input::ControllerInput>) {
    if replay.scripted || replay.release_controls || hud.editor.is_some() || hud.reel.is_some() {
        input.discard_gameplay();
    }
}

fn root(config: &Config) -> PathBuf {
    config.asset_root.join(FOLDER)
}

impl ReplayHud {
    fn ask(&mut self, root: &Path, ask: Ask, description: &str, options: &[&str], cancelable: bool) -> Result<(), String> {
        // perf 10-18: parsed once, not on every ask.
        let json = crate::apt_screen::kept_json(&root.join("runtime/sk8popup.json"))?;
        let mut runtime = popup_runtime::Runtime::load(&json, description, options, cancelable)?;
        // An engine string missing from an older table: its English.
        if let Some((id, english)) = ENGINE_TEXT.iter().find(|(id, _)| *id == description)
            && runtime.bindings.movie.text_assets.localize(id) == *id
        {
            runtime.set_description(english)?;
        }
        (self.popup, self.popup_accumulated) = (Some((ask, runtime)), 0.);
        Ok(())
    }
    fn notice(&mut self, root: &Path, text: &str) {
        if let Err(error) = self.ask(root, Ask::Notice, text, &["ID_COMMON_OK"], true) {
            warn!("Replay: {text} ({error})");
        }
    }
    /// The editor's popup / menu / settings; without the popup movie the choice is made at once.
    fn editor_popup(&mut self, root: &Path, out: &Out, seconds: f32) -> Vec<Out> {
        let opened = match out {
            Out::Popup(kind) => {
                let (body, options) = kind.text();
                self.ask(root, Ask::Editor, body, options, options.len() > 1)
            }
            Out::Menu => {
                // 0x82614f38: titled ID_REPLAYEDITOR_TITLE, its rows by mode and length (Editor::menu_rows).
                let rows = self.editor.as_ref().map(|e| e.menu_rows(seconds)).unwrap_or_default();
                let options: Vec<&str> = rows.iter().map(|m| m.text()).collect();
                self.ask(root, Ask::Menu(rows), "ID_REPLAYEDITOR_TITLE", &options, true)
            }
            _ => {
                // Retail's three rows, then the item 16 rows (Look::rows).
                let rows = self.editor.as_ref().map(|e| e.look.rows()).unwrap_or_default();
                let options: Vec<&str> = rows.iter().map(String::as_str).collect();
                self.ask(root, Ask::Settings, "ID_REPLAY_EDITOR_REPLAY_MODE", &options, true)
            }
        };
        let Err(error) = opened else { return Vec::new() };
        warn!("Replay popup unavailable ({error}); answering it at once");
        let Some(editor) = self.editor.as_mut() else { return Vec::new() };
        match out {
            Out::Popup(kind) => editor.answer(usize::from(kind.text().1.len() > 1)),
            Out::Menu => editor.pick(None),
            _ => editor.setting(None),
        }
    }
}

/// Opens the editor over the recording; false when nothing is recorded. Online (free skate: matches never open it) it
/// is the whole editor, not retail's state 20 (user 10-27), with the session running on under it (`replay.online`).
fn open(hud: &mut ReplayHud, replay: &mut Replay, layer: Option<&mut Layer>, root: &Path, online: bool, editor: Option<Editor>) -> bool {
    replay.enter();
    if !replay.active {
        return false;
    }
    info!("Replay Editor open ({})", if editor.is_some() { "a saved replay" } else { "the recording" });
    replay.online = online;
    let mut editor = editor.unwrap_or_else(|| Editor::new(false));
    if editor.legacy_follow {
        cameras::migrate_follow(&mut editor, |t| {
            replay.seek_fraction(t);
            replay.sample().map(|(a, b, x)| (blend(a.root, b.root, x), blend(a.camera, b.camera, x)))
        });
    }
    let seconds = replay.position().1 as f32;
    if let Some(layer) = layer {
        // Bug 10-16 #29: the movie's constructor reads GetModeText into mModeText (top left; Instant Replay never
        // rewrites it) and CanSaveMedia into mSignedIn, so they're answered at load; unanswered it showed "undefined".
        // Retail's GetModeText (TU3 0x825AE3A0) always returns a channel name.
        let opened = layer.open(EDITOR, "replay_editor3", &answers(&editor, seconds, &|id| id.to_owned())).and_then(|screen| {
            answer_all(screen, &editor, seconds);
            screen.play("intro").or(Ok::<(), String>(()))?;
            set_state(screen, editor.movie_state())
        });
        if let Err(error) = opened {
            // The editor still runs on the pad (prepare its screens with the launcher's replay-ui stage).
            error!("Replay Editor screen: {error}");
            layer.close(EDITOR);
        }
        if hud.sizes.is_none() {
            hud.sizes = Some(timeline::TEXTURES.map(|t| image::image_dimensions(root.join(t)).map_or([16, 30], |(w, h)| [w, h])));
        }
    }
    hud.sent = (None, -1., (-1., -1.));
    hud.accumulated = 0.;
    hud.editor = Some(editor);
    true
}

fn close(hud: &mut ReplayHud, replay: &mut Replay, layer: Option<&mut Layer>) {
    if let Some(export) = hud.export.take() {
        export.job.cancel();
    }
    if let Some(layer) = layer {
        for name in [EDITOR, HELP, TAGS] {
            layer.close(name);
        }
        layer.extra.remove("timeline");
        if let Some(stashed) = hud.stashed.take() {
            layer.open.extend(stashed);
        }
    }
    (hud.editor, hud.popup, hud.photo) = (None, None, None);
    if replay.active {
        replay.exit();
    }
}

/// The editor's requests: movie states, the help, popups, photo, save, upload.
/// The editor's FE sounds (Out::Sound) go to the sound queue; the rest is for `apply`.
fn sound_outs(outs: Vec<Out>, sounds: &mut MessageWriter<FrontEndSoundRequest>) -> Vec<Out> {
    outs.into_iter()
        .filter(|out| match out {
            Out::Sound(id) => {
                sounds.write(FrontEndSoundRequest(*id));
                false
            }
            _ => true,
        })
        .collect()
}

fn apply(
    hud: &mut ReplayHud,
    replay: &mut Replay,
    mut layer: Option<&mut Layer>,
    root: &Path,
    outs: Vec<Out>,
    map: (&str, Option<&Path>),
) {
    let seconds = replay.position().1 as f32;
    if let (Some(editor), Some(screen)) = (hud.editor.as_ref(), layer.as_deref_mut().and_then(|l| l.get(EDITOR))) {
        answer_all(screen, editor, seconds);
    }
    let mut queue: std::collections::VecDeque<Out> = outs.into();
    while let Some(out) = queue.pop_front() {
        match out {
            Out::State(state) => {
                if let Some(screen) = layer.as_deref_mut().and_then(|l| l.get(EDITOR))
                    && let Err(error) = set_state(screen, state)
                {
                    warn!("Replay Editor SetState({state}): {error}");
                }
            }
            Out::Help(true) => {
                let mode = hud.editor.as_ref().map_or(0, |e| help_mode(e.movie_state()));
                if let Some(layer) = layer.as_deref_mut()
                    && let Err(error) = layer.open(HELP, "replay_help2", &[("FEReplayMenu.GetHelpMode", json!(mode))]).and_then(|s| s.play("intro").or(Ok(())))
                {
                    warn!("Replay help: {error}");
                    layer.close(HELP);
                }
            }
            Out::Help(false) => {
                hud.help_page = None;
                if let Some(layer) = layer.as_deref_mut() {
                    layer.close(HELP);
                }
            }
            // Written to the sound queue by the callers (sound_outs).
            Out::Sound(_) => {}
            // RefreshMode's SetHeader(GetModeText(), "Small") again, for the key ease.
            Out::Header => {
                if let (Some(editor), Some(screen)) = (hud.editor.as_ref(), layer.as_deref_mut().and_then(|l| l.get(EDITOR)))
                    && let Err(error) = screen.call_at("mReplay.panelEdit", "SetHeader", &[json!(editor.mode_text()), json!("Small")])
                {
                    warn!("Replay Editor header: {error}");
                }
            }
            Out::Popup(_) | Out::Menu | Out::Settings => queue.extend(hud.editor_popup(root, &out, seconds)),
            Out::ClosePopup => hud.popup = None,
            Out::Exit => {
                close(hud, replay, layer.as_deref_mut());
                return;
            }
            Out::Photo => hud.photo = Some(2),
            Out::SaveReplay => {
                let Some(editor) = hud.editor.as_ref() else { continue };
                let header = file::Header::new(map.0, map.1, replay.frames());
                let text = match file::folder(Folder::Documents).ok_or_else(|| file::SaveError::Io("no Documents folder".into())).and_then(|dir| file::save(&dir, &header, replay.frames(), editor)) {
                    Ok(path) => {
                        info!("Replay saved: {}", path.display());
                        "ID_REPLAY_EDITOR_SAVE_REPLAY_COMPLETE".to_owned()
                    }
                    Err(file::SaveError::SlotsFull) => "ID_MEMCARD_MAX_SLOTS_REACHED_REPLAY".to_owned(),
                    Err(file::SaveError::Io(error)) => {
                        error!("Replay save: {error}");
                        "ID_SAVE_FAILED_OPTIONS_DESC".to_owned()
                    }
                };
                hud.notice(root, &text);
            }
            Out::Upload => {
                let Some(editor) = hud.editor.as_ref() else { continue };
                if let Err(text) = export::check(editor, seconds) {
                    hud.notice(root, text);
                    continue;
                }
                // Review C1: Media Foundation is loaded at run time; without it the export says why.
                if !mf::available() {
                    hud.notice(root, "ID_S3R_MEDIA_FEATURE_PACK");
                    continue;
                }
                let opened = layer.as_deref_mut().ok_or_else(|| "no screens".to_string()).and_then(|layer| {
                    let answers = tags_answers();
                    let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                    let tags = layer.open(TAGS, "skatereel_tags", &borrowed)?;
                    if let Err(error) = render_wording(tags) {
                        warn!("Replay export screen wording: {error}");
                    }
                    tags.play("intro").or(Ok(()))
                });
                if let Err(error) = opened {
                    error!("Replay export screen: {error}");
                    hud.notice(root, RENDER_FAILED);
                }
            }
        }
    }
}

/// Not retail, by request (user 10-27): the export screen's title, start row and help read as rendering a video
/// (its constructor's InitSkateReelTags set "Tag Your Footage", "Upload Footage" and the tags help).
fn render_wording(tags: &mut Screen) -> Result<(), String> {
    tags.call_at("mSkateReelTags.panel", "SetHeader", &[json!(super::editor::RENDER_VIDEO), json!("Small")])?;
    tags.call_at("mSkateReelTags.mContent.mSelector5", "SetTitle", &[json!(super::editor::RENDER_VIDEO)])?;
    let help = tags.clip("mSkateReelTags.mContent.txtHelp").ok_or("no tags help field")?;
    tags.vm.set(help, "text", crate::apt_vm::Value::Text(RENDER_HELP.into()))
}

/// The progress popup's text: "Generating Footage 42 %" (ID_REPLAY_EDITOR_GENERATING_FOOTAGE, ID_REPLAY_EDITOR_PERCENTAGE).
fn progress_text(runtime: &popup_runtime::Runtime, percent: u32) -> String {
    let percentage = runtime.bindings.movie.text_assets.localize("ID_REPLAY_EDITOR_PERCENTAGE").replace("{0:d}", &percent.to_string());
    format!("#ID_REPLAY_EDITOR_GENERATING_FOOTAGE {percentage}")
}

/// Ends the export: the playhead goes back (review M10); a failure cancels (the partial file goes) and says
/// `text`, a finish says where the file is.
fn end_export(hud: &mut ReplayHud, root: &Path, failure: Option<&str>) {
    let Some(export) = hud.export.take() else { return };
    if let Some(editor) = hud.editor.as_mut() {
        editor.playhead = export.playhead;
    }
    hud.popup = None;
    match failure {
        Some(text) => {
            export.job.cancel();
            hud.notice(root, text);
        }
        None => {
            info!("Replay exported: {}", export.job.path.display());
            hud.notice(root, &format!("{RENDER_COMPLETE} {}", export.job.path.display()));
        }
    }
}

/// Starts the export the tags screen asked for (`Tags.SetTags(resolution, rate, hd, watermark)`).
fn start_export(hud: &mut ReplayHud, replay: &Replay, root: &Path, assets: &Path, tags: &[Json], map: &str) {
    let Some(editor) = hud.editor.as_ref() else { return };
    let tag = |i: usize| tags.get(i).and_then(Json::as_u64).unwrap_or(0) as usize;
    let settings = export::ExportSettings::from_tags(tag(0), tag(1), tag(2), tag(3));
    let watermark = settings.watermark.texture().and_then(|t| image::open(root.join(t)).ok()).map(|i| i.to_rgba8());
    let name = format!("{}-{}", file::safe(map), file::stamp(file::unix_now()));
    let seconds = replay.position().1 as f32;
    let started = file::folder(Folder::Videos)
        .ok_or_else(|| "no Videos folder".to_string())
        .and_then(|dir| export::Job::start(&dir, &name, settings, editor, seconds, watermark));
    match started {
        Ok(job) => {
            info!("Replay export: {} ({}x{} {} fps, {} frames)", job.path.display(), settings.width, settings.height, settings.fps, job.frames);
            let sound = export::Sound::start(assets.to_owned(), settings.fps);
            let playhead = editor.playhead;
            hud.export = Some(Exporting {
                job,
                sound,
                sent: None,
                thunder: Default::default(),
                frames: Frames::default(),
                captured: Default::default(),
                shown: None,
                percent: None,
                playhead,
                waiting: false,
            });
            if let Some(editor) = hud.editor.as_mut() {
                editor.playing = false;
                // Bug 10-16 #32: an export is playback, so the Instant Replay stick orbit goes, as RB's play drops it.
                editor.orbit = None;
            }
            if let Err(error) = hud.ask(root, Ask::Progress, "ID_REPLAY_EDITOR_GENERATING_FOOTAGE", &["ID_COMMON_CANCEL"], true) {
                warn!("Replay export progress: {error}");
            }
        }
        Err(error) => {
            error!("Replay export: {error}");
            hud.notice(root, RENDER_FAILED);
        }
    }
}

/// Not retail, by request (user 10-27: clip a moment in an online game mode, "quickly go in replay to clip something",
/// without waiting on a render or leaving the match): Back in an online match saves the replay as it stands (its last
/// 90 s, every skater) to My Replays, written off the main thread; the event log says when it is done. It edits and
/// renders later from skate.Reel. Retail has no replay in matches; a background render was not taken: it needs the
/// world drawn at the replay's moment, the very world the match is playing in.
fn quick_save(hud: &mut ReplayHud, replay: &Replay, map: (&str, Option<&Path>), dir: Option<PathBuf>) {
    if hud.quick_saving || replay.active || replay.frames().len() < super::MIN_BACK_FRAMES {
        return;
    }
    let Some(dir) = dir else {
        warn!("Replay quick save: no Documents folder");
        return;
    };
    let header = file::Header::new(map.0, map.1, replay.frames());
    let bytes = file::encode(&header, replay.frames(), &Editor::new(false));
    let slot = hud.quick_saved.clone();
    let spawned = std::thread::Builder::new().name("replay-quick-save".into()).spawn(move || {
        let saved = file::save_encoded(&dir, &header, &bytes);
        if let Ok(mut slot) = slot.lock() {
            *slot = Some(saved);
        }
    });
    match spawned {
        Ok(_) => hud.quick_saving = true,
        Err(error) => warn!("Replay quick save: {error}"),
    }
}

/// The quick save's end, as an event log line (never a popup: the match runs on).
fn quick_saved(saved: Result<PathBuf, file::SaveError>) -> &'static str {
    match saved {
        Ok(path) => {
            info!("Replay saved: {}", path.display());
            "Replay saved to My Replays"
        }
        Err(file::SaveError::SlotsFull) => "Replay not saved: My Replays is full",
        Err(file::SaveError::Io(error)) => {
            error!("Replay quick save: {error}");
            "The replay could not be saved"
        }
    }
}

/// Retail's blocked modes (Stage 0 §5: the Back gate wants free skate; spec §2): a tutorial lesson or Skate 1 / 2
/// challenge, a match (Skate 3's modes), Party Play. Offline only: online the replay is state 20 over the session.
fn blocked(lesson: Option<&crate::challenge::LessonMenu>, matched: Option<&crate::match_core::Match>, party: Option<&crate::party_play::PartyPlay>) -> bool {
    lesson.is_some_and(|l| l.active) || matched.is_some_and(|m| m.mode.is_some()) || party.is_some_and(|p| p.active)
}

/// ID_REPLAY_QUIT_CONF's Yes: every running mode ends (its own abandon path), then the editor opens.
fn end_modes(
    lesson: Option<&mut crate::challenge::LessonMenu>,
    matched: Option<&mut crate::match_core::Match>,
    party: Option<&mut crate::party_play::PartyPlay>,
) {
    if let Some(lesson) = lesson.filter(|l| l.active) {
        lesson.request = Some(crate::challenge::LessonRequest::Abandon);
    }
    if let Some(m) = matched.filter(|m| m.mode.is_some()) {
        crate::match_core::abandon(m);
    }
    if let Some(party) = party.filter(|p| p.active) {
        party.confirmed = Some(crate::party_play::Ask::Exit);
    }
}

/// Opens skate.Reel > My Replays: the browser movie waits while the headers are read off the main thread.
fn open_reel(hud: &mut ReplayHud, layer: Option<&mut Layer>, root: &Path) {
    let reel = Reel::start(file::folder(Folder::Documents));
    let Some(layer) = layer else {
        warn!("Replay footage screen: no screen layers");
        return;
    };
    match layer.open(BROWSER, "browser", &[]).and_then(|screen| reel.boot(screen)) {
        Ok(()) => {
            info!("Replay footage screen open");
            hud.reel = Some(reel);
        }
        Err(error) => {
            error!("Replay footage screen: {error}");
            layer.close(BROWSER);
            hud.notice(root, "ID_SKATEREEL_BROWSER_NO_FOOTAGE");
        }
    }
}

fn close_reel(hud: &mut ReplayHud, layer: Option<&mut Layer>) {
    hud.reel = None;
    if let Some(layer) = layer {
        layer.close(BROWSER);
    }
}

/// A save picked on skate.Reel: loaded (its map asked for in load_pending), or the damaged-save popup.
fn load_save(hud: &mut ReplayHud, root: &Path, path: &Path) -> bool {
    match file::load(path) {
        Ok((header, ring, editor)) => {
            info!("Replay load: {}", path.display());
            hud.pending_load = Some(Loaded { header, ring, editor, since: Instant::now(), requested: false });
            true
        }
        Err(error) => {
            // Review Focus 2: retail's load-failure popup (delete the damaged save?); nothing half-loads.
            warn!("Replay load: {error:?}");
            if let Err(error) = hud.ask(root, Ask::DeleteDamaged(path.to_owned()), load_failure_text(&error), &["ID_COMMON_NO", "ID_COMMON_YES"], true) {
                warn!("Replay load popup: {error}");
            }
            false
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn interact(
    mut hud: ResMut<ReplayHud>,
    mut replay: ResMut<Replay>,
    mut input: ResMut<crate::input::ControllerInput>,
    (mut keys, mut nav): (ResMut<ButtonInput<KeyCode>>, ResMut<crate::customiser::Navigation>),
    (menu, net, boot, vehicles): (
        Option<Res<crate::graphics_menu::Menu>>,
        Option<Res<crate::multiplayer::Multiplayer>>,
        Option<Res<crate::boot::BootActive>>,
        Option<Res<crate::modding::vehicles::Vehicles>>,
    ),
    (config, current, transition): (
        Res<Config>,
        Option<Res<crate::map_transition::CurrentMap>>,
        Option<Res<crate::map_transition::MapTransition>>,
    ),
    mut screens: Option<ResMut<Screens>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    (mut lesson, mut matched, mut party): (
        Option<ResMut<crate::challenge::LessonMenu>>,
        Option<ResMut<crate::match_core::Match>>,
        Option<ResMut<crate::party_play::PartyPlay>>,
    ),
) {
    let hud = &mut *hud;
    let online = net.is_some_and(|n| n.active());
    let pad = input.raw_input();
    let pressed = pad.buttons & !hud.previous;
    let released = hud.previous & !pad.buttons;
    hud.previous = pad.buttons;
    let now = [pad.triggers[0] > 0.5, pad.triggers[1] > 0.5, pad.right[0] < -0.6, pad.right[0] > 0.6, pad.right[1] > 0.6, pad.right[1] < -0.6];
    let edge: [bool; 6] = std::array::from_fn(|i| now[i] && !hud.edges[i]);
    hud.edges = now;
    // Back opens the editor from gameplay; only an editor already open reads it as a key.
    let was_open = hud.editor.is_some();
    if boot.is_some_and(|b| b.0) {
        return;
    }
    // Review Focus 1: a scripted replay (the Skate 2 highlight reel) shows no editor and owns the pad.
    if replay.scripted {
        input.discard_gameplay();
        return;
    }
    let root = root(&config);
    let map_name = current.as_ref().map_or_else(String::new, |c| c.name.clone());
    let map_path = current.as_ref().and_then(|c| c.path.clone());
    let map = (map_name.as_str(), map_path.as_deref());
    let mut layer = screens.as_deref_mut().map(|s| s.layer(REPLAY));
    // An online match running: retail has no replay there (no crossbar row, Back wants session type 22).
    let match_online = online && matched.as_deref().is_some_and(|m| m.mode.is_some());
    // The editor goes when the session it opened over goes (or comes), or a match starts under it (an export with it).
    // Review I3: or a map move (an online host's, any load), and the recording starts again on the new map: its
    // frames would play in the old map's places.
    let moving = transition.is_some_and(|t| t.busy());
    if hud.editor.is_some() && (!replay.active || replay.online != online || match_online || moving) {
        close(hud, &mut replay, layer.as_deref_mut());
    }
    if moving {
        replay.reset_recording();
    }
    let gameplay = crate::graphics_menu::gameplay_active(menu);
    let back = pressed & BACK != 0 || keys.just_pressed(KeyCode::F6);
    let mode_running = !online && blocked(lesson.as_deref(), matched.as_deref(), party.as_deref());
    if let Some(online_request) = hud.request.take() {
        if hud.editor.is_some() {
        } else if mode_running && !online_request {
            // Review I6: retail asks before leaving the mode (ID_REPLAY_QUIT_CONF).
            if let Err(error) = hud.ask(&root, Ask::QuitMode(false), "ID_REPLAY_QUIT_CONF", &["ID_COMMON_NO", "ID_COMMON_YES"], true) {
                warn!("Replay quit popup: {error}");
            }
        } else if !open(hud, &mut replay, layer.as_deref_mut(), &root, online_request || online, None) {
            info!("Replay: nothing recorded yet");
        }
    } else if hud.editor.is_none() && hud.popup.is_none() && hud.reel.is_none() && gameplay && back && match_online {
        quick_save(hud, &replay, map, file::folder(Folder::Documents));
    } else if hud.editor.is_none()
        && hud.popup.is_none()
        && hud.reel.is_none()
        && gameplay
        && back
        && !mode_running
        && !vehicles.as_ref().is_some_and(|v| v.occupied())
        && replay.can_open_from_back()
        && open(hud, &mut replay, layer.as_deref_mut(), &root, online, None)
    {
        // Gameplay's Back (825EB998, its second button) plays `editor_select` (sk8_menu) as it opens the editor.
        sounds.write(FrontEndSoundRequest(EDITOR_SELECT));
    }
    if std::mem::take(&mut hud.open_list) && hud.editor.is_none() && hud.reel.is_none() {
        open_reel(hud, layer.as_deref_mut(), &root);
    }
    if let Some(reel) = hud.reel.as_mut()
        && reel.poll()
        && let Some(screen) = layer.as_deref_mut().and_then(|l| l.get(BROWSER))
        && let Err(error) = reel.refresh(screen)
    {
        warn!("Replay footage screen: {error}");
    }

    // After the editor: hold gameplay input until everything is released (no stray flick or push).
    if hud.editor.is_some() || replay.release_controls {
        input.discard_gameplay();
        if hud.editor.is_none()
            && pad.buttons == 0
            && pad.triggers.iter().all(|v| *v < 0.05)
            && pad.left.iter().chain(&pad.right).all(|v| v.abs() < 0.18)
        {
            replay.release_controls = false;
        }
    }
    if hud.reel.is_some() && hud.editor.is_none() {
        input.discard_gameplay();
    }
    if hud.editor.is_none() && hud.popup.is_none() && hud.reel.is_none() {
        return;
    }
    // The editor owns Start and Back (no pause menu over it).
    let escape = keys.just_pressed(KeyCode::Escape);
    // Navigation folds in the left stick: retail's popups move on it as on the D-pad.
    let nav_pressed = std::mem::take(&mut nav.pressed);
    keys.clear_just_pressed(KeyCode::Escape);
    if hud.editor.is_some() && !gameplay {
        return;
    }

    let key = |bit: u16, code: KeyCode| pressed & bit != 0 || keys.just_pressed(code);
    // Retail's menu moves (AptUp/Down/Left/Right = stick || D-pad, with repeat): Navigation alone. The raw D-pad edge
    // as well moved twice, this poll and the next (user 10-27).
    let menu_key = |bit: u16, code: KeyCode| nav_pressed & bit != 0 || keys.just_pressed(code);
    // The popup first.
    if hud.popup.is_some() {
        let popup_keys = [
            (menu_key(UP, KeyCode::ArrowUp), Key::Up),
            (menu_key(DOWN, KeyCode::ArrowDown), Key::Down),
            (key(A, KeyCode::Enter), Key::Accept),
            (key(B, KeyCode::Backspace) || escape, Key::Back),
        ];
        let Some((ask, runtime)) = hud.popup.as_mut() else { return };
        let choice = popup_keys.into_iter().filter(|(down, _)| *down).find_map(|(_, k)| {
            let input = runtime.input(k);
            sounds.write_batch(runtime.bindings.sounds.drain(..).map(FrontEndSoundRequest));
            input.transpose()
        });
        let choice = match choice {
            None => return,
            Some(Ok(choice)) => choice,
            Some(Err(error)) => {
                error!("Replay popup stopped: {error}");
                popup_runtime::CANCEL
            }
        };
        let cancel = choice == popup_runtime::CANCEL;
        let ask = std::mem::replace(ask, Ask::Notice);
        hud.popup = None;
        let outs = match ask {
            Ask::Editor => hud.editor.as_mut().map(|e| e.answer(if cancel { 0 } else { choice })).unwrap_or_default(),
            Ask::Menu(rows) => hud.editor.as_mut().map(|e| e.pick(if cancel { None } else { rows.get(choice).copied() })).unwrap_or_default(),
            Ask::Settings => match hud.editor.as_mut() {
                Some(editor) => {
                    let outs = editor.setting(if cancel { None } else { Some(choice) });
                    // A toggle keeps the list up.
                    if editor.state == State::Settings { vec![Out::Settings] } else { outs }
                }
                None => Vec::new(),
            },
            Ask::Progress => {
                info!("Replay export cancelled");
                end_export(hud, &root, Some(RENDER_CANCELLED));
                Vec::new()
            }
            Ask::DeleteDamaged(path) | Ask::DeleteSave(path) if choice == 1 => {
                // Bug 10-17 #35: a save that couldn't be removed stays listed, and the player is told.
                if let Err(error) = file::delete(&path) {
                    warn!("Replay delete {}: {error}", path.display());
                    hud.notice(&root, "ID_S3R_REPLAY_DELETE_FAILED");
                } else if let Some(reel) = hud.reel.as_mut() {
                    reel.remove(&path);
                    if let Some(screen) = layer.as_deref_mut().and_then(|l| l.get(BROWSER))
                        && let Err(error) = reel.refresh(screen)
                    {
                        warn!("Replay footage screen: {error}");
                    }
                }
                Vec::new()
            }
            Ask::QuitMode(online_request) if choice == 1 => {
                end_modes(lesson.as_deref_mut(), matched.as_deref_mut(), party.as_deref_mut());
                if !open(hud, &mut replay, layer.as_deref_mut(), &root, online_request, None) {
                    info!("Replay: nothing recorded yet");
                }
                Vec::new()
            }
            _ => Vec::new(),
        };
        let outs = sound_outs(outs, &mut sounds);
        apply(hud, &mut replay, layer, &root, outs, map);
        return;
    }
    // skate.Reel > My Replays: left / right move, A loads, Y deletes, B leaves.
    if hud.editor.is_none()
        && let Some(reel) = hud.reel.as_mut()
    {
        let Some(screen) = layer.as_deref_mut().and_then(|l| l.get(BROWSER)) else {
            hud.reel = None;
            return;
        };
        let mut picked = Ok(None);
        let presses = reel.takes_presses();
        if menu_key(LEFT, KeyCode::ArrowLeft) || menu_key(RIGHT, KeyCode::ArrowRight) {
            picked = reel.step(screen, menu_key(RIGHT, KeyCode::ArrowRight)).map(|()| None);
        } else if presses && key(A, KeyCode::Enter) {
            picked = reel.press(screen, true);
        } else if presses && key(Y, KeyCode::KeyY) {
            picked = reel.press(screen, false);
        } else if key(B, KeyCode::Backspace) || escape {
            close_reel(hud, layer.as_deref_mut());
            return;
        }
        match picked {
            Ok(None) => {}
            Ok(Some(Pick::Delete(path))) => {
                if let Err(error) = hud.ask(&root, Ask::DeleteSave(path), "ID_MEMCARD_PROMPT_DELETE_REPLAY", &["ID_COMMON_NO", "ID_COMMON_YES"], true) {
                    warn!("Replay delete popup: {error}");
                }
            }
            Ok(Some(Pick::Select(path, header))) => {
                if online {
                    hud.notice(&root, "ID_S3R_REPLAY_OFFLINE_ONLY");
                } else if header.is_none() {
                    // A header that does not parse: the damaged-save popup at once.
                    let error = file::load(&path).err().unwrap_or(file::LoadError::Corrupt("header".into()));
                    if let Err(error) = hud.ask(&root, Ask::DeleteDamaged(path), load_failure_text(&error), &["ID_COMMON_NO", "ID_COMMON_YES"], true) {
                        warn!("Replay load popup: {error}");
                    }
                } else if load_save(hud, &root, &path) {
                    close_reel(hud, layer.as_deref_mut());
                }
            }
            Err(error) => warn!("Replay footage screen: {error}"),
        }
        return;
    }
    if hud.editor.is_none() {
        return;
    }

    // The tags screen (export settings): its own UpdateInput; B leaves it, A on the last row reports SetTags.
    if let Some(tags) = layer.as_deref_mut().and_then(|l| l.get(TAGS)) {
        for (down, k) in [
            (menu_key(UP, KeyCode::ArrowUp), Key::Up),
            (menu_key(DOWN, KeyCode::ArrowDown), Key::Down),
            (menu_key(LEFT, KeyCode::ArrowLeft), Key::Left),
            (menu_key(RIGHT, KeyCode::ArrowRight), Key::Right),
            (key(A, KeyCode::Enter), Key::Accept),
        ] {
            if down && let Err(error) = tags.input(k) {
                warn!("Replay export screen: {error}");
            }
        }
        let set = tags.take_calls().into_iter().find(|(call, _)| call == "Tags.SetTags").map(|(_, args)| args);
        let leave = key(B, KeyCode::Backspace) || escape;
        if set.is_some() || leave {
            if let Some(layer) = layer.as_deref_mut() {
                layer.close(TAGS);
            }
        }
        if let Some(args) = set {
            start_export(hud, &replay, &root, &config.asset_root, &args, map.0);
        }
        return;
    }
    if hud.export.is_some() {
        return;
    }
    let Some(editor) = hud.editor.as_mut() else { return };

    // Retail's FE keys (research Q3): Edit Timeline's D-pad acts on release (0x82612c28 codes 32..35); Instant
    // Replay's on press (0x82613988), as everything else.
    let timeline = editor.state == State::MainEdit;
    let dpad = if timeline { released & DPAD } else { pressed & DPAD };
    let held = |bit: u16, code: KeyCode| if timeline { dpad & bit != 0 || keys.just_released(code) } else { dpad & bit != 0 || keys.just_pressed(code) };
    let mut pads = Vec::new();
    for (down, p) in [
        (key(START, KeyCode::F1), Pad::Start),
        (key(A, KeyCode::Enter), Pad::A),
        (key(B, KeyCode::Backspace) || escape, Pad::B),
        (key(X, KeyCode::KeyX), Pad::X),
        (key(Y, KeyCode::KeyY), Pad::Y),
        (key(RB, KeyCode::Space), Pad::Rb),
        (key(L3, KeyCode::KeyH), Pad::L3),
        (held(UP, KeyCode::ArrowUp), Pad::Up),
        (held(DOWN, KeyCode::ArrowDown), Pad::Down),
        (held(LEFT, KeyCode::ArrowLeft), Pad::Left),
        (held(RIGHT, KeyCode::ArrowRight), Pad::Right),
        (back && was_open, Pad::Back),
        (key(LB, KeyCode::ShiftLeft), Pad::Lb),
        (key(R3, KeyCode::KeyR), Pad::R3),
        (edge[0] || edge[1] || keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::KeyE), Pad::Trigger),
        // Item 16: right-stick flicks (keyboard [ ] C V).
        (edge[2] || keys.just_pressed(KeyCode::BracketLeft), Pad::RsLeft),
        (edge[3] || keys.just_pressed(KeyCode::BracketRight), Pad::RsRight),
        (edge[4] || keys.just_pressed(KeyCode::KeyC), Pad::RsUp),
        (edge[5] || keys.just_pressed(KeyCode::KeyV), Pad::RsDown),
        // Releases after the presses (a tap inside one update still presses first).
        (released & A != 0 || keys.just_released(KeyCode::Enter), Pad::ARelease),
        (released & LB != 0 || keys.just_released(KeyCode::ShiftLeft), Pad::LbRelease),
    ] {
        if down {
            pads.push(p);
        }
    }
    let mut outs = Vec::new();
    for p in pads {
        // replay_help2: A pages (SetState(0) = NextHelpPage, then the extra page); B / Start close it through the
        // editor.
        if editor.state == State::Help && p == Pad::A {
            let mode = help_mode(editor.movie_state());
            if let Some(help) = layer.as_deref_mut().and_then(|l| l.get(HELP)) {
                match next_help_page(help, mode, hud.help_page.unwrap_or(0)) {
                    Ok(page) => hud.help_page = Some(page),
                    Err(error) => warn!("Replay help: {error}"),
                }
            }
            continue;
        }
        outs.extend(editor.key(p));
    }
    let outs = sound_outs(outs, &mut sounds);
    if !outs.is_empty() {
        apply(hud, &mut replay, layer, &root, outs, map);
    }
}

/// A replay chosen on skate.Reel, once its map is up. The map is asked for once; a transition that ends on another
/// map, none to ask, a timeout or an online session drops the load with a popup (review M7). The live skater's rig
/// must match the save's (review I4c: the recorded bones drive it by index).
#[allow(clippy::too_many_arguments)]
fn load_pending(
    mut hud: ResMut<ReplayHud>,
    mut replay: ResMut<Replay>,
    current: Option<Res<crate::map_transition::CurrentMap>>,
    mut transition: Option<ResMut<crate::map_transition::MapTransition>>,
    net: Option<Res<crate::multiplayer::Multiplayer>>,
    mut screens: Option<ResMut<Screens>>,
    config: Res<Config>,
    skater: Option<Res<crate::physics::SkaterRuntime>>,
) {
    let hud = &mut *hud;
    if hud.editor.is_some() || transition.as_ref().is_some_and(|t| t.busy()) {
        return;
    }
    let Some(loaded) = hud.pending_load.as_mut() else { return };
    let root = root(&config);
    if net.is_some_and(|n| n.active()) {
        hud.pending_load = None;
        hud.notice(&root, "ID_S3R_REPLAY_OFFLINE_ONLY");
        return;
    }
    let current = current.as_ref().and_then(|c| c.path.clone());
    if let Some(entry) = needs_map(&loaded.header, current.as_deref()) {
        match transition.as_mut() {
            Some(transition) if !loaded.requested && loaded.since.elapsed() < LOAD_TIMEOUT => {
                transition.request(entry);
                loaded.requested = true;
            }
            _ => {
                warn!("Replay load: the map {} did not come up", loaded.header.map);
                hud.pending_load = None;
                hud.notice(&root, "ID_S3R_REPLAY_MAP_UNAVAILABLE");
            }
        }
        return;
    }
    let Some(loaded) = hud.pending_load.take() else { return };
    // A frame recorded before the skater had a pose (0 bones) plays as the live pose; any other count must match.
    let live = skater.map(|s| s.render_pose.len()).filter(|&n| n > 0);
    if let Some(live) = live
        && loaded.ring.rigs().iter().any(|&bones| bones != 0 && bones != live)
    {
        warn!("Replay load: the save's skeleton ({:?} bones) is not the live skater's ({live})", loaded.ring.rigs());
        if let Err(error) = hud.ask(&root, Ask::Notice, load_failure_text(&file::LoadError::Corrupt("skeleton".into())), &["ID_COMMON_OK"], true) {
            warn!("Replay load popup: {error}");
        }
        return;
    }
    replay.install(loaded.ring);
    let layer = screens.as_deref_mut().map(|s| s.layer(REPLAY));
    open(hud, &mut replay, layer, &root, false, Some(loaded.editor));
}

#[allow(clippy::too_many_arguments)]
fn present(
    mut hud: ResMut<ReplayHud>,
    mut replay: ResMut<Replay>,
    input: Res<crate::input::ControllerInput>,
    keys: Res<ButtonInput<KeyCode>>,
    real: Res<Time<Real>>,
    mut screens: Option<ResMut<Screens>>,
    mut grade: ResMut<Grade>,
    mut commands: Commands,
    (config, current, exporting): (Res<Config>, Option<Res<crate::map_transition::CurrentMap>>, Query<&ExportCamera>),
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    mut roster: Option<ResMut<crate::online_roster::OnlineRoster>>,
) {
    let hud = &mut *hud;
    let root = root(&config);
    // Take Photo's write landed (review M5: the notice follows the write).
    let photo = hud.photo_saved.lock().ok().and_then(|mut p| p.take());
    match photo {
        Some(Ok(())) => hud.notice(&root, "ID_REPLAY_EDITOR_SAVE_PHOTO_COMPLETE"),
        Some(Err(error)) => {
            error!("Replay photo: {error}");
            hud.notice(&root, "ID_SAVE_FAILED_OPTIONS_DESC");
        }
        None => {}
    }
    // An online match's quick save written: one event log line.
    if let Some(saved) = hud.quick_saved.lock().ok().and_then(|mut q| q.take()) {
        hud.quick_saving = false;
        let line = quick_saved(saved);
        if let Some(roster) = roster.as_deref_mut() {
            roster.lines.push((line, String::new()));
        }
    }
    // Q11: every layer steps aside for the photo too (online: other players' name tags, event log and chat).
    if let Some(s) = screens.as_deref_mut() {
        s.photo = hud.photo.is_some();
    }
    let mut layer = screens.as_deref_mut().map(|s| s.layer(REPLAY));

    // The popup (also the load list, with no editor open).
    if let Some(layer) = layer.as_deref_mut() {
        match hud.popup.as_mut() {
            None => {
                layer.extra.remove("z-popup");
            }
            Some((_, runtime)) => {
                hud.popup_accumulated = (hud.popup_accumulated + real.delta_secs()).min(4. * FRAME);
                let mut result = Ok(());
                while hud.popup_accumulated >= FRAME && result.is_ok() {
                    hud.popup_accumulated -= FRAME;
                    result = runtime.advance();
                }
                sounds.write_batch(runtime.bindings.sounds.drain(..).map(FrontEndSoundRequest));
                match result.and_then(|()| crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes)) {
                    Ok(draws) => {
                        layer.extra.insert("z-popup", draws);
                    }
                    Err(error) => {
                        error!("Replay popup stopped: {error}");
                        hud.popup = None;
                        layer.extra.remove("z-popup");
                    }
                }
            }
        }
    }

    let Some(editor) = hud.editor.as_mut().filter(|_| replay.active) else {
        if grade.active {
            *grade = Grade::default();
        }
        if let Some(layer) = layer {
            layer.extra.remove("timeline");
        }
        return;
    };
    let seconds = replay.position().1 as f32;

    // The export steps the timeline itself: frame by frame, each rendered by the export camera and encoded with its
    // own sound. Render speed (10-27): a frame is asked for every update while earlier ones are still being read back
    // and encoded.
    if let Some(export) = hud.export.as_mut() {
        // Not retail, by request (owner 10-18): the sound never fails an export. Its worker boots for as long as it
        // takes (B still cancels); only a boot that can load no sound at all ends it, with its own text.
        let booted = export.sound.ready();
        let ready = booted.as_ref().is_some_and(Result::is_ok);
        let boot_text = match &booted {
            None if export.sound.stopping() => Some(EXPORT_STOPPING),
            None => Some(EXPORT_LOADING_SOUND),
            Some(_) => None,
        };
        if boot_text.is_some() && boot_text != export.shown
            && let Some((Ask::Progress, runtime)) = hud.popup.as_mut()
        {
            let _ = runtime.set_description(boot_text.unwrap_or_default());
            export.shown = boot_text;
        }
        let mut outcome = booted.and_then(Result::err).map(|error| Err((error, "ID_S3R_EXPORT_AUDIO_MISSING")));
        let landed = export.captured.lock().map(|mut c| std::mem::take(&mut *c)).unwrap_or_default();
        for (frame, image) in landed {
            export.frames.land(frame, image);
        }
        if outcome.is_none() && export.frames.overdue(Instant::now(), CAPTURE_TIMEOUT) {
            outcome = Some(Err(("the export frame never rendered".to_owned(), RENDER_FAILED)));
        }
        // To the encoder in frame order, each with its sound (the mix answers in the order asked), however long the
        // mix takes (no timeout; the main thread never blocks).
        while outcome.is_none() && export.frames.has(export.job.frame) {
            let Some(sound) = export.sound.chunk() else { break };
            let image = export.frames.take(export.job.frame).expect("checked above");
            let pushed = sound.and_then(|sound| {
                image.try_into_dynamic().map_err(|e| e.to_string()).and_then(|i| export.job.push(i.into_rgba8(), sound))
            });
            if let Err(error) = pushed {
                outcome = Some(Err((error, RENDER_FAILED)));
            }
        }
        match export.job.poll() {
            None => {}
            Some(Ok(())) => outcome = outcome.or(Some(Ok(()))),
            Some(Err(error)) => outcome = outcome.or(Some(Err((error, RENDER_FAILED)))),
        }
        let percent = export.job.progress();
        if export.job.frame > 0
            && export.percent != Some(percent)
            && let Some((Ask::Progress, runtime)) = hud.popup.as_mut()
        {
            let text = progress_text(runtime, percent);
            let _ = runtime.set_description(&text);
            export.percent = Some(percent);
        }
        // The next frame: its playhead, its sound asked for, its render captured from the export camera.
        let editor = hud.editor.as_mut().expect("checked above");
        export.waiting = true;
        if outcome.is_none()
            && ready
            && let Some(camera) = exporting.iter().next()
            && let Some(frame) = export.frames.ask(export.job.frames, export.job.written(), Instant::now())
        {
            export.waiting = false;
            editor.playhead = export.job.playhead(editor, frame);
            let (fps, seconds) = (export.job.settings.fps, export.job.timeline_seconds);
            let weather = export::weather_ticks(&replay, editor, frame, fps, seconds, &mut export.thunder);
            export.sound.request_with_weather(export::audio_records(&replay, editor, frame, fps, seconds, &mut export.sent), weather);
            let slot = export.captured.clone();
            commands.spawn(Screenshot::image(camera.0.clone())).observe(move |captured: On<ScreenshotCaptured>| {
                if let Ok(mut slot) = slot.lock() {
                    slot.push((frame, captured.image.clone()));
                }
            });
        }
        match outcome {
            None => {}
            Some(Ok(())) => end_export(hud, &root, None),
            Some(Err((error, text))) => {
                // Review Focus 3: the partial file goes; the editor and its timeline stay.
                error!("Replay export: {error}");
                end_export(hud, &root, Some(text));
            }
        }
    } else {
        // 60 Hz editor updates (research Q3: per-update gains, no dt).
        hud.accumulated = (hud.accumulated + real.delta_secs()).min(4. * FRAME);
        let pad = input.raw_input();
        let axis = |v: f32| v.signum() * ((v.abs() - 0.18) / 0.82).clamp(0., 1.);
        let trigger = |v: f32| ((v - 0.05) / 0.95).clamp(0., 1.);
        let pressed = |code: KeyCode| f32::from(keys.pressed(code));
        let (rt, lt) = (trigger(pad.triggers[1]).max(pressed(KeyCode::KeyE)), trigger(pad.triggers[0]).max(pressed(KeyCode::KeyQ)));
        let lb = pad.buttons & LB != 0 || keys.pressed(KeyCode::ShiftLeft);
        let rb = pad.buttons & RB != 0 || keys.pressed(KeyCode::Space);
        let dpad = Vec2::new(
            f32::from(pad.buttons & RIGHT != 0) - f32::from(pad.buttons & LEFT != 0),
            f32::from(pad.buttons & UP != 0) - f32::from(pad.buttons & DOWN != 0),
        );
        let sticks = cameras::Sticks {
            left: Vec2::new(axis(pad.left[0]), axis(pad.left[1])),
            right: Vec2::new(axis(pad.right[0]), axis(pad.right[1])),
            dpad,
        };
        let recorded = replay.sample().map(|(a, b, t)| (blend(a.root, b.root, t), blend(a.camera, b.camera, t)));
        let subject = recorded.map_or(cameras::Subject { position: Vec3::ZERO, game: Transform::IDENTITY }, |(root, camera)| cameras::Subject::new(&root, camera));
        // The camera on screen (last frame's): where a new camera edit shot and the stick orbit start (1a / 1b).
        let on_screen = replay.free_camera.or(recorded.map(|(_, c)| c)).unwrap_or_default();
        cameras::start_edit(editor, on_screen, subject);
        let was_playing = editor.playing;
        let inputs_live = !matches!(editor.state, State::Help | State::Menu | State::Settings | State::Popup(_));
        while hud.accumulated >= FRAME {
            hud.accumulated -= FRAME;
            if !inputs_live {
                continue;
            }
            // Bug 10-17 #28: the hand camera's pose before this update, for the display blend.
            editor.before = editor.hand();
            match editor.state {
                State::MainEdit => {
                    // The marks ease and settle every update (review M2); mode 2 (LB pressed, until its release)
                    // holds the sticks on them.
                    let lb = editor.moving_marks;
                    let (left, right) = if lb { (sticks.left.x, sticks.right.x) } else { (0., 0.) };
                    let (in_before, out_before) = (editor.in_mark, editor.out_mark);
                    // 3c: the left stick's mark first; when it moved, retail returns before reading the right
                    // stick (0x82611620..2C).
                    if !editor.move_mark(false, left) {
                        editor.move_mark(true, right);
                    }
                    // GetCursorOffset reads the playhead for the marks too: the playhead rides the moved mark.
                    if editor.in_mark != in_before {
                        editor.playhead = editor.in_mark;
                    } else if editor.out_mark != out_before {
                        editor.playhead = editor.out_mark;
                    }
                    if !lb {
                        editor.scrub(rt, lt);
                    }
                }
                State::Camera(_) => {
                    let roll = f32::from(u8::from(rb)) - f32::from(u8::from(lb));
                    cameras::edit_update(editor, sticks, roll, subject);
                }
                State::Scrub | State::OnlineInstant => {
                    editor.scrub(rt, lt);
                    cameras::orbit_update(editor, sticks, subject, on_screen);
                }
                _ => {}
            }
            editor.play(FRAME, seconds);
        }
        if was_playing && !editor.playing {
            // The stop helper 0x82615648 at the end head: [21, 19] from Edit Timeline, [19] from Instant Replay.
            if let Some(screen) = layer.as_deref_mut().and_then(|l| l.get(EDITOR)) {
                for state in editor.stop_states() {
                    let _ = set_state(screen, state);
                }
            }
        }
    }
    // Bug 10-17 #28: between 60 Hz updates the replay, its cameras and its grade move on with the leftover time
    // (Editor::shown), so every drawn frame advances. An export steps in whole frames and shows the playhead.
    let scrubbing = hud.export.is_none()
        && hud.editor.as_ref().is_some_and(|e| match e.state {
            State::Scrub | State::OnlineInstant => true,
            State::MainEdit => !e.moving_marks,
            _ => false,
        });
    let ahead = if hud.export.is_none() { hud.accumulated } else { 0. };
    // An export (ahead 0) shows each update's own pose.
    let hand_blend = (ahead / FRAME).min(1.);
    let length = replay.position().1 as f32;
    let editor = hud.editor.as_mut().expect("checked above");
    let shown = editor.shown(ahead, length, scrubbing);

    // The replay follows the editor.
    replay.seek_fraction(shown);
    replay.playing = editor.playing;
    // Speed / 100, a ramp's eased value included (the audio follows Replay::speed).
    replay.speed = editor.time_scale(editor.playhead);
    let recorded = replay.sample().map(|(a, b, t)| (blend(a.root, b.root, t), blend(a.camera, b.camera, t), a.fov + (b.fov - a.fov) * t));
    let (camera, fov) = recorded
        .and_then(|(root, camera, recorded_fov)| {
            let seconds = replay.position().0 as f32;
            cameras::evaluate(editor, shown, hand_blend, cameras::Subject::new(&root, camera), recorded_fov, seconds)
        })
        .map_or((None, None), |(t, fov)| (Some(t), fov));
    (replay.free_camera, replay.camera_fov) = (camera, fov);
    let next = Grade::at(editor, shown);
    if *grade != next {
        *grade = next;
    }

    let Some(layer) = layer else { return };
    // Take Photo: the screens step aside for the capture (retail 0x82615560(1) hides the HUD).
    if let Some(frames) = hud.photo {
        if hud.stashed.is_none() {
            hud.stashed = Some(std::mem::take(&mut layer.open));
            layer.extra.remove("timeline");
            layer.extra.remove("z-popup");
        }
        if frames == 0 {
            if let Some(stashed) = hud.stashed.take() {
                layer.open.extend(stashed);
            }
            hud.photo = None;
        } else {
            if frames == 1 {
                let path = file::folder(Folder::Pictures).map(|dir| photo_path(&dir, current.as_ref().map_or("replay", |c| c.name.as_str()), file::unix_now()));
                match path {
                    Some(path) => {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        info!("Replay photo: {}", path.display());
                        let slot = hud.photo_saved.clone();
                        commands.spawn(Screenshot::primary_window()).observe(move |captured: On<ScreenshotCaptured>| {
                            // As save_to_disk, but the result reaches the notice.
                            let saved = captured.image.clone().try_into_dynamic().map_err(|e| e.to_string()).and_then(|image| {
                                image.to_rgb8().save(&path).map_err(|e| format!("{}: {e}", path.display()))
                            });
                            if let Ok(mut slot) = slot.lock() {
                                *slot = Some(saved);
                            }
                        });
                    }
                    None => {
                        warn!("Replay photo: no Pictures folder");
                        hud.notice(&root, "ID_SAVE_FAILED_OPTIONS_DESC");
                    }
                }
            }
            hud.photo = Some(frames - 1);
        }
        return;
    }
    if hud.help_page.is_none()
        && let Some(editor) = hud.editor.as_ref().filter(|e| e.state == State::Help)
        && let Some(help) = layer.get(HELP)
    {
        let mode = help_mode(editor.movie_state());
        if retail_help_pages(mode) == 1
            && let Err(error) = add_next(help)
        {
            warn!("Replay help: {error}");
        }
        hud.help_page = Some(0);
    }
    let editor = hud.editor.as_ref().expect("checked above");
    if let Some(screen) = layer.get(EDITOR) {
        answer_all(screen, editor, seconds);
        send_marks(screen, editor, &mut hud.sent.2);
        let mut states = Vec::new();
        if (editor.playhead - hud.sent.1).abs() > 1e-6 {
            hud.sent.1 = editor.playhead;
            states.push(999);
        }
        // kOverClipMarker / kOverKeyMarker / kOverClip / kHideClipKey (Edit clip needs the timeline's margins).
        let over = (editor.state == State::MainEdit).then(|| {
            if editor.marker_under().is_some() {
                1006
            } else if editor.key_under().is_some() {
                1009
            } else if editor.playhead > super::editor::MARKER_RADIUS && editor.playhead < 1. - super::editor::MARKER_RADIUS {
                1007
            } else {
                1008
            }
        });
        if over != hud.sent.0 {
            hud.sent.0 = over;
            states.extend(over);
        }
        // UpdateZoom (state 11) as the camera edit's lens (0x82610c08: D-pad lens + state 11) or roll / ease change.
        let zoom = matches!(editor.state, State::Camera(_)).then_some((editor.shot.lens, editor.shot.roll, editor.pending_ease));
        if zoom != hud.sent_zoom {
            if zoom.is_some() && hud.sent_zoom.is_some() {
                states.push(11);
            }
            hud.sent_zoom = zoom;
        }
        for state in states {
            if let Err(error) = set_state(screen, state) {
                warn!("Replay Editor SetState({state}): {error}");
            }
        }
    }
    // The native timeline, where the movie shows its render box.
    let shown = !editor.hidden && matches!(editor.state, State::Scrub | State::MainEdit | State::Camera(_) | State::Popup(_) | State::Menu | State::Settings | State::Help);
    match (shown, layer.get(EDITOR).map(|s| visible(s, "mCustomRender"))) {
        (true, Some(true)) => {
            let sizes = hud.sizes.unwrap_or([[16, 30]; 4]);
            layer.extra.insert("timeline", timeline::draws(editor, sizes));
        }
        _ => {
            layer.extra.remove("timeline");
        }
    }
}

/// The brackets (user 10-27: both jumped to the one being moved): kMoveStartMarker / kMoveEndMarker place [ / ] at
/// GetCursorOffset, which reads GetPlayheadPosition whatever it is passed (RE3 l.323-367). Retail seeks to the mark it
/// moved and posts that mark's state alone (in 0x82611600 -> 1000, out 0x82611694 -> 1001; both on open,
/// 0x82610560..5F8), so each changed mark goes with the playhead answered as that mark, then the playhead's own
/// answer is back. `sent`: the marks last sent.
fn send_marks(screen: &mut Screen, editor: &Editor, sent: &mut (f32, f32)) {
    let marks = [(editor.in_mark, sent.0, 1000), (editor.out_mark, sent.1, 1001)];
    if marks.iter().all(|(mark, sent, _)| mark == sent) {
        return;
    }
    for (mark, _, state) in marks.into_iter().filter(|(mark, sent, _)| mark != sent) {
        screen.answer("FEReplayMenu.GetPlayheadPosition", json!(mark));
        if let Err(error) = set_state(screen, state) {
            warn!("Replay Editor SetState({state}): {error}");
        }
    }
    *sent = (editor.in_mark, editor.out_mark);
    screen.answer("FEReplayMenu.GetPlayheadPosition", json!(editor.playhead));
}

/// The clip at `path` is visible and opaque (EnableTimeline sets `mCustomRender._alpha`).
fn visible(screen: &Screen, path: &str) -> bool {
    use crate::apt_vm::Value;
    let Some(id) = screen.clip(path) else { return false };
    let shown = !matches!(screen.vm.get(id, "_visible"), Value::Bool(false));
    let alpha = match screen.vm.get(id, "_alpha") {
        Value::Number(a) => a,
        _ => 100.,
    };
    shown && alpha > 0.
}

/// The export camera (review I1): while an export runs, a copy of the gameplay camera (its tone, grade and post
/// components, so the frame matches the screen) renders into an image at the export size, following the gameplay
/// camera's transform and lens with the export's 16:9 aspect; it goes when the export ends.
#[allow(clippy::type_complexity)]
fn export_camera(
    hud: Res<ReplayHud>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    gameplay: Query<
        (Entity, &Transform, &Projection, &bevy::camera::RenderTarget),
        (With<crate::camera::GameplayCamera>, Without<ExportCamera>),
    >,
    mut exports: Query<(Entity, &ExportCamera, &mut Transform, &mut Projection, &mut Camera)>,
    mut scene: Query<&mut ImageNode, With<crate::graphics_menu::SceneView>>,
) {
    // Render speed (10-27): the world camera stands down while an export renders (camera.rs), and the screen shows
    // the export's own frames; then the world's image again.
    let world = gameplay.single().ok().and_then(|(.., target)| target.as_image());
    let shown = match (hud.export.as_ref(), exports.iter().next()) {
        (Some(_), Some((_, export, ..))) => Some(&export.0),
        _ => world,
    };
    if let Some(shown) = shown {
        for mut node in &mut scene {
            if node.image != *shown {
                node.image = shown.clone();
            }
        }
    }
    let Some(export) = hud.export.as_ref() else {
        for (entity, ..) in &exports {
            commands.entity(entity).despawn();
        }
        return;
    };
    let Ok((source, transform, projection, _)) = gameplay.single() else { return };
    let (w, h) = (export.job.settings.width, export.job.settings.height);
    if exports.is_empty() {
        let image = images.add(Image::new_target_texture(w, h, bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb, None));
        spawn_export_camera(&mut commands, source, image);
        return;
    }
    for (_, _, mut t, mut p, mut camera) in &mut exports {
        *t = *transform;
        *p = projection.clone();
        if let Projection::Perspective(perspective) = &mut *p {
            perspective.aspect_ratio = w as f32 / h as f32;
        }
        camera.is_active = true;
    }
}

/// The export camera itself: a copy of the gameplay camera `source`, drawing into `image` before it. RenderTarget is
/// replaced, not denied: the cloner also denies what requires a denied component (Camera, so Camera3d), and the copy
/// then drew nothing (every exported frame black, bug 10-27).
fn spawn_export_camera(commands: &mut Commands, source: Entity, image: Handle<Image>) {
    commands
        .entity(source)
        .clone_and_spawn_with_opt_out(|builder| {
            builder.deny::<crate::camera::GameplayCamera>();
        })
        .insert((
            ExportCamera(image.clone()),
            bevy::camera::RenderTarget::Image(image.into()),
            Camera { order: -2, is_active: true, ..default() },
        ));
}

/// Item 16 extension (Replay Settings > Camera Path, default off): while editing, the Adv Free / Tripod clip's keyed
/// path at the playhead, a line through the interpolated eyes and a cross per key. Never during an export or photo.
fn camera_path(hud: Res<ReplayHud>, replay: Res<Replay>, mut gizmos: Gizmos) {
    let Some(editor) = hud.editor.as_ref() else { return };
    if !editor.look.camera_path || hud.export.is_some() || hud.photo.is_some() || !replay.active {
        return;
    }
    if !matches!(editor.state, State::MainEdit | State::Effects | State::Camera(_)) {
        return;
    }
    let keys = editor.camera_keys(editor.playhead);
    if !keys.iter().all(|(_, shot, ..)| matches!(shot.kind, CameraKind::AdvFree | CameraKind::AdvTripod)) || keys.len() < 2 {
        return;
    }
    // Adv Free / Tripod eyes are in the world: no skater or game camera needed.
    let subject = cameras::Subject { position: Vec3::ZERO, game: Transform::IDENTITY };
    let (a, b) = (keys[0].0, keys[keys.len() - 1].0);
    // ponytail: 64 samples per clip; more when a long clip looks faceted.
    let points = (0..=64).filter_map(|i| cameras::interpolate(&keys, a + (b - a) * i as f32 / 64.)).map(|shot| cameras::view(&shot, subject).0);
    gizmos.linestrip(points, Color::srgb(1., 0.8, 0.1));
    for (_, shot, ..) in &keys {
        gizmos.sphere(Isometry3d::from_translation(cameras::view(shot, subject).0), 0.15, Color::WHITE);
    }
}

/// U3: `Pictures\Skate 3 Remaster\<map>-<timestamp>.png`.
pub(crate) fn photo_path(pictures: &Path, map: &str, unix: u64) -> PathBuf {
    pictures.join(format!("{}-{}.png", file::safe(map), file::stamp(unix)))
}

/// Replay Settings (research Q2: cMsgReplayEnableRenderPlayer / OtherPlayers): the player, the other skaters, the
/// world (MapEntity) and the green background, put back as they were when the editor closes.
#[allow(clippy::type_complexity)]
fn look(
    hud: Res<ReplayHud>,
    mut clear: ResMut<ClearColor>,
    mut player: Query<&mut Visibility, (With<crate::world::PlayerRoot>, Without<crate::map_render::MapEntity>)>,
    mut others: Query<&mut Visibility, (With<crate::multiplayer::appearance::RemoteCharacter>, Without<crate::world::PlayerRoot>, Without<crate::map_render::MapEntity>)>,
    mut world: Query<(Entity, &mut Visibility), With<crate::map_render::MapEntity>>,
    mut saved: Local<Option<(ClearColor, Vec<(Entity, Visibility)>, Option<Visibility>)>>,
) {
    let wanted = hud.editor.as_ref().map(|e| grade::visibility(&e.look)).filter(|v| *v != (true, true, true, false));
    let Some((show_world, show_player, show_others, green)) = wanted else {
        if let Some((colour, world_was, player_was)) = saved.take() {
            *clear = colour;
            for (entity, was) in world_was {
                if let Ok((_, mut v)) = world.get_mut(entity) {
                    *v = was;
                }
            }
            if let Some(was) = player_was {
                for mut v in &mut player {
                    *v = was;
                }
            }
        }
        return;
    };
    let saved = saved.get_or_insert_with(|| (clear.clone(), Vec::new(), player.iter().next().copied()));
    if green && clear.0 != Color::srgb(0., 1., 0.) {
        clear.0 = Color::srgb(0., 1., 0.);
    } else if !green && clear.0 != saved.0.0 {
        *clear = saved.0.clone();
    }
    for (entity, mut v) in &mut world {
        if !show_world && *v != Visibility::Hidden {
            saved.1.push((entity, *v));
            *v = Visibility::Hidden;
        }
    }
    if show_world && !saved.1.is_empty() {
        for (entity, was) in saved.1.drain(..) {
            if let Ok((_, mut v)) = world.get_mut(entity) {
                *v = was;
            }
        }
    }
    let player_to = if show_player { saved.2.unwrap_or(Visibility::Inherited) } else { Visibility::Hidden };
    for mut v in &mut player {
        v.set_if_neq(player_to);
    }
    if !show_others {
        for mut v in &mut others {
            v.set_if_neq(Visibility::Hidden);
        }
    }
}

#[cfg(test)]
#[path = "hud_tests.rs"]
mod tests;
