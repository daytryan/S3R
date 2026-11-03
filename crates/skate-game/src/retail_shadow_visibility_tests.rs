use super::*;
fn frustum(radius: f32, x: f32) -> Frustum {
    Frustum::from_clip_from_world(
        &(Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, 300.)
            * Mat4::look_at_rh(Vec3::new(x, 20., 80.), Vec3::new(x, 0., 0.), Vec3::Y)),
    )
}
#[test]
fn spatial_tree_matches_flat_obb_checks_with_rotations_and_layers() {
    let mut world = World::new();
    let mut entries = vec![];
    for i in 0..4096 {
        entries.push(Entry {
            entity: world.spawn_empty().id(),
            aabb: Aabb {
                center: bevy::math::Vec3A::ZERO,
                half_extents: bevy::math::Vec3A::new(1., 2., 3.),
            },
            transform: GlobalTransform::from(
                Transform::from_xyz((i % 64) as f32 * 8. - 256., 0., (i / 64) as f32 * 8. - 256.)
                    .with_rotation(Quat::from_rotation_y(i as f32 * 0.17))
                    .with_scale(Vec3::new(1., 2., 0.5)),
            ),
            visible: i % 9 != 0,
            layers: RenderLayers::layer(i % 2),
        });
    }
    let mut index = Index::default();
    index.rebuild(entries);
    for x in [-200., -30., 0., 45., 180.] {
        for layer in [0, 1] {
            for radius in [5., 40., 150.] {
                let f = frustum(radius, x);
                let mask = RenderLayers::layer(layer);
                let mut expected: Vec<_> = index
                    .entries
                    .iter()
                    .filter(|e| {
                        e.visible
                            && e.layers.intersects(&mask)
                            && f.intersects_obb(&e.aabb, &e.transform.affine(), false, true)
                    })
                    .map(|e| e.entity)
                    .collect();
                let mut actual = vec![];
                index.visit(0, &f, false, &mask, &mut actual);
                expected.sort();
                actual.sort();
                assert_eq!(actual, expected);
            }
        }
    }
}
#[test]
fn indexed_system_matches_upstream_across_changes_and_removal() {
    bevy::tasks::ComputeTaskPool::get_or_init(Default::default);
    let mut world = World::new();
    world.init_resource::<Index>();
    let view = world.spawn_empty().id();
    let visibility = ViewVisibility::HIDDEN;
    let mut frusta = CascadesFrusta::default();
    frusta
        .frusta
        .insert(view, vec![frustum(10., 0.), frustum(100., 0.)]);
    let light = world
        .spawn((
            DirectionalLight {
                shadows_enabled: true,
                ..default()
            },
            frusta,
            CascadesVisibleEntities::default(),
            visibility,
        ))
        .id();
    world
        .get_mut::<ViewVisibility>(light)
        .unwrap()
        .set_visible();
    let mut entities = vec![];
    for i in 0..80 {
        let mut e = world.spawn((
            Mesh3d::default(),
            Aabb {
                center: bevy::math::Vec3A::ZERO,
                half_extents: bevy::math::Vec3A::ONE,
            },
            GlobalTransform::from_translation(Vec3::new(i as f32 * 5. - 200., 0., 0.)),
            InheritedVisibility::VISIBLE,
        ));
        if i % 2 == 0 {
            e.insert(StaticShadowCaster);
        }
        if i % 7 == 0 {
            e.insert(NoFrustumCulling);
        }
        if i % 11 == 0 {
            e.insert(RenderLayers::layer(28));
        }
        entities.push(e.id());
    }
    let mut original = bevy::ecs::schedule::Schedule::default();
    original.add_systems(bevy::light::check_dir_light_mesh_visibility);
    let mut indexed = bevy::ecs::schedule::Schedule::default();
    indexed.add_systems((refresh, check_indexed).chain());
    let read = |world: &World| {
        let v = world.get::<CascadesVisibleEntities>(light).unwrap();
        let mut lists: Vec<Vec<Entity>> = v.entities[&view]
            .iter()
            .map(|e| e.entities.clone())
            .collect();
        for list in &mut lists {
            list.sort();
        }
        lists
    };
    for step in 0..6 {
        match step {
            1 => {
                world
                    .entity_mut(entities[0])
                    .insert(GlobalTransform::from_translation(Vec3::ZERO));
            }
            2 => {
                world.entity_mut(entities[2]).insert(NotShadowCaster);
                world
                    .entity_mut(entities[4])
                    .insert(InheritedVisibility::HIDDEN);
            }
            3 => {
                world.entity_mut(entities[2]).remove::<NotShadowCaster>();
                world.entity_mut(entities[11]).remove::<RenderLayers>();
            }
            4 => {
                world
                    .entity_mut(light)
                    .get_mut::<DirectionalLight>()
                    .unwrap()
                    .shadows_enabled = false;
            }
            5 => {
                world
                    .entity_mut(light)
                    .get_mut::<DirectionalLight>()
                    .unwrap()
                    .shadows_enabled = true;
                world.despawn(entities[6]);
            }
            _ => {}
        }
        original.run(&mut world);
        let expected = read(&world);
        for &entity in &entities {
            if let Some(mut visibility) = world.get_mut::<ViewVisibility>(entity) {
                *visibility = ViewVisibility::HIDDEN;
            }
        }
        indexed.run(&mut world);
        for &entity in &entities {
            if let Some(visibility) = world.get::<ViewVisibility>(entity) {
                assert_eq!(
                    visibility.get(),
                    expected.iter().any(|list| list.contains(&entity)),
                    "visibility marking at step {step}"
                );
            }
        }
        assert_eq!(read(&world), expected, "step {step}");
    }
    for entity in entities {
        world.despawn(entity);
    }
    indexed.run(&mut world);
    assert!(world.resource::<Index>().entries.is_empty());
}
#[test]
fn replacement_preserves_schedule_ordering() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::asset::AssetPlugin::default(),
        bevy::transform::TransformPlugin,
        bevy::mesh::MeshPlugin,
        bevy::camera::visibility::VisibilityPlugin,
        bevy::light::LightPlugin,
    ));
    install(&mut app);
    finish(&mut app);
}

pub(super) fn benchmark_bounds(bounds: Vec<Aabb>) -> serde_json::Value {
    let mut world = World::new();
    let mut index = Index::default();
    let count = bounds.len();
    let system_bounds = bounds.clone();
    let start = std::time::Instant::now();
    index.rebuild(
        bounds
            .into_iter()
            .map(|aabb| Entry {
                entity: world.spawn_empty().id(),
                aabb,
                transform: GlobalTransform::IDENTITY,
                visible: true,
                layers: default(),
            })
            .collect(),
    );
    let build_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut frusta = vec![];
    for offset in [-50., 0., 50.] {
        for radius in [5., 10., 30., 100.] {
            let center = Vec3::new(330. + offset, 133., -710.);
            let light = Vec3::new(-0.60385966, 0.73098797, 0.31782088);
            frusta.push(Frustum::from_clip_from_world(
                &(Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, 400.)
                    * Mat4::look_at_rh(center + light * 200., center, Vec3::Y)),
            ));
        }
    }
    let mask = RenderLayers::default();
    let mut out = Vec::with_capacity(count);
    let mut expected = Vec::with_capacity(count);
    for f in &frusta {
        out.clear();
        expected.clear();
        index.visit(0, f, false, &mask, &mut out);
        for entry in &index.entries {
            if f.intersects_obb(&entry.aabb, &entry.transform.affine(), false, true) {
                expected.push(entry.entity);
            }
        }
        out.sort();
        expected.sort();
        assert_eq!(out, expected);
    }
    let repetitions = 100;
    let start = std::time::Instant::now();
    for _ in 0..repetitions {
        for f in &frusta {
            out.clear();
            for entry in &index.entries {
                if std::hint::black_box(f).intersects_obb(
                    &entry.aabb,
                    &entry.transform.affine(),
                    false,
                    true,
                ) {
                    out.push(entry.entity);
                }
            }
            std::hint::black_box(&out);
        }
    }
    let flat_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = std::time::Instant::now();
    for _ in 0..repetitions {
        for f in &frusta {
            out.clear();
            index.visit(0, std::hint::black_box(f), false, &mask, &mut out);
            std::hint::black_box(&out);
        }
    }
    let indexed_ms = start.elapsed().as_secs_f64() * 1000.;
    serde_json::json!({"full_system":benchmark_systems(system_bounds),"static_batches":count,"queries":repetitions*frusta.len(),"flat_ms":flat_ms,"indexed_ms":indexed_ms,"build_ms":build_ms,"visible_sets_equal":true,"note":"real University material bounds; representative shadow frusta, single-thread query kernel only, not whole-frame FPS"})
}

fn benchmark_systems(bounds: Vec<Aabb>) -> serde_json::Value {
    bevy::tasks::ComputeTaskPool::get_or_init(Default::default);
    let mut world = World::new();
    world.init_resource::<Index>();
    let view = world.spawn_empty().id();
    let center = Vec3::new(330., 133., -710.);
    let direction = Vec3::new(-0.60385966, 0.73098797, 0.31782088);
    let frusta: Vec<_> = [5., 10., 30., 100.]
        .into_iter()
        .map(|radius| {
            Frustum::from_clip_from_world(
                &(Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, 400.)
                    * Mat4::look_at_rh(center + direction * 200., center, Vec3::Y)),
            )
        })
        .collect();
    for layer in [0, 28] {
        let mut cascades = CascadesFrusta::default();
        cascades.frusta.insert(view, frusta.clone());
        let light = world
            .spawn((
                DirectionalLight {
                    shadows_enabled: true,
                    ..default()
                },
                cascades,
                CascadesVisibleEntities::default(),
                RenderLayers::layer(layer),
            ))
            .id();
        world
            .get_mut::<ViewVisibility>(light)
            .unwrap()
            .set_visible();
    }
    for aabb in bounds {
        world.spawn((
            StaticShadowCaster,
            Mesh3d::default(),
            aabb,
            GlobalTransform::IDENTITY,
            InheritedVisibility::VISIBLE,
        ));
    }
    // A second view has overlapping cascades, deliberately exercising duplicate
    // visibility publication, which the old query-only benchmark omitted.
    let second_view = world.spawn_empty().id();
    for mut cascades in world.query::<&mut CascadesFrusta>().iter_mut(&mut world) {
        cascades.frusta.insert(second_view, frusta.clone());
    }
    let mut old = bevy::ecs::schedule::Schedule::default();
    old.add_systems((refresh, check_indexed).chain());
    let mut upstream = bevy::ecs::schedule::Schedule::default();
    upstream.add_systems(bevy::light::check_dir_light_mesh_visibility);
    old.run(&mut world);
    upstream.run(&mut world);
    let mut old_times = vec![];
    let mut upstream_times = vec![];
    for round in 0..5 {
        let mut run = |schedule: &mut bevy::ecs::schedule::Schedule| {
            let start = std::time::Instant::now();
            for _ in 0..200 {
                schedule.run(&mut world);
            }
            start.elapsed().as_secs_f64() * 1000. / 200.
        };
        if round % 2 == 0 {
            old_times.push(run(&mut old));
            upstream_times.push(run(&mut upstream));
        } else {
            upstream_times.push(run(&mut upstream));
            old_times.push(run(&mut old));
        }
    }
    old_times.sort_by(f64::total_cmp);
    upstream_times.sort_by(f64::total_cmp);
    serde_json::json!({"v9_ms_per_update":old_times[2],"upstream_ms_per_update":upstream_times[2],"note":"complete headless schedules including refresh and deferred writes; real map bounds, fixed representative frusta, two overlapping views, not gameplay FPS"})
}
#[test]
fn camera_drops_static_map_batches_wholly_past_its_far_plane() {
    use bevy::camera::visibility::VisibleEntities;
    use bevy::ecs::system::RunSystemOnce;
    let mut world = World::new();
    let mut spawn = |z: f32, half: f32, static_map: bool| {
        let aabb = Aabb { center: bevy::math::Vec3A::new(0., 0., z), half_extents: bevy::math::Vec3A::splat(half) };
        let mut e = world.spawn((Mesh3d::default(), aabb, GlobalTransform::IDENTITY));
        if static_map {
            e.insert(StaticShadowCaster);
        }
        e.id()
    };
    let near = spawn(-50., 10., true);
    let straddling = spawn(-150., 60., true);
    let far = spawn(-300., 10., true);
    let far_prop = spawn(-300., 10., false);
    let mut visible = VisibleEntities::default();
    visible.entities.insert(std::any::TypeId::of::<Mesh3d>(), vec![near, straddling, far, far_prop]);
    let camera = world
        .spawn((
            Camera::default(),
            Projection::Perspective(PerspectiveProjection { far: 100., ..default() }),
            GlobalTransform::IDENTITY,
            visible,
        ))
        .id();
    world.run_system_once(cull_past_far).unwrap();
    let kept = world.get::<VisibleEntities>(camera).unwrap().get(std::any::TypeId::of::<Mesh3d>()).to_vec();
    assert_eq!(kept, vec![near, straddling, far_prop]);
}
#[test]
fn camera_leaves_far_retail_map_batches_out_of_its_prepass() {
    use bevy::camera::visibility::VisibleEntities;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::pbr::PrepassExcluded;
    use bevy::render::{experimental::occlusion_culling::OcclusionCulling, sync_world::MainEntity};
    let mut world = World::new();
    let mut spawn = |z: f32, static_map: bool, retail: bool| {
        let aabb = Aabb { center: bevy::math::Vec3A::new(0., 0., z), half_extents: bevy::math::Vec3A::splat(10.) };
        let mut e = world.spawn((Mesh3d::default(), aabb, GlobalTransform::IDENTITY));
        if static_map {
            e.insert(StaticShadowCaster);
        }
        if retail {
            e.insert(MeshMaterial3d::<super::super::RetailWorldMaterial>::default());
        }
        e.id()
    };
    let near = spawn(-50., true, true);
    let far = spawn(-PREPASS_FAR - 60., true, true);
    // The PBR fallback loads its normals from the prepass; props move.
    let far_fallback = spawn(-PREPASS_FAR - 60., true, false);
    let far_prop = spawn(-PREPASS_FAR - 60., false, true);
    let mut visible = VisibleEntities::default();
    visible.entities.insert(std::any::TypeId::of::<Mesh3d>(), vec![near, far, far_fallback, far_prop]);
    let projection = Projection::Perspective(PerspectiveProjection { far: 1000., ..default() });
    // A stale entry from the previous frame goes too.
    let stale = PrepassExcluded([MainEntity::from(near)].into_iter().collect());
    let depth = bevy::core_pipeline::prepass::DepthPrepass;
    let camera = world.spawn((Camera::default(), projection.clone(), GlobalTransform::IDENTITY, visible.clone(), stale.clone(), depth.clone())).id();
    // perf 10-18: no prepass, nothing reads the set.
    let no_prepass = world.spawn((Camera::default(), projection.clone(), GlobalTransform::IDENTITY, visible.clone(), stale.clone())).id();
    // Occlusion culling builds its depth pyramid from the prepass: everything stays in it.
    let occluding = world
        .spawn((Camera::default(), projection.clone(), GlobalTransform::IDENTITY, visible.clone(), stale.clone(), OcclusionCulling, depth))
        .id();
    // DLSS reads the prepass depth and motion vectors: everything stays in it too.
    let upscaled = world
        .spawn((Camera::default(), projection, GlobalTransform::IDENTITY, visible, stale, bevy::core_pipeline::prepass::MotionVectorPrepass))
        .id();
    world.run_system_once(cull_past_far).unwrap();
    let excluded = |camera| world.get::<PrepassExcluded>(camera).unwrap().0.iter().map(|e| e.id()).collect::<Vec<_>>();
    assert_eq!(excluded(camera), vec![far]);
    assert!(excluded(no_prepass).is_empty());
    assert!(excluded(occluding).is_empty());
    assert!(excluded(upscaled).is_empty());
    let kept = world.get::<VisibleEntities>(camera).unwrap().get(std::any::TypeId::of::<Mesh3d>()).to_vec();
    assert_eq!(kept, vec![near, far, far_fallback, far_prop], "the main pass still draws them");
}
/// FPS pass 10-23 (final review): batches whose main-pass alpha test the depth shader can't repeat (NoCameraPrepass,
/// skate_world.rs) leave the depth-only prepass at any distance, static or prop. Occlusion, SSAO and DLSS keep them, as shipped.
#[test]
fn camera_leaves_main_pass_only_alpha_tests_out_of_its_prepass() {
    use bevy::camera::visibility::VisibleEntities;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::pbr::PrepassExcluded;
    use bevy::render::experimental::occlusion_culling::OcclusionCulling;
    let mut world = World::new();
    let aabb = Aabb { center: bevy::math::Vec3A::new(0., 0., -50.), half_extents: bevy::math::Vec3A::splat(10.) };
    let material = MeshMaterial3d::<super::super::RetailWorldMaterial>::default();
    let marked = world.spawn((Mesh3d::default(), aabb, GlobalTransform::IDENTITY, StaticShadowCaster, material.clone(), NoCameraPrepass)).id();
    let prop = world.spawn((Mesh3d::default(), aabb, GlobalTransform::IDENTITY, material.clone(), NoCameraPrepass)).id();
    let plain = world.spawn((Mesh3d::default(), aabb, GlobalTransform::IDENTITY, StaticShadowCaster, material)).id();
    let mut visible = VisibleEntities::default();
    visible.entities.insert(std::any::TypeId::of::<Mesh3d>(), vec![marked, prop, plain]);
    let projection = Projection::Perspective(PerspectiveProjection { far: 1000., ..default() });
    let depth = bevy::core_pipeline::prepass::DepthPrepass;
    let none = || PrepassExcluded(Default::default());
    let camera = world.spawn((Camera::default(), projection.clone(), GlobalTransform::IDENTITY, visible.clone(), none(), depth.clone())).id();
    let occluding = world.spawn((Camera::default(), projection.clone(), GlobalTransform::IDENTITY, visible.clone(), none(), OcclusionCulling, depth.clone())).id();
    let ao = bevy::core_pipeline::prepass::NormalPrepass;
    let ambient = world.spawn((Camera::default(), projection, GlobalTransform::IDENTITY, visible, none(), ao, depth)).id();
    world.run_system_once(cull_past_far).unwrap();
    let excluded = |camera| {
        let mut ids = world.get::<PrepassExcluded>(camera).unwrap().0.iter().map(|e| e.id()).collect::<Vec<_>>();
        ids.sort();
        ids
    };
    let mut want = vec![marked, prop];
    want.sort();
    assert_eq!(excluded(camera), want);
    assert!(excluded(occluding).is_empty());
    assert!(excluded(ambient).is_empty(), "SSAO keeps its shipped prepass");
    let kept = world.get::<VisibleEntities>(camera).unwrap().get(std::any::TypeId::of::<Mesh3d>()).to_vec();
    assert_eq!(kept, vec![marked, prop, plain], "the main pass still draws them");
}
#[test]
fn shadow_cascades_cover_only_3d_cameras() {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::light::cascade::Cascades;
    let mut world = World::new();
    let camera3d = world.spawn(Camera3d::default()).id();
    let camera2d = world.spawn(Camera2d).id();
    let mut cascades = Cascades::default();
    cascades.cascades.insert(camera3d, vec![]);
    cascades.cascades.insert(camera2d, vec![]);
    let light = world.spawn(cascades).id();
    world.run_system_once(cascades_for_3d_views).unwrap();
    let views: Vec<Entity> = world.get::<Cascades>(light).unwrap().cascades.keys().copied().collect();
    assert_eq!(views, vec![camera3d]);
}
/// perf 10-18: a cascade's sun-side face moves height / sin(elevation) up the light, past every caster that can
/// shadow something in it; a light at the horizon gets none.
#[test]
fn the_sun_side_face_keeps_every_caster_that_can_reach_the_cascade() {
    // 30 degrees up: a 50 m tall map casts 100 m along the ray.
    let toward_sun = Vec3::new(0., 0.5, -(0.75f32).sqrt());
    let origin = Vec3::new(10., 2., 3.);
    let cascade = Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Z, toward_sun), origin);
    let plane = sun_side(cascade, 50.).unwrap();
    let inside = |s: f32| plane.normal_d().dot((origin + toward_sun * s).extend(1.)) > 0.;
    assert!(inside(-30.) && inside(0.) && inside(99.) && !inside(101.));
    let horizon = Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Z, Vec3::X), origin);
    assert!(sun_side(horizon, 50.).is_none());
}
/// perf 11-01 (vendor prepass node, SKATE_SKIP_DEPTH_COPY): a view without normal or motion-vector prepass textures
/// skips the prepass depth copy, as SSAO, DLSS, TAA and motion blur are the copy's only readers in the game. Nothing
/// else may read it: no game source or shader imports Bevy's prepass bindings or names the copy or its other readers.
#[test]
fn nothing_else_reads_the_prepass_depth_copy() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs" || e == "wgsl") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."), &mut files);
    assert!(files.iter().any(|f| f.ends_with("retail_world.wgsl")) && files.iter().any(|f| f.ends_with("retail_godray.rs")));
    let readers = [
        "prepass_utils", "prepass_bindings", "depth_prepass_texture", "prepass_depth", "depth_view()", "ForwardDecal",
        "_transmission", "ScreenSpaceReflections", "DeferredPrepass",
    ];
    for file in files.iter().filter(|f| !f.ends_with("retail_shadow_visibility_tests.rs")) {
        let text = std::fs::read_to_string(file).unwrap();
        for name in readers {
            assert!(!text.contains(name), "{} names {name}", file.display());
        }
    }
}
/// perf 11-01: a traffic car's meshes (NotShadowCaster) despawning rebuilt the whole index every second or so
/// (6-14 ms). Only a removal on a map caster rebuilds it now; one on an indexed caster still does.
#[test]
fn only_removals_on_map_casters_rebuild_the_index() {
    let mut world = World::new();
    world.init_resource::<Index>();
    let bounds = Aabb { center: bevy::math::Vec3A::ZERO, half_extents: bevy::math::Vec3A::ONE };
    let map = world
        .spawn((Mesh3d::default(), bounds, GlobalTransform::IDENTITY, InheritedVisibility::VISIBLE, StaticShadowCaster, RenderLayers::layer(0)))
        .id();
    let car = world.spawn((Mesh3d::default(), NotShadowCaster, RenderLayers::layer(0))).id();
    let mut schedule = bevy::ecs::schedule::Schedule::default();
    schedule.add_systems(refresh);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Index>().entries.len(), 1);
    // A marker a rebuild would undo.
    world.resource_mut::<Index>().entries[0].visible = false;
    world.despawn(car);
    schedule.run(&mut world);
    assert!(!world.resource::<Index>().entries[0].visible, "a car's removal left the index alone");
    world.entity_mut(map).remove::<RenderLayers>();
    schedule.run(&mut world);
    assert!(world.resource::<Index>().entries[0].visible, "a map caster's removal rebuilt it");
}
