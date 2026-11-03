//! 82D4A378 hand reach, crouch (S3DC) and the 82BD9728/82BD97D0 hand IK targets.
use super::frames::spline_point;
use super::math::*;
use super::{Processed, SkitchSettings, SkitchState};
use crate::animation::foot_ik::drive::clamp_point;

/// Skeleton reads: sk+0x2E90 and ProcessData14160 originals[3|5|7|9].row3
/// (animation space; 3/7 hands, 5/9 the arm parts the IK clamps against).
#[derive(Clone, Copy, Debug)]
pub struct HandInput {
    pub animation_to_world: Frame,
    pub left_hand_3: Vector,
    pub left_arm_5: Vector,
    pub right_hand_7: Vector,
    pub right_arm_9: Vector,
}

/// SkeletonIK writes: external_targets[limb].world_position (+0x240/+0x250),
/// limbs[limb].external_target_set (+0x326/+0x327) and target_blend (+0x334/+0x338).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandTarget {
    pub limb: usize,
    pub world_position: Vector,
    pub blend: f32,
}

impl SkitchState {
    /// 82D4A378. Returns the left (limb 2) then right (limb 3) IK writes.
    pub fn update_reach(&mut self, s: &SkitchSettings, p: &Processed, h: &HandInput) -> [Option<HandTarget>; 2] {
        let deg = self.heading_3ac * f32::from_bits(0x4265_2ee1);
        let limit = s.arm_length + s.hand_radius;
        if self.phase_52c == 0 {
            self.left_free_530 = 0;
            self.right_free_534 = 0;
        }
        if deg > s.left_angle_high || !(deg >= s.left_angle_low) {
            self.left_free_530 = 1;
        }
        if deg > s.right_angle_high || !(deg >= s.right_angle_low) {
            self.right_free_534 = 1;
        }
        let grab = [0.0, p.world_grab_y_2816, p.world_grab_z_2820, 0.0];
        let base = add(grab, s.grab_offset);
        let (mut l, mut r) = (h.left_hand_3, h.right_hand_7);
        if p.flags_2476 & 0x04 != 0 {
            l = [-l[0], l[1], -l[2], l[3]];
            r = [-r[0], r[1], -r[2], r[3]];
        }
        let half = self.half_length_3a0;
        let sl = clamp(self.hands_38c + sub(l, base)[0], -half, half);
        let sr = clamp(self.hands_38c + sub(r, base)[0], -half, half);
        // The three spline reads do not depend on S3DC, so they run first here.
        let point = |at: f32| transform(&self.frame_180, spline_point(self.record(), at).0);
        let (hands, wl, wr) = (point(self.hands_38c), point(sl), point(sr));
        let anchor = transform(&h.animation_to_world, grab);
        let crouch = self.crouch_3dc + (anchor[1] - hands[1]) * 0.1;
        self.crouch_3dc = clamp(crouch, 0.0, 0.8);

        let a = &h.animation_to_world;
        let c = &self.frame_1c0;
        let row = |v: Vector| madd(c[2], v[2], madd(c[1], v[1], scale(c[0], v[0])));
        let n = [row(a[0]), row(a[1]), row(a[2]), transform(c, a[3])];
        self.left_reach_394 = length(sub(transform(&n, h.left_arm_5), wl));
        self.right_reach_398 = length(sub(transform(&n, h.right_arm_9), wr));
        if !(self.left_reach_394 <= limit) {
            self.left_free_530 = 1;
        }
        if !(self.right_reach_398 <= limit) {
            self.right_free_534 = 1;
        }
        if self.left_free_530 != self.left_free_538 {
            self.left_free_538 = self.left_free_530;
            self.left_goal_3d0 = if self.left_free_530 == 1 { 0.0 } else { 1.0 };
        }
        if self.right_free_534 != self.right_free_53c {
            self.right_free_53c = self.right_free_534;
            self.right_goal_3d4 = if self.right_free_534 == 1 { 0.0 } else { 1.0 };
        }
        let blend = |b: f32, goal: f32| clamp(goal, b - 0.2, b + 0.03);
        self.left_blend_3c8 = blend(self.left_blend_3c8, self.left_goal_3d0);
        self.right_blend_3cc = blend(self.right_blend_3cc, self.right_goal_3d4);
        self.hands_set_3d8 = 0;
        let lead = scale(self.line_velocity_2f0, 1.0 / 60.0);
        let mut out = [None; 2];
        // 82BD9728/82BD97D0: clamp to hand_radius around the original arm part.
        let target = |limb: usize, w: Vector, arm: Vector, b: f32| HandTarget {
            limb,
            world_position: clamp_point(add(w, lead), transform(a, arm), s.hand_radius),
            blend: b,
        };
        if self.left_blend_3c8 > 0.0 {
            self.left_hand_2d0 = wl;
            out[0] = Some(target(2, wl, h.left_arm_5, self.left_blend_3c8));
            self.hands_set_3d8 |= 1;
        }
        if self.right_blend_3cc > 0.0 {
            self.right_hand_2c0 = wr;
            out[1] = Some(target(3, wr, h.right_arm_9, self.right_blend_3cc));
            self.hands_set_3d8 |= 2;
        }
        out
    }
}
