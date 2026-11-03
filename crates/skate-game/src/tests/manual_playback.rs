//! Raw controller -> authored manual states -> real shared solve and release.
use super::*;
use skate_core::player::state::PhysicalStateId;

#[test]
#[ignore = "requires private stock graph/animation/physics assets"]
fn raw_controller_manuals_enter_balance_and_release_in_gameplay() {
    for sign in [-1_i16, 1] {
        run_manual(sign);
    }
}

fn run_manual(sign: i16) {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    let assets = skate_data::GameAssets::load(root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
    let mut physics = GamePhysics::load(root).unwrap();
    let mut skater = SkaterRuntime::load(root, &graphs, &physics, "normal").unwrap();
    let mut controls = PlayerControls::load(root).unwrap();
    let mut input = crate::input::ControllerInput::default();
    let mut camera = crate::camera::CameraRuntime::load(root).unwrap();
    let mut manual_seen = false;
    let mut balance_seen = false;
    let mut controller_seen = false;
    let mut timer_seen = false;
    let mut release_seen = false;
    for tick in 0..330 {
        // A sustained partial stick is not a Flickit flick. Use the actual
        // converter and gesture pipeline, including its authored exclusions.
        let holding = (45..195).contains(&tick);
        input.sample_raw_for_test(skate_core::input::xbox::XboxState {
            buttons: if (12..40).contains(&tick) { 0x1000 } else { 0 },
            triggers: [0; 2],
            left: [0; 2],
            right: if holding { [0, sign * 22000] } else { [0; 2] },
        });
        let mut actions = input.player_actions();
        controls.update(
            &mut actions,
            physics.settings.step.simulation.time_step,
            physics.settings.input_magnitude_threshold,
            skater.player_input.physical.scoring.capabilities_204,
        );
        controls.publish_gestures(
            physics.animation_profile.physics_mode,
            skater.player_input.physical.state.state_16,
        );
        frame::advance(
            &mut physics,
            &mut skater,
            &mut controls,
            &graphs,
            &mut actions,
            true,
            &mut camera,
        )
        .unwrap_or_else(|error| {
            panic!(
                "Manual sign={sign} tick={tick}: {error}; state={:?}; balance={}; deck={:?}",
                skater.player_state.current(),
                skater.animation_input.fields.balance,
                physics.board.bodies()[6].rates
            )
        });
        manual_seen |= skater.animation.motion.flags.manualing;
        balance_seen |=
            skater.animation.motion.flags.manualing && skater.animation_input.fields.balance != 0.0;
        controller_seen |= skater.ground.manual.elapsed > 0.0
            && skater.ground.manual.angular_correction != 0.0
            && skater.player_state.current() == PhysicalStateId::PhysicsGround;
        if tick >= 195 {
            assert!(controls.action_intents.get("Manual").is_none());
            assert!(
                skater
                    .animation
                    .motion
                    .animation
                    .motion_intents
                    .get("Manual")
                    .is_none()
            );
            timer_seen |= skater.animation.motion.riding.manual_out_timer > 0.0;
            release_seen |= !skater.animation.motion.flags.manualing
                && skater.animation_input.fields.balance == 0.0
                && skater.ground.manual.elapsed == 0.0;
        }
        assert_eq!(skater.pose_generation, physics.ticks);
        assert!(camera.frame.is_some());
        assert_eq!(physics.exchange.output().unwrap().tick + 1, physics.ticks);
    }
    assert!(manual_seen, "Authored manual never entered: sign={sign}");
    assert!(
        balance_seen,
        "Manual never published physical balance: sign={sign}"
    );
    assert!(
        controller_seen,
        "Manual never reached Ground controller: sign={sign}"
    );
    assert!(
        timer_seen,
        "Manual exit never armed authored timer: sign={sign}"
    );
    assert!(
        release_seen,
        "Manual controller did not clear after release: sign={sign}"
    );
    assert_eq!(skater.animation.motion.riding.manual_out_timer, 0.0);
    eprintln!(
        "Manual sign={sign}: authored entry, physical balance/controller, release, pose and camera passed {} ticks",
        physics.ticks
    );
}

/// Bug 10-25 #30: a kickflip landed with the right stick held part way rides out in a manual until the stick is let
/// go, in every difficulty. Retail's data: the air states land into Manual.TailManual / NoseManual.Holding while the
/// Manual intent is held (ManualEngageTime > 0.2), and every manual state needs `not HasIntent ManualBrake`.
/// Fill82599BC8 publishes ManualBrake once the stick length passes 820997B8 (0x3F666666, 0.9), so a full tilt
/// (30000 -> 0.95) gives the brake / nollie wind-up instead of a manual, the same as retail.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn a_kickflip_lands_into_a_manual_that_holds_until_the_stick_is_let_go() {
    use super::trick_completeness::{ground_flick, pad};
    use crate::difficulty::Difficulty;
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
    let assets = skate_data::GameAssets::load(&root).unwrap();
    let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
    let kickflip = skate_data::gesture_patterns::load(&root.join("private/stock/data/joystick/skater.pat"))
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Kickflip")
        .unwrap()
        .points;
    for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hardcore] {
        for y in [-22000_i16, 22000, 30000] {
            let mut physics = GamePhysics::load_with_difficulty(&root, None, difficulty).unwrap();
            let mut skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
            let mut camera = crate::camera::CameraRuntime::load(&root).unwrap();
            let mut controller = crate::input::ControllerInput::default();
            let mut controls = PlayerControls::load(&root).unwrap();
            let (mut airborne, mut landed, mut first, mut dropped) = (false, None, None, None);
            for step in 0..SPAWN_PIN_DELAY + 520 {
                let tick = step.saturating_sub(SPAWN_PIN_DELAY);
                // Roll at 8 m/s, flick at 100.., lift 4 m/s at pop so the flip lands, then hold the stick from 126:
                // ManualEngageTime must pass 0.2 s before touchdown (Hardcore lands at 146) for the air to land in it.
                if tick == 60 {
                    for body in physics.board.bodies_mut() {
                        body.rates.linear_velocity = Vector3::new(0., 0., 8.);
                    }
                    for body in skater.skeleton.bodies_mut() {
                        body.rates.linear_velocity = Vector3::new(0., 0., 8.);
                    }
                }
                if tick == 123 {
                    for body in physics.board.bodies_mut() {
                        body.rates.linear_velocity.y += 4.;
                    }
                    for body in skater.skeleton.bodies_mut() {
                        body.rates.linear_velocity.y += 4.;
                    }
                }
                let right = match tick {
                    _ if step < SPAWN_PIN_DELAY => [0; 2],
                    0..126 => ground_flick(&kickflip, tick, false),
                    126..460 => [0, y],
                    _ => [0; 2],
                };
                controller.sample_raw_for_test(pad(right));
                let mut actions = controller.player_actions();
                controls.update(
                    &mut actions,
                    physics.settings.step.simulation.time_step,
                    physics.settings.input_magnitude_threshold,
                    skater.player_input.physical.scoring.capabilities_204,
                );
                controls.publish_gestures(physics.animation_profile.physics_mode, skater.player_input.physical.state.state_16);
                frame::advance(&mut physics, &mut skater, &mut controls, &graphs, &mut actions, true, &mut camera).unwrap();
                if step < SPAWN_PIN_DELAY || tick < 100 {
                    continue;
                }
                let category = skater.player_state.current().category();
                airborne |= category == 200;
                if airborne && category != 200 && landed.is_none() {
                    landed = Some(tick);
                }
                let manualing = skater.animation.motion.flags.manualing;
                if manualing && first.is_none() {
                    first = Some(tick);
                }
                if first.is_some() && !manualing && dropped.is_none() && tick < 460 {
                    dropped = Some(tick);
                }
            }
            let case = format!("{difficulty:?} y={y}: landed {landed:?}, manual from {first:?}, dropped {dropped:?}");
            eprintln!("{case}");
            let landed = landed.unwrap_or_else(|| panic!("{case}: the flip never landed"));
            if y.abs() == 22000 {
                if difficulty == Difficulty::Hardcore {
                    // Hardcore's shorter air touches down inside B_KICKFLIP_OUT1, and T_Kickflip's End goes to Land
                    // (onboard.xml), which has no Manual transition (Landing.xml): the manual follows the land.
                    assert!(first.is_some_and(|t| t > landed), "{case}: the manual never followed the land");
                } else {
                    assert_eq!(first, Some(landed), "{case}: the landing did not go into the manual");
                }
                assert_eq!(dropped, None, "{case}: the manual dropped while the stick was held");
            } else {
                assert_eq!(first, None, "{case}: a full tilt is ManualBrake, not a manual");
            }
        }
    }
}
