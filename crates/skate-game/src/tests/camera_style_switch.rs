//! Camera Angle style switches mid-run (Skate 3 -> Skate 2 -> Skate 3) against never switching.
use super::*;

/// Camera offset from the board (x, y, z) and shot for each tick while rolling
/// forward, switching Camera Angle style at the given ticks.
fn roll_with_switches(switches: &[(usize, usize)], ticks: usize) -> Vec<([f32; 3], String)> {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
    let assets = skate_data::GameAssets::load(&root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
    let mut physics =
        GamePhysics::load_with_difficulty(&root, None, crate::difficulty::Difficulty::Normal).unwrap();
    let mut skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
    crate::camera::runtime_style_for_test(0);
    let mut camera = crate::camera::CameraRuntime::load(&root).unwrap();
    let mut input = crate::input::ControllerInput::default();
    let mut controls = PlayerControls::load(&root).unwrap();
    let mut out = Vec::new();
    for tick in 0..ticks {
        if let Some(&(_, style)) = switches.iter().find(|(at, _)| *at == tick) {
            camera.switch_style(style, &root);
        }
        input.sample_raw_for_test(skate_core::input::xbox::XboxState {
            buttons: 0, triggers: [0; 2],
            left: if tick > 20 { [0, 20000] } else { [0; 2] }, right: [0; 2],
        });
        let mut actions = input.player_actions();
        controls.update(&mut actions, physics.settings.step.simulation.time_step,
            physics.settings.input_magnitude_threshold, skater.player_input.physical.scoring.capabilities_204);
        controls.publish_gestures(physics.animation_profile.physics_mode, skater.player_input.physical.state.state_16);
        frame::advance(&mut physics, &mut skater, &mut controls, &graphs, &mut actions, true, &mut camera)
            .unwrap();
        let board = physics.board.bodies()[6].rates.position; // the deck
        let at = camera.frame.unwrap().position;
        out.push(([at[0] - board.x, at[1] - board.y, at[2] - board.z], camera.selected_shot().to_string()));
    }
    crate::camera::runtime_style_for_test(0);
    out
}

#[test]
#[ignore = "requires private stock graphs, animation and camera-styles assets"]
fn switching_camera_style_mid_run_matches_starting_in_that_style() {
    let report = |name: &str, run: &[([f32; 3], String)]| {
        for (tick, (offset, shot)) in run.iter().enumerate().filter(|(t, _)| t % 25 == 0 || *t > 395) {
            eprintln!("{name} tick {tick:3} offset {:6.2} {:6.2} {:6.2} dist {:5.2} shot {shot}",
                offset[0], offset[1], offset[2], (offset[0].powi(2) + offset[1].powi(2) + offset[2].powi(2)).sqrt());
        }
    };
    let stock = roll_with_switches(&[], 400);
    let switched = roll_with_switches(&[(150, 1), (275, 0)], 400);
    report("stock   ", &stock);
    report("switched", &switched);
    // The skater faces +z here: behind and above it from the first frames after a switch.
    for (tick, (offset, _)) in switched.iter().enumerate().filter(|(t, _)| (153..160).contains(t) || (278..285).contains(t)) {
        assert!(offset[2] < -1.5 && offset[1] > 1.0, "tick {tick}: camera not behind the skater after switching: {offset:?}");
    }
    // Skate 1's camera is low, but it too starts behind the skater.
    let skate1 = roll_with_switches(&[(150, 2)], 200);
    report("skate1  ", &skate1[140..].to_vec());
    for (tick, (offset, _)) in skate1.iter().enumerate().skip(153) {
        assert!(offset[2] < -1.0, "tick {tick}: Skate 1 camera not behind the skater after switching: {offset:?}");
    }
    // Back on Skate 3 it is where it would be had it never switched.
    for tick in 280..400 {
        let (a, b) = (stock[tick].0, switched[tick].0);
        let gap = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        assert!(gap < 0.25, "tick {tick}: camera after switching back is {gap} m from never switching");
    }
}


/// Skate 1 has no off-board skating, so its camera hands over to Skate 3's off-board shots
/// (skate2port/old_cameras.py grafts them in) once the skater steps off, and back after.
#[test]
#[ignore = "requires private stock graphs, animation and camera-styles assets"]
fn skate1_camera_uses_skate3_offboard_shots() {
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
    let assets = skate_data::GameAssets::load(&root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
    let mut physics = GamePhysics::load(&root).unwrap();
    let mut skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
    crate::camera::runtime_style_for_test(2);
    let mut camera = crate::camera::CameraRuntime::load(&root).unwrap();
    crate::camera::runtime_style_for_test(0);
    assert_eq!(camera.style, 2);
    let mut controls = PlayerControls::default();
    let mut input = crate::input::ControllerInput::default();
    let mut shots = Vec::new();
    for tick in 0..600 {
        input.sample_raw_for_test(skate_core::input::xbox::XboxState {
            // Y steps off at 20 and back on at 400; walk forward in between.
            buttons: if tick == 20 || tick == 400 { 0x8000 } else { 0 },
            triggers: [0; 2],
            left: if (120..300).contains(&tick) { [0, 24000] } else { [0; 2] },
            right: [0; 2],
        });
        let mut actions = input.player_actions();
        controls.update_for_physics(&mut actions, &physics, &skater, &camera).unwrap();
        frame::advance(&mut physics, &mut skater, &mut controls, &graphs, &mut actions, true, &mut camera)
            .unwrap();
        shots.push(camera.selected_shot().to_string());
    }
    for tick in (0..600).step_by(50) {
        eprintln!("tick {tick:3} shot {}", shots[tick]);
    }
    const OFFBOARD: [&str; 8] = ["drop_in", "hippyhurdle", "hippyjump", "hippyjump_high", "moving_object",
        "offboard_air", "offboard_ground", "offboard_low_air"];
    assert!(!OFFBOARD.contains(&shots[10].as_str()), "riding picked {}", shots[10]);
    assert!(shots[150..300].iter().all(|s| OFFBOARD.contains(&s.as_str())),
        "walking picked {:?}", shots[150..300].iter().find(|s| !OFFBOARD.contains(&s.as_str())));
    assert!(!OFFBOARD.contains(&shots[599].as_str()), "back on the board picked {}", shots[599]);
}
