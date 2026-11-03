//! Ordinary command payload slice of `PlayerUI::Update828af9f8`.
//! Caller reaches this only after the native simulation admission, focus command,
//! session marker and optional network teleport stages. Spawner and proxy tick
//! follow delivery; this leaf neither schedules nor acknowledges those stages.
use super::SelectedDeviceInputRuntime;
use crate::{
    game_input_frame_session::GameInputFrameRequest,
    game_input_player::PlayerInputData,
    input_pad::device::PadRemap,
    input_pad_cache::InputPadCacheError,
    player::PlayerLoaderSlot,
    simulation_assembly_backend::constraint_session::world_services::{
        PhysicalGameInfo, PhysicalWorldServicesError,
    },
    skeleton_collision_numeric::SkeletonCollisionNumeric,
};

/// Actual pause-owner query, reached only for an unfocused sample.
pub(crate) trait PlayerInputPause {
    type Error;
    fn try_is_simulation_active(&mut self) -> Result<bool, Self::Error>;
}
/// Actual selected command proxy; its unsigned frame is sampled only when simulation is active.
pub(crate) trait PlayerInputProxyFrame {
    type Error;
    fn try_next_generation_frame(&mut self) -> Result<u32, Self::Error>;
}

#[derive(Debug)]
pub(crate) enum SelectedPlayerSampleError<P, F> {
    World(PhysicalWorldServicesError),
    Device(InputPadCacheError),
    Pause(P),
    Proxy(F),
}
impl<P: std::fmt::Debug, F: std::fmt::Debug> std::fmt::Display for SelectedPlayerSampleError<P, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::World(error) => write!(f, "player input network state: {error:?}"),
            Self::Device(error) => write!(f, "player input current cache: {error}"),
            Self::Pause(error) => write!(f, "player input simulation pause: {error:?}"),
            Self::Proxy(error) => write!(f, "player input proxy frame: {error:?}"),
        }
    }
}

pub(crate) struct SelectedPlayerSample {
    input: PlayerInputData,
    receiver_id: u32,
}
impl SelectedPlayerSample {
    /// Copy the same produced payload into the retained ordinary command proxy.
    /// Native send and admitted command consumption occur separately later.
    pub(crate) fn try_queue_into<N: crate::game_input_command::GameInputCommandNumeric>(
        &self,
        proxy: &mut crate::selected_sim_command_queue::SelectedSimCommandQueue,
        numeric: &mut N,
    ) -> Result<(), crate::selected_sim_command_queue::QueuePlayerInputError<N::Error>> {
        proxy.try_queue_player_input(self.receiver_id, &self.input, numeric)
    }

    /// Borrow directly into the existing sparse codec/command/recognizer path.
    /// Update mask and CRC admission remain supplied by that path's caller.
    pub(crate) const fn frame_request(
        &self,
        update_mask: u32,
        add_to_crc: bool,
    ) -> GameInputFrameRequest<'_> {
        GameInputFrameRequest {
            input: &self.input,
            receiver_id: self.receiver_id,
            update_mask,
            add_to_crc,
        }
    }
}

impl<R: Clone, N: SkeletonCollisionNumeric, M: PadRemap<R>> SelectedDeviceInputRuntime<R, N, M> {
    /// Native payload slice `828afc1c..828afcc0`, over the same selected loader,
    /// global game information and device manager. Controller selection is an
    /// explicit host-supplied native identity, independently of player index.
    /// No pause/proxy default is provided: unavailable collaborators fail lazily.
    pub(crate) fn try_player_sample<P: PlayerInputPause, F: PlayerInputProxyFrame>(
        &self,
        loader: &PlayerLoaderSlot,
        controller: usize,
        game_info: &PhysicalGameInfo,
        has_focus: bool,
        pause: &mut P,
        proxy: &mut F,
    ) -> Result<SelectedPlayerSample, SelectedPlayerSampleError<P::Error, F::Error>> {
        let allow_sensor = !game_info
            .try_is_network_game()
            .map_err(SelectedPlayerSampleError::World)?;
        let mut input = PlayerInputData {
            sample: self
                .raw_current_cache_line(controller)
                .map_err(SelectedPlayerSampleError::Device)?,
            allow_sensor,
            has_focus: true,
        };
        if !has_focus {
            // Source clears all 24 words, including stale disconnected values;
            // it retains NumButtons and initially leaves payload focus true.
            input.sample.values.fill(0.0);
            if pause
                .try_is_simulation_active()
                .map_err(SelectedPlayerSampleError::Pause)?
                && proxy
                    .try_next_generation_frame()
                    .map_err(SelectedPlayerSampleError::Proxy)?
                    > 1
            {
                input.has_focus = false;
            }
        }
        Ok(SelectedPlayerSample {
            input,
            receiver_id: loader.player_index().wrapping_add(10),
        })
    }
}

#[cfg(test)]
#[path = "input_selected_player_sample_tests.rs"]
mod tests;
