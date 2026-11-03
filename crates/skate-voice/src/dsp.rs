//! The voice DSP on 10 ms frames: noise suppression (RNNoise / DeepFilterNet), the talk gate and the level meter. Mic
//! Volume is the caller's: `sample * mic_volume / 100` before [`Filter::process`].

pub const RATE: u32 = 48_000;
pub const FRAME: usize = 480;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    Off,
    Standard,
    High,
}

/// Open Mic: the speech probability (RNNoise's) a frame needs as well as the level.
const SPEECH: f32 = 0.5;
const PTT_TAIL_MS: u32 = 200;
const HANGOVER_MS: u32 = 300;

pub struct Filter {
    noise: Noise,
    rnnoise: Box<nnnoiseless::DenoiseState<'static>>,
    /// DeepFilterNet and its output frame, for High (None: it would not load, so High runs as Standard).
    deep: Option<(Box<df::tract::DfTract>, ndarray::Array2<f32>)>,
}

impl Filter {
    pub fn new(noise: Noise) -> Self {
        let deep = (noise == Noise::High)
            .then(|| df::tract::DfTract::new(df::tract::DfParams::default(), &df::tract::RuntimeParams::default()).ok())
            .flatten()
            .map(|d| (Box::new(d), ndarray::Array2::zeros((1, FRAME))));
        Self { noise, rnnoise: nnnoiseless::DenoiseState::new(), deep }
    }
    /// Filters `frame` in place and returns RNNoise's speech probability for it (RNNoise runs for that under every
    /// setting).
    pub fn process(&mut self, frame: &mut [f32; FRAME]) -> f32 {
        // RNNoise works in 16-bit scale.
        let mut denoised = [0f32; FRAME];
        let speech = self.rnnoise.process_frame(&mut denoised, &frame.map(|s| s * 32768.));
        match (self.noise, &mut self.deep) {
            (Noise::Off, _) => {}
            (Noise::High, Some((deep, out))) => {
                let noisy = ndarray::ArrayView2::from_shape((1, FRAME), &frame[..]).unwrap();
                if deep.process(noisy, out.view_mut()).is_ok() {
                    frame.copy_from_slice(out.as_slice().unwrap());
                }
            }
            _ => *frame = denoised.map(|s| s / 32768.),
        }
        speech
    }
    pub fn delay_ms(&self) -> u32 {
        match (self.noise, &self.deep) {
            (Noise::Off, _) => 0,
            (Noise::High, Some((d, _))) => ((d.fft_size - d.hop_size + d.lookahead * d.hop_size) * 1000 / d.sr) as u32,
            // RNNoise: a 20 ms window every 10 ms puts out the frame before.
            _ => 10,
        }
    }
}

/// A frame's RMS in dBFS (full scale 1.0); silence reads -120 dBFS rather than minus infinity.
pub fn level_dbfs(frame: &[f32]) -> f32 {
    let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
    20. * rms.max(1e-6).log10()
}

/// The Open Mic line (dBFS) at Sensitivity 1..=10: -54 + 4·(s−1).
pub fn line_dbfs(sensitivity: u8) -> f32 {
    -54. + 4. * f32::from(sensitivity.clamp(1, 10) - 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    PushToTalk,
    OpenMic { sensitivity: u8 },
}

#[derive(Default)]
pub struct Gate {
    /// The tail still to send after Push to Talk lets go (200 ms), or Open Mic's hangover after a word (300 ms).
    tail_ms: u32,
    /// Open Mic: the last frame was speech over the line.
    heard: bool,
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }
    /// Whether to send this frame (`dt_ms` long).
    pub fn step(&mut self, mode: Mode, ptt_held: bool, level_dbfs: f32, speech: f32, dt_ms: u32) -> bool {
        let (open, tail) = match mode {
            Mode::PushToTalk => (ptt_held, PTT_TAIL_MS),
            Mode::OpenMic { sensitivity } => {
                let heard = level_dbfs >= line_dbfs(sensitivity) && speech >= SPEECH;
                // Shut, it takes two such frames in a row: RNNoise calls a lone frame of a noisy room speech now and then.
                let open = heard && (self.heard || self.tail_ms > 0);
                self.heard = heard;
                (open, HANGOVER_MS)
            }
        };
        if open {
            self.tail_ms = tail;
            return true;
        }
        let send = self.tail_ms > 0;
        self.tail_ms = self.tail_ms.saturating_sub(dt_ms);
        send
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One 10 ms frame through `gate`: loud speech (-30 dBFS, probability 0.9) or a quiet room (-60 dBFS, 0.1).
    fn step(gate: &mut Gate, mode: Mode, ptt: bool, talking: bool) -> bool {
        let (level, speech) = if talking { (-30., 0.9) } else { (-60., 0.1) };
        gate.step(mode, ptt, level, speech, 10)
    }

    #[test]
    fn push_to_talk_sends_while_held_and_200_ms_after() {
        let mut gate = Gate::new();
        // Frames at 0, 10, 20 ... ms; the key is held through the one at 500 ms; the room is quiet (PTT ignores it).
        let sent: Vec<(u32, bool)> = (0..=80).map(|i| (i * 10, step(&mut gate, Mode::PushToTalk, i * 10 <= 500, false))).collect();
        assert!(sent.iter().filter(|(t, _)| *t <= 700).all(|(_, s)| *s), "held 0-500 ms, then 200 ms more");
        assert!(sent.iter().filter(|(t, _)| *t >= 710).all(|(_, s)| !*s), "then nothing");
    }

    #[test]
    fn open_mic_needs_loudness_and_speech() {
        let mode = Mode::OpenMic { sensitivity: 5 };
        // Two frames: a shut gate opens on the second (`a_lone_speech_frame_does_not_open_open_mic`).
        let opens = |level, speech| {
            let mut gate = Gate::new();
            gate.step(mode, false, level, speech, 10);
            gate.step(mode, false, level, speech, 10)
        };
        assert!(opens(-30., 0.9), "loud speech");
        assert!(!opens(-30., 0.2), "loud, but not speech");
        assert!(!opens(-50., 0.9), "speech under the line (-38 dBFS at 5)");
    }

    /// RNNoise calls a lone frame of a noisy room speech now and then (one at 0.50, 350 ms after a word, in brown noise
    /// at -35 dBFS): that must not send 300 ms of the room.
    #[test]
    fn a_lone_speech_frame_does_not_open_open_mic() {
        let (mode, mut gate) = (Mode::OpenMic { sensitivity: 5 }, Gate::new());
        let frames = [false, false, true, false, false, true].into_iter().chain([false; 40]);
        let sent: Vec<bool> = frames.map(|talking| step(&mut gate, mode, false, talking)).collect();
        assert!(sent.iter().all(|s| !s), "{sent:?}");
    }

    #[test]
    fn open_mic_holds_300_ms_between_words() {
        let (mode, mut gate) = (Mode::OpenMic { sensitivity: 5 }, Gate::new());
        let word: Vec<bool> = (0..10).map(|_| step(&mut gate, mode, false, true)).collect();
        assert!(!word[0] && word[1..].iter().all(|s| *s), "a word, from its second frame: {word:?}");
        assert!((0..25).all(|_| step(&mut gate, mode, false, false)), "a 250 ms pause stays open");
        assert!(step(&mut gate, mode, false, true), "the next word");
        assert!((0..30).all(|_| step(&mut gate, mode, false, false)), "300 ms after the last word");
        assert!(!step(&mut gate, mode, false, false), "then closed");
    }

    #[test]
    fn line_follows_sensitivity() {
        assert_eq!((line_dbfs(1), line_dbfs(5), line_dbfs(10)), (-54., -38., -18.));
    }

    #[test]
    fn level_is_rms_in_dbfs() {
        let square = |a: f32| -> Vec<f32> { (0..FRAME).map(|i| if i % 2 == 0 { a } else { -a }).collect() };
        assert!(level_dbfs(&square(1.)).abs() < 1e-3);
        assert!((level_dbfs(&square(0.1)) + 20.).abs() < 1e-3);
        assert!(level_dbfs(&[0.; FRAME]).is_finite(), "silence reads a number, below any line");
        assert!(level_dbfs(&[0.; FRAME]) < line_dbfs(1));
    }

    /// `frames` frames of white noise at `dbfs` RMS.
    fn white(dbfs: f32, frames: usize) -> Vec<[f32; FRAME]> {
        // Uniform in +-a has an RMS of a / sqrt(3).
        let a = 10f32.powf(dbfs / 20.) * 3f32.sqrt();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            (x ^= x << 13, x ^= x >> 7, x ^= x << 17);
            (x as f64 / u64::MAX as f64 * 2. - 1.) as f32 * a
        };
        (0..frames).map(|_| std::array::from_fn(|_| next())).collect()
    }

    #[test]
    fn noise_off_still_reports_speech() {
        let mut filter = Filter::new(Noise::Off);
        for frame in white(-20., 20) {
            let mut out = frame;
            let speech = filter.process(&mut out);
            assert!((0.0..=1.0).contains(&speech), "{speech}");
            assert_eq!(out, frame, "Off leaves the samples alone");
        }
    }

    /// `frames` frames of brown noise at `dbfs` RMS: white through a leaky integrator, mostly low like a fan or a road.
    pub(crate) fn brown(dbfs: f32, frames: usize) -> Vec<[f32; FRAME]> {
        let mut y = 0f32;
        let mut frames = white(dbfs, frames);
        frames.iter_mut().flatten().for_each(|s| (y = 0.98 * y + *s, *s = y).1);
        let gain = 10f32.powf((dbfs - level_dbfs(frames.as_flattened())) / 20.);
        frames.iter_mut().flatten().for_each(|s| *s *= gain);
        frames
    }

    /// The level of the second second of 2 s of `noise` frames through `filter`.
    fn through(filter: Noise, noise: Vec<[f32; FRAME]>) -> f32 {
        let mut filter = Filter::new(filter);
        let out: Vec<f32> = noise
            .into_iter()
            .flat_map(|mut frame| {
                filter.process(&mut frame);
                frame
            })
            .collect();
        level_dbfs(&out[RATE as usize..])
    }

    /// Steady background noise: a fan or a road. (RNNoise leaves flat white hiss nearly alone: it reads like unvoiced
    /// speech, -40 dBFS in, -42.5 out.)
    #[test]
    fn standard_filter_quiets_steady_noise() {
        let level = through(Noise::Standard, brown(-40., 200));
        assert!(level <= -50., "{level} dBFS");
    }

    /// DeepFilterNet (High) quiets white hiss as well, with its 30 ms of delay (DECISIONS.md).
    #[test]
    fn high_filter_quiets_steady_noise() {
        let level = through(Noise::High, white(-40., 200));
        assert!(level <= -50., "{level} dBFS");
        assert_eq!((Filter::new(Noise::Off).delay_ms(), Filter::new(Noise::Standard).delay_ms(), Filter::new(Noise::High).delay_ms()), (0, 10, 30));
    }
}
