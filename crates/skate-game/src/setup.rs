//! Each portable copy owns its installation; explicit --assets is for development.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn installed(base: &Path) -> Result<Option<(PathBuf, serde_json::Value)>, String> {
    let bytes = match std::fs::read(base.join("installation.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    // A marker cut short by a power loss: the pipeline's atomic_json keeps the previous one beside it.
    let marker: serde_json::Value = match serde_json::from_slice(&bytes).or_else(|e| {
        std::fs::read(base.join("installation.json.bak"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(e)
    }) {
        Ok(marker) => marker,
        // Release review 10-18 (no crashes): both damaged is no installation, so setup (or the launcher) builds it
        // again, rather than the game refusing to start until the file is deleted by hand.
        Err(e) => {
            eprintln!("installation.json and its .bak are damaged ({e}); treating this copy as not set up");
            return Ok(None);
        }
    };
    if marker["version"].as_u64() != Some(1) {
        return Err("Unsupported installation version".into());
    }
    let relative = Path::new(
        marker["directory"]
            .as_str()
            .ok_or("Invalid installation path")?,
    );
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 2
        || parts[0].as_os_str() != "installations"
        || !parts[1].as_os_str().to_str().is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err("Invalid installation path".into());
    }
    let assets = base.join(relative).join("assets");
    if !assets.is_dir() {
        return Ok(None);
    }
    if !assets
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(base.canonicalize().map_err(|e| e.to_string())?)
    {
        return Err("Installation escapes its package data directory".into());
    }
    Ok(Some((assets, marker)))
}

pub(crate) fn asset_root() -> Result<PathBuf, String> {
    if std::env::args_os().any(|arg| arg == "--assets") {
        return Ok(PathBuf::from("assets"));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = exe.parent().ok_or("No executable directory")?;
    let base = root.join("data");
    let mut expected_customiser = None;
    let expected = match std::fs::read(root.join("release.json")) {
        Ok(bytes) => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|e| e.to_string())?
                .trim_start_matches('\u{feff}');
            let release: serde_json::Value =
                serde_json::from_str(text).map_err(|e| e.to_string())?;
            expected_customiser = release["character_customiser"].as_str().map(str::to_owned);
            release.get("asset_pipelines").cloned()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let existing = installed(&base)?;
    if let Some((assets, marker)) = &existing {
        if expected
            .as_ref()
            .is_none_or(|versions| pipelines_current(marker, versions))
            && assets.join("private/game.json").is_file()
            && marker.get("outputs").is_none_or(|groups| {
                groups.as_object().is_some_and(|groups| {
                    groups
                        .values()
                        .all(|files| receipt_present(assets.parent().unwrap(), files))
                })
            })
            && customiser_current(assets, expected_customiser.as_deref())
        {
            return Ok(assets.clone());
        }
    }
    if let Some(message) = launcher_message(root) {
        return Err(message);
    }
    let setup = root.join("support/skate3setup.exe");
    if !setup.is_file() {
        return Err("This copy has not been set up. Use the complete Windows package, or --assets DIRECTORY for development.".into());
    }
    let mut command = Command::new(setup);
    command.arg("--base").arg(&base).arg("--game-exe").arg(&exe);
    if existing.is_some() {
        command.arg("--refresh");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let status = command
        .status()
        .map_err(|e| format!("Could not start setup: {e}"))?;
    if !status.success() {
        return Err("Setup was cancelled or did not complete".into());
    }
    let (assets, marker) =
        installed(&base)?.ok_or("Setup did not publish a complete installation")?;
    if expected
        .as_ref()
        .is_some_and(|versions| !pipelines_current(&marker, versions))
    {
        return Err("Setup helper does not match this release's asset extractors. Unpack the complete package.".into());
    }
    if !customiser_current(&assets, expected_customiser.as_deref()) {
        return Err("Character customiser preparation did not complete for this release.".into());
    }
    Ok(assets)
}

/// Every pipeline fingerprint release.json lists must be the one the assets were built with. The marker may hold more
/// keys (`skate3-base`, stages a later release dropped); an empty or missing release list matches nothing.
fn pipelines_current(marker: &serde_json::Value, expected: &serde_json::Value) -> bool {
    expected
        .as_object()
        .is_some_and(|want| !want.is_empty() && want.iter().all(|(k, v)| marker["pipelines"].get(k) == Some(v)))
}

/// In the launcher's layout (`<folder>\Skate 3 Remaster.exe` + `<folder>\game\`), the launcher owns builds.
fn launcher_message(game_dir: &Path) -> Option<String> {
    game_dir
        .parent()
        .filter(|p| p.join("Skate 3 Remaster.exe").is_file())
        .map(|_| "The game files need building. Open S3R and press Build game.".to_string())
}

fn customiser_current(assets: &Path, expected: Option<&str>) -> bool {
    expected.is_none_or(|expected| {
        let degraded =
            std::fs::read(assets.join("private/customisation/customiser-availability.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if let Some(v) = degraded.filter(|v| {
            v["version"].as_u64() == Some(1) && v["fingerprint"].as_str() == Some(expected)
        }) {
            if v["status"] == "unavailable" {
                return true;
            }
            if v["status"] == "retained" {
                let directory = crate::customiser_parts::asset_directory(assets);
                return ["catalog", "library", "menu", "lighting", "roster"]
                    .iter()
                    .all(|stage| {
                        std::fs::read(directory.join(format!("{stage}-complete.json")))
                            .ok()
                            .and_then(|bytes| {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            })
                            .is_some_and(|v| receipt_present(&directory, &v["files"]))
                    });
            }
        }
        std::fs::read(assets.join("private/customisation/current.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| {
                v["version"].as_u64() == Some(1)
                    && v["fingerprint"].as_str() == Some(expected)
                    && v["set"]
                        .as_str()
                        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            })
            && [
                "library-v3.json",
                "extra-menu.json",
                "native-lighting.json",
                "native-roster/complete.json",
            ]
            .iter()
            .all(|name| {
                crate::customiser_parts::asset_directory(assets)
                    .join(name)
                    .is_file()
            })
            && ["catalog", "library", "menu", "lighting", "roster"]
                .iter()
                .all(|stage| {
                    let directory = crate::customiser_parts::asset_directory(assets);
                    std::fs::read(directory.join(format!("{stage}-complete.json")))
                        .ok()
                        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                        .is_some_and(|v| receipt_present(&directory, &v["files"]))
                })
    })
}

// Cheap launch-time completeness check. Setup verifies SHA-256 before reuse;
// hashing every map on every game launch would read gigabytes unnecessarily.
fn receipt_present(root: &Path, files: &serde_json::Value) -> bool {
    // perf 11-02: one listing per folder (on Windows its entries carry their sizes) instead of opening each of the
    // ~75,600 files: the open-per-file check took 7-9 s of every start on the owner's PC, before the window showed.
    // The ~9,200 folders are listed on several threads (one after another they still took ~2 s).
    let Some(files) = files.as_object().filter(|files| !files.is_empty()) else { return false };
    let safe = |name: &str| {
        let relative = Path::new(name);
        !relative.is_absolute()
            && relative.file_name().is_some()
            && relative.components().all(|c| matches!(c, std::path::Component::Normal(_)))
    };
    if !files.keys().all(|name| safe(name)) {
        return false;
    }
    let folders: Vec<&Path> = files
        .keys()
        .filter_map(|name| Path::new(name.as_str()).parent())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let listed = list_folders(root, &folders);
    files.iter().all(|(name, entry)| {
        let relative = Path::new(name.as_str());
        let (Some(folder), Some(file)) = (relative.parent(), relative.file_name()) else { return false };
        listed
            .get(folder)
            .and_then(Option::as_ref)
            .and_then(|listed| listed.get(&listing_key(file)))
            .is_some_and(|&(is_file, len)| is_file && Some(len) == entry["size"].as_u64())
    })
}

/// [`listing`] of each of `folders` under `root`, several at once.
fn list_folders<'a>(root: &Path, folders: &[&'a Path]) -> std::collections::HashMap<&'a Path, Option<Listing>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8).min(folders.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&folder) = folders.get(index) else { break done };
                        done.push((folder, listing(&root.join(folder))));
                    }
                })
            })
            .collect();
        // A worker that died leaves its folders out: their files then count as missing.
        workers.into_iter().flat_map(|worker| worker.join().unwrap_or_default()).collect()
    })
}

/// A folder's entries by [`listing_key`]: whether each is a file (links followed) and its length.
type Listing = std::collections::HashMap<String, (bool, u64)>;

fn listing(folder: &Path) -> Option<Listing> {
    let mut listed = Listing::new();
    for entry in std::fs::read_dir(folder).ok()?.flatten() {
        // A link's own entry says nothing about its target: that one is looked up as the old check did.
        let metadata = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => std::fs::metadata(entry.path()),
            _ => entry.metadata(),
        };
        if let Ok(m) = metadata {
            listed.insert(listing_key(&entry.file_name()), (m.is_file(), m.len()));
        }
    }
    Some(listed)
}

/// Windows names match whatever their case, as the per-file open did.
fn listing_key(name: &std::ffi::OsStr) -> String {
    let name = name.to_string_lossy();
    if cfg!(windows) { name.to_lowercase() } else { name.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_marker_cut_short_falls_back_to_its_backup() {
        let base = std::env::temp_dir().join(format!("sk8-marker-bak-{}", std::process::id()));
        let id = "0123456789abcdef0123456789abcdef";
        std::fs::create_dir_all(base.join("installations").join(id).join("assets")).unwrap();
        let marker = serde_json::json!({"version": 1, "directory": format!("installations/{id}")});
        std::fs::write(base.join("installation.json"), b"{\"version\": 1, \"dir").unwrap();
        assert!(installed(&base).unwrap().is_none(), "no backup: a torn marker is no installation (set up again), not an error");
        std::fs::write(base.join("installation.json.bak"), marker.to_string()).unwrap();
        let (assets, read) = installed(&base).unwrap().unwrap();
        assert!(assets.ends_with("assets") && read == marker);
        let _ = std::fs::remove_dir_all(base);
    }
    #[test]
    fn acknowledged_optional_failure_is_versioned_and_retained_data_is_checked() {
        let root = std::env::temp_dir().join(format!(
            "sk8-availability-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let base = root.join("private/customisation");
        std::fs::create_dir_all(&base).unwrap();
        let availability = base.join("customiser-availability.json");
        std::fs::write(
            &availability,
            br#"{"version":1,"fingerprint":"new","status":"unavailable"}"#,
        )
        .unwrap();
        assert!(customiser_current(&root, Some("new")));
        assert!(!customiser_current(&root, Some("future")));
        assert_eq!(
            crate::customiser_parts::asset_directory(&root),
            base.join("unavailable")
        );
        std::fs::write(
            &availability,
            br#"{"version":1,"fingerprint":"new","status":"retained"}"#,
        )
        .unwrap();
        assert!(!customiser_current(&root, Some("new")));
        std::fs::write(
            base.join("current.json"),
            br#"{"set":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        let directory = base.join("sets/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("payload"), b"ok").unwrap();
        for stage in ["catalog", "library", "menu", "lighting", "roster"] {
            std::fs::write(
                directory.join(format!("{stage}-complete.json")),
                br#"{"files":{"payload":{"size":2}}}"#,
            )
            .unwrap();
        }
        assert!(customiser_current(&root, Some("new")));
        std::fs::remove_file(directory.join("payload")).unwrap();
        assert!(!customiser_current(&root, Some("new")));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn launcher_layout_asks_for_the_launcher_instead_of_running_setup() {
        let temp = std::env::temp_dir().join(format!("s3r-launcher-{}", std::process::id()));
        std::fs::create_dir_all(temp.join("game/support")).unwrap();
        std::fs::write(temp.join("Skate 3 Remaster.exe"), b"x").unwrap();
        assert_eq!(launcher_message(&temp.join("game")).as_deref(),
                   Some("The game files need building. Open S3R and press Build game."));
        std::fs::remove_file(temp.join("Skate 3 Remaster.exe")).unwrap();
        assert!(launcher_message(&temp.join("game")).is_none());
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn release_pipelines_are_a_subset_check_of_the_marker() {
        let release = serde_json::json!({"core": "c1", "hud": "h1", "mod-maps": "m1"});
        let marker = |pipelines: serde_json::Value| serde_json::json!({"version": 1, "pipelines": pipelines});
        // extra keys (skate3-base, a stage a later release dropped) are fine
        assert!(pipelines_current(
            &marker(serde_json::json!({"core": "c1", "hud": "h1", "mod-maps": "m1", "skate3-base": "b", "old-stage": "x"})),
            &release
        ));
        // a different value, or a listed key that is missing, is not
        assert!(!pipelines_current(&marker(serde_json::json!({"core": "c1", "hud": "h2", "mod-maps": "m1"})), &release));
        assert!(!pipelines_current(&marker(serde_json::json!({"core": "c1", "hud": "h1"})), &release));
        assert!(!pipelines_current(&serde_json::json!({"version": 1}), &release));
        // an empty (or non-object) release list matches nothing
        assert!(!pipelines_current(&marker(serde_json::json!({"core": "c1"})), &serde_json::json!({})));
        assert!(!pipelines_current(&marker(serde_json::json!({"core": "c1"})), &serde_json::json!(null)));
    }
    #[test]
    fn receipts_reject_missing_files_empty_lists_and_traversal() {
        let root = std::env::temp_dir();
        assert!(!receipt_present(&root, &serde_json::json!({})));
        assert!(!receipt_present(
            &root,
            &serde_json::json!({"../missing": {"size": 0}})
        ));
        assert!(!receipt_present(
            &root,
            &serde_json::json!({"nonexistent-skate-setup-test": {"size": 0}})
        ));
    }
    /// perf 11-02: the folder-listing check passes and fails exactly where the per-file one did.
    #[test]
    fn receipts_check_every_file_by_its_folder_listing() {
        let root = std::env::temp_dir().join(format!("sk8-receipts-{}", std::process::id()));
        std::fs::create_dir_all(root.join("maps/sub")).unwrap();
        std::fs::write(root.join("top.bin"), b"abc").unwrap();
        std::fs::write(root.join("maps/a.skate"), b"12345").unwrap();
        std::fs::write(root.join("maps/sub/B.json"), b"{}").unwrap();
        let ok = serde_json::json!({"top.bin": {"size": 3}, "maps/a.skate": {"size": 5}, "maps/sub/B.json": {"size": 2}});
        assert!(receipt_present(&root, &ok));
        assert!(!receipt_present(&root, &serde_json::json!({"maps/a.skate": {"size": 4}})), "a wrong size");
        assert!(!receipt_present(&root, &serde_json::json!({"maps/gone.skate": {"size": 5}})), "a missing file");
        assert!(!receipt_present(&root, &serde_json::json!({"nowhere/a.skate": {"size": 5}})), "a missing folder");
        assert!(!receipt_present(&root, &serde_json::json!({"maps/sub": {"size": 0}})), "a folder is not a file");
        assert!(!receipt_present(&root, &serde_json::json!({"maps/../top.bin": {"size": 3}})), "traversal");
        assert!(!receipt_present(&root, &serde_json::json!({"top.bin": {"size": 3}, "maps/a.skate": {}})), "no size");
        // Windows matches names whatever their case, as opening each file did.
        assert_eq!(receipt_present(&root, &serde_json::json!({"MAPS/sub/b.JSON": {"size": 2}})), cfg!(windows));
        let _ = std::fs::remove_dir_all(&root);
    }
}
