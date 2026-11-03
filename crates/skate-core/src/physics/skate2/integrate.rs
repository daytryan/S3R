//! The rigid-body integrator: Skate 2's `0x8293DE40`, the per-body `DynamicUpdate` the batch
//! integrator calls (`0x82BC51F4`). Same job as TU3 `0x82AE6590` (`rigid_body.rs`), different code.
//!
//! Field names follow what the code does with them; the native offsets are on each field.

use super::float::{
    V4, fdivs, fmadds, fmuls, insert, negate, permwi, select, splat, vaddfp, vcmpeqfp, vcmpgtfp, vmaddfp,
    vmsum3fp, vmsum4fp, vmulfp, vnmsubfp, vrsqrtefp, vsubfp,
};

/// The rigid body fields the integrator and the force accumulator touch. A `w` lane the code
/// carries through untouched is kept as-is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RigidBody {
    /// `+0`: orientation quaternion `[x, y, z, w]`.
    pub orientation: V4,
    /// `+16`.
    pub position: V4,
    /// `+32`.
    pub linear_velocity: V4,
    /// `+48`.
    pub angular_velocity: V4,
    /// `+64`, `+80`, `+96`: rotation rows rebuilt from the orientation.
    pub rotation: [V4; 3],
    /// `+112 = [I00, I01, I02, inverse mass]`, `+128 = [I22, I11, I12, -]`: the world inverse
    /// inertia, packed symmetric.
    pub inverse_inertia: [V4; 2],
    /// `+144`: linear acceleration accumulator; `w` (`+156`) is the body's energy.
    pub acceleration: V4,
    /// `+160`: angular acceleration accumulator; `w` (`+172`) holds the sleep counter's bits.
    pub angular_acceleration: V4,
}

impl RigidBody {
    pub fn inverse_mass(&self) -> f32 {
        self.inverse_inertia[0][3]
    }
    pub fn energy(&self) -> f32 {
        self.acceleration[3]
    }
    pub fn sleep_counter(&self) -> u32 {
        self.angular_acceleration[3].to_bits()
    }
}

/// The simulation fields the integrator reads (`*(body+76)`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SimulationStep {
    /// `+144`: gravity, the acceleration every body starts its next tick with.
    pub gravity: V4,
    /// `+160`.
    pub time_step: f32,
    /// `+164`: `1 / time_step`, as stored.
    pub frequency: f32,
    /// `+168`: the sleep counter saturates here.
    pub sleep_limit: u32,
    /// `+172`: energy below which the sleep counter runs.
    pub minimum_energy: f32,
}

/// The body definition fields the integrator reads (`*(body+92)`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyDefinition {
    /// `+0`: local inverse inertia (principal axes) `[x, y, z, -]`.
    pub inverse_inertia: V4,
    /// `+20`: weight of the angular term in the energy.
    pub angular_energy_weight: f32,
    /// `+24`: linear speed cap.
    pub max_linear_speed: f32,
    /// `+28`: angular speed cap.
    pub max_angular_speed: f32,
    /// `+32`: subtracted from the frequency before it turns linear displacement into velocity.
    pub linear_drag: f32,
    /// `+36`: the same for angular.
    pub angular_drag: f32,
}

/// The solver's displacement corrections (`*(body+28)`), consumed and zeroed by the integrator.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corrections {
    /// `+0`: added to the linear displacement (and so to the velocity).
    pub linear: V4,
    /// `+16`: added to the position only.
    pub position: V4,
    /// `+32`: added to the angular displacement (and so to the angular velocity).
    pub angular: V4,
    /// `+48`: added to the rotation only.
    pub orientation: V4,
}

const HALF: V4 = [0.5; 4];
const ONE: V4 = [1.0; 4];
const ZERO: V4 = [0.0; 4];
/// `0x820EFA80`.
const SQRT_2: V4 = [f32::from_bits(0x3FB5_04F3); 4];

/// Two Newton steps on the table estimate: `e += 0.5 e (1 - n e²)`, twice.
fn reciprocal_sqrt(n: V4) -> V4 {
    let mut e = vrsqrtefp(n);
    for _ in 0..2 {
        e = vmaddfp(vmulfp(e, HALF), vnmsubfp(n, vmulfp(e, e), ONE), e);
    }
    e
}

/// `sqrt(ratio)` as the clamp computes it: `ratio * rsqrt(ratio)`, `0` when the ratio is zero.
fn clamp_scale(ratio: f32) -> V4 {
    let n = [ratio; 4];
    select(vcmpeqfp(ZERO, n), vmulfp(n, reciprocal_sqrt(n)), ZERO)
}

/// `0x8293DE40`: one integration step of one body.
pub fn dynamic_update(body: &mut RigidBody, step: &SimulationStep, definition: &BodyDefinition, corrections: &mut Corrections) {
    let dt = [step.time_step; 4];

    // Velocities from the accumulated accelerations, then this tick's displacements.
    let linear_velocity = vmaddfp(body.acceleration, dt, body.linear_velocity);
    let angular_velocity = vmaddfp(body.angular_acceleration, dt, body.angular_velocity);
    let linear_displacement = vmaddfp(linear_velocity, dt, corrections.linear);
    let angular_displacement = vmaddfp(angular_velocity, dt, corrections.angular);

    let moved = vaddfp(vaddfp(body.position, corrections.position), linear_displacement);
    body.position = insert(moved, body.position, 0b0001);

    // Orientation: q += 0.5 [θ q.w + θ × q, -θ·q], renormalised.
    let q = body.orientation;
    let theta = vaddfp(angular_displacement, corrections.orientation);
    let cross = yzx(vnmsubfp(yzx(theta), q, vmulfp(theta, yzx(q))));
    let dq = vmulfp(vmaddfp(theta, splat(q, 3), cross), HALF);
    let dw = vmulfp(vmsum3fp(theta, q), negate(HALF));
    let q = vaddfp(q, insert(dq, dw, 0b0001));
    let q = vmulfp(q, reciprocal_sqrt(vmsum4fp(q, q)));
    body.orientation = q;

    // Rotation rows from the quaternion, with s = √2 q.
    let s = vmulfp(SQRT_2, q);
    let products = vmulfp(s, permwi(s, 96));
    let diagonal = vnmsubfp(s, s, HALF);
    let w_terms = vmulfp(splat(s, 3), permwi(s, 132));
    let sum = vaddfp(products, w_terms);
    let difference = vsubfp(products, w_terms);
    let pairs = vaddfp(diagonal, permwi(diagonal, 96));
    let [row0, row1, row2] = body.rotation;
    let row0 = [pairs[1], sum[0], difference[2], row0[3]];
    let row1 = [difference[0], pairs[2], sum[1], row1[3]];
    let row2 = [sum[2], difference[1], pairs[0], row2[3]];
    body.rotation = [row0, row1, row2];

    // World inverse inertia, Rᵀ diag(I) R, packed as the accumulator reads it.
    let local = definition.inverse_inertia;
    let scaled0 = vmulfp(row0, splat(local, 0));
    let scaled1 = vmulfp(row1, splat(local, 1));
    let scaled2 = vmulfp(row2, splat(local, 2));
    let first = vmaddfp(
        scaled2,
        splat(row2, 0),
        vmaddfp(scaled0, splat(row0, 0), vmulfp(scaled1, splat(row1, 0))),
    );
    let second = vmaddfp(
        permwi(row2, 151),
        permwi(scaled2, 155),
        vmaddfp(permwi(row0, 151), permwi(scaled0, 155), vmulfp(permwi(row1, 151), permwi(scaled1, 155))),
    );
    let [packed_a, packed_b] = body.inverse_inertia;
    body.inverse_inertia = [insert(first, packed_a, 0b0001), insert(second, packed_b, 0b0001)];

    // New velocities: displacement times (frequency - drag), the factor floored at zero.
    let frequency = [step.frequency; 4];
    let angular_factor = vsubfp(frequency, [definition.angular_drag; 4]);
    let linear_factor = vsubfp(frequency, [definition.linear_drag; 4]);
    let mut angular = vmulfp(angular_displacement, select(vcmpgtfp(angular_factor, ZERO), ZERO, angular_factor));
    let mut linear = vmulfp(linear_displacement, select(vcmpgtfp(linear_factor, ZERO), ZERO, linear_factor));

    // Speed caps; the energy uses the capped squares.
    let max_angular_squared = fmuls(definition.max_angular_speed, definition.max_angular_speed);
    let mut angular_squared = vmsum3fp(angular, angular)[0];
    if angular_squared > max_angular_squared {
        angular = vmulfp(angular, clamp_scale(fdivs(max_angular_squared, angular_squared)));
        angular_squared = max_angular_squared;
    }
    let max_linear_squared = fmuls(definition.max_linear_speed, definition.max_linear_speed);
    let mut linear_squared = vmsum3fp(linear, linear)[0];
    if linear_squared > max_linear_squared {
        linear = vmulfp(linear, clamp_scale(fdivs(max_linear_squared, linear_squared)));
        linear_squared = max_linear_squared;
    }

    // Energy and the sleep counter: a quiet body (energy below the minimum and not rising) counts
    // up to the limit; `bge`/`bgt` send a NaN energy to the reset and to the count respectively.
    let weight = fmuls(definition.angular_energy_weight, body.inverse_mass());
    let energy = fmadds(weight, angular_squared, linear_squared);
    let mut counter = 0;
    if energy < step.minimum_energy {
        counter = body.sleep_counter();
        if !(energy > body.energy()) {
            counter = counter.wrapping_add(1);
        }
        counter = counter.min(step.sleep_limit);
    }

    body.linear_velocity = insert(linear, body.linear_velocity, 0b0001);
    body.angular_velocity = insert(angular, body.angular_velocity, 0b0001);
    *corrections = Corrections::default();
    body.acceleration = insert(step.gravity, [energy; 4], 0b0001);
    body.angular_acceleration = [0.0, 0.0, 0.0, f32::from_bits(counter)];
}

/// `vpermwi128 …,99`: `[y, z, x, w]`.
fn yzx(v: V4) -> V4 {
    permwi(v, 0b01_10_00_11)
}
