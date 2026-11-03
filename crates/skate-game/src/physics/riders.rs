//! Other skaters on this machine: the Skate 2 tutorial's AI (round2 T5, user 10-13 "Full skater"). Retail's AI is a
//! full cSkater, the player's own class plus an AIController (round2-research3 Q1): its own physics, animation and
//! wipeouts. Each rider owns the per-skater half of [`GamePhysics`] ([`Own`]), its own [`SkaterRuntime`] and
//! [`PlayerControls`]; its tick swaps that half in and runs the player's frame with the local-only stages off (camera,
//! climbing, props). It scores its own tricks (Community Spirit's competitors, user 10-15). The world, rails, props and settings stay shared (the world is ~490 MB, research3 Q6).
//! Its input is [`Rider::intents`], the ActionGraph signals its driver writes each tick (skate2_rider.rs), as retail's
//! PathController writes the AI's blackboard (research4 Q1); the pad stays neutral.
use super::{GamePhysics, PlayerControls, RidingOutputs, SkaterData, SkaterRuntime, frame};
use crate::graph_runtime::StockGraphs;
use bevy::prelude::*;
use skate_core::{
    input::{gameplay_map::GameplayActions, riding_intentions::RidingIntent},
    physics::{
        board::BodyId,
        board_runtime::{BoardMotion, BoardRuntime},
        drive_frames::RetailAffineTransform,
    },
    player::state::PhysicalStateId,
};
use std::{path::Path, sync::Arc};

/// The per-skater half of [`GamePhysics`] (research3 Q6 step 2); the animation profile stays shared (host difficulty).
struct Own {
    clock: super::clock::SimulationClock,
    board: BoardRuntime,
    riding: RidingOutputs,
    ticks: u64,
    contact_count: usize,
    failed: bool,
    exchange: super::SimulationExchange,
    processed_flags_2468: u32,
    board_wiping_out: bool,
}

impl Own {
    fn swap(&mut self, physics: &mut GamePhysics) {
        use std::mem::swap;
        swap(&mut self.clock, &mut physics.clock);
        swap(&mut self.board, &mut physics.board);
        swap(&mut self.riding, &mut physics.riding);
        swap(&mut self.ticks, &mut physics.ticks);
        swap(&mut self.contact_count, &mut physics.contact_count);
        swap(&mut self.failed, &mut physics.failed);
        swap(&mut self.exchange, &mut physics.exchange);
        swap(&mut self.processed_flags_2468, &mut physics.processed_flags_2468);
        swap(&mut self.board_wiping_out, &mut physics.board_wiping_out);
    }
}

/// Where a rider is, as its driver reads it after the tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Body {
    pub position: Vec3,
    pub velocity: Vec3,
    /// The board's At (row 2) on the skater's side, negated riding fakie ([`forward`]).
    pub forward: Vec3,
    /// The skater's own At: its animation root (the AI's air turn compares it with the path's skater quat).
    pub facing: Vec3,
    pub up: Vec3,
    pub airborne: bool,
    pub wiping_out: bool,
    /// The skater's published trick (score_packet trick_names.first): what 0x823f6ad8 reads from its trick list,
    /// INFERRED.
    pub trick: Option<skate_core::animation::output::attributes::AttributeName>,
}

pub(crate) struct Rider {
    pub key: String,
    own: Own,
    skater: SkaterRuntime,
    controls: PlayerControls,
    /// This tick's ActionGraph signals (name, value), from the rider's driver.
    pub intents: Vec<(&'static str, f32)>,
    /// Its audio observation's own cursor and the inputs of its last tick (round2 T5, bug 11): the worker builds its
    /// record from them on a guest skater of its own (player_audio/game_side.rs).
    audio_cursor: super::audio_observation::AudioObservationCursor,
    audio: Option<crate::skate_audio::RetailAudioInputs>,
}

impl Rider {
    pub(crate) fn body(&self) -> Body {
        let deck = self.own.board.part_transforms()[BodyId::Deck.index()];
        let velocity = self.own.board.bodies()[BodyId::Deck.index()].rates.linear_velocity;
        let [_, up, at] = deck.basis.columns.map(Vec3::from_array);
        let state = self.skater.player_state.current();
        let facing = Vec3::from_slice(&self.skater.animated_skeleton.roots.animation_to_world[2][..3]);
        Body {
            position: Vec3::new(deck.translation.x, deck.translation.y, deck.translation.z),
            velocity: Vec3::new(velocity.x, velocity.y, velocity.z),
            forward: forward(at, facing, self.skater.animation.packet.riding_fakie),
            facing,
            up,
            airborne: matches!(
                state,
                PhysicalStateId::PhysicsAir
                    | PhysicalStateId::PhysicsAirSecondary
                    | PhysicalStateId::KnownAir
                    | PhysicalStateId::BipedAir
            ),
            wiping_out: self.skater.wiping_out(),
            trick: self.skater.animation.motion.score_packet.trick_names.first,
        }
    }
    /// Its animation root, world space.
    pub(crate) fn root(&self) -> [[f32; 4]; 4] {
        self.skater.animated_skeleton.roots.animation_to_world
    }
    /// The completed pose, world space, for drawing under an identity root. render_pose is model space (the player
    /// draws it under PlayerRoot = animation_to_world: presentation::capture), so each bone is carried by the root;
    /// bare, every rider was drawn at the world origin (playtest 10-15: P-Rod invisible).
    pub(crate) fn pose(&self) -> Vec<Mat4> {
        let root = crate::animation::native_matrix(self.root());
        self.skater.render_pose.iter().map(|&bone| root * crate::animation::native_matrix(bone)).collect()
    }
    /// A teleport waits for its next tick (one at a time: a second `travel_to` before then is refused).
    pub(crate) fn teleporting(&self) -> bool {
        self.skater.player_input.pending_teleport().is_some()
    }
    /// A game teleport (RepositionAI, a path reset): grounded by the retail probe on the rider's next tick.
    pub(crate) fn travel_to(&mut self, at: [[f32; 4]; 4]) {
        if let Err(error) = self.skater.travel_to(at) {
            warn!("Rider {}: teleport refused ({error})", self.key);
        }
    }
    pub(crate) fn failed(&self) -> bool {
        self.own.failed
    }
    /// It has a pose of its own (its first tick ran): before that, the skeleton's initial T-pose.
    pub(crate) fn posed(&self) -> bool {
        self.skater.pose_generation > 0
    }
    /// Its scoring this tick (physics/frame.rs scores riders): the landed sequence's score when one published, and
    /// whether a sequence is open.
    pub(crate) fn sequence(&self) -> (Option<f32>, bool) {
        let s = &self.skater.scoring;
        (s.sequence_landed.then_some(s.sequence_score), s.sequence_open())
    }
    /// The audio inputs of its last tick; None before its first and once it has failed.
    pub(crate) fn audio(&self) -> Option<&crate::skate_audio::RetailAudioInputs> {
        self.audio.as_ref()
    }
}

#[derive(Resource, Default)]
pub(crate) struct Riders {
    list: Vec<Rider>,
    /// Spawns waiting for the next fixed tick (they need the world): key, locator matrix (row 2 At, row 3 position).
    wanted: Vec<(String, [[f32; 4]; 4])>,
    /// Loaded with the first rider: the tuning collections (RidingOutputs) and a camera the riders' frames never show.
    shared: Option<(skate_data::collections::Collections, crate::camera::CameraRuntime)>,
    /// The stock body geometry the riders' collision proxies use (network.rs), built from the player once.
    schema: Option<Option<super::network::Schema>>,
    /// The rider the gameplay camera follows ([`Riders::watch`]).
    watched: Option<String>,
    /// A watch changed since the watched rider last ran the gameplay camera: his next frame cuts its rig.
    cut: bool,
    /// A rider driver has its challenge data up (skate1_rider, skate2_challenge set it each fixed tick): keep a spare.
    pub warm: bool,
    spare: Option<Spare>,
    /// The graphs the spares load with (the resource never changes), cloned once while warm.
    graphs: Option<Arc<StockGraphs>>,
}

type Shared = (skate_data::collections::Collections, crate::camera::CameraRuntime);
/// D4 (perf 10-17): a spawn's file loads (~210 ms: its skeleton and animation data with their sha256, the controls;
/// the first one also the shared collections and camera, ~130 ms) made on a worker while a challenge map is up, so the
/// spawn on the fixed tick only builds the rider on the world (~30 ms). The same loads, so the same rider.
/// ponytail: one spare, refilled after each spawn; a second spawn in the same tick loads as before.
struct Spare {
    source: Arc<crate::skater_animation::AnimationSource>,
    worker: std::thread::JoinHandle<Result<Loaded, String>>,
}
struct Loaded {
    shared: Option<Shared>,
    data: SkaterData,
    controls: PlayerControls,
}

impl Riders {
    pub(crate) fn spawn(&mut self, key: &str, at: [[f32; 4]; 4]) {
        self.remove(key);
        self.wanted.push((key.to_owned(), at));
    }
    pub(crate) fn remove(&mut self, key: &str) {
        self.list.retain(|r| r.key != key);
        self.wanted.retain(|(k, _)| k != key);
    }
    /// Keeps only the riders `keep` names (a lesson's competitors outlive no clear of its AI).
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.list.retain(|r| keep(&r.key));
        self.wanted.retain(|(k, _)| keep(k));
    }
    /// A game teleport, for a rider spawned or still waiting for its first tick.
    pub(crate) fn teleport(&mut self, key: &str, at: [[f32; 4]; 4]) {
        if let Some((_, wanted)) = self.wanted.iter_mut().find(|(k, _)| k == key) {
            *wanted = at;
        } else if let Some(rider) = self.get_mut(key) {
            rider.travel_to(at);
        }
    }
    pub(crate) fn get(&self, key: &str) -> Option<&Rider> {
        self.list.iter().find(|r| r.key == key)
    }
    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut Rider> {
        self.list.iter_mut().find(|r| r.key == key)
    }
    /// Each rider's audio inputs of its last tick, by key (audio_observation::publish ships them to the worker).
    pub(crate) fn audio(&self) -> Vec<(String, crate::skate_audio::RetailAudioInputs)> {
        self.list.iter().filter_map(|rider| Some((rider.key.clone(), rider.audio.clone()?))).collect()
    }
    /// ChallengeManager.SwitchCamera(n), 0x827bd5c8 -> 0x82419688: the camera manager's skater (+0x7c) becomes n and a
    /// change of the camera it resolves (0x82417930) is posted (0x82419720). Retail's one gameplay camera follows any
    /// cSkater, so the watched rider's frame runs the player's CameraRuntime. True on a change (INFERRED: the posted
    /// change cuts the rig, as Party Play's handover: the caller's reset_rig).
    pub(crate) fn watch(&mut self, key: Option<String>) -> bool {
        let changed = self.watched != key;
        self.watched = key;
        self.cut |= changed;
        changed
    }
    /// A live watched rider holds the camera; the player's frame leaves it alone.
    pub(crate) fn watching(&self) -> bool {
        self.watched.as_deref().is_some_and(|key| self.list.iter().any(|r| r.key == key && !r.own.failed))
    }
    fn is_empty(&self) -> bool {
        self.list.is_empty() && self.wanted.is_empty()
    }
    /// Keeps a spare loading or loaded while `warm`, for the player's current skater (`source`); drops it otherwise.
    fn prefetch(&mut self, warm: bool, root: &Path, graphs: &StockGraphs, source: &Arc<crate::skater_animation::AnimationSource>) {
        if !warm {
            (self.spare, self.graphs) = (None, None);
            return;
        }
        if self.spare.as_ref().is_some_and(|s| !Arc::ptr_eq(&s.source, source)) {
            self.spare = None;
        }
        if self.spare.is_some() {
            return;
        }
        let graphs = self.graphs.get_or_insert_with(|| Arc::new(graphs.clone())).clone();
        let (root, theirs, need_shared) = (root.to_owned(), source.clone(), self.shared.is_none());
        let worker = std::thread::Builder::new().name("rider-prefetch".into()).spawn(move || -> Result<Loaded, String> {
            let shared = if need_shared {
                Some((skate_data::collections::Collections::load(&root)?, crate::camera::CameraRuntime::load(&root)?))
            } else {
                None
            };
            Ok(Loaded { shared, data: SkaterData::load(&root, &graphs, Some(theirs))?, controls: PlayerControls::load(&root)? })
        });
        match worker {
            Ok(worker) => self.spare = Some(Spare { source: source.clone(), worker }),
            Err(error) => warn!("Rider prefetch: {error}"),
        }
    }
    /// The spare for `source`, waiting for its worker if it is still loading. None (a failed load too): load in place.
    fn take_spare(&mut self, source: &Arc<crate::skater_animation::AnimationSource>) -> Option<Loaded> {
        self.spare.take_if(|s| Arc::ptr_eq(&s.source, source))?.worker.join().ok()?.ok()
    }
}

/// The riders' fixed tick, after the player's.
pub(crate) fn advance(
    mut riders: ResMut<Riders>,
    mut physics: ResMut<GamePhysics>,
    mut camera: ResMut<crate::camera::CameraRuntime>,
    player: Res<SkaterRuntime>,
    graphs: Res<StockGraphs>,
    config: Res<crate::config::Config>,
) {
    let warm = std::mem::take(&mut riders.warm);
    riders.prefetch(warm, &config.asset_root, &graphs, &player.animation.source);
    if !riders.is_empty() {
        advance_all(&mut riders, &mut physics, &player, &graphs, &config.asset_root, Some(&mut *camera));
    }
}

/// Before the player's tick (after multiplayer::prepare refilled the proxies): each rider near the player joins its
/// solve as a proxy body, as a remote skater does online (network.rs). Retail runs its SkaterSkater pipeline offline
/// (round2-research4 Q5, INFERRED from the shared SkaterPool); the pair response is UNKNOWN: the port's online one.
/// ponytail: one way; a rider's own tick sees no proxies (the player is not one for it).
pub(crate) fn collide(mut riders: ResMut<Riders>, mut physics: ResMut<GamePhysics>, player: Res<SkaterRuntime>) {
    if riders.list.is_empty() {
        return;
    }
    let riders = &mut *riders;
    let schema = riders.schema.get_or_insert_with(|| {
        super::network::Schema::new(&physics, &player).map_err(|error| warn!("Riders do not collide: {error}")).ok()
    });
    let Some(schema) = schema.as_ref() else { return };
    let mut proxies = std::mem::take(&mut physics.network_proxies);
    for rider in riders.list.iter_mut().filter(|r| !r.own.failed) {
        rider.own.swap(&mut physics);
        let body = super::network::capture_body(&physics, &rider.skater);
        rider.own.swap(&mut physics);
        proxies.append(&body, schema, &physics, &player, 0.);
    }
    physics.network_proxies = proxies;
}

/// `gameplay`: the player's camera, which the watched rider's frame runs.
pub(crate) fn advance_all(
    riders: &mut Riders,
    physics: &mut GamePhysics,
    player: &SkaterRuntime,
    graphs: &StockGraphs,
    root: &Path,
    mut gameplay: Option<&mut crate::camera::CameraRuntime>,
) {
    for (key, at) in std::mem::take(&mut riders.wanted) {
        let spare = riders.take_spare(&player.animation.source);
        match spawn(physics, &mut riders.shared, player, graphs, root, at, spare) {
            Ok((own, skater, controls)) => riders.list.push(Rider {
                key,
                own,
                skater,
                controls,
                intents: Vec::new(),
                audio_cursor: Default::default(),
                audio: None,
            }),
            Err(error) => error!("Rider {key} cannot be built: {error}"),
        }
    }
    let Some((_, spare)) = riders.shared.as_mut() else { return };
    // The player's proxies (riders among them) are not the riders' to hit; the player's count stays its own.
    let (proxies, contacts) = (std::mem::take(&mut physics.network_proxies), physics.network_contacts);
    for rider in riders.list.iter_mut().filter(|r| !r.own.failed) {
        let subject = gameplay.as_deref_mut().filter(|_| riders.watched.as_deref() == Some(rider.key.as_str()));
        let (camera, subject) = match subject {
            Some(camera) => {
                // The cut lands on the watched rider's first frame: until his spawn's fixed tick the player's frame
                // still runs the camera (`watching` is false), so a cut at the switch alone left the player's tick
                // ahead of the rider's own (user 10-15 run13: "non-monotonic subject tick: previous=12803,
                // current=0" stopped Shingo on his first S.K.A.T.E. turn).
                if std::mem::take(&mut riders.cut) {
                    camera.reset_rig();
                }
                (camera, true)
            }
            None => (&mut *spare, false),
        };
        rider.own.swap(physics);
        let result = tick(physics, rider, graphs, camera, subject);
        // Its tick as the audio sees it, on its own half (publish's body with its own cursor). The full observation is
        // built; only its RetailAudioInputs travel (the game side's builder reads nothing else).
        // ponytail: the hand-ported path (SKATE_AUDIO_GAME_SIDE=0) plays no riders.
        rider.audio = match result {
            Ok(()) => super::audio_observation::observe(physics, &rider.skater, camera, &mut rider.audio_cursor)
                .map(|observation| observation.retail),
            Err(_) => None,
        };
        rider.own.swap(physics);
        if let Err(error) = result {
            rider.own.failed = true;
            error!("Rider {} stops: {error}", rider.key);
        }
    }
    (physics.network_proxies, physics.network_contacts) = (proxies, contacts);
}

/// One rider frame on its own half of `physics` (already swapped in): a neutral pad, the driver's signals on top.
fn tick(
    physics: &mut GamePhysics,
    rider: &mut Rider,
    graphs: &StockGraphs,
    camera: &mut crate::camera::CameraRuntime,
    subject: bool,
) -> Result<(), String> {
    let mut pad = GameplayActions::from_values([0.; 18]);
    rider.controls.update(
        &mut pad,
        physics.settings.step.simulation.time_step,
        physics.settings.input_magnitude_threshold,
        rider.skater.player_input.physical.scoring.capabilities_204,
    );
    for &(name, value) in &rider.intents {
        rider.controls.action_intents.insert(name, value);
        rider.controls.intents.push(RidingIntent { name, value });
    }
    frame::advance_as(physics, &mut rider.skater, &mut rider.controls, graphs, &mut pad, true, camera, false, subject)
}

/// A rider's half of the physics and its skater, at `at` grounded as a game teleport lands (physics::spawn_at). Its
/// file loads come from `spare` (a worker's, [`Riders::prefetch`]) or, without one, load here on the fixed tick.
fn spawn(
    physics: &mut GamePhysics,
    shared: &mut Option<Shared>,
    player: &SkaterRuntime,
    graphs: &StockGraphs,
    root: &Path,
    at: [[f32; 4]; 4],
    spare: Option<Loaded>,
) -> Result<(Own, SkaterRuntime, PlayerControls), String> {
    let (spare_shared, spare) = match spare {
        Some(Loaded { shared, data, controls }) => (shared, Some((data, controls))),
        None => (None, None),
    };
    if shared.is_none() {
        *shared = Some(match spare_shared {
            Some(loaded) => loaded,
            None => (skate_data::collections::Collections::load(root)?, crate::camera::CameraRuntime::load(root)?),
        });
    }
    let (data, _) = shared.as_ref().ok_or("rider collections")?;
    let place = super::spawn_at(crate::skate_world::probe_ground(&physics.world, at).0, RetailAffineTransform::IDENTITY.basis);
    let settings = &physics.settings;
    let board = BoardRuntime::new(settings.masses, settings.authored, place, settings.step.simulation, BoardMotion::Active);
    let processed_flags_2468 = 0x2000;
    let riding = RidingOutputs::load(data, &board, processed_flags_2468)?;
    let mut own = Own {
        clock: Default::default(),
        board,
        riding,
        ticks: 0,
        contact_count: 0,
        failed: false,
        exchange: super::SimulationExchange::new(0),
        processed_flags_2468,
        board_wiping_out: false,
    };
    let (loaded, controls) = match spare {
        Some((data, controls)) => (data, Some(controls)),
        None => (SkaterData::load(root, graphs, Some(player.animation.source.clone()))?, None),
    };
    // UNTRACED: the AI's physics profile (AIController+108); the riders take the host's difficulty.
    let mode = crate::difficulty::NATIVE_MODES[physics.difficulty_index() as usize];
    // with_data builds the skeleton on the board it finds in `physics`: the rider's.
    own.swap(physics);
    let skater = SkaterRuntime::with_data(root, loaded, physics, mode);
    own.swap(physics);
    let controls = match controls {
        Some(controls) => controls,
        None => PlayerControls::load(root)?,
    };
    Ok((own, skater?, controls))
}

/// The AI's forward (research4: 0x823f64a8, row 2 of the board matrix, negated when `[attrs+56]+151`; INFERRED there
/// as fakie). The board's At swaps ends at every shove-it: the shipped recordings' board quats flip against travel at
/// each pop shove-it while the skater's own facing doesn't (fp_u3_tutorial02 path 0 node 21, path 1 nodes 14, 61,
/// 77, ...). Bare, a landed shove-it left P-Rod facing backwards to his driver: no push, a turn back up his path
/// (user 10-15 run12: stuck, the wrong way after his nollie tricks). INFERRED: the At on the skater's side, negated
/// riding fakie (riding fakie the recordings' skater faces against travel, he_u1_skate01_10 after its 222-degree ollie).
fn forward(at: Vec3, facing: Vec3, fakie: bool) -> Vec3 {
    let at = if at.dot(facing) < 0. { -at } else { at };
    if fakie { -at } else { at }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-15 run12: a shove-it swaps the board's ends under a skater still facing his way; forward stays his way.
    #[test]
    fn forward_survives_a_shove_it() {
        let (way, nose) = (Vec3::Z, Vec3::new(0.1, 0., 1.).normalize());
        assert_eq!(forward(nose, way, false), nose);
        assert_eq!(forward(-nose, way, false), nose, "the board turned 180 under him");
        assert_eq!(forward(-nose, -way, true), nose, "riding fakie: the skater faces back, his travel is ahead");
    }

    /// User 10-13 (bugs 9-11, "Full skater"): retail's AI is the player's own skater class (round2-research3 Q1). A
    /// rider spawned beside the player runs its own physics and animation: pushed by its intents it rolls off and
    /// poses, and the player's board, tick count and output are left as they were.
    #[test]
    #[ignore = "requires private stock animation banks and collections (SKATE3_ASSET_ROOT)"]
    fn a_rider_rolls_on_its_own_and_leaves_the_player_untouched() {
        let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
        let root = std::path::Path::new(&root);
        let assets = skate_data::GameAssets::load(root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
        let mut physics = GamePhysics::load(root).unwrap();
        let mut player = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
        let mut controls = PlayerControls::load(root).unwrap();
        let mut camera = crate::camera::CameraRuntime::load(root).unwrap();
        // "Rider ... stops: ..." is logged, not returned: shown with a failure.
        let _log = bevy::log::tracing::subscriber::set_default(
            bevy::log::tracing_subscriber::fmt().with_test_writer().finish(),
        );
        // User 10-15 run13/run14, the game's own order: the player's frames run the camera (its tick count in both
        // of the camera's monotonic checks, runtime.rs and subject.rs); SwitchCamera (skate1_challenge::watch_camera)
        // watches the rider and cuts the rig before his spawn; the player's next fixed frame still runs the camera
        // (physics.rs: !watching, he isn't spawned yet); then advance_all spawns him and his tick 0 runs it.
        let mut player_frame = |physics: &mut GamePhysics, camera: &mut crate::camera::CameraRuntime| {
            let mut pad = GameplayActions::from_values([0.; 18]);
            let step = physics.settings.step.simulation.time_step;
            let capabilities = player.player_input.physical.scoring.capabilities_204;
            controls.update(&mut pad, step, physics.settings.input_magnitude_threshold, capabilities);
            frame::advance(physics, &mut player, &mut controls, &graphs, &mut pad, true, camera).unwrap();
        };
        for _ in 0..30 {
            player_frame(&mut physics, &mut camera);
        }
        let mut riders = Riders::default();
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [5., 0., 0., 1.]];
        riders.spawn("slappy#ai1", at);
        assert!(riders.watch(Some("slappy#ai1".into())) && !riders.watch(Some("slappy#ai1".into())), "a change only");
        camera.reset_rig();
        assert!(!riders.watching(), "not spawned before advance_all");
        player_frame(&mut physics, &mut camera);
        let deck = |physics: &GamePhysics| physics.board.part_transforms()[BodyId::Deck.index()].translation;
        let before = (deck(&physics), physics.ticks);
        advance_all(&mut riders, &mut physics, &player, &graphs, root, Some(&mut camera));
        assert!(!riders.get("slappy#ai1").expect("spawned").failed(), "his first watched frame (the log has why)");
        assert!(camera.latest_subject.is_some_and(|s| s.tick < 30), "his first frame publishes his own tick");
        for tick in 0..240 {
            if let Some(rider) = riders.get_mut("slappy#ai1") {
                rider.intents = if tick % 40 < 2 { vec![("LeftPush", 1.), ("NewPush", 1.), ("Pushing", 1.)] } else { Vec::new() };
            }
            advance_all(&mut riders, &mut physics, &player, &graphs, root, Some(&mut camera));
        }
        assert!(riders.watching());
        let shot = camera.frame.expect("the watched rider's frame runs the gameplay camera").position;
        let near = Vec3::new(shot[0], shot[1], shot[2]).distance(riders.get("slappy#ai1").unwrap().body().position);
        assert!(near < 8., "the camera follows the watched rider: {near} m off");
        let rider = riders.get("slappy#ai1").expect("spawned");
        let body = rider.body();
        assert!(!rider.failed(), "the rider's frames run");
        assert!((body.position.x - 5.).abs() < 1. && body.position.z > 1., "pushed forward along +Z: {:?}", body.position);
        assert!(!body.airborne && !body.wiping_out);
        assert_eq!(rider.pose().len(), player.render_pose.len(), "posed on the player's skeleton");
        let far = rider.pose().iter().map(|bone| bone.w_axis.truncate().distance(body.position)).fold(0., f32::max);
        assert!(far < 3., "posed where the rider stands, not at the origin: {far}");
        let after = deck(&physics);
        assert_eq!((after.x, after.y, after.z, physics.ticks), (before.0.x, before.0.y, before.0.z, before.1), "the player is untouched");
        riders.remove("slappy#ai1");
        assert!(riders.get("slappy#ai1").is_none());
    }

    /// D4 (perf 10-17): a rider spawned from a prefetched spare is the rider a spawn loading in place makes, bit for
    /// bit after 240 ticks of the same intents; the spawn's fixed tick is the cheaper for it (prints both).
    #[test]
    #[ignore = "requires private stock animation banks and collections (SKATE3_ASSET_ROOT)"]
    fn a_prefetched_rider_is_the_same_rider() {
        let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
        let root = std::path::Path::new(&root);
        let assets = skate_data::GameAssets::load(root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [5., 0., 0., 1.]];
        let run = |prefetch: bool| {
            let mut physics = GamePhysics::load(root).unwrap();
            let player = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
            let mut riders = Riders::default();
            if prefetch {
                riders.prefetch(true, root, &graphs, &player.animation.source);
                while !riders.spare.as_ref().unwrap().worker.is_finished() {
                    std::thread::yield_now();
                }
            }
            riders.spawn("slappy#ai1", at);
            let t = std::time::Instant::now();
            advance_all(&mut riders, &mut physics, &player, &graphs, root, None);
            println!("RIDER_SPAWN prefetched={prefetch}: {:.1} ms (spawn + first tick)", t.elapsed().as_secs_f64() * 1e3);
            assert!(riders.spare.is_none(), "taken");
            let t = std::time::Instant::now();
            for tick in 0..240 {
                if let Some(rider) = riders.get_mut("slappy#ai1") {
                    rider.intents = if tick % 40 < 2 { vec![("LeftPush", 1.), ("NewPush", 1.), ("Pushing", 1.)] } else { Vec::new() };
                }
                advance_all(&mut riders, &mut physics, &player, &graphs, root, None);
            }
            println!("RIDER_TICK {:.3} ms per rider tick", t.elapsed().as_secs_f64() * 1e3 / 240.);
            let rider = riders.get("slappy#ai1").unwrap();
            assert!(!rider.failed());
            let b = rider.body();
            let bits = |v: Vec3| v.to_array().map(f32::to_bits);
            (bits(b.position), bits(b.velocity), bits(b.facing), rider.pose().iter().flat_map(|m| m.to_cols_array().map(f32::to_bits)).collect::<Vec<_>>())
        };
        assert!(run(false) == run(true), "the same rider either way");
        // Not warm (the challenge map left): no spare kept.
        let physics = GamePhysics::load(root).unwrap();
        let player = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
        let mut riders = Riders::default();
        riders.prefetch(true, root, &graphs, &player.animation.source);
        assert!(riders.spare.is_some() && riders.graphs.is_some());
        riders.prefetch(false, root, &graphs, &player.animation.source);
        assert!(riders.spare.is_none() && riders.graphs.is_none());
    }

    /// round2-research4 Q5: retail's SkaterSkater pipeline runs offline, so a rider beside the player is a body in the
    /// player's solve (network.rs proxies); a rider across the map is not, and a rider's own tick leaves them alone.
    #[test]
    #[ignore = "requires private stock animation banks and collections (SKATE3_ASSET_ROOT)"]
    fn a_rider_beside_the_player_is_solid_to_it() {
        use bevy::ecs::system::RunSystemOnce;
        let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
        let root = std::path::Path::new(&root);
        let assets = skate_data::GameAssets::load(root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
        let mut app = App::new();
        app.insert_resource(GamePhysics::load(root).unwrap());
        let player = SkaterRuntime::load(root, &graphs, app.world().resource::<GamePhysics>(), "normal").unwrap();
        let deck = app.world().resource::<GamePhysics>().board.part_transforms()[BodyId::Deck.index()].translation;
        let mut riders = Riders::default();
        for (key, x) in [("near#ai1", 1.), ("far#ai2", 200.)] {
            riders.spawn(key, [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [deck.x + x, deck.y, deck.z, 1.]]);
        }
        advance_all(&mut riders, &mut app.world_mut().resource_mut::<GamePhysics>(), &player, &graphs, root, None);
        assert!(app.world().resource::<GamePhysics>().network_proxies.bodies.is_empty(), "a rider tick hands the proxies back");
        app.insert_resource(riders).insert_resource(player);
        app.world_mut().run_system_once(collide).unwrap();
        let proxies = &app.world().resource::<GamePhysics>().network_proxies;
        let player = app.world().resource::<SkaterRuntime>();
        let per_rider = app.world().resource::<GamePhysics>().board.bodies().len() + player.skeleton.bodies().len();
        assert!(!proxies.bodies.is_empty() && proxies.bodies.len() <= per_rider, "the near rider only: {}", proxies.bodies.len());
    }

    /// User bug 11 (10-13, "Slappy's board makes no sound"): retail plays every skater in the audio frame with its own
    /// player code (`sub_824F1FB0`) from the record `sub_827A1B78` builds per skater. Each rider tick observes its own
    /// half (audio_observation::observe with its own cursor): a rolling rider hands over its own wheels and speed at
    /// its own place, and `Riders::audio` ships them by key for the worker's guest skater (game_side).
    #[test]
    #[ignore = "requires private stock animation banks and collections (SKATE3_ASSET_ROOT)"]
    fn a_rolling_rider_observes_its_own_audio_inputs() {
        let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
        let root = std::path::Path::new(&root);
        let assets = skate_data::GameAssets::load(root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
        let mut physics = GamePhysics::load(root).unwrap();
        let player = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
        let mut riders = Riders::default();
        riders.spawn("slappy#ai1", [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [5., 0., 0., 1.]]);
        assert!(riders.audio().is_empty(), "nothing before its first tick");
        for tick in 0..240 {
            if let Some(rider) = riders.get_mut("slappy#ai1") {
                rider.intents = if tick % 40 < 2 { vec![("LeftPush", 1.), ("NewPush", 1.), ("Pushing", 1.)] } else { Vec::new() };
            }
            advance_all(&mut riders, &mut physics, &player, &graphs, root, None);
        }
        let rider = riders.get("slappy#ai1").expect("spawned");
        let audio = rider.audio().expect("the rider's tick was observed");
        let body = rider.body();
        assert!(audio.wheel_count > 0 && audio.ground_speed > 0.1, "rolling on its wheels: {} at {}", audio.wheel_count, audio.ground_speed);
        assert!(
            (audio.deck_position[0] - body.position.x).abs() < 0.5 && (audio.deck_position[2] - body.position.z).abs() < 0.5,
            "its own deck, not the player's: {:?} vs {:?}",
            audio.deck_position,
            body.position
        );
        let shipped = riders.audio();
        assert_eq!(shipped.len(), 1);
        assert_eq!(shipped[0].0, "slappy#ai1");
        assert_eq!(&shipped[0].1, audio);
    }
}
