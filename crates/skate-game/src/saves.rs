//! Where the player's saves live (F2, launcher audit 10-12). Not retail (user 10-13).
//!
//! They used to sit in `data\installations\<id>\settings`, inside the folder a build replaces and deletes. They now
//! live in `data\settings`, which no build touches. A development checkout (`--assets`) keeps `<checkout>\settings`.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set when [`migrate`] could not move the saves: this run keeps using the old folder, and the next start tries again.
static NOT_MOVED: AtomicBool = AtomicBool::new(false);

/// The settings folder for an asset root: `data\settings` for an installed game (`data\installations\<id>\assets`),
/// else the folder beside the assets, as before.
pub(crate) fn settings_dir(asset_root: &Path) -> PathBuf {
    let install = asset_root.parent().unwrap_or(asset_root);
    if NOT_MOVED.load(Ordering::Relaxed) {
        return install.join("settings");
    }
    match install.parent() {
        Some(installations) if installations.file_name().is_some_and(|n| n.eq_ignore_ascii_case("installations")) => {
            installations.parent().unwrap_or(installations).join("settings")
        }
        _ => install.join("settings"),
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) { out.extend(files(&path)) } else { out.push(path) }
    }
    out
}

/// The newest save in a settings folder (None: none). default-map.json is the build's (install.py writes it into
/// every new installation), not the player's, so it doesn't count.
fn newest(dir: &Path) -> Option<std::time::SystemTime> {
    files(dir).iter().filter(|p| !p.ends_with("default-map.json")).filter_map(|p| p.metadata().ok()?.modified().ok()).max()
}

/// First start with `data\settings` missing: the saves move there from the installation that holds the newest ones
/// (normally this one; after a lost installation.json the build may have kept an older one, runner.py leftovers()).
/// They are copied to `settings.migrating` beside the target, every file is compared, and only then is the copy
/// renamed into place (same folder, so never across drives) and the old folder removed. On any failure the old saves
/// stay where they are and this run keeps using them (`settings_dir`). This installation's default-map.json comes
/// along when the saves came from another one.
pub(crate) fn migrate(asset_root: &Path) -> Result<bool, String> {
    let moved = migrate_from(asset_root);
    if moved.is_err() {
        NOT_MOVED.store(true, Ordering::Relaxed);
    }
    moved
}

fn migrate_from(asset_root: &Path) -> Result<bool, String> {
    let target = settings_dir(asset_root);
    let own = asset_root.parent().unwrap_or(asset_root).join("settings");
    if target == own || target.exists() {
        return Ok(false);
    }
    let installations = own.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let source = std::fs::read_dir(&installations).into_iter().flatten().flatten()
        .map(|e| e.path().join("settings"))
        .filter_map(|dir| Some((newest(&dir)?, dir)))
        .max_by_key(|(time, dir)| (*time, *dir == own))
        .map(|(_, dir)| dir)
        .or_else(|| own.is_dir().then(|| own.clone()));  // only the build's default map so far
    let Some(source) = source else { return Ok(false) };
    let staging = target.with_extension("migrating");
    let _ = std::fs::remove_dir_all(&staging);
    let copied = (|| -> Result<(), String> {
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        for file in files(&source) {
            let to = staging.join(file.strip_prefix(&source).unwrap());
            std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::copy(&file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
            if std::fs::read(&file).ok() != std::fs::read(&to).ok() {
                return Err(format!("{}: the copy differs", file.display()));
            }
        }
        let map = staging.join("default-map.json");
        if !map.exists() && own.join("default-map.json").is_file() {
            std::fs::copy(own.join("default-map.json"), map).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&source);
    Ok(true)
}

// Save safety (release review 10-18, B1-B3; not retail, by request (owner 10-18)). Every save in `settings_dir` is
// written whole ([`write_atomic`]) and read through [`load`]: a file cut short by a crash comes back from the last copy
// that loaded, a damaged one is kept aside, and one that can't be read (locked, access denied) is never saved over.

/// Saves whose read failed for a reason other than "not there" this session: never written over, or the defaults
/// they fell back to would replace the player's file.
static BLOCKED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
/// Saves whose good copy has been kept as `.last-good` this session (once each: a later bad write never reaches it).
static BACKED_UP: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Logged, and on stderr: gameplay.json and default-map.json are read before the app (and its log) start.
// ponytail: printed twice once the log is up, fine for a rare damaged-save line.
macro_rules! report {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        eprintln!("{line}");
        bevy::log::error!("{line}");
    }};
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}
/// `<name>.last-good`: the last copy of `path` that loaded. Not `.bak`: graphics.json.bak is the player's own settings
/// a --safe start or the launcher's reset moved aside (F4), and must neither be replaced nor restored from.
pub(crate) fn backup(path: &Path) -> PathBuf {
    beside(path, ".last-good")
}

/// `bytes` as `path`, whole or not at all: a temp file beside it, flushed to disk, then renamed over it. Refused for
/// a save [`load`] could not read this session.
///
/// perf 10-18: the write itself goes to the save thread ([`queue`]); a failure there is logged. SKATE_SYNC_SAVES=1
/// writes here, as before.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if BLOCKED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p == path) {
        return Err(std::io::Error::other(format!("{} could not be read this session, so it is not saved over", path.display())));
    }
    if sync_saves() {
        return write_now(path, bytes);
    }
    queue(path, bytes);
    Ok(())
}

/// SKATE_SYNC_SAVES=1: saves are written on the calling thread. Tests write there too: they read files right back.
fn sync_saves() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| cfg!(test) || std::env::var("SKATE_SYNC_SAVES").is_ok_and(|v| v == "1"))
}

/// Saves waiting for the save thread, oldest first, the latest bytes for each path; and whether it is writing one.
#[derive(Default)]
struct Queue {
    waiting: Vec<(PathBuf, Vec<u8>)>,
    writing: bool,
}
static QUEUE: std::sync::Mutex<Queue> = std::sync::Mutex::new(Queue { waiting: Vec::new(), writing: false });
/// Signals the save thread (new work) and [`flush`] (work done).
static CHANGED: std::sync::Condvar = std::sync::Condvar::new();
/// Saves queued or being written; the crash path waits on it without a lock ([`drain_for_crash`]).
static OUTSTANDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn queue_lock() -> std::sync::MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

/// perf 10-18: `bytes` for `path` on the save thread: sync_all took a few ms of the main thread for every settings
/// change and progress save. A later save of the same path replaces one still waiting (latest wins).
fn queue(path: &Path, bytes: &[u8]) {
    static THREAD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let started = *THREAD.get_or_init(|| std::thread::Builder::new().name("saves".into()).spawn(save_thread).is_ok());
    if !started {
        if let Err(e) = write_now(path, bytes) {
            report!("Save {}: {e}", path.display());
        }
        return;
    }
    let mut queue = queue_lock();
    match queue.waiting.iter_mut().find(|(p, _)| p == path) {
        Some((_, waiting)) => *waiting = bytes.to_vec(),
        None => {
            OUTSTANDING.fetch_add(1, Ordering::SeqCst);
            queue.waiting.push((path.to_path_buf(), bytes.to_vec()));
        }
    }
    CHANGED.notify_all();
}

fn save_thread() {
    let mut queue = queue_lock();
    loop {
        if queue.waiting.is_empty() {
            queue = CHANGED.wait(queue).unwrap_or_else(|e| e.into_inner());
            continue;
        }
        let (path, bytes) = queue.waiting.remove(0);
        queue.writing = true;
        drop(queue);
        if let Err(e) = write_now(&path, &bytes) {
            report!("Save {}: {e}", path.display());
        }
        queue = queue_lock();
        queue.writing = false;
        OUTSTANDING.fetch_sub(1, Ordering::SeqCst);
        CHANGED.notify_all();
    }
}

/// Waits until every queued save is on disk.
pub(crate) fn flush() {
    if OUTSTANDING.load(Ordering::SeqCst) == 0 {
        return;
    }
    let mut queue = queue_lock();
    while !queue.waiting.is_empty() || queue.writing {
        queue = CHANGED.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// A save's bytes as written last, queued or not: what `std::fs::read` would give once the save thread is done.
pub(crate) fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    flush();
    std::fs::read(path)
}

/// Whether a save is there, counting one still queued.
pub(crate) fn exists(path: &Path) -> bool {
    flush();
    path.exists()
}

/// The crash path (a panic, a native fault): the save thread gets up to two seconds to finish, without taking a lock
/// the crashing thread may hold.
pub(crate) fn drain_for_crash() {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while OUTSTANDING.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// On AppExit, every queued save is written before the process ends.
pub(crate) fn flush_on_exit(exits: bevy::ecs::message::MessageReader<bevy::app::AppExit>) {
    if !exits.is_empty() {
        flush();
    }
}

fn write_now(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let pending = beside(path, ".pending");
    let written = (|| {
        let mut file = std::fs::File::create(&pending)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written.and_then(|()| std::fs::rename(&pending, path)) {
        let _ = std::fs::remove_file(&pending);
        return Err(error);
    }
    Ok(())
}

/// What [`load`] found.
#[derive(Debug, PartialEq)]
pub(crate) enum Loaded<T> {
    /// No file: a first run.
    Missing,
    Loaded(T),
    /// The file was damaged: it is kept as `.bad-<unix>`, and this came from `.last-good` (now the file again).
    Restored(T, PathBuf),
    /// The file was damaged and there is no good `.last-good`: kept as `.bad-<unix>`; defaults.
    Damaged(PathBuf),
    /// The file could not be read (locked, access denied): defaults for this session, and it is not saved over.
    Unreadable,
}
impl<T> Loaded<T> {
    pub(crate) fn ok(self) -> Option<T> {
        match self {
            Self::Loaded(value) | Self::Restored(value, _) => Some(value),
            _ => None,
        }
    }
    /// The line the player is told, when there is one to tell.
    pub(crate) fn problem(&self) -> Option<String> {
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match self {
            Self::Restored(_, bad) => Some(format!("A damaged save was restored from its backup (kept as {})", name(bad))),
            Self::Damaged(bad) => Some(format!("A damaged save couldn't be loaded (kept as {})", name(bad))),
            Self::Unreadable => Some("A save couldn't be read, so it won't be saved over this session".into()),
            _ => None,
        }
    }
}

/// Nothing saves over `path` this session (it could not be judged: [`load`]'s Unreadable).
pub(crate) fn block(path: &Path) {
    BLOCKED.lock().unwrap_or_else(|e| e.into_inner()).push(path.to_path_buf());
}

/// `path` renamed to `<name>.bad-<unix>` (`-2`, `-3`... when one is already there that second): kept, never overwritten.
pub(crate) fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let bad = (1..).map(|n| beside(path, &if n == 1 { format!(".bad-{stamp}") } else { format!(".bad-{stamp}-{n}") }))
        .find(|p| !p.exists())
        .unwrap_or_default();
    std::fs::rename(path, &bad)?;
    Ok(bad)
}

/// `path` through `parse` (None: damaged), with the rules above.
pub(crate) fn load<T>(path: &Path, parse: impl Fn(&[u8]) -> Option<T>) -> Loaded<T> {
    let bytes = match read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => {
            report!("Save {}: could not be read ({e}); not saved over this session", path.display());
            block(path);
            return Loaded::Unreadable;
        }
    };
    if let Some(value) = parse(&bytes) {
        let mut backed_up = BACKED_UP.lock().unwrap_or_else(|e| e.into_inner());
        if !backed_up.iter().any(|p| p == path) {
            backed_up.push(path.to_path_buf());
            if let Err(e) = write_atomic(&backup(path), &bytes) {
                report!("Save backup {}: {e}", backup(path).display());
            }
        }
        return Loaded::Loaded(value);
    }
    let bad = match set_aside(path) {
        Ok(bad) => bad,
        Err(e) => {
            // Not kept aside, so not saved over either.
            report!("Save {} is damaged and could not be set aside ({e}); not saved over this session", path.display());
            block(path);
            return Loaded::Unreadable;
        }
    };
    report!("Save {} is damaged; kept as {}", path.display(), bad.display());
    let restored = std::fs::read(backup(path)).ok().and_then(|b| parse(&b).map(|v| (v, b)));
    match restored {
        Some((value, bytes)) => {
            if let Err(e) = write_atomic(path, &bytes) {
                report!("Save {}: restoring its backup: {e}", path.display());
            }
            report!("Save {}: restored from {}", path.display(), backup(path).display());
            Loaded::Restored(value, bad)
        }
        None => Loaded::Damaged(bad),
    }
}

/// [`load`] of a JSON save.
pub(crate) fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Loaded<T> {
    load(path, |bytes| serde_json::from_slice(bytes).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s3r-save-safety-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn number(bytes: &[u8]) -> Option<u32> {
        std::str::from_utf8(bytes).ok()?.trim().parse().ok()
    }

    /// Release review 10-18 B1: a save cut short by a crash comes back from the last copy that loaded, and the damaged
    /// one is kept, not overwritten.
    #[test]
    fn a_save_cut_short_loads_from_its_backup() {
        let dir = scratch("cut");
        let path = dir.join("career.json");
        assert_eq!(load(&path, number), Loaded::Missing);
        write_atomic(&path, b"41").unwrap();
        assert_eq!(load(&path, number), Loaded::Loaded(41));
        assert_eq!(std::fs::read(backup(&path)).unwrap(), b"41", "the good copy is kept");
        write_atomic(&path, b"42").unwrap();
        assert_eq!(load(&path, number), Loaded::Loaded(42));
        assert_eq!(std::fs::read(backup(&path)).unwrap(), b"41", "once a session: a later write never reaches .last-good");
        std::fs::write(&path, b"4x").unwrap();  // a crash mid-write
        let Loaded::Restored(41, bad) = load(&path, number) else { panic!("restored from .last-good") };
        assert_eq!(std::fs::read(&bad).unwrap(), b"4x", "the damaged file is kept aside");
        assert_eq!(std::fs::read(&path).unwrap(), b"41", "and the save is whole again");
        // No good backup: the damaged file is still kept, and defaults follow.
        let lone = dir.join("gameplay.json");
        std::fs::write(&lone, b"{").unwrap();
        let Loaded::Damaged(bad) = load(&lone, number) else { panic!("damaged") };
        assert_eq!(std::fs::read(bad).unwrap(), b"{");
        assert!(!dir.read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".pending")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// perf 10-18: saves of one path queue for the save thread, the latest replacing one still waiting; a flush (or a
    /// read) sees the last.
    #[test]
    fn queued_saves_land_latest_first() {
        let dir = scratch("queued");
        let path = dir.join("graphics.json");
        for n in 0..20 {
            queue(&path, n.to_string().as_bytes());
        }
        assert_eq!(read(&path).unwrap(), b"19", "a read waits for the queue");
        queue(&path, b"20");
        queue(&dir.join("other.json"), b"x");
        queue(&path, b"21");
        flush();
        assert_eq!(std::fs::read(&path).unwrap(), b"21");
        assert_eq!(std::fs::read(dir.join("other.json")).unwrap(), b"x");
        assert_eq!(OUTSTANDING.load(Ordering::SeqCst), 0);
        assert!(!dir.read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".pending")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Boss 10-18: a save that can't be read (here: another program holds it open with no sharing) is not damage.
    /// It isn't renamed or replaced by defaults, and nothing saves over it this session.
    #[cfg(windows)]
    #[test]
    fn a_locked_save_is_not_saved_over() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("locked");
        let path = dir.join("skate2-tutorial.json");
        std::fs::write(&path, b"7").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        assert_eq!(load(&path, number), Loaded::Unreadable);
        drop(held);
        assert!(write_atomic(&path, b"0").is_err(), "the defaults never replace it");
        assert_eq!(std::fs::read(&path).unwrap(), b"7");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing renamed, nothing left over");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_installed_game_keeps_its_saves_outside_the_installation() {
        let installed = Path::new(r"D:\S3R\game\data\installations\0123abcd\assets");
        assert_eq!(settings_dir(installed), Path::new(r"D:\S3R\game\data\settings"));
        assert_eq!(settings_dir(Path::new(r"C:\dev\checkout\assets")), Path::new(r"C:\dev\checkout\settings"));
    }

    #[test]
    fn the_newest_saves_move_once_verified_and_the_old_folder_goes() {
        let data = std::env::temp_dir().join(format!("s3r-saves-{}", std::process::id())).join("data");
        let _ = std::fs::remove_dir_all(&data);
        let (mine, older) = (data.join("installations/new"), data.join("installations/old"));
        std::fs::create_dir_all(mine.join("assets")).unwrap();
        std::fs::create_dir_all(older.join("settings/mods")).unwrap();
        std::fs::write(older.join("settings/character.json"), b"me").unwrap();
        std::fs::write(older.join("settings/mods/x.json"), b"{}").unwrap();
        // a build after a lost installation.json: this installation has only the build's default map
        std::fs::create_dir_all(mine.join("settings")).unwrap();
        std::fs::write(mine.join("settings/default-map.json"), br#""maps/u.skate""#).unwrap();
        assert!(migrate(&mine.join("assets")).unwrap());
        assert_eq!(std::fs::read(data.join("settings/character.json")).unwrap(), b"me");
        assert_eq!(std::fs::read(data.join("settings/mods/x.json")).unwrap(), b"{}");
        assert_eq!(std::fs::read(data.join("settings/default-map.json")).unwrap(), br#""maps/u.skate""#);
        assert!(!older.join("settings").exists() && !data.join("settings.migrating").exists());
        assert!(!migrate(&mine.join("assets")).unwrap(), "only once: data/settings exists now");
        // a first install: the build's default map alone moves too
        std::fs::remove_dir_all(data.join("settings")).unwrap();
        std::fs::remove_dir_all(&older).unwrap();
        assert!(migrate(&mine.join("assets")).unwrap());
        assert!(data.join("settings/default-map.json").is_file() && !mine.join("settings").exists());
        std::fs::remove_dir_all(data.parent().unwrap()).unwrap();
    }

    /// Every place the game reads or writes a save goes through settings_dir (the F2 swap). A new
    /// `asset_root.parent()...join("settings")` would read the old place, which the next build deletes.
    #[test]
    fn no_code_builds_the_old_settings_path() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                if e.path().is_dir() { walk(&e.path(), out) } else if e.path().extension().is_some_and(|x| x == "rs") { out.push(e.path()) }
            }
        }
        let mut sources = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut sources);
        let patterns = [".join(\"settings\")", ".join(\"settings/"];
        let offenders: Vec<String> = sources.iter().filter(|p| !p.ends_with("saves.rs"))
            .flat_map(|p| std::fs::read_to_string(p).unwrap().lines().enumerate()
                .filter(|(_, l)| patterns.iter().any(|pat| l.contains(pat)))
                .map(|(i, _)| format!("{}:{}", p.display(), i + 1)).collect::<Vec<_>>())
            .collect();
        assert!(offenders.is_empty(), "use crate::saves::settings_dir: {offenders:?}");
    }

    /// Boss 10-18: every save in data/settings is written through write_atomic. The modules that own one write
    /// nothing raw outside their tests (the two renames are deliberate: F4's graphics.json.bak and the old
    /// my-graphics.json marked migrated).
    #[test]
    fn every_save_is_written_whole() {
        let owners = ["boot.rs", "customiser.rs", "config.rs", "difficulty.rs", "graphics_menu.rs", "map_library.rs", "skate1_challenge.rs",
            "online_roster.rs", "player_list.rs", "skate2_challenge.rs", "team_data.rs", "trick_stats.rs", "graphic_store.rs", "skate3_career/save.rs"];
        let allowed = ["std::fs::rename(&file, bak)", "std::fs::rename(&old, old.with_extension(\"json.migrated\"))"];
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for owner in owners {
            let text = std::fs::read_to_string(src.join(owner)).unwrap();
            let body = text.split("\n#[cfg(test)]\nmod tests").next().unwrap();
            let body = body.split("\n#[cfg(test)]\npub(crate) mod tests").next().unwrap();
            for (i, line) in body.lines().enumerate() {
                let raw = ["fs::write(", "fs::rename(", "File::create("].iter().any(|p| line.contains(p));
                if raw && !allowed.iter().any(|a| line.contains(a)) {
                    offenders.push(format!("{owner}:{}", i + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "use crate::saves::write_atomic: {offenders:?}");
    }
}
