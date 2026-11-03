//! World-audio data: `private/stock/data/audio/world-audio.json`, written at setup by
//! `tools/asset_pipeline/audio_world.py` from the owned disc.
//!
//! * `classes`: the audio vault classes (reverb presets, audio volumes, painter ambiences and
//!   emitter sets, district configs, the output wobble) with each collection's raw layout. The
//!   title's own loaders run over these layouts as original code, so they are kept verbatim.
//! * `districts`: per `DIST_<Name>`, the audio painter layers of its Sim stream and the zone boxes
//!   of every `.ems` file its district config lists.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

pub const PATH: &str = "private/stock/data/audio/world-audio.json";

/// `Attrib::RefSpec` field of an audio volume: the collection it points at (a reverb preset).
const VOLUME_REFSPEC: &str = "Hash_99FD793BC30CF0FA";
/// `Sk8::Audio::eVolumeType` of an audio volume.
const VOLUME_TYPE: &str = "Hash_6D18B8674D7E5337";

#[derive(Deserialize)]
struct RawWorld {
    classes: HashMap<String, Vec<RawCollection>>,
    districts: HashMap<String, RawDistrict>,
}

#[derive(Deserialize)]
struct RawCollection {
    key: String,
    parent: String,
    layout: String,
    fields: HashMap<String, Field>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Field {
    #[serde(rename = "type")]
    pub kind: String,
    pub data: String,
    #[serde(default)]
    pub array: Option<FieldArray>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct FieldArray {
    pub items: Vec<String>,
    #[serde(default)]
    pub text_items: Vec<String>,
}

#[derive(Deserialize)]
struct RawDistrict {
    layers: HashMap<String, Vec<RawTile>>,
    #[serde(default)]
    zones: HashMap<String, Vec<RawZone>>,
    #[serde(default)]
    crossfade_bank: String,
}

#[derive(Deserialize)]
struct RawTile {
    cx: f32,
    cz: f32,
    hx: f32,
    hz: f32,
    nodes: String,
    palette: Vec<String>,
    high: bool,
}

#[derive(Deserialize)]
struct RawZone {
    index: u32,
    flags: u32,
    pos: [f32; 3],
    ext: [f32; 3],
    shape: [f32; 4],
    key: String,
    gains: [f32; 4],
}

/// One vault collection: its parent, raw layout block and decoded fields.
#[derive(Clone, Debug)]
pub struct Collection {
    pub parent: u64,
    pub layout: Vec<u8>,
    pub fields: HashMap<String, Field>,
}

/// One class's collections, in the order Attrib's class iteration visits them (the exporter
/// replays the game's hash-table fill; see `audio_world.py`).
#[derive(Clone, Debug, Default)]
pub struct Class {
    pub order: Vec<u64>,
    pub collections: HashMap<u64, Collection>,
}

impl Class {
    /// `field` of `key`, searched up the parent chain the way an Attrib lookup falls back.
    pub fn field(&self, key: u64, field: &str) -> Option<&Field> {
        let mut at = self.collections.get(&key);
        for _ in 0..16 {
            let collection = at?;
            if let Some(found) = collection.fields.get(field) {
                return Some(found);
            }
            at = self.collections.get(&collection.parent);
        }
        None
    }
}

/// A painter layer tile: a 128 m quadtree whose leaves index a palette of vault keys.
#[derive(Clone, Debug)]
pub struct Tile {
    pub centre: [f32; 2],
    pub half: [f32; 2],
    /// `{child (-x,-z), (-x,+z), (+x,-z), (+x,+z), value}`; a leaf has `child[0] == 0xFFFF`.
    pub nodes: Vec<[u16; 5]>,
    pub palette: Vec<u64>,
    /// From a `cSim_<x>_<z>_high.xsf` cell rather than `cSim_Global.xsf`.
    pub high: bool,
}

impl Tile {
    fn covers(&self, x: f32, z: f32) -> bool {
        (self.centre[0] - self.half[0]..self.centre[0] + self.half[0]).contains(&x)
            && (self.centre[1] - self.half[1]..self.centre[1] + self.half[1]).contains(&z)
    }

    /// `sub_82C0F0B0`: descend to the leaf at (x, z). `None` is unpainted (value `0xFFFF`).
    pub fn sample(&self, x: f32, z: f32) -> Option<u64> {
        let (mut cx, mut cz) = (self.centre[0], self.centre[1]);
        let (mut hx, mut hz) = (self.half[0], self.half[1]);
        let mut node = *self.nodes.first()?;
        for _ in 0..self.nodes.len() {
            if node[0] == 0xFFFF {
                return self.palette.get(usize::from(node[4])).copied();
            }
            let (px, pz) = (x >= cx, z >= cz);
            hx *= 0.5;
            hz *= 0.5;
            cx += if px { hx } else { -hx };
            cz += if pz { hz } else { -hz };
            let child = node[usize::from(px) * 2 + usize::from(pz)];
            node = *self.nodes.get(usize::from(child))?;
        }
        None
    }
}

/// The painter value of a layer at (x, z): the high-detail tile where one is loaded, else the
/// global one. `None` is outside every tile or unpainted.
pub fn sample(tiles: &[Tile], x: f32, z: f32) -> Option<u64> {
    let mut covering = tiles.iter().filter(|tile| tile.covers(x, z));
    let first = covering.next()?;
    let tile = if first.high {
        first
    } else {
        covering.find(|tile| tile.high).unwrap_or(first)
    };
    tile.sample(x, z)
}

/// An `.ems` zone: a box rotated about y, pointing at an audio volume collection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub index: u32,
    pub flags: u32,
    pub position: [f32; 3],
    /// Half extents.
    pub extent: [f32; 3],
    /// `{inner fraction, cos, 0, sin}` of the rotation about y.
    pub shape: [f32; 4],
    pub key: u64,
    pub gains: [f32; 4],
}

impl Zone {
    /// `p` in the box frame, each axis as a fraction of its half extent.
    fn local(&self, p: [f32; 3]) -> [f32; 3] {
        let d = [
            p[0] - self.position[0],
            p[1] - self.position[1],
            p[2] - self.position[2],
        ];
        let (c, s) = (self.shape[1], self.shape[3]);
        let l = [c * d[0] + s * d[2], d[1], -s * d[0] + c * d[2]];
        [
            (l[0] / self.extent[0]).abs(),
            (l[1] / self.extent[1]).abs(),
            (l[2] / self.extent[2]).abs(),
        ]
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.local(p).iter().all(|u| *u <= 1.0)
    }

    /// The trigger entry's `+0xC` weight: 0 in the inner `shape[0]` fraction of the box, rising
    /// to 1 at its edge.
    ///
    /// FITTED, not the title's function: measured from the camera in two Xenia walks
    /// (AUDIO_HANDOFF §7.8; median error 0, p90 0.18). The real function was not found.
    pub fn weight(&self, p: [f32; 3]) -> f32 {
        let u = self.local(p);
        let inner = self.shape[0];
        let edge = |u: f32| ((u - inner) / (1.0 - inner)).clamp(0.0, 1.0);
        edge(u[0]).max(edge(u[2]))
    }

    /// The raw 72-byte `.ems` record, as the title keeps it in memory.
    pub fn record(&self) -> [u8; 72] {
        let mut out = [0u8; 72];
        out[0..4].copy_from_slice(&self.index.to_be_bytes());
        out[4..8].copy_from_slice(&self.flags.to_be_bytes());
        let floats = self
            .position
            .iter()
            .chain(&self.extent)
            .chain(&self.shape)
            .collect::<Vec<_>>();
        for (i, v) in floats.iter().enumerate() {
            out[8 + 4 * i..12 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        out[48..56].copy_from_slice(&self.key.to_be_bytes());
        for (i, v) in self.gains.iter().enumerate() {
            out[56 + 4 * i..60 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

#[derive(Clone, Debug, Default)]
pub struct District {
    /// Painter layers by name (`audio_reverb`, `audio_ambience`, `audio_emitters`).
    pub layers: HashMap<String, Vec<Tile>>,
    /// Zone boxes by `.ems` file name.
    pub zones: HashMap<String, Vec<Zone>>,
    /// `data/audio/Main_Ambience_Crossfade_<Uni|DT|Ind>.abk`, for the three big districts.
    pub crossfade_bank: String,
}

impl District {
    pub fn layer(&self, name: &str) -> &[Tile] {
        self.layers.get(name).map_or(&[], Vec::as_slice)
    }

    /// Every zone of every `.ems` file, in file order.
    pub fn all_zones(&self) -> impl Iterator<Item = &Zone> {
        let mut names: Vec<&String> = self.zones.keys().collect();
        names.sort();
        names.into_iter().flat_map(|name| self.zones[name].iter())
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorldAudio {
    pub classes: HashMap<String, Class>,
    pub districts: HashMap<String, District>,
}

/// AttribSys's 64-bit name hash (lookup2's 64-bit mix), as `tools/asset_pipeline/vlt.py` has it.
pub fn hash64(text: &str) -> u64 {
    fn mix(mut a: u64, mut b: u64, mut c: u64) -> (u64, u64, u64) {
        for (who, shift) in [
            (0, 43),
            (1, -9),
            (2, 8),
            (0, 38),
            (1, -23),
            (2, 5),
            (0, 35),
            (1, -49),
            (2, 11),
            (0, 12),
            (1, -18),
            (2, 22),
        ] {
            match who {
                0 => a = a.wrapping_sub(b).wrapping_sub(c) ^ (c >> shift),
                1 => b = b.wrapping_sub(c).wrapping_sub(a) ^ (a << -shift),
                _ => c = c.wrapping_sub(a).wrapping_sub(b) ^ (b >> shift),
            }
        }
        (a, b, c)
    }
    if text.is_empty() {
        return 0;
    }
    let data = text.as_bytes();
    let word = |bytes: &[u8]| {
        let mut w = [0u8; 8];
        w[..bytes.len()].copy_from_slice(bytes);
        u64::from_le_bytes(w)
    };
    let (mut a, mut b, mut c) = (0xABCD_EF00_1122_3344u64, 0xABCD_EF00_1122_3344u64, 0x9E37_79B9_7F4A_7C13u64);
    let mut chunks = data.chunks_exact(24);
    for chunk in &mut chunks {
        (a, b, c) = mix(
            a.wrapping_add(word(&chunk[0..8])),
            b.wrapping_add(word(&chunk[8..16])),
            c.wrapping_add(word(&chunk[16..24])),
        );
    }
    let tail = chunks.remainder();
    let part = |from: usize, to: usize| word(&tail[from.min(tail.len())..to.min(tail.len())]);
    a = a.wrapping_add(part(0, 8));
    b = b.wrapping_add(part(8, 16));
    c = c
        .wrapping_add(data.len() as u64)
        .wrapping_add(part(16, 24) << 8);
    mix(a, b, c).2
}

fn hex_key(text: &str) -> Result<u64, String> {
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 16).map_err(|e| format!("vault key {text:?}: {e}"))
}

fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

impl WorldAudio {
    pub fn load(assets: &Path) -> Result<Self, String> {
        let path = assets.join(PATH);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e} (rerun setup)", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawWorld = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut classes = HashMap::new();
        for (name, collections) in raw.classes {
            let mut class = Class::default();
            for collection in collections {
                let key = hex_key(&collection.key)?;
                class.order.push(key);
                class.collections.insert(
                    key,
                    Collection {
                        parent: hex_key(&collection.parent)?,
                        layout: hex_bytes(&collection.layout)?,
                        fields: collection.fields,
                    },
                );
            }
            classes.insert(name, class);
        }
        let mut districts = HashMap::new();
        for (name, district) in raw.districts {
            let mut layers = HashMap::new();
            for (layer, tiles) in district.layers {
                let tiles = tiles
                    .into_iter()
                    .map(|tile| {
                        let bytes = hex_bytes(&tile.nodes)?;
                        Ok(Tile {
                            centre: [tile.cx, tile.cz],
                            half: [tile.hx, tile.hz],
                            nodes: bytes
                                .chunks_exact(10)
                                .map(|n| {
                                    core::array::from_fn(|i| {
                                        u16::from_be_bytes([n[2 * i], n[2 * i + 1]])
                                    })
                                })
                                .collect(),
                            palette: tile
                                .palette
                                .iter()
                                .map(|k| hex_key(k))
                                .collect::<Result<_, _>>()?,
                            high: tile.high,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                layers.insert(layer, tiles);
            }
            let mut zones = HashMap::new();
            for (file, boxes) in district.zones {
                let boxes = boxes
                    .into_iter()
                    .map(|z| {
                        Ok(Zone {
                            index: z.index,
                            flags: z.flags,
                            position: z.pos,
                            extent: z.ext,
                            shape: z.shape,
                            key: hex_key(&z.key)?,
                            gains: z.gains,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                zones.insert(file, boxes);
            }
            districts.insert(
                name,
                District {
                    layers,
                    zones,
                    crossfade_bank: district.crossfade_bank,
                },
            );
        }
        Ok(Self { classes, districts })
    }

    pub fn class(&self, name: &str) -> Option<&Class> {
        self.classes.get(name)
    }

    /// The district a map plays in: `DIST_<map name>` (the engine's maps are named after the
    /// disc world they were converted from).
    pub fn district(&self, map: &str) -> Option<&District> {
        self.districts.get(&format!("DIST_{map}"))
    }

    /// An audio volume's `eVolumeType` (1 emitter, 4 music, 5 reverb, 6 speaker, 7 crowd).
    pub fn volume_type(&self, key: u64) -> Option<u32> {
        let field = self.class("audio_volume")?.field(key, VOLUME_TYPE)?;
        u32::from_str_radix(&field.data, 16).ok()
    }

    /// An audio volume's `RefSpec` target: for a reverb zone, its `aud_reverb` preset key.
    pub fn volume_refspec(&self, key: u64) -> Option<(u64, u64)> {
        let field = self.class("audio_volume")?.field(key, VOLUME_REFSPEC)?;
        let bytes = hex_bytes(&field.data).ok()?;
        let word = |at: usize| Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
        Some((word(0)?, word(8)?))
    }

    /// The classes SFXObj_Reverb's code reads, for `AuthoredRuntime::reverb_install`.
    pub fn reverb_classes(&self) -> Vec<skate_audio_core::environment::WorldClass> {
        use skate_audio_core::environment::{AUD_REVERB, AUDIO_VOLUME, OUTPUT_WOBBLE, WorldClass};
        [("aud_reverb", AUD_REVERB), ("audio_volume", AUDIO_VOLUME), ("output_wobble", OUTPUT_WOBBLE)]
            .into_iter()
            .filter_map(|(name, key)| {
                let class = self.class(name)?;
                Some(WorldClass {
                    key,
                    collections: class
                        .order
                        .iter()
                        .map(|k| {
                            let refspec = (key == AUDIO_VOLUME).then(|| self.volume_refspec(*k)).flatten();
                            (*k, class.collections[k].layout.clone(), refspec)
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// A district's reverb zones: every `.ems` box whose volume is a reverb volume (type 5).
    pub fn reverb_zones(&self, district: &District) -> Vec<Zone> {
        district
            .all_zones()
            .filter(|zone| self.volume_type(zone.key) == Some(5))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(nodes: Vec<[u16; 5]>, palette: Vec<u64>, high: bool) -> Tile {
        Tile {
            centre: [64.0, 64.0],
            half: [64.0, 64.0],
            nodes,
            palette,
            high,
        }
    }

    #[test]
    fn a_tile_descends_by_quadrant_and_reads_the_leaf_palette() {
        // Root split once: (-x,-z) -> palette 0, (-x,+z) unpainted, (+x,-z) -> 1, (+x,+z) -> 0.
        let t = tile(
            vec![
                [1, 2, 3, 4, 0],
                [0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0],
                [0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF],
                [0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 1],
                [0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0],
            ],
            vec![0xA, 0xB],
            false,
        );
        assert_eq!(t.sample(10.0, 10.0), Some(0xA));
        assert_eq!(t.sample(10.0, 100.0), None);
        assert_eq!(t.sample(100.0, 10.0), Some(0xB));
        assert_eq!(t.sample(100.0, 100.0), Some(0xA));
        let high = tile(vec![[0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0]], vec![0xC], true);
        assert_eq!(sample(&[t.clone(), high], 10.0, 10.0), Some(0xC), "high cells win");
        assert_eq!(sample(&[t], 200.0, 10.0), None, "outside every tile");
    }

    #[test]
    fn a_rotated_zone_fades_from_its_inner_fraction_to_its_edge() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let zone = Zone {
            index: 0,
            flags: 0,
            position: [0.0, 0.0, 0.0],
            extent: [10.0, 5.0, 4.0],
            shape: [0.5, s, 0.0, s],
            key: 1,
            gains: [1.0; 4],
        };
        assert!(zone.contains([0.0, 0.0, 0.0]));
        assert_eq!(zone.weight([0.0, 0.0, 0.0]), 0.0);
        // Along the box's local x axis, which is 45 degrees between world x and z.
        let along = |d: f32| [d * s, 0.0, d * s];
        assert!((zone.weight(along(7.5)) - 0.5).abs() < 1e-5);
        assert!(zone.contains(along(9.9)) && !zone.contains(along(10.1)));
        assert!(!zone.contains([0.0, 6.0, 0.0]), "the box has a height");
        assert_eq!(&zone.record()[48..56], &1u64.to_be_bytes());
    }

    #[test]
    fn hash64_matches_the_vault_tools() {
        assert_eq!(hash64("reverb01"), 0xA278_2D75_A971_CC8C);
        assert_eq!(hash64("reverb02"), 0x407A_FA1D_6C7C_EAD8);
        assert_eq!(hash64("dist_university"), 0xCB47_3E68_A2EA_FABA);
        assert_eq!(hash64("audio_reverb"), 0x6879_AFCF_A737_F03A);
    }

    /// Replays the session-3 Xenia log `xlive_uni.txt` (University; the running game's active
    /// preset at the player's (x, z), outdoors) through the exported painter layer.
    #[test]
    #[ignore = "needs SKATE_ASSETS (with world-audio.json) and SKATE_XLIVE=<xlive_uni.txt>"]
    fn the_outdoor_reverb_map_matches_the_running_game() {
        let (Some(assets), Some(log)) = (std::env::var_os("SKATE_ASSETS"), std::env::var_os("SKATE_XLIVE")) else {
            return;
        };
        let world = WorldAudio::load(Path::new(&assets)).unwrap();
        let tiles = world.district("University").unwrap().layer("audio_reverb");
        let names: HashMap<u64, String> =
            (1..=24).map(|i| (hash64(&format!("reverb{i:02}")), format!("reverb{i:02}"))).collect();
        let (mut same, mut total) = (0, 0);
        for line in std::fs::read_to_string(log).unwrap().lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            // "t pos x z zones - active <name> ..."
            if f.len() < 7 || f[1] != "pos" || f[4] != "zones" || f[5] != "-" || f[7] == "fading" {
                continue;
            }
            let (x, z) = (f[2].parse::<f32>().unwrap(), f[3].parse::<f32>().unwrap());
            let got = sample(tiles, x, z).map_or("reverb01".to_string(), |k| names[&k].clone());
            total += 1;
            if got == f[7] {
                same += 1;
            } else {
                eprintln!("({x}, {z}): game {} map {got}", f[7]);
            }
        }
        eprintln!("{same}/{total}");
        assert!(total > 30 && same == total);
    }

    /// SFXObj_Reverb's own init and update, run over the exported University data from the
    /// start position (outdoors, painter key reverb02) until the crossfade settles, against the
    /// running game's network there (session-2 Xenia dump `xnet_uni1.txt`).
    #[test]
    #[ignore = "needs SKATE_ASSETS (world-audio.json and the audio runtime image)"]
    fn the_reverb_controller_reaches_the_running_games_network() {
        use skate_audio_core::authored::AuthoredRuntime;
        use skate_audio_core::environment::ReverbFrame;
        let Some(assets) = std::env::var_os("SKATE_ASSETS") else { return };
        let assets = Path::new(&assets);
        let world = WorldAudio::load(assets).unwrap();
        let guest = super::super::catalog::load_guest_image(
            &assets.join("private/stock/audio-runtime-image"),
        )
        .unwrap();
        let mut rt = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
        rt.load_mixmap(&std::fs::read(assets.join(super::super::catalog::MIXMAP_PATH)).unwrap())
            .unwrap();
        // The global controllers' free-skate values (the player sound sets these at start).
        for &(key, id, value) in skate_audio_core::mixmap::inputs::FREE_SKATE_GLOBALS {
            let ctrl = rt.mixmap_controller(key).unwrap();
            rt.mixmap_set(ctrl, id, value).unwrap();
        }
        let district = world.district("University").unwrap();
        let zones: Vec<[u8; 72]> = world.reverb_zones(district).iter().map(Zone::record).collect();
        rt.reverb_install(&world.reverb_classes(), &zones).unwrap();
        let (x, z) = (403.8, -725.1);
        let outdoor = sample(district.layer("audio_reverb"), x, z).unwrap_or(0);
        assert_eq!(outdoor, hash64("reverb02"));
        let player = [
            [x, 135.1, z, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [0.0; 4],
            [x, 135.0, z - 0.3, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [0.0; 4],
        ];
        for _ in 0..300 {
            let frame = ReverbFrame { outdoor, player, zones: &[] };
            rt.reverb_process(&frame, 1.0 / 60.0).unwrap();
            rt.mixmap_tick(1.0 / 60.0).unwrap();
            rt.reverb_update().unwrap();
            for _ in 0..3 {
                rt.pump_once().unwrap();
            }
        }
        assert_network_matches_the_running_game(&mut rt);
    }

    /// The network values `xnet_uni1.txt` read from the running game at the University start
    /// (outdoor preset reverb02), and the final outputs' wobble records.
    fn assert_network_matches_the_running_game(rt: &mut skate_audio_core::authored::AuthoredRuntime) {
        let g = rt.guest();
        const MANAGER: u32 = 0x4000_2000;
        // The install wired each final output's first Send (module 3) into C7's Sub0:
        // `repoint_link` leaves the Send's node owner at `[send + 72]` = C7 + 52.
        let c7 = g.u32(g.u32(MANAGER + 52).unwrap()).unwrap();
        for i in 0..2 {
            let send = g.u32(g.u32(MANAGER + 116 + 4 * i).unwrap() + 12).unwrap();
            assert_eq!(g.u32(send + 72).unwrap(), c7 + 52, "output {i}'s first send is not in C7");
        }
        let module = |graph: u32, m: u32| {
            let player = g.u32(MANAGER + 4 * graph).unwrap();
            g.u32(player + 80 + 4 * m).unwrap()
        };
        let param = |graph: u32, m: u32, k: u32| {
            g.f32(g.u32(module(graph, m) + 16).unwrap() + 4 * k).unwrap()
        };
        // The branch that settled active (the dump's is A; which one it is depends on history).
        let active = if param(0, 1, 1) > 0.5 { 0 } else { 1 };
        let (c8, rv, ca) = (1 + active, 3 + active, 5 + 2 * active);
        let out = |i: u32, m: u32, k: u32| {
            // Manager +116: the final outputs' module tables.
            let table = g.u32(MANAGER + 116 + 4 * i).unwrap();
            let module = g.u32(table + 4 * m).unwrap();
            g.f32(g.u32(module + 16).unwrap() + 4 * k).unwrap()
        };
        let checks = [
            ("C7 send active", param(0, 1 + active, 1), 1.0),
            ("C7 send idle", param(0, 2 - active, 1), 0.0),
            ("C8 gain", param(c8, 1, 1), 0.75),
            ("C8 send", param(c8, 3, 1), 0.998871),
            ("pre-delay", param(rv, 1, 1), 0.08),
            ("RM10 +4", param(rv, 2, 1), 1.04),
            ("RM10 +12", param(rv, 2, 3), 8.3),
            ("RM10 +20", param(rv, 2, 5), 1.0),
            ("return gain", param(rv, 3, 1), 1.5),
            ("CA HI20", param(ca, 1, 1), 0.0),
            ("CA LI20", param(ca, 2, 1), 10000.0),
            ("CA pan L", param(ca, 4, 1), 270.0),
            ("CA pan R", param(ca + 1, 4, 1), 90.0),
            ("output send to C7", out(0, 3, 1), 0.0705893),
            ("output gain", out(0, 4, 1), 0.997711),
        ];
        if std::env::var_os("SKATE_REVERB_DEBUG").is_some() {
            for off in (0x60..0xD0).step_by(4) {
                eprintln!("manager+{off:#x} = {:#010x}", g.u32(MANAGER + off).unwrap());
            }
            let ctrl = rt.mixmap_controller(0x4000_0050).unwrap();
            eprintln!("reverb outputs {:?}", (0..8).map(|i| rt.mixmap_level(ctrl, i)).collect::<Vec<_>>());
            for i in 0..2 {
                let table = g.u32(MANAGER + 116 + 4 * i).unwrap();
                eprintln!("out{i} modules {:?}", (0..7).map(|m| format!("{:#x}", g.u32(table + 4 * m).unwrap())).collect::<Vec<_>>());
            }
        }
        // The final outputs' wobble (modulator records 10..13 at manager +0x188), from the output
        // wobble presets `sub_824DDF58` loads: {rate, depth, min, max} as the dump has them.
        let record = |r: u32, k: u32| g.f32(MANAGER + 0x188 + 32 * r + 4 * k).unwrap();
        let wobble = [
            (10, [4.0, 0.6, 0.0005, 0.0005]),
            (11, [3.33333, 0.0, 250.0, 250.0]),
            (12, [2.0, 0.9, 0.002, 0.002]),
            (13, [3.33333, 0.2, 1500.0, 1500.0]),
        ];
        let checks: Vec<(String, f32, f32)> = checks
            .into_iter()
            .map(|(n, got, want)| (n.to_string(), got, want))
            .chain(wobble.into_iter().flat_map(|(r, want)| {
                [1, 3, 4, 5]
                    .into_iter()
                    .zip(want)
                    .map(move |(k, want)| (format!("record {r} word {k}"), record(r, k), want))
            }))
            .collect();
        let mut bad = Vec::new();
        for (name, got, want) in checks {
            let same = (got - want).abs() <= 1e-3 * want.abs().max(1.0);
            eprintln!("{name:20} engine {got:12} retail {want} {}", if same { "" } else { "DIFFER" });
            if !same {
                bad.push(name);
            }
        }
        assert!(bad.is_empty(), "differ from the running game: {bad:?}");
    }

    #[test]
    /// The same values with the title world (the whole title audio boot, world only) driving the
    /// network instead of the standalone controller.
    #[test]
    #[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
    fn the_title_world_reaches_the_running_games_network() {
        use skate_audio_core::authored::AuthoredRuntime;
        let Some(assets) = std::env::var_os("SKATE_ASSETS") else { return };
        let assets = Path::new(&assets);
        let world = WorldAudio::load(assets).unwrap();
        let catalog = super::super::catalog::PlayerAudioCatalog::from_assets(assets, None).unwrap();
        let mixmap = catalog.mixmap.clone().unwrap();
        let mut rt = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks).unwrap();
        rt.load_mixmap(&mixmap).unwrap();
        let db = assets.join("private/stock/data/db");
        let read = |name: &str| std::fs::read(db.join(name)).unwrap();
        let vault = skate_audio_core::attrib::Vault::parse(
            &read("skaterschema.vlt"),
            &read("skaterschema.bin"),
            &read("skatercollections.vlt"),
            &read("skatercollections.bin"),
        )
        .unwrap();
        let files = super::super::title::files(super::super::title::roots(assets));
        rt.wire_environment().unwrap();
        rt.game_boot(vault, files).0.unwrap();
        rt.game_world_only();
        rt.game_set_district(hash64("dist_university")).unwrap();
        let district = world.district("University").unwrap();
        let (x, y, z) = (403.8f32, 135.1f32, -725.1f32);
        // A camera 3 m from the skater outside every reverb zone (the dump's was outside them: its
        // C7 sat fully on one branch), so the outdoor preset alone decides.
        let zones = world.reverb_zones(district);
        let camera = [1.5f32, 4.0, 8.0]
            .into_iter()
            .flat_map(|dy| {
                [3.0f32, 6.0].into_iter().flat_map(move |r| {
                    (0..8).map(move |k| {
                        let a = k as f32 * std::f32::consts::FRAC_PI_4;
                        [x + r * a.cos(), y + dy, z + r * a.sin()]
                    })
                })
            })
            .find(|c| zones.iter().all(|zone| !zone.contains(*c)))
            .expect("a camera spot outside the zones");
        eprintln!("camera {camera:?}");
        {
            let g = rt.guest_mut();
            let game = g.u32(skate_audio_core::game::GAME_OBJECT).unwrap();
            let rows: [[f32; 4]; 4] =
                [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, -0.447, -0.894, 0.0], [camera[0], camera[1], camera[2], 1.0]];
            for (r, row) in rows.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    g.set_u32(game + 0x2_FD20 + 0x1230 + 16 * r as u32 + 4 * c as u32, v.to_bits()).unwrap();
                }
            }
            g.set_u8(game + 0x2_FD20 + 0x1220, 1).unwrap();
            let frame = game + skate_audio_core::game::SKATER_FRAME;
            for (offset, layer) in [(0x40u32, "audio_emitters"), (0x48, "audio_ambience"), (0x50, "audio_reverb")] {
                g.set_u64(frame + offset, sample(district.layer(layer), x, z).unwrap_or(0)).unwrap();
            }
            let record = frame + skate_audio_core::game::SKATER_RECORDS;
            let player = [[x, y, z, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0; 4], [x, y - 0.1, z - 0.3, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0; 4]];
            for (i, v) in player.iter().flatten().enumerate() {
                g.set_u32(record + 4 * i as u32, v.to_bits()).unwrap();
            }
            g.set_u32(frame + 8, record).unwrap();
            g.set_u32(frame + 0xC, 1).unwrap();
            g.set_u32(frame, 1.0f32.to_bits()).unwrap();
        }
        for _ in 0..300 {
            rt.game_frame(1.0 / 60.0).0.unwrap();
            for _ in 0..3 {
                rt.pump_once().unwrap();
            }
        }
        assert_network_matches_the_running_game(&mut rt);
    }

    #[test]
    fn classes_keep_their_order_and_fields_fall_back_to_parents() {
        let text = r#"{"classes":{"c":[{"key":"00000000000000B2","parent":"00000000000000A1","layout":"0102","fields":{}},{"key":"00000000000000A1","parent":"","layout":"","fields":{"x":{"type":"t","data":"7"}}}]},"districts":{}}"#;
        let world = WorldAudio::parse(text).unwrap();
        let class = world.class("c").unwrap();
        assert_eq!(class.order, [0xB2, 0xA1]);
        assert_eq!(class.collections[&0xB2].layout, [1, 2]);
        assert_eq!(class.field(0xB2, "x").unwrap().data, "7");
    }
}
