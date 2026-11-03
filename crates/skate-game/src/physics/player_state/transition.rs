//! Native SetPhysicsState82DB8540 publication, followed by owned Enter/Exit.
use super::*;
use bevy::log::{info, warn};
use skate_core::{physics::board_toolkit::BoardToolkit, player::lifecycle::*};

struct Calls;
impl PhysicalStateCalls for Calls {
    fn get_type(&mut self, state: StateBinding) -> PhysicalStateId {
        state.state
    }
    //Host lifecycle publication is completed below before applying physical
    //effects. These supported Exit methods do not inspect the active binding.
    fn exit(&mut self, call: StateCall) {
        assert!(
            call.state.state.is_grind()
                || call.state.state == PhysicalStateId::Nonspecific
                || matches!(
                    call.state.state,
                    PhysicalStateId::Sleeping
                        | PhysicalStateId::PhysicsGround
                        | PhysicalStateId::PhysicsAir
                        | PhysicalStateId::PhysicsAirSecondary
                        | PhysicalStateId::FootPlant
                        | PhysicalStateId::Boneless
                        | PhysicalStateId::HandPlant
                        | PhysicalStateId::RevertGround
                        | PhysicalStateId::KnownAir
                        | PhysicalStateId::BipedAir
                        | PhysicalStateId::BipedGround
                        | PhysicalStateId::OffBoardPushing
                        | PhysicalStateId::GroundAnimation
                        | PhysicalStateId::SlideGround
                        | PhysicalStateId::WipeoutGround
                        | PhysicalStateId::Teleporting
                        | PhysicalStateId::LandingOnDeck
                        | PhysicalStateId::Skitching
                )
        );
    }
    fn enter(&mut self, call: StateCall) {
        assert!(
            call.state.state.is_grind()
                || call.state.state == PhysicalStateId::Nonspecific
                || matches!(
                    call.state.state,
                    PhysicalStateId::PhysicsGround
                        | PhysicalStateId::PhysicsAir
                        | PhysicalStateId::PhysicsAirSecondary
                        | PhysicalStateId::FootPlant
                        | PhysicalStateId::Boneless
                        | PhysicalStateId::HandPlant
                        | PhysicalStateId::RevertGround
                        | PhysicalStateId::KnownAir
                        | PhysicalStateId::BipedAir
                        | PhysicalStateId::BipedGround
                        | PhysicalStateId::OffBoardPushing
                        | PhysicalStateId::GroundAnimation
                        | PhysicalStateId::SlideGround
                        | PhysicalStateId::WipeoutGround
                        | PhysicalStateId::Teleporting
                        | PhysicalStateId::LandingOnDeck
                        | PhysicalStateId::Skitching
                )
        );
    }
}
/// Retail 502 has its own owner (1776 vs Ground 1764) whose Enter binds the
/// grab Ground already qualified. The port runs 502 on the Ground owner, so
/// BipedGround -> 502 skips Ground Exit/Enter: their reset would drop the
/// latched descriptor (flags144/152/156) and the grab pipeline (G7 review C1).
/// 502 -> BipedGround still re-enters Ground, as retail Ground Enter does.
fn keeps_ground_owner(current: PhysicalStateId, requested: PhysicalStateId) -> bool {
    current == PhysicalStateId::BipedGround && requested == PhysicalStateId::OffBoardPushing
}

pub(super) fn set(
    physics: &mut GamePhysics,
    skater: &mut SkaterRuntime,
    requested: PhysicalStateId,
) -> Result<(), String> {
    // RevertGround owns a real native Enter/Update/Fill lifecycle. Preserve
    // the selector request so its turning controller can run.
    skater.player_state.requested_state = requested;
    let current = skater.player_state.current();
    if current == requested {
        return Ok(());
    }
    crate::crash_context::requested(physics.ticks, current, requested);
    let board_observation =
        super::super::offboard::board_manager::runtime::observe(physics, skater);
    let p = &mut skater.player_input.processed;
    info!(current = ?current, requested = ?requested,
        board_controller_state = skater.skateboard_controller.fields.state_448,
        board_controller_on = skater.skateboard_controller.fields.system_on_452,
        processed_state = p.state_2508, processed_category = p.category_2512,
        "physical state transition");
    // Keep the actual selector request for the coordinator; do not publish a
    // state whose physical Enter/Exit owner has not been connected.
    if !skater
        .player_state
        .registry
        .can_transition(current, requested)
    {
        // Release review 10-18 (no crashes): a state the port has no Enter/Exit for is refused and the current one
        // kept; before, the error stopped the game (physics.rs AppExit). Logged once per pair, as it is asked each tick.
        static REFUSED: std::sync::Mutex<Vec<(PhysicalStateId, PhysicalStateId)>> = std::sync::Mutex::new(Vec::new());
        let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
        if !refused.contains(&(current, requested)) {
            refused.push((current, requested));
            warn!("Physical state transition {current:?} -> {requested:?} has no native Enter/Exit adapter; refused, staying in {current:?}");
        }
        return Ok(());
    }
    let player = &mut skater.player_input.player;
    let mut data = StateChangeData {
        player: PlayerStateChangeFields {
            word_1312: player.state_count_1312,
            previous_category_latch_1336: player.state_value_1336,
            scalar_1344: player.state_timer_1344,
        },
        processed: ProcessedStateChangeFields {
            flags_2480: p.flags_2480,
            requested_state_2500: requested as u32,
            previous_state_2504: p.state_2504,
            current_state_2508: p.state_2508,
            current_category_2512: p.category_2512,
            previous_category_2516: p.category_2516,
            previous_category_latch_2520: p.player_state_value_2520,
            word_2564: p.state_count_2564,
            scalar_2664: p.state_timer_2664,
        },
        skateboard_controller: skater.skateboard_controller.fields,
    };
    let mut effects = skater.board_possession_live.effects(
        physics,
        &mut skater.ground_lifecycle.board_animated_290,
        p.timestep_2604,
    );
    let mut controller_actions = super::super::offboard::board_manager::Transition::new(
        &mut skater.board_possession,
        &board_observation,
        &mut effects,
        data.skateboard_controller,
    );
    skater
        .player_state
        .lifecycle
        .set_physics_state(
            requested as u32,
            &mut data,
            &mut Calls,
            &mut controller_actions,
        )
        .map_err(|e| format!("Unknown physical state {}", e.0))?;
    controller_actions.finish(&mut data.skateboard_controller);
    skater.board_possession_live.publish_volumes(physics);
    player.state_count_1312 = data.player.word_1312;
    player.state_value_1336 = data.player.previous_category_latch_1336;
    player.state_timer_1344 = data.player.scalar_1344;
    p.flags_2480 = data.processed.flags_2480;
    p.state_2504 = data.processed.previous_state_2504;
    p.state_2508 = data.processed.current_state_2508;
    p.category_2512 = data.processed.current_category_2512;
    p.category_2516 = data.processed.previous_category_2516;
    p.player_state_value_2520 = data.processed.previous_category_latch_2520;
    p.state_count_2564 = data.processed.word_2564;
    p.state_timer_2664 = data.processed.scalar_2664;
    skater.skateboard_controller.fields = data.skateboard_controller;
    if skater.player_input.toolkit.is_none() {
        skater.player_input.toolkit = Some(BoardToolkit::from_board(
            &physics.board,
            p.flags_2468,
            p.scalar_2612,
            p.vectors_464_480_496_512_528[0].map(f32::from_bits),
            [0.0, 1.0, 0.0, 0.0],
        ));
    }
    //Native82DB8540 publishes Processed state/history BEFORE old Exit. The
    //same retained objects then receive Exit followed by the new Enter.
    match current {
        PhysicalStateId::RevertGround => {
            // Native Exit is empty; this is diagnostic-only host reporting.
            info!(tick = physics.ticks, requested = ?requested, "REVERT_EXIT");
        }
        PhysicalStateId::HandPlant => skater.handplant.reset(),
        PhysicalStateId::FootPlant => skater.footplant.reset(), //Exit82D4C5A8
        PhysicalStateId::Boneless => {}                         //empty82D4C9B4
        PhysicalStateId::PhysicsGround => super::super::ground_exit::exit(physics, skater),
        PhysicalStateId::PhysicsAir | PhysicalStateId::PhysicsAirSecondary => {
            super::super::air_phase::exit(skater)
        }
        PhysicalStateId::KnownAir => {
            super::super::known_air::exit(physics, skater, requested as u32)?
        }
        PhysicalStateId::BipedAir => super::super::biped_air::exit(skater),
        PhysicalStateId::BipedGround if keeps_ground_owner(current, requested) => {}
        PhysicalStateId::BipedGround => super::super::biped_ground::exit(skater),
        PhysicalStateId::OffBoardPushing => super::super::biped_ground::exit(skater),
        PhysicalStateId::GroundAnimation => super::super::ground_animation::exit(physics, skater)?,
        PhysicalStateId::LandingOnDeck => super::super::landing_on_deck::exit(physics, skater)?,
        PhysicalStateId::SlideGround => super::super::slide_state::exit(physics, skater)?,
        PhysicalStateId::WipeoutGround => super::super::wipeout_states::exit(physics, skater),
        PhysicalStateId::Sleeping | PhysicalStateId::Teleporting | PhysicalStateId::Skitching => {} //82B61BB8.
        state if state.is_grind() || state == PhysicalStateId::Nonspecific => {
            super::super::grind::exit(physics, skater)?
        }
        _ => unreachable!("state support checked before publication"),
    }
    match requested {
        PhysicalStateId::RevertGround => super::super::revert_state::enter(physics, skater),
        PhysicalStateId::HandPlant => super::super::handplant::enter(physics, skater),
        PhysicalStateId::FootPlant => super::super::footplant::ground::enter(physics, skater),
        PhysicalStateId::Boneless => super::super::boneless::enter(physics, skater),
        PhysicalStateId::PhysicsGround => super::super::ground_phase::enter(physics, skater),
        // PhysicsAirSecondary 202 runs the recovered PhysicsAir200 lifecycle.
        //
        // INFERRED, not recovered: no vtable or method address for the object at
        // PhysicalPlayer offset 1716 appears anywhere in this port, because the S2 debug
        // build that named the rest has no counterpart for it -- 202 is an S3 addition, like
        // the sixth (darkslide) grind family grind_chromosome.rs records. Five checks support
        // sharing 200's lifecycle rather than inventing one:
        //
        // 1. Owner offsets are adjacent, 1712 and 1716 (state.rs native_owner_offset), which
        //    is how a second instance of one class appears, not a different class.
        // 2. air_phase::enter already branches on previous_physics_category_2516 == 400, the
        //    grind family, so entry to air out of a grind is an already-recovered path.
        // 3. Both 200 and GroundAnimation103 enter through enable_angular_only, so this
        //    lifecycle does not fight the FORCE_ANIM_SKATEBOARD the authored DarkSlideTrick
        //    state sets -- the crash that motivated this recorded force_mode=2, that mode.
        // 4. selector/mod.rs treats 202 as transient: it leaves for 200 the moment
        //    flags_2484 bit 20 clears, and for WipeoutGround on a wipeout. Same family.
        // 5. 202 is only ever entered while the board is animation-driven, so the solver
        //    difference between the two objects, if any, has nothing to act on.
        //
        // What would disprove it: 202's native Update or Fill writing different PhysOut
        // fields than 200's. That shows up as wrong air metrics or scoring on darkslide
        // exits, never as a crash. See docs/engine-defects.md #10.
        PhysicalStateId::PhysicsAir | PhysicalStateId::PhysicsAirSecondary => {
            super::super::air_phase::enter(physics, skater)
        }
        PhysicalStateId::KnownAir => super::super::known_air::enter(physics, skater),
        PhysicalStateId::BipedAir => super::super::biped_air::enter(physics, skater),
        PhysicalStateId::BipedGround => super::super::biped_ground::enter(physics, skater),
        PhysicalStateId::OffBoardPushing if keeps_ground_owner(current, requested) => Ok(()),
        PhysicalStateId::OffBoardPushing => super::super::biped_ground::enter(physics, skater),
        PhysicalStateId::GroundAnimation => super::super::ground_animation::enter(physics, skater),
        PhysicalStateId::LandingOnDeck => super::super::landing_on_deck::enter(physics, skater),
        PhysicalStateId::SlideGround => super::super::slide_state::enter(physics, skater),
        PhysicalStateId::Skitching => super::super::skitch::enter(physics, skater),
        PhysicalStateId::WipeoutGround => super::super::wipeout_states::enter(physics, skater),
        PhysicalStateId::Teleporting => {
            skater.teleport_state.enter();
            Ok(())
        }
        state if state.is_grind() || state == PhysicalStateId::Nonspecific => {
            super::super::grind::enter(physics, skater)
        }
        _ => unreachable!("state support checked before publication"),
    }?;
    // 502 Enter 82D442D0 lets go of the board in hand before the hands take the object (Skate 2's 501 Enter
    // 82B69600 too); Exit 82D449B8 leaves it where it fell.
    if requested == PhysicalStateId::OffBoardPushing {
        super::super::offboard::board_manager::runtime::request_drop(physics, skater);
    }
    // Keep the command stream alongside the native lifecycle publication.
    // The direct calls above remain the owner of TU3 Enter/Exit ordering;
    // this record is the coordinator-facing audit/consumer boundary.
    physics.exchange.request_state(requested)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ground_to_offboard_pushing_keeps_the_ground_owner() {
        use PhysicalStateId::*;
        assert!(keeps_ground_owner(BipedGround, OffBoardPushing));
        assert!(!keeps_ground_owner(OffBoardPushing, BipedGround));
        assert!(!keeps_ground_owner(BipedAir, OffBoardPushing));
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (private stock graphs and animation assets)"]
    fn grab_latch_survives_entering_offboard_pushing() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        let mut physics =
            GamePhysics::load_with_difficulty(&root, None, crate::difficulty::Difficulty::Normal)
                .unwrap();
        let mut skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
        super::super::initialize(&mut physics, &mut skater).unwrap();
        set(&mut physics, &mut skater, PhysicalStateId::BipedGround).unwrap();
        // What grab.rs leaves after qualifying face 3 of prop 1 with RB held.
        let ground = &mut skater.biped_ground.ground;
        ground.flags_144_to_150[0] = true;
        ground.counter_152 = 2;
        ground.counter_156 = 7;
        skater.offboard_grab.flags_12836 = 0x40;
        set(&mut physics, &mut skater, PhysicalStateId::OffBoardPushing).unwrap();
        assert_eq!(skater.player_state.current(), PhysicalStateId::OffBoardPushing);
        let ground = &skater.biped_ground.ground;
        assert!(ground.flags_144_to_150[0]);
        assert_eq!((ground.counter_152, ground.counter_156), (2, 7));
        assert_eq!(skater.offboard_grab.flags_12836 & 0x40, 0x40);
        // Leaving 502 re-enters Ground, whose reset drops the latch.
        set(&mut physics, &mut skater, PhysicalStateId::BipedGround).unwrap();
        assert_eq!(skater.biped_ground.ground.counter_152, 0);
        assert_eq!(skater.offboard_grab.flags_12836 & 0xc0, 0);
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (private stock graphs and animation assets)"]
    fn entering_offboard_pushing_drops_the_board_in_hand() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        let mut physics =
            GamePhysics::load_with_difficulty(&root, None, crate::difficulty::Difficulty::Normal)
                .unwrap();
        let mut skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
        super::super::initialize(&mut physics, &mut skater).unwrap();
        set(&mut physics, &mut skater, PhysicalStateId::BipedGround).unwrap();
        assert_eq!(skater.skateboard_controller.fields.state_448, 1, "board in hand");
        // 82D442D0: LetGo, then FREE; the controller stays on and neither hand drives the board.
        set(&mut physics, &mut skater, PhysicalStateId::OffBoardPushing).unwrap();
        let fields = skater.skateboard_controller.fields;
        assert_eq!((fields.state_448, fields.system_on_452), (2, true));
        let possession = &skater.board_possession.state;
        assert_eq!(possession.selected_hand_424, 2);
        assert!(possession.hands.iter().all(|hand| hand.dynamics == [[0, 0, 0, 2]; 2]));
        // Letting go of the object leaves the board where it fell (82D449B8, 82DB8540).
        set(&mut physics, &mut skater, PhysicalStateId::BipedGround).unwrap();
        assert_eq!(skater.skateboard_controller.fields.state_448, 2);
    }

    /// A forward stick held through the real controller chain, in 502.
    fn forward_stick() -> skate_core::input::controller::DerivedControllerInput {
        use skate_core::input::{controller, gameplay_map::GameplayActions, pad::Pad, xbox};
        let mut pad = Pad::new();
        pad.update(&xbox::convert(
            &xbox::XboxState {
                buttons: 0,
                triggers: [0; 2],
                left: [0, 32767],
                right: [0; 2],
            },
            0,
        ));
        let mut controller = controller::DerivedControllerInput::from_words([0; 26]);
        controller.initialize();
        controller.update(
            &mut GameplayActions::from_pad(&pad),
            1. / 60.,
            true,
            false,
            &controller::MagnitudeHeldSettings {
                attribute: Some(0.),
                missing_attribute_value: 0.,
            },
        );
        controller
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT and SKATE_PROPS_MAP (an installed Skate 3 district .skate)"]
    fn held_stick_moves_the_grabbed_prop_in_502() {
        use crate::physics::{offboard::grab_scene::Registry, props};
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
        let map = skate_data::skate_map::SkateMap::load(std::path::Path::new(
            &std::env::var_os("SKATE_PROPS_MAP").unwrap(),
        ))
        .unwrap();
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        let mut physics = GamePhysics::load_with_difficulty(
            &root,
            Some(&map),
            crate::difficulty::Difficulty::Normal,
        )
        .unwrap();
        // The append physics.rs makes under SKATE_PROPS=1.
        let empty = skate_core::physics::board_world::BoardWorld::new(Vec::new());
        let world = std::mem::replace(&mut physics.world, empty);
        let (world, loaded, objects, bindings) =
            props::Props::load(&root, &map, world, physics.settings.floor_material).unwrap();
        physics.offboard_grab_scene = Registry::new(&world, objects, bindings).unwrap();
        physics.world = world;
        physics.props = loaded;
        let mut skater = SkaterRuntime::load(&root, &graphs, &physics, "normal").unwrap();
        super::super::initialize(&mut physics, &mut skater).unwrap();
        set(&mut physics, &mut skater, PhysicalStateId::BipedGround).unwrap();
        // Ground qualified face 0 (+X) of prop 0 with RB held.
        let ground = &mut skater.biped_ground.ground;
        ground.flags_144_to_150[0] = true;
        ground.counter_152 = 2;
        ground.counter_156 = 1;
        set(&mut physics, &mut skater, PhysicalStateId::OffBoardPushing).unwrap();
        skater.player_input.processed.timestep_2604 = 1. / 60.;
        let stick = forward_stick();
        props::advance(&mut physics, &mut skater, &stick);
        let start = physics.props.held_centre().expect("502 binds the latched prop");
        for _ in 0..20 {
            props::advance(&mut physics, &mut skater, &stick);
        }
        let moved = physics.props.held_centre().unwrap() - start;
        println!("prop moved {moved:?}");
        // 20 ticks of push (3 m/s, 20 m/s^2 ramp) cover ~0.75 m, flat.
        assert!(moved.length() > 0.3 && moved.y == 0., "{moved:?}");
    }
}
