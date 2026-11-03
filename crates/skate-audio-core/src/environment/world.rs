//! SFXObj_Reverb, the title's reverb controller, run as original code.
//!
//! Its init (`sub_824DDE50`, vfunc 7) loads the default preset into both network branches and the
//! final outputs' wobble presets (`sub_824DDF58`); its update (`sub_824DE548`, vfunc 9, once per
//! game frame) picks a preset from the active reverb zones or the outdoor painter key, requests it
//! into the idle branch (`sub_824DE468` → `sub_824DE140`) and crossfades the C7 sends: timed
//! outdoors (`sub_824DE850`), by the zone's weight inside one (`sub_824DE970` → `sub_824DEEF0`,
//! which also pans the zone's reverb returns toward it), and between two (`sub_824DEAE8`).
//!
//! Its process also writes the MixMap Reverb controller's seven inputs (`sub_824DF468`, a one-hot
//! of the requested preset's category), and its update (`sub_824DF220`, vfunc 10, after the MixMap
//! evaluation) reads that controller's outputs 0..4 into the network (C8 sends, the outputs' send
//! to C7 and gain). The object's `+0xC` is the runtime's own MixMap controller, whose retail
//! vtable the interpreter follows.
//!
//! What the interpreter cannot run is hosted natively here, from exported data:
//! * the Attrib calls (class lookup and iteration, instances, the zone volume's `RefSpec`) over
//!   the exported collections laid out in guest memory ([`WorldClass`]);
//! * the trigger system's active-zone list (`sub_82488278`): one entry per zone, stable while it
//!   is active, carrying the weight the caller measured ([`ReverbFrame::zones`]).

use std::collections::HashMap;

use super::{HOOKS, OUTER_SP, Ports, run_with};
use crate::mathlib::Trig;
use crate::patch::Heap;
use crate::ppc::{Cpu, Host};
use crate::{Error, Guest, Result};

/// Guest span for the controller, its game-side inputs and the exported collections.
pub const WORLD_BASE: u32 = 0x6A00_0000;
pub const WORLD_BYTES: u32 = 0x0010_0000;

/// `*(0x83083C38)`, the game object. Only the two fields the reverb code reads are backed: the
/// span starts `0x2F000` into it, so `+0x2F078` (the local player's audio record pointer) and
/// `+0x2F0B0` (the frame record; `+0x10` = the outdoor `audio_reverb` painter key, written by
/// `sub_827A2E88`) land at the start of [`WORLD_BASE`].
///
/// When the game object already exists (`*(GAME_POINTER)` set: the ambience component's boot,
/// `game::boot_ambience`, builds the real one) the reverb reads the same fields from it instead.
const GAME_POINTER: u32 = 0x8308_3C38;
const GAME: u32 = WORLD_BASE - 0x2F000;
const PLAYER_POINTER_AT: u32 = 0x2F078;
const OUTDOOR_KEY_AT: u32 = 0x2F0B0 + 0x10;
/// The local player's audio record: position, facing, velocity, board position, board facing,
/// board velocity (`sub_824DEEF0` reads the board's `+0x30` / `+0x40`).
const PLAYER_RECORD: u32 = WORLD_BASE + 0x100;
/// The SFXObj_Reverb object (`0x28C` bytes) and its vtable in the image.
const REVERB: u32 = WORLD_BASE + 0x200;
const REVERB_VTABLE: u32 = 0x822F_CDE8;
const CLASS_HANDLES: u32 = WORLD_BASE + 0x800;
const DATA: u32 = WORLD_BASE + 0x1000;
/// The zero block in the image's `.bss` the title points an instance at when a lookup misses.
const MISSING: u32 = 0x830D_0850;

const INIT: u32 = 0x824D_DE50;
const PROCESS: u32 = 0x824D_E548;
const UPDATE: u32 = 0x824D_F220;

const GET_CLASS: u32 = 0x82B6_9B08;
const DATABASE_CLASS: u32 = 0x82B6_BC30;
const FIRST_KEY: u32 = 0x82B6_DED0;
const NEXT_KEY: u32 = 0x82B6_DF30;
const REVERB_INSTANCE: u32 = 0x824D_DC10;
const VOLUME_INSTANCE: u32 = 0x8248_CDC8;
const WOBBLE_INSTANCE: u32 = 0x824C_2580;
const ASSIGN: u32 = 0x82B6_D218;
const RELEASE: u32 = 0x82B6_9978;
const VOLUME_REFSPEC: u32 = 0x8248_CE30;
const ACTIVE_ZONE: u32 = 0x8248_8278;
const WORLD_HOOKS: [u32; 11] = [
    GET_CLASS,
    DATABASE_CLASS,
    FIRST_KEY,
    NEXT_KEY,
    REVERB_INSTANCE,
    VOLUME_INSTANCE,
    WOBBLE_INSTANCE,
    ASSIGN,
    RELEASE,
    VOLUME_REFSPEC,
    ACTIVE_ZONE,
];

pub const AUD_REVERB: u64 = 0x204C_AC1F_D770_88B8;
pub const AUDIO_VOLUME: u64 = 0xF0CE_F367_088E_FFF8;
pub const OUTPUT_WOBBLE: u64 = 0x4CDD_7CDC_1A95_5D5C;

/// One exported Attrib class, its collections in the order Attrib's iteration visits them.
pub struct WorldClass {
    pub key: u64,
    /// `(collection key, raw layout, RefSpec target (class, key))`.
    pub collections: Vec<(u64, Vec<u8>, Option<(u64, u64)>)>,
}

/// One game frame's inputs to the controller.
pub struct ReverbFrame<'a> {
    /// The `audio_reverb` painter key at the player; 0 = none (the title then uses reverb01).
    pub outdoor: u64,
    /// The local player's audio record (six vec4s, see [`PLAYER_RECORD`]).
    pub player: [[f32; 4]; 6],
    /// The active reverb zones, by their index in [`ReverbWorld::install_zones`], in the order
    /// they became active, each with its trigger weight.
    pub zones: &'a [(u32, f32)],
}

pub struct ReverbWorld {
    classes: Vec<(u64, Vec<u64>)>,
    collections: HashMap<(u64, u64), u32>,
    refspecs: HashMap<u32, u32>,
    layouts: HashMap<u32, u32>,
    entries: Vec<u32>,
    active: Vec<u32>,
    next: u32,
    /// Where the game object's outdoor key lives.
    outdoor_key: u32,
}

impl ReverbWorld {
    /// Lay the classes out in the (already mapped) world span and point the game object at it.
    pub fn new(g: &mut Guest, classes: &[WorldClass]) -> Result<Self> {
        g.fill(WORLD_BASE, 0, WORLD_BYTES)?;
        let mut world = Self {
            classes: Vec::new(),
            collections: HashMap::new(),
            refspecs: HashMap::new(),
            layouts: HashMap::new(),
            entries: Vec::new(),
            active: Vec::new(),
            next: DATA,
            outdoor_key: 0,
        };
        for (i, class) in classes.iter().enumerate() {
            let handle = CLASS_HANDLES + 16 * i as u32;
            g.set_u64(handle, class.key)?;
            world
                .classes
                .push((class.key, class.collections.iter().map(|c| c.0).collect()));
            for (key, layout, refspec) in &class.collections {
                // {+0 layout, +4 class handle, +8 key}, then the layout.
                let object = world.take(16 + layout.len() as u32)?;
                let at = object + 16;
                g.set_u32(object, at)?;
                g.set_u32(object + 4, handle)?;
                g.set_u64(object + 8, *key)?;
                g.set_span(at, layout)?;
                world.layouts.insert(object, at);
                world.collections.insert((class.key, *key), object);
                if let Some((class_key, target)) = refspec {
                    let block = world.take(24)?;
                    g.set_u64(block, *class_key)?;
                    g.set_u64(block + 8, *target)?;
                    world.refspecs.insert(object, block);
                }
            }
        }
        let game = match g.u32(GAME_POINTER)? {
            0 => {
                g.set_u32(GAME_POINTER, GAME)?;
                GAME
            }
            real => real,
        };
        g.set_u32(game + PLAYER_POINTER_AT, PLAYER_RECORD)?;
        world.outdoor_key = game + OUTDOOR_KEY_AT;
        Ok(world)
    }

    fn take(&mut self, bytes: u32) -> Result<u32> {
        let at = self.next;
        let end = at + ((bytes + 15) & !15);
        if end > WORLD_BASE + WORLD_BYTES {
            return Err(Error::new(at, "world audio span exhausted"));
        }
        self.next = end;
        Ok(at)
    }

    /// One trigger entry per reverb zone (`+0` → its `.ems` record, `+0xC` weight, `+0x18`
    /// volume key), at fixed addresses: the controller keeps entry pointers across frames.
    pub fn install_zones(&mut self, g: &mut Guest, zones: &[[u8; 72]]) -> Result<()> {
        self.entries.clear();
        self.active.clear();
        for (i, record) in zones.iter().enumerate() {
            let entry = self.take(32 + 72)?;
            g.set_u32(entry, entry + 32)?;
            g.set_u32(entry + 4, 1)?;
            g.set_u32(entry + 8, u32::MAX)?;
            g.set_u32(entry + 0x10, 0x0100_0100 | (i as u32 & 0xFF))?;
            g.set_span(entry + 0x18, &record[48..56])?;
            g.set_span(entry + 32, record)?;
            self.entries.push(entry);
        }
        Ok(())
    }

    /// `sub_824DDE50` on the controller: default preset into both branches (A at 1, B at 0), the
    /// output wobble presets, and the MixMap Reverb controller (`mixmap`, 0 for none) enabled.
    pub fn init<H: Heap + ?Sized, T: Trig + ?Sized>(
        &mut self,
        g: &mut Guest,
        heap: &mut H,
        trig: &mut T,
        mixmap: u32,
    ) -> Result<()> {
        g.fill(REVERB, 0, 0x290)?;
        g.set_u32(REVERB, REVERB_VTABLE)?;
        g.set_u32(REVERB + 0xC, mixmap)?;
        self.call(g, heap, trig, INIT, 0.0)
    }

    /// `sub_824DE548` (process, before the MixMap evaluation) for one frame of `dt` seconds.
    pub fn process<H: Heap + ?Sized, T: Trig + ?Sized>(
        &mut self,
        g: &mut Guest,
        heap: &mut H,
        trig: &mut T,
        frame: &ReverbFrame<'_>,
        dt: f32,
    ) -> Result<()> {
        g.set_u64(self.outdoor_key, frame.outdoor)?;
        for (i, v) in frame.player.iter().enumerate() {
            for (k, x) in v.iter().enumerate() {
                g.set_u32(PLAYER_RECORD + 16 * i as u32 + 4 * k as u32, x.to_bits())?;
            }
        }
        self.active.clear();
        for &(index, weight) in frame.zones {
            let entry = *self
                .entries
                .get(index as usize)
                .ok_or_else(|| Error::new(index, "reverb zone index out of range"))?;
            g.set_u32(entry + 0xC, weight.to_bits())?;
            self.active.push(entry);
        }
        self.call(g, heap, trig, PROCESS, dt)
    }

    /// `sub_824DF220` (update, after the MixMap evaluation): the Reverb controller's outputs into
    /// the network.
    pub fn update<H: Heap + ?Sized, T: Trig + ?Sized>(
        &mut self,
        g: &mut Guest,
        heap: &mut H,
        trig: &mut T,
    ) -> Result<()> {
        self.call(g, heap, trig, UPDATE, 0.0)
    }

    fn call<H: Heap + ?Sized, T: Trig + ?Sized>(
        &mut self,
        g: &mut Guest,
        heap: &mut H,
        trig: &mut T,
        target: u32,
        f1: f32,
    ) -> Result<()> {
        let hooks: Vec<u32> = HOOKS.iter().chain(&WORLD_HOOKS).copied().collect();
        let mut host = WorldPorts {
            ports: Ports { heap, trig },
            world: self,
        };
        run_with(g, &mut host, &hooks, OUTER_SP, target, &[REVERB as u64], &[f64::from(f1)])?;
        Ok(())
    }

    /// The class handle for `class`, or 0.
    fn handle(&self, class: u64) -> u32 {
        self.classes
            .iter()
            .position(|(key, _)| *key == class)
            .map_or(0, |i| CLASS_HANDLES + 16 * i as u32)
    }

    fn keys(&self, g: &Guest, handle: u32) -> Result<&[u64]> {
        let class = g.u64(handle)?;
        Ok(self
            .classes
            .iter()
            .find(|(key, _)| *key == class)
            .map_or(&[][..], |(_, keys)| keys))
    }

    /// Fill a 16-byte `Attrib::Instance` `{collection, layout, 0, 0}` for `(class, key)`.
    fn instance(&self, g: &mut Guest, at: u32, class: u64, key: u64) -> Result<()> {
        let object = self.collections.get(&(class, key)).copied().unwrap_or(0);
        g.set_u32(at, object)?;
        g.set_u32(at + 4, self.layouts.get(&object).copied().unwrap_or(MISSING))?;
        g.set_u32(at + 8, 0)?;
        g.set_u32(at + 12, 0)
    }
}

struct WorldPorts<'a, H: Heap + ?Sized, T: Trig + ?Sized> {
    ports: Ports<'a, H, T>,
    world: &'a mut ReverbWorld,
}

impl<H: Heap + ?Sized, T: Trig + ?Sized> Host for WorldPorts<'_, H, T> {
    fn hook(&mut self, cpu: &mut Cpu, g: &mut Guest, target: u32) -> Result<()> {
        let r = |i: usize| cpu.r[i] as u32;
        let world = &mut *self.world;
        cpu.r[3] = match target {
            GET_CLASS => u64::from(world.handle(cpu.r[3])),
            DATABASE_CLASS => u64::from(world.handle(cpu.r[4])),
            FIRST_KEY => world.keys(g, r(3))?.first().copied().unwrap_or(0),
            NEXT_KEY => {
                let keys = world.keys(g, r(3))?;
                keys.iter()
                    .position(|k| *k == cpu.r[4])
                    .and_then(|i| keys.get(i + 1))
                    .copied()
                    .unwrap_or(0)
            }
            REVERB_INSTANCE | VOLUME_INSTANCE | WOBBLE_INSTANCE => {
                let class = match target {
                    REVERB_INSTANCE => AUD_REVERB,
                    VOLUME_INSTANCE => AUDIO_VOLUME,
                    _ => OUTPUT_WOBBLE,
                };
                world.instance(g, r(3), class, cpu.r[4])?;
                return Ok(());
            }
            // Instance::ChangeCollection: the collection and its layout.
            ASSIGN => {
                g.set_u32(r(3), r(4))?;
                g.set_u32(r(3) + 4, world.layouts.get(&r(4)).copied().unwrap_or(MISSING))?;
                return Ok(());
            }
            // ponytail: collections here are never freed, so a release is a no-op.
            RELEASE => return Ok(()),
            VOLUME_REFSPEC => {
                let object = g.u32(r(3))?;
                u64::from(world.refspecs.get(&object).copied().unwrap_or(MISSING))
            }
            // `sub_82488278(root, prev)`: the first active reverb zone that is not `prev`.
            ACTIVE_ZONE => world
                .active
                .iter()
                .find(|&&entry| entry != r(4))
                .map_or(0, |&entry| u64::from(entry)),
            _ => return self.ports.hook(cpu, g, target),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest() -> Guest {
        let mut g = Guest::single(WORLD_BASE, WORLD_BYTES as usize);
        g.put(GAME_POINTER & !0xFFF, vec![0; 0x1000]);
        g.put(0x7000_0000, vec![0; 0x4_0000]);
        g
    }

    #[test]
    fn the_reverb_backs_its_own_game_object_only_when_there_is_none() {
        let mut g = guest();
        let world = ReverbWorld::new(&mut g, &[]).unwrap();
        assert_eq!(g.u32(GAME_POINTER).unwrap(), GAME);
        assert_eq!(g.u32(GAME + PLAYER_POINTER_AT).unwrap(), PLAYER_RECORD);
        assert_eq!(world.outdoor_key, GAME + OUTDOOR_KEY_AT);
    }

    #[test]
    fn the_reverb_shares_the_game_object_the_ambience_boot_built() {
        let mut g = guest();
        g.set_u32(GAME_POINTER, 0x7000_0000).unwrap();
        let world = ReverbWorld::new(&mut g, &[]).unwrap();
        assert_eq!(g.u32(GAME_POINTER).unwrap(), 0x7000_0000);
        assert_eq!(g.u32(0x7000_0000 + PLAYER_POINTER_AT).unwrap(), PLAYER_RECORD);
        assert_eq!(world.outdoor_key, 0x7000_0000 + 0x2F0C0);
    }
}
