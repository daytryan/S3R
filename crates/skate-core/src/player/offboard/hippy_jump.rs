//! TU3 `Skateboard::Toolkit_CalcHippyJumpVel` at `0x82D79F00`.
//!
//! The helper constructs the rider trajectory velocity used by
//! `PhysState_LandingOnDeck::InitTraj`; it does not write board-body velocity.

pub type Vector = [f32; 4];

#[derive(Clone, Copy, Debug)]
pub struct Input {
    pub desired_height: f32,
    pub board_position: Vector,
    pub centre_of_mass_position: Vector,
    pub reference_up: Vector,
    pub current_velocity: Vector,
}

pub fn calculate(input: Input) -> Vector {
    let up = input.reference_up;
    let displacement = sub(input.centre_of_mass_position, input.board_position);
    let current_height = dot3(up, displacement);
    let vertical_velocity = dot3(up, input.current_velocity);
    let planar_velocity = sub(input.current_velocity, scale(up, vertical_velocity));
    let launch_speed = launch_speed((input.desired_height - current_height) * 19.6);
    //82D7A008 one fused `vmaddfp up * s + planar` (D4).
    core::array::from_fn(|i| up[i].mul_add(launch_speed, planar_velocity[i]))
}

///82D79FAC-82D79FE4: `vrsqrtefp`, two `vnmsubfp`/`vmaddfp` Newton steps, `x * r`, then `vsel`
/// exactly 0 where `x == 0`. The seed is host math (D11).
fn launch_speed(x: f32) -> f32 {
    let mut r = crate::physics::reciprocal_sqrt::estimate(x);
    for _ in 0..2 {
        r = (r * 0.5).mul_add(-(x.mul_add(r * r, -1.0)), r);
    }
    if x == 0.0 { 0.0 } else { x * r }
}

fn dot3(a: Vector, b: Vector) -> f32 {
    a[0].mul_add(b[0], a[1].mul_add(b[1], a[2] * b[2]))
}
fn sub(a: Vector, b: Vector) -> Vector {
    core::array::from_fn(|i| a[i] - b[i])
}
fn scale(v: Vector, scalar: f32) -> Vector {
    core::array::from_fn(|i| v[i] * scalar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_planar_velocity_and_replaces_up_component() {
        let output = calculate(Input {
            desired_height: 1.5,
            board_position: [0.0, 0.0, 0.0, 0.0],
            centre_of_mass_position: [0.0, 0.5, 0.0, 0.0],
            reference_up: [0.0, 1.0, 0.0, 0.0],
            current_velocity: [3.0, -2.0, 4.0, 0.0],
        });
        assert_eq!(output[0], 3.0);
        assert_eq!(output[2], 4.0);
        assert!((output[1] - 19.6f32.sqrt()).abs() < 1.0e-6);
    }

    /// D4: 82D79FAC-82D79FE4 is `vrsqrtefp`, two `vnmsubfp`/`vmaddfp` Newton steps, `x * r` and a
    /// `vsel` to exactly 0 when `x == 0`; 82D7A008 is one `vmaddfp up * s + planar`. The estimate
    /// seed is host math (D11) and each VMX op is taken as one IEEE single rounding (section 6).
    #[test]
    fn launch_speed_is_the_82d79f00_refine_and_the_add_is_fused() {
        let retail_speed = |x: f32| {
            let mut r = crate::physics::reciprocal_sqrt::estimate(x);
            for _ in 0..2 {
                r = (r * 0.5).mul_add(-(x.mul_add(r * r, -1.0)), r);
            }
            if x == 0.0 { 0.0 } else { x * r }
        };
        let mut seed = 0x1234_5679u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        for _ in 0..5000 {
            let (a, b) = (next() * 6.283, next() * 1.2);
            let up = [b.sin() * a.cos(), b.cos(), b.sin() * a.sin(), 0.0];
            let input = Input {
                desired_height: 0.3 + next() * 2.0,
                board_position: [next() * 50.0, next() * 5.0, next() * 50.0, 0.0],
                centre_of_mass_position: [next() * 50.0, next() * 5.0, next() * 50.0, 0.0],
                reference_up: up,
                current_velocity: [next() * 20.0 - 10.0, next() * 6.0 - 3.0, next() * 20.0 - 10.0, 0.0],
            };
            let displacement = sub(input.centre_of_mass_position, input.board_position);
            let x = (input.desired_height - dot3(up, displacement)) * 19.6;
            let planar = sub(input.current_velocity, scale(up, dot3(up, input.current_velocity)));
            let s = retail_speed(x);
            let expected: Vector = core::array::from_fn(|i| up[i].mul_add(s, planar[i]));
            let got = calculate(input);
            for i in 0..4 {
                assert!(
                    got[i].to_bits() == expected[i].to_bits() || (got[i].is_nan() && expected[i].is_nan()),
                    "lane {i}: {:e} vs {:e} ({input:?})",
                    got[i],
                    expected[i]
                );
            }
        }
        let flat = Input {
            desired_height: 0.5,
            board_position: [0.0; 4],
            centre_of_mass_position: [0.0, 0.5, 0.0, 0.0],
            reference_up: [0.0, 1.0, 0.0, 0.0],
            current_velocity: [3.0, -2.0, 4.0, 0.0],
        };
        assert_eq!(calculate(flat), [3.0, 0.0, 4.0, 0.0], "x == 0 selects exactly 0");
    }
}
