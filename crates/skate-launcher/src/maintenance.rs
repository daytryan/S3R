//! Looking after the install: the launcher's log, temp folders, leftovers, where the game lives (#3), storage (P8),
//! saves (F2), the support bundle (F7) and the desktop shortcut (F8).
use crate::app::{App, Command};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, sync::{atomic::Ordering, Arc}, time::{Duration, SystemTime}};

const DAY: Duration = Duration::from_secs(24 * 3600);

/// `%LOCALAPPDATA%\Skate3RustEngine`: crash reports, the audio cache, the WebView profile and the launcher's log.
/// H1: tests get their own folder, so `cargo test` never writes the player's launcher.log, playtime or audio cache.
pub fn local_data() -> PathBuf {
    #[cfg(test)]
    return std::env::temp_dir().join("s3r-test-local-data");
    #[cfg(not(test))]
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Skate3RustEngine")
}

/// #12: the launcher's own log, one line per problem (downloads, installs, panics). Past 1 MB it becomes
/// `launcher.old.log`, so it never holds more than 2 MB.
pub fn log(text: &str) { log_to(&local_data().join("launcher.log"), text) }

pub fn log_to(path: &Path, text: &str) {
    use std::io::Write;
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 1 << 20) { let _ = std::fs::rename(path, path.with_extension("old.log")); }
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{stamp} build {} {text}", crate::update::version());
    }
}

/// Empty `game\data\tmp` (#2/P2), keeping the folder. Files still in use stay.
pub fn empty_tmp(game: &Path) {
    for entry in std::fs::read_dir(crate::pipeline::tmp_dir(game)).into_iter().flatten().flatten() {
        let path = entry.path();
        let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
    }
}

fn age(path: &Path, now: SystemTime) -> Duration {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).unwrap_or_default()
}

/// Remove a folder only when nothing holds a file in it: a folder in use can't be renamed, so it is renamed first.
fn remove_if_unused(dir: &Path) -> bool {
    let doomed = dir.with_extension("s3r-delete");
    std::fs::rename(dir, &doomed).is_ok() && std::fs::remove_dir_all(&doomed).is_ok()
}

/// P3: what builds, checks and game sessions left in `%TEMP%` (C1, C7, C8) once they are older than `min_age`:
/// our own PyInstaller unpack folders (they hold `tools\pipeline`; other apps' `_MEI*` don't), the pipeline's scratch
/// folders and crashed online sessions' looks. A folder in use is skipped. Returns how many went.
pub fn sweep_temp(temp: &Path, min_age: Duration) -> usize {
    const OURS: [&str; 10] = ["s3r-fanout-", "skate-online-", "skate-sky-", "skate-backdrop-", "skate-dmo-", "skate-teleports-",
                              "dlc-db-", "dlc-sky-", "pro-clothing-", "s3r-launcher-"];
    let now = SystemTime::now();
    let mut swept = 0;
    for entry in std::fs::read_dir(temp).into_iter().flatten().flatten() {
        let (path, name) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
        if !path.is_dir() || age(&path, now) < min_age { continue; }
        let ours = (name.starts_with("_MEI") && path.join("tools").join("pipeline").is_dir()) || OURS.iter().any(|p| name.starts_with(p));
        if ours && remove_if_unused(&path) { swept += 1; }
    }
    swept
}

/// The installation `data\installation.json` names (its `directory`), if any.
fn named_installation(data: &Path) -> Option<String> {
    // A8: installation.json can be ~23 MB; serde skips all but "directory" without building a tree of it
    #[derive(serde::Deserialize)]
    struct Marker { directory: Option<String> }
    // B5: a marker cut short (power loss mid-write) falls back to the copy atomic_json keeps
    ["installation.json", "installation.json.bak"].iter()
        .find_map(|f| serde_json::from_slice::<Marker>(&std::fs::read(data.join(f)).ok()?).ok())?.directory
}

/// Build leftovers (C3, C4): extracted discs and DLC, and installations the marker doesn't name. Before F2 moved the
/// saves out, an unnamed installation could still hold them: one with a settings folder is kept until `data\settings`
/// exists.
fn leftovers(data: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = ["discs", "dlc"].iter().map(|d| data.join("work").join(d)).filter(|p| p.is_dir()).collect();
    let named = named_installation(data);
    let saves_moved = data.join("settings").is_dir();
    for entry in std::fs::read_dir(data.join("installations")).into_iter().flatten().flatten() {
        let rel = format!("installations/{}", entry.file_name().to_string_lossy());
        if named.as_deref().map(|n| n.replace('\\', "/")) == Some(rel) || !entry.path().is_dir() { continue; }
        let holds_saves = std::fs::read_dir(entry.path().join("settings")).is_ok_and(|mut d| d.next().is_some());
        if saves_moved || !holds_saves { out.push(entry.path()); }
    }
    out
}

/// P4: a build that failed or was stopped more than 7 days ago, with none since, gives its leftovers back. The last
/// build activity is the newest write among the work folders, the logs (both generations: P5 renames logs\ at build
/// start) and the marker. `running` (app.running) is held throughout, so a build can't start under the deletion.
pub fn expire_abandoned(data: &Path, now: SystemTime, running: &std::sync::Mutex<Option<u32>>) -> usize {
    let moved = {
        let running = running.lock().unwrap();
        if running.is_some() { return 0; }
        abandoned(data, now).iter().filter(|p| to_trash(data, p)).count()  // A6: only renames under the lock
    };
    empty_trash(data);
    moved
}

fn abandoned(data: &Path, now: SystemTime) -> Vec<PathBuf> {
    let work = data.join("work");
    if !work.join("discs").is_dir() && !work.join("dlc").is_dir() { return vec![]; }
    let mut times: Vec<PathBuf> = ["logs", "logs.previous"].iter()
        .flat_map(|d| std::fs::read_dir(data.join(d)).into_iter().flatten().flatten().map(|e| e.path())).collect();
    times.extend([data.join("installation.json"), work.clone(), work.join("discs"), work.join("dlc")]);
    let Some(newest) = times.iter().filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).max() else { return vec![] };
    if now.duration_since(newest).unwrap_or_default() < 7 * DAY { return vec![]; }
    leftovers(data)
}

/// A6: move `path` into `data\trash` so a lock can be let go before the slow delete. A rename on one volume is
/// instant. False when it can't move (a file in it is open); the caller then leaves it or deletes in place.
pub fn to_trash(data: &Path, path: &Path) -> bool {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let trash = data.join("trash");
    let _ = std::fs::create_dir_all(&trash);
    let ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    std::fs::rename(path, trash.join(format!("{ms}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)))).is_ok()
}

/// A6: delete what is in `data\trash` on a low-priority thread, with no lock held. Each entry goes on its own, so a
/// `to_trash` running meanwhile still finds the folder.
pub fn empty_trash(data: &Path) {
    let trash = data.join("trash");
    if !trash.is_dir() { return; }
    std::thread::spawn(move || {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };  // low disk priority too
        for entry in std::fs::read_dir(&trash).into_iter().flatten().flatten() {
            let path = entry.path();
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    });
}

/// #3: what is wrong with where the game lives (or `None`). `env` reads an environment variable.
pub fn location_problem(game: &Path, writable: bool, env: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    let path = game.to_string_lossy().to_lowercase();
    let under = |vars: &[&str]| vars.iter().filter_map(|v| env(v)).filter(|v| !v.is_empty())
        .any(|root| path.starts_with(&root.to_lowercase().trim_end_matches('\\').to_owned()));
    if under(&["TEMP", "TMP"]) || path.contains("\\appdata\\local\\temp\\") { return Some("temp"); }  // run from inside the zip
    if under(&["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]) { return Some("program_files"); }
    if !writable { return Some("readonly"); }
    if under(&["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]) { return Some("onedrive"); }
    if path.len() > 100 { return Some("long"); }
    None
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".s3r-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

pub fn location_message(app: &App) -> Value {
    let game = app.game();
    let problem = location_problem(&game, writable(&app.dir), |v| std::env::var(v).ok());
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    json!({"type": "location", "problem": problem, "path": game.to_string_lossy(), "suggest": format!("{drive}\\Games\\Skate 3 Remaster")})
}

/// The page's `ready`: undo an interrupted update (#16), empty our temp folder, say where the game lives (#3), and
/// in the background sweep `%TEMP%` (P3), expire abandoned build files (P4) and, before a first build, check the PC (F16).
/// #16 at start: once per launcher, and never while an install runs (a page reload must not undo it mid-way).
fn recover_once(game: &Path, done: &std::sync::atomic::AtomicBool) -> bool {
    if crate::update::installing() || done.swap(true, Ordering::SeqCst) { return false; }
    crate::update::recover_interrupted(game)
}
static RECOVERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn on_ready(app: &Arc<App>) {
    let game = app.game();
    if recover_once(&game, &RECOVERED) { log("an interrupted update was undone"); }
    crate::pipeline::empty_tmp_when_idle(app);
    app.send(location_message(app));
    let app = app.clone();
    empty_trash(&game.join("data"));  // A6: what an earlier session left in data\trash
    std::thread::spawn(move || {
        // D1: the WebView2 profile from before P7 sat beside the exe; it now lives in local_data\webview
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_dir_all(PathBuf::from(format!("{}.WebView2", exe.display())));
        }
        let swept = sweep_temp(&std::env::temp_dir(), DAY);
        if swept > 0 { log(&format!("removed {swept} old temp folders")); }
        let gone = expire_abandoned(&game.join("data"), SystemTime::now(), &app.running);
        if gone > 0 { log(&format!("removed {gone} build leftovers older than 7 days")); }
        if game.join("skate3rust.exe").is_file() && !game.join("data").join("installation.json").is_file() {
            app.send(crate::pc::check_in_child());
        }
    });
}

/// F8: where the desktop shortcut goes (the real Desktop, even when OneDrive moved it).
pub fn shortcut_path() -> Option<PathBuf> {
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath}};
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    let ok = unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, 0, std::ptr::null_mut(), &mut raw) } == 0 && !raw.is_null();
    let path = ok.then(|| {
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        PathBuf::from(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) }))
    });
    unsafe { CoTaskMemFree(raw as *const _) };
    Some(path?.join("Skate 3 Remaster.lnk"))
}

/// Waits for `pid` to exit, then deletes `files` and `dirs` (errors ignored: a missing one is already gone).
fn cleanup_script(pid: u32, files: &[PathBuf], dirs: &[PathBuf]) -> String {
    let list = |paths: &[PathBuf]| paths.iter().map(|p| ps_quote(&p.to_string_lossy())).collect::<Vec<_>>().join(",");
    format!("Wait-Process -Id {pid} -Timeout 30 -EA 0;Start-Sleep 1;Remove-Item -LiteralPath {} -Force -EA 0;\
             Remove-Item -LiteralPath {} -Recurse -Force -EA 0", list(files), list(dirs))
}

/// What "Uninstall everything" deletes of `local` (local_data()): all of it, except (audit 10-24) imported characters,
/// which Back up saves treats as saves.
fn local_data_to_remove(local: &Path) -> Vec<PathBuf> {
    let characters = local.join("custom-characters");
    if !std::fs::read_dir(&characters).is_ok_and(|mut d| d.next().is_some()) { return vec![local.to_path_buf()]; }
    std::fs::read_dir(local).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| *p != characters).collect()
}

/// A PowerShell single-quoted string: only `'` needs doubling.
fn ps_quote(text: &str) -> String { format!("'{}'", text.replace('\'', "''")) }

/// A5: the shell's own IShellLinkW, called through its vtable: no PowerShell (slow, scanned, blocked on some PCs).
fn make_shortcut(link: &Path, exe: &Path) -> Result<(), String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};
    use windows_sys::{core::GUID, Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED}};
    const SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    type Text = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
    type Icon = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32;
    type Query = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    type Save = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32;
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (target, dir, out) = (wide(exe), wide(exe.parent().unwrap_or(Path::new("."))), wide(link));
    unsafe {
        let init = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as _);
        let slot = |obj: *mut c_void, i: usize| *(*(obj as *const *const usize)).add(i);
        let mut shell: *mut c_void = std::ptr::null_mut();
        let mut saved = -1;
        if CoCreateInstance(&SHELL_LINK, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &ISHELL_LINK_W, &mut shell) >= 0 {
            // IShellLinkW: 9 SetWorkingDirectory, 17 SetIconLocation, 20 SetPath; IPersistFile: 6 Save
            let ok = std::mem::transmute::<usize, Text>(slot(shell, 20))(shell, target.as_ptr()) >= 0
                && std::mem::transmute::<usize, Text>(slot(shell, 9))(shell, dir.as_ptr()) >= 0
                && std::mem::transmute::<usize, Icon>(slot(shell, 17))(shell, target.as_ptr(), 0) >= 0;
            let mut file: *mut c_void = std::ptr::null_mut();
            if ok && std::mem::transmute::<usize, Query>(slot(shell, 0))(shell, &IPERSIST_FILE, &mut file) >= 0 {
                saved = std::mem::transmute::<usize, Save>(slot(file, 6))(file, out.as_ptr(), 1);
                std::mem::transmute::<usize, Release>(slot(file, 2))(file);
            }
            std::mem::transmute::<usize, Release>(slot(shell, 2))(shell);
        }
        if init >= 0 { CoUninitialize(); }
        if saved >= 0 && link.is_file() { Ok(()) } else { Err("Windows did not make the shortcut.".into()) }
    }
}

/// B9: turn on Windows long paths through the normal UAC prompt (reg.exe as administrator). True once reg.exe said
/// it worked; false when the player said no or it failed.
fn long_paths_on() -> bool {
    use windows_sys::Win32::{Foundation::CloseHandle, System::{Com::{CoInitializeEx, COINIT_APARTMENTTHREADED},
        Threading::{GetExitCodeProcess, WaitForSingleObject}}, UI::Shell::*};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("runas"), wide("reg.exe"));
    let args = wide(r#"add "HKLM\SYSTEM\CurrentControlSet\Control\FileSystem" /v LongPathsEnabled /t REG_DWORD /d 1 /f"#);
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as _);  // ShellExecuteEx wants COM; a thread of its own
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        (info.lpVerb, info.lpFile, info.lpParameters, info.nShow) = (verb.as_ptr(), file.as_ptr(), args.as_ptr(), 0);  // SW_HIDE
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() { return false; }
        WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        code == 0
    }
}

/// The saves folder: `data\settings` once F2 moved it there, else the current installation's own.
pub fn saves_dir(game: &Path) -> Option<PathBuf> {
    let data = game.join("data");
    if data.join("settings").is_dir() { return Some(data.join("settings")); }
    let dir = named_installation(&data)?;
    Some(data.join(dir).join("settings")).filter(|p| p.is_dir())
}

/// Zip `dirs` (name in the zip, folder) into `out`. Only files; folders are implied.
fn zip_dirs(out: &Path, dirs: &[(&str, PathBuf)], extra: &[(&str, Vec<u8>)]) -> Result<usize, String> {
    use std::io::Write;
    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    let mut count = 0;
    for (name, dir) in dirs {
        for path in walk(dir) {
            let rel = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
            zip.start_file(format!("{name}/{rel}"), options).map_err(|e| e.to_string())?;
            zip.write_all(&std::fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    for (name, bytes) in extra {
        zip.start_file(*name, options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
        count += 1;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(count)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}


/// F2 Restore: a backup zip back into place. Only `settings/` and `custom-characters/` entries with plain relative
/// paths are taken. Everything is unpacked beside the live folders first (`*.restoring`); only a complete unpack is
/// swapped in, and the folders it replaces are kept as `*.before-restore` (the ones before that go).
pub fn restore_saves(zip_path: &Path, settings: &Path, characters: &Path) -> Result<usize, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path).map_err(|e| e.to_string())?).map_err(|_| "That is not a saves backup.")?;
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    let safe = |rel: &str| !rel.is_empty() && !rel.contains('\\') && !rel.contains(':') && rel.split('/').all(|p| !p.is_empty() && p != "." && p != "..");
    let mut plan = Vec::new();
    for name in &names {
        if name.ends_with('/') { continue; }
        let target = if let Some(rel) = name.strip_prefix("settings/") { settings.join(rel) }
                     else if let Some(rel) = name.strip_prefix("custom-characters/") { characters.join(rel) }
                     else { return Err("That is not a saves backup.".into()) };
        if !safe(name.split_once('/').unwrap().1) { return Err("That is not a saves backup.".into()); }
        plan.push((name.clone(), target));
    }
    if !plan.iter().any(|(n, _)| n.starts_with("settings/")) { return Err("That backup holds no saves.".into()); }
    let staged = |live: &Path| live.with_extension("restoring");
    let unpacked = (|| -> Result<(), String> {
        for live in [settings, characters] { let _ = std::fs::remove_dir_all(staged(live)); }
        for (name, target) in &plan {
            let live = if target.starts_with(settings) { settings } else { characters };
            let target = staged(live).join(target.strip_prefix(live).unwrap());
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut entry = zip.by_name(name).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut std::fs::File::create(&target).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = unpacked {
        for live in [settings, characters] { let _ = std::fs::remove_dir_all(staged(live)); }
        return Err(e);
    }
    for live in [settings, characters] {
        if !staged(live).is_dir() { continue; }  // a backup without characters leaves them alone
        let before = live.with_extension("before-restore");
        let _ = std::fs::remove_dir_all(&before);
        if live.is_dir() { std::fs::rename(live, &before).map_err(|e| e.to_string())?; }
        if let Err(e) = std::fs::rename(staged(live), live) {
            let _ = std::fs::rename(&before, live);  // put the old ones back
            return Err(e.to_string());
        }
    }
    Ok(plan.len())
}

/// F4: graphics.json (where a setting that crashes the game lives) moved aside to graphics.json.bak. A .bak from an
/// earlier reset is the player's own settings and is kept; the file now is the defaults since, and just goes.
pub fn reset_graphics(settings: &Path) -> bool {
    let (file, bak) = (settings.join("graphics.json"), settings.join("graphics.json.bak"));
    // and the game's last good copy (skate-game saves.rs), or a damaged default would bring the crashing settings back
    let _ = std::fs::remove_file(settings.join("graphics.json.last-good"));
    file.is_file() && if bak.exists() { std::fs::remove_file(&file).is_ok() } else { std::fs::rename(&file, bak).is_ok() }
}

/// Audit 10-24: beside the exe, the player's own songs (skate-game music.rs) and mods (modding::package_root); neither
/// ships in game.zip. The game's own README.txt in one is not the player's.
pub const PLAYER_FOLDERS: [&str; 2] = ["music", "mods"];

/// B4 (release review 10-18): Uninstall deletes the built game but never the player's saves: everything in `game`
/// except `data\settings` (saves.rs in the game) goes, and (audit 10-24) the [`PLAYER_FOLDERS`] holding their files.
/// Returns the folders kept; with none, the game folder goes whole.
pub fn remove_game_keeping_saves(game: &Path) -> Vec<PathBuf> {
    let settings = game.join("data").join("settings");
    let saves = std::fs::read_dir(&settings).is_ok_and(|mut d| d.next().is_some());
    let own: Vec<PathBuf> = PLAYER_FOLDERS.iter().map(|f| game.join(f))
        .filter(|f| std::fs::read_dir(f).is_ok_and(|d| d.flatten().any(|e| e.file_name() != "README.txt"))).collect();
    if !saves && own.is_empty() {
        let _ = std::fs::remove_dir_all(game);
        return vec![];
    }
    let remove = |p: &Path| { let _ = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }; };
    for entry in std::fs::read_dir(game).into_iter().flatten().flatten() {
        if !(entry.file_name() == "data" && saves) && !own.contains(&entry.path()) { remove(&entry.path()); }
    }
    for entry in std::fs::read_dir(game.join("data")).into_iter().flatten().flatten() {
        if entry.file_name() != "settings" { remove(&entry.path()); }
    }
    saves.then_some(settings).into_iter().chain(own).collect()
}

/// Move the game folder (#3). On one drive it is a rename or nothing: a folder in use is never copied instead (50 GB).
/// Across drives it is a copy, each file's size checked, without `data\tmp` and `data\work` (build scratch, re-made by
/// the next build). Ok(true): copied, and the caller removes `from` only after it saved the new place.
pub fn move_tree(from: &Path, to: &Path, same_drive: bool) -> Result<bool, String> {
    if to.exists() { return Err("There is already a game folder there.".into()); }
    if to.starts_with(from) { return Err("Pick a folder outside the game folder.".into()); }
    std::fs::create_dir_all(to.parent().ok_or("bad folder")?).map_err(|e| e.to_string())?;
    if same_drive {
        return std::fs::rename(from, to).map(|_| false).map_err(|_| "Some game files are in use. Close File Explorer windows \
            showing the game folder and any program using it, then try again.".into());
    }
    let skip = [from.join("data").join("tmp"), from.join("data").join("work")];
    let copied = (|| -> Result<(), String> {
        for path in walk(from) {
            if skip.iter().any(|s| path.starts_with(s)) { continue; }
            let target = to.join(path.strip_prefix(from).unwrap());
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            let size = std::fs::copy(&path, &target).map_err(|e| format!("{}: {e}", path.display()))?;
            if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(size) { return Err(format!("{}: the copy differs", path.display())); }
        }
        Ok(())
    })();
    if let Err(e) = copied { let _ = std::fs::remove_dir_all(to); return Err(e); }
    Ok(true)
}

/// Storage (P8/F11): the sizes the last build stored (`data\storage.json`, P), with the drive's free space now.
pub fn storage_message(game: &Path) -> Value {
    let mut m: Value = std::fs::read(game.join("data").join("storage.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
        .filter(Value::is_object).unwrap_or_else(|| json!({}));
    m["type"] = json!("storage");
    if let Some((free, _)) = crate::system::disk_space(game) { m["free"] = json!(free); }
    m["drive"] = json!(crate::system::drive(game));
    m
}

/// Clear one Storage pile. Saves (`settings`) and imported characters are never a pile.
fn clear(game: &Path, what: &str) {
    let data = game.join("data");
    match what {
        "leftovers" => for p in leftovers(&data).into_iter().chain([data.join("trash")]) { let _ = std::fs::remove_dir_all(p); },  // B8: storage.json counts trash here
        "temp" => { empty_tmp(game); sweep_temp(&std::env::temp_dir(), Duration::ZERO); }
        // Audit 10-24: the texture cache too (it had no pile and no way to be cleared).
        "audio" => for cache in ["audio-pcm-cache", "bc7-cache"] { let _ = std::fs::remove_dir_all(local_data().join(cache)); },
        "cache" => for d in ["cache", "convert"] { let _ = std::fs::remove_dir_all(data.join("work").join(d)); },  // D4
        // D4: the next build downloads them again. ffmpeg stays: the game itself decodes its audio, music and movies
        // with it (skate-data audio/ffmpeg.rs), and a finished game never builds again to get it back.
        "tools" => for e in std::fs::read_dir(data.join("tools")).into_iter().flatten().flatten() {
            if e.file_name() != "ffmpeg" { let _ = std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path())); }
        },
        "logs" => {
            let _ = std::fs::remove_dir_all(data.join("logs.previous"));
            for f in [data.join("hero").join("hero.log"), data.join("setup-stderr.log"), local_data().join("launcher.old.log")] {
                let _ = std::fs::remove_file(f);
            }
        }
        "crashes" => for root in [local_data(), std::env::temp_dir().join("Skate3RustEngine")] {
            let _ = std::fs::remove_dir_all(root.join("CrashReports"));
        },
        _ => {}
    }
}

/// F7: one zip for a bug report: the build report, the logs, the launcher's log, the newest crash report and the PC.
/// Folder names are replaced as in Copy report, so it can go into a public issue.
fn support_bundle(app: &App, out: &Path) -> Result<(), String> {
    use std::io::Write;
    let game = app.game();
    let data = game.join("data");
    let sources: Vec<String> = app.folders().iter().map(|f| f.display().to_string()).collect();
    let trim = |text: &str| crate::pipeline::trim_report(text, &game, &sources);
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for dir in ["logs", "logs.previous"] {
        for path in walk(&data.join(dir)) { files.push((format!("{dir}/{}", path.strip_prefix(data.join(dir)).unwrap().to_string_lossy()), path)); }
    }
    for (name, path) in [("setup-stderr.log", data.join("setup-stderr.log")), ("hero.log", data.join("hero").join("hero.log")),
                         ("launcher.log", local_data().join("launcher.log")), ("launcher.old.log", local_data().join("launcher.old.log"))] {
        files.push((name.into(), path));
    }
    let crash = std::fs::read_dir(local_data().join("CrashReports")).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt")).max_by_key(|p| p.file_name().map(|n| n.to_owned()));
    if let Some(crash) = crash { files.push(("newest-crash-report.txt".into(), crash)); }
    let mut zip = zip::ZipWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
    let options = zip::write::SimpleFileOptions::default();
    for (name, path) in files {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let tail = &bytes[bytes.len().saturating_sub(4 << 20)..];  // ponytail: the last 4 MB of a log is the useful part
        zip.start_file(name.replace('\\', "/"), options).map_err(|e| e.to_string())?;
        zip.write_all(trim(&String::from_utf8_lossy(tail)).as_bytes()).map_err(|e| e.to_string())?;
    }
    let pc = format!("Launcher build: {}\nGame build: {}\nWindows: {}\nCPU: {} ({} threads)\nMemory: {}\nGame drive: {} ({} GB free)\n",
        crate::update::version(), crate::update::installed_build(&game).map_or("-".into(), |b| b.to_string()),
        std::env::var("OS").unwrap_or_default(), std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default(),
        std::thread::available_parallelism().map_or(0, |n| n.get()), crate::system::memory_text(),
        crate::system::drive(&game), crate::system::disk_space(&game).map_or(0, |(f, _)| f >> 30));
    zip.start_file("pc.txt", options).map_err(|e| e.to_string())?;
    zip.write_all(pc.as_bytes()).map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn date() -> String {
    let days = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // civil-from-days (Howard Hinnant): yyyy-mm-dd without a date crate
    let z = days + 719468;
    let (era, doe) = (z.div_euclid(146097), z.rem_euclid(146097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

pub fn handle(app: &Arc<App>, command: Command) {
    let game = app.game();
    match command {
        Command::Storage => app.send(storage_message(&game)),
        Command::Clear(what) => {
            if app.refuse_while_busy() { return app.send(storage_message(&game)); }  // the page waits for one to re-enable Clear
            {   // A1: taken under app.running, which spawn re-checks it under, so no build starts beneath the deletion
                let running = app.running.lock().unwrap();
                if running.is_some() || app.clearing.swap(true, Ordering::SeqCst) {
                    drop(running);
                    app.toast("Wait for the clean-up to finish.");
                    return app.send(storage_message(&game));
                }
            }
            app.toast("Clearing…");
            let app = app.clone();
            std::thread::spawn(move || {  // A1: a 14 GB installation takes minutes on an HDD
                clear(&game, &what);
                app.clearing.store(false, Ordering::SeqCst);
                crate::pipeline::measure_storage(&app);
            });
        }
        Command::Shortcut => { std::thread::spawn({ let app = app.clone(); move || {  // A5: off the UI thread
            match shortcut_path().ok_or("No desktop folder.".to_string())
                .and_then(|link| make_shortcut(&link, &std::env::current_exe().map_err(|e| e.to_string())?)) {
                Ok(()) => { app.toast("Shortcut added to the desktop."); app.send(app.settings_message()); }
                Err(e) => { log(&format!("shortcut: {e}")); app.toast(&format!("Could not add the shortcut: {e}")); }
            }
        }}); }
        Command::LongPathsOn => { std::thread::spawn({ let app = app.clone(); move || {  // B9: the UAC prompt can wait a while
            if long_paths_on() { app.toast("Long paths are on."); crate::pipeline::check(&app); }  // the check clears the alert
            else { app.toast("Long paths were not turned on. Use Copy command instead."); }
        }}); }
        Command::SupportBundle => {
            let Some(out) = rfd::FileDialog::new().set_title("Save the support bundle").set_file_name(format!("S3R support {}.zip", date()))
                .add_filter("Zip", &["zip"]).save_file() else { return };
            let app = app.clone();
            std::thread::spawn(move || match support_bundle(&app, &out) {  // A5: the dialog needs the UI thread, the zip doesn't
                Ok(()) => app.toast(&format!("Support bundle saved: {}", out.display())),
                Err(e) => app.toast(&format!("Could not save the support bundle: {e}")),
            });
        }
        Command::BackupSaves => {
            let Some(saves) = saves_dir(&game) else { return app.toast("There are no saves yet.") };
            let Some(out) = rfd::FileDialog::new().set_title("Back up your saves").set_file_name(format!("S3R saves {}.zip", date()))
                .add_filter("Zip", &["zip"]).save_file() else { return };
            let app = app.clone();
            std::thread::spawn(move || {  // A5
                match zip_dirs(&out, &[("settings", saves), ("custom-characters", local_data().join("custom-characters"))], &[]) {
                    Ok(n) => app.toast(&format!("Saves backed up ({n} files): {}", out.display())),
                    Err(e) => app.toast(&format!("Could not back up the saves: {e}")),
                }
            });
        }
        Command::RestoreSaves | Command::ResetGameSettings if app.game_running() => app.toast("Close the game first."),
        Command::RestoreSaves => {
            let Some(zip) = rfd::FileDialog::new().set_title("Restore saves").add_filter("Zip", &["zip"]).pick_file() else { return };
            let settings = saves_dir(&game).unwrap_or_else(|| game.join("data").join("settings"));
            let app = app.clone();
            std::thread::spawn(move || {  // A5
                if app.game_running() { return app.toast("Close the game first."); }  // started while the dialog was open
                match restore_saves(&zip, &settings, &local_data().join("custom-characters")) {
                    Ok(n) => { app.toast(&format!("Saves restored ({n} files). The ones they replaced are in settings.before-restore.")); crate::hero::poke(); }
                    Err(e) => app.toast(&format!("Could not restore the saves: {e}")),
                }
            });
        }
        Command::ResetGameSettings => {
            let done = saves_dir(&game).is_some_and(|s| reset_graphics(&s));
            app.toast(if done { "Graphics settings reset. The old ones are kept as graphics.json.bak." } else { "The graphics settings are already the defaults." });
        }
        Command::MoveGame | Command::Uninstall { .. } if app.refuse_while_busy() => {}
        Command::MoveGame => {
            let Some(picked) = rfd::FileDialog::new().set_title("Move the game to this folder").pick_folder() else { return };
            let to = picked.join("game");
            let app = app.clone();
            std::thread::spawn(move || {
                app.busy_moving.store(true, Ordering::SeqCst);
                app.toast("Moving the game…");
                crate::pipeline::stop_helpers(&app.helpers);
                let same = crate::system::drive(&game).eq_ignore_ascii_case(&crate::system::drive(&to));
                // the new place is saved before anything is deleted, so the launcher never points at a half-removed folder
                let result = move_tree(&game, &to, same).and_then(|copied| {
                    app.settings.lock().unwrap().game = (to != app.dir.join("game")).then_some(to.clone());
                    app.save().map(|_| copied).map_err(|e| format!("the new place could not be saved ({e}); the game is still at {}", game.display()))
                });
                let mut left = Vec::new();
                if let Ok(copied) = result {
                    if copied && std::fs::remove_dir_all(&game).is_err() { left.push(game.clone()); }
                    for tag in ["bak", "download"] {  // an update's backup and download sit beside the game folder
                        let (from, into) = (game.with_extension(tag), to.with_extension(tag));
                        let moved = !from.exists() || move_tree(&from, &into, same)
                            .and_then(|copied| if copied { std::fs::remove_dir_all(&from).map_err(|e| e.to_string()) } else { Ok(()) }).is_ok();
                        if !moved { left.push(from); }
                    }
                }
                app.busy_moving.store(false, Ordering::SeqCst);
                let left: Vec<String> = left.iter().map(|p| p.display().to_string()).collect();
                match result {
                    Ok(_) if left.is_empty() => app.toast(&format!("Game moved to {}", to.display())),
                    Ok(_) => {
                        log(&format!("move game: left behind {left:?}"));
                        app.toast(&format!("Game moved to {}. These could not be moved or removed; delete them once nothing uses them: {}",
                                           to.display(), left.join("; ")));
                    }
                    Err(e) => {
                        if to.exists() && app.game() != to { let _ = std::fs::remove_dir_all(&to); }  // a copy nobody points at
                        log(&format!("move game: {e}"));
                        app.toast(&format!("Could not move the game: {e}"));
                    }
                }
                app.send(location_message(&app));
                app.refresh();
            });
        }
        Command::Uninstall { all } => {
            crate::pipeline::stop_helpers(&app.helpers);
            let kept = remove_game_keeping_saves(&game);
            for path in [game.with_extension("bak"), game.with_extension("new"), game.with_extension("download")] {
                let _ = std::fs::remove_dir_all(path);
            }
            // Audit 10-24: and the game's decoded audio and texture caches (up to 8 + 1 GiB), made again by the next play.
            for cache in ["audio-pcm-cache", "bc7-cache"] {
                let _ = std::fs::remove_dir_all(local_data().join(cache));
            }
            crate::credentials::delete();
            sweep_temp(&std::env::temp_dir(), Duration::ZERO);
            {
                let mut s = app.settings.lock().unwrap();
                s.game = None;
            }
            app.save_or_report();
            log("game data removed");
            let left = std::fs::read_dir(&game).is_ok_and(|d| d.flatten().any(|e| e.file_name() != "data" && !kept.contains(&e.path())))
                || std::fs::read_dir(game.join("data")).is_ok_and(|d| d.flatten().any(|e| e.file_name() != "settings"));
            let kept = kept.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
            app.toast(&if left { "Some files are in use and were left. Close other programs and try again.".to_owned() }
                else if !kept.is_empty() { format!("Game data removed. Your saves and your own files are kept in {kept}") }
                else { "Game data removed.".to_owned() });
            app.send(json!({"type": "key", "saved": false}));
            app.send(app.folders_message());
            app.send(json!({"type": "state", "state": "update", "first": true}));
            // User 10-14 (feature #3): Uninstall everything also takes the launcher, its settings, its local data
            // (logs, crash reports, WebView profile) and the desktop shortcut. The running exe and the WebView's files
            // can't go while the launcher runs, so a hidden PowerShell waits for it to exit and removes them.
            if all {
                if let Some(link) = shortcut_path() { let _ = std::fs::remove_file(link); }
                crate::playtime::forget_key();  // its file goes with local_data
                let exe = std::env::current_exe().unwrap_or_default();
                let script = cleanup_script(std::process::id(), &[exe, app.dir.join("launcher.json")], &local_data_to_remove(&local_data()));
                use std::os::windows::process::CommandExt;
                let spawned = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .creation_flags(0x0800_0000).spawn();
                log(&format!("uninstall everything: cleanup {:?}", spawned.as_ref().map(|c| c.id())));
                std::process::exit(0);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B4: Uninstall takes the built game and leaves the saves; with no saves, the whole folder goes.
    #[test]
    fn uninstall_leaves_the_saves_in_place() {
        let game = std::env::temp_dir().join(format!("s3r-uninstall-{}", std::process::id())).join("game");
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
        for rel in ["skate3rust.exe", "release.json", "data/installation.json", "data/installations/x/assets/a.bin", "data/settings/career.json", "data/settings/mods/m.json",
            "music/README.txt", "music/Artist - Song.mp3", "mods/m.zip"] {
            std::fs::create_dir_all(game.join(rel).parent().unwrap()).unwrap();
            std::fs::write(game.join(rel), rel).unwrap();
        }
        // Audit 10-24: the player's own songs and mods are theirs too.
        assert_eq!(remove_game_keeping_saves(&game), [game.join("data").join("settings"), game.join("music"), game.join("mods")]);
        let mut left: Vec<_> = walk_names(&game);
        left.sort();
        assert_eq!(left, ["data", "data/settings", "data/settings/career.json", "data/settings/mods", "data/settings/mods/m.json",
            "mods", "mods/m.zip", "music", "music/Artist - Song.mp3", "music/README.txt"]);
        assert_eq!(std::fs::read_to_string(game.join("data/settings/career.json")).unwrap(), "data/settings/career.json");
        for p in ["data/settings", "music/Artist - Song.mp3", "mods/m.zip"] {
            let _ = std::fs::remove_dir_all(game.join(p)).or_else(|_| std::fs::remove_file(game.join(p)));
        }
        // The game's own music README is no song.
        assert!(remove_game_keeping_saves(&game).is_empty());
        assert!(!game.exists(), "nothing of the player's: the folder goes");
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
    }
    /// Audit 10-24: "Uninstall everything" deleted the whole local data folder, imported characters too, which Back up
    /// saves treats as saves. They stay now; with none, the folder goes whole.
    #[test]
    fn uninstall_everything_keeps_imported_characters() {
        let local = std::env::temp_dir().join(format!("s3r-uninstall-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&local);
        for rel in ["webview/x.dat", "logs/launcher.log", "custom-characters/crab.glb", "playtime.key"] {
            std::fs::create_dir_all(local.join(rel).parent().unwrap()).unwrap();
            std::fs::write(local.join(rel), rel).unwrap();
        }
        let mut gone = local_data_to_remove(&local);
        gone.sort();
        assert_eq!(gone, [local.join("logs"), local.join("playtime.key"), local.join("webview")]);
        std::fs::remove_file(local.join("custom-characters/crab.glb")).unwrap();
        assert_eq!(local_data_to_remove(&local), [local.clone()]);
        let _ = std::fs::remove_dir_all(&local);
    }
    /// Clearing the build tools keeps ffmpeg, which the finished game plays its sound with.
    #[test]
    fn clearing_build_tools_keeps_ffmpeg() {
        let game = std::env::temp_dir().join(format!("s3r-clear-tools-{}", std::process::id())).join("game");
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
        for rel in ["data/tools/ffmpeg/bin/ffmpeg.exe", "data/tools/java/bin/java.exe", "data/tools/xenia-key.download"] {
            std::fs::create_dir_all(game.join(rel).parent().unwrap()).unwrap();
            std::fs::write(game.join(rel), rel).unwrap();
        }
        clear(&game, "tools");
        let mut left = walk_names(&game.join("data/tools"));
        left.sort();
        assert_eq!(left, ["ffmpeg", "ffmpeg/bin", "ffmpeg/bin/ffmpeg.exe"]);
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
    }
    fn walk_names(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                out.push(e.path().strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                if e.path().is_dir() { stack.push(e.path()); }
            }
        }
        out
    }

    #[test]
    fn uninstall_everything_quotes_its_paths() {
        let s = cleanup_script(42, &[PathBuf::from(r"C:\it's\l.exe")], &[PathBuf::from(r"C:\x y")]);
        assert_eq!(s, r"Wait-Process -Id 42 -Timeout 30 -EA 0;Start-Sleep 1;Remove-Item -LiteralPath 'C:\it''s\l.exe' -Force -EA 0;Remove-Item -LiteralPath 'C:\x y' -Recurse -Force -EA 0");
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s3r-m-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn set_age(path: &Path, days: u64) {
        let when = SystemTime::now() - DAY * days as u32;
        let f = std::fs::File::options().write(true).open(path).or_else(|_| {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::File::options().write(true).custom_flags(0x0200_0000).open(path)  // FILE_FLAG_BACKUP_SEMANTICS: a folder
        }).unwrap();
        f.set_modified(when).unwrap();
    }

    #[test]
    fn the_temp_sweep_takes_only_our_old_folders_and_never_one_in_use() {
        let t = temp("sweep");
        for d in ["_MEI1234/tools/pipeline", "_MEI9999/other", "s3r-fanout-abc", "skate-online-42", "someone-else", "s3r-fanout-new", "skate-online-held"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        for d in ["_MEI1234", "_MEI9999", "s3r-fanout-abc", "skate-online-42", "someone-else", "skate-online-held"] { set_age(&t.join(d), 3); }
        std::fs::write(t.join("skate-online-held/look.json"), b"x").unwrap();
        set_age(&t.join("skate-online-held"), 3);
        let held = std::fs::File::open(t.join("skate-online-held/look.json")).unwrap();
        assert_eq!(sweep_temp(&t, DAY), 3);
        let left: std::collections::BTreeSet<String> = std::fs::read_dir(&t).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["_MEI9999", "s3r-fanout-new", "skate-online-held", "someone-else"].map(String::from).into(),
                   "another app's _MEI, a young folder, a folder in use and a stranger's folder stay");
        drop(held);
        std::fs::remove_dir_all(t).unwrap();
    }

    #[test]
    fn abandoned_build_files_expire_after_a_week_and_never_the_named_installation_or_unmoved_saves() {
        let data = temp("expire").join("data");
        for d in ["work/discs/s3", "work/dlc/x", "installations/aaa/settings", "installations/old", "installations/saves/settings", "logs"] {
            std::fs::create_dir_all(data.join(d)).unwrap();
        }
        std::fs::write(data.join("installations/saves/settings/character.json"), b"{}").unwrap();
        std::fs::write(data.join("installation.json"), r#"{"directory":"installations/aaa"}"#).unwrap();
        std::fs::write(data.join("logs/mod-maps.log"), b"failed").unwrap();
        let idle = std::sync::Mutex::new(None);
        assert_eq!(expire_abandoned(&data, SystemTime::now(), &idle), 0, "a recent failure keeps its resume files");
        assert_eq!(expire_abandoned(&data, SystemTime::now() + DAY * 8, &std::sync::Mutex::new(Some(1))), 0, "never under a build");
        let later = SystemTime::now() + DAY * 8;
        assert_eq!(expire_abandoned(&data, later, &idle), 3, "discs, dlc and the unnamed installation without saves");
        assert!(data.join("installations/aaa").is_dir() && data.join("installations/saves/settings/character.json").is_file());
        assert!(!data.join("work/discs").exists() && !data.join("installations/old").exists());
        std::fs::create_dir_all(data.join("settings")).unwrap();  // F2 moved the saves: the old copy can go now
        std::fs::create_dir_all(data.join("work/discs")).unwrap();
        assert_eq!(expire_abandoned(&data, later, &idle), 2);
        assert_eq!(expire_abandoned(&data, later, &idle), 0, "nothing left: the build finished or was cleaned");
        let _ = std::fs::remove_dir_all(data.parent().unwrap());  // the trash thread may still be at it
    }

    /// A6: a folder leaves its place at once (under a lock), and the slow delete happens later on a thread.
    #[test]
    fn trash_moves_at_once_and_empties_later() {
        let data = temp("trash");
        std::fs::create_dir_all(data.join("tmp/run")).unwrap();
        std::fs::write(data.join("tmp/run/a.bin"), vec![0u8; 1 << 16]).unwrap();
        assert!(to_trash(&data, &data.join("tmp")) && !data.join("tmp").exists());
        assert!(!to_trash(&data, &data.join("missing")));
        empty_trash(&data);
        let start = std::time::Instant::now();
        while std::fs::read_dir(data.join("trash")).unwrap().next().is_some() {
            assert!(start.elapsed() < Duration::from_secs(10), "the trash was never emptied");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A5: the shortcut is made in-process, with no PowerShell.
    #[test]
    fn a_shortcut_is_made_without_powershell() {
        let dir = temp("shortcut");
        let link = dir.join("Skate 3 Remaster.lnk");
        make_shortcut(&link, &std::env::current_exe().unwrap()).unwrap();
        assert!(std::fs::metadata(&link).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A8: only "directory" is read; the receipts beside it are skipped.
    #[test]
    fn the_named_installation_is_read_without_the_receipts() {
        let data = temp("marker");
        std::fs::write(data.join("installation.json"),
                       r#"{"version":3,"outputs":{"stages":{"a":{"x.bin":{"size":1,"sha256":"00"}}}},"directory":"installations/aaa"}"#).unwrap();
        assert_eq!(named_installation(&data).as_deref(), Some("installations/aaa"));
        std::fs::write(data.join("installation.json.bak"), r#"{"directory":"installations/bbb"}"#).unwrap();
        std::fs::write(data.join("installation.json"), br#"{"directory":"install"#).unwrap();  // B5: cut short
        assert_eq!(named_installation(&data).as_deref(), Some("installations/bbb"));
        std::fs::write(data.join("installation.json"), b"{}").unwrap();
        assert_eq!(named_installation(&data), None, "a whole marker without a directory is believed");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_bad_place_for_the_game_is_named() {
        let env = |v: &str| match v {
            "TEMP" => Some(r"C:\Users\Al\AppData\Local\Temp".into()), "ProgramFiles" => Some(r"C:\Program Files".into()),
            "OneDrive" => Some(r"C:\Users\Al\OneDrive".into()), _ => None,
        };
        let at = |p: &str, w: bool| location_problem(Path::new(p), w, env);
        assert_eq!(at(r"C:\Users\Al\AppData\Local\Temp\Temp1_S3R.zip\Skate 3 Remaster\game", true), Some("temp"));
        assert_eq!(at(r"C:\Program Files\Skate 3 Remaster\game", true), Some("program_files"));
        assert_eq!(at(r"D:\Games\S3R\game", false), Some("readonly"));
        assert_eq!(at(r"C:\Users\Al\OneDrive\Desktop\S3R\game", true), Some("onedrive"));
        assert_eq!(at(&format!(r"D:\{}\game", "x".repeat(100)), true), Some("long"));
        assert_eq!(at(r"C:\Games\Skate 3 Remaster\game", true), None);
        assert_eq!(at(r"C:\Program Files (x86)x\game", true), Some("program_files"), "a prefix match, as Windows paths go");
    }

    #[test]
    fn saves_back_up_and_restore_and_a_strange_zip_is_refused() {
        let t = temp("saves");
        let (settings, chars) = (t.join("data/settings"), t.join("custom-characters"));
        std::fs::create_dir_all(settings.join("mods")).unwrap();
        std::fs::create_dir_all(&chars).unwrap();
        std::fs::write(settings.join("character.json"), b"me").unwrap();
        std::fs::write(settings.join("mods/list.json"), b"[]").unwrap();
        std::fs::write(chars.join("crab.glb"), b"glb").unwrap();
        let zip = t.join("backup.zip");
        assert_eq!(zip_dirs(&zip, &[("settings", settings.clone()), ("custom-characters", chars.clone())], &[]).unwrap(), 3);
        std::fs::write(settings.join("character.json"), b"changed").unwrap();
        std::fs::remove_file(chars.join("crab.glb")).unwrap();
        assert_eq!(restore_saves(&zip, &settings, &chars).unwrap(), 3);
        assert_eq!(std::fs::read(settings.join("character.json")).unwrap(), b"me");
        assert_eq!(std::fs::read(settings.join("mods/list.json")).unwrap(), b"[]");
        assert_eq!(std::fs::read(chars.join("crab.glb")).unwrap(), b"glb");
        assert_eq!(std::fs::read(settings.with_extension("before-restore").join("character.json")).unwrap(), b"changed");
        let evil = t.join("evil.zip");
        zip_dirs(&evil, &[], &[("settings/../../x.exe", b"x".to_vec())]).unwrap();
        assert!(restore_saves(&evil, &settings, &chars).is_err());
        zip_dirs(&evil, &[], &[("Windows/x.dll", b"x".to_vec())]).unwrap();
        assert!(restore_saves(&evil, &settings, &chars).is_err());
        assert_eq!(std::fs::read(settings.join("character.json")).unwrap(), b"me", "a refused zip changes nothing");
        assert!(reset_graphics(&settings) == false);
        std::fs::write(settings.join("graphics.json"), b"{\"fsr\":true}").unwrap();
        assert!(reset_graphics(&settings) && !settings.join("graphics.json").exists() && settings.join("graphics.json.bak").is_file());
        std::fs::remove_dir_all(t).unwrap();
    }

    #[test]
    fn the_game_folder_moves_whole_without_its_scratch() {
        let t = temp("move");
        let from = t.join("a/game");
        for d in ["data/settings", "data/tmp/x", "data/work/discs"] { std::fs::create_dir_all(from.join(d)).unwrap(); }
        std::fs::write(from.join("skate3rust.exe"), b"exe").unwrap();
        std::fs::write(from.join("data/settings/c.json"), b"me").unwrap();
        std::fs::write(from.join("data/work/discs/big.iso"), b"iso").unwrap();
        assert!(move_tree(&from, &from.join("inside"), true).is_err());
        // #3: on one drive a folder in use is never copied (50 GB): the player closes what holds it
        let held = std::fs::File::open(from.join("data/settings/c.json")).unwrap();
        let to = t.join("b/game");
        let err = move_tree(&from, &to, true).unwrap_err();
        assert!(err.contains("in use"), "{err}");
        assert!(!to.exists() && from.join("data/settings/c.json").is_file());
        // across drives: a verified copy without the scratch; the old folder goes only after the setting is saved
        assert_eq!(move_tree(&from, &to, false), Ok(true));
        assert!(from.join("data/settings/c.json").is_file(), "the caller removes the old folder");
        assert!(to.join("data/settings/c.json").is_file() && !to.join("data/work").exists() && !to.join("data/tmp").exists());
        drop(held);
        std::fs::remove_dir_all(&to).unwrap();
        assert_eq!(move_tree(&from, &to, true), Ok(false));  // same drive: a rename, everything comes along
        assert!(!from.exists() && to.join("data/settings/c.json").is_file());
        assert!(move_tree(&to, &to, true).is_err(), "there is a game there already");
        std::fs::remove_dir_all(t).unwrap();
    }

    #[test]
    fn the_launcher_log_rolls_over_at_1_mb() {
        let t = temp("log");
        let log = t.join("launcher.log");
        std::fs::write(&log, vec![b'x'; (1 << 20) + 1]).unwrap();
        log_to(&log, "download failed");
        assert!(t.join("launcher.old.log").is_file());
        assert!(std::fs::read_to_string(&log).unwrap().ends_with("download failed\n"));
        std::fs::remove_dir_all(t).unwrap();
    }

    #[test]
    fn small_helpers() {
        assert_eq!(ps_quote("C:\\Al's games\\x"), "'C:\\Al''s games\\x'");
        assert_eq!(date().len(), 10);
        assert!(local_data().starts_with(std::env::temp_dir()), "H1: tests never write the player's launcher.log");
        assert!(shortcut_path().is_some_and(|p| p.ends_with("Skate 3 Remaster.lnk")));
    }

    #[test]
    fn a_build_whose_logs_were_rotated_away_is_not_abandoned() {
        let data = temp("rotated").join("data");
        std::fs::create_dir_all(data.join("work/discs/s3")).unwrap();  // extracting now
        std::fs::create_dir_all(data.join("logs")).unwrap();           // P5 just renamed logs\ to logs.previous\
        std::fs::write(data.join("installation.json"), r#"{"directory":"installations/aaa"}"#).unwrap();
        set_age(&data.join("installation.json"), 30);
        assert_eq!(expire_abandoned(&data, SystemTime::now(), &std::sync::Mutex::new(None)), 0);
        assert!(data.join("work/discs/s3").is_dir());
        std::fs::remove_dir_all(data.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_interrupted_update_is_undone_once_per_launcher() {
        let game = temp("recover-once").join("game");
        let interrupted = || {
            std::fs::create_dir_all(crate::update::backup_dir(&game)).unwrap();
            std::fs::create_dir_all(&game).unwrap();
            std::fs::write(crate::update::backup_dir(&game).join("a.bin"), b"old").unwrap();
        };
        let done = std::sync::atomic::AtomicBool::new(false);
        interrupted();
        assert!(recover_once(&game, &done));
        interrupted();
        assert!(!recover_once(&game, &done), "a page reload mid-install must not undo the install");
        std::fs::remove_dir_all(game.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_restore_that_fails_half_way_changes_nothing_and_characters_are_kept_too() {
        let t = temp("restore-fail");
        let (settings, chars) = (t.join("settings"), t.join("custom-characters"));
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::create_dir_all(&chars).unwrap();
        std::fs::write(settings.join("character.json"), b"mine").unwrap();
        std::fs::write(chars.join("crab.glb"), b"old crab").unwrap();
        let bad = t.join("bad.zip");  // the second entry can't be written: x.json is a file, not a folder
        zip_dirs(&bad, &[], &[("settings/x.json", b"{}".to_vec()), ("settings/x.json/y", b"{}".to_vec())]).unwrap();
        assert!(restore_saves(&bad, &settings, &chars).is_err());
        assert_eq!(std::fs::read(settings.join("character.json")).unwrap(), b"mine");
        assert!(!settings.join("x.json").exists(), "nothing half-restored");
        let left: Vec<String> = std::fs::read_dir(&t).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(left.iter().all(|n| !n.contains("restoring")), "{left:?}");
        let good = t.join("good.zip");
        zip_dirs(&good, &[], &[("settings/character.json", b"backup".to_vec()), ("custom-characters/crab.glb", b"new crab".to_vec())]).unwrap();
        restore_saves(&good, &settings, &chars).unwrap();
        assert_eq!(std::fs::read(chars.join("crab.glb")).unwrap(), b"new crab");
        assert_eq!(std::fs::read(chars.with_extension("before-restore").join("crab.glb")).unwrap(), b"old crab");
        std::fs::remove_dir_all(t).unwrap();
    }

    #[test]
    fn a_second_reset_keeps_the_first_backup() {
        let t = temp("reset-twice");
        std::fs::write(t.join("graphics.json"), b"the player's").unwrap();
        assert!(reset_graphics(&t));
        std::fs::write(t.join("graphics.json"), b"defaults the game wrote").unwrap();
        assert!(reset_graphics(&t));
        assert_eq!(std::fs::read(t.join("graphics.json.bak")).unwrap(), b"the player's");
        assert!(!t.join("graphics.json").exists());
        std::fs::remove_dir_all(t).unwrap();
    }
}
