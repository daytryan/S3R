//! A Xenon PowerPC interpreter over [`Guest`] memory: the title's own game-side audio code, run
//! instruction by instruction instead of transcribed.
//!
//! The rest of this crate ports functions by hand and checks each one against the recomp. That is
//! the right trade for the audio thread's hot kernels, but the game-side logic around them — bus
//! builders, class constructors, SFX components, managers — is large, runs once per frame or once
//! at boot, and is exactly where transcription errors hide. Running the original instructions makes
//! those parts exact by construction.
//!
//! ## Semantics
//!
//! The reference is the same one the rest of the crate uses: the recomp (RexGlue) as
//! `docs/vmx128-exactness.md` and [`crate::fp`] describe it, not an idealised Xenon.
//!
//! - **Registers.** GPRs are 64-bit. Effective addresses are the low 32 bits. Word operations
//!   leave the full 64-bit result the way RexGlue's `add`/`mullw` do; record forms of word
//!   operations set CR0 from the low word, doubleword forms from all 64 bits; XER[CA] is computed on
//!   the low words (RexGlue's `subfc`/`subfe` chains; see [`crate::counter`]).
//! - **Floating point.** MXCSR is set to flush-to-zero, denormals-are-zero, all exceptions masked
//!   and round-to-nearest for the whole call ([`vmx::VMX_CSR`]), as RexGlue runs both the scalar
//!   and the vector side. The scalar single-precision forms round through [`fp`]; the vector
//!   float operations call the [`vmx`] lowerings, which are bit-checked against the recomp.
//! - **Vector registers** hold the RexGlue lane layout: the 128-bit value is the guest's sixteen
//!   bytes read big-endian, so guest element 0 is bits 127..96 and SSE lane 0 is guest element 3.
//! - **Traps** (`tw`, `twi`, `td`, `tdi`) warn and continue, as RexGlue's type-0 `ppc_trap` does.
//!
//! ## Host functions
//!
//! Some guest functions are not code this process can run: allocators behind vtables, kernel
//! imports, locks. [`Host::hook`] is asked about every taken branch whose target is in
//! [`Interp::hooks`]; it runs the function natively, sets the result registers, and the
//! interpreter resumes at LR. Anything the interpreter cannot decode or reach is an [`Error`] naming
//! the address, never a silent skip.

use std::arch::x86_64::*;
use std::collections::HashSet;

/// The hook set's hasher: guest addresses are word-aligned and already well spread, so one
/// multiply stands in for SipHash on every taken branch.
#[derive(Clone, Copy, Default)]
pub struct AddressHasher(u64);

impl std::hash::Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8 | u64::from(b)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

pub type Hooks = HashSet<u32, std::hash::BuildHasherDefault<AddressHasher>>;

use crate::{Error, Guest, Result, fp, vmx};

/// The link-register value that ends the outermost [`Interp::call`].
pub const RETURN_SENTINEL: u32 = 0xFFFF_FFF0;

/// Functions the host runs natively.
pub trait Host {
    /// Run the guest function at `target` natively: read the arguments from `cpu`, write the
    /// results back. The interpreter then continues at `cpu.lr`.
    fn hook(&mut self, cpu: &mut Cpu, g: &mut Guest, target: u32) -> Result<()>;
}

/// A host with no native functions.
pub struct NoHost;

impl Host for NoHost {
    fn hook(&mut self, _cpu: &mut Cpu, _g: &mut Guest, target: u32) -> Result<()> {
        Err(Error::new(target, "no host function registered"))
    }
}

/// The register file.
#[derive(Clone)]
pub struct Cpu {
    pub r: [u64; 32],
    pub f: [f64; 32],
    /// VMX128 has 128 vector registers; the RexGlue lane layout (see the module note).
    pub v: Box<[u128; 128]>,
    pub cr: u32,
    pub lr: u64,
    pub ctr: u64,
    pub ca: bool,
    pub so: bool,
    pub ov: bool,
    pub fpscr: u32,
    pub msr: u64,
    pub vrsave: u32,
    pub pc: u32,
    reserve: Option<u32>,
    /// A monotonically increasing time base for `mftb`.
    pub timebase: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Self {
            r: [0; 32],
            f: [0.0; 32],
            v: Box::new([0; 128]),
            cr: 0,
            lr: 0,
            ctr: 0,
            ca: false,
            so: false,
            ov: false,
            fpscr: 0,
            msr: 0x8000,
            vrsave: 0,
            pc: 0,
            reserve: None,
            timebase: 0,
        }
    }
}

/// The interpreter: a register file, the host-function table, and a step budget.
pub struct Interp {
    pub cpu: Cpu,
    /// Branch targets the host runs natively.
    pub hooks: Hooks,
    /// Instructions a single [`Interp::call`] may execute before it is abandoned as a hang.
    pub step_limit: u64,
    /// Instructions executed so far, over every call.
    pub steps: u64,
    /// MXCSR flush/trap bits for a call (rounding comes from FPSCR): the recomp's [`vmx::VMX_CSR`]
    /// by default; `0x1F80` runs scalar and vector IEEE, with no flush on either side.
    pub mxcsr: u32,
    /// When set, calls are counted and a guest call stack is kept, and an error names the chain
    /// of calls that reached it: for finding what original code touches before it is hooked.
    pub trace: Option<CallTrace>,
    /// Test-only: run the VMX arithmetic the physics port's way instead of RexGlue's, so a TU3
    /// oracle isolates structural differences from rounding conventions Xenon's binary cannot
    /// decide (PHYSICS-PARITY-AUDIT section 6): `vmaddfp`/`vnmsubfp` fused (one rounding, the ISA's
    /// `-(a*c - b)`), `vmsum3fp`/`vmsum4fp` as `((p0 + p1) + p2) + p3` of rounded products, and
    /// `vrsqrtefp` as the port's exact `1/sqrt` seed with subnormals to 0 (D11). Default off.
    pub fused_vmx: bool,
}

/// See [`Interp::trace`].
#[derive(Default, Debug)]
pub struct CallTrace {
    /// Every call target (hooked or not) with how often it was called.
    pub calls: std::collections::BTreeMap<u32, u64>,
    /// The targets of the calls still open, innermost last.
    pub stack: Vec<u32>,
    /// Each open call's site (the linking branch), parallel to `stack`; a branch to `site + 4`
    /// closes it (so tail calls and helper-ended epilogues pop exactly the frames they end).
    pub sites: Vec<u32>,
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------------------ bit helpers

#[inline]
fn bits(w: u32, hi: u32, lo: u32) -> u32 {
    // IBM numbering: bit 0 is the most significant.
    (w >> (31 - lo)) & ((1u32 << (lo - hi + 1)) - 1)
}

/// `MASK(mb, me)` over 64 bits, IBM numbering, wrapping when `mb > me`.
#[inline]
fn mask64(mb: u32, me: u32) -> u64 {
    let begin = u64::MAX >> mb;
    let end = if me >= 63 { u64::MAX } else { !(u64::MAX >> (me + 1)) };
    if mb <= me { begin & end } else { begin | end }
}

#[inline]
fn low(x: u64) -> u32 {
    x as u32
}

#[inline]
fn sext16(x: u32) -> i64 {
    (x as u16 as i16) as i64
}

/// CR field value (LT GT EQ SO) for a signed comparison.
#[inline]
fn cmp_field<T: PartialOrd>(a: T, b: T, so: bool) -> u32 {
    let mut f = if a < b {
        8
    } else if a > b {
        4
    } else {
        2
    };
    if so {
        f |= 1;
    }
    f
}

// ------------------------------------------------------------------------------- vector helpers

#[inline]
fn m128(v: u128) -> __m128 {
    unsafe { std::mem::transmute::<u128, __m128>(v) }
}
#[inline]
fn m128i(v: u128) -> __m128i {
    unsafe { std::mem::transmute::<u128, __m128i>(v) }
}
#[inline]
fn from_ps(v: __m128) -> u128 {
    unsafe { std::mem::transmute::<__m128, u128>(v) }
}
#[inline]
fn from_si(v: __m128i) -> u128 {
    unsafe { std::mem::transmute::<__m128i, u128>(v) }
}
/// Guest word element `i` (0 = most significant).
#[inline]
fn elem(v: u128, i: u32) -> u32 {
    (v >> (32 * (3 - i))) as u32
}
#[inline]
fn from_elems(e: [u32; 4]) -> u128 {
    ((e[0] as u128) << 96) | ((e[1] as u128) << 64) | ((e[2] as u128) << 32) | (e[3] as u128)
}
/// Guest byte `k` (0 = most significant).
#[inline]
fn byte(v: u128, k: u32) -> u8 {
    (v >> (8 * (15 - k))) as u8
}
#[inline]
fn from_bytes(b: [u8; 16]) -> u128 {
    u128::from_be_bytes(b)
}
#[inline]
fn halves(v: u128) -> [u16; 8] {
    std::array::from_fn(|k| (v >> (16 * (7 - k))) as u16)
}
#[inline]
fn from_halves(h: [u16; 8]) -> u128 {
    h.iter().fold(0u128, |acc, &x| (acc << 16) | x as u128)
}

/// `vperm`: bytes of `a || b` chosen by the low five bits of each control byte.
fn permute(control: u128, a: u128, b: u128) -> u128 {
    let src: [u8; 32] = {
        let mut s = [0u8; 32];
        s[..16].copy_from_slice(&a.to_be_bytes());
        s[16..].copy_from_slice(&b.to_be_bytes());
        s
    };
    let c = control.to_be_bytes();
    from_bytes(std::array::from_fn(|k| src[(c[k] & 31) as usize]))
}

/// `lvsl`'s control vector for shift `sh`.
fn lvsl(sh: u32) -> u128 {
    from_bytes(std::array::from_fn(|k| (sh + k as u32) as u8))
}
/// `lvsr`'s control vector for shift `sh`.
fn lvsr(sh: u32) -> u128 {
    from_bytes(std::array::from_fn(|k| (16 - sh + k as u32) as u8))
}

/// `lfs`: single to double **by bits**. A format conversion, not arithmetic, so the hardware
/// keeps denormals and signalling NaNs exactly; `f32 as f64` would be `cvtss2sd` under the call's
/// DAZ and flush them. (A port's `lfs ; stfs` pair folds away in LLVM and keeps them too.)
fn lfs_value(bits: u32) -> f64 {
    let sign = ((bits >> 31) as u64) << 63;
    let exp = (bits >> 23) & 0xFF;
    let mant = (bits & 0x7F_FFFF) as u64;
    match exp {
        0xFF => f64::from_bits(sign | (0x7FF << 52) | (mant << 29)),
        0 => {
            // mant * 2^-149: both operands and the product are normal doubles, so DAZ/FTZ can't bite.
            let v = mant as f64 * f64::from_bits((1023 - 149) << 52);
            f64::from_bits(sign | v.to_bits())
        }
        _ => f64::from_bits(sign | ((exp as u64 + 1023 - 127) << 52) | (mant << 29)),
    }
}

/// `stfs`: double to single bits, rounding like [`fp::single_to_bits`] but without the call's FTZ,
/// so a value in the single-denormal range is stored denormal as the hardware stores it, and a NaN
/// keeps its payload unquieted.
fn stfs_bits(x: f64) -> u32 {
    let b = x.to_bits();
    let sign = ((b >> 63) as u32) << 31;
    if x.is_nan() {
        return sign | 0x7F80_0000 | ((b >> 29) as u32 & 0x7F_FFFF);
    }
    let a = x.abs();
    if a != 0.0 && a < f32::MIN_POSITIVE as f64 {
        // |x| * 2^149 is exact; rounding it to an integer is the single's denormal mantissa (2^23
        // rounds up into the smallest normal, whose bits are the same integer).
        return sign | (a * f64::from_bits((1023 + 149) << 52)).round_ties_even() as u32;
    }
    (x as f32).to_bits()
}

/// Scalar float lane view.
#[inline]
fn fl(v: u128, i: u32) -> f32 {
    f32::from_bits(elem(v, i))
}

/// A lane-wise float operation of three vectors.
fn lanes3(a: u128, b: u128, c: u128, f: impl Fn(f32, f32, f32) -> f32) -> u128 {
    from_elems(std::array::from_fn(|i| f(fl(a, i as u32), fl(b, i as u32), fl(c, i as u32)).to_bits()))
}

// -------------------------------------------------------------------------------- the machine

impl Interp {
    pub fn new() -> Self {
        Self {
            cpu: Cpu::default(),
            hooks: Hooks::default(),
            step_limit: 200_000_000,
            steps: 0,
            mxcsr: vmx::VMX_CSR,
            trace: None,
            fused_vmx: false,
        }
    }

    /// `vmaddfp` `a * c + b`; see [`Interp::fused_vmx`].
    fn vmadd(&self, a: u128, c: u128, b: u128) -> u128 {
        if self.fused_vmx {
            lanes3(a, c, b, |a, c, b| a.mul_add(c, b))
        } else {
            unsafe { from_ps(vmx::vmaddfp(m128(a), m128(c), m128(b))) }
        }
    }
    /// `vnmsubfp` `-(a * c - b)`; see [`Interp::fused_vmx`].
    fn vnmsub(&self, a: u128, c: u128, b: u128) -> u128 {
        if self.fused_vmx {
            lanes3(a, c, b, |a, c, b| -a.mul_add(c, -b))
        } else {
            unsafe { from_ps(vmx::vnmsubfp(m128(a), m128(c), m128(b))) }
        }
    }
    /// `vmsum3fp128` / `vmsum4fp128`; see [`Interp::fused_vmx`].
    fn vmsum(&self, a: u128, b: u128, n: u32) -> u128 {
        if !self.fused_vmx {
            return unsafe {
                from_ps(if n == 3 { vmx::vmsum3fp(m128(a), m128(b)) } else { vmx::vmsum4fp(m128(a), m128(b)) })
            };
        }
        let p = |i| fl(a, i) * fl(b, i);
        let mut sum = p(0) + p(1) + p(2);
        if n == 4 {
            sum += p(3);
        }
        from_elems([sum.to_bits(); 4])
    }
    /// `vrsqrtefp`; see [`Interp::fused_vmx`].
    fn vrsqrte(&self, b: u128) -> u128 {
        if !self.fused_vmx {
            return unsafe { from_ps(vmx::vrsqrtefp(m128(b))) };
        }
        let seed = |x: f32| if x > 0.0 && x < f32::MIN_POSITIVE { 0.0 } else { x.sqrt().recip() };
        from_elems(std::array::from_fn(|i| seed(fl(b, i as u32)).to_bits()))
    }

    /// Call the guest function at `target` with `args` in r3.. and return r3.
    ///
    /// The caller owns the stack: `cpu.r[1]` must point into writable guest memory with room
    /// below it (and `cpu.r[13]` at a thread block, if the code reads one).
    pub fn call(
        &mut self,
        g: &mut Guest,
        host: &mut dyn Host,
        target: u32,
        args: &[u64],
    ) -> Result<u64> {
        for (i, &a) in args.iter().enumerate() {
            self.cpu.r[3 + i] = a;
        }
        let saved_lr = self.cpu.lr;
        let saved_pc = self.cpu.pc;
        // A guest that faults mid-body leaves its frame pushed; a clean return restores r1 itself.
        let saved_sp = self.cpu.r[1];
        self.cpu.lr = RETURN_SENTINEL as u64;
        self.cpu.pc = target;
        let csr = vmx::get_mxcsr();
        vmx::set_mxcsr(self.mxcsr | rounding_bits(self.cpu.fpscr));
        let result = self.run(g, host);
        vmx::set_mxcsr(csr);
        self.cpu.lr = saved_lr;
        self.cpu.pc = saved_pc;
        self.cpu.r[1] = saved_sp;
        result.map(|()| self.cpu.r[3])
    }

    fn run(&mut self, g: &mut Guest, host: &mut dyn Host) -> Result<()> {
        let budget = self.steps + self.step_limit;
        if self.hooks.contains(&self.cpu.pc) {
            self.run_hook(g, host, self.cpu.pc)?;
        }
        while self.cpu.pc != RETURN_SENTINEL {
            if self.steps >= budget {
                return Err(Error::new(
                    self.cpu.pc,
                    format!("guest call exceeded {} instructions", self.step_limit),
                ));
            }
            self.steps += 1;
            let pc = self.cpu.pc;
            let w = g.fetch(pc).map_err(|e| Error::new(pc, format!("fetch: {}", e.message)))?;
            match self.step(g, w) {
                Ok(Some(target)) => {
                    self.cpu.pc = target;
                    let hooked = self.hooks.contains(&target);
                    if let Some(trace) = &mut self.trace {
                        let (op, xo) = (w >> 26, (w >> 1) & 0x3FF);
                        if w & 1 != 0 && (op == 16 || op == 18 || (op == 19 && (xo == 16 || xo == 528))) {
                            *trace.calls.entry(target).or_default() += 1;
                            if !hooked {
                                trace.stack.push(target);
                                trace.sites.push(pc);
                            }
                        } else if let Some(depth) = trace.sites.iter().rposition(|s| s.wrapping_add(4) == target) {
                            trace.stack.truncate(depth);
                            trace.sites.truncate(depth);
                        }
                    }
                    if hooked {
                        self.run_hook(g, host, target)
                            .map_err(|e| self.with_stack(e))?;
                    }
                }
                Ok(None) => self.cpu.pc = pc.wrapping_add(4),
                Err(e) => {
                    let e = Error::new(
                        e.address,
                        format!("{} (at pc {pc:#010x}, insn {w:08X})", e.message),
                    );
                    return Err(self.with_stack(e));
                }
            }
        }
        Ok(())
    }

    /// `e` with the traced guest call stack appended, innermost first.
    fn with_stack(&self, e: Error) -> Error {
        match &self.trace {
            Some(trace) if !trace.stack.is_empty() => {
                let chain: Vec<String> = trace
                    .stack
                    .iter()
                    .zip(&trace.sites)
                    .rev()
                    .map(|(a, s)| format!("{a:08X}@{s:08X}"))
                    .collect();
                let c = &self.cpu;
                let regs: Vec<String> = (3..=12).map(|i| format!("r{i}={:X}", c.r[i] as u32)).collect();
                Error::new(
                    e.address,
                    format!("{} [in {}] [{} lr={:X}]", e.message, chain.join(" <- "), regs.join(" "), c.lr as u32),
                )
            }
            _ => e,
        }
    }

    fn run_hook(&mut self, g: &mut Guest, host: &mut dyn Host, target: u32) -> Result<()> {
        let csr = vmx::get_mxcsr();
        let result = host.hook(&mut self.cpu, g, target);
        vmx::set_mxcsr(csr);
        result?;
        self.cpu.pc = (self.cpu.lr as u32) & !3;
        if self.cpu.lr as u32 == RETURN_SENTINEL {
            self.cpu.pc = RETURN_SENTINEL;
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------- CR helpers

    #[inline]
    fn set_crf(&mut self, field: u32, value: u32) {
        let shift = 28 - 4 * field;
        self.cpu.cr = (self.cpu.cr & !(0xF << shift)) | ((value & 0xF) << shift);
    }
    #[inline]
    fn crbit(&self, n: u32) -> bool {
        (self.cpu.cr >> (31 - n)) & 1 != 0
    }
    #[inline]
    fn set_crbit(&mut self, n: u32, v: bool) {
        let m = 1u32 << (31 - n);
        if v {
            self.cpu.cr |= m;
        } else {
            self.cpu.cr &= !m;
        }
    }
    #[inline]
    fn cr0_word(&mut self, value: u64) {
        let so = self.cpu.so;
        self.set_crf(0, cmp_field(value as u32 as i32, 0, so));
    }
    #[inline]
    fn cr0_double(&mut self, value: u64) {
        let so = self.cpu.so;
        self.set_crf(0, cmp_field(value as i64, 0, so));
    }
    #[inline]
    fn ea(&self, ra: u32, offset: i64) -> u32 {
        let base = if ra == 0 { 0 } else { self.cpu.r[ra as usize] };
        base.wrapping_add(offset as u64) as u32
    }
    #[inline]
    fn ea_x(&self, ra: u32, rb: u32) -> u32 {
        let base = if ra == 0 { 0 } else { self.cpu.r[ra as usize] };
        base.wrapping_add(self.cpu.r[rb as usize]) as u32
    }

    fn branch_cond(&mut self, bo: u32, bi: u32) -> bool {
        if bo & 4 == 0 {
            self.cpu.ctr = self.cpu.ctr.wrapping_sub(1);
        }
        let ctr_ok = bo & 4 != 0 || (((self.cpu.ctr as u32) != 0) != (bo & 2 != 0));
        let cond_ok = bo & 16 != 0 || (self.crbit(bi) == (bo & 8 != 0));
        ctr_ok && cond_ok
    }

    // ------------------------------------------------------------------------------ one step

    /// Execute one instruction. `Ok(Some(target))` is a taken branch.
    fn step(&mut self, g: &mut Guest, w: u32) -> Result<Option<u32>> {
        let pc = self.cpu.pc;
        let op = w >> 26;
        let rd = bits(w, 6, 10);
        let ra = bits(w, 11, 15);
        let rb = bits(w, 16, 20);
        let rc_bit = w & 1 != 0;
        let d = sext16(w & 0xFFFF);
        let uimm = (w & 0xFFFF) as u64;
        match op {
            2 | 3 => {} // tdi, twi: RexGlue's type-0 trap warns and continues
            4 => return self.op4(g, w),
            5 => return self.op5(w),
            6 => return self.op6(w),
            7 => {
                // mulli
                self.cpu.r[rd as usize] = (self.cpu.r[ra as usize] as i64).wrapping_mul(d) as u64;
            }
            8 => {
                // subfic
                let a = self.cpu.r[ra as usize];
                self.cpu.r[rd as usize] = (d as u64).wrapping_sub(a);
                self.cpu.ca = (!low(a) as u64 + (d as u32) as u64 + 1) > 0xFFFF_FFFF;
            }
            10 | 11 => {
                // cmpli, cmpi
                let crf = rd >> 2;
                let l = rd & 1 != 0;
                let a = self.cpu.r[ra as usize];
                let so = self.cpu.so;
                let v = match (op, l) {
                    (10, false) => cmp_field(low(a), uimm as u32, so),
                    (10, true) => cmp_field(a, uimm, so),
                    (11, false) => cmp_field(low(a) as i32, d as i32, so),
                    _ => cmp_field(a as i64, d, so),
                };
                self.set_crf(crf, v);
            }
            12 | 13 => {
                // addic, addic.
                let a = self.cpu.r[ra as usize];
                let res = a.wrapping_add(d as u64);
                self.cpu.r[rd as usize] = res;
                self.cpu.ca = (low(a) as u64 + (d as u32) as u64) > 0xFFFF_FFFF;
                if op == 13 {
                    self.cr0_word(res);
                }
            }
            14 => {
                // addi / li
                let base = if ra == 0 { 0 } else { self.cpu.r[ra as usize] };
                self.cpu.r[rd as usize] = base.wrapping_add(d as u64);
            }
            15 => {
                // addis / lis
                let base = if ra == 0 { 0 } else { self.cpu.r[ra as usize] };
                self.cpu.r[rd as usize] = base.wrapping_add((d << 16) as u64);
            }
            16 => {
                // bc
                let bo = rd;
                let bi = ra;
                let bd = sext16(w & 0xFFFC) as u32;
                let lk = w & 1 != 0;
                let aa = w & 2 != 0;
                let take = self.branch_cond(bo, bi);
                if lk {
                    self.cpu.lr = pc.wrapping_add(4) as u64;
                }
                if take {
                    return Ok(Some(if aa { bd } else { pc.wrapping_add(bd) }));
                }
            }
            17 => return Err(Error::new(pc, "sc (system call) is not supported")),
            18 => {
                // b
                let mut li = w & 0x03FF_FFFC;
                if li & 0x0200_0000 != 0 {
                    li |= 0xFC00_0000;
                }
                if w & 1 != 0 {
                    self.cpu.lr = pc.wrapping_add(4) as u64;
                }
                return Ok(Some(if w & 2 != 0 { li } else { pc.wrapping_add(li) }));
            }
            19 => return self.op19(w),
            20 | 21 | 23 => {
                // rlwimi, rlwinm, rlwnm
                let s = self.cpu.r[rd as usize];
                let sh = if op == 23 { low(self.cpu.r[rb as usize]) & 31 } else { rb };
                let mb = bits(w, 21, 25);
                let me = bits(w, 26, 30);
                let m = mask64(mb + 32, me + 32);
                let rot = fp::rlwinm(s, sh, u64::MAX);
                let res = if op == 20 {
                    (rot & m) | (self.cpu.r[ra as usize] & !m)
                } else {
                    rot & m
                };
                self.cpu.r[ra as usize] = res;
                if rc_bit {
                    self.cr0_word(res);
                }
            }
            24 => self.cpu.r[ra as usize] = self.cpu.r[rd as usize] | uimm,
            25 => self.cpu.r[ra as usize] = self.cpu.r[rd as usize] | (uimm << 16),
            26 => self.cpu.r[ra as usize] = self.cpu.r[rd as usize] ^ uimm,
            27 => self.cpu.r[ra as usize] = self.cpu.r[rd as usize] ^ (uimm << 16),
            28 => {
                let res = self.cpu.r[rd as usize] & uimm;
                self.cpu.r[ra as usize] = res;
                self.cr0_word(res);
            }
            29 => {
                let res = self.cpu.r[rd as usize] & (uimm << 16);
                self.cpu.r[ra as usize] = res;
                self.cr0_word(res);
            }
            30 => return self.op30(w),
            31 => return self.op31(g, w),
            32..=55 => return self.load_store_d(g, w),
            56 | 57 | 60 | 61 => {
                return Err(Error::new(pc, format!("unsupported primary opcode {op}")));
            }
            58 => {
                // ld, ldu, lwa (DS form)
                let ds = sext16(w & 0xFFFC);
                let ea = self.ea(ra, ds);
                match w & 3 {
                    0 => self.cpu.r[rd as usize] = g.u64(ea)?,
                    1 => {
                        self.cpu.r[rd as usize] = g.u64(ea)?;
                        self.cpu.r[ra as usize] = ea as u64;
                    }
                    2 => self.cpu.r[rd as usize] = g.u32(ea)? as i32 as i64 as u64,
                    _ => return Err(Error::new(pc, "invalid DS-form load")),
                }
            }
            59 => return self.op59(w),
            62 => {
                let ds = sext16(w & 0xFFFC);
                let ea = self.ea(ra, ds);
                match w & 3 {
                    0 => g.set_u64(ea, self.cpu.r[rd as usize])?,
                    1 => {
                        g.set_u64(ea, self.cpu.r[rd as usize])?;
                        self.cpu.r[ra as usize] = ea as u64;
                    }
                    _ => return Err(Error::new(pc, "invalid DS-form store")),
                }
            }
            63 => return self.op63(w),
            _ => return Err(Error::new(pc, format!("unsupported primary opcode {op}"))),
        }
        Ok(None)
    }

    fn load_store_d(&mut self, g: &mut Guest, w: u32) -> Result<Option<u32>> {
        let op = w >> 26;
        let rd = bits(w, 6, 10) as usize;
        let ra = bits(w, 11, 15);
        let d = sext16(w & 0xFFFF);
        let ea = self.ea(ra, d);
        let update = matches!(op, 33 | 35 | 37 | 39 | 41 | 43 | 45 | 49 | 51 | 53 | 55);
        match op {
            32 | 33 => self.cpu.r[rd] = g.u32(ea)? as u64,
            34 | 35 => self.cpu.r[rd] = g.u8(ea)? as u64,
            36 | 37 => g.set_u32(ea, low(self.cpu.r[rd]))?,
            38 | 39 => g.set_u8(ea, self.cpu.r[rd] as u8)?,
            40 | 41 => self.cpu.r[rd] = g.u16(ea)? as u64,
            42 | 43 => self.cpu.r[rd] = g.u16(ea)? as i16 as i64 as u64,
            44 | 45 => g.set_u16(ea, self.cpu.r[rd] as u16)?,
            46 => {
                // lmw
                let mut at = ea;
                for reg in rd..32 {
                    self.cpu.r[reg] = g.u32(at)? as u64;
                    at = at.wrapping_add(4);
                }
            }
            47 => {
                // stmw
                let mut at = ea;
                for reg in rd..32 {
                    g.set_u32(at, low(self.cpu.r[reg]))?;
                    at = at.wrapping_add(4);
                }
            }
            48 | 49 => self.cpu.f[rd] = lfs_value(g.u32(ea)?),
            50 | 51 => self.cpu.f[rd] = fp::load_double(g, ea)?,
            52 | 53 => g.set_u32(ea, stfs_bits(self.cpu.f[rd]))?,
            54 | 55 => g.set_u64(ea, self.cpu.f[rd].to_bits())?,
            _ => unreachable!(),
        }
        if update {
            self.cpu.r[ra as usize] = ea as u64;
        }
        Ok(None)
    }

    // ------------------------------------------------------------------------ opcode 19

    fn op19(&mut self, w: u32) -> Result<Option<u32>> {
        let pc = self.cpu.pc;
        let xo = bits(w, 21, 30);
        let bt = bits(w, 6, 10);
        let ba = bits(w, 11, 15);
        let bb = bits(w, 16, 20);
        match xo {
            0 => {
                // mcrf
                let src = (self.cpu.cr >> (28 - 4 * (ba >> 2))) & 0xF;
                self.set_crf(bt >> 2, src);
            }
            16 | 528 => {
                // bclr, bcctr
                let lk = w & 1 != 0;
                let take = if xo == 16 {
                    self.branch_cond(bt, ba)
                } else {
                    bt & 16 != 0 || (self.crbit(ba) == (bt & 8 != 0))
                };
                let target = if xo == 16 {
                    (self.cpu.lr as u32) & !3
                } else {
                    (self.cpu.ctr as u32) & !3
                };
                if lk {
                    self.cpu.lr = pc.wrapping_add(4) as u64;
                }
                if take {
                    return Ok(Some(target));
                }
            }
            150 => {} // isync
            33 | 129 | 193 | 225 | 257 | 289 | 417 | 449 => {
                let a = self.crbit(ba);
                let b = self.crbit(bb);
                let v = match xo {
                    33 => !(a | b),  // crnor
                    129 => a & !b,   // crandc
                    193 => a ^ b,    // crxor
                    225 => !(a & b), // crnand
                    257 => a & b,    // crand
                    289 => !(a ^ b), // creqv
                    417 => a | !b,   // crorc
                    _ => a | b,      // cror
                };
                self.set_crbit(bt, v);
            }
            _ => return Err(Error::new(pc, format!("unsupported opcode 19/{xo}"))),
        }
        Ok(None)
    }

    // ------------------------------------------------------------------------ opcode 30

    fn op30(&mut self, w: u32) -> Result<Option<u32>> {
        let rs = bits(w, 6, 10) as usize;
        let ra = bits(w, 11, 15) as usize;
        let rb = bits(w, 16, 20) as usize;
        let sh = bits(w, 16, 20) | (bits(w, 30, 30) << 5);
        let mb = bits(w, 21, 25) | (bits(w, 26, 26) << 5);
        let s = self.cpu.r[rs];
        let res = match bits(w, 27, 29) {
            0 => s.rotate_left(sh) & mask64(mb, 63),      // rldicl
            1 => s.rotate_left(sh) & mask64(0, mb),       // rldicr (me in the mb field)
            2 => s.rotate_left(sh) & mask64(mb, 63 - sh), // rldic
            3 => {
                // rldimi
                let m = mask64(mb, 63 - sh);
                (s.rotate_left(sh) & m) | (self.cpu.r[ra] & !m)
            }
            4 => {
                let n = low(self.cpu.r[rb]) & 63;
                match bits(w, 27, 30) {
                    8 => s.rotate_left(n) & mask64(mb, 63), // rldcl
                    9 => s.rotate_left(n) & mask64(0, mb),  // rldcr
                    x => {
                        return Err(Error::new(self.cpu.pc, format!("unsupported opcode 30/{x}")));
                    }
                }
            }
            x => return Err(Error::new(self.cpu.pc, format!("unsupported opcode 30/{x}"))),
        };
        self.cpu.r[ra] = res;
        if w & 1 != 0 {
            self.cr0_double(res);
        }
        Ok(None)
    }

    // ------------------------------------------------------------------------ opcode 31

    fn op31(&mut self, g: &mut Guest, w: u32) -> Result<Option<u32>> {
        let pc = self.cpu.pc;
        let rd = bits(w, 6, 10);
        let ra = bits(w, 11, 15);
        let rb = bits(w, 16, 20);
        let rc_bit = w & 1 != 0;
        let xo10 = bits(w, 21, 30);
        let xo9 = bits(w, 22, 30);
        let a = self.cpu.r[ra as usize];
        let b = self.cpu.r[rb as usize];
        let s = self.cpu.r[rd as usize];

        // sradi (XS form, 9-bit xo at 21-29)
        if bits(w, 21, 29) == 413 {
            let sh = rb | (bits(w, 30, 30) << 5);
            let v = s as i64;
            let res = if sh == 0 { v } else { v >> sh };
            self.cpu.ca = v < 0 && sh != 0 && (s & ((1u64 << sh) - 1)) != 0;
            self.cpu.r[ra as usize] = res as u64;
            if rc_bit {
                self.cr0_double(res as u64);
            }
            return Ok(None);
        }

        // XO-form arithmetic (9-bit xo, OE at bit 21)
        let arith = match xo9 {
            8 | 9 | 10 | 11 | 40 | 73 | 75 | 104 | 136 | 138 | 200 | 202 | 232 | 233 | 234 | 235
            | 266 | 457 | 459 | 489 | 491 => true,
            _ => false,
        };
        if arith {
            let (res, word_cr) = match xo9 {
                8 => {
                    // subfc
                    self.cpu.ca = (!low(a) as u64 + low(b) as u64 + 1) > 0xFFFF_FFFF;
                    (b.wrapping_sub(a), true)
                }
                9 => (((a as u128 * b as u128) >> 64) as u64, false), // mulhdu
                10 => {
                    // addc
                    self.cpu.ca = (low(a) as u64 + low(b) as u64) > 0xFFFF_FFFF;
                    (a.wrapping_add(b), true)
                }
                11 => (((low(a) as u64 * low(b) as u64) >> 32), true), // mulhwu
                40 => (b.wrapping_sub(a), true),                       // subf
                73 => ((((a as i64 as i128) * (b as i64 as i128)) >> 64) as u64, false), // mulhd
                75 => (
                    (((low(a) as i32 as i64) * (low(b) as i32 as i64)) >> 32) as u64,
                    true,
                ), // mulhw
                104 => ((a as i64).wrapping_neg() as u64, true), // neg
                136 => {
                    // subfe
                    let ci = self.cpu.ca as u64;
                    self.cpu.ca = (!low(a) as u64 + low(b) as u64 + ci) > 0xFFFF_FFFF;
                    ((!a).wrapping_add(b).wrapping_add(ci), true)
                }
                138 => {
                    // adde
                    let ci = self.cpu.ca as u64;
                    self.cpu.ca = (low(a) as u64 + low(b) as u64 + ci) > 0xFFFF_FFFF;
                    (a.wrapping_add(b).wrapping_add(ci), true)
                }
                200 => {
                    // subfze
                    let ci = self.cpu.ca as u64;
                    self.cpu.ca = (!low(a) as u64 + ci) > 0xFFFF_FFFF;
                    ((!a).wrapping_add(ci), true)
                }
                202 => {
                    // addze
                    let ci = self.cpu.ca as u64;
                    self.cpu.ca = (low(a) as u64 + ci) > 0xFFFF_FFFF;
                    (a.wrapping_add(ci), true)
                }
                232 => {
                    // subfme
                    let ci = self.cpu.ca as u64;
                    self.cpu.ca = (!low(a) as u64 + ci + 0xFFFF_FFFF) > 0xFFFF_FFFF;
                    ((!a).wrapping_add(ci).wrapping_sub(1), true)
                }
                233 => ((a as i64).wrapping_mul(b as i64) as u64, false), // mulld
                234 => {
                    // addme
                    let ci = self.cpu.ca as u64;
                    self.cpu.ca = (low(a) as u64 + ci + 0xFFFF_FFFF) > 0xFFFF_FFFF;
                    (a.wrapping_add(ci).wrapping_sub(1), true)
                }
                235 => (
                    ((low(a) as i32 as i64).wrapping_mul(low(b) as i32 as i64)) as u64,
                    true,
                ), // mullw: the full product of the two sign-extended words
                266 => (a.wrapping_add(b), true), // add
                457 => (if b == 0 { 0 } else { a / b }, false), // divdu
                459 => (
                    if low(b) == 0 { 0 } else { (low(a) / low(b)) as u64 },
                    true,
                ), // divwu
                489 => {
                    // divd
                    let (x, y) = (a as i64, b as i64);
                    (
                        if y == 0 || (x == i64::MIN && y == -1) { 0 } else { (x / y) as u64 },
                        false,
                    )
                }
                491 => {
                    // divw
                    let (x, y) = (low(a) as i32, low(b) as i32);
                    (
                        if y == 0 || (x == i32::MIN && y == -1) {
                            0
                        } else {
                            (x / y) as u32 as u64
                        },
                        true,
                    )
                }
                _ => unreachable!(),
            };
            self.cpu.r[rd as usize] = res;
            if bits(w, 21, 21) != 0 {
                // OE: signed overflow of the word result, approximated on the low words.
                let ov = matches!(xo9, 266 | 10 | 138) && {
                    let (x, y) = (low(a) as i32, low(b) as i32);
                    x.checked_add(y).is_none()
                };
                self.cpu.ov = ov;
                self.cpu.so |= ov;
            }
            if rc_bit {
                if word_cr {
                    self.cr0_word(res);
                } else {
                    self.cr0_double(res);
                }
            }
            return Ok(None);
        }

        match xo10 {
            0 | 32 => {
                // cmp, cmpl
                let crf = rd >> 2;
                let l = rd & 1 != 0;
                let so = self.cpu.so;
                let v = match (xo10, l) {
                    (0, false) => cmp_field(low(a) as i32, low(b) as i32, so),
                    (0, true) => cmp_field(a as i64, b as i64, so),
                    (_, false) => cmp_field(low(a), low(b), so),
                    _ => cmp_field(a, b, so),
                };
                self.set_crf(crf, v);
            }
            4 | 68 => {} // tw, td
            19 => self.cpu.r[rd as usize] = self.cpu.cr as u64, // mfcr
            144 => {
                // mtcrf
                let fxm = bits(w, 12, 19);
                for field in 0..8 {
                    if fxm & (0x80 >> field) != 0 {
                        let shift = 28 - 4 * field;
                        self.cpu.cr =
                            (self.cpu.cr & !(0xF << shift)) | (low(s) & (0xF << shift));
                    }
                }
            }
            83 => self.cpu.r[rd as usize] = self.cpu.msr, // mfmsr
            146 | 178 => self.cpu.msr = s,                  // mtmsr, mtmsrd
            339 | 467 => {
                // mfspr, mtspr
                let spr = ra | (rb << 5);
                if xo10 == 339 {
                    self.cpu.r[rd as usize] = match spr {
                        1 => self.xer(),
                        8 => self.cpu.lr,
                        9 => self.cpu.ctr,
                        256 => self.cpu.vrsave as u64,
                        268 | 269 => {
                            self.cpu.timebase += 1;
                            self.cpu.timebase
                        }
                        _ => return Err(Error::new(pc, format!("mfspr {spr} is not supported"))),
                    };
                } else {
                    match spr {
                        1 => {
                            self.cpu.so = s & (1 << 31) != 0;
                            self.cpu.ov = s & (1 << 30) != 0;
                            self.cpu.ca = s & (1 << 29) != 0;
                        }
                        8 => self.cpu.lr = s,
                        9 => self.cpu.ctr = s,
                        256 => self.cpu.vrsave = low(s),
                        _ => return Err(Error::new(pc, format!("mtspr {spr} is not supported"))),
                    }
                }
            }
            371 => {
                // mftb
                self.cpu.timebase += 1;
                self.cpu.r[rd as usize] = self.cpu.timebase;
            }
            24 => {
                // slw
                let n = low(b) & 63;
                let res = if n > 31 { 0 } else { (low(s) << n) as u64 };
                self.cpu.r[ra as usize] = res;
                if rc_bit {
                    self.cr0_word(res);
                }
            }
            536 => {
                // srw
                let n = low(b) & 63;
                let res = if n > 31 { 0 } else { (low(s) >> n) as u64 };
                self.cpu.r[ra as usize] = res;
                if rc_bit {
                    self.cr0_word(res);
                }
            }
            792 | 824 => {
                // sraw, srawi
                let n = if xo10 == 824 { rb } else { low(b) & 63 };
                let v = low(s) as i32;
                let (res, ca) = if n > 31 {
                    (if v < 0 { -1 } else { 0 }, v < 0)
                } else if n == 0 {
                    (v, false)
                } else {
                    (v >> n, v < 0 && (low(s) & ((1u32 << n) - 1)) != 0)
                };
                self.cpu.ca = ca;
                self.cpu.r[ra as usize] = res as i64 as u64;
                if rc_bit {
                    self.cr0_word(res as i64 as u64);
                }
            }
            27 => {
                // sld
                let n = low(b) & 127;
                let res = if n > 63 { 0 } else { s << n };
                self.cpu.r[ra as usize] = res;
                if rc_bit {
                    self.cr0_double(res);
                }
            }
            539 => {
                // srd
                let n = low(b) & 127;
                let res = if n > 63 { 0 } else { s >> n };
                self.cpu.r[ra as usize] = res;
                if rc_bit {
                    self.cr0_double(res);
                }
            }
            794 => {
                // srad
                let n = low(b) & 127;
                let v = s as i64;
                let (res, ca) = if n > 63 {
                    (if v < 0 { -1 } else { 0 }, v < 0)
                } else if n == 0 {
                    (v, false)
                } else {
                    (v >> n, v < 0 && (s & ((1u64 << n) - 1)) != 0)
                };
                self.cpu.ca = ca;
                self.cpu.r[ra as usize] = res as u64;
                if rc_bit {
                    self.cr0_double(res as u64);
                }
            }
            26 | 58 | 28 | 60 | 124 | 284 | 316 | 412 | 444 | 476 | 922 | 954 | 986 => {
                let res = match xo10 {
                    26 => low(s).leading_zeros() as u64, // cntlzw
                    58 => s.leading_zeros() as u64,      // cntlzd
                    28 => s & b,                         // and
                    60 => s & !b,                        // andc
                    124 => !(s | b),                     // nor
                    284 => !(s ^ b),                     // eqv
                    316 => s ^ b,                        // xor
                    412 => s | !b,                       // orc
                    444 => s | b,                        // or
                    476 => !(s & b),                     // nand
                    922 => s as u16 as i16 as i64 as u64, // extsh
                    954 => s as u8 as i8 as i64 as u64,  // extsb
                    _ => s as u32 as i32 as i64 as u64,  // extsw
                };
                self.cpu.r[ra as usize] = res;
                if rc_bit {
                    if matches!(xo10, 58 | 986) {
                        self.cr0_double(res);
                    } else if matches!(xo10, 26 | 922 | 954) {
                        self.cr0_word(res);
                    } else {
                        // Logical ops set CR0 from the full register in 64-bit mode; RexGlue
                        // compares the low word for word-sized code, which is what these are.
                        self.cr0_word(res);
                    }
                }
            }
            // ------------------------------------------------------------- X-form loads/stores
            23 | 55 | 87 | 119 | 279 | 311 | 343 | 375 | 21 | 53 | 341 | 373 | 151 | 183 | 215
            | 247 | 407 | 439 | 149 | 181 | 534 | 662 | 790 | 918 | 532 | 660 | 535 | 567 | 599
            | 631 | 663 | 695 | 727 | 759 | 983 => {
                let ea = self.ea_x(ra, rb);
                let rdu = rd as usize;
                match xo10 {
                    23 | 55 => self.cpu.r[rdu] = g.u32(ea)? as u64,
                    87 | 119 => self.cpu.r[rdu] = g.u8(ea)? as u64,
                    279 | 311 => self.cpu.r[rdu] = g.u16(ea)? as u64,
                    343 | 375 => self.cpu.r[rdu] = g.u16(ea)? as i16 as i64 as u64,
                    21 | 53 => self.cpu.r[rdu] = g.u64(ea)?,
                    341 | 373 => self.cpu.r[rdu] = g.u32(ea)? as i32 as i64 as u64,
                    151 | 183 => g.set_u32(ea, low(s))?,
                    215 | 247 => g.set_u8(ea, s as u8)?,
                    407 | 439 => g.set_u16(ea, s as u16)?,
                    149 | 181 => g.set_u64(ea, s)?,
                    534 => self.cpu.r[rdu] = g.u32(ea)?.swap_bytes() as u64, // lwbrx
                    662 => g.set_u32(ea, low(s).swap_bytes())?,                // stwbrx
                    790 => self.cpu.r[rdu] = g.u16(ea)?.swap_bytes() as u64, // lhbrx
                    918 => g.set_u16(ea, (s as u16).swap_bytes())?,            // sthbrx
                    532 => self.cpu.r[rdu] = g.u64(ea)?.swap_bytes(),        // ldbrx
                    660 => g.set_u64(ea, s.swap_bytes())?,                     // stdbrx
                    535 | 567 => self.cpu.f[rdu] = lfs_value(g.u32(ea)?),
                    599 | 631 => self.cpu.f[rdu] = fp::load_double(g, ea)?,
                    663 | 695 => g.set_u32(ea, stfs_bits(self.cpu.f[rdu]))?,
                    727 | 759 => g.set_u64(ea, self.cpu.f[rdu].to_bits())?,
                    983 => g.set_u32(ea, self.cpu.f[rdu].to_bits() as u32)?, // stfiwx
                    _ => unreachable!(),
                }
                if matches!(
                    xo10,
                    55 | 119 | 311 | 375 | 53 | 373 | 183 | 247 | 439 | 181 | 567 | 631 | 695 | 759
                ) {
                    self.cpu.r[ra as usize] = ea as u64;
                }
            }
            20 | 84 => {
                // lwarx, ldarx
                let ea = self.ea_x(ra, rb);
                self.cpu.reserve = Some(ea);
                self.cpu.r[rd as usize] = if xo10 == 20 {
                    g.u32(ea)? as u64
                } else {
                    g.u64(ea)?
                };
            }
            150 | 214 => {
                // stwcx., stdcx.
                let ea = self.ea_x(ra, rb);
                let ok = self.cpu.reserve == Some(ea);
                if ok {
                    if xo10 == 150 {
                        g.set_u32(ea, low(s))?;
                    } else {
                        g.set_u64(ea, s)?;
                    }
                }
                self.cpu.reserve = None;
                let so = self.cpu.so as u32;
                self.set_crf(0, if ok { 2 } else { 0 } | so);
            }
            1014 => {
                // dcbz (32 bytes) / dcbz128 (the L bit at 10)
                let ea = self.ea_x(ra, rb);
                if rd & 1 != 0 {
                    vmx::dcbzl(g, ea)?;
                } else {
                    g.fill(ea & !31, 0, 32)?;
                }
            }
            54 | 86 | 246 | 278 | 470 | 982 | 598 | 854 => {} // dcbst dcbf dcbtst dcbt dcbi icbi sync eieio
            512 => {
                // mcrxr
                let v = ((self.cpu.so as u32) << 3) | ((self.cpu.ov as u32) << 2) | ((self.cpu.ca as u32) << 1);
                self.set_crf(rd >> 2, v);
                self.cpu.so = false;
                self.cpu.ov = false;
                self.cpu.ca = false;
            }
            // ------------------------------------------------------------ AltiVec X-form memory
            6 | 38 | 7 | 39 | 71 | 103 | 359 | 135 | 167 | 199 | 231 | 487 | 519 | 551 | 647
            | 679 | 775 | 807 | 903 | 935 => {
                return self.vmx_memory(g, xo10, rd, ra, rb);
            }
            _ => return Err(Error::new(pc, format!("unsupported opcode 31/{xo10}"))),
        }
        Ok(None)
    }

    fn xer(&self) -> u64 {
        ((self.cpu.so as u64) << 31) | ((self.cpu.ov as u64) << 30) | ((self.cpu.ca as u64) << 29)
    }

    /// The AltiVec X-form vector memory operations, shared with their VMX128 forms.
    fn vmx_memory(&mut self, g: &mut Guest, xo10: u32, vd: u32, ra: u32, rb: u32) -> Result<Option<u32>> {
        let ea = self.ea_x(ra, rb);
        let vd = vd as usize;
        match xo10 {
            6 => self.cpu.v[vd] = lvsl(ea & 15),
            38 => self.cpu.v[vd] = lvsr(ea & 15),
            7 | 39 | 71 | 103 | 359 => self.cpu.v[vd] = load16(g, ea & !15)?, // lvebx lvehx lvewx lvx lvxl
            135 => g.set_u8(ea, byte(self.cpu.v[vd], ea & 15))?,           // stvebx
            167 => {
                let at = ea & !1;
                let k = (at & 15) / 2;
                g.set_u16(at, halves(self.cpu.v[vd])[k as usize])?; // stvehx
            }
            199 => {
                let at = ea & !3;
                g.set_u32(at, elem(self.cpu.v[vd], (at >> 2) & 3))?; // stvewx
            }
            231 | 487 => store16(g, ea & !15, self.cpu.v[vd])?, // stvx stvxl
            519 | 775 => self.cpu.v[vd] = lvlx(g, ea)?,       // lvlx lvlxl
            551 | 807 => self.cpu.v[vd] = lvrx(g, ea)?,       // lvrx lvrxl
            647 | 903 => stvlx(g, ea, self.cpu.v[vd])?,       // stvlx stvlxl
            679 | 935 => stvrx(g, ea, self.cpu.v[vd])?,       // stvrx stvrxl
            _ => unreachable!(),
        }
        Ok(None)
    }

    // ---------------------------------------------------------------- floating point, 59 / 63

    fn op59(&mut self, w: u32) -> Result<Option<u32>> {
        let fd = bits(w, 6, 10) as usize;
        let a = self.cpu.f[bits(w, 11, 15) as usize];
        let b = self.cpu.f[bits(w, 16, 20) as usize];
        let c = self.cpu.f[bits(w, 21, 25) as usize];
        let res = match bits(w, 26, 30) {
            18 => fp::div_single(a, b),
            20 => fp::sub_single(a, b),
            21 => fp::add_single(a, b),
            22 => fp::sqrt_single(b),
            24 => fp::frsp(1.0 / b),        // fres
            25 => fp::mul_single(a, c),
            28 => fp::fmsub_single(a, c, b), // fmsubs
            29 => fp::fmadd_single(a, c, b), // fmadds
            30 => fp::nmsub_single(a, c, b), // fnmsubs
            31 => fp::frsp(-(a.mul_add(c, b))), // fnmadds
            x => return Err(Error::new(self.cpu.pc, format!("unsupported opcode 59/{x}"))),
        };
        self.cpu.f[fd] = res;
        if w & 1 != 0 {
            self.set_crf(1, self.cpu.fpscr >> 28);
        }
        Ok(None)
    }

    fn op63(&mut self, w: u32) -> Result<Option<u32>> {
        let pc = self.cpu.pc;
        let fd = bits(w, 6, 10) as usize;
        let fa = bits(w, 11, 15) as usize;
        let fb = bits(w, 16, 20) as usize;
        let a = self.cpu.f[fa];
        let b = self.cpu.f[fb];
        let c = self.cpu.f[bits(w, 21, 25) as usize];
        let xo5 = bits(w, 26, 30);
        let res = match xo5 {
            18 => Some(a / b),
            20 => Some(a - b),
            21 => Some(a + b),
            22 => Some(b.sqrt()),
            23 => Some(fp::fsel(a, c, b)),
            25 => Some(a * c),
            26 => Some(1.0 / b.sqrt()), // frsqrte
            28 => Some(a.mul_add(c, -b)),
            29 => Some(a.mul_add(c, b)),
            30 => Some(-(a.mul_add(c, -b))),
            31 => Some(-(a.mul_add(c, b))),
            _ => None,
        };
        if let Some(v) = res {
            self.cpu.f[fd] = v;
        } else {
            match bits(w, 21, 30) {
                0 | 32 => {
                    // fcmpu, fcmpo
                    let v = if a.is_nan() || b.is_nan() {
                        1
                    } else if a < b {
                        8
                    } else if a > b {
                        4
                    } else {
                        2
                    };
                    self.set_crf(bits(w, 6, 8), v);
                    self.cpu.fpscr = (self.cpu.fpscr & !0xF000) | (v << 12);
                    return Ok(None);
                }
                12 => self.cpu.f[fd] = fp::frsp(b),
                14 => {
                    // fctiw: round per FPSCR[RN]
                    let rounded = round_fpscr(b, self.cpu.fpscr);
                    self.cpu.f[fd] = f64::from_bits(fp::fctiwz_low_word(rounded) as u64);
                }
                15 => self.cpu.f[fd] = f64::from_bits(fp::fctiwz_low_word(b) as u64),
                40 => self.cpu.f[fd] = fp::neg_double(b),
                72 => self.cpu.f[fd] = b,
                136 => self.cpu.f[fd] = f64::from_bits(b.to_bits() | (1 << 63)),
                264 => self.cpu.f[fd] = fp::abs_double(b),
                814 => self.cpu.f[fd] = f64::from_bits(fp::fctid(round_fpscr(b, self.cpu.fpscr)) as u64),
                815 => self.cpu.f[fd] = f64::from_bits(fp::fctidz(b) as u64),
                846 => self.cpu.f[fd] = fp::fcfid(b.to_bits() as i64),
                583 => self.cpu.f[fd] = f64::from_bits(self.cpu.fpscr as u64), // mffs
                711 => {
                    // mtfsf
                    let fm = bits(w, 7, 14);
                    let v = b.to_bits() as u32;
                    for field in 0..8 {
                        if fm & (0x80 >> field) != 0 {
                            let shift = 28 - 4 * field;
                            self.cpu.fpscr = (self.cpu.fpscr & !(0xF << shift)) | (v & (0xF << shift));
                        }
                    }
                    self.apply_rounding();
                }
                134 => {
                    // mtfsfi
                    let field = bits(w, 6, 8);
                    let imm = bits(w, 16, 19);
                    let shift = 28 - 4 * field;
                    self.cpu.fpscr = (self.cpu.fpscr & !(0xF << shift)) | (imm << shift);
                    self.apply_rounding();
                }
                38 | 70 => {
                    // mtfsb1, mtfsb0
                    let bit = 1u32 << (31 - fd as u32);
                    if bits(w, 21, 30) == 38 {
                        self.cpu.fpscr |= bit;
                    } else {
                        self.cpu.fpscr &= !bit;
                    }
                    self.apply_rounding();
                }
                64 => {
                    // mcrfs
                    let src = (self.cpu.fpscr >> (28 - 4 * (fa as u32 >> 2))) & 0xF;
                    self.set_crf(fd as u32 >> 2, src);
                }
                x => return Err(Error::new(pc, format!("unsupported opcode 63/{x}"))),
            }
        }
        if w & 1 != 0 {
            self.set_crf(1, self.cpu.fpscr >> 28);
        }
        Ok(None)
    }

    fn apply_rounding(&self) {
        vmx::set_mxcsr(self.mxcsr | rounding_bits(self.cpu.fpscr));
    }

    // ------------------------------------------------------------------- VMX, opcode 4/5/6

    fn op4(&mut self, g: &mut Guest, w: u32) -> Result<Option<u32>> {
        let pc = self.cpu.pc;
        // VMX128 memory forms: bits 21..27 and 30..31.
        let key = (bits(w, 21, 27) << 4) | bits(w, 30, 31);
        let mem_xo = match key {
            0b00000000011 => Some(6),   // lvsl128
            0b00001000011 => Some(38),  // lvsr128
            0b00010000011 => Some(71),  // lvewx128
            0b00011000011 => Some(103), // lvx128
            0b00110000011 => Some(199), // stvewx128
            0b00111000011 => Some(231), // stvx128
            0b01011000011 => Some(359), // lvxl128
            0b01111000011 => Some(487), // stvxl128
            0b10000000011 => Some(519), // lvlx128
            0b10001000011 => Some(551), // lvrx128
            0b10100000011 => Some(647), // stvlx128
            0b10101000011 => Some(679), // stvrx128
            0b11000000011 => Some(775), // lvlxl128
            0b11001000011 => Some(807), // lvrxl128
            0b11100000011 => Some(903), // stvlxl128
            0b11101000011 => Some(935), // stvrxl128
            _ => None,
        };
        if let Some(xo) = mem_xo {
            let vd = bits(w, 6, 10) | (bits(w, 28, 29) << 5);
            return self.vmx_memory(g, xo, vd, bits(w, 11, 15), bits(w, 16, 20));
        }
        let vd = bits(w, 6, 10) as usize;
        let va = self.cpu.v[bits(w, 11, 15) as usize];
        let vb = self.cpu.v[bits(w, 16, 20) as usize];
        let uimm = bits(w, 11, 15);
        let xo11 = bits(w, 21, 31);
        let res = unsafe {
            match xo11 {
                0 => Some(bytewise(va, vb, |a, b| a.wrapping_add(b))), // vaddubm
                4 => Some(bytewise(va, vb, |a, b| a.rotate_left(b as u32 & 7))), // vrlb
                12 => Some(merge_bytes(va, vb, 0)),                    // vmrghb
                66 => Some(halfwise(va, vb, |a, b| a.max(b))),         // vmaxuh
                68 => Some(halfwise(va, vb, |a, b| a.rotate_left(b as u32 & 15))), // vrlh
                76 => Some(merge_halves(va, vb, 0)),                   // vmrghh
                258 => Some(bytewise(va, vb, |a, b| (a as i8).max(b as i8) as u8)), // vmaxsb
                260 => Some(bytewise(va, vb, |a, b| a.wrapping_shl(b as u32 & 7))), // vslb
                268 => Some(merge_bytes(va, vb, 8)),                   // vmrglb
                322 => Some(halfwise(va, vb, |a, b| (a as i16).max(b as i16) as u16)), // vmaxsh
                324 => Some(halfwise(va, vb, |a, b| a.wrapping_shl(b as u32 & 15))), // vslh
                332 => Some(merge_halves(va, vb, 4)),                  // vmrglh
                452 => Some(va << (byte(vb, 15) & 7)),                 // vsl
                516 => Some(bytewise(va, vb, |a, b| a.wrapping_shr(b as u32 & 7))), // vsrb
                578 => Some(halfwise(va, vb, |a, b| a.min(b))),        // vminuh
                580 => Some(halfwise(va, vb, |a, b| a.wrapping_shr(b as u32 & 15))), // vsrh
                708 => Some(va >> (byte(vb, 15) & 7)),                 // vsr
                770 => Some(bytewise(va, vb, |a, b| (a as i8).min(b as i8) as u8)), // vminsb
                772 => Some(bytewise(va, vb, |a, b| ((a as i8) >> (b & 7)) as u8)), // vsrab
                834 => Some(halfwise(va, vb, |a, b| (a as i16).min(b as i16) as u16)), // vminsh
                836 => Some(halfwise(va, vb, |a, b| ((a as i16) >> (b & 15)) as u16)), // vsrah
                1024 => Some(bytewise(va, vb, |a, b| a.wrapping_sub(b))), // vsububm
                1088 => Some(halfwise(va, vb, |a, b| a.wrapping_sub(b))), // vsubuhm
                2 => Some(bytewise(va, vb, |a, b| a.max(b))),          // vmaxub
                10 => Some(from_ps(vmx::vaddfp(m128(va), m128(vb)))),
                14 => Some(pack_mod16(va, vb)),                        // vpkuhum
                64 => Some(halfwise(va, vb, |a, b| a.wrapping_add(b))), // vadduhm
                74 => Some(from_ps(vmx::vsubfp(m128(va), m128(vb)))),
                78 => Some(pack_mod32(va, vb)),                        // vpkuwum
                128 => Some(wordwise(va, vb, |a, b| a.wrapping_add(b))), // vadduwm
                130 => Some(wordwise(va, vb, |a, b| a.max(b))),        // vmaxuw
                132 => Some(wordwise(va, vb, |a, b| a.rotate_left(b & 31))), // vrlw
                140 => Some(from_elems([elem(va, 0), elem(vb, 0), elem(va, 1), elem(vb, 1)])), // vmrghw
                142 => Some(pack_sat_uh(va, vb)),                      // vpkuhus
                206 => Some(pack_sat_uw(va, vb)),                      // vpkuwus
                266 => Some(from_ps(vmx::vrefp(m128(vb)))),
                270 => Some(pack_sat_hu(va, vb)),                      // vpkshus
                330 => Some(self.vrsqrte(vb)),
                334 => Some(pack_sat_wu(va, vb)),                      // vpkswus
                386 => Some(wordwise(va, vb, |a, b| (a as i32).max(b as i32) as u32)), // vmaxsw
                388 => Some(wordwise(va, vb, |a, b| a.wrapping_shl(b & 31))), // vslw
                394 => Some(from_ps(vmx::vexptefp(m128(vb)))),
                396 => Some(from_elems([elem(va, 2), elem(vb, 2), elem(va, 3), elem(vb, 3)])), // vmrglw
                398 => Some(pack_sat_hs(va, vb)),                      // vpkshss
                458 => Some(from_ps(vmx::vlogefp(m128(vb)))),
                462 => Some(pack_sat_ws(va, vb)),                      // vpkswss
                514 => Some(bytewise(va, vb, |a, b| a.min(b))),        // vminub
                522 => Some(from_ps(vmx::vrfin(m128(vb)))),
                524 => Some(splat_byte(vb, uimm & 15)),                // vspltb
                526 => Some(unpack_hi_sb(vb)),                         // vupkhsb
                586 => Some(from_ps(vmx::vrfiz(m128(vb)))),
                588 => Some(splat_half(vb, uimm & 7)),                 // vsplth
                590 => Some(unpack_hi_sh(vb)),                         // vupkhsh
                642 => Some(wordwise(va, vb, |a, b| a.min(b))),        // vminuw
                644 => Some(wordwise(va, vb, |a, b| a.wrapping_shr(b & 31))), // vsrw
                650 => Some(from_ps(vmx::vrfip(m128(vb)))),
                652 => Some(splat_word(vb, uimm & 3)),                 // vspltw
                654 => Some(unpack_lo_sb(vb)),                         // vupklsb
                714 => Some(from_ps(vmx::vrfim(m128(vb)))),
                718 => Some(unpack_lo_sh(vb)),                         // vupklsh
                778 => Some(vcfux(vb, uimm)),
                780 => Some(splat_byte_imm(uimm)),                     // vspltisb
                832 => Some(from_si(vmx::vaddshs(m128i(va), m128i(vb)))), // vaddshs
                842 => Some(vcfsx(vb, uimm)),
                844 => Some(splat_half_imm(uimm)),                     // vspltish
                896 => Some(from_si(vmx::vaddsws(m128i(va), m128i(vb)))), // vaddsws (modular in RexGlue)
                898 => Some(wordwise(va, vb, |a, b| (a as i32).min(b as i32) as u32)), // vminsw
                900 => Some(wordwise(va, vb, |a, b| ((a as i32) >> (b & 31)) as u32)), // vsraw
                906 => Some(vctuxs(vb, uimm)),
                908 => Some(splat_word_imm(uimm)),                     // vspltisw
                970 => Some(vctsxs(vb, uimm)),
                1028 => Some(va & vb),
                1034 => Some(from_ps(vmx::vmaxfp(m128(va), m128(vb)))),
                1036 => Some(vslo(va, vb)),
                1092 => Some(va & !vb),
                1098 => Some(from_ps(vmx::vminfp(m128(va), m128(vb)))),
                1100 => Some(vsro(va, vb)),
                1152 => Some(wordwise(va, vb, |a, b| a.wrapping_sub(b))), // vsubuwm
                1156 => Some(va | vb),
                1220 => Some(va ^ vb),
                1284 => Some(!(va | vb)),
                1920 => Some(from_si(vmx::vsubsws(m128i(va), m128i(vb)))), // vsubsws (modular in RexGlue)
                _ => None,
            }
        };
        if let Some(v) = res {
            self.cpu.v[vd] = v;
            return Ok(None);
        }
        // Compare forms (VXR: xo at 22..31, Rc at 21).
        let rc = bits(w, 21, 21) != 0;
        let cmp = unsafe {
            match bits(w, 22, 31) {
                6 => Some(bytewise(va, vb, |a, b| if a == b { 0xFF } else { 0 })),
                70 => Some(halfwise(va, vb, |a, b| if a == b { 0xFFFF } else { 0 })),
                134 => Some(wordwise(va, vb, |a, b| if a == b { u32::MAX } else { 0 })),
                198 => Some(from_ps(vmx::vcmpeqfp(m128(va), m128(vb)))),
                454 => Some(from_ps(vmx::vcmpgefp(m128(va), m128(vb)))),
                518 => Some(bytewise(va, vb, |a, b| if a > b { 0xFF } else { 0 })),
                582 => Some(halfwise(va, vb, |a, b| if a > b { 0xFFFF } else { 0 })),
                646 => Some(wordwise(va, vb, |a, b| if a > b { u32::MAX } else { 0 })),
                710 => Some(from_ps(vmx::vcmpgtfp(m128(va), m128(vb)))),
                774 => Some(bytewise(va, vb, |a, b| if (a as i8) > (b as i8) { 0xFF } else { 0 })),
                838 => Some(halfwise(va, vb, |a, b| if (a as i16) > (b as i16) { 0xFFFF } else { 0 })),
                902 => Some(wordwise(va, vb, |a, b| if (a as i32) > (b as i32) { u32::MAX } else { 0 })),
                966 => Some(vcmpbfp(va, vb)),
                _ => None,
            }
        };
        if let Some(v) = cmp {
            self.cpu.v[vd] = v;
            if rc {
                self.update_cr6(v);
            }
            return Ok(None);
        }
        // VA forms (xo at 26..31).
        let vc = self.cpu.v[bits(w, 21, 25) as usize];
        let va_res = unsafe {
            match bits(w, 26, 31) {
                42 => Some(from_si(vmx::vsel(m128i(va), m128i(vb), m128i(vc)))),
                43 => Some(permute(vc, va, vb)),
                44 => Some(vsldoi(va, vb, bits(w, 22, 25))),
                46 => Some(self.vmadd(va, vc, vb)),
                47 => Some(self.vnmsub(va, vc, vb)),
                _ => None,
            }
        };
        if let Some(v) = va_res {
            self.cpu.v[vd] = v;
            return Ok(None);
        }
        // vsldoi128 (bit 27)
        if bits(w, 27, 27) == 1 {
            let (vd, va, vb) = vx128_regs(w);
            let sh = bits(w, 22, 25);
            self.cpu.v[vd] = vsldoi(self.cpu.v[va], self.cpu.v[vb], sh);
            return Ok(None);
        }
        Err(Error::new(pc, format!("unsupported VMX opcode 4 word {w:08X}")))
    }

    fn update_cr6(&mut self, v: u128) {
        let all = v == u128::MAX;
        let none = v == 0;
        self.set_crf(6, ((all as u32) << 3) | ((none as u32) << 1));
    }

    fn op5(&mut self, w: u32) -> Result<Option<u32>> {
        let (vd, va, vb) = vx128_regs(w);
        let a = self.cpu.v[va];
        let b = self.cpu.v[vb];
        let d = self.cpu.v[vd];
        // vperm128: bits 22 and 27 both zero; VC is three bits at 23..25.
        if bits(w, 22, 22) == 0 && bits(w, 27, 27) == 0 {
            let vc = bits(w, 23, 25) as usize;
            self.cpu.v[vd] = permute(self.cpu.v[vc], a, b);
            return Ok(None);
        }
        let res = unsafe {
            match (bits(w, 22, 25) << 2) | bits(w, 27, 27) {
                0b000001 => from_ps(vmx::vaddfp(m128(a), m128(b))),
                0b000101 => from_ps(vmx::vsubfp(m128(a), m128(b))),
                0b001001 => from_ps(vmx::vmulfp(m128(a), m128(b))),
                0b001101 => self.vmadd(a, b, d), // (VA*VB)+VD
                0b010001 => self.vmadd(a, d, b), // (VA*VD)+VB
                0b010101 => self.vnmsub(a, b, d), // VD-(VA*VB)
                0b011001 => self.vmsum(a, b, 3),
                0b011101 => self.vmsum(a, b, 4),
                0b100000 => pack_sat_hs(a, b),                                 // vpkshss128
                0b100001 => a & b,
                0b100100 => pack_sat_hu(a, b),                                 // vpkshus128
                0b100101 => a & !b,
                0b101000 => pack_sat_ws(a, b),                                 // vpkswss128
                0b101001 => !(a | b),
                0b101100 => pack_sat_wu(a, b),                                 // vpkswus128
                0b101101 => a | b,
                0b110000 => pack_mod16(a, b),                                  // vpkuhum128
                0b110001 => a ^ b,
                0b110100 => pack_sat_uh(a, b),                                 // vpkuhus128
                0b110101 => from_si(vmx::vsel(m128i(a), m128i(b), m128i(d))),  // vsel128
                0b111000 => pack_mod32(a, b),                                  // vpkuwum128
                0b111001 => vslo(a, b),
                0b111100 => pack_sat_uw(a, b),                                 // vpkuwus128
                0b111101 => vsro(a, b),
                _ => {
                    return Err(Error::new(self.cpu.pc, format!("unsupported VMX128 opcode 5 word {w:08X}")));
                }
            }
        };
        self.cpu.v[vd] = res;
        Ok(None)
    }

    fn op6(&mut self, w: u32) -> Result<Option<u32>> {
        let pc = self.cpu.pc;
        let vd = (bits(w, 6, 10) | (bits(w, 28, 29) << 5)) as usize;
        let vb_idx = (bits(w, 16, 20) | (bits(w, 30, 31) << 5)) as usize;
        let vb = self.cpu.v[vb_idx];
        // vpermwi128
        if (bits(w, 21, 22) << 5 | bits(w, 26, 27)) == 0b0100001 {
            let perm = bits(w, 11, 15) | (bits(w, 23, 25) << 5);
            let pick = |shift: u32| elem(vb, (perm >> shift) & 3);
            self.cpu.v[vd] = from_elems([pick(6), pick(4), pick(2), pick(0)]);
            return Ok(None);
        }
        let imm = bits(w, 11, 15);
        match (bits(w, 21, 23) << 4) | bits(w, 26, 27) {
            0b1100001 => {
                // vpkd3d128
                let z = bits(w, 24, 25);
                self.cpu.v[vd] = vpkd3d(self.cpu.v[vd], vb, imm >> 2, imm & 3, z)
                    .ok_or_else(|| Error::new(pc, format!("unsupported vpkd3d128 type {}", imm >> 2)))?;
                return Ok(None);
            }
            0b1110001 => {
                // vrlimi128
                let z = bits(w, 24, 25);
                let rotated = if z == 0 {
                    vb
                } else {
                    let e = |i: u32| elem(vb, (i + z) & 3);
                    from_elems([e(0), e(1), e(2), e(3)])
                };
                let old = self.cpu.v[vd];
                let pick = |i: u32| {
                    if (imm >> (3 - i)) & 1 != 0 { elem(rotated, i) } else { elem(old, i) }
                };
                self.cpu.v[vd] = from_elems([pick(0), pick(1), pick(2), pick(3)]);
                return Ok(None);
            }
            _ => {}
        }
        let res = unsafe {
            match bits(w, 21, 27) {
                0b0100011 => Some(vctsxs(vb, imm)),  // vcfpsxws128
                0b0100111 => Some(vctuxs(vb, imm)),  // vcfpuxws128
                0b0101011 => Some(vcfsx(vb, imm)),   // vcsxwfp128
                0b0101111 => Some(vcfux(vb, imm)),   // vcuxwfp128
                0b0110011 => Some(from_ps(vmx::vrfim(m128(vb)))),
                0b0110111 => Some(from_ps(vmx::vrfin(m128(vb)))),
                0b0111011 => Some(from_ps(vmx::vrfip(m128(vb)))),
                0b0111111 => Some(from_ps(vmx::vrfiz(m128(vb)))),
                0b1100011 => Some(from_ps(vmx::vrefp(m128(vb)))),
                0b1100111 => Some(self.vrsqrte(vb)),
                0b1101011 => Some(from_ps(vmx::vexptefp(m128(vb)))),
                0b1101111 => Some(from_ps(vmx::vlogefp(m128(vb)))),
                0b1110011 => Some(splat_word(vb, imm & 3)),
                0b1110111 => Some(splat_word_imm(imm)),
                0b1111111 => {
                    Some(vupkd3d(vb, imm >> 2).ok_or_else(|| {
                        Error::new(pc, format!("unsupported vupkd3d128 type {}", imm >> 2))
                    })?)
                }
                _ => None,
            }
        };
        if let Some(v) = res {
            self.cpu.v[vd] = v;
            return Ok(None);
        }
        let (vd, va, vb) = vx128_regs(w);
        let a = self.cpu.v[va];
        let b = self.cpu.v[vb];
        // Compares (Rc at bit 25).
        let rc = bits(w, 25, 25) != 0;
        let cmp = unsafe {
            match (bits(w, 22, 24) << 3) | bits(w, 27, 27) {
                0b000000 => Some(from_ps(vmx::vcmpeqfp(m128(a), m128(b)))),
                0b001000 => Some(from_ps(vmx::vcmpgefp(m128(a), m128(b)))),
                0b010000 => Some(from_ps(vmx::vcmpgtfp(m128(a), m128(b)))),
                0b011000 => Some(vcmpbfp(a, b)),
                0b100000 => Some(wordwise(a, b, |x, y| if x == y { u32::MAX } else { 0 })),
                _ => None,
            }
        };
        if let Some(v) = cmp {
            self.cpu.v[vd] = v;
            if rc {
                self.update_cr6(v);
            }
            return Ok(None);
        }
        let res = unsafe {
            match (bits(w, 22, 25) << 2) | bits(w, 27, 27) {
                0b000101 => wordwise(a, b, |x, y| x.rotate_left(y & 31)), // vrlw128
                0b001101 => wordwise(a, b, |x, y| x.wrapping_shl(y & 31)), // vslw128
                0b010101 => wordwise(a, b, |x, y| ((x as i32) >> (y & 31)) as u32), // vsraw128
                0b011101 => wordwise(a, b, |x, y| x.wrapping_shr(y & 31)), // vsrw128
                0b101000 => from_ps(vmx::vmaxfp(m128(a), m128(b))),
                0b101100 => from_ps(vmx::vminfp(m128(a), m128(b))),
                0b110000 => from_elems([elem(a, 0), elem(b, 0), elem(a, 1), elem(b, 1)]), // vmrghw128
                0b110100 => from_elems([elem(a, 2), elem(b, 2), elem(a, 3), elem(b, 3)]), // vmrglw128
                0b111000 => {
                    if va == 0x60 { unpack_hi_sh(b) } else { unpack_hi_sb(b) }
                }
                0b111100 => {
                    if va == 0x60 { unpack_lo_sh(b) } else { unpack_lo_sb(b) }
                }
                _ => return Err(Error::new(pc, format!("unsupported VMX128 opcode 6 word {w:08X}"))),
            }
        };
        self.cpu.v[vd] = res;
        Ok(None)
    }
}

/// FPSCR[RN] as MXCSR rounding bits.
fn rounding_bits(fpscr: u32) -> u32 {
    match fpscr & 3 {
        0 => 0x0000, // nearest
        1 => 0x6000, // toward zero
        2 => 0x4000, // toward +inf
        _ => 0x2000, // toward -inf
    }
}

fn round_fpscr(value: f64, fpscr: u32) -> f64 {
    match fpscr & 3 {
        0 => value.round_ties_even(),
        1 => value.trunc(),
        2 => value.ceil(),
        _ => value.floor(),
    }
}

/// VX128 register fields: VD, VA (7 bits with the extra high bit at 21), VB.
fn vx128_regs(w: u32) -> (usize, usize, usize) {
    let vd = bits(w, 6, 10) | (bits(w, 28, 29) << 5);
    let va = bits(w, 11, 15) | (bits(w, 26, 26) << 5) | (bits(w, 21, 21) << 6);
    let vb = bits(w, 16, 20) | (bits(w, 30, 31) << 5);
    (vd as usize, va as usize, vb as usize)
}

// -------------------------------------------------------------------------- vector memory ops

fn load16(g: &Guest, ea: u32) -> Result<u128> {
    let s = g.span(ea, 16)?;
    Ok(u128::from_be_bytes(s.try_into().expect("16 bytes")))
}

fn store16(g: &mut Guest, ea: u32, v: u128) -> Result<()> {
    g.set_span(ea, &v.to_be_bytes())
}

/// `lvlx`: the bytes from `ea` to the end of its 16-byte block, left-justified, zero-filled.
fn lvlx(g: &Guest, ea: u32) -> Result<u128> {
    let base = load16(g, ea & !15)?;
    let sh = ea & 15;
    Ok(if sh == 0 { base } else { base << (8 * sh) })
}

/// `lvrx`: the bytes of the block before `ea`, right-justified; nothing is loaded when aligned.
fn lvrx(g: &Guest, ea: u32) -> Result<u128> {
    let sh = ea & 15;
    if sh == 0 {
        return Ok(0);
    }
    let base = load16(g, ea & !15)?;
    Ok(base >> (8 * (16 - sh)))
}

fn stvlx(g: &mut Guest, ea: u32, v: u128) -> Result<()> {
    let sh = ea & 15;
    let bytes = v.to_be_bytes();
    for k in 0..(16 - sh) {
        g.set_u8(ea + k, bytes[k as usize])?;
    }
    Ok(())
}

fn stvrx(g: &mut Guest, ea: u32, v: u128) -> Result<()> {
    let sh = ea & 15;
    if sh == 0 {
        return Ok(());
    }
    let bytes = v.to_be_bytes();
    let base = ea & !15;
    for k in 0..sh {
        g.set_u8(base + k, bytes[(16 - sh + k) as usize])?;
    }
    Ok(())
}

// ----------------------------------------------------------------------- lane-wise helpers

/// `vmrghb`/`vmrglb`: bytes `from..from + 8` of `a` and `b`, interleaved.
fn merge_bytes(a: u128, b: u128, from: u32) -> u128 {
    from_bytes(std::array::from_fn(|k| {
        let src = if k % 2 == 0 { a } else { b };
        byte(src, from + k as u32 / 2)
    }))
}

/// `vmrghh`/`vmrglh`: halfwords `from..from + 4` of `a` and `b`, interleaved.
fn merge_halves(a: u128, b: u128, from: usize) -> u128 {
    let (ha, hb) = (halves(a), halves(b));
    from_halves(std::array::from_fn(|k| if k % 2 == 0 { ha[from + k / 2] } else { hb[from + k / 2] }))
}

fn bytewise(a: u128, b: u128, f: impl Fn(u8, u8) -> u8) -> u128 {
    from_bytes(std::array::from_fn(|k| f(byte(a, k as u32), byte(b, k as u32))))
}
fn halfwise(a: u128, b: u128, f: impl Fn(u16, u16) -> u16) -> u128 {
    let (x, y) = (halves(a), halves(b));
    from_halves(std::array::from_fn(|k| f(x[k], y[k])))
}
fn wordwise(a: u128, b: u128, f: impl Fn(u32, u32) -> u32) -> u128 {
    from_elems(std::array::from_fn(|i| f(elem(a, i as u32), elem(b, i as u32))))
}
fn splat_word(v: u128, i: u32) -> u128 {
    let e = elem(v, i);
    from_elems([e; 4])
}
fn splat_half(v: u128, i: u32) -> u128 {
    let h = halves(v)[i as usize];
    from_halves([h; 8])
}
fn splat_byte(v: u128, i: u32) -> u128 {
    from_bytes([byte(v, i); 16])
}
fn simm5(imm: u32) -> i32 {
    if imm & 0x10 != 0 { (imm | 0xFFFF_FFE0) as i32 } else { imm as i32 }
}
fn splat_word_imm(imm: u32) -> u128 {
    from_elems([simm5(imm) as u32; 4])
}
fn splat_half_imm(imm: u32) -> u128 {
    from_halves([simm5(imm) as u16; 8])
}
fn splat_byte_imm(imm: u32) -> u128 {
    from_bytes([simm5(imm) as u8; 16])
}

fn vsldoi(a: u128, b: u128, sh: u32) -> u128 {
    if sh == 0 {
        a
    } else {
        (a << (8 * sh)) | (b >> (8 * (16 - sh)))
    }
}
fn vslo(a: u128, b: u128) -> u128 {
    let sh = (byte(b, 15) >> 3) & 15;
    if sh == 0 { a } else { a << (8 * sh as u32) }
}
fn vsro(a: u128, b: u128) -> u128 {
    let sh = (byte(b, 15) >> 3) & 15;
    if sh == 0 { a } else { a >> (8 * sh as u32) }
}

/// `vcfsx`/`vcsxwfp128`: signed words to float, scaled by 2^-uimm.
unsafe fn vcfsx(v: u128, uimm: u32) -> u128 {
    unsafe {
        let f = _mm_cvtepi32_ps(m128i(v));
        let f = if uimm != 0 {
            _mm_mul_ps(f, _mm_set1_ps((-(uimm as i32) as f32).exp2()))
        } else {
            f
        };
        from_ps(f)
    }
}
/// `vcfux`/`vcuxwfp128`.
unsafe fn vcfux(v: u128, uimm: u32) -> u128 {
    unsafe {
        let f = vmx::cvtepu32_ps(m128i(v));
        let f = if uimm != 0 {
            _mm_mul_ps(f, _mm_set1_ps((-(uimm as i32) as f32).exp2()))
        } else {
            f
        };
        from_ps(f)
    }
}
/// `vctsxs`/`vcfpsxws128`: float times 2^uimm to saturated signed words.
unsafe fn vctsxs(v: u128, uimm: u32) -> u128 {
    unsafe {
        let scaled = _mm_mul_ps(m128(v), _mm_set1_ps((uimm as f32).exp2()));
        from_si(vmx::vctsxs(scaled))
    }
}
/// `vctuxs`/`vcfpuxws128`.
unsafe fn vctuxs(v: u128, uimm: u32) -> u128 {
    unsafe {
        let scaled = _mm_mul_ps(m128(v), _mm_set1_ps((uimm as f32).exp2()));
        from_si(vmx::vctuxs(scaled))
    }
}

fn vcmpbfp(a: u128, b: u128) -> u128 {
    wordwise(a, b, |x, y| {
        let (x, y) = (f32::from_bits(x), f32::from_bits(y));
        let gt = x > y;
        let lt = !(x >= -y);
        ((gt as u32) << 31) | ((lt as u32) << 30)
    })
}

fn pack_mod16(a: u128, b: u128) -> u128 {
    let (x, y) = (halves(a), halves(b));
    from_bytes(std::array::from_fn(|k| if k < 8 { x[k] as u8 } else { y[k - 8] as u8 }))
}
fn pack_mod32(a: u128, b: u128) -> u128 {
    from_halves(std::array::from_fn(|k| {
        if k < 4 { elem(a, k as u32) as u16 } else { elem(b, k as u32 - 4) as u16 }
    }))
}
fn pack_sat_hs(a: u128, b: u128) -> u128 {
    let (x, y) = (halves(a), halves(b));
    from_bytes(std::array::from_fn(|k| {
        let h = if k < 8 { x[k] } else { y[k - 8] } as i16;
        h.clamp(-128, 127) as i8 as u8
    }))
}
fn pack_sat_hu(a: u128, b: u128) -> u128 {
    let (x, y) = (halves(a), halves(b));
    from_bytes(std::array::from_fn(|k| {
        let h = if k < 8 { x[k] } else { y[k - 8] } as i16;
        h.clamp(0, 255) as u8
    }))
}
fn pack_sat_ws(a: u128, b: u128) -> u128 {
    from_halves(std::array::from_fn(|k| {
        let w = if k < 4 { elem(a, k as u32) } else { elem(b, k as u32 - 4) } as i32;
        w.clamp(-32768, 32767) as i16 as u16
    }))
}
fn pack_sat_uh(a: u128, b: u128) -> u128 {
    let (x, y) = (halves(a), halves(b));
    from_bytes(std::array::from_fn(|k| {
        let h = if k < 8 { x[k] } else { y[k - 8] };
        h.min(255) as u8
    }))
}
fn pack_sat_wu(a: u128, b: u128) -> u128 {
    from_halves(std::array::from_fn(|k| {
        let w = if k < 4 { elem(a, k as u32) } else { elem(b, k as u32 - 4) } as i32;
        w.clamp(0, 0xFFFF) as u16
    }))
}
fn pack_sat_uw(a: u128, b: u128) -> u128 {
    from_halves(std::array::from_fn(|k| {
        let w = if k < 4 { elem(a, k as u32) } else { elem(b, k as u32 - 4) };
        w.min(0xFFFF) as u16
    }))
}
fn unpack_hi_sb(v: u128) -> u128 {
    from_halves(std::array::from_fn(|k| byte(v, k as u32) as i8 as i16 as u16))
}
fn unpack_lo_sb(v: u128) -> u128 {
    from_halves(std::array::from_fn(|k| byte(v, 8 + k as u32) as i8 as i16 as u16))
}
fn unpack_hi_sh(v: u128) -> u128 {
    let h = halves(v);
    from_elems(std::array::from_fn(|k| h[k] as i16 as i32 as u32))
}
fn unpack_lo_sh(v: u128) -> u128 {
    let h = halves(v);
    from_elems(std::array::from_fn(|k| h[4 + k] as i16 as i32 as u32))
}

/// `vupkd3d128`, the D3D unpack formats Xenia implements; `None` for the rest.
fn vupkd3d(v: u128, kind: u32) -> Option<u128> {
    let x = elem(v, 3);
    Some(match kind {
        0 => {
            // D3DCOLOR (ARGB bytes) to 1.0 + n/2^22 floats
            let c = |shift: u32| 0x3F80_0000 | ((x >> shift) & 0xFF);
            from_elems([c(16), c(8), c(0), c(24)])
        }
        1 => {
            // NORMSHORT2: 3.0 + n/2^22 for x, y; z = 0, w = 1
            let s = |shift: u32| 0x4040_0000u32.wrapping_add(((x >> shift) as u16 as i16) as i32 as u32);
            from_elems([s(16), s(0), 0, 0x3F80_0000])
        }
        3 => {
            // FLOAT16_2
            let h = |shift: u32| half_to_float((x >> shift) as u16).to_bits();
            from_elems([h(16), h(0), 0, 0x3F80_0000])
        }
        5 => {
            let y = elem(v, 2);
            let h = |w: u32, shift: u32| half_to_float((w >> shift) as u16).to_bits();
            from_elems([h(y, 16), h(y, 0), h(x, 16), h(x, 0)])
        }
        _ => return None,
    })
}

/// `vpkd3d128` for the formats this title uses; `None` for the rest.
fn vpkd3d(old: u128, v: u128, kind: u32, pack: u32, shift: u32) -> Option<u128> {
    let packed: u128 = match kind {
        0 => {
            // D3DCOLOR from floats biased as 1.0 + n/2^22
            let b = |i: u32| {
                let f = fl(v, i).to_bits();
                let n = if fl(v, i) < 1.0 { 0 } else if f > 0x3F80_00FF { 0xFF } else { f & 0xFF };
                n
            };
            ((b(3) << 24) | (b(0) << 16) | (b(1) << 8) | b(2)) as u128
        }
        _ => return None,
    };
    // Xenia's permute control: which words come from the old register and which from the packed.
    let control: [(u32, u32); 4] = match (pack, shift) {
        (1, 0) => [(0, 0), (0, 1), (0, 2), (1, 3)],
        (1, 1) => [(0, 0), (0, 1), (1, 3), (0, 3)],
        (1, 2) => [(0, 0), (1, 3), (0, 2), (0, 3)],
        (1, 3) => [(1, 3), (0, 1), (0, 2), (0, 3)],
        _ => return None,
    };
    let src = [old, packed];
    Some(from_elems(std::array::from_fn(|i| {
        let (which, lane) = control[i];
        elem(src[which as usize], lane)
    })))
}

fn half_to_float(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mant = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            // subnormal half to normal float
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 31 {
        // Xbox 360 halves have no infinities; Xenia treats exp 31 as a normal exponent.
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

#[cfg(test)]
#[path = "ppc/tests.rs"]
mod tests;
