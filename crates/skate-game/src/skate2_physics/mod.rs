//! The Skate 2 physics switch (user 10-20): the player can switch the game to the Skate 2 port's skater physics.
//! Not tested in play.
//!
//! One program (Amendment A): the game links the vendored port's host library (`skate2_physics_host`, in
//! `vendor/skate2-rust-port/host`) and runs the port's session on a thread of its own, swapping one message each way
//! per 60 Hz tick over in-process channels (`skate2-physics-protocol`). There is no helper process.
//!
//! This module holds:
//! - the setting (`settings`) and the sim thread with the `Skate2Physics` resource that follows it (`sim`);
//! - the pad mapping (`input`), and the bypass of the engine's own skater sim with the port's frame on the local
//!   skater (`step`, with `pose` and `camera`);
//! - the switch itself (`switch`): when Skate 2 physics has the skater, the hand-overs and the teleports under it;
//! - the skater's sound from the port's frame (`audio`, Task 18). The tricks go into the engine's scorer from `step`
//!   (`scoring_runtime::Runtime::apply_port_frame`, Task 17), and Skate 2's Hall of Meat panel takes the port's bail
//!   (`skate2_hall_of_meat`, Task 18).
//!
//! With the switch off (the default) none of it runs: `switch::update` only compares the menu's setting with the last
//! one it saw, and `step` (`PhysicsPlugin`) returns at once, so no sim thread is started and the engine's tick is
//! today's. The switch starts the sim the first time Skate 2 physics is wanted.
//!
//! What the rest of the game reads of the skater (`SkaterRuntime`) while Skate 2 physics has it: every field read
//! outside `physics/` (a grep for `skater\.` in `crates/skate-game/src`; readers that name the resource otherwise are
//! not in it). "Filled by `step`" is the port's value each tick; "stale" keeps the engine's last value from before the
//! switch, because the engine's sim does not run. Not tested in play.
//!
//! | Field | Readers | Under the switch | Effect |
//! |---|---|---|---|
//! | `render_pose`, `pose_generation` | animation, presentation, custom_models, multiplayer, verification, cas_pose, customiser, cutscene, skate2_cast | filled by `step` | the skater is drawn (and sent online) in the port's pose |
//! | `animated_skeleton.roots.animation_to_world` (and `world_to_animation`) | presentation, camera/publication, the modes (skate1/2/3_*), match/online/map code, session_marker, traffic, minimap, vehicles | filled by `step` (the port's skater root) | where the modes, the map and the minimap see the skater |
//! | `player_input.physical.filtered_state_0` (and `wiping_out()`) | match_core, skate1_online, skate2_hall_of_meat, hud_meters, every mode's bail check | filled by `step` (the port's filtered category) | bails, air and teleports as the port has them |
//! | `animation.evaluator.frames.bone_names`, `.parents`, `.clip_count` | animation, multiplayer, hom_bones, customiser, cutscene, skate2_cast, retail_grading, team_photo, verification | static | none |
//! | `scoring.*` (announced, completed, hud_input, new_trick, sequence_landed, sequence_score, spin_turns, ...) | the scoring HUDs and every mode | filled by `step` from the port's tricks (Task 17) | the Skate 3 trick HUD and the modes see the port's tricks and scores |
//! | `scoring` collector reads (`riding`, `in_air`, `grinding`, `holding`, `carrier_metrics`, `air_measure`) | contests, challenges, trick stats | stale (cleared when Skate 2 starts driving, after the placement) | air and grind measures, held-trick timers |
//! | `hall_of_meat.*`, `collision_feedback`, `skeleton.record`, `skeleton.bodies`, `wipeout_state.state` | hall_of_meat, hom_bones, skate2_hall_of_meat, skate2_contests, skate3_uniform | stale | no engine Hall of Meat score or bail state; Skate 2's Hall of Meat (`skate2_hall_of_meat::tick`) takes the port's bail and score from `last_good.hom` instead (Task 18) |
//! | `player_state.current()`, `.state_flags`, `.filtered_output` | skate1_challenge, skate3_film, crash_context, customiser, hall_of_meat | stale | the modes that read the engine's state id see the last one before the switch |
//! | `player_input.physical.state.category_12`, `.state_16`, `.skateboard.vector_80`, `.scoring.capabilities_204`, the rest of `physical` | retail_grading, skate3_skate_game, skate2_challenge, skate3_film, session_marker, camera/publication, hall_of_meat | stale | off-board checks, board speed and the marker's stance read the last engine values |
//! | `player_input.processed.flags_2472` | presentation | bit 10 cleared by `step` (the rest stale) | no stale teleport cut; the port's teleport cuts through `camera.frame.discontinuity` |
//! | `hall_of_meat` (the gate: `enabled`, `stick`, the slow-motion request) | hall_of_meat's `gate` | written by the gate, which queues no request while `physics_skipped` | no engine Hall of Meat slow motion under the switch, and no queue that nothing drains |
//! | `player_input.pending_teleport()`, `travel_to*`, `travel_and_mark`, `teleport_state.request_*`, `mark_after_travel` | match_core, session_marker, online_boundary, vehicles, core_menu, teleport_menu, skate1/2 modes, performance | stale (the engine never takes the request) | the switch sends a travel to the port as a placement that ends the line; the request stays pending (a teleport in flight to the modes) until the port settles, then is dropped |
//! | `animation.goofy()`, `.foot_forward()`, `.packet.riding_*`, `.pose`, `.ticks`, `.motion` | hud_meters, session_marker, skate1_challenge, skate3_film, multiplayer/render, verification, customiser | stale | stance and the online fallback pose are the engine's last |
//! | `animation_input.*`, `ground.*`, `skeleton_collision.is_ragdoll`, `wipeout_state.hold_reset` | camera/publication, skate1_challenge, verification | stale | Skate 1 challenge checks (manual, pumps, ragdoll) read the last engine values |
//!
//! `camera.frame` (not the skater) is filled by `step` from the port's camera, except while the player watches a rider;
//! it cuts (`discontinuity`) while the port's category is Teleport and on the tick that leaves it. While a restart holds
//! the skater, `step` adds one `pose_generation` when the hold begins, so presentation stops blending.
//! The engine's audio observation (`physics::audio_observation::publish`) publishes per engine tick; under the switch
//! it publishes the port's frame once per port tick instead (`audio`, Task 18), with the riders' sounds.

pub mod audio;
pub mod camera;
pub mod input;
pub mod pose;
pub mod settings;
pub mod sim;
pub mod step;
pub mod switch;

use bevy::prelude::*;

pub use sim::Skate2Physics;

/// Adds the `Skate2Physics` resource, `Off`, and the switch, which starts the sim when Skate 2 physics is wanted.
pub struct Skate2PhysicsPlugin;

impl Plugin for Skate2PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Skate2Physics>().add_systems(Update, switch::update);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use skate2_physics_host::{Host, data, unclean_environment};
    use skate2_physics_protocol::{Boot, Pad};

    #[test]
    fn the_port_links_into_the_game() {
        assert_eq!(unclean_environment([("PATH".into(), "x".into())]), None);
        let dir = std::env::temp_dir().join(format!("sk8-skate2-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mounts = data::mount(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(mounts.unwrap().archives().is_empty(), "an empty folder holds no Skate 2 archives");
    }

    #[test]
    #[ignore = "needs the Skate 2 disc: S2HOST_TEST_DISC"]
    fn the_port_boots_inside_the_game() {
        // The port reads this process's environment (its `SKATE2_*` variables), so the switch's check runs here too.
        let vars = std::env::vars_os()
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()));
        if let Some(name) = unclean_environment(vars) {
            panic!("unset {name}: the port runs without SKATE2_* variables");
        }
        let root = std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to a Skate 2 data folder");
        // As the game will run it: the session lives on a thread of its own, so its boot cannot freeze the game.
        let frames = std::thread::Builder::new()
            .name("skate2-physics".into())
            .spawn(move || {
                let mut host = Host::boot(Path::new(&root), &Boot::default()).expect("the port boots");
                (0..60).map(|_| host.tick(Pad::default()).expect("a tick runs")).collect::<Vec<_>>()
            })
            .expect("the thread starts")
            .join()
            .expect("the port's thread did not panic");
        assert_eq!(frames.iter().map(|frame| frame.tick).collect::<Vec<_>>(), (1..=60).collect::<Vec<_>>());
        for frame in &frames {
            assert!(frame.deck.iter().all(|c| c.is_finite()), "tick {}: the deck is {:?}", frame.tick, frame.deck);
        }
    }
}
