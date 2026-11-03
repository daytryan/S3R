//! The weather mix as pure DSP: rain beds, surface layers, the roof, thunder and the wet-wheel hiss from one
//! [`Params`] block, smoothed here. Deterministic for a seed, so the WAV previews and the MP4 export render it
//! offline exactly as the mixer thread plays it. `render` allocates nothing of its own.
use super::{
    assets::Bank,
    stream::LoopVoice,
    thunder,
    wheels::Hiss,
};
use std::{f32::consts::FRAC_PI_2, sync::Arc};

pub(crate) const RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Surface {
    Hard,
    Metal,
    Wood,
    Foliage,
    Water,
}

/// Everything the game decides; the mixer smooths towards it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Params {
    /// 0 none, 0.5 light, 1 heavy (already strength-gated).
    pub rain: f32,
    /// Per Surface: [left, right] weight, each 0..1, sum ≤ 1 per side.
    pub surfaces: [[f32; 2]; 5],
    /// 0 open .. 1 fully covered.
    pub cover: f32,
    /// Roof material under cover.
    pub roof_metal: bool,
    /// Menus / pause muffle (U4).
    pub paused: bool,
    /// SFX volume 0..1 (tenths / 10).
    pub master: f32,
    /// m/s, 0 when no wet wheel contact.
    pub wheel_speed: f32,
    /// Radians, world, for thunder pan.
    pub camera_yaw: f32,
}

/// A lightning strike: where (world azimuth `atan2(z, x)`, metres) and which recording/pitch (seed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Thunder {
    pub azimuth: f32,
    pub distance_m: f32,
    pub seed: u32,
}

const SURFACE_GAIN: f32 = 0.8;
const ROOF_GAIN: f32 = 0.9;
/// Thunder against the rain: the calibration knob for "peaks 6-12 dB above the bed" (build-progress, Task 5).
const THUNDER_GAIN: f32 = 0.8;
/// Thunder voices at once; another steals the oldest.
const THUNDER_VOICES: usize = 3;
const THUNDER_BINS: [&str; 3] = ["thunder_crack", "thunder_roll", "thunder_rumble"];
/// The whole mix's level: the calibration knob the preview measurements set (build-progress, Task 5); halved (-6 dB)
/// 10-11, the user: "The default rain volume is too loud naturally" (weather is ours, no retail level to match).
const MIX_GAIN: f32 = 1.2;
/// Light rain sits this far under heavy: the stage masters both beds to the same loudness.
const LIGHT_TRIM_DB: f32 = -5.;
/// Frames between parameter updates (1.3 ms): gains step this often, far below a zipper.
const BLOCK: usize = 64;
/// Smoothers move at least this much a second, so a fade ends instead of tailing off for ever.
const MIN_SLEW: f32 = 0.05;
/// Gain drift on every loop: ±1.5 dB, re-aimed every 5-15 s.
const DRIFT_DB: f32 = 1.5;
/// Cover (and roof material) smoothing: the census flips at a roof's edge, this keeps it from fluttering (U3).
pub(crate) const COVER_TAU: f32 = 0.5;
/// A loop under -90 dB is not decoded (heavy rain's f32 `cos(π/2)` is -4e-8, not 0).
const INAUDIBLE: f32 = 3e-5;

/// One-pole smoothing, `y += (x - y)(1 - e^(-dt/τ))`, with a [`MIN_SLEW`] floor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Smoother {
    tau: f32,
    value: f32,
}
impl Smoother {
    pub(crate) fn new(tau: f32) -> Self {
        Self { tau, value: 0. }
    }
    pub(crate) fn step(&mut self, target: f32, dt: f32) {
        let d = target - self.value;
        let floor = MIN_SLEW * dt;
        let mv = d * (1. - (-dt / self.tau).exp());
        self.value = if d.abs() <= floor { target } else if mv.abs() < floor { self.value + floor.copysign(d) } else { self.value + mv };
    }
    pub(crate) fn value(&self) -> f32 {
        self.value
    }
}

/// xorshift64*, uniform in 0..1 (as retail_weather's Storm).
pub(crate) struct Rng(u64);
impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub(crate) fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Stereo one-pole low-pass, `a = 1 - e^(-2π fc / RATE)`.
#[derive(Clone, Copy, Default)]
pub(crate) struct OnePole([f32; 2]);
impl OnePole {
    pub(crate) fn coefficient(cutoff: f32) -> f32 {
        1. - (-std::f32::consts::TAU * cutoff / RATE as f32).exp()
    }
    pub(crate) fn run(&mut self, frame: [f32; 2], a: f32) -> [f32; 2] {
        for (y, x) in self.0.iter_mut().zip(frame) {
            *y += (x - *y) * a;
        }
        self.0
    }
}

/// One channel of an RBJ 2nd-order filter, Q 0.707.
#[derive(Clone, Copy, Default)]
pub(crate) struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}
impl Biquad {
    pub(crate) fn low_pass(cutoff: f32) -> Self {
        Self::rbj(cutoff, false)
    }
    pub(crate) fn high_pass(cutoff: f32) -> Self {
        Self::rbj(cutoff, true)
    }
    fn rbj(cutoff: f32, high: bool) -> Self {
        let w = std::f32::consts::TAU * cutoff / RATE as f32;
        let (cos, alpha) = (w.cos(), w.sin() * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1. + alpha;
        let b = if high { [(1. + cos) / 2., -(1. + cos), (1. + cos) / 2.] } else { [(1. - cos) / 2., 1. - cos, (1. - cos) / 2.] };
        Self { b: b.map(|v| v / a0), a: [-2. * cos / a0, (1. - alpha) / a0], ..Default::default() }
    }
    pub(crate) fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// A thunder clip playing: resampled for pitch (linear), darkened and lowered for distance, panned each block.
struct ThunderVoice {
    voice: LoopVoice,
    /// Where `voice` goes back to: its bin and clip in `Mixer::thunder_pool`.
    home: (usize, usize),
    strike: Thunder,
    lp: OnePole,
    a: f32,
    gain: f32,
    pitch: f32,
    phase: f32,
    /// The two source frames `phase` sits between.
    frames: [[f32; 2]; 2],
    ended: bool,
    started: u64,
}
impl ThunderVoice {
    /// Adds into `out`; false once the clip has played out.
    fn mix(&mut self, out: &mut [f32], pan: [f32; 2]) -> bool {
        for frame in out.chunks_exact_mut(2) {
            let [cur, next] = self.frames;
            let v = [cur[0] + (next[0] - cur[0]) * self.phase, cur[1] + (next[1] - cur[1]) * self.phase];
            let [l, r] = self.lp.run(v, self.a);
            frame[0] += l * self.gain * pan[0];
            frame[1] += r * self.gain * pan[1];
            self.phase += self.pitch;
            while self.phase >= 1. {
                self.phase -= 1.;
                if self.ended {
                    return false;
                }
                let following = self.voice.next_frame().unwrap_or_else(|| {
                    self.ended = true;
                    [0.; 2]
                });
                self.frames = [next, following];
            }
        }
        true
    }
}

/// The loops, in [`LAYER_ROLES`] order: beds, hard (light, heavy), metal, wood, foliage, water, roofs.
const LAYER_ROLES: [&str; 10] =
    ["bed_light", "bed_heavy", "hard_light", "hard_heavy", "metal", "wood", "foliage", "water", "roof_metal", "roof_concrete"];
const ROOF_METAL: usize = 8;
const ROOF_CONCRETE: usize = 9;

struct Layer {
    voice: Option<LoopVoice>,
    clip_gain: f32,
    drift: Smoother,
    drift_target: f32,
    drift_wait: f32,
    /// The gain the last block played at (both 0: silent, not decoded).
    gain: [f32; 2],
}

pub(crate) struct Mixer {
    bank: Arc<Bank>,
    rng: Rng,
    target: Params,
    rain: Smoother,
    surfaces: [[Smoother; 2]; 5],
    cover: Smoother,
    roof_metal: Smoother,
    paused: Smoother,
    master: Smoother,
    layers: Vec<Layer>,
    cover_lp: OnePole,
    pause_lp: OnePole,
    /// The outdoor layers (beds, surfaces) before the cover filter, one block.
    outdoor: Vec<f32>,
    wheel_speed: Smoother,
    hiss: Hiss,
    /// Strikes not heard yet: frames still to wait, the strike.
    pending: [Option<(u64, Thunder)>; 4],
    thunder: [Option<ThunderVoice>; THUNDER_VOICES],
    /// Per bin, one voice per clip, opened in `new` (the main thread, in `decoder()`) and rewound for reuse, so a
    /// strike never parses or allocates on the mixer thread. None while that clip is playing.
    thunder_pool: [Vec<Option<LoopVoice>>; 3],
    /// Per bin, the clip played last (never repeated back to back).
    last: [Option<usize>; 3],
    /// Frames rendered.
    clock: u64,
}

impl Mixer {
    pub(crate) fn new(bank: Arc<Bank>, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let layers = LAYER_ROLES
            .iter()
            .map(|role| {
                let clips = bank.clips(role);
                let clip = (!clips.is_empty()).then(|| &clips[(rng.next() * clips.len() as f32) as usize % clips.len()]);
                let start = rng.next();
                Layer {
                    voice: clip.and_then(|c| LoopVoice::new(c, start)),
                    clip_gain: clip.map_or(0., |c| c.gain),
                    drift: Smoother::new(3.),
                    drift_target: 0.,
                    drift_wait: 5. + 10. * rng.next(),
                    gain: [0.; 2],
                }
            })
            .collect();
        let thunder_pool = THUNDER_BINS.map(|role| bank.clips(role).iter().map(|c| LoopVoice::new(c, 0.)).collect());
        Self {
            bank,
            rng,
            target: Params::default(),
            rain: Smoother::new(3.),
            surfaces: [[Smoother::new(1.); 2]; 5],
            cover: Smoother::new(COVER_TAU),
            roof_metal: Smoother::new(COVER_TAU),
            paused: Smoother::new(0.3),
            master: Smoother::new(0.1),
            layers,
            cover_lp: OnePole::default(),
            pause_lp: OnePole::default(),
            outdoor: vec![0.; BLOCK * 2],
            wheel_speed: Smoother::new(0.15),
            hiss: Hiss::new(seed ^ 0x5DEE_CE66),
            pending: [None; 4],
            thunder: [const { None }; THUNDER_VOICES],
            thunder_pool,
            last: [None; 3],
            clock: 0,
        }
    }

    /// Schedules a strike heard `delay_s` from now (thunder::delay_s). With four already waiting, the one due
    /// last gives way.
    pub(crate) fn strike(&mut self, strike: Thunder, delay_s: f32) {
        // Any caller (a replay file included) is held to the storm's range: a distance near 0 would make the gain
        // infinite and the whole output NaN.
        if !strike.distance_m.is_finite() || !delay_s.is_finite() {
            return;
        }
        let strike = Thunder {
            azimuth: if strike.azimuth.is_finite() { strike.azimuth } else { 0. },
            distance_m: strike.distance_m.clamp(thunder::MIN_KM * 1000., thunder::MAX_KM * 1000.),
            ..strike
        };
        let wait = (delay_s.max(0.) * RATE as f32) as u64;
        let slot = self
            .pending
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| (0..4).max_by_key(|&i| self.pending[i].map_or(0, |p| p.0)).unwrap_or(0));
        self.pending[slot] = Some((wait, strike));
    }

    /// Drops every strike not yet heard (replay scrub, weather off); thunder already rolling plays out.
    pub(crate) fn cancel_pending(&mut self) {
        self.pending = [None; 4];
    }

    /// A finished or stolen thunder voice back to its pool, rewound.
    fn give_back(&mut self, done: ThunderVoice) {
        let (b, index) = done.home;
        let mut voice = done.voice;
        voice.rewind();
        if let Some(home) = self.thunder_pool[b].get_mut(index) {
            *home = Some(voice);
        }
    }

    fn start_thunder(&mut self, strike: Thunder) {
        let role = thunder::bin(strike.distance_m);
        let b = THUNDER_BINS.iter().position(|r| *r == role).unwrap_or(0);
        let count = self.thunder_pool[b].len();
        if count == 0 {
            return;
        }
        let slot = self.thunder.iter().position(Option::is_none).unwrap_or_else(|| {
            (0..THUNDER_VOICES).min_by_key(|&i| self.thunder[i].as_ref().map_or(0, |v| v.started)).unwrap_or(0)
        });
        // The oldest voice gives way first, so its clip can be picked again.
        if let Some(stolen) = self.thunder[slot].take() {
            self.give_back(stolen);
        }
        // The picked clip still rolling from an earlier strike: the next one that is free.
        let picked = thunder::pick(strike.seed, count, self.last[b]);
        let Some(index) = (0..count).map(|k| (picked + k) % count).find(|&i| self.thunder_pool[b][i].is_some()) else {
            return;
        };
        let Some(mut voice) = self.thunder_pool[b][index].take() else { return };
        self.last[b] = Some(index);
        let clip_gain = self.bank.clips(role).get(index).map_or(1., |c| c.gain);
        let first = voice.next_frame().unwrap_or([0.; 2]);
        self.thunder[slot] = Some(ThunderVoice {
            voice,
            home: (b, index),
            strike,
            lp: OnePole::default(),
            a: OnePole::coefficient(thunder::cutoff_hz(strike.distance_m)),
            gain: 10f32.powf(thunder::gain_db(strike.distance_m) / 20.) * THUNDER_GAIN * clip_gain,
            pitch: thunder::pitch(strike.seed),
            phase: 0.,
            frames: [[0.; 2], first],
            ended: false,
            started: self.clock,
        });
    }

    pub(crate) fn set(&mut self, params: Params) {
        self.target = params;
    }

    /// Fill interleaved stereo f32.
    pub(crate) fn render(&mut self, out: &mut [f32]) {
        out.fill(0.);
        for block in out.chunks_mut(BLOCK * 2) {
            self.block(block);
        }
    }

    fn step(&mut self, dt: f32) {
        let t = self.target;
        let finite = |v: f32| if v.is_finite() { v.clamp(0., 1.) } else { 0. };
        self.rain.step(finite(t.rain), dt);
        for (side, weights) in self.surfaces.iter_mut().zip(t.surfaces) {
            for (s, w) in side.iter_mut().zip(weights) {
                s.step(finite(w), dt);
            }
        }
        self.cover.step(finite(t.cover), dt);
        self.roof_metal.step(if t.roof_metal { 1. } else { 0. }, dt);
        self.paused.step(if t.paused { 1. } else { 0. }, dt);
        self.master.step(finite(t.master), dt);
        // Paused, the physics holds the deck's last velocity: the wheels are still.
        let wheels = if t.paused || !t.wheel_speed.is_finite() { 0. } else { t.wheel_speed.clamp(0., 40.) };
        self.wheel_speed.step(wheels, dt);
        for layer in &mut self.layers {
            layer.drift_wait -= dt;
            if layer.drift_wait <= 0. {
                layer.drift_wait += 5. + 10. * self.rng.next();
                layer.drift_target = (self.rng.next() * 2. - 1.) * DRIFT_DB;
            }
            layer.drift.step(layer.drift_target, dt);
        }
    }

    /// Each loop's [left, right] gain for the smoothed parameters, before drift and make-up.
    fn layer_gains(&self) -> [[f32; 2]; 10] {
        let r = self.rain.value();
        let (h, s) = (((r - 0.5) * 2.).clamp(0., 1.), (r * 2.).clamp(0., 1.));
        let s = s * 10f32.powf(LIGHT_TRIM_DB * (1. - h) / 20.);
        let (light, heavy) = ((h * FRAC_PI_2).cos(), (h * FRAC_PI_2).sin());
        let heavy_bed = heavy * s;
        // Heavy rain keeps the light bed 6 dB under it.
        let light_bed = (light * s).max(0.5 * heavy_bed);
        let side = |surface: Surface| self.surfaces[surface as usize].map(|w| s * w.value() * SURFACE_GAIN);
        let scale = |g: [f32; 2], k: f32| g.map(|v| v * k);
        let hard = side(Surface::Hard);
        let roof = self.cover.value() * s * ROOF_GAIN;
        let metal_roof = self.roof_metal.value();
        [
            [light_bed; 2],
            [heavy_bed; 2],
            scale(hard, light),
            scale(hard, heavy),
            side(Surface::Metal),
            side(Surface::Wood),
            side(Surface::Foliage),
            side(Surface::Water),
            [roof * metal_roof.sqrt(); 2],
            [roof * (1. - metal_roof).sqrt(); 2],
        ]
    }

    fn block(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        self.step(frames as f32 / RATE as f32);
        let gains = self.layer_gains();
        let outdoor = &mut self.outdoor[..out.len()];
        outdoor.fill(0.);
        for (i, (layer, g)) in self.layers.iter_mut().zip(gains).enumerate() {
            let k = layer.clip_gain * 10f32.powf(layer.drift.value() / 20.);
            let gain = g.map(|v| if (v * k).abs() < INAUDIBLE { 0. } else { v * k });
            let silent = gain == [0.; 2] && layer.gain == [0.; 2];
            layer.gain = gain;
            if let (false, Some(voice)) = (silent, &mut layer.voice) {
                let into = if i == ROOF_METAL || i == ROOF_CONCRETE { &mut *out } else { &mut *outdoor };
                voice.mix_into(into, gain);
            }
        }
        // Cover: the outdoor layers through a low-pass gliding 18 kHz -> 800 Hz and down 9 dB.
        let cover = self.cover.value();
        let a = OnePole::coefficient(18_000. * (800f32 / 18_000.).powf(cover));
        let level = 10f32.powf(-9. * cover / 20.);
        for (o, x) in out.chunks_exact_mut(2).zip(outdoor.chunks_exact(2)) {
            let [l, r] = self.cover_lp.run([finite(x[0]), finite(x[1])], a);
            o[0] += l * level;
            o[1] += r * level;
        }
        // Thunder (not under the cover filter: it comes from the sky, not the street) and the wheel hiss (the
        // wheels are where the skater is).
        for i in 0..self.pending.len() {
            match self.pending[i] {
                Some((wait, strike)) if wait < frames as u64 => {
                    self.pending[i] = None;
                    self.start_thunder(strike);
                }
                Some((wait, strike)) => self.pending[i] = Some((wait - frames as u64, strike)),
                None => {}
            }
        }
        let yaw = if self.target.camera_yaw.is_finite() { self.target.camera_yaw } else { 0. };
        for i in 0..THUNDER_VOICES {
            let Some(voice) = &mut self.thunder[i] else { continue };
            let s = voice.strike;
            if !voice.mix(out, thunder::pan(s.azimuth, yaw, s.distance_m))
                && let Some(done) = self.thunder[i].take()
            {
                self.give_back(done);
            }
        }
        self.hiss.mix_into(out, self.wheel_speed.value(), self.rain.value());
        self.clock += frames as u64;
        // Pause (U4): everything through a low-pass gliding to 2 kHz and down 10 dB; then the SFX volume.
        let paused = self.paused.value();
        let a = OnePole::coefficient(18_000. * (2_000f32 / 18_000.).powf(paused));
        let level = 10f32.powf(-10. * paused / 20.) * self.master.value().powi(2) * MIX_GAIN;
        for o in out.chunks_exact_mut(2) {
            let [l, r] = self.pause_lp.run([finite(o[0]), finite(o[1])], a);
            o[0] = soft_clip(l * level);
            o[1] = soft_clip(r * level);
        }
    }
}

/// A sample that is not finite (a corrupt clip, a strike from a bad file) is dropped before a filter could hold
/// it for ever.
fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0. }
}

#[cfg(test)]
impl Mixer {
    /// The smoothed cover the filter and roof follow.
    pub(crate) fn cover_level(&self) -> f32 {
        self.cover.value()
    }
    /// The [left, right] gain a loop last played at (0: not decoded).
    pub(crate) fn layer_gain(&self, role: &str) -> [f32; 2] {
        LAYER_ROLES.iter().position(|r| *r == role).map_or([0.; 2], |i| self.layers[i].gain)
    }
    /// Thunder voices free to start, per bin.
    pub(crate) fn thunder_ready(&self) -> [usize; 3] {
        self.thunder_pool.each_ref().map(|p| p.iter().flatten().count())
    }
}

/// Linear to 0.8, then a knee that bends towards 0.89 (-1 dBFS) and never reaches it: continuous, slope 1 at the
/// knee. Only drip transients and a close crack reach it.
fn soft_clip(x: f32) -> f32 {
    let e = x.abs() - 0.8;
    if e <= 0. { x } else { (0.8 + e / (1. + e / 0.09)).copysign(x) }
}
