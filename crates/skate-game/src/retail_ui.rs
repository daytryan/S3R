//! Retail Skate 3 front-end screens the crossbar opens over the paused game, on the shared APT runtime (apt_screen,
//! layer RETAIL), prepared by tools/prepare_retail_screens.py (launcher stage `retail-screens`): Learn > Trick Guide
//! (`trickguide`), Game Settings > Video Calibration (`videocal`, its `sk8popup` question), Options > Extras (`extras`,
//! `credits`, `nucleus_tos`, and its videos on the movie player) and the movie subtitles (`subtitle`, Game Settings >
//! Subtitles). Their ActionScript runs as shipped; what retail's natives answer is answered here.
use crate::{
    apt_screen::{Layer, RETAIL, Screen, Screens},
    config::Config,
    core_menu_runtime::{Key, SOUND_A_BUTTON, SOUND_B_BUTTON, fe_sound_id},
    customiser::Navigation,
    popup_runtime,
    skate_audio::FrontEndSoundRequest,
    trick_guide::TrickGuide,
};
use bevy::{prelude::*, ui::IsDefaultUiCamera};
use serde_json::{Value as Json, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, atomic::{AtomicBool, Ordering}},
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    /// Learn > Trick Guide (crossbar item 3, `trickguide`).
    TrickGuide,
    /// Game Settings > Video Calibration (`videocal`).
    VideoCalibration,
    /// Options > Extras (crossbar item 34, `extras`): FrontEndState_Extras.
    Extras,
    /// Extras > Credits (FE state 25, `credits`).
    Credits,
    /// Extras > Licenses: the TermsOfServiceManager's window (`nucleus_tos`) on the WebKit licence.
    Licenses,
    /// Extras > Videos: a movie playing full screen (no screen of its own).
    Video,
}

/// Game Settings > Subtitles (retail game settings +157), published by graphics_menu.
pub(crate) static SUBTITLES: AtomicBool = AtomicBool::new(false);
/// The front-end movie playing (its fe_movie record stem) and when it started: boot.rs sets it.
/// A subtitle line is on screen: fmv_popup's Audio.HasSubtitles() lifts the skip prompt over it.
pub(crate) static SUBTITLE_SHOWING: AtomicBool = AtomicBool::new(false);
pub(crate) static MOVIE: Mutex<Option<(String, Instant)>> = Mutex::new(None);
/// The subtitle movie's name on the layer (drawn under "panel").
const SUBTITLE: &str = "0-subtitle";
const FOLDER: &str = "private/retail-screens";

#[derive(Resource, Default)]
pub(crate) struct RetailUi {
    /// core_menu's pick: open this screen.
    pub request: Option<Panel>,
    /// A screen is open: it owns input and the crossbar hides.
    pub modal: bool,
    /// The screen closed this frame; core_menu ignores that press.
    pub closed_this_frame: bool,
    open: Option<Panel>,
    /// Seconds since B played the outro.
    closing: Option<f32>,
    /// Built from the stock vault on first use.
    guide: Option<TrickGuide>,
    /// Video Calibration: the next videocal state to show (one past the shown one), and its closing question.
    calibration: i64,
    popup: Option<popup_runtime::Runtime>,
    popup_time: f32,
    /// Extras' popup is Start a New Career's: Some(false) its Yes / No, Some(true) its notice.
    new_career: Option<bool>,
    /// Every movie's subtitle lines (`subtitles.json`), read on first use; the line on screen ("" blank, None closed).
    cues: Option<HashMap<String, Vec<(f32, String)>>>,
    subtitle: Option<String>,
    /// Extras > Skate 2 Gestures was picked: the screen closes, then core_menu opens the gestures page (it takes this).
    pub gestures: bool,
    /// Extras: the Videos list is showing (FrontEndState_Extras +132) and its rows (fe_movie stem, title), built from
    /// the vault on first use.
    in_videos: bool,
    videos: Option<Vec<(String, String)>>,
    /// Extras > Videos: the stem to start, the playing movie, B/A pressed, and the movie over.
    play: Option<String>,
    movie: Option<crate::challenge::movie::Movie>,
    stop_video: bool,
    video_done: bool,
}

/// Outro fallback if a screen never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;
/// trickguide's SetState: 1 lists the level, 2 shows the row's description and button help, 10 hides the gesture
/// guide, 11 shows the row's.
const LIST: i64 = 1;
const DESCRIBE: i64 = 2;
const HIDE_GESTURES: i64 = 10;
const SHOW_GESTURES: i64 = 11;

/// One authored frame.
const FRAME: f32 = 1. / 60.;
/// videocal's SetState: 0 the settings card, 1 contrast, 2 brightness (videocal.SettingsState / ContrastState /
/// BrightnessState).
const CALIBRATION_STATES: i64 = 3;

/// Video Calibration's A (and the open, and the question's No): the next card, or after brightness the question
/// "Are you happy with your video calibration settings?" (Yes / No).
fn calibrate(screen: &mut Screen, ui: &mut RetailUi, root: &Path) -> Result<(), String> {
    if ui.calibration < CALIBRATION_STATES {
        screen.call("SetState", &[json!(ui.calibration)])?;
        ui.calibration += 1;
        return Ok(());
    }
    ask(ui, root, "ID_VIDEO_CALIBRATION_FINISHED_DESC", &["ID_COMMON_YES", "ID_COMMON_NO"], false)
}

/// The line showing `seconds` into a movie: each cue's from its time ("" before the first).
fn cue(lines: &[(f32, String)], seconds: f32) -> String {
    lines.iter().take_while(|(at, _)| *at <= seconds).last().map_or_else(String::new, |(_, text)| text.clone())
}

fn load_cues(root: &Path) -> HashMap<String, Vec<(f32, String)>> {
    let path = root.join(FOLDER).join("runtime/subtitles.json");
    std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
        .unwrap_or_else(|error| {
            warn!("Subtitles unavailable ({}: {error}); install the launcher's retail-screens stage", path.display());
            HashMap::new()
        })
}

/// Game Settings > Subtitles: the playing movie's line on the subtitle movie (0x8263E6B0 steps the .sub; the movie's
/// RefreshSubtitle asks Game.GetMovieSubtitleString). A movie without a .sub (the EA logo) shows nothing.
fn subtitles(
    mut ui: ResMut<RetailUi>,
    config: Res<Config>,
    screens: Option<ResMut<Screens>>,
    cutscene: Option<Res<crate::challenge::cutscene::Cutscene>>,
) {
    let Some(mut screens) = screens else { return };
    let ui = &mut *ui;
    let on = SUBTITLES.load(Ordering::Relaxed);
    let playing = MOVIE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let line = playing.filter(|_| on).and_then(|(stem, started)| {
        let cues = ui.cues.get_or_insert_with(|| load_cues(&config.asset_root));
        Some(cue(cues.get(&stem.to_lowercase())?, started.elapsed().as_secs_f32()))
    });
    // A career scene's own cues when no movie plays (spec 15.5), behind the same switch.
    let line = line.or_else(|| cutscene.as_deref()?.subtitle().filter(|_| on).map(str::to_owned));
    SUBTITLE_SHOWING.store(line.as_deref().is_some_and(|l| !l.is_empty()), Ordering::Relaxed);
    if line == ui.subtitle {
        return;
    }
    let layer = screens.layer(RETAIL);
    if let Err(error) = show_line(layer, line.as_deref()) {
        error!("Subtitles: {error} (install the launcher's retail-screens stage)");
        layer.close(SUBTITLE);
    }
    ui.subtitle = line;
}

/// Puts `line` on the subtitle movie: None closes it, "" blanks it.
fn show_line(layer: &mut Layer, line: Option<&str>) -> Result<(), String> {
    let Some(text) = line else {
        layer.close(SUBTITLE);
        return Ok(());
    };
    if layer.get(SUBTITLE).is_none() {
        layer.open(SUBTITLE, "subtitle", &[])?;
    }
    let screen = layer.get(SUBTITLE).ok_or("no subtitle movie")?;
    if text.is_empty() {
        return screen.call("Clear", &[]).map(drop);
    }
    screen.answer("Game.GetMovieSubtitleString", json!(text));
    screen.call("RefreshSubtitle", &[]).map(drop)
}

/// Extras_GetOptionTitle's table (0x83025EE0, read by 0x82655708) in its order, then the engine's own rows.
pub(crate) const EXTRAS: [&str; 9] = [
    "ID_EXTRA_CHEAT_CODE",
    "ID_EXTRA_EMERICA_SHOE_CODE",
    "ID_EXTRA_ETNIES_SHOE_CODE",
    "ID_EXTRA_PRE_SELL_CODE",
    "ID_EXTRA_CREDITS",
    "ID_EXTRA_LICENSES",
    "ID_SKATE_REEL_VIDEOS",
    // Not retail (user 10-20, Skate 2 physics, not tested in play): the four D-pad gestures Skate 2 gives the skater.
    "#Skate 2 Gestures",
    // Not retail (user 10-20, story mode stage 1, not tested in play): the Options page is full (spec 11).
    "#Start a New Career",
];
const CREDITS_ROW: u64 = 4;
const LICENSES_ROW: u64 = 5;
const VIDEOS_ROW: u64 = 6;
pub(crate) const GESTURES_ROW: u64 = 7;
const NEW_CAREER_ROW: u64 = 8;
const NEW_CAREER_ASK: &str = "#Start a new career? Your career, Unlock Everything answer and team are backed up, then reset. Your skater and settings stay.";
const NEW_CAREER_DONE: &str = "#The new career starts the next time you launch the game.";
/// The files a new career backs up; all but the first are removed (`career.json` is rewritten with the intro to do).
const NEW_CAREER_FILES: [&str; 4] = ["career.json", "skate3-career.json", "unlocks.json", "team.json"];

/// The Extras rows: without story mode in the build there is no career to start again, so its row goes.
fn extras(story_mode: bool) -> &'static [&'static str] {
    if story_mode { &EXTRAS } else { &EXTRAS[..NEW_CAREER_ROW as usize] }
}

/// UTC `yyyy-mm-dd-hhmmss` for `secs` past the Unix epoch (days to civil date: H. Hinnant's algorithm).
fn stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Extras > Start a New Career's Yes: copies [`NEW_CAREER_FILES`] that exist into `backups/<stamp>/`, removes the last
/// three and writes `career.json` with `intro_done: false`; `character.json` and the settings stay.
fn start_new_career(settings: &Path, stamp: &str) -> std::io::Result<std::path::PathBuf> {
    let backup = settings.join("backups").join(stamp);
    std::fs::create_dir_all(&backup)?;
    for name in NEW_CAREER_FILES.iter().filter(|n| settings.join(n).exists()) {
        std::fs::copy(settings.join(name), backup.join(name))?;
    }
    for name in &NEW_CAREER_FILES[1..] {
        match std::fs::remove_file(settings.join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let mut career = crate::boot::load_career(settings);
    career.intro_done = false;
    crate::saves::write_atomic(&settings.join("career.json"), &serde_json::to_vec_pretty(&career).map_err(std::io::Error::other)?)?;
    Ok(backup)
}

/// A question on retail's sk8popup over this layer.
fn ask(ui: &mut RetailUi, root: &Path, description: &str, options: &[&str], cancelable: bool) -> Result<(), String> {
    let path = root.join(FOLDER).join("runtime/sk8popup.json");
    let json = serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| e.to_string())?;
    (ui.popup, ui.popup_time) = (Some(popup_runtime::Runtime::load(&json, description, options, cancelable)?), 0.);
    Ok(())
}
/// ponytail: the first four rows are greyed. Retail's Enter Cheat Code opens the Xbox keyboard (0x826397D8) and
/// checks the code against its cheats (hoverboards, mini skaters, zombie mode, Isaac Clarke, 3D, street sweeper),
/// none of which is ported; the shoe and pre-sell codes are Xbox LIVE redemptions (flow messages 134..136).
const GREYED: u64 = 4;
/// extras' SetState(10) rebuilds the list from the Extras_ natives.
const EXTRAS_LIST: i64 = 10;
/// fe_movie (0x18B275357481FB52): stem, Extras title, listed in Videos, needs an entitlement.
const FE_MOVIE: &str = "Hash_18B275357481FB52";

/// Extras > Videos (0x82655934): every fe_movie listed in Videos that needs no DLC entitlement, in vault order.
fn videos(data: &skate_data::collections::Collections) -> Vec<(String, String)> {
    use skate_data::attrib_hash::numeric_id;
    let class = numeric_id(FE_MOVIE);
    let text = |key: &str, field: &str| data.field(FE_MOVIE, key, field).map(|f| f.data.clone()).unwrap_or_default();
    (data.entries().iter())
        .filter(|c| numeric_id(&c.class_name) == class)
        .filter(|c| {
            data.boolean(FE_MOVIE, &c.key, "Hash_07B87C1F8036DDDB").unwrap_or(false)
                && !data.boolean(FE_MOVIE, &c.key, "Hash_FBE0266E967A137F").unwrap_or(false)
        })
        .map(|c| (text(&c.key, "Hash_1971FB20F9154669"), text(&c.key, "Hash_AA753A536859233D")))
        .collect()
}

/// Answers extras' list natives for the main list or the Videos list, and rebuilds it.
fn refresh_extras(screen: &mut Screen, ui: &RetailUi) -> Result<(), String> {
    let rows: Vec<String> = match (ui.in_videos, &ui.videos) {
        (true, Some(videos)) => videos.iter().map(|(_, title)| title.clone()).collect(),
        _ => extras(crate::skate3_career::STORY_MODE).iter().map(|row| (*row).to_owned()).collect(),
    };
    // 0x825A6BD0 / 0x825A6C80.
    screen.answer("Game.Extras_MenuTitle", json!(if ui.in_videos { "ID_SKATE_REEL_VIDEOS" } else { "ID_MAIN_MENU_EXTRAS" }));
    screen.answer("Game.Extras_GetNumOptions", json!(rows.len()));
    for (i, row) in rows.iter().enumerate() {
        screen.answer(&format!("Game.Extras_GetOptionTitle({i})"), json!(row));
    }
    let greyed = !ui.in_videos;
    screen.bindings.native = Some(Box::new(move |key, args| {
        let row = args.get(1).and_then(Json::as_u64).unwrap_or(0);
        (key == "Game.OnMenuGetItemEnabled").then(|| json!(!(greyed && row < GREYED)))
    }));
    screen.call("SetState", &[json!(EXTRAS_LIST)]).map(drop)
}

/// The credits.txt lines (prepare_retail_screens `data`) as FECredits answers (0x825AAF68..).
fn credits_answers(root: &Path) -> Result<Vec<(String, Json)>, String> {
    let path = root.join(FOLDER).join("runtime/credit-lines.json");
    #[cfg_attr(not(feature = "dlss"), allow(unused_mut))]
    let mut lines: Vec<(f64, f64, f64, String)> =
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())?;
    // NVIDIA's DLSS attribution, in the last line's style (dlss.rs; docs/dlss.md).
    #[cfg(feature = "dlss")]
    if let Some(last) = lines.last().cloned() {
        lines.push((last.0, last.1, last.2, "NVIDIA DLSS".into()));
    }
    let mut answers = vec![("FECredits.GetNumCredits".to_string(), json!(lines.len()))];
    for (i, (_, size, spacing, text)) in lines.into_iter().enumerate() {
        // Literal: the lines are localised already.
        answers.push((format!("FECredits.GetLine({i})"), json!(format!("#{text}"))));
        answers.push((format!("FECredits.GetFontSize({i})"), json!(size)));
        answers.push((format!("FECredits.GetLineSpacing({i})"), json!(spacing)));
    }
    Ok(answers)
}

/// The licence as the TermsOfServiceManager's words: runs of text, and each space, CR and LF on its own (the window
/// lays them out itself, PREFERRED_WORD_CLUMP_SIZE at a time). ponytail: the native's own splitter is not traced.
fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\r' | '\n') {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            out.push(if c == '\t' { " ".into() } else { c.to_string() });
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// nucleus_tos in its view state (eTOS_VIEW) on the licence: tos.GetWordRange(start, count) answers
/// {numWords, word0, word1, ...} from `words`.
fn licenses(screen: &mut Screen, root: &Path) -> Result<(), String> {
    let path = root.join(FOLDER).join("runtime/license.txt");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let words = words(&String::from_utf8_lossy(&bytes));
    screen.answer("tos.GetNumWords", json!(words.len()));
    screen.bindings.native = Some(Box::new(move |key, args| {
        (key == "tos.GetWordRange").then(|| {
            let start = args.first().and_then(Json::as_u64).unwrap_or(0) as usize;
            let count = args.get(1).and_then(Json::as_u64).unwrap_or(0) as usize;
            // _FillLine reads m_aWords["word" + i].
            let mut range = serde_json::Map::new();
            for (i, word) in words.iter().skip(start).take(count).enumerate() {
                range.insert(format!("word{i}"), json!(word));
            }
            range.insert("numWords".into(), json!(range.len()));
            Json::Object(range)
        })
    }));
    // The window's stick scroll keys (FE key table 0x83027BF0 entries 24, 25).
    screen.set_global("AptLStickUp", json!(LSTICK_UP))?;
    screen.set_global("AptLStickDown", json!(LSTICK_DOWN))?;
    // TOS state 1: the data is ready; InitWords lays out the first page.
    screen.call("SetState", &[json!(1)]).map(drop)
}
/// nucleus_tos's Initialise finds its 20 line fields by target path from the root timeline ("mcTOSText.txtBox" + i).
/// This VM resolves a path's first name from the method's scope or _global, not the timeline the class was defined on,
/// so the root's text clip goes on _global under its own name.
fn tos_lines(screen: &mut Screen) -> Result<&mut Screen, String> {
    let clip = screen.clip("mcTOSText").ok_or("nucleus_tos: no mcTOSText")?;
    let global = screen.vm.global;
    screen.vm.set(global, "mcTOSText", crate::apt_vm::Value::Object(clip))?;
    Ok(screen)
}
const LSTICK_UP: i64 = 24;
const LSTICK_DOWN: i64 = 25;

/// Answers the guide's current level and redraws it (`list`: the rows changed too).
fn refresh_guide(screen: &mut Screen, guide: &TrickGuide, list: bool) -> Result<(), String> {
    for (key, value) in guide.answers() {
        screen.answer(&key, value);
    }
    if list {
        screen.call("SetState", &[json!(LIST)])?;
    }
    screen.call("SetState", &[json!(DESCRIBE)])?;
    screen.call("SetState", &[json!(if guide.on_trick() { SHOW_GESTURES } else { HIDE_GESTURES })])?;
    Ok(())
}

fn open(layer: &mut Layer, panel: Panel, ui: &mut RetailUi, config: &Config, goofy: bool) -> Result<(), String> {
    match panel {
        Panel::TrickGuide => {
            if ui.guide.is_none() {
                let data = skate_data::collections::Collections::shared(&config.asset_root)?; // perf 10-18
                ui.guide = Some(TrickGuide::build(&data));
            }
            let guide = ui.guide.as_mut().ok_or("no trick guide")?;
            guide.reset();
            let mut answers = guide.answers();
            answers.push(("FETrickTutorial.IsGoofy".into(), json!(goofy)));
            answers.push(("HUDComponents.PlayerStance_IsRegular".into(), json!(!goofy)));
            answers.push(("Tricks.IsStanceTrickGesturesFlipped".into(), json!(goofy)));
            let borrowed: Vec<(&str, serde_json::Value)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let screen = layer.open("panel", "trickguide", &borrowed)?;
            screen.play("intro")?;
            refresh_guide(screen, guide, true)
        }
        Panel::VideoCalibration => {
            let screen = loaded(layer.open("panel", "videocal", &[])?)?;
            screen.play("intro")?;
            ui.calibration = 0;
            calibrate(screen, ui, &config.asset_root)
        }
        Panel::Extras => {
            if ui.videos.is_none() {
                let data = skate_data::collections::Collections::shared(&config.asset_root)?; // perf 10-18
                ui.videos = Some(videos(&data));
            }
            let screen = loaded(layer.open("panel", "extras", &[])?)?;
            screen.play("intro")?;
            refresh_extras(screen, ui)
        }
        Panel::Credits => {
            let answers = credits_answers(&config.asset_root)?;
            let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let screen = loaded(layer.open("panel", "credits", &borrowed)?)?;
            // The native's SetState(kCreditScreenState_Rolling) starts the roll.
            screen.call("SetState", &[json!(1)]).map(drop)
        }
        Panel::Licenses => {
            let answers = [("tos.GetHeaderText", json!("ID_EXTRA_LICENSES_TITLE")), ("tos.GetTOSstate", json!(1))];
            let screen = loaded(tos_lines(layer.open("panel", "nucleus_tos", &answers)?)?)?;
            licenses(screen, &config.asset_root)
        }
        Panel::Video => {
            layer.close("panel");
            Ok(())
        }
    }
}

/// The screen manager's OnLoaded callback (RegisterScreen -> ScreenManager.OnLoaded -> the screen's OnLoaded, which
/// runs its Initialise), as music_ui and skate2_screens answer it.
fn loaded(screen: &mut Screen) -> Result<&mut Screen, String> {
    if screen.has("OnLoaded") {
        screen.call("OnLoaded", &[])?;
    }
    Ok(screen)
}

/// Where a closed screen returns: Extras' own screens go back to the list, the rest to the crossbar.
fn parent(panel: Panel) -> Option<Panel> {
    matches!(panel, Panel::Credits | Panel::Licenses | Panel::Video).then_some(Panel::Extras)
}

/// Extras > Videos: starts and runs the picked movie on the movie player (z 60), its subtitles over it.
fn play_videos(
    mut ui: ResMut<RetailUi>,
    config: Res<Config>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut audio: ResMut<Assets<crate::skate_audio::StreamPcm>>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
) {
    let ui = &mut *ui;
    if let Some(stem) = ui.play.take() {
        // The boot's movies (tools/prepare_boot.py): runtime/boot.json lists each stem's video and soundtrack.
        let boot = config.asset_root.join("private/boot");
        let started = (|| -> Result<crate::challenge::movie::Movie, String> {
            let index: Json = serde_json::from_slice(&std::fs::read(boot.join("runtime/boot.json")).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let entry = &index["movies"][&stem];
            let video = boot.join(entry["video"].as_str().ok_or("not prepared")?);
            let wav = entry["audio"].as_str().map(|a| boot.join(a));
            let camera = cameras.single().map_err(|e| e.to_string())?;
            crate::challenge::movie::Movie::start(&mut commands, &mut images, &mut audio, camera, &video, wav.as_deref())
        })();
        match started {
            Ok(movie) => {
                ui.movie = Some(movie);
                *MOVIE.lock().unwrap_or_else(|e| e.into_inner()) = Some((stem, Instant::now()));
            }
            Err(error) => {
                warn!("Extras video {stem} unavailable ({error}); install the launcher's boot stage");
                ui.video_done = true;
            }
        }
    }
    if let Some(movie) = ui.movie.as_mut() {
        if movie.advance(&mut images) || std::mem::take(&mut ui.stop_video) {
            if let Some(movie) = ui.movie.take() {
                movie.stop(&mut commands, &mut images);
            }
            *MOVIE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            ui.video_done = true;
        }
    }
}

pub(crate) struct RetailUiPlugin;
impl Plugin for RetailUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RetailUi>()
            .add_systems(PreUpdate, interact.after(crate::customiser::navigation).before(crate::core_menu::interact))
            .add_systems(PreUpdate, open_requested.after(crate::core_menu::interact).before(crate::graphics_menu::MenuInput))
            .add_systems(Update, (play_videos, subtitles).chain());
    }
}

/// perf 10-18: the crossbar's pick opens on its own frame; `interact` runs before the crossbar (it must not see the
/// screen's presses), so a request waited there a frame. SKATE_SAME_FRAME_OPEN=0: it does again.
fn open_requested(
    mut ui: ResMut<RetailUi>,
    config: Res<Config>,
    skater: Option<Res<crate::physics::SkaterRuntime>>,
    screens: Option<ResMut<Screens>>,
) {
    if let Some(mut screens) = screens.filter(|_| ui.request.is_some() && crate::config::perf_on("SKATE_SAME_FRAME_OPEN")) {
        open_request(&mut ui, screens.layer(RETAIL), &config, skater.as_deref());
    }
}
fn open_request(ui: &mut RetailUi, layer: &mut Layer, config: &Config, skater: Option<&crate::physics::SkaterRuntime>) {
    if let Some(panel) = ui.request.take() {
        let goofy = skater.is_some_and(|s| s.animation.goofy());
        match open(layer, panel, ui, config, goofy) {
            Ok(()) => (ui.open, ui.closing) = (Some(panel), None),
            Err(error) => {
                error!("Retail screen {panel:?}: {error} (install the launcher's retail-screens stage)");
                layer.close("panel");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn interact(
    mut ui: ResMut<RetailUi>,
    (keys, nav): (Res<ButtonInput<KeyCode>>, Res<Navigation>),
    config: Res<Config>,
    skater: Option<Res<crate::physics::SkaterRuntime>>,
    screens: Option<ResMut<Screens>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    real: Res<Time<Real>>,
    mut career: Option<ResMut<crate::skate3_career::glue::Career>>,
) {
    let ui = &mut *ui;
    ui.closed_this_frame = false;
    let Some(mut screens) = screens else {
        ui.request = None;
        return;
    };
    let layer = screens.layer(RETAIL);
    open_request(ui, layer, &config, skater.as_deref());
    let Some(panel) = ui.open else {
        (ui.modal, layer.modal) = (false, false);
        return;
    };
    let pressed = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
    ];
    if panel == Panel::Video {
        // A or B stops the movie; then back to the Videos list.
        if pressed.iter().any(|&(down, key)| down && matches!(key, Key::Accept | Key::Back)) {
            ui.stop_video = true;
        }
        if std::mem::take(&mut ui.video_done) {
            ui.in_videos = true;
            switch(layer, ui, Panel::Extras, &config);
        }
        (ui.modal, layer.modal) = (true, true);
        return;
    }
    let Some(screen) = layer.get("panel") else {
        (ui.open, ui.modal, layer.modal, ui.closed_this_frame, ui.popup) = (None, false, false, true, None);
        layer.extra.remove("z-popup");
        return;
    };
    if let Some(closing) = ui.closing.as_mut() {
        *closing += real.delta_secs();
        if screen.bindings.outro_complete || *closing >= CLOSE_TIMEOUT {
            layer.close("panel");
            (ui.open, ui.closing, ui.modal, layer.modal, ui.closed_this_frame) = (None, None, false, false, true);
            if let Some(back) = parent(panel) {
                switch(layer, ui, back, &config);
                (ui.modal, layer.modal, ui.closed_this_frame) = (true, true, false);
            }
        }
        return;
    }
    // The screen to switch to (a row of Extras, or back from one).
    let mut next = None;
    let result = (|| -> Result<(), String> {
        // The credits roll ends itself (FECredits.ExitCredits).
        if panel == Panel::Credits && screen.take_calls().iter().any(|(call, _)| call == "FECredits.ExitCredits") {
            ui.closing = Some(0.);
            return screen.play("outro");
        }
        for key in pressed.into_iter().filter(|(down, _)| *down).map(|(_, key)| key) {
            if let Some(popup) = ui.popup.as_mut() {
                let answer = popup.input(key);
                sounds.write_batch(popup.bindings.sounds.drain(..).map(FrontEndSoundRequest));
                if panel == Panel::Extras {
                    let Some(answer) = answer? else { continue };
                    let confirmed = ui.new_career == Some(false) && answer == 0;
                    (ui.popup, ui.new_career) = (None, None);
                    if confirmed {
                        let settings = crate::saves::settings_dir(&config.asset_root);
                        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                        start_new_career(&settings, &stamp(secs)).map_err(|e| format!("Start a New Career: {e}"))?;
                        // The running career writes nothing more this session.
                        if let Some(career) = career.as_mut() {
                            (career.state, career.off) = (None, Some("a new career starts next launch".into()));
                        }
                        ask(ui, &config.asset_root, NEW_CAREER_DONE, &["ID_COMMON_OK"], false)?;
                        ui.new_career = Some(true);
                    }
                    continue;
                }
                match answer? {
                    // Yes: done.
                    Some(0) => {
                        ui.popup = None;
                        ui.closing = Some(0.);
                        return screen.play("outro");
                    }
                    // No: from the first card again.
                    Some(_) => {
                        ui.popup = None;
                        ui.calibration = 0;
                        calibrate(screen, ui, &config.asset_root)?;
                    }
                    None => {}
                }
                continue;
            }
            match (panel, key) {
                (Panel::VideoCalibration, Key::Accept) => {
                    sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
                    calibrate(screen, ui, &config.asset_root)?;
                }
                // B steps back a card; on the first it leaves.
                (Panel::VideoCalibration, Key::Back) => {
                    sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON));
                    if ui.calibration < 2 {
                        ui.closing = Some(0.);
                        return screen.play("outro");
                    }
                    ui.calibration -= 1;
                    screen.call("SetState", &[json!(ui.calibration - 1)])?;
                }
                (Panel::VideoCalibration, _) => {}
                (Panel::Extras | Panel::Credits | Panel::Licenses, Key::Back) => {
                    sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON));
                    // The Videos list goes back to the main list.
                    if panel == Panel::Extras && ui.in_videos {
                        ui.in_videos = false;
                        refresh_extras(screen, ui)?;
                        continue;
                    }
                    ui.closing = Some(0.);
                    return screen.play("outro");
                }
                (Panel::Extras, _) => {
                    screen.take_calls();
                    screen.input(key)?;
                    let picked = (screen.take_calls().into_iter())
                        .find(|(call, _)| call == "Game.OnMenuSelect")
                        .and_then(|(_, args)| args.get(1).and_then(Json::as_u64));
                    match (ui.in_videos, picked) {
                        (true, Some(row)) => {
                            ui.play = ui.videos.as_ref().and_then(|v| v.get(row as usize)).map(|(stem, _)| stem.clone());
                            next = Some(Panel::Video);
                        }
                        // 0x826558E8: Credits pushes FE state 25, Licenses the TOS window, Videos the list.
                        (false, Some(CREDITS_ROW)) => next = Some(Panel::Credits),
                        (false, Some(LICENSES_ROW)) => next = Some(Panel::Licenses),
                        (false, Some(VIDEOS_ROW)) => {
                            ui.in_videos = true;
                            refresh_extras(screen, ui)?;
                        }
                        // The engine's row: close Extras like Back, and the crossbar shows the gestures page.
                        (false, Some(GESTURES_ROW)) => {
                            ui.gestures = true;
                            ui.closing = Some(0.);
                            return screen.play("outro");
                        }
                        (false, Some(NEW_CAREER_ROW)) => {
                            ask(ui, &config.asset_root, NEW_CAREER_ASK, &["ID_COMMON_YES", "ID_COMMON_NO"], true)?;
                            ui.new_career = Some(false);
                        }
                        _ => {}
                    }
                }
                (Panel::Licenses, Key::Up | Key::Down) => {
                    let stick = if key == Key::Up { LSTICK_UP } else { LSTICK_DOWN };
                    screen.call("UpdateInput", &[json!(stick), json!(0)])?;
                }
                (Panel::Credits | Panel::Licenses | Panel::Video, _) => {}
                (Panel::TrickGuide, _) => {
                    let Some(guide) = ui.guide.as_mut() else { continue };
                    if key == Key::Back {
                        sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON));
                        if !guide.back() {
                            ui.closing = Some(0.);
                            return screen.play("outro");
                        }
                        refresh_guide(screen, guide, true)?;
                        continue;
                    }
                    screen.take_calls();
                    screen.input(key)?;
                    let mut list = false;
                    for (call, args) in screen.take_calls() {
                        match call.as_str() {
                            "Game.OnMenuHilight" => guide.highlight(args.get(1).and_then(|a| a.as_u64()).unwrap_or(0) as usize),
                            // ponytail: menu_picker plays the A sound itself (mEnableSelectSound).
                            "Game.OnMenuSelect" => list |= guide.select(),
                            _ => {}
                        }
                    }
                    // The movie's own UpdateInput already asked for the row before the highlight landed here.
                    refresh_guide(screen, guide, list)?;
                }
            }
        }
        Ok(())
    })();
    sounds.write_batch(screen.bindings.sounds.drain(..).map(|key| FrontEndSoundRequest(fe_sound_id(&key))));
    // The question advances on real time and draws over the screen.
    let popup_time = &mut ui.popup_time;
    let popup = ui.popup.as_mut().map(|popup| {
        *popup_time = (*popup_time + real.delta_secs()).min(4. * FRAME);
        while *popup_time >= FRAME {
            *popup_time -= FRAME;
            popup.advance()?;
        }
        crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes)
    });
    match popup {
        Some(Ok(draws)) => drop(layer.extra.insert("z-popup", draws)),
        Some(Err(error)) => {
            error!("Retail popup stopped: {error}");
            ui.popup = None;
            layer.extra.remove("z-popup");
        }
        None => drop(layer.extra.remove("z-popup")),
    }
    if let Err(error) = result {
        error!("Retail screen stopped: {error}");
        layer.close("panel");
        (ui.open, ui.closing, ui.closed_this_frame, ui.popup) = (None, None, true, None);
        layer.extra.remove("z-popup");
    } else if let Some(next) = next {
        switch(layer, ui, next, &config);
    }
    ui.modal = ui.open.is_some();
    layer.modal = ui.modal;
}

/// Replaces the open screen with `panel`'s (Extras and the screens it opens); a screen that fails closes.
fn switch(layer: &mut Layer, ui: &mut RetailUi, panel: Panel, config: &Config) {
    match open(layer, panel, ui, config, false) {
        Ok(()) => (ui.open, ui.closing) = (Some(panel), None),
        Err(error) => {
            error!("Retail screen {panel:?}: {error} (install the launcher's retail-screens stage)");
            layer.close("panel");
            (ui.open, ui.closing, ui.closed_this_frame) = (None, None, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backup_stamp_is_utc() {
        assert_eq!(stamp(0), "1970-01-01-000000");
        // 2026-10-22 03:04:05 UTC.
        assert_eq!(stamp(1_792_638_245), "2026-10-22-030405");
        assert_eq!(stamp(951_782_400), "2000-02-29-000000");
    }

    #[test]
    fn the_extras_list_ends_with_start_a_new_career() {
        assert_eq!(EXTRAS[NEW_CAREER_ROW as usize], "#Start a New Career");
        assert_eq!(EXTRAS.len() as u64, NEW_CAREER_ROW + 1);
    }

    /// A release without story mode (user 10-22) lists no Start a New Career, so no player's team or unlocks reset.
    #[test]
    fn without_story_mode_extras_has_no_new_career_row() {
        assert_eq!(extras(true), &EXTRAS[..]);
        assert_eq!(extras(false).len() as u64, NEW_CAREER_ROW);
        assert!(!extras(false).contains(&"#Start a New Career"));
    }

    #[test]
    fn start_a_new_career_backs_up_and_resets() {
        let dir = std::env::temp_dir().join(format!("s3newcareer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("career.json"), r#"{"intro_done": true, "team_name": "Crew", "tutorial": true}"#).unwrap();
        for name in ["skate3-career.json", "unlocks.json", "team.json", "character.json"] {
            std::fs::write(dir.join(name), name).unwrap();
        }
        let backup = start_new_career(&dir, "2026-10-22-030405").unwrap();
        assert_eq!(backup, dir.join("backups").join("2026-10-22-030405"));
        for name in NEW_CAREER_FILES {
            assert!(backup.join(name).exists(), "{name} backed up");
        }
        assert!(std::fs::read_to_string(backup.join("career.json")).unwrap().contains("Crew"));
        for name in &NEW_CAREER_FILES[1..] {
            assert!(!dir.join(name).exists(), "{name} removed");
        }
        let career = crate::boot::load_career(&dir);
        assert!(!career.intro_done && career.team_name.as_deref() == Some("Crew"));
        assert_eq!(std::fs::read_to_string(dir.join("character.json")).unwrap(), "character.json", "the skater stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
    use std::path::PathBuf;

    /// The prepared retail screens (SKATE_RETAIL_SCREENS, else SKATE3_ASSET_ROOT/private/retail-screens), driven as
    /// the game drives them.
    fn root() -> Option<PathBuf> {
        let root = std::env::var_os("SKATE_RETAIL_SCREENS").map(PathBuf::from).or_else(|| {
            Some(PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?).join("private/retail-screens"))
        })?;
        root.join("runtime/trickguide.json").is_file().then_some(root)
    }
    fn collections() -> Option<skate_data::collections::Collections> {
        skate_data::collections::Collections::load(&PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?)).ok()
    }
    fn texts(screen: &Screen) -> Vec<String> {
        let mut out: Vec<String> = (screen.bindings.movie.instances.keys())
            .filter_map(|&id| match screen.vm.get(id, "_displayText") {
                crate::apt_vm::Value::Text(t) if !t.is_empty() => Some(t),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn trick_guide_lists_describes_and_descends() {
        let Some(root) = root() else { return };
        let Some(data) = collections() else { return };
        let mut guide = TrickGuide::build(&data);
        let answers = guide.answers();
        let borrowed: Vec<(&str, serde_json::Value)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&root.join("runtime/trickguide.json"), &borrowed).unwrap();
        screen.play("intro").unwrap();
        refresh_guide(&mut screen, &guide, true).unwrap();
        (0..40).for_each(|_| screen.advance().unwrap());
        let shown = texts(&screen);
        for want in ["Trick Guide", "Flip Tricks", "Terminology"] {
            assert!(shown.iter().any(|t| t == want), "{want} missing from {shown:?}");
        }
        screen.take_calls();
        screen.input(Key::Down).unwrap();
        let hilight = screen.take_calls().into_iter().find(|(c, _)| c == "Game.OnMenuHilight").map(|(_, a)| a[1].clone());
        assert_eq!(hilight, Some(json!(1)));
        guide.highlight(0);
        assert!(guide.select());
        assert!(guide.select());
        guide.highlight(1);
        refresh_guide(&mut screen, &guide, true).unwrap();
        (0..20).for_each(|_| screen.advance().unwrap());
        assert!(!screen.draws().unwrap().is_empty());
    }

    #[test]
    fn a_cue_shows_from_its_time_until_the_next() {
        let lines = [(2.0, String::new()), (3.5, "Live tonight".to_string()), (18.0, "Ever since".to_string())];
        assert_eq!(cue(&lines, 0.5), "");
        assert_eq!(cue(&lines, 3.5), "Live tonight");
        assert_eq!(cue(&lines, 17.9), "Live tonight");
        assert_eq!(cue(&lines, 60.0), "Ever since");
    }

    /// Video Calibration's three cards, then its question; subtitles show a line and clear.
    #[test]
    fn video_calibration_steps_its_cards_and_asks() {
        let Some(root) = root() else { return };
        let assets = root.parent().unwrap().parent().unwrap().to_path_buf();
        let mut ui = RetailUi::default();
        let mut screen = Screen::load(&root.join("runtime/videocal.json"), &[]).unwrap();
        loaded(&mut screen).unwrap();
        screen.play("intro").unwrap();
        for want in ["We recommend these HDTV settings", "Adjust Contrast (Picture)", "Adjust Brightness TV"] {
            calibrate(&mut screen, &mut ui, &assets).unwrap();
            (0..60).for_each(|_| screen.advance().unwrap());
            let shown = texts(&screen);
            assert!(shown.iter().any(|t| t.contains(want)), "{want} missing from {shown:?}");
        }
        assert!(ui.popup.is_none());
        calibrate(&mut screen, &mut ui, &assets).unwrap();
        let popup = ui.popup.as_mut().expect("the question");
        (0..40).for_each(|_| popup.advance().unwrap());
        assert_eq!(popup.input(Key::Accept).unwrap(), Some(0), "Yes first");

        let mut subtitle = Screen::load(&root.join("runtime/subtitle.json"), &[]).unwrap();
        subtitle.answer("Game.GetMovieSubtitleString", json!("Welcome to Port Carverton!"));
        subtitle.call("RefreshSubtitle", &[]).unwrap();
        (0..10).for_each(|_| subtitle.advance().unwrap());
        assert!(texts(&subtitle).iter().any(|t| t == "Welcome to Port Carverton!"), "{:?}", texts(&subtitle));
        assert!(!subtitle.draws().unwrap().is_empty());
        subtitle.call("Clear", &[]).unwrap();
        (0..10).for_each(|_| subtitle.advance().unwrap());
        // Clear hides the line and its dimmer (EnableSubtitle(false)).
        assert!(subtitle.draws().unwrap().is_empty());
        let cues = load_cues(&assets);
        assert_eq!(cue(&cues["attract"], 18.0), "Welcome to Port Carverton!");
        assert_eq!(cue(&cues["intro_movie"], 1.0), "");
    }

    #[test]
    fn licence_words_keep_spaces_and_breaks() {
        assert_eq!(words("Copyright (C) 2009\r\nApple  Inc."), ["Copyright", " ", "(C)", " ", "2009", "\r", "\n", "Apple", " ", " ", "Inc."]);
    }

    /// Extras: its seven rows (the code rows greyed), the Videos list, the credits roll and the licence window.
    #[test]
    fn extras_lists_rolls_credits_and_shows_the_licences() {
        let Some(root) = root() else { return };
        let Some(data) = collections() else { return };
        let assets = root.parent().unwrap().parent().unwrap().to_path_buf();
        let mut ui = RetailUi { videos: Some(videos(&data)), ..Default::default() };
        let owned = |v: &[(&str, &str)]| v.iter().map(|&(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        assert_eq!(ui.videos, Some(owned(&[("intro_movie", "ID_EXTRA_VIDEO_1"), ("Attract", "ID_ATTRACT_VIDEO")])));

        let mut screen = Screen::load(&root.join("runtime/extras.json"), &[]).unwrap();
        loaded(&mut screen).unwrap();
        screen.play("intro").unwrap();
        refresh_extras(&mut screen, &ui).unwrap();
        (0..40).for_each(|_| screen.advance().unwrap());
        let shown = texts(&screen);
        for want in ["Extras", "Enter Cheat Code", "Enter Pre-Sell Code", "Credits", "Licenses", "Videos"] {
            assert!(shown.iter().any(|t| t == want), "{want} missing from {shown:?}");
        }
        // The highlight opens on Credits, past the greyed code rows; Down goes to Licenses, then Videos, which opens the list.
        let mut row = None;
        for _ in 0..2 {
            screen.take_calls();
            screen.input(Key::Down).unwrap();
            if let Some((_, args)) = screen.take_calls().into_iter().find(|(c, _)| c == "Game.OnMenuHilight") {
                row = args[1].as_u64();
            }
            assert!(row.is_none_or(|r| r >= GREYED), "highlighted greyed row {row:?}");
        }
        assert_eq!(row, Some(VIDEOS_ROW));
        screen.take_calls();
        screen.input(Key::Accept).unwrap();
        let picked = screen.take_calls().into_iter().find(|(c, _)| c == "Game.OnMenuSelect").map(|(_, a)| a[1].clone());
        assert_eq!(picked, Some(json!(VIDEOS_ROW)));
        ui.in_videos = true;
        refresh_extras(&mut screen, &ui).unwrap();
        (0..20).for_each(|_| screen.advance().unwrap());
        let shown = texts(&screen);
        for want in ["Intro Video", "Attract Video"] {
            assert!(shown.iter().any(|t| t == want), "{want} missing from {shown:?}");
        }

        let answers = credits_answers(&assets).unwrap();
        let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut credits = Screen::load(&root.join("runtime/credits.json"), &borrowed).unwrap();
        loaded(&mut credits).unwrap();
        credits.call("SetState", &[json!(1)]).unwrap();
        let shot = |s: &Screen, name: &str| {
            if let Ok(out) = std::env::var("SKATE_RETAIL_SHOTS") {
                std::fs::create_dir_all(&out).unwrap();
                crate::core_menu_runtime::tests::raster_draws(&s.draws().unwrap(), &root, &Path::new(&out).join(name));
            }
        };
        (0..600).for_each(|_| credits.advance().unwrap());
        shot(&credits, "credits.png");
        // User 10-14: the credits drew giant in the debug bitmap font ("Helvetica LT Std" isn't in the movie).
        for o in &credits.vm.objects {
            if let Some(crate::apt_vm::Value::Text(t)) = o.fields.get("_displayText") {
                let w = o.fields.get("_width").map_or(0., crate::apt_vm::Value::number);
                assert!(w < 640., "credit {t:?} is {w} px wide");
            }
        }
        let rolling = texts(&credits);
        assert!(rolling.iter().any(|t| t.contains("Black Box")), "{rolling:?}");
        assert!(!credits.draws().unwrap().is_empty());

        let answers = [("tos.GetHeaderText", json!("ID_EXTRA_LICENSES_TITLE")), ("tos.GetTOSstate", json!(1))];
        let mut window = Screen::load(&root.join("runtime/nucleus_tos.json"), &answers).unwrap();
        loaded(tos_lines(&mut window).unwrap()).unwrap();
        licenses(&mut window, &assets).unwrap();
        (0..20).for_each(|_| window.advance().unwrap());
        let page = texts(&window);
        shot(&window, "licenses.png");
        // User 10-14: a filled line wrapped again inside its field and overlapped the next.
        for o in &window.vm.objects {
            if o.fields.get("wordWrap") == Some(&crate::apt_vm::Value::Bool(false)) {
                let h = o.fields.get("_height").map_or(0., crate::apt_vm::Value::number);
                assert!(h < 30., "a licence line wraps: {:?} {h}", o.fields.get("_displayText"));
            }
        }
        assert!(page.iter().any(|t| t == "Licenses"), "{page:?}");
        assert!(page.iter().any(|t| t.contains("The software included")), "{page:?}");
        window.call("UpdateInput", &[json!(LSTICK_DOWN), json!(0)]).unwrap();
        (0..5).for_each(|_| window.advance().unwrap());
        assert_ne!(texts(&window), page, "the stick scrolls a line");
    }
}
