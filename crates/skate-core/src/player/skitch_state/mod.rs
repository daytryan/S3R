//! TU3 PHYSICS_STATE_SKITCHING (104), vtable 0x82327398. Pure state S and its
//! Update callees; the game adapter supplies live owners in Update82D477C0 order.
//! Field suffixes are the native S offsets. Not play-tested.
mod forces;
mod frames;
pub mod ground;
mod line;
pub mod math;
mod reach;
#[cfg(test)]
mod tests;

pub use forces::{BoardForces, PointForce};
pub use reach::{HandInput, HandTarget};

use crate::player::offboard::grab_scene::Record;
use crate::point_graph::PointGraph;
use math::{Frame, IDENTITY, Vector};

/// physics_state_skitching/default values. Retail falls back to the zero block
/// 0x830D0850 on a miss; the stock vault defines every key read here.
#[derive(Clone, Copy, Debug)]
pub struct SkitchSettings {
    /// Hash_CC257F85657759A1: enter S3FC, phase-0 hold time.
    pub phase_0_time: f32,
    /// Hash_81A1FC1E9C2B44D7.
    pub phase_2_time: f32,
    /// Hash_08FE14711B1C2BFA.
    pub phase_2_fast_time: f32,
    /// Hash_D60A7AE08B00E714.
    pub phase_3_min_time: f32,
    /// Hash_44EE9C498EE7C3C2.
    pub phase_3_cooldown: f32,
    /// Hash_BA1984761D057860.
    pub phase_3_max_time: f32,
    /// Hash_F5F9D6CF4D6AC6D1.
    pub phase_3_exit_cooldown: f32,
    /// Hash_60272903AD60FF3B.
    pub reach_margin: f32,
    /// Hash_1F85F500908C5E17.
    pub fast_speed: f32,
    /// Hash_40AAD3FD99B464F4, Hash_F178F963558D24A0, Hash_03A59CDFA0B0B967.
    pub wheel_distance: f32,
    pub wheel_distance_airborne: f32,
    pub wheel_airborne_time: f32,
    /// Hash_1EA52A4BDF8964F7 (82D37250).
    pub velocity_filter: f32,
    /// Hash_E4965C6A27F1FF96 (82D372A8).
    pub shimmy_speed: f32,
    /// Hash_30DAD137EE602142.
    pub fast_line_speed: f32,
    /// Hash_466FC01CF6CA2FD2.
    pub line_speed_allowance: PointGraph<8>,
    /// Hash_7FA1FB6630F75C67 (82D37300).
    pub shimmy_limit: PointGraph<8>,
    /// Hash_6C62538D99512F75.
    pub position_lag: f32,
    /// Hash_007CF9EAE1F64C3C.
    pub phase_2_line_speed: f32,
    /// Hash_AED1D7B20DA7CBBA.
    pub phase_3_reach: f32,
    /// Hash_1F5F8C47EF1C7C97, Hash_4CC8C0D741C121D4.
    pub hands_settled_rate: f32,
    pub hands_settled_distance: f32,
    /// Hash_8FEF0AD38542CECD, Hash_CFBBDDD08082CF69.
    pub hands_move_distance: f32,
    pub hands_move_time: f32,
    /// Hash_EA885312130505B0, Hash_FF6B65AF360C6707.
    pub hands_step: f32,
    pub hands_follow: f32,
    /// Hash_E2DC234CA3A588E9, Hash_C6C3D74A4A964B3D, Hash_11B7A22CEE198ED8, Hash_C50525FE8E95D076.
    pub left_angle_low: f32,
    pub right_angle_low: f32,
    pub left_angle_high: f32,
    pub right_angle_high: f32,
    /// Hash_2C7533BE2CFAD9B0, Hash_EFF730F7546C6422.
    pub hand_radius: f32,
    pub arm_length: f32,
    /// Hash_858D610290DE4F96 (Math::Vector3, w = 0).
    pub grab_offset: Vector,
    /// Hash_30FDFCE185CD3D4F.
    pub yaw_lean: PointGraph<8>,
    /// Hash_95D9DE65A73BB685, Hash_B7A817055629EC4B.
    pub tow_offset_phase_2: f32,
    pub tow_offset: f32,
    /// Hash_00A1B0B33F1BA1BF.
    pub tow: PointGraph<8>,
    /// Hash_71A894F6AD65C859.
    pub tow_damping: f32,
    /// Hash_0B199E27AF7B96A8 (degrees per second).
    pub heading_rate: f32,
    /// Hash_3F79D445A24E12BA, HeadingAdjustVsSpeed, Hash_1FF78FAE8AD467FC.
    pub heading_vs_time: PointGraph<8>,
    pub heading_vs_speed: PointGraph<8>,
    pub heading_fine: PointGraph<8>,
    /// Hash_659537728CE30B02.
    pub release_time: f32,
    /// Hash_58B697CA9FF570E4, Hash_8524E22B25D7D0D7.
    pub release_kick_inner: f32,
    pub release_kick_outer: f32,
}

/// Native S. The record is the 82762AB0 copy at S+0x400 with its geometry.
#[derive(Clone, Debug)]
pub struct SkitchState {
    pub record_400: Option<Record>,
    pub frame_40: Frame,
    pub frame_80: Frame,
    pub frame_c0: Frame,
    pub frame_100: Frame,
    pub frame_140: Frame,
    pub frame_180: Frame,
    pub frame_1c0: Frame,
    pub frame_210: Frame,
    pub board_250: Vector,
    pub vector_260: Vector,
    pub vector_270: Vector,
    pub vector_280: Vector,
    pub vector_290: Vector,
    pub vector_2a0: Vector,
    pub vector_2b0: Vector,
    pub right_hand_2c0: Vector,
    pub left_hand_2d0: Vector,
    pub vector_2e0: Vector,
    pub line_velocity_2f0: Vector,
    pub vector_300: Vector,
    pub drive_310: Vector,
    pub normal_320: Vector,
    pub tow_force_330: Vector,
    pub speed_340: f32,
    pub position_344: f32,
    pub position_rate_348: f32,
    pub scalar_34c: f32,
    pub scalar_350: f32,
    pub distance_354: f32,
    pub distance_rate_358: f32,
    pub scalar_35c: f32,
    pub target_360: f32,
    pub line_speed_364: f32,
    pub scalar_368: f32,
    pub scalar_36c: f32,
    pub scalar_370: f32,
    pub scalar_374: f32,
    pub scalar_378: f32,
    pub scalar_37c: f32,
    pub scalar_380: f32,
    pub end_384: f32,
    pub hands_target_388: f32,
    pub hands_38c: f32,
    pub hands_step_390: f32,
    pub left_reach_394: f32,
    pub right_reach_398: f32,
    pub steer_39c: f32,
    pub half_length_3a0: f32,
    pub reach_3a4: f32,
    pub line_fraction_3a8: f32,
    pub heading_3ac: f32,
    pub yaw_lean_3b0: f32,
    pub phase_time_3b4: f32,
    pub angle_3b8: f32,
    pub absorb_3bc: f32,
    pub fast_time_3c0: f32,
    pub hands_time_3c4: f32,
    pub left_blend_3c8: f32,
    pub right_blend_3cc: f32,
    pub left_goal_3d0: f32,
    pub right_goal_3d4: f32,
    pub hands_set_3d8: u32,
    pub crouch_3dc: f32,
    pub time_3e0: f32,
    pub push_speed_3e4: f32,
    pub direction_3e8: i32,
    pub word_3ec: u32,
    pub scalar_3f0: f32,
    pub scalar_3f4: f32,
    pub cooldown_3f8: f32,
    pub cooldown_3fc: f32,
    pub kind_520: u32,
    pub id_524: u32,
    pub release_frames_528: i32,
    pub phase_52c: u32,
    pub left_free_530: u32,
    pub right_free_534: u32,
    pub left_free_538: u32,
    pub right_free_53c: u32,
    pub flags_540: u8,
    pub flags_541: u8,
}

impl Default for SkitchState {
    /// Constructor state; 82D47318 rewrites everything Update reads.
    fn default() -> Self {
        let z = [0.0; 4];
        Self {
            record_400: None,
            frame_40: IDENTITY,
            frame_80: IDENTITY,
            frame_c0: IDENTITY,
            frame_100: IDENTITY,
            frame_140: IDENTITY,
            frame_180: IDENTITY,
            frame_1c0: IDENTITY,
            frame_210: IDENTITY,
            board_250: z,
            vector_260: z,
            vector_270: z,
            vector_280: z,
            vector_290: z,
            vector_2a0: z,
            vector_2b0: z,
            right_hand_2c0: z,
            left_hand_2d0: z,
            vector_2e0: z,
            line_velocity_2f0: z,
            vector_300: z,
            drive_310: z,
            normal_320: z,
            tow_force_330: z,
            speed_340: 0.0,
            position_344: 0.0,
            position_rate_348: 0.0,
            scalar_34c: 0.0,
            scalar_350: 0.0,
            distance_354: 0.0,
            distance_rate_358: 0.0,
            scalar_35c: 0.0,
            target_360: 0.0,
            line_speed_364: 0.0,
            scalar_368: 0.0,
            scalar_36c: 0.0,
            scalar_370: 0.0,
            scalar_374: 0.0,
            scalar_378: 0.0,
            scalar_37c: 0.0,
            scalar_380: 0.0,
            end_384: 0.0,
            hands_target_388: 0.0,
            hands_38c: 0.0,
            hands_step_390: 0.0,
            left_reach_394: 0.0,
            right_reach_398: 0.0,
            steer_39c: 0.0,
            half_length_3a0: 0.0,
            reach_3a4: 0.0,
            line_fraction_3a8: 0.0,
            heading_3ac: 0.0,
            yaw_lean_3b0: 0.0,
            phase_time_3b4: 0.0,
            angle_3b8: 0.0,
            absorb_3bc: 0.0,
            fast_time_3c0: 0.0,
            hands_time_3c4: 0.0,
            left_blend_3c8: 0.0,
            right_blend_3cc: 0.0,
            left_goal_3d0: 0.0,
            right_goal_3d4: 0.0,
            hands_set_3d8: 0,
            crouch_3dc: 0.0,
            time_3e0: 0.0,
            push_speed_3e4: 0.0,
            direction_3e8: 0,
            word_3ec: 0,
            scalar_3f0: 0.0,
            scalar_3f4: 0.0,
            cooldown_3f8: 0.0,
            cooldown_3fc: 0.0,
            kind_520: 0,
            id_524: 0,
            release_frames_528: 0,
            phase_52c: 0,
            left_free_530: 0,
            right_free_534: 0,
            left_free_538: 0,
            right_free_53c: 0,
            flags_540: 0,
            flags_541: 0,
        }
    }
}

/// ProcessedPhysIn fields read by Update and its callees.
#[derive(Clone, Copy, Debug)]
pub struct Processed {
    /// P+0x60, P+0x80, P+0xA0, P+0xB0: deck row 2 and effective rows 0/2/3.
    pub deck_forward_96: Vector,
    pub effective_right_128: Vector,
    pub effective_forward_160: Vector,
    pub effective_position_176: Vector,
    /// P+0x1D0 vectors_464[0].
    pub normal_464: Vector,
    pub flags_2476: u32,
    pub flags_2480: u32,
    pub flags_2488: u32,
    pub scalar_2612: f32,
    pub total_mass_2660: f32,
    pub state_timer_2664: f32,
    pub spin_2672: f32,
    pub turn_2676: f32,
    pub brake_2728: f32,
    /// P+0xB00/0xB04 world grab y/z attributes.
    pub world_grab_y_2816: f32,
    pub world_grab_z_2820: f32,
}

/// Fill82D4C078 values for PhysOut.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Publication {
    /// OffBoard+304. Fill stores 1 when true and never stores 0.
    pub holding_304: bool,
    /// Animation+132/136/140.
    pub crouch_132: f32,
    pub hands_speed_136: f32,
    pub hands_set_140: u32,
    /// Ground+280/284/288/292/308.
    pub hand_height_280: f32,
    pub absorb_284: f32,
    pub line_fraction_288: f32,
    pub cooldown_292: f32,
    pub push_speed_308: f32,
    /// State+36/40/53.
    pub kind_36: u32,
    pub id_40: u32,
    pub pushing_53: bool,
}

impl SkitchState {
    /// Enter82D47278 after 82D47318; `record` is the 82762AB0 copy of
    /// ProcessedPhysIn+0x760. The board drive disable is the caller's.
    pub fn enter(&mut self, settings: &SkitchSettings, record: Record) {
        self.reset(settings);
        self.kind_520 = record.0[47];
        self.id_524 = record.0[48];
        self.record_400 = Some(record);
        self.flags_540 &= !0x10;
    }

    /// 82D47318. S+0x180 and the record copy are not touched.
    fn reset(&mut self, settings: &SkitchSettings) {
        let record = self.record_400.take();
        let frame_180 = self.frame_180;
        let flags = (self.flags_540 & 0x04, self.flags_541 & 0x87);
        *self = Self {
            record_400: record,
            frame_180,
            left_free_530: 1,
            right_free_534: 1,
            left_free_538: 1,
            right_free_53c: 1,
            flags_540: flags.0,
            flags_541: flags.1 | 0x80,
            cooldown_3fc: settings.phase_0_time,
            // 0x368 is not written by the reset.
            scalar_368: self.scalar_368,
            ..Self::default()
        };
    }

    /// Update82D477C0 prefix: record readiness (2480 bit22) into S540 bit 0x80.
    pub fn begin_update(&mut self, p: &Processed) -> bool {
        self.flags_540 = (self.flags_540 & 0x7f) | if p.flags_2480 & 0x0040_0000 != 0 { 0x80 } else { 0 };
        self.flags_540 & 0x80 != 0
    }

    /// 82D47FC0: S541 bit 0x20 from SkateboardBody+856 and +7692.
    pub fn update_wheel_contact(&mut self, s: &SkitchSettings, wheel_distance_856: f32, airborne_7692: f32) {
        let away = wheel_distance_856 > s.wheel_distance
            || (wheel_distance_856 > s.wheel_distance_airborne && airborne_7692 > s.wheel_airborne_time);
        self.flags_541 = (self.flags_541 & !0x20) | if away { 0x20 } else { 0 };
    }

    /// Update82D47824..78C4: steer input S39C and normal S320.
    pub fn set_input(&mut self, p: &Processed) {
        let (turn, spin) = (p.turn_2676, p.spin_2672);
        self.steer_39c = turn;
        if !math::opposite_sign(turn, spin) {
            let s = if turn > 0.0 { 1.0 } else { -1.0 };
            let (a, b) = (s * turn, s * spin);
            self.steer_39c = math::fsel(a - b, a, b) * s;
        }
        self.normal_320 = p.normal_464;
    }

    /// Update82D479B4: clears the first-frame bit after the whole update.
    pub fn end_update(&mut self) {
        self.flags_541 &= 0x7f;
    }

    /// Fill82D4C078.
    pub fn publication(&self, p: &Processed) -> Publication {
        // *S204 is always S+0x80 after Enter/82D48148.
        let f = &self.frame_80;
        let hand = math::madd(f[0], self.hands_38c, f[3]);
        Publication {
            holding_304: self.flags_540 & 0x10 == 0 && self.flags_540 & 0x80 != 0,
            crouch_132: self.crouch_3dc,
            hands_speed_136: self.hands_step_390 * math::RATE,
            hands_set_140: self.hands_set_3d8,
            hand_height_280: hand[1] - p.effective_position_176[1],
            absorb_284: self.absorb_3bc,
            line_fraction_288: self.line_fraction_3a8,
            cooldown_292: self.cooldown_3f8,
            push_speed_308: self.push_speed_3e4,
            kind_36: self.kind_520,
            id_40: self.id_524,
            pushing_53: self.flags_541 & 0x08 != 0,
        }
    }

    fn record(&self) -> &Record {
        self.record_400
            .as_ref()
            .expect("Skitch Enter copies the grab record before Update")
    }
}
