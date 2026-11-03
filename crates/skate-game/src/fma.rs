//! The C runtime's `fmaf`, replaced for the whole program. Every `f32::mul_add` outside an FMA-enabled function calls
//! it, because the x86-64 baseline has no FMA (the vendored Skate 2 port alone has 2051 of them). Perf 10-26, per call:
//! the release build's static CRT takes ~1 ns with FMA3 but ~21 ns without (Ivy Bridge and older, Pentium/Celeron),
//! a dev build's DLL ~4 ns either way; this one ~1 ns with FMA3 and ~3 ns without, with the same bits.

use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

/// 0 not yet known, 1 FMA3, 2 emulated: no FMA3, or `SKATE_NO_FMA` set (as skate-core's solver dispatch).
static MODE: AtomicU8 = AtomicU8::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn fmaf(a: f32, b: f32, c: f32) -> f32 {
    // The hot path stays a leaf: with the detection inlined, saving XMM registers doubled its cost.
    if MODE.load(Relaxed) == 1 { hardware(a, b, c) } else { slow(a, b, c) }
}

#[inline(always)]
fn hardware(a: f32, b: f32, c: f32) -> f32 {
    let mut x = a;
    // SAFETY: reached only once MODE says the CPU has FMA3. `vfmadd213ss x, b, c` is b * x + c rounded once; assembly,
    // because a `mul_add` here would call this function again.
    unsafe {
        std::arch::asm!("vfmadd213ss {x}, {b}, {c}", x = inout(xmm_reg) x, b = in(xmm_reg) b, c = in(xmm_reg) c,
            options(pure, nomem, nostack));
    }
    x
}

#[cold]
#[inline(never)]
fn slow(a: f32, b: f32, c: f32) -> f32 {
    if MODE.load(Relaxed) == 0 {
        let fma = std::is_x86_feature_detected!("fma") && std::env::var_os("SKATE_NO_FMA").is_none();
        MODE.store(if fma { 1 } else { 2 }, Relaxed);
    }
    if MODE.load(Relaxed) == 1 { hardware(a, b, c) } else { skate_core::physics::solver::emulated_fma(a, b, c) }
}

#[cfg(test)]
mod tests {
    use skate_core::physics::solver::emulated_fma;

    /// `f32::mul_add` reaches this `fmaf` and gets one rounding: skate-core's solver tests hold the emulation to the
    /// CRT's FMA, this holds the override to the emulation, and to the instruction when the CPU has it.
    #[test]
    fn mul_add_rounds_once_through_the_override() {
        let same = |x: f32, y: f32| x.to_bits() == y.to_bits() || x.is_nan() && y.is_nan();
        let mut seed = 0x9E37_79B9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f32::from_bits(seed)
        };
        // A naive double sum rounds this one wrong (skate-core's the_emulation_rounds_once_like_an_fma).
        let mut cases = vec![(f32::from_bits(0x3380_0001), f32::from_bits(0x3F7F_FFFE), f32::from_bits(0x3F80_0001))];
        cases.extend((0..200_000).map(|_| (next(), next(), next())));
        for (a, b, c) in cases {
            let want = emulated_fma(a, b, c);
            assert!(same(std::hint::black_box(a).mul_add(b, c), want), "{a:e} * {b:e} + {c:e}");
            if std::is_x86_feature_detected!("fma") {
                assert!(same(super::hardware(a, b, c), want), "{a:e} * {b:e} + {c:e}");
            }
        }
        assert_eq!(super::fmaf(2., 3., 1.), 7.);
    }
}
