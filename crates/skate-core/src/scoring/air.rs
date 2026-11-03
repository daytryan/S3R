//! The Air and Handplant collectors' own maths, ported instruction by instruction.
//!
//! Everything here is computed the way the TU3 oracle computes it (`physics::skate2::float`):
//! VMX lanes flush denormals and round `vmaddfp` twice, `vrsqrtefp` is RexGlue's table, scalar
//! `fmadds` is fused. `air_oracle` runs each function's TU3 original in the PowerPC interpreter
//! and compares the bits, so a change here that drifts from retail fails a test rather than a
//! playtest.
use crate::physics::skate2::float::{self as ppc, V4};

pub type Matrix = [V4; 4];

const ONE: V4 = [1.0; 4];
const HALF: V4 = [0.5; 4];
const ZERO: V4 = [0.0; 4];
/// 0x82060C44 / 0x822F8908, the wrap bounds of 82DAC880.
const PI: f32 = f32::from_bits(0x4049_0FDB);
/// 0x820B411C and 0x821647F0.
const TAU: f32 = f32::from_bits(0x40C9_0FDB);
/// 0x82084620: radians to degrees in 82DA8BE0.
pub const DEGREES: f32 = f32::from_bits(0x4265_2EE1);
/// 0x822F8B30: 82DA8BE0 multiplies by this rather than dividing by 180.
const PER_HALF_TURN: f32 = f32::from_bits(0x3BB6_0B61);

/// The title's inlined `vrsqrtefp` + Newton step(s): `r = (r * 0.5) * (1 - q * r * r) + r`.
fn rsqrt(q: V4, steps: usize) -> V4 {
    let mut r = ppc::vrsqrtefp(q);
    for _ in 0..steps {
        let rr = ppc::vmulfp(r, r);
        let h = ppc::vmulfp(r, HALF);
        r = ppc::vmaddfp(h, ppc::vnmsubfp(q, rr, ONE), r);
    }
    r
}

/// `a x b` the way the title writes it: `vpermwi 99` (yzx) on both sides of a `vnmsubfp`.
fn cross(a: V4, b: V4) -> V4 {
    let rotated = ppc::vnmsubfp(ppc::permwi(a, 99), b, ppc::vmulfp(a, ppc::permwi(b, 99)));
    ppc::permwi(rotated, 99)
}

/// Unguarded normalise, two Newton steps (82DAC9D0, 82D2CDC0).
fn normalize(v: V4) -> V4 {
    ppc::vmulfp(v, rsqrt(ppc::vmsum3fp(v, v), 2))
}

/// `|v|` as 82DA7B50 / 82DA8FF8 / 82D2CDC0 take it: `q * rsqrt(q)`, and exactly zero for `q == 0`.
fn length(v: V4) -> f32 {
    let q = ppc::vmsum3fp(v, v);
    let l = ppc::vmulfp(q, rsqrt(q, 2));
    ppc::select(ppc::vcmpeqfp(ZERO, q), l, ZERO)[0]
}

/// `vcmpgtfp128.` + `mfocrf`/`rlwinm 25`: true only when every lane compares greater.
fn all_gt(a: V4, b: V4) -> bool {
    ppc::vcmpgtfp(a, b).iter().all(|&g| g)
}

/// RexGlue `vmaxfp`/`vminfp` are `maxps`/`minps`: the second operand unless the first wins.
fn vmax(a: V4, b: V4) -> V4 {
    std::array::from_fn(|i| if a[i] > b[i] { a[i] } else { b[i] })
}
fn vmin(a: V4, b: V4) -> V4 {
    std::array::from_fn(|i| if a[i] < b[i] { a[i] } else { b[i] })
}

/// RexGlue `vrefp` is a true division (`skate-audio-core::vmx::vrefp`).
fn vrefp(a: V4) -> V4 {
    a.map(|x| {
        if x.is_nan() {
            f32::from_bits(x.to_bits() | 0x0040_0000)
        } else {
            ppc::round_single(1.0 / f64::from(ppc::daz(x)))
        }
    })
}

fn bits(b: [u32; 4]) -> V4 {
    b.map(f32::from_bits)
}

/// 82453298, the vector `acos` the angle helper calls (lanes in, lanes out).
fn acos(x: V4) -> V4 {
    let c0 = bits([0xBD6D_D42D, 0xBED6_5553, 0x3E66_3246, 0x400B_1889]); // 822F9820
    let c1 = bits([0x3F1D_D7B6, 0x4089_80BD, 0xBF98_3F2F, 0xC0D1_360E]); // 822F9830
    let c2 = bits([0xBFAF_4418, 0xC08F_6AD9, 0x3FB5_8485, 0x40AF_6AD8]); // 822F9840
    let pi = [f32::from_bits(0x4049_0FDB); 4]; // 822F9850 lane 0
    let one_plus = [f32::from_bits(0x3F80_0001); 4]; // 822FB840
    let s = |v: V4, i| ppc::splat(v, i);
    let x2 = ppc::vmulfp(x, x);
    let ax = x.map(|l| f32::from_bits(l.to_bits() & 0x7FFF_FFFF)); // vandc with the sign mask
    let headroom = ppc::vsubfp(one_plus, ax);
    let odd = ppc::vnmsubfp(ax, x, x);
    let p3 = ppc::vmaddfp(s(c0, 3), ax, s(c1, 3));
    let x3 = ppc::vmulfp(x2, ax);
    let p2 = ppc::vmaddfp(s(c0, 2), ax, s(c1, 2));
    let p1 = ppc::vmaddfp(s(c0, 1), ax, s(c1, 1));
    let p0 = ppc::vmaddfp(s(c0, 0), ax, s(c1, 0));
    let r = ppc::vrsqrtefp(headroom);
    let half_headroom = ppc::vmulfp(headroom, HALF);
    let half_pi = ppc::vmulfp(pi, HALF);
    let q3 = ppc::vmaddfp(p3, ax, s(c2, 3));
    let q2 = ppc::vmaddfp(p2, ax, s(c2, 2));
    let q1 = ppc::vmaddfp(p1, ax, s(c2, 1));
    let q0 = ppc::vmaddfp(p0, ax, s(c2, 0));
    let rr = ppc::vmulfp(r, r);
    let high = ppc::vmaddfp(q2, x3, q3);
    let low = ppc::vmaddfp(q0, x3, q1);
    let e = ppc::vnmsubfp(half_headroom, rr, HALF);
    let a0 = ppc::vmulfp(x, high);
    let a1 = ppc::vmulfp(odd, low);
    let r = ppc::vmaddfp(r, e, r);
    ppc::vsubfp(half_pi, ppc::vmaddfp(a1, r, a0))
}

/// 8296EC98: the angle from `a` to `b`, taken the long way round when `a x b` points against
/// `axis`. Either input with a squared length not above 1e-4 (0x8209BE90) gives zero.
fn signed_angle(a: V4, b: V4, axis: V4) -> f32 {
    let epsilon = [f32::from_bits(0x38D1_B717); 4];
    let qa = ppc::vmsum3fp(a, a);
    if !all_gt(qa, epsilon) {
        return 0.0;
    }
    let qb = ppc::vmsum3fp(b, b);
    if !all_gt(qb, epsilon) {
        return 0.0;
    }
    let ua = ppc::vmulfp(a, rsqrt(qa, 1));
    let ub = ppc::vmulfp(b, rsqrt(qb, 1));
    let cosine = vmin(vmax(ppc::vmsum3fp(ua, ub), [-1.0; 4]), ONE);
    let angle = acos(cosine);
    if all_gt(ZERO, ppc::vmsum3fp(cross(ua, ub), axis)) {
        ppc::vsubfp([TAU; 4], angle)[0]
    } else {
        angle[0]
    }
}

/// 82DAC9D0 with axis selector 0 (`[acc+68]`, which both 82DA8BE0 accumulators leave zero):
/// the new forward (row 2) flattened into the old frame's plane and measured against the old
/// forward about the old up (row 1). A step of more than a quarter turn reads zero.
pub fn yaw_delta(reference: &Matrix, current: &Matrix) -> f32 {
    let (up, forward) = (reference[1], reference[2]);
    let normal = normalize(cross(up, current[2]));
    let flattened = normalize(cross(normal, up));
    if all_gt(ZERO, ppc::vmsum3fp(flattened, forward)) {
        return 0.0;
    }
    signed_angle(flattened, forward, up)
}

/// 82DAC880: one of 82DA8BE0's two rotation accumulators (`+0` reference, `+64` angle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotationAccumulator {
    pub reference: Matrix,
    pub angle: f32,
}

impl RotationAccumulator {
    pub fn new(reference: Matrix) -> Self {
        Self { reference, angle: 0.0 }
    }
    pub fn update(&mut self, current: &Matrix) {
        let mut step = yaw_delta(&self.reference, current);
        if step > PI {
            step = ppc::fsubs(step, TAU);
        } else if step < -PI {
            step = ppc::fadds(step, TAU);
        }
        self.angle = ppc::fadds(step, self.angle);
        self.reference = *current;
    }
}

/// 82D2CDC0(up, forward): rows right = up x forward, up, forward = right x up, all unit, and a
/// zero translation; the identity when `|up x forward|` is not above 0x830BD300 (2^-16,
/// splatted there from 0x821647E0 by 82F82690).
pub fn frame(up: V4, forward: V4) -> Matrix {
    let right = cross(up, forward);
    let q = ppc::vmsum3fp(right, right);
    let l = ppc::vmulfp(q, rsqrt(q, 2));
    let len = ppc::select(ppc::vcmpeqfp(ZERO, q), l, ZERO);
    if !all_gt(len, [f32::from_bits(0x3780_0000); 4]) {
        return [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], ZERO];
    }
    let uq = ppc::vmsum3fp(up, up);
    let estimate = vrefp(len);
    let inverse = ppc::vmaddfp(estimate, ppc::vnmsubfp(estimate, len, ONE), estimate);
    let inverse = ppc::vmaddfp(inverse, ppc::vnmsubfp(inverse, len, ONE), inverse);
    let row0 = ppc::vmulfp(inverse, right);
    let row1 = ppc::vmulfp(up, rsqrt(uq, 2));
    let row2 = normalize(cross(row0, row1));
    [row0, row1, row2, ZERO]
}

/// 82DA8BE0's rotation state: accumulator A (`+816`) over the rider skeleton's root, B (`+896`)
/// over a frame built from the reckoning up and the board's forward, the banked degrees
/// (`+2384`) and the latch `+2395` that says B, not A, is the live contributor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpinTracker {
    pub rider: RotationAccumulator,
    pub board: RotationAccumulator,
    pub banked_degrees: f32,
    pub board_phase: bool,
}

impl SpinTracker {
    /// The Air collector's Reset 82DA7DA0: A from `[[PhysOut+20]+368]`, B from `[PhysOut+0]`.
    pub fn new(rider: Matrix, board: Matrix) -> Self {
        Self {
            rider: RotationAccumulator::new(rider),
            board: RotationAccumulator::new(board),
            banked_degrees: 0.0,
            board_phase: true,
        }
    }

    fn bank_rider(&mut self, rider: &Matrix, board: &Matrix) {
        self.banked_degrees = ppc::fmadds(self.rider.angle, DEGREES, self.banked_degrees);
        self.rider = RotationAccumulator::new(*rider);
        self.board = RotationAccumulator::new(*board);
    }

    /// One frame of 82DA8BE0; `carried` is "byte 708 and a carrier id in [0, 332)". Returns the
    /// signed total in degrees that the half-turn count is taken from.
    pub fn update(&mut self, carried: bool, rider: &Matrix, board: &Matrix, reckoning_up: V4) -> f32 {
        self.rider.update(rider);
        match (carried, self.board_phase) {
            (true, true) => {
                self.bank_rider(rider, board);
                self.board_phase = false;
            }
            (true, false) => self.board = RotationAccumulator::new(*board),
            (false, true) => self.board.update(&frame(reckoning_up, board[2])),
            (false, false) => {
                self.bank_rider(rider, board);
                self.board_phase = true;
            }
        }
        let board_degrees = ppc::fmuls(self.board.angle, DEGREES);
        let base = if self.board_phase {
            self.banked_degrees
        } else {
            ppc::fmadds(self.rider.angle, DEGREES, self.banked_degrees)
        };
        ppc::fadds(base, board_degrees)
    }
}

/// 82DA8BE0's `+2344`: `fctiwz((|deg| + [0x63C]) * (1/180))`, negated for a negative total.
pub fn half_turns(degrees: f32, offset: f32) -> i32 {
    let scaled = ppc::fmuls(ppc::fadds(degrees.abs(), offset), PER_HALF_TURN);
    let turns = if scaled.is_nan() { i32::MIN } else { scaled as i32 };
    // `fcmpu f0,0.0 / bge`: a NaN total is not "greater or equal", so it is negated too.
    if degrees >= 0.0 { turns } else { turns.wrapping_neg() }
}

/// 82DA7A38's result block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroundQuery {
    /// +0: Ground+192, the hips ray's origin, copied whether or not it hit.
    pub origin: V4,
    /// +16: origin height above the hit.
    pub clearance: f32,
    /// +20: the lower of the reference and the origin, above the hit.
    pub drop: f32,
    /// +24..+26.
    pub hit: bool,
    pub tag_10: bool,
    pub class_8: bool,
}

/// 82DA7A38. `hit` is Ground+208 when Ground+320 says the hips test hit, `surface` Ground+260.
pub fn ground_query(origin: V4, hit: Option<V4>, surface: u32, reference: Option<V4>) -> GroundQuery {
    let mut out = GroundQuery { origin, ..GroundQuery::default() };
    let Some(contact) = hit else {
        return out;
    };
    out.hit = true;
    out.clearance = ppc::vsubfp(ppc::splat(origin, 1), ppc::splat(contact, 1))[0];
    out.drop = match reference {
        None => out.clearance,
        Some(reference) => {
            let lower = if all_gt(ppc::splat(reference, 1), ppc::splat(origin, 1)) {
                origin
            } else {
                reference
            };
            ppc::fsubs(lower[1], contact[1])
        }
    };
    out.tag_10 = surface & 0x7F == 10;
    out.class_8 = surface & 0xF80 == 0x400;
    out
}

/// One of 82DA89A0's four run accumulators, 82DA7B50 (`+0` start, `+16` current, `+20` banked,
/// `+24` longest, `+28` count, byte `+32` running).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextRun {
    pub start: V4,
    pub current: f32,
    pub banked: f32,
    pub longest: f32,
    pub runs: u32,
    pub running: bool,
}

impl ContextRun {
    /// 82DA7B50(run, query, active, bank): a closing run is added to the total only when
    /// `bank` is set, otherwise its distance is dropped.
    pub fn advance(&mut self, position: V4, active: bool, bank: bool) {
        if active && !self.running {
            self.current = 0.0;
            self.running = true;
            self.start = position;
        }
        if !self.running {
            return;
        }
        let d = ppc::vsubfp(position, self.start);
        // vperm by 0x822FB890 against a zero vector, then vrlimi z: [x, 0, z, x].
        let travelled = length([d[0], 0.0, d[2], d[0]]);
        if active {
            self.current = travelled;
            return;
        }
        if bank {
            let longest = self.longest;
            self.runs = self.runs.wrapping_add(1);
            self.banked = ppc::fadds(self.banked, travelled);
            self.longest = ppc::fsel(ppc::fsubs(travelled, longest), travelled, longest);
        }
        self.current = 0.0;
        self.running = false;
    }

    /// 82DA88E8 sums `+20 + +16` before each curve.
    pub fn total(&self) -> f32 {
        ppc::fadds(self.banked, self.current)
    }
}

/// 82DA8FF8's horizontal distance: `[x, 0, z, w]` of the displacement.
pub fn horizontal_distance(from: V4, to: V4) -> f32 {
    let d = ppc::vsubfp(to, from);
    length([d[0], 0.0, d[2], d[3]])
}

/// 82DAC780: the hips' ground contact lies between the deck probe's origin (Ground+144) and
/// the hips (Ground+192): both vectors from it are longer than 0.1 and point opposite ways.
pub fn contact_between(deck_origin: V4, hips_origin: V4, contact: Option<V4>) -> bool {
    let Some(contact) = contact else {
        return false;
    };
    let epsilon = [f32::from_bits(0x3C23_D70A); 4]; // 0x820D71E8
    let to_deck = ppc::vsubfp(deck_origin, contact);
    let to_hips = ppc::vsubfp(hips_origin, contact);
    all_gt(ppc::vmsum3fp(to_deck, to_deck), epsilon)
        && all_gt(ppc::vmsum3fp(to_hips, to_hips), epsilon)
        && all_gt(ZERO, ppc::vmsum3fp(to_deck, to_hips))
}

/// 82DAB7D0's peak: the higher of the hips probe's and the deck probe's clearance this frame,
/// each zero when its probe missed, and the running maximum of that (`+320`).
pub fn handplant_peak(peak: f32, hips: Option<(V4, V4)>, deck: Option<(V4, V4)>) -> f32 {
    let clearance = |probe: Option<(V4, V4)>| {
        probe.map_or(0.0, |(origin, hit)| {
            ppc::vsubfp(ppc::splat(origin, 1), ppc::splat(hit, 1))[0]
        })
    };
    let (hips, deck) = (clearance(hips), clearance(deck));
    let higher = ppc::fsel(ppc::fsubs(hips, deck), hips, deck);
    ppc::fsel(ppc::fsubs(peak, higher), peak, higher)
}

/// What 82DAB2A8, the Ground collector's "does the sequence continue?" (vtable slot 5), reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct GroundHold {
    /// `r4`: the previous answer, i.e. a sequence is open.
    pub open: bool,
    /// `+116`: the four slots' returns.
    pub live: f32,
    /// `+944`: the revert slot's State70 latch.
    pub revert_latch: bool,
    /// `+104`: the MotionGraph score packet's flags (82DAC498 copies the packet), not input +2788.
    pub flags: u32,
    /// Slot `+528` (powerslide): present, its id in `(-1, 332)` and announced (`+652`, `+540`, `+716`).
    pub slide: bool,
    /// Slot `+128` (ollie / nollie): present and its id in `(-1, 332)` (`+252`, `+140`).
    pub ollie: bool,
    /// `[[owner+24]+0]`: the wheel contact count (signed compare).
    pub wheels: i32,
    /// `+928`: frames since the collector's Reset.
    pub frames: u32,
    /// `+932`: frames the manual slot was refused by State70 (82DAA8E0).
    pub manual_blocked: u32,
    /// `+945`: input +2788 bit 0x40000000 (82DA3C68), the player record's ManualsAreConnectors byte.
    pub manuals_connect: bool,
    /// `L+128`: tricks on the display.
    pub tricks: i32,
    /// `[owner+32]+268`: the pump acceleration.
    pub pump: f32,
    /// `[owner+0]+80` lane 1: the effective deck up's y.
    pub deck_up_y: f32,
}

/// 82DAB2A8. Fewer than three wheels hold an open sequence while manuals connect (+945) and the manual slot has
/// not been blocked for more than six frames; with +945 clear, only until the display holds a trick, and then for
/// the first eight frames on the ground.
pub fn ground_continues(g: &GroundHold) -> bool {
    let wheels_low = g.open && g.wheels < 3;
    let mut r7 = wheels_low;
    let mut r9 = g.open && g.frames < 3;
    if wheels_low {
        if g.manuals_connect {
            r7 = g.manual_blocked <= 6;
        } else if g.tricks > 0 {
            r7 = false;
            r9 = g.frames < 8;
        }
    }
    g.live > 0.0
        || g.revert_latch
        || g.open && g.flags & 0x3000_0000 != 0
        || g.flags & 0x0100_0000 != 0
        || g.slide
        || g.ollie
        || r7
        || r9
        || g.open && g.pump > f32::from_bits(0x3DCC_CCCD) // 0x820641A8
        || g.open && f32::from_bits(0x3F26_6666) > ppc::daz(g.deck_up_y) // 0x820BB0EC, vcmpgtfp128
}

/// One context slot: up to eight `(key, value)` pairs and the count at +64 (68 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextSlot {
    pub entries: [(u32, u32); 8],
    pub count: u32,
}

impl ContextSlot {
    /// 82DA14A0: keys are unique, and a full slot ignores the rest.
    pub fn insert(&mut self, key: u32, value: u32) {
        let n = self.count as usize;
        if self.entries[..n.min(8)].iter().any(|e| e.0 == key) || n >= 8 {
            return;
        }
        self.entries[n] = (key, value);
        self.count += 1;
    }
}

/// The ten context slots of output+13648 (82DA7888 fills them; see [`CONTEXT_SLOT_SCALARS`]).
pub type ContextBlock = [ContextSlot; 10];

/// 82DA8A70's per-frame union into the collector's `+1656`.
pub fn union(into: &mut ContextBlock, from: &ContextBlock) {
    for (to, from) in into.iter_mut().zip(from) {
        for &(key, value) in &from.entries[..from.count.min(8) as usize] {
            to.insert(key, value);
        }
    }
}

/// 82DA1570(dst, src): every key of `src` removed from the matching slot of `dst`, the last
/// entry moved into the gap. It is a difference, not a merge.
pub fn remove(dst: &mut ContextBlock, src: &ContextBlock) {
    for (dst, src) in dst.iter_mut().zip(src) {
        for &(key, _) in &src.entries[..src.count as usize] {
            let n = dst.count as usize;
            let Some(found) = dst.entries[..n].iter().position(|e| e.0 == key) else {
                continue;
            };
            dst.count -= 1;
            let last = dst.count as usize;
            if found < last {
                dst.entries[found] = dst.entries[last];
            }
        }
    }
}

/// 82DA8550's four context slots, in the order it applies them: `(slot, multiplier, bonus)`
/// tuning offsets in the collector class. Slot 4 is the offline-only "skater within 3 m".
pub const CONTEXT_SLOT_SCALARS: [(usize, u16, u16); 4] =
    [(2, 0x644, 0x648), (0, 0x654, 0x658), (4, 0x64C, 0x650), (6, 0x65C, 0x660)];

/// 82DA8550 (82DA85F4..82DA87CC): the landing multiplier for the five air metrics and the flat
/// bonuses paid as scorable 237. `present[k]` is "slot k's count is non-zero" after the air's
/// union has had the take-off and landing frames removed; `offline` is input +2788 bit
/// 0x00800000. `scalar` reads the collector class; a bonus is
/// `((scalar * [+2380]) * [+2376]) * multiplier`.
pub fn landing_context(
    present: [bool; 10],
    offline: bool,
    scale_2380: f32,
    scale_2376: f32,
    scalar: impl Fn(u16) -> f32,
) -> (f32, Vec<f32>) {
    let paid = |slot: usize| present[slot] && (slot != 4 || offline);
    // Slot 2 comes first and is a plain `lfs f11` (0x82DA860C), not a multiply.
    let mut multiplier = if paid(2) { scalar(0x644) } else { 1.0 };
    for &(slot, factor, _) in &CONTEXT_SLOT_SCALARS[1..] {
        if paid(slot) {
            multiplier = ppc::fmuls(scalar(factor), multiplier);
        }
    }
    let bonuses = CONTEXT_SLOT_SCALARS
        .iter()
        .filter(|(slot, _, _)| paid(*slot))
        .map(|&(_, _, bonus)| {
            ppc::fmuls(ppc::fmuls(ppc::fmuls(scalar(bonus), scale_2380), scale_2376), multiplier)
        })
        .collect();
    (multiplier, bonuses)
}

/// One entry of 82761100's object list (input `+1760`, 112 bytes, 82760ED0 writes it): the
/// DMO's world matrix (`+0x00`, rows right / up / forward / translation), its physics model's
/// local bounds (`+0x40` min, `+0x50` max), its id (`+0x68`, the slot key) and the Score
/// entity type 82DA6F58 looks up for it (the slot value).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextObject {
    pub matrix: Matrix,
    pub min: V4,
    pub max: V4,
    pub id: u32,
    pub entity: u32,
}

/// 82587F88's keep test: the horizontal (`[x, 0, z, x]`) distance from the player to the
/// matrix translation under 8 m (`fmuls(8, 8)` against `vmsum3fp`, strict).
fn object_candidate(position: V4, object: &ContextObject) -> bool {
    let d = ppc::vsubfp(position, object.matrix[3]);
    let flat = [d[0], 0.0, d[2], d[0]];
    all_gt([ppc::fmuls(8.0, 8.0); 4], ppc::vmsum3fp(flat, flat))
}

/// 82761100's own test (0x827612C8..0x82761378): the bounds' world centre within half their
/// diagonal plus 1 m of the player, horizontally. So "within 8 m" is only the broad phase.
fn object_close(position: V4, object: &ContextObject) -> bool {
    let size = ppc::vsubfp(object.max, object.min);
    let mid = ppc::vmaddfp(size, HALF, object.min);
    let m = &object.matrix;
    let world = ppc::vmaddfp(m[0], ppc::splat(mid, 0), m[3]);
    let world = ppc::vmaddfp(m[1], ppc::splat(mid, 1), world);
    let world = ppc::vmaddfp(m[2], ppc::splat(mid, 2), world);
    let d = ppc::vsubfp(world, position);
    let flat = [d[0], 0.0, d[2], d[0]]; // vperm 0x822FB890 against zero, then vrlimi z
    let reach = ppc::vmaddfp(HALF, [length(size); 4], ONE);
    all_gt(ppc::vmulfp(reach, reach), ppc::vmsum3fp(flat, flat))
}

/// 82761100's list (count `+2776`): at most 32 broad-phase candidates in the source's order
/// (82587F88 online, the DMO manager `[TheSimulation+0x104]`), of which at most 8 pass
/// [`object_close`]. `position` is PhysOut_Skeleton `+0x1A0`, the rider root's translation.
pub fn near_objects(
    position: V4,
    objects: impl IntoIterator<Item = ContextObject>,
) -> impl Iterator<Item = ContextObject> {
    objects
        .into_iter()
        .filter(move |o| object_candidate(position, o))
        .take(32)
        .filter(move |o| object_close(position, o))
        .take(8)
}

/// 82DA6CB0: the segment `origin + t * direction`, `t` in `[0, 1]`, against the box, one
/// axis at a time. `Some((t_enter, t_exit))` on a hit.
fn slab(origin: V4, direction: V4, min: V4, max: V4) -> Option<(f32, f32)> {
    let (mut near, mut far) = (0.0, 1.0);
    for i in 0..3 {
        let (d, o) = (direction[i], origin[i]);
        // `fcmpu`/`bge`: a NaN direction takes the parallel branch, as does |d| < 1e-6.
        if !(d.abs() >= f32::from_bits(0x3586_37BD)) {
            if o < min[i] || o > max[i] {
                return None;
            }
            continue;
        }
        let inverse = ppc::fdivs(1.0, d);
        let mut t1 = ppc::fmuls(ppc::fsubs(min[i], o), inverse);
        let mut t2 = ppc::fmuls(ppc::fsubs(max[i], o), inverse);
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        near = ppc::fsel(ppc::fsubs(near, t1), near, t1);
        far = ppc::fsel(ppc::fsubs(far, t2), t2, far);
        if near > far {
            return None;
        }
    }
    Some((near, far))
}

/// 82DA6DB8(origin, hit, entry, columns) == 1: the 10 m drop line from the probe's origin
/// passes through the box, and the probe's hit lies above the point where it leaves the box
/// (by more than 0.001), i.e. the probe hit the object. The line goes into the box's frame by
/// the transposed rows 82DA6F58 builds (`vmrghw`/`vmrglw`).
fn over_object(origin: V4, hit: V4, object: &ContextObject) -> bool {
    let m = &object.matrix;
    let columns: [V4; 3] = std::array::from_fn(|k| [m[0][k], m[1][k], m[2][k], m[3][k]]);
    let local = |p: V4| {
        let v = ppc::vmulfp(columns[0], ppc::splat(p, 0));
        let v = ppc::vmaddfp(columns[1], ppc::splat(p, 1), v);
        ppc::vmaddfp(columns[2], ppc::splat(p, 2), v)
    };
    let relative = ppc::vsubfp(origin, m[3]);
    let start = ppc::vaddfp(relative, ZERO);
    let end = ppc::vaddfp(relative, [0.0, -10.0, 0.0, 0.0]);
    let from = local(start);
    let Some((_, exit)) = slab(from, ppc::vsubfp(local(end), from), object.min, object.max) else {
        return false;
    };
    let leave = ppc::vmaddfp(ppc::vsubfp(end, start), [exit; 4], ppc::vaddfp(m[3], start));
    ppc::vsubfp(ppc::splat(hit, 1), ppc::splat(leave, 1))[0] > f32::from_bits(0x3A83_126F)
}

/// 82DA6F58's slot 6 (output `+0x198`): every listed object the probe is over, appended as
/// `(id, entity)` with no dedupe (unlike 82DA14A0), at most 8. `probe` is the hips probe's
/// origin and hit (Ground +192 / +208) while Ground +320 says it is valid. Slot 7 (its `0`
/// answer) is dead in TU3: 82DA6DB8 only returns 1 or 2.
pub fn object_slot(near: impl IntoIterator<Item = ContextObject>, probe: Option<(V4, V4)>) -> ContextSlot {
    let mut slot = ContextSlot::default();
    let Some((origin, hit)) = probe else {
        return slot;
    };
    for object in near {
        if slot.count < 8 && over_object(origin, hit, &object) {
            slot.entries[slot.count as usize] = (object.id, object.entity);
            slot.count += 1;
        }
    }
    slot
}

#[cfg(test)]
#[path = "air_oracle.rs"]
mod air_oracle;
