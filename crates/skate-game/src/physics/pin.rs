//! User 10-27 (Party Play's Spot Battle on the Super Ultra Mega Park: sitting on "press A to start", the skater rolled
//! away). Retail's AI.PinAI holds the local skater with cMsgPinPlayer, the one message AI.ForceLocalPlayerStop sends
//! (TU3 0x8284adc8 / 0x8284ab18): the player is stopped, not only given neutral input. Retail pins right after the
//! mode's RepositionAI (spot_battle.lua 350-351, skate.lua 349-351) and keeps the pin through the Press-A card and the
//! countdown. Here a board that a held match (`ControllerInput::match_hold`) teleported is held at rest where it settled.
//! Retail's own pin is only a force brake (physics::advance; online fixes 11-01: command 7 at 0x825980F4 sets
//! actor+0x770 bit 26, nothing holds the board), which keeps a board landing across an edge from grinding off it; this
//! hold on top keeps the 10-27 and 10-31 stops on slopes.
use skate_core::{
    math::Vector3,
    physics::{assembly::BodySnapshot, board::BODY_COUNT, board_runtime::BoardRuntime, filtered_state::FilteredCategory},
};

/// Ground ticks a teleported board has to land and settle (its 0.1 m drop and bounce) before it is held.
pub(crate) const SETTLE: u32 = 15;

#[derive(Default)]
pub(crate) struct Pin {
    /// Ground ticks since the held match's teleport.
    landed: Option<u32>,
    /// The board as it settled, at rest: put back after every tick.
    held: Option<[BodySnapshot; BODY_COUNT]>,
}

impl Pin {
    /// After each physics tick: `pinned` (match_core's hold) and the tick's published category (`filtered_state_0`).
    pub(crate) fn step(&mut self, pinned: bool, category: u32, board: &mut BoardRuntime) {
        const GROUND: u32 = FilteredCategory::Ground as u32;
        const TELEPORT: u32 = FilteredCategory::Teleport as u32;
        match category {
            _ if !pinned => *self = Self::default(),
            TELEPORT => *self = Self { landed: Some(0), held: None },
            GROUND => {
                if let Some(landed) = self.landed.as_mut() {
                    *landed = landed.saturating_add(1);
                    if self.held.is_none() && *landed >= SETTLE {
                        let mut bodies = *board.bodies();
                        for body in &mut bodies {
                            (body.rates.linear_velocity, body.rates.angular_velocity) = (Vector3::ZERO, Vector3::ZERO);
                        }
                        self.held = Some(bodies);
                    }
                }
            }
            // A bail (or anything else off the ground) lets go until the next teleport.
            _ if self.held.is_some() => *self = Self::default(),
            _ => {}
        }
        if let Some(bodies) = self.held {
            *board.bodies_mut() = bodies;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skate_core::{
        math::Vector3,
        physics::{
            board_runtime::BoardMotion,
            drive_frames::RetailAffineTransform,
            filtered_state::FilteredCategory,
            rigid_body::{RetailBodyMassProperties, RetailInertiaDynamics, RetailLocalMassFrame, RetailSimulationStep},
        },
    };

    fn board() -> BoardRuntime {
        let mass = RetailBodyMassProperties {
            local_mass_frame: RetailLocalMassFrame::IDENTITY,
            dynamics: RetailInertiaDynamics {
                inverse_tensor: Vector3::new(1., 1., 1.),
                inverse_mass: 1.,
                spherical: 0.,
                maximum_linear_velocity: 100.,
                maximum_angular_velocity: 100.,
                linear_drag: 0.,
                angular_drag: 0.,
            },
        };
        BoardRuntime::new(
            [mass; 7],
            [RetailAffineTransform::IDENTITY; 7],
            RetailAffineTransform::IDENTITY,
            RetailSimulationStep::fixed_60_hz(0, 0., Vector3::ZERO),
            BoardMotion::Active,
        )
    }
    /// What a tick of physics does to a board rolling down a slope: on along +x, faster.
    fn roll(board: &mut BoardRuntime) {
        for body in board.bodies_mut() {
            body.rates.linear_velocity.x += 0.1;
            body.rates.position.x += body.rates.linear_velocity.x / 60.;
        }
    }
    fn x(board: &BoardRuntime) -> f32 {
        board.bodies()[6].rates.position.x
    }

    /// The pin catches a board the held match teleported once it has landed and settled, holds it there at rest
    /// through every tick that rolls it, and lets go with the hold, or with a bail.
    #[test]
    fn a_teleported_board_is_held_where_it_settled_until_the_hold_ends() {
        let [ground, teleport, air, wipeout] =
            [FilteredCategory::Ground, FilteredCategory::Teleport, FilteredCategory::Air, FilteredCategory::Wipeout].map(|c| c as u32);
        let (mut board, mut pin) = (board(), Pin::default());
        // A hold with no teleport before it (a run's end, the results) leaves the board to the physics.
        roll(&mut board);
        pin.step(true, ground, &mut board);
        assert!(x(&board) > 0., "a hold alone coasts");
        // The mode's teleport, the drop onto the spot (air), the landing: the physics' until it settles.
        pin.step(true, teleport, &mut board);
        for _ in 0..3 {
            roll(&mut board);
            pin.step(true, air, &mut board);
        }
        for _ in 1..SETTLE {
            roll(&mut board);
            pin.step(true, ground, &mut board);
        }
        let settling = x(&board);
        assert!(board.bodies()[6].rates.linear_velocity.x > 0., "not held before it settles");
        roll(&mut board);
        pin.step(true, ground, &mut board);
        let caught = x(&board);
        assert!(caught > settling, "caught where it got to");
        assert!(board.bodies().iter().all(|b| b.rates.linear_velocity == Vector3::ZERO && b.rates.angular_velocity == Vector3::ZERO), "at rest");
        for _ in 0..3600 {
            roll(&mut board);
            pin.step(true, ground, &mut board);
        }
        assert_eq!(x(&board), caught, "a minute on, still there");
        // The hold ends: the physics has it again.
        roll(&mut board);
        pin.step(false, ground, &mut board);
        assert!(x(&board) > caught);
        // Held again after another teleport, then a bail: let go.
        pin.step(true, teleport, &mut board);
        for _ in 0..SETTLE {
            roll(&mut board);
            pin.step(true, ground, &mut board);
        }
        let held = x(&board);
        roll(&mut board);
        pin.step(true, ground, &mut board);
        assert_eq!(x(&board), held);
        roll(&mut board);
        pin.step(true, wipeout, &mut board);
        roll(&mut board);
        pin.step(true, ground, &mut board);
        assert!(x(&board) > held, "a bail lets go until the next teleport");
    }
}
