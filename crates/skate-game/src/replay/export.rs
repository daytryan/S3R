//! The export (U1: retail's upload flow, rendering an MP4 instead of uploading). The edited timeline is stepped one
//! encoded frame at a time, honouring the Speed channel as retail's encoder does (GetEncodedLength 0x8250b590:
//! slow clips encode longer). hud.rs renders each step through its own export camera at the chosen size (the grade
//! included, the HUD not) and hands it here with the frame's sound; this module stamps the watermark and feeds the
//! Media Foundation writer (mf.rs). The sound is the game's own mix (research Q5: retail's encoder took the master
//! mix offline): a private player-audio worker ([`Sound`]) re-renders the recorded audio records at the export's
//! speed, one 60 Hz record per tick, exactly as the live worker takes them.
use super::{
    Replay,
    editor::{Editor, MAX_ENCODED_SECONDS},
    mf::{CHANNELS, Mp4Writer},
};
use crate::skate_audio::{
    PlayerAudioObservation,
    player::{OFFLINE_BLOCK_FRAMES, OFFLINE_RATE, OfflineRender},
};
use crate::weather_audio::replay::{OfflineWeather, ReplayThunder, WeatherTick};
use image::RgbaImage;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, mpsc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

/// The tags screen's rows relabelled (plan Task 10): Resolution, Frame Rate, HD (retail's encoder rate flag,
/// 175 / 700, research Q8) and Watermark (Off / Default / Pro).
pub(crate) const RESOLUTIONS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
pub(crate) const RATES: [u32; 2] = [30, 60];
pub(crate) const SAMPLE_RATE: u32 = OFFLINE_RATE;
/// Stereo frames per 60 Hz audio record.
const TICK_SAMPLES: u64 = SAMPLE_RATE as u64 / 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Watermark {
    Off,
    Default,
    Pro,
}
impl Watermark {
    /// research Q2: logo_watermark_easy / logo_watermark / logo_watermark_pro (0x82507ee8).
    pub fn texture(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Default => Some("textures/logo_watermark.png"),
            Self::Pro => Some("textures/logo_watermark_pro.png"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub watermark: Watermark,
    pub hd: bool,
}
impl ExportSettings {
    /// `Tags.SetTags(style, type, hd, watermark)` from the relabelled tags screen.
    pub fn from_tags(resolution: usize, rate: usize, hd: usize, watermark: usize) -> Self {
        let (width, height) = RESOLUTIONS[resolution.min(RESOLUTIONS.len() - 1)];
        Self {
            width,
            height,
            fps: RATES[rate.min(RATES.len() - 1)],
            watermark: [Watermark::Off, Watermark::Default, Watermark::Pro][watermark.min(2)],
            hd: hd != 0,
        }
    }
    /// RULING (build-progress, Task 10): retail's 175 / 700 kbps were for its small VP6 video; H.264 here gets
    /// 0.1 / 0.2 bits per pixel per frame (Standard / HD).
    pub fn bitrate(&self) -> u32 {
        let pixels = f64::from(self.width) * f64::from(self.height) * f64::from(self.fps);
        (pixels * if self.hd { 0.2 } else { 0.1 }) as u32
    }
}

/// The encoded length in seconds, unrounded (the sum GetEncodedLength rounds; speed ramps included).
pub(crate) fn encoded_length(editor: &Editor, timeline_seconds: f32) -> f32 {
    editor.encoded_length(timeline_seconds)
}

pub(crate) fn frame_count(encoded_seconds: f32, fps: u32) -> u32 {
    (encoded_seconds * fps as f32).round() as u32
}

/// The playhead (0..1) for encoded frame `frame`: walk the clips, each lasting its length / speed (an eased Speed
/// key's ramp walks its midpoint steps, Editor::ramp_playhead).
pub(crate) fn playhead_for_frame(editor: &Editor, frame: u32, fps: u32, timeline_seconds: f32) -> f32 {
    let mut left = frame as f32 / fps as f32;
    let mut last = editor.in_mark.min(editor.out_mark);
    for (a, b, speed) in editor.clips() {
        let length = editor.span_seconds(a, b, speed, timeline_seconds);
        if left < length {
            if editor.ramped(a) {
                return editor.ramp_playhead(a, b, left, timeline_seconds);
            }
            let speed = speed.max(1) as f32 * 0.01;
            return a + left * speed / timeline_seconds.max(1e-6);
        }
        left -= length;
        last = b;
    }
    last
}

/// U2 / research Q8: over 60 s retail refuses (the red length text and ID_REPLAY_LENGTH_EXCEEDED); nothing to
/// encode is "No footage to display" (review M9). Err is the popup's text id.
pub(crate) fn check(editor: &Editor, timeline_seconds: f32) -> Result<(), &'static str> {
    if timeline_seconds <= 0. || encoded_length(editor, timeline_seconds) <= 0. {
        return Err("ID_SKATEREEL_BROWSER_NO_FOOTAGE");
    }
    if editor.encoded_seconds(timeline_seconds) > MAX_ENCODED_SECONDS {
        return Err("ID_REPLAY_LENGTH_EXCEEDED");
    }
    Ok(())
}

/// The 60 Hz audio ticks inside encoded frame `frame`: tick t starts at sample t * 800.
pub(crate) fn audio_ticks(frame: u32, fps: u32) -> std::ops::Range<u32> {
    (frame * 60).div_ceil(fps)..((frame + 1) * 60).div_ceil(fps)
}

/// The recorded audio records for encoded frame `frame`, one per tick: each tick's playhead walks the clips like
/// the video's (60 Hz in place of the frame rate) and carries its clip's speed as the time scale.
pub(crate) fn audio_records(
    replay: &Replay,
    editor: &Editor,
    frame: u32,
    fps: u32,
    timeline_seconds: f32,
    sent: &mut Option<usize>,
) -> Vec<Option<PlayerAudioObservation>> {
    audio_ticks(frame, fps)
        .map(|tick| {
            let playhead = playhead_for_frame(editor, tick, 60, timeline_seconds);
            replay.audio_at(playhead, editor.time_scale(playhead), sent)
        })
        .collect()
}

/// The weather for encoded frame `frame`, one tick per 60 Hz audio tick, walked like [`audio_records`].
pub(crate) fn weather_ticks(
    replay: &Replay,
    editor: &Editor,
    frame: u32,
    fps: u32,
    timeline_seconds: f32,
    thunder: &mut ReplayThunder,
) -> Vec<WeatherTick> {
    audio_ticks(frame, fps)
        .map(|tick| {
            let playhead = playhead_for_frame(editor, tick, 60, timeline_seconds);
            crate::weather_audio::replay::export_tick(replay, playhead, editor.time_scale(playhead), thunder)
        })
        .collect()
}

/// What [`Mix`] drives: [`Rebooting`], or a test double.
trait Render {
    fn frame(&mut self, observation: &PlayerAudioObservation);
    fn block(&mut self) -> Vec<f32>;
}

/// The export renderer's own calls, each of which may fail: the offline worker, or a test double.
trait Source {
    fn frame(&mut self, observation: &PlayerAudioObservation) -> Result<(), String>;
    fn block(&mut self) -> Result<Vec<f32>, String>;
}
impl Source for OfflineRender {
    fn frame(&mut self, observation: &PlayerAudioObservation) -> Result<(), String> {
        OfflineRender::frame(self, observation)
    }
    fn block(&mut self) -> Result<Vec<f32>, String> {
        OfflineRender::block(self)
    }
}

/// Failed frames in a row before a fresh renderer: the live worker's 5 s (`run_game_side`).
const FRAME_REBOOT: u32 = 300;

/// One call a [`Rebooting`] source was given, kept to warm a fresh one.
enum Call {
    Frame(PlayerAudioObservation),
    Block,
}
enum Fail {
    Error(String),
    /// The panic's message and, when the game's panic hook is in, where it was raised.
    Panic(String),
}
impl Fail {
    fn why(&self) -> String {
        match self {
            Fail::Error(error) => error.clone(),
            Fail::Panic(panic) => format!("a panic: {panic}"),
        }
    }
}
/// `f`, with a panic caught (release builds unwind; the panic hook's REPORT_PANIC line only reaches a crash report
/// if the game later exits with a failure, crash_report.rs).
fn guard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, Fail> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Fail::Error(error)),
        Err(payload) => Err(Fail::Panic(crate::crash_report::take_last_panic().unwrap_or_else(|| {
            let message = payload.downcast_ref::<String>().map(String::as_str);
            message.or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("non-string panic payload").to_owned()
        }))),
    }
}
fn run<S: Source>(source: &mut S, call: &Call) -> Result<Vec<f32>, Fail> {
    match call {
        Call::Frame(observation) => guard(|| source.frame(observation)).map(|()| Vec::new()),
        Call::Block => match guard(|| source.block())? {
            block if block.is_empty() => Err(Fail::Error("an empty block".into())),
            block => Ok(block),
        },
    }
}

/// A [`Source`] whose sound never stops (not retail, by request (owner 10-18): retail took its master mix offline
/// with no fallback). A failed or panicking block throws the renderer away and renders that same block on a fresh
/// one, so the output has no gap; a fresh one first replays every call so far (its blocks thrown away), so the voices
/// that were playing still play. A block that fails on the fresh one too is deterministic: silence for that block
/// only, logged with its record tick. A failed frame is skipped as the live worker does, and [`FRAME_REBOOT`] of
/// them in a row reboot; a panicking frame reboots at once. A block that panics the fresh one too starts the search
/// ([`Rebooting::search`]).
/// ponytail: the warm-up replays from the export's start (at most 60 s of records, offline, so no real time lost);
/// replay from the oldest live voice if reboots ever get slow.
struct Rebooting<S, B> {
    /// None after a throw-away, until the next call boots a fresh one.
    source: Option<S>,
    boot: B,
    history: Vec<Call>,
    /// Calls left out when warming one: calls that panicked a fresh renderer too (or every warm-up would panic
    /// again), and the frames the search found a block's panic comes from.
    poison: std::collections::HashSet<usize>,
    /// The history index of the last block that rendered.
    good: usize,
    /// Failed frames in a row.
    frames: u32,
    /// The last record's tick, for the log.
    tick: u64,
    /// A reboot whose boot failed: the rest of the export is silence rather than a boot per block.
    dead: bool,
}
impl<S: Source, B: FnMut() -> Result<S, String>> Rebooting<S, B> {
    fn new(source: S, boot: B) -> Self {
        Self { source: Some(source), boot, history: Vec::new(), poison: Default::default(), good: 0, frames: 0, tick: 0, dead: false }
    }
    /// A fresh source warmed with the first `upto` calls (not `without`; then a warm-up panic is None, not poison).
    fn fresh(&mut self, upto: usize, without: Option<usize>) -> Option<S> {
        'boot: loop {
            if self.dead {
                return None;
            }
            let mut source = match guard(&mut self.boot) {
                Ok(source) => source,
                Err(fail) => {
                    bevy::log::error!("replay export audio: a fresh renderer did not boot ({}); the rest is silent", fail.why());
                    self.dead = true;
                    return None;
                }
            };
            for (i, call) in self.history[..upto].iter().enumerate() {
                if self.poison.contains(&i) || Some(i) == without {
                    continue;
                }
                if let Err(Fail::Panic(panic)) = run(&mut source, call) {
                    if without.is_some() {
                        return None; // a search's candidate: this omission does not work, nothing is learned
                    }
                    bevy::log::error!("replay export audio: call {i} panics every renderer ({panic}); skipped when warming");
                    self.poison.insert(i);
                    continue 'boot;
                }
            }
            return Some(source);
        }
    }
    /// The newest call on the current source, or a fresh one.
    fn attempt(&mut self) -> Result<Vec<f32>, Fail> {
        let at = self.history.len() - 1;
        if self.source.is_none() {
            self.source = self.fresh(at, None);
        }
        match self.source.as_mut() {
            Some(source) => run(source, &self.history[at]),
            None => Err(Fail::Error("no renderer".into())),
        }
    }
    fn call(&mut self, call: Call) -> Option<Vec<f32>> {
        if self.dead {
            return None;
        }
        let is_block = matches!(call, Call::Block);
        if let Call::Frame(observation) = &call {
            self.tick = observation.tick;
        }
        self.history.push(call);
        match self.attempt() {
            Ok(out) => {
                self.frames = 0;
                if is_block {
                    self.good = self.history.len() - 1;
                }
                return Some(out);
            }
            Err(Fail::Error(error)) if !is_block => {
                // A failed frame is skipped, as the live worker does; the voices already playing go on.
                bevy::log::warn!("replay export audio frame (tick {}): {error}", self.tick);
                self.frames += 1;
                if self.frames >= FRAME_REBOOT {
                    (self.source, self.frames) = (None, 0);
                }
                return None;
            }
            Err(_) => self.source = None,
        }
        let fail = match self.attempt() {
            Ok(out) => {
                if is_block {
                    self.good = self.history.len() - 1;
                }
                return Some(out);
            }
            Err(fail) => fail,
        };
        let what = if is_block { "block" } else { "frame" };
        bevy::log::error!("replay export audio: {what} after record tick {} fails on a fresh renderer too: {}", self.tick, fail.why());
        match fail {
            Fail::Panic(_) if is_block => self.search(),
            Fail::Panic(_) => {
                self.poison.insert(self.history.len() - 1);
                self.source = None;
                None
            }
            Fail::Error(_) => None,
        }
    }
    /// A block that panics a warmed renderer too (by request, owner 10-18): warm fresh ones, each leaving out ONE
    /// frame, newest first (the frames since the last good block, then the older ones), and keep the first that
    /// lets the block render; that frame is left out for the rest of the export, logged with its tick. If none does,
    /// the rest of the export is silent and it still finishes. Each frame found is a bug to fix at its source.
    /// ponytail: one boot per older frame at worst (B still cancels the boot); bound the older frames by the voices
    /// still playing if a search ever runs long.
    fn search(&mut self) -> Option<Vec<f32>> {
        let at = self.history.len() - 1;
        let order: Vec<usize> = (self.good..at).rev().chain((0..self.good).rev()).collect();
        for i in order {
            let Call::Frame(observation) = &self.history[i] else { continue };
            if self.poison.contains(&i) {
                continue;
            }
            let tick = observation.tick;
            let Some(mut source) = self.fresh(at, Some(i)) else {
                if self.dead {
                    return None;
                }
                continue;
            };
            if let Ok(out) = run(&mut source, &self.history[at]) {
                bevy::log::error!("replay export audio: the record at tick {tick} panics the renderer; left out of the export");
                self.poison.insert(i);
                (self.source, self.good) = (Some(source), at);
                return Some(out);
            }
        }
        bevy::log::error!("replay export audio: no one record explains the panic after tick {}; the rest is silent", self.tick);
        (self.source, self.dead) = (None, true);
        None
    }
}
impl<S: Source, B: FnMut() -> Result<S, String>> Render for Rebooting<S, B> {
    fn frame(&mut self, observation: &PlayerAudioObservation) {
        self.call(Call::Frame(observation.clone()));
    }
    fn block(&mut self) -> Vec<f32> {
        self.call(Call::Block).unwrap_or_else(|| vec![0.; OFFLINE_BLOCK_FRAMES * CHANNELS as usize])
    }
}

/// The export's audio clock: records go in at their tick's sample, blocks render up to it, each frame takes its
/// own 48000 / fps stereo frames.
struct Mix {
    tick: u64,
    rendered: u64,
    delivered: u64,
    frame_samples: u64,
    /// Rendered, not yet delivered, interleaved stereo (starts at `delivered`).
    buffer: Vec<f32>,
}
impl Mix {
    fn new(fps: u32) -> Self {
        Self { tick: 0, rendered: 0, delivered: 0, frame_samples: u64::from(SAMPLE_RATE / fps.max(1)), buffer: Vec::new() }
    }
    fn fill(&mut self, render: &mut impl Render, until: u64) {
        while self.rendered < until {
            let mut block = render.block();
            if block.is_empty() {
                block = vec![0.; OFFLINE_BLOCK_FRAMES * CHANNELS as usize];
            }
            self.rendered += (block.len() / CHANNELS as usize) as u64;
            self.buffer.extend(block);
        }
    }
    fn frame(&mut self, render: &mut impl Render, records: &[Option<PlayerAudioObservation>]) -> Vec<f32> {
        for record in records {
            self.fill(render, self.tick * TICK_SAMPLES);
            if let Some(record) = record {
                render.frame(record);
            }
            self.tick += 1;
        }
        let end = self.delivered + self.frame_samples;
        self.fill(render, end);
        self.delivered = end;
        self.buffer.drain(..self.frame_samples as usize * CHANNELS as usize).collect()
    }
}

/// The worker of the export before this one, until it has ended (owner 10-18: two audio boots at once could take a
/// low-memory PC down). A new worker waits for it on its own thread; the main thread never joins.
static PREVIOUS: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(None);

/// The export's private player-audio worker (review I2), on its own thread: it boots like the live one, then turns
/// each frame's records into that frame's sound.
pub(crate) struct Sound {
    requests: mpsc::Sender<(Vec<Option<PlayerAudioObservation>>, Vec<WeatherTick>)>,
    /// In a Mutex only for Sync (the HUD resource); one thread uses it.
    replies: std::sync::Mutex<mpsc::Receiver<Result<Vec<f32>, String>>>,
    ready: bool,
    /// Set when this export ends: a boot still loading stops at its next load point.
    stop: Arc<AtomicBool>,
    /// True while the worker waits for the previous export's to end.
    waiting: Arc<AtomicBool>,
}
impl Drop for Sound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Sound {
    pub fn start(assets: PathBuf, fps: u32) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let weather = assets.join(crate::weather_audio::FOLDER);
        let cancelled = stop.clone();
        Self::start_with(fps, Some(weather), stop, move || OfflineRender::new(&assets, &|| cancelled.load(Ordering::Relaxed)))
    }
    /// [`Sound::start`] with the renderer's boot (tests pass a double) and the weather recordings' folder.
    fn start_with<S: Source + 'static, B: FnMut() -> Result<S, String> + Send + 'static>(
        fps: u32,
        weather: Option<PathBuf>,
        stop: Arc<AtomicBool>,
        mut boot: B,
    ) -> Self {
        let (requests, inbox) = mpsc::channel::<(Vec<Option<PlayerAudioObservation>>, Vec<WeatherTick>)>();
        let (outbox, replies) = mpsc::channel();
        let previous = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()).take();
        let waiting = Arc::new(AtomicBool::new(previous.as_ref().is_some_and(|p| !p.is_finished())));
        let (stopped, waited) = (stop.clone(), waiting.clone());
        let spawned = std::thread::Builder::new().name("replay-export-audio".into()).spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            waited.store(false, Ordering::Relaxed);
            if stopped.load(Ordering::Relaxed) {
                return;
            }
            // Not retail, by request (owner 10-18): the first boot is the only refusal (no sound loads at all: its own
            // text). It takes as long as it takes, and a panic in it is that refusal, not a dead worker.
            let source = match guard(&mut boot) {
                Ok(source) => source,
                Err(fail) => {
                    let _ = outbox.send(Err(match fail {
                        Fail::Error(error) => error,
                        Fail::Panic(panic) => format!("the export audio boot panicked: {panic}"),
                    }));
                    return;
                }
            };
            let mut render = Rebooting::new(source, boot);
            // The weather (weather_audio), mixed in lockstep with the game mix when the recordings are installed.
            let bank = weather.map(|dir| crate::weather_audio::Bank::load(&dir));
            let mut weather =
                bank.and_then(|bank| OfflineWeather::new(Arc::new(bank), TICK_SAMPLES, u64::from(SAMPLE_RATE / fps.max(1))));
            let _ = outbox.send(Ok(Vec::new()));
            let mut mix = Mix::new(fps);
            while let Ok((records, ticks)) = inbox.recv() {
                // The ambience ducks under the frame's recorded rain, not the live game's.
                crate::weather_audio::export_rain(ticks.first().map_or(0., |t| t.params.rain));
                let mut chunk = mix.frame(&mut render, &records);
                // A weather mixer that panics goes; the game mix goes on (not retail, by request (owner 10-18)).
                let rain = weather.as_mut().map(|w| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.frame(&ticks))));
                match rain {
                    Some(Ok(rain)) => chunk.iter_mut().zip(rain).for_each(|(a, b)| *a += b),
                    Some(Err(_)) => {
                        let panic = crate::crash_report::take_last_panic().unwrap_or_default();
                        bevy::log::error!("replay export weather panicked ({panic}); the export goes on without it");
                        weather = None;
                    }
                    None => {}
                }
                if outbox.send(Ok(chunk)).is_err() {
                    return;
                }
            }
        });
        let replies = match spawned {
            Ok(handle) => {
                *PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
                replies
            }
            Err(error) => {
                waiting.store(false, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel();
                let _ = tx.send(Err(error.to_string()));
                rx
            }
        };
        Self { requests, replies: std::sync::Mutex::new(replies), ready: false, stop, waiting }
    }
    /// True while the previous export's worker is still ending ("Stopping the previous export...").
    pub fn stopping(&self) -> bool {
        self.waiting.load(Ordering::Relaxed)
    }
    /// Some once the worker has booted (or failed to).
    pub fn ready(&mut self) -> Option<Result<(), String>> {
        if self.ready {
            return Some(Ok(()));
        }
        let Ok(replies) = self.replies.lock() else { return Some(Err("the export audio worker stopped".into())) };
        match replies.try_recv() {
            Ok(Ok(_)) => {
                self.ready = true;
                Some(Ok(()))
            }
            Ok(Err(error)) => Some(Err(error)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("the export audio worker stopped".into())),
        }
    }
    /// The next frame's records; its sound comes back from [`Sound::chunk`].
    pub fn request(&self, records: Vec<Option<PlayerAudioObservation>>) {
        self.request_with_weather(records, Vec::new());
    }
    /// [`Sound::request`] with the frame's weather ticks ([`weather_ticks`]).
    pub fn request_with_weather(&self, records: Vec<Option<PlayerAudioObservation>>, weather: Vec<WeatherTick>) {
        let _ = self.requests.send((records, weather));
    }
    /// The requested frame's sound once it is mixed; None until then (owner 10-18: the main thread never waits on
    /// it, however long a reboot or a slow first frame takes). Err only when the worker is gone.
    pub fn chunk(&self) -> Option<Result<Vec<f32>, String>> {
        let Ok(replies) = self.replies.lock() else { return Some(Err("export audio: poisoned".into())) };
        match replies.try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("the export audio worker stopped".into())),
        }
    }
}

/// One export in flight. Written to `<name>.partial.mp4` and renamed when finished; cancelled or failed, the
/// partial file is deleted. Render speed (10-27): the letterbox, watermark and encoding run on the job's own thread,
/// so the game goes on rendering the next frames meanwhile.
pub(crate) struct Job {
    pub settings: ExportSettings,
    pub path: PathBuf,
    partial: PathBuf,
    /// Frames handed to the encoder.
    pub frame: u32,
    pub frames: u32,
    pub timeline_seconds: f32,
    /// Item 16 extension: Replay Settings' letterbox aspect (0 = off), drawn black into every frame.
    letterbox: f32,
    encoder: Option<Encoder>,
}

/// The encoder thread's ends.
struct Encoder {
    frames: mpsc::Sender<(RgbaImage, Vec<f32>)>,
    /// Frames in the file.
    written: Arc<AtomicU32>,
    /// Set by a cancel: the frames still queued are dropped.
    stop: Arc<AtomicBool>,
    /// The file finished and renamed, or why not. In a Mutex only for Sync (the HUD resource).
    done: std::sync::Mutex<mpsc::Receiver<Result<(), String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Encoder {
    /// Writes `total` frames as they come, each letterboxed (`aspect`) and stamped, then finishes the file and
    /// renames `partial` to `path`.
    fn start(mut writer: Mp4Writer, total: u32, aspect: f32, watermark: Option<RgbaImage>, partial: PathBuf, path: PathBuf) -> Self {
        let (frames, queue) = mpsc::channel::<(RgbaImage, Vec<f32>)>();
        let (report, done) = mpsc::channel();
        let (written, stop) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (count, stopped, failed) = (written.clone(), stop.clone(), report.clone());
        let spawned = std::thread::Builder::new().name("replay-export-video".into()).spawn(move || {
            let encoded = (|| -> Result<(), String> {
                for n in 0..total {
                    let (mut frame, sound) = queue.recv().map_err(|_| "cancelled".to_owned())?;
                    if stopped.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    let bars = letterbox(&mut frame, aspect);
                    if let Some(logo) = &watermark {
                        stamp(&mut frame, logo, bars);
                    }
                    writer.video(frame.as_raw(), n)?;
                    writer.audio(&sound)?;
                    count.store(n + 1, Ordering::Relaxed);
                }
                Ok(())
            })();
            let finished = encoded
                .and_then(|()| writer.finish())
                .and_then(|()| std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display())));
            let _ = report.send(finished);
        });
        let thread = spawned.map_err(|error| failed.send(Err(error.to_string()))).ok();
        Self { frames, written, stop, done: std::sync::Mutex::new(done), thread }
    }
}

impl Job {
    pub fn start(
        dir: &Path,
        name: &str,
        settings: ExportSettings,
        editor: &Editor,
        timeline_seconds: f32,
        watermark: Option<RgbaImage>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        sweep_partials(dir);
        let path = (0..)
            .map(|n| dir.join(if n == 0 { format!("{name}.mp4") } else { format!("{name}-{n}.mp4") }))
            .find(|p| !p.exists())
            .expect("an unused name");
        let partial = path.with_extension("partial.mp4");
        let writer = Mp4Writer::create(&partial, &settings, SAMPLE_RATE)?;
        Ok(Self::with_writer(path, partial, Some(writer), settings, editor, timeline_seconds, watermark))
    }
    fn with_writer(
        path: PathBuf,
        partial: PathBuf,
        writer: Option<Mp4Writer>,
        settings: ExportSettings,
        editor: &Editor,
        timeline_seconds: f32,
        watermark: Option<RgbaImage>,
    ) -> Self {
        let frames = frame_count(encoded_length(editor, timeline_seconds), settings.fps).max(1);
        let letterbox = editor.look.aspect();
        let encoder = writer.map(|writer| Encoder::start(writer, frames, letterbox, watermark, partial.clone(), path.clone()));
        Self { settings, path, partial, frame: 0, frames, timeline_seconds, letterbox, encoder }
    }

    /// The playhead to render for `frame`.
    pub fn playhead(&self, editor: &Editor, frame: u32) -> f32 {
        playhead_for_frame(editor, frame, self.settings.fps, self.timeline_seconds)
    }
    /// Frames in the file.
    pub fn written(&self) -> u32 {
        self.encoder.as_ref().map_or(0, |e| e.written.load(Ordering::Relaxed))
    }
    pub fn progress(&self) -> u32 {
        self.written() * 100 / self.frames.max(1)
    }

    /// The next rendered frame (already at the export size: the export camera renders it there, review I1) and its
    /// sound, to the encoder.
    pub fn push(&mut self, frame: RgbaImage, sound: Vec<f32>) -> Result<(), String> {
        let (w, h) = (self.settings.width, self.settings.height);
        if frame.dimensions() != (w, h) {
            return Err(format!("captured {:?}, expected {w}x{h}", frame.dimensions()));
        }
        let samples = (SAMPLE_RATE / self.settings.fps) as usize * CHANNELS as usize;
        if sound.len() != samples {
            return Err(format!("{} audio samples, expected {samples}", sound.len()));
        }
        let encoder = self.encoder.as_ref().ok_or("no encoder")?;
        // A refused frame means the thread has ended, and it reported why before it did.
        encoder.frames.send((frame, sound)).map_err(|_| self.poll().and_then(Result::err).unwrap_or_else(|| "the encoder stopped".into()))?;
        self.frame += 1;
        Ok(())
    }

    /// Some once the encoder has ended: Ok when the file is finished and renamed.
    pub fn poll(&self) -> Option<Result<(), String>> {
        let encoder = self.encoder.as_ref()?;
        let Ok(done) = encoder.done.lock() else { return Some(Err("the encoder stopped".into())) };
        match done.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("the encoder stopped".into())),
        }
    }

    /// B on the progress popup, or a failure: the partial file goes. The media sink may still hold it for a moment
    /// after the writer drops, so the delete is retried; one that never succeeds is logged and swept by the next
    /// export (review M4).
    pub fn cancel(mut self) {
        if let Some(encoder) = self.encoder.take() {
            encoder.stop.store(true, Ordering::Relaxed);
            drop(encoder.frames);
            // The writer drops with the thread; only then is the file free to delete.
            if let Some(thread) = encoder.thread {
                let _ = thread.join();
            }
        }
        for attempt in 0..5 {
            match std::fs::remove_file(&self.partial) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) if attempt == 4 => bevy::log::warn!("replay export: {}: {error}", self.partial.display()),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

/// A crash or a held handle leaves `*.partial.mp4` behind: an export start clears them (review M4).
fn sweep_partials(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".partial.mp4")) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Item 16 extension: the letterbox's bars black (grade.wgsl draws the same on screen); returns them (rows,
/// columns).
fn letterbox(frame: &mut RgbaImage, aspect: f32) -> (u32, u32) {
    let (w, h) = frame.dimensions();
    let (rows, columns) = super::grade::letterbox_bars(w, h, aspect);
    if rows == 0 && columns == 0 {
        return (0, 0);
    }
    for (x, y, p) in frame.enumerate_pixels_mut() {
        if y < rows || y >= h - rows || x < columns || x >= w - columns {
            *p = image::Rgba([0, 0, 0, 255]);
        }
    }
    (rows, columns)
}

fn margin_of(frame: &RgbaImage) -> u32 {
    (frame.height() as f32 * 0.03) as u32
}

/// RULING (build-progress, Task 10): the logo bottom right, 3 % in from the edges, at its own size on a 720p frame;
/// inside the letterbox's picture when there is one (`bars` = rows, columns).
fn stamp(frame: &mut RgbaImage, logo: &RgbaImage, bars: (u32, u32)) {
    let scale = frame.height() as f32 / 720.;
    let (w, h) = ((logo.width() as f32 * scale) as u32, (logo.height() as f32 * scale) as u32);
    if w == 0 || h == 0 || w + bars.1 * 2 + margin_of(frame) >= frame.width() || h + bars.0 * 2 + margin_of(frame) >= frame.height() {
        return;
    }
    let logo = image::imageops::resize(logo, w, h, image::imageops::FilterType::Triangle);
    let margin = margin_of(frame);
    let (x0, y0) = (frame.width() - w - margin - bars.1, frame.height() - h - margin - bars.0);
    for (x, y, p) in logo.enumerate_pixels() {
        let a = f32::from(p[3]) / 255.;
        let q = frame.get_pixel_mut(x0 + x, y0 + y);
        for c in 0..3 {
            q[c] = (f32::from(p[c]) * a + f32::from(q[c]) * (1. - a)) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::editor::Channel;

    fn editor() -> Editor {
        Editor::new(false)
    }
    fn job(dir: &Path, editor: &Editor, seconds: f32) -> Job {
        let path = dir.join("a.mp4");
        let partial = path.with_extension("partial.mp4");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&partial, b"half an mp4").unwrap();
        Job::with_writer(path, partial, None, ExportSettings::from_tags(0, 1, 0, 0), editor, seconds, None)
    }

    #[test]
    fn frame_count_matches_encoded_length() {
        assert_eq!(frame_count(encoded_length(&editor(), 10.), 60), 600);
        assert_eq!(frame_count(10., 30), 300);
    }

    #[test]
    fn half_speed_clip_doubles_frames() {
        let mut e = editor();
        e.set_whole_clip_speed(50);
        assert_eq!(frame_count(encoded_length(&e, 10.), 60), 1200);
        // Frame 600 of 1200 is halfway through the recording.
        assert!((playhead_for_frame(&e, 600, 60, 10.) - 0.5).abs() < 1e-5);
        // Two clips: the first at 100 %, the second at 50 %.
        let mut e = editor();
        e.state = crate::replay::editor::State::MainEdit;
        e.playhead = 0.5;
        e.drop_marker().unwrap();
        e.playhead = 0.75;
        e.adjust(Channel::Speed, false);
        for _ in 0..4 {
            e.adjust(Channel::Speed, false);
        }
        assert_eq!(e.speed_at(0.75), 50);
        assert_eq!(frame_count(encoded_length(&e, 10.), 60), 300 + 600);
        assert!((playhead_for_frame(&e, 300 + 300, 60, 10.) - 0.75).abs() < 1e-4);
    }

    #[test]
    fn over_60s_refused() {
        let mut e = editor();
        assert_eq!(check(&e, 60.), Ok(()));
        assert_eq!(check(&e, 61.), Err("ID_REPLAY_LENGTH_EXCEEDED"));
        e.set_whole_clip_speed(50);
        assert_eq!(check(&e, 40.), Err("ID_REPLAY_LENGTH_EXCEEDED"), "40 s at half speed encodes as 80");
        assert_eq!(check(&e, 0.), Err("ID_SKATEREEL_BROWSER_NO_FOOTAGE"), "review M9: an empty clip has its own text");
        e.in_mark = 0.5;
        e.out_mark = 0.5;
        assert_eq!(check(&e, 10.), Err("ID_SKATEREEL_BROWSER_NO_FOOTAGE"));
    }

    #[test]
    fn audio_ticks_tile_the_frames() {
        assert_eq!(audio_ticks(0, 60), 0..1);
        assert_eq!(audio_ticks(7, 60), 7..8);
        assert_eq!(audio_ticks(0, 30), 0..2);
        assert_eq!(audio_ticks(5, 30), 10..12);
        let all: Vec<u32> = (0..30).flat_map(|f| audio_ticks(f, 30)).collect();
        assert_eq!(all, (0..60).collect::<Vec<_>>());
    }

    /// A fake worker: each block carries the number of records it had seen when it rendered.
    struct Counting {
        records: Vec<u64>,
        fed: f32,
    }
    impl Render for Counting {
        fn frame(&mut self, observation: &PlayerAudioObservation) {
            self.records.push(observation.tick);
            self.fed += 1.;
        }
        fn block(&mut self) -> Vec<f32> {
            vec![self.fed; OFFLINE_BLOCK_FRAMES * 2]
        }
    }

    /// Review I2: records land at their tick's sample (never late, never early by more than a block), each frame
    /// takes exactly 48000 / fps stereo frames, and the records arrive in order.
    #[test]
    fn mix_places_records_on_their_ticks() {
        for fps in RATES {
            let mut render = Counting { records: Vec::new(), fed: 0. };
            let mut mix = Mix::new(fps);
            let mut out = Vec::new();
            for frame in 0..fps {
                let records: Vec<_> = audio_ticks(frame, fps)
                    .map(|tick| Some(crate::replay::record::tests::audio(tick as usize)))
                    .collect();
                let chunk = mix.frame(&mut render, &records);
                assert_eq!(chunk.len(), (SAMPLE_RATE / fps) as usize * 2);
                out.extend(chunk);
            }
            assert_eq!(out.len(), SAMPLE_RATE as usize * 2, "{fps} fps: one second of sound");
            assert_eq!(render.records, (0..60).collect::<Vec<_>>());
            for tick in 1..60usize {
                let at = tick * TICK_SAMPLES as usize;
                let block = OFFLINE_BLOCK_FRAMES;
                // The block holding sample `at` was rendered with `tick` records at most, and the one a block later
                // with all `tick + 1`.
                assert!(out[at * 2] <= tick as f32 + 1., "{fps} fps tick {tick}");
                assert!(out[(at + block) * 2] >= tick as f32 + 1., "{fps} fps tick {tick}");
            }
        }
    }

    /// A source that renders `level`; its own block number `fail` errs; the block right after the record at tick
    /// `panic` panics (a fault that one record causes). `frames` counts the records every source from this boot has taken (the
    /// warm-up included).
    struct Probe {
        level: f32,
        blocks: usize,
        fail: Option<usize>,
        panic: Option<u64>,
        last: Option<u64>,
        fresh: bool,
        frames: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Source for Probe {
        fn frame(&mut self, observation: &PlayerAudioObservation) -> Result<(), String> {
            self.frames.set(self.frames.get() + 1);
            (self.last, self.fresh) = (Some(observation.tick), true);
            Ok(())
        }
        fn block(&mut self) -> Result<Vec<f32>, String> {
            self.blocks += 1;
            let first = std::mem::take(&mut self.fresh);
            assert!(!(first && self.last.is_some() && self.last == self.panic), "a decoder that blows up");
            if Some(self.blocks) == self.fail {
                return Err("bank 7 decode failed".into());
            }
            Ok(vec![self.level; OFFLINE_BLOCK_FRAMES * 2])
        }
    }
    fn probe(level: f32, fail: Option<usize>, panic: Option<u64>) -> Probe {
        Probe { level, blocks: 0, fail, panic, last: None, fresh: false, frames: Default::default() }
    }
    /// Six record-then-block steps; each block's first sample.
    fn steps(render: &mut impl Render) -> Vec<f32> {
        (0..6)
            .map(|i| {
                render.frame(&crate::replay::record::tests::audio(i));
                let block = render.block();
                assert_eq!(block.len(), OFFLINE_BLOCK_FRAMES * 2);
                block[0]
            })
            .collect()
    }

    /// Owner 10-18: a failed block reboots at once and that same block is rendered on the fresh renderer, warmed
    /// with every record so far, so the output has no gap. A panic is the same, and the panicked one is dropped.
    #[test]
    fn a_failed_block_is_rendered_again_on_a_warmed_renderer() {
        for first in [probe(0.5, Some(3), None), probe(0.5, None, Some(2))] {
            let (boots, frames) = (std::rc::Rc::new(std::cell::Cell::new(0)), std::rc::Rc::new(std::cell::Cell::new(0)));
            let (b, f) = (boots.clone(), frames.clone());
            let mut render = Rebooting::new(first, move || {
                b.set(b.get() + 1);
                Ok(Probe { frames: f.clone(), ..probe(0.25, None, None) })
            });
            assert_eq!(steps(&mut render), [0.5, 0.5, 0.25, 0.25, 0.25, 0.25]);
            assert_eq!(boots.get(), 1);
            assert_eq!(frames.get(), 6, "the fresh one replayed the first three records, then took the rest");
        }
    }

    /// Owner 10-18: a block that errs on the fresh renderer too is deterministic: silence for that block only, and
    /// the export goes on with that renderer. One that panics it too is searched for the record it comes from.
    #[test]
    fn a_deterministic_failure_silences_one_block() {
        let boots = std::rc::Rc::new(std::cell::Cell::new(0));
        let b = boots.clone();
        let mut render = Rebooting::new(probe(0.5, Some(3), None), move || {
            b.set(b.get() + 1);
            Ok(probe(0.5, Some(3), None))
        });
        assert_eq!(steps(&mut render), [0.5, 0.5, 0., 0.5, 0.5, 0.5]);
        assert_eq!(boots.get(), 1);
    }

    /// Owner 10-18: a block that panics a warmed renderer too: fresh ones each leave out one record, newest first;
    /// the first that renders the block is left out for the rest of the export, so the export keeps its sound.
    #[test]
    fn a_panic_every_renderer_hits_leaves_out_the_record_it_comes_from() {
        let mut render = Rebooting::new(probe(0.5, None, Some(2)), || Ok(probe(0.5, None, Some(2))));
        assert_eq!(steps(&mut render), [0.5; 6], "the third block renders without its record");
        let left: Vec<_> = render.poison.iter().map(|&i| match &render.history[i] {
            Call::Frame(observation) => Some(observation.tick),
            Call::Block => None,
        }).collect();
        assert_eq!(left, [Some(2)], "only the record at tick 2 is left out");
        assert!(!render.dead);
    }

    /// No one record explains it (the renderer itself wears out): the rest is silent and the export still finishes.
    #[test]
    fn a_panic_no_record_explains_silences_the_rest() {
        struct Worn(usize);
        impl Source for Worn {
            fn frame(&mut self, _: &PlayerAudioObservation) -> Result<(), String> {
                Ok(())
            }
            fn block(&mut self) -> Result<Vec<f32>, String> {
                self.0 += 1;
                assert!(self.0 < 3, "a renderer that wears out");
                Ok(vec![0.5; OFFLINE_BLOCK_FRAMES * 2])
            }
        }
        let mut render = Rebooting::new(Worn(0), || Ok(Worn(0)));
        assert_eq!(steps(&mut render), [0.5, 0.5, 0., 0., 0., 0.]);
        assert!(render.dead);
    }

    /// The log names the panic: its message, and where it was raised once the game's hook is in.
    #[test]
    fn a_caught_panic_keeps_its_message() {
        let Err(Fail::Panic(why)) = guard(|| -> Result<(), String> { panic!("bank {} missing", 7) }) else {
            panic!("a panic is a Fail::Panic")
        };
        assert!(why.starts_with("bank 7 missing"), "{why}");
    }

    /// A frame that panics reboots at once; one that errs is skipped, as the live worker does.
    #[test]
    fn a_panicking_frame_reboots() {
        struct Bad(bool);
        impl Source for Bad {
            fn frame(&mut self, _: &PlayerAudioObservation) -> Result<(), String> {
                assert!(!self.0, "a guest fault");
                Ok(())
            }
            fn block(&mut self) -> Result<Vec<f32>, String> {
                Ok(vec![if self.0 { 0.5 } else { 0.75 }; OFFLINE_BLOCK_FRAMES * 2])
            }
        }
        let mut render = Rebooting::new(Bad(true), || Ok(Bad(false)));
        assert_eq!(steps(&mut render), [0.75; 6]);
    }

    /// Owner 10-18: a slow boot and a slow mix never time out and the main thread never waits; a boot that panics is
    /// the refusal, not a dead worker.
    #[test]
    fn slow_audio_never_times_out_and_still_sounds() {
        struct Slow(f32, Duration);
        impl Source for Slow {
            fn frame(&mut self, _: &PlayerAudioObservation) -> Result<(), String> {
                Ok(())
            }
            fn block(&mut self) -> Result<Vec<f32>, String> {
                std::thread::sleep(std::mem::take(&mut self.1));
                Ok(vec![self.0; OFFLINE_BLOCK_FRAMES * 2])
            }
        }
        let wait = |what: &str, mut poll: Box<dyn FnMut() -> bool + '_>| {
            let t = std::time::Instant::now();
            while !poll() {
                assert!(t.elapsed() < Duration::from_secs(60), "{what}");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let mut sound = Sound::start_with(30, None, stop, || {
            std::thread::sleep(Duration::from_millis(200));
            Ok(Slow(0.5, Duration::from_millis(300)))
        });
        let mut booted = None;
        wait("boot", Box::new(|| { booted = sound.ready(); booted.is_some() }));
        assert_eq!(booted, Some(Ok(())));
        sound.request(vec![None, None]);
        assert!(sound.chunk().is_none(), "the main thread does not wait on a slow mix");
        let mut chunk = None;
        wait("mix", Box::new(|| { chunk = sound.chunk(); chunk.is_some() }));
        let chunk = chunk.unwrap().unwrap();
        assert_eq!(chunk.len(), (SAMPLE_RATE / 30) as usize * 2);
        assert!(chunk.iter().all(|&s| s == 0.5));
        drop(sound);
        let stop = Arc::new(AtomicBool::new(false));
        let mut refused = Sound::start_with(30, None, stop, || -> Result<Slow, String> { panic!("no banks") });
        let mut booted = None;
        wait("refusal", Box::new(|| { booted = refused.ready(); booted.is_some() }));
        assert_eq!(booted, Some(Err("the export audio boot panicked: no banks".into())));
    }

    /// Owner 10-18: an export started while the last one's worker is still booting waits for it on its own thread
    /// ("Stopping the previous export..."), and the ended export's boot is told to stop.
    #[test]
    fn a_new_export_waits_for_the_previous_worker() {
        let (release, gate) = mpsc::channel::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, saw) = (stop.clone(), Arc::new(AtomicBool::new(false)));
        let told = saw.clone();
        let (started, booting) = mpsc::channel::<()>();
        let first = Sound::start_with(30, None, stop, move || {
            let _ = started.send(());
            let _ = gate.recv();
            told.store(seen.load(Ordering::Relaxed), Ordering::Relaxed);
            Err::<Probe, _>(String::from("cancelled"))
        });
        booting.recv_timeout(Duration::from_secs(60)).expect("the first boot starts");
        drop(first);
        let mut second = Sound::start_with(30, None, Arc::new(AtomicBool::new(false)), || Ok(probe(0.5, None, None)));
        assert!(second.stopping());
        assert_eq!(second.ready(), None);
        release.send(()).unwrap();
        let t = std::time::Instant::now();
        while second.ready().is_none() {
            assert!(t.elapsed() < Duration::from_secs(60));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!second.stopping());
        assert_eq!(second.ready(), Some(Ok(())));
        assert!(saw.load(Ordering::Relaxed), "the ended export's boot saw its stop");
    }

    /// Every export rate gives whole sample counts of the 48 kHz stream mf.rs declares (no drift, push's check holds).
    #[test]
    fn every_rate_divides_the_audio_clock() {
        assert_eq!(SAMPLE_RATE, 48_000);
        for fps in RATES {
            assert_eq!(SAMPLE_RATE % fps, 0, "{fps} fps");
        }
    }

    #[test]
    fn mismatched_frames_are_refused() {
        let dir = std::env::temp_dir().join(format!("s3r-export-size-{}", std::process::id()));
        let mut job = job(&dir, &editor(), 2.);
        let silence = vec![0.; (SAMPLE_RATE / 60) as usize * 2];
        assert!(job.push(RgbaImage::new(1920, 1080), silence.clone()).unwrap_err().contains("expected 1280x720"));
        assert!(job.push(RgbaImage::new(1280, 720), silence[1..].to_vec()).unwrap_err().contains("audio"));
        assert_eq!(job.frame, 0, "a refused frame is not counted");
        job.cancel();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_sweeps_stale_partials() {
        let dir = std::env::temp_dir().join(format!("s3r-export-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.partial.mp4"), b"crashed").unwrap();
        std::fs::write(dir.join("kept.mp4"), b"done").unwrap();
        sweep_partials(&dir);
        assert!(!dir.join("old.partial.mp4").exists());
        assert!(dir.join("kept.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_deletes_partial_file() {
        let dir = std::env::temp_dir().join(format!("s3r-export-cancel-{}", std::process::id()));
        let job = job(&dir, &editor(), 2.);
        let partial = job.partial.clone();
        assert!(partial.is_file());
        job.cancel();
        assert!(!partial.exists());
        assert!(!dir.join("a.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An encoder that fails mid-export leaves no file and touches no editor state (hud.rs shows the error popup
    /// and stays in the editor).
    #[test]
    fn encoder_failure_returns_to_editor_with_state() {
        let dir = std::env::temp_dir().join(format!("s3r-export-fail-{}", std::process::id()));
        let mut e = editor();
        e.set_whole_clip_speed(70);
        let before = e.clone();
        let mut job = job(&dir, &e, 2.);
        let playhead = job.playhead(&e, 0);
        assert_eq!(playhead, 0.);
        assert!(job.push(RgbaImage::new(64, 36), Vec::new()).is_err());
        let partial = job.partial.clone();
        job.cancel();
        assert!(!partial.exists());
        assert_eq!((e.keys, e.clip_markers, e.in_mark, e.out_mark), (before.keys, before.clip_markers, before.in_mark, before.out_mark));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watermark_lands_bottom_right() {
        let mut frame = RgbaImage::from_pixel(1280, 720, image::Rgba([0, 0, 0, 255]));
        let logo = RgbaImage::from_pixel(100, 50, image::Rgba([255, 255, 255, 255]));
        stamp(&mut frame, &logo, (0, 0));
        assert_eq!(frame.get_pixel(1280 - 21 - 50, 720 - 21 - 25)[0], 255);
        assert_eq!(frame.get_pixel(10, 10)[0], 0);
    }

    /// Item 16: the letterbox's bars are black in the frame buffer, the picture untouched, the watermark inside it.
    #[test]
    fn letterbox_pixels_are_black() {
        let grey = image::Rgba([128, 128, 128, 255]);
        let mut frame = RgbaImage::from_pixel(1280, 720, grey);
        let bars = letterbox(&mut frame, 2.39);
        assert_eq!(bars, (92, 0), "(720 - 1280 / 2.39) / 2");
        for y in [0, 91, 720 - 92, 719] {
            assert!((0..1280).all(|x| frame.get_pixel(x, y).0 == [0, 0, 0, 255]), "row {y}");
        }
        for y in [92, 360, 720 - 93] {
            assert!((0..1280).all(|x| *frame.get_pixel(x, y) == grey), "row {y}");
        }
        let logo = RgbaImage::from_pixel(100, 50, image::Rgba([255, 255, 255, 255]));
        stamp(&mut frame, &logo, bars);
        assert_eq!(frame.get_pixel(1280 - 21 - 50, 720 - 92 - 21 - 25)[0], 255, "the logo sits on the picture");
        assert_eq!(frame.get_pixel(1280 - 21 - 50, 720 - 21 - 25)[0], 0, "not on the bar");
        // 4:3 pillarboxes; off leaves the frame alone.
        let mut frame = RgbaImage::from_pixel(1280, 720, grey);
        assert_eq!(letterbox(&mut frame, 4. / 3.), (0, 160));
        assert_eq!(frame.get_pixel(159, 300).0, [0, 0, 0, 255]);
        assert_eq!(*frame.get_pixel(160, 300), grey);
        let mut frame = RgbaImage::from_pixel(64, 36, grey);
        assert_eq!(letterbox(&mut frame, 0.), (0, 0));
        assert!(frame.pixels().all(|p| *p == grey));
        // The job draws it into every pushed frame: an editor with the 2.39 letterbox.
        let mut e = editor();
        e.look.letterbox = 1;
        let dir = std::env::temp_dir().join(format!("s3r-export-letterbox-{}", std::process::id()));
        let job = job(&dir, &e, 2.);
        assert_eq!(job.letterbox, 2.39);
        job.cancel();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Item 16: a speed ramp (an eased Speed key) maps encoded frames to the source frames its integral gives, the
    /// audio ticks carry the ramp's time scale, and the frame count is the ramp's length.
    #[test]
    fn speed_ramp_frames_and_audio() {
        use crate::replay::editor::{Ease, Key, Value};
        let mut e = editor();
        // 100 % at 0 easing linearly to 20 % at 0.5, then held: 10 s of recording.
        e.keys[Channel::Speed.index()] = vec![
            Key { t: 0., soft: false, cut: false, value: Value::Int(100), ease: Ease::Linear },
            Key { t: 0.5, soft: true, cut: false, value: Value::Int(20), ease: Ease::Hold },
        ];
        // Encoded: the ramp is 5 s * ln(5) / 0.8 = 10.06 s, the held half 5 s / 0.2 = 25 s.
        let ramp = 5. * 5f32.ln() / 0.8;
        let length = encoded_length(&e, 10.);
        assert!((length - (ramp + 25.)).abs() < 0.01, "{length}");
        assert_eq!(frame_count(length, 30), ((ramp + 25.) * 30.).round() as u32);
        assert_eq!(e.encoded_seconds(10.), (ramp + 25. + 0.5) as u32);
        // The playhead after s encoded seconds of the ramp: speed(p) = 1 - 1.6 p, so p = (1 - exp(-0.16 s)) / 1.6.
        for s in [0.5f32, 2., 5., 9.] {
            let expected = (1. - (-0.16 * s).exp()) / 1.6;
            let at = playhead_for_frame(&e, (s * 60.) as u32, 60, 10.);
            assert!((at - expected).abs() < 2e-4, "{s} s: {at} vs {expected}");
        }
        // Past the ramp the held 20 % walks exactly as retail.
        let at = playhead_for_frame(&e, ((ramp + 10.) * 60.).round() as u32, 60, 10.);
        assert!((at - 0.7).abs() < 1e-3, "{at}");
        // Frames walk forward and the audio ticks carry speed / 100 at their playhead.
        let mut last = 0.;
        for frame in 0..frame_count(length, 30) {
            let p = playhead_for_frame(&e, frame, 30, 10.);
            assert!(p >= last, "frame {frame}");
            last = p;
            for tick in audio_ticks(frame, 30) {
                let q = playhead_for_frame(&e, tick, 60, 10.);
                let scale = e.time_scale(q);
                let expected = if q < 0.5 { (100. - 160. * q) * 0.01 } else { 0.2 };
                assert!((scale - expected).abs() < 1e-4, "tick {tick}: {scale} vs {expected}");
            }
        }
        assert!(last > 0.99, "the last frame reaches the end: {last}");
    }

    /// Writes 2 s of 1280x720 / 30 synthetic frames and a tone, then reads the file back with a source reader.
    #[test]
    #[ignore = "needs Windows Media Foundation (H.264 / AAC encoders)"]
    #[cfg(windows)]
    fn mf_writes_playable_mp4() {
        let path = std::env::temp_dir().join(format!("s3r-mf-{}.mp4", std::process::id()));
        let settings = ExportSettings::from_tags(0, 0, 1, 0);
        let mut writer = Mp4Writer::create(&path, &settings, SAMPLE_RATE).unwrap();
        for frame in 0..60u32 {
            let rgba = RgbaImage::from_fn(1280, 720, |x, y| image::Rgba([(x + frame * 8) as u8, y as u8, 128, 255]));
            writer.video(rgba.as_raw(), frame).unwrap();
            let tone: Vec<f32> = (0..SAMPLE_RATE / 30)
                .flat_map(|i| {
                    let t = (frame * (SAMPLE_RATE / 30) + i) as f32 / SAMPLE_RATE as f32;
                    let v = (t * 440. * std::f32::consts::TAU).sin() * 0.3;
                    [v, v]
                })
                .collect();
            writer.audio(&tone).unwrap();
        }
        writer.finish().unwrap();
        let (duration, frames, size, peak) = super::super::mf::probe(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!((duration - 2.).abs() < 0.05, "{duration} s");
        assert_eq!(frames, 60);
        assert_eq!(size, (1280, 720));
        assert!(peak > 0.1, "the AAC track carries the tone: peak {peak}");
    }

    /// Review M4: cancelling with a live Media Foundation writer still deletes the partial file.
    #[test]
    #[ignore = "needs Windows Media Foundation (H.264 / AAC encoders)"]
    #[cfg(windows)]
    fn mf_cancel_with_live_writer_deletes_partial() {
        let dir = std::env::temp_dir().join(format!("s3r-export-live-cancel-{}", std::process::id()));
        let e = editor();
        let mut job = Job::start(&dir, "clip", ExportSettings::from_tags(0, 0, 0, 0), &e, 2., None).unwrap();
        let silence = vec![0.; (SAMPLE_RATE / 30) as usize * 2];
        for _ in 0..10 {
            job.push(RgbaImage::new(1280, 720), silence.clone()).unwrap();
        }
        let partial = job.partial.clone();
        assert!(partial.is_file());
        job.cancel();
        assert!(!partial.exists(), "the sink let go of the partial file");
        assert!(!dir.join("clip.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Render speed 10-27: frames go to the encoder thread as fast as they are pushed; it writes every one in order
    /// with its sound, then finishes and renames the file, and says so.
    #[test]
    #[ignore = "needs Windows Media Foundation (H.264 / AAC encoders)"]
    #[cfg(windows)]
    fn mf_export_thread_writes_every_frame_then_finishes() {
        let dir = std::env::temp_dir().join(format!("s3r-export-thread-{}", std::process::id()));
        let e = editor();
        let mut job = Job::start(&dir, "clip", ExportSettings::from_tags(0, 0, 0, 0), &e, 2., None).unwrap();
        let silence = vec![0.; (SAMPLE_RATE / 30) as usize * 2];
        for n in 0..job.frames {
            job.push(RgbaImage::from_pixel(1280, 720, image::Rgba([(n * 4) as u8, 0, 0, 255])), silence.clone()).unwrap();
        }
        assert_eq!(job.frame, job.frames);
        let done = std::iter::repeat_with(|| {
            std::thread::sleep(Duration::from_millis(10));
            job.poll()
        })
        .take(3000)
        .flatten()
        .next();
        assert_eq!(done, Some(Ok(())));
        assert_eq!((job.written(), job.progress()), (job.frames, 100));
        let (duration, frames, size, _) = super::super::mf::probe(&job.path).unwrap();
        assert_eq!((frames, size), (job.frames, (1280, 720)));
        assert!((duration - f64::from(job.frames) / 30.).abs() < 0.05, "{duration} s");
        assert!(!job.partial.exists(), "renamed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
