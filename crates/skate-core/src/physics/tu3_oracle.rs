//! TEST-ONLY oracle: Skate 3 TU3's own PowerPC code, run instruction by instruction in
//! `skate-audio-core`'s interpreter (the same harness as `skate2::oracle`). Nothing here ships and
//! no EA bytes are in the repo: the image is the installed runtime image
//! `$SKATE3_ASSET_ROOT/private/stock/audio-runtime-image/g_XXXX.bin`, whose pages are byte-identical
//! to the TU3 `tu3.pe` the parity audit cites (SHA-256 `f4aa113e…c4`, checked 2026-10-04).
//!
//! The interpreter runs scalar code IEEE (`mxcsr = 0x1F80`, no flush). The title has no `mtvscr`
//! and its only `mtfsf` sites are the CRT helpers `0x82F60DC0..0x82F60E40` and the `longjmp`
//! restore at `0x82F646FC`; what those are called with is NOT traced, so the FPU's NI mode is
//! assumed IEEE. Only scalar-FPU functions are a bit-exact oracle here: VMX estimate instructions
//! (`vrsqrtefp`, `vrefp`) stay host math (D11).
use skate_audio_core::ppc::{Cpu, Host, Interp, NoHost};
use skate_audio_core::{Guest, Segment};

const HEAP: u32 = 0x6000_0000;
const HEAP_BYTES: u32 = 0x0010_0000;
const STACK_TOP: u32 = 0x7000_0000;
const STACK_BYTES: u32 = 0x0001_0000;

pub struct Tu3 {
    pub g: Guest,
    cpu: Interp,
    next: u32,
}

/// The installed TU3 image, or `None` without `SKATE3_ASSET_ROOT` (the tests are `#[ignore]`d).
pub fn load() -> Option<Tu3> {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?);
    let mut segments = Vec::new();
    for entry in std::fs::read_dir(root.join("private/stock/audio-runtime-image")).ok()? {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        let Some(page) = name.strip_prefix("g_").and_then(|s| s.strip_suffix(".bin")) else {
            continue;
        };
        segments.push(Segment {
            base: u32::from_str_radix(page, 16).ok()? << 16,
            bytes: std::fs::read(&path).ok()?,
        });
    }
    segments.push(Segment { base: HEAP, bytes: vec![0; HEAP_BYTES as usize] });
    segments.push(Segment { base: STACK_TOP - STACK_BYTES, bytes: vec![0; STACK_BYTES as usize] });
    let mut cpu = Interp::new();
    cpu.mxcsr = 0x1F80;
    Some(Tu3 { g: Guest::from_segments(segments), cpu, next: HEAP })
}

impl Tu3 {
    /// Zeroed, 16-byte aligned guest memory.
    pub fn alloc(&mut self, bytes: u32) -> u32 {
        let at = self.next;
        self.next = (at + bytes + 15) & !15;
        assert!(self.next <= HEAP + HEAP_BYTES, "oracle heap exhausted");
        at
    }
    pub fn u32(&self, at: u32) -> u32 {
        self.g.u32(at).unwrap()
    }
    pub fn set_u32(&mut self, at: u32, v: u32) {
        self.g.set_u32(at, v).unwrap();
    }
    pub fn f32(&self, at: u32) -> f32 {
        f32::from_bits(self.u32(at))
    }
    pub fn set_f32(&mut self, at: u32, v: f32) {
        self.set_u32(at, v.to_bits());
    }
    pub fn set_u8(&mut self, at: u32, v: u8) {
        self.g.set_u8(at, v).unwrap();
    }
    /// Run the TU3 function at `target` with `args` in r3.. on a fresh stack.
    pub fn call(&mut self, target: u32, args: &[u32]) {
        self.cpu.cpu.r[1] = u64::from(STACK_TOP - 0x100);
        let args: Vec<u64> = args.iter().map(|&a| u64::from(a)).collect();
        if let Err(e) = self.cpu.call(&mut self.g, &mut NoHost, target, &args) {
            panic!("TU3 {target:08X} stopped at {:08X}: {}", e.address, e.message);
        }
    }
    /// Run a scalar `float f(float)` leaf: `x` in f1, the result from f1.
    pub fn call_f1(&mut self, target: u32, x: f32) -> f32 {
        self.cpu.cpu.f[1] = f64::from(x);
        self.call(target, &[]);
        self.cpu.cpu.f[1] as f32
    }
    /// [`Tu3::call`], with every branch to an address in `hooked` run by `hook(cpu, guest, target)`
    /// instead (it sets the results; execution resumes at LR).
    pub fn call_hooked(
        &mut self,
        target: u32,
        args: &[u32],
        hooked: &[u32],
        hook: &mut dyn FnMut(&mut Cpu, &mut Guest, u32),
    ) {
        struct Hooks<'a>(&'a mut dyn FnMut(&mut Cpu, &mut Guest, u32));
        impl Host for Hooks<'_> {
            fn hook(&mut self, cpu: &mut Cpu, g: &mut Guest, target: u32) -> skate_audio_core::Result<()> {
                (self.0)(cpu, g, target);
                Ok(())
            }
        }
        self.cpu.hooks = hooked.iter().copied().collect();
        self.cpu.cpu.r[1] = u64::from(STACK_TOP - 0x100);
        let args: Vec<u64> = args.iter().map(|&a| u64::from(a)).collect();
        let result = self.cpu.call(&mut self.g, &mut Hooks(hook), target, &args);
        self.cpu.hooks.clear();
        if let Err(e) = result {
            panic!("TU3 {target:08X} stopped at {:08X}: {}", e.address, e.message);
        }
    }
    /// `floats` into f1.. for the next call.
    pub fn floats(&mut self, floats: &[f32]) {
        for (i, &x) in floats.iter().enumerate() {
            self.cpu.cpu.f[1 + i] = f64::from(x);
        }
    }
    /// [`Tu3::call`] with `floats` in f1.., returning f1.
    pub fn call_floats(&mut self, target: u32, args: &[u32], floats: &[f32]) -> f32 {
        self.floats(floats);
        self.call(target, args);
        self.cpu.cpu.f[1] as f32
    }
    /// The return register r3.
    pub fn r3(&self) -> u32 {
        self.cpu.cpu.r[3] as u32
    }
    /// VMX arithmetic the port's way (`skate_audio_core::ppc::Interp::fused_vmx`): a mismatch
    /// is then structural, not a rounding convention the binary cannot decide.
    pub fn port_vmx(&mut self) {
        self.cpu.fused_vmx = true;
    }
    pub fn v4(&self, at: u32) -> [f32; 4] {
        std::array::from_fn(|i| self.f32(at + 4 * i as u32))
    }
    pub fn set_v4(&mut self, at: u32, v: [f32; 4]) {
        for (i, x) in v.iter().enumerate() {
            self.set_f32(at + 4 * i as u32, *x);
        }
    }
    pub fn u8(&self, at: u32) -> u8 {
        self.g.u8(at).unwrap()
    }
}

/// Bit equality, any NaN equal to any NaN.
pub fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Panics naming the first lane that differs.
pub fn check(what: &str, step: usize, retail: &[f32], port: &[f32]) {
    assert_eq!(retail.len(), port.len());
    for (k, (r, p)) in retail.iter().zip(port).enumerate() {
        assert!(
            same(*r, *p),
            "step {step} {what}[{k}]: retail {r:e} ({:08X}) port {p:e} ({:08X})",
            r.to_bits(),
            p.to_bits()
        );
    }
}

/// Deterministic xorshift inputs.
pub struct Rng(pub u64);
impl Rng {
    pub fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
    pub fn v4(&mut self, lo: f32, hi: f32) -> [f32; 4] {
        std::array::from_fn(|_| self.range(lo, hi))
    }
    /// A value from `lo..hi`, now and then an edge: +-0, a tiny value, or exactly `lo`/`hi`.
    pub fn edgy(&mut self, lo: f32, hi: f32) -> f32 {
        match (self.unit() * 24.0) as u32 {
            0 => 0.0,
            1 => -0.0,
            2 => 1e-7,
            3 => lo,
            4 => hi,
            _ => self.range(lo, hi),
        }
    }
}
