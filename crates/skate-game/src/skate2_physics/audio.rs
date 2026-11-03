//! The skater's sound under the Skate 2 physics switch (Task 18). Not tested in play.
//!
//! While Skate 2 physics has the local skater the engine's sim does not run, so `physics::audio_observation::publish`
//! builds the player's audio observation from the port's frame (`Skate2Physics::last_good`) instead, once per port
//! tick. The sounds stay Skate 3's (spec 3.3): the port's motion drives our sound engine.
//!
//! What the observation takes from the frame (`Frame.audio`, the port's `SkaterAudioPhysOut`, and `Frame.tricks`):
//!
//! | Field | From |
//! |---|---|
//! | `board_speed` | `skateboard_surface_speed` |
//! | `rider_speed` | the length of `centre_of_mass_velocity`'s xyz |
//! | `grounded` | `!is_skateboard_airborne` |
//! | `grinding` | `tricks.grinding` |
//! | `wiping_out` | `is_wiping_out` |
//! | `landed` | `tricks.just_landed` |
//! | `landing_sketchy` | `tricks.sketchy` |
//! | `contact_count` | `num_wheels_in_contact` |
//! | `powersliding` | trick 0 or 1 in `tricks.active` |
//! | `wheel_surface` | the engine's ground probe at the port's wheel positions (`wheel_surfaces`) |
//!
//! Every other field keeps its default: `state`, the grind fields, the landing's impact, type, spin and clean flag, the
//! footsteps, `foot_surface`, the trick id and name, the animation name, the stance flags and `events`.
//!
//! `retail` (the record the sound engine actually runs on, `player_audio::game_side`) takes the same port values in
//! its own slots, listed in `retail`; the rest of it keeps its default. Not native Skate 2 (user 10-20): Skate 2's
//! sound is not used, so Skate 3's sound engine hears the port's motion through Skate 3's record.
//!
//! User 10-22 ("pushing sounds like a powerslide"): the record's deck rows were left at identity, so the skid voice
//! measured world X as the board's sideways speed, and its push, brake and revert flags were never set. Both now come
//! from the port (`deck_rows`, `state_flags`; protocol 3).
//!
//! User 10-22 ("there doesnt seem to be any bail sounds or any walking sounds or any wheel spinning sounds"): the
//! record's off-board feet, footplants, foot velocities, ragdoll spins, limb speeds and body contacts were left at
//! their defaults, and its bail (59) and balance (60) flags were never set; the bail voices gate on 59, the steps on
//! the feet going down, the manual's wheel spin on 60 and the walking one on the board in the hand. All now come from
//! the port's audio PhysOut (protocol 4), the surfaces from the engine's ground under the skater (`Wheels::body`).

use skate2_physics_protocol::{Frame, SkaterAudio};
use skate_core::math::Vector3;
use skate_core::physics::board_ground::WHEEL_LINE_LENGTH;
use skate_core::physics::board_world::BoardWorld;
use skate_core::physics::filtered_state::FilteredCategory;

use super::Skate2Physics;
use crate::skate_audio::{PlayerAudioObservation, RetailAudioInputs};

/// The port's powerslide trick ids (the brief's 0 and 1).
const POWERSLIDES: [u16; 2] = [0, 1];

fn xyz(v: [f32; 4]) -> [f32; 3] {
    [v[0], v[1], v[2]]
}

fn length(v: [f32; 4]) -> f32 {
    v[0].mul_add(v[0], v[1].mul_add(v[1], v[2] * v[2])).sqrt()
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = length([v[0], v[1], v[2], 0.0]);
    (l.is_finite() && l > 1e-6).then(|| v.map(|c| c / l))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The board's right, up and forward axes, as the engine's deck rows (`deck.basis.columns`: 0 right, 1 up, 2
/// forward): up is the port's `effective_transform_y`, forward its deck nose, right = up x forward (the engine's level
/// heading `h` has right `[cos h, 0, -sin h]`). `None` for a frame with no axes (a default one).
fn deck_rows(audio: &SkaterAudio) -> Option<[[f32; 3]; 3]> {
    let up = normalized(xyz(audio.deck_up))?;
    let forward = normalized(xyz(audio.deck_nose))?;
    Some([normalized(cross(up, forward))?, up, forward])
}

/// What the engine's wheel line found under each of the port's wheels ([`wheel_surfaces`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Wheels {
    /// The material, `tag & 0x7f`.
    pub surfaces: [u32; 4],
    /// The seam pattern, `(tag >> 12) & 0xF`, as the engine's wheel lines read it.
    pub seams: [u32; 4],
    pub hits: [bool; 4],
    /// The engine's collision tag under the skater's centre of mass (`BODY_LINE_LENGTH` down), 0 with none: the feet's
    /// and the body's surface. Not native Skate 2 (user 10-20): the port's own surface ids are Skate 2's world's.
    pub body: u32,
}

/// How far below the centre of mass `wheel_surfaces` looks for the ground under the skater: standing it is about 1 m
/// up.
const BODY_LINE_LENGTH: f32 = 2.5;

/// The vault's body impact scale (`physics_collision` +164, `player_audio/tuning.rs`), which the conditioner multiplies
/// the record's weighted changes by.
const BODY_IMPACT_SCALE: f32 = 10.0;

/// The audio observation of the port's frame `frame`, numbered `tick`, on the ground material `wheel_surface` (the
/// vote over `wheels`).
pub(crate) fn observation(frame: &Frame, tick: u64, wheel_surface: u32, wheels: &Wheels) -> PlayerAudioObservation {
    let audio = &frame.audio;
    let contacts = audio.num_wheels_in_contact.clamp(0, 4) as u32;
    let grinding = frame.tricks.grinding;
    PlayerAudioObservation {
        tick,
        board_speed: audio.skateboard_surface_speed,
        rider_speed: length(audio.centre_of_mass_velocity),
        grounded: !audio.is_skateboard_airborne,
        grinding,
        wiping_out: audio.is_wiping_out,
        landed: frame.tricks.just_landed,
        landing_sketchy: frame.tricks.sketchy,
        contact_count: contacts,
        powersliding: frame.tricks.active.iter().any(|id| POWERSLIDES.contains(id)),
        wheel_surface,
        retail: retail(frame, contacts, wheel_surface, wheels),
        ..PlayerAudioObservation::default()
    }
}

/// Skate 3's `(state_category, state)` (skate-core `player/state.rs`) for the port's filtered category, which the
/// sound bridge gates on: grinds on category 400 (`audio_state.rs` `grind`), walking on 500 (`walking_716`). One
/// representative state each: PhysicsGround, KnownAir, WipeoutGround, GrindFiftyFifty, BipedGround, BipedAir,
/// Teleporting (review I3). Not native Skate 2 (user 10-20): the port's own state numbers are Skate 2's.
fn state_ids(category: FilteredCategory) -> (u32, u32) {
    match category {
        FilteredCategory::Ground => (100, 100),
        FilteredCategory::Air => (200, 201),
        FilteredCategory::Wipeout => (300, 300),
        FilteredCategory::Grind => (400, 401),
        FilteredCategory::Offboard => (500, 500),
        FilteredCategory::OffboardAir => (500, 501),
        FilteredCategory::Teleport => (700, 702),
        FilteredCategory::Invalid => (0, 0),
    }
}

/// Skate 3's state flags (`state_flags[state - 52]`, physics/player_state/publication.rs) from the port's bits, which
/// are the same native bits (Processed+0x8C4 / `flags_2468`): 52 foot brake, 54 manual brake, 55 physically pushing,
/// 56 generally pushing, 57 pushing with the right foot, 66 reverting. 55 goes only with 56 or 57: alone it re-posts
/// the push on every tick (`audio_state.rs` `push_plant_335`). 58 (generally braking) is not in the port's audio
/// PhysOut and stays clear. 59 is the wipeout bit (`flags_2468` bit 18, the PhysOut's `is_wiping_out`), which the
/// bail voices gate on (`audio_state.rs` `bail_676`); 60 is a non-zero balance, the manual (`is_manual`), which the
/// manual's wheel spin gates on (`balance_340`).
fn state_flags(audio: &SkaterAudio) -> [bool; 36] {
    let mut flags = [false; 36];
    let pushing = audio.is_generally_pushing || audio.is_generally_pushing_right_foot;
    for (state, on) in [
        (52, audio.is_foot_braking),
        (54, audio.is_manual_braking),
        (55, audio.is_physically_pushing && pushing),
        (56, audio.is_generally_pushing),
        (57, audio.is_generally_pushing_right_foot),
        (59, audio.is_wiping_out),
        (60, audio.is_manual),
        (66, audio.is_reverting),
    ] {
        flags[state - 52] = on;
    }
    flags
}

/// The retail record's slots that have a port value of the same meaning. Not native Skate 2 (user 10-20): the record
/// is Skate 3's. The state ids come from the port's category ([`state_ids`]), with RevertGround (102) for a revert on
/// the ground; `motion_200` is 1, the hard-wheel set (as the headless roll, `player_audio/headless.rs`; below 0.5 the
/// bridge picks soft wheels); while grinding the rail's material is the probed ground's (shortcut: the protocol has no
/// rail material; carry it when it does). The wheels that touch are the ones the engine's wheel line found ground
/// under while the port has any wheel down (shortcut: the port's own per-wheel contacts are not in its audio PhysOut;
/// with no probe hit, the first `contacts`). Off the board the feet (`offboard_feet`, the air footplant, their
/// velocities), the board in the hand (`offboard_311`), the ragdoll spins, limb speeds and body contacts are the port's
/// ([`body_contacts`]); the feet's and body's surface is the engine's ground under the skater (`Wheels::body`). Left
/// at the default: the grind family and impact, the hold timer (`offboard_309`/`310`), the toe positions (the step
/// up/down codes: shortcut: the protocol has no toes; flat steps until it does), the footstep strength, and the
/// scoring slots.
fn retail(frame: &Frame, contacts: u32, wheel_surface: u32, wheels: &Wheels) -> RetailAudioInputs {
    let audio = &frame.audio;
    let touching = if contacts > 0 && wheels.hits.contains(&true) {
        wheels.hits
    } else {
        std::array::from_fn(|i| (i as u32) < contacts)
    };
    let category = super::step::category(frame.category);
    let (state_category, state) = state_ids(category);
    let state = if category == FilteredCategory::Ground && audio.is_reverting { 102 } else { state };
    let deck_velocity = xyz(audio.deck_velocity);
    let rows = deck_rows(audio);
    let defaults = RetailAudioInputs::default();
    // The port's angular speed is in the deck's axes; the record's is the world's, which the bridge projects on the
    // rows again.
    let local = xyz(audio.deck_local_angular_speed);
    let angular_velocity = rows.map_or(defaults.angular_velocity, |rows| {
        std::array::from_fn(|k| (0..3).map(|i| local[i] * rows[i][k]).sum())
    });
    let part = |on: bool| if on { wheel_surface } else { 0 };
    RetailAudioInputs {
        deck_rows: rows.unwrap_or(defaults.deck_rows),
        effective_deck_up: rows.map_or(defaults.effective_deck_up, |rows| rows[1]),
        deck_forward: rows.map_or(defaults.deck_forward, |rows| rows[2]),
        angular_velocity,
        deck_tilt: audio.deck_tilt,
        state_flags: state_flags(audio),
        feet_in_deck_box: [audio.is_left_foot_on_deck, audio.is_right_foot_on_deck],
        truck_contacts: [audio.is_front_truck_in_contact, audio.is_back_truck_in_contact],
        deck_contact: audio.is_deck_in_contact,
        part_audio_surfaces: [
            part(audio.is_front_truck_in_contact),
            part(audio.is_back_truck_in_contact),
            part(audio.is_deck_in_contact),
        ],
        wheel_seam_patterns: std::array::from_fn(|i| if touching[i] && wheels.hits[i] { wheels.seams[i] } else { 0 }),
        state_category,
        state,
        // As `audio_observation::in_known_air`: an air state with no wheel down.
        in_known_air: category == FilteredCategory::Air && contacts == 0,
        // Flat ground under each touching wheel (the probe's normal is not kept): the wheel landing's closing speed.
        wheel_contact_normals: touching.map(|t| if t { [0.0, 1.0, 0.0] } else { [0.0; 3] }),
        wheel_velocities: [deck_velocity; 4],
        motion_200: 1.0,
        grind_audio_surface: if frame.tricks.grinding { wheel_surface } else { 0 },
        dt: frame.sim_period_s,
        ground_speed: audio.skateboard_surface_speed,
        com_velocity: xyz(audio.centre_of_mass_velocity),
        position: frame.deck,
        wheel_count: contacts,
        air_time_in_state: audio.time_in_known_air,
        air_jump_height: audio.jump_height,
        filtered_state: category as u32,
        grinding: frame.tricks.grinding,
        wheel_contacts: touching,
        wheel_audio_surfaces: std::array::from_fn(|i| match (touching[i], wheels.hits[i]) {
            (true, true) => wheels.surfaces[i],
            (true, false) => wheel_surface,
            (false, _) => 0,
        }),
        deck_slide_speed: audio.deck_slide_speed,
        linear_velocity: deck_velocity,
        wheel_positions: audio.wheel_pos.map(xyz),
        camera: Some((frame.camera.forward, frame.camera.position)),
        deck_position: frame.deck,
        turn: audio.turn,
        offboard_feet: [audio.left_foot_down, audio.right_foot_down],
        footplant: [audio.is_left_foot_planting, audio.is_right_foot_planting],
        footplant_448: audio.is_foot_planting,
        footplant_surface: wheels.body,
        offboard_surface: wheels.body,
        offboard_311: audio.is_holding_skateboard,
        skeleton_599: audio.is_wipeout_over,
        foot_local_velocity: [xyz(audio.foot_left_velocity_deck), xyz(audio.foot_right_velocity_deck)],
        foot_world_velocity: [xyz(audio.foot_left_velocity), xyz(audio.foot_right_velocity)],
        // Skate 3's parts 23, 20 and 16 (`audio_observation.rs`): the groin, the right foot and the left foot.
        ragdoll_spin: [audio.torso_angular_velocity, audio.right_foot_angular_velocity, audio.left_foot_angular_velocity]
            .map(xyz),
        limb_speeds: audio.average_limb_speed,
        body_contacts: body_contacts(audio, wheels.body),
        // User 10-22 ("when i do a trick and im in the air i feel like the sound instantly stops"): the trick being
        // scored, which the trick and air voices key on (measured headless: a kickflip's air is ~20 dB louder than an
        // ollie's). The port's trick ids are the EScorableIDs (`scoring_runtime_port.rs`, `IDENTIFIERS[id]`).
        scorable_id: frame
            .tricks
            .active
            .first()
            .filter(|id| usize::from(**id) < skate_core::scoring::catalog::IDENTIFIERS.len())
            .map_or(-1, |id| i32::from(*id)),
        ..RetailAudioInputs::default()
    }
}

/// The record's body contacts from the port's averaged ones (`averaged_skeleton_collision_info`). The port's contact
/// force is a region's finished impact (0..1, Skate 3's Collision+80); the record carries the weighted change the
/// conditioner finishes (`audio_state.rs` `body_contacts`: × `BODY_IMPACT_SCALE`, then clamped), so it goes in
/// divided by the scale. INFERRED: the two games' averaged contact forces are the same quantity (same native member
/// name, `SkeletonCollisionInfoAudio::mContactForce`). The material of every touching region is the ground's under the
/// skater (shortcut: the protocol has no per-region surface; carry it when it does). Not native Skate 2 (user 10-20):
/// the forces feed Skate 3's bail sounds.
fn body_contacts(audio: &SkaterAudio, ground: u32) -> crate::skate_audio::BodyContacts {
    let contact = audio.body_contact_force.map(|force| force > 0.0);
    crate::skate_audio::BodyContacts {
        contact,
        weighted_change: audio.body_contact_force.map(|force| force.max(0.0) / BODY_IMPACT_SCALE),
        slide: std::array::from_fn(|i| if contact[i] { audio.body_slide_speed[i] } else { 0.0 }),
        material: contact.map(|on| if on { ground & 0x7f } else { 0 }),
        specific_current: [audio.is_groin_contact, audio.is_face_contact],
        other_skater: -1,
        ..crate::skate_audio::BodyContacts::default()
    }
}

/// The ground materials under the port's wheels (`Frame.audio.wheel_pos`) and which wheels found ground: the engine's
/// own wheel line, 0.2 m straight down from each wheel into the engine's collision. `audio_observation::publish` takes
/// the engine's vote over the four (`ground_runtime::choose_audio_surface`). Not native Skate 2 (user 10-20): the
/// materials are Skate 3's sound banks'; the engine's line runs along the skater's up, this one along the world's (the
/// protocol has no reckoning up).
pub(crate) fn wheel_surfaces(world: &BoardWorld, audio: &SkaterAudio) -> Wheels {
    let mut found = Wheels::default();
    for (i, [x, y, z, _]) in audio.wheel_pos.iter().copied().enumerate() {
        if let Ok(Some(hit)) = world.query_thin_line(Vector3::new(x, y, z), Vector3::new(x, y - WHEEL_LINE_LENGTH, z)) {
            found.surfaces[i] = hit.tag & 0x7f;
            found.seams[i] = (hit.tag >> 12) & 0xF;
            found.hits[i] = true;
        }
    }
    let [x, y, z, _] = audio.centre_of_mass;
    if let Ok(Some(hit)) = world.query_thin_line(Vector3::new(x, y, z), Vector3::new(x, y - BODY_LINE_LENGTH, z)) {
        found.body = hit.tag;
    }
    found
}

/// The port's frame for this publish: `None` while the engine's sim has the skater (the engine's own observation
/// publishes); `Some(None)` while Skate 2 physics has it but its last frame was already published (`published`, the
/// last port tick published), which holds through a restart; else `Some(frame)`, marked published.
pub(crate) fn due<'a>(skate2: Option<&'a Skate2Physics>, published: &mut Option<u32>) -> Option<Option<&'a Frame>> {
    let skate2 = skate2.filter(|s| s.drives_local())?;
    let frame = skate2.last_good.as_deref().filter(|frame| *published != Some(frame.tick));
    if let Some(frame) = frame {
        *published = Some(frame.tick);
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use skate2_physics_protocol::{Boot, SkaterAudio, Tricks};

    use super::*;
    use crate::skate2_physics::sim::{HostState, Link, Refused};

    #[test]
    fn the_audio_record_takes_the_ports_motion() {
        let frame = Frame {
            audio: SkaterAudio {
                skateboard_surface_speed: 5.5,
                centre_of_mass_velocity: [3.0, 0.0, 4.0, 0.0],
                is_skateboard_airborne: false,
                num_wheels_in_contact: 4,
                is_wiping_out: true,
                ..SkaterAudio::default()
            },
            tricks: Tricks { grinding: true, active: vec![1], ..Tricks::default() },
            ..Frame::default()
        };
        let o = observation(&frame, 9, 3, &Wheels::default());
        assert_eq!(o.tick, 9);
        assert_eq!(o.board_speed, 5.5);
        assert!((o.rider_speed - 5.0).abs() < 1e-6, "{}", o.rider_speed);
        assert!(o.grounded);
        assert_eq!(o.contact_count, 4);
        assert!(o.wiping_out && o.grinding);
        assert!(o.powersliding, "trick 1 is a powerslide");
        assert_eq!(o.wheel_surface, 3);
        // The rest keeps its default.
        assert!(!o.landed && !o.landing_sketchy && o.events.is_empty() && o.trick_id.is_none());
        // The record the sound engine runs on.
        assert_eq!(o.retail.ground_speed, 5.5);
        assert_eq!(o.retail.wheel_count, 4);
        assert_eq!(o.retail.wheel_audio_surfaces, [3; 4]);
        assert_eq!(o.retail.com_velocity, [3.0, 0.0, 4.0]);
        // Review I3: the retail record's state ids by the port's category (Skate 3's, skate-core player/state.rs),
        // which the sound bridge gates grinds (state_category 400) and walking (500) on.
        for (byte, ids) in [(1u8, (100, 100)), (2, (200, 201)), (3, (400, 401)), (4, (300, 300)), (5, (700, 702)), (6, (500, 500)), (7, (500, 501)), (0, (0, 0))] {
            let o = observation(&Frame { category: byte, ..frame.clone() }, 9, 3, &Wheels::default());
            assert_eq!((o.retail.state_category, o.retail.state), ids, "category byte {byte}");
        }
        let grind = observation(&Frame { category: 3, ..frame.clone() }, 9, 3, &Wheels::default());
        assert_eq!(grind.retail.state_category, 400, "a grind sounds");
        assert_eq!(grind.retail.grind_audio_surface, 3, "on the probed material");
        assert_eq!(o.retail.motion_200, 1.0, "hard wheels, as the headless roll");
        assert_eq!(o.retail.wheel_contact_normals, [[0.0, 1.0, 0.0]; 4], "the touching wheels' normals");
        let moving = Frame { audio: SkaterAudio { deck_velocity: [1.0, -2.0, 3.0, 0.0], ..frame.audio.clone() }, ..frame.clone() };
        assert_eq!(observation(&moving, 9, 3, &Wheels::default()).retail.wheel_velocities, [[1.0, -2.0, 3.0]; 4]);
        let flying = Frame {
            category: 2,
            audio: SkaterAudio { num_wheels_in_contact: 0, is_skateboard_airborne: true, ..frame.audio.clone() },
            ..frame.clone()
        };
        let flown = observation(&flying, 9, 3, &Wheels::default());
        assert!(flown.retail.in_known_air && flown.retail.wheel_contact_normals == [[0.0; 3]; 4]);
        assert!(!observation(&Frame { category: 1, ..frame.clone() }, 9, 3, &Wheels::default()).retail.in_known_air);

        // In the air: not grounded, no powerslide.
        let mut air = frame.clone();
        air.audio.is_skateboard_airborne = true;
        air.tricks.active = vec![2];
        let o = observation(&air, 10, 3, &Wheels::default());
        assert!(!o.grounded && !o.powersliding);
    }

    /// A frame rolling at `speed` m/s on flat ground with its nose along `nose`, four wheels down.
    fn rolling(nose: [f32; 3], speed: f32) -> Frame {
        Frame {
            category: 1,
            audio: SkaterAudio {
                deck_velocity: [nose[0] * speed, nose[1] * speed, nose[2] * speed, 0.0],
                deck_nose: [nose[0], nose[1], nose[2], 0.0],
                deck_up: [0.0, 1.0, 0.0, 0.0],
                num_wheels_in_contact: 4,
                skateboard_surface_speed: speed,
                ..SkaterAudio::default()
            },
            ..Frame::default()
        }
    }

    /// The skid voice's slip for `retail`, as the bridge computes it (`audio_state.rs` `Conditioner::slip`, with the
    /// vault's offset -0.75 and divisor 45).
    fn slip(retail: &RetailAudioInputs) -> f32 {
        let lateral: f32 = (0..3).map(|i| retail.linear_velocity[i] * retail.deck_rows[0][i]).sum();
        ((lateral.abs() + 0.75) / 45.0).min(1.0)
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn a_straight_roll_is_not_a_slide() {
        // User 10-22: with identity rows a roll along world X read as a slide (slip 0.15 at 6 m/s), louder with every
        // push. A straight roll is Skate 3's rest value, 0.75 / 45, whatever the heading.
        let s = 0.5f32.sqrt();
        for nose in [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-s, 0.0, s]] {
            let r = observation(&rolling(nose, 6.0), 1, 3, &Wheels::default()).retail;
            assert!((slip(&r) - 0.75 / 45.0).abs() < 1e-4, "nose {nose:?}: slip {}", slip(&r));
            assert!(near(r.deck_forward, nose), "nose {nose:?}: forward {:?}", r.deck_forward);
            assert!(near(r.effective_deck_up, [0.0, 1.0, 0.0]));
        }
        // Heading +x: the engine's rows for heading 90 degrees, right = [cos, 0, -sin].
        let r = observation(&rolling([1.0, 0.0, 0.0], 6.0), 1, 3, &Wheels::default()).retail;
        assert_eq!(r.deck_rows, [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
        // Sliding sideways still is a slide: 6 m/s along the right axis.
        let mut sideways = rolling([1.0, 0.0, 0.0], 6.0);
        sideways.audio.deck_velocity = [0.0, 0.0, 6.0, 0.0];
        let slide = slip(&observation(&sideways, 1, 3, &Wheels::default()).retail);
        assert!((slide - 6.75 / 45.0).abs() < 1e-4, "{slide}");
        // A frame with no axes keeps the record's defaults.
        let none = observation(&Frame::default(), 1, 3, &Wheels::default()).retail;
        assert_eq!(none.deck_rows, RetailAudioInputs::default().deck_rows);
    }

    #[test]
    fn pushing_braking_and_reverting_reach_the_sound() {
        let base = rolling([0.0, 0.0, 1.0], 3.0).audio;
        let record = |audio: SkaterAudio| {
            observation(&Frame { category: 1, audio, ..Frame::default() }, 1, 3, &Wheels::default()).retail
        };
        let left = record(SkaterAudio { is_physically_pushing: true, is_generally_pushing: true, ..base.clone() });
        assert!(left.state_flags[55 - 52] && left.state_flags[56 - 52] && !left.state_flags[57 - 52], "a left-foot push");
        let right = record(SkaterAudio {
            is_physically_pushing: true,
            is_generally_pushing: true,
            is_generally_pushing_right_foot: true,
            ..base.clone()
        });
        assert!(right.state_flags[55 - 52] && right.state_flags[57 - 52], "a right-foot push");
        let alone = record(SkaterAudio { is_physically_pushing: true, ..base.clone() });
        assert!(!alone.state_flags[55 - 52], "55 alone re-posts the push every tick");
        let brakes = record(SkaterAudio { is_foot_braking: true, is_manual_braking: true, ..base.clone() });
        assert!(brakes.state_flags[52 - 52] && brakes.state_flags[54 - 52]);
        let revert = record(SkaterAudio { is_reverting: true, ..base.clone() });
        assert!(revert.state_flags[66 - 52]);
        assert_eq!((revert.state_category, revert.state), (100, 102), "RevertGround");
        assert_eq!(record(base).state_flags, [false; 36], "a plain roll sets no flag");
    }

    /// User 10-22: no walking, bail or wheel spin sounds under Skate 2 physics.
    #[test]
    fn walking_bails_and_manuals_reach_the_sound() {
        let wheels = Wheels { body: 0x0001_B084, ..Wheels::default() };
        let record = |category: u8, audio: SkaterAudio| {
            observation(&Frame { category, audio, ..Frame::default() }, 1, 3, &wheels).retail
        };
        // Walking with the board in the hand: the feet, their speeds and surface, and the hold.
        let walk = record(6, SkaterAudio {
            left_foot_down: true,
            is_holding_skateboard: true,
            foot_left_velocity: [1.0, 2.0, 3.0, 0.0],
            foot_right_velocity_deck: [4.0, 5.0, 6.0, 0.0],
            ..SkaterAudio::default()
        });
        assert_eq!((walk.state_category, walk.offboard_feet, walk.offboard_311), (500, [true, false], true));
        assert_eq!(walk.foot_world_velocity, [[1.0, 2.0, 3.0], [0.0; 3]]);
        assert_eq!(walk.foot_local_velocity, [[0.0; 3], [4.0, 5.0, 6.0]]);
        assert_eq!(walk.offboard_surface, 0x0001_B084, "the engine's ground under the skater");
        // An air footplant.
        let plant = record(2, SkaterAudio { is_foot_planting: true, is_right_foot_planting: true, ..SkaterAudio::default() });
        assert_eq!((plant.footplant, plant.footplant_448, plant.footplant_surface), ([false, true], true, 0x0001_B084));
        // A bail: the wipeout flag, the body's hits through the conditioner's scale, the spins and limb speeds.
        let mut force = [0.0; 8];
        force[2] = 0.5;
        force[5] = 1.0;
        let bail = record(4, SkaterAudio {
            is_wiping_out: true,
            body_contact_force: force,
            body_slide_speed: [3.0; 8],
            is_face_contact: true,
            torso_angular_velocity: [0.0, 7.0, 0.0, 0.0],
            right_foot_angular_velocity: [0.0, 8.0, 0.0, 0.0],
            left_foot_angular_velocity: [0.0, 9.0, 0.0, 0.0],
            average_limb_speed: [1.0, 2.0, 3.0, 4.0],
            ..SkaterAudio::default()
        });
        assert!(bail.state_flags[59 - 52], "the bail voices gate on 59");
        let body = &bail.body_contacts;
        assert_eq!(body.contact, [false, false, true, false, false, true, false, false]);
        let finished: Vec<f32> = body.weighted_change.iter().map(|w| (w * BODY_IMPACT_SCALE).clamp(0.0, 1.0)).collect();
        assert_eq!(finished, [0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0], "the conditioner gets the port's impacts back");
        assert_eq!((body.slide[2], body.slide[0]), (3.0, 0.0), "slides only where a region touches");
        assert_eq!((body.material[2], body.material[0]), (0x0001_B084 & 0x7f, 0));
        assert_eq!(body.specific_current, [false, true]);
        assert_eq!(bail.ragdoll_spin, [[0.0, 7.0, 0.0], [0.0, 8.0, 0.0], [0.0, 9.0, 0.0]]);
        assert_eq!(bail.limb_speeds, [1.0, 2.0, 3.0, 4.0]);
        assert!(record(4, SkaterAudio { is_wipeout_over: true, ..SkaterAudio::default() }).skeleton_599);
        // A manual spins the wheels (balance, 60).
        assert!(record(1, SkaterAudio { is_manual: true, ..SkaterAudio::default() }).state_flags[60 - 52]);
        // The trick being scored: a kickflip (96) in the air, none on a plain roll or for an id past the table.
        let tricked = |active: Vec<u16>| {
            let frame = Frame { category: 2, tricks: Tricks { active, ..Tricks::default() }, ..Frame::default() };
            observation(&frame, 1, 3, &wheels).retail.scorable_id
        };
        assert_eq!((tricked(vec![96]), tricked(vec![]), tricked(vec![400])), (96, -1, -1));
    }

    #[test]
    fn the_wheels_are_the_engines_probe() {
        let mut frame = rolling([1.0, 0.0, 0.0], 4.0);
        frame.audio.num_wheels_in_contact = 3;
        let wheels = Wheels { surfaces: [5, 6, 7, 8], seams: [1, 2, 3, 4], hits: [true, false, true, true], body: 0 };
        let r = observation(&frame, 1, 9, &wheels).retail;
        assert_eq!(r.wheel_contacts, [true, false, true, true]);
        assert_eq!(r.wheel_audio_surfaces, [5, 0, 7, 8]);
        assert_eq!(r.wheel_seam_patterns, [1, 0, 3, 4]);
        assert_eq!(r.wheel_count, 3);
        // In the air no wheel touches, whatever the probe found below.
        let mut flying = frame.clone();
        flying.category = 2;
        flying.audio.num_wheels_in_contact = 0;
        let r = observation(&flying, 1, 9, &wheels).retail;
        assert_eq!((r.wheel_contacts, r.wheel_seam_patterns), ([false; 4], [0; 4]));
        // The board's other parts, its tilt, the feet and its turn.
        frame.audio.is_front_truck_in_contact = true;
        frame.audio.is_deck_in_contact = true;
        frame.audio.deck_tilt = 0.25;
        frame.audio.is_left_foot_on_deck = true;
        frame.audio.is_right_foot_on_deck = true;
        frame.audio.deck_local_angular_speed = [0.0, 3.0, 2.0, 0.0];
        let r = observation(&frame, 1, 9, &wheels).retail;
        assert_eq!((r.truck_contacts, r.deck_contact), ([true, false], true));
        assert_eq!(r.part_audio_surfaces, [9, 0, 9]);
        assert_eq!(r.deck_tilt, 0.25);
        assert_eq!(r.feet_in_deck_box, [true, true]);
        // Nose +x: 3 about the deck's up is 3 about the world's; 2 about its forward is 2 about world x.
        assert_eq!(r.angular_velocity, [2.0, 3.0, 0.0]);
    }

    #[test]
    fn audio_publishes_once_per_host_tick() {
        let mut published = None;
        assert_eq!(due(None, &mut published), None, "no switch: the engine publishes");
        let (to_host, _from_engine) = channel();
        let (_to_engine, from_host) = channel();
        let mut skate2 = Skate2Physics::with(Box::new(|_: &Path, _: &Boot| Err(Refused::Busy)), Box::new(Instant::now));
        assert_eq!(due(Some(&skate2), &mut published), None, "off: the engine publishes");
        skate2.state = HostState::Driving(Link::from_channels(to_host, from_host));
        skate2.last_good = Some(Box::new(Frame { tick: 7, ..Frame::default() }));
        assert_eq!(due(Some(&skate2), &mut published).map(|f| f.map(|f| f.tick)), Some(Some(7)));
        assert_eq!(due(Some(&skate2), &mut published), Some(None), "the same host tick: nothing more");
        skate2.last_good = Some(Box::new(Frame { tick: 8, ..Frame::default() }));
        assert_eq!(due(Some(&skate2), &mut published).map(|f| f.map(|f| f.tick)), Some(Some(8)));
    }
}
