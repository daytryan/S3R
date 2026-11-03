use crate::{
    animation, assets, camera,
    config::Config,
    graph_runtime::StockGraphs,
    input,
    physics::{GamePhysics, PhysicsPlugin, SkaterRuntime},
    verification, world,
};
use bevy::{
    prelude::*,
    render::{
        RenderPlugin,
        settings::{Backends, InstanceFlags, RenderCreation, WgpuSettings},
    },
};
use skate_data::GameAssets;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub(crate) enum FrameSet {
    Assets,
    Physics,
    Animation,
    Verification,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub(crate) enum SimulationSet {
    Input,
    Controls,
    Physics,
}

pub(crate) fn build(
    config: Config,
    manifest: GameAssets,
    graphs: StockGraphs,
    physics: GamePhysics,
    skater: SkaterRuntime,
    controls: Option<crate::physics::PlayerControls>,
) -> App {
    let retail_scene = config
        .map
        .as_ref()
        .is_some_and(|map| crate::retail_render::RetailScene::for_map(map));
    let hero = crate::hero_clip::is_active(&config);
    let mut app = App::new();
    // perf 10-19: main()'s threaded controls, in before PhysicsPlugin so it skips its own load (SKATE_CONTROLS_ONCE).
    if let Some(controls) = controls {
        app.insert_resource(controls);
    }
    crate::custom_models::register_source(&mut app);
    app.register_asset_source(
        "mods",
        bevy::asset::io::AssetSourceBuilder::platform_default(
            &crate::modding::package_root().to_string_lossy(),
            None,
        ),
    );
    let plugins =
        DefaultPlugins
            .set(AssetPlugin {
                file_path: config.asset_root.to_string_lossy().into_owned(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: config
                        .multiplayer
                        .title
                        .clone()
                        .unwrap_or_else(|| "S3R".into()),
                    // the launcher's clip run never shows its window
                    resolution: if hero { (480, 360).into() } else { (1280, 720).into() },
                    visible: !hero,
                    focused: !hero,
                    ..default()
                }),
                ..default()
            })
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: Some(Backends::VULKAN),
                    // Existing machine's validation layer rejects wgpu atomic shaders.
                    // This workaround belongs only to the rendering adapter.
                    instance_flags: InstanceFlags::empty(),
                    ..default()
                }),
                ..default()
            })
            .build()
            .disable::<bevy::log::LogPlugin>()
            // Gameplay and menu navigation both use raw XInput. No game system
            // consumes Bevy gamepad events/rumble; its second device backend can
            // stall PreUpdate (70.68 ms in the University capture).
            .disable::<bevy::gilrs::GilrsPlugin>();
    // Settings > Low Latency (B3) renders each frame on the main thread right after its update instead of overlapping
    // the next one: about 7 ms less input lag for ~15% fewer frames. SKATE_PIPELINED=0/1 forces it (benchmarks).
    let pipelined = std::env::var("SKATE_PIPELINED").map_or_else(|_| !crate::graphics_menu::low_latency(&config), |v| v != "0");
    let plugins = if !pipelined {
        plugins.disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
    } else {
        plugins
    };
    // Pipelined or not, graphics_menu::pace starts each frame later by the time it would sit waiting on the renderer.
    if std::env::var_os("SKATE_JIT").is_none_or(|v| v != "0") {
        app.insert_resource(crate::graphics_menu::JustInTime::new(pipelined));
    }
    // Settings > Audio > Output Device (voice chat, user 10-26): AudioPlugin opens it as the plugins are added.
    bevy::audio::set_output_device(crate::graphics_menu::output_device(&config));
    // NVIDIA DLSS (dlss.rs): DefaultPlugins' DlssInitPlugin registers NGX with the Vulkan instance under this id.
    #[cfg(feature = "dlss")]
    app.insert_resource(bevy::anti_alias::dlss::DlssProjectId(crate::dlss::PROJECT_ID));
    #[cfg(windows)]
    app.add_systems(Update, window_icon);
    app.add_plugins(plugins)
    // Alt-tabbed or minimized, the game keeps simulating (multiplayer peers, a second
    // local instance) but no longer renders as fast as the GPU allows: ~60 updates a
    // second. Bevy does not honour this on Windows; graphics_menu::pace enforces it.
    .insert_resource(bevy::winit::WinitSettings {
        focused_mode: bevy::winit::UpdateMode::Continuous,
        // the hidden clip window is never focused: low power would stall its frames
        unfocused_mode: if hero {
            bevy::winit::UpdateMode::Continuous
        } else {
            bevy::winit::UpdateMode::reactive_low_power(std::time::Duration::from_secs_f64(1. / 60.))
        },
    })
    .insert_resource(crate::skate_audio::player::PlayerAudioAssets(
        config.asset_root.clone(),
    ))
    .insert_resource(config)
    .insert_resource(crate::retail_render::RetailScene(retail_scene))
    .insert_resource(assets::AssetManifest(manifest))
    .insert_resource(graphs)
    .insert_resource(physics)
    .insert_resource(skater)
    .configure_sets(
        FixedUpdate,
        (
            SimulationSet::Input,
            SimulationSet::Controls,
            SimulationSet::Physics,
        )
            .chain(),
    )
    .configure_sets(
        Update,
        (
            FrameSet::Assets,
            FrameSet::Physics,
            FrameSet::Animation,
            FrameSet::Verification,
        )
            .chain(),
    )
    .add_plugins(crate::fps_overlay::FpsOverlayPlugin)
    .add_plugins((
        crate::retail_render::RetailRenderPlugin,
        input::InputPlugin,
        PhysicsPlugin,
        crate::presentation::PresentationPlugin,
        crate::replay::ReplayPlugin,
        assets::GameAssetsPlugin,
        animation::AnimationPlugin,
        world::WorldPlugin,
        crate::grind_world::GrindGeometryPlugin,
        camera::CameraPlugin,
        crate::graphics_menu::GraphicsMenuPlugin,
        crate::map_transition::MapTransitionPlugin,
        crate::render_capacity::RenderCapacityPlugin,
        verification::VerificationPlugin,
        crate::performance::PerformancePlugin,
    ));
    app.add_plugins((
        crate::session_marker::SessionMarkerPlugin,
        crate::customiser::CustomiserPlugin,
    ));
    app.add_plugins(crate::custom_models::CustomModelsPlugin);
    app.add_plugins(crate::modding::ModdingPlugin);
    app.add_plugins(crate::skate_audio::SkateAudioPlugin);
    // Voice chat (user 10-26): the voice thread, Push to Talk, its output and Lower Game Sound While Talking.
    app.add_plugins(crate::voice::VoicePlugin);
    app.add_plugins(crate::weather_audio::WeatherAudioPlugin { headless: false });
    app.add_plugins(crate::seasonal::SeasonalPlugin);
    crate::teleport_menu::install(&mut app);
    app.add_plugins(crate::updater::UpdaterPlugin);
    app.add_plugins(crate::multiplayer::MultiplayerPlugin);
    app.add_plugins(crate::online_session::OnlineSessionPlugin);
    app.add_plugins((
        crate::presence::PresencePlugin,
        crate::online_roster::OnlineRosterPlugin,
        crate::online_moves::OnlineMovesPlugin,
        crate::match_core::MatchCorePlugin,
        crate::discord_presence::DiscordPresencePlugin,
    ));
    app.add_plugins(crate::skate2_death_race::Skate2DeathRacePlugin);
    app.add_plugins(crate::scoring_hud::ScoringHudPlugin);
    // Reuses the scoring HUD's APT materials; must follow ScoringHudPlugin.
    app.add_plugins(crate::core_menu::CoreMenuPlugin);
    #[cfg(feature = "dlss")]
    app.add_plugins(crate::dlss::DlssSettingsPlugin);
    app.add_plugins(crate::challenge_map::ChallengeMapPlugin);
    app.add_plugins(crate::tip_loader::TipLoaderPlugin);
    app.add_plugins(crate::autosave_tag::AutosaveTagPlugin);
    // The power-on sequence (ESRB .. Press Start .. new-career welcome) in front of the world.
    app.add_plugins(crate::boot::BootPlugin);
    // Retail Skate 3's career (stage 1): its export and the gate. Not tested in play.
    app.add_plugins(crate::skate3_career::Skate3CareerPlugin);
    app.add_plugins(crate::skate2_hud::Skate2HudPlugin);
    app.add_plugins(crate::skate2_cast::Skate2CastPlugin);
    app.add_plugins(crate::skate1_cast::Skate1PeoplePlugin);
    app.add_plugins(crate::traffic::TrafficPlugin);
    app.add_plugins(crate::challenge::cutscene::CutscenePlugin);
    app.add_plugins(crate::skate2_arrows::Skate2ArrowsPlugin);
    app.add_plugins(crate::skate2_challenge::Skate2TutorialPlugin);
    app.add_plugins(crate::skate2_hall_of_meat::Skate2HallOfMeatPlugin);
    // The Skate 2 physics switch's resource, Off; no system until the switch (Task 19). Not tested in play.
    app.add_plugins(crate::skate2_physics::Skate2PhysicsPlugin);
    app.add_plugins(crate::hall_of_meat::HallOfMeatPlugin);
    app.add_plugins(crate::hom_bones::HomBonesPlugin);
    app.add_plugins(crate::skate2_skate_game::Skate2SkateGamePlugin);
    app.add_plugins(crate::skate3_skate_game::Skate3SkateGamePlugin);
    app.add_plugins((crate::skate3_modes::Skate3ModesPlugin, crate::skate3_one_up::Skate3OneUpPlugin, crate::skate3_uniform::Skate3UniformPlugin, crate::skate3_race::Skate3RacePlugin, crate::skate3_contest::Skate3ContestPlugin, crate::skate3_domination::Skate3DominationPlugin, crate::skate3_ots::Skate3OtsPlugin, crate::skate3_film::Skate3FilmPlugin, crate::skate3_otl::Skate3OtlPlugin));
    app.add_plugins(crate::skate2_contests::Skate2ContestsPlugin);
    app.add_plugins((crate::skate1_online::Skate1OnlinePlugin, crate::skate1_online_hud::Skate1OnlineHudPlugin));
    app.add_plugins(crate::skate1_hud::Skate1HudPlugin);
    app.add_plugins(crate::skate1_screens::Skate1ScreensPlugin);
    app.add_plugins(crate::skate1_challenge::Skate1ChallengePlugin);
    app.add_plugins(crate::skate1_pros::Skate1ProsPlugin);
    app.add_plugins((crate::apt_screen::ScreensPlugin, crate::intro_overlay::IntroOverlayPlugin));
    app.add_plugins((crate::online_hud::OnlineHudPlugin, crate::online_chat::OnlineChatPlugin));
    app.add_plugins(crate::online_menu::OnlineMenuPlugin);
    app.add_plugins((crate::music::MusicPlugin, crate::music_ui::MusicUiPlugin));
    app.add_plugins(crate::retail_ui::RetailUiPlugin);
    app.add_plugins(crate::team_select::TeamSelectPlugin);
    app.add_plugins(crate::friend_skaters::FriendSkatersPlugin);
    app.add_plugins(crate::party_play::PartyPlayPlugin);
    app.add_plugins(crate::replay::hud::ReplayHudPlugin);
    app.add_plugins(crate::hud_meters::HudMetersPlugin);
    app.add_plugins(crate::create_skater::CreateSkaterPlugin);
    app.add_plugins(crate::cas_pose::CasPosePlugin);
    app.add_plugins(crate::cas_room::CasRoomPlugin);
    app.add_plugins(crate::online_scoring::OnlineScoringPlugin);
    if hero {
        app.add_plugins(crate::hero_clip::HeroClipPlugin);
    } else {
        app.add_systems(First, catch_up_limit);
    }
    app.add_systems(Last, (crate::crash_context::sample, crate::saves::flush_on_exit));
    // The menus' setups (PostStartup) have taken what main() prefetched.
    app.add_systems(First, crate::apt_canvas::drop_prefetch.run_if(bevy::ecs::schedule::common_conditions::run_once));
    single_threaded_schedules(&mut app);
    crate::profiling::install(&mut app);
    if std::env::var_os("SKATE_PERF_LATENCY").is_some() {
        latency::install(&mut app);
    }
    app
}

/// perf 10-18: after a hitch, offline play catches up at most SKATE_MAX_CATCHUP_MS (default 67: 4 physics ticks,
/// not Bevy's 250 ms = 15 ticks, each a frame's worth of simulation in one frame); the time past it is dropped. A
/// session keeps Bevy's 250 ms so peers stay in step. SKATE_MAX_CATCHUP_MS=250 is the old behaviour.
fn catch_up_limit(online: Option<Res<crate::multiplayer::Online>>, mut time: ResMut<Time<Virtual>>) {
    static OFFLINE: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    let offline = *OFFLINE.get_or_init(|| {
        let ms = std::env::var("SKATE_MAX_CATCHUP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(67);
        std::time::Duration::from_millis(ms)
    });
    let max = if online.is_some_and(|o| o.0) { std::time::Duration::from_millis(250) } else { offline }; // Bevy's default
    if time.max_delta() != max {
        time.set_max_delta(max);
    }
}

/// perf 10-18: the main schedules' systems are mostly tiny and ~22 exclusive ones each act as a full barrier, so the
/// multi-threaded executor's handoffs cost more than they overlap (431 us/frame in play, 739 in the menus). These run
/// on one thread; their systems' own par_iter / task pools still fan out. SKATE_ST_SCHEDULES lists them
/// (comma-separated, PostUpdate allowed), "none" keeps every schedule multi-threaded.
fn single_threaded_schedules(app: &mut App) {
    use bevy::ecs::schedule::ExecutorKind;
    let list = std::env::var("SKATE_ST_SCHEDULES").unwrap_or_else(|_| "First,PreUpdate,Update,FixedUpdate,Last".into());
    for name in list.split(',').map(str::trim) {
        let single = |s: &mut Schedule| { s.set_executor_kind(ExecutorKind::SingleThreaded); };
        match name {
            "First" => app.edit_schedule(First, single),
            "PreUpdate" => app.edit_schedule(PreUpdate, single),
            "Update" => app.edit_schedule(Update, single),
            "FixedUpdate" => app.edit_schedule(FixedUpdate, single),
            "PostUpdate" => app.edit_schedule(PostUpdate, single),
            "Last" => app.edit_schedule(Last, single),
            _ => continue,
        };
    }
}

/// SKATE_PERF_LATENCY (benchmarks): each frame's start (First, before the pads are read) to the GPU finishing that
/// frame, logged every 1000 frames. ponytail: ends at GPU completion, not scanout; ~1 ms polling granularity.
mod latency {
    use bevy::{
        prelude::*,
        render::{ExtractSchedule, Render, RenderApp, RenderSystems, renderer::{RenderDevice, RenderQueue}},
    };
    use std::{sync::Mutex, time::Instant};
    static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
    static EXTRACTED: Mutex<Option<Instant>> = Mutex::new(None);
    static SAMPLES: Mutex<Vec<f64>> = Mutex::new(Vec::new());
    pub(super) fn install(app: &mut App) {
        // After the pacer's sleeps: the frame's input is read after them.
        app.add_systems(First, (|| *STARTED.lock().unwrap() = Some(Instant::now())).after(crate::graphics_menu::pace));
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render
            .add_systems(ExtractSchedule, || *EXTRACTED.lock().unwrap() = *STARTED.lock().unwrap())
            .add_systems(Render, submitted.in_set(RenderSystems::Cleanup));
    }
    fn submitted(device: Res<RenderDevice>, queue: Res<RenderQueue>, mut polling: Local<bool>) {
        if !std::mem::replace(&mut *polling, true) {
            let device = device.wgpu_device().clone();
            std::thread::spawn(move || loop {
                let _ = device.poll(wgpu::PollType::Poll);
                std::thread::sleep(std::time::Duration::from_micros(250));
            });
        }
        let Some(start) = *EXTRACTED.lock().unwrap() else { return };
        queue.on_submitted_work_done(move || {
            let mut samples = SAMPLES.lock().unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1000.);
            if samples.len() == 1000 {
                samples.sort_by(f64::total_cmp);
                let mean = samples.iter().sum::<f64>() / 1000.;
                eprintln!("SKATE_PERF_LATENCY mean={mean:.2} p50={:.2} p99={:.2}", samples[500], samples[990]);
                samples.clear();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    /// PreUpdate's ordering graph as the app's plugins register it (every plugin that puts a system in PreUpdate,
    /// in app.rs's order, but the render-only ones): a cycle through MenuInput / poll_controllers / the menus' interacts
    /// (the 10-05 replay editor one) fails here at schedule build, with no window, GPU or game.
    #[test]
    fn pre_update_builds_without_an_ordering_cycle() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), bevy::input::InputPlugin));
        app.init_asset::<Shader>().init_asset::<Image>().init_asset::<Mesh>().init_asset::<bevy::audio::AudioSource>();
        app.insert_resource(crate::config::Config {
            asset_root: std::env::temp_dir().join("skate3-schedule-check"),
            verification_capture: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
            map: None,
            map_path: None,
            difficulty: default(),
            check_assets: false,
            start_paused: false,
            multiplayer: default(),
            map_fingerprint: 0,
            teleport: None,
        });
        app.add_plugins((
            crate::input::InputPlugin,
            crate::replay::ReplayPlugin,
            crate::graphics_menu::GraphicsMenuPlugin,
            crate::map_transition::MapTransitionPlugin,
            crate::session_marker::SessionMarkerPlugin,
            crate::customiser::CustomiserPlugin,
            crate::custom_models::CustomModelsPlugin,
        ));
        // ponytail: ModdingPlugin and MultiplayerPlugin read the stock SkaterRuntime in build (modding/mod.rs:55,
        // multiplayer.rs:456), so their PreUpdate systems are not in this graph; add them when it has a test constructor.
        crate::teleport_menu::install(&mut app);
        app.add_plugins((
            crate::core_menu::CoreMenuPlugin,
            crate::challenge_map::ChallengeMapPlugin,
            crate::boot::BootPlugin,
            crate::skate3_career::Skate3CareerPlugin,
            crate::skate1_challenge::Skate1ChallengePlugin,
            crate::online_menu::OnlineMenuPlugin,
            crate::music_ui::MusicUiPlugin,
            crate::retail_ui::RetailUiPlugin,
            crate::team_select::TeamSelectPlugin,
            crate::party_play::PartyPlayPlugin,
            crate::replay::hud::ReplayHudPlugin,
            crate::voice::VoicePlugin,
        ));
        let world = app.world_mut();
        let mut schedule = world.resource_mut::<Schedules>().remove(PreUpdate).expect("PreUpdate");
        schedule.initialize(world).expect("PreUpdate builds");
        world.resource_mut::<Schedules>().insert(schedule);
    }

    /// perf 10-18 (vendor/bevy_pbr): meshes keep previous transforms only while an active camera draws motion vectors.
    #[test]
    fn previous_transforms_only_for_motion_vectors() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::pbr::{PreviousGlobalTransform, update_mesh_previous_global_transforms};
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default); // its par_iter_mut
        let mut world = World::new();
        let camera = world.spawn(Camera3d::default()).id();
        let mesh = world.spawn((Mesh3d::default(), GlobalTransform::from_xyz(1., 2., 3.))).id();
        world.run_system_once(update_mesh_previous_global_transforms).unwrap();
        assert!(world.get::<PreviousGlobalTransform>(mesh).is_none(), "no motion vectors, no copy");
        world.entity_mut(camera).insert(bevy::core_pipeline::prepass::MotionVectorPrepass);
        world.run_system_once(update_mesh_previous_global_transforms).unwrap();
        let previous = world.get::<PreviousGlobalTransform>(mesh).expect("copied for motion vectors");
        assert_eq!(previous.0, GlobalTransform::from_xyz(1., 2., 3.).affine());
    }
}

/// The headless GPU tests (graphic tiles, team photos) build their own DefaultPlugins app: in `dlss` builds that would
/// register NGX too, and panic without the game's DlssProjectId. They leave DLSS out.
#[cfg(test)]
pub(crate) fn without_dlss(plugins: bevy::app::PluginGroupBuilder) -> bevy::app::PluginGroupBuilder {
    #[cfg(feature = "dlss")]
    let plugins = plugins.disable::<bevy::anti_alias::dlss::DlssInitPlugin>();
    plugins
}

/// User 10-29: the window shows the exe's S3R icon (resource 1, build.rs) in its title bar and on the taskbar; winit
/// gives a window none, so only the console window showed it. Once, as soon as the window exists.
#[cfg(windows)]
fn window_icon(mut done: Local<bool>, window: Query<Entity, With<bevy::window::PrimaryWindow>>, _main: bevy::ecs::system::NonSendMarker) {
    use winit::platform::windows::{IconExtWindows, WindowExtWindows};
    let Ok(entity) = window.single() else { return };
    if *done {
        return;
    }
    bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        let Some(window) = windows.get_window(entity) else { return };
        match winit::window::Icon::from_resource(1, None) {
            Ok(icon) => {
                window.set_window_icon(Some(icon.clone()));
                window.set_taskbar_icon(Some(icon));
            }
            Err(error) => warn!("Window icon: {error}"),
        }
        *done = true;
    });
}
