//! Actual UpdatePoseBuffer82b4d970..de1c global/local publication.
//! Child of `SkeletonNormalAssembly`. Mandatory board visual tail follows.
use super::SkeletonNormalAssembly;
#[path = "physical_player_post_wipeout_pose_board.rs"]
pub(crate) mod board_visuals;
use crate::{
    actor_construction::anim_out_phys_in::{
        AnimOutPhysInOwner,
        pose_lanes::{PoseAccessError, PoseBuffer},
    },
    dynamic_assembly_session::part_transform::DynamicAssemblyPartTransformError,
    ragdoll::SkeletonPartIndex,
    skateboard_assembly::{SkateboardAssemblySession, SkateboardSimulationBackend},
    skeleton_root_transforms::SkeletonRootMatrix as Matrix,
};

#[derive(Debug)]
pub(crate) enum PosePublicationError {
    MissingBoneParameters(usize),
    Pose(PoseAccessError),
    Part(DynamicAssemblyPartTransformError),
}
impl std::fmt::Display for PosePublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBoneParameters(part) => write!(
                f,
                "UpdatePoseBuffer Part {part} authored parameters missing"
            ),
            Self::Pose(error) => error.fmt(f),
            Self::Part(error) => write!(f, "UpdatePoseBuffer deck: {error:?}"),
        }
    }
}

/// Native deck local captured dbd8, before reverse local publication. This
/// value authorizes only the mandatory de30..ee98 continuation, not success
/// of `UpdatePoseBuffer` or the enclosing `PostWipeoutCheck`.
pub(crate) struct PoseBeforeBoardVisuals {
    pub(crate) deck: Matrix,
}

impl SkeletonNormalAssembly {
    /// `pose` must be the SAME `constructor.bindings.animation_input` locked by
    /// the retained world-pair caller. No secondary matrices are constructed.
    pub(crate) fn try_post_wipeout_pose_records(
        &self,
        pose: &mut AnimOutPhysInOwner,
        board: &SkateboardAssemblySession,
        backend: &SkateboardSimulationBackend,
    ) -> Result<PoseBeforeBoardVisuals, PosePublicationError> {
        use PosePublicationError as E;
        let data = &self.state.binding.body().data;
        for part in SkeletonPartIndex::ALL {
            let index = part.index();
            // PDB Skeleton+40 SkeletonData+8 mBoneHandles is an array of
            // PhysicsParamBoneData pointers, NOT attached rigid-body handles.
            data.parameters()
                .bones
                .get(index)
                .ok_or(E::MissingBoneParameters(index))?;
            let record = self.physics_history.post_wipeout_record_transform(index);
            let world_to_animation = self.root_transforms.world_to_animation();
            let volume_to_bone = Matrix::known(data.volume_to_bone()[index]);
            let global = world_to_animation.compose(&record.compose(&volume_to_bone));
            pose.set_pose_matrix_typed(PoseBuffer::Global, data.bone_ids()[index], &global)
                .map_err(E::Pose)?;
        }
        // Source GetSkateboard/GetBody/GetPosition calls separate the loops.
        // A late deck failure leaves all globals and every old local intact.
        let deck = board
            .try_live_part_transform_typed(backend, 6)
            .map_err(E::Part)?;
        for part in SkeletonPartIndex::ALL.into_iter().rev() {
            let index = part.index();
            let bone = data.bone_ids()[index];
            let local = if let Some(parent) = data.parents()[index] {
                let parent = pose
                    .pose_matrix_typed(PoseBuffer::Global, data.bone_ids()[parent.index()])
                    .map_err(E::Pose)?;
                let inverse = parent.inverse();
                let child = pose
                    .pose_matrix_typed(PoseBuffer::Global, bone)
                    .map_err(E::Pose)?;
                inverse.compose(&child)
            } else {
                pose.pose_matrix_typed(PoseBuffer::Global, bone)
                    .map_err(E::Pose)?
            };
            pose.set_pose_matrix_typed(PoseBuffer::Local, bone, &local)
                .map_err(E::Pose)?;
        }
        Ok(PoseBeforeBoardVisuals { deck })
    }
}
