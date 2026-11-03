//! The title's environmental send network (reverb, delay-pan and their returns), built and run by
//! the original code through [`crate::ppc`].
//!
//! Retail's bus-manager constructor `sub_826DD3A8` calls `sub_8248EEE8` before anything else: it
//! registers nine module classes and builds thirteen graphs, `C7` (the intermediate target every
//! final-output graph sends into) → two `C8` splits → two `C9` ReverbModel1 graphs and four `C9`
//! delay-pan graphs → four `CA` returns → the default output. Without it every sound plays dry.
//!
//! Nothing here transcribes that function or ReverbModel1 (`RM10`). The interpreter runs them, and
//! [`Ports`] hands the callees that already have verified ports (class registration, the graph
//! builder, the delay allocator, the scheduler node pool, module configure) to those ports, so the
//! objects they create are the same objects the rest of the runtime already drives.

use crate::mathlib::Trig;
use crate::patch::Heap;
use crate::ppc::{Cpu, Host, Interp};
use crate::{Guest, Result, classes, delay, device, modules, scheduler};

/// `sub_8248EEE8(manager)`: build the network into the bus manager.
pub const BUILD_NETWORK: u32 = 0x8248_EEE8;
/// ReverbModel1's class functions (descriptor `0x82FD20F4`).
pub const REVERB_SIZE: u32 = 0x82B2_EC00;
pub const REVERB_CONSTRUCT: u32 = 0x82B2_EC08;
pub const REVERB_PROCESS: u32 = 0x82B2_F8C0;
/// The per-frame task the constructor registers at priority `0x4A`.
pub const REVERB_TASK: u32 = 0x82B2_F038;
/// `li r3,0x450 ; blr`.
pub const REVERB_BYTES: u32 = 0x450;

/// Guest stack for interpreted calls. Two tops: a constructor run from inside the network build
/// (through the graph builder) must not land on the build's own frames. Clear of the MixMap span
/// (`0x6800_0000`, 4 MB): sharing its base made `put` swap this stack for the MixMap, and every
/// block's reverb task then wrote its frames over the MixMap's tables.
pub const STACK_BASE: u32 = 0x6900_0000;
pub const STACK_BYTES: u32 = 0x2_0000;
/// `sub_824907D8(manager)`: build the 16 parameter-modulator records at manager `+0x188` and
/// register [`MODULATE`] as a scheduler bucket-0 task.
pub const BUILD_MODULATORS: u32 = 0x8249_07D8;
/// The bucket-0 task `(manager + 0x184, delta)`: advance 14 modulators, post each value.
pub const MODULATE: u32 = 0x8249_0B60;

/// Guest scratch for the reverb controller: its object and the preset layout it points at.
pub const DATA_BASE: u32 = STACK_BASE + STACK_BYTES;
pub const DATA_BYTES: u32 = 0x1000;
const REVERB_OBJECT: u32 = DATA_BASE;
const PRESET_LAYOUT: u32 = DATA_BASE + 0x800;
/// An `aud_reverb` collection's layout block, as `sub_824DE140` reads it (fields up to `+0xAC`).
pub const PRESET_BYTES: usize = 0xB0;
/// `sub_824DE140(this, slot, level)`: copy a preset into `this+0x1B4` and apply it
/// (`sub_8248DD18(manager, block)`).
pub const LOAD_PRESET: u32 = 0x824D_E140;
const OUTER_SP: u32 = STACK_BASE + STACK_BYTES - 0x100;
const INNER_SP: u32 = STACK_BASE + STACK_BYTES / 2 - 0x100;

const REGISTER_CLASS: u32 = 0x82B4_6770;
const BUILD_GRAPH: u32 = 0x82B4_8C48;
const ENTER_CRITICAL: u32 = 0x82F9_CB44;
const LEAVE_CRITICAL: u32 = 0x82F9_CB54;
const DELAY_ALLOCATE: u32 = 0x82B3_D590;
const DELAY_RESIZE: u32 = 0x82B3_D668;
const POOL_GROW: u32 = 0x82B3_9728;
const POOL_TAKE: u32 = 0x82B3_95E8;
const POOL_RECYCLE: u32 = 0x82B3_9690;

/// ReverbModel1's per-sample stages, reached through the function pointers its process wires:
/// the mix stage `sub_82B39FA0` ([`crate::stage::run_stage`]) and the allpass `sub_82B38B68`
/// ([`crate::dsp::allpass::dispatch_stage`]), both verified ports. Interpreted, they were most of
/// the reverb's cost (≈ 0.25 ms of every 5.3 ms block).
const MIX_STAGE: u32 = 0x82B3_9FA0;
const ALLPASS_STAGE: u32 = 0x82B3_8B68;

pub(crate) const HOOKS: [u32; 15] = [
    MIX_STAGE,
    ALLPASS_STAGE,
    REGISTER_CLASS,
    BUILD_GRAPH,
    ENTER_CRITICAL,
    LEAVE_CRITICAL,
    DELAY_ALLOCATE,
    DELAY_RESIZE,
    POOL_GROW,
    POOL_TAKE,
    POOL_RECYCLE,
    device::CONFIGURE_NONE,
    device::CONFIGURE_SEND,
    device::CONFIGURE_FADER,
    device::CONFIGURE_SNDPLAYER,
];

/// The callees with verified ports, run natively against the runtime's own heap.
pub(crate) struct Ports<'a, H: Heap + ?Sized, T: Trig + ?Sized> {
    pub heap: &'a mut H,
    pub trig: &'a mut T,
}

impl<H: Heap + ?Sized, T: Trig + ?Sized> Host for Ports<'_, H, T> {
    fn hook(&mut self, cpu: &mut Cpu, g: &mut Guest, target: u32) -> Result<()> {
        let r = |i: usize| cpu.r[i] as u32;
        cpu.r[3] = match target {
            REGISTER_CLASS => classes::register_class(g, r(3), r(4))? as u64,
            BUILD_GRAPH => {
                modules::build_graph(g, self.heap, self.trig, r(3), r(4) as u8, r(5), r(6))? as u64
            }
            // ponytail: the audio worker is the only thread touching these objects.
            ENTER_CRITICAL | LEAVE_CRITICAL => return Ok(()),
            MIX_STAGE => {
                crate::stage::run_stage(g, r(3), cpu.r[4], cpu.r[5], r(7), r(1))?;
                return Ok(());
            }
            ALLPASS_STAGE => match crate::dsp::allpass::dispatch_stage(g, cpu.r[3], cpu.r[4], cpu.r[5], cpu.r[7])? {
                crate::dsp::allpass::DispatchOutcome::Ran(result) => result.r3,
                crate::dsp::allpass::DispatchOutcome::Cleared => return Ok(()),
            },
            DELAY_ALLOCATE => delay::allocate(g, self.heap, r(3), r(4), r(5), r(6))? as u64,
            DELAY_RESIZE => delay::resize(g, self.heap, r(3), r(4))? as u64,
            POOL_GROW => modules::pool_grow(g, self.heap, r(3), cpu.r[4])? as u64,
            POOL_TAKE => modules::pool_take(g, self.heap, r(3), r(4))? as u64,
            POOL_RECYCLE => {
                scheduler::recycle_node(g, r(3), r(4))?;
                return Ok(());
            }
            // The module `vtable+4` configure entries the network's Send wiring reaches.
            _ => {
                device::configure(g, r(3), r(4), r(5))?;
                return Ok(());
            }
        };
        Ok(())
    }
}

/// Run guest `target` with `args` in r3.. and `floats` in f1.., on the stack top `sp`.
fn run<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    sp: u32,
    target: u32,
    args: &[u64],
    floats: &[f64],
) -> Result<u64> {
    run_with(g, &mut Ports { heap, trig }, &HOOKS, sp, target, args, floats)
}

/// [`run`] with another host and hook set.
fn run_with(
    g: &mut Guest,
    host: &mut dyn Host,
    hooks: &[u32],
    sp: u32,
    target: u32,
    args: &[u64],
    floats: &[f64],
) -> Result<u64> {
    let mut interp = Interp::new();
    interp.hooks.extend(hooks);
    interp.cpu.r[1] = sp as u64;
    for (i, &f) in floats.iter().enumerate() {
        interp.cpu.f[1 + i] = f;
    }
    interp.call(g, host, target, args)
}

mod world;
pub use world::{
    AUD_REVERB, AUDIO_VOLUME, OUTPUT_WOBBLE, ReverbFrame, ReverbWorld, WORLD_BASE, WORLD_BYTES,
    WorldClass,
};

/// `sub_8248EEE8(manager)`. The caller has mapped [`STACK_BASE`] and set up the registry, the
/// system and the default output the network's last sends target.
pub fn build_network<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    manager: u32,
) -> Result<()> {
    run(g, heap, trig, OUTER_SP, BUILD_NETWORK, &[manager as u64], &[])?;
    Ok(())
}

/// ReverbModel1's constructor on a laid-out instance, for [`modules::construct`].
pub fn construct_reverb<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    instance: u32,
    arg: u32,
) -> Result<bool> {
    let r3 = run(g, heap, trig, INNER_SP, REVERB_CONSTRUCT, &[instance as u64, arg as u64], &[])?;
    Ok(r3 as u8 != 0)
}

/// The network's two ReverbModel1 instances: module 2 of the `C9` reverb graphs at manager `+0xC`
/// and `+0x10`.
fn reverbs(g: &Guest, manager: u32) -> Result<[u32; 2]> {
    let module = |slot: u32| -> Result<u32> { g.u32(g.u32(manager + slot)? + 80 + 2 * 4) };
    Ok([module(0xC)?, module(0x10)?])
}

/// RM10's scheduler task `sub_82B2F038(instance)` for both reverbs, at retail's bucket-1 point in
/// the block (after the command drain).
///
/// ponytail: called directly rather than through a bucket-1 tick, because bucket 1 also holds the
/// SndPlayer1 nodes this runtime drives through its own source pump; ticking the bucket would
/// drive them twice. Tick the bucket once SndPlayer1's task is the one driving playback.
pub fn tick_reverbs<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    manager: u32,
    delta: f32,
) -> Result<()> {
    for instance in reverbs(g, manager)? {
        run(g, heap, trig, INNER_SP, REVERB_TASK, &[instance as u64], &[delta as f64])?;
    }
    Ok(())
}

/// A process never allocates; if RM10's ever does, that is an error naming the call.
struct NoHeap;

impl Heap for NoHeap {
    fn alloc(&mut self, _g: &mut Guest, _size: u32, _align: u32) -> Result<u32> {
        Err(crate::Error::new(REVERB_PROCESS, "ReverbModel1 process allocated"))
    }
    fn free(&mut self, _g: &mut Guest, at: u32) -> Result<()> {
        Err(crate::Error::new(at, "ReverbModel1 process freed"))
    }
}

/// ReverbModel1's process `sub_82B2F8C0(object, owner)`, for the kernel dispatch.
pub fn process_reverb<T: Trig + ?Sized>(g: &mut Guest, trig: &mut T, object: u32, owner: u32) -> Result<u64> {
    run(g, &mut NoHeap, trig, INNER_SP, REVERB_PROCESS, &[object as u64, owner as u64], &[])
}

/// The audio framework's root graphs, as `sub_826D4C30` builds them after the bus root: the class
/// lookups `sub_828DD760(root)`, then FE `sub_828DDF78` (`Sub0 DCl0 Gai0 LI20 HI20 Sen0`, the master
/// section every output targets through `[[root+0x2C]]`) and the two C8 VU buses `sub_828DDC58` /
/// `sub_828DDDE8` (`Sub0 VuM0 Sen0`). All of them send into `[system+0x34]`, the DAC graph's input
/// (FF `Sub0 Scp0 Dac0`, `sub_828DD9F0`), which the caller provides.
pub fn build_root_graphs<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    root: u32,
) -> Result<()> {
    let registry = g.u32(root + 12)?;
    // SampleCapture, Dac, VuMeter: registered by the audio device init, not by the runtime's.
    for descriptor in [0x82FD_2634, 0x82FC_DDAC, 0x82FD_2FF8, device::CLIP_DESCRIPTOR] {
        classes::register_class(g, registry, descriptor)?;
    }
    for function in [0x828D_D760, 0x828D_DF78, 0x828D_DC58, 0x828D_DDE8] {
        run(g, heap, trig, OUTER_SP, function, &[root as u64], &[])?;
    }
    Ok(())
}

/// Any module class function without a native port, run as original code: a constructor
/// `(instance, arg)` (from [`modules::construct`]).
pub fn interpret_construct<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    function: u32,
    instance: u32,
    arg: u32,
) -> Result<bool> {
    let r3 = run(g, heap, trig, INNER_SP, function, &[instance as u64, arg as u64], &[])?;
    Ok(r3 as u8 != 0)
}

/// A class table's prepare `(object, owner, flag, request)` or process `(object, owner, flag)`
/// without a native port, run as original code (from [`crate::kernels`]).
pub fn interpret_kernel<T: Trig + ?Sized>(
    g: &mut Guest,
    trig: &mut T,
    function: u32,
    args: &[u64],
) -> Result<u64> {
    run(g, &mut NoHeap, trig, INNER_SP, function, args, &[])
}

/// A command-ring handler without a native port, run as original code on its record (from
/// [`crate::commands::dispatch`]); returns the record size it consumed.
pub fn interpret_command<H: Heap + ?Sized>(g: &mut Guest, heap: &mut H, handler: u32, record: u32) -> Result<u32> {
    let r3 = run(g, heap, &mut crate::mathlib::Image, INNER_SP, handler, &[record as u64], &[])?;
    Ok(r3 as u32)
}

/// A hooked module entry (e.g. a configure id the port does not cover), run as original code with
/// the entry itself left unhooked.
pub fn interpret_entry(g: &mut Guest, entry: u32, args: &[u64]) -> Result<u64> {
    let hooks: Vec<u32> = HOOKS.iter().copied().filter(|&h| h != entry).collect();
    run_with(
        g,
        &mut Ports { heap: &mut NoHeap, trig: &mut crate::mathlib::Image },
        &hooks,
        INNER_SP,
        entry,
        args,
        &[],
    )
}

/// Load an `aud_reverb` preset into network branch `slot` (0 = A, 1 = B) with its C7 send at
/// `level`, by running the title's own loader. The game's reverb object is reduced to what the
/// loader reads: the slot's layout pointer at `+0x20 + slot*0xB8`. The parameter posts it queues
/// are applied by the next command drain.
pub fn load_preset<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    layout: &[u8; PRESET_BYTES],
    slot: u32,
    level: f32,
) -> Result<()> {
    let at = PRESET_LAYOUT + slot * PRESET_BYTES as u32;
    g.set_span(at, layout)?;
    g.set_u32(REVERB_OBJECT + 0x20 + slot * 0xB8, at)?;
    run(g, heap, trig, OUTER_SP, LOAD_PRESET, &[REVERB_OBJECT as u64, slot as u64], &[level as f64])?;
    Ok(())
}

/// `sub_824907D8(manager)`, after the final-output graphs exist (retail order).
pub fn build_modulators<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    manager: u32,
) -> Result<()> {
    run(g, heap, trig, OUTER_SP, BUILD_MODULATORS, &[manager as u64], &[])?;
    Ok(())
}

/// The [`MODULATE`] task body, for the bucket-0 scheduler host.
pub fn modulate<H: Heap + ?Sized, T: Trig + ?Sized>(
    g: &mut Guest,
    heap: &mut H,
    trig: &mut T,
    context: u32,
    delta: f32,
) -> Result<()> {
    run(g, heap, trig, INNER_SP, MODULATE, &[context as u64], &[delta as f64])?;
    Ok(())
}
