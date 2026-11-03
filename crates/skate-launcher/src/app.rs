use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, sync::{atomic::{AtomicBool, Ordering}, Mutex}};
use tao::event_loop::EventLoopProxy;

pub enum Command { Ready, PickFolders { add: bool }, RemoveFolder(usize), Scan, Build { fresh: bool }, Cancel, Play { safe: bool },
                   CheckUpdates, UpdateNow, CopyReport, CopyTimes, Open(String),
                   SetAnimations(bool), SetAutoUpdate(bool), Set(String, Value), OpenFolder(String), Verify, Storage,
                   Clear(String), OpenDefender, LongPathsOn, MoveGame, Uninstall { all: bool }, BackupSaves, RestoreSaves, ResetGameSettings,
                   SupportBundle, Shortcut, SendCrash(String), OpenCrashFolder(String),
                   // sent by the launcher itself, never by the page:
                   CheckUpdatesQuietly, UpdateAutomatically }

impl Command {
    pub fn parse(body: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
        let text = |k: &str| v[k].as_str().map(str::to_owned).ok_or(format!("missing {k}"));
        let flag = |k: &str| v[k].as_bool().unwrap_or(false);
        Ok(match v["cmd"].as_str().unwrap_or("") {
            "ready" => Self::Ready, "pick_folders" => Self::PickFolders { add: flag("add") }, "scan" => Self::Scan,
            "remove_folder" => Self::RemoveFolder(v["index"].as_u64().ok_or("missing index")? as usize),
            "build" => Self::Build { fresh: flag("fresh") }, "cancel" => Self::Cancel, "play" => Self::Play { safe: flag("safe") },
            "check_updates" => Self::CheckUpdates, "update_now" => Self::UpdateNow,
            "copy_report" => Self::CopyReport, "copy_times" => Self::CopyTimes, "open" => Self::Open(text("url")?),
            "set_animations" => Self::SetAnimations(v["on"].as_bool().ok_or("missing on")?),
            "set_auto_update" => Self::SetAutoUpdate(v["on"].as_bool().ok_or("missing on")?),
            "set" => Self::Set(text("key")?, v["value"].clone()),
            "open_folder" => match v["which"].as_str() { Some(w @ ("game" | "logs")) => Self::OpenFolder(w.into()), _ => return Err("bad which".into()) },
            "verify" => Self::Verify, "storage" => Self::Storage,
            "clear" => match v["what"].as_str() {
                Some(w @ ("leftovers" | "temp" | "audio" | "logs" | "crashes" | "cache" | "tools")) => Self::Clear(w.into()),
                _ => return Err("bad what".into()),
            },
            "open_defender" => Self::OpenDefender, "long_paths_on" => Self::LongPathsOn, "move_game" => Self::MoveGame, "uninstall" => Self::Uninstall { all: flag("all") },
            "backup_saves" => Self::BackupSaves, "restore_saves" => Self::RestoreSaves, "reset_game_settings" => Self::ResetGameSettings,
            "support_bundle" => Self::SupportBundle, "shortcut" => Self::Shortcut,
            "send_crash" => Self::SendCrash(text("name")?), "open_crash_folder" => Self::OpenCrashFolder(text("name")?),
            other => return Err(format!("unknown command {other:?}")),
        })
    }
}

/// The launcher's saved settings, `<dir>\launcher.json`. A missing or unreadable file (or field) means the default.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub folders: Vec<PathBuf>,
    pub animations: bool,
    pub auto_update: bool,
    pub on_game_start: String,     // "open" | "minimize" | "close" (F5)
    pub keep_awake: bool,          // #7
    pub low_priority: bool,        // #7: the build at below-normal priority
    pub fewer_resources: bool,     // #19: SKATE_BUILD_JOBS=2, SKATE_FANOUT=2
    pub language: String,          // F12
    pub game: Option<PathBuf>,     // #3 Move game; None = <launcher folder>\game
    pub minimize_to_tray: bool,    // user 11-01: minimized, the launcher waits in the tray (tray.rs)
}

impl Default for Settings {
    fn default() -> Self {
        Self { folders: vec![], animations: true, auto_update: false, on_game_start: "minimize".into(), keep_awake: true,
               low_priority: true, fewer_resources: false, language: "en".into(), game: None, minimize_to_tray: false }
    }
}

impl Settings {
    /// The page's `set {key, value}`. False (nothing changed) for an unknown key or a value of the wrong kind.
    pub fn set(&mut self, key: &str, value: &Value) -> bool {
        match (key, value) {
            ("on_game_start", Value::String(s)) if ["open", "minimize", "close"].contains(&s.as_str()) => self.on_game_start = s.clone(),
            ("keep_awake", Value::Bool(b)) => self.keep_awake = *b,
            ("low_priority", Value::Bool(b)) => self.low_priority = *b,
            ("fewer_resources", Value::Bool(b)) => self.fewer_resources = *b,
            ("minimize_to_tray", Value::Bool(b)) => self.minimize_to_tray = *b,
            ("language", Value::String(s)) if (2..=10).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') =>
                self.language = s.clone(),
            _ => return false,
        }
        true
    }
}

pub fn load_settings(dir: &Path) -> Settings {
    std::fs::read(dir.join("launcher.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Written to a temp file and renamed over the old one, so a crash never leaves half a file.
pub fn save_settings(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    let text = serde_json::to_string(settings)?;
    let temp = dir.join("launcher.json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, dir.join("launcher.json")).inspect_err(|_| { let _ = std::fs::remove_file(&temp); })
}

/// The update check at startup runs once, and only for an installed game. No key is needed: a public repository
/// answers without one, and a private one's refusal shows on the Updates page only.
pub fn startup_check_due(game_installed: bool, already: &AtomicBool) -> bool {
    game_installed && !already.swap(true, Ordering::SeqCst)
}

pub struct App {
    pub proxy: Mutex<EventLoopProxy<Value>>,
    pub dir: PathBuf,                   // the launcher's folder
    pub settings: Mutex<Settings>,      // saved in <dir>\launcher.json
    pub running: Mutex<Option<u32>>,    // the build's process id while building
    pub helpers: Mutex<Vec<u32>>,       // running checks and scans (pipeline::stop_helpers)
    pub game_pid: Mutex<Option<u32>>,   // F1: the game while it runs, whoever started it
    pub last_error: Mutex<Option<Value>>,
    pub last_state: Mutex<Value>,       // the last `state` sent: a refused Build puts the page back
    pub last_check: Mutex<Value>,       // the last check's `state` event (estimate, sizes)
    pub build_after_check: AtomicBool,  // an update installed: rebuild what it changed once the check says so
    pub busy_moving: AtomicBool,        // a Move game or Remove game data is in progress
    pub clearing: AtomicBool,           // A1: a Storage clear is deleting files
    pub stopping: std::sync::atomic::AtomicU32,  // A2: the build Stop was pressed for, until it has exited (0: none)
    startup_check: AtomicBool,
    hero_watch: AtomicBool,             // hero::watch runs once per launcher
    played_since: Mutex<Option<std::time::Instant>>,  // playtime.rs: the running game's start
}

impl App {
    pub fn new(proxy: EventLoopProxy<Value>) -> Self {
        let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
        Self { proxy: Mutex::new(proxy), settings: Mutex::new(load_settings(&dir)), dir, running: Mutex::new(None),
               helpers: Mutex::new(Vec::new()), game_pid: Mutex::new(None), last_error: Mutex::new(None),
               last_state: Mutex::new(Value::Null), last_check: Mutex::new(Value::Null), build_after_check: AtomicBool::new(false),
               busy_moving: AtomicBool::new(false), clearing: AtomicBool::new(false), stopping: Default::default(), startup_check: AtomicBool::new(false), hero_watch: AtomicBool::new(false),
               played_since: Mutex::new(None) }
    }
    pub fn save(&self) -> std::io::Result<()> { save_settings(&self.dir, &self.settings.lock().unwrap()) }
    pub fn folders(&self) -> Vec<PathBuf> { self.settings.lock().unwrap().folders.clone() }
    pub fn game(&self) -> PathBuf { self.settings.lock().unwrap().game.clone().unwrap_or_else(|| self.dir.join("game")) }
    pub fn send(&self, message: Value) {
        if message["type"] == "state" { *self.last_state.lock().unwrap() = message.clone(); }
        let _ = self.proxy.lock().unwrap().send_event(message);
    }
    pub fn toast(&self, text: &str) { self.send(json!({"type": "toast", "text": text})); }

    /// The page's `folders` message: the source folders it shows, and the folder the game lives in.
    /// Paths go through `to_string_lossy` because `json!` would panic on a path that is not UTF-8.
    pub fn folders_message(&self) -> Value {
        let folders: Vec<String> = self.folders().iter().map(|f| f.to_string_lossy().into_owned()).collect();
        json!({"type": "folders", "folders": folders, "game": self.game().to_string_lossy()})
    }

    pub fn settings_message(&self) -> Value {
        let s = self.settings.lock().unwrap().clone();
        json!({"type": "settings", "animations": s.animations, "auto_update": s.auto_update, "version": crate::update::version(),
               "installed": crate::update::installed_build(&self.game()), "on_game_start": s.on_game_start,
               "keep_awake": s.keep_awake, "low_priority": s.low_priority, "fewer_resources": s.fewer_resources,
               "language": s.language, "minimize_to_tray": s.minimize_to_tray,
               "shortcut": crate::maintenance::shortcut_path().is_some_and(|p| p.is_file())})
    }

    /// F1: the game is running (our Play, or one found from this game folder). Its exit is waited for on its handle.
    pub fn game_started(self: &std::sync::Arc<Self>, pid: u32, external: bool) {
        *self.game_pid.lock().unwrap() = Some(pid);
        self.played_since.lock().unwrap().get_or_insert_with(std::time::Instant::now);
        self.send(json!({"type": "game_running", "on": true, "external": external}));
        if external {
            let app = self.clone();
            std::thread::spawn(move || { crate::system::wait_for_exit(pid, u32::MAX); app.game_exited(pid); });
        }
    }
    pub fn game_exited(&self, pid: u32) {
        let mut game = self.game_pid.lock().unwrap();
        if *game != Some(pid) { return; }
        *game = None;
        drop(game);
        if let Some(since) = self.played_since.lock().unwrap().take() {
            crate::playtime::add(&crate::playtime::path(), &crate::playtime::key(), since.elapsed().as_secs());
            self.send(crate::playtime::message());
        }
        self.send(json!({"type": "game_running", "on": false, "external": false}));
        crate::hero::poke();  // the look may have changed in Create-a-Skater
    }
    /// Whether the game runs now. A game the launcher didn't start is looked for too, and then tracked.
    pub fn game_running(self: &std::sync::Arc<Self>) -> bool {
        if self.game_pid.lock().unwrap().is_some() { return true; }
        // the launcher's own --hero-clip render (hero.rs) runs the same exe; it sits with the helpers
        let ours = self.helpers.lock().unwrap().clone();
        match crate::system::find_process_except(&self.game().join("skate3rust.exe"), &ours) {
            Some(pid) => { self.game_started(pid, true); true }
            None => false,
        }
    }
    /// Anything that rewrites the game folder must wait for the game (F1) and for a build or move.
    pub fn refuse_while_busy(self: &std::sync::Arc<Self>) -> bool {
        let reason = if self.game_running() { "Close the game first." }
            else if self.running.lock().unwrap().is_some() { "Wait for the build to finish first." }
            else if self.clearing.load(Ordering::SeqCst) { "Wait for the clean-up to finish." }
            else if self.busy_moving.load(Ordering::SeqCst) || crate::update::installing() { "Wait for the current task to finish first." }
            else { return false };
        self.toast(reason);
        let state = self.last_state.lock().unwrap().clone();
        if !state.is_null() { self.send(state); }  // the page had already switched to building
        true
    }

    pub fn handle(self: &std::sync::Arc<Self>, body: &str) {
        let command = match Command::parse(body) { Ok(c) => c, Err(_) => return };
        match command {
            // Feature #2 (user 10-14): off the UI thread, so the window paints and answers while this reads and spawns.
            Command::Ready => {
                let app = self.clone();
                std::thread::spawn(move || {
                    crate::maintenance::on_ready(&app);
                    app.refresh();
                    app.send(crate::playtime::message());
                    // the current clip, never an out-of-date one (bug 10-17 #51), then keep it current (hero.rs)
                    if let Some(m) = crate::hero::paths(&app.game()).as_ref().and_then(crate::hero::current) { app.send(m); }
                    if !app.hero_watch.swap(true, Ordering::SeqCst) { crate::hero::watch(app.clone()); }
                });
            }
            Command::Play { .. } if crate::update::installing() => self.toast("Finishing the update first. Press Play when it's done."),
            Command::Play { .. } if self.game_running() => self.toast("The game is already running."),
            // B4: an automatic rebuild, a repair or a clear can start between the page's last render and the click
            Command::Play { .. } if self.running.lock().unwrap().is_some() => self.toast("Wait for the build to finish first."),
            Command::Play { .. } if self.clearing.load(Ordering::SeqCst) => self.toast("Wait for the clean-up to finish."),
            Command::Play { safe } => {
                crate::hero::stop(self);
                let mut game = std::process::Command::new(self.game().join("skate3rust.exe"));
                game.current_dir(self.game()).env(crate::crash::LAUNCHER_PID, std::process::id().to_string());
                if let Ok(me) = std::env::current_exe() { game.env("SKATE_LAUNCHER_EXE", me); }  // crash_report.rs: wherever the game moved (#3)
                if safe { game.arg("--safe"); }
                match game.spawn() {
                    Ok(game) => {
                        self.toast(if safe { "Starting S3R in safe mode…" } else { "Starting S3R…" });
                        self.game_started(game.id(), false);
                        self.send(json!({"game_started": self.settings.lock().unwrap().on_game_start}));  // main.rs (F5)
                        crate::crash::watch(self.clone(), game);
                    }
                    Err(e) => self.send(json!({"type": "error", "scope": "play", "message": format!("Could not start the game: {e}")})),
                }
            }
            Command::SetAnimations(on) => { self.settings.lock().unwrap().animations = on; self.save_or_report(); }
            Command::SetAutoUpdate(on) => { self.settings.lock().unwrap().auto_update = on; self.save_or_report(); }
            Command::Set(key, value) => {
                if self.settings.lock().unwrap().set(&key, &value) { self.save_or_report(); }
                self.send(self.settings_message());
            }
            Command::SendCrash(name) => crate::crash::send(self, &name),
            Command::OpenCrashFolder(name) => crate::crash::open_folder(&name),
            Command::Open(url) if openable(&url) => { let _ = std::process::Command::new("explorer").arg(url).spawn(); }
            Command::OpenDefender => { let _ = std::process::Command::new("explorer").arg("windowsdefender://threatsettings").spawn(); }
            Command::OpenFolder(which) => {
                let folder = if which == "logs" { self.game().join("data").join("logs") } else { self.game() };
                let _ = std::fs::create_dir_all(&folder);
                let _ = std::process::Command::new("explorer").arg(folder).spawn();
            }
            other => crate::pipeline::handle(self, other),
        }
    }

    pub fn save_or_report(&self) {
        if let Err(e) = self.save() { self.send(json!({"type": "error", "scope": "settings", "message": format!("Could not save the setting: {e}")})); }
    }

    /// What the Play button should be: download the game first, missing files, ready to build, or play.
    pub fn refresh(self: &std::sync::Arc<Self>) {
        self.send(self.folders_message());
        self.send(self.settings_message());
        let installed = self.game().join("skate3rust.exe").is_file();
        if !installed {
            return self.send(json!({"type": "state", "state": "update", "first": true}));
        }
        self.game_running();  // F1: a game started outside the launcher
        // A3: Play at once from the last answer; the real check corrects it when it answers
        if !crate::update::installing() && self.running.lock().unwrap().is_none() {
            let file = crate::maintenance::local_data().join("last-check.json");
            if let Some(state) = crate::pipeline::remembered_check(&file, &self.game()) { self.send(state); }
        }
        crate::pipeline::check(self);
        if startup_check_due(installed, &self.startup_check) {
            let auto = self.settings.lock().unwrap().auto_update;
            crate::update::handle(self, if auto { Command::UpdateAutomatically } else { Command::CheckUpdatesQuietly });
            crate::update::check_every_few_hours(self.clone());
        }
    }

    pub fn shutdown(&self) {
        // The launcher closing with the game still up: count the time so far (a later restart counts on from there).
        if let Some(since) = self.played_since.lock().unwrap().as_mut() {
            crate::playtime::add(&crate::playtime::path(), &crate::playtime::key(), since.elapsed().as_secs());
            *since = std::time::Instant::now();
        }
        if let Some(pid) = self.running.lock().unwrap().take() { crate::pipeline::kill(pid); }
        crate::pipeline::stop_helpers(&self.helpers);  // checks and a hero render: their jobs end them at once
        crate::maintenance::empty_tmp(&self.game());
    }
}

/// The project's Discord invite, the one page off GitHub the launcher opens.
const DISCORD: &str = "https://discord.gg/PC2W9K4xan";

/// The page's `open`: only the project's own GitHub pages (releases, issues, the guide) and its Discord invite, in
/// plain characters, so explorer gets neither another site nor a switch or a comma it would read.
fn openable(url: &str) -> bool {
    url == DISCORD || url.strip_prefix(&format!("https://github.com/{}", crate::update::REPO)).is_some_and(|rest|
        (rest.is_empty() || rest.starts_with('/')) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-#".contains(&b)))
}

pub fn fatal(text: &str) {
    rfd::MessageDialog::new().set_title("S3R").set_description(text).set_level(rfd::MessageLevel::Error).show();
}

/// An App for tests: its proxy goes to an event loop nobody runs, so what it sends is dropped.
#[cfg(test)]
pub fn test_app(game: &Path) -> std::sync::Arc<App> {
    use tao::platform::windows::EventLoopBuilderExtWindows;
    static PROXY: std::sync::OnceLock<Mutex<EventLoopProxy<Value>>> = std::sync::OnceLock::new();
    let proxy = PROXY.get_or_init(|| {
        let event_loop = tao::event_loop::EventLoopBuilder::<Value>::with_user_event().with_any_thread(true).build();
        let proxy = event_loop.create_proxy();
        std::mem::forget(event_loop);
        Mutex::new(proxy)
    }).lock().unwrap().clone();
    let app = App::new(proxy);
    app.settings.lock().unwrap().game = Some(game.to_path_buf());
    std::sync::Arc::new(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_parse_and_unknown_is_rejected() {
        assert!(matches!(Command::parse(r#"{"cmd":"open","url":"https://example.com"}"#), Ok(Command::Open(_))));
        assert!(Command::parse(r#"{"cmd":"rm -rf"}"#).is_err());
        assert!(matches!(Command::parse(r#"{"cmd":"build"}"#), Ok(Command::Build { fresh: false })));
        assert!(matches!(Command::parse(r#"{"cmd":"build","fresh":true}"#), Ok(Command::Build { fresh: true })));
        assert!(matches!(Command::parse(r#"{"cmd":"set_animations","on":false}"#), Ok(Command::SetAnimations(false))));
        assert!(Command::parse(r#"{"cmd":"set_animations"}"#).is_err());
        assert!(matches!(Command::parse(r#"{"cmd":"set_auto_update","on":true}"#), Ok(Command::SetAutoUpdate(true))));
        assert!(Command::parse(r#"{"cmd":"check_updates_quietly"}"#).is_err(), "the page cannot ask for the quiet check");
        assert!(Command::parse(r#"{"cmd":"update_automatically"}"#).is_err(), "nor for the automatic install");
    }

    #[test]
    fn open_takes_only_the_projects_github_pages() {
        let page = include_str!("../ui/launcher.html");
        assert!(page.contains("const REPO = 'https://github.com/daytryan/S3R';"), "the page's links are the ones allowed");
        for url in ["https://github.com/daytryan/S3R/releases", "https://github.com/daytryan/S3R/issues",
                    "https://github.com/daytryan/S3R/blob/main/docs/source-files-guide.md#skate3_disc"] {
            assert!(openable(url), "{url}");
        }
        assert!(page.contains(&format!("const DISCORD = '{DISCORD}';")) && openable(DISCORD), "the Discord button's link");
        for url in ["https://discord.gg/someone-else", "https://discord.gg/PC2W9K4xan,/root,C:\\",
                    "https://evil.example/", "https://github.com/daytryan/S3R.evil.example/", "https://github.com/someone/else",
                    "https://github.com/daytryan/S3R/x,/root,C:\\", "https://github.com/daytryan/S3R/a b",
                    "http://github.com/daytryan/S3R/releases", "file:///C:/Windows/System32/calc.exe"] {
            assert!(!openable(url), "{url}");
        }
    }

    #[test]
    fn the_contracts_new_commands_parse() {
        assert!(matches!(Command::parse(r#"{"cmd":"play","safe":true}"#), Ok(Command::Play { safe: true })));
        assert!(matches!(Command::parse(r#"{"cmd":"play"}"#), Ok(Command::Play { safe: false })));
        assert!(matches!(Command::parse(r#"{"cmd":"pick_folders","add":true}"#), Ok(Command::PickFolders { add: true })));
        assert!(matches!(Command::parse(r#"{"cmd":"remove_folder","index":2}"#), Ok(Command::RemoveFolder(2))));
        assert!(matches!(Command::parse(r#"{"cmd":"open_folder","which":"logs"}"#), Ok(Command::OpenFolder(w)) if w == "logs"));
        assert!(Command::parse(r#"{"cmd":"open_folder","which":"C:\\Windows"}"#).is_err(), "only the two folders");
        assert!(matches!(Command::parse(r#"{"cmd":"clear","what":"audio"}"#), Ok(Command::Clear(w)) if w == "audio"));
        assert!(Command::parse(r#"{"cmd":"clear","what":"settings"}"#).is_err(), "saves are never a pile to clear");
        assert!(matches!(Command::parse(r#"{"cmd":"set","key":"keep_awake","value":false}"#), Ok(Command::Set(k, v)) if k == "keep_awake" && v == false));
        for cmd in ["verify", "storage", "open_defender", "move_game", "uninstall", "backup_saves", "restore_saves",
                    "reset_game_settings", "support_bundle", "shortcut"] {
            assert!(Command::parse(&format!(r#"{{"cmd":"{cmd}"}}"#)).is_ok(), "{cmd}");
        }
    }

    #[test]
    fn the_set_command_takes_only_known_keys_and_good_values() {
        let mut s = Settings::default();
        assert_eq!((s.on_game_start.as_str(), s.keep_awake, s.low_priority, s.fewer_resources, s.language.as_str()),
                   ("minimize", true, true, false, "en"));
        assert!(s.set("on_game_start", &json!("close")) && s.on_game_start == "close");
        assert!(!s.set("on_game_start", &json!("explode")) && s.on_game_start == "close");
        assert!(s.set("keep_awake", &json!(false)) && !s.keep_awake);
        assert!(!s.minimize_to_tray && s.set("minimize_to_tray", &json!(true)) && s.minimize_to_tray, "off until asked for");
        assert!(!s.set("keep_awake", &json!("no")));
        assert!(s.set("language", &json!("pt-BR")) && s.language == "pt-BR");
        assert!(!s.set("language", &json!("../../x")));
        assert!(!s.set("game", &json!("C:\\")), "the game folder moves only through Move game");
        assert!(!s.set("folders", &json!([])));
    }

    #[test]
    fn settings_round_trip_atomically_and_default_when_missing_or_broken() {
        let dir = std::env::temp_dir().join(format!("s3r-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_settings(&dir), Settings::default());
        let mut s = Settings { folders: vec![PathBuf::from(r"D:\Games\Skate"), PathBuf::from(r"E:\More")], animations: false,
                               auto_update: true, ..Settings::default() };
        save_settings(&dir, &s).unwrap();
        assert_eq!(load_settings(&dir), s);
        s.game = Some(PathBuf::from(r"D:\Games\S3R\game"));
        save_settings(&dir, &s).unwrap();
        assert_eq!(load_settings(&dir), s, "the second save replaces the first");
        assert!(!dir.join("launcher.json.tmp").exists(), "no temp file left behind");
        // an older launcher's file: three fields, the rest default
        std::fs::write(dir.join("launcher.json"), r#"{"folders":["D:\\x"],"animations":false,"auto_update":true}"#).unwrap();
        let old = load_settings(&dir);
        assert_eq!((old.folders.len(), old.animations, old.auto_update, old.on_game_start.as_str()), (1, false, true, "minimize"));
        std::fs::write(dir.join("launcher.json"), b"{not json").unwrap();
        assert_eq!(load_settings(&dir), Settings::default());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_startup_update_check_runs_once_and_only_for_an_installed_game() {
        let once = AtomicBool::new(false);
        assert!(!startup_check_due(false, &once));
        assert!(startup_check_due(true, &once), "no key needed");
        assert!(!startup_check_due(true, &once), "only once per launch");
    }

    #[test]
    fn a_hero_render_is_not_the_players_game() {
        let game = std::env::temp_dir().join(format!("s3r-hero-game-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&game);
        std::fs::create_dir_all(&game).unwrap();
        std::fs::copy(r"C:\Windows\System32\PING.EXE", game.join("skate3rust.exe")).unwrap();
        let mut render = std::process::Command::new(game.join("skate3rust.exe")).args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null()).spawn().unwrap();
        let app = test_app(&game);
        app.helpers.lock().unwrap().push(render.id());  // hero.rs: a --hero-clip render sits with the helpers
        assert!(!app.game_running(), "the launcher's own render is not the player's game");
        assert!(!app.refuse_while_busy());
        app.helpers.lock().unwrap().clear();
        assert!(app.game_running(), "the same exe started by anyone else is");
        render.kill().unwrap();
        let _ = render.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = std::fs::remove_dir_all(game);
    }
    #[test]
    fn play_stops_a_hero_render_but_not_the_checks() {
        let game = std::env::temp_dir().join(format!("s3r-hero-play-{}", std::process::id()));
        let app = test_app(&game);
        let mut render = std::process::Command::new(r"C:\Windows\System32\PING.EXE").args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null()).spawn().unwrap();
        app.helpers.lock().unwrap().extend([render.id(), 1]);  // 1: a check's pid, left alone
        crate::hero::RENDER.store(render.id(), Ordering::SeqCst);
        crate::hero::stop(&app);
        assert_eq!(*app.helpers.lock().unwrap(), [1], "the render left the helpers, so it reads as stopped, not failed");
        let start = std::time::Instant::now();
        let _ = render.wait();
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "the render was stopped");
    }
}
