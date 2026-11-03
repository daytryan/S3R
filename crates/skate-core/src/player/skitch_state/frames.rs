//! 82D48148 line frames and 82D2D550 spline point.
use super::math::{self, *};
use super::{Processed, SkitchSettings, SkitchState};
use crate::player::offboard::grab_scene::Record;

/// Record word 50, byte 0xC8 bit 0x20: endpoints reversed.
const REVERSED: u32 = 0x2000_0000;

impl SkitchState {
    /// 82D48148. Frames are row-vector; S204 = &S80 and S200 = S208 = &SC0 after.
    pub fn update_frames(&mut self, s: &SkitchSettings, p: &Processed) {
        let first = self.flags_541 & 0x80 != 0;
        let rec = self.record_400.as_mut().expect("Skitch record");
        let mut dir = rec.vector(0x70);
        if 0.0 > dot3(p.effective_right_128, dir) {
            let (a, b) = (rec.vector(0x40), rec.vector(0x50));
            rec.set_vector(0x40, b);
            rec.set_vector(0x50, a);
            dir = neg(dir);
            rec.set_vector(0x70, dir);
            rec.0[50] ^= REVERSED;
        }
        self.half_length_3a0 = rec.scalar(0xb4);
        self.reach_3a4 = self.half_length_3a0 + s.reach_margin;
        let (a, b) = (rec.vector(0x40), rec.vector(0x50));

        self.frame_40 = self.frame_80;
        self.frame_80 = self.frame_c0;
        let c = cross(dir, self.normal_320);
        let len2 = dot3(c, c);
        let r = rsqrt(len2);
        let len = if len2 == 0.0 { 0.0 } else { len2 * r };
        // 830BD350 (1e-6), the math::normalize threshold.
        let lateral = if len > 1e-6 { scale(c, r) } else { [0.0; 4] };
        self.frame_c0 = [dir, self.normal_320, lateral, scale(add(a, b), 0.5)];
        if first {
            self.frame_40 = self.frame_c0;
            self.frame_80 = self.frame_c0;
        }
        self.frame_100 = rigid_inverse_mul(&self.frame_80, &self.frame_c0);
        self.frame_140 = rigid_inverse_mul(&self.frame_c0, &self.frame_80);
        self.frame_180 = rigid_inverse_mul(&self.frame_c0, &self.frame_c0);
        self.frame_1c0 = self.frame_100;
        self.board_250 = p.effective_position_176;

        let f = self.frame_80;
        let position = dot3(f[0], sub(self.board_250, f[3]));
        self.position_rate_348 = (position - self.position_344) * RATE;
        self.position_344 = position;
        let projected = madd(f[0], position, f[3]);
        self.vector_280 = sub(projected, self.board_250);
        let target = if position.abs() < self.half_length_3a0 {
            transform(&self.frame_140, spline_point(self.record(), position).0)
        } else {
            projected
        };
        self.vector_2a0 = sub(target, projected);
        self.scalar_34c = self.position_rate_348 - self.scalar_37c;
        let old = madd(self.frame_40[0], position, self.frame_40[3]);
        let now = madd(f[0], position, f[3]);
        self.line_velocity_2f0 = scale(sub(now, old), recip(DT));

        let n = self.normal_320;
        let to_line = reject(self.vector_280, n);
        let to_line_len2 = dot3(to_line, to_line);
        self.vector_290 = scale(to_line, rsqrt(to_line_len2));
        let w = reject(self.vector_2a0, n);
        self.scalar_350 = length(w);
        if 0.0 > dot3(w, f[2]) {
            self.scalar_350 = -self.scalar_350;
        }
        self.vector_2b0 = scale(w, recip(self.scalar_350));
        let d = if to_line_len2 == 0.0 {
            0.0
        } else {
            to_line_len2 * rsqrt(to_line_len2)
        };
        self.distance_rate_358 = (d - self.distance_354) * RATE;
        self.distance_354 = d;
        self.vector_300 = scale(self.vector_290, dot3(self.line_velocity_2f0, self.vector_290));
        self.speed_340 = length(self.vector_300);
        if first {
            self.distance_rate_358 = 0.0;
        }
        self.frame_210 = f;
        self.frame_210[3] = madd(f[2], 0.5, f[3]);
        self.vector_260 = sub(
            madd(self.frame_210[0], position, self.frame_210[3]),
            self.board_250,
        );
        let v = reject(self.vector_260, n);
        self.vector_270 = normalize(v);
        self.scalar_35c = length(v);
        let fast = !(self.speed_340 < s.fast_speed);
        self.flags_541 = (self.flags_541 & !0x40) | if fast { 0x40 } else { 0 };
    }
}

/// 82D2D550(out, rec, s): world point at chord distance `s` along the record's
/// authored polyline. Returns the point and the rewritten `s` lane.
pub fn spline_point(rec: &Record, s: f32) -> (math::Vector, f32) {
    let h = rec.scalar(0xb4);
    if -h >= s {
        return (rec.vector(0x40), s);
    }
    if s >= h {
        return (rec.vector(0x50), s);
    }
    let pts = rec.points();
    if pts.len() <= 1 {
        return (rec.vector(0x40), s);
    }
    let s = if rec.0[50] & REVERSED != 0 { h - s } else { s + h };
    let axis = normalize(sub(pts[pts.len() - 1], pts[0]));
    let mut acc = 0.0;
    let mut proj = 0.0;
    let mut seg = [0.0; 4];
    let mut next = [0.0; 4];
    let mut found = false;
    for pair in pts.windows(2) {
        next = pair[1];
        seg = sub(pair[1], pair[0]);
        let d = dot3(seg, axis);
        // vmaxfp against 0x8209BE90 (1e-4).
        proj = if d > 1e-4 { d } else { 1e-4 };
        acc += proj;
        found = acc > s;
        if found {
            break;
        }
    }
    if !found {
        return (rec.vector(0x50), s);
    }
    let frame = [rec.vector(0), rec.vector(0x10), rec.vector(0x20), rec.vector(0x30)];
    if 0.0 >= proj {
        return (transform(&frame, next), s);
    }
    let t = recip(proj) * (acc - s);
    (transform(&frame, sub(next, scale(seg, t))), s)
}
