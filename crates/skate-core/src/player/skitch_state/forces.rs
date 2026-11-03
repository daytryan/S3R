//! 82D4AC38's skitch-owned parts (82D4B500 tow, 82D4B8C0 heading drive),
//! 82D4A0C0 yaw lean, the state side of 82D47BD8/82D479C8 and 82D4AFB8 release.
use super::line::STEP;
use super::math::*;
use super::{Processed, SkitchSettings, SkitchState};
use crate::player::wipeout_state::math::limit_length;
use crate::player::wipeout_state::orientation::projected_angle;

/// 82C03EF0 force-queue entry {world force, body point}.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointForce {
    pub force: Vector,
    pub point: Vector,
}

/// 82D4AC38 outside phase 4: tag-6 tow force, and the 82C07328 drive torque
/// when S541 bit 0x40 is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardForces {
    pub tow: PointForce,
    pub drive: Option<Vector>,
}

impl SkitchState {
    /// The skitch-owned part of 82D4AC38: S330 = 0, then (phase != 4) tow
    /// before heading drive. The caller runs the Ground services between them
    /// and must add `STEP` to S3E0 via `finish_board_forces` afterwards.
    pub fn board_forces(&mut self, s: &SkitchSettings, p: &Processed, root_z: f32) -> Option<BoardForces> {
        self.tow_force_330 = [0.0; 4];
        if self.phase_52c == 4 {
            return None;
        }
        let tow = self.tow_force(s, p, root_z);
        let drive = self.heading_drive(s, p);
        self.tow_force_330 = tow.force;
        Some(BoardForces {
            tow,
            drive: (self.flags_541 & 0x40 != 0).then_some(drive),
        })
    }

    /// 82D4AC38 tail outside phase 4.
    pub fn finish_board_forces(&mut self) {
        self.time_3e0 += STEP;
    }

    /// 82D4B500. Uses last tick's S310; `root_z` is originals[0].row3.z (sk+0x3788).
    fn tow_force(&mut self, s: &SkitchSettings, p: &Processed, root_z: f32) -> PointForce {
        let c = dot3(self.vector_270, self.drive_310).max(0.5).min(1.0);
        let z = if p.flags_2476 & 0x04 != 0 { root_z * -1.0 } else { root_z };
        let e = ((self.speed_340 - p.scalar_2612) + self.speed_340).mul_add(STEP, self.distance_354)
            + self.scalar_350;
        let o = if self.phase_52c == 2 { s.tow_offset_phase_2 } else { s.tow_offset };
        let g = s.tow.evaluate(e - ((p.world_grab_z_2820 - z) + o));
        self.absorb_3bc = -self.distance_rate_358;
        let damp = -(s.tow_damping * self.absorb_3bc);
        let ic = recip(c);
        let f = madd(scale(self.drive_310, g), ic, scale(scale(self.drive_310, damp), ic));
        let f = scale(scale(f, p.total_mass_2660), recip(STEP));
        PointForce {
            force: limit_length(f, 500.0),
            point: [0.0; 4],
        }
    }

    /// 82D4B8C0 with 82D4BAC0 and 82D4BCF8.
    fn heading_drive(&mut self, s: &SkitchSettings, p: &Processed) -> Vector {
        self.vector_2e0 = madd(self.frame_210[0], self.target_360, self.frame_210[3]);
        let (drive, t) = self.drive_direction(p);
        self.drive_310 = drive;
        self.angle_3b8 = t;
        let a1 = wrap(projected_angle(p.effective_forward_160, self.drive_310, self.normal_320));
        let h = self.heading_error(p);
        let d = wrap(a1 - h);
        let out = self.drive_torque(s, a1, d);
        self.heading_3ac = if self.flags_541 & 0x40 != 0 { d } else { -h };
        out
    }

    /// 82D4BAC0: unit direction to S2E0 in the normal plane and its angle t.
    fn drive_direction(&self, p: &Processed) -> (Vector, f32) {
        let q = p.effective_position_176;
        let n = self.normal_320;
        let u = reject(sub(self.vector_2e0, q), n);
        let w = reject(sub(madd(self.frame_210[0], self.position_344, self.frame_210[3]), q), n);
        let out = normalize(u);
        (out, wrap(projected_angle(normalize(w), out, n)))
    }

    /// 82D4BCF8.
    fn drive_torque(&self, s: &SkitchSettings, a: f32, d: f32) -> Vector {
        let m = s.heading_rate * f32::from_bits(0x3c8e_fa35) * STEP;
        let step = m.min((-m).max(a));
        let sg = if step >= 0.0 { 1.0 } else { -1.0 };
        let mut up = self.normal_320;
        if 0.0 > up[1] {
            up = scale(up, -1.0);
        }
        let out = scale(up, crate::trigonometry::sin((step * sg) * sg));
        let g1 = s.heading_vs_time.evaluate(self.time_3e0);
        let g2 = s.heading_vs_speed.evaluate(self.speed_340);
        let deg = f32::from_bits(0x3ea2_f983) * (d * 180.0);
        let f = if self.word_3ec == 1 && !(deg.abs() >= 1.0) { s.heading_fine.evaluate(deg) } else { 1.0 };
        scale(out, fsel(g1 - f * g2, g1, f * g2))
    }

    /// 82D4A0C0. Returns the SkateboardOffset orientation rows (sk+0x3D50,
    /// refreshed flag 0x4005 = 1, frames 0x4008 = 15) when S3B0 exceeds 2 deg.
    pub fn update_yaw_lean(&mut self, s: &SkitchSettings, p: &Processed) -> Option<Frame> {
        const RAD: f32 = f32::from_bits(0x3c8e_fa35);
        let sign = fsel(self.heading_3ac, 1.0, -1.0);
        let deg = (self.heading_3ac * sign) * f32::from_bits(0x4265_2ee1);
        let mut v = (s.yaw_lean.evaluate(deg) * RAD) * sign;
        if p.flags_2488 & 0x0080_0000 != 0 {
            v = 0.0;
        }
        let lim = 10.0 * RAD;
        self.yaw_lean_3b0 += clamp((v - self.yaw_lean_3b0) * 0.25, -lim, lim);
        if self.yaw_lean_3b0.abs() <= 2.0 * RAD {
            return None;
        }
        let (sin, cos) = crate::trigonometry::sin_cos(self.yaw_lean_3b0);
        Some([
            [cos, 0.0, -sin, cos],
            [0.0, 1.0, 0.0, 0.0],
            [sin, 0.0, cos, sin],
            [0.0; 4],
        ])
    }

    /// State effects of 82D47BD8 then 82D479C8. Their grab-manager pushes
    /// (8275FF00) do nothing for kind 1. Kind 2 goes to LivingWorld+0x5790's
    /// prop entity (82C4C370/3E0 -> entity vfuncs 0x54/0x50), which the port
    /// does not simulate outside the 502 mover, so no force is produced here.
    pub fn update_pushes(&mut self, p: &Processed) {
        let holding = p.flags_2488 & 0x2000_0000 != 0;
        if self.flags_541 & 0x10 == 0 {
            let x = self.speed_340;
            let gap = fsel(8.0 - x, 8.0 - x, 0.0);
            let cap = 200.0 / (1.0 / f32::from_bits(self.record().0[60]));
            let cap = fsel(3.5 - cap, cap, 3.5);
            self.push_speed_3e4 = fsel(gap - cap, cap, gap) + x;
            if holding {
                self.flags_541 |= 0x10;
            }
        }
        if self.flags_541 & 0x10 != 0 && !holding {
            self.flags_541 &= !0x10;
        }
        self.flags_541 &= !0x08;
        if p.brake_2728 != 0.0 && length(self.line_velocity_2f0) > f32::from_bits(0x3780_0000) {
            self.flags_541 |= 0x08;
        }
    }

    /// 82D4AFB8. Returns the tag-6 release kicks in 82C03EF0 order
    /// (82D4B1D0 for S540 bit 0x40, then 82D4B378 for bit 0x20).
    pub fn update_release(&mut self, s: &SkitchSettings, p: &Processed) -> [Option<PointForce>; 2] {
        if p.flags_2476 & 0x0040_0000 == 0 {
            self.phase_52c = 4;
        }
        let phase = self.phase_52c;
        if !matches!(phase, 0 | 3 | 4)
            && self.left_free_530 == 1
            && self.right_free_534 == 1
            && !(self.cooldown_3fc > 0.0)
        {
            self.phase_52c = 4;
            self.flags_540 = (self.flags_540 & !0x70) | 0x10;
        }
        let mut kicks = [None; 2];
        if self.phase_52c == 4 {
            if !(self.cooldown_3f8 > 0.0) {
                self.cooldown_3f8 = s.release_time;
            }
            if self.release_frames_528 == 1 {
                if self.flags_540 & 0x40 != 0 {
                    kicks[0] = Some(self.release_kick(p, s.release_kick_inner, true));
                }
                if self.flags_540 & 0x20 != 0 {
                    kicks[1] = Some(self.release_kick(p, s.release_kick_outer, false));
                }
            }
            self.release_frames_528 += 1;
            if self.release_frames_528 > 3 {
                self.flags_540 |= 0x10;
            }
        } else {
            self.release_frames_528 = 0;
        }
        if self.kind_520 == 2 {
            let r = self.record().vector(0x90);
            if r[0] * r[0] + r[2] * r[2] > 1.0 {
                self.flags_540 |= 0x10;
            }
        }
        kicks
    }

    /// 82D4B1D0 (`inner`, clamped) and 82D4B378.
    fn release_kick(&self, p: &Processed, c: f32, inner: bool) -> PointForce {
        let k = if inner {
            let a = -self.speed_340;
            let b = self.distance_rate_358 + c;
            let x = fsel(a - b, a, b);
            fsel(-x, x, 0.0)
        } else {
            self.distance_rate_358 + c
        };
        PointForce {
            force: scale(scale(scale(self.vector_270, k), p.total_mass_2660), recip(STEP)),
            point: [0.0; 4],
        }
    }
}
