//! What a completed tick publishes, read from the session into the protocol's `Frame`. Not tested in play.
//!
//! Filled here: the tick, its period, the camera, the trick events, the state ids, the category, the board, the
//! skater's pose, the skater's audio outputs, the Hall of Meat state and the placement.
//!
//! The pose's matrices (`Frame::root`, `Frame::bones`) are the port's `[f32; 16]` in row-vector order: elements 0-3,
//! 4-7 and 8-11 are rows 0-2, the x, y and z axes in world space, and elements 12-14 (row 3) the world position, with
//! column 3 (elements 3, 7, 11, 15) 0, 0, 0, 1. A point `(x, y, z)` in a bone's space is at
//! `x * row0 + y * row1 + z * row2 + row3` in the world, so "`a` then `b`" is the row-major product `a * b`.
//! - `root` is the presentation's `local_to_world` (`root * hips`): the DataPostBlend root after the saved Hips
//!   transform, the matrix the port's skin palette is packed with. The presentation's own space is the hips'.
//! - `bones[i]` is presentation bone `i` (its names are `PORT_BONES`, `Host::bone_names`) where the port draws it:
//!   its global pose in that space, then `root`. `bones[0]`, the trajectory, is the skater root (`GetSkaterRoot`),
//!   since the port's bone buffer holds the saved Hips transform in that slot.

use skate2_core::physics::FilteredPhysicsState;
use skate2_core::ui::trick_backend::PhysicsFrame;
use skate2_core::{SelectedVisualSnapshot, SkaterAudioPhysOut};
use skate2_physics_protocol::{Camera, Frame, Hom, HomHud, SkaterAudio, TICK_SECONDS, Tricks};

use crate::session::Session;

/// The game camera of the snapshot: `CameraMan`'s matrix and the projection the port renders with.
pub(crate) fn camera(visual: &SelectedVisualSnapshot<'_>) -> Camera {
    Camera {
        position: visual.camera_world_position,
        forward: visual.camera_forward,
        right: visual.camera_right,
        up: visual.camera_up,
        clip_from_world: visual.provisional_clip_from_world,
    }
}

/// The port's last scorable id: its scorables are 0..=268 (`scoring/catalog.rs`), the engine's `IDENTIFIERS` 0..=268.
const LAST_SCORABLE: u16 = 268;

/// `Frame::tricks` from the port's HUD frames of the tick numbered `tick` (`take_scoring_hud_frames`, oldest first):
/// the trick events (`begun`, `converted`, `ended`) of every frame, in order, and the rest from the last frame, the
/// state the tick ended in (`active` too, which is a state, not an event). No frame gives the default. The port
/// publishes one frame per completed tick. A trick id outside 0..=268 is left out, never wrapped, with a line in
/// `logs`.
pub(crate) fn tricks(frames: &[PhysicsFrame], tick: u32, logs: &mut Vec<String>) -> Tricks {
    let mut scorable = |id: i32, list: &str| -> Option<u16> {
        let kept = u16::try_from(id).ok().filter(|id| *id <= LAST_SCORABLE);
        if kept.is_none() {
            logs.push(format!(
                "tick {tick}: the port's {list} trick id {id} is not a scorable (0..={LAST_SCORABLE}); left out"
            ));
        }
        kept
    };
    let mut tricks = Tricks::default();
    for frame in frames {
        tricks.begun.extend(frame.begun_all.iter().filter_map(|trick| scorable(trick.scorable_id, "begun")));
        tricks
            .converted
            .extend(frame.converted_all.iter().filter_map(|trick| scorable(trick.scorable_id, "converted")));
        tricks.ended.extend(frame.ended.iter().filter_map(|&(id, points)| Some((scorable(id, "ended")?, points))));
    }
    let Some(last) = frames.last() else {
        return tricks;
    };
    Tricks {
        active: last.active_ids.iter().filter_map(|&id| scorable(id, "active")).collect(),
        grinding: last.grinding,
        sequence_begun: last.sequence_begun,
        sequence_completed: last.sequence_completed,
        completed_successfully: last.completed_successfully,
        interrupted: last.interrupted,
        line_score: last.line_score,
        sequence_score: last.hud_score,
        line_points: last.line_points,
        line_max_points: last.line_max_points,
        multiplier: last.multiplier,
        spins: last.spins,
        flips: last.flips,
        just_landed: last.just_landed,
        sketchy: last.sketchy,
        ..tricks
    }
}

/// `Frame::hom` from the port's HUD frames of the tick (`take_scoring_hud_frames`, oldest first): the one-update events
/// (`just_wiped_out`, `just_reactivated`, `bail_completed`) of any frame, and the rest from the last frame, the state the
/// tick ended in. No frame gives the default (the effect off). `effect_enabled` is the port's
/// `Physics::IsHallOfMeatEffectEnabled`, which is off in the game modes 0, 3 and 4.
pub(crate) fn hom(frames: &[PhysicsFrame]) -> Hom {
    let mut hom = Hom::default();
    for frame in frames {
        hom.just_wiped_out |= frame.hom.just_wiped_out;
        hom.just_reactivated |= frame.hom.just_reactivated;
        hom.bail_completed |= frame.hom.bail_completed;
    }
    let Some(last) = frames.last() else {
        return hom;
    };
    let data = &last.hom.data;
    Hom {
        effect_enabled: last.hom.effect_enabled,
        bail_in_progress: last.hom.bail_in_progress,
        hud: HomHud {
            dirty: data.dirty,
            score: data.score,
            multiplier: data.multiplier,
            rotation: data.rotation,
            rotation_score: data.rotation_score,
            wipeout_speed_score: data.wipeout_speed_score,
            max_air_drop_score: data.max_air_drop_score,
            max_air_time_score: data.max_air_time_score,
            wipeout_duration_score: data.wipeout_duration_score,
            wipeout_speed: data.wipeout_speed,
            max_air_drop: data.max_air_drop,
            max_air_time: data.max_air_time,
            wipeout_duration: data.wipeout_duration,
            new_car_collisions: data.new_car_collisions,
            new_ped_collisions: data.new_ped_collisions,
            new_dmo_collisions: data.new_dmo_collisions,
            new_skater_collisions: data.new_skater_collisions,
            new_body_tweak: data.new_body_tweak,
            car_scores: data.car_collision_score,
            ped_scores: data.ped_collision_score,
            dmo_scores: data.dmo_collision_score,
            skater_scores: data.skater_collision_score,
            tweak: data.body_tweak_score,
        },
        ..hom
    }
}

/// The frame of the tick numbered `tick` (from 1), which has just completed, seen through `camera`, with its trick
/// events, its Hall of Meat state and the last placement's generation and whether it has settled.
pub(crate) fn read(
    session: &Session,
    tick: u32,
    camera: Camera,
    tricks: Tricks,
    hom: Hom,
    placed: Option<(u32, bool)>,
) -> Result<Frame, String> {
    let state = session.diagnostic_state_ids().ok_or("no state ids after a completed frame")?;
    let filtered = session.diagnostic_filtered_state().ok_or("no filtered state after a completed frame")?;
    let (deck, _z_speed, speed) =
        session.diagnostic_board_kinematics().ok_or("no board kinematics after a completed frame")?;
    let (root, bones) = session.host_skater_pose().ok_or("no skater pose after a completed frame")?;
    let audio = session
        .audio_skaters()
        .and_then(|skaters| skaters.into_iter().find(|skater| skater.local))
        .map(|skater| audio(&skater.phys_out))
        .unwrap_or_default();
    Ok(Frame {
        tick,
        // Every tick is 1/60 s while the Hall of Meat slow motion, which the port removed, is not back (VENDORED.md,
        // port deviation 1).
        sim_period_s: TICK_SECONDS,
        root,
        bones,
        camera,
        tricks,
        hom,
        audio,
        state,
        category: category(filtered),
        deck,
        // PhysOut's motion deck speed, in m/s.
        speed,
        placed,
        ..Frame::default()
    })
}

/// `Frame::category`: the port's filtered physics state (PhysOut's `Filtered`, which the Physics conditioner
/// publishes) as a byte, in the port's own order, which is also the engine's `FilteredCategory`: 0 invalid, 1 ground,
/// 2 air, 3 grind, 4 wipeout, 5 teleport, 6 off board, 7 off board in the air.
fn category(state: FilteredPhysicsState) -> u8 {
    match state {
        FilteredPhysicsState::Invalid => 0,
        FilteredPhysicsState::Ground => 1,
        FilteredPhysicsState::Air => 2,
        FilteredPhysicsState::Grind => 3,
        FilteredPhysicsState::Wipeout => 4,
        FilteredPhysicsState::Teleport => 5,
        FilteredPhysicsState::OffBoard => 6,
        FilteredPhysicsState::OffBoardAir => 7,
    }
}

fn audio(out: &SkaterAudioPhysOut) -> SkaterAudio {
    SkaterAudio {
        centre_of_mass_velocity: out.centre_of_mass_velocity,
        deck_velocity: out.deck_velocity,
        wheel_pos: out.wheel_pos,
        skateboard_surface_speed: out.skateboard_surface_speed,
        time_in_known_air: out.time_in_known_air,
        jump_height: out.jump_height,
        deck_slide_speed: out.deck_slide_speed,
        wheel_slide: out.wheel_slide,
        turn: out.turn,
        num_wheels_in_contact: out.num_wheels_in_contact,
        physics_state: out.physics_state,
        is_skateboard_airborne: out.is_skateboard_airborne,
        is_wiping_out: out.is_wiping_out,
        is_wipeout_over: out.is_wipeout_over,
        is_manual: out.is_manual,
        is_foot_braking: out.is_foot_braking,
        is_physically_pushing: out.is_physically_pushing,
        is_left_foot_planting: out.is_left_foot_planting,
        is_right_foot_planting: out.is_right_foot_planting,
        deck_up: out.effective_transform_y,
        deck_nose: out.deck_nose_dir,
        deck_local_angular_speed: out.deck_local_xyz_angular_speed,
        deck_tilt: out.deck_tilt,
        is_manual_braking: out.is_manual_braking,
        is_generally_pushing: out.is_generally_pushing,
        is_generally_pushing_right_foot: out.is_generally_pushing_right_foot,
        is_reverting: out.is_reverting,
        is_left_foot_on_deck: out.is_left_foot_on_deck,
        is_right_foot_on_deck: out.is_right_foot_on_deck,
        is_front_truck_in_contact: out.is_front_truck_in_contact,
        is_back_truck_in_contact: out.is_back_truck_in_contact,
        is_deck_in_contact: out.is_deck_in_contact,
        centre_of_mass: out.centre_of_mass,
        foot_left_velocity: out.foot_left_vel,
        foot_right_velocity: out.foot_right_vel,
        foot_left_velocity_deck: out.foot_l_vel_deck_space,
        foot_right_velocity_deck: out.foot_r_vel_deck_space,
        torso_angular_velocity: out.torso_angular_vel,
        right_foot_angular_velocity: out.right_foot_angular_vel,
        left_foot_angular_velocity: out.left_foot_angular_vel,
        average_limb_speed: out.average_limb_speed,
        left_foot_down: out.left_foot_down,
        right_foot_down: out.right_foot_down,
        is_holding_skateboard: out.is_holding_skateboard,
        is_foot_planting: out.is_foot_planting,
        body_contact_force: out.averaged_skeleton_collision_info.contact_force,
        body_slide_speed: out.averaged_skeleton_collision_info.slide_speed,
        is_groin_contact: out.averaged_skeleton_collision_info.is_groin_contact,
        is_face_contact: out.averaged_skeleton_collision_info.is_face_contact,
    }
}
