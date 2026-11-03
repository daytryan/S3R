//! Construction/lifetime checks only. Never advance physics or drive gameplay.
use super::*;

#[test]
fn a_pending_request_cannot_be_replaced_by_another_menu_action() {
    let mut transition = MapTransition::default();
    transition.request(Entry {
        label: "first".into(),
        path: None,
    });
    transition.request(Entry {
        label: "second".into(),
        path: Some("other.skate".into()),
    });
    assert!(transition.busy());
    assert!(matches!(&transition.phase, Phase::Requested(entry) if entry.label == "first"));
}

/// B1 (online audit 10-06): a Challenge Map / Party Play pick on another map queues its exact spot (a S.K.A.T.E.
/// row has no teleport id). The new world must be built there; left queued, teleport_menu ran it before the new
/// skater's first tick, from the package spawn across the map.
#[test]
fn a_queued_spot_is_where_the_new_world_spawns() {
    let mut world = World::new();
    let spot = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [154.9, 64.7, -194.6, 1.]];
    let mut travel = crate::teleport_menu::Travel::default();
    travel.pending_matrix = Some(spot);
    world.insert_resource(travel);
    assert_eq!(take_pending_spot(&mut world), Some(spot));
    let travel = world.resource::<crate::teleport_menu::Travel>();
    assert!(travel.pending_matrix.is_none() && travel.pending.is_none(), "nothing left to teleport after the load");
}

/// The live world a transition runs against: the test world committed, the stock skater, controls and graphs.
fn installed_world(root: &std::path::Path) -> (World, Entity) {
    let mut world = World::new();
    let character = install(&mut world, root);
    (world, character)
}

/// `installed_world` into an existing world (an App's, when asset maintenance must run).
fn install(world: &mut World, root: &std::path::Path) -> Entity {
    let root = root.to_path_buf();
    let manifest = skate_data::GameAssets::load(&root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(&root, &manifest).unwrap();
    let difficulty = crate::difficulty::Difficulty::Hardcore;
    let physics = GamePhysics::load_with_difficulty(&root, None, difficulty).unwrap();
    let skater = SkaterRuntime::load(&root, &graphs, &physics, difficulty.key()).unwrap();
    let mut controls = PlayerControls::load(&root).unwrap();
    controls.preferences = skate_core::input::riding_intentions::PushPreferences {
        automatic_push_enabled: true,
        automatic_push_right: true,
    };
    world.insert_resource(Config {
        asset_root: root.clone(),
        verification_capture: None,
        hero_clip: None,
        skate2_physics_dir: None,
        physics_override: None,
        map: None,
        map_path: None,
        difficulty,
        check_assets: false,
        start_paused: false,
        teleport: None,
        multiplayer: Default::default(),
        map_fingerprint: crate::config::map_fingerprint(None).unwrap(),
    });
    world.insert_resource(CurrentMap::from_package(None, None));
    world.insert_resource(graphs);
    world.insert_resource(physics);
    world.insert_resource(skater);
    world.insert_resource(controls);
    world.init_resource::<ButtonInput<KeyCode>>();
    world.init_resource::<ButtonInput<MouseButton>>();
    world.init_resource::<crate::replay::Replay>();
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<Image>>();
    world.init_resource::<Assets<StandardMaterial>>();
    world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
    let character = world
        .spawn((crate::world::PlayerRoot, Transform::default()))
        .id();
    let mut initial = PreparedScene::new(world);
    initial.prepare(None, &root);
    initial.publish(world);
    character
}

#[test]
#[ignore = "requires private installed assets; CPU lifecycle only, no gameplay"]
fn installed_worlds_prepare_commit_and_retire_without_simulation() {
    let root = PathBuf::from(std::env::var("SKATE_TRANSITION_TEST_ASSETS").unwrap())
        .canonicalize()
        .unwrap();
    let (mut world, character) = installed_world(&root);
    let difficulty = crate::difficulty::Difficulty::Hardcore;
    let source = world.resource::<SkaterRuntime>().animation.source.clone();
    let evaluator = world.resource::<SkaterRuntime>().animation.evaluator.clone();
    let procedural_meshes = world.resource::<Assets<Mesh>>().len();
    let procedural_entities = world
        .query_filtered::<Entity, With<crate::map_render::MapEntity>>()
        .iter(&world)
        .count();

    for (iteration, file) in [
        Some("BlackBoxPark.skate"),
        None,
        Some("University.skate"),
        None,
        Some("BlackBoxPark.skate"),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let entry = Entry {
            label: file.unwrap_or("Test world").into(),
            path: file.map(|name| root.parent().unwrap().join("maps").join(name)),
        };
        let old: Vec<_> = world
            .query_filtered::<Entity, With<crate::map_render::MapEntity>>()
            .iter(&world)
            .collect();
        world.resource_mut::<GamePhysics>().contact_count = 123;
        world.resource_mut::<SkaterRuntime>().pose_generation = 999;
        let Phase::Loading {
            job, mut streamed, ..
        } = start(&world, entry, None).unwrap()
        else {
            panic!("expected worker");
        };
        let prepared = job.join().unwrap().unwrap();
        assert!(
            old.iter().all(|&id| world.get_entity(id).is_ok()),
            "preparation changed live scene"
        );
        // As poll() does: release the old world, then upload the new one before commit.
        crate::map_render::MapAssets::retire(&mut world);
        assert!(old.iter().all(|&id| world.get_entity(id).is_err()));
        assert_eq!(
            world.resource::<Assets<Image>>().len(),
            0,
            "old world's images released before upload"
        );
        while !streamed.drain(&mut world) {}
        assert_eq!(world.resource::<GamePhysics>().contact_count, 123);
        let triangles = prepared.physics.world_triangles().len();
        let spawn = prepared.metadata.spawn;
        assert!(Arc::ptr_eq(&prepared.skater.animation.source, &source));
        assert!(Arc::ptr_eq(
            &prepared.skater.animation.evaluator,
            &evaluator
        ));
        // map_change_resets_recording: the old map's replay never plays on the new one.
        world.resource_mut::<crate::replay::Replay>().record(
            crate::presentation::Snapshot { root: Transform::default(), bones: Vec::new(), camera: Transform::default(), fov: 1. },
            1. / 60.,
            false,
        );
        commit(&mut world, prepared);
        assert!(world.resource::<crate::replay::Replay>().frames().is_empty());
        assert!(old.into_iter().all(|id| world.get_entity(id).is_err()));
        assert!(world.get_entity(character).is_ok());
        assert_eq!(
            world.resource::<CurrentMap>().generation,
            iteration as u64 + 1
        );
        assert_eq!(world.resource::<CurrentMap>().spawn, spawn);
        let physics = world.resource::<GamePhysics>();
        assert_eq!(physics.world_triangles().len(), triangles);
        assert_eq!(physics.contact_count, 0);
        assert_eq!(physics.ticks, 0);
        assert!(!physics.failed);
        assert_eq!(physics.difficulty_index(), difficulty as u32);
        assert_eq!(world.resource::<SkaterRuntime>().pose_generation, 0);
        assert_eq!(world.resource::<PlayerControls>().ticks, 0);
        assert!(
            world
                .resource::<PlayerControls>()
                .preferences
                .automatic_push_enabled
        );
        assert!(
            world
                .resource::<PlayerControls>()
                .preferences
                .automatic_push_right
        );
        assert!(
            world
                .resource::<crate::presentation::Presentation>()
                .pair()
                .is_none()
        );
        assert!(!world.resource::<crate::replay::Replay>().active);
        assert!(
            world
                .resource::<crate::camera::CameraRuntime>()
                .frame
                .is_none()
        );
        if file.is_none() {
            assert_eq!(world.resource::<Assets<Mesh>>().len(), procedural_meshes);
            assert_eq!(world.resource::<Assets<Image>>().len(), 0);
            assert_eq!(
                world
                    .resource::<Assets<crate::retail_render::RetailSkyMaterial>>()
                    .len(),
                0
            );
            assert_eq!(
                world
                    .resource::<Assets<crate::retail_render::RetailWorldMaterial>>()
                    .len(),
                0
            );
            assert_eq!(world.resource::<Assets<crate::retail_render::RetailBlendMaterial>>().len(), 0);
            assert_eq!(
                world
                    .query_filtered::<Entity, With<crate::map_render::MapEntity>>()
                    .iter(&world)
                    .count(),
                procedural_entities
            );
        }
        eprintln!(
            "LIFECYCLE_CHECK generation={} map={file:?} triangles={triangles} entities={} meshes={} images={}",
            iteration + 1,
            world
                .query_filtered::<Entity, With<crate::map_render::MapEntity>>()
                .iter(&world)
                .count(),
            world.resource::<Assets<Mesh>>().len(),
            world.resource::<Assets<Image>>().len()
        );
    }
    let missing = Entry {
        label: "missing".into(),
        path: Some(root.join("nonexistent-transition-check.skate")),
    };
    let Phase::Loading { job, .. } = start(&world, missing, None).unwrap() else {
        panic!("expected worker");
    };
    assert!(job.join().unwrap().is_err());
    assert_eq!(world.resource::<CurrentMap>().generation, 6);
    assert_eq!(world.resource::<Assets<Mesh>>().len(), procedural_meshes);
    assert!(world.get_entity(character).is_ok());
}

/// Where a map load spends its time (SKATE_TRANSITION_TEST_ASSETS, SKATE_TIMING_MAP = a .skate).
#[test]
#[ignore = "requires private installed assets and a converted map"]
fn map_load_stage_timings() {
    let root = PathBuf::from(std::env::var("SKATE_TRANSITION_TEST_ASSETS").unwrap())
        .canonicalize()
        .unwrap();
    let path = PathBuf::from(std::env::var("SKATE_TIMING_MAP").unwrap());
    let manifest = skate_data::GameAssets::load(&root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(&root, &manifest).unwrap();
    let difficulty = crate::difficulty::Difficulty::Normal;
    let time = |name: &str, started: Instant| {
        eprintln!("STAGE {name} {} ms", started.elapsed().as_millis())
    };
    let t = Instant::now();
    let map = skate_data::skate_map::SkateMap::load(&path).unwrap();
    time("read", t);
    let t = Instant::now();
    crate::config::map_fingerprint(Some(&path)).unwrap();
    time("fingerprint", t);
    let t = Instant::now();
    crate::skate_world::validate_runtime(&map).unwrap();
    time("validate", t);
    let t = Instant::now();
    let physics = GamePhysics::load_with_difficulty(&root, Some(&map), difficulty).unwrap();
    time("physics", t);
    let t = Instant::now();
    let first =
        SkaterRuntime::load_for_world(&root, &graphs, &physics, difficulty.key(), None).unwrap();
    time("skater_first", t);
    // A transition reuses the live animation source.
    let t = Instant::now();
    let _skater = SkaterRuntime::load_for_world(
        &root,
        &graphs,
        &physics,
        difficulty.key(),
        Some(first.animation.source.clone()),
    )
    .unwrap();
    time("skater", t);
    let t = Instant::now();
    PlayerControls::load(&root).unwrap();
    time("controls", t);
    let t = Instant::now();
    crate::camera::CameraRuntime::load(&root).unwrap();
    time("camera", t);
    let mut world = World::new();
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<Image>>();
    world.init_resource::<Assets<StandardMaterial>>();
    world.init_resource::<Assets<crate::retail_render::PropMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailWorldMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailBlendMaterial>>();
    world.init_resource::<Assets<crate::retail_render::RetailSkyMaterial>>();
    let t = Instant::now();
    let mut scene = PreparedScene::new(&world);
    scene.prepare(Some(&map), &root);
    time("render", t);
    scene.publish(&mut world);
    let images = world.resource::<Assets<Image>>();
    let mut by_format = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for (_, image) in images.iter() {
        let entry = by_format
            .entry(format!("{:?}", image.texture_descriptor.format))
            .or_default();
        entry.0 += 1;
        entry.1 += image.data.as_ref().map_or(0, Vec::len);
    }
    for (format, (count, bytes)) in by_format {
        eprintln!("IMAGES {format} count={count} MB={}", bytes >> 20);
    }
    eprintln!("MESHES count={}", world.resource::<Assets<Mesh>>().len());
}

/// Memory 10-12: University <-> Skate 1 SV x4 through the real loader and commit (skater, camera, controls rebuilt
/// each time; the old scene retired, the new one drained in), process memory after each commit. The CPU side of a
/// switch should come back to the same level every visit. `SKATE3_ASSET_ROOT=…`
#[test]
#[ignore = "requires the installation; measurement, run alone with --nocapture"]
fn switches_return_cpu_memory() {
    let root = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).canonicalize().unwrap();
    // An App, not a bare World: the game's asset maintenance (handle drops recycling slots, queued asset events
    // flushed) must run between frames or the Assets storage itself grows every switch.
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
    app.init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .init_asset::<crate::retail_render::PropMaterial>()
        .init_asset::<crate::retail_render::RetailWorldMaterial>()
        .init_asset::<crate::retail_render::RetailBlendMaterial>()
        .init_asset::<crate::retail_render::RetailSkyMaterial>();
    install(app.world_mut(), &root);
    app.update();
    crate::skate_world::assume_bc7();
    let mut levels: std::collections::HashMap<&str, Vec<usize>> = Default::default();
    for cycle in 0..4 {
        for name in ["Skate_1_San_Vanelona", "University"] {
            let entry = Entry { label: name.into(), path: Some(root.parent().unwrap().join("maps").join(format!("{name}.skate"))) };
            let Phase::Loading { job, mut streamed, .. } = start(app.world(), entry, None).unwrap() else { panic!("expected worker") };
            let prepared = job.join().unwrap().unwrap();
            crate::map_render::MapAssets::retire(app.world_mut());
            app.update();
            while !streamed.drain(app.world_mut()) {
                app.update();
            }
            drop(streamed);
            commit(app.world_mut(), prepared);
            let world = app.world_mut();
            // What extraction does: render-only data leaves the main world.
            for (_, image) in world.resource_mut::<Assets<Image>>().iter_mut() {
                image.data = None;
            }
            for (_, mesh) in world.resource_mut::<Assets<Mesh>>().iter_mut() {
                drop(mesh.take_gpu_data());
            }
            app.update();
            app.update();
            let world = app.world();
            let m = crate::performance::process_memory(true);
            eprintln!(
                "SKATE_SWITCH cycle {cycle} {name:<22} private {:>6.0} ws {:>6.0} heap live {:>6.0} committed {:>6.0} MB \
                 (images {} meshes {} entities {})",
                m.private as f64 / 1048576.,
                m.working_set as f64 / 1048576.,
                m.heap_live as f64 / 1048576.,
                m.heap_committed as f64 / 1048576.,
                world.resource::<Assets<Image>>().len(),
                world.resource::<Assets<Mesh>>().len(),
                world.entities().len(),
            );
            levels.entry(name).or_default().push(m.heap_live);
        }
    }
    // Live heap per map: the last visit within 32 MB of the second (the first warms caches).
    for (name, live) in levels {
        let (second, last) = (live[1] as f64 / 1048576., live[3] as f64 / 1048576.);
        assert!((last - second).abs() < 32., "{name}: heap live {second:.0} -> {last:.0} MB");
    }
}

/// Bisects `switches_return_cpu_memory`: each part the loader builds, rebuilt and dropped four times, then whole
/// test-world switches through start/commit; heap live after each.
#[test]
#[ignore = "requires the installation; measurement, run alone with --nocapture"]
fn switch_parts_return_cpu_memory() {
    let root = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).canonicalize().unwrap();
    let (mut world, _) = installed_world(&root);
    let difficulty = world.resource::<Config>().difficulty;
    let graphs = world.resource::<crate::graph_runtime::StockGraphs>().clone();
    let source = world.resource::<SkaterRuntime>().animation.source.clone();
    let physics = GamePhysics::load_at(&root, None, difficulty, None).unwrap();
    let live = |what: &str| {
        let m = crate::performance::process_memory(true);
        eprintln!("SKATE_SWITCH_PART {what:<28} heap live {:>7.1} committed {:>7.1} MB", m.heap_live as f64 / 1048576., m.heap_committed as f64 / 1048576.);
    };
    for cycle in 0..4 {
        drop(SkaterRuntime::load_for_world(&root, &graphs, &physics, difficulty.key(), Some(source.clone())).unwrap());
        live(&format!("{cycle} skater"));
        drop(PlayerControls::load(&root).unwrap());
        live(&format!("{cycle} controls"));
        drop(crate::camera::CameraRuntime::load(&root).unwrap());
        live(&format!("{cycle} camera"));
        drop(GamePhysics::load_at(&root, None, difficulty, None).unwrap());
        live(&format!("{cycle} physics (test world)"));
    }
    for cycle in 0..4 {
        let entry = Entry { label: "Test world".into(), path: None };
        let Phase::Loading { job, mut streamed, .. } = start(&world, entry, None).unwrap() else { panic!("expected worker") };
        let prepared = job.join().unwrap().unwrap();
        live(&format!("{cycle} test world prepared"));
        crate::map_render::MapAssets::retire(&mut world);
        while !streamed.drain(&mut world) {}
        drop(streamed);
        commit(&mut world, prepared);
        live(&format!("{cycle} test world committed"));
    }
}
