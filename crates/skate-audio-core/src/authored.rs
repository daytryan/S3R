//! Concrete single-thread owner of the recovered rider/board audio graph.
//!
//! The host supplies the unmodified guest constants, CSI projects, ABK banks and decoded PCM.
//! Every block runs the bank evaluator, drains its graph commands, feeds SndPlayer1's segment
//! ring, runs the installed voice and bus graphs, and submits the final six-channel Dac buffer.
//! File access and codec decoding never occur in this owner.

mod mixmap_host;
pub mod oneshot;

use std::collections::{BTreeMap, HashMap};

use crate::eval::interp;
use crate::patch::{self, Heap};
use crate::pcm::{CachedPcm, PcmStreams};
use crate::voice::{OpenRequest, VoiceDevice};
use crate::{
    Error, Guest, Result, classes, commands, device, graph, kernels, mathlib, modules, output,
    play, pump, scheduler, stream, symbols, voices, worker,
};

pub const PCM_CHANNELS: u16 = 6;
pub const PCM_SAMPLE_RATE: u32 = 48_000;
pub const PCM_FRAMES_PER_BLOCK: u32 = 256;

const SPACE: u32 = 0x4000_0000;
const SYSTEM: u32 = SPACE;
/// The bus root `*(0x830CFDBC)`, retail's 0xA8-byte audio framework object (`+8` system, `+0xC`
/// registry, `+0x10..+0x2C` its four root graphs' `{player, table}`, `+0x30..+0x50` classes,
/// `+0x54` the Splice system). The default output holder the runtime adds sits past its end.
const ROOT: u32 = SPACE + 0x1000;
/// `[root+0x2C]`: the default output target's holder (retail: the FE graph's module table), and the
/// null target cell `MANAGER+52` points at until the environment is wired.
const DEFAULT_HOLDER: u32 = ROOT + 0x200;
const NULL_TARGET: u32 = ROOT + 0x204;
/// `*(system+0)`: the mixer's arena root (`+32` the scratch stack top the kernels carve from),
/// a separate object from the bus root in retail.
const ARENA_ROOT: u32 = SPACE + 0x1800;
const MANAGER: u32 = SPACE + 0x2000;
const DEVICE: u32 = SPACE + 0x3000;
const PASS: u32 = SPACE + 0x4000;
const PARAMS: u32 = SPACE + 0x4100;
const WORKER: u32 = SPACE + 0x4200;
const OUTPUT_HOLDER: u32 = SPACE + 0x4300;
const OUTPUT_DESC: u32 = SPACE + 0x4400;
const SCRATCH: u32 = SPACE + 0x5000;
const RING: u32 = SPACE + 0x10000;
const PLANE_A: u32 = SPACE + 0x20000;
const PLANE_B: u32 = SPACE + 0x22000;
const PLANE_C: u32 = SPACE + 0x24000;
const PLANES_OUT: u32 = SPACE + 0x26000;
const PCM: u32 = SPACE + 0x28000;
const STACK: u32 = SPACE + 0x3F000;
const ARENA: u32 = 0x5000_0000;
const PATCH_HEAP: u32 = 0x6000_0000;
const GRAPH_HEAP: u32 = 0x6400_0000;
const HEAP_BYTES: u32 = 0x0400_0000;
/// Spare decoder blocks kept for the game side's own SndPlayer1s.
const GAME_SIDE_VOICES: usize = 64;

/// Host allocator with reusable guest allocations, owned by the audio thread.
#[derive(Default)]
pub(crate) struct ArenaHeap {
    next: u32,
    end: u32,
    live: HashMap<u32, u32>,
    /// Free blocks by address. A freed block merges with its free neighbours, and one that
    /// reaches `next` gives its space back to it, so freed space keeps its size. The old
    /// unmerged list let small allocations carve up every large free block; large ones then
    /// only ever came from `next`, which climbed ~7 KB a second of play towards the 64 MB end
    /// ("authored audio guest heap exhausted", 2026-10-03).
    free: BTreeMap<u32, u32>,
}

impl ArenaHeap {
    pub(crate) fn new(base: u32) -> Self {
        Self {
            next: base,
            end: base + HEAP_BYTES,
            ..Self::default()
        }
    }

    /// Returns `[at, at + size)` to the free blocks, merged with its free neighbours.
    fn release(&mut self, mut at: u32, mut size: u32) {
        if let Some(after) = self.free.remove(&(at + size)) {
            size += after;
        }
        if let Some((&before, &before_size)) = self.free.range(..at).next_back() {
            if before + before_size == at {
                self.free.remove(&before);
                at = before;
                size += before_size;
            }
        }
        if at + size == self.next {
            self.next = at;
        } else {
            self.free.insert(at, size);
        }
    }
}

impl Heap for ArenaHeap {
    fn alloc(&mut self, g: &mut Guest, bytes: u32, align: u32) -> Result<u32> {
        let align = align.max(16);
        let bytes = (bytes.max(16) + 15) & !15;
        // The lowest free block that holds `bytes` once aligned (address-ordered first fit).
        // ponytail: linear scan; merging keeps the list short.
        let found = self.free.iter().find_map(|(&at, &size)| {
            let start = at.checked_add(align - 1)? & !(align - 1);
            (u64::from(start - at) + u64::from(bytes) <= u64::from(size)).then_some((at, size, start))
        });
        let at = if let Some((at, size, start)) = found {
            self.free.remove(&at);
            if start > at {
                self.free.insert(at, start - at);
            }
            let tail = at + size - (start + bytes);
            if tail > 0 {
                self.free.insert(start + bytes, tail);
            }
            start
        } else {
            let at = (self.next + align - 1) & !(align - 1);
            if at.checked_add(bytes).is_none_or(|end| end > self.end) {
                return Err(Error::new(at, "authored audio guest heap exhausted"));
            }
            let (pad_at, pad) = (self.next, at - self.next);
            self.next = at + bytes;
            if pad > 0 {
                self.release(pad_at, pad);
            }
            at
        };
        g.fill(at, 0, bytes)?;
        self.live.insert(at, bytes);
        Ok(at)
    }

    fn free(&mut self, _g: &mut Guest, at: u32) -> Result<()> {
        if let Some(size) = self.live.remove(&at) {
            self.release(at, size);
        }
        Ok(())
    }
}

/// The heap the patch interpreter ends instances with. The hand port posts from the patch heap,
/// the game side from the device heap, and an instance from either can end in the same tick.
/// Before this, every tick freed into the patch heap, which ignores an address it does not hold:
/// each game-side instance and post node that ended leaked, ~1.3 KB a second of play (2026-10-03).
/// The device heap's blocks are freed after the tick, because the voice op uses that heap
/// through the device meanwhile.
struct TickHeap<'a> {
    patch: &'a mut ArenaHeap,
    device_frees: Vec<u32>,
}

impl Heap for TickHeap<'_> {
    fn alloc(&mut self, g: &mut Guest, bytes: u32, align: u32) -> Result<u32> {
        self.patch.alloc(g, bytes, align)
    }

    fn free(&mut self, g: &mut Guest, at: u32) -> Result<()> {
        if self.patch.live.contains_key(&at) {
            return self.patch.free(g, at);
        }
        self.device_frees.push(at);
        Ok(())
    }
}

/// Small diagnostic counters; no output device or filesystem is retained by this object.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeStats {
    pub blocks: u64,
    pub voices_opened: u64,
    pub live_voices: usize,
    pub commands: u64,
    pub peak: f32,
}

/// `GlobalFEPlaySound` (`sub_825DFAF0`).
const GLOBAL_FE_PLAY_SOUND: u32 = 0x825D_FAF0;

pub struct AuthoredRuntime {
    guest: Guest,
    owner: Owner,
    banks: HashMap<String, u32>,
    objects: HashMap<String, u32>,
    messages: HashMap<u32, u32>,
    /// SFXObj_Reverb and its world-side inputs ([`crate::environment::ReverbWorld`]).
    reverb: Option<crate::environment::ReverbWorld>,
    /// The retail game side run as original code ([`crate::game`]), once booted.
    game: Option<crate::game::GameState>,
    /// SFXObj_Ambience run as original code without the rest of the game side
    /// ([`Self::ambience_boot`]): its component object.
    ambience: Option<u32>,
    /// SFXObj_Master, created with the ambience ([`Self::ambience_boot`]).
    master: Option<u32>,
}

struct Owner {
    patch_heap: ArenaHeap,
    device: AuthoredDevice,
    submitted: Vec<f32>,
    stats: RuntimeStats,
    clip_next: u64,
    recent_peak: f32,
}

thread_local! {
    static HUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

struct InspectKernels<'a> {
    inner: kernels::VoiceKernels<'a, mathlib::Image>,
    trace: bool,
}
impl graph::GraphHost for InspectKernels<'_> {
    fn prepare(&mut self, g: &mut Guest, f: u32, o: u32, p: u32, flag: u32, n: u64) -> Result<u64> {
        graph::GraphHost::prepare(&mut self.inner, g, f, o, p, flag, n)
    }
    fn process(&mut self, g: &mut Guest, f: u32, o: u32, p: u32, flag: u32) -> Result<u64> {
        // `SKATE_AUDIO_HUNT`: the first few kernels whose output leaves [-16, 16], with the
        // resampler's state from before the call.
        let hunt = HUNT.with(|h| h.get()) < 6 && env_set!("SKATE_AUDIO_HUNT");
        let before = if hunt && f == 0x82B2_DBA8 {
            let tail = o + u32::from(g.u16(o + 76)?);
            let src = g.u32(p + 28)?;
            let base = g.u32(src + 4)?;
            format!(
                "bias={} credit={} step={:#x} fraction={:#x} cap={} in_frames={} tail={:?} input={:?}",
                g.u8(o + 80)?,
                g.u8(o + 81)?,
                g.u32(o + 68)?,
                g.u32(o + 72)?,
                g.u16(o + 78)?,
                g.u32(p + 48)?,
                (0..6).map(|i| g.f32(tail + 4 * i).unwrap_or(f32::NAN)).collect::<Vec<_>>(),
                (0..8).map(|i| g.f32(base + 4 * i).unwrap_or(f32::NAN)).collect::<Vec<_>>(),
            )
        } else {
            String::new()
        };
        let result = graph::GraphHost::process(&mut self.inner, g, f, o, p, flag)?;
        if hunt {
            let d = g.u32(p + 28)?;
            let data = g.u32(d + 4)?;
            let frames = g.u32(p + 48)?.min(256);
            let channels = u32::from(g.u8(p + 60)?).clamp(1, 8);
            let bad = (0..channels)
                .flat_map(|c| (0..frames).map(move |i| c * 256 + i))
                .find(|&k| !(g.f32(data + k * 4).unwrap_or(0.0).abs() <= 16.0));
            if let Some(at) = bad {
                HUNT.with(|h| h.set(h.get() + 1));
                let class = g.u32(o + 20).and_then(|c| g.u32(c + 0x24)).unwrap_or(0);
                eprintln!(
                    "HUNT kernel {f:08x} class={} object={o:08x} owner={p:08x} frames={frames} ch={channels} at={at} value={} {before}",
                    String::from_utf8_lossy(&class.to_be_bytes()),
                    g.f32(data + at * 4)?
                );
            }
        }
        if self.trace {
            let d = g.u32(p + 28)?;
            let data = g.u32(d + 4)?;
            let channels = u32::from(g.u8(p + 60)?).clamp(1, 8);
            let peak: Vec<String> = (0..channels)
                .map(|c| {
                    let v = (0..256)
                        .map(|i| g.f32(data + (c * 256 + i) * 4).unwrap_or(0.0).abs())
                        .fold(0.0f32, f32::max);
                    format!("{v:.4}")
                })
                .collect();
            let peak = peak.join(",");
            if f == 0x82B2_C658 {
                let w = |k: u32| g.f32(o + k).unwrap_or(f32::NAN);
                eprintln!(
                    "peq {o:08x} cutoff={} gain={} q={} coef={:?} hist={:?} filtering={}",
                    w(52),
                    w(60),
                    w(68),
                    (0..5).map(|i| w(204 + 4 * i)).collect::<Vec<_>>(),
                    (0..8).map(|i| w(72 + 4 * i)).collect::<Vec<_>>(),
                    g.u32(o + 200).unwrap_or(0)
                );
            }
            eprintln!(
                "kernel {f:08x} object={o:08x} result={result} frames={} channels={} peak={peak}",
                g.u32(p + 48)?,
                g.u8(p + 60)?
            );
        }
        Ok(result)
    }
}

struct SourceState {
    decoder: u32,
    loop_range: Option<(usize, usize)>,
}

struct AuthoredDevice {
    heap: ArenaHeap,
    sources: HashMap<u32, CachedPcm>,
    streams: PcmStreams,
    players: HashMap<u32, SourceState>, // SndPlayer1 -> decoder state
    live: HashMap<u32, u32>,            // voice -> player
    opened: u64,
    /// Voices whose player has published an active span; see `query`.
    sounded: std::collections::HashSet<u32>,
    /// Grain-player hook (`grain::host`): grain files, pending grain plays and the plug-in clock.
    grains: crate::grain::host::GrainRuntime,
    /// Samples a play asked for that have no PCM (reported once each).
    missing: std::collections::HashSet<u32>,
}

/// Zero every image section's tail past its stored bytes (`VirtualSize > SizeOfRawData`), as the
/// console loader does. The staged image carries a repeating junk pattern there (`.data`'s
/// uninitialised globals, `0x83046600..0x830EF9F8`), which original code reads as pointers.
fn zero_uninitialised(g: &mut Guest) -> Result<()> {
    const BASE: u32 = 0x8200_0000;
    let le = |g: &Guest, at: u32| g.u32(at).map(u32::swap_bytes);
    if g.u16(BASE).ok() != Some(0x4D5A) {
        return Ok(()); // no image mapped
    }
    let pe = BASE + le(g, BASE + 0x3C)?;
    let sections = g.u16(pe + 6)?.swap_bytes() as u32;
    let table = pe + 24 + g.u16(pe + 20)?.swap_bytes() as u32;
    for i in 0..sections {
        let s = table + 40 * i;
        let (virtual_size, address, raw_size) = (le(g, s + 8)?, le(g, s + 12)?, le(g, s + 16)?);
        if virtual_size > raw_size {
            g.fill(BASE + address + raw_size, 0, virtual_size - raw_size)?;
        }
    }
    Ok(())
}

impl AuthoredRuntime {
    pub fn new(
        mut guest: Guest,
        projects: Vec<Vec<u8>>,
        banks: Vec<(String, Vec<u8>)>,
    ) -> Result<Self> {
        zero_uninitialised(&mut guest)?;
        guest.put(SPACE, vec![0; 0x40000]);
        guest.put(PATCH_HEAP, vec![0; HEAP_BYTES as usize]);
        guest.put(GRAPH_HEAP, vec![0; HEAP_BYTES as usize]);
        guest.put(crate::environment::STACK_BASE, vec![0; crate::environment::STACK_BYTES as usize]);
        guest.put(crate::environment::DATA_BASE, vec![0; crate::environment::DATA_BYTES as usize]);
        guest.put(crate::environment::WORLD_BASE, vec![0; crate::environment::WORLD_BYTES as usize]);
        for cell in [
            symbols::PROJECT_LIST_HEAD,
            interp::LIST_HEAD,
            patch::BANK_LIST,
            interp::DELTA_CACHE,
            interp::FRAME_COUNT,
            interp::COUNTDOWN,
            interp::SCALE_GLOBAL,
            modules::SUBMIX_HEAD,
        ] {
            guest.set_u32(cell, 0)?;
        }
        guest.set_u16(symbols::GENERATION, 0)?;
        guest.set_u32(interp::PERIOD_DENOM, 30.0f32.to_bits())?;
        guest.set_u8(classes::REGISTERED, 0)?;
        let mut next = ARENA;
        let mut place = |g: &mut Guest, bytes: Vec<u8>| -> Result<u32> {
            let at = next;
            next = at
                .checked_add(bytes.len() as u32 + 0xFFF)
                .map(|end| end & !0xFFF)
                .ok_or_else(|| Error::new(at, "bank address overflow"))?;
            if next >= PATCH_HEAP {
                return Err(Error::new(at, "banks exceed guest arena"));
            }
            g.put(at, bytes);
            Ok(at)
        };
        let mut objects = HashMap::new();
        let mut patch_heap = ArenaHeap::new(PATCH_HEAP);
        for bytes in projects {
            let at = place(&mut guest, bytes)?;
            symbols::install_project(&mut guest, at)?;
            let table = guest.u32(at + 24)?;
            for i in 0..u32::from(guest.u16(at + 12)?) {
                let record = table + 12 * i;
                let name_at = guest.u32(record + 4)?;
                let mut name = Vec::new();
                while guest.u8(name_at + name.len() as u32)? != 0 {
                    name.push(guest.u8(name_at + name.len() as u32)?);
                }
                let slot = patch_heap.alloc(&mut guest, 8, 4)?;
                guest.set_u32(slot, record)?;
                guest.set_u32(slot + 4, guest.u32(record + 8)?)?;
                objects.insert(String::from_utf8_lossy(&name).into_owned(), slot);
            }
        }
        let mut installed = HashMap::new();
        for (name, bytes) in banks {
            // `SPLC` sound banks (the one-shot voices' banks) are not patch banks: the retail
            // loader takes its other branch for them (`sub_828DC660`'s `[resource+4] == 2`, which
            // registers their sound table instead of installing patches), so `load_bank`'s `.abk`
            // fixups must not run over one. They are placed as they are and addressed by
            // `bank_base` + the sample's offset.
            let splice = bytes.get(..4) == Some(b"SPLC");
            let at = place(&mut guest, bytes)?;
            if !splice {
                patch::load_bank(&mut guest, at, STACK)?;
            }
            installed.insert(name, at);
        }
        let mut device = AuthoredDevice::new();
        device.initialize(&mut guest)?;
        Ok(Self {
            guest,
            owner: Owner {
                patch_heap,
                device,
                submitted: Vec::new(),
                stats: RuntimeStats::default(),
                clip_next: 0,
                recent_peak: 0.0,
            },
            banks: installed,
            objects,
            messages: HashMap::new(),
            reverb: None,
            game: None,
            ambience: None,
            master: None,
        })
    }

    /// The graph arena's top (the most it has handed out past its free blocks) and how many
    /// blocks are currently live. A long session must see both plateau: either climbing without
    /// bound ends in "authored audio guest heap exhausted".
    pub fn arena_usage(&self) -> (u32, usize) {
        (
            self.owner.device.heap.next - GRAPH_HEAP,
            self.owner.device.heap.live.len(),
        )
    }

    /// Every live graph-arena block as `(address, bytes)`, for finding what a climbing
    /// [`Self::arena_usage`] is made of.
    pub fn arena_live_blocks(&self) -> Vec<(u32, u32)> {
        self.owner.device.heap.live.iter().map(|(&at, &size)| (at, size)).collect()
    }

    /// Any sample key with decoded PCM installed. For tests that need a voice to open and do not
    /// care which sound it is.
    pub fn any_installed_sample(&self) -> Option<u32> {
        self.owner.device.sources.keys().min().copied()
    }

    pub fn bank_base(&self, name: &str) -> Option<u32> {
        self.banks.get(name).copied()
    }

    pub fn insert_pcm(&mut self, sample_key: u32, pcm: CachedPcm) -> Result<()> {
        if pcm.sample_rate == 0 || pcm.source.frames() == 0 {
            return Err(Error::new(sample_key, "empty decoded PCM source"));
        }
        self.owner.device.sources.insert(sample_key, pcm);
        Ok(())
    }

    /// Drop the PCM installed at `sample_key` (a voice on it then plays silence).
    pub fn remove_pcm(&mut self, sample_key: u32) {
        self.owner.device.sources.remove(&sample_key);
    }

    pub fn has_pcm(&self, sample_key: u32) -> bool {
        self.owner.device.sources.contains_key(&sample_key)
    }

    pub fn has_object(&self, object: &str) -> bool {
        self.objects.contains_key(object)
    }

    /// Post the exact recovered payload. The returned handle can be redelivered or released.
    pub fn post(&mut self, object: &str, payload: &[u32]) -> Result<u32> {
        self.post_relocated(object, payload, &[])
    }

    /// Post a recovered message whose native packet includes pointers into that packet.
    ///
    /// Trace payloads are process-relative: fields which pointed into the caller's packet in the
    /// retail process cannot be copied verbatim into this guest. `relocations` gives each field's
    /// word index and byte offset from the payload base, preserving the same self-reference after
    /// the packet has been allocated here.
    pub fn post_relocated(
        &mut self,
        object: &str,
        payload: &[u32],
        relocations: &[(usize, u32)],
    ) -> Result<u32> {
        if payload.len() > 128 {
            return Err(Error::new(0, "audio payload exceeds 128 words"));
        }
        if relocations.iter().any(|(index, _)| *index >= payload.len()) {
            return Err(Error::new(
                0,
                "audio payload relocation is outside the packet",
            ));
        }
        let slot = *self
            .objects
            .get(object)
            .ok_or_else(|| Error::new(0, format!("unknown audio object {object}")))?;
        let message = self.owner.patch_heap.alloc(&mut self.guest, 516, 16)?;
        for (i, word) in payload.iter().enumerate() {
            self.guest.set_u32(message + 4 + i as u32 * 4, *word)?;
        }
        for &(index, offset) in relocations {
            self.guest
                .set_u32(message + 4 + index as u32 * 4, message + 4 + offset)?;
        }
        let status = patch::post(
            &mut self.guest,
            &mut self.owner.patch_heap,
            slot,
            message + 4,
            message,
        )?;
        if status != 0 {
            return Err(Error::new(slot, format!("post {object} returned {status}")));
        }
        let node = self.guest.u32(message)?;
        self.messages.insert(node, message);
        Ok(node)
    }

    pub fn redeliver(&mut self, handle: u32, payload: &[u32]) -> Result<()> {
        self.redeliver_relocated(handle, payload, &[])
    }

    /// Re-deliver a recovered packet with payload-relative pointer fields relocated into the
    /// already allocated guest message.
    pub fn redeliver_relocated(
        &mut self,
        handle: u32,
        payload: &[u32],
        relocations: &[(usize, u32)],
    ) -> Result<()> {
        let Some(&message) = self.messages.get(&handle) else {
            return Ok(());
        };
        if payload.len() > 128 {
            return Err(Error::new(handle, "audio payload exceeds 128 words"));
        }
        if relocations.iter().any(|(index, _)| *index >= payload.len()) {
            return Err(Error::new(
                handle,
                "audio payload relocation is outside the packet",
            ));
        }
        for (i, word) in payload.iter().enumerate() {
            self.guest.set_u32(message + 4 + i as u32 * 4, *word)?;
        }
        for &(index, offset) in relocations {
            self.guest
                .set_u32(message + 4 + index as u32 * 4, message + 4 + offset)?;
        }
        patch::redeliver(&mut self.guest, handle, message + 4)?;
        Ok(())
    }

    pub fn release(&mut self, handle: u32) -> Result<()> {
        if let Some(message) = self.messages.remove(&handle) {
            patch::release_message(&mut self.guest, &mut self.owner.patch_heap, handle)?;
            self.owner.patch_heap.free(&mut self.guest, message)?;
        }
        Ok(())
    }

    pub fn stats(&self) -> RuntimeStats {
        self.owner.stats
    }

    /// Grain-player hook: the game-side grain API (`grain::host::Grains`) over this owner's guest,
    /// device heap and decoded sources.
    pub fn grains(&mut self) -> crate::grain::host::Grains<'_> {
        let device = &mut self.owner.device;
        crate::grain::host::Grains {
            g: &mut self.guest,
            heap: &mut device.heap,
            sources: &mut device.sources,
            runtime: &mut device.grains,
            sp: STACK,
        }
    }

    /// Grain-player hook: voice starts the grain players have made so far (drained).
    pub fn take_grain_starts(&mut self) -> Vec<crate::grain::host::StartEvent> {
        std::mem::take(&mut self.owner.device.grains.starts)
    }

    /// The guest memory (diagnostics and tests).
    pub fn guest(&self) -> &Guest {
        &self.guest
    }

    /// One draw from the shared evaluator counter, as another patch instance's op would make.
    pub fn advance_counter_for_test(&mut self) {
        let _ = crate::counter::advance(&mut self.guest);
    }

    /// Write every guest segment to `dir/seg_XXXXXXXX.bin`, for replaying retail code over it.
    pub fn dump_guest_for_test(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for s in self.guest.segments() {
            std::fs::write(dir.join(format!("seg_{:08X}.bin", s.base)), &s.bytes)?;
        }
        Ok(())
    }

    /// Load an `aud_reverb` preset layout into environment branch `slot` with its C7 send at
    /// `level` ([`crate::environment::load_preset`]); applied by the next block's command drain.
    pub fn load_reverb_preset(
        &mut self,
        layout: &[u8; crate::environment::PRESET_BYTES],
        slot: u32,
        level: f32,
    ) -> Result<()> {
        crate::environment::load_preset(
            &mut self.guest,
            &mut self.owner.device.heap,
            &mut mathlib::Image,
            layout,
            slot,
            level,
        )
    }

    /// The rest of retail's boot wiring of the environment network, once: each final output's
    /// first Send retargeted from the null bus to C7 (`[[manager + 52]]`, as `sub_82490270` wires
    /// it; applied by the next command drain), then `sub_824907D8`'s modulators, whose bucket-0
    /// task needs the scheduler's block delta. [`Self::reverb_install`] calls it before the
    /// controller's init, which writes into the modulator records.
    pub fn wire_environment(&mut self) -> Result<()> {
        let g = &mut self.guest;
        let c7 = g.u32(g.u32(MANAGER + 52)?)?;
        for index in 0..device::OUTPUT_GRAPH_COUNT {
            let table = g.u32(MANAGER + device::OUTPUT_MODULE_TABLES + 4 * index)?;
            g.set_u32(SCRATCH, classes::TAG_POINTER)?;
            g.set_u32(SCRATCH + 4, c7)?;
            device::configure(g, g.u32(table + 12)?, 0, SCRATCH)?;
        }
        let delta = SYSTEM + crate::grain::host::SCHEDULER_DELTA;
        if g.u32(delta)? == 0 {
            g.set_u32(delta, crate::grain::host::BLOCK_DELTA_BITS)?;
        }
        crate::environment::build_modulators(
            g,
            &mut self.owner.device.heap,
            &mut mathlib::Image,
            MANAGER,
        )
    }

    /// Boot the retail audio game side as original code ([`crate::game::boot`]) on this runtime,
    /// mapping its heap and stack first. Returns the root, or where it stopped, with the traces.
    pub fn game_boot(
        &mut self,
        vault: crate::attrib::Vault,
        files: crate::game::Files,
    ) -> (Result<u32>, Vec<crate::ppc::CallTrace>) {
        let mut state = match crate::game::prepare(&mut self.guest, vault, files) {
            Ok(state) => state,
            Err(e) => return (Err(e), Vec::new()),
        };
        let t = std::time::Instant::now();
        let result = self.game_boot_with(&mut state);
        let booted = t.elapsed();
        self.game = Some(state);
        self.install_title_banks();
        if std::env::var_os("SKATE_BOOT_TIMING").is_some() {
            eprintln!("game_boot: boot {:.2}s, banks {:.2}s", booted.as_secs_f32(), (t.elapsed() - booted).as_secs_f32());
        }
        result
    }

    /// Install the PCM of every bank the game side loaded since the last call.
    fn install_title_banks(&mut self) {
        let Some(state) = self.game.as_mut() else { return };
        for (path, at, bytes) in std::mem::take(&mut state.files.banks) {
            let name = path.replace('\\', "/");
            let name = name.rsplit('/').next().unwrap_or(&name).to_owned();
            let t = std::time::Instant::now();
            let decoded = (state.files.decode)(&name, &bytes);
            if std::env::var_os("SKATE_BOOT_TIMING").is_some() && t.elapsed().as_millis() > 50 {
                eprintln!("  bank {name} {} bytes: {} streams in {:.2}s", bytes.len(), decoded.len(), t.elapsed().as_secs_f32());
            }
            for (offset, pcm) in decoded {
                if pcm.sample_rate != 0 && pcm.source.frames() != 0 {
                    // Every play with a start time (grain windows, the wheel spin-downs) starts
                    // inside the stream at the retail offset (`grain::host::seek`,
                    // `seek::start_frame`: never on a looping slot), not at its first frame.
                    self.owner.device.grains.register(&name, at, at + offset, &pcm);
                    self.owner.device.sources.insert(at + offset, pcm);
                }
            }
        }
    }

    /// Run the booted game side for the world only ([`crate::game::GameState::world_only`]).
    pub fn game_world_only(&mut self) {
        if let Some(state) = self.game.as_mut() {
            state.world_only = true;
        }
    }

    /// The local skater on the booted game side ([`crate::game::Skater`]): its PhysOut slot
    /// blocks are for the caller to fill each frame before [`Self::game_publish`].
    pub fn game_skater(&mut self) -> Result<crate::game::Skater> {
        let state = self.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
        crate::game::Skater::new(&mut self.guest, state, &mut self.owner.device.heap)
    }

    /// Build and publish the skater's record for this frame.
    pub fn game_publish(&mut self, skater: &crate::game::Skater) -> Result<()> {
        let state = self.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
        skater.publish(&mut self.guest, state, &mut self.owner.device.heap)
    }

    /// Enter a district on the game side: its trigger manager (`[root+0x1E8]`, per frame
    /// `sub_824A24F8`) loads the `district_audio` collection `key` names (`+0x110`, state `+0x118`)
    /// — its `.ems` reverb/music/emitter/speaker/crowd zones — on the next frame.
    pub fn game_set_district(&mut self, key: u64) -> Result<()> {
        let root = self.guest.u32(crate::game::ROOT_GLOBAL)?;
        let triggers = self.guest.u32(root + 0x1E8)?;
        if triggers == 0 {
            return Err(Error::new(root, "game side has no trigger manager"));
        }
        self.guest.set_u64(triggers + 0x110, key)?;
        self.guest.set_u32(triggers + 0x118, 0)
    }

    /// Load the traffic banks as the title does on entering play: the gameplay load `sub_82487B98` asks
    /// (`sub_82487168(root, 1)`: root `+596` = 2) and the root's frame `sub_824854A8` loads them (`sub_82487168(root,
    /// 0)`: the eight engine banks of table 0x8302D408, Traffic_Horn, Traffic_Skid and car_alarms, by the title's TOC
    /// size into the root's pool), here at once, with the resource manager drained as the loading screen waits.
    /// `reload`: the loaded ones go first (`sub_82487408`), for another game's banks under the same names, the
    /// audio rendering a block at a time until the resource manager has freed their slots
    /// ([`crate::game::release_step`]; the blocks' output is dropped, as under a loading screen). Returns root
    /// `+596` (3: loaded).
    pub fn game_traffic_banks(&mut self, reload: bool) -> Result<u32> {
        const LOAD: u32 = 0x8248_7168;
        const UNLOAD: u32 = 0x8248_7408;
        let root = self.guest.u32(crate::game::ROOT_GLOBAL)?;
        let call = |runtime: &mut Self, function: u32, args: &[u64]| -> Result<()> {
            let state = runtime.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
            let heap = &mut runtime.owner.device.heap;
            crate::game::run(&mut runtime.guest, state, &mut *heap, function, args).0?;
            crate::game::drain_loads(&mut runtime.guest, state, &mut *heap)
        };
        if reload {
            call(self, UNLOAD, &[root as u64])?;
            let mut blocks = 0;
            loop {
                let state = self.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
                if !crate::game::release_step(&mut self.guest, state, &mut self.owner.device.heap)? {
                    break;
                }
                blocks += 1;
                if blocks > 64 {
                    return Err(Error::new(root, "the traffic banks never left the sound engine"));
                }
                self.pump_once()?;
            }
        }
        call(self, LOAD, &[root as u64, 1])?;
        call(self, LOAD, &[root as u64, 0])?;
        self.install_title_banks();
        self.guest.u32(root + 596)
    }

    /// This frame's traffic cars, nearest first ([`crate::game::publish_traffic`]).
    pub fn game_traffic(&mut self, cars: &[crate::game::TrafficCar]) -> Result<()> {
        let state = self.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
        crate::game::publish_traffic(&mut self.guest, state, cars)
    }

    /// This frame's DMOs ([`crate::game::publish_dmos`]): the held prop SFXObj_Moving drags (round2 bug 7).
    pub fn game_dmos(&mut self, dmos: &[crate::game::DmoEntry]) -> Result<()> {
        let state = self.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
        crate::game::publish_dmos(&mut self.guest, state, dmos)
    }

    /// Boot SFXObj_Ambience as original code ([`crate::game::boot_ambience`]) over this runtime, for
    /// a runtime that keeps its own (hand-ported) components: the title's audio boot objects and
    /// singletons, none of its managers, banks or MixMap (the runtime's own MixMap supplies the
    /// component's controller). Needs the MixMap loaded. Call before [`Self::reverb_install`],
    /// which then shares the game object this builds. `files` serves the title's files (the
    /// component loads `ambienceresident.big` whole; see [`Self::game_files`]).
    pub fn ambience_boot(
        &mut self,
        vault: crate::attrib::Vault,
        files: crate::game::Files,
    ) -> (Result<u32>, Vec<crate::ppc::CallTrace>) {
        // ponytail: a runtime boots the ambience once.
        if let Some(component) = self.ambience {
            return (Ok(component), Vec::new());
        }
        let Some(controller) = self.mixmap_controller(crate::mixmap::key(2, 2, 0, 0)) else {
            return (Err(Error::new(0, "the MixMap has no Ambience controller")), Vec::new());
        };
        if let Err(e) = crate::grain::host::install(
            &mut self.guest,
            &mut self.owner.device.heap,
            &mut self.owner.device.grains,
        ) {
            return (Err(e), Vec::new());
        }
        // Decoder blocks for the SndPlayer1s the bed graphs start between two top-ups.
        self.owner.device.grains.reserve_spare(GAME_SIDE_VOICES);
        let mut state = match crate::game::prepare(&mut self.guest, vault, files) {
            Ok(state) => state,
            Err(e) => return (Err(e), Vec::new()),
        };
        // ponytail: the title's 7,107 static constructors are not run: the component works
        // without them (its output is identical, measured), and they would write over the
        // hand-ported runtime's own image state.
        let (booted, traces) = crate::game::boot_ambience(
            &mut self.guest,
            &mut state,
            &mut self.owner.device.heap,
            controller,
        );
        let mut traces = traces;
        // SFXObj_Master over the same boot: without it nothing sets the master section FE (its
        // clipper and levels) from the Master controller, as the title's per-frame update does.
        if booted.is_ok() {
            match self.mixmap_controller(0x4000_0020) {
                Some(controller) => match crate::game::create_component(
                    &mut self.guest,
                    &mut state,
                    &mut self.owner.device.heap,
                    crate::game::MASTER_FACTORY,
                    crate::game::MASTER_CREATE,
                    controller,
                    &mut traces,
                ) {
                    Ok(component) => self.master = Some(component),
                    Err(e) => eprintln!("SKATE_PLAYER_AUDIO master_failed create error={e}"),
                },
                None => eprintln!("SKATE_PLAYER_AUDIO master_failed the MixMap has no Master controller"),
            }
        }
        // The resident archive is not a sound bank to install (the caller installs its beds).
        state.files.banks.clear();
        self.game = Some(state);
        if let Ok(component) = booted {
            self.ambience = Some(component);
        }
        (booted, traces)
    }

    /// SFXObj_Master's process for one game frame, before the MixMap evaluation. A no-op without
    /// the component.
    pub fn master_process(&mut self, dt: f32) -> Result<()> {
        let (Some(component), Some(state)) = (self.master, self.game.as_mut()) else { return Ok(()) };
        let process = crate::game::MASTER_PROCESS;
        crate::game::component_process(&mut self.guest, state, &mut self.owner.device.heap, process, component, dt).0?;
        Ok(())
    }

    /// SFXObj_Master's update, after the MixMap evaluation: the master section's parameters.
    pub fn master_update(&mut self) -> Result<()> {
        let (Some(component), Some(state)) = (self.master, self.game.as_mut()) else { return Ok(()) };
        crate::game::run(&mut self.guest, state, &mut self.owner.device.heap, crate::game::MASTER_UPDATE, &[component as u64]).0?;
        Ok(())
    }

    /// SFXObj_Ambience's process for one game frame, with the `audio_ambience` painter key at the
    /// player (0 = unpainted). Run before the MixMap evaluation. A no-op before
    /// [`Self::ambience_boot`].
    pub fn ambience_process(&mut self, key: u64, dt: f32) -> Result<()> {
        let (Some(component), Some(state)) = (self.ambience, self.game.as_mut()) else { return Ok(()) };
        crate::game::ambience_process(&mut self.guest, state, &mut self.owner.device.heap, component, key, dt).0?;
        Ok(())
    }

    /// `(state, bed key)` of SFXObj_Ambience: state 0 idle, 1 playing, 2 fading out, 3 fading in;
    /// the bed's `aud_wp_ambiences` key (`+0x40`, `+0x30`). `None` before [`Self::ambience_boot`].
    pub fn ambience_status(&self) -> Option<(u32, u64)> {
        let component = self.ambience?;
        Some((self.guest.u32(component + 0x40).ok()?, self.guest.u64(component + 0x30).ok()?))
    }

    /// The weather's duck on the ambience bed, 0..1 ([`crate::game::GameState::ambience_duck`]).
    pub fn set_ambience_duck(&mut self, scale: f32) {
        if let Some(state) = self.game.as_mut() {
            state.ambience_duck = if scale.is_finite() { scale.clamp(0., 1.) } else { 1. };
        }
    }

    /// SFXObj_Ambience's update, after the MixMap evaluation (with the weather's duck on its controller).
    pub fn ambience_update(&mut self) -> Result<()> {
        let duck = self.game.as_ref().map_or(1., |s| s.ambience_duck);
        if duck < 1.
            && let Some(ctrl) = self.mixmap_controller(crate::mixmap::key(2, 2, 0, 0))
        {
            crate::mixmap::controller::scale_gain(&mut self.guest, ctrl, 0, duck)?;
        }
        let (Some(component), Some(state)) = (self.ambience, self.game.as_mut()) else { return Ok(()) };
        crate::game::ambience_update(&mut self.guest, state, &mut self.owner.device.heap, component).0?;
        Ok(())
    }

    /// The engine's pause for the booted game side ([`crate::game::GameState::paused`]), and
    /// whether a cutscene rather than a menu holds it ([`crate::game::GameState::cutscene`]).
    pub fn game_set_paused(&mut self, paused: bool, cutscene: bool) {
        if let Some(state) = self.game.as_mut() {
            state.paused = paused;
            state.cutscene = cutscene;
        }
    }

    /// Audio Settings' [SFX, Dialog, Music] (0..1) into the game's settings object `+0x80 / +0x84 /
    /// +0x88`, which SFXObj_Master's process reads every frame. A no-op before the object exists.
    pub fn set_volumes(&mut self, volumes: [f32; 3]) -> Result<()> {
        let settings = self.guest.u32(crate::game::SETTINGS)?;
        if settings == 0 {
            return Ok(());
        }
        for (i, v) in volumes.iter().enumerate() {
            self.guest.set_u32(settings + 0x80 + 4 * i as u32, v.to_bits())?;
        }
        Ok(())
    }

    /// Mutable guest memory (the caller writes the game side's inputs).
    pub fn guest_mut(&mut self) -> &mut Guest {
        &mut self.guest
    }

    /// One game frame of the booted game side ([`crate::game::frame`]).
    pub fn game_frame(&mut self, dt: f32) -> (Result<u64>, crate::ppc::CallTrace) {
        let result = match self.game.as_mut() {
            Some(state) => crate::game::frame(&mut self.guest, state, &mut self.owner.device.heap, dt),
            None => (Err(Error::new(0, "game side not booted")), Default::default()),
        };
        self.install_title_banks();
        result
    }

    /// `GlobalFEPlaySound(id, 0)` (`sub_825DFAF0`) on the booted game side: look the `fe` record
    /// `id` up and hand it to the audio root's front-end player (`[0x830CFDC4]` vtable `+0x2C`),
    /// which starts its `sk8_menu` Splice voice. The session marker, the score multiplier and the
    /// menus all play their sounds this way.
    pub fn game_fe_sound(&mut self, id: u64) -> Result<()> {
        let state = self.game.as_mut().ok_or_else(|| Error::new(0, "game side not booted"))?;
        crate::game::run(&mut self.guest, state, &mut self.owner.device.heap, GLOBAL_FE_PLAY_SOUND, &[id, 0]).0?;
        Ok(())
    }

    /// Every file the game side has loaded, with where it went.
    pub fn game_files(&self) -> Vec<(String, u32)> {
        self.game.as_ref().map_or_else(Vec::new, |s| s.files.loaded.clone())
    }

    fn game_boot_with(
        &mut self,
        state: &mut crate::game::GameState,
    ) -> (Result<u32>, Vec<crate::ppc::CallTrace>) {
        let mut state = state;
        // The audio setup's remaining class registrations (`sub_828DD590`: the grain fader, DCl0,
        // HS20, ...), which the title's components find by id.
        if let Err(e) = crate::grain::host::install(
            &mut self.guest,
            &mut self.owner.device.heap,
            &mut self.owner.device.grains,
        ) {
            return (Err(e), Vec::new());
        }
        // Decoder blocks for the SndPlayer1s the game side's graphs start between two top-ups.
        self.owner.device.grains.reserve_spare(GAME_SIDE_VOICES);
        match crate::game::initialize_statics(&mut self.guest, state, &mut self.owner.device.heap) {
            Ok(stopped) => {
                eprintln!("static initializers: {} stopped", stopped.len());
                for (function, e) in stopped.iter().take(40) {
                    eprintln!("  {function:08X}: {e}");
                }
            }
            Err(e) => return (Err(e), Vec::new()),
        }
        crate::game::boot(&mut self.guest, state, &mut self.owner.device.heap)
    }

    /// Lay out the exported audio classes and one level's reverb zones (their raw `.ems`
    /// records), then run SFXObj_Reverb's init (`sub_824DDE50`): the default preset into both
    /// branches, the output wobble presets, and the MixMap Reverb controller (`0x40000050`, when
    /// the MixMap is loaded) enabled. Call again on a level change.
    pub fn reverb_install(
        &mut self,
        classes: &[crate::environment::WorldClass],
        zones: &[[u8; 72]],
    ) -> Result<()> {
        if self.reverb.is_none() {
            self.wire_environment()?;
        }
        let mixmap = self.mixmap_controller(0x4000_0050).unwrap_or(0);
        let mut world = crate::environment::ReverbWorld::new(&mut self.guest, classes)?;
        world.install_zones(&mut self.guest, zones)?;
        world.init(&mut self.guest, &mut self.owner.device.heap, &mut mathlib::Image, mixmap)?;
        self.reverb = Some(world);
        Ok(())
    }

    /// SFXObj_Reverb's process (`sub_824DE548`) for one game frame: run it before the MixMap
    /// evaluation, as retail's first half does. A no-op before [`Self::reverb_install`].
    pub fn reverb_process(
        &mut self,
        frame: &crate::environment::ReverbFrame<'_>,
        dt: f32,
    ) -> Result<()> {
        match self.reverb.as_mut() {
            Some(world) => world.process(
                &mut self.guest,
                &mut self.owner.device.heap,
                &mut mathlib::Image,
                frame,
                dt,
            ),
            None => Ok(()),
        }
    }

    /// SFXObj_Reverb's update (`sub_824DF220`), after the MixMap evaluation.
    pub fn reverb_update(&mut self) -> Result<()> {
        match self.reverb.as_mut() {
            Some(world) => {
                world.update(&mut self.guest, &mut self.owner.device.heap, &mut mathlib::Image)
            }
            None => Ok(()),
        }
    }

    pub fn pump_once(&mut self) -> Result<Vec<f32>> {
        self.owner.submitted.clear();
        worker::pump_once(&mut self.guest, &mut self.owner, WORKER)?;
        Ok(std::mem::take(&mut self.owner.submitted))
    }
}

impl AuthoredDevice {
    fn new() -> Self {
        AuthoredDevice {
            heap: ArenaHeap::new(GRAPH_HEAP),
            sources: HashMap::new(),
            streams: PcmStreams::default(),
            players: HashMap::new(),
            live: HashMap::new(),
            opened: 0,
            sounded: Default::default(),
            grains: crate::grain::host::GrainRuntime::default(),
            missing: Default::default(),
        }
    }

    fn initialize(&mut self, g: &mut Guest) -> Result<()> {
        g.set_u32(modules::SYSTEM, SYSTEM)?;
        g.set_u32(SYSTEM, ARENA_ROOT)?;
        g.set_u32(SYSTEM + 48, RING)?;
        g.set_u32(SYSTEM + 276, 64)?;
        g.set_u32(SYSTEM + 252, 6)?;
        g.set_u32(ARENA_ROOT + 32, STACK - 0x2000)?;
        g.set_u32(ROOT + 8, SYSTEM)?;
        g.set_u32(ROOT + 44, DEFAULT_HOLDER)?;
        g.set_u32(classes::BUS_ROOT, ROOT)?;
        g.set_u32(device::BUS_MANAGER, MANAGER)?;
        let registry = classes::class_registry(g, &mut self.heap, SYSTEM)?;
        g.set_u32(ROOT + 12, registry)?;
        let submix = classes::register_class(g, registry, device::SUBMIX_DESCRIPTOR)?;
        let delay = classes::register_class(g, registry, device::DELAY_DESCRIPTOR)?;
        classes::register_class(g, registry, device::SEND_DESCRIPTOR)?;
        classes::register_voice_classes(g, &mut self.heap)?;
        // The final Sub0 and measured 15 ms Del0 precede the recovered output-pass route/clamp.
        for (i, class) in [submix, delay].into_iter().enumerate() {
            let at = SCRATCH + i as u32 * 12;
            g.set_u32(at, if i == 1 { SCRATCH + 32 } else { 0 })?;
            g.set_u32(at + 4, class)?;
            g.set_u8(at + 8, 6)?;
        }
        g.set_u32(SCRATCH + 32, classes::TAG_SINGLE)?;
        g.set_u32(SCRATCH + 36, g.u32(device::OUTPUT_DELAY_SECONDS)?)?;
        let master = modules::build_graph(
            g,
            &mut self.heap,
            &mut mathlib::Image,
            SYSTEM,
            255,
            2,
            SCRATCH,
        )?;
        let bus = g.u32(master + 80)?;
        // The master stands in for retail's DAC graph FF (`Sub0 Scp0 Dac0`, whose Dac0 process is
        // the output pass this runtime calls itself): `root+0x10/0x14` and the DAC input at
        // `system+0x34`.
        g.set_u32(ROOT + 0x10, master)?;
        g.set_u32(ROOT + 0x14, master + 80)?;
        g.set_u32(SYSTEM + 0x34, bus)?;
        g.set_u32(DEFAULT_HOLDER, bus)?;
        // The framework's master section FE and VU buses, as original code; every later output
        // targets FE's Sub0 through `[[root+0x2C]]`.
        crate::environment::build_root_graphs(g, &mut self.heap, &mut mathlib::Image, ROOT)?;
        let bus = g.u32(g.u32(ROOT + 44)?)?;
        g.set_u32(classes::DEFAULT_BUS, bus)?;
        device::init_bus_players(g, &mut self.heap, &mut mathlib::Image, MANAGER, SCRATCH)?;
        // Material buses retain the actual class default parameter blocks installed above.
        for index in 0..device::BUS_GRAPH_COUNT {
            g.set_u8(MANAGER + device::BUS_CREATED_FLAGS + index, 1)?;
        }
        // sub_8248EEE8: the reverb/delay network. It fills MANAGER + 52, the intermediate target
        // each final-output graph's first send feeds, and its returns end at the default bus.
        crate::environment::build_network(g, &mut self.heap, &mut mathlib::Image, MANAGER)?;
        // Its boot defaults are not what retail plays (every Gai0 and Send at 1.0: the delay-pans
        // would echo the whole mix), so the outputs start on the null target and
        // `AuthoredRuntime::wire_environment` points them at C7 once SFXObj_Reverb runs.
        let c7 = g.u32(MANAGER + 52)?;
        g.set_u32(MANAGER + 52, NULL_TARGET)?;
        device::init_output_players(g, &mut self.heap, &mut mathlib::Image, MANAGER, SCRATCH)?;
        g.set_u32(MANAGER + 52, c7)?;
        g.set_u32(PASS, PLANE_A)?;
        g.set_u32(PASS + 4, PLANE_B)?;
        g.set_u32(PASS + 8, PLANE_C)?;
        g.set_u32(PASS + 24, SYSTEM)?;
        g.set_u32(OUTPUT_DESC + 4, PLANES_OUT)?;
        g.set_u16(OUTPUT_DESC + 14, 256)?;
        g.set_u32(OUTPUT_HOLDER + 32, OUTPUT_DESC)?;
        g.set_u32(WORKER + output::DESC_HOLDER, OUTPUT_HOLDER)?;
        g.set_u32(WORKER + worker::PCM_BASE, PCM)?;
        g.set_u8(output::CHANNEL_COUNT_BYTE, 6)?;
        g.set_u8(output::RAMP_FLAG_BYTE, 0)?;
        Ok(())
    }

    fn source_pump(&mut self, g: &mut Guest, stream: u32) -> Result<()> {
        // The pump's two image doubles, as `sub_82B31EE0` loads them (`lis -32208` − 31232, +256
        // and +4192). The idle time is 0.0, not a far-future marker: with an invented 1e30 a
        // finished one-shot reported an elapsed time no patch ever sees in retail, so the skid's
        // second layer never re-triggered and a revert kept only its quiet first voice.
        let constants = pump::PumpConstants {
            far_future: f64::from_bits(g.u64(0x822F_8700)?),
            time_bump: f64::from_bits(g.u64(0x822F_9660)?),
        };
        pump::pump(g, self, stream, constants)
    }

    /// `heap` is passed in rather than taken from `self`, because the command drain moves the
    /// device's arena out for the duration of the drain — freeing through `self.heap` there frees
    /// into a temporary and silently leaks.
    fn stop_graph(&mut self, g: &mut Guest, heap: &mut dyn Heap, player: u32) -> Result<()> {
        for i in 0..u32::from(g.u8(player + 68)?) {
            let child = g.u32(player + 80 + 4 * i)?;
            if g.u32(child)? == modules::SEND_VTABLE {
                voices::release_voice(g, u64::from(child), STACK)?;
            }
            // Every free here goes through `heap`: its only caller is the drain, where
            // `self.heap` is the empty stand-in. The submix and the SndPlayer1's voice buffer
            // went there and leaked, ~17 blocks a second of play, until a long session ended in
            // "authored audio guest heap exhausted" (2026-10-03).
            if g.u32(child)? == modules::SUBMIX_VTABLE {
                modules::destroy_submix(g, &mut *heap, child)?;
            }
            if let Some(source) = self.players.remove(&child) {
                self.streams.detach(source.decoder);
                heap.free(g, source.decoder)?;
                scheduler::detach_instance(
                    g,
                    u64::from(SYSTEM + scheduler::SYSTEM_SCHEDULER),
                    child + 72,
                )?;
                heap.free(g, g.u32(child + 440)?)?;
            }
        }
        // `sub_82B48F28`'s unlink: a graph left on its owner's processing list (a bus built at
        // type 1 or 3, such as `sub_82488DD0`'s owner send) was still mixed after this free,
        // out of whatever reused the block.
        crate::player::detach_graph(g, player)?;
        g.set_u8(player + 71, 2)?;
        Ok(())
    }

    /// The recoverable half of `sub_82B1E458`: append the exact eight-byte deferred player-stop
    /// record before destroying the small voice wrapper. The player, its SndPlayer1 child and the
    /// host decoder must remain alive until command phase reaches that record; a patch is allowed
    /// to open and release a voice in the same evaluator walk, after its play command was queued.
    fn defer_stop_graph(&mut self, g: &mut Guest, player: u32) -> Result<()> {
        let system = g.u32(player + 16)?;
        let offset = g.u32(system + commands::COMMAND_WRITE_OFFSET)?;
        let ring = g.u32(system + commands::COMMAND_BUFFER)?;
        let record = ring.wrapping_add(offset);
        // Preserve the producer's observed publication order.
        g.set_u32(
            system + commands::COMMAND_WRITE_OFFSET,
            offset.wrapping_add(8),
        )?;
        g.set_u32(record, device::COMMAND_PLAYER_STOP)?;
        g.set_u32(record + 4, player)?;
        Ok(())
    }
}

impl VoiceDevice for AuthoredDevice {
    fn open(&mut self, g: &mut Guest, request: &OpenRequest) -> Result<u32> {
        let sample = request.sample as u32;
        // A sample without decoded PCM (ffmpeg drops frames of some EA-XMA streams, so its
        // bank's partial decode left it out): the open fails as the device's does when it has no
        // voice to give, reported once. `open_voice` deactivates the voice and the tick goes on;
        // an `Err` failed the whole block — silent, every block the sound tried to start.
        let Some(cached) = self.sources.get(&sample).cloned() else {
            if self.missing.insert(sample) {
                eprintln!("SKATE_PLAYER_AUDIO open_without_pcm sample={sample:#010x} index={}", request.index);
            }
            return Ok(0);
        };
        for (i, word) in request.shifted.iter().enumerate() {
            g.set_u32(SCRATCH + i as u32 * 4, *word)?;
        }
        g.set_u32(SCRATCH + 24, request.record_count)?;
        g.set_u32(SCRATCH + 28, request.records)?;
        let voice = device::open_voice_graph(
            g,
            &mut self.heap,
            &mut mathlib::Image,
            &mut device::NoBuses,
            DEVICE,
            sample,
            u32::from(request.byte2),
            SCRATCH,
            request.bank_72,
            request.arg8 as u32,
            SCRATCH + 24,
            STACK,
        )?;
        let stream = g.u32(voice + 8)?;
        let decoder = self.heap.alloc(g, 128, 16)?;
        self.players.insert(
            stream,
            SourceState {
                decoder,
                loop_range: cached.source.loop_range(),
            },
        );
        self.live.insert(voice, g.u32(voice + 4)?);
        self.opened += 1;
        self.sounded.remove(&voice);
        crate::voice::report_open(g, request, voice)?;
        if env_set!("SKATE_AUDIO_VOICE_TRACE") {
            eprintln!(
                "SKATE_AUDIO_VOICE open voice={voice:08x} sample={sample:08x} live={}",
                self.live.len()
            );
        }
        Ok(voice)
    }

    fn release(&mut self, g: &mut Guest, voice: u32) -> Result<()> {
        if let Some(player) = self.live.remove(&voice) {
            crate::voice::report_release(voice);
            if env_set!("SKATE_AUDIO_VOICE_TRACE") {
                eprintln!(
                    "SKATE_AUDIO_VOICE release voice={voice:08x} live={}",
                    self.live.len()
                );
            }
            self.defer_stop_graph(g, player)?;
            self.heap.free(g, voice)?;
        }
        Ok(())
    }
    fn suspend(&mut self, g: &mut Guest, voice: u32) -> Result<()> {
        g.set_u8(g.u32(voice + 4)? + 72, 3)
    }
    fn resume(&mut self, g: &mut Guest, voice: u32) -> Result<()> {
        g.set_u8(g.u32(voice + 4)? + 72, 1)
    }
    fn set(&mut self, g: &mut Guest, voice: u32, id: u32, value: u32) -> Result<()> {
        if env_set!("SKATE_AUDIO_KERNEL_TRACE") {
            eprintln!("set {voice:08x} id={id} value={value}");
        }
        let value = f64::from(value as i32);
        match id {
            0 => device::post_property(g, g.u32(voice + 12)?, 0, value / 4096.0)?,
            2 | 5 | 8 => {
                let offset = match id {
                    2 => 40,
                    5 => 48,
                    _ => 44,
                };
                // Store single exactly as the setter's stfs, then multiply its lfs values.
                g.set_u32(voice + offset, ((value / 32767.0) as f32).to_bits())?;
                let master = f64::from(g.f32(voice + 40)?);
                let send = g.u32(voice + 24)?;
                if send != 0 {
                    device::post_property(g, send, 0, master * f64::from(g.f32(voice + 48)?))?;
                }
                device::post_property(
                    g,
                    g.u32(voice + 28)?,
                    0,
                    master * f64::from(g.f32(voice + 44)?),
                )?;
            }
            6 => device::post_property(g, g.u32(voice + 20)?, 0, value)?,
            7 => device::post_property(g, g.u32(voice + 16)?, 0, value)?,
            11 if g.u32(voice + 88)? != 0 => {
                device::post_property(g, g.u32(voice + 84)?, 0, value / 32767.0)?
            }
            _ => {}
        }
        Ok(())
    }
    fn set_alternate(&mut self, g: &mut Guest, voice: u32, value: u32) -> Result<()> {
        device::post_property(
            g,
            g.u32(voice + 32)?,
            0,
            f64::from(value as i32) * (360.0 / 65536.0),
        )
    }
    fn query(&mut self, g: &mut Guest, voice: u32, out: &mut [u32; 11]) -> Result<()> {
        out.fill(0);
        if g.u8(g.u32(voice + 4)? + 71)? == 2 {
            return Ok(());
        }
        out[0] = 1;
        let stream = g.u32(voice + 8)?;
        // A voice whose player has played and then gone idle has ended.
        //
        // **Approximation, deliberate.** Retail reports this through the player's state byte: a
        // finished player is retired by `sub_82B49100`, which stores 2 into `+71`, and the test
        // above is retail's own (`sub_824A2DD8`). Nothing retires a player here, because the
        // per-block `rw_system` driver (`sub_82B48530`, ported as `runtime::run_frame`) is not
        // driven by this runtime and its object list at `SYSTEM+20` stays empty, so a one-shot
        // that finished still read as live for ever. A patch that re-arms a layer when its voice
        // ends -- the skid's second layer -- then never fired again for the life of the message:
        // a powerslide kept one quiet voice instead of retriggering, and the revert that followed
        // stayed ~10 dB below retail's. Replacing this with the real retirement path is the fix.
        // `SKATE_AUDIO_NO_VOICE_END=1` restores the old behaviour, for an A/B by ear.
        let idle = g.u64(stream + pump::END_TIME)? == 0
            && !env_set!("SKATE_AUDIO_NO_VOICE_END");
        if idle && self.sounded.contains(&voice) {
            out[0] = 0;
            return Ok(());
        }
        if !idle {
            self.sounded.insert(voice);
        }
        let status = g.u32(stream + 16)?;
        let duration = f64::from_bits(g.u64(voice + 56)?);
        let unit = f64::from_bits(g.u64(0x822F_8890)?);
        let elapsed = if g.f32(status + 4)? < g.f32(voice + 52)? {
            0.0
        } else {
            f64::from_bits(g.u64(status + 8)?)
        };
        out[1] = crate::fp::fctiwz_low_word((duration - elapsed) * unit);
        out[2] = crate::fp::fctiwz_low_word(elapsed * unit);
        Ok(())
    }
    fn poke(&mut self, _g: &mut Guest, _voice: u32) -> Result<()> {
        Ok(())
    }
}

impl play::PlayHost for AuthoredDevice {
    fn prepare(&mut self, g: &mut Guest, stream: u32, index: u8, sample: u32) -> Result<bool> {
        // A sample without decoded PCM (its stream did not decode, or its bank is not one the
        // runtime was given): the play is abandoned (silence), reported once.
        let Some(cached) = self.sources.get(&sample).cloned() else {
            if self.missing.insert(sample) {
                eprintln!("SKATE_PLAYER_AUDIO play_without_pcm sample={sample:#010x}");
            }
            return Ok(false);
        };
        // A SndPlayer1 the game side's own graphs built (Splice one-shots, grain players, the
        // wheels): it gets its decoder at its first play, from the spare pool.
        if !self.players.contains_key(&stream) {
            if let Some(decoder) = self.grains.take_decoder() {
                self.players.insert(
                    stream,
                    SourceState { decoder, loop_range: cached.source.loop_range() },
                );
            }
        }
        let source = self
            .players
            .get(&stream)
            .ok_or_else(|| Error::new(stream, "play has no decoder owner"))?;
        let decoder = source.decoder;
        let slot = play::slot_address(g, stream, index)?;
        let record = play::record_address(g, stream, index)?;
        let frames = cached
            .source
            .loop_range()
            .map_or(cached.source.frames(), |(_, end)| end) as u32;
        g.set_u32(slot + 8, decoder)?;
        g.set_u32(
            slot + play::SLOT_RATE,
            (cached.sample_rate as f32).to_bits(),
        )?;
        g.set_u32(slot + play::SLOT_QUEUED, frames)?;
        g.set_u32(
            slot + play::SLOT_CURSOR,
            cached
                .source
                .loop_range()
                .map_or(u32::MAX, |(start, _)| start as u32),
        )?;
        g.set_u8(slot + play::SLOT_MODE, cached.source.channels())?;
        g.set_u8(record + play::RECORD_KIND, 0)?;
        g.set_u32(decoder + 36, 64)?;
        g.set_u32(decoder + 64 + 12, frames)?;
        g.set_u8(decoder + 46, cached.source.channels())?;
        g.set_u8(decoder + 50, 1)?;
        g.set_u32(stream + 436, frames)?;
        g.set_u32(stream + 424, g.u32(slot + 12)?)?;
        g.set_u32(stream + 428, (cached.sample_rate as f32).to_bits())?;
        self.streams.attach(decoder, cached.source);
        Ok(true)
    }
    fn decode(&mut self, g: &mut Guest, stream: u32, index: u8, detail: u32) -> Result<bool> {
        // Grain-player hook: a grain play starts at its retail seek frame (`grain::host`).
        if let Some(decoder) = self.players.get(&stream).map(|s| s.decoder) {
            self.grains.seek(
                g,
                &mut self.streams,
                stream,
                index,
                detail,
                decoder,
                STACK - 0x1000,
            )?;
        }
        Ok(true)
    }
}

impl commands::CommandHost for AuthoredDevice {
    fn play(&mut self, g: &mut Guest, record: u32) -> Result<u32> {
        // Grain-player hook: grain graphs are built by the player, not opened through the voice
        // device, so their SndPlayer1 gets its decoder here, as `open` gives bank voices theirs.
        if let Some(stream) = self.grains.begin_play(g, record)? {
            if !self.players.contains_key(&stream) {
                let decoder = self
                    .grains
                    .take_decoder()
                    .ok_or_else(|| Error::new(stream, "no spare decoder for a grain voice"))?;
                self.players.insert(
                    stream,
                    SourceState {
                        decoder,
                        loop_range: None,
                    },
                );
            }
        }
        let result = play::append_resident(g, self, record);
        self.grains.end_play();
        result
    }
    fn stop_player(&mut self, g: &mut Guest, heap: &mut dyn Heap, record: u32) -> Result<u32> {
        let player = g.u32(record + 4)?;
        self.stop_graph(g, heap, player)?;
        // `build_graph` takes the whole graph — header, module table and every module — out of the
        // arena in one allocation, and this deferred teardown is the point at which nothing
        // references it any more. Without this the block leaked: `release` freed only the voice,
        // so every one-shot cost a graph for the rest of the session and a long session ended in
        // "authored audio guest heap exhausted".
        heap.free(g, player)?;
        Ok(8)
    }
}

impl pump::PumpHost for AuthoredDevice {
    fn preflight(&mut self, g: &mut Guest, stream: u32) -> Result<()> {
        let Some(source) = self.players.get(&stream) else {
            return Ok(());
        };
        if let Some((start, end)) = source.loop_range {
            if g.u32(source.decoder + 64 + 12)? == 0 {
                g.set_u32(source.decoder + 28, start as u32)?;
                g.set_u32(source.decoder + 64 + 8, start as u32)?;
                g.set_u32(source.decoder + 64 + 12, end as u32)?;
            }
        }
        for index in 0..20 {
            if g.u8(stream + index * 16 + 113)? == 2 {
                g.set_u8(stream + index * 16 + 113, 0)?;
            }
        }
        Ok(())
    }
    fn retire(&mut self, g: &mut Guest, stream: u32, index: u8) -> Result<()> {
        g.set_u8(play::slot_address(g, stream, index)? + play::SLOT_STATE, 0)
    }
    fn prepare(
        &mut self,
        _g: &mut Guest,
        _stream: u32,
        _index: u8,
        _counter: &mut u32,
    ) -> Result<bool> {
        Ok(true)
    }
    fn at_end(&mut self, g: &mut Guest, stream: u32, index: u8, counter: &mut u32) -> Result<bool> {
        self.submit(g, stream, index, counter)
    }
    fn at_work(
        &mut self,
        _g: &mut Guest,
        _stream: u32,
        _index: u8,
        _counter: &mut u32,
    ) -> Result<(bool, bool)> {
        Ok((false, false))
    }
    fn submit(
        &mut self,
        g: &mut Guest,
        stream: u32,
        _index: u8,
        _counter: &mut u32,
    ) -> Result<bool> {
        let consumer = u32::from(g.u8(stream + 474)?);
        let at = stream + consumer * 16;
        if g.u8(at + 113)? != 1 {
            g.set_u8(at + 112, 0)?;
            g.set_u32(at + 108, 0)?;
            g.set_u8(at + 113, 1)?;
        }
        Ok(false) // all resident frames are ready; one publication completes this pump
    }
}

impl stream::StreamFill for AuthoredDevice {
    fn fill(&mut self, g: &mut Guest, stream: u32, descriptor: u32, frames: u64) -> Result<u64> {
        stream::StreamFill::fill(&mut self.streams, g, stream, descriptor, frames)
    }
}

impl worker::WorkerHost for Owner {
    fn render(&mut self, g: &mut Guest, _worker: u32) -> Result<bool> {
        let mut tick_heap = TickHeap { patch: &mut self.patch_heap, device_frees: Vec::new() };
        interp::tick_with(
            g,
            (256.0f32 / 48000.0) as f64,
            &mut patch::PatchHost {
                heap: &mut tick_heap,
                device: &mut self.device,
            },
        )?;
        for at in tick_heap.device_frees {
            self.device.heap.free(g, at)?;
        }
        // Grain-player hook: retail phase 1 ticks scheduler bucket 0 (the grain plug-ins) before
        // the command drain (`grain::host::tick_bucket_zero`).
        crate::grain::host::tick_bucket_zero(
            g,
            &mut self.device.heap,
            &mut self.device.grains,
            STACK,
        )?;
        let mut heap = std::mem::take(&mut self.device.heap);
        let drained = commands::drain(g, &mut heap, &mut self.device, SYSTEM, STACK);
        self.device.heap = heap;
        self.stats.commands += u64::from(drained?.records);
        // Retail phase 5 (scheduler bucket 1): the environment network's ReverbModel1 tasks.
        let delta = g.f32(SYSTEM + scheduler::SYSTEM_SCHEDULER + 64)?;
        crate::environment::tick_reverbs(g, &mut self.device.heap, &mut mathlib::Image, MANAGER, delta)?;
        for stream in self.device.players.keys().copied().collect::<Vec<_>>() {
            self.device.source_pump(g, stream)?;
        }
        g.set_u64(
            PARAMS,
            (self.stats.blocks as f64 * 256.0 / 48000.0).to_bits(),
        )?;
        g.set_u32(PARAMS + 8, g.u32(SYSTEM + 108)?)?;
        g.set_u32(PARAMS + 12, 48000.0f32.to_bits())?;
        g.set_u16(PARAMS + 20, g.u16(SYSTEM + 280)?)?;
        // `SKATE_AUDIO_KERNEL_TRACE`: every kernel's output peak at block 60, and at the first block
        // after the output reached full scale.
        let clip_trace = self.recent_peak >= 0.999 && self.stats.blocks >= self.clip_next;
        self.recent_peak = 0.0;
        if clip_trace {
            self.clip_next = self.stats.blocks + 188;
        }
        let trace = env_set!("SKATE_AUDIO_KERNEL_TRACE")
            && (self.stats.blocks == 60 || clip_trace);
        if trace {
            eprintln!(
                "nodes={} sources={}",
                g.u16(PARAMS + 20)?,
                self.device.players.len()
            );
            for (&s, state) in &self.device.players {
                let slot = play::slot_address(g, s, g.u8(s + 469)?)?;
                eprintln!(
                    "source {s:08x} slot {slot:08x} words={:?} state={} ready={} decoder={:08x} remaining={}",
                    (0..12)
                        .map(|i| g.u32(slot + i * 4).unwrap())
                        .collect::<Vec<_>>(),
                    g.u8(slot + 46)?,
                    g.u8(s + 113)?,
                    state.decoder,
                    crate::leaves::stream_remaining(g, state.decoder, 0)?
                );
            }
        }
        graph::run_pass(
            g,
            &mut InspectKernels {
                inner: kernels::VoiceKernels {
                    trig: &mut mathlib::Image,
                    fill: &mut self.device,
                    sp: STACK,
                },
                trace,
            },
            PASS,
            PARAMS,
        )?;
        if trace {
            // Which node the pass ended on: the output pass reads what that node left in the
            // pass's first plane, which must be the master.
            let array = g.u32(PARAMS + 8)?;
            let count = u32::from(g.u16(PARAMS + 20)?);
            let orders: Vec<String> = (0..count)
                .map(|i| {
                    let node = g.u32(array + 8 * i).unwrap_or(0);
                    format!("{node:08X}:{:02X}", g.u8(node + 73).unwrap_or(0))
                })
                .collect();
            eprintln!("pass nodes master={:08X} [{}]", g.u32(ROOT + 0x10)?, orders.join(" "));
        }
        g.set_u32(OUTPUT_HOLDER + 28, g.u32(PASS + 28)?)?;
        output::output_pass(g, WORKER, STACK)?;
        self.stats.blocks += 1;
        self.stats.voices_opened = self.device.opened;
        self.stats.live_voices = self.device.live.len();
        Ok(true)
    }
    fn queued_buffers(&mut self, _g: &mut Guest, _worker: u32) -> Result<u32> {
        Ok(0)
    }
    fn submit(&mut self, g: &mut Guest, worker: u32, descriptor: u32) -> Result<()> {
        let index = (descriptor - worker - worker::DESCRIPTORS) / worker::DESCRIPTOR_BYTES;
        let pcm = g.u32(worker + worker::PCM_BASE)? + index * worker::BUFFER_BYTES;
        for sample in 0..worker::BUFFER_BYTES / 4 {
            let value = g.f32(pcm + sample * 4)?;
            if !value.is_finite() {
                return Err(Error::new(
                    pcm + sample * 4,
                    "authored graph produced nonfinite PCM",
                ));
            }
            self.stats.peak = self.stats.peak.max(value.abs());
            self.recent_peak = self.recent_peak.max(value.abs());
            self.submitted.push(value);
        }
        g.set_u32(worker + (index + worker::SLOT_BASE) * 4, 0)
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[test]
    fn a_voice_for_a_sample_without_pcm_fails_its_open_as_retail_does() {
        // vehicle_away#1 in the traffic bank: its XMA stream does not decode whole.
        let sample = 0x6C18_3C6F;
        let request = OpenRequest {
            sample: u64::from(sample),
            index: 1,
            byte2: 0,
            shifted: [0; 6],
            bank_72: 0,
            arg8: 0,
            record_count: 0,
            records: 0,
        };
        let mut device = AuthoredDevice::new();
        let mut g = Guest::from_segments(Vec::new());
        // 0 is the device's open failure: `open_voice` deactivates the voice and the patch's
        // tick goes on, where an `Err` failed the whole frame and silenced its block.
        assert_eq!(device.open(&mut g, &request).unwrap(), 0);
        assert_eq!(device.open(&mut g, &request).unwrap(), 0);
        assert!(device.missing.contains(&sample));
    }
}

#[cfg(test)]
mod environment_tests {
    use super::*;

    /// The TU3 image from `SKATE_AUDIO_IMAGE`.
    fn image() -> Option<Guest> {
        let mut segments = Vec::new();
        for entry in std::fs::read_dir(std::env::var_os("SKATE_AUDIO_IMAGE")?).ok()? {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            let hex = name.strip_prefix("g_")?.strip_suffix(".bin")?;
            segments.push(crate::Segment {
                base: u32::from_str_radix(hex, 16).ok()? << 16,
                bytes: std::fs::read(&path).ok()?,
            });
        }
        Some(Guest::from_segments(segments))
    }

    fn fourcc(word: u32) -> String {
        String::from_utf8_lossy(&word.to_be_bytes()).into_owned()
    }

    #[test]
    #[ignore = "needs the audio runtime image: SKATE_AUDIO_IMAGE=<dir of g_*.bin>"]
    fn the_environment_network_is_built_by_the_original_code() {
        let Some(guest) = image() else { return };
        let rt = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
        let g = &rt.guest;
        let mut shapes = Vec::new();
        for i in 0..13 {
            let player = g.u32(MANAGER + 4 * i).unwrap();
            let modules: Vec<String> = (0..u32::from(g.u8(player + 68).unwrap()))
                .map(|m| {
                    let module = g.u32(player + 80 + 4 * m).unwrap();
                    let class = g.u32(module + 20).unwrap();
                    fourcc(g.u32(class + 36).unwrap())
                })
                .collect();
            let shape = format!("{:02X} {}", g.u8(player + 73).unwrap(), modules.join(" "));
            eprintln!("graph {i:2} @{player:08x}: {shape}");
            shapes.push(shape);
        }
        // manager + 52 is C7's module table: the target each final output's first send feeds.
        assert_eq!(g.u32(MANAGER + 52).unwrap(), g.u32(MANAGER).unwrap() + 80);
        assert_eq!(shapes[0], "C7 Sub0 Sen0 Sen0");
        assert_eq!(shapes[3], "C9 Sub0 Del0 RM10 Gai0 Sen0 Sen0");
        assert_eq!(shapes[5], "CA Sub0 HI20 LI20 Gai0 Pn21 Sen0");
        assert_eq!(shapes[9], "C9 Sub0 Del0 HI20 LI20 Gai0 Pn21 Sen0");
    }

    /// Module `m` of network graph `graph`: its data word `k` as a single.
    fn param(g: &Guest, graph: u32, m: u32, k: u32) -> f32 {
        let module = g.u32(g.u32(MANAGER + 4 * graph).unwrap() + 80 + 4 * m).unwrap();
        g.f32(g.u32(module + 16).unwrap() + 4 * k).unwrap()
    }

    /// Needs `SKATE_REVERB_LAYOUTS`: 24 x (u64 collection key, 0xB0-byte `aud_reverb` layout).
    #[test]
    #[ignore = "needs SKATE_AUDIO_IMAGE and SKATE_REVERB_LAYOUTS"]
    fn reverb02_loads_into_branch_a_as_retail_measured_it() {
        let Some(guest) = image() else { return };
        let Some(path) = std::env::var_os("SKATE_REVERB_LAYOUTS") else { return };
        let blob = std::fs::read(path).unwrap();
        let reverb02 = 0x407A_FA1D_6C7C_EAD8u64; // hash64("reverb02")
        let layout = blob
            .chunks(8 + crate::environment::PRESET_BYTES)
            .find(|c| u64::from_be_bytes(c[..8].try_into().unwrap()) == reverb02)
            .unwrap()[8..]
            .try_into()
            .unwrap();
        let mut rt = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
        rt.load_reverb_preset(layout, 0, 1.0).unwrap();
        for _ in 0..4 {
            rt.pump_once().unwrap();
        }
        let g = &rt.guest;
        // University, outdoors at the start (session-3 Xenia dump, branch A = reverb02).
        let measured = [
            ("C7 send A", param(g, 0, 1, 1), 1.0),
            ("C8a gain", param(g, 1, 1, 1), 0.75),
            ("RM10 A +4", param(g, 3, 2, 1), 1.04),
            ("RM10 A +12", param(g, 3, 2, 3), 8.3),
            ("RM10 A +20", param(g, 3, 2, 5), 1.0),
            ("reverb A return gain", param(g, 3, 3, 1), 1.5),
            ("CA1 HI20", param(g, 5, 1, 1), 0.0),
            ("CA1 LI20", param(g, 5, 2, 1), 10000.0),
            ("CA1 Pn21", param(g, 5, 4, 1), 270.0),
            ("CA2 Pn21", param(g, 6, 4, 1), 90.0),
            ("DP1 LI20", param(g, 9, 3, 1), 8000.0),
            ("DP1 gain", param(g, 9, 4, 1), 1.0),
        ];
        let mut bad = Vec::new();
        for (name, got, want) in measured {
            eprintln!("{name:22} engine {got:10} retail {want}");
            if (got - want).abs() > 1e-4 * want.abs().max(1.0) {
                bad.push(name);
            }
        }
        assert!(bad.is_empty(), "differ from retail: {bad:?}");
    }

    fn layout(key: u64) -> Option<[u8; crate::environment::PRESET_BYTES]> {
        let blob = std::fs::read(std::env::var_os("SKATE_REVERB_LAYOUTS")?).ok()?;
        let chunk = blob
            .chunks(8 + crate::environment::PRESET_BYTES)
            .find(|c| u64::from_be_bytes(c[..8].try_into().unwrap()) == key)?;
        chunk[8..].try_into().ok()
    }

    /// With retail's two presets loaded (reverb02 in A, reverb01 in B), all 16 modulator records
    /// carry retail's static fields, and they move.
    #[test]
    #[ignore = "needs SKATE_AUDIO_IMAGE and SKATE_REVERB_LAYOUTS"]
    fn the_modulators_run_like_retail() {
        let Some(guest) = image() else { return };
        let (Some(a), Some(b)) = (layout(0x407A_FA1D_6C7C_EAD8), layout(0xA278_2D75_A971_CC8C)) else { return };
        let mut rt = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
        rt.wire_environment().unwrap();
        rt.load_reverb_preset(&a, 0, 1.0).unwrap();
        rt.load_reverb_preset(&b, 1, 0.0).unwrap();
        rt.pump_once().unwrap();
        // Retail (session-3 Xenia dump, xnet_uni1.txt) {rate, depth, min, max} per record 0..13.
        let retail: [[f32; 4]; 14] = [
            [10.0, 0.2, 1161.0, 1161.0], [2.5, 0.21, 0.17, 0.17], [2.5641, 0.2, 0.18, 0.18],
            [3.33333, 0.6, 270.0, 90.0], [3.33333, 0.6, 90.0, 90.0], [10.0, 0.34, 1161.0, 1161.0],
            [1.96078, 0.5, 0.28, 0.28], [2.0, 0.45, 0.3, 0.3], [5.0, 0.6, 270.0, 90.0],
            [5.0, 0.6, 90.0, 90.0], [4.0, 0.6, 0.0005, 0.0005], [3.33333, 0.0, 250.0, 250.0],
            [2.0, 0.9, 0.002, 0.002], [3.33333, 0.2, 1500.0, 1500.0],
        ];
        // Records 10..13 wobble the final outputs' Del0/PI20 and are set by a controller that is
        // not a reverb preset (source still unidentified); they are printed, not asserted.
        let mut bad = Vec::new();
        for (r, want) in retail.iter().enumerate() {
            let f = |k: u32| rt.guest.f32(MANAGER + 0x188 + 32 * r as u32 + 4 * k).unwrap();
            let got = [f(1), f(3), f(4), f(5)];
            let same = got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1e-4 * w.abs().max(1.0));
            eprintln!("record {r:2} engine {got:?} retail {want:?} {}", if same { "" } else { "DIFFER" });
            if !same && r < 10 {
                bad.push(r);
            }
        }
        let before = param(&rt.guest, 9, 1, 1);
        for _ in 0..200 {
            rt.pump_once().unwrap();
        }
        let after = param(&rt.guest, 9, 1, 1);
        eprintln!("delay-pan 1 Del0 time {before} -> {after} (retail seen 0.205)");
        assert!(bad.is_empty(), "records differ: {bad:?}");
        assert!(before != after);
    }

    #[test]
    #[ignore = "needs the audio runtime image: SKATE_AUDIO_IMAGE=<dir of g_*.bin>"]
    fn the_environment_network_renders() {
        let Some(guest) = image() else { return };
        let mut rt = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
        let started = std::time::Instant::now();
        let blocks = 400;
        for block in 0..blocks {
            if let Err(e) = rt.pump_once() {
                panic!("block {block}: {e}");
            }
        }
        let per_block = started.elapsed() / blocks;
        let rm = rt.guest.u32(rt.guest.u32(MANAGER + 0xC).unwrap() + 88).unwrap();
        eprintln!(
            "{per_block:?} per 256-frame block (real time: 5.33 ms); RM10 state {} stages {}",
            rt.guest.u32(rm + 0x444).unwrap(),
            rt.guest.u8(rm + 0x441).unwrap()
        );
    }
}
