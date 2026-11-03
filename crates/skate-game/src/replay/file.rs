//! Local replay saves (`.s3replay`, research Q8 / U3): the recording's zstd blocks as the ring holds them, a JSON
//! header and the editor's timeline. Saved under `Documents\Skate 3 Remaster\Replays\`, at most [`MAX_SLOTS`] (retail
//! 60, research Q8 [disc, R2]). A file is written to a temp name and renamed, so a failed save leaves nothing behind.
//!
//! Layout: MAGIC, VERSION u16 LE, header length u32 + header JSON, block count u32 + (length u32 + zstd block)...,
//! timeline length u32 + timeline JSON.
use super::{
    editor::{CameraKind, Channel, Ease, Editor, Key, LETTERBOX, Look, Shot, Value},
    record::{self, Reader, Ring},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};

pub(crate) const MAGIC: &[u8; 8] = b"S3REPLAY";
/// 2: replay blocks carry the weather (record.rs block version 3); version 1 saves still load, dry.
/// 3 (item 16): the timeline's keys carry an ease, camera shots a roll, and Replay Settings the letterbox and camera
/// path; a version 1 / 2 timeline loads with them at their retail defaults (the layout is unchanged).
pub(crate) const VERSION: u16 = 3;
/// research Q8 [disc, R2]: retail has 60 local replay slots. Not retail (owner, bug 10-17 #34): 200. A save is at most
/// 11 MiB (full_ring_file_under_11_mib), so all 200 slots full is about 2.2 GB in total.
pub(crate) const MAX_SLOTS: usize = 200;
pub(crate) const EXTENSION: &str = "s3replay";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Header {
    pub version: u16,
    pub build: String,
    /// The map's name (CurrentMap::name) and its package, to load it first.
    pub map: String,
    #[serde(default)]
    pub map_path: Option<String>,
    pub created_unix: u64,
    pub seconds: f32,
    pub rate_hz: u32,
    // RULING (review M8): no appearance blobs. A save plays on the live skater's rig (load checks its bone count)
    // and remote skaters are recorded as bone transforms, so there is nothing for a look to restore.
}
impl Header {
    pub fn new(map: &str, map_path: Option<&Path>, ring: &Ring) -> Self {
        let span = match (ring.time(0), ring.len().checked_sub(1).and_then(|i| ring.time(i))) {
            (Some(a), Some(b)) => (b - a) as f32,
            _ => 0.,
        };
        Self {
            version: VERSION,
            build: env!("CARGO_PKG_VERSION").into(),
            map: map.into(),
            map_path: map_path.map(|p| p.to_string_lossy().into_owned()),
            created_unix: unix_now(),
            seconds: span,
            rate_hz: record::RATE_HZ,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum SaveError {
    SlotsFull,
    Io(String),
}
#[derive(Debug, PartialEq)]
pub(crate) enum LoadError {
    Magic,
    Version(u16),
    Corrupt(String),
    Io(String),
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `YYYYMMDD-HHMMSS` in local time for file names (review M11).
pub(crate) fn stamp(unix: u64) -> String {
    let [y, m, d, h, mi, s] = local(unix);
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// `unix` as the local calendar time (Windows' time zone rules for that date, DST included); UTC elsewhere or if
/// Windows refuses.
pub(crate) fn local(unix: u64) -> [i64; 6] {
    let utc = civil(unix);
    #[cfg(windows)]
    {
        use windows::Win32::{Foundation::SYSTEMTIME, System::Time::SystemTimeToTzSpecificLocalTime};
        let [y, m, d, h, mi, s] = utc.map(|v| v as u16);
        let from = SYSTEMTIME { wYear: y, wMonth: m, wDay: d, wHour: h, wMinute: mi, wSecond: s, ..Default::default() };
        let mut to = SYSTEMTIME::default();
        // SAFETY: two plain structs; None = the current time zone.
        if unsafe { SystemTimeToTzSpecificLocalTime(None, &from, &mut to) }.is_ok() {
            return [to.wYear, to.wMonth, to.wDay, to.wHour, to.wMinute, to.wSecond].map(i64::from);
        }
    }
    utc
}

/// `unix` as the UTC calendar: [year, month, day, hour, minute, second].
fn civil(unix: u64) -> [i64; 6] {
    let (days, rest) = ((unix / 86400) as i64, (unix % 86400) as i64);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    [y, m, d, rest / 3600, rest / 60 % 60, rest % 60]
}

/// A file-name-safe map name.
pub(crate) fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "replay".into() } else { s }
}

/// U3: `Documents` / `Pictures` / `Videos` (the Windows known folders) + `Skate 3 Remaster`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Folder {
    Documents,
    Pictures,
    Videos,
}
pub(crate) fn known_folder(which: Folder) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use windows::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{FOLDERID_Documents, FOLDERID_Pictures, FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
        };
        let id = match which {
            Folder::Documents => FOLDERID_Documents,
            Folder::Pictures => FOLDERID_Pictures,
            Folder::Videos => FOLDERID_Videos,
        };
        // SAFETY: the returned string is owned by us and freed with CoTaskMemFree.
        unsafe {
            let path = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None).ok()?;
            let text = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as *const _));
            text.map(PathBuf::from)
        }
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(home.join(match which {
            Folder::Documents => "Documents",
            Folder::Pictures => "Pictures",
            Folder::Videos => "Videos",
        }))
    }
}
pub(crate) fn folder(which: Folder) -> Option<PathBuf> {
    let sub = match which {
        Folder::Documents => Path::new("Skate 3 Remaster").join("Replays"),
        Folder::Pictures | Folder::Videos => PathBuf::from("Skate 3 Remaster"),
    };
    known_folder(which).map(|p| p.join(sub))
}

// ---------------------------------------------------------------- timeline JSON
fn shot_json(s: &Shot) -> Json {
    json!({"kind": s.kind.index(), "yaw": s.yaw, "pitch": s.pitch, "distance": s.distance, "height": s.height,
        "lens": s.lens, "eye": s.eye.to_array(), "look": s.look.to_array(), "roll": s.roll})
}
fn shot_from(j: &Json) -> Option<Shot> {
    let f = |k: &str| j[k].as_f64().map(|v| v as f32);
    let v3 = |k: &str| -> Option<bevy::math::Vec3> {
        let a = j[k].as_array()?;
        Some(bevy::math::Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32))
    };
    Some(Shot {
        kind: *CameraKind::LIST.get(j["kind"].as_u64()? as usize)?,
        yaw: f("yaw")?,
        pitch: f("pitch")?,
        distance: f("distance")?,
        height: f("height")?,
        lens: f("lens")?,
        eye: v3("eye")?,
        look: v3("look")?,
        // Version 3; absent before (0 = retail).
        roll: if j["roll"].is_null() { 0. } else { f("roll")? },
    })
}
pub(crate) fn timeline_json(editor: &Editor) -> Json {
    let keys: Vec<Json> = editor
        .keys
        .iter()
        .map(|keys| {
            Json::Array(
                keys.iter()
                    .map(|k| {
                        let value = match &k.value {
                            Value::Int(v) => json!({"int": v}),
                            Value::Tint(v) => json!({"tint": v}),
                            Value::Camera(s) => json!({"camera": shot_json(s)}),
                        };
                        json!({"t": k.t, "soft": k.soft, "cut": k.cut, "value": value, "ease": k.ease as u8})
                    })
                    .collect(),
            )
        })
        .collect();
    let l = &editor.look;
    json!({
        "in": editor.in_mark, "out": editor.out_mark, "markers": editor.clip_markers, "keys": keys,
        "look": [l.green_screen, l.hide_player, l.hide_skaters],
        "letterbox": l.letterbox, "path": l.camera_path, "cameras": CAMERAS,
    })
}
/// The camera model a timeline's shots are in: 2 (10-27) = retail's (a Follow eye in the game camera's frame,
/// `height` the aim's offset); absent = the earlier one, migrated as it loads.
const CAMERAS: u64 = 2;
pub(crate) fn editor_from(j: &Json) -> Result<Editor, String> {
    let bad = |what: &str| format!("timeline: bad {what}");
    let mut editor = Editor::new(false);
    let unit = |v: &Json| v.as_f64().map(|v| v as f32).filter(|v| (0. ..=1.).contains(v));
    let int = |v: &Json| v.as_i64().and_then(|i| i32::try_from(i).ok());
    editor.in_mark = unit(&j["in"]).ok_or_else(|| bad("in mark"))?;
    editor.out_mark = unit(&j["out"]).ok_or_else(|| bad("out mark"))?;
    editor.clip_markers = j["markers"].as_array().ok_or_else(|| bad("markers"))?.iter().map(unit).collect::<Option<_>>().ok_or_else(|| bad("marker"))?;
    let channels = j["keys"].as_array().filter(|k| k.len() == 7).ok_or_else(|| bad("keys"))?;
    for ((slot, keys), channel) in editor.keys.iter_mut().zip(channels).zip(Channel::ALL) {
        for k in keys.as_array().ok_or_else(|| bad("key list"))? {
            let v = &k["value"];
            let value = if let Some(i) = int(&v["int"]) {
                Value::Int(i)
            } else if let Some(t) = v["tint"].as_array().filter(|t| t.len() == 4) {
                Value::Tint([0, 1, 2, 3].map(|i| int(&t[i]).unwrap_or(0)))
            } else {
                Value::Camera(shot_from(&v["camera"]).ok_or_else(|| bad("camera key"))?)
            };
            // Version 3's ease; a version 1 / 2 key moves as retail's did.
            let ease = match k["ease"].as_u64() {
                None if k["ease"].is_null() => Ease::retail(channel),
                Some(i) if (i as usize) < Ease::ALL.len() => Ease::ALL[i as usize],
                _ => return Err(bad("key ease")),
            };
            slot.push(Key { t: unit(&k["t"]).ok_or_else(|| bad("key time"))?, soft: k["soft"].as_bool().unwrap_or(false), cut: k["cut"].as_bool().unwrap_or(false), value, ease });
        }
    }
    let look: Vec<bool> = j["look"].as_array().map(|a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect()).unwrap_or_default();
    let at = |i: usize| look.get(i).copied().unwrap_or(false);
    let letterbox = j["letterbox"].as_u64().filter(|&i| (i as usize) < LETTERBOX.len()).unwrap_or(0) as u8;
    editor.look = Look { green_screen: at(0), hide_player: at(1), hide_skaters: at(2), letterbox, camera_path: j["path"].as_bool().unwrap_or(false) };
    if j["cameras"].as_u64() != Some(CAMERAS) {
        // The earlier model raised a Tripod eye by `height` and looked at the skater: the same view with the eye
        // raised and no aim offset. Its Follow shots need the recording (cameras::migrate_follow, as hud.rs opens it).
        for key in &mut editor.keys[0] {
            if let Value::Camera(shot) = &mut key.value {
                match shot.kind {
                    CameraKind::Tripod | CameraKind::AdvTripod => (shot.eye.y, shot.height) = (shot.eye.y + shot.height, 0.),
                    CameraKind::Follow | CameraKind::AdvFollow => editor.legacy_follow = true,
                    _ => {}
                }
            }
        }
    }
    super::editor::sanitise(&mut editor)?;
    Ok(editor)
}

// ---------------------------------------------------------------- the file
pub(crate) fn encode(header: &Header, ring: &Ring, editor: &Editor) -> Vec<u8> {
    let mut out = Vec::new();
    let chunk = |out: &mut Vec<u8>, bytes: &[u8]| {
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend(bytes);
    };
    out.extend(MAGIC);
    out.extend(VERSION.to_le_bytes());
    chunk(&mut out, &serde_json::to_vec(header).unwrap_or_default());
    let blocks = ring.export_blocks();
    out.extend((blocks.len() as u32).to_le_bytes());
    for block in &blocks {
        chunk(&mut out, block);
    }
    chunk(&mut out, &serde_json::to_vec(&timeline_json(editor)).unwrap_or_default());
    out
}

pub(crate) fn decode(bytes: &[u8]) -> Result<(Header, Ring, Editor), LoadError> {
    let corrupt = LoadError::Corrupt;
    if bytes.len() < 10 || &bytes[..8] != MAGIC {
        return Err(LoadError::Magic);
    }
    let mut r = Reader { bytes: &bytes[8..], at: 0 };
    let version = r.u16().map_err(corrupt)?;
    if !(1..=VERSION).contains(&version) {
        return Err(LoadError::Version(version));
    }
    let chunk = |r: &mut Reader| -> Result<Vec<u8>, LoadError> {
        let n = r.u32().map_err(corrupt)? as usize;
        r.take(n).map(<[u8]>::to_vec).map_err(corrupt)
    };
    let header: Header = serde_json::from_slice(&chunk(&mut r)?).map_err(|e| corrupt(e.to_string()))?;
    let count = r.u32().map_err(corrupt)? as usize;
    // Review I4: a full ring is RING_FRAMES / BLOCK_FRAMES blocks, plus a trimmed front and the open block.
    if count > record::RING_FRAMES / record::BLOCK_FRAMES + 2 {
        return Err(corrupt(format!("{count} blocks")));
    }
    let blocks = (0..count).map(|_| chunk(&mut r)).collect::<Result<Vec<_>, _>>()?;
    let timeline: Json = serde_json::from_slice(&chunk(&mut r)?).map_err(|e| corrupt(e.to_string()))?;
    let ring = Ring::from_blocks(blocks).map_err(corrupt)?;
    let editor = editor_from(&timeline).map_err(corrupt)?;
    Ok((header, ring, editor))
}

pub(crate) fn load(path: &Path) -> Result<(Header, Ring, Editor), LoadError> {
    decode(&std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?)
}

/// A write's temp file this old was left by a crash or kill between write and rename (bug 10-17 #35): a live save
/// renames its own within the same frame.
const STALE_TEMP: std::time::Duration = std::time::Duration::from_secs(60);

/// Removes the stale `<name>.tmp` files `write_atomic` leaves when the game dies mid-save.
fn sweep_temps(dir: &Path) {
    for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        let stale = path.extension().is_some_and(|e| e == "tmp")
            && std::fs::metadata(&path).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age >= STALE_TEMP));
        if stale && let Err(error) = std::fs::remove_file(&path) {
            bevy::log::warn!("Replay temp {}: {error}", path.display());
        }
    }
}

/// Deletes a save; one already gone counts as deleted. An error (the file in use, read-only) leaves it listed.
pub(crate) fn delete(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The saves in `dir` (stale temp files swept first).
fn replays(dir: &Path) -> Vec<PathBuf> {
    sweep_temps(dir);
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == EXTENSION)).collect()
        })
        .unwrap_or_default()
}

/// Writes `bytes` to `path` through a temp file; `before_rename` stands in for a failure between the two (tests).
fn write_atomic(path: &Path, bytes: &[u8], before_rename: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
    let temp = path.with_extension("tmp");
    let result = std::fs::write(&temp, bytes).and_then(|()| before_rename()).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub(crate) fn save(dir: &Path, header: &Header, ring: &Ring, editor: &Editor) -> Result<PathBuf, SaveError> {
    save_encoded(dir, header, &encode(header, ring, editor))
}
/// [`save`] of a replay already [`encode`]d (an online match's quick save writes it off the main thread: hud.rs).
pub(crate) fn save_encoded(dir: &Path, header: &Header, bytes: &[u8]) -> Result<PathBuf, SaveError> {
    save_with(dir, header, bytes, || Ok(()))
}
fn save_with(dir: &Path, header: &Header, bytes: &[u8], before_rename: impl FnOnce() -> std::io::Result<()>) -> Result<PathBuf, SaveError> {
    std::fs::create_dir_all(dir).map_err(|e| SaveError::Io(e.to_string()))?;
    if replays(dir).len() >= MAX_SLOTS {
        return Err(SaveError::SlotsFull);
    }
    let base = format!("{}-{}", safe(&header.map), stamp(header.created_unix));
    let path = (0..)
        .map(|n| dir.join(if n == 0 { format!("{base}.{EXTENSION}") } else { format!("{base}-{n}.{EXTENSION}") }))
        .find(|p| !p.exists())
        .expect("an unused name");
    write_atomic(&path, bytes, before_rename).map_err(|e| SaveError::Io(e.to_string()))?;
    Ok(path)
}

/// A save's header, read alone (review I10): 14 bytes, then at most 64 KiB of header JSON. None for a file that is
/// not a readable replay of this version (it is listed as damaged).
pub(crate) fn header(path: &Path) -> Option<Header> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 14];
    file.read_exact(&mut head).ok()?;
    let version = u16::from_le_bytes([head[8], head[9]]);
    let n = u32::from_le_bytes([head[10], head[11], head[12], head[13]]) as usize;
    if &head[..8] != MAGIC || !(1..=VERSION).contains(&version) || n > 64 * 1024 {
        return None;
    }
    let mut json = vec![0; n];
    file.read_exact(&mut json).ok()?;
    serde_json::from_slice(&json).ok()
}

/// Every save in `dir`, newest first (by its header, else its file time); a damaged one has no header. Reads only
/// the headers; hud.rs runs it off the main thread.
pub(crate) fn list(dir: &Path) -> Vec<(PathBuf, Option<Header>)> {
    let modified = |p: &Path| {
        std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
    };
    let mut found: Vec<(PathBuf, Option<Header>, u64)> = replays(dir)
        .into_iter()
        .map(|path| {
            let header = header(&path);
            let when = header.as_ref().map_or_else(|| modified(&path), |h| h.created_unix);
            (path, header, when)
        })
        .collect();
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.0.cmp(&a.0)));
    found.into_iter().map(|(p, h, _)| (p, h)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{editor::Channel, record::tests::frame};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s3r-replay-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
    fn ring(frames: usize, skaters: usize) -> Ring {
        let mut ring = Ring::default();
        for i in 0..frames {
            ring.push(frame(i, skaters as u32));
        }
        ring
    }
    fn edited() -> Editor {
        let mut editor = Editor::new(false);
        editor.state = crate::replay::editor::State::MainEdit;
        editor.playhead = 0.4;
        editor.drop_marker().unwrap();
        editor.adjust(Channel::Speed, false);
        editor.look.green_screen = true;
        editor
    }

    #[test]
    fn save_load_round_trip() {
        let dir = temp("round");
        let ring = ring(200, 2);
        let editor = edited();
        let header = Header::new("San Vanelona", Some(Path::new("maps/sv.skate")), &ring);
        let path = save(&dir, &header, &ring, &editor).unwrap();
        assert!(path.starts_with(&dir));
        let (h, r, e) = load(&path).unwrap();
        assert_eq!(h, header);
        assert_eq!(r.len(), ring.len());
        assert_eq!(r.time(150), ring.time(150));
        assert_eq!((e.clip_markers.clone(), e.keys.clone(), e.look), (editor.clip_markers.clone(), editor.keys.clone(), editor.look));
        assert_eq!(list(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Item 16: a version 2 save (no ease, roll, letterbox or path) loads with the retail defaults and comes back
    /// out of a version 3 save unchanged; the version 3 fields round-trip.
    #[test]
    fn v2_saves_load_and_v3_fields_round_trip() {
        let ring = ring(130, 1);
        let header = Header::new("m", None, &ring);
        let v2 = crate::replay::video_tests::retail_timeline_v2();
        let with = |version: u16, timeline: &Json| {
            let mut out = Vec::new();
            out.extend(MAGIC);
            out.extend(version.to_le_bytes());
            let h = serde_json::to_vec(&header).unwrap();
            out.extend((h.len() as u32).to_le_bytes());
            out.extend(h);
            let blocks = ring.export_blocks();
            out.extend((blocks.len() as u32).to_le_bytes());
            for b in &blocks {
                out.extend((b.len() as u32).to_le_bytes());
                out.extend(b);
            }
            let t = serde_json::to_vec(timeline).unwrap();
            out.extend((t.len() as u32).to_le_bytes());
            out.extend(t);
            out
        };
        let (_, _, old) = decode(&with(2, &v2)).unwrap();
        for (channel, keys) in Channel::ALL.iter().zip(&old.keys) {
            assert!(keys.iter().all(|k| k.ease == Ease::retail(*channel)), "{channel:?}");
        }
        assert!(old.keys[0].iter().all(|k| matches!(k.value, Value::Camera(s) if s.roll == 0.)));
        assert_eq!((old.look.letterbox, old.look.camera_path), (0, false));
        // v2 -> v3 -> load: the same timeline.
        let (_, _, again) = decode(&encode(&header, &ring, &old)).unwrap();
        assert_eq!((again.keys.clone(), again.clip_markers.clone(), again.look), (old.keys.clone(), old.clip_markers.clone(), old.look));
        // The version 3 fields survive.
        let mut edited = old.clone();
        edited.keys[1][2].ease = Ease::Smooth;
        edited.keys[3][1].ease = Ease::Linear;
        if let Value::Camera(shot) = &mut edited.keys[0][2].value {
            shot.roll = -35.5;
        }
        edited.look.letterbox = 3;
        edited.look.camera_path = true;
        let (_, _, back) = decode(&encode(&header, &ring, &edited)).unwrap();
        assert_eq!((back.keys, back.look), (edited.keys, edited.look));
        // A bad ease is a load error; a letterbox out of range loads as Off.
        let mut bad = timeline_json(&old);
        bad["keys"][1][0]["ease"] = json!(7);
        assert!(matches!(decode(&with(3, &bad)), Err(LoadError::Corrupt(_))));
        let mut wide = timeline_json(&old);
        wide["letterbox"] = json!(9);
        assert_eq!(decode(&with(3, &wide)).unwrap().2.look.letterbox, 0);
    }

    /// 10-27 (the cameras made retail's): a timeline without `"cameras": 2` migrates as it loads: a Tripod takes its
    /// raised eye (`height` was an eye lift: the same view), a Follow shot waits for its recording
    /// (cameras::migrate_follow); a timeline saved now loads as saved.
    #[test]
    fn an_earlier_timelines_cameras_migrate_as_it_loads() {
        let mut editor = Editor::new(false);
        let mut tripod = Shot::new(CameraKind::Tripod);
        (tripod.eye, tripod.height) = (bevy::math::Vec3::new(1., 2., 3.), 0.5);
        let mut follow = Shot::new(CameraKind::Follow);
        follow.height = 0.25;
        let key = |t: f32, shot: Shot| Key { t, soft: false, cut: false, value: Value::Camera(shot), ease: Ease::Smooth };
        editor.keys[0] = vec![key(0., tripod), key(0.5, follow)];
        editor.clip_markers = vec![0.5];
        let mut earlier = timeline_json(&editor);
        earlier.as_object_mut().expect("an object").remove("cameras");
        let loaded = editor_from(&earlier).unwrap();
        let shot = |e: &Editor, i: usize| match e.keys[0][i].value {
            Value::Camera(s) => s,
            _ => panic!("a camera key"),
        };
        assert_eq!((shot(&loaded, 0).eye, shot(&loaded, 0).height), (bevy::math::Vec3::new(1., 2.5, 3.), 0.));
        assert!(loaded.legacy_follow && shot(&loaded, 1) == follow, "the Follow shot waits for cameras::migrate_follow");
        let now = editor_from(&timeline_json(&editor)).unwrap();
        assert!(!now.legacy_follow);
        assert_eq!(now.keys, editor.keys, "a timeline saved now loads as saved");
    }

    #[test]
    fn the_slot_limit_holds() {
        let dir = temp("slots");
        let ring = ring(10, 1);
        let mut header = Header::new("m", None, &ring);
        for i in 0..MAX_SLOTS {
            header.created_unix = i as u64;
            save(&dir, &header, &ring, &Editor::new(false)).unwrap();
        }
        assert_eq!(save(&dir, &header, &ring, &Editor::new(false)), Err(SaveError::SlotsFull));
        assert_eq!(list(&dir).first().and_then(|(_, h)| h.as_ref()).map(|h| h.created_unix), Some(MAX_SLOTS as u64 - 1), "newest first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_magic_is_load_error() {
        let mut bytes = encode(&Header::new("m", None, &ring(5, 1)), &ring(5, 1), &Editor::new(false));
        bytes[0] = b'X';
        assert!(matches!(decode(&bytes), Err(LoadError::Magic)));
    }

    #[test]
    fn future_version_is_load_error() {
        let mut bytes = encode(&Header::new("m", None, &ring(5, 1)), &ring(5, 1), &Editor::new(false));
        bytes[8..10].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(decode(&bytes).err(), Some(LoadError::Version(VERSION + 1)));
    }

    #[test]
    fn truncated_file_is_load_error_not_panic() {
        let bytes = encode(&Header::new("m", None, &ring(130, 2)), &ring(130, 2), &edited());
        for cut in [10, 13, 40, bytes.len() / 2, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        // A flipped byte inside a block is caught too (zstd or the frame codec).
        let mut flipped = bytes.clone();
        let middle = bytes.len() / 2;
        flipped[middle] ^= 0xff;
        let _ = decode(&flipped);
    }

    #[test]
    fn partial_write_leaves_no_file() {
        let dir = temp("partial");
        let ring = ring(10, 1);
        let header = Header::new("m", None, &ring);
        let failed = save_with(&dir, &header, &encode(&header, &ring, &Editor::new(false)), || Err(std::io::Error::other("disk full")));
        assert!(matches!(failed, Err(SaveError::Io(_))));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "neither the replay nor its temp file remain");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bug 10-17 #35: a crash between write and rename leaves `<name>.tmp`; the next listing sweeps it once it is
    /// stale, never a save in progress, and never a save.
    #[test]
    fn stale_temp_files_are_swept() {
        let dir = temp("sweep");
        std::fs::create_dir_all(&dir).unwrap();
        let (stale, fresh, save) = (dir.join("m-1.tmp"), dir.join("m-2.tmp"), dir.join(format!("m-3.{EXTENSION}")));
        for path in [&stale, &fresh, &save] {
            std::fs::write(path, b"x").unwrap();
        }
        let old = std::time::SystemTime::now() - STALE_TEMP - std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
        assert_eq!(replays(&dir), vec![save.clone()]);
        assert!(!stale.exists() && fresh.exists() && save.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bug 10-17 #35: a delete that fails reports it (the reel keeps the save); one already gone is deleted.
    #[test]
    fn a_failed_delete_is_reported() {
        let dir = temp("delete");
        // A folder under a save's name: remove_file refuses it on every platform.
        let blocked = dir.join(format!("m.{EXTENSION}"));
        std::fs::create_dir_all(&blocked).unwrap();
        assert!(delete(&blocked).is_err());
        assert!(blocked.exists());
        assert!(delete(&dir.join("gone.s3replay")).is_ok());
        let save = dir.join(format!("n.{EXTENSION}"));
        std::fs::write(&save, b"x").unwrap();
        assert!(delete(&save).is_ok() && !save.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_ring_file_under_11_mib() {
        let ring = ring(record::RING_FRAMES, 4);
        let bytes = encode(&Header::new("m", None, &ring), &ring, &edited());
        assert!(bytes.len() <= record::RING_BYTES, "{} bytes", bytes.len());
    }

    /// The header names the map, so a replay of another map loads that map first (hud.rs: MapTransition::request).
    #[test]
    fn loading_other_map_requests_map_load() {
        let ring = ring(10, 1);
        let header = Header::new("Port Carverton", Some(Path::new("maps/pc.skate")), &ring);
        let (h, ..) = decode(&encode(&header, &ring, &Editor::new(false))).unwrap();
        let other = super::super::hud::needs_map(&h, Some(Path::new("maps/sv.skate")));
        assert_eq!(other.map(|e| e.path), Some(Some(PathBuf::from("maps/pc.skate"))));
        assert!(super::super::hud::needs_map(&h, Some(Path::new("maps/pc.skate"))).is_none());
    }

    #[test]
    fn stamps_are_civil() {
        assert_eq!(civil(0), [1970, 1, 1, 0, 0, 0]);
        assert_eq!(civil(1_791_201_845), [2026, 10, 5, 12, 4, 5]);
        // Local time is a whole number of quarter hours from UTC, never more than a day away.
        let [.., mi, _] = local(1_791_201_845);
        assert!(stamp(1_791_201_845).starts_with("2026100"), "{}", stamp(1_791_201_845));
        assert_eq!(mi % 15, 4 % 15);
    }

    /// Review I10: list() reads headers only, lists damaged files too, and finds every save, not the newest 8.
    #[test]
    fn list_reads_headers_and_keeps_damaged_files() {
        let dir = temp("list");
        let ring = ring(10, 1);
        let mut header = Header::new("m", None, &ring);
        for i in 0..12 {
            header.created_unix = 1_000 + i;
            save(&dir, &header, &ring, &Editor::new(false)).unwrap();
        }
        std::fs::write(dir.join("junk.s3replay"), b"not a replay").unwrap();
        let found = list(&dir);
        assert_eq!(found.len(), 13);
        assert_eq!(found.iter().filter(|(_, h)| h.is_none()).count(), 1);
        // Newest first: the damaged file by its file time (now), then the saves by their headers.
        let times: Vec<u64> = found.iter().filter_map(|(_, h)| h.as_ref().map(|h| h.created_unix)).collect();
        assert_eq!(times, (1_000..1_012).rev().collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review I4: one case per hostile save the review lists, each a LoadError and no panic.
    #[test]
    fn hostile_saves_are_load_errors() {
        let ring = ring(130, 2);
        let header = Header::new("m", None, &ring);
        let good = encode(&header, &ring, &edited());
        assert!(decode(&good).is_ok());
        // In after out.
        let mut timeline = timeline_json(&edited());
        timeline["in"] = json!(0.999);
        timeline["out"] = json!(0.0005);
        let with = |timeline: &Json| {
            let mut out = Vec::new();
            out.extend(MAGIC);
            out.extend(VERSION.to_le_bytes());
            let h = serde_json::to_vec(&header).unwrap();
            out.extend((h.len() as u32).to_le_bytes());
            out.extend(h);
            let blocks = ring.export_blocks();
            out.extend((blocks.len() as u32).to_le_bytes());
            for b in &blocks {
                out.extend((b.len() as u32).to_le_bytes());
                out.extend(b);
            }
            let t = serde_json::to_vec(timeline).unwrap();
            out.extend((t.len() as u32).to_le_bytes());
            out.extend(t);
            out
        };
        assert!(matches!(decode(&with(&timeline)), Err(LoadError::Corrupt(_))));
        // Reversed times, a frame without the skater, a zstd bomb: the ring refuses each.
        let mut backwards: Vec<_> = (0..60).map(|i| frame(i, 1)).collect();
        backwards.reverse();
        let mut empty: Vec<_> = (0..60).map(|i| frame(i, 1)).collect();
        empty[7].actors.clear();
        let bomb = zstd::bulk::compress(&vec![0u8; 64 << 20], 3).unwrap();
        for block in [record::encode_block(&backwards), record::encode_block(&empty), bomb] {
            let mut bytes = Vec::new();
            bytes.extend(MAGIC);
            bytes.extend(VERSION.to_le_bytes());
            let h = serde_json::to_vec(&header).unwrap();
            bytes.extend((h.len() as u32).to_le_bytes());
            bytes.extend(h);
            bytes.extend(1u32.to_le_bytes());
            bytes.extend((block.len() as u32).to_le_bytes());
            bytes.extend(&block);
            let t = serde_json::to_vec(&timeline_json(&Editor::new(false))).unwrap();
            bytes.extend((t.len() as u32).to_le_bytes());
            bytes.extend(t);
            assert!(matches!(decode(&bytes), Err(LoadError::Corrupt(_))));
        }
    }

    /// Review I4: 10 000 random and mutated save files. None may panic; every failure is a LoadError, which hud.rs
    /// shows as retail's ID_MEMCARD_REPLAY_LOAD_FAILED popup (hud::load_failure_text).
    #[test]
    fn fuzzed_saves_never_panic() {
        let ring = ring(130, 2);
        let mut editor = edited();
        editor.playhead = 0.6;
        editor.drop_marker().unwrap();
        // Item 16: the version 3 fields are in the fuzzed file too (an eased key, a rolled shot, the letterbox).
        editor.keys[Channel::Speed.index()][0].ease = Ease::Smooth;
        if let Some(Value::Camera(shot)) = editor.keys[0].first_mut().map(|k| &mut k.value) {
            shot.roll = 42.;
        }
        editor.look.letterbox = 2;
        editor.look.camera_path = true;
        let good = encode(&Header::new("m", None, &ring), &ring, &editor);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut ok, mut failed) = (0, 0);
        for i in 0..10_000 {
            let mut bytes = if i % 10 == 0 {
                // Pure noise, sometimes behind the right magic and version.
                let n = (next() % 4096) as usize;
                let mut b: Vec<u8> = (0..n).map(|_| next() as u8).collect();
                if i % 20 == 0 && b.len() > 10 {
                    b[..8].copy_from_slice(MAGIC);
                    b[8..10].copy_from_slice(&VERSION.to_le_bytes());
                }
                b
            } else {
                good.clone()
            };
            if i % 10 != 0 {
                for _ in 0..1 + next() % 8 {
                    let at = (next() as usize) % bytes.len();
                    match next() % 4 {
                        0 => bytes[at] ^= 1 << (next() % 8),
                        1 => bytes[at] = next() as u8,
                        2 => bytes.truncate(at.max(1)),
                        _ => {
                            // A length or count field set huge.
                            let end = (at + 4).min(bytes.len());
                            bytes[at..end].fill(0xff);
                        }
                    }
                }
            }
            match std::panic::catch_unwind(|| decode(&bytes)) {
                Ok(Ok((_, ring, mut editor))) => {
                    ok += 1;
                    // What load does next must not panic either: open, seek, step the marks.
                    let mut replay = crate::replay::Replay::default();
                    replay.install(ring);
                    replay.enter();
                    replay.seek_fraction(editor.playhead);
                    editor.state = crate::replay::editor::State::MainEdit;
                    editor.move_mark(true, -1.);
                    editor.move_mark(false, 1.);
                    let seconds = replay.position().1 as f32;
                    let _ = editor.encoded_seconds(seconds);
                    let _ = crate::replay::grade::Grade::at(&editor, 0.5);
                    let _ = crate::replay::export::playhead_for_frame(&editor, 7, 30, seconds);
                }
                Ok(Err(error)) => {
                    failed += 1;
                    assert_eq!(super::super::hud::load_failure_text(&error), "ID_MEMCARD_REPLAY_LOAD_FAILED");
                }
                Err(_) => panic!("save {i} panicked"),
            }
        }
        println!("fuzzed saves: {ok} loaded, {failed} refused with the retail popup, 0 panics");
        assert_eq!(ok + failed, 10_000);
    }
}
