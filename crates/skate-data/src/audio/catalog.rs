//! Player bank loading and offline PCM preparation from the owner's retail archives.
//!
//! The whitelist is the object-to-bank attribution in `docs/audio-player-sounds-handoff.md`.
//! It includes every variant in each bank; the authored patch chooses what to play. Dialogue,
//! Hall of Meat voices, ambient emitters, and unproven dynamic-object routes are not installed.
//! Archive IO, external decoding, and the content-validated disk cache happen before the audio
//! runtime starts. Guest addresses are assigned by the runtime, never persisted in the cache.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skate_audio_core::pcm::{CachedPcm, PcmSource};
use skate_audio_core::{Guest, Segment};
use skate_audio_formats::{banks, eaac, eb, splc};

use super::{Error, describe};

/// Evidence-backed resident banks used by the rider and board's authored sound objects.
pub const PLAYER_BANKS: &[&str] = &[
    "Sk8_Air_Flip_Tricks.abk",
    "Foley_Cloth.abk",
    "Treatments.abk",
    "Brd_Squeaks.abk",
    "Seams_Bank.abk",
    "PatchBank_Rolling_Surfaces.abk",
    "PatchBank_Objects.abk",
    "PatchBank_SpiderCracks.abk",
    "PatchBank_RocksBounce.abk",
    "Rolling_Rattles.abk",
    "WHEEL_SKID_BANK.abk",
    "GRINDS.abk",
    "board_scrapes.abk",
    "Bodyslide.abk",
    "fstep_skateshoe1_sm.abk",
    "FOOT_DRAG.abk",
    "sense_of_speed.abk",
    // No samples: `Start_up_Play_ctl`, the boot utility that answers `rnd_call` broadcasts with the
    // `send_random_0_to_9*` values the seam programs pick their variants from.
    "Common.abk",
];

/// The `SPLC` sound banks the one-shot ("Splice") voices play from: wheel pops and the landing
/// impact (`sub_824B9CC8`, `sub_824BA630`) name `Skate_Collisions` samples through the vault.
/// `DLC_Cartoon_Collisions.bnk` is the pops' DLC variant and is absent from a base installation,
/// so it is loaded only when the archive holds it.
pub const SPLICE_BANKS: &[&str] = &["Skate_Collisions.bnk"];
/// `Skate_Metal.bnk` and `HOM_Set_1.bnk` are the other two banks the collision materials name:
/// `sub_824967F8` picks a material's sample out of the field family its kind word selects, and an
/// AttribSys field's type name is its bank (`Skate_Metal` -> `Skate_Metal.bnk`). A metal rail
/// resolves into `Skate_Metal.bnk`, which is why a rail grind does not sound like concrete. Both
/// are optional so a trimmed installation still boots.
/// `sk8_menu.bnk` holds the front-end one-shots. The session marker's three sounds name it the
/// same way a collision material does -- the vault's `fe` records carry their sample id in a field
/// whose *type name* is `sk8_menu`, so the bank falls out of the same convention:
///
/// | vault `fe` key | sample | |
/// |---|---|---|
/// | `cellphone_place_marker` | 237 | the marker is set |
/// | `cellphone_goto_marker` | 236 | the session returns to it |
/// | `cellphone_marker_error` | 209 | the placement was refused |
///
/// Optional like the rest, so a trimmed installation still boots -- without it the marker is
/// simply silent again rather than failing to start.
pub const MENU_BANK: &str = "sk8_menu.bnk";
pub const OPTIONAL_SPLICE_BANKS: &[&str] = &[
    "DLC_Cartoon_Collisions.bnk",
    "Skate_Metal.bnk",
    "HOM_Set_1.bnk",
    MENU_BANK,
];

/// Where the installer stages the MixMap under `assets`.
pub const MIXMAP_PATH: &str = "private/stock/data/audio/MixMapSK8.mxb";

/// One decoded sample, addressed by its original EAAC header offset within its bank.
#[derive(Clone, Debug)]
pub struct BankPcm {
    pub bank: String,
    pub index: usize,
    pub header_offset: u32,
    pub header: eaac::Header,
    pub pcm: CachedPcm,
}

/// Ready-to-install assets. Nothing here performs IO after construction.
pub struct PlayerAudioCatalog {
    pub guest: Guest,
    pub projects: Vec<Vec<u8>>,
    pub banks: Vec<(String, Vec<u8>)>,
    pub samples: Vec<BankPcm>,
    pub cache_hits: usize,
    /// `data/audio/MixMapSK8.mxb`, the MixMap the game loads in `sub_82484FE8` (a loose file,
    /// staged at `<assets>/private/stock/data/audio/`). Set by [`Self::from_assets`]; `None` when
    /// the file is not staged or the catalog was built from explicit paths. Install it with
    /// `AuthoredRuntime::load_mixmap`.
    pub mixmap: Option<Vec<u8>>,
}

impl PlayerAudioCatalog {
    /// Load the prepared setup layout under an installation's `assets` directory.
    pub fn from_assets(assets: &Path, cache: Option<&Path>) -> Result<Self, Error> {
        let mut catalog = Self::load(
            &assets.join("private/stock/data/audio/audiofiles.big"),
            &assets.join("private/stock/audio-runtime-image"),
            cache,
        )?;
        catalog.mixmap = std::fs::read(assets.join(MIXMAP_PATH)).ok();
        Ok(catalog)
    }

    /// Decode all rider/board samples, or load a cache whose source bytes match exactly.
    pub fn load(archive: &Path, image: &Path, cache: Option<&Path>) -> Result<Self, Error> {
        Self::load_banks(archive, image, cache, PLAYER_BANKS)
    }

    /// The `SPLC` sound banks of [`SPLICE_BANKS`], with every sample in their tables decoded. The
    /// samples land in [`Self::samples`] addressed by their EAAC header offset, exactly as the
    /// patch banks' are, so `AuthoredRuntime::insert_pcm(bank_base + header_offset, ..)` serves
    /// both.
    pub fn load_splice_banks(&mut self, archive: &Path, cache: Option<&Path>) -> Result<(), Error> {
        let data = read_archive(archive).map_err(|e| io_error(archive, e))?; // perf 10-19: shared
        let parsed = eb::Archive::parse(&data)?;
        let wanted: Vec<(&str, bool)> = SPLICE_BANKS
            .iter()
            .map(|n| (*n, true))
            .chain(OPTIONAL_SPLICE_BANKS.iter().map(|n| (*n, false)))
            .collect();
        for (name, required) in wanted {
            let member = parsed.entries.iter().find(|e| {
                e.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            });
            let member = match (member, required) {
                (Some(member), _) => member,
                (None, false) => continue,
                (None, true) => return Err(Error::Format(format!("missing sound bank {name}"))),
            };
            if member.is_compressed() {
                return Err(Error::Format(format!("compressed sound bank {name}")));
            }
            let bytes = member_bytes(&data, member)?;
            let bank = splc::Splc::parse(bytes)?;
            let ranges: Vec<std::ops::Range<usize>> = (0..bank.samples.len())
                .map(|index| {
                    let start = bank.streams_offset + bank.samples[index].offset as usize;
                    let end = bank.streams_offset + bank.samples[index].end as usize;
                    start..end.max(start)
                })
                .collect();
            let path = cache
                .map(|root| root.join(format!("{name}.{:016x}.v3.pcm", fingerprint(bytes))));
            let cached = path
                .as_deref()
                .and_then(|path| read_cache_ranges(path, bytes, &ranges));
            let decoded = match cached {
                Some(decoded) => {
                    self.cache_hits += decoded.len();
                    decoded
                }
                None => match decode_ranges(name, bytes, &ranges) {
                    Ok(decoded) => {
                        if let Some(path) = path.as_deref() {
                            let _ = write_cache(path, bytes, &decoded);
                        }
                        decoded
                    }
                    // An optional bank must never take the whole player-sound path down. Some of
                    // them use a container that needs ffmpeg, which is not present everywhere, and
                    // before these banks were listed that only cost the contacts they carry. Skip
                    // the bank and say so once; a material whose sample lives here then resolves
                    // to silence exactly as an uninstalled bank already does.
                    Err(error) if !required => {
                        eprintln!("SKATE_PLAYER_AUDIO optional sound bank {name} not decoded: {error}");
                        continue;
                    }
                    Err(error) => return Err(error),
                },
            };
            for (index, (offset, header, samples)) in decoded.into_iter().enumerate() {
                let mut source = PcmSource::new(Arc::from(samples), header.channels())
                    .map_err(|e| Error::Format(e.to_string()))?;
                if let Some(start) = header.loop_start {
                    source = source
                        .with_loop(start as usize, header.num_samples as usize)
                        .map_err(|e| Error::Format(format!("{name}#{index}: {e}")))?;
                }
                self.samples.push(BankPcm {
                    bank: name.into(),
                    index,
                    header_offset: offset,
                    header,
                    pcm: CachedPcm {
                        source,
                        sample_rate: header.sample_rate,
                    },
                });
            }
            self.banks.push((name.into(), bytes.to_vec()));
        }
        Ok(())
    }

    /// An explicit bank subset for fixture replay; normal gameplay loads the full catalog.
    pub fn load_banks(
        archive: &Path,
        image: &Path,
        cache: Option<&Path>,
        names: &[&str],
    ) -> Result<Self, Error> {
        let guest = load_guest_image(image)?;
        let data = read_archive(archive).map_err(|e| io_error(archive, e))?; // perf 10-19: shared
        let archive = eb::Archive::parse(&data)?;
        let mut projects = Vec::new();
        for member in &archive.entries {
            if member
                .name
                .as_deref()
                .is_some_and(|name| name.ends_with(".csi"))
            {
                if member.is_compressed() {
                    return Err(Error::Format(
                        "compressed CSI project is unsupported".into(),
                    ));
                }
                let bytes = member_bytes(&data, member)?;
                banks::Csi::parse(bytes)?;
                projects.push(bytes.to_vec());
            }
        }
        if projects.is_empty() {
            return Err(Error::Format("audiofiles.big has no CSI projects".into()));
        }
        let mut catalog = Self {
            guest,
            projects,
            banks: Vec::new(),
            samples: Vec::new(),
            cache_hits: 0,
            mixmap: None,
        };
        for &name in names {
            let member = archive
                .entries
                .iter()
                .find(|e| {
                    e.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
                .ok_or_else(|| Error::Format(format!("missing player bank {name}")))?;
            if member.is_compressed() {
                return Err(Error::Format(format!(
                    "compressed player bank {name} is unsupported"
                )));
            }
            let bytes = member_bytes(&data, member)?;
            let parsed = banks::Abk::parse(bytes)?;
            let path =
                cache.map(|root| root.join(format!("{name}.{:016x}.v3.pcm", fingerprint(bytes))));
            let cached = path
                .as_deref()
                .and_then(|path| read_cache(path, bytes, &parsed));
            let decoded = match cached {
                Some(decoded) => {
                    catalog.cache_hits += decoded.len();
                    decoded
                }
                None => {
                    let decoded = decode_bank(name, bytes, &parsed)?;
                    if let Some(path) = path.as_deref() {
                        // A cache is an optimization. A read-only installation must still play.
                        let _ = write_cache(path, bytes, &decoded);
                    }
                    decoded
                }
            };
            for (index, (offset, header, samples)) in decoded.into_iter().enumerate() {
                let mut source = PcmSource::new(Arc::from(samples), header.channels())
                    .map_err(|e| Error::Format(e.to_string()))?;
                if let Some(start) = header.loop_start {
                    source = source
                        .with_loop(start as usize, header.num_samples as usize)
                        .map_err(|e| Error::Format(format!("{name}#{index}: {e}")))?;
                }
                catalog.samples.push(BankPcm {
                    bank: name.into(),
                    index,
                    header_offset: offset,
                    header,
                    pcm: CachedPcm {
                        source,
                        sample_rate: header.sample_rate,
                    },
                });
            }
            catalog.banks.push((name.into(), bytes.to_vec()));
        }
        Ok(catalog)
    }
}

/// perf 10-19: an archive's bytes, shared with every other reader while anyone still holds them. The audio boot
/// read audiofiles.big (105 MB) whole three times (`load_banks`, `load_splice_banks`, the contact voices'
/// `SpliceBanks::load`); the game's `player_audio::prepare` holds one read across all three
/// (SKATE_AUDIO_READ_ONCE). Nobody holding it: a fresh read, as before, and nothing kept.
pub fn read_archive(path: &Path) -> std::io::Result<Arc<Vec<u8>>> {
    use std::sync::{Mutex, PoisonError, Weak};
    static HELD: Mutex<Vec<(PathBuf, Weak<Vec<u8>>)>> = Mutex::new(Vec::new());
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    held.retain(|(_, data)| data.strong_count() > 0);
    if let Some(data) = held.iter().find(|(at, _)| at == path).and_then(|(_, data)| data.upgrade()) {
        return Ok(data);
    }
    let data = Arc::new(std::fs::read(path)?);
    held.push((path.to_owned(), Arc::downgrade(&data)));
    Ok(data)
}

fn io_error(path: &Path, error: std::io::Error) -> Error {
    Error::Format(format!("{}: {error}", path.display()))
}

fn member_bytes<'a>(data: &'a [u8], entry: &eb::Entry) -> Result<&'a [u8], Error> {
    data.get(entry.range())
        .ok_or_else(|| Error::Format(format!("archive member {:?} is out of bounds", entry.name)))
}

/// Load only correctly named guest-image regions and reject overlapping mappings.
pub fn load_guest_image(directory: &Path) -> Result<Guest, Error> {
    let mut segments = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| io_error(directory, e))? {
        let path = entry.map_err(|e| io_error(directory, e))?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("bin") {
            continue;
        }
        let Some(hex) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("g_"))
        else {
            continue;
        };
        if hex.len() != 4 {
            continue;
        }
        let page = u32::from_str_radix(hex, 16)
            .map_err(|_| Error::Format(format!("invalid guest image region {}", path.display())))?;
        let bytes = std::fs::read(&path).map_err(|e| io_error(&path, e))?;
        if bytes.is_empty() || page < 0x8200 || page > 0x8320 {
            return Err(Error::Format(format!(
                "invalid guest image span {}",
                path.display()
            )));
        }
        segments.push(Segment {
            base: page << 16,
            bytes,
        });
    }
    segments.sort_by_key(|s| s.base);
    for pair in segments.windows(2) {
        if u64::from(pair[0].base) + pair[0].bytes.len() as u64 > u64::from(pair[1].base) {
            return Err(Error::Format("guest image regions overlap".into()));
        }
    }
    let guest = Guest::from_segments(segments);
    for at in [
        0x82FC_E4CC,
        0x82FD_28C0,
        0x82FD_35F4,
        0x8302_D4A4,
        0x8303_6F4C,
    ] {
        guest
            .span(at, 4)
            .map_err(|e| Error::Format(format!("incomplete guest audio image: {e}")))?;
    }
    Ok(guest)
}

type Decoded = Vec<(u32, eaac::Header, Vec<i16>)>;

/// Any sound data the title loads itself, decoded into PCM addressed by the EAAC header's offset
/// in what was loaded, through the same content-checked cache, for
/// `AuthoredRuntime::insert_pcm(load address + offset, ..)`: an `.abk` patch bank, an `SPLC` sound
/// bank, a lone resident EAAC stream (`.snr`), or an EB archive of those loaded whole (the
/// resident ambience archive). A sample that does not decode is left out (it plays as silence)
/// rather than losing its whole bank.
pub fn decode_title_bank(
    name: &str,
    bytes: &[u8],
    cache: Option<&Path>,
) -> Result<Vec<(u32, CachedPcm)>, Error> {
    if name.to_ascii_lowercase().ends_with(".grain") {
        // A grain file: its EAAC stream starts after the grain header (length in word 0), where
        // the grain player's plays address it (`grain::host`'s `data + header`).
        let grain = skate_audio_formats::grain::Grain::parse(bytes)?;
        let at = u32::from_be_bytes(bytes[..4].try_into().unwrap_or([0; 4]));
        let pcm = super::ffmpeg::decode_resident(grain.stream_bytes())
            .map_err(|e| Error::Format(format!("{name}: {e}")))?;
        let source = PcmSource::new(Arc::from(pcm), grain.stream.channels())
            .map_err(|e| Error::Format(e.to_string()))?;
        return Ok(vec![(at, CachedPcm { source, sample_rate: grain.stream.sample_rate })]);
    }
    let ranges = if bytes.get(..4) == Some(b"SPLC") {
        let bank = splc::Splc::parse(bytes)?;
        (0..bank.samples.len())
            .map(|index| {
                let start = bank.streams_offset + bank.samples[index].offset as usize;
                let end = bank.streams_offset + bank.samples[index].end as usize;
                start..end.max(start)
            })
            .collect::<Vec<_>>()
    } else if let Ok(bank) = banks::Abk::parse(bytes) {
        (0..bank.present())
            .filter_map(|index| bank.sample_range(index))
            .collect::<Vec<_>>()
    } else if let Ok(archive) = eb::Archive::parse(bytes) {
        let mut out = Vec::new();
        for member in archive.entries.iter().filter(|e| !e.is_compressed()) {
            let range = member.range();
            let (Some(data), Some(member_name)) = (bytes.get(range.clone()), member.name.as_deref())
            else {
                continue;
            };
            if let Ok(pcm) = decode_title_bank(member_name, data, cache) {
                out.extend(pcm.into_iter().map(|(o, p)| (o + range.start as u32, p)));
            }
        }
        return Ok(out);
    } else if eaac::Header::parse(bytes, 0).is_ok_and(|h| h.stream_type == 0) {
        vec![0..bytes.len()]
    } else {
        return Ok(Vec::new());
    };
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    let path = cache.map(|root| root.join(format!("{name}.{:016x}.v3.pcm", fingerprint(bytes))));
    // A bank with samples that do not decode is cached as the ones that do, with their starts in
    // a `.idx` beside it (without it such a bank was decoded again on every boot: seconds each).
    // `v5p`: the samples ffmpeg cut short (a chunk's last byte left out, a flush frame ending a
    // read: ffmpeg.rs `close_flush_frames`) now decode whole, so the `v4p` subsets, which left
    // them out or padded them, are decoded again once (those banks now cache as complete `v3`).
    let partial = cache.map(|root| root.join(format!("{name}.{:016x}.v5p.pcm", fingerprint(bytes))));
    let cached = path
        .as_deref()
        .and_then(|path| read_cache_ranges(path, bytes, &ranges))
        .or_else(|| read_partial_cache(partial.as_deref()?, bytes, &ranges));
    let decoded = match cached {
        Some(decoded) => decoded,
        None => match decode_ranges(name, bytes, &ranges) {
            Ok(decoded) => {
                if let Some(path) = path.as_deref() {
                    let _ = write_cache(path, bytes, &decoded);
                }
                decoded
            }
            // A sample that does not decode: decode the others one by one.
            Err(_) => {
                let decoded: Decoded = ranges
                    .iter()
                    .filter_map(|r| {
                        let one = std::slice::from_ref(r);
                        decode_ranges(name, bytes, one)
                            .or_else(|_| super::ffmpeg::lenient(|| decode_ranges(name, bytes, one)))
                            .ok()
                    })
                    .flatten()
                    .collect();
                if let Some(path) = partial.as_deref() {
                    let _ = write_partial_cache(path, bytes, &decoded);
                }
                decoded
            }
        },
    };
    let mut out = Vec::with_capacity(decoded.len());
    for (index, (offset, header, samples)) in decoded.into_iter().enumerate() {
        let Ok(source) = PcmSource::new(Arc::from(samples), header.channels()) else {
            continue;
        };
        let source = match header.loop_start {
            Some(start) => match source.with_loop(start as usize, header.num_samples as usize) {
                Ok(looped) => looped,
                Err(e) => {
                    eprintln!("SKATE_PLAYER_AUDIO {name}#{index}: {e}");
                    continue;
                }
            },
            None => source,
        };
        out.push((offset, CachedPcm { source, sample_rate: header.sample_rate }));
    }
    Ok(out)
}

fn decode_bank(name: &str, bytes: &[u8], bank: &banks::Abk) -> Result<Decoded, Error> {
    let ranges = (0..bank.present())
        .map(|index| {
            bank.sample_range(index)
                .ok_or_else(|| Error::Format(format!("{name}#{index}: absent sample")))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    decode_ranges(name, bytes, &ranges)
}

/// Decode the EAAC stream in each range, as the resident-sample path needs it.
fn decode_ranges(
    name: &str,
    bytes: &[u8],
    ranges: &[std::ops::Range<usize>],
) -> Result<Decoded, Error> {
    // Several samples at once: each is its own ffmpeg run (~60 ms, mostly process start), so a cold cache decoded the
    // title's boot banks one after another for ~150 s of no skating sound on a first play (11-02 measurement).
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8).min(ranges.len());
    if workers <= 1 {
        return ranges.iter().enumerate().map(|(index, range)| decode_range(name, bytes, index, range)).collect();
    }
    let lenient = super::ffmpeg::is_lenient();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<Option<Result<_, Error>>> = (0..ranges.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(range) = ranges.get(index) else { break done };
                        let one = || decode_range(name, bytes, index, range);
                        // the lenient flag is per thread: carry the caller's into the worker
                        done.push((index, if lenient { super::ffmpeg::lenient(one) } else { one() }));
                    }
                })
            })
            .collect();
        for handle in handles {
            for (index, result) in handle.join().unwrap_or_default() {
                results[index] = Some(result);
            }
        }
    });
    results
        .into_iter()
        .enumerate()
        .map(|(index, r)| r.unwrap_or_else(|| Err(Error::Format(format!("{name}#{index}: decode worker failed")))))
        .collect()
}

/// One sample of [`decode_ranges`].
fn decode_range(
    name: &str,
    bytes: &[u8],
    index: usize,
    range: &std::ops::Range<usize>,
) -> Result<(u32, eaac::Header, Vec<i16>), Error> {
    {
        let range = range.clone();
        let data = bytes
            .get(range.clone())
            .ok_or_else(|| Error::Format(format!("{name}#{index}: sample outside bank")))?;
        let header = eaac::Header::parse(data, 0)?;
        if header.codec != eaac::Codec::Xma || header.stream_type != 0 {
            return Err(Error::Format(format!(
                "{name}#{index}: unsupported {:?} stream type {}",
                header.codec, header.stream_type
            )));
        }
        let blocks = eaac::blocks(&data[header.size()..])?;
        let frames: u64 = blocks
            .iter()
            .map(|block| u64::from(block.num_samples))
            .sum();
        if frames != u64::from(header.num_samples) {
            return Err(Error::Format(format!(
                "{name}#{index}: header/block sample counts disagree"
            )));
        }
        let info = describe(data, 0)?;
        let pcm = super::ffmpeg::decode_resident(data)
            .map_err(|e| Error::Format(format!("{name}#{index}: {e}")))?;
        let expected = info.num_samples as usize * usize::from(info.channels);
        if pcm.len() != expected {
            return Err(Error::Format(format!(
                "{name}#{index}: decoded {} frames, EAAC declares {}",
                pcm.len() / usize::from(info.channels),
                info.num_samples
            )));
        }
        Ok((range.start as u32, header, pcm))
    }
}

// The fingerprint selects a filename only: read_cache also compares the complete encoded bank.
fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

const CACHE_MAGIC: &[u8; 8] = b"S3PCM003";

/// How long a decode in the PCM cache may go unread before [`prune_cache`] drops it.
pub const CACHE_IDLE: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 60 * 60);

/// When this session first read or wrote the cache: a decode touched since is this session's.
static SESSION_START: std::sync::LazyLock<std::time::SystemTime> =
    std::sync::LazyLock::new(std::time::SystemTime::now);

/// Marks a cache file read now; [`prune_cache`] goes by this.
pub(super) fn touch_cache(path: &Path) {
    std::sync::LazyLock::force(&SESSION_START);
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

/// Deletes the decodes in `cache` (each `.pcm` with its `.idx`) that no session has read for
/// `idle`; the bytes freed. A bank re-exported or replaced leaves its old decode behind under a
/// new fingerprint, and the cache only grew. ponytail: by idle time, not a count per name — the
/// Skate 1, 2 and 3 traffic sets share bank names, so a count would evict live sets in turn.
/// Idle time runs up to the last session's reads, not to now: a player back after a month away
/// keeps what they last played with.
pub fn prune_cache(cache: &Path, idle: std::time::Duration) -> u64 {
    prune_cache_to(cache, idle, CACHE_CAP)
}

/// The most the PCM cache keeps after the idle prune (P6, launcher audit 10-12): past it the decodes read longest ago
/// go, never one this session read or wrote. Not retail (user 10-13).
pub const CACHE_CAP: u64 = 8 << 30;

fn prune_cache_to(cache: &Path, idle: std::time::Duration, cap: u64) -> u64 {
    let start = *SESSION_START;
    let decodes: Vec<_> = std::fs::read_dir(cache)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "pcm"))
        .filter_map(|path| Some((path.metadata().ok()?, path)))
        .collect();
    let modified = |meta: &std::fs::Metadata| meta.modified().ok().filter(|m| *m < start);
    let Some(last_use) = decodes.iter().filter_map(|(meta, _)| modified(meta)).max() else { return 0 };
    let (mut freed, mut kept) = (0, Vec::new());
    for (meta, path) in decodes {
        let idle_for = modified(&meta).and_then(|m| last_use.duration_since(m).ok());
        if idle_for.is_some_and(|t| t > idle) && std::fs::remove_file(&path).is_ok() {
            freed += meta.len();
            let _ = std::fs::remove_file(path.with_extension("idx"));
        } else {
            kept.push((meta, path));
        }
    }
    // P6: over the cap, the least recently read go first; this session's decodes (modified() None) never
    let mut size: u64 = kept.iter().map(|(meta, _)| meta.len()).sum();
    kept.retain(|(meta, _)| modified(meta).is_some());
    kept.sort_by_key(|(meta, _)| modified(meta));
    for (meta, path) in kept {
        if size <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            size -= meta.len();
            freed += meta.len();
            let _ = std::fs::remove_file(path.with_extension("idx"));
        }
    }
    freed
}

/// Whether [`decode_title_bank`] has `bytes` decoded in `cache` (marking it read).
pub fn title_bank_cached(name: &str, bytes: &[u8], cache: &Path) -> bool {
    let stem = format!("{name}.{:016x}", fingerprint(bytes));
    ["v3", "v5p"].iter().map(|v| cache.join(format!("{stem}.{v}.pcm"))).any(|path| {
        let hit = path.is_file();
        if hit {
            touch_cache(&path);
        }
        hit
    })
}

fn read_cache(path: &Path, bank: &[u8], parsed: &banks::Abk) -> Option<Decoded> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.get(..8)? != CACHE_MAGIC || bytes.get(8..8 + bank.len())? != bank {
        return None;
    }
    touch_cache(path);
    let mut cursor = 8 + bank.len();
    let mut decoded = Vec::with_capacity(parsed.present());
    for index in 0..parsed.present() {
        let range = parsed.sample_range(index)?;
        let header = eaac::Header::parse(bank.get(range.clone())?, 0).ok()?;
        let length = header.num_samples as usize * usize::from(header.channels()) * 2;
        let pcm = bytes
            .get(cursor..cursor.checked_add(length)?)?
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        decoded.push((range.start as u32, header, pcm));
        cursor += length;
    }
    (cursor == bytes.len()).then_some(decoded)
}

/// [`read_cache`] for a bank whose samples are addressed by explicit ranges (`SPLC`).
fn read_cache_ranges(path: &Path, bank: &[u8], ranges: &[std::ops::Range<usize>]) -> Option<Decoded> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.get(..8)? != CACHE_MAGIC || bytes.get(8..8 + bank.len())? != bank {
        return None;
    }
    touch_cache(path);
    let mut cursor = 8 + bank.len();
    let mut decoded = Vec::with_capacity(ranges.len());
    for range in ranges {
        let header = eaac::Header::parse(bank.get(range.clone())?, 0).ok()?;
        let length = header.num_samples as usize * usize::from(header.channels()) * 2;
        let pcm = bytes
            .get(cursor..cursor.checked_add(length)?)?
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        decoded.push((range.start as u32, header, pcm));
        cursor += length;
    }
    (cursor == bytes.len()).then_some(decoded)
}

/// [`read_cache_ranges`] over the ranges a partial cache's `.idx` names (their starts, u32 LE).
fn read_partial_cache(path: &Path, bank: &[u8], ranges: &[std::ops::Range<usize>]) -> Option<Decoded> {
    let index = std::fs::read(path.with_extension("idx")).ok()?;
    let subset: Vec<_> = index
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize)
        .map(|start| ranges.iter().find(|r| r.start == start).cloned())
        .collect::<Option<_>>()?;
    read_cache_ranges(path, bank, &subset)
}

fn write_partial_cache(path: &Path, bank: &[u8], decoded: &Decoded) -> std::io::Result<()> {
    let index: Vec<u8> = decoded.iter().flat_map(|(start, _, _)| start.to_le_bytes()).collect();
    std::fs::write(path.with_extension("idx"), index)?;
    write_cache(path, bank, decoded)
}

fn write_cache(path: &Path, bank: &[u8], decoded: &Decoded) -> std::io::Result<()> {
    use std::io::Write;
    std::sync::LazyLock::force(&SESSION_START);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Unique per write: the map prewarm and the audio boot can decode the same bank at once in one process.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = PathBuf::from(format!("{}.{}.{n}.tmp", path.display(), std::process::id()));
    let mut file = std::io::BufWriter::new(std::fs::File::create(&temporary)?);
    file.write_all(CACHE_MAGIC)?;
    file.write_all(bank)?;
    for (_, _, samples) in decoded {
        for sample in samples {
            file.write_all(&sample.to_le_bytes())?;
        }
    }
    file.flush()?;
    drop(file);
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skate-pcm-cache-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn aged(path: &Path, days: u64) {
        std::fs::write(path, b"x").unwrap();
        let when = SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    #[test]
    fn decodes_unread_past_the_idle_time_are_pruned_with_their_index() {
        let cache = dir("prune");
        aged(&cache.join("stale.abk.0000000000000001.v4p.pcm"), 40);
        aged(&cache.join("stale.abk.0000000000000001.v4p.idx"), 40);
        aged(&cache.join("live.abk.0000000000000002.v3.pcm"), 2);
        aged(&cache.join("live.abk.0000000000000003.v4p.idx"), 40); // its .pcm was read lately
        aged(&cache.join("live.abk.0000000000000003.v4p.pcm"), 1);
        aged(&cache.join("notes.txt"), 40);
        assert_eq!(prune_cache(&cache, CACHE_IDLE), 1);
        let mut left: Vec<String> = std::fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "live.abk.0000000000000002.v3.pcm",
                "live.abk.0000000000000003.v4p.idx",
                "live.abk.0000000000000003.v4p.pcm",
                "notes.txt",
            ]
        );
    }

    #[test]
    fn over_the_cap_the_least_recently_read_go_and_never_this_sessions() {
        let cache = dir("cap");
        for (name, days) in [("a", 5), ("b", 3), ("c", 1)] {
            let path = cache.join(format!("{name}.abk.0000000000000007.v3.pcm"));
            aged(&path, days);
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            let when = SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
            aged(&path.with_extension("idx"), days);
        }
        let mine = cache.join("mine.abk.0000000000000008.v3.pcm");
        std::fs::write(&mine, vec![0u8; 100]).unwrap();
        touch_cache(&mine); // read this session: never evicted, whatever the cap
        assert_eq!(prune_cache_to(&cache, CACHE_IDLE, 250), 200, "a then b: 400 bytes down to 200");
        assert!(!cache.join("a.abk.0000000000000007.v3.pcm").exists() && !cache.join("a.abk.0000000000000007.v3.idx").exists());
        assert!(!cache.join("b.abk.0000000000000007.v3.pcm").exists());
        assert!(cache.join("c.abk.0000000000000007.v3.pcm").exists() && mine.exists());
        assert_eq!(prune_cache_to(&cache, CACHE_IDLE, 0), 100, "only c can go; this session's decode stays");
        assert!(mine.exists());
    }

    #[test]
    fn a_month_away_from_the_game_is_not_idle_time() {
        let cache = dir("away");
        aged(&cache.join("last.abk.0000000000000004.v3.pcm"), 45); // read in the last session
        aged(&cache.join("before.abk.0000000000000005.v3.pcm"), 60); // 15 days before it
        aged(&cache.join("old.abk.0000000000000006.v3.pcm"), 80); // 35 days before it
        assert_eq!(prune_cache(&cache, CACHE_IDLE), 1);
        assert!(cache.join("last.abk.0000000000000004.v3.pcm").exists());
        assert!(cache.join("before.abk.0000000000000005.v3.pcm").exists());
        assert!(!cache.join("old.abk.0000000000000006.v3.pcm").exists());
    }

    #[test]
    fn a_cached_title_bank_is_found_and_marked_read() {
        let cache = dir("cached");
        let bank = b"ABKC a bank";
        assert!(!title_bank_cached("x.abk", bank, &cache));
        let path = cache.join(format!("x.abk.{:016x}.v5p.pcm", fingerprint(bank)));
        aged(&path, 40);
        assert!(title_bank_cached("x.abk", bank, &cache));
        assert_eq!(prune_cache(&cache, CACHE_IDLE), 0, "a read decode is not idle");
        assert!(!title_bank_cached("x.abk", b"ABKC another export", &cache));
    }

    /// perf 10-19: a held read is shared; once nobody holds it the next read is fresh and nothing stays cached.
    #[test]
    fn an_archive_read_is_shared_only_while_held() {
        let path = dir("read-once").join("audiofiles.big");
        std::fs::write(&path, b"first").unwrap();
        let held = read_archive(&path).unwrap();
        std::fs::write(&path, b"second").unwrap();
        let shared = read_archive(&path).unwrap();
        assert!(Arc::ptr_eq(&held, &shared) && shared.as_slice() == b"first");
        drop((held, shared));
        assert_eq!(read_archive(&path).unwrap().as_slice(), b"second");
    }
}

#[cfg(test)]
mod xma_tests {
    use super::*;

    /// The eight retail samples the external decoder used to lose part of: six to a frame ending
    /// flush with a read's last XMA packet, two to a chunk length one byte short.
    #[test]
    #[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets, and ffmpeg"]
    fn samples_ffmpeg_cut_short_decode_whole() {
        let Some(assets) = std::env::var_os("SKATE_ASSETS").or_else(|| std::env::var_os("SKATE3_ASSET_ROOT")) else {
            return;
        };
        let file = PathBuf::from(assets).join("private/stock/data/audio/audiofiles.big");
        let data = std::fs::read(&file).unwrap();
        let archive = eb::Archive::parse(&data).unwrap();
        for (name, index) in [
            ("coyote_howl.abk", 3),
            ("Delivery_truck_leave_1.abk", 2),
            ("delivery_truck_in_2.abk", 2),
            ("truck_by_2.abk", 2),
            ("vehicle_away.abk", 1),
            ("water_lapping_pond.abk", 3),
            ("retail_amb.abk", 2),
            ("corporate_party_amb.abk", 0),
        ] {
            let entry = archive.entries.iter().find(|e| e.name.as_deref() == Some(name)).unwrap();
            let bank = &data[entry.range()];
            let range = banks::Abk::parse(bank).unwrap().sample_range(index).unwrap();
            // decode_ranges fails on any decoder diagnostic or a count short of the header's.
            let decoded = decode_ranges(name, bank, &[range]).unwrap_or_else(|e| panic!("{e}"));
            let (_, header, pcm) = &decoded[0];
            assert_eq!(pcm.len(), header.num_samples as usize * usize::from(header.channels()), "{name}#{index}");
        }
    }
}
