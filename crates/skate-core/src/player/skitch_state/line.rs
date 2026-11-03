//! 82D49580 phases, 82D48C98 shimmy integrator, 82D49AB8 targets, 82D49D70
//! hand smoothing, 82D4BC58 heading error. fsel min/max kept literal.
use super::math::*;
use super::{Processed, SkitchSettings, SkitchState};
use crate::player::offboard::contact_toolkit::analyzer_math::tangent;
use crate::player::wipeout_state::orientation::projected_angle;

/// 0x820849C8 1/60.
pub(super) const STEP: f32 = 1.0 / 60.0;
/// 0x822F8BDC: 60 * 120 rounded down, the line-speed scale of 82D48C98.
const LINE_SPEED: f32 = f32::from_bits(0x45e0_fffe);

impl SkitchState {
    fn in_range(&self) -> bool {
        self.position_344.abs() <= self.half_length_3a0
    }
    /// 82D4C180.
    fn toward_centre(&self) -> bool {
        self.steer_39c.abs() > 0.01 && (self.steer_39c > 0.0) == (self.position_344 < 0.0)
    }
    /// 82D4C1F0.
    fn outward(&self) -> bool {
        self.steer_39c.abs() > 0.5 && (self.steer_39c > 0.0) == (self.position_344 > 0.0)
    }
    /// 82D471A8.
    fn in_reach_band(&self, s: &SkitchSettings) -> bool {
        let x = self.position_344.abs();
        x > self.half_length_3a0 && x <= self.half_length_3a0 + s.reach_margin
    }
    fn go(&mut self, phase: u32) {
        self.phase_52c = phase;
        self.flags_540 |= 0x08;
    }

    /// 82D49580.
    pub fn update_phase(&mut self, s: &SkitchSettings) {
        let t = self.phase_time_3b4;
        match self.phase_52c {
            0 => {
                self.flags_540 &= !0x60;
                if !(t > s.phase_0_time) && self.left_free_530 != 0 && self.right_free_534 != 0 {
                    return;
                }
                self.hands_38c = self.position_344;
                self.target_360 = self.position_344;
                self.phase_time_3b4 = 0.0;
                self.go(if self.in_range() { 1 } else { 2 });
            }
            1 => {
                self.flags_540 = (self.flags_540 & !0x60) | 0x40;
                if self.in_range() || self.toward_centre() {
                    return;
                }
                self.go(2);
                self.phase_time_3b4 = 0.0;
            }
            2 => {
                self.flags_540 = (self.flags_540 & !0x60) | 0x20;
                if t <= s.phase_2_time {
                    return;
                }
                let n = self.direction_3e8 as f32;
                if self.in_reach_band(s) && self.outward() && !opposite_sign(n, self.position_344) {
                    self.go(3);
                } else if self.in_range() || self.toward_centre() {
                    self.go(1);
                } else if self.fast_time_3c0 > s.phase_2_fast_time {
                    self.flags_540 = (self.flags_540 & 0x97) | 0x08;
                    self.phase_52c = 4;
                } else {
                    return;
                }
                self.phase_time_3b4 = 0.0;
                if self.phase_52c != 2 {
                    self.fast_time_3c0 = 0.0;
                    self.flags_540 &= !0x03;
                }
            }
            3 => {
                self.flags_540 = (self.flags_540 & !0x60) | 0x20;
                if t < s.phase_3_min_time && !self.outward() {
                    self.go(2);
                    self.phase_time_3b4 = 0.0;
                    self.cooldown_3fc = s.phase_3_cooldown;
                }
                if self.phase_52c != 3 {
                    self.flags_540 &= !0x03;
                }
                if self.phase_time_3b4 > s.phase_3_max_time {
                    self.cooldown_3f8 = s.phase_3_exit_cooldown;
                    self.phase_52c = 4;
                }
            }
            _ => {}
        }
    }

    /// 82D4BC58: wrapped angle from the board forward to S270 about the normal.
    pub(super) fn heading_error(&self, p: &Processed) -> f32 {
        wrap(projected_angle(p.effective_forward_160, self.vector_270, self.normal_320))
    }

    /// 82D48C98.
    pub fn integrate(&mut self, s: &SkitchSettings, p: &Processed) {
        let input = self.steer_39c;
        let in_dz = if input.abs() > 0.01 { input } else { 0.0 };
        let sgn_in = if in_dz > 0.0 { 1.0 } else { fsel(in_dz, 0.0, -1.0) };
        let reach = self.reach_3a4;
        if self.flags_541 & 0x80 != 0 || self.flags_541 & 0x40 == 0 {
            let a = self.heading_error(p);
            let d = clamp(self.scalar_35c, 0.001, 1e9);
            self.target_360 = clamp(self.position_344 + tangent(-a) * d, -reach, reach);
        }
        let x = self.position_344;
        let now = madd(self.frame_80[0], x, self.frame_80[3]);
        let old = madd(self.frame_40[0], x, self.frame_40[3]);
        let lv = -(-dot3(sub(now, old), self.frame_80[0]) * LINE_SPEED);
        let k = s.velocity_filter;
        self.direction_3e8 = 0;
        self.line_speed_364 = (1.0 - k).mul_add(self.line_speed_364, k * lv);
        if !(self.line_speed_364.abs() <= s.fast_line_speed) {
            self.direction_3e8 = if self.line_speed_364 > 0.0 { 1 } else { -1 };
        }
        let v = self.line_speed_364;
        let sg = if v < 0.0 { -1.0 } else if !(v <= 0.0) { 1.0 } else { 0.0 };
        let m = v * sg;
        let excess = m - s.line_speed_allowance.evaluate(m);
        self.scalar_370 = -(fsel(-excess, 0.0, excess) * sg);

        let steering = input.abs() > 0.01 || self.direction_3e8 != 0;
        let holding = !steering && self.flags_540 & 0x02 != 0;
        if !holding {
            self.flags_540 &= !0x03;
        }
        if self.phase_52c == 0 || self.phase_52c == 4 || self.flags_541 & 0x40 == 0 {
            self.scalar_36c = 0.0;
            self.line_speed_364 = (1.0 - k) * self.line_speed_364;
            self.clear_370_380();
            return;
        }
        if self.flags_540 & 0x02 != 0 && self.flags_540 & 0x01 != 0 {
            self.scalar_36c = 0.0;
            self.target_360 = self.end_384;
            self.line_speed_364 = (1.0 - k).mul_add(self.line_speed_364, k * lv);
            self.clear_370_380();
            return;
        }
        let limit = if input.abs() > 0.01 {
            let at = if self.direction_3e8 != 0 && opposite_sign(in_dz, self.line_speed_364) { m } else { 0.0 };
            (s.shimmy_limit.evaluate(at) * in_dz).abs()
        } else {
            0.0
        };
        let l = s.shimmy_speed;
        self.scalar_36c = l * sgn_in;
        let speed = self.line_speed_364 + self.scalar_370;
        self.scalar_374 = 0.0;
        if !steering {
            let t = if self.flags_540 & 0x02 != 0 {
                clamp((self.end_384 - self.position_344) * RATE, -l, l)
            } else {
                0.0
            };
            self.scalar_374 = clamp((t - speed.mul_add(STEP, self.scalar_37c)) * RATE, -l, l);
        }
        let (a, b) = (self.scalar_36c, self.scalar_374);
        if same_sign(a, b) {
            let d = a - b;
            self.scalar_36c = if a.abs() > 0.0 { fsel(-d, 0.0, d) } else { fsel(-d, d, 0.0) };
        }
        self.scalar_378 = ((self.scalar_374 + self.line_speed_364) + self.scalar_370) + self.scalar_36c;
        let lim = if holding { s.shimmy_limit.evaluate(0.0) } else { limit };
        let (mut lo, mut hi) = (-lim, lim);
        if self.direction_3e8 == 1 {
            hi = f32::MAX;
        } else if self.direction_3e8 == -1 {
            lo = f32::MIN;
        }
        self.scalar_37c = clamp(self.scalar_378.mul_add(STEP, self.scalar_37c), lo, hi);
        let i340 = 1.0 / clamp(self.speed_340, 0.01, 1e9);
        let c35 = clamp(self.scalar_35c, 0.01, 1e9);
        let i35 = 1.0 / c35;
        let p340 = clamp(self.speed_340, 0.0, 1e9);
        self.scalar_37c = (((i340 * self.scalar_37c) * c35) * i35) * p340;
        if !steering && !holding {
            self.word_3ec = 1;
            return;
        }
        let lag = (-self.scalar_34c).mul_add(s.position_lag, self.scalar_37c);
        let x = if self.direction_3e8 != 0 {
            self.word_3ec = 1;
            lag.mul_add(STEP, self.target_360)
        } else {
            self.word_3ec = 0;
            (i340 * lag).mul_add(c35, self.position_344)
        };
        let x = clamp(x, -reach, reach);
        self.scalar_380 = ((x - self.position_344) * i35) * p340;
        let y = (self.scalar_380 * i340).mul_add(c35, self.position_344);
        if holding && same_sign(self.end_384 - self.target_360, y - self.end_384) {
            self.target_360 = self.end_384;
            self.flags_540 |= 0x01;
            return;
        }
        self.target_360 = y;
    }

    fn clear_370_380(&mut self) {
        self.scalar_380 = 0.0;
        self.scalar_37c = 0.0;
        self.scalar_378 = 0.0;
        self.scalar_374 = 0.0;
        self.scalar_370 = 0.0;
    }

    /// 82D49AB8, ending with 82D49D70.
    pub fn update_targets(&mut self, s: &SkitchSettings) {
        let x = self.position_344;
        match self.phase_52c {
            0 => self.hands_target_388 = clamp(self.target_360, -self.half_length_3a0, self.half_length_3a0),
            1 => {
                self.left_free_530 = 0;
                self.right_free_534 = 0;
            }
            2 | 3 => {
                let e = if self.phase_52c == 2 {
                    if x > 0.0 { self.reach_3a4 } else { -self.reach_3a4 }
                } else {
                    let e = self.half_length_3a0 + s.phase_3_reach;
                    if x > 0.0 { e } else { -e }
                };
                if self.flags_540 & 0x02 == 0 {
                    self.end_384 = e;
                    self.flags_540 = (self.flags_540 & 0xfc) | 0x02;
                }
                if self.flags_540 & 0x01 != 0 {
                    self.target_360 = e;
                }
                if self.phase_52c == 2 {
                    let v = self.line_speed_364;
                    if same_sign(v, x) && v.abs() > s.phase_2_line_speed {
                        self.fast_time_3c0 += STEP;
                    } else {
                        self.fast_time_3c0 = 0.0;
                    }
                    (self.left_free_530, self.right_free_534) = if x > 0.0 { (1, 0) } else { (0, 1) };
                } else {
                    self.left_free_530 = 1;
                    self.right_free_534 = 1;
                }
            }
            _ => {}
        }
        self.smooth_hands(s);
        self.line_fraction_3a8 = clamp(x / self.half_length_3a0, -1.0, 1.0);
        if !(self.cooldown_3fc <= 0.0) {
            self.cooldown_3fc -= STEP;
        }
        self.phase_time_3b4 += STEP;
    }

    /// 82D49D70.
    fn smooth_hands(&mut self, s: &SkitchSettings) {
        let dpos = (self.position_344 - self.hands_38c).abs();
        if self.flags_540 & 0x08 != 0 {
            let input = if self.steer_39c.abs() > 0.01 { self.steer_39c } else { 0.0 };
            let mut c = clamp(self.position_344, -self.reach_3a4, self.reach_3a4);
            let t = self.hands_target_388;
            if (!(input <= 0.5) && c < t) || (input < -0.5 && !(c <= t)) {
                c = c.mul_add(0.0, t);
            }
            self.hands_target_388 = c;
            if !(self.position_rate_348.abs() >= s.hands_settled_rate) && !(dpos >= s.hands_settled_distance) {
                self.flags_540 &= !0x08;
            }
            self.hands_time_3c4 = 0.0;
        } else {
            self.hands_time_3c4 += STEP;
            if dpos > s.hands_move_distance || !(self.hands_time_3c4 <= s.hands_move_time) {
                self.flags_540 |= 0x08;
            }
        }
        let m = s.hands_step;
        let step = (self.hands_target_388 - self.hands_38c) * s.hands_follow;
        self.hands_step_390 = clamp(step, self.hands_step_390 - m, m + self.hands_step_390);
        self.hands_38c += self.hands_step_390;
    }
}
