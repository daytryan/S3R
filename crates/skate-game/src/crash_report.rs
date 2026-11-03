//! Process supervision uses only std; no renderer or game assets are initialized here.
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CHILD: &str = "SKATE_REPORT_CHILD";
/// Not retail (user 10-27: Performance Mode takes a restart): the game prints this, then ends cleanly, to be started
/// again by the supervisor ([`entry`]), which the launcher keeps watching throughout.
pub(crate) const RESTART: &str = "REPORT_RESTART";
const LINE_LIMIT: usize = 4096;
const LOG_LIMIT: usize = 256;
#[cfg(windows)]
#[path = "crash_native.rs"]
mod native;

#[derive(Default)]
struct Capture {
    logs: VecDeque<String>,
    transitions: VecDeque<String>,
    metadata: std::collections::BTreeMap<String, String>,
    panic: VecDeque<String>,
    /// The game printed [`RESTART`].
    restart: bool,
}
impl Capture {
    fn line(&mut self, stream: &str, line: &[u8], elapsed: f64) {
        self.restart |= line.trim_ascii_end() == RESTART.as_bytes();
        let line = sanitize(&String::from_utf8_lossy(line));
        // Advanced capture status must reach the launch console even while the
        // supervisor keeps normal logs bounded and private. Sanitize first.
        if line.starts_with("TRACE ") {
            eprintln!("{line}");
        }
        let entry = format!("+{elapsed:.3}s {stream}: {line}");
        if let Some(value) = line.strip_prefix("REPORT_META ") {
            if let Some((key, _)) = value.split_once('=') {
                // A fixed key vocabulary prevents arbitrary diagnostic accumulation.
                if ["startup", "stage", "cpu", "gpu", "gpu_lost", "panic", "state", "graphics", "network"].contains(&key) {
                    self.metadata.insert(key.into(), entry.clone());
                }
            }
        }
        if line.starts_with("REPORT_TRANSITION ") {
            push(&mut self.transitions, entry.clone(), 64);
        }
        if line.starts_with("REPORT_PANIC ") || line.starts_with("REPORT_NATIVE ") {
            push(&mut self.panic, entry.clone(), 128);
        }
        push(&mut self.logs, entry, LOG_LIMIT);
    }
}
fn push(queue: &mut VecDeque<String>, value: String, limit: usize) {
    if queue.len() == limit {
        queue.pop_front();
    }
    queue.push_back(value);
}

/// Fail closed for credential/network messages. Never collect args or environment dumps.
fn sanitize(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if [
        "token",
        "ticket",
        "password",
        "secret",
        "authorization",
        "cookie",
        "session",
        "steam",
        "join_code",
        "host_code",
        "peer=",
        "address=",
        "lobby=",
        "owner=",
        "appearance",
        "unknown argument",
    ]
    .iter()
    .any(|key| lower.contains(key))
    {
        return "[sensitive-context line omitted]".into();
    }
    // Omit the entire line when it contains absolute paths (including paths with spaces).
    // Keep source locations portable by reporting relative paths from our panic hook.
    if value
        .as_bytes()
        .windows(3)
        .any(|w| w[0].is_ascii_alphabetic() && w[1] == b':' && (w[2] == b'\\' || w[2] == b'/'))
        || value.contains("\\\\")
        || value
            .split_whitespace()
            .any(|s| s.trim_start_matches(['\'', '"', '(']).starts_with('/'))
    {
        return "[absolute-path line omitted]".into();
    }
    let clean: String = value.chars().filter(|c| !c.is_control() || *c == '\t').take(LINE_LIMIT).collect();
    mask(&clean)
}

/// Replace IP endpoints and long numeric ids (Steam, lobby) with `[masked]`. A word is a run of hex digits, `.` and
/// `:`, so brackets, `=` and spaces split it.
// ponytail: heuristic; misses an IP packed into a short integer and may mask a 4-part version number.
fn mask(line: &str) -> String {
    let word = |c: char| c.is_ascii_hexdigit() || c == '.' || c == ':';
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find(word) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail.find(|c: char| !word(c)).unwrap_or(tail.len());
        let w = &tail[..end];
        out.push_str(if private(w) { "[masked]" } else { w });
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn private(w: &str) -> bool {
    let quad = |p: &str| {
        let p: Vec<_> = p.split('.').collect();
        p.len() == 4 && p.iter().all(|n| (1..=3).contains(&n.len()) && n.parse::<u8>().is_ok())
    };
    let long_id = w.split(|c: char| !c.is_ascii_digit()).any(|run| run.len() >= 15);
    let v6 = w.starts_with(|c: char| c.is_ascii_hexdigit())
        && w.contains(|c: char| c.is_ascii_digit())
        && (w.contains("::") || w.matches(':').count() >= 7)
        && w.split(':').all(|g| g.len() <= 4 && g.chars().all(|c| c.is_ascii_hexdigit()) || quad(g));
    long_id || v6 || w.split(':').any(quad)
}

fn reader(
    mut input: impl Read + Send + 'static,
    stream: &'static str,
    capture: Arc<Mutex<Capture>>,
    start: Instant,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        let mut line = Vec::with_capacity(LINE_LIMIT);
        let mut truncated = false;
        loop {
            let count = match input.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &byte in &buffer[..count] {
                if byte == b'\n' {
                    if truncated {
                        line.clear();
                        line.extend_from_slice(b"[oversized line omitted]");
                    }
                    capture.lock().unwrap_or_else(|e| e.into_inner()).line(
                        stream,
                        &line,
                        start.elapsed().as_secs_f64(),
                    );
                    line.clear();
                    truncated = false;
                } else if line.len() < LINE_LIMIT {
                    line.push(byte);
                } else {
                    truncated = true;
                }
            }
            // Preserve existing launcher logs. Capture happens before forwarding.
            if stream == "stderr" {
                let _ = std::io::stderr().write_all(&buffer[..count]);
            } else {
                let _ = std::io::stdout().write_all(&buffer[..count]);
            }
        }
        if truncated {
            line.clear();
            line.extend_from_slice(b"[oversized line omitted]");
        }
        if !line.is_empty() {
            capture.lock().unwrap_or_else(|e| e.into_inner()).line(
                stream,
                &line,
                start.elapsed().as_secs_f64(),
            );
        }
    })
}

pub(crate) fn entry() -> Option<i32> {
    if std::env::var_os(CHILD).is_some() {
        // Entry runs before game threads. Do not let setup/relay descendants
        // accidentally pass the supervisor bypass marker to a later game launch.
        unsafe {
            std::env::remove_var(CHILD);
        }
        #[cfg(windows)]
        native::install();
        install_panic_hook();
        return None;
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--crash-report-preview"))
    {
        present("Skate 3 diagnostic UI preview\nSynthetic report only; no game was started.\n");
        return Some(0);
    }
    let capture = Arc::new(Mutex::new(Capture::default()));
    let start = Instant::now();
    let result = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(CHILD, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    });
    let mut child = match result {
        Ok(child) => child,
        Err(error) => {
            present(&report(
                &Capture::default(),
                &format!("Could not start game: {}", sanitize(&error.to_string())),
                0.,
            ));
            return Some(1);
        }
    };
    let out = reader(
        child.stdout.take().unwrap(),
        "stdout",
        capture.clone(),
        start,
    );
    let err = reader(
        child.stderr.take().unwrap(),
        "stderr",
        capture.clone(),
        start,
    );
    let status = child.wait();
    // A relay may inherit a pipe. Never wait indefinitely for its EOF.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !(out.is_finished() && err.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    // Performance Mode's restart: the game again, under this supervisor (which the launcher keeps watching).
    if restarting(&capture.lock().unwrap_or_else(|e| e.into_inner()), &status) {
        return entry();
    }
    let code = match &status {
        Ok(s) => s.code().unwrap_or(1),
        Err(_) => 1,
    };
    if !status.as_ref().is_ok_and(|s| s.success()) {
        let outcome = match status {
            Ok(s) => format!(
                "Process status: {s}; exit code hex: 0x{:08X}. Native exception interpretation is platform-dependent.",
                code as u32
            ),
            Err(e) => format!("Process wait failed: {}", sanitize(&e.to_string())),
        };
        let text = report(
            &capture.lock().unwrap_or_else(|e| e.into_inner()),
            &outcome,
            start.elapsed().as_secs_f64(),
        );
        present(&text);
    }
    Some(code)
}

/// The game asked to be started again: its [`RESTART`] line, then a clean exit (a crash after it is a crash).
fn restarting(capture: &Capture, status: &std::io::Result<std::process::ExitStatus>) -> bool {
    capture.restart && status.as_ref().is_ok_and(|s| s.success())
}

thread_local! {
    static LAST_PANIC: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// This thread's last panic, "message at file:line:col", for code that catches one and goes on. Taken: a second
/// call returns None until the next panic.
pub(crate) fn take_last_panic() -> Option<String> {
    LAST_PANIC.with(|cell| cell.borrow_mut().take())
}

fn install_panic_hook() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    std::panic::set_hook(Box::new(|info| {
        // A panic within a second of the last one is part of its burst: a lost GPU device panics every render system.
        let first = LAST.lock().map_or(true, |mut at| at.replace(Instant::now()).is_none_or(|t| t.elapsed() > Duration::from_secs(1)));
        let payload = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic payload");
        eprintln!("REPORT_PANIC {}", sanitize(payload));
        let mut last = payload.to_owned();
        if let Some(location) = info.location() {
            let file = location.file().replace('\\', "/");
            let portable = file
                .find("crates/")
                .map(|i| &file[i..])
                .unwrap_or_else(|| file.rsplit('/').next().unwrap_or("unknown"));
            eprintln!(
                "REPORT_PANIC at {portable}:{}:{}",
                location.line(),
                location.column()
            );
            last = format!("{last} at {portable}:{}:{}", location.line(), location.column());
        }
        if first {
            let thread = std::thread::current();
            eprintln!("REPORT_META panic={} (thread {})", sanitize(&last), thread.name().unwrap_or("unnamed"));
        }
        LAST_PANIC.with(|cell| *cell.borrow_mut() = Some(last));
        // Only a burst's first panic walks its stack, without the unsymbolized frames: ten stacks of `<unknown>` pushed
        // the cause out of the 256-line log (Sentry 10-26).
        // The independent supervisor owns the popup.
        if first {
            for line in stack_lines(&std::backtrace::Backtrace::force_capture().to_string()).take(120) {
                eprintln!("REPORT_PANIC {}", sanitize(line));
            }
        }
        // perf 10-18: queued saves reach the disk before the process goes.
        crate::saves::drain_for_crash();
    }));
}

fn stack_lines(trace: &str) -> impl Iterator<Item = &str> {
    trace.lines().filter(|l| !l.ends_with(": <unknown>"))
}

fn report(capture: &Capture, outcome: &str, elapsed: f64) -> String {
    #[cfg(windows)]
    let os_version = native::os_version();
    #[cfg(not(windows))]
    let os_version = "OS version unavailable";
    let mut text = format!(
        "Skate 3 Remaster diagnostic report v1\nBuild: {}\nPlatform: {} / {}\nUTC Unix seconds: {}\nRuntime seconds: {elapsed:.3}\n{outcome}\n\nNo automatic upload. Review before sharing. Paths and sensitive-context log lines are omitted.\nNative fault stack/registers: unavailable (no memory dump collected).\nGPU/driver, map and settings: available only if initialized and recorded below.\nMods: no authoritative mod inventory; modified asset contents are not collected.\nLogs: last 256 bounded lines; transitions: last 64; panic: last 128 lines.\nState is sampled every second; brief transitions can be missed. Abrupt exits can lose pending pipe data.\n",
        env!("SKATE_BUILD_ID"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
    text.push_str(&format!("OS version: {os_version}\n"));
    #[cfg(windows)]
    for line in native::hardware() {
        text.push_str(&sanitize(&line));
        text.push('\n');
    }
    for (name, entries) in [
        (
            "Latest metadata",
            capture.metadata.values().collect::<Vec<_>>(),
        ),
        (
            "Recent state transitions",
            capture.transitions.iter().collect(),
        ),
        (
            "Panic, native exception and stack",
            capture.panic.iter().collect(),
        ),
        ("Recent logs", capture.logs.iter().collect()),
    ] {
        text.push_str(&format!("\n{name}\n"));
        if entries.is_empty() {
            text.push_str("Unavailable / not recorded\n");
        }
        for entry in entries {
            text.push_str(entry);
            text.push('\n');
        }
    }
    text
}

fn save(text: &str) -> std::io::Result<PathBuf> {
    let roots = [
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir),
        std::env::temp_dir(),
    ];
    let mut error = None;
    for root in roots {
        let result = (|| {
            let folder = root.join("Skate3RustEngine/CrashReports");
            std::fs::create_dir_all(&folder)?;
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = folder.join(format!("report-{stamp}-{}.txt", std::process::id()));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(text.as_bytes())?;
            prune_reports(&folder, KEEP_REPORTS);
            Ok(path)
        })();
        match result {
            Ok(path) => return Ok(path),
            Err(e) => error = Some(e),
        }
    }
    Err(error.unwrap())
}

/// Reports kept: saving one drops the oldest past this many (they only piled up: 173 on 10-12).
const KEEP_REPORTS: usize = 50;

/// Deletes all but the newest `keep` `report-<ns>-<pid>.txt` in `folder`; any other file stays.
fn prune_reports(folder: &Path, keep: usize) {
    let mut reports: Vec<(u128, PathBuf)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let (stamp, pid) = name.to_str()?.strip_prefix("report-")?.strip_suffix(".txt")?.split_once('-')?;
            pid.parse::<u32>().ok()?;
            Some((stamp.parse().ok()?, entry.path()))
        })
        .collect();
    reports.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in reports.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}
fn present(text: &str) {
    match save(text) {
        Ok(path) => {
            eprintln!("Crash report saved: {}", path.display());
            if launcher_shows(&path) {
                return;
            }
            if let Err(error) = popup(&path) {
                eprintln!(
                    "Report popup unavailable: {error}. Open the saved text report manually."
                );
                #[cfg(windows)]
                native::fallback_notice(&format!(
                    "The game stopped. The report popup could not start.\nSaved report: {}",
                    path.display()
                ));
            }
        }
        Err(error) => {
            eprintln!("Could not save crash report: {error}\n{text}");
            #[cfg(windows)]
            native::fallback_notice(
                "The game stopped, but its diagnostic report could not be saved. Check free disk space and directory permissions. Details were sent to the launcher log.",
            );
        }
    }
}
/// The launcher shows the report in its own window and offers to send it: the launcher that started the game when it
/// is still open (it waits for this process, skate-launcher crash.rs watch), else a new one from the launcher's layout
/// (`<folder>\Skate 3 Remaster.exe` + `<folder>\game\`). False (the PowerShell popup) for a game run on its own.
fn launcher_shows(path: &Path) -> bool {
    #[cfg(windows)]
    if std::env::var("SKATE_LAUNCHER_PID")
        .ok()
        .and_then(|pid| pid.parse().ok())
        .is_some_and(running)
    {
        return true;
    }
    std::env::var_os("SKATE_LAUNCHER_EXE").map(std::path::PathBuf::from) // the launcher that started us, wherever the game moved: Not retail (user 10-13)
        .or_else(|| std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.join("Skate 3 Remaster.exe"))))
        .filter(|launcher| launcher.is_file())
        .is_some_and(|launcher| Command::new(launcher).arg("--crash").arg(path).spawn().is_ok())
}

#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
        CloseHandle(process);
        alive
    }
}

fn popup(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-STA",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                include_str!("crash_report_ui.ps1"),
            ])
            .env("SKATE_REPORT_PATH", path)
            .creation_flags(0x08000000)
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other("PowerShell UI failed"));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_a_report_keeps_only_the_newest_fifty() {
        let folder = std::env::temp_dir().join("skate-crash-prune");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // Stamps of differing digit counts: newest by number, not by name.
        for stamp in (1..=53u128).map(|i| i * 1_000_000_000_000_000 / 7) {
            std::fs::write(folder.join(format!("report-{stamp}-42.txt")), "x").unwrap();
        }
        for other in ["notes.txt", "report-x-1.txt"] {
            std::fs::write(folder.join(other), "x").unwrap();
        }
        prune_reports(&folder, KEEP_REPORTS);
        let mut left: Vec<String> =
            std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left.len(), 52);
        assert!(left.contains(&"notes.txt".to_owned()) && left.contains(&"report-x-1.txt".to_owned()));
        for gone in 1..=3u128 {
            assert!(!left.contains(&format!("report-{}-42.txt", gone * 1_000_000_000_000_000 / 7)));
        }
        assert!(left.contains(&format!("report-{}-42.txt", 53 * 1_000_000_000_000_000u128 / 7)));
    }

    #[test]
    fn bounded_and_private() {
        let mut c = Capture::default();
        for i in 0..10000 {
            c.line("stderr", format!("REPORT_TRANSITION {i}").as_bytes(), 0.);
        }
        assert_eq!(c.logs.len(), LOG_LIMIT);
        assert_eq!(c.transitions.len(), 64);
        for secret in [
            "ticket abc",
            "SESSION=123",
            "C:\\Users\\Some Person\\file",
            "/home/name/a",
            "Unknown argument abc",
        ] {
            assert!(sanitize(secret).contains("omitted"));
        }
        assert_eq!(
            sanitize("panic at crates/skate-game/src/main.rs:12"),
            "panic at crates/skate-game/src/main.rs:12"
        );
        assert!(sanitize(&"x".repeat(100000)).len() <= LINE_LIMIT);
    }

    #[test]
    fn endpoints_and_ids_are_masked() {
        for (line, want) in [
            ("MULTIPLAYER link lost (peer 76561198012345678)", "MULTIPLAYER link lost (peer [masked])"),
            ("connected to 192.168.1.20:27015 ok", "connected to [masked] ok"),
            ("from [2001:db8::1]:9000", "from [[masked]]:9000"),
            ("bare fe80::1c2:3 and ::ffff:10.0.0.1", "bare [masked] and [masked]"),
            ("lobby 109775241000000000", "lobby [masked]"),
            // Kept: source locations, Rust paths, clock times, short numbers.
            ("bevy_ecs::schedule at main.rs:12:5", "bevy_ecs::schedule at main.rs:12:5"),
            ("at 12:34:56.789 frame 1234567", "at 12:34:56.789 frame 1234567"),
        ] {
            assert_eq!(sanitize(line), want, "{line}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn only_a_running_launcher_counts() {
        assert!(running(std::process::id()));
        let mut ended = Command::new("cmd").args(["/C", "exit"]).spawn().unwrap();
        let pid = ended.id();
        ended.wait().unwrap();
        drop(ended); // its handle closed: the pid names no process now
        assert!(!running(pid));
    }

    /// User 10-27 (Performance Mode): the game's RESTART line, then a clean exit, starts it again; a crash after the
    /// line, or a clean exit without it (Quit), ends the supervisor as before.
    #[cfg(windows)]
    #[test]
    fn a_clean_exit_after_the_restart_line_starts_the_game_again() {
        let exit = |code: &str| Command::new("cmd").args(["/C", "exit", code]).status();
        let mut asked = Capture::default();
        asked.line("stderr", RESTART.as_bytes(), 1.);
        assert!(restarting(&asked, &exit("0")));
        assert!(!restarting(&asked, &exit("1")), "a crash after the line is a crash");
        assert!(!restarting(&Capture::default(), &exit("0")), "Quit");
        let mut said = Capture::default();
        said.line("stderr", format!("INFO {RESTART} mentioned").as_bytes(), 1.);
        assert!(!restarting(&said, &exit("0")), "only the line itself");
    }

    #[cfg(windows)]
    #[test]
    fn the_report_says_what_pc_it_ran_on() {
        let text = report(&Capture::default(), "exit=1", 0.);
        let cpu = text.lines().find(|l| l.starts_with("CPU: ")).expect("CPU line");
        assert!(cpu.ends_with(" threads)") && !cpu.contains("unknown"), "{cpu}");
        let memory = text.lines().find(|l| l.starts_with("Memory: ")).expect("Memory line");
        assert!(memory.contains(" GB total, ") && memory.ends_with(" GB free"), "{memory}");
        let gpu = text.lines().find(|l| l.starts_with("GPU: ")).expect("a graphics card on the test machine");
        assert!(gpu.ends_with(" GB VRAM)") && !gpu.contains("Basic Render"), "{gpu}");
        println!("{cpu}\n{memory}\n{gpu}");
    }

    #[test]
    fn failure_evidence_survives_log_flood_and_missing_data_is_explicit() {
        let mut capture = Capture::default();
        capture.line("stderr", b"REPORT_META stage=stock_graphs", 1.);
        capture.line("stderr", b"REPORT_PANIC bad index", 2.);
        capture.line(
            "stderr",
            b"REPORT_NATIVE code=0xC0000005 fault_pc=0x1234",
            2.,
        );
        for _ in 0..1000 {
            capture.line("stdout", b"ordinary log", 3.);
        }
        let text = report(&capture, "exit=1", 4.);
        assert!(text.contains("stage=stock_graphs"));
        assert!(text.contains("bad index"));
        assert!(text.contains("fault_pc=0x1234"));
        assert!(text.contains("Unavailable / not recorded"));
        assert!(!report(&Capture::default(), "exit=1", 0.).contains("stage=stock_graphs"));
    }

    /// Sentry 10-26: ten render systems panicked at once on a lost GPU device and their `<unknown>` frames filled the
    /// log. The first panic and the device-lost reason are metadata, which no flood can push out.
    #[test]
    fn the_first_panic_and_a_lost_gpu_outlast_a_panic_flood() {
        let mut capture = Capture::default();
        capture.line("stderr", b"REPORT_META gpu_lost=Unknown: Device is lost (thread unnamed)", 1.);
        capture.line("stderr", b"REPORT_META panic=wgpu error: Out of Memory at wgpu_core.rs:2218:18 (thread main)", 1.);
        for _ in 0..1000 {
            capture.line("stderr", b"REPORT_PANIC called `Option::unwrap()` on a `None` value", 2.);
        }
        let text = report(&capture, "exit=1", 3.);
        assert!(text.contains("gpu_lost=Unknown: Device is lost"), "{text}");
        assert!(text.contains("panic=wgpu error: Out of Memory"), "{text}");
        let trace = "   0: <unknown>\n   1: skate3rust::main\n             at crates/skate-game/src/main.rs:1:1\n   2: <unknown>";
        assert_eq!(stack_lines(trace).collect::<Vec<_>>(), ["   1: skate3rust::main", "             at crates/skate-game/src/main.rs:1:1"]);
    }
}
