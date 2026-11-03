//! `update` against TU3 `0x82DEFF38` itself, run in the PowerPC interpreter (see
//! `physics::tu3_oracle`). Run with
//! `SKATE3_ASSET_ROOT=<install>/assets cargo test -p skate-core turn_conditioner_oracle -- --ignored`.
use super::*;
use crate::physics::tu3_oracle::{self, Tu3};

const CONDITIONER: u32 = 0x82DE_FF38;
/// `*(0x830CFDA4)` -> `+204` -> `+4` is the settings layout the conditioner reads.
const TWEAKABLES: u32 = 0x830C_FDA4;
const PARAMETERS: [u32; 13] = [768, 868, 872, 896, 900, 904, 908, 912, 924, 928, 932, 936, 940];
const FILTERS: [u32; 3] = [216, 180, 144];

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
    fn curve<const N: usize>(&mut self, x_hi: f32, y_lo: f32, y_hi: f32) -> PointGraph<N> {
        let mut x = [0.0; N];
        for i in 1..N {
            x[i] = x[i - 1] + self.range(0.0, x_hi / N as f32);
        }
        PointGraph { x, y: std::array::from_fn(|_| self.range(y_lo, y_hi)) }
    }
}

struct Harness {
    o: Tu3,
    this: u32,
    layout: u32,
    body: u32,
    field32: u32,
    field36: u32,
    animation: u32,
}

fn harness() -> Harness {
    let mut o = tu3_oracle::load().expect("set SKATE3_ASSET_ROOT to an installation's assets");
    assert_eq!(o.u32(CONDITIONER), 0x7D88_02A6, "0x82DEFF38 is not `mflr r12`: not the TU3 image");
    let layout = o.alloc(1024);
    let table = o.alloc(256);
    let slot = o.alloc(16);
    o.set_u32(TWEAKABLES, table);
    o.set_u32(table + 204, slot);
    o.set_u32(slot + 4, layout);
    let this = o.alloc(256);
    let bundle = o.alloc(64);
    let (body, field32, field36, animation) = (o.alloc(256), o.alloc(512), o.alloc(256), o.alloc(256));
    o.set_u32(this + 4, bundle);
    o.set_u32(bundle + 4, body);
    o.set_u32(bundle + 32, field32);
    o.set_u32(bundle + 36, field36);
    o.set_u32(bundle + 56, animation);
    Harness { o, this, layout, body, field32, field36, animation }
}

impl Harness {
    fn put_settings(&mut self, s: &Settings) {
        let l = self.layout;
        let mut put = |at: u32, v: &[f32]| {
            for (i, v) in v.iter().enumerate() {
                self.o.set_f32(l + at + 4 * i as u32, *v);
            }
        };
        put(304, &s.input_curve.x);
        put(336, &s.input_curve.y);
        put(240, &s.quickness_curve.x);
        put(272, &s.quickness_curve.y);
        put(32, &s.speed_curve.x);
        put(64, &s.speed_curve.y);
        put(384, &s.smoothing_curve.x);
        put(400, &s.smoothing_curve.y);
        for (filter, at) in [688, 704, 720].into_iter().enumerate() {
            put(at, &s.filter_coefficients[filter]);
        }
        for (p, at) in PARAMETERS.iter().enumerate() {
            put(*at, &[s.parameters[p]]);
        }
    }
    fn put_state(&mut self, s: &State) {
        for (i, v) in s.history.iter().enumerate() {
            self.o.set_f32(self.this + 112 + 4 * i as u32, *v);
        }
        for (filter, at) in FILTERS.iter().enumerate() {
            for (i, v) in s.filters[filter].iter().enumerate() {
                self.o.set_f32(self.this + at + 4 * i as u32, *v);
            }
        }
    }
    fn state(&self) -> State {
        State {
            history: std::array::from_fn(|i| self.o.f32(self.this + 112 + 4 * i as u32)),
            filters: std::array::from_fn(|f| {
                std::array::from_fn(|i| self.o.f32(self.this + FILTERS[f] + 4 * i as u32))
            }),
        }
    }
    fn run(&mut self, i: Input) -> Output {
        self.o.set_f32(self.body + 160, i.body_160);
        self.o.set_f32(self.body + 176, i.body_176);
        self.o.set_f32(self.field36 + 160, i.bundle_36_field_160);
        self.o.set_f32(self.field32 + 264, i.bundle_32_field_264);
        self.o.set_u8(self.animation + 152, i.animation_152);
        self.o.set_u8(self.animation + 156, i.animation_156);
        self.o.call(CONDITIONER, &[self.this]);
        std::array::from_fn(|k| self.o.f32(self.animation + 32 + 4 * k as u32))
    }
}

fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn assert_same(what: &str, step: usize, retail: &[f32], port: &[f32]) {
    for (k, (r, p)) in retail.iter().zip(port).enumerate() {
        assert!(
            same(*r, *p),
            "step {step} {what}[{k}]: retail {r:e} ({:08X}) port {p:e} ({:08X})",
            r.to_bits(),
            p.to_bits()
        );
    }
}

fn random_settings(r: &mut Rng) -> Settings {
    let mut parameters: [f32; 13] = std::array::from_fn(|_| r.range(0.01, 1.0));
    // Divisors (868, 932, 936, 940) and the clamp rate stay away from zero like the stock data.
    for p in [1, 10, 11, 12] {
        parameters[p] = r.range(0.2, 10.0);
    }
    Settings {
        filter_coefficients: std::array::from_fn(|_| std::array::from_fn(|_| r.range(0.0, 1.0))),
        input_curve: r.curve(1.2, -1.0, 2.0),
        quickness_curve: r.curve(1.2, -0.5, 1.5),
        speed_curve: r.curve(15.0, 0.0, 1.0),
        smoothing_curve: r.curve(3.0, 0.0, 1.0),
        parameters,
    }
}

fn random_input(r: &mut Rng) -> Input {
    let value = |r: &mut Rng, lo: f32, hi: f32| match (r.unit() * 16.0) as u32 {
        0 => 0.0,
        1 => -0.0,
        _ => r.range(lo, hi),
    };
    Input {
        body_160: value(r, -1.0, 20.0),
        body_176: value(r, 0.0, 3.0),
        bundle_36_field_160: value(r, -2.0, 2.0),
        bundle_32_field_264: value(r, -1.0, 1.0),
        animation_152: u8::from(r.unit() < 0.5),
        animation_156: u8::from(r.unit() < 0.3),
    }
}

/// Persistent filters and histories over many calls, settings re-randomised per sequence.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn turn_conditioner_oracle_random_sequences_match_82deff38() {
    let mut h = harness();
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for sequence in 0..40 {
        let settings = random_settings(&mut r);
        h.put_settings(&settings);
        let mut state = State { history: [0.0; 8], filters: [[0.0; 9]; 3] };
        h.put_state(&state);
        for step in 0..200 {
            let input = random_input(&mut r);
            let retail = h.run(input);
            let port = update(&mut state, input, &settings);
            let at = sequence * 1000 + step;
            assert_same("output", at, &retail, &port);
            let rs = h.state();
            assert_same("history", at, &rs.history, &state.history);
            for f in 0..3 {
                assert_same("filter", at, &rs.filters[f], &state.filters[f]);
            }
        }
    }
}

fn single(state: State, settings: Settings, input: Input) -> (Output, Output) {
    let mut h = harness();
    h.put_settings(&settings);
    h.put_state(&state);
    let retail = h.run(input);
    let mut port_state = state;
    (retail, update(&mut port_state, input, &settings))
}

/// `fnmsubs f9 = -((1 - p924) * body176 - 1)` at 0x82DF0030 is `-0` when the product is exactly
/// 1; with a `-0` base the conditioned target keeps that sign (PhysOut_Animation+36).
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn turn_conditioner_oracle_keeps_the_fnmsubs_negative_zero() {
    let mut settings = random_settings(&mut Rng(7));
    settings.parameters[8] = 0.5;
    let input = Input {
        body_160: 3.0,
        body_176: 2.0,
        bundle_36_field_160: 0.25,
        bundle_32_field_264: 0.0,
        animation_152: 0,
        animation_156: 1,
    };
    settings.input_curve.y = [0.5; 8];
    let (retail, port) = single(State { history: [0.0; 8], filters: [[0.0; 9]; 3] }, settings, input);
    assert_same("output", 0, &retail, &port);
}

/// `fcmpu |x|,0.01 ; ble` at 0x82DF00C0/0x82DF017C: `ble` is "branch if not greater", so it is
/// also taken on unordered and a NaN filter result becomes zero, as the port's `> epsilon` does.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image)"]
fn turn_conditioner_oracle_keeps_a_nan_filter_result() {
    let settings = random_settings(&mut Rng(11));
    let mut state = State { history: [0.0; 8], filters: [[0.0; 9]; 3] };
    state.filters[0][4] = f32::NAN;
    state.filters[1][4] = f32::NAN;
    let input = Input {
        body_160: 3.0,
        body_176: 0.5,
        bundle_36_field_160: 0.25,
        bundle_32_field_264: 0.5,
        animation_152: 1,
        animation_156: 0,
    };
    let (retail, port) = single(state, settings, input);
    assert_same("output", 0, &retail, &port);
}
