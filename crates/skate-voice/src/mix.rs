//! The voices into one output frame.

/// Above this a sum bends smoothly toward full scale instead of clipping (a hard clip crackles on a shout or two loud
/// talkers at once).
const KNEE: f32 = 0.5;

/// `out` becomes the sum of `voices` (samples × gain), soft-clipped to ±1.
pub fn mix(voices: &[(&[f32], f32)], out: &mut [f32]) {
    out.fill(0.);
    for (samples, gain) in voices {
        out.iter_mut().zip(samples.iter()).for_each(|(o, s)| *o += s * gain);
    }
    for o in out {
        let x = o.abs();
        if x > KNEE {
            *o = o.signum() * (KNEE + (1. - KNEE) * ((x - KNEE) / (1. - KNEE)).tanh());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_voices_add_and_loud_ones_stay_inside_full_scale() {
        let mut out = [9f32; 4];
        mix(&[(&[0.1; 4], 2.), (&[0.05; 4], 1.)], &mut out);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 1e-6), "summed with gain: {out:?}");
        mix(&[(&[0.9, -0.9, 0.3, 0.], 1.), (&[0.9, -0.9, 0., 0.], 1.)], &mut out);
        assert!(out[0] > 0.9 && out[0] < 1. && out[1] < -0.9 && out[1] > -1., "soft-clipped: {out:?}");
        assert!((out[2] - 0.3).abs() < 1e-6 && out[3] == 0., "quiet samples untouched: {out:?}");
        mix(&[(&[0.2; 2], 1.)], &mut out);
        assert_eq!(out, [0.2, 0.2, 0., 0.], "a short voice leaves silence after it");
    }
}
