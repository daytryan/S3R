//! WipeoutGround300's bail-path kernels against TU3 itself, run in the PowerPC interpreter (see
//! `physics::tu3_oracle`) with the port's VMX convention (`Tu3::port_vmx`): a mismatch is a
//! structural difference, never a rounding question the binary cannot decide (audit section 6).
//! Run with
//! `SKATE3_ASSET_ROOT=<install>/assets cargo test -p skate-core wipeout_oracle -- --include-ignored`.
use super::*;
use crate::physics::tu3_oracle::{self, Rng, Tu3, check};
use crate::point_graph::PointGraph;
use math::V;
use profiles::Profile;

/// `this+16`: ProcessedPhysIn. `this+52 + 16k`: profile handles, `[handle+4]` the row.
pub(super) struct Wipe {
    pub o: Tu3,
    pub this: u32,
    pub processed: u32,
    pub rows: [u32; 5],
}

/// TU3's own leaf static initialisers of the VMX constant block 0x830BD300..0x830BD600 (the
/// image holds them zeroed): e.g. 82F826F8 `0x830BD350 = splat(1e-6)`, 82F825D0 `+320 = 1.0`,
/// 82F825F0 `+4A0 = -1.0`, 82F82610 `+380 = 0`, 82F826B0 `+330 = [0, -9.8, 0, 0]` (gravity).
pub(crate) fn statics(o: &mut Tu3) {
    for init in [
        0x82F8_2588, 0x82F8_25B0, 0x82F8_25D0, 0x82F8_25F0, 0x82F8_2610, 0x82F8_2630, 0x82F8_2650,
        0x82F8_2670, 0x82F8_2690, 0x82F8_26B0, 0x82F8_26F8, 0x82F8_2718, 0x82F8_27E8, 0x82F8_2808, 0x82F8_2828,
        0x82F8_2848, 0x82F8_2868, 0x82F8_2888, 0x82F8_28A8, 0x82F8_28C8, 0x82F8_2BA0, 0x82F8_2BC0,
        0x82F8_2BE0, 0x82F8_2C20, 0x82F8_2C40, 0x82F8_2C60, 0x82F8_2C80, 0x82F8_2CA0, 0x82F8_2CC0,
        0x82F8_2D00,
    ] {
        o.call(init, &[]);
    }
    assert_eq!(o.u32(0x830B_D350), 0x3586_37BD);
    assert_eq!(o.u32(0x830B_D4A0), 0xBF80_0000);
    assert_eq!(o.u32(0x830B_D334), 0xC11C_CCCD);
}

pub(super) fn wipe() -> Wipe {
    let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
    assert_eq!(o.u32(0x82D3_B9C8), 0x7D88_02A6, "0x82D3B9C8 is not `mflr r12`: not the TU3 image");
    o.port_vmx();
    statics(&mut o);
    let this = o.alloc(1024);
    let processed = o.alloc(3072);
    o.set_u32(this + 16, processed);
    let rows = [0; 5].map(|_| o.alloc(192));
    for (k, row) in rows.iter().enumerate() {
        // The handle is inline at this+52+16k (82D3C430 `addi r10,r3,52`); `+4` is the row.
        o.set_u32(this + 52 + 16 * k as u32 + 4, *row);
    }
    Wipe { o, this, processed, rows }
}

const FLOATS: [(u32, fn(&mut State) -> &mut f32); 9] = [
    (304, |s| &mut s.time),
    (308, |s| &mut s.settled_time),
    (312, |s| &mut s.time_until_teleport),
    (316, |s| &mut s.impaled_time),
    (320, |s| &mut s.no_support_time),
    (324, |s| &mut s.response_time),
    (328, |s| &mut s.extra_weight_zero_time),
    (332, |s| &mut s.slow_time),
    (336, |s| &mut s.surface_height),
];
const FLOATS2: [(u32, fn(&mut State) -> &mut f32); 13] = [
    (492, |s| &mut s.sideways_input),
    (496, |s| &mut s.forward_input),
    (500, |s| &mut s.orientation),
    (504, |s| &mut s.collision_weight),
    (508, |s| &mut s.controlled_weight),
    (512, |s| &mut s.control_time),
    (516, |s| &mut s.retained_sideways_input),
    (520, |s| &mut s.retained_forward_input),
    (524, |s| &mut s.response_scalar),
    (528, |s| &mut s.extra_weight),
    (532, |s| &mut s.maximum_speed),
    (536, |s| &mut s.response_start_speed),
    (540, |s| &mut s.response_change),
];
const VECTORS: [(u32, fn(&mut State) -> &mut V); 8] = [
    (352, |s| &mut s.board_offset),
    (368, |s| &mut s.forward),
    (384, |s| &mut s.right),
    (400, |s| &mut s.angular_velocity),
    (416, |s| &mut s.retained_tilt),
    (432, |s| &mut s.velocity),
    (448, |s| &mut s.retained_velocity),
    (464, |s| &mut s.predicted_position),
];
const BYTES: [(u32, fn(&mut State) -> &mut bool); 22] = [
    (480, |s| &mut s.over),
    (481, |s| &mut s.slow),
    (482, |s| &mut s.material_eleven_response),
    (483, |s| &mut s.air_collision_mode),
    (484, |s| &mut s.request_teleport),
    (485, |s| &mut s.move_board),
    (486, |s| &mut s.special_surface),
    (487, |s| &mut s.below_surface),
    (488, |s| &mut s.retained_velocity_active),
    (564, |s| &mut s.prevent_manual),
    (565, |s| &mut s.ignore_reset),
    (566, |s| &mut s.reset_ever),
    (567, |s| &mut s.ever_settled),
    (568, |s| &mut s.recovery_eligible),
    (569, |s| &mut s.teleport_pending),
    (570, |s| &mut s.ever_impaled),
    (571, |s| &mut s.direction_initialized),
    (572, |s| &mut s.material_ten_response),
    (573, |s| &mut s.imminent_surface_twelve),
    (574, |s| &mut s.allow_retained_velocity),
    (575, |s| &mut s.response_finished),
    (576, |s| &mut s.surface_query),
];
const WORDS: [(u32, fn(&mut State) -> &mut u32); 2] =
    [(544, |s| &mut s.board_move_frames), (548, |s| &mut s.response_count)];
const SIGNED: [(u32, fn(&mut State) -> &mut i32); 3] = [
    (552, |s| &mut s.response_frames),
    (556, |s| &mut s.airborne_frames),
    (560, |s| &mut s.teleport_countdown),
];

impl Wipe {
    pub fn put_state(&mut self, s: &State) {
        let mut s = s.clone();
        let t = self.this;
        for (at, f) in FLOATS.iter().chain(&FLOATS2) {
            self.o.set_f32(t + at, *f(&mut s));
        }
        self.o.set_f32(t + 832, s.predicted_time);
        for (at, f) in VECTORS {
            self.o.set_v4(t + at, *f(&mut s));
        }
        for (at, f) in BYTES {
            self.o.set_u8(t + at, u8::from(*f(&mut s)));
        }
        for (at, f) in WORDS {
            self.o.set_u32(t + at, *f(&mut s));
        }
        for (at, f) in SIGNED {
            self.o.set_u32(t + at, *f(&mut s) as u32);
        }
        self.o.set_u32(t + 580, s.profile as u32);
    }
    pub fn state(&self) -> State {
        let mut s = State::default();
        let t = self.this;
        for (at, f) in FLOATS.iter().chain(&FLOATS2) {
            *f(&mut s) = self.o.f32(t + at);
        }
        s.predicted_time = self.o.f32(t + 832);
        for (at, f) in VECTORS {
            *f(&mut s) = self.o.v4(t + at);
        }
        for (at, f) in BYTES {
            *f(&mut s) = self.o.u8(t + at) != 0;
        }
        for (at, f) in WORDS {
            *f(&mut s) = self.o.u32(t + at);
        }
        for (at, f) in SIGNED {
            *f(&mut s) = self.o.u32(t + at) as i32;
        }
        s.profile = self.o.u32(t + 580) as usize;
        s
    }
    /// Lays a profile out at the native offsets `Profile` documents.
    pub fn put_profile(&mut self, k: usize, p: &Profile) {
        let r = self.rows[k];
        let o = &mut self.o;
        o.set_v4(r, p.roll_axis);
        o.set_v4(r + 16, p.horizontal_axis);
        o.set_v4(r + 32, p.align_euler);
        for i in 0..8 {
            o.set_f32(r + 64 + 4 * i as u32, p.spin_vs_time.x[i]);
            o.set_f32(r + 96 + 4 * i as u32, p.spin_vs_time.y[i]);
        }
        for (at, v) in [
            (128, p.direction_follow),
            (132, p.torque_velocity),
            (136, p.torque_distance),
            (140, p.spin_inertia),
            (148, p.ground_roll_torque),
            (152, p.sideways_spin),
            (156, p.forward_spin),
            (164, p.drift_maximum_speed),
            (168, p.drift_forward),
            (172, p.drift_sideways),
            (180, p.ground_align_torque),
            (184, p.tilt_degrees),
            (188, p.drift_drag),
        ] {
            o.set_f32(r + at, v);
        }
        for (at, v) in [
            (144, p.roll_on_ground),
            (160, p.horizontal_directed),
            (176, p.align_with_velocity),
            (177, p.align_ground_with_velocity),
        ] {
            o.set_u8(r + at, u8::from(v));
        }
    }
}

/// Every state field, bit for bit.
pub(super) fn check_state(step: usize, retail: &State, port: &State) {
    let (mut r, mut p) = (retail.clone(), port.clone());
    for (at, f) in FLOATS.iter().chain(&FLOATS2) {
        check(&format!("state+{at}"), step, &[*f(&mut r)], &[*f(&mut p)]);
    }
    check("state+832", step, &[r.predicted_time], &[p.predicted_time]);
    for (at, f) in VECTORS {
        check(&format!("state+{at}"), step, f(&mut r), f(&mut p));
    }
    for (at, f) in BYTES {
        assert_eq!(*f(&mut r), *f(&mut p), "step {step} state+{at}");
    }
    for (at, f) in WORDS {
        assert_eq!(*f(&mut r), *f(&mut p), "step {step} state+{at}");
    }
    for (at, f) in SIGNED {
        assert_eq!(*f(&mut r), *f(&mut p), "step {step} state+{at}");
    }
    assert_eq!(r.profile, p.profile, "step {step} state+580");
}

pub(super) fn random_profile(r: &mut Rng) -> Profile {
    let mut x = [0.0; 8];
    for i in 1..8 {
        x[i] = x[i - 1] + r.range(0.0, 0.5);
    }
    let axis = |r: &mut Rng| if r.chance(0.1) { [0.0; 4] } else { [r.edgy(-1.0, 1.0), r.edgy(-1.0, 1.0), r.edgy(-1.0, 1.0), 0.0] };
    Profile {
        roll_axis: axis(r),
        horizontal_axis: axis(r),
        align_euler: [r.edgy(-180.0, 180.0), r.edgy(-180.0, 180.0), r.edgy(-180.0, 180.0), 0.0],
        spin_vs_time: PointGraph { x, y: std::array::from_fn(|_| r.range(0.0, 2.0)) },
        direction_follow: r.edgy(0.0, 1.0),
        torque_velocity: r.range(0.0, 2.0),
        torque_distance: r.range(0.0, 2.0),
        spin_inertia: r.edgy(0.0, 1.0),
        roll_on_ground: r.chance(0.7),
        ground_roll_torque: r.range(0.0, 3.0),
        sideways_spin: r.edgy(-10.0, 10.0),
        forward_spin: r.edgy(-10.0, 10.0),
        horizontal_directed: r.chance(0.5),
        drift_maximum_speed: r.range(0.0, 8.0),
        drift_forward: r.edgy(-5.0, 5.0),
        drift_sideways: r.edgy(-5.0, 5.0),
        align_with_velocity: r.chance(0.7),
        align_ground_with_velocity: r.chance(0.7),
        ground_align_torque: r.range(0.0, 3.0),
        tilt_degrees: r.edgy(-45.0, 45.0),
        drift_drag: r.edgy(0.0, 1.0),
    }
}

/// A velocity: now and then zero, below the 1e-6 normalise threshold, vertical, huge or NaN (the
/// branch direction of an unordered compare is part of the contract).
pub(super) fn velocity(r: &mut Rng) -> V {
    if r.chance(0.01) {
        return [f32::NAN, r.range(-1.0, 1.0), 0.0, 0.0];
    }
    match (r.unit() * 10.0) as u32 {
        0 => [0.0; 4],
        1 => [r.range(-1e-7, 1e-7), r.range(-1.0, 1.0), r.range(-1e-7, 1e-7), 0.0],
        2 => [0.0, r.range(-20.0, 20.0), 0.0, r.range(-1.0, 1.0)],
        3 => r.v4(-1e-3, 1e-3),
        _ => [r.range(-15.0, 15.0), r.range(-15.0, 15.0), r.range(-15.0, 15.0), r.range(-1.0, 1.0)],
    }
}

/// Stick values with the 0.2 dead-zone edges, both zeros and (rarely) NaN.
pub(super) fn stick(r: &mut Rng) -> [f32; 2] {
    let one = |r: &mut Rng| match (r.unit() * 100.0) as u32 {
        0 => f32::NAN,
        1..=8 => 0.2,
        9..=16 => -0.2,
        17..=24 => 0.0,
        25..=32 => -0.0,
        33..=40 => r.range(-0.2, 0.2),
        _ => r.range(-1.0, 1.0),
    };
    [one(r), one(r)]
}

/// 82D3C3F8: profile select from the gesture (Processed+2916/2920), control clock, the followed
/// direction (+368/+384) and the projected control input (Processed+2824/2828 -> +492/+496).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_profile_prepare_matches_82d3c3f8() {
    let mut w = wipe();
    let mut r = Rng(0x5EED_0025_C3F8_0001);
    for sequence in 0..60 {
        let profiles: [Profile; 5] = std::array::from_fn(|_| random_profile(&mut r));
        for (k, p) in profiles.iter().enumerate() {
            w.put_profile(k, p);
        }
        let mut port = State::default();
        port.direction_initialized = r.chance(0.5);
        port.forward = velocity(&mut r);
        port.profile = (r.unit() * 5.0) as usize;
        port.control_time = r.range(0.0, 3.0);
        port.retained_sideways_input = r.range(-1.0, 1.0);
        port.retained_forward_input = r.range(-1.0, 1.0);
        w.put_state(&port);
        for step in 0..100 {
            port.velocity = velocity(&mut r);
            w.o.set_v4(w.this + 432, port.velocity);
            let (gesture, input) = (stick(&mut r), stick(&mut r));
            for (at, v) in [(2916, gesture[0]), (2920, gesture[1]), (2824, input[0]), (2828, input[1])] {
                w.o.set_f32(w.processed + at, v);
            }
            w.o.call(0x82D3_C3F8, &[w.this]);
            profiles::prepare(&mut port, &profiles, gesture, input);
            let at = sequence * 1000 + step;
            check_state(at, &w.state(), &port);
            assert_eq!(w.o.u32(w.this + 132), w.this + 52 + 16 * port.profile as u32, "step {at} +132");
        }
    }
}

/// Rows right, up, forward of yaw/pitch/roll, w lanes junk; translation in `-t..t`.
pub(super) fn frame(r: &mut Rng, t: f32) -> [V; 4] {
    if r.chance(0.05) {
        return std::array::from_fn(|_| if r.chance(0.3) { [0.0; 4] } else { r.v4(-2.0, 2.0) });
    }
    let (yaw, pitch, roll) = (r.range(-3.2, 3.2), r.range(-1.6, 1.6), r.range(-3.2, 3.2));
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    let forward = [sy * cp, -sp, cy * cp];
    let up0 = [sy * sp, cp, cy * sp];
    let right0 = [cy, 0.0, -sy];
    let up: [f32; 3] = std::array::from_fn(|i| up0[i] * cr - right0[i] * sr);
    let right: [f32; 3] = std::array::from_fn(|i| right0[i] * cr + up0[i] * sr);
    let w = if r.chance(0.5) { 0.0 } else { r.range(-1.0, 1.0) };
    [
        [right[0], right[1], right[2], w],
        [up[0], up[1], up[2], w],
        [forward[0], forward[1], forward[2], w],
        [r.range(-t, t), r.range(-t, t), r.range(-t, t), 1.0],
    ]
}

pub(super) fn put_frame(o: &mut Tu3, at: u32, m: &[V; 4]) {
    for (i, row) in m.iter().enumerate() {
        o.set_v4(at + 16 * i as u32, *row);
    }
}

/// A unit-ish normal, sometimes straight up, sideways, zero or steep.
pub(super) fn normal(r: &mut Rng) -> V {
    match (r.unit() * 8.0) as u32 {
        0 => [0.0, 1.0, 0.0, 0.0],
        1 => [0.0; 4],
        2 => [1.0, 0.0, 0.0, 0.0],
        _ => {
            let v = [r.range(-1.0, 1.0), r.range(-0.2, 1.0), r.range(-1.0, 1.0), 0.0];
            math::normalize_or(v, [0.0, 1.0, 0.0, 0.0])
        }
    }
}

/// Skeleton offsets the kernels read through `[this+8]`: +6448 the body manager, +6464
/// SkeletonState (+8016 its physical pose, 64 bytes a part), +6480 the 24 animation-pose parts,
/// +11920 the animation-to-world transform, +16144 the physical COM.
pub(super) const SKELETON_BYTES: u32 = 16_384;

/// 82D3D660 with 82BEC0E8 (angular estimate) and 82BE3B58 (torque) hooked: the control vector
/// it hands 82BE3B58, from the hands (82BE3220 composed, run), the effective transform
/// (Processed+192), the ground normal (Processed+464) and the profile row.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_ground_control_matches_82d3d660() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let mut r = Rng(0x5EED_0025_D660_0002);
    let mut active = 0;
    for step in 0..30_000 {
        let k = (r.unit() * 5.0) as usize;
        let profile = random_profile(&mut r);
        w.put_profile(k, &profile);
        w.o.set_u32(w.this + 132, w.this + 52 + 16 * k as u32);
        let mut port = State::default();
        port.velocity = velocity(&mut r);
        if r.chance(0.2) {
            port.velocity = math::scale(port.velocity, 0.15);
        }
        port.forward_input = r.edgy(-1.0, 1.0);
        let angular = if r.chance(0.1) { [0.0; 4] } else { r.v4(-8.0, 8.0) };
        w.put_state(&port);
        let effective = frame(&mut r, 5.0);
        put_frame(&mut w.o, w.processed + 192, &effective);
        let ground = normal(&mut r);
        w.o.set_v4(w.processed + 464, ground);
        let to_world = frame(&mut r, 50.0);
        put_frame(&mut w.o, skeleton + 11920, &to_world);
        let mut pose = [[[0.0; 4]; 4]; 24];
        for i in [3, 7] {
            pose[i] = frame(&mut r, 0.6);
            if r.chance(0.01) {
                pose[i][3][0] = f32::NAN;
            }
            put_frame(&mut w.o, skeleton + 6480 + 64 * i as u32, &pose[i]);
        }
        let mut control = None;
        w.o.call_hooked(0x82D3_D660, &[w.this], &[0x82BE_C0E8, 0x82BE_3B58], &mut |cpu, g, at| {
            let r3 = cpu.r[3] as u32;
            if at == 0x82BE_C0E8 {
                assert_eq!(cpu.r[4] as u32, skeleton + 6464);
                for (i, v) in angular.iter().enumerate() {
                    g.set_u32(r3 + 4 * i as u32, v.to_bits()).unwrap();
                }
            } else {
                assert_eq!(r3, skeleton);
                let at = cpu.r[4] as u32;
                control = Some(std::array::from_fn::<f32, 4, _>(|i| f32::from_bits(g.u32(at + 4 * i as u32).unwrap())));
            }
        });
        port.angular_velocity = angular;
        let hands = [3, 7].map(|i| crate::physics::skeleton_animation_record::compose_affine(&to_world, &pose[i])[3]);
        let ported = control_ground::control(&port, &effective, ground, &profile, hands);
        check("control", step, &control.expect("82BE3B58 called"), &ported);
        check_state(step, &w.state(), &port);
        active += usize::from(ported.iter().any(|v| *v != 0.0));
    }
    assert!(active > 10_000, "only {active} non-zero controls");
}

/// 82D3CB50 with 82BEC0E8 and 82BE3B58 hooked; 82473A08 (sin/cos), 82D3C770 (alignment),
/// 8296EC98 / 8286CD88 (angles) and 82481E10 (SpinVsTime) run. Compares the control vector and
/// the retained tilt, inputs and control clock it updates.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_profile_air_control_matches_82d3cb50() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let mut r = Rng(0x5EED_0025_CB50_0003);
    for sequence in 0..150 {
        let k = (r.unit() * 5.0) as usize;
        let profile = random_profile(&mut r);
        w.put_profile(k, &profile);
        w.o.set_u32(w.this + 132, w.this + 52 + 16 * k as u32);
        let mut port = State::default();
        port.retained_tilt = if r.chance(0.2) { [0.0; 4] } else { r.v4(-0.5, 0.5) };
        port.control_time = r.range(0.0, 4.0);
        port.retained_sideways_input = r.edgy(-1.0, 1.0);
        port.retained_forward_input = r.edgy(-1.0, 1.0);
        w.put_state(&port);
        for step in 0..100 {
            let at = sequence * 1000 + step;
            port.velocity = velocity(&mut r);
            port.forward = normal(&mut r);
            port.right = if r.chance(0.1) { [0.0; 4] } else { math::cross([0.0, 1.0, 0.0, 0.0], port.forward) };
            port.sideways_input = if r.chance(0.01) { f32::NAN } else { r.edgy(-1.0, 1.0) };
            port.forward_input = r.edgy(-1.0, 1.0);
            for (at, v) in [(432, port.velocity), (368, port.forward), (384, port.right)] {
                w.o.set_v4(w.this + at, v);
            }
            w.o.set_f32(w.this + 492, port.sideways_input);
            w.o.set_f32(w.this + 496, port.forward_input);
            let effective = frame(&mut r, 5.0);
            put_frame(&mut w.o, w.processed + 192, &effective);
            let input = stick(&mut r);
            w.o.set_f32(w.processed + 2824, input[0]);
            w.o.set_f32(w.processed + 2828, input[1]);
            let angular = if r.chance(0.1) { [0.0; 4] } else { r.v4(-8.0, 8.0) };
            let mut control = None;
            w.o.call_hooked(0x82D3_CB50, &[w.this], &[0x82BE_C0E8, 0x82BE_3B58], &mut |cpu, g, at| {
                let r3 = cpu.r[3] as u32;
                if at == 0x82BE_C0E8 {
                    for (i, v) in angular.iter().enumerate() {
                        g.set_u32(r3 + 4 * i as u32, v.to_bits()).unwrap();
                    }
                } else {
                    let at = cpu.r[4] as u32;
                    control = Some(std::array::from_fn::<f32, 4, _>(|i| f32::from_bits(g.u32(at + 4 * i as u32).unwrap())));
                }
            });
            port.angular_velocity = angular;
            let ported = control_profile_air::control(&mut port, &effective, input, &profile);
            check("control", at, &control.expect("82BE3B58 called"), &ported);
            check_state(at, &w.state(), &port);
        }
    }
}

/// 82D3DD40 with 82BEC0E8 and 82BE3B58 hooked; 82BD3D90 (length limit) runs. Spine from the
/// physical record (skeleton+8016, 64 bytes a part): pose 11 and 23.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_air_control_matches_82d3dd40() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let mut r = Rng(0x5EED_0025_DD40_0004);
    let mut active = 0;
    for step in 0..30_000 {
        let mut port = State::default();
        port.velocity = velocity(&mut r);
        w.put_state(&port);
        let hips = frame(&mut r, 3.0);
        let mut neck = hips[3];
        let up = if r.chance(0.1) { r.v4(-1.0, 1.0) } else { hips[1] };
        for i in 0..3 {
            neck[i] += up[i] * r.range(-0.6, 0.6);
        }
        if r.chance(0.05) {
            neck = hips[3];
        }
        put_frame(&mut w.o, skeleton + 8016 + 64 * 23, &hips);
        w.o.set_v4(skeleton + 8016 + 64 * 11 + 48, neck);
        let input = stick(&mut r);
        let input = if r.chance(0.2) { [input[0] * 2.5, input[1]] } else { input };
        w.o.set_f32(w.processed + 2824, input[0]);
        w.o.set_f32(w.processed + 2828, input[1]);
        let contact = r.chance(0.2);
        let angular = if r.chance(0.1) { [0.0; 4] } else { r.v4(-8.0, 8.0) };
        let mut control = None;
        w.o.call_hooked(0x82D3_DD40, &[w.this, u32::from(contact)], &[0x82BE_C0E8, 0x82BE_3B58], &mut |cpu, g, at| {
            let r3 = cpu.r[3] as u32;
            if at == 0x82BE_C0E8 {
                for (i, v) in angular.iter().enumerate() {
                    g.set_u32(r3 + 4 * i as u32, v.to_bits()).unwrap();
                }
            } else {
                let at = cpu.r[4] as u32;
                control = Some(std::array::from_fn::<f32, 4, _>(|i| f32::from_bits(g.u32(at + 4 * i as u32).unwrap())));
            }
        });
        assert_eq!(w.o.r3(), u32::from(!contact), "step {step} return");
        port.angular_velocity = angular;
        let ported = control_air::control(&port, neck, &hips, input, contact);
        assert_eq!(control.is_some(), ported.is_some(), "step {step} applied");
        if let (Some(c), Some(p)) = (control, ported) {
            check("control", step, &c, &p);
            active += usize::from(p.iter().any(|v| *v != 0.0));
        }
        check_state(step, &w.state(), &port);
    }
    assert!(active > 10_000, "only {active} non-zero controls");
}

/// A 26-part SkeletonBody from synthetic bone boxes (the kernels below only read and write the
/// live rates, inverse mass, drags and the physical record, which the tests then randomise).
pub(super) fn body() -> crate::physics::skeleton_body::SkeletonBody {
    use crate::math::Vector3;
    use crate::physics::{rigid_body::RetailSimulationStep, skeleton_animation_record::IDENTITY, skeleton_body::*};
    let bone = BoneSettings {
        mass_factor: 1.0,
        ragdoll_mass_factor: 1.0,
        has_collision: true,
        use_root_drive: false,
        volume_type: 0,
        volume_scalar: 1.0,
        num_parents: 1,
    };
    let settings = SkeletonBodySettings {
        density: 1000.0,
        root_radius: 0.1,
        root_half_length: 0.1,
        capsule_radius_scalar: 1.0,
        capsule_length_scalar: 1.0,
        ragdoll_inverse_mass_factor: 1.0,
        inertia_multiply_type: 0,
        inertia_factor: 1.0,
    };
    let definition =
        SkeletonBodyDefinition::new([Vector3::new(0.2, 0.3, 0.2); 24], [bone; 24], settings, None).unwrap();
    let step = RetailSimulationStep::fixed_60_hz(0, 0.0, Vector3::new(0.0, -9.8, 0.0));
    SkeletonBody::new(definition, &[IDENTITY; 24], IDENTITY, step)
}

/// The body chain behind `[skeleton+6448]`: manager, `[+24]` list, `[list+8]` entries (96 bytes a
/// part), `[list+28]` count; `[entry+76]` the 44-word rigid body (+32 velocity, +124 inverse mass,
/// +144 force accumulator, +172 cool-down), `[body+92]` its Inertia (+32/+36 linear/angular drag).
pub(super) struct Chain {
    pub manager: u32,
    rigid: [u32; 26],
    inertia: [u32; 26],
}

pub(super) fn chain(o: &mut Tu3, skeleton: u32) -> Chain {
    let manager = o.alloc(256);
    o.set_u32(skeleton + 6448, manager);
    let list = o.alloc(32);
    o.set_u32(manager + 24, list);
    let entries = o.alloc(96 * 26);
    o.set_u32(list + 8, entries);
    o.set_u32(list + 28, 26);
    let rigid: [u32; 26] = std::array::from_fn(|i| {
        let rb = o.alloc(176);
        o.set_u32(entries + 96 * i as u32 + 76, rb);
        rb
    });
    let inertia = std::array::from_fn(|i| {
        let at = o.alloc(48);
        o.set_u32(rigid[i] + 92, at);
        at
    });
    Chain { manager, rigid, inertia }
}

/// The live fields [`Chain::check`] compares, to prove a kernel wrote something.
fn live(body: &crate::physics::skeleton_body::SkeletonBody) -> Vec<u32> {
    body.bodies()
        .iter()
        .flat_map(|p| {
            let (v, f) = (p.rates.linear_velocity, p.rates.force_acceleration);
            [v.x, v.y, v.z, f.x, f.y, f.z, p.inertia.linear_drag, p.inertia.angular_drag]
        })
        .map(f32::to_bits)
        .collect()
}

impl Chain {
    /// Random live rates and drags into both the port body and the guest, and the physical record
    /// pose translations (SkeletonState1552 = skeleton+8016).
    pub fn randomise(
        &self,
        o: &mut Tu3,
        r: &mut Rng,
        body: &mut crate::physics::skeleton_body::SkeletonBody,
        skeleton: u32,
    ) {
        use crate::math::Vector3;
        for (i, part) in body.bodies_mut().iter_mut().enumerate() {
            let v = if r.chance(0.05) { [0.0; 4] } else { r.v4(-12.0, 12.0) };
            let f = r.v4(-50.0, 50.0);
            part.rates.linear_velocity = Vector3::new(v[0], v[1], v[2]);
            part.rates.force_acceleration = Vector3::new(f[0], f[1], f[2]);
            part.rates.cool_down = (r.unit() * 30.0) as u32;
            part.inertia.inverse_mass = r.range(0.05, 2.0);
            part.inertia.linear_drag = r.range(0.0, 60.0);
            part.inertia.angular_drag = r.range(0.0, 60.0);
            let (rb, inertia) = (self.rigid[i], self.inertia[i]);
            o.set_v4(rb + 32, v);
            o.set_v4(rb + 144, f);
            o.set_u32(rb + 172, part.rates.cool_down);
            o.set_f32(rb + 124, part.inertia.inverse_mass);
            o.set_f32(inertia + 16, part.inertia.inverse_mass);
            o.set_f32(inertia + 32, part.inertia.linear_drag);
            o.set_f32(inertia + 36, part.inertia.angular_drag);
        }
        for i in 0..26 {
            let t = [
                r.range(-3.0, 3.0),
                r.range(-1.0, 3.0),
                r.range(-3.0, 3.0),
                if r.chance(0.5) { 1.0 } else { 0.0 },
            ];
            body.record.pose[i][3] = t;
            o.set_v4(skeleton + 8016 + 64 * i as u32 + 48, t);
        }
    }
    /// Every live field the bail kernels write: velocity xyz (w is the guest's own), the force
    /// accumulator, cool-down and both drags.
    pub fn check(&self, o: &Tu3, step: usize, body: &crate::physics::skeleton_body::SkeletonBody) {
        for (i, part) in body.bodies().iter().enumerate() {
            let (rb, inertia) = (self.rigid[i], self.inertia[i]);
            let v = part.rates.linear_velocity;
            let f = part.rates.force_acceleration;
            check(&format!("part {i} velocity"), step, &o.v4(rb + 32)[..3], &[v.x, v.y, v.z]);
            check(&format!("part {i} force"), step, &o.v4(rb + 144)[..3], &[f.x, f.y, f.z]);
            assert_eq!(o.u32(rb + 172), part.rates.cool_down, "step {step} part {i} cool-down");
            check(
                &format!("part {i} drags"),
                step,
                &[o.f32(inertia + 32), o.f32(inertia + 36)],
                &[part.inertia.linear_drag, part.inertia.angular_drag],
            );
        }
    }
}

/// 82BEC0E8: the SkeletonState angular estimate from the physical record (+3216 positions,
/// +3632 velocities, +4464/+4480 COM and its velocity, +4656 fractional masses). Its reciprocal
/// is `vrefp` + two Newton steps: D11-tolerant in principle, bit-exact under the port convention.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_angular_estimate_matches_82bec0e8() {
    let mut w = wipe();
    let state = w.o.alloc(4800);
    let out = w.o.alloc(16);
    let mut r = Rng(0x5EED_0025_C0E8_0005);
    let mut record = crate::physics::skeleton_body::SkeletonPhysicalRecord::default();
    for step in 0..20_000 {
        let com = r.v4(-5.0, 5.0);
        let com_velocity = r.v4(-8.0, 8.0);
        record.centre_of_mass = com;
        record.centre_of_mass_velocity = com_velocity;
        let mut fractional = [0.0; 24];
        for i in 0..26 {
            let reach = if r.chance(0.1) { 0.005 } else { 1.0 };
            let mut p = r.v4(-reach, reach);
            p[3] = if r.chance(0.5) { 1.0 } else { r.range(-1.0, 1.0) };
            record.positions[i] = std::array::from_fn(|k| com[k] + p[k]);
            record.velocities[i] = if r.chance(0.05) { com_velocity } else { r.v4(-15.0, 15.0) };
            w.o.set_v4(state + 3216 + 16 * i as u32, record.positions[i]);
            w.o.set_v4(state + 3632 + 16 * i as u32, record.velocities[i]);
            if i < 24 {
                fractional[i] = r.edgy(0.0, 0.2);
                w.o.set_f32(state + 4656 + 4 * i as u32, fractional[i]);
            }
        }
        w.o.set_v4(state + 4464, com);
        w.o.set_v4(state + 4480, com_velocity);
        w.o.call(0x82BE_C0E8, &[out, state]);
        check("angular", step, &w.o.v4(out), &torque::angular_velocity(&record, &fractional));
    }
}

/// 82BE3B58 with 82BE7788 (the per-part push into the force accumulator) run, against
/// torque::apply on a live body.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_torque_matches_82be3b58() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    let c = chain(&mut w.o, skeleton);
    let control_at = w.o.alloc(16);
    let mut body = body();
    let mut r = Rng(0x5EED_0025_3B58_0006);
    let mut moved = 0;
    for step in 0..3_000 {
        c.randomise(&mut w.o, &mut r, &mut body, skeleton);
        let before = live(&body);
        let com = if r.chance(0.1) {
            body.record.pose[(r.unit() * 23.0) as usize + 1][3]
        } else {
            r.v4(-2.0, 2.0)
        };
        let control = match (r.unit() * 10.0) as u32 {
            0 => [0.0; 4],
            1 => r.v4(-1e-4, 1e-4),
            _ => r.v4(-6.0, 6.0),
        };
        w.o.set_v4(skeleton + 16144, com);
        w.o.set_v4(control_at, control);
        w.o.call(0x82BE_3B58, &[skeleton, control_at]);
        torque::apply(&mut body, com, control);
        c.check(&w.o, step, &body);
        moved += usize::from(live(&body) != before);
    }
    assert!(moved > 1_500, "{moved} steps changed the body");
}

/// 82BE74C8 (normal removal), 82BE75B0 with 82BD41B0 (blend and distance limit), 82BE76B0
/// (speed limit), 82BE32D0 with 82D9CD60/82D9CE58 (wipeout drag), 82BE3998 (special surface)
/// on the live body chain.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_body_kernels_match_82be74c8_75b0_76b0_32d0_3998() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    let c = chain(&mut w.o, skeleton);
    let vector_at = w.o.alloc(16);
    let mut body = body();
    let mut r = Rng(0x5EED_0025_74C8_0007);
    let mut moved = 0;
    for step in 0..10_000 {
        c.randomise(&mut w.o, &mut r, &mut body, skeleton);
        let before = live(&body);
        match step % 5 {
            0 => {
                let n = normal(&mut r);
                w.o.set_v4(vector_at, n);
                w.o.call(0x82BE_74C8, &[c.manager, vector_at]);
                body::remove_normal_velocity(&mut body, n);
            }
            1 => {
                let target = r.v4(-12.0, 12.0);
                w.o.set_v4(vector_at, target);
                w.o.call(0x82BE_75B0, &[c.manager, vector_at]);
                body::blend_velocity(&mut body, target);
            }
            2 => {
                let maximum = if r.chance(0.5) { 5.0 } else { r.range(0.0, 20.0) };
                w.o.set_v4(vector_at, [maximum; 4]);
                w.o.call(0x82BE_76B0, &[c.manager, vector_at]);
                body::limit_velocity(&mut body, maximum);
            }
            3 => {
                let drag = r.edgy(-0.5, 1.5);
                w.o.call_floats(0x82BE_32D0, &[skeleton], &[drag]);
                body::set_wipeout_drag(&mut body, drag);
            }
            _ => {
                let height = r.range(-1.0, 2.0);
                w.o.call_floats(0x82BE_3998, &[skeleton], &[height]);
                body::special_surface(&mut body, height);
            }
        }
        c.check(&w.o, step, &body);
        moved += usize::from(live(&body) != before);
    }
    assert!(moved > 5_000, "{moved} steps changed the body");
}

/// 82D3C928: the selected profile's drift (+164..+188) from the followed direction, inputs and
/// velocity into parts 1..23.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_drift_matches_82d3c928() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let c = chain(&mut w.o, skeleton);
    let mut body = body();
    let mut r = Rng(0x5EED_0025_C928_0008);
    let mut moved = 0;
    for step in 0..5_000 {
        c.randomise(&mut w.o, &mut r, &mut body, skeleton);
        let before = live(&body);
        let k = (r.unit() * 5.0) as usize;
        let profile = random_profile(&mut r);
        w.put_profile(k, &profile);
        w.o.set_u32(w.this + 132, w.this + 52 + 16 * k as u32);
        let mut port = State::default();
        port.retained_velocity_active = r.chance(0.1);
        port.velocity = velocity(&mut r);
        port.forward = normal(&mut r);
        port.right = normal(&mut r);
        port.sideways_input = r.edgy(-1.0, 1.0);
        port.forward_input = r.edgy(-1.0, 1.0);
        w.put_state(&port);
        w.o.call(0x82D3_C928, &[w.this]);
        profiles::drift(&port, &mut body, &profile);
        c.check(&w.o, step, &body);
        moved += usize::from(live(&body) != before);
        check_state(step, &w.state(), &port);
    }
    assert!(moved > 2_500, "{moved} steps changed the body");
}

/// One call 82D3B9C8 made: target, r4, f1..f4, r8.
#[derive(Debug)]
struct Call {
    at: u32,
    r4: u32,
    f: [f32; 4],
    r8: u32,
}

/// Every call 82D3B9C8 makes, all hooked: their own effects are their own oracles.
const UPDATE_CALLS: [u32; 19] = [
    0x82D9_11A8, 0x82BE_76B0, 0x82D3_E348, 0x82BE_3998, 0x82D3_EC08, 0x82BE_32D0, 0x82D3_E478,
    0x82D3_E678, 0x82D3_C3F8, 0x82D3_CB50, 0x82D3_C928, 0x82D3_D660, 0x82D3_DD40, 0x82C0_40F0,
    0x82BD_FA88, 0x82C0_3F98, 0x82D3_EAE0, 0x82D2_D988, 0x82E0_99A0,
];

/// 82D3B9C8 with every call hooked: its inline arithmetic (update::spikes, drag,
/// advance_clocks, control, weights::update, drives::angular_root at drives+320..332) and the
/// calls it makes, in order, with their arguments. this+816 = 0 keeps the prediction tail to its
/// 82E099A0 query, whose `576 = below` is skate-game prediction.rs:50-51.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_update_matches_82d3b9c8() {
    use update::Control;
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let board = w.o.alloc(16);
    w.o.set_u32(w.this + 4, board);
    let requester = w.o.alloc(16);
    w.o.set_u32(w.this + 28, requester);
    let settings_at = w.o.alloc(512);
    w.o.set_u32(w.this + 40, settings_at);
    let manager = w.o.alloc(256);
    w.o.set_u32(skeleton + 6448, manager);
    let feedback = w.o.alloc(4096);
    w.o.set_u32(manager + 160, feedback);
    let drives_at = w.o.alloc(512);
    w.o.set_u32(skeleton + 6452, drives_at);
    // [0x830CFDA4] -> +204 -> +4: physics_animation/default, HookSoft* at +884/+888/+892.
    let (a, b, row) = (w.o.alloc(256), w.o.alloc(16), w.o.alloc(1024));
    w.o.set_u32(0x830C_FDA4, a);
    w.o.set_u32(a + 204, b);
    w.o.set_u32(b + 4, row);
    let mut r = Rng(0x5EED_0025_B9C8_0009);
    let mut chosen = [0usize; 4];
    for step in 0..30_000 {
        let mut port = State::default();
        port.airborne_frames = [-1, 0, 14, 15, 16, 40][(r.unit() * 6.0) as usize];
        port.special_surface = r.chance(0.2);
        port.surface_height = r.range(-1.0, 1.0);
        port.below_surface = r.chance(0.15);
        port.imminent_surface_twelve = r.chance(0.1);
        port.velocity = velocity(&mut r);
        port.slow = r.chance(0.5);
        port.over = r.chance(0.4);
        port.settled_time = r.edgy(0.0, 3.0);
        port.time = r.edgy(0.0, 2.0);
        port.response_time = if r.chance(0.02) { f32::NAN } else { r.edgy(0.0, 0.2) };
        port.extra_weight_zero_time = r.range(0.0, 2.0);
        port.extra_weight = r.edgy(-0.5, 1.0);
        port.slow_time = r.range(0.0, 2.0);
        port.maximum_speed = r.range(0.0, 10.0);
        port.collision_weight = r.edgy(0.0, 1.0);
        port.controlled_weight = r.edgy(0.0, 1.0);
        port.move_board = r.chance(0.3);
        port.board_move_frames = (r.unit() * 25.0) as u32;
        port.retained_velocity_active = r.chance(0.2);
        w.put_state(&port);
        w.o.set_u8(w.this + 816, 0);
        let flags_2488 = if r.chance(0.2) { 0x4000_0000 } else { 0 };
        let height_2924 = r.range(-1.0, 1.0);
        let position = r.v4(-3.0, 3.0);
        let mut velocity = velocity(&mut r);
        if r.chance(0.2) {
            velocity[1] = port.velocity[1] + r.range(4.0, 7.0);
        }
        if r.chance(0.05) {
            // An unordered rise: 82D3BB14 `ble` is taken on NaN, so it must not limit.
            port.velocity[1] = f32::NAN;
            w.put_state(&port);
            velocity[1] = r.range(4.0, 8.0);
        }
        let flags_2484 = if r.chance(0.4) { 0x100 } else { 0 };
        let flags_2472 = if r.chance(0.3) { 0x8_0000 } else { 0 };
        let gesture = stick(&mut r);
        let p = w.processed;
        w.o.set_u32(p + 2488, flags_2488);
        w.o.set_f32(p + 2924, height_2924);
        w.o.set_v4(p + 592, position);
        w.o.set_v4(p + 608, velocity);
        w.o.set_u32(p + 2484, flags_2484);
        w.o.set_u32(p + 2472, flags_2472);
        w.o.set_f32(p + 2916, gesture[0]);
        w.o.set_f32(p + 2920, gesture[1]);
        let contact_byte = r.chance(0.5);
        w.o.set_u8(feedback + 4082, u8::from(contact_byte));
        let settings = weights::Settings {
            remove_target_time: r.edgy(0.0, 2.0),
            remove_drives_time: r.edgy(0.0, 2.0),
            controlled_step: r.range(0.0, 0.2),
            collision_step: r.range(0.0, 0.2),
        };
        for (at, v) in [
            (272, settings.remove_target_time),
            (276, settings.remove_drives_time),
            (348, settings.controlled_step),
            (352, settings.collision_step),
        ] {
            w.o.set_f32(settings_at + at, v);
        }
        let hook = drives::Settings {
            bone: [[0.0; 5]; 24],
            root: [0.0; 4],
            strength: [0.0; 2],
            hook_spring: r.range(0.0, 50.0),
            hook_strength: r.range(0.0, 50.0),
            hook_damping: r.range(0.0, 5.0),
        };
        w.o.set_f32(row + 884, hook.hook_strength);
        w.o.set_f32(row + 888, hook.hook_spring);
        w.o.set_f32(row + 892, hook.hook_damping);
        let air_return = r.chance(0.5);
        let mut calls = Vec::new();
        w.o.call_hooked(0x82D3_B9C8, &[w.this], &UPDATE_CALLS, &mut |cpu, g, at| {
            let f = std::array::from_fn(|i| cpu.f[1 + i] as f32);
            if at == 0x82BE_76B0 {
                let limit: [f32; 4] = std::array::from_fn(|i| f32::from_bits(g.u32(cpu.r[4] as u32 + 4 * i as u32).unwrap()));
                assert_eq!(limit, [5.0; 4], "82BE76B0 limit");
            }
            if at == 0x82D3_DD40 {
                cpu.r[3] = u64::from(air_return);
            }
            calls.push(Call { at, r4: cpu.r[4] as u32, f, r8: cpu.r[8] as u32 });
        });

        // The port, in skate-game physics/wipeout_states/update.rs order.
        let mut expect: Vec<(u32, Option<u32>, Vec<f32>)> = Vec::new();
        let contact = contact_byte || port.below_surface;
        port.airborne_frames = if contact { -1 } else { port.airborne_frames.wrapping_add(1) };
        if !port.special_surface && flags_2488 & 0x4000_0000 != 0 {
            port.special_surface = true;
            port.surface_height = height_2924;
            expect.push((0x82D9_11A8, Some(10), vec![]));
        }
        if update::spikes(port.velocity, velocity) {
            expect.push((0x82BE_76B0, None, vec![]));
        }
        port.velocity = velocity;
        let speed = math::length(velocity);
        expect.push((0x82D3_E348, Some(u32::from(contact)), vec![]));
        if port.special_surface {
            port.below_surface = position[1] - port.surface_height < 0.0;
            expect.push((0x82BE_3998, None, vec![port.surface_height]));
        }
        if !port.below_surface {
            expect.push((0x82D3_EC08, Some(u32::from(contact)), vec![]));
            expect.push((0x82BE_32D0, None, vec![update::drag(&port, contact)]));
        }
        update::advance_clocks(&mut port, speed);
        expect.push((0x82D3_E478, Some(u32::from(contact)), vec![]));
        let control = update::control(&port, contact, flags_2484);
        let applied = match control {
            Control::Submerged => {
                expect.push((0x82D3_DD40, Some(1), vec![]));
                false
            }
            Control::Air { contact } => {
                expect.push((0x82D3_DD40, Some(u32::from(contact)), vec![]));
                air_return
            }
            Control::Response => {
                expect.push((0x82D3_E678, None, vec![]));
                false
            }
            Control::ProfileAir => {
                for at in [0x82D3_C3F8, 0x82D3_CB50, 0x82D3_C928] {
                    expect.push((at, None, vec![]));
                }
                true
            }
            Control::Ground => {
                for at in [0x82D3_C3F8, 0x82D3_D660] {
                    expect.push((at, None, vec![]));
                }
                true
            }
        };
        chosen[match control {
            Control::Submerged | Control::Air { .. } => 0,
            Control::Response => 1,
            Control::ProfileAir => 2,
            Control::Ground => 3,
        }] += 1;
        let weight = weights::update(&mut port, settings, contact, applied, flags_2472, gesture);
        expect.push((0x82C0_40F0, None, vec![0.0]));
        expect.push((
            0x82BD_FA88,
            Some(u32::from(port.retained_velocity_active)),
            vec![weight.start, weight.end, weight.controlled, weight.extra],
        ));
        if port.move_board && (port.board_move_frames as i32) < 21 {
            expect.push((0x82C0_3F98, Some(w.this + 352), vec![]));
            port.board_move_frames = port.board_move_frames.wrapping_add(1);
        }
        expect.push((0x82D3_EAE0, None, vec![]));
        expect.push((0x82E0_99A0, None, vec![]));
        port.surface_query = port.below_surface;

        let at: Vec<u32> = calls.iter().map(|c| c.at).collect();
        let want: Vec<u32> = expect.iter().map(|e| e.0).collect();
        assert_eq!(
            at.iter().map(|a| format!("{a:08X}")).collect::<Vec<_>>(),
            want.iter().map(|a| format!("{a:08X}")).collect::<Vec<_>>(),
            "step {step} calls"
        );
        for (call, (target, int, floats)) in calls.iter().zip(&expect) {
            if let Some(v) = int {
                let got = if *target == 0x82BD_FA88 { call.r8 } else { call.r4 };
                assert_eq!(got, *v, "step {step} {target:08X} integer argument");
            }
            check(&format!("{target:08X} f1.."), step, &call.f[..floats.len()], floats);
        }
        let root = drives::angular_root(&hook, weight.target_weight);
        check(
            "drives+320..328",
            step,
            &[w.o.f32(drives_at + 320), w.o.f32(drives_at + 324), w.o.f32(drives_at + 328)],
            &[root.spring_or_max_velocity, root.damping, root.max_strength],
        );
        assert_eq!(w.o.u32(drives_at + 332), root.drive_type as u32, "step {step} drives+332");
        check_state(step, &w.state(), &port);
    }
    assert!(chosen.iter().all(|&n| n > 1_000), "control branches {chosen:?}");
}

/// 82D3E478: the response clock (+552), finish flag (+575) and change (+540) from the velocity
/// (+432), the start speed (+536) and contact (r4). Start speeds just under 1 reach the
/// double-precision `fsub`/`fadd` at 82D3E53C/E540.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_response_counter_matches_82d3e478() {
    let mut w = wipe();
    let mut r = Rng(0x5EED_0025_E478_000A);
    let mut finished = [0usize; 2];
    for step in 0..40_000 {
        let mut port = State::default();
        port.response_frames = [-1, -5, 0, 1, 2, 2, 3, 6, 7, 7, 8, 20][(r.unit() * 12.0) as usize];
        port.velocity = velocity(&mut r);
        port.response_start_speed = match (r.unit() * 8.0) as u32 {
            0 => r.range(0.0, 1e-6),
            1 => r.range(0.999, 1.001),
            2 => 1.0,
            3 => f32::NAN,
            _ => r.edgy(0.0, 20.0),
        };
        port.response_change = if r.chance(0.01) { f32::NAN } else { r.edgy(-5.0, 5.0) };
        port.response_finished = r.chance(0.5);
        let contact = r.chance(0.5);
        w.put_state(&port);
        w.o.call(0x82D3_E478, &[w.this, u32::from(contact)]);
        let frames = port.response_frames;
        response::update_counter(&mut port, contact);
        check_state(step, &w.state(), &port);
        if frames == 2 || frames == 7 {
            finished[usize::from(frames == 7)] += 1;
        }
    }
    assert!(finished.iter().all(|&n| n > 3_000), "{finished:?}");
}

/// 82D3E5E8: the response strength from the peak speed (+532), response count (+548), slow
/// clock (+332) and the special-surface bytes (+486/+487).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_response_strength_matches_82d3e5e8() {
    let mut w = wipe();
    let mut r = Rng(0x5EED_0025_E5E8_000B);
    let mut nonzero = 0;
    for step in 0..30_000 {
        let mut port = State::default();
        port.maximum_speed = match (r.unit() * 10.0) as u32 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            _ => r.edgy(0.0, 30.0),
        };
        port.response_count = match (r.unit() * 8.0) as u32 {
            0 => u32::MAX - (r.unit() * 3.0) as u32,
            1 => 0x8000_0000 + (r.unit() * 4.0) as u32,
            _ => (r.unit() * 9.0) as u32,
        };
        port.slow_time = if r.chance(0.02) { f32::NAN } else { r.edgy(0.0, 2.0) };
        port.special_surface = r.chance(0.1);
        port.below_surface = r.chance(0.1);
        w.put_state(&port);
        let retail = w.o.call_floats(0x82D3_E5E8, &[w.this], &[]);
        let strength = port.response_strength();
        check("strength", step, &[retail], &[strength]);
        nonzero += usize::from(strength != 0.0);
    }
    assert!(nonzero > 10_000, "{nonzero} non-zero strengths");
}

/// 82D3E678 with 82D3E5E8 run and 82D3C2B0 (the response normal, its own port
/// `response::response_normal`) hooked to hand back a chosen normal: state, the Processed
/// +2824/+2828 control projection and the push into parts 1..23.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_response_trigger_matches_82d3e678() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let c = chain(&mut w.o, skeleton);
    let mut body = body();
    let mut r = Rng(0x5EED_0025_E678_000C);
    let mut moved = 0;
    for step in 0..20_000 {
        c.randomise(&mut w.o, &mut r, &mut body, skeleton);
        let before = live(&body);
        let mut port = State::default();
        port.velocity = velocity(&mut r);
        port.maximum_speed = if r.chance(0.01) { f32::NAN } else { r.edgy(0.0, 30.0) };
        port.response_count = (r.unit() * 6.0) as u32;
        port.slow_time = r.edgy(0.0, 1.5);
        port.special_surface = r.chance(0.05);
        port.below_surface = r.chance(0.05);
        port.response_frames = -1;
        port.response_time = r.range(0.0, 2.0);
        port.extra_weight = r.range(0.0, 1.0);
        port.response_scalar = r.range(0.0, 1.0);
        w.put_state(&port);
        let input = stick(&mut r);
        w.o.set_f32(w.processed + 2824, input[0]);
        w.o.set_f32(w.processed + 2828, input[1]);
        let n = match (r.unit() * 10.0) as u32 {
            0 => [0.0, 1.0, 0.0, 0.0],
            1 => [0.0; 4],
            _ => normal(&mut r),
        };
        w.o.call_hooked(0x82D3_E678, &[w.this], &[0x82D3_C2B0], &mut |cpu, g, _| {
            let out = cpu.r[3] as u32;
            for (k, x) in n.iter().enumerate() {
                g.set_u32(out + 4 * k as u32, x.to_bits()).unwrap();
            }
        });
        response::trigger(&mut port, &mut body, n, input);
        c.check(&w.o, step, &body);
        check_state(step, &w.state(), &port);
        moved += usize::from(live(&body) != before);
    }
    assert!(moved > 8_000, "{moved} steps pushed the body");
}

/// A SkeletonDrives on identity frames, every bone hung off part 0: the root-drive kernels only
/// touch target 0 (`drives+304..332`).
pub(super) fn drives_fixture() -> crate::physics::skeleton_body::SkeletonDrives {
    use crate::math::Vector3;
    use crate::physics::{rigid_body::RetailSimulationStep, skeleton_animation_record::IDENTITY, skeleton_body::*};
    let animation = AnimationDriveSettings {
        linear_strength: 1.0,
        linear_displacement: 1.0,
        angular_strength: 1.0,
        angular_displacement: 1.0,
    };
    let interpolation = DriveInterpolation { spring: [0.0; 2], strength: [0.0; 2], damping: [0.0; 2] };
    let settings = SkeletonDriveSettings {
        bone: BoneDriveSettings {
            animation: [animation; 2],
            collision_soft_displacement: 0.0,
            collision_soft_strength: 0.0,
            ragdoll_soft_displacement: 0.0,
            ragdoll_soft_strength: 0.0,
            transition_linear: interpolation,
            transition_angular: interpolation,
            transition_calls: 1.0,
        },
        enabled: true,
        strength: [1.0; 2],
        collision_strength: [[0.0; 2]; 24],
    };
    let parents = std::array::from_fn(|i| (i != 0 && i != 23).then_some(0));
    let step = RetailSimulationStep::fixed_60_hz(0, 0.0, Vector3::new(0.0, -9.8, 0.0));
    SkeletonDrives::new(&[IDENTITY; 24], &[IDENTITY; 24], &parents, IDENTITY, IDENTITY, step, settings).unwrap()
}

/// `[0x830CFDA4]+204 -> +4`: physics_animation/default, HookSoftStr/Dsp/Dmp at +884/+888/+892.
fn hook_row(o: &mut Tu3) -> u32 {
    let (a, b, row) = (o.alloc(256), o.alloc(16), o.alloc(1024));
    o.set_u32(0x830C_FDA4, a);
    o.set_u32(a + 204, b);
    o.set_u32(b + 4, row);
    row
}

/// Random HookSoft* into the row, edges included (zero, -0, NaN, negative, subnormal springs).
fn hook_settings(o: &mut Tu3, row: u32, r: &mut Rng) -> drives::Settings {
    let spring = match (r.unit() * 12.0) as u32 {
        0 => 0.0,
        1 => -0.0,
        2 => f32::NAN,
        3 => -r.range(0.0, 50.0),
        4 => r.range(0.0, 1e-30),
        _ => r.edgy(0.0, 400.0),
    };
    let s = drives::Settings {
        bone: [[0.0; 5]; 24],
        root: [0.0; 4],
        strength: [0.0; 2],
        hook_spring: spring,
        hook_strength: r.edgy(0.0, 50.0),
        hook_damping: r.range(0.0, 5.0),
    };
    o.set_f32(row + 884, s.hook_strength);
    o.set_f32(row + 888, s.hook_spring);
    o.set_f32(row + 892, s.hook_damping);
    s
}

fn put_linear(o: &mut Tu3, at: u32, d: crate::physics::drive_parameters::RetailDriveParams) {
    o.set_f32(at + 304, d.spring_or_max_velocity);
    o.set_f32(at + 308, d.damping);
    o.set_f32(at + 312, d.max_strength);
    o.set_u32(at + 316, d.drive_type as u32);
}

fn check_linear(o: &Tu3, at: u32, step: usize, d: crate::physics::drive_parameters::RetailDriveParams) {
    check(
        "drives+304..312",
        step,
        &[o.f32(at + 304), o.f32(at + 308), o.f32(at + 312)],
        &[d.spring_or_max_velocity, d.damping, d.max_strength],
    );
    assert_eq!(o.u32(at + 316), d.drive_type as u32, "step {step} drives+316");
}

/// 82BEB790: target 0's linear root drive (drives+304..316) from the weight and HookSoft*.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_linear_root_matches_82beb790() {
    let mut w = wipe();
    let drives_at = w.o.alloc(512);
    let row = hook_row(&mut w.o);
    let mut port = drives_fixture();
    let mut r = Rng(0x5EED_0025_B790_000D);
    for step in 0..20_000 {
        let settings = hook_settings(&mut w.o, row, &mut r);
        let weight = match (r.unit() * 10.0) as u32 {
            0 => f32::NAN,
            1 => -0.0,
            _ => r.edgy(-0.5, 1.5),
        };
        put_linear(&mut w.o, drives_at, port.targets.dynamics[0].linear);
        w.o.call_floats(0x82BE_B790, &[drives_at], &[weight]);
        drives::set_linear_root(&mut port, &settings, weight);
        check_linear(&w.o, drives_at, step, port.targets.dynamics[0].linear);
    }
}

/// 82D3E348 with 82D3E8C0 (release) and 82BEB790 run: the retained-velocity latch (+488/+574),
/// +432/+448, the clock rewind, the root drive and the release push into parts 1..23.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_retained_velocity_matches_82d3e348() {
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let c = chain(&mut w.o, skeleton);
    let drives_at = w.o.alloc(512);
    w.o.set_u32(skeleton + 6452, drives_at);
    let row = hook_row(&mut w.o);
    let mut body = body();
    let mut drives = drives_fixture();
    let mut r = Rng(0x5EED_0025_E348_000E);
    let mut paths = [0usize; 3];
    for step in 0..20_000 {
        c.randomise(&mut w.o, &mut r, &mut body, skeleton);
        let settings = hook_settings(&mut w.o, row, &mut r);
        let mut port = State::default();
        port.retained_velocity_active = r.chance(0.4);
        port.allow_retained_velocity = r.chance(0.7);
        port.velocity = velocity(&mut r);
        port.retained_velocity = velocity(&mut r);
        port.time = r.edgy(0.0, 3.0);
        w.put_state(&port);
        let flags = (if r.chance(0.6) { 8 } else { 0 }) | (if r.chance(0.3) { 0x10 } else { 0 });
        let mut com_velocity = velocity(&mut r);
        com_velocity[1] = match (r.unit() * 6.0) as u32 {
            0 => 2.0,
            1 => f32::NAN,
            _ => r.range(-4.0, 6.0),
        };
        w.o.set_u32(w.processed + 2468, flags);
        w.o.set_v4(w.processed + 608, com_velocity);
        let contact = r.chance(0.3);
        put_linear(&mut w.o, drives_at, drives.targets.dynamics[0].linear);
        let was = port.retained_velocity_active;
        w.o.call(0x82D3_E348, &[w.this, u32::from(contact)]);
        response::retained_velocity(&mut port, &mut body, &mut drives, &settings, contact, flags, com_velocity);
        c.check(&w.o, step, &body);
        check_linear(&w.o, drives_at, step, drives.targets.dynamics[0].linear);
        check_state(step, &w.state(), &port);
        paths[match (was, port.retained_velocity_active) {
            (true, false) => 0,
            (false, true) => 1,
            _ => 2,
        }] += 1;
    }
    assert!(paths.iter().all(|&n| n > 1_000), "release/latch/other {paths:?}");
}

/// The root_drive_* keys 82BEB32C..B408 hand 82B72420, in `drives::Settings::root` order.
const ROOT_KEYS: [u64; 4] =
    [0x8F84_492B_5338_ADC0, 0x0B97_4171_BA60_1791, 0x9661_DE4E_0288_7B77, 0x1272_733F_2CF1_B5DC];

/// A pose part: identity-aligned, a signed axis permutation, a random rotation, now and then
/// scaled or degenerate (a zero row), the translation random.
fn pose_part(r: &mut Rng) -> crate::physics::skeleton_animation_record::AnimationPartTransform {
    let mut m = match (r.unit() * 10.0) as u32 {
        0 | 1 => [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0; 4]],
        2 => {
            let s = |r: &mut Rng| if r.chance(0.5) { 1.0 } else { -1.0 };
            [[0.0, s(r), 0.0, 0.0], [0.0, 0.0, s(r), 0.0], [s(r), 0.0, 0.0, 0.0], [0.0; 4]]
        }
        3 => [r.v4(-1.0, 1.0), [0.0; 4], r.v4(-1.0, 1.0), [0.0; 4]],
        _ => {
            let q = r.v4(-1.0, 1.0);
            let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-3);
            let [x, y, z, w] = q.map(|v| v / n);
            let k = if r.chance(0.2) { r.range(0.5, 2.0) } else { 1.0 };
            [
                [k * (1.0 - 2.0 * (y * y + z * z)), k * 2.0 * (x * y + w * z), k * 2.0 * (x * z - w * y), 0.0],
                [k * 2.0 * (x * y - w * z), k * (1.0 - 2.0 * (x * x + z * z)), k * 2.0 * (y * z + w * x), 0.0],
                [k * 2.0 * (x * z + w * y), k * 2.0 * (y * z - w * x), k * (1.0 - 2.0 * (x * x + y * y)), 0.0],
                [0.0; 4],
            ]
        }
    };
    m[3] = [r.range(-2.0, 2.0), r.range(-2.0, 2.0), r.range(-2.0, 2.0), 1.0];
    if r.chance(0.1) {
        m[3] = [0.0, 0.0, 0.0, 1.0];
    }
    m
}

/// 82BEB0C8 with 82BECB40 (each bone's frames + enable, its own kernels) hooked and 82B72420
/// answering the four root keys: the residual (D+16), each bone's mode (+204), the two strengths
/// it is handed (f1/f2) and the 23 pose inverses (82BEB134..B200) against foot_ik::inverse_affine.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_drives_match_82beb0c8() {
    let mut w = wipe();
    let d = w.o.alloc(784 + 336 * 22);
    let table = w.o.alloc(748 + 36 * 24);
    w.o.set_u32(d + 8, table);
    let pose_at = w.o.alloc(64 * 24);
    let root_at = w.o.alloc(16);
    // [0x830CFDA4]: +212 -> +4 animation/default (+420/+424), +240 -> [0] the collection.
    let (a, b, row, collection) = (w.o.alloc(256), w.o.alloc(16), w.o.alloc(1024), w.o.alloc(16));
    w.o.set_u32(0x830C_FDA4, a);
    w.o.set_u32(a + 212, b);
    w.o.set_u32(b + 4, row);
    w.o.set_u32(a + 240, collection);
    w.o.set_u32(collection, collection + 8);
    let mut fixture = drives_fixture();
    let mut r = Rng(0x5EED_0025_B0C8_000F);
    let mut extra = 0;
    for step in 0..6_000 {
        let mut settings = drives::Settings {
            bone: [[0.0; 5]; 24],
            root: std::array::from_fn(|_| r.edgy(0.0, 2.0)),
            strength: [r.edgy(0.0, 3.0), r.edgy(0.0, 3.0)],
            hook_spring: 0.0,
            hook_strength: 0.0,
            hook_damping: 0.0,
        };
        for part in 0..24 {
            settings.bone[part] = std::array::from_fn(|_| r.edgy(0.0, 1.5));
            for (i, v) in settings.bone[part].iter().enumerate() {
                w.o.set_f32(table + 748 + 36 * part as u32 + 4 * i as u32, *v);
            }
        }
        for (i, v) in settings.root.iter().enumerate() {
            w.o.set_f32(root_at + 4 * i as u32, *v);
        }
        w.o.set_f32(row + 424, settings.strength[0]);
        w.o.set_f32(row + 420, settings.strength[1]);
        let pose: [_; 24] = std::array::from_fn(|_| pose_part(&mut r));
        for (k, m) in pose.iter().enumerate() {
            for (i, v) in m.iter().enumerate() {
                w.o.set_v4(pose_at + 64 * k as u32 + 16 * i as u32, *v);
            }
        }
        let weight = |r: &mut Rng| if r.chance(0.01) { f32::NAN } else { r.edgy(-0.5, 1.0) };
        let weights = drives::Weights {
            start: r.edgy(0.0, 1.0),
            end: r.edgy(0.0, 1.0),
            controlled: r.edgy(0.0, 1.0),
            upper_extra: weight(&mut r),
            lower_extra: weight(&mut r),
        };
        let mut handed = Vec::new();
        let mut inverses = None;
        let hooked = [0x82BE_CB40, 0x82B7_2420];
        w.o.floats(&[weights.start, weights.end, weights.controlled, weights.upper_extra, weights.lower_extra]);
        w.o.call_hooked(0x82BE_B0C8, &[d, pose_at], &hooked, &mut |cpu, g, at| {
            if at == 0x82B7_2420 {
                let key = ROOT_KEYS.iter().position(|k| *k == cpu.r[4]).expect("a root key");
                cpu.r[3] = u64::from(root_at + 4 * key as u32);
                return;
            }
            let bone = cpu.r[3] as u32;
            let f = |at: u32| f32::from_bits(g.u32(at).unwrap());
            handed.push(((bone - (d + 784)) / 336 + 1, g.u32(bone + 204).unwrap(), [cpu.f[1] as f32, cpu.f[2] as f32]));
            if inverses.is_none() {
                let at = cpu.r[5] as u32;
                inverses = Some(std::array::from_fn::<[[f32; 4]; 4], 24, _>(|k| {
                    std::array::from_fn(|i| std::array::from_fn(|j| f(at + 64 * k as u32 + 16 * i as u32 + 4 * j as u32)))
                }));
            }
        });
        let residual = drives::update(&mut fixture, &pose, &settings, weights);
        check("residual D+16", step, &[w.o.f32(d + 16)], &[residual]);
        assert_eq!(handed.len(), 22, "step {step} bones");
        for (part, mode, strengths) in &handed {
            let bone = fixture.bones[*part as usize].as_ref().unwrap();
            assert_eq!(*mode, bone.dynamics.mode, "step {step} part {part} mode");
            check(&format!("part {part} strengths"), step, strengths, &bone.dynamics.strengths);
            extra += usize::from(*mode == 5);
        }
        let inverses = inverses.unwrap();
        for k in 1..24 {
            let port = crate::animation::foot_ik::inverse_affine(&pose[k]);
            for i in 0..4 {
                check(&format!("inverse {k} row {i}"), step, &inverses[k][i], &port[i]);
            }
        }
    }
    assert!(extra > 6_000, "{extra} extra-mode bones");
}

/// `[0x830CFDA4]+264 -> +4`: physics_wipeout/default (+80/+84 over speed and time, +280/+284/
/// +292/+296 the recovery clocks), randomised into both sides.
fn wipeout_row(o: &mut Tu3) -> u32 {
    let (a, b, row) = (o.alloc(512), o.alloc(16), o.alloc(512));
    o.set_u32(0x830C_FDA4, a);
    o.set_u32(a + 264, b);
    o.set_u32(b + 4, row);
    row
}

fn recovery_settings(o: &mut Tu3, row: u32, r: &mut Rng) -> recovery::Settings {
    let s = recovery::Settings {
        minimum_time: r.edgy(0.0, 4.0),
        minimum_settled: r.edgy(0.0, 2.0),
        maximum_time: r.edgy(0.0, 12.0),
        fade_time: r.edgy(0.0, 1.0),
        over_speed: r.edgy(0.0, 3.0),
        over_minimum_time: r.edgy(0.0, 2.0),
    };
    for (at, v) in [
        (280, s.minimum_time),
        (284, s.minimum_settled),
        (292, s.maximum_time),
        (296, s.fade_time),
        (80, s.over_speed),
        (84, s.over_minimum_time),
    ] {
        o.set_f32(row + at, v);
    }
    s
}

/// A clock: mostly in range, now and then exactly on a threshold or NaN.
fn clock(r: &mut Rng, hi: f32) -> f32 {
    if r.chance(0.01) { f32::NAN } else { r.edgy(0.0, hi) }
}

/// 82D3EAE0 with 82D3E980 run: the sticky recovery bytes (+564..+570), the settle clock, the
/// impaled / pending-teleport clocks and the countdown (+560) into the request (+484).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_recovery_matches_82d3eae0() {
    let mut w = wipe();
    let row = wipeout_row(&mut w.o);
    let mut r = Rng(0x5EED_0025_EAE0_0010);
    let mut requested = 0;
    for step in 0..40_000 {
        let settings = recovery_settings(&mut w.o, row, &mut r);
        let mut port = State::default();
        port.prevent_manual = r.chance(0.1);
        port.ignore_reset = r.chance(0.2);
        port.reset_ever = r.chance(0.3);
        port.ever_settled = r.chance(0.3);
        port.over = r.chance(0.4);
        port.recovery_eligible = r.chance(0.3);
        port.ever_impaled = r.chance(0.2);
        port.teleport_pending = r.chance(0.3);
        port.request_teleport = r.chance(0.1);
        port.teleport_countdown = [-1, -1, -1, 0, 1, 2, 3][(r.unit() * 7.0) as usize];
        port.time = clock(&mut r, 10.0);
        port.settled_time = clock(&mut r, 2.0);
        port.response_time = clock(&mut r, 12.0);
        port.impaled_time = clock(&mut r, 1.0);
        port.time_until_teleport = clock(&mut r, 0.1);
        w.put_state(&port);
        let f2468 = u32::from(r.chance(0.1));
        let f2472 = (if r.chance(0.2) { 0x8000_0000 } else { 0 })
            | (if r.chance(0.3) { 0x0010_0000 } else { 0 })
            | (if r.chance(0.3) { 0x1000_0000 } else { 0 });
        let f2484 = if r.chance(0.2) { 0x0040_0000 } else { 0 };
        let timestep = if r.chance(0.01) { f32::NAN } else { r.edgy(0.0, 0.05) };
        let p = w.processed;
        w.o.set_u32(p + 2468, f2468);
        w.o.set_u32(p + 2472, f2472);
        w.o.set_u32(p + 2484, f2484);
        w.o.set_f32(p + 2604, timestep);
        w.o.call(0x82D3_EAE0, &[w.this]);
        port.manage_recovery(f2468, f2472, f2484, timestep, &settings);
        check_state(step, &w.state(), &port);
        requested += usize::from(port.request_teleport);
    }
    assert!(requested > 5_000, "{requested} teleport requests");
}

/// 82D3ED30 with 82D3E5E8 run: over (+480), slow (+481) and the no-support clock (+320) from
/// the hips (skeleton+10464 = velocities[23]) and neck (+10112 = velocities[1]) speeds and the
/// compliant byte `[[skeleton+6448]+160]+4070`.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_post_physics_matches_82d3ed30() {
    let mut w = wipe();
    let row = wipeout_row(&mut w.o);
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let manager = w.o.alloc(256);
    w.o.set_u32(skeleton + 6448, manager);
    let feedback = w.o.alloc(4096 + 16);
    w.o.set_u32(manager + 160, feedback);
    let mut r = Rng(0x5EED_0025_ED30_0011);
    let mut over = 0;
    for step in 0..40_000 {
        let settings = recovery_settings(&mut w.o, row, &mut r);
        let mut port = State::default();
        port.extra_weight_zero_time = clock(&mut r, 1.0);
        port.time = clock(&mut r, 3.0);
        port.no_support_time = clock(&mut r, 2.0);
        port.over = r.chance(0.5);
        port.slow = r.chance(0.5);
        port.maximum_speed = r.edgy(0.0, 20.0);
        port.response_count = (r.unit() * 6.0) as u32;
        port.slow_time = r.edgy(0.0, 2.0);
        port.special_surface = r.chance(0.1);
        port.below_surface = r.chance(0.1);
        w.put_state(&port);
        let speed = |r: &mut Rng| {
            let mut v = velocity(r);
            if r.chance(0.5) {
                v = v.map(|x| x * 0.1);
            }
            v
        };
        let hips = speed(&mut r);
        let neck = speed(&mut r);
        w.o.set_v4(skeleton + 10464, hips);
        w.o.set_v4(skeleton + 10112, neck);
        let compliant = r.chance(0.5);
        w.o.set_u8(feedback + 4070, u8::from(compliant));
        w.o.call(0x82D3_ED30, &[w.this]);
        port.post_physics(hips, neck, compliant, &settings);
        check_state(step, &w.state(), &port);
        over += usize::from(port.over);
    }
    assert!(over > 3_000, "{over} over");
}

/// 82D3EC08 with both material responses run (82D917A0/82D91908 + 82D91A40/82D91B28/82C1E170 for
/// material 10 at this+144, 82D91C98 + 82D91DF8/82D92160/82E0A4E8 for material 11 at this+224;
/// each `+0` the skeleton, `+4` Processed) and the requester 82D911A8 hooked. Episodes from
/// reset (82D91708 / 82D91BF8), so the helpers' hidden clocks and velocities show up in the
/// phases, the requests and the parts 1..23 they push.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_contact_response_matches_82d3ec08() {
    use contact_response::{ContactResponse, ContactResponseInput};
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(w.this + 8, skeleton);
    let c = chain(&mut w.o, skeleton);
    let feedback = w.o.alloc(4096 + 16);
    w.o.set_u32(c.manager + 160, feedback);
    let (ten, eleven) = (w.this + 144, w.this + 224);
    for helper in [ten, eleven] {
        w.o.set_u32(helper, skeleton);
        w.o.set_u32(helper + 4, w.processed);
    }
    let requester = w.o.alloc(16);
    w.o.set_u32(w.this + 28, requester);
    let mut body = body();
    let mut response = ContactResponse::new();
    let mut r = Rng(0x5EED_0025_EC08_0012);
    let (mut phases, mut requests, mut active) = ([0usize; 5], 0, 0);
    for episode in 0..600 {
        w.o.call(0x82D9_1708, &[ten]);
        w.o.call(0x82D9_1BF8, &[eleven]);
        response.reset();
        let mut port = State::default();
        let mut velocity = r.v4(-8.0, 8.0);
        for frame in 0..50 {
            let step = episode * 1_000 + frame;
            c.randomise(&mut w.o, &mut r, &mut body, skeleton);
            port.special_surface = r.chance(0.05);
            port.predicted_time = if r.chance(0.03) { f32::NAN } else { r.edgy(0.0, 0.6) };
            w.put_state(&port);
            // A falling, sliding body: mostly continuous, now and then a jolt.
            velocity = if r.chance(0.05) {
                r.v4(-10.0, 10.0)
            } else {
                math::add(velocity, [r.range(-0.3, 0.3), r.range(-0.6, 0.2), r.range(-0.3, 0.3), 0.0])
            };
            let contact = r.chance(0.5);
            // Most material-10 contacts face the fall, so 82D917A0's `-dot > 2` opens phase 1.
            let ten_normal = if !r.chance(0.3) {
                None
            } else if r.chance(0.7) {
                let n = math::normalize_or(math::add(velocity, r.v4(-2.0, 2.0)), [0.0, 1.0, 0.0, 0.0]);
                Some([-n[0], -n[1], -n[2], 0.0])
            } else {
                Some(normal(&mut r))
            };
            let eleven_normal = (r.chance(0.3)).then(|| normal(&mut r));
            let axis = if r.chance(0.2) { [0.0, 1.0, 0.0, 0.0] } else { normal(&mut r) };
            let control = stick(&mut r);
            let p = w.processed;
            w.o.set_v4(p + 608, velocity);
            w.o.set_v4(p + 224, axis);
            w.o.set_f32(p + 2824, control[0]);
            w.o.set_f32(p + 2828, control[1]);
            w.o.set_u8(feedback + 4079, u8::from(ten_normal.is_some()));
            w.o.set_u8(feedback + 4080, u8::from(eleven_normal.is_some()));
            w.o.set_v4(feedback + 1168, ten_normal.unwrap_or(normal(&mut r)));
            w.o.set_v4(feedback + 1184, eleven_normal.unwrap_or(normal(&mut r)));
            let mut asked = Vec::new();
            w.o.call_hooked(0x82D3_EC08, &[w.this, u32::from(contact)], &[0x82D9_11A8], &mut |cpu, _, _| {
                asked.push(cpu.r[4] as u32);
            });
            let output = response.update(
                &mut body,
                ContactResponseInput {
                    com_velocity_608: velocity,
                    material10_normal: ten_normal,
                    material11_normal: eleven_normal,
                    effective_axis_224: axis,
                    wipeout_control_2824_2828: control,
                },
            );
            let requested = contact_response::apply(&mut port, output, contact);
            assert_eq!(asked, requested.into_iter().collect::<Vec<_>>(), "step {step} requests");
            assert_eq!(w.o.u32(ten + 72), output.material10_phase, "step {step} phase");
            c.check(&w.o, step, &body);
            check_state(step, &w.state(), &port);
            phases[output.material10_phase.min(4) as usize] += 1;
            requests += usize::from(requested.is_some());
            active += usize::from(output.material11_active_73);
        }
    }
    assert!(phases.iter().all(|&n| n > 100), "phases {phases:?}");
    assert!(requests > 1_000 && active > 3_000, "{requests} requests, {active} active");
}

/// 82BDFBE8: the wipeout root (skeleton+11920) and its inverse (+11984) from the animation hips
/// (+11792), the physical hips (+9488, record pose[23]) and their velocity (+10464, velocities
/// [23]), the physical COM (+16144), Processed+608 and the state clock Processed+2664
/// (`[skeleton+0]` is Processed).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn wipeout_oracle_roots_match_82bdfbe8() {
    use crate::physics::skeleton_root::SkeletonRootFrames;
    let mut w = wipe();
    let skeleton = w.o.alloc(SKELETON_BYTES);
    w.o.set_u32(skeleton, w.processed);
    let dt = f32::from_bits(0x3C88_8889);
    let mut r = Rng(0x5EED_0025_FBE8_0013);
    for step in 0..20_000 {
        let animation_hips = pose_part(&mut r);
        let physical_hips = pose_part(&mut r);
        let hips_velocity = velocity(&mut r);
        let mut com = r.v4(-3.0, 3.0);
        for lane in &mut com {
            if r.chance(0.15) {
                *lane = if r.chance(0.5) { -0.0 } else { 0.0 };
            }
        }
        let com_velocity = velocity(&mut r);
        let clock = match (r.unit() * 8.0) as u32 {
            0 => 0.0,
            1 => -dt,
            2 => f32::NAN,
            3 => 1.0,
            4 => r.range(1.0, 3.0),
            _ => r.edgy(-0.1, 1.2),
        };
        for (at, m) in [(11792, &animation_hips), (9488, &physical_hips)] {
            for (i, row) in m.iter().enumerate() {
                w.o.set_v4(skeleton + at + 16 * i as u32, *row);
            }
        }
        w.o.set_v4(skeleton + 10464, hips_velocity);
        w.o.set_v4(skeleton + 16144, com);
        w.o.set_v4(w.processed + 608, com_velocity);
        w.o.set_f32(w.processed + 2664, clock);
        w.o.call(0x82BD_FBE8, &[skeleton]);
        let mut roots = SkeletonRootFrames::default();
        skeleton::update_roots(&mut roots, &animation_hips, &physical_hips, hips_velocity, com, com_velocity, clock);
        for (at, m) in [(11920, &roots.animation_to_world), (11984, &roots.world_to_animation)] {
            for (i, row) in m.iter().enumerate() {
                check(&format!("skeleton+{at} row {i}"), step, &w.o.v4(skeleton + at + 16 * i as u32), row);
            }
        }
    }
}
