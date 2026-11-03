//! The voice engine: one thread, no Bevy. Mic frames → Mic Volume → noise suppression → the talk gate → Opus → voice
//! datagrams, under the upload cap; voice datagrams → the receive checks → a jitter buffer per allowed speaker → the mix
//! (× their volume × Voice Volume), paced by the output device's clock.
use crate::{
    codec::{Encoder, Packet, bitrate_for, frame_ms_for},
    dsp::{FRAME, Filter, Gate, Mode, Noise, level_dbfs},
    jitter::Jitter,
    mix::mix,
};
use skate_net::voice::{Limits, VoiceListen, VoiceRoute, accept, datagrams};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, Ordering::Relaxed},
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Voice datagram bytes a talker sends in any second at most; a packet that would pass it is dropped, never queued.
/// Audit 10-30: was 110 kB, 5 % over a full lobby's speech, so Opus running a little over its target cut a talker out.
const UPLOAD_CAP: usize = 130_000;
/// How far ahead (30 ms of samples) the output is kept: the device draining it paces the mix.
const OUTPUT_AHEAD: usize = 48 * 30;
/// A speaker is talking while their last packet came within this long; ducking holds as long after a voice's last frame.
const TALKING_MS: u64 = 250;
/// How often the voice thread wakes without a mic buffer, to keep the output fed.
const TICK: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub mode: Mode,
    pub noise: Noise,
    /// Percent, 0-200.
    pub mic_volume: u16,
    /// Percent, 0-100.
    pub voice_volume: u8,
    pub mic_test: bool,
}

/// What the game and the voice thread share. Each side holds a lock only to copy a value in or out.
pub struct Shared {
    pub config: Mutex<Config>,
    /// From the game: where our voice goes, and whose we take (`None`: no session).
    pub route: Mutex<Option<VoiceRoute>>,
    pub listen: Mutex<Option<VoiceListen>>,
    /// From the game: who may be heard, at what volume (%). Absent: muted, blocked or outside Voice From; their packets
    /// are never decoded.
    pub volumes: Mutex<BTreeMap<u64, u16>>,
    pub ptt: AtomicBool,
    /// Published: who is talking (actor → when their last packet came, voice clock ms); gone 250 ms after it.
    pub talking: Mutex<BTreeMap<u64, u64>>,
    /// Published: our gate is open (we are sending; alone in a session too, so our own mic lights while nothing goes
    /// out: user 10-27).
    pub sending: AtomicBool,
    /// Published: someone else's voice was mixed in within the last 250 ms (Lower Game Sound While Talking).
    pub audible: AtomicBool,
    /// Published: the last mic frame's level, f32 bits (dBFS): the Mic Test meter.
    pub mic_level_dbfs: AtomicU32,
    /// Published: the capture's mean time a 10 ms frame over the last second (µs), for the loopback test's log.
    pub capture_us: AtomicU32,
}

impl Shared {
    pub fn new(config: Config) -> Self {
        Self {
            config: Mutex::new(config),
            route: Mutex::default(),
            listen: Mutex::default(),
            volumes: Mutex::default(),
            ptt: AtomicBool::new(false),
            talking: Mutex::default(),
            sending: AtomicBool::new(false),
            audible: AtomicBool::new(false),
            mic_level_dbfs: AtomicU32::new(level_dbfs(&[]).to_bits()),
            capture_us: AtomicU32::new(0),
        }
    }
}

/// A poisoned lock still holds a whole value (each is copied in or out in one go), so a panic elsewhere never stops voice.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub trait Io: Send {
    fn send(&mut self, endpoint: u64, datagram: &[u8]);
    fn output(&mut self, pcm: &[f32]);
    /// Samples still waiting at the output.
    fn queued(&self) -> usize;
}

/// The voice thread; dropping it stops and joins the thread.
pub struct Handle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the voice thread: it wakes on each mic buffer (or every 5 ms) and runs the engine on what came.
pub fn spawn(shared: Arc<Shared>, mic: Receiver<Vec<f32>>, incoming: Receiver<(u64, Vec<u8>)>, io: Box<dyn Io>) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread = std::thread::Builder::new()
        .name("voice".into())
        .spawn({
            let stop = stop.clone();
            move || {
                // Ahead of ordinary threads, whatever the window does (crate::priority).
                if let Err(error) = crate::priority::raise() {
                    eprintln!("Voice chat could not raise its thread: {error}");
                }
                let (mut engine, start) = (Engine::new(shared, io), Instant::now());
                let (mut samples, mut datagrams) = (vec![], vec![]);
                while !stop.load(Relaxed) {
                    match mic.recv_timeout(TICK) {
                        Ok(buffer) => samples.extend(buffer),
                        Err(RecvTimeoutError::Timeout) => {}
                        // No microphone: listen only.
                        Err(RecvTimeoutError::Disconnected) => std::thread::sleep(TICK),
                    }
                    samples.extend(mic.try_iter().flatten());
                    datagrams.extend(incoming.try_iter());
                    engine.step(start.elapsed().as_millis() as u64, &samples, &datagrams);
                    samples.clear();
                    datagrams.clear();
                }
            }
        })
        .expect("voice thread");
    Handle { stop, thread: Some(thread) }
}

/// A speaker being heard: their jitter buffer and the samples pulled from it not mixed yet.
struct Voice {
    jitter: Jitter,
    pending: VecDeque<f32>,
}

pub(crate) struct Engine {
    shared: Arc<Shared>,
    io: Box<dyn Io>,
    config: Config,
    route: Option<VoiceRoute>,
    listen: Option<VoiceListen>,
    volumes: BTreeMap<u64, u16>,
    filter: Filter,
    gate: Gate,
    encoder: Encoder,
    /// Mic samples short of a whole 10 ms frame.
    mic: Vec<f32>,
    /// The voice clock: 10 ms mic frames since the start (a packet's `seq` is its first frame's).
    clock: u32,
    /// The packet being gathered: its first frame's tick, its length (ms) and its samples so far.
    packet: Option<(u32, u32, Vec<f32>)>,
    /// What we sent in the last second (when, bytes), for the upload cap.
    upload: VecDeque<(u64, usize)>,
    limits: Limits,
    voices: BTreeMap<u64, Voice>,
    /// Mic Test: our processed frames waiting to be played back.
    loopback: VecDeque<f32>,
    last_audible: Option<u64>,
    /// The capture's frames and time so far this second ([`Shared::capture_us`]).
    timing: (u32, Duration),
}

impl Engine {
    pub(crate) fn new(shared: Arc<Shared>, io: Box<dyn Io>) -> Self {
        let config = *lock(&shared.config);
        Self {
            filter: Filter::new(config.noise),
            config,
            shared,
            io,
            route: None,
            listen: None,
            volumes: BTreeMap::new(),
            gate: Gate::new(),
            encoder: Encoder::new(),
            mic: Vec::new(),
            clock: 0,
            packet: None,
            upload: VecDeque::new(),
            limits: Limits::default(),
            voices: BTreeMap::new(),
            loopback: VecDeque::new(),
            last_audible: None,
            timing: (0, Duration::ZERO),
        }
    }
    /// One pass of the voice thread at `now_ms`: the game's latest settings, then `mic` samples, `incoming` datagrams
    /// (endpoint, bytes), then the output.
    pub(crate) fn step(&mut self, now_ms: u64, mic: &[f32], incoming: &[(u64, Vec<u8>)]) {
        self.sync();
        self.mic.extend_from_slice(mic);
        let mut taken = 0;
        while self.mic.len() - taken >= FRAME {
            let frame: [f32; FRAME] = self.mic[taken..taken + FRAME].try_into().unwrap();
            taken += FRAME;
            let started = Instant::now();
            self.capture(now_ms, frame);
            self.timed(started.elapsed());
        }
        self.mic.drain(..taken);
        for (from, data) in incoming {
            self.receive(now_ms, *from, data);
        }
        self.play(now_ms);
    }

    /// One frame's capture time; each 100 frames (a second) their mean goes to [`Shared::capture_us`].
    fn timed(&mut self, took: Duration) {
        self.timing = (self.timing.0 + 1, self.timing.1 + took);
        if self.timing.0 == 100 {
            self.shared.capture_us.store((self.timing.1.as_micros() / 100) as u32, Relaxed);
            self.timing = (0, Duration::ZERO);
        }
    }

    /// The game's latest settings, route, listeners and volumes; speakers who left or may no longer be heard are freed.
    fn sync(&mut self) {
        let config = *lock(&self.shared.config);
        if config.noise != self.config.noise {
            self.filter = Filter::new(config.noise);
        }
        if !config.mic_test {
            self.loopback.clear();
        }
        if !config.enabled {
            self.voices.clear();
        }
        self.config = config;
        self.route = lock(&self.shared.route).clone();
        let listen = lock(&self.shared.listen).clone();
        if listen != self.listen {
            let actors: BTreeSet<u64> = listen.iter().flat_map(|l| l.peers.values().map(|&(actor, _)| actor)).collect();
            self.voices.retain(|actor, _| actors.contains(actor));
            lock(&self.shared.talking).retain(|actor, _| actors.contains(actor));
            match &listen {
                Some(listen) => self.limits.retain(listen),
                None => self.limits = Limits::default(),
            }
            self.listen = listen;
        }
        self.volumes = lock(&self.shared.volumes).clone();
        self.voices.retain(|actor, _| self.volumes.contains_key(actor));
    }

    /// One 10 ms mic frame: Mic Volume, the noise filter and the meter; then heard back (Mic Test) or through the gate
    /// into a packet.
    fn capture(&mut self, now_ms: u64, mut frame: [f32; FRAME]) {
        let gain = f32::from(self.config.mic_volume) / 100.;
        frame.iter_mut().for_each(|s| *s = (*s * gain).clamp(-1., 1.));
        let speech = self.filter.process(&mut frame);
        let level = level_dbfs(&frame);
        self.shared.mic_level_dbfs.store(level.to_bits(), Relaxed);
        if self.config.mic_test {
            self.loopback.extend(frame);
            let stale = self.loopback.len().saturating_sub(OUTPUT_AHEAD);
            self.loopback.drain(..stale);
            self.packet = None;
            self.shared.sending.store(false, Relaxed);
        } else {
            let open = self.config.enabled
                && self.gate.step(self.config.mode, self.shared.ptt.load(Relaxed), level, speech, 10);
            self.shared.sending.store(open, Relaxed);
            self.gather(now_ms, &frame, open);
        }
        self.clock = self.clock.wrapping_add(1);
    }

    /// Adds `frame` to the packet being gathered (one starts only while the gate is `open`; one started is finished with
    /// the mic's own audio, not cut), and sends the packet once whole, under the upload cap.
    fn gather(&mut self, now_ms: u64, frame: &[f32; FRAME], open: bool) {
        let Some(route) = self.route.as_ref().filter(|r| self.config.enabled && !r.peers.is_empty()) else {
            self.packet = None;
            return;
        };
        if self.packet.is_none() && !open {
            return;
        }
        // The frame size is picked as a packet starts, so it only changes between packets.
        let (_, frame_ms, pcm) = self.packet.get_or_insert_with(|| (self.clock, frame_ms_for(route.peers.len(), route.congested), Vec::new()));
        pcm.extend_from_slice(frame);
        if pcm.len() < 48 * *frame_ms as usize {
            return;
        }
        let (seq, frame_ms, pcm) = self.packet.take().unwrap();
        let opus = self.encoder.encode(&pcm, frame_ms, bitrate_for(route.peers.len(), route.congested));
        // DTX silence is not sent.
        if opus.len() <= 2 {
            return;
        }
        let sends = datagrams(route, seq, &Packet { seq, frame_ms: frame_ms as u8, opus }.payload());
        let bytes: usize = sends.iter().map(|(_, d)| d.len()).sum();
        while self.upload.front().is_some_and(|&(at, _)| now_ms.saturating_sub(at) >= 1000) {
            self.upload.pop_front();
        }
        if self.upload.iter().map(|(_, b)| b).sum::<usize>() + bytes > UPLOAD_CAP {
            return;
        }
        self.upload.push_back((now_ms, bytes));
        for (peer, data) in &sends {
            self.io.send(*peer, data);
        }
    }

    /// A datagram from `from`: voice from a member we may hear, within their caps, into their jitter buffer.
    fn receive(&mut self, now_ms: u64, from: u64, data: &[u8]) {
        let Some(listen) = self.listen.as_ref().filter(|_| self.config.enabled) else { return };
        let Some((actor, seq, payload)) = accept(listen, &mut self.limits, from, data, now_ms) else { return };
        // Muted, blocked or outside Voice From: never decoded.
        if !self.volumes.contains_key(&actor) {
            return;
        }
        let Some(packet) = Packet::parse(seq, &payload) else { return };
        lock(&self.shared.talking).insert(actor, now_ms);
        let voice = self.voices.entry(actor).or_insert_with(|| Voice { jitter: Jitter::new(), pending: VecDeque::new() });
        voice.jitter.push(packet, now_ms);
    }

    /// Tops the output up to 30 ms ahead, 10 ms at a time: each voice's due audio at its gains, and ours in the Mic Test.
    fn play(&mut self, now_ms: u64) {
        lock(&self.shared.talking).retain(|_, at| now_ms.saturating_sub(*at) <= TALKING_MS);
        let master = f32::from(self.config.voice_volume) / 100.;
        // Never more than 30 ms a pass, whatever the output reports.
        for _ in 0..OUTPUT_AHEAD / FRAME {
            if self.io.queued() >= OUTPUT_AHEAD {
                break;
            }
            let mut frames: Vec<(Vec<f32>, f32)> = vec![];
            for (actor, voice) in &mut self.voices {
                while voice.pending.len() < FRAME {
                    let Some(audio) = voice.jitter.pull(now_ms) else { break };
                    voice.pending.extend(audio);
                }
                let n = voice.pending.len().min(FRAME);
                if n == 0 {
                    continue;
                }
                let gain = f32::from(self.volumes.get(actor).copied().unwrap_or(0)) / 100. * master;
                if gain > 0. {
                    self.last_audible = Some(now_ms);
                }
                frames.push((voice.pending.drain(..n).collect(), gain));
            }
            let n = self.loopback.len().min(FRAME);
            frames.push((self.loopback.drain(..n).collect(), 1.));
            let mut out = [0f32; FRAME];
            mix(&frames.iter().map(|(s, g)| (s.as_slice(), *g)).collect::<Vec<_>>(), &mut out);
            self.io.output(&out);
        }
        self.shared.audible.store(self.last_audible.is_some_and(|at| now_ms.saturating_sub(at) <= TALKING_MS), Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codec::tests::rms, dsp::tests::brown};
    use std::{
        sync::atomic::Ordering::Relaxed,
        time::{Duration, Instant},
    };

    fn correlation(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().min(b.len());
        let mean = |x: &[f32]| x[..n].iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
        let (ma, mb) = (mean(a), mean(b));
        let (mut ab, mut aa, mut bb) = (0., 0., 0.);
        for (&x, &y) in a[..n].iter().zip(&b[..n]) {
            let (x, y) = (f64::from(x) - ma, f64::from(y) - mb);
            (ab, aa, bb) = (ab + x * y, aa + x * x, bb + y * y);
        }
        ab / (aa * bb).sqrt().max(f64::MIN_POSITIVE)
    }

    /// `ms` of a vowel, peak 1: a jittered 120 Hz glottal pulse train through formants at 700, 1220 and 2600 Hz, which
    /// RNNoise calls speech.
    fn vowel(ms: usize) -> Vec<f32> {
        let n = 48 * ms;
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut jitter = move || {
            (x ^= x << 13, x ^= x >> 7, x ^= x << 17);
            (x as f64 / u64::MAX as f64) as f32 - 0.5
        };
        let (mut s, mut t) = (vec![0f32; n], 0f32);
        while (t as usize) < n {
            s[t as usize] = 1.;
            t += 48_000. / (120. * (1. + 0.04 * jitter()));
        }
        for (f, bw) in [(700f32, 80f32), (1220., 100.), (2600., 150.)] {
            let r = (-std::f32::consts::PI * bw / 48_000.).exp();
            let (a1, a2) = (2. * r * (std::f32::consts::TAU * f / 48_000.).cos(), -r * r);
            let (mut y1, mut y2) = (0f32, 0f32);
            for v in &mut s {
                let y = *v + a1 * y1 + a2 * y2;
                (y2, y1, *v) = (y1, y, y);
            }
        }
        let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
        s.iter().map(|v| v / peak).collect()
    }

    /// [`vowel`] shaped into words, (start ms, length ms, peak) each with 5 ms fades, silence between: `ms` long.
    fn words(ms: usize, list: &[(usize, usize, f32)]) -> Vec<f32> {
        let v = vowel(ms);
        let mut out = vec![0f32; 48 * ms];
        for &(start, len, peak) in list {
            let (a, b) = (48 * start, (48 * (start + len)).min(out.len()));
            for i in a..b {
                out[i] = v[i] * peak * ((i - a).min(b - 1 - i) as f32 / 240.).min(1.);
            }
        }
        out
    }

    /// Syllables of random length and loudness over `ms`: no stretch repeats, so a delay is unambiguous.
    fn syllables(ms: usize) -> Vec<f32> {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut below = move |n: u64| {
            (x ^= x << 13, x ^= x >> 7, x ^= x << 17);
            (x % n) as usize
        };
        let (mut list, mut t) = (vec![], 0);
        while t < ms {
            let len = 80 + below(160);
            list.push((t, len, 0.05 + below(20) as f32 / 100.));
            t += len + 20 + below(80);
        }
        words(ms, &list)
    }

    /// 20 ms loudness.
    fn envelope(x: &[f32]) -> Vec<f32> {
        x.chunks(960).map(rms).collect()
    }

    /// The delay (20 ms frames, up to 400 ms) at which `out`'s loudness best follows `input`'s, and how well.
    fn follows(input: &[f32], out: &[f32]) -> (usize, f64) {
        let (a, b) = (envelope(input), envelope(out));
        (0..=20).map(|lag| (lag, correlation(&a, &b[lag.min(b.len())..]))).max_by(|x, y| x.1.total_cmp(&y.1)).unwrap()
    }

    #[derive(Default)]
    struct Wire {
        sent: Vec<(u64, Vec<u8>)>,
        out: Vec<f32>,
        queued: usize,
    }

    /// An in-memory network and output device.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Wire>>);

    impl Io for Fake {
        fn send(&mut self, endpoint: u64, datagram: &[u8]) {
            self.0.lock().unwrap().sent.push((endpoint, datagram.to_vec()));
        }
        fn output(&mut self, pcm: &[f32]) {
            let mut wire = self.0.lock().unwrap();
            wire.out.extend(pcm);
            wire.queued += pcm.len();
        }
        fn queued(&self) -> usize {
            self.0.lock().unwrap().queued
        }
    }

    struct Side {
        engine: Engine,
        shared: Arc<Shared>,
        wire: Fake,
    }

    impl Side {
        fn new(config: Config, route: Option<VoiceRoute>, listen: Option<VoiceListen>, volumes: &[(u64, u16)]) -> Self {
            let shared = Arc::new(Shared::new(config));
            *shared.route.lock().unwrap() = route;
            *shared.listen.lock().unwrap() = listen;
            *shared.volumes.lock().unwrap() = volumes.iter().copied().collect();
            let wire = Fake::default();
            Self { engine: Engine::new(shared.clone(), Box::new(wire.clone())), shared, wire }
        }
        fn sent(&self) -> Vec<(u64, Vec<u8>)> {
            std::mem::take(&mut self.wire.0.lock().unwrap().sent)
        }
        fn out(&self) -> Vec<f32> {
            self.wire.0.lock().unwrap().out.clone()
        }
        /// The output device plays 10 ms.
        fn play(&self) {
            let mut wire = self.wire.0.lock().unwrap();
            wire.queued = wire.queued.saturating_sub(FRAME);
        }
    }

    fn config(mode: Mode) -> Config {
        Config { enabled: true, mode, noise: Noise::Off, mic_volume: 100, voice_volume: 100, mic_test: false }
    }

    fn route(local: u64, peers: &[(u64, u64)], congested: bool) -> Option<VoiceRoute> {
        Some(VoiceRoute { session: 7, local, congested, peers: peers.to_vec() })
    }

    /// A (endpoint 1, actor 10) and B (endpoint 2, actor 20); B hears A at `volume` (`None`: not allowed).
    fn pair(mode: Mode, volume: Option<u16>) -> (Side, Side) {
        let listen = |peer, actor, token| Some(VoiceListen { session: 7, peers: [(peer, (actor, token))].into() });
        let a = Side::new(config(mode), route(10, &[(2, 0xB0B)], false), listen(2, 20, 0xA0A), &[(20, 100)]);
        let b = Side::new(config(mode), route(20, &[(1, 0xA0A)], false), listen(1, 10, 0xB0B), &volume.map(|v| (10, v)).into_iter().collect::<Vec<_>>());
        (a, b)
    }

    /// 10 ms steps to `end` ms: A's mic plays `input` (then silence) with Push to Talk held before `ptt_until`; its
    /// datagrams reach B 30 ms later; each device plays 10 ms a step, B's not during `stall`. `each` sees every step
    /// with how many datagrams A sent in it.
    fn run(a: &mut Side, b: &mut Side, input: &[f32], ptt_until: u64, end: u64, stall: std::ops::Range<u64>, mut each: impl FnMut(u64, &mut Side, &mut Side, usize)) {
        let mut wire: Vec<(u64, Vec<u8>)> = vec![];
        for t in (0..end).step_by(10) {
            a.shared.ptt.store(t < ptt_until, Relaxed);
            let i = 48 * t as usize;
            let mic: Vec<f32> = (i..i + FRAME).map(|j| input.get(j).copied().unwrap_or(0.)).collect();
            a.engine.step(t, &mic, &[]);
            let sent = a.sent();
            let n = sent.len();
            wire.extend(sent.into_iter().map(|(_, d)| (t + 30, d)));
            let (due, later): (Vec<_>, Vec<_>) = wire.into_iter().partition(|(at, _)| *at <= t);
            wire = later;
            b.engine.step(t, &[], &due.into_iter().map(|(_, d)| (1, d)).collect::<Vec<_>>());
            a.play();
            if !stall.contains(&t) {
                b.play();
            }
            each(t, a, b, n);
        }
    }

    /// The loopback test's log: the capture's mean time a frame, published each second (Standard suppression here).
    #[test]
    fn the_capture_time_a_frame_is_published() {
        let mut a = Side::new(Config { noise: Noise::Standard, ..config(Mode::PushToTalk) }, None, None, &[]);
        for (i, frame) in brown(-30., 100).iter().enumerate() {
            a.engine.step(10 * i as u64, frame, &[]);
        }
        assert!(a.shared.capture_us.load(Relaxed) > 0, "after 100 frames");
    }

    #[test]
    fn two_engines_talk() {
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        let input = syllables(2000);
        let (mut last_sent, mut heard, mut cleared, mut sending) = (None, false, None, vec![]);
        run(&mut a, &mut b, &input, 2000, 3000, 0..0, |t, a, b, sent| {
            if sent > 0 {
                last_sent = Some(t);
            }
            if t == 1000 || t == 2500 {
                sending.push(a.shared.sending.load(Relaxed));
            }
            let talking = b.shared.talking.lock().unwrap().contains_key(&10);
            heard |= talking;
            if !talking && t > 2000 && cleared.is_none() {
                cleared = Some(t);
            }
        });
        let (lag, c) = follows(&input, &b.out());
        assert!(c >= 0.9, "B's output follows A's voice: {c:.3} at {} ms", lag * 20);
        assert!(heard, "B saw A talking");
        let (last, cleared) = (last_sent.unwrap(), cleared.unwrap());
        assert!(cleared > last && cleared - last <= 30 + 250 + 10, "talking cleared {} ms after A's last packet left", cleared - last);
        assert_eq!(sending, [true, false], "A's own sending state: held, then after the tail");
    }

    /// Alone in a session (user 10-27): Push to Talk still lights our own mic, so it can be tried solo; nothing goes out.
    #[test]
    fn alone_our_mic_lights_and_nothing_is_sent() {
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        *a.shared.route.lock().unwrap() = route(10, &[], false);
        let (mut sent, mut sending) = (0, vec![]);
        run(&mut a, &mut b, &syllables(2000), 2000, 3000, 0..0, |t, a, _, n| {
            sent += n;
            if t == 1000 || t == 2500 {
                sending.push(a.shared.sending.load(Relaxed));
            }
        });
        assert_eq!((sending, sent), (vec![true, false], 0), "held, then after the tail; nothing sent");
    }

    /// B's output level (dBFS, 0.5-1 s) while A says a steady vowel at peak 0.1, B hearing A at `volume` with Voice
    /// Volume at `master`; and the most speakers B decoded at once.
    fn heard_at(volume: Option<u16>, master: u8) -> (f32, usize) {
        let (mut a, mut b) = pair(Mode::PushToTalk, volume);
        b.shared.config.lock().unwrap().voice_volume = master;
        let input: Vec<f32> = vowel(1000).iter().map(|v| v * 0.1).collect();
        let mut decoded = 0;
        run(&mut a, &mut b, &input, 1000, 1000, 0..0, |_, _, b, _| decoded = decoded.max(b.engine.voices.len()));
        (level_dbfs(&b.out()[48 * 500..48 * 1000]), decoded)
    }

    #[test]
    fn mute_and_volume_apply() {
        let ((full, _), (double, _), (half, _)) = (heard_at(Some(100), 100), heard_at(Some(200), 100), heard_at(Some(100), 50));
        assert!((double - full - 6.).abs() <= 1., "200 %: {:.1} dB over 100 %", double - full);
        assert!((full - half - 6.).abs() <= 1., "Voice Volume 50 %: {:.1} dB under 100 %", full - half);
        assert!(heard_at(Some(0), 100).0 < -100., "0 %: silence");
        let (absent, decoded) = heard_at(None, 100);
        assert!(absent < -100. && decoded == 0, "not allowed: nothing decoded ({decoded} speakers, {absent} dBFS)");
    }

    /// Ducking's signal: someone else's voice mixed in at a volume over 0, until 250 ms after its last frame.
    #[test]
    fn audible_while_a_voice_is_mixed_in() {
        for (volume, heard) in [(100, true), (0, false)] {
            let (mut a, mut b) = pair(Mode::PushToTalk, Some(volume));
            let mut audible = vec![];
            run(&mut a, &mut b, &syllables(1000), 1000, 2500, 0..0, |t, _, b, _| {
                if t == 900 || t == 2400 {
                    audible.push(b.shared.audible.load(Relaxed));
                }
            });
            assert_eq!(audible, [heard, false], "at {volume} %: while A talks, then after");
        }
    }

    /// Also with Voice Chat Off (it never sends).
    #[test]
    fn mic_test_plays_locally_and_sends_nothing() {
        for enabled in [true, false] {
            let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
            (a.shared.config.lock().unwrap().mic_test, a.shared.config.lock().unwrap().enabled) = (true, enabled);
            let input: Vec<f32> = vowel(1000).iter().map(|v| v * 0.2).collect();
            let mut sent = 0;
            run(&mut a, &mut b, &input, 1000, 1000, 0..0, |_, _, _, n| sent += n);
            assert_eq!(sent, 0, "nothing sent");
            let heard = level_dbfs(&a.out()[48 * 200..48 * 1000]);
            assert!((heard - level_dbfs(&input[48 * 200..48 * 1000])).abs() <= 1., "we hear ourselves at {heard:.1} dBFS");
            let meter = f32::from_bits(a.shared.mic_level_dbfs.load(Relaxed));
            assert_eq!(meter, level_dbfs(&input[48 * 990..48 * 1000]), "the meter reads the last mic frame");
        }
    }

    #[test]
    fn voice_off_sends_and_hears_nothing() {
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        a.shared.config.lock().unwrap().enabled = false;
        let (mut sent, mut sending) = (0, false);
        run(&mut a, &mut b, &syllables(1000), 1000, 1000, 0..0, |_, a, _, n| {
            sent += n;
            sending |= a.shared.sending.load(Relaxed);
        });
        assert_eq!((sent, sending), (0, false), "A off: nothing goes out, and A's mic shows no sending");
        // A turns it off mid-sentence, half a 20 ms packet in (tick 50 of 50-51): that packet is not finished and sent.
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        let mut late = 0;
        run(&mut a, &mut b, &syllables(1000), 1000, 1000, 0..0, |t, a, _, n| {
            if t == 500 {
                a.shared.config.lock().unwrap().enabled = false;
            } else if t > 500 {
                late += n;
            }
        });
        assert_eq!(late, 0, "nothing after Voice Chat Off");
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        b.shared.config.lock().unwrap().enabled = false;
        let mut decoded = 0;
        run(&mut a, &mut b, &syllables(1000), 1000, 1000, 0..0, |_, _, b, _| decoded = decoded.max(b.engine.voices.len()));
        assert!(decoded == 0 && level_dbfs(&b.out()) < -100., "B off: A not heard ({decoded} speakers)");
    }

    #[test]
    fn a_leaver_is_freed() {
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        let (mut before, mut after) = ((0, false), (9, true));
        run(&mut a, &mut b, &syllables(1000), 1000, 1000, 0..0, |t, _, b, _| {
            let state = (b.engine.voices.len(), b.shared.talking.lock().unwrap().contains_key(&10));
            if t == 490 {
                before = state;
                *b.shared.listen.lock().unwrap() = Some(VoiceListen { session: 7, peers: BTreeMap::new() });
            } else if t == 500 {
                after = state;
            }
        });
        assert_eq!(before, (1, true), "A heard");
        assert_eq!(after, (0, false), "A left: no jitter buffer, not talking");
    }

    #[test]
    fn output_stalls_drop_stale_audio() {
        let (mut a, mut b) = pair(Mode::PushToTalk, Some(100));
        let input = syllables(5000);
        let mut resumed = 0;
        run(&mut a, &mut b, &input, 5000, 5000, 1000..3000, |t, _, b, _| {
            if t == 2990 {
                resumed = b.out().len();
            }
        });
        // B's device stopped for 2 s; then B plays A as of the moment, not A from 2 s before.
        let after = envelope(&b.out()[resumed..]);
        let (lag, c) = (0..=150usize).map(|lag| (lag, correlation(&envelope(&input[48 * 3000 - 960 * lag..]), &after))).max_by(|x, y| x.1.total_cmp(&y.1)).unwrap();
        assert!(lag * 20 < 400 && c >= 0.8, "after the stall B plays A from {} ms before ({c:.2})", lag * 20);
    }

    #[test]
    fn open_mic_noise_sends_nothing_between_words() {
        let (mut a, mut b) = pair(Mode::OpenMic { sensitivity: 5 }, Some(100));
        // A noisy room: brown noise at -35 dBFS (over the -38 dBFS line) all along, two words in it. Noise suppression
        // off, so only the speech check keeps the gate shut.
        let mut input = words(3000, &[(500, 400, 0.3), (2000, 400, 0.3)]);
        for (s, n) in input.iter_mut().zip(brown(-35., 300).into_iter().flatten()) {
            *s += n;
        }
        let mut sent_at = vec![];
        run(&mut a, &mut b, &input, 0, 3000, 0..0, |t, _, _, n| {
            if n > 0 {
                sent_at.push(t);
            }
        });
        let during = |from: u64, to: u64| sent_at.iter().filter(|t| (from..to).contains(*t)).count();
        assert!(during(500, 900) > 0 && during(2000, 2400) > 0, "both words go out: {sent_at:?}");
        // RNNoise still calls 30-40 ms after a word speech; then the 300 ms hangover; then the 10 ms that finishes the
        // 20 ms packet the hangover's last frame started.
        let quiet = |word_end: u64| word_end + 50 + 300 + 10;
        assert_eq!(during(quiet(900), 2000) + during(quiet(2400), 3000), 0, "nothing past the 300 ms hangover: {sent_at:?}");
    }

    #[test]
    /// A full lobby: the 60 ms 12 kbps tier alone keeps it under the cap (speech peaks ~101 kB/s, noise ~85), with a
    /// fifth to spare for Opus running over its target (audit 10-30).
    fn upload_stays_under_the_cap() {
        let peers: Vec<(u64, u64)> = (2..50).map(|e| (e, e)).collect();
        let mut a = Side::new(config(Mode::PushToTalk), route(10, &peers, false), None, &[]);
        a.shared.ptt.store(true, Relaxed);
        let input: Vec<f32> = vowel(60_000).iter().map(|v| v * 0.5).collect();
        let mut per_step = vec![];
        for t in (0..60_000u64).step_by(10) {
            let i = 48 * t as usize;
            a.engine.step(t, &input[i..i + FRAME], &[]);
            per_step.push(a.sent().iter().map(|(_, d)| d.len()).sum::<usize>());
            a.play();
        }
        let most = per_step.windows(100).map(|w| w.iter().sum::<usize>()).max().unwrap();
        let mean = per_step.iter().sum::<usize>() / 60;
        assert!(most <= 110_000, "{most} bytes in a second to 48 listeners");
        assert!(most + most / 5 <= UPLOAD_CAP, "{most} bytes leave a fifth to spare under {UPLOAD_CAP}");
        assert!(mean > 80_000, "and the voice does go out: {mean} bytes a second");
    }

    /// Past the cap (more listeners than a lobby holds), whole packets are dropped, never queued: each one sent goes to
    /// everyone in the step it was finished.
    #[test]
    fn past_the_cap_whole_packets_are_dropped_not_queued() {
        let peers: Vec<(u64, u64)> = (2..102).map(|e| (e, e)).collect();
        let mut a = Side::new(config(Mode::PushToTalk), route(10, &peers, false), None, &[]);
        a.shared.ptt.store(true, Relaxed);
        let input = syllables(20_000);
        let mut per_step = vec![];
        for t in (0..20_000u64).step_by(10) {
            let i = 48 * t as usize;
            a.engine.step(t, &input[i..i + FRAME], &[]);
            let sent = a.sent();
            let seqs: BTreeSet<u32> = sent.iter().map(|(_, d)| skate_net::packed::envelope(d).unwrap().3).collect();
            // A 60 ms packet from tick `seq` is finished by the mic frame of tick seq + 5.
            let whole_and_now = sent.len() == 100 && seqs.len() == 1 && seqs.first().is_some_and(|s| u64::from(s + 5) * 10 == t);
            assert!(sent.is_empty() || whole_and_now, "at {t} ms: {} datagrams of packets {seqs:?}", sent.len());
            per_step.push(sent.iter().map(|(_, d)| d.len()).sum::<usize>());
            a.play();
        }
        let most = per_step.windows(100).map(|w| w.iter().sum::<usize>()).max().unwrap();
        let mean = per_step.iter().sum::<usize>() / 20;
        assert!(most <= UPLOAD_CAP && mean >= 95_000, "{most} bytes at most in a second, {mean} a second on average");
    }

    #[test]
    fn a_congested_route_uses_the_smallest_tier() {
        for (congested, frame_ms) in [(false, 20u8), (true, 60)] {
            let mut a = Side::new(config(Mode::PushToTalk), route(10, &[(2, 2), (3, 3)], congested), None, &[]);
            a.shared.ptt.store(true, Relaxed);
            let input: Vec<f32> = vowel(500).iter().map(|v| v * 0.3).collect();
            let mut sizes = std::collections::BTreeSet::new();
            for t in (0..500u64).step_by(10) {
                let i = 48 * t as usize;
                a.engine.step(t, &input[i..i + FRAME], &[]);
                sizes.extend(a.sent().iter().map(|(_, d)| d[skate_net::packed::HEADER]));
                a.play();
            }
            assert_eq!(sizes, [frame_ms].into(), "congested {congested}");
        }
    }

    #[test]
    fn the_thread_sends_and_stops() {
        let shared = Arc::new(Shared::new(config(Mode::PushToTalk)));
        *shared.route.lock().unwrap() = route(10, &[(2, 2)], false);
        shared.ptt.store(true, Relaxed);
        let (mic, mic_rx) = std::sync::mpsc::channel();
        let (_incoming, incoming_rx) = std::sync::mpsc::channel();
        let wire = Fake::default();
        let handle = spawn(shared, mic_rx, incoming_rx, Box::new(wire.clone()));
        // A device's odd-sized buffers.
        for chunk in vowel(500).chunks(441) {
            mic.send(chunk.iter().map(|v| v * 0.3).collect()).unwrap();
        }
        let t = Instant::now();
        while wire.0.lock().unwrap().sent.is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!wire.0.lock().unwrap().sent.is_empty(), "the voice thread sends");
        drop(handle);
        assert_eq!(Arc::strong_count(&wire.0), 1, "and stops on drop: its Io is gone");
    }

    /// The voice thread runs time-critical, as the game's mixer does: in a background window a normal-priority thread
    /// is starved under load, and a stall comes out as lost speech (the 10-27 two-copy run lost 220 ms once).
    #[cfg(windows)]
    #[test]
    fn the_voice_thread_runs_time_critical() {
        struct Probe(std::sync::mpsc::Sender<i32>);
        impl Io for Probe {
            fn send(&mut self, _: u64, _: &[u8]) {}
            fn output(&mut self, _: &[f32]) {
                let _ = self.0.send(crate::priority::current());
            }
            fn queued(&self) -> usize {
                0
            }
        }
        let (probe, priority) = std::sync::mpsc::channel();
        let (_mic, mic_rx) = std::sync::mpsc::channel();
        let (_incoming, incoming_rx) = std::sync::mpsc::channel();
        let _voice = spawn(Arc::new(Shared::new(config(Mode::PushToTalk))), mic_rx, incoming_rx, Box::new(Probe(probe)));
        assert_eq!(priority.recv_timeout(Duration::from_secs(5)), Ok(crate::priority::TIME_CRITICAL));
    }
}
