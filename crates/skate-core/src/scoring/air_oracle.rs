//! `scoring::air` against the TU3 functions themselves, run in the PowerPC interpreter (see
//! `physics::tu3_oracle`). Run with
//! `SKATE3_ASSET_ROOT=<install>/assets cargo test -p skate-core air_oracle -- --ignored`.
use super::*;
use crate::physics::tu3_oracle::{self, Tu3};
use crate::point_graph::PointGraph;

struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
    fn v4(&mut self, lo: f32, hi: f32) -> V4 {
        std::array::from_fn(|_| self.range(lo, hi))
    }
    fn curve(&mut self, x_hi: f32) -> PointGraph<8> {
        let mut x = [0.0; 8];
        for i in 1..8 {
            x[i] = x[i - 1] + self.range(0.0, x_hi / 8.0);
        }
        PointGraph { x, y: std::array::from_fn(|_| self.range(0.0, 500.0)) }
    }
}

/// Rows right, up, forward of yaw/pitch/roll (inputs only, host trig is fine), the w lanes
/// and translation row filled with junk the functions must ignore.
fn rotation(yaw: f32, pitch: f32, roll: f32, r: &mut Rng) -> Matrix {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    let forward = [sy * cp, -sp, cy * cp];
    let up0 = [sy * sp, cp, cy * sp];
    let right0 = [cy, 0.0, -sy];
    let up = std::array::from_fn::<f32, 3, _>(|i| up0[i] * cr - right0[i] * sr);
    let right = std::array::from_fn::<f32, 3, _>(|i| right0[i] * cr + up0[i] * sr);
    let w = r.range(-2.0, 2.0);
    [
        [right[0], right[1], right[2], w],
        [up[0], up[1], up[2], w],
        [forward[0], forward[1], forward[2], w],
        r.v4(-50.0, 50.0),
    ]
}

fn junk(r: &mut Rng) -> Matrix {
    std::array::from_fn(|_| {
        if r.chance(0.1) { [0.0; 4] } else { r.v4(-3.0, 3.0) }
    })
}

fn load() -> Tu3 {
    let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
    assert_eq!(o.u32(0x82DA_C880), 0x7D88_02A6, "0x82DAC880 is not `mflr r12`: not the TU3 image");
    // 82F82690 (a static initialiser the oracle does not run): 0x830BD300 = splat(0x821647E0).
    for lane in 0..4 {
        o.set_u32(0x830B_D300 + 4 * lane, 0x3780_0000);
    }
    o
}

fn put_v4(o: &mut Tu3, at: u32, v: V4) {
    for (i, x) in v.iter().enumerate() {
        o.set_f32(at + 4 * i as u32, *x);
    }
}
fn put_matrix(o: &mut Tu3, at: u32, m: &Matrix) {
    for (i, row) in m.iter().enumerate() {
        put_v4(o, at + 16 * i as u32, *row);
    }
}
fn get_v4(o: &Tu3, at: u32) -> V4 {
    std::array::from_fn(|i| o.f32(at + 4 * i as u32))
}
fn get_matrix(o: &Tu3, at: u32) -> Matrix {
    std::array::from_fn(|i| get_v4(o, at + 16 * i as u32))
}
fn put_curve(o: &mut Tu3, at: u32, c: &PointGraph<8>) {
    for i in 0..8 {
        o.set_f32(at + 16 + 4 * i as u32, c.x[i]);
        o.set_f32(at + 48 + 4 * i as u32, c.y[i]);
    }
}

fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}
fn check(what: &str, step: usize, retail: &[f32], port: &[f32]) {
    for (k, (r, p)) in retail.iter().zip(port).enumerate() {
        assert!(
            same(*r, *p),
            "step {step} {what}[{k}]: retail {r:e} ({:08X}) port {p:e} ({:08X})",
            r.to_bits(),
            p.to_bits()
        );
    }
}
fn flat(m: &Matrix) -> Vec<f32> {
    m.iter().flatten().copied().collect()
}

/// A spin-like sequence: yaw steps up to ~0.9 rad either way with pitch and roll wobble, and
/// now and then a junk or degenerate frame.
fn next_frame(r: &mut Rng, angles: &mut [f32; 3]) -> Matrix {
    if r.chance(0.05) {
        return junk(r);
    }
    angles[0] += r.range(-0.9, 0.9);
    angles[1] = (angles[1] + r.range(-0.3, 0.3)).clamp(-1.6, 1.6);
    angles[2] += r.range(-0.3, 0.3);
    rotation(angles[0], angles[1], angles[2], r)
}

#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_rotation_accumulator_matches_82dac880() {
    let mut o = load();
    let acc = o.alloc(80);
    let current = o.alloc(64);
    let mut r = Rng(0x2545_F491_4F6C_DD1D);
    for sequence in 0..60 {
        let mut angles = [r.range(-3.0, 3.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0)];
        let start = next_frame(&mut r, &mut angles);
        let mut port = RotationAccumulator { reference: start, angle: r.range(-20.0, 20.0) };
        put_matrix(&mut o, acc, &start);
        o.set_f32(acc + 64, port.angle);
        o.set_u32(acc + 68, 0);
        for step in 0..120 {
            let m = next_frame(&mut r, &mut angles);
            put_matrix(&mut o, current, &m);
            o.call(0x82DA_C880, &[acc, current]);
            port.update(&m);
            let at = sequence * 1000 + step;
            check("angle", at, &[o.f32(acc + 64)], &[port.angle]);
            check("reference", at, &flat(&get_matrix(&o, acc)), &flat(&port.reference));
        }
    }
}

#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_frame_matches_82d2cdc0() {
    let mut o = load();
    let (out, up, forward) = (o.alloc(64), o.alloc(16), o.alloc(16));
    let mut r = Rng(0x0BAD_5EED_1234_5678);
    for step in 0..20_000 {
        let u = match step % 7 {
            0 => [0.0; 4],
            1 => r.v4(-1e-3, 1e-3),
            _ => r.v4(-2.0, 2.0),
        };
        let f = if step % 11 == 0 {
            u.map(|x| x * r.range(-2.0, 2.0) + r.range(-1e-5, 1e-5))
        } else {
            r.v4(-2.0, 2.0)
        };
        put_v4(&mut o, up, u);
        put_v4(&mut o, forward, f);
        put_matrix(&mut o, out, &[[f32::NAN; 4]; 4]);
        o.call(0x82D2_CDC0, &[out, up, forward]);
        check("frame", step, &flat(&get_matrix(&o, out)), &flat(&frame(u, f)));
    }
}

#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_context_run_matches_82da7b50() {
    let mut o = load();
    let (run, position) = (o.alloc(48), o.alloc(16));
    let mut r = Rng(0x1357_9BDF_2468_ACE0);
    for sequence in 0..100 {
        let mut port = ContextRun::default();
        for i in 0..48 {
            o.set_u8(run + i, 0);
        }
        let mut p = r.v4(-100.0, 100.0);
        for step in 0..200 {
            if r.chance(0.02) {
                p = [0.0; 4];
            } else {
                for k in 0..4 {
                    p[k] += r.range(-0.5, 0.5);
                }
            }
            let (active, bank) = (r.chance(0.6), r.chance(0.7));
            put_v4(&mut o, position, p);
            o.call(0x82DA_7B50, &[run, position, u32::from(active), u32::from(bank)]);
            port.advance(p, active, bank);
            let at = sequence * 1000 + step;
            let retail = [&get_v4(&o, run)[..], &[o.f32(run + 16), o.f32(run + 20), o.f32(run + 24)]].concat();
            let ported = [&port.start[..], &[port.current, port.banked, port.longest]].concat();
            check("run", at, &retail, &ported);
            assert_eq!(o.u32(run + 28), port.runs, "step {at} runs");
            assert_eq!(o.u32(run + 32) >> 24 != 0, port.running, "step {at} running");
        }
    }
}

#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_ground_query_matches_82da7a38() {
    let mut o = load();
    let (out, x, ground, reference) = (o.alloc(32), o.alloc(64), o.alloc(336), o.alloc(16));
    o.set_u32(x + 32, ground);
    let mut r = Rng(0xDEAD_BEEF_CAFE_F00D);
    for step in 0..20_000 {
        let origin = r.v4(-5.0, 5.0);
        let contact = if step % 13 == 0 { origin } else { r.v4(-5.0, 5.0) };
        let hit = r.chance(0.8);
        let surface = (r.unit() * 4096.0) as u32 | if r.chance(0.3) { 10 } else { 0 };
        let with_reference = r.chance(0.6);
        let reference_value = if step % 5 == 0 { [origin[0], origin[1], 0.0, 0.0] } else { r.v4(-5.0, 5.0) };
        put_v4(&mut o, ground + 192, origin);
        put_v4(&mut o, ground + 208, contact);
        o.set_u32(ground + 260, surface);
        o.set_u8(ground + 320, u8::from(hit));
        put_v4(&mut o, reference, reference_value);
        o.call(0x82DA_7A38, &[out, x, if with_reference { reference } else { 0 }]);
        let port = ground_query(
            origin,
            hit.then_some(contact),
            surface,
            with_reference.then_some(reference_value),
        );
        let retail = [&get_v4(&o, out)[..], &[o.f32(out + 16), o.f32(out + 20)]].concat();
        check("query", step, &retail, &[&port.origin[..], &[port.clearance, port.drop]].concat());
        let flags = o.u32(out + 24);
        assert_eq!(
            [flags >> 24 != 0, flags >> 16 & 0xFF != 0, flags >> 8 & 0xFF != 0],
            [port.hit, port.tag_10, port.class_8],
            "step {step} flags"
        );
    }
}

#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_contact_between_and_handplant_peak_match_82dac780_82dab7d0() {
    let mut o = load();
    let (this, x, ground, output, class) = (o.alloc(400), o.alloc(64), o.alloc(336), o.alloc(14656), o.alloc(512));
    o.set_u32(this + 4, x);
    o.set_u32(this + 20, class);
    o.set_u32(x + 32, ground);
    o.set_u32(x + 60, output);
    let mut r = Rng(0x0F0F_1E1E_2D2D_3C3C);
    for sequence in 0..50 {
        let curve = r.curve(3.0);
        put_curve(&mut o, class + 400, &curve);
        let mut peak = 0.0;
        o.set_f32(this + 320, peak);
        for step in 0..200 {
            let at = sequence * 1000 + step;
            let contact = r.v4(-3.0, 3.0);
            let near = |r: &mut Rng, s: f32| contact.map(|c| c + r.range(-s, s));
            let deck = if r.chance(0.2) { near(&mut r, 0.08) } else { r.v4(-3.0, 3.0) };
            let hips = if r.chance(0.2) { near(&mut r, 0.08) } else { r.v4(-3.0, 3.0) };
            let deck_hit = r.v4(-3.0, 3.0);
            let (hips_hit, deck_probe) = (r.chance(0.8), r.chance(0.7));
            put_v4(&mut o, ground + 144, deck);
            put_v4(&mut o, ground + 160, deck_hit);
            put_v4(&mut o, ground + 192, hips);
            put_v4(&mut o, ground + 208, contact);
            o.set_u8(ground + 319, u8::from(deck_probe));
            o.set_u8(ground + 320, u8::from(hips_hit));
            o.set_u8(output + 14648, 0);
            o.call(0x82DA_C780, &[this]);
            let between = contact_between(deck, hips, hips_hit.then_some(contact));
            assert_eq!(o.u32(output + 14648) >> 24 != 0, between, "step {at} contact_between");
            o.call(0x82DA_B7D0, &[this]);
            peak = handplant_peak(peak, hips_hit.then_some((hips, contact)), deck_probe.then_some((deck, deck_hit)));
            check("peak", at, &[o.f32(this + 320), o.f32(this + 324)], &[peak, curve.evaluate(peak)]);
        }
    }
}

/// The whole of 82DA8BE0: both accumulators, the latch, the banked degrees, the half turns
/// (`+2344`) and the spin metric (`+2368`).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_spin_tracker_matches_82da8be0() {
    let mut o = load();
    let this = o.alloc(2400);
    let (x, motion, skeleton, reckoning, class, holder) =
        (o.alloc(64), o.alloc(256), o.alloc(512), o.alloc(128), o.alloc(1664), o.alloc(7264));
    o.set_u32(this + 4, x);
    o.set_u32(this + 12, holder);
    o.set_u32(this + 20, class);
    o.set_u32(x, motion);
    o.set_u32(x + 20, skeleton);
    o.set_u32(x + 36, reckoning);
    o.set_u8(holder + 7261, 1);
    let mut r = Rng(0x7777_1234_ABCD_0001);
    for sequence in 0..40 {
        let curve = r.curve(1500.0);
        put_curve(&mut o, class + 880, &curve);
        let offset = r.range(0.0, 90.0);
        o.set_f32(class + 1596, offset);
        let (factor, repetition) = (r.range(0.0, 2.0), r.range(0.0, 2.0));
        o.set_f32(this + 2380, factor);
        o.set_f32(this + 2376, repetition);
        let (mut rider_angles, mut board_angles) = ([0.0, 0.0, 0.0], [r.range(-3.0, 3.0), 0.0, 0.0]);
        let rider = next_frame(&mut r, &mut rider_angles);
        let board = next_frame(&mut r, &mut board_angles);
        let mut port = SpinTracker::new(rider, board);
        put_matrix(&mut o, this + 816, &rider);
        put_matrix(&mut o, this + 896, &board);
        for (at, v) in [(880, 0.0), (884, 0.0), (960, 0.0), (964, 0.0), (2384, 0.0)] {
            o.set_f32(this + at, v);
        }
        o.set_u8(this + 2395, 1);
        let mut carried = false;
        for step in 0..150 {
            if r.chance(0.1) {
                carried = !carried;
            }
            o.set_u8(this + 708, u8::from(carried || r.chance(0.1)));
            o.set_u32(this + 596, if carried { (r.unit() * 331.0) as u32 } else { [u32::MAX, 332, 400][step % 3] });
            let carried_now = o.u32(this + 708) >> 24 != 0 && carried;
            let rider = next_frame(&mut r, &mut rider_angles);
            let board = next_frame(&mut r, &mut board_angles);
            let up = if r.chance(0.1) { [0.0; 4] } else { r.v4(-1.0, 1.0) };
            put_matrix(&mut o, skeleton + 368, &rider);
            put_matrix(&mut o, motion, &board);
            put_v4(&mut o, reckoning + 96, up);
            o.call(0x82DA_8BE0, &[this]);
            let degrees = port.update(carried_now, &rider, &board, up);
            let turns = half_turns(degrees, offset);
            let metric = ppc::fmuls(
                ppc::fmuls(curve.evaluate((turns.wrapping_mul(180) as f32).abs()), factor),
                repetition,
            );
            let at = sequence * 1000 + step;
            assert_eq!(o.u32(this + 2344) as i32, turns, "step {at} half turns ({degrees})");
            check("metric", at, &[o.f32(this + 2368)], &[metric]);
            check("banked", at, &[o.f32(this + 2384), o.f32(this + 880), o.f32(this + 960)], &[
                port.banked_degrees,
                port.rider.angle,
                port.board.angle,
            ]);
            assert_eq!(o.u32(this + 2392) & 0xFF != 0, port.board_phase, "step {at} latch");
            check("rider", at, &flat(&get_matrix(&o, this + 816)), &flat(&port.rider.reference));
            check("board", at, &flat(&get_matrix(&o, this + 896)), &flat(&port.board.reference));
        }
    }
}

#[test]
fn context_slots_insert_and_remove_like_82da14a0_82da1570() {
    let mut a = ContextBlock::default();
    for key in [5, 6, 5, 7, 8, 9, 10, 11, 12, 13] {
        a[0].insert(key, key * 2);
    }
    assert_eq!(a[0].count, 8);
    assert_eq!(a[0].entries[7], (12, 24));
    let mut b = ContextBlock::default();
    b[0].insert(6, 0);
    b[0].insert(99, 0);
    remove(&mut a, &b);
    assert_eq!(a[0].count, 7);
    assert_eq!(a[0].entries[1], (12, 24), "the last entry fills the gap");
}

#[test]
fn landing_context_applies_slots_in_retail_order_and_gates_slot_4_offline() {
    let scalar = |at: u16| match at {
        0x644 => 1.5,
        0x648 => 500.0,
        0x654 => 1.15,
        0x658 => 200.0,
        0x64C => 1.5,
        0x650 => 300.0,
        0x65C => 1.15,
        0x660 => 100.0,
        _ => unreachable!(),
    };
    let mut present = [false; 10];
    present[0] = true;
    present[4] = true;
    let (online, online_bonus) = landing_context(present, false, 1.0, 1.0, scalar);
    assert_eq!(online, 1.15);
    assert_eq!(online_bonus, vec![ppc::fmuls(200.0, 1.15)]);
    let (offline, offline_bonus) = landing_context(present, true, 1.0, 1.0, scalar);
    assert_eq!(offline, ppc::fmuls(1.5, 1.15));
    assert_eq!(offline_bonus.len(), 2);
}

fn object(id: u32, at: V4, min: V4, max: V4) -> ContextObject {
    let matrix = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], at];
    ContextObject { matrix, min, max, id, entity: 7 }
}

#[test]
fn context_objects_near_like_82761100() {
    // A 2 x 1 x 2 box: half its diagonal (1.5) plus 1 m, horizontally, from its centre.
    let unit = |id, x| object(id, [x, 0.0, 0.0, 1.0], [-1.0, 0.0, -1.0, 0.0], [1.0, 1.0, 1.0, 0.0]);
    let ids = |player: V4, objects: Vec<ContextObject>| near_objects(player, objects).map(|o| o.id).collect::<Vec<_>>();
    assert_eq!(ids([0.0, 40.0, 0.0, 1.0], vec![unit(1, 2.4), unit(2, 2.6), unit(3, -2.4)]), [1, 3], "height is ignored");
    // The 8 m broad phase measures the matrix translation, so a big box whose translation is
    // past 8 m is never tested, though its reach (16 m) covers the player.
    let big = object(4, [9.0, 0.0, 0.0, 1.0], [-15.0, 0.0, -15.0, 0.0], [15.0, 1.0, 15.0, 0.0]);
    assert!(ids([0.0; 4], vec![big]).is_empty());
    // At most 32 broad-phase candidates (82587F88), then at most 8 kept (82761100).
    let mut far: Vec<_> = (0..32).map(|i| unit(10 + i, 7.0)).collect();
    far.push(unit(99, 0.0));
    assert!(ids([0.0; 4], far).is_empty(), "the 33rd candidate is never looked at");
    assert_eq!(ids([0.0; 4], (0..12).map(|i| unit(i, 0.0)).collect()), (0..8u32).collect::<Vec<_>>());
}

#[test]
fn object_slot_like_82da6f58() {
    let crate_ = object(5, [10.0, 0.0, 0.0, 1.0], [-1.0, 0.0, -1.0, 0.0], [1.0, 1.0, 1.0, 0.0]);
    let slot = |probe| object_slot([crate_, crate_], probe);
    // The hips probe hit the crate's top: over it.
    let over = slot(Some(([10.0, 3.0, 0.0, 1.0], [10.0, 1.0, 0.0, 1.0])));
    assert_eq!((over.count, over.entries[0], over.entries[1]), (2, (5, 7), (5, 7)), "appended, not deduped");
    // The probe went through it to the ground (it is not solid), or missed it beside it.
    assert_eq!(slot(Some(([10.0, 3.0, 0.0, 1.0], [10.0, 0.0, 0.0, 1.0]))).count, 0);
    assert_eq!(slot(Some(([12.0, 3.0, 0.0, 1.0], [12.0, 1.0, 0.0, 1.0]))).count, 0);
    assert_eq!(slot(None).count, 0);
    // A turned box: the test runs in its own frame.
    let (s, c) = (0.6f32.sin(), 0.6f32.cos());
    let turned = ContextObject {
        matrix: [[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]],
        ..object(6, [0.0; 4], [-3.0, 0.0, -0.2, 0.0], [3.0, 1.0, 0.2, 0.0])
    };
    let along = [2.5 * c, 3.0, -2.5 * s, 1.0];
    assert_eq!(object_slot([turned], Some((along, [along[0], 1.0, along[2], 1.0]))).count, 1);
    assert_eq!(object_slot([turned], Some(([2.5, 3.0, 0.0, 1.0], [2.5, 1.0, 0.0, 1.0]))).count, 0);
}

#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_slab_matches_82da6cb0() {
    let mut o = load();
    let [origin, direction, min, max, near, far] = std::array::from_fn(|_| o.alloc(16));
    let mut r = Rng(0x51AB_6CB0_0000_0001);
    let mut hits = 0;
    for step in 0..20_000 {
        let lo = r.v4(-3.0, 1.0);
        let hi: V4 = std::array::from_fn(|i| lo[i] + r.range(0.0, 3.0));
        let from = r.v4(-5.0, 5.0);
        let mut d = r.v4(-8.0, 8.0);
        if r.chance(0.5) {
            // Aimed through a point of the box, and past it or short of it.
            let scale = r.range(0.3, 2.0);
            d = std::array::from_fn(|i| (lo[i] + (hi[i] - lo[i]) * r.unit() - from[i]) * scale);
        }
        for lane in 0..3 {
            if r.chance(0.15) {
                d[lane] = if r.chance(0.5) { 0.0 } else { r.range(-2e-6, 2e-6) };
            }
        }
        for (at, v) in [(origin, from), (direction, d), (min, lo), (max, hi)] {
            put_v4(&mut o, at, v);
        }
        o.set_f32(near, 0.0);
        o.set_f32(far, 1.0);
        o.call(0x82DA_6CB0, &[origin, direction, min, max, near, far]);
        let port = slab(from, d, lo, hi);
        assert_eq!(o.r3() & 0xFF != 0, port.is_some(), "step {step}");
        if let Some((n, f)) = port {
            check("t", step, &[o.f32(near), o.f32(far)], &[n, f]);
            hits += 1;
        }
    }
    assert!(hits > 2_000, "only {hits} hits");
}

/// 82DA6DB8 with the transposed rows 82DA6F58 builds for it (0x82DA7184..0x82DA71DC).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_probe_matches_82da6db8() {
    let mut o = load();
    let (origin, hit, entry, columns) = (o.alloc(16), o.alloc(16), o.alloc(112), o.alloc(64));
    let mut r = Rng(0x0BE8_6DB8_0000_0002);
    let mut over = 0;
    for step in 0..20_000 {
        let mut matrix = rotation(r.range(-3.0, 3.0), r.range(-0.4, 0.4), r.range(-0.4, 0.4), &mut r);
        matrix[3] = r.v4(-3.0, 3.0);
        let lo = r.v4(-2.0, 0.0);
        let object = ContextObject {
            matrix,
            min: lo,
            max: std::array::from_fn(|i| lo[i] + r.range(0.05, 3.0)),
            id: 0,
            entity: 0,
        };
        let mut start = [r.range(-3.0, 3.0), r.range(0.0, 5.0), r.range(-3.0, 3.0), r.range(-1.0, 1.0)];
        if r.chance(0.5) {
            // Over the box's origin, give or take.
            start[0] = matrix[3][0] + r.range(-1.0, 1.0);
            start[2] = matrix[3][2] + r.range(-1.0, 1.0);
            start[1] = matrix[3][1] + r.range(0.0, 4.0);
        }
        let ground = [start[0], start[1] - r.range(0.0, 8.0), start[2], r.range(-1.0, 1.0)];
        put_v4(&mut o, origin, start);
        put_v4(&mut o, hit, ground);
        put_matrix(&mut o, entry, &matrix);
        put_v4(&mut o, entry + 0x40, object.min);
        put_v4(&mut o, entry + 0x50, object.max);
        for k in 0..3 {
            put_v4(&mut o, columns + 16 * k as u32, [matrix[0][k], matrix[1][k], matrix[2][k], matrix[3][k]]);
        }
        put_v4(&mut o, columns + 48, [0.0; 4]);
        o.call(0x82DA_6DB8, &[origin, hit, entry, columns]);
        assert!(matches!(o.r3(), 1 | 2), "step {step}: {}", o.r3());
        assert_eq!(o.r3() == 1, over_object(start, ground, &object), "step {step}");
        over += usize::from(o.r3() == 1);
    }
    assert!(over > 1_000, "only {over} over");
}

/// 82761100 online: the broad phase 82587F88 runs over a fake DMO manager; the player's vfuncs
/// 0x5C / 0x64 / 0x68, the DMO world matrix 82585CB0 and the entry writer 82760ED0 are hooked.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_object_list_matches_82761100() {
    const OWNER: u32 = 0x9100_005C;
    const FLAGS: u32 = 0x9100_0064;
    const ID: u32 = 0x9100_0068;
    const WORLD: u32 = 0x8258_5CB0;
    const ENTRY: u32 = 0x8276_0ED0;
    const N: usize = 40;
    let mut o = load();
    let (input, player, vtable) = (o.alloc(0xB00), o.alloc(16), o.alloc(0x80));
    o.set_u32(player, vtable);
    for (at, f) in [(0x5C, OWNER), (0x64, FLAGS), (0x68, ID)] {
        o.set_u32(vtable + at, f);
    }
    let (owner, skeleton, state, grind, model) = (o.alloc(0x40), o.alloc(0x200), o.alloc(16), o.alloc(0x100), o.alloc(0x90));
    o.set_u32(owner + 0x10, grind);
    o.set_u32(owner + 0x14, skeleton);
    o.set_u32(owner + 0x1C, state);
    let (simulation, manager, records) = (o.alloc(0x110), o.alloc(0x8730), o.alloc(96 * N as u32));
    o.set_u32(0x830C_FD94, simulation);
    o.set_u32(simulation + 0x104, manager);
    o.set_u32(manager + 0x8720, records);
    let handles: Vec<u32> = (0..N as u32)
        .map(|k| {
            let (dmo, inner) = (o.alloc(0x20), o.alloc(0x30));
            o.set_u32(dmo + 0x18, inner);
            o.set_u32(inner + 8, k);
            o.set_u32(inner + 0x28, u32::MAX);
            for at in [0x2C, 0x30] {
                o.set_u32(records + 96 * k + at, dmo);
            }
            o.set_u32(records + 96 * k + 0x34, 1);
            dmo
        })
        .collect();
    let mut r = Rng(0x1100_8276_0000_0003);
    let mut kept = 0;
    for step in 0..3_000 {
        let n = (r.unit() * (N + 1) as f32) as usize % (N + 1);
        o.set_u32(manager + 0x8724, records + 96 * n as u32);
        let position = r.v4(-5.0, 5.0);
        put_v4(&mut o, skeleton + 0x1A0, position);
        let objects: Vec<ContextObject> = (0..n as u32)
            .map(|k| {
                let mut matrix = rotation(r.range(-3.0, 3.0), r.range(-0.3, 0.3), r.range(-0.3, 0.3), &mut r);
                matrix[3] = r.v4(-12.0, 12.0);
                let lo = r.v4(-3.0, 0.0);
                let size = if r.chance(0.2) { 12.0 } else { 3.0 };
                let max = std::array::from_fn(|i| lo[i] + r.range(0.1, size));
                ContextObject { matrix, min: lo, max, id: k, entity: 0 }
            })
            .collect();
        o.call_hooked(0x8276_1100, &[input, player, 1], &[OWNER, FLAGS, ID, WORLD, ENTRY], &mut |cpu, g, at| {
            let put = |g: &mut skate_audio_core::Guest, at: u32, v: V4| {
                for (i, x) in v.iter().enumerate() {
                    g.set_u32(at + 4 * i as u32, x.to_bits()).unwrap();
                }
            };
            let r3 = cpu.r[3] as u32;
            cpu.r[3] = match at {
                OWNER => u64::from(owner),
                FLAGS => 0,
                ID => 3,
                WORLD => {
                    let k = cpu.r[4] as usize;
                    for (i, row) in objects[k].matrix.iter().enumerate() {
                        put(g, r3 + 16 * i as u32, *row);
                    }
                    u64::from(r3)
                }
                _ => {
                    let k = handles.iter().position(|&h| h == cpu.r[5] as u32).expect("a listed DMO");
                    let object = &objects[k];
                    for (i, row) in object.matrix.iter().enumerate() {
                        put(g, r3 + 16 * i as u32, *row);
                    }
                    put(g, r3 + 0x40, object.min);
                    put(g, r3 + 0x50, object.max);
                    g.set_u32(r3 + 0x68, k as u32).unwrap();
                    u64::from(model)
                }
            };
        });
        let port: Vec<u32> = near_objects(position, objects.iter().copied()).map(|o| o.id).collect();
        let retail: Vec<u32> = (0..o.u32(input + 0xAD8)).map(|j| o.u32(input + 0x6E0 + 0x70 * j + 0x68)).collect();
        assert_eq!(retail, port, "step {step}");
        kept += port.len();
    }
    assert!(kept > 3_000, "only {kept} kept");
}

/// The whole of 82DAB2A8, the Ground collector's keep-going answer, over random collector states: every term,
/// both +945 branches (input +2788 bit 0x40000000, manuals connect) and the float edges of the pump and tilt tests.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_ground_continues_matches_82dab2a8() {
    let mut o = load();
    let (this, owner, holder, display) = (o.alloc(0x400), o.alloc(0x40), o.alloc(0x10), o.alloc(0x100));
    let (motion, wheels, ground) = (o.alloc(0x60), o.alloc(0x10), o.alloc(0x110));
    o.set_u32(this + 4, owner);
    o.set_u32(this + 0xC, holder);
    o.set_u32(holder + 4, display);
    o.set_u32(owner, motion);
    o.set_u32(owner + 0x18, wheels);
    o.set_u32(owner + 0x20, ground);
    let mut r = Rng(0x2A8B_DA82_1357_9BDF);
    let pick = |r: &mut Rng, values: &[u32]| values[(r.unit() * values.len() as f32) as usize % values.len()];
    let (mut held, mut connected) = (0, 0);
    for step in 0..40_000 {
        let byte = |r: &mut Rng, p: f32| u8::from(r.chance(p));
        let id = |r: &mut Rng| pick(r, &[u32::MAX, 0, 7, 331, 332, 400]);
        let floats = |r: &mut Rng, edges: &[u32]| {
            if r.chance(0.3) { f32::from_bits(pick(r, edges)) } else { r.range(-1.0, 1.0) }
        };
        let open = r.chance(0.6);
        let flags = if r.chance(0.5) { 0 } else { pick(&mut r, &[0x0100_0000, 0x1000_0000, 0x2000_0000, 0x0200_0000, 0x0C00_0000]) };
        let live = if r.chance(0.7) { 0.0 } else { floats(&mut r, &[0, 0x8000_0000, 1, 0x7FC0_0000]) };
        let (ollie_present, ollie_id) = (byte(&mut r, 0.2), id(&mut r));
        let (slide_present, slide_id, slide_announced) = (byte(&mut r, 0.2), id(&mut r), byte(&mut r, 0.5));
        let latch = byte(&mut r, 0.1);
        let connect = byte(&mut r, 0.5);
        let count = pick(&mut r, &[u32::MAX, 0, 1, 2, 2, 3, 4]);
        let frames = pick(&mut r, &[0, 1, 2, 3, 7, 8, 9, 40, u32::MAX]);
        let blocked = pick(&mut r, &[0, 1, 6, 7, 30, u32::MAX]);
        let tricks = pick(&mut r, &[u32::MAX, 0, 1, 4]);
        let pump = if r.chance(0.6) { 0.0 } else { floats(&mut r, &[0x3DCC_CCCD, 0x3DCC_CCCE, 0x7FC0_0000, 1]) };
        let up = if r.chance(0.5) { 1.0 } else { floats(&mut r, &[0x3F26_6666, 0x3F26_6665, 0x0000_0001, 0x7FC0_0000]) };
        o.set_u32(this + 0x68, flags);
        o.set_f32(this + 0x74, live);
        o.set_u32(this + 0x8C, ollie_id);
        o.set_u8(this + 0xFC, ollie_present);
        o.set_u32(this + 0x21C, slide_id);
        o.set_u8(this + 0x28C, slide_present);
        o.set_u8(this + 0x2CC, slide_announced);
        o.set_u32(this + 0x3A0, frames);
        o.set_u32(this + 0x3A4, blocked);
        o.set_u8(this + 0x3B0, latch);
        o.set_u8(this + 0x3B1, connect);
        o.set_u32(display + 0x80, tricks);
        o.set_u32(wheels, count);
        o.set_f32(ground + 0x10C, pump);
        o.set_v4(motion + 0x50, [0.3, up, 0.2, 0.1]);
        o.call(0x82DA_B2A8, &[this, u32::from(open)]);
        let in_range = |id: u32| (id as i32) > -1 && (id as i32) < 332;
        let port = ground_continues(&GroundHold {
            open,
            live,
            revert_latch: latch != 0,
            flags,
            slide: slide_present != 0 && in_range(slide_id) && slide_announced != 0,
            ollie: ollie_present != 0 && in_range(ollie_id),
            wheels: count as i32,
            frames,
            manual_blocked: blocked,
            manuals_connect: connect != 0,
            tricks: tricks as i32,
            pump,
            deck_up_y: up,
        });
        assert_eq!(o.r3() & 0xFF != 0, port, "step {step}");
        held += usize::from(port);
        connected += usize::from(port && connect != 0 && tricks == 1 && (count as i32) < 3 && frames >= 8);
    }
    assert!(held > 10_000 && connected > 100, "held {held} connected {connected}");
}

/// A float from `values` or, the rest of the time, from `lo..hi`.
fn pick_f32(r: &mut Rng, values: &[f32], lo: f32, hi: f32) -> f32 {
    if r.chance(0.4) { values[(r.unit() * values.len() as f32) as usize % values.len()] } else { r.range(lo, hi) }
}

/// The ScoreHolder 82DA6198..82DA6538 work on: `+0` the owner, whose `+0x3C` stats block they copy into.
fn holder(o: &mut Tu3) -> u32 {
    let (holder, owner, stats) = (o.alloc(0x1D00), o.alloc(0x40), o.alloc(0x4000));
    o.set_u32(holder, owner);
    o.set_u32(owner + 0x3C, stats);
    holder
}

/// 82DA4C28, the line and multiplier timers' drain: `fmuls drain,dt`, one fused `fnmsubs` by the scale, the
/// near-one hold and the expiry edge. The vtable's `+8` is the line timer's own drain getter 82DA2798
/// (`[this+8]+0x1DC`), and `[[this+0x14]+0xC]` points at dt.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_point_timer_matches_82da4c28() {
    use crate::scoring::timer::PointTimer;
    let mut o = load();
    let (timer, vtable, tuning, clock, dt_at) = (o.alloc(0x20), o.alloc(0x10), o.alloc(0x200), o.alloc(0x10), o.alloc(4));
    o.set_u32(timer, vtable);
    o.set_u32(vtable + 8, 0x82DA_2798);
    o.set_u32(timer + 8, tuning);
    o.set_u32(timer + 0x14, clock);
    o.set_u32(clock + 0xC, dt_at);
    let mut r = Rng(0x4C28_82DA_0BAD_F00D);
    let (mut held, mut expired) = (0, 0);
    for step in 0..40_000 {
        let edges = [0.0, -0.0, -1.0, 1.001, 1.0011, f32::from_bits(0x3F80_0008), 1.000_001, 0.01, f32::NAN];
        let points = pick_f32(&mut r, &edges, 0.0, 400.0);
        let drain = pick_f32(&mut r, &[0.0, 30.0, 100.0, 333.3], 0.0, 400.0);
        let dt = pick_f32(&mut r, &[1.0 / 60.0, 1.0 / 30.0, 0.0], 0.0, 0.1);
        let scale = pick_f32(&mut r, &[1.0, 0.2, 0.05, 0.1, 0.5], 0.0, 1.5);
        let hold = r.chance(0.5);
        let was_expired = u8::from(r.chance(0.3));
        o.set_f32(timer + 0x18, points);
        o.set_u8(timer + 0x1C, was_expired);
        o.set_f32(tuning + 0x1DC, drain);
        o.set_f32(dt_at, dt);
        o.call_floats(0x82DA_4C28, &[timer, 0, u32::from(hold)], &[scale]);
        let mut port = PointTimer { points, expired: was_expired != 0 };
        let port_held = port.advance(dt, drain, scale, hold);
        check("points", step, &[o.f32(timer + 0x18)], &[port.points]);
        assert_eq!(o.u8(timer + 0x1C) != 0, port.expired, "step {step} expired");
        assert_eq!(o.r3() & 0xFF != 0, port_held, "step {step} held");
        held += usize::from(port_held);
        expired += usize::from(port.expired);
    }
    assert!(held > 100 && expired > 1_000, "held {held} expired {expired}");
}

/// 82DA6468, RewardAirSequence: a pending sequence's two buckets go into the accumulator times the landing
/// factor in one `fmadds`, then empty; without one pending, nothing moves.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_reward_sequence_matches_82da6468() {
    use crate::scoring::ScoreHolder;
    let mut o = load();
    let h = holder(&mut o);
    let mut r = Rng(0x6468_82DA_5EED_1234);
    for step in 0..20_000 {
        let edges = [0.0, -0.0, 1.0, 0.1, 1e-7, 1e30];
        let general = pick_f32(&mut r, &edges, 0.0, 5_000.0);
        let ff = pick_f32(&mut r, &edges, 0.0, 500.0);
        let accumulated = pick_f32(&mut r, &edges, 0.0, 20_000.0);
        let factor = pick_f32(&mut r, &[1.0, 1.1, 0.9, 1.21, 0.99, 0.0], 0.0, 2.0);
        let pending = r.chance(0.8);
        o.set_f32(h + 0x20, accumulated);
        o.set_f32(h + 0x28, general);
        o.set_f32(h + 0x2C, ff);
        o.set_u8(h + 0x1C5C, u8::from(pending));
        o.call_floats(0x82DA_6468, &[h], &[factor]);
        let mut port = ScoreHolder::default();
        port.snapshot.accumulated = accumulated;
        port.snapshot.general_pending = general;
        port.snapshot.fingerflip_pending = ff;
        if pending {
            port.finish_collector();
        }
        port.reward_sequence(factor);
        let s = &port.snapshot;
        let retail = [o.f32(h + 0x20), o.f32(h + 0x28), o.f32(h + 0x2C)];
        check("accumulated, general, ff", step, &retail, &[s.accumulated, s.general_pending, s.fingerflip_pending]);
        assert_eq!(o.u8(h + 0x1C5C) != 0, port.has_pending_sequence(), "step {step} pending");
    }
}

/// 82DA6538, PublishAndResetAirSequence: the reward onto the line (when asked) and into the last reward; the
/// sequence's accumulator, buckets and grind reward cleared.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_publish_matches_82da6538() {
    use crate::scoring::ScoreHolder;
    let mut o = load();
    let h = holder(&mut o);
    let mut r = Rng(0x6538_82DA_ABCD_0001);
    for step in 0..20_000 {
        let edges = [0.0, -0.0, 1.0, -5.0, 1e-7];
        let line = pick_f32(&mut r, &edges, 0.0, 50_000.0);
        let reward = pick_f32(&mut r, &edges, 0.0, 20_000.0);
        let (accumulated, general, ff, grind) = (r.range(0.0, 900.0), r.range(0.0, 900.0), r.range(0.0, 90.0), r.range(0.0, 90.0));
        let add = r.chance(0.6);
        for (at, v) in [(0x1C, line), (0x20, accumulated), (0x28, general), (0x2C, ff), (0x12A8, grind)] {
            o.set_f32(h + at, v);
        }
        o.call_floats(0x82DA_6538, &[h, 0, u32::from(add)], &[reward]);
        let mut port = ScoreHolder::default();
        let s = &mut port.snapshot;
        (s.line, s.accumulated, s.general_pending, s.fingerflip_pending, s.grind_reward) = (line, accumulated, general, ff, grind);
        port.publish(reward, add);
        let s = &port.snapshot;
        let retail: Vec<f32> = [0x1C, 0x24, 0x20, 0x28, 0x2C, 0x12A8].iter().map(|&at| o.f32(h + at)).collect();
        let ported = [s.line, s.last_reward, s.accumulated, s.general_pending, s.fingerflip_pending, s.grind_reward];
        check("line, last, accumulated, general, ff, grind", step, &retail, &ported);
    }
}

/// The fixed scorable table 820862A8 the holder indexes (332 x 24 bytes: `+0` id, `+0xC` the class 82DA6260
/// tests, `+0x10` the score type 82DA6198 counts, `+0x14` the identifier) against [`crate::scoring::catalog`].
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_scorable_table_matches_820862a8() {
    use crate::scoring::catalog::IDENTIFIERS;
    let o = load();
    for (id, &(identifier, class, score_type)) in IDENTIFIERS.iter().enumerate() {
        let row = 0x8208_62A8 + 24 * id as u32;
        let name_at = o.u32(row + 0x14);
        let name: String = (0..).map(|i| o.u8(name_at + i)).take_while(|&b| b != 0).map(char::from).collect();
        let retail = (o.u32(row), o.u32(row + 0xC), o.u32(row + 0x10) as usize, name.as_str());
        assert_eq!(retail, (id as u32, class, score_type, identifier), "id {id}");
    }
}

/// 82DA6260, EndAirTrick, through 82DA6198: the id's repetition and sequence counts (saturating at 127), the
/// fingerflip bucket flushed into the general one by anything but a class-5 metric, a class-3 reward held apart,
/// and nothing at all while the holder is suppressed or for an id outside the table.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn air_oracle_end_trick_matches_82da6260() {
    use crate::scoring::{ScoreHolder, catalog};
    let mut o = load();
    let h = holder(&mut o);
    let mut port = ScoreHolder::default();
    let mut r = Rng(0x6260_82DA_7777_0042);
    let ids = [0usize, 2, 3, 96, 129, 237, 253, 331, 50, 140, 200];
    for step in 0..30_000 {
        if step % 3_000 == 0 {
            // A fresh holder now and then (what 82DA5C78 clears), so the counts both saturate and restart.
            for at in 0x1000..0x12A6 {
                o.set_u8(h + at, 0);
            }
            o.set_f32(h + 0x28, 0.0);
            o.set_f32(h + 0x2C, 0.0);
            port = ScoreHolder::default();
        }
        let suppressed = r.chance(0.05);
        o.set_u8(h + 0x1C5D, u8::from(suppressed));
        port.set_suppressed(suppressed);
        let id = if r.chance(0.02) { 332 } else { ids[(r.unit() * ids.len() as f32) as usize % ids.len()] };
        let reward = pick_f32(&mut r, &[0.0, 1.0, 1e-7], 0.0, 300.0);
        o.call_floats(0x82DA_6260, &[h, id as u32], &[reward]);
        if let Some(scorable) = catalog::metadata(id) {
            port.end_trick(scorable, reward);
            let retail = Some(o.u8(h + 0x1000 + id as u32) as i8);
            assert_eq!(retail, port.repetition_count(scorable), "step {step} repetition {id}");
        }
        let s = &port.snapshot;
        check("general, ff", step, &[o.f32(h + 0x28), o.f32(h + 0x2C)], &[s.general_pending, s.fingerflip_pending]);
        let retail: Vec<(usize, i8)> =
            (0..332).map(|i| (i, o.u8(h + 0x114C + i as u32) as i8)).filter(|&(_, n)| n > 0).collect();
        assert_eq!(retail, port.sequence_tricks().collect::<Vec<_>>(), "step {step} sequence history");
    }
}
