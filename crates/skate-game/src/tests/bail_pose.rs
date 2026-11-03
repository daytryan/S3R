//! Bug 10-26 (user): lying on the ground after a bail, some body parts and clothes vanish, and they are back once the
//! skater stands up. A manual bail on DownTown's package spawn, to the get-up: every tick, each bone of the published
//! pose (what the skin draws) stays a finite rigid matrix (a NaN or squashed bone takes its triangles off screen), and
//! no skeleton body nor drawn joint lies under the floor. Both held on 10-26 (water culling on and off), so neither is
//! that bug. Part 0 / bone 0 (TRAJECTORY) is left out: the root's collision is off in a ragdoll (collision_mode
//! disable_root) and it sits ~0.6 m under the floor while the body lies; no skin vertex is weighted to it (10-26 scan of
//! every installed skater GLB). Install assets, read-only.
use super::mode_harness::{Harness, Spawn, pads};
use bevy::math::{Mat4, Vec3};

/// Something under the floor at its deepest: (tick, state, which, its centre, the floor over or under it, how many of
/// its kind were under the floor then).
#[derive(Debug)]
#[allow(dead_code)] // read through Debug, in the report
struct Sunk {
    tick: u64,
    state: String,
    which: usize,
    centre: [f32; 3],
    floor: f32,
    under: usize,
}
impl Sunk {
    fn depth(&self) -> f32 {
        self.floor - self.centre[1]
    }
}

/// The bail (`water`: today's water culling, or the code before it): the bones gone off in it, its wipeout ticks, the
/// floor under the deck before it, the deepest skeleton body and drawn joint, and the deepest body each second.
fn bail(water: bool) -> (Vec<String>, u32, Option<f32>, Option<Sunk>, Option<Sunk>, Vec<(u64, f32)>) {
    let mut h = Harness::load("DownTown", Spawn::Package);
    h.physics.water = water;
    h.run(150, |_| pads::idle());
    let names = h.skater.animation.evaluator.frames.bone_names.clone();
    let off = |m: Mat4| {
        let (scale, _, _) = m.to_scale_rotation_translation();
        !m.is_finite() || (scale - Vec3::ONE).abs().max_element() > 0.05 || m.determinant() <= 0.
    };
    // As the rig has it before the bail (a bone authored off is not this bug).
    let riding: Vec<bool> = h.skater.render_pose.iter().map(|&m| off(crate::animation::native_matrix(m))).collect();
    let spawn_floor = h.ground(h.sample().deck);
    let (mut broken, mut wipeout, mut body, mut joint, mut track) = (Vec::new(), 0, None::<Sunk>, None::<Sunk>, Vec::new());
    // The deepest of `points` (index, centre) under the floor, if deeper than `worst`.
    let deepest = |h: &Harness, t: u64, state: &str, points: Vec<(usize, [f32; 3])>, worst: &mut Option<Sunk>| {
        let found: Vec<Sunk> = points
            .into_iter()
            .filter_map(|(which, centre)| Some(Sunk { tick: t, state: state.to_owned(), which, centre, floor: h.ground(centre)?, under: 0 }))
            .collect();
        let under = found.iter().filter(|s| s.depth() > 0.).count();
        let mut best = found.into_iter().max_by(|a, b| a.depth().total_cmp(&b.depth()))?;
        best.under = under;
        let depth = best.depth();
        if worst.as_ref().is_none_or(|w| depth > w.depth()) {
            *worst = Some(best);
        }
        Some(depth)
    };
    for t in 0..900u64 {
        let s = h.tick(if t < 30 { pads::bail() } else { pads::idle() });
        let state = format!("{:?} cat {}", s.state, s.category);
        let root = crate::animation::native_matrix(h.skater.animated_skeleton.roots.animation_to_world);
        let mut joints = Vec::new();
        for (i, &m) in h.skater.render_pose.iter().enumerate() {
            let m = crate::animation::native_matrix(m);
            if (!m.is_finite() || off(m) && !riding.get(i).copied().unwrap_or(false)) && broken.len() < 40 {
                let (scale, _, translation) = m.to_scale_rotation_translation();
                let name = names.get(i).map_or("?", String::as_str);
                broken.push(format!("t{t} {state} bone {i} {name}: scale {scale:.3?} at {translation:.2?}"));
            }
            if i > 0 {
                joints.push((i, (root * m).w_axis.truncate().to_array()));
            }
        }
        if s.bailing() {
            wipeout += 1;
            let bodies = h.skater.skeleton.bodies().iter().enumerate().skip(1).map(|(k, b)| (k, [b.rates.position.x, b.rates.position.y, b.rates.position.z]));
            let depth = deepest(&h, t, &state, bodies.collect(), &mut body);
            deepest(&h, t, &state, joints, &mut joint);
            if t % 60 == 0 {
                track.extend(depth.map(|d| (t, d)));
            }
        }
    }
    (broken, wipeout, spawn_floor, body, joint, track)
}

/// The spot's matrix turned `quarter` quarter turns about the vertical.
fn turned(mut at: super::mode_harness::Matrix, quarter: u32) -> super::mode_harness::Matrix {
    let (sin, cos) = (quarter as f32 * std::f32::consts::FRAC_PI_2).sin_cos();
    for row in &mut at[..3] {
        let (x, z) = (row[0], row[2]);
        (row[0], row[2]) = (x * cos + z * sin, z * cos - x * sin);
    }
    at
}

/// Bug 10-26, the user's screenshot: lying in a bowl after a bail, the head, hair and hat (the parts skinned to HEAD:
/// tools' skin scan of the user's GLBs) were gone while the shirt, pants, arms and hands drew. Manual bails at speed in
/// DownTown's and Industrial's bowls, four headings and three run-ups each: every tick of the wipeout, each bone stays a
/// finite rigid matrix as far from its parent as when riding (the root's children aside: TRAJECTORY sits under the
/// floor in a ragdoll), and HEAD stays on NECK1's side of the world. Install assets, read-only.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn bails_in_bowls_keep_the_head_on() {
    use skate_core::math::Vector3;
    let teleports = super::mode_harness::private_json("teleports.json");
    let spot = |id: &str| {
        let all = teleports["destinations"].as_array().unwrap();
        super::mode_harness::matrix(&all.iter().find(|d| d["id"] == id).unwrap_or_else(|| panic!("{id}"))["matrix"])
    };
    let (mut faults, mut runs, mut wipeout) = (Vec::new(), 0, 0u64);
    for (map, ids) in [
        ("DownTown", &["dist_downtown_skatepark"][..]),
        ("Industrial", &["dist_industrial_reclaimed", "dist_industrial_spillwaybottom"][..]),
    ] {
        let mut h = Harness::load(map, Spawn::Package);
        let names = h.skater.animation.evaluator.frames.bone_names.clone();
        let parents: Vec<i64> = h.skater.animation.evaluator.frames.parents.iter().map(|&p| p as i64).collect();
        let bone = |name: &str| names.iter().position(|n| n.eq_ignore_ascii_case(name)).unwrap();
        let (head, neck) = (bone("HEAD"), bone("NECK1"));
        // Every bone's world position (the render's PlayerRoot times its pose).
        let world = |h: &Harness| {
            let root = crate::animation::native_matrix(h.skater.animated_skeleton.roots.animation_to_world);
            h.skater.render_pose.iter().map(|&m| (root * crate::animation::native_matrix(m)).w_axis.truncate()).collect::<Vec<Vec3>>()
        };
        // A bone whose parent is a bone (TRAJECTORY's children aside), not a helper placed on a hand or toe.
        let checked = |i: usize| parents[i] > 0 && !names[i].ends_with("_REPARENTED");
        let off = |m: Mat4| {
            let (scale, _, _) = m.to_scale_rotation_translation();
            !m.is_finite() || (scale - Vec3::ONE).abs().max_element() > 0.05 || m.determinant() <= 0.
        };
        for &id in ids {
            for quarter in 0..4 {
                for push in [45u64, 90, 150] {
                    h.teleport(turned(spot(id), quarter));
                    h.run(30, |_| pads::idle());
                    let riding = world(&h);
                    let rest: Vec<f32> = (0..riding.len()).map(|i| if checked(i) { riding[i].distance(riding[parents[i] as usize]) } else { 0. }).collect();
                    let authored_off: Vec<bool> = h.skater.render_pose.iter().map(|&m| off(crate::animation::native_matrix(m))).collect();
                    h.run(push, pads::push);
                    runs += 1;
                    for t in 0..700u64 {
                        let s = h.tick(if t < 30 { pads::bail() } else { pads::idle() });
                        if !s.bailing() {
                            continue;
                        }
                        wipeout += 1;
                        let at = world(&h);
                        let mut fault = |what: String| {
                            if faults.len() < 60 {
                                faults.push(format!("{id} turn {quarter} push {push} t{t} {:?}: {what}", s.state));
                            }
                        };
                        for (i, &m) in h.skater.render_pose.iter().enumerate() {
                            let m = crate::animation::native_matrix(m);
                            if !m.is_finite() || off(m) && !authored_off.get(i).copied().unwrap_or(false) {
                                let (scale, _, _) = m.to_scale_rotation_translation();
                                fault(format!("bone {i} {} not rigid: scale {scale:.3?}", names[i]));
                            }
                            if checked(i) && (at[i].distance(at[parents[i] as usize]) - rest[i]).abs() > 0.05 {
                                let gap = at[i].distance(at[parents[i] as usize]);
                                fault(format!("bone {i} {} {gap:.3} m from its parent (riding {:.3})", names[i], rest[i]));
                            }
                        }
                        let (from, to) = (at[neck], at[head]);
                        if let Ok(Some(hit)) = h.physics.world().query_thin_line(Vector3::new(from.x, from.y, from.z), Vector3::new(to.x, to.y, to.z)) {
                            let p = hit.geometry.position;
                            fault(format!("HEAD {:.3} m behind a face at {:?} from NECK1", Vec3::new(p.x, p.y, p.z).distance(to), [p.x, p.y, p.z]));
                        }
                    }
                }
            }
        }
    }
    eprintln!("BOWL BAILS: {runs} runs, {wipeout} wipeout ticks, {} faults:\n{}", faults.len(), faults.join("\n"));
    assert!(wipeout > 1000, "too few wipeout ticks: {wipeout}");
    assert!(faults.is_empty(), "{} faults", faults.len());
}

#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
fn a_bailed_skater_lying_on_the_ground_keeps_every_bone() {
    let runs: Vec<_> = [true, false].map(|water| (water, bail(water))).into_iter().collect();
    for (water, (broken, wipeout, spawn_floor, body, joint, track)) in &runs {
        eprintln!("BAIL POSE water {water}: {wipeout} wipeout ticks; the floor under the deck before it {spawn_floor:?}");
        eprintln!("  deepest body {body:?}");
        eprintln!("  deepest drawn joint {joint:?}");
        eprintln!("  deepest body each second (tick, m under the floor): {track:.3?}");
        eprintln!("  bones gone off: {}", broken.len());
    }
    let (_, (broken, wipeout, _, body, joint, _)) = &runs[0];
    assert!(*wipeout > 60, "never bailed");
    assert!(broken.is_empty(), "bones broken in the bail:\n{}", broken.join("\n"));
    for (what, sunk) in [("body", body), ("drawn joint", joint)] {
        assert!(sunk.as_ref().is_none_or(|s| s.depth() < 0.05), "a {what} under the floor: {sunk:?}");
    }
}
