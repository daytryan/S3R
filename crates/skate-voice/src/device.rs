//! Audio devices for voice: the names to choose from, and the microphone captured as 48 kHz mono 10 ms frames.
use crate::dsp::{FRAME, RATE};
use cpal::{
    SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc::{Receiver, Sender, SyncSender, channel, sync_channel},
    },
    thread::JoinHandle,
};

/// Frames the callback may have waiting for the voice thread (160 ms); past that new ones are dropped.
const QUEUE: usize = 16;

/// The microphones by name (the Windows default is not listed: it is `None`). Enumerating takes tens of ms: cache it.
pub fn inputs() -> Vec<String> {
    names(cpal::default_host().input_devices())
}

/// The outputs by name, as [`inputs`].
pub fn outputs() -> Vec<String> {
    names(cpal::default_host().output_devices())
}

fn names(devices: Result<impl Iterator<Item = cpal::Device>, cpal::DevicesError>) -> Vec<String> {
    devices.map(|d| d.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

/// `wanted`'s index in `names`; `None` (the default) when it is not there.
pub fn pick(names: &[String], wanted: Option<&str>) -> Option<usize> {
    names.iter().position(|n| Some(n.as_str()) == wanted)
}

/// The wanted device, else the default, else the default's error (listen-only, and why).
pub fn open_with_fallback<T>(wanted: Option<&str>, open: impl Fn(Option<&str>) -> Result<T, String>) -> Result<T, String> {
    wanted.and_then(|name| open(Some(name)).ok()).map_or_else(|| open(None), Ok)
}

/// Streaming sample-rate conversion by cubic Hermite interpolation; equal rates copy.
pub struct Resampler {
    from: i64,
    to: i64,
    /// The 3 samples before the next input.
    last: [f32; 3],
    /// The next output's place in the next input, in 1/`to` of a sample.
    pos: i64,
}

impl Resampler {
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        Self { from: from_hz.into(), to: to_hz.into(), last: [0.; 3], pos: 0 }
    }
    /// Appends `input` at the new rate to `out`; its last 2 samples wait for the next call's to interpolate past them.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.from == self.to {
            out.extend_from_slice(input);
            return;
        }
        let n = input.len() as i64;
        let x = |i: i64| if i < 0 { self.last[(i + 3) as usize] } else { input[i as usize] };
        // Each output needs the 2 samples before it and the 2 after.
        while self.pos.div_euclid(self.to) + 2 < n {
            let (i, t) = (self.pos.div_euclid(self.to), self.pos.rem_euclid(self.to) as f32 / self.to as f32);
            let (x0, x1, x2, x3) = (x(i - 1), x(i), x(i + 1), x(i + 2));
            let (c1, c2, c3) = (0.5 * (x2 - x0), x0 - 2.5 * x1 + 2. * x2 - 0.5 * x3, 0.5 * (x3 - x0) + 1.5 * (x1 - x2));
            out.push(((c3 * t + c2) * t + c1) * t + x1);
            self.pos += self.from;
        }
        self.pos -= n * self.to;
        self.last = [x(n - 3), x(n - 2), x(n - 1)];
    }
}

/// The capture callback's work: interleaved samples → mono (the channels' average) → 48 kHz → 10 ms frames.
struct Frames {
    channels: usize,
    resampler: Resampler,
    mono: Vec<f32>,
    out: Vec<f32>,
}

impl Frames {
    fn new(channels: usize, rate: u32) -> Self {
        Self { channels: channels.max(1), resampler: Resampler::new(rate, RATE), mono: vec![], out: vec![] }
    }
    fn push(&mut self, interleaved: impl IntoIterator<Item = f32>, mut send: impl FnMut(Vec<f32>)) {
        self.mono.clear();
        let (mut sum, mut n) = (0., 0);
        for s in interleaved {
            (sum, n) = (sum + s, n + 1);
            if n == self.channels {
                self.mono.push(sum / n as f32);
                (sum, n) = (0., 0);
            }
        }
        self.resampler.process(&self.mono, &mut self.out);
        let whole = self.out.len() / FRAME * FRAME;
        self.out[..whole].chunks_exact(FRAME).for_each(|frame| send(frame.to_vec()));
        self.out.drain(..whole);
    }
}

/// A microphone being captured. cpal's stream lives on a thread of its own (it is not `Send`), so this can be kept
/// anywhere; dropping it stops the stream.
pub struct Capture {
    device: String,
    failed: Arc<AtomicBool>,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// `name`'s microphone (`None`: the default), and its frames: 480 samples at 48 kHz mono, at most 16 waiting (past
    /// that new ones are dropped; the device's callback never waits).
    pub fn open(name: Option<&str>) -> Result<(Capture, Receiver<Vec<f32>>), String> {
        let (frames, received) = sync_channel(QUEUE);
        let (opened, result) = sync_channel(1);
        let (stop, stopped) = channel::<()>();
        let failed = Arc::new(AtomicBool::new(false));
        let (name, error) = (name.map(str::to_owned), failed.clone());
        let thread = std::thread::Builder::new()
            .name("voice capture".into())
            .spawn(move || match start(name.as_deref(), frames, error) {
                Ok((stream, device)) => {
                    let _ = opened.send(Ok(device));
                    // Until the Capture drops, and its sender with it.
                    let _ = stopped.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = opened.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        let device = result.recv().map_err(|e| e.to_string())??;
        Ok((Capture { device, failed, stop: Some(stop), thread: Some(thread) }, received))
    }
    pub fn device(&self) -> String {
        self.device.clone()
    }
    /// The device reported an error (it went away): open it again.
    pub fn failed(&self) -> bool {
        self.failed.load(Relaxed)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens and starts `name`'s microphone (`None`: the default): its stream and its name.
fn start(name: Option<&str>, frames: SyncSender<Vec<f32>>, failed: Arc<AtomicBool>) -> Result<(Stream, String), String> {
    let host = cpal::default_host();
    let device = match name {
        Some(name) => host.input_devices().map_err(|e| e.to_string())?.find(|d| d.name().is_ok_and(|n| n == name)),
        None => host.default_input_device(),
    }
    .ok_or("no microphone")?;
    let device_name = device.name().map_err(|e| e.to_string())?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let (config, chain) = (supported.config(), Frames::new(supported.channels().into(), supported.sample_rate().0));
    let on_error = move |_| failed.store(true, Relaxed);
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, chain, frames, on_error),
        SampleFormat::I16 => build::<i16>(&device, &config, chain, frames, on_error),
        SampleFormat::I32 => build::<i32>(&device, &config, chain, frames, on_error),
        SampleFormat::U16 => build::<u16>(&device, &config, chain, frames, on_error),
        format => return Err(format!("{format:?} microphones are not supported")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, device_name))
}

fn build<T: SizedSample>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut chain: Frames,
    frames: SyncSender<Vec<f32>>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream, cpal::BuildStreamError>
where
    f32: cpal::FromSample<T>,
{
    let mut raised = false;
    let callback = move |data: &[T], _: &cpal::InputCallbackInfo| {
        // cpal's own raise fails (crate::priority): its thread is raised on its first buffer.
        if !std::mem::replace(&mut raised, true) {
            let _ = crate::priority::raise();
        }
        // A full queue drops this frame: the device's callback never waits.
        chain.push(data.iter().map(|s| s.to_sample::<f32>()), |frame| drop(frames.try_send(frame)));
    };
    device.build_input_stream(config, callback, on_error, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, time::Duration};

    #[test]
    fn a_missing_device_falls_back_to_default() {
        let names = ["Headset Mic".to_string(), "USB Mic".to_string()];
        assert_eq!(pick(&names, Some("USB Mic")), Some(1));
        assert_eq!(pick(&names, Some("Gone Mic")), None, "unplugged: the default");
        assert_eq!(pick(&names, None), None);
    }

    #[test]
    fn a_failed_capture_falls_back_to_default_then_listens_only() {
        let tried = RefCell::new(vec![]);
        let default_only = |name: Option<&str>| {
            tried.borrow_mut().push(name.map(str::to_owned));
            name.is_none().then_some("default").ok_or("gone".to_string())
        };
        assert_eq!(open_with_fallback(Some("USB Mic"), default_only), Ok("default"));
        assert_eq!(*tried.borrow(), [Some("USB Mic".to_string()), None], "the wanted one, then the default");
        // Audit 10-30: why neither opened (Windows' mic privacy, another app holding it) is kept, not dropped.
        let refused = |name: Option<&str>| Err::<(), _>(format!("{} refused", name.unwrap_or("default")));
        assert_eq!(open_with_fallback(Some("USB Mic"), refused), Err("default refused".into()), "neither: listen-only, and why");
        assert_eq!(open_with_fallback(None, |name| name.is_none().then_some(7).ok_or(String::new())), Ok(7), "none chosen: the default");
    }

    /// A 44.1 kHz stereo microphone in a device's 10 ms buffers: 48 kHz mono, in tune, no click where buffers meet.
    #[test]
    fn downmix_and_resample_to_48k() {
        let (mut frames, mut out) = (Frames::new(2, 44_100), vec![]);
        let tone = |n: usize| (std::f32::consts::TAU * 1000. * n as f32 / 44_100.).sin();
        for call in 0..100 {
            // Left at 0.6, right at 0.2: their average is 0.4.
            let buffer: Vec<f32> = (441 * call..441 * (call + 1)).flat_map(|n| [0.6 * tone(n), 0.2 * tone(n)]).collect();
            frames.push(buffer, |frame| out.push(frame));
        }
        assert!(out.iter().all(|f| f.len() == FRAME), "10 ms frames");
        let mono = out.concat();
        assert!(mono.len() >= 48_000 - 2 * FRAME, "{} samples from 1 s", mono.len());
        let rising = mono.windows(2).filter(|w| w[0] < 0. && w[1] >= 0.).count();
        let hz = rising as f32 * 48_000. / mono.len() as f32;
        assert!((hz - 1000.).abs() <= 10., "{hz} Hz");
        let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
        assert!((rms / (0.4 / 2f32.sqrt()) - 1.).abs() <= 0.01, "the channels' average: rms {rms}");
        let most = 0.4 * std::f32::consts::TAU * 1000. / 48_000.;
        let step = mono.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(step <= 2. * most, "no click: largest step {step} (the tone's own {most})");
        let mut same = vec![];
        Resampler::new(48_000, 48_000).process(&[0.1, -0.2, 0.3], &mut same);
        assert_eq!(same, [0.1, -0.2, 0.3], "a 48 kHz microphone passes through");
    }

    #[test]
    fn a_microphone_that_is_gone_does_not_open() {
        assert!(Capture::open(Some("no such microphone")).is_err());
    }

    /// A real microphone (skipped without one): 10 ms frames come.
    #[test]
    #[ignore]
    fn opens_the_default_microphone() {
        let Ok((capture, frames)) = Capture::open(None) else {
            eprintln!("no microphone: skipped");
            return;
        };
        let frame = frames.recv_timeout(Duration::from_secs(2)).expect("a frame within 2 s");
        assert_eq!(frame.len(), FRAME);
        assert!(!capture.device().is_empty() && !capture.failed(), "{}", capture.device());
    }
}
