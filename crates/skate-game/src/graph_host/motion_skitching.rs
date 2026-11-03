//! TU3 skitch graph leaves: conditions IsSkitchingWithAbsorb82BBBD00,
//! SkitchingPosition82BBC068 and IsSkitchShimmying82BBC1A0; behaviours
//! Skitching (VT82320ED4), EnterSkitching (VT82320F10) and SkitchShimmying
//! (VT82320F4C). Attribute names are the static AttributeNames built at
//! 82F85708.. and 82F874E8... Not play-tested.
use super::motion_animation::MotionAnimation;
use skate_core::animation::channel_playback::ChannelSettings;
use skate_core::animation::playback_parameters::{AttributeSink, SettableAttribute};
use skate_core::animation::skeleton_input::name::encode;
use skate_core::point_graph::PointGraph;
use skate_data::collections::Collections;

const LEFT: &str = "SKCH_2H_SHIMMY_LEFT_CHANNEL";
const RIGHT: &str = "SKCH_2H_SHIMMY_RIGHT_CHANNEL";

/// anim_skitching/default, layout offsets from the stock skaterschema.
pub(super) struct Settings {
    /// 0x00 PointNegGraphData8, Ground284 -> absorb.
    absorb: PointGraph<8>,
    /// 0x50, 0.1: reach ramp start.
    reach_start: f32,
    /// 0x54/0x58, 0.25/0.85: shimmy speed window.
    shimmy_low: f32,
    shimmy_high: f32,
    /// 0x5C, 0.05: the identity clamp ahead of the shimmy window.
    shimmy_step: f32,
    /// 0x60 LongSkitchIntoReachTime.
    reach_end: f32,
    /// 0x64: added to Ground280 for Crouch.
    crouch_offset: f32,
}

impl Settings {
    pub(super) fn load(data: &Collections) -> Result<Self, String> {
        let f = |name| data.float("anim_skitching", "default", name);
        let w = data.words::<20>("anim_skitching", "default", "Hash_1C04030E5B00A8EA")?;
        Ok(Self {
            absorb: PointGraph {
                x: std::array::from_fn(|i| f32::from_bits(w[4 + i])),
                y: std::array::from_fn(|i| f32::from_bits(w[12 + i])),
            },
            reach_start: f("Hash_8738E31E171E2A87")?,
            shimmy_low: f("Hash_B2D3A764E1D86D13")?,
            shimmy_high: f("Hash_F146CFA3B03529A5")?,
            shimmy_step: f("Hash_1C7DF104E24101FF")?,
            reach_end: f("LongSkitchIntoReachTime")?,
            crouch_offset: f("Hash_6E6EFE7CF9BEF4B8")?,
        })
    }
}

/// Completed PhysOut fields written by Skitch Fill82D4C078.
#[derive(Clone, Copy, Debug, Default)]
pub struct Physical {
    pub time_to_skitch_276: f32,
    pub crouch_280: f32,
    pub absorb_284: f32,
    pub push_speed_308: f32,
    pub hands_speed_136: f32,
    pub hands_set_140: u32,
}

fn fsel(a: f32, b: f32, c: f32) -> f32 {
    if a >= 0.0 { b } else { c }
}
/// fsel max/min pair, NaN taking the second operand like retail.
fn clamp(v: f32, low: f32, high: f32) -> f32 {
    let v = fsel(low - v, low, v);
    fsel(high - v, v, high)
}

/// 82BBBD00: State16 == 104 && graph(Ground284) > 0.
pub(super) fn with_absorb(settings: &Settings, state: u32, p: &Physical) -> bool {
    state == 104 && settings.absorb.evaluate(p.absorb_284) > 0.0
}

/// 82BBC068: authored side front=1 back=2 both=3, else 0.
pub(super) fn position(side: u32, p: &Physical, stance: (bool, bool)) -> bool {
    // (Anim158 != 0) == (Anim157 != 0); stance is (158, 157).
    let r = stance.0 == stance.1;
    let pos = match p.hands_set_140 {
        1 => if r { 2 } else { 1 },
        2 => if r { 1 } else { 2 },
        _ => 3,
    };
    side == pos
}

/// 82BBC1A0: authored direction right=2 left=1, else 0.
pub(super) fn shimmying(direction: u32, p: &Physical, mirrored: bool) -> bool {
    let v = if mirrored { -1.0 } else { 1.0 } * p.hands_speed_136;
    let dir = if v > 0.25 {
        1
    } else if v < -0.25 {
        2
    } else {
        0
    };
    direction == dir
}

fn set(animation: &mut MotionAnimation, name: &[u8], value: f32) {
    animation.set_attribute(SettableAttribute {
        name: encode(name),
        value,
        normalized: false,
        sequence_id: -1,
    });
}

/// SkitchingBehaviour instance +0xC. Begin's +8 = Anim132 has no reader.
#[derive(Default)]
pub(super) struct Skitching {
    absorb: f32,
}
impl Skitching {
    /// 82BBC2D8.
    pub fn begin(&mut self, s: &Settings, p: &Physical) {
        self.absorb = s.absorb.evaluate(p.absorb_284);
    }
    /// 82BBC390.
    pub fn update(&mut self, s: &Settings, p: &Physical, animation: &mut MotionAnimation) {
        let step = f32::from_bits(0x3d23_d70a); // 0x82216FEC, 0.04
        let target = s.absorb.evaluate(p.absorb_284);
        self.absorb = clamp(target, self.absorb - step, self.absorb + step);
        set(animation, b"Crouch", p.crouch_280 + s.crouch_offset);
        set(animation, b"PushSpeed", p.push_speed_308);
        set(animation, b"absorbspeed", self.absorb);
    }
}

/// EnterSkitchingBehaviour instance +0x10 (Begin's +8/+0xC have no reader).
pub(super) struct Enter {
    reach: f32,
}
impl Default for Enter {
    fn default() -> Self {
        Self { reach: -1.0 }
    }
}
impl Enter {
    /// 82BBC538.
    pub fn begin(&mut self) {
        self.reach = -1.0;
    }
    /// 82BBC5E0. The root is 825310E0's current tree (Clip VT slots 0x14/0x18).
    pub fn update(
        &mut self,
        s: &Settings,
        p: &Physical,
        animation: &mut MotionAnimation,
    ) -> Result<(), String> {
        let crouch = p.crouch_280 + s.crouch_offset;
        let length = animation.current_length()?;
        let left = 1.0 - animation.current_time()? / length;
        let x = if left > 0.0 { p.time_to_skitch_276 / left } else { length };
        let span = s.reach_end - s.reach_start;
        let mut t = clamp(x - s.reach_start, 0.0, span) / span;
        // blt: a NaN previous value still clamps.
        if !(self.reach < 0.0) {
            let step = f32::from_bits(0x3dcc_cccd); // 0x820641A8
            t = clamp(t, self.reach - step, self.reach + step);
        }
        self.reach = t;
        set(animation, b"Crouch", crouch);
        set(animation, b"reachspeed", 1.0 - t);
        Ok(())
    }
}

/// SkitchShimmyingBehaviour, alloc 82BBCB50 (+8 = 0).
#[derive(Default)]
pub(super) struct Shimmy {
    speed: f32,
}
impl Shimmy {
    /// 82BBC7C8.
    pub fn begin(&mut self, p: &Physical) {
        self.speed = p.hands_speed_136;
    }
    /// 82BBC858.
    pub fn update(
        &mut self,
        s: &Settings,
        direction: u32,
        p: &Physical,
        animation: &mut MotionAnimation,
    ) -> Result<(), String> {
        let v = p.hands_speed_136;
        let v = clamp(v, v - s.shimmy_step, v + s.shimmy_step);
        let m = clamp(v.abs(), s.shimmy_low, s.shimmy_high);
        self.speed = if m > s.shimmy_low {
            (m - s.shimmy_low) / (s.shimmy_high - s.shimmy_low)
        } else {
            0.0
        };
        let (l, r) = (animation.channels.has(LEFT), animation.channels.has(RIGHT));
        let blend = f32::from_bits(0x3e4c_cccd); // 0x82099280
        let settings = ChannelSettings {
            priority: 0,
            keep_alive: true,
            mirrored: false,
            speed: 1.0,
            blend_in: blend,
            hold_during_blend_in: false,
            blend_out: blend,
            hold_during_blend_out: false,
            use_attributes: false,
        };
        // vfunc 0x2C is EndChannelPrematurely, 0x0C NewChannel(name, tree).
        let (stop, start, running) = if direction == 1 { (RIGHT, LEFT, l) } else { (LEFT, RIGHT, r) };
        if if direction == 1 { r } else { l } {
            animation.channels.end(stop);
        }
        if !running {
            animation.new_channel(start, start, settings)?;
        }
        set(animation, b"ShimmySpeed", self.speed);
        Ok(())
    }
    /// 82BBCAB8.
    pub fn end(animation: &mut MotionAnimation) {
        let seconds = f32::from_bits(0x3e99_999a); // 0x820D06C0
        animation.channels.end_with(LEFT, seconds, false);
        animation.channels.end_with(RIGHT, seconds, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_and_shimmy_follow_the_retail_tables() {
        let p = Physical { hands_set_140: 1, hands_speed_136: 0.3, ..Default::default() };
        assert!(position(2, &p, (true, true)) && position(1, &p, (true, false)));
        let p2 = Physical { hands_set_140: 0, ..p };
        assert!(position(3, &p2, (false, false)));
        assert!(shimmying(1, &p, false) && shimmying(2, &p, true));
        let nan = Physical { hands_speed_136: f32::NAN, ..p };
        assert!(shimmying(0, &nan, false));
    }

    #[test]
    fn clamp_keeps_the_fsel_nan_path() {
        assert_eq!(clamp(0.5, 0.0, 0.3), 0.3);
        assert_eq!(clamp(-1.0, 0.0, 0.3), 0.0);
        // NaN: first fsel keeps NaN, second selects `high`.
        assert_eq!(clamp(f32::NAN, 0.0, 0.3), 0.3);
    }
}
