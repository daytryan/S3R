use super::*;

/// Push to speed on a large flat floor, then either coast straight or hold a full turn,
/// logging ground speed once a second. User report: retail loses speed while circling,
/// the port does not. Measurement only: prints the curve, asserts nothing about retail.
fn run(turn: bool, push_ticks: u32) -> Vec<f32> {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    let manifest = skate_data::GameAssets::load(root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(root, &manifest).unwrap();
    let mut physics = GamePhysics::load(root).unwrap();
    physics.world = ground::Terrain::Flat.world(physics.settings.floor_material);
    let mut skater = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
    let mut controls = PlayerControls::default();
    let mut camera = crate::camera::CameraRuntime::load(root).unwrap();
    let mut input = crate::input::ControllerInput::default();
    let step = physics.settings.step.simulation.time_step;
    let mut speeds = Vec::new();
    // 0..240: push (A held); 240..840: coast or hold full left.
    for tick in 0..840 {
        let pushing = tick < push_ticks;
        input.sample_raw_for_test(skate_core::input::xbox::XboxState {
            buttons: if pushing && tick % 40 < 30 { 0x1000 } else { 0 },
            triggers: [0; 2],
            left: if !pushing && turn { [-32768, 0] } else { [0; 2] },
            right: [0; 2],
        });
        let mut actions = input.player_actions();
        controls.update(
            &mut actions,
            step,
            physics.settings.input_magnitude_threshold,
            skater.player_input.physical.scoring.capabilities_204,
        );
        frame::advance(&mut physics, &mut skater, &mut controls, &graphs, &mut actions, true, &mut camera)
            .unwrap_or_else(|e| panic!("tick{tick}: {e}"));
        if tick % 30 == 29 {
            let v = physics.board.bodies()[6].rates.linear_velocity;
            let s = v.x.hypot(v.z);
            let side = physics.board.part_transforms()[6].basis.columns[0];
            let slip = v.x * side[0] + v.y * side[1] + v.z * side[2];
            eprintln!(
                "turn={turn} t={:>4.1}s ground_speed={:.4} deck_planar={s:.4} side_slip={slip:.4} idle={:.2} state={:?} contacts={}",
                (tick + 1) as f32 / 60.0,
                physics.riding.motion.ground_speed,
                skater.player_input.processed.time_since_last_input_2748,
                skater.player_state.current(),
                physics.riding.ground.wheel_contact_count
            );
            speeds.push(s);
        }
    }
    speeds
}

#[test]
#[ignore = "requires private stock skater, animation banks and collections"]
fn carve_speed_decay_against_straight_coast() {
    let straight = run(false, 40);
    let turning = run(true, 240);
    eprintln!("straight {straight:?}");
    eprintln!("turning  {turning:?}");
}
