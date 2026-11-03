//! The Skate 2 data folder: the disc's archives, mounted as the port mounts them. Not tested in play.
//!
//! The folder is either the disc itself or the `skate2-physics` stage's cut-down copy of it, which holds a `manifest.json`
//! (`{"version": 1, "disc": <sha256 of default.xex>, "entries": [[archive, key, stored_size, sha256], ...]}`) naming
//! every entry the sim reads. [`check`] refuses a copy that lacks one of them.

use std::collections::HashMap;
use std::fmt;
use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

use skate2_formats::big::BigEntryKey;
use skate2_formats::host_path::on_disk;
use skate2_formats::mount::{MountError, MountSet, MountedBig, SKATE2_ARCHIVES};

/// The stage's manifest, directly under the data folder.
pub const MANIFEST: &str = "manifest.json";

/// The manifest version this host reads.
const VERSION: u64 = 1;

#[derive(Debug)]
pub enum DataError {
    /// An archive that is under the folder could not be opened.
    Open(PathBuf, MountError),
    /// The first entry the manifest lists that the folder does not hold (the archive is missing, or has no such key, or
    /// holds it with another size), as `"<archive> <key>"`: `data\big\anim.big 0x07138dc7`.
    Missing(String),
    /// The manifest cannot be read or is not a version 1 manifest.
    Manifest(PathBuf, String),
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(path, why) => write!(f, "{}: {why}", path.display()),
            Self::Missing(entry) => write!(f, "the Skate 2 data lacks {entry}"),
            Self::Manifest(path, why) => write!(f, "{}: {why}", path.display()),
        }
    }
}

impl std::error::Error for DataError {}

/// Mounts every archive of `SKATE2_ARCHIVES` whose file is under `root`, in table order, as the spike's
/// `mount_archives` did. An archive that is not there is skipped: the sim reads only some of them.
pub fn mount(root: &Path) -> Result<MountSet, DataError> {
    let mut mounts = MountSet::new();
    for spec in &SKATE2_ARCHIVES {
        let path = spec.path_under(root);
        if path.is_file() {
            let archive = MountedBig::open(&path, spec.use_external_header).map_err(|why| DataError::Open(path, why))?;
            mounts.push(archive);
        }
    }
    Ok(mounts)
}

/// Checks the folder against its manifest and returns how many entries it lists.
///
/// Each archive is opened the way the port opens it (its `.bh` for the three hashed ones) and each listed entry must be in
/// its table with the listed stored size. Only the tables are read, not the entries' bytes, so the check is quick; the
/// manifest's sha256 for each entry is for the stage's own verification.
///
/// # Errors
///
/// [`DataError::Missing`] names the first listed entry that is not there; [`DataError::Manifest`] says the manifest is
/// unreadable. Neither panics.
pub fn check(root: &Path) -> Result<usize, DataError> {
    let path = root.join(MANIFEST);
    let manifest = |why: String| DataError::Manifest(path.clone(), why);
    let text = std::fs::read(&path).map_err(|why| manifest(why.to_string()))?;
    let entries = parse_manifest(&text).map_err(manifest)?;
    // Each archive's table once: key -> stored size, or `None` when the archive cannot be opened.
    let mut tables: HashMap<&str, Option<HashMap<String, u64>>> = HashMap::new();
    for entry in &entries {
        let table = tables.entry(entry.archive.as_str()).or_insert_with(|| table_of(root, &entry.archive));
        if table.as_ref().and_then(|table| table.get(&lookup_key(&entry.key))) != Some(&entry.stored) {
            return Err(DataError::Missing(format!("{} {}", entry.archive, entry.key)));
        }
    }
    Ok(entries.len())
}

/// A name is found ignoring ASCII case, a hash (`0x%08x`) exactly.
fn lookup_key(key: &str) -> String {
    key.to_ascii_lowercase()
}

/// The table the port reads `archive` through: key -> stored size.
fn table_of(root: &Path, archive: &str) -> Option<HashMap<String, u64>> {
    let spec = SKATE2_ARCHIVES.iter().find(|spec| spec.relative_path.eq_ignore_ascii_case(archive));
    let path = match spec {
        Some(spec) => spec.path_under(root),
        None => on_disk(&root.join(archive.replace('\\', MAIN_SEPARATOR_STR))),
    };
    let mounted = MountedBig::open(&path, spec.is_some_and(|spec| spec.use_external_header)).ok()?;
    let mut table = HashMap::new();
    for entry in &mounted.archive().entries {
        let key = match &entry.key {
            BigEntryKey::Name(name) => lookup_key(name),
            BigEntryKey::Hash(hash) => format!("0x{hash:08x}"),
        };
        // the port finds the first of two entries that share a name
        table.entry(key).or_insert(entry.stored_size);
    }
    Some(table)
}

struct Listed {
    archive: String,
    key: String,
    stored: u64,
}

/// The manifest's entries. It is a fixed shape, so this reads it with a small JSON reader instead of a dependency.
fn parse_manifest(text: &[u8]) -> Result<Vec<Listed>, String> {
    let Json::Object(fields) = Reader::new(text).document()? else { return Err("not a JSON object".into()) };
    let field = |name: &str| fields.iter().find(|(key, _)| key == name).map(|(_, value)| value);
    match field("version") {
        Some(Json::Number(number)) if number.parse::<u64>() == Ok(VERSION) => {}
        other => return Err(format!("version {other:?}, this host reads {VERSION}")),
    }
    if !matches!(field("disc"), Some(Json::String(_))) {
        return Err("no \"disc\"".into());
    }
    let Some(Json::Array(rows)) = field("entries") else { return Err("no \"entries\" list".into()) };
    let mut entries = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let bad = || format!("entry {index} is not [archive, key, stored size, sha256]");
        let Json::Array(row) = row else { return Err(bad()) };
        let [Json::String(archive), Json::String(key), Json::Number(stored), Json::String(_)] = row.as_slice() else {
            return Err(bad());
        };
        let stored = stored.parse().map_err(|_| bad())?;
        // The archive is joined to the data folder: it stays inside it.
        if archive.is_empty() || archive.contains("..") || archive.contains(':') || archive.starts_with(['\\', '/']) {
            return Err(format!("entry {index}: archive {archive:?} is not a path under the data folder"));
        }
        entries.push(Listed { archive: archive.clone(), key: key.clone(), stored });
    }
    Ok(entries)
}

#[derive(Debug)]
enum Json {
    /// `true`, `false` or `null`.
    Literal,
    /// The number's text; the caller parses it as the integer it wants.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

struct Reader<'a> {
    text: &'a [u8],
    at: usize,
}

/// Nesting deeper than any manifest: a hostile file must not overflow the stack.
const DEPTH: usize = 8;

impl<'a> Reader<'a> {
    fn new(text: &'a [u8]) -> Self {
        Self { text, at: 0 }
    }

    fn document(mut self) -> Result<Json, String> {
        let value = self.value(0)?;
        self.space();
        if self.at != self.text.len() {
            return Err(format!("text after the document at byte {}", self.at));
        }
        Ok(value)
    }

    fn space(&mut self) {
        while self.text.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.text.get(self.at).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at byte {}", char::from(byte), self.at))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > DEPTH {
            return Err("nested too deep".into());
        }
        match self.peek().ok_or("the document ends early")? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Json::Object(fields));
                }
                loop {
                    let key = self.string()?;
                    self.expect(b':')?;
                    fields.push((key, self.value(depth + 1)?));
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Object(fields));
                        }
                        _ => return Err(format!("expected , or }} at byte {}", self.at)),
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(format!("expected , or ] at byte {}", self.at)),
                    }
                }
            }
            b'"' => self.string().map(Json::String),
            b'-' | b'0'..=b'9' => {
                let start = self.at;
                while self.text.get(self.at).is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.at += 1;
                }
                Ok(Json::Number(String::from_utf8_lossy(&self.text[start..self.at]).into_owned()))
            }
            b't' | b'f' | b'n' => {
                for word in [&b"true"[..], b"false", b"null"] {
                    if self.text[self.at..].starts_with(word) {
                        self.at += word.len();
                        return Ok(Json::Literal);
                    }
                }
                Err(format!("unknown word at byte {}", self.at))
            }
            other => Err(format!("unexpected {:?} at byte {}", char::from(other), self.at)),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let byte = *self.text.get(self.at).ok_or("a string is not closed")?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).map_err(|_| "a string is not UTF-8".to_string()),
                b'\\' => {
                    let escape = *self.text.get(self.at).ok_or("a string is not closed")?;
                    self.at += 1;
                    match escape {
                        b'"' | b'\\' | b'/' => out.push(escape),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let digits = self.text.get(self.at..self.at + 4).ok_or("a \\u escape is cut short")?;
                            let code = std::str::from_utf8(digits)
                                .ok()
                                .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                                .ok_or("a \\u escape is not four hex digits")?;
                            self.at += 4;
                            // a surrogate half is not a character: the paths in a manifest are ASCII
                            let character = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
                            out.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return Err(format!("unknown escape at byte {}", self.at - 1)),
                    }
                }
                _ => out.push(byte),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{DataError, check};

    /// A named `BIG4` table with `entries`, and their bytes after it.
    fn named_big(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let header: usize = 16 + entries.iter().map(|(name, _)| 8 + name.len() + 1).sum::<usize>();
        let mut bytes = Vec::from(*b"BIG4");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(&u32::try_from(header).unwrap().to_be_bytes());
        let mut at = header;
        for (name, payload) in entries {
            bytes.extend_from_slice(&u32::try_from(at).unwrap().to_be_bytes());
            bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
            at += payload.len();
        }
        for (_, payload) in entries {
            bytes.extend_from_slice(payload);
        }
        bytes
    }

    /// A hashed `Viv4` table of `(hash, offset, stored)` rows.
    fn viv4(rows: &[(u32, u32, u32)]) -> Vec<u8> {
        let size = u32::try_from(16 + rows.len() * 16).unwrap();
        let mut bytes = Vec::from(*b"Viv4");
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(rows.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(&size.to_be_bytes());
        for &(hash, offset, stored) in rows {
            for word in [offset, stored, 0, hash] {
                bytes.extend_from_slice(&word.to_be_bytes());
            }
        }
        bytes
    }

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            // the tests run side by side: one folder each
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir()
                .join(format!("skate2-physics-host-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn put(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The stage's manifest text for `(archive, key, stored size)` rows.
    fn manifest(entries: &[(&str, String, usize)]) -> String {
        let rows: Vec<String> = entries
            .iter()
            .map(|(archive, key, size)| {
                format!("[\"{}\", \"{}\", {size}, \"00\"]", archive.replace('\\', "\\\\"), key.replace('\\', "\\\\"))
            })
            .collect();
        format!("{{\"version\": 1, \"disc\": \"00\", \"entries\": [\n{}\n]}}", rows.join(",\n"))
    }

    /// `data\big\anim.big` (the port reads it through its hashed `.bh`) and `data\db\db.big` (a named table).
    fn set() -> Folder {
        let folder = Folder::new("check");
        let hash = skate2_formats::big::djb2(b"data\\anim\\one.abin");
        folder.put("data/big/anim.big", &[7_u8; 64]);
        folder.put("data/big/anim.bh", &viv4(&[(hash, 16, 30)]));
        folder.put("data/db/db.big", &named_big(&[("data\\db\\skater.vlt", b"vlt-bytes"), ("data\\db\\Skater.bin", b"bin")]));
        folder
    }

    fn listed() -> Vec<(&'static str, String, usize)> {
        let hash = skate2_formats::big::djb2(b"data\\anim\\one.abin");
        vec![
            ("data\\big\\anim.big", format!("0x{hash:08x}"), 30),
            ("data\\db\\db.big", "data\\db\\skater.vlt".into(), 9),
            ("data\\db\\db.big", "data\\db\\skater.bin".into(), 3),
        ]
    }

    fn write_manifest(folder: &Folder, entries: &[(&str, String, usize)]) {
        folder.put("manifest.json", manifest(entries).as_bytes());
    }

    #[test]
    fn a_complete_set_passes_the_check() {
        let folder = set();
        write_manifest(&folder, &listed());
        assert_eq!(check(folder.path()).unwrap(), 3);
    }

    #[test]
    fn a_partial_set_is_refused_by_name() {
        // an entry the archive does not hold, after one it does
        let folder = set();
        let mut entries = listed();
        entries.insert(2, ("data\\db\\db.big", "data\\db\\gone.vlt".into(), 5));
        write_manifest(&folder, &entries);
        match check(folder.path()) {
            Err(DataError::Missing(name)) => assert_eq!(name, "data\\db\\db.big data\\db\\gone.vlt"),
            other => panic!("expected Missing, got {other:?}"),
        }

        // a hash the .bh does not hold
        let mut entries = listed();
        entries[0].1 = "0x00000001".into();
        write_manifest(&folder, &entries);
        match check(folder.path()) {
            Err(DataError::Missing(name)) => assert_eq!(name, "data\\big\\anim.big 0x00000001"),
            other => panic!("expected Missing, got {other:?}"),
        }

        // a size that is not the listed one
        let mut entries = listed();
        entries[1].2 = 10;
        write_manifest(&folder, &entries);
        assert!(matches!(check(folder.path()), Err(DataError::Missing(name)) if name.contains("skater.vlt")));

        // a whole archive gone: the first entry listed for it is named
        write_manifest(&folder, &listed());
        fs::remove_file(folder.path().join("data/db/db.big")).unwrap();
        match check(folder.path()) {
            Err(DataError::Missing(name)) => assert_eq!(name, "data\\db\\db.big data\\db\\skater.vlt"),
            other => panic!("expected Missing, got {other:?}"),
        }

        // the .bh gone: the port would then read the .big table, which holds no hashes
        let folder = set();
        write_manifest(&folder, &listed());
        fs::remove_file(folder.path().join("data/big/anim.bh")).unwrap();
        assert!(matches!(check(folder.path()), Err(DataError::Missing(name)) if name.contains("anim.big")));
    }

    static MISSES: AtomicUsize = AtomicUsize::new(0);

    fn count_miss(_path: &str) {
        MISSES.fetch_add(1, Ordering::Relaxed);
    }

    /// The stage test counts misses through this observer, so a miss must reach it and a hit must not.
    #[test]
    fn a_path_read_that_finds_nothing_reaches_the_miss_observer() {
        skate2_formats::mount::observe_misses(count_miss);
        let folder = set();
        let mounts = super::mount(folder.path()).unwrap();
        assert_eq!(mounts.read_path("data\\db\\skater.vlt").unwrap(), b"vlt-bytes");
        assert_eq!(MISSES.load(Ordering::Relaxed), 0);
        assert!(matches!(mounts.read_path("data\\db\\nothing.vlt"), Err(skate2_formats::mount::MountError::EntryNotFound(_))));
        assert_eq!(MISSES.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_bad_manifest_is_an_error_not_a_panic() {
        let folder = set();
        for text in [
            "",
            "[",
            "{\"version\": 2, \"disc\": \"\", \"entries\": []}",
            "{\"version\": 1, \"entries\": [[\"a\", \"b\", -1, \"\"]]}",
            "{\"version\": 1, \"disc\": \"\", \"entries\": [[\"a\", \"b\"]]}",
            "{\"version\": 1, \"disc\": \"\", \"entries\": [[\"a\\\"]}",
        ] {
            folder.put("manifest.json", text.as_bytes());
            assert!(matches!(check(folder.path()), Err(DataError::Manifest(..))), "{text:?}");
        }
        fs::remove_file(folder.path().join("manifest.json")).unwrap();
        assert!(matches!(check(folder.path()), Err(DataError::Manifest(..))));
    }
}
