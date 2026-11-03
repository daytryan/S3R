//! Online voice chat (user 10-26): the voice thread (crates/skate-voice) wired to the game. The engine runs in a
//! session with Voice Chat on, and for the Mic Test (offline too); the microphone opens off the main thread (opening
//! waits on the device). Each frame the game hands the thread the route, whose voices it may play and how loud, the
//! settings and Push to Talk, and reads back who is talking, whether we send, our mic level and whether a voice is heard
//! (Lower Game Sound While Talking). The two-copy loopback test drives it through [`Hooks`].
use crate::skate_audio::LivePcm;
use bevy::prelude::*;
use skate_net::roster::Member;
use skate_voice::{
    device::{Capture, open_with_fallback},
    engine::{self, Handle, Io, Shared},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::Ordering::Relaxed,
        mpsc::{self, Receiver, TryRecvError},
    },
};

/// The voice output holds 100 ms of mono 48 kHz (the engine keeps 30 ms ahead).
const OUTPUT_FRAMES: usize = 4_800;
/// A missing or failed microphone is looked for again this often, off the main thread.
const RETRY_SECS: f32 = 2.;

pub(crate) struct VoicePlugin;
impl Plugin for VoicePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VoiceState>()
            .insert_resource(Voice { hooks: Hooks::from_env(), ..default() })
            .add_systems(Startup, start_output)
            .add_systems(
                PreUpdate,
                push_to_talk.after(crate::online_chat::keys).after(crate::customiser::navigation).after(crate::input::poll_controllers),
            )
            .add_systems(Update, run.after(crate::player_list::guard));
    }
}

/// Who is talking and our own mic, for the talking icons (online_hud, the Player List).
#[derive(Resource, Default, PartialEq)]
pub(crate) struct VoiceState {
    pub talking: BTreeSet<u64>,
    /// Everyone heard this session or whose `@voice` is on: the players with voice.
    pub heard: BTreeSet<u64>,
    pub sending: bool,
    pub mic: Option<String>,
    pub level_dbfs: f32,
}

/// The voice thread runs in a session with Voice Chat on, and for the Mic Test (offline too).
pub(crate) fn engine_wanted(lobby: bool, voice_on: bool, mic_test: bool) -> bool {
    lobby && voice_on || mic_test
}

/// Push to Talk: the key (or a mouse side button) while the window is focused and nothing else takes typing (the chat
/// bar, a menu, an editor: online_chat::Overlays); the pad's button while focused, menus included.
pub(crate) fn ptt_held(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    key: &str,
    pad_held: bool,
    focused: bool,
    typing: bool,
) -> bool {
    let key_held = match key {
        "Mouse4" => mouse.pressed(MouseButton::Back),
        "Mouse5" => mouse.pressed(MouseButton::Forward),
        key => keys.get_pressed().any(|k| format!("{k:?}") == key),
    };
    focused && (pad_held || key_held && !typing)
}

/// The voice status retail's movies ask for (FELeaderboard.GetVoipStatus, Skate 1's voip field), one enum in all three
/// games (.superpowers/sdd/2026-10-26-voice-chat/voip-status-trace.md): 0 no voice (hidden; Skate 1's `off`), 1 has
/// voice (`micoff`; `on`), 2 talking (`micon`; `active`), 3 muted (hidden; `muted`). Muted wins: a mute or block shows
/// whether or not they were ever heard.
pub(crate) fn voip_status(talking: bool, muted: bool, has_voice: bool) -> i64 {
    match (muted, has_voice, talking) {
        (true, ..) => 3,
        (false, false, _) => 0,
        (false, true, true) => 2,
        (false, true, false) => 1,
    }
}

/// Each session player's [`voip_status`] this frame, for the boards, the name tags and the Player List ([`run`] sets it
/// each frame, as player_list publishes its verdicts).
#[derive(Default)]
pub(crate) struct Voip {
    by_actor: BTreeMap<u64, i64>,
    /// The actor a name a board shows stands for: the roster's shown name and the player's own (a match's boards show
    /// that); None where two players share one.
    by_name: BTreeMap<String, Option<u64>>,
}
impl Voip {
    /// Us: talking while our gate is open (alone too), with voice while our microphone is open. The others: talking
    /// while their voice plays, with voice once heard this session, muted while muted or blocked (`quiet`).
    pub fn new(members: &[Member], local: u64, state: &VoiceState, quiet: impl Fn(u64) -> bool) -> Self {
        let mut voip = Self::default();
        for (m, shown) in members.iter().zip(crate::player_list::shown_names(members)) {
            let status = match m.actor == local {
                true => voip_status(state.sending, false, state.mic.is_some()),
                false => voip_status(state.talking.contains(&m.actor), quiet(m.actor), state.heard.contains(&m.actor)),
            };
            voip.by_actor.insert(m.actor, status);
            for name in [shown, m.name.clone()] {
                let actor = voip.by_name.entry(name).or_insert(Some(m.actor));
                if *actor != Some(m.actor) {
                    *actor = None;
                }
            }
        }
        voip
    }
    pub fn of(&self, actor: u64) -> i64 {
        self.by_actor.get(&actor).copied().unwrap_or(0)
    }
    pub fn named(&self, name: &str) -> i64 {
        self.by_name.get(name).copied().flatten().map_or(0, |actor| self.of(actor))
    }
}
static VOIP: Mutex<Voip> = Mutex::new(Voip { by_actor: BTreeMap::new(), by_name: BTreeMap::new() });
/// `actor`'s [`voip_status`] this frame (0 outside a session).
pub(crate) fn voip(actor: u64) -> i64 {
    lock(&VOIP).of(actor)
}
/// The [`voip_status`] of the player a board shows as `name` (0 for a name two players share).
pub(crate) fn voip_named(name: &str) -> i64 {
    lock(&VOIP).named(name)
}
/// Talking this frame: a lit mic by their name.
pub(crate) fn talking(actor: u64) -> bool {
    voip(actor) == 2
}
/// Tests: the player shown as `name` (actor `actor`) has voice status `status`, beside what other tests set (each
/// uses names and actors of its own).
#[cfg(test)]
pub(crate) fn hear(name: &str, actor: u64, status: i64) {
    let mut voip = lock(&VOIP);
    voip.by_actor.insert(actor, status);
    voip.by_name.insert(name.to_owned(), Some(actor));
}

/// The two-copy loopback test's hooks (voice plan Task 12), read once at start: `SKATE_VOICE_CLIP=<wav>` stands in for
/// the microphone (48 kHz mono 16-bit, on a loop, Push to Talk held), `SKATE_VOICE_DUMP=<wav>` gets everything the
/// engine plays too, `SKATE_VOICE_MUTE_BOT=1` hears nobody named Bot. With any of them, talking and the capture's time a
/// frame go to the log. Only a `voice-test` build reads them (user 10-27): the game itself never records a voice.
#[derive(Default)]
struct Hooks {
    clip: Option<Arc<[f32]>>,
    dump: Option<PathBuf>,
    mute_bot: bool,
}
impl Hooks {
    fn from_env() -> Self {
        if !cfg!(feature = "voice-test") {
            return Self::default();
        }
        let clip = std::env::var_os("SKATE_VOICE_CLIP").and_then(|path| match wav_read(Path::new(&path)) {
            Ok(clip) => Some(clip.into()),
            Err(error) => {
                error!("SKATE_VOICE_CLIP: {error}");
                None
            }
        });
        let dump = std::env::var_os("SKATE_VOICE_DUMP").map(PathBuf::from);
        Self { clip, dump, mute_bot: std::env::var_os("SKATE_VOICE_MUTE_BOT").is_some() }
    }
    fn on(&self) -> bool {
        self.clip.is_some() || self.dump.is_some() || self.mute_bot
    }
}

/// The loopback test's hooks: a 48 kHz mono 16-bit PCM wav's samples (the clip).
fn wav_read(path: &Path) -> Result<Vec<f32>, String> {
    let file = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = file.as_slice();
    let u16_at = |b: &[u8], at: usize| b.get(at..at + 2).map(|v| u16::from_le_bytes([v[0], v[1]]));
    let u32_at = |b: &[u8], at: usize| b.get(at..at + 4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
    if bytes.get(..4) != Some(b"RIFF".as_slice()) || bytes.get(8..12) != Some(b"WAVE".as_slice()) {
        return Err(format!("{}: not a wav", path.display()));
    }
    let (mut at, mut format, mut data) = (12, None, None);
    while let Some(size) = u32_at(bytes, at + 4).map(|size| size as usize) {
        let body = &bytes[(at + 8).min(bytes.len())..(at + 8 + size).min(bytes.len())];
        match &bytes[at..at + 4] {
            b"fmt " => format = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    // PCM (1), one channel, 48000 Hz, 16 bits.
    let format = format.ok_or_else(|| format!("{}: no fmt chunk", path.display()))?;
    if (u16_at(format, 0), u16_at(format, 2), u32_at(format, 4), u16_at(format, 14)) != (Some(1), Some(1), Some(48_000), Some(16)) {
        return Err(format!("{}: not 48 kHz mono 16-bit PCM", path.display()));
    }
    let data = data.ok_or_else(|| format!("{}: no data chunk", path.display()))?;
    let samples: Vec<f32> = data.chunks_exact(2).map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.).collect();
    if samples.is_empty() {
        return Err(format!("{}: no samples", path.display()));
    }
    Ok(samples)
}
/// A 48 kHz mono 16-bit PCM wav's 44-byte header over `data` bytes of samples.
fn wav_header(data: u32) -> Vec<u8> {
    let parts: [&[u8]; 12] = [
        b"RIFF",
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &48_000u32.to_le_bytes(),
        &96_000u32.to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ];
    parts.concat()
}
/// The loopback test's hooks: what the engine plays, written as a 48 kHz mono 16-bit wav whose sizes stay right as it
/// grows (the copies are closed, not quit): the file and its sample bytes so far.
struct Dump(std::fs::File, u32);
impl Dump {
    fn create(path: &Path) -> std::io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&wav_header(0))?;
        Ok(Self(file, 0))
    }
    fn write(&mut self, pcm: &[f32]) -> std::io::Result<()> {
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| ((s.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes()).collect();
        self.0.seek(SeekFrom::End(0))?;
        self.0.write_all(&bytes)?;
        self.1 += bytes.len() as u32;
        self.0.seek(SeekFrom::Start(0))?;
        self.0.write_all(&wav_header(self.1))
    }
}
/// The loopback test's hooks: the clip as a microphone, 10 ms frames on a loop paced in real time, until the receiver
/// goes.
fn clip_feed(clip: Arc<[f32]>) -> Receiver<Vec<f32>> {
    use skate_voice::dsp::FRAME;
    let (frames, received) = mpsc::sync_channel(16);
    let feed = move || {
        // Raised as the microphone's callback is (skate_voice::priority).
        let _ = skate_voice::priority::raise();
        let (start, mut at) = (Instant::now(), 0);
        for n in 1u64.. {
            let frame: Vec<f32> = (0..FRAME).map(|i| clip[(at + i) % clip.len()]).collect();
            at = (at + FRAME) % clip.len();
            if frames.send(frame).is_err() {
                return;
            }
            if let Some(wait) = (start + Duration::from_millis(10 * n)).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    };
    if let Err(e) = std::thread::Builder::new().name("voice clip".into()).spawn(feed) {
        error!("Voice chat: no thread for the clip: {e}");
    }
    received
}

/// The engine and what it runs on.
#[derive(Resource)]
struct Voice {
    shared: Arc<Shared>,
    /// The voice output (played as player_audio plays the skater's sounds).
    output: Option<LivePcm>,
    /// The voice thread and its microphone (None: listen only).
    running: Option<(Handle, Option<Capture>)>,
    /// What the running (or starting) engine is for: the session, if any, and Settings' Microphone.
    started: Option<(Option<(u64, u64)>, Option<String>)>,
    /// A microphone opening off the main thread (no Capture: the loopback test's clip).
    opening: Option<Mutex<Receiver<Result<(Option<Capture>, Receiver<Vec<f32>>), String>>>>,
    /// Why the last look found no microphone (audit 10-30: it failed silently), for Settings' Microphone row.
    mic_error: Option<String>,
    /// When a missing or failed microphone is next looked for.
    retry: f32,
    /// [`VoiceState::heard`], and the session (and our actor) it was heard in.
    heard: (Option<(u64, u64)>, BTreeSet<u64>),
    hooks: Hooks,
    /// When the hooks last logged the capture's time a frame.
    logged: f32,
}
impl Default for Voice {
    fn default() -> Self {
        let config = engine::Config {
            enabled: false,
            mode: skate_voice::dsp::Mode::PushToTalk,
            noise: skate_voice::dsp::Noise::Standard,
            mic_volume: 100,
            voice_volume: 100,
            mic_test: false,
        };
        let (heard, hooks) = ((None, BTreeSet::new()), Hooks::default());
        Self { shared: Arc::new(Shared::new(config)), output: None, running: None, started: None, opening: None, mic_error: None, retry: 0., heard, hooks, logged: 0. }
    }
}
impl Voice {
    /// Stops the thread and the microphone; what they published is cleared.
    fn stop(&mut self) {
        (self.running, self.opening, self.started) = (None, None, None);
        self.shared.sending.store(false, Relaxed);
        self.shared.audible.store(false, Relaxed);
        lock(&self.shared.talking).clear();
    }
    /// Opens Settings' microphone (or Windows' default) off the main thread: Capture::open waits on the device.
    fn open(&mut self, wanted: Option<String>, now: f32) {
        self.retry = now + RETRY_SECS;
        let (found, opened) = mpsc::sync_channel(1);
        if let Some(clip) = self.hooks.clip.clone() {
            // The loopback test: the clip stands in for the microphone.
            let _ = found.send(Ok((None, clip_feed(clip))));
            self.opening = Some(Mutex::new(opened));
            return;
        }
        let thread = std::thread::Builder::new().name("voice mic".into()).spawn(move || {
            let _ = found.send(open_with_fallback(wanted.as_deref(), Capture::open).map(|(capture, mic)| (Some(capture), mic)));
        });
        match thread {
            Ok(_) => self.opening = Some(Mutex::new(opened)),
            Err(e) => error!("Voice chat: no thread to open the microphone: {e}"),
        }
    }
    /// What the open thread found, once it is done (`Some(Err)`: no microphone, listen only, and why).
    fn take_opened(&mut self) -> Option<Result<(Option<Capture>, Receiver<Vec<f32>>), String>> {
        let opened = match self.opening.as_mut()?.get_mut().unwrap_or_else(PoisonError::into_inner).try_recv() {
            Ok(opened) => opened,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the microphone thread stopped".into()),
        };
        self.opening = None;
        let error = opened.as_ref().err().cloned();
        if error.is_some() && error != self.mic_error {
            warn!("Voice chat: no microphone: {}", error.as_deref().unwrap_or_default());
        }
        self.mic_error = error;
        Some(opened)
    }
}

/// A poisoned lock still holds a whole value (each is copied in or out at once), as on the voice thread.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the engine's datagrams and sound go: the session's transport (nowhere offline) and the voice output.
struct Output {
    send: Box<dyn FnMut(u64, &[u8]) + Send>,
    live: Option<LivePcm>,
    /// The loopback test's SKATE_VOICE_DUMP.
    dump: Option<Dump>,
}
impl Io for Output {
    fn send(&mut self, endpoint: u64, datagram: &[u8]) {
        (self.send)(endpoint, datagram);
    }
    fn output(&mut self, pcm: &[f32]) {
        if let Some(live) = &self.live {
            let _ = live.push(pcm);
        }
        if let Some(dump) = &mut self.dump
            && let Err(error) = dump.write(pcm)
        {
            error!("SKATE_VOICE_DUMP: {error}");
            self.dump = None;
        }
    }
    fn queued(&self) -> usize {
        self.live.as_ref().map_or(0, LivePcm::queued_frames)
    }
}

/// The voice output: player_audio's stream type, so its decoder raises the mixer thread as the skater's sounds do.
fn start_output(
    mut commands: Commands,
    mut assets: ResMut<Assets<crate::skate_audio::player::PlayerStream>>,
    mut voice: ResMut<Voice>,
) {
    let live = match LivePcm::new(1, 48_000, OUTPUT_FRAMES) {
        Ok(live) => live,
        Err(error) => {
            error!("Voice chat output: {error}");
            return;
        }
    };
    let handle = assets.add(crate::skate_audio::player::PlayerStream(live.clone()));
    // LivePcm never ends; Loop mode would wrap it in Rodio's buffering repeater (as player_audio's start).
    commands.spawn((AudioPlayer(handle), PlaybackSettings::ONCE));
    voice.output = Some(live);
}

/// Push to Talk, each frame after the chat bar took its keys and the pads were read.
fn push_to_talk(
    voice: Res<Voice>,
    (keys, mouse): (Res<ButtonInput<KeyCode>>, Res<ButtonInput<MouseButton>>),
    menu: Res<crate::graphics_menu::Menu>,
    chat: Option<Res<crate::online_chat::Chat>>,
    overlays: crate::online_chat::Overlays,
    windows: Query<&Window>,
) {
    let focused = windows.iter().any(|w| w.focused);
    let typing = chat.is_some_and(|c| c.bar.is_some()) || overlays.open();
    // Taken each frame: the pads polled since (the menus' active pad always, every pad in play) set it again.
    let pad = crate::input::platform::PTT_PAD_HELD.swap(0, Relaxed) != 0;
    // The loopback test's clip talks all the time.
    let held = voice.hooks.clip.is_some() || ptt_held(&keys, &mouse, menu.ptt_key(), pad, focused, typing);
    voice.shared.ptt.store(held, Relaxed);
}

/// Each frame: start, restart or stop the engine; hand it the session and the settings; read back what it publishes.
fn run(
    mut voice: ResMut<Voice>,
    mut state: ResMut<VoiceState>,
    mut net: ResMut<crate::multiplayer::Multiplayer>,
    mut menu: ResMut<crate::graphics_menu::Menu>,
    roster: Res<crate::online_roster::OnlineRoster>,
    time: Res<Time<Real>>,
) {
    let voice = &mut *voice;
    let config = menu.voice_config();
    let lobby = net.session().map(|s| (s.session, s.local));
    let key = (lobby, menu.microphone().map(str::to_owned));
    let now = time.elapsed_secs();
    if !engine_wanted(lobby.is_some(), config.enabled, config.mic_test) {
        if voice.started.is_some() {
            voice.stop();
        }
    } else if voice.started.as_ref() != Some(&key) {
        // A session came or went, or another microphone: start over.
        voice.stop();
        voice.open(key.1.clone(), now);
        voice.started = Some(key);
    } else if voice.opening.is_none()
        && now >= voice.retry
        && voice.hooks.clip.is_none()
        && voice.running.as_ref().is_none_or(|(_, mic)| mic.as_ref().is_none_or(Capture::failed))
    {
        // No microphone, or it went away (or the engine never started): look again; a running engine keeps running
        // until one opens.
        let wanted = menu.microphone().map(str::to_owned);
        voice.open(wanted, now);
    }
    if let Some(opened) = voice.take_opened()
        && (voice.running.is_none() || opened.is_ok())
    {
        // The old thread stops before the new one takes the output.
        voice.running = None;
        let (capture, mic) = match opened {
            Ok((capture, mic)) => (capture, mic),
            // No microphone: the engine listens only.
            Err(_) => (None, mpsc::sync_channel(0).1),
        };
        let (send, incoming): (Box<dyn FnMut(u64, &[u8]) + Send>, Receiver<(u64, Vec<u8>)>) = match net.voice_io() {
            Some(io) => io,
            // No session (the Mic Test offline): nothing sent, nothing heard.
            None => (Box::new(|_: u64, _: &[u8]| {}), mpsc::channel().1),
        };
        if let Some(live) = &voice.output {
            live.clear();
        }
        let dump = voice.hooks.dump.as_deref().and_then(|path| Dump::create(path).map_err(|e| error!("SKATE_VOICE_DUMP: {e}")).ok());
        let io = Box::new(Output { send, live: voice.output.clone(), dump });
        voice.running = Some((engine::spawn(voice.shared.clone(), mic, incoming, io), capture));
    }
    let shared = &voice.shared;
    *lock(&shared.config) = config;
    *lock(&shared.route) = net.voice_route(crate::player_list::blocked);
    *lock(&shared.listen) = net.voice_listen();
    let bot = |m: &Member| voice.hooks.mute_bot && m.name == "Bot";
    *lock(&shared.volumes) = (roster.members.iter().filter(|m| m.actor != roster.local_actor && !bot(m)))
        .filter_map(|m| Some((m.actor, crate::player_list::voice_volume(m.actor)?)))
        .collect();
    let level = f32::from_bits(shared.mic_level_dbfs.load(Relaxed));
    if menu.mic_test {
        menu.mic_level_dbfs = level;
    }
    if menu.mic_error != voice.mic_error {
        menu.mic_error = voice.mic_error.clone();
    }
    let talking: BTreeSet<u64> = lock(&shared.talking).keys().copied().collect();
    // Another session's actors are other players.
    if voice.heard.0 != lobby {
        voice.heard = (lobby, BTreeSet::new());
    }
    voice.heard.1.extend(&talking);
    if voice.hooks.on() {
        for actor in talking.symmetric_difference(&state.talking) {
            info!("Voice test: {actor} {}", if talking.contains(actor) { "talking" } else { "stopped talking" });
        }
        if now - voice.logged >= 5. {
            voice.logged = now;
            let fma = std::is_x86_feature_detected!("fma") && std::env::var_os("SKATE_NO_FMA").is_none();
            let us = shared.capture_us.load(Relaxed);
            info!("Voice test: capture {us} us a 10 ms frame (Noise Suppression {:?}, FMA {fma})", config.noise);
        }
    }
    let mic = voice.running.as_ref().and_then(|(_, mic)| mic.as_ref().map(Capture::device));
    // User 10-29: the others show our grey mic before we talk (it showed only once heard: like voice off).
    net.publish_voice(config.enabled && mic.is_some());
    let voiced = net.voiced();
    state.set_if_neq(VoiceState {
        talking,
        heard: voice.heard.1.union(&voiced).copied().collect(),
        sending: shared.sending.load(Relaxed),
        mic,
        level_dbfs: level,
    });
    *lock(&VOIP) = Voip::new(&roster.members, roster.local_actor, &state, crate::player_list::quiet);
    crate::music::ease_duck(shared.audible.load(Relaxed), menu.duck_for_voice(), time.delta_secs());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keyboard Push to Talk counts only while the window is focused and nothing takes typing; the pad's button counts
    /// while typing too (it only talks), never unfocused.
    #[test]
    fn push_to_talk_ignores_typing_and_unfocused() {
        let (mut keys, mut mouse) = (ButtonInput::<KeyCode>::default(), ButtonInput::<MouseButton>::default());
        keys.press(KeyCode::KeyV);
        assert!(ptt_held(&keys, &mouse, "KeyV", false, true, false));
        assert!(!ptt_held(&keys, &mouse, "KeyV", false, true, true), "typing");
        assert!(!ptt_held(&keys, &mouse, "KeyV", false, false, false), "unfocused");
        assert!(!ptt_held(&keys, &mouse, "KeyB", false, true, false), "another key");
        assert!(ptt_held(&keys, &mouse, "KeyB", true, true, true), "the pad's button while typing");
        assert!(!ptt_held(&keys, &mouse, "KeyB", true, false, false), "not unfocused");
        mouse.press(MouseButton::Forward);
        let none = ButtonInput::<KeyCode>::default();
        assert!(ptt_held(&none, &mouse, "Mouse5", false, true, false), "a mouse side button");
        assert!(!ptt_held(&none, &mouse, "Mouse4", false, true, false));
    }

    /// The Mic Test runs the engine offline; otherwise it needs a session with Voice Chat on.
    #[test]
    fn mic_test_runs_without_a_lobby() {
        assert!(engine_wanted(false, false, true) && engine_wanted(false, true, true) && engine_wanted(true, true, false));
        assert!(!engine_wanted(false, true, false) && !engine_wanted(true, false, false) && !engine_wanted(false, false, false));
    }

    /// Only a voice-test build takes the loopback hooks (user 10-27).
    #[test]
    fn only_test_builds_take_the_loopback_hooks() {
        // Mute Bot: the one hook with no file behind it.
        unsafe { std::env::set_var("SKATE_VOICE_MUTE_BOT", "1") };
        assert_eq!(Hooks::from_env().on(), cfg!(feature = "voice-test"));
        unsafe { std::env::remove_var("SKATE_VOICE_MUTE_BOT") };
    }

    /// The loopback test's hooks: the dump is a 48 kHz mono 16-bit wav, its sizes right as it grows, which the clip
    /// reader takes back sample for sample (to 16 bits); anything else is refused.
    #[test]
    fn the_loopback_hooks_read_and_write_wavs() {
        let dir = std::env::temp_dir().join(format!("skate-voice-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dump.wav");
        let pcm: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let mut dump = Dump::create(&path).unwrap();
        dump.write(&pcm[..480]).unwrap();
        assert_eq!(wav_read(&path).unwrap().len(), 480, "readable as it grows");
        dump.write(&pcm[480..]).unwrap();
        let back = wav_read(&path).unwrap();
        assert_eq!(back.len(), 1000);
        assert!(back.iter().zip(&pcm).all(|(a, b)| (a - b).abs() < 1. / 16000.));
        std::fs::write(&path, b"RIFF\0\0\0\0WAVEjunk").unwrap();
        assert!(wav_read(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The loopback test's clip as a microphone: 10 ms frames of it in order, looping, paced in real time.
    #[test]
    fn the_clip_loops_as_a_microphone() {
        let clip: Vec<f32> = (0..600).map(|i| i as f32).collect();
        let start = std::time::Instant::now();
        let frames = clip_feed(clip.into());
        let got: Vec<Vec<f32>> = (0..3).map(|_| frames.recv().unwrap()).collect();
        assert!(got.iter().all(|f| f.len() == 480));
        assert_eq!((got[0][0], got[1][0], got[1][120], got[2][0]), (0., 480., 0., 360.));
        assert!(start.elapsed() >= std::time::Duration::from_millis(15), "paced: {:?}", start.elapsed());
    }

    /// voip-status-trace.md: one enum in all three games' movies (Skate 3's and 2's voip clip: 1 micoff, 2 micon,
    /// anything else hidden; Skate 1's mcVoipIcon: 0 off, 1 on, 2 active, 3 muted).
    #[test]
    fn voip_status_follows_the_trace() {
        assert_eq!(voip_status(true, false, true), 2, "talking: micon / active");
        assert_eq!(voip_status(false, false, true), 1, "silent, with voice: micoff / on");
        assert_eq!(voip_status(false, false, false), 0, "no voice: hidden / off");
        assert_eq!(voip_status(false, true, true), 3, "muted: hidden / muted");
        assert_eq!(voip_status(true, true, true), 3, "a muted talker is not shown talking");
        assert_eq!(voip_status(false, true, false), 3, "muted whether or not ever heard");
    }

    /// We light while we send (our mic open, silent: with voice); the others while heard, keep their mic once heard and
    /// show muted when muted or blocked. A board finds a player by a name it shows (the roster's or their own), never
    /// by one two players share.
    #[test]
    fn boards_find_a_talker_by_name() {
        let m = |actor, name: &str| Member { actor, name: name.into(), steam: 0, host: false };
        let members = [m(1, "Me"), m(2, "Ann"), m(3, "Hal"), m(4, "Hal"), m(5, "Bo")];
        let state = VoiceState { talking: [2].into(), heard: [2, 4, 5].into(), sending: true, mic: Some("Mic".into()), ..default() };
        let voip = Voip::new(&members, 1, &state, |actor| actor == 5);
        assert_eq!([1, 2, 3, 4, 5, 9].map(|a| voip.of(a)), [2, 2, 0, 1, 3, 0]);
        assert_eq!(["Me", "Ann", "Bo", "Hal #2", "Nobody"].map(|n| voip.named(n)), [2, 2, 3, 1, 0]);
        assert_eq!(voip.named("Hal"), 0, "two players are Hal");
        let silent = VoiceState { sending: false, ..state };
        assert_eq!(Voip::new(&members, 1, &silent, |_| false).of(1), 1, "our mic open, not sending");
        assert_eq!(Voip::new(&members, 1, &VoiceState::default(), |_| false).of(1), 0, "no microphone");
    }
}
