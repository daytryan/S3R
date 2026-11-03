//! Weather audio in replays and the MP4 export (U5, plan Task 9): the recorded rain at the playhead, and thunder
//! re-fired from the recorded strikes as the playhead crosses them, never twice for one crossing.
use super::{
    assets::Bank,
    mixer::{Mixer, Params, Thunder},
    thunder::delay_s,
};
use crate::retail_render::LightningStrike;
use std::sync::Arc;

/// Frames the playhead may advance in one step and still count as playing through them (half a second).
const MAX_STEP: usize = 30;

/// What the playhead's move asks of the mixer.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Step {
    /// Drop the thunder still pending (a seek, a scrub, reverse play).
    pub cancel: bool,
    /// Strikes to queue, with the delay left from now (seconds, real time).
    pub strikes: Vec<(Thunder, f32)>,
}

pub(crate) fn thunder_of(s: LightningStrike) -> Thunder {
    Thunder { azimuth: s.azimuth, distance_m: s.distance_m, seed: s.seed }
}

/// Follows the playhead's recorded frame index. Forward play through a strike's frame queues its thunder with the
/// delay that is left. Anything else (entering, a seek, a scrub, reverse, a pause) cancels what is pending; when
/// play goes on from there, the thunder still on its way from strikes before the playhead is queued again with the
/// delay it has left, so a pause holds thunder and a seek lands in the middle of it.
#[derive(Default)]
pub(crate) struct ReplayThunder {
    last: Option<usize>,
    playing: bool,
}
impl ReplayThunder {
    /// `time(i)` is recorded frame i's time, `strikes` the recording's strikes with their frames' times
    /// (`Ring::strikes`); `now` the playhead's time; `speed` the playback speed.
    pub(crate) fn step(
        &mut self,
        index: usize,
        now: f64,
        playing: bool,
        speed: f32,
        time: impl Fn(usize) -> Option<f64>,
        strikes: impl Iterator<Item = (f64, LightningStrike)>,
    ) -> Step {
        let mut step = Step::default();
        let last = self.last.replace(index);
        let was_playing = std::mem::replace(&mut self.playing, playing);
        // Strikes after `from` (all of them for None) up to the playhead's frame are queued.
        let from = match last {
            Some(l) if playing && was_playing && index >= l && index - l <= MAX_STEP => {
                if index == l {
                    return step;
                }
                Some(time(l).unwrap_or(f64::INFINITY))
            }
            Some(l) if !playing && !was_playing && index == l => return step,
            _ => {
                step.cancel = true;
                if !playing {
                    return step;
                }
                None
            }
        };
        let Some(to) = time(index) else { return step };
        for (t, strike) in strikes.filter(|&(t, _)| t <= to && from.is_none_or(|f| t > f)) {
            // The thunder is already on its way: only what is left of its delay, at the playback speed.
            let left = (delay_s(strike.distance_m) - (now - t) as f32) / speed.max(0.1);
            if left >= 0. {
                step.strikes.push((thunder_of(strike), left));
            }
        }
        step
    }

    /// Back to live play; true when a replay was being followed (its pending thunder must not carry into live play).
    pub(crate) fn reset(&mut self) -> bool {
        self.playing = false;
        self.last.take().is_some()
    }
}

/// One 60 Hz tick of the export's weather.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WeatherTick {
    pub params: Params,
    pub cancel: bool,
    pub strikes: [Option<(Thunder, f32)>; 2],
}

/// The export's weather mixer, stepped in lockstep with the export's game mix (replay/export.rs `Mix`): each tick's
/// parameters land at the tick's sample (`tick_samples` apart), each video frame takes `frame_samples`.
pub(crate) struct OfflineWeather {
    mixer: Mixer,
    tick: u64,
    rendered: u64,
    tick_samples: u64,
    frame_samples: u64,
    buffer: Vec<f32>,
}
impl OfflineWeather {
    /// None without recordings (no weather in the export).
    pub(crate) fn new(bank: Arc<Bank>, tick_samples: u64, frame_samples: u64) -> Option<Self> {
        (!bank.is_empty()).then(|| Self {
            // The same seed every export: the mixer is deterministic, so an export renders the same twice.
            mixer: Mixer::new(bank, 0x5EED),
            tick: 0,
            rendered: 0,
            tick_samples,
            frame_samples,
            buffer: Vec::new(),
        })
    }

    fn fill(&mut self, until: u64) {
        if until > self.rendered {
            let start = self.buffer.len();
            self.buffer.resize(start + (until - self.rendered) as usize * 2, 0.);
            self.mixer.render(&mut self.buffer[start..]);
            self.rendered = until;
        }
    }

    /// One video frame's weather, interleaved stereo, `frame_samples` long.
    pub(crate) fn frame(&mut self, ticks: &[WeatherTick]) -> Vec<f32> {
        let delivered = self.rendered - self.buffer.len() as u64 / 2;
        for tick in ticks {
            self.fill(self.tick * self.tick_samples);
            if tick.cancel {
                self.mixer.cancel_pending();
            }
            self.mixer.set(tick.params);
            for &(thunder, delay) in tick.strikes.iter().flatten() {
                self.mixer.strike(thunder, delay);
            }
            self.tick += 1;
        }
        self.fill(delivered + self.frame_samples);
        self.buffer.drain(..self.frame_samples as usize * 2).collect()
    }
}

/// The export's weather tick at playhead `fraction`: the recorded rain on open hard ground (the census is not re-run
/// offline) and the thunder its own [`ReplayThunder`] fires, always moving forward at `speed`.
pub(crate) fn export_tick(replay: &crate::replay::Replay, fraction: f32, speed: f32, thunder: &mut ReplayThunder) -> WeatherTick {
    let (index, now) = replay.index_at(fraction);
    let rain = replay.weather_at(index).map_or(0., |w| w.1);
    let ring = replay.frames();
    let step = thunder.step(index, now, true, speed, |i| ring.time(i), ring.strikes());
    let hard = [[1., 1.], [0.; 2], [0.; 2], [0.; 2], [0.; 2]];
    let mut tick = WeatherTick {
        params: Params { rain, surfaces: super::census::water_boost(hard, rain), master: 1., ..Default::default() },
        cancel: step.cancel,
        strikes: [None; 2],
    };
    for (slot, strike) in tick.strikes.iter_mut().zip(step.strikes) {
        *slot = Some(strike);
    }
    tick
}
