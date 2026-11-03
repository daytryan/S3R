//! The world side of the audio frame: what the title's world-audio objects see each frame, from
//! the exported district data (`skate_data::audio::world`).
//!
//! Reverb: SFXObj_Reverb runs as original code in the runtime (`AuthoredRuntime::reverb_process`
//! / `reverb_update`). This supplies its inputs the way the rest of the game does in retail:
//! * the outdoor key, `sub_827A2E88`'s `audio_reverb` painter lookup at the player's (x, z);
//! * the trigger system's active reverb zones: a zone is active while the camera is inside its
//!   rotated box, listed in the order the zones became active, each with its weight.
//!
//! Ambience: SFXObj_Ambience runs as original code too (`AuthoredRuntime::ambience_boot`): it
//! picks the district's bed from the `audio_ambience` painter key at the player (`sub_827A2E88`'s
//! lookup at the player's (x, z), written at the frame record `+0x48`), fades it in and out as the
//! key changes and plays it as a 6-channel SndPlayer graph. What this supplies is the key, the
//! title's files and the district's decoded beds (`skate_data::audio::title::Beds`).
//!
//! **Title world (default).** The whole title audio boot runs as original code over this runtime
//! (`AuthoredRuntime::game_boot`) for the world only (`game_world_only`: the player manager's sync
//! is skipped), so the title makes no player sounds (the hand-ported components keep those) while its own managers and components
//! run the world: SFXObj_Reverb with the trigger manager's `.ems` zones, Ambience, Master, the
//! emitters (painter `audio_emitters` sets and `sfx_*` zones), crowds, speakers. It has its own
//! MixMap. This supplies the camera, the painter keys and the followed record (frame `+8`, count
//! 1, with the player manager's sync skipped: `AuthoredRuntime::game_world_only`). `SKATE_AUDIO_TITLE_WORLD=0`, or a failed boot, falls back to the standalone components above.

use skate_audio_core::authored::AuthoredRuntime;
use skate_audio_core::environment::{ReverbFrame, WorldClass};
use skate_data::audio::world::{self, WorldAudio, Zone};

use crate::skate_audio::PlayerAudioObservation;

pub(crate) struct WorldSound {
    assets: std::path::PathBuf,
    /// The title world runs (see the module note); the standalone components are off.
    title: bool,
    /// The title world stopped on an error: the world side is frozen, the player sound goes on.
    title_stopped: bool,
    /// SFXObj_Ambience is running (it failed to boot, or stopped, otherwise).
    ambience: bool,
    /// SFXObj_Master (booted with the ambience) is running.
    master: bool,
    /// The ambience beds installed, and Skate 2's bed loader.
    beds: skate_data::audio::title::Beds,
    /// The last `(state, bed key)` the component reported, to log changes.
    ambience_seen: (u32, u64),
    /// The rain's duck on the ambience bed (weather_audio::duck).
    duck: crate::weather_audio::AmbienceDuck,
    data: WorldAudio,
    classes: Vec<WorldClass>,
    /// The map the runtime's zones are installed for (`None` before the first frame).
    map: Option<String>,
    zones: Vec<Zone>,
    /// Indices into `zones` of the active ones, in the order they became active.
    active: Vec<u32>,
}

impl WorldSound {
    /// Load the exported districts and boot SFXObj_Ambience on `runtime` (which must have its
    /// MixMap loaded). Must run before the first frame: the reverb then shares the game object the
    /// ambience boot builds.
    pub(crate) fn load(runtime: &mut AuthoredRuntime, assets: &std::path::Path) -> Result<Self, String> {
        let data = WorldAudio::load(assets)?;
        let classes = data.reverb_classes();
        let title = std::env::var("SKATE_AUDIO_TITLE_WORLD").map_or(true, |v| v != "0")
            && match boot_title_world(runtime, assets) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("SKATE_PLAYER_AUDIO title_world_failed error={e}; standalone world components");
                    false
                }
            };
        if title {
            skate_data::audio::title::prewarm(assets, &data);
        }
        let ambience = !title
            && match boot_ambience(runtime, assets) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("SKATE_PLAYER_AUDIO ambience_failed error={e}; the world has no ambience");
                    false
                }
            };
        Ok(Self {
            assets: assets.to_path_buf(),
            title,
            title_stopped: false,
            ambience,
            master: ambience,
            beds: Default::default(),
            ambience_seen: (0, 0),
            duck: Default::default(),
            data,
            classes,
            map: None,
            zones: Vec::new(),
            active: Vec::new(),
        })
    }

    pub(crate) fn title(&self) -> bool {
        self.title
    }

    /// Install the level on a map change: its reverb zones, then SFXObj_Reverb's init.
    fn enter(&mut self, runtime: &mut AuthoredRuntime, map: &str, old: bool) -> Result<(), String> {
        self.zones = self
            .data
            .district(map)
            .map(|district| self.data.reverb_zones(district))
            .unwrap_or_default();
        self.active.clear();
        let records: Vec<[u8; 72]> = self.zones.iter().map(Zone::record).collect();
        runtime
            .reverb_install(&self.classes, &records)
            .map_err(|e| format!("reverb install: {e}"))?;
        eprintln!(
            "SKATE_PLAYER_AUDIO world map={map} district={} reverb_zones={}",
            self.data.district(map).is_some(),
            self.zones.len()
        );
        self.map = Some(map.to_owned());
        if self.ambience {
            let started = std::time::Instant::now();
            match self.beds.enter(runtime, &self.data, &self.assets, map, old) {
                Ok(n) => eprintln!(
                    "SKATE_PLAYER_AUDIO ambience_beds map={map} installed={n} total={} in {:.1}s",
                    self.beds.len(),
                    started.elapsed().as_secs_f32()
                ),
                Err(e) => eprintln!("SKATE_PLAYER_AUDIO ambience_beds_failed map={map} error={e}"),
            }
        }
        Ok(())
    }

    /// SFXObj_Reverb's process for this frame; call before the MixMap evaluation.
    pub(crate) fn process(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
        player: [[f32; 4]; 6],
        dt: f32,
    ) -> Result<(), String> {
        if self.title {
            if self.title_stopped {
                return Ok(());
            }
            // A title-world failure costs the world side, not the player sound.
            if let Err(e) = self.title_frame(runtime, observation, player, dt) {
                eprintln!("SKATE_PLAYER_AUDIO title_world_failed error={e}; the world side is stopped");
                self.title_stopped = true;
            }
            return Ok(());
        }
        if self.map.as_deref() != Some(observation.map.as_str()) {
            self.enter(runtime, &observation.map, observation.skate2 || observation.skate1())?;
        }
        let retail = &observation.retail;
        if let Some((_, camera)) = retail.camera {
            let zones = &self.zones;
            self.active.retain(|&i| zones[i as usize].contains(camera));
            for (i, zone) in zones.iter().enumerate() {
                if zone.contains(camera) && !self.active.contains(&(i as u32)) {
                    self.active.push(i as u32);
                }
            }
        }
        let weights: Vec<(u32, f32)> = match retail.camera {
            Some((_, camera)) => self
                .active
                .iter()
                .map(|&i| (i, self.zones[i as usize].weight(camera)))
                .collect(),
            None => Vec::new(),
        };
        let outdoor = self
            .data
            .district(&observation.map)
            .and_then(|district| {
                world::sample(
                    district.layer("audio_reverb"),
                    retail.position[0],
                    retail.position[2],
                )
            })
            .unwrap_or(0);
        if self.master {
            if let Err(e) = runtime.master_process(dt) {
                eprintln!("SKATE_PLAYER_AUDIO master_failed process error={e}; master stopped");
                self.master = false;
            }
        }
        if self.ambience {
            let (old, gain) =
                super::game_side::old_ambience(&mut self.beds, runtime, &self.data, &self.assets, observation);
            let key = super::game_side::painter_key(&self.data, observation, "audio_ambience", old);
            runtime.set_ambience_duck(self.duck.step(super::game_side::ambience_bed(&self.data, key), dt) * gain);
            // An ambience failure costs the ambience, not the player sound.
            if let Err(e) = runtime.ambience_process(key, dt) {
                eprintln!("SKATE_PLAYER_AUDIO ambience_failed process error={e}; ambience stopped");
                self.ambience = false;
            }
        }
        runtime
            .reverb_process(
                &ReverbFrame {
                    outdoor,
                    player,
                    zones: &weights,
                },
                dt,
            )
            .map_err(|e| format!("reverb process: {e}"))
    }

    /// SFXObj_Reverb's and SFXObj_Ambience's update; call after the MixMap evaluation.
    pub(crate) fn update(&mut self, runtime: &mut AuthoredRuntime) -> Result<(), String> {
        // The title's frame ran both halves in `process`.
        if self.title {
            return Ok(());
        }
        if self.master {
            if let Err(e) = runtime.master_update() {
                eprintln!("SKATE_PLAYER_AUDIO master_failed update error={e}; master stopped");
                self.master = false;
            }
        }
        if self.ambience {
            if let Err(e) = runtime.ambience_update() {
                eprintln!("SKATE_PLAYER_AUDIO ambience_failed update error={e}; ambience stopped");
                self.ambience = false;
            } else if let Some(seen) = runtime.ambience_status().filter(|s| *s != self.ambience_seen) {
                self.ambience_seen = seen;
                let bed = self.data.classes.get("aud_wp_ambiences").and_then(|c| {
                    c.field(seen.1, "Hash_4042D2F96C4E4B8C")
                        .and_then(|f| u32::from_str_radix(&f.data, 16).ok())
                });
                // state: 1 playing, 2 fading out, 3 fading in (`sub_824D3FE0`).
                eprintln!(
                    "SKATE_PLAYER_AUDIO ambience state={} painter_key={:#018x} bed={bed:?}",
                    seen.0, seen.1
                );
            }
        }
        runtime
            .reverb_update()
            .map_err(|e| format!("reverb update: {e}"))
    }
}

impl WorldSound {
    /// One title-world frame (both halves, its own MixMap tick): the district and beds on a map
    /// change, then this frame's camera, painter keys and followed record.
    fn title_frame(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
        player: [[f32; 4]; 6],
        dt: f32,
    ) -> Result<(), String> {
        let map = observation.map.as_str();
        if let Some(next) = super::game_side::announced_map() {
            self.beds.prefetch(&self.data, &self.assets, &next);
        }
        if self.map.as_deref() != Some(map) {
            self.map = Some(map.to_owned());
            // The first candidate the vault has; the trigger manager ignores an unknown key.
            if let Some(&key) = super::game_side::district_key(map).first() {
                runtime.game_set_district(key).map_err(|e| e.to_string())?;
            }
            let started = std::time::Instant::now();
            match self.beds.enter(runtime, &self.data, &self.assets, map, observation.skate2 || observation.skate1()) {
                Ok(n) => eprintln!(
                    "SKATE_PLAYER_AUDIO title_world map={map} district={} ambience_beds={n} in {:.1}s",
                    self.data.district(map).is_some(),
                    started.elapsed().as_secs_f32()
                ),
                Err(e) => eprintln!("SKATE_PLAYER_AUDIO ambience_beds_failed map={map} error={e}"),
            }
        }
        let retail = &observation.retail;
        let (ambience, gain) =
            super::game_side::old_ambience(&mut self.beds, runtime, &self.data, &self.assets, observation);
        let g = runtime.guest_mut();
        if let Some((at, position)) = retail.camera {
            super::game_side::write_camera(g, at, position).map_err(|e| e.to_string())?;
        }
        let game = g.u32(skate_audio_core::game::GAME_OBJECT).map_err(|e| e.to_string())?;
        let frame = game + skate_audio_core::game::SKATER_FRAME;
        for (offset, layer) in super::game_side::PAINTER_KEYS {
            let key = super::game_side::painter_key(&self.data, observation, layer, ambience);
            g.set_u64(frame + offset, key).map_err(|e| e.to_string())?;
        }
        // The followed record (`+8`; position, facing, velocity, board position, facing, velocity)
        // as the one skater (`+0xC`; the listener follows it only with a count, `sub_8248CC08`; the
        // player manager's sync is skipped, `game_world_only`), and the time scale (`+0`) at 1.0.
        let record = frame + skate_audio_core::game::SKATER_RECORDS;
        for (i, v) in player.iter().flatten().enumerate() {
            g.set_u32(record + 4 * i as u32, v.to_bits()).map_err(|e| e.to_string())?;
        }
        g.set_u32(frame + 8, record).map_err(|e| e.to_string())?;
        g.set_u32(frame + 0xC, 1).map_err(|e| e.to_string())?;
        g.set_u32(frame, 1.0f32.to_bits()).map_err(|e| e.to_string())?;
        let key = super::game_side::painter_key(&self.data, observation, "audio_ambience", ambience);
        runtime.set_ambience_duck(self.duck.step(super::game_side::ambience_bed(&self.data, key), dt) * gain);
        runtime.game_frame(dt).0.map(|_| ()).map_err(|e| format!("title world frame: {e}"))
    }
}

/// The whole title audio boot over this runtime (see the module note), its outputs wired into the
/// reverb network.
fn boot_title_world(runtime: &mut AuthoredRuntime, assets: &std::path::Path) -> Result<(), String> {
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).map_err(|e| format!("{name}: {e}"));
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt")?,
        &read("skaterschema.bin")?,
        &read("skatercollections.vlt")?,
        &read("skatercollections.bin")?,
    )
    .map_err(|e| e.to_string())?;
    let files = skate_data::audio::title::files(skate_data::audio::title::roots(assets));
    let started = std::time::Instant::now();
    // The modulators first, as retail's bus manager builds them before SFXObj_Reverb's init loads
    // the output wobble into records 10..13 (after the boot, the rebuild wiped it).
    runtime.wire_environment().map_err(|e| e.to_string())?;
    let root = runtime.game_boot(vault, files).0.map_err(|e| format!("title boot: {e}"))?;
    runtime.game_world_only();
    eprintln!(
        "SKATE_PLAYER_AUDIO title_world booted root={root:#010x} in {:.1}s",
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Boot SFXObj_Ambience over the title's own files and Attrib vault.
fn boot_ambience(runtime: &mut AuthoredRuntime, assets: &std::path::Path) -> Result<(), String> {
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).map_err(|e| format!("{name}: {e}"));
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt")?,
        &read("skaterschema.bin")?,
        &read("skatercollections.vlt")?,
        &read("skatercollections.bin")?,
    )
    .map_err(|e| e.to_string())?;
    let files = skate_data::audio::title::files(skate_data::audio::title::roots(assets));
    let started = std::time::Instant::now();
    let (booted, _) = runtime.ambience_boot(vault, files);
    let component = booted.map_err(|e| format!("ambience boot: {e}"))?;
    eprintln!(
        "SKATE_PLAYER_AUDIO ambience booted component={component:#010x} in {:.2}s",
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
