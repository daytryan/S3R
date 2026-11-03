//! The retail audio game side, run as original code: the root at `*(0x830CFDC4)` that
//! `sub_826D5500` allocates, its fourteen `CSTATEMGR_*` managers, their `CSTATE_*` slots and the
//! `SFXObj_*` components (`docs/audio-object-registry.md`), booted and driven through
//! [`crate::ppc`] the way [`crate::environment`] drives SFXObj_Reverb.
//!
//! The runtime below it (patch programs, voices, the mixer, the verified DSP kernels) stays native;
//! the calls the game side makes into it, and into the rest of the game (allocators, files,
//! Attrib, physics), are hooked. Which calls those are is found by running the code with call
//! tracing on ([`crate::ppc::CallTrace`]) and hooking each place it stops.

use crate::patch::Heap;
use crate::ppc::{CallTrace, Cpu, Host, Interp};
use crate::{Error, Guest, Result};

/// `*(0x830CFDC4)`: the game-side root.
pub const ROOT_GLOBAL: u32 = 0x830C_FDC4;
/// `sub_826D5500()`: the title's audio boot. In order: the bus manager (`sub_826DD3A8`), a
/// `0x1C1B8` object at `0x830CFDDC` (`sub_824A3C28`), the resource manager at `0x830CFDCC`
/// (`0x30418`, `sub_828DBD98`), the root (`0x890`, `sub_824845B8`), the listener
/// (`sub_824AD380`).
pub const AUDIO_BOOT: u32 = 0x826D_5500;
/// The bus manager's constructor: [`crate::authored`]'s device already built it natively.
const BUS_MANAGER_BOOT: u32 = 0x826D_D3A8;
/// `sub_82484ED8(root)`: registries, managers, MixMap (AUDIO_HANDOFF §9.4).
pub const ROOT_INIT: u32 = 0x8248_4ED8;

/// The game side's own heap and stack spans.
pub const GAME_STACK: u32 = 0x6B00_0000;
pub const GAME_STACK_BYTES: u32 = 0x4_0000;
pub const GAME_HEAP: u32 = 0x6C00_0000;
pub const GAME_HEAP_BYTES: u32 = 0x0400_0000;

/// EA's memory manager, `*(0x830CFEEC)`: every allocation entry point (`sub_828AAC28`,
/// `sub_828AAD30`, …) finds heap `i` at `[[mm+4]+0xC1B4] + 0x6C·i + 0xC` and calls its vtable:
/// slot 1 `Alloc(size, name, flags, align, offset)`, slot 2 `Alloc(size, name, flags)`, slot 3
/// `Free(p, size)`. Installing one whose heaps all share a vtable of host functions sends every
/// game-side allocation to the runtime's own heap; the title's manager (`sub_828AB0E8`, which
/// carves console memory) never runs.
const MEMORY_MANAGER: u32 = 0x830C_FEEC;
const HEAP_TABLE: u32 = 0xC1B4;
const HEAP_ENTRY: u32 = 0x6C;
const HEAP_COUNT: u32 = 256;
/// `sub_82476088(*out, name)`: the heap index for a named allocation category (an FNV-1 lookup
/// in the registry `sub_828AB0E8` would build). Every heap is the same one here, so: 0.
const HEAP_BY_NAME: u32 = 0x8247_6088;
/// Host function addresses for fake vtables: never fetched, only branched to (and hooked).
const HOST_FUNCS: u32 = 0x6BF0_0000;
const HOST_SLOTS: u32 = 16;
const HOOKS: std::ops::Range<u32> = HOST_FUNCS..HOST_FUNCS + 4 * HOST_SLOTS;
/// The audio system's allocator (`[*(0x8307762C)+0x24]`, the same ICoreAllocator shape): what the
/// title allocates through it (graphs, pools, stream players) the native mixer owns, so it goes to
/// the runtime's graph heap.
const DEVICE_FUNCS: u32 = 0x6BF2_0000;
const DEVICE_HOOKS: std::ops::Range<u32> = DEVICE_FUNCS..DEVICE_FUNCS + 4 * HOST_SLOTS;
/// A heap entry's second interface (at the ICoreAllocator `+4`, i.e. entry `+0x10`): slot 6
/// `Alloc(*out, this, request {size, align, ...})` returns `{pointer, ...}` through `out` (the
/// trigger system's cell pool, `sub_828EB888`).
const REQUEST_FUNCS: u32 = 0x6BF3_0000;
const REQUEST_HOOKS: std::ops::Range<u32> = REQUEST_FUNCS..REQUEST_FUNCS + 4 * HOST_SLOTS;
/// The game object's mode machine (`[game+0x70]`): slot 4 → the current mode, whose slot 1
/// (`GetId(*out, this)`) the audio update compares with `0x01FFFFFF`, the front end, which runs
/// only the Main and Environment managers (`sub_82485608`); any other mode is gameplay (every
/// manager's frame intake, the listener, both halves).
const MODE_FUNCS: u32 = 0x6BF4_0000;
const MODE_HOOKS: std::ops::Range<u32> = MODE_FUNCS..MODE_FUNCS + 4 * HOST_SLOTS;
pub const FRONT_END: u32 = 0x01FF_FFFF;
pub const IN_GAME: u32 = 0;
/// An inert object's vtable: every slot returns 0.
const INERT_FUNCS: u32 = 0x6BF1_0000;
const INERT_SLOTS: u32 = 64;
const INERT_HOOKS: std::ops::Range<u32> = INERT_FUNCS..INERT_FUNCS + 4 * INERT_SLOTS;
/// `sub_82528410()`: creates `*(0x830CFDE4)` (`0x688` bytes, `sub_824984F8`), the console
/// notification and custom-soundtrack (XMP) listener, over EA's system messaging. The engine has
/// neither system notifications nor custom soundtracks, so it is an inert object: zeroed, every
/// virtual returning 0 ("no soundtrack playing"). `0x830CFDE0` gets its owner-thread handle.
const SYSTEM_LISTENER_BOOT: u32 = 0x8252_8410;
/// The CRT's `_onexit(fn)` (`sub_82F4D870`): the static constructors register their destructors
/// here, in a table the CRT heap would hold. The runtime never runs the title's exit path, so a
/// registration is accepted (returns `fn`) and dropped.
const ON_EXIT: u32 = 0x82F4_D870;
const SYSTEM_LISTENER: u32 = 0x830C_FDE4;
const SYSTEM_LISTENER_BYTES: u32 = 0x688;

fn inert_object(g: &mut Guest, heap: &mut dyn Heap, bytes: u32) -> Result<u32> {
    let vtable = heap.alloc(g, 4 * INERT_SLOTS, 16)?;
    for slot in 0..INERT_SLOTS {
        g.set_u32(vtable + 4 * slot, INERT_FUNCS + 4 * slot)?;
    }
    let object = heap.alloc(g, bytes, 16)?;
    g.set_u32(object, vtable)?;
    Ok(object)
}

/// Install the memory manager described at [`MEMORY_MANAGER`].
pub fn install_allocator(g: &mut Guest, heap: &mut dyn Heap) -> Result<()> {
    let vtable = heap.alloc(g, 4 * HOST_SLOTS, 16)?;
    for slot in 0..HOST_SLOTS {
        g.set_u32(vtable + 4 * slot, HOST_FUNCS + 4 * slot)?;
    }
    let requests = heap.alloc(g, 4 * HOST_SLOTS, 16)?;
    for slot in 0..HOST_SLOTS {
        g.set_u32(requests + 4 * slot, REQUEST_FUNCS + 4 * slot)?;
    }
    let table = heap.alloc(g, HEAP_ENTRY * HEAP_COUNT, 16)?;
    for i in 0..HEAP_COUNT {
        g.set_u32(table + HEAP_ENTRY * i + 0xC, vtable)?;
        g.set_u32(table + HEAP_ENTRY * i + 0x10, requests)?;
    }
    let heaps = heap.alloc(g, HEAP_TABLE + 4, 16)?;
    g.set_u32(heaps + HEAP_TABLE, table)?;
    let mm = heap.alloc(g, 16, 16)?;
    g.set_u32(mm + 4, heaps)?;
    g.set_u32(MEMORY_MANAGER, mm)?;
    // EA-core ICoreAllocator globals `main()` (`sub_826D35F8`) points at a heap entry's object.
    for global in CORE_ALLOCATORS {
        g.set_u32(global, table + 0xC)?;
    }
    Ok(())
}

/// ICoreAllocator globals the title's boot points at an EA heap entry object: `0x83084440`
/// (`main()`, `sub_826D35F8` at `826D3648`), `0x8308454C` (the file system's, `sub_82A88998`),
/// `0x83083CCC` (the audio framework's, `bus root + 0x84`, set by `sub_828DD590`; `sub_828E2730`
/// allocates through it), and every global the image assigns a heap entry object (`mulli 0x6C ;
/// addi 0xC ; stw`: `main()` sets `0x83083BD0` (Attrib instances), `0x83084100`; the rest are set
/// by other subsystems' inits).
const CORE_ALLOCATORS: [u32; 12] = [
    0x8308_4440,
    0x8308_454C,
    0x8308_3CCC,
    0x8308_3BD0,
    0x8308_4100,
    0x8308_46B8,
    0x8308_434C,
    0x8308_3CD4,
    0x8308_5454,
    0x830C_A9C4,
    0x8308_4544,
    0x8308_293C,
];

/// The image's kernel import stubs: 16-byte slots whose word at `+4`... is `0x01LLOOOO` before the
/// loader patches them (library `LL`: 0 = xam, 1 = xboxkrnl; `OOOO` = export ordinal).
const STUBS: std::ops::Range<u32> = 0x82F9_C000..0x82FA_8000;
const XAM: u32 = 0;
const XBOXKRNL: u32 = 1;

/// `(library, ordinal)` of every import stub in the image.
pub fn import_stubs(g: &Guest) -> std::collections::HashMap<u32, (u32, u32)> {
    STUBS
        .step_by(4)
        .filter(|at| at % 16 == 4)
        .filter_map(|at| {
            let w = g.u32(at).ok()?;
            (w >> 24 == 1 && (w >> 16) & 0xFF <= XBOXKRNL).then_some((at, ((w >> 16) & 0xFF, w & 0xFFFF)))
        })
        .collect()
}

/// The kernel services the game side calls, for one thread: the audio worker runs all of it, so
/// locks are no-ops, waits succeed at once, and handles are opaque numbers. What would change the
/// sound (threads, files, XAudio, XMA, formatted strings) is not stubbed: it stops with its name.
#[derive(Default)]
pub struct Kernel {
    tls: Vec<u32>,
    next_handle: u32,
}

impl Kernel {
    fn handle(&mut self) -> u32 {
        self.next_handle += 1;
        0xF800_0000 | (self.next_handle << 2)
    }

    /// `Some(r3)` for an import it serves.
    fn call(
        &mut self,
        g: &mut Guest,
        heap: &mut dyn Heap,
        cpu: &Cpu,
        library: u32,
        ordinal: u32,
    ) -> Result<Option<u64>> {
        let r = |i: usize| cpu.r[i] as u32;
        if library == XAM {
            return Ok(match ordinal {
                0x28A => Some(self.handle() as u64), // XamNotifyCreateListener
                0x28B => Some(0),                    // XNotifyGetNext: nothing pending
                0x3CD => Some(1),                    // XGetLanguage: English
                0x3CC => Some(0xFF),                 // XGetGameRegion: all
                // XMsgInProcessCall(app, message, buffer, length): only the music player (XMP,
                // app 0xFA), answered as Xenia's XmpApp does on a console with no custom
                // soundtrack playing — stopped, volume 1, the title in control.
                0x1F4 if r(3) == 0xFA => {
                    // (out-pointer slots in the argument block, value written through each)
                    let writes: &[(u32, u32)] = match r(4) {
                        0x0007_0009 => &[(4, 0)],                         // GetStatus: stopped
                        0x0007_000B => &[(4, 0x3F80_0000)],               // GetVolume: 1.0
                        0x0007_001B => &[(4, 0), (8, 0)],                 // GetPlaybackController
                        0x0007_0029 => &[(4, 0), (8, 0), (12, 0)],        // GetPlaybackBehavior
                        _ => &[],
                    };
                    for &(slot, value) in writes {
                        let at = g.u32(r(5) + slot)?;
                        g.set_u32(at, value)?;
                    }
                    Some(0)
                }
                _ => None,
            });
        }
        Ok(Some(match ordinal {
            // RtlInitialize/Enter/LeaveCriticalSection, Ke(Enter|Leave)CriticalRegion,
            // KfReleaseSpinLock, KeReleaseSpinLockFromRaisedIrql, KeAcquireSpinLockAtRaisedIrql.
            0x12E | 0x125 | 0x130 | 0x5F | 0x7D | 0xB4 | 0x89 | 0x4D => cpu.r[3],
            0x12F => 0,          // RtlInitializeCriticalSectionAndSpinCount
            0x141 => 1,          // RtlTryEnterCriticalSection
            0xB1 => 0,           // KfAcquireSpinLock: the old IRQL
            0x03 => 0,           // DbgPrint
            0x66 => 1,           // KeGetCurrentProcessType: title
            0x81 => 0,           // KeQueryBasePriorityThread
            0x97 | 0x99 => 0,    // KeSetAffinityThread, KeSetBasePriorityThread
            0x5A | 0x101 => 0,   // KeDelayExecutionThread, NtYieldExecution
            0x83 => 50_000_000,  // KeQueryPerformanceFrequency
            0x194 => 0,          // XexCheckExecutablePrivilege: not privileged
            0x84 => {
                // KeQuerySystemTime(*time)
                g.set_u64(r(3), 0x01D0_0000_0000_0000)?;
                0
            }
            // NtDuplicateObject(source, *target, options)
            0xDA => {
                let h = self.handle();
                g.set_u32(r(4), h)?;
                0
            }
            // NtCreateEvent / NtCreateSemaphore / NtCreateMutant / NtCreateTimer (*handle, ...)
            0xD1 | 0xD5 | 0xD4 | 0xD7 => {
                let h = self.handle();
                g.set_u32(r(3), h)?;
                0
            }
            // NtSetEvent, NtClearEvent, KeSetEvent, KeResetEvent, NtReleaseSemaphore,
            // NtReleaseMutant, NtClose, NtCancelTimer, NtSetTimerEx, ObDereferenceObject.
            0xF6 | 0xCE | 0x9D | 0x8F | 0xF3 | 0xF2 | 0xCF | 0xCD | 0xFA | 0x105 => 0,
            // KeWaitForSingleObject, NtWaitForSingleObjectEx, KeWaitForMultipleObjects: signalled.
            0xB0 | 0xFD | 0xAF => 0,
            // KeTlsAlloc / KeTlsFree / KeTlsGetValue / KeTlsSetValue
            0x152 => {
                self.tls.push(0);
                (self.tls.len() - 1) as u64
            }
            0x153 => 1,
            0x154 => self.tls.get(r(3) as usize).copied().unwrap_or(0) as u64,
            0x155 => match self.tls.get_mut(r(3) as usize) {
                Some(slot) => {
                    *slot = r(4);
                    1
                }
                None => 0,
            },
            // ExAllocatePoolTypeWithTag(size, tag, type) / ExFreePool(p)
            0x0B => heap.alloc(g, r(3), 16)? as u64,
            0x0F => {
                heap.free(g, r(3))?;
                0
            }
            // MmAllocatePhysicalMemoryEx(flags, size, protect, min, max, align) / MmFreePhysicalMemory(type, p)
            0xBA => heap.alloc(g, r(4), r(8).clamp(16, 4096))? as u64,
            0xBD => {
                heap.free(g, r(4))?;
                0
            }
            0xBE => cpu.r[3], // MmGetPhysicalAddress: identity
            0xC4 => 4,        // MmQueryAddressProtect: PAGE_READWRITE
            // NtAllocateVirtualMemory(*base, *size, type, protect) / NtFreeVirtualMemory
            0xCC => {
                let size = g.u32(r(4))?;
                let at = heap.alloc(g, size, 4096)?;
                g.set_u32(r(3), at)?;
                0
            }
            0xDC => 0,
            // RtlInitAnsiString(*string, source)
            0x12C => {
                let (dst, src) = (r(3), r(4));
                let mut len = 0u32;
                while src != 0 && g.u8(src + len)? != 0 {
                    len += 1;
                }
                g.set_u16(dst, len as u16)?;
                g.set_u16(dst + 2, if src == 0 { 0 } else { len as u16 + 1 })?;
                g.set_u32(dst + 4, src)?;
                cpu.r[3]
            }
            // RtlFillMemoryUlong(dst, bytes, pattern)
            0x126 => {
                for k in 0..r(4) / 4 {
                    g.set_u32(r(3) + 4 * k, r(5))?;
                }
                cpu.r[3]
            }
            _ => return Ok(None),
        }))
    }
}

/// `*(0x83084614)`: the Attrib database the lookups start from (`sub_82B69B08`).
const DATABASE: u32 = 0x8308_4614;
/// `Database::GetClass(db, class key)`, `Class::GetCollection(class, key)`, and class iteration
/// `FirstKey(class)` / `NextKey(class, key)`: answered from the vault. Everything built on them
/// (`sub_82B69B08`, the instance constructors, field reads) runs as original code.
const DB_GET_CLASS: u32 = 0x82B6_BC30;
const CLASS_GET_COLLECTION: u32 = 0x82B6_DE10;
const FIRST_KEY: u32 = 0x82B6_DED0;
const NEXT_KEY: u32 = 0x82B6_DF30;
/// `Collection::Release`: vault collections live as long as the runtime, so a no-op.
const COLLECTION_RELEASE: u32 = 0x82B6_9978;
/// `GetAttributePointer(collection, field key, index)`: the field's data in this collection or
/// the nearest parent that has it, element `index` of an array.
const ATTRIBUTE_POINTER: u32 = 0x82B7_2420;
const ATTRIB_HOOKS: [u32; 7] = [
    FIND_NODE,
    DB_GET_CLASS,
    CLASS_GET_COLLECTION,
    FIRST_KEY,
    NEXT_KEY,
    COLLECTION_RELEASE,
    ATTRIBUTE_POINTER,
];
/// The vault's two collection files in guest memory, above the game heap
/// (`0x6C00_0000..0x7000_0000`; inside it, the heap's segment shadowed them).
const ATTRIB_VLT: u32 = 0x7000_0000;
const ATTRIB_BIN: u32 = 0x7040_0000;
/// The zero block the title points an instance at when a collection has no layout.
const NO_LAYOUT: u32 = 0x830D_0850;

/// The Attrib objects handed to original code, with the fields it reads:
/// * class: `+0` key (for the hooks), `+8` → private block with `+0x26` u16 collection count
///   (`sub_828DA0D8`);
/// * collection: `+4` reference count, `+8` and `+0x20` class, `+0x10` parent collection,
///   `+0x18` key, `+0x24` layout, `+0x28` nonzero (`sub_82B6CC78` Instance, `sub_828DA0D8`,
///   `sub_82B72420`).
pub struct AttribHost {
    vault: crate::attrib::Vault,
    classes: std::collections::HashMap<u64, u32>,
    collections: std::collections::HashMap<(u64, u64), u32>,
    keys_of: std::collections::HashMap<u32, (u64, u64)>,
    /// Synthesized nodes for in-layout fields, by (collection object, field).
    layout_nodes: std::collections::HashMap<(u32, u64), u32>,
}

/// `sub_82B722C0(collection, field, *owner)`: the field's node in this collection or the nearest
/// parent that has it (its `.vlt` entry; for an in-layout field a node with flag `0x10` and the
/// layout offset), with the collection that owns it. The accessors built on it run as original code.
const FIND_NODE: u32 = 0x82B7_22C0;
/// A node's runtime flags (byte `+0xF`), as the accessors (`sub_82B6D740`, `sub_82B6D848`,
/// `sub_82B72420`) test them.
const NODE_PRESENT: u8 = 0x80;
const NODE_INLINE: u8 = 0x40;
const NODE_LAYOUT: u8 = 0x10;
const NODE_ARRAY: u8 = 0x02;

impl AttribHost {
    pub fn new(g: &mut Guest, heap: &mut dyn Heap, vault: crate::attrib::Vault) -> Result<Self> {
        vault.install(g, ATTRIB_VLT, ATTRIB_BIN)?;
        let database = heap.alloc(g, 64, 16)?;
        g.set_u32(DATABASE, database)?;
        Ok(Self {
            vault,
            classes: Default::default(),
            collections: Default::default(),
            keys_of: Default::default(),
            layout_nodes: Default::default(),
        })
    }

    /// See [`FIND_NODE`]: `(node, owner)`, or `(0, 0)`.
    fn node(&mut self, g: &mut Guest, heap: &mut dyn Heap, object: u32, field: u64) -> Result<(u32, u32)> {
        let Some(&(class, mut key)) = self.keys_of.get(&object) else { return Ok((0, 0)) };
        let Some(spec) = self.vault.classes.get(&class).and_then(|c| c.fields.get(&field)).copied() else {
            return Ok((0, 0));
        };
        loop {
            let Some(c) = self.vault.collections.get(&(class, key)) else { return Ok((0, 0)) };
            let (has_layout, entry, parent) = (c.layout.is_some(), c.nodes.get(&field).copied(), c.parent);
            let owner = self.collection(g, heap, class, key)?;
            if spec.flags & 2 != 0 {
                if has_layout {
                    if let Some(&n) = self.layout_nodes.get(&(owner, field)) {
                        return Ok((n, owner));
                    }
                    let n = heap.alloc(g, 16, 16)?;
                    g.set_u64(n, field)?;
                    g.set_u32(n + 8, u32::from(spec.offset))?;
                    g.set_u16(n + 12, 0)?;
                    g.set_u8(n + 14, 0)?;
                    g.set_u8(n + 15, NODE_PRESENT | NODE_LAYOUT | if spec.flags & 1 != 0 { NODE_ARRAY } else { 0 })?;
                    self.layout_nodes.insert((owner, field), n);
                    return Ok((n, owner));
                }
            } else if let Some(e) = entry {
                // The runtime flags the loader derives from the schema (the file holds 0 there).
                let inline = spec.size <= 4 && spec.flags & 1 == 0;
                let flags = NODE_PRESENT
                    | if spec.flags & 1 != 0 { NODE_ARRAY } else { 0 }
                    | if inline { NODE_INLINE } else { 0 };
                g.set_u8(ATTRIB_VLT + e + 15, flags)?;
                return Ok((ATTRIB_VLT + e, owner));
            }
            if parent == 0 {
                return Ok((0, 0));
            }
            key = parent;
        }
    }

    fn class(&mut self, g: &mut Guest, heap: &mut dyn Heap, key: u64) -> Result<u32> {
        if !self.vault.classes.contains_key(&key) {
            return Ok(0);
        }
        if let Some(&object) = self.classes.get(&key) {
            return Ok(object);
        }
        let object = heap.alloc(g, 64, 16)?;
        let private = heap.alloc(g, 64, 16)?;
        g.set_u64(object, key)?;
        g.set_u32(object + 8, private)?;
        let count = self.vault.classes[&key].order.len();
        g.set_u16(private + 0x26, count.min(0xFFFF) as u16)?;
        self.classes.insert(key, object);
        Ok(object)
    }

    fn collection(&mut self, g: &mut Guest, heap: &mut dyn Heap, class: u64, key: u64) -> Result<u32> {
        let Some((layout, parent)) = self
            .vault
            .collections
            .get(&(class, key))
            .map(|c| (c.layout, c.parent))
        else {
            return Ok(0);
        };
        if let Some(&object) = self.collections.get(&(class, key)) {
            return Ok(object);
        }
        let class_object = self.class(g, heap, class)?;
        let object = heap.alloc(g, 64, 16)?;
        // Registered before the parent is resolved, so a cycle ends here.
        self.collections.insert((class, key), object);
        self.keys_of.insert(object, (class, key));
        g.set_u32(object + 4, 1)?;
        g.set_u32(object + 8, class_object)?;
        g.set_u32(object + 0x20, class_object)?;
        g.set_u64(object + 0x18, key)?;
        g.set_u32(object + 0x24, layout.map_or(NO_LAYOUT, |at| ATTRIB_BIN + at))?;
        g.set_u32(object + 0x28, 1)?;
        let parent_class = self.vault.class_of.get(&parent).copied();
        if let Some(parent_class) = parent_class.filter(|_| parent != 0) {
            let parent_object = self.collection(g, heap, parent_class, parent)?;
            g.set_u32(object + 0x10, parent_object)?;
        }
        Ok(object)
    }

    /// See [`ATTRIBUTE_POINTER`]. The rules are `tools/asset_pipeline/vlt.py`'s: a field the schema
    /// marks in-layout (flags bit 1) is at `layout + offset`; any other is the collection's entry
    /// (inline in the `.vlt` entry, or in the `.bin`); an array (flags bit 0) is `{u16 capacity,
    /// count, stride, pad}` then elements aligned to `1 << alignment`.
    fn attribute(&self, g: &Guest, object: u32, field: u64, index: u32) -> Result<u32> {
        use crate::attrib::Data;
        let Some(&(class, mut key)) = self.keys_of.get(&object) else { return Ok(0) };
        let Some(spec) = self.vault.classes.get(&class).and_then(|c| c.fields.get(&field)) else {
            return Ok(0);
        };
        let base = loop {
            let Some(c) = self.vault.collections.get(&(class, key)) else { return Ok(0) };
            if spec.flags & 2 != 0 {
                if let Some(layout) = c.layout {
                    break ATTRIB_BIN + layout + u32::from(spec.offset);
                }
            } else if let Some(data) = c.entries.get(&field) {
                break match *data {
                    Data::Inline(at) => ATTRIB_VLT + at,
                    Data::Bin(at) => ATTRIB_BIN + at,
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

    fn keys(&self, g: &Guest, class_object: u32) -> Result<&[u64]> {
        let class = g.u64(class_object)?;
        Ok(self.vault.classes.get(&class).map_or(&[][..], |c| c.order.as_slice()))
    }

    /// `Some(r3)` for an Attrib hook.
    fn call(&mut self, g: &mut Guest, heap: &mut dyn Heap, cpu: &Cpu, target: u32) -> Result<Option<u64>> {
        Ok(Some(match target {
            DB_GET_CLASS => self.class(g, heap, cpu.r[4])? as u64,
            CLASS_GET_COLLECTION => {
                let class = g.u64(cpu.r[3] as u32)?;
                self.collection(g, heap, class, cpu.r[4])? as u64
            }
            FIRST_KEY => self.keys(g, cpu.r[3] as u32)?.first().copied().unwrap_or(0),
            NEXT_KEY => {
                let keys = self.keys(g, cpu.r[3] as u32)?;
                keys.iter()
                    .position(|k| *k == cpu.r[4])
                    .and_then(|i| keys.get(i + 1))
                    .copied()
                    .unwrap_or(0)
            }
            COLLECTION_RELEASE => cpu.r[3],
            FIND_NODE => {
                let (node, owner) = self.node(g, heap, cpu.r[3] as u32, cpu.r[4])?;
                if env_set!("SKATE_ATTRIB_DEBUG") {
                    let words: Vec<String> = (0..4).map(|i| format!("{:08X}", g.u32(node + 4 * i).unwrap_or(0))).collect();
                    let data = g.u32(node + 8).unwrap_or(0);
                    let at: Vec<String> = (0..6).map(|i| format!("{:08X}", g.u32(data + 4 * i).unwrap_or(0))).collect();
                    eprintln!("ATTRIB node field={:016X} -> {node:08X} [{}] data [{}]", cpu.r[4], words.join(" "), at.join(" "));
                }
                g.set_u32(cpu.r[5] as u32, owner)?;
                node as u64
            }
            ATTRIBUTE_POINTER => {
                let at = self.attribute(g, cpu.r[3] as u32, cpu.r[4], cpu.r[5] as u32)?;
                if env_set!("SKATE_ATTRIB_DEBUG") {
                    eprintln!("ATTRIB get object={:08X} field={:016X} -> {at:08X} [{:08X}]", cpu.r[3] as u32, cpu.r[4], g.u32(at).unwrap_or(0xDEAD));
                }
                if at == 0 && env_set!("SKATE_ATTRIB_DEBUG") {
                    let (class, key) = self.keys_of.get(&(cpu.r[3] as u32)).copied().unwrap_or_default();
                    let spec = self.vault.classes.get(&class).map(|c| c.fields.get(&cpu.r[4]).map(|f| (f.offset, f.flags)));
                    let c = self.vault.collections.get(&(class, key)).map(|c| (c.layout, c.parent, c.entries.contains_key(&cpu.r[4])));
                    eprintln!("ATTRIB miss object={:08X} class={class:016X} key={key:016X} field={:016X} spec={spec:?} coll={c:?}", cpu.r[3] as u32, cpu.r[4]);
                }
                at as u64
            }
            _ => return Ok(None),
        }))
    }
}

/// Everything the game side keeps between calls.
pub struct GameState {
    pub heap: crate::authored::ArenaHeap,
    pub kernel: Kernel,
    pub attrib: AttribHost,
    pub files: Files,
    /// The processor control region `r13` points at ([`PCR_BYTES`]).
    pub pcr: u32,
    /// The camera-update stamp game-mode slot 13 returns: advanced once per [`frame`], so the
    /// listener (`sub_8248CC08`, which compares it with its `+0x80`) derives its velocity from
    /// the camera's movement every frame, as it does in the running game.
    pub camera_stamp: u32,
    /// The world only: the player manager's per-frame sync ([`PLAYER_SYNC`]) is skipped, so a
    /// published skater makes no player states (and no player sounds) while the listener, reverb
    /// and the rest still follow its record. For a runtime whose player audio is its own.
    pub world_only: bool,
    /// The engine's pause (menus, cutscenes, the editor): SFXObj_Pause's input 0, which ducks every
    /// player level to silence. Its own process (`sub_824E1D00`) tests game state this side never
    /// has, so the request is written at each MixMap tick instead ([`MIXMAP_TICK`]).
    pub paused: bool,
    /// With `paused`: a cutscene, not a menu, holds play; the player ducks and the world plays on.
    pub cutscene: bool,
    /// The weather's duck on the ambience bed (1 = none): scales SFXObj_Ambience's controller gain (output 0,
    /// which its update `sub_824D42C8` reads through vfunc60) after each MixMap tick. Host-side, not retail.
    pub ambience_duck: f32,
    /// The traffic block [`publish_traffic`] writes the cars into (0 before the first car).
    pub traffic: u32,
    /// The DMO block [`publish_dmos`] writes the list into (0 before the first DMO).
    pub dmos: u32,
}

/// `sub_824F1FB0`: CSTATEMGR_Player's per-frame sync of its states with the frame's skaters.
const PLAYER_SYNC: u32 = 0x824F_1FB0;

/// The title's files, by the path it asks for (`data/audio/...`, either slash, any case): what
/// `sub_8298ED88` reads whole. `.csi` projects the runtime installed natively are handed back at
/// their installed address instead (a project is patched in place once; see [`INSTALL_PROJECT`]).
pub struct Files {
    /// Streams the title opened (`sub_8298C768`), by handle index: their paths.
    pub open: Vec<String>,
    pub read: Box<dyn FnMut(&str) -> Option<Vec<u8>>>,
    /// `len` bytes of a file from `offset` (fewer at its end), for reads into big archives.
    pub read_at: Box<dyn FnMut(&str, u64, usize) -> Option<Vec<u8>>>,
    /// Every path the title loaded, with where it went.
    pub loaded: Vec<(String, u32)>,
    /// Decode a sound bank the title loaded (`.abk` patch bank or `SPLC`) into PCM by EAAC header
    /// offset; the runtime installs it at the bank's address.
    pub decode: Box<dyn FnMut(&str, &[u8]) -> Vec<(u32, crate::pcm::CachedPcm)>>,
    /// Banks loaded since the runtime last installed their PCM: `(path, address, bytes)`.
    pub banks: Vec<(String, u32, Vec<u8>)>,
}

impl Files {
    fn note(&mut self, path: String, at: u32, bytes: &[u8]) {
        let lower = path.to_ascii_lowercase();
        let bank = [".abk", ".bnk", ".snr", ".big", ".grain"].iter().any(|e| lower.ends_with(e))
            || bytes.get(..4) == Some(b"SPLC");
        if bank && at != 0 {
            self.banks.push((path.clone(), at, bytes.to_vec()));
        }
        self.loaded.push((path, at));
    }
}

impl Files {
    pub fn none() -> Self {
        Self {
            open: Vec::new(),
            read: Box::new(|_| None),
            read_at: Box::new(|_, _, _| None),
            loaded: Vec::new(),
            decode: Box::new(|_, _| Vec::new()),
            banks: Vec::new(),
        }
    }
}

/// `sub_8298ED88(path, allocator, 0, 1, align)`: read a whole file into a fresh block; returns it.
const LOAD_FILE: u32 = 0x8298_ED88;
/// `sub_8298F430(fs, path, 1, align, request, allocator)`: the file system's asynchronous whole-file
/// read. On completion it calls the request's vtable slot 1 (`sub_828DBCA8` for the resource
/// manager's), which stores `+8` data, `+0xC` size and sets byte `+4`; the caller polls `+4`.
/// Served synchronously: the request is complete when the call returns.
const READ_FILE: u32 = 0x8298_F430;
/// `sub_82B1DF50(bank, fixups)`: install a patch bank ([`crate::patch::install_bank`]; the port
/// leaves out the AEMS unload-handler registration `sub_82B48260` and the first bank's interpreter
/// registration, which the runtime's own per-block interpreter tick stands for).
const INSTALL_BANK: u32 = 0x82B1_DF50;
/// `sub_82486FE0(dest, *size)`: read the first `*size` bytes of `dataudio/audiofiles.big` (its
/// table of contents, for the root's by-name member lookups) into `dest`; `*size` becomes what was
/// read.
const READ_ARCHIVE_HEAD: u32 = 0x8248_6FE0;
const ARCHIVE: &str = "data/audio/audiofiles.big";
/// `sub_8298C768(file, path, mode)`: open a file stream for later reads (the EA file system's
/// `sub_82A83080`), storing the stream at `file+0`. Served with an unmapped marker handle
/// ([`STREAM_HANDLES`] + 16·index) so whatever touches the stream later stops by name.
const OPEN_STREAM: u32 = 0x8298_C768;
pub const STREAM_HANDLES: u32 = 0x7E00_0000;
/// `sub_8298F5B0(fs, path, align or buffer, flags)`: queue an asynchronous whole-file read; returns
/// the request (`+0x10C` state, 4 = done; `+0x141` failed; `+0x138` data; `+0x120` u64 size). Served
/// complete: the file is read at once. `sub_8298F930(request)` (status) then runs as original code;
/// `sub_8298FA10(request, *data, *size)` (take the result and retire the request into the file
/// system, which is not there) is served from the request's fields.
const QUEUE_READ: u32 = 0x8298_F5B0;
const TAKE_READ: u32 = 0x8298_FA10;
const READ_REQUEST_BYTES: u32 = 0x150;
/// The AEMS seam: the game side's post (`sub_828E2B48(slot, payload, *node)`), redeliver
/// (`sub_828E2D18(node, payload)`) and release (`sub_828E2C78(node)`) run the runtime's ports
/// ([`crate::patch`]), so every instance they make is the one the runtime's interpreter ticks.
/// `SKATE_AEMS_TRACE` prints each call with the payload's first 28 words.
/// `sub_8294BAE8(manager, dt)`: the MixMap manager's tick, run by the runtime's port
/// ([`crate::mixmap::tick`], on the same host layout) — interpreted it was most of a frame.
const MIXMAP_TICK: u32 = 0x8294_BAE8;
const AEMS_POST: u32 = 0x828E_2B48;
/// What an observer of the seam sees: 0 post (object name, new node), 1 redeliver (node), 2
/// release (node); the payload's 28 words (empty for a release).
pub type AemsObserver = fn(kind: u8, object: &str, node: u32, payload: &[u32]);
static AEMS_OBSERVER: std::sync::Mutex<Option<AemsObserver>> = std::sync::Mutex::new(None);

/// Report every post, redelivery and release the game side makes to `observer` (traces).
pub fn observe_aems(observer: AemsObserver) {
    *AEMS_OBSERVER.lock().unwrap_or_else(|p| p.into_inner()) = Some(observer);
}
const AEMS_REDELIVER: u32 = 0x828E_2D18;
const AEMS_RELEASE: u32 = 0x828E_2C78;

/// `SKATE_AUDIO_MIXMAP_SHADOW` totals: ticks compared, bytes the port wrote (so a match is not a
/// tick that did nothing), ticks whose memory then differed from the original's, the bytes that
/// differed, and ticks the original could not finish.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MixMapShadow {
    pub ticks: u64,
    pub written: u64,
    pub diverged: u64,
    pub bytes: u64,
    pub failed: u64,
}
static MIXMAP_SHADOW: std::sync::Mutex<MixMapShadow> = std::sync::Mutex::new(MixMapShadow {
    ticks: 0,
    written: 0,
    diverged: 0,
    bytes: 0,
    failed: 0,
});

pub fn mixmap_shadow_totals() -> MixMapShadow {
    *MIXMAP_SHADOW.lock().unwrap_or_else(|p| p.into_inner())
}

/// The byte runs where `a` and `b` differ outside the game stack, as `(address, length)`.
fn guest_differences(a: &Guest, b: &Guest) -> Vec<(u32, usize)> {
    let mut runs = Vec::new();
    for (x, y) in a.segments().iter().zip(b.segments()) {
        if x.base != y.base || x.bytes.len() != y.bytes.len() {
            runs.push((x.base, usize::MAX));
            continue;
        }
        if x.base == GAME_STACK {
            continue;
        }
        let mut i = 0;
        while i < x.bytes.len() {
            if x.bytes[i] == y.bytes[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < x.bytes.len() && x.bytes[i] != y.bytes[i] {
                i += 1;
            }
            runs.push((x.base + start as u32, i - start));
        }
    }
    runs
}

/// The MixMap tick's original code (`sub_8294BAE8`, with every callee interpreted) on a copy of
/// the guest taken just before the port ran, then every byte outside the game stack compared with
/// what the port left. A diagnostic: it never changes the live guest.
fn mixmap_shadow(original: &mut Guest, port: &Guest, cpu: &Cpu, manager: u32) {
    let written: u64 = guest_differences(original, port).iter().map(|r| r.1 as u64).sum();
    let mut interp = Interp::new();
    interp.cpu.r[13] = cpu.r[13];
    // Below the live frames, so the interpreted call cannot overwrite them.
    interp.cpu.r[1] = cpu.r[1] - 0x2000;
    interp.cpu.f[1] = cpu.f[1];
    interp.cpu.fpscr = cpu.fpscr;
    let result = interp.call(original, &mut crate::ppc::NoHost, MIXMAP_TICK, &[u64::from(manager)]);
    let mut totals = MIXMAP_SHADOW.lock().unwrap_or_else(|p| p.into_inner());
    totals.ticks += 1;
    totals.written += written;
    if let Err(e) = result {
        totals.failed += 1;
        if totals.failed <= 3 {
            eprintln!("MIXMAP_SHADOW original failed: {e}");
        }
        return;
    }
    let runs = guest_differences(original, port);
    if runs.is_empty() {
        return;
    }
    totals.diverged += 1;
    totals.bytes += runs.iter().map(|r| r.1.min(1 << 20) as u64).sum::<u64>();
    if totals.diverged <= 3 {
        let shown: Vec<String> = runs
            .iter()
            .take(12)
            .map(|&(at, len)| {
                let word = at & !3;
                format!(
                    "{at:08X}+{len} orig {:08X} port {:08X}",
                    original.u32(word).unwrap_or(0),
                    port.u32(word).unwrap_or(0)
                )
            })
            .collect();
        eprintln!(
            "MIXMAP_SHADOW tick {} diverged in {} runs: {}",
            totals.ticks,
            runs.len(),
            shown.join(" | ")
        );
    }
}
/// The CRT's `calloc(count, size)` (`sub_82F56C20`): its heap (`_crtheap`) is never set up here
/// (the CRT startup's heap init stops), so it takes zeroed blocks from the game heap. (First reached
/// by `_getptd`, the per-thread data `rand` keeps its seed in.)
const CRT_CALLOC: u32 = 0x82F5_6C20;
/// The processor control region `r13` points at on the console: `+0x100` the current thread
/// (`+0x160` its last error), `+0x150` zero. One per game side, for the one thread it runs on.
const PCR_BYTES: u32 = 0x1000;
const THREAD_BYTES: u32 = 0x400;
/// `sub_828E2818(csi)`: install a `.csi` project ([`crate::symbols::install_project`]).
const INSTALL_PROJECT: u32 = 0x828E_2818;

/// The installed project (list at [`crate::symbols::PROJECT_LIST_HEAD`], node at `+32`) whose id and
/// table counts equal the file's.
fn installed_project(g: &Guest, file: &[u8]) -> Result<Option<u32>> {
    let be16 = |at: usize| file.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let want = [be16(10), be16(12), be16(14), be16(16)];
    let mut node = g.u32(crate::symbols::PROJECT_LIST_HEAD)?;
    let mut guard = 0;
    while node != 0 && guard < 4096 {
        let csi = node - 32;
        let have = [g.u16(csi + 10)?, g.u16(csi + 12)?, g.u16(csi + 14)?, g.u16(csi + 16)?];
        if want.iter().zip(have).all(|(w, h)| *w == Some(h)) {
            return Ok(Some(csi));
        }
        node = g.u32(node)?;
        guard += 1;
    }
    Ok(None)
}

/// Whether another bank on the installed list ([`crate::patch::BANK_LIST`], node at `+80`) has the
/// same header as `bank`: its first 56 bytes, which installing leaves alone (it writes `+60` on).
fn installed_copy(g: &Guest, bank: u32) -> Result<bool> {
    const HEADER: usize = 56;
    let want = g.span(bank, HEADER)?.to_vec();
    let mut node = g.u32(crate::patch::BANK_LIST)?;
    let mut guard = 0;
    while node != 0 && guard < 4096 {
        let other = node - 80;
        if other != bank && g.span(other, HEADER)? == want.as_slice() {
            return Ok(true);
        }
        node = g.u32(node)?;
        guard += 1;
    }
    Ok(false)
}

fn guest_string(g: &Guest, mut at: u32) -> Result<String> {
    let mut bytes = Vec::new();
    while bytes.len() < 1024 {
        let b = g.u8(at)?;
        if b == 0 {
            break;
        }
        bytes.push(b);
        at += 1;
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

struct Game<'a> {
    heap: &'a mut dyn Heap,
    /// The runtime's graph heap: objects the native mixer drives (graphs, modules, delay lines,
    /// scheduler nodes) are allocated there by the verified ports ([`crate::environment::HOOKS`]).
    device: &'a mut dyn Heap,
    kernel: &'a mut Kernel,
    attrib: &'a mut AttribHost,
    files: &'a mut Files,
    imports: &'a std::collections::HashMap<u32, (u32, u32)>,
    camera_stamp: u32,
    paused: bool,
    cutscene: bool,
    ambience_duck: f32,
}

impl Host for Game<'_> {
    fn hook(&mut self, cpu: &mut Cpu, g: &mut Guest, target: u32) -> Result<()> {
        let r = |i: usize| cpu.r[i] as u32;
        // Only hooked for a world-only game side.
        if target == PLAYER_SYNC {
            return Ok(());
        }
        if let Some(&(library, ordinal)) = self.imports.get(&target) {
            return match self.kernel.call(g, self.heap, cpu, library, ordinal)? {
                Some(r3) => {
                    cpu.r[3] = r3;
                    Ok(())
                }
                None => {
                    let lib = if library == XAM { "xam" } else { "xboxkrnl" };
                    Err(Error::new(target, format!("kernel import {lib} {ordinal:#06x}")))
                }
            };
        }
        // (`CONFIGURE_NONE` is the image's shared empty virtual, `blr`: it runs as itself.)
        if crate::environment::HOOKS.contains(&target) && target != crate::device::CONFIGURE_NONE {
            let mut ports = crate::environment::Ports { heap: self.device, trig: &mut crate::mathlib::Image };
            return ports.hook(cpu, g, target);
        }
        if let Some(r3) = self.attrib.call(g, self.heap, cpu, target)? {
            cpu.r[3] = r3;
            return Ok(());
        }
        if target == BUS_MANAGER_BOOT {
            return Ok(());
        }
        if target == ON_EXIT {
            return Ok(());
        }
        if INERT_HOOKS.contains(&target) {
            cpu.r[3] = 0;
            return Ok(());
        }
        if target == SYSTEM_LISTENER_BOOT {
            let object = inert_object(g, self.heap, SYSTEM_LISTENER_BYTES)?;
            g.set_u32(SYSTEM_LISTENER, object)?;
            let handle = self.kernel.handle();
            g.set_u32(SYSTEM_LISTENER - 4, handle)?;
            return Ok(());
        }
        if target == LOAD_FILE {
            let path = guest_string(g, r(3))?;
            let Some(bytes) = (self.files.read)(&path) else {
                return Err(Error::new(target, format!("file not served: {path}")));
            };
            let at = match path.to_ascii_lowercase().ends_with(".csi") {
                true => installed_project(g, &bytes)?,
                false => None,
            };
            let at = match at {
                Some(at) => at,
                None => {
                    let at = self.heap.alloc(g, bytes.len().max(1) as u32, r(7).max(16))?;
                    g.set_span(at, &bytes)?;
                    at
                }
            };
            self.files.note(path, at, &bytes);
            cpu.r[3] = at as u64;
            return Ok(());
        }
        if target == READ_FILE {
            let path = guest_string(g, r(4))?;
            let request = r(7);
            let (data, size) = match (self.files.read)(&path) {
                Some(bytes) => {
                    let at = self.heap.alloc(g, bytes.len().max(1) as u32, r(6).max(16))?;
                    g.set_span(at, &bytes)?;
                    self.files.note(path, at, &bytes);
                    (at, bytes.len() as u32)
                }
                None => {
                    self.files.note(path, 0, &[]);
                    (0, 0)
                }
            };
            g.set_u32(request + 8, data)?;
            g.set_u32(request + 0xC, size)?;
            g.set_u8(request + 4, 1)?;
            cpu.r[3] = 1;
            return Ok(());
        }
        if target == READ_ARCHIVE_HEAD {
            let want = g.u32(r(4))? as usize;
            let bytes = (self.files.read_at)(ARCHIVE, 0, want)
                .ok_or_else(|| Error::new(target, format!("file not served: {ARCHIVE}")))?;
            g.set_span(r(3), &bytes)?;
            g.set_u32(r(4), bytes.len() as u32)?;
            return Ok(());
        }
        if target == OPEN_STREAM {
            let path = guest_string(g, r(4))?;
            let handle = STREAM_HANDLES + 16 * self.files.open.len() as u32;
            self.files.open.push(path);
            g.set_u32(r(3), handle)?;
            return Ok(());
        }
        if target == QUEUE_READ {
            let path = guest_string(g, r(4))?;
            let request = self.heap.alloc(g, READ_REQUEST_BYTES, 16)?;
            g.fill(request, 0, READ_REQUEST_BYTES)?;
            let (data, size) = match (self.files.read)(&path) {
                Some(bytes) => {
                    let at = if r(5) >= 0x1_0000 {
                        r(5)
                    } else {
                        self.heap.alloc(g, bytes.len().max(1) as u32, r(5).max(16))?
                    };
                    g.set_span(at, &bytes)?;
                    self.files.note(path, at, &bytes);
                    (at, bytes.len() as u32)
                }
                None => {
                    self.files.note(path, 0, &[]);
                    (0, 0)
                }
            };
            g.set_u32(request + 0x10C, 4)?;
            g.set_u8(request + 0x141, (data == 0) as u8)?;
            g.set_u32(request + 0x138, data)?;
            g.set_u64(request + 0x120, size as u64)?;
            cpu.r[3] = request as u64;
            return Ok(());
        }
        if target == TAKE_READ {
            let request = r(3);
            let failed = g.u8(request + 0x141)? != 0;
            g.set_u8(request + 0x143, 1)?;
            if r(4) != 0 {
                g.set_u32(r(4), if failed { 0 } else { g.u32(request + 0x138)? })?;
            }
            if r(5) != 0 {
                g.set_u32(r(5), if failed { 0 } else { g.u64(request + 0x120)? as u32 })?;
            }
            return Ok(());
        }
        if target == MIXMAP_TICK {
            pause(g, r(3), self.paused, self.cutscene)?;
            let shadow = env_set!("SKATE_AUDIO_MIXMAP_SHADOW").then(|| g.clone());
            crate::mixmap::tick(g, r(3), cpu.f[1])?;
            duck_ambience(g, r(3), self.ambience_duck)?;
            if let Some(mut original) = shadow {
                mixmap_shadow(&mut original, g, cpu, r(3));
            }
            return Ok(());
        }
        if target == AEMS_POST || target == AEMS_REDELIVER || target == AEMS_RELEASE {
            if env_set!("SKATE_AEMS_TRACE") {
                let payload = if target == AEMS_RELEASE { 0 } else { r(4) };
                let words: Vec<String> = (0..28)
                    .map(|i| format!("{:X}", g.u32(payload + 4 * i).unwrap_or(0)))
                    .collect();
                let name = match target {
                    AEMS_POST => {
                        let record = g.u32(r(3)).unwrap_or(0);
                        format!("post {}", guest_string(g, g.u32(record + 4).unwrap_or(0)).unwrap_or_default())
                    }
                    AEMS_REDELIVER => format!("redeliver {:08X}", r(3)),
                    _ => format!("release {:08X}", r(3)),
                };
                eprintln!("AEMS {name} [{}]", if payload == 0 { String::new() } else { words.join(" ") });
            }
            let observer = *AEMS_OBSERVER.lock().unwrap_or_else(|p| p.into_inner());
            let payload: Vec<u32> = match (observer, target) {
                (Some(_), AEMS_POST | AEMS_REDELIVER) => {
                    (0..28).map(|i| g.u32(r(4) + 4 * i).unwrap_or(0)).collect()
                }
                _ => Vec::new(),
            };
            let status = match target {
                AEMS_POST => crate::patch::post(g, self.device, r(3), r(4), r(5))?,
                AEMS_REDELIVER => crate::patch::redeliver(g, r(3), r(4))?,
                _ => crate::patch::release_message(g, self.device, r(3))?,
            };
            if let Some(observe) = observer {
                match target {
                    AEMS_POST => {
                        let record = g.u32(r(3)).unwrap_or(0);
                        let name = guest_string(g, g.u32(record + 4).unwrap_or(0)).unwrap_or_default();
                        observe(0, &name, g.u32(r(5)).unwrap_or(0), &payload);
                    }
                    AEMS_REDELIVER => observe(1, "", r(3), &payload),
                    _ => observe(2, "", r(3), &[]),
                }
            }
            cpu.r[3] = status as i64 as u64;
            return Ok(());
        }
        if target == CRT_CALLOC {
            let bytes = r(3).saturating_mul(r(4)).max(1);
            let at = self.heap.alloc(g, bytes, 16)?;
            g.fill(at, 0, bytes)?;
            cpu.r[3] = at as u64;
            return Ok(());
        }
        if target == INSTALL_BANK {
            // A bank the runtime already installed natively (a hand-ported player's) is not
            // installed twice: each install links its input records as listeners on the objects'
            // symbols, so a second copy made every post open both (every sound doubled).
            if installed_copy(g, r(3))? {
                cpu.r[3] = 0;
                return Ok(());
            }
            let first = crate::patch::install_bank(g, r(3), r(4), r(1))?;
            cpu.r[3] = first as u64;
            return Ok(());
        }
        if target == INSTALL_PROJECT {
            let csi = r(3);
            let mut node = g.u32(crate::symbols::PROJECT_LIST_HEAD)?;
            let mut installed = false;
            while node != 0 && !installed {
                installed = node - 32 == csi;
                node = g.u32(node)?;
            }
            if !installed {
                crate::symbols::install_project(g, csi)?;
            }
            cpu.r[3] = 0;
            return Ok(());
        }
        if target == HEAP_BY_NAME {
            g.set_u32(r(3), 0)?;
            return Ok(());
        }
        if PLAYER_HOOKS.contains(&target) {
            match (target - PLAYER_FUNCS) / 4 {
                11 => cpu.r[3] = 0,
                23 => cpu.r[3] = g.u32(r(3) + 8)? as u64,
                slot => return Err(Error::new(target, format!("player slot {slot}"))),
            }
            return Ok(());
        }
        if MODE_HOOKS.contains(&target) {
            match (target - MODE_FUNCS) / 4 {
                4 => cpu.r[3] = g.u32(r(3) + 8)? as u64,
                1 => g.set_u32(r(3), IN_GAME)?,
                // The camera-update stamp: the listener recomputes its velocity only when it
                // differs from the one it kept at `+0x80` (see `GameState::camera_stamp`).
                13 => cpu.r[3] = u64::from(self.camera_stamp),
                slot => return Err(Error::new(target, format!("game mode slot {slot}"))),
            }
            return Ok(());
        }
        if REQUEST_HOOKS.contains(&target) {
            return match (target - REQUEST_FUNCS) / 4 {
                6 => {
                    let (size, align) = (g.u32(r(5))?, g.u32(r(5) + 4)?);
                    let at = self.heap.alloc(g, size, align.max(16))?;
                    g.set_u32(r(3), at)?;
                    g.set_u32(r(3) + 4, size)?;
                    Ok(())
                }
                // Free(this, request {pointer, ...}) (the MixMap build's scratch, `sub_8294CC48`).
                7 => {
                    let at = g.u32(r(4))?;
                    if at != 0 {
                        self.heap.free(g, at)?;
                    }
                    Ok(())
                }
                slot => Err(Error::new(target, format!("allocator request interface slot {slot}"))),
            };
        }
        let (heap, base) = match DEVICE_HOOKS.contains(&target) {
            true => (&mut *self.device, DEVICE_FUNCS),
            false => (&mut *self.heap, HOST_FUNCS),
        };
        match (target - base) / 4 {
            1 => cpu.r[3] = heap.alloc(g, r(4), r(7).max(16))? as u64,
            2 => cpu.r[3] = heap.alloc(g, r(4), 16)? as u64,
            3 => heap.free(g, r(4))?,
            slot => return Err(Error::new(target, format!("allocator vtable slot {slot}"))),
        }
        Ok(())
    }
}

/// Run guest `target(args)` on the game stack with call tracing; the trace comes back either way.
pub fn run(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    target: u32,
    args: &[u64],
) -> (Result<u64>, CallTrace) {
    run_with_floats(g, state, device, target, args, &[])
}

/// [`run`] with `floats` in f1...
pub fn run_with_floats(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    target: u32,
    args: &[u64],
    floats: &[f64],
) -> (Result<u64>, CallTrace) {
    let imports = import_stubs(g);
    let mut interp = Interp::new();
    interp.hooks.extend(HOOKS);
    interp.hooks.extend(DEVICE_HOOKS);
    interp.hooks.extend(REQUEST_HOOKS);
    interp.hooks.extend(MODE_HOOKS);
    interp.hooks.extend(PLAYER_HOOKS);
    interp.hooks.extend(ATTRIB_HOOKS);
    interp.hooks.insert(BUS_MANAGER_BOOT);
    interp.hooks.insert(HEAP_BY_NAME);
    interp.hooks.insert(SYSTEM_LISTENER_BOOT);
    interp.hooks.insert(ON_EXIT);
    interp.hooks.extend(crate::environment::HOOKS.iter().filter(|&&h| h != crate::device::CONFIGURE_NONE));
    interp.hooks.insert(LOAD_FILE);
    interp.hooks.insert(READ_FILE);
    interp.hooks.insert(INSTALL_BANK);
    interp.hooks.insert(CRT_CALLOC);
    interp.cpu.r[13] = state.pcr as u64;
    interp.hooks.extend([AEMS_POST, AEMS_REDELIVER, AEMS_RELEASE]);
    if std::env::var_os("SKATE_AUDIO_MIXMAP_INTERPRETED").is_none() {
        interp.hooks.insert(MIXMAP_TICK);
    }
    interp.hooks.insert(READ_ARCHIVE_HEAD);
    interp.hooks.insert(OPEN_STREAM);
    interp.hooks.insert(QUEUE_READ);
    interp.hooks.insert(TAKE_READ);
    interp.hooks.insert(INSTALL_PROJECT);
    interp.hooks.extend(INERT_HOOKS.step_by(4));
    interp.hooks.extend(imports.keys());
    if state.world_only {
        interp.hooks.insert(PLAYER_SYNC);
    }
    interp.trace = Some(CallTrace::default());
    interp.cpu.r[1] = (GAME_STACK + GAME_STACK_BYTES - 0x100) as u64;
    for (i, &f) in floats.iter().enumerate() {
        interp.cpu.f[1 + i] = f;
    }
    let mut host = Game {
        heap: &mut state.heap,
        device,
        kernel: &mut state.kernel,
        attrib: &mut state.attrib,
        files: &mut state.files,
        imports: &imports,
        camera_stamp: state.camera_stamp,
        paused: state.paused,
        cutscene: state.cutscene,
        ambience_duck: state.ambience_duck,
    };
    let result = interp.call(g, &mut host, target, args);
    (result, interp.trace.take().unwrap_or_default())
}

/// Map the game side's spans, install the allocator and the vault.
pub fn prepare(g: &mut Guest, vault: crate::attrib::Vault, files: Files) -> Result<GameState> {
    g.put(GAME_STACK, vec![0; GAME_STACK_BYTES as usize]);
    g.put(GAME_HEAP, vec![0; GAME_HEAP_BYTES as usize]);
    let mut heap = crate::authored::ArenaHeap::new(GAME_HEAP);
    install_allocator(g, &mut heap)?;
    let attrib = AttribHost::new(g, &mut heap, vault)?;
    let system = g.u32(crate::modules::SYSTEM)?;
    if system != 0 && g.u32(system + 0x24)? == 0 {
        let vtable = heap.alloc(g, 4 * HOST_SLOTS, 16)?;
        for slot in 0..HOST_SLOTS {
            g.set_u32(vtable + 4 * slot, DEVICE_FUNCS + 4 * slot)?;
        }
        let object = heap.alloc(g, 16, 16)?;
        g.set_u32(object, vtable)?;
        g.set_u32(system + 0x24, object)?;
    }
    let pcr = heap.alloc(g, PCR_BYTES, 16)?;
    g.fill(pcr, 0, PCR_BYTES)?;
    let thread = heap.alloc(g, THREAD_BYTES, 16)?;
    g.fill(thread, 0, THREAD_BYTES)?;
    g.set_u32(pcr + 0x100, thread)?;
    Ok(GameState { heap, kernel: Kernel::default(), attrib, files, pcr, camera_stamp: 0, world_only: false, paused: false, cutscene: false, ambience_duck: 1., traffic: 0, dmos: 0 })
}

/// The CRT's `_cinit` (`sub_82EE7520`, from the entry point `0x82EDFBB8`) runs the C initializers
/// `[0x82FA6F50, 0x82FA6F5C)`, then the C++ static constructors `[0x82FA0010, 0x82FA6F4C)` (7107
/// slots; 0 and -1 are skipped). They build the globals the rest of the title assumes (registries,
/// hash tables, singletons). Each runs on its own; one that stops is recorded and skipped.
const C_INITIALIZERS: std::ops::Range<u32> = 0x82FA_6F50..0x82FA_6F5C;
const CXX_CONSTRUCTORS: std::ops::Range<u32> = 0x82FA_0010..0x82FA_6F4C;

/// What the entry point runs before `_cinit`, in order, with its argument: the CRT's thread,
/// heap and I/O setup (without them `atexit` registrations in the constructors bug-check).
const CRT_STARTUP: [(u32, u64); 5] = [
    (0x82EE_7828, 0),
    (0x82EE_7678, 1),
    (0x82ED_F9D0, 0),
    (0x82F5_7DE0, 0),
    (0x82EE_7600, 0),
];

/// Run the CRT startup and every static initializer; returns `(function, why it stopped)` for
/// those that did.
pub fn initialize_statics(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
) -> Result<Vec<(u32, Error)>> {
    let mut stopped = Vec::new();
    for (function, arg) in CRT_STARTUP {
        if let (Err(e), _) = run(g, state, device, function, &[arg]) {
            stopped.push((function, e));
        }
    }
    for slot in C_INITIALIZERS.step_by(4).chain(CXX_CONSTRUCTORS.step_by(4)) {
        let function = g.u32(slot)?;
        if function == 0 || function == u32::MAX {
            continue;
        }
        if let (Err(e), _) = run(g, state, device, function, &[]) {
            stopped.push((function, e));
        }
    }
    Ok(stopped)
}

/// The AttribSys core init `sub_82B925F8(params)` the system boot `sub_826D35F8` runs (its
/// globals sit beside the database at `0x83084614`, e.g. the registry at `0x83084678` the audio
/// root's constructor uses). Params as `sub_826D35F8` builds them at `+0x1A0`: `{allocator =
/// sub_826DCDB0(), 0, 0x4B000, 0x100, 0, u8 1, u8 0}`.
const ATTRIB_CORE_INIT: u32 = 0x82B9_25F8;
const ATTRIB_ALLOCATOR: u32 = 0x826D_CDB0;

fn attrib_core(g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> (Result<u64>, CallTrace) {
    let (allocator, trace) = run(g, state, device, ATTRIB_ALLOCATOR, &[]);
    let allocator = match allocator {
        Ok(a) => a as u32,
        Err(e) => return (Err(e), trace),
    };
    let params = match state.heap.alloc(g, 0x20, 16) {
        Ok(p) => p,
        Err(e) => return (Err(e), trace),
    };
    let fill = |g: &mut Guest| -> Result<()> {
        g.set_u32(params, allocator)?;
        g.set_u32(params + 8, 0x4B000)?;
        g.set_u32(params + 0xC, 0x100)?;
        g.set_u8(params + 0x14, 1)
    };
    if let Err(e) = fill(g) {
        return (Err(e), trace);
    }
    run(g, state, device, ATTRIB_CORE_INIT, &[params as u64])
}

/// Retail's audio framework leaves four graphs on the bus root (`*(0x830CFDBC)`), `{player, module
/// table}` at `+0x10/+0x14` FF `Sub0 Scp0 Dac0`, `+0x18/+0x1C` and `+0x20/+0x24` C8 `Sub0 VuM0 Sen0`
/// (the VU meter buses), `+0x28/+0x2C` FE `Sub0 DCl0 Gai0 LI20 HI20 Sen0` (read from the running
/// game). The runtime's init builds them (`environment::build_root_graphs`; its master stands in
/// for FF).
fn root_outputs(g: &mut Guest) -> Result<()> {
    // The runtime's init built the framework's root graphs (`environment::build_root_graphs`).
    let root = g.u32(crate::classes::BUS_ROOT)?;
    match g.u32(root + 0x1C)? {
        0 => Err(Error::new(root, "the bus root's graphs are not built")),
        _ => Ok(()),
    }
}

/// Subsystems `main()` (`sub_826D35F8`) creates before the audio boot that the audio game side
/// reads, in main's order.
const PREREQUISITES: [(u32, &str); 0] = [];

/// Game singletons `main()` allocates and constructs before the audio boot that the audio game side
/// reads: `(global, bytes, constructor, what)`, each constructed as original code.
const SINGLETONS: [(u32, u32, u32, &str); 1] = [
    // The game's Attrib instance cache (`sub_826D35F8` at `826D4928`): `+0x18` the collision
    // contacts, `+0x84` the board audio collection, ...
    (0x830C_FDA4, 0x19C, 0x8289_D5C8, "attrib cache"),
];

/// `*(0x830CEB84)`: the game world (0x207A0 bytes, `sub_826DCBA0` → `sub_824777E8`, which builds
/// the whole simulation). The audio root init reads only its world box at `+0xEE20` (min, max as
/// two vec4s) and hands it to the trigger manager (`[root+0x1E8]` vfunc 8). Values read from the
/// running game (University): min (-900, -436.885, -1600, 0), max (1000, 581.881, 950, 1).
const GAME_WORLD: u32 = 0x830C_EB84;
const GAME_WORLD_BYTES: u32 = 0x2_07A0;
const WORLD_BOX: [f32; 8] = [-900.0, -436.885_13, -1600.0, 0.0, 1000.0, 581.880_8, 950.0, 1.0];

/// `*(0x830CFD94)`: the world root (painter world at `+0xE8`). The audio root init
/// (`sub_8248A5C8`) registers five message handlers in the dispatcher at `[+0xDC]` (a hash map at
/// `+0x30`, its lock at `+0x40`); empty zeroed objects take the registrations.
const WORLD_ROOT: u32 = 0x830C_FD94;
/// `[root+0xD4]`: a manager whose vfunc 39 (`+0x9C`) vetoes a voice start (`sub_824A75F0` at `0x824A7934`: a
/// true return clears the start). Unset, that read was a null load failing the whole frame — reached from
/// `sub_824BC188`/`sub_824BD358`'s body-contact sounds, so by any bailing skater's record. Every slot here is the
/// title's shared `li r3,0; blr` (`0x8274CA90`, in 7,835 vtable slots): no veto.
// ponytail: never vetoes; the retail manager's own rule if one is ever heard to matter.
const VOICE_VETO: u32 = 0xD4;
const RETURN_FALSE: u32 = 0x8274_CA90;
const VETO_SLOTS: u32 = 64;

/// `[0x83067060 + 0x1C]`: the game's settings object; the root init (`sub_82487A60`) selects the
/// SFX pack by its `+0x90` key (`sub_82486100`). Retail (read from the running game): "default"
/// (`0xD7EDBD362D7D2152`), the pack-off collection of class `0x11A631878B239355`.
pub(crate) const SETTINGS: u32 = 0x8306_707C;
const DEFAULT_SFX_PACK: u64 = 0xD7ED_BD36_2D7D_2152;
/// The settings object's audio words from `+0x80` as the running game holds them (free skate,
/// default options): `+0x80` / `+0x84` / `+0x88` the volume levels 1.0 (SFXObj_Master inputs
/// 2 / 3 / 1; the snapshot's profile had `+0x88` at 0, the capture's free skate at full), `+0x90` the SFX pack key, then
/// the option bytes.
const SETTINGS_WORDS: [u32; 20] = [
    0x3F80_0000, 0x3F80_0000, 0x3F80_0000, 0, 0xD7ED_BD36, 0x2D7D_2152, 0, 0x0100_0100, 0x0000_0101,
    0x0100_0009, 0x0000_0001, 0, 0, 0, 0, 0x0000_0001, 0x0000_0100, 0x0101_0101, 0x0000_0005,
    0x0000_0002,
];

/// `*(0x83083C38)`: the game object. The audio reads `+0x70` (the mode machine) and the player
/// records from `+0x2F070`; the engine writes those (see [`GAME_OBJECT_BYTES`]).
pub const GAME_OBJECT: u32 = 0x8308_3C38;
pub const GAME_OBJECT_BYTES: u32 = 0x4_0000;
/// Sub-objects of the game object the audio reads through a pointer, given zeroed blocks:
/// `(offset, bytes)`. `+0x2FCB4`: read by SFXObj_ObjectInstance's process (`sub_824ED840`,
/// `+0x30C0` flag, `+0x30C4` level).
const GAME_SUBOBJECTS: [(u32, u32); 1] = [(0x2_FCB4, 0x4000)];

fn game_object(g: &mut Guest, heap: &mut dyn Heap) -> Result<()> {
    if g.u32(GAME_OBJECT)? != 0 {
        return Ok(());
    }
    let object = heap.alloc(g, GAME_OBJECT_BYTES, 16)?;
    g.fill(object, 0, GAME_OBJECT_BYTES)?;
    let vtable = heap.alloc(g, 4 * HOST_SLOTS, 16)?;
    for slot in 0..HOST_SLOTS {
        g.set_u32(vtable + 4 * slot, MODE_FUNCS + 4 * slot)?;
    }
    let machine = heap.alloc(g, 16, 16)?;
    let mode = heap.alloc(g, 16, 16)?;
    g.set_u32(machine, vtable)?;
    g.set_u32(machine + 8, mode)?;
    g.set_u32(mode, vtable)?;
    g.set_u32(object + 0x70, machine)?;
    for (offset, bytes) in GAME_SUBOBJECTS {
        let sub = heap.alloc(g, bytes, 16)?;
        g.fill(sub, 0, bytes)?;
        g.set_u32(object + offset, sub)?;
    }
    g.set_u32(GAME_OBJECT, object)
}

/// A traffic car as the game thread's collector `sub_827A3790` hands it to the audio side: its 32-byte record
/// (`+0` accel, `+4` speed, `+8` distance from the skater, `+12` handle, `+16` flags: top nibble the status veh3416
/// & 0xF, bit 27 hard braking; `+24` its engine row in class 259095163B974174) and its world matrix's forward and
/// position rows, which SFXCTL_TrafficCarPhysics (`sub_824B2A28`) reads through the car object.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrafficCar {
    pub handle: u32,
    pub accel: f32,
    pub speed: f32,
    pub distance: f32,
    pub flags: u32,
    pub key: u64,
    pub forward: [f32; 3],
    pub position: [f32; 3],
}

/// The collector's cap (`sub_827A3790`: the spatial query's 10 within 40 m).
pub const TRAFFIC_CARS: usize = 10;
/// The frame's traffic list: `+48` the records, `+52` their count (CSTATEMGR_TrafficCar's sync `sub_824F2460`).
const TRAFFIC_LIST: u32 = 48;
/// `game + 0x22BC4`: the game thread's eastl::map<handle, car object> (anchor `+4`, root `+12`, size `+20`; node
/// `+0` right, `+4` left, `+8` parent, `+16` key, `+20` value) that `sub_824B2A28` finds a car in (`sub_82528840`).
const TRAFFIC_MAP: u32 = 0x2_2BC4;
/// The title's empty virtual (`blr`, in 7,195 vtable slots): as a car object's slot 10 (`+40`, its world matrix) it
/// hands back the object itself, which holds the matrix rows the audio reads (`+32` forward, `+48` position).
const RETURN_SELF: u32 = 0x82B6_1BB8;
/// The traffic block: the cars' shared vtable, then the records, the car objects (64 B) and the map nodes (32 B).
const TRAFFIC_RECORDS: u32 = 0x40;
const TRAFFIC_OBJECTS: u32 = TRAFFIC_RECORDS + 32 * TRAFFIC_CARS as u32;
const TRAFFIC_NODES: u32 = TRAFFIC_OBJECTS + 64 * TRAFFIC_CARS as u32;
const TRAFFIC_BYTES: u32 = TRAFFIC_NODES + 32 * TRAFFIC_CARS as u32;

/// This frame's traffic cars (nearest first; the first [`TRAFFIC_CARS`]), written where the game thread would: the
/// records into the frame's list and each car object into the map, in a block allocated once (`state.traffic`).
pub fn publish_traffic(g: &mut Guest, state: &mut GameState, cars: &[TrafficCar]) -> Result<()> {
    if state.traffic == 0 {
        if cars.is_empty() {
            return Ok(());
        }
        state.traffic = state.heap.alloc(g, TRAFFIC_BYTES, 16)?;
    }
    let game = g.u32(GAME_OBJECT)?;
    write_traffic(g, state.traffic, game, cars)
}

fn write_traffic(g: &mut Guest, block: u32, game: u32, cars: &[TrafficCar]) -> Result<()> {
    let cars = &cars[..cars.len().min(TRAFFIC_CARS)];
    for slot in 0..11 {
        g.set_u32(block + 4 * slot, RETURN_SELF)?;
    }
    let f = |g: &mut Guest, at: u32, v: f32| g.set_u32(at, v.to_bits());
    for (k, car) in cars.iter().enumerate() {
        let record = block + TRAFFIC_RECORDS + 32 * k as u32;
        f(g, record, car.accel)?;
        f(g, record + 4, car.speed)?;
        f(g, record + 8, car.distance)?;
        g.set_u32(record + 12, car.handle)?;
        g.set_u32(record + 16, car.flags)?;
        g.set_u32(record + 20, 0)?;
        g.set_u64(record + 24, car.key)?;
        let object = block + TRAFFIC_OBJECTS + 64 * k as u32;
        g.set_u32(object, block)?;
        for i in 0..3 {
            f(g, object + 32 + 4 * i as u32, car.forward[i])?;
            f(g, object + 48 + 4 * i as u32, car.position[i])?;
        }
        f(g, object + 44, 0.)?;
        f(g, object + 60, 1.)?;
    }
    let objects: Vec<(u32, u32)> =
        cars.iter().enumerate().map(|(k, car)| (car.handle, block + TRAFFIC_OBJECTS + 64 * k as u32)).collect();
    write_map(g, game + TRAFFIC_MAP, block + TRAFFIC_NODES, &objects)?;
    let frame = game + SKATER_FRAME;
    g.set_u32(frame + TRAFFIC_LIST, block + TRAFFIC_RECORDS)?;
    g.set_u32(frame + TRAFFIC_LIST + 4, cars.len() as u32)
}

/// One of the game thread's eastl::map<handle, object> (anchor `+4`, root `+12`, size `+20`; node `+0` right, `+4`
/// left, `+8` parent, `+16` key, `+20` value) as `sub_82528840` finds in it, from (handle, object) pairs: a chain down
/// the right in ascending handle order (the find walks right past every smaller key), its 32-byte nodes at `nodes`.
fn write_map(g: &mut Guest, map: u32, nodes: u32, objects: &[(u32, u32)]) -> Result<()> {
    let anchor = map + 4;
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by_key(|&k| objects[k].0);
    let node = |j: usize| nodes + 32 * j as u32;
    for (j, &k) in order.iter().enumerate() {
        let at = node(j);
        g.set_u32(at, if j + 1 < order.len() { node(j + 1) } else { 0 })?;
        g.set_u32(at + 4, 0)?;
        g.set_u32(at + 8, if j == 0 { anchor } else { node(j - 1) })?;
        g.set_u32(at + 12, 1)?;
        g.set_u32(at + 16, objects[k].0)?;
        g.set_u32(at + 20, objects[k].1)?;
    }
    let (first, last) = match order.len() {
        0 => (anchor, anchor),
        n => (node(0), node(n - 1)),
    };
    g.set_u32(anchor, last)?;
    g.set_u32(anchor + 4, first)?;
    g.set_u32(anchor + 8, if order.is_empty() { 0 } else { first })?;
    g.set_u32(anchor + 16, order.len() as u32)
}

/// A dynamic object (DMO) as the game thread hands it to the audio side (round2 bug 7): the 96-byte entry `sub_827A3FD8`
/// fills for each DMO its collector `sub_827A43C8` finds (RETAIL: at most [`DMOS`] within 30 m), which
/// CSTATEMGR_DynamicObject's sync `sub_824F4108` picks from (held entries first, then the largest impulse, then the
/// fastest; five at most) and SFXCTL_DynamicObjectPhysics `sub_824B5558` reads for SFXObj_Moving (c_sliding_start,
/// c_moveable_sliding_objects) - plus its position, which `sub_824B5558` reads off the DMO object when nobody holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DmoEntry {
    /// `+0` the DMO's handle (nonzero; unique in the list).
    pub id: u32,
    /// `+16` linear velocity (m/s, vfunc 120), `+48` angular velocity (rad/s, vfunc 124).
    pub velocity: [f32; 3],
    pub angular: [f32; 3],
    /// `+32` the local skater is grabbing this DMO (`sub_827A43C8`: player `[+72]+304` and its grabbed handle
    /// `[+28]+40`). The drag can only start for it (`sub_824B5558` sets `+116` only for a held entry).
    pub held: bool,
    /// `+40` its `livingworld_dynamicobject_characteristics` row (DMO `[[+4380]+0]+24`).
    pub row: u64,
    /// `sub_827A8AD8` over its contacts this frame: `+64` the largest impulse, `+68` any contact, `+72` the int of the
    /// impulse's slot (UNTRACED what it means; `sub_824B5558` copies it to the record's `+148` only).
    pub impulse: f32,
    pub contact: bool,
    pub material: u32,
    /// `+76` lying on its side (|up.y| < 0.3, `sub_827A3E58`), `+77` bit (player index) of DMO `+4392` (UNTRACED),
    /// `+78` DMO byte 4465 bit 4 (INFERRED "has wheels"), `+79` upright (up.y > 0.65).
    pub on_side: bool,
    pub touched: bool,
    pub wheeled: bool,
    pub upright: bool,
    /// `+80` 0 unless wheeled and upright and moving: then |velocity direction . a DMO axis| (the record's `+176`; the
    /// rolling path's, UNTRACED).
    pub roll_alignment: f32,
    /// `+84` DMO byte 4466 bit 6 (UNTRACED; the record's `+180`).
    pub flag_84: bool,
    /// The DMO's world position (not in the entry): its object's slot 14 row.
    pub position: [f32; 3],
}

/// The collector's cap (`sub_827A43C8`: the spatial query's 30 within 30 m).
pub const DMOS: usize = 30;
/// The frame's DMO list: `+216` the entries, `+220` their count (`sub_824F4108`, `sub_824B5558`).
const DMO_LIST: u32 = 216;
/// `game + 0x22EB4`: the map<handle, DMO> `sub_824B5558` finds an unheld DMO in (`0x824B56E0`; the other map, `+0x3A988`
/// with refcounted values, only when byte 0x830B7C2B or 0x83082929 is set: UNTRACED which modes, 0 on the game side).
const DMO_MAP: u32 = 0x2_2EB4;
/// A title getter (`addi r3,r3,16; blr`, in 10+ vtables): as a DMO object's slot 14 (`+56`; `sub_824B5558` loads 16
/// bytes from what it returns) it hands back the object's `+16`, where its position row sits.
const RETURN_PLUS_16: u32 = 0x825A_2118;
/// The DMO block: the objects' shared vtable, then the entries (96 B), the objects (32 B) and the map nodes (32 B).
const DMO_ENTRIES: u32 = 0x40;
const DMO_OBJECTS: u32 = DMO_ENTRIES + 96 * DMOS as u32;
const DMO_NODES: u32 = DMO_OBJECTS + 32 * DMOS as u32;
const DMO_BYTES: u32 = DMO_NODES + 32 * DMOS as u32;

/// This frame's DMOs (the first [`DMOS`]), written where the game thread would: the entries into the frame's list and
/// each DMO object into the map, in a block allocated once (`state.dmos`).
pub fn publish_dmos(g: &mut Guest, state: &mut GameState, dmos: &[DmoEntry]) -> Result<()> {
    if state.dmos == 0 {
        if dmos.is_empty() {
            return Ok(());
        }
        state.dmos = state.heap.alloc(g, DMO_BYTES, 16)?;
    }
    let game = g.u32(GAME_OBJECT)?;
    write_dmos(g, state.dmos, game, dmos)
}

fn write_dmos(g: &mut Guest, block: u32, game: u32, dmos: &[DmoEntry]) -> Result<()> {
    let dmos = &dmos[..dmos.len().min(DMOS)];
    for slot in 0..16 {
        g.set_u32(block + 4 * slot, RETURN_PLUS_16)?;
    }
    let f = |g: &mut Guest, at: u32, v: f32| g.set_u32(at, v.to_bits());
    for (k, dmo) in dmos.iter().enumerate() {
        let entry = block + DMO_ENTRIES + 96 * k as u32;
        g.fill(entry, 0, 96)?;
        g.set_u32(entry, dmo.id)?;
        for i in 0..3 {
            f(g, entry + 16 + 4 * i as u32, dmo.velocity[i])?;
            f(g, entry + 48 + 4 * i as u32, dmo.angular[i])?;
        }
        g.set_u8(entry + 32, dmo.held as u8)?;
        g.set_u64(entry + 40, dmo.row)?;
        f(g, entry + 64, dmo.impulse)?;
        g.set_u8(entry + 68, dmo.contact as u8)?;
        g.set_u32(entry + 72, dmo.material)?;
        for (at, flag) in [(76, dmo.on_side), (77, dmo.touched), (78, dmo.wheeled), (79, dmo.upright), (84, dmo.flag_84)] {
            g.set_u8(entry + at, flag as u8)?;
        }
        f(g, entry + 80, dmo.roll_alignment)?;
        let object = block + DMO_OBJECTS + 32 * k as u32;
        g.set_u32(object, block)?;
        for i in 0..3 {
            f(g, object + 16 + 4 * i as u32, dmo.position[i])?;
        }
        f(g, object + 28, 1.)?;
    }
    let objects: Vec<(u32, u32)> =
        dmos.iter().enumerate().map(|(k, dmo)| (dmo.id, block + DMO_OBJECTS + 32 * k as u32)).collect();
    write_map(g, game + DMO_MAP, block + DMO_NODES, &objects)?;
    let frame = game + SKATER_FRAME;
    g.set_u32(frame + DMO_LIST, block + DMO_ENTRIES)?;
    g.set_u32(frame + DMO_LIST + 4, dmos.len() as u32)
}

/// `[0x830670B8 + 0x14]`: the game's flow state; 5 is gameplay, the only state in which the audio
/// side takes skaters from the frame (`sub_824F1FB0`) and swaps SFX packs (`sub_82486570`).
const GAME_FLOW: u32 = 0x8306_70CC;
pub const GAMEPLAY: u32 = 5;

fn game_world(g: &mut Guest, heap: &mut dyn Heap) -> Result<()> {
    game_object(g, heap)?;
    g.set_u32(GAME_FLOW, GAMEPLAY)?;
    if g.u32(SETTINGS)? == 0 {
        let settings = heap.alloc(g, 0x400, 16)?;
        g.fill(settings, 0, 0x400)?;
        for (i, w) in SETTINGS_WORDS.iter().enumerate() {
            g.set_u32(settings + 0x80 + 4 * i as u32, *w)?;
        }
        g.set_u64(settings + 0x90, DEFAULT_SFX_PACK)?;
        g.set_u32(SETTINGS, settings)?;
    }
    if g.u32(WORLD_ROOT)? == 0 {
        let root = heap.alloc(g, 0x400, 16)?;
        let dispatcher = heap.alloc(g, 0x400, 16)?;
        g.set_u32(root + 0xDC, dispatcher)?;
        g.set_u32(WORLD_ROOT, root)?;
    }
    let root = g.u32(WORLD_ROOT)?;
    if g.u32(root + VOICE_VETO)? == 0 {
        let veto = heap.alloc(g, 16, 16)?;
        let vtable = heap.alloc(g, 4 * VETO_SLOTS, 16)?;
        for slot in 0..VETO_SLOTS {
            g.set_u32(vtable + 4 * slot, RETURN_FALSE)?;
        }
        g.set_u32(veto, vtable)?;
        g.set_u32(root + VOICE_VETO, veto)?;
    }
    if g.u32(GAME_WORLD)? != 0 {
        return Ok(());
    }
    let world = heap.alloc(g, GAME_WORLD_BYTES, 16)?;
    for (i, v) in WORLD_BOX.iter().enumerate() {
        g.set_u32(world + 0xEE20 + 4 * i as u32, v.to_bits())?;
    }
    g.set_u32(GAME_WORLD, world)
}

const SPLICE_BYTES: u32 = 0x1098;
const SPLICE_CONSTRUCT: u32 = 0x8297_4BD0;
const SPLICE_INIT: u32 = 0x8297_4D28;

fn splice_system(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    traces: &mut Vec<CallTrace>,
) -> Result<u32> {
    let root = g.u32(crate::classes::BUS_ROOT)?;
    if let Ok(at) = g.u32(root + 0x54) {
        if at != 0 {
            return Ok(at);
        }
    }
    let object = state.heap.alloc(g, SPLICE_BYTES, 16)?;
    for function in [SPLICE_CONSTRUCT, SPLICE_INIT] {
        let (done, trace) = run(g, state, device, function, &[object as u64]);
        traces.push(trace);
        done?;
        g.set_u32(root + 0x54, object)?;
    }
    Ok(object)
}

/// Run the title's audio boot, then the root's init. Returns the root (or where it stopped) and
/// each step's trace.
pub fn boot(g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> (Result<u32>, Vec<CallTrace>) {
    let mut traces = Vec::new();
    if let Err(e) = root_outputs(g).and_then(|_| game_world(g, &mut state.heap)) {
        return (Err(e), traces);
    }
    let (core, trace) = attrib_core(g, state, device);
    traces.push(trace);
    if let Err(e) = core {
        return (Err(Error::new(e.address, format!("attrib core init: {}", e.message))), traces);
    }
    for (global, bytes, constructor, what) in SINGLETONS {
        let built = (|| -> Result<()> {
            if g.u32(global)? != 0 {
                return Ok(());
            }
            let object = state.heap.alloc(g, bytes, 16)?;
            g.set_u32(global, object)?;
            let (done, trace) = run(g, state, device, constructor, &[object as u64]);
            traces.push(trace);
            done.map(|_| ())
        })();
        if let Err(e) = built {
            return (Err(Error::new(e.address, format!("{what}: {}", e.message))), traces);
        }
    }
    for (function, what) in PREREQUISITES {
        let (done, trace) = run(g, state, device, function, &[]);
        traces.push(trace);
        if let Err(e) = done {
            return (Err(Error::new(e.address, format!("{what}: {}", e.message))), traces);
        }
    }
    // The audio framework's Splice (one-shot bank) system, `[bus root + 0x54]`, as `sub_826D4C30`
    // creates it: 0x1098 bytes, `sub_82974BD0` (constructor), then `sub_82974D28` (init).
    let splice = match splice_system(g, state, device, &mut traces) {
        Ok(at) => at,
        Err(e) => return (Err(Error::new(e.address, format!("splice system: {}", e.message))), traces),
    };
    let _ = splice;
    let (booted, trace) = run(g, state, device, AUDIO_BOOT, &[]);
    traces.push(trace);
    if let Err(e) = booted {
        return (Err(Error::new(e.address, format!("audio boot: {}", e.message))), traces);
    }
    let root = match g.u32(ROOT_GLOBAL) {
        Ok(root) => root,
        Err(e) => return (Err(e), traces),
    };
    let (init, trace) = run(g, state, device, ROOT_INIT, &[root as u64]);
    traces.push(trace);
    if let Err(e) = init {
        return (Err(Error::new(e.address, format!("root init: {}", e.message))), traces);
    }
    // The level load: the resource manager (`*(0x830CFDCC)`, `sub_828DC0A8` per frame) drains
    // its queued loads (the components' banks, grain files, streams) before gameplay starts, as
    // the title's loading screen waits for them.
    if let Err(e) = drain_loads(g, state, device).and_then(|_| configure_buses(g, state, device)) {
        return (Err(Error::new(e.address, format!("level load: {}", e.message))), traces);
    }
    (Ok(root), traces)
}

const RESOURCE_MANAGER: u32 = 0x830C_FDCC;
const RESOURCE_UPDATE: u32 = 0x828D_C0A8;
/// `rm + 0x2EE24`: queued requests; `rm + 0x2EE4C`: the one in flight (state, 0 idle).
const RM_QUEUED: u32 = 0x2_EE24;
const RM_IN_FLIGHT: u32 = 0x2_EE4C;
/// `rm + 0x2EE34`: the release list's count (`sub_828DC8A0` frees each entry once its bank has left the sound engine).
const RM_RELEASING: u32 = 0x2_EE34;

/// One resource manager update while it has releases waiting; true while some still wait. A bank leaves the
/// sound engine on the audio thread (`sub_82B1DCD0` queues the command `sub_82B1DC00`), so the caller renders a
/// block between steps, as the title's game and audio threads interleave.
pub fn release_step(g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> Result<bool> {
    let rm = g.u32(RESOURCE_MANAGER)?;
    if g.u32(rm + RM_RELEASING)? == 0 {
        return Ok(false);
    }
    run(g, state, device, RESOURCE_UPDATE, &[rm as u64]).0?;
    Ok(g.u32(rm + RM_RELEASING)? != 0)
}

/// Run the resource manager until nothing is queued or in flight.
pub fn drain_loads(g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> Result<()> {
    let rm = g.u32(RESOURCE_MANAGER)?;
    for _ in 0..10_000 {
        if g.u32(rm + RM_QUEUED)? == 0 && g.u32(rm + RM_IN_FLIGHT)? == 0 {
            return Ok(());
        }
        let (done, _) = run(g, state, device, RESOURCE_UPDATE, &[rm as u64]);
        done?;
    }
    Err(Error::new(rm, "resource loads never drained"))
}

/// `sub_82485190(root, dt, both)`: the audio game side's per-frame update (virtual; the title calls
/// it about every 7.4 ms). With `both` set, or `dt` above 0.02 s, one call runs both halves (the
/// components' process, the MixMap tick, their update) with the call's own `dt`.
pub const FRAME: u32 = 0x8248_5190;

/// SFXObj_Pause's inputs ([`crate::mixmap::inputs::pause_inputs`]) on manager `manager`'s MixMap,
/// written every tick, paused or not, as retail's process does.
fn pause(g: &mut Guest, manager: u32, paused: bool, cutscene: bool) -> Result<()> {
    let x = g.u32(manager + 8)?;
    if x == 0 {
        return Ok(());
    }
    let Some(ctrl) = crate::mixmap::find_controller(g, g.u32(x)?, 0x4000_0070)? else { return Ok(()) };
    // A cutscene holds play, not a menu: id 2 (the request's route outside free skate) ducks the
    // player controllers alone, where id 0 takes the world's ambience bed to silence with them
    // (measured: skate-game's `headless_skate2_cutscene_keeps_the_ambience`).
    let inputs = if paused && cutscene {
        [(0, 0), (1, 0), (2, 32767)]
    } else {
        crate::mixmap::inputs::pause_inputs(paused)
    };
    for (id, value) in inputs {
        crate::mixmap::controller::set(g, ctrl, id, value)?;
    }
    Ok(())
}

/// The weather's duck on manager `manager`'s Ambience controller ([`GameState::ambience_duck`]).
fn duck_ambience(g: &mut Guest, manager: u32, scale: f32) -> Result<()> {
    let x = g.u32(manager + 8)?;
    if x == 0 || scale >= 1. {
        return Ok(());
    }
    match crate::mixmap::find_controller(g, g.u32(x)?, crate::mixmap::key(2, 2, 0, 0))? {
        Some(ctrl) => crate::mixmap::controller::scale_gain(g, ctrl, 0, scale),
        None => Ok(()),
    }
}

pub fn frame(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    dt: f32,
) -> (Result<u64>, CallTrace) {
    let root = match g.u32(ROOT_GLOBAL) {
        Ok(root) => root,
        Err(e) => return (Err(e), CallTrace::default()),
    };
    state.camera_stamp = state.camera_stamp.wrapping_add(1);
    let result = run_with_floats(g, state, device, FRAME, &[root as u64, 0, 1], &[dt as f64]);
    if result.0.is_ok() {
        if let Err(e) = configure_buses(g, state, device) {
            return (Err(e), result.1);
        }
    }
    result
}

/// `sub_824916E8(manager, bus)`: stamp an effect bus's authored settings (vault values, with its
/// random interpolation) the first time a voice asks for it, or again after the title cleared its
/// flag (`manager + 0x47C + bus`). The runtime's voice op asks from inside the audio block, where
/// the title's Attrib is out of reach, so every bus the title has left unconfigured is configured
/// here, after each frame, as original code.
const CONFIGURE_BUS: u32 = 0x8249_16E8;
const BUS_FLAGS: u32 = 0x47C;

pub fn configure_buses(g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> Result<()> {
    let manager = g.u32(crate::device::BUS_MANAGER)?;
    for bus in 0..crate::device::BUS_GRAPH_COUNT {
        if g.u8(manager + BUS_FLAGS + bus)? == 0 {
            let (done, _) = run(g, state, device, CONFIGURE_BUS, &[manager as u64, bus as u64]);
            done?;
        }
    }
    Ok(())
}

/// The PhysOut bundle's slot sizes (`sub_82DE53F0` reset, templates `sub_82DE4940`), by slot
/// index (`B + 4·i`): 0 deck transforms, 1 Motion, 2 Air, 3, 4 Grinds, 5 Skeleton, 6 Collision,
/// 7 State, 8 Ground, 9 SystemReckoning, 10 the audio conditioner's output, 11, 12, 13
/// Interaction, 14 Animation, 15 PhysOutScoring2, 16 Filtered, 17, 18 OffBoard.
pub const PHYSOUT_SLOTS: [u32; 19] = [
    288, 288, 464, 48, 336, 608, 3488, 88, 336, 176, 224, 112, 128, 80, 176, 14672, 88, 140, 336,
];

/// The game object's per-skater audio frame (`+0x2F070`): `+0xC` skater count, records of
/// `0x220` bytes from `+0xF0` (what the audio side's player manager `sub_824F1FB0` and the bridge
/// `sub_824B0DA8` read).
pub const SKATER_FRAME: u32 = 0x2_F070;
pub const SKATER_RECORDS: u32 = 0xF0;
pub const SKATER_RECORD_BYTES: u32 = 0x220;
/// `sub_827A1B78(builder, player, record, local)`: the game thread's per-skater record builder,
/// reading the player's PhysOut bundle (`[[player+0x710]]` vfunc 23) and two Attrib instances on
/// the builder (`+0x10` class `0xC26949FCB638A2CA`, `+0x20` class `0xB29C3B2C13D96482`, both
/// "default").
const BUILD_RECORD: u32 = 0x827A_1B78;
const FIND_COLLECTION: u32 = 0x82B6_9B08;
const INSTANCE: u32 = 0x82B6_CC78;
const DEFAULT_KEY: u64 = 0xD7ED_BD36_2D7D_2152;
/// The player object's virtuals the builder calls: slot 11 (`+0x2C`) its controller index, and
/// its `+0x710` holder's slot 23 (`+0x5C`) the PhysOut bundle (kept at holder `+8`).
const PLAYER_FUNCS: u32 = 0x6BF6_0000;
const PLAYER_HOOKS: std::ops::Range<u32> = PLAYER_FUNCS..PLAYER_FUNCS + 4 * 32;

/// The local skater as the audio game side sees it: the player object, its PhysOut bundle and the
/// record builder.
pub struct Skater {
    pub player: u32,
    pub bundle: u32,
    pub slots: [u32; 19],
    pub builder: u32,
}

impl Skater {
    pub fn new(g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> Result<Self> {
        let heap = &mut state.heap;
        let vtable = heap.alloc(g, 4 * 32, 16)?;
        for slot in 0..32 {
            g.set_u32(vtable + 4 * slot, PLAYER_FUNCS + 4 * slot)?;
        }
        let bundle = heap.alloc(g, 4 * 19, 16)?;
        let mut slots = [0; 19];
        for (i, &bytes) in PHYSOUT_SLOTS.iter().enumerate() {
            slots[i] = heap.alloc(g, bytes, 16)?;
            g.fill(slots[i], 0, bytes)?;
            g.set_u32(bundle + 4 * i as u32, slots[i])?;
        }
        let holder = heap.alloc(g, 16, 16)?;
        g.set_u32(holder, vtable)?;
        g.set_u32(holder + 8, bundle)?;
        let player = heap.alloc(g, 0x800, 16)?;
        g.fill(player, 0, 0x800)?;
        g.set_u32(player, vtable)?;
        g.set_u32(player + 0x710, holder)?;
        let builder = state.heap.alloc(g, 0x100, 16)?;
        g.fill(builder, 0, 0x100)?;
        for (offset, class) in [(0x10u32, 0xC269_49FC_B638_A2CAu64), (0x20, 0xB29C_3B2C_13D9_6482)] {
            let (collection, _) = run(g, state, device, FIND_COLLECTION, &[class, DEFAULT_KEY]);
            let (done, _) = run(g, state, device, INSTANCE, &[(builder + offset) as u64, collection? & 0xFFFF_FFFF]);
            done?;
        }
        Ok(Self { player, bundle, slots, builder })
    }

    /// Build the skater's record from the bundle (as the game thread does) into the game object's
    /// audio frame as its only skater.
    pub fn publish(&self, g: &mut Guest, state: &mut GameState, device: &mut dyn Heap) -> Result<()> {
        let game = g.u32(GAME_OBJECT)?;
        let frame = game + SKATER_FRAME;
        let record = frame + SKATER_RECORDS;
        g.fill(record, 0, SKATER_RECORD_BYTES)?;
        let (done, _) = run(g, state, device, BUILD_RECORD, &[self.builder as u64, self.player as u64, record as u64, 1]);
        done?;
        // `+8` the local skater's record (the listener follows its `+0` position, `+0x10` facing,
        // `+0x20` velocity: `sub_8248CC08`), `+0xC` the count.
        g.set_u32(frame + 8, record)?;
        g.set_u32(frame + 0xC, 1)?;
        // `+0` the game's time scale (1.0 outside slow motion; SFXObj_Master's input 4 follows it).
        g.set_u32(frame, 1.0f32.to_bits())
    }
}

/// SFXObj_Ambience (`docs/audio-object-registry.md`, category 2 kind 0), run as original code
/// without the rest of the game side (the hand-ported player audio keeps its own components):
/// factory `sub_824D3010` (a `0x80`-byte object, vtable `0x822FC290`), then slot 7 create
/// (`sub_824D3550`: enables the MixMap controller at `[this+0xC]`, `root+0x2D8 = this`), slot 9
/// process (`sub_824D3C28`: picks the bed from the painter key and steps the fade state machine,
/// writing the controller's input 0) and slot 10 update (`sub_824D42C8`: reads the controller's
/// outputs into the bed graph's gains). It plays the bed as a 6-channel SndPlayer1 graph
/// (`SnP1 Rch0 Rsp0 Gai0 LI20 Sen0`, `sub_824D3590`), reading the bed's `.snr` header out of the
/// resident archive its constructor loads whole.
const AMBIENCE_FACTORY: u32 = 0x824D_3010;
const AMBIENCE_CREATE: u32 = 0x824D_3550;
const AMBIENCE_PROCESS: u32 = 0x824D_3C28;
const AMBIENCE_UPDATE: u32 = 0x824D_42C8;
/// The frame record's `audio_ambience` painter key (`sub_827A2E88`; `sub_824D3E40` reads the
/// record's `+8`, i.e. game object `+0x2F0B0 + 8`).
pub const AMBIENCE_KEY: u32 = SKATER_FRAME + 0x48;

/// Boot what SFXObj_Ambience needs (the audio boot's objects and the title's singletons, no
/// managers, states, other components or MixMap) and create the component with the MixMap
/// controller `controller` (key `mixmap::key(2, 2, 0, 0)`) as its owner. Returns the component.
pub fn boot_ambience(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    controller: u32,
) -> (Result<u32>, Vec<CallTrace>) {
    let mut traces = Vec::new();
    if let Err(e) = root_outputs(g).and_then(|_| game_world(g, &mut state.heap)) {
        return (Err(e), traces);
    }
    let (core, trace) = attrib_core(g, state, device);
    traces.push(trace);
    if let Err(e) = core {
        return (Err(Error::new(e.address, format!("attrib core init: {}", e.message))), traces);
    }
    for (global, bytes, constructor, what) in SINGLETONS {
        let built = (|| -> Result<()> {
            if g.u32(global)? != 0 {
                return Ok(());
            }
            let object = state.heap.alloc(g, bytes, 16)?;
            g.set_u32(global, object)?;
            let (done, trace) = run(g, state, device, constructor, &[object as u64]);
            traces.push(trace);
            done.map(|_| ())
        })();
        if let Err(e) = built {
            return (Err(Error::new(e.address, format!("{what}: {}", e.message))), traces);
        }
    }
    let (booted, trace) = run(g, state, device, AUDIO_BOOT, &[]);
    traces.push(trace);
    if let Err(e) = booted {
        return (Err(Error::new(e.address, format!("audio boot: {}", e.message))), traces);
    }
    let created = create_component(g, state, device, AMBIENCE_FACTORY, AMBIENCE_CREATE, controller, &mut traces);
    (created.map_err(|e| Error::new(e.address, format!("ambience: {}", e.message))), traces)
}

/// A component as `sub_8248CE88`'s registry makes it: `factory(0)` allocates and constructs it,
/// `+0xC` takes its MixMap controller, then vtable slot 7 `create(this)`. Returns the component.
pub fn create_component(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    factory: u32,
    create: u32,
    controller: u32,
    traces: &mut Vec<CallTrace>,
) -> Result<u32> {
    let (made, trace) = run(g, state, device, factory, &[0]);
    traces.push(trace);
    let component = match made? as u32 {
        0 => return Err(Error::new(factory, "the factory made no component")),
        c => c,
    };
    g.set_u32(component + 0xC, controller)?;
    let (done, trace) = run(g, state, device, create, &[component as u64]);
    traces.push(trace);
    done.map(|_| component)
}

/// SFXObj_Master (category 0 kind 2, desc `0x8302D190`, vtable `0x822FC170`), controller
/// `0x40000020`: its process (slot 9) writes the Master inputs from the settings volumes and the
/// frame's time scale; its update (slot 10) sets the framework master section FE from the outputs
/// and the Attrib master collection (`[[0x830CFDA4]+0x68]`): `sub_828DE300` / `sub_828DE3E8` the
/// two output levels, `sub_828DE130` / `sub_828DE218` fields `B0178C76089D0448` / `42481BE2E5DEA74A`
/// (`sub_828DE130` posts to FE module 1, the `DCl0` clipper).
pub const MASTER_FACTORY: u32 = 0x824D_4F60;
pub const MASTER_CREATE: u32 = 0x824D_5120;
pub const MASTER_PROCESS: u32 = 0x824D_5160;
pub const MASTER_UPDATE: u32 = 0x824D_5868;

/// A component's process (vtable slot 9, `(this, f1 = dt)`), before the MixMap evaluation. The
/// frame record's time scale (`+0x2F070`, 1.0 outside slow motion; SFXObj_Master's input 4)
/// is kept at 1.0: the engine has no slow motion.
pub fn component_process(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    process: u32,
    component: u32,
    dt: f32,
) -> (Result<u64>, CallTrace) {
    let put = g
        .u32(GAME_OBJECT)
        .and_then(|game| g.set_u32(game + SKATER_FRAME, 1.0f32.to_bits()));
    if let Err(e) = put {
        return (Err(e), CallTrace::default());
    }
    run_with_floats(g, state, device, process, &[component as u64], &[dt as f64])
}

/// SFXObj_Ambience's process for one game frame of `dt` seconds with the painter key `key` at the
/// player (0 = unpainted: the title then uses its default ambience collection). Run before the
/// MixMap evaluation.
pub fn ambience_process(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    component: u32,
    key: u64,
    dt: f32,
) -> (Result<u64>, CallTrace) {
    let put = g
        .u32(GAME_OBJECT)
        .and_then(|game| g.set_u64(game + AMBIENCE_KEY, key));
    if let Err(e) = put {
        return (Err(e), CallTrace::default());
    }
    run_with_floats(g, state, device, AMBIENCE_PROCESS, &[component as u64], &[dt as f64])
}

/// SFXObj_Ambience's update, after the MixMap evaluation.
pub fn ambience_update(
    g: &mut Guest,
    state: &mut GameState,
    device: &mut dyn Heap,
    component: u32,
) -> (Result<u64>, CallTrace) {
    run(g, state, device, AMBIENCE_UPDATE, &[component as u64])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sub_82528840` as SFXCTL_TrafficCarPhysics runs it on the map: root `+12`, end = the anchor `+4`.
    fn find(g: &Guest, map: u32, key: u32) -> Option<u32> {
        let end = map + 4;
        let (mut node, mut best) = (g.u32(map + 12).unwrap(), end);
        while node != 0 {
            if g.u32(node + 16).unwrap() < key {
                node = g.u32(node).unwrap();
            } else {
                best = node;
                node = g.u32(node + 4).unwrap();
            }
        }
        (best != end && key >= g.u32(best + 16).unwrap()).then(|| g.u32(best + 20).unwrap())
    }

    /// `sub_824A75F0` (0x824A7934) loads `[[0x830CFD94]+0xD4]` and calls its `+0x9C` on a voice start; a zeroed root
    /// made that a null load, failing every frame with a remote skater's body contact in it (section 3.1 fuzz).
    #[test]
    fn the_world_root_has_a_voice_veto_that_never_vetoes() {
        let mut g = Guest::single(0x8300_0000, 0x40_0000);
        let mut heap = crate::patch::BumpHeap { next: 0x8310_0000, end: 0x8340_0000 };
        game_world(&mut g, &mut heap).unwrap();
        let veto = g.u32(g.u32(WORLD_ROOT).unwrap() + 0xD4).unwrap();
        assert_ne!(veto, 0);
        assert_eq!(g.u32(g.u32(veto).unwrap() + 0x9C).unwrap(), 0x8274_CA90);
        // Booting again keeps the same one.
        game_world(&mut g, &mut heap).unwrap();
        assert_eq!(g.u32(g.u32(WORLD_ROOT).unwrap() + 0xD4).unwrap(), veto);
    }

    #[test]
    fn the_cars_go_where_the_collector_and_the_map_put_them() {
        let (game, block) = (0x4000_0000, 0x4003_8000);
        let mut g = Guest::single(game, 0x4_0000);
        let car = |handle, x| TrafficCar {
            handle,
            speed: 5.,
            distance: x,
            flags: 6 << 28 | 1 << 27,
            key: 0xAD9F_4E17_8868_DAD6,
            forward: [0., 0., 1.],
            position: [x, 0., 2.],
            ..Default::default()
        };
        let cars = [car(7, 3.), car(2, 9.), car(30, 12.)];
        write_traffic(&mut g, block, game, &cars).unwrap();
        let frame = game + SKATER_FRAME;
        let records = g.u32(frame + 48).unwrap();
        assert_eq!(g.u32(frame + 52).unwrap(), 3);
        // The second record as sub_824F2460 / sub_824B2A28 read it: speed, distance, handle, flags, engine row.
        assert_eq!(g.f32(records + 32 + 4).unwrap(), 5.);
        assert_eq!(g.f32(records + 32 + 8).unwrap(), 9.);
        assert_eq!(g.u32(records + 32 + 12).unwrap(), 2);
        assert_eq!(g.u32(records + 32 + 16).unwrap(), 0x6800_0000);
        assert_eq!(g.u64(records + 32 + 24).unwrap(), 0xAD9F_4E17_8868_DAD6);
        // Each handle finds its car; its slot 10 hands back the object, whose +32 / +48 rows are forward and position.
        for c in &cars {
            let object = find(&g, game + 0x2_2BC4, c.handle).unwrap();
            assert_eq!(g.u32(g.u32(object).unwrap() + 40).unwrap(), 0x82B6_1BB8);
            assert_eq!(g.f32(object + 40).unwrap(), 1.);
            assert_eq!(g.f32(object + 48).unwrap(), c.position[0]);
        }
        assert_eq!(find(&g, game + 0x2_2BC4, 5), None);
        // A car gone: the next frame's list and map no longer hold it; none left, an empty map.
        write_traffic(&mut g, block, game, &cars[..1]).unwrap();
        assert_eq!(g.u32(frame + 52).unwrap(), 1);
        assert_eq!(find(&g, game + 0x2_2BC4, 2), None);
        assert!(find(&g, game + 0x2_2BC4, 7).is_some());
        write_traffic(&mut g, block, game, &[]).unwrap();
        assert_eq!(g.u32(frame + 52).unwrap(), 0);
        assert_eq!(find(&g, game + 0x2_2BC4, 7), None);
    }

    /// Round2 bug 7: a DMO entry lands where the game thread's `sub_827A3FD8` writes it and SFXCTL_DynamicObjectPhysics
    /// (`sub_824B5558`) reads it (RETAIL: `+0` id, `+16` velocity, `+32` held, `+40` characteristics row, `+48` angular
    /// velocity, `+68` contact, `+76` on its side, `+79` upright); the frame's `+216` / `+220` list the entries
    /// (CSTATEMGR_DynamicObject's sync `sub_824F4108`); each id finds its object in the map at game `+0x22EB4`, whose slot
    /// 14 (`sub_824B5558` reads the DMO's position through it when nobody holds it) is `addi r3,r3,16; blr` over the
    /// position row at `+16`.
    #[test]
    fn the_dmos_go_where_the_collector_and_the_map_put_them() {
        let (game, block) = (0x4000_0000, 0x4003_8000);
        let mut g = Guest::single(game, 0x4_0000);
        let wedge = DmoEntry {
            id: 1,
            velocity: [2., 0., 0.],
            angular: [0., 0.5, 0.],
            held: true,
            row: 0x733F_4D47_5186_E147,
            contact: true,
            upright: true,
            position: [3., 0.5, -2.],
            ..Default::default()
        };
        let drum = DmoEntry { id: 9, contact: true, on_side: true, position: [-4., 0.4, 1.], ..Default::default() };
        write_dmos(&mut g, block, game, &[wedge, drum]).unwrap();
        let frame = game + SKATER_FRAME;
        let list = g.u32(frame + 216).unwrap();
        assert_eq!(g.u32(frame + 220).unwrap(), 2);
        assert_eq!(g.u32(list).unwrap(), 1);
        assert_eq!(g.f32(list + 16).unwrap(), 2.);
        assert_eq!(g.u8(list + 32).unwrap(), 1);
        assert_eq!(g.u64(list + 40).unwrap(), 0x733F_4D47_5186_E147);
        assert_eq!(g.f32(list + 52).unwrap(), 0.5);
        assert_eq!(g.u8(list + 68).unwrap(), 1);
        assert_eq!((g.u8(list + 76).unwrap(), g.u8(list + 79).unwrap()), (0, 1));
        // The second entry, 96 bytes on: nobody holds it, it lies on its side.
        assert_eq!(g.u32(list + 96).unwrap(), 9);
        assert_eq!((g.u8(list + 96 + 32).unwrap(), g.u8(list + 96 + 76).unwrap()), (0, 1));
        for dmo in [wedge, drum] {
            let object = find(&g, game + 0x2_2EB4, dmo.id).unwrap();
            assert_eq!(g.u32(g.u32(object).unwrap() + 56).unwrap(), 0x825A_2118);
            assert_eq!(g.f32(object + 16).unwrap(), dmo.position[0]);
            assert_eq!(g.f32(object + 24).unwrap(), dmo.position[2]);
            assert_eq!(g.f32(object + 28).unwrap(), 1.);
        }
        assert_eq!(find(&g, game + 0x2_2EB4, 5), None);
        // The prop gone: an empty list and map.
        write_dmos(&mut g, block, game, &[]).unwrap();
        assert_eq!(g.u32(frame + 220).unwrap(), 0);
        assert_eq!(find(&g, game + 0x2_2EB4, 1), None);
    }
}
