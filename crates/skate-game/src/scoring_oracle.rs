//! TEST-ONLY oracle: TU3's whole ScoreModule (ctor 82DA2930, reset 82DA3070, update 82DA33E0) run instruction by
//! instruction beside [`Runtime`], on the same inputs, and compared after every frame. Nothing here ships and no EA
//! bytes are in the repo: the code is the installed runtime image (`$SKATE3_ASSET_ROOT/private/stock/
//! audio-runtime-image`, the pages `skate_core::physics::tu3_oracle` loads) and the vault records are
//! `$SKATE_SCORING_LAYOUTS`, the raw layout blobs of the collector, tuning and scorable collections dumped from the
//! owned disc's db.big.
//!
//! Run natively instead of from the image, because they reach state the image does not hold (built at boot):
//! - 828AAE90 / 828AAF88, the allocator: a bump heap.
//! - 82B69B08, the Attrib find: served from the dumped blobs (records whose +0x24 is the layout, so the real bind
//!   82B6CC78 and release 82B69978 run unchanged).
//! - 82DA5AC8, the score packet's name -> scorable id (its table 830C72C0 is filled at boot): the frame's id.
//! - The four context fillers 82DA7888 calls (82DA69B0 pedestrians, 82DA7550 vehicles, 82DA76F8 skaters, 82DA6F58
//!   objects): they query the living world. They find nothing here, as the port's empty slots find nothing.
use super::{Frame, Runtime};
use skate_audio_core::ppc::{Cpu, Host, Interp};
use skate_audio_core::{Guest, Segment};
use skate_core::physics::filtered_state::FilteredCategory;
use std::collections::HashMap;

const HEAP: u32 = 0x6000_0000;
const HEAP_BYTES: u32 = 0x0100_0000;
const STACK_TOP: u32 = 0x7000_0000;
const STACK_BYTES: u32 = 0x0004_0000;

const COLLECTOR_CLASS: u64 = 0x546C_36B6_5603_8E04;
const TUNING_CLASS: u64 = 0x3492_15E2_E817_703C;
const SCORABLE_CLASS: u64 = 0x6918_4699_84A8_C596;

const ALLOC: u32 = 0x828A_AE90;
const FREE: u32 = 0x828A_AF88;
const FIND: u32 = 0x82B6_9B08;
/// 82B72420, a record's attribute by key outside its layout (it walks the class's table, built at boot).
const GET: u32 = 0x82B7_2420;
const PACKET_ID: u32 = 0x82DA_5AC8;
const CONTEXT_FILLERS: [u32; 4] = [0x82DA_69B0, 0x82DA_7550, 0x82DA_76F8, 0x82DA_6F58];

struct Natives {
    next: u32,
    collections: HashMap<(u64, u64), u32>,
    /// Each record's attributes outside its layout (the dump's `entries`): (record, key) -> a word holding the value.
    attributes: HashMap<(u32, u64), u32>,
    descriptor: i32,
}

impl Natives {
    fn alloc(&mut self, bytes: u32) -> u32 {
        let at = self.next;
        self.next = (at + bytes.max(1) + 15) & !15;
        assert!(self.next <= HEAP + HEAP_BYTES, "oracle heap exhausted");
        at
    }
}

impl Host for Natives {
    fn hook(&mut self, cpu: &mut Cpu, _g: &mut Guest, target: u32) -> skate_audio_core::Result<()> {
        match target {
            ALLOC => cpu.r[3] = u64::from(self.alloc(cpu.r[3] as u32)),
            FIND => cpu.r[3] = u64::from(self.collections.get(&(cpu.r[3], cpu.r[4])).copied().unwrap_or(0)),
            GET => cpu.r[3] = u64::from(self.attributes.get(&(cpu.r[3] as u32, cpu.r[4])).copied().unwrap_or(0)),
            PACKET_ID => cpu.r[3] = u64::from(self.descriptor as u32),
            _ => {}
        }
        Ok(())
    }
}

/// One frame's physical inputs, written both into the owner blocks the module reads and into the port's [`Frame`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Raw {
    pub dt: f32,
    /// `[owner+0x40]+0`, the filtered category (1 ground .. 7 offboard air).
    pub category: i32,
    /// `[owner+0x1C]+0x10` and `+0xC`: the physical state and its hundreds.
    pub state: i32,
    /// The score packet's scorable (-1 none); its flags are `flags`.
    pub descriptor: i32,
    pub grind_id: i32,
    pub flags: u32,
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub rider_up: [f32; 3],
    pub rider_forward: [f32; 3],
    pub board_up: [f32; 3],
    pub board_forward: [f32; 3],
    pub reckoning_up: [f32; 3],
    pub com_velocity: [f32; 3],
    pub ground_normal: [f32; 3],
    pub grind_normal: [f32; 3],
    pub wheel_count: u32,
    pub pump: f32,
    pub switch: bool,
    pub fakie: bool,
    pub body_flip: bool,
    pub body_flip_side: bool,
    pub suspend_air: bool,
    pub hips_position: [f32; 3],
    pub hips_ground: Option<[f32; 3]>,
    pub landing: skate_core::animation::landing_quality::Output,
    pub reverting: bool,
    pub state70: bool,
    pub request_teleport: bool,
    pub tipslide_326: bool,
    pub half_turns: i32,
    pub teleported: bool,
    /// The challenge's trick-type mask (input +0x74..+0x81, the port's `trick_types`).
    pub trick_types: [bool; 14],
    /// Input +0xAE4 bit 0x40000000, ManualsAreConnectors (the port's `manuals_connect`).
    pub manuals_connect: bool,
    /// State +0x4E, landing +0xA2 and the board block's +0x143 (the port's `runout_78`, `flags_2484_31`,
    /// `retrieving_board_323`).
    pub runout: bool,
    pub flags_2484_31: bool,
    pub retrieving_board: bool,
}

impl Default for Raw {
    fn default() -> Self {
        Self {
            dt: 1. / 60.,
            category: 1,
            state: 100,
            descriptor: -1,
            grind_id: -1,
            flags: 0,
            position: [0.; 3],
            forward: [0., 0., 1.],
            rider_up: [0., 1., 0.],
            rider_forward: [0., 0., 1.],
            board_up: [0., 1., 0.],
            board_forward: [0., 0., 1.],
            reckoning_up: [0., 1., 0.],
            com_velocity: [0., 0., 5.],
            ground_normal: [0., 1., 0.],
            grind_normal: [0., 1., 0.],
            wheel_count: 4,
            pump: 0.,
            switch: false,
            fakie: false,
            body_flip: false,
            body_flip_side: false,
            suspend_air: false,
            hips_position: [0., 1., 0.],
            hips_ground: None,
            landing: Default::default(),
            reverting: false,
            state70: false,
            request_teleport: false,
            tipslide_326: false,
            half_turns: 0,
            teleported: false,
            trick_types: [true; 14],
            manuals_connect: true,
            runout: false,
            flags_2484_31: false,
            retrieving_board: false,
        }
    }
}

/// The owner (PhysOut) blocks, at the offsets 82DA33E0's callees load them from.
#[derive(Clone, Copy)]
struct Blocks {
    motion: u32,
    air: u32,
    clock: u32,
    grinds: u32,
    skeleton: u32,
    collision: u32,
    state: u32,
    ground: u32,
    reckoning: u32,
    animation: u32,
    landing: u32,
    out: u32,
    filtered: u32,
    /// `[owner+0x48]`, the board controller's block (82D76D20).
    board: u32,
}

pub(super) struct Retail {
    g: Guest,
    cpu: Interp,
    host: Natives,
    pub module: u32,
    blocks: Blocks,
    pub input: u32,
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

impl Retail {
    /// The installed image and the dumped vault, or `None` without `SKATE3_ASSET_ROOT` / `SKATE_SCORING_LAYOUTS`.
    pub fn load() -> Option<Self> {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?);
        let layouts = std::path::PathBuf::from(std::env::var_os("SKATE_SCORING_LAYOUTS")?);
        let mut segments = Vec::new();
        for entry in std::fs::read_dir(root.join("private/stock/audio-runtime-image")).ok()? {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            let Some(page) = name.strip_prefix("g_").and_then(|s| s.strip_suffix(".bin")) else {
                continue;
            };
            segments.push(Segment { base: u32::from_str_radix(page, 16).ok()? << 16, bytes: std::fs::read(&path).ok()? });
        }
        segments.push(Segment { base: HEAP, bytes: vec![0; HEAP_BYTES as usize] });
        segments.push(Segment { base: STACK_TOP - STACK_BYTES, bytes: vec![0; STACK_BYTES as usize] });
        let mut cpu = Interp::new();
        cpu.mxcsr = 0x1F80;
        cpu.hooks = [ALLOC, FREE, FIND, GET, PACKET_ID].into_iter().chain(CONTEXT_FILLERS).collect();
        let mut o = Self {
            g: Guest::from_segments(segments),
            cpu,
            host: Natives { next: HEAP, collections: HashMap::new(), attributes: HashMap::new(), descriptor: -1 },
            module: 0,
            blocks: Blocks {
                motion: 0,
                air: 0,
                clock: 0,
                grinds: 0,
                skeleton: 0,
                collision: 0,
                state: 0,
                ground: 0,
                reckoning: 0,
                animation: 0,
                landing: 0,
                out: 0,
                filtered: 0,
                board: 0,
            },
            input: 0,
        };
        assert_eq!(o.u32(0x82DA_33E0), 0x7D88_02A6, "0x82DA33E0 is not `mflr r12`: not the TU3 image");
        // The dumped records, as Attrib collections whose +0x24 is the layout (82B6CC78) and whose refcount (+4) a
        // release (82B69978) never takes to zero.
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(layouts).ok()?).ok()?;
        for c in json["collections"].as_array()? {
            let class = match c["cls"].as_str()? {
                "collector" => COLLECTOR_CLASS,
                "tuning" => TUNING_CLASS,
                "scorable" => SCORABLE_CLASS,
                _ => continue,
            };
            let key = skate_data::attrib_hash::numeric_id(c["key"].as_str()?);
            let blob = hex(c["layout"].as_str()?);
            let at = o.host.alloc(blob.len() as u32);
            for (i, b) in blob.iter().enumerate() {
                o.set_u8(at + i as u32, *b);
            }
            let record = o.host.alloc(0x40);
            o.set_u32(record + 4, 0x0100_0000);
            // +0x18: the collection's key, which 82DA25B8 looks its scorable id up by.
            o.set_u32(record + 0x18, (key >> 32) as u32);
            o.set_u32(record + 0x1C, key as u32);
            o.set_u32(record + 0x24, at);
            o.set_u32(record + 0x28, 1);
            o.host.collections.insert((class, key), record);
            // shortcut: a word holding each entry's raw value; the pointer-typed ones (text) point nowhere. Upgrade
            // with the vault's data blob if a scored path reads one.
            for e in c["entries"].as_array().into_iter().flatten() {
                let (Some(name), Some(raw)) = (e["name"].as_str(), e["raw"].as_str()) else { continue };
                let word = o.host.alloc(4);
                o.set_u32(word, u32::from_str_radix(raw, 16).ok()?);
                o.host.attributes.insert((record, skate_data::attrib_hash::numeric_id(name)), word);
            }
        }
        // 82DA25B8's key -> scorable id map at [0x83085448], built at boot (826DCE80): `+8` buckets, `+0xC` their count,
        // each a chain of {key, +8 id, +0x10 next} by the key's low word modulo the count; `buckets[count]` ends it.
        // From the scorable table 820862A8 (stride 24, the name at +0x14).
        const BUCKETS: u32 = 337;
        let table = o.host.alloc(0x10);
        let buckets = o.host.alloc(4 * (BUCKETS + 1));
        o.set_u32(table + 8, buckets);
        o.set_u32(table + 0xC, BUCKETS);
        for id in 0..332u32 {
            let name = o.c_string(o.u32(0x8208_62A8 + 24 * id + 0x14));
            let key = skate_data::attrib_hash::numeric_id(&name);
            let node = o.host.alloc(0x18);
            o.set_u32(node, (key >> 32) as u32);
            o.set_u32(node + 4, key as u32);
            o.set_u32(node + 8, id);
            let slot = buckets + 4 * (key as u32 % BUCKETS);
            let next = o.u32(slot);
            o.set_u32(node + 0x10, next);
            o.set_u32(slot, node);
        }
        o.set_u32(0x8308_5448, table);
        // 82DA45C8, 82DA46D0, 82DA5E78 and 82DA6378 read the tuning (the announcement curve +0xB4, the unannounced factor
        // +0x15C) through a boot-time instance, `[[0x830CFDA4] + 0x118] + 4`: its layout.
        let tuning = *o.host.collections.get(&(TUNING_CLASS, skate_data::attrib_hash::numeric_id("default")))?;
        let (singleton, instance) = (o.host.alloc(0x120), o.host.alloc(8));
        o.set_u32(instance + 4, o.u32(tuning + 0x24));
        o.set_u32(singleton + 0x118, instance);
        o.set_u32(0x830C_FDA4, singleton);
        // The ground slots' static scorable names (CRT initializers 82F901C0..82F90238 -> 823C3B00): bspowerslide,
        // fspowerslide, nosemanual, tailmanual, bsrevert, fsrevert.
        for init in [0x82F9_01C0, 0x82F9_01D8, 0x82F9_01F0, 0x82F9_0208, 0x82F9_0220, 0x82F9_0238] {
            o.call(init, &[]);
        }
        // The owner: every pointer slot 82DA33E0's callees load (82DA3B38 reads `[owner+0x48]+0x143`), each a zeroed block.
        let owner = o.host.alloc(0x80);
        let block = |o: &mut Self, slot: u32, bytes: u32| {
            let at = o.host.alloc(bytes);
            o.set_u32(owner + slot, at);
            at
        };
        o.blocks = Blocks {
            motion: block(&mut o, 0x00, 0x200),
            air: block(&mut o, 0x08, 0x400),
            clock: block(&mut o, 0x0C, 0x40),
            grinds: block(&mut o, 0x10, 0x400),
            skeleton: block(&mut o, 0x14, 0x400),
            collision: block(&mut o, 0x18, 0x100),
            state: block(&mut o, 0x1C, 0x100),
            ground: block(&mut o, 0x20, 0x400),
            reckoning: block(&mut o, 0x24, 0x100),
            animation: block(&mut o, 0x30, 0x200),
            landing: block(&mut o, 0x38, 0x200),
            out: block(&mut o, 0x3C, 0x4000),
            filtered: block(&mut o, 0x40, 0x100),
            board: 0,
        };
        for slot in [0x04, 0x28, 0x2C, 0x34].into_iter().chain((0x44..0x80).step_by(4)) {
            block(&mut o, slot, 0x400);
        }
        o.blocks.board = o.u32(owner + 0x48);
        // 82DF23D4: the module is 0x90 bytes, built on the owner, then reset (82DF23F4).
        o.module = o.host.alloc(0x90);
        o.call(0x82DA_2930, &[o.module, owner]);
        o.call(0x82DA_3070, &[o.module]);
        // The per-frame input 8275F430 builds: every trick type scoring (ResetTrickTypeScoring), the bias 1, and
        // +2788 with ManualsAreConnectors (0x40000000), the multiplier gate (0x04000000) and offline (0x00800000).
        o.input = o.host.alloc(0xB00);
        for t in 0..14 {
            o.set_u8(o.input + 0x74 + t, 1);
        }
        o.set_f32(o.input + 0xAE0, 1.);
        o.set_u32(o.input + 0xAE4, 0x4480_0000);
        Some(o)
    }

    pub fn u32(&self, at: u32) -> u32 {
        self.g.u32(at).unwrap()
    }
    pub fn f32(&self, at: u32) -> f32 {
        f32::from_bits(self.u32(at))
    }
    pub fn u8(&self, at: u32) -> u8 {
        self.g.u8(at).unwrap()
    }
    fn set_u32(&mut self, at: u32, v: u32) {
        self.g.set_u32(at, v).unwrap();
    }
    fn set_f32(&mut self, at: u32, v: f32) {
        self.set_u32(at, v.to_bits());
    }
    fn set_u8(&mut self, at: u32, v: u8) {
        self.g.set_u8(at, v).unwrap();
    }
    fn c_string(&self, at: u32) -> String {
        (at..).map(|a| self.u8(a)).take_while(|&b| b != 0).map(char::from).collect()
    }
    fn set_v3(&mut self, at: u32, v: [f32; 3]) {
        for (i, x) in v.into_iter().enumerate() {
            self.set_f32(at + 4 * i as u32, x);
        }
        self.set_f32(at + 12, 0.);
    }
    fn call(&mut self, target: u32, args: &[u32]) {
        self.cpu.cpu.r[1] = u64::from(STACK_TOP - 0x100);
        let args: Vec<u64> = args.iter().map(|&a| u64::from(a)).collect();
        if let Err(e) = self.cpu.call(&mut self.g, &mut self.host, target, &args) {
            panic!("TU3 {target:08X} stopped at {:08X}: {}", e.address, e.message);
        }
    }

    /// One frame (`tick`, the port's Frame::tick): the teleport's reset (82DB8998 -> 82DF2848 -> 82DA3070) first, then
    /// the owner blocks, then the update (82DEF810 -> 82DA33E0).
    pub fn step(&mut self, tick: u32, raw: &Raw) {
        if raw.teleported {
            self.call(0x82DA_3070, &[self.module]);
        }
        // The input's frame counter (+0xAC0), which 82DA3C68 -> 82DA7888 copies to the clock (module+0x20)+0x238 at the
        // update's start: carriers announce once it passes their start tick + hold (82DA5DE0), and only an announced
        // grind or ground carrier gains its metric (82DA9F90 / 82DB0588). Left 0, none ever announced.
        self.set_u32(self.input + 0xAC0, tick);
        let b = self.blocks;
        self.set_f32(b.clock, raw.dt);
        self.set_u32(b.filtered, raw.category as u32);
        self.set_u32(b.filtered + 0x30, raw.grind_id as u32);
        self.set_u32(b.state + 0xC, (raw.state / 100 * 100) as u32);
        self.set_u32(b.state + 0x10, raw.state as u32);
        self.set_u8(b.state + 0x42, u8::from(raw.reverting));
        self.set_u8(b.state + 0x45, u8::from(raw.request_teleport));
        self.set_u8(b.state + 0x46, u8::from(raw.state70));
        self.host.descriptor = raw.descriptor;
        // The packet (`[owner+0x30]+0x30`, which 82DAC498 copies): the scorable's five-word name, then its flags at +0x30.
        let name = u32::try_from(raw.descriptor).map_or([0; 5], |id| {
            let text = self.c_string(self.u32(0x8208_62A8 + 24 * id + 0x14));
            skate_core::animation::skeleton_input::name::encode(text.as_bytes()).0
        });
        for (i, word) in (0u32..).zip(name) {
            self.set_u32(b.animation + 0x30 + 4 * i, word);
        }
        self.set_u32(b.animation + 0x60, raw.flags);
        self.set_u8(b.state + 0x4E, u8::from(raw.runout));
        self.set_u8(b.landing + 0xA2, u8::from(raw.flags_2484_31));
        self.set_u8(b.board + 0x143, u8::from(raw.retrieving_board));
        self.set_v3(b.motion + 0x90, raw.position);
        self.set_v3(b.motion + 0xA0, raw.forward);
        // The two deck matrices (+0x00 for 82DA8BE0, +0x40 the effective one 82DAB2A8 tilts on), rows right/up/fwd.
        for base in [0x00, 0x40] {
            let right = cross(raw.board_up, raw.board_forward);
            self.set_v3(b.motion + base, right);
            self.set_v3(b.motion + base + 0x10, raw.board_up);
            self.set_v3(b.motion + base + 0x20, raw.board_forward);
            self.set_v3(b.motion + base + 0x30, raw.position);
        }
        self.set_v3(b.skeleton + 0x170, cross(raw.rider_up, raw.rider_forward));
        self.set_v3(b.skeleton + 0x180, raw.rider_up);
        self.set_v3(b.skeleton + 0x190, raw.rider_forward);
        self.set_v3(b.skeleton + 0x1A0, raw.position);
        self.set_v3(b.reckoning, raw.com_velocity);
        self.set_v3(b.reckoning + 0x60, raw.reckoning_up);
        self.set_v3(b.ground + 0x50, raw.ground_normal);
        self.set_v3(b.grinds + 0x20, raw.grind_normal);
        self.set_u8(b.grinds + 0x146, u8::from(raw.tipslide_326));
        self.set_u32(b.collision, raw.wheel_count);
        self.set_f32(b.ground + 0x10C, raw.pump);
        self.set_u32(b.ground + 0x138, raw.half_turns as u32);
        self.set_v3(b.ground + 0xC0, raw.hips_position);
        self.set_v3(b.ground + 0xD0, raw.hips_ground.unwrap_or([0.; 3]));
        self.set_u8(b.ground + 0x140, u8::from(raw.hips_ground.is_some()));
        self.set_u8(b.air + 0x1B9, u8::from(raw.body_flip));
        self.set_u8(b.air + 0x1BD, u8::from(raw.body_flip_side));
        self.set_u8(b.air + 0x1C4, u8::from(raw.suspend_air));
        self.set_u8(b.landing + 0x9B, u8::from(raw.fakie));
        self.set_u8(b.landing + 0x9D, u8::from(raw.switch));
        let l = raw.landing;
        self.set_f32(b.landing + 0x50, l.landing_adjust_80);
        self.set_f32(b.landing + 0x54, l.sideways_speed_84);
        self.set_f32(b.landing + 0x58, l.forward_speed_88);
        self.set_f32(b.landing + 0x5C, l.spin_92);
        self.set_u32(b.landing + 0x60, l.landing_type_96);
        self.set_u8(b.landing + 0xA7, u8::from(l.landing_data_167));
        for (t, &on) in (0u32..).zip(&raw.trick_types) {
            self.set_u8(self.input + 0x74 + t, u8::from(on));
        }
        let bits = self.u32(self.input + 0xAE4) & !0x4000_0000;
        self.set_u32(self.input + 0xAE4, bits | if raw.manuals_connect { 0x4000_0000 } else { 0 });
        self.call(0x82DA_33E0, &[self.module, self.input]);
    }

    pub fn holder(&self) -> u32 {
        self.u32(self.module + 0x24)
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The port's frame for `raw`, as physics/frame.rs builds it from the same PhysOut fields.
pub(super) fn frame(tick: u32, raw: &Raw, runtime: &Runtime) -> Frame {
    let category = match raw.category {
        1 => FilteredCategory::Ground,
        2 => FilteredCategory::Air,
        3 => FilteredCategory::Grind,
        4 => FilteredCategory::Wipeout,
        5 => FilteredCategory::Teleport,
        6 => FilteredCategory::Offboard,
        7 => FilteredCategory::OffboardAir,
        _ => FilteredCategory::Invalid,
    };
    let hundreds = raw.state / 100 * 100;
    Frame {
        tick,
        dt: raw.dt,
        category,
        state: raw.state as u32,
        descriptor: usize::try_from(raw.descriptor).ok().and_then(|id| runtime.data.by_id(id)).map(|d| d.encoded_name),
        grind_id: raw.grind_id,
        flags: raw.flags,
        position: raw.position,
        com_velocity: raw.com_velocity,
        forward: raw.forward,
        rider_up: raw.rider_up,
        rider_forward: raw.rider_forward,
        board_up: raw.board_up,
        board_forward: raw.board_forward,
        reckoning_up: raw.reckoning_up,
        takeoff_normal: match hundreds {
            100 => raw.ground_normal,
            400 => raw.grind_normal,
            _ => [0., 1., 0.],
        },
        wheel_count: raw.wheel_count,
        pump_acceleration: raw.pump,
        switch: raw.switch,
        regular: true,
        fakie: raw.fakie,
        nollie: false,
        body_flip: raw.body_flip,
        body_flip_side: raw.body_flip_side,
        hips_position: raw.hips_position,
        hips_ground: raw.hips_ground,
        hips_surface: 0,
        suspend_air: raw.suspend_air,
        landing: raw.landing,
        teleported: raw.teleported,
        reverting: raw.reverting,
        manual_block_70: raw.state70,
        request_teleport: raw.request_teleport,
        tipslide_326: raw.tipslide_326,
        half_turns_312: raw.half_turns,
        runout_78: raw.runout,
        flags_2484_31: raw.flags_2484_31,
        retrieving_board_323: raw.retrieving_board,
    }
}

/// What both sides must agree on after a frame: the holder's sequence and line, its live part, the pending flag, the
/// two timers, and on the ground the slots' total.
pub(super) fn compare(step: usize, label: &str, retail: &Retail, port: &Runtime) -> Result<(), String> {
    let h = retail.holder();
    let s = &port.session.holder.snapshot;
    let mut diffs = Vec::new();
    let mut f = |what: &str, r: f32, p: f32| {
        if r.to_bits() != p.to_bits() && !(r.is_nan() && p.is_nan()) {
            diffs.push(format!("{what}: retail {r} port {p}"));
        }
    };
    f("completed lines +0x18", retail.f32(h + 0x18), s.completed_lines);
    f("line +0x1C", retail.f32(h + 0x1C), s.line);
    f("accumulated +0x20", retail.f32(h + 0x20), s.accumulated);
    f("last reward +0x24", retail.f32(h + 0x24), s.last_reward);
    f("general +0x28", retail.f32(h + 0x28), s.general_pending);
    f("fingerflip +0x2C", retail.f32(h + 0x2C), s.fingerflip_pending);
    f("grind +0x12A8", retail.f32(h + 0x12A8), s.grind_reward);
    f("live +0x30", retail.f32(h + 0x30), port.live);
    // The ground collector's slot total (82DAA4C0 +116) while both sides run it.
    if port.collector == super::Collector::Ground && retail.u32(retail.module + 0x38) == 0 {
        f("ground slots +0x74", retail.f32(retail.u32(retail.module + 0x10) + 0x74), port.ground_live);
    }
    let line = retail.u32(retail.module + 0x28);
    let combo = retail.u32(retail.module + 0x2C);
    f("line timer", retail.f32(line + 0x18), port.session.line.points);
    f("combo timer", retail.f32(combo + 0x18), port.session.combo.timer.points);
    let pending = retail.u8(h + 0x1C5C) != 0;
    if pending != port.session.holder.has_pending_sequence() {
        diffs.push(format!("pending +0x1C5C: retail {pending} port {}", port.session.holder.has_pending_sequence()));
    }
    if diffs.is_empty() { Ok(()) } else { Err(format!("{label} frame {step}: {}", diffs.join("; "))) }
}

fn runtime() -> Runtime {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
    Runtime::load(&skate_data::collections::Collections::load(&root).unwrap()).unwrap()
}

fn id(port: &Runtime, identifier: &str) -> i32 {
    port.data.definitions.iter().find(|d| d.identifier == identifier).map(|d| d.metadata.id as i32).unwrap_or_else(|| {
        panic!("no scorable {identifier}")
    })
}

/// Both sides through `frames`; the first disagreement is the error.
/// Both sides through `frames`: the first disagreement, with each side's collector, or `None`. A retail stop (an
/// unmapped load, an unhooked boot-time table) is caught and reported the same way, so one run reports every scene.
fn run(label: &str, frames: &[Raw]) -> Option<String> {
    let label = label.to_string();
    let frames = frames.to_vec();
    std::panic::catch_unwind(move || {
        let mut retail = Retail::load().expect("SKATE3_ASSET_ROOT and SKATE_SCORING_LAYOUTS");
        let mut port = runtime();
        let traced = trace_frames(&label);
        // The first disagreement; a traced scene runs on through its traced frames after it (retail's own course).
        let mut first = None;
        for (i, raw) in frames.iter().enumerate() {
            retail.step(i as u32, raw);
            (port.trick_types, port.manuals_connect) = (raw.trick_types, raw.manuals_connect);
            if let Err(e) = port.advance(frame(i as u32, raw, &port)) {
                return first.or(Some(format!("{label} frame {i}: port error {e}")));
            }
            if traced.contains(&i) {
                eprintln!("{}", trace(i, &retail, &port));
            }
            if first.is_none()
                && let Err(e) = compare(i, &label, &retail, &port)
            {
                let index = retail.u32(retail.module + 0x38) as i32;
                first = Some(format!("{e} [retail collector {index}, port {:?}; raw {raw:?}]", port.collector));
            }
            if first.is_some() && !traced.contains(&(i + 1)) {
                break;
            }
        }
        first
    })
    .unwrap_or_else(|panic| {
        let text = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()));
        Some(format!("panicked: {}", text.unwrap_or_default()))
    })
}

/// `SKATE_ORACLE_TRACE=<scene>:<first>-<last>[;...]` (scene names hold commas): the frames of a scene to print both
/// sides of ([`trace`]).
fn trace_frames(label: &str) -> std::ops::RangeInclusive<usize> {
    let wanted = std::env::var("SKATE_ORACLE_TRACE").unwrap_or_default();
    let range = (wanted.split(';').filter_map(|w| w.rsplit_once(':')))
        .find(|(scene, _)| *scene == label)
        .and_then(|(_, r)| r.split_once('-'));
    match range.and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) {
        Some((first, last)) => first..=last,
        None => 1..=0,
    }
}

/// One frame of both sides: retail's collector, bail and lost bytes, pending flag and landing countdown, the
/// holder's buckets, its +0x30 and the line, the line and combo objects; then the port's.
fn trace(i: usize, retail: &Retail, port: &Runtime) -> String {
    let h = retail.holder();
    let (line, combo) = (retail.u32(retail.module + 0x28), retail.u32(retail.module + 0x2C));
    let words = |at: u32| (0..8).map(|k| format!("{}", retail.f32(at + 0x10 + 4 * k))).collect::<Vec<_>>().join(",");
    let s = &port.session.holder.snapshot;
    format!(
        "TRACE {i}: retail tick {} idx {} bail {} lost {} pend {} count {} gen {} ff {} acc {} last {} line {} done {} h30 {} slots {} L728 {:08x} \
         line[{}] combo[{}] | port {:?} pend {} count {} gen {} ff {} acc {} last {} line {} done {} h30 {} slots {} open {} mult {} line {} combo {}",
        retail.u32(retail.u32(retail.module + 0x20) + 0x238),
        retail.u32(retail.module + 0x38) as i32,
        retail.u8(retail.module + 0x80),
        retail.u8(retail.blocks.out + 0x3941),
        retail.u8(h + 0x1C5C),
        retail.u32(h + 0x1C58),
        retail.f32(h + 0x28),
        retail.f32(h + 0x2C),
        retail.f32(h + 0x20),
        retail.f32(h + 0x24),
        retail.f32(h + 0x1C),
        retail.f32(h + 0x18),
        retail.f32(h + 0x30),
        retail.f32(retail.u32(retail.module + 0x10) + 0x74),
        retail.u32(retail.u32(h + 4) + 0x728),
        words(line),
        words(combo),
        port.collector,
        port.session.holder.has_pending_sequence(),
        port.landing_countdown,
        s.general_pending,
        s.fingerflip_pending,
        s.accumulated,
        s.last_reward,
        s.line,
        s.completed_lines,
        port.live,
        port.ground_live,
        port.sequence_active,
        port.session.combo.multiplier,
        port.session.line.points,
        port.session.combo.timer.points,
    )
}

/// A scripted skater: rolling, popping, flying and landing, building [`Raw`] frames.
struct Script {
    frames: Vec<Raw>,
    raw: Raw,
    ground_y: f32,
}

impl Script {
    fn new() -> Self {
        Self { frames: Vec::new(), raw: Raw::default(), ground_y: 0. }
    }
    fn push(&mut self, n: usize, mut each: impl FnMut(usize, &mut Raw)) -> &mut Self {
        for i in 0..n {
            each(i, &mut self.raw);
            self.raw.position = std::array::from_fn(|k| self.raw.position[k] + self.raw.com_velocity[k] * self.raw.dt);
            self.frames.push(self.raw);
            self.raw.landing.landing_data_167 = false;
            self.raw.teleported = false;
        }
        self
    }
    fn roll(&mut self, n: usize) -> &mut Self {
        let y = self.ground_y;
        self.push(n, |_, r| {
            r.category = 1;
            r.state = 100;
            r.wheel_count = 4;
            r.descriptor = -1;
            r.flags = 0;
            r.com_velocity[1] = 0.;
            r.position[1] = y;
        })
    }
    /// A pop of `up` m/s for `n` frames carrying `trick` (with `flags`) from frame `trick_from` on.
    fn air(&mut self, n: usize, up: f32, trick: i32, flags: u32, trick_from: usize) -> &mut Self {
        self.raw.com_velocity[1] = up;
        self.push(n, |i, r| {
            r.category = 2;
            r.state = 200;
            r.wheel_count = 0;
            r.com_velocity[1] -= 9.81 * r.dt;
            (r.descriptor, r.flags) = if i >= trick_from { (trick, flags) } else { (-1, 0) };
        })
    }
    fn land(&mut self, clean: bool) -> &mut Self {
        self.land_at(clean, 0.)
    }
    /// Lands at height `y` (a ledge above the take-off, or a drop below it).
    fn land_at(&mut self, clean: bool, y: f32) -> &mut Self {
        self.ground_y = y;
        self.push(1, |_, r| {
            r.category = 1;
            r.state = 100;
            r.wheel_count = 4;
            r.descriptor = -1;
            r.flags = 0;
            r.com_velocity[1] = 0.;
            r.position[1] = y;
            r.landing.landing_data_167 = true;
            r.landing.landing_type_96 = if clean { 0 } else { 1 };
        })
    }
    fn manual(&mut self, n: usize, nose: bool) -> &mut Self {
        self.push(n, |_, r| {
            r.category = 1;
            r.state = 100;
            r.wheel_count = 2;
            r.flags = if nose { 0x0800_0000 } else { 0x0400_0000 };
        });
        self.raw.flags = 0;
        self
    }
    fn grind(&mut self, n: usize, grind: i32, state: i32) -> &mut Self {
        self.push(n, |_, r| {
            r.category = 3;
            r.state = state;
            r.grind_id = grind;
            r.wheel_count = 0;
            r.descriptor = -1;
            r.flags = 0;
            r.com_velocity[1] = 0.;
        });
        self.raw.grind_id = -1;
        self
    }
    /// Off the board for `n` frames of offboard air in physical `state`, naming `trick` for the first `named` and
    /// turning up to `turns` half turns.
    fn hippy(&mut self, n: usize, trick: i32, named: usize, turns: i32, state: i32) -> &mut Self {
        self.raw.com_velocity[1] = 4.;
        self.push(n, |i, r| {
            r.category = 7;
            r.state = state;
            r.wheel_count = 0;
            r.com_velocity[1] -= 9.81 * r.dt;
            (r.descriptor, r.flags) = if i < named { (trick, 0x0100_0000) } else { (-1, 0) };
            r.half_turns = turns * i as i32 / n as i32;
        });
        self.raw.half_turns = 0;
        self
    }
}

/// Every scene through both scorers; the report lists each scene's first disagreement.
#[test]
#[ignore = "needs SKATE3_ASSET_ROOT (the installed TU3 runtime image) and SKATE_SCORING_LAYOUTS (the dumped vault)"]
fn the_score_module_matches_retail() {
    let port = runtime();
    let kickflip = id(&port, "kickflip");
    let fiftyfifty = port.data.definitions.iter().find(|d| d.metadata.score_type == 5).map(|d| d.metadata.id as i32).unwrap();
    let mut scenes: Vec<(&str, Vec<Raw>)> = Vec::new();
    scenes.push(("idle", Script::new().roll(240).frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, -1, 0, 0).land(true).roll(400);
    scenes.push(("ollie", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(400);
    scenes.push(("kickflip", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).manual(180, false).roll(400);
    scenes.push(("kickflip + manual", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(20, 3.5, -1, 0, 0).grind(90, fiftyfifty, 401).air(20, 2., -1, 0, 0).land(true).roll(400);
    scenes.push(("grind", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60);
    // Up a quarter pipe: the reckoning up is the ramp's normal and the take-off velocity runs up its face.
    s.push(1, |_, r| {
        r.reckoning_up = [0., 0., -1.];
        r.com_velocity = [0., 6., 0.5];
    });
    s.air(60, 6., kickflip, 0x0100_0000, 6).land(true).roll(400);
    scenes.push(("vert air", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(20);
    s.push(1, |_, r| r.teleported = true).roll(300);
    scenes.push(("teleport", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(20, 3.5, -1, 0, 0).grind(60, fiftyfifty, 401).land(true).roll(30).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(400);
    scenes.push(("grind, roll, kickflip", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(30, 5.5, -1, 0, 0).land_at(true, 1.).roll(400);
    scenes.push(("ollie up a 1 m ledge", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(70, 2., kickflip, 0x0100_0000, 4).land_at(true, -5.).roll(400);
    scenes.push(("kickflip down a 5 m drop", s.frames.clone()));
    let mut s = Script::new();
    s.air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(300);
    scenes.push(("spawned in the air", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(20);
    s.push(60, |_, r| (r.category, r.state, r.wheel_count) = (4, 300, 0)).roll(300);
    scenes.push(("kickflip, bail", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(20, 3.5, -1, 0, 0);
    s.raw.tipslide_326 = true;
    s.grind(60, fiftyfifty, 401);
    s.raw.tipslide_326 = false;
    s.air(20, 2., -1, 0, 0).land(true).roll(400);
    scenes.push(("tipslide", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).push(90, |_, r| r.flags = 0x8000_0000).roll(400);
    scenes.push(("powerslide", s.frames.clone()));
    let hippyjump = id(&port, "hippyjump");
    let mut s = Script::new();
    s.roll(60).hippy(40, hippyjump, 40, 2, 503).land(true).roll(400);
    scenes.push(("hippy jump 360", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).hippy(40, hippyjump, 10, 2, 503).land(true).roll(400);
    scenes.push(("hippy jump held by state 503", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).hippy(40, hippyjump, 10, 2, 500).land(true).roll(400);
    scenes.push(("hippy jump lost before the landing", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).push(1, |_, r| r.com_velocity = [0., 0., 1.]).hippy(40, hippyjump, 40, 0, 503).land(true).roll(400);
    scenes.push(("slow hippy jump", s.frames.clone()));
    // contest_shared 687-692's Air_Only mask.
    let air_only: [bool; 14] = std::array::from_fn(|t| matches!(t, 1..=4));
    let mut s = Script::new();
    s.raw.trick_types = air_only;
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(10).air(20, 3.5, -1, 0, 0);
    s.grind(90, fiftyfifty, 401).air(20, 2., -1, 0, 0).land(true).roll(400);
    scenes.push(("air only: kickflip, grind", s.frames.clone()));
    let mut s = Script::new();
    s.raw.trick_types = air_only;
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).manual(180, false).push(90, |_, r| r.flags = 0x8000_0000).roll(400);
    scenes.push(("air only: kickflip, manual, powerslide", s.frames.clone()));
    // 82DAAC10: a side-1 revert, then one with no side bit (the side cleared at the first one's end).
    let revert = |s: &mut Script, n: usize, flags: u32| {
        s.push(n, |_, r| (r.state70, r.flags) = (true, flags)).push(1, |_, r| (r.state70, r.flags) = (false, 0));
    };
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true);
    revert(&mut s, 30, 0x2000_0000);
    s.roll(30).air(40, 4.5, -1, 0, 0).land(true);
    revert(&mut s, 30, 0);
    s.roll(400);
    scenes.push(("kickflip, revert, ollie, revert", s.frames.clone()));
    let coffin = id(&port, "coffin");
    let mut s = Script::new();
    s.roll(60).push(120, |_, r| (r.descriptor, r.flags) = (coffin, 0x0300_0000)).roll(400);
    scenes.push(("coffin", s.frames.clone()));
    // 82DAAA44 / 82DAAF80: manuals not connecting, the trick bit on every other frame.
    let mut s = Script::new();
    s.raw.manuals_connect = false;
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true);
    s.push(120, |i, r| (r.descriptor, r.flags) = (coffin, if i % 2 == 0 { 0x0300_0000 } else { 0x0200_0000 })).roll(400);
    scenes.push(("manuals off: kickflip, coffin", s.frames.clone()));
    let mut s = Script::new();
    s.raw.manuals_connect = false;
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true);
    s.push(180, |i, r| (r.wheel_count, r.flags) = (2, if i % 2 == 0 { 0x0500_0000 } else { 0x0400_0000 })).roll(400);
    scenes.push(("manuals off: kickflip, manual", s.frames.clone()));
    // 82DA3430..3480 / 82DA3B38: the three line-loss inputs, one frame each mid-line.
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(20);
    s.push(1, |_, r| r.runout = true).roll(400);
    scenes.push(("kickflip, runout", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(20);
    s.push(1, |_, r| r.flags_2484_31 = true).roll(400);
    scenes.push(("kickflip, flags 2484 bit 31", s.frames.clone()));
    let mut s = Script::new();
    s.roll(60).air(40, 4.5, kickflip, 0x0100_0000, 4).land(true).roll(200);
    s.push(5, |_, r| r.retrieving_board = true).roll(300);
    scenes.push(("kickflip, retrieving the board", s.frames.clone()));
    let mut report = Vec::new();
    for (label, frames) in &scenes {
        match run(label, frames) {
            Some(e) => report.push(format!("FAIL {e}")),
            None => report.push(format!("ok   {label} ({} frames)", frames.len())),
        }
    }
    let text = report.join("\n");
    eprintln!("{text}");
    assert!(report.iter().all(|r| r.starts_with("ok")), "\n{text}");
}
