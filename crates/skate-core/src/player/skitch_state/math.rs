//! VMX helpers in the recovered Skitch operand order. vmaddfp is `mul_add`;
//! estimates take the same two Newton steps as the native code.
use crate::physics::native_arithmetic::{reciprocal_estimate, reciprocal_square_root_estimate};

pub type Vector = [f32; 4];
pub type Frame = [Vector; 4];

pub const IDENTITY: Frame = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 0.0],
];
/// Retail divides by the stored 1/60 (0x820849C8) rather than multiplying.
pub const DT: f32 = f32::from_bits(0x3c88_8889);
/// 0x822F860C, the frame-rate scalar applied to per-frame differences.
pub const RATE: f32 = f32::from_bits(0x426f_ffff);

pub fn dot3(a: Vector, b: Vector) -> f32 {
    crate::physics::native_arithmetic::dot3(a, b)
}
pub fn add(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] + b[i])
}
pub fn sub(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] - b[i])
}
pub fn scale(a: Vector, k: f32) -> Vector {
    a.map(|v| v * k)
}
pub fn mul(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] * b[i])
}
/// vmaddfp a*k + b.
pub fn madd(a: Vector, k: f32, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i].mul_add(k, b[i]))
}
pub fn neg(a: Vector) -> Vector {
    a.map(|v| -v)
}
pub fn splat(v: f32) -> Vector {
    [v; 4]
}
/// vrsqrtefp and two Newton steps with 0.5/1.0 splats.
pub fn rsqrt(x: f32) -> f32 {
    let mut r = reciprocal_square_root_estimate(x);
    for _ in 0..2 {
        r = (r * 0.5).mul_add((-x).mul_add(r * r, 1.0), r);
    }
    r
}
/// vrefp and two Newton steps.
pub fn recip(x: f32) -> f32 {
    let mut r = reciprocal_estimate(x);
    for _ in 0..2 {
        r = r.mul_add((-r).mul_add(x, 1.0), r);
    }
    r
}
/// XMVector3Length: exact zero for a zero square, otherwise len2 * rsqrt.
pub fn length(v: Vector) -> f32 {
    let d = dot3(v, v);
    if d == 0.0 { 0.0 } else { d * rsqrt(d) }
}
/// Unguarded normalisation, all four lanes scaled.
pub fn normalize(v: Vector) -> Vector {
    scale(v, rsqrt(dot3(v, v)))
}
/// v - n * dot3(v, n), as separate multiply and subtract.
pub fn reject(v: Vector, n: Vector) -> Vector {
    sub(v, scale(n, dot3(v, n)))
}
/// Row-vector point transform: ((row0*x + row3) + row1*y) + row2*z.
pub fn transform(f: &Frame, p: Vector) -> Vector {
    madd(f[2], p[2], madd(f[1], p[1], madd(f[0], p[0], f[3])))
}
/// d x n in the vnmsubfp/vpermwi yzxw form of 82D48148.
pub fn cross(d: Vector, n: Vector) -> Vector {
    let r = |i: usize| {
        let j = (i + 1) % 3;
        (-d[j]).mul_add(n[i], d[i] * n[j])
    };
    [r(1), r(2), r(0), (-d[3]).mul_add(n[3], d[3] * n[3])]
}
/// inverse(a) * b for rigid `a`: transposed 3x3 and t = -(p.x c0 + p.y c1 + p.z c2),
/// in the 82D48148 vmrghw/vmaddfp order.
pub fn rigid_inverse_mul(a: &Frame, b: &Frame) -> Frame {
    let col = |c: usize| [a[0][c], a[1][c], a[2][c], 0.0];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let p = sub([0.0; 4], a[3]);
    let t = madd(c0, p[0], madd(c1, p[1], scale(c2, p[2])));
    let row = |r: Vector| madd(b[2], r[2], madd(b[1], r[1], scale(b[0], r[0])));
    [
        row(c0),
        row(c1),
        row(c2),
        madd(b[2], t[2], madd(b[1], t[1], madd(b[0], t[0], b[3]))),
    ]
}
/// fsel(test, a, b): `a` when test >= -0.0 (NaN selects `b`).
pub fn fsel(test: f32, a: f32, b: f32) -> f32 {
    if test >= -0.0 { a } else { b }
}
/// fsel max(x, lo) then min(.., hi), the common clamp pair.
pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    let m = fsel(lo - x, lo, x);
    fsel(hi - m, m, hi)
}
/// a != 0 && b != 0 && (a > 0) == (b > 0).
pub fn same_sign(a: f32, b: f32) -> bool {
    a != 0.0 && b != 0.0 && (a > 0.0) == (b > 0.0)
}
/// a != 0 && b != 0 && (a > 0) != (b > 0).
pub fn opposite_sign(a: f32, b: f32) -> bool {
    a != 0.0 && b != 0.0 && (a > 0.0) != (b > 0.0)
}
/// x mapped to (-pi, pi] via the 0x82139A60/0x82139A50 vrfim form.
pub use crate::player::wipeout_state::math::wrap_angle as wrap;
