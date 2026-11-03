//! Differential tests: the Rust port against Skate 2's own code (see [`super::oracle`]).
//!
//! Ignored: they need the user's disc data, `SKATE2_IMAGE` (from `tools/build_skate2_image.py`)
//! and `SKATE2_VAULT` (the disc's `data/db` directory). Every comparison is of `f32` *bits*.
//!
//! ```text
//! cargo test -p skate-core --no-default-features skate2 -- --ignored --nocapture
//! ```

use std::time::Instant;

use super::float::V4;
use super::oracle::{self, CLASS_PHYSICSDECK, COLLECTION_DEFAULT, Oracle, TWEAKABLES};
use super::*;

const CALC_PUSH: u32 = 0x8297_B7A8;
const QUEUE_FORCE: u32 = 0x8297_EB80;
const ACCUMULATE: u32 = 0x8297_A8B8;
const DYNAMIC_UPDATE: u32 = 0x8293_DE40;

const FIELD_SPEED_CHANGE_LIMIT: u64 = 0x76FE_EE54_A3AD_2AF5;
const FIELD_MAX_PUSHABLE_SPEED: u64 = 0xA9B5_32EB_BED1_6F38;
const FIELD_DECK_FORCE_Y_OFFSET: u64 = 0xDE53_599E_9C53_FA4A;

/// `1/60` as the image stores it (`0x82060140`).
const SIXTIETH: f32 = f32::from_bits(0x3C88_8889);

fn oracle() -> Oracle {
    oracle::load().expect("set SKATE2_IMAGE and SKATE2_VAULT (see tests.rs)")
}

// ------------------------------------------------------------------------------ inputs

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// Zeros, the denormal/normal boundary both ways, the largest finite, infinities, quiet and
/// signalling NaNs of both signs, and a few ordinary constants the code itself uses.
const SPECIAL_BITS: [u32; 26] = [
    0x0000_0000, 0x8000_0000, 0x0000_0001, 0x8000_0001, 0x007F_FFFF, 0x807F_FFFF, 0x0080_0000,
    0x8080_0000, 0x0080_0001, 0x0040_0000, 0x7F7F_FFFF, 0xFF7F_FFFF, 0x7F80_0000, 0xFF80_0000,
    0x7FC0_0000, 0xFFC0_0000, 0x7F80_0001, 0xFFBF_FFFF, 0x7FC1_2345, 0x3F80_0000, 0xBF80_0000,
    0x3F00_0000, 0x3C88_8889, 0x4270_0000, 0x3F35_04F3, 0x1F80_0000,
];

/// One float input: mostly physically sized, the rest special values, raw bits, numbers whose
/// products and sums land on the flush-to-zero boundary, and huge ones.
fn value(r: &mut Rng, scale: f32) -> f32 {
    match r.below(16) {
        0 => f32::from_bits(SPECIAL_BITS[r.below(SPECIAL_BITS.len() as u64) as usize]),
        1 => f32::from_bits(r.next() as u32),
        // 2^-126 .. 2^-96 and 2^-26 .. 2^4 mantissas: products and quotients around MIN_POSITIVE.
        2 => f32::from_bits(((1 + r.below(30)) as u32) << 23 | (r.next() as u32 & 0x807F_FFFF)),
        3 => f32::from_bits(((101 + r.below(30)) as u32) << 23 | (r.next() as u32 & 0x807F_FFFF)),
        4 => f32::from_bits(((200 + r.below(54)) as u32) << 23 | (r.next() as u32 & 0x807F_FFFF)),
        5 => 0.0,
        _ => r.range(-scale, scale),
    }
}

fn vector(r: &mut Rng, scale: f32) -> V4 {
    [value(r, scale), value(r, scale), value(r, scale), value(r, scale)]
}

/// A unit quaternion (as the body carries one), sometimes perturbed.
fn quaternion(r: &mut Rng) -> V4 {
    let q = [r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0)];
    let n = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-3);
    let q = q.map(|x| x / n);
    if r.chance(10) { vector(r, 1.0) } else { q }
}

fn bits(v: V4) -> [u32; 4] {
    v.map(f32::to_bits)
}

// ------------------------------------------------------------------------------ guest layouts

/// The 176 bytes of a native rigid body as eleven vectors, in [`RigidBody`] field order.
fn body_vectors(b: &RigidBody) -> [V4; 11] {
    let [r0, r1, r2] = b.rotation;
    let [i0, i1] = b.inverse_inertia;
    [b.orientation, b.position, b.linear_velocity, b.angular_velocity, r0, r1, r2, i0, i1, b.acceleration, b.angular_acceleration]
}

fn body_from_vectors(v: [V4; 11]) -> RigidBody {
    RigidBody {
        orientation: v[0],
        position: v[1],
        linear_velocity: v[2],
        angular_velocity: v[3],
        rotation: [v[4], v[5], v[6]],
        inverse_inertia: [v[7], v[8]],
        acceleration: v[9],
        angular_acceleration: v[10],
    }
}

fn put_body(o: &mut Oracle, at: u32, b: &RigidBody) {
    for (i, v) in body_vectors(b).into_iter().enumerate() {
        o.set_v4(at + 16 * i as u32, v);
    }
}

fn get_body(o: &Oracle, at: u32) -> RigidBody {
    body_from_vectors(std::array::from_fn(|i| o.v4(at + 16 * i as u32)))
}

fn body_bits(b: &RigidBody) -> [[u32; 4]; 11] {
    body_vectors(b).map(bits)
}

/// The body's `w` lanes at `+28`, `+76`, `+92` are the corrections, simulation and definition
/// pointers: the code carries them through untouched, so the port sees them as opaque bits.
fn link_body(b: &mut RigidBody, corrections: u32, step: u32, definition: u32) {
    b.position[3] = f32::from_bits(corrections);
    b.rotation[0][3] = f32::from_bits(step);
    b.rotation[1][3] = f32::from_bits(definition);
}

const PPI_BYTES: u32 = 2432;
const SKATEBOARD_BYTES: u32 = 1344;

/// The objects the push chain touches, laid out natively in the oracle's heap.
struct Rig {
    /// `{force, point}` written by the push and read by the queue.
    out: u32,
    skateboard: u32,
    ppi: u32,
    records: u32,
    body: u32,
    step: u32,
    definition: u32,
    corrections: u32,
    limit_at: u32,
    max_at: u32,
    deck_offset_at: u32,
}

impl Rig {
    fn new(o: &mut Oracle) -> Self {
        let out = o.alloc(32);
        let skateboard = o.alloc(SKATEBOARD_BYTES);
        let ppi = o.alloc(PPI_BYTES);
        let records = o.alloc(48 * FORCE_RECORD_CAPACITY as u32);
        let body = o.alloc(176);
        let step = o.alloc(176);
        let definition = o.alloc(48);
        let corrections = o.alloc(64);
        o.set_u32(skateboard + 16, body);
        o.set_u32(skateboard + 272, ppi);
        // Storage reserved for 21 records, so the queue never reaches its grow path (0x82989590).
        o.set_u32(skateboard + 288, records);
        o.set_u32(skateboard + 292, records);
        o.set_u32(skateboard + 296, records + 48 * FORCE_RECORD_CAPACITY as u32);
        // The physicsdeck instance is embedded at +1332 (its first word is the collection);
        // the tweakables table holds a *pointer* to the physics_push instance.
        let deck = o.instance(CLASS_PHYSICSDECK, COLLECTION_DEFAULT);
        let deck_collection = o.u32(deck);
        o.set_u32(skateboard + 1332, deck_collection);
        let deck = skateboard + 1332;
        let push = o.u32(o.u32(TWEAKABLES) + 176);
        let rig = Self {
            out,
            skateboard,
            ppi,
            records,
            body,
            step,
            definition,
            corrections,
            limit_at: o.attribute(push, FIELD_SPEED_CHANGE_LIMIT),
            max_at: o.attribute(push, FIELD_MAX_PUSHABLE_SPEED),
            deck_offset_at: o.attribute(deck, FIELD_DECK_FORCE_Y_OFFSET),
        };
        assert!(rig.limit_at != 0 && rig.max_at != 0, "physics_push/default lacks a push field");
        assert!(rig.deck_offset_at != 0, "physicsdeck/default lacks DeckForceYOffset");
        rig
    }

    fn tweaks(&self, o: &Oracle) -> PushTweaks {
        PushTweaks { speed_change_limit: o.f32(self.limit_at), max_pushable_speed: o.f32(self.max_at) }
    }

    fn put_input(&self, o: &mut Oracle, i: &PushInput) {
        let p = self.ppi;
        o.set_u32(p + 2244, i.flags);
        o.set_u32(p + 2248, i.flags2);
        o.set_f32(p + 2364, i.delta_seconds);
        o.set_f32(p + 2368, i.target_speed);
        o.set_f32(p + 2372, i.current_speed);
        o.set_f32(p + 2376, i.body_speed);
        o.set_f32(p + 2416, i.mass);
        o.set_v4(p + 352, i.direction);
    }

    fn put_step(&self, o: &mut Oracle, s: &SimulationStep) {
        o.set_v4(self.step + 144, s.gravity);
        o.set_f32(self.step + 160, s.time_step);
        o.set_f32(self.step + 164, s.frequency);
        o.set_u32(self.step + 168, s.sleep_limit);
        o.set_f32(self.step + 172, s.minimum_energy);
    }

    fn put_definition(&self, o: &mut Oracle, d: &BodyDefinition) {
        o.set_v4(self.definition, d.inverse_inertia);
        o.set_f32(self.definition + 20, d.angular_energy_weight);
        o.set_f32(self.definition + 24, d.max_linear_speed);
        o.set_f32(self.definition + 28, d.max_angular_speed);
        o.set_f32(self.definition + 32, d.linear_drag);
        o.set_f32(self.definition + 36, d.angular_drag);
    }

    fn put_corrections(&self, o: &mut Oracle, c: &Corrections) {
        for (i, v) in [c.linear, c.position, c.angular, c.orientation].into_iter().enumerate() {
            o.set_v4(self.corrections + 16 * i as u32, v);
        }
    }

    fn get_corrections(&self, o: &Oracle) -> Corrections {
        let v = |i: u32| o.v4(self.corrections + 16 * i);
        Corrections { linear: v(0), position: v(1), angular: v(2), orientation: v(3) }
    }

    fn put_records(&self, o: &mut Oracle, records: &[ForceRecord]) {
        for (i, r) in records.iter().enumerate() {
            let at = self.records + 48 * i as u32;
            o.set_u32(at, r.tag);
            o.set_v4(at + 16, r.force);
            o.set_v4(at + 32, r.point);
        }
        o.set_u32(self.skateboard + 292, self.records + 48 * records.len() as u32);
    }

    fn get_records(&self, o: &Oracle) -> Vec<ForceRecord> {
        let end = o.u32(self.skateboard + 292);
        (self.records..end)
            .step_by(48)
            .map(|at| ForceRecord { tag: o.u32(at), force: o.v4(at + 16), point: o.v4(at + 32) })
            .collect()
    }
}

fn random_input(r: &mut Rng) -> PushInput {
    let mut flags = r.next() as u32;
    if r.chance(85) {
        flags |= PUSH_REQUESTED;
    }
    let mut flags2 = r.next() as u32;
    if r.chance(85) {
        flags2 &= !PUSH_BLOCKED;
    }
    PushInput {
        flags,
        flags2,
        delta_seconds: if r.chance(50) { SIXTIETH } else { value(r, 0.1) },
        target_speed: value(r, 15.0),
        current_speed: value(r, 15.0),
        body_speed: value(r, 15.0),
        mass: value(r, 100.0),
        direction: vector(r, 1.0),
    }
}

fn random_tweaks(r: &mut Rng, vault: PushTweaks) -> PushTweaks {
    if r.chance(30) {
        return vault;
    }
    PushTweaks { speed_change_limit: value(r, 2.0), max_pushable_speed: value(r, 15.0) }
}

fn random_body(r: &mut Rng) -> RigidBody {
    let mut b = body_from_vectors(std::array::from_fn(|_| vector(r, 10.0)));
    if r.chance(70) {
        b.orientation = quaternion(r);
    }
    b.inverse_inertia[0][3] = if r.chance(80) { r.range(0.0, 1.0) } else { value(r, 1.0) };
    b.angular_acceleration[3] = f32::from_bits(match r.below(4) {
        0 => r.next() as u32,
        _ => r.below(130) as u32,
    });
    b
}

fn random_step(r: &mut Rng) -> SimulationStep {
    let (time_step, frequency) = if r.chance(60) { (SIXTIETH, 60.0) } else { (value(r, 0.1), value(r, 100.0)) };
    SimulationStep {
        gravity: vector(r, 10.0),
        time_step,
        frequency,
        sleep_limit: if r.chance(80) { r.below(128) as u32 } else { r.next() as u32 },
        minimum_energy: value(r, 1.0),
    }
}

fn random_definition(r: &mut Rng) -> BodyDefinition {
    BodyDefinition {
        inverse_inertia: vector(r, 5.0),
        angular_energy_weight: value(r, 2.0),
        max_linear_speed: value(r, 20.0),
        max_angular_speed: value(r, 20.0),
        linear_drag: value(r, 70.0),
        angular_drag: value(r, 70.0),
    }
}

fn random_corrections(r: &mut Rng) -> Corrections {
    if r.chance(30) {
        return Corrections::default();
    }
    Corrections { linear: vector(r, 0.1), position: vector(r, 0.1), angular: vector(r, 0.1), orientation: vector(r, 0.1) }
}

fn random_record(r: &mut Rng) -> ForceRecord {
    ForceRecord { tag: r.below(8) as u32, force: vector(r, 500.0), point: vector(r, 1.0) }
}

// ------------------------------------------------------------------------------ tests

#[test]
#[ignore = "needs SKATE2_IMAGE and SKATE2_VAULT from the user's Skate 2 disc"]
fn push_matches_skate2() {
    let mut o = oracle();
    let rig = Rig::new(&mut o);
    let vault = rig.tweaks(&o);
    println!("physics_push/default: limit {} MaxPushableSpeed {}", vault.speed_change_limit, vault.max_pushable_speed);
    let mut r = Rng(0x5EED_0001);
    let (cases, mut steps, mut pushed) = (200_000, 0u64, 0);
    for case in 0..cases {
        let input = random_input(&mut r);
        let tweaks = random_tweaks(&mut r, vault);
        let skateboard_flags = r.next() as u32;
        rig.put_input(&mut o, &input);
        o.set_f32(rig.limit_at, tweaks.speed_change_limit);
        o.set_f32(rig.max_at, tweaks.max_pushable_speed);
        o.set_u32(rig.skateboard + 1328, skateboard_flags);
        o.g.fill(rig.out, 0xA5, 32).unwrap();
        steps += o.call(CALC_PUSH, &[rig.out, rig.skateboard]);

        let ours = calc_push_acceleration(&input, &tweaks);
        let flags = skateboard_flags | if ours.blocked { PUSH_BLOCKED_FLAG } else { 0 };
        let (force, point) = (o.v4(rig.out), o.v4(rig.out + 16));
        assert_eq!(
            (bits(ours.force), bits(ours.point), flags),
            (bits(force), bits(point), o.u32(rig.skateboard + 1328)),
            "case {case}: {input:?} {tweaks:?}"
        );
        pushed += usize::from(ours.force != [0.0; 4]);
    }
    println!("push: {cases} cases bit-identical ({pushed} with a force), {:.0} instructions/case", steps as f64 / cases as f64);

    // No collection behind the tweakables slot: both fields read the image's zero block.
    let null = o.instance(0, 0);
    let table = o.u32(TWEAKABLES);
    o.set_u32(table + 176, null);
    let reads = o.fakes().attribute_reads.len();
    for case in 0..20_000 {
        let input = random_input(&mut r);
        rig.put_input(&mut o, &input);
        o.call(CALC_PUSH, &[rig.out, rig.skateboard]);
        let ours = calc_push_acceleration(&input, &PushTweaks { speed_change_limit: 0.0, max_pushable_speed: 0.0 });
        assert_eq!(bits(ours.force), bits(o.v4(rig.out)), "null tweakables, case {case}: {input:?}");
    }
    assert_eq!(o.fakes().attribute_reads.len(), reads, "no attribute lookups without a collection");
}

#[test]
#[ignore = "needs SKATE2_IMAGE and SKATE2_VAULT from the user's Skate 2 disc"]
fn queue_and_accumulate_match_skate2() {
    let mut o = oracle();
    let rig = Rig::new(&mut o);
    let vault_offset = o.f32(rig.deck_offset_at);
    println!("physicsdeck/default DeckForceYOffset {vault_offset}");
    let mut r = Rng(0x5EED_0002);
    let (cases, mut steps) = (50_000, 0u64);
    for case in 0..cases {
        // Queue one record onto 0..=21 existing ones.
        let mut records: Vec<ForceRecord> = (0..r.below(22)).map(|_| random_record(&mut r)).collect();
        rig.put_records(&mut o, &records);
        let new = random_record(&mut r);
        o.set_v4(rig.out, new.force);
        o.set_v4(rig.out + 16, new.point);
        steps += o.call(QUEUE_FORCE, &[rig.skateboard, new.tag, rig.out]);
        queue_force(&mut records, new.tag, new.force, new.point);
        let queued = rig.get_records(&o);
        assert_eq!(queued.len(), records.len(), "case {case}: record count");
        for (a, b) in queued.iter().zip(&records) {
            assert_eq!((a.tag, bits(a.force), bits(a.point)), (b.tag, bits(b.force), bits(b.point)), "case {case}");
        }

        // Accumulate them into a random body.
        let offset = if r.chance(30) { vault_offset } else { value(&mut r, 1.0) };
        o.set_f32(rig.deck_offset_at, offset);
        let mut body = random_body(&mut r);
        put_body(&mut o, rig.body, &body);
        steps += o.call(ACCUMULATE, &[rig.skateboard]);
        accumulate_forces(&records, offset, &mut body);
        assert_eq!(body_bits(&body), body_bits(&get_body(&o, rig.body)), "case {case}: offset {offset} {records:?}");
    }
    println!("queue + accumulate: {cases} cases bit-identical, {:.0} instructions/case", steps as f64 / cases as f64);

    // No physicsdeck collection: the offset reads the image's zero block.
    o.set_u32(rig.skateboard + 1332, 0);
    for case in 0..5_000 {
        let records: Vec<ForceRecord> = (0..1 + r.below(21)).map(|_| random_record(&mut r)).collect();
        rig.put_records(&mut o, &records);
        let mut body = random_body(&mut r);
        put_body(&mut o, rig.body, &body);
        o.call(ACCUMULATE, &[rig.skateboard]);
        accumulate_forces(&records, 0.0, &mut body);
        assert_eq!(body_bits(&body), body_bits(&get_body(&o, rig.body)), "null deck, case {case}");
    }
}

#[test]
#[ignore = "needs SKATE2_IMAGE and SKATE2_VAULT from the user's Skate 2 disc"]
fn integrator_matches_skate2() {
    let mut o = oracle();
    let rig = Rig::new(&mut o);
    let mut r = Rng(0x5EED_0003);
    let (cases, mut steps) = (100_000, 0u64);
    for case in 0..cases {
        let step = random_step(&mut r);
        let definition = random_definition(&mut r);
        let mut corrections = random_corrections(&mut r);
        let mut body = random_body(&mut r);
        link_body(&mut body, rig.corrections, rig.step, rig.definition);
        rig.put_step(&mut o, &step);
        rig.put_definition(&mut o, &definition);
        rig.put_corrections(&mut o, &corrections);
        put_body(&mut o, rig.body, &body);
        let before = body;
        steps += o.call(DYNAMIC_UPDATE, &[rig.body]);
        dynamic_update(&mut body, &step, &definition, &mut corrections);
        assert_eq!(
            (body_bits(&body), bits(corrections.linear)),
            (body_bits(&get_body(&o, rig.body)), bits(rig.get_corrections(&o).linear)),
            "case {case}: {before:?} {step:?} {definition:?}"
        );
        assert_eq!(rig.get_corrections(&o), Corrections::default());
    }
    println!("integrator: {cases} cases bit-identical, {:.0} instructions/case", steps as f64 / cases as f64);
}

/// A skater pushing on flat ground for 20 seconds per run, every tick through the whole chain on
/// both sides, comparing the body's bits after every tick; also the oracle's speed.
///
/// Harness fakes (what the rest of Skate 2 would supply): the `ProcessedPhysIn` speeds are the
/// body's horizontal speed, the push direction is the deck's forward row, the push button is held
/// in strokes of 50 ticks out of 120; the records are cleared after each tick. The ground contact
/// the solver would add is stood in for by zero gravity, no corrections and, on three runs in
/// four, a deck that cannot pitch or roll (local inverse inertia `x = z = 0`) and starts upright;
/// the fourth run tumbles freely to exercise the full 3-D path.
#[test]
#[ignore = "needs SKATE2_IMAGE and SKATE2_VAULT from the user's Skate 2 disc"]
fn pushing_chain_matches_skate2_tick_for_tick() {
    let mut o = oracle();
    let rig = Rig::new(&mut o);
    let tweaks = rig.tweaks(&o);
    let deck_offset = o.f32(rig.deck_offset_at);
    let mut r = Rng(0x5EED_0004);
    const RUNS: usize = 20;
    const TICKS: usize = 1200;
    let (mut steps, mut oracle_time, mut rust_time) = (0u64, 0.0, 0.0);
    o.cpu.trace = Some(Default::default());
    for run in 0..RUNS {
        let step = SimulationStep {
            gravity: [0.0; 4],
            time_step: SIXTIETH,
            frequency: 60.0,
            sleep_limit: 60,
            minimum_energy: r.range(0.0, 0.05),
        };
        let flat = run % 4 != 3;
        let mut definition = BodyDefinition {
            inverse_inertia: [r.range(1.0, 40.0), r.range(1.0, 40.0), r.range(1.0, 40.0), 0.0],
            angular_energy_weight: r.range(0.0, 1.0),
            max_linear_speed: r.range(3.0, 20.0),
            max_angular_speed: r.range(5.0, 30.0),
            linear_drag: r.range(0.0, 1.0),
            angular_drag: r.range(0.0, 5.0),
        };
        let mut body = RigidBody { orientation: quaternion(&mut r), ..RigidBody::default() };
        body.angular_velocity = [r.range(-0.5, 0.5), r.range(-2.0, 2.0), r.range(-0.5, 0.5), 0.0];
        if flat {
            let half_yaw = r.range(-1.5, 1.5);
            body.orientation = [0.0, half_yaw.sin(), 0.0, half_yaw.cos()];
            body.angular_velocity = [0.0, r.range(-0.5, 0.5), 0.0, 0.0];
            definition.inverse_inertia[0] = 0.0;
            definition.inverse_inertia[2] = 0.0;
        }
        // The push's mass is the deck body's, so one push tick changes the speed by what it asked.
        let mass = r.range(20.0, 90.0);
        body.inverse_inertia[0][3] = 1.0 / mass;
        let mut corrections = Corrections::default();
        // One integration with no forces builds the rotation and inertia from the orientation.
        link_body(&mut body, rig.corrections, rig.step, rig.definition);
        rig.put_step(&mut o, &step);
        rig.put_definition(&mut o, &definition);
        rig.put_corrections(&mut o, &corrections);
        put_body(&mut o, rig.body, &body);
        o.call(DYNAMIC_UPDATE, &[rig.body]);
        dynamic_update(&mut body, &step, &definition, &mut corrections);

        let target = r.range(2.0, 9.0);
        let mut records = Vec::new();
        for tick in 0..TICKS {
            let v = body.linear_velocity;
            let speed = (v[0] * v[0] + v[2] * v[2]).sqrt();
            let forward = body.rotation[2];
            let input = PushInput {
                flags: if tick % 120 < 50 { PUSH_REQUESTED } else { 0 },
                flags2: 0,
                delta_seconds: SIXTIETH,
                target_speed: target,
                current_speed: speed,
                body_speed: speed,
                mass,
                direction: [forward[0], forward[1], forward[2], 0.0],
            };

            let t = Instant::now();
            rig.put_input(&mut o, &input);
            steps += o.call(CALC_PUSH, &[rig.out, rig.skateboard]);
            steps += o.call(QUEUE_FORCE, &[rig.skateboard, 3, rig.out]);
            steps += o.call(ACCUMULATE, &[rig.skateboard]);
            steps += o.call(DYNAMIC_UPDATE, &[rig.body]);
            o.set_u32(rig.skateboard + 292, rig.records);
            oracle_time += t.elapsed().as_secs_f64();

            let t = Instant::now();
            let push = calc_push_acceleration(&input, &tweaks);
            queue_force(&mut records, 3, push.force, push.point);
            accumulate_forces(&records, deck_offset, &mut body);
            dynamic_update(&mut body, &step, &definition, &mut corrections);
            records.clear();
            rust_time += t.elapsed().as_secs_f64();

            assert_eq!(body_bits(&body), body_bits(&get_body(&o, rig.body)), "run {run} tick {tick}");
        }
        let v = body.linear_velocity;
        println!(
            "run {run:2} ({}): speed {:.3} m/s after {TICKS} ticks, position {:?}, sleep {}",
            if flat { "flat" } else { "tumbling" },
            (v[0] * v[0] + v[2] * v[2]).sqrt(),
            &body.position[..3],
            body.sleep_counter()
        );
    }
    let ticks = (RUNS * TICKS) as f64;
    println!(
        "chain: {} ticks bit-identical; oracle {:.0} instructions/tick, {:.2} us/tick ({:.1} M instructions/s); Rust {:.3} us/tick",
        RUNS * TICKS,
        steps as f64 / ticks,
        oracle_time / ticks * 1e6,
        steps as f64 / oracle_time / 1e6,
        rust_time / ticks * 1e6
    );
    let trace = o.cpu.trace.take().unwrap();
    println!("Skate 2 functions entered (address: calls):");
    for (target, calls) in &trace.calls {
        println!("  {target:08X}: {calls}");
    }
}

/// Tier B probe: Skate 2's real board tick entry point, `UpdateSkateboard` 0x82B5FC68, on the rig's
/// board with a push requested. Fake: a zeroed page at guest address 0 stands in for every
/// pointer the harness does not set (the board's other subsystems), so null-guarded code takes its
/// empty paths. The push record the real tick queues must equal the Rust push's bits.
#[test]
#[ignore = "needs SKATE2_IMAGE and SKATE2_VAULT from the user's Skate 2 disc"]
fn real_update_skateboard_queues_the_same_push() {
    let mut o = oracle();
    o.g.put(0, vec![0; 0x1_0000]);
    let rig = Rig::new(&mut o);
    let (this, character, player) = (o.alloc(512), o.alloc(16384), o.alloc(8192));
    o.set_u32(this + 4, rig.skateboard);
    o.set_u32(this + 8, character);
    o.set_u32(rig.skateboard + 12, player);
    let input = PushInput {
        flags: PUSH_REQUESTED,
        flags2: 0,
        delta_seconds: SIXTIETH,
        target_speed: 6.0,
        current_speed: 2.0,
        body_speed: 2.0,
        mass: 75.0,
        direction: [0.6, 0.0, 0.8, 0.0],
    };
    rig.put_input(&mut o, &input);
    o.cpu.trace = Some(Default::default());
    let steps = o.call(0x82B5_FC68, &[this]);
    let trace = o.cpu.trace.take().unwrap();
    let null_writes = o.g.span(0, 0x1_0000).unwrap().iter().filter(|&&b| b != 0).count();
    let records = rig.get_records(&o);
    println!(
        "UpdateSkateboard: {steps} instructions, {} functions entered, {null_writes} bytes written through null; records (tag, force) {:?}",
        trace.calls.len(),
        records.iter().map(|r| (r.tag, r.force)).collect::<Vec<_>>()
    );
    assert_eq!(trace.calls.get(&CALC_PUSH), Some(&1));
    let push: Vec<_> = records.iter().filter(|r| r.tag == 3).collect();
    let ours = calc_push_acceleration(&input, &rig.tweaks(&o));
    assert_eq!(push.len(), 1);
    assert_eq!((bits(push[0].force), bits(push[0].point)), (bits(ours.force), bits(ours.point)));
}
