use super::*;

fn tick(
    (physics, skater, controls, camera, graphs): (&mut GamePhysics, &mut SkaterRuntime, &mut PlayerControls, &mut crate::camera::CameraRuntime, &crate::graph_runtime::StockGraphs),
    input: &mut crate::input::ControllerInput,
    state: skate_core::input::xbox::XboxState,
) {
    input.sample_raw_for_test(state);
    let mut actions = input.player_actions();
    controls.update(
        &mut actions,
        physics.settings.step.simulation.time_step,
        physics.settings.input_magnitude_threshold,
        skater.player_input.physical.scoring.capabilities_204,
    );
    frame::advance(physics, skater, controls, graphs, &mut actions, true, camera).unwrap();
}

/// The lead's condition 3: Player 2's turn starts with nothing of Player 1's (no score, no trick or bail flags, no
/// momentum). Player 1 pushes and ollies; the handover (the turn's teleport onto the slot, then
/// party_play::reset_turn) leaves Player 2 a skater at rest with a fresh scorer.
#[test]
#[ignore = "requires private stock skater, animation banks and collections"]
fn player_2_starts_with_nothing_left_from_player_1() {
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
    let fresh = format!("{:?}", skater.scoring.session);
    let slot = skater.animated_skeleton.roots.animation_to_world;
    let pad = |buttons: u16, right: [i16; 2]| skate_core::input::xbox::XboxState { buttons, triggers: [0; 2], left: [0; 2], right };

    // Player 1: pushes (A), then pops an ollie and rolls on.
    for t in 0..330 {
        let state = match t {
            0..240 => pad(if t % 40 < 30 { 0x1000 } else { 0 }, [0; 2]),
            240..270 => pad(0, [0, -32767]),
            270..273 => pad(0, [0, 32767]),
            _ => pad(0, [0; 2]),
        };
        tick((&mut physics, &mut skater, &mut controls, &mut camera, &graphs), &mut input, state);
    }
    let speed = physics.riding.motion.ground_speed;
    assert!(speed > 1., "player 1 is rolling ({speed})");
    eprintln!("player 1: speed {speed}, scorer {}", if format!("{:?}", skater.scoring.session) == fresh { "untouched" } else { "used" });
    // A flat-ground ollie may score nothing: player 1 leaves a line and a combo behind either way.
    skater.scoring.session.line.points += 1234.;
    skater.scoring.session.combo.multiplier += 1.;
    assert_ne!(format!("{:?}", skater.scoring.session), fresh);

    // The handover, in the game's order: SetPlayer (reset_turn) at the countdown, the skater rolling on behind the
    // loading screen, then the turn's teleport about TELEPORT_DELAY later.
    crate::party_play::reset_turn(&mut skater, Some(&mut camera));
    assert_eq!(format!("{:?}", skater.scoring.session), fresh, "no score or line left over");
    for _ in 0..30 {
        tick((&mut physics, &mut skater, &mut controls, &mut camera, &graphs), &mut input, pad(0, [0; 2]));
    }
    skater.travel_to(slot).unwrap();
    for _ in 0..60 {
        tick((&mut physics, &mut skater, &mut controls, &mut camera, &graphs), &mut input, pad(0, [0; 2]));
    }
    let v = physics.board.bodies()[6].rates.linear_velocity;
    let speed = v.x.hypot(v.z);
    assert!(speed < 0.05, "player 2 starts at rest, not with player 1's momentum ({speed})");
    assert_eq!(skater.player_input.physical.air.known_air_valid_437, 0, "not in the air");
    assert!(!physics.board_wiping_out, "no bail");
    assert_eq!(format!("{:?}", skater.scoring.session), fresh, "nothing scored while standing");
    eprintln!("player 2: speed {speed}, state {:?}", skater.player_state.current());
}
