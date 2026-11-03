//! Shared TU3 constraint iteration (`0x82AE27D0`).
//!
//! Constraint families must exchange reactions on every pass. Solving each
//! family to completion independently changes the physical result.
use super::{
    contact_solver::RetailContactJacobian, drive_solver::RetailDriveRows,
    joint_solver::RetailJointJacobian, rigid_body::RetailReactionCorrections,
};
mod contact;
pub mod contact_build;
mod drive;
mod joint;
pub mod packed;
mod packing;

/// The single-rounding multiply-add the kernels compute with. Their FMA build uses [`Native`], one instruction; a CPU
/// without FMA (Intel before Haswell, AMD before Piledriver) uses [`Emulated`], since `mul_add` there is the C
/// runtime's software `fmaf`, about 250,000 calls a physics tick (user 10-25: 12-15 FPS on a Xeon E5-2670 v2).
pub(crate) trait Fused {
    fn fma(a: f32, b: f32, c: f32) -> f32;
}

/// `f32::mul_add`: an FMA instruction where the code is compiled with FMA, else the C runtime's `fmaf`.
pub(crate) struct Native;
impl Fused for Native {
    #[inline(always)]
    fn fma(a: f32, b: f32, c: f32) -> f32 {
        a.mul_add(b, c)
    }
}

/// [`emulated_fma`]: the same bits as an FMA, in plain double arithmetic.
pub(crate) struct Emulated;
impl Fused for Emulated {
    #[inline(always)]
    fn fma(a: f32, b: f32, c: f32) -> f32 {
        emulated_fma(a, b, c)
    }
}

/// `a * b + c` rounded once, without an FMA unit. The product of two singles is exact in a double; the sum is rounded
/// to odd there (TwoSum's exact error picks the direction), and rounding that to single is then the correctly rounded
/// result (Boldo and Melquiond, "Emulation of FMA and correctly rounded sums: proved algorithms using rounding to odd",
/// IEEE TC 2008). Also the game's `fmaf` (skate-game fma.rs) on a CPU without FMA.
#[inline(always)]
pub fn emulated_fma(a: f32, b: f32, c: f32) -> f32 {
    let (p, c) = (f64::from(a) * f64::from(b), f64::from(c));
    let s = p + c;
    // The double sum already rounds to single like the exact one unless it sits on a single midpoint (its 29 low bits
    // 1 then 28 zeros) or below the single normal range, where the midpoints sit elsewhere. Skipping the rest there
    // also skips a coin-flip branch (perf 10-26: 4.0 -> 2.6 ns).
    let bits = s.to_bits();
    if bits & 0x1FFF_FFFF != 0x1000_0000 && bits & 0x7FF0_0000_0000_0000 >= (1023 - 126) << 52 {
        return s as f32;
    }
    let v = s - p;
    let error = (p - (s - v)) + (c - v);
    if error != 0. && s.is_finite() && s.to_bits() & 1 == 0 {
        // Inexact and even: one step toward the exact sum makes it odd.
        f64::from_bits(if (error > 0.) == (s > 0.) { s.to_bits() + 1 } else { s.to_bits() - 1 }) as f32
    } else {
        s as f32
    }
}

/// VMX `vnmsubfp`: `-(a * b - c)`, fused. Unlike `(-a).mul_add(b, c)`, an exact cancellation
/// (or a zero product and a +0 `c`) is -0, which is what every solver site below stores.
#[inline(always)]
fn nmsub<F: Fused>(a: f32, b: f32, c: f32) -> f32 {
    -F::fma(a, b, -c)
}

pub(crate) fn compile_contact(record: &mut [u32; 64], time_step: f32) {
    contact_build::build(record, time_step);
}

pub(crate) fn solve_joint_record(record: &mut [u32], a: &mut [u32], b: &mut [u32]) {
    joint::solve::<Native>(record, a, b);
}

/// Host indices replace the two guest reaction pointers stored in a Jacobian.
#[derive(Clone, Debug)]
pub struct JointConstraint {
    pub jacobian: RetailJointJacobian,
    pub reaction_a: usize,
    pub reaction_b: usize,
}

/// Runs the retail contact â†’ joint â†’ drive ordering. The caller clears the
/// reaction workspace and builds fresh rows once per simulation step.
pub fn solve_constraints(
    contacts: &mut [RetailContactJacobian],
    joints: &mut [JointConstraint],
    drives: &mut [RetailDriveRows],
    reactions: &mut [RetailReactionCorrections],
    iterations: u32,
) {
    let mut c: Vec<_> = contacts.iter().map(packing::contact).collect();
    let mut j: Vec<_> = joints
        .iter()
        .map(|j| packed::Joint {
            words: j.jacobian.words,
            reaction_a: j.reaction_a,
            reaction_b: j.reaction_b,
        })
        .collect();
    let mut d: Vec<_> = drives.iter().map(packing::drive).collect();
    let mut r: Vec<_> = reactions.iter().copied().map(packing::reaction).collect();
    packed::solve(&mut c, &mut j, &mut d, &mut r, iterations);
    for (contact, native) in contacts.iter_mut().zip(c) {
        contact.words = native.words;
    }
    for (joint, native) in joints.iter_mut().zip(j) {
        joint.jacobian.words = native.words;
    }
    for (drive, native) in drives.iter_mut().zip(d) {
        drive.accumulated_linear_impulse = packing::read_xyz(&native.words, 2);
        drive.accumulated_angular_impulse = packing::read_xyz(&native.words, 3);
    }
    for (reaction, native) in reactions.iter_mut().zip(r) {
        *reaction = packing::unpack_reaction(native);
    }
}

#[cfg(test)]
mod tests {
    use super::emulated_fma;

    /// Bits, with every NaN alike (the kernels never feed one; its payload is not claimed).
    fn bits(x: f32) -> u32 {
        if x.is_nan() { u32::MAX } else { x.to_bits() }
    }

    #[test]
    fn the_emulation_rounds_once_like_an_fma() {
        // 1 + 2^-23 plus a product just under the next half-ulp: one rounding keeps 1 + 2^-23. Rounding the double sum
        // (exactly that midpoint) to single picks the even neighbour 1 + 2^-22 instead.
        let (a, b, c) = (f32::from_bits(0x3380_0001), f32::from_bits(0x3F7F_FFFE), f32::from_bits(0x3F80_0001));
        assert_eq!(a.mul_add(b, c).to_bits(), 0x3F80_0001);
        assert_eq!(((f64::from(a) * f64::from(b) + f64::from(c)) as f32).to_bits(), 0x3F80_0002, "the case must matter");
        assert_eq!(emulated_fma(a, b, c).to_bits(), 0x3F80_0001);

        let specials = [
            0., -0., 1., -1., 1.5, f32::MIN_POSITIVE, -f32::MIN_POSITIVE, f32::from_bits(1), f32::from_bits(0x8000_0001),
            f32::from_bits(0x007F_FFFF), f32::MAX, f32::MIN, f32::EPSILON, 1. + f32::EPSILON, 1. - f32::EPSILON / 2.,
            f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 3.0e19, -3.0e-20, 6.0e-39,
        ];
        for &a in &specials {
            for &b in &specials {
                for &c in &specials {
                    assert_eq!(bits(emulated_fma(a, b, c)), bits(a.mul_add(b, c)), "{a:e} * {b:e} + {c:e}");
                }
            }
        }
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for i in 0..2_000_000u32 {
            let r = next();
            // Any bits, then (every other case) operands near one another's scale, where cancellation and ties live.
            let (a, b, c) = if i % 2 == 0 {
                (f32::from_bits(r as u32), f32::from_bits((r >> 32) as u32), f32::from_bits(next() as u32))
            } else {
                let m = |w: u64, e: u64| f32::from_bits(((w & 0x807F_FFFF) as u32) | ((e as u32) << 23));
                let e = 100 + (r >> 56) % 50;
                (m(r, e), m(r >> 20, 127 - (e - 100) % 20), m(next(), e))
            };
            assert_eq!(bits(emulated_fma(a, b, c)), bits(a.mul_add(b, c)), "{a:e} * {b:e} + {c:e}");
        }
        // The fast path's edges: sums a few units from a single midpoint, and results below the normal range.
        for _ in 0..500_000 {
            let (r, s) = (next(), next());
            let a = f32::from_bits(0x3F80_0000 | (r as u32 >> 9));
            let b = f32::from_bits(((r >> 32) as u32 & 0x0F7F_FFFF) + 0x3000_0000);
            let p = f64::from(a) * f64::from(b);
            let midpoint = f64::from(p as f32) + f64::from((p as f32) * f32::EPSILON) * 0.5;
            let c = f32::from_bits(((midpoint - p) as f32).to_bits().wrapping_add(s as u32 % 3));
            assert_eq!(bits(emulated_fma(a, b, c)), bits(a.mul_add(b, c)), "{a:e} * {b:e} + {c:e}");
            let tiny = f32::from_bits(0x0080_0000 + (s as u32 >> 8));
            let (b, c) = (f32::from_bits(0x3F00_0000 | ((s >> 32) as u32 >> 9)), -f32::from_bits(0x0080_0000 + (r as u32 >> 9)));
            assert_eq!(bits(emulated_fma(tiny, b, c)), bits(tiny.mul_add(b, c)), "{tiny:e} * {b:e} + {c:e}");
        }
    }
}
