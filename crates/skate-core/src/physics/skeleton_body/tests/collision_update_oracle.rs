//! TU3 oracle for SkeletonCollision::Update 82BD4A30 (and Reset 82BD5DA0), run live with 82BD3D90 (clamp_length),
//! 82BD5770 (CheckConflictingContacts) and 82BD5918 (impaled). Hooked: GetPartTransform 82585CB0
//! (the port's `body_frames`) and the other skater's entity-id virtual (`[vtable+44]`, 82BD541C).
//!
//! Native layout (this = r3): planes `+32i` (normal, part `+16`), count `+4084`; `+640+4p` age,
//! `+736+4p` priority, `+832+p` current, `+856+p` compliant; regions `+880/+912/+944/+976+4r`,
//! normal `+1008+16r`, part `+1200+4r`; `+1136/+1152/+1168/+1184` normals; bones `+1232+112p`;
//! specific parts `+3920+4i`, world `+3936+16i`, local `+3968+16i`, radius^2 `+4000+4i`,
//! current/recent `+4008+i`/`+4010+i`; physical record `+4016`, skeleton `+4020`; `+4024..+4064`
//! scalars; flags `+4068..+4082`. Input (r4): `+4` -> {begin, end} of 96-byte reports, then
//! `+16/+32/+48/+64` plane point, plane normal, reference velocity, COM velocity, `+80` dt,
//! `+84..+90` the flags. Settings: physics_skeleton `[[0x830CFDA4]+236]+4` (`+140`, `+216`),
//! physics_collision `[[0x830CFDA4]+268]+4` (`+116`, `+136`, `+140`, `+188`).
use super::*;
use crate::physics::contact::RetailContactMaterial;
use crate::physics::skeleton_animation_record::AnimationPartTransform;
use crate::physics::tu3_oracle::{self, check, Rng, Tu3};

type V = [f32; 4];
const UPDATE: u32 = 0x82BD_4A30;
const GET_PART_TRANSFORM: u32 = 0x8258_5CB0;
const MAX_REPORTS: u32 = 40;

struct Native {
    o: Tu3,
    this: u32,
    input: u32,
    list: u32,
    reports: u32,
    pairs: u32,
    bodies: u32,
    objects: u32,
    vtable: u32,
    entity_fn: u32,
    record: u32,
    parts: u32,
    skeleton_row: u32,
    collision_row: u32,
}

impl Native {
    fn new() -> Self {
        let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
        assert_eq!(o.u32(UPDATE), 0x7D88_02A6, "0x82BD4A30 is not `mflr r12`: not the TU3 image");
        o.port_vmx();
        crate::player::wipeout_state::oracle::statics(&mut o);
        let this = o.alloc(4096);
        let input = o.alloc(96);
        let list = o.alloc(16);
        o.set_u32(input + 4, list);
        let reports = o.alloc(96 * MAX_REPORTS);
        let pairs = o.alloc(112 * MAX_REPORTS);
        let bodies = o.alloc(144 * 2 * MAX_REPORTS);
        let objects = o.alloc(96 * MAX_REPORTS);
        let vtable = o.alloc(48);
        let entity_fn = o.alloc(16);
        o.set_u32(vtable + 44, entity_fn);
        let record = o.alloc(4672);
        o.set_u32(this + 4016, record);
        let (skeleton, chain, parts) = (o.alloc(32), o.alloc(16), o.alloc(26 * 96));
        o.set_u32(skeleton + 24, chain);
        o.set_u32(chain + 8, parts);
        o.set_u32(this + 4020, skeleton);
        let vault = o.alloc(512);
        o.set_u32(0x830C_FDA4, vault);
        let (handles, skeleton_row, collision_row) = (o.alloc(32), o.alloc(256), o.alloc(256));
        o.set_u32(vault + 236, handles);
        o.set_u32(handles + 4, skeleton_row);
        o.set_u32(vault + 268, handles + 16);
        o.set_u32(handles + 20, collision_row);
        Self {
            o,
            this,
            input,
            list,
            reports,
            pairs,
            bodies,
            objects,
            vtable,
            entity_fn,
            record,
            parts,
            skeleton_row,
            collision_row,
        }
    }
}

fn val(r: &mut Rng, lo: f32, hi: f32) -> f32 {
    if r.chance(0.005) { f32::NAN } else { r.edgy(lo, hi) }
}
fn vector(r: &mut Rng, lo: f32, hi: f32) -> V {
    std::array::from_fn(|i| if i == 3 && r.chance(0.7) { 0.0 } else { val(r, lo, hi) })
}
fn pick<T: Copy>(r: &mut Rng, items: &[T]) -> T {
    items[((r.unit() * items.len() as f32) as usize).min(items.len() - 1)]
}
fn unit_normal(r: &mut Rng) -> V {
    const AXES: [V; 6] = [
        [0.0, 1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0, 0.0],
    ];
    if r.chance(0.5) {
        return pick(r, &AXES);
    }
    let v = r.v4(-1.0, 1.0);
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-3);
    [v[0] / l, v[1] / l, v[2] / l, if r.chance(0.8) { 0.0 } else { r.range(-1.0, 1.0) }]
}
fn frame(r: &mut Rng) -> AnimationPartTransform {
    let mut m = [0; 4].map(|_| vector(r, -1.5, 1.5));
    m[3] = vector(r, -3.0, 3.0);
    m
}
fn body(r: &mut Rng) -> SkeletonContactBody {
    SkeletonContactBody {
        state_flags: if r.chance(0.3) { 1 } else { pick(r, &[0, 2, 4]) },
        inverse_mass: match (r.unit() * 20.0) as u32 {
            0 => 0.0,
            1 => f32::NAN,
            2 => r.range(0.0, 50.0),
            _ => r.edgy(0.0, 2.0),
        },
        linear_velocity: vector(r, -10.0, 10.0),
    }
}

fn settings(r: &mut Rng) -> SkeletonFeedbackSettings {
    SkeletonFeedbackSettings {
        body: SkeletonCollisionSettings {
            enabled: true,
            normal_material: RetailContactMaterial { static_friction: 0.0, dynamic_friction: 0.0, restitution: 0.0 },
            compliant: [true; 24],
            priority: [0.0; 24],
            effect_time: match (r.unit() * 8.0) as u32 {
                0 => 0.0,
                1 => 0.25,
                2 if r.chance(0.2) => f32::NAN,
                _ => r.edgy(0.0, 1.0),
            },
        },
        small_object_mass: r.edgy(0.0, 10.0),
        ground_plane_max_distance: r.edgy(0.0, 0.5),
        ground_plane_max_angle: r.edgy(0.0, 1.0),
        skater_scalar: r.edgy(0.0, 2.0),
        ai_scalar: r.edgy(0.0, 2.0),
        groin_offset: [0.0; 4],
        face_offset: [0.0; 4],
        groin_radius: 0.0,
        face_radius: 0.0,
    }
}

/// Fresh settings and persistent state (everything Update reads but does not reset), both sides.
fn episode(n: &mut Native, r: &mut Rng) -> SkeletonCollisionFeedback {
    let mut port = SkeletonCollisionFeedback::new(settings(r));
    let (o, t) = (&mut n.o, n.this);
    let s = port.settings;
    o.set_f32(n.skeleton_row + 216, s.body.effect_time);
    o.set_f32(n.skeleton_row + 140, s.small_object_mass);
    o.set_f32(n.collision_row + 116, s.skater_scalar);
    o.set_f32(n.collision_row + 136, s.ground_plane_max_distance);
    o.set_f32(n.collision_row + 140, s.ground_plane_max_angle);
    o.set_f32(n.collision_row + 188, s.ai_scalar);
    // Equal priorities let a frame fill all 20 planes.
    let flat = r.chance(0.3);
    for p in 0..24 {
        port.compliant[p] = r.chance(0.7);
        port.priority[p] = if flat {
            0.0
        } else if r.chance(0.01) { f32::NAN } else { pick(r, &[0.0, 0.0, 0.0, 0.5, 1.0, 2.0]) };
        port.contact_age[p] = val(r, -1.0, 1.0);
        let at = t + 4 * p as u32;
        o.set_u8(t + 856 + p as u32, u8::from(port.compliant[p]));
        o.set_f32(at + 736, port.priority[p]);
        o.set_f32(at + 640, port.contact_age[p]);
    }
    for (i, region) in port.regions.iter_mut().enumerate() {
        region.material_flags = (r.unit() * 128.0) as u32;
        o.set_u32(t + 976 + 4 * i as u32, region.material_flags);
    }
    for (i, point) in port.specific.iter_mut().enumerate() {
        point.part = if r.chance(0.8) { [23, 1][i] } else { (r.unit() * 24.0) as usize };
        point.local_point = vector(r, -0.3, 0.3);
        point.radius_squared = r.edgy(0.0, 0.05);
        point.recent = r.chance(0.5);
        let i = i as u32;
        o.set_u32(t + 3920 + 4 * i, point.part as u32);
        o.set_v4(t + 3968 + 16 * i, point.local_point);
        o.set_f32(t + 4000 + 4 * i, point.radius_squared);
        o.set_u8(t + 4010 + i, u8::from(point.recent));
    }
    port.timer = val(r, -1.0, 1.0);
    port.wipeout_times = [0; 3].map(|_| val(r, 0.0, 5.0));
    o.set_f32(t + 4024, port.timer);
    for (i, w) in port.wipeout_times.iter().enumerate() {
        o.set_f32(t + 4036 + 4 * i as u32, *w);
    }
    port
}

struct Frame {
    physical: SkeletonPhysicalRecord,
    weights: [f32; 24],
    frames: [AnimationPartTransform; 26],
    reports: Vec<SkeletonContactReport>,
    objects: Vec<bool>,
    dt: f32,
    plane_point: V,
    plane_normal: V,
    reference_velocity: V,
    com_velocity: V,
    flags: [bool; 7],
}

fn frame_inputs(r: &mut Rng, port: &SkeletonCollisionFeedback) -> Frame {
    let mut physical = SkeletonPhysicalRecord::default();
    for p in 0..26 {
        physical.pose[p] = frame(r);
        physical.velocities[p] = vector(r, -10.0, 10.0);
    }
    let weights = [0; 24].map(|_| val(r, 0.0, 1.0));
    let frames = [0; 26].map(|_| frame(r));
    let cluster = vector(r, -1.0, 1.0);
    let world = port.specific.map(|s| (s.part, super::collision_vector::transform(physical.pose[s.part], s.local_point)));
    let count = if r.chance(0.1) { 20 + (r.unit() * 20.0) as usize } else { (r.unit() * 12.0) as usize };
    let (mut reports, mut objects) = (Vec::new(), Vec::new());
    for _ in 0..count {
        let k = (r.unit() * 2.0) as usize;
        let part = if r.chance(0.15) { world[k].0 } else { (r.unit() * 24.0) as usize };
        let offset = vector(r, -0.1, 0.1);
        let point: V = if part == world[k].0 && r.chance(0.6) {
            std::array::from_fn(|i| world[k].1[i] + offset[i])
        } else if r.chance(0.4) {
            std::array::from_fn(|i| cluster[i] + offset[i] * 0.1)
        } else {
            vector(r, -2.0, 2.0)
        };
        let material = pick(r, &[0, 6, 10, 11, 12, 10, 11]).max((r.chance(0.1) as u32) * (r.unit() * 32.0) as u32);
        let tag = ((r.unit() * 65536.0) as u32) << 12 | material << 7 | (r.unit() * 128.0) as u32;
        let object = r.chance(0.85);
        let other_group = if object { pick(r, &[0, 3, 4, 4, 5, 5, 8, 8, 11, 11]) } else { 0 };
        let other_entity = (object && r.chance(0.7)).then(|| (r.unit() * 1.0e6) as i32 - 500_000);
        reports.push(SkeletonContactReport {
            part,
            normal: unit_normal(r),
            point,
            tag,
            other_group,
            other_entity,
            body_a: body(r),
            body_b: body(r),
            side_a: r.chance(0.5),
            solved_vector: vector(r, -300.0, 300.0),
        });
        objects.push(object);
    }
    let reference_velocity = match (r.unit() * 20.0) as u32 {
        0 | 1 => [0.0; 4],
        2 => vector(r, -1e-4, 1e-4),
        _ => vector(r, -10.0, 10.0),
    };
    Frame {
        physical,
        weights,
        frames,
        reports,
        objects,
        dt: val(r, 0.0, 0.05),
        plane_point: vector(r, -1.0, 1.0),
        plane_normal: if r.chance(0.7) { [0.0, 1.0, 0.0, 0.0] } else { unit_normal(r) },
        reference_velocity,
        com_velocity: vector(r, -10.0, 10.0),
        flags: [0.5, 0.2, 0.3, 0.2, 0.1, 0.3, 0.1].map(|p| r.chance(p)),
    }
}

fn write_frame(n: &mut Native, f: &Frame) {
    let o = &mut n.o;
    for p in 0..26u32 {
        for (k, row) in f.physical.pose[p as usize].iter().enumerate() {
            o.set_v4(n.record + 1552 + 64 * p + 16 * k as u32, *row);
        }
        o.set_v4(n.record + 3632 + 16 * p, f.physical.velocities[p as usize]);
    }
    for (p, w) in f.weights.iter().enumerate() {
        o.set_f32(n.record + 4560 + 4 * p as u32, *w);
    }
    let i = n.input;
    o.set_v4(i + 16, f.plane_point);
    o.set_v4(i + 32, f.plane_normal);
    o.set_v4(i + 48, f.reference_velocity);
    o.set_v4(i + 64, f.com_velocity);
    o.set_f32(i + 80, f.dt);
    for (k, flag) in f.flags.iter().enumerate() {
        o.set_u8(i + 84 + k as u32, u8::from(*flag));
    }
    o.set_u32(n.list, n.reports);
    o.set_u32(n.list + 4, n.reports + 96 * f.reports.len() as u32);
    for (k, (report, object)) in f.reports.iter().zip(&f.objects).enumerate() {
        let k = k as u32;
        let (at, pair, a, b, obj) =
            (n.reports + 96 * k, n.pairs + 112 * k, n.bodies + 288 * k, n.bodies + 288 * k + 144, n.objects + 96 * k);
        o.set_u32(at, pair);
        o.set_v4(at + 16, report.normal);
        o.set_v4(at + 32, report.point);
        o.set_u32(at + 64, report.part as u32);
        o.set_u32(at + 68, report.tag);
        o.set_u32(at + 76, if *object { obj } else { 0 });
        o.set_u8(at + 81, u8::from(report.side_a));
        o.set_v4(pair + 80, report.solved_vector);
        o.set_u32(pair + 96, a);
        o.set_u32(pair + 100, b);
        for (body, base) in [(report.body_a, a), (report.body_b, b)] {
            o.set_v4(base + 32, body.linear_velocity);
            o.set_f32(base + 124, body.inverse_mass);
            o.set_u32(base + 140, body.state_flags);
        }
        // [[obj+24]+24] is the group; [obj+32] the entity, whose [vtable+44] returns its id.
        o.set_u32(obj + 24, obj + 48);
        o.set_u32(obj + 72, report.other_group);
        o.set_u32(obj + 32, if report.other_entity.is_some() { obj + 80 } else { 0 });
        o.set_u32(obj + 80, n.vtable);
        o.set_u32(obj + 84, report.other_entity.unwrap_or(0) as u32);
    }
}

fn compare(n: &Native, port: &SkeletonCollisionFeedback, step: usize) {
    let (o, t) = (&n.o, n.this);
    let count = o.u32(t + 4084) as usize;
    assert_eq!(count, port.planes.len(), "step {step} plane count");
    for (i, plane) in port.planes.iter().enumerate() {
        let at = t + 32 * i as u32;
        check(&format!("plane {i} normal"), step, &o.v4(at), &plane.normal);
        assert_eq!(o.u32(at + 16), plane.part as u32, "step {step} plane {i} part");
    }
    for p in 0..24 {
        let at = t + 4 * p as u32;
        check(&format!("contact_age {p}"), step, &[o.f32(at + 640)], &[port.contact_age[p]]);
        assert_eq!(o.u8(t + 832 + p as u32) != 0, port.current[p], "step {step} current {p}");
        let (b, bone) = (t + 1232 + 112 * p as u32, &port.bones[p]);
        for (name, off, v) in [
            ("normal", 0, bone.normal),
            ("specific_normal", 16, bone.specific_normal),
            ("tangent", 32, bone.tangent),
            ("specific_tangent", 48, bone.specific_tangent),
            ("point", 64, bone.point),
        ] {
            check(&format!("bone {p} {name}"), step, &o.v4(b + off), &v);
        }
        check(&format!("bone {p} forces"), step, &[o.f32(b + 80), o.f32(b + 84)], &[bone.force, bone.specific_force]);
        assert_eq!([o.u32(b + 88), o.u32(b + 92)], [bone.tag, bone.specific_tag], "step {step} bone {p} tags");
        let groups: [bool; 4] = std::array::from_fn(|g| o.u8(b + 96 + g as u32) != 0);
        assert_eq!(groups, bone.groups, "step {step} bone {p} groups");
    }
    for (i, region) in port.regions.iter().enumerate() {
        let at = t + 4 * i as u32;
        check(
            &format!("region {i} scalars"),
            step,
            &[o.f32(at + 880), o.f32(at + 912), o.f32(at + 944)],
            &[region.force, region.weighted_force, region.tangent_speed],
        );
        assert_eq!(o.u32(at + 976), region.material_flags, "step {step} region {i} material");
        check(&format!("region {i} normal"), step, &o.v4(t + 1008 + 16 * i as u32), &region.normal);
        let part = o.u32(at + 1200) as i32;
        assert_eq!(part, region.part.map_or(-1, |p| p as i32), "step {step} region {i} part");
    }
    for (name, off, v) in [
        ("highest_normal", 1136, port.highest_normal),
        ("foot_normal", 1152, port.foot_normal),
        ("material_10_normal", 1168, port.material_normals[0]),
        ("material_11_normal", 1184, port.material_normals[1]),
    ] {
        check(name, step, &o.v4(t + off), &v);
    }
    for (i, point) in port.specific.iter().enumerate() {
        let i32_ = i as u32;
        check(&format!("specific {i} world"), step, &o.v4(t + 3936 + 16 * i32_), &point.world_point);
        assert_eq!(
            [o.u8(t + 4008 + i32_) != 0, o.u8(t + 4010 + i32_) != 0],
            [point.current, point.recent],
            "step {step} specific {i} current/recent"
        );
    }
    let scalars = [4024, 4028, 4032, 4036, 4040, 4044, 4048, 4056, 4060, 4064].map(|off| o.f32(t + off));
    let w = port.wipeout_times;
    check(
        "timer, drive_weight, max priority, wipeout times, skater/group 8/group 11 forces, material 12 height",
        step,
        &scalars,
        &[
            port.timer,
            port.drive_weight,
            port.maximum_priority,
            w[0],
            w[1],
            w[2],
            port.maximum_skater_force,
            port.maximum_group_8_force,
            port.maximum_group_11_force,
            port.material_12_height,
        ],
    );
    assert_eq!(o.u32(t + 4052) as i32, port.other_skater, "step {step} other skater");
    let f = port.flags;
    let flags = [
        f.material_6,
        f.noncompliant,
        f.compliant,
        f.recovering,
        f.group_8,
        f.nonboard,
        f.ragdoll,
        f.conflicting,
        f.impaled,
        f.has_impulse,
        f.foot_board,
        f.material_10,
        f.material_11,
        f.material_12,
        f.any,
    ];
    let native: [bool; 15] = std::array::from_fn(|k| o.u8(t + 4068 + k as u32) != 0);
    assert_eq!(native, flags, "step {step} flags 4068..4082");
}

#[test]
#[ignore = "needs the installed TU3 image: SKATE3_ASSET_ROOT"]
fn wipeout_oracle_collision_feedback_matches_82bd4a30() {
    let mut n = Native::new();
    let mut r = Rng(0x82BD_4A30_5EED_0001);
    let (mut step, mut seen) = (0, [0usize; 9]);
    let mut resets = 0;
    for _ in 0..8000 {
        let mut port = episode(&mut n, &mut r);
        for _ in 0..10 {
            let f = frame_inputs(&mut r, &port);
            write_frame(&mut n, &f);
            let [ragdoll, disable_ground_filter, ai_collision_scalar, offboard, entering_offboard, category_600, request_partial_ragdoll] =
                f.flags;
            let input = SkeletonCollisionInput {
                dt: f.dt,
                plane_point: f.plane_point,
                plane_normal: f.plane_normal,
                reference_velocity: f.reference_velocity,
                com_velocity: f.com_velocity,
                ragdoll,
                disable_ground_filter,
                ai_collision_scalar,
                offboard,
                entering_offboard,
                category_600,
                request_partial_ragdoll,
                physical: &f.physical,
                part_weights: &f.weights,
                body_frames: &f.frames,
            };
            port.update(&input, &f.reports);
            let (parts, entity_fn, frames) = (n.parts, n.entity_fn, &f.frames);
            n.o.call_hooked(UPDATE, &[n.this, n.input], &[GET_PART_TRANSFORM, entity_fn], &mut |cpu, g, target| {
                if target == GET_PART_TRANSFORM {
                    let (out, part) = (cpu.r[3] as u32, ((cpu.r[4] as u32 - parts) / 96) as usize);
                    for (k, row) in frames[part].iter().enumerate() {
                        for (i, x) in row.iter().enumerate() {
                            g.set_u32(out + 16 * k as u32 + 4 * i as u32, x.to_bits()).unwrap();
                        }
                    }
                } else {
                    cpu.r[3] = u64::from(g.u32(cpu.r[3] as u32 + 4).unwrap());
                }
            });
            compare(&n, &port, step);
            if r.chance(0.1) {
                // Reset 82BD5DA0 keeps compliant, the specific points and `any` (4082).
                port.reset();
                n.o.call(0x82BD_5DA0, &[n.this]);
                compare(&n, &port, step);
                resets += 1;
                step += 1;
                continue;
            }
            let fl = port.flags;
            for (k, hit) in [
                fl.conflicting,
                fl.impaled,
                fl.material_10 || fl.material_11,
                port.specific.iter().any(|s| s.current),
                port.planes.len() == 20,
                fl.foot_board,
                port.other_skater != -1,
                fl.recovering,
                port.regions.iter().any(|g| g.part.is_some()),
            ]
            .into_iter()
            .enumerate()
            {
                seen[k] += usize::from(hit);
            }
            step += 1;
        }
    }
    assert!(resets > 1000);
    eprintln!("82BD4A30 {step} frames, {resets} resets; conflicting/impaled/material/specific/20 planes/foot board/skater/recovering/region {seen:?}");
    assert!(seen.iter().all(|&c| c > 50), "coverage {seen:?}");
}
