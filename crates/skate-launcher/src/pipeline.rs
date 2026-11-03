use crate::app::{App, Command};
use serde_json::{json, Value};
use std::{io::BufRead, os::windows::process::CommandExt, path::Path, process::{Command as Proc, Stdio}, sync::Arc};

const NO_WINDOW: u32 = 0x08000000;

pub fn parse_line(line: &str) -> Option<Value> {
    let mut v: Value = serde_json::from_str(line).ok()?;
    let kind = v.get("event")?.as_str()?.to_owned();
    v["type"] = json!(kind);
    Some(v)
}

/// Replace a folder with a placeholder in every spelling a log can hold it in: as written, with doubled
/// backslashes (a Python list repr) and with forward slashes. An empty folder is skipped (it would match everywhere).
fn trim_path(text: &str, folder: &str, placeholder: &str) -> String {
    if folder.is_empty() { return text.to_owned(); }
    [folder.to_owned(), folder.replace('\\', "\\\\"), folder.replace('\\', "/")]
        .iter().fold(text.to_owned(), |text, spelling| text.replace(spelling.as_str(), placeholder))
}

/// The text Copy report puts on the clipboard. The game folder, every source folder (whatever drive it is on)
/// and the home folder are replaced by placeholders, so a report can be pasted into a public issue.
/// `text` with the game folder, every source folder and the home folder replaced by placeholders.
pub fn trim_report(text: &str, game: &Path, sources: &[String]) -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let s = trim_path(text, &game.display().to_string(), "<game>");
    let s = sources.iter().enumerate().fold(s, |s, (i, folder)| trim_path(&s, folder, &format!("<sources {}>", i + 1)));
    trim_path(&s, &home, "~")
}

pub fn report_text(error: &Value, log_tail: &str, game: &Path, sources: &[String], game_build: Option<u64>) -> String {
    let trim = |s: &str| trim_report(s, game, sources);
    format!("S3R build report\nLauncher build: {}\nGame build: {}\nReason: {}\nStage: {}\n\nLog (last lines):\n{}\n",
            crate::update::version(), game_build.map_or("-".to_owned(), |b| b.to_string()),
            trim(error["message"].as_str().unwrap_or("")), error["stage"].as_str().unwrap_or("-"), trim(log_tail))
}

/// The text Copy build report puts on the clipboard: the pipeline's logs/build-times.json report (how long the build
/// and each step took, the PC's CPU, memory and disks; no folder names) under the launcher and game builds.
pub fn times_text(report: &str, game_build: Option<u64>) -> String {
    format!("S3R build times
Launcher build: {}
Game build: {}
{report}
",
            crate::update::version(), game_build.map_or("-".to_owned(), |b| b.to_string()))
}

/// The pipeline's arguments for a build. Without `--resume` it runs every stage again (tools/pipeline/runner.py).
fn build_args(fresh: bool) -> Vec<String> { if fresh { vec![] } else { vec!["--resume".into()] } }

/// #12: a tool that dies before it can report (a quarantined DLL, a Python that can't start) leaves its reason only in
/// data\setup-stderr.log, so the message names it and the usual cause.
const SETUP_CRASH_HELP: &str = " Its own error is in the game folder's data\\setup-stderr.log. Antivirus often causes \
    this: allow skate3setup.exe (in support\\skate3setup), then use Check and repair in Settings.";

/// What the page hears when the helper exits: nothing if it already reported a problem or we killed it.
/// A build that exits 0 without a `done` event did not finish (`missing_done`).
fn unexpected_exit(code: Option<i32>, saw_error: bool, missing_done: bool, cancelled: bool) -> Option<Value> {
    if saw_error || cancelled || (code == Some(0) && !missing_done) { return None; }
    let message = match code {
        Some(0) => "The build tool ended without finishing the build.".to_owned(),
        Some(c) => format!("The build tool stopped unexpectedly (exit code {c}).{SETUP_CRASH_HELP}"),
        None => format!("The build tool stopped unexpectedly (exit code unknown).{SETUP_CRASH_HELP}"),
    };
    Some(json!({"event": "error", "type": "error", "stage": null, "kind": "failed", "log": null, "message": message}))
}

/// The setup tool: a folder since B1 (`support\skate3setup\skate3setup.exe`, nothing to unpack per start), the
/// single exe before it (a game installed by an older release, until its update lands).
pub fn setup_exe(game: &Path) -> std::path::PathBuf {
    let onedir = game.join("support").join("skate3setup").join("skate3setup.exe");
    if onedir.is_file() { onedir } else { game.join("support").join("skate3setup.exe") }
}

/// The launcher's own temp folder on the game's drive (#2/P2): every run's TMP and TEMP, emptied when nothing runs.
pub fn tmp_dir(game: &Path) -> std::path::PathBuf { game.join("data").join("tmp") }

const BELOW_NORMAL_PRIORITY: u32 = 0x0000_4000;

/// Where the setup tool's own stderr goes (#12): a bootloader that can't unpack, a Python that can't start. Outside
/// `data\logs` on purpose: a running build holds this file open, and the pipeline renames `logs` at build start (P5).
/// Over 1 MB it starts again (when no run holds it).
fn stderr_log(game: &Path) -> Stdio {
    let path = game.join("data").join("setup-stderr.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) { let _ = std::fs::remove_file(&path); }
    let _ = std::fs::create_dir_all(game.join("data"));
    std::fs::OpenOptions::new().create(true).append(true).open(&path).map_or_else(|_| Stdio::null(), Stdio::from)
}

/// The environment and priority a run gets: TMP/TEMP on the game drive (#2), the "use fewer resources" caps (#19),
/// and below-normal priority for a build when the player keeps that on (#7; its children inherit it).
pub fn run_setup(game: &Path, build: bool, low_priority: bool, fewer: bool) -> (Vec<(&'static str, std::ffi::OsString)>, u32) {
    let tmp = tmp_dir(game).into_os_string();
    let mut env = vec![("TMP", tmp.clone()), ("TEMP", tmp)];
    if build && fewer { env.extend([("SKATE_BUILD_JOBS", "2".into()), ("SKATE_FANOUT", "2".into())]); }
    (env, NO_WINDOW | if build && low_priority { BELOW_NORMAL_PRIORITY } else { 0 })
}

/// Run `skate3setup.exe --pipeline ARGS` and stream its JSON lines to `on_line`. The build is tracked in `app.running`
/// (Cancel and closing the window stop it), a check or scan in `app.helpers` (an update stops them). All of them are in
/// the job that closes with the launcher.
fn spawn(app: &Arc<App>, args: Vec<String>, build: bool, on_line: impl Fn(&Arc<App>, Value) + Send + 'static) {
    // `--verify` holds the build lock and is stopped by Stop, so it is tracked like a build (app.running)
    let verify = args.first().is_some_and(|a| a == "--verify");
    let tracked = build || verify;
    if tracked && app.running.lock().unwrap().is_some() {
        return app.send(json!({"type": "toast", "text": "The build is already running."}));
    }
    let game = app.game();
    let settings = app.settings.lock().unwrap().clone();
    let (env, flags) = run_setup(&game, build, settings.low_priority, settings.fewer_resources);
    let _ = std::fs::create_dir_all(tmp_dir(&game));
    let mut command = Proc::new(setup_exe(&game));
    command.arg("--pipeline").args(&args).arg("--base").arg(game.join("data")).arg("--game-exe").arg(game.join("skate3rust.exe"));
    for folder in &settings.folders { command.arg("--sources").arg(folder); }
    command.envs(env);
    // both locks while it starts, so empty_tmp_when_idle never empties the folder under a run that is starting
    let mut running = app.running.lock().unwrap();
    let mut helpers = app.helpers.lock().unwrap();
    // A7: checked again under the lock: Verify's repair, the automatic rebuild and the player can all get here at once.
    // A1: and never under a clear (it sets `clearing`, then looks at `running`)
    if tracked && (running.is_some() || app.clearing.load(std::sync::atomic::Ordering::SeqCst)) {
        drop((running, helpers));
        return app.send(json!({"type": "toast", "text": "The build is already running."}));
    }
    if tracked && crate::update::installing() {  // under the lock update::begin_install takes: never both
        drop((running, helpers));
        app.toast("Finishing the update first.");
        return check(app);
    }
    let mut child = match command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(stderr_log(&game)).creation_flags(flags).spawn() {
        Ok(child) => child,
        Err(e) => {
            drop((running, helpers));
            let error = json!({"event": "error", "type": "error", "stage": null, "kind": "failed", "log": null,
                               "message": format!("Could not start the build tool: {e}")});
            *app.last_error.lock().unwrap() = Some(error.clone());
            return app.send(page_error(app, error));
        }
    };
    let pid = child.id();
    dies_with_launcher(&child);
    if tracked { *running = Some(pid); } else { helpers.push(pid); }
    drop((running, helpers));
    if build { let _ = app.proxy.lock().unwrap().send_event(json!({"building": true})); }  // E1: WebView2 memory Low
    let app = app.clone();
    std::thread::spawn(move || {
        if build && settings.keep_awake { crate::system::keep_awake(true); }  // this thread lives as long as the build
        let (mut saw_error, mut saw_done) = (false, false);
        // B6: a line that is not UTF-8 (a stray print in the ANSI code page) is read on, never the end of reading: an
        // undrained pipe would block the tool on its next print
        for line in std::io::BufReader::new(child.stdout.take().unwrap()).split(b'\n').map_while(Result::ok) {
            if let Some(event) = parse_line(&String::from_utf8_lossy(&line)) {
                saw_error |= event["type"] == "error";
                saw_done |= event["type"] == "done";
                if build { taskbar(&app, &event); }
                on_line(&app, event);
            }
        }
        let code = child.wait().ok().and_then(|status| status.code());
        released(pid);
        if build { crate::system::keep_awake(false); }
        // Closing the window takes the pid first; a newer run's pid must survive this cleanup. Stop (A2) leaves the pid
        // in `running` until here, so nothing new starts while the old tree still holds build.lock.
        let mut stopped = false;
        let cancelled = if tracked { let mut running = app.running.lock().unwrap();  // Cancel sets `stopping` under it too
                                   stopped = app.stopping.compare_exchange(pid, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok();
                                   let ours = *running == Some(pid);
                                   if ours { *running = None; }
                                   !ours || stopped } else { !finished(&app.helpers, pid) };
        if let Some(error) = unexpected_exit(code, saw_error, build && !saw_done, cancelled) {
            if tracked { if build { taskbar(&app, &error); } on_line(&app, error); }  // kept like the tool's own errors
            else { *app.last_error.lock().unwrap() = Some(error.clone()); app.send(page_error(&app, error)); }
        }
        // the tool's files and build lock are free now: build() restores a backup on it, Verify starts the repair on it
        if stopped { app.send(json!({"type": "state", "state": "ready"})); }  // A2: Build works from here (Verify's check follows)
        if tracked { on_line(&app, json!({"type": "exited", "cancelled": cancelled})); }
        if build && cancelled { let _ = app.proxy.lock().unwrap().send_event(json!({"taskbar": null})); }
        if build { crate::hero::poke(); let _ = app.proxy.lock().unwrap().send_event(json!({"building": false})); }
        empty_tmp_when_idle(&app);
    });
}

/// The taskbar button follows the build (F6): its progress, red on an error, and a flash when it ends.
pub fn taskbar_message(event: &Value) -> Option<Value> {
    Some(match event["type"].as_str()? {
        "progress" if event["percent"].is_number() => json!({"taskbar": event["percent"]}),
        "stage" if event["total"].as_f64().is_some_and(|t| t > 0.0) =>
            json!({"taskbar": 100.0 * event["index"].as_f64().unwrap_or(0.0) / event["total"].as_f64().unwrap()}),
        "done" => json!({"taskbar": null, "flash": true}),
        "error" => json!({"taskbar": "error", "flash": true}),
        _ => return None,
    })
}
fn taskbar(app: &Arc<App>, event: &Value) {
    if let Some(m) = taskbar_message(event) { let _ = app.proxy.lock().unwrap().send_event(m); }
}

/// The page's form of a build error: `log` says whether there are logs to open (#12); the pipeline's log path stays
/// in `last_error` for Copy report.
fn page_error(app: &App, mut error: Value) -> Value {
    error["log"] = json!(app.game().join("data").join("logs").is_dir());
    error
}

/// A4: the last 40 lines of a log, from its last 64 KiB only (logs can be tens of MB), lossy so a line in the ANSI
/// code page doesn't lose the whole tail.
fn log_tail(path: &std::path::Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let start = file.metadata().map_or(0, |m| m.len().saturating_sub(64 << 10));
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_to_end(&mut bytes)).is_err() { return String::new(); }
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 { text.split_once('\n').map_or("", |(_, rest)| rest) } else { &text };  // a cut first line
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

/// Empty `data\tmp` once no build, check or scan of ours runs (P2). Both locks: nothing can start meanwhile.
pub fn empty_tmp_when_idle(app: &App) {
    let (game, data) = (app.game(), app.game().join("data"));
    {
        let running = app.running.lock().unwrap();
        let helpers = app.helpers.lock().unwrap();
        if running.is_some() || !helpers.is_empty() { return; }
        let tmp = tmp_dir(&game);
        // A6: a rename under the locks, the delete after them. ponytail: a tmp that can't move (a file open in it)
        // is emptied in place, under the locks, as before
        if std::fs::read_dir(&tmp).is_ok_and(|mut d| d.next().is_some()) {
            if crate::maintenance::to_trash(&data, &tmp) { let _ = std::fs::create_dir_all(&tmp); }
            else { crate::maintenance::empty_tmp(&game); }
        }
    }
    crate::maintenance::empty_trash(&data);  // B8, launcher half: also what the tool put there before "done"
}

/// After an automatic update the player chose not to press anything, so the check's answer starts the rebuild the
/// update needs ("ready"). The request is used up by the first answer, whatever it is: "play" means nothing to rebuild.
fn rebuild_now(state: &Value, requested: &std::sync::atomic::AtomicBool) -> bool {
    requested.swap(false, std::sync::atomic::Ordering::SeqCst) && state["state"] == "ready"
}

pub fn check(app: &Arc<App>) {
    spawn(app, vec!["--check".into()], false, |app, e| {
        let rebuild = e["type"] == "state" && rebuild_now(&e, &app.build_after_check);
        if e["type"] == "state" {
            *app.last_check.lock().unwrap() = e.clone();
            // #9: an installed update that needs no build is done: its program-file backup has served its purpose
            if e["state"] == "play" { crate::update::drop_backup(&app.game()); }
            remember_check(&crate::maintenance::local_data().join("last-check.json"), &app.game(), &e);
        }
        app.send(e);
        if rebuild && !app.refuse_while_busy() {
            app.send(json!({"type": "state", "state": "building"}));
            build(app, build_args(false));
        }
    });
}

/// B7: a drive root or the user profile itself as a source folder: the scan walks all of it on every check.
fn too_broad(folder: &Path, profile: Option<std::path::PathBuf>) -> bool {
    folder.parent().is_none() || profile.is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(folder.as_os_str().to_string_lossy().trim_end_matches('\\')))
}

/// A3: what a "play" answer was about: the installed build and when installation.json was last written.
fn check_key(game: &Path) -> Value {
    let written = std::fs::metadata(game.join("data").join("installation.json")).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64);
    json!([crate::update::installed_build(game), written])
}

/// A3: keep the check's "play" answer (anything else forgets it), so the next start can show Play at once.
fn remember_check(file: &Path, game: &Path, state: &Value) {
    if state["state"] == "play" { let _ = std::fs::write(file, json!({"key": check_key(game), "state": state}).to_string()); }
    else { let _ = std::fs::remove_file(file); }
}

/// A3: the last "play" answer while the game is still the one it was about. App::refresh shows it before the real
/// check answers (and never while an update installs or a build runs).
pub fn remembered_check(file: &Path, game: &Path) -> Option<Value> {
    let saved: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    (saved["key"] == check_key(game) && saved["state"]["state"] == "play").then(|| saved["state"].clone())
}

/// A build (or a `--verify` repair): its errors are kept for Copy report, a finished one makes the game playable, and
/// an update's program-file backup is kept until then (#9): dropped on `done`, put back after an error. The error waits
/// for the tool to exit, because the backup holds the tool's own files, which can't be replaced while it runs.
fn build(app: &Arc<App>, args: Vec<String>) {
    let failed: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);
    spawn(app, args, true, move |app, e| {
        if e["type"] == "error" {
            *app.last_error.lock().unwrap() = Some(e.clone());
            failed.lock().unwrap().get_or_insert(e);
            return;
        }
        if e["type"] == "exited" {
            let Some(mut e) = failed.lock().unwrap().take() else { return };
            if crate::update::restore_backup(&app.game()) {
                e["message"] = json!(format!("{}\n\nThe update was undone. If Play shows, you can keep playing the build you \
                                              had; if Build shows, the game needs a build first. Press Update to try the \
                                              update again.", e["message"].as_str().unwrap_or("")));
                let installed = crate::update::installed_build(&app.game());
                app.send(json!({"type": "update", "available": true, "installed": installed, "quiet": true}));
            }
            let restored = e["message"].as_str().is_some_and(|m| m.contains("The update was undone"));
            app.send(page_error(app, e));
            if restored { check(app); }  // the build that was undone is complete again: Play, not Resume
            return;
        }
        if e["type"] == "done" {
            crate::update::drop_backup(&app.game());
            app.send(json!({"type": "state", "state": "play"}));
        }
        app.send(e);
    });
}

/// After a clear: the pipeline walks the piles again (`--storage`, P) and its `storage` event goes to the page as is.
/// Without the setup tool (no game yet) the stored sizes answer.
pub fn measure_storage(app: &Arc<App>) {
    let game = app.game();
    if !setup_exe(&game).is_file() { return app.send(crate::maintenance::storage_message(&game)); }
    spawn(app, vec!["--storage".into()], false, |app, e| if e["type"] == "storage" { app.send(e) });
}

/// A failed check-and-repair is a message (the page toasts `scope` errors), not a build that failed.
pub fn verify_error(e: &Value) -> Value { json!({"type": "error", "scope": "verify", "message": e["message"]}) }

/// #5: the pipeline's `verify` answer with the launcher's own check of the program files added in.
pub fn merge_verify(mut e: Value, program: &Value) -> Value {
    let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
    e["files"] = json!(n(&e, "files") + n(program, "files"));
    e["bad"] = json!(n(&e, "bad") + n(program, "bad"));
    e["ok"] = json!(e["ok"].as_bool().unwrap_or(true) && program["ok"].as_bool().unwrap_or(true));
    if let Some(note) = program["message"].as_str().filter(|m| !m.is_empty()) {
        e["message"] = json!(format!("{note} {}", e["message"].as_str().unwrap_or("")).trim().to_owned());
    }
    e
}

/// Put every run in a job that Windows closes with the launcher's last handle, so it and everything it starts stop even
/// when the launcher crashes or is ended from Task Manager (Cancel and a normal close use `kill`). A build left running
/// would race the next one over the same files; a check left running kept support/skate3setup.exe open, so the next
/// launcher's update could not replace it (10-08). The game itself is never in the job.
static JOB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// A2: each run also gets its own job, nested in that one (Windows 8+), so Stop ends the run's whole tree at once
/// (TerminateJobObject) instead of waiting on taskkill walking it. (pid, job handle); `released` closes it.
static RUNS: std::sync::Mutex<Vec<(u32, usize)>> = std::sync::Mutex::new(Vec::new());

pub fn dies_with_launcher(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::*;
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits as *const _ as *const _,
                                std::mem::size_of_val(&limits) as u32);
        job as usize // never closed: the handle lives as long as the launcher
    });
    if job != 0 {
        unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) }; // ponytail: on failure, kill() still covers Cancel and close
    }
    let run = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if run.is_null() { return; }
    if unsafe { AssignProcessToJobObject(run, child.as_raw_handle() as _) } != 0 { RUNS.lock().unwrap().push((child.id(), run as usize)); }
    else { unsafe { windows_sys::Win32::Foundation::CloseHandle(run) }; }  // ponytail: kill() falls back to taskkill
}

/// The run `pid` ended: its own job goes.
pub fn released(pid: u32) {
    let mut runs = RUNS.lock().unwrap();
    for (_, job) in runs.iter().filter(|(p, _)| *p == pid) { unsafe { windows_sys::Win32::Foundation::CloseHandle(*job as _) }; }
    runs.retain(|(p, _)| *p != pid);
}

/// Read `child`'s stdout to its end, at most `limit` (A11/A12): past it the child (and its tree) is stopped and None.
pub fn stdout_within(child: &mut std::process::Child, limit: std::time::Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let mut b = Vec::new(); let _ = out.read_to_end(&mut b); let _ = tx.send(b); });
    let got = rx.recv_timeout(limit).ok();
    if got.is_none() { kill(child.id()); let _ = child.kill(); }
    let _ = child.wait();
    released(child.id());
    got
}

/// Stop every check and scan: they run from support/skate3setup.exe, which a game update replaces. Their cleanup then
/// finds its pid gone and stays quiet. Returns whether any was running (its answer never came: ask again).
pub fn stop_helpers(helpers: &std::sync::Mutex<Vec<u32>>) -> bool {
    let pids = std::mem::take(&mut *helpers.lock().unwrap());
    for &pid in &pids { kill(pid); }
    !pids.is_empty()
}

/// A check or scan ended: false when stop_helpers took it first (it was stopped, not crashed).
fn finished(helpers: &std::sync::Mutex<Vec<u32>>, pid: u32) -> bool {
    let mut helpers = helpers.lock().unwrap();
    let before = helpers.len();
    helpers.retain(|&p| p != pid);
    helpers.len() != before
}

/// Stop `pid` and everything it started, without waiting (A2: Cancel, close and Play call this on the UI thread).
/// A run of ours ends through its job; anything else (a hero render) through taskkill on its own thread.
pub fn kill(pid: u32) {
    let runs = RUNS.lock().unwrap();
    if let Some((_, job)) = runs.iter().find(|(p, _)| *p == pid) {
        if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(*job as _, 1) } != 0 { return; }
    }
    drop(runs);
    std::thread::spawn(move || {
        let _ = Proc::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).creation_flags(NO_WINDOW).status();
    });
}

pub fn handle(app: &Arc<App>, command: Command) {
    match command {
        Command::PickFolders { add } => {
            if let Some(picked) = rfd::FileDialog::new().set_title("Folders with your Skate source files").pick_folders() {
                if let Some(big) = picked.iter().find(|p| too_broad(p, std::env::var_os("USERPROFILE").map(Into::into))) {
                    app.toast(&format!("{} is a big folder: every check looks through all of it. A folder with just \
                                        your Skate files is much faster.", big.display()));  // B7: a warning, the pick still counts
                }
                {
                    let mut settings = app.settings.lock().unwrap();
                    settings.folders = if add { merge_folders(&settings.folders, picked) } else { picked };
                }
                app.save_or_report();
                let mut message = app.folders_message();
                message["picked"] = json!(true);   // the page shows "Checking" until the scan answers
                app.send(message);
                spawn(app, vec!["--identify".into()], false, |app, e| app.send(e));
            }
        }
        Command::RemoveFolder(index) => {
            let removed = { let mut s = app.settings.lock().unwrap();
                            (index < s.folders.len()).then(|| s.folders.remove(index)).is_some() };
            if removed {
                app.save_or_report();
                let mut message = app.folders_message();
                message["picked"] = json!(true);
                app.send(message);
                spawn(app, vec!["--identify".into()], false, |app, e| app.send(e));
            }
        }
        Command::Scan => spawn(app, vec!["--identify".into()], false, |app, e| app.send(e)),
        Command::Build { .. } | Command::Verify if app.refuse_while_busy() => {}
        Command::Build { fresh } => build(app, build_args(fresh)),
        Command::Verify => {
            let app = app.clone();
            std::thread::spawn(move || {
                app.send(json!({"type": "state", "state": "building"}));
                // #5: the program files first (a quarantined exe is the likely break), then every receipt (P's --verify,
                // which only checks); damaged stages are rebuilt by a resumed build once the check has exited
                let program = crate::update::verify_program(&app);
                let repair = std::sync::atomic::AtomicBool::new(false);
                spawn(&app, vec!["--verify".into()], false, move |app, e| match e["type"].as_str() {
                    Some("verify") => {
                        repair.store(e["stages"].as_array().is_some_and(|s| !s.is_empty()), std::sync::atomic::Ordering::SeqCst);
                        app.send(merge_verify(e, &program));
                    }
                    // Stop, nothing to repair, or the game/an install now in the way: the real state, never "building"
                    Some("exited") if e["cancelled"] == true || !repair.load(std::sync::atomic::Ordering::SeqCst) => check(app),
                    Some("exited") if app.refuse_while_busy() => check(app),
                    Some("exited") => build(app, build_args(false)),
                    Some("error") => app.send(verify_error(&e)),
                    _ => app.send(e),
                });
            });
        }
        // A2: the pid stays in `running` until the reader sees the tree gone and sends "ready", so a quick Build press
        // gets "wait" from the launcher instead of the pipeline's build.lock error. The kill itself never blocks.
        Command::Cancel => {
            let running = app.running.lock().unwrap();
            let Some(pid) = *running else { drop(running); return app.send(json!({"type": "state", "state": "ready"})) };
            app.stopping.store(pid, std::sync::atomic::Ordering::SeqCst);
            drop(running);
            app.send(json!({"type": "state", "state": "stopping"}));
            kill(pid);
        }
        Command::CopyReport => {
            let error = app.last_error.lock().unwrap().clone().unwrap_or(json!({"message": "No error recorded"}));
            let tail = error["log"].as_str().map(|l| log_tail(&app.game().join("data").join(l))).unwrap_or_default();
            let sources: Vec<String> = app.folders().iter().map(|f| f.display().to_string()).collect();
            let text = report_text(&error, &tail, &app.game(), &sources, crate::update::installed_build(&app.game()));
            app.send(json!({"type": "report", "text": text}));
        }
        Command::CopyTimes => {
            let saved = std::fs::read(app.game().join("data/logs/build-times.json")).ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            match saved.as_ref().and_then(|v| v["report"].as_str()) {
                Some(report) => app.send(json!({"type": "report", "toast": "Build report copied.",
                                                "text": times_text(report, crate::update::installed_build(&app.game()))})),
                None => app.send(json!({"type": "toast", "text": "No build report yet. One is saved when a build ends."})),
            }
        }
        Command::Storage | Command::Clear(_) | Command::MoveGame | Command::Uninstall { .. } | Command::BackupSaves
        | Command::RestoreSaves | Command::ResetGameSettings | Command::SupportBundle | Command::Shortcut | Command::LongPathsOn =>
            crate::maintenance::handle(app, command),
        other => crate::update::handle(app, other),
    }
}

/// Add Folder: the new folders after the saved ones, none twice (Windows paths compare without case).
pub fn merge_folders(saved: &[std::path::PathBuf], picked: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    let mut out = saved.to_vec();
    for folder in picked {
        let key = folder.to_string_lossy().to_lowercase();
        if !out.iter().any(|f| f.to_string_lossy().to_lowercase() == key) { out.push(folder); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_build_runs_in_the_job_that_closes_with_the_launcher() {
        use std::os::windows::io::AsRawHandle;
        let mut child = Proc::new("cmd").args(["/c", "ping -n 3 127.0.0.1 >nul"]).creation_flags(NO_WINDOW).spawn().unwrap();
        dies_with_launcher(&child);
        let mut inside = 0;
        unsafe { windows_sys::Win32::System::JobObjects::IsProcessInJob(child.as_raw_handle() as _, *JOB.get().unwrap() as _, &mut inside) };
        let _ = child.kill();
        assert_ne!(inside, 0, "the build must be in a job");
    }

    /// A2: Stop ends the whole tree through the run's own job, and returns at once (no taskkill wait on the UI thread).
    #[test]
    fn stop_ends_the_tree_at_once() {
        use windows_sys::Win32::System::JobObjects::*;
        let mut child = Proc::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]).creation_flags(NO_WINDOW).spawn().unwrap();
        dies_with_launcher(&child);
        std::thread::sleep(std::time::Duration::from_millis(300));  // cmd has started ping, inside the job
        let start = std::time::Instant::now();
        kill(child.id());
        assert!(start.elapsed() < std::time::Duration::from_millis(200), "kill must not wait");
        child.wait().unwrap();
        let job = RUNS.lock().unwrap().iter().find(|(p, _)| *p == child.id()).unwrap().1;
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let mut alive = 1;
        for _ in 0..50 {  // the grandchild's exit is a moment behind cmd's
            unsafe { QueryInformationJobObject(job as _, JobObjectBasicAccountingInformation, &mut info as *mut _ as *mut _,
                                               std::mem::size_of_val(&info) as u32, std::ptr::null_mut()) };
            alive = info.ActiveProcesses;
            if alive == 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        released(child.id());
        assert_eq!((alive, info.TotalProcesses >= 2), (0, true), "ping (the grandchild) must be gone too");
    }

    /// A7: two builds asked for at once (Verify's repair, the automatic rebuild, the player) start one tool.
    #[test]
    fn two_builds_at_once_start_one_tool() {
        let game = std::env::temp_dir().join(format!("s3r-p-twice-{}", std::process::id()));
        let tool = game.join("support").join("skate3setup");
        std::fs::create_dir_all(&tool).unwrap();
        std::fs::copy(r"C:\Windows\System32\PING.EXE", tool.join("skate3setup.exe")).unwrap();  // exits at once on --pipeline
        let app = crate::app::test_app(&game);
        let exited = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let helpers = app.helpers.lock().unwrap();  // both get past the first look before either can start
        let threads: Vec<_> = (0..2).map(|_| {
            let (app, exited) = (app.clone(), exited.clone());
            std::thread::spawn(move || spawn(&app, vec![], true, move |_, e| {
                if e["type"] == "exited" { exited.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
            }))
        }).collect();
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(helpers);
        for t in threads { t.join().unwrap(); }
        let start = std::time::Instant::now();
        while app.running.lock().unwrap().is_some() && start.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(exited.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&game);
    }

    /// A3: the last "play" answer comes back only while the installation is the one it was about.
    #[test]
    fn the_last_play_answer_is_shown_until_the_installation_changes() {
        let game = std::env::temp_dir().join(format!("s3r-p-last-{}", std::process::id()));
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("data/installation.json"), b"{}").unwrap();
        let file = game.join("last-check.json");
        let play = json!({"type": "state", "state": "play"});
        remember_check(&file, &game, &play);
        assert_eq!(remembered_check(&file, &game), Some(play.clone()));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(game.join("data/installation.json"), b"{}").unwrap();  // a build wrote it again
        assert_eq!(remembered_check(&file, &game), None);
        remember_check(&file, &game, &play);
        remember_check(&file, &game, &json!({"type": "state", "state": "ready"}));
        assert_eq!(remembered_check(&file, &game), None, "any other answer forgets it");
        let _ = std::fs::remove_dir_all(&game);
    }

    /// B7: a drive root or the profile itself is named; a real folder isn't.
    #[test]
    fn a_drive_or_the_whole_profile_is_too_broad() {
        let profile = Some(std::path::PathBuf::from(r"C:\Users\Al"));
        assert!(too_broad(Path::new(r"D:\"), profile.clone()) && too_broad(Path::new(r"c:\users\al\"), profile.clone()));
        assert!(!too_broad(Path::new(r"C:\Users\Al\Skate"), profile));
    }

    /// A4: Copy report's tail comes from the end of a big log, and a non-UTF-8 byte doesn't lose it.
    #[test]
    fn the_report_tail_is_read_from_the_end_and_lossily() {
        let path = std::env::temp_dir().join(format!("s3r-p-tail-{}.log", std::process::id()));
        let mut log = Vec::new();
        for i in 0..20_000 { log.extend_from_slice(format!("line {i} caf\u{e9} ").as_bytes()); log.extend_from_slice(b"\xe9\n"); }
        std::fs::write(&path, &log).unwrap();
        let tail = log_tail(&path);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 40);
        assert!(lines[0].starts_with("line 19960 ") && lines[39].starts_with("line 19999 "), "{}", lines[0]);
        std::fs::write(&path, b"one\ntwo").unwrap();
        assert_eq!(log_tail(&path), "one\ntwo");
        let _ = std::fs::remove_file(&path);
        assert_eq!(log_tail(&path), "");
    }

    #[test]
    fn an_update_stops_a_running_check_and_its_exit_is_not_an_error() {
        let mut child = Proc::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]).creation_flags(NO_WINDOW).spawn().unwrap();
        let helpers = std::sync::Mutex::new(vec![child.id()]);
        assert!(stop_helpers(&helpers));
        assert!(child.wait().unwrap().code() != Some(0), "the check must be stopped");
        assert!(!finished(&helpers, child.id()), "a stopped check is cancelled, not crashed");
        assert!(!stop_helpers(&helpers), "nothing left to stop");
        helpers.lock().unwrap().push(7);
        assert!(finished(&helpers, 7) && helpers.lock().unwrap().is_empty(), "a check that ends by itself is removed");
    }
    #[test]
    fn times_report_carries_the_builds_and_the_pipelines_text() {
        let text = times_text("Built in 17:36", Some(5));
        assert!(text.contains("Game build: 5") && text.ends_with("Built in 17:36
"));
        assert!(times_text("x", None).contains("Game build: -"));
    }

    #[test]
    fn report_trims_paths_and_keeps_reason_stage_and_log() {
        let error = serde_json::json!({"event":"error","stage":"mod-maps","kind":"failed","message":"mod-maps failed: x","log":"logs/mod-maps.log"});
        let log = "line1\nC:\\Users\\Friend\\S3R\\game\\data\\work\\a\nD:\\Private\\Skate Source Files\\s3.iso failed\nline3";
        let sources = [r"D:\Private\Skate Source Files".to_owned(), r"E:\Other".to_owned()];
        let text = report_text(&error, log, std::path::Path::new(r"C:\Users\Friend\S3R\game"), &sources, Some(12));
        assert!(text.contains("mod-maps failed: x") && text.contains("Stage: mod-maps") && text.contains("line3"));
        assert!(!text.contains(r"C:\Users\Friend"), "{text}");
        assert!(text.contains(r"<game>\data\work\a"));
        assert!(!text.contains("Private"), "a source folder outside the home folder must be hidden: {text}");
        assert!(text.contains(r"<sources 1>\s3.iso failed"), "{text}");
        assert!(text.contains("Game build: 12"), "{text}");
        assert!(report_text(&error, "", std::path::Path::new("g"), &[], None).contains("Game build: -"));
    }
    #[test]
    fn a_normal_build_resumes_and_a_fresh_build_does_not() {
        assert_eq!(build_args(false), ["--resume"]);
        let requested = std::sync::atomic::AtomicBool::new(false);
        assert!(!rebuild_now(&json!({"state": "ready"}), &requested), "no automatic update: the player presses Build");
        requested.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(rebuild_now(&json!({"state": "ready"}), &requested));
        assert!(!rebuild_now(&json!({"state": "ready"}), &requested), "once per update");
        requested.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!rebuild_now(&json!({"state": "play"}), &requested), "nothing changed: nothing to build");
        assert!(!rebuild_now(&json!({"state": "ready"}), &requested), "and the request is gone");
        assert!(build_args(true).is_empty(), "no --resume means every stage runs again");
    }
    #[test]
    fn event_lines_pass_through_and_garbage_is_ignored() {
        assert_eq!(parse_line(r#"{"event":"stage","name":"a","step":1,"index":0,"total":3,"status":"start"}"#).unwrap()["type"], "stage");
        assert!(parse_line("Traceback (most recent call last):").is_none());
    }
    #[test]
    fn paths_are_trimmed_in_every_spelling_and_an_empty_folder_changes_nothing() {
        let game = r"D:\S3R\game";
        let log = r"$ ['D:\\S3R\\game\\data\\a', 'D:/S3R/game/data/b', 'D:\S3R\game\data\c']";
        let text = trim_path(log, game, "<game>");
        assert!(!text.contains("S3R"), "{text}");
        assert_eq!(text.matches("<game>").count(), 3);
        assert_eq!(trim_path("abc", "", "~"), "abc");
    }
    #[test]
    fn a_silent_crash_is_reported_but_a_cancel_or_a_reported_error_is_not() {
        assert!(unexpected_exit(Some(1), false, false, false).is_some());
        assert!(unexpected_exit(None, false, false, false).is_some());
        assert!(unexpected_exit(Some(1), true, false, false).is_none());
        assert!(unexpected_exit(Some(1), false, false, true).is_none());
        assert!(unexpected_exit(Some(0), false, false, false).is_none());
        let crash = unexpected_exit(Some(-1073741515), false, false, false).unwrap();
        assert!(crash["message"].as_str().unwrap().contains("data\\setup-stderr.log"), "names the file with the reason");
    }
    #[test]
    fn a_build_that_exits_0_without_done_is_an_error() {
        let error = unexpected_exit(Some(0), false, true, false).expect("exit 0 without done must be reported");
        assert!(error["message"].as_str().unwrap().contains("without finishing"));
        assert!(unexpected_exit(Some(0), true, true, false).is_none(), "an error already reported is not repeated");
        assert!(unexpected_exit(Some(0), false, true, true).is_none(), "a cancelled build is not an error");
    }

    #[test]
    fn every_run_gets_the_game_drives_temp_and_a_build_runs_gently() {
        let game = std::path::Path::new(r"D:\S3R\game");
        let (env, flags) = run_setup(game, true, true, false);
        assert_eq!(env, [("TMP", r"D:\S3R\game\data\tmp".into()), ("TEMP", r"D:\S3R\game\data\tmp".into())]);
        assert_eq!(flags, NO_WINDOW | BELOW_NORMAL_PRIORITY);
        let (env, flags) = run_setup(game, true, false, true);
        assert!(env.contains(&("SKATE_BUILD_JOBS", "2".into())) && env.contains(&("SKATE_FANOUT", "2".into())));
        assert_eq!(flags, NO_WINDOW, "Build in the background off: normal priority");
        let (env, flags) = run_setup(game, false, true, true);
        assert_eq!((env.len(), flags), (2, NO_WINDOW), "a check is never slowed or capped");
    }

    #[test]
    fn the_taskbar_follows_the_build() {
        assert_eq!(taskbar_message(&json!({"type": "progress", "percent": 42.5})), Some(json!({"taskbar": 42.5})));
        assert_eq!(taskbar_message(&json!({"type": "stage", "index": 1, "total": 4})), Some(json!({"taskbar": 25.0})));
        assert_eq!(taskbar_message(&json!({"type": "done"})), Some(json!({"taskbar": null, "flash": true})));
        assert_eq!(taskbar_message(&json!({"type": "error"})), Some(json!({"taskbar": "error", "flash": true})));
        assert_eq!(taskbar_message(&json!({"type": "log"})), None);
    }

    #[test]
    fn added_folders_come_after_the_saved_ones_once() {
        use std::path::PathBuf;
        let saved = [PathBuf::from(r"D:\Skate"), PathBuf::from(r"E:\DLC")];
        let merged = merge_folders(&saved, vec![PathBuf::from(r"d:\skate"), PathBuf::from(r"F:\More"), PathBuf::from(r"F:\More")]);
        assert_eq!(merged, [PathBuf::from(r"D:\Skate"), PathBuf::from(r"E:\DLC"), PathBuf::from(r"F:\More")]);
    }

    #[test]
    fn verify_counts_the_program_files_too() {
        let pipeline = json!({"type": "verify", "ok": true, "files": 100, "bad": 0, "message": "All game data is fine."});
        let ok = merge_verify(pipeline.clone(), &json!({"ok": true, "files": 20, "bad": 0, "message": ""}));
        assert_eq!((ok["ok"].as_bool(), ok["files"].as_u64(), ok["message"].as_str()), (Some(true), Some(120), Some("All game data is fine.")));
        let bad = merge_verify(pipeline, &json!({"ok": false, "files": 20, "bad": 2, "message": "2 program files were damaged and downloaded again."}));
        assert_eq!((bad["ok"].as_bool(), bad["bad"].as_u64()), (Some(false), Some(2)));
        assert!(bad["message"].as_str().unwrap().starts_with("2 program files"));
    }

    #[test]
    fn report_paths_are_replaced() {
        let trimmed = trim_report(r"D:\S3R\game\data\x failed reading E:\Mine\s3.iso", std::path::Path::new(r"D:\S3R\game"), &[r"E:\Mine".into()]);
        assert_eq!(trimmed, r"<game>\data\x failed reading <sources 1>\s3.iso");
    }

    #[test]
    fn a_verify_failure_is_a_plain_message_not_a_build_error() {
        let e = verify_error(&json!({"type": "error", "stage": null, "kind": "failed", "log": null, "message": "A build is running for this game."}));
        assert_eq!(e, json!({"type": "error", "scope": "verify", "message": "A build is running for this game."}));
    }
}
