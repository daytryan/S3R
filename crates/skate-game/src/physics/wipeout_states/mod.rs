//! Original physical WipeoutGround300. Off-board recovery destinations are
//! separate states; this owner retains the native request and timing fields.
mod contact;
mod drive_settings;
mod lifecycle;
mod output;
mod prediction;
pub(crate) mod ragdoll;
mod settings;
mod skeleton;
mod update;
use skate_core::player::wipeout_state::{
    State, contact_response::ContactResponse, drives, profiles::Profile, recovery,
};
use skate_data::collections::Collections;
use std::path::Path;

pub(crate) struct WipeoutState {
    pub(crate) ragdoll: ragdoll::RagdollSetup,
    pub state: State,
    settings: settings::Settings,
    drives: drives::Settings,
    profiles: [Profile; 5],
    contact: ContactResponse,
    prediction: prediction::Prediction,
    /// Skate 1 Trick.DisableWipeoutReset latched for this bail (RETAIL 0x827abe10 sets skater+516 bit 28;
    /// 0x824153a8 copies it to the state each frame and clears it; the bail update 0x8268e220 -> 0x8268e368 latches
    /// it, enter 0x8268e1f0 clears it). While held the timed auto-reset never arms.
    held_reset: bool,
}
impl WipeoutState {
    /// Trick.DisableWipeoutReset. INFERRED: retail's bit is a one-frame pulse only a running bail latches, so a call
    /// outside a bail (`wiping` false) does nothing. Not retail (user 10-14): the S1 latch drives TU3's recovery here.
    // Called by skate1_challenge's Request::DisableWipeoutReset arm.
    pub fn hold_reset(&mut self, wiping: bool) {
        self.held_reset |= wiping;
    }
}

/// TU3's recovery (manage_recovery) under the Skate 1 hold: while held, the timed reset (`automatic`, retail +81)
/// never arms, so no fade and no teleport. ponytail: undone after the native call instead of a gate inside
/// skate-core's oracle-checked recovery; the impale reset still runs (TU3-only, no S1 trace).
fn recover(state: &mut State, held: bool, flags: [u32; 3], timestep: f32, settings: &recovery::Settings) {
    state.manage_recovery(flags[0], flags[1], flags[2], timestep, settings);
    if held {
        state.teleport_pending = false;
    }
}

impl WipeoutState {
    pub fn load(data: &Collections, assets: &Path, primary_bank_sha: &str) -> Result<Self, String> {
        //Ctor82D3B1EC..B25C full64 handle hashes, verified against the stock keys.
        let names = [
            "free_fall",
            "cannon_ball",
            "judo_kick",
            "swan_dive",
            "torpedo",
        ];
        let profiles = names.map(|name| settings::load_profile(data, name));
        let [a, b, c, d, e] = profiles;
        Ok(Self {
            ragdoll: ragdoll::RagdollSetup::load(data, assets, primary_bank_sha)?,
            state: State::default(),
            settings: settings::Settings::load(data)?,
            drives: drive_settings::load(data, assets, primary_bank_sha)?,
            profiles: [a?, b?, c?, d?, e?],
            contact: ContactResponse::new(),
            prediction: prediction::Prediction::new(),
            held_reset: false,
        })
    }
}
pub(crate) use lifecycle::{enter, exit, post_physics};
pub(crate) use output::fill;
pub(crate) use update::advance;

#[cfg(test)]
mod tests {
    use super::*;

    /// L15: a bail held by DisableWipeoutReset never auto-resets; the next bail (enter clears the hold) does.
    #[test]
    fn a_held_bail_does_not_auto_reset_and_the_next_one_does() {
        let settings = recovery::Settings {
            minimum_time: 0.5, minimum_settled: 0.1, maximum_time: 1.0, fade_time: 0.05, over_speed: 0., over_minimum_time: 0.,
        };
        let bail = |held: bool| {
            let mut state = State { time: 2.0, response_time: 2.0, ..State::default() };
            (0..120).any(|_| {
                recover(&mut state, held, [0; 3], 1. / 30., &settings);
                state.request_teleport
            })
        };
        assert!(!bail(true), "held: no auto-reset");
        assert!(bail(false), "the next bail resets");
    }
}
