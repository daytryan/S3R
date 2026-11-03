//! The retail audio game side run as original code (`skate_audio_core::game`, AUDIO_HANDOFF
//! §9.4): the title's own boot, managers, states, components and MixMap, driven by what the game
//! thread would hand it each frame — the local skater's PhysOut bundle (from which the title's
//! record builder `sub_827A1B78` builds the skater record) and the camera. Replaces the
//! hand-ported bridge and components unless `SKATE_AUDIO_GAME_SIDE=0`.

use skate_audio_core::authored::AuthoredRuntime;
use skate_audio_core::game::Skater;
use skate_audio_core::Guest;

use super::audio_state::{AudioState, ConditionerOutput};
use super::sound::FRAME_SECONDS;
use super::tuning::AudioTuning;
use crate::skate_audio::{PlayerAudioObservation, RetailAudioInputs};

/// Whether the worker runs the game side instead of the hand-ported components: on unless
/// `SKATE_AUDIO_GAME_SIDE=0` (the worker falls back to the ported path if it fails).
///
/// On by default since 2026-09-30: it is the only path whose one-shots (pops, landings, truck
/// touches, collisions) are the original Splice voices — the hand port reads the member's pitch
/// words as its gain and trims the result by measured guesses, which put its pops ~10 dB and its
/// landing tails far above the original's. Its old "crunchy" defect was a freed `Sub0` left on the
/// accumulator list (`modules::destroy_submix`); the 2026-09-29 pitch/send problems were fixed in
/// session 8.
pub(crate) fn enabled() -> bool {
    std::env::var("SKATE_AUDIO_GAME_SIDE").map_or(true, |v| v != "0")
}

/// The map the map transition is loading ([`announce_map`]), taken by the audio worker's next frame.
static NEXT_MAP: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Names the map being loaded, so the audio worker's loader decodes its district's ambience beds while it loads
/// (`Beds::prefetch`) and the map change only installs them.
pub(crate) fn announce_map(name: &str) {
    if let Ok(mut next) = NEXT_MAP.lock() {
        *next = Some(name.to_owned());
    }
}

/// The map [`announce_map`] named since the last call.
pub(crate) fn announced_map() -> Option<String> {
    NEXT_MAP.lock().ok().and_then(|mut next| next.take())
}

pub(crate) struct GameSide {
    skater: Skater,
    /// Runs the PhysOut audio conditioner port (`sub_82772748`) whose output is slot B+40.
    audio: AudioState,
    tuning: AudioTuning,
    tick: u64,
    /// `SKATE_AUDIO_SPLICE_TRACE`: the Splice voices seen last frame, with (sample, peak gain).
    splice_seen: std::collections::HashMap<u32, (u16, f32)>,
    /// `SKATE_AUDIO_STATE_DIFF`: the retail bridge's audio state in guest memory, once found.
    state_at: Option<u32>,
    /// The painter layers (`world-audio.json`); `None` without it.
    world: Option<skate_data::audio::world::WorldAudio>,
    /// The map whose district the game side was last told about.
    map: Option<String>,
    assets: std::path::PathBuf,
    /// The ambience beds installed, and Skate 2's bed loader.
    beds: skate_data::audio::title::Beds,
    /// `SKATE_AUDIO_CONTACT_HOLD`: frames each wheel stays in contact after the physics drops it.
    held: [(u32, [f32; 3], u32, u32); 4],
    /// The boot's FE audio state for the sound root `+1196` ([`crate::skate_audio::fe_audio_state`]); `None`: the
    /// root keeps its own value.
    pub(crate) fe_state: Option<u32>,
    /// The root's `+1196` from before the boot drove it, put back when it stops.
    root_state: Option<u32>,
    /// The rain's duck on the ambience bed (weather_audio::duck).
    duck: crate::weather_audio::AmbienceDuck,
    /// This frame's local record as the builder wrote it, what goes to the other players ([`super::remote`]).
    record: skate_net::audio::Record,
    /// The folder of Skate 1's or 2's traffic banks the title's files serve in place of its own (None: Skate 3's).
    traffic_set: skate_data::audio::title::TrafficSet,
    /// The traffic banks loaded (`Some(set)`), or None before the first traffic map.
    traffic_banks: Option<Option<std::path::PathBuf>>,
    /// This machine's riders as the title hears them, one guest skater each (round2 T5, bug 11; [`RiderGuest`]).
    riders: Vec<RiderGuest>,
}

/// A rider on this machine (physics/riders.rs: the Skate 2 tutorial's AI) as the title hears it: a guest skater of its
/// own (`game_skater`: player object, PhysOut bundle, record builder `sub_827A1B78` with its two Attrib instances) and
/// its own PhysOut audio conditioner (`sub_82772748`), so the player's builder and conditioner state stay untouched.
/// Retail builds a record per skater in its player list (`sub_827A11B0`) from each one's own bundle.
struct RiderGuest {
    key: String,
    skater: Skater,
    audio: AudioState,
}

/// One of the frame's other skaters (records 1..3): `remote_records[i]` (online) or `riders[i]` (this machine).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Remote {
    Online(usize),
    Rider(usize),
}

/// Which other skaters the frame carries: the online ones (their record's `+0` position; remote.rs already keeps the
/// nearest three) and this machine's riders (their inputs' position, SystemReckoning+64), nearest `me` first (a tie
/// keeps the online skater), at most [`super::remote::REMOTE_SLOTS`]. INFERRED: retail's list builder `sub_827A11B0`
/// takes skaters in player-list order up to its cap of four; nearest-first is remote.rs's own choice, kept here.
pub(crate) fn nearest_remotes(me: [f32; 3], online: &[[f32; 3]], riders: &[[f32; 3]]) -> Vec<Remote> {
    let distance = |p: &[f32; 3]| (0..3).map(|i| (p[i] - me[i]) * (p[i] - me[i])).sum::<f32>();
    let mut all: Vec<(f32, Remote)> = online
        .iter()
        .enumerate()
        .map(|(i, p)| (distance(p), Remote::Online(i)))
        .chain(riders.iter().enumerate().map(|(i, p)| (distance(p), Remote::Rider(i))))
        .collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    all.into_iter().take(super::remote::REMOTE_SLOTS).map(|(_, remote)| remote).collect()
}

/// The controller nibble a rider's record plays with (`+152` bits 16..19, which key its player state in
/// `sub_824F1FB0`): its guest's own (guest k: 1 + k mod 3) unless a skater already placed this frame holds it, then the
/// next free one. INFERRED: retail's AI carry their own controller index (player vfunc 11); the port's guests answer 0,
/// so the nibble is chosen here as online slots choose theirs (remote.rs: slot k plays nibble k + 1).
pub(crate) fn rider_nibble(taken: &[u8], guest: usize) -> u8 {
    let slots = super::remote::REMOTE_SLOTS;
    (0..slots).map(|i| ((guest + i) % slots) as u8 + 1).find(|n| !taken.contains(n)).unwrap_or(1)
}

/// Record 0 of the frame (`+0xF0`), as the builder just wrote it.
fn first_record(g: &Guest) -> Result<skate_net::audio::Record, skate_audio_core::Error> {
    use skate_audio_core::game::{GAME_OBJECT, SKATER_FRAME, SKATER_RECORDS};
    let at = g.u32(GAME_OBJECT)? + SKATER_FRAME + SKATER_RECORDS;
    let mut record = [0; skate_net::audio::RECORD_WORDS];
    for (i, word) in record.iter_mut().enumerate() {
        *word = g.u32(at + 4 * i as u32)?;
    }
    Ok(record)
}

fn record_position(record: &skate_net::audio::Record) -> [f32; 3] {
    [0, 1, 2].map(|i| f32::from_bits(record[i]))
}

/// The traffic banks to (re)load this frame: the set `map` plays, and whether another set goes first. Nothing without
/// cars (online, replays, the menus: [`crate::traffic::Traffic::voices`]) or with `map`'s set already loaded.
pub(crate) fn traffic_banks_wanted(
    loaded: &Option<Option<std::path::PathBuf>>,
    assets: &std::path::Path,
    observation: &PlayerAudioObservation,
) -> Option<(Option<std::path::PathBuf>, bool)> {
    observation.traffic.as_ref()?;
    let want = skate_data::audio::title::traffic_banks(assets, &observation.map);
    (loaded.as_ref() != Some(&want)).then(|| (want, loaded.is_some()))
}

/// The other skaters' records after ours (item 30b), as the player-list builder `sub_827A11B0` lays out the frame:
/// record k at `+0xF0 + 0x220 k` (k = 1..3, the 4-record cap at 0x827A1220), count `1 + n`. Each is the sender's own
/// local record made remote as `sub_827A1B78` builds one with `local` = 0 (0x827A1C00..1C34, 0x827A2A00): `+152` bit
/// 31 (local) and bit 0x80 (OffBoard +304, read only for the local bundle) cleared, bits 16..19 the controller nibble,
/// which keys its player state in `sub_824F1FB0`.
///
/// Spectating, the watched skater is the local one and ours a remote ([`watch`]). A remote is heard from the camera:
/// `sub_824F1FB0` drops it past 30 m of listener pos0 (`sub_824F8EF8`).
fn write_remotes(g: &mut Guest, remotes: &[(u8, Box<skate_net::audio::Record>)]) -> Result<(), skate_audio_core::Error> {
    use skate_audio_core::game::{GAME_OBJECT, SKATER_FRAME, SKATER_RECORD_BYTES, SKATER_RECORDS};
    let frame = g.u32(GAME_OBJECT)? + SKATER_FRAME;
    let remotes = &remotes[..remotes.len().min(super::remote::REMOTE_SLOTS)];
    for (k, (nibble, record)) in remotes.iter().enumerate() {
        let at = frame + SKATER_RECORDS + SKATER_RECORD_BYTES * (k as u32 + 1);
        for (i, &word) in record.iter().enumerate() {
            let word = if i == 152 / 4 { (word & !(0x8000_0000 | 0x000F_0000 | 0x80)) | (u32::from(nibble & 0xF) << 16) } else { word };
            g.set_u32(at + 4 * i as u32, word)?;
        }
    }
    if !remotes.is_empty() {
        g.set_u32(frame + 0xC, 1 + remotes.len() as u32)?;
    }
    Ok(())
}

/// Spectating (user 10-30, tu3), as retail does with the camera on another player (`SwitchCamera` and the spectator UI
/// set the player list's local id, vfunc 0x28 `sub_82598C38`): `sub_827A11B0` builds the watched player's record as the
/// local one (`local` = 1: bit 31, OffBoard 0x80, both kept from its sender's) and ours as a remote. It goes in record
/// 0, which `+8` names (the record with bit 31, 0x827A514C), on its slot's nibble; ours keeps its own nibble. The record
/// sent to the others stays ours (`GameSide::record`). UNTRACED: the builder's other local-only header words.
fn watch(
    g: &mut Guest,
    ours: &skate_net::audio::Record,
    remotes: &[(u8, Box<skate_net::audio::Record>)],
    watched: usize,
) -> Result<(), skate_audio_core::Error> {
    use skate_audio_core::game::{GAME_OBJECT, SKATER_FRAME, SKATER_RECORDS};
    let at = g.u32(GAME_OBJECT)? + SKATER_FRAME + SKATER_RECORDS;
    let (nibble, record) = &remotes[watched];
    for (i, &word) in record.iter().enumerate() {
        let word = if i == 152 / 4 { (word & !0x000F_0000) | 0x8000_0000 | (u32::from(nibble & 0xF) << 16) } else { word };
        g.set_u32(at + 4 * i as u32, word)?;
    }
    let ours = ((ours[152 / 4] >> 16 & 0xF) as u8, Box::new(*ours));
    let others: Vec<_> =
        std::iter::once(ours).chain(remotes.iter().enumerate().filter(|(k, _)| *k != watched).map(|(_, r)| r.clone())).collect();
    write_remotes(g, &others)
}

/// Spectating while the watched skater plays no record (a switch: its new slot sits a frame out, or primes; a stall
/// past its hold; muted): record 0's `+0` position, `+16` facing and `+32` velocity, which the listener's
/// pos1/dir1/vel1 follow (`sub_8248CC08`), become its, so the world is heard from it and not from our parked skater.
fn listen_from(g: &mut Guest, watched: &skate_net::audio::Record) -> Result<(), skate_audio_core::Error> {
    use skate_audio_core::game::{GAME_OBJECT, SKATER_FRAME, SKATER_RECORDS};
    let at = g.u32(GAME_OBJECT)? + SKATER_FRAME + SKATER_RECORDS;
    for (i, &word) in watched[..48 / 4].iter().enumerate() {
        g.set_u32(at + 4 * i as u32, word)?;
    }
    Ok(())
}

/// The name of the bed an `audio_ambience` key plays (its `aud_wp_ambiences` bed number), for the rain's duck.
pub(crate) fn ambience_bed(world: &skate_data::audio::world::WorldAudio, key: u64) -> &'static str {
    let bed = world.classes.get("aud_wp_ambiences").and_then(|c| {
        c.field(key, "Hash_4042D2F96C4E4B8C").and_then(|f| u32::from_str_radix(&f.data, 16).ok())
    });
    crate::weather_audio::bed_name(bed.unwrap_or(0))
}

/// The sound root's FE audio state (`[0x830CFDC4]+1196`, `Sk8::Audio::eAudioFEStates`), written by the
/// cMsgFEAudioStateChange handler 0x8248AA10. SFXObj_Ambience's key selector 0x824D3E40 plays the title bed
/// (`aud_wp_ambiences` 0x5EADF1818A721AF9, bed 23 `23_Press_start_screen`, fades 1.0 s) while it is 10 and the
/// root's `+1204` is still set (1 from the root ctor 0x824845B8, cleared when `titlescreen_start` plays).
const ROOT_FE_STATE: u32 = 1196;

/// The `district_audio` collection a map's district uses (as `tools/asset_pipeline/audio_world.py`
/// pairs them): three parks by their `.ems`, the rest `dist_<name>` or `dist_<snake_name>`.
pub(crate) fn district_key(map: &str) -> Vec<u64> {
    use skate_data::audio::world::hash64;
    match map {
        "MegaPark" => return vec![0x3016_BE7C_6B34_E27A],
        "StartPark" => return vec![0x5ACC_9E68_DA54_9C74],
        "DownTownSkatePark" => return vec![hash64("dist_downtown_skatepark")],
        "MaloofMoneyCup" => return vec![hash64("dist_maloof_money_cup")],
        _ => {}
    }
    let mut snake = String::new();
    for (i, c) in map.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            snake.push('_');
        }
        snake.push(c.to_ascii_lowercase());
    }
    vec![hash64(&format!("dist_{}", map.to_ascii_lowercase())), hash64(&format!("dist_{snake}"))]
}

/// The frame's painter keys (`sub_827A2E88`'s lookups at the skater's (x, z)): `+0x40`
/// `audio_emitters`, `+0x48` `audio_ambience`, `+0x50` `audio_reverb`; 0 where unpainted.
pub(crate) const PAINTER_KEYS: [(u32, &str); 3] =
    [(0x40, "audio_emitters"), (0x48, "audio_ambience"), (0x50, "audio_reverb")];

/// The painter key of `layer` at the player: the map district's painted layer. Skate 1's and 2's maps
/// have no Skate 3 painter: their ambience is [`old_ambience`]'s, the rest unpainted.
pub(crate) fn painter_key(
    world: &skate_data::audio::world::WorldAudio,
    observation: &PlayerAudioObservation,
    layer: &str,
    old_ambience: u64,
) -> u64 {
    use skate_data::audio::world;
    if observation.skate2 || observation.skate1() {
        return if layer == "audio_ambience" { old_ambience } else { 0 };
    }
    let [x, _, z] = observation.heard_at();
    world.district(&observation.map).and_then(|d| world::sample(d.layer(layer), x, z)).unwrap_or(0)
}

/// On the Skate 1 or 2 city, this frame's ambience key and the gain on its controller: the Skate 3 row
/// playing the old bed that game's painter names at the player (`Beds::old_game_key`, at the old row's
/// volume), once that bed is installed; (0, 1) elsewhere, Skate 2's parks included (their own districts).
pub(crate) fn old_ambience(
    beds: &mut skate_data::audio::title::Beds,
    runtime: &mut AuthoredRuntime,
    world: &skate_data::audio::world::WorldAudio,
    assets: &std::path::Path,
    observation: &PlayerAudioObservation,
) -> (u64, f32) {
    let Some(game) = skate_data::audio::title::OldGame::of(&observation.map) else { return (0, 1.) };
    let [x, _, z] = observation.heard_at();
    beds.old_game_key(runtime, world, assets, game, x, z)
}

/// The game object's camera block (`+0x2FD20`, read by `sub_827A0C70`): its highest-priority
/// override, a world matrix (rows right, up, at, position) at `+0x1230` enabled by byte `+0x1220`.
const CAMERA: u32 = 0x2_FD20;
const CAMERA_OVERRIDE: u32 = 0x1230;
const CAMERA_OVERRIDE_ON: u32 = 0x1220;

impl GameSide {
    /// Boot the game side on `runtime` (created without banks or projects: the title loads its
    /// own) from the installation's staged data.
    pub(crate) fn build(runtime: &mut AuthoredRuntime, assets: &std::path::Path) -> Result<Self, String> {
        Self::build_cancellable(runtime, assets, &|| false)
    }
    /// [`Self::build`], asking `cancelled` at each load point (the replay export's boot, owner 10-18).
    pub(crate) fn build_cancellable(
        runtime: &mut AuthoredRuntime,
        assets: &std::path::Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let stop = || if cancelled() { Err(String::from(super::CANCELLED)) } else { Ok(()) };
        let db = assets.join("private/stock/data/db");
        let read = |name: &str| std::fs::read(db.join(name)).map_err(|e| format!("{name}: {e}"));
        let vault = skate_audio_core::attrib::Vault::parse(
            &read("skaterschema.vlt")?,
            &read("skaterschema.bin")?,
            &read("skatercollections.vlt")?,
            &read("skatercollections.bin")?,
        )
        .map_err(|e| e.to_string())?;
        let traffic_set = skate_data::audio::title::TrafficSet::default();
        let files = skate_data::audio::title::with_traffic(
            skate_data::audio::title::files(skate_data::audio::title::roots(assets)),
            assets,
            traffic_set.clone(),
        );
        let started = std::time::Instant::now();
        // The reverb network's wiring and modulators first, as retail's bus manager does before
        // SFXObj_Reverb's init (without it the outputs stayed on the null target: no reverb at all).
        runtime.wire_environment().map_err(|e| e.to_string())?;
        stop()?;
        let (booted, _) = runtime.game_boot(vault, files);
        let root = booted.map_err(|e| format!("audio game side boot: {e}"))?;
        eprintln!(
            "SKATE_PLAYER_AUDIO game_side booted root={root:#010x} in {:.1}s",
            started.elapsed().as_secs_f32()
        );
        stop()?;
        let skater = runtime.game_skater().map_err(|e| e.to_string())?;
        let world = skate_data::audio::world::WorldAudio::load(assets)
            .map_err(|e| eprintln!("SKATE_PLAYER_AUDIO world_audio_missing error={e}"))
            .ok();
        if let Some(world) = &world {
            skate_data::audio::title::prewarm(assets, world);
        }
        stop()?;
        let collections = skate_data::collections::Collections::shared(assets)?; // perf 10-18
        let tuning = AudioTuning::from_collections(&collections)?;
        let mut side = Self { skater, audio: AudioState::default(), tuning,
            tick: 0,
            splice_seen: Default::default(),
            state_at: None,
            world,
            map: None,
            assets: assets.to_path_buf(),
            beds: Default::default(),
            held: Default::default(),
            fe_state: None,
            root_state: None,
            duck: Default::default(),
            record: [0; skate_net::audio::RECORD_WORDS],
            traffic_set,
            traffic_banks: None,
            riders: Vec::new(),
        };
        // Bed 23 (the title's, `Beds::enter` with no map) now, before the worker reports ready: installed on the
        // first frame it read both ambience archives whole and decoded the bed there, late for the ESRB's fade-in.
        let started = std::time::Instant::now();
        match side.install_beds(runtime, "", false) {
            Ok(n) => eprintln!("SKATE_PLAYER_AUDIO game_side title_bed={n} in {:.2}s", started.elapsed().as_secs_f32()),
            Err(e) => eprintln!("SKATE_PLAYER_AUDIO game_side title_bed_failed error={e}"),
        }
        Ok(side)
    }

    pub(crate) fn audio(&self) -> &AudioState {
        &self.audio
    }

    /// The local record of the last frame (`sub_827A1B78`'s, before the other skaters were added).
    pub(crate) fn local_record(&self) -> skate_net::audio::Record {
        self.record
    }

    /// The boot's FE audio state onto the sound root (`+1196`), as 0x8248AA10 stores it; the root's own value
    /// comes back once the boot lets go.
    fn write_fe_state(&mut self, g: &mut Guest) -> Result<(), skate_audio_core::Error> {
        let at = g.u32(skate_audio_core::game::ROOT_GLOBAL)? + ROOT_FE_STATE;
        match (self.fe_state, self.root_state) {
            (Some(state), saved) => {
                self.root_state = Some(match saved {
                    Some(saved) => saved,
                    None => g.u32(at)?,
                });
                g.set_u32(at, state)
            }
            (None, Some(saved)) => {
                self.root_state = None;
                g.set_u32(at, saved)
            }
            (None, None) => Ok(()),
        }
    }

    /// One 60 Hz game frame; `cutscene`: a cutscene, not a menu, holds a paused frame.
    pub(crate) fn frame(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
        cutscene: bool,
    ) -> Result<(), String> {
        let mut held_inputs;
        let mut retail = &observation.retail;
        // ponytail: our physics drops wheel contacts for a few frames where retail's stays down, and
        // every drop restarts the rolling sounds; hold each contact briefly (off unless set).
        let hold = std::env::var("SKATE_AUDIO_CONTACT_HOLD").ok().and_then(|v| v.parse::<u32>().ok());
        if let Some(hold) = hold.filter(|h| *h > 0) {
            held_inputs = observation.retail.clone();
            for i in 0..4 {
                if held_inputs.wheel_contacts[i] {
                    self.held[i] = (
                        hold,
                        held_inputs.wheel_contact_normals[i],
                        held_inputs.wheel_audio_surfaces[i],
                        held_inputs.wheel_seam_patterns[i],
                    );
                } else if self.held[i].0 > 0 {
                    self.held[i].0 -= 1;
                    held_inputs.wheel_contacts[i] = true;
                    held_inputs.wheel_contact_normals[i] = self.held[i].1;
                    held_inputs.wheel_audio_surfaces[i] = self.held[i].2;
                    held_inputs.wheel_seam_patterns[i] = self.held[i].3;
                }
            }
            held_inputs.wheel_count = held_inputs.wheel_contacts.iter().filter(|c| **c).count() as u32;
            retail = &held_inputs;
        }
        if std::env::var_os("SKATE_AUDIO_INPUTS").is_some() {
            eprintln!(
                "SKATE_AUDIO_INPUTS tick={} count={} contacts={:?} trucks={:?} deck={} surfaces={:?} seams={:?} speed={:.3}",
                self.tick,
                retail.wheel_count,
                retail.wheel_contacts.map(u8::from),
                retail.truck_contacts.map(u8::from),
                u8::from(retail.deck_contact),
                retail.wheel_audio_surfaces,
                retail.wheel_seam_patterns,
                retail.ground_speed,
            );
        }
        super::trace::frame(self.tick);
        self.tick += 1;
        self.audio.update(retail, &self.tuning);
        let conditioner = *self.audio.conditioner();
        let (ambience, gain) = match &self.world {
            Some(world) => old_ambience(&mut self.beds, runtime, world, &self.assets, observation),
            None => (0, 1.),
        };
        let g = runtime.guest_mut();
        write_physout(g, &self.skater.slots, retail, &conditioner, self.tuning.body_impact_scale)
            .map_err(|e| e.to_string())?;
        if let Some((at, position)) = retail.camera {
            write_camera(g, at, position).map_err(|e| e.to_string())?;
        }
        if let Some(world) = &self.world {
            let game = g.u32(skate_audio_core::game::GAME_OBJECT).map_err(|e| e.to_string())?;
            let frame = game + skate_audio_core::game::SKATER_FRAME;
            for (offset, layer) in PAINTER_KEYS {
                g.set_u64(frame + offset, painter_key(world, observation, layer, ambience)).map_err(|e| e.to_string())?;
            }
        }
        if let (Some(next), Some(world)) = (announced_map(), &self.world) {
            self.beds.prefetch(world, &self.assets, &next);
        }
        if self.map.as_deref() != Some(observation.map.as_str()) {
            self.map = Some(observation.map.clone());
            let keys = district_key(&observation.map);
            // The first candidate the vault has; the trigger manager ignores an unknown key.
            if let Some(&key) = keys.first() {
                runtime.game_set_district(key).map_err(|e| e.to_string())?;
                eprintln!("SKATE_PLAYER_AUDIO game_side district map={} key={key:#018x}", observation.map);
            }
            let started = std::time::Instant::now();
            match self.install_beds(runtime, &observation.map, observation.skate2 || observation.skate1()) {
                Ok(n) => eprintln!(
                    "SKATE_PLAYER_AUDIO game_side ambience_beds={n} in {:.1}s",
                    started.elapsed().as_secs_f32()
                ),
                Err(e) => eprintln!("SKATE_PLAYER_AUDIO game_side ambience_beds_failed error={e}"),
            }
        }

        if let Some((set, reload)) = traffic_banks_wanted(&self.traffic_banks, &self.assets, observation) {
            if let Ok(mut serving) = self.traffic_set.lock() {
                *serving = set.clone();
            }
            let started = std::time::Instant::now();
            // ponytail: one try per set; a failed load leaves the cars silent rather than stalling every frame.
            match runtime.game_traffic_banks(reload) {
                Ok(state) => eprintln!(
                    "SKATE_PLAYER_AUDIO game_side traffic_banks={set:?} state={state} in {:.2}s",
                    started.elapsed().as_secs_f32()
                ),
                Err(e) => eprintln!("SKATE_PLAYER_AUDIO game_side traffic_banks_failed set={set:?} error={e}"),
            }
            self.traffic_banks = Some(set);
        }
        runtime.game_traffic(observation.traffic.as_deref().unwrap_or(&[])).map_err(|e| e.to_string())?;
        runtime.game_dmos(&observation.dmos).map_err(|e| e.to_string())?;
        self.write_fe_state(runtime.guest_mut()).map_err(|e| e.to_string())?;
        // Round2 T5 (bug 11): the riders' records first, each built as the frame's only skater and read back; the
        // player's is built last, so the frame's local record (`+8`, the listener) is the player's.
        let others = self.rider_records(runtime, observation)?;
        runtime.game_publish(&self.skater).map_err(|e| e.to_string())?;
        self.record = first_record(runtime.guest()).map_err(|e| e.to_string())?;
        let remotes = others.as_deref().unwrap_or(&observation.remote_records[..]);
        // The watched record is the one `next_records` played this frame (`RemoteAudio::watched_record`), word for word.
        match observation.watched.as_ref().and_then(|w| remotes.iter().position(|(_, r)| r == w)) {
            Some(i) => watch(runtime.guest_mut(), &self.record, remotes, i),
            None => write_remotes(runtime.guest_mut(), remotes)
                .and_then(|()| observation.watched.as_ref().map_or(Ok(()), |w| listen_from(runtime.guest_mut(), w))),
        }
        .map_err(|e| e.to_string())?;
        // The UI's `GlobalFEPlaySound` calls (session marker, score multiplier, menus), as the
        // original code plays them: their `sk8_menu` voices start in this frame's Splice pass.
        for &id in &observation.frontend_sounds {
            if let Err(error) = runtime.game_fe_sound(id) {
                eprintln!("SKATE_PLAYER_AUDIO frontend_sound_failed id={id:#018x} error={error}");
            }
        }
        // Menus, cutscenes and the editor duck the player through SFXObj_Pause, as retail's pause
        // does; a menu's duck takes the world's ambience too, a cutscene's leaves it playing.
        runtime.game_set_paused(observation.retail.paused, cutscene);
        runtime.set_volumes(observation.volumes).map_err(|e| e.to_string())?;
        let bed = self.world.as_ref().map_or("", |world| ambience_bed(world, painter_key(world, observation, "audio_ambience", ambience)));
        runtime.set_ambience_duck(self.duck.step(bed, FRAME_SECONDS) * gain);
        let started = std::time::Instant::now();
        let (done, trace) = runtime.game_frame(FRAME_SECONDS);
        done.map_err(|e| format!("audio game side frame: {e}"))?;
        if std::env::var_os("SKATE_AUDIO_SPLICE_TRACE").is_some() {
            self.trace_splice(runtime.guest());
        }
        // Grain voice starts (drained every frame so the log does not grow).
        for start in runtime.take_grain_starts() {
            if std::env::var_os("SKATE_AUDIO_OBSERVE").is_some() {
                eprintln!(
                    "SKATE_PLAYER_AUDIO grain_start stream={:#010x} seconds={:.3} frame={}",
                    start.stream, start.seconds, start.frame
                );
            }
        }
        if std::env::var_os("SKATE_AUDIO_PROFILE").is_some() && self.tick % 60 == 0 {
            let mut top: Vec<(u32, u64)> = trace.calls.iter().map(|(a, n)| (*a, *n)).collect();
            top.sort_by_key(|t| std::cmp::Reverse(t.1));
            eprintln!(
                "SKATE_PLAYER_AUDIO profile frame_ms={:.2} calls={} top={:X?}",
                started.elapsed().as_secs_f64() * 1000.0,
                trace.calls.values().sum::<u64>(),
                &top[..top.len().min(12)]
            );
        }
        if self.tick == 120 && std::env::var_os("SKATE_AUDIO_FILES").is_some() {
            for (path, at) in runtime.game_files() {
                eprintln!("SKATE_PLAYER_AUDIO game_file {at:#010x} {path}");
            }
        }
        if std::env::var_os("SKATE_AUDIO_STATE_DIFF").is_some() {
            self.diff_state(runtime.guest(), retail);
        }
        Ok(())
    }

    /// The frame's other skaters with this machine's riders among them, or None without riders (the online records go
    /// as they came). Of the nearest ([`nearest_remotes`]), each rider's record is built by the title's own builder
    /// `sub_827A1B78` on its own guest skater as the frame's only skater (`Skater::publish`, local = 1: its PhysOut
    /// bundle written as the player's is, its conditioner stepped) and read back; [`write_remotes`] then makes it
    /// remote as it does a received one. INFERRED (as for online records): a local = 1 build with the bits
    /// `write_remotes` clears equals the builder's local = 0 build (0x827A1C00..1C34, 0x827A2A00 are where `local`
    /// enters). UNTRACED: whether the builder (or its callee 0x82B72420 on 0x830D0850) keeps state outside the record,
    /// the builder and player object; the player's build runs last either way.
    /// ponytail: only the chosen riders' conditioners step (a rider out of range resumes from its last heard frame);
    /// step every rider's if a returning rider is heard to click.
    fn rider_records(
        &mut self,
        runtime: &mut AuthoredRuntime,
        observation: &PlayerAudioObservation,
    ) -> Result<Option<Vec<(u8, Box<skate_net::audio::Record>)>>, String> {
        if observation.riders.is_empty() {
            return Ok(None);
        }
        let online: Vec<[f32; 3]> = observation.remote_records.iter().map(|(_, r)| record_position(r)).collect();
        let positions: Vec<[f32; 3]> = observation.riders.iter().map(|(_, inputs)| inputs.position).collect();
        let picks = nearest_remotes(observation.retail.position, &online, &positions);
        let mut taken: Vec<u8> = picks
            .iter()
            .filter_map(|pick| match *pick {
                Remote::Online(i) => Some(observation.remote_records[i].0),
                Remote::Rider(_) => None,
            })
            .collect();
        let mut others = Vec::with_capacity(picks.len());
        for pick in picks {
            let i = match pick {
                Remote::Online(i) => {
                    others.push(observation.remote_records[i].clone());
                    continue;
                }
                Remote::Rider(i) => i,
            };
            let (key, inputs) = &observation.riders[i];
            let guest = self.rider_guest(runtime, key, &observation.riders)?;
            let rider = &mut self.riders[guest];
            rider.audio.update(inputs, &self.tuning);
            write_physout(runtime.guest_mut(), &rider.skater.slots, inputs, rider.audio.conditioner(), self.tuning.body_impact_scale)
                .map_err(|e| e.to_string())?;
            runtime.game_publish(&rider.skater).map_err(|e| e.to_string())?;
            let nibble = rider_nibble(&taken, guest);
            taken.push(nibble);
            others.push((nibble, Box::new(first_record(runtime.guest()).map_err(|e| e.to_string())?)));
        }
        Ok(Some(others))
    }

    /// The guest skater playing rider `key`: its own, else one whose rider has gone (guest memory is never freed), else
    /// a new one (`game_skater`; `Skater::new` only allocates on the booted title, so any number live beside the
    /// player's).
    /// ponytail: a reused guest keeps its builder's and player object's words from its last rider (a frame of carry-over
    /// at most); a fresh guest per rider if that is ever heard.
    fn rider_guest(
        &mut self,
        runtime: &mut AuthoredRuntime,
        key: &str,
        riders: &[(String, RetailAudioInputs)],
    ) -> Result<usize, String> {
        if let Some(at) = self.riders.iter().position(|rider| rider.key == key) {
            return Ok(at);
        }
        if let Some(at) = self.riders.iter().position(|rider| !riders.iter().any(|(k, _)| *k == rider.key)) {
            let rider = &mut self.riders[at];
            rider.key = key.to_owned();
            rider.audio = AudioState::default();
            return Ok(at);
        }
        let skater = runtime.game_skater().map_err(|e| e.to_string())?;
        self.riders.push(RiderGuest { key: key.to_owned(), skater, audio: AudioState::default() });
        Ok(self.riders.len() - 1)
    }

    /// `SKATE_AUDIO_SPLICE_TRACE`: the Splice (one-shot) voices in the title's active table
    /// (`[0x83084348] + 0x5A8`, 60 × 36 bytes): at start the member's sample index, gain word
    /// (`+4`), the voice's random gain (`+0x44`) and pitch (`+0x40`); when it leaves the table the
    /// largest gain it posted to its `Gai0` (`+0x30`).
    fn trace_splice(&mut self, g: &Guest) {
        let Ok(splice) = g.u32(0x8308_4348) else { return };
        if splice == 0 {
            return;
        }
        let mut live = std::collections::HashMap::new();
        for i in 0..60u32 {
            let voice = g.u32(splice + 0x5A8 + 0x24 * i + 4).unwrap_or(0);
            if voice == 0 {
                continue;
            }
            let gain = g.f32(voice + 0x30).unwrap_or(0.0);
            let member = g.u32(voice + 0x58).unwrap_or(0);
            let sample = g.u16(member).unwrap_or(0);
            let entry = self.splice_seen.get(&voice).copied();
            if entry.is_none() {
                // The bank slot (`+0xE24 + 20·s`: `+8` header, `+0xC` records) holding the record.
                let container = g.u32(splice + 0x5A8 + 0x24 * i).unwrap_or(0);
                let record = g.u32(container + 0x54).unwrap_or(0);
                let bank = (0..8u32)
                    .filter_map(|s| {
                        let at = splice + 0xE24 + 20 * s;
                        let header = g.u32(at + 8).ok().filter(|h| *h != 0)?;
                        let records = g.u32(at + 12).ok()?;
                        (records <= record && record < g.u32(at).ok()?).then(|| {
                            let name: String = (0..24)
                                .map(|k| g.u8(header + 28 + k).unwrap_or(0))
                                .take_while(|&b| b != 0)
                                .map(char::from)
                                .collect();
                            format!("{name}#{}", (record - records) / 36)
                        })
                    })
                    .next()
                    .unwrap_or_default();
                eprintln!(
                    "SKATE_SPLICE start tick={} voice={voice:08x} {bank} sample={sample} m4={} v44={} v40={} gain={gain}",
                    self.tick,
                    g.f32(member + 4).unwrap_or(0.0),
                    g.f32(voice + 0x44).unwrap_or(0.0),
                    g.f32(voice + 0x40).unwrap_or(0.0),
                );
            }
            let peak = entry.map_or(gain, |(_, p)| p.max(gain));
            live.insert(voice, (sample, peak));
        }
        for (voice, (sample, peak)) in &self.splice_seen {
            if !live.contains_key(voice) {
                eprintln!("SKATE_SPLICE end tick={} voice={voice:08x} sample={sample} peak_gain={peak}", self.tick);
            }
        }
        self.splice_seen = live;
    }

    /// The district's ambience beds, decoded and installed where the title's SndPlayer1 plays them
    /// from (`skate_data::audio::title::Beds::enter`).
    fn install_beds(&mut self, runtime: &mut AuthoredRuntime, map: &str, old: bool) -> Result<usize, String> {
        let Some(world) = &self.world else { return Ok(0) };
        self.beds.enter(runtime, world, &self.assets, map, old)
    }

    /// The build's sound-cache step (`--warm-audio`, user 11-02): what arriving on `map` loads, decoded now, as
    /// [`Self::frame`] does on a map change: its district's ambience beds, then the traffic banks of every game
    /// (Skate 3's own, Skate 1's and 2's folders). Without them a first play stayed silent ~4 s after the map was in.
    pub(crate) fn warm_map(&mut self, runtime: &mut AuthoredRuntime, map: &str) -> Result<(), String> {
        if let Some(&key) = district_key(map).first() {
            runtime.game_set_district(key).map_err(|e| e.to_string())?;
        }
        self.install_beds(runtime, map, false)?;
        let sets = skate_data::audio::title::TRAFFIC_BANKS.iter().map(|(_, dir)| Some(self.assets.join(dir)));
        for (i, set) in std::iter::once(None).chain(sets).enumerate() {
            if let Ok(mut serving) = self.traffic_set.lock() {
                *serving = set;
            }
            runtime.game_traffic_banks(i > 0).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Diagnostics: the retail bridge's audio state (found by the skater position it holds at
    /// `+48` and the wheel count at `+200`) against the hand-ported bridge's, once a second.
    fn diff_state(&mut self, g: &Guest, retail: &RetailAudioInputs) {
        if self.state_at.is_none() && self.tick % 30 == 0 && retail.wheel_count > 0 {
            let want: Vec<u8> = retail.position.iter().flat_map(|v| v.to_bits().to_be_bytes()).collect();
            for seg in g.segments() {
                if seg.base != skate_audio_core::game::GAME_HEAP {
                    continue;
                }
                let mut i = 0;
                while let Some(k) = seg.bytes[i..].windows(12).position(|w| w == want.as_slice()) {
                    let at = seg.base + (i + k) as u32;
                    if at >= seg.base + 48 && g.u32(at - 48 + 200).ok() == Some(retail.wheel_count) {
                        self.state_at = Some(at - 48);
                        eprintln!("SKATE_PLAYER_AUDIO state_diff found state at {:#010x}", at - 48);
                        break;
                    }
                    i += k + 4;
                }
            }
        }
        let (Some(s), true) = (self.state_at, self.tick % 60 == 0) else { return };
        let a = &self.audio;
        let f = |o: u32| g.f32(s + o).unwrap_or(f32::NAN);
        let w = |o: u32| g.u32(s + o).unwrap_or(u32::MAX);
        let b = |o: u32| g.u8(s + o).unwrap_or(255);
        eprintln!(
            "SKATE_PLAYER_AUDIO state_diff v208 {:.3}/{:.3} com212 {:.3}/{:.3} turn204 {:.3}/{:.3} wheels200 {}/{} tilt264 {:.3}/{:.3} slip232 {:.3}/{:.3} air332 {}/{} grind341 {}/{} walk716 {}/{} soft684 {}/{} land300 {}/{}",
            f(208), a.ground_speed_208, f(212), a.com_speed_212, f(204), a.turn_204,
            w(200), a.wheel_count_200, f(264), a.deck_tilt_264, f(232), a.slip_232,
            b(332), a.in_known_air_332 as u8, b(341), a.grinding_341 as u8, b(716), a.walking_716 as u8,
            w(684), a.soft_wheels_684, w(300), a.landing_bucket_300,
        );
    }
}

fn set_f(g: &mut Guest, at: u32, v: f32) -> skate_audio_core::Result<()> {
    g.set_u32(at, v.to_bits())
}

fn set_v3(g: &mut Guest, at: u32, v: [f32; 3]) -> skate_audio_core::Result<()> {
    for (i, x) in v.iter().enumerate() {
        set_f(g, at + 4 * i as u32, *x)?;
    }
    Ok(())
}

/// The camera as the game object's override matrix: rows right, up, at, position.
pub(crate) fn write_camera(g: &mut Guest, at: [f32; 3], position: [f32; 3]) -> skate_audio_core::Result<()> {
    let game = g.u32(skate_audio_core::game::GAME_OBJECT)?;
    let base = game + CAMERA + CAMERA_OVERRIDE;
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    };
    let normalize = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
    };
    let right = normalize(cross([0.0, 1.0, 0.0], at));
    let up = normalize(cross(at, right));
    for (row, (v, w)) in [(right, 0.0), (up, 0.0), (at, 0.0), (position, 1.0)].into_iter().enumerate() {
        set_v3(g, base + 16 * row as u32, v)?;
        set_f(g, base + 16 * row as u32 + 12, w)?;
    }
    g.set_u8(game + CAMERA + CAMERA_OVERRIDE_ON, 1)
}

/// The PhysOut bundle slots (`skate_audio_core::game::PHYSOUT_SLOTS`) from the engine's
/// published physics, each field at the retail offset `RetailAudioInputs` documents.
fn write_physout(
    g: &mut Guest,
    slots: &[u32; 19],
    r: &RetailAudioInputs,
    b40: &ConditionerOutput,
    body_impact_scale: f32,
) -> skate_audio_core::Result<()> {
    let (deck, motion, air, grinds, skeleton, collision, state, ground, reckoning) =
        (slots[0], slots[1], slots[2], slots[4], slots[5], slots[6], slots[7], slots[8], slots[9]);
    let (cond, interaction, scoring, filtered, off_board) =
        (slots[10], slots[13], slots[15], slots[16], slots[18]);
    let b = |v: bool| v as u8;

    // B+0: deck part transform rows, effective up, forward, position, wheel positions.
    for (i, row) in r.deck_rows.iter().enumerate() {
        set_v3(g, deck + 16 * i as u32, *row)?;
    }
    set_v3(g, deck + 80, r.effective_deck_up)?;
    set_v3(g, deck + 128, r.deck_forward)?;
    set_v3(g, deck + 144, r.deck_position)?;
    for (i, p) in r.wheel_positions.iter().enumerate() {
        set_v3(g, deck + 208 + 16 * i as u32, *p)?;
    }

    // B+4 SkateboardMotion.
    set_v3(g, motion + 64, r.angular_velocity)?;
    set_v3(g, motion + 80, r.linear_velocity)?;
    set_f(g, motion + 164, r.ground_speed)?;
    set_f(g, motion + 184, r.deck_tilt)?;
    set_f(g, motion + 200, r.motion_200)?;
    for (i, v) in r.wheel_velocities.iter().enumerate() {
        set_v3(g, motion + 208 + 16 * i as u32, *v)?;
    }

    // B+8 Air.
    set_v3(g, air + 112, r.jump_velocity_delta)?;
    set_f(g, air + 176, r.air_time_in_state)?;
    set_f(g, air + 184, r.air_time_until_landing)?;
    set_f(g, air + 200, r.air_jump_height)?;
    g.set_u32(air + 224, r.footplant_surface)?;
    g.set_u8(air + 438, b(r.in_known_air))?;
    g.set_u8(air + 440, b(r.air_440))?;
    g.set_u8(air + 448, b(r.footplant_448))?;
    g.set_u8(air + 449, b(r.footplant[0]))?;
    g.set_u8(air + 450, b(r.footplant[1]))?;

    // B+16 Grinds.
    set_f(g, grinds + 128, r.grind_impact_speed)?;
    g.set_u32(grinds + 136, r.grind_family)?;
    g.set_u32(grinds + 216, r.grind_audio_surface)?;
    g.set_u8(grinds + 316, b(r.grinding))?;
    g.set_u8(grinds + 323, b(r.grind_flag_323))?;

    // B+20 Skeleton.
    set_v3(g, skeleton + 144, r.toe_positions[0])?;
    set_v3(g, skeleton + 160, r.toe_positions[1])?;
    set_v3(g, skeleton + 192, r.foot_local_velocity[0])?;
    set_v3(g, skeleton + 208, r.foot_local_velocity[1])?;
    set_v3(g, skeleton + 224, r.foot_world_velocity[0])?;
    set_v3(g, skeleton + 240, r.foot_world_velocity[1])?;
    for (i, v) in r.ragdoll_spin.iter().enumerate() {
        set_v3(g, skeleton + 288 + 16 * i as u32, *v)?;
    }
    set_f(g, skeleton + 516, r.pump_absorption)?;
    for (i, v) in r.limb_speeds.iter().enumerate() {
        set_f(g, skeleton + 560 + 4 * i as u32, *v)?;
    }
    g.set_u8(skeleton + 599, b(r.skeleton_599))?;
    g.set_u8(skeleton + 600, b(r.feet_in_deck_box[0]))?;
    g.set_u8(skeleton + 601, b(r.feet_in_deck_box[1]))?;

    // B+24 Collision.
    g.set_u32(collision, r.wheel_count)?;
    for (i, m) in r.part_audio_surfaces.iter().enumerate() {
        g.set_u32(collision + 4 + 4 * i as u32, *m)?;
    }
    set_f(g, collision + 20, r.deck_slide_speed)?;
    set_f(g, collision + 24, r.deck_scrape)?;
    let c = &r.body_contacts;
    for i in 0..8 {
        // `sub_82BD60C8`: clamp(max(change × K164, 0.001), 0, 1) with a contact, else 0.
        let impact = if c.contact[i] {
            (c.weighted_change[i] * body_impact_scale).max(0.001).min(1.0)
        } else {
            0.0
        };
        set_f(g, collision + 80 + 4 * i as u32, impact)?;
        set_f(g, collision + 112 + 4 * i as u32, if c.contact[i] { c.slide[i] } else { 0.0 })?;
        g.set_u32(collision + 144 + 4 * i as u32, if c.contact[i] { c.material[i] } else { 0 })?;
    }
    g.set_u8(collision + 176, b(c.specific_current[0]))?;
    g.set_u8(collision + 177, b(c.specific_current[1]))?;
    set_f(g, collision + 180, c.group_8_force)?;
    set_f(g, collision + 184, c.skater_force)?;
    g.set_u32(collision + 188, c.other_skater as u32)?;
    set_f(g, collision + 192, c.group_11_force)?;
    for i in 0..4u32 {
        let k = i as usize;
        g.set_u8(collision + 3296 + i, b(r.wheel_contacts[k]))?;
        set_v3(g, collision + 3376 + 16 * i, r.wheel_contact_normals[k])?;
        g.set_u32(collision + 3440 + 4 * i, r.wheel_audio_surfaces[k])?;
        g.set_u32(collision + 3456 + 4 * i, r.wheel_seam_patterns[k])?;
    }
    g.set_u8(collision + 3473, b(r.truck_contacts[0]))?;
    g.set_u8(collision + 3474, b(r.truck_contacts[1]))?;
    g.set_u8(collision + 3475, b(r.deck_contact))?;

    // B+28 State.
    g.set_u32(state + 12, r.state_category)?;
    g.set_u32(state + 16, r.state)?;
    for (i, &flag) in r.state_flags.iter().enumerate() {
        g.set_u8(state + 52 + i as u32, b(flag))?;
    }

    // B+32 Ground, B+36 SystemReckoning.
    set_v3(g, ground + 80, r.ground_normal)?;
    set_f(g, ground + 264, r.turn)?;
    set_f(g, ground + 300, r.jump_strength)?;
    set_v3(g, reckoning + 16, r.com_velocity)?;
    set_v3(g, reckoning + 64, r.position)?;

    // B+40: the audio conditioner's output.
    set_v3(g, cond, b40.deck_angular_velocity_0)?;
    set_f(g, cond + 16, b40.slip_rise_16)?;
    set_f(g, cond + 20, b40.slip_20)?;
    set_f(g, cond + 24, b40.jump_velocity_24)?;
    g.set_u32(cond + 28, b40.grind_family_28)?;
    set_f(g, cond + 32, b40.grind_impact_32)?;
    set_f(g, cond + 36, b40.deck_scrape_36)?;
    g.set_u32(cond + 40, b40.grind_material_40)?;
    g.set_u32(cond + 44, b40.landing_bucket_44)?;
    g.set_u32(cond + 48, b40.jump_bucket_48)?;
    for i in 0..4u32 {
        set_f(g, cond + 52 + 4 * i, b40.wheel_impact_52[i as usize])?;
        g.set_u8(cond + 68 + i, b(b40.wheel_landed_68[i as usize]))?;
    }
    let bc = &b40.body_contacts_92;
    for i in 0..8u32 {
        let k = i as usize;
        set_f(g, cond + 92 + 4 * i, bc.impact[k])?;
        set_f(g, cond + 124 + 4 * i, bc.slide[k])?;
        g.set_u32(cond + 156 + 4 * i, bc.material[k])?;
    }
    g.set_u8(cond + 188, b(bc.specific_current[0]))?;
    g.set_u8(cond + 189, b(bc.specific_current[1]))?;
    set_f(g, cond + 192, bc.group_8_force)?;
    set_f(g, cond + 196, bc.skater_force)?;
    g.set_u32(cond + 200, bc.other_skater as u32)?;
    set_f(g, cond + 204, bc.group_11_force)?;
    g.set_u8(cond + 208, b(b40.grinding_208))?;
    g.set_u8(cond + 211, b(b40.foot_surface_8_211))?;
    g.set_u8(cond + 212, b(b40.step_212))?;
    g.set_u8(cond + 213, b(b40.step_213))?;
    g.set_u8(cond + 214, b(b40.left_plant_214))?;
    g.set_u8(cond + 215, b(b40.right_plant_215))?;

    // B+52 Interaction, B+60 PhysOutScoring2, B+64 Filtered, B+72 OffBoard.
    set_f(g, interaction, r.footstep_strength)?;
    set_f(g, scoring + 56, r.combo_multiplier)?;
    g.set_u32(scoring + 152, r.scorable_id as u32)?;
    g.set_u32(filtered, r.filtered_state)?;
    g.set_u32(off_board + 52, r.offboard_surface)?;
    g.set_u32(off_board + 56, r.offboard_surface)?;
    g.set_u8(off_board + 306, b(r.offboard_feet[0]))?;
    g.set_u8(off_board + 307, b(r.offboard_feet[1]))?;
    g.set_u8(off_board + 309, b(r.offboard_309))?;
    g.set_u8(off_board + 310, b(r.offboard_310))?;
    g.set_u8(off_board + 311, b(r.offboard_311))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Banks load only with cars (never online: no cars there), once per set, the old set going first.
    #[test]
    fn the_traffic_banks_load_once_per_game_and_never_without_cars() {
        let assets = std::path::Path::new("a");
        let at = |map: &str, cars: bool| PlayerAudioObservation {
            map: map.into(),
            traffic: cars.then(Vec::new),
            ..Default::default()
        };
        let s2 = Some(assets.join("private/traffic/skate2/audio"));
        let s1 = Some(assets.join("private/traffic/skate1/audio"));
        assert_eq!(traffic_banks_wanted(&None, assets, &at("New_San_Vanelona", false)), None);
        assert_eq!(traffic_banks_wanted(&None, assets, &at("New_San_Vanelona", true)), Some((s2.clone(), false)));
        assert_eq!(traffic_banks_wanted(&Some(s2.clone()), assets, &at("New_San_Vanelona", true)), None);
        assert_eq!(traffic_banks_wanted(&Some(s2.clone()), assets, &at("Skate_1_San_Vanelona", true)), Some((s1, true)));
        assert_eq!(traffic_banks_wanted(&Some(s2), assets, &at("DownTown", true)), Some((None, true)));
        assert_eq!(traffic_banks_wanted(&Some(None), assets, &at("DownTown", true)), None);
    }

    /// Bug 11 (round2 T5): this machine's riders join the online skaters' records nearest the player first, three at
    /// most (the frame holds four records, local first: 0x827A1220); an online skater keeps its slot's nibble and a
    /// rider takes its guest's own nibble unless that one is held, so its player state (`sub_824F1FB0`) stays its own.
    #[test]
    fn riders_join_the_remote_records_nearest_first_and_keep_their_nibbles() {
        use Remote::{Online, Rider};
        let me = [100., 0., 0.];
        let at = |x: f32| [100. + x, 0., 0.];
        assert!(nearest_remotes(me, &[], &[]).is_empty());
        assert_eq!(nearest_remotes(me, &[], &[at(-3.)]), [Rider(0)]);
        assert_eq!(nearest_remotes(me, &[at(20.)], &[at(5.), at(-50.), at(-10.)]), [Rider(0), Rider(2), Online(0)]);
        assert_eq!(nearest_remotes(me, &[at(1.), at(9.), at(3.)], &[at(4.)]), [Online(0), Online(2), Rider(0)]);
        assert_eq!(nearest_remotes(me, &[at(1.), at(2.), at(3.)], &[at(3.)]), [Online(0), Online(1), Online(2)], "a tie keeps the online skater");
        assert_eq!(rider_nibble(&[], 0), 1);
        assert_eq!(rider_nibble(&[], 1), 2);
        assert_eq!(rider_nibble(&[], 5), 3);
        assert_eq!(rider_nibble(&[2], 1), 3);
        assert_eq!(rider_nibble(&[2, 3], 1), 1);
    }
}

#[cfg(test)]
mod builder_probe {
    /// Probe (assets): the builder over random PhysOut bundles; per record word the bits it can set and its range, and
    /// [`super::super::remote::clean`] keeps every one of them.
    #[test]
    #[ignore = "needs the owner's assets; a probe"]
    fn probe_builder_output_over_random_bundles() {
        use skate_audio_core::authored::AuthoredRuntime;
        let assets = std::path::PathBuf::from(std::env::var_os("SKATE_ASSETS").expect("SKATE_ASSETS"));
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image")).unwrap();
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).unwrap();
        let side = super::GameSide::build(&mut runtime, &assets).unwrap();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let n = skate_net::audio::RECORD_WORDS;
        let (mut or, mut lo, mut hi, mut errors) = (vec![0u32; n], vec![i32::MAX; n], vec![i32::MIN; n], 0);
        let mut changed = std::collections::BTreeMap::new();
        let runs: u32 = std::env::var("SKATE_PROBE_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
        for run in 0..runs {
            for (i, &bytes) in skate_audio_core::game::PHYSOUT_SLOTS.iter().enumerate() {
                // Slot 15 (PhysOutScoring2) holds counts the builder loops over (0x827A2920 -> 0x82DA1570, +0x10A8):
                // zero but for its scorable id (+152).
                if i == 15 {
                    runtime.guest_mut().fill(side.skater.slots[i], 0, bytes).unwrap();
                    let id = (next() % 402) as i32 - 1;
                    runtime.guest_mut().set_u32(side.skater.slots[i] + 152, id as u32).unwrap();
                    continue;
                }
                for w in 0..bytes / 4 {
                    let r = next();
                    let v = match r % 6 {
                        0 => (r >> 32) as u32,
                        1 => ((r >> 32) % 9) as u32,
                        2 => u32::MAX,
                        3 => ((r >> 32) as u32) & 0x0101_0101,
                        4 => (((r >> 32) % 2000) as f32 / 10.0 - 100.0).to_bits(),
                        _ => 0,
                    };
                    runtime.guest_mut().set_u32(side.skater.slots[i] + 4 * w, v).unwrap();
                }
            }
            let started = std::time::Instant::now();
            let published = runtime.game_publish(&side.skater);
            if started.elapsed().as_millis() > 50 {
                eprintln!("run {run}: publish took {:?} ({published:?})", started.elapsed());
            }
            if published.is_err() {
                errors += 1;
                continue;
            }
            let r = super::first_record(runtime.guest()).unwrap();
            // The receive side's whitelist keeps whatever the builder makes (but for the body-contact block, copied
            // raw from the random conditioner output, floats past the bound, and positions, held near +0 on purpose).
            use super::super::remote::{Word, clean, word};
            let kept = clean(&r, bevy::prelude::Vec3::from_array(super::record_position(&r)));
            // +96 and +452..+464 too: B40+28 and Collision+3456 copied (the probe's random input; the engine and retail's
            // conditioner 0x82C079E0 write a grind family and a 4-bit seam pattern there).
            let copied = |i: usize| (0x50..0x6D).contains(&i) || i == 0x60 / 4 || (0x1C4 / 4..=0x1D0 / 4).contains(&i);
            for i in (0..n).filter(|&i| !copied(i) && !matches!(word(i), Word::Position)) {
                let f = f32::from_bits(r[i]);
                if kept[i] != r[i] && f.is_finite() && f.abs() <= 10_000. {
                    changed.insert(4 * i, (r[i], kept[i]));
                }
            }
            for i in 0..n {
                or[i] |= r[i];
                lo[i] = lo[i].min(r[i] as i32);
                hi[i] = hi[i].max(r[i] as i32);
            }
        }
        eprintln!("errors={errors}");
        assert!(changed.is_empty(), "the whitelist changed builder output: {changed:#x?}");
        for i in 0..n {
            eprintln!("+{:#05x} or={:#010x} lo={} hi={}", 4 * i, or[i], lo[i], hi[i]);
        }
    }
}
