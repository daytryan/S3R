//! Thunder for a strike `distance_m` away (U2: 1-4 km, real speed of sound): which recordings, how dark, how loud
//! and where. The mixer's thunder voice (mixer.rs) applies these when the sound arrives.
use std::f32::consts::{FRAC_PI_4, PI, TAU};

pub(crate) const SPEED_OF_SOUND: f32 = 343.0;
pub(crate) const MIN_KM: f32 = 1.0;
pub(crate) const MAX_KM: f32 = 4.0;

pub(crate) fn delay_s(distance_m: f32) -> f32 {
    distance_m / SPEED_OF_SOUND
}

pub(crate) fn bin(distance_m: f32) -> &'static str {
    if distance_m < 1500. {
        "thunder_crack"
    } else if distance_m < 3000. {
        "thunder_roll"
    } else {
        "thunder_rumble"
    }
}

/// Air absorption as a low-pass cutoff (ISO 9613-1 fit, research §2.1).
pub(crate) fn cutoff_hz(distance_m: f32) -> f32 {
    (4000. * (1000. / distance_m).powf(0.8)).clamp(600., 14000.)
}

/// Spreading loss at half slope (games compress it): 0 dB at 1 km, -6 dB at 4 km.
pub(crate) fn gain_db(distance_m: f32) -> f32 {
    -20. * (distance_m / 1000.).log10() * 0.5
}

/// Constant-power pan from the strike's world azimuth relative to the camera yaw (both `atan2(z, x)` of a
/// direction, so a positive difference is to the right); rumbles are 50% wider (blend towards centre).
/// Returns [left, right].
pub(crate) fn pan(azimuth: f32, camera_yaw: f32, distance_m: f32) -> [f32; 2] {
    let relative = (azimuth - camera_yaw + PI).rem_euclid(TAU) - PI;
    let width = if bin(distance_m) == "thunder_rumble" { 0.5 } else { 1. };
    let angle = (relative.sin() * width + 1.) * FRAC_PI_4;
    [angle.cos(), angle.sin()]
}

/// A well-mixed 0..1 from the strike seed and a salt, so each choice the seed makes is independent.
pub(crate) fn unit(seed: u32, salt: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA77);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Picks a clip index in `count`, never equal to `last` when count > 1.
pub(crate) fn pick(seed: u32, count: usize, last: Option<usize>) -> usize {
    if count <= 1 {
        return 0;
    }
    match last.filter(|&l| l < count) {
        Some(l) => {
            let i = (unit(seed, 1) * (count - 1) as f32) as usize % (count - 1);
            if i >= l { i + 1 } else { i }
        }
        None => (unit(seed, 1) * count as f32) as usize % count,
    }
}

/// The voice's pitch, ±6% from the seed (a resample: thunder has no pitch reference).
pub(crate) fn pitch(seed: u32) -> f32 {
    1. + 0.06 * (2. * unit(seed, 2) - 1.)
}
