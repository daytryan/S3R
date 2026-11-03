//! TEST-ONLY oracle: Skate 2's own PowerPC code, read from the user's disc at test time and run
//! instruction by instruction in `skate-audio-core`'s interpreter. Nothing here ships and no EA
//! bytes are in the repo: the image comes from `SKATE2_IMAGE` (built by
//! `tools/build_skate2_image.py` from the disc `default.xex`) and the Attrib vault from
//! `SKATE2_VAULT` (a directory holding the disc's `data/db/skater.vlt` and `skater.bin`).
//!
//! Every host fake is listed in [`Fakes`] and in `docs/superpowers/S2_PHYSICS_PILOT.md`.

use std::collections::HashMap;

use skate_audio_core::attrib::{Data, Vault};
use skate_audio_core::ppc::{Cpu, Host, Interp};
use skate_audio_core::{Error, Guest, Result, Segment};

use super::float::V4;

pub const IMAGE_BASE: u32 = 0x8200_0000;
pub const IMAGE_BYTES: u32 = 0x00F1_0000;

/// `GetAttributePointer(collection, field key, index)`: the one call out of the physics code that
/// the host answers (from the vault, by `tools/asset_pipeline/vlt.py`'s rules).
pub const GET_ATTRIBUTE_POINTER: u32 = 0x8237_39F8;
/// `*(0x82D8EC58)`: the physics tweakables table built by `0x826FA718` at boot; `+176` is the
/// `physics_push` instance the push reads.
pub const TWEAKABLES: u32 = 0x82D8_EC58;
/// Import thunks (pristine `0x01LLOOOO, 0x02LLOOOO, mtctr r11, bctr`); any call into one stops.
const THUNKS: std::ops::Range<u32> = 0x82D0_7F5C..0x82D0_91A0;

const STACK_TOP: u32 = 0x7000_0000;
const STACK_BYTES: u32 = 0x0001_0000;
const HEAP: u32 = 0x6000_0000;
const HEAP_BYTES: u32 = 0x0010_0000;
const VAULT_VLT: u32 = 0x7100_0000;
const VAULT_BIN: u32 = 0x7200_0000;

pub const CLASS_PHYSICS_PUSH: u64 = 0xD4D0_4D2F_A87A_9A31; // hash64("physics_push")
pub const CLASS_PHYSICSDECK: u64 = 0x9ED2_91A2_7445_3FC0; // hash64("physicsdeck")
pub const COLLECTION_DEFAULT: u64 = 0xD7ED_BD36_2D7D_2152; // hash64("default")

/// What the harness stands in for, counted so a test can show nothing else was reached.
#[derive(Default, Debug, Clone)]
pub struct Fakes {
    /// `GetAttributePointer` answered from the vault: (class, collection, field, returned address).
    pub attribute_reads: Vec<(u64, u64, u64, u32)>,
}

struct S2Host {
    vault: Vault,
    /// Collection object address -> (class, collection key).
    objects: HashMap<u32, (u64, u64)>,
    fakes: Fakes,
}

impl S2Host {
    /// `vlt.py` rules: in-layout field at `layout + offset`; otherwise the collection's entry
    /// (inline in the `.vlt` or in the `.bin`), walking parents; arrays index past their header.
    fn attribute(&self, g: &Guest, object: u32, field: u64, index: u32) -> Result<u32> {
        let Some(&(class, mut key)) = self.objects.get(&object) else { return Ok(0) };
        let Some(spec) = self.vault.classes.get(&class).and_then(|c| c.fields.get(&field)) else {
            return Ok(0);
        };
        let base = loop {
            let Some(c) = self.vault.collections.get(&(class, key)) else { return Ok(0) };
            if spec.flags & 2 != 0 {
                if let Some(layout) = c.layout {
                    break VAULT_BIN + layout + u32::from(spec.offset);
                }
            } else if let Some(data) = c.entries.get(&field) {
                break match *data {
                    Data::Inline(at) => VAULT_VLT + at,
                    Data::Bin(at) => VAULT_BIN + at,
                };
            }
            if c.parent == 0 {
                return Ok(0);
            }
            key = c.parent;
        };
        if spec.flags & 1 == 0 {
            return Ok(if index == 0 { base } else { 0 });
        }
        let (count, stride) = (u32::from(g.u16(base + 2)?), u32::from(g.u16(base + 4)?));
        if index >= count {
            return Ok(0);
        }
        let boundary = 1u32 << spec.alignment;
        Ok(((base + 8 + boundary - 1) & !(boundary - 1)) + stride * index)
    }
}

impl Host for S2Host {
    fn hook(&mut self, cpu: &mut Cpu, g: &mut Guest, target: u32) -> Result<()> {
        if target == GET_ATTRIBUTE_POINTER {
            let (object, field, index) = (cpu.r[3] as u32, cpu.r[4], cpu.r[5] as u32);
            let at = self.attribute(g, object, field, index)?;
            let (class, key) = self.objects.get(&object).copied().unwrap_or_default();
            self.fakes.attribute_reads.push((class, key, field, at));
            cpu.r[3] = u64::from(at);
            return Ok(());
        }
        let word = g.u32(target).unwrap_or(0);
        Err(Error::new(target, format!("not faked (thunk word {word:08X}); the oracle stops here")))
    }
}

pub struct Oracle {
    pub g: Guest,
    pub cpu: Interp,
    host: S2Host,
    heap_next: u32,
}

/// The disc image and vault, or `None` when the env vars are not set (the tests are `#[ignore]`d).
pub fn load() -> Option<Oracle> {
    let image = std::fs::read(std::env::var_os("SKATE2_IMAGE")?).expect("SKATE2_IMAGE unreadable");
    let vault_dir = std::path::PathBuf::from(std::env::var_os("SKATE2_VAULT")?);
    assert_eq!(image.len() as u32, IMAGE_BYTES, "SKATE2_IMAGE is not the Skate 2 (Europe) basefile");
    check_image(&image);
    let read = |name: &str| std::fs::read(vault_dir.join(name)).expect("SKATE2_VAULT/skater.*");
    let (vlt, bin) = (read("skater.vlt"), read("skater.bin"));
    // Skate 2's skater vault holds schema and collections in one pair.
    let vault = Vault::parse(&vlt, &bin, &vlt, &bin).expect("parse the Skate 2 vault");
    Some(Oracle::new(image, vault))
}

/// Refuse an image that is not the pristine basefile: the PE signature, the push function's first
/// instruction, and a zero `.bss` (an image from the unfixed LZX decoder fails the last one).
fn check_image(image: &[u8]) {
    assert_eq!(&image[..2], b"MZ", "SKATE2_IMAGE has no PE header");
    let word = |a: u32| u32::from_be_bytes(image[(a - IMAGE_BASE) as usize..][..4].try_into().unwrap());
    assert_eq!(word(0x8297_B7A8), 0x7D88_02A6, "0x8297B7A8 is not `mflr r12`: wrong Skate 2 build");
    let bss = &image[(0x82D8_EC00 - IMAGE_BASE) as usize..(0x82E7_57F8 - IMAGE_BASE) as usize];
    assert!(
        bss.iter().all(|&b| b == 0),
        "SKATE2_IMAGE has a non-zero .bss: rebuild it with tools/build_skate2_image.py"
    );
}

impl Oracle {
    fn new(image: Vec<u8>, vault: Vault) -> Self {
        let mut g = Guest::from_segments(vec![Segment { base: IMAGE_BASE, bytes: image }]);
        g.put(STACK_TOP - STACK_BYTES, vec![0; STACK_BYTES as usize]);
        g.put(HEAP, vec![0; HEAP_BYTES as usize]);
        vault.install(&mut g, VAULT_VLT, VAULT_BIN).expect("install vault");
        let mut cpu = Interp::new();
        cpu.hooks.insert(GET_ATTRIBUTE_POINTER);
        for at in THUNKS.step_by(16) {
            cpu.hooks.insert(at);
        }
        let mut oracle = Self {
            g,
            cpu,
            host: S2Host { vault, objects: HashMap::new(), fakes: Fakes::default() },
            heap_next: HEAP,
        };
        // The tweakables table (412 bytes); only the push slot is filled.
        let table = oracle.alloc(412);
        oracle.set_u32(TWEAKABLES, table);
        let push = oracle.instance(CLASS_PHYSICS_PUSH, COLLECTION_DEFAULT);
        oracle.set_u32(table + 176, push);
        oracle
    }

    pub fn fakes(&self) -> &Fakes {
        &self.host.fakes
    }

    /// Zeroed, 16-byte aligned guest memory from a bump heap.
    pub fn alloc(&mut self, bytes: u32) -> u32 {
        let at = self.heap_next;
        self.heap_next = (at + bytes + 15) & !15;
        assert!(self.heap_next <= HEAP + HEAP_BYTES, "oracle heap exhausted");
        self.g.fill(at, 0, bytes).unwrap();
        at
    }

    /// A 16-byte Attrib instance holder whose `+0` is a collection object the host knows; `0` for
    /// the collection pointer when `class == 0` (the code then reads its null fallback).
    pub fn instance(&mut self, class: u64, collection: u64) -> u32 {
        let holder = self.alloc(16);
        if class != 0 {
            let object = self.alloc(64);
            self.host.objects.insert(object, (class, collection));
            self.set_u32(holder, object);
        }
        holder
    }

    /// Where `GetAttributePointer` would point for `holder`'s collection and `field` (`0` when the
    /// vault has none): the test writes its own value there to vary a tweakable.
    pub fn attribute(&self, holder: u32, field: u64) -> u32 {
        self.host.attribute(&self.g, self.u32(holder), field, 0).unwrap()
    }

    pub fn set_u32(&mut self, at: u32, v: u32) {
        self.g.set_u32(at, v).unwrap();
    }
    pub fn u32(&self, at: u32) -> u32 {
        self.g.u32(at).unwrap()
    }
    pub fn set_f32(&mut self, at: u32, v: f32) {
        self.set_u32(at, v.to_bits());
    }
    pub fn f32(&self, at: u32) -> f32 {
        f32::from_bits(self.u32(at))
    }
    pub fn set_v4(&mut self, at: u32, v: V4) {
        for (i, x) in v.iter().enumerate() {
            self.set_f32(at + 4 * i as u32, *x);
        }
    }
    pub fn v4(&self, at: u32) -> V4 {
        [0, 1, 2, 3].map(|i| self.f32(at + 4 * i))
    }

    /// Call a Skate 2 function with `args` in r3.., on a fresh stack; returns the instructions run.
    pub fn call(&mut self, target: u32, args: &[u32]) -> u64 {
        self.cpu.cpu.r[1] = u64::from(STACK_TOP - 0x100);
        let args: Vec<u64> = args.iter().map(|&a| u64::from(a)).collect();
        let before = self.cpu.steps;
        if let Err(e) = self.cpu.call(&mut self.g, &mut self.host, target, &args) {
            panic!("Skate 2 {target:08X} stopped at {:08X}: {}", e.address, e.message);
        }
        self.cpu.steps - before
    }
}
