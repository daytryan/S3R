//! The title's own files, served to the audio game side run as original code
//! (`skate_audio_core::game::Files`): loose files under the staged stock data or the disc, members
//! of the EB archives the title names (`archive.big|member`) or mounts, and the PCM of every sound
//! bank it loads.

use std::path::{Path, PathBuf};

use skate_audio_formats::eb;

/// The title's files under `roots` (in order), by the path the title asks for (either slash, any
/// case for archive members). `archive.big|member` names a member of an EB archive; a bare name
/// that is no file is looked up in `data/audio/audiofiles.big` (the title mounts it).
pub fn files(roots: Vec<PathBuf>) -> skate_audio_core::game::Files {
    use std::collections::HashMap;
    type Index = HashMap<String, (u64, usize)>;
    let archives: std::sync::Arc<std::sync::Mutex<HashMap<PathBuf, Index>>> = Default::default();
    let locate = move |path: &str| -> Option<PathBuf> {
        roots.iter().map(|r| r.join(path)).find(|p| p.is_file())
    };
    // Where `member` sits in the archive `file`: its directory is indexed once, from the head
    // ([`archive_head`]), and looked up in place.
    let index = {
        let archives = archives.clone();
        move |file: &Path, member: &str| -> Option<(u64, usize)> {
            let mut archives = archives.lock().unwrap();
            if !archives.contains_key(file) {
                let parsed = eb::Archive::parse(&archive_head(file)?).ok()?;
                let i: Index = parsed
                    .entries
                    .iter()
                    .filter(|e| !e.is_compressed())
                    .filter_map(|e| {
                        let r = e.range();
                        Some((e.name.clone()?.to_ascii_lowercase(), (r.start as u64, r.len())))
                    })
                    .collect();
                archives.insert(file.to_owned(), i);
            }
            archives.get(file)?.get(member).copied()
        }
    };
    // (file, offset, length) of what a title path names.
    let resolve = std::sync::Arc::new(move |path: &str| -> Option<(PathBuf, u64, usize)> {
        let path = path.replace('\\', "/");
        if let Some((archive, member)) = path.split_once('|') {
            let file = locate(archive)?;
            let (offset, len) = index(&file, &member.to_ascii_lowercase())?;
            return Some((file, offset, len));
        }
        if let Some(file) = locate(&path) {
            let len = std::fs::metadata(&file).ok()?.len() as usize;
            return Some((file, 0, len));
        }
        // A bare name: a member of an archive the title mounts.
        let member = path.rsplit('/').next().unwrap_or(&path).to_ascii_lowercase();
        MOUNTED.iter().find_map(|archive| {
            let file = locate(archive)?;
            let (offset, len) = index(&file, &member)?;
            Some((file, offset, len))
        })
    });
    let whole = resolve.clone();
    skate_audio_core::game::Files {
        open: Vec::new(),
        read: Box::new(move |path: &str| {
            let (file, offset, len) = whole(path)?;
            read_range(&file, offset, len)
        }),
        read_at: Box::new(move |path: &str, at: u64, want: usize| {
            let (file, offset, len) = resolve(path)?;
            let at = at.min(len as u64);
            read_range(&file, offset + at, want.min(len - at as usize))
        }),
        loaded: Vec::new(),
        decode: Box::new(|name: &str, bytes: &[u8]| {
            let cache = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
            shared_decode(name, bytes, |bytes| super::catalog::decode_title_bank(name, bytes, cache.as_deref()))
                .unwrap_or_else(|e| {
                    eprintln!("SKATE_PLAYER_AUDIO title bank {name} not decoded: {e}");
                    Vec::new()
                })
        }),
        banks: Vec::new(),
    }
}

type Decoded = Vec<(u32, skate_audio_core::pcm::CachedPcm)>;
type Shared = std::collections::HashMap<(String, usize, u64), Decoded>;
/// Every bank decoded in this process, while some runtime still holds its PCM.
static SHARED: std::sync::Mutex<Option<Shared>> = std::sync::Mutex::new(None);

/// `decode` of a bank, shared between the runtimes in this process (not retail, by request (owner 10-18)): the
/// replay export boots its own runtime, which loaded every bank the live game already holds and decoded it again
/// (`.grain` through ffmpeg each time), a second copy of all the PCM in memory. A bank with the same name and
/// bytes takes the PCM another runtime still holds (each runtime keeps its installed PCM, `device.sources`);
/// entries no runtime holds any more go at the next decode.
/// ponytail: the lock is held through a decode, so a bank both runtimes want decodes once; the two runtimes' other
/// decodes wait behind it.
fn shared_decode(
    name: &str,
    bytes: &[u8],
    decode: impl FnOnce(&[u8]) -> Result<Decoded, super::Error>,
) -> Result<Decoded, super::Error> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let key = (name.to_ascii_lowercase(), bytes.len(), hasher.finish());
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    let shared = shared.get_or_insert_with(Default::default);
    shared.retain(|_, pcm| pcm.iter().any(|(_, p)| p.source.is_shared()));
    if let Some(pcm) = shared.get(&key) {
        return Ok(pcm.clone());
    }
    let pcm = decode(bytes)?;
    shared.insert(key, pcm.clone());
    Ok(pcm)
}

/// `len` bytes of `file` from `offset` (fewer at its end). ponytail: opened per read — the title
/// reads per bank load, not per block; keep handles if that ever changes.
fn read_range(file: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(file).ok()?;
    f.seek(std::io::SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::with_capacity(len);
    f.take(len as u64).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// An EB archive's head through its directory and name table: all [`eb::Archive::parse`] needs to
/// index it. Indexing read the whole archive (99 MB for audiofiles.big), twice a boot.
fn archive_head(file: &Path) -> Option<Vec<u8>> {
    let head = read_range(file, 0, eb::HEADER_SIZE)?;
    let word = |at: usize| Some(u32::from_be_bytes(head.get(at..at + 4)?.try_into().ok()?) as usize);
    let directory = eb::HEADER_SIZE + word(4)? * eb::ENTRY_SIZE;
    read_range(file, 0, directory.max(word(0x0C)? + word(0x10)?))
}

/// Skate 1's and 2's traffic banks (skate2port/old_traffic.py), each under the name of the Skate 3 bank whose slot it
/// takes (`sub_82487168`: the engine banks of table 0x8302D408 by engine type, Traffic_Horn, Traffic_Skid,
/// car_alarms), by city.
pub const TRAFFIC_BANKS: [(&str, &str); 2] =
    [(SKATE1_MAP, "private/traffic/skate1/audio"), (SKATE2_MAP, "private/traffic/skate2/audio")];

/// The traffic banks `map` plays: its game's folder on Skate 1's and 2's cities; None, Skate 3's own.
pub fn traffic_banks(assets: &Path, map: &str) -> Option<PathBuf> {
    TRAFFIC_BANKS.iter().find(|(city, _)| *city == map).map(|(_, dir)| assets.join(dir))
}

/// The folder whose traffic banks [`with_traffic`] serves (None: the title's own).
pub type TrafficSet = std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>;

/// `files` with the traffic banks of `set`'s folder in place of the title's, and the title's TOC (the head of
/// audiofiles.big, by whose sizes `sub_82487168` lays the banks back to back in the root's pool) giving each such bank
/// its largest file's size across the folders, so whichever game's set is loaded fits where the title puts it.
pub fn with_traffic(
    mut files: skate_audio_core::game::Files,
    assets: &Path,
    set: TrafficSet,
) -> skate_audio_core::game::Files {
    let mut largest = std::collections::HashMap::<String, u32>::new();
    for (_, dir) in TRAFFIC_BANKS {
        for entry in std::fs::read_dir(assets.join(dir)).into_iter().flatten().flatten() {
            let size = entry.metadata().map_or(0, |m| m.len() as u32);
            let most = largest.entry(entry.file_name().to_string_lossy().to_ascii_lowercase()).or_default();
            *most = (*most).max(size);
        }
    }
    let (mut read, mut read_at) = (files.read, files.read_at);
    let serve = move |path: &str| -> Option<PathBuf> {
        let dir = set.lock().ok()?.clone()?;
        let file = dir.join(path.replace('\\', "/").rsplit('/').next()?);
        file.is_file().then_some(file)
    };
    files.read = Box::new(move |path: &str| match serve(path) {
        Some(file) => std::fs::read(file).ok(),
        None => read(path),
    });
    files.read_at = Box::new(move |path: &str, at: u64, want: usize| {
        let mut bytes = read_at(path, at, want)?;
        if at == 0 && path.eq_ignore_ascii_case(MOUNTED[0]) {
            grow_toc(&mut bytes, &largest);
        }
        Some(bytes)
    });
    files
}

/// Raise each `largest` member's size in an EB directory to that size (never lower it).
fn grow_toc(head: &mut [u8], largest: &std::collections::HashMap<String, u32>) {
    let Ok(archive) = eb::Archive::parse(head) else { return };
    for (i, entry) in archive.entries.iter().enumerate() {
        let name = entry.name.as_deref().map(|n| n.rsplit('/').next().unwrap_or(n).to_ascii_lowercase());
        let Some(&size) = name.and_then(|n| largest.get(&n)) else { continue };
        // The TOC lookup sub_82A823E8 hands back the size (+8), or the compressed size (+4) when that is 0.
        let at = eb::HEADER_SIZE + eb::ENTRY_SIZE * i + if entry.uncompressed_size != 0 { 8 } else { 4 };
        let Some(field) = head.get_mut(at..at + 4) else { continue };
        let old = u32::from_be_bytes([field[0], field[1], field[2], field[3]]);
        field.copy_from_slice(&old.max(size).to_be_bytes());
    }
}

/// Decode, in the background, the PCM of every sound bank `world` names (the audio volumes' bank
/// field `BE88128A30BE926E`: the emitters' loops and one-shots), through the title's own file layer
/// and decode ([`files`]), so the cache holds them before the title loads one mid-ride: decoded on
/// the audio worker, a first-time bank stalled the output for its whole decode.
pub fn prewarm(assets: &Path, world: &super::world::WorldAudio) {
    let Some(volumes) = world.classes.get("audio_volume") else { return };
    let mut names: Vec<String> = volumes
        .order
        .iter()
        .filter_map(|key| volumes.field(*key, "Hash_BE88128A30BE926E"))
        .map(|f| f.data.clone())
        .filter(|n| n.ends_with(".abk") || n.ends_with(".bnk"))
        .collect();
    names.sort();
    names.dedup();
    let roots = roots(assets);
    let spawned = std::thread::Builder::new().name("audio-prewarm".into()).spawn(move || {
        let mut files = files(roots);
        let cache = pcm_cache();
        for name in names {
            if let Some(bytes) = (files.read)(&format!("data/audio/{name}")) {
                // A bank already decoded is only marked read: reading its decode back to drop it
                // cost every boot ~856 MB of disk reads for University's 291 banks.
                if cache.as_deref().is_some_and(|c| super::catalog::title_bank_cached(&name, &bytes, c)) {
                    continue;
                }
                // Straight to the disk cache, not through `files.decode`: that holds the shared decode lock for the
                // whole decode, and this loop retook it bank after bank, so a fresh cache kept the audio boot's own
                // banks waiting for all ~290 (user 11-02: no sound but music for the first ~5 minutes).
                let _ = super::catalog::decode_title_bank(&name, &bytes, cache.as_deref());
            }
        }
        // Once a session, after this world's banks are marked read.
        static PRUNED: std::sync::Once = std::sync::Once::new();
        if let Some(cache) = cache {
            PRUNED.call_once(|| {
                let freed = super::catalog::prune_cache(&cache, super::catalog::CACHE_IDLE);
                if freed > 0 {
                    eprintln!("SKATE_PLAYER_AUDIO pcm cache pruned {} MB", freed >> 20);
                }
            });
        }
    });
    if let Ok(handle) = spawned {
        PREWARMS.lock().unwrap_or_else(|e| e.into_inner()).push(handle);
    }
}

/// The [`prewarm`] threads started in this process, for [`wait_prewarm`].
static PREWARMS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Until every [`prewarm`] started so far has finished (the build's `--warm-audio`, which exits after it).
pub fn wait_prewarm() {
    let handles = std::mem::take(&mut *PREWARMS.lock().unwrap_or_else(|e| e.into_inner()));
    for handle in handles {
        let _ = handle.join();
    }
}

/// The archives the title mounts, whose members it names bare.
const MOUNTED: [&str; 4] = [
    "data/audio/audiofiles.big",
    "data/audio/grains.big",
    "data/audio/wheels.big",
    "data/audio/ambienceresident.big",
];

/// The staged stock files, then the disc the installation was made from (`installation.json`'s
/// `source`, the disc's `default.xex`), then any disc folder beside the installation.
pub fn roots(assets: &Path) -> Vec<PathBuf> {
    let mut roots = vec![assets.join("private/stock")];
    if let Some(disc) = std::env::var_os("SKATE_DISC") {
        roots.push(PathBuf::from(disc));
    }
    // <install>/data/installations/<id>/assets
    let data = assets.parent().and_then(Path::parent).and_then(Path::parent);
    if let Some(source) = data
        .and_then(|d| {
            // Only `source`: the 16-23 MB of receipts beside it are skipped, not built into a DOM.
            #[derive(serde::Deserialize)]
            struct Marker {
                source: Option<String>,
            }
            // installation.json.bak: atomic_json's previous marker, for one a power loss cut short
            ["installation.json", "installation.json.bak"]
                .iter()
                .find_map(|name| serde_json::from_slice::<Marker>(&std::fs::read(d.join(name)).ok()?).ok())
        })
        .and_then(|m| m.source.map(PathBuf::from))
        .and_then(|xex| data.map(|d| d.join(xex)))  // launcher installs store it relative to data\; join keeps an absolute one
        .and_then(|xex| xex.parent().map(Path::to_path_buf))
    {
        roots.push(source);
    }
    if let Some(install) = data.and_then(Path::parent) {
        if let Ok(entries) = std::fs::read_dir(install) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if dir.join("data/audio/ambience.big").is_file() {
                    roots.push(dir);
                }
            }
        }
    }
    roots
}

/// Skate 2's ambience beds under the assets, loose (`NN_<name>.snr` header, `NN_<name>.sns` blocks),
/// and its painter and rows as `world-audio.json` (skate2port/old_ambience.py): the same stream type as Skate 3's.
pub const SKATE2_BEDS: &str = "private/skate2-audio";

/// `23_Press_start_screen`, the bed of `aud_wp_ambiences` 0x5EADF1818A721AF9 (field 4042D2F96C4E4B8C = 23).
pub const PRESS_START_BED: u32 = 23;

/// Marks a Skate 2 bed in [`Beds`]: its PCM and header sit in the Skate 3 slot of that number.
const SKATE2: u32 = 1 << 31;

/// Skate 1's ambience beds, painter and rows, as [`SKATE2_BEDS`] holds Skate 2's.
pub const SKATE1_BEDS: &str = "private/skate1-audio";

/// Marks a Skate 1 bed in [`Beds`], as [`SKATE2`] does a Skate 2 one.
const SKATE1: u32 = 1 << 30;

/// The converted Skate 1 and Skate 2 cities' names (their districts in their `world-audio.json`).
pub const SKATE1_MAP: &str = "Skate_1_San_Vanelona";
pub const SKATE2_MAP: &str = "New_San_Vanelona";

/// The bed and volume fields of an `aud_wp_ambiences` row.
const BED: &str = "Hash_4042D2F96C4E4B8C";
const VOLUME: &str = "volume";

fn field_u32(class: &super::world::Class, key: u64, field: &str) -> Option<u32> {
    class.field(key, field).and_then(|f| u32::from_str_radix(&f.data, 16).ok())
}

/// A converted Skate 1 or Skate 2 city: its world audio plays that game's ambience painter and beds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldGame {
    Skate1,
    Skate2,
}

impl OldGame {
    /// The game whose city `map` is.
    pub fn of(map: &str) -> Option<Self> {
        match map {
            SKATE1_MAP => Some(Self::Skate1),
            SKATE2_MAP => Some(Self::Skate2),
            _ => None,
        }
    }

    /// Its tag on a bed in [`Beds`], its folder under the assets and its city's name.
    fn parts(self) -> (u32, &'static str, &'static str) {
        match self {
            Self::Skate1 => (SKATE1, SKATE1_BEDS, SKATE1_MAP),
            Self::Skate2 => (SKATE2, SKATE2_BEDS, SKATE2_MAP),
        }
    }
}

/// The `audio_ambience` painter key that plays slot N: the Skate 3 `aud_wp_ambiences` row whose bed
/// is N. Skate 3 has one row per bed.
fn bed_key(world: &super::world::WorldAudio, bed: u32) -> Option<u64> {
    let class = world.classes.get("aud_wp_ambiences")?;
    class.order.iter().copied().find(|key| field_u32(class, *key, BED) == Some(bed))
}

/// A bed's resident form, as the title's SndPlayer1 plays it: the `.snr` header with stream type 0
/// (and without the streamed loop's block offset), then the `.sns` block chain.
fn resident_stream(head: &[u8], blocks: &[u8]) -> Vec<u8> {
    let h2 = u32::from_be_bytes(head[4..8].try_into().unwrap());
    let looping = (h2 >> 29) & 1 != 0;
    let mut combined = head[..4].to_vec();
    combined.extend_from_slice(&(h2 & 0x3FFF_FFFF).to_be_bytes());
    if looping {
        combined.extend_from_slice(&head[8..12]);
    }
    combined.extend_from_slice(blocks);
    combined
}

/// `data/audio/<name>` under the title's roots.
fn audio_path(roots: &[PathBuf], name: &str) -> Result<PathBuf, String> {
    roots
        .iter()
        .map(|r| r.join("data/audio").join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{name} not found"))
}

/// `data/audio/<name>` under the title's roots, read whole.
pub(super) fn read_audio(roots: &[PathBuf], name: &str) -> Result<Vec<u8>, String> {
    audio_path(roots, name).and_then(|p| std::fs::read(&p).map_err(|e| e.to_string()))
}

/// Skate 3's slot for bed N: its `NN_*.snr` header in the resident archive.
pub(super) fn slot(index: &eb::Archive, bed: u32) -> Option<&eb::Entry> {
    let prefix = format!("{bed:02}_");
    index
        .entries
        .iter()
        .find(|e| e.name.as_deref().is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".snr")))
}

/// Where the title loaded the resident ambience archive (`Files::loaded`).
pub(super) fn resident_at(runtime: &skate_audio_core::authored::AuthoredRuntime) -> Option<u32> {
    runtime
        .game_files()
        .into_iter()
        .find(|(p, at)| *at != 0 && p.to_ascii_lowercase().ends_with("ambienceresident.big"))
        .map(|(_, at)| at)
}

fn pcm_cache() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("Skate3RustEngine/audio-pcm-cache"))
}

/// A Skate 1 or 2 bed (tagged) from the loader thread: its slot's range in the resident archive, its header, its PCM.
type Loaded = (u32, Result<(std::ops::Range<usize>, Vec<u8>, skate_audio_core::pcm::CachedPcm), String>);

/// The ambience beds whose PCM is installed where the title's SndPlayer1 plays them (SFXObj_Ambience's
/// streamed `NN_*.snr` in the resident archive the title loads whole, their blocks in `ambience.big`'s
/// `NN_*.sns`; each decoded whole through the PCM cache and installed at its `.snr` header's address
/// in the title's loaded copy), and the loader of Skate 1's and 2's beds for their converted maps: those
/// go in the Skate 3 slot of the same number with their own header written over the slot's.
#[derive(Default)]
pub struct Beds {
    /// By bed number; `| SKATE1` / `| SKATE2` where Skate 1's / 2's bed is in the slot.
    installed: std::collections::HashSet<u32>,
    /// Tagged beds asked of the loader and not installed (a bed that failed stays here: silent,
    /// not asked again).
    pending: std::collections::HashSet<u32>,
    loader: Option<(std::sync::mpsc::Sender<(u32, f32)>, std::sync::mpsc::Receiver<Loaded>)>,
    /// Skate 1's and 2's painters and rows ([`OldGame`] order), each read on its city's first frame
    /// (`None` inside: not exported).
    painters: [Option<Option<super::world::WorldAudio>>; 2],
    /// Skate 3 beds the loader decoded for the map being loaded ([`Beds::prefetch`]), installed by [`Beds::enter`].
    ready: std::collections::HashMap<u32, skate_audio_core::pcm::CachedPcm>,
    /// Skate 1 / 2 beds that arrived while a Skate 3 map was entered, installed on the old city's next frame.
    held: Vec<Loaded>,
}

/// A map's district's Skate 3 beds: those its `audio_ambience` painter palette names (`aud_wp_ambiences` field
/// `4042D2F96C4E4B8C`); none for a map without a district (an old game's city, most parks).
fn district_beds(world: &super::world::WorldAudio, map: &str) -> std::collections::BTreeSet<u32> {
    let mut beds = std::collections::BTreeSet::new();
    if let (Some(district), Some(class)) = (world.district(map), world.classes.get("aud_wp_ambiences")) {
        for tile in district.layer("audio_ambience") {
            beds.extend(tile.palette.iter().filter_map(|key| field_u32(class, *key, BED)));
        }
    }
    beds
}

/// `SKATE_AUDIO_KEEP_BEDS=1`: every bed stays once decoded and none is decoded ahead (before 2026-10-12).
fn keep_beds() -> bool {
    std::env::var_os("SKATE_AUDIO_KEEP_BEDS").is_some_and(|v| v != "0")
}

/// `ambience.big` and its index (its head: the archive is 137 MB).
fn ambience_streams(roots: &[PathBuf]) -> Result<(PathBuf, eb::Archive), String> {
    let streams = audio_path(roots, "ambience.big")?;
    let index = archive_head(&streams)
        .ok_or_else(|| "ambience.big unreadable".to_string())
        .and_then(|head| eb::Archive::parse(&head).map_err(|e| e.to_string()))?;
    Ok((streams, index))
}

/// The PCM of the Skate 3 bed whose resident `.snr` header is `header`: with its `.sns` blocks from `ambience.big`,
/// decoded whole through the PCM cache. None where the title has no blocks for it (skipped, as ever).
fn skate3_bed(
    resident: &[u8],
    header: &eb::Entry,
    (streams, index): &(PathBuf, eb::Archive),
    cache: Option<&Path>,
) -> Option<Result<skate_audio_core::pcm::CachedPcm, String>> {
    let name = header.name.clone().unwrap_or_default();
    let stem = name.trim_end_matches(".snr");
    let blocks = index
        .entries
        .iter()
        .find(|e| e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&format!("{stem}.sns"))))?;
    let range = blocks.range();
    let blocks = read_range(streams, range.start as u64, range.len())?;
    let combined = resident_stream(&resident[header.range()], &blocks);
    match super::catalog::decode_title_bank(&name, &combined, cache) {
        Ok(pcm) => pcm.into_iter().next().map(|(_, pcm)| Ok(pcm)),
        Err(e) => Some(Err(format!("{name}: {e}"))),
    }
}

impl Beds {
    /// How many beds are installed.
    pub fn len(&self) -> usize {
        self.installed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.installed.is_empty()
    }

    /// On a map change. A Skate 3 map puts Skate 3's own headers back in the slots Skate 1 or 2 used
    /// and drops their PCM, then gets the beds its district's `audio_ambience` painter palette names
    /// (`aud_wp_ambiences` field `4042D2F96C4E4B8C`) that are not in yet; a Skate 1 or 2 map (`old`)
    /// gets none here ([`Beds::old_game_key`] loads each bed when it is first needed). Either drops the
    /// PCM of the Skate 3 beds it does not play but the title's ([`Beds::unplayed`]). Beds the loader
    /// decoded ahead ([`Beds::prefetch`]) are installed as they are, the rest decoded here.
    /// Returns how many beds this call added; a bed that fails is logged and skipped.
    pub fn enter(
        &mut self,
        runtime: &mut skate_audio_core::authored::AuthoredRuntime,
        world: &super::world::WorldAudio,
        assets: &Path,
        map: &str,
        old: bool,
    ) -> Result<usize, String> {
        let dropped: Vec<u32> = if old {
            Vec::new()
        } else {
            self.installed.iter().copied().filter(|b| b & (SKATE1 | SKATE2) != 0).collect()
        };
        let mut wanted = if old { Default::default() } else { district_beds(world, map) };
        // Retail streams its beds from the disc and holds none it does not play: the other districts' go
        // (0.3-0.6 GB each). A voice still on one (the old district's, fading out) plays its own copy to its end.
        let evicted = if keep_beds() { Vec::new() } else { self.unplayed(&wanted) };
        // The title screen's bed, which SFXObj_Ambience plays off the FE audio state rather than a painter key
        // (key selector 0x824D3E40, aud_wp_ambiences 0x5EADF1818A721AF9): no district names it.
        if !old {
            wanted.insert(PRESS_START_BED);
        }
        wanted.retain(|bed| !self.installed.contains(bed));
        if wanted.is_empty() && dropped.is_empty() && evicted.is_empty() {
            return Ok(0);
        }
        let Some(resident_at) = resident_at(runtime) else { return Ok(0) };
        let roots = roots(assets);
        let resident = read_audio(&roots, "ambienceresident.big")?;
        let index = eb::Archive::parse(&resident).map_err(|e| e.to_string())?;
        for bed in evicted {
            if let Some(header) = slot(&index, bed) {
                runtime.remove_pcm(resident_at + header.range().start as u32);
            }
            self.installed.remove(&bed);
        }
        for tag in dropped {
            if let Some(header) = slot(&index, tag & !(SKATE1 | SKATE2)) {
                let at = resident_at + header.range().start as u32;
                runtime.remove_pcm(at);
                if let Err(e) = runtime.guest_mut().set_span(at, &resident[header.range()]) {
                    eprintln!("SKATE_PLAYER_AUDIO ambience_bed_restore_failed bed={} error={e}", tag & !(SKATE1 | SKATE2));
                    continue;
                }
            }
            self.installed.remove(&tag);
        }
        // The loader's finished beds, then the wanted ones it is still decoding (sooner than decoding them here).
        self.arrivals(runtime, false);
        while wanted.iter().any(|bed| self.pending.contains(bed)) {
            let Some(Ok(loaded)) = self.loader.as_ref().map(|(_, done)| done.recv()) else { break };
            self.arrived(runtime, loaded, false);
        }
        let mut ready = std::mem::take(&mut self.ready);
        if wanted.is_empty() {
            return Ok(0);
        }
        // ambience.big is 137 MB: its directory and the wanted beds' blocks are read, not all of
        // it (on the audio worker, each map with a bed the loader did not decode ahead).
        let mut streams = None;
        let cache = pcm_cache();
        let mut added = 0;
        for bed in wanted {
            self.pending.remove(&bed);
            let Some(header) = slot(&index, bed) else { continue };
            let name = header.name.as_deref().unwrap_or_default();
            let pcm = match ready.remove(&bed) {
                Some(pcm) => pcm,
                None => {
                    let streams = match &streams {
                        Some(streams) => streams,
                        None => streams.insert(ambience_streams(&roots)?),
                    };
                    match skate3_bed(&resident, header, streams, cache.as_deref()) {
                        Some(Ok(pcm)) => pcm,
                        Some(Err(e)) => {
                            eprintln!("SKATE_PLAYER_AUDIO ambience_bed_failed {e}");
                            continue;
                        }
                        None => continue,
                    }
                }
            };
            match runtime.insert_pcm(resident_at + header.range().start as u32, pcm) {
                Ok(()) => {
                    self.installed.insert(bed);
                    added += 1;
                }
                Err(e) => eprintln!("SKATE_PLAYER_AUDIO ambience_bed_failed {name}: {e}"),
            }
        }
        Ok(added)
    }

    /// The installed Skate 3 beds a map whose district plays `district` does not: all but those and the title's
    /// ([`PRESS_START_BED`]). Skate 1 and 2 beds keep their own rules.
    fn unplayed(&self, district: &std::collections::BTreeSet<u32>) -> Vec<u32> {
        let mut beds: Vec<u32> = self
            .installed
            .iter()
            .copied()
            .filter(|bed| bed & (SKATE1 | SKATE2) == 0 && *bed != PRESS_START_BED && !district.contains(bed))
            .collect();
        beds.sort_unstable();
        beds
    }

    /// Starts decoding `map`'s district's Skate 3 beds on the loader thread while the map loads (the map transition
    /// names it), so that [`Beds::enter`] only installs them: a cold district is 0.3-0.6 GB of PCM-cache reads, which
    /// on the audio worker stalled the mix. Held until entered; a later prefetch keeps only its own map's.
    pub fn prefetch(&mut self, world: &super::world::WorldAudio, assets: &Path, map: &str) {
        if keep_beds() {
            return;
        }
        let mut beds = district_beds(world, map);
        // The title's bed, if Skate 1's or 2's took its slot.
        if OldGame::of(map).is_none() {
            beds.insert(PRESS_START_BED);
        }
        self.ready.retain(|bed, _| beds.contains(bed));
        for bed in beds {
            if self.installed.contains(&bed) || self.ready.contains_key(&bed) || !self.pending.insert(bed) {
                continue;
            }
            let (requests, _) = self.loader.get_or_insert_with(|| loader(assets.to_path_buf()));
            let _ = requests.send((bed, 1.));
        }
    }

    /// On Skate 1's or 2's city, each frame: that game's ambience painter at (x, z) (its row: bed and
    /// volume), as the Skate 3 row of that bed once the old bed is in its slot, and the gain on the
    /// ambience controller that turns the Skate 3 row's volume into the old row's; (0, 1) where unpainted.
    /// A bed is decoded the first time it is needed, on a loader thread (never on the audio worker: a cold
    /// decode takes seconds), and installed here when it arrives.
    // ponytail: the Skate 3 row's fades (Skate 1's are 3 s, 1-6 s on four rows; Skate 2's 1-3 s), and two
    // old rows on one bed (Skate 1's _alt next to its main, Skate 2's dt_courtyard/dt_parks) share a key:
    // no cross-fade between them, the gain steps. No neighbour prefetch: a bed's first area is silent
    // until it decodes (~7 s cold, a cache read after).
    pub fn old_game_key(
        &mut self,
        runtime: &mut skate_audio_core::authored::AuthoredRuntime,
        world: &super::world::WorldAudio,
        assets: &Path,
        game: OldGame,
        x: f32,
        z: f32,
    ) -> (u64, f32) {
        self.arrivals(runtime, true);
        let (_, dir, _) = game.parts();
        let painter = self.painters[game as usize].get_or_insert_with(|| {
            let path = assets.join(dir).join("world-audio.json");
            std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| super::world::WorldAudio::parse(&text))
                .inspect_err(|e| eprintln!("SKATE_PLAYER_AUDIO old_ambience_failed {}: {e}", path.display()))
                .ok()
        });
        let Some((tag, gain, boost)) = painter.as_ref().and_then(|painter| old_bed(painter, game, world, x, z)) else {
            return (0, 1.);
        };
        match self.old_key(world, assets, tag, boost) {
            0 => (0, 1.),
            key => (key, gain),
        }
    }

    /// Takes the beds the loader has finished ([`Beds::arrived`]); `old`: on Skate 1's or 2's city.
    fn arrivals(&mut self, runtime: &mut skate_audio_core::authored::AuthoredRuntime, old: bool) {
        let mut arrived: Vec<Loaded> = if old { std::mem::take(&mut self.held) } else { Vec::new() };
        arrived.extend(self.loader.as_ref().into_iter().flat_map(|(_, done)| done.try_iter()));
        for loaded in arrived {
            self.arrived(runtime, loaded, old);
        }
    }

    /// One bed from the loader: a Skate 3 one waits in `ready` for its map; a Skate 1 / 2 one is installed in its
    /// slot on its city (`old`), else held until the city's next frame.
    fn arrived(&mut self, runtime: &mut skate_audio_core::authored::AuthoredRuntime, (tag, result): Loaded, old: bool) {
        if tag & (SKATE1 | SKATE2) == 0 {
            self.pending.remove(&tag);
            match result {
                Ok((_, _, pcm)) => {
                    self.ready.insert(tag, pcm);
                }
                Err(e) => eprintln!("SKATE_PLAYER_AUDIO ambience_bed_prefetch_failed bed={tag} error={e}"),
            }
            return;
        }
        if !old {
            self.held.push((tag, result));
            return;
        }
        let installed = result.and_then(|(range, head, pcm)| {
            let at = resident_at(runtime).ok_or("resident archive not loaded")? + range.start as u32;
            runtime.insert_pcm(at, pcm).map_err(|e| e.to_string())?;
            runtime.guest_mut().set_span(at, &head).map_err(|e| e.to_string())
        });
        let bed = tag & !(SKATE1 | SKATE2);
        match installed {
            Ok(()) => {
                self.pending.remove(&tag);
                for other in [bed, bed | SKATE1, bed | SKATE2] {
                    self.installed.remove(&other);
                }
                self.installed.insert(tag);
                eprintln!("SKATE_PLAYER_AUDIO old_ambience_bed={bed} tag={tag:#010x} installed");
            }
            Err(e) => eprintln!("SKATE_PLAYER_AUDIO old_ambience_bed_failed bed={bed} tag={tag:#010x} error={e}"),
        }
    }

    /// The Skate 3 row key of a tagged Skate 1 / 2 bed once it is in its slot; else asks the loader
    /// for it (its PCM scaled by `boost`) and gives 0.
    fn old_key(&mut self, world: &super::world::WorldAudio, assets: &Path, tag: u32, boost: f32) -> u64 {
        if self.installed.contains(&tag) {
            return bed_key(world, tag & !(SKATE1 | SKATE2)).unwrap_or(0);
        }
        if self.pending.insert(tag) {
            let (requests, _) = self.loader.get_or_insert_with(|| loader(assets.to_path_buf()));
            let _ = requests.send((tag, boost));
        }
        0
    }
}

/// The old game's row under (x, z) as (its bed tagged, the ambience gain, the bed's PCM boost): the
/// Skate 3 row plays the bed at its own volume, so the PCM is boosted by the loudest old row on that bed
/// over the Skate 3 one (the controller gain can only cut) and the gain cuts it to this row's.
fn old_bed(
    painter: &super::world::WorldAudio,
    game: OldGame,
    world: &super::world::WorldAudio,
    x: f32,
    z: f32,
) -> Option<(u32, f32, f32)> {
    let (flag, _, map) = game.parts();
    let volume = |class: &super::world::Class, key| field_u32(class, key, VOLUME).map(f32::from_bits);
    let rows = painter.classes.get("aud_wp_ambiences")?;
    let row = super::world::sample(painter.district(map)?.layer("audio_ambience"), x, z).filter(|k| *k != 0)?;
    let bed = field_u32(rows, row, BED)?;
    let own = volume(world.classes.get("aud_wp_ambiences")?, bed_key(world, bed)?).filter(|v| *v > 0.)?;
    let loudest = rows.order.iter().filter(|k| field_u32(rows, **k, BED) == Some(bed)).filter_map(|k| volume(rows, *k));
    let boost = loudest.fold(own, f32::max) / own;
    Some((bed | flag, volume(rows, row)? / own / boost, boost))
}

/// The thread that decodes beds off the audio worker (through the PCM cache): Skate 1's and 2's as [`Beds::old_key`]
/// asks, Skate 3's as [`Beds::prefetch`] does.
fn loader(assets: PathBuf) -> (std::sync::mpsc::Sender<(u32, f32)>, std::sync::mpsc::Receiver<Loaded>) {
    let (requests, asked) = std::sync::mpsc::channel::<(u32, f32)>();
    let (loaded, done) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("old-ambience".into()).spawn(move || {
        let resident = read_audio(&roots(&assets), "ambienceresident.big");
        let cache = pcm_cache();
        let mut streams = None;
        for (tag, boost) in asked {
            // A Skate 3 bed for the map being loaded (Beds::prefetch): what Beds::enter would decode.
            if tag & (SKATE1 | SKATE2) == 0 {
                let result = (|| {
                    let resident = resident.as_ref().map_err(Clone::clone)?;
                    let index = eb::Archive::parse(resident).map_err(|e| e.to_string())?;
                    let header = slot(&index, tag).ok_or_else(|| "no slot".to_string())?;
                    let streams = match &streams {
                        Some(streams) => streams,
                        None => streams.insert(ambience_streams(&roots(&assets))?),
                    };
                    let pcm = skate3_bed(resident, header, streams, cache.as_deref())
                        .unwrap_or_else(|| Err("no blocks".into()))?;
                    Ok((header.range(), Vec::new(), pcm))
                })();
                if loaded.send((tag, result)).is_err() {
                    break;
                }
                continue;
            }
            let (dir, bed) = if tag & SKATE1 != 0 { (SKATE1_BEDS, tag & !SKATE1) } else { (SKATE2_BEDS, tag & !SKATE2) };
            let result = (|| {
                let resident = resident.as_ref().map_err(Clone::clone)?;
                let index = eb::Archive::parse(resident).map_err(|e| e.to_string())?;
                let header = slot(&index, bed).ok_or_else(|| "no Skate 3 slot".to_string())?;
                let (name, head, blocks) =
                    bed_files(&assets.join(dir), &format!("{bed:02}_")).ok_or_else(|| format!("not exported to {dir}"))?;
                if head.len() != header.range().len() {
                    return Err(format!("{name}: header {} bytes, slot {}", head.len(), header.range().len()));
                }
                let pcm = super::catalog::decode_title_bank(&name, &resident_stream(&head, &blocks), cache.as_deref())
                    .map_err(|e| format!("{name}: {e}"))?;
                let (_, pcm) = pcm.into_iter().next().ok_or_else(|| format!("{name}: no stream"))?;
                Ok((header.range(), head, boosted(pcm, boost)?))
            })();
            if loaded.send((tag, result)).is_err() {
                break;
            }
        }
    });
    (requests, done)
}

/// `pcm` with every sample scaled by `boost` (saturating), its loop kept.
fn boosted(pcm: skate_audio_core::pcm::CachedPcm, boost: f32) -> Result<skate_audio_core::pcm::CachedPcm, String> {
    if boost == 1. {
        return Ok(pcm);
    }
    let source = &pcm.source;
    let samples: std::sync::Arc<[i16]> =
        source.samples().iter().map(|s| (f32::from(*s) * boost).round().clamp(-32768., 32767.) as i16).collect();
    let mut scaled = skate_audio_core::pcm::PcmSource::new(samples, source.channels()).map_err(|e| e.to_string())?;
    if let Some((start, end)) = source.loop_range() {
        scaled = scaled.with_loop(start, end).map_err(|e| e.to_string())?;
    }
    Ok(skate_audio_core::pcm::CachedPcm { source: scaled, sample_rate: pcm.sample_rate })
}

/// A Skate 1 or 2 bed's `.snr` name, header and `.sns` blocks in `dir`, by its `NN_` prefix.
fn bed_files(dir: &Path, prefix: &str) -> Option<(String, Vec<u8>, Vec<u8>)> {
    let name = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with(prefix) && n.ends_with(".snr"))?;
    let head = std::fs::read(dir.join(&name)).ok()?;
    let blocks = std::fs::read(dir.join(name.replace(".snr", ".sns"))).ok()?;
    (head.len() >= 12).then_some((name, head, blocks))
}

#[cfg(test)]
mod tests {
    /// Owner 10-18: a second runtime loading a bank the first still holds gets the same PCM (one decode, one copy in
    /// memory), grains included; once no runtime holds it, the next load decodes again.
    #[test]
    fn a_bank_another_runtime_holds_is_shared_not_decoded_again() {
        use skate_audio_core::pcm::{CachedPcm, PcmSource};
        let decodes = std::cell::Cell::new(0);
        let decode = |_: &[u8]| {
            decodes.set(decodes.get() + 1);
            let source = PcmSource::new(std::sync::Arc::from(vec![1i16, 2, 3, 4]), 2).unwrap();
            Ok(vec![(0x40, CachedPcm { source, sample_rate: 48_000 })])
        };
        for name in ["s3r-shared-test.abk", "s3r-shared-test.grain"] {
            decodes.set(0);
            // The live game: its runtime keeps what it installed (`device.sources`) for as long as the bank is loaded.
            let live = super::shared_decode(name, b"bank bytes", decode).unwrap();
            let export = super::shared_decode(name, b"bank bytes", decode).unwrap();
            assert_eq!(decodes.get(), 1, "{name}: decoded once");
            assert!(std::sync::Arc::ptr_eq(live[0].1.source.samples(), export[0].1.source.samples()), "{name}");
            // Other bytes under the same name are another bank.
            let other = super::shared_decode(name, b"other bytes", decode).unwrap();
            assert_eq!(decodes.get(), 2);
            drop((live, export, other));
            super::shared_decode(name, b"bank bytes", decode).unwrap();
            assert_eq!(decodes.get(), 3, "{name}: nobody held it, so it was let go");
        }
    }

    /// The TOC entry of a bank another game's set replaces grows to the largest file, never shrinks.
    #[test]
    fn the_toc_makes_room_for_the_largest_traffic_bank() {
        let mut head = vec![0u8; 0x80];
        head[..4].copy_from_slice(b"EB\0\x03");
        head[4..8].copy_from_slice(&2u32.to_be_bytes());
        head[8..12].copy_from_slice(&0x400u32.to_be_bytes());
        head[12..16].copy_from_slice(&0x50u32.to_be_bytes());
        head[0x38..0x3C].copy_from_slice(&1000u32.to_be_bytes());
        head[0x48..0x4C].copy_from_slice(&5000u32.to_be_bytes());
        head[0x50..0x6A].copy_from_slice(b"\0\0Traffic_Horn.abk\0other\0\0");
        let largest = [("traffic_horn.abk".to_owned(), 3000), ("other".to_owned(), 10)].into_iter().collect();
        super::grow_toc(&mut head, &largest);
        assert_eq!(head[0x38..0x3C], 3000u32.to_be_bytes());
        assert_eq!(head[0x48..0x4C], 5000u32.to_be_bytes());
        assert_eq!(super::traffic_banks(std::path::Path::new("a"), "New_San_Vanelona"), Some("a/private/traffic/skate2/audio".into()));
        assert_eq!(super::traffic_banks(std::path::Path::new("a"), "University"), None);
    }

    /// An archive is indexed from its head (directory and names) as from the whole file.
    #[test]
    fn an_archive_indexes_from_its_head() {
        let mut file = vec![0u8; 0x500 + 2000];
        file[..4].copy_from_slice(b"EB\0\x03");
        file[4..8].copy_from_slice(&2u32.to_be_bytes());
        file[8..12].copy_from_slice(&0x400u32.to_be_bytes());
        file[12..16].copy_from_slice(&0x50u32.to_be_bytes());
        file[16..20].copy_from_slice(&26u32.to_be_bytes());
        file[0x30..0x34].copy_from_slice(&0x10u32.to_be_bytes());
        file[0x38..0x3C].copy_from_slice(&1000u32.to_be_bytes());
        file[0x40..0x44].copy_from_slice(&0x50u32.to_be_bytes());
        file[0x48..0x4C].copy_from_slice(&2000u32.to_be_bytes());
        file[0x50..0x6A].copy_from_slice(b"\0\0Traffic_Horn.abk\0other\0\0");
        let path = std::env::temp_dir().join("s3r-title-archive-head.big");
        std::fs::write(&path, &file).unwrap();
        let head = super::archive_head(&path).unwrap();
        assert_eq!(head.len(), 0x6A);
        let whole = super::eb::Archive::parse(&file).unwrap().entries;
        assert_eq!(super::eb::Archive::parse(&head).unwrap().entries, whole);
        assert_eq!(whole[1].name.as_deref(), Some("other"));
        assert_eq!(super::read_range(&path, 0x500, 4000).unwrap().len(), 2000);
    }

    /// A launcher install stores the disc relative to data\, so the game folder can be moved or renamed.
    #[test]
    fn the_disc_root_follows_a_moved_install() {
        let data = std::env::temp_dir().join(format!("s3r-title-roots-{}", std::process::id()));
        let assets = data.join("installations/abc/assets");
        std::fs::create_dir_all(&assets).unwrap();
        for (source, want) in [("work/discs/s3-disc/default.xex", data.join("work/discs/s3-disc")),
                               ("D:/Discs/Skate 3/default.xex", std::path::PathBuf::from("D:/Discs/Skate 3"))] {
            std::fs::write(data.join("installation.json"), serde_json::json!({"source": source}).to_string()).unwrap();
            assert!(super::roots(&assets).contains(&want), "{source}");
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn only_the_skate1_and_skate2_cities_play_old_ambience() {
        assert_eq!(super::OldGame::of("Skate_1_San_Vanelona"), Some(super::OldGame::Skate1));
        assert_eq!(super::OldGame::of("New_San_Vanelona"), Some(super::OldGame::Skate2));
        // Skate 2's parks are their own districts, not in the city's painter.
        assert_eq!(super::OldGame::of("Skate_2_Maloof"), None);
        assert_eq!(super::OldGame::of("University"), None);
    }

    /// An old game's painter picks its row; the bed plays from the Skate 3 slot of its number, its PCM boosted by
    /// the loudest old row on that bed over the Skate 3 row, the gain bringing each row to its own volume.
    #[test]
    fn old_rows_play_skate3_slots_at_their_own_volume() {
        let row = |key: &str, bed: u32, volume: f32| {
            serde_json::json!({"key": key, "parent": "", "layout": "", "fields": {
                "Hash_4042D2F96C4E4B8C": {"type": "EA::Reflection::Int32", "data": format!("{bed:08X}")},
                "volume": {"type": "EA::Reflection::Float", "data": format!("{:08X}", volume.to_bits())}}})
        };
        let world = |rows: Vec<serde_json::Value>, district: &str, tiles: serde_json::Value| {
            let text = serde_json::json!({"classes": {"aud_wp_ambiences": rows},
                "districts": {(format!("DIST_{district}")): {"layers": {"audio_ambience": tiles}}}});
            super::super::world::WorldAudio::parse(&text.to_string()).unwrap()
        };
        let skate3 = world(vec![row("00000000000000B1", 1, 0.8), row("00000000000000B2", 2, 0.75)], "University", serde_json::json!([]));
        // Root {1, 2, 3, 3}; leaves 0 (main), 1 (alt), 2 (unpainted).
        let nodes = ["00010002000300030000", "ffff0000000000000000", "ffff0000000000000001", "ffff0000000000000002"].concat();
        let tiles = serde_json::json!([{"cx": 0.0, "cz": 0.0, "hx": 64.0, "hz": 64.0, "nodes": nodes,
                                        "palette": ["000000000000000A", "000000000000000B", ""], "high": false}]);
        let rows = || vec![row("000000000000000A", 1, 1.0), row("000000000000000B", 1, 0.7)];
        let close = |(tag, gain, boost): (u32, f32, f32), want: (u32, f32, f32)| {
            assert_eq!(tag, want.0);
            assert!((gain - want.1).abs() < 1e-6 && (boost - want.2).abs() < 1e-6, "{gain} {boost} vs {want:?}");
        };
        let skate1 = world(rows(), "Skate_1_San_Vanelona", tiles.clone());
        let at = |painter, game, x, z| super::old_bed(painter, game, &skate3, x, z);
        close(at(&skate1, super::OldGame::Skate1, -10., -10.).unwrap(), (1 | super::SKATE1, 1.0, 1.25));
        close(at(&skate1, super::OldGame::Skate1, -10., 10.).unwrap(), (1 | super::SKATE1, 0.7, 1.25));
        assert_eq!(at(&skate1, super::OldGame::Skate1, 10., 0.), None, "unpainted");
        assert_eq!(at(&skate1, super::OldGame::Skate1, 100., 0.), None, "outside every tile");
        let skate2 = world(rows(), "New_San_Vanelona", tiles);
        close(at(&skate2, super::OldGame::Skate2, -10., 10.).unwrap(), (1 | super::SKATE2, 0.7, 1.25));
        assert_eq!(at(&skate1, super::OldGame::Skate2, -10., 10.), None, "another city's painter");
    }

    /// A map keeps its district's Skate 3 beds and the title's (23); every other Skate 3 bed's PCM goes. Old games'
    /// beds (tagged) are left to their own rules.
    #[test]
    fn a_map_keeps_only_its_districts_beds() {
        let row = |key: &str, bed: u32| {
            serde_json::json!({"key": key, "parent": "", "layout": "", "fields": {
                "Hash_4042D2F96C4E4B8C": {"type": "EA::Reflection::Int32", "data": format!("{bed:08X}")}}})
        };
        let tile = |palette: &[&str]| {
            serde_json::json!([{"cx": 0.0, "cz": 0.0, "hx": 64.0, "hz": 64.0, "nodes": "ffff0000000000000000",
                                "palette": palette, "high": false}])
        };
        let text = serde_json::json!({
            "classes": {"aud_wp_ambiences": [row("000000000000000A", 1), row("000000000000000B", 2), row("000000000000000C", 3)]},
            "districts": {"DIST_A": {"layers": {"audio_ambience": tile(&["000000000000000A", "000000000000000B"])}},
                          "DIST_B": {"layers": {"audio_ambience": tile(&["000000000000000C", ""])}}}});
        let world = super::super::world::WorldAudio::parse(&text.to_string()).unwrap();
        assert_eq!(super::district_beds(&world, "A"), [1, 2].into());
        assert_eq!(super::district_beds(&world, "B"), [3].into());
        assert!(super::district_beds(&world, "Art_Gallery").is_empty());
        let mut beds = super::Beds::default();
        beds.installed = [1, 2, 3, super::PRESS_START_BED, 5 | super::SKATE2].into();
        assert_eq!(beds.unplayed(&super::district_beds(&world, "B")), vec![1, 2]);
        assert_eq!(beds.unplayed(&Default::default()), vec![1, 2, 3], "an old game's city or a park without beds");
    }

    #[test]
    #[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets (with skate2port/old_ambience.py run)"]
    fn a_skate1_bed_decodes_and_fits_its_skate3_slot() {
        let Some(assets) = std::env::var_os("SKATE_ASSETS").map(std::path::PathBuf::from) else { return };
        let (name, head, blocks) = super::bed_files(&assets.join(super::SKATE1_BEDS), "01_").expect("01 bed exported");
        let header = skate_audio_formats::eaac::Header::parse(&head, 0).unwrap();
        // Skate 1's beds: EA-XMA, 4 channels, 48 kHz, a streamed loop; the header as long as Skate 3's.
        assert_eq!((header.channels(), header.sample_rate, header.stream_type, head.len()), (4, 48_000, 1, 16), "{header:?}");
        let pcm = super::super::catalog::decode_title_bank(&name, &super::resident_stream(&head, &blocks), None).unwrap();
        let [(_, pcm)] = &pcm[..] else { panic!("{name}: {} streams", pcm.len()) };
        let loud = super::boosted(pcm.clone(), 1.25).unwrap();
        assert_eq!((loud.source.frames(), loud.source.loop_range()), (pcm.source.frames(), pcm.source.loop_range()));
        let seconds = pcm.source.frames() as f32 / 48_000.0;
        eprintln!("{name}: {seconds:.1}s, {} channels", header.channels());
        assert!(seconds > 30.0, "{name}: {seconds}s");
    }

    #[test]
    #[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets (with skate2port/old_ambience.py run)"]
    fn a_skate2_bed_decodes_as_skate3_beds_do() {
        let Some(assets) = std::env::var_os("SKATE_ASSETS").map(std::path::PathBuf::from) else { return };
        let (name, head, blocks) = super::bed_files(&assets.join(super::SKATE2_BEDS), "06_").expect("06_soho bed exported");
        let header = skate_audio_formats::eaac::Header::parse(&head, 0).unwrap();
        // Skate 3's beds: EA-XMA, 5 channels, 48 kHz, a streamed loop.
        assert_eq!((header.channels(), header.sample_rate, header.stream_type), (5, 48_000, 1), "{header:?}");
        let pcm = super::super::catalog::decode_title_bank(&name, &super::resident_stream(&head, &blocks), None).unwrap();
        let [(offset, pcm)] = &pcm[..] else { panic!("{name}: {} streams", pcm.len()) };
        assert_eq!((*offset, pcm.sample_rate), (0, 48_000));
        let seconds = pcm.source.frames() as f32 / 48_000.0;
        eprintln!("{name}: {seconds:.1}s, {} channels", header.channels());
        assert!(seconds > 60.0, "{name}: {seconds}s");
    }
}
