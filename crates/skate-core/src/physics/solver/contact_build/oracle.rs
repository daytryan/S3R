//! TU3 oracle for ContactBatchBuild `82AE10C8`: the restitution target (row 6) that decides
//! whether a contact bounces or sticks, plus every other compiled word.
use crate::physics::tu3_oracle::{self, Rng};

/// A unit vector, now and then an axis or degenerate.
fn unit(r: &mut Rng) -> [f32; 3] {
    match (r.unit() * 10.0) as u32 {
        0 => [0.0, 1.0, 0.0],
        1 => [0.0; 3],
        _ => {
            let v = [r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0)];
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-3);
            v.map(|x| x / n)
        }
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// An uncompiled contact laid out as the native producer copies it (`input.rs`).
fn record(r: &mut Rng) -> [u32; 64] {
    let mut w = [0u32; 64];
    let row = |w: &mut [u32; 64], k: usize, v: [f32; 3], carry: u32| {
        w[k * 4..k * 4 + 3].copy_from_slice(&v.map(f32::to_bits));
        w[k * 4 + 3] = carry;
    };
    let small = |r: &mut Rng, s: f32| [r.range(-s, s), r.range(-s, s), r.range(-s, s)];
    let centers = [small(r, 3.0), small(r, 3.0)];
    let on_a = std::array::from_fn::<f32, 3, _>(|i| centers[0][i] + r.range(-0.5, 0.5));
    // Separated, touching or penetrating: B's point along the normal by `depth`.
    let normal = unit(r);
    let depth = match (r.unit() * 6.0) as u32 {
        0 => 0.0,
        1 => -0.0,
        _ => r.edgy(-0.05, 0.05),
    };
    let on_b = std::array::from_fn::<f32, 3, _>(|i| on_a[i] + normal[i] * depth);
    let t0 = unit(r);
    let t1 = cross(normal, t0);
    let id = |r: &mut Rng| (r.unit() * 65536.0) as u32;
    row(&mut w, 0, on_a, id(r));
    row(&mut w, 1, on_b, id(r));
    let restitution = match (r.unit() * 12.0) as u32 {
        0 => f32::NAN,
        1 => 1.0,
        _ => r.edgy(0.0, 1.0),
    };
    row(&mut w, 2, normal, restitution.to_bits());
    row(&mut w, 3, t0, r.edgy(0.0, 1.5).to_bits());
    row(&mut w, 4, t1, r.edgy(0.0, 1.5).to_bits());
    // Approaching, receding, resting: the target's three branches.
    let speed = if r.chance(0.1) { [0.0; 3] } else { small(r, 8.0) };
    row(&mut w, 5, speed, id(r));
    for body in 0..2 {
        let state = [0, 4, 8, 12, 0x14, 0xFF][(r.unit() * 6.0) as usize];
        let mass = if r.chance(0.1) { 0.0 } else { r.range(0.01, 2.0) };
        row(&mut w, 6 + body, centers[body], id(r));
        row(&mut w, 8 + body, [r.range(0.0, 3.0), r.range(-0.2, 0.2), r.range(-0.2, 0.2)], mass.to_bits());
        row(&mut w, 10 + body, [r.range(0.0, 3.0), r.range(0.0, 3.0), r.range(-0.2, 0.2)], state);
        row(&mut w, 12 + body, small(r, 20.0), r.range(0.0, 5.0).to_bits());
        row(&mut w, 14 + body, small(r, 20.0), id(r));
    }
    w
}

/// 82AE10C8 over a batch (`ctx+16` records of 256 bytes, `ctx+100` count, `ctx+160` time step)
/// against `contact_build::build` on each record, all 64 words.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn contact_build_oracle_matches_82ae10c8() {
    let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
    o.port_vmx();
    const BATCH: u32 = 16;
    let ctx = o.alloc(256);
    let records = o.alloc(256 * BATCH);
    let mut r = Rng(0x5EED_0025_10C8_0014);
    // 82AE1590..15F4's normal adjustment: none (predicted <= 0), restitution (separated),
    // separation + restitution (penetrating and approaching), none (penetrating, receding).
    let mut branches = [0usize; 4];
    for round in 0..2_000 {
        let dt = match (r.unit() * 8.0) as u32 {
            0 => 1.0 / 30.0,
            1 => r.range(0.001, 0.05),
            _ => f32::from_bits(0x3C88_8889),
        };
        let inputs: Vec<[u32; 64]> = (0..BATCH).map(|_| record(&mut r)).collect();
        for (i, words) in inputs.iter().enumerate() {
            for (k, word) in words.iter().enumerate() {
                o.set_u32(records + 256 * i as u32 + 4 * k as u32, *word);
            }
        }
        o.set_u32(ctx + 16, records);
        o.set_u32(ctx + 100, BATCH);
        o.set_f32(ctx + 160, dt);
        o.call(0x82AE_10C8, &[ctx]);
        for (i, input) in inputs.iter().enumerate() {
            let step = round * BATCH as usize + i;
            let mut port = *input;
            super::build(&mut port, dt);
            for k in 0..64 {
                let retail = o.u32(records + 256 * i as u32 + 4 * k as u32);
                assert!(
                    retail == port[k] || f32::from_bits(retail).is_nan() && f32::from_bits(port[k]).is_nan(),
                    "step {step} word {k} (row {} lane {}): retail {retail:08X} port {:08X}",
                    k / 4,
                    k % 4,
                    port[k]
                );
            }
            let p = super::prepare_contact(input, dt);
            let inverse = p.effective_mass[0].recip();
            let separation = p.separation_projection[0] * inverse;
            branches[if !(p.predicted_separation_projection[0] * inverse > 0.0) {
                0
            } else if separation >= 0.0 {
                1
            } else if p.restitution_projection[0] * inverse < 0.0 {
                2
            } else {
                3
            }] += 1;
        }
    }
    assert!(branches.iter().all(|&n| n > 500), "branches {branches:?}");
}

fn same_word(retail: u32, port: u32) -> bool {
    retail == port || f32::from_bits(retail).is_nan() && f32::from_bits(port).is_nan()
}

/// HorusPipeline 82AE27D0 (r3/r4 contacts, r5/r6 joints, r7/r8 drives, r9 iterations): the
/// contact loop 82AE2914..2B8C with its normal/friction clamps, the joint loop 82AE2BC8..2E44
/// and the drive loop 82AE2E78..30D8, all pushing into the shared reactions (`+0` linear, `+16`
/// position, `+32` angular, `+48` orientation; each record's reaction pointers at +12/+28).
/// Joint and drive records are random finite words: every lane the kernels read is exercised.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn constraint_solve_oracle_matches_82ae27d0() {
    use crate::physics::solver::packed;
    let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
    o.port_vmx();
    const MAX: usize = 10;
    const REACTIONS: usize = 6;
    let contacts_at = o.alloc(256 * MAX as u32);
    let joints_at = o.alloc(384 * MAX as u32);
    let drives_at = o.alloc(384 * MAX as u32);
    let reactions_at = o.alloc(64 * REACTIONS as u32);
    let mut r = Rng(0x5EED_0025_27D0_0015);
    let mut clamped = 0;
    let pair = |r: &mut Rng| {
        let a = (r.unit() * REACTIONS as f32) as usize;
        (a, (a + 1 + (r.unit() * (REACTIONS - 1) as f32) as usize) % REACTIONS)
    };
    let pointer = |k: usize| reactions_at + 64 * k as u32;
    for round in 0..2_000 {
        let dt = f32::from_bits(0x3C88_8889);
        // The caller clears the reaction workspace each step, so most passes start from zeros.
        let mut reactions: Vec<packed::Reaction> = (0..REACTIONS)
            .map(|_| {
                let cleared = r.chance(0.4);
                std::array::from_fn(|k| {
                    let v = if cleared || k % 4 == 3 && r.chance(0.8) { 0.0 } else { r.range(-2.0, 2.0) };
                    v.to_bits()
                })
            })
            .collect();
        let count = |r: &mut Rng| (r.unit() * (MAX + 1) as f32) as usize;
        let mut contacts: Vec<packed::Contact> = (0..count(&mut r))
            .map(|_| {
                let (a, b) = pair(&mut r);
                let mut words = record(&mut r);
                words[27] = pointer(a);
                words[31] = pointer(b);
                super::build(&mut words, dt);
                if r.chance(0.5) {
                    // A warm accumulated impulse (row 5), as after earlier iterations.
                    for k in 20..24 {
                        words[k] = r.range(-1.0, 3.0).to_bits();
                    }
                }
                packed::Contact { words, reaction_a: a, reaction_b: b }
            })
            .collect();
        let random_rows = |r: &mut Rng| -> Vec<packed::Constraint<96>> {
            (0..count(r))
                .map(|_| {
                    let (a, b) = pair(r);
                    let mut words: [u32; 96] = std::array::from_fn(|_| {
                        if r.chance(0.1) { 0.0f32.to_bits() } else { r.range(-1.0, 1.0).to_bits() }
                    });
                    words[3] = pointer(a);
                    words[7] = pointer(b);
                    packed::Constraint { words, reaction_a: a, reaction_b: b }
                })
                .collect()
        };
        let mut joints = random_rows(&mut r);
        let mut drives = random_rows(&mut r);
        let put = |o: &mut tu3_oracle::Tu3, at: u32, stride: u32, rows: &mut dyn Iterator<Item = &[u32]>| {
            for (i, words) in rows.enumerate() {
                for (k, w) in words.iter().enumerate() {
                    o.set_u32(at + stride * i as u32 + 4 * k as u32, *w);
                }
            }
        };
        put(&mut o, contacts_at, 256, &mut contacts.iter().map(|c| &c.words[..]));
        put(&mut o, joints_at, 384, &mut joints.iter().map(|c| &c.words[..]));
        put(&mut o, drives_at, 384, &mut drives.iter().map(|c| &c.words[..]));
        put(&mut o, reactions_at, 64, &mut reactions.iter().map(|w| &w[..]));
        let iterations = 1 + (r.unit() * 8.0) as u32;
        let n = [contacts.len(), joints.len(), drives.len()].map(|n| n as u32);
        o.call(0x82AE_27D0, &[n[0], contacts_at, n[1], joints_at, n[2], drives_at, iterations]);
        let before: Vec<[u32; 64]> = contacts.iter().map(|c| c.words).collect();
        packed::solve(&mut contacts, &mut joints, &mut drives, &mut reactions, iterations);
        let compare = |what: &str, at: u32, stride: u32, rows: &mut dyn Iterator<Item = &[u32]>| {
            for (i, words) in rows.enumerate() {
                for (k, w) in words.iter().enumerate() {
                    let retail = o.u32(at + stride * i as u32 + 4 * k as u32);
                    assert!(same_word(retail, *w), "round {round} {what} {i} word {k}: retail {retail:08X} port {w:08X}");
                }
            }
        };
        compare("contact", contacts_at, 256, &mut contacts.iter().map(|c| &c.words[..]));
        compare("joint", joints_at, 384, &mut joints.iter().map(|c| &c.words[..]));
        compare("drive", drives_at, 384, &mut drives.iter().map(|c| &c.words[..]));
        compare("reaction", reactions_at, 64, &mut reactions.iter().map(|w| &w[..]));
        for (c, before) in contacts.iter().zip(&before) {
            // A friction lane pinned at its limit: the clamp ran.
            let limit = f32::from_bits(c.words[15]) * f32::from_bits(c.words[20]);
            clamped += usize::from(f32::from_bits(c.words[21]).abs() == limit.abs() && before[21] != c.words[21]);
        }
    }
    assert!(clamped > 500, "{clamped} friction lanes at a limit");
}

/// CombinePhysicalMaterials `82763078` (out r3, a r4, b r5; +0 static, +4 dynamic, +8 restitution):
/// the friction and restitution each contact compiles from, ties and NaN included.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn contact_material_oracle_matches_82763078() {
    use crate::physics::contact::{RetailContactMaterial, combine_contact_materials};
    let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
    let (out, a_at, b_at) = (o.alloc(16), o.alloc(16), o.alloc(16));
    let mut r = Rng(0x5EED_0025_3078_0017);
    let pool = [0.0, -0.0, 0.3, 0.5, 1.0, f32::NAN];
    for step in 0..20_000 {
        let field = |r: &mut Rng| if r.chance(0.5) { pool[(r.unit() * 6.0) as usize] } else { r.range(-0.5, 1.5) };
        let material = |r: &mut Rng| RetailContactMaterial {
            static_friction: field(r),
            dynamic_friction: field(r),
            restitution: field(r),
        };
        let (a, b) = (material(&mut r), material(&mut r));
        for (at, m) in [(a_at, a), (b_at, b)] {
            o.set_f32(at, m.static_friction);
            o.set_f32(at + 4, m.dynamic_friction);
            o.set_f32(at + 8, m.restitution);
        }
        o.call(0x8276_3078, &[out, a_at, b_at]);
        let port = combine_contact_materials(a, b);
        tu3_oracle::check(
            "material",
            step,
            &[o.f32(out), o.f32(out + 4), o.f32(out + 8)],
            &[port.static_friction, port.dynamic_friction, port.restitution],
        );
    }
}
