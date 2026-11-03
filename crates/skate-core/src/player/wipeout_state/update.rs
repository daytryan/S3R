//! The inline arithmetic of WipeoutGround300::Update 82D3B9C8 between its calls; the game's
//! wipeout update (skate-game physics/wipeout_states/update.rs) runs the calls around these.
use super::{State, math::V};

const DT: f32 = f32::from_bits(0x3C88_8889);

/// 82D3BAC4..BB14: an upward spike sends 82BE76B0 a 5 m/s limit. Both tests skip on
/// unordered (`vcmpgtfp` false, `ble` taken), so a NaN rise never limits.
pub fn spikes(previous: V, velocity: V) -> bool {
    velocity[1] > 5.0 && velocity[1] - previous[1] > 5.0
}

/// 82D3BC48..BCA4: the drag 82BE32D0 receives once the contact response ran (not below the
/// special surface): `fmadds settled*0.15 + 0.05` clamped by `fsel` to 0..0.5.
pub fn drag(state: &State, contact: bool) -> f32 {
    if !(state.slow && contact) {
        0.0
    } else if state.over {
        super::math::clamp(state.settled_time.mul_add(0.15, 0.05), 0.0, 0.5)
    } else {
        0.05
    }
}

/// 82D3BCAC..BD28: the clocks, response scalar and peak speed.
pub fn advance_clocks(state: &mut State, speed: f32) {
    state.time += DT;
    state.response_time += DT;
    state.extra_weight_zero_time += DT;
    if state.extra_weight > 0.0 {
        state.extra_weight_zero_time = 0.0;
    }
    state.response_scalar = 0.0;
    state.slow_time += DT;
    state.maximum_speed = if speed - state.maximum_speed >= 0.0 {
        speed
    } else {
        state.maximum_speed
    };
    if speed > 2.0 {
        state.slow_time = 0.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// 82D3BDF8: 82D3DD40 with contact = 1 below the special surface or before surface 12; its
    /// return is ignored, so no control counts as applied.
    Submerged,
    /// 82D3BDE4: 82D3DD40 with the contact argument; its return marks the control applied.
    Air { contact: bool },
    /// 82D3E678.
    Response,
    /// 82D3C3F8, 82D3CB50, then 82D3C928.
    ProfileAir,
    /// 82D3C3F8 then 82D3D660.
    Ground,
}

/// 82D3BD38..BE00. The response gate is `blt` skipping, so a NaN response clock responds.
pub fn control(state: &State, contact: bool, flags_2484: u32) -> Control {
    if state.below_surface || state.imminent_surface_twelve {
        Control::Submerged
    } else if flags_2484 & 0x100 != 0 && !(state.response_time < 0.1) {
        Control::Response
    } else if state.airborne_frames > 15 && !state.over {
        Control::ProfileAir
    } else if (!contact && state.airborne_frames > 15) || state.over {
        Control::Air { contact }
    } else {
        Control::Ground
    }
}
