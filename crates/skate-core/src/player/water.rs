//! PhysicalPlayer 82DB8120, which ProcessOutput 82DB6EC0 calls at 82DB7C58 after every Fill: per physical state, the
//! water (physics surface 12) the skater touches or is about to land on. Collision3481/28 become Processed2488 bit30
//! and 2924 on the next input, Wipeout's special surface (82D3B9C8); 82DB80C8 requests the bail itself (State65).
//! 82DB80C8 also requests a teleport (State69, `stb 1,0x45`) for surface 6 (Unrideable: Industrial's whole sea is one
//! such plane at y -4), and Wipeout300 (82DB81CC) teleports on a ragdoll touch of 6 (Collision214). Skate 3 maps only
//! (the caller's gate). shortcut: 82DB80C8's wipeout for 9 (InstantBail) is unported; port it when the user asks.
use super::input_phase::{PhysicalPlayerInput, ProcessedPhysicsInput};

/// The water surface of a packed collision tag (`rlwinm 25,27,31`).
pub const WATER: u32 = 12;
/// Unrideable: 82DB80C8's teleport surface.
pub const UNRIDEABLE: u32 = 6;

/// What 82DB8120 reads besides the processed input and the completed physical output.
pub struct Sources<'a> {
    pub processed: &'a ProcessedPhysicsInput,
    pub physical: &'a PhysicalPlayerInput,
    /// Collision16, the board's surface vote 82C08818.
    pub board_surface: u32,
    /// Body872 bit25 and Body864: a board part touched water, at this height.
    pub board_water: Option<f32>,
    /// TrajectorySelector9640, which ProcessOutput 82DB7054 copies to Air232.
    pub selection_surface: u32,
    /// SkeletonCollision4081/4064, which 82BD60C8 copies to Collision217/208.
    pub ragdoll_water: Option<f32>,
    /// Collision214 (82BD60C8 copies SkeletonCollision4078): the ragdoll touched surface 6.
    pub ragdoll_unrideable: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Found {
    /// State65: request the wipeout.
    pub wipeout: bool,
    /// Collision3481 and Collision28: in, or about to land in, water at this height.
    pub water: Option<f32>,
    /// State69: request the teleport.
    pub teleport: bool,
}

/// 8208081C: the air branches' "lands within" gate. 82DB8190/82DB82DC `bge` also rejects NaN.
const LANDING_SOON: f32 = f32::from_bits(0x3D85_1EB8);

/// 82DB80C8's wipeout, water only (see the shortcut above).
fn requests_wipeout(surface: u32) -> bool {
    surface == WATER
}

/// 82DB80C8's teleport (82DB8100).
fn requests_teleport(surface: u32) -> bool {
    surface == UNRIDEABLE
}

/// One surface the state stands on, from 82DB80C8's input and the same branch's height.
fn standing(surface: u32, height: f32) -> Found {
    Found {
        wipeout: requests_wipeout(surface),
        water: (surface == WATER).then_some(height),
        teleport: requests_teleport(surface),
    }
}

pub fn check(s: &Sources<'_>) -> Found {
    let (p, o) = (s.processed, s.physical);
    let line = |i: usize| {
        let line = p.line_tests_960_1008_1056[i];
        ((line.surface >> 7) & 31, f32::from_bits(line.position[1]))
    };
    match p.state_2508 {
        //82DB832C: the board's vote, then Body872 bit25 and Body864 whether or not that vote is water.
        100..=104 | 503 => Found {
            wipeout: requests_wipeout(s.board_surface),
            water: s.board_water,
            teleport: requests_teleport(s.board_surface),
        },
        //82DB816C: Air232/184/20.
        201 if s.selection_surface == WATER && o.air.scalar_184 < LANDING_SOON => Found {
            wipeout: true,
            water: Some(f32::from_bits(o.air.collision_position_16[1])),
            teleport: false,
        },
        //82DB81CC: Collision214 teleports; Collision217/208, else Collision3483 (mIsLandingOnWater) and 68.
        300 => Found {
            wipeout: false,
            teleport: s.ragdoll_unrideable,
            water: s.ragdoll_water.or((o.collision.flag_3483 != 0)
                .then(|| f32::from_bits(o.collision.predicted_position_64[1]))),
        },
        //82DB8258: the planted foot's line (Offboard306 left, 307 right), else both lines in water at the lower.
        500 | 502 => {
            let ((s0, y0), (s1, y1)) = (line(0), line(1));
            match o.off_board.flags_306_307 {
                [left, _] if left != 0 => standing(s0, y0),
                [_, right] if right != 0 => standing(s1, y1),
                _ if s0 == WATER && s1 == WATER => Found {
                    wipeout: true,
                    water: Some(y1.min(y0)),
                    teleport: false,
                },
                _ => Found::default(),
            }
        }
        //82DB82B8: Offboard144/32/148.
        501 if o.off_board.word_144 == WATER && o.off_board.scalar_32 < LANDING_SOON => Found {
            wipeout: true,
            water: Some(o.off_board.scalar_148),
            teleport: false,
        },
        //82DB847C: Air448, Air224's surface, Air216.
        601 if o.air.flag_448 != 0 => standing(
            (o.air.footplant_surface_224 >> 7) & 31,
            o.air.footplant_surface_height_216,
        ),
        //82DB84D8: line test 2 while it hit.
        602 if p.line_tests_960_1008_1056[2].valid != 0 => {
            let (surface, height) = line(2);
            standing(surface, height)
        }
        _ => Found::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::input_phase::LineTestFields;

    const WATER_TAG: u32 = 0x637; // University's water triangles: surface 12, audio 0x37.
    const GROUND_TAG: u32 = 0x80;

    fn sources<'a>(p: &'a ProcessedPhysicsInput, o: &'a PhysicalPlayerInput) -> Sources<'a> {
        Sources {
            processed: p,
            physical: o,
            board_surface: 1,
            board_water: None,
            selection_surface: 0,
            ragdoll_water: None,
            ragdoll_unrideable: false,
        }
    }
    fn state(id: u32) -> ProcessedPhysicsInput {
        ProcessedPhysicsInput {
            state_2508: id,
            ..Default::default()
        }
    }
    fn line(tag: u32, y: f32) -> LineTestFields {
        LineTestFields {
            position: [0.0, y, 0.0, 0.0].map(f32::to_bits),
            surface: tag,
            valid: 1,
            ..Default::default()
        }
    }

    #[test]
    fn riding_onto_water_requests_the_bail_and_publishes_the_board_height() {
        let o = PhysicalPlayerInput::default();
        for id in [100, 101, 102, 103, 104, 503] {
            let p = state(id);
            let mut s = sources(&p, &o);
            s.board_surface = WATER;
            s.board_water = Some(217.87);
            assert_eq!(check(&s), Found { wipeout: true, water: Some(217.87), teleport: false }, "state {id}");
        }
        // FollowPath105 and the air/grind states have no branch.
        for id in [105, 200, 202, 400] {
            let p = state(id);
            let mut s = sources(&p, &o);
            s.board_surface = WATER;
            s.board_water = Some(1.0);
            assert_eq!(check(&s), Found::default(), "state {id}");
        }
    }

    #[test]
    fn dry_ground_and_the_unported_surfaces_request_nothing() {
        let o = PhysicalPlayerInput::default();
        let p = state(100);
        for surface in [1, 9] {
            let mut s = sources(&p, &o);
            s.board_surface = surface;
            assert_eq!(check(&s), Found::default(), "surface {surface}");
        }
    }

    /// 82DB80C8 82DB8100: Unrideable sets State69 alone (no wipeout, no water), from every branch that passes it a
    /// surface: the riding vote, the planted foot, FootPlant and Boneless.
    #[test]
    fn unrideable_ground_requests_the_teleport() {
        let teleport = Found { teleport: true, ..Found::default() };
        const UNRIDEABLE_TAG: u32 = 0x300; // Industrial's sea plane: surface 6, audio 0.
        let mut o = PhysicalPlayerInput::default();
        for id in [100, 101, 102, 103, 104, 503] {
            let p = state(id);
            let mut s = sources(&p, &o);
            s.board_surface = UNRIDEABLE;
            assert_eq!(check(&s), teleport, "state {id}");
        }
        // The air states pass no surface to 82DB80C8.
        for id in [200, 201, 501] {
            let p = state(id);
            let mut s = sources(&p, &o);
            (s.board_surface, s.selection_surface) = (UNRIDEABLE, UNRIDEABLE);
            assert_eq!(check(&s), Found::default(), "state {id}");
        }
        let mut p = state(500);
        p.line_tests_960_1008_1056[0] = line(UNRIDEABLE_TAG, -4.0);
        p.line_tests_960_1008_1056[1] = line(UNRIDEABLE_TAG, -4.0);
        assert_eq!(check(&sources(&p, &o)), Found::default(), "no planted foot: only water counts (82DB8258)");
        o.off_board.flags_306_307 = [1, 0];
        assert_eq!(check(&sources(&p, &o)), teleport, "planted left foot");
        let mut o = PhysicalPlayerInput::default();
        o.air.flag_448 = 1;
        o.air.footplant_surface_224 = UNRIDEABLE_TAG;
        assert_eq!(check(&sources(&state(601), &o)), teleport, "FootPlant");
        let mut p = state(602);
        p.line_tests_960_1008_1056[2] = line(UNRIDEABLE_TAG, -4.0);
        assert_eq!(check(&sources(&p, &PhysicalPlayerInput::default())), teleport, "Boneless");
    }

    /// 82DB81CC: Wipeout300 teleports on Collision214, the ragdoll's touch of surface 6, beside its water outputs.
    #[test]
    fn a_ragdoll_touching_unrideable_requests_the_teleport() {
        let o = PhysicalPlayerInput::default();
        let p = state(300);
        let mut s = sources(&p, &o);
        s.ragdoll_unrideable = true;
        assert_eq!(check(&s), Found { teleport: true, ..Found::default() });
        s.ragdoll_water = Some(4.5);
        assert_eq!(check(&s), Found { wipeout: false, water: Some(4.5), teleport: true });
    }

    #[test]
    fn known_air_bails_only_just_before_landing_in_water() {
        let mut o = PhysicalPlayerInput::default();
        o.air.collision_position_16 = [0.0, 8.93, 0.0, 0.0].map(f32::to_bits);
        let p = state(201);
        for (surface, time, found) in [
            (WATER, 0.05, Found { wipeout: true, water: Some(8.93), teleport: false }),
            (WATER, 0.065, Found::default()), // 82DB8190 bge: not below 8208081C
            (WATER, f32::NAN, Found::default()),
            (1, 0.0, Found::default()),
        ] {
            o.air.scalar_184 = time;
            let mut s = sources(&p, &o);
            s.selection_surface = surface;
            assert_eq!(check(&s), found, "surface {surface} time {time}");
        }
    }

    #[test]
    fn a_wipeout_finds_water_by_touch_then_by_prediction_without_a_new_request() {
        let mut o = PhysicalPlayerInput::default();
        o.collision.predicted_position_64 = [0.0, 5.0, 0.0, 0.0].map(f32::to_bits);
        let p = state(300);
        assert_eq!(check(&sources(&p, &o)), Found::default());
        o.collision.flag_3483 = 1;
        assert_eq!(check(&sources(&p, &o)), Found { wipeout: false, water: Some(5.0), teleport: false });
        let mut s = sources(&p, &o);
        s.ragdoll_water = Some(4.5);
        assert_eq!(check(&s), Found { wipeout: false, water: Some(4.5), teleport: false });
    }

    #[test]
    fn walking_uses_the_planted_foot_or_both_feet_in_water() {
        let mut o = PhysicalPlayerInput::default();
        for id in [500, 502] {
            let mut p = state(id);
            p.line_tests_960_1008_1056[0] = line(WATER_TAG, 2.0);
            p.line_tests_960_1008_1056[1] = line(GROUND_TAG, 3.0);
            // Neither foot planted: both lines must be water, at the lower height.
            assert_eq!(check(&sources(&p, &o)), Found::default());
            p.line_tests_960_1008_1056[1] = line(WATER_TAG, 1.5);
            assert_eq!(check(&sources(&p, &o)), Found { wipeout: true, water: Some(1.5), teleport: false });
            // The left foot (Offboard306) decides alone, then the right (307).
            o.off_board.flags_306_307 = [1, 0];
            assert_eq!(check(&sources(&p, &o)), Found { wipeout: true, water: Some(2.0), teleport: false });
            p.line_tests_960_1008_1056[0] = line(GROUND_TAG, 2.0);
            assert_eq!(check(&sources(&p, &o)), Found::default());
            o.off_board.flags_306_307 = [0, 1];
            assert_eq!(check(&sources(&p, &o)), Found { wipeout: true, water: Some(1.5), teleport: false });
            o.off_board.flags_306_307 = [0, 0];
        }
    }

    #[test]
    fn biped_air_footplant_and_boneless_read_their_own_landing() {
        let mut o = PhysicalPlayerInput::default();
        o.off_board.word_144 = WATER;
        o.off_board.scalar_32 = 0.01;
        o.off_board.scalar_148 = 7.0;
        assert_eq!(check(&sources(&state(501), &o)), Found { wipeout: true, water: Some(7.0), teleport: false });
        o.off_board.scalar_32 = 0.5;
        assert_eq!(check(&sources(&state(501), &o)), Found::default());

        o.air.footplant_surface_224 = WATER_TAG;
        o.air.footplant_surface_height_216 = 6.0;
        assert_eq!(check(&sources(&state(601), &o)), Found::default(), "Air448 clear");
        o.air.flag_448 = 1;
        assert_eq!(check(&sources(&state(601), &o)), Found { wipeout: true, water: Some(6.0), teleport: false });

        let mut p = state(602);
        p.line_tests_960_1008_1056[2] = line(WATER_TAG, 9.0);
        assert_eq!(check(&sources(&p, &o)), Found { wipeout: true, water: Some(9.0), teleport: false });
        p.line_tests_960_1008_1056[2].valid = 0;
        assert_eq!(check(&sources(&p, &o)), Found::default());
    }
}
