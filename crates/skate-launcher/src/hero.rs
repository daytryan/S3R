//! The Play screen's hero: the player's own skater and board, a transparent clip the game renders on this PC
//! (`skate3rust.exe --hero-clip`, crates/skate-game hero_clip). This side decides when the clip is out of date, renders
//! it while nothing else uses the game or the GPU, and hands it to the page as base64 (the page loads nothing else).
use crate::app::App;
use serde_json::{json, Value};
use std::{os::windows::process::CommandExt, path::{Component, Path, PathBuf}, sync::Arc, time::{Duration, Instant, SystemTime}};

/// What the game tries, simplest last: a run that lands no trick (exit 4) retries with fewer tricks.
pub const LADDER: [&str; 3] = ["Ollie,Kickflip,360Flip", "Ollie,Kickflip", "Ollie"];
/// The page never gets a clip bigger than this (the game aims for far less).
const MAX_CLIP: u64 = 8 * 1024 * 1024;
/// The game's own watchdog ends a run at 90 s (exit 6); this is for a run that hangs past it.
const TIMEOUT: Duration = Duration::from_secs(120);
const NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY: u32 = 0x0000_4000;

pub struct Paths { pub webm: PathBuf, pub json: PathBuf, pub character: PathBuf, pub installation: PathBuf }

/// The clip's files and what it depends on, for the installation `game\data\installation.json` names. None before a build.
pub fn paths(game: &Path) -> Option<Paths> {
    let data = game.join("data");
    let installation = data.join("installation.json");
    // Feature #2 (user 10-14): only `directory` is read; the ~23 MB rest is skipped, never built as a Value tree.
    #[derive(serde::Deserialize)]
    struct Marker { directory: String }
    let marker: Marker = [installation.clone(), data.join("installation.json.bak")].iter()  // B5: the kept copy if cut short
        .find_map(|f| serde_json::from_slice(&std::fs::read(f).ok()?).ok())?;
    let directory = Path::new(&marker.directory);
    if !directory.components().all(|c| matches!(c, Component::Normal(_))) { return None; }
    // F2: the game keeps saves in data\settings once it moved them there (saves.rs)
    let saves = if data.join("settings").is_dir() { data.join("settings") } else { data.join(directory).join("settings") };
    Some(Paths { webm: data.join("hero/hero.webm"), json: data.join("hero/hero.json"), character: active_look(&saves), installation })
}

/// The look the player skates as: team.json's active member (teammate<k>.json), else the boss's character.json.
/// Mirrors the game's team_data::active_look_path (team version 1, member below 1 + teammates, at most 5).
fn active_look(settings: &Path) -> PathBuf {
    let team = std::fs::read(settings.join("team.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let member = team.filter(|t| t["version"] == 1).map_or(0, |t| {
        let active = t["active"].as_u64().unwrap_or(0) as usize;
        if active < 1 + t["teammates"].as_array().map_or(0, |m| m.len().min(4)) { active } else { 0 }
    });
    if member == 0 { settings.join("character.json") } else { settings.join(format!("teammate{member}.json")) }
}

/// sha256 of the active look file, or "none" before the player ever saved a skater: what hero.json records.
fn character(p: &Paths) -> String {
    use sha2::{Digest, Sha256};
    std::fs::read(&p.character).map_or("none".into(), |b| format!("{:x}", Sha256::digest(b)))
}

fn modified(path: &Path) -> Option<SystemTime> { std::fs::metadata(path).and_then(|m| m.modified()).ok() }

/// Missing, made for another look (a Create-a-Skater edit), or older than the installation (a build or update).
pub fn stale(p: &Paths) -> bool {
    let recorded = std::fs::read(&p.json).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let Some(recorded) = recorded.filter(|_| p.webm.is_file()) else { return true };
    recorded["character"].as_str() != Some(character(p).as_str())
        || !matches!((modified(&p.webm), modified(&p.installation)), (Some(clip), Some(install)) if clip >= install)
}

/// One game on the GPU at a time: never while the player's game, a build, an update or another render runs.
pub fn should_render(stale: bool, game: bool, building: bool, updating: bool, rendering: bool) -> bool {
    stale && !game && !building && !updating && !rendering
}

pub fn next_rung(rung: usize, exit: i32) -> Option<usize> { (exit == 4 && rung + 1 < LADDER.len()).then_some(rung + 1) }

#[derive(Debug, PartialEq)]
enum Next { Send, Rung(usize), Fail, Later }

/// After a run (`run`'s result, and whether the clip is still stale): send it, try fewer tricks, give up on this look
/// and installation, or try again later. Only a run something else stopped is tried again, so nothing loops forever.
fn after(rung: usize, code: Option<i32>, stale: bool) -> Next {
    match code {
        Some(0) if !stale => Next::Send,
        None => Next::Later,
        Some(c) => next_rung(rung, c).map_or(Next::Fail, Next::Rung),
    }
}

/// Windows refuses write access to a running program's file, so this sees the game however it was started.
// ponytail: checked only while the clip is stale; the open could make a Play pressed in that same instant fail
fn game_running(exe: &Path) -> bool { exe.is_file() && std::fs::OpenOptions::new().write(true).open(exe).is_err() }

/// The page's `hero` message, or None when there is no clip or it is too big to send.
pub fn message(p: &Paths) -> Option<Value> {
    if std::fs::metadata(&p.webm).ok()?.len() > MAX_CLIP { return None; }
    let info: Value = serde_json::from_slice(&std::fs::read(&p.json).ok()?).ok()?;
    Some(json!({"type": "hero", "webm": base64(&std::fs::read(&p.webm).ok()?), "speed": info["speed"].as_f64()?,
                "still": info["still"].as_f64().unwrap_or(0.3)}))   // the frame to rest on: just after the first landing
}

/// What the page shows now (bug 10-17 #51): the clip, or `off` (the plain board) while it is out of date, so an old
/// outfit never shows; the watch sends the new clip once it is rendered.
pub fn current(p: &Paths) -> Option<Value> {
    if stale(p) { Some(off()) } else { message(p) }
}

fn off() -> Value { json!({"type": "hero", "off": true}) }

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 { out.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' }); }
    }
    out
}

/// One line per render outcome in `data\hero\hero.log` (the launcher has no window to show it in, by design).
fn log(p: &Paths, text: &str) {
    use std::io::Write;
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let file = p.webm.with_file_name("hero.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 1 << 20) { let _ = std::fs::remove_file(&file); }  // C6: never grows past 1 MB
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{stamp} {text}");
    }
}

/// The `--hero-clip` render running now (0: none).
pub(crate) static RENDER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Play pressed: stop a render so it does not share the GPU with the player's game. Taken out of the helpers first,
/// so `run` reads it as stopped (tried again once the game is free), not as a failed ladder rung.
pub fn stop(app: &App) {
    let pid = RENDER.swap(0, std::sync::atomic::Ordering::SeqCst);
    let mut helpers = app.helpers.lock().unwrap();
    if pid != 0 && helpers.contains(&pid) {
        helpers.retain(|&h| h != pid);
        crate::pipeline::kill(pid);
    }
}

/// One `--hero-clip` run. Its pid sits with the checks (`app.helpers`), so an update or closing the launcher stops it.
/// The exit code, -1 when it could not start or hung (a failure, like any code but 0 and 4); None only when an update
/// or closing the launcher stopped it, so it is tried again once the game is free.
fn run(app: &Arc<App>, p: &Paths, rung: usize) -> Option<i32> {
    let game = app.game();
    let _ = std::fs::create_dir_all(p.webm.parent()?);
    let spawned = std::process::Command::new(game.join("skate3rust.exe")).current_dir(&game)
        .arg("--hero-clip").arg(&p.webm)
        .env("SKATE_REPORT_CHILD", "1").env("SKATE_HERO_TRICKS", LADDER[rung])
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .creation_flags(NO_WINDOW | BELOW_NORMAL_PRIORITY).spawn();
    let mut child = match spawned { Ok(c) => c, Err(e) => { log(p, &format!("could not start: {e}")); return Some(-1); } };
    let pid = child.id();
    crate::pipeline::dies_with_launcher(&child);  // A2: Play's stop is then instant, and it never outlives the launcher
    app.helpers.lock().unwrap().push(pid);
    RENDER.store(pid, std::sync::atomic::Ordering::SeqCst);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(250)),
            Ok(None) => { crate::pipeline::kill(pid); log(p, "timed out"); break Some(-1); }
            done => break done.ok().flatten().map(|s| s.code().unwrap_or(-1)),
        }
    };
    crate::pipeline::released(pid);
    let _ = RENDER.compare_exchange(pid, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst);
    let mut helpers = app.helpers.lock().unwrap();
    let ours = helpers.contains(&pid);
    helpers.retain(|&h| h != pid);
    if !ours { log(p, "stopped (update or close)"); return None; }
    Some(status.unwrap_or(-1))
}

/// Wakes the watch: the game closed, a build or an update ended. The watch sleeps on this, so an idle launcher does no
/// work at all (it used to look every 5 s, re-reading the 23 MB installation.json each time).
type Poke = (std::sync::Mutex<bool>, std::sync::Condvar);
static POKE: Poke = (std::sync::Mutex::new(false), std::sync::Condvar::new());

pub fn poke() { poke_on(&POKE) }
fn wait_for_poke() { wait_on(&POKE) }

fn poke_on(p: &Poke) {
    *p.0.lock().unwrap() = true;
    p.1.notify_all();
}

fn wait_on(p: &Poke) {
    let mut poked = p.0.lock().unwrap();
    while !*poked { poked = p.1.wait(poked).unwrap(); }
    *poked = false;
}

/// Keeps the clip current for as long as the launcher runs: once at start and after each poke, a stale clip is rendered
/// if nothing else needs the game (the player's game, a build or an update), and the result goes to the page. A ladder
/// that fails is not retried until the look or the installation changes again. Started once, on the page's `ready`.
pub fn watch(app: Arc<App>) {
    std::thread::spawn(move || {
        let mut failed: Option<(String, Option<SystemTime>)> = None;
        loop {
            if let Some(p) = paths(&app.game()) {
                let key = (character(&p), modified(&p.installation));
                let building = app.running.lock().unwrap().is_some();
                // #51: a look saved in the game (or a rebuild) hides the old clip at once, rendered or not
                if stale(&p) { app.send(off()); }
                if failed.as_ref() != Some(&key) && should_render(stale(&p), app.game_pid.lock().unwrap().is_some() || game_running(&app.game().join("skate3rust.exe")),
                                                                   building, crate::update::updating(), false) {
                    let mut rung = 0;
                    loop {
                        let code = run(&app, &p, rung);
                        log(&p, &format!("tricks {} -> {}", LADDER[rung], code.map_or("no exit code".into(), |c| c.to_string())));
                        match after(rung, code, stale(&p)) {
                            Next::Send => { if let Some(m) = message(&p) { app.send(m); } failed = None; break; }
                            Next::Rung(next) => rung = next,
                            Next::Fail => { failed = Some(key); break; }   // a look changed mid-run makes a new key: rendered again
                            Next::Later => break,
                        }
                    }
                }
            }
            wait_for_poke();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap(); d
    }
    fn fake_paths(game: &Path) -> Paths {   // the layout setup.rs expects: data\installation.json -> installations\<hex>
        let data = game.join("data");
        std::fs::create_dir_all(data.join("installations/abc/settings")).unwrap();
        std::fs::write(data.join("installation.json"), r#"{"directory":"installations/abc"}"#).unwrap();
        std::fs::write(data.join("installations/abc/settings/character.json"), b"look A").unwrap();
        paths(game).unwrap()
    }
    fn sha_hex(bytes: &[u8]) -> String { use sha2::{Digest, Sha256}; format!("{:x}", Sha256::digest(bytes)) }
    fn write_clip(p: &Paths, character: &str) {
        std::thread::sleep(std::time::Duration::from_millis(20));   // mtime after installation.json
        std::fs::create_dir_all(p.webm.parent().unwrap()).unwrap();
        std::fs::write(&p.webm, b"\x1aE\xdf\xa3").unwrap();
        std::fs::write(&p.json, format!(r#"{{"speed":5.0,"character":"{character}"}}"#)).unwrap();
    }
    fn touch_later(path: &Path) { std::thread::sleep(std::time::Duration::from_millis(20)); let b = std::fs::read(path).unwrap(); std::fs::write(path, b).unwrap(); }

    #[test]
    fn the_clip_follows_the_member_the_player_skates_as() {
        let d = temp("hero-team"); let p = fake_paths(&d);
        let settings = p.character.parent().unwrap().to_path_buf();
        assert_eq!(p.character, settings.join("character.json"), "no team yet: the boss");
        let team = |active: usize| std::fs::write(settings.join("team.json"),
            format!(r#"{{"version":1,"name":"t","logo":"0","boss":"b","teammates":["a","c"],"active":{active}}}"#)).unwrap();
        team(2);
        assert_eq!(paths(&d).unwrap().character, settings.join("teammate2.json"));
        team(3);
        assert_eq!(paths(&d).unwrap().character, settings.join("character.json"), "member 3 is not on the team");
        std::fs::write(settings.join("team.json"), r#"{"version":1,"teammates":["a"]}"#).unwrap();
        assert_eq!(paths(&d).unwrap().character, settings.join("character.json"), "a team.json from before Select Skater");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_clip_is_stale_when_missing_when_the_look_changed_or_after_a_rebuild() {
        let d = temp("hero-stale"); let p = fake_paths(&d);   // writes installation.json + character.json
        assert!(stale(&p), "no clip yet");
        write_clip(&p, &sha_hex(b"look A"));                   // webm + hero.json written after installation.json
        std::fs::write(&p.character, b"look A").unwrap();
        assert!(!stale(&p));
        std::fs::write(&p.character, b"look B").unwrap();
        assert!(stale(&p), "Create-a-Skater edit");
        write_clip(&p, &sha_hex(b"look B"));
        touch_later(&p.installation);
        assert!(stale(&p), "a build or update rewrote the installation");
        // never used Create-a-Skater: "none", until a look is saved
        std::fs::remove_file(&p.character).unwrap();
        write_clip(&p, "none");
        assert!(!stale(&p), "the default skater's clip is current");
        std::fs::write(&p.character, b"look C").unwrap();
        assert!(stale(&p), "\"none\" -> a real look");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn an_old_looks_clip_is_never_sent() {
        let d = temp("hero-current"); let p = fake_paths(&d);
        std::fs::write(&p.character, b"look A").unwrap();
        write_clip(&p, &sha_hex(b"look A"));
        assert!(current(&p).unwrap()["webm"].is_string(), "the current clip");
        std::fs::write(&p.character, b"look B").unwrap();
        assert_eq!(current(&p), Some(json!({"type": "hero", "off": true})), "bug 10-17 #51: the plain board instead");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn never_renders_while_the_game_a_build_an_update_or_another_render_runs() {
        assert!(should_render(true, false, false, false, false));
        for busy in [(true, false, false, false), (false, true, false, false), (false, false, true, false), (false, false, false, true)] {
            assert!(!should_render(true, busy.0, busy.1, busy.2, busy.3));
        }
        assert!(!should_render(false, false, false, false, false));
    }

    #[test]
    fn the_retry_ladder_drops_tricks_and_then_gives_up() {
        assert_eq!(LADDER, ["Ollie,Kickflip,360Flip", "Ollie,Kickflip", "Ollie"]);
        assert_eq!(next_rung(0, 4), Some(1));
        assert_eq!(next_rung(2, 4), None);
        assert_eq!(next_rung(0, 5), None, "only 'no landed trick' (4) retries");
    }

    #[test]
    fn only_a_stopped_render_is_tried_again_and_a_run_that_leaves_no_current_clip_fails() {
        assert_eq!(after(0, Some(0), false), Next::Send);
        assert_eq!(after(0, Some(0), true), Next::Fail, "exit 0 but no current clip: not every 5 s forever");
        assert_eq!(after(0, Some(4), true), Next::Rung(1));
        assert_eq!(after(2, Some(4), true), Next::Fail);
        assert_eq!(after(0, Some(-1), true), Next::Fail, "could not start or hung: not every 5 s forever");
        assert_eq!(after(1, None, true), Next::Later, "an update or close stopped it");
    }

    #[test]
    fn a_clip_over_8_mb_is_not_sent_to_the_page() {
        let d = temp("hero-big"); let p = fake_paths(&d);
        std::fs::create_dir_all(p.webm.parent().unwrap()).unwrap();
        std::fs::write(&p.json, r#"{"speed":5.5,"character":"none"}"#).unwrap();
        std::fs::write(&p.webm, vec![0u8; 8 * 1024 * 1024 + 1]).unwrap();
        assert!(message(&p).is_none());
        std::fs::write(&p.webm, b"\x1aE\xdf\xa3tiny").unwrap();
        let m = message(&p).unwrap();
        assert_eq!((m["type"].as_str(), m["speed"].as_f64(), m["webm"].as_str()), (Some("hero"), Some(5.5), Some("GkXfo3Rpbnk=")));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn the_still_moment_reaches_the_page_and_defaults_to_0_3_s() {
        let d = temp("hero-still"); let p = fake_paths(&d);
        std::fs::create_dir_all(p.webm.parent().unwrap()).unwrap();
        std::fs::write(&p.webm, b"\x1aE\xdf\xa3").unwrap();
        std::fs::write(&p.json, r#"{"speed":5.0,"character":"none","still":1.4}"#).unwrap();
        assert_eq!(message(&p).unwrap()["still"].as_f64(), Some(1.4));
        std::fs::write(&p.json, r#"{"speed":5.0,"character":"none"}"#).unwrap();
        assert_eq!(message(&p).unwrap()["still"].as_f64(), Some(0.3), "an older game's hero.json");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn the_watch_sleeps_until_poked_and_a_poke_is_not_lost() {
        // its own signal: other tests poke the launcher's one (a restore, a build) while this runs in parallel
        static P: Poke = (std::sync::Mutex::new(false), std::sync::Condvar::new());
        poke_on(&P);  // before anyone waits: the next wait returns at once
        wait_on(&P);
        let waiter = std::thread::spawn(|| wait_on(&P));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waiter.is_finished(), "nothing to do: it sleeps");
        poke_on(&P);
        waiter.join().unwrap();
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!([b"" as &[u8], b"M", b"Ma", b"Man", b"\xff\xfe\xfd\xfc"].map(base64), ["", "TQ==", "TWE=", "TWFu", "//79/A=="]);
    }

    #[test]
    fn the_render_log_starts_again_past_1_mb() {
        let d = temp("hero-log"); let p = fake_paths(&d);
        std::fs::create_dir_all(p.webm.parent().unwrap()).unwrap();
        let file = p.webm.with_file_name("hero.log");
        std::fs::write(&file, vec![b'x'; (1 << 20) + 1]).unwrap();
        log(&p, "tricks Ollie -> 0");
        assert!(std::fs::metadata(&file).unwrap().len() < 100);
        std::fs::remove_dir_all(d).unwrap();
    }
}
