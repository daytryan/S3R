//! GroundAnimation's Toolkit_CalcGroundJump, original TU3 82D93618.
//! Stock graph values and live animation/physical inputs determine launch.
mod math;
mod types;
use crate::{
    air::state::{AirMath, PhysicsAirMath},
    trigonometry::acos,
};
use math::*;
pub use types::{GroundJump, GroundJumpInput, GroundJumpMode, GroundJumpSettings};

pub fn calculate(
    input: GroundJumpInput,
    mode: GroundJumpMode,
    s: &GroundJumpSettings,
) -> GroundJump {
    //82D93678 the only gate: flags_2468 & 0x00400000. Nothing in 82D93618-82D93DEC reads +2480 (D3).
    if input.flags_2468 & 0x0040_0000 == 0 {
        return GroundJump::default();
    }
    let normal = input.filtered_ground_normal;
    let y_scalar = s.y_scalar_vs_normal_y.evaluate(normal[1]);
    let current = planar(input.current_velocity, normal);
    let mut prepared = planar(input.prepared_velocity, normal);
    if dot(sub(current, prepared), sub(current, prepared)) > 7.0 {
        let agreement = vector_clamp(dot(normalize(current), normalize(prepared)), 0.0, 1.0);
        let fraction = acos(agreement) * reciprocal(f32::from_bits(0x4049_0fdb));
        let bound = length(prepared) * s.speed_scalar_vs_angle.evaluate(fraction);
        prepared = AirMath.clamp_vector_within_max_length(current, bound);
    }
    let mut velocity = if input.flags_2488 & 0x1000_0000 != 0 {
        let mut value = madd(input.effective_forward, 2.3, prepared);
        value[1] += 1.0;
        value
    } else {
        let minimum = if input.flags_2484 & 0x800 != 0 {
            mode.minimum_height_64
        } else {
            mode.minimum_height_68
        };
        let speed_fraction = clamp(input.surface_speed / s.speed_response_max_speed, 0.0, 1.0);
        let low = s.minimum_height_vs_speed.evaluate(speed_fraction).mul_add(
            minimum - s.absolute_minimum_height,
            s.absolute_minimum_height,
        );
        let high = s.maximum_height_vs_speed.evaluate(speed_fraction).mul_add(
            mode.maximum_height - s.absolute_minimum_height,
            s.absolute_minimum_height,
        );
        let height = low.mul_add(1.0 - input.jump_strength, high * input.jump_strength);
        let current_height = dot(
            sub(
                input.animation_com_position,
                input.ground_reference_position,
            ),
            input.reference_up,
        );
        let remaining = maximum(height - current_height, 0.0);
        let square = (remaining * input.gravity_y) * -2.0;
        let speed = square_root(square);
        let angle = acos(vector_clamp(normal[1], -1.0, 1.0));
        let fraction = clamp(angle * f32::from_bits(0x3f22_f983), 0.0, 1.0);
        let response = maximum(s.vertical_response.evaluate(fraction), s.minimum_scalar);
        madd(normal, speed * response, prepared)
    };
    let bonus = clamp((y_scalar - 1.0) * velocity[1], 0.0, s.maximum_y_bonus);
    velocity[1] += bonus;
    let projected = dot(velocity, normal);
    let side = normalize(cross(UP, input.forward));
    let x = -((input.jump_controls[0] * s.adjust_x_factor) * projected);
    let z = (input.jump_controls[1] * s.adjust_z_factor) * projected;
    let correction = madd(side, x, scale(cross(side, UP), z));
    GroundJump {
        velocity: add(velocity, correction),
        scalar_16: 0.0,
        active: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point_graph::PointGraph;

    /// D3: 82D93678-84 `lwz r8,2468(r5); rlwinm r7,r8,0,9,9; beq 0x82D93DCC` is the only gate, and
    /// nothing in 82D93618-82D93DEC reads +2480 (the old port also launched on +2480 & 0x1000).
    #[test]
    fn only_flags_2468_bit_22_passes_the_82d93618_gate() {
        let g8 = PointGraph { x: [0.0; 8], y: [1.0; 8] };
        let settings = GroundJumpSettings {
            vertical_response: PointGraph { x: [0.0; 16], y: [1.0; 16] },
            y_scalar_vs_normal_y: g8,
            speed_scalar_vs_angle: g8,
            minimum_height_vs_speed: g8,
            maximum_height_vs_speed: g8,
            speed_response_max_speed: 10.0,
            minimum_scalar: 0.5,
            maximum_y_bonus: 1.0,
            adjust_z_factor: 0.0,
            adjust_x_factor: 0.0,
            absolute_minimum_height: 0.1,
        };
        let up = [0.0, 1.0, 0.0, 0.0];
        let input = GroundJumpInput {
            flags_2468: 0,
            flags_2484: 0,
            flags_2488: 0,
            effective_forward: [0.0, 0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0, 0.0],
            current_velocity: [0.0, 0.0, 3.0, 0.0],
            ground_reference_position: [0.0; 4],
            reference_up: up,
            filtered_ground_normal: up,
            animation_com_position: [0.0; 4],
            prepared_velocity: [0.0, 0.0, 3.0, 0.0],
            jump_strength: 1.0,
            jump_controls: [0.0; 2],
            gravity_y: -9.81,
            surface_speed: 3.0,
        };
        let mode = GroundJumpMode { minimum_height_64: 0.2, minimum_height_68: 0.2, maximum_height: 0.6 };
        assert!(!calculate(input, mode, &settings).active);
        let input = GroundJumpInput { flags_2468: 0x0040_0000, ..input };
        assert!(calculate(input, mode, &settings).active);
    }
}
