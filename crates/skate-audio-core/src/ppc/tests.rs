use super::*;

const CODE: u32 = 0x8200_0000;
const DATA: u32 = 0x4000_0000;
const STACK: u32 = 0x7000_0000;

/// A guest with `words` assembled at CODE, a data page and a stack.
fn machine(words: &[u32]) -> (Interp, Guest) {
    let mut code = Vec::new();
    for w in words {
        code.extend_from_slice(&w.to_be_bytes());
    }
    let mut g = Guest::from_segments(vec![
        crate::Segment { base: CODE, bytes: code },
        crate::Segment { base: DATA, bytes: vec![0; 0x1000] },
        crate::Segment { base: STACK - 0x1000, bytes: vec![0; 0x2000] },
    ]);
    g.set_u32(DATA, 0).unwrap();
    let mut i = Interp::new();
    i.cpu.r[1] = STACK as u64;
    (i, g)
}

const BLR: u32 = 0x4E80_0020;

#[test]
fn integer_arithmetic_and_return() {
    // addi r3,r3,5 ; mullw r3,r3,r4 ; blr
    let (mut i, mut g) = machine(&[0x3863_0005, 0x7C63_21D6, BLR]);
    let r = i.call(&mut g, &mut NoHost, CODE, &[2, 3]).unwrap();
    assert_eq!(r, 21);
}

#[test]
fn rlwinm_uses_the_doubled_word_for_a_wrapping_mask() {
    // rlwinm r3,r3,4,28,3 : mask wraps (mb > me)
    let w = (21 << 26) | (3 << 21) | (3 << 16) | (4 << 11) | (28 << 6) | (3 << 1);
    let (mut i, mut g) = machine(&[w, BLR]);
    let r = i.call(&mut g, &mut NoHost, CODE, &[0x1234_5678]).unwrap();
    let expected = fp::rlwinm(0x1234_5678, 4, mask64(60, 35));
    assert_eq!(r, expected);
}

#[test]
fn a_counted_loop_with_bdnz_and_stores() {
    // li r5,0 ; mtctr r4 ; loop: stwx r5,r3,r6? keep simple: addi r5,r5,1 ; bdnz loop ; mr r3,r5 ; blr
    let words = [
        0x38A0_0000,                    // li r5,0
        0x7C89_03A6,                    // mtctr r4
        0x38A5_0001,                    // addi r5,r5,1
        0x4200_FFFC,                    // bdnz -4
        0x7CA3_2B78,                    // mr r3,r5
        BLR,
    ];
    let (mut i, mut g) = machine(&words);
    assert_eq!(i.call(&mut g, &mut NoHost, CODE, &[0, 7]).unwrap(), 7);
}

#[test]
fn single_precision_arithmetic_rounds_like_the_recomp() {
    // lfs f1,0(r3) ; lfs f2,4(r3) ; fadds f3,f1,f2 ; stfs f3,8(r3) ; blr
    let words = [0xC023_0000, 0xC043_0004, 0xEC61_102A, 0xD063_0008, BLR];
    let (mut i, mut g) = machine(&words);
    g.set_u32(DATA, 0.1f32.to_bits()).unwrap();
    g.set_u32(DATA + 4, 0.2f32.to_bits()).unwrap();
    i.call(&mut g, &mut NoHost, CODE, &[DATA as u64]).unwrap();
    assert_eq!(g.u32(DATA + 8).unwrap(), fp::single_to_bits(fp::add_single(0.1f32 as f64, 0.2f32 as f64)));
}

#[test]
fn lfs_stfs_copy_denormals_and_nans_bit_exactly_under_the_flush_mode() {
    // lfs f0,0(r3) ; stfs f0,4(r3) ; blr — run inside `call`, i.e. under FZ|DAZ.
    let words = [0xC003_0000, 0xD003_0004, BLR];
    for bits in [1, 0xD4, 0x7F_FFFF, 0x8000_0001, 0x7F80_0001, 0xFFC0_1234, 0x3F80_0000, 0, 0x8000_0000] {
        let (mut i, mut g) = machine(&words);
        g.set_u32(DATA, bits).unwrap();
        i.call(&mut g, &mut NoHost, CODE, &[DATA as u64]).unwrap();
        assert_eq!(g.u32(DATA + 4).unwrap(), bits, "{bits:#x}");
    }
    assert_eq!(lfs_value(1), 2f64.powi(-149));
}

#[test]
fn byte_and_halfword_vector_ops() {
    let a = u128::from_be_bytes(std::array::from_fn(|k| 0x80 | k as u8));
    let b = u128::from_be_bytes(std::array::from_fn(|k| k as u8));
    // vslb v0,v1,v2 ; vsrab v3,v1,v2 ; vmrghb v4,v1,v2 ; vmrglh v5,v1,v2 ; blr
    let words = [0x1001_1104, 0x1061_1304, 0x1081_100C, 0x10A1_114C, BLR];
    let (mut i, mut g) = machine(&words);
    i.cpu.v[1] = a;
    i.cpu.v[2] = b;
    i.call(&mut g, &mut NoHost, CODE, &[]).unwrap();
    let bytes = |v: u128| v.to_be_bytes();
    assert_eq!(bytes(i.cpu.v[0]), std::array::from_fn(|k| (0x80 | k as u8).wrapping_shl(k as u32 & 7)));
    assert_eq!(bytes(i.cpu.v[3]), std::array::from_fn(|k| (((0x80 | k as u8) as i8) >> (k & 7)) as u8));
    assert_eq!(bytes(i.cpu.v[4]), std::array::from_fn(|k| if k % 2 == 0 { 0x80 | (k / 2) as u8 } else { (k / 2) as u8 }));
    assert_eq!(bytes(i.cpu.v[5]), [0x88, 0x89, 8, 9, 0x8A, 0x8B, 10, 11, 0x8C, 0x8D, 12, 13, 0x8E, 0x8F, 14, 15]);
}

#[test]
fn host_hooks_run_natively_and_return_to_the_caller() {
    struct Double;
    impl Host for Double {
        fn hook(&mut self, cpu: &mut Cpu, _g: &mut Guest, _target: u32) -> Result<()> {
            cpu.r[3] *= 2;
            Ok(())
        }
    }
    // mflr r12 ; stw r12,-8(r1) ; stwu r1,-16(r1) ; bl +16 (hooked) ; addi r1,r1,16 ; lwz r12,-8(r1) ; mtlr r12 ; blr ; <hooked target>
    let words = [
        0x7D88_02A6, 0x9181_FFF8, 0x9421_FFF0, 0x4800_0015, 0x3821_0010, 0x8181_FFF8, 0x7D88_03A6,
        BLR, 0x0000_0000,
    ];
    let (mut i, mut g) = machine(&words);
    i.hooks.insert(CODE + 32);
    assert_eq!(i.call(&mut g, &mut Double, CODE, &[21]).unwrap(), 42);
}

// ------------------------------------------------------------------ differential against ports

/// The TU3 audio runtime image (`g_XXXX.bin` files), from `SKATE_AUDIO_IMAGE`.
fn image_segments() -> Option<Vec<crate::Segment>> {
    let dir = std::env::var_os("SKATE_AUDIO_IMAGE")?;
    let mut segs = Vec::new();
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        if let Some(hex) = name.strip_prefix("g_").and_then(|s| s.strip_suffix(".bin")) {
            segs.push(crate::Segment {
                base: u32::from_str_radix(hex, 16).ok()? << 16,
                bytes: std::fs::read(&path).ok()?,
            });
        }
    }
    Some(segs)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    /// A word biased toward the values these ops actually see.
    fn word(&mut self) -> u32 {
        match self.next() % 10 {
            0 => 0,
            1 => self.next() % 16,
            2 => (self.next() % 64).wrapping_neg(),
            3 => (((self.next() % 4000) as f32 - 2000.0) / 1000.0).to_bits(),
            4 => ((self.next() % 65536) as f32).to_bits(),
            5 => 1.0f32.to_bits(),
            6 => self.next() % 32767,
            7 => 4 * (self.next() % 64),
            _ => self.next(),
        }
    }
}

#[test]
#[ignore = "needs the audio runtime image: SKATE_AUDIO_IMAGE=<dir of g_*.bin>"]
fn evaluator_ops_match_their_verified_ports() {
    let Some(segs) = image_segments() else {
        eprintln!("skipped: set SKATE_AUDIO_IMAGE");
        return;
    };
    const BLOCK: u32 = 0x4010_0000;
    const WINDOW: u32 = 0x1000;
    let mut a = Guest::from_segments(segs);
    a.put(BLOCK - 0x800, vec![0; 0x3000]);
    a.put(STACK - 0x8000, vec![0; 0x10000]);
    let mut b = a.clone();
    let mut interp = Interp::new();
    interp.cpu.r[1] = STACK as u64;
    interp.step_limit = 200_000;
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut failures = Vec::new();
    for (slot, entry) in crate::eval::TABLE.iter().enumerate() {
        let Some(port) = entry.port else { continue };
        if let Ok(only) = std::env::var("SLOTS") {
            if !only.split(',').any(|s| s.trim().parse() == Ok(slot)) {
                continue;
            }
        }
        let (mut same, mut upper_only, mut both_err, mut denormal, mut diff) = (0, 0, 0, 0, 0);
        let cases: usize = std::env::var("CASES").ok().and_then(|c| c.parse().ok()).unwrap_or(3000);
        for case in 0..cases {
            let mut block: Vec<u32> = (0..(WINDOW / 4)).map(|_| rng.word()).collect();
            // Slot 28's wrap loop subtracts 1.0 until the phase is below it: a NaN, infinite or
            // huge phase never gets there, in the original as in the port. Keep it finite and near.
            let phase = f32::from_bits(block[256 + 1]);
            if slot == 28 && !(phase < 1000.0) {
                block[256 + 1] = ((rng.next() % 3000) as f32 / 1000.0).to_bits();
            }
            let counter: Vec<u32> = (0..6).map(|_| rng.next()).collect();
            let inputs = |g: &mut Guest| {
                for (i, w) in block.iter().enumerate() {
                    g.set_u32(BLOCK - 0x400 + 4 * i as u32, *w).unwrap();
                }
                for (i, w) in counter.iter().enumerate() {
                    g.set_u32(crate::counter::COUNTER + 4 * i as u32, *w).unwrap();
                }
            };
            inputs(&mut a);
            inputs(&mut b);
            let run_port = |g: &mut Guest, mode: u32| {
                let csr = crate::vmx::get_mxcsr();
                crate::vmx::set_mxcsr(mode);
                let r = port(g, BLOCK);
                crate::vmx::set_mxcsr(csr);
                r
            };
            // Same FP mode as the interpreter (the recomp's FZ|DAZ on the scalar side too).
            let ra = run_port(&mut a, crate::vmx::VMX_CSR);
            let rb = interp.call(&mut b, &mut NoHost, entry.guest, &[BLOCK as u64]);
            let agrees = |a: &Guest, b: &Guest| {
                a.span(BLOCK - 0x400, WINDOW as usize).unwrap() == b.span(BLOCK - 0x400, WINDOW as usize).unwrap()
                    && a.span(crate::counter::COUNTER, 24).unwrap() == b.span(crate::counter::COUNTER, 24).unwrap()
            };
            match (&ra, &rb) {
                (Err(_), Err(_)) => both_err += 1,
                (Ok(x), Ok(y)) => {
                    let mem_same = agrees(&a, &b);
                    if mem_same && x == y {
                        same += 1;
                    } else if mem_same && (*x as u32) == (*y as u32) {
                        upper_only += 1;
                    } else if {
                        // Under FZ|DAZ the two sides flush in different places: the interpreter's
                        // lfs/stfs keep denormals (hardware) and its arithmetic flushes; the port's
                        // flushes depend on which conversions LLVM folds. Where both, rerun with
                        // no flush at all, agree, the gap is that policy, not a transcription error.
                        let mut b2 = b.clone();
                        inputs(&mut a);
                        inputs(&mut b2);
                        interp.mxcsr = 0x1F80;
                        let ry = interp.call(&mut b2, &mut NoHost, entry.guest, &[BLOCK as u64]);
                        interp.mxcsr = crate::vmx::VMX_CSR;
                        run_port(&mut a, 0x1F80).ok() == ry.ok() && agrees(&a, &b2)
                    } {
                        denormal += 1;
                    } else {
                        diff += 1;
                        if diff <= 2 {
                            let (ma, mb) = (
                                a.span(BLOCK - 0x400, WINDOW as usize).unwrap(),
                                b.span(BLOCK - 0x400, WINDOW as usize).unwrap(),
                            );
                            let at = (0..ma.len()).step_by(4).find(|&i| ma[i..i + 4] != mb[i..i + 4]);
                            let word = |m: &[u8], i: usize| u32::from_be_bytes(m[i..i + 4].try_into().unwrap());
                            let f = format!(
                                "slot {slot} {} case {case}: port r3={x:#x} interp r3={y:#x} in={:08x?} first diff {}",
                                entry.name,
                                &block[256..264],
                                at.map(|i| format!("+{:#x}: port {:08x} interp {:08x}", i as i64 - 0x400, word(ma, i), word(mb, i)))
                                    .unwrap_or_else(|| "in the counter".into())
                            );
                            eprintln!("{f}");
                            failures.push(f);
                        }
                    }
                }
                _ => {
                    diff += 1;
                    if diff <= 2 {
                        let f = format!(
                            "slot {slot} {} case {case}: port {:?} interp {:?}",
                            entry.name,
                            ra.as_ref().map(|v| format!("{v:#x}")).map_err(|e| e.to_string()),
                            rb.as_ref().map(|v| format!("{v:#x}")).map_err(|e| e.to_string())
                        );
                        eprintln!("{f}");
                        failures.push(f);
                    }
                }
            }
            // Keep the two guests identical for the next case whatever happened.
            let snapshot = a.span(BLOCK - 0x800, 0x3000).unwrap().to_vec();
            b.set_span(BLOCK - 0x800, &snapshot).unwrap();
        }
        eprintln!(
            "slot {slot:2} {:14} same {same:5} upper-bits-only {upper_only:5} both-error {both_err:5} denormal-policy {denormal:3} DIFFER {diff}",
            entry.name
        );
    }
    assert!(failures.is_empty(), "{} slot(s) diverged", failures.len());
}

#[test]
#[ignore = "needs the audio runtime image: SKATE_AUDIO_IMAGE=<dir of g_*.bin>"]
fn counter_advance_matches_its_verified_port() {
    let Some(segs) = image_segments() else {
        eprintln!("skipped: set SKATE_AUDIO_IMAGE");
        return;
    };
    use crate::counter::{COUNTER, advance};
    let mut a = Guest::from_segments(segs);
    a.put(STACK - 0x8000, vec![0; 0x10000]);
    let mut b = a.clone();
    let mut interp = Interp::new();
    interp.cpu.r[1] = STACK as u64;
    interp.step_limit = 10_000;
    let mut rng = Rng(0x0DDB_A11C_AFE5_1234);
    for case in 0..100_000 {
        // Carry chains need all-ones and all-zero words, not just random ones.
        let state: Vec<u32> = (0..6)
            .map(|_| match rng.next() % 4 {
                0 => u32::MAX,
                1 => 0,
                2 => u32::MAX - rng.next() % 4,
                _ => rng.next(),
            })
            .collect();
        for g in [&mut a, &mut b] {
            for (i, w) in state.iter().enumerate() {
                g.set_u32(COUNTER + 4 * i as u32, *w).unwrap();
            }
        }
        let x = advance(&mut a).unwrap();
        let y = interp.call(&mut b, &mut NoHost, 0x82B1_F360, &[]).unwrap();
        assert_eq!(
            (x, a.span(COUNTER, 24).unwrap()),
            (y, b.span(COUNTER, 24).unwrap()),
            "case {case} state {state:08x?}"
        );
    }
}

// ------------------------------------------------------------ DSP kernels (the VMX128 side)

const KBASE: u32 = 0x4020_0000;
const KSIZE: u32 = 0x10000;
const A: u32 = KBASE;
const B: u32 = KBASE + 0x1000;
const C: u32 = KBASE + 0x2000;
const D: u32 = KBASE + 0x3000;
const E: u32 = KBASE + 0x4000; // small state records
const TABLE: u32 = KBASE + 0x5000;
const OUT: u32 = KBASE + 0x8000;

/// Registers r3..r10, f1..f2, and word pokes over the random fill.
type KernelArgs = ([u64; 8], [f64; 2], Vec<(u32, u32)>);
struct Kernel {
    name: &'static str,
    guest: u32,
    args: fn(&mut Rng) -> KernelArgs,
    /// The port; `Some(r3)` when r3 is a result worth comparing.
    port: fn(&mut Guest, &[u64; 8], &[f64; 2]) -> Result<Option<u64>>,
}

impl Rng {
    fn sample(&mut self) -> f32 {
        ((self.next() % 40001) as f32 - 20000.0) / 10000.0
    }
    fn gain(&mut self) -> f64 {
        match self.next() % 4 {
            0 => 0.0,
            1 => 1.0,
            _ => self.sample() as f64,
        }
    }
    /// Mostly the mixer's aligned buffers, sometimes not.
    fn skew(&mut self) -> u32 {
        match self.next() % 4 {
            0 => 4 * (self.next() % 4),
            1 => 16 * (self.next() % 8),
            _ => 0,
        }
    }
}

fn regs(r: &[u64]) -> [u64; 8] {
    let mut out = [0; 8];
    out[..r.len()].copy_from_slice(r);
    out
}

fn scale_gen(g: &mut Rng) -> KernelArgs {
    let count = if g.next() % 3 == 0 { g.next() % 520 } else { 256 };
    (regs(&[(B + g.skew()) as u64, (A + g.skew()) as u64, 0, count as u64]), [g.gain(), 0.0], vec![])
}

fn ramp_gen(g: &mut Rng) -> KernelArgs {
    let first = (g.next() % 600) as i32 - 300;
    let length = 1 + g.next() % 600;
    (
        regs(&[OUT as u64, 0, 0, first as i64 as u64, length as u64]),
        [g.sample() as f64, g.sample() as f64 * 2.0],
        vec![],
    )
}

fn gain_ramp_gen(g: &mut Rng) -> KernelArgs {
    let step = if g.next() % 3 == 0 { 0.0 } else { g.sample() as f64 / 64.0 };
    (regs(&[(B + g.skew()) as u64, (A + g.skew()) as u64]), [g.gain(), step], vec![])
}

fn kernels() -> Vec<Kernel> {
    use crate::dsp;
    vec![
        Kernel {
            name: "scale",
            guest: 0x82B3_BED8,
            args: scale_gen,
            port: |g, r, f| dsp::scale::scale(g, r[0] as u32, r[1] as u32, r[3] as u32, f[0]).map(|()| None),
        },
        Kernel {
            name: "scale_accumulate",
            guest: 0x82B4_4B20,
            args: scale_gen,
            port: |g, r, f| {
                dsp::scale::scale_accumulate(g, r[0] as u32, r[1] as u32, r[3] as u32, f[0]).map(|()| None)
            },
        },
        Kernel {
            name: "biquad",
            guest: 0x82B4_3AF8,
            args: |g| {
                let count = 8 * (1 + g.next() % 40);
                let pokes = (0..5).map(|i| (E + 0x40 + 4 * i, (g.sample() / 2.0).to_bits())).collect();
                (regs(&[E as u64, B as u64, A as u64, (E + 0x40) as u64, count as u64]), [0.0; 2], pokes)
            },
            port: |g, r, _| {
                dsp::biquad::biquad(g, r[0] as u32, r[1] as u32, r[2] as u32, r[3] as u32, r[4] as u32)
                    .map(|()| None)
            },
        },
        Kernel {
            name: "resample",
            guest: 0x82B4_3FB8,
            args: |g| {
                let count = g.next() % 257;
                let step = (g.next() % 0x30000) as u64;
                let pokes = vec![(E + 0x80, g.next() % 64), (E + 0x84, g.next())];
                (
                    regs(&[count as u64, TABLE as u64, B as u64, (E + 0x80) as u64, (E + 0x84) as u64, step]),
                    [0.0; 2],
                    pokes,
                )
            },
            port: |g, r, _| {
                dsp::resample::resample(g, r[0] as u32, r[1] as u32, r[2] as u32, r[3] as u32, r[4] as u32, r[5])
                    .map(|()| None)
            },
        },
        Kernel {
            name: "scale_add_with_copy",
            guest: 0x82B3_CF58,
            args: |g| {
                let n = (g.next() % 300) as i32 - 4;
                (
                    regs(&[n as i64 as u64, 0, A as u64, (C + g.skew()) as u64, B as u64, D as u64]),
                    [g.gain(), 0.0],
                    vec![],
                )
            },
            port: |g, r, f| {
                dsp::scale_add::scale_add_with_copy(
                    g,
                    r[0] as u32,
                    r[2] as u32,
                    r[3] as u32,
                    r[4] as u32,
                    r[5] as u32,
                    f[0],
                )
                .map(|()| None)
            },
        },
        Kernel {
            name: "gain_ramp_copy",
            guest: 0x82B3_C098,
            args: gain_ramp_gen,
            port: |g, r, f| dsp::gain_ramp::gain_ramp_copy(g, r[0] as u32, r[1] as u32, f[0], f[1]).map(|_| None),
        },
        Kernel {
            name: "gain_ramp_accumulate",
            guest: 0x82B4_4D18,
            args: gain_ramp_gen,
            port: |g, r, f| {
                dsp::gain_ramp::gain_ramp_accumulate(g, r[0] as u32, r[1] as u32, f[0], f[1]).map(|_| None)
            },
        },
        Kernel {
            name: "linear_ramp",
            guest: 0x82B4_27D8,
            args: ramp_gen,
            port: |g, r, f| dsp::ramps::linear_ramp(g, r[0] as u32, r[3] as i32, r[4] as i32, f[0], f[1]).map(Some),
        },
        Kernel {
            name: "sqrt_ramp",
            guest: 0x82B4_2C98,
            args: ramp_gen,
            port: |g, r, f| dsp::ramps::sqrt_ramp(g, r[0] as u32, r[3] as i32, r[4] as i32, f[0], f[1]).map(Some),
        },
        Kernel {
            name: "hard_clip",
            guest: 0x82B2_2678,
            args: |g| {
                use crate::dsp::clip::*;
                let (state, pair, src, dst) = (E + 0x100, E + 0x200, E + 0x300, E + 0x340);
                let pokes = vec![
                    (state + CHANNEL_COUNT - 3, g.next() % 7), // the byte at +42 is this word's low byte
                    (state + CLIP_LEVEL, (g.sample().abs() + 0.01).to_bits()),
                    (pair + SOURCE_BUFFER, src),
                    (pair + DEST_BUFFER, dst),
                    (src + BUFFER_DATA, KBASE + 0x9000),
                    (dst + BUFFER_DATA, KBASE + 0xC000),
                    (src + BUFFER_STRIDE - 2, 256), // the halfword at +14 is this word's low half
                    (dst + BUFFER_STRIDE - 2, 256),
                ];
                (regs(&[state as u64, pair as u64]), [0.0; 2], pokes)
            },
            port: |g, r, _| dsp::clip::hard_clip(g, r[0] as u32, r[1] as u32).map(Some),
        },
        Kernel {
            name: "allpass_stage",
            guest: 0x82B3_89A0,
            args: |g| {
                // A primed call with n < 0 runs 2^25 `dcbt` prefetches (no architectural effect,
                // which is why the port skips them) before the same early exit: past any budget.
                let primed = g.next() % 2;
                let n = (g.next() % 300) as i32 - if primed == 1 { 0 } else { 4 };
                let t = C + 4 * (g.next() % 64);
                (
                    regs(&[n as i64 as u64, 0, 0, A as u64, t as u64, B as u64, D as u64, primed as u64]),
                    [g.sample() as f64 / 2.0, g.gain()],
                    vec![],
                )
            },
            port: |g, r, f| {
                dsp::allpass::allpass_stage(g, r[0] as i32, r[3], r[4] as u32, r[5] as u32, r[6], r[7] as i32, f[0], f[1])
                    .map(|x| Some(x.r3))
            },
        },
    ]
}

#[test]
#[ignore = "needs the audio runtime image: SKATE_AUDIO_IMAGE=<dir of g_*.bin>"]
fn dsp_kernels_match_their_verified_ports() {
    let Some(segs) = image_segments() else {
        eprintln!("skipped: set SKATE_AUDIO_IMAGE");
        return;
    };
    let mut a = Guest::from_segments(segs);
    a.put(KBASE, vec![0; KSIZE as usize]);
    a.put(STACK - 0x8000, vec![0; 0x10000]);
    let mut b = a.clone();
    let mut interp = Interp::new();
    interp.cpu.r[1] = STACK as u64;
    interp.step_limit = 2_000_000;
    let mut rng = Rng(0xF00D_CAFE_1357_9BDF);
    let cases: usize = std::env::var("CASES").ok().and_then(|c| c.parse().ok()).unwrap_or(300);
    let only = std::env::var("KERNELS").ok();
    let mut failures = 0;
    for k in kernels() {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|n| n == k.name)) {
            continue;
        }
        let (mut same, mut both_err, mut denormal, mut diff) = (0, 0, 0, 0);
        for case in 0..cases {
            let fill: Vec<u8> = (0..KSIZE / 4).flat_map(|_| rng.sample().to_bits().to_be_bytes()).collect();
            let (r, f, pokes) = (k.args)(&mut rng);
            let inputs = |g: &mut Guest| {
                g.set_span(KBASE, &fill).unwrap();
                for &(at, w) in &pokes {
                    g.set_u32(at, w).unwrap();
                }
            };
            let run = |ga: &mut Guest, gb: &mut Guest, interp: &mut Interp, mode: u32| {
                inputs(ga);
                inputs(gb);
                let csr = crate::vmx::get_mxcsr();
                crate::vmx::set_mxcsr(mode);
                let x = (k.port)(ga, &r, &f);
                crate::vmx::set_mxcsr(csr);
                interp.mxcsr = mode;
                interp.cpu.f[1] = f[0];
                interp.cpu.f[2] = f[1];
                let y = interp.call(gb, &mut NoHost, k.guest, &r);
                interp.mxcsr = crate::vmx::VMX_CSR;
                (x, y)
            };
            let agrees = |x: &Result<Option<u64>>, y: &Result<u64>, ga: &Guest, gb: &Guest| match (x, y) {
                (Ok(x), Ok(y)) => {
                    x.is_none_or(|x| x == *y)
                        && ga.span(KBASE, KSIZE as usize).unwrap() == gb.span(KBASE, KSIZE as usize).unwrap()
                }
                _ => false,
            };
            let (x, y) = run(&mut a, &mut b, &mut interp, crate::vmx::VMX_CSR);
            if x.is_err() && y.is_err() {
                both_err += 1;
            } else if agrees(&x, &y, &a, &b) {
                same += 1;
            } else if {
                let (x2, y2) = run(&mut a, &mut b, &mut interp, 0x1F80);
                agrees(&x2, &y2, &a, &b)
            } {
                denormal += 1;
            } else {
                diff += 1;
                if diff <= 3 {
                    let (x, y) = run(&mut a, &mut b, &mut interp, crate::vmx::VMX_CSR);
                    let (ma, mb) = (a.span(KBASE, KSIZE as usize).unwrap(), b.span(KBASE, KSIZE as usize).unwrap());
                    let diffs: Vec<u32> =
                        (0..ma.len()).step_by(4).filter(|&i| ma[i..i + 4] != mb[i..i + 4]).map(|i| i as u32).collect();
                    let word = |m: &[u8], i: u32| u32::from_be_bytes(m[i as usize..i as usize + 4].try_into().unwrap());
                    eprintln!(
                        "{} case {case}: r={r:x?} f={f:?} port {:x?} interp {:x?}; {} words differ, first {}",
                        k.name,
                        x.map_err(|e| e.to_string()),
                        y.map_err(|e| e.to_string()),
                        diffs.len(),
                        diffs
                            .iter()
                            .take(4)
                            .map(|&i| format!("+{i:#x} {:08x}/{:08x}", word(ma, i), word(mb, i)))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
            }
        }
        eprintln!("{:22} same {same:4} both-error {both_err:4} denormal-policy {denormal:3} DIFFER {diff}", k.name);
        failures += diff;
    }
    assert_eq!(failures, 0);
}
