//! Measurements for the mixer tests, on channel 0 of interleaved stereo.
use super::mixer::RATE;

/// Mean energy above 6 kHz (RBJ 2nd-order high-pass, Q 0.707).
pub(crate) fn hf_energy(buf: &[f32]) -> f32 {
    let w = std::f32::consts::TAU * 6000. / RATE as f32;
    let alpha = w.sin() / (2. * std::f32::consts::FRAC_1_SQRT_2);
    let (cos, a0) = (w.cos(), 1. + alpha);
    let b = [(1. + cos) / 2. / a0, -(1. + cos) / a0, (1. + cos) / 2. / a0];
    let a = [-2. * cos / a0, (1. - alpha) / a0];
    let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
    let mut energy = 0.;
    for x in buf.iter().step_by(2).copied() {
        let y = b[0] * x + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
        (x2, x1, y2, y1) = (x1, x, y1, y);
        energy += y * y;
    }
    energy / (buf.len() / 2).max(1) as f32
}

/// Mean power of the `freq` component (Goertzel): amplitude² / 2.
pub(crate) fn tone_energy(buf: &[f32], freq: f32) -> f32 {
    let coeff = 2. * (std::f32::consts::TAU * freq / RATE as f32).cos();
    let (mut s1, mut s2) = (0f64, 0f64);
    for x in buf.iter().step_by(2) {
        let s = f64::from(*x) + f64::from(coeff) * s1 - s2;
        (s2, s1) = (s1, s);
    }
    let n = (buf.len() / 2).max(1) as f64;
    let power = s1 * s1 + s2 * s2 - f64::from(coeff) * s1 * s2;
    (2. * power / (n * n)) as f32
}
