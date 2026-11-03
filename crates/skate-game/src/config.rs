use bevy::prelude::Resource;
use std::path::PathBuf;

#[derive(Resource)]
pub(crate) struct Config {
    pub asset_root: PathBuf,
    pub verification_capture: Option<PathBuf>,
    pub map: Option<skate_data::skate_map::SkateMap>,
    pub map_path: Option<PathBuf>,
    pub difficulty: crate::difficulty::Difficulty,
    pub check_assets: bool,
    pub start_paused: bool,
    pub multiplayer: crate::multiplayer::Options,
    pub map_fingerprint: u64,
    pub teleport: Option<String>,
    /// `--hero-clip OUT.webm`: render the launcher's skater clip in the test world, hidden, then exit (hero_clip.rs).
    pub hero_clip: Option<PathBuf>,
    /// `--skate2-physics DIR`: the Skate 2 data for the Skate 2 physics switch (the `skate2-physics` stage's output),
    /// made absolute; else `<assets>/private/skate2-physics` (`Skate2Physics::data_dir`). Not tested in play.
    pub skate2_physics_dir: Option<PathBuf>,
    /// `--physics skate3|skate2`: this run's physics, not saved (like `--difficulty`); `None` uses the saved setting.
    #[allow(dead_code, reason = "Task 19's switch reads it (the wanted physics)")]
    pub physics_override: Option<crate::skate2_physics::settings::Physics>,
}

/// `--skate2-physics DIR` and `--physics skate3|skate2`, as `Config::from_env` reads them (Task 14).
#[derive(Default)]
pub(crate) struct PhysicsArgs {
    pub skate2_physics_dir: Option<PathBuf>,
    pub physics_override: Option<crate::skate2_physics::settings::Physics>,
}

impl PhysicsArgs {
    /// Takes `flag`, one of the two, and its value from `args`.
    fn take(&mut self, flag: &str, args: &mut impl Iterator<Item = std::ffi::OsString>) -> Result<(), String> {
        use crate::skate2_physics::settings::Physics;
        match flag {
            "--skate2-physics" => {
                self.skate2_physics_dir = Some(args.next().ok_or("--skate2-physics requires a directory")?.into());
            }
            "--physics" => {
                let value = args.next().ok_or("--physics requires skate3 or skate2")?;
                self.physics_override = Some(match value.to_str() {
                    Some("skate3") => Physics::Skate3,
                    Some("skate2") => Physics::Skate2,
                    _ => return Err(format!("--physics requires skate3 or skate2, not {value:?}")),
                });
            }
            _ => return Err(format!("{flag} is not a physics option")),
        }
        Ok(())
    }
}

impl Config {
    /// The configuration, and the saved default map when it loads behind the boot screens instead (G1,
    /// map_transition::StartupMap). Runs that skip the boot, and perf reports, still load it here.
    pub fn from_env() -> Result<(Self, Option<PathBuf>), String> {
        let mut config = Self {
            asset_root: crate::setup::asset_root()?,
            verification_capture: None,
            map: None,
            map_path: None,
            difficulty: crate::difficulty::Difficulty::default(),
            check_assets: false,
            start_paused: false,
            multiplayer: crate::multiplayer::Options::default(),
            map_fingerprint: 0,
            teleport: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
        };
        let mut difficulty_override = None;
        let mut physics = PhysicsArgs::default();
        let mut explicit_map = false;
        let mut safe = false; // F4 --safe: Not retail (user 10-13)
        let mut ip_sharing = false;
        // Hash of the map file read with it (load_map).
        let mut fingerprint = None;
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--trace" | "--trace-seconds" | "--trace-delay" | "--trace-min-us") => {
                    args.next().ok_or("Trace option requires a value")?;
                }
                Some("--trace-wait" | "--trace-gpu") => {}
                Some("--net-host") => {
                    config.multiplayer.host = Some(
                        args.next()
                            .ok_or("Missing host bind address")?
                            .to_string_lossy()
                            .parse()
                            .map_err(|_| "Invalid host bind address")?,
                    )
                }
                Some("--net-local") => {
                    let bind = args
                        .next()
                        .ok_or("--net-local requires bind and peer addresses")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "Invalid bind address")?;
                    let peer = args
                        .next()
                        .ok_or("--net-local requires peer address")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "Invalid peer address")?;
                    config.multiplayer.direct = Some((bind, peer));
                }
                // Q1: a non-loopback direct session shows this player's IP address to everyone in it.
                Some("--net-allow-ip-sharing") => ip_sharing = true,
                Some("--net-session") => {
                    config.multiplayer.session = args
                        .next()
                        .ok_or("Missing session")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "Invalid session")?
                }
                Some("--spawn-offset") => {
                    let offset: f32 = args
                        .next()
                        .ok_or("Missing spawn offset")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "Invalid spawn offset")?;
                    if !offset.is_finite() || offset.abs() > 20. {
                        return Err("Spawn offset must be within 20 metres".into());
                    }
                    config.multiplayer.spawn_offset = offset;
                }
                Some("--player-title") => {
                    config.multiplayer.title =
                        Some(args.next().ok_or("Missing title")?.to_string_lossy().into())
                }
                Some("--appearance") => {
                    let value = args
                        .next()
                        .ok_or("Missing appearance")?
                        .to_string_lossy()
                        .into_owned();
                    if value.len() > 128 {
                        return Err("Appearance ID too long".into());
                    }
                    config.multiplayer.appearance = Some(value);
                }
                Some("--controller") => {
                    let slot: u32 = args
                        .next()
                        .ok_or("Missing controller index")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "Invalid controller index")?;
                    if slot > 3 {
                        return Err("Controller index must be 0 to 3".into());
                    }
                    config.multiplayer.controller = Some(slot);
                }
                Some("--assets") => {
                    config.asset_root = args.next().ok_or("--assets requires a directory")?.into()
                }
                Some("--map") => {
                    let path = PathBuf::from(args.next().ok_or("--map requires a .skate file")?);
                    let (map, hash) = load_map(&path)?;
                    config.map = Some(map);
                    fingerprint = Some(hash);
                    config.map_path = Some(path.canonicalize().map_err(|e| e.to_string())?);
                    explicit_map = true;
                }
                Some("--test-world") => {
                    explicit_map = true;
                    config.map = None;
                    config.map_path = None;
                }
                Some("--check-assets") => config.check_assets = true,
                Some("--safe") => safe = true, // Not retail (user 10-13)
                Some("--start-paused") => config.start_paused = true,
                Some("--teleport") => {
                    config.teleport = Some(
                        args.next()
                            .ok_or("--teleport requires a destination ID")?
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
                Some("--difficulty") => {
                    let value = args
                        .next()
                        .ok_or("--difficulty requires easy, normal or hardcore")?;
                    difficulty_override = Some(crate::difficulty::Difficulty::parse(
                        &value.to_string_lossy(),
                    )?);
                }
                Some(flag @ ("--skate2-physics" | "--physics")) => physics.take(flag, &mut args)?,
                Some("--verify") => {
                    config.verification_capture = Some(
                        args.next()
                            .ok_or("--verify requires an output PNG path")?
                            .into(),
                    )
                }
                Some("--hero-clip") => {
                    let out = args.next().ok_or("--hero-clip requires an output .webm path")?;
                    config.hero_clip = Some(hero_clip_path(out.into())?);
                    // the procedural test world, never the player's default map
                    explicit_map = true;
                    config.map = None;
                    config.map_path = None;
                }
                _ => {
                    return Err(format!(
                        "Unknown argument {arg:?}. Usage: skate3rust [--assets DIRECTORY] [--map MAP.skate | --test-world] [--difficulty easy|normal|hardcore] [--physics skate3|skate2] [--skate2-physics DIRECTORY] [--verify CAPTURE.png] [--check-assets] [--start-paused]"
                    ));
                }
            }
        }
        config.asset_root = config
            .asset_root
            .canonicalize()
            .map_err(|e| format!("Asset root {}: {e}", config.asset_root.display()))?;
        // Not retail (user 10-13). The build's own asset check (install.py --check-assets) leaves saves alone.
        if !config.check_assets {
            // F2: the saves out of the installation a build deletes, before anything reads them (saves.rs)
            if let Err(e) = crate::saves::migrate(&config.asset_root) {
                eprintln!("Saves stay in the installation this run: {e}");
            }
            // F4: safe mode starts with the default graphics; the player's are kept as graphics.json.bak
            if safe {
                reset_graphics(&crate::saves::settings_dir(&config.asset_root));
            }
            // C7: online looks a crashed session left in %TEMP%
            std::thread::spawn(|| sweep_old_online_looks(&std::env::temp_dir(), std::time::Duration::from_secs(24 * 3600)));
        }
        // Not saved, like --difficulty: the switch (Task 19) prefers it to the saved setting.
        config.physics_override = physics.physics_override;
        config.skate2_physics_dir = match physics.skate2_physics_dir {
            Some(dir) => Some(std::path::absolute(&dir).map_err(|e| format!("--skate2-physics {}: {e}", dir.display()))?),
            None => None,
        };
        config.difficulty = match difficulty_override {
            Some(mode) => mode,
            None => crate::difficulty::Difficulty::load(&config.asset_root),
        };
        let mut deferred = None;
        if !explicit_map {
            // a default map an update removed (install.py no longer fixes data/settings) boots the test world: Not retail (user 10-13)
            // A damaged or unsafe default-map.json starts the test world rather than stopping the game (release review 10-18)
            let default = crate::map_library::default_map(&config.asset_root).unwrap_or_else(|e| {
                eprintln!("Default map: {e}; starting in the test world");
                None
            });
            if let Some(path) = default.filter(|p| p.is_file()) {
                // G1: the boot (ESRB, EA logo, its loading tag) plays while the map loads; boot::manifests is empty when
                // it won't run (a test run's arguments).
                let boot = crate::boot::manifests(&config.asset_root).first().is_some_and(|p| p.is_file());
                if boot && !config.check_assets && std::env::var_os("SKATE_PERF_REPORT").is_none() {
                    deferred = Some(path.canonicalize().map_err(|e| e.to_string())?);
                } else {
                    // A default map that won't load starts the test world (release review 10-18); --check-assets
                    // still fails on it.
                    match load_map(&path) {
                        Ok((map, hash)) => {
                            config.map = Some(map);
                            fingerprint = Some(hash);
                            config.map_path = Some(path.canonicalize().map_err(|e| e.to_string())?);
                        }
                        Err(e) if config.check_assets => return Err(e),
                        Err(e) => eprintln!("Default map {}: {e}; starting in the test world", path.display()),
                    }
                }
            }
        }
        config.map_fingerprint = match fingerprint.filter(|_| config.map_path.is_some()) {
            Some(hash) => hash,
            None => map_fingerprint(config.map_path.as_deref())?,
        };
        if config.multiplayer.direct.is_some() && config.multiplayer.session == 0 {
            return Err("Direct multiplayer requires --net-session (a nonzero number shared by both players)".into());
        }
        check_direct(&config.multiplayer, ip_sharing, !cfg!(debug_assertions))?;
        if let Some(id) = &config.teleport {
            let destinations = crate::teleport_menu::load(&config.asset_root)?;
            let target = destinations
                .iter()
                .find(|d| &d.id == id)
                .ok_or("Unknown teleport destination")?;
            if target.matrix.is_none()
                || !config
                    .map_path
                    .as_ref()
                    .is_some_and(|p| crate::teleport_menu::same_map(p, &target.map))
            {
                return Err(
                    "Teleport destination is unavailable or belongs to a different map".into(),
                );
            }
        }
        if let Some(path) = &mut config.verification_capture {
            if path.extension().and_then(|x| x.to_str()) != Some("png") {
                return Err("--verify output must be a PNG file".into());
            }
            if !path.is_absolute() {
                *path = std::env::current_dir()
                    .map_err(|e| e.to_string())?
                    .join(&path);
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        if let Some(parent) = config.hero_clip.as_ref().and_then(|p| p.parent()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        Ok((config, deferred))
    }
}

/// perf 10-18: an optimisation's switch, on unless `name=0` (benchmarks, bisecting); read once per name.
pub(crate) fn perf_on(name: &'static str) -> bool {
    static SEEN: std::sync::Mutex<Vec<(&str, bool)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&(_, on)) = seen.iter().find(|(n, _)| *n == name) {
        return on;
    }
    let on = std::env::var_os(name).is_none_or(|v| v != "0");
    seen.push((name, on));
    on
}

/// Shows `path` in the system's file browser (the mods menu's Open Folder, the Music Player's Y).
pub(crate) fn open_folder(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program).arg(path).spawn().map(|_| ())
}

/// The `--hero-clip` output: a .webm path, made absolute against the working directory.
pub(crate) fn hero_clip_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.extension().and_then(|x| x.to_str()) != Some("webm") {
        return Err("--hero-clip output must be a .webm file".into());
    }
    Ok(if path.is_absolute() { path } else { std::env::current_dir().map_err(|e| e.to_string())?.join(path) })
}

/// Hash map bytes identically for startup and background map replacement.
/// Read and decode a map while its identity hash reads the file on another
/// thread; hashing after the decode added ~0.4 s to a large map's load.
pub(crate) fn load_map(
    path: &std::path::Path,
) -> Result<(skate_data::skate_map::SkateMap, u64), String> {
    std::thread::scope(|scope| {
        let fingerprint = std::thread::Builder::new()
            .name("map-fingerprint".into())
            .spawn_scoped(scope, || map_fingerprint(Some(path)))
            .map_err(|e| format!("Map fingerprint: {e}"))?;
        let map = skate_data::skate_map::SkateMap::load(path);
        let fingerprint = fingerprint
            .join()
            .map_err(|_| "Map fingerprint failed unexpectedly".to_string())?;
        Ok((map?, fingerprint?))
    })
}

pub(crate) fn map_fingerprint(path: Option<&std::path::Path>) -> Result<u64, String> {
    Ok(if let Some(path) = path {
        use std::io::Read;
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut hash = 0xcbf29ce484222325u64;
        let mut buffer = [0; 65536];
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            for b in &buffer[..n] {
                hash = (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
            }
        }
        hash
    } else {
        skate_net::hash(b"skate-test-world-v1")
    })
}

/// F4: graphics.json (where a setting that crashes the game at start lives) moved aside to graphics.json.bak.
/// A .bak from an earlier --safe start is the player's own settings and is kept. Not retail (user 10-13).
fn reset_graphics(settings: &std::path::Path) -> bool {
    let (file, bak) = (settings.join("graphics.json"), settings.join("graphics.json.bak"));
    // and its last good copy, or a damaged default would bring the crashing settings back (saves::load)
    let _ = std::fs::remove_file(crate::saves::backup(&file));
    file.is_file() && if bak.exists() { std::fs::remove_file(&file).is_ok() } else { std::fs::rename(&file, bak).is_ok() }
}

/// C7: `skate-online-*` folders older than `age` in `temp`. Each session makes a new one and removes it only on a clean
/// exit (multiplayer/appearance.rs), so one that old is a crashed session's. Not retail (user 10-13).
fn sweep_old_online_looks(temp: &std::path::Path, age: std::time::Duration) -> usize {
    let old = |e: &std::fs::DirEntry| e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > age);
    std::fs::read_dir(temp).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("skate-online-") && e.path().is_dir() && old(e))
        .filter(|e| std::fs::remove_dir_all(e.path()).is_ok())
        .count()
}

/// Online safety Q1: a direct session off this machine shares IP addresses with strangers, so a release build only
/// opens one with --net-allow-ip-sharing, and its number must be long enough that it is not guessed (it is the
/// only thing a stranger must know beyond the port).
fn check_direct(m: &crate::multiplayer::Options, ip_sharing: bool, release: bool) -> Result<(), String> {
    let mut addresses = m.host.into_iter().chain(m.direct.into_iter().flat_map(|(b, p)| [b, p]));
    if addresses.all(|a| a.ip().is_loopback()) {
        return Ok(());
    }
    if release && !ip_sharing {
        return Err("Direct multiplayer off this computer shows your IP address to every player; add --net-allow-ip-sharing to accept that".into());
    }
    if m.session < 1_000_000_000 {
        return Err("Direct multiplayer off this computer requires a --net-session of at least 10 digits, so strangers cannot guess it".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn direct_sessions_off_this_computer_need_consent_and_a_long_number() {
        let m = |host: &str, session| crate::multiplayer::Options { host: Some(host.parse().unwrap()), session, ..Default::default() };
        let check = super::check_direct;
        assert!(check(&m("127.0.0.1:31030", 1), false, true).is_ok(), "loopback testing is unchanged");
        assert!(check(&m("0.0.0.0:31030", 48031030123), false, true).is_err(), "release needs the flag");
        assert!(check(&m("0.0.0.0:31030", 48031030123), true, true).is_ok());
        assert!(check(&m("192.168.1.2:31030", 48031030), true, true).is_err(), "a short number is guessable");
        assert!(check(&m("192.168.1.2:31030", 48031030), true, false).is_err(), "debug builds too");
        let joiner = crate::multiplayer::Options {
            direct: Some(("127.0.0.1:0".parse().unwrap(), "203.0.113.9:31030".parse().unwrap())),
            session: 48031030123,
            ..Default::default()
        };
        assert!(check(&joiner, false, true).is_err(), "joining a remote host shares our address too");
        assert!(check(&joiner, false, false).is_ok());
    }
    #[test]
    fn config_takes_the_skate2_physics_folder_and_override() {
        use crate::skate2_physics::settings::Physics;
        // The tokens through the same branch `from_env` takes for the two flags.
        let parse = |tokens: &[&str]| -> Result<super::PhysicsArgs, String> {
            let mut parsed = super::PhysicsArgs::default();
            let mut args = tokens.iter().map(std::ffi::OsString::from);
            while let Some(flag) = args.next() {
                parsed.take(flag.to_str().unwrap(), &mut args)?;
            }
            Ok(parsed)
        };
        let parsed = parse(&["--skate2-physics", "D:/s2", "--physics", "skate2"]).unwrap();
        assert_eq!(parsed.skate2_physics_dir, Some("D:/s2".into()));
        assert_eq!(parsed.physics_override, Some(Physics::Skate2));
        assert_eq!(parse(&["--physics", "skate3"]).unwrap().physics_override, Some(Physics::Skate3));
        assert_eq!(parse(&[]).unwrap().physics_override, None, "no flag: the saved setting");
        assert!(parse(&["--physics", "fast"]).is_err());
        assert!(parse(&["--physics"]).is_err());
        assert!(parse(&["--skate2-physics"]).is_err());
    }

    #[test]
    fn hero_clip_takes_a_webm_path_and_makes_it_absolute() {
        let path = super::hero_clip_path("out/hero.webm".into()).unwrap();
        assert!(path.is_absolute() && path.ends_with("out/hero.webm"));
        assert!(super::hero_clip_path("hero.mp4".into()).is_err());
        assert!(super::hero_clip_path("hero".into()).is_err());
    }

    #[test]
    fn safe_mode_and_crashed_online_looks() {
        let t = std::env::temp_dir().join(format!("s3r-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("skate-online-00ff/looks")).unwrap();
        std::fs::create_dir_all(t.join("skate-online-live")).unwrap();
        std::fs::create_dir_all(t.join("other")).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600);
        for d in ["skate-online-00ff", "other"] {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::File::options().write(true).custom_flags(0x0200_0000).open(t.join(d)).unwrap().set_modified(old).unwrap();
        }
        assert_eq!(super::sweep_old_online_looks(&t, std::time::Duration::from_secs(24 * 3600)), 1);
        assert!(!t.join("skate-online-00ff").exists() && t.join("skate-online-live").exists() && t.join("other").exists());
        assert!(!super::reset_graphics(&t));
        std::fs::write(t.join("graphics.json"), b"{}").unwrap();
        assert!(super::reset_graphics(&t) && t.join("graphics.json.bak").is_file() && !t.join("graphics.json").exists());
        // a second --safe start keeps the player's own settings, not the defaults written since
        std::fs::write(t.join("graphics.json"), b"defaults").unwrap();
        assert!(super::reset_graphics(&t));
        assert_eq!(std::fs::read(t.join("graphics.json.bak")).unwrap(), b"{}");
        assert!(!t.join("graphics.json").exists());
        std::fs::remove_dir_all(t).unwrap();
    }
}
