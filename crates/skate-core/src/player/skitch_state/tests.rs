use super::frames::spline_point;
use super::*;
use crate::player::offboard::grab_scene::record::Geometry;
use std::sync::Arc;

fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}

fn graph(hex: &str) -> PointGraph<8> {
    let w: Vec<f32> = (0..20)
        .map(|i| f(u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).unwrap()))
        .collect();
    PointGraph {
        x: std::array::from_fn(|i| w[4 + i]),
        y: std::array::from_fn(|i| w[12 + i]),
    }
}

/// physics_state_skitching/default as authored in the stock vault.
pub(super) fn stock() -> SkitchSettings {
    SkitchSettings {
        phase_0_time: 0.5,
        phase_2_time: 0.5,
        phase_2_fast_time: f(0x3e4c_cccd),
        phase_3_min_time: 0.5,
        phase_3_cooldown: 0.5,
        phase_3_max_time: 0.5,
        phase_3_exit_cooldown: 0.5,
        reach_margin: f(0x3ecc_cccd),
        fast_speed: 3.0,
        wheel_distance: f(0x3dcc_cccd),
        wheel_distance_airborne: f(0x3d4c_cccd),
        wheel_airborne_time: f(0x3dcc_cccd),
        velocity_filter: f(0x3dcc_cccd),
        shimmy_speed: 10.0,
        fast_line_speed: 40.0,
        line_speed_allowance: graph("42480000000000004348000040A0000042700000428C000042A0000042C8000042F00000430C000043200000433400004020000040200000403000004040000040600000408000004080000040800000"),
        shimmy_limit: graph("000000000000000043480000408000000000000042C8000042DC000042F0000043020000430C000043160000432000003F80000000000000000000000000000000000000000000000000000000000000"),
        position_lag: 0.0,
        phase_2_line_speed: 50.0,
        phase_3_reach: f(0x3f4c_cccd),
        hands_settled_rate: f(0x3ca3_d70a),
        hands_settled_distance: f(0x3dcc_cccd),
        hands_move_distance: f(0x3e4c_cccd),
        hands_move_time: 2.0,
        hands_step: f(0x3cf5_c28f),
        hands_follow: 0.25,
        left_angle_low: -60.0,
        right_angle_low: -35.0,
        left_angle_high: 35.0,
        right_angle_high: 60.0,
        hand_radius: f(0x3f19_999a),
        arm_length: f(0x3f33_3333),
        grab_offset: [0.0, f(0xbd75_c28f), f(0x3ccc_cccd), 0.0],
        yaw_lean: graph("0000000000000000424800004234000000000000416E6FBD41A76D3E41CBE8A741EF1682420EFF29422D4A38424800000000000041624928419F6DB641C2249441DFB6D842032491421E249442340000"),
        tow_offset_phase_2: f(0x3dcc_cccd),
        tow_offset: f(0x3e4c_cccd),
        tow: graph("C0000000C1A000004000000041A00000C0000000BF8DC24CBF86410BBF800000000000003F8000003FC0000040000000C1A00000C1A00000C1A00000C180000000000000408000004080000040800000"),
        tow_damping: f(0x3e4c_cccd),
        heading_rate: 180.0,
        heading_vs_time: graph("00000000000000003F8000003F333333000000003E856B913EBC74BF3ECD22323F092C323F3633B43F59A4473F80000000000000000000003F3333333F3333333F2CCCC93EE147B13E0A3D7100000000"),
        heading_vs_speed: graph("0000000000000000400000003F800000000000003D12C3213E0F6D403EB2731A3F1CC4CB3F4FA29C3FA731334000000000000000000000003E5B645A3F2DB6DD3F61D14E3F7507533F8000003F800000"),
        heading_fine: graph("00000000000000003F8000003F800000000000003DCCCCCD3E4CCCCD3E99999A3ECCCCCD3F0000003F4000003F8000003F0000003F0CCCCD3F19999A3F2666663F3333333F4CCCCD3F6666663F800000"),
        release_time: 0.5,
        release_kick_inner: -2.0,
        release_kick_outer: 2.5,
    }
}

/// A straight three-point record along +x, half length 1, identity frame.
fn record() -> Record {
    let mut w = [0u32; 72];
    let mut set = |at: usize, v: [f32; 4]| w[at..at + 4].copy_from_slice(&v.map(f32::to_bits));
    set(0, [1.0, 0.0, 0.0, 0.0]);
    set(4, [0.0, 1.0, 0.0, 0.0]);
    set(8, [0.0, 0.0, 1.0, 0.0]);
    set(12, [0.0, 0.0, 0.0, 1.0]);
    set(16, [-1.0, 0.0, 0.0, 1.0]);
    set(20, [1.0, 0.0, 0.0, 1.0]);
    set(28, [1.0, 0.0, 0.0, 0.0]);
    w[45] = 1.0f32.to_bits();
    w[47] = 2;
    w[48] = 7;
    w[49] = 1;
    w[60] = 1.0f32.to_bits();
    let points = vec![[-1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]];
    Record(w, Arc::new(Geometry { id: 1, points, approach_vectors: Vec::new(), word_60: 0 }))
}

fn processed() -> Processed {
    Processed {
        deck_forward_96: [0.0, 0.0, 1.0, 0.0],
        effective_right_128: [1.0, 0.0, 0.0, 0.0],
        effective_forward_160: [0.0, 0.0, 1.0, 0.0],
        effective_position_176: [0.25, 0.0, 0.5, 1.0],
        normal_464: [0.0, 1.0, 0.0, 0.0],
        flags_2476: 0x0040_0000,
        flags_2480: 0x0040_0000,
        flags_2488: 0,
        scalar_2612: 0.0,
        total_mass_2660: 80.0,
        state_timer_2664: 0.0,
        spin_2672: 0.0,
        turn_2676: 0.0,
        brake_2728: 0.0,
        world_grab_y_2816: 1.0,
        world_grab_z_2820: 0.3,
    }
}

fn entered() -> (SkitchState, SkitchSettings, Processed) {
    let s = stock();
    let p = processed();
    let mut state = SkitchState::default();
    state.enter(&s, record());
    state.begin_update(&p);
    state.set_input(&p);
    (state, s, p)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn spline_point_walks_the_authored_polyline() {
    let rec = record();
    assert!(near(spline_point(&rec, 0.5).0[0], 0.5));
    assert_eq!(spline_point(&rec, 2.0).0, rec.vector(0x50));
    assert_eq!(spline_point(&rec, -1.0).0, rec.vector(0x40));
    let mut reversed = rec.clone();
    reversed.0[50] |= 0x2000_0000;
    let (point, s) = spline_point(&reversed, 0.5);
    assert!(near(point[0], -0.5));
    assert!(near(s, 0.5));
}

#[test]
fn enter_copies_descriptor_and_resets() {
    let (state, s, _) = entered();
    assert_eq!((state.kind_520, state.id_524), (2, 7));
    assert_eq!(state.flags_541 & 0x80, 0x80);
    assert_eq!(state.cooldown_3fc, s.phase_0_time);
    assert_eq!([state.left_free_530, state.right_free_534], [1, 1]);
}

#[test]
fn first_frame_fills_history_and_projects_the_board() {
    let (mut state, s, p) = entered();
    state.update_frames(&s, &p);
    assert_eq!(state.frame_40, state.frame_c0);
    assert_eq!(state.frame_80, state.frame_c0);
    assert!(near(state.frame_c0[2][2], 1.0) && state.frame_c0[2][0] == 0.0);
    assert!(near(state.position_344, 0.25));
    assert!(near(state.distance_354, 0.5));
    assert_eq!(state.distance_rate_358, 0.0);
}

#[test]
fn facing_away_swaps_the_record_endpoints() {
    let (mut state, s, mut p) = entered();
    p.effective_right_128 = [-1.0, 0.0, 0.0, 0.0];
    state.update_frames(&s, &p);
    let rec = state.record_400.as_ref().unwrap();
    assert_eq!(rec.vector(0x40)[0], 1.0);
    assert_eq!(rec.vector(0x70)[0], -1.0);
    assert_eq!(rec.0[50] & 0x2000_0000, 0x2000_0000);
}

#[test]
fn phase_0_waits_for_its_hold_time() {
    let (mut state, s, p) = entered();
    state.update_frames(&s, &p);
    state.update_phase(&s);
    assert_eq!(state.phase_52c, 0);
    state.phase_time_3b4 = 0.6;
    state.update_phase(&s);
    assert_eq!(state.phase_52c, 1);
    assert_eq!(state.flags_540 & 0x08, 0x08);
    assert_eq!(state.phase_time_3b4, 0.0);
}

#[test]
fn release_kicks_on_the_second_frame_and_ends_on_the_fourth() {
    let (mut state, s, p) = entered();
    state.update_frames(&s, &p);
    state.phase_52c = 4;
    state.flags_540 |= 0x20;
    let mut kicks = Vec::new();
    for _ in 0..4 {
        assert_eq!(state.flags_540 & 0x10, 0);
        kicks.push(state.update_release(&s, &p));
    }
    assert_eq!(state.flags_540 & 0x10, 0x10);
    assert!(kicks[0].iter().all(Option::is_none));
    assert!(kicks[1][0].is_none() && kicks[1][1].is_some());
    assert!(kicks[2].iter().all(Option::is_none));
    assert_eq!(state.cooldown_3f8, s.release_time);
}

#[test]
fn letting_go_of_grab_releases() {
    let (mut state, s, mut p) = entered();
    p.flags_2476 = 0;
    state.update_release(&s, &p);
    assert_eq!(state.phase_52c, 4);
}

#[test]
fn yaw_lean_writes_a_y_rotation_once_past_two_degrees() {
    let (mut state, s, p) = entered();
    assert!(state.update_yaw_lean(&s, &p).is_none());
    state.heading_3ac = 0.5;
    let rows = state.update_yaw_lean(&s, &p).unwrap();
    let (sin, cos) = crate::trigonometry::sin_cos(state.yaw_lean_3b0);
    assert_eq!(rows[0], [cos, 0.0, -sin, cos]);
    assert_eq!(rows[3], [0.0; 4]);
    assert!(state.yaw_lean_3b0 > 0.0 && state.yaw_lean_3b0 <= 10.0f32.to_radians());
}

#[test]
fn braking_while_the_car_moves_sets_the_push_bit() {
    let (mut state, _, mut p) = entered();
    state.line_velocity_2f0 = [0.0, 0.0, 5.0, 0.0];
    state.update_pushes(&p);
    assert_eq!(state.flags_541 & 0x08, 0);
    p.brake_2728 = 1.0;
    state.update_pushes(&p);
    assert_eq!(state.flags_541 & 0x08, 0x08);
    assert!(state.publication(&p).pushing_53);
}

#[test]
fn hands_move_at_most_one_step_per_frame() {
    let (mut state, s, p) = entered();
    state.update_frames(&s, &p);
    state.flags_540 |= 0x08;
    state.hands_target_388 = 1.0;
    state.update_targets(&s);
    assert!(state.hands_step_390.abs() <= s.hands_step);
}
