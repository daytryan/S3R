//! Native solver records with host indices replacing guest reaction pointers.
use super::{Emulated, Fused, Native, contact, drive, joint};

#[derive(Clone, Debug)]
pub struct Constraint<const WORDS: usize> {
    pub words: [u32; WORDS],
    pub reaction_a: usize,
    pub reaction_b: usize,
}

pub type Contact = Constraint<64>;
pub type Joint = Constraint<96>;
pub type Drive = Constraint<96>;
pub type Reaction = [u32; 16];

/// Complete contact → joint → drive loops of HorusPipeline (82AE27D0).
/// Contact position corrections remain distinct from velocity-producing
/// reactions; all families see preceding writes during the same iteration.
#[allow(unsafe_code)]
pub fn solve(
    contacts: &mut [Contact],
    joints: &mut [Joint],
    drives: &mut [Drive],
    reactions: &mut [Reaction],
    iterations: u32,
) {
    validate(contacts, reactions.len());
    validate(joints, reactions.len());
    validate(drives, reactions.len());
    #[cfg(target_arch = "x86_64")]
    if *NATIVE_FMA {
        // SAFETY: the CPU supports FMA (NATIVE_FMA checked it).
        unsafe {
            iterate_fma(contacts, joints, drives, reactions, iterations)
        };
    } else {
        iterate::<Emulated>(contacts, joints, drives, reactions, iterations);
    }
    #[cfg(not(target_arch = "x86_64"))]
    iterate::<Native>(contacts, joints, drives, reactions, iterations);
}

/// Whether the solver runs its FMA build: the CPU has FMA, and `SKATE_NO_FMA` is unset (set, it stands in for a CPU
/// without one, to measure that path).
#[cfg(target_arch = "x86_64")]
static NATIVE_FMA: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::is_x86_feature_detected!("fma") && std::env::var_os("SKATE_NO_FMA").is_none());

/// The same loop with FMA instructions. The kernels' `mul_add`s otherwise compile to calls into the C runtime's
/// `fmaf` (the x86-64 baseline has no FMA), which was most of the physics tick, and on a CPU without FMA its software
/// fallback is slower still: there the loop runs [`Emulated`]. Every path rounds once, so the results are
/// bit-identical (test below).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
fn iterate_fma(
    contacts: &mut [Contact],
    joints: &mut [Joint],
    drives: &mut [Drive],
    reactions: &mut [Reaction],
    iterations: u32,
) {
    iterate::<Native>(contacts, joints, drives, reactions, iterations);
}

#[inline(always)]
fn iterate<F: Fused>(
    contacts: &mut [Contact],
    joints: &mut [Joint],
    drives: &mut [Drive],
    reactions: &mut [Reaction],
    iterations: u32,
) {
    // Direct calls, not a kernel parameter: a fn item passed as `impl Fn` goes through a call shim that LLVM
    // does not inline, and the shim is compiled without FMA.
    for _ in 0..iterations {
        for c in contacts.iter_mut() {
            let (a, b) = pair(reactions, c.reaction_a, c.reaction_b);
            contact::solve::<F>(&mut c.words, a, b);
        }
        for c in joints.iter_mut() {
            let (a, b) = pair(reactions, c.reaction_a, c.reaction_b);
            joint::solve::<F>(&mut c.words, a, b);
        }
        for c in drives.iter_mut() {
            let (a, b) = pair(reactions, c.reaction_a, c.reaction_b);
            drive::solve::<F>(&mut c.words, a, b);
        }
    }
}

fn validate<const N: usize>(constraints: &[Constraint<N>], count: usize) {
    for c in constraints {
        assert_ne!(c.reaction_a, c.reaction_b);
        assert!(c.reaction_a < count && c.reaction_b < count);
    }
}

#[inline(always)]
fn pair(reactions: &mut [Reaction], a: usize, b: usize) -> (&mut [u32], &mut [u32]) {
    if a < b {
        let (left, right) = reactions.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = reactions.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    #[allow(unsafe_code)]
    fn fma_loop_matches_the_portable_loop_bit_for_bit() {
        if !std::is_x86_feature_detected!("fma") {
            return;
        }
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut value = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 40) as f32 / (1u32 << 24) as f32 - 0.5).to_bits()
        };
        let reactions: Vec<Reaction> = (0..12).map(|_| core::array::from_fn(|_| value())).collect();
        fn rows<const N: usize>(count: usize, value: &mut impl FnMut() -> u32) -> Vec<Constraint<N>> {
            (0..count)
                .map(|i| Constraint { words: core::array::from_fn(|_| value()), reaction_a: i % 12, reaction_b: (i * 5 + 1) % 12 })
                .filter(|c| c.reaction_a != c.reaction_b)
                .collect()
        }
        let (contacts, joints, drives) = (rows::<64>(40, &mut value), rows::<96>(30, &mut value), rows::<96>(30, &mut value));
        let run = |fma: bool| {
            let (mut c, mut j, mut d, mut r) = (contacts.clone(), joints.clone(), drives.clone(), reactions.clone());
            if fma {
                // SAFETY: FMA support was checked at the top of the test.
                unsafe {
                    iterate_fma(&mut c, &mut j, &mut d, &mut r, 6)
                };
            } else {
                iterate::<Emulated>(&mut c, &mut j, &mut d, &mut r, 6);
            }
            let words = |v: &[u32]| v.to_vec();
            (
                c.iter().flat_map(|c| words(&c.words)).collect::<Vec<_>>(),
                j.iter().chain(&d).flat_map(|c| words(&c.words)).collect::<Vec<_>>(),
                r.concat(),
            )
        };
        let (portable, fused) = (run(false), run(true));
        assert!(portable.2.iter().any(|&w| w != 0), "the loop ran");
        assert_eq!(portable, fused);
    }

    /// Speed of the two paths over one tick's worth of rows (~100 rows, 25 iterations): `--ignored --nocapture`.
    #[test]
    #[ignore = "timing"]
    #[allow(unsafe_code)]
    fn the_emulated_loop_costs() {
        if !std::is_x86_feature_detected!("fma") {
            return;
        }
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut value = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 40) as f32 / (1u32 << 24) as f32 - 0.5).to_bits()
        };
        let reactions: Vec<Reaction> = (0..12).map(|_| core::array::from_fn(|_| value())).collect();
        fn rows<const N: usize>(count: usize, value: &mut impl FnMut() -> u32) -> Vec<Constraint<N>> {
            (0..count)
                .map(|i| Constraint { words: core::array::from_fn(|_| value()), reaction_a: i % 12, reaction_b: (i * 5 + 1) % 12 })
                .filter(|c| c.reaction_a != c.reaction_b)
                .collect()
        }
        let (contacts, joints, drives) = (rows::<64>(60, &mut value), rows::<96>(20, &mut value), rows::<96>(20, &mut value));
        let time = |fma: bool| {
            let start = std::time::Instant::now();
            for _ in 0..200 {
                let (mut c, mut j, mut d, mut r) = (contacts.clone(), joints.clone(), drives.clone(), reactions.clone());
                if fma {
                    // SAFETY: FMA support was checked at the top of the test.
                    unsafe { iterate_fma(&mut c, &mut j, &mut d, &mut r, 25) };
                } else {
                    iterate::<Emulated>(&mut c, &mut j, &mut d, &mut r, 25);
                }
                std::hint::black_box(&r);
            }
            start.elapsed().as_secs_f64() * 1e6 / 200.
        };
        let (fused, emulated) = (time(true), time(false));
        println!("SOLVER_COST fma {fused:.1} us/tick, emulated {emulated:.1} us/tick");
    }
}
