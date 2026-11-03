//! U6: the hiss of wheels rolling through a film of water. No CC0 recording exists (research §1.10), so it is
//! generated: band-passed noise (1.5-7 kHz) for the sizzle plus a low spray body (under 700 Hz) that flutters at
//! 6-14 Hz, louder with speed. Only in rain; the driver zeroes the speed off the ground or under cover.
use super::mixer::{Biquad, Rng, RATE};

/// Calibrated by the previews: at 8 m/s about level with the light-rain bed (build-progress, Task 5).
const HISS_GAIN: f32 = 0.06;

pub(crate) struct Hiss {
    rng: Rng,
    /// Per side: high-pass 1.5 kHz, low-pass 7 kHz, spray low-pass 700 Hz.
    filters: [[Biquad; 3]; 2],
    flutter: f32,
    flutter_target: f32,
    flutter_wait: f32,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

impl Hiss {
    pub(crate) fn new(seed: u64) -> Self {
        let side = [Biquad::high_pass(1500.), Biquad::low_pass(7000.), Biquad::low_pass(700.)];
        Self { rng: Rng::new(seed), filters: [side; 2], flutter: 0., flutter_target: 0., flutter_wait: 0. }
    }

    /// Adds the hiss for `speed` (m/s) in `rain` (0..1) into interleaved stereo `out`.
    pub(crate) fn mix_into(&mut self, out: &mut [f32], speed: f32, rain: f32) {
        let gain = HISS_GAIN * smoothstep(0.3, 8., speed) * (rain * 2.).clamp(0., 1.);
        if !(gain > 0.) {
            return;
        }
        let smooth = 1. - (-std::f32::consts::TAU * 10. / RATE as f32).exp();
        for frame in out.chunks_exact_mut(2) {
            self.flutter_wait -= 1.;
            if self.flutter_wait <= 0. {
                self.flutter_target = self.rng.next();
                self.flutter_wait = RATE as f32 / (6. + 8. * self.rng.next());
            }
            self.flutter += (self.flutter_target - self.flutter) * smooth;
            let body = 0.5 + 0.5 * self.flutter;
            let shared = self.rng.next() * 2. - 1.;
            for (sample, [hp, lp, spray]) in frame.iter_mut().zip(&mut self.filters) {
                // 80% shared between the sides, power kept.
                let noise = 0.894 * shared + 0.447 * (self.rng.next() * 2. - 1.);
                let hiss = lp.run(hp.run(noise));
                *sample += gain * (0.7 * hiss + 0.3 * spray.run(noise) * body);
            }
        }
    }
}
