//! Physical-output publications consumed by gameplay camera82DF69C0.
//! Reset82DE53F0 runs before the actual producers82DB6EC0/82D3A388.
use super::{GamePhysics, SkaterRuntime};
use crate::camera::{
    CameraAirOutput, CameraAnimationOutput, CameraEventsOutput, CameraGrindOutput,
    CameraOffboardOutput, CameraPreferences, CameraPublicationInputs, CameraStateOutput,
};
use skate_core::{
    animation::physical_feedback::PhysicalFeedback,
    camera::{MovingObstacleProvider, PathObstacle},
    physics::centre_of_mass_filter::CentreOfMassOutput,
};

/// Game Settings > Control Settings choices. The derived default (High, uninverted) is what tests and replays start
/// from; the game applies Game Settings, whose unsaved Camera Angle is retail's Low (+188 = 0, getter 0x8260D9A0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CameraOptions {
    /// Camera Angle: Low selects the normal stock graph type0 instead of type1.
    pub low: bool,
    /// Offboard X/Y Axis: Inverted.
    pub invert_look: [bool; 2],
}

/// The actual game camera call, after completed physical output conditioning.
/// The user's Camera Angle picks graph type0 (Low) or type1 (High). This custom world
/// has no road/ledge/camera-volume annotations or other moving actors.
pub(crate) fn advance(
    physics: &GamePhysics,
    skater: &SkaterRuntime,
    feedback: &PhysicalFeedback,
    camera: &mut crate::camera::CameraRuntime,
) -> Result<(), String> {
    let output = physics
        .exchange
        .output()
        .ok_or("Camera requires the completed physical output snapshot")?;
    let completed_tick = physics
        .ticks
        .checked_sub(1)
        .ok_or("Camera received physical output before the first completed tick")?;
    if output.tick != completed_tick || output.state != skater.player_state.current() {
        return Err(format!(
            "Camera received stale physical output: output_tick={}, completed_tick={}, output_state={:?}, selected_state={:?}",
            output.tick,
            completed_tick,
            output.state,
            skater.player_state.current(),
        ));
    }
    let inputs = publish(
        skater,
        output,
        feedback,
        skater.centre_of_mass_output,
        CameraPreferences {
            invert_look: physics.camera.invert_look,
            // Native preferences constructor82DF6294/629C initializes both
            // fields to zero.82DF8E80 only updates its air-side bytes37..39.
            shake_variant: 0,
            value_32: 0.0,
        },
        // SkaterAnim vtable8231E170+28=82B97140: full15180 bit30.
        u8::from(skater.animation.stance().1),
        1, // Stable host player identity replaces the original actor pointer.
        output.tick,
    )?;
    let snapshot = crate::camera::publish_camera_subject(physics, skater, &inputs)?;
    let grinds = &skater.player_input.physical.grinds;
    camera.ledge.update(
        grinds.grinding_316 != 0,
        grinds.is_ledge_320 != 0,
        skater.player_input.physical.skateboard.vector_80.map(f32::from_bits),
        grinds.high_side_112.map(f32::from_bits),
    );
    let environment = crate::camera::CameraGraphEnvironment {
        camera_type: u32::from(!physics.camera.low),
        // ponytail: SkaterIsOnRoad (+36, 0x82DF68FC) asks the world's road network ([0x830854C0], "Road Points
        // KDTree", loaded at 0x826B1540) for a road under the hips (getter444; slot 11 0x826B3880 -> 2D lookup
        // 0x826B3B18). No map's road network is extracted yet, so it stays false.
        on_road: false,
        ledge_left: camera.ledge.left,
        ledge_right: camera.ledge.right,
        volumes: Vec::new(),
    };
    let gravity = physics.settings.step.simulation.gravity_acceleration;
    camera
        .advance(
            physics.settings.step.simulation.time_step,
            snapshot,
            physics.world(),
            [gravity.x, gravity.y, gravity.z, 0.0],
            &environment,
            &mut StaticWorld,
        )
        .map(|_| ())
}

/// The camera manager's IsGrindLedgeLeft / IsGrindLedgeRight bytes (+37/+38, 0x82DF8E80). The manager updates them
/// before the subject publishes (0x82DF68A0), so they read the previous publication: grinding (getter596, Grinds+316;
/// getter680 is cleared by the publisher) latched in +39. On the tick a grind starts, on a ledge (getter600,
/// Grinds+320) with horizontal board velocity (getter392, Motion+80, y dropped) of at least 0.001: Left when the
/// grind's high side (getter432, Grinds+112) points against velocity x up, else Right. Otherwise both clear. They
/// hold until the next grind starts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LedgeSide {
    grinding: bool,
    pub left: bool,
    pub right: bool,
    /// The previous publication: grinding, on a ledge, board velocity, high side.
    last: Option<(bool, bool, [f32; 4], [f32; 4])>,
}
impl LedgeSide {
    pub(crate) fn update(&mut self, grinding: bool, ledge: bool, velocity: [f32; 4], high_side: [f32; 4]) {
        let Some((grinding, ledge, [x, _, z, _], high)) = self.last.replace((grinding, ledge, velocity, high_side)) else {
            return;
        };
        let started = grinding && !self.grinding;
        self.grinding = grinding;
        if !started {
            return;
        }
        (self.left, self.right) = (false, false);
        if !ledge || (x * x + z * z).sqrt() < 0.001 {
            return;
        }
        // (velocity / |velocity|) x (0, 1, 0) = (-z, 0, x) / |velocity|; the scale leaves the sign alone.
        let dot = high[0] * -z + high[2] * x;
        self.left = 0.0 > dot;
        self.right = !self.left;
    }
}

struct StaticWorld;
impl MovingObstacleProvider for StaticWorld {
    fn collect(&mut self, _: [f32; 4], _: [f32; 4], _: f32, _: &mut [PathObstacle; 50]) -> usize {
        // The subject actor is excluded by the native provider. Our current
        // world contains this subject and static triangles, with no other actors.
        0
    }
}

/// Call after current state, board/Skeleton and animation conditioning have
/// published, before clearing AnimationControlOutput's per-frame intent bits.
///Selected state owners publish their output before this common consumer.
pub(crate) fn publish(
    skater: &SkaterRuntime,
    output: &skate_core::physics::phase::PhysicalOutputSnapshot,
    feedback: &PhysicalFeedback,
    com: CentreOfMassOutput,
    preferences: CameraPreferences,
    skater_animation_stance: u8,
    context: u32,
    tick: u64,
) -> Result<CameraPublicationInputs, String> {
    let p = &skater.player_input.processed;
    let physical = &skater.player_input.physical;
    let fields = &skater.animation_input.fields;
    let intents = skater.animation_input.output.flags;
    let packet = &skater.animation.packet;
    Ok(CameraPublicationInputs {
        tick,
        state: CameraStateOutput {
            height_32: physical.state.surface_height_32,
            physically_pushing_55: bit(p.flags_2468, 25),
            wiping_out_59: bit(p.flags_2468, 18),
            manual_60: u8::from(fields.balance != 0.0),
            reset_62: bit(p.flags_2472, 10),
            use_skeleton_root_75: u8::from(physical.state.category_12 == 500),
            flag_79: u8::from(p.state_variant_index_2528 == 3),
            flag_81: u8::from(skater.player_state.state_flags[81 - 52]),
        },
        animation: CameraAnimationOutput {
            conditioned_turn: feedback.conditioned_turn,
            input_turn_64: fields.turn,
            input_kickturn_68: p.spin_input_2672,
            time_since_input_128: p.time_since_last_input_2748,
            wipeout_tweak_148: physical.animation.profile_148,
            stance_155: u8::from(packet.riding_fakie),
            running_out_160: bit(p.flags_2480, 2),
            skater_animation_stance,
        },
        //Consume current output, including KnownAir82D36880's predicted
        //landing/apex. Other states retain their actual template fields.
        air: CameraAirOutput {
            apex_0: physical.air.trajectory_apex_0.map(f32::from_bits),
            landing_position_16: physical.air.collision_position_16.map(f32::from_bits),
            landing_normal_32: physical.air.landing_normal_32.map(f32::from_bits),
            launch_position_48: physical.air.selector_vector_48.map(f32::from_bits),
            heading_80: physical.air.landing_heading_80.map(f32::from_bits),
            time_176: physical.air.time_in_state_176,
            duration_180: physical.air.collision_time_180,
            apex_time_196: physical.air.time_to_apex_196,
            flag_440: bit(p.flags_2468, 22),
        },
        offboard: offboard_output(&physical.off_board),
        grinds: CameraGrindOutput {
            direction_0: physical.grinds.direction_0.map(f32::from_bits),
            camera_target_96: physical.grinds.camera_target_96.map(f32::from_bits),
            grinding_316: physical.grinds.grinding_316,
        },
        events: CameraEventsOutput {
            intent_51: bit(intents, 28),
            preparing_52: bit(intents, 27),
            dropping_in_63: bit(intents, 22),
            trick_125: bit(p.flags_2480, 11),
            //Completed State503 Fill82D4E0A8, consumed as Ground322 by the camera.
            hippy_jump_322: physical.ground.hippy_jumping_322,
            //Scoring2+200: reset82DE4468 each frame, set by the HoM scorer (hall_of_meat.rs).
            broken_bone_duration_200: skater.hall_of_meat.broken_bone_200,
            //AirCollector82DA78D0 publishes the world conditioner capability mask.
            capabilities_204: physical.scoring.capabilities_204,
        },
        damped_com_80: com.position,
        //82C03304..3340 copies actual BoardBody80 into Ground80 and96.
        ground_up_80: [
            output.ground_normal.x,
            output.ground_normal.y,
            output.ground_normal.z,
            0.0,
        ],
        //Ground288 resets82DE3728; selected ordinary Ground Fill leaves it.
        ground_scalar_288: 0.0,
        look_552_556: [
            skater.animation_input.extra.look_x,
            skater.animation_input.extra.look_y,
        ],
        collision_look_target_64: [
            output.predicted_position.x,
            output.predicted_position.y,
            output.predicted_position.z,
            0.0,
        ],
        preferences,
        context,
    })
}

fn bit(value: u32, shift: u32) -> u8 {
    ((value >> shift) & 1) as u8
}

///82DF6A68..6B8C copies the distinct OffBoard trajectory when byte331 is set.
///Keep the owner's actual availability byte: neither state category nor the
///existence of this packet establishes that a trajectory has been produced.
fn offboard_output(
    output: &skate_core::player::input_phase::OffBoardOutputFields,
) -> CameraOffboardOutput {
    CameraOffboardOutput {
        duration_92: output.scalar_92,
        time_152: output.scalar_152,
        apex_time_156: output.scalar_156,
        //82DF6B74..6B8C supplies OffBoard160 to setter52 at82DF6CA0.
        launch_normal_160: output.vector_160.map(f32::from_bits),
        launch_position_176: output.vector_176.map(f32::from_bits),
        landing_normal_192: output.vector_192.map(f32::from_bits),
        landing_position_208: output.vector_208.map(f32::from_bits),
        heading_224: output.vector_224.map(f32::from_bits),
        apex_240: output.vector_240.map(f32::from_bits),
        //82DF74E4 reads308; the camera packet's historical name is misleading.
        object_held_304: output.flag_308,
        hurdle_317: output.hippy_hurdling_317,
        use_trajectory_331: output.trajectory_valid_331,
        //82DF7474..7490 combines this byte with the dropping-in event.
        dropping_in_334: output.flag_334,
    }
}

#[cfg(test)]
#[path = "camera_output_tests.rs"]
mod tests;
