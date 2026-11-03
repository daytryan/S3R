//! Single-owner host for the recovered rider/board sound graph.
//!
//! Asset IO and XMA decoding happen once on this worker before the runtime is published. The
//! Bevy mixer sees one permanent stereo source; neither the mixer callback nor gameplay touches
//! guest memory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Mutex,
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};
use std::thread;
use std::time::{Duration, Instant};

use bevy::audio::{AddAudioSource, AudioPlayer, PlaybackSettings, Source, Volume};
use bevy::prelude::*;
use skate_audio_core::authored::{AuthoredRuntime, PCM_CHANNELS, PCM_FRAMES_PER_BLOCK};
use skate_audio_core::pcm::PcmSource;
use skate_data::audio::catalog::PlayerAudioCatalog;

use super::{LivePcm, PlayerAudioObservation};

#[path = "player_audio/audio_state.rs"]
mod audio_state;
#[path = "player_audio/collision_materials.rs"]
mod collision_materials;
#[path = "player_audio/collision_states.rs"]
mod collision_states;
#[path = "player_audio/components/mod.rs"]
mod components;
#[path = "player_audio/contact_voices.rs"]
mod contact_voices;
#[path = "player_audio/frontend.rs"]
mod frontend;
#[path = "player_audio/game_side.rs"]
mod game_side;
pub(crate) use game_side::announce_map;
#[cfg(test)]
#[path = "player_audio/headless.rs"]
mod headless;
#[path = "player_audio/remote.rs"]
pub(crate) mod remote;
#[path = "player_audio/rumble.rs"]
mod rumble;
pub(crate) use rumble::RESET as RUMBLE_RESET;
#[path = "player_audio/sound.rs"]
mod sound;
#[path = "player_audio/trace.rs"]
pub(crate) mod trace;
#[path = "player_audio/tuning.rs"]
mod tuning;
#[path = "player_audio/world.rs"]
mod world;

const OUTPUT_CHANNELS: u16 = 2;
const SAMPLE_RATE: u32 = 48_000;
const QUEUE_FRAMES: usize = 4096;
const OBSERVATION_QUEUE: usize = 16;
// The host edge is the console's 5.1 -> stereo fold-down, and nothing else: the native block
// `pump_once` returns has already been through retail's own master section and output pass
// (`sub_82B21D98`, each channel clamped to +/-1).
//
// On the 360 RwAudioCore always hands the console 5.1 and the console folds it down (the
// dashboard picks the mode, `EVENT_SETMODE` cannot). The image's own RwAudioCore docs (the Stereo
// output-mode string at 0x820F0528) give the fold its PC build does and say the 360's and PS3's
// "seem to produce the same results": drop the LFE, mix centre and backs into the fronts at -3 dB,
// then bring the whole mix down another 3 dB "to keep the signal from running too hot and
// clipping excessively". This replaces the old front + 0.707 centre + 0.5 surround at x0.6, a
// fitted headroom from before the game side ran retail's master stage (audit 2026-10-03).
const FOLD_DOWN: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Each observation; whether a cutscene (not a menu) holds it ([`crate::skate_audio::CutsceneHold`]); and the boot's FE
/// audio state ([`crate::skate_audio::fe_audio_state`]) as of the frame the observation went out, not when the worker
/// gets to it.
type Frame = (PlayerAudioObservation, bool, Option<u32>);

#[derive(Resource)]
pub(crate) struct PlayerAudioHost {
    input: SyncSender<Frame>,
    /// The last forwarded observation, repeated while paused (see [`forward`]).
    last_observation: Option<PlayerAudioObservation>,
    paused_carry: f32,
    status: Mutex<Receiver<WorkerStatus>>,
    live: LivePcm,
    last_report: Instant,
    dropped_observations: u64,
    /// Front-end one-shot ids waiting for the next observation to carry them to the worker.
    pending_frontend: Vec<u64>,
}

#[derive(Resource)]
pub(crate) struct PlayerAudioAssets(pub PathBuf);

enum WorkerStatus {
    Ready { samples: usize, cache_hits: usize },
    Failed(String),
}

pub(super) fn install(app: &mut App) {
    // The session marker registers this too, but its plugin returns early when its validation
    // data will not load, and the multiplier sounds must not depend on that.
    app.add_message::<crate::skate_audio::FrontEndSoundRequest>()
        .add_audio_source::<PlayerStream>()
        .add_systems(Startup, start)
        .add_systems(Update, (forward, report, rumble::update));
}

/// The player stream as the mixer sees it: [`LivePcm`]'s own decoder, which also raises the
/// output thread on its first pull. cpal means to run that thread time-critical but hands
/// `SetThreadPriority` its thread id where Windows wants a handle (cpal 0.15.3
/// `boost_current_thread_priority`), so it ran at normal priority, and a background window's
/// mixer could miss the device's deadline.
/// Voice chat's output plays as one too (voice.rs).
#[derive(Asset, TypePath, Clone)]
pub(crate) struct PlayerStream(pub(crate) LivePcm);

pub(crate) struct PlayerPlayback {
    live: super::LivePlayback,
    raised: bool,
}

impl Decodable for PlayerStream {
    type DecoderItem = f32;
    type Decoder = PlayerPlayback;

    fn decoder(&self) -> PlayerPlayback {
        PlayerPlayback {
            live: self.0.decoder(),
            raised: false,
        }
    }
}

impl Iterator for PlayerPlayback {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if !self.raised {
            self.raised = true;
            raise_audio_thread();
        }
        self.live.next()
    }
}

impl Source for PlayerPlayback {
    fn current_frame_len(&self) -> Option<usize> {
        self.live.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.live.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.live.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.live.total_duration()
    }
}

/// Runs the calling audio thread (the worker, the mixer) ahead of ordinary threads, whatever
/// the window does (skate_voice::priority: time-critical, never power-throttled). A background
/// window loses Windows' foreground boost and its QoS drops, and the worker, which needs most
/// of a core in real time, fell behind whenever anything else wanted the CPU (a compile, a
/// browser): the device played the gaps as static.
fn raise_audio_thread() {
    if let Err(error) = skate_voice::priority::raise() {
        eprintln!("SKATE_PLAYER_AUDIO could not raise {:?}: {error}", thread::current().name());
    }
}

/// 82666BC0's sound for a sequence multiplier that has just changed, or `None` for the
/// multipliers retail is silent for. The caller owns the change detection, as the HUD module
/// does; this module is shared by binaries that have no simulation to poll.
pub(crate) fn multiplier_sound_id(multiplier: f32) -> Option<u64> {
    frontend::FrontEndSound::for_multiplier(multiplier).map(frontend::FrontEndSound::id)
}

fn start(
    mut commands: Commands,
    mut assets: ResMut<Assets<PlayerStream>>,
    configured: Option<Res<PlayerAudioAssets>>,
) {
    let Some(configured) = configured else { return };
    let live = match LivePcm::new(OUTPUT_CHANNELS, SAMPLE_RATE, QUEUE_FRAMES) {
        Ok(live) => live,
        Err(error) => {
            error!("skate-audio player source: {error}");
            return;
        }
    };
    let handle = assets.add(PlayerStream(live.clone()));
    commands.spawn((
        AudioPlayer(handle),
        // LivePcm never ends. Loop mode would wrap it in Rodio's buffering repeater, which
        // eagerly captures startup silence and retains every streamed sample.
        PlaybackSettings::ONCE.with_volume(Volume::Linear(1.0)),
    ));

    let (input, observations) = mpsc::sync_channel(OBSERVATION_QUEUE);
    let (status_tx, status) = mpsc::channel();
    let assets = configured.0.clone();
    let worker_live = live.clone();
    thread::Builder::new()
        .name("skate-player-audio".into())
        .spawn(move || {
            raise_audio_thread();
            if let Err(error) = run(assets, worker_live, observations, &status_tx) {
                eprintln!("SKATE_PLAYER_AUDIO failed={error}");
                let _ = status_tx.send(WorkerStatus::Failed(error));
            }
        })
        .map_err(|error| error!("skate-audio could not start player worker: {error}"))
        .ok();
    commands.insert_resource(PlayerAudioHost {
        input,
        last_observation: None,
        paused_carry: 0.0,
        status: Mutex::new(status),
        live,
        last_report: Instant::now(),
        dropped_observations: 0,
        pending_frontend: Vec::new(),
    });
}

pub(crate) fn forward(
    mut host: Option<ResMut<PlayerAudioHost>>,
    mut observations: MessageReader<PlayerAudioObservation>,
    mut marker_sounds: MessageReader<crate::skate_audio::FrontEndSoundRequest>,
    virtual_time: Res<Time<bevy::time::Virtual>>,
    real_time: Res<Time<bevy::time::Real>>,
    elsewhere: Option<Res<crate::skate_audio::FrontEndSoundsVoicedElsewhere>>,
    hold: Option<Res<crate::skate_audio::CutsceneHold>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    mut remote: Option<ResMut<remote::RemoteAudio>>,
    traffic: Option<Res<crate::traffic::Traffic>>,
    replay: Option<Res<crate::replay::Replay>>,
) {
    let Some(host) = host.as_deref_mut() else {
        return;
    };
    // The other skaters' records for each frame sent (item 30b), none offline, and the watched one's while spectating.
    let mut remotes = || remote.as_deref_mut().map_or_else(|| (Vec::new(), None), |r| (r.next_records(), r.watched_record()));
    // The traffic the title's car sounds hear: none online (Traffic has no cars there) or in a replay.
    let replaying = replay.is_some_and(|r| r.active);
    let cars = || traffic.as_deref().filter(|_| !replaying).and_then(crate::traffic::Traffic::voices);
    let cutscene = hold.is_some_and(|h| h.0);
    // Audio Settings, in tenths: stamped on frozen frames too, since the sliders move in the pause menu.
    let volumes = menu.map_or([1.0; 3], |m| m.audio().volumes.map(|v| f32::from(v.min(10)) / 10.0)).map(|v| v * crate::music::duck());
    // Under the Skate 2 HUD the session marker speaks in Skate 2's voice, played by skate2_hud.
    let elsewhere = elsewhere.as_deref().map_or(&[][..], |e| &e.0[..]);
    // The session marker fires on its own schedule, not with the simulation, so its sounds are
    // buffered and ride out on the next observation rather than being dropped if this frame
    // produced none. Retail calls `GlobalFEPlaySound` straight from the UI; this is the closest
    // equivalent that still reaches the one audio worker.
    host.pending_frontend.extend(
        marker_sounds
            .read()
            .map(|sound| sound.0)
            .filter(|id| !elsewhere.contains(id)),
    );
    // Retail keeps publishing an audio state every frame while the game is paused — its physics
    // values simply stay frozen — and silences the player path through SFXObj_Pause's input. The
    // simulation schedule does not run while `Time<Virtual>` is paused, so repeat the last
    // observation at the simulation rate with `paused` set, which is what retail's frozen state is.
    // Paused as of `First`, when the frame's virtual delta (and so whether physics steps) was decided:
    // is_paused() flickers within a frame (graphics_menu unpauses in PreUpdate, the tutorials re-pause in Update).
    // While the boot owns the screen the world is held from its first frame and has no skater for the audio:
    // the frames are the front end's (no map, a skater at rest, not paused: retail's boot runs no pause menu),
    // carrying its sounds and the FE audio state that picks the title's ambience bed (game_side).
    let fe_state = crate::skate_audio::fe_audio_state();
    let boot = fe_state.is_some();
    if virtual_time.delta().is_zero() {
        let last = if boot { Some(PlayerAudioObservation::default()) } else { host.last_observation.clone() };
        if let Some(last) = last {
            host.paused_carry += real_time.delta_secs();
            while host.paused_carry >= sound::FRAME_SECONDS {
                host.paused_carry -= sound::FRAME_SECONDS;
                let mut frozen = last.clone();
                frozen.retail.paused = !boot;
                frozen.volumes = volumes;
                // Menu sounds happen while paused; they ride the frozen frames.
                frozen.frontend_sounds = std::mem::take(&mut host.pending_frontend);
                (frozen.remote_records, frozen.watched) = remotes();
                frozen.traffic = cars();
                let _ = host.input.try_send((frozen, cutscene, fe_state));
            }
        }
        return;
    }
    host.paused_carry = 0.0;
    for observation in observations.read() {
        host.last_observation = Some(observation.clone());
        let mut observation = observation.clone();
        observation.volumes = volumes;
        observation
            .frontend_sounds
            .append(&mut std::mem::take(&mut host.pending_frontend));
        (observation.remote_records, observation.watched) = remotes();
        observation.traffic = cars();
        match host.input.try_send((observation, false, fe_state)) {
            Ok(()) => {}
            Err(TrySendError::Full((lost, ..))) => {
                host.dropped_observations += 1;
                eprintln!(
                    "SKATE_PLAYER_AUDIO observation_overflow tick={} footstep={} push={} landed={} events={:?}",
                    lost.tick,
                    lost.footstep_strength,
                    lost.foot_push_speed,
                    lost.landed,
                    lost.events
                );
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
}

fn report(mut host: Option<ResMut<PlayerAudioHost>>) {
    let Some(host) = host.as_deref_mut() else {
        return;
    };
    let statuses: Vec<_> = host
        .status
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .try_iter()
        .collect();
    for status in statuses {
        match status {
            WorkerStatus::Ready {
                samples,
                cache_hits,
            } => {
                info!("skate-audio player graph ready: {samples} samples ({cache_hits} from cache)")
            }
            WorkerStatus::Failed(error) => error!("skate-audio player graph stopped: {error}"),
        }
    }
    if host.dropped_observations >= 60 {
        warn!(
            "skate-audio dropped {} stale physics observations",
            host.dropped_observations
        );
        host.dropped_observations = 0;
    }
    if host.last_report.elapsed() >= Duration::from_secs(2) {
        let stats = host.live.snapshot();
        eprintln!(
            "SKATE_PLAYER_AUDIO stream pushed={} consumed={} nonzero={} underflow={} queued_frames={}",
            stats.pushed,
            stats.consumed,
            stats.consumed_nonzero,
            stats.underflow,
            host.live.queued_frames(),
        );
        host.last_report = Instant::now();
    }
}

fn cache_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"))
}

/// Everything the worker builds before its first block: the authored runtime with its banks and
/// PCM, the boot utilities, the MixMap and every player-sound component.
pub(crate) struct Prepared {
    pub runtime: AuthoredRuntime,
    pub sound: sound::PlayerSound,
    pub samples: usize,
    pub cache_hits: usize,
}

pub(crate) fn prepare(assets: &std::path::Path) -> Result<Prepared, String> {
    // perf 10-19: SKATE_AUDIO_READ_ONCE: one read of audiofiles.big for the catalog, the Splice banks and the contact
    // voices (sound::build), dropped when prepare returns (skate_data's read_archive).
    let _archive = crate::config::perf_on("SKATE_AUDIO_READ_ONCE")
        .then(|| skate_data::audio::catalog::read_archive(&assets.join("private/stock/data/audio/audiofiles.big")).ok())
        .flatten();
    let mut catalog = PlayerAudioCatalog::from_assets(assets, cache_directory().as_deref())
        .map_err(|error| error.to_string())?;
    // The Splice collision banks (`Skate_Collisions.bnk`) hold the wheel pops and the landing
    // impact one-shots; they install exactly like the patch banks' samples.
    catalog
        .load_splice_banks(
            &assets.join("private/stock/data/audio/audiofiles.big"),
            cache_directory().as_deref(),
        )
        .map_err(|error| error.to_string())?;
    let sample_count = catalog.samples.len();
    let cache_hits = catalog.cache_hits;
    let mixmap = catalog
        .mixmap
        .clone()
        .ok_or("MixMapSK8.mxb is not staged in the assets (rerun setup)")?;
    let mut runtime = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks)
        .map_err(|error| error.to_string())?;
    for sample in catalog.samples {
        let base = runtime
            .bank_base(&sample.bank)
            .ok_or_else(|| format!("installed bank {} disappeared", sample.bank))?;
        runtime
            .insert_pcm(base + sample.header_offset, sample.pcm)
            .map_err(|error| error.to_string())?;
    }
    runtime
        .load_mixmap(&mixmap)
        .map_err(|error| error.to_string())?;
    let sound = sound::build(&mut runtime, assets, cache_directory().as_deref())?;
    if !sound.title_world() {
        // The title world (`world.rs`) posts its own copies of these at gameplay start, against the
        // same installed projects; a second set doubled every seam crack (two `rnd_call` publishers).
        // Retail's audio boot creates these three persistent utilities before any rider/board
        // message. They all use the same eight packet-local control-block references; their order
        // is observable in the retail boot trace and establishes the broadcaster bindings consumed
        // later by rolling-surface programs.
        let utility_relocations = [
            (3, 0x1c),
            (7, 0x2c),
            (11, 0x3c),
            (15, 0x4c),
            (19, 0x5c),
            (23, 0x6c),
            (27, 0x7c),
        ];
        let utility_packet = [0u32; 28];
        let _emitter_utility = if runtime.has_object("c_emitter_utility") {
            Some(
                runtime
                    .post_relocated("c_emitter_utility", &utility_packet, &utility_relocations)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let _startup_play = if runtime.has_object("Start_up_Play_ctl") {
            Some(
                runtime
                    .post_relocated("Start_up_Play_ctl", &utility_packet, &utility_relocations)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        // `c_foley_utility` owns the foley-side subscriptions. Its eight
        // packet-local control blocks own the table-1 subscriptions used by dependent foley
        // programs; posting a rolling input without it lets the selector open then immediately
        // tear down its voices. Preserve every self-relative block exactly from the boot trace.
        let _foley_utility = if runtime.has_object("c_foley_utility") {
            let handle = runtime
                .post_relocated("c_foley_utility", &utility_packet, &utility_relocations)
                .map_err(|error| error.to_string())?;
            eprintln!("SKATE_PLAYER_AUDIO foley_utility_start handle={handle:#010x}");
            Some(handle)
        } else {
            None
        };
    }
    eprintln!("SKATE_PLAYER_AUDIO player_sound ready");
    Ok(Prepared {
        runtime,
        sound,
        samples: sample_count,
        cache_hits,
    })
}

fn run(
    assets: PathBuf,
    live: LivePcm,
    observations: Receiver<Frame>,
    status: &mpsc::Sender<WorkerStatus>,
) -> Result<(), String> {
    open_audio_error_log();
    if game_side::enabled() {
        // A game side that gets stuck mid-session is booted again: a fresh runtime is a fresh
        // guest heap. ponytail: three boots, then the ported path is the safety net — a
        // game-side failure costs its exactness, not the sound.
        for boot in 1..=3 {
            match run_game_side(&assets, &live, &observations, status) {
                Ok(()) => return Ok(()),
                Err(error) => log_audio_error(&format!("game_side_failed boot={boot} error={error}")),
            }
        }
        log_audio_error("game side failed three boots; falling back to the ported player audio");
    }
    let Prepared {
        mut runtime,
        mut sound,
        samples: sample_count,
        cache_hits,
    } = prepare(&assets)?;
    let _ = status.send(WorkerStatus::Ready {
        samples: sample_count,
        cache_hits,
    });
    eprintln!("SKATE_PLAYER_AUDIO ready samples={sample_count} cache_hits={cache_hits}");

    let trace = std::env::var_os("SKATE_AUDIO_OBSERVE").is_some();
    // Diagnostics: one line per second with the rendered level and voice counts, plus the audio
    // state that drives the board, so a playtest log shows what the retail components saw.
    let mut window_peak = 0.0f32;
    let mut window_blocks = 0u32;
    loop {
        // Every observation may contain a one-tick contact or landing. Consume FIFO, allowing
        // the evaluator to run between observations; draining to the newest packet loses edges
        // and can overwrite a post's controls before its first audio block. At 187.5 blocks/s
        // this catches up with the normal 60 Hz simulation without discarding intermediate ticks.
        // The game closing its end is the app exiting.
        let Ok(next) = next_observation(&observations) else {
            return Ok(());
        };
        if let Some((observation, ..)) = next {
            sound.frame(&mut runtime, &observation)?;
        }
        let native = runtime.pump_once().map_err(|error| error.to_string())?;
        if native.len() != PCM_FRAMES_PER_BLOCK as usize * usize::from(PCM_CHANNELS) {
            return Err(format!(
                "authored graph returned {} samples for one block",
                native.len()
            ));
        }
        // Player audio reaches the host only through the recovered retail message and graph
        // path.  Do not substitute decoded-bank one-shots here: that bypasses the authored
        // selector, pitch, envelope, filter, layer, and bus controls and therefore cannot be
        // considered a Skate 3 match.
        let stereo = downmix(&native);
        trace::output(&native, usize::from(PCM_CHANNELS), &stereo);
        window_peak = native
            .iter()
            .fold(window_peak, |peak, sample| peak.max(sample.abs()));
        window_blocks += 1;
        if trace && window_blocks as u64 * u64::from(PCM_FRAMES_PER_BLOCK) >= u64::from(SAMPLE_RATE)
        {
            let stats = runtime.stats();
            // The graph arena: blocks live must plateau. It climbing without bound was the
            // voice-graph leak that ended a long session in "guest heap exhausted".
            let arena = runtime.arena_usage();
            let audio = sound.audio();
            eprintln!(
                "SKATE_PLAYER_AUDIO second block={} peak_dbfs={:.1} opened={} live={} arena={}k/{} speed={:.2} wheels={} air={} grind={} walk={}",
                stats.blocks,
                20.0 * window_peak.max(1.0e-9).log10(),
                stats.voices_opened,
                stats.live_voices,
                arena.0 / 1024,
                arena.1,
                audio.ground_speed_208,
                audio.wheel_count_200,
                audio.in_known_air_332,
                audio.grinding_341,
                audio.walking_716,
            );
            window_peak = 0.0;
            window_blocks = 0;
        }
        live.push(&stereo)?;
        pace(&live);
    }
}

/// `skate3rust --warm-audio` (the build's last step, user 11-02): the game side boots as [`run_game_side`] does,
/// decoding the banks it starts with and the world's into the PCM cache, so a first play has its sounds at once.
pub(crate) fn warm_cache(assets: &std::path::Path) -> Result<(), String> {
    let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
        .map_err(|error| error.to_string())?;
    let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).map_err(|error| error.to_string())?;
    let mut side = game_side::GameSide::build(&mut runtime, assets)?;
    // A new install starts on University (install.py's default map).
    side.warm_map(&mut runtime, "University")?;
    skate_data::audio::title::wait_prewarm();
    Ok(())
}

/// The worker with the retail audio game side run as original code ([`game_side`]): the runtime
/// holds no banks of its own, the title loads them. `Ok` when the game exits; `Err` when it
/// could not boot or got stuck.
fn run_game_side(
    assets: &std::path::Path,
    live: &LivePcm,
    observations: &Receiver<Frame>,
    status: &mpsc::Sender<WorkerStatus>,
) -> Result<(), String> {
    let guest = skate_data::audio::catalog::load_guest_image(
        &assets.join("private/stock/audio-runtime-image"),
    )
    .map_err(|error| error.to_string())?;
    let mut runtime =
        AuthoredRuntime::new(guest, Vec::new(), Vec::new()).map_err(|error| error.to_string())?;
    let mut side = game_side::GameSide::build(&mut runtime, assets)?;
    let _ = status.send(WorkerStatus::Ready { samples: 0, cache_hits: 0 });
    eprintln!("SKATE_PLAYER_AUDIO ready game_side");
    let trace = std::env::var_os("SKATE_AUDIO_OBSERVE").is_some();
    let mut window_peak = 0.0f32;
    let mut window_blocks = 0u32;
    // A frame or block the original code fails is skipped (and logged) rather than ending the
    // game side; only a run of failures does. Frames and blocks count apart: an exhausted guest
    // heap failed every frame while the blocks between them still rendered, and one shared
    // count, reset by each of those blocks, never reached its limit — the session ran on for
    // hours with every frame failing and the block that followed each one silent.
    let (mut frame_failures, mut block_failures) = (0u32, 0u32);
    loop {
        // The game closing its end is the app exiting.
        let Ok(next) = next_observation(observations) else {
            return Ok(());
        };
        if let Some((observation, cutscene, fe_state)) = next {
            side.fe_state = fe_state;
            match side.frame(&mut runtime, &observation, cutscene) {
                Ok(()) => {
                    frame_failures = 0;
                    // Online: this frame's own record goes to the other players (item 30b).
                    if remote::SHARING.load(std::sync::atomic::Ordering::Relaxed) {
                        let mut outbox = remote::OUTBOX.lock().unwrap_or_else(|e| e.into_inner());
                        outbox.push_back((observation.tick as u32, side.local_record()));
                        // The game thread drains it each frame; a stalled one never grows it.
                        while outbox.len() > skate_net::audio::MAX_FRAMES {
                            outbox.pop_front();
                        }
                    }
                }
                Err(error) => {
                    frame_failures += 1;
                    log_audio_error(&format!("game_side_frame_error error={error}"));
                    // ponytail: 5 s of failed frames means the game side is stuck; `run` reboots it.
                    if frame_failures >= 300 {
                        return Err(error);
                    }
                }
            }
        }
        // A failed frame still renders its block: the beds and the voices already playing go on.
        let native = match runtime.pump_once() {
            Ok(native) => {
                block_failures = 0;
                native
            }
            Err(error) => {
                block_failures += 1;
                log_audio_error(&format!("game_side_block_error error={error}"));
                // ponytail: ~2 s of failed blocks means the game side is stuck.
                if block_failures > 400 {
                    return Err(error.to_string());
                }
                vec![0.0; usize::from(PCM_CHANNELS) * PCM_FRAMES_PER_BLOCK as usize]
            }
        };
        let stereo = downmix(&native);
        trace::output(&native, usize::from(PCM_CHANNELS), &stereo);
        window_peak = native.iter().fold(window_peak, |peak, sample| peak.max(sample.abs()));
        window_blocks += 1;
        if trace && window_blocks as u64 * u64::from(PCM_FRAMES_PER_BLOCK) >= u64::from(SAMPLE_RATE)
        {
            let stats = runtime.stats();
            let audio = side.audio();
            eprintln!(
                "SKATE_PLAYER_AUDIO second block={} peak_dbfs={:.1} opened={} live={} speed={:.2} wheels={}",
                stats.blocks,
                20.0 * window_peak.max(1.0e-9).log10(),
                stats.voices_opened,
                stats.live_voices,
                audio.ground_speed_208,
                audio.wheel_count_200,
            );
            window_peak = 0.0;
            window_blocks = 0;
        }
        live.push(&stereo)?;
        pace(live);
    }
}

/// Frames the worker keeps queued for the device (32 ms). It covers the longest gap between
/// two pushes: a whole 15.6 ms timer tick (a background window's sleeps can be that coarse),
/// a block's render, and the device taking its period in one bite.
const CUSHION_FRAMES: usize = 1536;

/// Sleeps until the next block is due: [`block_wait`] for the queue this push left.
fn pace(live: &LivePcm) {
    let wait = block_wait(live.queued_frames());
    if !wait.is_zero() {
        thread::sleep(wait);
    }
}

/// How long the worker waits, after a push left `queued_frames` for the device, before
/// rendering the next block: until the device has played the queue down to [`CUSHION_FRAMES`].
/// The device's clock paces the worker. The old wall-clock deadline wrote off any lag over
/// 21 ms (four blocks), so after every stall — a map load, a slow frame, a background window
/// descheduled — the queue never refilled and every later sleep ran it dry: static. Now a
/// stall's debt is rendered back at once and the two clocks cannot drift apart. The two-block
/// cap keeps the worker taking observations when no device pulls: the queue drops its oldest.
fn block_wait(queued_frames: usize) -> Duration {
    let block_us = 1_000_000 * u64::from(PCM_FRAMES_PER_BLOCK) / u64::from(SAMPLE_RATE);
    let ahead = queued_frames.saturating_sub(CUSHION_FRAMES) as u64;
    Duration::from_micros((ahead * 1_000_000 / u64::from(SAMPLE_RATE)).min(2 * block_us))
}

/// stderr plus `%LOCALAPPDATA%/Skate3RustEngine/audio-errors.log`, so a failure in a session
/// launched without a console is still on record. A message repeating every frame is recorded
/// on its 1st, 10th, 100th, ... occurrence with its count: one line per failure wrote a 173 MB
/// log, and opened the file on the audio thread each time.
fn log_audio_error(line: &str) {
    let mut log = ERROR_LOG.lock().unwrap_or_else(|e| e.into_inner());
    // Before the worker opens the file (tools, tests) a message still reaches stderr.
    let log = log.get_or_insert_with(|| ErrorLog { file: None, seen: HashMap::new() });
    if let Some(text) = log.note(line) {
        eprintln!("SKATE_PLAYER_AUDIO {text}");
    }
}

/// Opens the session's audio error log, trimming it first: called once as the worker starts.
fn open_audio_error_log() {
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let path = std::path::Path::new(&dir).join("Skate3RustEngine/audio-errors.log");
        *ERROR_LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(ErrorLog::open(&path));
    }
}

static ERROR_LOG: Mutex<Option<ErrorLog>> = Mutex::new(None);

/// Past this size the log keeps only its newest [`ERROR_LOG_KEEP`] bytes of whole lines.
const ERROR_LOG_MAX: u64 = 1 << 20;
const ERROR_LOG_KEEP: u64 = 256 << 10;
/// ponytail: past 1000 distinct messages new ones go unrecorded — a key per message forever
/// would make the dedupe map itself the leak.
const ERROR_LOG_KEYS: usize = 1000;

struct ErrorLog {
    file: Option<std::fs::File>,
    seen: HashMap<String, u64>,
}

impl ErrorLog {
    fn open(path: &std::path::Path) -> Self {
        let _ = trim_log(path);
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
        Self { file, seen: HashMap::new() }
    }

    /// Counts one occurrence of `line`; the text written when this one is recorded.
    fn note(&mut self, line: &str) -> Option<String> {
        let known = self.seen.len();
        let count = match self.seen.get_mut(line) {
            Some(count) => {
                *count += 1;
                *count
            }
            None if known < ERROR_LOG_KEYS => {
                self.seen.insert(line.to_owned(), 1);
                1
            }
            None => return None,
        };
        let mut n = count;
        while n % 10 == 0 {
            n /= 10;
        }
        if n != 1 {
            return None;
        }
        let text = if count == 1 { line.to_owned() } else { format!("{line} (x{count})") };
        if let Some(file) = &mut self.file {
            use std::io::Write;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let _ = writeln!(file, "{stamp} {text}");
        }
        Some(text)
    }
}

/// Cuts a log over [`ERROR_LOG_MAX`] down to its newest whole lines.
fn trim_log(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= ERROR_LOG_MAX {
        return Ok(());
    }
    file.seek(SeekFrom::Start(len - ERROR_LOG_KEEP))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    drop(file);
    let start = tail.iter().position(|&b| b == b'\n').map_or(tail.len(), |i| i + 1);
    std::fs::write(path, &tail[start..])
}

fn next_observation<T>(observations: &Receiver<T>) -> Result<Option<T>, String> {
    match observations.try_recv() {
        Ok(observation) => Ok(Some(observation)),
        Err(TryRecvError::Empty) => Ok(None),
        Err(TryRecvError::Disconnected) => {
            Err("player audio observation source disconnected".into())
        }
    }
}

#[derive(Clone)]
struct Clip {
    index: usize,
    source: PcmSource,
    sample_rate: u32,
}

struct DirectVoice {
    tag: Option<&'static str>,
    clip: Clip,
    frame: f64,
    gain: f32,
    target_gain: f32,
    repeat: bool,
    releasing: bool,
}

/// Mixes decoded retail samples for gameplay families whose patch payloads are not recovered yet.
/// The authored runtime remains authoritative for grinds; this host fallback makes the rest of the
/// rider and board audible from the correct shipped banks instead of substituting synthetic audio.
struct DirectMixer {
    banks: HashMap<String, Vec<Clip>>,
    voices: Vec<DirectVoice>,
    serial: usize,
    last_airborne: bool,
    last_wipeout: bool,
    last_speed: f32,
    last_footstep: f32,
    last_push: f32,
    next_footstep_tick: u64,
    next_push_tick: u64,
    next_skid_tick: u64,
    next_squeak_tick: u64,
    landing_skid_suppressed_until_tick: u64,
    offboard_step_distance: f32,
    offboard_step_side: usize,
}

impl DirectMixer {
    fn new(samples: &[skate_data::audio::catalog::BankPcm]) -> Self {
        let mut banks: HashMap<String, Vec<Clip>> = HashMap::new();
        for sample in samples {
            banks.entry(sample.bank.clone()).or_default().push(Clip {
                index: sample.index,
                source: sample.pcm.source.clone(),
                sample_rate: sample.pcm.sample_rate,
            });
        }
        Self {
            banks,
            voices: Vec::new(),
            serial: 0,
            last_airborne: false,
            last_wipeout: false,
            last_speed: 0.0,
            last_footstep: 0.0,
            last_push: 0.0,
            next_footstep_tick: 0,
            next_push_tick: 0,
            next_skid_tick: 0,
            next_squeak_tick: 0,
            landing_skid_suppressed_until_tick: 0,
            offboard_step_distance: 0.0,
            offboard_step_side: 0,
        }
    }

    /// Retail `Class_Flips` and `cloth_trick` divide the clip banks by trick family. The exact
    /// variants below are the groups opened by the recorded ollie, flip and shuv payloads; the
    /// counter still varies the member within each authored family.
    fn trick_groups(identifier: Option<&str>) -> (&'static [usize], &'static [usize]) {
        let name = identifier.unwrap_or_default().to_ascii_lowercase();
        if name.contains("shuv") {
            (&[6, 16, 17], &[24, 25, 26, 27])
        } else if name.contains("kickflip")
            || name.contains("heelflip")
            || name.contains("hardflip")
            || name.contains("laserflip")
            || name.contains("360flip")
            || name.contains("varial")
        {
            (&[5, 9, 18, 21], &[15, 16, 17, 18, 19])
        } else {
            (&[10, 11, 12, 13], &[8, 9, 10, 11, 12, 13, 14])
        }
    }

    fn choose(&mut self, bank: &str, salt: usize) -> Option<Clip> {
        let clips = self.banks.get(bank)?;
        let clip = clips
            .get((self.serial.wrapping_add(salt)) % clips.len())?
            .clone();
        self.serial = self.serial.wrapping_add(1);
        Some(clip)
    }

    fn choose_from(&mut self, bank: &str, indices: &[usize], salt: usize) -> Option<Clip> {
        let clips = self.banks.get(bank)?;
        let start = self.serial.wrapping_add(salt);
        let wanted = indices.get(start % indices.len())?;
        let clip = clips.iter().find(|clip| clip.index == *wanted)?.clone();
        self.serial = self.serial.wrapping_add(1);
        Some(clip)
    }

    fn play(&mut self, bank: &str, salt: usize, gain: f32) {
        if let Some(clip) = self.choose(bank, salt) {
            self.voices.push(DirectVoice {
                tag: None,
                clip,
                frame: 0.0,
                gain,
                target_gain: gain,
                repeat: false,
                releasing: false,
            });
        }
    }

    fn play_from(&mut self, bank: &str, indices: &[usize], salt: usize, gain: f32) {
        if let Some(clip) = self.choose_from(bank, indices, salt) {
            self.voices.push(DirectVoice {
                tag: None,
                clip,
                frame: 0.0,
                gain,
                target_gain: gain,
                repeat: false,
                releasing: false,
            });
        }
    }

    fn set_loop(&mut self, tag: &'static str, bank: &str, salt: usize, gain: f32, enabled: bool) {
        if !enabled {
            if let Some(voice) = self.voices.iter_mut().find(|voice| voice.tag == Some(tag)) {
                voice.target_gain = 0.0;
                voice.releasing = true;
            }
            return;
        }
        if let Some(voice) = self.voices.iter_mut().find(|voice| voice.tag == Some(tag)) {
            voice.target_gain = gain;
            voice.releasing = false;
            return;
        }
        if let Some(clip) = self.choose(bank, salt) {
            self.voices.push(DirectVoice {
                tag: Some(tag),
                clip,
                frame: 0.0,
                gain: 0.0,
                target_gain: gain,
                repeat: true,
                releasing: false,
            });
        }
    }

    fn set_loop_from(
        &mut self,
        tag: &'static str,
        bank: &str,
        indices: &[usize],
        salt: usize,
        gain: f32,
        enabled: bool,
    ) {
        if !enabled {
            if let Some(voice) = self.voices.iter_mut().find(|voice| voice.tag == Some(tag)) {
                voice.target_gain = 0.0;
                voice.releasing = true;
            }
            return;
        }
        if let Some(voice) = self.voices.iter_mut().find(|voice| voice.tag == Some(tag)) {
            voice.target_gain = gain;
            voice.releasing = false;
            return;
        }
        if let Some(clip) = self.choose_from(bank, indices, salt) {
            self.voices.push(DirectVoice {
                tag: Some(tag),
                clip,
                frame: 0.0,
                gain: 0.0,
                target_gain: gain,
                repeat: true,
                releasing: false,
            });
        }
    }

    fn observe(&mut self, observation: &PlayerAudioObservation) {
        let speed = observation.board_speed.max(observation.rider_speed);
        let airborne = matches!(observation.state, 103 | 200 | 201 | 202);
        if observation.landed {
            // A vertical landing commonly sheds enough reported horizontal speed to satisfy the
            // fallback deceleration heuristic. Retail does not turn that impact into a wheel-skid
            // post: Class_Treatment owns the landing, while Class_wheels_skid is reserved for
            // actual stops/reverts/slip. Cover the settling frames as well as the landing tick.
            self.landing_skid_suppressed_until_tick = observation.tick.saturating_add(12);
        }
        let rolling = observation.grounded
            && !observation.grinding
            && !observation.wiping_out
            && speed > 0.35;
        let surface = observation.wheel_surface as usize;
        let foot_surface = if observation.state / 100 == 5 {
            observation.foot_surface as usize
        } else {
            surface
        };
        let footstep_slots = material_slots::<8>(foot_surface);
        let foot_drag_slots = material_slots::<7>(surface);
        let wheel_skid_slots = material_slots::<4>(surface);
        if observation.landed {
            // `Class_Treatment` is a held retail message: its patch picks the impact sample
            // from the ten following re-deliveries. The authored treatment graph is posted and
            // updated above, but its voice path is not yet reliably audible on the host mixer.
            // Keep the authored path alive and supply the matching shipped-bank one-shot until
            // that path can replace this fallback. This must be keyed to the physics landing
            // edge, never to speed loss (which would turn ordinary braking into a landing).
            let strength = landing_treatment_strength(
                observation.landing_impact_speed,
                observation.landing_clean,
                observation.landing_sketchy,
                observation.landing_type,
            );
            let gain = treatment_fallback_gain(strength);
            // The recorded ordinary ollie landing opens Treatments slot 17.  Do not turn every
            // impact into that slot: the heavier/sketchy branches have not yet been exercised
            // through the retail selector, so retain the shipped bank's variation for those.
            let ordinary_retail_landing = (600..=750).contains(&strength)
                && !observation.landing_sketchy
                && observation.landing_type == 0;
            if ordinary_retail_landing {
                self.play_from("Treatments.abk", &[17], observation.tick as usize, gain);
            } else {
                self.play(
                    "Treatments.abk",
                    observation.tick as usize + observation.landing_type as usize,
                    gain,
                );
            }
            eprintln!(
                "SKATE_PLAYER_AUDIO landing_fallback tick={} impact={:.3} strength={} clean={} sketchy={} type={} source={}",
                observation.tick,
                observation.landing_impact_speed,
                strength,
                observation.landing_clean,
                observation.landing_sketchy,
                observation.landing_type,
                if ordinary_retail_landing {
                    "treatments:17"
                } else {
                    "treatments:severity-fallback"
                },
            );
        }
        let roll_gain = (0.015 + speed * 0.0035).min(0.065);
        self.set_loop_from(
            "rolling",
            "PatchBank_Rolling_Surfaces.abk",
            // These are the four surface-loop slots actually opened by Class_rolling across the
            // retail captures. One material loop is active at a time; layering two fixed slots was
            // the source of the uniform board hum in the previous build.
            &[11, 12, 14, 15],
            surface,
            roll_gain,
            rolling,
        );
        self.set_loop_from(
            "rattle",
            "Rolling_Rattles.abk",
            // The captured Rolling_Rattle_Class opens its looping speed layers 6/7/8. Audio
            // material is not an input to this family; selecting it from the surface ID made a
            // concrete change sound like a different set of trucks.
            &[6, 7, 8],
            (speed / 2.5) as usize,
            (speed * 0.0045).min(0.05),
            rolling && speed > 1.5,
        );
        self.set_loop_from(
            "wind",
            "sense_of_speed.abk",
            &[0, 1, 2],
            0,
            ((speed - 5.0) * 0.005).clamp(0.0, 0.055),
            speed > 5.0 && !observation.wiping_out,
        );
        self.set_loop_from(
            "speed-rattle",
            "sense_of_speed.abk",
            &[15, 16],
            0,
            ((speed - 7.0) * 0.004).clamp(0.0, 0.04),
            speed > 7.0 && !observation.wiping_out,
        );

        // `c_board_slide` belongs to a loose/deck scrape, not the rail-grind loop. Layering it on
        // every boardslide/tipslide/darkslide doubled the authored Class_grind scrape and made all
        // three families sound alike. The retail traces post it beside bail/ground-slide activity.
        let board_slide = observation.wiping_out && observation.contact_count > 0 && speed > 0.4;
        self.set_loop_from(
            "board-slide",
            "board_scrapes.abk",
            &[10, 11, 12, 13, 14],
            surface,
            0.11,
            board_slide,
        );
        self.set_loop_from(
            "body-slide",
            "Bodyslide.abk",
            &[0, 5, 15],
            surface,
            0.13,
            observation.wiping_out && observation.contact_count > 0 && speed > 0.4,
        );
        self.set_loop_from(
            "foot-drag",
            "FOOT_DRAG.abk",
            &foot_drag_slots,
            surface,
            (observation.foot_push_speed.abs() * 0.03).clamp(0.04, 0.12),
            matches!(observation.state, 101 | 102) && observation.foot_push_speed.abs() > 0.35,
        );

        // The animation signal can remain above zero through an entire planted-foot interval.
        // A rising-edge-only trigger consequently missed later steps when two contacts overlapped.
        // Keep the retail variants, but permit another contact after a short gait cooldown.
        if observation.footstep_strength > 0.05 && observation.tick >= self.next_footstep_tick {
            self.play_from(
                "fstep_skateshoe1_sm.abk",
                &footstep_slots,
                observation.footstep_bone.unsigned_abs() as usize + observation.tick as usize,
                (0.14 + observation.footstep_strength * 0.08).min(0.22),
            );
            self.next_footstep_tick = observation.tick + 18;
            self.offboard_step_distance = 0.0;
        } else if observation.footstep_strength <= 0.05 && observation.state == 500 {
            if advance_offboard_stride(&mut self.offboard_step_distance, observation.rider_speed)
                && observation.tick >= self.next_footstep_tick
            {
                self.play_from(
                    "fstep_skateshoe1_sm.abk",
                    &footstep_slots,
                    observation.tick as usize + self.offboard_step_side,
                    (0.13 + observation.rider_speed * 0.018).min(0.21),
                );
                self.offboard_step_side ^= 1;
                self.next_footstep_tick = observation.tick + 8;
            }
        } else {
            self.offboard_step_distance = 0.0;
        }
        if observation.foot_push_speed.abs() > 0.35 && observation.tick >= self.next_push_tick {
            self.play_from(
                "fstep_skateshoe1_sm.abk",
                &footstep_slots,
                surface + observation.tick as usize,
                0.16,
            );
            self.next_push_tick = observation.tick + 16;
        }
        if !self.last_airborne && airborne && !observation.wiping_out {
            let (_, cloth_clips) = Self::trick_groups(observation.trick_identifier.as_deref());
            let stance_salt = usize::from(observation.riding_switch)
                + usize::from(observation.riding_fakie) * 2
                + usize::from(observation.nollie) * 4;
            self.play_from(
                "Foley_Cloth.abk",
                cloth_clips,
                observation.tick as usize + stance_salt,
                0.10,
            );
            // The bank is 24 authored audio materials by four variants. Selecting from the
            // current material group preserves the retail surface identity at pop time.
            self.play_from(
                "WHEEL_SKID_BANK.abk",
                &wheel_skid_slots,
                observation.tick as usize + stance_salt,
                0.024,
            );
        }
        if observation.wiping_out && !self.last_wipeout {
            // `c_cloth_falls` occupies the fall/impact families left outside the trick selector's
            // ollie (8..14), flip (15..19), and shuv (24..27) groups.
            self.play_from(
                "Foley_Cloth.abk",
                &[0, 1, 2, 3, 4, 5, 6, 7],
                observation.tick as usize,
                0.20,
            );
        }
        let deceleration = self.last_speed - speed;
        if deceleration_skid_allowed(
            rolling,
            observation.powersliding,
            self.last_airborne,
            observation.tick,
            self.landing_skid_suppressed_until_tick,
            self.next_skid_tick,
            deceleration,
        ) {
            self.play_from(
                "WHEEL_SKID_BANK.abk",
                &wheel_skid_slots,
                surface + observation.tick as usize,
                (0.050 + deceleration * 0.016).min(0.10),
            );
            self.next_skid_tick = observation.tick + 8;
        }
        let sliding_or_reverting =
            observation.powersliding || matches!(observation.state, 101 | 102);
        if sliding_or_reverting
            && observation.contact_count > 0
            && speed > 0.5
            && observation.tick >= self.landing_skid_suppressed_until_tick
            && observation.tick >= self.next_squeak_tick
        {
            self.play_from(
                "Brd_Squeaks.abk",
                &[2, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17],
                surface + observation.tick as usize,
                (0.13 + speed * 0.012).min(0.24),
            );
            self.next_squeak_tick = observation.tick + (13.0 - speed * 0.5).clamp(6.0, 11.0) as u64;
        }
        if matches!(observation.state, 101 | 102)
            && observation.contact_count > 0
            && speed > 0.5
            && observation.tick >= self.landing_skid_suppressed_until_tick
            && observation.tick >= self.next_skid_tick
        {
            self.play_from(
                "WHEEL_SKID_BANK.abk",
                &wheel_skid_slots,
                observation.tick as usize + surface,
                (0.085 + speed * 0.007).min(0.15),
            );
            self.next_skid_tick = observation.tick + 9;
        }

        self.last_airborne = airborne;
        self.last_wipeout = observation.wiping_out;
        self.last_speed = speed;
        self.last_footstep = observation.footstep_strength;
        self.last_push = observation.foot_push_speed;
    }

    fn mix(&mut self, stereo: &mut [f32]) {
        for frame in stereo.chunks_exact_mut(2) {
            let mut left = 0.0;
            let mut right = 0.0;
            for voice in &mut self.voices {
                voice.gain += (voice.target_gain - voice.gain) * 0.0025;
                let source_frame = voice.frame as usize;
                let (source_left, source_right) =
                    direct_stereo_frame(&voice.clip.source, source_frame);
                left += source_left * voice.gain;
                right += source_right * voice.gain;
                voice.frame += f64::from(voice.clip.sample_rate) / f64::from(SAMPLE_RATE);
                if voice.frame as usize >= voice.clip.source.frames() && voice.repeat {
                    let (start, end) = voice
                        .clip
                        .source
                        .loop_range()
                        .unwrap_or((0, voice.clip.source.frames()));
                    let length = end.saturating_sub(start).max(1);
                    voice.frame =
                        (start + (voice.frame as usize).saturating_sub(start) % length) as f64;
                }
            }
            frame[0] = (frame[0] + left).clamp(-1.0, 1.0);
            frame[1] = (frame[1] + right).clamp(-1.0, 1.0);
        }
        self.voices.retain(|voice| {
            (!voice.releasing || voice.gain > 0.0005)
                && (voice.repeat || (voice.frame as usize) < voice.clip.source.frames())
        });
    }
}

/// The first live family uses the recovered `Class_grind` constructor/update payload. Additional
/// families stay out until their gameplay constructors have typed inputs; inventing payload words
/// here would make the graph sound plausible while bypassing the authored selection programs.
#[derive(Default)]
struct EventProducer {
    treatment: Option<u32>,
    treatment_phase: usize,
    rolling: Option<[u32; 2]>,
    rattle: Option<u32>,
    rattle_next_tick: u64,
    rattle_speed: u32,
    footsteps: Option<[u32; 2]>,
    foot_down: [bool; 2],
    foot_value: [u32; 2],
    grind: Option<u32>,
    flip: Option<u32>,
    flip_started: u64,
    last_airborne: bool,
    last_trick: Option<String>,
}

impl EventProducer {
    /// Retail `sub_824C9830` posts two held `Class_rolling` messages (layers 0 and 3) once, and
    /// `sub_824C9948` rewrites and re-delivers both every frame for the rest of the session. They
    /// are never released while the rider owns the board; silence comes from the gain word.
    fn update_rolling(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<(), String> {
        let speed = rolling_speed_word(observation.board_speed);
        let handles = match self.rolling {
            Some(handles) => handles,
            None => {
                let mut handles = [0; 2];
                for (handle, layer) in handles.iter_mut().zip(ROLLING_LAYERS) {
                    *handle = runtime
                        .post(
                            "Class_rolling",
                            &rolling_constructor(speed, layer.selector, DEFAULT_ROLLING_SURFACE),
                        )
                        .map_err(|error| error.to_string())?;
                }
                self.rolling = Some(handles);
                eprintln!(
                    "SKATE_PLAYER_AUDIO rolling_start tick={} handles={:#010x},{:#010x}",
                    observation.tick, handles[0], handles[1]
                );
                handles
            }
        };
        let contact = observation.grounded && !observation.grinding && !observation.wiping_out;
        for (handle, layer) in handles.into_iter().zip(ROLLING_LAYERS) {
            let gain = if contact {
                rolling_gain_word(speed, layer.gain_cap)
            } else {
                0
            };
            runtime
                .redeliver(
                    handle,
                    &rolling_update(speed, layer.selector, DEFAULT_ROLLING_SURFACE, gain),
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Retail `sub_824C6198`: while the wheels are down, each rattle trigger releases the previous
    /// `Rolling_Rattle_Class` message and posts a fresh one carrying the current speed. The retail
    /// traces show those triggers about every 1.07 s, including at rest (speed word 0).
    fn update_rattle(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<(), String> {
        let contact = observation.grounded && !observation.grinding && !observation.wiping_out;
        if !contact {
            self.rattle_next_tick = 0;
        } else if observation.tick >= self.rattle_next_tick {
            self.rattle_next_tick = observation.tick + RATTLE_RETRIGGER_TICKS;
            if let Some(handle) = self.rattle.take() {
                runtime.release(handle).map_err(|error| error.to_string())?;
            }
            let speed = rattle_speed_word(observation.board_speed);
            self.rattle_speed = speed;
            let handle = runtime
                .post(
                    "Rolling_Rattle_Class",
                    &rattle_constructor(speed, DEFAULT_RATTLE_SURFACE, RATTLE_TWEAK),
                )
                .map_err(|error| error.to_string())?;
            self.rattle = Some(handle);
        }
        // `sub_824C80C0` re-delivers the held rattle every frame, contact or not; its gain words
        // follow the live wheel contact, so the voice is silent in the air and between triggers.
        if let Some(handle) = self.rattle {
            let contact_gain = if contact {
                rolling_gain_word(
                    rolling_speed_word(observation.board_speed),
                    ROLLING_LAYERS[0].gain_cap,
                )
            } else {
                0
            };
            runtime
                .redeliver(
                    handle,
                    &rattle_update(
                        self.rattle_speed,
                        DEFAULT_RATTLE_SURFACE,
                        RATTLE_TWEAK,
                        contact_gain,
                    ),
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Retail `sub_824E9FD8` posts two held `playercharacter_footstep` messages (object +36 first
    /// updated, +220 second) and `sub_824EAEA8` rewrites and re-delivers both every frame. Word 8
    /// is each foot's *down level* (`sub_824E9270` copies audio-state bytes +724/+725 into +52/+236
    /// every frame); the authored patch starts its heel/toe/scuff layers on the 0->1 edge, so a
    /// foot held down must not re-trigger. On the board the +36 foot is down for the duration of
    /// the animation's `push_contact` (all 193 push plants in the recorded `play1` session used
    /// it); walking uses the native offboard foot manager's per-foot support flags.
    fn update_footsteps(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<(), String> {
        let handles = match self.footsteps {
            Some(handles) => handles,
            None => {
                let constructor = footstep_constructor();
                let handles = [
                    runtime
                        .post("playercharacter_footstep", &constructor)
                        .map_err(|error| error.to_string())?,
                    runtime
                        .post("playercharacter_footstep", &constructor)
                        .map_err(|error| error.to_string())?,
                ];
                self.footsteps = Some(handles);
                handles
            }
        };
        let walking = observation.state == 500 && !observation.wiping_out;
        let pushing = observation.foot_push_speed > 0.0 && !observation.wiping_out;
        let down = [
            pushing || (walking && observation.feet_supported[0]),
            walking && observation.feet_supported[1],
        ];
        for foot in 0..2 {
            if down[foot] && !self.foot_down[foot] {
                self.foot_value[foot] = if pushing && foot == 0 {
                    push_foot_value(observation.foot_push_speed)
                } else {
                    WALKING_FOOT_VALUE
                };
                eprintln!(
                    "SKATE_PLAYER_AUDIO foot_down tick={} foot={foot} {} value={}",
                    observation.tick,
                    if pushing && foot == 0 { "push" } else { "step" },
                    self.foot_value[foot]
                );
            }
        }
        self.foot_down = down;
        let body_speed = (observation.rider_speed * 100.0).round().clamp(0.0, 1000.0) as u32;
        // Word 13 is `fctiwz` of the animation's AudibleFootStepStrength, clamped to 1..99.
        let strength = (observation.footstep_strength as i32).clamp(1, 99) as u32;
        for (foot, handle) in handles.into_iter().enumerate() {
            runtime
                .redeliver(
                    handle,
                    &footstep_update(down[foot], self.foot_value[foot], body_speed, strength),
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn update_treatment(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<(), String> {
        let handle = match self.treatment {
            Some(handle) => handle,
            None => {
                // Exact Class_Treatment constructor message from the retail session trace. The
                // object is posted once when the player-audio owner starts and remains held.
                let post = [
                    0, 0x7fff, 0, 0, 0x1000, 0x61a8, 0, 0, 0, 0, 0x01f4, 0, 0, 0, 0, 0x1b58,
                    0x6d60, 0x7fff, 0, 1, 1, 8,
                ];
                let handle = runtime
                    .post("Class_Treatment", &post)
                    .map_err(|error| error.to_string())?;
                self.treatment = Some(handle);
                eprintln!(
                    "SKATE_PLAYER_AUDIO treatment_start tick={} handle={handle:#010x}",
                    observation.tick
                );
                handle
            }
        };

        if observation.landed {
            self.treatment_phase = 1;
        }
        let update = treatment_update(self.treatment_phase);
        runtime
            .redeliver(handle, &update)
            .map_err(|error| error.to_string())?;
        if self.treatment_phase != 0 {
            self.treatment_phase += 1;
            if self.treatment_phase > TREATMENT_LANDING_UPDATES.len() {
                self.treatment_phase = 0;
            }
        }
        Ok(())
    }

    fn flip_selector(observation: &PlayerAudioObservation) -> u32 {
        let name = observation
            .trick_identifier
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches("N_")
            .trim_start_matches("90_")
            .to_ascii_lowercase();
        let selector = if name.contains("360inwardheelflip") {
            14
        } else if name.contains("360hardflip") {
            13
        } else if name.contains("laserflip") {
            12
        } else if name.contains("360flip") {
            11
        } else if name.contains("fs360popshuv") {
            10
        } else if name.contains("360popshuv") {
            9
        } else if name.contains("inwardheelflip") {
            8
        } else if name.contains("hardflip") {
            7
        } else if name.contains("varialheelflip") {
            6
        } else if name.contains("varialkickflip") {
            5
        } else if name.contains("fspopshuv") {
            4
        } else if name.contains("popshuv") || name.contains("shuv") {
            3
        } else if name.contains("heelflip") {
            2
        } else if name.contains("kickflip") {
            1
        } else if observation.nollie || name == "nollie" {
            29
        } else {
            28
        };
        selector
    }

    fn start_flip(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<(), String> {
        if let Some(handle) = self.flip.take() {
            runtime.release(handle).map_err(|error| error.to_string())?;
        }
        let selector = Self::flip_selector(observation);
        let speed = observation.board_speed.max(observation.rider_speed);
        let strength = (650.0 + speed * 45.0).round().clamp(0.0, 1000.0) as u32;
        // Recovered Class_Flips constructor layout. Words 7..11 are its five clamped gameplay
        // inputs; words 16..27 are the stock voice-property defaults captured from retail.
        let post = [
            0, 32_767, 0, 0, 0x1000, 0x61a8, 0, 0, 0, strength, 500, selector, 0, 0, 0, 0, 1,
            0x6bfe, 0x3a00, 0x1a00, 0, 0x2710, 0, 0, 1, 1, 0, 6,
        ];
        let handle = runtime
            .post("Class_Flips", &post)
            .map_err(|error| error.to_string())?;
        self.flip = Some(handle);
        self.flip_started = observation.tick;
        eprintln!(
            "SKATE_PLAYER_AUDIO flip_start tick={} trick={:?} selector={} strength={} handle={handle:#010x}",
            observation.tick, observation.trick_identifier, selector, strength
        );
        Ok(())
    }

    fn apply(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<(), String> {
        self.update_treatment(runtime, observation)?;
        self.update_rolling(runtime, observation)?;
        self.update_rattle(runtime, observation)?;
        self.update_footsteps(runtime, observation)?;
        let airborne = matches!(observation.state, 103 | 200 | 201 | 202);
        let trick_changed = observation.trick_identifier.is_some()
            && observation.trick_identifier != self.last_trick;
        let took_off = !self.last_airborne && airborne;
        if !observation.wiping_out && airborne && (took_off || trick_changed) {
            self.start_flip(runtime, observation)?;
        }
        if let Some(handle) = self.flip {
            if !airborne
                || observation.wiping_out
                || observation.tick.saturating_sub(self.flip_started) > 36
            {
                runtime.release(handle).map_err(|error| error.to_string())?;
                self.flip = None;
            } else {
                let speed = observation.board_speed.max(observation.rider_speed);
                let strength = (650.0 + speed * 45.0).round().clamp(0.0, 1000.0) as u32;
                let selector = Self::flip_selector(observation);
                // Retail re-delivery replaces the constructor header and drives the patch's gain,
                // pitch and trick-selector ramps. Without this update, the selected voice is
                // intentionally opened at zero gain.
                let update = [
                    0x38d7, 32_767, 0, 0x0999, 0x0fe6, 0x618b, 0, 0, 0, strength, 500, selector, 0,
                    0, 0, 0, 1,
                ];
                runtime
                    .redeliver(handle, &update)
                    .map_err(|error| error.to_string())?;
            }
        }

        if observation.grinding {
            let physical_speed = observation.board_speed.max(observation.rider_speed);
            let (speed, level) = grind_controls(physical_speed);
            let surface = observation.grind_audio_surface.min(14);
            // sub_824C39E0 maps the six retained physical grind families to the authored patch
            // layers before posting/updating: boardslide=2, 50-50/tipslide/backslash=0,
            // five-o=2, darkslide=3. Previously every grind was forced through layer 2.
            let layer = grind_layer(observation.grind_family);
            if let Some(handle) = self.grind {
                // Recorded Class_grind updates replace the constructor's first word and publish
                // the live level, contact scalar, filters, speed, surface and grind layer.
                let update = [
                    32_767, level, 0x0434, 0, 0x1000, 0x618b, 0x004d, speed, 0x0400, surface,
                    layer, 0x628e, 0, 0, 0, 0, 5,
                ];
                runtime
                    .redeliver(handle, &update)
                    .map_err(|error| error.to_string())?;
            } else {
                // Exact 17-word constructor payload from the retail Class_grind trace. Layer 2 is
                // the sustained rail voice; the old shifted payload selected near-silent impulses.
                let post = [
                    0, 32_767, 0, 0, 0, 25_000, 0, speed, 0x0400, surface, layer, 0x628e, 0, 0, 0,
                    0, 5,
                ];
                let handle = runtime
                    .post("Class_grind", &post)
                    .map_err(|error| error.to_string())?;
                eprintln!(
                    "SKATE_PLAYER_AUDIO grind_start tick={} speed={physical_speed:.3} family={} substate={} surface={} impact={:.3} layer={} handle={handle:#010x}",
                    observation.tick,
                    observation.grind_family,
                    observation.grind_substate,
                    observation.grind_audio_surface,
                    observation.grind_impact_speed,
                    layer
                );
                self.grind = Some(handle);
            }
        } else if let Some(handle) = self.grind.take() {
            runtime.release(handle).map_err(|error| error.to_string())?;
            eprintln!(
                "SKATE_PLAYER_AUDIO grind_stop tick={} handle={handle:#010x}",
                observation.tick
            );
        }
        self.last_airborne = airborne;
        self.last_trick = observation.trick_identifier.clone();
        Ok(())
    }
}

// Wheel packets, from the retail constructors `sub_824C4C18` (Class_rolling) and `sub_824B0248`
// (Rolling_Rattle_Class). Each constructor fills a 52-byte object and posts object+4, so a packet
// is exactly 12 words. Retail trace lines print 28 words; everything past word 11 is the next heap
// block (including the next object's message node, which is not a linked control graph).

/// Surface word for every wheel packet until per-material mapping exists. 2 is the surface retail
/// reports on ~71% of rolling updates in the recorded sessions.
const DEFAULT_ROLLING_SURFACE: u32 = 2;
/// Rattle's surface code for rolling surface 2, as observed in every retail surface-2 rattle post.
const DEFAULT_RATTLE_SURFACE: u32 = 2;
/// Rattle's sixth tuning input; constant (8) in every recorded retail rattle post.
const RATTLE_TWEAK: u32 = 8;
/// Retail rattle triggers: median 1.09 s, p10 0.97 s apart while the wheels are down (60 Hz ticks).
const RATTLE_RETRIGGER_TICKS: u64 = 64;

struct RollingLayer {
    selector: u32,
    /// Retail gain word (vfunc 7) once rolling faster than ~0.92 m/s.
    gain_cap: u32,
}

/// `sub_824C9830` posts layer 0 into holder +1304 and layer 3 into +1308.
const ROLLING_LAYERS: [RollingLayer; 2] = [
    RollingLayer {
        selector: 0,
        gain_cap: 12_999,
    },
    RollingLayer {
        selector: 3,
        gain_cap: 4_913,
    },
];

/// `sub_824C9948`: `clamp(v * 3.6 / maxSpeed[layer], 0, 1) * 10000`, truncated (`fctiwz`). The 3.6
/// and 10000 are image constants (0x822F8628, 0x821161A0). `maxSpeed` is the AttribSys float array
/// 0x880C82E8EF647EC4 read by `sub_824C97B8(layer)`; the owner's skatercollections vault holds
/// [70, 65, 65, 70, 100, 45, ...], so both held layers (0 and 3) use 70 km/h.
fn rolling_speed_word(board_speed: f32) -> u32 {
    const MAX_SPEED_KMH: f32 = 70.0;
    ((board_speed * 3.6 / MAX_SPEED_KMH).clamp(0.0, 1.0) * 10_000.0) as u32
}

/// `sub_824C6198`: `clamp((v - 1) * 3.6 / D, 0, 1) * 10000`, truncated. `D` is the AttribSys float
/// 0x12275AA8AC4A63FB, 30 km/h in the owner's vault (independently regressed from 69 recorded
/// rattle posts as 30.5).
fn rattle_speed_word(board_speed: f32) -> u32 {
    const DIVISOR_KMH: f32 = 30.0;
    (((board_speed - 1.0) * 3.6 / DIVISOR_KMH).clamp(0.0, 1.0) * 10_000.0) as u32
}

/// Recorded retail gain rises linearly with the speed word and saturates at 460 (≈0.92 m/s).
fn rolling_gain_word(speed: u32, cap: u32) -> u32 {
    ((u64::from(speed) * u64::from(cap)) / 460).min(u64::from(cap)) as u32
}

/// `sub_824C4C18` constructor fields, object +4..+48.
fn rolling_constructor(speed: u32, selector: u32, surface: u32) -> [u32; 12] {
    [
        0,
        0,
        0x1000,
        speed.min(10_000),
        selector.min(15),
        0,
        surface.min(13),
        0,
        0,
        25_000,
        0,
        32_767,
    ]
}

/// Footstep packets: 25 words, from the retail constructor `sub_824B73E0` (as called by
/// `sub_824E9FD8`) and updater `sub_824EAEA8`. Retail update trace lines print only words 0..16;
/// words 17..24 come from the updater itself.
///
/// Word 10 on a walking step: the median of 20,000+ recorded retail step frames.
const WALKING_FOOT_VALUE: u32 = 48;
/// Words 18..23: AttribSys Int32 array 0x636464FBAD0D71A3 (six entries, read through
/// `sub_824ADF90` every frame) from the owner's skatercollections vault. Word 23 multiplies the
/// footstep patch's start gain, so it is required for any audible step.
const FOOTSTEP_TUNING: [u32; 6] = [32_767, 10_000, 15_000, 25_000, 32_767, 28_000];

fn footstep_constructor() -> [u32; 25] {
    let mut words = [0u32; 25];
    words[2] = 0x1000;
    words[4] = 25_000;
    words[7] = 32_767;
    words[12] = 1;
    words[13] = 1;
    words[16] = 1;
    words[17] = 1;
    words[24] = 12;
    words
}

/// Retail push plants carry word 10 at p10 169 / p50 513 / p90 527 against engine foot push speeds
/// of p10 0.68 / p50 3.25 / p90 7.45 m/s; this linear map reproduces that distribution.
fn push_foot_value(foot_push_speed: f32) -> u32 {
    (foot_push_speed * 160.0).round().clamp(0.0, 527.0) as u32
}

/// `sub_824EAEA8` per-foot rewrite. Words 1 (azimuth), 2, 4..7 are the owner's spatial virtuals,
/// constant with the follow camera; 12, 15, 16, 17 are constant in the recorded sessions
/// (16 = default surface); 13 is the step strength; 14 is the rider speed in cm/s.
fn footstep_update(planted: bool, foot_value: u32, body_speed: u32, strength: u32) -> [u32; 25] {
    let mut words = [0u32; 25];
    words[0] = 32_767;
    words[2] = 4_086;
    words[4] = 24_971;
    words[5] = 77;
    words[6] = 1_789;
    words[7] = 9_202;
    words[8] = u32::from(planted);
    words[9] = if planted { 2 } else { 0 };
    words[10] = foot_value.min(1_000);
    words[12] = 1;
    words[13] = strength.clamp(1, 99);
    words[14] = body_speed.min(1_000);
    words[15] = 1;
    words[16] = 2;
    words[17] = 1;
    words[18..24].copy_from_slice(&FOOTSTEP_TUNING);
    words[24] = 12;
    words
}

/// `sub_824C9948` per-frame rewrite. Words 1, 2, 8, 9 and 10 come from the owner's spatial
/// virtuals; with the follow camera they are constant in every recorded session except word 1, a
/// small azimuth, which is left centred.
fn rolling_update(speed: u32, selector: u32, surface: u32, gain: u32) -> [u32; 12] {
    [
        32_767,
        0,
        4_086,
        speed.min(10_000),
        selector.min(15),
        0,
        surface.min(13),
        0,
        0,
        24_971,
        77,
        gain.min(32_767),
    ]
}

/// `sub_824B0248` constructor fields, object +4..+48.
fn rattle_constructor(speed: u32, surface: u32, tweak: u32) -> [u32; 12] {
    [
        0,
        0,
        0x1000,
        speed.min(10_000),
        surface.min(8),
        0,
        1,
        0,
        25_000,
        0,
        32_767,
        tweak.min(32_767),
    ]
}

/// `sub_824C80C0` per-frame rewrite (object +4, +8, +12, +32, +36, +40, +44); the speed, surface and
/// tweak words keep their constructor values. Across 60,863 recorded frames word 10 is 0.92x and
/// word 7 0.131x the concurrent rolling layer-0 gain, both zero whenever that gain is zero.
fn rattle_update(speed: u32, surface: u32, tweak: u32, contact_gain: u32) -> [u32; 12] {
    let gain = u64::from(contact_gain);
    [
        32_767,
        0,
        4_055,
        speed.min(10_000),
        surface.min(8),
        0,
        1,
        (gain * 131 / 1000) as u32,
        24_971,
        77,
        (gain * 9226 / 10_000).min(32_767) as u32,
        tweak.min(32_767),
    ]
}

// These are ordered 60 Hz re-deliveries captured from the held retail Class_Treatment during a
// normal ollie landing. Replaying the sequence through the authored Treatments patch opens one
// treatment voice; directly choosing sample slots 13/14 bypassed its severity/variation graph.
const TREATMENT_LANDING_UPDATES: [[u32; 3]; 10] = [
    [0x0010, 0x02ae, 0x0025],
    [0x0032, 0x028c, 0x0039],
    [0x0064, 0x025a, 0x005d],
    [0x00c8, 0x01f6, 0x0098],
    [0x010a, 0x01b4, 0x00b2],
    [0x013c, 0x0182, 0x00b7],
    [0x0190, 0x012e, 0x00b7],
    [0x01f4, 0x00ca, 0x00b7],
    [0x028a, 0x0034, 0x00b7],
    [0x02ab, 0x0013, 0x00b7],
];

/// Retail updater `sub_824DD6F0`. Word 10 is `timeScale * 500` (image constant 0x820BD5C4 = 500,
/// applied to the global slow-motion timer that also gates Hall of Meat), so it stays 500 at
/// normal game speed. Writing a landing strength here played the Treatments patch up to 2x fast.
fn treatment_update(phase: usize) -> [u32; 17] {
    let mut update = [
        0x2caf, 0x7fff, 0, 0x097d, 0x0ff6, 0x61a8, 0, 0, 0, 0, 0x01f4, 0, 0, 0, 1, 0x1b58, 0x6d60,
    ];
    if let Some(values) = phase
        .checked_sub(1)
        .and_then(|index| TREATMENT_LANDING_UPDATES.get(index))
    {
        update[7..10].copy_from_slice(values);
    }
    update
}

fn landing_treatment_strength(
    impact_speed: f32,
    clean: bool,
    sketchy: bool,
    landing_type: u32,
) -> u32 {
    let severity = ((impact_speed - 1.5) / 5.0).clamp(0.0, 1.0);
    let quality = if sketchy {
        140.0
    } else if !clean && landing_type != 0 {
        70.0
    } else {
        0.0
    };
    (350.0 + severity * 650.0 + quality)
        .round()
        .clamp(1.0, 1_000.0) as u32
}

/// Temporary direct-mixer level for the shipped treatment bank while the recovered authored
/// `Class_Treatment` graph is still silent on the host output. Its input is the same 1..=1000
/// strength sent to the retail-style patch above, so clean landings remain quiet and hard or
/// sketchy landings remain clearly distinct.
fn treatment_fallback_gain(strength: u32) -> f32 {
    0.055 + strength.clamp(1, 1_000) as f32 * 0.000_145
}

fn grind_controls(physical_speed: f32) -> (u32, u32) {
    let speed = (physical_speed * 1_000.0).round().clamp(0.0, 10_000.0) as u32;
    // An admitted rail contact keeps a quiet texture at a stall. Motion speed still reaches zero,
    // so the authored patch can settle pitch without muting contact entirely.
    let level = (physical_speed * 700.0).round().clamp(650.0, 32_767.0) as u32;
    (speed, level)
}

/// Exact family-to-patch-layer switch from retail `sub_824C39E0`.
fn grind_layer(family: u32) -> u32 {
    match family {
        1 | 2 | 4 => 0,
        5 => 3,
        _ => 2,
    }
}

/// The retail shoe and wheel banks are laid out as 24 audio materials with a fixed number of
/// variants per material: footsteps 24x8, foot drag 24x7, wheel skid 24x4. Clamp custom-map IDs to
/// the final stock material instead of wrapping them into a different sounding surface.
fn material_slots<const VARIANTS: usize>(surface: usize) -> [usize; VARIANTS] {
    let first = surface.min(23) * VARIANTS;
    std::array::from_fn(|variant| first + variant)
}

fn advance_offboard_stride(distance: &mut f32, speed: f32) -> bool {
    if speed <= 0.25 {
        *distance = 0.0;
        return false;
    }
    *distance += speed * (1.0 / 60.0);
    let stride = (0.74 + speed * 0.035).clamp(0.74, 0.92);
    if *distance < stride {
        return false;
    }
    *distance -= stride;
    true
}

fn deceleration_skid_allowed(
    rolling: bool,
    powersliding: bool,
    was_airborne: bool,
    tick: u64,
    landing_suppressed_until: u64,
    next_skid_tick: u64,
    deceleration: f32,
) -> bool {
    rolling
        && !powersliding
        && !was_airborne
        && tick >= landing_suppressed_until
        && tick >= next_skid_tick
        && deceleration > 0.35
}

/// One side of the documented fold-down (see [`FOLD_DOWN`]): `front + centre/√2 + back/√2`, all
/// down another 3 dB. The LFE is dropped.
fn fold_side(front: f32, centre: f32, back: f32) -> f32 {
    (front + FOLD_DOWN * (centre + back)) * FOLD_DOWN
}

fn downmix(native: &[f32]) -> Vec<f32> {
    let mut stereo = Vec::with_capacity(native.len() / 3);
    for frame in native.chunks_exact(usize::from(PCM_CHANNELS)) {
        // Retail Dac order is front L/R, centre, LFE, surround L/R.
        // ponytail: the clamp is the output's full scale. The console's own fold is not public
        // (the docs say so), so whether it saturates or limits there is unknown.
        // A NaN passes `clamp` untouched and reaches the device as whatever it makes of one: silence it.
        let full_scale = |s: f32| if s.is_nan() { 0.0 } else { s.clamp(-1.0, 1.0) };
        stereo.push(full_scale(fold_side(frame[0], frame[2], frame[4])));
        stereo.push(full_scale(fold_side(frame[1], frame[2], frame[5])));
    }
    stereo
}

/// A private copy of the worker for the replay export (review I2): the same boot, frame and block path as [`run`] /
/// [`run_game_side`] (the game side when it is on, else the port), driven by the caller one record and one block at
/// a time instead of by the device, so a frame-stepped export gets exactly the mix the player hears.
pub(crate) struct OfflineRender {
    runtime: AuthoredRuntime,
    path: OfflinePath,
}
enum OfflinePath {
    Side(Box<game_side::GameSide>),
    Ported(Box<sound::PlayerSound>),
}
/// Output rate and stereo frames per [`OfflineRender::block`].
pub(crate) const OFFLINE_RATE: u32 = SAMPLE_RATE;
pub(crate) const OFFLINE_BLOCK_FRAMES: usize = PCM_FRAMES_PER_BLOCK as usize;

impl OfflineRender {
    /// `cancelled` is asked at each load point: the export that wanted this one has ended.
    pub(crate) fn new(assets: &std::path::Path, cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
        // Not retail, by request (owner 10-18): booted as the live worker boots (`run`), so the export's sound fails
        // only where the game's own does.
        first_boot(game_side::enabled(), cancelled, || Self::game_side(assets, cancelled), || {
            let Prepared { runtime, sound, .. } = prepare(assets)?;
            Ok(Self { runtime, path: OfflinePath::Ported(Box::new(sound)) })
        })
    }
    fn game_side(assets: &std::path::Path, cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
            .map_err(|error| error.to_string())?;
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).map_err(|error| error.to_string())?;
        if cancelled() {
            return Err(CANCELLED.into());
        }
        let side = game_side::GameSide::build_cancellable(&mut runtime, assets, cancelled)?;
        Ok(Self { runtime, path: OfflinePath::Side(Box::new(side)) })
    }
    /// One 60 Hz audio record, as the worker takes it from the game.
    pub(crate) fn frame(&mut self, observation: &PlayerAudioObservation) -> Result<(), String> {
        match &mut self.path {
            OfflinePath::Side(side) => side.frame(&mut self.runtime, observation, false),
            OfflinePath::Ported(sound) => sound.frame(&mut self.runtime, observation),
        }
    }
    /// One rendered block, folded to interleaved stereo at [`OFFLINE_RATE`].
    pub(crate) fn block(&mut self) -> Result<Vec<f32>, String> {
        Ok(downmix(&self.runtime.pump_once().map_err(|error| error.to_string())?))
    }
}

/// An [`OfflineRender`] boot stopped because its export ended.
const CANCELLED: &str = "cancelled";

/// The live worker's boot order (`run`): the game side, three boots, then the ported path; `cancelled` stops it
/// between them.
fn first_boot<T>(
    game_side: bool,
    cancelled: &dyn Fn() -> bool,
    mut side: impl FnMut() -> Result<T, String>,
    ported: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if game_side {
        for boot in 1..=3 {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            match side() {
                Ok(done) => return Ok(done),
                Err(error) if error == CANCELLED => return Err(error),
                Err(error) => log_audio_error(&format!("export game_side_failed boot={boot} error={error}")),
            }
        }
        log_audio_error("export: game side failed three boots; falling back to the ported player audio");
    }
    if cancelled() {
        return Err(CANCELLED.into());
    }
    ported()
}

/// Fold decoded resident bank material into the same stereo convention as the recovered retail
/// output path.  The old direct fallback averaged all channels and copied the result to both
/// speakers, which discarded the treatment bank's four-channel separation and made every
/// stereo board/cloth source sound centred and thinner than retail.
fn direct_stereo_frame(source: &PcmSource, frame: usize) -> (f32, f32) {
    let sample = |channel| source.sample(frame, channel).unwrap_or(0.0);
    match source.channels() {
        0 => (0.0, 0.0),
        1 => {
            let mono = sample(0);
            (mono, mono)
        }
        2 => (sample(0), sample(1)),
        3 => (sample(0) + 0.707 * sample(2), sample(1) + 0.707 * sample(2)),
        // EAAC/XMA four-channel material is front L/R followed by surround L/R.  In particular,
        // the traced normal Treatments voice is four-channel; preserve its rear impact layers.
        4 => (sample(0) + 0.5 * sample(2), sample(1) + 0.5 * sample(3)),
        _ => (
            sample(0) + 0.707 * sample(2) + 0.5 * sample(4),
            sample(1) + 0.707 * sample(2) + 0.5 * sample(5),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("skate-audio-errors-{name}.log"));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Owner 10-18: the export boots as the live worker does (three game-side boots, then the ported path), and an
    /// ended export's boot stops between them.
    #[test]
    fn export_boot_falls_back_to_the_ported_path() {
        let no = || false;
        let mut tries = 0;
        let got = first_boot(true, &no, || { tries += 1; Err::<&str, String>("guest heap".into()) }, || Ok("ported"));
        assert_eq!((got, tries), (Ok("ported"), 3));
        assert_eq!(first_boot(true, &no, || Ok("side"), || Ok("ported")), Ok("side"));
        assert_eq!(first_boot(false, &no, || Ok("side"), || Ok("ported")), Ok("ported"));
        let mut tries = 0;
        let got = first_boot(true, &|| true, || { tries += 1; Ok("side") }, || Ok("ported"));
        assert_eq!((got, tries), (Err(CANCELLED.into()), 0));
        let got = first_boot(true, &no, || { tries += 1; Err::<&str, String>(CANCELLED.into()) }, || Ok("ported"));
        assert_eq!((got, tries), (Err(CANCELLED.into()), 1), "a boot cancelled inside is not tried again");
    }

    #[test]
    fn a_repeating_audio_error_is_written_once_then_at_each_power_of_ten() {
        let path = temp_log("repeat");
        let mut log = ErrorLog::open(&path);
        for _ in 0..1500 {
            log.note("game_side_block_error error=boom");
        }
        log.note("game_side_failed boot=1");
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().map(|l| l.split_once(' ').unwrap().1).collect();
        assert_eq!(
            lines,
            [
                "game_side_block_error error=boom",
                "game_side_block_error error=boom (x10)",
                "game_side_block_error error=boom (x100)",
                "game_side_block_error error=boom (x1000)",
                "game_side_failed boot=1",
            ]
        );
    }

    #[test]
    fn an_oversized_audio_error_log_keeps_its_newest_whole_lines() {
        let path = temp_log("trim");
        let mut text = String::new();
        let mut n = 0u32;
        while text.len() < 2 << 20 {
            text.push_str(&format!("1760000000 game_side_frame_error error=line {n}\n"));
            n += 1;
        }
        std::fs::write(&path, &text).unwrap();
        ErrorLog::open(&path).note("fresh");
        let kept = std::fs::read_to_string(&path).unwrap();
        assert!(kept.len() as u64 <= ERROR_LOG_KEEP + 64, "{} bytes kept", kept.len());
        assert!(kept.starts_with("1760000000 game_side_frame_error"), "cut mid-line");
        let last = format!("error=line {}\n", n - 1);
        assert!(kept.contains(&last), "newest old line lost");
        assert!(kept.ends_with(" fresh\n"));
    }

    // Expected words below are copied from the recorded retail session traces (sk8Audio
    // probe/traces/sessions/play1 and play4), not recomputed from these helpers.

    #[test]
    fn wheel_constructors_match_the_recorded_retail_posts() {
        // play1 msg 3: Class_rolling payload 40C884E4, words 0..11 (later words are heap).
        assert_eq!(
            rolling_constructor(0, 0, 3),
            [0, 0, 0x1000, 0, 0, 0, 3, 0, 0, 0x61A8, 0, 0x7FFF]
        );
        // play4 msg 16: Rolling_Rattle_Class payload 40C88564, words 0..11.
        assert_eq!(
            rattle_constructor(0, 2, 8),
            [0, 0, 0x1000, 0, 2, 0, 1, 0, 0x61A8, 0, 0x7FFF, 8]
        );
    }

    #[test]
    fn footstep_constructor_matches_the_recorded_retail_post() {
        // play1: every playercharacter_footstep post (payloads 40C93AA4, 40C936A4, ...).
        assert_eq!(
            footstep_constructor(),
            [
                0, 0, 0x1000, 0, 0x61A8, 0, 0, 0x7FFF, 0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0,
                0, 0x0C
            ]
        );
    }

    #[test]
    fn rolling_update_reproduces_recorded_retail_frames() {
        // play1 node 40C02F10 rows: (speed word, gain word) pairs from the ramp-in region.
        for (speed, gain) in [(59, 1666), (125, 3542), (182, 5150)] {
            let computed = rolling_gain_word(speed, 12_999);
            assert!(
                computed.abs_diff(gain) <= 20,
                "speed {speed}: {computed} vs recorded {gain}"
            );
        }
        assert_eq!(rolling_gain_word(4_000, 12_999), 12_999);
        assert_eq!(rolling_gain_word(4_000, 4_913), 4_913);
        let update = rolling_update(2_999, 0, DEFAULT_ROLLING_SURFACE, 12_999);
        assert_eq!(update[0], 32_767);
        assert_eq!((update[2], update[9], update[10]), (4_086, 24_971, 77));
        assert_eq!(
            (update[3], update[4], update[6], update[11]),
            (2_999, 0, 2, 12_999)
        );
    }

    #[test]
    fn wheel_speed_words_use_the_vault_tuning() {
        // 70 km/h rolling maximum and 30 km/h rattle divisor; truncation as fctiwz.
        assert_eq!(rolling_speed_word(0.0), 0);
        assert_eq!(rolling_speed_word(25.0), 10_000);
        assert!(rolling_speed_word(35.0 / 3.6).abs_diff(5_000) <= 1);
        assert_eq!(rattle_speed_word(1.0), 0);
        assert_eq!(rattle_speed_word(0.5), 0);
        assert_eq!(rattle_speed_word(20.0), 10_000);
        assert!(rattle_speed_word(1.0 + 15.0 / 3.6).abs_diff(5_000) <= 1);
    }

    #[test]
    fn footstep_update_carries_the_gain_tuning_the_patch_multiplies() {
        let idle = footstep_update(false, 0, 0, 0);
        assert_eq!((idle[8], idle[9], idle[13]), (0, 0, 1));
        assert_eq!(idle[23], 28_000, "word 23 zero would silence every step");
        assert_eq!(idle[24], 12, "voice property 9 reads word 24");
        let plant = footstep_update(true, 513, 297, 3);
        assert_eq!(
            (plant[8], plant[9], plant[10], plant[13], plant[14]),
            (1, 2, 513, 3, 297)
        );
    }

    #[test]
    fn queued_contact_and_landing_survive_a_following_idle_tick() {
        let (sender, receiver) = mpsc::sync_channel(16);
        // One presentation frame can forward multiple completed physics ticks together.
        sender.send((10, "foot contact")).unwrap();
        sender.send((11, "landing")).unwrap();
        sender.send((12, "idle")).unwrap();
        assert_eq!(
            next_observation(&receiver).unwrap(),
            Some((10, "foot contact"))
        );
        assert_eq!(next_observation(&receiver).unwrap(), Some((11, "landing")));
        assert_eq!(next_observation(&receiver).unwrap(), Some((12, "idle")));
        assert_eq!(next_observation(&receiver).unwrap(), None);
    }

    #[test]
    fn grind_families_select_the_retail_patch_layers() {
        assert_eq!(grind_layer(0), 2);
        assert_eq!(grind_layer(1), 0);
        assert_eq!(grind_layer(2), 0);
        assert_eq!(grind_layer(3), 2);
        assert_eq!(grind_layer(4), 0);
        assert_eq!(grind_layer(5), 3);
    }

    #[test]
    fn material_banks_select_only_the_current_retail_surface_group() {
        assert_eq!(material_slots::<8>(0), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(material_slots::<4>(14), [56, 57, 58, 59]);
        assert_eq!(material_slots::<7>(23), [161, 162, 163, 164, 165, 166, 167]);
        assert_eq!(material_slots::<4>(127), [92, 93, 94, 95]);
    }

    #[test]
    fn landing_speed_loss_does_not_post_a_wheel_skid() {
        assert!(!deceleration_skid_allowed(
            true, false, true, 100, 112, 0, 3.0
        ));
        assert!(!deceleration_skid_allowed(
            true, false, false, 108, 112, 0, 3.0
        ));
        assert!(deceleration_skid_allowed(
            true, false, false, 112, 112, 0, 0.5
        ));
    }

    #[test]
    fn hard_landings_drive_a_stronger_treatment_payload() {
        assert_eq!(landing_treatment_strength(0.5, true, false, 0), 350);
        assert!(landing_treatment_strength(4.0, true, false, 0) > 500);
        assert_eq!(landing_treatment_strength(8.0, true, false, 0), 1_000);
        assert!(
            landing_treatment_strength(3.0, false, true, 1)
                > landing_treatment_strength(3.0, true, false, 0)
        );
    }

    #[test]
    fn direct_treatment_fallback_preserves_landing_severity() {
        let clean = landing_treatment_strength(0.5, true, false, 0);
        let hard = landing_treatment_strength(8.0, true, false, 0);
        let sketchy = landing_treatment_strength(3.0, false, true, 1);
        assert!(treatment_fallback_gain(hard) > treatment_fallback_gain(clean));
        assert!(treatment_fallback_gain(sketchy) > treatment_fallback_gain(clean));
        assert!(treatment_fallback_gain(hard) <= 0.20);
    }

    #[test]
    fn direct_bank_downmix_preserves_the_treatment_rear_layers() {
        let source =
            PcmSource::new(std::sync::Arc::from([10_000i16, -8_000, 6_000, -4_000]), 4).unwrap();
        let (left, right) = direct_stereo_frame(&source, 0);
        assert!(left > 0.39, "left keeps the front and rear-left layers");
        assert!(right < -0.30, "right keeps the front and rear-right layers");
        assert_ne!(left, right, "four-channel source must not collapse to mono");
    }

    #[test]
    fn admitted_grind_retains_quiet_contact_at_a_stall() {
        assert_eq!(grind_controls(0.0), (0, 650));
        assert_eq!(grind_controls(2.0), (2_000, 1_400));
    }

    #[test]
    fn offboard_stride_continues_without_animation_contact_markers() {
        let mut distance = 0.0;
        let steps = (0..120)
            .filter(|_| advance_offboard_stride(&mut distance, 2.0))
            .count();
        assert!((4..=5).contains(&steps));
        assert!(!advance_offboard_stride(&mut distance, 0.0));
        assert_eq!(distance, 0.0);
    }

    #[test]
    fn downmix_keeps_frames_and_separates_sides() {
        let input = [0.000_005, 0.000_01, 0.000_02, 100.0, 0.000_04, 0.000_08];
        let output = downmix(&input);
        assert_eq!(output.len(), 2);
        assert!(output[1] > output[0]);
        assert!(
            output.iter().all(|sample| *sample < 1.0),
            "LFE must not enter full-range output"
        );
    }

    /// RwAudioCore's documented fold (image 0x820F0528): fronts at -3 dB overall, centre and
    /// backs a further -3 dB (-6 dB in all), LFE dropped, and the result held to full scale.
    #[test]
    fn downmix_is_rwaudiocores_documented_fold_down() {
        let close = |a: f32, b: f32| (a - b).abs() < 1.0e-6;
        let one = |channel: usize| {
            let mut frame = [0.0f32; 6];
            frame[channel] = 0.5;
            downmix(&frame)
        };
        let half_minus_3db = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        let [l, r]: [f32; 2] = one(0).try_into().unwrap();
        assert!(close(l, half_minus_3db) && r == 0.0, "front left {l} {r}");
        let [l, r]: [f32; 2] = one(2).try_into().unwrap();
        assert!(close(l, 0.25) && close(r, 0.25), "centre {l} {r}");
        let [l, r]: [f32; 2] = one(3).try_into().unwrap();
        assert!(l == 0.0 && r == 0.0, "LFE {l} {r}");
        let [l, r]: [f32; 2] = one(5).try_into().unwrap();
        assert!(l == 0.0 && close(r, 0.25), "back right {l} {r}");
        // Full scale on front, centre and back together folds past unity and is held there.
        assert_eq!(downmix(&[1.0, 0.0, 1.0, 1.0, 1.0, -1.0]), vec![1.0, 0.0]);
        assert_eq!(downmix(&[f32::NAN, f32::INFINITY, 0.0, 0.0, 0.0, f32::NEG_INFINITY]), vec![0.0, 0.0]);
    }

    /// The worker's pacing ([`block_wait`]) against the real stream: the `LivePcm` queue, the
    /// decoder the mixer pulls from it, and the observation channel and [`next_observation`].
    /// Simulated clock, so the run is exact and instant. The cadence is an unfocused window's:
    /// the device takes 480 frames every 10 ms, the game sends an observation every 16.7 ms,
    /// every sleep the worker asks for lasts whole 15.625 ms timer ticks, a block costs 2.5 ms
    /// to render plus 1-4.1 ms when it carries a game-side frame (as measured headless: the
    /// worker needs ~65% of a core in real time). Then a 350 ms worker stall (the
    /// thread descheduled) and a 400 ms game stall (a burst of 15 catch-up ticks after it).
    /// Outside the stalls and a short refill after each, the device must never run dry.
    #[test]
    fn unfocused_cadence_never_runs_the_device_dry_outside_a_stall() {
        use bevy::audio::Decodable;
        const TICK: u64 = 15_625; // the default Windows timer period, in us
        let live = LivePcm::new(OUTPUT_CHANNELS, SAMPLE_RATE, QUEUE_FRAMES).unwrap();
        let mut device = live.decoder();
        let (input, observations) = mpsc::sync_channel::<u64>(OBSERVATION_QUEUE);
        let block = vec![0.25f32; usize::from(OUTPUT_CHANNELS) * PCM_FRAMES_PER_BLOCK as usize];
        let worker_stall = 4_000_000..4_350_000u64;
        let game_stall = 7_000_000..7_400_000u64;
        // Underflow is only excused while a stall has the stream empty and for 100 ms of refill.
        let excused = |t: u64| {
            (worker_stall.start..worker_stall.end + 100_000).contains(&t)
                || (game_stall.start..game_stall.end + 100_000).contains(&t)
        };
        let (mut t_device, mut t_game, mut t_worker) = (10_000u64, 16_667u64, 0u64);
        let mut rendering = false; // the worker is between taking an observation and its push
        let mut rng = 0x2545_F491_4F6C_DD1Du64;
        let (mut tick, mut dropped, mut late_underflow, mut lowest) = (0u64, 0u32, 0u64, usize::MAX);
        let after_warmup = 500_000u64;
        while t_device.min(t_game).min(t_worker) < 12_000_000 {
            if t_device <= t_game && t_device <= t_worker {
                let before = live.snapshot().underflow;
                for _ in 0..480 * usize::from(OUTPUT_CHANNELS) {
                    device.next().expect("LivePcm never ends");
                }
                let underflow = live.snapshot().underflow - before;
                if t_device >= after_warmup && !excused(t_device) {
                    late_underflow += underflow / u64::from(OUTPUT_CHANNELS);
                    lowest = lowest.min(live.queued_frames());
                }
                t_device += 10_000;
            } else if t_game <= t_worker {
                let catch_up = if game_stall.contains(&t_game) {
                    t_game = game_stall.end;
                    15 // Time<Virtual>'s 250 ms max delta, in 60 Hz ticks
                } else {
                    1
                };
                for _ in 0..catch_up {
                    tick += 1;
                    if input.try_send(tick).is_err() && !excused(t_game) {
                        dropped += 1;
                    }
                }
                t_game += 16_667;
            } else if !rendering {
                let frame = next_observation(&observations).expect("the game never disconnects");
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let cost = 2_500 + if frame.is_some() { 1_000 + rng % 3_100 } else { 0 };
                t_worker += cost;
                if worker_stall.contains(&t_worker) {
                    t_worker = worker_stall.end;
                }
                rendering = true;
            } else {
                live.push(&block).unwrap();
                let wait = block_wait(live.queued_frames()).as_micros() as u64;
                t_worker += wait.div_ceil(TICK) * TICK;
                if worker_stall.contains(&t_worker) {
                    t_worker = worker_stall.end;
                }
                rendering = false;
            }
        }
        let stats = live.snapshot();
        eprintln!(
            "underflow outside the stalls {late_underflow} frames, lowest queue {lowest} frames, \
             dropped observations {dropped}, pushed {} consumed {} underflow {}",
            stats.pushed, stats.consumed, stats.underflow
        );
        assert_eq!(late_underflow, 0, "the device ran dry outside a stall (static)");
        assert_eq!(dropped, 0, "the worker fell behind the game's observations");
        assert!(stats.consumed_nonzero * 10 > stats.consumed * 9, "the stream went quiet");
    }

    /// The mixer's first pull through [`PlayerStream`] leaves its thread time-critical (what
    /// cpal meant to do and did not), and the samples pass through untouched.
    #[cfg(windows)]
    #[test]
    fn the_player_stream_raises_the_mixer_thread() {
        use bevy::audio::Decodable;
        let live = LivePcm::new(OUTPUT_CHANNELS, SAMPLE_RATE, QUEUE_FRAMES).unwrap();
        live.push(&[0.5, -0.5]).unwrap();
        let mut mixer = PlayerStream(live).decoder();
        let (samples, priority) = thread::spawn(move || {
            let samples = [mixer.next(), mixer.next()];
            (samples, skate_voice::priority::current())
        })
        .join()
        .unwrap();
        assert_eq!(samples, [Some(0.5), Some(-0.5)]);
        assert_eq!(priority, skate_voice::priority::TIME_CRITICAL);
    }
}
