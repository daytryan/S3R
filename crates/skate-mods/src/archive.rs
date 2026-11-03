//! Bounded ZIP extraction into `.cache/<content hash>`. Identical archives share one
//! extraction across launches and instances; a folder is only ever published complete.
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::{Path, PathBuf},
};
const LIMIT: u64 = 64 * 1024 * 1024;

fn safe_name(name: &str) -> Result<PathBuf, String> {
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty()
        || name.len() > 240
        || name.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
    {
        return Err("Invalid ZIP path".into());
    }
    for part in name.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_lowercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || matches!(
                stem.as_str(),
                "con"
                    | "prn"
                    | "aux"
                    | "nul"
                    | "com1"
                    | "com2"
                    | "com3"
                    | "com4"
                    | "com5"
                    | "com6"
                    | "com7"
                    | "com8"
                    | "com9"
                    | "lpt1"
                    | "lpt2"
                    | "lpt3"
                    | "lpt4"
                    | "lpt5"
                    | "lpt6"
                    | "lpt7"
                    | "lpt8"
                    | "lpt9"
            )
        {
            return Err(
                "ZIP paths must be ordinary relative paths without traversal or device names"
                    .into(),
            );
        }
    }
    Ok(PathBuf::from(name))
}

/// The folder holding `archive`'s validated contents under `root/.cache`.
pub(crate) fn materialize(root: &Path, archive: &Path) -> Result<PathBuf, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(archive)
        .map_err(|e| e.to_string())?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("ZIP exceeds 64 MiB compressed".into());
    }
    let hash = blake3::hash(&bytes);
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let cache = root.join(".cache");
    if cache.exists()
        && std::fs::symlink_metadata(&cache)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("ZIP cache must not be a link".into());
    }
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    if !cache
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&root)
    {
        return Err("ZIP cache escapes mods folder".into());
    }
    sweep_sessions(&cache);
    // These exact bytes were validated when this folder was published.
    let destination = cache.join(&hash.to_hex()[..32]);
    if std::fs::symlink_metadata(&destination).is_ok_and(|m| m.is_dir()) {
        return Ok(destination);
    }
    // Validate and decompress into bounded memory before publishing anything.
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).map_err(|e| e.to_string())?;
    if zip.len() > 512 {
        return Err("ZIP exceeds 512 entries".into());
    }
    let mut names = BTreeSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let path = safe_name(file.name())?;
        if !names.insert(path.to_string_lossy().to_lowercase()) {
            return Err("Duplicate/case-colliding ZIP path".into());
        }
        let kind = file.unix_mode().unwrap_or(0) & 0o170000;
        if kind != 0 && kind != 0o100000 && kind != 0o040000 {
            return Err("ZIP links and special files are unsupported".into());
        }
        if file.is_dir() {
            continue;
        }
        if file.size() > LIMIT - total {
            return Err("ZIP exceeds 64 MiB expanded".into());
        }
        let mut contents = Vec::new();
        file.by_ref()
            .take(LIMIT - total + 1)
            .read_to_end(&mut contents)
            .map_err(|e| e.to_string())?;
        total += contents.len() as u64;
        if total > LIMIT {
            return Err("ZIP exceeds 64 MiB expanded".into());
        }
        files.push((path, contents));
    }
    if !files.iter().any(|(p, _)| p == Path::new("mod.json")) {
        return Err("ZIP must contain mod.json at its root, not inside a wrapper folder".into());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let staging = cache.join(format!(".tmp-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&staging).map_err(|e| e.to_string())?;
    let written = files.into_iter().try_for_each(|(path, contents)| {
        let target = staging.join(path);
        std::fs::create_dir_all(target.parent().unwrap())?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?
            .write_all(&contents)
    });
    // Publish whole folders only. Another instance may have published the same one first.
    if let Err(e) = written.and_then(|()| std::fs::rename(&staging, &destination)) {
        let _ = std::fs::remove_dir_all(&staging);
        if !std::fs::symlink_metadata(&destination).is_ok_and(|m| m.is_dir()) {
            return Err(e.to_string());
        }
    }
    Ok(destination)
}

/// Remove the old per-launch cache folders (`<pid>-<nanos>`). Their cleanup only ran
/// when the cache was dropped, which a normal game exit skips: ~8 MB of stock mods per launch.
fn sweep_sessions(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let legacy = entry
            .file_name()
            .to_str()
            .and_then(|n| n.split_once('-'))
            .is_some_and(|(pid, nanos)| {
                [pid, nanos]
                    .iter()
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            });
        // file_type does not follow links: never delete through one.
        if legacy && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_archive_reuses_one_extraction_and_sweeps_legacy_sessions() {
        let root =
            std::env::temp_dir().join(format!("skate-mods-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".cache/1234-5678/1")).unwrap();
        std::fs::create_dir_all(root.join(".cache/keep-me")).unwrap();
        let archive = root.join("m.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"{}").unwrap();
        zip.finish().unwrap();
        let first = super::materialize(&root, &archive).unwrap();
        let second = super::materialize(&root, &archive).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(first.join("mod.json")).unwrap(), b"{}");
        assert!(!root.join(".cache/1234-5678").exists());
        assert!(root.join(".cache/keep-me").exists());
        let names: Vec<_> = std::fs::read_dir(root.join(".cache"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 2, "no staging folder left behind: {names:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
