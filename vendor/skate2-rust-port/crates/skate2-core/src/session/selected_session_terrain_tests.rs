use super::{SelectedSessionTerrain, SelectedSessionTerrainError, SelectedTerrainSource};
use crate::{
    collection_resident_streamer::CollectionId,
    game::{WorldStreamPoll, WorldStreamSpec},
    selected_original_assets_bootstrap::SelectedOriginalAssets,
    selected_session_world_startup::SelectedSessionWorldStartup,
    simulation_environment_unlock_router::EnvironmentUnlockSeed,
    world_stream_catalog::WorldStreamCatalog,
    world_stream_runtime::WorldStreamEffects,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
struct StreamCount {
    high_collections: usize,
    finalized: usize,
}
impl WorldStreamEffects for StreamCount {
    fn add_high_collection(&mut self, _id: u64, _x: u32, _z: u32) {
        self.high_collections += 1;
    }

    fn finalize_world_bounds(&mut self) {
        self.finalized += 1;
    }
}

#[test]
#[ignore = "requires original external game data via SKATE2_GAME_ROOT"]
fn selected_terrain_unloads_departed_focus_and_reloads_original_ground() {
    let root =
        std::path::PathBuf::from(std::env::var_os("SKATE2_GAME_ROOT").expect("original game root"));
    let mounts =
        Arc::new(crate::simulation_contact_jacobian::ground_preparation::corpus::mounted_corpus());
    let assets = SelectedOriginalAssets::try_new(Arc::clone(&mounts), &root).unwrap();
    let world = SelectedSessionWorldStartup::try_new(&assets.cvars, &assets.vault, &assets.mounts).unwrap();
    let source = SelectedTerrainSource::try_new(
        Arc::clone(&world.world),
        Arc::clone(&mounts),
        WorldStreamCatalog::new(),
        assets.cvars.clone(),
        WorldStreamSpec::derive("BAM", None).unwrap(),
        StreamCount::default(),
        // Empty contexts and zero lock word are the native constructor state.
        EnvironmentUnlockSeed {
            locations_lock_status_raw: 0,
            contexts_in_native_iteration_order: Vec::new(),
        },
    )
    .unwrap();
    // The raw message userdata and parallel answer are explicit corpus policy.
    let mut terrain =
        SelectedSessionTerrain::new(source, CollectionId(0x823c_4449_fb31_63b4), false, 0);
    assert!(matches!(
        terrain.try_request(),
        Err(SelectedSessionTerrainError::Bounds(
            crate::world_stream_runtime::WorldBoundsError::Pending
        ))
    ));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "selected world stream timed out");
        match terrain.try_poll_stream().unwrap() {
            WorldStreamPoll::Pending => std::thread::yield_now(),
            WorldStreamPoll::CompletedThisPoll => break,
        }
    }
    terrain.try_request().unwrap();
    loop {
        assert!(
            Instant::now() < deadline,
            "selected resident load timed out"
        );
        if terrain.try_poll().unwrap() {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(terrain.terrain_for(&world.world).unwrap().plans.len(), 21);
    let ground = terrain.ground_pool_for(&world.world).unwrap();
    assert_eq!(ground.capacity(), 128);
    assert!(ground.published_volume_count() > 0);
    // Native A_RobsPark focus includes neighboring collision collections.
    // Reuse the same terrain owner, then cross east and revisit the spawn.
    let spawn = [588.8617, -54.231647, 996.5048];
    let mut spawn_counts = None;
    for position in [spawn, [701.0, -54.231647, 996.5048], spawn] {
        let space = terrain.focus_space.as_ref().unwrap();
        let radius = skate2_formats::stream::WORLD_FOCUS_RADIUS;
        let cap = assets
            .cvars
            .try_sim_max_focus_collections()
            .unwrap()
            .min(16);
        let selected = if assets.cvars.try_enable_stream_xml().unwrap() {
            terrain.focus_xml.as_ref().map_or_else(
                || space.focus_collections(position, radius, cap),
                |xml| xml.focus_collections(space, position, radius, cap),
            )
        } else {
            space.focus_collections(position, radius, cap)
        };
        assert!(!selected.is_empty());
        loop {
            assert!(Instant::now() < deadline, "moving terrain load timed out");
            terrain.try_advance_asset_release().unwrap();
            terrain.try_update_focus(position).unwrap();
            if terrain
                .terrain_for(&world.world)
                .unwrap()
                .focus_resident_for_test(&selected)
            {
                break;
            }
            std::thread::yield_now();
        }
        let count = terrain
            .ground_pool_for(&world.world)
            .unwrap()
            .published_volume_count();
        let plans = terrain.terrain_for(&world.world).unwrap().plans.len();
        for _ in 0..20 {
            terrain.try_advance_asset_release().unwrap();
            terrain.try_update_focus(position).unwrap();
        }
        assert_eq!(
            terrain
                .terrain_for(&world.world)
                .unwrap()
                .assets
                .retained_counts()
                .0,
            plans
        );
        assert_eq!(
            terrain
                .ground_pool_for(&world.world)
                .unwrap()
                .published_volume_count(),
            count
        );
        assert_eq!(
            terrain.terrain_for(&world.world).unwrap().plans.len(),
            plans
        );
        if position == spawn {
            if let Some(previous) = spawn_counts {
                assert_eq!(
                    (count, plans),
                    previous,
                    "revisiting spawn must not accumulate registrations"
                );
            } else {
                spawn_counts = Some((count, plans));
            }
        }
    }
}
