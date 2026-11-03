//! The game's crash reports (saved by crates/skate-game/src/crash_report.rs) shown in the launcher, and sent to the
//! developers' Sentry project only when the player presses Send report. The report text is already free of paths
//! and account lines (crash_report.rs sanitize); the event adds nothing about the player (no user, no IP, no names).
use crate::app::App;
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};

/// Public by design: a DSN key can only send events to this project. The Sentry auth token never goes in the repo.
const KEY: &str = "533553d30a1093ebac602309297c7e2a";
const ENVELOPE: &str = "https://o4512223351930880.ingest.us.sentry.io/api/4512223366152192/envelope/";
/// Tells the game's crash reporter that this launcher shows its report (crash_report.rs launcher_shows).
pub const LAUNCHER_PID: &str = "SKATE_LAUNCHER_PID";

/// The two places crash_report.rs save() tries, in its order.
fn folders() -> Vec<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    [local, std::env::temp_dir()].map(|root| root.join("Skate3RustEngine").join("CrashReports")).into()
}

/// `report-<ns>-<pid>.txt` -> (ns, pid). Anything else (another file, a path, "..") is not a report.
fn parse(name: &str) -> Option<(u128, u32)> {
    let (stamp, pid) = name.strip_prefix("report-")?.strip_suffix(".txt")?.split_once('-')?;
    Some((stamp.parse().ok()?, pid.parse().ok()?))
}

/// A report by name, from the game's own folders only (`--crash` and the page pass names, never trusted paths).
pub fn find(name: &str) -> Option<PathBuf> {
    parse(name)?;
    folders().into_iter().map(|f| f.join(name)).find(|p| p.is_file())
}

/// The report the game's supervisor process `pid` saved after `since` (ns): pids get reused, report names don't.
fn saved_by(pid: u32, since: u128) -> Option<PathBuf> {
    folders().into_iter().filter_map(|f| std::fs::read_dir(f).ok()).flatten().flatten()
        .filter_map(|e| { let (stamp, p) = parse(e.file_name().to_str()?)?; (p == pid && stamp >= since).then_some((stamp, e.path())) })
        .max().map(|(_, path)| path)
}

fn now_ns() -> u128 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() }

/// After Play: when the game ends, the launcher hears it (App::game_exited); when it ends with an error, the crash
/// window opens with its report (main.rs crash_window).
pub fn watch(app: Arc<App>, mut game: std::process::Child) {
    let since = now_ns().saturating_sub(1_000_000_000);  // ponytail: 1 s of clock slack, a same-pid report that old is not ours
    std::thread::spawn(move || {
        let status = game.wait();
        app.game_exited(game.id());  // F1: builds and updates may run again
        if status.is_ok_and(|s| !s.success()) {
            if let Some(report) = saved_by(game.id(), since).as_deref().and_then(message) {
                app.send(json!({"crash": report}));
            }
        }
    });
}

/// The crash window's report: the text, its file name (for Send) and a one-line headline.
pub fn message(path: &Path) -> Option<Value> {
    let text = String::from_utf8_lossy(&std::fs::read(path).ok()?).into_owned();
    let name = path.file_name()?.to_str()?;
    Some(json!({"type": "crash", "name": name, "text": text, "summary": headline(&text).1}))
}

/// Report lines look like `+2.000s stderr: REPORT_PANIC bad index`; this is the part after `tag`.
fn tagged<'a>(text: &'a str, tag: &str) -> impl Iterator<Item = &'a str> {
    text.lines().filter_map(move |l| l.split_once(tag).map(|(_, rest)| rest.trim()))
}

/// (kind, what happened): the panic message, the Windows exception, or the last thing the game said before it exited.
fn headline(text: &str) -> (&'static str, String) {
    if let Some(panic) = tagged(text, "REPORT_PANIC ").find(|l| !l.starts_with("at ")) {
        return ("panic", panic.to_owned());
    }
    if let Some(native) = tagged(text, "REPORT_NATIVE ").next() {
        return ("native exception", native.to_owned());
    }
    let logs = text.split("\nRecent logs\n").nth(1).unwrap_or("");
    let said = tagged(logs, "stderr: ").filter(|l| !l.starts_with("REPORT_") && !l.contains("omitted]")).last();
    let outcome = text.lines().find(|l| l.starts_with("Process status:") || l.starts_with("Could not start game"));
    ("exit", said.or(outcome).unwrap_or("The game stopped unexpectedly.").to_owned())
}

/// Sentry's os/device/gpu contexts from the report's `CPU:`, `Memory:` and first `GPU:` lines (skate-game
/// crash_native.rs hardware), so issues can be filtered by graphics card or RAM. A missing line leaves its fields out.
fn contexts(text: &str, os: String) -> Value {
    const GIB: f64 = 1073741824.0;
    let line = |prefix: &str| text.lines().find_map(|l| l.strip_prefix(prefix));
    let gb = |s: &str| s.trim().strip_suffix("GB")?.trim().parse::<f64>().ok();
    let (cpu, threads) = line("CPU: ").and_then(|l| l.rsplit_once(" (")).map_or((None, None), |(name, rest)| {
        (Some(name), rest.strip_suffix(" threads)").and_then(|n| n.parse::<u32>().ok()))
    });
    let (total, free) = line("Memory: ").and_then(|l| l.split_once(" total, ")).map_or((None, None), |(total, free)| {
        (gb(total).map(|g| (g * GIB) as u64), free.strip_suffix(" free").and_then(gb).map(|g| (g * GIB) as u64))
    });
    let gpu = line("GPU: ").and_then(|l| l.rsplit_once(" (")).map_or(json!({}), |(name, vram)| {
        json!({"name": name, "memory_size": vram.strip_suffix(" VRAM)").and_then(gb).map(|g| (g * 1024.0).round() as u64)})  // MB
    });
    json!({
        "os": {"name": "Windows", "raw_description": os},
        "device": {"cpu_description": cpu, "processor_count": threads, "memory_size": total, "free_memory": free},
        "gpu": gpu,
    })
}

/// A Sentry event for the report: grouped by the panic's source line when there is one, with the whole report attached.
fn envelope(text: &str, build: Option<u64>) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let random = || { let mut h = std::collections::hash_map::RandomState::new().build_hasher(); h.write_u128(now_ns()); h.finish() };
    let id = format!("{:016x}{:016x}", random(), random());
    let (kind, value) = headline(text);
    let line = |prefix: &str| text.lines().find_map(|l| l.strip_prefix(prefix)).unwrap_or("").trim().to_owned();
    let mut exception = json!({"type": kind, "value": value, "mechanism": {"type": "skate3-crash-report", "handled": false}});
    // `REPORT_PANIC at crates/skate-game/src/x.rs:12:5`
    if let Some(at) = tagged(text, "REPORT_PANIC at ").next() {
        let mut parts = at.rsplitn(3, ':');
        let (col, row, file) = (parts.next(), parts.next(), parts.next());
        if let (Some(file), Some(Ok(row)), Some(Ok(col))) = (file, row.map(str::parse::<u32>), col.map(str::parse::<u32>)) {
            exception["stacktrace"] = json!({"frames": [{"filename": file, "lineno": row, "colno": col, "in_app": true}]});
        }
    }
    let event = json!({
        "event_id": id, "timestamp": now_ns() as f64 / 1e9, "platform": "native", "level": "fatal", "logger": "skate3-crash-report",
        "release": build.map(|b| format!("skate3-remaster@{b}")), "environment": "release",
        "sdk": {"name": "skate3-remaster.launcher", "version": crate::update::version()},
        "exception": {"values": [exception]},
        "tags": {"revision": line("Build: ").split(' ').find_map(|p| p.strip_prefix("revision=")).unwrap_or("unknown")},
        "contexts": contexts(text, line("OS version: ")),
        "extra": {"build": line("Build: ")},
    }).to_string();
    let mut body = Vec::new();
    for (header, payload) in [
        (json!({"event_id": id}), None),
        (json!({"type": "event", "length": event.len()}), Some(event.as_bytes())),
        (json!({"type": "attachment", "length": text.len(), "filename": "report.txt", "content_type": "text/plain"}), Some(text.as_bytes())),
    ] {
        body.extend_from_slice(header.to_string().as_bytes());
        body.push(b'\n');
        if let Some(payload) = payload { body.extend_from_slice(payload); body.push(b'\n'); }
    }
    body
}

fn post(text: &str, build: Option<u64>) -> Result<(), String> {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build().post(ENVELOPE)
        .set("Content-Type", "application/x-sentry-envelope")
        .set("X-Sentry-Auth", &format!("Sentry sentry_version=7, sentry_key={KEY}, sentry_client=skate3-remaster.launcher/{}",
                                       crate::update::version()))
        .send_bytes(&envelope(text, build)).map(|_| ()).map_err(|e| e.to_string())
}

/// The page's Send report.
pub fn send(app: &Arc<App>, name: &str) {
    let (app, report, build) = (app.clone(), find(name), crate::update::installed_build(&app.game()));
    std::thread::spawn(move || {
        let result = report.ok_or_else(|| "That report is gone.".to_owned()).and_then(|path| {
            post(&String::from_utf8_lossy(&std::fs::read(path).map_err(|e| e.to_string())?), build)
        });
        app.send(match result {
            Ok(()) => json!({"type": "crash_sent", "ok": true}),
            Err(error) => json!({"type": "crash_sent", "ok": false, "message": format!("Could not send the report: {error}")}),
        });
    });
}

/// The page's Open folder: the folder holding the report.
pub fn open_folder(name: &str) {
    if let Some(folder) = find(name).as_deref().and_then(Path::parent) { let _ = std::process::Command::new("explorer").arg(folder).spawn(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANIC: &str = "Skate 3 Remaster diagnostic report v1\nBuild: 0.1.0 revision=3e373f3 dirty=false\nPlatform: windows / x86_64\n\
        Process status: exit code: 101; exit code hex: 0x00000065.\nOS version: Windows 10.0.22631\n\
        CPU: AMD Ryzen 5 3600 6-Core Processor (12 threads)\nMemory: 15.9 GB total, 7.5 GB free\n\
        GPU: NVIDIA GeForce GTX 1650 (4.0 GB VRAM)\nGPU: AMD Radeon(TM) Graphics (0.5 GB VRAM)\n\n\
        Panic, native exception and stack\n+2.000s stderr: REPORT_PANIC index out of bounds\n\
        +2.000s stderr: REPORT_PANIC at crates/skate-game/src/physics.rs:812:17\n\nRecent logs\n+1.000s stderr: loading\n";

    #[test]
    fn only_the_games_report_names_are_reports() {
        assert_eq!(parse("report-1759940000000000000-4242.txt"), Some((1759940000000000000, 4242)));
        for name in ["report-1-2.txt.exe", "..\\report-1-2.txt", "report-1.txt", "report-a-2.txt", "notes.txt", "report-1-2-3.txt"] {
            assert_eq!(parse(name), None, "{name}");
        }
        assert_eq!(find("..\\..\\Windows\\win.ini"), None);
    }

    #[test]
    fn the_report_for_this_play_is_the_one_its_game_saved() {
        let folder = &folders()[0];
        std::fs::create_dir_all(folder).unwrap();
        let pid = u32::MAX - 7;  // never a real process id
        let since = now_ns();
        let older = folder.join(format!("report-{}-{pid}.txt", since - 5_000_000_000));
        let mine = folder.join(format!("report-{}-{pid}.txt", since + 1));
        let other = folder.join(format!("report-{}-{}.txt", since + 2, pid - 1));
        for f in [&older, &mine, &other] { std::fs::write(f, PANIC).unwrap(); }
        assert_eq!(saved_by(pid, since), Some(mine.clone()), "not the older one with a reused pid, not another process's");
        let m = message(&mine).unwrap();
        assert_eq!((m["summary"].as_str(), m["name"] == mine.file_name().unwrap().to_str().unwrap()), (Some("index out of bounds"), true));
        assert_eq!(find(mine.file_name().unwrap().to_str().unwrap()), Some(mine.clone()));
        for f in [older, mine, other] { std::fs::remove_file(f).unwrap(); }
    }

    #[test]
    fn the_headline_says_what_stopped_the_game() {
        assert_eq!(headline(PANIC), ("panic", "index out of bounds".into()));
        let native = "Panic, native exception and stack\n+3.000s stderr: REPORT_NATIVE code=0xC0000005 fault_pc=0x1234\n";
        assert_eq!(headline(native).0, "native exception");
        let refused = "Process status: exit code: 1\n\nRecent logs\n+0.100s stderr: REPORT_META stage=setup\n\
                       +0.200s stderr: The game files need building. Open S3R and press Build game.\n\
                       +0.300s stderr: [absolute-path line omitted]\n";
        assert_eq!(headline(refused), ("exit", "The game files need building. Open S3R and press Build game.".into()));
        assert_eq!(headline("Process status: exit code: 3\n").1, "Process status: exit code: 3");
    }

    #[test]
    fn the_envelope_is_one_event_with_the_report_attached_and_nothing_about_the_player() {
        let body = envelope(PANIC, Some(7));
        let text = String::from_utf8(body).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let head: Value = serde_json::from_str(lines[0]).unwrap();
        let item: Value = serde_json::from_str(lines[1]).unwrap();
        let event: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(item["length"].as_u64(), Some(lines[2].len() as u64), "the event's byte length");
        assert_eq!((head["event_id"].as_str().unwrap().len(), &event["event_id"]), (32, &head["event_id"]));
        assert_eq!(event["release"], "skate3-remaster@7");
        assert_eq!(event["tags"]["revision"], "3e373f3");
        let exception = &event["exception"]["values"][0];
        assert_eq!((exception["type"].as_str(), exception["value"].as_str()), (Some("panic"), Some("index out of bounds")));
        assert_eq!(exception["stacktrace"]["frames"][0], json!({"filename": "crates/skate-game/src/physics.rs", "lineno": 812, "colno": 17, "in_app": true}));
        let attachment: Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!((attachment["filename"].as_str(), attachment["length"].as_u64()), (Some("report.txt"), Some(PANIC.len() as u64)));
        assert!(text.ends_with(&format!("{PANIC}\n")));
        assert!(event.get("user").is_none() && event.get("server_name").is_none());
        assert_eq!(event["contexts"]["device"], json!({"cpu_description": "AMD Ryzen 5 3600 6-Core Processor", "processor_count": 12,
                                                       "memory_size": 17072495001u64, "free_memory": 8053063680u64}));
        assert_eq!(event["contexts"]["gpu"], json!({"name": "NVIDIA GeForce GTX 1650", "memory_size": 4096}), "the first card");
        let bare = contexts("OS version: Windows 10.0.22631\n", "Windows 10.0.22631".into());  // a report from before build 7
        assert_eq!((bare["device"]["memory_size"].is_null(), bare["gpu"].as_object().unwrap().len()), (true, 0));
        assert_ne!(envelope(PANIC, None)[..60], text.as_bytes()[..60], "a new event id every send");
    }

    /// Sends one real event to the project (one of the month's 5k): `cargo test -p skate-launcher -- --ignored sentry`.
    #[test]
    #[ignore]
    fn sentry_accepts_the_envelope() {
        post(&PANIC.replace("index out of bounds", "TEST EVENT from the launcher's tests, not a real crash"), Some(0)).unwrap();
    }
}
