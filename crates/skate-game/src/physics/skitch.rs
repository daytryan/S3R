//! PHYSICS_STATE_SKITCHING 104 lifecycle, TU3 vtable 0x82327398: Enter 82D47278,
//! Update 82D477C0, Fill 82D4C078, Exit none (82B61BB8). The pure state is
//! skate_core::player::skitch_state; this file supplies the live owners in
//! retail order. Not play-tested.
use super::{GamePhysics, SkaterRuntime};
use crate::physics::ground_runtime::GroundInputObservations;
use skate_core::{
    math::Vector3,
    physics::{
        board::BodyId, deck_angular_correction, force_queue::QueuedPointForce,
        manual::controller::{self, ManualAngleMeasurement},
    },
    player::{
        input_phase::PhysicalPlayerInput,
        offboard::grab_scene::Record,
        post_input::copy_grab_record_82762ab0,
        skitch_state::{HandInput, PointForce, Processed, Publication, SkitchSettings, SkitchState},
    },
    point_graph::PointGraph,
    riding::{
        anti_flip, collision_response::signed_angle, ground_correction_math, heading,
        slide_friction, speed_wobble, steering,
    },
};
use skate_data::collections::Collections;

pub(crate) struct Skitch {
    pub state: SkitchState,
    settings: SkitchSettings,
    /// The geometry behind ProcessedPhysIn+0x760, kept when 82762AB0 copies
    /// the first published grab record (the processed copy holds only words).
    pub published_1888: Option<Record>,
}

impl Skitch {
    pub fn load(data: &Collections) -> Result<Self, String> {
        Ok(Self {
            state: SkitchState::default(),
            settings: load_settings(data)?,
            published_1888: None,
        })
    }
}

const CLASS: &str = "physics_state_skitching";

fn load_settings(data: &Collections) -> Result<SkitchSettings, String> {
    let f = |hash: &str| data.float(CLASS, "default", &format!("Hash_{hash}"));
    let graph = |hash: &str| -> Result<PointGraph<8>, String> {
        let w = data.words::<20>(CLASS, "default", &format!("Hash_{hash}"))?;
        Ok(PointGraph {
            x: std::array::from_fn(|i| f32::from_bits(w[4 + i])),
            y: std::array::from_fn(|i| f32::from_bits(w[12 + i])),
        })
    };
    let offset = data.words::<4>(CLASS, "default", "Hash_858D610290DE4F96")?;
    Ok(SkitchSettings {
        phase_0_time: f("CC257F85657759A1")?,
        phase_2_time: f("81A1FC1E9C2B44D7")?,
        phase_2_fast_time: f("08FE14711B1C2BFA")?,
        phase_3_min_time: f("D60A7AE08B00E714")?,
        phase_3_cooldown: f("44EE9C498EE7C3C2")?,
        phase_3_max_time: f("BA1984761D057860")?,
        phase_3_exit_cooldown: f("F5F9D6CF4D6AC6D1")?,
        reach_margin: f("60272903AD60FF3B")?,
        fast_speed: f("1F85F500908C5E17")?,
        wheel_distance: f("40AAD3FD99B464F4")?,
        wheel_distance_airborne: f("F178F963558D24A0")?,
        wheel_airborne_time: f("03A59CDFA0B0B967")?,
        velocity_filter: f("1EA52A4BDF8964F7")?,
        shimmy_speed: f("E4965C6A27F1FF96")?,
        fast_line_speed: f("30DAD137EE602142")?,
        line_speed_allowance: graph("466FC01CF6CA2FD2")?,
        shimmy_limit: graph("7FA1FB6630F75C67")?,
        position_lag: f("6C62538D99512F75")?,
        phase_2_line_speed: f("007CF9EAE1F64C3C")?,
        phase_3_reach: f("AED1D7B20DA7CBBA")?,
        hands_settled_rate: f("1F5F8C47EF1C7C97")?,
        hands_settled_distance: f("4CC8C0D741C121D4")?,
        hands_move_distance: f("8FEF0AD38542CECD")?,
        hands_move_time: f("CFBBDDD08082CF69")?,
        hands_step: f("EA885312130505B0")?,
        hands_follow: f("FF6B65AF360C6707")?,
        left_angle_low: f("E2DC234CA3A588E9")?,
        right_angle_low: f("C6C3D74A4A964B3D")?,
        left_angle_high: f("11B7A22CEE198ED8")?,
        right_angle_high: f("C50525FE8E95D076")?,
        hand_radius: f("2C7533BE2CFAD9B0")?,
        arm_length: f("EFF730F7546C6422")?,
        // Math::Vector3; 82D4A378 uses the w lane as stored (stock 0).
        grab_offset: offset.map(f32::from_bits),
        yaw_lean: graph("30FDFCE185CD3D4F")?,
        tow_offset_phase_2: f("95D9DE65A73BB685")?,
        tow_offset: f("B7A817055629EC4B")?,
        tow: graph("00A1B0B33F1BA1BF")?,
        tow_damping: f("71A894F6AD65C859")?,
        heading_rate: f("0B199E27AF7B96A8")?,
        heading_vs_time: graph("3F79D445A24E12BA")?,
        heading_vs_speed: graph("6FB7A3D992163663")?,
        heading_fine: graph("1FF78FAE8AD467FC")?,
        release_time: f("659537728CE30B02")?,
        release_kick_inner: f("58B697CA9FF570E4")?,
        release_kick_outer: f("8524E22B25D7D0D7")?,
    })
}

/// 82762AB0 into the retained S+0x400 words, with the published geometry.
fn copy_record(skater: &SkaterRuntime, into: Option<&Record>) -> Result<Record, String> {
    let source = &skater.player_input.processed.grab_records_1888_2176[0];
    let geometry = skater
        .skitch
        .published_1888
        .as_ref()
        .filter(|r| r.0[49] == source[49])
        .ok_or("Skitch needs the published grab record's geometry")?
        .1
        .clone();
    let mut words = into.map_or([0; 72], |r| r.0);
    copy_grab_record_82762ab0(&mut words, source);
    Ok(Record(words, geometry))
}

/// Enter82D47278. The board drive disable is (S+4)+0x122 = 0 with the
/// X+0x3C0..0x3DC resets, the same call Slide's Enter makes.
pub(crate) fn enter(physics: &mut GamePhysics, skater: &mut SkaterRuntime) -> Result<(), String> {
    physics
        .board
        .hook_mut()
        .drive
        .disable_animation(&mut skater.ground_lifecycle.board_animated_290);
    let record = copy_record(skater, skater.skitch.state.record_400.as_ref())?;
    let settings = skater.skitch.settings;
    skater.skitch.state.enter(&settings, record);
    Ok(())
}

fn processed(skater: &SkaterRuntime) -> Result<Processed, String> {
    let p = &skater.player_input.processed;
    let t = skater
        .player_input
        .toolkit
        .as_ref()
        .ok_or("Skitch requires current BoardToolkit")?;
    let f = &skater.animation_input.fields;
    Ok(Processed {
        deck_forward_96: t.deck[2],
        effective_right_128: t.effective[0],
        effective_forward_160: t.effective[2],
        effective_position_176: t.effective[3],
        normal_464: raw(p.vectors_464_480_496_512_528[0]),
        flags_2476: p.flags_2476,
        flags_2480: p.flags_2480,
        flags_2488: p.flags_2488,
        scalar_2612: p.scalar_2612,
        total_mass_2660: t.total_mass,
        state_timer_2664: p.state_timer_2664,
        spin_2672: p.spin_input_2672,
        turn_2676: f.turn,
        brake_2728: f.brake,
        world_grab_y_2816: skater.animation_input.extra.world_grab_y,
        world_grab_z_2820: skater.animation_input.extra.world_grab_z,
    })
}

/// Update82D477C0.
pub(crate) fn update(physics: &mut GamePhysics, skater: &mut SkaterRuntime) -> Result<(), String> {
    if skater.skitch.state.record_400.is_none() {
        return Err("Skitch::Enter must copy the grab record before Update".into());
    }
    let s = skater.skitch.settings;
    let proc = processed(skater)?;
    let ready = skater.skitch.state.begin_update(&proc);
    skater.skeleton_collision.disable_handplant_contacts(5);
    if ready {
        let record = copy_record(skater, skater.skitch.state.record_400.as_ref())?;
        let state = &mut skater.skitch.state;
        state.record_400 = Some(record);
        state.update_wheel_contact(
            &s,
            physics.riding.wheel_lines.minimum_distance,
            physics.riding.ground.time_without_wheel_contact,
        );
        state.set_input(&proc);
        state.update_frames(&s, &proc);
        state.update_phase(&s);
        state.integrate(&s, &proc);
        state.update_targets(&s);
        let originals = &skater.foot_ik.originals_14160;
        let hands = state.update_reach(
            &s,
            &proc,
            &HandInput {
                animation_to_world: skater.animated_skeleton.roots.animation_to_world,
                left_hand_3: originals[3][3],
                left_arm_5: originals[5][3],
                right_hand_7: originals[7][3],
                right_arm_9: originals[9][3],
            },
        );
        for hand in hands.into_iter().flatten() {
            let ik = &mut skater.foot_ik.state;
            ik.external_targets[hand.limb].world_position = hand.world_position;
            ik.limbs[hand.limb].external_target_set = true;
            ik.limbs[hand.limb].target_blend = hand.blend;
        }
        board_forces(physics, skater, &proc)?;
        if let Some(rows) = skater.skitch.state.update_yaw_lean(&s, &proc) {
            skater.animated_skeleton.board_offset.refresh_orientation(rows);
        }
        skater.skitch.state.update_pushes(&proc);
        // 82585DD8 + vcall 0x2C: request S+0x4BC's record again. The vcall 0x28
        // notify (8275FF90) reaches only the provider objects. Kind 1 (cars,
        // not published yet): 82C361E8 -> car vfunc 0x7C(1) sets +0x1132 bit
        // 0x02, and for LW's own player vfunc 0x80(1) sets +0x1133 bit 0x80;
        // 82C34CD0 latches/clears both per frame, 82C34B30 reacts (see
        // engine-defects.md §11). Kind 2 is LivingWorld+0x5790's +0x1C face
        // (VT 0x823232FC, installed at 828581E0): 82C4C450 finds the entity by
        // id and calls its vfunc 0x78(1); the tow/slide forces go through
        // 82C4C370/3E0 to entity vfuncs 0x54/0x50. Port props are moved only
        // while held (502 mover), so these have no owner to reach.
        // ponytail: add the prop sinks when props get a free rigid-body owner.
        if let Some(record) = &skater.skitch.state.record_400 {
            skater.offboard_grab.request_primary(record.descriptor());
        }
    }
    let kicks = skater.skitch.state.update_release(&s, &proc);
    for kick in kicks.into_iter().flatten() {
        queue(physics, 6, kick);
    }
    let p = &skater.player_input.processed;
    let t = skater
        .player_input
        .toolkit
        .as_ref()
        .ok_or("Skitch requires current BoardToolkit")?;
    // 82D8E5C0, seeds +0x470/+0x4B0 from P+0x210/P+0x60, 82D8C8F0: Slide's sequence.
    physics.riding.update_slide_reckoning(
        p,
        t,
        skater.animation_input.extra.physical_body_spin,
        skater.animation_input.fields.balance,
    );
    super::input_phase::update_skeleton_ground(physics, skater)?;
    skater.skitch.state.end_update();
    Ok(())
}

/// 82D4AC38: Ground's board services around the skitch tow and heading drive.
fn board_forces(
    physics: &mut GamePhysics,
    skater: &mut SkaterRuntime,
    proc: &Processed,
) -> Result<(), String> {
    // 82C04368 on the current board animation target.
    let target = skater.animated_skeleton.board_frames.animation_target;
    skater
        .skeleton_air
        .capture_physics_error(&physics.board, &target);
    let p = &skater.player_input.processed;
    let t = skater
        .player_input
        .toolkit
        .as_ref()
        .ok_or("Skitch requires current BoardToolkit")?;
    let edge = skater.ground_lifecycle.edge;
    let mut input = skater.ground_settings.input(
        t,
        p,
        &skater.animation_input,
        &skater.ground.pumping,
        skater
            .ground
            .pumping_settings
            .mode(p.state_variant_index_2528)?
            .unintentional_scalar,
        &physics.riding,
        &skater.animated_skeleton,
        physics.settings.step.base_truck_transforms,
        GroundInputObservations {
            manual_drag_2724: skater.ground_lifecycle.manual_drag_2724,
            trajectory_state_bits: p.external_physics_1616.flags,
            edge_flags: edge.map_or(0, |e| e.flags),
            edge_point: edge.map_or([0.0; 4], |e| e.point),
        },
    );
    let settings = skater.ground_settings.board();
    let s = skater.skitch.settings;
    // sk+0x3788: originals[0].row3.z.
    let root_z = skater.foot_ik.originals_14160[0][3][2];
    let state = &mut skater.skitch.state;
    let skitch = state.board_forces(&s, proc, root_z);
    let away = state.flags_541 & 0x20 != 0;
    // 82D92970 with S3B4; pure, so computing it before the tilt keeps order.
    input.slide_friction.heading_time = state.phase_time_3b4;
    let slide = skitch.map(|_| slide_friction::calculate(settings.slide_friction, &input.slide_friction));
    // 82D92440(T, P, 0, 0) then 82C03638 with |Skeleton11008|.
    let mut tilt = if away {
        0.0
    } else {
        steering::calculate_tilt(settings.steering, input.steering, None, None)
    };
    if !away {
        input.speed_wobble.tilt = tilt;
        input.speed_wobble.center_of_mass_height = ground_correction_math::center_of_mass_height(
            skater.animated_skeleton.record.com_to_deck_world,
        );
        tilt = speed_wobble::calculate(&mut skater.ground.wobble, settings.speed_wobble, input.speed_wobble);
    }
    let Some(skitch) = skitch else {
        // Phase 4: only the truck targets.
        skater.ground.steering.update(tilt, settings.steering.tilt_blending, p.flags_2468, p.flags_2472);
        return Ok(());
    };
    let anti_flip = anti_flip::calculate(settings.anti_flip, &input.anti_flip);
    skater.ground.steering.update(tilt, settings.steering.tilt_blending, p.flags_2468, p.flags_2472);
    // P+0xAA0 and P+0xA70 both nonzero: 82D92CF8 with (S+0x14)+0x2F0 and body+0x11C.
    if input.heading.balance != 0.0 && p.spin_input_2672 != 0.0 {
        let h = heading::calculate(settings.heading, &input.heading, &mut skater.ground.heading_previous);
        skater.ground_runtime.apply_angular_target(&mut physics.board, h);
    }
    input.manual.powersliding = false;
    let manual = controller::calculate(
        &mut skater.ground.manual,
        settings.manual,
        settings.manual_mode,
        &input.manual,
        &mut Angle,
    )
    .map_err(|e| format!("Skitch manual controller: {e:?}"))?;
    skater
        .ground_runtime
        .apply_angular_displacement(&mut physics.board, manual.angular_displacement);
    skater
        .ground_runtime
        .apply_angular_displacement(&mut physics.board, anti_flip);
    if let Some(drive) = skitch.drive {
        deck_angular_correction::apply_axis_displacement(
            &mut physics.board.bodies_mut()[BodyId::Deck.index()].rates,
            xyz(drive),
        );
    }
    queue(physics, 6, skitch.tow);
    if let Some(f) = slide {
        physics.board.forces_mut().append(QueuedPointForce {
            tag: 1,
            force_world: Vector3::new(f[0], f[1], f[2]),
            point_body: Vector3::new(f[4], f[5], f[6]),
        });
    }
    skater.skitch.state.finish_board_forces();
    Ok(())
}

/// Fill82D4C078 values; `fill` stores them and the caller sets State+53.
pub(crate) fn publication(skater: &SkaterRuntime) -> Result<Publication, String> {
    Ok(skater.skitch.state.publication(&processed(skater)?))
}
pub(crate) fn fill(out: &Publication, physical: &mut PhysicalPlayerInput) {
    if out.holding_304 {
        physical.off_board.flag_304 = 1;
    }
    physical.animation.crouch_132 = out.crouch_132;
    physical.animation.hands_speed_136 = out.hands_speed_136;
    physical.animation.hands_set_140 = out.hands_set_140;
    physical.ground.scalar_280 = out.hand_height_280;
    physical.ground.scalar_284 = out.absorb_284;
    physical.ground.scalar_288 = out.line_fraction_288;
    physical.ground.scalar_292 = out.cooldown_292;
    physical.ground.scalar_308 = out.push_speed_308;
    physical.state.counter_36 = out.kind_36;
    physical.state.skitch_value_40 = out.id_40;
}

fn queue(physics: &mut GamePhysics, tag: u32, f: PointForce) {
    physics.board.forces_mut().append(QueuedPointForce {
        tag,
        force_world: xyz(f.force),
        point_body: xyz(f.point),
    });
}

struct Angle;
impl ManualAngleMeasurement for Angle {
    type Error = std::convert::Infallible;
    fn angle_between(&mut self, a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> Result<f32, Self::Error> {
        Ok(signed_angle(xyz(a), xyz(b), xyz(c)))
    }
}
fn xyz(v: [f32; 4]) -> Vector3 {
    Vector3::new(v[0], v[1], v[2])
}
fn raw(v: [u32; 4]) -> [f32; 4] {
    v.map(f32::from_bits)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires extracted private stock collections"]
    fn stock_skitch_settings_load_without_defaults() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let data = skate_data::collections::Collections::load(&root).unwrap();
        let s = super::load_settings(&data).unwrap();
        assert_eq!((s.heading_rate, s.release_kick_inner, s.release_kick_outer), (180.0, -2.0, 2.5));
        assert_eq!(s.tow_damping.to_bits(), 0x3e4c_cccd);
        assert_eq!(s.grab_offset[1].to_bits(), 0xbd75_c28f);
        assert_eq!(s.heading_vs_speed.y[7], 1.0);
        super::super::ground_runtime::GroundState::load(&data, "default", true).unwrap();
    }
}
