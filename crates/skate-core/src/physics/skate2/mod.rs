//! Skate 2 physics, rewritten from the Skate 2 (Europe) disc executable so that every `f32` it
//! produces has the same bits as the original's.
//!
//! **Pilot scope: pushing.** Input → push force → force record → the deck's acceleration
//! accumulators → the rigid-body integrator's new velocity. Not wired into the game and no
//! physics toggle; see `docs/superpowers/S2_PHYSICS_PILOT.md`.
//!
//! Each function names its Skate 2 address. Float semantics are [`float`]'s (the oracle's model of
//! the recompiled Xenon, not measured silicon). The ignored tests in `tests.rs` run the original
//! functions from the user's disc (`oracle.rs`) on the same inputs and compare bits.

pub mod float;
mod integrate;
mod push;

pub use integrate::{BodyDefinition, Corrections, RigidBody, SimulationStep, dynamic_update};
pub use push::{
    FORCE_RECORD_CAPACITY, ForceRecord, PUSH_BLOCKED, PUSH_BLOCKED_FLAG, PUSH_REQUESTED, PushInput,
    PushOutput, PushTweaks, accumulate_forces, calc_push_acceleration, queue_force,
};

#[cfg(test)]
mod oracle;
#[cfg(test)]
mod tests;
