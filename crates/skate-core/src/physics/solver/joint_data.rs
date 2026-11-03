//! Decoded joint/drive geometry and shared reaction application.
//!
//! Record boundaries are four-float columns because native solver stores also
//! update their fourth lanes. Physical vectors/impulses have three components;
//! the fourth values are packed carry fields, not extra spatial dimensions.

use crate::math::Vector3;
use crate::physics::solver::{Fused, nmsub};

#[inline(always)]
pub(in crate::physics::solver) fn read_column(words: &[u32], byte_offset: usize) -> [f32; 4] {
    core::array::from_fn(|component| f32::from_bits(words[byte_offset / 4 + component]))
}

#[inline(always)]
pub(in crate::physics::solver) fn write_column(
    words: &mut [u32],
    byte_offset: usize,
    values: [f32; 4],
) {
    for (destination, value) in words[byte_offset / 4..byte_offset / 4 + 4]
        .iter_mut()
        .zip(values)
    {
        *destination = value.to_bits();
    }
}

#[inline(always)]
pub(in crate::physics::solver) fn xyz(values: [f32; 4]) -> Vector3 {
    Vector3::new(values[0], values[1], values[2])
}

/// `82AE1AE8` and `82AE3BC8` write these same response fields. Columns at
/// 64/96/128 and 80/112/144 project relative corrections into constraint axes;
/// 192..224 and 240..272 convert impulse changes back into world space.
pub(in crate::physics::solver) struct ConstraintGeometry {
    pub arm_a: Vector3,
    pub arm_b: Vector3,
    pub linear_projection: [[f32; 4]; 3],
    pub angular_projection: [[f32; 4]; 3],
    linear_axes: [[f32; 4]; 3],
    angular_axes: [[f32; 4]; 3],
    inverse_inertia_a: [[f32; 4]; 3],
    inverse_inertia_b: [[f32; 4]; 3],
}

impl ConstraintGeometry {
    #[inline(always)]
    pub fn decode(words: &[u32]) -> Self {
        assert_eq!(words.len(), 96);
        Self {
            arm_a: xyz(read_column(words, 0)),
            arm_b: xyz(read_column(words, 16)),
            linear_projection: core::array::from_fn(|axis| read_column(words, 64 + axis * 32)),
            angular_projection: core::array::from_fn(|axis| read_column(words, 80 + axis * 32)),
            linear_axes: core::array::from_fn(|axis| read_column(words, 192 + axis * 16)),
            angular_axes: core::array::from_fn(|axis| read_column(words, 240 + axis * 16)),
            inverse_inertia_a: core::array::from_fn(|axis| read_column(words, 288 + axis * 16)),
            inverse_inertia_b: core::array::from_fn(|axis| read_column(words, 336 + axis * 16)),
        }
    }

    /// Both candidates must read this incoming snapshot before either impulse
    /// is applied: 82AE2C00..2CC4 (joint), 82AE2E90..2F80 (drive).
    #[inline(always)]
    pub fn relative_corrections<F: Fused>(&self, a: &ReactionState, b: &ReactionState) -> (Vector3, Vector3) {
        let point_a = point_correction::<F>(a, self.arm_a);
        let point_b = point_correction::<F>(b, self.arm_b);
        (
            Vector3::new(
                point_b.x - point_a.x,
                point_b.y - point_a.y,
                point_b.z - point_a.z,
            ),
            Vector3::new(
                b.angular[0] - a.angular[0],
                b.angular[1] - a.angular[1],
                b.angular[2] - a.angular[2],
            ),
        )
    }

    /// 82AE2D24..2E40 / 82AE2FE0..30D4. One linear and one angular impulse
    /// change affect both bodies through their own arm and inverse inertia.
    #[inline(always)]
    pub fn apply_impulse_changes<F: Fused>(
        &self,
        a: &mut ReactionState,
        b: &mut ReactionState,
        linear_change: Vector3,
        angular_change: Vector3,
    ) {
        let linear_impulse = project_correction::<F>(&self.linear_axes, linear_change, [0.0; 4]);
        let angular_impulse = xyz(project_correction::<F>(
            &self.angular_axes,
            angular_change,
            [0.0; 4],
        ));
        let torque_a = torque_at_offset::<F>(self.arm_a, xyz(linear_impulse), angular_impulse);
        let torque_b = torque_at_offset::<F>(self.arm_b, xyz(linear_impulse), angular_impulse);
        let inverse_mass_a = self.inverse_inertia_a[0][3];
        let inverse_mass_b = self.inverse_inertia_b[0][3];
        // Native publication order: linear A, linear B, angular A, angular B.
        // Body B's terms are `vnmsubfp` (82AE2DDC/2E18..2E3C, 82AE3068/30A4..30D0).
        for (component, impulse) in linear_impulse.into_iter().enumerate() {
            a.linear[component] = F::fma(impulse, inverse_mass_a, a.linear[component]);
            b.linear[component] = nmsub::<F>(impulse, inverse_mass_b, b.linear[component]);
        }
        a.angular = project_correction::<F>(&self.inverse_inertia_a, torque_a, a.angular);
        for component in 0..4 {
            let after_x = nmsub::<F>(self.inverse_inertia_b[0][component], torque_b.x, b.angular[component]);
            let after_y = nmsub::<F>(self.inverse_inertia_b[1][component], torque_b.y, after_x);
            b.angular[component] = nmsub::<F>(self.inverse_inertia_b[2][component], torque_b.z, after_y);
        }
    }
}

/// Velocity-producing displacement corrections at reaction offsets 0/32.
/// Contact-only pose corrections at 16/48 are neither read nor overwritten.
pub(in crate::physics::solver) struct ReactionState {
    linear: [f32; 4],
    angular: [f32; 4],
}

impl ReactionState {
    #[inline(always)]
    pub fn decode(words: &[u32]) -> Self {
        assert_eq!(words.len(), 16);
        Self {
            linear: read_column(words, 0),
            angular: read_column(words, 32),
        }
    }
}

#[inline(always)]
pub(in crate::physics::solver) fn publish_reactions(
    a: &ReactionState,
    b: &ReactionState,
    words_a: &mut [u32],
    words_b: &mut [u32],
) {
    write_column(words_a, 0, a.linear);
    write_column(words_b, 0, b.linear);
    write_column(words_a, 32, a.angular);
    write_column(words_b, 32, b.angular);
}

/// Matrix projection with the native X, Y, Z fused accumulation order. The
/// incoming accumulated impulse is the initial value for candidate projection.
#[inline(always)]
pub(in crate::physics::solver) fn project_correction<F: Fused>(
    columns: &[[f32; 4]; 3],
    correction: Vector3,
    initial: [f32; 4],
) -> [f32; 4] {
    core::array::from_fn(|component| {
        let after_x = F::fma(columns[0][component], correction.x, initial[component]);
        let after_y = F::fma(columns[1][component], correction.y, after_x);
        F::fma(columns[2][component], correction.z, after_y)
    })
}

/// 82AE2C2C..2C98 / 82AE2EDC..2F4C: `vmaddfp` then `vnmsubfp`.
#[inline(always)]
fn point_correction<F: Fused>(reaction: &ReactionState, arm: Vector3) -> Vector3 {
    let [x, y, z, _] = reaction.angular;
    Vector3::new(
        nmsub::<F>(z, arm.y, F::fma(y, arm.z, reaction.linear[0])),
        nmsub::<F>(x, arm.z, F::fma(z, arm.x, reaction.linear[1])),
        nmsub::<F>(y, arm.x, F::fma(x, arm.y, reaction.linear[2])),
    )
}

/// 82AE2DE0..2DF8 / 82AE3070..3088: `vmaddfp` then `vnmsubfp`.
#[inline(always)]
fn torque_at_offset<F: Fused>(arm: Vector3, impulse: Vector3, angular_impulse: Vector3) -> Vector3 {
    Vector3::new(
        nmsub::<F>(arm.z, impulse.y, F::fma(arm.y, impulse.z, angular_impulse.x)),
        nmsub::<F>(arm.x, impulse.z, F::fma(arm.z, impulse.x, angular_impulse.y)),
        nmsub::<F>(arm.y, impulse.x, F::fma(arm.x, impulse.y, angular_impulse.z)),
    )
}
