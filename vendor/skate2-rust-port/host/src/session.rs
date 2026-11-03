//! The port's session, booted and ticked as the spike harness did (`s2spike/src/main.rs`), which follows the port
//! app's direct gameplay entry (`apps/skate2-pc/src/live_skating_client.rs`) with no window, renderer, audio, Juice
//! socket or updater. Not tested in play.

use std::cell::Cell;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use skate2_core::{
    JuiceHostOption, SelectedBootProgress, SelectedCommandStartPolicy, SelectedControlledBootPolicy,
    SelectedFrameNumerics, SelectedGestureAllocation, SelectedGravityPolicy, SelectedLiveSession,
    SelectedQueuedFrameOutcome, SelectedQueuedFramePolicy, SelectedRootWPolicy, SelectedSpawnPolicy,
    collection_resident_streamer::CollectionId,
    game::{CommonBackEndSeed, WorldStreamSpec},
    player::{DEFAULT_MARQUEE_ID, selected_spawn_location},
    simulation_environment_unlock_router::EnvironmentUnlockSeed,
    world_stream_catalog::WorldStreamCatalog,
};
use skate2_formats::{
    location::LocationTable,
    mount::MountSet,
    stream::{CollectionMap, high_detail_collection_filename},
};
use skate2_physics_protocol::{Boot, TICK_SECONDS};

use crate::platform::{HeadlessEffects, HostPad, NoJuice};

/// The state of the port's two process-wide generators: `global_next`'s six words and the CRT `rand`'s `holdrand`.
pub(crate) type RngState = ([u32; 6], u32);

/// The session with the host's Actor-thread answer and Juice's connection type.
pub(crate) type Session = SelectedLiveSession<fn() -> Result<u64, Infallible>, ()>;

const SIM_LOCATIONS_PATH: &str = "data\\content\\world\\stream\\BAM\\BAM_Sim.loc";
const SIM_COLLECTION_MAP_PATH: &str = "data\\content\\world\\stream\\BAM\\BAM_Sim.xsm";
/// How long the boot may poll before the host gives up; the spike's limit. A boot takes about 2.5 s.
const BOOT_DEADLINE: Duration = Duration::from_secs(300);
const BOOT_POLL: Duration = Duration::from_millis(5);

/// Boots the session on `mounts` (the Skate 2 archives) with `pad` on controller 0, and polls it until its
/// controlled frame is ready. `data_root` is the session's data root, where the port looks only for a debug build's
/// loose `data\startup.ini` (`Host::boot` gives its empty save folder). A `boot.seed` is applied before the boot
/// starts, so the boot's draws start from it; the answer holds the generators' state right after the seed was applied,
/// and whether the ambient gate was closed before the boot's initial census (`Boot::ambient` off and the world already
/// there). It gives up at its next poll once `cancel` is set, and between the stages before the polls (the collection,
/// the session and its start, which take most of the boot and cannot stop inside).
pub(crate) fn boot(
    mounts: Arc<MountSet>,
    data_root: &Path,
    pad: &mut HostPad,
    boot: &Boot,
    cancel: &AtomicBool,
) -> Result<(Box<Session>, Option<RngState>, bool), String> {
    let cancelled = || -> Result<(), String> {
        if cancel.load(Ordering::Acquire) { Err("cancelled".into()) } else { Ok(()) }
    };
    let collection = sim_collection(&mounts)?;
    cancelled()?;
    let terrain_spec = WorldStreamSpec::derive("BAM", None).map_err(|why| why.to_string())?;
    let mut session: Box<Session> = Box::new(SelectedLiveSession::new(mounts, data_root));
    cancelled()?;
    let seeded = boot.seed.map(|seed| {
        session.set_rng_seed(seed);
        session.diagnostic_rng_state()
    });
    let policy = SelectedControlledBootPolicy {
        spawn_policy: SelectedSpawnPolicy::AuthoredSelectedLocation,
        marquee: DEFAULT_MARQUEE_ID,
        actor_flags: 1,
        capsule_resource_allocated: true,
        camera_set: CommonBackEndSeed::debug_reference().camera_set.to_owned(),
        terrain_catalog: WorldStreamCatalog::new(),
        terrain_spec,
        unlock_seed: EnvironmentUnlockSeed {
            locations_lock_status_raw: 0,
            contexts_in_native_iteration_order: Vec::new(),
        },
        collection,
        dmo_update_in_parallel: false,
        arena_userdata: 0,
        presentation_lod: 0,
        presentation_submission_capacity: 65536,
        presentation_gather_capacity: 65536,
        presentation_provider: 0,
        presentation_root_w: SelectedRootWPolicy::ReadSkaterOpaque { bits: 0 },
        camera_submit_mask: 3,
        gravity_policy: SelectedGravityPolicy::VaultSkateboardDefault,
        command_start: SelectedCommandStartPolicy::NativeStaticFields,
    };
    session
        .try_begin_controlled(
            pad,
            &mut NoJuice,
            None,
            JuiceHostOption::DisableBeforeStart,
            animation_thread as fn() -> Result<u64, Infallible>,
            HeadlessEffects,
            policy,
        )
        .map_err(|why| why.to_string())?;
    cancelled()?;
    // Not native Skate 2 (user 10-20): the ambient gate closes before the boot's initial census, if the world already
    // exists, so no pedestrian or car is ever spawned. `Host::boot` sets it again after the boot either way.
    let gated_before_census = !boot.ambient && session.set_ambient_census(false);

    // The spike panicked in these two answers, which its runs never reached. The host fails the boot instead, so
    // the game hears why.
    let unanswered = Cell::new(None);
    let deadline = Instant::now() + BOOT_DEADLINE;
    loop {
        // The game abandoned this sim: stop at this poll, so it does not boot on beside the next one.
        cancelled()?;
        let progress = session
            .try_poll_controlled_boot(
                &mut |_, _, _| Ok::<_, Infallible>(SelectedGestureAllocation::Host),
                &mut || unanswered.set(Some("the source spawn orthogonality assertion fired")),
                &mut || {
                    unanswered.set(Some("the hat-enable host answer is not reconstructed"));
                    false
                },
            )
            .map_err(|why| why.to_string())?;
        if let Some(why) = unanswered.get() {
            return Err(why.to_owned());
        }
        if progress == SelectedBootProgress::ControlledFrameReady {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!("stuck at {progress:?}"));
        }
        std::thread::sleep(BOOT_POLL);
    }
    if !session.controlled_frame_ready() {
        return Err("the boot finished but the controlled frame is not ready".into());
    }
    Ok((session, seeded, gated_before_census))
}

/// Runs one queued frame of `TICK_SECONDS` with the spike's policy. A frame that does not complete is an error.
pub(crate) fn tick(session: &mut Session, pad: &mut HostPad) -> Result<(), String> {
    let policy = SelectedQueuedFramePolicy {
        dt: TICK_SECONDS,
        lod: 0,
        wait_for_bs_job: true,
        can_simulate: true,
        has_focus: true,
        ground_group: 0,
        allow_multiple_writes: true,
        spy_flags: 4,
        numerics: SelectedFrameNumerics::ExistingHostApproximation,
        physics_flags3: true,
        park_grind_preparation: true,
        exclude_grind_fill: true,
    };
    let outcome = session
        .try_tick(pad, policy, &mut || Err::<i32, _>("hands clamp service has no first-test host answer"))
        .map_err(|why| why.to_string())?;
    match outcome {
        SelectedQueuedFrameOutcome::Completed { missing_receivers: 0, .. } => Ok(()),
        SelectedQueuedFrameOutcome::Completed { missing_receivers, .. } => {
            Err(format!("{missing_receivers} missing input receivers"))
        }
        other => Err(format!("the frame did not complete: {other:?}")),
    }
}

/// The high-detail Sim collection around the authored spawn, which the boot streams in first.
fn sim_collection(mounts: &MountSet) -> Result<CollectionId, String> {
    let read = |path: &str| mounts.read_path(path).map_err(|why| format!("{path}: {why}"));
    let locations = LocationTable::parse(&read(SIM_LOCATIONS_PATH)?).map_err(|why| why.to_string())?;
    let position = locations
        .find(selected_spawn_location())
        .ok_or_else(|| format!("the spawn {} is not in BAM_Sim.loc", selected_spawn_location()))?
        .position();
    let filename = high_detail_collection_filename("Sim", position);
    let map = CollectionMap::parse(&read(SIM_COLLECTION_MAP_PATH)?).map_err(|why| why.to_string())?;
    map.collections
        .iter()
        .find(|entry| entry.filename == filename)
        .map(|entry| CollectionId(entry.id))
        .ok_or_else(|| format!("{filename} is not in BAM_Sim.xsm"))
}

/// The Actor thread's identity, as the spike gave it: a hash of the calling thread's id.
#[allow(clippy::unnecessary_wraps, reason = "the session's thread answer is a Result")]
fn animation_thread() -> Result<u64, Infallible> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    Ok(hasher.finish())
}
