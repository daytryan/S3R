//! The game mode word and when the career runs (freeplay 1.1, spec 4).

use std::path::Path;

use bevy::prelude::{Message, Resource};

use super::core::Place;
use super::data::CareerData;
use crate::map_transition::Requester;

/// The word at 0x830B7AE8 (freeplay 1.1).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GameMode {
    NoProfileFreeSkate = 0,
    #[default]
    SinglePlayer = 1,
    PartyPlay = 2,
    ProfileFreeSkate = 3,
    OnlineFreeSkate = 4,
    OnlineChallenge = 5,
    Tutorial = 6,
    DebugFe = 7,
}

/// The career runs only with its gate on, in single player, offline, outside Party Play, on a career world.
#[must_use]
pub fn career_runs(gate_on: bool, mode: GameMode, online: bool, party: bool, world: Option<&str>) -> bool {
    gate_on && mode == GameMode::SinglePlayer && !online && !party && world.is_some()
}

/// The career world an engine map is, by file name. Several worlds share `StartPark.skate`; the shortest name (the
/// plain `dist_startpark`) answers, and the career's own loads carry their world (Task 21).
#[must_use]
pub fn world_of(d: &CareerData, map: &str) -> Option<String> {
    let name = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().to_ascii_lowercase());
    let wanted = name(map)?;
    d.worlds
        .iter()
        .filter(|(_, m)| m.as_deref().and_then(name).as_ref() == Some(&wanted))
        .map(|(w, _)| w)
        .min_by_key(|w| w.len())
        .cloned()
}

/// Written whenever `career_runs` turns false; the glue answers with `Core::drop_pending` and hides markers/prompts.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeExit;

/// A finished load is an arrival when the player or the career asked for it (`Not retail (user 10-20)` for the map
/// library: retail has no map library). The startup map is where the save already says the player is.
#[must_use]
pub fn arrival(req: Requester, d: &CareerData, map: &str, location: Option<&str>) -> Option<Place> {
    match req {
        Requester::MapLibrary | Requester::Career => {
            Some(Place { world: world_of(d, map), map: map.to_owned(), location: location.map(str::to_owned) })
        }
        Requester::Startup => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> CareerData {
        let mut d = CareerData::default();
        for (w, m) in [
            ("dist_university", Some("maps/University.skate")),
            ("dist_startpark", Some("maps/StartPark.skate")),
            ("dist_startpark_progression", Some("maps/StartPark.skate")),
            ("dist_dannywaydlc", None),
        ] {
            d.worlds.insert(w.into(), m.map(str::to_owned));
        }
        d
    }

    #[test]
    fn the_career_runs_only_on_a_career_world_in_single_player() {
        let d = data();
        let univ = world_of(&d, "C:/game/assets/maps/University.skate");
        assert_eq!(univ.as_deref(), Some("dist_university"));
        assert!(career_runs(true, GameMode::SinglePlayer, false, false, univ.as_deref()));
        let s2 = world_of(&d, "C:/game/assets/maps/Skate_2_New_San_Vanelona.skate");
        assert_eq!(s2, None);
        assert!(!career_runs(true, GameMode::SinglePlayer, false, false, s2.as_deref()));
        assert!(!career_runs(true, GameMode::SinglePlayer, true, false, univ.as_deref()), "online");
        assert!(!career_runs(true, GameMode::SinglePlayer, false, true, univ.as_deref()), "Party Play");
        assert!(!career_runs(true, GameMode::ProfileFreeSkate, false, false, univ.as_deref()), "Free Play");
        assert!(!career_runs(false, GameMode::SinglePlayer, false, false, univ.as_deref()), "gate off");
        assert_eq!(world_of(&d, "maps/startpark.skate").as_deref(), Some("dist_startpark"));
    }

    #[test]
    fn a_map_library_load_writes_place_with_no_locator() {
        let d = data();
        let place = arrival(Requester::MapLibrary, &d, "maps/University.skate", None).unwrap();
        assert_eq!(place.world.as_deref(), Some("dist_university"));
        assert_eq!(place.map, "maps/University.skate");
        assert_eq!(place.location, None);
        let place = arrival(Requester::Career, &d, "maps/University.skate", Some("ftl_univ_02_challengelocator_01")).unwrap();
        assert_eq!(place.location.as_deref(), Some("ftl_univ_02_challengelocator_01"));
    }

    #[test]
    fn the_startup_load_leaves_place_alone() {
        assert_eq!(arrival(Requester::Startup, &data(), "maps/University.skate", None), None);
    }

    #[test]
    fn the_default_mode_is_single_player() {
        assert_eq!(GameMode::default(), GameMode::SinglePlayer);
        assert_eq!(GameMode::DebugFe as u32, 7);
    }
}
