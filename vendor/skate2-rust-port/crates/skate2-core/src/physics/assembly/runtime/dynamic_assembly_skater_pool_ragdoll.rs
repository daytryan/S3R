//! Same admitted pair and controller state8, with retained constructor inputs.
use super::{
    MemberContent, SimulationSkaterAssemblyHandle, SimulationSkaterAssemblyPool,
    SimulationSkaterPoolError, SkateboardSimulationBackend,
};
use crate::{
    dynamic_assembly_session::{
        skater_pool::SimulationSkeletonJumpPairError,
        skeleton_normal::{
            SkeletonNormalAssembly,
            ragdoll_owner::{RagdollInputs, RagdollSetupError},
        },
    },
    input_cvar_runtime::InputCvarReferences,
    skeleton_controller::{SkeletonControllerCollision, SkeletonControllerError},
    skeleton_runtime_state::SkeletonRuntimeStateError,
};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum RagdollCollisionError<C> {
    Runtime(SkeletonRuntimeStateError),
    Setup(RagdollSetupError<C>),
}
impl<C: std::fmt::Debug> std::fmt::Display for RagdollCollisionError<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(e) => write!(f, "{e}"),
            Self::Setup(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum PairRagdollError<C> {
    Pair(SimulationSkeletonJumpPairError),
    Pool(SimulationSkaterPoolError),
    Poisoned,
    Controller(SkeletonControllerError<RagdollCollisionError<C>>),
}
impl<C: std::fmt::Debug> std::fmt::Display for PairRagdollError<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pair(e) => write!(f, "ragdoll pair: {e}"),
            Self::Pool(e) => write!(f, "ragdoll pool: {e}"),
            Self::Poisoned => f.write_str("ragdoll controller lock poisoned"),
            Self::Controller(e) => write!(f, "{e}"),
        }
    }
}

struct Collision<'a, C, F> {
    normal: &'a mut SkeletonNormalAssembly,
    backend: &'a mut SkateboardSimulationBackend,
    inputs: Option<RagdollInputs<'a>>,
    cosine: &'a mut F,
    marker: std::marker::PhantomData<C>,
}
impl<C, F: FnMut(f32) -> Result<f32, C>> SkeletonControllerCollision for Collision<'_, C, F> {
    type Error = RagdollCollisionError<C>;
    fn set_ground_collision(&mut self) -> Result<(), Self::Error> {
        self.normal
            .try_set_ground_collision()
            .map_err(RagdollCollisionError::Runtime)
    }
    fn collide_with_world_only(&mut self) -> Result<(), Self::Error> {
        self.normal
            .try_collide_with_world_only()
            .map_err(RagdollCollisionError::Runtime)
    }
    fn set_ragdoll_collision(&mut self) -> Result<(), SkeletonControllerError<Self::Error>> {
        self.normal
            .try_set_up_ragdoll(
                self.backend,
                self.inputs.take().expect("one controller Enter"),
                self.cosine,
            )
            .map_err(RagdollCollisionError::Setup)
            .map_err(SkeletonControllerError::Collision)
    }
}

impl SimulationSkaterAssemblyPool {
    /// Restore SkeletonBody's constructor byte (82b62594) when a new game
    /// reuses a host Actor. Ordinary teleports and ragdoll entries retain it.
    pub(crate) fn try_pair_set_spawn_bouncy(
        &mut self,
        backend: &SkateboardSimulationBackend,
        board: &SimulationSkaterAssemblyHandle,
        skeleton: &SimulationSkaterAssemblyHandle,
        bouncy: bool,
    ) -> Result<(), PairRagdollError<std::convert::Infallible>> {
        use PairRagdollError as E;
        self.try_with_jump_pair(backend, board, skeleton, |_, _| ())
            .map_err(E::Pair)?;
        let index = self.skeleton_mutation_index(backend, skeleton).map_err(E::Pool)?;
        let MemberContent::Skeleton { candidate, .. } = &mut self.members[index].content else {
            unreachable!("validated Skeleton")
        };
        let stored = candidate.construction.as_mut()
            .and_then(|construction| construction.constructor.bindings.as_mut())
            .and_then(|bindings| bindings.bouncy_ragdoll.as_mut())
            .ok_or(E::Pool(SimulationSkaterPoolError::MissingPhysicalConstruction))?;
        *stored = u8::from(bouncy);
        Ok(())
    }

    pub(crate) fn try_pair_ragdoll_collision<C>(
        &mut self,
        backend: &mut SkateboardSimulationBackend,
        board: &SimulationSkaterAssemblyHandle,
        skeleton: &SimulationSkaterAssemblyHandle,
        refs: &InputCvarReferences,
        cosine: &mut impl FnMut(f32) -> Result<f32, C>,
    ) -> Result<(), PairRagdollError<C>> {
        use PairRagdollError as E;
        let owner = self
            .try_with_jump_pair(backend, board, skeleton, |_, owner| Arc::clone(owner))
            .map_err(E::Pair)?;
        let index = self
            .skeleton_mutation_index(backend, skeleton)
            .map_err(E::Pool)?;
        self.invalidate_refresh();
        let MemberContent::Skeleton { candidate, .. } = &mut self.members[index].content else {
            unreachable!("validated Skeleton")
        };
        let (bouncy, culling, body_reader, joint_reader) = candidate
            .construction
            .as_mut()
            .and_then(|construction| construction.ragdoll_inputs_mut())
            .ok_or(E::Pool(
                SimulationSkaterPoolError::MissingPhysicalConstruction,
            ))?;
        let mut collision = Collision {
            normal: &mut candidate.owner,
            backend,
            inputs: Some(RagdollInputs {
                bouncy,
                culling,
                body_reader,
                joint_reader,
                cvars: refs,
            }),
            cosine,
            marker: std::marker::PhantomData,
        };
        owner
            .skeleton_controller()
            .lock()
            .map_err(|_| E::Poisoned)?
            .try_set_collision_state(8, &mut collision)
            .map_err(E::Controller)
    }
}
