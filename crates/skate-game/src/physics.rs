//! Host game physics ownership and schedule. Physical calculations stay in core.
/// Shared stock physics_mode key for loaded settings and per-frame mode packets.
#[cfg(test)]
pub(crate) const PHYSICS_MODE: &str = "easy";
/// Idle frames playback scripts prepend so their tick-60 roll-in lands on frame 90.
///
/// Retail `ConsiderPinning` 82D39A18 holds an idle human skater at its spawn X/Z
/// while frames-since-teleport < 90 (`cmpwi r9,90` at 82D39A70), until a frame
/// arrives without +2472 bit 23 (82D39AA8..AC). The D12 `NoInput` intent sets
/// that bit on every idle frame (82BDAEBC), so a velocity seeded inside the
/// window is pinned away. Before D12 nothing set it and the pin let go at frame 1.
#[cfg(test)]
pub(crate) const SPAWN_PIN_DELAY: u32 = 90 - 60;

mod air_phase;
mod air_reckoning;
mod air_trajectory;
mod animated_skeleton;
mod boneless;
pub(crate) mod camera_output;
mod climbing;
mod clock;
mod colliders;
mod controls;
mod foot_ik;
mod foot_ik_queries;
mod foot_physical_output;
mod footplant;
pub(crate) mod ground;
mod handplant;
pub(crate) mod network;
mod plant_skeleton;
mod render_pose;
pub(crate) use render_pose::{REPARENTED_HELPERS, reparented_helpers};
mod riding_outputs;
mod skateboard_controller;
mod skater;
mod skeleton_air;
mod skeleton_body;
mod skeleton_colliders;
mod skeleton_controller;
mod skeleton_feedback;
mod skeleton_input_runtime;
mod skeleton_output;
mod solve;
pub(crate) use skater::{SkaterData, SkaterRuntime};
mod animation_feedback;
mod animation_feedback_settings;
mod animation_input;
mod animation_phase;
mod biped_ground;
mod frame;
mod grind;
mod grind_air_settings;
mod grind_camera;
mod grind_chromosome;
mod grind_host;
mod grind_materials;
mod grind_names;
mod ground_animation;
mod ground_exit;
mod ground_phase;
mod ground_runtime;
mod input_phase;
mod landing_quality;
mod offboard;
/// User 10-27: a held match's teleported skater stays put (retail's AI.PinAI / ForceLocalPlayerStop).
pub(crate) mod pin;
mod player_input;
mod player_state;
pub(crate) mod props;
mod respawn;
pub(crate) use respawn::unrideable_below;
/// Other skaters on this machine: the Skate 2 tutorial's AI (round2 T5).
pub(crate) mod riders;
mod revert_state;
mod settings;
mod skeleton_grind_air;
mod skitch;
mod slide_state;
mod teleport_state;
mod wipeout;
mod wipeout_states;
// Immutable transport from a completed physics tick into the player-audio runtime. It carries
// evidence only; sound selection remains in the recovered audio program.
mod audio_observation;
mod biped_air;
mod known_air;
mod landing_on_deck;
mod offboard_audit_trace;
use crate::{
    app::{FrameSet, SimulationSet},
    world::PlayerRoot,
};
use bevy::prelude::*;
pub(crate) use controls::PlayerControls;
use riding_outputs::RidingOutputs;
use settings::PhysicsSettings;
#[cfg(test)]
use skate_core::physics::board::BodyId;
use skate_core::{
    math::Vector3,
    physics::{
        board_runtime::{BoardMotion, BoardRuntime},
        board_world::{BoardWorld, ContactRetentionSettings},
        collision::WorldContactSettings,
        drive_frames::RetailAffineTransform,
    },
};
use skate_data::collections::Collections;

#[derive(Resource)]
pub(crate) struct GamePhysics {
    pub(crate) network_proxies: network::Proxies,
    pub(crate) network_active: bool,
    pub(crate) network_contacts: usize,
    clock: clock::SimulationClock,
    pub board: BoardRuntime,
    pub riding: RidingOutputs,
    world: BoardWorld,
    grind_world: std::sync::Arc<crate::grind_world::StaticProvider>,
    grind_materials: grind_materials::GrindMaterials,
    offboard_grab_scene: offboard::grab_scene::Registry,
    props: props::Props,
    settings: PhysicsSettings,
    animation_profile: animation_phase::AnimationProfile,
    query: WorldContactSettings,
    retention: ContactRetentionSettings,
    pub ticks: u64,
    pub contact_count: usize,
    pub failed: bool,
    exchange: SimulationExchange,
    /// ProcessedPhysIn reset82BF9EF0 sets0x2000; initial stancebit20 is clear.
    /// Animation packet publication owns subsequent stance-bit updates.
    pub processed_flags_2468: u32,
    /// Toolkit ctor82C0680C clears8384bit7; wipeout entry/exit owns changes.
    pub board_wiping_out: bool,
    pub trainer: skate_mods::TrainerTuning,
    pub camera: camera_output::CameraOptions,
    /// Skate 3's water (surface 12): the 82DB8120 check and the world's surface table. Off on the Skate 1/2 maps
    /// (brief 10-26): neither s1.bin nor s2.bin ever tests surface 12, yet Skate 1's waterfront carries it.
    pub(crate) water: bool,
    /// Unrideable (surface 6) requests the teleport: TU3 82DB80C8/82DB81CC and Skate 2's ConsiderSurface (s2.bin
    /// 82332F80, `stb 1,0x3B`), whose New San Vanelona water is surface 6. Off on Skate 1's map (s1.bin unchecked).
    pub(crate) unrideable: bool,
}

/// Cross-phase records for the current fixed tick. Subsystems retain their
/// private native-shaped storage; only these buffers cross the coordinator.
pub(crate) struct SimulationExchange {
    commands: skate_core::physics::phase::PhysicsCommandBuffer,
    events: skate_core::physics::phase::PhysicsEventBuffer,
    physical_output: Option<skate_core::physics::phase::PhysicalOutputSnapshot>,
}

impl SimulationExchange {
    fn new(tick: u64) -> Self {
        Self {
            commands: skate_core::physics::phase::PhysicsCommandBuffer::new(tick),
            events: skate_core::physics::phase::PhysicsEventBuffer::new(tick),
            physical_output: None,
        }
    }

    fn emit_event(
        &mut self,
        tick: u64,
        event: skate_core::physics::phase::PhysicsEvent,
    ) -> Result<(), String> {
        self.events.emit(tick, event)
    }

    fn publish_output(&mut self, output: skate_core::physics::phase::PhysicalOutputSnapshot) {
        self.physical_output = Some(output);
    }

    fn output(&self) -> Option<&skate_core::physics::phase::PhysicalOutputSnapshot> {
        self.physical_output.as_ref()
    }

    fn events(&self) -> &[skate_core::physics::phase::PhysicsEvent] {
        self.events.events()
    }

    pub(super) fn request_state(
        &mut self,
        state: skate_core::player::state::PhysicalStateId,
    ) -> Result<(), String> {
        let tick = self.commands.tick();
        self.commands.push(
            tick,
            skate_core::physics::phase::PhysicsCommand::RequestState(state),
        )
    }
}

#[cfg(test)]
mod exchange_tests {
    use super::*;

    #[test]
    fn exchange_keeps_events_on_the_authoritative_tick() {
        let mut exchange = SimulationExchange::new(8);
        assert!(
            exchange
                .emit_event(
                    7,
                    skate_core::physics::phase::PhysicsEvent::StateChanged {
                        from: skate_core::player::state::PhysicalStateId::PhysicsGround,
                        to: skate_core::player::state::PhysicalStateId::PhysicsAir,
                    },
                )
                .is_err()
        );
        assert!(
            exchange
                .emit_event(
                    8,
                    skate_core::physics::phase::PhysicsEvent::StateChanged {
                        from: skate_core::player::state::PhysicalStateId::PhysicsGround,
                        to: skate_core::player::state::PhysicalStateId::PhysicsAir,
                    },
                )
                .is_ok()
        );
        assert!(
            exchange
                .request_state(skate_core::player::state::PhysicalStateId::PhysicsAir)
                .is_ok()
        );
        assert_eq!(exchange.commands.tick(), 8);
        assert_eq!(exchange.commands.commands().len(), 1);
        assert_eq!(exchange.events().len(), 1);
    }
}

impl GamePhysics {
    pub(crate) fn set_gesture_preferences(&mut self, gestures: Option<[u32; 4]>) {
        self.animation_profile.gesture_selections = gestures.filter(|g| g.iter().all(|v| *v < 37));
    }
    pub(crate) fn set_equipment_preferences(&mut self, truck: f32, wheel: f32) {
        if truck.is_finite() && wheel.is_finite() {
            self.animation_profile.truck_tightness = truck.clamp(0.0, 1.0);
            self.animation_profile.wheel_hardness = wheel.clamp(0.0, 1.0);
        }
    }
    pub(crate) fn set_difficulty(&mut self, difficulty: crate::difficulty::Difficulty) {
        // Actor publication carries this selector into the next physical packet.
        // Keep the board, active trick, equipment preferences and controller history.
        self.animation_profile.physics_mode = difficulty as u32;
    }
    pub(crate) fn simulation_elapsed_us(&self) -> u64 {
        (self.ticks as f64 * f64::from(self.settings.step.simulation.time_step) * 1_000_000.0)
            as u64
    }

    pub(crate) fn period(&self) -> std::time::Duration {
        self.clock.period()
    }

    pub(crate) fn difficulty_index(&self) -> u32 {
        self.animation_profile.physics_mode
    }

    pub(crate) fn world_triangles(&self) -> &[skate_core::physics::board_world::WorldTriangle] {
        self.world.triangles()
    }

    pub(crate) fn world(&self) -> &BoardWorld {
        &self.world
    }

    /// P3 HUD hook: grab candidate present, or holding a prop.
    #[allow(dead_code)] // consumed by the P3 prompt HUD
    pub(crate) fn prop_prompt(&self) -> props::Prompt {
        self.props.prompt
    }

    /// G8: the Skate 2 tutorial DMOs of `lesson` (None = no lesson) are solid, grabbable and drawn; so are the rows of
    /// each DynamicHull's state (`hulls`, round2 10-14: None = no career, no hull).
    pub(crate) fn stage_props(&mut self, lesson: Option<&str>, hulls: Option<&std::collections::BTreeMap<String, String>>) {
        self.props.set_hulls(hulls);
        self.props.stage(&mut self.world, &mut self.offboard_grab_scene.objects.objects, lesson);
    }

    /// G8 event 34: props in a DMO reset volume go back to their authored pose.
    pub(crate) fn reset_props(&mut self, inside: impl Fn(Vec3) -> bool) {
        self.props.reset(&mut self.world, &mut self.offboard_grab_scene.objects.objects, inside);
    }

    /// G8: the running lesson's DMOs, (instance id, centre).
    pub(crate) fn tutorial_dmos(&self) -> impl Iterator<Item = (u64, Vec3)> + '_ {
        self.props.dmos()
    }

    /// Flat-world convenience used by private-asset integration tests.
    #[cfg(test)]
    pub fn load(asset_root: &std::path::Path) -> Result<Self, String> {
        Self::load_with_terrain(asset_root, ground::Terrain::Flat)
    }

    pub(crate) fn load_with_terrain(
        asset_root: &std::path::Path,
        terrain: ground::Terrain,
    ) -> Result<Self, String> {
        Self::load_with_world(asset_root, terrain, None)
    }

    pub(crate) fn load_with_world(
        asset_root: &std::path::Path,
        terrain: ground::Terrain,
        map: Option<&skate_data::skate_map::SkateMap>,
    ) -> Result<Self, String> {
        Self::load_world_difficulty(
            asset_root,
            terrain,
            map,
            crate::difficulty::Difficulty::Easy,
            None,
        )
    }

    pub fn load_with_map(
        asset_root: &std::path::Path,
        map: Option<&skate_data::skate_map::SkateMap>,
    ) -> Result<Self, String> {
        Self::load_with_difficulty(asset_root, map, crate::difficulty::Difficulty::Easy)
    }

    pub fn load_with_difficulty(
        asset_root: &std::path::Path,
        map: Option<&skate_data::skate_map::SkateMap>,
        difficulty: crate::difficulty::Difficulty,
    ) -> Result<Self, String> {
        Self::load_world_difficulty(asset_root, ground::Terrain::Course, map, difficulty, None)
    }

    /// Spawn at `at` (a teleport destination: row 2 At, row 3 position) instead of
    /// the package spawn. A teleport requested before the new skater's first tick
    /// runs in the same tick as its spawn reset and drags the body across the map.
    pub fn load_at(
        asset_root: &std::path::Path,
        map: Option<&skate_data::skate_map::SkateMap>,
        difficulty: crate::difficulty::Difficulty,
        at: Option<[[f32; 4]; 4]>,
    ) -> Result<Self, String> {
        Self::load_world_difficulty(asset_root, ground::Terrain::Course, map, difficulty, at)
    }

    fn load_world_difficulty(
        asset_root: &std::path::Path,
        terrain: ground::Terrain,
        map: Option<&skate_data::skate_map::SkateMap>,
        difficulty: crate::difficulty::Difficulty,
        at: Option<[[f32; 4]; 4]>,
    ) -> Result<Self, String> {
        let data = Collections::shared(asset_root)?; // perf 10-18: one parse per process
        let settings = PhysicsSettings::load(&data)?;
        let animation_profile = animation_phase::AnimationProfile::load(&data, difficulty.key())?;
        eprintln!(
            "SKATE_PHYSICS_MODE {} index={}",
            difficulty.key(),
            animation_profile.physics_mode
        );
        let mut spawn = RetailAffineTransform {
            translation: Vector3::new(
                0.0,
                ground::HEIGHT + settings.wheel_radius - settings.authored[0].translation.y,
                0.0,
            ),
            ..RetailAffineTransform::IDENTITY
        };
        // Collision and grind rails only read the package; building them on two
        // threads takes the longer of the two instead of their sum.
        let (world, grind_world) = std::thread::scope(|scope| -> Result<_, String> {
            let rails = std::thread::Builder::new()
                .name("map-grind-loader".into())
                .spawn_scoped(scope, || {
                    if map.is_none() && terrain == ground::Terrain::Course {
                        crate::grind_world::StaticProvider::authored(
                            &crate::grind_world::test_rails(),
                        )
                    } else {
                        crate::grind_world::StaticProvider::new(map)
                    }
                })
                .map_err(|e| format!("Grind loader: {e}"))?;
            let world = match map {
                Some(map) => crate::skate_world::collision_world(map, settings.floor_material),
                None => Ok(terrain.world(settings.floor_material)),
            };
            // Join before returning either error.
            let rails = rails
                .join()
                .map_err(|_| "Grind loader failed unexpectedly".to_string())?;
            Ok((world?, std::sync::Arc::new(rails?)))
        })?;
        if let Some(map) = map {
            // Package spawn is the wheel-ground anchor, in native Y-up metres.
            let [x, y, z] = crate::skate_world::supported_spawn(&world, map.spawn);
            spawn.translation = Vector3::new(
                x,
                y + settings.wheel_radius - settings.authored[0].translation.y,
                z,
            );
            spawn.basis = skate_core::math::Basis3 {
                columns: Mat3::from_rotation_y(map.heading).to_cols_array_2d(),
            };
        }
        if let Some(map) = map {
            eprintln!(
                "SKATE_GRIND_READY splines={} primitives={}",
                map.rails.len(),
                grind_world.primitives().len()
            );
        }
        let grind_materials = grind_materials::GrindMaterials::new(&settings);
        //Authored terrains are static only. Skate 3 district packages add
        //their movable props (one query mesh, assembly and object each); never
        //infer those identities from triangle/mesh IDs.
        // G8: other maps load only the Skate 2 tutorial's own DMOs (dmos.json), props on or off.
        let (world, props, objects, bindings) = match map {
            Some(map) if props::enabled() && crate::retail_render::RetailScene::for_map(map) => {
                props::Props::load(asset_root, map, world, settings.floor_material)?
            }
            Some(map) => props::Props::load_staged(asset_root, map, world, settings.floor_material)?,
            None => (
                world,
                props::Props::empty(settings.floor_material),
                Vec::new(),
                Vec::new(),
            ),
        };
        // A trip to a spot is a game teleport (the Teleporter's cross-district command-3 job probes, 0x82597a70 ->
        // 0x82598428): grounded in the finished world, props included, as a same-map travel_to is (on its board).
        if let Some(at) = at {
            spawn = spawn_at(crate::skate_world::travel_ground(&world, at, true), spawn.basis);
        }
        let board = BoardRuntime::new(
            settings.masses,
            settings.authored,
            spawn,
            settings.step.simulation,
            BoardMotion::Active,
        );
        // A rejected prop registry leaves district props solid but ungrabbable;
        // Skate 2 tutorial DMOs then never become solid (props::Props::stage, G8 I1).
        let offboard_grab_scene = offboard::grab_scene::Registry::new(&world, objects, bindings)
            .or_else(|error| {
                error!("SKATE_PROPS: grab registry: {error}");
                offboard::grab_scene::Registry::new(&world, Vec::new(), Vec::new())
            })?;
        let processed_flags_2468 = 0x2000;
        let riding = RidingOutputs::load(&data, &board, processed_flags_2468)?;
        let (query, retention) = ground::query_settings();
        Ok(Self {
            network_proxies: network::Proxies::default(),
            network_active: false,
            network_contacts: 0,
            clock: clock::SimulationClock::default(),
            board,
            riding,
            world,
            grind_world,
            grind_materials,
            offboard_grab_scene,
            props,
            settings,
            animation_profile,
            query,
            retention,
            ticks: 0,
            contact_count: 0,
            failed: false,
            exchange: SimulationExchange::new(0),
            processed_flags_2468,
            board_wiping_out: false,
            trainer: Default::default(),
            camera: Default::default(),
            water: !map.is_some_and(|m| {
                skate_data::audio::title::OldGame::of(&m.name).is_some() || m.name.starts_with("Skate_2_")
            }),
            unrideable: !map.is_some_and(|m| m.name == skate_data::audio::title::SKATE1_MAP),
        })
    }

    #[cfg(test)]
    fn advance_board(&mut self) -> Result<(), String> {
        self.board.clear_forces();
        self.riding.start_wheel_queries(&self.board, &self.world)?;
        self.riding.finish_wheel_queries()?;
        let volumes = colliders::world_volumes(&self.board, &self.settings);
        let contacts = self
            .world
            .query_primitives(&volumes, self.query, self.retention);
        self.contact_count = contacts.len();
        // The graph/ground-state consumer must supply recovered steering and
        // forces before this is playable. This tick verifies physical assembly.
        self.board.advance(contacts, [0.0; 2], self.settings.step);
        self.riding.finish_post_physics(
            &mut self.board,
            self.board_wiping_out,
            self.processed_flags_2468,
            self.settings.step.simulation.time_step,
        )?;
        self.ticks += 1;
        if self.board.bodies().iter().any(|body| {
            let p = body.rates.position;
            !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite()
        }) {
            self.failed = true;
            return Err(format!(
                "Board solver produced a non-finite pose on tick {}",
                self.ticks
            ));
        }
        Ok(())
    }
}

/// The board transform a teleport to `at` (already ground-probed) gives (input_teleport's
/// horizontal_spawn, TU3 0x82DB8998): the board at `at` plus the 0.1 m world-Y clearance,
/// its At flattened. A near-vertical At keeps `fallback`. A world built at a spot must
/// stand where a teleport there would (B1, C1).
fn spawn_at(at: [[f32; 4]; 4], fallback: skate_core::math::Basis3) -> RetailAffineTransform {
    let forward = Vec2::new(at[2][0], at[2][2]);
    let basis = if forward.length() > f32::from_bits(0x3a83_126f) {
        skate_core::math::Basis3 {
            columns: Mat3::from_rotation_y(forward.x.atan2(forward.y)).to_cols_array_2d(),
        }
    } else {
        fallback
    };
    RetailAffineTransform {
        basis,
        translation: Vector3::new(at[3][0], at[3][1] + 0.1, at[3][2]),
    }
}

#[cfg(test)]
mod spawn_at_tests {
    use super::*;
    #[test]
    fn destination_spawn_matches_the_teleport_frame() {
        // At points along +X, tilted up: flattened to +X, right = (At.z, 0, -At.x).
        let at = [
            [0., 0., -1., 0.],
            [0., 1., 0., 0.],
            [0.8, 0.6, 0., 0.],
            [-183.4, 12.3, 433.2, 1.],
        ];
        let spawn = spawn_at(at, RetailAffineTransform::IDENTITY.basis);
        assert_eq!(
            [
                spawn.translation.x,
                spawn.translation.y,
                spawn.translation.z
            ],
            [-183.4, 12.3 + 0.1, 433.2]
        );
        let [right, up, forward] = spawn.basis.columns;
        for (got, want) in [
            (right, [0., 0., -1.]),
            (up, [0., 1., 0.]),
            (forward, [1., 0., 0.]),
        ] {
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6),
                "{got:?} != {want:?}"
            );
        }
        // Straight up: no usable heading, keep the fallback.
        let up_only = [
            [1., 0., 0., 0.],
            [0., 0., 1., 0.],
            [0., 1., 0., 0.],
            [0., 0., 0., 1.],
        ];
        assert_eq!(
            spawn_at(up_only, RetailAffineTransform::IDENTITY.basis)
                .basis
                .columns,
            RetailAffineTransform::IDENTITY.basis.columns
        );
    }
}

#[cfg(test)]
#[path = "tests/map_startup.rs"]
mod map_startup;
#[cfg(test)]
#[path = "tests/mode_harness.rs"]
pub(crate) mod mode_harness;

pub(crate) struct PhysicsPlugin;
impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        let period = app.world().resource::<GamePhysics>().clock.period();
        let asset_root = app
            .world()
            .resource::<crate::config::Config>()
            .asset_root
            .clone();
        // perf 10-19: main() may have put load_startup's copy in already (SKATE_CONTROLS_ONCE); tests and headless
        // apps still load them here.
        if !app.world().contains_resource::<PlayerControls>() {
            let controls = PlayerControls::load(&asset_root)
                .unwrap_or_else(|error| panic!("Cannot initialize trick gesture recognizers: {error}"));
            app.insert_resource(controls);
        }
        app.insert_resource(Time::<Fixed>::from_duration(period))
            .add_systems(
                FixedUpdate,
                controls::sample.in_set(SimulationSet::Controls),
            )
            .add_systems(FixedUpdate, advance.in_set(SimulationSet::Physics))
            // Task 16: while Skate 2 physics has the local skater, `advance` skips the engine's sim and this applies the
            // port's frame instead (it returns at once otherwise). Not tested in play.
            .add_systems(FixedUpdate, crate::skate2_physics::step::step.in_set(SimulationSet::Physics).after(advance))
            .init_resource::<riders::Riders>()
            .add_systems(FixedUpdate, riders::advance.after(SimulationSet::Physics))
            .add_systems(FixedUpdate, riders::collide.after(crate::multiplayer::prepare).before(SimulationSet::Physics))
            .init_resource::<audio_observation::AudioObservationCursor>()
            .add_systems(
                FixedUpdate,
                audio_observation::publish.after(SimulationSet::Physics).after(riders::advance),
            )
            .add_systems(Update, (present, props::present).in_set(FrameSet::Physics));
    }
}

pub(crate) fn advance(
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    mut physics: ResMut<GamePhysics>,
    mut skater: ResMut<SkaterRuntime>,
    mut controls: ResMut<PlayerControls>,
    graphs: Res<crate::graph_runtime::StockGraphs>,
    input: Res<crate::input::PublishedTickInput>,
    mut camera: ResMut<crate::camera::CameraRuntime>,
    mut cadence: ResMut<Time<Fixed>>,
    mut exit: MessageWriter<AppExit>,
    mut performance: Option<ResMut<crate::performance::Performance>>,
    riders: Res<riders::Riders>,
    skate2: Option<Res<crate::skate2_physics::Skate2Physics>>,
    // User 10-27: a held match's teleported skater stays put (pin.rs).
    controller: Option<Res<crate::input::ControllerInput>>,
    mut pin: Local<pin::Pin>,
) {
    if physics.failed || vehicles.occupied() {
        *pin = Default::default();
        return;
    }
    // Task 16: Skate 2 physics has the local skater (or holds it while its sim restarts): `skate2_physics::step` runs
    // instead, right after this. The riders keep the engine's sim. Not tested in play.
    if !crate::skate2_physics::step::engine_runs_local(skate2.as_deref()) {
        *pin = Default::default();
        return;
    }
    let timer = performance.as_ref().map(|_| std::time::Instant::now());
    let mut actions = input.0.actions();
    let input_available = input.0.controller_available();
    // Retail's pin is a force brake on the local skater (actor+0x770 bit 26, command 7 at 0x825980F4): the physics then
    // skips the grind query once the board stops on the ground (flags_2476 bit 24). A hold that lets the player emote
    // (bug #44/#45) lets the gestures past the brake. Riders share the profile: off again after this frame.
    let held = controller.as_ref().is_some_and(|c| c.match_hold);
    physics.animation_profile.force_braking = held;
    physics.animation_profile.gesture_force_brake_bypass = held && controller.as_ref().is_some_and(|c| c.match_gestures);
    let advanced = frame::advance_as(
        &mut physics,
        &mut skater,
        &mut controls,
        &graphs,
        &mut actions,
        input_available,
        &mut camera,
        true,
        !riders.watching(),
    );
    (physics.animation_profile.force_braking, physics.animation_profile.gesture_force_brake_bypass) = (false, false);
    if let Err(message) = advanced {
        physics.failed = true;
        error!(
            "{message}; state={:?}; tick={}; mapped_input={:?}; force_mode={}; board_axis_y={}; flags={:08x}/{:08x}/{:08x}/{:08x}/{:08x}",
            skater.player_state.current(),
            physics.ticks,
            input.0.actions(),
            skater.skeleton_input.force_mode,
            skater.skeleton_input.drive_frames[0][2][1],
            skater.player_input.processed.flags_2468,
            skater.player_input.processed.flags_2472,
            skater.player_input.processed.flags_2476,
            skater.player_input.processed.flags_2480,
            skater.player_input.processed.flags_2484,
        );
        exit.write(AppExit::error());
    } else {
        let category = skater.player_input.physical.filtered_state_0;
        pin.step(held, category, &mut physics.board);
    }
    cadence.set_timestep(physics.clock.period());
    if let (Some(performance), Some(timer)) = (performance.as_mut(), timer) {
        performance.physics(timer.elapsed());
    }
}

impl GamePhysics {
    /// physicswheels WheelRadius (retail_grading.rs puts Skate 1's deck AO on the wheels' contact plane).
    pub(crate) fn wheel_radius(&self) -> f32 {
        self.settings.wheel_radius
    }
    fn finish_skater(&mut self, skater: &mut SkaterRuntime) -> Result<(), String> {
        //Skateboard::UpdatePostPhysics82C02158 prepares the wall probe for
        //the next StartBoard, using this input phase's cached deck toolkit.
        self.riding.probes.prepare_wall(
            &skater.player_input.processed,
            skater
                .player_input
                .toolkit
                .as_ref()
                .ok_or("Postphysics wall probe requires the current board toolkit")?,
        );
        self.riding.finish_post_physics(
            &mut self.board,
            self.board_wiping_out,
            self.processed_flags_2468,
            self.settings.step.simulation.time_step,
        )?;
        let partial = skateboard_controller::partial_request(
            &skater.skateboard_controller,
            &self.riding.ground,
        );
        skeleton_feedback::publish(self, skater, partial);
        if skater.player_state.current()
            == skate_core::player::state::PhysicalStateId::WipeoutGround
        {
            wipeout_states::post_physics(skater);
        }
        if skater.player_state.current() == skate_core::player::state::PhysicalStateId::PhysicsAir {
            air_phase::update_apex(self, &mut skater.air_state);
        }
        if skater.player_state.current() == skate_core::player::state::PhysicalStateId::KnownAir {
            known_air::post_physics(self, skater)?;
        }
        if skater.player_state.current().is_grind()
            || skater.player_state.current()
                == skate_core::player::state::PhysicalStateId::Nonspecific
        {
            grind::post(self, skater)?;
        }
        wipeout::check_after_physics(self, skater)?;
        offboard::post_physics::advance(self, skater)?;
        let compression = skater.skeleton_output.average_compressions(&self.board);
        render_pose::publish(self, skater, compression)?;
        self.ticks += 1;
        let invalid_board = self.board.bodies().iter().enumerate().find(|(_, body)| {
            let p = body.rates.position;
            let v = body.rates.linear_velocity;
            let w = body.rates.angular_velocity;
            [p.x, p.y, p.z, v.x, v.y, v.z, w.x, w.y, w.z]
                .into_iter()
                .any(|value| !value.is_finite())
        });
        let invalid_skeleton = skater
            .skeleton
            .bodies()
            .iter()
            .enumerate()
            .find(|(_, body)| {
                let p = body.rates.position;
                let v = body.rates.linear_velocity;
                let w = body.rates.angular_velocity;
                [p.x, p.y, p.z, v.x, v.y, v.z, w.x, w.y, w.z]
                    .into_iter()
                    .any(|value| !value.is_finite())
            });
        if invalid_board.is_some() || invalid_skeleton.is_some() {
            self.failed = true;
            return Err(format!(
                "Shared skater solver produced non-finite rates on tick{}; board={invalid_board:?}; skeleton={invalid_skeleton:?}",
                self.ticks
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/physics_startup.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/air_playback.rs"]
mod air_tests;

#[cfg(test)]
#[path = "tests/wipeout_playback.rs"]
mod wipeout_tests;

#[cfg(test)]
#[path = "tests/camera_style_switch.rs"]
mod camera_style_switch_tests;

#[cfg(test)]
#[path = "tests/flip_playback.rs"]
mod flip_tests;

#[cfg(test)]
#[path = "tests/grab_playback.rs"]
mod grab_tests;

#[cfg(test)]
#[path = "tests/revert_playback.rs"]
mod revert_tests;

#[cfg(test)]
#[path = "tests/footplant_playback.rs"]
mod footplant_tests;

#[cfg(test)]
#[path = "tests/dark_playback.rs"]
mod dark_tests;

#[cfg(test)]
#[path = "tests/trick_completeness.rs"]
mod trick_completeness;

#[cfg(test)]
#[path = "tests/carve_decay.rs"]
mod carve_decay_tests;
#[cfg(test)]
#[path = "tests/tick_cost.rs"]
mod tick_cost_tests;
#[cfg(test)]
#[path = "tests/memory_cost.rs"]
mod memory_cost_tests;
#[cfg(test)]
#[path = "tests/skate1_walls.rs"]
mod skate1_walls_tests;
#[cfg(test)]
#[path = "tests/binocular_plaza_ride.rs"]
mod binocular_plaza_ride;
#[cfg(test)]
#[path = "tests/water_bail.rs"]
mod water_bail;
#[cfg(test)]
#[path = "tests/bail_pose.rs"]
mod bail_pose;
#[cfg(test)]
#[path = "tests/party_handover.rs"]
mod party_handover_tests;

fn present(
    physics: Res<GamePhysics>,
    history: Res<crate::presentation::Presentation>,
    replay: Res<crate::replay::Replay>,
    time: Res<Time<Fixed>>,
    mut roots: Query<&mut Transform, With<PlayerRoot>>,
) {
    if physics.failed {
        return;
    }
    let Some((previous, current, alpha)) = history.view(&replay, time.overstep_fraction()) else {
        return;
    };
    for mut root in &mut roots {
        *root = crate::presentation::blend(previous.root, current.root, alpha);
    }
}

#[cfg(test)]
#[path = "tests/powerslide_playback.rs"]
mod powerslide_tests;

#[cfg(test)]
#[path = "tests/manual_playback.rs"]
mod manual_tests;

#[cfg(test)]
#[path = "tests/hippy_playback.rs"]
mod hippy_tests;

#[cfg(test)]
#[path = "tests/recorded_playback.rs"]
mod recorded_tests;

#[cfg(test)]
#[path = "tests/offboard_air_playback.rs"]
mod offboard_air_tests;

#[cfg(test)]
#[path = "tests/offboard_midair_playback.rs"]
mod offboard_midair_playback;

#[cfg(test)]
#[path = "tests/offboard_recall_playback.rs"]
mod offboard_recall_playback_tests;

#[cfg(test)]
#[path = "tests/offboard_jump_playback.rs"]
mod offboard_jump_playback_tests;

#[cfg(test)]
#[path = "tests/offboard_root_trace.rs"]
mod offboard_root_trace;

impl SkaterRuntime {
    /// Observe the current grind publication without adding a second grind owner.
    pub(crate) fn mod_grind(&self) -> (bool, &str, u32, f32) {
        let out = &self.player_input.physical.grinds;
        let active = self.grind.active_family().is_some();
        let name = if active {
            grind_chromosome::names::lookup(out.animation_chromosome_268)
                .map(|name| name.attribute)
                .unwrap_or("")
        } else {
            ""
        };
        (
            active,
            name,
            out.words_136_140[0],
            self.grind.last_grind_distance(),
        )
    }
}

#[cfg(test)]
#[path = "tests/gesture_playback.rs"]
mod gesture_tests;
