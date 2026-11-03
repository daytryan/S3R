// User 10-29: no console window with the game (closing it closed the game); logs still reach crash_report's pipes and
// any file a script redirects them to. The test harness needs its console.
#![cfg_attr(not(test), windows_subsystem = "windows")]
mod animation;
mod animation_pose;
mod app;
mod apt_canvas;
mod button_icons;
mod apt_display;
mod apt_movie;
mod apt_scene;
mod apt_screen;
mod apt_text;
mod apt_vm;
mod assets;
mod boot;
mod boot_flow;
mod boot_screens;
mod camera;
mod cas_menu;
mod cas_pose;
mod cas_room;
mod colour_wheel;
mod challenge;
mod challenge_map;
mod online_map;
mod online_map_runtime;
mod challenge_map_runtime;
mod chat_filter;
mod config;
#[cfg(target_arch = "x86_64")]
mod fma;
mod hero_clip;
mod core_menu;
mod core_menu_runtime;
mod crash_context;
mod crash_report;
mod creator;
mod create_skater;
mod create_skater_runtime;
mod custom_graphic;
mod custom_models;
mod customiser;
mod customiser_material;
mod customiser_parts;
mod difficulty;
mod discord_presence;
#[cfg(feature = "dlss")]
mod dlss;
mod fps_overlay;
mod friend_skaters;
mod graph_host;
mod graph_runtime;
mod graphic_board;
mod graphic_creator;
mod graphic_editor;
mod graphic_outline;
mod graphic_pages;
mod graphic_shapes;
mod graphic_store;
mod graphic_tile;
mod graphics_menu;
mod grind_world;
mod hall_of_meat;
mod hom_bones;
mod hud_meters;
mod hud_runtime;
mod intro_overlay;
mod input;
mod map_library;
mod map_render;
mod map_transition;
mod modding;
mod multiplayer;
mod music;
mod music_ui;
mod old_minimap;
mod online_boundary;
mod online_chat;
mod online_hud;
mod match_core;
mod online_menu;
mod proposal_tab;
mod online_moves;
mod online_roster;
mod online_scoring;
mod online_session;
mod party_play;
mod performance;
mod physics;
mod player_list;
mod popup_runtime;
mod presence;
mod presentation;
mod profiling;
mod render_capacity;
mod replay;
mod retail_character;
mod retail_irradiance;
mod retail_render;
mod retail_ui;
mod scoring_hud;
/// Not retail (user 10-19): the Halloween and Christmas modes.
mod seasonal;
mod saves; // F2: Not retail (user 10-13)
mod scoring_runtime;
mod server_browser; // Not retail (user 10-25)
mod session_details; // Not retail (user 10-25)
mod session_marker;
mod setup;
mod skate1_challenge;
mod skate1_missions;
mod skate1_pros;
mod skate1_hud;
mod skate1_online;
mod skate1_scenes;
mod skate1_cast;
mod skate1_rider;
mod skate1_online_hud;
mod skate1_screens;
mod skate2_arrows;
mod skate2_cast;
mod traffic;
mod skate2_contests;
mod skate2_death_race;
mod skate2_hall_of_meat;
mod skate2_screens;
mod skate2_challenge;
mod skate2_hud;
mod skate2_physics;
/// The Skate 2 tutorial AI's path driver (round2 T5).
mod skate2_rider;
mod skate2_skate_game;
mod skate3_contest;
mod skate3_domination;
mod skate3_modes;
mod skate3_objectives;
mod skate3_one_up;
mod skate3_ots;
mod skate3_film;
mod skate3_otl;
mod skate3_race;
mod skate3_uniform;
mod skate3_skate_game;
mod share_code;
mod skate_audio;
mod skate_world;
mod skater_animation;
mod team_data;
mod team_photo;
mod team_select;
mod teleport_menu;
mod text_entry;
mod text_fallback;
mod autosave_tag;
mod tip_loader;
mod tip_loader_runtime;
mod trick_guide;
mod trick_stats;
mod updater;
mod skate3_career;
mod skate3_teleport; // story mode stage 1. Not tested in play.
mod skate3_tutorial; // story mode stage 1. Not tested in play.
mod skate3_paths; // story mode stage 1. Not tested in play.
mod skate3_follow; // story mode stage 1. Not tested in play.
mod skate3_present; // story mode stage 1. Not tested in play.
mod skate3_indicators; // story mode stage 1. Not tested in play.
mod verification;
mod voice;
mod weather_audio;
mod world;
mod world_time;

/// `--s2-fe-sounds S2_ROOT COLLECTIONS OUT`: the launcher's pipeline exports Skate 2's front-end sounds with it.
/// It needs no game data and opens no window, so it runs before anything reads the installation.
fn s2_fe_sounds_command() -> Option<bevy::app::AppExit> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !args.first().is_some_and(|flag| flag == "--s2-fe-sounds") {
        return None;
    }
    let result = match &args[1..] {
        [root, collections, out] => skate_data::s2_fe_sounds(root.as_ref(), collections.as_ref(), out.as_ref()),
        _ => Err("Usage: skate3rust --s2-fe-sounds S2_ROOT COLLECTIONS OUT".into()),
    };
    Some(match result {
        Ok(()) => bevy::app::AppExit::Success,
        Err(error) => {
            eprintln!("{error}");
            bevy::app::AppExit::error()
        }
    })
}

/// `--warm-audio ASSETS` (user 11-02): the build's last step decodes the sound banks a first play starts with into
/// the PCM cache, so that play has its sounds at once. Like `--s2-fe-sounds` it opens no window and runs before the
/// configuration (which would move saves and load the default map).
fn warm_audio_command() -> Option<bevy::app::AppExit> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !args.first().is_some_and(|flag| flag == "--warm-audio") {
        return None;
    }
    let [_, assets] = args.as_slice() else {
        eprintln!("Usage: skate3rust --warm-audio ASSETS");
        return Some(bevy::app::AppExit::error());
    };
    let started = std::time::Instant::now();
    Some(match skate_audio::player::warm_cache(std::path::Path::new(assets)) {
        Ok(()) => {
            eprintln!("SKATE_AUDIO_WARM done in {:.1}s", started.elapsed().as_secs_f32());
            bevy::app::AppExit::Success
        }
        Err(error) => {
            eprintln!("SKATE_AUDIO_WARM failed: {error}");
            bevy::app::AppExit::error()
        }
    })
}

fn main() -> bevy::app::AppExit {
    map_transition::LAUNCH.get_or_init(std::time::Instant::now);
    if let Some(exit) = s2_fe_sounds_command() {
        return exit;
    }
    if let Some(exit) = warm_audio_command() {
        return exit;
    }
    if let Some(code) = crash_report::entry() {
        std::process::exit(code);
    }
    let _trace = match profiling::init() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("{error}");
            return bevy::app::AppExit::error();
        }
    };
    let _startup = bevy::log::info_span!("startup").entered();
    eprintln!("REPORT_META stage=configuration_and_installation");
    let (config, startup_map) = match bevy::log::info_span!("load_configuration_and_map")
        .in_scope(config::Config::from_env)
    {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return bevy::app::AppExit::error();
        }
    };
    profiling::map_metadata(&config);
    eprintln!(
        "REPORT_META startup=map_fingerprint:{:016x} difficulty:{} multiplayer_requested:{} custom_appearance:{} renderer:Vulkan",
        config.map_fingerprint,
        config.difficulty.key(),
        config.multiplayer.host.is_some() || config.multiplayer.direct.is_some(),
        config.multiplayer.appearance.is_some()
    );
    // A CPU without FMA takes the physics solver's emulated path (skate-core solver::Emulated) and fma.rs's `fmaf`.
    #[cfg(target_arch = "x86_64")]
    if !std::is_x86_feature_detected!("fma") {
        eprintln!("REPORT_META cpu=no_fma");
    }
    // The menus' manifests and textures read while everything below loads (apt_canvas::prefetch).
    if !config.check_assets {
        let root = &config.asset_root;
        let mut menus = boot::manifests(root);
        menus.extend([core_menu::manifest(root), challenge_map::manifest(root)]);
        menus.extend(create_skater::manifests(root));
        // perf 10-18: the loading screen (2.6 MB) and the trick HUD, read on the main thread at setup before.
        if config::perf_on("SKATE_PREFETCH_MORE") {
            menus.extend([tip_loader::manifest(root), scoring_hud::manifest(root)]);
        }
        apt_canvas::prefetch(menus);
    }
    let Loaded { manifest, graphs, physics, skater, controls } = match load_startup(&config) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{error}");
            return bevy::app::AppExit::error();
        }
    };
    if config.check_assets {
        if let Err(error) = camera::CameraRuntime::load(&config.asset_root) {
            eprintln!("{error}");
            return bevy::app::AppExit::error();
        }
        eprintln!("SKATE_ASSETS_READY");
        return bevy::app::AppExit::Success;
    }
    eprintln!("REPORT_META stage=renderer_and_app_initialization");
    // perf 10-19: SKATE_CONTROLS_ONCE: load_startup's controls (read on a thread) are the only load; PhysicsPlugin
    // read them again serially and this insert then replaced its copy.
    let (early, late) = if config::perf_on("SKATE_CONTROLS_ONCE") { (Some(controls), None) } else { (None, Some(controls)) };
    let mut app = app::build(config, manifest, graphs, physics, skater, early);
    if let Some(controls) = late {
        app.insert_resource(controls);
    }
    if let Some(path) = startup_map {
        app.insert_resource(map_transition::StartupMap(path));
    }
    eprintln!("REPORT_META stage=app_run");
    drop(_startup);
    let exit = app.run();
    saves::flush(); // perf 10-18: saves still queued when the app ends
    exit
}

/// What main() loads before the app.
pub(crate) struct Loaded {
    pub manifest: skate_data::GameAssets,
    pub graphs: graph_runtime::StockGraphs,
    pub physics: physics::GamePhysics,
    pub skater: physics::SkaterRuntime,
    pub controls: physics::PlayerControls,
}

/// main()'s loads before the app, side by side on scoped threads (perf 10-12): the physics; the manifest, graphs and
/// the skater's physics-free part (SkaterData); the controls. The skater finishes once the physics is there.
/// Results, their REPORT_META stages and the first error still come in the order these always ran.
pub(crate) fn load_startup(config: &config::Config) -> Result<Loaded, String> {
    let (root, map, difficulty) = (config.asset_root.as_path(), config.map.as_ref(), config.difficulty);
    // First: the physics only loads a map that passed (its error is reported at its stage below).
    let validation = map.map_or(Ok(()), skate_world::validate_runtime);
    std::thread::scope(|scope| {
        let physics = validation.is_ok().then(|| {
            scope.spawn(|| {
                bevy::log::info_span!("load_physics")
                    .in_scope(|| physics::GamePhysics::load_with_difficulty(root, map, difficulty))
            })
        });
        let manifest_graphs = scope.spawn(|| -> Result<_, String> {
            let manifest = bevy::log::info_span!("load_manifest")
                .in_scope(|| skate_data::GameAssets::load(root))
                .map_err(|e| e.to_string())?;
            let graphs = bevy::log::info_span!("load_graphs").in_scope(|| graph_runtime::StockGraphs::load(root, &manifest));
            let skater = graphs.as_ref().ok().map(|graphs| physics::SkaterData::load(root, graphs, None));
            Ok((manifest, graphs, skater))
        });
        let controls = scope.spawn(|| physics::PlayerControls::load(root));
        eprintln!("REPORT_META stage=gameplay_configuration");
        skate_data::input_config::StockGameplayConfig::load(root).map_err(|e| e.to_string())?;
        eprintln!("REPORT_META stage=asset_manifest");
        let (manifest, graphs, skater) = join(manifest_graphs)?;
        eprintln!("REPORT_META stage=stock_graphs");
        let graphs = graphs?;
        eprintln!("REPORT_META stage=map_validation");
        validation?;
        eprintln!("SKATE_DIFFICULTY mode={} native_index={}", difficulty.key(), difficulty as u32);
        eprintln!("REPORT_META stage=physics_initialization");
        let mut physics = join(physics.expect("spawned after validation"))?;
        if config.multiplayer.spawn_offset != 0.0 {
            let mut spawn =
                physics.board.part_transforms()[skate_core::physics::board::BodyId::Deck.index()];
            spawn.translation.x += config.multiplayer.spawn_offset;
            physics.board.set_transform(spawn);
        }
        eprintln!("REPORT_META stage=skater_initialization");
        let skater = skater.expect("loaded with the graphs")?;
        let skater = bevy::log::info_span!("load_skater")
            .in_scope(|| physics::SkaterRuntime::with_data(root, skater, &physics, difficulty.key()))?;
        eprintln!("REPORT_META stage=controls_initialization");
        let controls = join(controls)?;
        Ok(Loaded { manifest, graphs, physics, skater, controls })
    })
}

/// A scoped thread's result; its panic goes on here, as if it had run on this thread.
fn join<T>(thread: std::thread::ScopedJoinHandle<'_, T>) -> T {
    thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[cfg(test)]
#[path = "tests/action_host.rs"]
mod action_host_tests;
