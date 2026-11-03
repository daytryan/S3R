//! The prepared weather recordings (tools/prepare_weather_audio.py): `manifest.json` plus OGG Vorbis clips, held
//! compressed in memory and decoded as they play (stream.rs). A missing or partial folder leaves the bank empty or
//! short, with one warning, never a panic.
use bevy::log::warn;
use serde::Deserialize;
use std::{collections::HashMap, path::Path, sync::Arc};

/// Every role the stage writes, in its order.
pub(crate) const ROLES: [&str; 13] = [
    "bed_light",
    "bed_heavy",
    "hard_light",
    "hard_heavy",
    "metal",
    "wood",
    "foliage",
    "water",
    "roof_metal",
    "roof_concrete",
    "thunder_crack",
    "thunder_roll",
    "thunder_rumble",
];

/// The stage's loudness target per role (prepare_weather_audio.py ROLES). A loop the stage had to leave quieter,
/// to keep its peaks under -1.5 dBTP, is brought back up to it here.
fn target_lufs(role: &str) -> f32 {
    match role {
        "bed_light" | "bed_heavy" => -30.,
        "roof_metal" | "roof_concrete" => -28.,
        r if r.starts_with("thunder") => -20.,
        _ => -32.,
    }
}

#[derive(Clone)]
pub(crate) enum ClipData {
    /// The file's bytes and its headers, parsed once at load (stream.rs).
    Ogg(Arc<[u8]>, Arc<super::stream::Headers>),
    /// Interleaved samples (tests: no encoder at hand).
    #[cfg(test)]
    Pcm(Arc<[f32]>),
}

#[derive(Clone)]
pub(crate) struct Clip {
    pub data: ClipData,
    pub channels: usize,
    pub looped: bool,
    /// Length in frames (the last Ogg page's granule position).
    pub frames: u64,
    /// Loudness make-up to the role's target (loops only).
    pub gain: f32,
}

#[derive(Default)]
pub(crate) struct Bank {
    roles: HashMap<&'static str, Vec<Clip>>,
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    sample_rate: u32,
    roles: HashMap<String, Vec<Entry>>,
}
#[derive(Deserialize)]
struct Entry {
    file: String,
    #[serde(rename = "loop")]
    looped: bool,
    lufs: f32,
}

impl Bank {
    /// `root`/manifest.json and its clips. Missing or unreadable pieces are skipped and reported in one warning.
    pub(crate) fn load(root: &Path) -> Bank {
        let (bank, problems) = Bank::load_report(root);
        if !problems.is_empty() {
            warn!("weather audio: {} problem(s) in {}: {}", problems.len(), root.display(), problems.join("; "));
        }
        bank
    }

    /// [`Bank::load`] without the warning: the bank and every problem met (for the one warning, and tests).
    pub(crate) fn load_report(root: &Path) -> (Bank, Vec<String>) {
        let manifest = match std::fs::read(root.join("manifest.json"))
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<Manifest>(&b).map_err(|e| e.to_string()))
        {
            Ok(m) if m.version == 1 && m.sample_rate == super::mixer::RATE => m,
            Ok(m) => return (Bank::default(), vec![format!("manifest version {} at {} Hz; silent", m.version, m.sample_rate)]),
            Err(e) => {
                return (Bank::default(), vec![format!("no prepared sounds ({e}); silent (run tools/prepare_weather_audio.py)")]);
            }
        };
        let mut bank = Bank::default();
        let mut problems = Vec::new();
        for role in ROLES {
            for entry in manifest.roles.get(role).into_iter().flatten() {
                match clip(&root.join(&entry.file), entry, role) {
                    Ok(clip) => bank.roles.entry(role).or_default().push(clip),
                    Err(e) => problems.push(format!("{}: {e}", entry.file)),
                }
            }
        }
        (bank, problems)
    }

    pub(crate) fn clips(&self, role: &str) -> &[Clip] {
        self.roles.get(role).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.roles.values().all(Vec::is_empty)
    }
}

fn clip(path: &Path, entry: &Entry, role: &str) -> Result<Clip, String> {
    let bytes: Arc<[u8]> = std::fs::read(path).map_err(|e| e.to_string())?.into();
    let headers = super::stream::headers(&bytes)?;
    let (rate, channels) = (headers.ident.audio_sample_rate, usize::from(headers.ident.audio_channels));
    if rate != super::mixer::RATE || !(1..=2).contains(&channels) {
        return Err(format!("{rate} Hz, {channels} channels"));
    }
    let gain = if entry.looped { 10f32.powf((target_lufs(role) - entry.lufs) / 20.).clamp(0.5, 2.) } else { 1. };
    Ok(Clip { frames: last_granule(&bytes), data: ClipData::Ogg(bytes, Arc::new(headers)), channels, looped: entry.looped, gain })
}

/// The granule position of the last Ogg page: the stream's length in frames.
fn last_granule(bytes: &[u8]) -> u64 {
    bytes
        .windows(4)
        .rposition(|w| w == b"OggS")
        .and_then(|at| bytes.get(at + 6..at + 14))
        .map_or(0, |g| u64::from_le_bytes(g.try_into().unwrap()))
}

/// Tests: every role as a one-second loop of pure tones (whole cycles, so seamless): a low tone of its own plus a
/// 9 kHz one for the cover filter to take out. The roofs are a single low tone; thunder is a decaying 3 s rumble.
#[cfg(test)]
pub(crate) const ROOF_TEST_TONE: f32 = 330.;
#[cfg(test)]
impl Bank {
    pub(crate) fn synthetic() -> Bank {
        use super::mixer::RATE;
        let tone = |f: f32, i: usize| (std::f32::consts::TAU * f * i as f32 / RATE as f32).sin();
        let mut bank = Bank::default();
        for (n, role) in ROLES.into_iter().enumerate() {
            let thunder = role.starts_with("thunder");
            let frames = if thunder { 3 * RATE as usize } else { RATE as usize };
            let mut rng = 0x9E37_79B9_7F4A_7C15u64 + n as u64;
            let samples: Vec<f32> = (0..frames)
                .flat_map(|i| {
                    let v = if thunder {
                        rng ^= rng << 13;
                        rng ^= rng >> 7;
                        rng ^= rng << 17;
                        let noise = (rng >> 40) as f32 / (1u64 << 24) as f32 * 2. - 1.;
                        (0.3 * tone(80., i) + 0.1 * noise) * (-(i as f32) / RATE as f32).exp()
                    } else if role.starts_with("roof") {
                        0.05 * tone(ROOF_TEST_TONE, i)
                    } else {
                        0.02 * tone(500. + 40. * n as f32, i) + 0.004 * tone(9000., i)
                    };
                    [v, v]
                })
                .collect();
            bank.roles.entry(role).or_default().push(Clip {
                data: ClipData::Pcm(samples.into()),
                channels: 2,
                looped: !thunder,
                frames: frames as u64,
                gain: 1.,
            });
        }
        bank
    }
}
