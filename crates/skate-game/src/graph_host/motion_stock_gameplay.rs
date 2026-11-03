//! Typed bindings for the remaining stock MotionGraph gameplay operations.
//!
//! These operations are deliberately represented as authored gameplay nodes,
//! not compatibility no-ops. Their producers are completed incrementally and
//! execution fails with the exact authored operation until that producer is
//! published by the corresponding physics owner.
use skate_core::animation::output::attributes::AttributeName;
use skate_core::animation::skeleton_input::name::encode;
use skate_data::state_graph::attributes::Attributes;
use skate_core::point_graph::PointGraph;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrabType {
    Fs,
    Bs,
    Nose,
    Tail,
}

impl GrabType {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "FS" => Ok(Self::Fs),
            "BS" => Ok(Self::Bs),
            "Nose" => Ok(Self::Nose),
            "Tail" => Ok(Self::Tail),
            other => Err(format!("SetGrabType has unknown grab type {other:?}")),
        }
    }
    pub fn as_value(self) -> f32 {
        // TU3 82BB9BD0: zero is no grab; authored types are one through four.
        match self {
            Self::Fs => 1.0,
            Self::Bs => 2.0,
            Self::Nose => 3.0,
            Self::Tail => 4.0,
        }
    }
}

/// 82BB6A20's frame-rate constant (also 82BA7598's).
const FPS: f32 = f32::from_bits(0x426f_ffff);
/// The tracking nodes' chase limit: 2 degrees a frame.
const MAX_STEP: f32 = 0.034_906_585;

/// The 40-byte MovingObject block on ISkaterAnim (F2). InitMovingObjects 82BB63D8 zeroes it,
/// MovingObject Enter 82BB6860 and Update 82BB6A20 drive it, and
/// AngleAdjustByAnimationEndLessThan reads angle and target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MovingObjectBlock {
    pub angle: f32,
    pub blend: f32,
    pub spd01: f32,
    pub spd_curve: f32,
    pub e_f: f32,
    pub target: f32,
    pub cat: u8,
    pub prev: u8,
    /// +36 bit 0x80: the half flip already happened this entry.
    pub flipped: bool,
}

impl MovingObjectBlock {
    /// Enter 82BB6860.
    pub fn enter(&mut self, category: u8) {
        self.prev = self.cat;
        self.cat = category;
        self.flipped = false;
    }

    /// Update 82BB6A20 for a node of category `k`. `[a, b, c, d, e]` are the completed
    /// OffBoard +0, +4, +8, +12 and +28; `length`/`time` are the current clip's (INFERRED
    /// clip vfunc20/vfunc24), read only by transition nodes.
    pub fn update(
        &mut self,
        k: u8,
        blend_out_over_next_cycle: bool,
        [a, b, c, d, e]: [f32; 5],
        length: f32,
        time: f32,
        curve: &PointGraph<8>,
    ) {
        use skate_core::player::wipeout_state::math::wrap_angle as wrap;
        use std::f32::consts::{FRAC_PI_2, PI};
        let mag2 = a * a + b * b;
        let mag = if mag2 > 1e-4 { mag2.sqrt() } else { 0. };
        let p = b * d + a * c;
        let proj = if mag2 > 1e-4 && p >= 0. { p / mag } else { 0. };
        self.spd_curve = clamp(curve.evaluate(proj), self.spd_curve - 0.01, self.spd_curve + 0.01);
        self.e_f = clamp(e, self.e_f - 0.03, self.e_f + 0.03);
        self.spd01 = clamp(clamp(0.4 * mag, 0., 1.), self.spd01 - 0.03, self.spd01 + 0.03);
        let front = |k: u8| (1..=6).contains(&k);
        let back = |k: u8| (7..=12).contains(&k);
        if !self.flipped
            && (front(self.prev) && back(self.cat) || back(self.prev) && front(self.cat))
        {
            self.flipped = true;
            self.angle = wrap(self.angle + PI);
        }
        if transition(k) {
            let frames = FPS * (length - time + if blend_out_over_next_cycle { length } else { 0. });
            let frames = if 1.0 - frames >= 0.0 { 1.0 } else { frames };
            self.target = if back(k) { PI } else { 0. };
            self.angle = wrap(self.angle + wrap(self.target - self.angle) / frames);
        } else {
            let want = (-b).atan2(a);
            self.angle = wrap(self.angle + clamp(wrap(want - self.angle), -MAX_STEP, MAX_STEP));
        }
        // Category 0 (still) keeps the chased angle unclamped and the blend as it was.
        if k == 0 {
            return;
        }
        let left = k % 2 == 1;
        let (lo, hi) = match (back(k), left) {
            (false, true) => (-FRAC_PI_2, 0.),
            (false, false) => (0., FRAC_PI_2),
            (true, true) => (PI, -FRAC_PI_2),
            (true, false) => (FRAC_PI_2, PI),
        };
        self.angle = clamp_arc(self.angle, lo, hi);
        self.blend = match (back(k), left) {
            (false, true) => -self.angle / FRAC_PI_2,
            (false, false) => self.angle / FRAC_PI_2,
            (true, true) => wrap(self.angle + PI) / FRAC_PI_2,
            (true, false) => wrap(PI - self.angle) / FRAC_PI_2,
        };
    }
}

/// The transition nodes (3..6, 9..12) sweep to the clip's end and read its length and time.
pub fn transition(k: u8) -> bool {
    matches!(k, 3..=6 | 9..=12)
}

/// Enter 82BB6860's ISkaterAnim vfunc40(m, m), the one-shot clip mirror. vfunc32 82B97388 is
/// natural and relative stance both 0 or both 1; RIGHT (even) nodes mirror when it is 0, LEFT
/// (odd) nodes when it is 1, category 0 never.
pub fn mirror_request(k: u8, natural: u32, relative: u32) -> bool {
    let same = natural == relative && natural <= 1;
    match k {
        0 => false,
        k if k % 2 == 0 => !same,
        _ => same,
    }
}

/// curve8 (F7): the parser binds class 0x97B0272A2A7E54F8 `default`, field C3A15E5DE438066F.
pub fn moving_object_curve(
    data: &skate_data::collections::Collections,
) -> Result<PointGraph<8>, String> {
    let w = data.words::<20>("Hash_97B0272A2A7E54F8", "default", "Hash_C3A15E5DE438066F")?;
    Ok(PointGraph {
        x: std::array::from_fn(|i| f32::from_bits(w[4 + i])),
        y: std::array::from_fn(|i| f32::from_bits(w[12 + i])),
    })
}

/// The fsel clamp: lo when x <= lo, then hi when above it (NaN ends at hi).
fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    let m = if lo - x >= 0. { lo } else { x };
    if hi - m >= 0. { m } else { hi }
}

/// 82BB79D8: inside (inclusive; lo >= hi is the arc through pi) is kept; outside goes to the
/// nearer bound by |wrap(distance)|, ties and NaN as the exe orders them.
fn clamp_arc(angle: f32, lo: f32, hi: f32) -> f32 {
    use skate_core::player::wipeout_state::math::wrap_angle as wrap;
    let inside = if lo < hi {
        lo <= angle && angle <= hi
    } else {
        angle >= lo || angle <= hi
    };
    if inside {
        angle
    } else if wrap(lo - angle).abs() >= wrap(angle - hi).abs() {
        hi
    } else {
        lo
    }
}

/// TU3 82BBF180..F2EC, constants at 820DDEA8..DEE4. Sector windows
/// expand when neighboring variants are absent; first matching window wins.
pub(super) fn select_grab_score<'a>(
    base: &'a str,
    directions: [Option<&'a str>; 4],
    x: f32,
    y: f32,
) -> &'a str {
    let [up, left, down, right] = directions;
    if (x * x + y * y).sqrt() <= 0.5 {
        return base;
    }
    let angle = y.atan2(x).rem_euclid(std::f32::consts::TAU);
    let windows = [
        (
            up,
            if right.is_some() { 44.0 } else { 359.0 },
            if left.is_some() { 134.0 } else { 181.0 },
        ),
        (
            left,
            if up.is_some() { 44.0 } else { 89.0 },
            if down.is_some() { 226.0 } else { 271.0 },
        ),
        (
            down,
            if left.is_some() { 224.0 } else { 179.0 },
            if right.is_some() { 316.0 } else { 1.0 },
        ),
        (
            right,
            if down.is_some() { 314.0 } else { 269.0 },
            if up.is_some() { 46.0 } else { 91.0 },
        ),
    ];
    for (name, start, end) in windows {
        let start = start * (std::f32::consts::PI / 180.0);
        let end = end * (std::f32::consts::PI / 180.0);
        let inside = if end < start {
            angle >= start || angle <= end
        } else {
            angle >= start && angle <= end
        };
        if inside && let Some(name) = name {
            return name;
        }
    }
    base
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    MatchAirTime,
    UpdateStandingOnCar,
    /// 82BB63D8 zeroes the block; it registers nothing (F7).
    InitMovingObjects,
    MovingObject {
        category: u8,
        blend_out_over_next_cycle: bool,
    },
    SetGrabType {
        grab: String,
    },
    JumpInto {
        attribute: AttributeName,
    },
    ScoringHandPlants,
    SetHandPlantAnticLength,
    ScoringGrabs {
        grab_name: String,
        up: Option<String>,
        left: Option<String>,
        down: Option<String>,
        right: Option<String>,
        intent_x: String,
        intent_y: String,
        invert_y: bool,
        polar: bool,
    },
    AirDismounting,
    TweakProject {
        intent_x: String,
        intent_y: String,
        attribute_x: AttributeName,
        attribute_y: AttributeName,
    },
    FootPlantAbsorb,
    FingerFlipOut {
        grab_intent: String,
    },
    EnterSkitchingBehaviour,
    SkitchingBehaviour,
    SkitchShimmyingBehaviour {
        /// Factory82BCACD8: right=2, left=1, else 0.
        direction: u32,
    },
    SetManualAngle {
        attribute: AttributeName,
    },
    HippyJumpAntic,
    LandOnBoard,
}

impl Operation {
    pub fn recognizes(name: &str) -> bool {
        matches!(
            name,
            "MatchAirTime"
                | "UpdateStandingOnCar"
                | "InitMovingObjects"
                | "MovingObject"
                | "SetGrabType"
                | "JumpInto"
                | "ScoringHandPlants"
                | "SetHandPlantAnticLength"
                | "ScoringGrabs"
                | "AirDismounting"
                | "TweakProject"
                | "FootPlantAbsorb"
                | "FingerFlipOut"
                | "EnterSkitchingBehaviour"
                | "SkitchingBehaviour"
                | "SkitchShimmyingBehaviour"
                | "SetManualAngle"
                | "HippyJumpAntic"
                | "LandOnBoard"
        )
    }

    pub fn parse(a: &Attributes<'_>) -> Self {
        match a.text("name").unwrap_or("") {
            "MatchAirTime" => Self::MatchAirTime,
            "UpdateStandingOnCar" => Self::UpdateStandingOnCar,
            "InitMovingObjects" => Self::InitMovingObjects,
            //82BB64B8: def+44 from `direction` (82AE89B0 compares in this order); anything else,
            //`still` included, is 0. def+168 = BlendOutOverNextCycle.
            "MovingObject" => Self::MovingObject {
                category: [
                    "PUSHLEFT",
                    "PUSHRIGHT",
                    "PUSHLEFTToRIGHT",
                    "PUSHRIGHTToLEFT",
                    "PUSHToPULLLEFT",
                    "PUSHToPULLRIGHT",
                    "PULLLEFT",
                    "PULLRIGHT",
                    "PULLLEFTToRIGHT",
                    "PULLRIGHTToLEFT",
                    "PULLToPUSHLEFT",
                    "PULLToPUSHRIGHT",
                ]
                .iter()
                .position(|d| a.text("direction") == Some(*d))
                .map_or(0, |i| i as u8 + 1),
                blend_out_over_next_cycle: a.boolean_byte("BlendOutOverNextCycle", 0) != 0,
            },
            "SetGrabType" => Self::SetGrabType {
                grab: a.text("grab").unwrap_or("").to_owned(),
            },
            "JumpInto" => Self::JumpInto {
                attribute: encode(
                    a.text("attribute")
                        .or_else(|| a.text("attr"))
                        .unwrap_or("JumpInto")
                        .as_bytes(),
                ),
            },
            "ScoringHandPlants" => Self::ScoringHandPlants,
            "SetHandPlantAnticLength" => Self::SetHandPlantAnticLength,
            "ScoringGrabs" => Self::ScoringGrabs {
                grab_name: required_text(a, "grabName"),
                up: a.text("up").map(str::to_owned),
                left: a.text("left").map(str::to_owned),
                down: a.text("down").map(str::to_owned),
                right: a.text("right").map(str::to_owned),
                intent_x: a
                    .text("intentX")
                    .or_else(|| a.text("angle"))
                    .unwrap_or("")
                    .to_owned(),
                intent_y: a
                    .text("intentY")
                    .or_else(|| a.text("magnitude"))
                    .unwrap_or("")
                    .to_owned(),
                invert_y: a.boolean_byte("invertY", 0) != 0,
                polar: a.text("intentX").is_none(),
            },
            "AirDismounting" => Self::AirDismounting,
            "TweakProject" => Self::TweakProject {
                // TU3 factory 82BC9510 binds these names directly.
                intent_x: "TweakX".to_owned(),
                intent_y: "TweakY".to_owned(),
                attribute_x: encode(b"tweak_x"),
                attribute_y: encode(b"tweak_y"),
            },
            "FootPlantAbsorb" => Self::FootPlantAbsorb,
            "FingerFlipOut" => Self::FingerFlipOut {
                //Factory82BC7C18 binds authored grabintent and literal Grabbing.
                grab_intent: required_text(a, "grabintent"),
            },
            "EnterSkitchingBehaviour" => Self::EnterSkitchingBehaviour,
            "SkitchingBehaviour" => Self::SkitchingBehaviour,
            "SkitchShimmyingBehaviour" => Self::SkitchShimmyingBehaviour {
                direction: match a.text("direction").unwrap_or("") {
                    "right" => 2,
                    "left" => 1,
                    _ => 0,
                },
            },
            "SetManualAngle" => Self::SetManualAngle {
                // Factory82BC9678/84 uses literal8231D5C8, no authored override.
                attribute: encode(b"manual_angle"),
            },
            "HippyJumpAntic" => Self::HippyJumpAntic,
            "LandOnBoard" => Self::LandOnBoard,
            _ => unreachable!("unrecognized stock gameplay operation"),
        }
    }
}

fn required_text(a: &Attributes<'_>, name: &str) -> String {
    a.text(name).unwrap_or("").to_owned()
}

#[cfg(test)]
mod tests {
    use super::{MovingObjectBlock, Operation, mirror_request};
    use skate_core::player::wipeout_state::math::wrap_angle;
    use skate_core::point_graph::PointGraph;
    use skate_data::state_graph::GraphAttribute;
    use skate_data::state_graph::attributes::Attributes;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    /// Identity up to 1; the real curve8 is collections data (F7).
    const CURVE: PointGraph<8> = PointGraph {
        x: [0., 1., 1., 1., 1., 1., 1., 1.],
        y: [0., 1., 1., 1., 1., 1., 1., 1.],
    };
    /// 82BB6A20's frame-rate constant.
    const FPS: f32 = 59.999_996;

    fn run(cat: u8, ab: [f32; 2], frames: usize) -> MovingObjectBlock {
        let mut b = MovingObjectBlock::default();
        b.enter(cat);
        for _ in 0..frames {
            b.update(cat, false, [ab[0], ab[1], 0., 0., 0.], 1., 0., &CURVE);
        }
        b
    }

    #[test]
    fn moving_object_block_follows_retail_82bb6a20() {
        // Category 1 (PUSHLEFT) holds the angle in [-pi/2, 0]: input at +pi/4 clamps to 0.
        assert!(run(1, [1., -1.], 120).angle.abs() < 1e-6);
        // Category 2 (PUSHRIGHT): settles at pi/4, blend 0.5.
        let b = run(2, [1., -1.], 120);
        assert!((b.angle - FRAC_PI_4).abs() < 1e-4 && (b.blend - 0.5).abs() < 1e-4, "{b:?}");
        // Category 8 (PULLRIGHT) from (-1,-1): 3pi/4, blend 0.5 (entering 7..12 from 0 adds no flip: prev 0).
        let b = run(8, [-1., -1.], 240);
        assert!((b.angle - 3. * FRAC_PI_4).abs() < 1e-4 && (b.blend - 0.5).abs() < 1e-4, "{b:?}");
        // The chase is at most 2 degrees a frame.
        assert!(run(2, [1., -1.], 1).angle <= 0.034_906_585 + 1e-7);
    }

    #[test]
    fn moving_object_flips_half_once_per_entry() {
        // PUSHRIGHT at pi/4, then PULLRIGHT: the angle turns by pi once (82bb6a20 flag 0x80).
        let mut b = run(2, [1., -1.], 120);
        b.enter(8);
        b.update(8, false, [-1., -1., 0., 0., 0.], 1., 0., &CURVE);
        // -3pi/4 after the flip, chased 2 degrees away from 3pi/4, clamps to hi = pi.
        // Without the flip it would clamp to lo = pi/2.
        assert!((b.angle - PI).abs() < 1e-5 && b.flipped, "{b:?}");
        assert!(b.blend.abs() < 1e-5, "{b:?}");
        b.update(8, false, [-1., -1., 0., 0., 0.], 1., 0., &CURVE);
        assert!((b.angle - (PI - 0.034_906_585)).abs() < 1e-5, "flipped twice: {b:?}");
    }

    #[test]
    fn moving_object_transitions_sweep_to_the_clip_end() {
        // k 3..6 and 9..12: frames = max(1, 59.999996 * (L - T + (bo ? L : 0))).
        let swept = |k: u8, angle: f32, bo: bool, length: f32, time: f32| {
            let mut b = MovingObjectBlock { angle, ..Default::default() };
            b.enter(k);
            b.update(k, bo, [0.; 5], length, time, &CURVE);
            b
        };
        let b = swept(3, -FRAC_PI_4, false, 1., 0.5);
        assert!((b.angle - (-FRAC_PI_4 + FRAC_PI_4 / (0.5 * FPS))).abs() < 1e-6, "{b:?}");
        assert_eq!(b.target, 0.);
        assert!((b.blend - (-b.angle / FRAC_PI_2)).abs() < 1e-6, "{b:?}");
        let b = swept(3, -FRAC_PI_4, true, 1., 0.5);
        assert!((b.angle - (-FRAC_PI_4 + FRAC_PI_4 / (1.5 * FPS))).abs() < 1e-6, "{b:?}");
        // Past the clip's end: one frame, straight to the target.
        assert!(swept(3, -FRAC_PI_4, false, 1., 1.5).angle.abs() < 1e-6);
        // Back half (9 = PULLLEFTToRIGHT) sweeps toward pi the short way round.
        let b = swept(9, -3. * FRAC_PI_4, false, 1., 0.5);
        let want = wrap_angle(-3. * FRAC_PI_4 + wrap_angle(PI + 3. * FRAC_PI_4) / (0.5 * FPS));
        assert!((b.angle - want).abs() < 1e-6 && b.target == PI, "{b:?}");
    }

    #[test]
    fn moving_object_filters_rate_limit_each_output() {
        let mut b = MovingObjectBlock { blend: 0.7, ..Default::default() };
        b.enter(0);
        // mag 10, proj = (b*d + a*c)/mag = 2: spd01 +0.03, spdCurve +0.01, eF +0.03 a frame.
        b.update(0, false, [10., 0., 2., 0., 0.5], 1., 0., &CURVE);
        assert_eq!((b.spd01, b.spd_curve, b.e_f), (0.03, 0.01, 0.03));
        // Category 0 chases unclamped and leaves blend alone.
        b.update(0, false, [0., -1., 0., 0., 0.5], 1., 0., &CURVE);
        assert!((b.angle - 0.034_906_585).abs() < 1e-7 && b.blend == 0.7, "{b:?}");
    }

    #[test]
    fn moving_object_enter_requests_the_mirror_by_side_and_stance() {
        // 82bb6860: k even -> vfunc32()==0, k odd -> vfunc32()==1, k 0 -> no mirror.
        // vfunc32 82b97388: natural and relative stance both 0 or both 1.
        for (k, natural, relative, mirror) in [
            (2, 0, 0, false),
            (2, 0, 1, true),
            (2, 2, 2, true),
            (1, 0, 0, true),
            (1, 1, 1, true),
            (1, 0, 1, false),
            (8, 1, 0, true),
            (0, 0, 0, false),
            (0, 0, 1, false),
        ] {
            assert_eq!(mirror_request(k, natural, relative), mirror, "k={k} {natural}/{relative}");
        }
    }

    #[test]
    fn moving_object_takes_its_category_from_direction() {
        let attr = |name: &str, text: &str, boolean_byte: u8| GraphAttribute {
            name: name.into(),
            text: text.into(),
            float_bits: 0,
            boolean_byte,
        };
        let parse = |a: &[GraphAttribute]| Operation::parse(&Attributes::new(a));
        let mo = |category, blend_out_over_next_cycle| Operation::MovingObject {
            category,
            blend_out_over_next_cycle,
        };
        let name = attr("name", "MovingObject", 0);
        assert_eq!(parse(&[name.clone(), attr("direction", "PUSHLEFT", 0)]), mo(1, false));
        assert_eq!(
            parse(&[
                name.clone(),
                attr("direction", "PULLToPUSHRIGHT", 0),
                attr("BlendOutOverNextCycle", "true", 1),
            ]),
            mo(12, true)
        );
        assert_eq!(parse(&[name.clone(), attr("direction", "still", 0)]), mo(0, false));
        let init = [attr("name", "InitMovingObjects", 0)];
        assert_eq!(parse(&init), Operation::InitMovingObjects);
    }
}
