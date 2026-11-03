//! Skate 2's float instructions, computed the way the oracle computes them.
//!
//! The oracle is `skate-audio-core`'s PowerPC interpreter, which models the ReXGlue recompiler
//! (`docs/vmx128-exactness.md`), not measured Xenon silicon. These helpers reproduce it in plain
//! Rust: no MXCSR changes, no intrinsics, so they give the same bits on any host.
//!
//! - Scalar single ops (`fadds`, `fsubs`, `fmuls`, `fdivs`) compute in `f64` and round once to
//!   `f32`. Operands are not flushed (a single denormal is a normal double), the result is
//!   (flush-to-zero, x86 rule: tiny *after* rounding).
//! - `fmadds` is one `f64` fused multiply-add, then the same rounding to single.
//! - Vector float ops (`vmulfp`, `vaddfp`, `vsubfp`) flush denormal operands to zero and flush tiny
//!   results. `vmaddfp`/`vnmsubfp` are a multiply then an add, two roundings.
//! - `vmsum3fp`/`vmsum4fp` are SSE4.1 `dpps`: lane products rounded, summed in pairs.
//! - `vrsqrtefp` is ReXGlue's integer table, not a host estimate.
//! - A NaN operand comes back quieted (the first NaN operand when both are); an invalid operation
//!   (`inf - inf`, `0 * inf`, `0 / 0`) gives the x86 default NaN `0xFFC00000`. Which payload wins
//!   when *both* operands are NaN is not pinned by the oracle either (vmx.rs "Rule 4").
//!
//! Vectors are `[f32; 4]` in guest element order: `[x, y, z, w]` = the 16 bytes at `ea..ea+16`.

pub type V4 = [f32; 4];

const DEFAULT_NAN: u32 = 0xFFC0_0000;
const QUIET: u32 = 0x0040_0000;

#[inline]
fn quiet(x: f32) -> f32 {
    f32::from_bits(x.to_bits() | QUIET)
}

/// Denormals-are-zero: a subnormal operand reads as a zero of the same sign.
#[inline]
pub fn daz(x: f32) -> f32 {
    if x.is_subnormal() { f32::from_bits(x.to_bits() & 0x8000_0000) } else { x }
}

/// Round a double to single with x86 flush-to-zero: a result that is tiny after rounding to 24
/// bits with an unbounded exponent becomes a zero of its sign. Rounding to the denormal format
/// first would keep a few values just under `f32::MIN_POSITIVE` that the hardware flushes.
#[inline]
pub fn round_single(v: f64) -> f32 {
    if v.is_nan() {
        return f32::from_bits(DEFAULT_NAN);
    }
    let min = f32::MIN_POSITIVE as f64;
    if v != 0.0 && v.abs() < min {
        // 2^64 scaling is exact and lands in the normal range, where `as f32` rounds to 24 bits.
        const SCALE: f64 = 18_446_744_073_709_551_616.0;
        if ((v.abs() * SCALE) as f32) < (min * SCALE) as f32 {
            return if v < 0.0 { -0.0 } else { 0.0 };
        }
    }
    v as f32
}

#[inline]
fn nan_of(a: f32, b: f32) -> Option<f32> {
    if a.is_nan() {
        Some(quiet(a))
    } else if b.is_nan() {
        Some(quiet(b))
    } else {
        None
    }
}

// ------------------------------------------------------------------------------------ scalar

/// `fadds`.
#[inline]
pub fn fadds(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(a as f64 + b as f64))
}

/// `fsubs`.
#[inline]
pub fn fsubs(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(a as f64 - b as f64))
}

/// `fmuls`.
#[inline]
pub fn fmuls(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(a as f64 * b as f64))
}

/// `fdivs`.
#[inline]
pub fn fdivs(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(a as f64 / b as f64))
}

/// `fmadds frD,frA,frC,frB`: `a * c + b`, one `f64` fused multiply-add then rounded to single.
///
/// NaN operands: the oracle's `fmadds` is the host's `f64::mul_add` (the UCRT `fma`), which returns
/// the first NaN of `c`, `a`, `b`, quieted, signalling or not (measured on the host, all 27
/// combinations). Not the PowerPC precedence.
#[inline]
pub fn fmadds(a: f32, c: f32, b: f32) -> f32 {
    if let Some(n) = nan_of(c, a).or_else(|| nan_of(b, b)) {
        return n;
    }
    round_single((a as f64).mul_add(c as f64, b as f64))
}

/// `fsel frD,frA,frC,frB`: `a >= 0 ? c : b`; a NaN selector takes `b`.
#[inline]
pub fn fsel(a: f32, c: f32, b: f32) -> f32 {
    if a >= 0.0 { c } else { b }
}

// ------------------------------------------------------------------------------------ vector

#[inline]
fn lanes(f: impl Fn(usize) -> f32) -> V4 {
    [f(0), f(1), f(2), f(3)]
}

#[inline]
fn vmul1(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(daz(a) as f64 * daz(b) as f64))
}

#[inline]
fn vadd1(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(daz(a) as f64 + daz(b) as f64))
}

#[inline]
fn vsub1(a: f32, b: f32) -> f32 {
    nan_of(a, b).unwrap_or_else(|| round_single(daz(a) as f64 - daz(b) as f64))
}

/// `vmulfp128`.
#[inline]
pub fn vmulfp(a: V4, b: V4) -> V4 {
    lanes(|i| vmul1(a[i], b[i]))
}

/// `vaddfp128`.
#[inline]
pub fn vaddfp(a: V4, b: V4) -> V4 {
    lanes(|i| vadd1(a[i], b[i]))
}

/// `vsubfp128`.
#[inline]
pub fn vsubfp(a: V4, b: V4) -> V4 {
    lanes(|i| vsub1(a[i], b[i]))
}

/// `vmaddfp`: `a * b + c`, product rounded first.
#[inline]
pub fn vmaddfp(a: V4, b: V4, c: V4) -> V4 {
    vaddfp(vmulfp(a, b), c)
}

/// `vnmsubfp`: `c - a * b`, product rounded first.
#[inline]
pub fn vnmsubfp(a: V4, b: V4, c: V4) -> V4 {
    vsubfp(c, vmulfp(a, b))
}

/// `dpps` over the selected lanes, then ReXGlue's `ppc_vmsumfp_result` (overflow or NaN becomes
/// the PowerPC default NaN `0x7FC00000`). SSE lane `k` holds guest element `3 - k`, and `dpps`
/// adds lanes (0 + 1) + (2 + 3), i.e. (w + z) + (y + x), an unselected lane contributing `+0`.
#[inline]
fn dpps(a: V4, b: V4, with_w: bool) -> f32 {
    let w = if with_w { vmul1(a[3], b[3]) } else { 0.0 };
    let sum = vadd1(vadd1(w, vmul1(a[2], b[2])), vadd1(vmul1(a[1], b[1]), vmul1(a[0], b[0])));
    if sum.is_finite() { sum } else { f32::from_bits(0x7FC0_0000) }
}

/// `vmsum3fp128`: x·x' + y·y' + z·z', splatted.
#[inline]
pub fn vmsum3fp(a: V4, b: V4) -> V4 {
    [dpps(a, b, false); 4]
}

/// `vmsum4fp128`: the four-lane dot product, splatted.
#[inline]
pub fn vmsum4fp(a: V4, b: V4) -> V4 {
    [dpps(a, b, true); 4]
}

/// `vrsqrtefp128`: ReXGlue's `ppc_vrsqrtefp_bits` per lane (a base/slope table, integer only).
#[inline]
pub fn vrsqrtefp(a: V4) -> V4 {
    lanes(|i| f32::from_bits(rsqrte_bits(a[i].to_bits())))
}

fn rsqrte_bits(bits: u32) -> u32 {
    const TABLE: [u32; 32] = [
        0x0568B4FD, 0x04F3AF97, 0x048DAAA5, 0x0435A618, 0x03E7A1E4, 0x03A29DFE, 0x03659A5C,
        0x032E96F8, 0x02FC93CA, 0x02D090CE, 0x02A88DFE, 0x02838B57, 0x026188D4, 0x02438673,
        0x02268431, 0x020B820B, 0x03D27FFA, 0x03807C29, 0x033878AA, 0x02F97572, 0x02C27279,
        0x02926FB7, 0x02666D26, 0x023F6AC0, 0x021D6881, 0x01FD6665, 0x01E16468, 0x01C76287,
        0x01AF60C1, 0x01995F12, 0x01855D79, 0x01735BF4,
    ];
    let sign = bits >> 31;
    let biased_exp = (bits >> 23) & 0xFF;
    let mantissa = bits & 0x007F_FFFF;
    if bits == 0xFF80_0000 {
        return 0x7FC0_0000;
    }
    if biased_exp == 0 {
        return if sign != 0 { 0xFF80_0000 } else { 0x7F80_0000 };
    }
    if biased_exp == 0xFF {
        return if mantissa == 0 { 0 } else { bits | 0x0040_0000 };
    }
    if sign != 0 {
        return 0x7FC0_0000;
    }
    let unbiased_exp = biased_exp as i32 - 127;
    let index = ((((unbiased_exp as u32) << 4) & 16) | (mantissa >> 19)) ^ 16;
    let interp = (mantissa >> 9) & 1023;
    let mut result_exp = (127 - biased_exp as i32) >> 1;
    let entry = TABLE[index as usize];
    let slope = entry >> 16;
    let base = (entry << 10) & 0x03FF_FC00;
    let mut raw = (base as i32).wrapping_sub(interp.wrapping_mul(slope) as i32);
    if raw & (1 << 25) == 0 {
        let lz = ((raw as u32) & 0x01FF_FFFF).leading_zeros() as i32;
        result_exp += 6 - lz;
        raw = ((raw as u32) << (((lz - 6) as u32) & 31)) as i32;
    }
    if raw & 5 != 0 && raw & 2 != 0 {
        raw = raw.wrapping_add(4);
    }
    let result = ((result_exp << 23).wrapping_add(0x3F80_0000)) as u32 | (((raw as u32) >> 2) & 0x007F_FFFF);
    if (result >> 23) & 0xFF == 0 && result & 0x007F_FFFF != 0 { 0 } else { result }
}

/// `vcmpgtfp128`: an all-ones lane where `a > b` (denormals compare as zero).
#[inline]
pub fn vcmpgtfp(a: V4, b: V4) -> [bool; 4] {
    [0, 1, 2, 3].map(|i| daz(a[i]) > daz(b[i]))
}

/// `vcmpeqfp128`.
#[inline]
pub fn vcmpeqfp(a: V4, b: V4) -> [bool; 4] {
    [0, 1, 2, 3].map(|i| daz(a[i]) == daz(b[i]))
}

/// `vsel` with a whole-lane mask: `mask ? b : a`.
#[inline]
pub fn select(mask: [bool; 4], a: V4, b: V4) -> V4 {
    lanes(|i| if mask[i] { b[i] } else { a[i] })
}

/// `vspltw`.
#[inline]
pub fn splat(a: V4, i: usize) -> V4 {
    [a[i]; 4]
}

/// `vpermwi128 vD,vB,imm`: element `k` of the result is element `(imm >> (6 - 2k)) & 3` of `b`.
#[inline]
pub fn permwi(b: V4, imm: u32) -> V4 {
    lanes(|k| b[((imm >> (6 - 2 * k as u32)) & 3) as usize])
}

/// `vrlimi128 vD,vB,mask,0` with no rotation: elements whose mask bit (8 = x … 1 = w) is set come
/// from `b`, the rest keep `d`.
#[inline]
pub fn insert(d: V4, b: V4, mask: u32) -> V4 {
    lanes(|i| if mask >> (3 - i) & 1 != 0 { b[i] } else { d[i] })
}

/// `vxor` of a float vector with the sign mask: every lane negated by its bits.
#[inline]
pub fn negate(a: V4) -> V4 {
    a.map(|x| f32::from_bits(x.to_bits() ^ 0x8000_0000))
}
