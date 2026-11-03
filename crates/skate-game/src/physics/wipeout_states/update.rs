//! Complete Wipeout300 update order82D3B9C8, on the game body/pose owners.
use super::*;
use crate::physics::{GamePhysics, SkaterRuntime, skeleton_input_runtime::SkeletonOwners};
use skate_core::{
    math::Vector3,
    physics::board::BodyId,
    player::wipeout_state::{
        body, control_air, control_ground, control_profile_air, math, profiles, response, update,
        update::Control, weights,
    },
    riding::ground_correction_math::apply_world_force,
};

pub(crate) fn advance(physics: &mut GamePhysics, skater: &mut SkaterRuntime) -> Result<(), String> {
    let p = &skater.player_input.processed;
    let runtime = &mut skater.wipeout_state;
    let state = &mut runtime.state;
    let contact = skater.collision_feedback.flags.any || state.below_surface;
    state.airborne_frames = if contact {
        -1
    } else {
        state.airborne_frames.wrapping_add(1)
    };
    if !state.special_surface && p.flags_2488 & 0x4000_0000 != 0 {
        state.special_surface = true;
        state.surface_height = p.collision_scalar_2924;
        runtime.ragdoll.request(
            &mut skater.ground_lifecycle.skeleton_controller,
            10,
            &mut skater.skeleton,
            &mut skater.skeleton_joints,
            &mut skater.skeleton_collision,
        )?;
    }
    let velocity = p.vectors_544_560_592_608[3].map(f32::from_bits);
    if update::spikes(state.velocity, velocity) {
        body::limit_velocity(&mut skater.skeleton, 5.0);
    }
    state.velocity = velocity;
    let speed = math::length(velocity);
    response::retained_velocity(
        state,
        &mut skater.skeleton,
        &mut skater.skeleton_drives,
        &runtime.drives,
        contact,
        p.flags_2468,
        velocity,
    );
    let position = p.vectors_544_560_592_608[2].map(f32::from_bits);
    if state.special_surface {
        state.below_surface = position[1] - state.surface_height < 0.0;
        body::special_surface(&mut skater.skeleton, state.surface_height);
    }
    let effective = p
        .effective_anim_transform_192
        .map(|row| row.map(f32::from_bits));
    let controls = skater.animation_input.extra.wipeout_control;
    let gesture = skater.animation_input.extra.wipeout_gesture;
    if !state.below_surface {
        contact::update(
            state,
            &mut runtime.contact,
            &mut skater.skeleton,
            &skater.collision_feedback,
            &mut skater.ground_lifecycle.skeleton_controller,
            &mut skater.skeleton_collision,
            &mut skater.skeleton_joints,
            &runtime.ragdoll,
            contact,
            velocity,
            effective[2],
            controls,
        )?;
        body::set_wipeout_drag(&mut skater.skeleton, update::drag(state, contact));
    }
    update::advance_clocks(state, speed);
    response::update_counter(state, contact);
    let com = skater.animated_skeleton.board_frames.centre_of_mass;
    let mut control_applied = false;
    match update::control(state, contact, p.flags_2484) {
        Control::Submerged => {
            control_air::update(state, &mut skater.skeleton, com, controls, true);
        }
        Control::Response => {
            let normal = response::response_normal(
                skater
                    .collision_feedback
                    .flags
                    .compliant
                    .then_some(skater.collision_feedback.highest_normal),
                runtime
                    .prediction
                    .result
                    .valid()
                    .then_some(runtime.prediction.result.landing_normal),
            );
            response::trigger(state, &mut skater.skeleton, normal, controls);
        }
        Control::ProfileAir => {
            profiles::prepare(state, &runtime.profiles, gesture, controls);
            let profile = &runtime.profiles[state.profile];
            control_profile_air::update(
                state,
                &mut skater.skeleton,
                com,
                &effective,
                controls,
                profile,
            );
            profiles::drift(state, &mut skater.skeleton, profile);
            control_applied = true;
        }
        Control::Air { contact } => {
            control_applied =
                control_air::update(state, &mut skater.skeleton, com, controls, contact);
        }
        Control::Ground => {
            profiles::prepare(state, &runtime.profiles, gesture, controls);
            control_ground::update(
                state,
                &mut skater.skeleton,
                com,
                &effective,
                // 82D3D734: Processed+464 (the ground vector_96, publication.rs).
                p.vectors_464_480_496_512_528[0].map(f32::from_bits),
                &runtime.profiles[state.profile],
                &skater.animated_skeleton.roots.animation_to_world,
                &skater.animated_skeleton.record.pose,
            );
            control_applied = true;
        }
    }
    let weight = weights::update(
        state,
        weights::Settings {
            remove_target_time: runtime.settings.remove_target_time,
            remove_drives_time: runtime.settings.remove_drives_time,
            collision_step: runtime.settings.collision_weight_step,
            controlled_step: runtime.settings.controlled_weight_step,
        },
        contact,
        control_applied,
        p.flags_2472,
        gesture,
    );
    skater.ground.steering.update(
        0.0,
        skater.air_settings.steering_blend,
        p.flags_2468,
        p.flags_2472,
    );
    drives::set_angular_root(
        &mut skater.skeleton_drives,
        &runtime.drives,
        weight.target_weight,
    );
    let mut owners = SkeletonOwners {
        animated: &mut skater.animated_skeleton,
        body: &mut skater.skeleton,
        drives: &mut skater.skeleton_drives,
        ik: &mut skater.foot_ik,
        animation_input: &mut skater.animation_input,
        correction: &mut skater.skeleton_output.correction,
        pose_errors: &mut skater.pose_errors,
    };
    skeleton::update(
        &mut skater.skeleton_input,
        &mut owners,
        p,
        &skater.animation.packet.hierarchy,
        &runtime.drives,
        weight.start,
        weight.end,
        weight.controlled,
        weight.extra,
        state.retained_velocity_active,
    )?;
    if state.move_board && (state.board_move_frames as i32) < 21 {
        let deck = physics.board.part_transforms()[BodyId::Deck.index()].translation;
        let point = skater
            .player_input
            .toolkit
            .as_ref()
            .ok_or("Wipeout board force requires the current BoardToolkit")?
            .deck[3];
        apply_world_force(
            &mut physics.board.bodies_mut()[BodyId::Deck.index()],
            deck,
            vector(state.board_offset),
            vector(point),
        );
        state.board_move_frames = state.board_move_frames.wrapping_add(1);
    }
    super::recover(
        state,
        runtime.held_reset,
        [p.flags_2468, p.flags_2472, p.flags_2484],
        p.timestep_2604,
        &runtime.settings.recovery,
    );
    let gravity = physics.settings.step.simulation.gravity_acceleration;
    runtime.prediction.advance(
        state,
        physics.world(),
        position,
        [gravity.x, gravity.y, gravity.z, 0.0],
    )
}
fn vector(v: [f32; 4]) -> Vector3 {
    Vector3::new(v[0], v[1], v[2])
}
