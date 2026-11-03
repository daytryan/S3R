//! The Skate 2 physics host: the port's simulation, booted and ticked one message at a time
//! (`skate2-physics-protocol`). The game links this library and runs a `Host` on a thread of its own, swapping the
//! protocol's messages with it as values over channels (Amendment A: one program). The binary (`main.rs`) runs the same
//! loop over stdin and stdout; it is only the headless test tool. Not tested in play.
//!
//! The port's own files (`SKATER.P`, `online-config.json`, `config.json`) live in a fresh, empty folder of the host's
//! for each boot (`Host::boot`), never in the working folder, which inside the game is the game's.

pub mod data;
mod frame;
pub mod platform;
mod session;

use std::convert::Infallible;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use skate2_core::SelectedLiveSession;
use skate2_physics_protocol::{Boot, Frame, Pad, Place, Profile};

pub use data::DataError;
use platform::HostPad;

/// The first environment variable whose name starts with `SKATE2_`, ignoring case, or `None`.
///
/// The port reads 203 `SKATE2_*` variables, several of which change game state when set (design-inputs 1), so the
/// host refuses to start with any of them. Inside the game the port reads the game's own environment, so the game
/// refuses the switch while this names one (Amendment A); the binary refuses to start.
#[must_use]
pub fn unclean_environment<I: IntoIterator<Item = (String, String)>>(vars: I) -> Option<String> {
    const PREFIX: &str = "SKATE2_";
    vars.into_iter()
        .map(|(name, _value)| name)
        .find(|name| name.get(..PREFIX.len()).is_some_and(|start| start.eq_ignore_ascii_case(PREFIX)))
}

/// One booted session of the port, ticked once per message from the game.
pub struct Host {
    session: Box<session::Session>,
    pad: HostPad,
    /// How many ticks have run; the next frame's `tick` is one more.
    tick: u32,
    /// The last placement, followed until the next one.
    placement: Option<Placement>,
    /// The port's `skater_hips_position` after the last tick, from the visual snapshot the tick takes.
    hips_position: Option<[f32; 3]>,
    /// The log lines the ticks wrote since `take_logs`.
    logs: Vec<String>,
    /// The generators' state right after the boot's seed was applied, if it had one.
    seeded: Option<([u32; 6], u32)>,
    /// Whether the ambient gate was closed before the boot's initial census.
    gated_before_census: bool,
    /// The folder the port keeps its own files in for this boot. Last, so the session ends before it is removed.
    _saves: SaveFolder,
}

/// A fresh, empty folder of the host's in the system's temporary folder, for the port's own files: `SKATER.P`,
/// `online-config.json` and `config.json` (the port's `set_host_save_root`). Removed when the host ends.
///
/// The port would read and write them in the working folder, which inside the game is the game's (Amendment A): a
/// stray `saves\SKATER.P` there fails the boot, and a valid one would change the skater. In a fresh folder the port
/// starts from its first-use state on every boot, and what it writes goes nowhere the game reads.
struct SaveFolder(std::path::PathBuf);

impl SaveFolder {
    fn create() -> std::io::Result<Self> {
        static BOOTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let boot = BOOTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("skate2-physics-saves-{}-{boot}", std::process::id()));
        // A folder of that name is a crashed run's (the same process id, reused): start it empty.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for SaveFolder {
    /// The save root stays pointed here: a session still running somewhere in the process (one the game abandoned)
    /// must never fall back to the working folder, so the port's late writes come back here instead.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Where the port would read and write its own files now: `SKATER.P`, `online-config.json` and `config.json`, as
/// the port's own path functions give them. For the tests that check they are not in the working folder.
#[doc(hidden)]
#[must_use]
pub fn port_files() -> Vec<std::path::PathBuf> {
    use skate2_core::edit_skater_profile::EditSkaterProfileStore;
    use skate2_core::ui::{game_config::GameConfig, online_config::OnlineConfig};
    [EditSkaterProfileStore::default_path(), OnlineConfig::path(), GameConfig::path()]
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}

/// The most ticks a placement waits for the ground at its target (`Host::place`): 2.5 s, inside the game's 3 s deadline.
/// Past it the placement goes on as if the ground were in.
const GROUND_WAIT: u32 = 150;

/// A placement sent to the port (see `Host::place`).
struct Placement {
    /// The teleport generation the placement publishes, which the frames name.
    generation: u32,
    /// The port's teleport generation now: the placement's own, or a later one that put the skater back at the target
    /// while its ground streamed in.
    teleport: u32,
    position: [f32; 3],
    yaw: f32,
    /// Ticks since the placement was sent.
    waited: u32,
    /// The board's velocity, until it is given.
    velocity: Option<[f32; 3]>,
    stage: Stage,
}

/// How far a placement has got, after a tick.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The port has not completed the teleport yet.
    Sent,
    /// The port completed the teleport on this tick (the TELEPORTING one). Physics moves the bodies on the next tick,
    /// when `CheckForTeleport` consumes the completion in its `Update1`, and that move stops them.
    Completed,
    /// The bodies are at the placement and the board has its velocity.
    Moved,
    /// The frames since have been simulated from the placement and its velocity.
    Settled,
}

#[derive(Debug)]
pub enum HostError {
    Data(DataError),
    Boot(String),
    /// The text starts with `tick N:`.
    Tick(String),
    Place(String),
    /// A setting the port refused: the gesture slots, the game mode or the profile.
    Setting(String),
    /// `Host::boot_with`'s cancel flag was set: the game abandoned the sim, which gave up at its next poll.
    Cancelled,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(why) => write!(f, "data: {why}"),
            Self::Boot(why) => write!(f, "boot: {why}"),
            Self::Tick(why) => f.write_str(why),
            Self::Place(why) => write!(f, "place: {why}"),
            Self::Setting(why) => write!(f, "setting: {why}"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<DataError> for HostError {
    fn from(why: DataError) -> Self {
        Self::Data(why)
    }
}

impl Host {
    /// Boots the port's session on the Skate 2 data under `root`, as the spike did, and blocks until its controlled
    /// frame is ready.
    ///
    /// What `boot` asks for is applied: a `seed` before the boot starts (the generators are process-wide, so a test
    /// that seeds runs its hosts one after the other), then, once the session is up, `exact_delivery` (both only when
    /// `Some`: in play they are `None` and the run is clock-seeded, as native Skate 2's was), the gesture slots, the
    /// game mode when it is 1, 3 or 4 (any other value keeps the mode the port boots in), the profile and the ambient
    /// gate (`ambient` false, the game's value, closes it before the boot's initial census when the world exists by
    /// then, and after the boot in any case).
    ///
    /// # Errors
    ///
    /// A `Data` error when an archive cannot be opened, or when the folder holds a `manifest.json` (the
    /// `skate2-physics` stage's copy of the disc) and an entry it lists is not there (`DataError::Missing` names
    /// it, and the game answers with `HostMessage::Missing`); a `Boot` error when the session fails or stalls.
    pub fn boot(root: &Path, boot: &Boot) -> Result<Self, HostError> {
        Self::boot_with(root, boot, &AtomicBool::new(false))
    }

    /// `boot`, which gives up with `HostError::Cancelled` at its next poll once `cancel` is set: before the data is
    /// opened, before the session starts, and on every poll of the port's boot (every 5 ms). The game sets it when it
    /// abandons a sim, so an abandoned boot does not run on beside the next one (one sim at a time, Task 14).
    ///
    /// # Errors
    ///
    /// As `boot`, and `Cancelled`.
    pub fn boot_with(root: &Path, boot: &Boot, cancel: &AtomicBool) -> Result<Self, HostError> {
        let cancelled = || cancel.load(Ordering::Acquire);
        if cancelled() {
            return Err(HostError::Cancelled);
        }
        // The port prints its diagnostics on stdout unless it is quiet: the binary's stdout carries the protocol, and
        // the game's is its console.
        skate2_core::diagnostics::set_quiet(true);
        // Before the port touches a file: its own files go to this boot's empty folder, not the working folder.
        let saves = SaveFolder::create().map_err(|why| HostError::Boot(format!("the save folder: {why}")))?;
        skate2_core::edit_skater_profile::set_host_save_root(Some(saves.0.clone()));
        // A cut-down copy says what it should hold: say which entry is gone, before the port fails on it somewhere deep.
        if root.join(data::MANIFEST).is_file() {
            data::check(root)?;
        }
        if cancelled() {
            return Err(HostError::Cancelled);
        }
        let mounts = Arc::new(data::mount(root)?);
        if cancelled() {
            return Err(HostError::Cancelled);
        }
        let mut pad = HostPad::default();
        // The session's data root is only where the port looks for a debug build's loose `data\startup.ini` (cvars).
        // Retail Skate 2 has none, nor does the disc or the stage's copy; giving the save folder keeps a stray one in
        // the data folder from changing the sim.
        let (session, seeded, gated_before_census) = session::boot(mounts, &saves.0, &mut pad, boot, cancel)
            .map_err(|why| if cancelled() { HostError::Cancelled } else { HostError::Boot(why) })?;
        let mut host = Self {
            session,
            pad,
            tick: 0,
            placement: None,
            hips_position: None,
            logs: Vec::new(),
            seeded,
            gated_before_census,
            _saves: saves,
        };
        host.set_ambient(boot.ambient).map_err(|why| HostError::Boot(why.to_string()))?;
        if let Some(on) = boot.exact_delivery {
            if !host.session.set_exact_delivery(on) {
                return Err(HostError::Boot("the port has no world to set exact delivery on".into()));
            }
        }
        host.set_gestures(boot.gestures).map_err(|why| HostError::Boot(why.to_string()))?;
        if matches!(boot.mode, 1 | 3 | 4) {
            host.set_mode(boot.mode).map_err(|why| HostError::Boot(why.to_string()))?;
        }
        host.set_profile(boot.profile);
        Ok(host)
    }

    /// Runs one 60 Hz tick with `pad` held, and returns what it published.
    ///
    /// The frame's trick events are the port's HUD frames of the tick (`take_scoring_hud_frames`). The port gathers
    /// them only after the first visual snapshot, which this tick takes after its port tick, so the first tick reports
    /// none. An id the port reports outside its scorables is left out and written to `take_logs`.
    ///
    /// # Errors
    ///
    /// A `Tick` error, naming the tick, when the tick fails or publishes no frame.
    pub fn tick(&mut self, pad: Pad) -> Result<Frame, HostError> {
        let tick = self.advance(pad)?;
        let failed = |why: String| HostError::Tick(format!("tick {tick}: {why}"));
        let (camera, hips_position) = {
            let visual = self.session.try_visual_snapshot().map_err(|why| failed(why.to_string()))?;
            (frame::camera(&visual), visual.skater_hips_position)
        };
        self.hips_position = Some(hips_position);
        // As the spike did after each tick: the completed pose is ready to draw.
        self.session.acknowledge_frontend_scene_ready();
        // The port keeps the host's own copy of the HUD frames, which the snapshot above leaves in place.
        let hud = self.session.take_scoring_hud_frames();
        let tricks = frame::tricks(&hud, tick, &mut self.logs);
        let hom = frame::hom(&hud);
        let placed = self.follow_placement().map_err(failed)?;
        frame::read(&self.session, tick, camera, tricks, hom, placed).map_err(failed)
    }

    /// Runs one tick with the pad at rest and publishes nothing: the frame and its trick events are dropped, the log
    /// lines are kept for `take_logs`, and the next `tick` is numbered 1 again. The game runs it once inside the boot,
    /// before it answers `Ready`, so the first tick the game waits for costs what any tick does (the first tick also
    /// takes the port's first visual snapshot, which subscribes its HUD frames). The host binary and its disc tests
    /// never call it, so the runs they pin are unchanged; in the game the sim is one tick further on than those runs.
    ///
    /// # Errors
    ///
    /// As `tick`.
    pub fn warm_up(&mut self) -> Result<(), HostError> {
        self.tick(Pad::default())?;
        self.tick = 0;
        Ok(())
    }

    /// Puts the skater down on its board at `place.position`, facing `place.yaw`, through the port's own teleport
    /// flow (which keeps its spawn-position adjustment), and answers the placement's generation.
    ///
    /// The flow runs over the next ticks. Each frame carries the last placement's generation in `placed`, with
    /// `false` until it has settled. While native Skate 2's `IsAreaLoaded` fails at the target (its ground still
    /// streaming in), the completed teleport is sent again, for at most `GROUND_WAIT` ticks: native Skate 2 waits
    /// behind a load screen instead (user 10-22). After the tick on which the port completes the teleport, the breadcrumb trail
    /// starts over there, as after a map travel. After the next tick, on which physics moves the bodies there, the
    /// board takes `place.velocity`. The frame after that is the first to say settled, so the frame that says settled
    /// is the first one simulated with that velocity. A placement with a velocity also releases the teleport's ground
    /// pin, which would hold the board where its wheels land: not native Skate 2 (user 10-20), which never places a
    /// moving skater (VENDORED.md, S1 finding for row 3). A placement at rest keeps the native pin.
    ///
    /// With `place.end_line`, the trick line ends first, with no score, on the next tick (the port's
    /// `host_end_line`): not native Skate 2 (user 10-20), whose teleports keep the line (spec 3.5).
    ///
    /// # Errors
    ///
    /// A `Place` error when the port has no local player to place or cannot send the teleport.
    pub fn place(&mut self, place: &Place) -> Result<u32, HostError> {
        // Not native Skate 2 (user 10-20): an engine teleport ends the line, so no score carries across it (spec 3.5).
        if place.end_line && !self.session.host_end_line() {
            return Err(HostError::Place("the port has no local player whose line to end".into()));
        }
        let mut generation = self
            .session
            .diagnostic_place_local_player(place.position, place.yaw)
            .ok_or_else(|| HostError::Place("the port could not send the teleport".into()))?;
        // A placement sent before the last one's teleport is published comes after it.
        if let Some(last) = self.placement.as_ref().filter(|last| last.stage == Stage::Sent) {
            generation = generation.max(last.teleport.wrapping_add(1));
        }
        // A zero velocity is the teleport's own: its move stops the board.
        let velocity = (place.velocity != [0.0; 3]).then_some(place.velocity);
        self.placement = Some(Placement {
            generation,
            teleport: generation,
            position: place.position,
            yaw: place.yaw,
            waited: 0,
            velocity,
            stage: Stage::Sent,
        });
        Ok(generation)
    }

    /// Equips the skater's four off-board gestures: its D-pad Up, Down, Left and Right slots, each an index into the 35
    /// gestures of Skate 2 (world-objects 8; all are available, which is `Not native Skate 2` as VENDORED.md row 9
    /// says). They stay equipped across a character reload and a respawn.
    ///
    /// # Errors
    ///
    /// A `Setting` error when an index is 35 or more or the port has no local skater.
    pub fn set_gestures(&mut self, slots: [u32; 4]) -> Result<(), HostError> {
        if self.session.set_local_gestures(slots) {
            Ok(())
        } else {
            Err(HostError::Setting(format!("the gesture slots {slots:?} are not 35 gestures of a local skater")))
        }
    }

    /// Tells the port its game mode: 1 (single player: the Hall of Meat effect on), 3 (free skate: it is off, as native
    /// Skate 2) or 4 (online free skate: also off; the online flow itself is S5).
    ///
    /// # Errors
    ///
    /// A `Setting` error for any other mode, or when the port has no local skater.
    pub fn set_mode(&mut self, mode: u8) -> Result<(), HostError> {
        if self.session.set_host_game_mode(mode) {
            Ok(())
        } else {
            Err(HostError::Setting(format!("the game mode {mode} is not 1, 3 or 4, or there is no local skater")))
        }
    }

    /// Opens or closes the ambient gate: Skate 2's pedestrians and traffic (the port's census). Closed, none is
    /// spawned and the ones there are leave over the next ticks. The game's `Boot::ambient` and `SetAmbient` call it;
    /// the game always closes it (Amendment C), so the sim holds no Skate 2 city.
    ///
    /// # Errors
    ///
    /// A `Setting` error when the port has no world to gate (it cannot be told from a lock the port poisoned).
    pub fn set_ambient(&mut self, enabled: bool) -> Result<(), HostError> {
        if self.session.set_ambient_census(enabled) {
            Ok(())
        } else {
            Err(HostError::Setting("the port has no living world to set the ambient gate on".into()))
        }
    }

    /// Sets Skate 2's camera options. They reach the camera on the next tick (the port applies its profile to the
    /// camera every tick) and nothing else the port does changes them.
    pub fn set_profile(&mut self, profile: Profile) {
        self.session.set_profile_settings(skate2_core::ProfileSettingsInput {
            camera_angle: profile.camera_angle,
            invert_x: profile.invert_x,
            invert_y: profile.invert_y,
            grind_transparency: profile.grind_transparency,
        });
    }

    /// The gestures the skater started since the last call, oldest first, as indexes of the 35 gestures. For the disc
    /// tests: it is what tells which slot a D-pad press fired.
    #[doc(hidden)]
    pub fn take_started_gestures(&mut self) -> Vec<u32> {
        self.session.take_started_gestures()
    }

    /// The gesture slots as the port's gesture code reads them, or `None` without a local skater. For the disc tests.
    #[doc(hidden)]
    #[must_use]
    pub fn local_gestures(&self) -> Option<[u32; 4]> {
        self.session.local_gestures()
    }

    /// The port's `ReloadCharacterData` on the local skater, which a character change and a travel respawn run. For
    /// the disc tests of what it keeps.
    #[doc(hidden)]
    pub fn reload_character_data(&mut self) -> bool {
        self.session.diagnostic_reload_character_data()
    }

    /// The port's two process-wide generators: `global_next`'s six words and the CRT `rand`'s `holdrand`. For the disc
    /// tests of the seed.
    #[doc(hidden)]
    #[must_use]
    pub fn rng_state(&self) -> ([u32; 6], u32) {
        self.session.diagnostic_rng_state()
    }

    /// The generators' state right after `Boot::seed` was applied, before the boot began (`None` without a seed). The
    /// boot's own draws, and the ambient world's later ones, depend on timing, so this is the state a test can compare.
    #[doc(hidden)]
    #[must_use]
    pub fn rng_at_seed(&self) -> Option<([u32; 6], u32)> {
        self.seeded
    }

    /// Whether the port's census bans its pedestrians and its vehicles, as it last updated (`None` without a world).
    /// For the disc tests of the ambient gate.
    #[doc(hidden)]
    #[must_use]
    pub fn census_banned(&self) -> Option<(bool, bool)> {
        self.session.diagnostic_census_banned()
    }

    /// Whether the boot closed the ambient gate before the port's initial census ran (so that census spawned no one).
    /// For the disc tests of the ambient gate.
    #[doc(hidden)]
    #[must_use]
    pub fn gated_before_census(&self) -> bool {
        self.gated_before_census
    }

    /// The log lines the ticks since the last call wrote, oldest first: each names a trick id the port reported
    /// outside its scorables (0..=268), which the frame left out rather than wrap. The binary sends them to the game,
    /// as `HostMessage::Log`, before the tick's frame.
    pub fn take_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.logs)
    }

    /// The port's session itself, for the disc tests of the port's own hooks below the host's tick
    /// (`hud_frames_drain_on_each_call`, the line's end). The game has only the protocol.
    #[doc(hidden)]
    pub fn port(&mut self) -> &mut SelectedLiveSession<fn() -> Result<u64, Infallible>, ()> {
        &mut self.session
    }

    /// One port tick with `pad` held, as `tick` runs it, with no snapshot, no trick frames and no frame read, and
    /// without following a placement. For the disc tests of the port's own hooks; the game ticks with `tick`.
    ///
    /// # Errors
    ///
    /// A `Tick` error, naming the tick, when the tick fails.
    #[doc(hidden)]
    pub fn tick_port(&mut self, pad: Pad) -> Result<(), HostError> {
        self.advance(pad)?;
        self.session.acknowledge_frontend_scene_ready();
        Ok(())
    }

    /// The board's nose direction after the last tick (PhysOut's deck nose), or `None` before the first. The frames
    /// carry the drawn board's whole pose in `bones` (`skateboard_root`, the trucks and the wheels).
    #[must_use]
    pub fn deck_nose(&self) -> Option<[f32; 3]> {
        self.session.diagnostic_deck_nose()
    }

    /// The port's `skater_hips_position` after the last tick (`SkaterPresentation::GetHipsPos`: the published hips,
    /// or a NIS scene character's while it borrows the skater), or `None` before the first.
    #[must_use]
    pub fn hips_position(&self) -> Option<[f32; 3]> {
        self.hips_position
    }

    /// The names of the bones of the port's presentation pose, in its order. `Frame::bones` holds the first 32.
    #[must_use]
    pub fn bone_names(&self) -> Vec<&str> {
        self.session.host_skater_bone_names()
    }

    /// The breadcrumb trail's crumb positions after the last tick, newest first, and the wipeout spot a bail's
    /// respawn tries first; `None` without a trail.
    #[must_use]
    pub fn breadcrumbs(&self) -> Option<(Vec<[f32; 3]>, [f32; 3])> {
        self.session.diagnostic_breadcrumbs()
    }

    /// Counts the tick and runs the port's tick with `pad` held; answers the tick's number.
    fn advance(&mut self, pad: Pad) -> Result<u32, HostError> {
        self.tick += 1;
        let tick = self.tick;
        self.pad.set(pad);
        session::tick(&mut self.session, &mut self.pad).map_err(|why| HostError::Tick(format!("tick {tick}: {why}")))?;
        Ok(tick)
    }

    /// After a tick: the last placement's generation and whether it has settled, moving it on as `place` says.
    fn follow_placement(&mut self) -> Result<Option<(u32, bool)>, String> {
        let Some(placement) = self.placement.as_mut() else {
            return Ok(None);
        };
        if placement.stage == Stage::Sent {
            placement.waited += 1;
        }
        placement.stage = match placement.stage {
            // Native Skate 2 teleports behind a load screen while `IsAreaLoaded` fails at the target
            // (`TeleportRequiresLoadScreen`); here the skater is put back at the target until it holds, so the board
            // cannot fall through ground still streaming in (user 10-22: the sim's first ticks after its boot).
            Stage::Sent
                if self.session.diagnostic_teleport_settled(placement.teleport) == Some(true)
                    && self.session.diagnostic_area_loaded(placement.position) == Some(false)
                    && placement.waited < GROUND_WAIT =>
            {
                placement.teleport = self
                    .session
                    .diagnostic_place_local_player(placement.position, placement.yaw)
                    .ok_or("the port could not put the skater back at the placement")?;
                Stage::Sent
            }
            Stage::Sent if self.session.diagnostic_teleport_settled(placement.teleport) == Some(true) => {
                if !self.session.diagnostic_respawn_breadcrumbs() {
                    return Err("the breadcrumb trail did not start over at the placement".into());
                }
                Stage::Completed
            }
            Stage::Sent => Stage::Sent,
            Stage::Completed => {
                if let Some(velocity) = placement.velocity.take() {
                    if !self.session.diagnostic_set_board_velocity(velocity) {
                        return Err("the board did not take the placement's velocity".into());
                    }
                    // Not native Skate 2 (user 10-20): native Skate 2 never places a moving skater, and the
                    // teleport's ground pin would stop the board where its wheels land (VENDORED.md, row 3).
                    if !self.session.diagnostic_release_spawn_pin() {
                        return Err("the teleport's ground pin was not released".into());
                    }
                }
                Stage::Moved
            }
            Stage::Moved | Stage::Settled => Stage::Settled,
        };
        Ok(Some((placement.generation, placement.stage == Stage::Settled)))
    }
}

#[cfg(test)]
mod tests {
    use super::unclean_environment;

    #[test]
    fn refuses_a_skate2_variable() {
        assert_eq!(
            unclean_environment([("SKATE2_DIAGNOSTIC_SPAWN_LOCATION".into(), "1".into())]),
            Some("SKATE2_DIAGNOSTIC_SPAWN_LOCATION".into())
        );
        assert!(unclean_environment([("skate2_party_play".into(), "1".into())]).is_some());
        assert_eq!(unclean_environment([("PATH".into(), "x".into())]), None);
    }
}
