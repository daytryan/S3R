use super::*;
use bevy::log::{
    tracing::{self, span::Id},
    tracing_subscriber::{self, Layer, layer::Context, prelude::*, registry::LookupSpan},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

thread_local! {
    static ENTERED: RefCell<Vec<(Id, Instant)>> = const { RefCell::new(Vec::new()) };
}

/// Inclusive time per span name.
#[derive(Default)]
struct Spans(Mutex<HashMap<&'static str, (u32, Duration)>>);
impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for &'static Spans {
    fn on_enter(&self, id: &Id, _: Context<'_, S>) {
        ENTERED.with(|s| s.borrow_mut().push((id.clone(), Instant::now())));
    }
    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(start) = ENTERED.with(|s| s.borrow_mut().pop().filter(|(i, _)| i == id).map(|(_, t)| t)) else {
            return;
        };
        if let Some(span) = ctx.span(id) {
            let mut spans = self.0.lock().unwrap();
            let entry = spans.entry(span.metadata().name()).or_default();
            entry.0 += 1;
            entry.1 += start.elapsed();
        }
    }
}

/// Measurement only (run with --release): 10 s of pushing and carving on SKATE_MAP_TEST_PATH (flat ground
/// when unset), printing the mean microseconds per tick of each physics span and of the whole tick.
#[test]
#[ignore = "requires private stock skater, animation banks and collections; timing, run with --release"]
fn physics_tick_cost() {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    let manifest = skate_data::GameAssets::load(root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(root, &manifest).unwrap();
    let map = std::env::var_os("SKATE_MAP_TEST_PATH").filter(|p| !p.is_empty()).map(|p| skate_data::skate_map::SkateMap::load(std::path::Path::new(&p)).unwrap());
    let mut physics = GamePhysics::load_with_world(root, ground::Terrain::Flat, map.as_ref()).unwrap();
    let mut skater = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
    let mut controls = PlayerControls::default();
    let mut camera = crate::camera::CameraRuntime::load(root).unwrap();
    let mut input = crate::input::ControllerInput::default();
    let step = physics.settings.step.simulation.time_step;
    let spans: &'static Spans = Box::leak(Box::default());
    let subscriber = tracing_subscriber::registry().with(spans);
    const WARMUP: u32 = 60;
    const TICKS: u32 = 600;
    let mut total = Duration::ZERO;
    tracing::subscriber::with_default(subscriber, || {
        for tick in 0..WARMUP + TICKS {
            if tick == WARMUP {
                spans.0.lock().unwrap().clear();
            }
            // Push 3 s, then carve left and right.
            let pushing = tick < 180;
            input.sample_raw_for_test(skate_core::input::xbox::XboxState {
                buttons: if pushing && tick % 40 < 30 { 0x1000 } else { 0 },
                triggers: [0; 2],
                left: if pushing { [0; 2] } else if tick % 240 < 120 { [-20000, 0] } else { [20000, 0] },
                right: [0; 2],
            });
            let mut actions = input.player_actions();
            controls.update(&mut actions, step, physics.settings.input_magnitude_threshold, skater.player_input.physical.scoring.capabilities_204);
            let start = Instant::now();
            frame::advance(&mut physics, &mut skater, &mut controls, &graphs, &mut actions, true, &mut camera)
                .unwrap_or_else(|e| panic!("tick{tick}: {e}"));
            if tick >= WARMUP {
                total += start.elapsed();
            }
        }
    });
    let mut rows: Vec<_> = spans.0.lock().unwrap().iter().map(|(n, (c, d))| (*n, *c, *d)).collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    eprintln!("SKATE_TICK_COST map={:?} tick_us={:.1} contacts={}", map.as_ref().map(|m| &m.name), total.as_secs_f64() * 1e6 / f64::from(TICKS), physics.contact_count);
    for (name, count, d) in rows {
        eprintln!("  {:>8.1} us/tick {count:>6}x {name}", d.as_secs_f64() * 1e6 / f64::from(TICKS));
    }
}

/// Perf 10-12 measurement: the main thread's work from launch to the first frame, step by step, on the installed
/// default map (SKATE3_ASSET_ROOT): main()'s loads before the app (serially, then as `load_startup` runs them), the
/// Startup scene (BC7 as on a BC-capable GPU) and the PostStartup menu loads (manifest parse, movie load, texture
/// reads), read on this thread and then taken from the prefetch started before the scene.
/// `cargo test ... startup_cost -- --ignored --nocapture`
#[test]
#[ignore = "requires the installation; timing"]
fn startup_cost() {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let root = root.canonicalize().unwrap();
    let spans: &'static Spans = Box::leak(Box::default());
    let subscriber = tracing_subscriber::registry().with(spans);
    let mut steps: Vec<(&str, Duration)> = Vec::new();
    let mut time = |name, t: Instant| steps.push((name, t.elapsed()));
    tracing::subscriber::with_default(subscriber, || {
        let path = crate::map_library::default_map(&root).unwrap().expect("a default map");
        let t = Instant::now();
        drop(skate_data::skate_map::SkateMap::load(&path).unwrap());
        time("  (map parse alone)", t);
        let t = Instant::now();
        crate::config::map_fingerprint(Some(&path)).unwrap();
        time("  (map fingerprint alone)", t);
        let start = Instant::now();
        let t = Instant::now();
        let (map, _) = crate::config::load_map(&path).unwrap();
        time("load_map (parse + fingerprint threads)", t);
        let t = Instant::now();
        skate_data::input_config::StockGameplayConfig::load(&root).unwrap();
        time("StockGameplayConfig::load", t);
        let t = Instant::now();
        let manifest = skate_data::GameAssets::load(&root).unwrap();
        time("GameAssets::load", t);
        let t = Instant::now();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &manifest).unwrap();
        time("StockGraphs::load", t);
        let t = Instant::now();
        crate::skate_world::validate_runtime(&map).unwrap();
        time("validate_runtime", t);
        let difficulty = crate::difficulty::Difficulty::default();
        let t = Instant::now();
        let physics = GamePhysics::load_with_difficulty(&root, Some(&map), difficulty).unwrap();
        time("GamePhysics::load_with_difficulty", t);
        let t = Instant::now();
        drop(SkaterRuntime::load(&root, &graphs, &physics, difficulty.key()).unwrap());
        time("SkaterRuntime::load", t);
        let t = Instant::now();
        drop(PlayerControls::load(&root).unwrap());
        time("PlayerControls::load", t);
        let serial = start.elapsed();
        let config = crate::config::Config {
            asset_root: root.clone(),
            verification_capture: None,
            map: Some(map),
            map_path: Some(path),
            difficulty,
            check_assets: false,
            start_paused: false,
            multiplayer: crate::multiplayer::Options::default(),
            map_fingerprint: 0,
            teleport: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
        };
        let t = Instant::now();
        drop(crate::load_startup(&config).unwrap());
        time("load_startup (the same loads but load_map, side by side)", t);
        let map = config.map.as_ref().unwrap();
        // PostStartup menus as their setups read them, on this thread.
        let menus: Vec<(&'static str, Vec<std::path::PathBuf>)> = vec![
            ("boot screens (10 manifests + textures)", crate::boot::manifests(&root)),
            ("core menu", vec![crate::core_menu::manifest(&root)]),
            ("challenge map", vec![crate::challenge_map::manifest(&root)]),
            ("create-a-skater + popup", crate::create_skater::manifests(&root).into()),
        ];
        assert_eq!(menus[0].1.len(), 10, "the boot sequence runs without arguments");
        let setups = |prefetched: bool, time: &mut dyn FnMut(&'static str, Instant)| {
            for &(name, ref manifests) in &menus {
                let t = Instant::now();
                for manifest in manifests {
                    let json = prefetched.then(|| crate::apt_canvas::prefetched_json(manifest)).flatten().unwrap_or_else(|| {
                        serde_json::from_slice::<serde_json::Value>(&std::fs::read(manifest).unwrap()).unwrap()
                    });
                    if json.get("format").is_some_and(|f| f != "skate3-boot" && f != "skate3-popup") {
                        drop(crate::apt_movie::Movie::load(&json).unwrap());
                    }
                    let folder = manifest.parent().and_then(std::path::Path::parent).unwrap();
                    if json.get("shapes").is_some() {
                        for (texture, size) in crate::apt_canvas::movie_textures(&json).unwrap() {
                            crate::apt_canvas::texture_bytes(folder, &texture, size).unwrap();
                        }
                    }
                }
                time(name, t);
            }
        };
        setups(false, &mut time);
        let before_app = serial;
        // world::spawn (Startup): the scene, textures BC7-encoded as on the player's GPU; main() started the prefetch.
        crate::apt_canvas::prefetch(menus.iter().flat_map(|(_, m)| m.clone()).collect());
        crate::skate_world::assume_bc7();
        let mut world = bevy::ecs::world::World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
        world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
        world.init_resource::<Assets<Image>>();
        let t = Instant::now();
        let mut scene = crate::map_render::PreparedScene::new(&world);
        scene.prepare(Some(map), &root);
        time("PreparedScene::prepare (prefetch running)", t);
        let t = Instant::now();
        scene.publish(&mut world);
        time("PreparedScene::publish", t);
        setups(true, &mut time);
        crate::apt_canvas::drop_prefetch();
        eprintln!("SKATE_STARTUP_COST map={:?} before_app_serial_ms={:.0}", map.name, before_app.as_secs_f64() * 1e3);
    });
    for (name, d) in &steps {
        eprintln!("  {:>8.1} ms {name}", d.as_secs_f64() * 1e3);
    }
    let mut rows: Vec<_> = spans.0.lock().unwrap().iter().map(|(n, (c, d))| (*n, *c, *d)).collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    eprintln!("spans (inclusive, every thread):");
    for (name, count, d) in rows.into_iter().take(25) {
        eprintln!("  {:>8.1} ms {count:>6}x {name}", d.as_secs_f64() * 1e3);
    }
}
