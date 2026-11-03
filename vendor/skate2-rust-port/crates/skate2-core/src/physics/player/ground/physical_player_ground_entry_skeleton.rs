//! Selected SAME-pair Skeleton stores within `PhysicsGround::Enter`.
use crate::{
    dynamic_assembly_session::skater_pool::{
        SimulationSkaterAssemblyHandle, SimulationSkeletonJumpPairError,
    },
    physical_player_assembly::PhysicalAssemblyWorld,
};

#[derive(Debug)]
pub(crate) enum GroundEntrySkeletonError {
    MissingPool,
    Pair(SimulationSkeletonJumpPairError),
    MissingIk,
}
impl std::fmt::Display for GroundEntrySkeletonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPool => f.write_str("ground entry skater pool unavailable"),
            Self::Pair(error) => write!(f, "ground entry pair: {error}"),
            Self::MissingIk => f.write_str("ground entry Skeleton IK not constructed"),
        }
    }
}
impl PhysicalAssemblyWorld {
    /// Source 0x82d99c64..9c74, between actual board `SetStandard` and actual
    /// `SkeletonController` transition. No volume/body/pose publication changes.
    pub(crate) fn try_ground_entry_skeleton_ik_and_tilt(
        &mut self,
        board: &SimulationSkaterAssemblyHandle,
        skeleton: &SimulationSkaterAssemblyHandle,
    ) -> Result<(), GroundEntrySkeletonError> {
        use GroundEntrySkeletonError as E;
        self.skaters
            .as_mut()
            .ok_or(E::MissingPool)?
            .try_with_jump_pair(&self.backend, board, skeleton, |normal, _owner| {
                normal
                    .try_ground_entry_ik_and_tilt()
                    .map_err(|_| E::MissingIk)
            })
            .map_err(E::Pair)?
    }
}
