//! The soundtrack: retail Skate 3's 46 licensed songs (its iPod music, `data/audio/music/Ipod_Stream.mus` on the owned
//! disc, read in place) or the player's own songs from the music folder beside the exe, shuffled, with the Music
//! Player's song toggles and Audio Settings' Music Volume. music_ui.rs draws the retail now-playing chyron and the
//! Audio Settings / Music Player screens. Retail evidence: docs/superpowers/MUSIC_2026-10-03.md.
//!
//! A worker thread decodes ahead through ffmpeg (a disc song one ~12 s segment at a time, a file as a stream) into a
//! LivePcm the mixer pulls. It holds a couple of seconds and paces to what the device has played, not to frames, so a
//! busy or unfocused window keeps playing like the rest of the game's audio. The mixer reads it through a [`Deck`],
//! which plays retail's tape stop (the Start menu and the replay slow the song to a halt; leaving them spins it back up).
use crate::skate_audio::{LivePcm, LivePlayback};
use bevy::{
    audio::{AddAudioSource, Decodable, Source as RodioSource, Volume},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use skate_audio_formats::mus;
use skate_data::audio::ffmpeg;
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Retail's song enum (`0x8302E048`, entries 1..=46 after IPOD_INVALID). Song k streams Ipod_Stream.mus segments
/// 30k..30k+29 in order (ipod.mpf: node 32k starts song k, nodes 32k+2.. play one segment each). Titles are the
/// language table's IPOD_<NAME>_ARTIST / _TITLE / _ALBUM.
const SONGS: [&str; 46] = [
    "3InchesOfBlood_BattleOfBrotherhood",
    "AgentOrange_Bloodstains",
    "AnimalCollective_SummertimeClothes",
    "BeastieBoys_LeeMajorsComeAgain",
    "BenjyFerree_ComingToMe",
    "BimSherman_LoversLeap",
    "CannedHeat_UpInTheCountry",
    "ChadVanGaalen_BareFeetOnWetGriptape",
    "Cheeseburger_ComingHome",
    "Clinic_TheWitch",
    "Deadmoon_DiamondsInTheRough",
    "DelTheFunkyHomosapien_YoungDre",
    "DinasaurJr_AlmostReady",
    "DrJohn_RightPlaceWrongTime",
    "DreamEvil_Immortal",
    "Hanger18_FeetToFeet",
    "Japanther_TheGravy",
    "JohnnyThunders_BornToLose",
    "JoyDivision_Disorder",
    "JuniorMurvin_CoolOutSon",
    "Kinski_PunchingGoodbyeOutFront",
    "LoveCity_TheOtherSide",
    "Mazarin_ForEnergyInfinite",
    "MobbDeep_PutEmInTheirPlace",
    "Mudhoney_imNow",
    "NeilDiamond_CracklinRosie",
    "Nilhilist_MetalAndMayhem",
    "NoAge_SleeperHold",
    "OlDirtyBastard_ShimmyShimmyYa",
    "Pixies_Debaser",
    "QLazarus_GoodbyeHorses",
    "RobSonic_BrandNewVandals",
    "Sorcerers_DrunkenSkateSession",
    "SteelPanther_EyesOfAPanther",
    "TheDemonics_750-Four",
    "TheGirls_WhereWolvesDrink",
    "TheGoat_Tantilizing",
    "TheGreenhornes_GoodTimes",
    "TheMightyUnderdogs_GunFight",
    "Misfits_LondonDungeon",
    "Perceptionists_PartyHard",
    "TheThermals_PillarOfSalt",
    "ThemCrookedVultures_DeadEndFriends",
    "TommyMcCookAndTheSupersonics_PsychedlicReggae",
    "WhiteRoseMovement_CigaretteMachine",
    "YoungJeezy_PutOn",
];
/// Retail's per-song gain, in SONGS order: AttribSys class 0xE93ABF17955A5264, collection 0xE7CC11BB107155EC, 46
/// IPodSongInfo (song id, gain) keyed by the ids at 0x8302E108. 8249C8D0 reads it into MusicSystem +1196 as a song
/// starts (8249C6D8), and ComputeVolume (824A0118) multiplies the Music level by it in Playlist Mode. An id without an
/// entry (the music folder's songs) gets 1.0.
const GAIN: [f32; 46] = [
    0.9, 1.05, 1.1, 0.9, 1.0, 1.1, 0.95, 0.9, 0.9, 0.8, 1.0, 1.1, 1.0, 0.95, 0.95, 1.0, 0.95, 1.0, 1.2, 1.0, 1.0, 0.8,
    1.05, 1.1, 1.0, 1.2, 1.05, 1.0, 1.0, 1.0, 1.1, 1.1, 0.9, 0.85, 0.85, 0.9, 1.0, 1.1, 1.0, 1.0, 0.9, 1.0, 0.7, 1.05,
    0.95, 1.1,
];
const SEGMENTS_PER_SONG: usize = 30;
/// Every Ipod_Stream.mus segment is XMA2 stereo at 44.1 kHz (its SNR table); files are decoded to the same.
const RATE: u32 = 44_100;
/// The tape stop's length: MusicCore::UpdateMusicState's PAUSE (82D153D8) and RESUME (82D1542C) ramp the song's PATH
/// pitchmult over MusicSystem vt+64 (8249A058) seconds, aud_music/default 0x3265D5585CC4B8E5 = 0.6.
const TAPE_FRAMES: u32 = RATE * 3 / 5;
/// PATH evaluates the ramp on its service thread (8295AE98), which sleeps 50 ms between ticks, so the rate moves in
/// 50 ms steps: 12 over the 0.6 s (INFERRED tick source; the user chose the stepped slide).
const TAPE_STEP: u32 = RATE / 20;
/// Frames queued for the mixer: rides out a stalled or unfocused machine, small enough that Stop is immediate.
const QUEUE_FRAMES: usize = 2 * RATE as usize;
/// Formats ffmpeg reads that people keep music in.
const EXTENSIONS: [&str; 11] = ["mp3", "ogg", "oga", "opus", "flac", "wav", "m4a", "aac", "wma", "aif", "aiff"];
/// Retail's fallbacks for user music without tags (exe strings 0x8209C1D0 / 0x8209C1F4).
const UNKNOWN_ARTIST: &str = "Unknown Artist";
const UNKNOWN_ALBUM: &str = "Unknown Album";
/// `AudioSettings::volumes` slots.
pub(crate) const SFX: usize = 0;
pub(crate) const DIALOG: usize = 1;
pub(crate) const MUSIC: usize = 2;
/// Not retail (user 10-14, feature #10): the rain's own volume, [`AudioSettings::rain`] (not a `volumes` slot, so old
/// saves keep loading).
pub(crate) const RAIN: usize = 3;

/// Not retail (voice chat, user 10-26): Lower Game Sound While Talking. Game sound's gain: 1, or 0.5 (-6 dB) while a
/// voice is heard with the setting on, easing 150 ms each way (voice.rs eases it each frame); the music, the weather,
/// the skaters' and cars' sounds and [`one_shot`] are scaled by it.
pub(crate) fn duck() -> f32 {
    f32::from_bits(DUCK.load(Ordering::Relaxed))
}
static DUCK: AtomicU32 = AtomicU32::new(1f32.to_bits());
/// One frame (`dt` seconds) of [`duck`]'s ease: toward 0.5 while a voice is `audible` and the setting (`lower`) is on,
/// else toward 1.
pub(crate) fn ease_duck(audible: bool, lower: bool, dt: f32) {
    let target = if audible && lower { 0.5 } else { 1. };
    // The whole 0.5 swing in 150 ms.
    let (now, step) = (duck(), dt / 0.15 * 0.5);
    let next = if now < target { (now + step).min(target) } else { (now - step).max(target) };
    DUCK.store(next.to_bits(), Ordering::Relaxed);
}

/// How a sound the game starts itself (a screen's wav, a scene's post bed or speech) plays: at its Audio Settings
/// `slot`'s volume, as the game side's sounds are. Retail's graph scales a sound linearly by the setting
/// (headless_sfx_volume_lowers_the_game_side: SFX 3/10 is -10.7 dB; 20 log10 0.3 = -10.5 dB).
pub(crate) fn one_shot(menu: Option<&crate::graphics_menu::Menu>, slot: usize) -> PlaybackSettings {
    let gain = menu.map_or(1., |m| f32::from(m.audio().volumes[slot].min(10)) / 10.) * duck();
    PlaybackSettings::DESPAWN.with_volume(Volume::Linear(gain))
}

/// Retail Audio Settings and Music Player state, saved with the other settings (graphics.json "audio").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AudioSettings {
    /// [SFX, Dialog, Music] in tenths: retail keeps 0..1 floats at settings +0x80 / +0x84 / +0x88, moved 0.1 a press.
    pub volumes: [u8; 3],
    /// Play the music folder's songs instead of the game's while it has any (the 360's custom soundtrack).
    pub my_music: bool,
    /// Not retail (user 10-11): with `my_music`, the folder's songs shuffle in with the game's (Soundtrack: Both).
    pub both: bool,
    /// Music Player: songs switched off, by track key.
    pub songs_off: Vec<String>,
    /// Not retail (user 10-14, feature #10): rain's volume in tenths, on top of SFX.
    pub rain: u8,
}
impl AudioSettings {
    /// A slider's tenths: a `volumes` slot, or [`RAIN`].
    pub(crate) fn level(&self, slot: usize) -> u8 {
        if slot == RAIN { self.rain } else { self.volumes[slot] }
    }
    pub(crate) fn level_mut(&mut self, slot: usize) -> &mut u8 {
        if slot == RAIN { &mut self.rain } else { &mut self.volumes[slot] }
    }
}
impl Default for AudioSettings {
    fn default() -> Self {
        // Retail free skate with default options holds all three at 1.0 (skate-audio-core game.rs SETTINGS_WORDS).
        Self { volumes: [10; 3], my_music: true, both: false, songs_off: Vec::new(), rain: 10 }
    }
}
/// The Soundtrack row's Left/Right: Game Music, My Music, Both (not retail, user 10-11), wrapping.
pub(crate) fn step_soundtrack(audio: &mut AudioSettings, right: bool) {
    let now = if !audio.my_music { 0 } else if !audio.both { 1 } else { 2 };
    let next = (now + if right { 1 } else { 2 }) % 3;
    (audio.my_music, audio.both) = (next > 0, next == 2);
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    /// A retail song index (SONGS).
    Disc(usize),
    File(PathBuf),
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Track {
    /// "disc:<SONGS name>" or "file:<path in the music folder>".
    pub key: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub source: Source,
}

/// The music folder: `SKATE3_MUSIC`, else `music` beside the exe (as mods/ is).
pub(crate) fn folder() -> PathBuf {
    std::env::var_os("SKATE3_MUSIC").map(PathBuf::from).unwrap_or_else(|| {
        std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join("music"))).unwrap_or_else(|| "music".into())
    })
}

/// Ipod_Stream.mus on the disc the installation was made from (or beside it).
pub(crate) fn disc_stream(assets: &Path) -> Option<PathBuf> {
    skate_data::audio::title::roots(assets).into_iter().map(|r| r.join("data/audio/music/Ipod_Stream.mus")).find(|p| p.is_file())
}

fn disc_tracks(assets: &Path) -> Vec<Track> {
    // The language table rides in every prepared music-ui manifest; without it the enum names stand in.
    let language = std::fs::read(assets.join("private/music-ui/runtime/chyron.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .map(|mut j| j["language"].take())
        .unwrap_or_default();
    SONGS
        .iter()
        .enumerate()
        .map(|(song, name)| {
            let text = |field: &str| {
                language[format!("IPOD_{}_{field}", name.to_uppercase())].as_str().map(|s| s.trim().to_owned())
            };
            let (artist, title) = name.split_once('_').unwrap_or((name, name));
            Track {
                key: format!("disc:{name}"),
                artist: text("ARTIST").unwrap_or_else(|| artist.into()),
                title: text("TITLE").unwrap_or_else(|| title.into()),
                album: text("ALBUM").unwrap_or_default(),
                source: Source::Disc(song),
            }
        })
        .collect()
}

const README: &str = "S3R: your own music\r\n\r\nPut songs in this folder (subfolders too): mp3, ogg, opus, flac, wav, m4a, aac, wma or aiff.\r\nWhile it holds any, they play instead of the game's soundtrack, shuffled. The now-playing popup shows each song's\r\ntitle, artist and album tags, or the file name (\"Artist - Title.mp3\" reads as both).\r\n\r\nSettings > Audio > Audio Settings > Soundtrack switches between My Music, Game Music and Both (yours shuffled in with the game's), Music Volume sets the volume,\r\nand the Music Player (Start menu > Options) turns single songs on and off; Y there opens this folder.\r\nSongs added while the Music Player is open show up in it within a second.\r\n";

/// Audio files under `folder`, sorted; creates the folder and its README the first time.
fn music_files(folder: &Path) -> Vec<PathBuf> {
    if !folder.exists() {
        let made = std::fs::create_dir_all(folder).and_then(|()| std::fs::write(folder.join("README.txt"), README));
        if let Err(error) = made {
            warn!("Music folder {}: {error}", folder.display());
        }
    }
    let mut files = Vec::new();
    let mut dirs = vec![folder.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for path in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
            let audio = path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            if path.is_dir() {
                dirs.push(path);
            } else if audio {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// [artist, title, album] from `ffmpeg -f ffmetadata` output (ffmetadata backslash-escapes `=;#\` and newlines).
fn parse_tags(text: &str) -> [String; 3] {
    let mut tags: [String; 3] = Default::default();
    for (key, value) in text.lines().filter_map(|l| l.split_once('=')) {
        let slot = match key.to_ascii_lowercase().as_str() {
            "artist" => 0,
            "title" => 1,
            "album" => 2,
            _ => continue,
        };
        if tags[slot].is_empty() {
            let mut chars = value.chars();
            while let Some(c) = chars.next() {
                tags[slot].push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
            }
            tags[slot] = tags[slot].trim().to_owned();
        }
    }
    tags
}

fn tags(path: &Path) -> [String; 3] {
    let out = ffmpeg::command()
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "ffmetadata", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    out.map(|o| parse_tags(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// A file's track: its tags, else its name ("Artist - Title" reads as both), else retail's Unknown Artist / Album.
fn file_track(folder: &Path, path: &Path, [artist, title, album]: [String; 3]) -> Track {
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let (named_artist, named_title) = match stem.split_once(" - ") {
        Some((a, t)) => (a.trim().to_owned(), t.trim().to_owned()),
        None => (String::new(), stem.clone()),
    };
    let or = |tag: String, fallback: String| if tag.is_empty() { fallback } else { tag };
    let artist = or(artist, or(named_artist, UNKNOWN_ARTIST.into()));
    let relative = path.strip_prefix(folder).unwrap_or(path).to_string_lossy().replace('\\', "/");
    Track {
        key: format!("file:{relative}"),
        title: or(title, named_title),
        artist,
        album: or(album, UNKNOWN_ALBUM.into()),
        source: Source::File(path.to_path_buf()),
    }
}

/// The libraries, sent twice: with file names at once, then with tags (an ffmpeg run per file not in `known`).
/// `disc`: None reads the disc's songs (the first scan); a rescan passes the ones it has.
fn scan(assets: &Path, folder: &Path, disc: Option<Vec<Track>>, known: &[Track], out: &mpsc::Sender<(Vec<Track>, Vec<Track>)>) {
    let disc = disc.unwrap_or_else(|| if disc_stream(assets).is_some() { disc_tracks(assets) } else { Vec::new() });
    let files = music_files(folder);
    // ponytail: a linear find per file; a map by path if folders reach tens of thousands of songs.
    let old = |f: &PathBuf| known.iter().find(|t| t.source == Source::File(f.clone())).cloned();
    let named = files.iter().map(|f| old(f).unwrap_or_else(|| file_track(folder, f, Default::default()))).collect();
    if out.send((disc.clone(), named)).is_err() || files.iter().all(|f| old(f).is_some()) {
        return;
    }
    let tagged = files.iter().map(|f| old(f).unwrap_or_else(|| file_track(folder, f, tags(f)))).collect();
    let _ = out.send((disc, tagged));
}

// ---------------------------------------------------------------- worker
enum Order {
    /// `cut`: drop what is queued (a picked song); else it follows the current one. `gain`: retail's per-song GAIN.
    Play { id: u64, source: Source, cut: bool, gain: f32 },
    Stop,
}
enum Report {
    Done(u64),
    Failed(u64, String),
}

/// Ipod_Stream.mus read a segment at a time. Segment offsets are found by walking block headers (12-byte reads
/// through a buffer) the first time a song is reached; nothing else of the 180 MB file is held.
struct Disc {
    file: BufReader<File>,
    len: u64,
    starts: Vec<u64>,
}
impl Disc {
    fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // parse() checks the SNR table and first block lie inside what it is given.
        // ponytail: the first MiB holds both (Ipod_Stream's table is 22 KB); read up to first_block_offset if one doesn't.
        let mut head = Vec::new();
        (&mut file).take(1 << 20).read_to_end(&mut head).map_err(|e| e.to_string())?;
        let header = mus::Header::parse(&head).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        Ok(Self { file: BufReader::with_capacity(1 << 16, file), len, starts: vec![header.first_block_offset as u64] })
    }
    fn segment(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let io = |e: std::io::Error| e.to_string();
        while self.starts.len() <= n + 1 {
            let mut at = *self.starts.last().expect("starts with the first block");
            if at >= self.len {
                return Err(format!("Ipod_Stream.mus has no segment {n}"));
            }
            self.file.seek(SeekFrom::Start(at)).map_err(io)?;
            loop {
                let mut word = [0u8; 4];
                self.file.read_exact(&mut word).map_err(io)?;
                let word = u32::from_be_bytes(word);
                let size = word & 0x00FF_FFFF;
                if size as usize <= mus::HEADER_SIZE {
                    return Err(format!("Ipod_Stream.mus: bad block at {at:#x}"));
                }
                at += u64::from(size);
                if (word >> 24) & mus::FLAG_SEGMENT_END != 0 {
                    break;
                }
                self.file.seek_relative(i64::from(size) - 4).map_err(io)?;
            }
            let align = mus::SEGMENT_ALIGN as u64;
            self.starts.push(at.div_ceil(align) * align);
        }
        let (start, end) = (self.starts[n], self.starts[n + 1].min(self.len));
        self.file.seek(SeekFrom::Start(start)).map_err(io)?;
        let mut bytes = vec![0; (end - start) as usize];
        self.file.read_exact(&mut bytes).map_err(io)?;
        Ok(bytes)
    }
}

/// One segment to interleaved stereo, exactly its authored sample count. Each segment is its own XMA stream (own
/// decoder priming), as the mpf plays them; decoded back to back they are gapless.
fn decode_segment(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let segment = mus::segment(bytes, 0).map_err(|e| e.to_string())?;
    let chunks: Vec<Vec<u8>> = segment.blocks.iter().map(|b| bytes[b.data_range()].to_vec()).collect();
    // ponytail: lenient, as resident samples decode: an XMA frame ffmpeg can't parse plays as silence (padded below)
    // instead of stopping the song.
    let mut pcm = ffmpeg::lenient(|| ffmpeg::decode_chain(&chunks, 2, RATE)).map_err(|e| e.message)?;
    pcm.resize(segment.num_samples() as usize * 2, 0);
    Ok(pcm.into_iter().map(|s| f32::from(s) / 32768.).collect())
}

/// A file through `ffmpeg ... -f s16le -`, read as it decodes. Killed when dropped.
struct FileStream {
    child: Child,
    carry: Vec<u8>,
}
impl FileStream {
    fn open(path: &Path) -> Result<Self, String> {
        let child = ffmpeg::command()
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-vn", "-f", "s16le", "-ac", "2", "-ar", "44100", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", ffmpeg::program().display()))?;
        Ok(Self { child, carry: Vec::new() })
    }
    /// Up to a quarter second; None at the end.
    fn next(&mut self) -> Option<Result<Vec<f32>, String>> {
        let mut buffer = vec![0u8; RATE as usize];
        let stdout = self.child.stdout.as_mut()?;
        match stdout.read(&mut buffer) {
            Ok(0) => None,
            Ok(n) => {
                self.carry.extend_from_slice(&buffer[..n]);
                let whole = self.carry.len() / 4 * 4;
                let pcm = self.carry[..whole].chunks_exact(2).map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.).collect();
                self.carry.drain(..whole);
                Some(Ok(pcm))
            }
            Err(error) => Some(Err(error.to_string())),
        }
    }
}
impl Drop for FileStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum Playing {
    Disc { next: usize, end: usize },
    File(FileStream),
}

fn run(live: LivePcm, orders: mpsc::Receiver<Order>, reports: mpsc::Sender<Report>, disc_path: Option<PathBuf>) {
    let mut disc: Option<Disc> = None;
    let mut current: Option<(u64, Playing)> = None;
    // Decoded, not yet queued: at most one disc segment (~12 s) or a file's quarter seconds.
    let mut pending: VecDeque<f32> = VecDeque::new();
    // Decoded to the end; reported once it has played out.
    let mut draining: Option<u64> = None;
    let mut gain = 1.0;
    loop {
        let idle = current.is_none() && pending.is_empty() && draining.is_none();
        let order = if idle {
            match orders.recv() {
                Ok(order) => Some(order),
                Err(_) => return,
            }
        } else {
            match orders.try_recv() {
                Ok(order) => Some(order),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        };
        match order {
            Some(Order::Play { id, source, cut, gain: song_gain }) => {
                if cut {
                    live.clear();
                    pending.clear();
                }
                (draining, gain) = (None, song_gain);
                current = match source {
                    Source::Disc(song) => Some((id, Playing::Disc { next: song * SEGMENTS_PER_SONG, end: (song + 1) * SEGMENTS_PER_SONG })),
                    Source::File(path) => match FileStream::open(&path) {
                        Ok(stream) => Some((id, Playing::File(stream))),
                        Err(error) => {
                            let _ = reports.send(Report::Failed(id, error));
                            None
                        }
                    },
                };
                continue;
            }
            Some(Order::Stop) => {
                live.clear();
                pending.clear();
                (current, draining) = (None, None);
                continue;
            }
            None => {}
        }
        // Decode ahead while less than a second waits beyond the queue.
        if pending.len() < 2 * RATE as usize
            && let Some((id, playing)) = current.as_mut()
        {
            let next = match playing {
                Playing::Disc { next, end } if *next < *end => {
                    let segment = *next;
                    *next += 1;
                    let opened = match disc.as_mut() {
                        Some(d) => Ok(d),
                        None => disc_path.as_deref().ok_or_else(|| "no Ipod_Stream.mus".to_string()).and_then(Disc::open).map(|d| disc.insert(d)),
                    };
                    Some(opened.and_then(|d| d.segment(segment)).and_then(|b| decode_segment(&b)))
                }
                Playing::Disc { .. } => None,
                Playing::File(stream) => stream.next(),
            };
            match next {
                // Applied as decoded, so it changes exactly where the songs meet.
                Some(Ok(pcm)) => pending.extend(pcm.into_iter().map(|s| s * gain)),
                Some(Err(error)) => {
                    let _ = reports.send(Report::Failed(*id, error));
                    current = None;
                }
                None => {
                    draining = Some(*id);
                    current = None;
                }
            }
        }
        while live.queued_frames() < QUEUE_FRAMES && !pending.is_empty() {
            let piece: Vec<f32> = pending.drain(..pending.len().min(8192)).collect();
            let _ = live.push(&piece);
        }
        // The next song is asked for as this one's last quarter second plays.
        if let Some(id) = draining.filter(|_| pending.is_empty() && live.queued_frames() < RATE as usize / 4) {
            let _ = reports.send(Report::Done(id));
            draining = None;
        } else if current.is_some() && pending.len() >= 2 * RATE as usize || pending.is_empty() || live.queued_frames() >= QUEUE_FRAMES {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

// ---------------------------------------------------------------- Bevy side
#[derive(Component)]
pub(crate) struct MusicSink;

/// The music as the mixer reads it: the worker's LivePcm through a [`Deck`].
#[derive(Asset, TypePath, Clone)]
pub(crate) struct MusicStream {
    live: LivePcm,
    stopped: Arc<AtomicBool>,
}
impl Decodable for MusicStream {
    type DecoderItem = f32;
    type Decoder = Deck<LivePlayback>;
    fn decoder(&self) -> Self::Decoder {
        Deck::new(self.live.decoder(), self.stopped.clone())
    }
}

/// Retail's tape stop, on stereo frames at RATE. MusicCore's PAUSE ramps the song's PATH pitchmult to 0 and RESUME back
/// to 4096 over TAPE_FRAMES; PATH_rampfunc (82959FF0) starts each ramp from the current value and always takes the full
/// time, and curve 1 is a straight line (8295A380). Pitchmult is the voice's linear playback rate (SetPitchMult 8295EFD8
/// sends pitchmult / 4096 to RwAudioCore's resampler), and at rate 0 the resampler repeats the sample it stopped on, so
/// the song freezes in place and RESUME carries on from there. Nothing touches the volume.
pub(crate) struct Deck<I> {
    src: I,
    stopped: Arc<AtomicBool>,
    target: f32,
    /// The ramp under way: the rate it left from, and frames into it.
    from: f32,
    step: u32,
    rate: f32,
    /// The playhead sits `pos` (0..1) of the way from frame `a` to frame `b`.
    a: [f32; 2],
    b: [f32; 2],
    pos: f32,
    right: Option<f32>,
}
impl<I: Iterator<Item = f32>> Deck<I> {
    pub(crate) fn new(src: I, stopped: Arc<AtomicBool>) -> Self {
        Self { src, stopped, target: 1., from: 1., step: TAPE_FRAMES, rate: 1., a: [0.; 2], b: [0.; 2], pos: 0., right: None }
    }
}
impl<I: Iterator<Item = f32>> Iterator for Deck<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        let target = if self.stopped.load(Ordering::Relaxed) { 0. } else { 1. };
        if target != self.target {
            (self.target, self.from, self.step) = (target, self.rate, 0);
        }
        if self.step < TAPE_FRAMES {
            self.step += 1;
            let ticked = self.step / TAPE_STEP * TAPE_STEP;
            self.rate = self.from + (target - self.from) * ticked as f32 / TAPE_FRAMES as f32;
            if self.step == TAPE_FRAMES {
                self.rate = target;
                // ponytail: back at full speed the sub-sample phase is dropped (a < 1-sample skip) so steady play
                // passes samples through untouched instead of interpolating forever.
                if target == 1. {
                    self.pos = 0.;
                }
            }
        }
        // ponytail: linear interpolation between neighbouring frames, the cheapest resampler.
        let at = |c: usize| self.a[c] + (self.b[c] - self.a[c]) * self.pos;
        let (left, right) = (at(0), at(1));
        self.pos += self.rate;
        while self.pos >= 1. {
            self.pos -= 1.;
            self.a = self.b;
            self.b = [self.src.next()?, self.src.next()?];
        }
        self.right = Some(right);
        Some(left)
    }
}
impl RodioSource for Deck<LivePlayback> {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// What is on screen, as retail's music audio state: the music field of the FE audio-state collection a screen posts in
/// cMsgFEAudioStateChange (handler 8248AA10 -> MusicSystem::OnGameStateChange 82499468). 5 (crossbar, replay2) is PAUSE,
/// the tape stop; 1 (idle), 3 (cac) and 9 (musicplayer) RESUME after it, the spin-up; 0 (options, loading and every
/// other screen without one) changes nothing. The engine's other pause screens open over the crossbar (`menu_open`) and
/// count as it, as retail's state-less screens leave the crossbar's stop in place.
fn music_state(menu_open: bool, panel: Option<crate::music_ui::Panel>, replay: bool, customiser: bool, loading: bool) -> u8 {
    match panel {
        Some(crate::music_ui::Panel::Player) => 9,
        Some(crate::music_ui::Panel::AudioSettings) => 0,
        None if customiser => 3,
        None if loading => 0,
        None if menu_open || replay => 5,
        None => 1,
    }
}

#[derive(Resource)]
pub(crate) struct Music {
    pub disc: Vec<Track>,
    pub custom: Vec<Track>,
    /// The game's songs, then the folder's (Soundtrack: Both).
    both: Vec<Track>,
    orders: mpsc::Sender<Order>,
    reports: Mutex<mpsc::Receiver<Report>>,
    libraries: Mutex<mpsc::Receiver<(Vec<Track>, Vec<Track>)>>,
    /// The folder scan running, if one is (a rescan waits for it: its tags would be lost).
    scanning: Option<thread::JoinHandle<()>>,
    /// (order id, track key) playing.
    pub playing: Option<(u64, String)>,
    next_id: u64,
    /// Shuffle: keys still to play this round.
    bag: Vec<String>,
    last: Option<String>,
    rng: u64,
    /// Tracks that would not decode; skipped for the session.
    broken: Vec<String>,
    /// The Music Player's Play: this track now.
    pub play_now: Option<String>,
    /// Next Track (the session marker panel's LB + D-pad Right): cut to the next song.
    pub skip: bool,
    /// Restart Song (the panel's LB + D-pad Left): the song again from its start.
    pub restart: bool,
    /// The track that just started (music_ui shows its chyron).
    pub started: Option<Track>,
    /// The Deck's tape stop (true: stopped or stopping).
    tape: Arc<AtomicBool>,
    /// GAIN of the song last ordered.
    gain: f32,
}
impl Music {
    /// The songs that play: My Music (or Both) while the folder has any and it is chosen, else the game's.
    pub fn library(&self, audio: &AudioSettings) -> &[Track] {
        match (audio.my_music && !self.custom.is_empty(), audio.both) {
            (false, _) => &self.disc,
            (true, false) => &self.custom,
            (true, true) => &self.both,
        }
    }
    fn set_libraries(&mut self, disc: Vec<Track>, custom: Vec<Track>) {
        self.both = disc.iter().chain(&custom).cloned().collect();
        (self.disc, self.custom) = (disc, custom);
    }
    /// Bug 10-17 #36 (not retail): reads `folder` again in the background (the Music Player calls this as it opens),
    /// so songs added while the game runs are found. Disc songs and already-tagged files are kept, not re-read.
    pub fn rescan(&mut self, folder: PathBuf) {
        if self.scanning.as_ref().is_some_and(|s| !s.is_finished()) {
            return;
        }
        // What the last scan sent and `sequence` has not taken yet would go with its channel.
        while let Ok((disc, custom)) = self.libraries.get_mut().unwrap_or_else(|p| p.into_inner()).try_recv() {
            self.set_libraries(disc, custom);
        }
        let (found, libraries) = mpsc::channel();
        let (disc, known) = (self.disc.clone(), self.custom.clone());
        let spawned = thread::Builder::new().name("skate-music-scan".into()).spawn(move || scan(Path::new(""), &folder, Some(disc), &known, &found));
        match spawned {
            Ok(handle) => (self.libraries, self.scanning) = (Mutex::new(libraries), Some(handle)),
            Err(error) => warn!("Music: no rescan thread: {error}"),
        }
    }
    /// The folder has songs (Audio Settings shows its Soundtrack row).
    pub fn has_custom(&self) -> bool {
        !self.custom.is_empty()
    }
    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
    /// Shuffle, TU3 825E46B0 (the Next Track pick 8249C230, and a song's end 8249C888 when 8249A270 has none): a random
    /// song not played this round; once all have played the flags clear and any song can follow, the last one too.
    fn next(&mut self, enabled: &[String]) -> Option<String> {
        self.bag.retain(|k| enabled.contains(k));
        if self.bag.is_empty() {
            self.bag = enabled.to_vec();
            for i in (1..self.bag.len()).rev() {
                let j = (self.random() % (i as u64 + 1)) as usize;
                self.bag.swap(i, j);
            }
        }
        self.bag.pop()
    }
    fn start(&mut self, track: Track, cut: bool) {
        self.next_id += 1;
        self.gain = match track.source {
            Source::Disc(song) => GAIN[song],
            Source::File(_) => 1.,
        };
        let _ = self.orders.send(Order::Play { id: self.next_id, source: track.source.clone(), cut, gain: self.gain });
        self.playing = Some((self.next_id, track.key.clone()));
        self.last = Some(track.key.clone());
        self.started = Some(track);
    }
    fn stop(&mut self) {
        if self.playing.take().is_some() {
            let _ = self.orders.send(Order::Stop);
        }
    }
}

pub(crate) struct MusicPlugin;
impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<MusicStream>()
            .add_systems(Startup, start)
            .add_systems(Update, sequence.before(crate::apt_screen::render));
    }
}

fn start(mut commands: Commands, config: Res<crate::config::Config>, mut sources: ResMut<Assets<MusicStream>>) {
    let live = match LivePcm::new(2, RATE, QUEUE_FRAMES + RATE as usize) {
        Ok(live) => live,
        Err(error) => return error!("Music: {error}"),
    };
    let tape = Arc::new(AtomicBool::new(false));
    let stream = MusicStream { live: live.clone(), stopped: tape.clone() };
    commands.spawn((AudioPlayer(sources.add(stream)), PlaybackSettings::ONCE.with_volume(Volume::Linear(0.)), MusicSink));
    let (orders, worker_orders) = mpsc::channel();
    let (worker_reports, reports) = mpsc::channel();
    let disc = disc_stream(&config.asset_root);
    if disc.is_none() {
        warn!("Music: Ipod_Stream.mus not found on the install's disc; only the music folder plays");
    }
    let spawned = thread::Builder::new().name("skate-music".into()).spawn(move || run(live, worker_orders, worker_reports, disc));
    if let Err(error) = spawned {
        return error!("Music: no worker thread: {error}");
    }
    let (found, libraries) = mpsc::channel();
    let (assets, folder) = (config.asset_root.clone(), folder());
    let scanning = thread::Builder::new().name("skate-music-scan".into()).spawn(move || scan(&assets, &folder, None, &[], &found)).ok();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    commands.insert_resource(Music {
        disc: Vec::new(),
        both: Vec::new(),
        custom: Vec::new(),
        orders,
        reports: Mutex::new(reports),
        libraries: Mutex::new(libraries),
        scanning,
        playing: None,
        next_id: 0,
        bag: Vec::new(),
        last: None,
        rng: seed | 1,
        broken: Vec::new(),
        play_now: None,
        skip: false,
        restart: false,
        started: None,
        tape,
        gain: 1.,
    });
}

/// Keeps a song playing while Music Volume is above 0 (retail: volume 0 plays no music), at that volume, and stops or
/// spins up the tape as screens open and close.
pub(crate) fn sequence(
    music: Option<ResMut<Music>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    screens: (
        Option<Res<crate::music_ui::MusicUi>>,
        Option<Res<crate::replay::Replay>>,
        Option<Res<crate::customiser::Customiser>>,
        Option<Res<crate::map_transition::MapTransition>>,
    ),
    mut sinks: Query<&mut AudioSink, With<MusicSink>>,
    boot: Option<Res<crate::boot::BootActive>>,
) {
    let (Some(mut music), Some(menu)) = (music, menu) else { return };
    let m = &mut *music;
    // No song while the boot owns the screen. Not from the bootflow collection: it sets the audio FE state 10, not a
    // music state (MUSIC_2026-10-03.md §9.2 correction). 8248AA10 only reaches OnGameStateChange when a slot's music
    // field is non-zero, and bootflow, coachfrank, tiploader, importskater and the movies' slot leave it 0. The boot's
    // non-zero ones: idle (slot 0, the loading wait) 1, firstteammember / teammanagement (67 / 63) 5, cac (15) 3, whose
    // case (0x82499748) selects a playlist by key 0xE7CC11BB107155EC (vt+84, vt+80) unless MusicSystem +1204 is 2 or 7
    // (ctor 0x824984F8: 4), then requests the iPod state 7 with the song 8249A270 picks (824831A8): the boot's
    // Create-a-Skater plays the soundtrack (first boot 10-29 #5). shortcut: retail fades it 250 ms on leaving (OnLeaveCaC
    // 0x82498BA0); this cuts it.
    if boot.is_some_and(|b| b.0) && !screens.2.as_ref().is_some_and(|c| c.open) {
        m.stop();
        return;
    }
    let audio = menu.audio();
    let (ui, replay, customiser, transition) = screens;
    let state = music_state(
        menu.open,
        ui.and_then(|ui| ui.panel()),
        // A scripted replay (the Skate 2 photo challenge's reel) is not the replay editor.
        replay.is_some_and(|r| r.active && !r.scripted),
        customiser.is_some_and(|c| c.open),
        transition.is_some_and(|t| t.busy()),
    );
    if state != 0 {
        m.tape.store(state == 5, Ordering::Relaxed);
    }
    while let Ok((disc, custom)) = m.libraries.get_mut().unwrap_or_else(|p| p.into_inner()).try_recv() {
        m.set_libraries(disc, custom);
    }
    while let Ok(report) = m.reports.get_mut().unwrap_or_else(|p| p.into_inner()).try_recv() {
        let (Report::Done(id) | Report::Failed(id, _)) = &report;
        if m.playing.as_ref().is_none_or(|(playing, _)| playing != id) {
            continue;
        }
        if let (Report::Failed(_, error), Some((_, key))) = (&report, m.playing.as_ref()) {
            warn!("Music: {key} skipped: {error}");
            m.broken.push(key.clone());
        }
        m.playing = None;
    }
    let volume = audio.volumes[MUSIC].min(10);
    // Linear, as retail feeds the Master controller Music x 32767 (the MixMap curve after it is not traced). The worker
    // already scaled the song by its GAIN; PATH_gain (82959CF0) clamps level x gain to 1.
    for mut sink in &mut sinks {
        sink.set_volume(Volume::Linear((f32::from(volume) / 10.).min(1. / m.gain) * duck()));
    }
    // Soundtrack switched: the other library's song stops.
    let switched = m.playing.as_ref().is_some_and(|(_, key)| !m.library(audio).iter().any(|t| t.key == *key));
    if volume == 0 || switched {
        m.stop();
    }
    if volume == 0 {
        (m.play_now, m.skip, m.restart) = (None, false, false);
        return;
    }
    // 8249C168 on the phonelist's restart flag (+1669): 8249C558 mode 3 clears the playing song (+1144), so
    // UpdateIPodState (8249C6D8) starts the requested one (+1104) again from its start. The last-shown song (+1384) is
    // left, so 8249A7E8 shows no chyron. ponytail: the stream cut-over inside PATH is UNTRACED; this cuts.
    if std::mem::take(&mut m.restart)
        && let Some(track) = m.last.clone().and_then(|key| m.library(audio).iter().find(|t| t.key == key).cloned())
    {
        m.start(track, true);
        m.started = None;
        return;
    }
    let picked = m.play_now.take().and_then(|key| m.library(audio).iter().find(|t| t.key == key).cloned());
    if let Some(track) = picked {
        return m.start(track, true);
    }
    // 8249C168 on the next flag (+1670): 8249C558 mode 1 with 825E46B0's pick.
    let skip = std::mem::take(&mut m.skip);
    if m.playing.is_some() && !skip {
        return;
    }
    let library = m.library(audio);
    let playable = |t: &&Track| !m.broken.contains(&t.key);
    let mut enabled: Vec<String> = library.iter().filter(playable).filter(|t| !audio.songs_off.contains(&t.key)).map(|t| t.key.clone()).collect();
    // A stale settings file can switch every song off: the Music Player keeps at least one on, so play them all.
    if enabled.is_empty() {
        enabled = library.iter().filter(playable).map(|t| t.key.clone()).collect();
    }
    let track = m.next(&enabled).and_then(|key| m.library(audio).iter().find(|t| t.key == key).cloned());
    if let Some(track) = track {
        if skip && m.playing.as_ref().is_some_and(|(_, key)| *key == track.key) {
            // Mode 1 zeroes the last-shown song (+1384) first, then finds the pick already playing (+1144): the song
            // plays on and the chyron shows it again.
            m.started = Some(track);
        } else {
            // ponytail: the PATH cut-over is UNTRACED; a skip cuts, as the Music Player's pick does.
            m.start(track, skip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-26: Lower Game Sound While Talking halves game sound (-6 dB) while a voice is heard, over 150 ms each
    /// way; with the setting off, or nobody heard, it is full.
    #[test]
    fn ducking_halves_game_sound_while_someone_talks() {
        assert_eq!(duck(), 1.);
        ease_duck(true, true, 0.075);
        assert!((duck() - 0.75).abs() < 1e-6, "halfway after 75 ms: {}", duck());
        (0..10).for_each(|_| ease_duck(true, true, 0.02));
        assert_eq!(duck(), 0.5);
        ease_duck(true, false, 0.2);
        assert_eq!(duck(), 1., "the setting off");
        ease_duck(true, true, 0.2);
        ease_duck(false, true, 0.2);
        assert_eq!(duck(), 1., "nobody heard");
    }

    fn music() -> Music {
        let (orders, _) = mpsc::channel();
        let (_, reports) = mpsc::channel();
        let (_, libraries) = mpsc::channel();
        Music {
            disc: Vec::new(),
            both: Vec::new(),
            custom: Vec::new(),
            orders,
            reports: Mutex::new(reports),
            libraries: Mutex::new(libraries),
            scanning: None,
            playing: None,
            next_id: 0,
            bag: Vec::new(),
            last: None,
            rng: 0x9e37_79b9_7f4a_7c15,
            broken: Vec::new(),
            play_now: None,
            skip: false,
            restart: false,
            started: None,
            tape: Arc::default(),
            gain: 1.,
        }
    }

    /// Not retail (user 10-11): the Soundtrack row's third choice, Both, shuffles the folder in with the game's songs.
    #[test]
    fn the_soundtrack_row_steps_game_my_music_both_and_both_plays_every_song() {
        let track = |key: &str| Track { key: key.into(), artist: String::new(), title: String::new(), album: String::new(), source: Source::File(key.into()) };
        let mut m = music();
        m.set_libraries(vec![track("disc:a"), track("disc:b")], vec![track("file:c")]);
        let keys = |m: &Music, a: &AudioSettings| m.library(a).iter().map(|t| t.key.clone()).collect::<Vec<_>>();
        let mut audio = AudioSettings { my_music: false, ..AudioSettings::default() };
        assert_eq!(keys(&m, &audio), ["disc:a", "disc:b"]);
        step_soundtrack(&mut audio, true);
        assert_eq!(keys(&m, &audio), ["file:c"]);
        step_soundtrack(&mut audio, true);
        assert_eq!(keys(&m, &audio), ["disc:a", "disc:b", "file:c"]);
        step_soundtrack(&mut audio, true);
        assert!(!audio.my_music && !audio.both, "wraps to Game Music");
        step_soundtrack(&mut audio, false);
        assert!(audio.my_music && audio.both, "Left from Game Music is Both");
        // An empty folder plays the game's songs whatever the row says.
        m.set_libraries(vec![track("disc:a")], Vec::new());
        assert_eq!(keys(&m, &audio), ["disc:a"]);
    }

    /// Bug 10-17 #36: a song added while the game runs is found by the rescan the Music Player starts; known ones
    /// keep what they had (no ffmpeg run for them).
    #[test]
    fn a_rescan_finds_songs_added_while_running() {
        let dir = std::env::temp_dir().join(format!("skate-music-rescan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Old - One.mp3"), b"").unwrap();
        let mut m = music();
        let mut known = file_track(&dir, &dir.join("Old - One.mp3"), Default::default());
        known.album = "Tagged".into();
        m.set_libraries(Vec::new(), vec![known.clone()]);
        std::fs::write(dir.join("New - Two.mp3"), b"").unwrap();
        m.rescan(dir.clone());
        m.scanning.take().unwrap().join().unwrap();
        while let Ok((disc, custom)) = m.libraries.get_mut().unwrap().try_recv() {
            m.set_libraries(disc, custom);
        }
        let _ = std::fs::remove_dir_all(&dir);
        let keys: Vec<_> = m.custom.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, ["file:New - Two.mp3", "file:Old - One.mp3"]);
        assert_eq!(m.custom[1], known, "a known song is kept as it was");
    }

    /// TU3 8249C168: Restart Song plays the same song again with no chyron; Next Track cuts to the pick and shows it,
    /// or, when the pick is the song already playing, plays on and shows it again.
    #[test]
    fn the_panels_restart_and_next_follow_8249c168() {
        use bevy::ecs::system::RunSystemOnce;
        let track = |key: &str| Track { key: key.into(), artist: String::new(), title: String::new(), album: String::new(), source: Source::File(key.into()) };
        let mut world = World::new();
        let mut m = music();
        m.set_libraries(vec![track("disc:a")], Vec::new());
        world.insert_resource(m);
        world.insert_resource(crate::graphics_menu::test_menu());
        let run = |world: &mut World| world.run_system_once(sequence).unwrap();
        run(&mut world);
        let first = world.resource::<Music>().playing.clone().unwrap();
        world.resource_mut::<Music>().started = None;
        world.resource_mut::<Music>().restart = true;
        run(&mut world);
        let m = world.resource::<Music>();
        assert_eq!(m.playing.as_ref().map(|p| &p.1), Some(&first.1), "the same song");
        assert_ne!(m.playing.as_ref().map(|p| p.0), Some(first.0), "started again");
        assert!(m.started.is_none(), "no chyron on a restart");
        let id = m.playing.as_ref().unwrap().0;
        world.resource_mut::<Music>().skip = true;
        run(&mut world);
        let m = world.resource::<Music>();
        assert_eq!(m.playing.as_ref().map(|p| p.0), Some(id), "the only song is the pick: it plays on");
        assert!(m.started.is_some(), "and the chyron shows it again");
        world.resource_mut::<Music>().set_libraries(vec![track("disc:a"), track("disc:b")], Vec::new());
        world.resource_mut::<Music>().bag = vec!["disc:b".into()];
        world.resource_mut::<Music>().skip = true;
        run(&mut world);
        let m = world.resource::<Music>();
        assert_eq!(m.playing.as_ref().map(|p| p.1.as_str()), Some("disc:b"), "cut to the pick");
        assert_eq!(m.started.as_ref().map(|t| t.key.as_str()), Some("disc:b"));
    }

    #[test]
    fn shuffle_plays_every_song_once_a_round() {
        let mut m = music();
        let songs: Vec<String> = (0..5).map(|i| i.to_string()).collect();
        // 825E46B0 clears its played flags once all are set: nothing keeps the last song from opening the next round.
        let mut repeats = 0;
        let mut last = None;
        for _ in 0..200 {
            let mut round: Vec<String> = (0..5).map(|_| m.next(&songs).unwrap()).collect();
            repeats += usize::from(last.as_ref() == Some(&round[0]));
            last = round.last().cloned();
            round.sort();
            assert_eq!(round, songs);
        }
        assert!(repeats > 0, "a round may open with the song that just played");
        assert_eq!(m.next(&["only".into()]), Some("only".into()), "one song repeats");
    }

    #[test]
    fn tags_then_file_names_then_retail_unknowns() {
        let tags = parse_tags(";FFMETADATA1\nTITLE=Debaser\nartist=Pixies\nalbum=Doolittle \\= 1989\nencoder=Lavf\n");
        assert_eq!(tags, ["Pixies".to_string(), "Debaser".into(), "Doolittle = 1989".into()]);
        let folder = Path::new("C:/music");
        let named = file_track(folder, Path::new("C:/music/Mix/Joy Division - Disorder.flac"), Default::default());
        assert_eq!((named.artist.as_str(), named.title.as_str(), named.album.as_str()), ("Joy Division", "Disorder", UNKNOWN_ALBUM));
        assert_eq!(named.key, "file:Mix/Joy Division - Disorder.flac");
        let bare = file_track(folder, Path::new("C:/music/track01.mp3"), Default::default());
        assert_eq!((bare.artist.as_str(), bare.title.as_str()), (UNKNOWN_ARTIST, "track01"));
    }

    #[test]
    fn the_folder_is_created_with_its_readme_and_lists_audio_only() {
        let root = std::env::temp_dir().join(format!("skate-music-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(music_files(&root).is_empty());
        assert!(root.join("README.txt").is_file());
        std::fs::create_dir_all(root.join("Album")).unwrap();
        for name in ["a.MP3", "Album/b.flac", "cover.jpg", "notes.txt"] {
            std::fs::write(root.join(name), b"").unwrap();
        }
        let names: Vec<String> = music_files(&root).iter().map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, ["Album/b.flac", "a.MP3"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Plays `frames` frames of a source whose every frame holds its own index, so the Deck's output is its playhead.
    fn playhead(deck: &mut Deck<impl Iterator<Item = f32>>, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| (deck.next().unwrap(), deck.next().unwrap())).inspect(|(l, r)| assert_eq!(l, r)).map(|(l, _)| l).collect()
    }
    /// The mean rate over the 100 frames from `k`.
    fn rate(play: &[f32], k: usize) -> f32 {
        (play[k + 100] - play[k]) / 100.
    }

    /// MusicCore's PAUSE / RESUME: pitchmult falls in a straight line to 0 over 0.6 s, in 12 service ticks of 50 ms,
    /// and holds, then rises back the same way from the same place; a ramp reversed midway starts from where the rate
    /// is and still takes 0.6 s.
    #[test]
    fn tape_stop_follows_retails_pitch_ramp() {
        let (n, s) = (TAPE_FRAMES as usize, TAPE_STEP as usize);
        assert_eq!((n, n / s), (26_460, 12), "0.6 s at 44.1 kHz in 50 ms ticks");
        let stopped = Arc::new(AtomicBool::new(false));
        let mut deck = Deck::new((0u32..).flat_map(|i| [i as f32; 2]), stopped.clone());
        let play = playhead(&mut deck, 1000);
        // Two frames of latency (the pair the playhead sits between), then frames pass through untouched.
        assert!(play[2..].windows(2).all(|w| w[1] - w[0] == 1.) && play[2] == 0.);

        stopped.store(true, Ordering::Relaxed);
        let stop = playhead(&mut deck, n + 1000);
        for k in 0..12 {
            let want = 1. - k as f32 / 12.;
            assert!((rate(&stop, k * s + 100) - want).abs() < 0.005, "tick {k}: rate {}, want {want}", rate(&stop, k * s + 100));
        }
        // The stepped fall covers 13/24 of the ground full speed would, then it holds that sample.
        let travelled = stop[n] - play[999];
        assert!((travelled - n as f32 * 13. / 24.).abs() < 2., "travelled {travelled}");
        assert!(stop[n..].windows(2).all(|w| w[0] == w[1]), "the stopped tape holds still");

        stopped.store(false, Ordering::Relaxed);
        let spin = playhead(&mut deck, n + 1000);
        assert_eq!(spin[0], stop[n], "the song resumes where it stopped");
        for k in 0..12 {
            let want = k as f32 / 12.;
            assert!((rate(&spin, k * s + 100) - want).abs() < 0.005, "tick {k}: rate {}, want {want}", rate(&spin, k * s + 100));
        }
        assert!((spin[n] - spin[0] - n as f32 * 11. / 24.).abs() < 2.);
        assert!(spin[n..].windows(2).all(|w| w[1] - w[0] == 1. && w[0].fract() == 0.), "back to untouched frames");

        stopped.store(true, Ordering::Relaxed);
        let half = playhead(&mut deck, n / 2);
        assert!((rate(&half, n / 2 - 101) - (1. - 5. / 12.)).abs() < 0.005);
        stopped.store(false, Ordering::Relaxed);
        let back = playhead(&mut deck, n + 200);
        assert!((rate(&back, n / 2 + 100) - 0.75).abs() < 0.005, "0.5 -> 1 over the full 0.6 s: {}", rate(&back, n / 2 + 100));
        assert!((rate(&back, n + 50) - 1.).abs() < 1e-6);
    }

    /// Retail's music audio states: the Start menu and the replay editor stop the tape, gameplay and the Music Player
    /// spin it up, Audio Settings (options, no music state) leaves it as it was.
    #[test]
    fn screens_stop_and_start_the_tape_like_retail() {
        use crate::music_ui::Panel::{AudioSettings, Player};
        let state = music_state;
        assert_eq!(state(true, None, false, false, false), 5, "crossbar");
        assert_eq!(state(false, None, true, false, false), 5, "replay2");
        assert_eq!(state(false, None, false, false, false), 1, "idle");
        assert_eq!(state(true, Some(Player), false, false, false), 9, "musicplayer");
        assert_eq!(state(true, Some(AudioSettings), false, false, false), 0, "options");
        assert_eq!(state(true, None, false, true, false), 3, "cac");
        assert_eq!(state(true, None, false, false, true), 0, "loading");
    }

    /// SKATE_MUSIC_WAV=<file.wav>: writes the tape stop as heard, 2 s of Pixies' Debaser off the disc (a chord without
    /// SKATE3_ASSET_ROOT), the 0.6 s stop, 1 s stopped, the 0.6 s spin-up and 2 s more.
    #[test]
    fn tape_stop_wav() {
        let Some(out) = std::env::var_os("SKATE_MUSIC_WAV").map(PathBuf::from) else { return };
        let song = SONGS.iter().position(|s| *s == "Pixies_Debaser").unwrap();
        let disc = std::env::var_os("SKATE3_ASSET_ROOT").and_then(|a| disc_stream(Path::new(&a)));
        let pcm = disc.and_then(|p| decode_segment(&Disc::open(&p).ok()?.segment(song * SEGMENTS_PER_SONG + 1).ok()?).ok());
        let pcm = pcm.unwrap_or_else(|| {
            let tone = |t: f32| [220., 277.2, 329.6].iter().map(|f| 0.2 * (t * f * std::f32::consts::TAU).sin()).sum::<f32>();
            (0..8 * RATE).flat_map(|i| [tone(i as f32 / RATE as f32); 2]).collect()
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let mut deck = Deck::new(pcm.into_iter(), stopped.clone());
        let mut wav: Vec<f32> = Vec::new();
        for (seconds, stop) in [(2., false), (1.6, true), (2.6, false)] {
            stopped.store(stop, Ordering::Relaxed);
            wav.extend(deck.by_ref().take(2 * (seconds * RATE as f32) as usize));
        }
        let mut bytes = Vec::new();
        let data = (wav.len() * 2) as u32;
        for chunk in [&b"RIFF"[..], &(36 + data).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(), &2u16.to_le_bytes()] {
            bytes.extend_from_slice(chunk);
        }
        for chunk in [&RATE.to_le_bytes()[..], &(RATE * 4).to_le_bytes(), &4u16.to_le_bytes(), &16u16.to_le_bytes(), b"data", &data.to_le_bytes()] {
            bytes.extend_from_slice(chunk);
        }
        bytes.extend(wav.iter().flat_map(|s| ((s.clamp(-1., 1.) * 32767.) as i16).to_le_bytes()));
        std::fs::write(&out, bytes).unwrap();
    }

    /// The disc's first song: two consecutive segments decode to exactly their authored sample counts, with audio in
    /// them, and its metadata reads from the language table (SKATE3_ASSET_ROOT + ffmpeg + the install's disc).
    #[test]
    fn disc_song_segments_decode_to_their_sample_counts() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(PathBuf::from) else { return };
        let Some(path) = disc_stream(&assets) else { return eprintln!("no disc beside the install; skipped") };
        let mut disc = Disc::open(&path).unwrap();
        for n in [0, 1, SEGMENTS_PER_SONG] {
            let bytes = disc.segment(n).unwrap();
            let segment = mus::segment(&bytes, 0).unwrap();
            let pcm = decode_segment(&bytes).unwrap();
            assert_eq!(pcm.len() as u64, 2 * segment.num_samples());
            assert!(pcm.iter().any(|s| s.abs() > 0.01), "segment {n} is silent");
        }
        let tracks = disc_tracks(&assets);
        assert_eq!(tracks.len(), 46);
        let first = &tracks[0];
        assert_eq!((first.artist.as_str(), first.title.as_str(), first.album.as_str()), ("3 Inches Of Blood", "Battles And Brotherhood", "Here Waits Thy Doom"));
        // Song 2 (Agent Orange, 109 s) starts at segment 30; the walk found every start before it.
        assert!(disc.starts.len() > SEGMENTS_PER_SONG);
    }

    /// First boot 10-29 #5: the boot's Create-a-Skater (cac, 3) plays the soundtrack; the boot's other screens play none.
    #[test]
    fn the_boots_create_a_skater_plays_a_song() {
        use bevy::ecs::system::RunSystemOnce;
        let track = Track { key: "disc:a".into(), artist: String::new(), title: String::new(), album: String::new(), source: Source::File("a".into()) };
        let mut world = World::new();
        let mut m = music();
        m.set_libraries(vec![track], Vec::new());
        world.insert_resource(m);
        world.insert_resource(crate::graphics_menu::test_menu());
        world.insert_resource(crate::boot::BootActive(true));
        let mut parts = crate::customiser_parts::Parts::default();
        parts.library.defaults.insert("male".into(), serde_json::json!({}));
        let mut cas = crate::customiser::Customiser::for_test(&parts, Vec::new(), "male");
        cas.open = false;
        world.insert_resource(cas);
        let run = |world: &mut World| world.run_system_once(sequence).unwrap();
        run(&mut world);
        assert!(world.resource::<Music>().playing.is_none(), "no song behind the boot's screens");
        world.resource_mut::<crate::customiser::Customiser>().open = true;
        run(&mut world);
        assert!(world.resource::<Music>().playing.is_some(), "the boot's Create-a-Skater plays the soundtrack");
        assert!(!world.resource::<Music>().tape.load(Ordering::Relaxed), "cac (3) spins the tape up");
        world.resource_mut::<crate::customiser::Customiser>().open = false;
        run(&mut world);
        assert!(world.resource::<Music>().playing.is_none(), "back in the boot: stopped");
    }
}
