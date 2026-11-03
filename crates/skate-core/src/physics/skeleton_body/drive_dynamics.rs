//! BoneDrives::EnableDrive82BECCE0 and mode-five interpolation82BECFB8.
//! A bone shares its interpolation counter between the LOCAL and LOCAL_ROOT
//! channels. Dispatch order is therefore observable even within one tick.
use crate::physics::drive_parameters::{RetailDriveDynamics, RetailDriveParams, RetailDriveType};

const FREQUENCY: f32 = f32::from_bits(0x426F_FFFF);
const FREQUENCY_SQUARED: f32 = f32::from_bits(0x4560_FFFE);
const HARD_VELOCITY: f32 = f32::from_bits(0x4415_FFFF);
const HARD_STRENGTH: f32 = f32::from_bits(0x470C_9FFF);

#[derive(Clone, Copy, Debug)]
pub struct AnimationDriveSettings {
    pub linear_strength: f32,
    pub linear_displacement: f32,
    pub angular_strength: f32,
    pub angular_displacement: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DriveInterpolation {
    pub spring: [f32; 2],
    pub strength: [f32; 2],
    pub damping: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct BoneDriveSettings {
    pub animation: [AnimationDriveSettings; 2],
    pub collision_soft_displacement: f32,
    pub collision_soft_strength: f32,
    pub ragdoll_soft_displacement: f32,
    pub ragdoll_soft_strength: f32,
    pub transition_linear: DriveInterpolation,
    pub transition_angular: DriveInterpolation,
    pub transition_calls: f32,
}

#[derive(Clone, Debug)]
pub struct BoneDriveDynamics {
    pub channels: [RetailDriveDynamics; 2],
    pub mode: u32,
    pub strengths: [f32; 2],
    pub transition_counter: f32,
    pub transition_active: bool,
}
impl Default for BoneDriveDynamics {
    fn default() -> Self {
        Self {
            channels: [RetailDriveDynamics {
                linear: RetailDriveParams::DISABLED,
                angular: RetailDriveParams::DISABLED,
            }; 2],
            mode: 0,
            strengths: [0.0; 2],
            transition_counter: 0.0,
            transition_active: false,
        }
    }
}
impl BoneDriveDynamics {
    pub fn enable(&mut self, channel: usize, strength: f32, settings: BoneDriveSettings) {
        assert!(channel < 2);
        if self.mode > 5 {
            return;
        }
        if self.mode != 5 {
            self.transition_active = false;
        }
        let animation = settings.animation[channel];
        let dynamics = match self.mode {
            0 => RetailDriveDynamics {
                linear: hard(
                    animation.linear_displacement * FREQUENCY,
                    animation.linear_strength * FREQUENCY_SQUARED,
                ),
                angular: hard(
                    animation.angular_displacement * FREQUENCY,
                    animation.angular_strength * FREQUENCY_SQUARED,
                ),
            },
            1 => RetailDriveDynamics {
                linear: hard(HARD_VELOCITY, HARD_STRENGTH),
                angular: hard(HARD_VELOCITY, HARD_STRENGTH),
            },
            2 => RetailDriveDynamics {
                linear: hard(0.0, 0.0),
                angular: hard(
                    animation.angular_displacement * FREQUENCY,
                    animation.angular_strength * FREQUENCY_SQUARED,
                ),
            },
            3 => RetailDriveDynamics {
                linear: soft(0.0, 0.0, 0.0),
                angular: soft(
                    settings.collision_soft_displacement * strength,
                    200.0,
                    (settings.collision_soft_strength * strength) * FREQUENCY_SQUARED,
                ),
            },
            4 => RetailDriveDynamics {
                // 82BECED0 `ble` is taken on NaN: an unordered strength drops the linear drive.
                linear: if !(strength > 0.5) {
                    soft(0.0, 0.0, 0.0)
                } else {
                    soft(
                        (settings.ragdoll_soft_displacement * strength) * 0.5,
                        200.0,
                        (strength * settings.ragdoll_soft_strength) * FREQUENCY_SQUARED,
                    )
                },
                angular: soft(
                    settings.ragdoll_soft_displacement * strength,
                    200.0,
                    (strength * settings.ragdoll_soft_strength) * FREQUENCY_SQUARED,
                ),
            },
            5 => {
                if !self.transition_active {
                    self.transition_active = true;
                    self.transition_counter = 1.0;
                } else if self.transition_counter < settings.transition_calls {
                    self.transition_counter += 1.0;
                }
                let progress = self.transition_counter / settings.transition_calls;
                RetailDriveDynamics {
                    linear: interpolate(settings.transition_linear, strength, progress),
                    angular: interpolate(settings.transition_angular, strength, progress),
                }
            }
            _ => unreachable!(),
        };
        self.channels[channel] = dynamics;
    }
}

fn hard(velocity: f32, strength: f32) -> RetailDriveParams {
    RetailDriveParams {
        spring_or_max_velocity: velocity,
        damping: 0.0,
        max_strength: strength,
        drive_type: RetailDriveType::HardDrive,
    }
}
fn soft(spring: f32, damping: f32, strength: f32) -> RetailDriveParams {
    RetailDriveParams {
        spring_or_max_velocity: spring,
        damping,
        max_strength: strength,
        drive_type: RetailDriveType::SoftDrive,
    }
}
fn interpolate(settings: DriveInterpolation, strength: f32, progress: f32) -> RetailDriveParams {
    // 82BED0C0..D8 `fmsubs` (end * strength - start * strength, rounded once), then
    // 82BED100..114 `fmadds`; damping's difference is a plain `fsubs` (82BED0E0/E8).
    let spring0 = settings.spring[0] * strength;
    let strength0 = settings.strength[0] * strength;
    let spring = settings.spring[1].mul_add(strength, -spring0).mul_add(progress, spring0);
    let force = settings.strength[1].mul_add(strength, -strength0).mul_add(progress, strength0);
    let damping =
        (settings.damping[1] - settings.damping[0]).mul_add(progress, settings.damping[0]);
    let nonnegative = |value| if value >= 0.0 { value } else { 0.0 };
    RetailDriveParams {
        spring_or_max_velocity: nonnegative(spring) * FREQUENCY,
        max_strength: nonnegative(force) * FREQUENCY_SQUARED,
        damping: nonnegative(damping),
        drive_type: RetailDriveType::HardDrive,
    }
}

//// TU3 oracles for the per-bone drive path on the native bone. EnableDrive 82BECCE0 (mode 5 =
/// 82BECFB8): `+204` mode, `+216..+228` the soft settings, `+232..+276` the transition settings,
/// `+280`/`+284` the counter and flag, the animation_drives rows behind 0x830C0BA4 (local) /
/// 0x830C0BB4 (root), transition_calls from the 0x830D0850 default (no physics_skeleton_drives
/// collection); out at `+128+32c` (linear) and `+144+32c` (angular). 82BECB40 adds `+312` part,
/// `+192/+196` parents, `+200/+201` active, `+208/+212` strengths and the frames
/// (SetDriveFrames 82BD6FD0 -> 82BD3A10 / 82BD3BD0) at `+64c`.
#[cfg(test)]
mod oracle {
    use super::*;
    use crate::physics::skeleton_animation_record::{AnimationPartTransform, IDENTITY};
    use crate::physics::skeleton_body::bone_drive_frames;
    use crate::physics::tu3_oracle::{self, Rng, Tu3};

    const TYPES: [RetailDriveType; 3] = [RetailDriveType::NoDrive, RetailDriveType::SoftDrive, RetailDriveType::HardDrive];

    fn words(p: RetailDriveParams) -> [u32; 4] {
        [p.spring_or_max_velocity.to_bits(), p.damping.to_bits(), p.max_strength.to_bits(), p.drive_type as u32]
    }

    fn value(r: &mut Rng, lo: f32, hi: f32) -> f32 {
        if r.chance(0.02) { f32::NAN } else { r.edgy(lo, hi) }
    }

    struct Fixture {
        o: Tu3,
        bone: u32,
        rows: [u32; 2],
    }

    impl Fixture {
        fn new() -> Self {
            let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
            o.port_vmx();
            // The quaternion setters' sign masks (static initialisers; the image holds them
            // zeroed): 82F83360 0x830BDB80 = [0, 0, -0, -0], 82F83390 0x830BDB60 = [0, -0, 0, -0],
            // 82F833C0 0x830BDB50 = [0, -0, -0, 0].
            for init in [0x82F8_3360, 0x82F8_3390, 0x82F8_33C0] {
                o.call(init, &[]);
            }
            assert_eq!([0x830B_DB80u32, 0x830B_DB60, 0x830B_DB50].map(|at| o.u32(at + 4)), [0, 0x8000_0000, 0x8000_0000]);
            let bone = o.alloc(320);
            let rows = [o.alloc(16), o.alloc(16)];
            o.set_u32(0x830C_0BA4, rows[0]);
            o.set_u32(0x830C_0BB4, rows[1]);
            let (vault, drives) = (o.alloc(256), o.alloc(16));
            o.set_u32(0x830C_FDA4, vault);
            o.set_u32(vault + 240, drives);
            Self { o, bone, rows }
        }

        /// Now and then a fresh counter/flag and fresh outputs, so stale values are compared too.
        fn scramble(&mut self, r: &mut Rng, port: &mut BoneDriveDynamics) {
            if r.chance(0.3) {
                port.transition_active = r.chance(0.5);
                port.transition_counter = value(r, 0.0, 12.0);
                self.o.set_u8(self.bone + 284, u8::from(port.transition_active));
                self.o.set_f32(self.bone + 280, port.transition_counter);
            }
            if r.chance(0.3) {
                for c in 0..2 {
                    for k in 0..2 {
                        let params = RetailDriveParams {
                            spring_or_max_velocity: r.range(-9.0, 9.0),
                            damping: r.range(-9.0, 9.0),
                            max_strength: r.range(-9.0, 9.0),
                            drive_type: TYPES[(r.unit() * 3.0) as usize],
                        };
                        let channel = &mut port.channels[c];
                        if k == 0 { channel.linear = params } else { channel.angular = params }
                        let at = self.bone + 128 + 32 * c as u32 + 16 * k as u32;
                        for (i, w) in words(params).into_iter().enumerate() {
                            self.o.set_u32(at + 4 * i as u32, w);
                        }
                    }
                }
            }
        }

        fn settings(&mut self, r: &mut Rng) -> BoneDriveSettings {
            let v = |r: &mut Rng| value(r, -5.0, 60.0);
            let animation = [(); 2].map(|_| AnimationDriveSettings {
                linear_strength: v(r),
                linear_displacement: v(r),
                angular_strength: v(r),
                angular_displacement: v(r),
            });
            let interpolation = |r: &mut Rng| DriveInterpolation {
                spring: [v(r), v(r)],
                strength: [v(r), v(r)],
                damping: [v(r), v(r)],
            };
            let settings = BoneDriveSettings {
                animation,
                collision_soft_displacement: v(r),
                collision_soft_strength: v(r),
                ragdoll_soft_displacement: v(r),
                ragdoll_soft_strength: v(r),
                transition_linear: interpolation(r),
                transition_angular: interpolation(r),
                transition_calls: [1.0, 2.0, 5.0, 10.0, 0.0, f32::NAN, value(r, 0.0, 12.0)]
                    [(r.unit() * 7.0) as usize],
            };
            for (c, a) in animation.iter().enumerate() {
                let row = [a.linear_strength, a.linear_displacement, a.angular_strength, a.angular_displacement];
                for (i, x) in row.into_iter().enumerate() {
                    self.o.set_f32(self.rows[c] + 4 * i as u32, x);
                }
            }
            let (l, a) = (settings.transition_linear, settings.transition_angular);
            let fields = [
                settings.collision_soft_displacement, settings.collision_soft_strength,
                settings.ragdoll_soft_displacement, settings.ragdoll_soft_strength,
                l.spring[0], l.spring[1], l.strength[0], l.strength[1], l.damping[0], l.damping[1],
                a.spring[0], a.spring[1], a.strength[0], a.strength[1], a.damping[0], a.damping[1],
            ];
            for (i, x) in fields.into_iter().enumerate() {
                self.o.set_f32(self.bone + 216 + 4 * i as u32, x);
            }
            self.o.set_f32(0x830D_0850, settings.transition_calls);
            settings
        }

        fn check(&self, step: usize, port: &BoneDriveDynamics) {
            for c in 0..2 {
                let at = self.bone + 128 + 32 * c as u32;
                let mine = [words(port.channels[c].linear), words(port.channels[c].angular)].concat();
                for (i, &y) in mine.iter().enumerate() {
                    let x = self.o.u32(at + 4 * i as u32);
                    assert!(
                        x == y || i % 4 != 3 && tu3_oracle::same(f32::from_bits(x), f32::from_bits(y)),
                        "step {step} mode {} channel {c} word {i}: retail {x:08X} port {y:08X}",
                        port.mode
                    );
                }
            }
            assert_eq!(self.o.u8(self.bone + 284), u8::from(port.transition_active), "step {step} +284");
            tu3_oracle::check("+280", step, &[self.o.f32(self.bone + 280)], &[port.transition_counter]);
        }
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
    fn wipeout_oracle_enable_drive_matches_82becce0() {
        let mut f = Fixture::new();
        let mut port = BoneDriveDynamics::default();
        let mut r = Rng(0x5EED_0025_CCE0_0016);
        let (mut modes, mut counted, mut half) = ([0usize; 8], 0, 0);
        for step in 0..60_000 {
            f.scramble(&mut r, &mut port);
            let settings = f.settings(&mut r);
            port.mode = [0, 1, 2, 3, 4, 4, 5, 5, 5, 6, 7][(r.unit() * 11.0) as usize];
            f.o.set_u32(f.bone + 204, port.mode);
            let channel = usize::from(r.chance(0.5));
            let strength = if r.chance(0.1) { 0.5 } else { value(&mut r, -0.5, 1.5) };
            f.o.floats(&[strength]);
            f.o.call(0x82BE_CCE0, &[f.bone, channel as u32]);
            let before = port.transition_counter;
            port.enable(channel, strength, settings);
            f.check(step, &port);
            modes[port.mode as usize] += 1;
            counted += usize::from(port.mode == 5 && !tu3_oracle::same(before, port.transition_counter));
            half += usize::from(port.mode == 4 && !(strength > 0.5));
        }
        assert!(modes.iter().all(|&n| n > 3_000) && counted > 5_000 && half > 2_000, "{modes:?} {counted} {half}");
    }

    /// A rigid frame: a unit quaternion's basis (now and then an axis-aligned quarter or half
    /// turn, so exact zeros and every quaternion branch show up) and a translation; w lanes 0/1.
    fn rigid(r: &mut Rng) -> AnimationPartTransform {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let q = match (r.unit() * 8.0) as u32 {
            0 => [0.0, 0.0, 0.0, 1.0],
            1 => {
                let mut q = [0.0; 4];
                q[(r.unit() * 3.0) as usize] = 1.0;
                q
            }
            2 => {
                let mut q = [0.0, 0.0, 0.0, half];
                q[(r.unit() * 3.0) as usize] = if r.chance(0.5) { half } else { -half };
                q
            }
            _ => {
                let q = r.v4(-1.0, 1.0);
                let n = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-3);
                q.map(|x| x / n)
            }
        };
        let [x, y, z, w] = q;
        let t = r.v4(-2.0, 2.0);
        [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w), 0.0],
            [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w), 0.0],
            [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y), 0.0],
            [t[0], t[1], t[2], 1.0],
        ]
    }

    /// 82BECB40(bone, pose, inverses, f1, f2): per active channel SetDriveFrames 82BD6FD0 from
    /// pose[part] / inverses[part] / inverses[parent], then EnableDrive; the drives oracle hooks it.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
    fn wipeout_oracle_bone_drive_matches_82becb40() {
        let mut f = Fixture::new();
        let (pose_at, inverse_at) = (f.o.alloc(24 * 64), f.o.alloc(24 * 64));
        let mut port = BoneDriveDynamics::default();
        let mut frames = [[0u32; 16]; 2];
        let mut r = Rng(0x5EED_0025_CB40_0018);
        let mut branches = [0usize; 4];
        for step in 0..20_000 {
            f.scramble(&mut r, &mut port);
            let settings = f.settings(&mut r);
            let part = 1 + (r.unit() * 22.0) as usize;
            let parent = [(r.unit() * 24.0) as usize, 23];
            let active = [r.chance(0.8), r.chance(0.8)];
            let mut pose = [IDENTITY; 24];
            let mut inverse = pose;
            for i in [part, parent[0], parent[1]] {
                pose[i] = rigid(&mut r);
                inverse[i] = rigid(&mut r);
            }
            for i in 0..24 {
                for k in 0..16 {
                    f.o.set_f32(pose_at + 64 * i as u32 + 4 * k as u32, pose[i][k / 4][k % 4]);
                    f.o.set_f32(inverse_at + 64 * i as u32 + 4 * k as u32, inverse[i][k / 4][k % 4]);
                }
            }
            port.mode = (r.unit() * 6.0) as u32;
            let strengths = [value(&mut r, -0.5, 1.5), value(&mut r, -0.5, 1.5)];
            f.o.set_u32(f.bone + 312, part as u32);
            f.o.set_u32(f.bone + 192, parent[0] as u32);
            f.o.set_u32(f.bone + 196, parent[1] as u32);
            f.o.set_u8(f.bone + 200, u8::from(active[0]));
            f.o.set_u8(f.bone + 201, u8::from(active[1]));
            f.o.set_u32(f.bone + 204, port.mode);
            f.o.floats(&strengths);
            f.o.call(0x82BE_CB40, &[f.bone, pose_at, inverse_at]);
            port.strengths = strengths;
            for c in 0..2 {
                if !active[c] {
                    continue;
                }
                // +0 child quaternion, +16 child translation, +32 parent quaternion, +48 parent
                // translation (its w lane is not modelled: carried over from retail below).
                let pair = bone_drive_frames(&pose[part], &inverse[part], &inverse[parent[c]]);
                for (k, frame) in [pair.body_a, pair.body_b].into_iter().enumerate() {
                    let (q, t) = (frame.orientation, frame.translation);
                    let w = f32::from_bits(frames[c][8 * k + 7]);
                    frames[c][8 * k..8 * k + 8]
                        .copy_from_slice(&[q.x, q.y, q.z, q.w, t.x, t.y, t.z, w].map(f32::to_bits));
                }
                port.enable(c, strengths[c], settings);
            }
            for c in 0..2 {
                for k in 0..16 {
                    let retail = f.o.u32(f.bone + 64 * c as u32 + 4 * k as u32);
                    if k % 8 == 7 {
                        frames[c][k] = retail;
                        continue;
                    }
                    assert!(
                        tu3_oracle::same(f32::from_bits(retail), f32::from_bits(frames[c][k])),
                        "step {step} channel {c} frame word {k}: retail {retail:08X} port {:08X}",
                        frames[c][k]
                    );
                }
            }
            f.check(step, &port);
            tu3_oracle::check("+208", step, &[f.o.f32(f.bone + 208), f.o.f32(f.bone + 212)], &strengths);
            if active[0] {
                // Which quaternion branch the child frame took: its largest component.
                let q: [f32; 4] = std::array::from_fn(|i| f32::from_bits(frames[0][i]));
                let big = (0..4).max_by(|&a, &b| q[a].abs().total_cmp(&q[b].abs())).unwrap();
                branches[big] += 1;
            }
        }
        assert!(branches.iter().all(|&n| n > 1_000), "{branches:?}");
    }
}
